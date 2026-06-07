# TaskHub C5 — Cron + background jobs con persistencia

Estado del proyecto al cerrar el cap
[C5](../../../docs/taskhub/c5-cron-jobs-persistencia.md). Sobre
el CRUD + WS del C4 sumamos:

- `@cron("0 0 3 * * *", tz="UTC", store=db_result) async fn cleanup_old_tasks()`
  — nocturno, borra tasks `done` con más de 90 días.
- `@cron("0 0 9 * * *", tz="UTC", store=db_result) async fn daily_due_reminders()`
  — matutino, dispara `spawn(send_due_reminder(...))` por cada
  task con `due_date` próxima.
- `@background async fn send_due_reminder(task_id: Int) -> Null`
  — mock del email envío con `print`.
- `spawn(send_due_reminder(new_task.id))` desde
  `POST /api/projects/{id}/tasks` cuando `due_date != null`.
- `@requires("admin") @get("/jobs")` lee `fitz_cron_runs` con
  `db.query` crudo + typed accessors.
- Tablas `fitz_cron_jobs` + `fitz_cron_runs` auto-creadas al boot
  del scheduler (idempotente con `CREATE TABLE IF NOT EXISTS`).

**El compose sigue con 5 services del C1** — sin Celery, sin
Redis, sin worker separados. Todo el scheduler vive en el
binario.

## Estructura

```text
c5-cron-jobs-persistencia/
├── fitz.toml
├── Dockerfile
├── docker-compose.yml
├── dev-env.sh
├── .env.example
├── .gitignore
├── src/
│   └── main.fitz             # ACTUALIZADO — 2 @cron + @background + spawn + GET /jobs
├── frontend/index.html
├── nginx/nginx.conf
├── prometheus/prometheus.yml
├── otel/
├── migrations/
│   └── 20260607130000_initial_schema.sql   # sin cambios desde C2
├── .github/workflows/ci.yml
└── README.md
```

## Setup (desde cero)

```bash
cp .env.example .env
# Editá DB_PASSWORD + JWT_SECRET.

docker compose up -d --build
source dev-env.sh
fitz db migrate
docker compose up -d --build app
```

Bootstrap admin (mismo patrón del C3):

```bash
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}'

psql "$DATABASE_URL" -c "UPDATE users SET role='admin' WHERE id=1;"
```

## Validación end-to-end

```bash
# A. Verificar tablas del scheduler auto-creadas.
psql "$DATABASE_URL" -c "\dt fitz_*"
# →             List of relations
#  Schema |       Name        | Type
# --------+-------------------+--------
#  public | fitz_cron_jobs    | table
#  public | fitz_cron_runs    | table

# B. Detalle de los jobs registrados.
psql "$DATABASE_URL" -c "SELECT name, schedule, tz FROM fitz_cron_jobs;"
# → name                 | schedule        | tz
#   cleanup_old_tasks    | 0 0 3 * * *     | UTC
#   daily_due_reminders  | 0 0 9 * * *     | UTC

# C. Login admin + crear project + task con due_date.
ADMIN_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}' \
  | jq -r .token)

PID=$(curl -sX POST http://localhost:8000/api/projects \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Sprint Q3"}' | jq -r .id)

curl -X POST "http://localhost:8000/api/projects/$PID/tasks" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Diseñar UI","due_date":"2026-06-08","assignee_id":1}'

# D. Verificar logs del app — debería aparecer el mock email.
docker compose logs app | tail -5
# → [email-mock] enviando a admin@taskhub.local: task 'Diseñar UI' vence pronto (due=2026-06-08)

# E. Endpoint admin lee el audit log del scheduler.
curl http://localhost:8000/api/jobs -H "Authorization: Bearer $ADMIN_TOKEN"
# → [] al principio (los cron no corrieron aún — los schedules son 3am y 9am)
```

### Forzar ejecución de un cron para testing

```bash
# Editá src/main.fitz cambiando temporalmente el schedule de
# cleanup_old_tasks a "*/30 * * * * *" (cada 30 segundos).
# Rebuild:
docker compose up -d --build app

# Esperá 30 segundos, mirá los logs:
docker compose logs app | grep cron
# → [cron] cleanup_old_tasks: borradas 0 tasks

# Verificar en la DB.
psql "$DATABASE_URL" -c "SELECT job_name, status, attempt FROM fitz_cron_runs ORDER BY started_at DESC LIMIT 5;"

# Endpoint admin:
curl http://localhost:8000/api/jobs -H "Authorization: Bearer $ADMIN_TOKEN" | jq .

# RECORDÁ revertir el schedule al production "0 0 3 * * *" antes de commitear.
```

## 3 descubrimientos del checker durante implementación

El cap C5 obligó a aprender 3 restricciones del MVP no obvias:

1. **`@cron` handlers no admiten params**: signature es
   `async fn () -> Result<Null>`. Para acceder a la DB usás
   `db_result` del closure scope con un `match`. `store=db_result`
   es para PERSISTENCIA de metadata de runs, NO para inyectar
   db en el handler.
2. **Strings multi-línea no compilan**: SQL queries largas las
   ponés en una sola línea o las concatenás con `+` (también en
   una línea — concatenación multi-línea tampoco funciona).
3. **Closures multi-línea no compilan**: `fn(t) => predicate`
   debe estar en una sola línea. WHERE complejos van inline con
   `and`/`or`.

## Limpiar

```bash
docker compose down       # mantiene data + fitz_cron_jobs + fitz_cron_runs
docker compose down -v    # resetea TODO
```

## Qué viene

**[Cap C6 — Interop Python: priorización IA con LLM](../../../docs/taskhub/c6-interop-python-llm.md)**.
Módulo Python `priority.py` con LLM (OpenAI gpt-4o-mini opcional)
+ fallback heurística por keywords. Endpoint
`POST /api/tasks/{id}/suggest-priority` con `match Result<Int>`.
Cache en `task.ai_suggested_priority`. **Cambia base Docker** de
distroless a `python:3.12-slim-bookworm` (C7 optimiza con
`--bundle-python`).

## Troubleshooting

Ver la sección "Troubleshooting" del
[cap C5](../../../docs/taskhub/c5-cron-jobs-persistencia.md#troubleshooting).
