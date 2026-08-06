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
    pub max_retries: u32,
    pub rate_limit: u64,
    pub cors_origins: Vec<String>,
    /// Optional path to a Chrome/Chromium executable used to obtain legitimate
    /// StreamGenerate attestation payloads in cookie-auth mode.
    pub gemini_headless_browser: Option<String>,
    /// Alias for `gemini_headless_browser`.  If both are set,
    /// `GEMINI_HEADLESS_BROWSER` wins.
    pub chrome_path: Option<String>,
    /// Google upload feed ID used by the Gemini web frontend when pushing
    /// attached files to `push.clients6.google.com/upload/`.  Observed value is
    /// usually `feeds/<...>`.  Can be overridden per-account via the
    /// `GEMINI_PUSH_ID` environment variable.
    pub push_id: Option<String>,
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

        let max_retries = env::var("MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);

        let rate_limit = env::var("RATE_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);

        let cors_origins = env::var("CORS_ORIGINS")
            .unwrap_or_else(|_| "*".into())
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();

        let gemini_headless_browser = env::var("GEMINI_HEADLESS_BROWSER")
            .ok()
            .filter(|s| !s.is_empty());
        let chrome_path = env::var("CHROME_PATH")
            .ok()
            .filter(|s| !s.is_empty());

        let push_id = env::var("GEMINI_PUSH_ID")
            .ok()
            .filter(|s| !s.is_empty());

        Ok(Config {
            listen_addr,
            gemini_base_url,
            gemini_cookies,
            gemini_api_key,
            auth_token,
            max_retries,
            rate_limit,
            cors_origins,
            gemini_headless_browser,
            chrome_path,
            push_id,
        })
    }

    /// Effective Chrome/Chromium executable path.  Returns `GEMINI_HEADLESS_BROWSER`
    /// if set, otherwise `CHROME_PATH`.
    pub fn browser_path(&self) -> Option<&str> {
        self.gemini_headless_browser
            .as_deref()
            .or(self.chrome_path.as_deref())
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
