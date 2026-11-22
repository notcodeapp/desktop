use crate::state::AppState;
use serde::Serialize;
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::Manager;

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const RUN_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFile {
    path: String,
    status: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    branch: String,
    files: Vec<GitFile>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLogEntry {
    hash: String,
    summary: String,
    author: String,
    date: String,
}

struct Output {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
impl Output {
    fn checked(self) -> Result<Vec<u8>, String> {
        if self.success {
            Ok(self.stdout)
        } else {
            let detail = String::from_utf8_lossy(&self.stderr)
                .chars()
                .take(4096)
                .collect::<String>();
            Err(if detail.trim().is_empty() {
                "Git command failed".into()
            } else {
                detail
            })
        }
    }
}

fn inherit_runtime(command: &mut Command, root: &Path) {
    command.env_clear();
    // Deliberately exclude inherited GIT_*, shell startup settings and token variables.
    for key in [
        "SystemRoot",
        "WINDIR",
        "PATHEXT",
        "TEMP",
        "TMP",
        "TMPDIR",
        "HOME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "XDG_CONFIG_HOME",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let directories = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|p| p.is_absolute())
                .filter_map(|p| p.canonicalize().ok())
                .filter(|p| !p.starts_with(root))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    command.env(
        "PATH",
        std::env::join_paths(directories)
            .map_err(|e| e.to_string())
            .unwrap_or_default(),
    );
    command
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "")
        .env("PAGER", "")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_ALLOW_PROTOCOL", "https:http:ssh:git")
        .env("GIT_ATTR_NOSYSTEM", "1");
    // Never accept user SSH ProxyCommand/LocalCommand or custom Git SSH executables.
    // HTTPS credential helpers are likewise disabled below; credentials are not stored here.
    #[cfg(windows)]
    command.env(
        "GIT_SSH_COMMAND",
        "ssh -F NUL -oBatchMode=yes -oPermitLocalCommand=no -oProxyCommand=none",
    );
    #[cfg(not(windows))]
    command.env(
        "GIT_SSH_COMMAND",
        "ssh -F /dev/null -oBatchMode=yes -oPermitLocalCommand=no -oProxyCommand=none",
    );
}

fn find_git(exclude: Option<&Path>) -> Result<PathBuf, String> {
    let path = std::env::var_os("PATH").ok_or("PATH is unavailable")?;
    for directory in std::env::split_paths(&path) {
        if !directory.is_absolute() {
            continue;
        }
        let canonical = match directory.canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if exclude.is_some_and(|exclude| canonical.starts_with(exclude)) {
            continue;
        }
        let candidate = canonical.join(if cfg!(windows) { "git.exe" } else { "git" });
        if candidate.is_file() {
            let candidate = candidate.canonicalize().map_err(|e| e.to_string())?;
            if !exclude.is_some_and(|exclude| candidate.starts_with(exclude)) {
                return Ok(candidate);
            }
        }
    }
    Err("Git executable was not found outside the project; install Git and add it to PATH".into())
}

fn git_executable(root: &Path) -> Result<PathBuf, String> {
    find_git(Some(root))
}

fn collect<R: Read + Send + 'static>(
    reader: R,
    overflow: Arc<AtomicBool>,
) -> Result<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    thread::Builder::new()
        .name("notcode-git-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = reader
                .take((OUTPUT_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())
                .and_then(|_| {
                    if bytes.len() > OUTPUT_LIMIT {
                        overflow.store(true, Ordering::Release);
                        Err("Git output exceeds 4 MiB".into())
                    } else {
                        Ok(bytes)
                    }
                });
            let _ = sender.send(result);
        })
        .map_err(|e| e.to_string())?;
    Ok(receiver)
}

struct Repository {
    root: PathBuf,
    executable: PathBuf,
    hooks: PathBuf,
    overrides: Vec<String>,
    config_env: Vec<(String, String)>,
}
impl Repository {
    fn raw_opts(&self, args: &[&str], allow_credentials: bool) -> Result<Output, String> {
        let mut command = Command::new(&self.executable);
        inherit_runtime(&mut command, &self.root);
        command
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Session-scoped config travels in the environment, never in argv or repo config,
        // so the worker bearer token is not visible in process listings or .git/config.
        if !self.config_env.is_empty() {
            command.env("GIT_CONFIG_COUNT", self.config_env.len().to_string());
            for (index, (key, value)) in self.config_env.iter().enumerate() {
                command.env(format!("GIT_CONFIG_KEY_{index}"), key);
                command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
            }
        }
        command
            .args(["--no-pager", "--literal-pathspecs", "-c"])
            .arg(format!("core.hooksPath={}", self.hooks.to_string_lossy()));
        for setting in [
            "core.fsmonitor=false",
            "core.untrackedCache=false",
            "ssh.variant=ssh",
            "core.pager=",
            "core.askPass=",
            "core.gitProxy=",
            "core.alternateRefsCommand=",
            "diff.external=",
            "interactive.diffFilter=",
            "commit.gpgSign=false",
            "tag.gpgSign=false",
            "gpg.program=",
            "gpg.ssh.program=",
            "submodule.recurse=false",
            "fetch.recurseSubmodules=false",
            "maintenance.auto=false",
            "gc.auto=0",
            "protocol.allow=never",
            "protocol.https.allow=always",
            "protocol.http.allow=always",
            "protocol.ssh.allow=always",
            "protocol.git.allow=always",
            "protocol.file.allow=never",
            "protocol.ext.allow=never",
        ] {
            command.arg("-c").arg(setting);
        }
        if !allow_credentials {
            // Read-only flows never invoke credential helpers.
            command
                .arg("-c")
                .arg("credential.helper=")
                .arg("-c")
                .arg("credential.interactive=false");
        }
        for setting in &self.overrides {
            command.arg("-c").arg(setting);
        }
        command.args(args);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Cannot start Git: {e}"))?;
        let overflow = Arc::new(AtomicBool::new(false));
        let pipes = (|| {
            let stdout = collect(
                child.stdout.take().ok_or("Missing Git stdout")?,
                overflow.clone(),
            )?;
            let stderr = collect(
                child.stderr.take().ok_or("Missing Git stderr")?,
                overflow.clone(),
            )?;
            Ok::<_, String>((stdout, stderr))
        })();
        let (stdout, stderr) = match pipes {
            Ok(p) => p,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let start = Instant::now();
        let success = loop {
            if overflow.load(Ordering::Acquire) || start.elapsed() > RUN_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(if overflow.load(Ordering::Acquire) {
                    "Git output exceeds 4 MiB"
                } else {
                    "Git timed out after 90 seconds"
                }
                .into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) => thread::sleep(Duration::from_millis(15)),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e.to_string());
                }
            }
        };
        // A helper retaining a pipe must not block the application indefinitely.
        let stdout = stdout
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "Git stdout did not close")??;
        let stderr = stderr
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "Git stderr did not close")??;
        Ok(Output {
            success,
            stdout,
            stderr,
        })
    }
    fn raw(&self, args: &[&str]) -> Result<Output, String> {
        self.raw_opts(args, false)
    }
    /// Push/pull may use the system credential helper (Git Credential Manager).
    fn raw_creds(&self, args: &[&str]) -> Result<Output, String> {
        self.raw_opts(args, true)
    }
    fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        self.raw(args)?.checked()
    }
    fn text(&self, args: &[&str]) -> Result<String, String> {
        Ok(String::from_utf8_lossy(&self.run(args)?).into_owned())
    }
    fn open(root: PathBuf) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let executable = git_executable(&root)?;
        let hooks =
            std::env::temp_dir().join(format!("notcode-disabled-hooks-{}", uuid::Uuid::new_v4()));
        let mut repo = Self {
            root,
            executable,
            hooks,
            overrides: Vec::new(),
            config_env: Vec::new(),
        };
        let top = repo.text(&["rev-parse", "--show-toplevel"])?;
        let top = PathBuf::from(top.trim_end_matches(['\r', '\n']))
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if top != repo.root || repo.text(&["rev-parse", "--is-bare-repository"])?.trim() != "false"
        {
            return Err(
                "Select the repository working-tree root, not a subdirectory or bare repository"
                    .into(),
            );
        }
        // Merely disabling hooks is insufficient: add/checkout can run arbitrary
        // clean/smudge/process filters configured in local or included config files.
        let keys = repo.run(&["config", "--null", "--name-only", "--list"])?;
        for key in keys.split(|b| *b == 0).filter(|k| !k.is_empty()) {
            let key = std::str::from_utf8(key).map_err(|_| "Git configuration key is not UTF-8")?;
            let lower = key.to_ascii_lowercase();
            if lower.starts_with("filter.") {
                if lower.ends_with(".required") {
                    repo.overrides.push(format!("{key}=false"));
                } else if [".clean", ".smudge", ".process"]
                    .iter()
                    .any(|suffix| lower.ends_with(suffix))
                {
                    repo.overrides.push(format!("{key}="));
                }
            }
            if lower.starts_with("remote.") {
                if lower.ends_with(".vcs") {
                    return Err("Custom remote-helper transports are disabled".into());
                }
                if lower.ends_with(".uploadpack") {
                    repo.overrides.push(format!("{key}=git-upload-pack"));
                }
                if lower.ends_with(".receivepack") {
                    repo.overrides.push(format!("{key}=git-receive-pack"));
                }
            }
        }
        Ok(repo)
    }
}

fn relative_path(input: &str) -> Result<(), String> {
    if input.is_empty()
        || input.len() > 4096
        || input.contains(['\0', '\\', ':'])
        || Path::new(input)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Use a project-relative file path without traversal".into());
    }
    if input.split('/').any(|p| p.eq_ignore_ascii_case(".git")) {
        return Err("Git internals are not a diff target".into());
    }
    Ok(())
}
fn consent(approved: bool) -> Result<(), String> {
    if approved {
        Ok(())
    } else {
        Err(
            "Explicit approval is required; Git may contact configured remotes and integrations"
                .into(),
        )
    }
}
fn parse_status(bytes: &[u8]) -> Result<Vec<GitFile>, String> {
    let mut entries = bytes.split(|b| *b == 0).filter(|p| !p.is_empty());
    let mut files = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 || entry[2] != b' ' {
            return Err("Malformed Git status record".into());
        }
        let status = String::from_utf8_lossy(&entry[..2]).into_owned();
        let path = String::from_utf8_lossy(&entry[3..]).into_owned();
        // -z prints destination first, followed by the original name as a second record.
        if entry[..2].iter().any(|c| *c == b'R' || *c == b'C') {
            entries.next().ok_or("Incomplete Git rename record")?;
        }
        files.push(GitFile { path, status });
    }
    Ok(files)
}

fn git_status_impl(state: &AppState) -> Result<GitStatus, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let repo = Repository::open(crate::project::root(state)?)?;
    let branch = match repo.raw(&["symbolic-ref", "--quiet", "--short", "HEAD"])? {
        Output {
            success: true,
            stdout,
            ..
        } => String::from_utf8_lossy(&stdout).trim().to_owned(),
        _ => "HEAD (detached)".into(),
    };
    let bytes = repo.run(&[
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=normal",
        "--ignore-submodules=all",
    ])?;
    Ok(GitStatus {
        branch,
        files: parse_status(&bytes)?,
    })
}

fn git_diff_impl(path: Option<String>, state: &AppState) -> Result<String, String> {
    if let Some(path) = &path {
        relative_path(path)?;
    }
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let repo = Repository::open(crate::project::root(state)?)?;
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--ignore-submodules=all",
    ];
    let has_head = repo.raw(&["rev-parse", "--verify", "HEAD"])?.success;
    if has_head {
        args.push("HEAD");
    }
    args.push("--");
    if let Some(path) = &path {
        args.push(path);
    }
    let unstaged = repo.text(&args)?;
    if has_head {
        return Ok(unstaged);
    }
    let mut staged_args = vec![
        "diff",
        "--cached",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--ignore-submodules=all",
        "--",
    ];
    if let Some(path) = &path {
        staged_args.push(path);
    }
    let staged = repo.text(&staged_args)?;
    if staged.len() + unstaged.len() > OUTPUT_LIMIT {
        return Err("Diff exceeds 4 MiB".into());
    }
    Ok(format!("{staged}{unstaged}"))
}

fn git_log_impl(state: &AppState) -> Result<Vec<GitLogEntry>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let repo = Repository::open(crate::project::root(state)?)?;
    if !repo.raw(&["rev-parse", "--verify", "HEAD"])?.success {
        return Ok(Vec::new());
    }
    let bytes = repo.run(&[
        "log",
        "-50",
        "-z",
        "--no-show-signature",
        "--format=%H%x00%s%x00%an%x00%aI",
    ])?;
    let mut fields: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if fields.last() == Some(&&b""[..]) {
        fields.pop();
    }
    if fields.len() % 4 != 0 {
        return Err("Malformed Git log records".into());
    }
    Ok(fields
        .chunks_exact(4)
        .map(|f| GitLogEntry {
            hash: String::from_utf8_lossy(f[0]).into_owned(),
            summary: String::from_utf8_lossy(f[1]).into_owned(),
            author: String::from_utf8_lossy(f[2]).into_owned(),
            date: String::from_utf8_lossy(f[3]).into_owned(),
        })
        .collect())
}

fn git_branches_impl(state: &AppState) -> Result<Vec<String>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let repo = Repository::open(crate::project::root(state)?)?;
    Ok(repo
        .text(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"])?
        .lines()
        .map(str::to_owned)
        .collect())
}

fn git_commit_impl(message: String, approved: bool, state: &AppState) -> Result<String, String> {
    consent(approved)?;
    if message.trim().is_empty() || message.len() > 16 * 1024 || message.contains('\0') {
        return Err("Commit message must contain 1–16384 bytes, without NUL".into());
    }
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let repo = Repository::open(crate::project::root(state)?)?;
    // Only explicitly staged changes are committed; staging is a separate user action.
    if repo
        .raw(&["diff", "--cached", "--quiet", "--ignore-submodules=all"])?
        .success
    {
        return Err(
            "Нет подготовленных изменений. Отметьте файлы кнопкой «В индекс» перед коммитом."
                .into(),
        );
    }
    let output = repo.raw(&["commit", "--no-gpg-sign", "--no-verify", "-m", &message])?;
    if !output.success {
        let detail = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(2048)
            .collect::<String>();
        if detail.contains("tell me who you are")
            || detail.contains("user.name")
            || detail.contains("user.email")
        {
            return Err("Git не знает автора коммита. Выполните в терминале: git config user.name \"Ваше имя\" и git config user.email \"ваша@почта\".".into());
        }
        return Err(if detail.trim().is_empty() {
            "Коммит не удался".into()
        } else {
            detail
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// When signed in, GitHub traffic goes through the NotCode worker: it attaches the
/// server-side GitHub token, while this PC only ever sends its own session token.
fn github_proxy_config(repo: &mut Repository) -> Result<bool, String> {
    let origin = match repo.raw(&["remote", "get-url", "origin"])? {
        Output {
            success: true,
            stdout,
            ..
        } => String::from_utf8_lossy(&stdout).trim().to_owned(),
        _ => return Ok(false),
    };
    if !origin.starts_with("https://github.com/") {
        return Ok(false);
    }
    let Some(token) = crate::auth::session_token()? else {
        return Ok(false);
    };
    let proxy = format!("{}/git/", crate::auth::api_base());
    repo.config_env.push((
        format!("url.{proxy}.insteadOf"),
        "https://github.com/".into(),
    ));
    repo.config_env.push((
        format!("http.{proxy}.extraHeader"),
        format!("Authorization: Bearer {token}"),
    ));
    Ok(true)
}

fn git_sync_impl(direction: String, approved: bool, state: &AppState) -> Result<String, String> {
    consent(approved)?;
    let args: &[&str] = match direction.as_str() {
        // -u origin HEAD: публикует и новые ветки (upstream), force не используется.
        "push" => &["push", "--no-verify", "--recurse-submodules=no", "-u", "origin", "HEAD"],
        "pull" => &[
            "pull",
            "--ff-only",
            "--no-rebase",
            "--no-autostash",
            "--no-recurse-submodules",
        ],
        _ => return Err("Direction must be push or pull".into()),
    };
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let mut repo = Repository::open(crate::project::root(state)?)?;
    let proxied = github_proxy_config(&mut repo)?;
    // Proxied sync authenticates with the session header; the credential helper stays off.
    // Direct sync is the only flow allowed to reach the system credential helper.
    let output = if proxied {
        repo.raw(args)?
    } else {
        repo.raw_creds(args)?
    };
    if !output.success {
        let detail = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(2048)
            .collect::<String>();
        if proxied && (detail.contains("401") || detail.contains("403")) {
            return Err(format!("{detail}\nСессия NotCode или доступ к репозиторию недействительны; войдите через GitHub заново."));
        }
        return Err(if detail.trim().is_empty() {
            "Синхронизация не удалась".into()
        } else {
            detail
        });
    }
    // Push/pull progress is commonly on stderr, even on success.
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn git_init_impl(state: &AppState) -> Result<(), String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let root = crate::project::root(state)?;
    let executable = git_executable(&root)?;
    let mut command = Command::new(&executable);
    inherit_runtime(&mut command, &root);
    command
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(["init", "--quiet", "-b", "main"]);
    let output = command
        .output()
        .map_err(|e| format!("Cannot start Git: {e}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(2048)
            .collect::<String>();
        return Err(if detail.trim().is_empty() {
            "Не удалось создать репозиторий".into()
        } else {
            detail
        });
    }
    Ok(())
}

fn git_stage_impl(
    paths: Vec<String>,
    all: bool,
    unstage: bool,
    state: &AppState,
) -> Result<(), String> {
    if !all {
        if paths.is_empty() || paths.len() > 512 {
            return Err("Выберите от 1 до 512 файлов".into());
        }
        for path in &paths {
            relative_path(path)?;
        }
    }
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let repo = Repository::open(crate::project::root(state)?)?;
    let mut args: Vec<&str> = if unstage {
        let has_head = repo.raw(&["rev-parse", "--verify", "HEAD"])?.success;
        if has_head {
            vec!["restore", "--staged", "--"]
        } else {
            vec!["rm", "--cached", "-r", "--quiet", "--"]
        }
    } else {
        vec!["add", "--"]
    };
    if all {
        args.push(".");
    } else {
        args.extend(paths.iter().map(String::as_str));
    }
    repo.run(&args)?;
    Ok(())
}

fn git_remote_impl(state: &AppState) -> Result<Option<String>, String> {
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let repo = Repository::open(crate::project::root(state)?)?;
    Ok(match repo.raw(&["remote", "get-url", "origin"])? {
        Output {
            success: true,
            stdout,
            ..
        } => Some(String::from_utf8_lossy(&stdout).trim().to_owned()),
        _ => None,
    })
}

fn git_checkout_impl(branch: String, create: bool, approved: bool, state: &AppState) -> Result<String, String> {
    consent(approved)?;
    if branch.is_empty()
        || branch.len() > 256
        || branch.starts_with('-')
        || branch.contains(['\0', '\r', '\n'])
    {
        return Err("Choose an existing local branch".into());
    }
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let repo = Repository::open(crate::project::root(state)?)?;
    if create {
        repo.run(&["check-ref-format", "--allow-onelevel", "--branch", &branch])?;
        let exists = repo.raw(&["show-ref", "--verify", &format!("refs/heads/{branch}")])?.success;
        if exists {
            return Err(format!("Ветка {branch} уже существует"));
        }
    } else {
        repo.run(&["check-ref-format", "--branch", &branch])?;
        repo.run(&["show-ref", "--verify", &format!("refs/heads/{branch}")])?;
    }
    // No force, reset, auto-stash, remote guessing, or submodule recursion.
    let mut args = vec![
        "checkout".to_owned(),
        "--no-guess".to_owned(),
        "--no-recurse-submodules".to_owned(),
    ];
    if create {
        args.push("-b".to_owned());
    }
    args.push(branch.clone());
    args.push("--".to_owned());
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = repo.raw(&args_ref)?;
    if !output.success {
        return output
            .checked()
            .map(|b| String::from_utf8_lossy(&b).into_owned());
    }
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

// Command wrappers run all process waits on the blocking pool. No state guard
// crosses the single await below; the closure obtains state on its worker thread.
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
    .map_err(|e| format!("Git worker failed: {e}"))?
}
#[tauri::command]
pub async fn git_status(app: tauri::AppHandle) -> Result<GitStatus, String> {
    blocking(app, git_status_impl).await
}
#[tauri::command]
pub async fn git_diff(path: Option<String>, app: tauri::AppHandle) -> Result<String, String> {
    blocking(app, move |state| git_diff_impl(path, state)).await
}
#[tauri::command]
pub async fn git_log(app: tauri::AppHandle) -> Result<Vec<GitLogEntry>, String> {
    blocking(app, git_log_impl).await
}
#[tauri::command]
pub async fn git_branches(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    blocking(app, git_branches_impl).await
}
#[tauri::command]
pub async fn git_commit(
    message: String,
    approved: bool,
    app: tauri::AppHandle,
) -> Result<String, String> {
    blocking(app, move |state| git_commit_impl(message, approved, state)).await
}
#[tauri::command]
pub async fn git_sync(
    direction: String,
    approved: bool,
    app: tauri::AppHandle,
) -> Result<String, String> {
    blocking(app, move |state| git_sync_impl(direction, approved, state)).await
}
#[tauri::command]
pub async fn git_checkout(
    branch: String,
    create: bool,
    approved: bool,
    app: tauri::AppHandle,
) -> Result<String, String> {
    blocking(app, move |state| git_checkout_impl(branch, create, approved, state)).await
}
#[tauri::command]
pub async fn git_init(app: tauri::AppHandle) -> Result<(), String> {
    blocking(app, git_init_impl).await
}
#[tauri::command]
pub async fn git_stage(
    paths: Vec<String>,
    all: bool,
    unstage: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    blocking(app, move |state| git_stage_impl(paths, all, unstage, state)).await
}
#[tauri::command]
pub async fn git_remote(app: tauri::AppHandle) -> Result<Option<String>, String> {
    blocking(app, git_remote_impl).await
}
#[tauri::command]
pub async fn git_stash(direction: String, approved: bool, app: tauri::AppHandle) -> Result<String, String> {
    blocking(app, move |state| git_stash_impl(direction, approved, state)).await
}

fn git_stash_impl(direction: String, approved: bool, state: &AppState) -> Result<String, String> {
    consent(approved)?;
    let args: &[&str] = match direction.as_str() {
        "push" => &["stash", "push", "--include-untracked", "--no-verify"],
        "pop" => &["stash", "pop", "--quiet"],
        _ => return Err("Direction must be push or pop".into()),
    };
    let _gate = state
        .project_gate
        .lock()
        .map_err(|_| "Project gate poisoned")?;
    let repo = Repository::open(crate::project::root(state)?)?;
    let output = repo.raw(args)?;
    if !output.success {
        let text = output.checked()?;
        return Ok(String::from_utf8_lossy(&text).into_owned());
    }
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

/// Only public HTTPS clones from github.com are supported; no embedded credentials.
fn github_clone_url(input: &str) -> Result<(String, String), String> {
    let url = input.trim();
    if url.len() > 300 {
        return Err("Слишком длинная ссылка".into());
    }
    let rest = url
        .strip_prefix("https://github.com/")
        .ok_or("Поддерживаются ссылки вида https://github.com/<owner>/<repo>")?;
    if !rest
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        || rest.contains("..")
        || rest.contains("//")
    {
        return Err("Некорректная ссылка репозитория".into());
    }
    let trimmed = rest.trim_end_matches('/');
    let mut segments = trimmed.split('/');
    let owner = segments.next().unwrap_or("");
    let mut repo = segments.next().unwrap_or("").to_owned();
    if owner.is_empty() || repo.is_empty() || segments.next().is_some() {
        return Err("Ожидается https://github.com/<owner>/<repo>".into());
    }
    if repo.len() > 100 {
        return Err("Имя репозитория слишком длинное".into());
    }
    if repo.ends_with(".git") {
        repo.truncate(repo.len() - 4);
    }
    Ok((format!("https://github.com/{owner}/{repo}.git"), repo))
}

fn clone_impl(
    state: &AppState,
    url: &str,
    dest_parent: &Path,
    name: &str,
) -> Result<crate::state::Project, String> {
    let dest = dest_parent.join(name);
    if dest.exists() {
        return Err(format!("Папка «{name}» уже существует; выберите другую"));
    }
    // Signed-in users clone private repos through the worker proxy; the GitHub token
    // stays server-side and the finished repo points straight at github.com.
    let session = crate::auth::session_token()?;
    let proxy = format!("{}/git/", crate::auth::api_base());
    let effective_url = match &session {
        Some(_) => format!(
            "{proxy}{}",
            url.strip_prefix("https://github.com/").unwrap_or(url)
        ),
        None => url.to_owned(),
    };
    let executable = find_git(None)?;
    let mut command = Command::new(&executable);
    inherit_runtime(&mut command, dest_parent);
    command
        .current_dir(dest_parent)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for setting in [
        "protocol.allow=never",
        "protocol.https.allow=always",
        "protocol.http.allow=always",
        "protocol.file.allow=never",
        "protocol.ext.allow=never",
        "credential.helper=",
        "credential.interactive=false",
        "core.fsmonitor=false",
        "submodule.recurse=false",
    ] {
        command.arg("-c").arg(setting);
    }
    if let Some(token) = &session {
        command.env("GIT_CONFIG_COUNT", "1");
        command.env("GIT_CONFIG_KEY_0", format!("http.{proxy}.extraHeader"));
        command.env(
            "GIT_CONFIG_VALUE_0",
            format!("Authorization: Bearer {token}"),
        );
    }
    command.args(["clone", &effective_url]).arg(&dest);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start Git: {e}"))?;
    let overflow = Arc::new(AtomicBool::new(false));
    let pipes = (|| {
        let stdout = collect(
            child.stdout.take().ok_or("Missing Git stdout")?,
            overflow.clone(),
        )?;
        let stderr = collect(
            child.stderr.take().ok_or("Missing Git stderr")?,
            overflow.clone(),
        )?;
        Ok::<_, String>((stdout, stderr))
    })();
    let (stdout, stderr) = match pipes {
        Ok(p) => p,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    let start = Instant::now();
    let success = loop {
        if overflow.load(Ordering::Acquire) || start.elapsed() > CLONE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if overflow.load(Ordering::Acquire) {
                "Git output exceeds 4 MiB"
            } else {
                "Клонирование прервано по таймауту (10 минут)"
            }
            .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
        }
    };
    let _stdout = stdout
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Git stdout did not close")??;
    let stderr = stderr
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Git stderr did not close")??;
    if !success {
        let detail = String::from_utf8_lossy(&stderr)
            .chars()
            .take(2048)
            .collect::<String>();
        let hint = if session.is_some() {
            " Проверьте, что аккаунт имеет доступ к репозиторию, и обновите вход через GitHub."
        } else if detail.contains("Authentication")
            || detail.contains("not found")
            || detail.contains("403")
        {
            " Возможно, репозиторий приватный: войдите через GitHub в настройках, чтобы клонировать его."
        } else {
            ""
        };
        // Only the partial clone created by this failed run; dest did not exist before.
        let _ = std::fs::remove_dir_all(&dest);
        return Err(if detail.trim().is_empty() {
            format!("Клонирование не удалось.{hint}")
        } else {
            format!("{}{}", detail.trim_end(), hint)
        });
    }
    if session.is_some() {
        // Never leave the proxy URL (or any token-bearing config) in the cloned repo.
        let mut rewrite = Command::new(&executable);
        inherit_runtime(&mut rewrite, &dest);
        rewrite
            .current_dir(&dest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .args(["remote", "set-url", "origin", url]);
        let status = rewrite
            .status()
            .map_err(|e| format!("Cannot rewrite origin: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_dir_all(&dest);
            return Err("Не удалось настроить origin репозитория".into());
        }
    }
    crate::project::select_project(state, &dest)
}

#[tauri::command]
pub async fn git_clone(
    url: String,
    approved: bool,
    app: tauri::AppHandle,
) -> Result<crate::state::Project, String> {
    consent(approved)?;
    let (clone_url, name) = github_clone_url(&url)?;
    // No state guard is held across the folder picker await.
    let selection = rfd::AsyncFileDialog::new()
        .set_title(format!("Папка для клонирования «{name}»"))
        .pick_folder()
        .await
        .ok_or("Папка не выбрана")?;
    let parent = selection.path().to_path_buf();
    blocking(app, move |state| {
        clone_impl(state, &clone_url, &parent, &name)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_handles_spaces_newlines_and_rename_pair() {
        let files = parse_status(b" M with space\0R  new\nname\0old name\0?? untracked\0").unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].status, " M");
        assert_eq!(files[1].path, "new\nname");
        assert_eq!(files[2].status, "??");
        assert!(parse_status(b"R  destination\0").is_err());
    }
    #[test]
    fn rejects_escaping_and_magic_paths() {
        for path in [
            "",
            "../secret",
            "/tmp/file",
            "a/../b",
            "C:/file",
            "a\\b",
            ":(glob)*",
            ".git/config",
        ] {
            assert!(relative_path(path).is_err(), "{path}");
        }
        assert!(relative_path("src/a file.rs").is_ok());
        assert!(consent(false).is_err());
    }
    #[test]
    fn isolated_repository_disables_hooks_and_clean_filters() {
        let root = std::env::temp_dir().join(format!("notcode-git-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let executable = git_executable(&root).expect("Git is required for this integration test");
        assert!(Command::new(&executable)
            .args(["init", "--quiet"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        let repo = Repository::open(root.clone()).unwrap();
        repo.run(&["config", "user.name", "NotCode Test"]).unwrap();
        repo.run(&["config", "user.email", "test@example.invalid"])
            .unwrap();
        repo.run(&["config", "filter.evil.clean", "notcode-must-not-execute"])
            .unwrap();
        repo.run(&["config", "filter.evil.required", "true"])
            .unwrap();
        std::fs::write(root.join(".gitattributes"), "*.txt filter=evil\n").unwrap();
        std::fs::write(root.join("tracked.txt"), "one\n").unwrap();
        let repo = Repository::open(root.clone()).unwrap();
        repo.run(&["add", "--", ".gitattributes", "tracked.txt"])
            .unwrap();
        repo.run(&["commit", "--no-gpg-sign", "-m", "initial"])
            .unwrap();
        std::fs::write(root.join("untracked.txt"), "never auto-stage").unwrap();
        std::fs::write(root.join("tracked.txt"), "two\n").unwrap();
        repo.run(&["add", "-u", "--", "."]).unwrap();
        let staged = repo.text(&["diff", "--cached", "--name-only"]).unwrap();
        assert!(staged.contains("tracked.txt"));
        assert!(!staged.contains("untracked.txt"));
        assert!(Repository::open(root.join(".git")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
