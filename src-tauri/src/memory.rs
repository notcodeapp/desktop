use crate::state::AppState;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

const MAX_MEMORY: usize = 32 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConfig {
    pub global: String,
    /// None when no project is open.
    pub project: Option<String>,
}

fn validate(content: &str) -> Result<(), String> {
    if content.len() > MAX_MEMORY {
        return Err("Agent memory is limited to 32768 bytes".into());
    }
    Ok(())
}

pub(crate) fn global_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("memory");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("memory.md"))
}

/// Caller holds project_gate.
fn project_path_locked(state: &AppState) -> Result<PathBuf, String> {
    let dir = crate::project::root(state)?.join(".notcode").join("memory");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("memory.md"))
}

fn read(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[tauri::command]
pub fn memory_get(app: AppHandle, state: State<'_, AppState>) -> Result<MemoryConfig, String> {
    let global = read(&global_path(&app)?);
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let project = if crate::project::root(&state).is_ok() {
        Some(read(&project_path_locked(&state)?))
    } else {
        None
    };
    Ok(MemoryConfig { global, project })
}

#[tauri::command]
pub fn memory_set(
    app: AppHandle,
    scope: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate(&content)?;
    let path = match scope.as_str() {
        "global" => global_path(&app)?,
        "project" => {
            let _gate = state
                .project_gate
                .lock()
                .map_err(|_| "Project gate poisoned")?;
            project_path_locked(&state)?
        }
        _ => return Err("Memory scope must be global or project".into()),
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    std::fs::write(&path, format!("{trimmed}\n")).map_err(|e| e.to_string())?;
    Ok(())
}

/// Caller holds project_gate. Memory is long-term context about the user and the
/// project, injected into every generation like enabled skills.
pub(crate) fn active_text_locked(app: &AppHandle, state: &AppState) -> String {
    let mut sections = Vec::new();
    let global = read(&global_path(app).unwrap_or_default());
    if !global.trim().is_empty() {
        sections.push(format!("### Global memory\n{}", global.trim()));
    }
    if crate::project::root(state).is_ok() {
        if let Ok(path) = project_path_locked(state) {
            let project = read(&path);
            if !project.trim().is_empty() {
                sections.push(format!("### Project memory\n{}", project.trim()));
            }
        }
    }
    if sections.is_empty() {
        return String::new();
    }
    format!(
        "The user maintains persistent agent memory below. It records durable facts, preferences and project conventions accumulated over past sessions. Use it as reliable background context; the user's latest request always wins on conflict.\n\n{}",
        sections.join("\n\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_memory_is_rejected() {
        assert!(validate("short note").is_ok());
        assert!(validate(&"x".repeat(MAX_MEMORY + 1)).is_err());
    }
}
