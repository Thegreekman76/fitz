# Boilerplates Fitz

Plantillas listas para arrancar proyectos reales con Fitz. Todas
están **Dockerizadas** — no necesitás instalar Rust, Python ni
Postgres en tu máquina. Solo Docker y `docker compose`.

## Los 7 boilerplates

| Boilerplate | Qué demuestra | Stack | Dockerfile | Compose |
|-------------|---------------|-------|------------|---------|
| [`cli-tool`](./cli-tool/)                       | CLI puro — sales report con `.reduce/.filter/.map`     | Fitz standalone (binario nativo)                       | distroless    | —       |
| [`api-simple`](./api-simple/)                   | REST API tipada + OpenAPI auto + Scalar UI             | Fitz standalone (binario nativo)                       | distroless    | —       |
| [`api-middleware-cors`](./api-middleware-cors/) | Auth nativa (JWT + Argon2) + middleware + CORS cross-origin + frontend | Fitz standalone + frontend nginx     | distroless    | 2 svcs  |
| [`api-websocket`](./api-websocket/)             | WebSockets tipados (broadcast) + frontend chat         | Fitz standalone + frontend nginx                       | distroless    | 2 svcs  |
| [`api-postgres-fitz`](./api-postgres-fitz/) ⭐  | **CRUD con ORM nativo Fitz** + Postgres — sin Python   | Fitz standalone + Postgres                             | distroless    | 2 svcs  |
| [`api-postgres-python`](./api-postgres-python/) | CRUD multi-archivo con SQLAlchemy + Postgres (interop) | Fitz + `--features python` + Postgres                  | python:3.12   | 2 svcs  |
| [`api-fullstack-postgres`](./api-fullstack-postgres/) | **CRUD fullstack** — API + frontend vanilla rico + Postgres | Fitz + `--features python` + Postgres + nginx  | python:3.12   | 3 svcs  |

Cada directorio tiene su **README exhaustivo** con paso a paso,
explicación del stack, troubleshooting y plan para llevar a
producción. Empezá por ahí.

## Quickstart genérico

Casi todos siguen el mismo flujo:

```bash
cd boilerplates/<nombre>
cp .env.example .env       # si existe (algunos no necesitan)
docker compose up --build  # o `docker build .` si no hay compose
```

Y abrir la URL que diga el README de ese boilerplate.

## Qué boilerplate elegir

### Estoy aprendiendo Fitz desde cero

Empezá por **[`cli-tool`](./cli-tool/)**. Es el más simple — un
binario standalone que hace un sales report. Sin HTTP, sin DB, sin
frontend. Muestra los blocks básicos: types, listas, métodos
funcionales (`.reduce`/`.filter`/`.map`), interpolación de strings.

### Quiero ver una REST API mínima

Pasá a **[`api-simple`](./api-simple/)**. Define un par de `type`,
expone GET/POST con `@get`/`@post`, y la **OpenAPI 3.1 + Scalar
UI** se autogenera en `/docs`. Sin DB — todo en memoria.

### Quiero auth, middleware y CORS reales

**[`api-middleware-cors`](./api-middleware-cors/)** es el más rico
del lado "API stateless". JWT + Argon2id como built-ins del
lenguaje (no deps externas), `@authenticated`/`@admin` decorators,
middleware encadenado custom + `cors({...})` built-in. Frontend
nginx en otro container para demostrar **CORS cross-origin REAL**
(con preflight OPTIONS dispared por el browser).

### Quiero ver WebSockets

**[`api-websocket`](./api-websocket/)** levanta un chat con
broadcast. `@ws("/chat")` + `WsConn<ChatMsg>` (tipado!) + heartbeat
ping/pong automático + frontend vanilla con `new WebSocket(...)`.
**AsyncAPI 3.0 auto-generado** además de OpenAPI (sirve para
clientes generados).

### Necesito DB persistente (Postgres)

Dos opciones según el stack que prefieras:

- **[`api-postgres-fitz`](./api-postgres-fitz/) ⭐ (recomendado para
  proyectos nuevos)** — usa el **ORM nativo del lenguaje** (cap 31
  de la guía). Sin Python, sin SQLAlchemy. Driver Postgres puro
  embebido en el binario. **~60 LoC total**, imagen distroless de
  **~15 MB**. Mismo dominio que el de Python, side-by-side.
- **[`api-postgres-python`](./api-postgres-python/)** — usa
  `fitz run --features python` para llamar a SQLAlchemy + psycopg2
  desde Fitz. Útil si tenés código SQLAlchemy existente que querés
  migrar gradualmente, o si necesitás librerías Python específicas
  (numpy/pandas/scipy/ML) en el mismo proceso.

### Quiero el stack web completo (API + DB + frontend)

**[`api-fullstack-postgres`](./api-fullstack-postgres/)** es el
**showcase fullstack** del proyecto: CRUD de tasks con frontend
vanilla rico (tabla, edit inline, filtros, badges), API Fitz con
CORS, Postgres con volume persistente, todo en **3 servicios** del
mismo `docker-compose.yml`. Es el más cercano a "lo que harías
en un proyecto real".

## Convenciones comunes a todos

### Variable de entorno: `FITZ_TAG`

Los Dockerfiles aceptan ARGs para pinear la versión de Fitz al
compilar:

```bash
docker build --build-arg FITZ_TAG=v0.9.25 .       # tag (recomendado)
docker build --build-arg FITZ_BRANCH=main .       # branch (default)
docker build --build-arg FITZ_REV=abc123def .     # commit SHA
```

Para producción, **siempre pinear un tag**. El default es el HEAD
del default branch que el repo público tenga.

### Tamaño de imagen final

Los boilerplates que no necesitan Python (`cli-tool`, `api-simple`,
`api-middleware-cors`, `api-websocket`, **`api-postgres-fitz`**)
buildean a **binario nativo standalone con `fitz build`** y usan
**distroless** como runtime → imágenes finales de ~15-40 MB. El
ORM nativo (cap 31) habilita usar Postgres sin necesidad de Python
en el container.

Los que necesitan interop Python (`api-postgres-python`,
`api-fullstack-postgres`) usan `python:3.12-slim` en runtime
(Python + libpq + el binario fitz) → imágenes de ~250 MB. Útil
cuando querés librerías Python específicas (SQLAlchemy con queries
complejas heredadas, numpy/pandas/scipy, ML inference).

### Persistencia

Los boilerplates con DB usan **volumes nombrados** (no bind
mounts). La data sobrevive `docker compose down`. Para reset
completo:

```bash
docker compose down -v   # -v borra los volumes
```

### CORS

Los boilerplates con frontend en container separado configuran
`@middleware(cors({...}))` por handler con `allow_origin:
"http://localhost:8080"`. Para producción, cambiá el origin
al dominio real del frontend.

## Próximos boilerplates planificados

Posibles si aparece demanda real:

- **`api-orm-full`** (en planificación): showcase del ORM nativo
  completo — User ↔ Post (HasMany) + Post ↔ Comment + User ↔ Profile
  (HasOne) + JSONB metadata + arrays tags + aggregates + GROUP BY
  + auth nativa + WebSockets para notif realtime + cron cleanup.
  Dominio rico (blog/CMS o e-commerce básico) que ejercita el stack
  Fitz entero.
- **`api-cron-jobs`**: `@cron` + `@background` + `spawn(...)`
  showcase. Hoy cubierto parcialmente en el cap 30 de la guía.
- **`fullstack-spa`**: el de Postgres + un SPA Vue/React/Svelte
  en lugar de vanilla. (Sumando build step del frontend.)
- **`grpc-or-graphql`**: depende del soporte futuro en Fitz.

Si necesitás un escenario que no está cubierto, abrí un issue
en el repo del lenguaje.

## Recursos del lenguaje

- **[Guía pedagógica](../docs/guide.md)** — 30+ capítulos cubriendo
  todas las features del lenguaje (CLI, HTTP, async, auth, WS,
  cron, interop Python).
- **[Roadmap](../docs/roadmap.md)** — qué se hizo, qué viene.
- **[Sintaxis](../docs/syntax-spec.md)** — referencia sintáctica
  completa.
- **[Deudas del lenguaje](../docs/deudas_lenguaje.md)** —
  limitaciones conocidas y workarounds.
