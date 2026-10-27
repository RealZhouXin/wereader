use super::*;
use crate::weread::Book;

pub(super) fn cursor(
    tx: &Transaction<'_>,
    identity: &Identity,
    scope: SyncScope,
    resource: &str,
    value: Option<&str>,
) -> Result<()> {
    let time = now()?;
    tx.execute("INSERT INTO sync_cursors(account_id,scope,resource_id,cursor,last_success_at_ms,last_attempt_at_ms) VALUES(?1,?2,?3,?4,?5,?5) ON CONFLICT(account_id,scope,resource_id) DO UPDATE SET cursor=excluded.cursor,last_success_at_ms=excluded.last_success_at_ms,last_attempt_at_ms=excluded.last_attempt_at_ms,last_error_code=NULL", params![identity.account_id,scope.as_str(),resource,value,time])?;
    Ok(())
}
impl Store {
    pub async fn load_shelf(&self, identity: &Identity) -> Result<Vec<Book>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let mut statement = tx.prepare("SELECT b.book_id,b.title,b.author,b.cover_url,s.finished FROM shelf_entries s JOIN books b USING(account_id,book_id) WHERE s.account_id=?1 ORDER BY s.sort_order,s.book_id")?;
            let result = Ok(statement.query_map([identity.account_id], |r| Ok(Book { id:r.get(0)?,title:r.get(1)?,author:r.get(2)?,cover:r.get(3)?,finished:r.get(4)? }))?.collect::<rusqlite::Result<Vec<_>>>()?);
            result
        }).await
    }
    /// Only call this for a complete shelf snapshot, never for an incremental response.
    pub async fn apply_shelf_snapshot(
        &self,
        identity: &Identity,
        books: Vec<Book>,
        sync_key: Option<String>,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let time = now()?;
            // Preserve existing relationship timestamps when replacing order/finished.
            tx.execute("UPDATE shelf_entries SET sort_order=-1 WHERE account_id=?1", [&identity.account_id])?;
            let mut seen = std::collections::HashSet::new();
            for (order,b) in books.iter().enumerate() {
                if !seen.insert(&b.id) { return Err(Error::new("Duplicate book in shelf snapshot.")); }
                tx.execute("INSERT INTO books(account_id,book_id,title,author,cover_url,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(account_id,book_id) DO UPDATE SET title=excluded.title,author=excluded.author,cover_url=excluded.cover_url,updated_at_ms=excluded.updated_at_ms", params![identity.account_id,b.id,b.title,b.author,b.cover,time])?;
                tx.execute("INSERT INTO shelf_entries(account_id,book_id,sort_order,finished,updated_at_ms) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(account_id,book_id) DO UPDATE SET sort_order=excluded.sort_order,finished=excluded.finished,updated_at_ms=excluded.updated_at_ms", params![identity.account_id,b.id,order as i64,b.finished,time])?;
            }
            tx.execute("DELETE FROM shelf_entries WHERE account_id=?1 AND sort_order=-1", [&identity.account_id])?;
            cursor(&tx,&identity,SyncScope::Shelf,"",sync_key.as_deref())?;
            tx.commit()?;
            Ok(())
        }).await
    }
    pub async fn save_book_details(
        &self,
        identity: &Identity,
        book_id: String,
        description: String,
        content_version: Option<String>,
        format: BookFormat,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            if tx.execute("UPDATE books SET description=?3,content_version=?4,format=?5,updated_at_ms=?6 WHERE account_id=?1 AND book_id=?2", params![identity.account_id,book_id,description,content_version,format.as_str(),now()?])? != 1 {
                return Err(Error::new("Book not found."));
            }
            tx.commit()?;
            Ok(())
        }).await
    }
    pub async fn load_book_details(
        &self,
        identity: &Identity,
        book_id: String,
    ) -> Result<Option<BookDetails>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx=db.connection.transaction()?;
            guard(&tx,&identity)?;
            let row=tx.query_row("SELECT b.title,b.author,b.cover_url,COALESCE(s.finished,0),b.description,b.content_version,b.format FROM books b LEFT JOIN shelf_entries s USING(account_id,book_id) WHERE b.account_id=?1 AND b.book_id=?2",params![identity.account_id,book_id],|r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,String>(6)?))).optional()?;
            row.map(|(title,author,cover,finished,description,content_version,format)| Ok(BookDetails { book:Book { id:book_id,title,author,cover,finished },description,content_version,format:BookFormat::parse(&format)? })).transpose()
        }).await
    }
    pub async fn apply_chapter_delta(
        &self,
        identity: &Identity,
        book_id: String,
        updated: Vec<Chapter>,
        deleted: Vec<String>,
        full_snapshot: bool,
        sync_key: Option<String>,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            if full_snapshot { tx.execute("DELETE FROM chapters WHERE account_id=?1 AND book_id=?2", params![identity.account_id,book_id])?; }
            for uid in deleted { tx.execute("DELETE FROM chapters WHERE account_id=?1 AND book_id=?2 AND chapter_uid=?3", params![identity.account_id,book_id,uid])?; }
            for ch in updated {
                tx.execute("INSERT INTO chapters VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(account_id,book_id,chapter_uid) DO UPDATE SET ordinal=excluded.ordinal,title=excluded.title,parent_uid=excluded.parent_uid,readable=excluded.readable,content_version=excluded.content_version,updated_at_ms=excluded.updated_at_ms", params![identity.account_id,book_id,ch.uid,ch.ordinal,ch.title,ch.parent_uid,ch.readable,ch.content_version,now()?])?;
            }
            cursor(&tx,&identity,SyncScope::Chapters,&book_id,sync_key.as_deref())?;
            tx.commit()?;
            Ok(())
        }).await
    }
    pub async fn load_chapters(
        &self,
        identity: &Identity,
        book_id: String,
    ) -> Result<Vec<Chapter>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let mut stmt = tx.prepare("SELECT chapter_uid,ordinal,title,parent_uid,readable,content_version FROM chapters WHERE account_id=?1 AND book_id=?2 ORDER BY ordinal,chapter_uid")?;
            let result = Ok(stmt.query_map(params![identity.account_id,book_id], |r| Ok(Chapter { uid:r.get(0)?,ordinal:r.get(1)?,title:r.get(2)?,parent_uid:r.get(3)?,readable:r.get(4)?,content_version:r.get(5)? }))?.collect::<rusqlite::Result<Vec<_>>>()?);
            result
        }).await
    }
    /// Explicit destructive book removal; shelf removal is handled by snapshots.
    pub async fn delete_book(&self, identity: &Identity, book_id: String) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            tx.execute("DELETE FROM reading_intervals WHERE account_id=?1 AND book_id=?2", params![identity.account_id,book_id])?;
            tx.execute("DELETE FROM sync_cursors WHERE account_id=?1 AND resource_id=?2 AND scope!='shelf'", params![identity.account_id,book_id])?;
            tx.execute("DELETE FROM reader_settings WHERE account_id=?1 AND book_id=?2", params![identity.account_id,book_id])?;
            tx.execute("DELETE FROM books WHERE account_id=?1 AND book_id=?2", params![identity.account_id,book_id])?;
            tx.commit()?;
            db.recover_files()
        }).await
    }
}
