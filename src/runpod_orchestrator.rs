//! `RunPod` orchestrator.
//!
//! High-level REST orchestration for managing pods with automatic reconciliation.
//!
//! This module provides:
//! - `ensure_ready_pod()`: Get a ready-to-use pod (create, start, or reuse as needed)
//! - `PodLease`: Handle to a running pod with connection helpers
//!
//! The orchestrator uses the REST API to:
//! - List pods and filter by name
//! - Check pod compatibility (image, ports, GPU)
//! - Start stopped pods or create new ones
//! - Wait for network readiness (publicIp + portMappings)

use std::{collections::HashMap, env, fmt, time::Duration};

use serde::Deserialize;

use crate::runpod_provisioner::{CreatedPod, RunpodProvisionConfig, RunpodProvisioner};

/// Configuration for the `RunPod` orchestrator.
#[derive(Clone, Debug)]
pub struct RunpodOrchestratorConfig {
    /// `RunPod` API key for authentication.
    /// Env: `RUNPOD_API_KEY` (required)
    pub api_key: String,

    /// REST API URL for `RunPod`.
    /// Env: `RUNPOD_REST_URL` (default: "<https://rest.runpod.io/v1>")
    pub rest_url: String,

    /// Pod name to find or create.
    /// Env: `RUNPOD_POD_NAME` (default: "halldyll-pod")
    pub pod_name: String,

    /// Container image name.
    /// Env: `RUNPOD_IMAGE_NAME` (required)
    pub image_name: String,

    /// Required ports (comma-separated).
    /// Env: `RUNPOD_PORTS` (default: "22/tcp,8888/http")
    pub required_ports: Vec<String>,

    /// GPU type IDs.
    /// Env: `RUNPOD_GPU_TYPE_IDS` (default: "NVIDIA A40")
    pub gpu_type_ids: Vec<String>,

    /// HTTP request timeout in milliseconds.
    /// Env: `RUNPOD_HTTP_TIMEOUT_MS` (default: 30000)
    pub timeout_ms: u64,

    /// Maximum time to wait for pod readiness in milliseconds.
    /// Env: `RUNPOD_READY_TIMEOUT_MS` (default: 300000 = 5 minutes)
    pub ready_timeout_ms: u64,

    /// Poll interval for readiness checks in milliseconds.
    /// Env: `RUNPOD_POLL_INTERVAL_MS` (default: 5000)
    pub poll_interval_ms: u64,

    /// Reconcile mode when pod exists.
    /// Env: `RUNPOD_RECONCILE_MODE` (default: "reuse")
    /// Options: "reuse", "recreate"
    pub reconcile_mode: ReconcileMode,
}

/// Mode for reconciling existing pods.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ReconcileMode {
    /// Reuse compatible existing pods.
    #[default]
    Reuse,
    /// Always recreate pods.
    Recreate,
}

impl RunpodOrchestratorConfig {
    /// Load configuration from environment variables.
    ///
    /// # Errors
    ///
    /// Returns an error if required environment variables are missing or invalid.
    pub fn from_env() -> Result<Self, OrchestratorError> {
        let _ = dotenvy::dotenv();

        let reconcile_mode = env::var("RUNPOD_RECONCILE_MODE").map_or(ReconcileMode::Reuse, |v| {
            if v.to_lowercase() == "recreate" {
                ReconcileMode::Recreate
            } else {
                ReconcileMode::Reuse
            }
        });

        Ok(Self {
            api_key: must_env("RUNPOD_API_KEY")?,
            rest_url: env::var("RUNPOD_REST_URL")
                .unwrap_or_else(|_| "https://rest.runpod.io/v1".to_string()),
            pod_name: env::var("RUNPOD_POD_NAME")
                .unwrap_or_else(|_| "halldyll-pod".to_string()),
            image_name: must_env("RUNPOD_IMAGE_NAME")?,
            required_ports: split_csv_env("RUNPOD_PORTS", "22/tcp,8888/http"),
            gpu_type_ids: split_csv_env("RUNPOD_GPU_TYPE_IDS", "NVIDIA A40"),
            timeout_ms: parse_u64_env("RUNPOD_HTTP_TIMEOUT_MS", 30_000)?,
            ready_timeout_ms: parse_u64_env("RUNPOD_READY_TIMEOUT_MS", 300_000)?,
            poll_interval_ms: parse_u64_env("RUNPOD_POLL_INTERVAL_MS", 5_000)?,
            reconcile_mode,
        })
    }
}

/// Handle to a running pod with connection helpers.
#[derive(Debug, Clone)]
pub struct PodLease {
    /// Pod ID.
    pub id: String,
    /// Pod name.
    pub name: String,
    /// Public IP address.
    pub public_ip: String,
    /// Port mappings (container port -> public port).
    pub port_mappings: HashMap<u16, u16>,
    /// Desired status.
    pub desired_status: String,
}

impl PodLease {
    /// Get the SSH endpoint (IP, port).
    ///
    /// Returns `None` if SSH port (22) is not mapped.
    #[must_use]
    pub fn ssh_endpoint(&self) -> Option<(&str, u16)> {
        self.port_mappings
            .get(&22)
            .map(|port| (self.public_ip.as_str(), *port))
    }

    /// Get the HTTP endpoint URL for a given container port.
    ///
    /// Returns `None` if the port is not mapped.
    #[must_use]
    pub fn http_endpoint(&self, container_port: u16) -> Option<String> {
        self.port_mappings
            .get(&container_port)
            .map(|public_port| format!("http://{}:{}", self.public_ip, public_port))
    }

    /// Get the Jupyter endpoint URL (port 8888).
    ///
    /// Returns `None` if Jupyter port is not mapped.
    #[must_use]
    pub fn jupyter_endpoint(&self) -> Option<String> {
        self.http_endpoint(8888)
    }

    /// Get raw TCP endpoint for a given container port.
    ///
    /// Returns `None` if the port is not mapped.
    #[must_use]
    pub fn tcp_endpoint(&self, container_port: u16) -> Option<(String, u16)> {
        self.port_mappings
            .get(&container_port)
            .map(|public_port| (self.public_ip.clone(), *public_port))
    }
}

/// `RunPod` orchestrator for high-level pod management.
pub struct RunpodOrchestrator {
    cfg: RunpodOrchestratorConfig,
    http: reqwest::Client,
}

impl RunpodOrchestrator {
    /// Create a new orchestrator from the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(cfg: RunpodOrchestratorConfig) -> Result<Self, OrchestratorError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .build()
            .map_err(OrchestratorError::Http)?;

        Ok(Self { cfg, http })
    }

    /// Get a reference to the current configuration.
    #[must_use]
    pub const fn config(&self) -> &RunpodOrchestratorConfig {
        &self.cfg
    }

    /// Ensure a ready pod is available.
    ///
    /// This method will:
    /// 1. List existing pods and find one matching the configured name
    /// 2. If found and compatible, start it if needed
    /// 3. If not found or incompatible, create a new pod
    /// 4. Wait for the pod to be ready (publicIp + required ports)
    ///
    /// Returns a `PodLease` with connection helpers.
    ///
    /// # Errors
    ///
    /// Returns an error if pod creation, starting, or readiness checks fail.
    pub async fn ensure_ready_pod(&self) -> Result<PodLease, OrchestratorError> {
        // Step 1: Find existing pod by name
        let existing = self.find_pod_by_name(&self.cfg.pod_name).await?;

        let pod_id = match existing {
            Some(pod) if self.is_compatible(&pod) && self.cfg.reconcile_mode == ReconcileMode::Reuse => {
                // Pod exists and is compatible
                if pod.desiredStatus.as_deref() == Some("EXITED") {
                    // Start the stopped pod
                    self.start_pod(&pod.id).await?;
                }
                pod.id
            }
            Some(pod) if self.cfg.reconcile_mode == ReconcileMode::Recreate => {
                // Terminate and recreate
                let _ = self.terminate_pod(&pod.id).await;
                self.create_new_pod().await?.id
            }
            Some(_) | None => {
                // Create new pod
                self.create_new_pod().await?.id
            }
        };

        // Step 2: Wait for readiness
        self.wait_for_ready(&pod_id).await
    }

    /// List all pods for the current user.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the API returns an error.
    pub async fn list_pods(&self) -> Result<Vec<PodInfo>, OrchestratorError> {
        let url = format!("{}/pods", self.cfg.rest_url.trim_end_matches('/'));

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.cfg.api_key)
            .send()
            .await
            .map_err(OrchestratorError::Http)?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(OrchestratorError::Api { status, body });
        }

        let pods: Vec<PodInfo> = serde_json::from_str(&body)
            .map_err(|e| OrchestratorError::Json(e.to_string()))?;

        Ok(pods)
    }

    /// Find a pod by name.
    async fn find_pod_by_name(&self, name: &str) -> Result<Option<PodInfo>, OrchestratorError> {
        let pods = self.list_pods().await?;
        Ok(pods.into_iter().find(|p| p.name.as_deref() == Some(name)))
    }

    /// Check if a pod is compatible with the current configuration.
    fn is_compatible(&self, pod: &PodInfo) -> bool {
        // Check image
        if pod.imageName.as_deref() != Some(&self.cfg.image_name) {
            return false;
        }

        // Check if not terminated
        if pod.desiredStatus.as_deref() == Some("TERMINATED") {
            return false;
        }

        true
    }

    /// Start a stopped pod.
    async fn start_pod(&self, pod_id: &str) -> Result<(), OrchestratorError> {
        let url = format!(
            "{}/pods/{}/start",
            self.cfg.rest_url.trim_end_matches('/'),
            pod_id
        );

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.cfg.api_key)
            .send()
            .await
            .map_err(OrchestratorError::Http)?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OrchestratorError::Api { status, body });
        }

        Ok(())
    }

    /// Terminate a pod.
    async fn terminate_pod(&self, pod_id: &str) -> Result<(), OrchestratorError> {
        let url = format!(
            "{}/pods/{}",
            self.cfg.rest_url.trim_end_matches('/'),
            pod_id
        );

        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&self.cfg.api_key)
            .send()
            .await
            .map_err(OrchestratorError::Http)?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OrchestratorError::Api { status, body });
        }

        Ok(())
    }

    /// Create a new pod using the provisioner.
    async fn create_new_pod(&self) -> Result<CreatedPod, OrchestratorError> {
        let provision_cfg = RunpodProvisionConfig::from_env()
            .map_err(|e| OrchestratorError::Provision(e.to_string()))?;

        let provisioner = RunpodProvisioner::new(provision_cfg)
            .map_err(|e| OrchestratorError::Provision(e.to_string()))?;

        provisioner
            .create_pod()
            .await
            .map_err(|e| OrchestratorError::Provision(e.to_string()))
    }

    /// Get detailed pod information.
    async fn get_pod(&self, pod_id: &str) -> Result<Option<PodDetails>, OrchestratorError> {
        let url = format!(
            "{}/pods/{}",
            self.cfg.rest_url.trim_end_matches('/'),
            pod_id
        );

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.cfg.api_key)
            .send()
            .await
            .map_err(OrchestratorError::Http)?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status.as_u16() == 404 {
            return Ok(None);
        }

        if !status.is_success() {
            return Err(OrchestratorError::Api { status, body });
        }

        let pod: PodDetails = serde_json::from_str(&body)
            .map_err(|e| OrchestratorError::Json(e.to_string()))?;

        Ok(Some(pod))
    }

    /// Wait for a pod to be ready (has publicIp and required port mappings).
    async fn wait_for_ready(&self, pod_id: &str) -> Result<PodLease, OrchestratorError> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(self.cfg.ready_timeout_ms);
        let poll_interval = Duration::from_millis(self.cfg.poll_interval_ms);

        loop {
            if start.elapsed() > timeout {
                return Err(OrchestratorError::Timeout);
            }

            if let Some(pod) = self.get_pod(pod_id).await? {
                // Check if running
                if pod.desiredStatus.as_deref() != Some("RUNNING") {
                    tokio::time::sleep(poll_interval).await;
                    continue;
                }

                // Check for public IP
                let public_ip = match &pod.publicIp {
                    Some(ip) if !ip.is_empty() => ip.clone(),
                    _ => {
                        tokio::time::sleep(poll_interval).await;
                        continue;
                    }
                };

                // Build port mappings
                let mut port_mappings = HashMap::new();
                if let Some(mappings) = &pod.portMappings {
                    for (container_port_str, public_port) in mappings {
                        if let Ok(container_port) = container_port_str.parse::<u16>() {
                            port_mappings.insert(container_port, *public_port);
                        }
                    }
                }

                // Check if required ports are mapped
                let has_required_ports = self.cfg.required_ports.iter().all(|port_spec| {
                    // Parse "22/tcp" or "8888/http"
                    if let Some(port_str) = port_spec.split('/').next()
                        && let Ok(port) = port_str.parse::<u16>()
                    {
                        return port_mappings.contains_key(&port);
                    }
                    false
                });

                if !has_required_ports {
                    tokio::time::sleep(poll_interval).await;
                    continue;
                }

                // Pod is ready!
                return Ok(PodLease {
                    id: pod.id,
                    name: pod.name.unwrap_or_default(),
                    public_ip,
                    port_mappings,
                    desired_status: pod.desiredStatus.unwrap_or_default(),
                });
            }
            return Err(OrchestratorError::PodNotFound(pod_id.to_string()));
        }
    }
}

// ============================================================================
// Response types
// ============================================================================

/// Basic pod information from list endpoint.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct PodInfo {
    /// Pod ID.
    pub id: String,
    /// Pod name.
    pub name: Option<String>,
    /// Desired status.
    pub desiredStatus: Option<String>,
    /// Image name.
    pub imageName: Option<String>,
    /// Machine ID.
    pub machineId: Option<String>,
}

/// Detailed pod information.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct PodDetails {
    /// Pod ID.
    pub id: String,
    /// Pod name.
    pub name: Option<String>,
    /// Desired status.
    pub desiredStatus: Option<String>,
    /// Image name.
    pub imageName: Option<String>,
    /// Public IP address.
    pub publicIp: Option<String>,
    /// Port mappings (container port string -> public port).
    pub portMappings: Option<HashMap<String, u16>>,
    /// Exposed ports.
    pub ports: Option<Vec<String>>,
}

// ============================================================================
// Error type
// ============================================================================

/// Error type for orchestrator operations.
#[derive(Debug)]
pub enum OrchestratorError {
    /// Missing required environment variable.
    MissingEnv(&'static str),
    /// Invalid environment variable value.
    InvalidEnv {
        /// The environment variable key.
        key: &'static str,
        /// The reason for invalidity.
        reason: &'static str,
    },
    /// HTTP client error.
    Http(reqwest::Error),
    /// JSON parsing error.
    Json(String),
    /// API error response.
    Api {
        /// HTTP status code.
        status: reqwest::StatusCode,
        /// Response body.
        body: String,
    },
    /// Provisioning error.
    Provision(String),
    /// Pod not found.
    PodNotFound(String),
    /// Timeout waiting for pod readiness.
    Timeout,
}

impl fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnv(k) => write!(f, "missing required env var: {k}"),
            Self::InvalidEnv { key, reason } => write!(f, "invalid env var {key}: {reason}"),
            Self::Http(e) => write!(f, "http error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
            Self::Api { status, body } => write!(f, "api error: status={status}, body={body}"),
            Self::Provision(e) => write!(f, "provisioning error: {e}"),
            Self::PodNotFound(id) => write!(f, "pod not found: {id}"),
            Self::Timeout => write!(f, "timeout waiting for pod readiness"),
        }
    }
}

impl std::error::Error for OrchestratorError {}

// ============================================================================
// Helper functions
// ============================================================================

fn must_env(key: &'static str) -> Result<String, OrchestratorError> {
    env::var(key).map_err(|_| OrchestratorError::MissingEnv(key))
}

fn parse_u64_env(key: &'static str, default: u64) -> Result<u64, OrchestratorError> {
    env::var(key).map_or_else(
        |_| Ok(default),
        |v| {
            v.parse::<u64>().map_err(|_| OrchestratorError::InvalidEnv {
                key,
                reason: "expected an unsigned integer",
            })
        },
    )
}

fn split_csv_env(key: &'static str, default: &str) -> Vec<String> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
