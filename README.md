# Halldyll Starter

A comprehensive Rust library for managing RunPod GPU pods with automatic provisioning, state management, and orchestration.

## Features

- **REST API Client** - Create, start, stop pods via RunPod REST API
- **GraphQL Client** - Full access to RunPod GraphQL API for advanced operations
- **State Management** - Persist pod state and compute idempotent action plans
- **Orchestration** - High-level pod management with automatic reconciliation
- **Fully Configurable** - All settings via environment variables (`.env` file)
- **Strict Linting** - Production-ready code with comprehensive lint rules

## Installation

### From GitHub

Add to your `Cargo.toml`:

```toml
[dependencies]
halldyll_starter = { git = "https://github.com/halldyll/starter" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

### From local path

```toml
[dependencies]
halldyll_starter = { path = "path/to/starter" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Configuration

Create a `.env` file in your project root:

```env
# Required
RUNPOD_API_KEY=your_api_key_here
RUNPOD_IMAGE_NAME=runpod/pytorch:2.1.0-py3.10-cuda11.8.0-devel

# Optional - Pod Configuration
RUNPOD_POD_NAME=my-gpu-pod
RUNPOD_GPU_TYPE_IDS=NVIDIA A40
RUNPOD_GPU_COUNT=1
RUNPOD_CONTAINER_DISK_GB=20
RUNPOD_VOLUME_GB=50
RUNPOD_VOLUME_MOUNT_PATH=/workspace
RUNPOD_PORTS=22/tcp,8888/http

# Optional - Timeouts
RUNPOD_HTTP_TIMEOUT_MS=30000
RUNPOD_READY_TIMEOUT_MS=300000
RUNPOD_POLL_INTERVAL_MS=5000

# Optional - API URLs
RUNPOD_REST_URL=https://rest.runpod.io/v1
RUNPOD_GRAPHQL_URL=https://api.runpod.io/graphql

# Optional - Behavior
RUNPOD_RECONCILE_MODE=reuse
```

### Environment Variables Reference

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|*
| `RUNPOD_API_KEY` | ✓ | - | RunPod API key |
| `RUNPOD_IMAGE_NAME` | ✓ | - | Container image (e.g., `runpod/pytorch:2.1.0-py3.10-cuda11.8.0-devel`) |
| `RUNPOD_POD_NAME` | | `halldyll-pod` | Name for the pod |
| `RUNPOD_GPU_TYPE_IDS` | | `NVIDIA A40` | Comma-separated GPU types (e.g., `NVIDIA A40,NVIDIA RTX 4090`) |
| `RUNPOD_GPU_COUNT` | | `1` | Number of GPUs |
| `RUNPOD_CONTAINER_DISK_GB` | | `20` | Container disk size in GB |
| `RUNPOD_VOLUME_GB` | | `0` | Persistent volume size (0 = no volume) |
| `RUNPOD_VOLUME_MOUNT_PATH` | | `/workspace` | Mount path for persistent volume |
| `RUNPOD_PORTS` | | `22/tcp,8888/http` | Exposed ports (format: `port/protocol`) |
| `RUNPOD_HTTP_TIMEOUT_MS` | | `30000` | HTTP request timeout (ms) |
| `RUNPOD_READY_TIMEOUT_MS` | | `300000` | Pod ready timeout (ms) |
| `RUNPOD_POLL_INTERVAL_MS` | | `5000` | Poll interval for readiness (ms) |
| `RUNPOD_RECONCILE_MODE` | | `reuse` | `reuse` or `recreate` existing pods |

### Pod Naming & Multiple Pods

The orchestrator uses the pod name to identify and reuse existing pods:

- **Same name** → Reuses the existing pod (starts it if stopped)
- **Different name** → Creates a new pod

To run multiple pods simultaneously, simply use different names:

```env
# Development pod
RUNPOD_POD_NAME=dev-pod

# Production pod  
RUNPOD_POD_NAME=prod-pod

# ML training pod
RUNPOD_POD_NAME=training-pod
```

Each unique name creates a separate pod on RunPod.

## Usage

### Quick Start with Orchestrator

The orchestrator provides the simplest way to get a ready-to-use pod:

```rust
use halldyll_starter::{RunpodOrchestrator, RunpodOrchestratorConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load config from .env
    let cfg = RunpodOrchestratorConfig::from_env()?;
    let orchestrator = RunpodOrchestrator::new(cfg)?;

    // Get a ready pod (creates, starts, or reuses as needed)
    let pod = orchestrator.ensure_ready_pod().await?;

    println!("Pod ready: {} at {}", pod.name, pod.public_ip);

    // Get SSH connection info
    if let Some((host, port)) = pod.ssh_endpoint() {
        println!("SSH: ssh -p {} user@{}", port, host);
    }

    // Get Jupyter URL
    if let Some(url) = pod.jupyter_endpoint() {
        println!("Jupyter: {}", url);
    }

    Ok(())
}
```

### Low-Level Provisioner

For direct pod creation:

```rust
use halldyll_starter::{RunpodProvisioner, RunpodProvisionConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = RunpodProvisionConfig::from_env()?;
    let provisioner = RunpodProvisioner::new(cfg)?;

    let pod = provisioner.create_pod().await?;
    println!("Created pod: {}", pod.id);

    Ok(())
}
```

### Pod Starter (Start/Stop)

For managing existing pods:

```rust
use halldyll_starter::{RunpodStarter, RunpodStarterConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = RunpodStarterConfig::from_env()?;
    let starter = RunpodStarter::new(cfg)?;

    // Start a pod
    let status = starter.start("pod_id_here").await?;
    println!("Pod status: {}", status.desired_status);

    // Stop a pod
    let status = starter.stop("pod_id_here").await?;
    println!("Pod stopped: {}", status.desired_status);

    Ok(())
}
```

### GraphQL Client

For advanced operations:

```rust
use halldyll_starter::{RunpodClient, RunpodClientConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = RunpodClientConfig::from_env()?;
    let client = RunpodClient::new(cfg)?;

    // List all pods
    let pods = client.list_pods().await?;
    for pod in pods {
        println!("Pod: {} ({})", pod.id, pod.desired_status);
    }

    // List available GPU types
    let gpus = client.list_gpu_types().await?;
    for gpu in gpus {
        println!("GPU: {} - Available: {}", gpu.display_name, gpu.available_count);
    }

    Ok(())
}
```

### State Management

For persistent state and reconciliation:

```rust
use halldyll_starter::{RunPodState, JsonFileStateStore, PlannedAction};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = JsonFileStateStore::new("./pod_state.json");
    
    // Load existing state
    let mut state = store.load()?.unwrap_or_default();

    // Record a pod
    state.record_pod("pod-123", "my-pod", "runpod/pytorch:latest");

    // Compute reconciliation plan
    let action = state.reconcile("my-pod", "runpod/pytorch:latest");
    match action {
        PlannedAction::DoNothing(id) => println!("Pod {} is ready", id),
        PlannedAction::Start(id) => println!("Need to start pod {}", id),
        PlannedAction::Create => println!("Need to create new pod"),
    }

    // Save state
    store.save(&state)?;

    Ok(())
}
```

## Modules

| Module | Description |
|--------|-------------|*
| `runpod_provisioner` | Create new pods via REST API |
| `runpod_starter` | Start/stop existing pods via REST API |
| `runpod_state` | State persistence and reconciliation |
| `runpod_client` | GraphQL client for advanced operations |
| `runpod_orchestrator` | High-level pod management |

## GPU Types

Common GPU types available on RunPod:

| GPU | ID |
|-----|-----|*
| NVIDIA A40 | `NVIDIA A40` |
| NVIDIA A100 80GB | `NVIDIA A100 80GB PCIe` |
| NVIDIA RTX 4090 | `NVIDIA GeForce RTX 4090` |
| NVIDIA RTX 3090 | `NVIDIA GeForce RTX 3090` |
| NVIDIA L40S | `NVIDIA L40S` |

Use `client.list_gpu_types()` to get the full list with availability.

## Running the Example

```bash
# Clone the project
git clone https://github.com/halldyll/starter.git
cd starter

# Create your .env file
cp .env.example .env
# Edit .env with your API key and settings

# Run the example
cargo run
```

## Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Check without building
cargo check

# Run with all lints
cargo clippy -- -D warnings
```

## Contributing

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

MIT
