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

    let image_path = std::env::var("CAPTURE_IMAGE_PATH").unwrap_or_else(|_| {
        // Create a tiny 1x1 PNG as a default fallback.
        let fallback = "/tmp/opencode/captures/test_image.png";
        std::fs::create_dir_all("/tmp/opencode/captures").unwrap();
        if !std::path::Path::new(fallback).exists() {
            // Minimal valid 1x1 PNG, base64.
            let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
            use base64::Engine;
            std::fs::write(fallback, base64::engine::general_purpose::STANDARD.decode(png_b64).unwrap()).unwrap();
        }
        fallback.into()
    });

    let prompts = vec![
        ("text_only", "What is the capital of France?", None::<String>),
        ("text_and_image", "Describe this image in one sentence.", Some(image_path.clone())),
        ("image_only", "", Some(image_path.clone())),
    ];

    for (name, prompt, maybe_image) in prompts {
        println!("\n=== Capturing prompt: {} ===", name);
        let result = if let Some(ref image) = maybe_image {
            client.get_stream_generate_payload_with_image(&cookies, prompt, None, Some(image.as_str())).await
        } else {
            client.get_stream_generate_payload(&cookies, prompt, None).await
        };
        match result {
            Ok(BrowserAttestationPayload { inner_req_list }) => {
                let out_dir = "/tmp/opencode/captures/image_upload";
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
