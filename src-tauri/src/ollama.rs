use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    io,
    panic::AssertUnwindSafe,
    path::PathBuf,
    time::{Duration, Instant},
};

use futures_util::{stream, FutureExt, StreamExt};
use reqwest::{Client, Response, StatusCode};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
    sync::watch,
};

use crate::{agent, state::AppState};

const MAX_LINE: usize = 64 * 1024;
const MAX_RECORDS: usize = 1_000_000;
const MAX_FILES: usize = 256;
const IO_CHUNK: usize = 256 * 1024;
const EVENT_INTERVAL: Duration = Duration::from_millis(100);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const JOB_TIMEOUT: Duration = Duration::from_secs(48 * 60 * 60);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProgress {
    pub job_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub cancelled: bool,
}

#[derive(Debug)]
enum JobError {
    Cancelled,
    Failed(String),
}
type JobResult<T> = Result<T, JobError>;
impl From<String> for JobError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}
impl From<&str> for JobError {
    fn from(value: &str) -> Self {
        Self::Failed(value.into())
    }
}
impl From<io::Error> for JobError {
    fn from(value: io::Error) -> Self {
        Self::Failed(format!("GGUF file I/O failed: {value}"))
    }
}
impl From<reqwest::Error> for JobError {
    fn from(value: reqwest::Error) -> Self {
        Self::Failed(format!("Ollama request failed: {value}"))
    }
}

fn endpoint(state: &AppState) -> Result<String, String> {
    let saved: Option<String> = {
        let db = state.db.lock().map_err(|_| "Database lock poisoned")?;
        db.query_row(
            "SELECT value FROM settings WHERE key='ollama_url'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    };
    agent::validate_url(saved.as_deref().unwrap_or("http://127.0.0.1:11434"))
}

fn client() -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(IDLE_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

fn name_part(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains("..")
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn validate_model(input: &str) -> Result<String, String> {
    let name = input.trim();
    if name.is_empty()
        || name.len() > 512
        || !name.is_ascii()
        || input.chars().any(char::is_control)
        || name.chars().any(char::is_whitespace)
    {
        return Err(
            "Enter an Ollama model name (up to 512 ASCII characters, no controls or spaces)".into(),
        );
    }
    let parts: Vec<&str> = name.split('/').collect();
    if parts.len() > 3 {
        return Err(
            "Use model[:tag], namespace/model[:tag], or registry/namespace/model[:tag]".into(),
        );
    }
    for (index, part) in parts.iter().enumerate() {
        if index == parts.len() - 1 {
            let mut tagged = part.split(':');
            if !name_part(tagged.next().unwrap_or_default())
                || tagged.next().is_some_and(|tag| !name_part(tag))
                || tagged.next().is_some()
            {
                return Err("Invalid model name or tag".into());
            }
        } else if index == 0 && part.contains(':') {
            let (host, port) = part.split_once(':').ok_or("Invalid registry")?;
            if !name_part(host) || port.parse::<u16>().ok().filter(|p| *p > 0).is_none() {
                return Err("Invalid registry hostname or port".into());
            }
        } else if !name_part(part) {
            return Err("Invalid model namespace".into());
        }
    }
    Ok(name.into())
}

fn validate_filename(name: &str) -> Result<(), String> {
    if name.len() > 255
        || name.len() <= 5
        || name.trim() != name
        || !name.to_ascii_lowercase().ends_with(".gguf")
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || name.contains("..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._- ".contains(&b))
    {
        return Err("GGUF filenames must start with a letter/digit and contain only letters, digits, spaces, dots, underscores or hyphens (max 255 bytes)".into());
    }
    Ok(())
}

fn selected_files(paths: Vec<PathBuf>) -> Result<Vec<(String, PathBuf)>, String> {
    if paths.is_empty() || paths.len() > MAX_FILES {
        return Err(format!("Select between 1 and {MAX_FILES} GGUF files"));
    }
    let mut names = HashSet::new();
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("GGUF filename must be UTF-8")?
                .to_owned();
            validate_filename(&name)?;
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(format!("Duplicate GGUF filename: {name}"));
            }
            Ok((name, path))
        })
        .collect()
}

fn validate_magic(header: &[u8]) -> Result<(), String> {
    if header.len() < 4 || &header[..4] != b"GGUF" {
        return Err("Selected file does not have GGUF magic".into());
    }
    Ok(())
}

// This RAII lease reserves the daemon before opening the dialog. Its Drop also
// releases the slot on dialog errors, panics and task abortion without any await.
struct JobLease {
    app: AppHandle,
    id: String,
    released: bool,
}
impl JobLease {
    fn reserve(app: &AppHandle, state: &AppState) -> Result<(Self, watch::Receiver<bool>), String> {
        let mut jobs = state
            .model_jobs
            .lock()
            .map_err(|_| "Model job lock poisoned")?;
        if !jobs.is_empty() {
            return Err(
                "Another local model job is running. Cancel it or wait for completion.".into(),
            );
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = watch::channel(false);
        jobs.insert(id.clone(), tx);
        Ok((
            Self {
                app: app.clone(),
                id,
                released: false,
            },
            rx,
        ))
    }
    fn release(&mut self) -> bool {
        if self.released {
            return false;
        }
        let state = self.app.state::<AppState>();
        let mut jobs = state.model_jobs.lock().unwrap_or_else(|e| e.into_inner());
        let was_cancelled = jobs
            .remove(&self.id)
            .map(|tx| {
                let cancelled = *tx.borrow();
                cancelled
            })
            .unwrap_or(true);
        self.released = true;
        was_cancelled
    }
}
impl Drop for JobLease {
    fn drop(&mut self) {
        self.release();
    }
}

struct Reporter {
    app: AppHandle,
    id: String,
    last: Option<Instant>,
}
impl Reporter {
    fn new(app: AppHandle, id: String) -> Self {
        Self {
            app,
            id,
            last: None,
        }
    }
    fn progress(&mut self, status: &str, completed: Option<u64>, total: Option<u64>, force: bool) {
        if !force
            && self
                .last
                .is_some_and(|last| last.elapsed() < EVENT_INTERVAL)
        {
            return;
        }
        self.last = Some(Instant::now());
        let _ = self.app.emit(
            "ollama:progress",
            ModelProgress {
                job_id: self.id.clone(),
                status: display_text(status),
                completed,
                total,
                done: false,
                error: None,
                cancelled: false,
            },
        );
    }
    fn terminal(&self, result: JobResult<()>) {
        let (status, error, cancelled) = match result {
            Ok(()) => ("success", None, false),
            Err(JobError::Cancelled) => ("cancelled", None, true),
            Err(JobError::Failed(error)) => ("error", Some(display_text(&error)), false),
        };
        let _ = self.app.emit(
            "ollama:progress",
            ModelProgress {
                job_id: self.id.clone(),
                status: status.into(),
                completed: None,
                total: None,
                done: true,
                error,
                cancelled,
            },
        );
    }
}

fn display_text(text: &str) -> String {
    text.chars()
        .take(2048)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

async fn cancellation(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

async fn cancellable<F, T, E>(cancel: &mut watch::Receiver<bool>, future: F) -> JobResult<T>
where
    F: Future<Output = Result<T, E>>,
    JobError: From<E>,
{
    tokio::select! {
        biased;
        _ = cancellation(cancel) => Err(JobError::Cancelled),
        value = future => value.map_err(JobError::from),
    }
}

#[derive(Debug, Deserialize)]
struct StatusRecord {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    completed: Option<u64>,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}
impl StatusRecord {
    fn parse(line: &[u8]) -> JobResult<Self> {
        if line.len() > MAX_LINE {
            return Err("Ollama progress line exceeds 64 KiB".into());
        }
        let record: Self = serde_json::from_slice(line)
            .map_err(|e| format!("Invalid Ollama progress JSON: {e}"))?;
        if let Some(error) = &record.error {
            return Err(format!("Ollama: {}", display_text(error)).into());
        }
        if record.status.as_deref().unwrap_or_default().is_empty() {
            return Err("Ollama progress is missing status".into());
        }
        if matches!((record.completed, record.total), (Some(c), Some(t)) if t > 0 && c > t) {
            return Err("Ollama returned invalid byte progress".into());
        }
        Ok(record)
    }
    fn is_success(&self) -> bool {
        self.status.as_deref() == Some("success")
    }
    fn report(&self, reporter: &mut Reporter) {
        let status = self.status.as_deref().unwrap_or_default();
        let label = match &self.digest {
            Some(digest) => format!("{status} · {}", display_text(digest)),
            None => status.into(),
        };
        reporter.progress(&label, self.completed, self.total, false);
    }
}

#[derive(Default)]
struct Ndjson {
    line: Vec<u8>,
    records: usize,
}
impl Ndjson {
    fn fragment(&mut self, fragment: &[u8]) -> JobResult<Option<StatusRecord>> {
        let ended = fragment.last() == Some(&b'\n');
        let bytes = if ended {
            &fragment[..fragment.len() - 1]
        } else {
            fragment
        };
        if self.line.len().saturating_add(bytes.len()) > MAX_LINE {
            return Err("Ollama progress line exceeds 64 KiB".into());
        }
        self.line.extend_from_slice(bytes);
        if !ended {
            return Ok(None);
        }
        self.take_line()
    }
    fn take_line(&mut self) -> JobResult<Option<StatusRecord>> {
        self.records += 1;
        if self.records > MAX_RECORDS {
            return Err("Too many Ollama progress records".into());
        }
        let result = if self.line.iter().all(u8::is_ascii_whitespace) {
            Ok(None)
        } else {
            StatusRecord::parse(&self.line).map(Some)
        };
        self.line.clear();
        result
    }
    fn eof(&mut self) -> JobResult<Option<StatusRecord>> {
        if self.line.is_empty() {
            Ok(None)
        } else {
            self.take_line()
        }
    }
}

async fn http_error(response: Response, cancel: &mut watch::Receiver<bool>) -> JobError {
    let status = response.status();
    let mut body = response.bytes_stream();
    let read = async {
        let mut bytes = Vec::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(JobError::from)?;
            let keep = chunk.len().min(8192 - bytes.len());
            bytes.extend_from_slice(&chunk[..keep]);
            if bytes.len() >= 8192 {
                break;
            }
        }
        Ok::<_, JobError>(display_text(&String::from_utf8_lossy(&bytes)))
    };
    match cancellable(cancel, tokio::time::timeout(Duration::from_secs(10), read)).await {
        Ok(Ok(detail)) => JobError::Failed(format!("Ollama returned HTTP {status}: {detail}")),
        Err(JobError::Cancelled) => JobError::Cancelled,
        _ => JobError::Failed(format!("Ollama returned HTTP {status}")),
    }
}
impl From<tokio::time::error::Elapsed> for JobError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        Self::Failed("Ollama timed out".into())
    }
}

async fn status_stream(
    response: Response,
    cancel: &mut watch::Receiver<bool>,
    reporter: &mut Reporter,
) -> JobResult<()> {
    if !response.status().is_success() {
        return Err(http_error(response, cancel).await);
    }
    let mut body = response.bytes_stream();
    let mut parser = Ndjson::default();
    loop {
        let next = cancellable(cancel, tokio::time::timeout(IDLE_TIMEOUT, body.next())).await?;
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk?;
        for (index, fragment) in chunk.split_inclusive(|b| *b == b'\n').enumerate() {
            if *cancel.borrow() {
                return Err(JobError::Cancelled);
            }
            if let Some(record) = parser.fragment(fragment)? {
                if record.is_success() {
                    return Ok(());
                }
                record.report(reporter);
            }
            // A coalesced HTTP chunk must not monopolize the runtime/event queue.
            if index % 128 == 127 {
                tokio::task::yield_now().await;
            }
        }
        tokio::task::yield_now().await;
    }
    if let Some(record) = parser.eof()? {
        if record.is_success() {
            return Ok(());
        }
        record.report(reporter);
    }
    Err("Ollama closed the progress stream without an explicit success status".into())
}

async fn pull(
    client: Client,
    url: String,
    model: String,
    cancel: &mut watch::Receiver<bool>,
    reporter: &mut Reporter,
) -> JobResult<()> {
    reporter.progress("Запрос загрузки модели…", None, None, true);
    let response = cancellable(
        cancel,
        client
            .post(format!("{url}/api/pull"))
            .json(&json!({ "model": model, "stream": true }))
            .send(),
    )
    .await?;
    status_stream(response, cancel, reporter).await
}

// Async file I/O, fixed-size buffers and per-read cancellation keep multi-GB
// files off the UI thread and out of memory. Hash and upload use the SAME handle.
async fn hash_file(
    file: &mut File,
    name: &str,
    cancel: &mut watch::Receiver<bool>,
    reporter: &mut Reporter,
) -> JobResult<(String, u64)> {
    let metadata = cancellable(cancel, file.metadata()).await?;
    if !metadata.is_file() || metadata.len() < 24 {
        return Err(
            format!("{name}: not a regular GGUF file (header needs at least 24 bytes)").into(),
        );
    }
    let total = metadata.len();
    let mut magic = [0; 4];
    cancellable(cancel, file.read_exact(&mut magic)).await?;
    validate_magic(&magic).map_err(|e| format!("{name}: {e}"))?;
    cancellable(cancel, file.seek(io::SeekFrom::Start(0))).await?;
    let status = format!("SHA-256 · {name}");
    reporter.progress(&status, Some(0), Some(total), true);
    let mut hash = Sha256::new();
    let mut buffer = vec![0; IO_CHUNK];
    let mut completed = 0u64;
    loop {
        let count = cancellable(cancel, file.read(&mut buffer)).await?;
        if count == 0 {
            break;
        }
        completed = completed
            .checked_add(count as u64)
            .ok_or("GGUF size overflow")?;
        if completed > total {
            return Err(
                format!("{name}: file grew while hashing; retry with an unchanged file").into(),
            );
        }
        hash.update(&buffer[..count]);
        reporter.progress(&status, Some(completed), Some(total), false);
        tokio::task::yield_now().await;
    }
    let after = cancellable(cancel, file.metadata()).await?;
    if completed != total
        || after.len() != total
        || metadata.modified().ok() != after.modified().ok()
    {
        return Err(
            format!("{name}: file changed while hashing; retry with an unchanged file").into(),
        );
    }
    cancellable(cancel, file.seek(io::SeekFrom::Start(0))).await?;
    reporter.progress(&status, Some(total), Some(total), true);
    Ok((format!("sha256:{:x}", hash.finalize()), total))
}

struct Upload {
    file: File,
    cancel: watch::Receiver<bool>,
    reporter: Reporter,
    status: String,
    total: u64,
    completed: u64,
    finished: bool,
}

async fn upload_blob(
    client: &Client,
    blob_url: &str,
    file: File,
    total: u64,
    name: &str,
    cancel: &mut watch::Receiver<bool>,
    reporter: &mut Reporter,
) -> JobResult<()> {
    let status = format!("Загрузка GGUF в Ollama · {name}");
    reporter.progress(&status, Some(0), Some(total), true);
    let upload = Upload {
        file,
        cancel: cancel.clone(),
        reporter: Reporter::new(reporter.app.clone(), reporter.id.clone()),
        status,
        total,
        completed: 0,
        finished: false,
    };
    let body = stream::unfold(upload, |mut upload| async move {
        if upload.finished {
            return None;
        }
        if upload.completed == upload.total {
            return None;
        }
        let remaining = (upload.total - upload.completed).min(IO_CHUNK as u64) as usize;
        let mut buffer = vec![0; remaining];
        let read = cancellable(&mut upload.cancel, upload.file.read(&mut buffer)).await;
        let result = match read {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "GGUF file shortened during upload",
            )),
            Ok(count) => {
                buffer.truncate(count);
                upload.completed += count as u64;
                upload.reporter.progress(
                    &upload.status,
                    Some(upload.completed),
                    Some(upload.total),
                    false,
                );
                Ok(buffer)
            }
            Err(JobError::Cancelled) => Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "GGUF upload cancelled",
            )),
            Err(JobError::Failed(error)) => Err(io::Error::other(error)),
        };
        if result.is_err() {
            upload.finished = true;
        }
        Some((result, upload))
    });
    let response = cancellable(
        cancel,
        client
            .post(blob_url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_LENGTH, total)
            .body(reqwest::Body::wrap_stream(body))
            .send(),
    )
    .await?;
    if response.status() != StatusCode::CREATED {
        return Err(http_error(response, cancel).await);
    }
    reporter.progress(
        &format!("GGUF загружен · {name}"),
        Some(total),
        Some(total),
        true,
    );
    Ok(())
}

async fn import(
    client: Client,
    url: String,
    model: String,
    files: Vec<(String, PathBuf)>,
    cancel: &mut watch::Receiver<bool>,
    reporter: &mut Reporter,
) -> JobResult<()> {
    let mut mapping = BTreeMap::new();
    for (name, path) in files {
        let mut file = cancellable(cancel, File::open(path)).await?;
        let (digest, total) = hash_file(&mut file, &name, cancel, reporter).await?;
        let blob_url = format!("{url}/api/blobs/{digest}");
        reporter.progress(&format!("Проверка blob · {name}"), None, None, true);
        let response = cancellable(
            cancel,
            client
                .head(&blob_url)
                .timeout(Duration::from_secs(30))
                .send(),
        )
        .await?;
        match response.status() {
            StatusCode::OK => {
                reporter.progress(
                    &format!("Blob уже есть · {name}"),
                    Some(total),
                    Some(total),
                    true,
                );
            }
            StatusCode::NOT_FOUND => {
                drop(response);
                upload_blob(&client, &blob_url, file, total, &name, cancel, reporter).await?;
            }
            _ => return Err(http_error(response, cancel).await),
        }
        mapping.insert(name, digest);
    }
    reporter.progress("Создание модели из GGUF…", None, None, true);
    let response = cancellable(
        cancel,
        client
            .post(format!("{url}/api/create"))
            .json(&json!({ "model": model, "files": mapping, "stream": true }))
            .send(),
    )
    .await?;
    status_stream(response, cancel, reporter).await
}

enum Operation {
    Pull,
    Import(Vec<(String, PathBuf)>),
}
fn launch(
    mut lease: JobLease,
    mut cancel: watch::Receiver<bool>,
    client: Client,
    url: String,
    model: String,
    operation: Operation,
) -> String {
    let id = lease.id.clone();
    tauri::async_runtime::spawn(async move {
        let mut reporter = Reporter::new(lease.app.clone(), lease.id.clone());
        let work = async {
            match operation {
                Operation::Pull => pull(client, url, model, &mut cancel, &mut reporter).await,
                Operation::Import(files) => {
                    import(client, url, model, files, &mut cancel, &mut reporter).await
                }
            }
        };
        let result = match AssertUnwindSafe(tokio::time::timeout(JOB_TIMEOUT, work))
            .catch_unwind()
            .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Local model job exceeded the 48-hour safety timeout".into()),
            Err(_) => Err("Local model job stopped unexpectedly".into()),
        };
        // Drop HTTP/file futures and release the slot BEFORE notifying consumers.
        // Cancel retains the sender until this point, preventing an overlapping job.
        let cancelled = lease.release();
        reporter.terminal(if cancelled {
            Err(JobError::Cancelled)
        } else {
            result
        });
    });
    id
}

#[tauri::command]
pub async fn ollama_pull(
    model: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let model = validate_model(&model)?;
    let url = endpoint(&state)?;
    let client = client()?;
    let (lease, cancel) = JobLease::reserve(&app, &state)?;
    Ok(launch(lease, cancel, client, url, model, Operation::Pull))
}

#[tauri::command]
pub async fn ollama_import(
    model: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let model = validate_model(&model)?;
    let url = endpoint(&state)?;
    let client = client()?;
    let (lease, cancel) = JobLease::reserve(&app, &state)?;
    let selection = rfd::AsyncFileDialog::new()
        .set_title("Import GGUF model — select all split shards")
        .add_filter("GGUF model", &["gguf"])
        .pick_files()
        .await;
    let Some(selection) = selection else {
        return Ok(None);
    };
    let files = selected_files(
        selection
            .into_iter()
            .map(|file| file.path().to_owned())
            .collect(),
    )?;
    Ok(Some(launch(
        lease,
        cancel,
        client,
        url,
        model,
        Operation::Import(files),
    )))
}

#[tauri::command]
pub fn ollama_cancel(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let jobs = state
        .model_jobs
        .lock()
        .map_err(|_| "Model job lock poisoned")?;
    if let Some(sender) = jobs.get(&id) {
        sender.send_replace(true);
    }
    // Idempotent for completion/cancel races. Never remove until the worker exits.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_allow_registry_namespace_tag_but_not_paths() {
        for name in [
            "llama3",
            "qwen2.5:7b-instruct-q4_K_M",
            "library/model:latest",
            "registry.example:5000/team/model:Q4_0",
        ] {
            assert_eq!(validate_model(name).unwrap(), name);
        }
        for name in [
            "",
            "../model",
            "model/../x",
            "a//b",
            "a/b/c/d",
            "https://host/model",
            "x:tag:extra",
            "model\n",
            "a\\b",
            "a?b",
            "registry:0/team/model",
            "a:bad tag",
            "x:",
            ".hidden",
        ] {
            assert!(validate_model(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn file_names_preserve_shards_and_reject_collisions() {
        let names = ["model-00001-of-00002.gguf", "model-00002-of-00002.gguf"];
        let selected = selected_files(names.iter().map(PathBuf::from).collect()).unwrap();
        assert_eq!(selected[0].0, names[0]);
        assert_eq!(selected[1].0, names[1]);
        assert!(selected_files(vec!["A.gguf".into(), "a.gguf".into()]).is_err());
        for name in [
            "../a.gguf",
            "a:bad.gguf",
            "a\n.gguf",
            "a.txt",
            ".gguf",
            " a.gguf",
            "a..b.gguf",
        ] {
            assert!(validate_filename(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn parser_requires_explicit_success_and_propagates_errors() {
        assert!(StatusRecord::parse(br#"{"status":"success"}"#)
            .unwrap()
            .is_success());
        assert!(
            !StatusRecord::parse(br#"{"status":"writing manifest","done":true}"#)
                .unwrap()
                .is_success()
        );
        assert!(matches!(
            StatusRecord::parse(br#"{"status":"success","error":"broken"}"#),
            Err(JobError::Failed(_))
        ));
        for line in [
            br#"{}"#.as_slice(),
            br#"{"status":"x","total":1,"completed":2}"#,
            br#"{"status":"x","completed":-1}"#,
            b"invalid",
        ] {
            assert!(StatusRecord::parse(line).is_err());
        }
        let progress = StatusRecord::parse(br#"{"status":"pulling","digest":"sha256:abc","total":6000000000,"completed":3000000000}"#).unwrap();
        assert_eq!(progress.completed, Some(3_000_000_000));
        assert_eq!(progress.total, Some(6_000_000_000));
        assert_eq!(progress.digest.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn ndjson_handles_chunks_crlf_final_line_and_bounds() {
        let mut parser = Ndjson::default();
        assert!(parser.fragment(b"{\"status\":\"pull").unwrap().is_none());
        assert_eq!(
            parser
                .fragment(b"ing\"}\r\n")
                .unwrap()
                .unwrap()
                .status
                .as_deref(),
            Some("pulling")
        );
        assert!(parser.fragment(b"\n").unwrap().is_none());
        assert!(parser
            .fragment(br#"{"status":"success"}"#)
            .unwrap()
            .is_none());
        assert!(parser.eof().unwrap().unwrap().is_success());
        assert!(Ndjson::default().eof().unwrap().is_none());
        let mut oversized = Ndjson::default();
        assert!(oversized.fragment(&vec![b' '; MAX_LINE]).is_ok());
        assert!(oversized.fragment(b"x").is_err());
        let mut too_many = Ndjson {
            records: MAX_RECORDS,
            ..Ndjson::default()
        };
        assert!(too_many.fragment(b"\n").is_err());
    }

    #[test]
    fn progress_payload_contract() {
        let payload = ModelProgress {
            job_id: "job".into(),
            status: "pulling".into(),
            completed: Some(3_000_000_000),
            total: Some(6_000_000_000),
            done: false,
            error: None,
            cancelled: false,
        };
        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(value["jobId"], "job");
        assert_eq!(value["completed"], 3_000_000_000u64);
        assert_eq!(value["cancelled"], false);
        assert!(value.get("error").is_none());
    }

    #[test]
    fn magic_and_sha256_known_vector() {
        assert!(validate_magic(b"GGUF\x03\0\0\0").is_ok());
        assert!(validate_magic(b"GGML").is_err());
        assert!(validate_magic(b"GGU").is_err());
        assert_eq!(
            format!("{:x}", Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_pending_read_and_closed_channel() {
        let (tx, mut rx) = watch::channel(false);
        tx.send_replace(true);
        let result = cancellable(&mut rx, std::future::pending::<io::Result<()>>()).await;
        assert!(matches!(result, Err(JobError::Cancelled)));
        let (tx, mut rx) = watch::channel(false);
        drop(tx);
        assert!(matches!(
            cancellable(&mut rx, std::future::pending::<io::Result<()>>()).await,
            Err(JobError::Cancelled)
        ));
    }
}
