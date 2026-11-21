use crate::state::AppState;
use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStatus {
    pub available: bool,
    pub engine: String,
    pub running: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceText {
    text: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceError {
    message: String,
}

fn powershell() -> Result<std::path::PathBuf, String> {
    #[cfg(windows)]
    {
        let system = std::env::var_os("SystemRoot").ok_or("SystemRoot is unavailable")?;
        let shell =
            std::path::PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        if shell.is_file() {
            Ok(shell)
        } else {
            Err("Windows PowerShell is not installed".into())
        }
    }
    #[cfg(not(windows))]
    {
        Err("Native voice input is implemented for Windows only".into())
    }
}

fn validate_language(language: &str) -> Result<(), String> {
    let bytes = language.as_bytes();
    if bytes.len() == 5
        && bytes[..2].iter().all(u8::is_ascii_lowercase)
        && bytes[2] == b'-'
        && bytes[3..].iter().all(u8::is_ascii_uppercase)
    {
        Ok(())
    } else {
        Err("Language must look like ru-RU or en-US".into())
    }
}

#[tauri::command]
pub fn voice_status(state: State<'_, AppState>) -> Result<VoiceStatus, String> {
    let running = state
        .voice
        .lock()
        .map_err(|_| "Voice lock poisoned")?
        .is_some();
    Ok(match powershell() {
        Ok(_) => VoiceStatus {
            available: true,
            engine: "Windows Speech (System.Speech)".into(),
            running,
        },
        Err(error) => VoiceStatus {
            available: false,
            engine: error,
            running,
        },
    })
}

pub(crate) fn stop_locked(state: &AppState) {
    if let Ok(mut slot) = state.voice.lock() {
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[tauri::command]
pub fn voice_stop(state: State<'_, AppState>) -> Result<(), String> {
    stop_locked(&state);
    Ok(())
}

#[tauri::command]
pub fn voice_start(
    language: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate_language(&language)?;
    let shell = powershell()?;
    stop_locked(&state);
    // DictationGrammar recognises free speech; each finished utterance is one stdout line.
    let script = format!(
        "$ErrorActionPreference='Stop';[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;\
         try{{Add-Type -AssemblyName System.Speech;\
         $engine=New-Object System.Speech.Recognition.SpeechRecognitionEngine([System.Globalization.CultureInfo]::GetCultureInfo('{language}'));\
         $engine.SetInputToDefaultAudioDevice();\
         $engine.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar()));\
         $engine.EndSilenceDuration=[TimeSpan]::FromMilliseconds(650);\
         [Console]::Out.WriteLine('READY');\
         while($true){{$r=$engine.Recognize();if($null -ne $r -and $r.Text){{[Console]::Out.WriteLine($r.Text)}}}}}}\
         catch{{[Console]::Out.WriteLine('ERROR: '+$_.Exception.Message);exit 1}}"
    );
    let mut command = Command::new(shell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start voice recognition: {e}"))?;
    let stdout = child.stdout.take().ok_or("Voice stdout is unavailable")?;
    *state.voice.lock().map_err(|_| "Voice lock poisoned")? = Some(child);
    std::thread::Builder::new()
        .name("notcode-voice".into())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let line = line.trim().to_owned();
                if line.is_empty() {
                    continue;
                }
                if line == "READY" {
                    let _ = app.emit(
                        "voice:ready",
                        VoiceText {
                            text: String::new(),
                        },
                    );
                } else if let Some(message) = line.strip_prefix("ERROR: ") {
                    let _ = app.emit(
                        "voice:error",
                        VoiceError {
                            message: message.chars().take(500).collect(),
                        },
                    );
                    break;
                } else {
                    let _ = app.emit(
                        "voice:text",
                        VoiceText {
                            text: line.chars().take(4096).collect(),
                        },
                    );
                }
            }
            let state = app.state::<AppState>();
            stop_locked(&state);
            let _ = app.emit(
                "voice:stopped",
                VoiceText {
                    text: String::new(),
                },
            );
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_are_validated() {
        assert!(validate_language("ru-RU").is_ok());
        assert!(validate_language("en-US").is_ok());
        for bad in [
            "",
            "russian",
            "ru_RU",
            "ru-ru",
            "RU-RU",
            "ru-RUX",
            "ru-RU; rm",
        ] {
            assert!(validate_language(bad).is_err(), "{bad}");
        }
    }
}
