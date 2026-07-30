# gemini-proxy

OpenAI and Anthropic-compatible API proxy for Google Gemini with cookie-based authentication.

Drop-in replacement for both OpenAI and Anthropic APIs -- use any compatible client (LiteLLM, Open WebUI, Continue, Claude SDK, etc.) with Google Gemini models.

## Features

- **OpenAI-compatible API** -- drop-in replacement for `/v1/chat/completions`, `/v1/models`
- **Anthropic Messages API** -- drop-in replacement for `/v1/messages`
- **Responses API** -- full support for `POST /v1/responses` endpoint
- **Dual auth modes** -- Google API key or browser cookie-based authentication
- **Streaming support** -- full SSE streaming with `stream: true`
- **stream_options.include_usage** -- streaming usage chunks support
- **Tool/function calling** -- OpenAI and Anthropic tools format converted to Gemini function declarations
- **Vision support** -- base64 and URL image inputs via `image_url` content parts
- **Web frontend mode** -- uses Google's internal web API for cookie auth (no API key needed)
- **Reasoning/thinking model support** -- automatic mode detection for Pro, Thinking, and Flash models with `thinking_config` budget for Pro models
- **New OpenAI fields** -- `system_fingerprint`, `service_tier`, `parallel_tool_calls`, `reasoning_effort`, `store`, `metadata`
- **CORS enabled** -- configurable origins
- **Docker ready** -- multi-stage Dockerfile included
- **Bearer token auth** -- optional `AUTH_TOKEN` to protect your proxy endpoint
- **Rate limiting** -- configurable per-IP rate limiting

## Quick Start

### From source

```bash
# Clone
git clone https://github.com/vitkuz573/gemini-proxy.git
cd gemini-proxy

# Configure
cp .env.example .env  # or create manually
# Edit .env with your credentials

# Build and run
cargo build --release
./target/release/gemini-proxy
```

### With Docker

```bash
docker build -t gemini-proxy .
docker run -p 3000:3000 --env-file .env gemini-proxy
```

## Configuration

Create a `.env` file (or export as environment variables):

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:3000` | Address and port to listen on |
| `GEMINI_COOKIES` | -- | Browser cookies string for web frontend auth |
| `GEMINI_API_KEY` | -- | Google AI API key |
| `AUTH_TOKEN` | -- | Bearer token to protect the proxy endpoint |
| `MAX_RETRIES` | `2` | Max retry attempts for upstream requests |
| `GEMINI_BASE_URL` | `https://generativelanguage.googleapis.com` | Gemini API base URL |
| `RATE_LIMIT` | `60` | Max requests per minute per IP |
| `CORS_ORIGINS` | `*` | Comma-separated list of allowed CORS origins |

### Authentication

**Option 1: API Key** -- set `GEMINI_API_KEY`:

```env
GEMINI_API_KEY=AIzaSy...
```

**Option 2: Cookie auth** -- set `GEMINI_COOKIES` with browser cookies from [gemini.google.com](https://gemini.google.com):

```env
GEMINI_COOKIES=__Secure-1PSID=...; __Secure-1PAPISID=...; ...
```

Cookies rotate and expire. If `/v1/models` starts returning errors, refresh
`GEMINI_COOKIES` from a live browser session.

## API Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/v1/chat/completions` | Chat completion (OpenAI-compatible) |
| `POST` | `/v1/messages` | Messages API (Anthropic-compatible) |
| `POST` | `/v1/responses` | Responses API (OpenAI-compatible) |
| `GET` | `/v1/models` | List available models |
| `GET` | `/v1/models/{model}` | Get model details |
| `GET` | `/health` | Health check |
| `GET` | `/` | Server info |

### Example usage

```bash
# Non-streaming
curl http://localhost:3000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "current-model",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# Streaming
curl http://localhost:3000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "current-model",
    "messages": [{"role": "user", "content": "Tell me a story"}],
    "stream": true
  }'

# With auth token
curl http://localhost:3000/v1/chat/completions \
  -H "Authorization: Bearer your-token" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "current-model",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

### OpenAI SDK / LiteLLM

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://localhost:3000/v1",
    api_key="not-needed"  # or your AUTH_TOKEN
)

response = client.chat.completions.create(
    model="current-model",
    messages=[{"role": "user", "content": "Hello!"}]
)
print(response.choices[0].message.content)
```

### Responses API

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://localhost:3000/v1",
    api_key="not-needed"  # or your AUTH_TOKEN
)

response = client.responses.create(
    model="current-model",
    input="Hello!"
)
print(response.output[0].content[0].text)
```

```bash
# Responses API with curl
curl http://localhost:3000/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "current-model",
    "input": "What is the capital of France?"
  }'
```

### Anthropic Messages API

```python
import anthropic

client = anthropic.Anthropic(
    base_url="http://localhost:3000",
    api_key="not-needed"  # or your AUTH_TOKEN
)

message = client.messages.create(
    model="current-model",
    max_tokens=1024,
    messages=[{"role": "user", "content": "Hello!"}]
)
print(message.content[0].text)
```

```bash
# Anthropic Messages API with curl
curl http://localhost:3000/v1/messages \
  -H "Content-Type: application/json" \
  -d '{
    "model": "current-model",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Models

When using an API key (`GEMINI_API_KEY`), the available models are whatever Google exposes through the official Generative Language API.

When using cookie auth (`GEMINI_COOKIES`), the proxy discovers live models from the Gemini web frontend model picker and exposes them as human-readable IDs like `gemini-3.6-flash`. The raw hex ID is available in the `root` field (e.g. `models/fbb127bbb056c959`).

You must set `model` in every request. Call `GET /v1/models` first to see available models. See [docs/protocol.md](docs/protocol.md) for the full reverse-engineered protocol details.

## License

MIT
