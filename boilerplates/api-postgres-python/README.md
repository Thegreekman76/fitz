# `api-postgres-python` — CRUD multi-archivo con SQLAlchemy + Postgres

Boilerplate del **stack realista** de un servicio CRUD HTTP:
- **API Fitz** multi-archivo (módulos en `types/` y `data/`),
  compilada con `--features python` para interop con CPython.
- **SQLAlchemy 2.x sync** + **psycopg2-binary** del lado Python
  (encapsulado en un solo módulo `db.py`).
- **Postgres 16** corriendo en otro container del docker-compose,
  con healthcheck + volume persistente.
- **Demostración explícita** de cómo se organiza un proyecto Fitz
  multi-archivo: tipos separados de la lógica de datos, ambos
  separados de los handlers HTTP.

```text
┌────────────────────────────┐       ┌────────────────────────────┐
│ Cliente HTTP (curl/Postman)│       │ Fitz API                   │
│                            │ ────► │  fitz run src/main.fitz    │
│ POST /users                │       │   @post /users             │
│ GET /users                 │       │   @get /users              │
│ GET /users/{id}            │       │   @get /users/{id}         │
└────────────────────────────┘       └─────────────┬──────────────┘
                                                   │ from python import db
                                                   ▼
                                     ┌────────────────────────────┐
                                     │ python/db.py               │
                                     │   SQLAlchemy + psycopg2    │
                                     └─────────────┬──────────────┘
                                                   │ TCP :5432
                                                   ▼
                                     ┌────────────────────────────┐
                                     │ Postgres 16                │
                                     │   docker container `db`    │
                                     │   volume: pgdata           │
                                     └────────────────────────────┘
```

## Qué demuestra

### Módulos Fitz multi-archivo

Estructura típica de proyectos Fitz reales: separación entre
**tipos** (lo que el HTTP/DB serializa) y **lógica de datos**
(las llamadas que mutan el estado). El `main.fitz` solo importa
fns tipadas, **no ve `from python import` directo** — el
wrapper de `data/users.fitz` encapsula la interop.

```
src/
├── main.fitz                    ← entry: @server, handlers HTTP
├── types/
│   ├── user.fitz                ← type User (mirror SQLAlchemy)
│   └── api.fitz                 ← type NewUser (body POST)
└── data/
    └── users.fitz               ← wrapper Fitz de db.py
```

Imports resueltos por el loader Fitz:
- `from types.user import User` → `src/types/user.fitz`.
- `from types.api import NewUser` → `src/types/api.fitz`.
- `from data.users import create, find, list_all, init_schema`
  → `src/data/users.fitz`.

### Interop Python end-to-end (Fase 8)

- `from python import db` + `from python import json` desde
  el wrapper `data/users.fitz`.
- Round-trip por JSON para coercer `dict` Python a `Instance`
  Fitz (patrón canónico mientras la coerción directa
  `dict → Instance` sobre PyAny es deuda residual).
- Excepciones SQLAlchemy (`NoResultFound`, errores de commit,
  etc.) wrapeadas automáticamente como `Result::Err` (Fase 8.3)
  → propagadas con `?` al handler HTTP → 500 con `{"error":
  "..."}` JSON.

### Postgres real con docker-compose

- Imagen oficial `postgres:16-alpine` con healthcheck.
- Credenciales via `.env` (template en `.env.example`).
- Volume nombrado `pgdata` — la data sobrevive `docker compose
  down`.
- `depends_on.db.condition: service_healthy` — el api espera
  que Postgres esté listo antes de arrancar (evita "Connection
  refused" en el primer query).

## Estructura del directorio

```
api-postgres-python/
├── README.md                   ← este archivo
├── fitz.toml                   ← manifest del package manager Fitz
├── src/
│   ├── main.fitz               ← handlers HTTP + init de DB
│   ├── types/
│   │   ├── user.fitz           ← type User (4 fields)
│   │   └── api.fitz            ← type NewUser (body POST)
│   └── data/
│       └── users.fitz          ← wrapper Fitz sobre python/db.py
├── python/
│   ├── models.py               ← SQLAlchemy declarative_base + User
│   └── db.py                   ← engine, session, CRUD helpers
├── requirements.txt            ← sqlalchemy, psycopg2-binary
├── Dockerfile                  ← multi-stage: rust:slim builder + python:slim runtime
├── docker-compose.yml          ← api + db (Postgres 16)
├── .env.example                ← POSTGRES_USER, POSTGRES_PASSWORD, POSTGRES_DB
├── .dockerignore
└── .gitignore
```

## Prerequisitos

**Solo Docker** con Compose v2. NO necesitás Fitz, Python, ni
Postgres instalados localmente — todo va adentro del compose.

```bash
docker --version            # 24+ recomendado
docker compose version      # v2 plugin
```

> **Build slow inicial**: a diferencia de los otros 4
> boilerplates que usan la imagen oficial
> `ghcr.io/thegreekman76/fitz:latest` como builder (~2-3 min),
> este boilerplate compila Fitz desde source con `--features
> python` adentro del Dockerfile (~8-12 min la primera vez).
> Razón: la imagen oficial NO trae la feature `python`
> activada (preserva la promesa "binario standalone sin libpython
> linkado" del binario default).
>
> Build subsiguientes son cacheados — si solo cambia `src/` o
> `python/`, el rebuild es de ~30s.

## Paso a paso

### 1. Setup de credenciales

```bash
cd boilerplates/api-postgres-python
cp .env.example .env
# editar .env si querés cambiar user/password/db (opcional para dev local)
```

Defaults del `.env.example`:
- `POSTGRES_USER=fitz`
- `POSTGRES_PASSWORD=fitz`
- `POSTGRES_DB=fitz`

Para **producción real**, cambialos por valores fuertes (no
commiteatear `.env` real).

### 2. Levantar todo

```bash
docker compose up --build
```

Primera vez: 8-12 min (compila fitz con `--features python`
desde source). Builds siguientes: ~30s si solo cambia el código
del proyecto.

Output esperado (resumido):

```text
db   | PostgreSQL init process complete; ready for start up.
db   | LOG:  database system is ready to accept connections
api  | Compiling fitz v0.1.0
api  | Finished `release` profile [optimized] target(s)
api  | [boot] DB conectada y schema inicializado
api  | 🏔️  Fitz HTTP escuchando en http://0.0.0.0:3000
api  |    POST /users
api  |    GET /users
api  |    GET /users/{id}
api  |    GET /openapi.json
api  |    GET /docs
api  | [ready] Server arrancando en :3000
```

### 3. Probar con curl

```bash
# Crear un user
curl -X POST localhost:3000/users \
     -H 'Content-Type: application/json' \
     -d '{"name":"Ada","email":"ada@example.com"}'
# → {"id":1,"name":"Ada","email":"ada@example.com","created_at":"2026-..."}

# Crear otro
curl -X POST localhost:3000/users \
     -H 'Content-Type: application/json' \
     -d '{"name":"Bob","email":"bob@example.com"}'
# → {"id":2,...}

# Listar todos (JSON crudo del lado Python)
curl localhost:3000/users
# → [{"id":1,...},{"id":2,...}]

# Buscar por id
curl localhost:3000/users/1
# → {"id":1,"name":"Ada","email":"ada@example.com","created_at":"..."}

# Buscar id que no existe (500 con detalle de SQLAlchemy)
curl -w "\nstatus: %{http_code}\n" localhost:3000/users/999
# → {"error":"NoResultFound: No row was found when one was required"}
#   status: 500
```

### 4. Conectarse a Postgres directo (opcional)

El docker-compose expone `:5432` al host para herramientas:

```bash
# psql del host (necesita postgres-client local)
PGPASSWORD=fitz psql -h localhost -U fitz -d fitz

# O via docker
docker compose exec db psql -U fitz -d fitz

# Adentro:
fitz=# SELECT * FROM users;
fitz=# \q
```

### 5. UI /docs

```text
http://localhost:3000/docs
```

UI Scalar con OpenAPI 3.1 auto-generado. Mostrá el shape `User`
y `NewUser` con sus fields tipados.

### 6. Parar

```bash
docker compose down       # mantiene la data en el volume
docker compose down -v    # borra TODO incluyendo el volume
```

## Cómo extender

### Agregar un campo nuevo a User

Cambiar 3 archivos en orden:

1. **`python/models.py`** — agregar la columna SQLAlchemy:
   ```python
   class User(Base):
       __tablename__ = "users"
       id = Column(Integer, primary_key=True)
       name = Column(String(120))
       email = Column(String(180), unique=True)
       role = Column(String(40), default="user")   # NUEVO
       ...
   ```
2. **`python/models.py::User.to_dict()`** — incluir el field en
   el dict serializado.
3. **`src/types/user.fitz`** — agregar el field al `type User`:
   ```fitz
   type User {
       id: Int = 0,
       name: Str = "",
       email: Str = "",
       role: Str = "user",   // NUEVO
       created_at: Str = "",
   }
   ```

(En proyectos serios: usar Alembic para migrar el schema. Para
boilerplate, `Base.metadata.create_all` solo crea tablas
nuevas — campos nuevos requieren `DROP TABLE` o migración manual.)

### Agregar un endpoint nuevo

Tres cambios:
1. **`python/db.py`** — sumar la fn helper (ej `update_user`).
2. **`src/data/users.fitz`** — wrapper Fitz tipado de esa fn.
3. **`src/main.fitz`** — handler `@put`/`@delete`/etc. que la usa.

### Async con asyncpg + bridge tokio↔asyncio

Hoy usamos SQLAlchemy sync + psycopg2. Para producción con
muchas conexiones concurrentes, lo más eficiente es SQLAlchemy
async + asyncpg, combinado con el bridge de Fase 8.6:

```fitz
fn create_async(name: Str, email: Str) -> Result<User> {
    let raw = db.add_user_async(name, email)?.await   // <py_call>?.await
    return from_py(raw)
}
```

Requiere reescribir `python/db.py` con `AsyncSession`. Deuda
del boilerplate — el sync cubre el caso típico hasta que
aparezca presión real de throughput.

## Variables de entorno

| Variable | Default | Uso |
|---|---|---|
| `POSTGRES_USER` | `fitz` | Usuario que Postgres crea al boot del container `db` |
| `POSTGRES_PASSWORD` | `fitz` | Idem |
| `POSTGRES_DB` | `fitz` | Nombre de la DB inicial |
| `DATABASE_URL` (interno) | construido del .env | Pasado al container `api`; `python/db.py` lo lee con `os.environ.get` |

El `docker-compose.yml` construye `DATABASE_URL` a partir de los
3 fields del `.env`, así no hay duplicación.

## Troubleshooting

### El `docker compose up --build` tarda muchísimo la primera vez

Es esperado (~8-12 min). El stage builder compila Fitz desde
source con `--features python` activada. Si querés ver el
progreso:

```bash
docker compose build api
# muestra el cargo install corriendo, con todos los crates que
# está compilando (pyo3, jsonwebtoken, axum, tokio, etc).
```

Una vez cacheado, los rebuilds que solo tocan `src/` o
`python/` son ~30s.

### El api dice "Connection refused" al conectarse a Postgres

Pasaba en versiones viejas sin el `healthcheck`. Si te aparece
ahora, verificá:
1. `docker compose ps` — el service `db` debe estar `Up
   (healthy)`. Si dice `Up (starting)` esperá unos segundos.
2. Los credenciales del `.env` matchean entre `db` y el
   `DATABASE_URL` del api (deberían — el yml los toma de la
   misma fuente).

### Error: "ImportError: No module named 'sqlalchemy'"

El `pip install` adentro del container falló. Verificá:
1. El stage builder del Dockerfile copió `requirements.txt`
   antes del `pip install`.
2. Tu `requirements.txt` tiene las versiones correctas
   (`sqlalchemy>=2.0,<3` y `psycopg2-binary>=2.9,<3`).
3. Internet del container OK durante el build.

### El POST devuelve 500 con `IntegrityError: duplicate key`

Estás intentando crear un user con un email que ya existe
(`email` tiene constraint `unique=True`). Cambiá el email o
borrá la DB:

```bash
docker compose down -v       # borra el volume
docker compose up --build    # rebuild + DB fresca
```

### `python/__pycache__/` aparece después del primer run

Es normal — CPython genera bytecode cache. Está en el
`.gitignore` y `.dockerignore` para no ensuciar el repo ni la
imagen.

### Mac M-series: `exec format error`

Multi-stage Dockerfile usa `rust:1.85-slim` y `python:3.12-slim`
que son multi-arch (ARM64 + x64). Debería andar nativo en M-series
sin tocar nada. Si aparece, agregá `platform: linux/amd64` al
service `api` del compose.

## Roadmap del boilerplate

- **Imagen `ghcr.io/thegreekman76/fitz:latest-python`**: cuando
  el `release.yml` publique también la variante con `--features
  python`, el Dockerfile se simplifica a `FROM
  ghcr.io/.../fitz:latest-python AS builder` y el build inicial
  baja a ~3 min (en lugar de 8-12).
- **`fitz build --bundle-python` (Fase 8.b cerrada 2026-05-23)** +
  **`fitz build --bundle-pip` (Fase 8.c cerrada 2026-05-23)**:
  el segundo flag empaqueta paquetes pip junto al CPython base.
  Este boilerplate puede simplificarse adoptándolo. El plan
  concreto (smoke local en Linux pendiente):

  Reemplazar el Dockerfile actual por algo así:

  ```dockerfile
  # Stage 1: builder con Python 3.14 (constraint del builder en
  # Linux por R.bug-pyo3-abi3-portable-link).
  FROM python:3.14-slim AS builder
  RUN apt-get update && apt-get install -y --no-install-recommends \
        curl ca-certificates git tar \
        python3-dev pkg-config libssl-dev build-essential && \
      rm -rf /var/lib/apt/lists/*
  RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
      sh -s -- -y --default-toolchain 1.95.0 --profile minimal
  ENV PATH="/root/.cargo/bin:${PATH}"
  RUN cargo install --git https://github.com/Thegreekman76/fitz \
      --tag v0.9.41 --features python --bin fitz --locked
  WORKDIR /app
  COPY fitz.toml ./
  COPY src/ ./src/
  COPY python/ ./python/
  COPY requirements.txt ./
  # Equivalente a --bundle-pip sqlalchemy --bundle-pip psycopg2-binary
  # pero leyendo del requirements.txt que ya existe en este
  # boilerplate (sigue siendo el mismo archivo que usa `pip install`
  # en el approach actual con `python:3.X-slim`). Cosecha de 8.c
  # cerrada en v0.9.42.
  RUN fitz build \
      --bundle-pip-requirements requirements.txt \
      src/main.fitz

  # Stage 2: runtime sin Python ni libpq ni pip.
  FROM gcr.io/distroless/cc-debian12
  COPY --from=builder /app/main /usr/local/bin/app
  ENV PYTHONPATH=/app/python
  EXPOSE 3000
  CMD ["/usr/local/bin/app"]
  ```

  **Beneficios**:
  - Imagen final: ~80-100 MB (CPython 30 MB + sqlalchemy +
    psycopg2-binary + binary) vs ~150 MB hoy con `python:3.12-slim`.
  - Sin `requirements.txt` en el runtime.
  - Sin `apt-get install libpq5` en el runtime (libpq viene
    adentro del wheel `psycopg2-binary`).
  - Sin `pip install` en el runtime.
  - Deploy con la imagen base más chica de la industria.

  **Constraint que sigue (heredado de `--bundle-python`)**: el
  builder en Linux/macOS requiere Python 3.14.x para que el
  linking de PyO3 coincida con el bundle PBS 3.14.5
  (R.bug-pyo3-abi3-portable-link componente Linux/macOS pendiente).
  En Windows el shim `python3.dll` evita este constraint.

  **Smoke real Docker pendiente**: el approach está validado en
  Windows con programas simples (`requests`). La validación
  end-to-end de `--bundle-pip sqlalchemy + psycopg2-binary`
  adentro de un Dockerfile multi-stage Linux es deuda nueva
  derivada de v0.9.41 — el primer user que lo pruebe confirma.
- **Auto-generar `types/user.fitz` con `fitz py-types`**: hoy
  está hardcoded. Sumar un step `fitz py-types python/models.py
  --out src/types/user.fitz` en el Dockerfile para regenerarlo
  cada build.
- **Async con `AsyncSession` + asyncpg + bridge 8.6**: para
  throughput alto.
- **Migraciones con Alembic**: hoy `Base.metadata.create_all`
  solo crea tablas nuevas — campos nuevos requieren migración
  manual o `DROP TABLE`.
- **DB nativa Fitz (Fase 10)**: reemplazo de la layer Python
  cuando llegue. Mismo API HTTP, sin interop.

## Siguientes pasos

- [`boilerplates/api-simple/`](../api-simple/) — REST sin DB
  (estado in-memory) para entender el patrón base.
- [`boilerplates/api-middleware-cors/`](../api-middleware-cors/)
  — sumar auth JWT + CORS encima de este CRUD.
- Cap 21 de la guía: detalle completo de interop Python con
  ejemplo CRUD SQLite (más simple para entender el round-trip
  por JSON antes de Postgres).
