use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::Write,
    path::{Component, Path},
};

fn safe_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::new("Invalid cache path."));
    }
    let first = relative.components().next().unwrap().as_os_str();
    if first != "content" && first != "images" && first != "tmp" {
        return Err(Error::new("Cache path is outside managed directories."));
    }
    let mut path = root.to_owned();
    for part in relative.components() {
        path.push(part.as_os_str());
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(Error::new("Cache path must not contain symlinks."));
        }
    }
    Ok(path)
}
fn scoped_path(root: &Path, account: &str, relative: &Path) -> Result<PathBuf> {
    let parts: Vec<_> = relative.components().collect();
    let account_hash = format!("{:x}", Sha256::digest(account.as_bytes()));
    if parts.len() != 4
        || parts[1].as_os_str() != std::ffi::OsStr::new(&account_hash)
        || (parts[0].as_os_str() != "content" && parts[0].as_os_str() != "images")
    {
        return Err(Error::new("Cache file is outside the account directory."));
    }
    safe_path(root, relative)
}
fn make_parent(root: &Path, relative: &Path) -> Result<()> {
    let mut path = root.to_owned();
    for part in relative
        .parent()
        .ok_or_else(|| Error::new("Invalid cache path."))?
        .components()
    {
        path.push(part.as_os_str());
        private_dir(&path)?;
    }
    Ok(())
}
impl Database {
    pub(super) fn recover_files(&mut self) -> Result<()> {
        let rows = {
            let mut stmt = self
                .connection
                .prepare("SELECT account_id,file_id,relative_path FROM cached_files")?;
            let result = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result
        };
        let mut referenced = HashSet::new();
        for (account, id, relative) in rows {
            if scoped_path(&self.root, &account, Path::new(&relative)).is_ok_and(|p| p.is_file()) {
                referenced.insert(relative);
            } else {
                self.connection.execute(
                    "DELETE FROM cached_files WHERE account_id=?1 AND file_id=?2",
                    params![account, id],
                )?;
            }
        }
        fn sweep(root: &Path, dir: &Path, keep: &HashSet<String>) -> Result<()> {
            if !dir.exists() {
                return Ok(());
            }
            if dir.symlink_metadata()?.file_type().is_symlink() {
                fs::remove_file(dir)?;
                return Ok(());
            }
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    sweep(root, &path, keep)?;
                } else if !keep.contains(
                    path.strip_prefix(root)
                        .map_err(|_| Error::new("Invalid cache root."))?
                        .to_str()
                        .ok_or_else(|| Error::new("Invalid cache filename."))?,
                ) {
                    fs::remove_file(path)?;
                }
            }
            Ok(())
        }
        for managed in ["content", "images", "tmp"] {
            sweep(&self.root, &self.root.join(managed), &referenced)?;
        }
        Ok(())
    }
}
impl Store {
    /// Input must already be validated/decoded by the content or image layer.
    /// File and DB work is serialized, so recovery cannot remove an in-progress file.
    pub async fn commit_cached_file(
        &self,
        identity: &Identity,
        resource: FileResource,
        bytes: Vec<u8>,
    ) -> Result<String> {
        let identity = identity.clone();
        self.call(move |db| {
            if resource.resource_key.is_empty() || resource.variant.is_empty() || resource.media_type.is_empty() || bytes.len() > 256*1024*1024 {
                return Err(Error::new("Invalid cached resource."));
            }
            { let tx = db.connection.transaction()?; guard(&tx,&identity)?; }
            let id = new_id();
            let account = format!("{:x}",Sha256::digest(identity.account_id.as_bytes()));
            let group = match resource.kind { FileKind::Book => "content", FileKind::Chapter => "content", _ => "images" };
            let relative = PathBuf::from(group).join(account).join(resource.kind.as_str()).join(format!("{id}.bin"));
            let temporary_relative = PathBuf::from("tmp").join(format!("{id}.part"));
            make_parent(&db.root,&temporary_relative)?;
            make_parent(&db.root,&relative)?;
            let temporary = safe_path(&db.root,&temporary_relative)?;
            let target = safe_path(&db.root,&relative)?;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            let result = (|| -> Result<Vec<String>> {
                let mut file = options.open(&temporary)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                fs::rename(&temporary,&target)?;
                fs::File::open(target.parent().unwrap())?.sync_all()?;
                fs::File::open(temporary.parent().unwrap())?.sync_all()?;
                let tx = db.connection.transaction()?;
                guard(&tx,&identity)?;
                let old_paths={
                    let mut stmt=tx.prepare("SELECT relative_path FROM cached_files WHERE account_id=?1 AND book_id=?2 AND kind=?3 AND resource_key=?4 AND content_version IS ?5 AND variant=?6 AND chapter_uid IS ?7")?;
                    let rows=stmt.query_map(params![identity.account_id,resource.book_id,resource.kind.as_str(),resource.resource_key,resource.content_version,resource.variant,resource.chapter_uid],|r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                tx.execute("DELETE FROM cached_files WHERE account_id=?1 AND book_id=?2 AND kind=?3 AND resource_key=?4 AND content_version IS ?5 AND variant=?6 AND chapter_uid IS ?7", params![identity.account_id,resource.book_id,resource.kind.as_str(),resource.resource_key,resource.content_version,resource.variant,resource.chapter_uid])?;
                tx.execute("INSERT INTO cached_files(account_id,file_id,book_id,chapter_uid,kind,resource_key,content_version,variant,relative_path,media_type,byte_size,sha256,pinned,created_at_ms,last_accessed_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)", params![identity.account_id,id,resource.book_id,resource.chapter_uid,resource.kind.as_str(),resource.resource_key,resource.content_version,resource.variant,relative.to_str(),resource.media_type,bytes.len() as i64,format!("{:x}",Sha256::digest(&bytes)),resource.pinned,now()?])?;
                tx.commit()?;
                Ok(old_paths)
            })();
            if result.is_err() { let _ = fs::remove_file(&temporary); let _ = fs::remove_file(&target); }
            let old_paths=result?;
            // Avoid scanning the cache tree after every cover download.
            for old in old_paths {
                if let Ok(path)=scoped_path(&db.root,&identity.account_id,Path::new(&old)) { let _=fs::remove_file(path); }
            }
            Ok(id)
        }).await
    }
    pub async fn read_cached_file(
        &self,
        identity: &Identity,
        resource: FileResource,
    ) -> Result<Option<Vec<u8>>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let row: Option<(String,String,i64,String,i64)> = tx.query_row("SELECT file_id,relative_path,byte_size,sha256,last_accessed_at_ms FROM cached_files WHERE account_id=?1 AND book_id=?2 AND kind=?3 AND resource_key=?4 AND content_version IS ?5 AND variant=?6 AND chapter_uid IS ?7 ORDER BY created_at_ms DESC LIMIT 1", params![identity.account_id,resource.book_id,resource.kind.as_str(),resource.resource_key,resource.content_version,resource.variant,resource.chapter_uid], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
            let Some((id,relative,size,hash,last_access)) = row else { return Ok(None); };
            let path = scoped_path(&db.root,&identity.account_id,Path::new(&relative));
            let bytes = path.as_ref().ok().and_then(|p| {
                let metadata = fs::metadata(p).ok()?;
                if metadata.len() > 256*1024*1024 || metadata.len() != size as u64 { return None; }
                fs::read(p).ok().filter(|b| format!("{:x}",Sha256::digest(b)) == hash)
            });
            if bytes.is_none() {
                tx.execute("DELETE FROM cached_files WHERE account_id=?1 AND file_id=?2", params![identity.account_id,id])?;
                tx.commit()?;
                if let Ok(path) = path { let _ = fs::remove_file(path); }
            } else {
                let time = now()?;
                if time.saturating_sub(last_access) >= 60_000 { tx.execute("UPDATE cached_files SET last_accessed_at_ms=?3 WHERE account_id=?1 AND file_id=?2", params![identity.account_id,id,time])?; }
                tx.commit()?;
            }
            Ok(bytes)
        }).await
    }
    /// Evict unpinned files in LRU order until the account is within its byte budget.
    pub async fn trim_cache(&self, identity: &Identity, budget: u64) -> Result<u64> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let mut rows = Vec::new();
            { let mut stmt = tx.prepare("SELECT file_id,byte_size,pinned,relative_path FROM cached_files WHERE account_id=?1 ORDER BY last_accessed_at_ms,file_id")?;
              for row in stmt.query_map([&identity.account_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)? as u64,r.get::<_,bool>(2)?,r.get::<_,String>(3)?)))? { rows.push(row?); } }
            let mut total: u64 = rows.iter().map(|r| r.1).sum();
            let original = total;
            let mut removed=Vec::new();
            for (id,size,pinned,path) in rows { if total <= budget { break; }
                if !pinned { tx.execute("DELETE FROM cached_files WHERE account_id=?1 AND file_id=?2", params![identity.account_id,id])?; total-=size; removed.push(path); } }
            tx.commit()?;
            for relative in removed {
                if let Ok(path)=scoped_path(&db.root,&identity.account_id,Path::new(&relative)) { let _=fs::remove_file(path); }
            }
            Ok(original-total)
        }).await
    }
}
