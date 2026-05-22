# Dockerfile multi-stage para el API Fitz del boilerplate
# api-websocket. Patrón idéntico a api-simple/api-middleware-cors.

FROM ghcr.io/thegreekman76/fitz:latest AS builder

WORKDIR /app

COPY fitz.toml ./
COPY src/ ./src/

RUN fitz build

FROM gcr.io/distroless/cc-debian12

COPY --from=builder /app/target/release/fitz-api-websocket /usr/local/bin/app

EXPOSE 43929

ENTRYPOINT ["/usr/local/bin/app"]
