# GSD Debug Knowledge Base

Resolved debug sessions. Used by `gsd-debugger` to surface known-pattern hypotheses at the start of new investigations.

---

## native-system-prompt-slot — Gemini web frontend has no usable native system-prompt slot for proxy use
- **Date:** 2026-08-01
- **Error patterns:** system instruction, slot 33, HTTP 400, StreamGenerate, inner_req_list
- **Root cause:** The Gemini web frontend StreamGenerate proto has field 34 (slot 33) of type _.AE intended for system instructions, but the live request builder only populates it for internal UI paths (a.Lc.y7.I7 or a.isSystemMessage). In normal user chat the system text is flattened into the user prompt text in field 1 (slot 0). Slot 33 is null in live captures, and setting it from the proxy triggers HTTP 400 because the attestation token (slots 3/4) is signed over the serialized proto.
- **Fix:** No code change; keep XML-in-prompt fallback. Updated doc comment in src/gemini/web_frontend.rs to document slot 33 rejection.
- **Files changed:** src/gemini/web_frontend.rs
---
## native-system-prompt-slot — Gemini web frontend has no usable native system-prompt slot for proxy use
- **Date:** 2026-08-01
- **Error patterns:** system instruction, slot 33, HTTP 400, StreamGenerate, inner_req_list
- **Root cause:** The Gemini web frontend StreamGenerate proto has field 34 (slot 33) of type _.AE intended for system instructions, but the live request builder only populates it for internal UI paths (a.Lc.y7.I7 or a.isSystemMessage). In normal user chat the system text is flattened into the user prompt text in field 1 (slot 0). Slot 33 is null in live captures, and setting it from the proxy triggers HTTP 400 because the attestation token (slots 3/4) is signed over the serialized proto.
- **Fix:** No code change; keep XML-in-prompt fallback. Updated doc comment in src/gemini/web_frontend.rs to document slot 33 rejection.
- **Files changed:** src/gemini/web_frontend.rs
---
