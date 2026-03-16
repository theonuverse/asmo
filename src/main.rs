mod discover;
mod monitor;
mod router;
mod types;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::process::Command;

use clap::Parser;
use local_ip_address::local_ip;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use types::{AtomicHealth, ServiceStatus, SystemStats};

/// Lightweight REST API server exposing real-time Android device stats.
#[derive(Parser)]
#[command(name = "asmo", version, about)]
struct Args {
    /// Port to listen on.
    #[arg(short, long, default_value_t = 3000)]
    port: u16,

    /// Polling interval in milliseconds.
    #[arg(short = 'i', long, default_value_t = 500)]
    interval: u64,

    /// Address to bind to.
    #[arg(short, long, default_value = "0.0.0.0")]
    bind: String,
}

#[tokio::main]
async fn main() {
    // Respect RUST_LOG for runtime verbosity without recompiling.
    // Examples: RUST_LOG=debug  RUST_LOG=asmo=debug  RUST_LOG=warn
    // Defaults to info — clean output suitable for service logs.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = Args::parse();
    let addr = format!("{}:{}", args.bind, args.port);

    ensure_rish_available_or_exit();

    let started_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (paths, static_info) = discover::discover_device_layout();
    let static_info = Arc::new(static_info);
    let health = Arc::new(AtomicHealth::default());

    let (tx, rx) = watch::channel(SystemStats::default());
    let poll_interval = Duration::from_millis(args.interval);

    let svc_status = Arc::new(ServiceStatus::new(
        started_unix_secs,
        args.interval,
        &paths.cpu_temp,
        &paths.gpu_temp,
        &addr,
        static_info.cores.len(),
    ));

    info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = %addr,
        interval_ms = args.interval,
        cores = static_info.cores.len(),
        "asmo starting"
    );

    tokio::spawn(monitor::run_monitor(
        tx,
        paths,
        Arc::clone(&static_info),
        Arc::clone(&health),
        Arc::clone(&svc_status),
        poll_interval,
    ));

    let app = router::build(rx, health, Arc::clone(&svc_status));

    let listener = TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(addr = %addr, error = %e, "failed to bind TCP listener");
            std::process::exit(1);
        });

    let host = local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "localhost".into());

    info!(addr = %addr, "TCP listener bound");
    println!("\n\u{1F680} Asmo v{} running on: http://{host}:{}", env!("CARGO_PKG_VERSION"), args.port);
    println!("   GET / for all endpoints \u{00B7} GET /debug for service diagnostics\n");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "HTTP server error");
            std::process::exit(1);
        });

    info!("shutdown complete");
}

/// Fail-fast preflight: asmo requires a working rish session.
/// Without rish we cannot collect privileged metrics, so startup is aborted.
fn ensure_rish_available_or_exit() {
    let output = Command::new("rish")
        .args(["-c", "echo asmo_rish_ok"])
        .output();

    let ok = match output {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            error!(
                status = ?out.status.code(),
                stdout = %stdout,
                stderr = %stderr,
                "rish preflight failed"
            );
            false
        }
        Err(e) => {
            error!(error = %e, "rish command not available");
            false
        }
    };

    if !ok {
        eprintln!(
            "asmo requires a working rish session and will not start without it.\n\
             Ensure Shizuku is running and Termux is authorized, then verify:\n\
             rish -c 'echo ok'"
        );
        std::process::exit(1);
    }

    info!("rish preflight ok");
}

/// Wait for a shutdown signal (Ctrl+C).
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for Ctrl+C");
    info!("received shutdown signal");
}
