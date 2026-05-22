# Dockerfile multi-stage para el boilerplate `cli-tool`.
#
# Stage 1 (builder): usa la imagen oficial de Fitz publicada en GHCR
# por el workflow `release.yml`. Trae `fitz` + toolchain Rust (cargo,
# rustc) listos. Compila el `.fitz` a binario nativo Linux x86_64.
#
# Stage 2 (runtime): `gcr.io/distroless/cc-debian12` — la base más
# chica que sigue trayendo glibc + libgcc. ~22 MB compresso, el
# binario `fitz-report-generator` adentro pesa ~5 MB. Sin shell, sin
# package manager, sin nada que no necesite el binario. Superficie
# mínima de ataque.
#
# Build local:
#   docker build -t fitz-cli-tool .
#
# Build pineando versión de Fitz (recomendado para reproducibilidad):
#   docker build --build-arg FITZ_TAG=v0.9.33 -t fitz-cli-tool .
#
# Correr:
#   docker run --rm fitz-cli-tool
#
# El binario corre, imprime el report, y sale. Modo CLI puro — no
# expone puertos ni queda en background.

# ---- Stage 1: builder ----------------------------------------------
ARG FITZ_TAG=latest
FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder

WORKDIR /app

# Copiamos solo lo necesario para el build. `fitz.toml` + `src/` —
# el resto del repo no hace falta.
COPY fitz.toml ./
COPY src/ ./src/

# Compila a binario nativo. `fitz build` sin args lee `fitz.toml`
# y usa `[bin].main` como entry. El binario sale en
# `target/release/<package.name>` = `target/release/fitz-report-generator`.
RUN fitz build

# ---- Stage 2: runtime ----------------------------------------------
FROM gcr.io/distroless/cc-debian12

# Copiamos solo el binario. Nada más.
COPY --from=builder /app/target/release/fitz-report-generator /usr/local/bin/app

# Modo CLI: el container ejecuta `app` y sale. No persiste estado,
# no expone puertos, no escucha nada.
ENTRYPOINT ["/usr/local/bin/app"]
