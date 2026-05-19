use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::get;
use tower_http::trace::TraceLayer;

use nix_search_config::AppConfig;
use nix_search_index::IndexStore;

mod handlers;
mod request;
mod scripts;
mod templates;
mod urls;

const DEFAULT_LIMIT: usize = 20;
const RECONCILE_EVENTS_URL: &str = "/-/state/events";

#[derive(Debug, Clone)]
struct AppState {
    config: Arc<AppConfig>,
    index_path: Arc<PathBuf>,
}

pub async fn serve(config: AppConfig) -> Result<()> {
    let index_store = IndexStore::new(&config.data.index_dir);
    let index_path = index_store.current_path().with_context(|| {
        format!(
            "failed to locate current index in {}; run `nix-search update` first",
            config.data.index_dir.display()
        )
    })?;

    let addr: SocketAddr =
        config.server.listen.parse().with_context(|| {
            format!("failed to parse listen address {:?}", config.server.listen)
        })?;

    let state = AppState {
        config: Arc::new(config),
        index_path: Arc::new(index_path),
    };

    let app = Router::new()
        .route("/-/health", get(handlers::health))
        .route(RECONCILE_EVENTS_URL, get(handlers::state_events))
        .route("/", get(handlers::root_page))
        .route("/{source}", get(handlers::source_page))
        .route("/{source}/{*entry}", get(handlers::entry_page))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    tracing::info!("serving nix-search web UI at http://{addr}");

    axum::serve(listener, app)
        .await
        .context("web server failed")?;

    Ok(())
}
