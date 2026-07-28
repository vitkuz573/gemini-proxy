use tracing_subscriber::EnvFilter;
use tower_http::cors::{CorsLayer, Any};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = gemini2openai::config::Config::from_env()?;

    tracing::info!("Starting gemini2openai proxy on {}", config.listen_addr);
    tracing::info!("Auth mode: {}", if config.has_api_key() { "API Key" } else { "Cookie" });

    let auth = gemini2openai::gemini::auth::GeminiAuth::from_config(&config)?;
    let gemini_client = gemini2openai::gemini::client::GeminiClient::new(auth, config.gemini_base_url.clone());
    let app = gemini2openai::openai::server::create_router(gemini2openai::openai::server::AppState {
        gemini_client,
        config: config.clone(),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    let app = app.layer(cors);

    let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
