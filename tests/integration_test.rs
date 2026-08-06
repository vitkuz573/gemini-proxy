use std::collections::HashMap;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use gemini_proxy::config::Config;
use gemini_proxy::gemini::auth::GeminiAuth;
use gemini_proxy::gemini::client::GeminiClient;
use gemini_proxy::openai::server::{create_router, AppState};

fn make_config() -> Config {
    Config {
        listen_addr: "0.0.0.0:3000".into(),
        gemini_base_url: "http://localhost:0".into(),
        gemini_cookies: HashMap::new(),
        gemini_api_key: Some("test_key".into()),
        auth_token: None,
        max_retries: 2,
        rate_limit: 60,
        cors_origins: vec!["*".to_string()],
        gemini_headless_browser: None,
        chrome_path: None,
        push_id: None,
    }
}

fn make_auth() -> GeminiAuth {
    GeminiAuth {
        cookies: HashMap::new(),
        api_key: Some("test_key".into()),
        chrome_path: None,
    }
}

fn make_client() -> GeminiClient {
    GeminiClient::new(make_auth(), "http://localhost:0".into()).unwrap()
}

fn app_state() -> AppState {
    AppState {
        gemini_client: make_client(),
        config: make_config(),
    }
}

#[tokio::test]
async fn test_health_returns_200() {
    let app = create_router(app_state());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_root_returns_info() {
    let app = create_router(app_state());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["name"], "gemini-proxy");
    assert_eq!(json["version"], "0.1.0");
}

#[tokio::test]
async fn test_chat_completions_without_auth_returns_401_when_auth_required() {
    let mut config = make_config();
    config.auth_token = Some("secret".into());

    let state = AppState {
        gemini_client: make_client(),
        config,
    };
    let app = create_router(state);

    let body = json!({
        "model": "current-model",
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_chat_completions_with_wrong_auth_returns_401() {
    let mut config = make_config();
    config.auth_token = Some("secret".into());

    let state = AppState {
        gemini_client: make_client(),
        config,
    };
    let app = create_router(state);

    let body = json!({
        "model": "current-model",
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", "Bearer wrong_token")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_chat_completions_with_valid_auth_reaches_upstream() {
    let mut config = make_config();
    config.auth_token = Some("secret".into());

    let state = AppState {
        gemini_client: make_client(),
        config,
    };
    let app = create_router(state);

    let body = json!({
        "model": "current-model",
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", "Bearer secret")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Will fail with 502 because localhost:0 is not a real Gemini server
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn test_models_endpoint_reaches_upstream() {
    let app = create_router(app_state());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Fails with 502 because localhost:0 is not a real Gemini server
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn test_get_model_endpoint_reaches_upstream() {
    let app = create_router(app_state());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/models/current-model")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Fails with 502 because localhost:0 is not a real Gemini server
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn test_chat_completions_empty_model_returns_400() {
    let state = app_state();
    let app = create_router(state);

    let body = json!({
        "model": "",
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_chat_completions_invalid_json_returns_400() {
    let app = create_router(app_state());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from("not json"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_auth_token_none_allows_unauthenticated() {
    let state = app_state();
    let app = create_router(state);

    let body = json!({
        "model": "current-model",
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // No auth_token configured, so auth is bypassed. Fails upstream (502).
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}
