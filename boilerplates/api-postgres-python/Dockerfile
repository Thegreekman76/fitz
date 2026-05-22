# Dockerfile multi-stage para api-postgres-python.
#
# Usa `fitz run --features python` (interop con CPython al runtime).
# Patrón distinto a los otros 4 boilerplates: en lugar de buildear
# a binario standalone distroless, mantenemos Python en el runtime.
#
# Builder + runtime ahora pueden tener Python versions distintas
# gracias al fix de R.bug-pyo3-abi3-autoinit (mini-tanda
# Cleanup-Residual+ 2026-05-22): quitamos `auto-initialize` del
# Cargo.toml de Fitz, ahora el binario es portable abi3-py310 — corre
# contra cualquier Python 3.10+. Usamos `rust:slim` como builder
# (más rápido) y `python:3.12-slim` como runtime.
#
# Build inicial: ~6-9 min (compila fitz + deps desde source).
# Builds subsiguientes: ~30s si solo cambia src/ o python/.
#
# Build con tag específico:
#   docker build --build-arg FITZ_TAG=v0.9.25 .

# ---- Stage 1: builder ----------------------------------------------
# Rust:slim ya trae Rust 1.95+ pre-instalado. Solo agregamos Python +
# headers + libssl para que pyo3 pueda buildear. El abi3-py310 del
# Cargo.toml de Fitz hace que el binario resultante corra contra
# cualquier Python 3.10+, así que la versión del builder no condiciona
# el runtime.
FROM rust:slim AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        git python3 python3-dev pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

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
# python:3.12-slim — la versión específica importa relativamente poco
# después del fix abi3-py310 puro. El binario fitz corre contra
# cualquier Python 3.10+.
FROM python:3.12-slim

# libpq5: client lib de Postgres que psycopg2 necesita en runtime.
# ca-certificates: HTTPS outbound (no usado por este boilerplate
# pero típico requirement de any service moderno).
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        libpq5 ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Binario fitz del stage builder. abi3-py310 garantiza compatibilidad
# con la libpython del runtime (cualquier 3.10+).
COPY --from=builder /usr/local/cargo/bin/fitz /usr/local/bin/fitz

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
