use crate::state::AppState;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};
use tauri::{Emitter, Manager, State};

const MAX_INPUT: usize = 64 * 1024;

// Consent is represented by possession of this handle, never by a global setting.
// Closing it (including application shutdown) revokes consent and kills the shell.
pub struct TerminalHandle {
    master: Box<dyn MasterPty + Send>,
    writer: std::sync::mpsc::SyncSender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}
impl Drop for TerminalHandle {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    id: String,
    shell: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DataEvent {
    id: String,
    data: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExitEvent {
    id: String,
    code: Option<u32>,
}

fn dimensions(cols: u16, rows: u16) -> Result<PtySize, String> {
    if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
        return Err("Terminal dimensions must be 2–500 columns and 1–300 rows".into());
    }
    Ok(PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    })
}

fn shell_command() -> Result<(String, CommandBuilder), String> {
    #[cfg(windows)]
    {
        let system = std::env::var_os("SystemRoot").ok_or("SystemRoot is unavailable")?;
        let shell =
            std::path::PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        if !shell.is_file() {
            return Err("Windows PowerShell is not installed".into());
        }
        let mut command = CommandBuilder::new(&shell);
        command.args(["-NoLogo", "-NoProfile", "-NoExit"]);
        Ok((shell.to_string_lossy().into_owned(), command))
    }
    #[cfg(not(windows))]
    {
        let shell = if std::path::Path::new("/bin/bash").is_file() {
            "/bin/bash"
        } else {
            "/bin/sh"
        };
        let mut command = CommandBuilder::new(shell);
        if shell.ends_with("bash") {
            command.args(["--noprofile", "--norc"]);
        }
        command.arg("-i");
        command.env("TERM", "xterm-256color");
        Ok((shell.into(), command))
    }
}

// Keep incomplete UTF-8 between PTY reads; replacing each chunk separately corrupts
// multibyte output (notably PowerShell/Cyrillic) at arbitrary read boundaries.
fn decode(bytes: &mut Vec<u8>, eof: bool) -> String {
    let mut text = String::new();
    let mut offset = 0;
    while offset < bytes.len() {
        match std::str::from_utf8(&bytes[offset..]) {
            Ok(valid) => {
                text.push_str(valid);
                offset = bytes.len();
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(std::str::from_utf8(&bytes[offset..offset + valid]).unwrap_or(""));
                offset += valid;
                match error.error_len() {
                    Some(len) => {
                        text.push('\u{fffd}');
                        offset += len;
                    }
                    None if eof => {
                        text.push('\u{fffd}');
                        offset = bytes.len();
                    }
                    None => break,
                }
            }
        }
    }
    bytes.drain(..offset);
    text
}

#[tauri::command]
pub fn terminal_create(
    approved: bool,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<TerminalInfo, String> {
    if !approved {
        return Err("Explicit terminal consent for this project is required".into());
    }
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let root = crate::project::root(&state)?;
    if state
        .terminals
        .lock()
        .map_err(|_| "Terminal lock poisoned")?
        .len()
        >= 8
    {
        return Err("Close a terminal before opening another (limit 8)".into());
    }
    let (shell, mut command) = shell_command()?;
    command.cwd(&root);
    let pair = native_pty_system()
        .openpty(dimensions(100, 30)?)
        .map_err(|e| e.to_string())?;
    // Obtain all fallible IO handles before spawning, so setup failure cannot orphan a child.
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| e.to_string())?;
    drop(pair.slave);
    let killer = child.clone_killer();
    let id = uuid::Uuid::new_v4().to_string();
    let (input_tx, input_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    let handle = TerminalHandle {
        master: pair.master,
        writer: input_tx,
        killer,
    };
    let mut terminals = match state.terminals.lock() {
        Ok(map) => map,
        Err(_) => {
            drop(handle);
            let _ = child.wait();
            return Err("Terminal lock poisoned".into());
        }
    };
    terminals.insert(id.clone(), handle);
    drop(terminals);

    // A separate reader drains the PTY and signals completion before the exit event.
    // The child mutex below is private to the waiter/setup cleanup, never a state lock.
    let (drained_tx, drained_rx) = std::sync::mpsc::channel();
    let read_id = id.clone();
    let read_app = app.clone();
    let reading = thread::Builder::new()
        .name("notcode-pty-reader".into())
        .spawn(move || {
            let mut pending = Vec::with_capacity(8196);
            let mut buffer = [0u8; 8192];
            loop {
                let count = match reader.read(&mut buffer) {
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                pending.extend_from_slice(&buffer[..count]);
                let data = decode(&mut pending, false);
                if !data.is_empty() {
                    let _ = read_app.emit(
                        "terminal:data",
                        DataEvent {
                            id: read_id.clone(),
                            data,
                        },
                    );
                }
            }
            let data = decode(&mut pending, true);
            if !data.is_empty() {
                let _ = read_app.emit("terminal:data", DataEvent { id: read_id, data });
            }
            let _ = drained_tx.send(());
        });
    if let Err(error) = reading {
        if let Ok(mut map) = state.terminals.lock() {
            map.remove(&id);
        }
        let _ = child.wait();
        return Err(error.to_string());
    }
    // Never write to a possibly backpressured PTY while holding the terminal map:
    // close/shutdown must remain able to kill a shell that stopped reading input.
    let write_id = id.clone();
    let write_app = app.clone();
    let writing = thread::Builder::new()
        .name("notcode-pty-writer".into())
        .spawn(move || {
            while let Ok(data) = input_rx.recv() {
                if writer
                    .write_all(&data)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    let state = write_app.state::<AppState>();
                    if let Ok(mut map) = state.terminals.lock() {
                        map.remove(&write_id);
                    }
                    break;
                }
            }
        });
    if let Err(error) = writing {
        if let Ok(mut map) = state.terminals.lock() {
            map.remove(&id);
        }
        let _ = child.wait();
        return Err(error.to_string());
    }
    let wait_id = id.clone();
    let wait_child = Arc::new(Mutex::new(child));
    let waiter = wait_child.clone();
    let waiting = thread::Builder::new()
        .name("notcode-pty-wait".into())
        .spawn(move || {
            let code = waiter
                .lock()
                .ok()
                .and_then(|mut child| child.wait().ok())
                .map(|s| s.exit_code());
            // Descendants may retain the slave; do not delay exit forever in that case.
            let _ = drained_rx.recv_timeout(std::time::Duration::from_millis(300));
            let state = app.state::<AppState>();
            if let Ok(mut map) = state.terminals.lock() {
                map.remove(&wait_id);
            }
            let _ = app.emit("terminal:exit", ExitEvent { id: wait_id, code });
        });
    if let Err(error) = waiting {
        if let Ok(mut map) = state.terminals.lock() {
            map.remove(&id);
        }
        if let Ok(mut child) = wait_child.lock() {
            let _ = child.wait();
        }
        return Err(error.to_string());
    }
    Ok(TerminalInfo { id, shell })
}

#[tauri::command]
pub fn terminal_write(id: String, data: String, state: State<'_, AppState>) -> Result<(), String> {
    if data.is_empty() || data.len() > MAX_INPUT {
        return Err("Terminal input must contain 1–65536 bytes".into());
    }
    let terminals = state
        .terminals
        .lock()
        .map_err(|_| "Terminal lock poisoned")?;
    let terminal = terminals
        .get(&id)
        .ok_or("Terminal is closed or has no consent")?;
    terminal.writer.try_send(data.into_bytes()).map_err(|_| {
        "Terminal input queue is full or closed; wait before sending more input".into()
    })
}

#[tauri::command]
pub fn terminal_resize(
    id: String,
    cols: u16,
    rows: u16,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let size = dimensions(cols, rows)?;
    let terminals = state
        .terminals
        .lock()
        .map_err(|_| "Terminal lock poisoned")?;
    terminals
        .get(&id)
        .ok_or("Terminal is closed")?
        .master
        .resize(size)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn terminal_close(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    // Idempotent: the waiter may already have removed a naturally exited shell.
    let terminal = state
        .terminals
        .lock()
        .map_err(|_| "Terminal lock poisoned")?
        .remove(&id);
    drop(terminal);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resize_is_bounded() {
        for (cols, rows) in [(0, 1), (1, 30), (501, 30), (80, 0), (80, 301)] {
            assert!(dimensions(cols, rows).is_err());
        }
        assert!(dimensions(80, 24).is_ok());
    }
    #[test]
    fn utf8_survives_chunk_boundaries_and_invalid_bytes() {
        let mut pending = vec![0xd0];
        assert_eq!(decode(&mut pending, false), "");
        pending.extend_from_slice(&[0x9f, b'!', 0xff, 0xe2]);
        assert_eq!(decode(&mut pending, false), "П!\u{fffd}");
        assert_eq!(decode(&mut pending, true), "\u{fffd}");
        assert!(pending.is_empty());
    }
}
