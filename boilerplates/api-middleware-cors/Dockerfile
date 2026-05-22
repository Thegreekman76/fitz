# Dockerfile multi-stage para el API Fitz del boilerplate.
#
# El frontend HTML+JS vanilla NO se buildea acá — vive en
# `frontend/index.html` y lo sirve nginx alpine en el otro
# container del docker-compose.yml.
#
# `FITZ_TAG` permite pinear la versión de Fitz (recomendado para
# reproducibilidad). Sin override usa `:latest`, pero requiere
# `docker pull ghcr.io/thegreekman76/fitz:latest` antes del build
# para refrescar la cache local — sin eso Docker reusa el sha que
# tenga en cache, que puede ser viejo. Pinear evita ese trampolín:
#
#   docker compose build --build-arg FITZ_TAG=v0.9.33

# ---- Stage 1: builder ----------------------------------------------
ARG FITZ_TAG=latest
FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder

WORKDIR /app

COPY fitz.toml ./
COPY src/ ./src/

RUN fitz build

# ---- Stage 2: runtime ----------------------------------------------
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /app/target/release/fitz-api-middleware-cors /usr/local/bin/app

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/app"]
