# TaskHub C7 — Observability + frontend + deploy production (FINAL)

Estado del proyecto al cerrar el cap
[C7](../../../docs/taskhub/c7-observability-frontend-deploy.md).
**Cap final del proyecto Construyendo TaskHub**. Sobre el interop
Python del C6 sumamos:

- `@server(prometheus=true)` → endpoint `/metrics` scrapeable
  + Prometheus UI target UP.
- `@healthz` + `@readyz` con check real de DB + SIGTERM drain
  automático.
- Frontend vanilla JS funcional (~500 LoC): login → projects
  list → board kanban con drag&drop + WebSocket live updates.
- Dockerfile final con `fitz build --bundle-python --bundle-pip
  openai` → image distroless ~50 MB.

**Stack único de Fitz completo en un binario**: HTTP + auth +
RBAC + ORM + relations + WS + cron + background + interop
Python + observability + frontend + healthz/readyz + SIGTERM
drain.

## Estructura

```text
c7-observability-frontend-deploy/
├── fitz.toml
├── Dockerfile                # FINAL — Path A bundling distroless ~50 MB
├── docker-compose.yml
├── dev-env.sh
├── .env.example
├── .gitignore
├── src/
│   └── main.fitz             # FINAL — @healthz/@readyz + prometheus=true
├── python/
│   ├── priority.py
│   └── requirements.txt
├── frontend/                 # ACTUALIZADO — vanilla JS funcional
│   ├── index.html
│   └── assets/
│       ├── style.css
│       ├── api.js
│       ├── ws.js
│       └── app.js
├── nginx/nginx.conf          # sin cambios desde C1
├── prometheus/prometheus.yml # sin cambios desde C1
├── otel/
├── migrations/
├── .github/workflows/ci.yml
└── README.md
```

## Setup (desde cero)

```bash
cp .env.example .env
# Editá DB_PASSWORD + JWT_SECRET. Opcional: OPENAI_API_KEY.

docker compose up -d --build
source dev-env.sh
fitz db migrate
docker compose up -d --build app

# Bootstrap admin.
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}'

psql "$DATABASE_URL" -c "UPDATE users SET role='admin' WHERE id=1;"
```

## Validación visual end-to-end

```bash
# Abrí el frontend en el browser.
open http://localhost:8000

# 1. Login con admin@taskhub.local / adminpass123.
# 2. Lista de projects vacía → creás "Sprint Q3".
# 3. Click → entrás al board.
# 4. Agregás tasks con títulos variados:
#    - "URGENT: server down" → priority 5 (heurística)
#    - "Refactor user models" → priority 2
#    - "Diseñar UI" → priority 3
# 5. Drag & drop entre columnas To do / Doing / Done.
# 6. Abrí otra ventana del browser con mismo user.
#    Drag & drop en una ventana → la otra se refresca via WS.
```

## Endpoints de observability

```bash
# Prometheus /metrics scrapeable.
curl http://localhost:8000/metrics | head -20

# Prometheus UI — target taskhub: UP.
open http://localhost:9090

# Jaeger UI — spans automáticos.
open http://localhost:16686

# Healthz + readyz con check real.
curl -i http://localhost:8000/healthz   # 200 OK
curl -i http://localhost:8000/readyz    # 200 OK
```

## Path A vs Path B del Dockerfile

El `Dockerfile` viene con **Path A** activo (bundling, image
distroless ~50 MB):

```dockerfile
FROM ghcr.io/thegreekman76/fitz:latest-python AS builder
RUN fitz build --bundle-python --bundle-pip openai

FROM gcr.io/distroless/cc-debian12 AS runtime
COPY --from=builder /build/target/release/taskhub /app/taskhub
```

Si Path A no funciona en tu setup (toolchain con `--features
python` no disponible, bundling falla, etc.), **descomentá el
Path B fallback** al final del Dockerfile y comentá Path A.
Image final ~250 MB con `python:3.12-slim-bookworm` runtime.

## Limpiar

```bash
docker compose down       # mantiene data
docker compose down -v    # resetea TODO
```

## Próximo paso

**[`boilerplates/taskhub/`](https://github.com/Thegreekman76/fitz/tree/main/boilerplates/taskhub)**:
este mismo estado publicado como **boilerplate descargable**
para que cualquiera lo pueda probar sin pasar por los 7 caps.

## Troubleshooting

Ver la sección "Troubleshooting" del
[cap C7](../../../docs/taskhub/c7-observability-frontend-deploy.md#troubleshooting).
