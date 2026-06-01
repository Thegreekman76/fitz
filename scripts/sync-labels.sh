#!/usr/bin/env bash
# Sincroniza las labels de `.github/labels.yml` con el repo en GitHub.
#
# Requiere:
#   - `gh` CLI autenticado (`gh auth status` debe estar OK).
#   - `yq` para parsear el YAML (https://github.com/mikefarah/yq).
#
# Uso:
#   bash scripts/sync-labels.sh                 # solo crea / actualiza
#   bash scripts/sync-labels.sh --delete-orphan # también borra labels que no estén en el YAML
#
# El script es idempotente: corrérlo varias veces no cambia nada si el
# YAML y GitHub ya están sincronizados.

set -euo pipefail

LABELS_FILE=".github/labels.yml"
DELETE_ORPHAN=false

for arg in "$@"; do
  case "$arg" in
    --delete-orphan) DELETE_ORPHAN=true ;;
    *) echo "Argumento desconocido: $arg" >&2; exit 2 ;;
  esac
done

if ! command -v gh >/dev/null 2>&1; then
  echo "✗ gh CLI no está instalado. https://cli.github.com/" >&2
  exit 1
fi

if ! command -v yq >/dev/null 2>&1; then
  echo "✗ yq no está instalado. https://github.com/mikefarah/yq" >&2
  exit 1
fi

if [[ ! -f "$LABELS_FILE" ]]; then
  echo "✗ No encuentro $LABELS_FILE — ¿corrés esto desde la raíz del repo?" >&2
  exit 1
fi

REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner)
echo "→ Sincronizando labels contra $REPO"
echo

# Cantidad de entries en el YAML
COUNT=$(yq '. | length' "$LABELS_FILE")

# Set de labels declaradas (para el diff de orphan)
DECLARED=()

for i in $(seq 0 $((COUNT - 1))); do
  NAME=$(yq ".[$i].name" "$LABELS_FILE")
  COLOR=$(yq ".[$i].color" "$LABELS_FILE")
  DESC=$(yq ".[$i].description" "$LABELS_FILE")
  DECLARED+=("$NAME")

  # ¿La label ya existe?
  if gh label list --limit 200 --json name -q '.[].name' | grep -qx "$NAME"; then
    echo "  ✎ update: $NAME"
    gh label edit "$NAME" --color "$COLOR" --description "$DESC" >/dev/null
  else
    echo "  + create: $NAME"
    gh label create "$NAME" --color "$COLOR" --description "$DESC" >/dev/null
  fi
done

echo
echo "✓ $COUNT labels sincronizadas."

if [[ "$DELETE_ORPHAN" == "true" ]]; then
  echo
  echo "→ Buscando labels en GitHub que NO están en $LABELS_FILE…"
  REMOTE=$(gh label list --limit 200 --json name -q '.[].name')
  for label in $REMOTE; do
    if ! printf '%s\n' "${DECLARED[@]}" | grep -qx "$label"; then
      echo "  - delete: $label"
      gh label delete "$label" --yes >/dev/null
    fi
  done
  echo "✓ Limpieza de orphans completa."
fi
