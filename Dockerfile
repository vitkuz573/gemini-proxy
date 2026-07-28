FROM rust:1.82-bookworm AS builder

WORKDIR /app
COPY src ./src
COPY Cargo.toml Cargo.lock ./
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/gemini2openai /usr/local/bin/

EXPOSE 3000
ENTRYPOINT ["gemini2openai"]