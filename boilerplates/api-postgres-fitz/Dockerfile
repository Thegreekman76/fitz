# Dockerfile multi-stage para api-postgres-fitz.
#
# A diferencia de `api-postgres-python`, este boilerplate NO usa
# Python, NO usa SQLAlchemy, NO necesita `pip install` ni libpython
# en el runtime. El ORM nativo de Fitz (cap 31 de la guía) habla
# wire protocol Postgres v3.0 directo desde el binario standalone.
#
# Resultado: imagen final ~30-50 MB (vs ~250 MB de api-postgres-python).
#
# Build inicial: ~5-8 min (compila fitz desde source SIN feature
# python). Builds subsiguientes: ~30s si solo cambia src/.
#
# Build con tag específico:
#   docker build --build-arg FITZ_TAG=v0.10.6 .

# ---- Stage 1: builder ----------------------------------------------
FROM rust:1.95-slim AS builder

# Herramientas mínimas para compilar fitz desde source. Sin
# python3-dev — no usamos PyO3 en este boilerplate.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        git ca-certificates pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Clonar Fitz desde el repo público.
# Pinear con --build-arg FITZ_TAG=v0.10.X para reproducibilidad.
ARG FITZ_BRANCH=
ARG FITZ_TAG=
ARG FITZ_REV=

RUN if [ -n "$FITZ_REV" ]; then \
        cargo install --git https://github.com/Thegreekman76/fitz --rev "$FITZ_REV" --bin fitz --locked; \
    elif [ -n "$FITZ_TAG" ]; then \
        cargo install --git https://github.com/Thegreekman76/fitz --tag "$FITZ_TAG" --bin fitz --locked; \
    elif [ -n "$FITZ_BRANCH" ]; then \
        cargo install --git https://github.com/Thegreekman76/fitz --branch "$FITZ_BRANCH" --bin fitz --locked; \
    else \
        cargo install --git https://github.com/Thegreekman76/fitz --bin fitz --locked; \
    fi

# Copia del proyecto y compilación a binario nativo. El binario
# resultante embebe el driver Postgres + ORM, no necesita nada más.
WORKDIR /app
COPY fitz.toml ./
COPY src/ ./src/

RUN /root/.cargo/bin/fitz build src/main.fitz

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
