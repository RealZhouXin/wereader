//! SQLite metadata and account-scoped file storage. All I/O runs on one worker.
mod files;
mod library;
mod migrations;
pub mod models;
mod reader;
mod session;
mod sync;
#[cfg(test)]
mod tests;

use crate::weread::{Error, Result};
pub use models::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::{
    fs,
    path::PathBuf,
    sync::{mpsc, Arc},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;

type Work = Box<dyn FnOnce(&mut Result<Database>) + Send>;
struct Worker {
    sender: Option<mpsc::Sender<Work>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[derive(Clone)]
pub struct Store {
    worker: Arc<Worker>,
}
struct Database {
    connection: Connection,
    root: PathBuf,
    _lock: fs::File,
}

impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::new("Local database operation failed.")
    }
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::new("Local file operation failed.")
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::new("Invalid stored data format.")
    }
}
pub(crate) fn now() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|v| i64::try_from(v.as_millis()).ok())
        .ok_or_else(|| Error::new("Set the device clock before using storage."))
}
pub fn new_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}
fn encode<T: serde::Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string(v)?)
}
fn decode<T: serde::de::DeserializeOwned>(v: &str) -> Result<T> {
    Ok(serde_json::from_str(v)?)
}

impl Store {
    /// Starts the storage thread; initialization errors are returned by `ready`/operations.
    pub fn new(root: PathBuf) -> Result<Self> {
        let (sender, receiver) = mpsc::channel::<Work>();
        let thread = thread::Builder::new()
            .name("rereader-storage".into())
            .spawn(move || {
                // Keep failures private, but deliver them to every queued caller.
                let mut database = Database::open(root);
                for work in receiver {
                    work(&mut database);
                }
            })?;
        Ok(Self {
            worker: Arc::new(Worker {
                sender: Some(sender),
                thread: Some(thread),
            }),
        })
    }
    pub async fn ready(&self) -> Result<()> {
        self.call(|_| Ok(())).await
    }
    async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Database) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (sender, receiver) = oneshot::channel();
        self.worker
            .sender
            .as_ref()
            .ok_or_else(|| Error::new("Storage has stopped."))?
            .send(Box::new(move |db| {
                let result = match db {
                    Ok(db) => f(db),
                    Err(error) => Err(error.clone()),
                };
                let _ = sender.send(result);
            }))
            .map_err(|_| Error::new("Storage has stopped."))?;
        receiver.await.map_err(|_| {
            Error::new(
                "Cannot initialize local storage. Check the data directory and database version.",
            )
        })?
    }
    pub fn default_path() -> Result<PathBuf> {
        if let Some(path) = std::env::var_os("REREADER_DATA_DIR") {
            return Ok(path.into());
        }
        if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(path).join("rereader"));
        }
        std::env::var_os("HOME")
            .map(|p| PathBuf::from(p).join(".local/share/rereader"))
            .ok_or_else(|| Error::new("Set REREADER_DATA_DIR to a writable directory."))
    }
}

fn private_dir(path: &std::path::Path) -> Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(Error::new("Storage directory must not be a symlink."));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)?;
    Ok(())
}
fn guard(tx: &Transaction<'_>, identity: &Identity) -> Result<()> {
    let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions s JOIN accounts a ON a.account_id=s.account_id WHERE s.account_id=?1 AND a.auth_epoch=?2)", params![identity.account_id, identity.auth_epoch], |r| r.get(0))?;
    if !valid {
        return Err(Error::new(
            "Account session changed; stale storage operation rejected.",
        ));
    }
    Ok(())
}
