use crate::{
    state::AppState,
    store::{self, ImportedChat, ImportedMessage},
};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashMap, fs::File, io::Read, path::Path};
use tauri::{AppHandle, State};

const MAX_SOURCE: usize = 8 * 1024 * 1024;
const TICKET_MS: i64 = 10 * 60 * 1000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliImportPreview {
    /// Opaque native file-selection ticket, NOT an absolute filesystem path.
    pub path: String,
    pub digest: String,
    pub provider: String,
    pub external_id: Option<String>,
    pub title: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub project_matches: bool,
    pub message_count: usize,
    pub sample: Vec<CliImportSample>,
}
#[derive(Serialize)]
pub struct CliImportSample {
    pub role: String,
    pub content: String,
}

#[tauri::command]
pub async fn cli_import_preview(
    _app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<CliImportPreview>, String> {
    let pid = {
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| "Project gate poisoned")?;
        store::project_id(&state)?
    };
    let selection = rfd::AsyncFileDialog::new()
        .set_title("Импорт CLI-чата (JSON / JSONL)")
        .add_filter("CLI chat", &["json", "jsonl"])
        .pick_file()
        .await;
    let Some(selection) = selection else {
        return Ok(None);
    };
    let file = selection.path().canonicalize().map_err(|e| e.to_string())?;
    let bytes = read_source(&file)?;
    let chat = parse_chat(&bytes)?;
    let digest = digest(&bytes);
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    if store::project_id(&state)? != pid {
        return Err("Проект изменился; выберите файл заново".into());
    }
    let root = crate::project::root(&state)?;
    let project_matches = cwd_matches(chat.cwd.as_deref(), &root);
    let token = uuid::Uuid::new_v4().to_string();
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    db.execute(
        "DELETE FROM cli_import_tickets WHERE expires_at < ?1",
        [store::now()],
    )
    .map_err(|e| e.to_string())?;
    // At most 16 pending selections per project, with no global statics or frontend path endpoint.
    db.execute("DELETE FROM cli_import_tickets WHERE project_id=?1 AND token NOT IN (SELECT token FROM cli_import_tickets WHERE project_id=?1 ORDER BY expires_at DESC LIMIT 15)", [&pid]).map_err(|e| e.to_string())?;
    db.execute("INSERT INTO cli_import_tickets(token,project_id,path,digest,expires_at) VALUES(?1,?2,?3,?4,?5)",
        params![token, pid, file.to_str().ok_or("Source path is not UTF-8")?, digest, store::now()+TICKET_MS]).map_err(|e| e.to_string())?;
    Ok(Some(CliImportPreview {
        path: token,
        digest,
        provider: chat.provider,
        external_id: chat.source_id,
        title: chat.title,
        model: chat.model,
        cwd: chat.cwd,
        project_matches,
        message_count: chat.messages.len(),
        sample: chat
            .messages
            .iter()
            .take(6)
            .map(|m| CliImportSample {
                role: m.role.clone(),
                content: m.content.chars().take(1200).collect(),
            })
            .collect(),
    }))
}

#[tauri::command]
pub fn cli_import_chat(
    path: String,
    digest: String,
    model: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<store::Session, String> {
    if !approved {
        return Err("Импорт требует явного подтверждения".into());
    }
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let pid = store::project_id(&state)?;
    let root = crate::project::root(&state)?;
    let mut db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    import_ticket(&mut db, &pid, &root, &path, &digest, &model)
}

fn import_ticket(
    db: &mut rusqlite::Connection,
    pid: &str,
    root: &Path,
    path: &str,
    digest: &str,
    model: &str,
) -> Result<store::Session, String> {
    let selected: Option<(String, String)> = db.query_row(
        "SELECT path,digest FROM cli_import_tickets WHERE token=?1 AND project_id=?2 AND expires_at>=?3",
        params![path, pid, store::now()], |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional().map_err(|e| e.to_string())?;
    let (file, expected) = selected
        .ok_or("Предпросмотр истёк или принадлежит другому проекту; выберите файл заново")?;
    if digest != expected {
        return Err("Digest не соответствует предпросмотру".into());
    }
    let bytes = read_source(Path::new(&file))?;
    verify_digest(&bytes, &expected)?;
    let chat = parse_chat(&bytes)?;
    let model = model.trim();
    crate::cli_agent::safe_argument(model)?;
    if chat.provider == "opencode" && !valid_opencode_model(model) {
        return Err("Модель OpenCode должна иметь вид provider/model".into());
    }
    let resume = cwd_matches(chat.cwd.as_deref(), root);
    store::import_chat(db, pid, &chat, &expected, model, resume)
}

fn valid_opencode_model(model: &str) -> bool {
    model
        .split_once('/')
        .is_some_and(|(p, m)| !p.is_empty() && !m.is_empty())
}

fn read_source(path: &Path) -> Result<Vec<u8>, String> {
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("json") || s.eq_ignore_ascii_case("jsonl"))
    {
        return Err("Выберите JSON или JSONL".into());
    }
    // Check the opened handle, not a racy path metadata check; never read more than the budget.
    let file = File::open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_SOURCE as u64 {
        return Err("CLI chat must be a regular file, at most 8 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_SOURCE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_SOURCE {
        return Err("CLI chat exceeds 8 MiB".into());
    }
    Ok(bytes)
}

pub(crate) fn cwd_matches(cwd: Option<&str>, root: &Path) -> bool {
    let Some(cwd) = cwd.filter(|s| Path::new(s).is_absolute()) else {
        return false;
    };
    match (Path::new(cwd).canonicalize(), root.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn metadata(target: &mut Option<String>, value: Option<String>, name: &str) -> Result<(), String> {
    if let Some(value) = value {
        if value.len() > 4096 || value.chars().any(char::is_control) {
            return Err(format!("Invalid {name} metadata"));
        }
        if target.as_ref().is_some_and(|old| old != &value) {
            return Err(format!("Multiple {name} values in one chat"));
        }
        *target = Some(value);
    }
    Ok(())
}
fn model_meta(value: &Value) -> Option<String> {
    let model = value.get("model");
    if let Some(model) = model.and_then(Value::as_str) {
        return Some(model.to_owned());
    }
    let provider = value
        .get("providerID")
        .or_else(|| model.and_then(|m| m.get("providerID")))
        .and_then(Value::as_str)?;
    let id = value
        .get("modelID")
        .or_else(|| model.and_then(|m| m.get("modelID").or_else(|| m.get("id"))))
        .and_then(Value::as_str)?;
    Some(format!("{provider}/{id}"))
}

fn add_message(
    chat: &mut ImportedChat,
    role: &str,
    content: String,
    id: Option<String>,
    time: Option<&Value>,
    seen: &mut HashMap<String, (String, String)>,
) -> Result<(), String> {
    if !["user", "assistant", "system"].contains(&role) || content.trim().is_empty() {
        return Ok(());
    }
    if content.len() > store::MAX_CONTENT {
        return Err("Сообщение превышает 128 KiB; чат не импортирован (без обрезки)".into());
    }
    if let Some(id) = &id {
        if id.len() > 4096 {
            return Err("Source message ID is too long".into());
        }
        if let Some((old_role, old_content)) = seen.get(id) {
            if old_role != role || old_content != &content {
                return Err("Conflicting records with the same source message ID".into());
            }
            return Ok(());
        }
        seen.insert(id.clone(), (role.into(), content.clone()));
    }
    let source_timestamp = time.map(Value::to_string);
    let timestamp = time.and_then(timestamp);
    chat.messages.push(ImportedMessage {
        role: role.into(),
        content,
        source_id: id,
        source_timestamp,
        timestamp,
    });
    Ok(())
}

/// Only explicit text blocks are copied. Tool arguments/results and auth/config objects are never flattened.
fn blocks(content: &Value) -> Result<(String, Vec<String>), String> {
    if let Some(text) = content.as_str() {
        if text.len() > store::MAX_CONTENT {
            return Err("Message exceeds 128 KiB".into());
        }
        return Ok((text.into(), vec![]));
    }
    let parts = content
        .as_array()
        .ok_or("Message content must be text or an array of blocks")?;
    let mut text = String::new();
    let mut tools = Vec::new();
    for part in parts {
        let kind = part
            .get("type")
            .and_then(Value::as_str)
            .ok_or("Invalid content block")?;
        match kind {
            "text" | "input_text" | "output_text" => {
                let value = part
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("Text block has no text")?;
                if text.len() + value.len() + usize::from(!text.is_empty()) > store::MAX_CONTENT {
                    return Err("Message exceeds 128 KiB".into());
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(value);
            }
            "tool_use" | "tool_result" | "tool" => {
                let name = part
                    .get("name")
                    .or_else(|| part.get("tool"))
                    .and_then(Value::as_str)
                    .unwrap_or(kind);
                tools.push(format!(
                    "[Импорт: {kind} {} — содержимое инструмента не выполняется и не копируется]",
                    name.chars().take(120).collect::<String>()
                ));
            }
            _ => {} // images, reasoning, credentials, configuration and opaque payloads
        }
    }
    Ok((text, tools))
}
fn message(
    chat: &mut ImportedChat,
    value: &Value,
    content: &Value,
    id: Option<String>,
    time: Option<&Value>,
    seen: &mut HashMap<String, (String, String)>,
) -> Result<(), String> {
    let role = value
        .get("role")
        .and_then(Value::as_str)
        .ok_or("Message has no role")?;
    let (text, tools) = blocks(content)?;
    add_message(chat, role, text, id.clone(), time, seen)?;
    for (i, text) in tools.into_iter().enumerate() {
        add_message(
            chat,
            "system",
            text,
            id.as_ref().map(|id| format!("{id}:tool:{i}")),
            time,
            seen,
        )?;
    }
    if chat.model.is_none() {
        chat.model = model_meta(value);
    }
    Ok(())
}

fn parse_chat(bytes: &[u8]) -> Result<ImportedChat, String> {
    if bytes.is_empty() || bytes.len() > MAX_SOURCE {
        return Err("Empty or oversized CLI chat".into());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "CLI chat must be UTF-8")?
        .trim_start_matches('\u{feff}');
    let mut chat = ImportedChat {
        provider: String::new(),
        source_id: None,
        title: String::new(),
        model: None,
        cwd: None,
        messages: vec![],
    };
    let mut seen = HashMap::new();
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        if value.get("info").is_some() && value.get("messages").is_some() {
            let info = value
                .get("info")
                .filter(|v| v.is_object())
                .ok_or("Invalid OpenCode info")?;
            chat.provider = "opencode".into();
            metadata(&mut chat.source_id, string(info, "id"), "session ID")?;
            metadata(&mut chat.cwd, string(info, "directory"), "cwd")?;
            chat.title = string(info, "title").unwrap_or_default();
            chat.model = model_meta(info);
            for item in value
                .get("messages")
                .and_then(Value::as_array)
                .ok_or("Invalid OpenCode messages")?
            {
                let info = item
                    .get("info")
                    .filter(|v| v.is_object())
                    .ok_or("Invalid OpenCode message info")?;
                message(
                    &mut chat,
                    info,
                    item.get("parts").ok_or("OpenCode message has no parts")?,
                    string(info, "id"),
                    info.pointer("/time/created"),
                    &mut seen,
                )?;
            }
        }
    }
    if chat.provider.is_empty() {
        let mut records = Vec::new();
        for (line, text) in text.lines().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(text)
                .map_err(|e| format!("Invalid JSONL line {}: {e}", line + 1))?;
            if !value.is_object() {
                return Err(format!("JSONL line {} is not an object", line + 1));
            }
            records.push(value);
        }
        let codex = records.iter().any(|v| {
            matches!(
                v.get("type").and_then(Value::as_str),
                Some("session_meta" | "response_item" | "event_msg")
            )
        });
        let claude = records.iter().any(|v| {
            matches!(
                v.get("type").and_then(Value::as_str),
                Some("user" | "assistant")
            ) && v.get("message").is_some()
        });
        if codex == claude {
            return Err("Неизвестный или смешанный формат CLI-чата".into());
        }
        chat.provider = if codex { "codex" } else { "claude" }.into();
        let canonical = codex
            && records.iter().any(|v| {
                v.get("type").and_then(Value::as_str) == Some("response_item")
                    && v.pointer("/payload/type").and_then(Value::as_str) == Some("message")
            });
        for value in &records {
            let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
            if codex {
                let payload = value.get("payload").unwrap_or(&Value::Null);
                match kind {
                    "session_meta" => {
                        metadata(&mut chat.source_id, string(payload, "id"), "session ID")?;
                        metadata(&mut chat.cwd, string(payload, "cwd"), "cwd")?;
                        if chat.model.is_none() {
                            chat.model = model_meta(payload);
                        }
                    }
                    "turn_context" => {
                        if chat.model.is_none() {
                            chat.model = model_meta(payload);
                        }
                    }
                    "response_item"
                        if payload.get("type").and_then(Value::as_str) == Some("message") =>
                    {
                        message(
                            &mut chat,
                            payload,
                            payload
                                .get("content")
                                .ok_or("Codex message has no content")?,
                            string(payload, "id"),
                            value.get("timestamp"),
                            &mut seen,
                        )?;
                    }
                    "event_msg" if !canonical => {
                        let role = match payload.get("type").and_then(Value::as_str) {
                            Some("user_message") => "user",
                            Some("agent_message") => "assistant",
                            _ => continue,
                        };
                        let text = payload
                            .get("message")
                            .and_then(Value::as_str)
                            .ok_or("Codex event message has no text")?;
                        add_message(
                            &mut chat,
                            role,
                            text.into(),
                            string(payload, "id"),
                            value.get("timestamp"),
                            &mut seen,
                        )?;
                    }
                    _ => {}
                }
            } else if matches!(kind, "user" | "assistant") {
                metadata(
                    &mut chat.source_id,
                    string(value, "sessionId").or_else(|| string(value, "session_id")),
                    "session ID",
                )?;
                metadata(&mut chat.cwd, string(value, "cwd"), "cwd")?;
                let msg = value.get("message").ok_or("Claude record has no message")?;
                message(
                    &mut chat,
                    msg,
                    msg.get("content").ok_or("Claude message has no content")?,
                    string(value, "uuid").or_else(|| string(msg, "id")),
                    value.get("timestamp"),
                    &mut seen,
                )?;
            }
        }
    }
    if chat.messages.is_empty() {
        return Err("В файле нет текстовых сообщений поддерживаемого CLI".into());
    }
    if chat.title.is_empty() {
        chat.title = chat
            .messages
            .iter()
            .find(|m| m.role == "user")
            .unwrap_or(&chat.messages[0])
            .content
            .lines()
            .next()
            .unwrap_or("Imported CLI chat")
            .chars()
            .take(72)
            .collect();
    }
    chat.title = chat
        .title
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    if chat
        .model
        .as_ref()
        .is_some_and(|s| s.len() > 200 || s.chars().any(char::is_control))
    {
        return Err("Invalid source model".into());
    }
    Ok(chat)
}

fn timestamp(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    // RFC3339 timestamps used by Claude/Codex; original representation is also retained verbatim.
    let s = value.as_str()?;
    let (date, time) = s.split_once('T')?;
    let date: Vec<i64> = date
        .split('-')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if date.len() != 3 {
        return None;
    }
    let (y, m, d) = (date[0], date[1], date[2]);
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) {
        return None;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=days[(m - 1) as usize]).contains(&d) {
        return None;
    }
    let (clock, offset) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, 0)
    } else {
        let i = time.find(['+', '-'])?;
        let (clock, zone) = time.split_at(i);
        let (h, min) = zone[1..].split_once(':')?;
        let h: i64 = h.parse().ok()?;
        let min: i64 = min.parse().ok()?;
        if h > 23 || min > 59 {
            return None;
        }
        (
            clock,
            (h * 60 + min) * 60 * if zone.starts_with('-') { -1 } else { 1 },
        )
    };
    let clock: Vec<&str> = clock.split(':').collect();
    if clock.len() != 3 {
        return None;
    }
    let h: i64 = clock[0].parse().ok()?;
    let min: i64 = clock[1].parse().ok()?;
    let (sec, frac) = clock[2].split_once('.').unwrap_or((clock[2], ""));
    let sec: i64 = sec.parse().ok()?;
    if h > 23
        || min > 59
        || sec > 59
        || h < 0
        || min < 0
        || sec < 0
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let ms: i64 = format!("{frac:0<3}")
        .chars()
        .take(3)
        .collect::<String>()
        .parse()
        .ok()?;
    let y = y - i64::from(m <= 2);
    let era = y / 400;
    let yo = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yo * 365 + yo / 4 - yo / 100 + doy;
    Some(((era * 146097 + doe - 719468) * 86400 + h * 3600 + min * 60 + sec - offset) * 1000 + ms)
}

fn verify_digest(bytes: &[u8], expected: &str) -> Result<(), String> {
    if digest(bytes) != expected {
        Err("Файл изменился после предпросмотра; импорт отменён. Выберите файл заново".into())
    } else {
        Ok(())
    }
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn claude() -> Vec<u8> {
        b"{\"type\":\"user\",\"sessionId\":\"thread-1\",\"uuid\":\"m1\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n{\"type\":\"assistant\",\"uuid\":\"m2\",\"message\":{\"role\":\"assistant\",\"model\":\"sonnet\",\"content\":[{\"type\":\"text\",\"text\":\"answer\"},{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"SECRET\"}}]}}".to_vec()
    }
    #[test]
    fn claude_text_tools_ids_and_timestamps() {
        let chat = parse_chat(&claude()).unwrap();
        assert_eq!(chat.provider, "claude");
        assert_eq!(chat.messages.len(), 3);
        assert_eq!(chat.messages[1].content, "answer");
        assert_eq!(chat.messages[2].role, "system");
        assert!(!chat.messages[2].content.contains("SECRET"));
        assert_eq!(chat.messages[0].source_id.as_deref(), Some("m1"));
        assert_eq!(chat.messages[0].timestamp, Some(1767225600000));
        assert_eq!(chat.model.as_deref(), Some("sonnet"));
    }
    #[test]
    fn codex_canonical_avoids_event_copies_and_falls_back() {
        let text = r#"{"type":"session_meta","payload":{"id":"t","cwd":"/project"}}
{"type":"event_msg","payload":{"type":"user_message","message":"hello"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}
{"type":"response_item","payload":{"type":"message","id":"a","role":"assistant","content":[{"type":"output_text","text":"answer"}]}}
{"type":"event_msg","payload":{"type":"agent_message","message":"answer"}}
{"type":"auth","payload":{"token":"secret"}}"#;
        let chat = parse_chat(text.as_bytes()).unwrap();
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.source_id.as_deref(), Some("t"));
        let chat = parse_chat(
            br#"{"type":"event_msg","payload":{"type":"agent_message","message":"fallback"}}"#,
        )
        .unwrap();
        assert_eq!(chat.messages[0].content, "fallback");
    }
    #[test]
    fn opencode_export_metadata_and_parts() {
        let value = json!({"info":{"id":"ses_1","directory":"/project","title":"Test","model":{"providerID":"p","id":"m"}},"messages":[{"info":{"id":"msg_1","role":"user","time":{"created":123}},"parts":[{"type":"text","text":"hi"}]}]});
        let chat = parse_chat(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(chat.provider, "opencode");
        assert_eq!(chat.model.as_deref(), Some("p/m"));
        assert_eq!(chat.messages[0].timestamp, Some(123));
    }
    #[test]
    fn malformed_and_limits_fail_without_truncation() {
        for bytes in [
            b"".as_slice(),
            b"{}",
            b"not json",
            b"{\"type\":\"assistant\",\"message\":{}}",
            b"{\"info\":{},\"messages\":[]}",
        ] {
            assert!(parse_chat(bytes).is_err());
        }
        assert!(parse_chat(&vec![b'x'; MAX_SOURCE + 1]).is_err());
        let value = json!({"type":"user","message":{"role":"user","content":"x".repeat(store::MAX_CONTENT+1)}});
        assert!(parse_chat(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut malformed = claude();
        malformed.extend_from_slice(b"\n{bad}");
        assert!(parse_chat(&malformed).is_err());
    }
    #[test]
    fn digest_vectors_and_changed_source() {
        assert_eq!(
            digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(verify_digest(b"new", &digest(b"old")).is_err());
        assert!(verify_digest(b"old", &digest(b"old")).is_ok());
    }
    #[test]
    fn imports_are_transactional_deduplicated_and_project_scoped() {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        store::initialize(&db).unwrap();
        db.execute(
            "INSERT INTO projects VALUES('p','/p','p'),('q','/q','q')",
            [],
        )
        .unwrap();
        let chat = parse_chat(&claude()).unwrap();
        let hash = digest(&claude());
        let a = store::import_chat(&mut db, "p", &chat, &hash, "sonnet", false).unwrap();
        let b = store::import_chat(&mut db, "p", &chat, "changed-digest", "sonnet", false).unwrap();
        assert_eq!(a.id, b.id);
        assert!(a.external_id.is_none());
        assert_eq!(store::messages(&db, &a.id).unwrap().len(), 3);
        let q = store::import_chat(&mut db, "q", &chat, &hash, "sonnet", true).unwrap();
        assert_ne!(a.id, q.id);
        assert_eq!(q.external_id.as_deref(), Some("thread-1"));
        let mut chat = chat;
        chat.source_id = None;
        let a = store::import_chat(&mut db, "p", &chat, &hash, "sonnet", false).unwrap();
        let b = store::import_chat(&mut db, "p", &chat, &hash, "sonnet", false).unwrap();
        assert_eq!(a.id, b.id);
        chat.messages[0].content = "x".repeat(store::MAX_CONTENT + 1);
        assert!(store::import_chat(&mut db, "p", &chat, "bad", "sonnet", false).is_err());
        assert_eq!(
            db.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
    #[test]
    fn native_tickets_block_arbitrary_paths_expiry_project_switch_and_changed_bytes() {
        let root = std::env::temp_dir();
        let path = root.join(format!(
            "notcode-import-fixture-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, claude()).unwrap();
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        store::initialize(&db).unwrap();
        db.execute(
            "INSERT INTO projects VALUES('p','/p','p'),('q','/q','q')",
            [],
        )
        .unwrap();
        let hash = digest(&claude());
        db.execute(
            "INSERT INTO cli_import_tickets VALUES('ticket','p',?1,?2,?3)",
            params![path.to_str().unwrap(), hash, store::now() + TICKET_MS],
        )
        .unwrap();
        assert!(
            import_ticket(&mut db, "p", &root, path.to_str().unwrap(), &hash, "sonnet").is_err()
        );
        assert!(import_ticket(&mut db, "q", &root, "ticket", &hash, "sonnet").is_err());
        assert!(import_ticket(&mut db, "p", &root, "ticket", "wrong", "sonnet").is_err());
        std::fs::write(&path, b"changed bytes").unwrap();
        assert!(
            import_ticket(&mut db, "p", &root, "ticket", &hash, "sonnet")
                .unwrap_err()
                .contains("изменился")
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        std::fs::write(&path, claude()).unwrap();
        let a = import_ticket(&mut db, "p", &root, "ticket", &hash, "sonnet").unwrap();
        let b = import_ticket(&mut db, "p", &root, "ticket", &hash, "sonnet").unwrap();
        assert_eq!(a.id, b.id);
        db.execute("UPDATE cli_import_tickets SET expires_at=0", [])
            .unwrap();
        assert!(import_ticket(&mut db, "p", &root, "ticket", &hash, "sonnet").is_err());
        let file = File::create(&path).unwrap();
        file.set_len((MAX_SOURCE + 1) as u64).unwrap();
        drop(file);
        assert!(read_source(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn failed_message_insert_rolls_back_session_and_provenance() {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        store::initialize(&db).unwrap();
        db.execute("INSERT INTO projects VALUES('p','/p','p')", [])
            .unwrap();
        db.execute_batch("CREATE TRIGGER synthetic_failure BEFORE INSERT ON messages WHEN NEW.role='assistant' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(store::import_chat(
            &mut db,
            "p",
            &parse_chat(&claude()).unwrap(),
            "hash",
            "sonnet",
            false
        )
        .is_err());
        for table in [
            "sessions",
            "messages",
            "cli_import_sources",
            "cli_import_messages",
        ] {
            assert_eq!(
                db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }
    #[test]
    fn repeated_source_message_ids_dedup_but_conflicting_text_fails() {
        let mut bytes = claude();
        bytes.push(b'\n');
        bytes.extend_from_slice(&claude());
        assert_eq!(parse_chat(&bytes).unwrap().messages.len(), 3);
        let changed = String::from_utf8(claude())
            .unwrap()
            .replace("hello", "different");
        let mut bytes = claude();
        bytes.push(b'\n');
        bytes.extend_from_slice(changed.as_bytes());
        assert!(parse_chat(&bytes).is_err());
    }
    #[test]
    fn cwd_missing_relative_or_other_project_cannot_resume() {
        let root = std::env::temp_dir();
        assert!(cwd_matches(root.to_str(), &root));
        assert!(!cwd_matches(None, &root));
        assert!(!cwd_matches(Some("relative"), &root));
        assert!(!cwd_matches(Some("/does-not-exist-notcode-fixture"), &root));
    }
    #[test]
    fn timestamps_accept_offsets_and_fractional_seconds() {
        assert_eq!(timestamp(&json!("1970-01-01T01:00:00+01:00")), Some(0));
        assert_eq!(timestamp(&json!("1970-01-01T00:00:00.123Z")), Some(123));
        assert!(timestamp(&json!("2026-02-30T00:00:00Z")).is_none());
    }
}
