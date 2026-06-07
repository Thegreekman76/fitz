# M5.C4 — Jobs sin Celery: `@cron`, `@background`, `spawn` y persistencia

**Pre-requisitos**: [M5.C1 — async](c1-async-await.md), [M5.C3 —
WS](c3-websockets.md). Vas a usar `async fn` para handlers de
cron con I/O, y vas a entender por qué el runtime tokio puede
correr cron + handlers HTTP + WS en simultáneo sin
configuración. Para la sección iter2 (persistencia), también es
útil tener el [stack de DB documentado en DB y ORM](../../db-orm.md)
visto, aunque acá vas a aprender el mínimo necesario.

**Objetivo**: hacer tareas programadas + fire-and-forget **sin
broker externo**, **sin Celery, sin Redis, sin systemd timers**,
sin pip install ni cargo add. Tres piezas nativas: `@cron("expr")`
para periódicos, `@background` para autorizar callsites,
`spawn(fn_call)` para fire-and-forget desde un handler. Y la
**iter2** (v0.11.2): `tz`, `retry`, `catch_up`, `store=db` para
endurecerlo para producción.

**Por qué importa**: el patrón "API + jobs" es universal. En
Python usás Celery + Redis + worker pool + beat. En Node, Bull +
Redis + cron node. En Java, Spring `@Scheduled` + Quartz +
ExecutorService. En Go, `robfig/cron` + goroutines + manual.
Todos comparten **3 problemas**: un broker externo, un proceso
worker aparte, y configuración de retry/timezone/persistencia
hecha a mano y bug-prone.

Fitz lo trae en el lenguaje. Cron expressions con tz IANA,
retry con backoff exponencial, persistencia opt-in sobre
Postgres, catch_up policy, todo built-in. **Ningún otro lenguaje
hoy combina cron + background workers + spawn tipado en el core
sin broker y con paridad intérprete↔binario**.

**Cross-link**: [Guía cap 30 — Jobs sin Celery](../../guide.md#30-jobs-sin-celery).

---

## Mapa del cap

```mermaid
flowchart LR
    A["@cron expr"] --> B[scheduler tokio]
    B --> C[fn corre cada tick]
    D["@background fn"] --> E[autorizada para spawn]
    E --> F[spawn fn_call desde handler]
    F --> G[Future detached]
    H["iter2 store=db"] --> I[persiste runs en fitz_cron_runs]
    J[iter2 retry] --> K[backoff exponential lineal const]
    L[iter2 tz IANA] --> M[interpretado en huso del usuario]
    N["iter2 catch_up"] --> O[un run al boot si missed]
```

---

## Por qué Fitz es distinto

| Feature | Python (Celery) | Node (Bull) | Java (Spring `@Scheduled`) | Go (`robfig/cron`) | **Fitz** |
|---|---|---|---|---|---|
| Setup mínimo | Celery + Redis + worker + beat | Bull + Redis + cron | Quartz config + `@EnableScheduling` | import + goroutine | **`@cron`/`@background`** |
| Broker externo requerido | ✅ Redis/RabbitMQ | ✅ Redis | ❌ in-memory | ❌ in-memory | ❌ **in-memory** |
| Spawn tipado | ❌ string task names | ⚠ TS opcional | ❌ reflection | ❌ closures sin marca | ✅ **`@background` + checker** |
| Validación estática del shape | ❌ runtime | ❌ runtime | ❌ reflection | N/A | ✅ **checker** |
| Cron syntax con seconds | ⚠ Celery beat schedule | ✅ con plugin | ⚠ Quartz cron 7 fields | ✅ con flag | ✅ **5/6/7 fields auto** |
| Timezone IANA | ✅ con setup `celery_beat_schedule` | ⚠ con node-schedule | ✅ con setup | ⚠ con `time.LoadLocation` | ✅ **`tz="IANA/Name"` kwarg** |
| Retry con backoff | ✅ con setup `autoretry_for` | ✅ Bull retry config | ⚠ con Spring Retry lib | ❌ manual | ✅ **`retry={...}` kwarg** |
| Persistencia + visibility | ✅ Flower UI + DB | ✅ Bull UI + Redis | ⚠ con setup | ❌ | ✅ **`store=db` + Postgres** |
| Sin pip install ni cargo add | ❌ | ❌ | ❌ | ❌ deps | ✅ **0 deps** |
| Compila a binario standalone | ❌ | ❌ | ✅ jar | ✅ | ✅ **`fitz build`** |

El diferencial mayor: **persistencia opt-in con un kwarg**.
Levantás Postgres (ya lo tenés para tu API), agregás `store=db`
al `@cron`, y al boot del binario las tablas `fitz_cron_jobs` /
`fitz_cron_runs` se crean automáticas. Inspeccionás con `psql`
sin más infra.

---

## Paso 1 — `@cron("expr")`: tareas periódicas

```fitz
@cron("0 3 * * *")          // todos los días a las 3 AM (5 fields)
fn cleanup() -> Null {
    print("[cron] cleanup nocturno")
    return null
}

@cron("*/5 * * * * *")      // cada 5 segundos (6 fields con seconds)
async fn heartbeat() -> Null {
    return null
}
```

**Sintaxis aceptada** del cron expression:

| Fields | Layout | Ejemplo |
|---|---|---|
| 5 | `min hora día mes día-semana` | `"0 3 * * *"` (3 AM cada día) |
| 6 | `sec min hora día mes día-semana` | `"*/5 * * * * *"` (cada 5 seg) |
| 7 | `sec min hora día mes día-semana año` | `"0 0 0 1 1 * 2027"` |

El runtime normaliza 5→6 automático (prepende `"0 "` como
segundo). 7 fields es para casos raros donde querés agendar
algo en un año específico.

**Lo que valida el checker estáticamente**:

- 1 arg posicional `Str` (el cron expression).
- Sin params en la fn — los jobs no reciben input.
- Return `Null`, `Result<Null>` o `Future<Null>` — el runtime
  descarta el valor; usá `Result` si querés loguear errores.
- No combinable con `@get`/`@post`/`@put`/`@delete`/`@ws`/
  `@background`/`@auth_provider`/`@test`.

Smoke MVP — corré el archivo y dejalo unos segundos:

```bash
$ cat tick.fitz
@cron("*/2 * * * * *")
fn tick() -> Null {
    print("[cron] tick")
    return null
}
print("scheduler arrancado")

$ fitz run tick.fitz
scheduler arrancado
🕐 Fitz scheduler arrancado con 1 job(s) cron
   @cron  tick (*/2 * * * * *)
[cron] tick
[cron] tick
[cron] tick
^C
```

**Cron-only mode**: si el programa solo tiene `@cron` (sin
`@server`, sin `@ws`, sin handlers HTTP), el main queda vivo
bloqueante hasta `Ctrl+C` — modo systemd-friendly. Compilás con
`fitz build`, lo ponés como `ExecStart=/usr/local/bin/cleanup`
en un service unit, y listo: cero scripts extra, cero intérpretes
embebidos.

---

## Paso 2 — `@background` y `spawn(...)`: fire-and-forget

A veces querés disparar trabajo desde un handler HTTP sin que el
handler espere al resultado. Caso típico: registrar analytics
de un click, mandar un email, invalidar cache. El cliente no
necesita esperar a eso para recibir su response.

Fitz lo trae con dos piezas coordinadas:

```fitz
// 1. @background MARCA la fn como autorizada para spawn.
//    Es opt-in del autor — sin esto, el checker rechaza el spawn.
@background
async fn track_click(slug: Str) -> Null {
    let _ = sleep(100).await       // simula I/O lento
    print("[bg] click registrado: /{slug}")
    return null
}

@get("/go/{slug}")
fn redirect(slug: Str) -> Str {
    // 2. spawn(fn_call) lanza la task fire-and-forget.
    //    Devuelve Future<T>; descartado con `let _` = task detached.
    let _ = spawn(track_click(slug))
    return "→ url-del-slug"
}
```

**Lo que pasa**:

1. Cliente llama `GET /go/fitz`.
2. El handler ejecuta `spawn(track_click("fitz"))` y obtiene un
   `Future<Null>`.
3. Lo descarta con `let _ = ...` → la task queda **detached**
   ejecutándose en otro task tokio.
4. El handler retorna `"→ url-del-slug"` **inmediato**, sin
   esperar el sleep + print.
5. Después de 100ms, el background imprime el log.

**Lo que valida el checker estáticamente**:

- `spawn(target_call)` exige que `target` esté declarada con
  `@background`.
- Si no, error claro:

```fitz
async fn enviar(to: Str) -> Null { ... }   // sin @background

@get("/x")
fn x() -> Str {
    let _ = spawn(enviar("ada"))           // ❌
    return "ok"
}
```

```
✗ archivo.fitz — 1 error(es) de tipo:
  Error en línea 5:18 — spawn: la fn `enviar` no está declarada
  con `@background`. Marcá la fn con `@background\nfn
  enviar(...) { ... }` para autorizar su ejecución
  fire-and-forget vía spawn.
```

- El target del `spawn(...)` debe ser un **call literal a fn
  con nombre**. `spawn(x)` con `x` variable, `spawn(obj.method())`,
  `spawn(closure())` — todos rechazados. El target debe ser
  estáticamente claro.

**`spawn(...).await`** si querés esperar el resultado (caso
raro):

```fitz
let result: Null = spawn(track_click(slug)).await
```

---

## Paso 3 — Iter2: timezone con `tz="IANA/Name"`

Sin `tz`, la cron expression se interpreta en **UTC**. Eso casi
siempre es lo que NO querés. `"0 9 * * *"` no es "9 AM en mi
zona" — es 9 AM UTC, lo cual en BsAs es 6 AM y en Madrid es 10
AM.

Para alinear con el huso real del operador, pasás `tz` con un
nombre IANA:

```fitz
@cron("0 9 * * *", tz="America/Argentina/Buenos_Aires")
fn reporte_diario() -> Null {
    print("[cron] reporte 9 AM hora BsAs")
    return null
}

@cron("0 14 * * 1-5", tz="Europe/Madrid")
fn aviso_oficina() -> Null { return null }
```

**Husos aceptados**: cualquier nombre IANA válido. La lista
canónica está en `https://en.wikipedia.org/wiki/List_of_tz_database_time_zones`.
Ejemplos comunes:

- `"UTC"` (default si omitís `tz`).
- `"America/Argentina/Buenos_Aires"`.
- `"America/New_York"`.
- `"Europe/Madrid"`.
- `"Asia/Tokyo"`.

**Validación**: el huso se valida al **boot del scheduler**.
Nombre IANA inválido (typo, zona vieja) → error claro al
arrancar, no espera al primer tick.

**Detalle técnico**: el runtime usa `chrono-tz` con la base IANA
embebida en el binario. Los cambios de DST (horario de verano)
se aplican automáticamente — `"0 9 * * *"` en Madrid es 9 AM
todo el año aunque el offset UTC oscile entre +1 y +2.

---

## Paso 4 — Iter2: retry con backoff

Cuando un job falla (devuelve `Err(...)` o paniquea), por
default no se reintenta — un solo intento. Pero los jobs reales
típicamente tocan recursos externos (API HTTP, DB transient,
cola de mensajes) que **bouncean**. Reintentar con backoff es
patrón estándar.

```fitz
@cron("*/10 * * * * *",
      retry={max: 3, backoff: "exponential", initial_secs: 1, max_secs: 30})
async fn sync_externa() -> Result<Null> {
    // Si esto Err-ea, el runtime reintenta hasta 3 veces más
    // con delays 1s, 2s, 4s (capeado a 30s).
    let _ = sleep(50).await
    return Ok(null)
}
```

**Forma canónica del `retry`** (Map con 4 keys):

```fitz
retry = {
    max: 3,                          // Int: reintentos extra (no incluye el primer intento)
    backoff: "exponential",          // Str: "exponential" | "linear" | "constant"
    initial_secs: 1,                 // Int: delay base en segundos
    max_secs: 30                     // Int: cap superior del delay
}
```

**Backoff kinds**:

| Kind | Cálculo del delay (segundos) | Cuándo usar |
|---|---|---|
| `"exponential"` | `initial_secs * 2^(attempt-1)` | Caso default — upstream que puede bouncear (API HTTP, DB) |
| `"linear"` | `initial_secs * attempt` | Cargas distribuidas predecibles |
| `"constant"` | `initial_secs` | Polling, retry simple |

Todos capeados por `max_secs`. Si `initial_secs=1, max_secs=30`
con exponential, los delays serían `1, 2, 4, 8, 16, 30, 30, 30,
...` (saturado).

**Sin retry** (default) — un solo intento, si falla queda
registrado pero no se reintenta. La política conservadora es la
correcta para muchos casos (jobs no-idempotentes, jobs que es
mejor que un humano vea el error y decida).

**El runtime persiste cada attempt** si tenés `store=db` (paso 6).
Sin store, los attempts quedan solo en stderr.

---

## Paso 5 — Iter2: `catch_up` policy

Caso típico: el proceso estuvo abajo durante uno o más ticks
programados. Cuando arranca, ¿qué hace con los ticks perdidos?

- **`catch_up=false`** (default): **skip silencioso**. El
  scheduler arranca y espera el próximo tick natural. Los ticks
  perdidos se ignoran.
- **`catch_up=true`**: ejecuta **UN** run inmediato al boot si
  el último `last_run_at` registrado está atrasado. No N runs
  (eso sería spam) — uno solo para "ponerse al día".

```fitz
@cron("0 */15 * * *",        // cada 15 min
      catch_up=true,
      store=db)              // catch_up requiere store
async fn sync_periodico() -> Result<Null> {
    print("[cron] sync (con catch_up al boot si missed)")
    return Ok(null)
}
```

**Cuándo usar `catch_up=true`**:

- El job es **idempotente** (correrlo dos veces no rompe nada).
- Querés garantizar que **al menos un run reciente** existe
  cuando arranca el proceso (típico: cleanup, sync, refresh de
  cache).

**Cuándo dejarlo en `false`** (default):

- El job NO es idempotente.
- "Perder un tick" está OK (caso típico: heartbeat metrics, log
  rotation que sí o sí va a llegar el próximo tick).

`catch_up=true` requiere `store=db` para saber `last_run_at`.
Sin store, no hay history.

---

## Paso 6 — Iter2: `store=db` persistencia sobre Postgres

El kwarg más jugoso. Persiste el registry + cada run en dos
tablas Postgres:

```fitz
let db = db.connect(
    env_or("DATABASE_URL", "postgres://postgres:secret@localhost:5432/postgres?sslmode=disable")
).await

@cron("*/10 * * * * *",
      tz="America/Argentina/Buenos_Aires",
      retry={max: 3, backoff: "exponential", initial_secs: 1, max_secs: 30},
      catch_up=true,
      store=db)
async fn heartbeat() -> Result<Null> {
    print("[cron] heartbeat tick")
    return Ok(null)
}

@get("/health")
fn health() -> Str { return "ok" }

@server(43931, docs=false)
fn main() => 0
```

**Lo que pasa al arrancar el scheduler**:

1. El runtime ejecuta `CREATE TABLE IF NOT EXISTS` para las dos
   tablas:

   ```sql
   CREATE TABLE IF NOT EXISTS fitz_cron_jobs (
       name TEXT PRIMARY KEY,
       schedule TEXT NOT NULL,
       tz TEXT NOT NULL,
       last_run_at TIMESTAMPTZ,
       last_status TEXT,
       last_error TEXT,
       next_run_at TIMESTAMPTZ
   );

   CREATE TABLE IF NOT EXISTS fitz_cron_runs (
       id BIGSERIAL PRIMARY KEY,
       job_name TEXT NOT NULL,
       started_at TIMESTAMPTZ NOT NULL,
       finished_at TIMESTAMPTZ,
       status TEXT NOT NULL,    -- 'running' | 'ok' | 'failed' | 'retrying'
       attempt INTEGER NOT NULL,  -- 1-indexed
       error TEXT
   );

   CREATE INDEX IF NOT EXISTS idx_cron_runs_by_job
       ON fitz_cron_runs (job_name, started_at DESC);
   ```

2. Hace `INSERT ... ON CONFLICT (name) DO UPDATE` del registry —
   el nombre del job + su cron expression actual + su tz.
3. Si `catch_up=true`, mira `last_run_at` y decide si dispara
   un run inmediato.
4. Entra al loop normal: por cada tick, `INSERT INTO
   fitz_cron_runs (status='running')`, ejecuta el handler,
   `UPDATE ... SET status='ok'/'failed'/'retrying'`.

**Visibility con `psql`**:

```sql
-- Último estado de cada job.
SELECT name, schedule, tz, last_run_at, last_status, last_error
FROM fitz_cron_jobs;

-- Últimas N ejecuciones (incluye attempts de retry).
SELECT job_name, started_at, finished_at, status, attempt, error
FROM fitz_cron_runs
ORDER BY id DESC
LIMIT 20;

-- Jobs que fallaron en las últimas 24 horas.
SELECT job_name, started_at, status, attempt, error
FROM fitz_cron_runs
WHERE status IN ('failed', 'retrying')
  AND started_at > NOW() - INTERVAL '24 hours'
ORDER BY started_at DESC;
```

**Detalle del binding `db`**:

```fitz
let db = db.connect("...").await   // tipo: Result<DbConn>
```

`db.connect(...).await` retorna `Result<DbConn>` (la conn puede
fallar al arrancar). Como `?` no está soportado a top-level del
archivo, **el binding queda como `Result<DbConn>` puro**.

El runtime/codegen desempaca automáticamente vía el trait
`__FitzCronStoreFrom`: si la conn falló, panea con mensaje claro
al arrancar el scheduler. Equivale al `expect()` que escribirías
a mano.

> **Limitación conocida** — `fitz run` en modo **cron-only**
> (con `@cron(..., store=db)` y SIN `@server` ni handlers HTTP)
> tiene un bug heredado del runtime tokio `current_thread` del
> intérprete: la conn DB queda atada al runtime del evaluator y
> el scheduler la pierde al pasar a `multi_thread`.
>
> **Workarounds**:
>
> 1. Usar `fitz build` (el binario nativo arma su propio runtime
>    multi-thread limpio).
> 2. Agregar al menos un handler HTTP trivial (como el `/health`
>    de arriba) que mantiene el runtime vivo durante el
>    desarrollo.

---

## Paso 7 — Combinando todo: la receta de producción

```fitz
let db = db.connect(env_or("DATABASE_URL", "...")).await

// Cron principal del negocio: sync con API externa, con retry y
// persistencia. Pierde tick si está abajo (no idempotente).
@cron("0 */15 * * * *",                       // cada 15 min
      tz="America/Argentina/Buenos_Aires",
      retry={max: 3, backoff: "exponential", initial_secs: 2, max_secs: 60},
      store=db)
async fn sync_orders() -> Result<Null> {
    // ... HTTP call externa, INSERT en DB ...
    return Ok(null)
}

// Cleanup nocturno: idempotente, con catch_up al boot.
@cron("0 0 3 * * *",                          // 3 AM
      tz="America/Argentina/Buenos_Aires",
      catch_up=true,
      store=db)
async fn cleanup_old_sessions() -> Result<Null> {
    // ... DELETE FROM sessions WHERE created_at < NOW() - INTERVAL '7 days' ...
    return Ok(null)
}

// Heartbeat metrics: si pierde tick, no pasa nada (no store).
@cron("0 * * * * *")                          // cada minuto
fn heartbeat() -> Null {
    // ... emitir métricas a Prometheus / Datadog ...
    return null
}

// Background del flow HTTP: tipado.
@background
async fn registrar_evento(user_id: Int, evento: Str) -> Null {
    // ... INSERT INTO events ...
    return null
}

@post("/api/click")
fn click(user_id: Int, evento: Str) -> Str {
    let _ = spawn(registrar_evento(user_id, evento))
    return "ok"
}

@server(8080)
fn main() => 0
```

**3 jobs con políticas distintas**, **1 background**, **1
handler HTTP**, una sola conn DB compartida, paridad bit-a-bit
`fitz run` ↔ `fitz build`. Sin Celery, sin Redis, sin
docker-compose extra.

---

## Subset compilable a binario

| Feature | `fitz run` | `fitz build` |
|---|---|---|
| `@cron("expr")` 5/6/7 fields | ✅ | ✅ |
| `@background` + `spawn(...)` tipado | ✅ | ✅ |
| `spawn(...).await` para esperar resultado | ✅ | ✅ |
| Cron-only mode (sin `@server`) | ✅ | ✅ |
| `@cron(..., tz="IANA/Name")` | ✅ | ✅ |
| `@cron(..., retry={...})` con 3 backoffs | ✅ | ✅ |
| `@cron(..., catch_up=true)` | ✅ | ✅ |
| `@cron(..., store=db)` con Postgres | ⚠ requiere `@server`* | ✅ |
| `@background` con `tz`/`retry` | ✅ | ✅ |
| `@background` con `store`/`catch_up` | ❌ iter3 | ❌ iter3 |
| Tablas `fitz_cron_jobs`/`fitz_cron_runs` auto-create | ✅ | ✅ |
| UI dashboard tipo Sidekiq Web | ❌ | ❌ |
| Coordinación multi-instancia (locks distribuidos) | ❌ | ❌ |

*Workaround documentado: agregar un handler HTTP trivial
(`/health`) para mantener el runtime tokio vivo. Ver Paso 6.

---

## Validación

- [ ] `@cron("*/2 * * * * *")` sobre fn sync dispara cada 2
      segundos (output verbatim en stdout).
- [ ] El log al boot del scheduler dice
      `🕐 Fitz scheduler arrancado con N job(s) cron`.
- [ ] `@cron` con kwarg desconocido (e.g. `oops="x"`) dispara
      error claro del checker citando "Aceptados: `tz`, `retry`,
      `catch_up`, `store`".
- [ ] `spawn(fn_sin_background)` dispara error del checker:
      "la fn no está declarada con `@background`".
- [ ] `spawn(call_no_literal)` (variable, method call, closure)
      dispara error del checker.
- [ ] Cron-only mode (sin `@server`) queda vivo hasta `Ctrl+C`.
- [ ] `tz="IANA/Name"` inválido (typo) → error al arrancar el
      scheduler, no al primer tick.
- [ ] `retry={max: 3, ...}` con backoff exponential aplica
      delays `initial_secs * 2^(attempt-1)`, capeados por
      `max_secs`.
- [ ] `store=db` arranca → las tablas `fitz_cron_jobs` y
      `fitz_cron_runs` existen en Postgres (verificable con
      `psql -c "\dt fitz_cron_*"`).
- [ ] Cada tick produce un INSERT en `fitz_cron_runs` con
      `status` que transiciona `running → ok/failed`.
- [ ] `catch_up=true` con `store=db` ejecuta UN run extra al
      boot si `last_run_at` está atrasado (no N).
- [ ] `fitz build` del programa con `@cron + @background +
      spawn + store=db` produce binario standalone.
- [ ] El binario nativo cron-only (sin HTTP) responde a
      `Ctrl+C` con cleanup limpio (no zombies).

---

## Troubleshooting

### `@cron sobre fn 'X': kwarg `oops` no reconocido. Aceptados: tz, retry, catch_up, store`

Typo en el nombre del kwarg. **Los aceptados son solo**: `tz`,
`retry`, `catch_up`, `store`.

### `@cron sobre fn 'X': debe tener 1 arg posicional (Str, la cron expression)`

Llamaste `@cron()` sin args, o pasaste algo que no es Str, o
pasaste 2+ args posicionales.

**Fix**: un solo arg posicional Str:

```fitz
@cron("0 0 * * *")        // ✅
fn diario() -> Null { ... }
```

### `spawn: la fn `X` no está declarada con @background`

Olvidaste marcar la fn target. **El opt-in es necesario** — sin
él, el checker rechaza el callsite estáticamente.

**Fix**:

```fitz
@background       // ← agregar
async fn enviar(to: Str) -> Null { ... }
```

### `spawn: el target debe ser un call literal a una fn @background`

Pasaste a `spawn(...)` algo que no es un call literal:

```fitz
let f = enviar
let _ = spawn(f("ada"))    // ❌ f es variable
```

```fitz
let _ = spawn(obj.method())  // ❌ method call
```

**Fix**: llamada directa a fn con nombre:

```fitz
let _ = spawn(enviar("ada"))   // ✅
```

### El cron NO dispara aunque la expression parece OK

Verificá:

1. La expression está bien tipeada (`*/5 * * * * *` con
   espacios, no `*/5*****`).
2. `tz` (si lo pusiste) es un nombre IANA válido.
3. El proceso no terminó antes del primer tick — `fitz run` en
   cron-only mode queda vivo hasta `Ctrl+C`; si el último stmt
   del archivo es `return null` o similar, el programa puede
   terminar antes de que llegue el tick. **Fix**: dejar un
   `print(...)` final o agregar `@server` para mantener el
   runtime vivo.
4. Tu reloj de sistema está OK. El scheduler usa
   `chrono::Utc::now()` + el offset de `tz`.

### `fitz run` con `store=db` pánico al arrancar el scheduler

Bug conocido del runtime tokio `current_thread` del intérprete
en modo cron-only. **Workarounds**:

1. Compilar con `fitz build` y correr el binario.
2. Agregar un handler HTTP trivial al programa:

   ```fitz
   @get("/health")
   fn health() -> Str { return "ok" }

   @server(3000, docs=false)
   fn main() => 0
   ```

   Eso fuerza al runtime a multi-thread y el scheduler funciona.

### `fitz_cron_runs` se llena demasiado rápido

Si tu `@cron` corre cada 2 segundos con `store=db`, vas a
acumular ~43k rows por día. Para producción, considerá:

1. Subir el intervalo del cron a algo realista (cada minuto vs
   cada 2 segundos).
2. Configurar un job de cleanup que borra rows viejas:

   ```fitz
   @cron("0 0 4 * * *", store=db)     // diario 4 AM
   async fn cleanup_runs() -> Result<Null> {
       let _ = db.exec(
           "DELETE FROM fitz_cron_runs WHERE started_at < NOW() - INTERVAL '30 days'",
           []
       ).await
       return Ok(null)
   }
   ```

3. Particionar `fitz_cron_runs` por `started_at` con declarative
   partitioning de Postgres (out of scope del cap, pero
   estándar de DBA).

### El `tz` IANA inválido → cómo lo veo

El error sale al **arrancar el scheduler**:

```
🕐 Fitz scheduler arrancado con 1 job(s) cron
panic: timezone 'America/Argentina/Cordoba' no es un nombre IANA
válido. Lista canónica en
https://en.wikipedia.org/wiki/List_of_tz_database_time_zones
```

Verificá la lista canónica. Errores comunes: poner espacios en
vez de `_`, abreviaciones (`"BsAs"` no es IANA), zonas viejas
deprecadas.

### Job que fallaba en attempt 1 ahora retry pero el delay parece raro

Calculá manual con la fórmula del backoff y comparalo:

- `exponential`: attempt 1 inicial, attempt 2 después de
  `initial_secs * 2^0`, attempt 3 después de
  `initial_secs * 2^1`, etc. Total: `1, 2, 4, 8, 16, ...`
  desde `initial_secs=1`.
- `linear`: `initial_secs, 2*initial_secs, 3*initial_secs, ...`
- `constant`: siempre `initial_secs`.

Todos capeados por `max_secs`. Si el delay observado supera
`max_secs`, hay un bug del scheduler (raro post-iter2) —
abrir issue con el log del scheduler.

### El @cron de `cleanup_old_sessions` corre 3 veces al boot

Tenés `catch_up=true` Y un proceso que se reinicia 3 veces
seguidas. Cada boot mira `last_run_at` y dispara UN run. Si
reinciás rápido, esos 3 runs salen.

**Fix**: en producción, el proceso debería ser estable. Si
estás iterando localmente, podés desactivar `catch_up`
temporalmente.

---

## Cerraste el módulo M5

**Felicidades** — completaste el módulo de async + auth +
real-time. Sabés:

- ✅ Declarar `async fn` + `.await` + `Future<T>` y entender
  por qué Fitz da **paralelismo HTTP real** post-F17, no event
  loop simulado (**C1**).
- ✅ Proteger handlers con `@auth_provider` + `@authenticated`
  + `@admin`, firmar JWT con `jwt.encode` (HS256/384/512) y
  hashear passwords con Argon2id via `hash.password`/`verify`
  — **sin dependencias externas** y validado estáticamente por
  el checker (**C2**).
- ✅ Abrir canales WebSocket con `@ws("/path")` + `WsConn<T>`
  para text JSON con marshaling automático, `WsConn<In, Out>`
  asimétrico, `WsConn<Bytes>` binario raw, heartbeat built-in
  con `@server(ws_heartbeat_secs=N)`, auth pre-upgrade, subprotocol
  `bearer.<token>` para browsers, y **AsyncAPI 3.0** auto en
  `/asyncapi.json` + UI en `/asyncapi` (**C3**).
- ✅ Tareas programadas con `@cron("expr")` + tz IANA + retry
  con backoff + catch_up + persistencia opt-in sobre Postgres
  con `store=db`, fire-and-forget con `@background` + `spawn`
  tipado, todo **sin Celery, sin Redis, sin systemd timers**
  (**C4**) ← acá.

**Entregable del módulo**: tenés todas las features modernas de
producción **integradas en el lenguaje**. La diferencia con
FastAPI/Spring/Express es que en esos stacks necesitás 5-10
dependencias externas + configuración manual + Redis/RabbitMQ
+ Celery worker + uvicorn workers + nginx WS proxy. En Fitz:
`fitz new`, escribir el código, `fitz build`, deploy de un
binario. **Cero pip install, cero npm install, cero docker
compose para infrastructure básica**.

### Comparativa final: lo que ganaste con M5 vs alternativas

| Feature | FastAPI + Celery | Express + Bull + passport | Spring + Quartz + Security | **Fitz M5** |
|---|---|---|---|---|
| Async/await ergonómico | ✅ asyncio | ✅ promises | ⚠ CompletableFuture | ✅ **built-in** |
| Multi-thread real | ⚠ uvicorn workers | ❌ event loop | ✅ | ✅ **post-F17** |
| JWT + bcrypt/Argon2 | 3 deps mínimo | 4 deps mínimo | Spring Security XML | ✅ **0 deps** |
| WebSocket tipado | ❌ json manual | ❌ events sin schema | ❌ STOMP setup | ✅ **`WsConn<T>`** |
| AsyncAPI auto | ❌ | ❌ | ⚠ plugin | ✅ **`/asyncapi.json`** |
| Cron + retry + tz | Celery beat + Redis | Bull + node-schedule | Quartz config | ✅ **kwargs nativos** |
| Job persistence + visibility | Flower + Redis Sentinel | Bull UI + Redis | Quartz JDBC store | ✅ **`store=db`** |
| Compila a binario | ❌ | ⚠ pkg hack | ✅ jar | ✅ **standalone** |
| Deploy = un binario | ❌ Python+Redis+Worker+nginx | ❌ Node+Redis+nginx | ⚠ jar + JVM | ✅ **un binario** |

## Qué viene en M6 — Capstone Postgres + ORM nativo

A partir del próximo módulo entramos al **stack persistente** de
verdad. Postgres con un **driver puro Fitz** (sin libpq, sin
tokio-postgres), **ORM declarativo** sobre `type` con relations
(`@belongs_to`/`@has_many`/`@has_one`), QueryBuilder chain,
agregados, eager loading, y todas las features de DB que vienen
con esto: `Map<Str, Any>` ↔ jsonb, arrays nativos `List<T>` ↔
`T[]`, JSON operators directos sobre `.where(...)`, navigation
methods generados, etc.

M6 cubre 7 capítulos:

- **[C1 — Setup Postgres + `db.connect` + driver crudo](../m6-postgres-orm/c1-setup-driver-crudo.md)**
- **[C2 — `@table`, `@primary` y lecturas tipadas](../m6-postgres-orm/c2-table-decoradores-reads.md)**
- **[C3 — Writes + QueryBuilder chain + agregados](../m6-postgres-orm/c3-writes-querybuilder-agregados.md)**
- **[C4 — Relations + navigation + eager loading](../m6-postgres-orm/c4-relations-navigation-preload.md)**
- **[C5 — Tipos avanzados: jsonb, arrays, Date/DateTime/Uuid](../m6-postgres-orm/c5-tipos-avanzados.md)**
- **[C6 — Migraciones con `fitz db`](../m6-postgres-orm/c6-migraciones-fitz-db.md)**
- **[C7 — Capstone: app CRUD completa con auth + ORM + WS + cron + Docker](../m6-postgres-orm/c7-capstone-crud-completo.md)**

Es el bloque más ambicioso del curso porque integra **TODO lo
visto** en una app production-ready end-to-end. Arrancá con
[M6.C1](../m6-postgres-orm/c1-setup-driver-crudo.md).
