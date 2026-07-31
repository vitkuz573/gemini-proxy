use std::collections::HashMap;
use gemini_proxy::gemini::browser_attestation::BrowserAttestationClient;
use gemini_proxy::gemini::web_frontend::BrowserAttestationPayload;
use serde_json::json;

#[tokio::main]
async fn main() {
    let chrome_path = std::env::var("CHROME_PATH").unwrap_or_else(|_| "/usr/bin/chromium".into());
    let cookies_str = std::env::var("GEMINI_COOKIES").expect("GEMINI_COOKIES env var");
    let cookies: HashMap<String, String> = cookies_str
        .split(';')
        .filter_map(|pair| {
            let mut kv = pair.trim().splitn(2, '=');
            let k = kv.next()?.trim().to_string();
            let v = kv.next()?.trim().to_string();
            Some((k, v))
        })
        .collect();

    let client = BrowserAttestationClient::new(chrome_path);

    let prompts = vec![
        ("weather", "What is the weather in Paris today?"),
        ("maps", "Show me directions from San Francisco to Palo Alto using Google Maps."),
        ("flights", "Find flights from NYC to London next week."),
        ("custom_tool", "Use the get_weather tool to find the weather in Tokyo."),
        ("simple", "What is the capital of France?"),
    ];

    for (name, prompt) in prompts {
        println!("\n=== Capturing prompt: {} ===", name);
        match client.get_stream_generate_payload(&cookies, prompt, None).await {
            Ok(BrowserAttestationPayload { inner_req_list }) => {
                let out_dir = "/tmp/opencode/captures/tools_native";
                std::fs::create_dir_all(out_dir).unwrap();
                let path = format!("{}/{}_capture.json", out_dir, name);
                let mut slots = serde_json::Map::new();
                for (i, v) in inner_req_list.iter().enumerate() {
                    if !v.is_null() && v != &json!([]) && v != &json!("") && v != &json!([0]) {
                        slots.insert(format!("slot_{}", i), v.clone());
                    }
                }
                let out = json!({
                    "prompt": prompt,
                    "inner_req_list": inner_req_list,
                    "non_empty_slots": slots,
                });
                std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
                println!("Wrote {} ({} slots)", path, inner_req_list.len());
                println!("Non-empty slots: {:?}", slots.keys().collect::<Vec<_>>());
            }
            Err(e) => {
                eprintln!("Failed to capture {}: {}", name, e);
            }
        }
    }

    client.close().await;
}
