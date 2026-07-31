---
status: awaiting_human_verify
trigger: "The user wants full native tool support in the Gemini web frontend cookie-auth path. No more XML-in-prompt hacks. Reverse engineer how the browser actually sends function declarations to Gemini and implement it."
created: 2026-07-31T21:30:00Z
updated: 2026-07-31T22:05:00Z
symptoms_prefilled: true
goal: find_and_fix
---

## Current Focus

hypothesis: The Gemini web frontend does not expose a native field for custom tool declarations in StreamGenerate; built-in extensions are triggered by plain text or @-mentions. The proxy must keep the XML-in-prompt fallback and should preserve browser-captured slot values that differ from the current hardcoded defaults (slots 6 and 66).
test: Compare tool vs non-tool live browser captures, inspect BardChatUi_modules.js for toolMentions usage, verify Rust build_inner_req_list behavior against captures, run cargo test/clippy.
expecting: No new slot/header appears for tools; tests and clippy pass; slot 6 stays [0] and slot 66 stays null when replaying a browser payload.
next_action: Await human verification of the fix and documentation.

## Symptoms

expected: "Cookie-auth mode supports native tool declarations in Gemini web frontend StreamGenerate; model emits proper FunctionCall parts and handles FunctionResponse follow-ups."
actual: "Tools are currently serialized into the prompt text as XML-style tags; model sometimes denies having tools or returns bad formats."
errors: []
reproduction: "Send /v1/chat/completions with tools through gemini-proxy; observe inline JSON or tool denials instead of structured tool_calls."
started: "Current commit 9068452"

## Eliminated

- hypothesis: "Gemini web frontend sends tool declarations in a dedicated inner_req_list slot."
  evidence: "Tool-triggering and non-tool browser captures have identical non-empty slot sets; no slot carries function schemas."
  timestamp: 2026-07-31T21:55:00Z
- hypothesis: "Tool declarations are encoded in side-channel headers like x-goog-ext-525001261-jspb, x-goog-ext-73010989-jspb, or x-goog-ext-73010990-jspb."
  evidence: "Headers are identical in structure across tool and non-tool captures; only request UUIDs/tokens differ."
  timestamp: 2026-07-31T21:55:00Z
- hypothesis: "The toolMentions field in _.ON can be reused for custom function declarations."
  evidence: "toolMentions is populated from @-mention UI entities (likenessUuid) and internal tool names, not user-provided schemas; no code path feeds arbitrary function declarations into it."
  timestamp: 2026-07-31T21:56:00Z

## Evidence

- timestamp: 2026-07-31T21:42:00Z
  checked: "Live browser captures via Playwright for simple, weather, youtube prompts"
  found: "All captures have non-empty slots 0,1,2,3,4,59 only; slot structures identical aside from prompt text and attestation/uuid values."
  implication: "No new slot appears when Gemini selects built-in tools; tool selection is driven by prompt text or backend-side inference."
- timestamp: 2026-07-31T21:55:00Z
  checked: "BardChatUi_modules.js reverse engineering around _.Ncd, _.ON, _.$G, toolMentions"
  found: "_.Ncd builds _.ON proto with toolMentions mapped to field 10 of _.$G; toolMentions come from _.Bi(B,_.TZb,6,_.ni()) (likeness mentions) or explicit internal calls like audio_gen_tool."
  implication: "The only native tool field is for built-in extension mentions, not OpenAI-style function declarations."
- timestamp: 2026-07-31T21:57:00Z
  checked: "Rust build_inner_req_list hardcoded slot overrides vs browser captures"
  found: "Browser sets slot 6 to [0] and leaves slot 66 null; Rust overrides slot 6 to [1] and slot 66 to [ts,0] even when replaying a browser payload."
  implication: "Current hardcoded overrides degrade attestation fidelity for browser-payload mode."

## Resolution

root_cause: "The Gemini web frontend StreamGenerate protocol has no native field for custom function declarations. The model relies on inline prompt text (and internal @-mention extension triggers). The proxy's existing XML-in-prompt fallback is the only viable mechanism, but it corrupts browser-captured payloads by overriding slots 6 and 66 with values that do not match a real browser."
fix: "(1) Document in docs/protocol.md that native tool declarations are unavailable and XML-in-prompt is the fallback. (2) In build_inner_req_list, when replaying a browser payload, preserve the browser's slot 6 ([0]) and slot 66 (null) instead of overwriting them."
verification: "cargo test --features browser-attestation passes (46 unit + 10 integration). cargo clippy --features browser-attestation passes with no warnings."
files_changed:
  - src/gemini/web_frontend.rs
  - docs/protocol.md
