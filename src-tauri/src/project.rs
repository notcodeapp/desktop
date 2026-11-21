use crate::state::{AppState, Project};
use cap_std::fs::{Dir, OpenOptions};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::{
    collections::HashSet,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};
use tauri::State;

pub const MAX_FILE_BYTES: usize = 1024 * 1024;
const SKIPPED: &[&str] = &[".git", "node_modules", "target", "dist"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub path: String,
    pub line: usize,
    pub preview: String,
}

pub fn root(state: &AppState) -> Result<PathBuf, String> {
    state
        .project
        .lock()
        .map_err(|_| "Project lock poisoned")?
        .as_ref()
        .map(|p| PathBuf::from(&p.path))
        .ok_or_else(|| "Open a project first".into())
}

/// Верхняя граница контекста проекта: NOTCODE.md — инструкции пользователя
/// агенту (как CLAUDE.md). Читается из корня проекта перед каждой генерацией.
pub(crate) const NOTCODE_CONTEXT_MAX: usize = 32 * 1024;

/// Читает NOTCODE.md из корня проекта: сначала корневой файл, затем
/// .notcode/notcode.md как альтернативное расположение. Нет файла → "".
pub(crate) fn notcode_context_text(root: &Path) -> String {
    for candidate in ["NOTCODE.md", ".notcode/notcode.md"] {
        if let Ok(value) = std::fs::read_to_string(root.join(candidate)) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.chars().take(NOTCODE_CONTEXT_MAX).collect();
            }
        }
    }
    String::new()
}

fn skipped(name: &str) -> bool {
    SKIPPED.iter().any(|s| name.eq_ignore_ascii_case(s))
}

fn relative(input: &str) -> Result<PathBuf, String> {
    // Reject Windows separators and prefixes even in Unix tests, not just host-native traversal.
    if input.len() > 4096 || input.contains(['\\', ':', '\0']) {
        return Err("Use a project-relative path with forward slashes".into());
    }
    let path = Path::new(input);
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_str().ok_or("Path is not UTF-8")?;
                if skipped(name) || name.ends_with([' ', '.']) {
                    return Err("This path is excluded from workspace access".into());
                }
                clean.push(name);
                // Windows device names do not name normal project files.
                let base = name.split('.').next().unwrap_or("").to_ascii_uppercase();
                if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&base.as_str())
                    || (base.len() == 4
                        && (base.starts_with("COM") || base.starts_with("LPT"))
                        && matches!(base.as_bytes()[3], b'1'..=b'9'))
                {
                    return Err("Device paths are not allowed".into());
                }
            }
            Component::CurDir => {}
            _ => return Err("Path must remain inside the selected project".into()),
        }
    }
    Ok(clean)
}

fn checked(root: &Path, path: &Path, create: bool) -> Result<PathBuf, String> {
    let joined = root.join(path);
    let resolved = if create && !joined.try_exists().map_err(|e| e.to_string())? {
        let parent = joined
            .parent()
            .ok_or("File has no parent")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !parent.starts_with(root) {
            return Err("Symlink escapes the selected project".into());
        }
        parent.join(joined.file_name().ok_or("File name required")?)
    } else {
        joined.canonicalize().map_err(|e| e.to_string())?
    };
    if !resolved.starts_with(root) {
        return Err("Symlink escapes the selected project".into());
    }
    // Also exclude aliases that resolve INTO .git/build/dependency directories.
    let within = resolved.strip_prefix(root).map_err(|e| e.to_string())?;
    if within
        .components()
        .any(|c| skipped(&c.as_os_str().to_string_lossy()))
    {
        return Err("Resolved path is excluded from workspace access".into());
    }
    Ok(resolved)
}

fn directory(state: &AppState) -> Result<Dir, String> {
    state
        .project_dir
        .lock()
        .map_err(|_| "Directory lock poisoned")?
        .as_ref()
        .ok_or("Open a project first")?
        .try_clone()
        .map_err(|e| e.to_string())
}

/// Working directory for agent commands: same traversal and symlink rules as file IO.
pub fn working_directory(state: &AppState, input: &str) -> Result<PathBuf, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    working_directory_locked(state, input)
}

/// Caller holds project_gate (native/local semantics, including permitted symlinks).
pub(crate) fn working_directory_locked(state: &AppState, input: &str) -> Result<PathBuf, String> {
    let root = root(state)?;
    let path = relative(input)?;
    let resolved = checked(&root, &path, false)?;
    if !resolved.is_dir() {
        return Err("Working directory must be an existing folder inside the project".into());
    }
    Ok(resolved)
}

pub fn read_context(state: &AppState, input: &str) -> Result<String, String> {
    let root = root(state)?;
    let path = relative(input)?;
    checked(&root, &path, false)?;
    let dir = directory(state)?;
    let file = dir.open(&path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Path is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err("File exceeds the 1 MiB limit".into());
    }
    if bytes.contains(&0) {
        return Err("Binary files cannot be read as text".into());
    }
    String::from_utf8(bytes).map_err(|_| "File is not UTF-8 text".into())
}

#[tauri::command]
pub async fn project_open(state: State<'_, AppState>) -> Result<Option<Project>, String> {
    // Do not retain any std MutexGuard over the native dialog await.
    let selection = rfd::AsyncFileDialog::new()
        .set_title("Open NotCode project")
        .pick_folder()
        .await;
    match selection {
        Some(folder) => select_project(&state, folder.path()).map(Some),
        None => Ok(None),
    }
}

/// Opens a project by absolute path (recent projects list). User-initiated.
#[tauri::command]
pub async fn project_open_path(path: String, state: State<'_, AppState>) -> Result<Project, String> {
    let candidate = PathBuf::from(path.trim());
    if !candidate.is_dir() {
        return Err("Папка не найдена — проект должен быть папкой".into());
    }
    select_project(&state, &candidate)
}

/// Recently used projects, newest first (recency = last dialog activity).
#[tauri::command]
pub fn projects_recent(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    let mut stmt = db
        .prepare(
            "SELECT p.id,p.path,p.name,MAX(COALESCE(s.updated_at,0)) AS last
             FROM projects p LEFT JOIN sessions s ON s.project_id=p.id
             GROUP BY p.id ORDER BY last DESC, p.name LIMIT 8",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Project { id: row.get(0)?, path: row.get(1)?, name: row.get(2)? })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub(crate) fn select_project(state: &AppState, selected: &Path) -> Result<Project, String> {    let canonical = selected.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.is_dir() {
        return Err("Project must be a directory".into());
    }
    let path = canonical
        .to_str()
        .ok_or("Project path must be UTF-8")?
        .to_owned();
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let mut current = state.project.lock().map_err(|_| "Project lock poisoned")?;
    if let Some(existing) = current.as_ref() {
        if existing.path == path {
            return Ok(existing.clone());
        }
    }
    if !state
        .generations
        .lock()
        .map_err(|_| "Generation lock poisoned")?
        .is_empty()
        || !state
            .terminals
            .lock()
            .map_err(|_| "Terminal lock poisoned")?
            .is_empty()
        || !state
            .mcps
            .lock()
            .map_err(|_| "MCP lock poisoned")?
            .is_empty()
    {
        return Err("Stop all generations, close terminals, and disconnect MCP servers before switching projects".into());
    }
    let dir = Dir::open_ambient_dir(&canonical, cap_std::ambient_authority())
        .map_err(|e| e.to_string())?;
    // Retain this root identity. Remote operations acquire a second, stricter
    // handle to THIS object for their duration; do not change native sharing
    // for the project's lifetime (that would break external local editors).
    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    let id: Option<String> = db
        .query_row("SELECT id FROM projects WHERE path=?1", [&path], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    let project = Project {
        id: id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        name,
        path,
    };
    db.execute("INSERT INTO projects(id,path,name) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET name=excluded.name",
        params![project.id,project.path,project.name]).map_err(|e| e.to_string())?;
    *state
        .project_dir
        .lock()
        .map_err(|_| "Directory lock poisoned")? = Some(dir);
    *current = Some(project.clone());
    // Terminal consent belongs to a TerminalHandle, so requiring an empty terminal map also
    // clears all terminal permission on transition. There is no global remembered approval.
    Ok(project)
}

pub fn initialize_default_project(state: &AppState, path: &Path) -> Result<Project, String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    select_project(state, path)
}

#[tauri::command]
pub fn project_current(state: State<'_, AppState>) -> Result<Option<Project>, String> {
    Ok(state
        .project
        .lock()
        .map_err(|_| "Project lock poisoned")?
        .clone())
}

#[tauri::command]
pub fn project_tree(
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<FileEntry>, String> {
    list_directory(&state, path.as_deref())
}

pub fn list_directory(state: &AppState, input: Option<&str>) -> Result<Vec<FileEntry>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    list_directory_locked(state, input)
}

/// Caller retains project_gate through the entire operation.
pub(crate) fn list_directory_locked(
    state: &AppState,
    input: Option<&str>,
) -> Result<Vec<FileEntry>, String> {
    let root = root(&state)?;
    let path = relative(input.unwrap_or(""))?;
    checked(&root, &path, false)?;
    let dir = directory(&state)?;
    let mut out = Vec::new();
    for entry in dir
        .read_dir(if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &path
        })
        .map_err(|e| e.to_string())?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        if skipped(&name) {
            continue;
        }
        let child = path.join(&name);
        if relative(&child.to_string_lossy().replace('\\', "/")).is_err()
            || checked(&root, &child, false).is_err()
        {
            continue;
        }
        let metadata = match dir.metadata(&child) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_dir() && !metadata.is_file() {
            continue;
        }
        out.push(FileEntry {
            name,
            path: child.to_string_lossy().replace('\\', "/"),
            is_dir: metadata.is_dir(),
        });
        if out.len() >= 2000 {
            break;
        }
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

#[tauri::command]
pub fn file_read(path: String, state: State<'_, AppState>) -> Result<String, String> {
    read_file(&state, &path)
}

pub fn read_file(state: &AppState, path: &str) -> Result<String, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    read_context(state, path)
}

#[tauri::command]
pub fn file_write(path: String, content: String, state: State<'_, AppState>) -> Result<(), String> {
    write_file(&state, &path, &content, true)
}

pub fn create_directory(state: &AppState, input: &str) -> Result<(), String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    create_directory_locked(state, input)
}

pub(crate) fn create_directory_locked(state: &AppState, input: &str) -> Result<(), String> {
    let root = root(state)?;
    let path = relative(input)?;
    if path.as_os_str().is_empty() {
        return Err("Choose a directory inside the project".into());
    }
    let dir = directory(state)?;
    dir.create_dir_all(&path).map_err(|e| e.to_string())?;
    checked(&root, &path, false)?;
    Ok(())
}

pub fn create_file(state: &AppState, input: &str) -> Result<(), String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    create_file_locked(state, input)
}

pub(crate) fn create_file_locked(state: &AppState, input: &str) -> Result<(), String> {
    let root = root(state)?;
    let path = relative(input)?;
    if path.as_os_str().is_empty() {
        return Err("Choose a file path inside the project".into());
    }
    checked(&root, &path, true)?;
    let dir = directory(state)?;
    let file = dir
        .open_with(&path, OpenOptions::new().write(true).create_new(true))
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn write_existing_file(state: &AppState, input: &str, content: &str) -> Result<(), String> {
    write_file(state, input, content, false)
}

pub(crate) fn write_existing_file_locked(
    state: &AppState,
    input: &str,
    content: &str,
) -> Result<(), String> {
    write_file_locked(state, input, content, false)
}

// Short aliases for the agent's native tool dispatch.
pub(crate) fn write_existing_locked(
    state: &AppState,
    input: &str,
    content: &str,
) -> Result<(), String> {
    write_existing_file_locked(state, input, content)
}

pub(crate) fn write_file(
    state: &AppState,
    input: &str,
    content: &str,
    allow_create: bool,
) -> Result<(), String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    write_file_locked(state, input, content, allow_create)
}

pub(crate) fn write_file_locked(
    state: &AppState,
    input: &str,
    content: &str,
    allow_create: bool,
) -> Result<(), String> {
    if content.len() > MAX_FILE_BYTES {
        return Err("File exceeds the 1 MiB limit".into());
    }
    let root = root(&state)?;
    let path = relative(input)?;
    if path.as_os_str().is_empty() {
        return Err("Choose a file path inside the project".into());
    }
    if !allow_create {
        let resolved = checked(&root, &path, false)?;
        if !resolved.is_file() {
            return Err("Only existing regular files can be written; create the file first".into());
        }
    } else {
        checked(&root, &path, true)?;
    }
    // Local history: snapshot the previous content before it is replaced, so any
    // save (user or approved agent write) is recoverable. Best-effort.
    if let Ok(previous) = read_context(&state, input) {
        if previous != content {
            crate::history::stash(&state, input, &previous);
        }
    }
    let dir = directory(&state)?;
    // Capability-relative IO enforces beneath-root resolution even if a symlink changes
    // after canonical validation. Never use std::fs::write on the checked path.
    let mut options = OpenOptions::new();
    options.write(true).truncate(true);
    if allow_create {
        options.create(true);
    }
    let mut file = dir.open_with(&path, &options).map_err(|e| e.to_string())?;
    file.write_all(content.as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    if !allow_create && read_context(state, input)? != content {
        return Err("The written file did not match the requested content".into());
    }
    Ok(())
}

#[tauri::command]
pub fn project_search(query: String, state: State<'_, AppState>) -> Result<Vec<SearchHit>, String> {
    search_files(&state, &query)
}

/// Результат замены: сколько файлов изменено, сколько вхождений заменено,
/// сколько файлов пропущено (внешние ошибки записи).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceResult {
    pub files_changed: usize,
    pub replacements: usize,
    pub skipped: usize,
}

const REPLACE_MAX_FILES: usize = 100;

#[tauri::command]
pub fn project_replace(query: String, replacement: String, state: State<'_, AppState>) -> Result<ReplaceResult, String> {
    if query.trim().is_empty() || query.len() > 256 {
        return Err("Search needs 1-256 bytes of text".into());
    }
    if replacement.len() > 4096 {
        return Err("Replacement is limited to 4096 bytes".into());
    }
    if query == replacement {
        return Err("Search and replacement are identical".into());
    }
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let root = root(&state)?;
    let dir = directory(&state)?;
    let started = Instant::now();
    let mut pending = vec![PathBuf::new()];
    let mut seen = HashSet::new();
    let mut candidates: Vec<String> = Vec::new();
    let mut visited = 0usize;
    let mut bytes_read = 0usize;
    // Фаза 1: тот же bounded-обход, что и поиск, собираем файлы с вхождениями.
    while let Some(path) = pending.pop() {
        let canonical = match checked(&root, &path, false) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !seen.insert(canonical) {
            continue;
        }
        let entries = match dir.read_dir(if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &path
        }) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            if visited >= 20_000
                || candidates.len() >= REPLACE_MAX_FILES
                || bytes_read >= 16 * 1024 * 1024
                || started.elapsed() > Duration::from_secs(3)
            {
                break;
            }
            visited += 1;
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            if skipped(&name) {
                continue;
            }
            let child = path.join(name);
            if relative(&child.to_string_lossy().replace('\\', "/")).is_err()
                || checked(&root, &child, false).is_err()
            {
                continue;
            }
            let metadata = match dir.metadata(&child) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                pending.push(child);
                continue;
            }
            if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
                continue;
            }
            let file = match dir.open(&child) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut bytes = Vec::new();
            if file
                .take((MAX_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .is_err()
            {
                continue;
            }
            bytes_read += bytes.len();
            if bytes.len() > MAX_FILE_BYTES || bytes.contains(&0) {
                continue;
            }
            let text = match String::from_utf8(bytes) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if text.contains(&query) {
                candidates.push(child.to_string_lossy().replace('\\', "/"));
            }
        }
        if candidates.len() >= REPLACE_MAX_FILES
            || bytes_read >= 16 * 1024 * 1024
            || started.elapsed() > Duration::from_secs(3)
        {
            break;
        }
    }
    // Фаза 2: замена через штатную запись (история сохраняется автоматически).
    let mut result = ReplaceResult { files_changed: 0, replacements: 0, skipped: 0 };
    for path in &candidates {
        let previous = match read_context(&state, path) {
            Ok(t) => t,
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        let count = previous.matches(&query).count();
        if count == 0 {
            continue;
        }
        let updated = previous.replace(&query, &replacement);
        match write_file_locked(&state, path, &updated, false) {
            Ok(()) => {
                result.files_changed += 1;
                result.replacements += count;
            }
            Err(_) => {
                result.skipped += 1;
            }
        }
    }
    Ok(result)
}

/// Flat recursive file listing for the quick-open switcher. Same exclusions and
/// safety checks as search, bounded so a huge checkout cannot stall the UI.
#[tauri::command]
pub fn project_files_all(state: State<'_, AppState>) -> Result<Vec<FileEntry>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    list_all_files_locked(&state)
}

pub(crate) fn list_all_files_locked(state: &AppState) -> Result<Vec<FileEntry>, String> {
    const MAX_FILES: usize = 5_000;
    const MAX_VISITED: usize = 30_000;
    const MAX_DEPTH: usize = 12;
    let root = root(state)?;
    let dir = directory(state)?;
    let mut pending = vec![PathBuf::new()];
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut visited = 0usize;
    let mut exhausted = false;
    while !exhausted {
        let Some(path) = pending.pop() else { break };
        let canonical = match checked(&root, &path, false) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !seen.insert(canonical) {
            continue;
        }
        let entries = match dir.read_dir(if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &path
        }) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            if files.len() >= MAX_FILES || visited >= MAX_VISITED {
                exhausted = true;
                break;
            }
            visited += 1;
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            if skipped(&name) || name.starts_with('.') {
                continue;
            }
            let child = path.join(&name);
            let rel = child.to_string_lossy().replace('\\', "/");
            if relative(&rel).is_err() || checked(&root, &child, false).is_err() {
                continue;
            }
            let metadata = match dir.metadata(&child) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                if rel.matches('/').count() < MAX_DEPTH {
                    pending.push(child);
                }
                continue;
            }
            if metadata.is_file() {
                files.push(FileEntry {
                    name,
                    path: rel,
                    is_dir: false,
                });
            }
        }
    }
    files.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    Ok(files)
}

/// User-initiated directory creation (quick-open exports, user scripts); unlike
/// the agent tool this needs no approval prompt because the user asked for it.
#[tauri::command]
pub fn project_ensure_dir(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    create_directory_locked(&state, &path)
}

pub fn search_files(state: &AppState, query: &str) -> Result<Vec<SearchHit>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    search_files_locked(state, query)
}

pub(crate) fn search_files_locked(state: &AppState, query: &str) -> Result<Vec<SearchHit>, String> {
    if query.trim().is_empty() || query.len() > 256 {
        return Err("Search needs 1–256 bytes of text".into());
    }
    let root = root(state)?;
    let dir = directory(state)?;
    let started = Instant::now();
    let mut pending = vec![PathBuf::new()];
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    let mut visited = 0;
    let mut bytes_read = 0;
    while let Some(path) = pending.pop() {
        let canonical = match checked(&root, &path, false) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !seen.insert(canonical) {
            continue;
        }
        let entries = match dir.read_dir(if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &path
        }) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            if visited >= 20_000
                || hits.len() >= 500
                || bytes_read >= 16 * 1024 * 1024
                || started.elapsed() > Duration::from_secs(3)
            {
                return Ok(hits);
            }
            visited += 1;
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            if skipped(&name) {
                continue;
            }
            let child = path.join(name);
            if relative(&child.to_string_lossy().replace('\\', "/")).is_err()
                || checked(&root, &child, false).is_err()
            {
                continue;
            }
            let metadata = match dir.metadata(&child) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                pending.push(child);
                continue;
            }
            if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
                continue;
            }
            let file = match dir.open(&child) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut bytes = Vec::new();
            if file
                .take((MAX_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .is_err()
            {
                continue;
            }
            bytes_read += bytes.len();
            if bytes.len() > MAX_FILE_BYTES || bytes.contains(&0) {
                continue;
            }
            let text = match String::from_utf8(bytes) {
                Ok(t) => t,
                Err(_) => continue,
            };
            for (line, text) in text.lines().enumerate() {
                if text.contains(query) {
                    hits.push(SearchHit {
                        path: child.to_string_lossy().replace('\\', "/"),
                        line: line + 1,
                        preview: text.chars().take(240).collect(),
                    });
                    if hits.len() >= 500 {
                        return Ok(hits);
                    }
                }
            }
        }
    }
    Ok(hits)
}

/// Expected-project operations never release/reacquire the project gate.
pub(crate) fn with_expected<T>(
    state: &AppState,
    expected: &str,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    expect_locked(state, expected)?;
    work()
}

pub(crate) fn expect_locked(state: &AppState, expected: &str) -> Result<(), String> {
    if crate::store::project_id(state)? != expected {
        return Err("Bridge project changed".into());
    }
    Ok(())
}

pub(crate) fn bridge_path(input: &str) -> Result<PathBuf, String> {
    let path = relative(input)?;
    if path.components().any(|c| {
        let n = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        [
            ".keys",
            ".ssh",
            ".aws",
            "credentials.json",
            "vendor",
            ".venv",
            "venv",
            "build",
            ".gradle",
            "pods",
            ".next",
        ]
        .contains(&n.as_str())
            || n == ".env"
            || n.starts_with(".env.")
    }) {
        return Err("Private files are not shared".into());
    }
    Ok(path)
}

// Remote IO is separate from native IO: cap-std 3.4.6 follows in-root aliases;
// on Windows entries/create_dir even use get_path + std::fs internally. Instead
// open ONE named component relative to a retained handle and validate after open.
const SHARED_DEPTH: usize = 128;
const SHARED_ENTRIES: usize = 20_000;
const SHARED_UNSUPPORTED: &str = "Secure remote filesystem operation unsupported on this platform";

pub(crate) fn list_locked(state: &AppState, input: &str) -> Result<Vec<FileEntry>, String> {
    list_directory_locked(state, Some(input))
}
pub(crate) fn search_locked(state: &AppState, input: &str) -> Result<Vec<SearchHit>, String> {
    search_files_locked(state, input)
}

fn shared_root(state: &AppState) -> Result<Dir, String> {
    let dir = directory(state).map_err(|_| "Project directory unavailable")?;
    let dir = shared_fs::retain_root(dir)?;
    shared_fs::local(&dir)?;
    Ok(dir)
}

fn shared_directories(state: &AppState, path: &Path, create: bool) -> Result<Vec<Dir>, String> {
    if path.components().count() > SHARED_DEPTH {
        return Err("Directory depth limit".into());
    }
    let mut held = vec![shared_root(state)?];
    for component in path.components() {
        let name = component.as_os_str().to_str().ok_or("Path unavailable")?;
        let parent = held.last().ok_or("Directory unavailable")?;
        let file = shared_fs::open_directory(parent, name, create)?;
        if !shared_fs::validate(&file)?.is_dir() {
            return Err("Directory required".into());
        }
        held.push(Dir::from_std_file(file));
    }
    Ok(held)
}

fn shared_file(
    state: &AppState,
    input: &str,
    write: bool,
    create: bool,
) -> Result<(PathBuf, Vec<Dir>, std::fs::File), String> {
    let path = bridge_path(input)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("File path required")?;
    let held = shared_directories(state, path.parent().ok_or("File path required")?, false)?;
    let file = shared_fs::open(
        held.last().ok_or("Directory unavailable")?,
        name,
        write,
        create,
        false,
    )?;
    if !shared_fs::validate(&file)?.is_file() {
        return Err("Regular file required".into());
    }
    Ok((path, held, file))
}

fn shared_text(file: &mut std::fs::File) -> Result<String, String> {
    if !shared_fs::validate(file)?.is_file() {
        return Err("Regular file required".into());
    }
    let mut bytes = Vec::new();
    (&mut *file)
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "File unavailable")?;
    shared_fs::validate(file)?;
    if bytes.len() > MAX_FILE_BYTES || bytes.contains(&0) {
        return Err("Invalid text file".into());
    }
    String::from_utf8(bytes).map_err(|_| "Invalid text file".into())
}

/// All bridge *_locked APIs require caller-held project_gate and authorization.
pub(crate) fn bridge_list_locked(state: &AppState, input: &str) -> Result<Vec<FileEntry>, String> {
    let path = bridge_path(input)?;
    let held = shared_directories(state, &path, false)?;
    let dir = held.last().ok_or("Directory unavailable")?;
    let mut out = Vec::new();
    for name in shared_fs::names(dir, SHARED_ENTRIES)? {
        let name = match name {
            Ok(n) => n,
            Err(_) => continue,
        };
        let label = path.join(&name).to_string_lossy().replace('\\', "/");
        if bridge_path(&label).is_err() {
            continue;
        }
        let file = match shared_fs::open(dir, &name, false, false, false) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let metadata = match shared_fs::validate(&file) {
            Ok(m) => m,
            Err(_) => continue,
        };
        out.push(FileEntry {
            name,
            path: label,
            is_dir: metadata.is_dir(),
        });
        if out.len() >= 2000 {
            break;
        }
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

pub(crate) fn bridge_create_directory_locked(state: &AppState, input: &str) -> Result<(), String> {
    let path = bridge_path(input)?;
    if path.as_os_str().is_empty() {
        return Err("Directory path required".into());
    }
    // FILE_OPEN_IF | FILE_DIRECTORY_FILE creates and retains each component
    // atomically. No ambient create_dir_all or missing-parent fallback.
    shared_directories(state, &path, true)?;
    Ok(())
}

pub(crate) fn bridge_create_file_locked(state: &AppState, input: &str) -> Result<(), String> {
    bridge_cas_locked(state, input, "", None).map(|_| ())
}

pub(crate) fn bridge_write_existing_locked(
    state: &AppState,
    input: &str,
    content: &str,
) -> Result<(), String> {
    let current = bridge_read_locked(state, input)?;
    // Exclusive CAS re-reads/compares the actual object it will write. There is
    // no fallback to the native unrestricted writer, even on unsupported OSes.
    bridge_cas_locked(state, input, content, Some(&current.hash)).map(|_| ())
}

struct SharedFrame {
    dir: Dir,
    path: PathBuf,
    names: std::vec::IntoIter<Result<String, String>>,
}
struct SharedWalk {
    stack: Vec<SharedFrame>,
    visited: usize,
    collected: usize,
    names_bytes: usize,
    started: Instant,
    seconds: u64,
    limit: usize,
    done: bool,
}
fn names_size(names: &[Result<String, String>]) -> usize {
    names
        .iter()
        .map(|n| n.as_ref().map_or(64, |n| n.len() + 64))
        .sum()
}
impl SharedWalk {
    fn new(state: &AppState, limit: usize, seconds: u64) -> Result<Self, String> {
        let dir = shared_root(state)?;
        let names = shared_fs::names(&dir, limit)?;
        let collected = names.len();
        let names_bytes = names_size(&names);
        Ok(Self {
            stack: vec![SharedFrame {
                dir,
                path: PathBuf::new(),
                names: names.into_iter(),
            }],
            visited: 0,
            collected,
            names_bytes,
            started: Instant::now(),
            seconds,
            limit,
            done: false,
        })
    }
    // Ancestors stay on this DFS stack until their children finish. Never
    // replace retained directory capabilities with a worklist of paths.
    fn next(&mut self) -> Option<(String, Result<std::fs::File, String>)> {
        if self.done {
            return None;
        }
        loop {
            if self.visited >= self.limit
                || self.started.elapsed() > Duration::from_secs(self.seconds)
            {
                self.done = true;
                return Some((
                    "[walk truncated: entry or time limit]".into(),
                    Err("Walk limit".into()),
                ));
            }
            let frame = self.stack.last_mut()?;
            let name = match frame.names.next() {
                Some(n) => n,
                None => {
                    self.stack.pop();
                    continue;
                }
            };
            self.visited += 1;
            let name = match name {
                Ok(n) => n,
                Err(e) => return Some(("[unreadable entry]".into(), Err(e))),
            };
            let child = frame.path.join(&name);
            let label = child.to_string_lossy().replace('\\', "/");
            if bridge_path(&label).is_err() {
                return Some((label, Err("Excluded path".into())));
            }
            let file = match shared_fs::open(&frame.dir, &name, false, false, false) {
                Ok(f) => f,
                Err(e) => return Some((label, Err(e))),
            };
            let metadata = match shared_fs::validate(&file) {
                Ok(m) => m,
                Err(e) => return Some((label, Err(e))),
            };
            if metadata.is_dir() {
                if self.stack.len() >= SHARED_DEPTH {
                    return Some((label, Err("Directory depth limit".into())));
                }
                let dir = Dir::from_std_file(file);
                if self.collected >= self.limit {
                    return Some((label, Err("Walk entry limit".into())));
                }
                let names = match shared_fs::names(&dir, self.limit - self.collected) {
                    Ok(n) => n,
                    Err(e) => return Some((label, Err(e))),
                };
                self.collected += names.len();
                self.names_bytes += names_size(&names);
                if self.names_bytes > 4 * 1024 * 1024 {
                    return Some((label, Err("Walk metadata limit".into())));
                }
                self.stack.push(SharedFrame {
                    dir,
                    path: child,
                    names: names.into_iter(),
                });
                continue;
            }
            return Some((label, Ok(file)));
        }
    }
}

pub(crate) fn bridge_search_locked(
    state: &AppState,
    query: &str,
) -> Result<Vec<SearchHit>, String> {
    if query.trim().is_empty() || query.len() > 256 {
        return Err("Search needs 1–256 bytes of text".into());
    }
    let mut walk = SharedWalk::new(state, SHARED_ENTRIES, 3)?;
    let mut hits = Vec::new();
    let mut bytes_read = 0usize;
    while let Some((path, file)) = walk.next() {
        let mut file = match file {
            Ok(f) => f,
            Err(_) => continue,
        };
        // Charge failures too; oversized/binary files must not evade IO limits.
        bytes_read += MAX_FILE_BYTES + 1;
        if bytes_read > 16 * 1024 * 1024 {
            break;
        }
        let text = match shared_text(&mut file) {
            Ok(t) => t,
            Err(_) => continue,
        };
        bytes_read -= MAX_FILE_BYTES + 1 - text.len();
        for (line, text) in text.lines().enumerate() {
            if text.contains(query) {
                hits.push(SearchHit {
                    path: path.clone(),
                    line: line + 1,
                    preview: text.chars().take(240).collect(),
                });
                if hits.len() >= 500 {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}

/// Legacy check-then-open cannot return an authorization safely. Keep the
/// symbol temporarily for bridge.rs compatibility, but fail closed until its
/// caller uses bridge_list_locked or another retained-handle operation.
pub(crate) fn bridge_check_locked(
    _state: &AppState,
    input: &str,
    _create: bool,
) -> Result<PathBuf, String> {
    bridge_path(input)?;
    Err("Secure bridge operation requires a retained-handle API".into())
}

pub(crate) fn text_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Serialize)]
pub(crate) struct BridgeFile {
    pub path: String,
    pub content: String,
    pub hash: String,
}

/// Caller holds project_gate and checked expected project.
pub(crate) fn bridge_read_locked(state: &AppState, input: &str) -> Result<BridgeFile, String> {
    let (path, _held, mut file) = shared_file(state, input, false, false)?;
    let content = shared_text(&mut file)?;
    let hash = text_hash(content.as_bytes());
    Ok(BridgeFile {
        path: path.to_string_lossy().replace('\\', "/"),
        content,
        hash,
    })
}

/// Filesystem-enforced CAS, NOT an advisory lock. The exclusive open is acquired
/// BEFORE reading/comparing, without truncation; parents remain held until sync.
/// Unix remote mutation fails closed: flock cannot exclude noncooperating editors.
pub(crate) fn bridge_cas_locked(
    state: &AppState,
    input: &str,
    content: &str,
    expected: Option<&str>,
) -> Result<BridgeFile, String> {
    use std::io::{Seek, SeekFrom};
    if content.len() > MAX_FILE_BYTES || content.contains('\0') {
        return Err("Invalid text file".into());
    }
    if expected.is_some_and(|h| h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err("Invalid file hash".into());
    }
    let (path, _held, mut file) = shared_file(state, input, true, expected.is_none())?;
    if let Some(hash) = expected {
        let current = shared_text(&mut file)?;
        if hash != text_hash(current.as_bytes()) {
            return Err("Stale file hash".into());
        }
    }
    shared_fs::validate(&file)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "File unavailable")?;
    // Do not truncate on open, nor truncate a stale/invalid/aliased file.
    file.write_all(content.as_bytes())
        .map_err(|_| "File unavailable")?;
    file.set_len(content.len() as u64)
        .map_err(|_| "File unavailable")?;
    file.sync_all().map_err(|_| "File unavailable")?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "File unavailable")?;
    if shared_text(&mut file)? != content {
        return Err("Written file verification failed".into());
    }
    Ok(BridgeFile {
        path: path.to_string_lossy().replace('\\', "/"),
        content: content.into(),
        hash: text_hash(content.as_bytes()),
    })
}

#[derive(Serialize)]
pub(crate) struct SnapshotFile {
    path: String,
    hash: String,
    size: usize,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BridgeSnapshot {
    project_id: String,
    files: Vec<SnapshotFile>,
    skipped: Vec<String>,
    truncated: bool,
}

pub(crate) fn bridge_snapshot_locked(
    state: &AppState,
    pid: &str,
) -> Result<BridgeSnapshot, String> {
    let mut walk = SharedWalk::new(state, 10_000, 5)?;
    let mut files = Vec::new();
    let mut skipped_paths = Vec::new();
    let mut total = 0usize;
    let mut metadata_bytes = 0usize;
    let mut truncated = false;
    while let Some((label, file)) = walk.next() {
        if label == "[walk truncated: entry or time limit]" {
            truncated = true;
            continue;
        }
        metadata_bytes = metadata_bytes.saturating_add(label.len().saturating_mul(6) + 256);
        if metadata_bytes > 4 * 1024 * 1024 || total + MAX_FILE_BYTES + 1 > 32 * 1024 * 1024 {
            truncated = true;
            break;
        }
        let mut file = match file {
            Ok(f) => f,
            Err(_) => {
                skipped_paths.push(label);
                continue;
            }
        };
        // Same already-opened file, never bridge_read(path) after a separate check.
        match shared_text(&mut file) {
            Ok(text) => {
                total += text.len();
                files.push(SnapshotFile {
                    path: label,
                    hash: text_hash(text.as_bytes()),
                    size: text.len(),
                });
            }
            Err(_) => {
                total += MAX_FILE_BYTES + 1;
                skipped_paths.push(label);
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    skipped_paths.sort();
    Ok(BridgeSnapshot {
        project_id: pid.into(),
        files,
        skipped: skipped_paths,
        truncated,
    })
}

// Minimal native bindings avoid changing Cargo.toml. Layouts/values/signatures
// checked against installed windows-sys 0.59/0.60, cap-primitives 3.4.6 and
// io-extras 0.18.4. cap_std::Dir implements std AsRawHandle/AsRawFd directly;
// io-extras is not required as a direct dependency to borrow these handles.
#[cfg(windows)]
mod shared_fs {
    use super::*;
    use std::{
        ffi::c_void,
        fs::{File, Metadata},
        mem,
        os::windows::io::{AsRawHandle, FromRawHandle},
        ptr,
    };
    type Handle = *mut c_void;
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }
    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root: Handle,
        name: *mut UnicodeString,
        attributes: u32,
        security: *mut c_void,
        qos: *mut c_void,
    }
    #[repr(C)]
    struct IoStatus {
        status: usize,
        information: usize,
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn NtCreateFile(
            handle: *mut Handle,
            access: u32,
            attributes: *mut ObjectAttributes,
            status: *mut IoStatus,
            allocation: *mut i64,
            file_attributes: u32,
            sharing: u32,
            disposition: u32,
            options: u32,
            ea: *mut c_void,
            ea_len: u32,
        ) -> i32;
        fn NtQueryDirectoryFile(
            handle: Handle,
            event: Handle,
            apc: *mut c_void,
            context: *mut c_void,
            status: *mut IoStatus,
            buffer: *mut c_void,
            length: u32,
            class: u32,
            single: u8,
            name: *mut UnicodeString,
            restart: u8,
        ) -> i32;
        fn NtQueryVolumeInformationFile(
            handle: Handle,
            status: *mut IoStatus,
            buffer: *mut c_void,
            length: u32,
            class: u32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(handle: Handle, info: *mut u32) -> i32;
        fn GetFileInformationByHandleEx(
            handle: Handle,
            class: u32,
            info: *mut c_void,
            length: u32,
        ) -> i32;
    }
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000; // NT equivalent of FILE_FLAG_OPEN_REPARSE_POINT
    const SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
    const DIRECTORY_FILE: u32 = 1; // directory semantics; not FILE_NON_DIRECTORY_FILE
    const NON_DIRECTORY_FILE: u32 = 0x40;
    const GENERIC_FILE_READ: u32 = 0x0012_0089;
    const GENERIC_FILE_WRITE: u32 = 0x0012_0116;
    const SHARE_READ: u32 = 1;
    const REPARSE_ATTRIBUTE: u32 = 0x400;

    fn nt_error(status: i32) -> String {
        // Never send OS errors (which can include ambient paths) to a device.
        match unsafe { RtlNtStatusToDosError(status) } {
            2 | 3 => "Path not found",
            80 | 183 => "Stale file hash",
            32 | 33 => "File busy",
            _ => "File unavailable",
        }
        .into()
    }
    fn information(handle: Handle) -> Result<[u32; 13], String> {
        // BY_HANDLE_FILE_INFORMATION: attributes; three FILETIMEs; volume;
        // size high/low; link count; index high/low (all DWORDs).
        let mut info = [0u32; 13];
        if unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) } == 0 {
            return Err("File unavailable".into());
        }
        Ok(info)
    }
    fn identity(info: &[u32; 13]) -> (u32, u32, u32) {
        (info[7], info[11], info[12])
    }

    fn open_at(
        parent: &Dir,
        name: &str,
        write: bool,
        create: bool,
        mkdir: bool,
        directory: bool,
    ) -> Result<File, String> {
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        let length: u16 = (wide.len() * 2)
            .try_into()
            .map_err(|_| "Path unavailable")?;
        let mut unicode = UnicodeString {
            length,
            maximum_length: length,
            buffer: wide.as_mut_ptr(),
        };
        let mut attributes = ObjectAttributes {
            length: mem::size_of::<ObjectAttributes>() as u32,
            root: parent.as_raw_handle(),
            name: &mut unicode,
            attributes: 0x40 | 0x1000, // OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE
            security: ptr::null_mut(),
            qos: ptr::null_mut(),
        };
        let mut status = IoStatus {
            status: 0,
            information: 0,
        };
        let mut handle = ptr::null_mut();
        let access = GENERIC_FILE_READ | if write { GENERIC_FILE_WRITE } else { 0 };
        // Directory/read handles deny ordinary WRITE as well as DELETE. Some
        // metadata-only handles can still set reparse data: OBJ_DONT_REPARSE
        // plus handle-relative lookup prevents that mutation from redirecting IO.
        // CAS share_mode(0) is enforced by the filesystem against existing AND
        // future read/write/delete handles, not a cooperating-editor lock.
        let sharing = if write { 0 } else { SHARE_READ };
        let disposition = if mkdir {
            3 /* FILE_OPEN_IF */
        } else if create {
            2 /* FILE_CREATE */
        } else {
            1 /* FILE_OPEN */
        };
        let options = OPEN_REPARSE_POINT
            | SYNCHRONOUS_IO_NONALERT
            | if directory {
                DIRECTORY_FILE
            } else if write {
                NON_DIRECTORY_FILE
            } else {
                0
            };
        // SAFETY: all counted-string/object/IO buffers live through this synchronous
        // call; the parent is borrowed for the call; success transfers ONE handle
        // into File's RAII ownership. No raw handle is duplicated or leaked.
        if information(parent.as_raw_handle())?[0] & REPARSE_ATTRIBUTE != 0 {
            return Err("Aliases are not shared".into());
        }
        let result = unsafe {
            NtCreateFile(
                &mut handle,
                access,
                &mut attributes,
                &mut status,
                ptr::null_mut(),
                0x80,
                sharing,
                disposition,
                options,
                ptr::null_mut(),
                0,
            )
        };
        if result != 0 {
            return Err(nt_error(result));
        }
        let file = unsafe { File::from_raw_handle(handle) };
        if information(parent.as_raw_handle())?[0] & REPARSE_ATTRIBUTE != 0 {
            return Err("Aliases are not shared".into());
        }
        Ok(file)
    }

    pub(super) fn validate(file: &File) -> Result<Metadata, String> {
        let info = information(file.as_raw_handle())?;
        // Reject ALL reparse tags, not just symlinks/junctions known to Rust.
        if info[0] & REPARSE_ATTRIBUTE != 0 {
            return Err("Aliases are not shared".into());
        }
        let metadata = file.metadata().map_err(|_| "File unavailable")?;
        if !metadata.is_dir() && !metadata.is_file() {
            return Err("Regular file or directory required".into());
        }
        if metadata.is_file() && info[10] != 1 {
            return Err("Multi-link files are not shared".into());
        }
        Ok(metadata)
    }

    pub(super) fn retain_root(dir: Dir) -> Result<Dir, String> {
        let original = information(dir.as_raw_handle())?;
        if original[0] & REPARSE_ATTRIBUTE != 0 {
            return Err("Aliases are not shared".into());
        }
        // A zero-length NT name addresses RootDirectory ITSELF, not its path.
        // Literal '.' is Win32 syntax, and ReOpenFile may reject directories.
        let file = open_at(&dir, "", false, false, false, true)?;
        if identity(&original) != identity(&information(file.as_raw_handle())?)
            || !validate(&file)?.is_dir()
        {
            return Err("Project directory identity changed".into());
        }
        Ok(Dir::from_std_file(file))
    }

    pub(super) fn local(dir: &Dir) -> Result<(), String> {
        let mut status = IoStatus {
            status: 0,
            information: 0,
        };
        let mut device = [0u32; 2]; // FILE_FS_DEVICE_INFORMATION
        let result = unsafe {
            NtQueryVolumeInformationFile(
                dir.as_raw_handle(),
                &mut status,
                device.as_mut_ptr().cast(),
                8,
                4,
            )
        };
        // Do not assume network providers honor local-disk sharing semantics.
        if result != 0 || device[0] != 7 || device[1] & 0x10 != 0 {
            return Err(SHARED_UNSUPPORTED.into());
        }
        Ok(())
    }

    pub(super) fn open_directory(parent: &Dir, name: &str, create: bool) -> Result<File, String> {
        bridge_path(name)?;
        if Path::new(name).components().count() != 1 || name == "." {
            return Err("Directory name required".into());
        }
        // Known directory components use FILE_DIRECTORY_FILE as well as
        // OPEN_REPARSE_POINT, not a followed open followed by is_dir(path).
        let file = open_at(parent, name, false, false, create, true)?;
        validate(&file)?;
        long_name(&file, name)?;
        Ok(file)
    }

    fn long_name(file: &File, requested: &str) -> Result<(), String> {
        // FILE_NAME_INFO is read from the opened handle, so 8.3 names cannot
        // alias excluded long names (e.g. CREDEN~1.JSON -> credentials.json).
        let mut buffer = vec![0u32; 16_385];
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                2,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 4) as u32,
            )
        } == 0
        {
            return Err("File name unavailable".into());
        }
        let bytes = buffer[0] as usize;
        if bytes % 2 != 0 || bytes > buffer.len() * 4 - 4 {
            return Err("File name unavailable".into());
        }
        let wide =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().add(1).cast::<u16>(), bytes / 2) };
        let actual = String::from_utf16(wide).map_err(|_| "File name unavailable")?;
        let actual = actual.rsplit('\\').next().ok_or("File name unavailable")?;
        bridge_path(actual)?;
        if actual.to_lowercase() != requested.to_lowercase() {
            return Err("Aliases are not shared".into());
        }
        Ok(())
    }
    pub(super) fn open(
        parent: &Dir,
        name: &str,
        write: bool,
        create: bool,
        mkdir: bool,
    ) -> Result<File, String> {
        bridge_path(name)?;
        if Path::new(name).components().count() != 1 || name == "." {
            return Err("File name required".into());
        }
        let file = open_at(parent, name, write, create, mkdir, mkdir)?;
        validate(&file)?;
        long_name(&file, name)?;
        Ok(file)
    }

    pub(super) fn names(dir: &Dir, limit: usize) -> Result<Vec<Result<String, String>>, String> {
        if information(dir.as_raw_handle())?[0] & REPARSE_ATTRIBUTE != 0 {
            return Err("Aliases are not shared".into());
        }
        let mut out = Vec::new();
        let mut buffer = vec![0u64; 8192]; // aligned native output buffer
        let started = Instant::now();
        let mut restart = 1u8;
        let mut metadata_bytes = 0usize;
        loop {
            let mut status = IoStatus {
                status: 0,
                information: 0,
            };
            // FileNamesInformation: NextEntryOffset, FileIndex, FileNameLength,
            // followed by counted UTF-16. Metadata here is NEVER trusted.
            let result = unsafe {
                NtQueryDirectoryFile(
                    dir.as_raw_handle(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut status,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                    12,
                    0,
                    ptr::null_mut(),
                    restart,
                )
            };
            restart = 0;
            if result as u32 == 0x8000_0006 {
                return Ok(out);
            } // STATUS_NO_MORE_FILES
            if result != 0 {
                return Err("Directory unavailable".into());
            }
            if status.information == 0 || status.information > buffer.len() * 8 {
                return Err("Directory unavailable".into());
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), status.information)
            };
            let mut offset = 0usize;
            loop {
                if offset + 12 > bytes.len() {
                    return Err("Directory unavailable".into());
                }
                let next = u32::from_le_bytes(
                    bytes[offset..offset + 4]
                        .try_into()
                        .map_err(|_| "Directory unavailable")?,
                ) as usize;
                let length = u32::from_le_bytes(
                    bytes[offset + 8..offset + 12]
                        .try_into()
                        .map_err(|_| "Directory unavailable")?,
                ) as usize;
                if length % 2 != 0 || length > 8192 || offset + 12 + length > bytes.len() {
                    return Err("Directory unavailable".into());
                }
                let wide: Vec<u16> = bytes[offset + 12..offset + 12 + length]
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect();
                let name = String::from_utf16(&wide).map_err(|_| "Non-UTF-8 entry".into());
                if !matches!(name.as_deref(), Ok(".") | Ok("..")) {
                    metadata_bytes += name.as_ref().map_or(64, |n| n.len() + 64);
                    if out.len() >= limit
                        || metadata_bytes > 4 * 1024 * 1024
                        || started.elapsed() > Duration::from_secs(1)
                    {
                        out.push(Err("Directory listing limit".into()));
                        return Ok(out);
                    }
                    out.push(name);
                }
                if next == 0 {
                    break;
                }
                if next < 12 + length || next > bytes.len() - offset {
                    return Err("Directory unavailable".into());
                }
                offset += next;
            }
        }
    }
}

// Unix reads retain O_NOFOLLOW component handles, with nlink checked on the
// actual opened inode. Do not use flock/lockf for CAS: external writers can
// ignore them. Mutations fail BEFORE opening/creating anything for writing.
#[cfg(all(
    not(windows),
    any(
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
))]
mod shared_fs {
    use super::*;
    use std::{
        ffi::CString,
        fs::{File, Metadata},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::MetadataExt,
        },
    };
    extern "C" {
        fn openat(dir: i32, name: *const std::ffi::c_char, flags: i32, ...) -> i32;
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const FLAGS: i32 = 0x20000 | 0x80000 | 0x800; // O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    const FLAGS: i32 = 0x8000 | 0x80000 | 0x800;
    #[cfg(target_os = "macos")]
    const FLAGS: i32 = 0x100 | 0x01000000 | 4;
    pub(super) fn retain_root(dir: Dir) -> Result<Dir, String> {
        if !dir
            .dir_metadata()
            .map_err(|_| "Directory unavailable")?
            .is_dir()
        {
            return Err("Directory required".into());
        }
        Ok(dir)
    }
    pub(super) fn local(_: &Dir) -> Result<(), String> {
        Ok(())
    }
    pub(super) fn open_directory(parent: &Dir, name: &str, create: bool) -> Result<File, String> {
        if create {
            return Err(SHARED_UNSUPPORTED.into());
        }
        let file = open(parent, name, false, false, false)?;
        if !validate(&file)?.is_dir() {
            return Err("Directory required".into());
        }
        Ok(file)
    }
    pub(super) fn validate(file: &File) -> Result<Metadata, String> {
        let metadata = file.metadata().map_err(|_| "File unavailable")?;
        if !metadata.is_dir() && !metadata.is_file() {
            return Err("Regular file or directory required".into());
        }
        if metadata.is_file() && metadata.nlink() != 1 {
            return Err("Multi-link files are not shared".into());
        }
        Ok(metadata)
    }
    pub(super) fn open(
        parent: &Dir,
        name: &str,
        write: bool,
        create: bool,
        mkdir: bool,
    ) -> Result<File, String> {
        if write || create || mkdir {
            return Err(SHARED_UNSUPPORTED.into());
        }
        bridge_path(name)?;
        if Path::new(name).components().count() != 1 || name == "." {
            return Err("File name required".into());
        }
        let name = CString::new(name).map_err(|_| "File name required")?;
        // SAFETY: parent and CString outlive openat; success returns one owned fd.
        // One normal component only, O_RDONLY | O_NOFOLLOW, no symlink resolver.
        let fd = unsafe { openat(parent.as_raw_fd(), name.as_ptr(), FLAGS) };
        if fd < 0 {
            return Err(
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                    "Path not found"
                } else {
                    "File unavailable"
                }
                .into(),
            );
        }
        let file = unsafe { File::from_raw_fd(fd) };
        validate(&file)?;
        Ok(file)
    }
    pub(super) fn names(dir: &Dir, limit: usize) -> Result<Vec<Result<String, String>>, String> {
        // On Unix cap-primitives read_base_dir enumerates the descriptor itself.
        let mut out = Vec::new();
        let started = Instant::now();
        let mut metadata_bytes = 0usize;
        for entry in dir.entries().map_err(|_| "Directory unavailable")? {
            let name: Result<String, String> =
                entry.map_err(|_| "Unreadable entry".into()).and_then(|e| {
                    e.file_name()
                        .into_string()
                        .map_err(|_| "Non-UTF-8 entry".into())
                });
            metadata_bytes += name.as_ref().map_or(64, |n| n.len() + 64);
            if out.len() >= limit
                || metadata_bytes > 4 * 1024 * 1024
                || started.elapsed() > Duration::from_secs(1)
            {
                out.push(Err("Directory listing limit".into()));
                break;
            }
            out.push(name);
        }
        Ok(out)
    }
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
mod shared_fs {
    use super::*;
    pub(super) fn retain_root(_: Dir) -> Result<Dir, String> {
        Err(SHARED_UNSUPPORTED.into())
    }
    pub(super) fn local(_: &Dir) -> Result<(), String> {
        Err(SHARED_UNSUPPORTED.into())
    }
    pub(super) fn open_directory(_: &Dir, _: &str, _: bool) -> Result<std::fs::File, String> {
        Err(SHARED_UNSUPPORTED.into())
    }
    pub(super) fn validate(_: &std::fs::File) -> Result<std::fs::Metadata, String> {
        Err(SHARED_UNSUPPORTED.into())
    }
    pub(super) fn open(
        _: &Dir,
        _: &str,
        _: bool,
        _: bool,
        _: bool,
    ) -> Result<std::fs::File, String> {
        Err(SHARED_UNSUPPORTED.into())
    }
    pub(super) fn names(_: &Dir, _: usize) -> Result<Vec<Result<String, String>>, String> {
        Err(SHARED_UNSUPPORTED.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notcode_context_reads_and_prioritizes() {
        // CARGO_TARGET_DIR всегда доступен на запись (в песочнице temp/ может быть закрыт).
        let target = std::path::PathBuf::from(std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into()));
        let dir = target.join("notcode-ctx-test").join(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos().to_string());
        std::fs::create_dir_all(dir.join(".notcode")).unwrap();
        let root = dir.clone();
        assert_eq!(notcode_context_text(&root), "");
        std::fs::write(dir.join(".notcode").join("notcode.md"), "alt context").unwrap();
        assert_eq!(notcode_context_text(&root), "alt context");
        std::fs::write(dir.join("NOTCODE.md"), "root wins").unwrap();
        assert_eq!(notcode_context_text(&root), "root wins");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn quick_open_exclusions_match_case_insensitively() {
        // The quick-open walker hides build/dependency folders on every host.
        assert!(skipped("node_modules"));
        assert!(skipped("NODE_MODULES"));
        assert!(skipped(".git"));
        assert!(skipped("Target"));
        assert!(!skipped("src"));
        assert!(!skipped("targeting"));
    }

    fn temp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("notcode-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        p
    }
    #[test]
    fn rejects_traversal_devices_and_exclusions_on_every_host() {
        for p in [
            "../secret",
            "/tmp/file",
            "C:/file",
            "\\\\server\\share",
            "a/../b",
            "a\\..\\b",
            "a/.git/config",
            "node_modules/a",
            "a/NUL.txt",
            "a/COM1",
            "foo.",
            "foo ",
        ] {
            assert!(relative(p).is_err(), "accepted {p}");
        }
        assert!(relative("src/main.rs").is_ok());
        assert!(relative("").is_ok());
    }
    #[test]
    fn missing_files_require_existing_inside_parent() {
        let root = temp().canonicalize().unwrap();
        assert_eq!(
            checked(&root, Path::new("new.txt"), true).unwrap(),
            root.join("new.txt")
        );
        assert!(checked(&root, Path::new("missing/new.txt"), true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symlink_escape_and_alias_to_excluded_tree_are_rejected() {
        let root = temp().canonicalize().unwrap();
        let outside = temp().canonicalize().unwrap();
        std::fs::write(outside.join("secret"), "private").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        assert!(checked(&root, Path::new("escape/secret"), false).is_err());
        assert!(checked(&root, Path::new("escape/new"), true).is_err());
        let cap = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        assert!(cap.open("escape/secret").is_err());
        assert!(cap
            .open_with("escape/new", OpenOptions::new().write(true).create(true))
            .is_err());
        assert!(!outside.join("new").exists());
        std::fs::create_dir(root.join(".git")).unwrap();
        std::os::unix::fs::symlink(root.join(".git"), root.join("alias")).unwrap();
        assert!(checked(&root, Path::new("alias"), false).is_err());
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }
    #[test]
    fn reads_utf8_but_rejects_binary_and_oversized_files() {
        let dir = temp();
        std::fs::write(dir.join("hello.txt"), "Привет\n").unwrap();
        std::fs::write(dir.join("binary.bin"), [0, 1, 2]).unwrap();
        std::fs::write(dir.join("large.txt"), vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::initialize(&db).unwrap();
        let state = AppState::new(db);
        select_project(&state, &dir).unwrap();
        assert_eq!(read_context(&state, "hello.txt").unwrap(), "Привет\n");
        assert!(read_context(&state, "binary.bin").is_err());
        assert!(read_context(&state, "large.txt").is_err());
        assert!(read_context(&state, "../hello.txt").is_err());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bridge_expected_project_rejects_before_work_and_after_switch() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::initialize(&db).unwrap();
        let state = AppState::new(db);
        let first = temp();
        let second = temp();
        let p = select_project(&state, &first).unwrap();
        let called = std::cell::Cell::new(false);
        assert!(with_expected(&state, "wrong", || {
            called.set(true);
            Ok(())
        })
        .is_err());
        assert!(!called.get());
        select_project(&state, &second).unwrap();
        assert!(with_expected(&state, &p.id, || bridge_cas_locked(
            &state,
            "new.txt",
            "wrong project",
            None
        ))
        .is_err());
        assert!(!second.join("new.txt").exists());
        drop(state);
        std::fs::remove_dir_all(first).unwrap();
        std::fs::remove_dir_all(second).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn bridge_cas_requires_matching_hash_or_exclusive_creation() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::initialize(&db).unwrap();
        let state = AppState::new(db);
        let dir = temp();
        let p = select_project(&state, &dir).unwrap();
        with_expected(&state, &p.id, || {
            let created = bridge_cas_locked(&state, "new.txt", "one", None)?;
            assert!(bridge_cas_locked(&state, "new.txt", "bad", None).is_err());
            assert!(
                bridge_cas_locked(&state, "new.txt", "bad", Some(&text_hash(b"wrong"))).is_err()
            );
            assert_eq!(bridge_read_locked(&state, "new.txt")?.content, "one");
            let updated = bridge_cas_locked(&state, "new.txt", "two", Some(&created.hash))?;
            assert_eq!(updated.hash, text_hash(b"two"));
            assert!(bridge_cas_locked(&state, "new.txt", "bad", Some(&created.hash)).is_err());
            assert!(bridge_cas_locked(&state, "../escape.txt", "bad", None).is_err());
            assert!(bridge_cas_locked(&state, "missing.txt", "bad", Some(&created.hash)).is_err());
            assert!(bridge_cas_locked(&state, ".env", "private", None).is_err());
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("new.txt")).unwrap(), "two");
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn bridge_snapshot_reports_omissions_and_only_hashes_text() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::initialize(&db).unwrap();
        let state = AppState::new(db);
        let dir = temp();
        std::fs::write(dir.join("ok.txt"), "hello").unwrap();
        std::fs::write(dir.join("binary.bin"), [0, 1]).unwrap();
        std::fs::write(dir.join("large.txt"), vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        std::fs::write(dir.join(".env"), "private").unwrap();
        std::fs::create_dir(dir.join("node_modules")).unwrap();
        let p = select_project(&state, &dir).unwrap();
        let snapshot =
            with_expected(&state, &p.id, || bridge_snapshot_locked(&state, &p.id)).unwrap();
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files[0].hash, text_hash(b"hello"));
        for path in ["binary.bin", "large.txt", ".env", "node_modules"] {
            assert!(snapshot.skipped.iter().any(|s| s == path));
        }
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn bridge_does_not_share_aliases_to_private_files() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::initialize(&db).unwrap();
        let state = AppState::new(db);
        let dir = temp();
        std::fs::write(dir.join(".env"), "private").unwrap();
        std::os::unix::fs::symlink(dir.join(".env"), dir.join("alias")).unwrap();
        let p = select_project(&state, &dir).unwrap();
        assert!(with_expected(&state, &p.id, || bridge_read_locked(&state, "alias")).is_err());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn fixture() -> (AppState, PathBuf, Project) {
        let dir = temp();
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::initialize(&db).unwrap();
        let state = AppState::new(db);
        let project = select_project(&state, &dir).unwrap();
        (state, dir, project)
    }

    #[cfg(any(
        windows,
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ))]
    #[test]
    fn private_and_hardlinked_files_never_leak_through_read_list_search_snapshot() {
        let (state, dir, p) = fixture();
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::create_dir(dir.join(".keys")).unwrap();
        for name in [".env", ".env.local", "credentials.json", ".keys/token"] {
            std::fs::write(dir.join(name), "synthetic-private-needle").unwrap();
        }
        std::fs::hard_link(dir.join(".env"), dir.join("public-alias.txt")).unwrap();
        std::fs::write(dir.join("src/ok.txt"), "public needle\n").unwrap();
        with_expected(&state, &p.id, || {
            for name in [
                ".env",
                ".ENV",
                ".env.local",
                "credentials.json",
                ".keys/token",
                "public-alias.txt",
            ] {
                assert!(bridge_read_locked(&state, name).is_err(), "shared {name}");
            }
            assert!(bridge_list_locked(&state, ".keys").is_err());
            let listed = bridge_list_locked(&state, "")?;
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].name, "src");
            let hits = bridge_search_locked(&state, "needle")?;
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].path, "src/ok.txt");
            let snapshot = bridge_snapshot_locked(&state, &p.id)?;
            assert_eq!(snapshot.files.len(), 1);
            assert_eq!(snapshot.files[0].path, "src/ok.txt");
            for name in [
                ".env",
                ".env.local",
                "credentials.json",
                ".keys",
                "public-alias.txt",
            ] {
                assert!(
                    snapshot.skipped.iter().any(|s| s == name),
                    "unreported {name}"
                );
            }
            Ok(())
        })
        .unwrap();
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        #[link(name = "kernel32")]
        extern "system" {
            fn DeviceIoControl(
                handle: *mut std::ffi::c_void,
                code: u32,
                input: *mut std::ffi::c_void,
                input_len: u32,
                output: *mut std::ffi::c_void,
                output_len: u32,
                returned: *mut u32,
                overlapped: *mut std::ffi::c_void,
            ) -> i32;
        }
        std::fs::create_dir(link).unwrap();
        let target = target
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let substitute = format!(
            "\\??\\{}",
            target.strip_prefix("\\\\?\\").unwrap_or(&target)
        );
        let wide: Vec<u16> = substitute.encode_utf16().collect();
        let length = (wide.len() * 2) as u16;
        let mut data = Vec::new();
        data.extend_from_slice(&0xA0000003u32.to_le_bytes()); // mount-point reparse tag
        data.extend_from_slice(&(8 + length + 4).to_le_bytes());
        for field in [0u16, 0, length, length + 2, 0] {
            data.extend_from_slice(&field.to_le_bytes());
        }
        for character in wide {
            data.extend_from_slice(&character.to_le_bytes());
        }
        data.extend_from_slice(&[0; 4]);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .custom_flags(0x02000000 | 0x00200000)
            .open(link)
            .unwrap();
        let mut returned = 0u32;
        let result = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                0x000900A4,
                data.as_mut_ptr().cast(),
                data.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(
            result,
            0,
            "junction creation: {:?}",
            std::io::Error::last_os_error()
        );
    }

    #[cfg(windows)]
    #[test]
    fn junctions_and_replaced_directory_names_are_not_followed() {
        let (state, dir, p) = fixture();
        std::fs::create_dir(dir.join(".keys")).unwrap();
        std::fs::write(dir.join(".keys/token"), "synthetic-private-needle").unwrap();
        junction(&dir.join("alias"), &dir.join(".keys"));
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/ok"), "public needle").unwrap();
        with_expected(&state, &p.id, || {
            assert!(bridge_read_locked(&state, "alias/token").is_err());
            assert!(bridge_list_locked(&state, "alias").is_err());
            assert!(!bridge_list_locked(&state, "")?
                .iter()
                .any(|e| e.name == "alias"));
            assert_eq!(bridge_search_locked(&state, "needle")?.len(), 1);
            let snapshot = bridge_snapshot_locked(&state, &p.id)?;
            assert!(snapshot.skipped.iter().any(|s| s == "alias"));
            let held = shared_directories(&state, Path::new("src"), false)?;
            // Includes parent/root handles without WRITE/DELETE sharing.
            assert!(std::fs::rename(dir.join("src"), dir.join("moved")).is_err());
            use std::os::windows::fs::OpenOptionsExt;
            assert!(std::fs::OpenOptions::new()
                .write(true)
                .share_mode(7)
                .custom_flags(0x02000000 | 0x00200000)
                .open(dir.join("src"))
                .is_err());
            let mut opened = shared_fs::open(held.last().unwrap(), "ok", false, false, false)?;
            assert_eq!(shared_text(&mut opened)?, "public needle");
            drop(opened);
            drop(held);
            std::fs::rename(dir.join("src"), dir.join("moved")).unwrap();
            junction(&dir.join("src"), &dir.join(".keys"));
            assert!(bridge_read_locked(&state, "src/token").is_err());
            Ok(())
        })
        .unwrap();
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn filesystem_exclusive_handle_blocks_existing_and_new_writers_deletion_and_replacement() {
        use std::os::windows::fs::OpenOptionsExt;
        let (state, dir, p) = fixture();
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/file"), "one").unwrap();
        std::fs::write(dir.join("replacement"), "replacement").unwrap();
        with_expected(&state, &p.id, || {
            let hash = text_hash(b"one");
            let writer = std::fs::OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(dir.join("src/file"))
                .unwrap();
            assert!(bridge_cas_locked(&state, "src/file", "bad", Some(&hash)).is_err());
            drop(writer);
            // DELETE-only handles are also incompatible, even when they share all.
            let deleting = std::fs::OpenOptions::new()
                .access_mode(0x10000)
                .share_mode(7)
                .open(dir.join("src/file"))
                .unwrap();
            assert!(bridge_cas_locked(&state, "src/file", "bad", Some(&hash)).is_err());
            drop(deleting);
            let (_path, held, mut file) = shared_file(&state, "src/file", true, false)?;
            assert!(std::fs::OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(dir.join("src/file"))
                .is_err());
            assert!(std::fs::rename(dir.join("src/file"), dir.join("renamed")).is_err());
            assert!(std::fs::remove_file(dir.join("src/file")).is_err());
            assert!(std::fs::rename(dir.join("replacement"), dir.join("src/file")).is_err());
            assert!(std::fs::hard_link(dir.join("src/file"), dir.join("new-alias")).is_err());
            assert!(std::fs::rename(dir.join("src"), dir.join("moved")).is_err());
            assert_eq!(shared_text(&mut file)?, "one");
            drop(file);
            drop(held);
            assert!(
                bridge_cas_locked(&state, "src/file", "bad", Some(&text_hash(b"stale"))).is_err()
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("src/file")).unwrap(),
                "one"
            );
            bridge_write_existing_locked(&state, "src/file", "two")?;
            assert_eq!(bridge_read_locked(&state, "src/file")?.content, "two");
            assert!(bridge_write_existing_locked(&state, "missing", "bad").is_err());
            bridge_create_directory_locked(&state, "new/deep")?;
            bridge_create_file_locked(&state, "new/deep/file")?;
            assert!(bridge_create_file_locked(&state, "new/deep/file").is_err());
            assert!(bridge_create_file_locked(&state, "missing/file").is_err());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("src/file")).unwrap(),
            "two"
        );
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn metadata_only_reparse_mutation_of_held_parent_cannot_redirect_io() {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        #[link(name = "kernel32")]
        extern "system" {
            fn DeviceIoControl(
                handle: *mut std::ffi::c_void,
                code: u32,
                input: *mut std::ffi::c_void,
                input_len: u32,
                output: *mut std::ffi::c_void,
                output_len: u32,
                returned: *mut u32,
                overlapped: *mut std::ffi::c_void,
            ) -> i32;
        }
        let (state, dir, p) = fixture();
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::create_dir(dir.join(".keys")).unwrap();
        std::fs::write(dir.join(".keys/token"), "synthetic-private").unwrap();
        let held = shared_directories(&state, Path::new("src"), false).unwrap();
        // FILE_WRITE_ATTRIBUTES is not a share-mode write. Test the real
        // adversarial case, not merely a GENERIC_WRITE open that sharing denies.
        let metadata_writer = std::fs::OpenOptions::new()
            .access_mode(0x100)
            .share_mode(7)
            .custom_flags(0x02000000 | 0x00200000)
            .open(dir.join("src"))
            .unwrap();
        let target = dir
            .join(".keys")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let substitute = format!(
            "\\??\\{}",
            target.strip_prefix("\\\\?\\").unwrap_or(&target)
        );
        let wide: Vec<u16> = substitute.encode_utf16().collect();
        let length = (wide.len() * 2) as u16;
        let mut data = Vec::new();
        data.extend_from_slice(&0xA0000003u32.to_le_bytes());
        data.extend_from_slice(&(8 + length + 4).to_le_bytes());
        for field in [0u16, 0, length, length + 2, 0] {
            data.extend_from_slice(&field.to_le_bytes());
        }
        for character in wide {
            data.extend_from_slice(&character.to_le_bytes());
        }
        data.extend_from_slice(&[0; 4]);
        let mut returned = 0u32;
        assert_ne!(
            unsafe {
                DeviceIoControl(
                    metadata_writer.as_raw_handle(),
                    0x000900A4,
                    data.as_mut_ptr().cast(),
                    data.len() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let parent = held.last().unwrap();
        assert!(shared_fs::open(parent, "token", false, false, false).is_err());
        assert!(shared_fs::names(parent, 100).is_err());
        assert!(shared_fs::open_directory(parent, "new", true).is_err());
        assert!(!dir.join(".keys/new").exists());
        assert!(with_expected(&state, &p.id, || bridge_read_locked(&state, "src/token")).is_err());
        drop(metadata_writer);
        drop(held);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn short_names_cannot_alias_excluded_long_names() {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetShortPathNameW(long: *const u16, short: *mut u16, length: u32) -> u32;
        }
        let (state, dir, p) = fixture();
        std::fs::write(dir.join("credentials.json"), "synthetic-private").unwrap();
        let path: Vec<u16> = dir
            .join("credentials.json")
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut buffer = vec![0u16; 32768];
        let length =
            unsafe { GetShortPathNameW(path.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
        assert!(length > 0 && (length as usize) < buffer.len());
        let short = String::from_utf16(&buffer[..length as usize]).unwrap();
        let name = Path::new(&short).file_name().unwrap().to_str().unwrap();
        // Volumes with 8.3 generation disabled simply return the original name;
        // either way no excluded long-name content may be shared.
        assert!(with_expected(&state, &p.id, || bridge_read_locked(&state, name)).is_err());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn surviving_writable_mapping_prevents_exclusive_cas_even_after_handles_close() {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateFileMappingW(
                file: *mut std::ffi::c_void,
                security: *mut std::ffi::c_void,
                protect: u32,
                high: u32,
                low: u32,
                name: *const u16,
            ) -> *mut std::ffi::c_void;
            fn MapViewOfFile(
                mapping: *mut std::ffi::c_void,
                access: u32,
                high: u32,
                low: u32,
                bytes: usize,
            ) -> *mut std::ffi::c_void;
            fn UnmapViewOfFile(view: *const std::ffi::c_void) -> i32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        let (state, dir, p) = fixture();
        std::fs::write(dir.join("file"), "one").unwrap();
        let writer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(7)
            .open(dir.join("file"))
            .unwrap();
        let mapping = unsafe {
            CreateFileMappingW(
                writer.as_raw_handle(),
                std::ptr::null_mut(),
                4,
                0,
                0,
                std::ptr::null(),
            )
        };
        assert!(!mapping.is_null());
        let view = unsafe { MapViewOfFile(mapping, 2, 0, 0, 0) };
        assert!(!view.is_null());
        drop(writer);
        assert_ne!(unsafe { CloseHandle(mapping) }, 0);
        with_expected(&state, &p.id, || {
            assert!(bridge_cas_locked(&state, "file", "bad", Some(&text_hash(b"one"))).is_err());
            assert_eq!(std::fs::read_to_string(dir.join("file")).unwrap(), "one");
            // A genuinely noncooperating write with no live user file handle.
            unsafe {
                *(view as *mut u8) = b'x';
            }
            assert!(bridge_read_locked(&state, "file").is_err());
            Ok(())
        })
        .unwrap();
        assert_ne!(unsafe { UnmapViewOfFile(view) }, 0);
        with_expected(&state, &p.id, || {
            assert!(bridge_cas_locked(&state, "file", "bad", Some(&text_hash(b"one"))).is_err());
            assert_eq!(bridge_read_locked(&state, "file")?.content, "xne");
            Ok(())
        })
        .unwrap();
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_remote_mutations_fail_without_touching_bytes_or_creating_paths() {
        let (state, dir, p) = fixture();
        std::fs::write(dir.join("file"), "one").unwrap();
        with_expected(&state, &p.id, || {
            assert!(bridge_cas_locked(&state, "file", "bad", Some(&text_hash(b"one"))).is_err());
            assert!(bridge_cas_locked(&state, "new", "bad", None).is_err());
            assert!(bridge_create_directory_locked(&state, "missing/deep").is_err());
            assert!(bridge_create_file_locked(&state, "new").is_err());
            assert!(bridge_write_existing_locked(&state, "file", "bad").is_err());
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("file")).unwrap(), "one");
        assert!(!dir.join("new").exists());
        assert!(!dir.join("missing").exists());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(any(
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ))]
    #[test]
    fn retained_unix_directory_and_root_do_not_reopen_replaced_names() {
        let (state, dir, p) = fixture();
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::create_dir(dir.join(".keys")).unwrap();
        std::fs::write(dir.join("src/ok"), "public").unwrap();
        std::fs::write(dir.join(".keys/ok"), "synthetic-private").unwrap();
        let held = shared_directories(&state, Path::new("src"), false).unwrap();
        std::fs::rename(dir.join("src"), dir.join("moved")).unwrap();
        std::os::unix::fs::symlink(dir.join(".keys"), dir.join("src")).unwrap();
        let mut file = shared_fs::open(held.last().unwrap(), "ok", false, false, false).unwrap();
        assert_eq!(shared_text(&mut file).unwrap(), "public");
        drop(file);
        drop(held);
        assert!(with_expected(&state, &p.id, || bridge_read_locked(&state, "src/ok")).is_err());
        let moved_root = dir.with_extension("moved");
        std::fs::rename(&dir, &moved_root).unwrap();
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("unexpected"), "synthetic-private").unwrap();
        assert_eq!(
            with_expected(&state, &p.id, || bridge_read_locked(&state, "moved/ok"))
                .unwrap()
                .content,
            "public"
        );
        assert!(with_expected(&state, &p.id, || bridge_read_locked(&state, "unexpected")).is_err());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(moved_root).unwrap();
    }

    #[test]
    fn native_locked_helpers_keep_local_behavior_without_relocking_the_gate() {
        let (state, dir, _p) = fixture();
        let gate = state.project_gate.lock().unwrap();
        create_directory_locked(&state, "src/deep").unwrap();
        create_file_locked(&state, "src/deep/file").unwrap();
        write_existing_file_locked(&state, "src/deep/file", "needle").unwrap();
        write_file_locked(&state, ".env", "synthetic-private needle", true).unwrap();
        assert_eq!(
            read_context(&state, ".env").unwrap(),
            "synthetic-private needle"
        );
        assert!(list_directory_locked(&state, Some("src/deep"))
            .unwrap()
            .iter()
            .any(|e| e.name == "file"));
        assert_eq!(search_files_locked(&state, "needle").unwrap().len(), 2);
        assert!(working_directory_locked(&state, "src/deep")
            .unwrap()
            .is_dir());
        drop(gate);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_switch_is_blocked_by_active_generation() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::initialize(&db).unwrap();
        let state = AppState::new(db);
        let first = temp();
        let second = temp();
        let selected = select_project(&state, &first).unwrap();
        let (tx, _) = tokio::sync::watch::channel(false);
        state.generations.lock().unwrap().insert("busy".into(), tx);
        assert!(select_project(&state, &second).is_err());
        assert_eq!(
            state.project.lock().unwrap().as_ref().unwrap().id,
            selected.id
        );
        state.generations.lock().unwrap().clear();
        assert_ne!(select_project(&state, &second).unwrap().id, selected.id);
        assert_eq!(select_project(&state, &first).unwrap().id, selected.id);
        drop(state);
        std::fs::remove_dir_all(first).unwrap();
        std::fs::remove_dir_all(second).unwrap();
    }
}
