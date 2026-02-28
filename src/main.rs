mod discover;
mod monitor;
mod router;
mod types;

use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use local_ip_address::local_ip;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::info;

use types::{AtomicHealth, SystemStats};

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
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let (paths, static_info) = discover::discover_device_layout();
    let static_info = Arc::new(static_info);
    let health = Arc::new(AtomicHealth::default());

    let (tx, rx) = watch::channel(SystemStats::default());
    let poll_interval = Duration::from_millis(args.interval);

    tokio::spawn(monitor::run_monitor(
        tx,
        paths,
        Arc::clone(&static_info),
        Arc::clone(&health),
        poll_interval,
    ));

    let app = router::build(rx, health);

    let addr = format!("{}:{}", args.bind, args.port);
    let listener = TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("failed to bind {addr}: {e}");
            std::process::exit(1);
        });

    let host = local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "localhost".into());

    info!(addr = %addr, "asmo v{} started", env!("CARGO_PKG_VERSION"));
    println!("\n\u{1F680} Asmo running on: http://{host}:{}", args.port);
    println!("   GET / for all available endpoints\n");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|e| {
            eprintln!("server error: {e}");
            std::process::exit(1);
        });

    info!("shutdown complete");
}

/// Wait for a shutdown signal (Ctrl+C).
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for Ctrl+C");
    info!("received shutdown signal");
}
