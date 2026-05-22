# `api-middleware-cors` — API con auth nativa + CORS + frontend separado

Boilerplate end-to-end de una API protegida con JWT/Argon2 + CORS
configurable + middleware logger custom + **frontend HTML+JS
vanilla en otro container** que demuestra el flow cross-origin
real.

Stack:
- **API Fitz** en `localhost:3000` (binario nativo, distroless,
  ~31 MB).
- **Frontend** en `localhost:8080` (nginx alpine sirviendo un
  HTML estático, ~25 MB).
- Origins distintos → CORS necesario.
- `docker compose up --build` levanta ambos en un comando.

```text
┌────────────────────────────┐     ┌────────────────────────────┐
│ Browser                    │     │ Fitz API                   │
│  http://localhost:8080     │     │  http://localhost:3000     │
│  index.html + fetch()      │ ──> │  @authenticated, @cors,    │
│                            │     │  @middleware, jwt.encode   │
└────────────────────────────┘     └────────────────────────────┘
         ▲                                  ▲
         │                                  │
   nginx:alpine                       fitz binario
   (frontend container)               (api container)
   puerto 8080:80                     puerto 3000:3000
```

## Qué demuestra

- **Auth nativa (Fase 9.w.1)**: `@auth_provider` singleton +
  `@authenticated` por handler con JWT bearer tokens.
- **Built-ins `jwt` y `hash`**: `jwt.encode/decode` (HS256) +
  `hash.password/verify` (Argon2id), sin deps externas.
- **`@middleware(fn)` custom**: logger que imprime
  `[METHOD] /path` antes de cada request.
- **`@middleware(cors({...}))` built-in**: configura preflight
  OPTIONS automático + headers `Access-Control-Allow-*` por
  handler. Cada endpoint puede tener su propio config (allow_origin,
  allow_methods, allow_headers, max_age).
- **Frontend cross-origin real**: el HTML+JS en `:8080` hace
  `fetch` al API en `:3000` — el browser dispara preflight CORS
  automático. Sin el `@middleware(cors(...))`, el browser bloquea
  los requests.
- **Status codes custom**: `return 401 { ... }` para credenciales
  inválidas.
- **OpenAPI 3.1 + UI Scalar** auto-documenta `bearerAuth` +
  responses 401 + security per-endpoint.

## Estructura del directorio

```
api-middleware-cors/
├── README.md
├── fitz.toml                    ← manifest del package manager
├── src/
│   └── main.fitz                ← API Fitz (~80 LoC con comments)
├── frontend/
│   ├── index.html               ← HTML+JS vanilla (~150 LoC)
│   └── nginx.conf               ← config minimal de nginx
├── Dockerfile                   ← multi-stage: fitz builder + distroless
├── docker-compose.yml           ← api + frontend (2 services)
├── .env.example                 ← JWT_SECRET (deuda — todavía no se usa)
├── .dockerignore
└── .gitignore
```

## Prerequisitos

**Solo Docker** con Compose v2:

```bash
docker --version            # 24+ recomendado
docker compose version      # v2 plugin (incluido en Docker Desktop)
```

NO necesitás Fitz instalado localmente — el Dockerfile usa la
imagen oficial.

## Paso a paso

### 1. (Opcional) Setup de secrets

Copiá el `.env.example` a `.env`:

```bash
cd boilerplates/api-middleware-cors
cp .env.example .env
```

**Nota honesta**: hoy el `.env` NO se usa todavía. Fitz no soporta
`env("KEY")` builtin (deuda futura del lenguaje). El `JWT_SECRET`
está hardcoded en `src/main.fitz` con un valor placeholder
público:

```fitz
let SECRET = "demo-secret-cambiame-antes-de-deploy-32-chars-min"
```

**Para producción real, editá esa línea en `src/main.fitz`** con un
secret de 32+ chars aleatorio antes del `docker compose up --build`.
Cuando Fitz soporte env vars, el .env va a tener sentido sin
editar código.

### 2. Levantar todo

```bash
docker compose up --build
```

Build inicial: ~3-4 min (compila el binario + descarga nginx).
Siguientes runs son cacheados.

Output esperado:

```text
api          | 🏔️  Fitz HTTP escuchando en http://0.0.0.0:3000
api          |    POST /login
api          |    GET /me
api          |    GET /items
api          |    GET /openapi.json  (schema autogenerado)
api          |    GET /docs          (UI Scalar)
frontend     | nginx ready on :80
```

### 3. Abrir el frontend en el browser

```text
http://localhost:8080
```

Vas a ver una página con 3 secciones:

1. **GET /items** — endpoint público. Click el botón, ves la lista
   de items. En la respuesta el browser muestra los headers CORS
   (DevTools → Network → Response Headers).
2. **POST /login** — formulario pre-rellenado con creds del
   boilerplate (`ada@example.com` / `secret-ada-123`). Click
   "Login" → recibe JWT, lo guarda en localStorage.
3. **GET /me** — endpoint protegido. Click "Llamar GET /me" (con
   token) → muestra el user del JWT. Click "Llamar SIN token" →
   recibe 401.

### 4. Validar CORS desde la terminal

En otra terminal, simulá un preflight OPTIONS:

```bash
# Preflight para POST /login desde origin http://localhost:8080
curl -X OPTIONS \
     -H 'Origin: http://localhost:8080' \
     -H 'Access-Control-Request-Method: POST' \
     -H 'Access-Control-Request-Headers: Content-Type' \
     -i localhost:3000/login

# Response esperado:
# HTTP/1.1 204 No Content
# access-control-allow-origin: http://localhost:8080
# access-control-allow-methods: POST, OPTIONS
# access-control-allow-headers: Content-Type
```

Sin el `@middleware(cors(...))`, el browser ABORTA antes del POST
real ("CORS preflight failed"). Con el middleware, axum responde
204 + headers y el browser procede.

### 5. UI interactiva /docs

```text
http://localhost:3000/docs
```

UI Scalar generada del schema OpenAPI 3.1. El `/me` muestra el
🔒 con el bearer auth requerido. Click "Authorize" → pegar el JWT
del login → probar el endpoint directo desde la UI.

### 6. Parar todo

```bash
docker compose down
```

## Credenciales del boilerplate

| Email | Password | Role |
|---|---|---|
| `ada@example.com` | `secret-ada-123` | `user` |

Los hashes Argon2id se generan al boot con `hash.password(...)`. Para
agregar más users, editá `src/main.fitz` + sumá entries al
`@auth_provider`.

## Cómo extender

### Agregar un user admin con `@admin`

```fitz
// En main.fitz:
let BOB_HASH = hash.password("bob-super-secret")

@auth_provider
fn check_token(headers: Map<Str, Str>) -> Result<User> {
    // ... (igual que ahora pero matchea más users) ...
    let email = claims["email"]
    if (email == "ada@example.com") {
        return Ok(User { id: 1, email: email, name: "Ada", role: "user" })
    }
    if (email == "bob@example.com") {
        return Ok(User { id: 2, email: email, name: "Bob", role: "admin" })
    }
    return Err("user no encontrado")
}

@middleware(cors({"allow_origin": "http://localhost:8080", "allow_methods": ["GET", "OPTIONS"], "allow_headers": ["Authorization"]}))
@admin
@get("/admin/secret")
fn admin_secret(user: User) -> Str => "solo admins ven esto"
```

El runtime valida `user.role == "admin"` automático → 403 si no
es admin.

### Más origins en CORS

`allow_origin` toma un Str (single origin) o List<Str> para echo
del Origin si matchea:

```fitz
@middleware(cors({
    "allow_origin": ["http://localhost:8080", "https://prod.example.com"],
    "allow_methods": ["GET", "POST", "OPTIONS"],
    "max_age": 3600
}))
```

### Middleware más complejo

El logger actual solo printea. Para auth custom (e.g. API key
header):

```fitz
fn require_api_key(req: Request) -> Null {
    let key = req.headers.get("x-api-key")
    if (not key.is_ok()) {
        return 401 { "error": "falta X-API-Key" }
    }
    if (key.unwrap_or("") != "demo-key") {
        return 401 { "error": "API key inválida" }
    }
    return null
}

@middleware(require_api_key)
@get("/api/internal")
fn internal() -> Str => "solo apps con la key"
```

El runtime corre `require_api_key` antes del handler. Si retorna
`null` o sin return, la chain continúa. Si retorna `return 401 {
... }`, short-circuitea.

## Variables de entorno

| Variable | Default | Uso |
|---|---|---|
| `JWT_SECRET` | hardcoded en main.fitz | **Deuda**: cuando Fitz soporte `env()`, se va a leer del `.env` |

Hoy todas las variables están hardcoded. El `.env.example` queda como placeholder para el día que Fitz tenga la feature.

## Troubleshooting

### El browser muestra "CORS error" en la consola

Verificá:
1. El frontend está en `http://localhost:8080` (no `127.0.0.1` —
   esos son origins distintos para CORS).
2. La API está en `http://localhost:3000` y respondió al preflight
   OPTIONS con `Access-Control-Allow-Origin: http://localhost:8080`.

DevTools → Network → click el OPTIONS request → ver Response
Headers. Si NO ves `Access-Control-Allow-*`, el `@middleware(cors(...))`
no se está aplicando. Revisá que esté arriba del `@get`/`@post`.

### El POST /login devuelve 401 con creds correctas

Verificá:
1. Email exacto: `ada@example.com` (case-sensitive).
2. Password exacto: `secret-ada-123`.
3. El boot del container imprime `Fitz HTTP escuchando en...` —
   si no, el `hash.password(...)` falló (raro).

### El GET /me devuelve 401 "JWT signature inválida"

El JWT_SECRET del `jwt.decode` no matchea con el del `jwt.encode`.
Si editaste `src/main.fitz`, asegurate de hacer `docker compose
down && docker compose up --build` para regenerar el binario con
el secret nuevo.

### `docker compose up` falla con "port 3000 already in use"

Otro proceso (otro container Fitz, otro server local) está en
:3000. Para liberar:

```bash
# Ver containers Docker que usen :3000
docker ps --filter "publish=3000"

# Parar/borrar el conflictivo
docker stop <container_name>
docker rm <container_name>

# Después
docker compose up --build
```

### Mac M-series: `exec format error`

La imagen base es Linux x64. Workaround:

```yaml
# En docker-compose.yml, agregá al service api:
platform: linux/amd64
```

Multi-arch (linux/amd64 + linux/arm64) es deuda del `release.yml` de Fitz.

## Roadmap del boilerplate

- **Env vars en Fitz**: cuando el lenguaje tenga `env("KEY")`
  builtin, mover el `JWT_SECRET` a `.env`.
- **Refresh tokens**: hoy el JWT no expira (no tiene `exp` claim).
  Para producción real, agregar `exp` + endpoint `/refresh`.
- **Sessions cookie-based** como alternativa a bearer (deuda Fase
  9.w iteración 2).
- **Multi-arch image** para Mac M-series nativo.
- **HTTPS via reverse proxy**: agregar un `traefik` o `caddy`
  delante de la API para SSL automático con Let's Encrypt.

## Siguientes pasos

- [`boilerplates/api-websocket/`](../api-websocket/) — chat
  broadcast tipado con `@ws` + frontend HTML.
- [`boilerplates/api-postgres-python/`](../api-postgres-python/) —
  CRUD con DB real via interop Python (SQLAlchemy + Postgres).
