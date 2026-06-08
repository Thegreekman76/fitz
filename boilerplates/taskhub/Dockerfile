# Dockerfile multi-stage real para el boilerplate TaskHub.
#
# **Por qué no usamos `ghcr.io/thegreekman76/fitz:latest-python` como
# builder**: esa imagen es runtime (binario `fitz` + Python, sin Rust
# toolchain). El contrato de la imagen es ejecutar `fitz run` /
# `fitz check`, NO `fitz build` adentro del container. `fitz build`
# necesita cargo + rustc para invocar `cargo build --release` sobre
# el código Rust que el codegen produce.
#
# El pattern correcto es multi-stage real:
#   - Stage builder: python:3.12-trixie + rustup + binario fitz copiado
#     desde :latest-python → corre `fitz build` ahí
#   - Stage runtime: python:3.12-slim-trixie + el binario taskhub
#     producido por el builder + libpython + paquetes pip del interop
#
# Notas técnicas:
#   - Trixie (Debian 13) requerido porque el fitz de :latest-python se
#     linkea contra GLIBC ≥ 2.39. Bookworm (Debian 12) trae GLIBC 2.36
#     y el binario fitz directamente no arranca ("GLIBC_2.39 not found").
#   - Sin --bundle-python / --bundle-pip: el runtime stage trae Python
#     3.12 + openai vía pip install. Imagen final ~280 MB. Si querés
#     una imagen sub-100MB con CPython + openai bundleados al binario,
#     usá `fitz build --bundle-python --bundle-pip openai` (requiere
#     GHCR action de release con bundling completo — al 2026-06 todavía
#     hay gaps documentados en docs/deudas-post-5b.md).
#   - entrypoint.sh corre `fitz db migrate` (subcomando de Fase 10.6)
#     antes del binario taskhub. Sin esto las tablas no existen al
#     primer arranque y la app rompe con `relation "users" does not exist`.
#     El script también espera a Postgres con `pg_isready` para
#     destrabar el race típico "app arranca antes que db".

# ───────────────────────────────────────────────────────────────────
# Stage 1A — extraer binario fitz desde la imagen oficial.
# ───────────────────────────────────────────────────────────────────
FROM ghcr.io/thegreekman76/fitz:latest-python AS fitz_source

# ───────────────────────────────────────────────────────────────────
# Stage 1B — builder con Python 3.12 + Rust toolchain.
# ───────────────────────────────────────────────────────────────────
FROM python:3.12-trixie AS builder

# Rust toolchain via rustup (pinnado a 1.95.0 — misma que el repo fitz).
ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain 1.95.0 --profile minimal

# Build deps (pyo3 puede necesitarlos en algunos casos).
RUN apt-get update && \
    apt-get install -y --no-install-recommends pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Copiar el binario fitz desde la imagen oficial.
COPY --from=fitz_source /usr/local/bin/fitz /usr/local/bin/fitz

WORKDIR /build
COPY fitz.toml .
COPY src/ ./src/
COPY migrations/ ./migrations/
COPY python/ ./python/

# `fitz build` invoca cargo internamente para producir el binario nativo.
RUN fitz build

# ───────────────────────────────────────────────────────────────────
# Stage 2 — runtime con Python 3.12 (necesario por interop dinámico).
# ───────────────────────────────────────────────────────────────────
FROM python:3.12-slim-trixie AS runtime

WORKDIR /app

# Runtime tools:
#   - postgresql-client: pg_isready en entrypoint.sh para esperar al
#     servicio db (el binario fitz tiene su propio cliente Postgres
#     interno, no depende de psql para connect/query/migrate).
#   - curl: healthcheck HTTP contra /healthz desde docker-compose.
#     python:3.12-slim NO trae curl por default (es slim, lo
#     verifiqué empíricamente — comentario previo incorrecto).
RUN apt-get update && \
    apt-get install -y --no-install-recommends postgresql-client curl && \
    rm -rf /var/lib/apt/lists/*

# Python deps del interop (priority.py usa openai cuando OPENAI_API_KEY
# está seteada, fallback a heurística pura sin la key).
COPY python/requirements.txt /app/python/requirements.txt
RUN pip install --no-cache-dir -r /app/python/requirements.txt

# El binario fitz también va en el runtime para correr `fitz db migrate`
# al arranque (ver entrypoint.sh).
COPY --from=fitz_source /usr/local/bin/fitz /usr/local/bin/fitz

# El binario producido por el builder + assets que necesita.
COPY --from=builder /build/target/release/taskhub /app/taskhub
COPY migrations/ /app/migrations/
COPY fitz.toml /app/fitz.toml
COPY python/priority.py /app/python/priority.py

# Entrypoint: corre migrations + arranca el binario.
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

ENV PYTHONPATH=/app/python
EXPOSE 8080

ENTRYPOINT ["/app/entrypoint.sh"]
