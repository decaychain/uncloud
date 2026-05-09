use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use axum::{extract::State, Json};
use futures::stream::TryStreamExt;
use mongodb::bson::{doc, oid::ObjectId, Bson};
use uncloud_common::{
    DuplicateFolder, DuplicateReport, MirrorCluster, StrayFile, StraySet, SubsetPair,
};

use crate::error::Result;
use crate::middleware::AuthUser;
use crate::models::{File, Folder};
use crate::routes::files::build_folder_path;
use crate::AppState;

const BACKUP_PATH_SEGMENTS: &[&str] = &[
    "backup", "backups", "archive", "archives", "old", "trash",
];

pub async fn get_duplicate_report(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<DuplicateReport>> {
    let report = compute_report(&state, user.id).await?;
    Ok(Json(report))
}

#[derive(Debug, Clone)]
struct DupFile {
    id: ObjectId,
    name: String,
    parent_id: Option<ObjectId>,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
struct DupSet {
    checksum: String,
    size_bytes: i64,
    files: Vec<DupFile>,
}

async fn compute_report(state: &AppState, owner_id: ObjectId) -> Result<DuplicateReport> {
    let dup_sets = aggregate_dup_sets(state, owner_id).await?;

    if dup_sets.is_empty() {
        return Ok(DuplicateReport {
            scanned_at: chrono::Utc::now().to_rfc3339(),
            total_duplicate_files: 0,
            total_wasted_bytes: 0,
            mirror_clusters: vec![],
            subsets: vec![],
            stray_sets: vec![],
        });
    }

    let folders = load_user_folders(state, owner_id).await?;
    let folder_by_id: HashMap<ObjectId, &Folder> =
        folders.iter().map(|f| (f.id, f)).collect();

    // folder_id (Some) -> set of checksums it contributes to dup_sets.
    // Files at the user's root (parent_id == None) are not folder-classifiable —
    // they always go to the stray bucket.
    let mut folder_hashes: HashMap<ObjectId, BTreeSet<String>> = HashMap::new();
    for set in &dup_sets {
        for f in &set.files {
            if let Some(pid) = f.parent_id {
                folder_hashes
                    .entry(pid)
                    .or_default()
                    .insert(set.checksum.clone());
            }
        }
    }

    let participating: Vec<ObjectId> = folder_hashes.keys().copied().collect();
    let (equal_pairs, subset_pairs) = classify_folder_pairs(&participating, &folder_hashes);

    let mirror_components = build_mirror_clusters(&equal_pairs, &folder_hashes);

    // Folders that ended up in a mirror cluster — used to skip them when
    // emitting subset pairs (an equal pair that survived classification is
    // already a mirror).
    let folders_in_mirror: HashSet<ObjectId> = mirror_components
        .iter()
        .flat_map(|c| c.iter().copied())
        .collect();

    let mut mirror_cards: Vec<MirrorCluster> = Vec::new();
    for (idx, members) in mirror_components.iter().enumerate() {
        let card = build_mirror_card(idx, members, &dup_sets, &folder_by_id);
        if let Some(card) = card {
            mirror_cards.push(card);
        }
    }

    let mut subset_cards: Vec<SubsetPair> = Vec::new();
    for (idx, (sub, sup)) in subset_pairs.iter().enumerate() {
        if folders_in_mirror.contains(sub) || folders_in_mirror.contains(sup) {
            continue;
        }
        if let Some(card) = build_subset_card(idx, *sub, *sup, &dup_sets, &folder_by_id) {
            subset_cards.push(card);
        }
    }

    let stray_sets = build_stray_sets(&dup_sets, &mirror_cards, &subset_cards, &folder_by_id);

    let total_duplicate_files: u32 = mirror_cards.iter().map(|c| c.file_count).sum::<u32>()
        + subset_cards.iter().map(|c| c.file_count).sum::<u32>()
        + stray_sets
            .iter()
            .map(|s| s.files.len() as u32)
            .sum::<u32>();
    let total_wasted_bytes: i64 = mirror_cards.iter().map(|c| c.total_bytes).sum::<i64>()
        + subset_cards.iter().map(|c| c.total_bytes).sum::<i64>()
        + stray_sets
            .iter()
            .map(|s| s.size_bytes * (s.files.len() as i64 - 1).max(0))
            .sum::<i64>();

    Ok(DuplicateReport {
        scanned_at: chrono::Utc::now().to_rfc3339(),
        total_duplicate_files,
        total_wasted_bytes,
        mirror_clusters: mirror_cards,
        subsets: subset_cards,
        stray_sets,
    })
}

async fn aggregate_dup_sets(state: &AppState, owner_id: ObjectId) -> Result<Vec<DupSet>> {
    let coll = state.db.collection::<File>("files");

    let pipeline = vec![
        doc! {
            "$match": {
                "owner_id": owner_id,
                "deleted_at": Bson::Null,
                "size_bytes": { "$gt": 0 },
                "checksum_sha256": { "$ne": "" },
            }
        },
        doc! {
            "$group": {
                "_id": "$checksum_sha256",
                "size_bytes": { "$first": "$size_bytes" },
                "files": {
                    "$push": {
                        "_id": "$_id",
                        "name": "$name",
                        "parent_id": "$parent_id",
                        "size_bytes": "$size_bytes",
                        "created_at": "$created_at",
                        "storage_path": "$storage_path",
                    }
                },
            }
        },
        doc! { "$match": { "files.1": { "$exists": true } } },
    ];

    let mut cursor = coll.aggregate(pipeline).await?;
    let mut out = Vec::new();

    while let Some(doc) = cursor.try_next().await? {
        let checksum = match doc.get_str("_id") {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };
        let size_bytes = doc.get_i64("size_bytes").unwrap_or(0);
        let raw_files = match doc.get_array("files") {
            Ok(a) => a,
            Err(_) => continue,
        };

        let mut files = Vec::new();
        for entry in raw_files {
            let f = match entry.as_document() {
                Some(d) => d,
                None => continue,
            };

            // Skip files inside the .uncloud/ namespace (versions, trash,
            // thumbnails). The aggregation can't easily filter by suffix on
            // owner_id key, so we filter here.
            let storage_path = f.get_str("storage_path").unwrap_or("");
            if storage_path
                .split('/')
                .any(|seg| seg == ".uncloud")
            {
                continue;
            }

            let id = match f.get_object_id("_id") {
                Ok(id) => id,
                Err(_) => continue,
            };
            let name = f.get_str("name").unwrap_or("").to_string();
            let parent_id = f.get_object_id("parent_id").ok();
            let created_at = f
                .get_datetime("created_at")
                .ok()
                .map(|d| d.to_chrono())
                .unwrap_or_else(chrono::Utc::now);

            files.push(DupFile {
                id,
                name,
                parent_id,
                created_at,
            });
        }

        if files.len() >= 2 {
            out.push(DupSet {
                checksum,
                size_bytes,
                files,
            });
        }
    }

    Ok(out)
}

async fn load_user_folders(state: &AppState, owner_id: ObjectId) -> Result<Vec<Folder>> {
    let coll = state.db.collection::<Folder>("folders");
    let cursor = coll
        .find(doc! { "owner_id": owner_id, "deleted_at": Bson::Null })
        .await?;
    Ok(cursor.try_collect().await?)
}

/// Classifies every unordered pair of folders that share at least one duplicate
/// hash into Equal / Subset. Partial overlaps are dropped — they don't get
/// their own bucket in v1.
fn classify_folder_pairs(
    participating: &[ObjectId],
    folder_hashes: &HashMap<ObjectId, BTreeSet<String>>,
) -> (Vec<(ObjectId, ObjectId)>, Vec<(ObjectId, ObjectId)>) {
    let mut equals = Vec::new();
    let mut subsets = Vec::new();

    for i in 0..participating.len() {
        for j in (i + 1)..participating.len() {
            let a = participating[i];
            let b = participating[j];
            let ha = folder_hashes.get(&a);
            let hb = folder_hashes.get(&b);
            if let (Some(ha), Some(hb)) = (ha, hb) {
                if ha.is_disjoint(hb) {
                    continue;
                }
                if ha == hb {
                    equals.push((a, b));
                } else if hb.is_subset(ha) {
                    // b ⊂ a strictly
                    subsets.push((b, a));
                } else if ha.is_subset(hb) {
                    subsets.push((a, b));
                }
                // Partial overlap → fall through silently.
            }
        }
    }

    (equals, subsets)
}

/// Connected-components clustering of equal-folder pairs, with a verification
/// step: every member of a cluster must have the exact same checksum set.
/// Pairs that don't survive verification are split apart (defensive — equal
/// is transitive over checksum-set equality, so this is mostly belt-and-
/// braces).
fn build_mirror_clusters(
    equal_pairs: &[(ObjectId, ObjectId)],
    folder_hashes: &HashMap<ObjectId, BTreeSet<String>>,
) -> Vec<Vec<ObjectId>> {
    if equal_pairs.is_empty() {
        return vec![];
    }

    // Union-find over the IDs we've seen.
    let mut parent: HashMap<ObjectId, ObjectId> = HashMap::new();
    fn find(parent: &mut HashMap<ObjectId, ObjectId>, x: ObjectId) -> ObjectId {
        let p = *parent.get(&x).unwrap_or(&x);
        if p == x {
            x
        } else {
            let root = find(parent, p);
            parent.insert(x, root);
            root
        }
    }
    fn union(parent: &mut HashMap<ObjectId, ObjectId>, a: ObjectId, b: ObjectId) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }
    for (a, b) in equal_pairs {
        parent.entry(*a).or_insert(*a);
        parent.entry(*b).or_insert(*b);
        union(&mut parent, *a, *b);
    }

    let nodes: Vec<ObjectId> = parent.keys().copied().collect();
    let mut groups: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
    for n in nodes {
        let r = find(&mut parent, n);
        groups.entry(r).or_default().push(n);
    }

    let mut out = Vec::new();
    for (_, members) in groups {
        if members.len() < 2 {
            continue;
        }
        // Verify all members share the exact same checksum set.
        let reference = match folder_hashes.get(&members[0]) {
            Some(h) => h,
            None => continue,
        };
        if members
            .iter()
            .all(|m| folder_hashes.get(m).is_some_and(|h| h == reference))
        {
            out.push(members);
        }
    }

    out
}

fn build_mirror_card(
    idx: usize,
    members: &[ObjectId],
    dup_sets: &[DupSet],
    folder_by_id: &HashMap<ObjectId, &Folder>,
) -> Option<MirrorCluster> {
    if members.len() < 2 {
        return None;
    }

    let member_set: HashSet<ObjectId> = members.iter().copied().collect();

    // For each cluster member, count files inside it that participate in
    // dup_sets entirely contained in member_set.
    let mut per_folder_count: HashMap<ObjectId, u32> = HashMap::new();
    let mut total_bytes: i64 = 0;
    let mut explained_files_in_cluster: u32 = 0;
    for set in dup_sets {
        let folders_in_set: HashSet<ObjectId> = set
            .files
            .iter()
            .filter_map(|f| f.parent_id)
            .collect();
        if folders_in_set.is_empty() || !folders_in_set.is_subset(&member_set) {
            continue;
        }
        // This duplicate set is fully explained by this cluster.
        let mut count_in_set = 0u32;
        for f in &set.files {
            if let Some(pid) = f.parent_id {
                if member_set.contains(&pid) {
                    *per_folder_count.entry(pid).or_default() += 1;
                    count_in_set += 1;
                }
            }
        }
        if count_in_set >= 2 {
            // wasted = (count_in_set - 1) copies × size
            total_bytes += set.size_bytes * (count_in_set as i64 - 1);
            explained_files_in_cluster += count_in_set;
        }
    }

    if explained_files_in_cluster == 0 {
        return None;
    }

    let mut folders: Vec<DuplicateFolder> = members
        .iter()
        .map(|fid| {
            let path = folder_by_id
                .get(fid)
                .map(|f| build_folder_path(f.id, folder_by_id))
                .unwrap_or_else(|| "(unknown)".to_string());
            DuplicateFolder {
                id: fid.to_hex(),
                path,
                file_count: per_folder_count.get(fid).copied().unwrap_or(0),
            }
        })
        .collect();
    folders.sort_by(|a, b| a.path.cmp(&b.path));

    let suggested_keep_folder_id = pick_keeper(members, folder_by_id);

    Some(MirrorCluster {
        id: format!("mc-{idx}"),
        folders,
        // file_count = the number of files we'd delete = total minus the
        // single keeper instance for each duplicate set.
        // For simplicity, we report total instances across the cluster; the
        // UI shows "delete the others" = (folders.len() - 1) × file_count_per_folder.
        file_count: per_folder_count.values().copied().min().unwrap_or(0),
        total_bytes,
        suggested_keep_folder_id: suggested_keep_folder_id.to_hex(),
    })
}

fn build_subset_card(
    idx: usize,
    subset: ObjectId,
    superset: ObjectId,
    dup_sets: &[DupSet],
    folder_by_id: &HashMap<ObjectId, &Folder>,
) -> Option<SubsetPair> {
    let pair_set: HashSet<ObjectId> = [subset, superset].into_iter().collect();

    let mut subset_count: u32 = 0;
    let mut superset_count: u32 = 0;
    let mut total_bytes: i64 = 0;

    for set in dup_sets {
        let folders_in_set: HashSet<ObjectId> = set
            .files
            .iter()
            .filter_map(|f| f.parent_id)
            .collect();
        if folders_in_set.is_empty() || !folders_in_set.is_subset(&pair_set) {
            continue;
        }
        let mut sub_in = 0u32;
        let mut sup_in = 0u32;
        for f in &set.files {
            if f.parent_id == Some(subset) {
                sub_in += 1;
            } else if f.parent_id == Some(superset) {
                sup_in += 1;
            }
        }
        if sub_in >= 1 && sup_in >= 1 {
            // wasted = the subset's copies (assuming we keep the superset)
            total_bytes += set.size_bytes * sub_in as i64;
            subset_count += sub_in;
            superset_count += sup_in;
        }
    }

    if subset_count == 0 {
        return None;
    }

    let subset_path = folder_by_id
        .get(&subset)
        .map(|f| build_folder_path(f.id, folder_by_id))
        .unwrap_or_else(|| "(unknown)".to_string());
    let superset_path = folder_by_id
        .get(&superset)
        .map(|f| build_folder_path(f.id, folder_by_id))
        .unwrap_or_else(|| "(unknown)".to_string());

    Some(SubsetPair {
        id: format!("ss-{idx}"),
        subset: DuplicateFolder {
            id: subset.to_hex(),
            path: subset_path,
            file_count: subset_count,
        },
        superset: DuplicateFolder {
            id: superset.to_hex(),
            path: superset_path,
            file_count: superset_count,
        },
        file_count: subset_count,
        total_bytes,
    })
}

fn build_stray_sets(
    dup_sets: &[DupSet],
    mirror_cards: &[MirrorCluster],
    subset_cards: &[SubsetPair],
    folder_by_id: &HashMap<ObjectId, &Folder>,
) -> Vec<StraySet> {
    // For each dup_set, decide whether it's fully explained by a mirror
    // cluster or a subset pair. If yes, skip. Otherwise emit as stray.
    let mirror_member_sets: Vec<HashSet<ObjectId>> = mirror_cards
        .iter()
        .map(|c| {
            c.folders
                .iter()
                .filter_map(|f| ObjectId::parse_str(&f.id).ok())
                .collect()
        })
        .collect();
    let subset_pair_sets: Vec<HashSet<ObjectId>> = subset_cards
        .iter()
        .map(|s| {
            [&s.subset.id, &s.superset.id]
                .iter()
                .filter_map(|s| ObjectId::parse_str(s).ok())
                .collect()
        })
        .collect();

    let mut out = Vec::new();
    for (idx, set) in dup_sets.iter().enumerate() {
        let folders_in_set: HashSet<ObjectId> = set
            .files
            .iter()
            .filter_map(|f| f.parent_id)
            .collect();

        // Files at user root (parent_id None) bypass folder explanation —
        // always stray.
        let has_root_files = set.files.iter().any(|f| f.parent_id.is_none());

        let explained_by_mirror = !has_root_files
            && !folders_in_set.is_empty()
            && mirror_member_sets
                .iter()
                .any(|m| folders_in_set.is_subset(m));
        let explained_by_subset = !has_root_files
            && !folders_in_set.is_empty()
            && subset_pair_sets
                .iter()
                .any(|p| folders_in_set.is_subset(p));

        if explained_by_mirror || explained_by_subset {
            continue;
        }

        let mut files: Vec<StrayFile> = set
            .files
            .iter()
            .map(|f| {
                let folder_path = match f.parent_id {
                    Some(pid) => folder_by_id
                        .get(&pid)
                        .map(|fld| build_folder_path(fld.id, folder_by_id))
                        .unwrap_or_else(|| "(unknown)".to_string()),
                    None => String::new(),
                };
                let path = if folder_path.is_empty() {
                    f.name.clone()
                } else {
                    format!("{} / {}", folder_path, f.name)
                };
                StrayFile {
                    id: f.id.to_hex(),
                    name: f.name.clone(),
                    path,
                    created_at: f.created_at.to_rfc3339(),
                }
            })
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));

        out.push(StraySet {
            id: format!("st-{idx}"),
            checksum: set.checksum.clone(),
            size_bytes: set.size_bytes,
            files,
        });
    }

    // Sort: largest wasted bytes first.
    out.sort_by(|a, b| {
        let wa = a.size_bytes * (a.files.len() as i64 - 1).max(0);
        let wb = b.size_bytes * (b.files.len() as i64 - 1).max(0);
        wb.cmp(&wa)
    });

    out
}

fn pick_keeper(members: &[ObjectId], folder_by_id: &HashMap<ObjectId, &Folder>) -> ObjectId {
    let mut scored: Vec<(usize, usize, chrono::DateTime<chrono::Utc>, ObjectId)> = members
        .iter()
        .map(|fid| {
            let path = folder_by_id
                .get(fid)
                .map(|f| build_folder_path(f.id, folder_by_id))
                .unwrap_or_default();
            let lower = path.to_lowercase();
            let backup_score = if BACKUP_PATH_SEGMENTS
                .iter()
                .any(|seg| lower.split(|c: char| c == ' ' || c == '/').any(|s| s == *seg))
            {
                1usize
            } else {
                0usize
            };
            let depth = path.matches(" / ").count();
            let created = folder_by_id
                .get(fid)
                .map(|f| f.created_at)
                .unwrap_or_else(chrono::Utc::now);
            (backup_score, depth, created, *fid)
        })
        .collect();

    // Lower is better: non-backup (0) < backup (1); shallower depth; older created_at.
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });

    scored[0].3
}
