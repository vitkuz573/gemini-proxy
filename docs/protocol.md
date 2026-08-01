# Gemini Web Frontend Protocol Notes

This document describes the undocumented endpoints the proxy uses to talk to
the Gemini web UI when cookie authentication is enabled. It is accurate as of
July 2026 and may need updates if Google changes the frontend internals.

## Authentication

Cookie mode needs the same cookies the browser sends on `https://gemini.google.com`.
At minimum `__Secure-1PSID` is required. The proxy copies these cookies verbatim
into every request to `gemini.google.com`.

Cookies expire and rotate; if `/v1/models` or chat start returning 400s, refresh
`GEMINI_COOKIES` from a live browser session.

## Session bootstrap (`/app`)

Before any batchexecute call the proxy fetches:

```text
GET https://gemini.google.com/app?hl=<lang>
```

with the browser cookies. The response is a heavily inlined HTML page that
contains `window.WIZ_global_data`. We extract three values from it:

| WIZ key | Usage | Example |
|---------|-------|---------|
| `SNlM0e` | `at` parameter for batchexecute | `ADR5za...:1785435735471` |
| `cfb2h`  | `bl` query parameter | `boq_assistant-bard-web-server_20260728.05_p0` |
| `FdrFJe` | `f.sid` query parameter | `-8285181667054680500` |

### SNlM0e / `at` token

The token is stored in WIZ_global_data as:

```json
"SNlM0e":"ADR5zaonohfEqq4JcG31782tYOO_:1785435735471"
```

Important:

- The **entire value including the `:` and 13-digit timestamp suffix** must be
  sent as `at`. Stripping the suffix causes batchexecute to return HTTP 400.
- If `SNlM0e` is missing from the page (rare), batchexecute currently still works
  with an empty `at`, but the proxy logs this as a debug event rather than a
  failure.
- The token changes every page load, so we re-fetch `/app` on session
  initialization.

## Discovering available models

OpenAI-compatible endpoint: `GET /v1/models`

Real upstream call:

```text
POST https://gemini.google.com/_/BardChatUi/data/batchexecute?rpcids=otAQ7b&source-path=%2Fapp&hl=<lang>&_reqid=<reqid>&rt=c&pageId=none&authuser=0&bl=<build_label>&f.sid=<session_id>
```

### Request

Headers:

```text
Content-Type: application/x-www-form-urlencoded;charset=UTF-8
Cookie: <browser cookies>
Origin: https://gemini.google.com
Referer: https://gemini.google.com/app
X-Same-Domain: 1
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
- `at` is the full `SNlM0e` value from `/app` (see Session bootstrap).

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
| `17`  | Category enum: `1` = Fast, `2` = Thinking, `3` = Pro, `4` = Auto, `5` = Fast-Dynamic-Thinking, `6` = Flash-Lite |

### Mapping to OpenAI model IDs

The proxy derives a stable, human-readable OpenAI-style ID from the versioned
name: `3.6 Flash` becomes `gemini-3.6-flash`. The raw hex ID is exposed in the
`root` field so clients can use either form.

Example response:

```json
{
  "id": "gemini-3.6-flash",
  "object": "model",
  "owned_by": "google",
  "root": "models/fbb127bbb056c959"
}
```

Examples captured live:

| OpenAI ID (`/v1/models`) | Hex mode ID (`root`) | Title |
|---------------------------|----------------------|-------|
| `gemini-3.5-flash-lite` | `models/cf41b0e0dd7d53e5` | 3.5 Flash-Lite |
| `gemini-3.6-flash` | `models/fbb127bbb056c959` | 3.6 Flash |
| `gemini-3.1-pro` | `models/9d8ca3786ebdfbea` | 3.1 Pro |

The hex IDs are **not stable**. Google rotates them between sessions, so
clients should either use the human-readable ID or call `GET /v1/models` first
and use the returned `root` for subsequent chat requests.

## Chat completions

The chat endpoint uses `StreamGenerate` (`assistant.lamda.BardFrontendService/StreamGenerate`)
with the same query parameters and `at` handling as model discovery. The proxy
resolves the requested model to the current hex mode ID and its category:

1. `models/<hex>` — used unchanged; category is inferred from the cached model
   list or from the hex ID/name.
2. `gemini-<version>-<category>` — looked up in the cached `/v1/models` list.
   If the cache is empty it is populated first.
3. Anything else returns a 400 with instructions to call `/v1/models`.

No legacy aliases are supported; the only stable identifiers are the human
readable IDs returned by `/v1/models`.

## Tools / function calling

The proxy supports OpenAI-compatible `tools` / `function_declarations` in two
ways depending on the authentication mode.

### API-key mode

When a Gemini API key is available the proxy forwards the native
`tools`/`tool_config` fields of the
[Generative Language API](https://ai.google.dev/api/rest/v1beta/Tools)
directly.  This path provides true native function calling and should be used
whenever possible.

### Cookie-auth / web-frontend mode

The Gemini web frontend's `StreamGenerate` endpoint does not expose an obvious
field for arbitrary function declarations in its public HTTP surface.  Reverse
engineering of the live frontend (`BardChatUi_modules.js`) shows one related
protobuf field, `toolMentions` (proto field 10 on `_.ON`), but it is used only
for built-in Google extensions triggered by `@` mentions in the UI
(e.g. `@Gmail`, `@YouTube`) or by explicit internal callers such as
`audio_gen_tool`.  No documented slot in the 97-slot `inner_req_list`, and no
side-channel header, carries custom function schemas.

Because of this, the proxy falls back to **serializing tool declarations into
the prompt text**.  The format used is XML-style markers produced by
`serialize_request_to_prompt` in `src/gemini/web_frontend.rs`:

```xml
<system>
...system instructions...
</system>

<tools>
  <tool name="get_weather" description="...">
    {"type":"object","properties":{"location":{"type":"string"}},"required":["location"]}
  </tool>
</tools>

<user>
What is the weather in Paris?
</user>
```

This fallback works when the model respects the XML markers and emits
`<function_call>` / `<function_response>` blocks, which the proxy parses back
into OpenAI-style `tool_calls`.  It is less reliable than the native API-key
path because it relies on prompt-level instruction following.

For best results, send a `name` field on `tool` messages so the proxy can label
the `<function_response>` with the correct function name instead of the
`tool_call_id`.

Native custom tool declarations for cookie-authenticated requests are still
being investigated.  If Google exposes such a field it will likely appear in the
`_.ON` protobuf (field 10 for tool metadata) or as a new side-channel header;
until then, the XML fallback is the supported option.

## Reasoning / thinking passthrough

`reasoning_effort` (OpenAI) and `thinking` (Anthropic) are converted to Gemini's
`thinking_config` in `GenerationConfig`. When an API key is configured, this
config is forwarded to the official Generative Language API and the model will
return explicit reasoning/thoughts when supported.

In cookie-auth mode the proxy talks to the Gemini web frontend, which does **not**
expose a dedicated thinking mode for all accounts. The request body sent to the
frontend contains a mode category (`inner_req_list[30]`) that is set from the
model picker enum:

| Enum | Category |
|------|----------|
| `1`  | Fast |
| `2`  | Thinking |
| `3`  | Pro |
| `4`  | Auto |
| `5`  | Fast-Dynamic-Thinking |
| `6`  | Flash-Lite |

If the account only lists Fast/Pro/Flash-Lite models (no enum `2` mode),
thinking requests are still served by the selected model, but the response will
contain inline reasoning rather than separate thinking blocks. There is no known
frontend field to force a thinking-style response beyond selecting the Pro/Auto
category.

## Stateful multi-turn

By default every request to `/v1/chat/completions` or `/v1/messages` in
cookie-auth mode is treated as a single-turn request: the proxy flattens the
full message history into the prompt text and sends `inner_req_list[2]` empty
with slot 17 set to `[[0]]`.

True stateful continuation is supported when the `browser-attestation` feature
is enabled and a Chrome/Chromium executable is configured via `GEMINI_HEADLESS_BROWSER`
or `CHROME_PATH`.  In that mode the proxy:

1. Extracts `conversation_id`, `response_id`, `response_part_id`, and the
   continuation token from each StreamGenerate response.
2. Replays those values into slot 2 of the next request and sets slot 17 to
   `[[1]]`.
3. Uses a headless Chromium instance to produce a legitimate StreamGenerate
   payload for the current turn, then replays it to Gemini.

### Slot usage

| Slot | Field | Meaning |
|------|-------|---------|
| 0  | 1  | Current user text (flattened prompt). |
| 1  | 2  | Locale, e.g. `["en"]`. |
| 2  | 3  | Conversation state: `[conversation_id, response_id, response_part_id, null, ..., continuation_token]`. |
| 3  | 4  | Browser-attestation token (`Ijb`). Empty when browser path is disabled. |
| 4  | 5  | Browser-attestation UUID (`Jjb`). Empty when browser path is disabled. |
| 17 | 18 | Turn counter: `[[0]]` for first turn, `[[1]]` for follow-ups. |
| 30 | 31 | Mode category enum from the model picker. |
| 59 | 60 | Client request UUID. |

### Continuation token

After a successful StreamGenerate turn the response contains a small meta entry:

```json
["wrb.fr", null, "[null,[null,\"r_...\"],{\"26\":\"AwAAAA...\"}]"]
```

The value of object key `"26"` is the continuation token.  The proxy stores it
in `WebConversationState::continuation_token` and places it at index `9` of the
10-element slot 2 array for the next turn.

### Headless-browser attestation

The browser integration lives in `src/gemini/browser_attestation.rs` and is
gated by the Cargo feature `browser-attestation`.  It uses raw Chrome DevTools
Protocol (CDP) over a local WebSocket, so the only extra dependency is
`tokio-tungstenite`.

At runtime the browser is enabled only when one of these environment variables
is set:

- `GEMINI_HEADLESS_BROWSER=/usr/bin/chromium`
- `CHROME_PATH=/usr/bin/google-chrome-stable`

If neither is set the proxy falls back to the flattened-prompt path even when
the feature is compiled in.

`BrowserAttestationClient` performs the following steps for each turn:

1. Launch Chrome with `--remote-debugging-port=0` and read the DevTools
   WebSocket URL from stderr.
2. Connect via CDP, enable `Runtime`, `Network`, and `Page` domains.
3. Navigate to `https://gemini.google.com/app?hl=en`.
4. Inject the cookies from `GEMINI_COOKIES` using `Network.setCookie`.
5. Simulate the user typing the current prompt and pressing Enter by executing
   the JS snippet in `src/gemini/browser_attestation_simulate.js`.
6. Wait for a `Network.requestWillBeSent` event whose URL contains
   `StreamGenerate`.
7. Call `Network.getRequestPostData` to read the form body, parse `f.req`, and
   return the 97-slot `inner_req_list`.

The proxy then uses that array as the base for its own request, overriding only
slot 0 (prompt), slot 30 (category enum), and slot 59 (fresh UUID).  Because
the payload came from a real browser, slots 2/3/4/17 already contain valid
state and attestation tokens.

### Caching and invalidation

The browser client keeps a single Chrome process and page alive for the
lifetime of the `WebFrontendClient`.  If a request returns HTTP 400 or an error
containing code `1096` (attestation/state invalid), the proxy clears the cached
`WebConversationState`, which forces the next turn to start a fresh
conversation.

## Implementation pointers

- `src/gemini/web_frontend.rs` — `WebFrontendClient::list_models`,
  `parse_user_status_model_list`, `build_inner_req_list`,
  `extract_conversation_state`, and `WebConversationState`.
- `src/gemini/browser_attestation.rs` — `BrowserAttestationClient` and the CDP
  driver used to capture real StreamGenerate payloads.
- `src/gemini/browser_attestation_simulate.js` — JS injected into the headless
  page to trigger a StreamGenerate request.
- `src/gemini/client.rs` — `GeminiClient::list_models_via_web`,
  `resolve_web_model`, and `update_conversation_state_from_body`.
- `src/openai/server.rs` — `/v1/models`, `/v1/chat/completions`, and streaming
  response handling.
- `src/config.rs` — reads `GEMINI_HEADLESS_BROWSER` / `CHROME_PATH`.
