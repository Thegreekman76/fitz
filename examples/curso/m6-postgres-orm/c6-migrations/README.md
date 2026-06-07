# M6.C6 — Ejemplo runnable: workflow `fitz db`

Ejemplo del cap [M6.C6 — Migraciones con `fitz db`](../../../../docs/curso/m6-postgres-orm/c6-migraciones-fitz-db.md).
Demuestra el workflow completo end-to-end contra un Postgres
local en Docker: `new` → `diff` → `migrate` → `status` →
`rollback` → `history` → `check` + una `.fitz` migration nativa
con backfill condicional.

## Estructura

```text
c6-migrations/
├── fitz.toml                # manifest del package manager
├── docker-compose.yml       # Postgres 16 local en :5432
├── src/
│   └── main.fitz            # schema final declarado con @table
├── migrations/
│   ├── 20260607120000_initial_schema.sql                   # crea tabla `users`
│   ├── 20260607123000_add_name_and_verified_to_users.sql   # ADD COLUMN x 2
│   └── 20260607130000_backfill_full_name.fitz              # .fitz native + backfill
└── README.md
```

## Setup

```bash
# 1. Arrancás Postgres local.
docker compose up -d

# 2. Exportás DATABASE_URL para el resto de los comandos.
export DATABASE_URL="postgres://postgres:secret@localhost:5432/demo?sslmode=disable"

# 3. Verificás que la DB responde.
psql "$DATABASE_URL" -c "SELECT version();"
```

## Smoke completo del workflow

```bash
# A. Status inicial — todo pending.
fitz db status
# → 3 migrations PENDING.

# B. Aplicás todo.
fitz db migrate
# → ✓ 3 migration(s) aplicada(s).

# C. Verificás contra Postgres.
psql "$DATABASE_URL" -c "\d users"
#                          Table "public.users"
#   Column         |  Type   | Nullable | Default
#  ----------------+---------+----------+----------
#   id             | bigint  | not null | nextval(...)
#   email          | text    | not null |
#   name           | text    | not null | ''
#   email_verified | boolean | not null | false
#   full_name      | text    | not null | ''

psql "$DATABASE_URL" -c "SELECT * FROM _fitz_migrations ORDER BY version;"
#       version       |          applied_at
#  -------------------+------------------------------
#   20260607120000    | 2026-06-07 12:00:23.456789-00
#   20260607123000    | 2026-06-07 12:00:23.567890-00
#   20260607130000    | 2026-06-07 12:00:23.678901-00

# D. Status final — todo applied.
fitz db status

# E. Audit log cronológico.
fitz db history

# F. Insertamos un user de prueba.
psql "$DATABASE_URL" -c "INSERT INTO users (email) VALUES ('ada@example.com');"

# G. Re-corremos la .fitz para que haga el backfill sobre el row nuevo.
#    Como ya está applied, fitz db migrate no hace nada — el backfill
#    es idempotente solo si lo escribís con WHERE adecuado (en este
#    ejemplo, `WHERE full_name = ''` cumple).
psql "$DATABASE_URL" -c "SELECT email, full_name FROM users;"
# → ada@example.com tiene full_name == 'ada@example.com' si re-ejecutaste

# H. Drift check — verifica que el código matchea la DB.
fitz db check
# → ✓ schema sincronizado (exit 0)

# I. Editás src/main.fitz agregando un field nuevo (ej. `phone: Str?`)
#    y verificás drift.
fitz db check
# → ✗ drift detectado: ALTER TABLE "users" ADD COLUMN "phone" text;
# (exit 1)

# J. Rollback de la última.
fitz db rollback
# → ✓ rollback aplicado: 20260607130000_backfill_full_name.fitz

# K. Re-aplicás.
fitz db migrate
```

## Limpiar

```bash
docker compose down -v   # borra la DB completa
```

## Qué demuestra este ejemplo

- Workflow canónico de 4 pasos: `new` → editar `@table` →
  `diff > file.sql` → `migrate`.
- Mezcla de `.sql` y `.fitz` en el mismo dir.
- Backfill condicional con `for` loop dentro de una `.fitz`
  migration.
- Rollback con sección `-- DOWN`.
- Audit log con `history`.
- Drift check para CI.

## Caveats

- Las **migrations `.fitz` corren via intérprete** (no codegen),
  igual que `fitz run`. Para bulk grandes (miles de iteraciones),
  preferí 1 `UPDATE` en `.sql` separado.
- El **rollback de N>1 NO es atómico** — cada migration corre en
  su propia tx. Para "todo o nada", escribí UNA migration con
  todo el rollback adentro.
- **Solo schema `public`** salvo que uses `@table("schema.tabla")`
  en el código.

Detalle completo: [DB y ORM § 26.c](../../../../docs/db-orm.md#26c-migraciones-automaticas-v01016).
