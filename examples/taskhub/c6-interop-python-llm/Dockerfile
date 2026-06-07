# Stage 1 — build con el toolchain de Fitz (asumiendo variante
# con --features python; si no, build local y copiar binario).
FROM ghcr.io/thegreekman76/fitz:python AS builder

WORKDIR /build
COPY fitz.toml .
COPY src/ ./src/
COPY migrations/ ./migrations/

RUN fitz build

# Stage 2 — runtime con Python (necesario para linkear libpython).
# Cambio importante vs C1-C5: NO distroless. Image total ~250 MB
# vs ~150 MB de C1-C5. C7 muestra cómo bajarlo con --bundle-python.
FROM python:3.12-slim-bookworm AS runtime

WORKDIR /app

# Instalar pip deps (openai opcional, solo necesario si la API key
# está set en runtime).
COPY python/requirements.txt /app/python/requirements.txt
RUN pip install --no-cache-dir -r /app/python/requirements.txt

# Copiar el módulo Python.
COPY python/priority.py /app/python/priority.py

# Copiar el binario Fitz.
COPY --from=builder /build/target/release/taskhub /app/taskhub

# Fitz busca módulos Python en sys.path. Agregamos /app/python.
ENV PYTHONPATH=/app/python

EXPOSE 8080

ENTRYPOINT ["/app/taskhub"]
