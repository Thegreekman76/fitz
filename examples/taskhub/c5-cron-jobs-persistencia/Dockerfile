# Stage 1 — build con el toolchain de Fitz.
FROM ghcr.io/thegreekman76/fitz:latest AS builder

WORKDIR /build
COPY fitz.toml .
COPY src/ ./src/
COPY migrations/ ./migrations/

RUN fitz build

# Stage 2 — runtime mínimo distroless.
FROM gcr.io/distroless/cc-debian12 AS runtime

WORKDIR /app
COPY --from=builder /build/target/release/taskhub /app/taskhub

EXPOSE 8080
USER nonroot:nonroot

ENTRYPOINT ["/app/taskhub"]
