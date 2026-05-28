# Dockerfile multi-stage para api-postgres-python.
#
# Usa `fitz run --features python` (interop con CPython al runtime).
# Patrón distinto a los otros 4 boilerplates: en lugar de buildear
# a binario standalone distroless, mantenemos Python en el runtime.
#
# Alternativa rápida (desde v0.9.36): si querés evitar los ~5-8 min
# del primer build (cargo install fitz --features python), usá la
# imagen pre-buildeada como base:
#
#   FROM ghcr.io/thegreekman76/fitz:latest-python AS builder
#   # ... resto sin cambios
#
# Trade-off: tag floating (mejor pin a `v0.9.X-python` para
# reproducibilidad). El Dockerfile actual sigue clonando + buildeando
# desde source para que el build sea independiente del tag de la
# imagen.
#
# IMPORTANTE — match Python entre builder y runtime:
# Aunque el Cargo.toml de Fitz ya NO tiene `auto-initialize`
# (mini-tanda Cleanup-Residual+ 2026-05-22 removió ese feature
# para limpieza del init lazy), el binario sigue linkeando contra
# la libpython específica del builder por default de PyO3 0.28.
#
# **Constraint arquitectural** (no deuda cerrable): en Linux NO
# existe equivalente al `python3.dll` stable-ABI shim de Windows.
# El archivo `/usr/local/lib/libpython3.so` que traen las imágenes
# `python:3.X-slim` es un dummy de 13 KB que solo exporta símbolos
# glibc (no exporta el API Python). Los símbolos abi3 viven SOLO
# en `libpython3.X.so.1.0` (versioned). Verificado empíricamente
# 2026-05-24 con experimento Docker; ver `docs/deudas_lenguaje.md`
# sección R.bug-pyo3-abi3-portable-link.
#
# Solución permanente: usar `python:3.12-slim` como base de AMBOS
# stages. El builder agrega Rust con `rustup`; el runtime descarta
# Rust y mantiene solo Python + libpq + el binario fitz.
#
# Build inicial: ~8-12 min (compila fitz + deps desde source).
# Builds subsiguientes: ~30s si solo cambia src/ o python/.
#
# Build con tag específico:
#   docker build --build-arg FITZ_TAG=v0.9.32 .

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
# MISMA imagen base que el builder → libpython3.12 garantizado en
# ambos lados. Constraint arquitectural permanente de PyO3 + Linux
# (no es deuda cerrable; ver comentario al inicio del archivo).
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
