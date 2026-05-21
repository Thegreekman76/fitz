# Dockerfile multi-stage para el boilerplate `api-simple`.
#
# Stage 1 (builder): imagen oficial de Fitz desde GHCR con `fitz`
# + Rust toolchain pre-instalados. Compila el `.fitz` a binario
# nativo Linux x86_64 con axum/tokio linkeados estático.
#
# Stage 2 (runtime): distroless/cc-debian12. Solo glibc + libgcc +
# ca-certificates. Sin shell, sin package manager. ~22 MB base +
# ~9 MB binario = ~31 MB total. Superficie mínima de ataque para
# un servicio HTTP en producción.
#
# Build:
#   docker build -t fitz-api-simple .
#
# Correr:
#   docker run --rm -p 3000:3000 fitz-api-simple
#
# Probar (en otra terminal):
#   curl localhost:3000/items
#   open http://localhost:3000/docs   # UI Scalar interactiva

# ---- Stage 1: builder ----------------------------------------------
FROM ghcr.io/thegreekman76/fitz:latest AS builder

WORKDIR /app

COPY fitz.toml ./
COPY src/ ./src/

# Compila a binario nativo. El binario sale en
# `target/release/<package.name>` = `target/release/fitz-api-simple`.
RUN fitz build

# ---- Stage 2: runtime ----------------------------------------------
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /app/target/release/fitz-api-simple /usr/local/bin/app

# El `.fitz` declara `@server(3000, "0.0.0.0")` para bindear a
# todas las interfaces — necesario adentro del container para que
# `-p 3000:3000` desde el host route requests al server.
EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/app"]
