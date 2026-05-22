# Dockerfile multi-stage para el API Fitz del boilerplate.
#
# El frontend HTML+JS vanilla NO se buildea acá — vive en
# `frontend/index.html` y lo sirve nginx alpine en el otro
# container del docker-compose.yml.

# ---- Stage 1: builder ----------------------------------------------
FROM ghcr.io/thegreekman76/fitz:latest AS builder

WORKDIR /app

COPY fitz.toml ./
COPY src/ ./src/

RUN fitz build

# ---- Stage 2: runtime ----------------------------------------------
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /app/target/release/fitz-api-middleware-cors /usr/local/bin/app

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/app"]
