use rusqlite::Connection;
use serde::Serialize;
use std::{collections::HashMap, sync::Mutex};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// Immutable origin is installed with the sender, before any approval event is published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteApprovalOrigin {
    pub runtime_id: String,
    pub project_id: String,
    pub session_id: String,
    pub device_id: String,
    pub message_id: String,
}

pub(crate) struct AgentApproval {
    pub remote: Option<RemoteApprovalOrigin>,
    pub sender: tokio::sync::oneshot::Sender<bool>,
}

pub(crate) struct LiveAssistant {
    pub id: String,
    pub content: String,
}

/// Inspect and remove in the same map critical section. A PC response must not
/// consume a remote sender, including after its runtime has stopped/restarted.
pub(crate) fn take_native_approval(
    map: &mut HashMap<String, AgentApproval>,
    id: &str,
) -> Result<AgentApproval, String> {
    let approval = map
        .get(id)
        .ok_or("This agent action is no longer waiting for approval")?;
    if approval.remote.is_some() {
        return Err("This approval belongs to the remote device".into());
    }
    Ok(map
        .remove(id)
        .expect("approval inspected under the same lock"))
}

pub struct AppState {
    pub project: Mutex<Option<Project>>,
    pub db: Mutex<Connection>,
    pub generations: Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>,
    pub(crate) live_assistants: Mutex<HashMap<String, LiveAssistant>>,
    pub model_jobs: Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>,
    pub bridge: Mutex<Option<crate::bridge::BridgeHandle>>,
    pub terminals: Mutex<HashMap<String, crate::terminal::TerminalHandle>>,
    pub mcps: Mutex<HashMap<String, crate::mcp::McpHandle>>,
    /// Lock BEFORE root lookup, and hold through process/generation handle insertion.
    /// Also guards root-scoped DB operations and the entire project transition.
    /// Lock order: project_gate -> project -> db/maps; never hold across .await.
    /// Systems terminal_create and mcp_connect MUST acquire this gate.
    pub project_gate: Mutex<()>,
    /// An opened capability directory prevents symlink TOCTOU escape in file IO.
    pub project_dir: Mutex<Option<cap_std::fs::Dir>>,
    /// Lock order for approvals: project_gate -> originating runtime.inner -> this map.
    /// Native decisions use project_gate -> this map and never consult the current bridge.
    pub(crate) agent_approvals: Mutex<HashMap<String, AgentApproval>>,
    /// Active Windows dictation child; at most one recognition session at a time.
    pub voice: Mutex<Option<std::process::Child>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_approval_origin_security_regression() {
        let mut approvals = HashMap::new();
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        let origin = RemoteApprovalOrigin {
            runtime_id: "stopped-runtime".into(),
            project_id: "p".into(),
            session_id: "s".into(),
            device_id: "phone".into(),
            message_id: "old-turn".into(),
        };
        approvals.insert(
            "remote".into(),
            AgentApproval {
                remote: Some(origin.clone()),
                sender,
            },
        );
        for _ in 0..2 {
            assert!(take_native_approval(&mut approvals, "remote").is_err());
        }
        assert_eq!(approvals["remote"].remote.as_ref(), Some(&origin));
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        approvals
            .remove("remote")
            .unwrap()
            .sender
            .send(false)
            .unwrap();
        assert!(!receiver.await.unwrap());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        approvals.insert(
            "native".into(),
            AgentApproval {
                remote: None,
                sender,
            },
        );
        take_native_approval(&mut approvals, "native")
            .unwrap()
            .sender
            .send(true)
            .unwrap();
        assert!(receiver.await.unwrap());
        assert!(approvals.is_empty());
    }
}

impl AppState {
    pub fn new(db: Connection) -> Self {
        Self {
            project: Mutex::new(None),
            db: Mutex::new(db),
            generations: Mutex::new(HashMap::new()),
            live_assistants: Mutex::new(HashMap::new()),
            model_jobs: Mutex::new(HashMap::new()),
            bridge: Mutex::new(None),
            terminals: Mutex::new(HashMap::new()),
            mcps: Mutex::new(HashMap::new()),
            project_gate: Mutex::new(()),
            project_dir: Mutex::new(None),
            agent_approvals: Mutex::new(HashMap::new()),
            voice: Mutex::new(None),
        }
    }
}
