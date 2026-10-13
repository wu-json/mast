FROM rust:1.99-slim AS build
WORKDIR /mast
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release --locked

FROM ghcr.io/charmbracelet/vhs:latest
RUN apt-get update \
    && apt-get install -y --no-install-recommends fonts-noto-color-emoji \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /mast/target/release/mast /usr/local/bin/mast
COPY demo /demo
ENV HOME=/home/dev
WORKDIR /out
ENTRYPOINT ["/demo/record.sh"]
