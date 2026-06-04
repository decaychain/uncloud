//! Path parsing and resolution for MCP tool inputs.
//!
//! Tools accept absolute, slash-delimited, case-sensitive paths
//! (`/Documents/notes.txt`). Parsing rejects parent traversal (`..`)
//! and backslashes — the model is meant to address files by the same
//! path strings the listing returns, with no separator ambiguity.
//! Resolution walks the user's `folders` and `files` collections by
//! `(owner_id, parent_id, name)`. The schema doesn't store a denormalised
//! path, so resolution is O(depth) Mongo lookups; that's fine for the
//! shallow trees these tools deal with.

use mongodb::bson::{doc, oid::ObjectId, Bson};

use crate::models::file::File;
use crate::models::folder::Folder;
use crate::AppState;

use super::tools::ToolError;

/// Split + validate a tool path. Empty string and `/` both mean root
/// (returns an empty Vec). Trailing slash is tolerated. Each segment
/// must be non-empty and must not equal `..`.
pub fn parse(input: &str) -> Result<Vec<String>, ToolError> {
    if input.contains('\\') {
        return Err(ToolError::invalid(
            "path must not contain backslashes; use forward slashes only",
        ));
    }
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.chars().all(|c| c == '/') {
        return Ok(Vec::new());
    }
    if !trimmed.starts_with('/') {
        return Err(ToolError::invalid("path must be absolute (start with '/')"));
    }
    let mut out = Vec::new();
    let stripped = trimmed.trim_start_matches('/').trim_end_matches('/');
    for seg in stripped.split('/') {
        if seg.is_empty() {
            // Disallow "//" — keeps round-tripping with build_path
            // unambiguous.
            return Err(ToolError::invalid(
                "path must not contain empty segments ('//')",
            ));
        }
        if seg == ".." || seg == "." {
            return Err(ToolError::invalid(
                "path must not contain '.' or '..' segments",
            ));
        }
        out.push(seg.to_string());
    }
    Ok(out)
}

/// Resolve `segments` to a `Folder` rooted at the user's root.
/// Empty segments → the root (returns `None`, meaning "the user's root,
/// which has no Folder document"). Any segment that names a file rather
/// than a folder yields `ToolError::Execution`.
pub async fn resolve_folder(
    state: &AppState,
    owner_id: ObjectId,
    segments: &[String],
) -> Result<Option<Folder>, ToolError> {
    if segments.is_empty() {
        return Ok(None);
    }
    let folders = state.db.collection::<Folder>("folders");
    let mut parent: Option<ObjectId> = None;
    let mut current: Option<Folder> = None;
    for seg in segments {
        let parent_filter = match parent {
            Some(pid) => Bson::ObjectId(pid),
            None => Bson::Null,
        };
        let folder = folders
            .find_one(doc! {
                "owner_id": owner_id,
                "parent_id": parent_filter,
                "name": seg,
                "deleted_at": Bson::Null,
            })
            .await
            .map_err(|e| ToolError::exec(format!("folder lookup failed: {}", e)))?
            .ok_or_else(|| ToolError::exec(format!("no folder named `{}` at this level", seg)))?;
        parent = Some(folder.id);
        current = Some(folder);
    }
    Ok(current)
}

/// Resolve a file path. The final segment is the file name; everything
/// before it identifies the parent folder.
pub async fn resolve_file(
    state: &AppState,
    owner_id: ObjectId,
    segments: &[String],
) -> Result<File, ToolError> {
    let (file_name, parent_segments) = match segments.split_last() {
        Some(parts) => parts,
        None => {
            return Err(ToolError::invalid("file path must include a filename"));
        }
    };
    let parent_folder = resolve_folder(state, owner_id, parent_segments).await?;
    let parent_filter = match &parent_folder {
        Some(f) => Bson::ObjectId(f.id),
        None => Bson::Null,
    };
    let files = state.db.collection::<File>("files");
    let file = files
        .find_one(doc! {
            "owner_id": owner_id,
            "parent_id": parent_filter,
            "name": file_name,
            "deleted_at": Bson::Null,
        })
        .await
        .map_err(|e| ToolError::exec(format!("file lookup failed: {}", e)))?
        .ok_or_else(|| ToolError::exec(format!("no file named `{}` at this path", file_name)))?;
    Ok(file)
}

/// Whatever lives at the path — a folder or a file. Used by the
/// write tools (move/copy/delete) that operate on either.
pub enum Target {
    Folder(Folder),
    File(File),
}

/// Resolve a path to whatever lives there. Folders take precedence if
/// somehow both existed at the same path (the unique indexes prevent
/// it, but we check folders first because directory traversal needs to
/// match folders mid-path anyway).
pub async fn resolve_target(
    state: &AppState,
    owner_id: ObjectId,
    segments: &[String],
) -> Result<Target, ToolError> {
    if segments.is_empty() {
        return Err(ToolError::exec("operation cannot target the root folder"));
    }
    let folders = state.db.collection::<Folder>("folders");
    let last = segments.last().unwrap();
    let parent_segments = &segments[..segments.len() - 1];
    let parent = resolve_folder(state, owner_id, parent_segments).await?;
    let parent_filter = match &parent {
        Some(f) => Bson::ObjectId(f.id),
        None => Bson::Null,
    };
    if let Some(folder) = folders
        .find_one(doc! {
            "owner_id": owner_id,
            "parent_id": parent_filter.clone(),
            "name": last,
            "deleted_at": Bson::Null,
        })
        .await
        .map_err(|e| ToolError::exec(format!("folder lookup failed: {}", e)))?
    {
        return Ok(Target::Folder(folder));
    }
    let files = state.db.collection::<File>("files");
    if let Some(file) = files
        .find_one(doc! {
            "owner_id": owner_id,
            "parent_id": parent_filter,
            "name": last,
            "deleted_at": Bson::Null,
        })
        .await
        .map_err(|e| ToolError::exec(format!("file lookup failed: {}", e)))?
    {
        return Ok(Target::File(file));
    }
    Err(ToolError::exec(format!(
        "nothing exists at path `{}`",
        format_args!("/{}", segments.join("/"))
    )))
}

/// Compute the absolute path of a file/folder by walking its parent
/// chain. Used to put a `path` field on every entry the tools return.
/// Bounded loop for safety in case of corrupted parent_id cycles.
pub async fn build_for_file(state: &AppState, file: &File) -> String {
    let parent_path = build_for_parent(state, file.parent_id).await;
    join(&parent_path, &file.name)
}

pub async fn build_for_folder(state: &AppState, folder: &Folder) -> String {
    let parent_path = build_for_parent(state, folder.parent_id).await;
    join(&parent_path, &folder.name)
}

async fn build_for_parent(state: &AppState, mut parent: Option<ObjectId>) -> Vec<String> {
    let folders = state.db.collection::<Folder>("folders");
    let mut chain = Vec::new();
    let mut hops = 0usize;
    while let Some(pid) = parent {
        if hops >= 64 {
            // Defensive: corrupted parent chain shouldn't hang the tool.
            break;
        }
        hops += 1;
        match folders
            .find_one(doc! { "_id": pid, "deleted_at": Bson::Null })
            .await
        {
            Ok(Some(f)) => {
                parent = f.parent_id;
                chain.push(f.name);
            }
            _ => break,
        }
    }
    chain.reverse();
    chain
}

fn join(parent_segments: &[String], name: &str) -> String {
    let mut out = String::from("/");
    for seg in parent_segments {
        out.push_str(seg);
        out.push('/');
    }
    out.push_str(name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_root_variants() {
        assert!(parse("").unwrap().is_empty());
        assert!(parse("/").unwrap().is_empty());
        assert!(parse("  /  ").unwrap().is_empty());
    }

    #[test]
    fn parse_absolute_required() {
        assert!(parse("foo/bar").is_err());
    }

    #[test]
    fn parse_rejects_backslash() {
        assert!(parse("/foo\\bar").is_err());
    }

    #[test]
    fn parse_rejects_dotdot_and_dot() {
        assert!(parse("/foo/../bar").is_err());
        assert!(parse("/./foo").is_err());
    }

    #[test]
    fn parse_rejects_empty_segment() {
        assert!(parse("/foo//bar").is_err());
    }

    #[test]
    fn parse_keeps_segments() {
        let p = parse("/Documents/notes.txt").unwrap();
        assert_eq!(p, vec!["Documents".to_string(), "notes.txt".to_string()]);
    }

    #[test]
    fn parse_tolerates_trailing_slash() {
        let p = parse("/Documents/").unwrap();
        assert_eq!(p, vec!["Documents".to_string()]);
    }
}
