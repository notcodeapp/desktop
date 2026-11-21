use crate::state::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::State;

pub const MAX_CONTENT: usize = 128 * 1024;
pub const PROVIDERS: &[&str] = &["ollama", "compatible", "codex", "claude", "opencode"];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub updated_at: i64,
    /// Thread/session id owned by an external CLI agent, used to resume its context.
    pub external_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub arguments: String,
    pub result: String,
    pub status: String,
    pub created_at: i64,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn initialize(db: &Connection) -> Result<(), String> {
    db.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    db.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id),
            title TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, updated_at INTEGER NOT NULL,
            external_id TEXT
         );
         CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_id, updated_at);
         CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            role TEXT NOT NULL CHECK(role IN ('user','assistant','system')),
            content TEXT NOT NULL, created_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS messages_session ON messages(session_id, created_at);
         CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS audit_log (
            id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            name TEXT NOT NULL, arguments TEXT NOT NULL, result TEXT NOT NULL,
            status TEXT NOT NULL, created_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS audit_log_session ON audit_log(session_id, created_at);
         CREATE TABLE IF NOT EXISTS cli_import_sources (
            project_id TEXT NOT NULL REFERENCES projects(id), provider TEXT NOT NULL,
            source_id TEXT NOT NULL, digest TEXT NOT NULL,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            imported_at INTEGER NOT NULL, cwd TEXT,
            UNIQUE(project_id,provider,source_id)
         );
         CREATE TABLE IF NOT EXISTS cli_import_messages (
            message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
            source_message_id TEXT, source_timestamp TEXT, ordinal INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS cli_import_tickets (
            token TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id),
            path TEXT NOT NULL, digest TEXT NOT NULL, expires_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS provider_profiles (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, base_url TEXT NOT NULL,
            models TEXT NOT NULL, created_at INTEGER NOT NULL
         );"
    ).map_err(|e| e.to_string())?;
    if let Err(error) = db.execute("ALTER TABLE sessions ADD COLUMN external_id TEXT", []) {
        if !error.to_string().contains("duplicate column") {
            return Err(error.to_string());
        }
    }
    Ok(())
}

/// Caller holds project_gate so this id cannot become stale before the operation.
pub fn project_id(state: &AppState) -> Result<String, String> {
    state
        .project
        .lock()
        .map_err(|_| "Project lock poisoned")?
        .as_ref()
        .map(|p| p.id.clone())
        .ok_or_else(|| "Open a project first".into())
}

/// Appends one tool invocation to the audit trail. Truncates oversized payloads so a
/// single large tool output cannot bloat the database.
pub fn record_audit(
    db: &Connection,
    session_id: &str,
    name: &str,
    arguments: &str,
    result: &str,
    status: &str,
) -> Result<(), String> {
    let truncate = |s: &str| -> String {
        if s.len() <= MAX_CONTENT {
            s.to_owned()
        } else {
            let mut end = MAX_CONTENT;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}… [truncated {} bytes]", &s[..end], s.len() - end)
        }
    };
    db.execute(
        "INSERT INTO audit_log(id,session_id,name,arguments,result,status,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![uuid::Uuid::new_v4().to_string(), session_id, name, truncate(arguments), truncate(result), status, now()],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn session_audit(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<AuditEntry>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let pid = project_id(&state)?;
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    find_session(&db, &pid, &session_id)?;
    audit_entries(&db, &session_id)
}

pub fn audit_entries(db: &Connection, session_id: &str) -> Result<Vec<AuditEntry>, String> {
    let mut stmt = db.prepare("SELECT id,session_id,name,arguments,result,status,created_at FROM audit_log WHERE session_id=?1 ORDER BY created_at,rowid")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([session_id], |r| {
            Ok(AuditEntry {
                id: r.get(0)?,
                session_id: r.get(1)?,
                name: r.get(2)?,
                arguments: r.get(3)?,
                result: r.get(4)?,
                status: r.get(5)?,
                created_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn find_session(db: &Connection, project_id: &str, id: &str) -> Result<Session, String> {
    db.query_row(
        "SELECT id,title,provider,model,updated_at,external_id FROM sessions WHERE id=?1 AND project_id=?2",
        params![id, project_id],
        |r| Ok(Session { id: r.get(0)?, title: r.get(1)?, provider: r.get(2)?, model: r.get(3)?, updated_at: r.get(4)?, external_id: r.get(5)? }),
    ).optional().map_err(|e| e.to_string())?.ok_or_else(|| "Session not found in this project".into())
}

pub fn messages(db: &Connection, id: &str) -> Result<Vec<Message>, String> {
    let mut stmt = db.prepare("SELECT id,session_id,role,content,created_at FROM messages WHERE session_id=?1 ORDER BY created_at,rowid")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([id], |r| {
            Ok(Message {
                id: r.get(0)?,
                session_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                created_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sessions_list(state: State<'_, AppState>) -> Result<Vec<Session>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let pid = project_id(&state)?;
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    let mut stmt = db.prepare("SELECT id,title,provider,model,updated_at,external_id FROM sessions WHERE project_id=?1 ORDER BY updated_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([pid], |r| {
            Ok(Session {
                id: r.get(0)?,
                title: r.get(1)?,
                provider: r.get(2)?,
                model: r.get(3)?,
                updated_at: r.get(4)?,
                external_id: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn session_create(
    provider: String,
    model: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    if !PROVIDERS.contains(&provider.as_str()) {
        return Err("Unknown AI provider".into());
    }
    let model = model.trim().to_owned();
    if model.is_empty() || model.len() > 200 || model.chars().any(char::is_control) {
        return Err("A valid model name is required (maximum 200 bytes)".into());
    }
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let pid = project_id(&state)?;
    let session = Session {
        id: uuid::Uuid::new_v4().to_string(),
        title: "New session".into(),
        provider,
        model,
        updated_at: now(),
        external_id: None,
    };
    state.db.lock().map_err(|_| "Database lock poisoned")?.execute(
        "INSERT INTO sessions(id,project_id,title,provider,model,updated_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![session.id,pid,session.title,session.provider,session.model,session.updated_at],
    ).map_err(|e| e.to_string())?;
    Ok(session)
}

/// Форк диалога: полная копия сессии (провайдер, модель, сообщения) в новый
/// чат того же проекта. Caller holds project_gate.
pub fn session_fork_locked(
    state: &AppState,
    pid: &str,
    session_id: &str,
) -> Result<Session, String> {
    let mut db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    let source = find_session(&db, pid, session_id)?;
    let fork = Session {
        id: uuid::Uuid::new_v4().to_string(),
        title: format!("{} (копия)", source.title.trim()).chars().take(200).collect(),
        provider: source.provider.clone(),
        model: source.model.clone(),
        updated_at: now(),
        external_id: None,
    };
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO sessions(id,project_id,title,provider,model,updated_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![fork.id,pid,fork.title,fork.provider,fork.model,fork.updated_at],
    ).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO messages(id,session_id,role,content,created_at) SELECT ?1,session_id,role,content,created_at FROM messages WHERE session_id=?2 ORDER BY created_at,rowid",
        params![fork.id, session_id],
    ).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(fork)
}

/// Caller holds project_gate and has verified the session belongs to the current project.
pub fn set_external_id(db: &Connection, session_id: &str, external_id: &str) -> Result<(), String> {
    db.execute(
        "UPDATE sessions SET external_id=?1 WHERE id=?2",
        params![external_id, session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn session_messages(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Message>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let pid = project_id(&state)?;
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    find_session(&db, &pid, &session_id)?;
    messages(&db, &session_id)
}

/// Форк диалога: копия сообщений в новый чат текущего проекта.
#[tauri::command]
pub fn session_fork(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let pid = project_id(&state)?;
    if state
        .generations
        .lock()
        .map_err(|_| "Generation lock poisoned")?
        .contains_key(&session_id)
    {
        return Err("Interrupt this session and wait for generation to finish before forking it".into());
    }
    session_fork_locked(&state, &pid, &session_id)
}

#[tauri::command]
pub fn session_delete(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let pid = project_id(&state)?;
    if state
        .generations
        .lock()
        .map_err(|_| "Generation lock poisoned")?
        .contains_key(&session_id)
    {
        return Err(
            "Interrupt this session and wait for generation to finish before deleting it".into(),
        );
    }
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    find_session(&db, &pid, &session_id)?;
    db.execute(
        "DELETE FROM sessions WHERE id=?1 AND project_id=?2",
        params![session_id, pid],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityDay {
    pub date: String,
    pub messages: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStats {
    pub sessions: i64,
    pub messages: i64,
    pub tool_calls: i64,
    pub active_days: i64,
    /// Consecutive active days ending today (or yesterday when today is still quiet).
    pub streak: i64,
    pub peak_day: Option<String>,
    pub peak_day_messages: i64,
    /// Most recent 14 days, oldest first, zero-filled.
    pub days: Vec<ActivityDay>,
}

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const STATS_WINDOW_DAYS: i64 = 365;
const STATS_CHART_DAYS: usize = 14;

/// Formats a local-epoch date key (days since 1970-01-01 local) as YYYY-MM-DD.
fn format_date_key(key: i64) -> String {
    // Civil-from-days algorithm (Howard Hinnant); keys are local days since epoch.
    let z = key + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Aggregates local activity across all projects. Pure over the connection so
/// tests can drive it with an in-memory database.
pub fn compute_activity(db: &Connection, now_ms: i64) -> Result<ActivityStats, String> {
    let sessions: i64 = db
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let tool_calls: i64 = db
        .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let window_start = now_ms - STATS_WINDOW_DAYS * DAY_MS;
    // Date math happens in SQLite with the 'localtime' modifier so the user's
    // own day boundaries define "today".
    let mut stmt = db.prepare(
        "SELECT CAST(strftime('%s', date(created_at/1000,'unixepoch','localtime')) AS INTEGER)/86400 AS day, COUNT(*)
         FROM messages WHERE created_at >= ?1 GROUP BY day",
    )
    .map_err(|e| e.to_string())?;
    let per_day: std::collections::HashMap<i64, i64> = stmt
        .query_map([window_start], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let today_key: i64 = db
        .query_row(
            "SELECT CAST(strftime('%s', date('now','localtime')) AS INTEGER)/86400",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut messages = 0;
    let mut peak_day = None;
    let mut peak_day_messages = 0;
    for (key, count) in &per_day {
        messages += count;
        if *count > peak_day_messages {
            peak_day_messages = *count;
            peak_day = Some(format_date_key(*key));
        }
    }
    // Allow the streak to survive until the end of "today" even before any message arrives.
    let streak_start = if per_day.contains_key(&today_key) {
        today_key
    } else {
        today_key - 1
    };
    let mut streak = 0;
    let mut key = streak_start;
    while per_day.contains_key(&key) {
        streak += 1;
        key -= 1;
    }
    let mut days = Vec::with_capacity(STATS_CHART_DAYS);
    for offset in (0..STATS_CHART_DAYS as i64).rev() {
        let key = today_key - offset;
        days.push(ActivityDay {
            date: format_date_key(key),
            messages: per_day.get(&key).copied().unwrap_or(0),
        });
    }
    Ok(ActivityStats {
        sessions,
        messages,
        tool_calls,
        active_days: per_day.len() as i64,
        streak,
        peak_day,
        peak_day_messages,
        days,
    })
}

/// Account-level statistics across every project; intentionally usable with no
/// project open (read-only aggregates, no project_gate).
#[tauri::command]
pub fn activity_stats(state: State<'_, AppState>) -> Result<ActivityStats, String> {
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    compute_activity(&db, now())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHit {
    pub session_id: String,
    pub session_title: String,
    pub snippet: String,
}

/// Поиск по содержимому всех чатов текущего проекта (LIKE, до 50 совпадений).
#[tauri::command]
pub fn chats_search(query: String, state: State<'_, AppState>) -> Result<Vec<ChatHit>, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.len() > 256 {
        return Err("Поиск: 1–256 символов".into());
    }
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    let mut stmt = db
        .prepare(
            r#"SELECT s.id, s.title, substr(m.content, max(1, instr(lower(m.content), lower(?1)) - 40), 120)
               FROM messages m JOIN sessions s ON s.id = m.session_id
               WHERE s.project_id = ?2 AND instr(lower(m.content), lower(?1)) > 0
               ORDER BY s.updated_at DESC, m.created_at LIMIT 50;"#,
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![trimmed, project_id(&state)?], |row| {
            Ok(ChatHit {
                session_id: row.get(0)?,
                session_title: row.get(1)?,
                snippet: {
                    let raw: String = row.get(2)?;
                    raw.replace('\u{1E}', " ").replace('\n', " ")
                },
            })
        })
        .map_err(|e| e.to_string())?;
    let mut hits: Vec<ChatHit> = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    hits.dedup_by(|a, b| a.session_id == b.session_id);
    Ok(hits)
}

/// Source identifiers/timestamps stay in provenance; local UUIDs cannot collide across projects.
#[derive(Clone, Debug)]
pub(crate) struct ImportedMessage {
    pub role: String,
    pub content: String,
    pub source_id: Option<String>,
    pub source_timestamp: Option<String>,
    pub timestamp: Option<i64>,
}

pub(crate) struct ImportedChat {
    pub provider: String,
    pub source_id: Option<String>,
    pub title: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub messages: Vec<ImportedMessage>,
}

pub(crate) fn import_chat(
    db: &mut Connection,
    pid: &str,
    chat: &ImportedChat,
    digest: &str,
    model: &str,
    resume: bool,
) -> Result<Session, String> {
    if !["claude", "codex", "opencode"].contains(&chat.provider.as_str())
        || chat.messages.is_empty()
    {
        return Err("Invalid CLI chat".into());
    }
    crate::cli_agent::safe_argument(model)?;
    if resume {
        if let Some(id) = &chat.source_id {
            crate::cli_agent::safe_argument(id)?;
        }
    }
    for message in &chat.messages {
        if !["user", "assistant", "system"].contains(&message.role.as_str())
            || message.content.len() > MAX_CONTENT
        {
            return Err("Invalid or oversized imported message".into());
        }
    }
    let source = chat
        .source_id
        .clone()
        .unwrap_or_else(|| format!("sha256:{digest}"));
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let existing: Option<String> = tx.query_row(
        "SELECT session_id FROM cli_import_sources WHERE project_id=?1 AND provider=?2 AND source_id=?3",
        params![pid, chat.provider, source], |r| r.get(0),
    ).optional().map_err(|e| e.to_string())?;
    if let Some(id) = existing {
        return find_session(&tx, pid, &id);
    }
    let live: Option<String> = if let Some(id) = &chat.source_id {
        tx.query_row("SELECT id FROM sessions WHERE project_id=?1 AND provider=?2 AND external_id=?3 LIMIT 1",
            params![pid, chat.provider, id], |r| r.get(0)).optional().map_err(|e| e.to_string())?
    } else {
        None
    };
    let session = if let Some(id) = live {
        find_session(&tx, pid, &id)?
    } else {
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            title: chat.title.clone(),
            provider: chat.provider.clone(),
            model: model.to_owned(),
            updated_at: now(),
            external_id: if resume { chat.source_id.clone() } else { None },
        };
        tx.execute("INSERT INTO sessions(id,project_id,title,provider,model,updated_at,external_id) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![session.id, pid, session.title, session.provider, session.model, session.updated_at, session.external_id])
            .map_err(|e| e.to_string())?;
        // Preserve file order even if source timestamps go backwards. Keep the exact original separately.
        let mut previous = chat
            .messages
            .first()
            .and_then(|m| m.timestamp)
            .unwrap_or(session.updated_at)
            .saturating_sub(1);
        for (ordinal, message) in chat.messages.iter().enumerate() {
            let id = uuid::Uuid::new_v4().to_string();
            let timestamp = message
                .timestamp
                .unwrap_or(previous.saturating_add(1))
                .max(previous.saturating_add(1));
            previous = timestamp;
            tx.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,?2,?3,?4,?5)",
                params![id, session.id, message.role, message.content, timestamp]).map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO cli_import_messages(message_id,source_message_id,source_timestamp,ordinal) VALUES(?1,?2,?3,?4)",
                params![id, message.source_id, message.source_timestamp, ordinal as i64]).map_err(|e| e.to_string())?;
        }
        session
    };
    tx.execute("INSERT INTO cli_import_sources(project_id,provider,source_id,digest,session_id,imported_at,cwd) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![pid, chat.provider, source, digest, session.id, now(), chat.cwd]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        db
    }

    #[test]
    fn projects_are_isolated_and_delete_cascades() {
        let db = db();
        db.execute(
            "INSERT INTO projects VALUES('p1','/one','one'),('p2','/two','two')",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO sessions(id,project_id,title,provider,model,updated_at) VALUES('s','p1','title','ollama','model',1)", []).unwrap();
        db.execute("INSERT INTO messages VALUES('m','s','user','hello',1)", [])
            .unwrap();
        assert!(find_session(&db, "p2", "s").is_err());
        assert_eq!(messages(&db, "s").unwrap().len(), 1);
        db.execute("DELETE FROM sessions WHERE id='s'", []).unwrap();
        assert!(messages(&db, "s").unwrap().is_empty());
    }

    #[test]
    fn roles_and_foreign_keys_are_enforced() {
        let db = db();
        assert!(db.execute("INSERT INTO sessions(id,project_id,title,provider,model,updated_at) VALUES('s','missing','t','ollama','m',1)", []).is_err());
        db.execute("INSERT INTO projects VALUES('p','/one','one')", [])
            .unwrap();
        db.execute("INSERT INTO sessions(id,project_id,title,provider,model,updated_at) VALUES('s','p','t','ollama','m',1)", []).unwrap();
        assert!(db
            .execute("INSERT INTO messages VALUES('m','s','tool','x',1)", [])
            .is_err());
    }

    #[test]
    fn audit_entries_are_isolated_and_cascade() {
        let db = db();
        db.execute("INSERT INTO projects VALUES('p','/one','one')", [])
            .unwrap();
        db.execute("INSERT INTO sessions(id,project_id,title,provider,model,updated_at) VALUES('s','p','t','ollama','m',1)", []).unwrap();
        record_audit(&db, "s", "read_file", "{\"path\":\"a\"}", "ok", "ok").unwrap();
        record_audit(&db, "s", "write_file", "{}", "boom", "error").unwrap();
        let entries = audit_entries(&db, "s").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, "ok");
        assert_eq!(entries[1].status, "error");
        assert!(record_audit(&db, "missing", "x", "", "", "ok").is_err());
        db.execute("DELETE FROM sessions WHERE id='s'", []).unwrap();
        assert!(audit_entries(&db, "s").unwrap().is_empty());
    }

    #[test]
    fn activity_stats_aggregate_days_streak_and_peak() {
        let db = db();
        db.execute("INSERT INTO projects VALUES('p','/one','one')", [])
            .unwrap();
        db.execute("INSERT INTO sessions(id,project_id,title,provider,model,updated_at) VALUES('s','p','t','ollama','m',1)", []).unwrap();
        // Local-day math must match the SQL the command itself uses.
        let offset: i64 = db.query_row(
            "SELECT CAST(strftime('%s','now','localtime') AS INTEGER)-CAST(strftime('%s','now') AS INTEGER)",
            [], |r| r.get(0),
        ).unwrap();
        let today_key: i64 = db
            .query_row(
                "SELECT CAST(strftime('%s', date('now','localtime')) AS INTEGER)/86400",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let at = |key: i64, hour: i64| (key * 86400 + hour * 3600 - offset) * 1000;
        let stamps = [
            at(today_key, 10),
            at(today_key, 11),                    // today: 2
            at(today_key - 1, 9),
            at(today_key - 1, 10),
            at(today_key - 1, 12),
            at(today_key - 1, 15),
            at(today_key - 1, 23),                // yesterday: 5 (peak)
            at(today_key - 2, 8),                 // two days ago: 1
            at(today_key - 400, 8),               // outside the 365-day window
        ];
        for (i, ts) in stamps.iter().enumerate() {
            db.execute(
                "INSERT INTO messages VALUES(?1,'s','user','x',?2)",
                params![format!("m{i}"), ts],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO audit_log VALUES('a','s','write_file','{}','ok','ok',?1)",
            [at(today_key, 10)],
        )
        .unwrap();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let stats = compute_activity(&db, now_ms).unwrap();
        assert_eq!(stats.sessions, 1);
        assert_eq!(stats.messages, 8);
        assert_eq!(stats.tool_calls, 1);
        assert_eq!(stats.active_days, 3);
        assert_eq!(stats.streak, 3);
        assert_eq!(stats.peak_day_messages, 5);
        assert_eq!(stats.peak_day.as_deref(), Some(format_date_key(today_key - 1).as_str()));
        assert_eq!(stats.days.len(), 14);
        assert_eq!(stats.days.last().unwrap().messages, 2);
        assert_eq!(stats.days.first().unwrap().date, format_date_key(today_key - 13));
    }

    #[test]
    fn activity_stats_are_zero_on_a_fresh_database() {
        let db = db();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let stats = compute_activity(&db, now_ms).unwrap();
        assert_eq!(stats.sessions, 0);
        assert_eq!(stats.messages, 0);
        assert_eq!(stats.tool_calls, 0);
        assert_eq!(stats.active_days, 0);
        assert_eq!(stats.streak, 0);
        assert_eq!(stats.peak_day, None);
        assert_eq!(stats.peak_day_messages, 0);
        assert!(stats.days.iter().all(|d| d.messages == 0));
        assert_eq!(stats.days.len(), 14);
    }

    #[test]
    fn external_id_survives_reopen_and_legacy_databases_migrate() {
        let dir = std::env::temp_dir().join(format!("notcode-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("workspace.sqlite3");
        {
            let db = Connection::open(&path).unwrap();
            // A database written before external_id existed.
            db.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL);
                 CREATE TABLE sessions (id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id),
                    title TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, updated_at INTEGER NOT NULL);").unwrap();
            db.execute("INSERT INTO projects VALUES('p','/one','one')", [])
                .unwrap();
            db.execute("INSERT INTO sessions VALUES('s','p','t','codex','m',1)", [])
                .unwrap();
        }
        let db = Connection::open(&path).unwrap();
        initialize(&db).unwrap();
        initialize(&db).unwrap();
        assert_eq!(find_session(&db, "p", "s").unwrap().external_id, None);
        set_external_id(&db, "s", "thread-1").unwrap();
        assert_eq!(
            find_session(&db, "p", "s").unwrap().external_id.as_deref(),
            Some("thread-1")
        );
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
