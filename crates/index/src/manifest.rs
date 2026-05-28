use time::OffsetDateTime;

use nixsearch_core::ArtifactKind;

use crate::schema::INDEX_SCHEMA_VERSION;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IndexGenerationManifest {
    pub schema_version: u32,

    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,

    pub document_count: usize,
    pub targets: Vec<IndexTargetManifest>,
}

impl IndexGenerationManifest {
    pub fn new(document_count: usize, targets: Vec<IndexTargetManifest>) -> Self {
        Self::with_generated_at(document_count, targets, OffsetDateTime::now_utc())
    }

    pub fn with_generated_at(
        document_count: usize,
        targets: Vec<IndexTargetManifest>,
        generated_at: OffsetDateTime,
    ) -> Self {
        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            generated_at,
            document_count,
            targets,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IndexTargetManifest {
    pub source: String,
    pub ref_id: String,
    pub artifact_kind: ArtifactKind,
    pub document_count: usize,
    pub artifact_hash: Option<String>,
    pub revision: Option<String>,
}
