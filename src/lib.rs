//! Halldyll Starter - `RunPod` orchestration library.
//!
//! A comprehensive library for managing `RunPod` GPU pods with:
//! - **Provisioning**: Create new pods via REST API
//! - **Starting**: Start/stop existing pods with retry logic
//! - **State Management**: Persist and reconcile pod state
//! - **GraphQL Client**: Full GraphQL API access for advanced operations
//! - **Orchestration**: High-level pod management with automatic reconciliation
//!
//! ## Quick Start
//!
//! All configuration is loaded from environment variables. Create a `.env` file:
//!
//! ```text
//! RUNPOD_API_KEY=your_api_key_here
//! RUNPOD_IMAGE_NAME=your/image:tag
//! RUNPOD_POD_NAME=my-pod
//! RUNPOD_GPU_TYPE_IDS=NVIDIA A40
//! ```
//!
//! Then use the orchestrator for simple pod management:
//!
//! ```ignore
//! use halldyll_starter::runpod_orchestrator::{RunpodOrchestrator, RunpodOrchestratorConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let cfg = RunpodOrchestratorConfig::from_env()?;
//!     let orchestrator = RunpodOrchestrator::new(cfg)?;
//!
//!     let pod = orchestrator.ensure_ready_pod().await?;
//!     println!("Pod ready: {} at {}", pod.name, pod.public_ip);
//!
//!     if let Some((host, port)) = pod.ssh_endpoint() {
//!         println!("SSH: ssh -p {} user@{}", port, host);
//!     }
//!
//!     Ok(())
//! }
//! ```

// ============================================================================
// Strict linting - Dangerous or non-idiomatic practices are forbidden
// ============================================================================

#![deny(warnings)]                    // All warnings are treated as errors
#![deny(unsafe_code)]                 // Unsafe code is forbidden
#![deny(missing_docs)]                // All public items must be documented
#![deny(dead_code)]                   // Unused code is forbidden
#![deny(non_camel_case_types)]        // Types must follow CamelCase convention

// Additional strictness - Leave nothing unchecked
#![deny(unused_imports)]              // Unused imports are forbidden
#![deny(unused_variables)]            // Unused variables are forbidden
#![deny(unused_must_use)]             // Must handle Result and Option explicitly
#![deny(non_snake_case)]              // Variables and functions must be snake_case
#![deny(non_upper_case_globals)]      // Constants must be UPPER_CASE
#![deny(nonstandard_style)]           // Non-standard code style is forbidden
#![forbid(unsafe_op_in_unsafe_fn)]    // Unsafe ops in unsafe fns are forbidden

// Clippy for strict discipline
#![deny(clippy::all)]                 // All standard Clippy lints
#![deny(clippy::pedantic)]            // Very strict Clippy lints
#![deny(clippy::nursery)]             // Experimental lints
#![deny(clippy::unwrap_used)]         // unwrap() is forbidden
#![deny(clippy::expect_used)]         // expect() is forbidden
#![deny(clippy::panic)]               // panic!() is forbidden
#![deny(clippy::print_stdout)]        // println!() is forbidden in production
#![deny(clippy::todo)]                // TODO is forbidden
#![deny(clippy::unimplemented)]       // unimplemented!() is forbidden
#![deny(clippy::missing_const_for_fn)] // Force const when possible
#![deny(clippy::unwrap_in_result)]    // unwrap() in Result is forbidden
#![deny(clippy::module_inception)]    // Module with same name as crate is forbidden
#![deny(clippy::redundant_clone)]     // Useless clones are forbidden
#![deny(clippy::shadow_unrelated)]    // Shadowing unrelated variables is forbidden
#![deny(clippy::too_many_arguments)]  // Limit function arguments
#![deny(clippy::cognitive_complexity)] // Limit cognitive complexity

// Safety and robustness lints
#![deny(overflowing_literals)]        // Overflowing literals are forbidden
#![deny(arithmetic_overflow)]         // Arithmetic overflow is forbidden

// ============================================================================
// Modules
// ============================================================================

/// Pod provisioning via RunPod REST API.
///
/// Use this module to create new GPU pods with custom configuration.
pub mod runpod_provisioner;

/// Pod starter for managing existing pods via REST API.
///
/// Use this module to start, stop, and check the status of pods.
pub mod runpod_starter;

/// State persistence and reconciliation.
///
/// Use this module to persist pod state and compute idempotent action plans.
pub mod runpod_state;

/// GraphQL client for advanced RunPod API operations.
///
/// Use this module for operations not available via REST API.
pub mod runpod_client;

/// High-level pod orchestration.
///
/// Use this module for simplified pod management with automatic reconciliation.
pub mod runpod_orchestrator;

// ============================================================================
// Re-exports for convenience
// ============================================================================

pub use runpod_client::{RunpodClient, RunpodClientConfig};
pub use runpod_orchestrator::{PodLease, RunpodOrchestrator, RunpodOrchestratorConfig};
pub use runpod_provisioner::{RunpodProvisionConfig, RunpodProvisioner};
pub use runpod_starter::{RunpodStarter, RunpodStarterConfig};
pub use runpod_state::{JsonFileStateStore, PlannedAction, RunPodState, StateStore};
