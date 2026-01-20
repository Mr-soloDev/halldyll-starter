//! Example binary demonstrating the halldyll_starter library.
//!
//! This example shows how to use the orchestrator to get a ready pod.
//!
//! ## Usage
//!
//! 1. Create a `.env` file with your configuration
//! 2. Run: `cargo run`

#![allow(clippy::print_stdout)] // Allow println! in the binary example

use halldyll_starter::{RunpodOrchestrator, RunpodOrchestratorConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration from environment
    let cfg = RunpodOrchestratorConfig::from_env()?;
    println!("Configuration loaded:");
    println!("  Pod name: {}", cfg.pod_name);
    println!("  Image: {}", cfg.image_name);
    println!("  GPU types: {:?}", cfg.gpu_type_ids);

    // Create orchestrator
    let orchestrator = RunpodOrchestrator::new(cfg)?;

    // Get a ready pod (creates, starts, or reuses as needed)
    println!("\nEnsuring pod is ready...");
    let pod = orchestrator.ensure_ready_pod().await?;

    println!("\nPod ready!");
    println!("  ID: {}", pod.id);
    println!("  Name: {}", pod.name);
    println!("  Public IP: {}", pod.public_ip);
    println!("  Status: {}", pod.desired_status);

    // Show connection info
    if let Some((host, port)) = pod.ssh_endpoint() {
        println!("\nSSH connection:");
        println!("  ssh -p {} user@{}", port, host);
    }

    if let Some(url) = pod.jupyter_endpoint() {
        println!("\nJupyter URL:");
        println!("  {}", url);
    }

    println!("\nPort mappings:");
    for (container_port, public_port) in &pod.port_mappings {
        println!("  {} -> {}", container_port, public_port);
    }

    Ok(())
}
