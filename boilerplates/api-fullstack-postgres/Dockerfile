# Dockerfile multi-stage para api-fullstack-postgres.
#
# Build inicial: ~6-9 min (compila fitz + deps desde source).
# Builds subsiguientes: ~30s si solo cambia src/ o python/ (cache
# buildkit reusa la capa de cargo install).
#
# Builder + runtime ahora pueden tener Python versions distintas
# gracias al fix de R.bug-pyo3-abi3-autoinit (mini-tanda
# Cleanup-Residual+ 2026-05-22): el binario fitz es portable
# abi3-py310 — corre contra cualquier Python 3.10+. Usamos
# `rust:slim` como builder (más rápido, Rust pre-instalado) y
# `python:3.12-slim` como runtime.

# ---- Stage 1: builder ----------------------------------------------
FROM rust:slim AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        git python3 python3-dev pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Pin opcional de versión de Fitz:
#   docker build --build-arg FITZ_TAG=v0.9.25 .
#   docker build --build-arg FITZ_REV=abc123 .
#   docker build --build-arg FITZ_BRANCH=main .
ARG FITZ_BRANCH=
ARG FITZ_TAG=
ARG FITZ_REV=

RUN if [ -n "$FITZ_REV" ]; then \
        cargo install --git https://github.com/Thegreekman76/fitz --rev "$FITZ_REV" --features python --bin fitz --locked; \
    elif [ -n "$FITZ_TAG" ]; then \
        cargo install --git https://github.com/Thegreekman76/fitz --tag "$FITZ_TAG" --features python --bin fitz --locked; \
    elif [ -n "$FITZ_BRANCH" ]; then \
        cargo install --git https://github.com/Thegreekman76/fitz --branch "$FITZ_BRANCH" --features python --bin fitz --locked; \
    else \
        cargo install --git https://github.com/Thegreekman76/fitz --features python --bin fitz --locked; \
    fi

# ---- Stage 2: runtime ----------------------------------------------
FROM python:3.12-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        libpq5 ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/cargo/bin/fitz /usr/local/bin/fitz

WORKDIR /app

# Pip install primero — capa cacheable separada del código.
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copia del proyecto Fitz (multi-archivo: src/main.fitz +
# src/types/*.fitz + src/data/*.fitz) + Python helpers.
COPY fitz.toml ./
COPY src/ ./src/
COPY python/ ./python/

# `from python import db` busca db.py en sys.path. `python/` no
# está en el default — extendemos.
ENV PYTHONPATH=/app/python

EXPOSE 3000

CMD ["fitz", "run", "src/main.fitz"]
