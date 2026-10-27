use super::*;
use crate::weread::Credentials;

pub(super) fn save(tx: &Transaction<'_>, c: &Credentials, login: bool) -> Result<()> {
    c.validate()?;
    let time = now()?;
    if login {
        tx.execute("UPDATE sync_jobs SET status='uncertain',last_error_code='session_changed' WHERE status='in_flight'", [])?;
        tx.execute("UPDATE accounts SET auth_epoch=auth_epoch+1 WHERE account_id IN (SELECT account_id FROM sessions)", [])?;
        tx.execute("DELETE FROM sessions", [])?;
    }
    tx.execute("INSERT INTO accounts(account_id,auth_epoch,created_at_ms,updated_at_ms) VALUES (?1,1,?2,?2) ON CONFLICT(account_id) DO UPDATE SET auth_epoch=accounts.auth_epoch+?3,updated_at_ms=?2", params![c.vid,time,i64::from(login)])?;
    tx.execute("INSERT INTO sessions VALUES(1,?1,?2,?3,?4,?5) ON CONFLICT(singleton) DO UPDATE SET account_id=excluded.account_id,access_token=excluded.access_token,refresh_token=excluded.refresh_token,device_id=excluded.device_id,updated_at_ms=excluded.updated_at_ms", params![c.vid,c.access_token,c.refresh_token,c.device_id,time])?;
    Ok(())
}
impl Store {
    pub async fn load(&self) -> Result<Option<Credentials>> {
        self.call(|db| Ok(db.connection.query_row("SELECT account_id,access_token,refresh_token,device_id FROM sessions WHERE singleton=1", [], |r| Ok(Credentials { vid:r.get(0)?,access_token:r.get(1)?,refresh_token:r.get(2)?,device_id:r.get(3)? })).optional()?)).await
    }
    /// Establishes a new login generation. Use rotate_session for token refresh.
    pub async fn save(&self, credentials: &Credentials) -> Result<Identity> {
        let credentials = credentials.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            save(&tx, &credentials, true)?;
            let epoch = tx.query_row(
                "SELECT auth_epoch FROM accounts WHERE account_id=?1",
                [&credentials.vid],
                |r| r.get(0),
            )?;
            tx.commit()?;
            Ok(Identity {
                account_id: credentials.vid,
                auth_epoch: epoch,
            })
        })
        .await
    }
    pub async fn identity(&self) -> Result<Option<Identity>> {
        self.call(|db| Ok(db.connection.query_row("SELECT a.account_id,a.auth_epoch FROM accounts a JOIN sessions s ON s.account_id=a.account_id", [], |r| Ok(Identity { account_id:r.get(0)?,auth_epoch:r.get(1)? })).optional()?)).await
    }
    pub async fn rotate_session(
        &self,
        identity: &Identity,
        credentials: &Credentials,
    ) -> Result<()> {
        let (identity, c) = (identity.clone(), credentials.clone());
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx, &identity)?;
            if c.vid != identity.account_id {
                return Err(Error::new("Cannot rotate credentials for another account."));
            }
            save(&tx, &c, false)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn clear(&self) -> Result<()> {
        self.call(|db| {
            let tx = db.connection.transaction()?;
            tx.execute("UPDATE accounts SET auth_epoch=auth_epoch+1 WHERE account_id IN (SELECT account_id FROM sessions)", [])?;
            tx.execute("DELETE FROM sessions", [])?;
            tx.execute("UPDATE sync_jobs SET status='uncertain',last_error_code='signed_out' WHERE status='in_flight'", [])?;
            tx.commit()?;
            Ok(())
        }).await
    }
    pub async fn delete_account(&self, identity: &Identity) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx, &identity)?;
            tx.execute(
                "DELETE FROM accounts WHERE account_id=?1",
                [&identity.account_id],
            )?;
            tx.commit()?;
            db.recover_files()
        })
        .await
    }
}
