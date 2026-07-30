# Gemini Web Frontend Protocol Notes

This document describes the undocumented endpoints the proxy uses to talk to
the Gemini web UI when cookie authentication is enabled. It is accurate as of
July 2026 and may need updates if Google changes the frontend internals.

## Authentication

Cookie mode needs the same cookies the browser sends on `https://gemini.google.com`.
At minimum `__Secure-1PSID` is required. The proxy copies these cookies verbatim
into every request to `gemini.google.com`.

## Discovering available models

OpenAI-compatible endpoint: `GET /v1/models`

Real upstream call:

```text
POST https://gemini.google.com/_/BardChatUi/data/batchexecute?rpcids=otAQ7b&source-path=%2Fapp&hl=<lang>
```

### Request

Headers:

```text
Content-Type: application/x-www-form-urlencoded;charset=UTF-8
Cookie: <browser cookies>
```

Body:

```text
f.req=[[["otAQ7b","[]",null,"generic"]]]&at=<SNlM0e>
```

Important:

- `f.req` is a **three-level** WIZ batch array: outer batch → RPC list →
  `[rpc_id, payload_json_string, null, "generic"]`.
- `otAQ7b` is the RPC ID for the `GetUserStatus` call that returns the mode
  picker state.
- `at` is the `SNlM0e` token from the initial `/app` page. In practice the
  request succeeds without it, but it is included when available.

### Response

The response is `text/plain` with WIZ anti-XSSI prefix:

```text
)]}'

[[["wrb.fr","otAQ7b",null,"<json-string>",null,null,null,"generic"]]]
58
[["di",1]]
25
[["e",4,null,null,130]]
```

Take the first line of JSON, strip the leading `)]}'\n\n`, and parse it. The
`otAQ7b` `wrb.fr` entry contains a JSON string at array index `2`. After parsing
that string, the mode list is at inner index `15`.

### Mode list format

Each mode is an array. Fields we care about:

| Index | Meaning |
|-------|---------|
| `0`   | Hex mode ID (e.g. `fbb127bbb056c959`) |
| `1`   | Display title (e.g. `3.6 Flash`) |
| `2`   | Description |
| `11` / `19` | Long versioned name (e.g. `Gemini 3.6 Flash`) |
| `17`  | Category enum: `1` = Auto, `2` = Fast, `4` = Thinking, `5` = Pro, `6` = Flash-Lite |

### Mapping to OpenAI model IDs

The proxy exposes each mode as `models/<hex_mode_id>`, matching the ID format
used by the Google Generative Language API. Examples captured live:

| OpenAI ID (`/v1/models`) | Hex mode ID | Title |
|---------------------------|-------------|-------|
| `models/cf41b0e0dd7d53e5` | `cf41b0e0dd7d53e5` | 3.5 Flash-Lite |
| `models/fbb127bbb056c959` | `fbb127bbb056c959` | 3.6 Flash |
| `models/9d8ca3786ebdfbea` | `9d8ca3786ebdfbea` | 3.1 Pro |

## Chat completions

The chat endpoint uses a different batchexecute RPC, `Fd0Qje`, which accepts the
mode as a hex ID. The proxy translates any `models/<hex>` model value back to
the raw hex ID before building the request body.

Resolution order in `resolve_model_mode`:

1. Strip optional `models/` prefix.
2. If the value is a 16-character hex string, return it unchanged.
3. Otherwise apply keyword heuristics:
   - `lite` → Flash-Lite hex ID
   - `thinking` / `deep` → Thinking hex ID
   - `pro` → Pro hex ID
   - anything else → Fast hex ID

This keeps both new dynamic IDs and legacy aliases (`gemini-2.5-flash`, etc.)
working.

## Implementation pointers

- `src/gemini/web_frontend.rs` — `WebFrontendClient::list_models`,
  `parse_user_status_model_list`, and `resolve_model_mode`.
- `src/gemini/client.rs` — `GeminiClient::list_models_via_web` wires the
  dynamic list into the public API.
- `src/openai/server.rs` — `/v1/models` and `/v1/models/{model}` handlers.
