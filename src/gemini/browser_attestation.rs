use std::collections::HashMap;
use std::process::Stdio;
use tokio::process::{Child as TokioChild, Command as TokioCommand};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc, Mutex, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, warn};
use urlencoding::decode;
use url::Url;

use crate::error::{ProxyError, Result};

/// Maximum time to wait for Chrome to print its DevTools WebSocket URL.
const CHROME_LAUNCH_TIMEOUT_SECS: u64 = 30;
/// Maximum time for a single CDP method call.
const CDP_COMMAND_TIMEOUT_SECS: u64 = 30;
/// Maximum time to wait for the page to load and emit a StreamGenerate request.
const STREAM_GENERATE_TIMEOUT_SECS: u64 = 60;

pub use super::web_frontend::BrowserAttestationPayload;

/// A headless Chromium driver used to obtain legitimate StreamGenerate payloads.
///
/// The driver launches a single Chromium process and keeps one page alive.  For
/// each turn it navigates (or reloads) `https://gemini.google.com/app`, injects
/// the configured cookies, simulates the user sending the current prompt, and
/// intercepts the outgoing `StreamGenerate` request.  The intercepted payload
/// already contains valid conversation state (slot 2), attestation tokens
/// (slots 3/4), and the turn counter (slot 17).  The proxy can replay it
/// directly, overriding only the model category (slot 30) and request UUID
/// (slot 59) if desired.
pub struct BrowserAttestationClient {
    chrome_path: String,
    process: Mutex<Option<TokioChild>>,
    conn: Mutex<Option<CdpConnection>>,
    /// The conversation id currently loaded in the browser page.  When it
    /// changes we reload the page so the browser starts a fresh conversation.
    loaded_conversation_id: Mutex<Option<String>>,
    /// Unique temporary user-data-dir for this client instance.  Kept so we can
    /// clean it up on drop/close.
    user_data_dir: std::path::PathBuf,
}

impl BrowserAttestationClient {
    /// Create a new browser attestation client.  `chrome_path` is the path to a
    /// Chromium/Chrome executable.  The browser process is not started until the
    /// first call to `get_stream_generate_payload`.
    pub fn new(chrome_path: String) -> Self {
        let user_data_dir = std::env::temp_dir()
            .join(format!("gemini-proxy-chrome-data-{}", uuid::Uuid::new_v4()));
        Self {
            chrome_path,
            process: Mutex::new(None),
            conn: Mutex::new(None),
            loaded_conversation_id: Mutex::new(None),
            user_data_dir,
        }
    }

    /// Ensure the browser is running and connected.
    async fn ensure_running(&self) -> Result<()> {
        let mut proc_guard = self.process.lock().await;
        let mut conn_guard = self.conn.lock().await;

        // If process died, clean up and restart.
        if let Some(ref mut child) = *proc_guard {
            match child.try_wait() {
                Ok(Some(_)) => {
                    warn!("Browser process exited; restarting");
                    let _ = child.kill().await;
                    *proc_guard = None;
                    *conn_guard = None;
                    *self.loaded_conversation_id.lock().await = None;
                }
                Ok(None) => {
                    if conn_guard.is_some() {
                        return Ok(());
                    }
                    // Process alive but no connection; kill and reconnect.
                    let _ = child.kill().await;
                    *proc_guard = None;
                    *self.loaded_conversation_id.lock().await = None;
                }
                Err(e) => {
                    return Err(ProxyError::Config(format!(
                        "Failed to check browser process status: {e}"
                    )));
                }
            }
        }

        let (child, browser_ws_url) = launch_chrome(&self.chrome_path, &self.user_data_dir).await?;

        // The DevTools URL printed by Chrome is a *browser* target.  We must
        // create and attach to a page target before we can navigate and
        // intercept network requests.
        let page_ws_url = find_or_create_page_ws_url(&browser_ws_url).await?;
        let conn = CdpConnection::connect(&page_ws_url).await?;

        // Enable required CDP domains.
        conn.call("Runtime.enable", json!({})).await?;
        conn.call("Network.enable", json!({"maxResourceBufferSize": 1024 * 1024})).await?;
        conn.call("Page.enable", json!({})).await?;

        *proc_guard = Some(child);
        *conn_guard = Some(conn);
        Ok(())
    }

    /// Stop the browser process and clear the connection.
    pub async fn close(&self) {
        let mut conn = self.conn.lock().await;
        *conn = None;
        *self.loaded_conversation_id.lock().await = None;
        let mut proc = self.process.lock().await;
        if let Some(mut child) = proc.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        let _ = tokio::fs::remove_dir_all(&self.user_data_dir).await;
    }


    /// Obtain a fresh StreamGenerate payload for `prompt`.
    ///
    /// `cookies` are injected into the browser before navigation.  If a
    /// `conversation_id` is supplied and differs from the conversation currently
    /// loaded in the browser page, the page is reloaded to start a new
    /// conversation.  The returned payload can be replayed by the proxy.
    pub async fn get_stream_generate_payload(
        &self,
        cookies: &HashMap<String, String>,
        prompt: &str,
        conversation_id: Option<&str>,
    ) -> Result<BrowserAttestationPayload> {
        self.get_stream_generate_payload_with_image(cookies, prompt, conversation_id, None)
            .await
    }

    /// Capture a StreamGenerate payload, optionally by attaching a local image
    /// file to the prompt. This is used to reverse-engineer how the Gemini web
    /// frontend represents images/files in slot 0 and any side-channel headers.
    pub async fn get_stream_generate_payload_with_image(
        &self,
        cookies: &HashMap<String, String>,
        prompt: &str,
        conversation_id: Option<&str>,
        image_path: Option<&str>,
    ) -> Result<BrowserAttestationPayload> {
        self.ensure_running().await?;

        let conn = self.conn.lock().await;
        let conn = conn.as_ref().ok_or_else(|| {
            ProxyError::Internal("Browser connection not available".into())
        })?;

        let mut loaded = self.loaded_conversation_id.lock().await;
        let needs_reload = loaded.as_deref() != conversation_id || conversation_id.is_none();

        // Navigate to /app (or reload) so the browser starts the correct
        // conversation context.
        let nav_url = "https://gemini.google.com/app?hl=en";
        if needs_reload {
            debug!(url = %nav_url, "Browser navigating to Gemini");
            conn.call("Page.navigate", json!({"url": nav_url})).await?;
            wait_for_event(conn, "Page.loadEventFired", Duration::from_secs(45)).await?;
            *loaded = conversation_id.map(|s| s.to_string());
        }

        // Wait for the Angular app to boot and render the input area.  The
        // SSR HTML contains styles but the interactive DOM is built by JS.
        conn.call(
            "Runtime.evaluate",
            json!({
                "expression": r#"
                    new Promise((resolve) => {
                        const deadline = Date.now() + 15000;
                        const check = () => {
                            const textarea = document.querySelector('.initial-input-area textarea, .initial-input-area-container textarea, textarea[placeholder*="Ask"], .ql-editor[contenteditable="true"]');
                            if (textarea && textarea.offsetParent !== null) {
                                resolve(true);
                                return;
                            }
                            if (Date.now() < deadline) {
                                setTimeout(check, 200);
                            } else {
                                resolve(false);
                            }
                        };
                        check();
                    })
                "#,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await?;

        // Inject cookies.  We do this after navigation so the page context is
        // ready.  Setting cookies before the first navigation also works, but
        // repeating it is harmless.
        for (name, value) in cookies {
            let res = conn
                .call(
                    "Network.setCookie",
                    json!({
                        "name": name,
                        "value": value,
                        "domain": "gemini.google.com",
                        "path": "/",
                        "secure": true,
                        "httpOnly": name.contains("SID"),
                        "sameSite": "None",
                    }),
                )
                .await;
            if let Err(ref e) = res {
                warn!(cookie = %name, error = %e, "Failed to set cookie in browser");
            }
        }

        // If not reloaded we still want to be sure we are on /app.
        if !needs_reload {
            let loc = conn
                .call("Runtime.evaluate", json!({"expression": "location.href"}))
                .await?;
            let href = loc
                .get("result")
                .and_then(|r| r.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !href.contains("gemini.google.com/app") {
                conn.call("Page.navigate", json!({"url": nav_url})).await?;
                wait_for_event(conn, "Page.loadEventFired", Duration::from_secs(30)).await?;
            }
        }

        // Clear any previous interception state.
        conn.call("Network.setRequestInterception", json!({"patterns": []}))
            .await
            .ok();

        // Subscribe to Network.requestWillBeSent before simulating input so we
        // do not miss the StreamGenerate request.
        let mut events = conn.subscribe();

        let simulate_js = if let Some(image) = image_path {
            // For file:// URLs the browser must be allowed to read them; we
            // copy the image into the user-data-dir and reference it there.
            let allowed_path = self.copy_image_to_user_data_dir(image).await?;
            let allowed_path_str = allowed_path.to_string_lossy();
            let mut js = include_str!("browser_attestation_simulate_image.js").to_string();
            js = js.replace("__IMAGE_PATH__", &allowed_path_str.replace('\\', "/"));
            js = js.replace("__PROMPT__", &json_string_escape(prompt));
            js
        } else {
            let escaped_prompt = json_string_escape(prompt);
            let mut js = include_str!("browser_attestation_simulate.js").to_string();
            js = js.replace("__PROMPT__", &escaped_prompt);
            js
        };

        let eval_res = conn
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": simulate_js,
                    "awaitPromise": true,
                    "returnByValue": true,
                }),
            )
            .await?;
        if let Some(exception) = eval_res.get("exceptionDetails") {
            let text = exception
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(ProxyError::GeminiApi(format!(
                "Browser failed to simulate user input: {text}"
            )));
        }
        let sim_ok = eval_res
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !sim_ok {
            // Retrieve client-side logs so failures are diagnosable.
            let logs = conn
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": "(typeof __geminiSimLogs !== 'undefined') ? JSON.stringify(__geminiSimLogs) : '[]'",
                        "returnByValue": true,
                    }),
                )
                .await;
            let log_text = match logs {
                Ok(v) => v
                    .get("result")
                    .and_then(|r| r.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("[]")
                    .to_string(),
                Err(_) => "[]".to_string(),
            };
            error!(logs = %log_text, "Browser could not find the Gemini input element");
            return Err(ProxyError::GeminiApi(format!(
                "Browser could not find the Gemini input element. JS logs: {log_text}"
            )));
        }

        // Wait for the StreamGenerate request.
        let request_id = wait_for_stream_generate_request(&mut events, conn).await?;
        drop(events);

        // Fetch request details so we can inspect headers too.
        let request_data = conn
            .call("Network.getRequestPostData", json!({"requestId": request_id}))
            .await?;
        let body_str = request_data
            .get("postData")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProxyError::GeminiApi("StreamGenerate body missing".into()))?;

        let inner_req_list = parse_f_req_inner_list(body_str)?;

        debug!(
            slots = inner_req_list.len(),
            "Extracted inner_req_list from browser StreamGenerate"
        );

        Ok(BrowserAttestationPayload { inner_req_list })
    }

    /// Copy an image from the host filesystem into the browser profile directory
    /// so the headless page can read it via file://.
    async fn copy_image_to_user_data_dir(&self, src: &str) -> Result<std::path::PathBuf> {
        let src_path = std::path::Path::new(src);
        if !src_path.exists() {
            return Err(ProxyError::Config(format!("Image not found: {src}")));
        }
        let dest = self.user_data_dir.join("upload.png");
        tokio::fs::copy(src_path, &dest)
            .await
            .map_err(|e| ProxyError::Config(format!("Failed to copy image for browser: {e}")))?;
        Ok(dest)
    }
}

/// Escape a string for safe insertion into a double-quoted JS string literal.
fn json_string_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}



/// Launch Chrome and return the child process plus the DevTools WebSocket URL.
async fn launch_chrome(
    chrome_path: &str,
    user_data_dir: &std::path::Path,
) -> Result<(TokioChild, String)> {
    let mut child = TokioCommand::new(chrome_path)
        .arg("--headless")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg("--disable-background-networking")
        .arg("--disable-background-timer-throttling")
        .arg("--disable-renderer-backgrounding")
        .arg("--disable-features=TranslateUI")
        .arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", user_data_dir.display()))
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| ProxyError::Config(format!("Failed to launch Chrome ({chrome_path}): {e}")))?;

    let stderr = child.stderr.take().ok_or_else(|| {
        ProxyError::Config("Failed to capture Chrome stderr".into())
    })?;
    let reader = BufReader::new(stderr);
    let mut lines = reader.lines();

    let ws_url = timeout(Duration::from_secs(CHROME_LAUNCH_TIMEOUT_SECS), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(start) = line.find("DevTools listening on ") {
                let url = line[start + "DevTools listening on ".len()..].trim();
                if !url.is_empty() {
                    return Some(url.to_string());
                }
            }
            if line.contains("error") || line.contains("ERROR") || line.contains("FATAL") {
                debug!(chrome_log = %line, "Chrome log");
            }
        }
        None
    })
    .await
    .map_err(|_| ProxyError::Config("Timeout waiting for Chrome DevTools URL".into()))?
    .ok_or_else(|| ProxyError::Config("Chrome did not print DevTools URL".into()))?;

    debug!(ws_url = %ws_url, "Chrome launched");
    Ok((child, ws_url))
}

/// Connect to the browser-level CDP target, create a new page, and return its
/// WebSocket debugger URL.
async fn find_or_create_page_ws_url(browser_ws_url: &str) -> Result<String> {
    let http_url = browser_ws_url
        .replace("ws://", "http://")
        .replace("wss://", "https://");
    let parsed = Url::parse(&http_url).map_err(|e| {
        ProxyError::Config(format!("Failed to parse Chrome DevTools URL: {e}"))
    })?;

    let client = reqwest::Client::new();
    let list_url = format!(
        "{}://{}{}/json/list",
        parsed.scheme(),
        parsed.host_str().unwrap_or("127.0.0.1"),
        if let Some(port) = parsed.port() {
            format!(":{port}")
        } else {
            String::new()
        }
    );

    let list_resp = client
        .get(&list_url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| ProxyError::Config(format!("Failed to list CDP targets: {e}")))?;
    let targets: Vec<Value> = list_resp.json().await.map_err(|e| {
        ProxyError::Config(format!("Failed to parse CDP target list: {e}"))
    })?;

    // Prefer an existing page.
    for target in &targets {
        if target.get("type").and_then(|v| v.as_str()) == Some("page")
            && let Some(ws) = target.get("webSocketDebuggerUrl").and_then(|v| v.as_str())
        {
            debug!(page_ws = %ws, "Reusing existing CDP page target");
            return Ok(ws.to_string());
        }
    }

    // Otherwise create a new page.
    let new_url = format!(
        "{}://{}{}/json/new",
        parsed.scheme(),
        parsed.host_str().unwrap_or("127.0.0.1"),
        if let Some(port) = parsed.port() {
            format!(":{port}")
        } else {
            String::new()
        }
    );
    let new_resp = client
        .put(&new_url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| ProxyError::Config(format!("Failed to create CDP page target: {e}")))?;
    let new_target: Value = new_resp.json().await.map_err(|e| {
        ProxyError::Config(format!("Failed to parse new CDP target: {e}"))
    })?;
    let ws_url = new_target
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProxyError::Config("New CDP target missing WebSocket URL".into()))?;
    debug!(page_ws = %ws_url, "Created new CDP page target");
    Ok(ws_url.to_string())
}

/// Wait until an event with the given method arrives.
async fn wait_for_event(
    conn: &CdpConnection,
    method: &str,
    within: Duration,
) -> Result<()> {
    let mut sub = conn.subscribe();
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        match timeout(remaining, sub.recv()).await {
            Ok(Ok(msg)) => {
                if msg.get("method").and_then(|v| v.as_str()) == Some(method) {
                    return Ok(());
                }
            }
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    Err(ProxyError::GeminiApi(format!("Timed out waiting for {method}")))
}

/// Wait for a Network.requestWillBeSent event whose URL contains
/// `StreamGenerate`, then return the CDP `requestId`.
async fn wait_for_stream_generate_request(
    events: &mut broadcast::Receiver<Value>,
    conn: &CdpConnection,
) -> Result<String> {
    let deadline = Duration::from_secs(STREAM_GENERATE_TIMEOUT_SECS);
    let until = tokio::time::Instant::now() + deadline;

    while tokio::time::Instant::now() < until {
        let remaining = until - tokio::time::Instant::now();
        let msg = match timeout(remaining, events.recv()).await {
            Ok(Ok(m)) => m,
            Ok(Err(_)) => break,
            Err(_) => break,
        };

        let method = msg.get("method").and_then(|v| v.as_str());
        if method != Some("Network.requestWillBeSent") {
            continue;
        }

        let params = msg.get("params").ok_or_else(|| {
            ProxyError::Internal("CDP event missing params".into())
        })?;
        let request = params.get("request").ok_or_else(|| {
            ProxyError::Internal("CDP request missing".into())
        })?;
        let url = request
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if url.contains("StreamGenerate") {
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ProxyError::Internal("CDP requestId missing".into()))?;

            // Post data may not be available immediately.  Wait for the
            // request to finish loading, then retry getRequestPostData without
            // consuming the matched event until the body is present.
            let mut post_data_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                match conn
                    .call("Network.getRequestPostData", json!({"requestId": request_id}))
                    .await
                {
                    Ok(data) => {
                        if data.get("postData").is_some() {
                            return Ok(request_id.to_string());
                        }
                    }
                    Err(e) => {
                        debug!(error = %e, "Post data not yet available, retrying");
                    }
                }

                // Wait for the request to finish, or a short poll interval.
                let remaining = post_data_deadline - tokio::time::Instant::now();
                if remaining.is_zero() {
                    break;
                }
                match timeout(remaining, events.recv()).await {
                    Ok(Ok(m)) => {
                        if m.get("method").and_then(|v| v.as_str())
                            == Some("Network.loadingFinished")
                        {
                            post_data_deadline =
                                tokio::time::Instant::now() + Duration::from_millis(500);
                        }
                    }
                    Ok(Err(_)) => break,
                    Err(_) => break,
                }
            }
        }
    }

    Err(ProxyError::GeminiApi(
        "Timed out waiting for StreamGenerate request from browser".into(),
    ))
}

/// Parse the `f.req` form body from a captured StreamGenerate request and return
/// the inner 97-slot `inner_req_list`.
fn parse_f_req_inner_list(body: &str) -> Result<Vec<Value>> {
    let mut f_req: Option<String> = None;
    for pair in body.split('&') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let value = kv.next().unwrap_or("");
        if key == "f.req" {
            f_req = Some(decode(value).unwrap_or_else(|_| value.into()).into_owned());
            break;
        }
    }

    let f_req = f_req.ok_or_else(|| {
        ProxyError::GeminiApi("Captured StreamGenerate body missing f.req".into())
    })?;

    let outer: Value = serde_json::from_str(&f_req).map_err(|e| {
        ProxyError::GeminiApi(format!("Failed to parse captured f.req JSON: {e}"))
    })?;

    // Shape: [null, "[<inner_req_list>]"]
    let inner_json_str = outer
        .as_array()
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ProxyError::GeminiApi("Captured f.req has unexpected shape".into())
        })?;

    let inner_req_list: Vec<Value> = serde_json::from_str(inner_json_str).map_err(|e| {
        ProxyError::GeminiApi(format!("Failed to parse captured inner_req_list: {e}"))
    })?;

    if inner_req_list.len() < 97 {
        return Err(ProxyError::GeminiApi(format!(
            "Captured inner_req_list has only {} slots, expected 97",
            inner_req_list.len()
        )));
    }

    Ok(inner_req_list)
}

/// A minimal CDP client over a WebSocket connection.
struct CdpConnection {
    next_id: AtomicI64,
    write: Mutex<mpsc::UnboundedSender<(Value, oneshot::Sender<Result<Value>>)>>,
    event_tx: broadcast::Sender<Value>,
}

impl CdpConnection {
    async fn connect(ws_url: &str) -> Result<Self> {
        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| ProxyError::Config(format!("Failed to connect to Chrome DevTools: {e}")))?;

        let (mut write_half, mut read_half) = ws_stream.split();
        type PendingResponse = oneshot::Sender<Result<Value>>;
        type CommandChannel = (Value, PendingResponse);
        let (cmd_tx, mut cmd_rx): (mpsc::UnboundedSender<CommandChannel>, mpsc::UnboundedReceiver<CommandChannel>) =
            mpsc::unbounded_channel();
        let (event_tx, _event_rx): (broadcast::Sender<Value>, _) = broadcast::channel(256);
        let event_tx_clone = event_tx.clone();

        tokio::spawn(async move {
            let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>> =
                Arc::new(Mutex::new(HashMap::new()));
            let pending_read = pending.clone();

            // Read loop.
            let read_handle = tokio::spawn(async move {
                while let Some(msg) = read_half.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                if value.get("id").is_some() {
                                    if let Some(id) = value.get("id").and_then(|v| v.as_i64()) {
                                        let mut map = pending_read.lock().await;
                                        if let Some(sender) = map.remove(&id) {
                                            if value.get("error").is_some() {
                                                let err_msg = value
                                                    .get("error")
                                                    .and_then(|e| e.get("message"))
                                                    .and_then(|m| m.as_str())
                                                    .unwrap_or("CDP error");
                                                let _ = sender.send(Err(ProxyError::GeminiApi(
                                                    err_msg.to_string(),
                                                )));
                                            } else {
                                                let _ = sender.send(Ok(value));
                                            }
                                        }
                                    }
                                } else if value.get("method").is_some() {
                                    let _ = event_tx_clone.send(value);
                                }
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(e) => {
                            error!(error = %e, "CDP WebSocket read error");
                            break;
                        }
                    }
                }
            });

            // Write loop.
            while let Some((value, sender)) = cmd_rx.recv().await {
                let id = value
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .expect("command must have id");
                {
                    let mut map = pending.lock().await;
                    map.insert(id, sender);
                }
                let text = serde_json::to_string(&value).unwrap_or_default();
                if let Err(e) = write_half.send(Message::Text(text.into())).await {
                    let mut map = pending.lock().await;
                    if let Some(sender) = map.remove(&id) {
                        let _ = sender.send(Err(ProxyError::GeminiApi(format!(
                            "Failed to send CDP command: {e}"
                        ))));
                    }
                }
            }

            let _ = read_handle.await;
        });

        Ok(Self {
            next_id: AtomicI64::new(1),
            write: Mutex::new(cmd_tx),
            event_tx,
        })
    }

    fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.event_tx.subscribe()
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = json!({
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        {
            let guard = self.write.lock().await;
            guard
                .send((msg, tx))
                .map_err(|_| ProxyError::Internal("CDP command channel closed".into()))?;
        }

        timeout(Duration::from_secs(CDP_COMMAND_TIMEOUT_SECS), rx)
            .await
            .map_err(|_| ProxyError::GeminiApi(format!("CDP command {method} timed out")))?
            .map_err(|_| ProxyError::Internal("CDP response channel dropped".into()))?
    }
}
