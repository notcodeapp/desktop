use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

const MAX_BODY: usize = 32 * 1024;
const MAX_SKILLS: usize = 100;
const MAX_INJECTED: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub enabled: bool,
    pub scope: String,
}

fn validate_id(id: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| "Invalid skill id".into())
}

fn validate(name: &str, description: &str, body: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 120 || name.chars().any(char::is_control) {
        return Err("Skill name must contain 1–120 characters without control symbols".into());
    }
    if description.len() > 500 || description.chars().any(char::is_control) {
        return Err("Skill description is limited to 500 characters".into());
    }
    if body.trim().is_empty() || body.len() > MAX_BODY {
        return Err("Skill instructions must contain 1–32768 bytes".into());
    }
    Ok(())
}

fn global_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("skills");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Caller holds project_gate.
fn project_dir_locked(state: &AppState) -> Result<PathBuf, String> {
    let dir = crate::project::root(state)?.join(".notcode").join("skills");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn read_dir(dir: &Path, scope: &str) -> Vec<Skill> {
    let mut skills = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return skills;
    };
    for entry in entries.filter_map(Result::ok).take(MAX_SKILLS + 1) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut skill) = serde_json::from_str::<Skill>(&text) else {
            continue;
        };
        if validate_id(&skill.id).is_err()
            || validate(&skill.name, &skill.description, &skill.body).is_err()
        {
            continue;
        }
        skill.scope = scope.to_owned();
        skills.push(skill);
        if skills.len() >= MAX_SKILLS {
            break;
        }
    }
    skills
}

fn dir_for(app: &AppHandle, state: &AppState, scope: &str) -> Result<PathBuf, String> {
    match scope {
        "global" => global_dir(app),
        "project" => project_dir_locked(state),
        _ => Err("Skill scope must be global or project".into()),
    }
}

#[tauri::command]
pub fn skills_list(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<Skill>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let mut skills = read_dir(&global_dir(&app)?, "global");
    if crate::project::root(&state).is_ok() {
        skills.extend(read_dir(&project_dir_locked(&state)?, "project"));
    }
    skills.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(skills)
}

#[tauri::command]
pub fn skill_save(
    app: AppHandle,
    id: Option<String>,
    name: String,
    description: String,
    body: String,
    enabled: bool,
    scope: String,
    state: State<'_, AppState>,
) -> Result<Skill, String> {
    validate(&name, &description, &body)?;
    let id = match id {
        Some(id) => {
            validate_id(&id)?;
            id
        }
        None => uuid::Uuid::new_v4().to_string(),
    };
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let dir = dir_for(&app, &state, &scope)?;
    let existing = read_dir(&dir, &scope);
    if !existing.iter().any(|s| s.id == id) && existing.len() >= MAX_SKILLS {
        return Err(format!("Skill limit ({MAX_SKILLS}) reached"));
    }
    let skill = Skill {
        id,
        name: name.trim().to_owned(),
        description: description.trim().to_owned(),
        body,
        enabled,
        scope: scope.clone(),
    };
    let json = serde_json::to_string_pretty(&skill).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", skill.id)), json).map_err(|e| e.to_string())?;
    Ok(skill)
}

#[tauri::command]
pub fn skill_delete(
    app: AppHandle,
    id: String,
    scope: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate_id(&id)?;
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let dir = dir_for(&app, &state, &scope)?;
    let path = dir.join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Caller holds project_gate. Enabled skills become persistent user-level guidance.
pub(crate) fn active_text_locked(app: &AppHandle, state: &AppState) -> String {
    let mut skills = read_dir(&global_dir(app).unwrap_or_default(), "global");
    if crate::project::root(state).is_ok() {
        if let Ok(dir) = project_dir_locked(state) {
            skills.extend(read_dir(&dir, "project"));
        }
    }
    let mut text = String::new();
    for skill in skills.iter().filter(|s| s.enabled) {
        let section = format!("## {}\n{}\n\n", skill.name, skill.body.trim());
        if text.len() + section.len() > MAX_INJECTED {
            break;
        }
        text.push_str(&section);
    }
    if text.is_empty() {
        return String::new();
    }
    format!("The user has enabled persistent skills below. Treat them as standing user instructions for this and future turns; follow them unless they conflict with safety rules or the user's latest request.\n\n{text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rejects_empty_oversized_and_control_fields() {
        assert!(validate("Review", "desc", "body").is_ok());
        assert!(validate("", "d", "b").is_err());
        assert!(validate("x", "d", "").is_err());
        assert!(validate("x", "d", &"b".repeat(MAX_BODY + 1)).is_err());
        assert!(validate("x\ny", "d", "b").is_err());
        assert!(validate_id("../escape").is_err());
        assert!(validate_id(&uuid::Uuid::new_v4().to_string()).is_ok());
    }

    #[test]
    fn only_valid_enabled_skills_are_injected() {
        let dir = std::env::temp_dir().join(format!("notcode-skills-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = Skill {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Tone".into(),
            description: String::new(),
            body: "Be concise.".into(),
            enabled: true,
            scope: "global".into(),
        };
        let off = Skill {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Off".into(),
            description: String::new(),
            body: "Hidden.".into(),
            enabled: false,
            scope: "global".into(),
        };
        std::fs::write(
            dir.join(format!("{}.json", good.id)),
            serde_json::to_string(&good).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join(format!("{}.json", off.id)),
            serde_json::to_string(&off).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "{not json").unwrap();
        let skills = read_dir(&dir, "global");
        assert_eq!(skills.len(), 2);
        assert!(skills.iter().all(|s| s.scope == "global"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
