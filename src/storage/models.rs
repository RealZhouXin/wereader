use crate::weread::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub account_id: String,
    pub auth_epoch: i64,
}

macro_rules! text_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $text)] $variant),+ }
        impl $name {
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant => $text),+ } }
            pub fn parse(s: &str) -> Result<Self> { match s { $($text => Ok(Self::$variant)),+, _ => Err(Error::new("Invalid stored enum value.")) } }
        }
    };
}
text_enum!(BookFormat { Unknown => "unknown", Txt => "txt", Html => "html", Epub => "epub", Pdf => "pdf" });
text_enum!(FileKind { Book => "book", Chapter => "chapter", Cover => "cover", Illustration => "illustration" });
text_enum!(AnnotationStatus { Local => "local", Pending => "pending", Synced => "synced", Conflict => "conflict" });
text_enum!(AnnotationKind { Note => "note", Bookmark => "bookmark", Highlight => "highlight" });
text_enum!(Origin { Local => "local", Remote => "remote" });
text_enum!(ProgressSource { Unknown => "unknown", Estimated => "estimated", Remote => "remote" });
text_enum!(SyncScope { Shelf => "shelf", Chapters => "chapters", Annotations => "annotations", Progress => "progress" });
text_enum!(JobKind { Progress => "progress", AnnotationPut => "annotation_put", AnnotationDelete => "annotation_delete" });
text_enum!(JobStatus { Pending => "pending", InFlight => "in_flight", Retry => "retry", Uncertain => "uncertain", Blocked => "blocked", Done => "done" });
text_enum!(FailureClass { Retryable => "retryable", Uncertain => "uncertain", Permission => "permission", Conflict => "conflict", Unsupported => "unsupported" });
text_enum!(OffsetUnit { UnicodeScalar => "unicode_scalar", Utf8Byte => "utf8_byte", Utf16 => "utf16" });

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub unit: OffsetUnit,
    pub offset: u64,
    pub text_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceAnchor {
    pub protocol: String,
    pub unit: OffsetUnit,
    pub offset: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Locator {
    pub schema_version: u32,
    pub render_anchor: Anchor,
    pub source_anchor: Option<SourceAnchor>,
    pub range_end: Option<Anchor>,
}
impl Locator {
    pub fn validate(&self) -> Result<()> {
        let a = &self.render_anchor;
        if self.schema_version != 1
            || !digest(&a.text_sha256)
            || self
                .source_anchor
                .as_ref()
                .is_some_and(|s| s.protocol.is_empty())
            || self.range_end.as_ref().is_some_and(|b| {
                b.unit != a.unit || b.text_sha256 != a.text_sha256 || b.offset < a.offset
            })
        {
            return Err(Error::new("Invalid or unsupported reading locator."));
        }
        Ok(())
    }
}
pub(crate) fn digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub book_id: String,
    pub chapter_uid: Option<String>,
    pub content_version: Option<String>,
    pub locator: Locator,
    pub progress_ratio: Option<f64>,
    pub progress_source: ProgressSource,
    pub revision: i64,
    pub remote_revision: Option<String>,
    pub remote_updated_at_ms: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub book_id: String,
    pub kind: AnnotationKind,
    pub remote_id: Option<String>,
    pub remote_revision: Option<String>,
    pub sync_status: AnnotationStatus,
    pub chapter_uid: Option<String>,
    pub content_version: Option<String>,
    pub locator: Locator,
    pub quote_text: String,
    pub note_text: String,
    pub style: AnnotationStyle,
    pub revision: i64,
    pub deleted_at_ms: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnnotationStyle {
    pub schema_version: u32,
    pub style: Option<String>,
    pub color: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct BookDetails {
    pub book: crate::weread::Book,
    pub description: String,
    pub content_version: Option<String>,
    pub format: BookFormat,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Chapter {
    pub uid: String,
    pub ordinal: i64,
    pub title: String,
    pub parent_uid: Option<String>,
    pub readable: bool,
    pub content_version: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadingInterval {
    pub id: String,
    pub book_id: String,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub active_ms: i64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct FileResource {
    pub book_id: String,
    pub chapter_uid: Option<String>,
    pub kind: FileKind,
    pub resource_key: String,
    pub content_version: Option<String>,
    pub variant: String,
    pub media_type: String,
    pub pinned: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SyncCursor {
    pub scope: SyncScope,
    pub resource_id: String,
    pub cursor: Option<String>,
    pub last_success_at_ms: Option<i64>,
    pub last_attempt_at_ms: Option<i64>,
    pub last_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SyncJob {
    pub id: String,
    pub book_id: String,
    pub kind: JobKind,
    pub entity_id: String,
    pub entity_revision: i64,
    pub payload: serde_json::Value,
    pub status: JobStatus,
    pub attempt_count: i64,
    pub next_attempt_at_ms: i64,
    pub attempt_auth_epoch: Option<i64>,
    pub last_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReaderSettings {
    pub schema_version: u32,
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub line_height: Option<f64>,
    pub margin: Option<f64>,
    pub theme: Option<String>,
}
impl Default for ReaderSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            font_family: None,
            font_size: None,
            line_height: None,
            margin: None,
            theme: None,
        }
    }
}
impl ReaderSettings {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || self
                .font_size
                .is_some_and(|v| !v.is_finite() || !(6.0..=200.0).contains(&v))
            || self
                .line_height
                .is_some_and(|v| !v.is_finite() || !(0.5..=5.0).contains(&v))
            || self
                .margin
                .is_some_and(|v| !v.is_finite() || !(0.0..=1000.0).contains(&v))
        {
            return Err(Error::new("Invalid reader settings."));
        }
        Ok(())
    }
}
