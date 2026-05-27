# `api-postgres-fitz` — CRUD HTTP con ORM nativo Fitz + Postgres

Boilerplate del **stack realista** de un servicio CRUD HTTP, igual
al de [`api-postgres-python`](../api-postgres-python/) **pero sin
una sola línea de Python**:

- **API Fitz** single-file usando el **ORM nativo** del lenguaje
  (cap 31 de la guía).
- **Driver Postgres puro** escrito en Fitz/Rust — wire protocol v3.0
  directo, sin libpq ni intermediarios.
- **Postgres 16** corriendo en otro container del docker-compose,
  con healthcheck + volume persistente.

```text
┌────────────────────────────┐       ┌────────────────────────────┐
│ Cliente HTTP (curl/Postman)│       │ Fitz API                   │
│                            │ ────► │  @table User { ... }       │
│ POST /users                │       │  @get /users               │
│ GET /users                 │       │  User.where(...).all(db)   │
│ GET /users/{id}            │       │                            │
└────────────────────────────┘       └─────────────┬──────────────┘
                                                   │ wire protocol v3.0
                                                   ▼
                                     ┌────────────────────────────┐
                                     │ Postgres 16                │
                                     │   docker container `db`    │
                                     │   volume: pgdata           │
                                     └────────────────────────────┘
```

## Side-by-side vs `api-postgres-python`

Mismo dominio (users CRUD), mismos endpoints, mismo Docker Compose,
mismo Postgres. La diferencia es **toda en el stack de datos**:

|                                | `api-postgres-python`            | `api-postgres-fitz` (este)      |
|--------------------------------|----------------------------------|---------------------------------|
| **Stack DB**                   | SQLAlchemy + psycopg2            | ORM nativo Fitz + driver puro   |
| **Líneas de código efectivas** | ~138 LoC (Fitz + Python)         | **~51 LoC** (Fitz only)         |
| **Archivos de proyecto**       | 6 (3 .fitz + 2 .py + reqs)       | **1** (1 .fitz)                 |
| **Dependencias runtime**       | SQLAlchemy + psycopg2 + libpython| **(ninguna)**                   |
| **Imagen Docker (regular)**    | ~250 MB (Python + deps)          | ~80 MB (debian-slim)            |
| **Imagen Docker (distroless)** | N/A                              | **~15 MB** (`gcr.io/distroless`)|
| **Build inicial**              | ~8-12 min                        | ~5-8 min                        |
| **Schema declarativo**         | `class User(Base): ...` Python   | `@table type User { ... }` Fitz |
| **Sync schema ↔ types**        | manual (o `fitz py-types`)       | **una sola definición**         |
| **SQL en runtime**             | construido por SQLAlchemy        | **constante en codegen-time**   |
| **Type safety end-to-end**     | parcial (SQLAlchemy → Fitz       | **completa** (un solo tipo)     |
|                                | requiere round-trip JSON)        |                                 |

(LoC efectivas = total - blanks - comments. Conteo total bruto:
`api-postgres-python` ~240 / `api-postgres-fitz` ~113.)

**Cuándo usar cuál**:

- **`api-postgres-fitz`** (este) — proyectos nuevos donde Postgres
  alcanza, querés mínimo overhead y deploy minimalista. Sin Python
  en el container.
- **`api-postgres-python`** — proyectos donde necesitás librerías
  Python específicas (numpy, pandas, scipy, ML), o querés migrar
  un código SQLAlchemy existente paso a paso.

## Cómo correr

### Setup (una vez)

```bash
cp .env.example .env
# editar .env si querés credenciales custom
```

### Desarrollo

```bash
docker compose up --build
```

El primer build compila `fitz` desde source (~5-8 min). Builds
subsiguientes solo recompilan tu código (~30s).

Esperá a ver en los logs:

```
[boot] DB conectada y schema inicializado
[ready] Server arrancando en :3000
```

### Probar con curl

```bash
# Crear users
curl -X POST localhost:3000/users \
     -H 'Content-Type: application/json' \
     -d '{"name":"Ada","email":"ada@example.com"}'

curl -X POST localhost:3000/users \
     -H 'Content-Type: application/json' \
     -d '{"name":"Alan","email":"alan@example.com"}'

# Listar
curl localhost:3000/users

# Get por id
curl localhost:3000/users/1
```

### Docs auto

OpenAPI 3.1 schema + UI Scalar embebida:

```bash
open http://localhost:3000/docs
```

## Estructura del proyecto

```
api-postgres-fitz/
├── src/
│   └── main.fitz              ← TODO el código del API
├── fitz.toml                  ← manifest del package
├── Dockerfile                 ← build standard (debian:bookworm-slim, ~80 MB)
├── Dockerfile.distroless      ← build minimal (gcr.io/distroless, ~15 MB)
├── docker-compose.yml         ← Postgres + api
├── .env.example               ← template de credenciales
├── .gitignore
└── README.md
```

Sin `python/`, sin `requirements.txt`, sin `data/`, sin `types/`.
El ORM nativo elimina la separación artificial entre "tipos del
HTTP" y "tipos del DB" — un solo `@table type User` declara
**ambos a la vez**.

## El código completo

Toda la API vive en `src/main.fitz` (~60 LoC con comments). El
patrón ORM es:

```fitz
@table("users") type User {
    @primary id: Int = 0
    name: Str
    email: Str
    created_at: Str = ""
}

@post("/users")
async fn create_user(body: NewUser) -> Result<User> {
    let conn = db.connect(DB_URL).await?
    return User.insert(conn, User {
        id: 0, name: body.name, email: body.email, created_at: ""
    }).await
}

@get("/users/{id}")
async fn get_user(id: Int) -> Result<User> {
    let conn = db.connect(DB_URL).await?
    return User.where(fn(u) => u.id == id).first(conn).await
}

@get("/users")
async fn list_users() -> Result<List<User>> {
    let conn = db.connect(DB_URL).await?
    return User.order_by(fn(u) => u.id).all(conn).await
}
```

El SQL que ejecuta es **constante en codegen-time** (el
`User.where(fn(u) => u.id == id)` se traduce al fragmento
`"id" = $1` durante el codegen, no en runtime). El `id: 0`
sentinel hace que Postgres asigne el id via `bigserial`. El
`@primary` + `bigserial PRIMARY KEY` están coordinados via
`init_schema()` que corre al boot.

## Distroless ultra-mini

Para deploy a prod donde el tamaño de imagen importa (Kubernetes,
serverless containers), usá `Dockerfile.distroless`:

```bash
docker build -f Dockerfile.distroless -t api-postgres-fitz:distroless .
docker images api-postgres-fitz:distroless
# REPOSITORY              TAG          SIZE
# api-postgres-fitz       distroless   ~15 MB
```

`gcr.io/distroless/cc-debian12` solo tiene glibc + ca-certificates +
tu binario. Sin shell, sin coreutils, sin package manager — superficie
de ataque mínima.

## Recursos

- [Cap 31 de la guía](https://thegreekman76.github.io/fitz/guide/#31-postgres--orm-nativo) — Postgres + ORM nativo.
- [Guía exhaustiva DB y ORM](https://thegreekman76.github.io/fitz/db-orm/) — referencia completa con todos los operadores.
- [Boilerplate side-by-side `api-postgres-python`](../api-postgres-python/) — mismo dominio con SQLAlchemy + interop Python.
- [Boilerplate `api-fullstack-postgres`](../api-fullstack-postgres/) — UI HTML + Tailwind sobre un CRUD similar.
