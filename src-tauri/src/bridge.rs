//! Explicit, project-bound TLS sharing. No listener or credential is created at startup.
use crate::{
    agent, project,
    state::{AgentApproval, AppState, RemoteApprovalOrigin},
    store,
};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State as HttpState},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{oneshot, Notify};

const MAX_BODY: usize = project::MAX_FILE_BYTES + 8192;
const MAX_EVENTS: usize = 512;
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENT: usize = 64 * 1024;
const MAX_POLL_BYTES: usize = 512 * 1024;
const MAX_PREVIEW: usize = 4 * 1024 * 1024;
const INVITE_TTL: Duration = Duration::from_secs(300);
const APPROVAL_TTL: Duration = Duration::from_secs(60);
const MAX_DEVICES: usize = 64;

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    pub running: bool,
    pub address: String,
    pub fingerprint: String,
    pub invitation: String,
    pub project_id: String,
    pub project_name: String,
    pub devices: Vec<Device>,
    pub preview_ports: Vec<u16>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub created_at: i64,
}
struct Credential {
    device: Device,
    hash: [u8; 32],
}
struct Invitation {
    token: String,
    hash: [u8; 32],
    expires: Instant,
    epoch: u64,
    used: bool,
}
struct PairPending {
    epoch: u64,
    sender: Option<oneshot::Sender<bool>>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Approval {
    request_id: String,
    session_id: String,
    name: String,
    arguments: Value,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct TurnOwner {
    device_id: String,
    message_id: String,
}
struct PendingAction {
    origin: RemoteApprovalOrigin,
    owner: String,
    approval: Approval,
    expires: Instant,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    seq: u64,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    data: Value,
}
struct StoredEvent {
    event: Event,
    owner: Option<String>,
    bytes: usize,
}
#[derive(Default)]
struct Replay {
    seq: u64,
    bytes: usize,
    events: VecDeque<StoredEvent>,
}
impl Replay {
    fn push(&mut self, kind: &str, session_id: Option<&str>, data: Value, owner: Option<String>) {
        let bytes = data.to_string().len() + 256;
        if bytes > MAX_EVENT {
            return;
        }
        self.seq = self.seq.saturating_add(1);
        self.bytes += bytes;
        self.events.push_back(StoredEvent {
            event: Event {
                seq: self.seq,
                kind: kind.into(),
                session_id: session_id.map(str::to_owned),
                data,
            },
            owner,
            bytes,
        });
        while self.events.len() > MAX_EVENTS || self.bytes > MAX_EVENT_BYTES {
            if let Some(old) = self.events.pop_front() {
                self.bytes -= old.bytes;
            }
        }
    }
    fn poll(&self, after: u64, owner: &str) -> Value {
        let reset = after > self.seq
            || self
                .events
                .front()
                .is_some_and(|e| after.saturating_add(1) < e.event.seq);
        let start = if reset { 0 } else { after };
        let mut cursor = start;
        let mut bytes = 0;
        let mut events = Vec::new();
        for stored in self.events.iter().filter(|e| e.event.seq > start) {
            if stored.owner.as_deref().is_none_or(|id| id == owner) {
                if bytes + stored.bytes > MAX_POLL_BYTES {
                    break;
                }
                bytes += stored.bytes;
                events.push(stored.event.clone());
            }
            cursor = stored.event.seq;
        }
        if cursor == start {
            cursor = self.seq;
        }
        json!({"cursor":cursor,"reset":reset,"events":events})
    }
}
struct Rate {
    start: Instant,
    count: u32,
}
impl Rate {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            count: 0,
        }
    }
    fn allow(&mut self, max: u32) -> bool {
        if self.start.elapsed() >= Duration::from_secs(60) {
            self.start = Instant::now();
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        self.count <= max
    }
}
struct Inner {
    running: bool,
    invitation: Invitation,
    devices: HashMap<String, Credential>,
    pairs: HashMap<String, PairPending>,
    owners: HashMap<String, TurnOwner>,
    approvals: HashMap<String, PendingAction>,
    preview_ports: HashSet<u16>,
    rates: HashMap<String, Rate>,
    pair_rate: Rate,
    polls: HashSet<String>,
    replay: Replay,
}
/// AppState owns this handle even after stopping, until a subsequent explicit start.
/// Ownership tombstones keep canceled remote jobs from falling back to a PC approval.
#[derive(Clone)]
pub struct BridgeHandle {
    pub runtime: Arc<BridgeRuntime>,
}
pub struct BridgeRuntime {
    id: String,
    app: AppHandle,
    project_id: String,
    project_name: String,
    root: String,
    address: String,
    fingerprint: String,
    server: axum_server::Handle<SocketAddr>,
    inner: Mutex<Inner>,
    notify: Notify,
    models: tokio::sync::Mutex<Option<(Instant, Value)>>,
    preview: reqwest::Client,
    requests: tokio::sync::Semaphore,
    pairing_requests: tokio::sync::Semaphore,
}
#[derive(Clone)]
pub(crate) struct RemoteTurn {
    pub runtime: Arc<BridgeRuntime>,
    pub device_id: String,
    pub message_id: String,
    pub digest: String,
}
impl RemoteTurn {
    pub(crate) fn validate_locked(&self) -> Result<(), String> {
        self.runtime
            .authorized_locked(&self.device_id)
            .map_err(|e| e.native())
    }
    pub(crate) fn reserve_locked(&self, session: &str) -> Result<(), String> {
        let mut inner = self
            .runtime
            .inner
            .lock()
            .map_err(|_| "Bridge unavailable")?;
        if !inner.running || !inner.devices.contains_key(&self.device_id) {
            return Err("Bridge authorization expired".into());
        }
        inner.owners.insert(session.into(), self.owner());
        Ok(())
    }
    fn owner(&self) -> TurnOwner {
        TurnOwner {
            device_id: self.device_id.clone(),
            message_id: self.message_id.clone(),
        }
    }
    fn approval_origin(&self, session: &str) -> RemoteApprovalOrigin {
        RemoteApprovalOrigin {
            runtime_id: self.runtime.id.clone(),
            project_id: self.runtime.project_id.clone(),
            session_id: session.into(),
            device_id: self.device_id.clone(),
            message_id: self.message_id.clone(),
        }
    }
    /// Caller retains project_gate through the operation, fencing stop/revoke.
    pub(crate) fn validate_active_locked(&self, session: &str) -> Result<(), String> {
        self.validate_locked()?;
        let inner = self
            .runtime
            .inner
            .lock()
            .map_err(|_| "Bridge unavailable")?;
        turn_allowed(&inner, session, &self.owner())
    }
}

fn turn_allowed(inner: &Inner, session: &str, owner: &TurnOwner) -> Result<(), String> {
    if !inner.running || !inner.devices.contains_key(&owner.device_id) {
        return Err("Bridge authorization expired".into());
    }
    if inner.owners.get(session) != Some(owner) {
        return Err("Generation belongs to another client".into());
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
/// Both operands are fixed-length digests; no prefix/early-exit comparison.
fn constant_time(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    std::hint::black_box(diff) == 0
}
fn random_token(prefix: &str) -> String {
    format!(
        "{prefix}{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn invitation(epoch: u64) -> Invitation {
    let token = random_token("nci_");
    Invitation {
        hash: digest(token.as_bytes()),
        token,
        expires: Instant::now() + INVITE_TTL,
        epoch,
        used: false,
    }
}
fn consume_invite(invite: &mut Invitation, token: &str) -> Result<u64, ApiError> {
    if invite.used
        || Instant::now() >= invite.expires
        || !constant_time(&invite.hash, &digest(token.as_bytes()))
    {
        return Err(ApiError::unauthorized());
    }
    invite.used = true;
    Ok(invite.epoch)
}
fn private_ip(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback()
}
#[tauri::command]
pub fn bridge_addresses() -> Result<Vec<String>, String> {
    let mut addresses: Vec<String> = if_addrs::get_if_addrs()
        .map_err(|_| "Cannot enumerate network interfaces")?
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(ip) if private_ip(ip) => Some(ip.to_string()),
            _ => None,
        })
        .collect();
    addresses.sort();
    addresses.dedup();
    Ok(addresses)
}
fn selected_ip(address: &str) -> Result<Ipv4Addr, String> {
    let ip: Ipv4Addr = address
        .parse()
        .map_err(|_| "Select a numeric private or loopback IPv4 interface")?;
    if !private_ip(ip) || !bridge_addresses()?.contains(&ip.to_string()) {
        return Err("Select a local private or loopback IPv4 interface".into());
    }
    Ok(ip)
}

#[derive(Serialize, Deserialize)]
struct TlsMaterial {
    cert: String,
    key: String,
}
fn decode_material(raw: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    if raw.len() > 8192 {
        return Err("Invalid saved bridge certificate".into());
    }
    let material: TlsMaterial =
        serde_json::from_str(raw).map_err(|_| "Invalid saved bridge certificate")?;
    let cert = STANDARD
        .decode(material.cert)
        .map_err(|_| "Invalid saved bridge certificate")?;
    let key = STANDARD
        .decode(material.key)
        .map_err(|_| "Invalid saved bridge key")?;
    if cert.is_empty() || key.is_empty() {
        return Err("Invalid saved bridge certificate".into());
    }
    Ok((cert, key))
}
fn new_material(addresses: Vec<String>) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut sans = addresses;
    sans.push("localhost".into());
    let certified = rcgen::generate_simple_self_signed(sans)
        .map_err(|_| "Cannot generate bridge certificate")?;
    Ok((
        certified.cert.der().to_vec(),
        certified.signing_key.serialize_der(),
    ))
}
/// Called ONLY from bridge_start, following an explicit native UI action.
fn tls_material() -> Result<(Vec<u8>, Vec<u8>), String> {
    #[cfg(windows)]
    {
        static LOCK: Mutex<()> = Mutex::new(());
        let _lock = LOCK.lock().map_err(|_| "Credential lock unavailable")?;
        let entry = keyring::Entry::new("dev.notcode.desktop.bridge", "tls-v1")
            .map_err(|_| "Credential Manager unavailable")?;
        match entry.get_password() {
            Ok(raw) => decode_material(&raw),
            Err(keyring::Error::NoEntry) => {
                let (cert, key) = new_material(bridge_addresses()?)?;
                let material = serde_json::to_string(&TlsMaterial {
                    cert: STANDARD.encode(&cert),
                    key: STANDARD.encode(&key),
                })
                .map_err(|_| "Cannot encode bridge certificate")?;
                entry
                    .set_password(&material)
                    .map_err(|_| "Cannot persist bridge certificate in Credential Manager")?;
                Ok((cert, key))
            }
            Err(_) => Err("Cannot read bridge certificate from Credential Manager".into()),
        }
    }
    #[cfg(not(windows))]
    {
        Err("Persistent bridge TLS requires Windows Credential Manager".into())
    }
}
fn initialize_bridge(db: &rusqlite::Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS bridge_devices (
        id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash BLOB NOT NULL,
        project_id TEXT NOT NULL REFERENCES projects(id), created_at INTEGER NOT NULL
    ); CREATE INDEX IF NOT EXISTS bridge_devices_project ON bridge_devices(project_id);
    CREATE TABLE IF NOT EXISTS bridge_turns (
        message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
        device_id TEXT NOT NULL, project_id TEXT NOT NULL, session_id TEXT NOT NULL,
        request_hash TEXT NOT NULL
    );",
    )
    .map_err(|_| "Cannot initialize bridge database".to_owned())
}
fn load_devices(
    db: &rusqlite::Connection,
    pid: &str,
) -> Result<HashMap<String, Credential>, String> {
    let mut stmt = db
        .prepare(
            "SELECT id,name,created_at,token_hash FROM bridge_devices WHERE project_id=?1 LIMIT 65",
        )
        .map_err(|_| "Cannot load devices")?;
    let rows = stmt
        .query_map([pid], |r| {
            Ok((
                Device {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get(2)?,
                },
                r.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(|_| "Cannot load devices")?;
    let mut devices = HashMap::new();
    for row in rows {
        let (device, bytes) = row.map_err(|_| "Cannot load devices")?;
        let hash: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "Invalid stored device credential")?;
        devices.insert(device.id.clone(), Credential { device, hash });
    }
    if devices.len() > MAX_DEVICES {
        return Err("Too many paired devices".into());
    }
    Ok(devices)
}
fn handle(state: &AppState) -> Result<Arc<BridgeRuntime>, String> {
    state
        .bridge
        .lock()
        .map_err(|_| "Bridge lock unavailable")?
        .as_ref()
        .map(|h| h.runtime.clone())
        .ok_or_else(|| "Bridge is off".into())
}
fn safe_name(name: &str) -> String {
    if name.contains(['/', '\\', ':']) {
        "Project".into()
    } else {
        name.chars().filter(|c| !c.is_control()).take(200).collect()
    }
}
#[tauri::command]
pub async fn bridge_start(
    address: String,
    port: u16,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<BridgeStatus, String> {
    let ip = selected_ip(&address)?;
    let captured = {
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| "Project gate unavailable")?;
        let project = state
            .project
            .lock()
            .map_err(|_| "Project unavailable")?
            .clone()
            .ok_or("Open a project before sharing")?;
        if state
            .bridge
            .lock()
            .map_err(|_| "Bridge unavailable")?
            .as_ref()
            .is_some_and(|h| h.runtime.is_running())
        {
            return Err("Bridge is already running".into());
        }
        if !state
            .generations
            .lock()
            .map_err(|_| "Generations unavailable")?
            .is_empty()
        {
            return Err("Wait for active generations before starting sharing".into());
        }
        project
    };
    let (cert, key) = tls_material()?;
    let fingerprint = project::text_hash(&cert);
    let tls = axum_server::tls_rustls::RustlsConfig::from_der(vec![cert], key)
        .await
        .map_err(|_| "Invalid bridge TLS certificate")?;
    let listener = TcpListener::bind(SocketAddr::from((ip, port)))
        .map_err(|_| "Cannot bind selected bridge interface and port")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "Cannot configure bridge listener")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Cannot read bridge port")?
        .port();
    let preview = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Cannot create preview client")?;
    let server = axum_server::Handle::new();
    let runtime = {
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| "Project gate unavailable")?;
        project::expect_locked(&state, &captured.id)?;
        let mut slot = state.bridge.lock().map_err(|_| "Bridge unavailable")?;
        if slot.as_ref().is_some_and(|h| h.runtime.is_running()) {
            return Err("Bridge is already running".into());
        }
        if !state
            .generations
            .lock()
            .map_err(|_| "Generations unavailable")?
            .is_empty()
        {
            return Err("Wait for active generations before starting sharing".into());
        }
        let devices = {
            let db = state.db.lock().map_err(|_| "Database unavailable")?;
            initialize_bridge(&db)?;
            load_devices(&db, &captured.id)?
        };
        let runtime = Arc::new(BridgeRuntime {
            id: uuid::Uuid::new_v4().to_string(),
            app: app.clone(),
            project_id: captured.id,
            project_name: safe_name(&captured.name),
            root: captured.path,
            address: format!("https://{ip}:{port}"),
            fingerprint,
            server: server.clone(),
            inner: Mutex::new(Inner {
                running: true,
                invitation: invitation(1),
                devices,
                pairs: HashMap::new(),
                owners: HashMap::new(),
                approvals: HashMap::new(),
                preview_ports: HashSet::new(),
                rates: HashMap::new(),
                pair_rate: Rate::new(),
                polls: HashSet::new(),
                replay: Replay::default(),
            }),
            notify: Notify::new(),
            models: tokio::sync::Mutex::new(None),
            preview,
            requests: tokio::sync::Semaphore::new(96),
            pairing_requests: tokio::sync::Semaphore::new(4),
        });
        *slot = Some(BridgeHandle {
            runtime: runtime.clone(),
        });
        runtime
    };
    let router = router(runtime.clone());
    let service =
        axum_server::from_tcp_rustls(listener, tls).map_err(|_| "Cannot configure TLS listener");
    let service = match service {
        Ok(s) => s,
        Err(e) => {
            runtime.stop();
            return Err(e.to_owned());
        }
    };
    let task_runtime = runtime.clone();
    tauri::async_runtime::spawn(async move {
        let _ = service
            .handle(server)
            .serve(router.into_make_service())
            .await;
        task_runtime.stop();
    });
    Ok(runtime.status())
}
#[tauri::command]
pub fn bridge_status(state: State<'_, AppState>) -> Result<BridgeStatus, String> {
    match state
        .bridge
        .lock()
        .map_err(|_| "Bridge unavailable")?
        .as_ref()
    {
        Some(h) => Ok(h.runtime.status()),
        None => Ok(BridgeStatus::default()),
    }
}
#[tauri::command]
pub fn bridge_stop(state: State<'_, AppState>) -> Result<(), String> {
    let runtime = state
        .bridge
        .lock()
        .map_err(|_| "Bridge unavailable")?
        .as_ref()
        .map(|h| h.runtime.clone());
    if let Some(runtime) = runtime {
        runtime.stop();
    }
    Ok(())
}
#[tauri::command]
pub fn bridge_invite(state: State<'_, AppState>) -> Result<BridgeStatus, String> {
    let runtime = handle(&state)?;
    project::with_expected(&state, &runtime.project_id, || {
        let mut inner = runtime.inner.lock().map_err(|_| "Bridge unavailable")?;
        if !inner.running {
            return Err("Bridge is off".into());
        }
        let epoch = inner.invitation.epoch.saturating_add(1);
        for (_, mut pair) in inner.pairs.drain() {
            if let Some(sender) = pair.sender.take() {
                let _ = sender.send(false);
            }
        }
        inner.invitation = invitation(epoch);
        Ok(())
    })?;
    Ok(runtime.status())
}
#[tauri::command]
pub fn bridge_pair_respond(
    request_id: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let runtime = handle(&state)?;
    project::with_expected(&state, &runtime.project_id, || {
        let mut inner = runtime.inner.lock().map_err(|_| "Bridge unavailable")?;
        if !inner.running {
            return Err("Bridge is off".into());
        }
        let epoch = inner.invitation.epoch;
        let pair = inner
            .pairs
            .get_mut(&request_id)
            .ok_or("Pair request expired")?;
        if pair.epoch != epoch {
            return Err("Pair request expired".into());
        }
        pair.sender
            .take()
            .ok_or("Pair request already answered")?
            .send(approved)
            .map_err(|_| "Pair request expired".into())
    })
}
#[tauri::command]
pub fn bridge_revoke(device_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let runtime = handle(&state)?;
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate unavailable")?;
    // Revocation/stop are always available, including after a project switch.
    let mut inner = runtime.inner.lock().map_err(|_| "Bridge unavailable")?;
    state
        .db
        .lock()
        .map_err(|_| "Database unavailable")?
        .execute(
            "DELETE FROM bridge_devices WHERE id=?1 AND project_id=?2",
            params![device_id, runtime.project_id],
        )
        .map_err(|_| "Cannot revoke device")?;
    inner.devices.remove(&device_id);
    inner.rates.remove(&device_id);
    runtime.cancel_locked(&state, &mut inner, Some(&device_id));
    runtime.notify.notify_waiters();
    Ok(())
}
#[tauri::command]
pub fn bridge_allow_preview(
    port: u16,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let runtime = handle(&state)?;
    if port == 0 || runtime.address.ends_with(&format!(":{port}")) {
        return Err("Invalid preview port".into());
    }
    project::with_expected(&state, &runtime.project_id, || {
        let mut inner = runtime.inner.lock().map_err(|_| "Bridge unavailable")?;
        if !inner.running {
            return Err("Bridge is off".into());
        }
        if approved {
            if inner.preview_ports.len() >= 16 && !inner.preview_ports.contains(&port) {
                return Err("At most 16 preview ports may be allowed".into());
            }
            inner.preview_ports.insert(port);
        } else {
            inner.preview_ports.remove(&port);
        }
        Ok(())
    })
}

impl BridgeRuntime {
    fn is_running(&self) -> bool {
        self.inner.lock().map(|i| i.running).unwrap_or(false)
    }
    fn status(&self) -> BridgeStatus {
        let Ok(inner) = self.inner.lock() else {
            return BridgeStatus::default();
        };
        let mut devices: Vec<Device> = inner.devices.values().map(|c| c.device.clone()).collect();
        devices.sort_by(|a, b| a.id.cmp(&b.id));
        let mut ports: Vec<u16> = inner.preview_ports.iter().copied().collect();
        ports.sort();
        let invitation = if inner.running
            && !inner.invitation.used
            && Instant::now() < inner.invitation.expires
        {
            json!({"version":1,"address":self.address,"fingerprint":self.fingerprint,"token":inner.invitation.token}).to_string()
        } else {
            String::new()
        };
        BridgeStatus {
            running: inner.running,
            address: self.address.clone(),
            fingerprint: self.fingerprint.clone(),
            invitation,
            project_id: self.project_id.clone(),
            project_name: self.project_name.clone(),
            devices,
            preview_ports: ports,
        }
    }
    fn cancel_locked(&self, state: &AppState, inner: &mut Inner, device: Option<&str>) {
        if let Ok(active) = state.generations.lock() {
            for (session, owner) in &inner.owners {
                if device.is_none_or(|id| id == owner.device_id) {
                    if let Some(sender) = active.get(session) {
                        let _ = sender.send(true);
                    }
                }
            }
        }
        let ids: Vec<String> = inner
            .approvals
            .iter()
            .filter(|(_, a)| device.is_none_or(|id| id == a.owner))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            close_action(state, inner, &id, false);
        }
        // Keep owners until each final agent event: never expose a canceled job to PC auto-routing.
    }
    pub fn stop(&self) {
        let state = self.app.state::<AppState>();
        let Ok(_gate) = state.project_gate.lock() else {
            self.server.shutdown();
            return;
        };
        let Ok(mut inner) = self.inner.lock() else {
            self.server.shutdown();
            return;
        };
        inner.running = false;
        inner.invitation.used = true;
        for (_, mut pair) in inner.pairs.drain() {
            if let Some(tx) = pair.sender.take() {
                let _ = tx.send(false);
            }
        }
        inner.preview_ports.clear();
        self.cancel_locked(&state, &mut inner, None);
        self.server.shutdown();
        self.notify.notify_waiters();
    }
    fn authorized_locked(&self, device: &str) -> Result<(), ApiError> {
        let state = self.app.state::<AppState>();
        project::expect_locked(&state, &self.project_id).map_err(ApiError::from)?;
        let inner = self.inner.lock().map_err(|_| ApiError::internal())?;
        if !inner.running {
            return Err(ApiError::conflict());
        }
        if !inner.devices.contains_key(device) {
            return Err(ApiError::unauthorized());
        }
        Ok(())
    }
    fn work<T>(
        &self,
        device: &str,
        work: impl FnOnce(&AppState) -> Result<T, String>,
    ) -> Result<T, ApiError> {
        let state = self.app.state::<AppState>();
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| ApiError::internal())?;
        self.authorized_locked(device)?;
        work(&state).map_err(ApiError::from)
    }
    fn admit_pair(&self) -> Result<(), ApiError> {
        let state = self.app.state::<AppState>();
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| ApiError::internal())?;
        project::expect_locked(&state, &self.project_id).map_err(ApiError::from)?;
        let mut inner = self.inner.lock().map_err(|_| ApiError::internal())?;
        if !inner.running {
            return Err(ApiError::conflict());
        }
        if !inner.pair_rate.allow(10) {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "Pairing rate limit exceeded",
            ));
        }
        Ok(())
    }
    fn authenticate(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        refuse_browser(headers)?;
        let token = bearer(headers)?;
        let hash = digest(token.as_bytes());
        let state = self.app.state::<AppState>();
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| ApiError::internal())?;
        project::expect_locked(&state, &self.project_id).map_err(ApiError::from)?;
        let mut inner = self.inner.lock().map_err(|_| ApiError::internal())?;
        if !inner.running {
            return Err(ApiError::conflict());
        }
        let mut found = None;
        for (id, credential) in &inner.devices {
            if constant_time(&hash, &credential.hash) {
                found = Some(id.clone());
            }
        }
        let id = found.ok_or_else(ApiError::unauthorized)?;
        if !inner
            .rates
            .entry(id.clone())
            .or_insert_with(Rate::new)
            // Bulk copy/sync issues several requests per file; 240/min stalled
            // projects over ~150 files mid-transfer with HTTP 429.
            .allow(1200)
        {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "Rate limit exceeded",
            ));
        }
        Ok(id)
    }
    fn clean(&self, mut value: Value) -> Value {
        fn walk(v: &mut Value, root: &str) {
            match v {
                Value::String(s) => {
                    *s = s
                        .replace(root, "[project]")
                        .replace(&root.replace('\\', "/"), "[project]");
                    // CLI diagnostics/actions sometimes contain unrelated absolute paths.
                    let mut out = String::new();
                    for (index, part) in s.split_inclusive(char::is_whitespace).enumerate() {
                        let trimmed = part.trim_start_matches(['\"', '\'', '(', '[']);
                        let drive = trimmed.as_bytes().get(1) == Some(&b':')
                            && trimmed
                                .as_bytes()
                                .get(2)
                                .is_some_and(|b| *b == b'/' || *b == b'\\');
                        let absolute = drive
                            || trimmed.starts_with("\\\\")
                            || trimmed.starts_with("/Users/")
                            || trimmed.starts_with("/home/")
                            || trimmed.starts_with("/tmp/")
                            || trimmed.starts_with("/var/");
                        if absolute {
                            out.push_str("[path]");
                            if part.ends_with(char::is_whitespace) {
                                out.push(' ');
                            }
                        } else {
                            out.push_str(part);
                        }
                        if index > 131_072 {
                            break;
                        }
                    }
                    *s = out;
                }
                Value::Array(a) => {
                    for v in a {
                        walk(v, root);
                    }
                }
                Value::Object(o) => {
                    for v in o.values_mut() {
                        walk(v, root);
                    }
                }
                _ => {}
            }
        }
        walk(&mut value, &self.root);
        value
    }
}
fn close_action(state: &AppState, inner: &mut Inner, id: &str, approved: bool) {
    if let Some(pending) = inner.approvals.remove(id) {
        {
            // A poisoned map may be drained for denial, never recovered for approval.
            let (mut senders, map_healthy) = match state.agent_approvals.lock() {
                Ok(map) => (map, true),
                Err(poison) => (poison.into_inner(), false),
            };
            if senders
                .get(id)
                .is_some_and(|entry| entry.remote.as_ref() == Some(&pending.origin))
            {
                if let Some(entry) = senders.remove(id) {
                    let _ = entry.sender.send(approved && map_healthy);
                }
            }
        }
        inner.replay.push(
            "approval-closed",
            Some(&pending.approval.session_id),
            json!({"requestId":id}),
            Some(pending.owner),
        );
    }
}

#[derive(Debug)]
struct ApiError(StatusCode, &'static str);
impl ApiError {
    fn unauthorized() -> Self {
        Self(StatusCode::UNAUTHORIZED, "Unauthorized")
    }
    fn conflict() -> Self {
        Self(StatusCode::CONFLICT, "Sharing state changed")
    }
    fn bad() -> Self {
        Self(StatusCode::BAD_REQUEST, "Invalid request")
    }
    fn internal() -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, "Bridge operation failed")
    }
    fn native(&self) -> String {
        self.1.into()
    }
}
impl From<String> for ApiError {
    fn from(s: String) -> Self {
        if s == agent::REMOTE_SANDBOX_LIMITATION {
            Self(
                StatusCode::NOT_IMPLEMENTED,
                agent::REMOTE_SANDBOX_LIMITATION,
            )
        } else if s == "Bridge project changed"
            || s == "Stale file hash"
            || s.contains("already generating")
            || s == "Request ID was reused with different input"
            || s == "Previous generation is finishing"
        {
            Self::conflict()
        } else if s == "Bridge authorization expired" {
            Self::unauthorized()
        } else if s == "Approval belongs to another client"
            || s == "Generation belongs to another client"
            || s == "Preview port is not allowed"
        {
            Self(StatusCode::FORBIDDEN, "Operation is not owned or allowed")
        } else if s == "Approval expired"
            || s == "Only one event poll per device is allowed"
            || s.contains("concurrent generations")
        {
            Self::conflict()
        } else if s == "Session not found in this project" {
            Self(StatusCode::NOT_FOUND, "Session not found")
        } else {
            Self::bad()
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
fn refuse_browser(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::ORIGIN)
        || headers.keys().any(|k| k.as_str().starts_with("sec-fetch-"))
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Browser requests are disabled",
        ));
    }
    if headers.len() > 64
        || headers
            .iter()
            .map(|(k, v)| k.as_str().len() + v.as_bytes().len())
            .sum::<usize>()
            > 16 * 1024
    {
        return Err(ApiError::bad());
    }
    Ok(())
}
fn bearer(headers: &HeaderMap) -> Result<&str, ApiError> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(ApiError::unauthorized());
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    if !token.starts_with("ncb_")
        || token.len() != 68
        || !token[4..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ApiError::unauthorized());
    }
    Ok(token)
}
fn router(runtime: Arc<BridgeRuntime>) -> Router {
    Router::new()
        .route("/v1/pair", any(api))
        .route("/v1/status", any(api))
        .route("/v1/models", any(api))
        .route("/v1/sessions", any(api))
        .route("/v1/sessions/{id}", any(api))
        .route("/v1/sessions/{id}/send", any(api))
        .route("/v1/sessions/{id}/cancel", any(api))
        .route("/v1/events", any(api))
        .route("/v1/approvals", any(api))
        .route("/v1/approvals/{requestId}", any(api))
        .route("/v1/files", any(api))
        .route("/v1/file", any(api))
        .route("/v1/snapshot", any(api))
        .route("/v1/preview/{port}", any(api))
        .route("/v1/preview/{port}/", any(api))
        .route("/v1/preview/{port}/{*path}", any(api))
        .fallback(api)
        .with_state(runtime)
}
fn request_pool<'a>(
    device: Option<&str>,
    authenticated: &'a tokio::sync::Semaphore,
    pairing: &'a tokio::sync::Semaphore,
) -> &'a tokio::sync::Semaphore {
    if device.is_some() {
        authenticated
    } else {
        pairing
    }
}
async fn api(HttpState(runtime): HttpState<Arc<BridgeRuntime>>, request: Request) -> Response {
    let result = async {
        // Admission never polls the body. Unauthenticated pairing has a separate,
        // small pool; incomplete pair bodies cannot reserve service slots.
        let device = admit_request(
            &request,
            || runtime.admit_pair(),
            || runtime.authenticate(request.headers()),
        )?;
        let pool = request_pool(
            device.as_deref(),
            &runtime.requests,
            &runtime.pairing_requests,
        );
        let _permit = pool
            .try_acquire()
            .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "Bridge is busy"))?;
        dispatch(runtime.clone(), request, device).await
    }
    .await;
    let mut response = match result {
        Ok(r) => r,
        Err(e) => e.into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        header::HeaderValue::from_static("nosniff"),
    );
    response
}
fn require_json(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.get_all(header::CONTENT_TYPE).iter().count() != 1 {
        return Err(ApiError::bad());
    }
    if !headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.split(';').next() == Some("application/json"))
    {
        return Err(ApiError::bad());
    }
    Ok(())
}
async fn input<T: serde::de::DeserializeOwned>(request: Request) -> Result<T, ApiError> {
    require_json(request.headers())?;
    let bytes = tokio::time::timeout(
        Duration::from_secs(10),
        to_bytes(request.into_body(), MAX_BODY),
    )
    .await
    .map_err(|_| ApiError::bad())?
    .map_err(|_| ApiError::bad())?;
    serde_json::from_slice(&bytes).map_err(|_| ApiError::bad())
}
fn json_response<T: Serialize>(runtime: &BridgeRuntime, value: T) -> Result<Response, ApiError> {
    let value = serde_json::to_value(value).map_err(|_| ApiError::internal())?;
    let bytes = serde_json::to_vec(&runtime.clean(value)).map_err(|_| ApiError::internal())?;
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Response exceeds bridge limit",
        ));
    }
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    Ok(response)
}
fn query(request: &Request, key: &str) -> Result<Option<String>, ApiError> {
    let raw = request.uri().query().unwrap_or("");
    if raw.len() > 8192 {
        return Err(ApiError::bad());
    }
    let url =
        reqwest::Url::parse(&format!("http://127.0.0.1/?{raw}")).map_err(|_| ApiError::bad())?;
    let mut values = url
        .query_pairs()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned());
    let first = values.next();
    if values.next().is_some() {
        return Err(ApiError::bad());
    }
    Ok(first)
}
fn valid_id(id: &str) -> Result<(), ApiError> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| ApiError::bad())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairInput {
    token: String,
    device_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionInput {
    provider: String,
    model: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendInput {
    content: String,
    context_path: Option<String>,
    request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    approved: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileInput {
    path: String,
    content: String,
    expected_hash: Option<String>,
}

fn admit_request(
    request: &Request,
    pair_rate: impl FnOnce() -> Result<(), ApiError>,
    authenticate: impl FnOnce() -> Result<String, ApiError>,
) -> Result<Option<String>, ApiError> {
    refuse_browser(request.headers())?;
    if request.uri().to_string().len() > 12 * 1024 {
        return Err(ApiError::bad());
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .is_some_and(|v| {
            v.to_str()
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .is_none_or(|n| n > MAX_BODY)
        })
    {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "Request too large"));
    }
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    if path == "/v1/pair" {
        if method != Method::POST {
            return Err(ApiError(
                StatusCode::METHOD_NOT_ALLOWED,
                "Method not allowed",
            ));
        }
        require_json(request.headers())?;
        pair_rate()?;
        return Ok(None);
    }
    authenticate().map(Some)
}

async fn dispatch(
    runtime: Arc<BridgeRuntime>,
    request: Request,
    admitted_device: Option<String>,
) -> Result<Response, ApiError> {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let Some(device) = admitted_device else {
        return pair(runtime, input(request).await?).await;
    };
    if path.starts_with("/v1/preview/") {
        return proxy_preview(runtime, device, request).await;
    }
    match (method.as_str(), path.as_str()) {
        ("GET", "/v1/status") => runtime.work(&device, |_| Ok(()))
            .and_then(|_| json_response(&runtime, json!({"projectId":runtime.project_id,"projectName":runtime.project_name,"running":true}))),
        ("GET", "/v1/models") => {
            let value = models(&runtime, &device).await?;
            json_response(&runtime, value)
        }
        ("GET", "/v1/sessions") => {
            let sessions = runtime.work(&device, |state| {
                let db = state.db.lock().map_err(|_| "Database unavailable")?;
                let mut stmt = db.prepare("SELECT id FROM sessions WHERE project_id=?1 ORDER BY updated_at DESC LIMIT 1000").map_err(|_| "Database unavailable")?;
                let rows = stmt.query_map([&runtime.project_id], |r| r.get::<_,String>(0)).map_err(|_| "Database unavailable")?;
                let mut sessions = Vec::new();
                for id in rows { sessions.push(store::find_session(&db, &runtime.project_id, &id.map_err(|_| "Database unavailable")?)?); }
                Ok(sessions)
            })?;
            json_response(&runtime, sessions)
        }
        ("POST", "/v1/sessions") => {
            let body: SessionInput = input(request).await?;
            if !store::PROVIDERS.contains(&body.provider.as_str()) || body.model.trim().is_empty() || body.model.len() > 200 || body.model.chars().any(char::is_control) { return Err(ApiError::bad()); }
            agent::remote_provider_allowed(&body.provider).map_err(ApiError::from)?;
            let session = runtime.work(&device, |state| {
                let session = store::Session { id: uuid::Uuid::new_v4().to_string(), title: "New session".into(), provider: body.provider, model: body.model.trim().into(), updated_at: store::now(), external_id: None };
                state.db.lock().map_err(|_| "Database unavailable")?.execute("INSERT INTO sessions(id,project_id,title,provider,model,updated_at) VALUES(?1,?2,?3,?4,?5,?6)", params![session.id,runtime.project_id,session.title,session.provider,session.model,session.updated_at]).map_err(|_| "Database unavailable")?;
                Ok(session)
            })?;
            json_response(&runtime, session)
        }
        ("GET", "/v1/events") => {
            let after = query(&request, "after")?.unwrap_or_else(|| "0".into()).parse().map_err(|_| ApiError::bad())?;
            events(runtime, device, after).await
        }
        ("GET", "/v1/approvals") => {
            let approvals = runtime.work(&device, |state| {
                let mut inner = runtime.inner.lock().map_err(|_| "Bridge unavailable")?;
                let expired: Vec<String> = inner.approvals.iter().filter(|(_,a)| Instant::now() >= a.expires).map(|(id,_)| id.clone()).collect();
                for id in expired { close_action(state, &mut inner, &id, false); }
                Ok(inner.approvals.values().filter(|a| a.owner == device).map(|a| a.approval.clone()).collect::<Vec<_>>())
            })?;
            json_response(&runtime, approvals)
        }
        ("GET", "/v1/files") => {
            let path = query(&request, "path")?.unwrap_or_default();
            let files = runtime.work(&device, |state| project::bridge_list_locked(state, &path))?;
            json_response(&runtime, files)
        }
        ("GET", "/v1/file") => {
            let path = query(&request, "path")?.ok_or_else(ApiError::bad)?;
            let file = runtime.work(&device, |state| project::bridge_read_locked(state, &path))?;
            // File content is application data; never alter it (hash/CAS must agree).
            Ok(Json(file).into_response())
        }
        ("PUT", "/v1/file") => {
            let body: FileInput = input(request).await?;
            if body.expected_hash.as_ref().is_some_and(|h| h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit())) { return Err(ApiError::bad()); }
            let file = runtime.work(&device, |state| {
                let file = project::bridge_cas_locked(state, &body.path, &body.content, body.expected_hash.as_deref())?;
                workspace_locked(&runtime, &runtime.project_id, &file.path);
                let _ = runtime.app.emit("workspace:file-changed", json!({"projectId":runtime.project_id,"path":file.path}));
                Ok(file)
            })?;
            Ok(Json(file).into_response())
        }
        ("GET", "/v1/snapshot") => {
            let snapshot = runtime.work(&device, |state| project::bridge_snapshot_locked(state, &runtime.project_id))?;
            json_response(&runtime, snapshot)
        }
        _ => {
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() >= 4 && parts[1] == "v1" && parts[2] == "sessions" {
                let session_id = parts[3]; valid_id(session_id)?;
                if method == Method::GET && parts.len() == 4 {
                    let detail = runtime.work(&device, |state| {
                        let db = state.db.lock().map_err(|_| "Database unavailable")?;
                        let session = store::find_session(&db, &runtime.project_id, session_id)?;
                        let mut stmt = db.prepare("SELECT id,session_id,role,content,created_at FROM messages WHERE session_id=?1 ORDER BY created_at DESC,rowid DESC LIMIT 1000").map_err(|_| "Database unavailable")?;
                        let rows = stmt.query_map([session_id], |r| Ok(store::Message { id:r.get(0)?,session_id:r.get(1)?,role:r.get(2)?,content:r.get(3)?,created_at:r.get(4)? })).map_err(|_| "Database unavailable")?;
                        let mut messages = Vec::new(); let mut size = 0;
                        for row in rows { let row = row.map_err(|_| "Database unavailable")?; size += row.content.len(); if size > 8 * 1024 * 1024 { return Err("Session history exceeds bridge limit".into()); } messages.push(row); }
                        messages.reverse(); drop(stmt); drop(db);
                        if let Some(live) = state.live_assistants.lock().map_err(|_| "Live response unavailable")?.get(session_id) {
                            overlay_live_message(&mut messages, live)?;
                        }
                        let running = state.generations.lock().map_err(|_| "Generations unavailable")?.contains_key(session_id);
                        Ok(json!({"session":session,"messages":messages,"running":running}))
                    })?;
                    return json_response(&runtime, detail);
                }
                if method == Method::POST && parts.len() == 5 && parts[4] == "send" {
                    let body: SendInput = input(request).await?;
                    let id = uuid::Uuid::parse_str(&body.request_id).map_err(|_| ApiError::bad())?;
                    if let Some(path) = &body.context_path { project::bridge_path(path).map_err(ApiError::from)?; }
                    let canonical = id.hyphenated().to_string();
                    let message_id = stable_message_id(&device, &canonical);
                    let digest = project::text_hash(json!({"sessionId":session_id,"content":body.content,"contextPath":body.context_path}).to_string().as_bytes());
                    let remote = RemoteTurn { runtime: runtime.clone(), device_id: device, message_id, digest };
                    agent::send_common(session_id.into(), body.content, body.context_path, runtime.app.clone(), &runtime.app.state::<AppState>(), Some((&runtime.project_id, remote))).map_err(ApiError::from)?;
                    return json_response(&runtime, json!({"accepted":true}));
                }
                if method == Method::POST && parts.len() == 5 && parts[4] == "cancel" {
                    runtime.work(&device, |state| {
                        let db = state.db.lock().map_err(|_| "Database unavailable")?;
                        store::find_session(&db, &runtime.project_id, session_id)?;
                        drop(db);
                        let inner = runtime.inner.lock().map_err(|_| "Bridge unavailable")?;
                        if !inner.owners.get(session_id).is_some_and(|owner| owner.device_id == device) { return Err("Generation belongs to another client".into()); }
                        if let Some(tx) = state.generations.lock().map_err(|_| "Generations unavailable")?.get(session_id) { let _ = tx.send(true); }
                        Ok(())
                    })?;
                    return json_response(&runtime, json!({"ok":true}));
                }
            }
            if parts.len() == 4 && parts[1] == "v1" && parts[2] == "approvals" && method == Method::POST {
                let id = parts[3]; valid_id(id)?;
                let body: Decision = input(request).await?;
                runtime.work(&device, |state| {
                    let mut inner = runtime.inner.lock().map_err(|_| "Bridge unavailable")?;
                    let pending = inner.approvals.get(id).ok_or("Approval expired")?;
                    if let Err(error) = action_allowed(pending, &device) {
                        if error == "Approval expired" { close_action(state, &mut inner, id, false); }
                        return Err(error);
                    }
                    let generating = state.generations.lock().map_err(|_| "Generations unavailable")?
                        .get(&pending.approval.session_id).is_some_and(|sender| !*sender.borrow());
                    if !action_generation_allowed(&inner, pending, &runtime.id, &runtime.project_id, generating) {
                        close_action(state, &mut inner, id, false);
                        return Err("Approval expired".into());
                    }
                    close_action(state, &mut inner, id, body.approved);
                    runtime.notify.notify_waiters();
                    Ok(())
                })?;
                return json_response(&runtime, json!({"ok":true}));
            }
            Err(ApiError(StatusCode::NOT_FOUND, "Route not found"))
        }
    }
}

/// Defensive tombstone check. Normal finalization now emits, cleans the captured
/// origin and releases the generation slot under one project_gate critical section.
pub(crate) fn before_send_locked(state: &AppState, session: &str) -> Result<(), String> {
    let runtime = match handle(state) {
        Ok(r) => r,
        Err(e) if e == "Bridge is off" => return Ok(()),
        Err(e) => return Err(e),
    };
    let inner = runtime.inner.lock().map_err(|_| "Bridge unavailable")?;
    if inner.owners.contains_key(session)
        && !state
            .generations
            .lock()
            .map_err(|_| "Generations unavailable")?
            .contains_key(session)
    {
        return Err("Previous generation is finishing".into());
    }
    Ok(())
}
pub(crate) fn stable_message_id(device: &str, request: &str) -> String {
    format!(
        "bridge-{}",
        project::text_hash(format!("{device}\0{request}").as_bytes())
    )
}
/// Called before active-generation rejection, under the same project gate and DB lock.
pub(crate) fn retry_locked(
    db: &rusqlite::Connection,
    pid: &str,
    session: &str,
    content: &str,
    turn: &RemoteTurn,
) -> Result<bool, String> {
    retry_record(
        db,
        pid,
        session,
        content,
        &turn.message_id,
        &turn.device_id,
        &turn.digest,
    )
}
fn retry_record(
    db: &rusqlite::Connection,
    pid: &str,
    session: &str,
    content: &str,
    message_id: &str,
    device_id: &str,
    request_hash: &str,
) -> Result<bool, String> {
    let prior: Option<(String, String, String, String, String)> = db.query_row(
        "SELECT t.project_id,t.session_id,t.request_hash,m.content,t.device_id FROM bridge_turns t JOIN messages m ON m.id=t.message_id WHERE t.message_id=?1",
        [message_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|_| "Database unavailable")?;
    if let Some((project, old_session, hash, old_content, owner)) = prior {
        if project == pid
            && old_session == session
            && hash == request_hash
            && old_content == content
            && owner == device_id
        {
            return Ok(true);
        }
        return Err("Request ID was reused with different input".into());
    }
    Ok(false)
}

struct PairGuard {
    runtime: Arc<BridgeRuntime>,
    request_id: String,
}
impl Drop for PairGuard {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.runtime.inner.lock() {
            inner.pairs.remove(&self.request_id);
        }
    }
}
fn pair_commit_allowed(
    running: bool,
    invite: &Invitation,
    epoch: u64,
    approved: bool,
    pending: bool,
) -> bool {
    running && approved && pending && invite.epoch == epoch && Instant::now() < invite.expires
}
fn action_allowed(pending: &PendingAction, owner: &str) -> Result<(), String> {
    if pending.owner != owner {
        return Err("Approval belongs to another client".into());
    }
    if Instant::now() >= pending.expires {
        return Err("Approval expired".into());
    }
    Ok(())
}
fn action_generation_allowed(
    inner: &Inner,
    pending: &PendingAction,
    runtime_id: &str,
    project_id: &str,
    generating: bool,
) -> bool {
    let owner = TurnOwner {
        device_id: pending.origin.device_id.clone(),
        message_id: pending.origin.message_id.clone(),
    };
    generating
        && pending.origin.runtime_id == runtime_id
        && pending.origin.project_id == project_id
        && pending.origin.session_id == pending.approval.session_id
        && action_allowed(pending, &pending.origin.device_id).is_ok()
        && turn_allowed(inner, &pending.approval.session_id, &owner).is_ok()
}
async fn pair(runtime: Arc<BridgeRuntime>, body: PairInput) -> Result<Response, ApiError> {
    if body.token.len() > 128
        || body.device_name.trim().is_empty()
        || body.device_name.len() > 120
        || body.device_name.chars().any(char::is_control)
        || body.device_name.contains(['/', '\\', ':'])
    {
        return Err(ApiError::bad());
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let (sender, receiver) = oneshot::channel();
    let epoch = {
        let state = runtime.app.state::<AppState>();
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| ApiError::internal())?;
        project::expect_locked(&state, &runtime.project_id).map_err(ApiError::from)?;
        let mut inner = runtime.inner.lock().map_err(|_| ApiError::internal())?;
        if !inner.running {
            return Err(ApiError::conflict());
        }
        if inner.pairs.len() >= 8 || inner.devices.len() >= MAX_DEVICES {
            return Err(ApiError::conflict());
        }
        let epoch = consume_invite(&mut inner.invitation, &body.token)?;
        inner.pairs.insert(
            request_id.clone(),
            PairPending {
                epoch,
                sender: Some(sender),
            },
        );
        epoch
    };
    let _pending = PairGuard {
        runtime: runtime.clone(),
        request_id: request_id.clone(),
    };
    let _ = runtime.app.emit(
        "bridge:pair-request",
        json!({"requestId":request_id,"deviceName":body.device_name.trim()}),
    );
    let approved = tokio::time::timeout(APPROVAL_TTL, receiver)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false);
    let token = random_token("ncb_");
    let device = Device {
        id: uuid::Uuid::new_v4().to_string(),
        name: body.device_name.trim().into(),
        created_at: store::now(),
    };
    {
        let state = runtime.app.state::<AppState>();
        let _gate = state
            .project_gate
            .lock()
            .map_err(|_| ApiError::internal())?;
        project::expect_locked(&state, &runtime.project_id).map_err(ApiError::from)?;
        let mut inner = runtime.inner.lock().map_err(|_| ApiError::internal())?;
        let pending = inner.pairs.remove(&request_id);
        if !pair_commit_allowed(
            inner.running,
            &inner.invitation,
            epoch,
            approved,
            pending.is_some(),
        ) {
            return Err(ApiError::unauthorized());
        }
        let hash = digest(token.as_bytes());
        state.db.lock().map_err(|_| ApiError::internal())?.execute("INSERT INTO bridge_devices(id,name,token_hash,project_id,created_at) VALUES(?1,?2,?3,?4,?5)", params![device.id,device.name,hash.as_slice(),runtime.project_id,device.created_at]).map_err(|_| ApiError::internal())?;
        inner.devices.insert(
            device.id.clone(),
            Credential {
                device: device.clone(),
                hash,
            },
        );
    }
    json_response(
        &runtime,
        json!({"deviceId":device.id,"token":token,"projectId":runtime.project_id,"projectName":runtime.project_name}),
    )
}
fn overlay_live_message(
    messages: &mut [store::Message],
    live: &crate::state::LiveAssistant,
) -> Result<(), String> {
    let message = messages
        .iter_mut()
        .find(|message| message.id == live.id && message.role == "assistant")
        .ok_or("Live assistant is outside the session snapshot")?;
    message.content.clone_from(&live.content);
    if messages
        .iter()
        .map(|message| message.content.len())
        .sum::<usize>()
        > 8 * 1024 * 1024
    {
        return Err("Session history exceeds bridge limit".into());
    }
    Ok(())
}

struct PollGuard {
    runtime: Arc<BridgeRuntime>,
    device: String,
}
impl Drop for PollGuard {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.runtime.inner.lock() {
            inner.polls.remove(&self.device);
        }
    }
}
async fn events(
    runtime: Arc<BridgeRuntime>,
    device: String,
    after: u64,
) -> Result<Response, ApiError> {
    runtime.work(&device, |_| {
        if !runtime
            .inner
            .lock()
            .map_err(|_| "Bridge unavailable")?
            .polls
            .insert(device.clone())
        {
            return Err("Only one event poll per device is allowed".into());
        }
        Ok(())
    })?;
    let _guard = PollGuard {
        runtime: runtime.clone(),
        device: device.clone(),
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        // Register before reading to avoid losing a notify between read and await.
        let notified = runtime.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let value = runtime.work(&device, |_| {
            Ok(runtime
                .inner
                .lock()
                .map_err(|_| "Bridge unavailable")?
                .replay
                .poll(after, &device))
        })?;
        if value["reset"] == true
            || value["cursor"].as_u64() != Some(after)
            || tokio::time::Instant::now() >= deadline
        {
            return json_response(&runtime, value);
        }
        let _ = tokio::time::timeout_at(deadline, notified).await;
    }
}
async fn models(runtime: &Arc<BridgeRuntime>, device: &str) -> Result<Value, ApiError> {
    runtime.work(device, |_| Ok(()))?;
    // One discovery at a time; parallel phones cannot spawn an unbounded CLI fleet.
    let mut cache = runtime.models.lock().await;
    if let Some((time, value)) = cache.as_ref() {
        if time.elapsed() < Duration::from_secs(60) {
            runtime.work(device, |_| Ok(()))?;
            return Ok(value.clone());
        }
    }
    let mut result = Vec::new();
    let mut errors = Vec::new();
    match tokio::time::timeout(
        Duration::from_secs(18),
        agent::agent_models(runtime.app.state::<AppState>()),
    )
    .await
    {
        Ok(Ok(ids)) => {
            for id in ids.into_iter().take(1000) {
                result.push(json!({"provider":"ollama","id":id,"label":id}));
            }
        }
        _ => errors.push("Ollama model discovery unavailable".to_owned()),
    }
    let compatible = runtime.work(device, |state| {
        let db = state.db.lock().map_err(|_| "Database unavailable")?;
        Ok(db
            .query_row(
                "SELECT value FROM settings WHERE key='compatible_model'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| "Database unavailable")?
            .unwrap_or_default())
    })?;
    for id in compatible
        .split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 200)
        .take(1000)
    {
        result.push(json!({"provider":"compatible","id":id,"label":id}));
    }
    for provider in ["codex", "claude", "opencode"] {
        errors.push(format!("{provider}: {}", agent::REMOTE_SANDBOX_LIMITATION));
    }
    runtime.work(device, |_| Ok(()))?;
    let value = json!({"models":result,"errors":errors});
    *cache = Some((Instant::now(), value.clone()));
    Ok(value)
}

/// Only normal HTTP is supported. Upgrade/WS, redirects and ambient proxy/DNS are disabled.
fn hop_header(name: &str, connection: &HashSet<String>) -> bool {
    connection.contains(name)
        || matches!(
            name,
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "host"
                | "content-length"
        )
}
fn connection_headers(headers: &HeaderMap) -> HashSet<String> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .collect()
}
async fn proxy_preview(
    runtime: Arc<BridgeRuntime>,
    device: String,
    request: Request,
) -> Result<Response, ApiError> {
    if request.headers().contains_key(header::UPGRADE)
        || request.headers().contains_key("sec-websocket-key")
    {
        return Err(ApiError(
            StatusCode::NOT_IMPLEMENTED,
            "WebSocket preview is not supported",
        ));
    }
    if !matches!(
        *request.method(),
        Method::GET
            | Method::HEAD
            | Method::POST
            | Method::PUT
            | Method::PATCH
            | Method::DELETE
            | Method::OPTIONS
    ) {
        return Err(ApiError::bad());
    }
    let suffix = request
        .uri()
        .path()
        .strip_prefix("/v1/preview/")
        .ok_or_else(ApiError::bad)?;
    let (port, path) = suffix.split_once('/').unwrap_or((suffix, ""));
    let port: u16 = port.parse().map_err(|_| ApiError::bad())?;
    if port == 0 || path.len() > 4096 {
        return Err(ApiError::bad());
    }
    runtime.work(&device, |_| {
        if !runtime
            .inner
            .lock()
            .map_err(|_| "Bridge unavailable")?
            .preview_ports
            .contains(&port)
        {
            return Err("Preview port is not allowed".into());
        }
        Ok(())
    })?;
    let url = format!(
        "http://127.0.0.1:{port}/{path}{}",
        request
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default()
    );
    let url = reqwest::Url::parse(&url).map_err(|_| ApiError::bad())?;
    if url.host_str() != Some("127.0.0.1")
        || url.port() != Some(port)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ApiError::bad());
    }
    let connection = connection_headers(request.headers());
    let mut upstream = runtime.preview.request(request.method().clone(), url);
    for (name, value) in request.headers() {
        if hop_header(name.as_str(), &connection)
            || matches!(
                name.as_str(),
                "authorization"
                    | "cookie"
                    | "forwarded"
                    | "x-forwarded-for"
                    | "x-forwarded-host"
                    | "x-forwarded-proto"
                    | "referer"
            )
        {
            continue;
        }
        upstream = upstream.header(name, value);
    }
    let body = tokio::time::timeout(
        Duration::from_secs(10),
        to_bytes(request.into_body(), MAX_BODY),
    )
    .await
    .map_err(|_| ApiError::bad())?
    .map_err(|_| ApiError::bad())?;
    // Recheck approval/project before reserving the outbound operation.
    runtime.work(&device, |_| {
        if !runtime
            .inner
            .lock()
            .map_err(|_| "Bridge unavailable")?
            .preview_ports
            .contains(&port)
        {
            return Err("Preview port is not allowed".into());
        }
        Ok(())
    })?;
    let mut upstream = upstream
        .body(body)
        .send()
        .await
        .map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "Preview server unavailable"))?;
    if upstream.status().is_redirection() {
        return Err(ApiError(
            StatusCode::BAD_GATEWAY,
            "Preview redirects are disabled",
        ));
    }
    let status = upstream.status();
    let connection = connection_headers(upstream.headers());
    let mut headers = HeaderMap::new();
    for (name, value) in upstream.headers() {
        if hop_header(name.as_str(), &connection)
            || matches!(
                name.as_str(),
                "set-cookie"
                    | "location"
                    | "www-authenticate"
                    | "access-control-allow-origin"
                    | "access-control-allow-credentials"
                    | "content-security-policy"
                    | "refresh"
            )
        {
            continue;
        }
        if headers.len() >= 64 || value.as_bytes().len() > 8192 {
            return Err(ApiError::bad());
        }
        headers.append(name.clone(), value.clone());
    }
    if headers
        .iter()
        .map(|(k, v)| k.as_str().len() + v.as_bytes().len())
        .sum::<usize>()
        > 16 * 1024
    {
        return Err(ApiError::bad());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = upstream
        .chunk()
        .await
        .map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "Preview response failed"))?
    {
        if bytes.len() + chunk.len() > MAX_PREVIEW {
            return Err(ApiError(
                StatusCode::BAD_GATEWAY,
                "Preview response too large",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    runtime.work(&device, |_| {
        if !runtime
            .inner
            .lock()
            .map_err(|_| "Bridge unavailable")?
            .preview_ports
            .contains(&port)
        {
            return Err("Preview port is not allowed".into());
        }
        Ok(())
    })?;
    headers.insert("content-security-policy", header::HeaderValue::from_static("default-src 'self' data: blob:; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

fn current_runtime(app: &AppHandle) -> Option<Arc<BridgeRuntime>> {
    handle(&app.state::<AppState>()).ok()
}
/// Legacy native CLI emitter; it can never route into a remote-owned turn.
pub(crate) fn emit_agent(app: &AppHandle, session: &str, kind: &str, text: &str) {
    let state = app.state::<AppState>();
    let Ok(_gate) = state.project_gate.lock() else {
        return;
    };
    emit_job_locked(app, None, session, kind, text);
}

/// Caller retains project_gate, including through terminal emission and slot release.
pub(crate) fn emit_job_locked(
    app: &AppHandle,
    turn: Option<&RemoteTurn>,
    session: &str,
    kind: &str,
    text: &str,
) {
    let runtime = match turn {
        Some(turn) => turn.runtime.clone(),
        None => match current_runtime(app) {
            Some(runtime) => runtime,
            None => return,
        },
    };
    let state = app.state::<AppState>();
    let Ok(mut inner) = runtime.inner.lock() else {
        return;
    };
    let owner = match turn {
        Some(turn) if inner.owners.get(session) == Some(&turn.owner()) => {
            Some(turn.device_id.clone())
        }
        Some(_) => return,
        None if inner.owners.contains_key(session) => return,
        None => None,
    };
    if !inner.running
        || project::expect_locked(&state, &runtime.project_id).is_err()
        || owner
            .as_ref()
            .is_some_and(|device| !inner.devices.contains_key(device))
    {
        return;
    }
    let belongs = state
        .db
        .lock()
        .ok()
        .is_some_and(|db| store::find_session(&db, &runtime.project_id, session).is_ok());
    if !belongs
        || !matches!(
            kind,
            "delta" | "done" | "error" | "status" | "tool" | "action" | "message"
        )
    {
        return;
    }
    let clean = if kind == "error" && text != agent::REMOTE_SANDBOX_LIMITATION {
        Value::String("Generation failed; details are available on Desktop".into())
    } else {
        runtime.clean(Value::String(text.into()))
    };
    let clean = clean.as_str().unwrap_or("");
    let message_id = turn.map(|turn| turn.message_id.as_str());
    let mut remaining = clean;
    while !remaining.is_empty() {
        let mut end = remaining.len().min(8 * 1024);
        while !remaining.is_char_boundary(end) {
            end -= 1;
        }
        inner.replay.push("agent", Some(session), json!({"sessionId":session,"messageId":message_id,"type":kind,"text":&remaining[..end]}), owner.clone());
        remaining = &remaining[end..];
    }
    if clean.is_empty() {
        inner.replay.push(
            "agent",
            Some(session),
            json!({"sessionId":session,"messageId":message_id,"type":kind,"text":""}),
            owner,
        );
    }
    runtime.notify.notify_waiters();
}

fn cleanup_origin(state: &AppState, inner: &mut Inner, origin: &RemoteApprovalOrigin) {
    let ids: Vec<String> = inner
        .approvals
        .iter()
        .filter(|(_, action)| &action.origin == origin)
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        close_action(state, inner, &id, false);
    }
    let owner = TurnOwner {
        device_id: origin.device_id.clone(),
        message_id: origin.message_id.clone(),
    };
    if inner.owners.get(&origin.session_id) == Some(&owner) {
        inner.owners.remove(&origin.session_id);
    }
}

pub(crate) fn cleanup_turn_locked(state: &AppState, turn: Option<&RemoteTurn>, session: &str) {
    if let Some(turn) = turn {
        let mut inner = turn
            .runtime
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        cleanup_origin(state, &mut inner, &turn.approval_origin(session));
        turn.runtime.notify.notify_waiters();
    }
}
fn workspace_locked(runtime: &BridgeRuntime, pid: &str, path: &str) {
    if pid != runtime.project_id
        || project::bridge_check_locked(&runtime.app.state::<AppState>(), path, false).is_err()
    {
        return;
    }
    if let Ok(mut inner) = runtime.inner.lock() {
        if inner.running {
            inner.replay.push(
                "workspace",
                None,
                json!({"projectId":pid,"path":path}),
                None,
            );
            runtime.notify.notify_waiters();
        }
    }
}
pub(crate) fn emit_workspace(app: &AppHandle, pid: &str, path: &str) {
    let Some(runtime) = current_runtime(app) else {
        return;
    };
    let state = app.state::<AppState>();
    let Ok(_gate) = state.project_gate.lock() else {
        return;
    };
    if project::expect_locked(&state, pid).is_ok() {
        workspace_locked(&runtime, pid, path);
    }
}
pub(crate) fn emit_workspace_locked(app: &AppHandle, pid: &str, path: &str) {
    if let Some(runtime) = current_runtime(app) {
        if project::expect_locked(&app.state::<AppState>(), pid).is_ok() {
            workspace_locked(&runtime, pid, path);
        }
    }
}
/// Register sender + immutable origin before publishing. Never infer ownership
/// from CURRENT runtime/session, and never fall back to Desktop for a remote job.
pub(crate) fn register_approval(
    app: &AppHandle,
    job: &agent::Job,
    request_id: &str,
    name: &str,
    arguments: &Value,
    sender: oneshot::Sender<bool>,
) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate unavailable")?;
    agent::authorize_job_locked(&state, job)?;
    let Some(turn) = job.remote.as_ref() else {
        state
            .agent_approvals
            .lock()
            .map_err(|_| "Approvals unavailable")?
            .insert(
                request_id.into(),
                AgentApproval {
                    remote: None,
                    sender,
                },
            );
        return Ok(false);
    };
    let runtime = &turn.runtime;
    let mut inner = runtime.inner.lock().map_err(|_| "Bridge unavailable")?;
    turn_allowed(&inner, &job.session_id, &turn.owner())?;
    let origin = turn.approval_origin(&job.session_id);
    let arguments = runtime.clean(arguments.clone());
    if arguments.to_string().len() > 48 * 1024 || inner.approvals.len() >= 64 {
        let _ = sender.send(false);
        return Ok(true);
    }
    let approval = Approval {
        request_id: request_id.into(),
        session_id: job.session_id.clone(),
        name: name.into(),
        arguments,
    };
    let mut data = serde_json::to_value(&approval).map_err(|_| "Cannot encode approval")?;
    data["messageId"] = json!(turn.message_id);
    state
        .agent_approvals
        .lock()
        .map_err(|_| "Approvals unavailable")?
        .insert(
            request_id.into(),
            AgentApproval {
                remote: Some(origin.clone()),
                sender,
            },
        );
    inner.approvals.insert(
        request_id.into(),
        PendingAction {
            origin,
            owner: turn.device_id.clone(),
            approval,
            expires: Instant::now() + APPROVAL_TTL,
        },
    );
    inner.replay.push(
        "approval",
        Some(&job.session_id),
        data,
        Some(turn.device_id.clone()),
    );
    runtime.notify.notify_waiters();
    Ok(true)
}

pub(crate) fn approval_closed(app: &AppHandle, turn: Option<&RemoteTurn>, request_id: &str) {
    let state = app.state::<AppState>();
    let _gate = state
        .project_gate
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if let Some(turn) = turn {
        let mut inner = turn
            .runtime
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        // Request IDs are unique, but origin still guards against crossed generations.
        if inner.approvals.get(request_id).is_some_and(|action| {
            action.origin.runtime_id == turn.runtime.id
                && action.origin.device_id == turn.device_id
                && action.origin.message_id == turn.message_id
        }) {
            close_action(&state, &mut inner, request_id, false);
        }
        turn.runtime.notify.notify_waiters();
    } else {
        let mut senders = state
            .agent_approvals
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if senders
            .get(request_id)
            .is_some_and(|entry| entry.remote.is_none())
        {
            senders.remove(request_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_origin(runtime: &str, device: &str, message: &str) -> RemoteApprovalOrigin {
        RemoteApprovalOrigin {
            runtime_id: runtime.into(),
            project_id: "p".into(),
            session_id: "s".into(),
            device_id: device.into(),
            message_id: message.into(),
        }
    }
    fn test_inner() -> Inner {
        let mut devices = HashMap::new();
        devices.insert(
            "one".into(),
            Credential {
                device: Device {
                    id: "one".into(),
                    name: "Synthetic".into(),
                    created_at: 1,
                },
                hash: digest(b"synthetic"),
            },
        );
        Inner {
            running: true,
            invitation: invitation(1),
            devices,
            pairs: HashMap::new(),
            owners: HashMap::new(),
            approvals: HashMap::new(),
            preview_ports: HashSet::new(),
            rates: HashMap::new(),
            pair_rate: Rate::new(),
            polls: HashSet::new(),
            replay: Replay::default(),
        }
    }
    fn test_pending(origin: RemoteApprovalOrigin, request: &str) -> PendingAction {
        PendingAction {
            owner: origin.device_id.clone(),
            origin,
            approval: Approval {
                request_id: request.into(),
                session_id: "s".into(),
                name: "write_file".into(),
                arguments: json!({"path":"synthetic.txt"}),
            },
            expires: Instant::now() + APPROVAL_TTL,
        }
    }
    #[tokio::test]
    async fn old_turn_cleanup_security_regression() {
        let state = AppState::new(rusqlite::Connection::open_in_memory().unwrap());
        let old = test_origin("old-runtime", "one", "old-turn");
        let new = test_origin("new-runtime", "one", "new-turn");
        let mut inner = test_inner();
        inner.owners.insert(
            "s".into(),
            TurnOwner {
                device_id: "one".into(),
                message_id: "new-turn".into(),
            },
        );
        let (old_tx, old_rx) = oneshot::channel();
        let (new_tx, mut new_rx) = oneshot::channel();
        state.agent_approvals.lock().unwrap().insert(
            "old".into(),
            AgentApproval {
                remote: Some(old.clone()),
                sender: old_tx,
            },
        );
        state.agent_approvals.lock().unwrap().insert(
            "new".into(),
            AgentApproval {
                remote: Some(new.clone()),
                sender: new_tx,
            },
        );
        inner
            .approvals
            .insert("old".into(), test_pending(old.clone(), "old"));
        inner
            .approvals
            .insert("new".into(), test_pending(new.clone(), "new"));
        {
            let _gate = state.project_gate.lock().unwrap();
            cleanup_origin(&state, &mut inner, &old);
            assert_eq!(inner.owners["s"].message_id, "new-turn");
            assert!(inner.approvals.contains_key("new"));
            assert!(action_generation_allowed(
                &inner,
                &inner.approvals["new"],
                "new-runtime",
                "p",
                true
            ));
            assert!(!action_generation_allowed(
                &inner,
                &inner.approvals["new"],
                "old-runtime",
                "p",
                true
            ));
            assert!(!action_generation_allowed(
                &inner,
                &inner.approvals["new"],
                "new-runtime",
                "p",
                false
            ));
        }
        assert!(!old_rx.await.unwrap());
        assert!(matches!(
            new_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        let same_runtime_old = test_origin("new-runtime", "one", "old-turn");
        cleanup_origin(&state, &mut inner, &same_runtime_old);
        assert_eq!(inner.owners["s"].message_id, "new-turn");
        inner.approvals.get_mut("new").unwrap().expires = Instant::now() - Duration::from_secs(1);
        assert!(!action_generation_allowed(
            &inner,
            &inner.approvals["new"],
            "new-runtime",
            "p",
            true
        ));
        cleanup_origin(&state, &mut inner, &new);
        assert!(!new_rx.await.unwrap());
        assert!(inner.owners.is_empty());
    }
    #[test]
    fn revoke_operation_fence_security_regression() {
        let state = AppState::new(rusqlite::Connection::open_in_memory().unwrap());
        let inner = Mutex::new(test_inner());
        let owner = TurnOwner {
            device_id: "one".into(),
            message_id: "turn".into(),
        };
        inner
            .lock()
            .unwrap()
            .owners
            .insert("s".into(), owner.clone());
        let mut operations = 0;
        {
            let _gate = state.project_gate.lock().unwrap();
            turn_allowed(&inner.lock().unwrap(), "s", &owner).unwrap();
            std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        assert!(matches!(
                            state.project_gate.try_lock(),
                            Err(std::sync::TryLockError::WouldBlock)
                        ))
                    })
                    .join()
                    .unwrap();
            });
            operations += 1; // synchronous IO is performed before releasing the same gate
        }
        {
            let _gate = state.project_gate.lock().unwrap();
            let mut inner = inner.lock().unwrap();
            inner.devices.remove("one");
            if turn_allowed(&inner, "s", &owner).is_ok() {
                operations += 1;
            }
            assert!(inner.owners.contains_key("s")); // revocation retains the tombstone
            inner.running = false;
            assert!(turn_allowed(&inner, "s", &owner).is_err());
        }
        assert_eq!(operations, 1);
    }
    #[test]
    fn admission_before_body_security_regression() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let polled = Arc::new(AtomicBool::new(false));
        let marker = polled.clone();
        let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
            marker.store(true, Ordering::SeqCst);
            std::task::Poll::<Option<Result<axum::body::Bytes, std::io::Error>>>::Pending
        }));
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/v1/pair")
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .unwrap();
        let mut rate = Rate::new();
        assert!(admit_request(
            &request,
            || {
                assert!(rate.allow(1));
                Ok(())
            },
            || panic!("pair must not authenticate")
        )
        .unwrap()
        .is_none());
        assert!(admit_request(
            &request,
            || {
                if rate.allow(1) {
                    Ok(())
                } else {
                    Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate"))
                }
            },
            || panic!()
        )
        .is_err());
        *request.method_mut() = Method::GET;
        assert!(admit_request(&request, || panic!("method before rate"), || panic!()).is_err());
        *request.method_mut() = Method::POST;
        request.headers_mut().remove(header::CONTENT_TYPE);
        assert!(
            admit_request(&request, || panic!("content type before rate"), || panic!()).is_err()
        );
        request
            .headers_mut()
            .insert(header::ORIGIN, "null".parse().unwrap());
        assert!(admit_request(&request, || panic!("browser before rate"), || panic!()).is_err());
        request.headers_mut().remove(header::ORIGIN);
        *request.uri_mut() = "/v1/sessions".parse().unwrap();
        let authentications = AtomicUsize::new(0);
        assert!(admit_request(
            &request,
            || panic!(),
            || {
                authentications.fetch_add(1, Ordering::SeqCst);
                Err(ApiError::unauthorized())
            }
        )
        .is_err());
        assert_eq!(authentications.load(Ordering::SeqCst), 1);
        assert!(!polled.load(Ordering::SeqCst));
        let authenticated = tokio::sync::Semaphore::new(96);
        let pairing = tokio::sync::Semaphore::new(4);
        let held: Vec<_> = (0..4)
            .map(|_| {
                request_pool(None, &authenticated, &pairing)
                    .try_acquire()
                    .unwrap()
            })
            .collect();
        assert!(request_pool(None, &authenticated, &pairing)
            .try_acquire()
            .is_err());
        let device = admit_request(
            &request,
            || panic!(),
            || {
                authentications.fetch_add(1, Ordering::SeqCst);
                Ok("one".into())
            },
        )
        .unwrap();
        assert!(request_pool(device.as_deref(), &authenticated, &pairing)
            .try_acquire()
            .is_ok());
        assert_eq!(authentications.load(Ordering::SeqCst), 2);
        assert_eq!(authenticated.available_permits(), 96);
        drop(held);
    }
    #[test]
    fn auth_refuses_origins_and_ambiguous_credentials() {
        let mut headers = HeaderMap::new();
        assert!(bearer(&headers).is_err());
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", random_token("ncb_")).parse().unwrap(),
        );
        assert!(bearer(&headers).is_ok());
        headers.insert(header::ORIGIN, "null".parse().unwrap());
        assert!(refuse_browser(&headers).is_err());
        headers.remove(header::ORIGIN);
        headers.insert("sec-fetch-site", "none".parse().unwrap());
        assert!(refuse_browser(&headers).is_err());
        headers.remove("sec-fetch-site");
        headers.append(header::AUTHORIZATION, "Bearer other".parse().unwrap());
        assert!(bearer(&headers).is_err());
    }
    #[test]
    fn invitation_is_one_use_expiring_and_epoch_bound() {
        let mut invite = invitation(7);
        let token = invite.token.clone();
        assert!(consume_invite(&mut invite, "wrong").is_err());
        assert_eq!(consume_invite(&mut invite, &token).unwrap(), 7);
        assert!(consume_invite(&mut invite, &token).is_err());
        let mut expired = invitation(8);
        expired.expires = Instant::now() - Duration::from_secs(1);
        let token = expired.token.clone();
        assert!(consume_invite(&mut expired, &token).is_err());
        let rotated = invitation(9);
        assert_ne!(rotated.epoch, 7);
        assert!(!constant_time(&digest(b"a"), &digest(b"b")));
        assert!(constant_time(&digest(b"a"), &digest(b"a")));
    }
    #[test]
    fn private_addresses_exclude_public_and_wildcard() {
        for ip in [
            "0.0.0.0",
            "8.8.8.8",
            "169.254.1.2",
            "224.0.0.1",
            "100.64.0.1",
        ] {
            assert!(!private_ip(ip.parse().unwrap()));
        }
        for ip in ["127.0.0.1", "192.168.1.1", "10.1.2.3", "172.16.0.1"] {
            assert!(private_ip(ip.parse().unwrap()));
        }
    }
    #[test]
    fn certificate_roundtrip_preserves_der_and_fingerprint() {
        let (cert, key) = new_material(vec!["127.0.0.1".into()]).unwrap();
        let raw = serde_json::to_string(&TlsMaterial {
            cert: STANDARD.encode(&cert),
            key: STANDARD.encode(&key),
        })
        .unwrap();
        let (saved_cert, saved_key) = decode_material(&raw).unwrap();
        assert_eq!(cert, saved_cert);
        assert_eq!(key, saved_key);
        assert_eq!(project::text_hash(&cert), project::text_hash(&saved_cert));
    }
    #[test]
    fn live_snapshot_replaces_persisted_prefix_without_duplication() {
        let mut messages = vec![store::Message {
            id: "a".into(),
            session_id: "s".into(),
            role: "assistant".into(),
            content: "saved".into(),
            created_at: 1,
        }];
        let live = crate::state::LiveAssistant {
            id: "a".into(),
            content: "saved + unsaved Привет".into(),
        };
        overlay_live_message(&mut messages, &live).unwrap();
        overlay_live_message(&mut messages, &live).unwrap();
        assert_eq!(messages[0].content, live.content);
        assert!(overlay_live_message(
            &mut messages,
            &crate::state::LiveAssistant {
                id: "other-turn".into(),
                content: "wrong".into()
            }
        )
        .is_err());
        messages[0].role = "user".into();
        assert!(overlay_live_message(&mut messages, &live).is_err());
    }
    #[test]
    fn replay_is_bounded_resets_and_does_not_leak_peer_approvals() {
        let mut replay = Replay::default();
        for i in 0..600 {
            replay.push("agent", Some("s"), json!({"i":i}), None);
        }
        assert_eq!(replay.events.len(), MAX_EVENTS);
        assert_eq!(replay.poll(0, "d")["reset"], true);
        replay.push(
            "approval",
            Some("s"),
            json!({"secret":"peer"}),
            Some("one".into()),
        );
        let poll = replay.poll(600, "two");
        assert_eq!(poll["events"].as_array().unwrap().len(), 0);
        assert_eq!(poll["cursor"], 601);
        assert_eq!(
            replay.poll(600, "one")["events"].as_array().unwrap().len(),
            1
        );
    }
    #[tokio::test]
    async fn pairing_denial_never_autoapproves() {
        let (tx, rx) = oneshot::channel();
        tx.send(false).unwrap();
        assert!(!tokio::time::timeout(APPROVAL_TTL, rx)
            .await
            .unwrap()
            .unwrap());
        let (tx, rx) = oneshot::channel::<bool>();
        drop(tx);
        assert!(!rx.await.unwrap_or(false));
    }
    #[test]
    fn stable_request_ids_separate_devices() {
        assert_eq!(stable_message_id("a", "r"), stable_message_id("a", "r"));
        assert_ne!(stable_message_id("a", "r"), stable_message_id("b", "r"));
    }
    #[test]
    fn pending_pair_is_invalidated_by_rotation_stop_or_denial() {
        let invite = invitation(3);
        assert!(pair_commit_allowed(true, &invite, 3, true, true));
        assert!(!pair_commit_allowed(true, &invite, 2, true, true));
        assert!(!pair_commit_allowed(false, &invite, 3, true, true));
        assert!(!pair_commit_allowed(true, &invite, 3, false, true));
        assert!(!pair_commit_allowed(true, &invite, 3, true, false));
    }
    #[test]
    fn retries_match_persisted_input_without_duplicating_messages() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        store::initialize(&db).unwrap();
        initialize_bridge(&db).unwrap();
        db.execute("INSERT INTO projects VALUES('p','/project','Project')", [])
            .unwrap();
        db.execute("INSERT INTO sessions(id,project_id,title,provider,model,updated_at) VALUES('s','p','Title','ollama','model',1)", []).unwrap();
        db.execute(
            "INSERT INTO messages VALUES('m','s','user','hello',1),('a','s','assistant','',2)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO bridge_turns VALUES('m','d','p','s','hash')",
            [],
        )
        .unwrap();
        for _ in 0..3 {
            assert!(retry_record(&db, "p", "s", "hello", "m", "d", "hash").unwrap());
        }
        assert!(retry_record(&db, "p", "s", "changed", "m", "d", "hash").is_err());
        assert!(retry_record(&db, "p", "other", "hello", "m", "d", "hash").is_err());
        assert!(retry_record(&db, "other", "s", "hello", "m", "d", "hash").is_err());
        assert!(retry_record(&db, "p", "s", "hello", "m", "peer", "hash").is_err());
        assert!(retry_record(&db, "p", "s", "hello", "m", "d", "different context").is_err());
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
    #[tokio::test]
    async fn peer_cannot_approve_and_revocation_closes_pending_action() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        store::initialize(&db).unwrap();
        let state = AppState::new(db);
        let (tx, rx) = oneshot::channel();
        state.agent_approvals.lock().unwrap().insert(
            "request".into(),
            AgentApproval {
                remote: Some(test_origin("runtime", "one", "turn")),
                sender: tx,
            },
        );
        let pending = PendingAction {
            origin: test_origin("runtime", "one", "turn"),
            owner: "one".into(),
            approval: Approval {
                request_id: "request".into(),
                session_id: "s".into(),
                name: "write_file".into(),
                arguments: json!({"path":"a"}),
            },
            expires: Instant::now() + APPROVAL_TTL,
        };
        assert!(action_allowed(&pending, "peer").is_err());
        assert!(action_allowed(&pending, "one").is_ok());
        let mut inner = Inner {
            running: true,
            invitation: invitation(1),
            devices: HashMap::new(),
            pairs: HashMap::new(),
            owners: HashMap::new(),
            approvals: HashMap::new(),
            preview_ports: HashSet::new(),
            rates: HashMap::new(),
            pair_rate: Rate::new(),
            polls: HashSet::new(),
            replay: Replay::default(),
        };
        let hash = digest(b"token");
        inner.devices.insert(
            "one".into(),
            Credential {
                device: Device {
                    id: "one".into(),
                    name: "Phone".into(),
                    created_at: 1,
                },
                hash,
            },
        );
        inner.approvals.insert("request".into(), pending);
        inner.devices.remove("one");
        close_action(&state, &mut inner, "request", false);
        assert!(!rx.await.unwrap());
        assert!(inner.approvals.is_empty());
        assert!(inner.devices.is_empty());
        assert_eq!(
            inner.replay.poll(0, "one")["events"][0]["kind"],
            "approval-closed"
        );
        assert!(inner.replay.poll(0, "peer")["events"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn rate_and_proxy_headers_are_bounded() {
        let mut rate = Rate::new();
        assert!(rate.allow(1));
        assert!(!rate.allow(1));
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, "x-private, Upgrade".parse().unwrap());
        let connection = connection_headers(&headers);
        assert!(hop_header("x-private", &connection));
        assert!(hop_header("upgrade", &connection));
        assert!(!hop_header("content-type", &connection));
    }
}
