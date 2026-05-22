# Dockerfile multi-stage para api-postgres-python.
#
# A diferencia de los otros boilerplates (cli-tool, api-simple,
# api-middleware-cors, api-websocket) que usan `fitz build` para
# producir un binario standalone distroless, este boilerplate
# usa **`fitz run`** porque la interop Python requiere CPython
# en el destino. Imagen base `python:3.12-slim` con `fitz`
# compilado con `--features python`.
#
# Stage 1 (builder): instala Rust toolchain + Python dev headers
# (PyO3 los necesita para linkear con libpython), clona el repo
# Fitz desde GitHub (público) y compila con `--features python`.
# El binario sale en /usr/local/cargo/bin/fitz.
#
# Stage 2 (runtime): python:3.12-slim, copia el binario fitz +
# instala el `requirements.txt` (SQLAlchemy + psycopg2-binary) +
# copia el código del proyecto. CMD final corre `fitz run`.
#
# Build inicial es lento (~8-12 min): cargo compila fitz +
# todas las deps incluyendo pyo3, jsonwebtoken, argon2, axum,
# tokio. Builds subsiguientes son cacheados si solo cambia el
# código del proyecto.

# ---- Stage 1: builder ----------------------------------------------
FROM rust:1.85-slim AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        python3 python3-dev pkg-config libssl-dev ca-certificates git && \
    rm -rf /var/lib/apt/lists/*

# Clonar Fitz desde el repo público. Pineamos a una versión
# estable conocida — cambiar acá cuando salga una versión nueva
# del compilador con fixes que querramos para el boilerplate.
ARG FITZ_REF=main

RUN cargo install --git https://github.com/Thegreekman76/fitz \
    --rev ${FITZ_REF} \
    --features python \
    --bin fitz \
    --locked

# ---- Stage 2: runtime ----------------------------------------------
FROM python:3.12-slim

# Dependencias del runtime: libpython (ya viene con python:slim) +
# libpq para psycopg2 (postgres client lib).
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        libpq5 ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Binario fitz del stage builder.
COPY --from=builder /usr/local/cargo/bin/fitz /usr/local/bin/fitz

WORKDIR /app

# Pip install requirements primero — capa cacheable separada
# del código del proyecto (los requirements cambian menos seguido
# que el código).
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copia del proyecto.
COPY fitz.toml ./
COPY src/ ./src/
COPY python/ ./python/

# Fitz invoca `from python import db` que llama a CPython —
# CPython busca módulos en `sys.path` (entry default: cwd +
# site-packages). PYTHONPATH extiende el lookup; acá apunta a
# `python/` donde viven `db.py` y `models.py`.
ENV PYTHONPATH=/app/python

EXPOSE 3000

CMD ["fitz", "run", "src/main.fitz"]
