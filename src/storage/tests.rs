use super::*;
use crate::weread::{
    tests::{credentials, temporary},
    Book,
};
use sha2::{Digest, Sha256};

fn book() -> Book {
    Book {
        id: "book".into(),
        title: "三体".into(),
        author: "刘慈欣".into(),
        cover: "https://qpic.cn/a.png".into(),
        finished: false,
    }
}
async fn setup() -> (PathBuf, Store, Identity) {
    let root = temporary();
    let store = Store::new(root.clone()).unwrap();
    store.ready().await.unwrap();
    let id = store.save(&credentials()).await.unwrap();
    store
        .apply_shelf_snapshot(&id, vec![book()], Some("shelf1".into()))
        .await
        .unwrap();
    (root, store, id)
}
fn locator() -> Locator {
    Locator {
        schema_version: 1,
        render_anchor: Anchor {
            unit: OffsetUnit::UnicodeScalar,
            offset: 10,
            text_sha256: format!("{:x}", Sha256::digest(b"chapter")),
        },
        source_anchor: None,
        range_end: None,
    }
}
fn position() -> Position {
    Position {
        book_id: "book".into(),
        chapter_uid: Some("chapter".into()),
        content_version: Some("v1".into()),
        locator: locator(),
        progress_ratio: Some(0.5),
        progress_source: ProgressSource::Estimated,
        revision: 0,
        remote_revision: None,
        remote_updated_at_ms: None,
    }
}
fn annotation() -> Annotation {
    Annotation {
        id: new_id(),
        book_id: "book".into(),
        kind: AnnotationKind::Note,
        remote_id: None,
        remote_revision: None,
        sync_status: AnnotationStatus::Local,
        chapter_uid: Some("chapter".into()),
        content_version: Some("v1".into()),
        locator: locator(),
        quote_text: "原文".into(),
        note_text: "笔记".into(),
        style: AnnotationStyle {
            schema_version: 1,
            style: None,
            color: None,
        },
        revision: 0,
        deleted_at_ms: None,
    }
}
fn resource() -> FileResource {
    FileResource {
        book_id: "book".into(),
        chapter_uid: Some("chapter".into()),
        kind: FileKind::Chapter,
        resource_key: "chapter".into(),
        content_version: Some("v1".into()),
        variant: "original".into(),
        media_type: "text/plain".into(),
        pinned: false,
    }
}
fn interval() -> ReadingInterval {
    ReadingInterval {
        id: new_id(),
        book_id: "book".into(),
        started_at_ms: 1,
        ended_at_ms: 1001,
        active_ms: 1000,
    }
}
fn cleanup(root: PathBuf, store: Store) {
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn session_migration_permissions_rotation_and_no_resurrection() {
    let root = temporary();
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("session.json"),
        serde_json::to_vec(&credentials()).unwrap(),
    )
    .unwrap();
    let store = Store::new(root.clone()).unwrap();
    assert_eq!(store.load().await.unwrap().unwrap().vid, "42");
    assert!(!root.join("session.json").exists());
    let identity = store.identity().await.unwrap().unwrap();
    let mut c = credentials();
    c.access_token = "rotated".into();
    c.refresh_token = "rotated-refresh".into();
    store.rotate_session(&identity, &c).await.unwrap();
    assert_eq!(store.identity().await.unwrap().unwrap(), identity);
    assert_eq!(
        store.load().await.unwrap().unwrap().refresh_token,
        "rotated-refresh"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.join("rereader.sqlite3"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    store.clear().await.unwrap();
    store.clear().await.unwrap();
    drop(store);
    fs::write(
        root.join("session.json"),
        serde_json::to_vec(&credentials()).unwrap(),
    )
    .unwrap();
    let store = Store::new(root.clone()).unwrap();
    assert!(store.load().await.unwrap().is_none());
    cleanup(root, store);
}
#[tokio::test]
async fn newer_database_and_simultaneous_owners_are_rejected() {
    let root = temporary();
    fs::create_dir_all(&root).unwrap();
    let conn = Connection::open(root.join("rereader.sqlite3")).unwrap();
    conn.execute_batch("PRAGMA user_version=99").unwrap();
    drop(conn);
    let store = Store::new(root.clone()).unwrap();
    assert!(store.ready().await.unwrap_err().message.contains("newer"));
    drop(store);
    let conn = Connection::open(root.join("rereader.sqlite3")).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        99
    );
    drop(conn);
    fs::remove_file(root.join("rereader.sqlite3")).unwrap();
    let store = Store::new(root.clone()).unwrap();
    store.ready().await.unwrap();
    let other = Store::new(root.clone()).unwrap();
    assert!(other
        .ready()
        .await
        .unwrap_err()
        .message
        .contains("Another process"));
    drop(other);
    cleanup(root, store);
}
#[tokio::test]
async fn accounts_and_epochs_isolate_data() {
    let (root, store, one) = setup().await;
    let note = store
        .save_annotation_and_enqueue(&one, annotation(), false)
        .await
        .unwrap();
    store
        .commit_cached_file(&one, resource(), b"private".to_vec())
        .await
        .unwrap();
    store.clear().await.unwrap();
    assert!(store.load_shelf(&one).await.is_err());
    let mut c = credentials();
    c.vid = "other".into();
    let two = store.save(&c).await.unwrap();
    assert!(store.load_shelf(&two).await.unwrap().is_empty());
    assert!(store
        .read_cached_file(&two, resource())
        .await
        .unwrap()
        .is_none());
    assert!(store
        .save_annotation_and_enqueue(&one, note, false)
        .await
        .is_err());
    let new_one = store.save(&credentials()).await.unwrap();
    assert_ne!(one.auth_epoch, new_one.auth_epoch);
    assert!(store.rotate_session(&one, &credentials()).await.is_err());
    assert_eq!(store.load_shelf(&new_one).await.unwrap().len(), 1);
    assert_eq!(
        store
            .load_annotations(&new_one, "book".into(), false)
            .await
            .unwrap()
            .len(),
        1
    );
    cleanup(root, store);
}
#[tokio::test]
async fn snapshots_are_atomic_and_shelf_removal_preserves_user_data() {
    let (root, store, id) = setup().await;
    store
        .save_annotation_and_enqueue(&id, annotation(), false)
        .await
        .unwrap();
    let duplicate = vec![book(), book()];
    assert!(store
        .apply_shelf_snapshot(&id, duplicate, Some("bad".into()))
        .await
        .is_err());
    assert_eq!(store.load_shelf(&id).await.unwrap(), vec![book()]);
    assert_eq!(
        store
            .load_sync_cursor(&id, SyncScope::Shelf, "".into())
            .await
            .unwrap()
            .unwrap()
            .cursor
            .as_deref(),
        Some("shelf1")
    );
    store
        .apply_shelf_snapshot(&id, vec![], Some("shelf2".into()))
        .await
        .unwrap();
    assert!(store.load_shelf(&id).await.unwrap().is_empty());
    assert_eq!(
        store
            .load_annotations(&id, "book".into(), false)
            .await
            .unwrap()
            .len(),
        1
    );
    cleanup(root, store);
}
#[tokio::test]
async fn positions_outbox_intervals_and_ack_are_atomic() {
    let (root, store, id) = setup().await;
    let mut bad = interval();
    bad.book_id = "wrong".into();
    assert!(store
        .save_position_and_enqueue(&id, position(), vec![bad], true)
        .await
        .is_err());
    assert!(store
        .load_position(&id, "book".into(), Origin::Local)
        .await
        .unwrap()
        .is_none());
    assert!(store.load_sync_jobs(&id).await.unwrap().is_empty());
    let p = store
        .save_position_and_enqueue(&id, position(), vec![interval()], true)
        .await
        .unwrap();
    let mut newer = p.clone();
    newer.locator.render_anchor.offset = 20;
    let newer = store
        .save_position_and_enqueue(&id, newer, vec![interval()], true)
        .await
        .unwrap();
    assert_eq!(newer.revision, 2);
    assert!(store
        .save_position_and_enqueue(&id, p.clone(), vec![], false)
        .await
        .is_err());
    store.save_remote_position(&id, p, None).await.unwrap();
    assert_eq!(
        store
            .load_position(&id, "book".into(), Origin::Local)
            .await
            .unwrap()
            .unwrap(),
        newer
    );
    let first = store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::Progress])
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::Progress])
        .await
        .unwrap()
        .is_none());
    assert!(store
        .complete_sync_job(&id, first.id.clone(), 999, None, None)
        .await
        .is_err());
    store
        .complete_sync_job(&id, first.id, first.entity_revision, None, None)
        .await
        .unwrap();
    store.prune_completed_jobs(&id).await.unwrap();
    assert_eq!(store.load_sync_jobs(&id).await.unwrap().len(), 1);
    let remaining = store
        .call(|db| {
            Ok(db.connection.query_row(
                "SELECT count(*) FROM reading_intervals WHERE acknowledged_at_ms IS NULL",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(remaining, 1);
    cleanup(root, store);
}
#[tokio::test]
async fn restart_and_logout_never_blindly_replay_inflight_writes() {
    let (root, store, id) = setup().await;
    store
        .save_position_and_enqueue(&id, position(), vec![interval()], true)
        .await
        .unwrap();
    let job = store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::Progress])
        .await
        .unwrap()
        .unwrap();
    drop(store);
    let store = Store::new(root.clone()).unwrap();
    assert_eq!(
        store.load_sync_jobs(&id).await.unwrap()[0].status,
        JobStatus::Uncertain
    );
    assert!(store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::Progress])
        .await
        .unwrap()
        .is_none());
    store.clear().await.unwrap();
    let current = store.save(&credentials()).await.unwrap();
    assert!(store
        .complete_sync_job(&id, job.id.clone(), job.entity_revision, None, None)
        .await
        .is_err());
    store
        .reconcile_sync_job(&current, job.id, true, None, None)
        .await
        .unwrap();
    assert_eq!(
        store.load_sync_jobs(&current).await.unwrap()[0].status,
        JobStatus::Done
    );
    cleanup(root, store);
}
#[tokio::test]
async fn retry_and_protocol_gates_are_explicit() {
    let (root, store, id) = setup().await;
    store
        .save_position_and_enqueue(&id, position(), vec![], true)
        .await
        .unwrap();
    assert!(store
        .claim_sync_job(&id, i64::MAX, vec![])
        .await
        .unwrap()
        .is_none());
    let j = store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::Progress])
        .await
        .unwrap()
        .unwrap();
    store
        .record_sync_failure(&id, j.id.clone(), FailureClass::Retryable)
        .await
        .unwrap();
    assert!(store
        .claim_sync_job(&id, 0, vec![JobKind::Progress])
        .await
        .unwrap()
        .is_none());
    let j = store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::Progress])
        .await
        .unwrap()
        .unwrap();
    store
        .record_sync_failure(&id, j.id.clone(), FailureClass::Uncertain)
        .await
        .unwrap();
    store
        .reconcile_sync_job(&id, j.id, false, None, None)
        .await
        .unwrap();
    assert!(store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::Progress])
        .await
        .unwrap()
        .is_some());
    cleanup(root, store);
}
#[tokio::test]
async fn old_annotation_ack_does_not_overwrite_new_edits_or_tombstones() {
    let (root, store, id) = setup().await;
    let a = store
        .save_annotation_and_enqueue(&id, annotation(), true)
        .await
        .unwrap();
    let j = store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::AnnotationPut])
        .await
        .unwrap()
        .unwrap();
    let mut updated = a.clone();
    updated.note_text = "新笔记".into();
    let updated = store
        .save_annotation_and_enqueue(&id, updated, true)
        .await
        .unwrap();
    store
        .complete_sync_job(&id, j.id, 1, Some("remote".into()), Some("r1".into()))
        .await
        .unwrap();
    let mut local = store
        .load_annotations(&id, "book".into(), false)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(local.note_text, "新笔记");
    assert_eq!(local.revision, updated.revision);
    assert_eq!(local.sync_status, AnnotationStatus::Pending);
    assert_eq!(local.remote_id.as_deref(), Some("remote"));
    local.deleted_at_ms = Some(now().unwrap());
    store
        .save_annotation_and_enqueue(&id, local, true)
        .await
        .unwrap();
    assert!(store
        .load_annotations(&id, "book".into(), false)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .load_annotations(&id, "book".into(), true)
            .await
            .unwrap()
            .len(),
        1
    );
    cleanup(root, store);
}
#[tokio::test]
async fn remote_annotation_conflicts_retain_both_versions() {
    let (root, store, id) = setup().await;
    let mut remote = annotation();
    remote.remote_id = Some("r".into());
    remote.remote_revision = Some("v1".into());
    store
        .apply_remote_annotations(&id, "book".into(), vec![remote.clone()], Some("c1".into()))
        .await
        .unwrap();
    let mut local = store
        .load_annotations(&id, "book".into(), false)
        .await
        .unwrap()
        .remove(0);
    local.note_text = "local".into();
    let local = store
        .save_annotation_and_enqueue(&id, local, true)
        .await
        .unwrap();
    remote.note_text = "remote edit".into();
    remote.remote_revision = Some("v2".into());
    store
        .apply_remote_annotations(&id, "book".into(), vec![remote], Some("c2".into()))
        .await
        .unwrap();
    assert_eq!(
        store
            .load_annotations(&id, "book".into(), false)
            .await
            .unwrap()[0]
            .note_text,
        "local"
    );
    assert_eq!(
        store
            .load_annotation_conflict(&id, local.id)
            .await
            .unwrap()
            .unwrap()
            .note_text,
        "remote edit"
    );
    assert_eq!(
        store.load_sync_jobs(&id).await.unwrap()[0].status,
        JobStatus::Blocked
    );
    cleanup(root, store);
}
#[tokio::test]
async fn file_commits_checksums_versions_eviction_and_recovery() {
    let (root, store, id) = setup().await;
    let r = resource();
    store
        .commit_cached_file(&id, r.clone(), "正文".as_bytes().to_vec())
        .await
        .unwrap();
    assert_eq!(
        store
            .read_cached_file(&id, r.clone())
            .await
            .unwrap()
            .unwrap(),
        "正文".as_bytes()
    );
    let mut wrong = r.clone();
    wrong.content_version = Some("v2".into());
    assert!(store.read_cached_file(&id, wrong).await.unwrap().is_none());
    let relative = store
        .call(|db| {
            Ok(db
                .connection
                .query_row("SELECT relative_path FROM cached_files", [], |r| {
                    r.get::<_, String>(0)
                })?)
        })
        .await
        .unwrap();
    fs::write(root.join(relative), "corrupt").unwrap();
    assert!(store
        .read_cached_file(&id, r.clone())
        .await
        .unwrap()
        .is_none());
    let mut pinned = r.clone();
    pinned.pinned = true;
    store
        .commit_cached_file(&id, pinned.clone(), b"keep".to_vec())
        .await
        .unwrap();
    assert_eq!(store.trim_cache(&id, 0).await.unwrap(), 0);
    assert!(store.read_cached_file(&id, pinned).await.unwrap().is_some());
    store
        .commit_cached_file(&id, r.clone(), b"replace".to_vec())
        .await
        .unwrap();
    assert_eq!(store.trim_cache(&id, 0).await.unwrap(), 7);
    assert!(store.read_cached_file(&id, r).await.unwrap().is_none());
    fs::create_dir_all(root.join("tmp")).unwrap();
    fs::write(root.join("tmp/orphan.part"), "orphan").unwrap();
    drop(store);
    let store = Store::new(root.clone()).unwrap();
    store.ready().await.unwrap();
    assert!(!root.join("tmp/orphan.part").exists());
    cleanup(root, store);
}
#[tokio::test]
async fn unsafe_paths_and_symlink_parents_are_rejected() {
    let (root, store, id) = setup().await;
    store
        .commit_cached_file(&id, resource(), b"text".to_vec())
        .await
        .unwrap();
    store
        .call(|db| {
            db.connection
                .execute("UPDATE cached_files SET relative_path='../outside'", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(store
        .read_cached_file(&id, resource())
        .await
        .unwrap()
        .is_none());
    #[cfg(unix)]
    {
        let outside = temporary();
        fs::create_dir_all(&outside).unwrap();
        fs::remove_dir_all(root.join("content")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("content")).unwrap();
        assert!(store
            .commit_cached_file(&id, resource(), b"text".to_vec())
            .await
            .is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_file(root.join("content")).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
    cleanup(root, store);
}
#[tokio::test]
async fn chapters_settings_and_deletion_preserve_relationships() {
    let (root, store, id) = setup().await;
    store
        .save_book_details(
            &id,
            "book".into(),
            "介绍".into(),
            Some("v1".into()),
            BookFormat::Epub,
        )
        .await
        .unwrap();
    let details = store
        .load_book_details(&id, "book".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(details.description, "介绍");
    assert_eq!(details.format, BookFormat::Epub);
    let ch = Chapter {
        uid: "100".into(),
        ordinal: 0,
        title: "第一章".into(),
        parent_uid: None,
        readable: true,
        content_version: Some("v1".into()),
    };
    store
        .apply_chapter_delta(
            &id,
            "book".into(),
            vec![ch.clone()],
            vec![],
            true,
            Some("toc".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        store.load_chapters(&id, "book".into()).await.unwrap(),
        vec![ch]
    );
    store
        .save_position_and_enqueue(&id, position(), vec![interval()], true)
        .await
        .unwrap();
    store
        .apply_chapter_delta(&id, "book".into(), vec![], vec!["100".into()], false, None)
        .await
        .unwrap();
    assert!(store
        .load_position(&id, "book".into(), Origin::Local)
        .await
        .unwrap()
        .is_some());
    let defaults = ReaderSettings {
        schema_version: 1,
        font_size: Some(24.0),
        margin: Some(20.0),
        ..Default::default()
    };
    let specific = ReaderSettings {
        schema_version: 1,
        font_size: Some(30.0),
        ..Default::default()
    };
    store.save_settings(&id, None, defaults).await.unwrap();
    store
        .save_settings(&id, Some("book".into()), specific)
        .await
        .unwrap();
    let settings = store.load_settings(&id, Some("book".into())).await.unwrap();
    assert_eq!(settings.font_size, Some(30.0));
    assert_eq!(settings.margin, Some(20.0));
    store.delete_book(&id, "book".into()).await.unwrap();
    assert!(store.load_sync_jobs(&id).await.unwrap().is_empty());
    assert!(store
        .load_sync_cursor(&id, SyncScope::Chapters, "book".into())
        .await
        .unwrap()
        .is_none());
    store.delete_account(&id).await.unwrap();
    assert!(store.load().await.unwrap().is_none());
    cleanup(root, store);
}

#[tokio::test]
async fn v1_database_upgrades_without_reimporting_old_session() {
    let root = temporary();
    fs::create_dir_all(&root).unwrap();
    let conn = Connection::open(root.join("rereader.sqlite3")).unwrap();
    conn.execute_batch(include_str!("../../docs/storage/001_initial.sql"))
        .unwrap();
    conn.execute("INSERT INTO accounts VALUES('42','account',9,1,1)", [])
        .unwrap();
    conn.execute("INSERT INTO books(account_id,book_id,title,updated_at_ms) VALUES('42','old','Saved book',1)",[]).unwrap();
    drop(conn);
    fs::write(
        root.join("session.json"),
        serde_json::to_vec(&credentials()).unwrap(),
    )
    .unwrap();
    let store = Store::new(root.clone()).unwrap();
    store.ready().await.unwrap();
    assert!(store.load().await.unwrap().is_none());
    let (version, title) = store
        .call(|db| {
            Ok((
                db.connection
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                db.connection
                    .query_row("SELECT title FROM books", [], |r| r.get::<_, String>(0))?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(version, 2);
    assert_eq!(title, "Saved book");
    cleanup(root, store);
}
#[tokio::test]
async fn failed_file_replacement_keeps_old_index_and_bytes() {
    let (root, store, id) = setup().await;
    let r = resource();
    store
        .commit_cached_file(&id, r.clone(), b"old".to_vec())
        .await
        .unwrap();
    let mut invalid = r.clone();
    invalid.chapter_uid = None;
    assert!(store
        .commit_cached_file(&id, invalid, b"bad".to_vec())
        .await
        .is_err());
    assert_eq!(
        store.read_cached_file(&id, r).await.unwrap().unwrap(),
        b"old"
    );
    let count = store
        .call(|db| {
            Ok(db
                .connection
                .query_row("SELECT count(*) FROM cached_files", [], |r| {
                    r.get::<_, i64>(0)
                })?)
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
    cleanup(root, store);
}
#[tokio::test]
async fn conflict_resolution_is_explicit_and_unchanged_remote_is_not_a_conflict() {
    let (root, store, id) = setup().await;
    let mut remote = annotation();
    remote.remote_id = Some("remote".into());
    remote.remote_revision = Some("r1".into());
    store
        .apply_remote_annotations(&id, "book".into(), vec![remote.clone()], None)
        .await
        .unwrap();
    let mut local = store
        .load_annotations(&id, "book".into(), false)
        .await
        .unwrap()
        .remove(0);
    local.note_text = "my edit".into();
    let local = store
        .save_annotation_and_enqueue(&id, local, true)
        .await
        .unwrap();
    store
        .apply_remote_annotations(&id, "book".into(), vec![remote.clone()], None)
        .await
        .unwrap();
    assert_eq!(
        store
            .load_annotations(&id, "book".into(), false)
            .await
            .unwrap()[0]
            .sync_status,
        AnnotationStatus::Pending
    );
    remote.remote_revision = Some("r2".into());
    remote.note_text = "their edit".into();
    store
        .apply_remote_annotations(&id, "book".into(), vec![remote], None)
        .await
        .unwrap();
    assert!(store
        .save_annotation_and_enqueue(&id, local.clone(), true)
        .await
        .is_err());
    let resolved = store
        .resolve_annotation_conflict(&id, local.id.clone(), local.revision, true, false)
        .await
        .unwrap();
    assert_eq!(resolved.note_text, "their edit");
    assert_eq!(resolved.sync_status, AnnotationStatus::Synced);
    assert!(store
        .load_annotation_conflict(&id, local.id)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::AnnotationPut])
        .await
        .unwrap()
        .is_none());
    cleanup(root, store);
}
#[tokio::test]
async fn queued_annotation_edits_use_identity_assigned_by_create_ack() {
    let (root, store, id) = setup().await;
    let a = store
        .save_annotation_and_enqueue(&id, annotation(), true)
        .await
        .unwrap();
    let j = store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::AnnotationPut])
        .await
        .unwrap()
        .unwrap();
    let mut edited = a;
    edited.note_text = "edited offline".into();
    store
        .save_annotation_and_enqueue(&id, edited, true)
        .await
        .unwrap();
    store
        .complete_sync_job(&id, j.id, j.entity_revision, Some("cloud-id".into()), None)
        .await
        .unwrap();
    let edit = store
        .claim_sync_job(&id, i64::MAX, vec![JobKind::AnnotationPut])
        .await
        .unwrap()
        .unwrap();
    assert!(edit.payload["annotation"]["remote_id"].is_null());
    let send = store.prepare_sync_payload(&id, edit.id).await.unwrap();
    assert_eq!(send["annotation"]["remote_id"], "cloud-id");
    assert_eq!(send["annotation"]["note_text"], "edited offline");
    cleanup(root, store);
}
