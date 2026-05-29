# `api-fullstack-postgres` — CRUD completo fullstack con frontend vanilla

Boilerplate que demuestra el **stack web entero** de Fitz: API
CRUD con Postgres, **frontend HTML+JS vanilla** consumiéndola, y
los tres servicios corriendo en containers separados con
docker-compose.

A diferencia del boilerplate hermano `api-postgres-python` (que es
solo API + curl), este suma:

- **UI rica vanilla** (~370 LoC HTML+CSS+JS, sin frameworks).
  Tabla con todos los tasks, formulario para crear, edit inline,
  delete con confirm, filtros pendientes/completas, badges de
  prioridad, contadores live.
- **CORS real cross-origin**: el frontend en `localhost:8080`
  (nginx) hace fetch a la API en `localhost:3000` (Fitz). El
  browser dispara preflight OPTIONS antes de los POST/PUT/DELETE
  — el `@middleware(cors(...))` de cada handler responde con los
  `Access-Control-Allow-*` necesarios.
- **3 servicios en compose** en lugar de 2: `db` + `api` +
  `frontend`.

```text
┌──────────────────────────────────────────────────────────────┐
│ Browser (http://localhost:8080)                              │
│                                                              │
│   fetch('http://localhost:3000/tasks') ─┐                    │
│                                         │                    │
└─────────────────────────────────────────┼────────────────────┘
                                          │ HTTP + CORS preflight
                                          ▼
┌──────────────────────────────────────────────────────────────┐
│ Fitz API (http://localhost:3000)                             │
│   @get/@post/@put/@delete /tasks                             │
│   @middleware(cors({allow_origin: "http://localhost:8080"})) │
└─────────────────────────────────────┬────────────────────────┘
                                      │ from python import db
                                      ▼
┌──────────────────────────────────────────────────────────────┐
│ python/db.py (SQLAlchemy 2.x + psycopg2-binary)              │
└─────────────────────────────────────┬────────────────────────┘
                                      │ TCP :5432
                                      ▼
┌──────────────────────────────────────────────────────────────┐
│ Postgres 16-alpine — volume `pgdata` persiste between runs   │
└──────────────────────────────────────────────────────────────┘
```

## Qué hace el sistema

Una **lista de tareas (todo list)** con:
- Crear tasks con título, descripción y prioridad (baja / media /
  alta).
- Marcar como completas / pendientes (checkbox).
- Editar inline (botón Editar abre la fila en modo edición).
- Borrar con confirmación.
- Filtrar entre Todas / Pendientes / Completas.
- Contadores en vivo (pendientes + completas en el header).

Cada interacción del frontend dispara un request HTTP a la API
Fitz, que persiste todo en Postgres.

## Quickstart

### Prerequisitos

- **Docker Desktop** (Windows/Mac) o **Docker + docker-compose**
  (Linux). Versión 24+ recomendada.
- Una terminal con `bash` o `pwsh`/`cmd.exe`.
- Browser moderno (cualquier evergreen — Chrome, Firefox, Edge,
  Safari).

No necesitás Rust, Python ni Postgres instalados en tu máquina —
todo corre adentro de Docker.

### Setup (una sola vez)

```bash
# 1. Clonar el repo (o copiar este directorio aparte).
cd boilerplates/api-fullstack-postgres

# 2. Copiar el archivo de variables de entorno.
cp .env.example .env

# 3. (Opcional) Editar .env si querés cambiar credenciales de
#    Postgres. Para correr en local, los defaults funcionan.
```

### Arranque

```bash
docker compose up --build
```

**Primer build**: ~30-60s (desde v0.10.13). El Dockerfile usa la
imagen pre-built `ghcr.io/thegreekman76/fitz:latest-python` que ya
viene con Fitz `--features python` compilado. Las builds
posteriores (cuando cambies código de `src/` o `python/`) son
~10-20s gracias al cache de Docker BuildKit.

Antes (pre-v0.10.13) el build era ~8-12 minutos porque compilaba
Fitz desde source con `cargo install --git`. Si necesitás reproducir
con versión pinned:

```bash
docker compose build --build-arg FITZ_TAG=v0.10.13-python
```

Cuando veas en los logs:

```
fitz-fullstack-api  | [boot] DB conectada y schema inicializado
fitz-fullstack-api  | [ready] Server arrancando en :3000
```

abrí en el browser: **http://localhost:8080**

### Tear down

```bash
# Parar todo. Los datos en Postgres SOBREVIVEN.
docker compose down

# Parar todo + borrar TODA la data de Postgres (reset completo).
docker compose down -v
```

## Estructura del directorio

```
api-fullstack-postgres/
├── README.md                   ← este archivo
├── fitz.toml                   ← manifest del package manager Fitz
├── docker-compose.yml          ← 3 servicios (db + api + frontend)
├── Dockerfile                  ← build del container api
├── .env.example                ← template de credenciales
├── .env                        ← (creado por vos, no commiteado)
├── requirements.txt            ← deps Python (sqlalchemy + psycopg2)
├── .dockerignore               ← excluye target/, __pycache__/, etc.
├── .gitignore                  ← excluye .env, target/, etc.
│
├── src/                        ← código Fitz
│   ├── main.fitz               ← handlers HTTP + init schema
│   ├── types/
│   │   ├── task.fitz           ← type Task (5 fields)
│   │   └── api.fitz            ← type NewTask, UpdateTask (bodies)
│   └── data/
│       └── tasks.fitz          ← wrapper Fitz sobre python/db.py
│
├── python/                     ← lado Python (helpers SQLAlchemy)
│   ├── models.py               ← SQLAlchemy Task model
│   └── db.py                   ← engine + sessionmaker + CRUD helpers
│
└── frontend/                   ← UI vanilla
    ├── index.html              ← HTML + CSS + JS en un solo archivo
    └── nginx.conf              ← config minimal de nginx
```

## URLs disponibles

Una vez arriba:

| URL                                  | Qué hay                                |
|--------------------------------------|----------------------------------------|
| http://localhost:8080                | **Frontend** (UI de tasks)             |
| http://localhost:3000/tasks          | API — `GET` lista todas las tasks      |
| http://localhost:3000/tasks?filter=pending | API — `GET` solo pendientes        |
| http://localhost:3000/tasks?filter=done    | API — `GET` solo completas         |
| http://localhost:3000/tasks/{id}     | API — `GET`/`PUT`/`DELETE` una task    |
| http://localhost:3000/docs           | **OpenAPI UI** (Scalar) autogenerada   |
| http://localhost:3000/openapi.json   | Schema OpenAPI 3.1 crudo               |
| `localhost:5432` (Postgres)          | Conectable con psql/DBeaver/etc.       |

## Endpoints de la API

Todos los handlers responden con CORS para `http://localhost:8080`.

| Método  | Path              | Body (JSON)                                              | Response (200)                |
|---------|-------------------|----------------------------------------------------------|-------------------------------|
| GET     | `/tasks?filter=`  | —                                                        | `List<Task>`                  |
| POST    | `/tasks`          | `{title, description?, priority?}`                       | `Task` con `id`/`created_at`  |
| GET     | `/tasks/{id}`     | —                                                        | `Task`                        |
| PUT     | `/tasks/{id}`     | `{title, description?, priority?, done}`                 | `Task` actualizada            |
| DELETE  | `/tasks/{id}`     | —                                                        | `{deleted: true, id}`         |

El tipo `Task` tiene 5 fields:
- `id: Int` — auto-asignado por Postgres
- `title: Str` — requerido
- `description: Str` — opcional (default `""`)
- `priority: Str` — `"low"` / `"med"` / `"high"` (default `"med"`)
- `done: Bool` — default `false`
- `created_at: Str` — ISO 8601, auto-asignado

## Test rápido con curl

Sin abrir el browser, podés ejercitar la API desde otra terminal:

```bash
# 1. Crear una task.
curl -X POST http://localhost:3000/tasks \
     -H 'Content-Type: application/json' \
     -d '{"title":"Comprar pan","priority":"high"}'

# 2. Listar.
curl http://localhost:3000/tasks

# 3. Marcar como completa.
curl -X PUT http://localhost:3000/tasks/1 \
     -H 'Content-Type: application/json' \
     -d '{"title":"Comprar pan","description":"","priority":"high","done":true}'

# 4. Listar solo completas.
curl 'http://localhost:3000/tasks?filter=done'

# 5. Borrar.
curl -X DELETE http://localhost:3000/tasks/1
```

## Qué demuestra este boilerplate

### 1. Stack web completo con Fitz como API

El frontend (HTML+JS) y la API (Fitz) son procesos **separados**
en containers distintos. Esto modela el deployment real moderno:
el frontend puede vivir en un CDN (Cloudflare/Vercel/etc.) y la
API en un servidor cualquiera. CORS resuelve el cross-origin
correctamente — sin configurar nada extra, los `@middleware(cors(...))`
en cada handler ya están listos.

### 2. CORS preflight automático

Los métodos POST, PUT, DELETE con `Content-Type: application/json`
disparan **preflight OPTIONS** en el browser antes del request
real. Fitz responde automáticamente con 204 + headers
Access-Control-Allow-* — no hay que escribir el preflight a mano.
Ver `src/main.fitz` para los `@middleware(cors({...}))` por handler.

### 3. Módulos Fitz multi-archivo

Estructura típica de un proyecto Fitz real: tipos en una carpeta,
data en otra, handlers en el entry. El `main.fitz` no ve
`from python import` directo — el wrapper de `data/tasks.fitz`
encapsula la interop.

```
from python import json
from types.task import Task
from types.api import NewTask, UpdateTask
from data.tasks import create_raw, find_raw, list_raw, update_raw, delete_raw, init_schema
```

Imports resueltos por el loader Fitz:
- `from types.task import Task` → `src/types/task.fitz`.
- `from types.api import NewTask, UpdateTask` → `src/types/api.fitz`.
- `from data.tasks import ...` → `src/data/tasks.fitz`.

### 4. Interop Python end-to-end (Fase 8)

- `from python import db` + `from python import json` desde
  el wrapper `data/tasks.fitz`.
- Round-trip por JSON para coercer `dict` Python a `Instance`
  Fitz (patrón canónico mientras la coerción directa
  `dict → Instance` sobre PyAny en `fitz build` sigue siendo
  deuda residual — ver más abajo).
- Excepciones SQLAlchemy (`NoResultFound` al hacer `find`/
  `update`/`delete` con id inexistente) wrapeadas
  automáticamente como `Result::Err` (Fase 8.3) → propagadas
  con `?` al handler HTTP → 500 con `{"error": "..."}` JSON.

### 5. OpenAPI 3.1 + Scalar UI autogenerados

Sin escribir nada extra, en http://localhost:3000/docs te encontrás
con una UI interactiva (powered by Scalar) que documenta todos
los endpoints. El schema bit-a-bit idéntico está en
`/openapi.json`. Ver Fase 7 del lenguaje.

## Cómo funciona — paso a paso

### Flujo de un POST /tasks

1. **Browser** carga `http://localhost:8080/index.html` desde
   nginx.
2. Usuario completa el form y aprieta "Agregar". JS hace:
   ```js
   fetch('http://localhost:3000/tasks', {
       method: 'POST',
       headers: { 'Content-Type': 'application/json' },
       body: JSON.stringify({ title, description, priority })
   });
   ```
3. **Browser** ve cross-origin (puertos distintos) + POST con
   JSON → dispara preflight OPTIONS automático a
   `localhost:3000/tasks`.
4. **Fitz** responde 204 con headers
   `Access-Control-Allow-Origin: http://localhost:8080` +
   `Access-Control-Allow-Methods: POST, OPTIONS` +
   `Access-Control-Allow-Headers: Content-Type`. Sin código
   manual — lo hace el `@middleware(cors({...}))`.
5. **Browser** valida y manda el POST real.
6. **Fitz** matchea la ruta `@post("/tasks")` →
   `create_task(body: NewTask)`. El JSON body se deserializa
   automáticamente a `type NewTask`.
7. **`create_task`** llama a `create_raw(body.title, ...)`
   (definido en `data/tasks.fitz`).
8. **`create_raw`** llama a `db.add_task(title, ...)`
   (definido en `python/db.py`) — interop con CPython adentro
   del mismo proceso, mismo GIL.
9. **`db.add_task`** abre una sesión SQLAlchemy, hace
   `session.add(task)` + `commit()`, devuelve `task.to_dict()`.
10. El `dict` Python vuelve a Fitz coercido como `Map`. El
    wrapper `create_raw` hace `json.dumps(raw)` → `Str` JSON.
11. **`create_task`** hace `let t: Task = json.loads(raw)?` —
    la anotación destino (`Task`) dispara la coerción runtime
    `Map → Instance` (Fase 8.4).
12. **Fitz** serializa la `Task` a JSON y responde 200 +
    headers CORS.
13. **Browser** recibe el JSON, actualiza el state JS, re-render.

Todo el round-trip toma < 50ms en local.

## Troubleshooting

### "Connection refused" al levantar

Esperá hasta ver `[boot] DB conectada y schema inicializado` en
los logs del api. Postgres tarda ~2-3s en estar listo al primer
boot; el `depends_on.condition: service_healthy` del compose hace
que el api espere automáticamente.

### El frontend muestra "Error al cargar"

Verificá que la API esté arriba:
```bash
curl http://localhost:3000/tasks
```
Si esto responde con `[]` o una lista, la API está OK y el
problema es CORS. Mirá la consola del browser (F12) — debería
mostrar el error específico.

Si `curl` falla:
- Confirmá que `docker compose up` no tuvo errores.
- Revisá `docker compose ps` — los 3 services deben estar `Up`.
- Mirá `docker compose logs api` para ver si Fitz logueó algo.

### Cambié el código de `src/*.fitz` pero no veo cambios

El container del api **no monta volume** del code — el código
está copiado al image en build time. Para ver cambios:
```bash
docker compose up --build
```
(El `--build` fuerza re-build del image del api).

Para desarrollo más cómodo con hot-reload, sumá un volume mount
al servicio `api` en `docker-compose.yml`:
```yaml
api:
  volumes:
    - ./src:/app/src:ro
```
y dentro del container corré `fitz dev src/main.fitz` (hot reload
nativo de Fitz). Por simplicidad, el boilerplate no incluye eso
por default.

### Quiero conectarme a Postgres con psql

```bash
docker compose exec db psql -U fitz -d fitz
```

O desde el host (Postgres expone :5432):
```bash
psql -h localhost -U fitz -d fitz
# password: fitz (de .env)
```

### Reset completo de la DB

```bash
docker compose down -v   # -v borra los volumes (incluyendo pgdata)
docker compose up --build
```

### El build del api tarda mucho

El primer build compila Fitz **desde source** con `--features
python`. Esto incluye PyO3, axum, tokio, serde, etc. — son ~600
crates. Tomá ~8-12 minutos en una máquina moderna.

Si querés acelerar:
1. Subis los recursos asignados a Docker Desktop (CPUs/RAM).
2. Si tu repo es público y tenés releases, podrías cambiar el
   `cargo install --git` por un `docker pull
   ghcr.io/<owner>/fitz:vX.Y.Z` y copiar el binario ya
   compilado. Hoy el boilerplate compila desde source para
   independencia del registry.

### "ERROR: command exited with non-zero status 1" en `fitz run`

Mirá `docker compose logs api`. Causas típicas:
- **DB no llegó a inicializar todavía**: esperá 5s y reintentá.
- **Error de tipo de Fitz**: probablemente cambiaste el código y
  rompiste algo. El log muestra el error exacto.
- **Error de sintaxis Python en `python/db.py`**: el log muestra
  la traceback Python embebida en el error Fitz (gracias a Fase
  8.3).

## Deuda residual conocida

Este boilerplate **demuestra el caso real** del stack actual de
Fitz, incluyendo algunas deudas que viven en el lenguaje:

### `fitz run` en lugar de `fitz build`

El container ejecuta `fitz run src/main.fitz` (el intérprete) en
lugar de compilar a binario nativo con `fitz build`. La razón es
**performance del boot**: `fitz build` produce binario standalone
pero el primer build adentro del container compila desde source
(~8-12 min). `fitz run` arranca instantáneo y el intérprete
ejecuta el código directamente. Para un boilerplate didáctico es
preferible.

> Nota técnica: la coerción `dict → Instance` y
> `list[dict] → List<Instance>` Python ya funciona también en
> `fitz build` desde la mini-fase 8.7.bis (deuda **R.bug-8.7-coercion-list-codegen**
> cerrada el 2026-05-22). Si querés compilar el boilerplate a
> binario nativo, cambiá el `CMD` del Dockerfile a
> `["fitz", "build", "src/main.fitz"]` y referenciá el binario
> emitido — funciona bit-a-bit como el intérprete.

### Sin async DB

`python/db.py` usa SQLAlchemy 2.x **sync** + psycopg2. La API
Fitz es async pero las llamadas a `db.*` son blocking adentro del
GIL (el bridge tokio↔asyncio de Fase 8.6 funciona, pero
SQLAlchemy async + asyncpg requiere reescritura de los helpers).
Para servicios con muchas conexiones concurrentes, este es un
límite a tener en cuenta.

### `fitz.toml` y package manager

El proyecto declara `fitz.toml` para activar el modo manifest del
package manager (Fase 9.y). El loader del Fitz interno **no
consume** el manifest todavía para resolver módulos relativos
(deuda **R.bug-loader-relative-only**). Por eso `data/tasks.fitz`
no puede hacer `from types.task import Task` directo — debería
buscar `src/data/types/task.fitz` y falla. El workaround
aplicado: el wrapper data/ devuelve `Str` (JSON crudo) y el
`main.fitz` hace la coerción `json.loads` allí donde el tipo
**sí** está en scope.

## Stack y versiones

- **Fitz**: lo que esté pineado al builddear (HEAD del default
  branch por default; pin con `FITZ_TAG=vX.Y.Z`).
- **Python**: 3.12 (slim) en ambos stages (builder y runtime) por
  match obligatorio de libpython (ver Dockerfile).
- **Postgres**: 16-alpine (oficial).
- **SQLAlchemy**: 2.x sync.
- **psycopg2-binary**: 2.9.x.
- **nginx**: alpine (cualquier versión reciente).
- **Browser**: cualquier evergreen.

## Para llevarlo a producción

Este boilerplate sirve como **plantilla**, no como deploy de prod
directo. Cambios necesarios antes de exponerlo público:

1. **Credenciales fuertes** en `.env`. Generar `POSTGRES_PASSWORD`
   aleatorio (no `fitz`).
2. **TLS / HTTPS**: poné nginx (o Caddy / Traefik) adelante con
   certificados (Let's Encrypt). Fitz no termina TLS hoy.
3. **CORS más estricto**: `allow_origin: "http://localhost:8080"`
   debe cambiar al dominio real del frontend.
4. **No exponer Postgres**: borrar `ports: ["5432:5432"]` del
   `db` en `docker-compose.yml` — el api se conecta por la
   network interna.
5. **Backups de Postgres**: scheduled `pg_dump` al volume o S3.
6. **Logs**: hoy todo va a stdout. Centralizar con Loki/ELK/etc.
7. **Migraciones**: hoy `init_db()` crea las tablas si no existen
   pero no maneja cambios de schema (alter table). Para
   producción real, sumar Alembic o esperar al ORM nativo de
   Fitz (Fase 10).
8. **Healthcheck del api**: sumar un `@get("/health")` y
   declararlo como healthcheck del service en compose.

## Próximos pasos para el lector

Si querés extender este boilerplate:

- **Sumar autenticación**: ver el boilerplate
  `api-middleware-cors` para `@auth_provider` + `@authenticated`
  + JWT + Argon2id. Stack-en-Fitz, sin deps extras.
- **Sumar WebSockets**: ver `api-websocket` para `@ws("/path")`
  con broadcast + heartbeat.
- **Cron jobs / background workers**: Fitz tiene `@cron` y
  `@background` nativos (Fase 9.w.3). Sin Celery, sin Redis.
- **Logging estructurado**: sumar un middleware custom que log el
  método/path/status/duración de cada request.

Para más, leé la **[guía del lenguaje](https://github.com/Thegreekman76/fitz/blob/main/docs/guide.md)**
del proyecto Fitz.

## Roadmap del boilerplate

- **`fitz build --bundle-python` (Fase 8.b cerrada 2026-05-23)** +
  **`fitz build --bundle-pip` (Fase 8.c cerrada 2026-05-23)** +
  **`fitz build --bundle-pip-requirements` (cosecha 8.c v0.9.42)**:
  el flag empaqueta paquetes pip junto al CPython base. **Blocker
  #1 (rechazo del codegen 8.7.1 transitiva) CERRADO en v0.9.43**
  + **Sub-deuda #1.5/#1.6 CERRADAS en v0.9.44** + **deuda
  distroless-tar-embedded CERRADA en v0.9.46** (launcher con
  `tar`+`flate2` inline destraba `gcr.io/distroless/cc-debian12`)
  + **Smoke real Docker end-to-end VALIDADO en v0.9.52**
  (imagen final 136 MB, POST/GET/CORS preflight todo OK, frontend
  SPA + Postgres). Variante `Dockerfile.distroless` +
  `docker-compose.distroless.yml` listos para production.
  Caveat residual:

  1. ~~Rechazo del codegen — `from python import` en módulos
     transitivos NO soportado~~ **CERRADO en v0.9.43**.
     `src/data/tasks.fitz` mantiene `from python import db`
     adentro sin refactor.
  1.5. ~~Coerción Python `dict → Instance<T>` y `list → List<T>`
     en `fitz build` para tipos `T` importados~~ **CERRADO en
     v0.9.44**. Main emite los helpers `__fitz_py_to_instance_<T>`
     /`__fitz_py_to_list_<T>` también para tipos custom de
     módulos transitivos.
  1.6. ~~Impls HTTP (`__ToFitzJson`/`__FromFitzJson`) para tipos
     importados~~ **CERRADO en v0.9.44**. El mismo pase
     unificado emite los impls HTTP — handlers que aceptan/
     devuelven `Task`, `NewTask`, etc. importados compilan.
  2. GLIBC mismatch builder/runtime — fix con
     `python:3.14-slim-bookworm`.
  3. Beneficio real ~10-20 MB (no 50-70 MB) — argumento queda
     como simplificación de runtime, no como ahorro de deploy size.

  El plan concreto del Dockerfile está documentado en el README
  del boilerplate hermano `api-postgres-python` y aplica idéntico
  acá (cambia solo el nombre del binario:
  `fitz-api-fullstack-postgres`).

  **Con el cierre v0.9.52**, el `fitz build` de este boilerplate
  (multi-archivo con data layer separado + frontend SPA + CORS)
  compila limpio end-to-end Y el smoke real Docker está
  validado. Usar `docker compose -f docker-compose.distroless.yml
  up --build` directo para production-ready.

  **Workaround temporal aplicado en `src/data/tasks.fitz`**: los 5
  helpers (`create_raw`/`find_raw`/`list_raw`/`update_raw`/
  `delete_raw`) usan binding intermedio anotado `let s: Str =
  json.dumps(raw)?` en lugar de `return Ok(json.dumps(raw)?)`
  inline. Razón: bug **8.7-ok-propagation** del codegen Python
  (expected type adentro de `Ok(...)` no propaga al inner).
  NO afecta `fitz run`. Cuando 8.7-ok-propagation cierre como
  mini-fase dedicada, los 5 helpers vuelven al patrón inline.
- **DB nativa Fitz (Fase 10)**: reemplazo de la layer Python
  cuando llegue. Mismo API HTTP + mismo frontend, sin interop
  ni `requirements.txt`.
