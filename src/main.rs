mod config;
mod dictionary;
mod hl7;
mod logging;
mod mllp;
mod store;
mod validation;
mod web;

use config::Config;
use mllp::MllpStats;
use std::sync::atomic::Ordering;
use store::MessageStore;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use web::{create_router, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load configuration (file → env vars → defaults) before tracing init
    // A broken configuration stops the start (#233): running on other
    // ports or limits than configured is worse than not running.
    let config = Config::load().map_err(|e| anyhow::anyhow!(e))?;

    // Initialize logging — use config level as fallback when RUST_LOG is not set
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logging.level));

    let fmt_layer = tracing_subscriber::fmt::layer();

    // File logging must not keep Harux from receiving (#234): if the file
    // can't be opened, log to stdout only and say so — the choice made in
    // #113. A panic, as `rolling::daily` does, is neither.
    let mut file_log_problem = None;
    let log_target = config
        .logging
        .file
        .as_deref()
        .filter(|f| !f.is_empty())
        .map(logging::LogTarget::from_config);
    let (file_layer, _appender_guard, log_target) = match log_target {
        Some(target) => match target.appender() {
            Ok(appender) => {
                let (non_blocking, guard) = tracing_appender::non_blocking(appender);
                let layer = tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(non_blocking);
                (Some(layer), Some(guard), Some(target))
            }
            Err(e) => {
                let problem = format!(
                    "cannot write the log file {:?}: {e}; logging to stdout only",
                    config.logging.file.as_deref().unwrap_or_default()
                );
                eprintln!("WARN harux: {problem}");
                file_log_problem = Some(problem);
                (None, None, None)
            }
        },
        None => (None, None, None),
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(file_layer)
        .init();
    if let Some(problem) = &file_log_problem {
        warn!("{problem}");
    }
    // Keep the last `logging.max_files` daily files (#240): once now, then
    // hourly, so a long-running service prunes after every rollover.
    if let (Some(target), keep @ 1..) = (log_target, config.logging.max_files) {
        prune_logs(&target, keep);
        tokio::spawn(async move {
            let hour = std::time::Duration::from_secs(3600);
            let mut every_hour = tokio::time::interval_at(tokio::time::Instant::now() + hour, hour);
            loop {
                every_hour.tick().await;
                prune_logs(&target, keep);
            }
        });
    }

    let mllp_port = config.server.mllp_port;
    let mllp_addr = config.server.mllp_addr();
    let web_addr = config.server.web_addr();

    let store = MessageStore::new(config.store.clone());
    let stats = MllpStats::new();

    {
        let title = format!("  Harux v{}  ", env!("CARGO_PKG_VERSION"));
        let mllp_line = format!("  MLLP Server:  {mllp_addr}  ");
        // An address a browser can open: localhost when the UI listens on
        // every interface or on loopback.
        let web_url = if web_addr.ip().is_unspecified() || web_addr.ip().is_loopback() {
            format!("http://localhost:{}", web_addr.port())
        } else {
            format!("http://{web_addr}")
        };
        let web_line = format!("  Web UI:       {web_url}  ");
        let w = title.len().max(mllp_line.len()).max(web_line.len());
        let bar: String = "═".repeat(w);
        info!("╔{}╗", bar);
        info!("║{:w$}║", title, w = w);
        info!("╠{}╣", bar);
        info!("║{:w$}║", mllp_line, w = w);
        info!("║{:w$}║", web_line, w = w);
        info!("╚{}╝", bar);
    }
    match &config.source {
        Some(path) => info!("Configuration loaded from {}", path.display()),
        None => info!("No harux.toml found — using defaults and environment variables"),
    }
    info!("Effective configuration:\n{}", config);
    for problem in web::allowed_hosts_problems(&config.server.allowed_hosts) {
        warn!(
            "server.allowed_hosts: {problem}; this entry never matches, and browsers \
             opening Harux by that name get 403"
        );
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Start MLLP server
    let mllp_store = store.clone();
    let mllp_stats = stats.clone();
    let mllp_shutdown = shutdown_rx.clone();
    let mllp_config = config.mllp.clone();
    let mllp_handle = tokio::spawn(async move {
        let addr = mllp_addr.to_string();
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
        allowed_hosts: std::sync::Arc::new(config.server.allowed_hosts.clone()),
    };
    let app = create_router(app_state);
    let listener = tokio::net::TcpListener::bind(web_addr).await.map_err(|e| {
        anyhow::anyhow!("cannot listen on {web_addr} (server.web_bind / web_port): {e}")
    })?;
    let web_shutdown = shutdown_rx.clone();
    let web_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let mut rx = web_shutdown;
                let _ = rx.changed().await;
            })
            .await
        {
            warn!("Web server error: {}", e);
        }
    });

    // Wait for a shutdown signal or an unexpected server exit
    let mut unexpected_exit = false;
    tokio::select! {
        _ = mllp_handle => {
            warn!("MLLP server stopped unexpectedly, initiating shutdown");
            unexpected_exit = true;
        }
        _ = web_handle => {
            warn!("Web server stopped unexpectedly, initiating shutdown");
            unexpected_exit = true;
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down gracefully");
        }
        _ = terminate_signal() => {
            info!("Received SIGTERM, shutting down gracefully");
        }
    }

    let _ = shutdown_tx.send(true);

    // Wait for active MLLP connections to drain — for the count to reach
    // zero, not for a single (possibly stale) notification (#241).
    let shutdown_timeout = std::time::Duration::from_secs(config.server.shutdown_timeout_secs);
    let active = stats.active_connections.load(Ordering::SeqCst);
    if active > 0 {
        info!(
            "Waiting up to {} seconds for {} MLLP connection(s) to drain...",
            config.server.shutdown_timeout_secs, active
        );
        if mllp::wait_for_drain(&stats, shutdown_timeout).await {
            info!("All MLLP connections drained");
        } else {
            warn!(
                "Shutdown timeout reached. Forcing exit with {} active connection(s)",
                stats.active_connections.load(Ordering::SeqCst)
            );
        }
    }

    info!("Harux stopped.");
    if unexpected_exit {
        // Non-zero exit so a service manager that restarts on failure does
        // restart it (#224).
        anyhow::bail!("a server task stopped unexpectedly");
    }
    Ok(())
}

fn prune_logs(target: &logging::LogTarget, keep: usize) {
    match target.prune(keep) {
        Ok(deleted) => {
            for path in deleted {
                info!("Deleted old log file {}", path.display());
            }
        }
        Err(e) => warn!(
            "Could not prune old log files in {}: {e}",
            target.dir.display()
        ),
    }
}

/// Resolves on SIGTERM — what `systemctl stop`, `docker stop` and `kill`
/// send (#225). Never resolves on non-Unix platforms, where Ctrl+C is the
/// shutdown signal.
async fn terminate_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                warn!("Could not install the SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    }
    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}
