//! `RunPod` state management.
//!
//! Unique responsibility: centralize and persist the management state of a `RunPod` Pod,
//! then produce an idempotent action plan (Create/Start/Stop/Terminate/Noop)
//! based on remote observations (desiredStatus) and local target.
//!
//! Non-goals:
//! - Call the `RunPod` API (done by runpod_starter.rs / runpod_provisioner.rs).
//! - Describe the full creation "spec" (image, env, volumes, etc.).
//!
//! This module is intentionally "boring" and strict:
//! - A serializable state (JSON) stored locally,
//! - A small state machine,
//! - Deterministic decisions.
//!
//! Why: without PodId persistence and reconciliation, your orchestrator
//! ends up recreating pods, losing IDs, or paying for forgotten resources.
//!
//! Expected integration:
//! 1) Load state (`JsonFileStateStore`)
//! 2) Observe remote (Find Pod by ID / List Pods filtered)
//! 3) state.reconcile(observation, now_ms) => `PlannedAction`
//! 4) Execute action in runpod_* (starter/provisioner)
//! 5) state.apply_result(...) then save

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// State file format version.
const STATE_FORMAT_VERSION: u32 = 1;

/// `RunPod` desired status (reflects `desiredStatus` from API).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PodDesiredStatus {
    /// Pod is running.
    Running,
    /// Pod has exited (stopped).
    Exited,
    /// Pod has been terminated.
    Terminated,
}

impl PodDesiredStatus {
    /// Check if the status is terminal (pod no longer exists).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Terminated)
    }
}

/// Local target: what your orchestrator wants to achieve on `RunPod`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TargetStatus {
    /// Pod should be running.
    #[default]
    Running,
    /// Pod should be exited (stopped but preserving storage).
    Exited,
    /// Pod should be terminated (deleted).
    Terminated,
}

/// Newtype for `PodId` (avoids confusion with arbitrary strings).
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PodId(String);

impl PodId {
    /// Create a new `PodId` from a string.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Get the string representation of the `PodId`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PodId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PodId").field(&self.0).finish()
    }
}

impl fmt::Display for PodId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Minimal snapshot of remote pod state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePodSnapshot {
    /// Pod ID.
    pub id: PodId,
    /// Pod name.
    pub name: String,
    /// Desired status of the pod.
    pub desired_status: PodDesiredStatus,
    /// Timestamp (ms since epoch) when this snapshot was observed.
    pub observed_at_ms: u64,
}

/// Remote observation result (from "get pod" / "find by id").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteObservation {
    /// Pod found with the given snapshot.
    Found(RemotePodSnapshot),
    /// Pod not found (deleted/terminated, or invalid ID).
    NotFound,
    /// Transient or unknown error (timeout, network, 5xx).
    Unknown,
}

/// Planned actions to take on a pod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAction {
    /// No operation needed.
    Noop,
    /// Create a new Pod.
    CreatePod {
        /// Pod name.
        name: String,
    },
    /// Start a Pod.
    StartPod {
        /// Pod ID.
        id: PodId,
    },
    /// Stop a Pod.
    StopPod {
        /// Pod ID.
        id: PodId,
    },
    /// Terminate a Pod.
    TerminatePod {
        /// Pod ID.
        id: PodId,
    },
}

/// Local policy for state management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatePolicy {
    /// If true: when Pod is EXITED, prefer `StartPod` over recreating.
    pub reuse_exited_pod: bool,
    /// If set: if Pod remains EXITED beyond this duration, plan `TerminatePod`.
    /// Useful to limit storage costs if you forget to clean up.
    pub auto_terminate_after_exited_ms: Option<u64>,
}

impl Default for StatePolicy {
    fn default() -> Self {
        Self {
            reuse_exited_pod: true,
            auto_terminate_after_exited_ms: None,
        }
    }
}

/// Persistent pod state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunPodState {
    /// Format version for state serialization.
    pub format_version: u32,
    /// Logical pod name (stable). E.g., "Halldyll-Agent".
    pub pod_name: String,
    /// Current `PodId` (if known).
    pub pod_id: Option<PodId>,
    /// Desired target state (Running/Exited/Terminated).
    pub target: TargetStatus,
    /// Last known remote snapshot.
    pub last_remote: Option<RemotePodSnapshot>,
    /// Last local update timestamp (ms).
    pub last_updated_ms: u64,
    /// Local policy.
    pub policy: StatePolicy,
}

impl RunPodState {
    /// Create a new initial state.
    #[must_use]
    pub fn new(pod_name: impl Into<String>, now_ms: u64) -> Self {
        Self {
            format_version: STATE_FORMAT_VERSION,
            pod_name: pod_name.into(),
            pod_id: None,
            target: TargetStatus::default(),
            last_remote: None,
            last_updated_ms: now_ms,
            policy: StatePolicy::default(),
        }
    }

    /// Set the local target state.
    pub const fn set_target(&mut self, target: TargetStatus, now_ms: u64) {
        self.target = target;
        self.last_updated_ms = now_ms;
    }

    /// Get the current `PodId` (if known).
    #[must_use]
    pub const fn pod_id(&self) -> Option<&PodId> {
        self.pod_id.as_ref()
    }

    /// Update state from a remote observation and produce an action plan.
    ///
    /// Key property: idempotence.
    /// - If remote == target, returns Noop.
    /// - If remote is inconsistent (`NotFound`/Terminated) and target != Terminated, returns `CreatePod`.
    ///
    /// Notes:
    /// - `NotFound` is treated as absence: if you want Running/Exited, you must recreate.
    /// - The decision of *how* to create is delegated to the provisioner.
    pub fn reconcile(&mut self, observation: RemoteObservation, now_ms: u64) -> PlannedAction {
        self.last_updated_ms = now_ms;

        // 1) Assimilate remote observation
        let remote_status_opt: Option<PodDesiredStatus> = match observation {
            RemoteObservation::Found(snapshot) => {
                self.pod_id = Some(snapshot.id.clone());
                self.last_remote = Some(snapshot.clone());
                Some(snapshot.desired_status)
            }
            RemoteObservation::NotFound => {
                // Pod likely deleted/terminated on RunPod side.
                self.last_remote = None;
                None
            }
            RemoteObservation::Unknown => {
                // Don't break local state on transient failures.
                // Keep last_remote as is.
                self.last_remote.as_ref().map(|s| s.desired_status)
            }
        };

        // 2) Apply policy (e.g., auto-terminate if EXITED too long)
        if let (Some(policy_ms), Some(remote)) =
            (self.policy.auto_terminate_after_exited_ms, self.last_remote.as_ref())
            && remote.desired_status == PodDesiredStatus::Exited
        {
            let elapsed = now_ms.saturating_sub(remote.observed_at_ms);
            if elapsed >= policy_ms {
                // Policy overrides target: force Terminated to cut costs.
                self.target = TargetStatus::Terminated;
            }
        }

        // 3) Decide action
        match (self.target, remote_status_opt, self.pod_id.clone()) {
            // --- Cases: Noop ---
            (TargetStatus::Terminated, None | Some(PodDesiredStatus::Terminated), _)
            | (TargetStatus::Running, Some(PodDesiredStatus::Running), _)
            | (TargetStatus::Exited, Some(PodDesiredStatus::Exited), _) => PlannedAction::Noop,

            // --- Cases: CreatePod ---
            (TargetStatus::Running | TargetStatus::Exited, None | Some(PodDesiredStatus::Terminated), _)
            | (_, Some(_), None) => PlannedAction::CreatePod {
                name: self.pod_name.clone(),
            },

            // --- Cases: StartPod or CreatePod ---
            (TargetStatus::Running, Some(PodDesiredStatus::Exited), Some(id)) => {
                if self.policy.reuse_exited_pod {
                    PlannedAction::StartPod { id }
                } else {
                    PlannedAction::CreatePod {
                        name: self.pod_name.clone(),
                    }
                }
            }

            // --- Cases: StopPod ---
            (TargetStatus::Exited, Some(PodDesiredStatus::Running), Some(id)) => {
                PlannedAction::StopPod { id }
            }

            // --- Cases: TerminatePod ---
            (TargetStatus::Terminated,
             Some(PodDesiredStatus::Running | PodDesiredStatus::Exited), Some(id)) => {
                PlannedAction::TerminatePod { id }
            }
        }
    }

    /// Call after a successful creation.
    pub fn apply_created(&mut self, id: PodId, now_ms: u64) {
        self.pod_id = Some(id);
        self.last_updated_ms = now_ms;
        // last_remote will be populated by the next observation (reconcile).
    }

    /// Call after a successful termination (or to "forget" the `PodId`).
    pub fn apply_terminated(&mut self, now_ms: u64) {
        self.pod_id = None;
        self.last_remote = None;
        self.last_updated_ms = now_ms;
    }
}

/// Errors for state store operations.
#[derive(Debug)]
pub enum StateStoreError {
    /// I/O error.
    Io(io::Error),
    /// Serialization error.
    Serde(serde_json::Error),
    /// Invalid state.
    InvalidState(&'static str),
}

impl fmt::Display for StateStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Serde(e) => write!(f, "serde error: {e}"),
            Self::InvalidState(msg) => write!(f, "invalid state: {msg}"),
        }
    }
}

impl std::error::Error for StateStoreError {}

impl From<io::Error> for StateStoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for StateStoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde(value)
    }
}

/// Trait for persisting pod state.
pub trait StateStore {
    /// Load the state from storage.
    ///
    /// # Errors
    ///
    /// Returns an error if loading fails (I/O, parsing, or validation).
    fn load(&self) -> Result<Option<RunPodState>, StateStoreError>;
    /// Save the state to storage.
    ///
    /// # Errors
    ///
    /// Returns an error if saving fails (I/O, serialization, or validation).
    fn save(&self, state: &RunPodState) -> Result<(), StateStoreError>;
}

/// File-based JSON state store with safe atomic writes.
#[derive(Debug, Clone)]
pub struct JsonFileStateStore {
    path: PathBuf,
}

impl JsonFileStateStore {
    /// Create a new JSON file state store.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Get the path to the state file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the default path from environment or fallback.
    ///
    /// Env: `RUNPOD_STATE_PATH` (default: `.runpod_state.json`)
    #[must_use]
    pub fn default_path() -> PathBuf {
        if let Some(p) = std::env::var_os("RUNPOD_STATE_PATH") {
            return PathBuf::from(p);
        }
        PathBuf::from(".runpod_state.json")
    }

    fn ensure_parent_dir(&self) -> Result<(), io::Error> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}

impl StateStore for JsonFileStateStore {
    fn load(&self) -> Result<Option<RunPodState>, StateStoreError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path)?;
        let state: RunPodState = serde_json::from_slice(&bytes)?;
        if state.format_version != STATE_FORMAT_VERSION {
            return Err(StateStoreError::InvalidState(
                "unsupported state format version",
            ));
        }
        if state.pod_name.trim().is_empty() {
            return Err(StateStoreError::InvalidState("pod_name is empty"));
        }
        Ok(Some(state))
    }

    fn save(&self, state: &RunPodState) -> Result<(), StateStoreError> {
        if state.format_version != STATE_FORMAT_VERSION {
            return Err(StateStoreError::InvalidState("wrong state format version"));
        }
        if state.pod_name.trim().is_empty() {
            return Err(StateStoreError::InvalidState("pod_name is empty"));
        }

        self.ensure_parent_dir()?;

        // Write to temp file in same directory for atomic rename.
        let mut tmp = self.path.clone();
        let tmp_name = format!(
            ".{}.tmp",
            self.path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("runpod_state")
        );
        tmp.set_file_name(tmp_name);

        let json = serde_json::to_vec_pretty(state)?;

        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }

        // Best-effort atomic replace (cross-platform pragmatic).
        if self.path.exists() {
            // On Windows, rename over existing can fail; remove first.
            let _ = fs::remove_file(&self.path);
        }
        fs::rename(&tmp, &self.path)?;

        Ok(())
    }
}

/// Utility: current timestamp in milliseconds since UNIX epoch.
#[must_use]
pub fn now_unix_ms() -> u64 {
    let Ok(dur) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return 0;
    };
    u64::try_from(dur.as_millis()).unwrap_or(u64::MAX)
}
