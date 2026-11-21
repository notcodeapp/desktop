use crate::state::AppState;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::auth::KEYRING_SERVICE_PROXY as KEYRING_SERVICE;

const MAX_PROFILES: usize = 20;
const MAX_MODELS: usize = 200;
const MAX_MODEL_NAME: usize = 200;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<String>,
    pub active: bool,
}

fn provider_key_name(id: &str) -> String {
    format!("profile:{id}")
}

fn read_provider_key(id: &str) -> Option<String> {
    #[cfg(windows)]
    {
        keyring::Entry::new(KEYRING_SERVICE, &provider_key_name(id))
            .ok()?
            .get_password()
            .ok()
    }
    #[cfg(not(windows))]
    {
        let _ = id;
        None
    }
}

fn provider_key_entry(id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &provider_key_name(id)).map_err(|e| e.to_string())
}

fn parse_models(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .collect()
}

/// Shared validation for name/base_url/models. Pure, so tests run anywhere.
pub(crate) fn validate_profile(name: &str, base_url: &str, models: &[String]) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err("Provider name must contain 1–120 characters".into());
    }
    let url = reqwest::Url::parse(base_url.trim()).map_err(|_| "Invalid API URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("API URL must use HTTPS without credentials, query or fragment".into());
    }
    if models.is_empty() || models.len() > MAX_MODELS {
        return Err("Add 1–200 models for this provider".into());
    }
    if models.iter().any(|id| {
        id.trim().is_empty() || id.trim().len() > MAX_MODEL_NAME || id.trim().chars().any(char::is_control)
    }) {
        return Err("Each model id must contain 1–200 characters".into());
    }
    Ok(())
}

fn row_to_profile(row: &rusqlite::Row<'_>, active_id: Option<&str>) -> rusqlite::Result<ProviderProfile> {
    let id: String = row.get(0)?;
    let models_raw: String = row.get(3)?;
    Ok(ProviderProfile {
        active: active_id == Some(id.as_str()),
        id,
        name: row.get(1)?,
        base_url: row.get(2)?,
        models: parse_models(&models_raw),
    })
}

/// Seeds the first profile from the legacy single-provider settings, so an
/// existing setup keeps working unchanged after the update.
fn ensure_seed(db: &rusqlite::Connection) -> Result<(), String> {
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM provider_profiles", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count > 0 {
        return Ok(());
    }
    let legacy: Option<(String, String)> = db
        .query_row(
            "SELECT value FROM settings WHERE key='compatible_url'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .and_then(|url| {
            db.query_row(
                "SELECT value FROM settings WHERE key='compatible_model'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .map(|model| (url, model))
        });
    let Some((url, model)) = legacy else { return Ok(()) };
    let id = uuid::Uuid::new_v4().to_string();
    db.execute(
        "INSERT INTO provider_profiles(id,name,base_url,models,created_at) VALUES(?1,?2,?3,?4,?5)",
        params![id, "Основной API", url, model, crate::store::now()],
    )
    .map_err(|e| e.to_string())?;
    set_active_id(db, Some(&id))
}

fn active_id(db: &rusqlite::Connection) -> Result<Option<String>, String> {
    db.query_row(
        "SELECT value FROM settings WHERE key='active_provider_profile'",
        [],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn set_active_id(db: &rusqlite::Connection, id: Option<&str>) -> Result<(), String> {
    match id {
        Some(id) => db
            .execute(
                "INSERT INTO settings(key,value) VALUES('active_provider_profile',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![id],
            )
            .map_err(|e| e.to_string())?,
        None => db
            .execute("DELETE FROM settings WHERE key='active_provider_profile'", [])
            .map_err(|e| e.to_string())?,
    };
    Ok(())
}

#[tauri::command]
pub fn provider_profiles_list(state: State<'_, AppState>) -> Result<Vec<ProviderProfile>, String> {
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    ensure_seed(&db)?;
    let active = active_id(&db)?;
    let mut stmt = db
        .prepare("SELECT id,name,base_url,models FROM provider_profiles ORDER BY created_at,rowid")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| row_to_profile(row, active.as_deref()))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn provider_profiles_save(
    id: Option<String>,
    name: String,
    base_url: String,
    models: Vec<String>,
    state: State<'_, AppState>,
) -> Result<ProviderProfile, String> {
    if let Some(id) = &id {
        if id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("Invalid profile id".into());
        }
    }
    let cleaned: Vec<String> = models.iter().map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()).collect();
    validate_profile(&name, &base_url, &cleaned)?;
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    ensure_seed(&db)?;
    if id.is_none() {
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM provider_profiles", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if count >= MAX_PROFILES as i64 {
            return Err(format!("Provider limit ({MAX_PROFILES}) reached"));
        }
    }
    let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let name = name.trim().to_owned();
    let base_url = base_url.trim().trim_end_matches('/').to_owned();
    let models_raw = cleaned.join(",");
    db.execute(
        "INSERT INTO provider_profiles(id,name,base_url,models,created_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(id) DO UPDATE SET name=excluded.name, base_url=excluded.base_url, models=excluded.models",
        params![id, name, base_url, models_raw, crate::store::now()],
    )
    .map_err(|e| e.to_string())?;
    let active = active_id(&db)?;
    if active.is_none() {
        set_active_id(&db, Some(&id))?;
    }
    let active = active_id(&db)?;
    Ok(ProviderProfile { active: active.as_deref() == Some(id.as_str()), id, name, base_url, models: cleaned })
}

#[tauri::command]
pub fn provider_profiles_set_active(id: Option<String>, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    ensure_seed(&db)?;
    if let Some(id) = &id {
        let exists: Option<String> = db
            .query_row("SELECT id FROM provider_profiles WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_none() {
            return Err("Profile not found".into());
        }
    }
    set_active_id(&db, id.as_deref())
}

#[tauri::command]
pub fn provider_profiles_delete(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    db.execute("DELETE FROM provider_profiles WHERE id=?1", [&id])
        .map_err(|e| e.to_string())?;
    if active_id(&db)?.as_deref() == Some(id.as_str()) {
        set_active_id(&db, None)?;
    }
    let _ = crate::agent::key_delete(format!("profile:{id}"));
    Ok(())
}

/// Base URL of the profile that should serve `compatible` requests, or None to
/// fall back to the legacy single-provider settings.
pub(crate) fn active_base_url(db: &rusqlite::Connection) -> Result<Option<String>, String> {
    ensure_seed(db)?;
    let Some(active) = active_id(db)? else { return Ok(None) };
    Ok(db
        .query_row(
            "SELECT base_url FROM provider_profiles WHERE id=?1",
            [&active],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?)
}

/// Keyring account name that holds the secret for the active compatible profile.
pub(crate) fn active_keyring_name(db: &rusqlite::Connection) -> Result<Option<String>, String> {
    ensure_seed(db)?;
    Ok(active_id(db)?.map(|id| format!("profile:{id}")))
}

/// Comma-joined model list of the active profile, for the composer dropdown.
pub(crate) fn active_models(db: &rusqlite::Connection) -> Result<Option<String>, String> {
    ensure_seed(db)?;
    let Some(active) = active_id(db)? else { return Ok(None) };
    Ok(db
        .query_row(
            "SELECT models FROM provider_profiles WHERE id=?1",
            [&active],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?)
}

// ---- Синхронизация с хранилищем аккаунта (/api/vault воркера) -----------------

fn worker_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

fn session_bearer() -> Result<String, String> {
    let token = crate::auth::session_token()?.ok_or("Требуется вход через GitHub")?;
    Ok(format!("Bearer {token}"))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<String>,
    pub key: Option<String>,
    pub updated_at: i64,
}

/// Провайдеры из хранилища аккаунта (сервер отдаёт расшифрованными).
#[tauri::command]
pub async fn vault_providers_list() -> Result<Vec<AccountProvider>, String> {
    let auth = session_bearer()?;
    let response = worker_client()?
        .get(format!("{}/api/vault", crate::auth::api_base()))
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("Ошибка сервера: {e}"))?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|e| format!("Неверный ответ сервера: {e}"))?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Сессия недействительна; войдите заново".into());
    }
    if !status.is_success() {
        return Err(body.get("error").and_then(serde_json::Value::as_str).unwrap_or("Ошибка сервера").into());
    }
    let mut result = Vec::new();
    for item in body.get("items").and_then(serde_json::Value::as_array).cloned().unwrap_or_default() {
        if item.get("kind").and_then(serde_json::Value::as_str) != Some("provider") { continue; }
        let data = item.get("data").cloned().unwrap_or(serde_json::Value::Null);
        result.push(AccountProvider {
            id: item.get("id").and_then(serde_json::Value::as_str).unwrap_or("").to_owned(),
            name: data.get("name").and_then(serde_json::Value::as_str).unwrap_or("").to_owned(),
            base_url: data.get("baseUrl").and_then(serde_json::Value::as_str).unwrap_or("").to_owned(),
            models: data.get("models").and_then(serde_json::Value::as_array)
                .map(|a| a.iter().filter_map(|m| m.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            key: data.get("key").and_then(serde_json::Value::as_str).map(String::from),
            updated_at: item.get("updatedAt").and_then(serde_json::Value::as_i64).unwrap_or(0),
        });
    }
    Ok(result)
}

/// Отправляет локальный профиль (вместе с ключом из keyring) в аккаунт.
#[tauri::command]
pub async fn vault_provider_push(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let profile = {
        let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
        ensure_seed(&db)?;
        db.query_row(
            "SELECT id,name,base_url,models FROM provider_profiles WHERE id = ?1",
            [&id], |row| row_to_profile(row, None),
        ).optional().map_err(|e| e.to_string())?.ok_or("Профиль не найден")?
    };
    let data = serde_json::json!({
        "name": profile.name,
        "baseUrl": profile.base_url,
        "models": profile.models,
        "key": read_provider_key(&id),
    });
    let auth = session_bearer()?;
    let response = worker_client()?
        .put(format!("{}/api/vault", crate::auth::api_base()))
        .header("Authorization", auth)
        .json(&serde_json::json!({
            "items": [{ "kind": "provider", "id": format!("acc-{}", profile.id), "updatedAt": crate::store::now(), "data": data }]
        }))
        .send()
        .await
        .map_err(|e| format!("Ошибка сервера: {e}"))?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|e| format!("Неверный ответ сервера: {e}"))?;
    if !status.is_success() {
        return Err(body.get("error").and_then(serde_json::Value::as_str).unwrap_or("Ошибка сервера").into());
    }
    Ok(())
}

// ---- OpenCode Zen: каталог моделей шлюза opencode.ai --------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeModel {
    pub id: String,
    pub name: String,
    pub ctx: i64,
    pub free: bool,
    pub description: String,
}

/// OpenAI-совместимый эндпоинт шлюза (тот же, что в agent.rs).
pub(crate) const OPENCODE_BASE_URL: &str = "https://opencode.ai/inference/openai/v1";

/// Резервный каталог, если живой список недоступен: актуальные бесплатные
/// и платные модели шлюза на момент релиза.
fn opencode_catalog() -> Vec<OpencodeModel> {
    vec![
        OpencodeModel { id: "big-pickle".into(), name: "Big Pickle".into(), ctx: 200000, free: true, description: "Reasoning-модель для анализа и инструментов".into() },
        OpencodeModel { id: "deepseek-v4.1-flash-free".into(), name: "DeepSeek V4.1 Flash Free".into(), ctx: 200000, free: true, description: "Быстрая DeepSeek с кодом и зрением".into() },
        OpencodeModel { id: "mimo-v2.6-flash-free".into(), name: "MiMo V2.6 Flash Free".into(), ctx: 200000, free: true, description: "Мультимодальная Xiaomi".into() },
        OpencodeModel { id: "nemotron-3.5-lightning-free".into(), name: "Nemotron 3.5 Lightning Free".into(), ctx: 200000, free: true, description: "NVIDIA, скорость".into() },
        OpencodeModel { id: "flint-40b-free".into(), name: "Flint 40B Free".into(), ctx: 200000, free: true, description: "Открытые веса".into() },
        OpencodeModel { id: "grok-code".into(), name: "Grok Code".into(), ctx: 256000, free: false, description: "xAI специально для кода".into() },
        OpencodeModel { id: "claude-sonnet-5".into(), name: "Claude Sonnet 5".into(), ctx: 200000, free: false, description: "Anthropic, лучшая для кода".into() },
        OpencodeModel { id: "gpt-5.4".into(), name: "GPT-5.4".into(), ctx: 400000, free: false, description: "OpenAI флагман".into() },
        OpencodeModel { id: "gemini-4-carbon".into(), name: "Gemini 4 Carbon".into(), ctx: 1000000, free: false, description: "Огромный контекст Google".into() },
        OpencodeModel { id: "kimi-k2.7-code".into(), name: "Kimi K2.7 Code".into(), ctx: 262144, free: false, description: "Moonshot для кода".into() },
    ]
}

/// Разбор живого каталога https://opencode.ai/inference/v1/models
/// в наши метаданные; бесплатно — когда цена входа и выхода равна нулю.
fn opencode_parse_models(bytes: &[u8]) -> Result<Vec<OpencodeModel>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "OpenCode вернул невалидный JSON")?;
    let items = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or("OpenCode вернул список без data")?;
    let mut models = Vec::new();
    for item in items {
        let id = match item.get("id").and_then(serde_json::Value::as_str) {
            Some(id) if !id.is_empty() && id.len() <= 200 => id.to_owned(),
            _ => continue,
        };
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&id)
            .to_owned();
        let ctx = item
            .pointer("/limit/context")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(128_000);
        let free = item
            .pointer("/cost/text/input/price")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0)
            == 0.0
            && item
                .pointer("/cost/text/output/price")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0)
                == 0.0;
        let description = item
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
        models.push(OpencodeModel { id, name, ctx, free, description });
    }
    if models.is_empty() {
        return Err("Каталог OpenCode пуст".into());
    }
    models.sort_by(|a, b| b.free.cmp(&a.free).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(models)
}

/// Живой каталог моделей OpenCode Zen (контекст, free/paid);
/// при ошибке сети — встроенный резервный список.
#[tauri::command]
pub async fn opencode_models() -> Result<Vec<OpencodeModel>, String> {
    let fetch = async {
        let response = worker_client()?
            .get("https://opencode.ai/inference/v1/models")
            .send()
            .await
            .map_err(|e| format!("Не удалось получить каталог OpenCode: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("OpenCode вернул HTTP {}", response.status()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Ошибка чтения каталога OpenCode: {e}"))?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Каталог OpenCode слишком большой".into());
        }
        opencode_parse_models(&bytes)
    };
    match fetch.await {
        Ok(models) => Ok(models),
        Err(_) => Ok(opencode_catalog()),
    }
}

/// Базовый URL шлюза OpenCode (chat completions).
#[tauri::command]
pub fn opencode_base_url() -> Result<String, String> {
    Ok(OPENCODE_BASE_URL.to_owned())
}

// ---- Google Drive: привязка аккаунта и бэкап чатов ---------------------------

#[tauri::command]
pub async fn google_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let auth = session_bearer()?;
    let response = worker_client()?
        .get(format!("{}/api/google/status", crate::auth::api_base()))
        .header("Authorization", auth)
        .send().await
        .map_err(|e| format!("Ошибка сервера: {e}"))?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(body.get("error").and_then(serde_json::Value::as_str).unwrap_or("Ошибка сервера").into());
    }
    Ok(body)
}

#[tauri::command]
pub async fn google_connect(app: AppHandle) -> Result<(), String> {
    let token = crate::auth::session_token()?.ok_or("Требуется вход через GitHub")?;
    app.opener()
        .open_url(format!("{}/auth/google?token={}", crate::auth::api_base(), token), None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn google_disconnect(app: AppHandle) -> Result<(), String> {
    let auth = session_bearer()?;
    let response = worker_client()?
        .delete(format!("{}/api/google", crate::auth::api_base()))
        .header("Authorization", auth)
        .send().await
        .map_err(|e| format!("Ошибка сервера: {e}"))?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(body.get("error").and_then(serde_json::Value::as_str).unwrap_or("Ошибка сервера").into());
    }
    Ok(())
}

/// Публичная ссылка на диалог: POST /api/share на воркере с сессией.
/// Возвращает { url } — unlisted-страница /s/<id>, действует 90 дней.
#[tauri::command]
pub async fn share_chat(app: AppHandle, state: State<'_, AppState>, session_id: String) -> Result<serde_json::Value, String> {
    let auth = session_bearer()?;
    let title = {
        let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
        let pid = crate::store::project_id(&state)?;
        crate::store::find_session(&db, &pid, &session_id)?.title
    };
    let messages = {
        let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
        let mut stmt = db.prepare(
            "SELECT role, content FROM messages WHERE session_id = ?1 ORDER BY created_at, rowid LIMIT 200;"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([&session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).map_err(|e| e.to_string())?;
        let mut messages = Vec::new();
        for row in rows {
            let (role, content) = row.map_err(|e| e.to_string())?;
            if content.trim().is_empty() { continue; }
            messages.push(serde_json::json!({ "role": role, "text": content }));
        }
        messages
    };
    if messages.is_empty() {
        return Err("В диалоге нет сообщений — делиться нечем".into());
    }
    let payload = serde_json::json!({ "title": title, "messages": messages });
    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    if body.len() > 128 * 1024 {
        return Err("Диалог слишком большой для публичной ссылки".into());
    }
    let response = worker_client()?
        .post(format!("{}/api/share", crate::auth::api_base()))
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Ошибка сервера: {e}"))?;
    let status = response.status();
    let result: serde_json::Value = response.json().await.map_err(|e| format!("Неверный ответ сервера: {e}"))?;
    if !status.is_success() {
        return Err(result.get("error").and_then(serde_json::Value::as_str).unwrap_or("Ошибка сервера").into());
    }
    Ok(result)
}

/// Бэкап чатов десктопа в Google Drive (один JSON-файл notcode-desktop-backup).
#[tauri::command]
pub async fn drive_backup_push(app: AppHandle, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let auth = session_bearer()?;
    let mut chats = Vec::new();
    {
        let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
        let mut stmt = db.prepare(
            "SELECT s.id, s.title, s.provider, s.model, s.updated_at, s.project_id, (SELECT IFNULL(GROUP_CONCAT(m.content, char(30)), '') FROM messages m WHERE m.session_id = s.id) AS joined FROM sessions s ORDER BY s.updated_at DESC LIMIT 500;"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                row.get::<_, String>(3)?, row.get::<_, i64>(4)?, row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        }).map_err(|e| e.to_string())?;
        for row in rows {
            let (id, title, provider, model, updated_at, project_id, joined) = row.map_err(|e| e.to_string())?;
            let messages: Vec<serde_json::Value> = joined.split('\u{1E}')
                .filter(|c| !c.is_empty())
                .map(|c| serde_json::json!({ "content": c }))
                .collect();
            chats.push(serde_json::json!({
                "id": id, "title": title, "provider": provider, "model": model,
                "updatedAt": updated_at, "projectId": project_id, "messages": messages,
            }));
        }
    }
    let payload = serde_json::json!({ "source": "desktop", "exportedAt": crate::store::now(), "chats": chats });
    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    if body.len() > 4 * 1024 * 1024 {
        return Err("Бэкап больше 4 МиБ — экспортируйте меньше чатов".into());
    }
    let response = worker_client()?
        .put(format!("{}/api/drive/backup?name=desktop", crate::auth::api_base()))
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(body)
        .send().await
        .map_err(|e| format!("Ошибка сервера: {e}"))?;
    let status = response.status();
    let result: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(result.get("error").and_then(serde_json::Value::as_str).unwrap_or("Ошибка сервера").into());
    }
    Ok(result)
}

#[tauri::command]
pub async fn drive_backup_pull(app: AppHandle) -> Result<serde_json::Value, String> {
    let auth = session_bearer()?;
    let response = worker_client()?
        .get(format!("{}/api/drive/backup?name=desktop", crate::auth::api_base()))
        .header("Authorization", auth)
        .send().await
        .map_err(|e| format!("Ошибка сервера: {e}"))?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(body.get("error").and_then(serde_json::Value::as_str).unwrap_or("Ошибка сервера").into());
    }
    Ok(body)
}

// ---- Чаты из хранилища аккаунта ----------------------------------------------

/// Чат из аккаунта (краткая информация для списка).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultChatInfo {
    pub vault_id: String,
    pub name: String,
    pub model: String,
    pub messages_count: usize,
    pub updated_at: i64,
}

/// Список чатов из хранилища аккаунта (для импорта).
#[tauri::command]
pub async fn vault_chats_list(app: AppHandle) -> Result<Vec<VaultChatInfo>, String> {
    let auth = session_bearer()?;
    let response = worker_client()?
        .get(format!("{}/api/vault", crate::auth::api_base()))
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("Ошибка сервера: {e}"))?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Сессия недействительна; войдите заново".into());
    }
    if !status.is_success() {
        return Err(body.get("error").and_then(serde_json::Value::as_str).unwrap_or("Ошибка сервера").into());
    }
    let mut result = Vec::new();
    for item in body.get("items").and_then(serde_json::Value::as_array).cloned().unwrap_or_default() {
        if item.get("kind").and_then(serde_json::Value::as_str) != Some("chat") { continue; }
        let data = item.get("data").cloned().unwrap_or(serde_json::Value::Null);
        result.push(VaultChatInfo {
            vault_id: item.get("id").and_then(serde_json::Value::as_str).unwrap_or("").to_owned(),
            name: data.get("name").and_then(serde_json::Value::as_str).unwrap_or("чат").to_owned(),
            model: data.get("model").and_then(serde_json::Value::as_str).unwrap_or("").to_owned(),
            messages_count: data.get("messages").and_then(serde_json::Value::as_array).map(|a| a.len()).unwrap_or(0),
            updated_at: item.get("updatedAt").and_then(serde_json::Value::as_i64).unwrap_or(0),
        });
    }
    Ok(result)
}

/// Импорт чата из аккаунта: создаёт сессию с сообщениями в текущем проекте.
/// Требует открытый проект. Возвращает созданную сессию.
#[tauri::command]
pub async fn vault_chat_import(vault_id: String, app: AppHandle, state: State<'_, AppState>) -> Result<crate::store::Session, String> {
    if vault_id.len() > 128 || vault_id.chars().any(|c| c.is_control()) {
        return Err("Invalid vault chat id".into());
    }
    let auth = session_bearer()?;
    let body: serde_json::Value = worker_client()?
        .get(format!("{}/api/vault", crate::auth::api_base()))
        .header("Authorization", auth)
        .send().await
        .map_err(|e| format!("Ошибка сервера: {e}"))?
        .json().await.map_err(|e| e.to_string())?;

    let item = body.get("items").and_then(serde_json::Value::as_array)
        .and_then(|items| items.iter().find(|i| {
            i.get("kind").and_then(serde_json::Value::as_str) == Some("chat")
                && i.get("id").and_then(serde_json::Value::as_str) == Some(vault_id.as_str())
        }))
        .ok_or("Чат не найден в аккаунте")?;
    let data = item.get("data").ok_or("Чат без данных")?;
    let name = data.get("name").and_then(serde_json::Value::as_str).unwrap_or("чат из аккаунта").to_owned();
    let model = data.get("model").and_then(serde_json::Value::as_str).unwrap_or("").to_owned();
    let messages = data.get("messages").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
    if messages.is_empty() || messages.len() > 500 {
        return Err("В чате нет сообщений или их слишком много".into());
    }

    // session_create сам захватывает project_gate — не держим его здесь.
    let session = crate::store::session_create("compatible".into(), if model.is_empty() { "imported".into() } else { model }, state.clone())?;

    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    db.execute("UPDATE sessions SET title = ?1 WHERE id = ?2", params![name, session.id]).map_err(|e| e.to_string())?;
    for (index, message) in messages.iter().enumerate() {
        let role = message.get("role").and_then(serde_json::Value::as_str).unwrap_or("user");
        if !["user", "assistant", "system"].contains(&role) { continue; }
        let content = message.get("content").and_then(serde_json::Value::as_str).unwrap_or("");
        if content.is_empty() || content.len() > crate::store::MAX_CONTENT { continue; }
        db.execute(
            "INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![uuid::Uuid::new_v4().to_string(), session.id, role, content, crate::store::now() + index as i64],
        ).map_err(|e| e.to_string())?;
    }
    Ok(session)
}

/// Импорт провайдера из аккаунта: сохраняет локальный профиль + ключ в keyring.
#[tauri::command]
pub async fn vault_provider_import(
    id: String,
    name: String,
    base_url: String,
    models: Vec<String>,
    key: Option<String>,
    state: State<'_, AppState>,
) -> Result<ProviderProfile, String> {
    if id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Invalid profile id".into());
    }
    let cleaned: Vec<String> = models.iter().map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()).collect();
    validate_profile(&name, &base_url, &cleaned)?;
    let saved = {
        let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
        ensure_seed(&db)?;
        let models_raw = cleaned.join(",");
        db.execute(
            "INSERT INTO provider_profiles(id,name,base_url,models,created_at) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, base_url=excluded.base_url, models=excluded.models",
            params![id, name.trim(), base_url.trim().trim_end_matches('/'), models_raw, crate::store::now()],
        ).map_err(|e| e.to_string())?;
        if active_id(&db)?.is_none() {
            set_active_id(&db, Some(&id))?;
        }
        let active = active_id(&db)?;
        db.query_row(
            "SELECT id,name,base_url,models FROM provider_profiles WHERE id = ?1",
            [&id], |row| row_to_profile(row, active.as_deref()),
        ).map_err(|e| e.to_string())?
    };
    if let Some(key) = key.filter(|k| !k.is_empty()) {
        provider_key_entry(&saved.id)?.set_password(&key).map_err(|e| e.to_string())?;
    }
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn profile_validation_accepts_https_and_rejects_the_rest() {
        assert!(validate_profile("OpenRouter", "https://openrouter.ai/api/v1", &models(&["a", "b"])).is_ok());
        assert!(validate_profile("DeepSeek", "https://api.deepseek.com/v1/", &models(&["deepseek-chat"])).is_ok());
        assert!(validate_profile("", "https://x.example", &models(&["a"])).is_err());
        assert!(validate_profile("x", "http://insecure.example", &models(&["a"])).is_err());
        assert!(validate_profile("x", "https://user:pw@example.com", &models(&["a"])).is_err());
        assert!(validate_profile("x", "https://ok.example?query=1", &models(&["a"])).is_err());
        assert!(validate_profile("x", "https://ok.example", &models(&[])).is_err());
        assert!(validate_profile("x", "https://ok.example", &models(&["a", " "])).is_err());
        assert!(validate_profile("x", "https://ok.example", &models(&["a"; MAX_MODELS + 1])).is_err());
    }

    #[test]
    fn models_parse_trims_and_dedups_nothing_but_filters_empty() {
        assert_eq!(parse_models(" a , b\n c "), vec!["a", "b", "c"]);
        assert!(parse_models(" , ,").is_empty());
    }
}
