use crate::state::AppState;
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

/// How many versions of one file to keep. Each snapshot is at most MAX_FILE_BYTES
/// (enforced by the writer), so 50 versions per file is a few tens of MiB worst case.
const MAX_VERSIONS_PER_FILE: usize = 50;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryEntry {
    pub timestamp: i64,
    pub size: u64,
}

fn fnv1a(value: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// One directory per (project, file path); the readable prefix keeps manual
/// browsing possible, the hash prevents collisions and filesystem-hostile names.
pub(crate) fn sanitize_relative_path(input: &str) -> String {
    let safe: String = input
        .chars()
        .map(|ch| if ch.is_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
        .collect();
    let safe = safe.trim_matches('_');
    if safe.is_empty() {
        format!("file-{:016x}", fnv1a(input))
    } else {
        format!("{safe}-{:016x}", fnv1a(input))
    }
}

/// App data dir, next to workspace.sqlite3.
fn history_root(state: &AppState) -> Result<PathBuf, String> {
    let db_path = PathBuf::from(
        state
            .db
            .lock()
            .map_err(|_| "Database lock poisoned")?
            .path()
            .ok_or("Database has no file path")?,
    );
    let base = db_path
        .parent()
        .ok_or("Database path has no parent")?
        .join("file-history");
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    Ok(base)
}

fn version_dir(state: &AppState, input: &str) -> Result<PathBuf, String> {
    let pid = crate::store::project_id(state)?;
    let dir = history_root(state)?.join(pid).join(sanitize_relative_path(input));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Stores the previous content of a project file before it gets overwritten.
/// Best-effort by design: history failures must never block a save.
pub(crate) fn stash(state: &AppState, input: &str, previous: &str) {
    let Ok(dir) = version_dir(state, input) else { return };
    let stamp = crate::store::now();
    if std::fs::write(dir.join(format!("{stamp}.txt")), previous).is_err() {
        return;
    }
    prune(&dir);
}

fn prune(dir: &PathBuf) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut versions: Vec<(i64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stamp = name.strip_suffix(".txt")?.parse::<i64>().ok()?;
            Some((stamp, entry.path()))
        })
        .collect();
    if versions.len() <= MAX_VERSIONS_PER_FILE {
        return;
    }
    versions.sort_unstable();
    let excess = versions.len() - MAX_VERSIONS_PER_FILE;
    for (_, path) in versions.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

fn version_path(state: &AppState, input: &str, timestamp: i64) -> Result<PathBuf, String> {
    if timestamp <= 0 {
        return Err("Invalid history timestamp".into());
    }
    let dir = version_dir(state, input)?;
    Ok(dir.join(format!("{timestamp}.txt")))
}

/// Caller holds project_gate. Versions are listed newest first.
pub(crate) fn list_locked(state: &AppState, input: &str) -> Result<Vec<FileHistoryEntry>, String> {
    let dir = version_dir(state, input)?;
    let mut versions = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(versions);
    };
    for entry in entries.filter_map(Result::ok) {
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        let Some(stamp) = name.strip_suffix(".txt").and_then(|s| s.parse::<i64>().ok()) else { continue };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        versions.push(FileHistoryEntry { timestamp: stamp, size });
    }
    versions.sort_unstable_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(versions)
}

/// Caller holds project_gate.
pub(crate) fn read_locked(state: &AppState, input: &str, timestamp: i64) -> Result<String, String> {
    let path = version_path(state, input, timestamp)?;
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if content.len() > crate::project::MAX_FILE_BYTES {
        return Err("History version exceeds the 1 MiB limit".into());
    }
    Ok(content)
}

#[tauri::command]
pub fn file_history_list(
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<FileHistoryEntry>, String> {
    if path.trim().is_empty() || path.len() > 1024 {
        return Err("Provide a project-relative file path".into());
    }
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    list_locked(&state, &path)
}

#[tauri::command]
pub fn file_history_read(
    path: String,
    timestamp: i64,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    read_locked(&state, &path, timestamp)
}

/// Restores a version as the new current content. The current text is stashed
/// first, so a restore is itself undoable from the same history list.
#[tauri::command]
pub fn file_history_restore(
    path: String,
    timestamp: i64,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !approved {
        return Err("Restoring a version requires your confirmation".into());
    }
    let content = {
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| "Project gate poisoned")?;
        read_locked(&state, &path, timestamp)?
    };
    crate::project::write_file(&state, &path, &content, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_path_maps_to_same_stable_bucket() {
        let first = sanitize_relative_path("src/agent/mod.rs");
        let second = sanitize_relative_path("src/agent/mod.rs");
        assert_eq!(first, second);
        assert!(first.starts_with("src_agent_mod_rs-"));
        // Hostile names become filesystem-safe buckets without traversal dots.
        for input in ["..\\..\\secret:file?", "../../secret:file?", "///", ""] {
            let bucket = sanitize_relative_path(input);
            assert!(!bucket.contains(".."));
            assert!(bucket.chars().all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_'));
        }
        // Empty and separator-only inputs still get distinct stable buckets.
        assert_eq!(sanitize_relative_path("///"), sanitize_relative_path("///"));
    }
}
