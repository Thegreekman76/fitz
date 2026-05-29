# Dockerfile multi-stage para api-postgres-python.
#
# Usa `fitz run --features python` (interop con CPython al runtime).
# Patrón distinto a los otros boilerplates Fitz puros: en lugar de
# buildear a binario standalone distroless, mantenemos Python en el
# runtime (libpython3.12 + pip install sqlalchemy + psycopg2).
#
# **v0.10.13 (2026-05-29)**: migrado de `cargo install --git` (que
# compilaba Fitz + PyO3 + deps desde source en ~8-12 min) al patrón
# pre-built `ghcr.io/thegreekman76/fitz:latest-python`. Reducción
# ~10x del build time. La imagen `:latest-python` se actualiza en
# cada release del repo (workflow `.github/workflows/release.yml`,
# job `docker-image-python`) — ya viene con fitz `--features python`
# instalado, libpython3.12 + dev headers, y Rust toolchain (este
# último descartado en stage 2).
#
# **IMPORTANTE — match Python entre builder y runtime**: la imagen
# `:latest-python` se construye sobre `python:3.12-slim` (mismo base
# que nuestro runtime). El binario fitz queda linkeado contra la
# libpython3.12 que el runtime también provee. **Constraint
# arquitectural permanente** de PyO3 + Linux — en Linux NO existe
# el equivalente al `python3.dll` stable-ABI shim de Windows
# (verificado empíricamente 2026-05-24; ver
# `docs/deudas_lenguaje.md` sección R.bug-pyo3-abi3-portable-link).
#
# Build con LATEST (default — recomendado):
#   docker compose build
#
# Build con tag específico (reproducibilidad pinned):
#   docker compose build --build-arg FITZ_TAG=v0.10.13-python

# ---- Stage 1: builder ----------------------------------------------
# Imagen pre-built con fitz `--features python` lista para usar.
# Trae libpython3.12, Rust toolchain (no usado acá), y el binario
# `fitz` en `/usr/local/bin/fitz`.
ARG FITZ_TAG=latest-python
FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder

# ---- Stage 2: runtime ----------------------------------------------
# `python:3.12-slim` limpio + COPY del binario fitz del builder.
# Sin Rust toolchain ni build-essential — ese baggage queda en el
# builder stage descartado, mantiene la imagen final liviana.
FROM python:3.12-slim

# libpq5: client lib de Postgres que psycopg2 necesita en runtime.
# ca-certificates: HTTPS outbound (no usado por este boilerplate
# pero típico requirement de cualquier service moderno).
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        libpq5 ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Binario fitz del stage builder. La imagen `:latest-python` lo deja
# en `/usr/local/bin/fitz` (ver `.github/workflows/release.yml` →
# `docker-image-python` job).
COPY --from=builder /usr/local/bin/fitz /usr/local/bin/fitz

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
