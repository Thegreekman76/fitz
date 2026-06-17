# Dockerfile multi-stage para apps/fitz/ del benchmark mixed-workload.
#
# Paralelo bit-a-bit al Dockerfile de `boilerplates/api-postgres-fitz/`
# pero apunta al ORM nativo + dominio users+posts del bench.
#
# Build con LATEST (default):
#   docker compose build
#
# Build con tag específico (reproducibilidad):
#   docker compose build --build-arg FITZ_TAG=v0.16.0

# ---- Stage 1: builder ----------------------------------------------
ARG FITZ_TAG=latest
FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder

WORKDIR /app
COPY fitz.toml ./
COPY src/ ./src/

RUN fitz build src/main.fitz

# ---- Stage 2: runtime ----------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/src/main /usr/local/bin/app

EXPOSE 3000

CMD ["/usr/local/bin/app"]
