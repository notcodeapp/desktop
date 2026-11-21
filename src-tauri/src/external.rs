use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

/// Files opened from outside the project context: Windows "Открыть с помощью",
/// drag & drop onto the window, or the in-app file picker. They live in
/// standalone tabs and are saved straight back to their absolute paths.
pub const MAX_EXTERNAL_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalFile {
    pub path: String,
    pub name: String,
    pub content: String,
}

fn validate(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Err("Нужен абсолютный путь к файлу".into());
    }
    let metadata = std::fs::metadata(candidate).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Это не обычный файл".into());
    }
    if metadata.len() > MAX_EXTERNAL_BYTES {
        return Err("Файл больше 1 МиБ — откройте его как проект".into());
    }
    Ok(candidate.to_path_buf())
}

fn normalize(p: &Path) -> String {
    // Canonicalized Windows paths carry the \\?\ prefix; drop it for display.
    let text = p.to_string_lossy().replace("\\\\?\\", "").replace('\\', "/");
    text
}

#[tauri::command]
pub fn external_file_read(path: String) -> Result<ExternalFile, String> {
    let candidate = validate(&path)?;
    let content = std::fs::read_to_string(&candidate).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => "Файл не является текстом UTF-8".to_owned(),
        _ => e.to_string(),
    })?;
    let name = candidate
        .file_name()
        .map(|n| n.to_string_lossy().to_owned())
        .unwrap_or_else(|| "файл".into());
    Ok(ExternalFile { path: normalize(&candidate), name: name.to_string(), content })
}

#[tauri::command]
pub fn external_file_write(path: String, content: String) -> Result<(), String> {
    let candidate = validate(&path)?;
    if content.len() > crate::project::MAX_FILE_BYTES {
        return Err("File exceeds the 1 MiB limit".into());
    }
    std::fs::write(&candidate, content).map_err(|e| e.to_string())
}

/// System file picker for "Открыть файл…". User-initiated, so no approval flow.
#[tauri::command]
pub async fn external_file_pick() -> Result<Option<String>, String> {
    Ok(rfd::AsyncFileDialog::new()
        .add_filter("Текст и код", &["txt", "md", "json", "yaml", "yml", "toml", "ini", "rs", "py", "js", "ts", "tsx", "jsx", "html", "css", "kt", "java", "c", "h", "cpp", "go", "rb", "php", "sh", "ps1", "sql", "xml"])
        .add_filter("Все файлы", &["*"])
        .pick_file()
        .await
        .map(|handle| normalize(&handle.path().to_path_buf())))
}

/// "Сохранить как" для внешнего файла: системный диалог сохранения, затем
/// запись по выбранному пути. Возвращает сохранённый путь или None при отмене.
#[tauri::command]
pub async fn external_file_save_as(content: String, file_name: String) -> Result<Option<String>, String> {
    if content.len() > crate::project::MAX_FILE_BYTES {
        return Err("File exceeds the 1 MiB limit".into());
    }
    // Only the name part may seed the dialog; any path components are stripped.
    let safe_name = file_name.split(['\\', '/']).last().unwrap_or("file.txt").to_owned();
    let Some(handle) = rfd::AsyncFileDialog::new().set_file_name(&safe_name).save_file().await else {
        return Ok(None);
    };
    let path = handle.path().to_path_buf();
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(Some(normalize(&path)))
}

/// True for paths a file association / second instance may open: existing files
/// that are neither flags nor deep links.
pub(crate) fn is_openable_argument(raw: &str) -> bool {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.starts_with('-') || trimmed.starts_with("notcode://") || trimmed.is_empty() {
        return false;
    }
    Path::new(trimmed).is_file()
}

pub(crate) fn emit_external_open(app: &AppHandle, path: &str) {
    let _ = app.emit("external:open", normalize(Path::new(path)));
}
