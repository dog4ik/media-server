FROM rustlang/rust:nightly AS chef
RUN cargo install cargo-chef 
WORKDIR app

# -----------------------------

FROM chef AS planner
COPY . .
RUN cargo chef prepare  --recipe-path recipe.json

# -----------------------------

FROM chef AS builder
RUN apt-get update && apt-get install -y \
      pkg-config ffmpeg libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libavdevice-dev libswscale-dev clang \
      curl jq \
      && rm -rf /var/lib/apt/lists/*
RUN cargo install sqlx-cli --no-default-features --features sqlite
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json
# Build application
COPY . .

# Fetch frontend artifacts
RUN MINOR_VERSION=$(sed -n 's/^version = "\([0-9]*\.[0-9]*\).*/\1/p' Cargo.toml) && \
    FRONTEND_VERSION=$(curl -fsSL https://api.github.com/repos/dog4ik/media-server-web/releases \
      | jq -r "[.[] | select(.tag_name | startswith(\"v${MINOR_VERSION}\"))] | last | .tag_name") && \
    curl -fsSL https://github.com/dog4ik/media-server-web/releases/download/${FRONTEND_VERSION}/dist.tar.gz \
      | tar -xz

ENV DATABASE_URL=sqlite://database.sqlite
RUN cargo sqlx database setup
RUN cargo build --release --bin media-server

# -----------------------------

FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y \
      pkg-config ffmpeg libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libavdevice-dev libswscale-dev clang \
      libssl3 ca-certificates curl \
      && rm -rf /var/lib/apt/lists/*
WORKDIR app
COPY --from=builder /app/target/release/media-server /usr/local/bin
COPY --from=builder /app/dist /usr/share/media-server/dist
ENTRYPOINT ["/usr/local/bin/media-server"]
