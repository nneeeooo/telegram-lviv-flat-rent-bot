# ==========================================
# Builder Stage
# ==========================================
FROM docker.io/library/rust:alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app

# Cache dependency compilation
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Build actual binary (data/streets.json is compiled in via include_str!)
COPY data ./data
COPY src ./src
RUN touch src/main.rs && cargo build --release

# ==========================================
# Runtime Stage (LTS Alpine 3.21)
# ==========================================
FROM docker.io/library/alpine:3.21

RUN apk add --no-cache ca-certificates tzdata

WORKDIR /app

COPY --from=builder /app/target/release/telegram-lviv-flat-rent-bot /app/telegram-lviv-flat-rent-bot
COPY config.toml ./

RUN mkdir -p /app/state

VOLUME ["/app/state"]

CMD ["/app/telegram-lviv-flat-rent-bot"]
