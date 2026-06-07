# TaskHub C6 — Interop Python: priorización IA con LLM

Estado del proyecto al cerrar el cap
[C6](../../../docs/taskhub/c6-interop-python-llm.md). Sobre el
cron + background del C5 sumamos:

- Módulo Python `python/priority.py` con `suggest_priority(title,
  description) -> int` (LLM real con OpenAI si `OPENAI_API_KEY`,
  fallback heurística por keywords si no).
- `from python import priority` al tope de `src/main.fitz`.
- Endpoint `POST /api/tasks/{id}/suggest-priority` (@authenticated
  + scope check del C4) con `match Result<Int>` para fallback de
  emergencia si Python mismo falla.
- Cache del resultado en `task.ai_suggested_priority` (field ya
  declarado desde C2 — sin migration nueva).

**Cambios de infraestructura**:

- `Dockerfile` base runtime: `distroless/cc-debian12` →
  `python:3.12-slim-bookworm` (~150 MB → ~250 MB; C7 optimiza
  con `--bundle-python`).
- `Dockerfile` base builder: `ghcr.io/thegreekman76/fitz:latest`
  → `ghcr.io/thegreekman76/fitz:python` (asumido — workaround si
  no existe abajo).
- `docker-compose.yml` pasa `OPENAI_API_KEY` con default vacío.
- `.env.example` documenta `OPENAI_API_KEY` opcional.
- `.gitignore` suma `.venv/`, `__pycache__/`.
- `requirements.txt` con `openai>=1.0` opcional.

## Estructura

```text
c6-interop-python-llm/
├── fitz.toml
├── Dockerfile                # ACTUALIZADO — base Python
├── docker-compose.yml        # ACTUALIZADO — OPENAI_API_KEY env
├── dev-env.sh
├── .env.example              # ACTUALIZADO — OPENAI_API_KEY opcional
├── .gitignore                # ACTUALIZADO — .venv/, __pycache__/
├── src/
│   └── main.fitz             # ACTUALIZADO — from python import + handler
├── python/                   # NUEVO
│   ├── priority.py
│   └── requirements.txt
├── frontend/
├── nginx/
├── prometheus/
├── otel/
├── migrations/               # sin cambios
├── .github/
└── README.md
```

## Pre-requisitos

1. **Python 3.10+** local (para `fitz run`).
2. **Binario `fitz`** buildeado con `cargo build --release --features
   python` (el default standalone NO incluye libpython link).
3. **Opcional**: API key de OpenAI (`OPENAI_API_KEY` env var).

## Setup local (sin Docker)

```bash
# Crear venv + instalar openai opcional.
python3 -m venv .venv
source .venv/bin/activate
pip install -r python/requirements.txt

# Apuntar PYTHONPATH al dir con priority.py.
export PYTHONPATH=$(pwd)/python

# (Opcional) setear API key.
export OPENAI_API_KEY="sk-..."   # o dejá vacía para heurística

# Correr fitz (con feature python).
fitz run src/main.fitz
```

## Setup Docker

```bash
cp .env.example .env
# Editá DB_PASSWORD + JWT_SECRET. Opcionalmente OPENAI_API_KEY.

docker compose up -d --build
source dev-env.sh
fitz db migrate
docker compose up -d --build app

# Bootstrap admin (igual que C3).
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}'

psql "$DATABASE_URL" -c "UPDATE users SET role='admin' WHERE id=1;"
```

## Validación end-to-end

```bash
ADMIN_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}' \
  | jq -r .token)

# Crear project + task con título "urgente".
PID=$(curl -sX POST http://localhost:8000/api/projects \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Sprint actual"}' | jq -r .id)

TID=$(curl -sX POST "http://localhost:8000/api/projects/$PID/tasks" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"title":"URGENT: server down","description":"prod en llamas"}' \
  | jq -r .id)

# Pedir sugerencia de prioridad.
curl -X POST "http://localhost:8000/api/tasks/$TID/suggest-priority" \
  -H "Authorization: Bearer $ADMIN_TOKEN"
# → {"id":...,"title":"URGENT: ...","ai_suggested_priority":5,...}
# Heurística detectó "urgent" → 5.

# Probar otra task de bajo priority.
TID2=$(curl -sX POST "http://localhost:8000/api/projects/$PID/tasks" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Refactor user model","description":"cleanup técnico"}' \
  | jq -r .id)

curl -X POST "http://localhost:8000/api/tasks/$TID2/suggest-priority" \
  -H "Authorization: Bearer $ADMIN_TOKEN"
# → ai_suggested_priority: 2  (heurística detectó "refactor")
```

## Trade-offs honestos del cap

- **Image size**: ~250 MB vs ~150 MB de C1-C5. El cap C7 muestra
  cómo bajarlo a ~50 MB con `--bundle-python` + base distroless.
- **Builder image `fitz:python`**: la variante puede no existir
  pre-built; workaround documentado es buildear localmente y
  COPY al Dockerfile.
- **Sync Python** en lugar de async: la decisión LLM-vs-heurística
  vive adentro del Python; Fitz solo consume `Result<Int>`.
  Versión async con `?.await` requiere refactorear el módulo
  para `async def` + bridge tokio↔asyncio (Fase 8.6).

## Limpiar

```bash
docker compose down       # mantiene data
docker compose down -v    # resetea TODO
```

## Qué viene

**[Cap C7 — Observability + frontend + deploy production](../../../docs/taskhub/c7-observability-frontend-deploy.md)**.
El cap final del proyecto:

- `@server(prometheus=true)` para `/metrics`.
- Frontend vanilla JS funcional (kanban con drag&drop).
- `/healthz` + `/readyz` + SIGTERM drain.
- `fitz build --bundle-python --bundle-pip openai` → image ~50 MB.
- Publicación final como `boilerplates/taskhub/` para clonar +
  arrancar sin pasar por los 7 caps.

## Troubleshooting

Ver la sección "Troubleshooting" del
[cap C6](../../../docs/taskhub/c6-interop-python-llm.md#troubleshooting).
