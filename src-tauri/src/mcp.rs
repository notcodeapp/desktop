use crate::state::AppState;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tauri::{Manager, State};

const PROTOCOL: &str = "2024-11-05";
const MAX_FRAME: usize = 1024 * 1024;
const STDERR_TAIL: usize = 16 * 1024;
const STDERR_TOTAL: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);
type Reply = Result<Value, String>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    id: String,
    name: String,
    command: String,
    args: Vec<String>,
    connected: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_schema: Value,
}

pub struct McpHandle {
    connection: Arc<Connection>,
}
impl Drop for McpHandle {
    fn drop(&mut self) {
        self.connection.shutdown("MCP server disconnected");
    }
}
pub(crate) struct Connection {
    child: Mutex<Child>,
    outgoing: SyncSender<Vec<u8>>,
    pending: Mutex<HashMap<u64, SyncSender<Reply>>>,
    next_id: AtomicU64,
    alive: AtomicBool,
    ready: AtomicBool,
    stderr: Mutex<Vec<u8>>,
    failure: Mutex<Option<String>>,
}
impl Connection {
    fn detail(&self, message: &str) -> String {
        let tail = self
            .stderr
            .lock()
            .ok()
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .chars()
                    .take(1024)
                    .collect::<String>()
            })
            .unwrap_or_default();
        if tail.is_empty() {
            message.to_owned()
        } else {
            format!("{message}\nServer stderr (bounded): {tail}")
        }
    }
    fn shutdown(&self, reason: &str) {
        if !self.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        self.ready.store(false, Ordering::Release);
        let reason = self.detail(reason);
        if let Ok(mut failure) = self.failure.lock() {
            *failure = Some(reason.clone());
        }
        // Kill is non-waiting. The monitor thread owns reaping, including after Drop.
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
        if let Ok(mut pending) = self.pending.lock() {
            for (_, sender) in pending.drain() {
                let _ = sender.try_send(Err(reason.clone()));
            }
        }
    }
    fn unavailable(&self) -> String {
        self.failure
            .lock()
            .ok()
            .and_then(|r| r.clone())
            .unwrap_or_else(|| "MCP server is disconnected".into())
    }
    fn send(&self, value: Value) -> Result<(), String> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(self.unavailable());
        }
        let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_FRAME {
            return Err("MCP message exceeds 1 MiB".into());
        }
        bytes.push(b'\n');
        self.outgoing
            .try_send(bytes)
            .map_err(|_| "MCP outgoing queue is full or closed".to_owned())
    }
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Reply {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel(1);
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| "MCP request lock poisoned")?;
            if !self.alive.load(Ordering::Acquire) {
                return Err(self.unavailable());
            }
            if pending.len() >= 16 {
                return Err("MCP server already has 16 pending requests".into());
            }
            pending.insert(id, sender);
        }
        if let Err(error) =
            self.send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
        {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }
        // The writer has its own bounded queue/thread: a server that never reads stdin
        // cannot block this timeout before it starts.
        match receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.shutdown("MCP request timed out; the server was terminated");
                Err(self.unavailable())
            }
            Err(_) => Err(self.unavailable()),
        }
    }
    fn notification(&self, method: &str) -> Result<(), String> {
        self.send(json!({"jsonrpc":"2.0", "method":method}))
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Ok(child) = self.child.get_mut() {
            let _ = child.kill();
            let _ = child.try_wait();
        }
    }
}

// MCP stdio uses newline-delimited JSON, NOT HTTP Content-Length framing.
// BufRead::read_line/read_until alone would allocate without a frame bound.
fn frame<R: BufRead>(reader: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|e| e.to_string())?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err("MCP stdout ended in an incomplete JSON line".into())
            };
        }
        let end = available.iter().position(|b| *b == b'\n');
        let count = end.map(|i| i + 1).unwrap_or(available.len());
        if bytes.len() + count > MAX_FRAME + 1 {
            return Err("MCP stdout frame exceeds 1 MiB".into());
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if end.is_some() {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            return Ok(Some(bytes));
        }
    }
}

fn response(value: &Value) -> Result<(u64, Reply), String> {
    let id = value
        .get("id")
        .and_then(Value::as_u64)
        .ok_or("MCP response has an invalid request id")?;
    match (value.get("result"), value.get("error")) {
        (Some(result), None) => Ok((id, Ok(result.clone()))),
        (None, Some(error)) => {
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .ok_or("Invalid MCP error code")?;
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .ok_or("Invalid MCP error message")?;
            Ok((
                id,
                Err(format!(
                    "MCP error {code}: {}",
                    message.chars().take(2048).collect::<String>()
                )),
            ))
        }
        _ => Err("MCP response must contain exactly one result or error".into()),
    }
}
fn route(connection: &Connection, value: Value) -> Result<(), String> {
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !value.is_object() {
        return Err("MCP stdout is not a JSON-RPC 2.0 message".into());
    }
    if let Some(method) = value.get("method") {
        let method = method.as_str().ok_or("Invalid MCP method")?;
        if let Some(id) = value.get("id") {
            if !(id.is_string() || id.as_u64().is_some() || id.as_i64().is_some()) {
                return Err("Invalid MCP server request id".into());
            }
            // No roots, sampling, elicitation or AI orchestration capabilities are advertised.
            // Reply rather than leaving unsupported server requests hanging forever.
            let reply = if method == "ping" {
                json!({"jsonrpc":"2.0", "id":id, "result":{}})
            } else {
                json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"Client capability not supported"}})
            };
            connection.send(reply)?;
        }
        // Notifications (logging, tools/list_changed, progress, etc.) need no reply.
        return Ok(());
    }
    let (id, result) = response(&value)?;
    let sender = connection
        .pending
        .lock()
        .map_err(|_| "MCP request lock poisoned")?
        .remove(&id);
    if let Some(sender) = sender {
        let _ = sender.try_send(result);
    }
    // Ignore late/unknown response IDs; never route one to an unrelated request.
    Ok(())
}

fn validate_config(name: &str, command: &str, args: &[String]) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err("Server name must contain 1–128 bytes without control characters".into());
    }
    if command.trim().is_empty() || command.len() > 4096 || command.chars().any(char::is_control) {
        return Err("Specify an executable, not an empty command or shell command line".into());
    }
    if args.len() > 128
        || args.iter().any(|a| a.len() > 8192 || a.contains('\0'))
        || args.iter().map(String::len).sum::<usize>() > 65536
    {
        return Err("MCP arguments exceed the 128 argument / 64 KiB limit, or contain NUL".into());
    }
    Ok(())
}
fn executable(command: &str, root: &Path) -> Result<PathBuf, String> {
    let path = Path::new(command);
    let candidate = if path.is_absolute() {
        path.canonicalize()
            .map_err(|e| format!("Cannot resolve MCP executable: {e}"))?
    } else {
        if command.contains(['/', '\\', ':']) {
            return Err("Use an absolute executable path or a bare executable name".into());
        }
        let mut found = None;
        for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            if !directory.is_absolute() {
                continue;
            }
            let directory = match directory.canonicalize() {
                Ok(p) if !p.starts_with(root) => p,
                _ => continue,
            };
            #[cfg(windows)]
            let name = if command.to_ascii_lowercase().ends_with(".exe") {
                command.to_owned()
            } else {
                format!("{command}.exe")
            };
            #[cfg(not(windows))]
            let name = command.to_owned();
            let candidate = directory.join(name);
            if candidate.is_file() {
                let candidate = candidate.canonicalize().map_err(|e| e.to_string())?;
                if !candidate.starts_with(root) {
                    found = Some(candidate);
                    break;
                }
            }
        }
        found.ok_or("MCP executable not found on the absolute system PATH (project directories are excluded)")?
    };
    if !candidate.is_file() {
        return Err("MCP command must be a regular executable file".into());
    }
    // Rust/Windows may implicitly wrap .cmd/.bat in cmd.exe. Avoid that injection
    // surface entirely: configure node.exe + a script argument instead of npx.cmd.
    #[cfg(windows)]
    if !candidate
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("exe"))
        .unwrap_or(false)
    {
        return Err("Windows MCP servers require a native .exe; use node.exe with the server script as an argument, not a .cmd/.bat wrapper".into());
    }
    Ok(candidate)
}
fn spawn(config: &McpConfig, root: &Path) -> Result<Arc<Connection>, String> {
    let mut command = Command::new(executable(&config.command, root)?);
    command
        .args(&config.args)
        .current_dir(root)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // No user-specified env object exists, and ambient tokens, API keys, NODE_OPTIONS,
    // PYTHONPATH and shell startup settings are not inherited by local servers.
    for key in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "PATHEXT",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot launch MCP server: {e}"))?;
    let pipes = match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
        (Some(stdin), Some(stdout), Some(stderr)) => (stdin, stdout, stderr),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("MCP server pipes are unavailable".into());
        }
    };
    let (outgoing, receiver) = mpsc::sync_channel::<Vec<u8>>(32);
    let connection = Arc::new(Connection {
        child: Mutex::new(child),
        outgoing,
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        alive: AtomicBool::new(true),
        ready: AtomicBool::new(false),
        stderr: Mutex::new(Vec::new()),
        failure: Mutex::new(None),
    });
    let (mut stdin, stdout, mut stderr) = pipes;
    let setup = (|| {
        // Start the reaper first, ensuring every later thread-setup failure is reaped.
        let monitor = connection.clone();
        thread::Builder::new()
            .name("notcode-mcp-wait".into())
            .spawn(move || loop {
                let result = match monitor.child.lock() {
                    Ok(mut child) => child.try_wait(),
                    Err(_) => break,
                };
                match result {
                    Ok(Some(status)) => {
                        monitor.shutdown(&format!("MCP server exited ({status})"));
                        break;
                    }
                    Err(error) => {
                        monitor.shutdown(&format!("Cannot wait for MCP server: {error}"));
                        break;
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(50)),
                }
            })
            .map_err(|e| e.to_string())?;
        let writer = connection.clone();
        thread::Builder::new()
            .name("notcode-mcp-writer".into())
            .spawn(move || {
                while writer.alive.load(Ordering::Acquire) {
                    match receiver.recv_timeout(Duration::from_millis(250)) {
                        Ok(bytes) => {
                            if let Err(error) = stdin.write_all(&bytes).and_then(|_| stdin.flush())
                            {
                                writer.shutdown(&format!("MCP stdin failed: {error}"));
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let reader = connection.clone();
        thread::Builder::new()
            .name("notcode-mcp-reader".into())
            .spawn(move || {
                let mut stdout = BufReader::new(stdout);
                while reader.alive.load(Ordering::Acquire) {
                    let bytes = match frame(&mut stdout) {
                        Ok(Some(bytes)) => bytes,
                        Ok(None) => {
                            reader.shutdown("MCP server closed stdout");
                            break;
                        }
                        Err(error) => {
                            reader.shutdown(&error);
                            break;
                        }
                    };
                    let value: Value = match serde_json::from_slice(&bytes) {
                        Ok(value) => value,
                        Err(_) => {
                            reader.shutdown("MCP server wrote non-JSON data to stdout");
                            break;
                        }
                    };
                    if let Err(error) = route(&reader, value) {
                        reader.shutdown(&error);
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let errors = connection.clone();
        thread::Builder::new()
            .name("notcode-mcp-stderr".into())
            .spawn(move || {
                let mut buffer = [0u8; 4096];
                let mut total = 0usize;
                loop {
                    let count = match stderr.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    total += count;
                    if let Ok(mut tail) = errors.stderr.lock() {
                        tail.extend_from_slice(&buffer[..count]);
                        if tail.len() > STDERR_TAIL {
                            let remove = tail.len() - STDERR_TAIL;
                            tail.drain(..remove);
                        }
                    }
                    if total > STDERR_TOTAL {
                        errors.shutdown("MCP stderr exceeded the 1 MiB connection limit");
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(error) = setup {
        connection.shutdown("Cannot start MCP IO threads");
        // Also covers failure to start the first/reaper thread.
        if let Ok(mut child) = connection.child.lock() {
            let _ = child.wait();
        }
        return Err(error);
    }
    Ok(connection)
}

fn schema(db: &rusqlite::Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS mcp_servers (
        id TEXT PRIMARY KEY NOT NULL, project_path TEXT NOT NULL,
        name TEXT NOT NULL, command TEXT NOT NULL, args_json TEXT NOT NULL
    ); CREATE INDEX IF NOT EXISTS mcp_servers_project ON mcp_servers(project_path);",
    )
    .map_err(|e| e.to_string())
}
fn root_key(state: &AppState) -> Result<String, String> {
    crate::project::root(state)?
        .into_os_string()
        .into_string()
        .map_err(|_| "Project path is not UTF-8".into())
}
fn config(state: &AppState, id: &str, root: &str) -> Result<McpConfig, String> {
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    schema(&db)?;
    let row: Option<(String, String, String, String)> = db
        .query_row(
            "SELECT id,name,command,args_json FROM mcp_servers WHERE id=?1 AND project_path=?2",
            params![id, root],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let (id, name, command, args) = row.ok_or("MCP server was not found in this project")?;
    let args: Vec<String> =
        serde_json::from_str(&args).map_err(|e| format!("Invalid stored MCP arguments: {e}"))?;
    validate_config(&name, &command, &args)?;
    Ok(McpConfig {
        id,
        name,
        command,
        args,
        connected: false,
    })
}
fn connected(state: &AppState, id: &str) -> Result<Arc<Connection>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    crate::project::root(state)?;
    let connection = state
        .mcps
        .lock()
        .map_err(|_| "MCP lock poisoned")?
        .get(id)
        .map(|h| h.connection.clone())
        .ok_or("Connect this MCP server first")?;
    if !connection.alive.load(Ordering::Acquire) {
        return Err(connection.unavailable());
    }
    if !connection.ready.load(Ordering::Acquire) {
        return Err("MCP server is still initializing".into());
    }
    Ok(connection)
}

#[tauri::command]
pub fn mcp_list(state: State<'_, AppState>) -> Result<Vec<McpConfig>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let root = root_key(&state)?;
    let mut configs = Vec::new();
    {
        let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
        schema(&db)?;
        let mut query = db.prepare("SELECT id,name,command,args_json FROM mcp_servers WHERE project_path=?1 ORDER BY name,id LIMIT 100").map_err(|e| e.to_string())?;
        let mut rows = query.query([root]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let args: String = row.get(3).map_err(|e| e.to_string())?;
            configs.push(McpConfig {
                id: row.get(0).map_err(|e| e.to_string())?,
                name: row.get(1).map_err(|e| e.to_string())?,
                command: row.get(2).map_err(|e| e.to_string())?,
                args: serde_json::from_str(&args).map_err(|e| e.to_string())?,
                connected: false,
            });
        }
    }
    let handles = state.mcps.lock().map_err(|_| "MCP lock poisoned")?;
    for config in &mut configs {
        config.connected = handles
            .get(&config.id)
            .map(|h| {
                h.connection.alive.load(Ordering::Acquire)
                    && h.connection.ready.load(Ordering::Acquire)
            })
            .unwrap_or(false);
    }
    Ok(configs)
}

#[tauri::command]
pub fn mcp_add(
    name: String,
    command: String,
    args: Vec<String>,
    state: State<'_, AppState>,
) -> Result<McpConfig, String> {
    validate_config(&name, &command, &args)?;
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let root = root_key(&state)?;
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    schema(&db)?;
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM mcp_servers WHERE project_path=?1",
            [&root],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if count >= 100 {
        return Err("MCP configuration limit (100 per project) reached".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let args_json = serde_json::to_string(&args).map_err(|e| e.to_string())?;
    db.execute(
        "INSERT INTO mcp_servers(id,project_path,name,command,args_json) VALUES(?1,?2,?3,?4,?5)",
        params![id, root, name, command, args_json],
    )
    .map_err(|e| e.to_string())?;
    Ok(McpConfig {
        id,
        name,
        command,
        args,
        connected: false,
    })
}

#[tauri::command]
pub fn mcp_remove(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let root = root_key(&state)?;
    let handle = state
        .mcps
        .lock()
        .map_err(|_| "MCP lock poisoned")?
        .remove(&id);
    drop(handle);
    let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
    schema(&db)?;
    db.execute(
        "DELETE FROM mcp_servers WHERE id=?1 AND project_path=?2",
        params![id, root],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn mcp_connect_impl(id: String, approved: bool, state: &AppState) -> Result<(), String> {
    if !approved {
        return Err("Explicit consent is required to launch the configured local MCP executable and arguments".into());
    }
    let connection = {
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| "Project gate poisoned")?;
        let root = crate::project::root(state)?;
        let config = config(
            state,
            &id,
            root.to_str().ok_or("Project path is not UTF-8")?,
        )?;
        let mut handles = state.mcps.lock().map_err(|_| "MCP lock poisoned")?;
        if let Some(handle) = handles.get(&id) {
            if handle.connection.alive.load(Ordering::Acquire) {
                return Err("MCP server is already connected or initializing".into());
            }
        }
        handles.remove(&id);
        if handles.len() >= 8 {
            return Err("Disconnect an MCP server before connecting another (limit 8)".into());
        }
        let connection = spawn(&config, &root)?;
        handles.insert(
            id.clone(),
            McpHandle {
                connection: connection.clone(),
            },
        );
        connection
    }; // Gate and map guards released before waiting for any RPC.
    let initialized = (|| {
        let result = connection.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL, "capabilities": {},
                "clientInfo": {"name":"NotCode Desktop", "version":env!("CARGO_PKG_VERSION")}
            }),
            Duration::from_secs(20),
        )?;
        if result.get("protocolVersion").and_then(Value::as_str) != Some(PROTOCOL) {
            return Err("MCP server did not negotiate protocol 2024-11-05".to_owned());
        }
        if !result
            .get("capabilities")
            .map(Value::is_object)
            .unwrap_or(false)
        {
            return Err("MCP initialize response has no capabilities object".to_owned());
        }
        connection.notification("notifications/initialized")?;
        if !connection.alive.load(Ordering::Acquire) {
            return Err(connection.unavailable());
        }
        connection.ready.store(true, Ordering::Release);
        Ok(())
    })();
    if let Err(error) = initialized {
        connection.shutdown("MCP initialization failed");
        if let Ok(mut handles) = state.mcps.lock() {
            if handles
                .get(&id)
                .map(|h| Arc::ptr_eq(&h.connection, &connection))
                .unwrap_or(false)
            {
                handles.remove(&id);
            }
        }
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub fn mcp_disconnect(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let handle = state
        .mcps
        .lock()
        .map_err(|_| "MCP lock poisoned")?
        .remove(&id);
    drop(handle);
    Ok(())
}

fn tool(value: &Value) -> Result<McpTool, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or("MCP tool is missing its name")?;
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err("Invalid MCP tool name".into());
    }
    let input_schema = value
        .get("inputSchema")
        .filter(|v| v.is_object())
        .ok_or("MCP tool is missing its inputSchema object")?
        .clone();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(16384)
        .collect();
    Ok(McpTool {
        name: name.into(),
        description,
        input_schema,
    })
}

fn mcp_tools_impl(id: String, state: &AppState) -> Result<Vec<McpTool>, String> {
    let connection = connected(state, &id)?;
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let params = match &cursor {
            Some(c) => json!({"cursor":c}),
            None => json!({}),
        };
        let result = connection.request("tools/list", params, REQUEST_TIMEOUT)?;
        for value in result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or("MCP tools/list response is missing tools")?
        {
            if tools.len() >= 1000 {
                return Err("MCP server returned more than 1000 tools".into());
            }
            tools.push(tool(value)?);
        }
        match result.get("nextCursor") {
            None | Some(Value::Null) => return Ok(tools),
            Some(Value::String(next)) if next.len() <= 4096 && Some(next) != cursor.as_ref() => {
                cursor = Some(next.clone())
            }
            _ => return Err("Invalid or repeated MCP pagination cursor".into()),
        }
    }
    Err("MCP tools/list exceeded 10 pages".into())
}

/// Caller holds project_gate. Snapshot ready connections with their configured names;
/// the RPC waits themselves happen after the gate is released.
pub(crate) fn agent_servers(state: &AppState) -> Vec<(String, String, Arc<Connection>)> {
    let Ok(handles) = state.mcps.lock() else {
        return Vec::new();
    };
    let mut names = HashMap::new();
    if let Ok(db) = state.db.lock() {
        if let Ok(mut stmt) = db.prepare("SELECT id,name FROM mcp_servers") {
            if let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            {
                for (id, name) in rows.filter_map(Result::ok) {
                    names.insert(id, name);
                }
            }
        }
    }
    handles
        .iter()
        .filter(|(_, handle)| {
            handle.connection.alive.load(Ordering::Acquire)
                && handle.connection.ready.load(Ordering::Acquire)
        })
        .map(|(id, handle)| {
            (
                id.clone(),
                names.get(id).cloned().unwrap_or_else(|| "MCP".into()),
                handle.connection.clone(),
            )
        })
        .collect()
}

/// Caller holds project_gate.
pub(crate) fn connection_for(state: &AppState, id: &str) -> Result<Arc<Connection>, String> {
    let connection = state
        .mcps
        .lock()
        .map_err(|_| "MCP lock poisoned")?
        .get(id)
        .map(|h| h.connection.clone())
        .ok_or("This MCP server is not connected")?;
    if !connection.alive.load(Ordering::Acquire) {
        return Err(connection.unavailable());
    }
    if !connection.ready.load(Ordering::Acquire) {
        return Err("MCP server is still initializing".into());
    }
    Ok(connection)
}

/// One bounded page for agent tool discovery; a slow server is skipped, not retried.
pub(crate) fn request_tools(
    connection: &Connection,
    timeout: Duration,
) -> Result<Vec<McpTool>, String> {
    let result = connection.request("tools/list", json!({}), timeout)?;
    let mut tools = Vec::new();
    for value in result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or("MCP tools/list response is missing tools")?
    {
        if tools.len() >= 64 {
            break;
        }
        if let Ok(tool) = tool(value) {
            tools.push(tool);
        }
    }
    Ok(tools)
}

pub(crate) fn request_call(
    connection: &Connection,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    if !arguments.is_object() {
        return Err("MCP tool arguments must be a JSON object".into());
    }
    if serde_json::to_vec(&arguments)
        .map_err(|e| e.to_string())?
        .len()
        > MAX_FRAME - 4096
    {
        return Err("MCP tool arguments exceed the message limit".into());
    }
    connection.request(
        "tools/call",
        json!({"name":name,"arguments":arguments}),
        TOOL_TIMEOUT,
    )
}

fn mcp_call_impl(
    id: String,
    name: String,
    arguments: Value,
    approved: bool,
    state: &AppState,
) -> Result<Value, String> {
    if !approved {
        return Err("Confirm this specific MCP tool and its raw arguments before each call".into());
    }
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err("Invalid MCP tool name".into());
    }
    if !arguments.is_object() {
        return Err("MCP tool arguments must be a JSON object".into());
    }
    if serde_json::to_vec(&arguments)
        .map_err(|e| e.to_string())?
        .len()
        > MAX_FRAME - 4096
    {
        return Err("MCP tool arguments exceed the message limit".into());
    }
    let connection = connected(state, &id)?;
    // Do not flatten content or turn result.isError into a transport error: preserve
    // the server's complete MCP tool result for the explicit user-invoked call.
    connection.request(
        "tools/call",
        json!({"name":name,"arguments":arguments}),
        TOOL_TIMEOUT,
    )
}

// Blocking stdio and timeout waits never run on Tauri's command dispatcher.
async fn blocking<T, F>(app: tauri::AppHandle, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        operation(&state)
    })
    .await
    .map_err(|e| format!("MCP worker failed: {e}"))?
}
#[tauri::command]
pub async fn mcp_connect(id: String, approved: bool, app: tauri::AppHandle) -> Result<(), String> {
    blocking(app, move |state| mcp_connect_impl(id, approved, state)).await
}
#[tauri::command]
pub async fn mcp_tools(id: String, app: tauri::AppHandle) -> Result<Vec<McpTool>, String> {
    blocking(app, move |state| mcp_tools_impl(id, state)).await
}
#[tauri::command]
pub async fn mcp_call(
    id: String,
    name: String,
    arguments: Value,
    approved: bool,
    app: tauri::AppHandle,
) -> Result<Value, String> {
    blocking(app, move |state| {
        mcp_call_impl(id, name, arguments, approved, state)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_is_newline_delimited_and_bounded() {
        let mut input = std::io::Cursor::new(b"{\"jsonrpc\":\"2.0\"}\r\n{}\n");
        assert_eq!(
            frame(&mut input).unwrap().unwrap(),
            b"{\"jsonrpc\":\"2.0\"}"
        );
        assert_eq!(frame(&mut input).unwrap().unwrap(), b"{}");
        assert!(frame(&mut input).unwrap().is_none());
        assert!(frame(&mut std::io::Cursor::new(b"{}")).is_err());
        assert!(frame(&mut std::io::Cursor::new(vec![b'x'; MAX_FRAME + 2])).is_err());
    }
    #[test]
    fn correlates_numeric_ids_and_preserves_tool_errors() {
        let (id, reply) =
            response(&json!({"id":7,"result":{"isError":true,"content":[]}})).unwrap();
        assert_eq!(id, 7);
        assert_eq!(reply.unwrap()["isError"], true);
        assert!(
            response(&json!({"id":7,"error":{"code":-32601,"message":"unknown"}}))
                .unwrap()
                .1
                .is_err()
        );
        assert!(response(&json!({"id":7,"result":{},"error":{}})).is_err());
        assert!(response(&json!({"id":"7","result":{}})).is_err());
    }
    #[test]
    fn config_is_bounded_and_tools_require_real_schemas() {
        assert!(validate_config("server", "node", &["server.js".into()]).is_ok());
        assert!(validate_config("", "node", &[]).is_err());
        assert!(validate_config("server", "node\nother", &[]).is_err());
        assert!(validate_config("server", "node", &["\0".into()]).is_err());
        assert!(tool(&json!({"name":"read","inputSchema":{"type":"object"}})).is_ok());
        assert!(tool(&json!({"name":"read"})).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn real_stdio_roundtrip_handles_notifications_unknown_ids_and_tool_results() {
        let script = r#"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/message","params":{"data":"hello"}}'
      printf '%s\n' '{"jsonrpc":"2.0","id":999,"result":{}}'
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}}}}'
      ;;
    *'"method":"tools/list"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}'
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"isError":true,"content":[{"type":"text","text":"tool-level error"}]}}'
      ;;
  esac
done
"#;
        let config = McpConfig {
            id: "test".into(),
            name: "Test".into(),
            command: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            connected: false,
        };
        let connection = spawn(&config, &std::env::temp_dir()).unwrap();
        let result = connection
            .request(
                "initialize",
                json!({"protocolVersion":PROTOCOL,"capabilities":{}}),
                Duration::from_secs(3),
            )
            .unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL);
        connection
            .notification("notifications/initialized")
            .unwrap();
        let result = connection
            .request("tools/list", json!({}), Duration::from_secs(3))
            .unwrap();
        assert_eq!(tool(&result["tools"][0]).unwrap().name, "echo");
        let result = connection
            .request(
                "tools/call",
                json!({"name":"echo","arguments":{"text":"a\nb"}}),
                Duration::from_secs(3),
            )
            .unwrap();
        assert_eq!(result["isError"], true);
        let handle = McpHandle {
            connection: connection.clone(),
        };
        drop(handle);
        assert!(!connection.alive.load(Ordering::Acquire));
        let start = std::time::Instant::now();
        loop {
            if connection
                .child
                .lock()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_some()
            {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "MCP child was not terminated on handle drop"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(unix)]
    #[test]
    fn timeout_terminates_unresponsive_server_and_stderr_is_bounded() {
        let config = McpConfig {
            id: "test".into(),
            name: "Test".into(),
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'diagnostic\\n' >&2; while IFS= read -r line; do :; done".into(),
            ],
            connected: false,
        };
        let connection = spawn(&config, &std::env::temp_dir()).unwrap();
        assert!(connection
            .request("initialize", json!({}), Duration::from_millis(150))
            .is_err());
        assert!(!connection.alive.load(Ordering::Acquire));
        assert!(connection.stderr.lock().unwrap().len() <= STDERR_TAIL);
        assert!(connection.pending.lock().unwrap().is_empty());
    }
    #[test]
    fn sqlite_configs_are_project_scoped_and_have_no_environment_column() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        schema(&db).unwrap();
        schema(&db).unwrap();
        db.execute(
            "INSERT INTO mcp_servers VALUES(?1,?2,?3,?4,?5)",
            params!["server", "/project-a", "Example", "node", "[]"],
        )
        .unwrap();
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM mcp_servers WHERE project_path=?1",
                ["/project-b"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        let mut stmt = db.prepare("PRAGMA table_info(mcp_servers)").unwrap();
        let columns = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            columns,
            ["id", "project_path", "name", "command", "args_json"]
        );
    }
}
