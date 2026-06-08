#!/bin/sh
# entrypoint.sh — corre migrations + arranca el binario taskhub.
#
# El boilerplate TaskHub no provee mecanismo para aplicar las
# migrations al boot del binario (el binario fitz produce una app
# HTTP, NO un runner que verifica schema). Por eso este script:
#
# 1. Espera a que Postgres esté ready (pg_isready loop).
# 2. Corre `fitz db migrate` (idempotente — solo aplica las pending).
# 3. exec del binario taskhub (sin shell wrapping para que las señales
#    SIGTERM lleguen directo al proceso del app — graceful shutdown).
#
# Si las migrations fallan, el script aborta con exit code != 0 y
# docker compose marca el container unhealthy.

set -eu

# DATABASE_URL viene del compose, formato:
# postgres://<user>:<pass>@db:5432/<dbname>?sslmode=...
DB_HOST=${DB_HOST:-db}
DB_USER=${DB_USER:-taskhub}
DB_NAME=${DB_NAME:-taskhub}

echo "[entrypoint] esperando a que postgres en ${DB_HOST}:5432 acepte conexiones..."
for i in $(seq 1 30); do
    if pg_isready -h "$DB_HOST" -p 5432 -U "$DB_USER" -d "$DB_NAME" -q; then
        echo "[entrypoint] postgres ready en intento $i"
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "[entrypoint] ERROR — postgres no respondió en 30 intentos, abortando"
        exit 1
    fi
    sleep 1
done

echo "[entrypoint] aplicando migrations con 'fitz db migrate'..."
cd /app
if ! fitz db migrate; then
    echo "[entrypoint] ERROR — fitz db migrate falló, abortando"
    exit 2
fi
echo "[entrypoint] migrations OK"

echo "[entrypoint] arrancando taskhub..."
exec /app/taskhub
