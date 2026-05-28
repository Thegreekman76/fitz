# `api-orm-full` — Stack web first-class entero (HTTP + auth + WS + cron + ORM)

Boilerplate showcase del **stack web first-class completo de Fitz**
en un único proyecto multi-archivo, sin Python, sin librerías
externas, sin broker, sin SQLAlchemy/Celery/Redis. Todo en el
binario `fitz` mismo, paridad bit-a-bit `fitz run` ↔ `fitz build`.

```text
┌─────────────────────────────┐       ┌────────────────────────────┐
│ Cliente HTTP / curl / wscat │       │ Fitz API (multi-archivo)   │
│                             │ ────► │  src/auth.fitz             │
│ POST   /auth/register       │       │  src/posts.fitz            │
│ POST   /auth/login          │       │  src/comments.fitz         │
│ GET    /me                  │       │  src/realtime.fitz (@ws)   │
│ GET    /posts?status&tag    │       │  src/jobs.fitz (@cron)     │
│ POST   /posts (auth)        │       │  src/schema.fitz           │
│ PUT    /posts/{id} (owner)  │       │  src/models.fitz (@table)  │
│ DELETE /posts/{id} (owner)  │       │  src/config.fitz (env)     │
│ POST   /posts/{id}/comments │       │  src/main.fitz             │
│ GET    /stats/posts-per-user│       │                            │
│ WS     /feed (auth)         │       └─────────────┬──────────────┘
└─────────────────────────────┘                     │ wire protocol v3.0
                                                    ▼
                                      ┌────────────────────────────┐
                                      │ Postgres 16                │
                                      │   docker container `db`    │
                                      │   volume: pgdata           │
                                      └────────────────────────────┘
```

## Qué showcasea

| Feature                              | Implementación                                          |
|--------------------------------------|---------------------------------------------------------|
| **HTTP nativo + OpenAPI 3.1 auto**   | `@get`/`@post`/`@put`/`@delete` + `/docs` UI Scalar     |
| **Auth nativa (JWT + Argon2id)**     | `@auth_provider`/`@authenticated` cross-module          |
| **WebSockets tipados + AsyncAPI 3.0**| `@ws("/feed")` + broadcast + heartbeat + `/asyncapi.json` |
| **Cron jobs sin broker**             | `@cron("0 0 * * *")` scheduler embebido en el binario   |
| **ORM declarativo sobre `type`**     | `@table`/`@primary`/`@belongs_to`/`@has_many`/`@has_one`|
| **Driver Postgres puro Fitz/Rust**   | Wire protocol v3.0 directo, sin libpq                   |
| **Relations + eager loading**        | `.preload("author")` + `.preload("comments")`           |
| **JSONB nativo**                     | `metadata: Map<Str, Any>` ↔ Postgres `jsonb`            |
| **Arrays nativos**                   | `tags: List<Str>` ↔ Postgres `text[]` + `.has(var)`     |
| **Aggregates + GROUP BY**            | `.group_by(fn(p) => p.author_id).count(db)`             |
| **Partial updates dinámicos**        | `Map<Str, Any>` con `m["k"] = v` + `.update(db, map)`   |
| **Cross-module multi-file**          | 9 módulos Fitz coordinados (W12 + W16 + W17 + W18)      |
| **Paridad `fitz run` ↔ `fitz build`**| Mismo behavior en intérprete y binario nativo           |

## Por qué este boilerplate es distinto

Otros lenguajes alcanzan piezas de este stack con frameworks
maduros, pero **ninguno los integra como features intrínsecos del
lenguaje** con un solo binario standalone sin dependencias externas:

- **Python + FastAPI**: HTTP + auth + OpenAPI con muchas libs
  (`fastapi-jwt`, `passlib`, `SQLAlchemy`, `celery`, `redis`,
  `psycopg2`). Múltiples procesos (api + worker + broker), imagen
  Docker ~250 MB+, Python en runtime.
- **Node + NestJS**: similar, con `passport`, `bull`,
  `typeorm`/`prisma`. JS runtime + node_modules en producción.
- **Go**: stdlib excelente, pero auth + WS + cron + ORM son libs
  separadas (gin/echo + jwt-go + gorilla/websocket + cron/v3 +
  gorm/ent). Más boilerplate manual entre ellas.
- **Rust + actix/axum**: idem Go, todo es lib (tokio + axum +
  jsonwebtoken + tokio-tungstenite + sqlx). Compile times largos,
  setup denso.

Fitz combina todos como **decoradores y módulos built-in del
compilador**. El checker estático valida el stack entero en
compile-time. El binario producido por `fitz build` no necesita
nada en el sistema destino — solo Postgres reachable por TCP.

## Estructura

```
api-orm-full/
├── fitz.toml                # manifest (paquete + [bin].main)
├── docker-compose.yml       # Postgres + api + healthcheck
├── Dockerfile               # build multi-stage (debian-slim)
├── Dockerfile.distroless    # variante distroless (~15-20 MB)
├── .env.example             # vars de entorno
├── .gitignore               # ignora target/ + secrets
├── README.md                # este archivo
└── src/
    ├── main.fitz            # entry — imports + @server + boot logs
    ├── config.fitz          # env_or() para DB_URL + JWT_SECRET + cron interval
    ├── models.fitz          # 4 @table types (User/Profile/Post/Comment)
    ├── schema.fitz          # CREATE TABLE IF NOT EXISTS (idempotente)
    ├── auth.fitz            # @auth_provider + /auth/register + /auth/login + /me
    ├── posts.fitz           # CRUD /posts + filtros + eager loading + aggregate
    ├── comments.fitz        # /posts/{id}/comments (lista + crear)
    ├── realtime.fitz        # @ws /feed (broadcast simétrico autenticado)
    └── jobs.fitz            # @cron cleanup_old_drafts
```

## Cómo correr

### Setup (una vez)

```bash
cp .env.example .env
# editar .env si querés cambiar JWT_SECRET o credenciales DB
```

### Desarrollo

```bash
# Pin a la versión que incluya los fixes del boilerplate (v0.10.9+).
# Mientras el tag no esté publicado, usar el FITZ_REV del commit:
docker compose build --build-arg FITZ_TAG=v0.10.9
docker compose up -d
```

> **Nota sobre versión de Fitz**: el boilerplate ejercita features
> introducidas en v0.10.9 (Map<Str, Any> indexing assignment
> dinámico, W18 cross-module virtuales, .has(var) sobre arrays).
> Hasta que el tag esté publicado, `cargo install --git` con
> default `main` puede no tenerlas. Pin con `--build-arg
> FITZ_TAG=v0.10.9` (o `FITZ_REV=<commit-sha>`) para builds
> reproducibles.

El primer build compila `fitz` desde source (~5-8 min). Builds
subsiguientes solo recompilan tu código (~30s).

Esperá a ver en los logs:

```
[boot] schema DB inicializado
[ready] server arrancando en :3000
```

### Probar con curl

```bash
# 1. Crear cuenta
curl -X POST localhost:3000/auth/register \
     -H 'Content-Type: application/json' \
     -d '{"email":"ada@example.com","name":"Ada","password":"secret-ada-123"}'

# 2. Login → obtener JWT
TOKEN=$(curl -sX POST localhost:3000/auth/login \
     -H 'Content-Type: application/json' \
     -d '{"email":"ada@example.com","password":"secret-ada-123"}' \
     | jq -r .token)

# 3. Crear post con tags + jsonb metadata
curl -X POST localhost:3000/posts \
     -H "Authorization: Bearer $TOKEN" \
     -H 'Content-Type: application/json' \
     -d '{
       "title": "Hola Fitz",
       "slug": "hola-fitz",
       "content": "Primer post",
       "status": "published",
       "tags": ["rust", "fitz"],
       "metadata": {"lang": "es"}
     }'

# 4. Listar con filtro por tag (ejercita .has(var) sobre text[])
curl 'localhost:3000/posts?tag=rust'

# 5. Listar drafts (filtro status)
curl 'localhost:3000/posts?status=draft'

# 6. Partial update (Map<Str, Any> construido dinámicamente)
curl -X PUT localhost:3000/posts/1 \
     -H "Authorization: Bearer $TOKEN" \
     -H 'Content-Type: application/json' \
     -d '{"title": "Hola Fitz (editado)", "slug": "", "content": "", "status": ""}'
# (los fields "" se ignoran del update — solo title se actualiza)

# 7. Comentar (auth required)
curl -X POST localhost:3000/posts/1/comments \
     -H "Authorization: Bearer $TOKEN" \
     -H 'Content-Type: application/json' \
     -d '{"content":"primer comentario"}'

# 8. Eager loading (1 post + author + comments en 3 queries batch)
curl localhost:3000/posts/1

# 9. Aggregate (GROUP BY author_id)
curl localhost:3000/stats/posts-per-user

# 10. Docs autogenerados
open localhost:3000/docs                       # OpenAPI 3.1 UI Scalar
curl localhost:3000/asyncapi.json | jq .       # AsyncAPI 3.0 WS schema
```

### WebSocket /feed (auth required)

```bash
# Con wscat instalado (npm install -g wscat):
wscat -c "ws://localhost:3000/feed" \
      -H "Authorization: Bearer $TOKEN"

# Después de conectar, mandar un evento:
> {"kind":"system","text":"Hola desde wscat"}

# Otros clientes conectados al /feed reciben el broadcast.
```

## Cron jobs

El `@cron("0 0 * * *")` en `src/jobs.fitz` ejecuta
`cleanup_old_drafts()` todos los días a medianoche UTC. Borra
posts en status `"draft"` con `created_at` más viejo que
`MAX_DRAFTS_AGE_DAYS` (default 30, configurable vía env var).

Para testear sin esperar 24h, cambiar el schedule
temporalmente a `"*/1 * * * *"` (cada minuto) y rebuild.

## Imagen distroless

Para deployment minimalista (~15-20 MB en lugar de ~80 MB):

```bash
docker build -f Dockerfile.distroless -t api-orm-full:distroless .
docker run --rm \
  -e DATABASE_URL='...' \
  -e JWT_SECRET='...' \
  -p 3000:3000 \
  api-orm-full:distroless
```

`gcr.io/distroless/cc-debian12` no tiene shell ni paquete manager
— solo el binario fitz-app + las libs C dinámicas mínimas
(glibc + libgcc).

## Notas de diseño

### Cross-module imports

Cada módulo que usa `@table` types o `@authenticated` necesita
**importar TODOS los `@table` types referenciados** (no solo los
que usa directamente). Esto es por el codegen del ORM cross-module
que valida cada relation target en compile-time.

```fitz
// models.fitz tiene User/Profile/Post/Comment. Si posts.fitz usa
// solo Post, igual hace:
from models import User, Profile, Post, Comment
```

Workaround conocido (deuda residual del codegen cross-module).

### Provider de auth cross-module

El `@auth_provider` declarado en `auth.fitz` se encuentra
cross-module vía W12 — pero los módulos que usan `@authenticated`
necesitan **importar `auth` también** (sino el pre-scan no lo
descubre):

```fitz
// posts.fitz
import auth   // necesario para que el provider del auth.fitz
              // sea visible al chequear los @authenticated locales

@authenticated
@post("/posts")
async fn create_post(user: User, body: PostInput) -> Result<Post> { ... }
```

### Order de fields en `@table type`

El codegen exige que los types target de `@belongs_to`/companion
estén declarados **antes** en el mismo archivo. Los `@has_many` con
`("Target", ...)` aceptan forward refs (Target después). Por eso
en `src/models.fitz`:

```fitz
@table("users") type User { ... }      // 1ro — referenciado por todos
@table("profiles") type Profile { ... } // 2do — companion User
@table("posts") type Post { ... }       // 3ro — companion User
@table("comments") type Comment { ... } // 4to — companion Post + User
```

## Deuda residual conocida

Estos gaps están abiertos y se documentan en `docs/deudas-post-5b.md`
del repo principal de Fitz:

- **Migraciones automáticas**: `fitz db diff` / `fitz db migrate`
  no existen todavía. El schema vive en `src/schema.fitz` como
  DDL crudo idempotente (`CREATE TABLE IF NOT EXISTS`).

Los gaps cross-module destapados al construir este boilerplate
(OpenAPI cross-module paths, WS Router cross-module, AsyncAPI
cross-module, ORM Str sentinel del INSERT, HTTP wrapper Result
tail, W17 eager loading, narrowing flow-sensitive de Nullable,
`ws_broadcast` cross-handler) se cerraron en **v0.10.9** —
detalle en `docs/deudas-post-5b.md` sección "Mini-fase post-
release v0.10.9" + CHANGELOG.
