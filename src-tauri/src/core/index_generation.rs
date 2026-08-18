use serde::{Deserialize, Serialize};

pub const INDEX_GENERATION_DATA_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexGenerationState {
    Building,
    Prepared,
    Active,
    Obsolete,
}

impl IndexGenerationState {
    pub(crate) fn from_storage(value: &str) -> Option<Self> {
        match value {
            "building" => Some(Self::Building),
            "prepared" => Some(Self::Prepared),
            "active" => Some(Self::Active),
            "obsolete" => Some(Self::Obsolete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct IndexGenerationScanSummary {
    pub scanned: u64,
    pub accepted: u64,
    pub skipped: u64,
    pub failures: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexGenerationRootState {
    pub root: String,
    pub stage: String,
    pub degraded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexGenerationRecord {
    pub batch_id: i64,
    #[serde(skip_serializing, default)]
    pub config_fingerprint: String,
    pub state: IndexGenerationState,
    pub data_version: i64,
    pub entry_count: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub scan_summary: IndexGenerationScanSummary,
    #[serde(skip_serializing, default)]
    pub roots: Vec<IndexGenerationRootState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexGenerationResumeKind {
    Idle,
    Prepared,
    Building,
    Create,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexGenerationRecovery {
    /// A compatible active baseline can be mounted immediately, even when maintenance work exists.
    pub active: Option<IndexGenerationRecord>,
    /// At most one compatible building or prepared generation is retained.
    pub pending: Option<IndexGenerationRecord>,
    pub resume: IndexGenerationResumeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStorageGcResult {
    pub deleted_generations: u64,
    pub deleted_entries: u64,
    pub freelist_pages_before: u64,
    pub freelist_pages_after: u64,
    pub completed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStorageDiagnostics {
    pub active_generation: Option<i64>,
    pub pending_generation: Option<i64>,
    pub generations: Vec<IndexGenerationRecord>,
    pub database_size_bytes: u64,
    pub wal_size_bytes: u64,
    pub freelist_page_count: u64,
    pub page_count: u64,
    pub auto_vacuum_incremental: bool,
    pub auto_vacuum_migration_required: bool,
    pub last_gc: Option<IndexStorageGcResult>,
}

#[cfg(test)]
mod tests {
    use super::{
        IndexGenerationRecord, IndexGenerationRootState, IndexGenerationScanSummary,
        IndexGenerationState,
    };

    #[test]
    fn generation_diagnostics_do_not_serialize_private_configuration() {
        let value = serde_json::to_value(IndexGenerationRecord {
            batch_id: 7,
            config_fingerprint: r#"{\"roots\":[\"/Users/example/private\"]}"#.to_owned(),
            state: IndexGenerationState::Active,
            data_version: 1,
            entry_count: 42,
            created_at_ms: 10,
            updated_at_ms: 20,
            scan_summary: IndexGenerationScanSummary::default(),
            roots: vec![IndexGenerationRootState {
                root: "/Users/example/private".to_owned(),
                stage: "completed".to_owned(),
                degraded: false,
            }],
        })
        .expect("generation diagnostics should serialize");

        assert!(value.get("configFingerprint").is_none());
        assert!(value.get("roots").is_none());
        assert_eq!(value["entryCount"], 42);
    }
}
