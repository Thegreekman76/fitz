#!/usr/bin/env bash
# dev-env.sh — exporta vars necesarias para correr fitz db
# desde el host contra el container db del compose.
#
# Uso: `source dev-env.sh`

if [ ! -f .env ]; then
    echo "ERROR: .env no existe. Copialo desde .env.example primero."
    return 1 2>/dev/null || exit 1
fi

# Carga el .env del compose.
set -a
source .env
set +a

# Construye DATABASE_URL apuntando al db expuesto en localhost.
export DATABASE_URL="postgres://taskhub:${DB_PASSWORD}@localhost:5432/taskhub?sslmode=disable"

echo "✓ DATABASE_URL exportada para fitz db (localhost:5432)"
echo "  Ahora podés correr: fitz db status / diff / migrate / ..."
