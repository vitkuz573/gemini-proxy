use std::collections::HashMap;

use crate::config::Config;
use crate::error::{ProxyError, Result};

#[derive(Debug, Clone)]
pub struct GeminiAuth {
    pub cookies: HashMap<String, String>,
    pub api_key: Option<String>,
}

impl GeminiAuth {
    pub fn from_config(config: &Config) -> Result<Self> {
        let cookies = config.gemini_cookies.clone();
        let api_key = config.gemini_api_key.clone();

        if !config.has_cookie_auth() && !config.has_api_key() {
            return Err(ProxyError::Config(
                "Either GEMINI_COOKIES or GEMINI_API_KEY must be set".into(),
            ));
        }

        Ok(GeminiAuth { cookies, api_key })
    }

    pub fn is_cookie_auth(&self) -> bool {
        self.api_key.is_none() && !self.cookies.is_empty()
    }

    pub fn is_api_key_auth(&self) -> bool {
        self.api_key.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_auth() -> GeminiAuth {
        let mut cookies = HashMap::new();
        cookies.insert("__Secure-1PAPISID".into(), "test_sapisid".into());
        cookies.insert("__Secure-1PSID".into(), "test_psid".into());
        GeminiAuth {
            cookies,
            api_key: None,
        }
    }

    #[test]
    fn test_cookie_auth_detection() {
        let auth = make_auth();
        assert!(auth.is_cookie_auth());
        assert!(!auth.is_api_key_auth());
    }

    #[test]
    fn test_api_key_auth_detection() {
        let auth = GeminiAuth {
            cookies: HashMap::new(),
            api_key: Some("test_key".into()),
        };
        assert!(!auth.is_cookie_auth());
        assert!(auth.is_api_key_auth());
    }

    #[test]
    fn test_no_auth_fails() {
        let config = Config {
            listen_addr: "0.0.0.0:3000".into(),
            gemini_base_url: "https://generativelanguage.googleapis.com".into(),
            gemini_cookies: HashMap::new(),
            gemini_api_key: None,
            auth_token: None,
            default_model: "current-model".into(),
            max_retries: 2,

            rate_limit: 60,
            cors_origins: vec!["*".to_string()],
        };
        let result = GeminiAuth::from_config(&config);
        assert!(result.is_err());
    }
}
