# TaskHub C3 — Auth con RBAC custom de 3 roles apilables

Estado del proyecto al cerrar el cap
[C3](../../../docs/taskhub/c3-auth-rbac.md). Sobre el setup
Docker-first + schema del C2 sumamos:

- `@hidden` en `User.password_hash` — no aparece en JSON
  responses.
- `@auth_provider` que valida el Bearer JWT contra la DB.
- Endpoints públicos: `POST /api/auth/register` + `POST /api/auth/login`.
- Endpoints autenticados: `GET /api/me`, `GET /api/users`
  (admin), `POST /api/users/{id}/promote` (admin), `GET /api/stats`
  (admin O owner — apilable demo).
- Tipos auxiliares (`RegisterInput`, `LoginInput`, `LoginResponse`,
  `PromoteInput`, `StatsResponse`) separados del shape DB.

**Schema sin cambios respecto a C2** — el `@hidden` es solo
decisión de código (codegen del JSON), no toca la DB.

## Estructura

```text
c3-auth-rbac/
├── fitz.toml
├── Dockerfile
├── docker-compose.yml
├── dev-env.sh
├── .env.example
├── .gitignore
├── src/
│   └── main.fitz             # ACTUALIZADO — auth + RBAC end-to-end
├── frontend/index.html
├── nginx/nginx.conf
├── prometheus/prometheus.yml
├── otel/
├── migrations/
│   └── 20260607130000_initial_schema.sql   # sin cambios desde C2
├── .github/workflows/ci.yml
└── README.md
```

## Setup (desde cero)

```bash
# 1. Generás secrets.
cp .env.example .env
# Editá .env y reemplazá los `cambiamelocal`.
# JWT_SECRET: openssl rand -hex 32

# 2. Arrancás los 5 services.
docker compose up -d --build

# 3. Aplicás migrations (DATABASE_URL exportada con dev-env.sh).
source dev-env.sh
fitz db migrate

# 4. Rebuild del binario para que tome el código del C3.
docker compose up -d --build app
```

## Validación end-to-end

```bash
# A. Verificar version del binario.
curl http://localhost:8000/healthz
# → {"status":"ok","version":"0.1.0-c3"}

# B. Registrar el primer user (será el admin).
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}'
# → {"id":1,"email":"admin@taskhub.local","role":"member","created_at":"..."}
# (password_hash NO aparece — @hidden funciona)

# C. Elevar manualmente a admin (bootstrap, una sola vez).
psql "$DATABASE_URL" -c "UPDATE users SET role='admin' WHERE id=1;"

# D. Login admin.
ADMIN_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}' \
  | jq -r .token)

# E. GET /me con token admin.
curl http://localhost:8000/api/me -H "Authorization: Bearer $ADMIN_TOKEN"
# → {"id":1,"email":"...","role":"admin","created_at":"..."}

# F. Lista de users como admin.
curl http://localhost:8000/api/users -H "Authorization: Bearer $ADMIN_TOKEN"
# → [{"id":1,...}]

# G. Registrar un member normal.
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"bob@taskhub.local","password":"bobpass123"}'

MEMBER_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"bob@taskhub.local","password":"bobpass123"}' \
  | jq -r .token)

# H. Member intenta GET /users → 403.
curl -i http://localhost:8000/api/users -H "Authorization: Bearer $MEMBER_TOKEN"
# → HTTP/1.1 403 Forbidden
#   {"error":"forbidden: requires role 'admin', user has 'member'"}

# I. Admin promueve a Bob a owner.
curl -X POST http://localhost:8000/api/users/2/promote \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"new_role":"owner"}'
# → {"id":2,"email":"bob@...","role":"owner","created_at":"..."}

# J. Apilable: Bob (ahora owner) puede ver /stats con su viejo token.
#    El provider hace lookup contra DB, el role efectivo es 'owner'.
curl http://localhost:8000/api/stats -H "Authorization: Bearer $MEMBER_TOKEN"
# → {"total_users":2,"total_projects":0,"total_tasks":0}
```

## Limpiar

```bash
# Detener todo manteniendo data + users registrados.
docker compose down

# Detener + borrar la DB (resetea schema + users).
docker compose down -v
```

## Qué viene

**[Cap C4 — CRUD + relations + WebSocket en vivo por project](../../../docs/taskhub/c4-crud-relations-ws.md)**.
Sumamos los CRUD de projects/tasks con `@belongs_to` /
`@has_many` para navigation methods + eager loading con
`.preload(...)` + WebSocket por project para broadcast de cambios
en vivo + limitación honesta del MVP del lenguaje.

## Troubleshooting

Ver la sección "Troubleshooting" del
[cap C3](../../../docs/taskhub/c3-auth-rbac.md#troubleshooting).
