use std::collections::HashMap;
use std::env;

use crate::error::{ProxyError, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: String,
    pub gemini_base_url: String,
    pub gemini_cookies: HashMap<String, String>,
    pub gemini_api_key: Option<String>,
    pub auth_token: Option<String>,
    pub default_model: String,
    pub max_retries: u32,
    pub gemini_models: Vec<String>,
    pub rate_limit: u64,
    pub cors_origins: Vec<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        let listen_addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());

        let gemini_base_url = env::var("GEMINI_BASE_URL")
            .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".into());

        let gemini_api_key = env::var("GEMINI_API_KEY").ok();

        let gemini_cookies = Self::parse_cookies(
            &env::var("GEMINI_COOKIES").unwrap_or_default(),
        );

        let auth_token = env::var("AUTH_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());

        let default_model = env::var("DEFAULT_MODEL")
            .unwrap_or_else(|_| "gemini-2.5-flash".into());

        let max_retries = env::var("MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);

        let gemini_models = env::var("GEMINI_MODELS")
            .ok()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_else(|| {
                vec![
                    "gemini-2.5-pro".to_string(),
                    "gemini-2.5-flash".to_string(),
                    "gemini-2.5-flash-lite".to_string(),
                    "gemini-3.5-flash".to_string(),
                    "gemini-3.5-flash-lite".to_string(),
                    "gemini-3.6-flash".to_string(),
                ]
            });

        let rate_limit = env::var("RATE_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);

        let cors_origins = env::var("CORS_ORIGINS")
            .unwrap_or_else(|_| "*".into())
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();

        Ok(Config {
            listen_addr,
            gemini_base_url,
            gemini_cookies,
            gemini_api_key,
            auth_token,
            default_model,
            max_retries,
            gemini_models,
            rate_limit,
            cors_origins,
        })
    }

    pub fn has_cookie_auth(&self) -> bool {
        self.gemini_cookies.contains_key("__Secure-1PSID")
            || self.gemini_cookies.contains_key("SID")
    }

    pub fn has_api_key(&self) -> bool {
        self.gemini_api_key.is_some()
    }

    pub fn validate(&self) -> Result<()> {
        if self.gemini_api_key.is_none() && self.gemini_cookies.is_empty() {
            return Err(ProxyError::Config(
                "Either GEMINI_API_KEY or GEMINI_COOKIES must be set".into(),
            ));
        }
        if !self.gemini_cookies.is_empty() && self.gemini_api_key.is_none() {
            let required = ["__Secure-1PSID", "SID"];
            for key in &required {
                if !self.gemini_cookies.contains_key(*key) {
                    tracing::warn!("Missing recommended cookie: {key} — auth may fail");
                }
            }
        }
        Ok(())
    }

    fn parse_cookies(cookie_str: &str) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for part in cookie_str.split(';') {
            let part = part.trim();
            if let Some((key, value)) = part.split_once('=') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                if !key.is_empty() {
                    map.insert(key, value);
                }
            }
        }
        map
    }
}
