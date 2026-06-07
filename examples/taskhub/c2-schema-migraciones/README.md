# TaskHub C2 — Schema + workflow `fitz db`

Estado del proyecto al cerrar el cap
[C2](../../../docs/taskhub/c2-schema-migraciones.md). Sobre el
setup Docker-first del C1 sumamos:

- 4 `@table type` del dominio (User / Project / Task / Comment).
- `db.connect()` top-level con `DATABASE_URL` del compose.
- Endpoint smoke `GET /api/users` que devuelve lista vacía.
- Primera migration aplicada (`initial_schema.sql` con FK
  constraints + indexes).
- Helper `dev-env.sh` para exportar `DATABASE_URL` en el shell.
- Workflow CI con `fitz db check` en GitHub Actions.

## Estructura

```text
c2-schema-migraciones/
├── fitz.toml
├── Dockerfile
├── docker-compose.yml
├── dev-env.sh                # NUEVO — source para exportar DATABASE_URL
├── .env.example
├── .gitignore
├── src/
│   └── main.fitz             # ACTUALIZADO — 4 @table types + db.connect + GET /api/users
├── frontend/index.html
├── nginx/nginx.conf
├── prometheus/prometheus.yml
├── otel/
├── migrations/
│   └── 20260607130000_initial_schema.sql   # NUEVO — schema completo
├── .github/
│   └── workflows/
│       └── ci.yml            # NUEVO — drift check con fitz db check
└── README.md
```

## Setup (desde cero)

```bash
# 1. Generás secrets.
cp .env.example .env
# Editá .env y reemplazá los `cambiamelocal`.
# Para JWT_SECRET: openssl rand -hex 32

# 2. Arrancás los 5 services.
docker compose up -d --build

# 3. Exportás DATABASE_URL en tu shell (apunta al db expuesto en
#    localhost:5432, no al hostname interno `db`).
source dev-env.sh
# → ✓ DATABASE_URL exportada para fitz db (localhost:5432)

# 4. Aplicás las migrations.
fitz db migrate
# → ✓ 1 migration(s) aplicada(s):
#   - 20260607130000_initial_schema.sql

# 5. Rebuild del binario para que conozca el schema declarado.
#    (En el primer setup desde cero esto NO es necesario porque
#    el paso 2 ya buildea con el src/main.fitz actual — pero
#    cualquier cambio posterior al schema sí lo requiere.)
docker compose up -d --build app
```

## Validación end-to-end

```bash
# A. App via nginx (version actualizada).
curl http://localhost:8000/healthz
# → {"status":"ok","version":"0.1.0-c2"}

# B. Smoke del binario contra la DB.
curl http://localhost:8000/api/users
# → []   ← lista vacía (no hay registros todavía), esperado

# C. Verificar schema en Postgres.
psql "$DATABASE_URL" -c "\dt"
# Debería listar: _fitz_migrations, comments, projects, tasks, users

psql "$DATABASE_URL" -c "\d tasks"
# Muestra columnas + FK constraints + indexes

# D. Status de migrations.
fitz db status
# → 20260607130000_initial_schema.sql   ✓ applied

# E. Sin drift.
fitz db check
# → ✓ schema sincronizado — schema declarado matchea la DB
#   (exit 0)
```

## Cambio de schema (demo del workflow)

```bash
# 1. Editás src/main.fitz para agregar un field al @table:
#    @table("tasks") type Task {
#        ...
#        estimated_hours: Int = 0    # ← nuevo
#        ...
#    }

# 2. Ver el SQL que generaría la migration.
fitz db diff
# → ALTER TABLE "tasks" ADD COLUMN "estimated_hours" bigint NOT NULL DEFAULT 0;

# 3. Crear archivo de migration + editar con UP/DOWN.
fitz db new add_estimated_hours_to_tasks
# → ✓ migrations/<timestamp>_add_estimated_hours_to_tasks.sql
# Editás el archivo poniendo el SQL en -- UP + DROP COLUMN en -- DOWN.

# 4. Aplicar + rebuild.
fitz db migrate
docker compose up -d --build app

# 5. (Opcional) Rollback si te arrepentiste.
fitz db rollback
# Ojo: dejar `estimated_hours` en src/main.fitz post-rollback
# dispara drift en `fitz db check`. Tenés que borrar el field del
# @table también.
```

## Limpiar

```bash
# Detener todo manteniendo el data.
docker compose down

# Detener + borrar la DB (resetea schema).
docker compose down -v
```

## Qué viene

**[Cap C3 — Auth con RBAC custom: 3 roles apilables](../../../docs/taskhub/c3-auth-rbac.md)**.
Sumamos register + login con JWT + Argon2id, `@auth_provider`, y
`@requires("admin")` / `@requires("owner")` / `@requires("member")`
sobre los handlers — con semántica OR apilable.

## Troubleshooting

Ver la sección "Troubleshooting" del
[cap C2](../../../docs/taskhub/c2-schema-migraciones.md#troubleshooting).
