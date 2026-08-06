<!-- generated-by: gsd-doc-writer -->

# Gemini Web Frontend Protocol Notes

This document describes the undocumented endpoints the proxy uses to talk to
the Gemini web UI when cookie authentication is enabled. It is accurate as of
August 2026 and may need updates if Google changes the frontend internals.

## Authentication

Cookie mode needs the same cookies the browser sends on `https://gemini.google.com`.
The minimum signed-in cookies are `__Secure-1PSID` and `__Secure-1PSIDCC`.
The proxy copies these cookies verbatim into every request to
`gemini.google.com`.

Cookies expire and rotate; if `/v1/models` or chat start returning 400s, refresh
`GEMINI_COOKIES` from a live browser session.

## Session bootstrap (`/app`)

Before any batchexecute call the proxy fetches:

```text
GET https://gemini.google.com/app?hl=<lang>
```

with the browser cookies. The response is a heavily inlined HTML page that
contains `window.WIZ_global_data` and a `<script id="bard-initial-data">`
payload. We extract session values from `WIZ_global_data`.

### `WIZ_global_data` keys

The live `/app` HTML exposes `window.WIZ_global_data` with more than 100 keys.
The curated table below lists the values the proxy currently depends on or is
likely to need.

| WIZ key | Type | Usage | Example / value |
|---------|------|-------|-----------------|
| `cfb2h` | str | `bl` query parameter | `boq_assistant-bard-web-server_20260804.05_p0` |
| `FdrFJe` | str | **`f.sid` query parameter** | `4202905934864668489` |
| `qKIAYe` | str | Primary `push-id` header for uploads | `feeds/mcudyrk2a4khkz` |
| `KnDnFf` | str | Secondary / alternate upload feed ID | `feeds/nrij2vo2gajxiu` |
| `h1eoVe` | str | Upload endpoint base URL | `https://push.clients6.google.com/upload/` |
| `Ylro7b` | str | `X-Client-Pctx` header value | `CgcSBWjK7pYx` |
| `PI9WOb` | str | `X-Server-Token` for PA backend | long base64 token |
| `thykhd` | str | Image/upload signing secret | base64 token |
| `GK6dn` | str | PA backend base URL | `https://geminiweb-pa.clients6.google.com` |
| `HUGLxb` | str | PA backend base URL (duplicate) | `https://geminiweb-pa.clients6.google.com` |
| `Im6cmf` | str | Base path for batchexecute/StreamGenerate | `/_/BardChatUi` |
| `MUE6Ne` | str | Application ID | `assistant-bard-web-server` |
| `qwAQke` | str | Module name | `BardChatUi` |
| `VVlN6d` | str | Browser API key | `AIzaSyD6n9asBjvx1yBHfhFhfw_kpS9Faq0BZHM` |
| `TuX5cc` | str | UI language | `en` |
| `rtQCxc` | int | Timezone offset in minutes | `-120` |
| `ZT1yof` | str | PA backend base URL | `https://geminiweb-pa.clients6.google.com` |
| `p9hQne` | str | Static asset root | `https://gemini.gstatic.com/_/boq-bard-web/_/r/` |
| `eptZe` | str | Base path prefix | `/_/BardChatUi/` |

**Notes:**

- `SNlM0e` is currently **absent** from `/app` for the tested session. The JS
  still references it at startup, but the live value is missing and `S06Grb` is
  empty. The proxy should omit the `at` parameter entirely when `SNlM0e` is not
  present; sending an empty `at` still works in current captures.
- `FdrFJe` is the canonical source of `f.sid`. The JS interceptor reads
  `_.If(_.Ad("FdrFJe"))` and attaches it to requests as `f.sid`.
- `qKIAYe` is the preferred `push-id`; `KnDnFf` is a secondary feed ID that
  returns references under a different environment prefix.

### `bard-initial-data` payload keys

The `<script id="bard-initial-data">` payload is plain JSON in the live HTML.
Relevant keys:

| Key | Meaning |
|-----|---------|
| `ZXlM5e` | **Consent interstitial required.** `true` when the latest consent banner has not been accepted. |
| `KEsM4` | Side-nav open on init. Unrelated to consent/auth. |
| `qw1mtf` | Pre-built "Reject all / non-essential" consent save URL (`set_eom=false&set_aps=true&set_sc=true`). |
| `acNycb` | Pre-built "Accept / essential + non-essential" consent save URL (`set_eom=true`). |
| `AFI83b` | Consent dialog page URL (`consent.google.com/d?...`). |
| `N1U0` | Base64 protobuf containing the build label, language, and a token. |
| `Mr43xd` | Alternate base64 protobuf with the same build label + language + token. |
| `Qd7BXc` | Forced availability bypass flag. When `true`, the router treats the user as `AVAILABLE` without waiting for `GetUserStatus`. Currently `false`. |

The consent keys (`qw1mtf`, `acNycb`, `AFI83b`, `N1U0`, `Mr43xd`) are only used
by the browser consent interstitial flow and do not affect batchexecute or
`StreamGenerate` directly.

## Consent flow and `SOCS`

When `/app` is fetched without a valid `SOCS` cookie, the returned HTML sets
`ZXlM5e: true` and exposes the consent decision URLs.

### Obtaining `SOCS`

1. Fetch `GET https://gemini.google.com/app?hl=en` without `SOCS`.
2. Read `bard-initial-data.ZXlM5e`. If `false`, the session is already consented.
3. Fetch the dialog URL `AFI83b` (`https://consent.google.com/d?...`).
4. Extract the hidden `<form action="https://consent.google.com/save">` fields,
   especially the one-time `escs` token.
5. POST to `https://consent.google.com/save` with:
   - `Content-Type: application/x-www-form-urlencoded`
   - `Referer: https://consent.google.com/`
   - `Origin: https://consent.google.com`
   - Form body including `escs`, `bl`, `hl`, `gl`, `m`, `pc`, `src`, `uxe`,
     `cm`, `x`, `continue`, and either:
     - `set_eom=false&set_sc=true&set_aps=true` (reject non-essential), or
     - `set_eom=true` (accept all).
6. The response sets `SOCS` and `__Secure-ENID` and deletes `NID`.

The `SOCS` cookie is returned with `Domain=.google.com`, `Path=/`, `Secure`, and
`SameSite=lax`. It is valid for roughly 13 months in captures.

### Required cookies

For the consent flow itself only `NID` is strictly required. For signed-in
access to `/app` and the backend APIs the minimum cookies are:

- `__Secure-1PSID`
- `__Secure-1PSIDCC`
- `SOCS` (to remove the consent overlay)

`__Secure-1PSIDTS`, `__Secure-1PAPISID`, `APISID`, `SAPISID`, `HSID`, `SSID`,
`SID`, `AEC`, and `COMPASS` were all non-essential for signed-in status in
cookie-drop tests, although the full browser cookie string is usually supplied.

### Auto-consent recommendation

The proxy **should auto-obtain `SOCS`** when `ZXlM5e: true` is detected. `SOCS`
is a consent-state cookie, not a long-lived account credential, and Google sets
it freely via `consent.google.com/save`. Requiring users to manually extract and
refresh `SOCS` is fragile, especially because the consent flow deletes `NID` and
replaces it with `__Secure-ENID`.

Auto-consent should only run when valid signed-in cookies (`__Secure-1PSID` and
`__Secure-1PSIDCC`) are present. It should not be attempted in API-key mode or
when no valid Google session cookies are supplied.

## Discovering available models

OpenAI-compatible endpoint: `GET /v1/models`

Real upstream call:

```text
POST https://gemini.google.com/_/BardChatUi/data/batchexecute?rpcids=Fd0Qje&source-path=%2Fapp&hl=<lang>&_reqid=<reqid>&rt=c&pageId=none&authuser=0&bl=<build_label>&f.sid=<session_id>
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
f.req=[[["otAQ7b","[]",null,"generic"]]]
```

Important:

- `f.req` is a **three-level** WIZ batch array: outer batch → RPC list →
  `[rpc_id, payload_json_string, null, "generic"]`.
- The URL query parameter is `rpcids=Fd0Qje`.
- The body RPC ID is `otAQ7b` (`GetUserStatus`).
- The `at` parameter should be omitted entirely when `SNlM0e` is absent.
- `f.sid` is sourced from `WIZ_global_data.FdrFJe`.

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

### Model category routing

Each mode entry in `GetUserStatus` carries a category enum at index `17`. The
proxy copies this value into `inner_req_list[30]` on every `StreamGenerate`
call, which tells the Gemini frontend which model family to use. The known
category values are:

| Enum | Category                | Typical model name(s)        |
|------|-------------------------|------------------------------|
| `1`  | Fast                    | `gemini-*-flash`             |
| `2`  | Thinking                | `gemini-*-thinking`          |
| `3`  | Pro                     | `gemini-*-pro`               |
| `4`  | Auto                    | fallback / unknown models    |
| `5`  | Fast-Dynamic-Thinking   | experimental dynamic thinking|
| `6`  | Flash-Lite              | `gemini-*-flash-lite`        |

Model resolution works like this:

1. `models/<hex>` — the proxy looks up the hex ID in the cached `/v1/models`
   list and uses its reported category enum. If the ID is not cached, it falls
   back to deriving the category from the ID string and logs a warning.
2. `gemini-<version>-<category>` — the proxy looks up the human-readable ID in
   the cached `/v1/models` list and uses the category enum returned by the
   frontend. If the ID is not in the cache, the cache is refreshed once.
3. Anything else that still does not resolve is treated as `Auto (4)` and a
   warning is logged, so the request can continue instead of failing outright.

The `inner_req_list[30]` slot is therefore the single control point for model
category routing. Previously the proxy always sent `[4]` (Auto); it now sends
the category that matches the requested model ID.

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

## StreamGenerate slot layout

`StreamGenerate` sends `f.req=<url-encoded 97-slot array>`. The slot index is
the protobuf field number minus one. Slots not listed below were `null` in
captures; their purpose is unknown.

| Slot | Field | Role | Text-only fallback | Browser-captured value |
|------|-------|------|--------------------|------------------------|
| 0 | 1 | Current user turn (proto `QN`) | `["<prompt>", 0, null, null, null, null, 0]` | same, or with attachment array for image turns |
| 1 | 2 | Locale (proto `NJ`) | `["en"]` | `["en"]` |
| 2 | 3 | Conversation state (proto `FM`) | `["", "", "", null, null, null, null, null, null, ""]` | `["c_...", "r_...", "rc_...", null, null, null, null, null, null, "<continuation>"]` |
| 3 | 4 | Web-attestation token (proto `Ijb` / `Ckb`) | `""` | `!0…` signed token (1500–2600 chars) |
| 4 | 5 | Attestation hash (proto `Jjb` / `Dkb`) | `""` | 32-char hex string |
| 5 | 6 | Unknown; always `null` in captures | `null` | `null` |
| 6 | 7 | Locale-wrapper submessage (proto `rcd`) | `[1]` | `[1]` |
| 7 | 8 | Always-`true` flag (`kcd`) | `1` | `1` |
| 10 | 11 | Platform enum (`mcd` / `_.MD`) | `1` | `1` |
| 11 | 12 | Boolean flag (`icd` / `Jnb`) | `0` | `0` |
| 17 | 18 | Turn counter (`Ycd`) | `[[0]]` | `[[0]]` / `[[1]]` |
| 18 | 19 | Enum (`lcd` / `Go`) | `0` | `0` |
| 27 | 28 | `true` flag (`jcd`) | `1` | `1` |
| 30 | 31 | Model category enum (`_.Dm`) | `[4]` or model enum | `[4]` / `[1]` / `[2]` / `[3]` / `[6]` |
| 41 | 42 | Mode picker option (`VEc` / `P$c`) | `[2]` | `[2]` or `[1]` |
| 53 | 54 | Boolean flag (`hcd` / `f8d`) | `0` | `0` |
| 59 | 60 | Client request UUID (`a.Hd`) | fresh UUID | fresh UUID |
| 61 | 62 | User context submessage (`_.aSc`) | `[]` | `[]` |
| 66 | 67 | Timestamp array (`uda`) | `[unix_secs, 0]` | `null` |
| 68 | 69 | Locale variant (`Gdd`) | `1` | `2` |
| 79 | 80 | Mode/experiment submessage (`_.udd`) | `6` | `6` or `3` |
| 80 | 81 | Model thinking mode enum (`wi` / `Zbd`) | not set | `1` or `null` |
| 91 | 92 | Boolean flag (`p` / `uda`) | `0` | `0` |
| 96 | 97 | Boolean flag (`t` / `Aa.Aa`) | `0` | `0` |

**Mode picker option values (slot 41):**

| Value | Meaning |
|-------|---------|
| `[1]` | Default / no explicit picker selection |
| `[2]` | Explicit model selection from the picker |

**Thinking-mode enum values (slot 80):**

| Value | Meaning |
|-------|---------|
| `0` | None / not specified |
| `1` | `THINKING_LEVEL_STANDARD` |
| `2` | `THINKING_LEVEL_EXTENDED` |
| `3` | `THINKING_LEVEL_DEEP_THINK` |

When a browser-captured payload is replayed, the proxy should only override the
slots it must control: slot 0 (prompt / attachments), slot 30 (model category),
and slot 59 (fresh request UUID). Browser-specific values in slots 3, 4, 6, 41,
66, 68, 79, and 80 should be preserved.

### Remaining unknowns

A few slots are present in captures but their exact semantics could not be
verified:

| Slot | Field | Observed | Notes |
|------|-------|----------|-------|
| 5 | 6 | `null` | Never populated in analyzed captures. |
| 18 | 19 | `0` | Enum `Go`; default `0`, purpose unknown. |
| 53 | 54 | `0` | Boolean flag `f8d`; default `false`. |
| 66 | 67 | `null` or `[ts, 0]` | Timestamp pair in some captures; not emitted by `_.Hdd`, likely merged from a side-channel context. |
| 79 | 80 | `3` or `6` | Submessage `_.udd` from `Kc.NM.cBa`; likely experiment/mode grouping. |
| 91 | 92 | `0` | Boolean flag `p` from `this.Aa.hb`; default `false`. |
| 96 | 97 | `0` | Boolean flag `t` from `this.Aa.Aa()`; default `false`. |

Slots not listed in this section were `null` in all available captures.

### Side-channel headers

Google's web frontend uses a few `x-goog-ext-*` headers to carry metadata that
is not part of the `f.req` protobuf. Captures show these headers on
`StreamGenerate`:

| Header | Example value | Purpose |
|--------|---------------|---------|
| `x-goog-ext-525005358-jspb` | `["<UUID>", 1]` | **Must match `inner_req_list[59]` (slot 59 / field 60).** The UUID is the client request id; the trailing `1` is a fixed marker. If the header UUID differs from the slot UUID the request is rejected. |
| `x-goog-ext-525001261-jspb` | varies | Secondary routing context; not required for basic requests. |
| `x-goog-ext-73010989-jspb` | varies | Telemetry / experiment marker. |
| `x-goog-ext-73010990-jspb` | varies | Telemetry / experiment marker. |

The proxy only needs to mirror `x-goog-ext-525005358-jspb` with the same value
it places in `inner_req_list[59]`. The other headers are optional in replays.

## Response frame layout

`StreamGenerate` returns `text/plain` with the same WIZ anti-XSSI prefix as
batchexecute:

```text
)]}'

[["wrb.fr",null,"<json-string>"]]
<length>
[["di",<n>],["af.httprm",<n>,"<token>",1]]
<length>
[["e",<n>,null,null,<total-length>]]
```

| Frame | Meaning |
|-------|---------|
| `wrb.fr` | The actual response payload. For a successful turn it is a JSON string containing the candidate list and meta fields. For errors it carries a `BardErrorInfo` protobuf entry. |
| `di` | End-of-stream / keepalive marker. |
| `af.httprm` | HTTP push / resume token; the trailing `1` means "done". |
| `e` | Final envelope with the total byte count. |

Error frames embed a `BardErrorInfo` message:

```json
[["wrb.fr", null, null, null, null,
  [13, null,
    [["type.googleapis.com/assistant.boq.bard.application.BardErrorInfo", [<code>]]]
  ]
]]
```

## Error codes

`StreamGenerate` errors are wrapped in a `BardErrorInfo` protobuf entry:

```json
[["wrb.fr", null, null, null, null,
  [13, null,
    [["type.googleapis.com/assistant.boq.bard.application.BardErrorInfo", [<code>]]]
  ]
]]
```

| Code | Meaning | Trigger |
|------|---------|---------|
| `1096` | Session/turn attestation or conversation state invalid. | Follow-up turn with missing/stale slot 3/slot 4 tokens, or replay of expired browser tokens. |
| `1100` | Image/file attestation failure. | Image turn with missing, invalid, or non-matching slot 3/slot 4 tokens. |
| `1155` | Session/parameter mismatch. | Real `f.sid` from WIZ with empty slot 3/slot 4 on text-only requests. |

On `1096` the proxy should clear the conversation state and start a fresh
conversation. On `1100` it should not retry the same image payload; fresh
browser attestation is required for that file. When browser attestation is
unavailable for text-only requests, prefer omitting `f.sid` (or sending a random
value) rather than using the real `f.sid` with empty attestation, to avoid
`1155`.

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

## Image / file uploads

OpenAI `image_url` data URLs, OpenAI Responses API `input_image` parts, and
Anthropic `image` blocks are converted to Gemini `Part::InlineData` by the
OpenAI and Anthropic converters. When cookie authentication is used, the proxy
uploads those bytes to the same Google resumable endpoint the Gemini web UI
uses:

```text
POST https://push.clients6.google.com/upload/
Headers:
  x-goog-upload-command: start
  x-goog-upload-header-content-length: <bytes>
  x-goog-upload-protocol: resumable
  x-tenant-id: bard-storage
  push-id: feeds/mcudyrk2a4khkz
Body: File name: <filename>
```

`push-id` is an upload feed identifier, not a session nonce. It tells Scotty /
Blobstore which feed the bytes belong to. The proxy should prefer the live
`WIZ_global_data.qKIAYe` value, fall back to the `GEMINI_PUSH_ID` environment
variable, and use `feeds/mcudyrk2a4khkz` as the last-resort default. The
alternate feed `feeds/nrij2vo2gajxiu` (from `KnDnFf`) returns references under
`/test/remy/skills/dropzone/`; the production image path uses
`feeds/mcudyrk2a4khkz`.

The response header `X-Goog-Upload-URL` contains the `upload_id` and the
finalize URL. The proxy then posts the raw bytes to that URL:

```text
POST <X-Goog-Upload-URL>
Headers:
  x-goog-upload-command: upload, finalize
  x-goog-upload-offset: 0
  push-id: feeds/mcudyrk2a4khkz
Body: <raw image bytes>
```

The response body is a `contrib_service` reference path such as
`/contrib_service/ttl_1d/<token>`. That reference is inserted into
`inner_req_list[0]` in the live-captured format. Slot 0 is the WIZ
serialization of the user-turn proto (`_.QN`). Its fields are:

| `inner_req_list[0]` index | Proto field | Meaning |
|---------------------------|-------------|---------|
| 0 | 1 | Prompt text (`setText`). |
| 1 | 2 | `Ah` boolean (`Acd`). |
| 2 | 3 | Always `null` in captures. |
| 3 | 4 | Attachment list (`_.zi(QN,4,B)`), present only when attachments exist. |
| 4 | 5 | `Mfa` string / override text, if any. |
| 5 | 6 | Always `null` in captures. |
| 6 | 7 | `Jh` boolean (`zcd`). |

When attachments are present, index `3` is an array of attachment tuples. Each
tuple is the WIZ serialization of a `_.QJ` message:

```json
[
  ["<reference>", <file_type>, null, "<mime_type>"],
  "<filename>",
  null, null, null, null, null, null,
  [0]
]
```

The first element is a `_.hvc` submessage (field 1 of `_.QJ`) with:

| Index | Field | Meaning |
|-------|-------|---------|
| 0 | 1 | Uploaded `contrib_service` reference path. |
| 1 | 2 | File type enum (`1` for inline image). |
| 2 | 3 | `rl` / role field; always `null` in captures. |
| 3 | 4 | MIME type, e.g. `image/png`. |

The second element is the file name. The trailing `[0]` at index 8 (field 9) is a
`_.bI` submessage placeholder; its exact meaning is unknown but it is required
for the attachment to be accepted.

Example slot 0 with one image:

```json
[
  "prompt text",
  0,
  null,
  [
    [
      ["/contrib_service/ttl_1d/...", 1, null, "image/png"],
      "test.png",
      null, null, null, null, null, null,
      [0]
    ]
  ],
  null,
  null,
  0
]
```

When no attachments are present slot 0 keeps the simple string-only format.

### Notable slot semantics

Slots whose role is not fully verified are marked with a question mark.

| Slot | Field | Notes |
|------|-------|-------|
| 1 | 2 | Locale string, e.g. `["en"]`. The value comes from `_.NJ` via `ocd`. |
| 2 | 3 | Conversation state (`_.FM`). Fields 1–3 are `conversation_id`, `xq`, `XY`; field 10 is the continuation token (`KQ`). |
| 6 | 7 | Locale wrapper (`_.rcd`). Captures always show `[1]`; the inner value is set by `qcd` from the locale service. |
| 7 | 8 | Hard-coded `true` by `kcd`. |
| 10 | 11 | Platform enum set by `mcd` / `_.MD(this.wb)`. Usually `1` for web. |
| 11 | 12 | Boolean flag `Jnb`, default `false`. |
| 17 | 18 | Turn counter. `[[0]]` for the first turn; `[[1]]` for follow-ups in a stateful conversation. |
| 18 | 19 | Enum `Go`, default `0`. Purpose unknown. |
| 27 | 28 | Hard-coded `true` by `jcd`. |
| 30 | 31 | Model category enum. See the category table above. |
| 41 | 42 | Mode picker option. `[1]` = default/no explicit selection; `[2]` = explicit picker selection. |
| 53 | 54 | Boolean flag `f8d`, default `false`. |
| 59 | 60 | Client request UUID. Must match `x-goog-ext-525005358-jspb`. |
| 61 | 62 | User context submessage (`_.aSc`). Captures show an empty array `[]`. |
| 66 | 67 | Timestamp pair `[unix_seconds, 0]` ? Only present in some captures. |
| 68 | 69 | Locale variant: `1` for English (`en`), `2` for non-English locales. |
| 79 | 80 | Mode/experiment submessage (`_.udd`). Values `3` or `6` observed; exact meaning unknown. |
| 80 | 81 | Thinking-level enum. Values listed above. |
| 91 | 92 | Boolean flag `p` from `this.Aa.hb`, default `false`. |
| 96 | 97 | Boolean flag `t` from `this.Aa.Aa()`, default `false`. |

The implementation lives in `src/gemini/web_frontend.rs`:

- `WebFrontendClient::upload_file` performs the two-step resumable upload.
- `upload_inline_attachments` extracts inline data parts from the request and
  uploads them before `StreamGenerate` is called.
- `build_inner_req_list` emits slot 0 with or without the attachment list.

### Image attestation requirement

Image uploads are **not** accepted by the Gemini web frontend unless the
`StreamGenerate` request carries a Google-signed attestation token in slot 3 and
a matching image hash in slot 4. These values are different from the
conversation attestation tokens used for text-only turns.

**There is no known non-browser source for valid image attestation tokens.** The
`SNlM0e` key is absent from `/app`, the base JS bundle does not contain static
BotGuard or token generation code, and no endpoint returns pre-computed slot 3 /
slot 4 values. The signed blob does not decode as plain base64, protobuf, gzip,
or zlib.

When the `browser-attestation` feature is enabled and `GEMINI_HEADLESS_BROWSER`
(or `CHROME_PATH`) is configured, the proxy:

1. Uploads the inline image bytes to `push.clients6.google.com/upload/` to
   obtain a `contrib_service` reference.
2. Pastes the same image into the headless Gemini UI via CDP.
3. Captures the outgoing `StreamGenerate` request using the CDP `Fetch`
   domain.
4. Extracts the image-specific slot 0[3] attachment array, slot 3 token, and
   slot 4 hash from the captured payload.
5. Replays those values into the proxy's own `StreamGenerate` request.

The captured `(bytes_hash -> WebAttachment)` tuple is cached in memory, so
identical images reuse the attestation token/hash without re-launching the
browser.

If browser attestation is disabled or the capture fails, the proxy falls back
to a synthesized token. This fallback is rejected by Gemini with `BardErrorInfo`
`1100` for image turns.

### Slot usage for image turns

| Slot | Image turn value |
|------|------------------|
| 0[3] | Attachment array captured from the browser (reference, mime type, filename). |
| 3    | Google-signed image attestation token (`Ijb`). |
| 4    | Image hash (`Jjb`) matching the token. |
| 30   | Model category enum. |

Supported attachment MIME types follow the Gemini web frontend: `image/png`,
`image/jpeg`, `image/webp`, `image/gif`, and `application/pdf`. File names are
derived from the MIME type (`image/png` -> `attachment.png`) and an index
suffix is added when a single request contains multiple attachments.

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

The headless browser requires a valid `SOCS` cookie to reach the authenticated
`/app` input area. Without `SOCS` the consent overlay blocks the prompt
`<textarea>` and CDP simulation times out. When auto-consent is implemented,
the proxy should obtain `SOCS` before launching the browser.

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
- `src/config.rs` — reads `GEMINI_HEADLESS_BROWSER` / `CHROME_PATH` and
  `GEMINI_PUSH_ID`.
