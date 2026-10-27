use super::*;

pub(super) fn enqueue(
    tx: &Transaction<'_>,
    identity: &Identity,
    book: &str,
    kind: JobKind,
    entity: &str,
    revision: i64,
    payload: &serde_json::Value,
) -> Result<String> {
    let id = new_id();
    let time = now()?;
    tx.execute("INSERT INTO sync_jobs(account_id,job_id,book_id,kind,entity_id,entity_revision,payload_json,next_attempt_at_ms,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8,?8)", params![identity.account_id,id,book,kind.as_str(),entity,revision,encode(payload)?,time])?;
    Ok(id)
}
type JobRow = (
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    i64,
    i64,
    Option<i64>,
    Option<String>,
);
fn job_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
    ))
}
fn job(row: JobRow) -> Result<SyncJob> {
    let (
        id,
        book_id,
        kind,
        entity_id,
        entity_revision,
        payload,
        status,
        attempt_count,
        next_attempt_at_ms,
        attempt_auth_epoch,
        last_error,
    ) = row;
    Ok(SyncJob {
        id,
        book_id,
        kind: JobKind::parse(&kind)?,
        entity_id,
        entity_revision,
        payload: decode(&payload)?,
        status: JobStatus::parse(&status)?,
        attempt_count,
        next_attempt_at_ms,
        attempt_auth_epoch,
        last_error,
    })
}
const JOB_COLUMNS: &str =
    "job_id,book_id,kind,entity_id,entity_revision,payload_json,status,attempt_count,next_attempt_at_ms,attempt_auth_epoch,last_error_code";
fn complete(
    tx: &Transaction<'_>,
    identity: &Identity,
    job_id: &str,
    expected_revision: i64,
    remote_id: Option<&str>,
    remote_revision: Option<&str>,
) -> Result<()> {
    let row = tx.query_row(&format!("SELECT {JOB_COLUMNS} FROM sync_jobs WHERE account_id=?1 AND job_id=?2 AND status='in_flight' AND attempt_auth_epoch=?3"), params![identity.account_id,job_id,identity.auth_epoch], job_row).optional()?.ok_or_else(|| Error::new("Sync job is no longer in flight for this session."))?;
    let j = job(row)?;
    if j.entity_revision != expected_revision {
        return Err(Error::new("Sync acknowledgement revision mismatch."));
    }
    if j.kind != JobKind::Progress {
        if j.kind == JobKind::AnnotationPut && remote_id.is_none_or(str::is_empty) {
            return Err(Error::new(
                "Annotation acknowledgement requires a remote identity.",
            ));
        }
        tx.execute("UPDATE annotations SET remote_id=COALESCE(?3,remote_id),remote_revision=COALESCE(?4,remote_revision),sync_status=CASE WHEN revision=?5 AND sync_status!='conflict' THEN 'synced' ELSE sync_status END WHERE account_id=?1 AND annotation_id=?2", params![identity.account_id,j.entity_id,remote_id,remote_revision,j.entity_revision])?;
    }
    if j.kind != JobKind::Progress {
        if let Some(value) = j.payload.get("annotation") {
            let mut accepted: Annotation = serde_json::from_value(value.clone())?;
            if let Some(id) = remote_id {
                accepted.remote_id = Some(id.into());
            }
            if let Some(revision) = remote_revision {
                accepted.remote_revision = Some(revision.into());
            }
            accepted.sync_status = AnnotationStatus::Synced;
            tx.execute("UPDATE annotations SET remote_snapshot_json=?3 WHERE account_id=?1 AND annotation_id=?2 AND sync_status!='conflict'",params![identity.account_id,j.entity_id,encode(&accepted)?])?;
        }
    }
    let time = now()?;
    tx.execute(
        "UPDATE reading_intervals SET acknowledged_at_ms=?3 WHERE account_id=?1 AND job_id=?2",
        params![identity.account_id, job_id, time],
    )?;
    tx.execute("UPDATE sync_jobs SET status='done',last_error_code=NULL,updated_at_ms=?3 WHERE account_id=?1 AND job_id=?2", params![identity.account_id,job_id,time])?;
    Ok(())
}
impl Store {
    /// Caller must enable only job kinds whose wire protocol has been verified.
    pub async fn claim_sync_job(
        &self,
        identity: &Identity,
        time: i64,
        enabled: Vec<JobKind>,
    ) -> Result<Option<SyncJob>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let mut candidates = Vec::new();
            {
                let mut stmt = tx.prepare(&format!("SELECT {JOB_COLUMNS} FROM sync_jobs j WHERE account_id=?1 AND status IN ('pending','retry') AND next_attempt_at_ms<=?2 AND NOT EXISTS(SELECT 1 FROM sync_jobs older WHERE older.account_id=j.account_id AND older.book_id=j.book_id AND older.status!='done' AND older.rowid<j.rowid) ORDER BY rowid"))?;
                for row in stmt.query_map(params![identity.account_id,time],job_row)? { candidates.push(job(row?)?); }
            }
            let Some(mut j) = candidates.into_iter().find(|j| enabled.contains(&j.kind)) else { return Ok(None); };
            tx.execute("UPDATE sync_jobs SET status='in_flight',attempt_count=attempt_count+1,attempt_auth_epoch=?3,updated_at_ms=?4 WHERE account_id=?1 AND job_id=?2", params![identity.account_id,j.id,identity.auth_epoch,time])?;
            j.status=JobStatus::InFlight;
            j.attempt_count+=1;
            j.attempt_auth_epoch=Some(identity.auth_epoch);
            tx.commit()?;
            Ok(Some(j))
        }).await
    }
    pub async fn load_sync_jobs(&self, identity: &Identity) -> Result<Vec<SyncJob>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx, &identity)?;
            let mut stmt = tx.prepare(&format!(
                "SELECT {JOB_COLUMNS} FROM sync_jobs WHERE account_id=?1 ORDER BY rowid"
            ))?;
            let result = stmt
                .query_map([identity.account_id], job_row)?
                .map(|r| job(r?))
                .collect();
            result
        })
        .await
    }
    /// Resolve remote identity assigned by an earlier create ACK without mutating
    /// the frozen job payload. The sender signs this derived payload at send time.
    pub async fn prepare_sync_payload(
        &self,
        identity: &Identity,
        job_id: String,
    ) -> Result<serde_json::Value> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx=db.connection.transaction()?;
            guard(&tx,&identity)?;
            let row=tx.query_row(&format!("SELECT {JOB_COLUMNS} FROM sync_jobs WHERE account_id=?1 AND job_id=?2 AND status='in_flight' AND attempt_auth_epoch=?3"),params![identity.account_id,job_id,identity.auth_epoch],job_row)?;
            let mut j=job(row)?;
            if j.kind!=JobKind::Progress {
                let remote_id: Option<String>=tx.query_row("SELECT remote_id FROM annotations WHERE account_id=?1 AND annotation_id=?2",params![identity.account_id,j.entity_id],|r| r.get(0))?;
                if let Some(id)=remote_id { j.payload["annotation"]["remote_id"]=serde_json::Value::String(id); }
            }
            Ok(j.payload)
        }).await
    }
    pub async fn complete_sync_job(
        &self,
        identity: &Identity,
        job_id: String,
        expected_revision: i64,
        remote_id: Option<String>,
        remote_revision: Option<String>,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx, &identity)?;
            complete(
                &tx,
                &identity,
                &job_id,
                expected_revision,
                remote_id.as_deref(),
                remote_revision.as_deref(),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn record_sync_failure(
        &self,
        identity: &Identity,
        job_id: String,
        failure: FailureClass,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let attempt: i64 = tx.query_row("SELECT attempt_count FROM sync_jobs WHERE account_id=?1 AND job_id=?2 AND status='in_flight' AND attempt_auth_epoch=?3", params![identity.account_id,job_id,identity.auth_epoch], |r| r.get(0))?;
            let status = match failure { FailureClass::Retryable => "retry", FailureClass::Uncertain => "uncertain", _ => "blocked" };
            let time = now()?;
            let delay = (1000_i64 * 2_i64.pow(attempt.clamp(0,12) as u32)).min(3_600_000);
            tx.execute("UPDATE sync_jobs SET status=?3,last_error_code=?4,next_attempt_at_ms=?5,updated_at_ms=?6 WHERE account_id=?1 AND job_id=?2", params![identity.account_id,job_id,status,failure.as_str(),time.saturating_add(delay),time])?;
            tx.commit()?;
            Ok(())
        }).await
    }
    /// Only after querying the remote state: applied=true confirms the old write;
    /// applied=false means the server has proved it did not execute the write.
    pub async fn reconcile_sync_job(
        &self,
        identity: &Identity,
        job_id: String,
        applied: bool,
        remote_id: Option<String>,
        remote_revision: Option<String>,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let revision: i64 = tx.query_row("SELECT entity_revision FROM sync_jobs WHERE account_id=?1 AND job_id=?2 AND status='uncertain'", params![identity.account_id,job_id], |r| r.get(0))?;
            if applied {
                tx.execute("UPDATE sync_jobs SET status='in_flight',attempt_auth_epoch=?3 WHERE account_id=?1 AND job_id=?2", params![identity.account_id,job_id,identity.auth_epoch])?;
                complete(&tx,&identity,&job_id,revision,remote_id.as_deref(),remote_revision.as_deref())?;
            } else {
                tx.execute("UPDATE sync_jobs SET status='retry',next_attempt_at_ms=?3,last_error_code=NULL WHERE account_id=?1 AND job_id=?2", params![identity.account_id,job_id,now()?])?;
            }
            tx.commit()?;
            Ok(())
        }).await
    }
    pub async fn prune_completed_jobs(&self, identity: &Identity) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            tx.execute("DELETE FROM reading_intervals WHERE account_id=?1 AND acknowledged_at_ms IS NOT NULL AND job_id IN (SELECT job_id FROM sync_jobs WHERE account_id=?1 AND status='done')", [&identity.account_id])?;
            tx.execute("DELETE FROM sync_jobs WHERE account_id=?1 AND status='done'", [&identity.account_id])?;
            tx.commit()?;
            Ok(())
        }).await
    }
    pub async fn load_sync_cursor(
        &self,
        identity: &Identity,
        scope: SyncScope,
        resource_id: String,
    ) -> Result<Option<SyncCursor>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            Ok(tx.query_row("SELECT cursor,last_success_at_ms,last_attempt_at_ms,last_error_code FROM sync_cursors WHERE account_id=?1 AND scope=?2 AND resource_id=?3", params![identity.account_id,scope.as_str(),resource_id], |r| Ok(SyncCursor { scope,resource_id:resource_id.clone(),cursor:r.get(0)?,last_success_at_ms:r.get(1)?,last_attempt_at_ms:r.get(2)?,last_error:r.get(3)? })).optional()?)
        }).await
    }
    pub async fn record_pull_failure(
        &self,
        identity: &Identity,
        scope: SyncScope,
        resource_id: String,
        failure: FailureClass,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            tx.execute("INSERT INTO sync_cursors(account_id,scope,resource_id,last_attempt_at_ms,last_error_code) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(account_id,scope,resource_id) DO UPDATE SET last_attempt_at_ms=excluded.last_attempt_at_ms,last_error_code=excluded.last_error_code", params![identity.account_id,scope.as_str(),resource_id,now()?,failure.as_str()])?;
            tx.commit()?;
            Ok(())
        }).await
    }
}
