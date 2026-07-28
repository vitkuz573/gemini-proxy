# gemini2openai

OpenAI-compatible API proxy for Google Gemini with cookie-based authentication.

Drop-in replacement for OpenAI API — use any OpenAI-compatible client (LiteLLM, Open WebUI, Continue, etc.) with Google Gemini models.

## Features

- **OpenAI-compatible API** — drop-in replacement for `/v1/chat/completions`, `/v1/models`
- **Dual auth modes** — Google API key or browser cookie-based authentication
- **Streaming support** — full SSE streaming with `stream: true`
- **Tool/function calling** — OpenAI tools format converted to Gemini function declarations
- **Vision support** — base64 and URL image inputs via `image_url` content parts
- **Web frontend mode** — uses Google's internal web API for cookie auth (no API key needed)
- **Thinking model support** — automatic mode detection for Pro, Thinking, and Flash models
- **Docker ready** — multi-stage Dockerfile included
- **Bearer token auth** — optional `AUTH_TOKEN` to protect your proxy endpoint

## Quick Start

### From source

```bash
# Clone
git clone https://github.com/vitkuz573/gemini2openai.git
cd gemini2openai

# Configure
cp .env.example .env  # or create manually
# Edit .env with your credentials

# Build and run
cargo build --release
./target/release/gemini2openai
```

### With Docker

```bash
docker build -t gemini2openai .
docker run -p 3000:3000 --env-file .env gemini2openai
```

## Configuration

Create a `.env` file (or export as environment variables):

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:3000` | Address and port to listen on |
| `GEMINI_COOKIES` | — | Browser cookies string for web frontend auth |
| `GEMINI_API_KEY` | — | Google AI API key |
| `AUTH_TOKEN` | — | Bearer token to protect the proxy endpoint |
| `DEFAULT_MODEL` | `gemini-2.5-flash` | Default model when none specified in request |
| `MAX_RETRIES` | `2` | Max retry attempts for upstream requests |
| `GEMINI_BASE_URL` | `https://generativelanguage.googleapis.com` | Gemini API base URL |

### Authentication

**Option 1: API Key** — set `GEMINI_API_KEY`:

```env
GEMINI_API_KEY=AIzaSy...
```

**Option 2: Cookie auth** — set `GEMINI_COOKIES` with browser cookies from [gemini.google.com](https://gemini.google.com):

```env
GEMINI_COOKIES=__Secure-1PSID=...; __Secure-1PAPISID=...; ...
```

## API Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/v1/chat/completions` | Chat completion (OpenAI-compatible) |
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
    "model": "gemini-2.5-flash",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# Streaming
curl http://localhost:3000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-2.5-flash",
    "messages": [{"role": "user", "content": "Tell me a story"}],
    "stream": true
  }'

# With auth token
curl http://localhost:3000/v1/chat/completions \
  -H "Authorization: Bearer your-token" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-2.5-flash",
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
    model="gemini-2.5-flash",
    messages=[{"role": "user", "content": "Hello!"}]
)
print(response.choices[0].message.content)
```

## License

MIT
