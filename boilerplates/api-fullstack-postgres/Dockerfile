# Dockerfile multi-stage para api-fullstack-postgres.
#
# Mismo patrón que api-postgres-python: ambos stages basados en
# python:3.12-slim porque PyO3 0.28 con `auto-initialize` + `abi3-py310`
# ignora abi3 y linkea contra la libpython del builder. Si los stages
# usan Pythons distintos (3.13 builder + 3.12 runtime), el binario
# fitz falla en boot con "libpython3.X.so.1.0 not found".
#
# Build inicial: ~8-12 min (compila fitz + deps desde source).
# Builds subsiguientes: ~30s si solo cambia src/ o python/ (cache
# buildkit reusa la capa de cargo install).

# ---- Stage 1: builder ----------------------------------------------
FROM python:3.12-slim AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        curl ca-certificates git \
        python3-dev pkg-config libssl-dev build-essential && \
    rm -rf /var/lib/apt/lists/*

# Rust 1.95+ (Fitz pinea `rust-version = "1.95"` en Cargo.toml).
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain 1.95.0 --profile minimal
ENV PATH="/root/.cargo/bin:${PATH}"

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

COPY --from=builder /root/.cargo/bin/fitz /usr/local/bin/fitz

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
