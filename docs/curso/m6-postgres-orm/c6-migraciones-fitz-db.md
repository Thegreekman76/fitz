# M6.C6 — Migraciones con `fitz db`

**Pre-requisitos**: [M6.C5 — Tipos avanzados](c5-tipos-avanzados.md).
Tenés el ORM nativo cubierto — sabés declarar `@table type`,
hacer reads + writes, navigation y tipos avanzados (jsonb,
arrays, Date/DateTime/Uuid). Lo único que te falta para
producción es **versionar los cambios de schema**.

**Objetivo**: dominar el subcomando `fitz db` end-to-end —
crear migrations con `new`, generar SQL automático del diff
con `diff`, aplicar con `migrate`, ver el estado con `status`,
revertir con `rollback`, auditar con `history`, validar drift
en CI con `check`, adoptar DBs legacy con `stamp`/`inspect`,
escribir data migrations en `.fitz` nativas, y emitir SQL
offline para handoff a un DBA.

**Por qué importa**: hasta acá usaste el patrón `CREATE TABLE
IF NOT EXISTS` al boot (idempotente, cubre el 90% de los casos
de development). En producción real **necesitás un registro
ordenado de cada cambio de schema** — agregar un column,
renombrar otro, dropear un índice, hacer un backfill condicional.
Esto es lo que cubren Alembic, Flyway, typeorm migration,
Diesel CLI en sus ecosistemas. **Fitz lo trae built-in en el
binario**, sin instalar nada extra, con la fuente de verdad en
los mismos `@table type` que ya escribiste — el diff los compara
contra el schema real de Postgres y emite el SQL exacto.

**Cross-link**: [DB y ORM § 26.c — Migraciones automáticas](../../db-orm.md#26c-migraciones-automaticas-v01016).
Es la referencia exhaustiva del subcomando.

---

## Mapa del cap

```mermaid
flowchart LR
    A["@table type editado"] --> B[fitz db new]
    B --> C[migrations/file.sql vacío con UP/DOWN]
    A --> D[fitz db diff]
    D --> E[SQL ALTER TABLE al stdout]
    E --> F["redirige > file.sql"]
    F --> G[fitz db migrate]
    G --> H[tabla _fitz_migrations]
    H --> I[fitz db status applied/pending]
    G --> J[fitz db rollback]
    J --> K[ejecuta -- DOWN]
    I --> L[fitz db check exit 0/1 para CI]
    G --> M[fitz db history audit log]
    N[.fitz migrations] --> G
    O[DB legacy] --> P[fitz db inspect]
    P --> Q[fitz db stamp --all]
```

---

## Por qué Fitz es distinto

| Feature | Alembic (Python) | TypeORM CLI | Flyway (Java) | Diesel (Rust) | **Fitz** |
|---|---|---|---|---|---|
| Setup | `pip install alembic` + `alembic init` + edit `env.py` | `npm install typeorm` + config en `data-source.ts` | jar standalone + `flyway.conf` | `cargo install diesel_cli` + `diesel setup` | **`fitz db new` — sin setup** |
| Schema source of truth | modelos SQLAlchemy + script `env.py` que carga `Base.metadata` | clases `@Entity` + DataSource config | YAML/SQL files separados | macros `table!` + `schema.rs` regenerado | **`@table type` del lenguaje, ya tipado** |
| Auto-generate diff | `alembic revision --autogenerate` ⚠ requiere conn al boot | `typeorm migration:generate` ⚠ requiere build TS | ❌ no existe (manual SQL) | `diesel print-schema` + `diff` manual | **`fitz db diff` directo del binario** |
| Down migrations | `def downgrade()` Python | método `down()` TS | callbacks `R__` repeatable (no down clásico) | método `down()` Rust | **`-- DOWN` SQL section o `async fn rollback(db)` `.fitz`** |
| Drift check para CI | ⚠ con scripts custom + `alembic check` (parcial) | ⚠ con scripts custom | `flyway info` + parsing manual | ⚠ con scripts custom | **`fitz db check` — exit 0/1 directo** |
| Data migrations | Python adentro del `def upgrade()` | TS adentro del `up()` | SQL crudo o callback Java | Rust adentro del `up()` | **`.fitz` con `async fn migrate(db)` — full language** |
| Adoptar DB legacy | `alembic stamp head` | manual via tracking table | `flyway baseline` | manual | **`fitz db stamp --all` idempotente** |
| Squash migrations | ⚠ via scripts custom | ⚠ manual | ⚠ manual SQL concatenado | ⚠ manual | **`fitz db squash <from> <to>`** |
| Offline SQL (handoff a DBA) | ⚠ con `alembic upgrade --sql` (texto al stdout) | ❌ | ⚠ con `flyway migrate -dryRunOutput=file` | ❌ | **`fitz db migrate --sql`** |
| Introspect schema real | ❌ (separado: `pg_dump` o tools externas) | ❌ | ❌ | `diesel print-schema` | **`fitz db inspect [--table X] [--schema S]`** |
| Renames seguros sin perder data | ⚠ via `op.alter_column` con `name=`, manual | ⚠ via `renameColumn` en el `up()` | ⚠ SQL `ALTER ... RENAME` manual | ⚠ manual | **`@renamed_from("old")` decorator transient** |
| Quoted identifiers automáticos | ⚠ (config) | ⚠ (config) | ⚠ (config) | ⚠ (config) | ✅ siempre |

El diferencial mayor: la **fuente de verdad es el código tipado**
(`@table type User { ... }`), el diff conoce el shape real de
Postgres porque el driver es el mismo del ORM (wire protocol v3.0
+ `information_schema` + `pg_catalog`), y todo vive en el binario
`fitz` — **sin `pip install`, sin `cargo install`, sin `npm install`,
sin `flyway.jar`**.

---

## Paso 1 — Setup: proyecto + Postgres

Arrancamos con un proyecto chico que va a evolucionar a lo largo
del cap. La idea es que cada subcomando se vea en un cambio real
de schema, no en ejemplos sueltos.

```bash
mkdir -p ~/fitz-migrations-demo && cd ~/fitz-migrations-demo
fitz new .          # crea fitz.toml + src/main.fitz
mkdir migrations    # dir donde van las .sql / .fitz
```

`fitz.toml`:

```toml
[package]
name = "migrations-demo"
version = "0.1.0"
edition = "2026"

[bin]
main = "src/main.fitz"
```

`docker-compose.yml` minimal para Postgres local:

```yaml
services:
  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: secret
      POSTGRES_DB: demo
    ports:
      - "5432:5432"
    volumes:
      - pg_data:/var/lib/postgresql/data

volumes:
  pg_data:
```

```bash
docker compose up -d
export DATABASE_URL="postgres://postgres:secret@localhost:5432/demo?sslmode=disable"
```

`src/main.fitz` arranca con un `User` mínimo:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
}

fn main() => 0

print("Schema declarado.")
```

> **Tip**: `fitz db diff` lee `[bin].main` del manifest por
> default, así que no hace falta pasarle el archivo
> explícitamente. Si querés override, pasalo:
> `fitz db diff src/otro.fitz`.

---

## Paso 2 — Primera migration con `fitz db new` + `diff` + `migrate`

El workflow canónico tiene **cuatro pasos**: editás el `@table`,
generás el archivo vacío con `new`, completás el SQL con `diff`,
aplicás con `migrate`.

### Paso 2.1 — `fitz db new`

```bash
fitz db new initial_schema
# ✓ migrations/20260607120000_initial_schema.sql
```

El comando genera un timestamp en el filename (`YYYYMMDDHHMMSS`)
y un stub vacío:

```sql
-- Migration: initial_schema
-- Created: 2026-06-07T12:00:00Z

-- UP


-- DOWN

```

Las **secciones `-- UP` y `-- DOWN`** son por convención. El UP
es lo que `migrate` ejecuta; el DOWN es lo que `rollback` ejecuta
si querés revertir.

### Paso 2.2 — `fitz db diff`

En lugar de escribir el SQL a mano, `fitz db diff` introspecciona
la DB real, la compara contra los `@table type` del código, y
emite el SQL exacto:

```bash
fitz db diff
```

Output al stdout:

```sql
CREATE TABLE "users" (
    "id" bigserial PRIMARY KEY,
    "email" text NOT NULL
);
```

Lo redirigís al archivo vacío:

```bash
fitz db diff > migrations/20260607120000_initial_schema.sql
```

El archivo ahora tiene el SQL — pero **se perdieron las secciones
`-- UP` / `-- DOWN`** que generó `new`. En la práctica, editás el
archivo para poner el SQL adentro del `-- UP` y agregás el DOWN
correspondiente a mano:

```sql
-- Migration: initial_schema
-- Created: 2026-06-07T12:00:00Z

-- UP
CREATE TABLE "users" (
    "id" bigserial PRIMARY KEY,
    "email" text NOT NULL
);

-- DOWN
DROP TABLE "users";
```

> **Por qué dos pasos separados**: `new` te da el esqueleto
> ordenado (timestamp + UP/DOWN); `diff` te da el SQL del cambio.
> Mantenerlos separados deja control sobre **qué entra en cada
> migration** — un sub-comando único (`diff --new`) sería más
> opaco cuando hacés varios cambios y querés repartirlos en
> migrations independientes.

### Paso 2.3 — `fitz db migrate`

```bash
fitz db migrate
# ✓ 1 migration(s) aplicada(s):
#   - 20260607120000_initial_schema.sql
```

Bajo el capó: `fitz db migrate` crea la tabla `_fitz_migrations`
si no existe (es el **tracking idempotente**), aplica cada `.sql`
pendiente del dir adentro de una transacción, e inserta el row
correspondiente.

Verificás contra Postgres:

```bash
psql $DATABASE_URL -c "\dt"
#                List of relations
#  Schema |       Name        | Type  |  Owner
# --------+-------------------+-------+----------
#  public | _fitz_migrations  | table | postgres
#  public | users             | table | postgres

psql $DATABASE_URL -c "SELECT * FROM _fitz_migrations"
#      version       |          applied_at
# -------------------+-------------------------------
#  20260607120000    | 2026-06-07 12:00:23.456789-00
```

> **Idempotencia**: re-correr `fitz db migrate` con todo aplicado
> es no-op (`✓ todas las migrations ya aplicadas`). Esto es lo
> que hace seguro arrancar el server con `fitz db migrate &&
> ./mi-app`.

### Paso 2.4 — `fitz db status`

```bash
fitz db status
# Migration                                              Estado
# ----------------------------------------------------- --------
# 20260607120000_initial_schema.sql                     ✓ applied
```

`status` cruza el dir `migrations/` con la tabla `_fitz_migrations`
y muestra cada archivo con su badge. Si agregás un archivo nuevo
sin aplicar, sale como `→ PENDING`. Si removés un archivo que ya
estaba applied, sale como `applied (file removido)` — útil para
detectar que alguien borró un archivo a mano.

---

## Paso 3 — Cambio de schema + segunda migration

Ahora demostramos el flujo real: **cambiás el `@table`, generás
una migration nueva, la aplicás**.

Editás `src/main.fitz` para sumar dos columns:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    name: Str = ""
    email_verified: Bool = false
}
```

Comparás contra la DB:

```bash
fitz db diff
```

Output al stdout:

```sql
ALTER TABLE "users" ADD COLUMN "name" text NOT NULL DEFAULT '';
ALTER TABLE "users" ADD COLUMN "email_verified" boolean NOT NULL DEFAULT false;
```

Generás archivo + redirigís + editás secciones:

```bash
fitz db new add_name_and_verified_to_users
# ✓ migrations/20260607123000_add_name_and_verified_to_users.sql
fitz db diff > migrations/20260607123000_add_name_and_verified_to_users.sql
```

Editás para mantener UP/DOWN:

```sql
-- Migration: add_name_and_verified_to_users
-- Created: 2026-06-07T12:30:00Z

-- UP
ALTER TABLE "users" ADD COLUMN "name" text NOT NULL DEFAULT '';
ALTER TABLE "users" ADD COLUMN "email_verified" boolean NOT NULL DEFAULT false;

-- DOWN
ALTER TABLE "users" DROP COLUMN "email_verified";
ALTER TABLE "users" DROP COLUMN "name";
```

Aplicás:

```bash
fitz db migrate
# ✓ 1 migration(s) aplicada(s):
#   - 20260607123000_add_name_and_verified_to_users.sql
```

`fitz db status`:

```
Migration                                                      Estado
-------------------------------------------------------------- --------
20260607120000_initial_schema.sql                              ✓ applied
20260607123000_add_name_and_verified_to_users.sql              ✓ applied
```

**Idempotencia del diff**: re-correr `fitz db diff` ahora emite
SQL vacío — el schema declarado matchea el real:

```bash
fitz db diff
# (sin output — schema sincronizado)
```

---

## Paso 4 — `fitz db rollback`

Suponiendo que el cambio de `email_verified` rompió algo en
producción, querés revertirlo:

```bash
fitz db rollback
# ✓ rollback aplicado:
#   - 20260607123000_add_name_and_verified_to_users.sql
```

El comando ejecuta la sección `-- DOWN` adentro de una tx y borra
el row de `_fitz_migrations`. Verificás:

```bash
psql $DATABASE_URL -c "\d users"
#                              Table "public.users"
#  Column |  Type  | Collation | Nullable |              Default
# --------+--------+-----------+----------+-----------------------------------
#  id     | bigint |           | not null | nextval('users_id_seq'::regclass)
#  email  | text   |           | not null |
```

El `name` y `email_verified` desaparecieron. `fitz db status`:

```
Migration                                                      Estado
-------------------------------------------------------------- --------
20260607120000_initial_schema.sql                              ✓ applied
20260607123000_add_name_and_verified_to_users.sql              → PENDING
```

### Rollback de varias migrations

`fitz db rollback --count N` revierte las últimas N aplicadas.
**Atomicidad**: cada migration corre adentro de su propia tx —
si la k-ésima falla en runtime, las anteriores ya persistieron.
Para "todo o nada" sobre N migrations, escribí UNA migration
única con todo el rollback adentro.

### Migrations sin DOWN

Si una `.sql` no tiene sección `-- DOWN` (o está vacía), el
rollback **aborta pre-flight** con mensaje claro citando el
filename — cero estado parcial. El stub que genera
`fitz db new` siempre incluye `-- DOWN` por convención, pero
podés borrarla deliberadamente si la migration es genuinamente
irreversible (típico: `DROP TABLE` con data crítica).

> **Re-aplicación**: después de un rollback, podés re-aplicar
> con `fitz db migrate` normalmente. Útil para iterar localmente
> ("aplico, pruebo, rollback, edito, re-aplico").

---

## Paso 5 — `fitz db history` (audit log)

Mientras `status` muestra el estado actual (qué archivo está
applied o pending), `history` muestra el **audit log cronológico**
con timestamps:

```bash
# Volvés a aplicar primero
fitz db migrate

fitz db history
# version              applied_at                       filename
# -------------------- -------------------------------- ----------------------------
# 20260607123000       2026-06-07 12:35:12.345678+00    add_name_and_verified_to_users.sql
# 20260607120000       2026-06-07 12:00:23.456789+00    initial_schema.sql
# 2 migration(s) applied.
```

Orden: `applied_at DESC` (más recientes primero). Cruza el
tracking con los archivos del dir; si una version está applied
pero el archivo fue removido (caso post-squash, ver Paso 8),
aparece como `(file removido)`.

**Útil para**: investigar cuándo se rompió algo en prod,
auditoría de cambios de schema, debug de migrations que aplicaron
en orden inesperado.

---

## Paso 6 — `fitz db check` para CI

`fitz db check` es el **drift detector**: corre el diff y devuelve
**exit 0** si el schema declarado matchea la DB, **exit 1** con
el SQL pendiente al stderr si hay drift. Hook clave para CI
bloqueante — *"no merge si el schema del código no matchea la DB
de staging"*.

```bash
fitz db check
# ✓ schema sincronizado — schema declarado matchea la DB
# (exit 0)
```

Si editás `@table` agregando un field sin generar la migration:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    name: Str = ""
    email_verified: Bool = false
    @column phone: Str?
}
```

```bash
fitz db check
# ✗ drift detectado — 1 change(s) pendiente(s):
#
# ALTER TABLE "users" ADD COLUMN "phone" text;
#
# 💡 corré `fitz db diff > migrations/<file>.sql` + `fitz db migrate`
# (exit 1)
```

### En CI (GitHub Actions)

```yaml
name: schema-drift
on:
  pull_request:
jobs:
  drift-check:
    runs-on: ubuntu-latest
    services:
      db:
        image: postgres:16-alpine
        env:
          POSTGRES_PASSWORD: secret
          POSTGRES_DB: ci
        ports: ["5432:5432"]
        options: >-
          --health-cmd pg_isready
          --health-interval 5s
          --health-timeout 5s
          --health-retries 5
    steps:
      - uses: actions/checkout@v4
      - run: |
          curl -fsSL https://fitzlang.org/install.sh | sh
          fitz db migrate
          fitz db check
        env:
          DATABASE_URL: postgres://postgres:secret@localhost:5432/ci
```

Si alguien edita un `@table` pero olvida generar la migration, el
PR queda **rojo** con el SQL exacto en el log.

---

## Paso 7 — Migrations nativas en `.fitz`

Hasta acá vimos `.sql` puro — DDL/DML cruda. Para **transforms
condicionales** (backfills, parseo de JSON viejo, calls a un
service externo), Fitz acepta migrations en `.fitz` con `async
fn migrate(db)` adentro.

Caso típico: agregamos un campo `full_name` derivado del `email`
con un backfill condicional para users existentes.

`migrations/20260607130000_backfill_full_name.fitz`:

```fitz
// migrations/20260607130000_backfill_full_name.fitz

async fn migrate(db: DbConn) -> Result<Null> {
    // 1. Agregamos la columna nullable primero.
    let _ = db.exec(
        "ALTER TABLE users ADD COLUMN full_name text",
        []
    ).await?

    // 2. Backfill: para cada user sin full_name, derivamos
    //    del email. Los typed accessors (`get_int`/`get_str`)
    //    devuelven `Result<T>` — usás `?` para propagar.
    match db.query("SELECT id, email FROM users WHERE full_name IS NULL", []).await {
        Ok(rows) => {
            for r in rows {
                let id: Int = r.get_int("id")?
                let email: Str = r.get_str("email")?
                let _ = db.exec(
                    "UPDATE users SET full_name = $1 WHERE id = $2",
                    [email, id],
                ).await?
            }
            return Ok(null)
        }
        Err(e) => return Err(e),
    }
}

async fn rollback(db: DbConn) -> Result<Null> {
    let _ = db.exec("ALTER TABLE users DROP COLUMN full_name", []).await?
    return Ok(null)
}
```

```bash
fitz db migrate
# ✓ 1 migration(s) aplicada(s):
#   - 20260607130000_backfill_full_name.fitz
```

**Cuándo usar `.fitz` vs `.sql`**:

- **`.sql`** — DDL puro (CREATE TABLE / ADD COLUMN / CREATE INDEX),
  backfills triviales (`UPDATE x SET col = ... WHERE ...`),
  fixtures. **80% de las migrations**.
- **`.fitz`** — backfills con lógica condicional o loops, parseo
  de JSON viejo a columns nuevas, HTTP calls a un service externo,
  transforms que requieren state.

**Política**:

- Los archivos `.sql` y `.fitz` se mezclan en el dir y se ordenan
  por timestamp del filename — `migrate` aplica todo en orden
  cronológico.
- `db` viene **pre-bindeado** al env del script (no requiere
  `db.connect(...)` adentro — el CLI ya conectó por vos).
- El env tiene los builtins normales (`print`, `len`, `env_or`,
  `jwt`, `hash`, etc.).
- Atomicidad NO automática: si querés "todo o nada", envolvé en
  `db.transaction(fn(tx) -> Result<Null> { ... }).await` adentro
  de `migrate`.

> **Caveat**: las `.fitz` migrations corren via **intérprete**
> (no codegen), igual que `fitz run`. Para migrations grandes
> con miles de iteraciones, considerá hacer el bulk via 1
> UPDATE SQL en una `.sql` separada en lugar de loop iterativo
> en `.fitz`.

---

## Paso 8 — `fitz db inspect` + `stamp` para adoptar DBs legacy

Caso típico: tu equipo tiene una DB Postgres existente (creada
hace años con Alembic o a mano) y querés empezar a usar Fitz.
**No querés re-crear las tablas** — ya están. Necesitás:

1. **Saber qué hay**: `fitz db inspect`.
2. **Generar un `@table type` que matchee**: o lo escribís a
   mano o lo derivás del output del `inspect`.
3. **Crear una migration que matchea el schema actual**: para
   que `diff` no proponga cambios.
4. **Marcar esa migration como aplicada SIN ejecutarla**:
   `fitz db stamp`.

### `fitz db inspect`

```bash
fitz db inspect
# Tables en schema "public":
#
# - users
#   - id              bigint NOT NULL    DEFAULT nextval('users_id_seq'::regclass)
#   - email           text NOT NULL
#   - created_at      timestamp with time zone NOT NULL    DEFAULT now()
#   - PRIMARY KEY (id)
#   - UNIQUE (email)
#   - INDEX users_email_idx ON (email)
#
# - posts
#   - id              bigint NOT NULL    DEFAULT nextval('posts_id_seq'::regclass)
#   - user_id         bigint NOT NULL
#   - title           text NOT NULL
#   - body            text NOT NULL
#   - PRIMARY KEY (id)
#   - FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
```

Flags útiles:

- `fitz db inspect --table users` — filtra una sola tabla.
- `fitz db inspect --schema tenant_a` — filtra por schema
  (default `public`).
- `fitz db inspect --all-schemas` — lista TODOS los schemas
  user-defined (excluye `pg_catalog`, `information_schema`).
- `fitz db inspect --json` — output machine-readable.
- `fitz db inspect --url postgres://prod-host/...` — apunta a
  otra DB sin tocar `DATABASE_URL`.

### `fitz db stamp` para "adoptar"

Una vez escribís los `@table` que matchean el schema actual,
generás la migration inicial sin aplicarla:

```bash
# 1. Generás migration con el shape actual.
fitz db diff > migrations/20260530000000_initial.sql

# 2. Marcás como aplicada SIN ejecutar (la DB ya tiene las tables).
fitz db stamp 20260530000000
# ✓ stamped: 20260530000000

# 3. A partir de acá, `fitz db migrate` aplica solo las nuevas.
fitz db migrate
# ✓ todas las migrations ya aplicadas
```

**`fitz db stamp --all`** marca todas las pending del dir como
aplicadas en una sola pasada. Útil cuando tenés varias migrations
legacy ya aplicadas manualmente.

**Idempotencia**: stamp sobre una version ya applied → no-op
silencioso (`✓ no-op: version X ya estaba aplicada`).

---

## Paso 9 — `fitz db migrate --sql` para handoff a DBA

En equipos con separación de roles, el dev genera migrations pero
**el DBA aplica el SQL a producción** (con sus propias herramientas
de auditoría / windows de mantenimiento / dry-runs). Para esto
existe `--sql`:

```bash
fitz db migrate --sql > pending.sql
# ✓ 1 migration emitida — pasalo al DBA.
```

El output es el SQL de TODAS las migrations pendientes,
concatenadas en orden cronológico:

```sql
-- migrations/20260607140000_add_avatar_url.sql
ALTER TABLE "users" ADD COLUMN "avatar_url" text;
```

El DBA aplica con `psql` y después vos marcás como aplicadas:

```bash
psql -h prod -f pending.sql
fitz db stamp 20260607140000
```

**Caveat**: rechaza `.fitz` data migrations en el rango — no se
materializan como SQL offline. Si tu pending tiene `.fitz`, el
comando aborta con mensaje claro: "el DBA no puede correr esto,
tenés que aplicarlo vos con `fitz db migrate`".

---

## Paso 10 — `fitz db squash` para limpieza histórica

Repos con años de uso acumulan cientos de migrations viejas. Para
**bootstrap de devs nuevos** o **CI más rápido**, podés squashear
un rango histórico en una sola migration consolidada:

```bash
fitz db squash 20240101000000 20251231000000
# ✓ tracking actualizado: 47 versions removidas, stamped `20240101000000`
# ✓ 47 migration(s) squashed → migrations/20240101000000_squashed.sql
#   Originales en migrations/squashed/.
```

**Política**:

- Concatena los `-- UP` en orden + los `-- DOWN` en orden inverso
  (para que el rollback siga funcionando).
- Mueve los archivos originales a `migrations/squashed/` (no los
  borra — quedan como histórico auditable).
- Si alguna del rango estaba applied en la DB, **borra** todas
  del rango de `_fitz_migrations` y **stampea** solo `from` (el
  nuevo squashed).
- Solo `.sql` — rechaza `.fitz` en el rango con mensaje claro
  (squashing de scripts del lenguaje no es semánticamente
  trivial).

> **Cuándo usar**: cuando el dir `migrations/` tiene >50 archivos
> viejos que ya están applied en todos los environments. NO uses
> para migrations recientes — perderías granularidad de rollback.

---

## Paso 11 — Renames seguros con `@renamed_from`

Si renombrás un field Fitz-side sin más, el diff lo ve como
`DROP COLUMN <viejo> + ADD COLUMN <nuevo>` — **perdés los datos**.
Para renames seguros existe el decorator `@renamed_from`:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    @renamed_from("name") full_name: Str = ""    // ← rename seguro
}
```

`fitz db diff` emite:

```sql
ALTER TABLE "users" RENAME COLUMN "name" TO "full_name";
```

Lo aplicás con `migrate` normalmente y después **borrás el
decorator** (es transient — vive solo el tiempo de la migration).
El diff lo ignora silenciosamente cuando ya no hay match en
current (la migration ya se aplicó).

Para renombrar la tabla entera:

```fitz
@table("users") @renamed_from("legacy_users") type User {
    @primary id: Int = 0
}
```

Output:

```sql
ALTER TABLE "legacy_users" RENAME TO "users";
```

**Por qué decorator y no subcomando** (`fitz db rename`): el
subcomando divorcia el rename del cambio en el código (fácil de
olvidar uno o el otro). El decorator es declarativo, vive
temporalmente en el código, atómico con el cambio del nombre.
Después de aplicar, borrás una línea — equivalente a cerrar un
PR.

---

## Paso 12 — Qué NO está en el MVP

Honestidad sobre los gaps conocidos del subcomando (v0.13.2):

- **Solo schema `public` por default**: para schemas custom,
  pasás `--schema mi_schema` a `inspect`. El `diff` y `migrate`
  trabajan sobre `public` salvo que el `@table` use sintaxis
  `@table("schema.tabla")`.
- **`ALTER COLUMN ... TYPE` sin USING**: cambios de tipo
  incompatibles (`text → int`) fallan. Editás la migration a mano
  para agregar `USING (col::int)` o split en data migration
  separada.
- **Rollback de N>1 NO es atómico**: cada migration corre en su
  propia tx. Para "todo o nada", escribí una migration única con
  todo el rollback adentro.
- **`stamp` de version inexistente emite warning pero inserta
  igual**: deliberado (patrón "adopto una version legacy que
  NUNCA voy a tener como file"). El warning evita typos
  accidentales.
- **Squash solo de `.sql`**: rechaza `.fitz` en el rango.

Detalle completo en [DB y ORM § 26.c — Limitaciones explícitas del MVP](../../db-orm.md#limitaciones-explicitas-del-mvp).

---

## Cheat sheet

| Subcomando | Qué hace |
|---|---|
| `fitz db new <name>` | Crea `migrations/YYYYMMDDHHMMSS_<name>.sql` con stub UP/DOWN |
| `fitz db diff [archivo.fitz]` | Compara schema declarado vs real, emite SQL al stdout |
| `fitz db diff --out file.sql` | Igual que diff, pero escribe a archivo |
| `fitz db migrate` | Aplica `.sql` y `.fitz` pendientes en orden cronológico |
| `fitz db migrate --dry-run` | Muestra qué se aplicaría sin tocar la DB |
| `fitz db migrate --sql` | Emite SQL pendiente al stdout (handoff DBA) |
| `fitz db status` | Lista cada archivo con `✓ applied` / `→ PENDING` |
| `fitz db rollback` | Revierte la última applied (ejecuta `-- DOWN`) |
| `fitz db rollback --count N` | Revierte las últimas N |
| `fitz db history` | Audit log `applied_at DESC` |
| `fitz db check` | Drift check — exit 0/1 para CI |
| `fitz db stamp <version>` | Marca version como applied SIN ejecutar |
| `fitz db stamp --all` | Marca todas las pending como applied |
| `fitz db squash <from> <to>` | Combina rango en una migration consolidada |
| `fitz db inspect` | Introspect del schema real |
| `fitz db inspect --table X` | Filtra una tabla |
| `fitz db inspect --schema S` | Filtra por schema (default `public`) |
| `fitz db inspect --all-schemas` | Lista TODOS los schemas user-defined |
| `fitz db inspect --json` | Output machine-readable |

**Flags globales del paquete `db`**:

- `--url postgres://...` — override de `DATABASE_URL`.
- `--dir migrations/otro` — override del dir default `./migrations`.

---

## Validación del cap

Si todo lo de arriba funciona, deberías poder verificar:

- [ ] `fitz db new initial_schema` crea archivo con timestamp +
      stub UP/DOWN.
- [ ] `fitz db diff` con `@table` editado emite el SQL correcto.
- [ ] `fitz db migrate` aplica y la tabla `_fitz_migrations`
      tiene los rows.
- [ ] `fitz db status` muestra `✓ applied` después del migrate.
- [ ] `fitz db rollback` ejecuta el `-- DOWN` y borra del
      tracking.
- [ ] Re-corres `fitz db migrate` y vuelve aplicar bit-a-bit.
- [ ] `fitz db history` muestra cada migration con timestamp.
- [ ] `fitz db check` devuelve exit 0 cuando matchea, exit 1
      cuando hay drift.
- [ ] Una `.fitz` migration con `async fn migrate(db)` aplica
      junto con las `.sql`.
- [ ] `fitz db inspect` lista las tablas de la DB real.
- [ ] `fitz db stamp 20260607120000` marca sin ejecutar.

Cubrís todo eso en el ejemplo runnable en
[`examples/curso/m6-postgres-orm/c6-migrations/`](https://github.com/Thegreekman76/fitz/tree/main/examples/curso/m6-postgres-orm/c6-migrations).

---

## Troubleshooting

### `Err("connection refused")` al correr `fitz db diff`

`DATABASE_URL` no está exportada o Postgres no está corriendo:

```bash
docker compose ps                 # verificá que `db` está up
echo $DATABASE_URL                # verificá la env var
```

### `fitz db diff` emite SQL que ya está en la DB

Re-applied silently sería destructivo. La causa típica es que
`_fitz_migrations` tiene state stale. Verificá con `fitz db
status` qué dice del archivo. Si el archivo está marcado como
PENDING pero la tabla ya existe, usá `fitz db stamp <version>`
para sincronizar el tracking.

### `fitz db rollback` aborta con "no tiene sección `-- DOWN`"

El archivo `.sql` no tiene `-- DOWN` (o está vacía / solo
whitespace). Si la migration es genuinamente irreversible,
agregá un `-- DOWN` con `RAISE EXCEPTION 'migration X es
irreversible'` para documentarlo. Si querés revertir igual a
mano, editá la DB con `psql` directo (sin pasar por `rollback`)
y después borrá el row de `_fitz_migrations` manualmente.

### Una `.fitz` migration corre bien pero `migrate` no la registra

Verificá que la función declarada es **exactamente** `async fn
migrate(db: DbConn) -> Result<Null>` (con la signature completa
y el nombre `migrate`). Si la nombrás distinto (ej. `apply`,
`up`), el runner no la detecta y aborta antes de aplicar.

### `fitz db check` falla en CI por "no DATABASE_URL"

Asegurate de que el job tiene el service `postgres` con
healthcheck y que el step exporta `DATABASE_URL` apuntando a
`localhost:5432` (o al hostname del service en `services:` —
`db:5432` si seguís el patrón del ejemplo).

### `fitz db squash` rechaza por `.fitz` en el rango

Por diseño — no podemos concatenar `.sql` con `.fitz` automático
de forma semánticamente segura. Workaround: squashear los `.sql`
del rango sin tocar las `.fitz`, dejando estas últimas
intercaladas como están.

---

## Lo que cubriste

Llegaste al final del cap. Lo que cubriste:

- **Workflow canónico**: `new` → editar `@table` → `diff > file.sql`
  → `migrate` → `status`.
- **Rollback**: `-- DOWN` SQL section + `fitz db rollback [--count N]`.
- **Audit**: `fitz db history` con `applied_at DESC`.
- **CI**: `fitz db check` con exit 0/1 + ejemplo GitHub Actions.
- **Data migrations**: `.fitz` con `async fn migrate(db)` para
  transforms condicionales.
- **DB legacy**: `fitz db inspect` para introspect + `stamp` /
  `stamp --all` para adoptar sin re-ejecutar.
- **Handoff a DBA**: `fitz db migrate --sql` para offline SQL.
- **Limpieza histórica**: `fitz db squash <from> <to>`.
- **Renames seguros**: `@renamed_from("old")` decorator transient.
- **Defensa contra drift**: `fitz db check` bloqueando PRs en CI.

### Cómo se compara con el ecosistema

| | Alembic | TypeORM | Flyway | Diesel | **Fitz** |
|---|---|---|---|---|---|
| Setup | `pip install alembic` + edit `env.py` | `npm install typeorm` + DataSource config | `flyway.jar` + `flyway.conf` | `cargo install diesel_cli` + `diesel setup` | **`fitz db new` — sin setup** |
| Source of truth | modelos SQLAlchemy + `env.py` | clases `@Entity` | SQL files | `schema.rs` regenerado | **`@table type` tipado** |
| Auto-generate diff | `revision --autogenerate` ⚠ requiere imports | `migration:generate` ⚠ requiere build TS | ❌ | manual | **`fitz db diff` directo** |
| Drift check para CI | ⚠ scripts custom | ⚠ scripts custom | `flyway info` parsing | manual | **`fitz db check` exit 0/1** |
| Data migrations en lenguaje | Python | TypeScript | callbacks Java | Rust | **Fitz** |
| Offline SQL | `alembic upgrade --sql` | ❌ | `flyway migrate -dryRunOutput` | ❌ | **`fitz db migrate --sql`** |
| Adoptar DB legacy | `alembic stamp head` | manual | `flyway baseline` | manual | **`fitz db stamp --all`** |
| Cero dependencias | requiere Python + Alembic | requiere Node + TypeORM | requiere JVM + Flyway | requiere Rust toolchain + diesel_cli | **solo el binario `fitz`** |

**Diferencial estructural**: la fuente de verdad es el código
tipado del lenguaje, el binario `fitz` trae todo built-in (sin
`pip install` / `cargo install` / `npm install`), el diff conoce
el shape real porque el driver Postgres del intérprete + codegen
+ migrations es el mismo (wire protocol v3.0 + introspect via
`information_schema` + `pg_catalog`).

---

## Próximo cap

Llegaste al final del cap de migraciones. **Próximo cap**:
[M6.C7 — Capstone: app CRUD completa con auth + ORM + WS + cron + Docker](c7-capstone-crud-completo.md).

Es el **capstone del módulo M6** — integramos TODO lo del curso
en una app real production-ready: auth con JWT + Argon2id, ORM
con relations, WebSocket para notificaciones en tiempo real,
cron job de cleanup, Docker compose con Postgres. El capstone
mantiene el patrón idempotente `CREATE TABLE IF NOT EXISTS` al
boot que ya viste en C1 — no porque las migrations no sirvan
(las acabás de aprender), sino porque el capstone se enfoca en
**integrar el stack web**, y ya conocés `fitz db` del cap
anterior. **En proyectos reales más allá del capstone, el
workflow `fitz db diff/migrate` es la herramienta default**.
