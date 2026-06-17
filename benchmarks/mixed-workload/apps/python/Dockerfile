# Dockerfile multi-stage para apps/python/ del benchmark mixed-workload.
#
# Paralelo bit-a-bit al Dockerfile de `boilerplates/api-postgres-python/`
# pero apunta al dominio users+posts del bench.
#
# Usa `fitz run --features python` (interop con CPython al runtime).
# Patrón distinto a los otros boilerplates Fitz puros: mantenemos
# Python en el runtime (libpython3.12 + pip install sqlalchemy +
# psycopg2).

# ---- Stage 1: builder (solo trae el binario fitz) ------------------
ARG FITZ_TAG=latest-python
FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder

# ---- Stage 2: runtime ----------------------------------------------
FROM python:3.12-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        libpq5 ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/fitz /usr/local/bin/fitz

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY fitz.toml ./
COPY src/ ./src/
COPY python/ ./python/

# `from python import db` busca módulos via sys.path. PYTHONPATH
# extiende el lookup a `python/` donde viven db.py y models.py.
ENV PYTHONPATH=/app/python

EXPOSE 3000

CMD ["fitz", "run", "src/main.fitz"]
