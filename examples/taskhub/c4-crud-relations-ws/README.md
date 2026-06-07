# TaskHub C4 — CRUD + relations + WebSocket en vivo por project

Estado del proyecto al cerrar el cap
[C4](../../../docs/taskhub/c4-crud-relations-ws.md). Sobre el
setup auth + RBAC del C3 sumamos:

- `@has_many("Task", via="project_id", on_delete="cascade")` en
  `Project` + companion `tasks: List<Task>`.
- `@belongs_to("Project", on_delete="cascade")` en `Task.project_id`
  + companion `project: Project?`.
- CRUD HTTP: POST/GET `/api/projects`, GET `/api/projects/{id}`
  con `.preload("tasks")`, POST `/api/projects/{id}/tasks`,
  PUT `/api/tasks/{id}` con scoping owner/admin/assignee.
- `@ws("/ws/events")` canal global con `WsConn<TaskEvent>` +
  `conn.broadcast` simétrico + heartbeat 30s. **Filtrado
  client-side por `project_id`** del frame (limitación del MVP
  documentada en el cap).

**Sin cambios al schema** vs C2 — las FK constraints ya están
con `ON DELETE CASCADE` en la migration `initial_schema.sql`.
Las relations declarativas son código-only (codegen del ORM).

## Estructura

```text
c4-crud-relations-ws/
├── fitz.toml
├── Dockerfile
├── docker-compose.yml
├── dev-env.sh
├── .env.example
├── .gitignore
├── src/
│   └── main.fitz             # ACTUALIZADO — relations + CRUD + WS
├── frontend/index.html
├── nginx/nginx.conf
├── prometheus/prometheus.yml
├── otel/
├── migrations/
│   └── 20260607130000_initial_schema.sql   # sin cambios
├── .github/workflows/ci.yml
└── README.md
```

## Setup (desde cero)

```bash
# 1. Generás secrets.
cp .env.example .env
# Editá .env (DB_PASSWORD + JWT_SECRET).

# 2. Arrancás los 5 services.
docker compose up -d --build

# 3. Migrations.
source dev-env.sh
fitz db migrate

# 4. Rebuild para que tome el código del C4.
docker compose up -d --build app

# 5. Bootstrap del primer admin (igual que C3).
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}'

psql "$DATABASE_URL" -c "UPDATE users SET role='admin' WHERE id=1;"
```

## Validación end-to-end (curl + wscat)

```bash
# A. Verificar version del binario.
curl http://localhost:8000/healthz
# → {"status":"ok","version":"0.1.0-c4"}

# B. Login admin + Bob (member).
ADMIN_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}' \
  | jq -r .token)

curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"bob@taskhub.local","password":"bobpass123"}'

BOB_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"bob@taskhub.local","password":"bobpass123"}' \
  | jq -r .token)

# C. Bob crea un project.
curl -X POST http://localhost:8000/api/projects \
  -H "Authorization: Bearer $BOB_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Lanzamiento Q3","description":"plan Q3 2026"}'
# → {"id":1,"name":"Lanzamiento Q3",...,"owner_id":2,"tasks":[]}

# D. Bob crea una task.
curl -X POST http://localhost:8000/api/projects/1/tasks \
  -H "Authorization: Bearer $BOB_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Diseñar mockups","priority":4}'

# E. Bob lista sus projects (scope por owner).
curl http://localhost:8000/api/projects -H "Authorization: Bearer $BOB_TOKEN"

# F. Admin ve todos.
curl http://localhost:8000/api/projects -H "Authorization: Bearer $ADMIN_TOKEN"

# G. GET con eager load — tasks[] poblado.
curl http://localhost:8000/api/projects/1 -H "Authorization: Bearer $BOB_TOKEN"

# H. Update status.
curl -X PUT http://localhost:8000/api/tasks/1 \
  -H "Authorization: Bearer $BOB_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"status":"doing"}'

# I. Carol (otro user) no puede ver el project de Bob.
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"carol@taskhub.local","password":"carolpass123"}'

CAROL_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"carol@taskhub.local","password":"carolpass123"}' \
  | jq -r .token)

curl -i http://localhost:8000/api/projects/1 -H "Authorization: Bearer $CAROL_TOKEN"
# → 500 con {"error":"no podés ver este project"}
```

## Validación WebSocket

```bash
# Terminal 1 — Bob se conecta al canal global.
wscat -c "ws://localhost:8000/ws/events" -s "bearer.$BOB_TOKEN"
# < {"kind":"connected","task_id":0,"project_id":0,"status":"","user_email":"system"}

# Terminal 2 — Admin se conecta al mismo canal.
wscat -c "ws://localhost:8000/ws/events" -s "bearer.$ADMIN_TOKEN"
# < {"kind":"connected","task_id":0,"project_id":0,"status":"","user_email":"system"}

# Terminal 1 emite un evento del project 1:
> {"kind":"updated","task_id":1,"project_id":1,"status":"doing","user_email":""}

# Ambas terminales reciben (user_email reescrito server-side):
# < {"kind":"updated","task_id":1,"project_id":1,"status":"doing","user_email":"bob@taskhub.local"}
```

## Limitación del MVP (importante)

`conn.broadcast(...)` **solo funciona desde un handler `@ws`**.
El patrón canónico hoy es: cliente hace HTTP mutation +
**también** emite un frame WS para informar a otros. Ver el
[cap C4 § Paso 9](../../../docs/taskhub/c4-crud-relations-ws.md#paso-9-patrón-canónico-del-cliente)
para el detalle.

## Limpiar

```bash
docker compose down       # mantiene data
docker compose down -v    # resetea schema + users + projects
```

## Qué viene

**Cap C5 — Cron + background jobs con persistencia**
(próximamente — en desarrollo). Sumamos un `@cron("0 0 3 * * *")`
nocturno con `store=db` que limpia tasks `done` viejas + envía
recordatorios de tasks con due_date próxima. **Sin Celery, sin
Redis** — `@cron` + `@background` viven en el binario.

## Troubleshooting

Ver la sección "Troubleshooting" del
[cap C4](../../../docs/taskhub/c4-crud-relations-ws.md#troubleshooting).
