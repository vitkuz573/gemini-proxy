---
status: investigating
trigger: "Continue reverse-engineering task: response parser returns 502 for live Gemini StreamGenerate responses despite valid raw body. Fix parse_response_parts and integrate with generate_content_via_web/streaming servers."
created: "2026-07-31T15:55:00Z"
updated: "2026-07-31T15:55:00Z"
---

## Current Focus

hypothesis: "parse_response_parts incorrectly unwraps the inner payload as inner_parsed.as_array().first().as_array(), getting inner[0] (null) instead of inner[4] (the parts array). This causes it to return an empty Vec<ResponsePart>, which generate_content_via_web converts into a 502."
test: "Fix the parser to read inner[4] directly, deduplicate/merge streaming chunks, and verify against live captured bodies and proxy curls."
expecting: "After the fix, WebFrontendClient::generate_content returns the raw response body, generate_content_via_web passes that body to parse_response_parts, and the proxy returns 200 with valid candidates."
next_action: "Change WebFrontendClient::generate_content to return String (raw body) instead of calling parse_stream_response. Update any callers if necessary. Then verify live proxy."

## Symptoms

expected: |
  Proxy returns valid chat completions for simple chat, system prompt, tool call, and thinking queries.
actual: |
  Live proxy reaches Gemini and receives valid raw response bodies, but final result is HTTP 502.
  Existing parse_response_parts and parse_stream_response miss text in some response shapes.
errors: []
reproduction: |
  Start gemini-proxy on 127.0.0.1:3001 with cookies in .env.
  POST /v1/chat/completions with various prompts.
  Observe 502.
started: "2026-07-31T15:55:00Z"

## Eliminated

## Evidence

- timestamp: "2026-07-31T15:58:00Z"
  checked: "Live StreamGenerate response shapes for simple/tool/thinking"
  found: "All successful responses use a 48-element inner array: inner[0]=null, inner[1]=[c_id,r_id], inner[4]=[[part_id, [text_strings], ...]]. Text is always at part[1] (a list of strings). No functionCall objects observed in tool prompt (model returned inline text)."
  implication: "Current parse_response_parts uses inner_parsed.as_array().first().as_array() which gets inner[0]=null; it should access inner[4] directly. That's why it returns empty and proxy emits 502."

- timestamp: "2026-07-31T16:00:00Z"
  checked: "Existing parse_response_parts implementation"
  found: "It tries inner_arr.get(4) but inner_arr is computed as inner_parsed.as_array().and_then(|a| a.first()). The first element of the inner payload is null."
  implication: "Fix: remove the .first() indirection and use inner_parsed[4] as the parts container."

- timestamp: "2026-07-31T16:05:00Z"
  checked: "Proxy debug logs and generate_content flow"
  found: "WebFrontendClient::generate_content calls parse_stream_response and returns a plain String. generate_content_via_web then calls parse_response_parts on that extracted text, which has no JSON to parse and fails."
  implication: "generate_content must return the raw response body so parse_response_parts can do its job. The legacy parse_stream_response path is no longer used for the proxy conversion; remove it or move it to a helper not called by generate_content."

## Resolution

root_cause: |
  Two issues caused the 502 in cookie-auth mode:
  1. `parse_response_parts` navigated the StreamGenerate payload incorrectly: it used `inner_parsed.as_array().first().as_array()` (expecting the inner array wrapped in a one-element list), but live Gemini returns the 48-element inner array directly with parts at `inner[4]`. This produced an empty `Vec<ResponsePart>`, which `generate_content_via_web` turned into a 502.
  2. `WebFrontendClient::generate_content` returned the *extracted plain text* from `parse_stream_response`, not the raw response body. `generate_content_via_web` then passed that short text string into `parse_response_parts`, which had no JSON structure to parse and also returned empty.
  3. Separate system-instruction slot (slot 33) in the 97-slot payload is rejected by live Gemini with HTTP 400.
fix: |
  - `src/gemini/web_frontend.rs`:
    - `generate_content` now returns the raw StreamGenerate response body.
    - `stream_generate` now accepts the full `GenerateContentRequest` and merges system instruction into the prompt text before building `inner_req_list`.
    - `parse_response_parts` now reads parts from `inner[4]` directly, with a fallback to the wrapped shape used in tests.
    - Added `merge_system_into_prompt` helper to avoid the rejected slot 33.
  - `src/gemini/client.rs`:
    - `generate_content_via_web` calls `parse_response_parts` on the raw body and constructs a proper `GenerateContentResponse` with candidates.
    - `stream_content_via_web` passes the full request object.
    - Removed the local `extract_prompt_text` helper (now in web_frontend).
verification: |
  - `cargo test`: 46 unit tests + 10 integration tests pass.
  - Live curls against http://127.0.0.1:3001 return HTTP 200 for simple chat, system message, thinking/reasoning (gemini-3.1-pro with reasoning_effort=high), and tool-declared chat.
files_changed:
  - src/gemini/web_frontend.rs
  - src/gemini/client.rs
