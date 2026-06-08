# C2 — Schema + workflow `fitz db` end-to-end

**Pre-requisitos**: [C1 — Setup Docker-first](c1-setup-docker-first.md)
cerrado. Tenés los 5 services arriba y `curl http://localhost:8000/healthz`
responde `200 OK`. Sabés el workflow `fitz db` del cap
[M6.C6 del curso](../curso/m6-postgres-orm/c6-migraciones-fitz-db.md)
— acá lo aplicamos contra TaskHub en un setup Docker-first real.

**Objetivo**: declarar el **schema del dominio** (los 4 `@table type`:
User / Project / Task / Comment), aplicar el **workflow completo
`fitz db`** (`new` → editar `@table` → `diff` → editar UP/DOWN →
`migrate` → `status`), verificar contra Postgres, demostrar un
**cambio de schema posterior** + `rollback`, y dejar el **drift
check `fitz db check` en GitHub Actions** corriendo en CI.

**Por qué importa**: es la **primera vez** en TaskHub que tocamos
la DB real. Marca el patrón para **todos los caps siguientes** —
cualquier cap que agregue una columna o cambie un type pasa por
`fitz db diff` + `migrate` + rebuild del binario. **El workflow
versionado** (vs `CREATE TABLE IF NOT EXISTS` al boot del M6.C7
capstone) es lo que hace que TaskHub sea **production-ready**: cada
cambio queda registrado, es auditable, y CI lo valida.

---

## Mapa del cap

```mermaid
flowchart LR
    A[Estado C1: 5 services] --> B[Declarar 4 @table types]
    B --> C[Agregar db.connect + GET /api/users smoke]
    C --> D[fitz db new initial_schema]
    D --> E[fitz db diff &gt; file.sql]
    E --> F[Editar secciones UP/DOWN]
    F --> G[fitz db migrate]
    G --> H[Verificar con psql]
    H --> I[docker compose up --build]
    I --> J[curl GET /api/users devuelve lista vacia]
    J --> K[Cambio schema: agregar field]
    K --> L[Segunda migration]
    L --> M[Rollback de la ultima]
    M --> N[CI con fitz db check + GitHub Actions]
```

---

## Por qué Fitz es distinto en TaskHub

| Workflow | Stack típico Alembic | Stack típico TypeORM | **TaskHub con `fitz db`** |
|---|---|---|---|
| Source of truth del schema | modelos SQLAlchemy + `env.py` configurado | clases `@Entity` + `DataSource` config | **`@table type` del lenguaje** en `src/main.fitz`, ya tipado y validado por el checker |
| Setup inicial del workflow | `pip install alembic` + `alembic init` + editar `env.py` para importar models | `npm install typeorm` + config DataSource + build TS | **Cero setup** — `fitz` ya está en el host del dev |
| Conexión a la DB del compose | usa SQLAlchemy engine config | usa TypeORM DataSource | usa `DATABASE_URL` directo (el dev exporta del `.env` del compose) |
| Generar el SQL del cambio | `alembic revision --autogenerate -m "add_field"` ⚠ requiere import de models al boot del CLI | `typeorm migration:generate` ⚠ requiere build TS | **`fitz db diff > file.sql`** directo desde el binario, lee el `@table` del fuente |
| Aplicar contra el container `db` | `alembic upgrade head` | `typeorm migration:run` | **`fitz db migrate`** |
| Rebuild del binario tras schema change | no aplica (runtime interpretation) | no aplica (runtime interpretation) | **`docker compose up -d --build app`** — el binario nuevo incorpora el schema declarado en `@table`, no hay runtime reflection |
| Drift check para CI | con scripts custom + `alembic check` (parcial) | con scripts custom | **`fitz db check` exit 0/1 directo** |
| Tipo de schema | objetos Python en runtime | clases TS compiladas | **types Fitz validados estáticos en compile-time** |

**Diferencial estructural**: el binario del container **no tiene
runtime reflection**. El schema que entiende el ORM es el que estaba
declarado en `@table` en **el momento del `fitz build`**. Por eso
después de cada `fitz db migrate` hace falta **rebuild del binario**
(`docker compose up -d --build app`). Es overhead manejable —
~30s — pero el binario resultante es 100% predecible. **Cero
sorpresas runtime**.

---

## Paso 1 — Declarar el schema en código

Editás `src/main.fitz` para declarar los 4 `@table type` del
dominio. En C2 los declaramos **sin `@belongs_to`** (esos llegan en
C4 cuando los usamos para navigation methods). Las FK constraints
van en SQL manual en las migrations.

```fitz
// src/main.fitz — TaskHub C2

// ───────────────────────────────────────────────
// Schema del dominio (4 tablas)
// ───────────────────────────────────────────────

@table("users") type User {
    @primary id: Int = 0
    email: Str
    password_hash: Str = ""
    role: Str = "member"      // admin / owner / member (validado en C3)
    created_at: DateTime
}

@table("projects") type Project {
    @primary id: Int = 0
    name: Str
    description: Str = ""
    owner_id: Int             // FK → users.id (constraint en migration)
    created_at: DateTime
}

@table("tasks") type Task {
    @primary id: Int = 0
    project_id: Int           // FK → projects.id
    title: Str
    description: Str = ""
    status: Str = "todo"      // todo / doing / done
    priority: Int = 3         // 1-5
    assignee_id: Int?         // FK nullable → users.id
    due_date: Date?
    ai_suggested_priority: Int?  // cache del C6
    created_at: DateTime
}

@table("comments") type Comment {
    @primary id: Int = 0
    task_id: Int              // FK → tasks.id
    user_id: Int              // FK → users.id (autor del comment)
    body: Str
    created_at: DateTime
}

// ───────────────────────────────────────────────
// Conexión a la DB + smoke endpoint
// ───────────────────────────────────────────────

let db_result = db.connect(env_or("DATABASE_URL", "")).await

type HealthResponse {
    status: Str
    version: Str
}

@get("/healthz")
fn healthz() -> HealthResponse {
    return HealthResponse {
        status: "ok",
        version: "0.1.0-c2"
    }
}

// Smoke endpoint para validar que el binario habla con la DB.
// En C4 lo vamos a transformar en un GET autenticado scoped al user.
@get("/users")
async fn list_users() -> Result<List<User>> {
    let conn: DbConn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }
    return User.all(conn).await
}

@server(8080)
fn main() => 0

print("TaskHub C2 — schema declarado, escuchando en :8080")
```

**Detalles importantes**:

- **`@primary id: Int = 0`** — sentinel `0` indica auto-asignación
  (`bigserial` PG). El ORM lo skipea en INSERT.
- **`created_at: DateTime`** sin default — vas a tener que pasar
  `DateTime.now()` cuando hagas INSERT en C4. (Alternativa:
  `@db_default("NOW()")` para que PG lo llene; lo dejamos como
  refinamiento futuro.)
- **`assignee_id: Int?`** — nullable. Postgres column `bigint NULL`.
- **`role: Str = "member"`** — default a `"member"`. Cuando registres
  usuarios sin especificar role, default a member.
- **`@get("/users")`** (no `/api/users`) porque nginx ya hace el
  strip del `/api/` prefix (ver `nginx.conf` del C1). El cliente
  pega a `http://localhost:8000/api/users`, nginx lo reenvía a
  `http://app:8080/users`.

---

## Paso 2 — Setup del `DATABASE_URL` en tu shell

`fitz db` se corre **desde el host** (no desde el container — el
runtime distroless no tiene shell). Necesita `DATABASE_URL`
apuntando a `localhost:5432` (el compose expone el puerto del db).

Creá un helper script `dev-env.sh` en la raíz del proyecto:

```bash
#!/usr/bin/env bash
# dev-env.sh — exporta vars necesarias para correr fitz db
# desde el host contra el container db del compose.
#
# Uso: `source dev-env.sh`

if [ ! -f .env ]; then
    echo "ERROR: .env no existe. Copialo desde .env.example primero."
    return 1 2>/dev/null || exit 1
fi

# Carga el .env del compose.
set -a
source .env
set +a

# Construye DATABASE_URL apuntando al db expuesto en localhost.
export DATABASE_URL="postgres://taskhub:${DB_PASSWORD}@localhost:5432/taskhub?sslmode=disable"

echo "✓ DATABASE_URL exportada para fitz db (localhost:5432)"
echo "  Ahora podés correr: fitz db status / diff / migrate / ..."
```

```bash
chmod +x dev-env.sh
source dev-env.sh
# → ✓ DATABASE_URL exportada para fitz db (localhost:5432)
```

Verificación rápida:

```bash
psql "$DATABASE_URL" -c "SELECT current_database();"
# → current_database
#   -----------------
#   taskhub
```

Si esto falla, el container `db` no está arriba o tu `.env` está
desconfigurado — revisá [C1 § Troubleshooting](c1-setup-docker-first.md#troubleshooting).

---

## Paso 3 — Primera migration: `fitz db new` → `diff` → editar → `migrate`

### 3.1 — `fitz db new initial_schema`

```bash
fitz db new initial_schema
# → ✓ migrations/20260607130000_initial_schema.sql
```

El stub generado tiene secciones `-- UP` y `-- DOWN` vacías:

```sql
-- Migration: initial_schema
-- Created: 2026-06-07T13:00:00Z

-- UP


-- DOWN

```

### 3.2 — `fitz db diff` para ver el SQL

```bash
fitz db diff
```

Output al stdout (todo el SQL para crear las 4 tablas desde una
DB vacía):

```sql
CREATE TABLE "users" (
    "id" bigserial PRIMARY KEY,
    "email" text NOT NULL,
    "password_hash" text NOT NULL DEFAULT '',
    "role" text NOT NULL DEFAULT 'member',
    "created_at" timestamp with time zone NOT NULL
);

CREATE TABLE "projects" (
    "id" bigserial PRIMARY KEY,
    "name" text NOT NULL,
    "description" text NOT NULL DEFAULT '',
    "owner_id" bigint NOT NULL,
    "created_at" timestamp with time zone NOT NULL
);

CREATE TABLE "tasks" (
    "id" bigserial PRIMARY KEY,
    "project_id" bigint NOT NULL,
    "title" text NOT NULL,
    "description" text NOT NULL DEFAULT '',
    "status" text NOT NULL DEFAULT 'todo',
    "priority" bigint NOT NULL DEFAULT 3,
    "assignee_id" bigint,
    "due_date" date,
    "ai_suggested_priority" bigint,
    "created_at" timestamp with time zone NOT NULL
);

CREATE TABLE "comments" (
    "id" bigserial PRIMARY KEY,
    "task_id" bigint NOT NULL,
    "user_id" bigint NOT NULL,
    "body" text NOT NULL,
    "created_at" timestamp with time zone NOT NULL
);
```

**Atajo útil**: redirigís a un archivo temporal en vez de
copiar/pegar:

```bash
fitz db diff > /tmp/initial.sql
```

### 3.3 — Editar el `.sql` con UP/DOWN + FK constraints

Editás `migrations/20260607130000_initial_schema.sql`:

- Pegás el SQL de `fitz db diff` adentro de la sección `-- UP`.
- **Agregás las FK constraints** (que el diff no emite porque los
  types no tienen `@belongs_to` todavía).
- Escribís el `-- DOWN` con DROP en orden inverso de creación.

```sql
-- Migration: initial_schema
-- Created: 2026-06-07T13:00:00Z

-- UP
CREATE TABLE "users" (
    "id" bigserial PRIMARY KEY,
    "email" text NOT NULL UNIQUE,
    "password_hash" text NOT NULL DEFAULT '',
    "role" text NOT NULL DEFAULT 'member',
    "created_at" timestamp with time zone NOT NULL
);

CREATE TABLE "projects" (
    "id" bigserial PRIMARY KEY,
    "name" text NOT NULL,
    "description" text NOT NULL DEFAULT '',
    "owner_id" bigint NOT NULL REFERENCES "users"("id") ON DELETE CASCADE,
    "created_at" timestamp with time zone NOT NULL
);

CREATE TABLE "tasks" (
    "id" bigserial PRIMARY KEY,
    "project_id" bigint NOT NULL REFERENCES "projects"("id") ON DELETE CASCADE,
    "title" text NOT NULL,
    "description" text NOT NULL DEFAULT '',
    "status" text NOT NULL DEFAULT 'todo',
    "priority" bigint NOT NULL DEFAULT 3,
    "assignee_id" bigint REFERENCES "users"("id") ON DELETE SET NULL,
    "due_date" date,
    "ai_suggested_priority" bigint,
    "created_at" timestamp with time zone NOT NULL
);

CREATE TABLE "comments" (
    "id" bigserial PRIMARY KEY,
    "task_id" bigint NOT NULL REFERENCES "tasks"("id") ON DELETE CASCADE,
    "user_id" bigint NOT NULL REFERENCES "users"("id") ON DELETE CASCADE,
    "body" text NOT NULL,
    "created_at" timestamp with time zone NOT NULL
);

-- Indexes recomendados para queries comunes.
CREATE INDEX "idx_projects_owner_id" ON "projects" ("owner_id");
CREATE INDEX "idx_tasks_project_id" ON "tasks" ("project_id");
CREATE INDEX "idx_tasks_assignee_id" ON "tasks" ("assignee_id");
CREATE INDEX "idx_comments_task_id" ON "comments" ("task_id");
CREATE INDEX "idx_comments_user_id" ON "comments" ("user_id");

-- DOWN
DROP TABLE "comments";
DROP TABLE "tasks";
DROP TABLE "projects";
DROP TABLE "users";
```

**Detalles** de las FK constraints:

- **`ON DELETE CASCADE`** en `projects.owner_id`, `tasks.project_id`,
  `comments.task_id`, `comments.user_id`: si borrás el padre, los
  hijos se borran automáticamente. Apropiado para data fuertemente
  acoplada.
- **`ON DELETE SET NULL`** en `tasks.assignee_id`: si borrás al
  usuario asignado, la task queda sin assignee (no se borra la
  task).
- **`UNIQUE`** en `users.email`: evitamos registros duplicados a
  nivel DB.

### 3.4 — `fitz db migrate`

```bash
fitz db migrate
# → ✓ 1 migration(s) aplicada(s):
#   - 20260607130000_initial_schema.sql
```

`fitz db status`:

```
Migration                                  Estado
-------------------------------------------- --------
20260607130000_initial_schema.sql           ✓ applied
```

---

## Paso 4 — Verificación con `psql`

```bash
psql "$DATABASE_URL" -c "\dt"
#                       List of relations
#  Schema |       Name        | Type  |  Owner
# --------+-------------------+-------+----------
#  public | _fitz_migrations  | table | taskhub
#  public | comments          | table | taskhub
#  public | projects          | table | taskhub
#  public | tasks             | table | taskhub
#  public | users             | table | taskhub

psql "$DATABASE_URL" -c "SELECT * FROM _fitz_migrations;"
#      version       |          applied_at
# -------------------+-------------------------------
#  20260607130000    | 2026-06-07 13:05:23.456789-00

# Detalle de columnas de `tasks`.
psql "$DATABASE_URL" -c "\d tasks"
```

Las 4 tablas + `_fitz_migrations` están creadas, los índices
existen, las FK están enlazadas. **Postgres está listo**. Falta
que el binario nuevo conozca el schema.

---

## Paso 5 — Rebuild del binario con el schema nuevo

El binario corriendo en `taskhub-app` es el de C1 — no conoce los
4 `@table type` que recién declaraste. Hay que rebuild:

```bash
docker compose up -d --build app
# → [+] Building 25s (12/12) FINISHED
# → [+] Running 1/1
#  ✔ Container taskhub-app  Started
```

`--build app` rebuilds solo el service `app` (no rebuild
postgres / prometheus / jaeger / nginx — esos no cambiaron).

Verificación: el nuevo binario respondiendo:

```bash
curl http://localhost:8000/healthz
# → {"status":"ok","version":"0.1.0-c2"}    ← version actualizada

curl http://localhost:8000/api/users
# → []                                       ← lista vacía, esperado
```

**El endpoint smoke `GET /api/users` devuelve `[]`** — significa
que el binario:

1. Levantó conn al `db:5432` (vía DATABASE_URL del compose).
2. Hizo `User.all(conn)` sin error.
3. Postgres respondió `0 rows` (la tabla existe y está vacía).
4. El ORM serializó la lista vacía a JSON.
5. Nginx proxeó la response al cliente.

**End-to-end OK** — el stack completo (cliente → nginx → app →
postgres) está vivo.

---

## Paso 6 — Cambio de schema posterior

Caso típico de TaskHub: querés sumar un campo `estimated_hours: Int = 0`
a `Task` para que los usuarios estimen cuánto les va a tomar.

Editás el `@table` en `src/main.fitz`:

```fitz
@table("tasks") type Task {
    @primary id: Int = 0
    project_id: Int
    title: Str
    description: Str = ""
    status: Str = "todo"
    priority: Int = 3
    estimated_hours: Int = 0          // ← campo nuevo
    assignee_id: Int?
    due_date: Date?
    ai_suggested_priority: Int?
    created_at: DateTime
}
```

`fitz db diff` ahora detecta el cambio:

```bash
fitz db diff
# → ALTER TABLE "tasks" ADD COLUMN "estimated_hours" bigint NOT NULL DEFAULT 0;
```

Generás migration + editás secciones + aplicás:

```bash
fitz db new add_estimated_hours_to_tasks
# → ✓ migrations/20260607140000_add_estimated_hours_to_tasks.sql

fitz db diff > /tmp/diff.sql
# Editás migrations/20260607140000_add_estimated_hours_to_tasks.sql
# pegando el SQL en -- UP, escribiendo el -- DOWN.
```

Contenido del archivo:

```sql
-- Migration: add_estimated_hours_to_tasks
-- Created: 2026-06-07T14:00:00Z

-- UP
ALTER TABLE "tasks" ADD COLUMN "estimated_hours" bigint NOT NULL DEFAULT 0;

-- DOWN
ALTER TABLE "tasks" DROP COLUMN "estimated_hours";
```

Aplicás + rebuild:

```bash
fitz db migrate
# → ✓ 1 migration(s) aplicada(s):
#   - 20260607140000_add_estimated_hours_to_tasks.sql

docker compose up -d --build app
```

`fitz db status` confirma las dos applied:

```
Migration                                                  Estado
---------------------------------------------------------- --------
20260607130000_initial_schema.sql                          ✓ applied
20260607140000_add_estimated_hours_to_tasks.sql            ✓ applied
```

`fitz db diff` re-corrido es no-op:

```bash
fitz db diff
# (sin output — schema sincronizado)
```

---

## Paso 7 — Rollback

Suponiendo que decidiste que `estimated_hours` no se va a usar:

```bash
fitz db rollback
# → ✓ rollback aplicado:
#   - 20260607140000_add_estimated_hours_to_tasks.sql
```

Verificás contra Postgres:

```bash
psql "$DATABASE_URL" -c "\d tasks" | grep estimated_hours
# (sin output — la columna desapareció)
```

**Atención**: tu `src/main.fitz` todavía declara `estimated_hours`
en el `@table` — el `fitz db check` (próximo paso) lo va a detectar
como drift. Después de un rollback tenés que:

- **Borrar el campo del `@table` en código** (consistencia con la
  DB), o
- **Re-aplicar la migration** (`fitz db migrate`) si fue un rollback
  temporal.

Para el cap, asumimos rollback intencional (cambiamos de idea):
borrás `estimated_hours: Int = 0` del `@table type Task` en
`src/main.fitz`. **Rebuild del binario:**

```bash
docker compose up -d --build app
```

---

## Paso 8 — `fitz db history`

Audit log de migrations aplicadas (orden `applied_at DESC`):

```bash
fitz db history
# version              applied_at                       filename
# -------------------- -------------------------------- ----------------------------
# 20260607130000       2026-06-07 13:05:23.456789-00    initial_schema.sql
# 1 migration(s) applied.
```

Solo `initial_schema` quedó aplicada (la otra fue rollback). Si más
adelante querés saber **qué cambió en producción y cuándo**, este
es el comando. **Útil cuando algo se rompe a las 3am** y necesitás
correlacionar con el último cambio de schema.

---

## Paso 9 — Drift check en CI con GitHub Actions

`fitz db check` corre el diff y devuelve:

- **exit 0** si el schema declarado matchea la DB.
- **exit 1** + SQL pendiente al stderr si hay drift.

Patrón típico en `.github/workflows/ci.yml`:

```yaml
name: ci
on:
  pull_request:
    branches: [main]

jobs:
  schema-drift:
    runs-on: ubuntu-latest
    services:
      db:
        image: postgres:16-alpine
        env:
          POSTGRES_USER: taskhub
          POSTGRES_PASSWORD: ci
          POSTGRES_DB: taskhub
        ports: ["5432:5432"]
        options: >-
          --health-cmd pg_isready
          --health-interval 5s
          --health-timeout 5s
          --health-retries 5

    env:
      DATABASE_URL: postgres://taskhub:ci@localhost:5432/taskhub?sslmode=disable

    steps:
      - uses: actions/checkout@v4

      - name: Install Fitz
        run: |
          curl -fsSL https://fitzlang.org/install.sh | sh
          echo "$HOME/.fitz/bin" >> $GITHUB_PATH

      - name: Apply migrations
        run: fitz db migrate

      - name: Schema drift check
        run: fitz db check
```

**Qué pasa si alguien edita `@table` pero olvida generar la
migration**:

- `fitz db migrate` aplica solo las migrations que existen (las
  viejas) — el schema queda **viejo** en la DB de CI.
- `fitz db check` compara `@table` en código vs DB → detecta drift.
- Exit 1 con el SQL pendiente → **el PR queda rojo**.

El dev ve el error en la PR antes del merge y corre `fitz db diff`
+ genera la migration que faltaba.

---

## Validación del cap

- [ ] `dev-env.sh` exporta `DATABASE_URL` correctamente.
- [ ] `psql "$DATABASE_URL" -c "\dt"` muestra las 4 tablas + `_fitz_migrations`.
- [ ] `fitz db status` muestra `initial_schema.sql` como `✓ applied`.
- [ ] `curl http://localhost:8000/api/users` devuelve `[]`.
- [ ] El campo `estimated_hours` se agregó, se rolled-back, y la
      tabla `tasks` quedó como al principio.
- [ ] `fitz db diff` post-rollback es no-op (schema sincronizado).
- [ ] `fitz db history` muestra solo `initial_schema.sql` applied.
- [ ] El workflow `.github/workflows/ci.yml` con `fitz db check`
      está commiteado.

---

## Troubleshooting

### `fitz db diff` devuelve SQL aunque ya migrate-é

Tu binario del container está **stale** (build del C1, sin los 4
`@table`). El `fitz db` corre desde el host con el `src/main.fitz`
**actual**, pero el binario del container tiene la versión vieja.
Esto es OK — `fitz db` compara `@table` del fuente vs DB, no el
binario del container.

Si el diff sigue mostrando algo después de `fitz db migrate`, es
drift real:

- ¿Editaste el `@table` después del último `migrate`? Generá nueva
  migration.
- ¿El SQL de la migration tiene un typo (col falta en la migration
  pero está en el `@table`)? Revisá el `.sql`.

### `fitz db migrate` falla con `connection refused`

Tu `DATABASE_URL` apunta a `localhost:5432` pero el container `db`
no está arriba o no expone el puerto.

```bash
# Verificá que está arriba.
docker compose ps db
# → taskhub-db  Up (healthy)  0.0.0.0:5432->5432/tcp

# Si el port no está expuesto, revisá tu docker-compose.yml — el
# service `db` tiene que tener `ports: ["5432:5432"]`.
```

### `curl GET /api/users` devuelve `500 Internal Server Error`

El binario no pudo conectar a la DB. Causas típicas:

- `DATABASE_URL` no resuelve el hostname `db` desde dentro del
  container del app (typo en compose).
- El service `db` no está `healthy` al momento que el app arranca
  (revisá `depends_on: db: condition: service_healthy`).
- Las tablas no existen (no corriste `fitz db migrate` antes del
  rebuild).

`docker compose logs app | tail -20` muestra el error específico
del binario.

### `fitz db rollback` aborta con `no tiene sección -- DOWN`

El archivo `.sql` de la migration no tiene `-- DOWN` (o está vacía).
Si la migration es genuinamente irreversible (ej. DROP de data
crítica), agregá un `-- DOWN` con `RAISE EXCEPTION 'migration X es
irreversible'` para documentarlo. Si querés revertir igual a mano:
editá la DB con `psql` directo y borrá el row de `_fitz_migrations`
manualmente.

### `docker compose up -d --build app` cuelga compilando

`fitz build` adentro del container hace `cargo build --release` —
la primera vez tarda 5-10 min porque baja todas las deps de Cargo
(axum, tokio, jsonwebtoken, argon2, etc). **Builds posteriores son
cache-friendly** (~30s).

Si querés iterar más rápido en desarrollo, podés correr el binario
con `fitz run src/main.fitz` desde el host (saltea el build de
Rust) pero **perdés las optimizaciones runtime** del binario nativo.
Es opcional — Docker-first sigue siendo lo recomendado para no
divergir del prod.

### El binario del container no encuentra la columna nueva

Olvidaste `docker compose up -d --build app` después del `fitz db
migrate`. El binario tiene el schema viejo hard-coded. **Solucion**:
rebuild.

---

## Lo que cubriste

- **Schema del dominio declarado** en 4 `@table type` (User,
  Project, Task, Comment) con sus tipos primitivos, nullables y
  defaults.
- **Setup del `DATABASE_URL`** en el shell del dev con un helper
  `dev-env.sh`.
- **Workflow canónico**: `new` → editar `@table` → `diff > /tmp/sql`
  → editar UP/DOWN + agregar FK constraints + indexes → `migrate`
  → `status`.
- **Verificación con `psql`** después de cada migration.
- **Rebuild del binario** con `docker compose up -d --build app`
  para que el binario incorpore el schema declarado.
- **Smoke endpoint `GET /api/users`** que prueba end-to-end:
  cliente → nginx → app → postgres.
- **Cambio de schema posterior** (ALTER TABLE ADD COLUMN) + rollback
  + re-aplicación.
- **Audit log con `fitz db history`**.
- **CI bloqueante con `fitz db check`** + GitHub Actions YAML.

**El schema de TaskHub está vivo**. El binario habla con la DB,
los 4 types existen como tablas con FK constraints y indexes, y
cualquier cambio futuro pasa por el workflow versionado.

---

## Próximo cap

**[C3 — Auth con RBAC custom: 3 roles apilables](c3-auth-rbac.md)**.

Vamos a sumar register + login con JWT + Argon2id, declarar el
`@auth_provider`, y aplicar `@authenticated` + `@requires("admin")` /
`@requires("owner")` / `@requires("member")` sobre los handlers.
Tests por cada rol verifican que el RBAC funciona — un member no
puede crear un project, un owner no puede ver projects de otros
owners. **El RBAC custom apilable es un diferencial fuerte vs
FastAPI/Express donde típicamente se implementa con middleware o
decoradores ad-hoc**.

Mientras tanto, **commiteá este cap**. Tu repo tiene los 4 types
del dominio + las migrations versionadas + CI con drift check.
Cualquier dev que clone el repo + `docker compose up -d --build` +
`source dev-env.sh && fitz db migrate` tiene el mismo estado.
