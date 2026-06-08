# Stage 1 — build con bundling (Path A — ideal, image ~50 MB).
# Asume ghcr.io/thegreekman76/fitz:latest-python con --features python +
# --bundle-python + --bundle-pip funcionando out of the box.
#
# Path B fallback (sin bundling, image ~250 MB): comentá el bloque
# de abajo y descomentá el bloque Path B al final.
FROM ghcr.io/thegreekman76/fitz:latest-python AS builder

WORKDIR /build
COPY fitz.toml .
COPY src/ ./src/
COPY migrations/ ./migrations/
COPY python/ ./python/

# fitz build con bundling: empaqueta CPython 3.14 + openai pip
# package adentro del binario. El binario final es standalone
# (no requiere Python instalado en el destino).
RUN fitz build --bundle-python --bundle-pip openai

# Stage 2 — runtime distroless (vuelve al patrón del C1).
# La image final es ~50 MB: distroless (~22 MB) + binario con
# CPython + openai bundleados (~28 MB).
FROM gcr.io/distroless/cc-debian12 AS runtime

WORKDIR /app
COPY --from=builder /build/target/release/taskhub /app/taskhub

EXPOSE 8080
USER nonroot:nonroot

ENTRYPOINT ["/app/taskhub"]

# ─────────────────────────────────────────────────────────────
# Path B (fallback) — sin bundling, usa python:3.12-slim runtime.
# Descomentá si --bundle-python no funciona en tu setup.
# ─────────────────────────────────────────────────────────────
# FROM python:3.12-slim-bookworm AS runtime
#
# WORKDIR /app
#
# COPY python/requirements.txt /app/python/requirements.txt
# RUN pip install --no-cache-dir -r /app/python/requirements.txt
#
# COPY python/priority.py /app/python/priority.py
# COPY --from=builder /build/target/release/taskhub /app/taskhub
#
# ENV PYTHONPATH=/app/python
# EXPOSE 8080
#
# ENTRYPOINT ["/app/taskhub"]
