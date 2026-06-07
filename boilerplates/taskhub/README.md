# TaskHub — boilerplate descargable

**Trello-style colaborativo en vivo** construido con el stack
único de Fitz: HTTP + auth + RBAC + ORM + WS + cron + interop
Python + observability + frontend — **todo en un binario** de
~50 MB. Sin Celery, sin Redis, sin SDK SaaS externo, sin ORM
externo.

Este boilerplate es **el estado final del proyecto
[Construyendo TaskHub](https://github.com/Thegreekman76/fitz/tree/main/docs/taskhub)**
(7 caps pedagógicos), publicado standalone para que cualquiera
lo pueda **clonar + arrancar sin pasar por el material
pedagógico**.

## Stack incluido

| Pieza | Implementación |
|---|---|
| HTTP nativo | `@get/@post/@put/@delete` + OpenAPI auto |
| Auth | `@auth_provider` + `@authenticated` + JWT + Argon2id |
| RBAC | `@requires("admin"\|"owner"\|"member")` apilable (semántica OR) |
| ORM Postgres | `@table` + `@has_many`/`@belongs_to` + `.preload(...)` eager |
| Migrations | `fitz db diff/migrate/rollback` versionado |
| WebSockets | `@authenticated @ws` + `WsConn<TaskEvent>` + broadcast |
| Cron + background | `@cron(store=db_result)` + `@background` + `spawn(...)` |
| Interop Python | `from python import` + LLM priorización (OpenAI opt + fallback heurística) |
| Observability | `/metrics` Prometheus + OTel spans → Jaeger + `@healthz`/`@readyz` |
| Frontend | vanilla JS (sin frameworks, sin build) + drag&drop kanban + WS live |
| Deploy | Dockerfile multi-stage + bundling CPython → distroless ~50 MB |

## Quickstart

```bash
# 1. Clonalo (asumiendo repo público).
git clone https://github.com/Thegreekman76/fitz
cd fitz/boilerplates/taskhub

# 2. Generá secrets.
cp .env.example .env
# Editá .env y reemplazá los `cambiamelocal`:
#   DB_PASSWORD=...           (mínimo 16 chars)
#   JWT_SECRET=...            (mínimo 32 chars random — openssl rand -hex 32)
#   OPENAI_API_KEY=           (opcional, vacío = heurística pura Python)

# 3. Arrancá los 5 services.
docker compose up -d --build

# 4. Aplicá migrations.
source dev-env.sh
fitz db migrate

# 5. Bootstrap del primer admin.
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}'

psql "$DATABASE_URL" -c "UPDATE users SET role='admin' WHERE id=1;"

# 6. Abrí el frontend.
open http://localhost:8000
# Login con admin@taskhub.local / adminpass123
# → lista de projects → board kanban → drag&drop con WS live updates
```

**~30 segundos** desde clone hasta TaskHub corriendo.

## Compose con 5 services

```text
                    docker compose ps
┌───────────────────┬──────────────────┬───────────────┐
│ NAME              │ STATUS           │ PORTS         │
├───────────────────┼──────────────────┼───────────────┤
│ taskhub-app       │ Up (healthy)     │ 8080/tcp      │
│ taskhub-db        │ Up (healthy)     │ 5432:5432     │
│ taskhub-prometheus│ Up               │ 9090:9090     │
│ taskhub-jaeger    │ Up               │ 16686:16686   │
│ taskhub-nginx     │ Up               │ 8000:80       │
└───────────────────┴──────────────────┴───────────────┘
```

**Endpoints accesibles**:

- `http://localhost:8000` — **frontend vanilla JS**.
- `http://localhost:8000/api/...` — REST API (nginx proxy).
- `http://localhost:8000/ws/events` — WebSocket (nginx proxy).
- `http://localhost:8000/healthz` — health check (true si DB OK).
- `http://localhost:8000/readyz` — readiness (drain auto en SIGTERM).
- `http://localhost:8000/metrics` — Prometheus exposition.
- `http://localhost:8000/docs` — OpenAPI Scalar UI (auto-gen).
- `http://localhost:8000/asyncapi` — AsyncAPI WS docs (auto-gen).
- `http://localhost:9090` — **Prometheus UI** (target taskhub UP).
- `http://localhost:16686` — **Jaeger UI** (spans HTTP + cron).

## Modelo de datos

```text
User           Project          Task                  Comment
─────          ─────────        ──────                ──────
id PK          id PK            id PK                 id PK
email UNIQUE   name             project_id FK → P     task_id FK → T
password_hash  description      title                 user_id FK → U
role           owner_id FK → U  description           body
created_at     created_at       status (todo/         created_at
                                       doing/done)
                                priority (1-5)
                                assignee_id FK → U?
                                due_date date?
                                ai_suggested_priority?
                                created_at
```

FK constraints `ON DELETE CASCADE` (excepto `assignee_id` que es
`SET NULL`). Indexes en todas las FK.

## Roles (RBAC)

- **`admin`** — bypass total. Ve todos los users (`/api/users`),
  promueve roles (`/api/users/{id}/promote`), ve stats globales
  (`/api/stats`), administra cualquier project.
- **`owner`** — administra sus propios projects (las que creó).
  Ve stats globales junto con admin (apilable).
- **`member`** — default al registrar. Ve sus propios projects.
  Pueden ser assignees de tasks; en ese caso pueden modificar el
  status de las tasks asignadas.

**Promote manual de roles** (desde admin):

```bash
ADMIN_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}' \
  | jq -r .token)

curl -X POST http://localhost:8000/api/users/2/promote \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"new_role":"owner"}'
```

## Cron jobs incluidos

- **`cleanup_old_tasks`** (`0 0 3 * * *` UTC) — borra tasks `done`
  con más de 90 días.
- **`daily_due_reminders`** (`0 0 9 * * *` UTC) — busca tasks
  con `due_date` próxima y dispara `spawn(send_due_reminder(task.id))`
  por cada assignee.
- **`send_due_reminder`** (background) — mock del envío de email
  con `print`. Integrá SendGrid/Postmark/SES acá en producción.

**Audit log** en `fitz_cron_runs` accesible desde
`GET /api/jobs` (admin only).

## Interop Python para IA

El endpoint `POST /api/tasks/{id}/suggest-priority` invoca
`python/priority.py` que decide internamente:

- Si `OPENAI_API_KEY` está set → GPT-4o-mini sugiere priority 1-5.
- Si no → heurística por keywords (urgent/asap → 5,
  bug/fix → 4, refactor/cleanup → 2, default → 3).

El resultado se cachea en `task.ai_suggested_priority`.

## Extender el boilerplate

Ideas naturales:

- **Cambiar el dominio**: `Task` → `Appointment`, `Event`,
  `Expense`, lo que sea.
- **Sumar más roles**: enterprise tier con scopes finos por
  resource type.
- **Cambiar el LLM**: en lugar de OpenAI, integrá Anthropic
  Claude o Ollama local.
- **Sumar email real**: reemplazá el `print` del
  `send_due_reminder` con SendGrid/Postmark/SES integration.
- **Sumar billing**: Stripe integration via interop Python
  (`from python import stripe_client`).

## Cómo entender cada pieza

Para entender **por qué cada decisión está como está** y **cómo
modificarla**, leé el material pedagógico
[Construyendo TaskHub](https://github.com/Thegreekman76/fitz/tree/main/docs/taskhub):

- **C1** — Setup Docker-first (los 5 services del compose).
- **C2** — Schema + workflow `fitz db`.
- **C3** — Auth con RBAC custom (3 roles apilables).
- **C4** — CRUD + relations + WebSocket.
- **C5** — Cron + background jobs.
- **C6** — Interop Python con LLM.
- **C7** — Observability + frontend + deploy production.

## Limitaciones honestas

- **`@ws` no acepta path params** (MVP del lenguaje): el canal
  es global `/ws/events` con filtrado client-side por
  `project_id`. Documentado en cap C4.
- **HTTP handlers no triggerean broadcasts WS** (MVP del
  lenguaje): cliente que muta por HTTP también emite frame WS.
  Documentado en cap C4.
- **Bundling `fitz build --bundle-python --bundle-pip`**: si la
  variante del toolchain con feature `python` no está pre-built
  para tu arquitectura, fallback a Path B del Dockerfile (image
  ~250 MB con `python:3.12-slim-bookworm`).
- **Sync Python LLM call**: la versión async con
  `<py_call>?.await` requiere refactorear `priority.py` a
  `async def`. Documentado en C6.

## Comparativa vs stack típico

| Métrica | FastAPI+Celery+Redis | Express+bull+Redis | Spring Boot | **TaskHub** |
|---|---|---|---|---|
| Services en compose | 6-7 | 5 | 3 | **5** |
| Image total compose | ~800 MB | ~700 MB | ~600 MB | **~330 MB** |
| Boot full stack | 60-90s | 30-60s | 60-120s | **20-30s** |
| Deps en el binario | 20-40 pip pkgs | 100+ npm pkgs | 30-80 jars | **0** |
| OpenAPI auto | ✅ | extras | extras | ✅ |
| AsyncAPI WS auto | ❌ | ❌ | ❌ | ✅ |
| Migrations | Alembic | typeorm-cli | Flyway | ✅ `fitz db` |
| Auth + RBAC | extras × 2 | extras × 2 | extras | ✅ |
| Cron sin broker | ❌ | ❌ | parcial | ✅ |
| Interop Python | nativo | ❌ | ❌ | ✅ |

## Licencia

MIT — usalo, modificalo, distribuilo. Si construís algo
interesante, contanos en
[github.com/Thegreekman76/fitz/issues](https://github.com/Thegreekman76/fitz/issues).
