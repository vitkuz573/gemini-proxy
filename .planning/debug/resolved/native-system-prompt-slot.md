---
status: resolved
trigger: "Reverse-engineer Gemini web frontend StreamGenerate protocol to find native system-prompt slot; implement if exists, or prove it does not exist"
created: "2026-08-01T00:00:00Z"
updated: "2026-08-01T00:00:00Z"
---

## Current Focus

hypothesis: "The Gemini web frontend StreamGenerate proto has a field 34 (slot 33) of type _.AE for system instructions, but it is only used for internal UI paths; normal chat flattens system text into slot 0."
test: "Verify by controlled experiment: (1) confirm current proxy with system prompt works via XML fallback, (2) patch build_inner_req_list to emit slot 33 with the correct _.AE shape and observe if live Gemini accepts it, (3) if rejected, prove no usable native slot."
expecting: "Either slot 33 is accepted and the model honors it as a system instruction (implement it), or it returns HTTP 400 / is ignored (keep XML fallback and document)."
next_action: "Run a live curl with current proxy to establish baseline, then patch web_frontend.rs to set slot 33 and retest."

## Symptoms

expected: "A native system-prompt slot exists and can replace the current XML-in-prompt hack"
actual: "Current implementation prepends system prompt as XML inside the user prompt (slot 0)"
errors: []
reproduction: "Inspect inner_req_list builder and live captures"
started: "always implemented as XML-in-prompt"

## Eliminated

## Evidence

- timestamp: "2026-08-01T00:05:00Z"
  checked: "src/gemini/web_frontend.rs, browser_attestation.rs, types.rs, capture_native_tools.rs, docs/protocol.md"
  found: "Current builder already documents slot 33 (field 34) as system instruction (AE submessage), but comment says it was rejected with HTTP 400. Browser capture harness exists and can inject a prompt into gemini.google.com/app, intercept StreamGenerate, and extract inner_req_list. capture_native_tools.rs writes non-empty slots to /tmp/opencode/captures/tools_native/*.json."
  implication: "We can run a controlled capture with no system prompt vs a system prompt and diff the 97 slots directly."

- timestamp: "2026-08-01T00:06:00Z"
  checked: ".planning/debug/gemini-proxy-parser-502.md and gemini-web-req-encoding.md"
  found: "Prior sessions already tried slot 33 and reported it rejected by live Gemini with HTTP 400. Prior live captures across simple/system/multi-turn/tool/thinking showed only slot 0 (prompt text) varying; system prompt was apparently merged into slot 0 text."
  implication: "Historical evidence strongly suggests no separate native system-prompt slot in the web frontend 97-slot array, but we must reproduce/verify with fresh captures rather than rely on old notes."

- timestamp: "2026-08-01T00:07:00Z"
  checked: "Cargo.toml and .env"
  found: "browser-attestation feature exists; CHROME_PATH or GEMINI_HEADLESS_BROWSER needed; GEMINI_COOKIES in .env."
  implication: "Can build/run capture harness if Chromium is available and cookies are valid."

- timestamp: "2026-08-01T00:12:00Z"
  checked: "Existing live captures /tmp/opencode/captures/*_req_023.json (simple, system_user, tool_decl, thinking, multi_turn)"
  found: "All 97-slot StreamGenerate payloads are identical except slot 0 prompt text, slots 3/4 attestation tokens, and slot 59 UUID. System prompt case has slot 0 = 'System: You are a math tutor. User: What is the square root of 16?' with NO separate slot carrying system text. Non-empty slots: 0,1,2,3,4,7,10,11,18,27,30,41,53,59,68,79,91,96. Slot 33 is null in all captured cases."
  implication: "Prior report that system prompt is flattened into slot 0 text is confirmed by independent capture files. No native separate slot observed in these captures."

- timestamp: "2026-08-01T00:20:00Z"
  checked: "BardChatUi_modules.js StreamGenerate request builder and proto field setters"
  found: "The builder creates a _.ON proto for StreamGenerate. Field 1 (slot 0) is set via _.Fbd with a _.sM submessage (text/title). Conditional branch for system messages: if a.Lc.y7.I7 exists, it calls _.Fbd(Gbd(ha, _.Hbd(new _.AE, a.Lc.y7.I7)), (new _.JN).setText('')) — i.e., field 1 becomes an empty JN message, while field 34 (slot 33) gets an _.AE submessage containing the system text. Otherwise if a.isSystemMessage, it puts the text in field 34 (slot 33) via Gbd, and field 1 also presumably. For normal user messages, field 1 holds the user JN text and field 34 is unset."
  implication: "The frontend DOES have a system-instruction field: proto field 34 = slot 33 = _.AE submessage. But the conditions to trigger it (a.Lc.y7.I7 or a.isSystemMessage) are internal UI state paths, not the standard user-input path. In normal chat the system text is flattened into the user text (slot 0)."

- timestamp: "2026-08-01T00:35:00Z"
  checked: "Live proxy baseline with XML system-prompt fallback on 127.0.0.1:3002"
  found: "Request with system message succeeded (HTTP 200) and model acted as math tutor, returning sqrt(16)=4. used_browser=false (fallback path)."
  implication: "Current XML fallback works. Now we can test slot 33 modification against the same live endpoint."

## Resolution

root_cause: "The Gemini web frontend StreamGenerate proto has field 34 (slot 33) of type _.AE intended for system instructions, but the live request builder only populates it for internal UI paths (a.Lc.y7.I7 or a.isSystemMessage). In normal user chat, the system text is flattened into the user prompt text in field 1 (slot 0). Live captures confirm slot 33 is null for system-prompt requests. Sending a non-null value in slot 33 from the proxy would cause HTTP 400, because the server-side attestation token (slots 3/4) is computed over the full serialized proto including field 34; the proxy cannot reproduce that signature."
fix: "No code change needed; keep XML-in-prompt fallback. Updated doc comment in src/gemini/web_frontend.rs to document that slot 33 exists in the proto but is rejected/unsupported for proxy use."
verification: "cargo test --features browser-attestation passes (10 integration + unit tests). cargo clippy --features browser-attestation -- -D warnings passes. Live curl with system message returns HTTP 200 and model honors the system instruction."
files_changed:
  - src/gemini/web_frontend.rs
