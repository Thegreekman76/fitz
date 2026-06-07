# TaskHub C1 — Setup Docker-first

Estado del proyecto al cerrar el cap
[C1](../../../docs/taskhub/c1-setup-docker-first.md). Compose con
5 services arriba + binario Fitz mínimo que responde `/healthz`.

## Estructura

```text
c1-setup/
├── fitz.toml                # manifest del package manager
├── Dockerfile               # multi-stage distroless
├── docker-compose.yml       # 5 services
├── .env.example             # copialo a .env antes de up
├── .gitignore
├── src/
│   └── main.fitz            # responde /healthz
├── frontend/
│   └── index.html           # placeholder
├── nginx/
│   └── nginx.conf           # proxy + static
├── prometheus/
│   └── prometheus.yml       # scrape config
├── otel/                    # vacío en C1 (Jaeger trae collector built-in)
├── migrations/              # vacío en C1 (llega en C2)
└── README.md                # este archivo
```

## Setup

```bash
# 1. Generás secrets.
cp .env.example .env
# Editá .env y reemplazá los `cambiamelocal`.
# Para JWT_SECRET: openssl rand -hex 32

# 2. Arrancás los 5 services.
docker compose up -d --build

# 3. Verificás que están todos arriba (~30s la primera vez).
docker compose ps
```

## Validación end-to-end

```bash
# A. App via nginx.
curl http://localhost:8000/healthz
# → {"status":"ok","version":"0.1.0-c1"}

# B. Postgres.
docker compose exec db psql -U taskhub -d taskhub -c "SELECT version();"

# C. Prometheus UI.
# → http://localhost:9090
# → Status → Targets:
#   - prometheus: UP
#   - taskhub: DOWN (esperado en C1 — /metrics se activa en C7)

# D. Jaeger UI.
# → http://localhost:16686
# → Services vacío (esperado — no emitimos traces en C1).

# E. Frontend.
# → http://localhost:8000
# → Placeholder de TaskHub.
```

## Limpiar

```bash
# Detener todo manteniendo el data de postgres.
docker compose down

# Detener todo + borrar el data de postgres (resetea).
docker compose down -v
```

## Qué viene

**Cap C2 — Schema + workflow `fitz db`** (próximamente — en
desarrollo). Declaramos los 4 `@table type` del dominio (`User`,
`Project`, `Task`, `Comment`) y generamos la primera migration
con `fitz db new` + `diff` + `migrate`.

## Troubleshooting

Ver la sección "Troubleshooting" del
[cap C1](../../../docs/taskhub/c1-setup-docker-first.md#troubleshooting).
