# C5 — Cron + background jobs con persistencia

**Pre-requisitos**: [C4 — CRUD + relations + WebSocket](c4-crud-relations-ws.md)
cerrado. Tenés projects + tasks creándose end-to-end, scope por
rol, WS broadcast funcionando.

**Objetivo**: agregar **dos cron jobs persistentes** al binario
de TaskHub — `cleanup_old_tasks` nocturno (borra tasks `done`
con más de 90 días) y `daily_due_reminders` matutino (notifica
tasks con `due_date` próxima) — más un **`@background async fn
send_due_reminder(task_id)`** invocable con `spawn(...)` desde
los handlers HTTP cuando una task se crea con `due_date`. Todo
con **`store=db`** para que las runs sobrevivan reinicios del
container. Endpoint `GET /api/jobs` para que admin vea el audit
log de runs leído de la tabla `fitz_cron_runs` auto-creada.

**Por qué importa**: este cap **borra Celery del compose**. Hasta
acá TaskHub tenía 5 services (app + db + prometheus + jaeger +
nginx). En cualquier stack Python+FastAPI equivalente, **sumar
cron jobs implicaría 3 services más**: `celery_worker` + `celery_beat`
+ `redis` (broker). Imagen total saltaría de ~150 MB a ~600 MB,
boot del compose pasaría de 30s a 90s+, y la **arquitectura
operativa** se vuelve significativamente más compleja (escalar
workers, monitorear queues, manejar redis failover, debuggear
tasks fantasma). **Fitz mantiene los 5 services de TaskHub
porque los cron + background jobs viven en el binario** —
diferenciador estructural fuerte.

**Cross-link**: [Cap 30 de la guía — Jobs sin Celery](../guide.md#30-jobs-sin-celery)
+ [`docs/db-orm.md`](../db-orm.md) para el SQL queries.

---

## Mapa del cap

```mermaid
flowchart LR
    A[@cron 0 0 3 * cleanup_old_tasks] --> B[DELETE tasks done >90d]
    C[@cron 0 0 9 * daily_due_reminders] --> D[buscar tasks due_date <= mañana]
    D --> E[spawn send_due_reminder]
    F[POST /tasks con due_date] --> E
    E --> G[@background send_due_reminder mockea email]
    H[store=db] --> I[fitz_cron_jobs tabla auto-creada]
    H --> J[fitz_cron_runs audit log]
    K[GET /api/jobs admin] --> J
```

---

## Por qué Fitz es distinto

| Feature | Python+FastAPI+Celery+Redis | Node+Express+bull+Redis | Spring Boot+Quartz | Rails+Sidekiq+Redis | **Fitz TaskHub** |
|---|---|---|---|---|---|
| Services en compose | app + worker + beat + redis | app + worker + redis | app + db (Quartz en proceso) | app + worker + redis | **app + db** (cron en proceso) |
| Imagen total Docker | ~600 MB | ~500 MB | ~400 MB | ~500 MB | **~150 MB** |
| Broker externo | Redis o RabbitMQ requerido | Redis requerido | DB Quartz (in-process puede) | Redis requerido | **Sin broker** |
| Setup del scheduler | `celery -A app beat` proceso separado | `node scheduler.js` separado | `@Scheduled` annotation | `Sidekiq::Cron` gem | **`@cron("expr")` decorator built-in** |
| Workers | `celery -A app worker -c N` proceso(s) separado(s) | `node worker.js` separado | thread pool dentro del app | `sidekiq` proceso separado | **tokio multi-thread del binario** |
| Persistencia de runs | tabla custom + workflow manual | tabla custom + manual | tablas Quartz autocreadas | Redis (volátil) o gem extra | **`store=db` → `fitz_cron_jobs` + `fitz_cron_runs` auto-creadas** |
| Retry con backoff | `@task(autoretry_for=..., retry_backoff=...)` | `bull.add(..., {attempts, backoff})` | callbacks manuales | `sidekiq_retries` opt | **`retry={max:3, backoff:"exponential", initial_secs:30}`** en el decorator |
| TZ-aware cron | `crontab(timezone=pytz.UTC)` | `node-cron` + manual TZ | `@Scheduled(zone="UTC")` | manual TZ handling | **`tz="UTC"` kwarg del decorator (IANA standard)** |
| Catch up missed runs | manual | manual | manual | manual | **`catch_up=true` opt-in** |
| Cron-only mode | proceso separado | proceso separado | profile spring custom | proceso separado | **`fitz` binary sin handlers HTTP** corre solo scheduler |
| Fire-and-forget desde handler | `.delay(args)` o `apply_async(args)` | `queue.add({...})` | `taskExecutor.submit(...)` | `worker.perform_async(...)` | **`spawn(fn_call)` builtin con `Future<T>` tipado** |

**Diferencial estructural**: los cron + background jobs **son
parte del lenguaje**, no de una lib opcional. El binario producido
por `fitz build` lleva el scheduler tokio adentro, los timers
viven en el mismo proceso que axum (los HTTP handlers), y la
persistencia opcional con `store=db` reusa la conn al Postgres
que el resto del programa ya usa. **Sin broker. Sin workers
separados. Sin gemfile / requirements.txt extra. Cero arquitectura
de runtime que aprender**.

---

## Paso 1 — Cron job de cleanup nocturno

Editás `src/main.fitz`. Sumás al final:

```fitz
// Los @cron handlers NO toman params — acceden a db_result desde
// closure scope. El kwarg `store=db_result` es para PERSISTENCIA
// del audit log de runs, NO para inyectar db en el handler.
@cron("0 0 3 * * *",
      tz="UTC",
      retry={max: 3, backoff: "exponential", initial_secs: 30, max_secs: 300},
      store=db_result)
async fn cleanup_old_tasks() -> Result<Null> {
    let conn: DbConn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }
    // Tasks marcadas done hace más de 90 días.
    let cutoff = DateTime.now().subtract_days(90)
    let deleted = Task.where(fn(t) => t.status == "done" and t.created_at < cutoff)
        .delete(conn)
        .await?

    print("[cron] cleanup_old_tasks: borradas {deleted} tasks")
    return Ok(null)
}
```

**Detalles**:

- **`@cron("0 0 3 * * *")`** — expresión cron de **6 campos**
  (Unix estándar 5 fields + el segundo): segundo / minuto / hora
  / día / mes / día-de-semana. `0 0 3 * * *` = "todos los días a
  las 03:00:00". Fitz normaliza 5-fields al 6-fields agregando
  `0` al segundo si falta.
- **`tz="UTC"`** — IANA name (`UTC`, `America/Argentina/Ushuaia`,
  `Asia/Tokyo`, etc.). El scheduler usa `chrono-tz` interno.
  Default si omitís el kwarg: UTC.
- **`retry={...}`** — si la run falla (excepción / Err), el
  scheduler espera `initial_secs` y reintenta. Backoff:
  `"exponential"` duplica el delay cada intento hasta `max_secs`;
  `"linear"` suma `initial_secs` cada vez; `"constant"` repite
  `initial_secs`. `max` es el número total de intentos antes de
  marcar la run como `failed`. **Útil para flakiness transitorio
  de DB / red**.
- **`store=db_result`** — el binding del top-level
  `let db_result = db.connect(...).await`. **Persistencia**: el
  scheduler crea las tablas `fitz_cron_jobs` y `fitz_cron_runs`
  al boot si no existen (idempotente con `CREATE TABLE IF NOT
  EXISTS`), y persiste cada attempt con shape
  `(job_name, started_at, finished_at, status, attempt, error)`.
  **Las runs sobreviven reinicios del container**. **Importante**:
  `store=db_result` es para PERSISTENCIA DE METADATA DE RUNS, NO
  para inyectar `db` en el handler.
- **`async fn cleanup_old_tasks() -> Result<Null>`** — signature
  exacta: **sin params** (`@cron` handlers no aceptan input). Devuelve
  `Result<Null>`. `Ok(null)` = success, `Err(...)` dispara retry.
  Para usar la DB, accedés a `db_result` del closure scope con
  un `match`.
- **`DateTime.now().subtract_days(90)`** — aritmética built-in.
  El comparison `t.created_at < cutoff` se compila a `WHERE
  "created_at" < $1::timestamptz`.
- **`.delete(conn)`** del ORM requiere un `.where(...)` antes
  (guard obligatorio).

---

## Paso 2 — Cron job de daily reminders

Sumás al final del archivo:

```fitz
@cron("0 0 9 * * *",
      tz="UTC",
      retry={max: 2, backoff: "linear", initial_secs: 60, max_secs: 120},
      store=db_result)
async fn daily_due_reminders() -> Result<Null> {
    let conn: DbConn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }
    // Tasks con due_date entre hoy y mañana, no done. Closure
    // del .where(...) en una sola línea — los closures
    // multi-línea no compilan en MVP.
    let today = Date.today()
    let tomorrow = today.add_days(1)
    let due_soon = Task.where(fn(t) => t.due_date != null and t.due_date >= today and t.due_date <= tomorrow and t.status != "done").all(conn).await?

    print("[cron] daily_due_reminders: {due_soon.len()} tasks con due_date próxima")

    for task in due_soon {
        // Cada task con assignee dispara send_due_reminder en
        // background (sin bloquear el cron run).
        let _ = match task.assignee_id {
            null => 0,
            _    => {
                let _ = spawn(send_due_reminder(task.id))
                1
            },
        }
    }

    return Ok(null)
}
```

**Detalles**:

- **`Date.today()`** + **`.add_days(1)`** — Date built-in del
  lenguaje (M6.C5 del curso). Compila a `date` Postgres,
  comparison nativa contra el column `due_date date`.
- **`t.due_date != null`** en el closure del `.where(...)` — el
  ORM lo traduce a `"due_date" IS NOT NULL` SQL.
- **`for task in due_soon`** — itera la `List<Task>` resultado.
- **Match sobre `task.assignee_id: Int?`** (nullable) con patrón
  `null` + bare ident `_` — el caso "tiene assignee" dispara
  `spawn(send_due_reminder(task.id))`.
- **`spawn(...)` fire-and-forget** — el cron run no espera al
  `send_due_reminder`. Si tarda 5s, el cron run sigue. Por eso
  iteramos N tasks en una sola corrida del cron sin sumar latency.

---

## Paso 3 — `@background async fn send_due_reminder`

```fitz
@background
async fn send_due_reminder(task_id: Int) -> Null {
    let conn: DbConn = match db_result {
        Ok(c) => c,
        Err(_) => return null,
    }

    // Cargar la task + el assignee (lookup separado porque no
    // sumamos @belongs_to a User en C4).
    let task = match Task.where(fn(t) => t.id == task_id).first(conn).await {
        Ok(t)  => t,
        Err(_) => return null,
    }

    let assignee_id = match task.assignee_id {
        null => return null,
        a    => a,
    }

    let user = match User.where(fn(u) => u.id == assignee_id).first(conn).await {
        Ok(u)  => u,
        Err(_) => return null,
    }

    // MVP: mock del email envío con print.
    // Producción real: integrar SendGrid / Postmark / SES / Mailgun.
    print("[email-mock] enviando a {user.email}: task '{task.title}' vence pronto (due={task.due_date})")

    return null
}
```

**Detalles**:

- **`@background`** marca la fn como **fire-and-forget invocable
  por `spawn`**. Sin este decorator, `spawn(...)` rechaza la
  llamada en compile-time (el checker validate que el callee
  está marcado opt-in).
- **`async fn` con `-> Null`** — los background jobs no
  devuelven Result al caller (el caller fue fire-and-forget). Los
  errores los manejás internamente con match.
- **Doble lookup**: task + user. En C4 decidimos no sumar
  `@belongs_to` entre Task y User para mantener el cap simple —
  refinamiento futuro acá podría usar `.preload("assignee")` para
  una sola query.
- **Mock del email con `print`** — en producción real integrás
  un email API (SendGrid, Postmark, Mailgun, SES). Para el MVP
  pedagógico el print es suficiente — se ve en
  `docker compose logs app`.

---

## Paso 4 — `spawn(...)` desde el handler de creación

Modificás el handler `POST /api/projects/{project_id}/tasks` del
C4 para que dispare `send_due_reminder` cuando la task se crea
con `due_date` no-null:

```fitz
@authenticated
@post("/projects/{project_id}/tasks")
async fn create_task(
    project_id: Int,
    input: CreateTaskInput,
    user: User
) -> Result<Task> {
    if (input.title == "") {
        return Err("title no puede estar vacío")
    }
    let conn: DbConn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }
    let project = Project.where(fn(p) => p.id == project_id).first(conn).await?
    if (user.role != "admin" and project.owner_id != user.id) {
        return Err("no podés agregar tasks a este project")
    }
    let new_task = Task.insert(conn, Task {
        id: 0,
        project_id: project_id,
        title: input.title,
        description: input.description,
        status: "todo",
        priority: input.priority,
        assignee_id: input.assignee_id,
        due_date: input.due_date,
        ai_suggested_priority: null,
        created_at: DateTime.now(),
        project: null,
    }).await?

    // NUEVO en C5: si la task tiene due_date, mandar reminder.
    let _ = match new_task.due_date {
        null => 0,
        _    => {
            let _ = spawn(send_due_reminder(new_task.id))
            1
        },
    }

    return Ok(new_task)
}
```

**Detalles**:

- **`spawn(send_due_reminder(new_task.id))`** — el checker valida
  que `send_due_reminder` está marcado `@background` antes de
  permitir el callsite. Si te olvidás del decorator, el checker
  aborta con mensaje claro.
- **Returns `Future<Null>`** — el `let _ = ...` la descarta. El
  reminder corre en background sin afectar la latency del
  response HTTP.
- **Fire-and-forget**: si `send_due_reminder` falla (excepción /
  print falla), **el handler HTTP NO se entera**. Por eso el
  pattern del background fn maneja errores con match interno y
  devuelve `null` silencioso.

---

## Paso 5 — Endpoint `GET /api/jobs` para admin

El scheduler con `store=db` mantiene la tabla `fitz_cron_runs`
con el audit log. Sumamos un endpoint admin que la lee para
debug + observability:

```fitz
type JobRun {
    job_name: Str
    started_at: Str
    finished_at: Str
    status: Str
    attempt: Int
    error: Str
}

@requires("admin")
@get("/jobs")
async fn list_job_runs(user: User) -> Result<List<JobRun>> {
    let conn: DbConn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }

    // db.query crudo porque fitz_cron_runs no la declaramos como
    // @table (la maneja el scheduler internamente). Strings
    // multi-línea NO compilan en MVP — todo en una sola línea.
    let sql = "SELECT job_name, started_at::text AS started_at, COALESCE(finished_at::text, '') AS finished_at, status, attempt, COALESCE(error, '') AS error FROM fitz_cron_runs ORDER BY started_at DESC LIMIT 50"
    let rows = db.query(sql, []).await?

    let runs: List<JobRun> = []
    for r in rows {
        let run = JobRun {
            job_name: r.get_str("job_name")?,
            started_at: r.get_str("started_at")?,
            finished_at: r.get_str("finished_at")?,
            status: r.get_str("status")?,
            attempt: r.get_int("attempt")?,
            error: r.get_str("error")?,
        }
        runs.push(run)
    }
    return Ok(runs)
}
```

**Detalles**:

- **`db.query` crudo** — `fitz_cron_runs` la mantiene el scheduler
  internamente, no declaramos un `@table type` para no acoplar
  a la implementación. El SQL es estable (documentado en
  Fase 9.w.3.iter2 del lenguaje).
- **`COALESCE(finished_at::text, '')`** — `finished_at` es nullable
  (la run en curso aún no terminó); coerce a string vacío para
  que el field `JobRun.finished_at: Str` no rompa el shape.
- **`::text` cast** en el SELECT — convierte timestamptz a text
  ISO 8601 para que `r.get_str(...)` funcione.
- **Typed accessors** `r.get_str(col)?` y `r.get_int(col)?` (la API
  correcta del `DbRow`, documentada en M6.C1 del curso).
- **LIMIT 50** — paginación simple, suficiente para debug.
  Refinable con cursor/offset si crece.

---

## Paso 6 — Rebuild + tests

```bash
docker compose up -d --build app
```

Verificación de las tablas auto-creadas por el scheduler:

```bash
psql "$DATABASE_URL" -c "\dt"
# Debería listar (además de las del C2):
#   - fitz_cron_jobs
#   - fitz_cron_runs

# Detalle de fitz_cron_jobs.
psql "$DATABASE_URL" -c "SELECT name, schedule, tz, last_run_at, last_status FROM fitz_cron_jobs;"
# →    name              | schedule         | tz  | last_run_at | last_status
#    -------------------+-----------------+-----+-------------+-------------
#     cleanup_old_tasks | 0 0 3 * * *     | UTC |             |
#     daily_due_reminders | 0 0 9 * * *   | UTC |             |
# (last_run_at y last_status vacíos hasta la primera corrida)
```

Test del `spawn(...)` desde handler:

```bash
# Login admin (del C3).
ADMIN_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}' \
  | jq -r .token)

# Crear un project (del C4).
PROJECT_ID=$(curl -sX POST http://localhost:8000/api/projects \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Sprint Q3","description":"trabajo Q3"}' \
  | jq -r .id)

# Crear task CON due_date — debería disparar send_due_reminder.
curl -X POST "http://localhost:8000/api/projects/$PROJECT_ID/tasks" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Diseñar UI","due_date":"2026-06-08","assignee_id":1}'

# Mirar logs del app — debería aparecer el mock del email.
docker compose logs app | tail -5
# → [email-mock] enviando a admin@taskhub.local: task 'Diseñar UI' vence pronto (due=2026-06-08)
```

Test del cron (forzando con scheduling cercano para no esperar
hasta las 3am):

```bash
# Editar src/main.fitz: cambiar temporalmente el cron
# de cleanup a "*/30 * * * * *" (cada 30 segundos para testear).
# Rebuild:
docker compose up -d --build app

# Esperar 30s, mirar logs:
docker compose logs app | grep cron
# → [cron] cleanup_old_tasks: borradas 0 tasks

# Verificar run en la DB.
psql "$DATABASE_URL" -c "SELECT job_name, status, attempt FROM fitz_cron_runs ORDER BY started_at DESC LIMIT 5;"

# Endpoint admin lee el audit log.
curl http://localhost:8000/api/jobs -H "Authorization: Bearer $ADMIN_TOKEN"
# → [{"job_name":"cleanup_old_tasks","started_at":"...","finished_at":"...","status":"ok","attempt":1,"error":""}, ...]
```

**Después de testear**, **revertí el schedule** del cron al
production `"0 0 3 * * *"` antes de commitear.

---

## Validación del cap

- [ ] `cleanup_old_tasks` declarado con `@cron(...)` + 4 kwargs.
- [ ] `daily_due_reminders` declarado con kwargs análogos.
- [ ] `send_due_reminder` declarado con `@background` + signature
      `async fn ... -> Null`.
- [ ] `POST /api/projects/{id}/tasks` dispara `spawn(...)` cuando
      `due_date` no es null.
- [ ] `GET /api/jobs` devuelve audit log de runs (admin only).
- [ ] Tablas `fitz_cron_jobs` + `fitz_cron_runs` creadas
      automáticamente al boot del binario.
- [ ] `last_run_at` y `last_status` se actualizan después de
      cada run del cron.
- [ ] Si el cron falla, los retries quedan registrados con
      `attempt > 1` en `fitz_cron_runs`.
- [ ] Mock del email envío aparece en `docker compose logs app`.

---

## Troubleshooting

### `fitz check` aborta con `@cron requiere store=<binding>`

Olvidaste el kwarg `store=db_result`. Sin él, el scheduler NO
persiste runs — corre en memoria y pierde state en cada reinicio.
**Para TaskHub queremos persistencia**, así que `store=db_result`
es obligatorio.

### `Err("relation 'fitz_cron_jobs' does not exist")` al boot

El scheduler intenta crear la tabla con `CREATE TABLE IF NOT
EXISTS` automático, pero el user de DB no tiene permisos. Si
estás contra un Postgres compartido, otorgá CREATE:

```sql
GRANT CREATE ON DATABASE taskhub TO taskhub;
```

(En el compose default de TaskHub, el user `taskhub` es owner del
schema y tiene permisos suficientes.)

### El cron no corre en el horario esperado

Verificá:

1. **TZ correcto**: el cron `0 0 3 * * *` con `tz="UTC"` corre a
   las 03:00 UTC, no a las 03:00 de tu timezone local. Para
   Argentina, usá `tz="America/Argentina/Ushuaia"`.
2. **Container UP**: si el `app` estuvo down a las 03:00, no hay
   catch-up por default (`catch_up=false`). Activá con `catch_up=true`
   en el decorator si querés que corra al boot del container
   siguiente.
3. **`fitz_cron_jobs.next_run_at`**: query la columna para ver
   cuándo es el próximo schedule calculado.

### `spawn(send_due_reminder(task.id))` aborta con
`spawn rechaza callee — falta @background`

El checker valida en compile-time que el target de `spawn` es
una fn marcada `@background`. Sumalo al decorator de la fn.

### Logs no muestran `[email-mock]`

- ¿`due_date` viene null en el body del POST? El spawn solo se
  dispara cuando la task se crea con due_date no-null.
- ¿`assignee_id` es null? `send_due_reminder` hace early return si
  no hay assignee.
- ¿`docker compose logs app` está mostrando solo lines recientes?
  Probá `docker compose logs --tail=50 app`.

### `fitz_cron_runs` crece sin parar

Los registros son **históricos permanentes**. Si tu cron corre
cada segundo (testing local con `*/1 * * * * *`), la tabla crece
~86k rows por día. Para production con runs diarias el crecimiento
es trivial (~365 rows/año). Si necesitás limpieza periódica de
audit log viejo, podés sumar otro cron:

```fitz
@cron("0 0 4 * * *", tz="UTC", store=db_result)
async fn cleanup_job_runs(db: DbConn) -> Result<Null> {
    let _ = db.exec(
        "DELETE FROM fitz_cron_runs WHERE started_at < NOW() - INTERVAL '30 days'",
        []
    ).await?
    return Ok(null)
}
```

---

## Lo que cubriste

- **`@cron("expr", tz=..., retry=..., store=...)`** — decorator
  built-in que registra una fn como job programado, con TZ IANA,
  retry con backoff exponential/linear/constant, y persistencia
  opcional sobre Postgres con tablas `fitz_cron_jobs` +
  `fitz_cron_runs` auto-creadas.
- **`@background async fn ... -> Null`** — marker para fns
  invocables con `spawn(...)`. Validado por el checker en
  compile-time.
- **`spawn(fn_call)`** — fire-and-forget desde un handler HTTP
  (o desde otro cron job). El caller NO espera el resultado.
- **`Date.today()` + `.add_days(N)`** — Date built-in con
  aritmética nativa, compila a comparison contra column `date`
  Postgres.
- **Endpoint admin `GET /api/jobs`** que lee el audit log de runs
  con `db.query` crudo + typed accessors (`r.get_str(...)`,
  `r.get_int(...)`).
- **Persistencia entre reinicios**: las tablas del scheduler
  sobreviven `docker compose down` (sin `-v`). `last_run_at` +
  `last_status` queda en `fitz_cron_jobs`; cada attempt en
  `fitz_cron_runs`.
- **El compose de TaskHub sigue con 5 services**: sin Celery,
  sin Redis, sin worker separados. **Diferenciador estructural
  vs stacks típicos**.

**El binario de TaskHub ahora hace HTTP + WS + cron + background
en un solo proceso**. Cap C6 suma interop Python para
priorización IA con LLM — el último diferencial grande antes del
deploy production.

---

## Próximo cap

**[C6 — Interop Python: priorización IA con LLM](c6-interop-python-llm.md)**.

Vamos a sumar un endpoint `POST /api/tasks/{id}/suggest-priority`
que invoca un **LLM via interop Python** (OpenAI / Anthropic
compatible, o fallback a una heurística local). El resultado se
cachea en la columna `ai_suggested_priority`. Demuestra el
**bridge tokio ↔ asyncio** (Fase 8.6 del lenguaje) con
`<py_call>?.await` y manejo de errores con `Result<T>` cuando la
API key falta o el LLM responde mal.

Mientras tanto, **commiteá este cap**. Tu repo tiene cron real
+ background jobs + spawn + persistencia. El compose sigue con
5 services. **Sin Celery, sin Redis.**
