# `api-multi-tenant` — SaaS multi-tenant con schemas Postgres custom

Showcase del feature **`@table("schema.name")`** introducido en
v0.10.21 (Fase 10.6.e.3 — schemas custom). Construye un SaaS
multi-tenant con **aislamiento de datos a nivel DB**: cada cliente
vive en su propio schema Postgres en vez del patrón típico de
`WHERE tenant_id = X`.

## ¿Por qué schemas custom?

| Enfoque | Aislamiento | Costo migrations | Backups | Recomendado para |
|---|---|---|---|---|
| 1 DB / 1 schema / `tenant_id` column | Bajo (bugs en WHERE → leak) | Bajo (1 migration global) | Filtros custom | Productos con un schema simple |
| **1 DB / 1 schema por tenant** *(este boilerplate)* | **Alto** (PG schema-level, bug-proof) | Medio (1 vez por schema con `fitz db migrate`) | Trivial (`pg_dump --schema=acme`) | **SaaS con datos sensibles por tenant** |
| 1 DB por tenant | Máximo | Alto (cada uno su pool) | Trivial | Enterprise con compliance estricto |

**Bug-proof**: con schemas custom, es imposible que una query SELECT
de tenant A accidentalmente lea data de tenant B. Postgres bloquea a
nivel qualified name (`"acme"."products"` vs `"beta"."products"`).
Sin riesgo de "olvidé el `WHERE tenant_id`".

## Los dos enfoques mostrados

Este boilerplate convive **dos estilos** que cubren los casos comunes:

### Enfoque A — ORM nativo estático per-tenant

Types Fitz separados con `@table("acme.products")` /
`@table("beta.products")`. El ORM genera SQL qualified
automáticamente en compile-time. Type-safe.

```fitz
@table("acme.products") type AcmeProduct {
    @primary id: Int = 0
    name: Str = ""
    price: Float = 0.0
}

@get("/acme/products")
async fn acme_products() -> Result<List<AcmeProduct>> {
    let conn = db.connect(db_url).await?
    return AcmeProduct.all(conn).await
    // SQL emitido: SELECT "id","name","price","created_at" FROM "acme"."products"
}
```

**Usá esto** para SaaS con # de tenants pequeño y fijo (~5-20).
Compile-time safe, sin queries strings dinámicas. Cada tenant
nuevo requiere code change + rebuild.

### Enfoque B — ORM raw dinámico con whitelist

Handler genérico que routea por header `X-Tenant: <slug>`. Valida
el slug contra `public.tenants` (whitelist) y emite query con SQL
dinámico. `Map<Str, Any>` en vez de type concreto.

```fitz
@header(name="X-Tenant")
@get("/products/dynamic")
async fn products_dynamic(x_tenant: Str) -> Result<List<Map<Str, Any>>> {
    let conn = db.connect(db_url).await?

    // Validar contra whitelist (`public.tenants`).
    let tenant_match = Tenant.where(fn(t) => t.slug == x_tenant)
        .first(conn).await
    let _ = match tenant_match {
        Ok(_) => null,
        Err(_) => return Err("tenant `{x_tenant}` no existe"),
    }

    // Slug whitelisted → safe de interpolar.
    let sql = "SELECT id, name, price FROM \"{x_tenant}\".\"products\""
    return conn.query(sql, []).await
}
```

**Usá esto** para SaaS con onboarding self-service (cliente se
registra → CREATE SCHEMA + CREATE TABLE en runtime). Sin type safety
del checker.

## Estructura

```
api-multi-tenant/
├── docker-compose.yml    # 3 servicios: db + api + frontend
├── Dockerfile            # backend Fitz (multi-stage)
├── fitz.toml
├── .env.example
├── src/
│   ├── main.fitz         # handlers HTTP + init schemas en boot
│   └── models.fitz       # @table types con schemas custom
└── frontend/
    ├── Dockerfile        # nginx:alpine
    ├── nginx.conf        # proxy /api/* → backend
    └── html/
        ├── index.html    # concepto general + tabla comparativa
        ├── acme.html     # tenant Acme (color rojo) — Enfoque A
        ├── beta.html     # tenant Beta (color azul) — Enfoque A
        ├── dynamic.html  # selector X-Tenant — Enfoque B
        └── styles.css    # vanilla, sin frameworks
```

## Setup

```bash
cd boilerplates/api-multi-tenant
cp .env.example .env       # opcional, defaults funcionan
docker compose up --build  # ~30s build + ~5s healthcheck Postgres
```

Esperá `[boot] schemas inicializados: acme, beta + tenants seedeados`
en los logs del container `fitz-api-multi-tenant`.

## URLs

| URL | Qué muestra |
|---|---|
| <http://localhost:8080/> | Landing con explicación del concepto + tabla comparativa |
| <http://localhost:8080/acme.html> | Vista del tenant Acme (Enfoque A) — POST + GET productos |
| <http://localhost:8080/beta.html> | Vista del tenant Beta (Enfoque A) — productos aislados |
| <http://localhost:8080/dynamic.html> | Enfoque B con selector de tenant + demo de validación |

## Endpoints del backend

| Endpoint | Enfoque | Qué hace |
|---|---|---|
| `GET /health` | — | Sanity check |
| `GET /tenants` | — | Lista tenants registrados (`public.tenants`) |
| `GET /acme/products` | **A** | `AcmeProduct.all(conn)` → SQL contra `"acme"."products"` |
| `POST /acme/products` | **A** | `AcmeProduct.insert(conn, row)` |
| `GET /beta/products` | **A** | `BetaProduct.all(conn)` → SQL contra `"beta"."products"` |
| `POST /beta/products` | **A** | `BetaProduct.insert(conn, row)` |
| `GET /products/dynamic` | **B** | Header `X-Tenant: <slug>`, valida + query raw |

## Curl examples (sin frontend, debug directo)

Por default el backend NO expone puerto 3000 al host (solo via nginx
proxy en :8080). Para curl directo, descomentá `ports: - "3000:3000"`
en `docker-compose.yml` y `docker compose up -d`.

```bash
# Enfoque A: ORM nativo estático
curl -X POST localhost:3000/acme/products \
     -H 'Content-Type: application/json' \
     -d '{"name":"widget","price":9.99}'
curl localhost:3000/acme/products

curl -X POST localhost:3000/beta/products \
     -H 'Content-Type: application/json' \
     -d '{"name":"gadget","price":19.99}'
curl localhost:3000/beta/products

# Enfoque B: ORM raw dinámico
curl -H "X-Tenant: acme" localhost:3000/products/dynamic
curl -H "X-Tenant: beta" localhost:3000/products/dynamic

# Validación: tenant no registrado → error claro
curl -H "X-Tenant: zeta" localhost:3000/products/dynamic
# → {"error":"tenant `zeta` no existe en `public.tenants`"}
```

O via nginx proxy (same-origin, sin exponer :3000):

```bash
curl localhost:8080/api/acme/products
curl -H "X-Tenant: beta" localhost:8080/api/products/dynamic
```

## Verificar aislamiento desde la DB

Conectarte directo a Postgres:

```bash
docker exec -it fitz-pg-multi-tenant psql -U fitz

# Lista schemas custom + public
\dn

# Lista tablas en cada schema
\dt acme.*
\dt beta.*
\dt public.*

# Confirma que NO podés ver beta desde acme sin qualified
SET search_path TO acme;
SELECT * FROM products;       -- OK (productos de acme)
SELECT * FROM beta.products;  -- requiere qualified explícito
```

## Cómo escalar a tenants dinámicos

Si necesitás onboarding self-service (cliente se registra y le crean
schema + tablas), agregás un handler `POST /tenants` que ejecuta:

```fitz
@post("/tenants")
async fn create_tenant(body: TenantInput) -> Result<Tenant> {
    let conn = db.connect(db_url).await?
    // 1. Validar slug (regex, longitud, etc).
    // 2. Crear schema + tables.
    let _ = conn.exec("CREATE SCHEMA \"{body.slug}\"", []).await?
    let _ = conn.exec(
        "CREATE TABLE \"{body.slug}\".products (...)",
        [],
    ).await?
    // 3. Registrar en public.tenants.
    return Tenant.insert(conn, Tenant { ... }).await
}
```

Después usás **Enfoque B** (`X-Tenant` header) para queries — los
handlers funcionan sobre cualquier slug registrado sin code change.

## Migrations versionadas (recomendado en producción)

Este boilerplate usa `CREATE SCHEMA/TABLE IF NOT EXISTS` en boot
(idempotente, simple). Para producción real, reemplazá por
**`fitz db migrate`** con migrations versionadas (Fase 10.6 — ver
[docs/db-orm.md](https://thegreekman76.github.io/fitz/db-orm/) sec
26.c).

```bash
fitz db diff src/main.fitz > migrations/20260530000000_initial.sql
fitz db migrate
fitz db check     # CI: exit 1 si schema declarado diverge de la DB
```

Las migrations soportan schemas custom — el diff emite
`CREATE SCHEMA IF NOT EXISTS "acme";` automático antes del
`CREATE TABLE "acme"."products" (...)`.

## Troubleshooting

| Síntoma | Causa | Fix |
|---|---|---|
| `[boot] ERROR init_db` | Postgres no healthy todavía | El compose tiene `depends_on: condition: service_healthy` — esperá ~10 seg |
| `tenant X no existe en public.tenants` | Slug no registrado | Insertarlo: `docker exec fitz-pg-multi-tenant psql -U fitz -c "INSERT INTO tenants (slug, name) VALUES ('new', 'New Co')"` |
| Frontend muestra `Error: TypeError fetch failed` | Backend caído | Ver logs: `docker logs fitz-api-multi-tenant` |
| Querés exponer puerto 3000 al host para debug | Default lo deja oculto detrás del proxy nginx | Descomentá `ports: - "3000:3000"` en `docker-compose.yml` |

## Stack

- **Backend**: Fitz binario standalone con ORM nativo. Sin Python,
  sin SQLAlchemy, sin Alembic. ~10 MB imagen final.
- **DB**: Postgres 16 Alpine.
- **Frontend**: HTML/CSS/JS vanilla en nginx. Sin build step, sin
  `node_modules`. ~10 KB total.
- **Proxy**: nginx routea `/api/*` al backend Fitz (same-origin,
  sin CORS).
