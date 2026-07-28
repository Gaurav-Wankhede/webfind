pub mod handlers;
pub mod state;
pub mod templates;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, routing::get};
use tower_http::services::ServeDir;

use crate::api::ApiState;
use crate::gui::state::GuiState;

/// Build the GUI Axum router.
pub fn app(state: Arc<ApiState>) -> Router {
    let gui_state = GuiState::from_api_state(state);

    Router::new()
        .route("/", get(handlers::home))
        .route("/web/search", get(handlers::search))
        .route("/api/web/suggest", get(handlers::suggest))
        .route("/api/web/categories", get(handlers::categories))
        .route("/api/web/visit", get(handlers::visit))
        .route("/api/web/research/stream", get(handlers::research_stream))
        .route("/about", get(handlers::about))
        .nest_service("/assets", ServeDir::new("assets"))
        .with_state(gui_state)
}

/// Run the GUI server on the given port.
pub async fn run_server(state: Arc<ApiState>, port: u16) -> anyhow::Result<()> {
    let app = app(state);
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("WebFind GUI listening on http://{}", addr);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
