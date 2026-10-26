use super::{Credentials, Error, Result};
use std::{fs, io::Write, path::PathBuf};

pub struct Store {
    pub(super) directory: PathBuf,
}
impl Store {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
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
    pub fn load(&self) -> Result<Option<Credentials>> {
        match fs::read(self.directory.join("session.json")) {
            Ok(bytes) => {
                let c: Credentials = serde_json::from_slice(&bytes).map_err(|_| {
                    Error::new("Saved session is damaged. Choose Sign in to replace it.")
                })?;
                c.validate()?;
                Ok(Some(c))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(Error::new("Cannot read saved session.")),
        }
    }
    pub fn save(&self, credentials: &Credentials) -> Result<()> {
        credentials.validate()?;
        fs::create_dir_all(&self.directory)
            .map_err(|_| Error::new("Cannot create session directory."))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::new("Cannot protect session directory."))?;
        }
        let temporary = self.directory.join("session.json.part");
        // A single worker owns this store. Never follow an old temporary symlink.
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::new("Cannot replace temporary session.")),
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options
                .open(&temporary)
                .map_err(|_| Error::new("Cannot save session."))?;
            let bytes = serde_json::to_vec(credentials)
                .map_err(|_| Error::new("Cannot encode session."))?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| Error::new("Cannot write session."))?;
            fs::rename(&temporary, self.directory.join("session.json"))
                .map_err(|_| Error::new("Cannot commit session."))
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
    pub fn clear(&self) -> Result<()> {
        match fs::remove_file(self.directory.join("session.json")) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(Error::new(
                "Cannot remove saved session. Please retry Sign out.",
            )),
        }
    }
}
