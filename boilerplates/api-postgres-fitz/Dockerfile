# Dockerfile multi-stage para api-postgres-fitz.
#
# A diferencia de `api-postgres-python`, este boilerplate NO usa
# Python, NO usa SQLAlchemy, NO necesita `pip install` ni libpython
# en el runtime. El ORM nativo de Fitz (cap 31 de la guía) habla
# wire protocol Postgres v3.0 directo desde el binario standalone.
#
# Resultado: imagen final ~30-50 MB (vs ~250 MB de api-postgres-python).
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

# Copia del proyecto y compilación a binario nativo. El binario
# resultante embebe el driver Postgres + ORM, no necesita nada más.
WORKDIR /app
COPY fitz.toml ./
COPY src/ ./src/

RUN fitz build src/main.fitz

# ---- Stage 2: runtime ----------------------------------------------
# Imagen minimalista — solo el binario fitz-app + ca-certificates
# para HTTPS outbound (no usado aquí pero estándar de servicios).
# `debian:bookworm-slim` mantiene compatibilidad con cualquier
# distro Debian/Ubuntu sin matching de versiones libpython como
# necesita api-postgres-python.
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# El binario producido por `fitz build src/main.fitz` queda en
# `src/main` (sin extensión en Linux). Lo renombramos a `app`
# para hacer el CMD limpio.
COPY --from=builder /app/src/main /usr/local/bin/app

EXPOSE 3000

CMD ["/usr/local/bin/app"]
