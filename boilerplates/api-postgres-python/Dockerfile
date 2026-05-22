# Dockerfile multi-stage para api-postgres-python.
#
# Usa `fitz run --features python` (interop con CPython al runtime).
# Patrón distinto a los otros 4 boilerplates: en lugar de buildear
# a binario standalone distroless, mantenemos Python en el runtime.
#
# IMPORTANTE — match de Python entre stages:
# PyO3 0.28 con `auto-initialize` (combinado con `abi3-py310` en el
# Cargo.toml de Fitz) IGNORA `abi3` y linkea el binario contra la
# versión específica de libpython detectada en el builder. Si los
# stages usan Pythons distintos (builder 3.13 + runtime 3.12), el
# binario falla con "cannot open shared object libpython3.X.so.1.0"
# al boot del runtime.
#
# Solución: usar `python:3.12-slim` como base de AMBOS stages. El
# builder agrega Rust con `rustup`; el runtime descarta Rust y
# mantiene solo Python + libpq + el binario fitz.
#
# Build inicial: ~8-12 min (compila fitz + deps desde source).
# Builds subsiguientes: ~30s si solo cambia src/ o python/.
#
# Build con tag específico:
#   docker build --build-arg FITZ_TAG=v0.9.25 .

# ---- Stage 1: builder ----------------------------------------------
FROM python:3.12-slim AS builder

# Dev headers para que pyo3 detecte Python correctamente, plus
# herramientas de compilación que Rust crates necesitan
# (build-essential trae gcc/g++/make; libssl-dev + pkg-config son
# necesarios para algunos crates de Fitz).
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        curl ca-certificates git \
        python3-dev pkg-config libssl-dev build-essential && \
    rm -rf /var/lib/apt/lists/*

# Rust 1.95+ (Fitz pinea `rust-version = "1.95"` en Cargo.toml).
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain 1.95.0 --profile minimal
ENV PATH="/root/.cargo/bin:${PATH}"

# Clonar Fitz desde el repo público.
# Por default toma `HEAD` del default branch (main). Para pinear:
#
#   docker build --build-arg FITZ_BRANCH=main      .   # branch (default)
#   docker build --build-arg FITZ_TAG=v0.9.25      .   # tag (versión)
#   docker build --build-arg FITZ_REV=abc123def    .   # commit SHA
#
# Para reproducibilidad en producción, lean tag (apunta a versión
# release con .vsix asociados; SHA puede pertenecer a un branch
# que después se reescribió).
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
# MISMA imagen base que el builder → libpython3.12 garantizado.
FROM python:3.12-slim

# libpq5: client lib de Postgres que psycopg2 necesita en runtime.
# ca-certificates: HTTPS outbound (no usado por este boilerplate
# pero típico requirement de any service moderno).
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        libpq5 ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Binario fitz del stage builder (lincado contra libpython3.12 del
# builder, que es la misma que el runtime tiene).
COPY --from=builder /root/.cargo/bin/fitz /usr/local/bin/fitz

WORKDIR /app

# Pip install requirements primero — capa cacheable separada del
# código del proyecto (los requirements cambian menos seguido).
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copia del proyecto.
COPY fitz.toml ./
COPY src/ ./src/
COPY python/ ./python/

# `from python import db` en Fitz invoca CPython, que busca módulos
# en `sys.path` (entry default: cwd + site-packages). PYTHONPATH
# extiende el lookup; acá apunta a `python/` donde viven db.py y
# models.py.
ENV PYTHONPATH=/app/python

EXPOSE 3000

CMD ["fitz", "run", "src/main.fitz"]
