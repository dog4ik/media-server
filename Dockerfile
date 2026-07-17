FROM rust:1.96.0 AS chef
RUN cargo install cargo-chef
WORKDIR /app

# -----------------------------

FROM chef AS planner
COPY . .
RUN cargo chef prepare  --recipe-path recipe.json

# -----------------------------

FROM chef AS builder
RUN apt-get update && apt-get install --no-install-recommends -y \
      pkg-config libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libavdevice-dev libswscale-dev clang \
      curl jq ca-certificates \
      && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .

# Fetch frontend artifacts
RUN MINOR_VERSION=$(sed -n 's/^version = "\([0-9]*\.[0-9]*\).*/\1/p' Cargo.toml) && \
    FRONTEND_VERSION=$(curl -fsSL https://api.github.com/repos/dog4ik/media-server-web/releases \
      | jq -r "[.[] | select(.tag_name | startswith(\"v${MINOR_VERSION}\"))] | sort_by(.tag_name) | last | .tag_name") && \
    curl -fsSL https://github.com/dog4ik/media-server-web/releases/download/${FRONTEND_VERSION}/dist.tar.gz \
      | tar -xz

# Builds against the committed .sqlx cache, so no sqlx-cli or database is needed.
RUN SQLX_OFFLINE=true cargo build --release --bin media-server

# -----------------------------

FROM debian:trixie-slim AS runtime
# Only the ffmpeg/ffprobe CLIs and the shared libav libs are needed at runtime.
# The mesa/LLVM stack is not needed so purge it
RUN apt-get update && apt-get install --no-install-recommends -y \
      ffmpeg ca-certificates \
      && dpkg --purge --force-depends libllvm19 mesa-libgallium libgl1-mesa-dri libglx-mesa0 libz3-4 \
      && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/media-server /usr/local/bin
COPY --from=builder /app/dist /usr/share/media-server/dist
ENTRYPOINT ["/usr/local/bin/media-server"]
