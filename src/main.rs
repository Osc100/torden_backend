mod api;
mod chats;
mod structs;
mod utils;
use crate::chats::{agent_pool_task, ws_handler};
use crate::structs::AppState;
use axum::{
    routing::{get, post},
    Extension, Router,
};
use chats::AgentPoolAction;
use sqlx::postgres::PgPoolOptions;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Load environment variables from .env file.
    dotenv::dotenv().ok();

    // Initialize tracing subscriber.
    tracing_subscriber::registry()
        // .with(tracing_subscriber::EnvFilter::new(
        //     std::env::var("RUST_LOG").unwrap_or_else(|_| "example_chat=trace".into()),
        // ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Arc is a thread-safe reference-counted pointer, used to share state between threads.
    let app_state = Arc::new(AppState {
        // Mutex is a mutual exclusion lock, used to synchronize access to the HashMaps.
        channels: Arc::new(Mutex::new(HashMap::new())),
        agent_pool: Arc::new(Mutex::new(HashMap::new())),
    });

    // Connection pool to the database, pased to the router as Extension.
    let pool = PgPoolOptions::new()
        .max_connections(50)
        .connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();

    // Inserting test company as a dev dependency.
    sqlx::query!(
        "INSERT INTO company (id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        1,
        "Test Company"
    )
    .execute(&pool)
    .await
    .unwrap();

    let (agent_tx, agent_rx) = mpsc::channel::<AgentPoolAction>(100);
    let agent_task = agent_pool_task(app_state.clone(), agent_rx);

    let app = Router::new()
        .route("/chat", get(ws_handler))
        .route("/login", post(api::login_handler))
        .route("/register", post(api::register_handler))
        .with_state(app_state)
        .layer(Extension(pool))
        .layer(Extension(agent_tx))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive().allow_methods(Any));

    let addr = SocketAddr::from(([0, 0, 0, 0], 4000));
    tracing::debug!("listening on {}", addr);

    axum::Server::bind(&addr)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();

    agent_task.abort();
}
