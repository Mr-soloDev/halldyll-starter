//! `RunPod` GraphQL client.
//!
//! Unique responsibility: interact with `RunPod` GraphQL API for pod lifecycle operations.
//!
//! API endpoint:
//! - POST <https://api.runpod.io/graphql>
//! - Header: Authorization: Bearer <token>
//!
//! This module encapsulates:
//! - Pod deployment (on-demand and spot)
//! - Pod lifecycle (stop, terminate, resume)
//! - Pod queries (list, get by ID)
//! - GPU type queries
//!
//! All configuration is loaded from environment variables.

use std::{env, fmt, time::Duration};

use serde::{Deserialize, Serialize};

/// Configuration for the `RunPod` GraphQL client.
#[derive(Clone, Debug)]
pub struct RunpodClientConfig {
    /// `RunPod` API key for authentication.
    /// Env: `RUNPOD_API_KEY` (required)
    pub api_key: String,

    /// GraphQL API URL for `RunPod`.
    /// Env: `RUNPOD_GRAPHQL_URL` (default: "<https://api.runpod.io/graphql>")
    pub graphql_url: String,

    /// HTTP request timeout in milliseconds.
    /// Env: `RUNPOD_HTTP_TIMEOUT_MS` (default: 30000)
    pub timeout_ms: u64,

    /// Maximum number of retry attempts.
    /// Env: `RUNPOD_HTTP_RETRY_MAX` (default: 3)
    pub retry_max: u32,

    /// Backoff time between retries in milliseconds.
    /// Env: `RUNPOD_HTTP_RETRY_BACKOFF_MS` (default: 500)
    pub retry_backoff_ms: u64,
}

impl RunpodClientConfig {
    /// Load configuration from environment variables.
    ///
    /// # Errors
    ///
    /// Returns an error if required environment variables are missing or invalid.
    pub fn from_env() -> Result<Self, RunpodClientError> {
        let _ = dotenvy::dotenv();

        Ok(Self {
            api_key: must_env("RUNPOD_API_KEY")?,
            graphql_url: env::var("RUNPOD_GRAPHQL_URL")
                .unwrap_or_else(|_| "https://api.runpod.io/graphql".to_string()),
            timeout_ms: parse_u64_env("RUNPOD_HTTP_TIMEOUT_MS", 30_000)?,
            retry_max: parse_u32_env("RUNPOD_HTTP_RETRY_MAX", 3)?,
            retry_backoff_ms: parse_u64_env("RUNPOD_HTTP_RETRY_BACKOFF_MS", 500)?,
        })
    }
}

/// GraphQL client for `RunPod` API.
pub struct RunpodClient {
    cfg: RunpodClientConfig,
    http: reqwest::Client,
}

impl RunpodClient {
    /// Create a new `RunPod` GraphQL client.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(cfg: RunpodClientConfig) -> Result<Self, RunpodClientError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .build()
            .map_err(RunpodClientError::Http)?;

        Ok(Self { cfg, http })
    }

    /// Get a reference to the current configuration.
    #[must_use]
    pub const fn config(&self) -> &RunpodClientConfig {
        &self.cfg
    }

    /// Deploy an on-demand pod.
    ///
    /// Uses the `podFindAndDeployOnDemand` mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns an error.
    pub async fn deploy_on_demand(&self, input: DeployPodInput) -> Result<PodDeployResult, RunpodClientError> {
        let query = r"
            mutation podFindAndDeployOnDemand($input: PodFindAndDeployOnDemandInput!) {
                podFindAndDeployOnDemand(input: $input) {
                    id
                    name
                    desiredStatus
                    imageName
                    machineId
                    machine {
                        podHostId
                    }
                }
            }
        ";

        let variables = serde_json::json!({ "input": input });
        let resp: GraphQLResponse<DeployOnDemandData> = self.execute(query, variables).await?;

        resp.data
            .and_then(|d| d.podFindAndDeployOnDemand)
            .ok_or(RunpodClientError::EmptyResponse)
    }

    /// Deploy a spot (interruptible) pod.
    ///
    /// Uses the `podRentInterruptable` mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns an error.
    pub async fn deploy_spot(&self, input: DeployPodInput) -> Result<PodDeployResult, RunpodClientError> {
        let query = r"
            mutation podRentInterruptable($input: PodRentInterruptableInput!) {
                podRentInterruptable(input: $input) {
                    id
                    name
                    desiredStatus
                    imageName
                    machineId
                    machine {
                        podHostId
                    }
                }
            }
        ";

        let variables = serde_json::json!({ "input": input });
        let resp: GraphQLResponse<DeploySpotData> = self.execute(query, variables).await?;

        resp.data
            .and_then(|d| d.podRentInterruptable)
            .ok_or(RunpodClientError::EmptyResponse)
    }

    /// Resume a stopped pod.
    ///
    /// Uses the `podResume` mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns an error.
    pub async fn resume_pod(&self, pod_id: &str, gpu_count: u32) -> Result<PodSummary, RunpodClientError> {
        let query = r"
            mutation podResume($input: PodResumeInput!) {
                podResume(input: $input) {
                    id
                    desiredStatus
                    imageName
                    machineId
                }
            }
        ";

        let variables = serde_json::json!({
            "input": {
                "podId": pod_id,
                "gpuCount": gpu_count
            }
        });
        let resp: GraphQLResponse<PodResumeData> = self.execute(query, variables).await?;

        resp.data
            .and_then(|d| d.podResume)
            .ok_or(RunpodClientError::EmptyResponse)
    }

    /// Stop a running pod.
    ///
    /// Uses the `podStop` mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns an error.
    pub async fn stop_pod(&self, pod_id: &str) -> Result<PodSummary, RunpodClientError> {
        let query = r"
            mutation podStop($input: PodStopInput!) {
                podStop(input: $input) {
                    id
                    desiredStatus
                }
            }
        ";

        let variables = serde_json::json!({
            "input": { "podId": pod_id }
        });
        let resp: GraphQLResponse<PodStopData> = self.execute(query, variables).await?;

        resp.data
            .and_then(|d| d.podStop)
            .ok_or(RunpodClientError::EmptyResponse)
    }

    /// Terminate a pod (delete it).
    ///
    /// Uses the `podTerminate` mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns an error.
    pub async fn terminate_pod(&self, pod_id: &str) -> Result<(), RunpodClientError> {
        let query = r"
            mutation podTerminate($input: PodTerminateInput!) {
                podTerminate(input: $input)
            }
        ";

        let variables = serde_json::json!({
            "input": { "podId": pod_id }
        });
        let _resp: GraphQLResponse<PodTerminateData> = self.execute(query, variables).await?;

        Ok(())
    }

    /// Get a pod by ID.
    ///
    /// Uses the `pod` query.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns an error.
    pub async fn get_pod(&self, pod_id: &str) -> Result<Option<PodDetails>, RunpodClientError> {
        let query = r"
            query pod($input: PodFilter!) {
                pod(input: $input) {
                    id
                    name
                    desiredStatus
                    imageName
                    machineId
                    machine {
                        podHostId
                    }
                    runtime {
                        uptimeInSeconds
                        ports {
                            ip
                            isIpPublic
                            privatePort
                            publicPort
                            type
                        }
                        gpus {
                            id
                            gpuUtilPercent
                            memoryUtilPercent
                        }
                    }
                }
            }
        ";

        let variables = serde_json::json!({
            "input": { "podId": pod_id }
        });
        let resp: GraphQLResponse<PodQueryData> = self.execute(query, variables).await?;

        Ok(resp.data.and_then(|d| d.pod))
    }

    /// List all pods for the current user.
    ///
    /// Uses the `myself` query.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns an error.
    pub async fn list_pods(&self) -> Result<Vec<PodSummary>, RunpodClientError> {
        let query = r"
            query myself {
                myself {
                    pods {
                        id
                        name
                        desiredStatus
                        imageName
                        machineId
                    }
                }
            }
        ";

        let resp: GraphQLResponse<MyselfData> = self.execute(query, serde_json::json!({})).await?;

        Ok(resp
            .data
            .and_then(|d| d.myself)
            .map(|m| m.pods)
            .unwrap_or_default())
    }

    /// Get available GPU types.
    ///
    /// Uses the `gpuTypes` query.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns an error.
    pub async fn list_gpu_types(&self) -> Result<Vec<GpuType>, RunpodClientError> {
        let query = r"
            query gpuTypes {
                gpuTypes {
                    id
                    displayName
                    memoryInGb
                    secureCloud
                    communityCloud
                }
            }
        ";

        let resp: GraphQLResponse<GpuTypesData> = self.execute(query, serde_json::json!({})).await?;

        Ok(resp.data.map(|d| d.gpuTypes).unwrap_or_default())
    }

    /// Execute a GraphQL query/mutation with retry logic.
    async fn execute<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<GraphQLResponse<T>, RunpodClientError> {
        let mut attempt: u32 = 0;
        let mut backoff = Duration::from_millis(self.cfg.retry_backoff_ms);

        loop {
            attempt = attempt.saturating_add(1);

            let body = serde_json::json!({
                "query": query,
                "variables": variables
            });

            let send_res = self
                .http
                .post(&self.cfg.graphql_url)
                .bearer_auth(&self.cfg.api_key)
                .json(&body)
                .send()
                .await;

            match send_res {
                Ok(resp) => {
                    let status = resp.status();

                    if !status.is_success() {
                        let body_text = resp.text().await.unwrap_or_default();

                        if attempt <= self.cfg.retry_max && is_retryable_status(status) {
                            tokio::time::sleep(backoff).await;
                            backoff = next_backoff(backoff);
                            continue;
                        }

                        return Err(RunpodClientError::Api {
                            status,
                            body: body_text,
                        });
                    }

                    let gql_resp: GraphQLResponse<T> = resp
                        .json()
                        .await
                        .map_err(|e| RunpodClientError::Json(e.to_string()))?;

                    // Check for GraphQL errors
                    if let Some(errors) = &gql_resp.errors
                        && !errors.is_empty()
                    {
                        let msg = errors
                            .iter()
                            .map(|e| e.message.as_str())
                            .collect::<Vec<_>>()
                            .join("; ");
                        return Err(RunpodClientError::GraphQL(msg));
                    }

                    return Ok(gql_resp);
                }
                Err(e) => {
                    if attempt <= self.cfg.retry_max && is_retryable_reqwest(&e) {
                        tokio::time::sleep(backoff).await;
                        backoff = next_backoff(backoff);
                        continue;
                    }

                    return Err(RunpodClientError::Http(e));
                }
            }
        }
    }
}

// ============================================================================
// Input/Output types
// ============================================================================

/// Input for deploying a pod (on-demand or spot).
#[derive(Debug, Clone, Serialize)]
#[allow(non_snake_case)]
pub struct DeployPodInput {
    /// Cloud type ("SECURE" or "COMMUNITY").
    pub cloudType: String,
    /// GPU count.
    pub gpuCount: u32,
    /// Volume size in GB.
    pub volumeInGb: u32,
    /// Container disk size in GB.
    pub containerDiskInGb: u32,
    /// Minimum vCPU count.
    pub minVcpuCount: u32,
    /// Minimum RAM in GB.
    pub minMemoryInGb: u32,
    /// GPU type ID (e.g., "NVIDIA A40").
    pub gpuTypeId: String,
    /// Pod name.
    pub name: String,
    /// Container image name.
    pub imageName: String,
    /// Docker arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dockerArgs: Option<String>,
    /// Exposed ports (format: "22/tcp,8888/http").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<String>,
    /// Volume mount path.
    pub volumeMountPath: String,
    /// Environment variables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<EnvVar>>,
    /// Template ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub templateId: Option<String>,
    /// Network volume ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub networkVolumeId: Option<String>,
    /// Whether to start SSH.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startSsh: Option<bool>,
    /// Whether to start Jupyter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startJupyter: Option<bool>,
}

/// Environment variable for pod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    /// Variable key.
    pub key: String,
    /// Variable value.
    pub value: String,
}

/// Result from pod deployment.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct PodDeployResult {
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
    /// Machine details.
    pub machine: Option<MachineInfo>,
}

/// Machine information.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct MachineInfo {
    /// Pod host ID.
    pub podHostId: Option<String>,
}

/// Pod summary (minimal info).
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct PodSummary {
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
    /// Machine ID.
    pub machineId: Option<String>,
    /// Machine details.
    pub machine: Option<MachineInfo>,
    /// Runtime information.
    pub runtime: Option<RuntimeInfo>,
}

/// Runtime information for a running pod.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct RuntimeInfo {
    /// Uptime in seconds.
    pub uptimeInSeconds: Option<u64>,
    /// Port mappings.
    pub ports: Option<Vec<PortMapping>>,
    /// GPU information.
    pub gpus: Option<Vec<GpuInfo>>,
}

/// Port mapping information.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct PortMapping {
    /// IP address.
    pub ip: Option<String>,
    /// Whether IP is public.
    pub isIpPublic: Option<bool>,
    /// Private (container) port.
    pub privatePort: Option<u16>,
    /// Public port.
    pub publicPort: Option<u16>,
    /// Port type (tcp/http).
    #[serde(rename = "type")]
    pub port_type: Option<String>,
}

/// GPU information.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct GpuInfo {
    /// GPU ID.
    pub id: Option<String>,
    /// GPU utilization percentage.
    pub gpuUtilPercent: Option<f32>,
    /// Memory utilization percentage.
    pub memoryUtilPercent: Option<f32>,
}

/// GPU type information.
#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct GpuType {
    /// GPU type ID.
    pub id: String,
    /// Display name.
    pub displayName: Option<String>,
    /// Memory in GB.
    pub memoryInGb: Option<u32>,
    /// Available in secure cloud.
    pub secureCloud: Option<bool>,
    /// Available in community cloud.
    pub communityCloud: Option<bool>,
}

// ============================================================================
// GraphQL response types (internal)
// ============================================================================

#[derive(Debug, Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQLError {
    message: String,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct DeployOnDemandData {
    podFindAndDeployOnDemand: Option<PodDeployResult>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct DeploySpotData {
    podRentInterruptable: Option<PodDeployResult>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct PodResumeData {
    podResume: Option<PodSummary>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct PodStopData {
    podStop: Option<PodSummary>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct PodTerminateData {
    #[allow(dead_code)]
    podTerminate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PodQueryData {
    pod: Option<PodDetails>,
}

#[derive(Debug, Deserialize)]
struct MyselfData {
    myself: Option<MyselfInfo>,
}

#[derive(Debug, Deserialize)]
struct MyselfInfo {
    pods: Vec<PodSummary>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct GpuTypesData {
    gpuTypes: Vec<GpuType>,
}

// ============================================================================
// Error type
// ============================================================================

/// Error type for `RunPod` client operations.
#[derive(Debug)]
pub enum RunpodClientError {
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
    /// GraphQL error from server.
    GraphQL(String),
    /// API error response.
    Api {
        /// HTTP status code.
        status: reqwest::StatusCode,
        /// Response body.
        body: String,
    },
    /// Empty response from server.
    EmptyResponse,
}

impl fmt::Display for RunpodClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnv(k) => write!(f, "missing required env var: {k}"),
            Self::InvalidEnv { key, reason } => write!(f, "invalid env var {key}: {reason}"),
            Self::Http(e) => write!(f, "http error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
            Self::GraphQL(e) => write!(f, "graphql error: {e}"),
            Self::Api { status, body } => {
                write!(f, "api error: status={status}, body={body}")
            }
            Self::EmptyResponse => write!(f, "empty response from server"),
        }
    }
}

impl std::error::Error for RunpodClientError {}

// ============================================================================
// Helper functions
// ============================================================================

fn must_env(key: &'static str) -> Result<String, RunpodClientError> {
    env::var(key).map_err(|_| RunpodClientError::MissingEnv(key))
}

fn parse_u32_env(key: &'static str, default: u32) -> Result<u32, RunpodClientError> {
    env::var(key).map_or_else(
        |_| Ok(default),
        |v| {
            v.parse::<u32>().map_err(|_| RunpodClientError::InvalidEnv {
                key,
                reason: "expected an unsigned integer",
            })
        },
    )
}

fn parse_u64_env(key: &'static str, default: u64) -> Result<u64, RunpodClientError> {
    env::var(key).map_or_else(
        |_| Ok(default),
        |v| {
            v.parse::<u64>().map_err(|_| RunpodClientError::InvalidEnv {
                key,
                reason: "expected an unsigned integer",
            })
        },
    )
}

#[inline]
const fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status.as_u16(),
        408 | 409 | 425 | 429 | 500 | 502 | 503 | 504
    )
}

#[inline]
fn is_retryable_reqwest(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect() || e.is_request()
}

#[inline]
fn next_backoff(current: Duration) -> Duration {
    let next = current.saturating_mul(2);
    next.min(Duration::from_secs(10))
}
