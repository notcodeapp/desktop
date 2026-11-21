use reqwest::StatusCode;
use serde::Serialize;
use serde_json::Value;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_opener::OpenerExt;

const API_BASE: &str = "https://gentle-base-825d.notkovvik.workers.dev";
const KEYRING_SERVICE: &str = "dev.notcode.desktop.github";
pub(crate) const KEYRING_SERVICE_PROXY: &str = "dev.notcode.desktop";

pub(crate) fn api_base() -> &'static str {
    API_BASE
}

/// Current session token from the OS keyring; None when signed out.
pub(crate) fn session_token() -> Result<Option<String>, String> {
    match keyring::Entry::new(KEYRING_SERVICE, "session")
        .map_err(|e| e.to_string())?
        .get_password()
    {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

struct AuthState {
    initial_callback: Mutex<Option<String>>,
    operation: tokio::sync::Mutex<()>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProfile {
    pub username: String,
    pub avatar_url: String,
    pub expires_at: String,
}

fn validate_token(token: &str) -> Result<(), String> {
    if token.len() < 20
        || token.len() > 132
        || !token.starts_with("ncs_")
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("Некорректный токен авторизации".into());
    }
    Ok(())
}

fn callback_credentials(url: &str) -> Result<(String, String), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "Некорректная ссылка авторизации")?;
    if parsed.scheme() != "notcode"
        || parsed.host_str() != Some("auth")
        || !matches!(parsed.path(), "" | "/")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return Err("Некорректная ссылка авторизации".into());
    }
    let tokens: Vec<_> = parsed
        .query_pairs()
        .filter(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned())
        .collect();
    let users: Vec<_> = parsed
        .query_pairs()
        .filter(|(key, _)| key == "user")
        .map(|(_, value)| value.into_owned())
        .collect();
    if tokens.len() != 1 || users.len() != 1 || users[0].is_empty() {
        return Err("В ссылке должны быть один токен и один пользователь".into());
    }
    validate_token(&tokens[0])?;
    Ok((tokens[0].clone(), users[0].clone()))
}

#[tauri::command]
pub async fn auth_login(app: AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(format!("{API_BASE}/auth/github?source=app"), None::<&str>)
        .map_err(|error| error.to_string())
}

pub fn register_deep_link(app: &AppHandle) -> Result<(), String> {
    let initial_callback = app
        .deep_link()
        .get_current()
        .map_err(|error| error.to_string())?
        .unwrap_or_default()
        .into_iter()
        .find(|url| url.scheme() == "notcode" && url.host_str() == Some("auth"))
        .map(|url| url.to_string());
    // The plugin emits startup URLs before the webview and its listeners exist.
    app.manage(AuthState {
        initial_callback: Mutex::new(initial_callback),
        operation: tokio::sync::Mutex::new(()),
    });
    let handle = app.clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            if url.scheme() != "notcode" || url.host_str() != Some("auth") {
                continue;
            }
            let callback = url.to_string();
            let app = handle.clone();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<AuthState>();
                let _guard = state.operation.lock().await;
                match accept_callback(&callback).await {
                    Ok(profile) => {
                        let _ = app.emit("auth:profile", Some(profile));
                    }
                    Err(error) => {
                        let _ = app.emit("auth:error", error);
                    }
                }
            });
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn auth_me(app: AppHandle) -> Result<Option<AuthProfile>, String> {
    let state = app.state::<AuthState>();
    let _guard = state.operation.lock().await;
    let initial = state
        .initial_callback
        .lock()
        .map_err(|error| error.to_string())?
        .take();
    if let Some(callback) = initial {
        return accept_callback(&callback).await.map(Some);
    }
    let token = match keyring::Entry::new(KEYRING_SERVICE, "session")
        .map_err(|e| e.to_string())?
        .get_password()
    {
        Ok(token) => token,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    match fetch_profile(&token).await {
        Ok(profile) => Ok(Some(profile)),
        Err(AuthFailure::Unauthorized) => {
            delete_session()?;
            Ok(None)
        }
        Err(AuthFailure::Other(error)) => Err(error),
    }
}

async fn accept_callback(url: &str) -> Result<AuthProfile, String> {
    let (token, callback_user) = callback_credentials(url)?;
    let profile = fetch_profile(&token)
        .await
        .map_err(|failure| match failure {
            AuthFailure::Unauthorized => "Сессия истекла или недействительна".to_owned(),
            AuthFailure::Other(error) => error,
        })?;
    if !profile.username.eq_ignore_ascii_case(&callback_user) {
        return Err("Имя пользователя не совпадает с профилем сервера".into());
    }
    set_session(token)?;
    Ok(profile)
}

#[tauri::command]
pub async fn auth_logout(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AuthState>();
    let _guard = state.operation.lock().await;
    state
        .initial_callback
        .lock()
        .map_err(|error| error.to_string())?
        .take();
    delete_session()?;
    app.emit("auth:profile", Option::<AuthProfile>::None)
        .map_err(|error| error.to_string())
}

fn set_session(token: String) -> Result<(), String> {
    validate_token(&token)?;
    keyring::Entry::new(KEYRING_SERVICE, "session")
        .map_err(|e| e.to_string())?
        .set_password(&token)
        .map_err(|e| e.to_string())
}

fn delete_session() -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, "session").map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

enum AuthFailure {
    Unauthorized,
    Other(String),
}

async fn fetch_profile(token: &str) -> Result<AuthProfile, AuthFailure> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| AuthFailure::Other(e.to_string()))?;
    let response = client
        .get(format!("{API_BASE}/api/me"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| AuthFailure::Other(format!("Ошибка сервера авторизации: {e}")))?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(AuthFailure::Unauthorized);
    }
    if !response.status().is_success() {
        return Err(AuthFailure::Other(format!(
            "Сервер авторизации вернул HTTP {}",
            response.status()
        )));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| AuthFailure::Other(format!("Неверный ответ профиля: {e}")))?;
    let user = body
        .get("user")
        .ok_or_else(|| AuthFailure::Other("В ответе нет профиля пользователя".into()))?;
    let username = user
        .get("username")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AuthFailure::Other("В профиле нет username".into()))?;
    let avatar_url = user
        .get("avatar_url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let expires_at = user
        .get("expires_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(AuthProfile {
        username: username.to_owned(),
        avatar_url: avatar_url.to_owned(),
        expires_at: expires_at.to_owned(),
    })
}

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepo {
    pub full_name: String,
    pub private: bool,
    pub default_branch: String,
    pub updated_at: String,
    pub html_url: String,
}

#[tauri::command]
pub async fn github_repos(app: AppHandle) -> Result<Vec<GithubRepo>, String> {
    let state = app.state::<AuthState>();
    let _guard = state.operation.lock().await;
    let token = match keyring::Entry::new(KEYRING_SERVICE, "session")
        .map_err(|e| e.to_string())?
        .get_password()
    {
        Ok(token) => token,
        Err(keyring::Error::NoEntry) => return Err("Требуется вход через GitHub".into()),
        Err(error) => return Err(error.to_string()),
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(format!("{API_BASE}/api/github/repos"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("Ошибка сервера авторизации: {e}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Неверный ответ сервера: {e}"))?;
    if status == StatusCode::UNAUTHORIZED {
        return Err("Сессия недействительна; войдите заново".into());
    }
    if !status.is_success() {
        let message = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Сервер вернул ошибку");
        return Err(message.to_owned());
    }
    let items = body
        .get("repos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<GithubRepo>(item).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_worker_callback() {
        let token = format!("ncs_{}", "a".repeat(32));
        assert_eq!(
            callback_credentials(&format!("notcode://auth?token={token}&user=notcodeapp")).unwrap(),
            (token, "notcodeapp".into())
        );
    }

    #[test]
    fn rejects_invalid_callbacks() {
        let token = format!("ncs_{}", "a".repeat(32));
        for url in [
            format!("https://auth?token={token}&user=notcodeapp"),
            format!("notcode://other?token={token}&user=notcodeapp"),
            format!("notcode://auth/other?token={token}&user=notcodeapp"),
            format!("notcode://auth?token={token}"),
            format!("notcode://auth?token={token}&token={token}&user=notcodeapp"),
            format!("notcode://auth?token={token}&user=a&user=b"),
            "notcode://auth?token=invalid&user=notcodeapp".into(),
        ] {
            assert!(callback_credentials(&url).is_err());
        }
    }
}
