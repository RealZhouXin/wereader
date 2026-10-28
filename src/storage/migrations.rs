use super::*;
use std::time::Duration;

impl Database {
    pub(super) fn open(root: PathBuf) -> Result<Self> {
        private_dir(&root)?;
        let root = root.canonicalize()?;
        let lock_path = root.join(".storage.lock");
        if lock_path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(Error::new("Storage lock must not be a symlink."));
        }
        let mut lock_options = fs::OpenOptions::new();
        lock_options
            .read(true)
            .write(true)
            .create(true)
            .truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            lock_options.mode(0o600);
        }
        let lock = lock_options.open(lock_path)?;
        lock.try_lock()
            .map_err(|_| Error::new("Another process is using this storage directory."))?;
        let path = root.join("rereader.sqlite3");
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(Error::new("Database path must not be a symlink."));
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let connection = Connection::open(&path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 3 {
            return Err(Error::new(
                "Database was created by a newer rereader version.",
            ));
        }
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;",
        )?;
        let mut db = Self {
            connection,
            root,
            _lock: lock,
        };
        if version == 0 {
            // Validate legacy credentials before creating schema: invalid data can be
            // discarded by the explicit recovery path below, never partly imported.
            let legacy = db.root.join("session.json");
            let credentials = match fs::read(&legacy) {
                Ok(bytes) => serde_json::from_slice::<crate::weread::Credentials>(&bytes)
                    .ok()
                    .filter(|c| c.validate().is_ok()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };
            // Schema and import are one transaction, including user_version.
            let sql = include_str!("../../docs/storage/001_initial.sql");
            let body = sql
                .split_once("BEGIN IMMEDIATE;")
                .unwrap()
                .1
                .split_once("COMMIT;")
                .unwrap()
                .0;
            let tx = db
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute_batch(body)?;
            if let Some(c) = credentials {
                session::save(&tx, &c, true)?;
            }
            tx.commit()?;
        }
        if version < 2 {
            let tx = db.connection.transaction()?;
            tx.execute_batch(include_str!(
                "../../docs/storage/002_annotation_conflicts.sql"
            ))?;
            tx.commit()?;
        }
        if version < 3 {
            let tx = db.connection.transaction()?;
            tx.execute_batch(include_str!("../../docs/storage/003_shelf_groups.sql"))?;
            tx.commit()?;
        }
        // Once schema exists, never re-import an old token, even after logout.
        match fs::remove_file(db.root.join("session.json")) {
            Ok(()) => {
                fs::File::open(&db.root)?.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {} // retried on next open; existing schema prevents resurrection
        }
        db.connection.execute("UPDATE sync_jobs SET status='uncertain',last_error_code='interrupted' WHERE status='in_flight'", [])?;
        db.recover_files()?;
        Ok(db)
    }
}
