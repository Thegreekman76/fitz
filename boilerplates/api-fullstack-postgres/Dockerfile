# Dockerfile multi-stage para api-fullstack-postgres.
#
# Build inicial: ~8-12 min (compila fitz + deps desde source).
# Builds subsiguientes: ~30s si solo cambia src/ o python/ (cache
# buildkit reusa la capa de cargo install).
#
# Alternativa rápida (desde v0.9.36): si querés evitar los ~5-8 min
# de build inicial, usá la imagen pre-buildeada `:latest-python`:
#
#   FROM ghcr.io/thegreekman76/fitz:latest-python AS builder
#   # ... resto sin cambios
#
# Trade-off: tag floating; mejor pin a `v0.9.X-python` para
# reproducibilidad. El Dockerfile actual sigue clonando desde source
# para que el build sea independiente del tag de la imagen.
#
# IMPORTANTE — match Python entre builder y runtime: aunque
# `auto-initialize` ya no está en el Cargo.toml de Fitz (mini-tanda
# Cleanup-Residual+ 2026-05-22), el binario sigue linkeando contra
# la libpython específica del builder.
#
# **Constraint arquitectural** (no deuda cerrable): en Linux NO
# existe equivalente al `python3.dll` stable-ABI shim de Windows.
# Verificado empíricamente 2026-05-24 con experimento Docker —
# `/usr/local/lib/libpython3.so` en `python:3.X-slim` es un dummy
# de 13 KB sin símbolos del API Python. Ver
# `docs/deudas_lenguaje.md` sección R.bug-pyo3-abi3-portable-link.
#
# Solución permanente: `python:3.12-slim` como base de AMBOS stages.

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
