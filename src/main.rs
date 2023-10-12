mod api;
mod chats;
mod structs;
mod utils;
use crate::chats::ws_handler;
use crate::structs::AppState;
use axum::{
    routing::{get, post},
    Extension, Router,
};
use sqlx::postgres::PgPoolOptions;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Load environment variables from .env file.
    dotenv::dotenv().ok();

    // Initialize tracing subscriber.
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "example_chat=trace".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Arc is a thread-safe reference-counted pointer, used to share state between threads.
    let app_state = Arc::new(AppState {
        channels: Mutex::new(HashMap::new()),
    });

    // Connection pool to the database, pased to the router as Extension.
    let pool = PgPoolOptions::new()
        .max_connections(50)
        .connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();

    // Insert test company.
    sqlx::query!(
        "INSERT INTO company (id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        1,
        "Test Company"
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = Router::new()
        .route("/", get(ws_handler))
        .with_state(app_state)
        .layer(Extension(pool));

    let addr = SocketAddr::from(([0, 0, 0, 0], 4000));
    tracing::debug!("listening on {}", addr);

    axum::Server::bind(&addr)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}
