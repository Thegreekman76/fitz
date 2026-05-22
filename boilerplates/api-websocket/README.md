# `api-websocket` — Chat broadcast tipado con `@ws` + AsyncAPI

Boilerplate de un chat realtime con **WebSockets tipados**:
- Frame text validado contra `type ChatMsg` automático (sin
  `json.loads` + Pydantic / `JSON.parse` + Zod manual).
- `WsConn<T>` con `recv` / `send` / `broadcast` / `close`.
- **AsyncAPI 3.0** auto-generado en `/asyncapi.json` (spec
  hermana de OpenAPI 3.1 para event-driven APIs).
- **Heartbeat built-in** con `@server(ws_heartbeat_secs=30)`.
- Frontend HTML+JS vanilla con `new WebSocket()` desde browser.

```text
┌──────────────────────────┐       ┌──────────────────────────┐
│ Browser (multi-tab)      │       │ Fitz WS server           │
│  http://localhost:8080   │       │  ws://localhost:43929    │
│  new WebSocket(...)      │ ◄───► │  @ws("/chat")            │
│  send / receive JSON     │       │  conn.broadcast(msg)     │
└──────────────────────────┘       └──────────────────────────┘
        ▲                                    ▲
        │                                    │
   nginx:alpine                         fitz binario
   (frontend container)                 (api container)
   puerto 8080:80                       puerto 43929:43929
```

## Qué demuestra

- **`@ws("/path")`** + **`async fn`** + **`WsConn<T>`** —
  WebSockets como ciudadanos de primera clase del lenguaje
  (Fase 9.w.2).
- **Marshaling JSON automático**: el `type ChatMsg { user, text }`
  se serializa/deserializa por frame text **sin código manual**.
  Frame inválido → `Err` del `conn.recv()` (no panic).
- **`conn.broadcast(msg)`**: envía a TODOS los clientes conectados
  al endpoint, incluido el sender (convención Socket.IO/Phoenix).
- **AsyncAPI 3.0** auto-generado en `/asyncapi.json` — consumible
  por AsyncAPI Studio + generadores de clientes JS/TS/Python/Java.
- **Heartbeat ping/pong** cada 30s para pasar de largo proxies
  idle-killers (Nginx 60s, Cloudflare ~100s, AWS ALB 60s).

## Estructura del directorio

```
api-websocket/
├── README.md
├── fitz.toml
├── src/
│   └── main.fitz                ← WS server (~50 LoC)
├── frontend/
│   ├── index.html               ← chat HTML+JS vanilla con WebSocket (~200 LoC)
│   └── nginx.conf
├── Dockerfile                   ← multi-stage: fitz builder + distroless
├── docker-compose.yml           ← api + frontend (2 services)
├── .dockerignore
└── .gitignore
```

## Prerequisitos

**Solo Docker** con Compose v2. NO necesitás Fitz instalado.

```bash
docker --version
docker compose version
```

## Paso a paso

### 1. Levantar todo

```bash
cd boilerplates/api-websocket
docker compose up --build
```

Build inicial: ~3-4 min. Output esperado:

```text
api          | 🏔️  Fitz HTTP escuchando en http://0.0.0.0:43929
api          |    WS  /chat
api          |    GET /health
api          |    GET /openapi.json  (schema autogenerado)
api          |    GET /asyncapi.json (canales WebSocket)
api          |    GET /docs          (UI Scalar)
frontend     | nginx ready on :80
```

### 2. Abrir el chat

```text
http://localhost:8080
```

Vas a ver:
- Tu nombre (pre-rellenado con `ada`, editable).
- Botón **"Conectar"** — abre el WebSocket.
- Área de mensajes (status: connected/disconnected/connecting).
- Input para escribir + Enter para enviar.

**Para ver el broadcast en acción**:
1. Abrí **2 tabs** del browser en `http://localhost:8080`.
2. En cada tab, poné un nombre distinto (`ada` y `bob` por ej).
3. Click "Conectar" en ambas.
4. Escribí un mensaje en tab 1 → aparece en tab 1 Y en tab 2.

Los mensajes de **otros** aparecen en naranja. Los **tuyos** en
azul. Los del **`system`** (welcome) en verde.

### 3. Validar el AsyncAPI schema

```bash
curl localhost:43929/asyncapi.json | python -m json.tool
```

Vas a ver el schema con:
- `channels./chat`: el endpoint WS declarado.
- `messages.ChatMsg`: el shape del frame con `user: String` y
  `text: String`.
- `operations.send_/chat` y `receive_/chat`.

Lo podés cargar en [AsyncAPI Studio](https://studio.asyncapi.com)
para verlo gráficamente o generar clientes para otros lenguajes.

### 4. Probar desde CLI con `wscat`

Si tenés `wscat` (`npm install -g wscat`):

```bash
wscat -c ws://localhost:43929/chat
> {"user":"alice","text":"hola desde wscat"}
< {"user":"system","text":"bienvenido al chat"}
< {"user":"alice","text":"hola desde wscat"}
```

Si tenés tabs del browser abiertas Y conectadas, el mensaje del
`wscat` aparece en todos ellos (broadcast).

### 5. Parar

```bash
docker compose down
```

## Auth en WebSocket — limitación del browser

El boilerplate usa **chat público sin auth**. Razón técnica: el
browser **WebSocket API no soporta headers custom** en el
constructor:

```javascript
// ✅ Esto funciona:
new WebSocket("ws://localhost:43929/chat")

// ❌ Esto NO funciona (no hay forma de pasar Authorization):
new WebSocket("ws://localhost:43929/chat", {
    headers: { Authorization: "Bearer ..." }  // ← unsupported
})
```

Workarounds reales para auth en WS desde browser:

1. **Token en query string**: `ws://localhost:43929/chat?token=...`
   y leer query params en el `@auth_provider`. Hoy Fitz lee solo
   `headers`, no query — deuda del lenguaje.
2. **Cookie-based auth**: el `/login` setea un cookie HTTP-only; el
   handshake WS lo incluye automático. Hoy Fitz `@authenticated`
   lee `Authorization` header, no cookies — deuda del lenguaje.
3. **Mensaje de auth post-handshake**: WS abre sin auth, primer
   frame envía `{"auth": "Bearer ..."}`, el handler valida con
   `jwt.decode` y cierra si falla. **Hoy se puede implementar
   manualmente** sin features extra.

Para CLI (`wscat`, clientes nativos), el patrón
`@authenticated @ws("/chat")` SÍ funciona con
`Authorization: Bearer <token>` header — el cap 29 del guide
documenta ese flow con `examples/guide/29-ws.fitz`.

## Cómo extender

### Tipos custom para diferentes tipos de mensajes

Hoy `ChatMsg` es plano (user + text). Para chat richer con tipos
de evento:

```fitz
type ChatMsg {
    kind: Str,         // "message", "joined", "left", "typing"
    user: Str,
    text: Str?,        // null para eventos sin texto
    timestamp: Int?,   // epoch ms opcional
}
```

El marshaling JSON sigue automático contra el shape nuevo.

### Validar el `Origin` del handshake

Hoy cualquier origin puede conectarse al WS. Para restringir, sumá
un middleware del HTTP del upgrade:

```fitz
fn require_origin(req: Request) -> Null {
    let origin = req.headers.get("origin")
    let allowed = ["http://localhost:8080", "https://app.example.com"]
    match origin {
        Ok(o) => {
            if (not allowed.contains(o)) {
                return 403 { "error": "origin no permitido" }
            }
        }
        Err(_) => return 403 { "error": "falta header Origin" },
    }
    return null
}

@middleware(require_origin)
@ws("/chat")
async fn chat(conn: WsConn<ChatMsg>) -> Null { ... }
```

### Rooms / channels separados

Hoy `broadcast` envía a TODOS los clientes del endpoint. Para
salas separadas (típico en chats grandes), Fitz no tiene `rooms`
built-in todavía — deuda visible. Workaround: usar el `text` para
encodear el room (`{"text": "#general:hola"}`) y filtrar en el
handler antes de broadcast. Para producción real esperar a la
feature nativa.

### Persistir history del chat

Hoy los mensajes son volátiles — al desconectarte, los pierdes.
Para persistencia, lo natural es la combinación con
[`boilerplates/api-postgres-python/`](../api-postgres-python/) —
SQLAlchemy + Postgres.

## Variables de entorno

El boilerplate no usa env vars. El port (`43929`) y heartbeat
(`30s`) están hardcoded en `@server(...)`. Para producción real,
ese día deberíamos exponerlo via env (deuda del lenguaje).

## Troubleshooting

### El botón "Conectar" del frontend dice `disconnected` inmediato

Causa probable: el server no levantó. Verificá:

```bash
docker compose ps
# Ambos servicios deben estar Up
```

Y mirá los logs:

```bash
docker compose logs api
# Debe decir "Fitz HTTP escuchando en http://0.0.0.0:43929"
```

### El frontend muestra "El server no responde"

El frontend hace health check a `GET /health` antes del WS. Si
falla, el server probablemente está caído o en otro puerto.
Revisá el `docker-compose.yml` (puerto 43929).

### Los mensajes no aparecen en otros tabs

Verificá que ambos tabs estén **conectados** (status verde).
Mensajes enviados ANTES de conectar el otro tab NO se replay —
no hay history (deuda).

### `WebSocket connection failed: 1006` en consola del browser

Error genérico de WS — el handshake falló o el server cerró
abruptamente. Mirá los logs del api:

```bash
docker compose logs api
```

Si ves "address already in use" → otro proceso en :43929; usa
`docker compose down` antes de levantar.

### Mac M-series: `exec format error`

Agregá `platform: linux/amd64` al service `api` del
`docker-compose.yml`. Multi-arch image es deuda del `release.yml`
de Fitz.

## Roadmap del boilerplate

- **Auth en WS desde browser**: cuando Fitz tenga `req.query` o
  cookie-based `@auth_provider`, agregar `@authenticated @ws`.
- **Rooms/channels**: built-in del lenguaje cuando aparezca demanda.
- **History persistente**: combinable con
  `api-postgres-python/` cuando llegue Fase 10 con DB nativa.
- **Reconnect con state replay**: el browser drop a veces; el
  server podría re-enviar últimos N mensajes al reconectar
  (deuda).
- **Binary frames** (`Vec<u8>`): para audio/video streaming
  (deuda Fase 9.w iteración 2).
- **AsyncAPI UI** equivalente al `/docs` de OpenAPI (hoy solo
  JSON).

## Siguientes pasos

- [`boilerplates/api-middleware-cors/`](../api-middleware-cors/) —
  auth real con JWT + CORS (sin WebSocket).
- [`boilerplates/api-postgres-python/`](../api-postgres-python/) —
  CRUD con DB persistente via interop Python.
- Cap 29 de la guía: detalle completo de WebSockets tipados con
  ejemplo `examples/guide/29-ws.fitz`.
