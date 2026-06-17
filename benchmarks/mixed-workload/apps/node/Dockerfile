# Dockerfile para apps/node/ del benchmark mixed-workload.
#
# Multi-stage:
#   1. `node:20-slim` + `npm ci --omit=dev` + `prisma generate`
#   2. `node:20-slim` final con código + node_modules + prisma client.
#
# `prisma db push` corre en boot (no en build) porque necesita acceso
# a la DB que solo está disponible runtime via docker-compose network.

# ---- Stage 1: deps + prisma generate -------------------------------
FROM node:20-slim AS builder

# OpenSSL 3 + perl: dependencies del client Prisma generado para Linux.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        openssl \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Capa cacheable separada: instala deps primero (npm ci es deterministic
# desde el package-lock — sin lock generamos uno al primer build).
COPY package.json ./
RUN npm install --omit=dev --no-audit --no-fund

# Copia del schema + generate del client. El generate baja el binario
# `query_engine-linux-X.so.node` que Prisma usa runtime.
COPY prisma/ ./prisma/
RUN npx prisma generate

# ---- Stage 2: runtime ----------------------------------------------
FROM node:20-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        openssl \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/node_modules ./node_modules
COPY --from=builder /app/prisma ./prisma
COPY package.json ./
COPY src/ ./src/

EXPOSE 3000

CMD ["node", "src/index.js"]
