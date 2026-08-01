use std::collections::HashMap;
use std::time::Duration;

use gemini_proxy::gemini::browser_attestation::BrowserAttestationClient;
use gemini_proxy::gemini::web_frontend::{extract_conversation_state, WebConversationState};
use reqwest::Client;
use serde_json::{json, Value};

const WEB_BASE_URL: &str = "https://gemini.google.com";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

fn parse_cookies(raw: &str) -> HashMap<String, String> {
    raw.split(';')
        .filter_map(|pair| {
            let mut kv = pair.trim().splitn(2, '=');
            let k = kv.next()?.trim().to_string();
            let v = kv.next()?.trim().to_string();
            Some((k, v))
        })
        .collect()
}

fn build_headers(cookies: &HashMap<String, String>) -> Vec<(String, String)> {
    let cookie_header = cookies
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    vec![
        (
            "Content-Type".into(),
            "application/x-www-form-urlencoded;charset=UTF-8".into(),
        ),
        ("User-Agent".into(), USER_AGENT.into()),
        ("Origin".into(), WEB_BASE_URL.into()),
        ("Referer".into(), format!("{WEB_BASE_URL}/app")),
        ("X-Same-Domain".into(), "1".into()),
        ("Cache-Control".into(), "no-cache".into()),
        ("Pragma".into(), "no-cache".into()),
        ("Cookie".into(), cookie_header),
    ]
}

async fn init_session(client: &Client, cookies: &HashMap<String, String>) -> (Option<String>, Option<String>, Option<String>) {
    let url = format!("{WEB_BASE_URL}/app?hl=en");
    let mut req = client.get(&url).header("User-Agent", USER_AGENT);
    let cookie_header = cookies
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    req = req.header("Cookie", cookie_header);
    for (k, v) in [
        ("Origin", WEB_BASE_URL),
        ("Referer", "https://gemini.google.com/app"),
        ("X-Same-Domain", "1"),
    ] {
        req = req.header(k, v);
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                eprintln!("init_session HTTP {status}");
                return (None, None, None);
            }
            match resp.text().await {
                Ok(body) => {
                    let access_token = extract_snlim0e(&body);
                    let build_label = extract_build_label(&body);
                    let session_id = extract_session_id(&body);
                    (access_token, build_label, session_id)
                }
                Err(e) => {
                    eprintln!("init_session read body error: {e}");
                    (None, None, None)
                }
            }
        }
        Err(e) => {
            eprintln!("init_session request error: {e}");
            (None, None, None)
        }
    }
}

fn extract_snlim0e(body: &str) -> Option<String> {
    if let Some(idx) = body.find("\"SNlM0e\":\"") {
        let start = idx + "\"SNlM0e\":\"".len();
        if let Some(end) = body[start..].find('"') {
            let token = &body[start..start + end];
            if token.len() > 10 {
                return Some(token.to_string());
            }
        }
    }
    None
}

fn extract_build_label(body: &str) -> Option<String> {
    let patterns = [
        "boq_assistant-bard-web-server_",
        "boq_assistant-bard-web-frontend_",
    ];
    for pattern in &patterns {
        if let Some(idx) = body.find(pattern) {
            let search_area = &body[idx..];
            for end_char in ['"', '\\', '\'', '`'] {
                if let Some(end) = search_area.find(end_char) {
                    let label = &search_area[..end];
                    if label.len() > 10 {
                        return Some(label.to_string());
                    }
                }
            }
        }
    }
    None
}

fn extract_session_id(body: &str) -> Option<String> {
    let patterns = ["\"FdrFJe\":\"", "session_id\":\""];
    for pattern in &patterns {
        if let Some(idx) = body.find(pattern) {
            let start = idx + pattern.len();
            if let Some(end) = body[start..].find('"') {
                let sid = &body[start..start + end];
                if !sid.is_empty() {
                    return Some(sid.to_string());
                }
            }
        }
    }
    None
}

fn build_inner_req_list_no_browser(
    prompt: &str,
    category_enum: u64,
    state: Option<&WebConversationState>,
    slot3: Option<Value>,
    slot4: Option<Value>,
    slot5: Option<Value>,
) -> Vec<Value> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut inner = vec![Value::Null; 97];
    inner[0] = json!([prompt, 0, null, null, null, null, 0]);
    inner[1] = json!(["en"]);
    inner[2] = match state {
        Some(s) => json!([
            s.conversation_id,
            s.response_id,
            s.response_part_id,
            null,
            null,
            null,
            null,
            null,
            null,
            s.continuation_token
        ]),
        None => json!(["", "", "", null, null, null, null, null, null, ""]),
    };

    inner[3] = slot3.unwrap_or(Value::Null);
    inner[4] = slot4.unwrap_or(Value::Null);
    inner[5] = slot5.unwrap_or(Value::Null);

    inner[6] = json!([1]);
    inner[7] = json!(1);
    inner[10] = json!(1);
    inner[11] = json!(0);
    inner[17] = if state.is_some() {
        json!([[1]])
    } else {
        json!([[0]])
    };
    inner[18] = json!(0);
    inner[27] = json!(1);
    inner[30] = json!([category_enum]);
    inner[41] = json!([2]);
    inner[53] = json!(0);
    inner[59] = json!(uuid::Uuid::new_v4().to_string().to_uppercase());
    inner[61] = json!([]);
    inner[66] = json!([ts, 0]);
    inner[68] = json!(1);
    inner[79] = json!(6);
    inner[91] = json!(0);
    inner[96] = json!(0);

    inner
}

fn build_stream_generate_body(inner_req_list: &[Value]) -> String {
    let inner_json = serde_json::to_string(inner_req_list).unwrap_or_default();
    let f_req = json!([null, inner_json]);
    let f_req_str = serde_json::to_string(&f_req).unwrap_or_default();
    format!("f.req={}", urlencoding::encode(&f_req_str))
}

async fn send_turn(
    client: &Client,
    headers: &[(String, String)],
    params: &[(&str, &str)],
    inner_req_list: &[Value],
) -> Result<String, String> {
    let body = build_stream_generate_body(inner_req_list);
    let url = format!("{WEB_BASE_URL}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate");

    let mut req = client.post(&url).query(params).body(body);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("read body failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    Ok(text)
}

fn response_text_snippet(body: &str) -> String {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with(")]}'")
            || line.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
        {
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<Value>(line)
            && let Some(text) =
                gemini_proxy::gemini::web_frontend::extract_text_from_parsed_response(&parsed)
        {
            return text.chars().take(200).collect();
        }
    }
    body.chars().take(300).collect()
}

#[tokio::main]
async fn main() {
    let cookies_str = std::env::var("GEMINI_COOKIES").expect("GEMINI_COOKIES env var");
    let cookies = parse_cookies(&cookies_str);
    let chrome_path = std::env::var("CHROME_PATH").unwrap_or_else(|_| "/usr/bin/chromium".into());

    let client = Client::builder()
        .pool_max_idle_per_host(20)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()
        .expect("build client");

    let out_dir = "/tmp/opencode/captures/multi_turn_no_browser";
    std::fs::create_dir_all(out_dir).unwrap();

    let (access_token, build_label, session_id) = init_session(&client, &cookies).await;
    println!("session: at={:?} bl={:?} sid={:?}", access_token.is_some(), build_label, session_id);

    let headers = build_headers(&cookies);
    let category_enum = 4u64;

    // Use the browser to generate a first-turn payload with valid attestation.
    let browser_client = BrowserAttestationClient::new(chrome_path);
    println!("\n=== Capturing first turn via browser ===");
    let payload = match browser_client
        .get_stream_generate_payload(&cookies, "My name is Alice. Remember it.", None)
        .await
    {
        Ok(p) => {
            std::fs::write(
                format!("{out_dir}/browser_turn1_inner_req_list.json"),
                serde_json::to_string_pretty(&p.inner_req_list).unwrap(),
            )
            .unwrap();
            println!("Browser first-turn payload captured");
            p
        }
        Err(e) => {
            println!("Browser capture failed: {e}");
            browser_client.close().await;
            return;
        }
    };

    // Send the browser payload via raw HTTP to verify it works and extract state.
    let _prompt1 = "My name is Alice. Remember it.";
    let reqid1 = {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        ((ts % 900_000) + 100_000).to_string()
    };
    let mut params1 = vec![
        ("hl", "en"),
        ("_reqid", reqid1.as_str()),
        ("rt", "c"),
        ("pageId", "none"),
    ];
    if let Some(ref bl) = build_label {
        params1.push(("bl", bl.as_str()));
    }
    if let Some(ref sid) = session_id {
        params1.push(("f.sid", sid.as_str()));
    }

    let inner1 = payload.inner_req_list.clone();
    let body1 = build_stream_generate_body(&inner1);
    std::fs::write(format!("{out_dir}/browser_turn1_request_body.txt"), &body1).unwrap();

    println!("\n=== Sending browser-generated first turn via HTTP ===");
    let state = match send_turn(&client, &headers, &params1, &inner1).await {
        Ok(body) => {
            std::fs::write(format!("{out_dir}/browser_turn1_response_raw.txt"), &body).unwrap();
            println!("Browser turn 1 HTTP OK; body length {}", body.len());
            println!("Text snippet: {}", response_text_snippet(&body));
            match extract_conversation_state(&body) {
                Ok(s) => {
                    println!("Extracted state: {:?}", s);
                    std::fs::write(
                        format!("{out_dir}/browser_turn1_state.json"),
                        serde_json::to_string_pretty(&json!({
                            "conversation_id": s.conversation_id,
                            "response_id": s.response_id,
                            "response_part_id": s.response_part_id,
                            "continuation_token": s.continuation_token,
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                    Some(s)
                }
                Err(e) => {
                    println!("Failed to extract state: {e}");
                    None
                }
            }
        }
        Err(e) => {
            std::fs::write(format!("{out_dir}/browser_turn1_error.txt"), &e).unwrap();
            println!("Browser turn 1 failed: {e}");
            None
        }
    };

    browser_client.close().await;

    let state = match state {
        Some(s) => s,
        None => {
            println!("No state; cannot continue.");
            return;
        }
    };

    // For the second turn we need fresh attestation tokens from the browser.
    // Launch a new browser client with the conversation_id so it continues the
    // same conversation and captures slots 3/4/5 for turn 2.
    let browser_client2 = BrowserAttestationClient::new(
        std::env::var("CHROME_PATH").unwrap_or_else(|_| "/usr/bin/chromium".into()),
    );
    println!("\n=== Capturing second turn attestation via browser ===");
    let turn2_payload = match browser_client2
        .get_stream_generate_payload(
            &cookies,
            "What is my name?",
            Some(&state.conversation_id),
        )
        .await
    {
        Ok(p) => {
            std::fs::write(
                format!("{out_dir}/browser_turn2_inner_req_list.json"),
                serde_json::to_string_pretty(&p.inner_req_list).unwrap(),
            )
            .unwrap();
            println!("Browser second-turn payload captured");
            p
        }
        Err(e) => {
            println!("Browser second-turn capture failed: {e}");
            browser_client2.close().await;
            return;
        }
    };
    browser_client2.close().await;

    // Extract attestation slots from browser second-turn payload.
    let slot3 = turn2_payload.inner_req_list.get(3).cloned().unwrap_or(Value::Null);
    let slot4 = turn2_payload.inner_req_list.get(4).cloned().unwrap_or(Value::Null);
    let slot5 = turn2_payload.inner_req_list.get(5).cloned().unwrap_or(Value::Null);
    println!("Browser turn2 slot3={slot3:?} slot4={slot4:?} slot5={slot5:?}");

    // Craft second-turn HTTP request with our state but browser attestation.
    let reqid2 = {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        ((ts % 900_000) + 100_000).to_string()
    };
    let mut params2 = vec![
        ("hl", "en"),
        ("_reqid", reqid2.as_str()),
        ("rt", "c"),
        ("pageId", "none"),
    ];
    if let Some(ref bl) = build_label {
        params2.push(("bl", bl.as_str()));
    }
    if let Some(ref sid) = session_id {
        params2.push(("f.sid", sid.as_str()));
    }

    let inner2 = build_inner_req_list_no_browser(
        "What is my name?",
        category_enum,
        Some(&state),
        Some(slot3),
        Some(slot4),
        Some(slot5),
    );
    let body2 = build_stream_generate_body(&inner2);
    std::fs::write(format!("{out_dir}/browser_turn2_request_body.txt"), &body2).unwrap();
    std::fs::write(
        format!("{out_dir}/browser_turn2_mixed_inner_req_list.json"),
        serde_json::to_string_pretty(&inner2).unwrap(),
    )
    .unwrap();

    println!("\n=== Sending second turn with browser attestation ===");
    match send_turn(&client, &headers, &params2, &inner2).await {
        Ok(body) => {
            std::fs::write(format!("{out_dir}/browser_turn2_response_raw.txt"), &body).unwrap();
            println!("Turn 2 HTTP OK; body length {}", body.len());
            println!("Text snippet: {}", response_text_snippet(&body));
        }
        Err(e) => {
            std::fs::write(format!("{out_dir}/browser_turn2_error.txt"), &e).unwrap();
            println!("Turn 2 failed: {e}");
        }
    }
}
