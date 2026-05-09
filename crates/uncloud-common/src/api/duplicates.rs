use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicateReport {
    pub scanned_at: String,
    pub total_duplicate_files: u32,
    pub total_wasted_bytes: i64,
    pub mirror_clusters: Vec<MirrorCluster>,
    pub subsets: Vec<SubsetPair>,
    pub stray_sets: Vec<StraySet>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicateFolder {
    pub id: String,
    pub path: String,
    pub file_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MirrorCluster {
    pub id: String,
    pub folders: Vec<DuplicateFolder>,
    pub file_count: u32,
    pub total_bytes: i64,
    pub suggested_keep_folder_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubsetPair {
    pub id: String,
    pub subset: DuplicateFolder,
    pub superset: DuplicateFolder,
    pub file_count: u32,
    pub total_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StraySet {
    pub id: String,
    pub checksum: String,
    pub size_bytes: i64,
    pub files: Vec<StrayFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrayFile {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: String,
}
