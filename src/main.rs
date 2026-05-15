mod config;
mod dictionary;
mod hl7;
mod mllp;
mod store;
mod validation;
mod web;

use config::Config;
use mllp::MllpStats;
use store::MessageStore;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use web::{create_router, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load configuration (file → env vars → defaults) before tracing init
    let config = Config::load();

    // Initialize logging — use config level as fallback when RUST_LOG is not set
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logging.level));

    let fmt_layer = tracing_subscriber::fmt::layer();

    let (file_layer, _appender_guard) = if let Some(file_path) = &config.logging.file {
        if !file_path.is_empty() {
            let path = std::path::Path::new(file_path);
            let dir = path.parent().unwrap_or(std::path::Path::new("."));
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("harux.log");
            let appender = tracing_appender::rolling::daily(dir, file_name);
            let (non_blocking, guard) = tracing_appender::non_blocking(appender);
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(non_blocking);
            (Some(layer), Some(guard))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(file_layer)
        .init();

    let mllp_port = config.server.mllp_port;
    let web_port = config.server.web_port;

    let store = MessageStore::new(config.store.clone());
    let stats = MllpStats::new();

    info!("╔══════════════════════════════════════════╗");
    info!(
        "║          Harux v{}                ║",
        env!("CARGO_PKG_VERSION")
    );
    info!("╠══════════════════════════════════════════╣");
    info!("║  MLLP Server:  0.0.0.0:{}              ║", mllp_port);
    info!("║  Web UI:       http://localhost:{}     ║", web_port);
    info!("╚══════════════════════════════════════════╝");
    info!("Effective configuration:\n{}", config);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Start MLLP server
    let mllp_store = store.clone();
    let mllp_stats = stats.clone();
    let mllp_shutdown = shutdown_rx.clone();
    let mllp_config = config.mllp.clone();
    let mllp_handle = tokio::spawn(async move {
        let addr = format!("0.0.0.0:{}", mllp_port);
        if let Err(e) =
            mllp::start_mllp_server(&addr, mllp_store, mllp_stats, mllp_shutdown, mllp_config).await
        {
            warn!("MLLP server error: {}", e);
        }
    });

    // Start Web server
    let app_state = AppState {
        store: store.clone(),
        stats: stats.clone(),
        mllp_port,
        max_connections: config.mllp.max_connections,
    };
    let app = create_router(app_state);
    let web_addr = format!("0.0.0.0:{}", web_port);
    let listener = tokio::net::TcpListener::bind(&web_addr).await?;
    let web_shutdown = shutdown_rx.clone();
    let web_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let mut rx = web_shutdown;
                let _ = rx.changed().await;
            })
            .await
            .expect("Web server failed");
    });

    // Wait for a shutdown signal or an unexpected server exit
    tokio::select! {
        _ = mllp_handle => {
            warn!("MLLP server stopped unexpectedly, initiating shutdown");
        }
        _ = web_handle => {
            warn!("Web server stopped unexpectedly, initiating shutdown");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down gracefully");
        }
    }

    let _ = shutdown_tx.send(true);

    // Wait for active MLLP connections to drain
    let shutdown_timeout = std::time::Duration::from_secs(config.server.shutdown_timeout_secs);
    info!(
        "Waiting up to {} seconds for MLLP connections to drain...",
        config.server.shutdown_timeout_secs
    );
    let start = std::time::Instant::now();
    loop {
        let active = stats
            .active_connections
            .load(std::sync::atomic::Ordering::Relaxed);
        if active == 0 {
            info!("All MLLP connections drained");
            break;
        }
        if start.elapsed() >= shutdown_timeout {
            warn!(
                "Shutdown timeout reached. Forcing exit with {} active connections",
                active
            );
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    info!("Harux stopped.");
    Ok(())
}
