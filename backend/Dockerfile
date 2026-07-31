# ---------- dev ----------
# Hot-reload via cargo-watch. O código vem por bind mount (ver docker-compose.yml),
# então nada de COPY do source aqui.
FROM rust:1-slim-bookworm AS dev

RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-watch --locked

WORKDIR /app
ENV CARGO_TARGET_DIR=/app/target

CMD ["cargo", "watch", "-w", "src", "-w", "migrations", "-w", "Cargo.toml", "-x", "run"]

# ---------- build (produção) ----------
FROM rust:1-slim-bookworm AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
# camada de cache de dependências
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src
COPY . .
RUN touch src/main.rs && cargo build --release

# ---------- runtime ----------
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/odeon /usr/local/bin/odeon
ENV ODEON_BIND=0.0.0.0:8080
EXPOSE 8080
CMD ["odeon"]
