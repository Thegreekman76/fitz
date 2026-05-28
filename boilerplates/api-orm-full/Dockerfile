# Dockerfile multi-stage para api-orm-full.
#
# Showcase del stack web first-class entero (HTTP + auth + WS + cron
# + Postgres ORM) en un solo binario standalone. No usa Python, no
# usa SQLAlchemy, no necesita libpython ni pip en el runtime —
# todo el driver Postgres v3.0 wire protocol + ORM declarativo
# viven en el binario `fitz` mismo.
#
# Resultado: imagen final ~30-50 MB (multi-archivo, varios handlers,
# WS, cron). Para tagged release, pin con --build-arg FITZ_TAG.
#
# Build inicial: ~5-8 min (compila fitz desde source).
# Builds subsiguientes: ~30s si solo cambia src/.
#
# Build con tag específico:
#   docker build --build-arg FITZ_TAG=v0.10.7 .

# ---- Stage 1: builder ----------------------------------------------
FROM rust:1.95-slim AS builder

# Herramientas mínimas para compilar fitz desde source. Sin
# python3-dev — no usamos PyO3.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        git ca-certificates pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Clonar Fitz desde el repo público.
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

# Copia del proyecto. Multi-archivo: el codegen carga los módulos
# transitivos via fitz.toml ([bin].main = src/main.fitz).
WORKDIR /app
COPY fitz.toml ./
COPY src/ ./src/

# Compilación al binario nativo. El binario embebe TODO: driver
# Postgres + ORM + axum + tokio + axum WS + auth (JWT/Argon2id) +
# cron scheduler. Sin runtime de Fitz en producción.
RUN /root/.cargo/bin/fitz build src/main.fitz

# ---- Stage 2: runtime ----------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# El binario producido por `fitz build src/main.fitz` queda en
# `src/main` (sin extensión en Linux). Lo renombramos para hacer el
# CMD limpio.
COPY --from=builder /app/src/main /usr/local/bin/app

EXPOSE 3000

CMD ["/usr/local/bin/app"]
