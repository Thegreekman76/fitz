# `api-simple` — HTTP API mínima con OpenAPI 3.1 auto

REST API básica con 3 endpoints (CRUD parcial), JSON marshaling
automático, **OpenAPI 3.1 schema autogenerado** en `/openapi.json`
y **UI Scalar interactiva** en `/docs` — todo sin librerías ni
build steps adicionales. Compilada a binario nativo standalone y
empaquetada en una imagen Docker de **~31 MB** (`distroless/cc`).

```text
$ curl localhost:3000/items
[{"id":1,"name":"widget","quantity":100},{"id":2,"name":"gadget","quantity":50}]

$ curl -X POST localhost:3000/items \
       -H 'Content-Type: application/json' \
       -d '{"id":3,"name":"thingamajig","quantity":75}'
{"id":3,"name":"thingamajig","quantity":75}

$ curl localhost:3000/items/2
{"id":2,"name":"gadget","quantity":50}

$ curl -w "%{http_code}" localhost:3000/items/99
{"error":"item con id=99 no encontrado"}
404
```

## Qué demuestra

- **`@server(port, host)`** — configurar el listener HTTP del
  binario nativo.
- **`@get` / `@post`** con path templates (`{id}`).
- **Tipos custom (`Item`)** usados como body de POST + return de
  GET. JSON marshaling bidireccional **automático**.
- **State compartido HTTP** via `let ITEMS = [...]` top-level
  (post-F17 — pre-F17 esto requería deuda; ahora compila a
  `LazyLock<Arc<Mutex<...>>>` y es safe).
- **Status codes custom** con `return 404 { ... }`.
- **OpenAPI 3.1** autogenerado en `/openapi.json` desde los
  decoradores. **UI Scalar** interactiva en `/docs`.

Sin pip install fastapi, sin npm install express, sin
`require 'sinatra'`. **Un solo binario `fitz`** del compilador
produce todo eso.

## Estructura del directorio

```
api-simple/
├── README.md          ← este archivo
├── fitz.toml          ← manifest del package manager Fitz
├── src/
│   └── main.fitz      ← código fuente (~50 LoC con comentarios)
├── Dockerfile         ← multi-stage: fitz builder + distroless runtime
├── .dockerignore
└── .gitignore
```

## Prerequisitos

**Solo Docker** (versión 24+ recomendada). NO necesitás Fitz
instalado localmente — el Dockerfile usa la imagen oficial
`ghcr.io/thegreekman76/fitz:latest`.

```bash
docker --version    # Docker version 24.x o superior
```

## Paso a paso

### 1. Construir la imagen

```bash
cd boilerplates/api-simple
docker build -t fitz-api-simple .
```

El primer build tarda ~2-3 min (descarga imagen base + compila
axum/tokio/serde desde cero). Builds subsiguientes son cacheados
si solo cambia `src/main.fitz`.

### 2. Correr el servicio

```bash
docker run --rm -p 3000:3000 fitz-api-simple
```

El flag `-p 3000:3000` mapea el puerto del container al host. Vas
a ver:

```text
🏔️  Fitz HTTP escuchando en http://0.0.0.0:3000
   GET /
   GET /items
   POST /items
   GET /items/{id}
   GET /openapi.json  (schema autogenerado)
   GET /docs          (UI Scalar)
```

### 3. Probar la API

En otra terminal:

```bash
# Health check
curl localhost:3000/

# Listar items
curl localhost:3000/items

# Crear un item
curl -X POST localhost:3000/items \
     -H 'Content-Type: application/json' \
     -d '{"id":3,"name":"thingamajig","quantity":75}'

# Verificar que se creó
curl localhost:3000/items
# → ahora muestra los 3 items (incluyendo el thingamajig)

# Get por id existente
curl localhost:3000/items/2

# Get por id inexistente (404)
curl -w "\nstatus: %{http_code}\n" localhost:3000/items/99
```

### 4. UI interactiva en /docs

Abrí en el browser:

```text
http://localhost:3000/docs
```

Vas a ver la UI Scalar con todos los endpoints documentados,
generada **automáticamente** del schema OpenAPI 3.1. Click en
cualquier endpoint → "Try it" → ejecutar requests sin escribir
un solo cliente. El schema bit-a-bit incluye los tipos `Item`
con sus fields, defaults, y nullables.

### 5. Schema OpenAPI raw

```bash
curl localhost:3000/openapi.json | python -m json.tool
```

Tres ventajas vs FastAPI/Express:

1. **Sin código extra**: en FastAPI necesitás escribir el modelo
   Pydantic separado del handler; en Fitz el `type Item` Y el
   `fn handler(input: Item)` comparten la definición.
2. **Bit-a-bit consistente**: el schema que ves en `/openapi.json`
   es el MISMO con el que el binario valida los requests (mismo
   `__FromFitzJson`/`__ToFitzJson` traits emitidos en compile-time).
3. **Cero overhead runtime**: el schema es un string estático
   embedded en el binario via `static __FITZ_OPENAPI_SCHEMA: &str
   = r###"..."###`. No reflection, no allocación al boot.

## Variables de entorno

Este boilerplate no usa ninguna. La data vive in-memory en el
`let ITEMS = [...]` top-level. Para datos persistentes, ver
[`boilerplates/api-postgres-python/`](../api-postgres-python/).

## Cómo extender

### Agregar más endpoints

Sumá `@get`/`@post`/`@put`/`@delete` con su handler:

```fitz
@put("/items/{id}")
fn update_item(id: Int, input: Item) -> Item {
    // ... lookup + update ...
    return input
}

@delete("/items/{id}")
fn delete_item(id: Int) -> Item {
    // ... lookup + remove ...
    return removed
}
```

El schema OpenAPI se regenera automático en cada `fitz build`.

### Agregar query params

```fitz
@get("/items?limit={limit}")
fn list_items(limit: Int) -> List<Item> {
    return ITEMS.take(limit)  // hypothetical method
}
```

### Agregar headers como params

```fitz
@get("/items")
@header(name="x-tenant-id")
fn list_items(x_tenant_id: Str) -> List<Item> {
    // x_tenant_id es required; sin el header → 400 auto
    return filter_by_tenant(ITEMS, x_tenant_id)
}
```

### Persistir cambios

Hoy `ITEMS.push(input)` modifica el state in-memory pero **se
pierde al reiniciar el container**. Para producción real:

- **DB nativa** (Fase 10): llega cuando se implemente el driver
  Postgres puro Fitz.
- **DB via interop Python** (hoy): ver
  [`boilerplates/api-postgres-python/`](../api-postgres-python/)
  con SQLAlchemy + Postgres real.

## Troubleshooting

### `docker run` no muestra output / curl falla con `Connection refused`

Asegurate de tener `-p 3000:3000`. Sin ese flag el puerto del
container no se publica al host.

```bash
docker run --rm -p 3000:3000 fitz-api-simple
```

### El POST devuelve 400 con `JSON inválido`

El body del POST debe ser JSON válido. El `Content-Type` también
debe ser `application/json`:

```bash
curl -X POST localhost:3000/items \
     -H 'Content-Type: application/json' \
     -d '{"id":3,"name":"thingamajig","quantity":75}'
```

Si omitís `Content-Type`, Fitz rechaza con 415 Unsupported Media
Type.

### El POST devuelve 400 con `field "id" requerido`

`type Item` declara `id: Int` y `name: Str` sin default — son
required. `quantity` tiene `= 0` así que es opcional. Si tu body
omite `id` o `name`, el deserializador rechaza con 400.

### El `docker build` se cuelga en el step `fitz build`

Si tarda más de 5 min, probablemente cargo está compilando deps
HTTP desde cero (axum + tokio + serde son ~200 crates). Buildkit
cache acelera builds subsiguientes — la primera vez es lenta y
es normal.

### Mac M-series: `exec format error` al `docker run`

La imagen base es Linux x64. Para correr en Mac M-series sin
tocar nada:

```bash
docker run --rm --platform linux/amd64 -p 3000:3000 fitz-api-simple
```

Multi-arch image (Linux x64 + ARM64) queda como deuda del
`release.yml` de Fitz.

## Roadmap del boilerplate

Mejoras planificadas cuando aparezca presión real:

- **DB nativa Fitz** (Fase 10): reemplazar el `let ITEMS = [...]`
  in-memory por queries Postgres con ORM declarativo.
- **JWT auth opt-in**: agregar `@authenticated` a algunos endpoints
  para mostrar el patrón típico de API protegida (ver
  [`boilerplates/api-middleware-cors/`](../api-middleware-cors/) que
  ya lo hace).
- **WebSockets opt-in**: agregar `@ws("/events")` para
  notificaciones de cambios en tiempo real (ver
  [`boilerplates/api-websocket/`](../api-websocket/)).
- **Multi-arch image** (linux/amd64 + linux/arm64) para Mac
  M-series nativo.
- **Health check endpoint** dedicado (`/healthz` con DB ping cuando
  haya DB) para Kubernetes / Cloud Run.

## Siguientes pasos

- Para auth (login + bearer tokens), mirá
  [`boilerplates/api-middleware-cors/`](../api-middleware-cors/).
- Para chat broadcast / notifications realtime, mirá
  [`boilerplates/api-websocket/`](../api-websocket/).
- Para CRUD con DB real, mirá
  [`boilerplates/api-postgres-python/`](../api-postgres-python/).
