//! `RunPod` pod starter (start/resume).
//!
//! Unique responsibility: start (or resume) a single existing Pod via `RunPod` REST API.
//!
//! API endpoint used:
//! - POST <https://rest.runpod.io/v1/pods/{podId}/start>
//! - Header: Authorization: Bearer <token>

use std::{env, fmt, time::Duration};

/// Configuration for starting/resuming a `RunPod` pod.
pub struct RunpodStarterConfig {
    /// `RunPod` API key for authentication.
    /// Env: `RUNPOD_API_KEY` (required)
    pub api_key: String,

    /// REST API URL for `RunPod`.
    /// Env: `RUNPOD_REST_URL` (default: "<https://rest.runpod.io/v1>")
    pub rest_url: String,

    /// Pod ID to start/resume.
    /// Env: `RUNPOD_POD_ID` (required)
    pub pod_id: String,

    /// HTTP request timeout in milliseconds.
    /// Env: `RUNPOD_HTTP_TIMEOUT_MS` (default: 15000)
    pub timeout_ms: u64,

    /// Maximum number of retry attempts.
    /// Env: `RUNPOD_HTTP_RETRY_MAX` (default: 3)
    pub retry_max: u32,

    /// Backoff time between retries in milliseconds.
    /// Env: `RUNPOD_HTTP_RETRY_BACKOFF_MS` (default: 250)
    pub retry_backoff_ms: u64,

    /// User agent for HTTP requests.
    /// Env: `RUNPOD_USER_AGENT` (default: "halldyll-starter/1.0")
    pub user_agent: String,
}

impl RunpodStarterConfig {
    /// Load configuration from environment variables.
    ///
    /// In local dev, this will also attempt to load `.env` from the current directory.
    /// If `.env` is missing, it does not fail.
    ///
    /// # Errors
    ///
    /// Returns an error if required environment variables are missing or invalid.
    pub fn from_env() -> Result<Self, RunpodError> {
        let _ = dotenvy::dotenv();

        let api_key = must_env("RUNPOD_API_KEY")?;
        let rest_url = env::var("RUNPOD_REST_URL")
            .unwrap_or_else(|_| "https://rest.runpod.io/v1".to_string());
        let pod_id = must_env("RUNPOD_POD_ID")?;

        let timeout_ms = parse_u64_env("RUNPOD_HTTP_TIMEOUT_MS", 15_000)?;
        let retry_max = parse_u32_env("RUNPOD_HTTP_RETRY_MAX", 3)?;
        let retry_backoff_ms = parse_u64_env("RUNPOD_HTTP_RETRY_BACKOFF_MS", 250)?;

        let user_agent = env::var("RUNPOD_USER_AGENT")
            .unwrap_or_else(|_| "halldyll-starter/1.0".to_string());

        Ok(Self {
            api_key,
            rest_url,
            pod_id,
            timeout_ms,
            retry_max,
            retry_backoff_ms,
            user_agent,
        })
    }

    /// Build the start URL for the configured pod.
    #[inline]
    fn start_url(&self) -> String {
        format!(
            "{}/pods/{}/start",
            self.rest_url.trim_end_matches('/'),
            self.pod_id
        )
    }

    /// Build the stop URL for the configured pod.
    #[inline]
    fn stop_url(&self) -> String {
        format!(
            "{}/pods/{}/stop",
            self.rest_url.trim_end_matches('/'),
            self.pod_id
        )
    }
}

/// Starter for resuming/starting `RunPod` pods.
pub struct RunpodStarter {
    cfg: RunpodStarterConfig,
    http: reqwest::Client,
}

impl RunpodStarter {
    /// Create a new `RunPod` starter from the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(cfg: RunpodStarterConfig) -> Result<Self, RunpodError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .user_agent(cfg.user_agent.clone())
            .build()
            .map_err(RunpodError::Http)?;

        Ok(Self { cfg, http })
    }

    /// Start or resume the configured pod.
    ///
    /// Returns the raw response body on success.
    /// Implements retry logic with exponential backoff for transient failures.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the API returns an error.
    pub async fn start_or_resume(&self) -> Result<String, RunpodError> {
        let url = self.cfg.start_url();
        self.post_with_retry(&url).await
    }

    /// Stop the configured pod.
    ///
    /// Returns the raw response body on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the API returns an error.
    pub async fn stop(&self) -> Result<String, RunpodError> {
        let url = self.cfg.stop_url();
        self.post_with_retry(&url).await
    }

    /// Get a reference to the current configuration.
    #[must_use]
    pub const fn config(&self) -> &RunpodStarterConfig {
        &self.cfg
    }

    /// Internal method to POST with retry logic.
    async fn post_with_retry(&self, url: &str) -> Result<String, RunpodError> {
        let mut attempt: u32 = 0;
        let mut backoff = Duration::from_millis(self.cfg.retry_backoff_ms);

        loop {
            attempt = attempt.saturating_add(1);

            let send_res = self
                .http
                .post(url)
                .bearer_auth(&self.cfg.api_key)
                .send()
                .await;

            match send_res {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();

                    if status.is_success() {
                        return Ok(body);
                    }

                    // Retry on typical transient statuses.
                    if attempt <= self.cfg.retry_max && is_retryable_status(status) {
                        tokio::time::sleep(backoff).await;
                        backoff = next_backoff(backoff);
                        continue;
                    }

                    return Err(RunpodError::Api { status, body });
                }
                Err(e) => {
                    // Retry on connection/timeout errors (transient).
                    if attempt <= self.cfg.retry_max && is_retryable_reqwest(&e) {
                        tokio::time::sleep(backoff).await;
                        backoff = next_backoff(backoff);
                        continue;
                    }

                    return Err(RunpodError::Http(e));
                }
            }
        }
    }
}

/// Error type for `RunPod` starter operations.
#[derive(Debug)]
pub enum RunpodError {
    /// Missing required environment variable.
    MissingEnv(&'static str),
    /// Invalid environment variable value.
    InvalidEnv {
        /// The environment variable key.
        key: &'static str,
        /// The environment variable value.
        value: String,
        /// The reason for invalidity.
        reason: &'static str,
    },
    /// HTTP client error.
    Http(reqwest::Error),
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
            Self::InvalidEnv { key, value, reason } => {
                write!(f, "invalid env var {key}={value:?}: {reason}")
            }
            Self::Http(e) => write!(f, "http error: {e}"),
            Self::Api { status, body } => {
                write!(f, "runpod api error: status={status}, body={body}")
            }
        }
    }
}

impl std::error::Error for RunpodError {}

#[inline]
fn must_env(key: &'static str) -> Result<String, RunpodError> {
    env::var(key).map_err(|_| RunpodError::MissingEnv(key))
}

#[inline]
fn parse_u64_env(key: &'static str, default: u64) -> Result<u64, RunpodError> {
    env::var(key).map_or_else(
        |_| Ok(default),
        |v| {
            v.parse::<u64>().map_err(|_| RunpodError::InvalidEnv {
                key,
                value: v,
                reason: "expected an unsigned integer",
            })
        },
    )
}

#[inline]
fn parse_u32_env(key: &'static str, default: u32) -> Result<u32, RunpodError> {
    env::var(key).map_or_else(
        |_| Ok(default),
        |v| {
            v.parse::<u32>().map_err(|_| RunpodError::InvalidEnv {
                key,
                value: v,
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
    // Exponential backoff capped at 5 seconds.
    let next = current.saturating_mul(2);
    next.min(Duration::from_secs(5))
}
