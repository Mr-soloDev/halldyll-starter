//! `RunPod` pod provisioner (create).
//!
//! Unique responsibility: create a new Pod via `RunPod` REST API.
//!
//! REST endpoint:
//! - POST <https://rest.runpod.io/v1/pods>
//!
//! All configuration is loaded from environment variables, making the provisioner
//! fully configurable without code changes.

use std::{collections::HashMap, env, fmt, time::Duration};

use serde::{Deserialize, Serialize};

/// Configuration for provisioning a new `RunPod` pod.
///
/// All fields can be configured via environment variables.
/// See `from_env()` for the mapping.
#[derive(Clone, Debug)]
pub struct RunpodProvisionConfig {
    /// `RunPod` API key for authentication.
    /// Env: `RUNPOD_API_KEY` (required)
    pub api_key: String,

    /// REST API URL for `RunPod`.
    /// Env: `RUNPOD_REST_URL` (default: "<https://rest.runpod.io/v1>")
    pub rest_url: String,

    /// Pod name.
    /// Env: `RUNPOD_POD_NAME` (default: "halldyll-pod")
    pub name: String,

    /// Cloud type ("SECURE" | "COMMUNITY").
    /// Env: `RUNPOD_CLOUD_TYPE` (default: "SECURE")
    pub cloud_type: String,

    /// Compute type ("GPU" | "CPU").
    /// Env: `RUNPOD_COMPUTE_TYPE` (default: "GPU")
    pub compute_type: String,

    /// Container image name.
    /// Env: `RUNPOD_IMAGE_NAME` (required)
    pub image_name: String,

    /// Number of GPUs.
    /// Env: `RUNPOD_GPU_COUNT` (default: 1)
    pub gpu_count: u32,

    /// GPU type IDs (comma-separated).
    /// Env: `RUNPOD_GPU_TYPE_IDS` (default: "NVIDIA A40")
    /// Examples: "NVIDIA A40", "NVIDIA `GeForce` RTX 4090", "NVIDIA RTX 5090"
    pub gpu_type_ids: Vec<String>,

    /// Container disk size in GB.
    /// Env: `RUNPOD_CONTAINER_DISK_GB` (default: 50)
    pub container_disk_gb: u32,

    /// Volume size in GB.
    /// Env: `RUNPOD_VOLUME_GB` (default: 20)
    pub volume_gb: u32,

    /// Volume mount path.
    /// Env: `RUNPOD_VOLUME_MOUNT_PATH` (default: "/workspace")
    pub volume_mount_path: String,

    /// Exposed ports (comma-separated).
    /// Env: `RUNPOD_PORTS` (default: "22/tcp,8888/http")
    /// Format: "<port>/<protocol>" where protocol is "tcp" or "http"
    pub ports: Vec<String>,

    /// Optional network volume ID for shared persistent storage.
    /// Env: `RUNPOD_NETWORK_VOLUME_ID` (optional)
    pub network_volume_id: Option<String>,

    /// HTTP request timeout in milliseconds.
    /// Env: `RUNPOD_HTTP_TIMEOUT_MS` (default: 15000)
    pub timeout_ms: u64,

    /// Whether to start Jupyter on pod creation.
    /// Env: `RUNPOD_START_JUPYTER` (default: false)
    pub start_jupyter: bool,

    /// Whether to start SSH on pod creation.
    /// Env: `RUNPOD_START_SSH` (default: true)
    pub start_ssh: bool,

    /// Additional environment variables for the pod (JSON object string).
    /// Env: `RUNPOD_POD_ENV` (optional, JSON format: {"KEY": "value"})
    pub pod_env: HashMap<String, String>,
}

impl RunpodProvisionConfig {
    /// Load configuration from environment variables.
    ///
    /// Required environment variables:
    /// - `RUNPOD_API_KEY`: Your `RunPod` API key
    /// - `RUNPOD_IMAGE_NAME`: Container image to use
    ///
    /// Optional environment variables (with defaults):
    /// - `RUNPOD_REST_URL`: REST API URL (default: "<https://rest.runpod.io/v1>")
    /// - `RUNPOD_POD_NAME`: Pod name (default: "halldyll-pod")
    /// - `RUNPOD_CLOUD_TYPE`: "SECURE" or "COMMUNITY" (default: "SECURE")
    /// - `RUNPOD_COMPUTE_TYPE`: "GPU" or "CPU" (default: "GPU")
    /// - `RUNPOD_GPU_COUNT`: Number of GPUs (default: 1)
    /// - `RUNPOD_GPU_TYPE_IDS`: Comma-separated GPU types (default: "NVIDIA A40")
    /// - `RUNPOD_CONTAINER_DISK_GB`: Container disk size (default: 50)
    /// - `RUNPOD_VOLUME_GB`: Volume size (default: 20)
    /// - `RUNPOD_VOLUME_MOUNT_PATH`: Mount path (default: "/workspace")
    /// - `RUNPOD_PORTS`: Comma-separated ports (default: "22/tcp,8888/http")
    /// - `RUNPOD_NETWORK_VOLUME_ID`: Network volume ID (optional)
    /// - `RUNPOD_HTTP_TIMEOUT_MS`: HTTP timeout (default: 15000)
    /// - `RUNPOD_START_JUPYTER`: Start Jupyter (default: false)
    /// - `RUNPOD_START_SSH`: Start SSH (default: true)
    /// - `RUNPOD_POD_ENV`: Additional pod env vars as JSON (optional)
    ///
    /// # Errors
    ///
    /// Returns an error if required environment variables are missing or invalid.
    pub fn from_env() -> Result<Self, RunpodError> {
        let _ = dotenvy::dotenv();

        let pod_env = parse_json_env("RUNPOD_POD_ENV")?;

        Ok(Self {
            api_key: must_env("RUNPOD_API_KEY")?,
            rest_url: env::var("RUNPOD_REST_URL")
                .unwrap_or_else(|_| "https://rest.runpod.io/v1".to_string()),

            name: env::var("RUNPOD_POD_NAME")
                .unwrap_or_else(|_| "halldyll-pod".to_string()),
            cloud_type: env::var("RUNPOD_CLOUD_TYPE")
                .unwrap_or_else(|_| "SECURE".to_string()),
            compute_type: env::var("RUNPOD_COMPUTE_TYPE")
                .unwrap_or_else(|_| "GPU".to_string()),
            image_name: must_env("RUNPOD_IMAGE_NAME")?,

            gpu_count: parse_u32_env("RUNPOD_GPU_COUNT", 1)?,
            gpu_type_ids: split_csv_env("RUNPOD_GPU_TYPE_IDS", "NVIDIA A40"),

            container_disk_gb: parse_u32_env("RUNPOD_CONTAINER_DISK_GB", 50)?,
            volume_gb: parse_u32_env("RUNPOD_VOLUME_GB", 20)?,
            volume_mount_path: env::var("RUNPOD_VOLUME_MOUNT_PATH")
                .unwrap_or_else(|_| "/workspace".to_string()),
            ports: split_csv_env("RUNPOD_PORTS", "22/tcp,8888/http"),

            network_volume_id: env::var("RUNPOD_NETWORK_VOLUME_ID")
                .ok()
                .filter(|s| !s.trim().is_empty()),

            timeout_ms: parse_u64_env("RUNPOD_HTTP_TIMEOUT_MS", 15_000)?,

            start_jupyter: parse_bool_env("RUNPOD_START_JUPYTER", false),
            start_ssh: parse_bool_env("RUNPOD_START_SSH", true),

            pod_env,
        })
    }
}

/// Provisioner for creating new `RunPod` pods.
pub struct RunpodProvisioner {
    cfg: RunpodProvisionConfig,
    http: reqwest::Client,
}

impl RunpodProvisioner {
    /// Create a new `RunPod` provisioner from the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(cfg: RunpodProvisionConfig) -> Result<Self, RunpodError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .build()
            .map_err(RunpodError::Http)?;

        Ok(Self { cfg, http })
    }

    /// Create a new Pod and return its newly assigned podId.
    ///
    /// Uses the configuration loaded from environment variables.
    /// The pod will be created with the specified GPU type, count, image, etc.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the API returns an error.
    pub async fn create_pod(&self) -> Result<CreatedPod, RunpodError> {
        let url = format!("{}/pods", self.cfg.rest_url.trim_end_matches('/'));

        let req_body = CreatePodRequest {
            cloudType: self.cfg.cloud_type.clone(),
            computeType: self.cfg.compute_type.clone(),
            name: self.cfg.name.clone(),
            imageName: self.cfg.image_name.clone(),
            gpuCount: self.cfg.gpu_count,
            gpuTypeIds: self.cfg.gpu_type_ids.clone(),
            containerDiskInGb: self.cfg.container_disk_gb,
            volumeInGb: self.cfg.volume_gb,
            volumeMountPath: self.cfg.volume_mount_path.clone(),
            ports: self.cfg.ports.clone(),
            env: self.cfg.pod_env.clone(),
            networkVolumeId: self.cfg.network_volume_id.clone(),
            startJupyter: self.cfg.start_jupyter,
            startSsh: self.cfg.start_ssh,
        };

        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.cfg.api_key)
            .json(&req_body)
            .send()
            .await
            .map_err(RunpodError::Http)?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(RunpodError::Api { status, body });
        }

        let created: CreatePodResponse =
            serde_json::from_str(&body).map_err(|e| RunpodError::Json { source: e, body })?;

        Ok(CreatedPod {
            id: created.id,
            desired_status: created.desiredStatus,
            public_ip: created.publicIp,
        })
    }

    /// Get a reference to the current configuration.
    #[must_use]
    pub const fn config(&self) -> &RunpodProvisionConfig {
        &self.cfg
    }
}

#[derive(Debug, Serialize)]
#[allow(non_snake_case)]
struct CreatePodRequest {
    cloudType: String,
    computeType: String,
    name: String,
    imageName: String,
    gpuCount: u32,
    gpuTypeIds: Vec<String>,
    containerDiskInGb: u32,
    volumeInGb: u32,
    volumeMountPath: String,
    ports: Vec<String>,
    env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    networkVolumeId: Option<String>,
    startJupyter: bool,
    startSsh: bool,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct CreatePodResponse {
    id: String,
    #[serde(default)]
    desiredStatus: Option<String>,
    #[serde(default)]
    publicIp: Option<String>,
}

/// Represents a newly created pod.
#[derive(Debug, Clone)]
pub struct CreatedPod {
    /// Pod ID assigned by `RunPod`.
    pub id: String,
    /// Desired status of the pod.
    pub desired_status: Option<String>,
    /// Public IP address (if available).
    pub public_ip: Option<String>,
}

/// Error type for `RunPod` provisioning operations.
#[derive(Debug)]
pub enum RunpodError {
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
    /// JSON deserialization error.
    Json {
        /// The JSON parsing error.
        source: serde_json::Error,
        /// The response body.
        #[allow(dead_code)]
        body: String,
    },
    /// API error response.
    Api {
        /// HTTP status code.
        status: reqwest::StatusCode,
        /// Response body.
        body: String,
    },
}

impl fmt::Display for RunpodError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnv(k) => write!(f, "missing required env var: {k}"),
            Self::InvalidEnv { key, reason } => write!(f, "invalid env var {key}: {reason}"),
            Self::Http(e) => write!(f, "http error: {e}"),
            Self::Json { source, .. } => write!(f, "json decode error: {source}"),
            Self::Api { status, body } => {
                write!(f, "runpod api error: status={status}, body={body}")
            }
        }
    }
}

impl std::error::Error for RunpodError {}

fn must_env(key: &'static str) -> Result<String, RunpodError> {
    env::var(key).map_err(|_| RunpodError::MissingEnv(key))
}

fn parse_u32_env(key: &'static str, default: u32) -> Result<u32, RunpodError> {
    env::var(key).map_or_else(
        |_| Ok(default),
        |v| {
            v.parse::<u32>().map_err(|_| RunpodError::InvalidEnv {
                key,
                reason: "expected an unsigned integer",
            })
        },
    )
}

fn parse_u64_env(key: &'static str, default: u64) -> Result<u64, RunpodError> {
    env::var(key).map_or_else(
        |_| Ok(default),
        |v| {
            v.parse::<u64>().map_err(|_| RunpodError::InvalidEnv {
                key,
                reason: "expected an unsigned integer",
            })
        },
    )
}

fn parse_bool_env(key: &'static str, default: bool) -> bool {
    env::var(key).map_or(default, |v| {
        matches!(v.to_lowercase().as_str(), "true" | "1" | "yes")
    })
}

fn split_csv_env(key: &'static str, default: &str) -> Vec<String> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_json_env(key: &'static str) -> Result<HashMap<String, String>, RunpodError> {
    env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map_or_else(
            || Ok(HashMap::new()),
            |v| {
                serde_json::from_str(&v).map_err(|_| RunpodError::InvalidEnv {
                    key,
                    reason: "expected valid JSON object",
                })
            },
        )
}
