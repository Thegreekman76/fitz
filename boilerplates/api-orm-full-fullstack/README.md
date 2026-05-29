# api-orm-full-fullstack

**9no boilerplate** del directorio `boilerplates/`. Replica el backend
de `api-orm-full` (HTTP + auth + WS + cron + Postgres ORM) **sumando
un frontend vanilla** (HTML+CSS+JS sin build step) en un container
nginx aparte. Cubre el ciclo entero "browser → server → DB" con todo
el stack Fitz trabajando en conjunto.

```
Browser  ─── http://localhost:8080 ───►  nginx (frontend)
                                         │
                                         ├── / (static HTML/JS/CSS)
                                         ├── /api/* (proxy)
                                         └── /ws/*  (proxy + Upgrade)
                                                  │
                                                  ▼
                                         Fitz API (binario standalone)
                                                  │
                                                  ▼
                                              Postgres 16
```

## Qué muestra que no muestran los otros boilerplates

- **Stack completo** — el browser usa **todo** el backend del ORM:
  CRUD HTTP autenticado, eager loading via `.preload(...)`,
  aggregate con GROUP BY, WebSocket realtime, JWT en el header
  Authorization, jsonb dinámico desde el browser.
- **Same-origin via nginx proxy** — el frontend hace `fetch("/api/...")`
  que nginx proxy-ea a `http://api:3000/...`. **Sin CORS**, sin
  preflight, sin gap del WS auth con token en query param —
  nginx inyecta el header `Authorization: Bearer <jwt>` desde el
  `?token=...` que el browser pasa al `new WebSocket(...)` (necesario
  porque los browsers NO permiten custom headers en el constructor
  del WebSocket).
- **Pantallas reales** — 7 vistas que ejercitan cada feature del
  backend de forma independiente.

## Pantallas

| URL | Endpoint(s) backend | Qué ejercita |
|---|---|---|
| `/` (index) | — | redirige a login o posts según localStorage |
| `/login.html` | `POST /auth/login`, `POST /auth/register` | JWT en localStorage |
| `/posts.html` | `GET /posts?status=&tag=` | listado con filtros query, links a CRUD |
| `/post-detail.html?id=N` | `GET /posts/{id}` + preload, `POST /posts/{id}/comments` | eager loading author + comments inline |
| `/new-post.html` | `POST /posts` | tags array + metadata jsonb desde el browser |
| `/edit-post.html?id=N` | `PUT /posts/{id}` | partial update con `Map<Str, Any>` dinámico |
| `/stats.html` | `GET /stats/posts-per-user` | GROUP BY del ORM rendereado con Chart.js |
| `/feed.html` | `WS /feed` | WS auth via token-in-query → header via nginx |

## Cómo correr

### Setup (una vez)

```bash
cp .env.example .env
# editar .env si querés cambiar JWT_SECRET o credenciales DB
```

### Arrancar el stack entero

```bash
docker compose up --build
# esperar a ver:
#   db        listo (pg_isready)
#   api       [boot] schema DB inicializado
#             [ready] server arrancando en :3000
#   frontend  start worker process
```

Builds:
- **api** — usa `ghcr.io/thegreekman76/fitz:latest` (pre-built, ~30-60s).
  Para reproducibilidad pinned: `docker compose build --build-arg FITZ_TAG=v0.10.12`.
- **frontend** — nginx-alpine + static files. <10s build inicial.

### Abrir en el browser

```
http://localhost:8080/
```

El index redirige a `/login.html` si no hay token en localStorage.
Creá una cuenta (botón "Crear una") con email + name + password (8+
chars). Después del registro, login con esas mismas credenciales →
JWT guardado en localStorage → redirige a `/posts.html`.

Desde ahí podés:
1. **Listar posts** con filtros por status (`draft`/`published`) y tag.
2. **Crear un post** (`/new-post.html`) con tags array + metadata jsonb.
3. **Ver detalle** (`/post-detail.html?id=N`) con author + comments
   embebidos (eager loading via `.preload("author").preload("comments")`).
4. **Comentar** en un post.
5. **Editar** (solo el autor del post) o **borrar** desde el listado.
6. **Stats** (`/stats.html`) — Chart.js sobre GROUP BY del ORM.
7. **Feed realtime** (`/feed.html`) — WebSocket con auth: abrí el
   feed en 2 tabs distintos y verás los mensajes broadcast simétricos
   (todos los conectados ven todos los mensajes, incluido el emisor).

### Docs autogenerados

- `http://localhost:8080/docs` — UI Scalar con la spec OpenAPI 3.1
  de los endpoints HTTP (proxy del `/docs` del backend).
- `http://localhost:8080/asyncapi.json` — schema AsyncAPI 3.0 de los
  endpoints WS (proxy del `/asyncapi.json` del backend).

## Arquitectura del nginx proxy

El frontend está en `:8080` y el backend en `:3000` (de la red
interna de compose, no expuesto al host por default). Para que el
browser no haga requests cross-origin, nginx proxy-ea todo:

```nginx
location /api/ {
    proxy_pass http://api:3000/;
    # ... headers estándar
}

location /ws/ {
    proxy_pass http://api:3000/;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    # Token JWT del browser pasa como ?token=... en la URL.
    # nginx lo convierte a header Authorization antes del proxy:
    set $auth_token "";
    if ($arg_token) {
        set $auth_token "Bearer $arg_token";
    }
    proxy_set_header Authorization $auth_token;
}
```

Eso resuelve **dos limitaciones de los browsers** sin tocar el backend
Fitz:
- **No-CORS**: las requests `fetch("/api/...")` son same-origin desde
  la perspectiva del browser.
- **WS auth header**: `new WebSocket(url)` no acepta custom headers,
  así que el frontend pasa `?token=...` en la URL y nginx lo transforma
  al header `Authorization: Bearer ...` que el backend Fitz lee
  normalmente.

## Diferencias con `api-orm-full`

- **`/src` idéntico** al de `api-orm-full` — ningún cambio al backend.
  Eso garantiza que el frontend ejercita exactamente lo mismo que se
  validó en el smoke real de v0.10.11 (preload, password_hash con
  `@hidden`, etc.).
- **Dockerfile + docker-compose** — agregan el frontend service y
  remueven el `ports: 3000:3000` del api (el backend queda solo
  accesible internamente desde el frontend).
- **`frontend/`** nuevo (Dockerfile + nginx.conf + html/) con las 7
  pantallas y el reverse proxy.

## Variables de entorno

Idénticas a `api-orm-full`:

| Var | Default | Notas |
|---|---|---|
| `POSTGRES_USER` | `fitz` | usuario PG |
| `POSTGRES_PASSWORD` | `fitz` | password PG |
| `POSTGRES_DB` | `fitz` | DB name |
| `JWT_SECRET` | `demo-fullstack-secret-cambiame-en-prod` | **cambiar en prod** |
| `MAX_DRAFTS_AGE_DAYS` | `30` | input del cron daily de cleanup |

## Próximos pasos sugeridos

- **Hot reload del frontend en dev**: montar `./frontend/html` como
  volume del nginx container (`volumes: ['./frontend/html:/usr/share/nginx/html']`)
  para que editar HTML/JS no requiera rebuild.
- **Modal admin**: panel para users con `role: "admin"` que liste
  todos los users + drafts orphaned + permita banear users.
- **Optimistic updates** en el frontend (mostrar el comment
  instantáneo antes del round-trip al server).
- **Markdown rendering** en posts (Chart.js de stats ya usa CDN —
  sumar marked.js es paralelo).
