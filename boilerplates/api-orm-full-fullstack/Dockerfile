# Dockerfile multi-stage para api-orm-full.
#
# Showcase del stack web first-class entero (HTTP + auth + WS + cron
# + Postgres ORM) en un solo binario standalone. No usa Python, no
# usa SQLAlchemy, no necesita libpython ni pip en el runtime —
# todo el driver Postgres v3.0 wire protocol + ORM declarativo
# viven en el binario `fitz` mismo.
#
# Resultado: imagen final ~30-50 MB (multi-archivo, varios handlers,
# WS, cron). Build inicial ~30-60s (descarga imagen base + compila
# el proyecto). Builds subsiguientes ~10-20s con cache.
#
# **v0.10.11 (2026-05-29)**: migrado de `cargo install --git` (que
# compilaba Fitz desde source en ~5-8min) al patrón pre-built
# `ghcr.io/thegreekman76/fitz:latest`. Reducción ~10x del build
# time. La imagen `:latest` se actualiza en cada release del repo
# (workflow `.github/workflows/release.yml`).
#
# Build con LATEST (default — recomendado para uso normal):
#   docker compose build
#
# Build con tag específico (reproducibilidad pinned):
#   docker compose build --build-arg FITZ_TAG=v0.10.10

# ---- Stage 1: builder ----------------------------------------------
ARG FITZ_TAG=latest
FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder

# Copia del proyecto. Multi-archivo: el codegen carga los módulos
# transitivos via fitz.toml ([bin].main = src/main.fitz).
WORKDIR /app
COPY fitz.toml ./
COPY src/ ./src/

# Compilación al binario nativo. El binario embebe TODO: driver
# Postgres + ORM + axum + tokio + axum WS + auth (JWT/Argon2id) +
# cron scheduler. Sin runtime de Fitz en producción.
RUN fitz build src/main.fitz

# ---- Stage 2: runtime ----------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# El binario producido por `fitz build src/main.fitz` queda en
# `src/main` (sin extensión en Linux). Lo renombramos para hacer el
# CMD limpio.
COPY --from=builder /app/src/main /usr/local/bin/app

EXPOSE 3000

CMD ["/usr/local/bin/app"]
