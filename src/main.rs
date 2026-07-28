use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;
use tower_http::cors::{CorsLayer, Any};

#[derive(Clone)]
struct RateLimitEntry {
    timestamps: Vec<Instant>,
}

#[derive(Clone)]
struct RateLimitState {
    entries: Arc<Mutex<HashMap<String, RateLimitEntry>>>,
    max_requests: u64,
    window: Duration,
}

async fn rate_limit_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<RateLimitState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let ip = addr.ip().to_string();
    let now = Instant::now();
    let window_start = now - state.window;

    let mut entries = state.entries.lock().await;
    let entry = entries.entry(ip).or_insert_with(|| RateLimitEntry {
        timestamps: Vec::new(),
    });
    entry.timestamps.retain(|t| *t > window_start);

    if entry.timestamps.len() as u64 >= state.max_requests {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    entry.timestamps.push(now);
    drop(entries);

    Ok(next.run(request).await)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = gemini2openai::config::Config::from_env()?;
    config.validate()?;

    tracing::info!("Starting gemini2openai proxy on {}", config.listen_addr);
    tracing::info!("Auth mode: {}", if config.has_api_key() { "API Key" } else { "Cookie" });

    let auth = gemini2openai::gemini::auth::GeminiAuth::from_config(&config)?;
    let gemini_client = gemini2openai::gemini::client::GeminiClient::new(auth, config.gemini_base_url.clone(), config.gemini_models.clone())?;
    let app = gemini2openai::openai::server::create_router(gemini2openai::openai::server::AppState {
        gemini_client,
        config: config.clone(),
    });

    let cors = if config.cors_origins.iter().any(|o| o == "*") {
        CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any)
    } else {
        let origins: Vec<_> = config.cors_origins.iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new().allow_origin(origins).allow_methods(Any).allow_headers(Any)
    };

    let rate_limit_state = RateLimitState {
        entries: Arc::new(Mutex::new(HashMap::new())),
        max_requests: config.rate_limit,
        window: Duration::from_secs(60),
    };

    let app = app
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(
            rate_limit_state,
            rate_limit_middleware,
        ));

    let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
    let shutdown_signal = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
        tracing::info!("Shutdown signal received");
    };
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    Ok(())
}
