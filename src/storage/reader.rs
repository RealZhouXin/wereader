use super::*;
use serde_json::json;

fn position_validate(p: &Position) -> Result<()> {
    p.locator.validate()?;
    if p.progress_ratio
        .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
    {
        return Err(Error::new("Invalid reading progress."));
    }
    Ok(())
}
fn write_position(
    tx: &Transaction<'_>,
    identity: &Identity,
    p: &Position,
    origin: Origin,
) -> Result<()> {
    position_validate(p)?;
    tx.execute("INSERT INTO reading_positions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(account_id,book_id,origin) DO UPDATE SET chapter_uid=excluded.chapter_uid,content_version=excluded.content_version,locator_json=excluded.locator_json,progress_ratio=excluded.progress_ratio,progress_source=excluded.progress_source,revision=excluded.revision,remote_revision=excluded.remote_revision,remote_updated_at_ms=excluded.remote_updated_at_ms,observed_at_ms=excluded.observed_at_ms", params![identity.account_id,p.book_id,origin.as_str(),p.chapter_uid,p.content_version,encode(&p.locator)?,p.progress_ratio,p.progress_source.as_str(),p.revision,p.remote_revision,p.remote_updated_at_ms,now()?])?;
    Ok(())
}
type ExistingAnnotation = (String, String, i64, Option<String>, Option<String>);
type AnnotationRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
);
fn annotation_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AnnotationRow> {
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
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
    ))
}
const ANNOTATION_COLUMNS: &str = "annotation_id,book_id,kind,remote_id,remote_revision,sync_status,chapter_uid,content_version,locator_json,quote_text,note_text,style_json,revision,deleted_at_ms";
fn load_annotations(
    tx: &Transaction<'_>,
    account: &str,
    book: &str,
    include_deleted: bool,
) -> Result<Vec<Annotation>> {
    let mut stmt = tx.prepare(&format!("SELECT {ANNOTATION_COLUMNS} FROM annotations WHERE account_id=?1 AND book_id=?2 AND (?3 OR deleted_at_ms IS NULL) ORDER BY created_at_ms,annotation_id"))?;
    let mut out = Vec::new();
    for row in stmt.query_map(params![account, book, include_deleted], annotation_row)? {
        let (
            id,
            book_id,
            kind,
            remote_id,
            remote_revision,
            status,
            chapter_uid,
            content_version,
            loc,
            quote_text,
            note_text,
            style,
            revision,
            deleted_at_ms,
        ) = row?;
        let locator: Locator = decode(&loc)?;
        locator.validate()?;
        out.push(Annotation {
            id,
            book_id,
            kind: AnnotationKind::parse(&kind)?,
            remote_id,
            remote_revision,
            sync_status: AnnotationStatus::parse(&status)?,
            chapter_uid,
            content_version,
            locator,
            quote_text,
            note_text,
            style: decode(&style)?,
            revision,
            deleted_at_ms,
        });
    }
    Ok(out)
}
fn write_annotation(tx: &Transaction<'_>, identity: &Identity, a: &Annotation) -> Result<()> {
    a.locator.validate()?;
    if a.id.is_empty() || a.style.schema_version != 1 {
        return Err(Error::new("Invalid annotation."));
    }
    tx.execute("INSERT INTO annotations(account_id,annotation_id,book_id,kind,remote_id,chapter_uid,content_version,locator_json,quote_text,note_text,style_json,revision,sync_status,remote_revision,deleted_at_ms,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?16) ON CONFLICT(account_id,annotation_id) DO UPDATE SET remote_id=excluded.remote_id,chapter_uid=excluded.chapter_uid,content_version=excluded.content_version,locator_json=excluded.locator_json,quote_text=excluded.quote_text,note_text=excluded.note_text,style_json=excluded.style_json,revision=excluded.revision,sync_status=excluded.sync_status,remote_revision=excluded.remote_revision,deleted_at_ms=excluded.deleted_at_ms,updated_at_ms=excluded.updated_at_ms", params![identity.account_id,a.id,a.book_id,a.kind.as_str(),a.remote_id,a.chapter_uid,a.content_version,encode(&a.locator)?,a.quote_text,a.note_text,encode(&a.style)?,a.revision,a.sync_status.as_str(),a.remote_revision,a.deleted_at_ms,now()?])?;
    Ok(())
}
impl Store {
    /// `position.revision` is the expected previous local revision (zero for new).
    /// enqueue=false supports local reading while the wire protocol is unverified.
    pub async fn save_position_and_enqueue(
        &self,
        identity: &Identity,
        mut position: Position,
        intervals: Vec<ReadingInterval>,
        enqueue: bool,
    ) -> Result<Position> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let revision: i64 = tx.query_row("SELECT revision FROM reading_positions WHERE account_id=?1 AND book_id=?2 AND origin='local'", params![identity.account_id,position.book_id], |r| r.get(0)).optional()?.unwrap_or(0);
            if position.revision != revision { return Err(Error::new("Reading position changed; reload before saving.")); }
            position.revision = revision.checked_add(1).ok_or_else(|| Error::new("Revision overflow."))?;
            write_position(&tx,&identity,&position,Origin::Local)?;
            for i in intervals {
                if i.book_id != position.book_id || i.active_ms < 0 || i.ended_at_ms < i.started_at_ms { return Err(Error::new("Invalid reading interval.")); }
                // Never overwrite an interval that may already have been uploaded.
                tx.execute("INSERT INTO reading_intervals(account_id,interval_id,book_id,started_at_ms,ended_at_ms,active_ms) VALUES(?1,?2,?3,?4,?5,?6)", params![identity.account_id,i.id,i.book_id,i.started_at_ms,i.ended_at_ms,i.active_ms])?;
            }
            tx.execute("UPDATE shelf_entries SET last_read_at_ms=?3 WHERE account_id=?1 AND book_id=?2", params![identity.account_id,position.book_id,now()?])?;
            if enqueue {
                let intervals = {
                    let mut stmt = tx.prepare("SELECT interval_id,book_id,started_at_ms,ended_at_ms,active_ms FROM reading_intervals WHERE account_id=?1 AND book_id=?2 AND job_id IS NULL AND acknowledged_at_ms IS NULL ORDER BY started_at_ms,interval_id")?;
                    let result = stmt.query_map(params![identity.account_id,position.book_id], |r| Ok(ReadingInterval { id:r.get(0)?,book_id:r.get(1)?,started_at_ms:r.get(2)?,ended_at_ms:r.get(3)?,active_ms:r.get(4)? }))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    result
                };
                let payload = json!({"schema_version":1,"position":position,"intervals":intervals});
                let job = sync::enqueue(&tx,&identity,&position.book_id,JobKind::Progress,&position.book_id,position.revision,&payload)?;
                tx.execute("UPDATE reading_intervals SET job_id=?3 WHERE account_id=?1 AND book_id=?2 AND job_id IS NULL AND acknowledged_at_ms IS NULL", params![identity.account_id,position.book_id,job])?;
            }
            tx.commit()?;
            Ok(position)
        }).await
    }
    pub async fn load_position(
        &self,
        identity: &Identity,
        book_id: String,
        origin: Origin,
    ) -> Result<Option<Position>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let row = tx.query_row("SELECT chapter_uid,content_version,locator_json,progress_ratio,progress_source,revision,remote_revision,remote_updated_at_ms FROM reading_positions WHERE account_id=?1 AND book_id=?2 AND origin=?3", params![identity.account_id,book_id,origin.as_str()], |r| Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<f64>>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<i64>>(7)?))).optional()?;
            row.map(|(chapter_uid,content_version,locator,progress_ratio,source,revision,remote_revision,remote_updated_at_ms)| {
                let p = Position { book_id,chapter_uid,content_version,locator:decode(&locator)?,progress_ratio,progress_source:ProgressSource::parse(&source)?,revision,remote_revision,remote_updated_at_ms };
                position_validate(&p)?;
                Ok(p)
            }).transpose()
        }).await
    }
    pub async fn save_remote_position(
        &self,
        identity: &Identity,
        position: Position,
        cursor: Option<String>,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx, &identity)?;
            write_position(&tx, &identity, &position, Origin::Remote)?;
            library::cursor(
                &tx,
                &identity,
                SyncScope::Progress,
                &position.book_id,
                cursor.as_deref(),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn load_annotations(
        &self,
        identity: &Identity,
        book_id: String,
        include_deleted: bool,
    ) -> Result<Vec<Annotation>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx, &identity)?;
            load_annotations(&tx, &identity.account_id, &book_id, include_deleted)
        })
        .await
    }
    /// Annotation revision is optimistic concurrency, including deletion tombstones.
    pub async fn save_annotation_and_enqueue(
        &self,
        identity: &Identity,
        mut annotation: Annotation,
        enqueue: bool,
    ) -> Result<Annotation> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let state: Option<(String,Option<String>)>=tx.query_row("SELECT sync_status,remote_revision FROM annotations WHERE account_id=?1 AND annotation_id=?2",params![identity.account_id,annotation.id],|r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((status,remote_revision))=state {
                if status=="conflict" { return Err(Error::new("Resolve the annotation conflict before editing.")); }
                annotation.remote_revision=remote_revision;
            }
            let old: Option<(i64,String,String,Option<String>)> = tx.query_row("SELECT revision,book_id,kind,remote_id FROM annotations WHERE account_id=?1 AND annotation_id=?2", params![identity.account_id,annotation.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let expected = old.as_ref().map_or(0,|v| v.0);
            if annotation.revision != expected || old.as_ref().is_some_and(|v| v.1!=annotation.book_id || v.2!=annotation.kind.as_str() || v.3!=annotation.remote_id) { return Err(Error::new("Annotation identity or revision changed.")); }
            annotation.revision = expected.checked_add(1).ok_or_else(|| Error::new("Revision overflow."))?;
            annotation.sync_status = if enqueue { AnnotationStatus::Pending } else { AnnotationStatus::Local };
            write_annotation(&tx,&identity,&annotation)?;
            if enqueue {
                let kind = if annotation.deleted_at_ms.is_some() { JobKind::AnnotationDelete } else { JobKind::AnnotationPut };
                sync::enqueue(&tx,&identity,&annotation.book_id,kind,&annotation.id,annotation.revision,&json!({"schema_version":1,"annotation":annotation}))?;
            }
            tx.commit()?;
            Ok(annotation)
        }).await
    }
    /// Remote tombstones are represented by deleted_at_ms. Dirty local edits survive.
    pub async fn apply_remote_annotations(
        &self,
        identity: &Identity,
        book_id: String,
        annotations: Vec<Annotation>,
        cursor: Option<String>,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            for mut remote in annotations {
                remote.locator.validate()?;
                if remote.book_id != book_id || remote.remote_id.as_ref().is_none_or(|s| s.is_empty()) { return Err(Error::new("Invalid remote annotation identity.")); }
                let existing: Option<ExistingAnnotation> = tx.query_row("SELECT annotation_id,sync_status,revision,remote_revision,remote_snapshot_json FROM annotations WHERE account_id=?1 AND book_id=?2 AND kind=?3 AND remote_id=?4", params![identity.account_id,book_id,remote.kind.as_str(),remote.remote_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
                if let Some((id,status,revision,remote_revision,baseline)) = existing {
                    remote.id = id;
                    if status != "synced" {
                        // An unchanged remote base is not a conflict with a local edit.
                        if remote.remote_revision.is_some() && remote.remote_revision == remote_revision { continue; }
                        if let Some(base) = baseline {
                            let mut base: Annotation = decode(&base)?;
                            let mut candidate = remote.clone();
                            base.revision=0; candidate.revision=0;
                            base.sync_status=AnnotationStatus::Synced; candidate.sync_status=AnnotationStatus::Synced;
                            if base == candidate { continue; }
                        }
                        tx.execute("UPDATE annotations SET sync_status='conflict',remote_snapshot_json=?3 WHERE account_id=?1 AND annotation_id=?2", params![identity.account_id,remote.id,encode(&remote)?])?;
                        tx.execute("UPDATE sync_jobs SET status='blocked',last_error_code='conflict' WHERE account_id=?1 AND entity_id=?2 AND kind IN ('annotation_put','annotation_delete') AND status IN ('pending','retry')", params![identity.account_id,remote.id])?;
                        continue;
                    }
                    remote.revision = revision.checked_add(1).ok_or_else(|| Error::new("Revision overflow."))?;
                } else { remote.id = new_id(); remote.revision = 1; }
                remote.sync_status = AnnotationStatus::Synced;
                write_annotation(&tx,&identity,&remote)?;
                tx.execute("UPDATE annotations SET remote_snapshot_json=?3 WHERE account_id=?1 AND annotation_id=?2", params![identity.account_id,remote.id,encode(&remote)?])?;
            }
            library::cursor(&tx,&identity,SyncScope::Annotations,&book_id,cursor.as_deref())?;
            tx.commit()?;
            Ok(())
        }).await
    }
    pub async fn load_annotation_conflict(
        &self,
        identity: &Identity,
        annotation_id: String,
    ) -> Result<Option<Annotation>> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let snapshot: Option<String> = tx.query_row("SELECT remote_snapshot_json FROM annotations WHERE account_id=?1 AND annotation_id=?2 AND sync_status='conflict'", params![identity.account_id,annotation_id], |r| r.get(0)).optional()?.flatten();
            snapshot.as_deref().map(decode).transpose()
        }).await
    }
    /// Explicitly resolve a retained conflict using the remote version or current
    /// local edits. In-flight/uncertain writes must be reconciled first.
    pub async fn resolve_annotation_conflict(
        &self,
        identity: &Identity,
        annotation_id: String,
        expected_revision: i64,
        use_remote: bool,
        enqueue: bool,
    ) -> Result<Annotation> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx=db.connection.transaction()?;
            guard(&tx,&identity)?;
            let (book, snapshot): (String,String)=tx.query_row("SELECT book_id,remote_snapshot_json FROM annotations WHERE account_id=?1 AND annotation_id=?2 AND revision=?3 AND sync_status='conflict'", params![identity.account_id,annotation_id,expected_revision], |r| Ok((r.get(0)?,r.get(1)?)))?;
            let uncertain: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_jobs WHERE account_id=?1 AND entity_id=?2 AND kind IN ('annotation_put','annotation_delete') AND status IN ('in_flight','uncertain'))", params![identity.account_id,annotation_id], |r| r.get(0))?;
            if uncertain { return Err(Error::new("Reconcile the outstanding annotation write before resolving its conflict.")); }
            let remote: Annotation=decode(&snapshot)?;
            let mut chosen=if use_remote { remote.clone() } else {
                let mut local=load_annotations(&tx,&identity.account_id,&book,true)?.into_iter().find(|a| a.id==annotation_id).ok_or_else(|| Error::new("Annotation not found."))?;
                local.remote_revision=remote.remote_revision.clone();
                local
            };
            chosen.revision=expected_revision.checked_add(1).ok_or_else(|| Error::new("Revision overflow."))?;
            chosen.sync_status=if use_remote { AnnotationStatus::Synced } else if enqueue { AnnotationStatus::Pending } else { AnnotationStatus::Local };
            tx.execute("UPDATE sync_jobs SET status='done',last_error_code='conflict_resolved' WHERE account_id=?1 AND entity_id=?2 AND kind IN ('annotation_put','annotation_delete') AND status IN ('pending','retry','blocked')",params![identity.account_id,annotation_id])?;
            write_annotation(&tx,&identity,&chosen)?;
            if !use_remote && enqueue {
                let kind=if chosen.deleted_at_ms.is_some() { JobKind::AnnotationDelete } else { JobKind::AnnotationPut };
                sync::enqueue(&tx,&identity,&chosen.book_id,kind,&chosen.id,chosen.revision,&json!({"schema_version":1,"annotation":chosen}))?;
            }
            tx.commit()?;
            Ok(chosen)
        }).await
    }
    pub async fn save_settings(
        &self,
        identity: &Identity,
        book_id: Option<String>,
        settings: ReaderSettings,
    ) -> Result<()> {
        let identity = identity.clone();
        self.call(move |db| {
            settings.validate()?;
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            tx.execute("INSERT INTO reader_settings VALUES(?1,?2,?3,?4) ON CONFLICT(account_id,book_id) DO UPDATE SET settings_json=excluded.settings_json,updated_at_ms=excluded.updated_at_ms", params![identity.account_id,book_id.unwrap_or_default(),encode(&settings)?,now()?])?;
            tx.commit()?;
            Ok(())
        }).await
    }
    pub async fn load_settings(
        &self,
        identity: &Identity,
        book_id: Option<String>,
    ) -> Result<ReaderSettings> {
        let identity = identity.clone();
        self.call(move |db| {
            let tx = db.connection.transaction()?;
            guard(&tx,&identity)?;
            let mut out = ReaderSettings { schema_version:1,..Default::default() };
            for id in [String::new(), book_id.unwrap_or_default()] {
                let row: Option<String> = tx.query_row("SELECT settings_json FROM reader_settings WHERE account_id=?1 AND book_id=?2", params![identity.account_id,id], |r| r.get(0)).optional()?;
                if let Some(row) = row { let s: ReaderSettings = decode(&row)?; s.validate()?;
                    if s.font_family.is_some() { out.font_family=s.font_family; }
                    if s.font_size.is_some() { out.font_size=s.font_size; }
                    if s.line_height.is_some() { out.line_height=s.line_height; }
                    if s.margin.is_some() { out.margin=s.margin; }
                    if s.theme.is_some() { out.theme=s.theme; }
                }
            }
            Ok(out)
        }).await
    }
}
