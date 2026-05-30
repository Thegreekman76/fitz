# Dockerfile multi-stage para api-multi-tenant.
#
# Showcase del feature schemas custom de v0.10.21:
# `@table("schema.name")` permite que tablas vivan en schemas
# Postgres no-`public`. Útil para SaaS multi-tenant con aislamiento
# de datos a nivel DB (no a nivel WHERE clause como suele hacerse
# en Rails/Django).
#
# Build:
#   docker compose build
#
# Run:
#   docker compose up

ARG FITZ_TAG=latest
FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder

WORKDIR /app
COPY fitz.toml ./
COPY src/ ./src/

RUN fitz build src/main.fitz

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/src/main /usr/local/bin/app

EXPOSE 3000

CMD ["/usr/local/bin/app"]
