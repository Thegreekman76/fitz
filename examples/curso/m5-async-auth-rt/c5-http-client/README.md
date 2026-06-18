# M5.C5 — HTTP client outbound (capstone integrador del módulo M5)

Capstone ejecutable del cap **[M5.C5 — HTTP client outbound](../../../../docs/curso/m5-async-auth-rt/c5-http-client.md)**.

Combina TODO el stack del módulo M5 (async + auth + jobs + HTTP
client outbound) en una app chica que simula una **API de
notificaciones admin**:

| Endpoint | Auth | Qué hace |
|---|---|---|
| `GET /healthz` | público | keep-alive trivial del runtime |
| `POST /notify` | `@admin` (Bearer JWT) | dispatcha webhook upstream con `spawn`, responde 202 inmediato |
| `@cron("*/30 * * * * *")` | — | hace `HEAD` al upstream cada 30s y loguea status + duration |

Lo que muestra:

- **C1** `async fn` + `.await` en todos los handlers + cliente HTTP.
- **C2** `@auth_provider` que decodifica JWT con `jwt.decode` y
  devuelve el `User` tipado; `@admin` valida `user.role == "admin"`.
- **C4** `@cron` para health checks periódicos + `@background +
  spawn(...)` para el webhook fire-and-forget.
- **C5** `http.head` (health checks) + `http.post(url, Map<...>)`
  (webhook dispatch con auto-JSON) + `http.request({...})`
  low-level con `timeout_ms` + modelo de errores `Result<T>` con
  `match` exhaustivo.

Cero deps externas (ni `requests`, ni `axios`, ni `reqwest`).
Cero `pip install`/`npm install`/`cargo add`. Todo built-in del
lenguaje. Compilable a binario standalone con `fitz build`.

## Cómo correr

```bash
# 1. Arrancar el server (intérprete o binario).
fitz run examples/curso/m5-async-auth-rt/c5-http-client/app.fitz

# Output esperado en stdout:
#   🔑 TOKEN admin de prueba: eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...
#   Fitz server arrancado en 127.0.0.1:43955
#   🕐 Fitz scheduler arrancado con 1 job(s) cron
#      @cron  check_upstream (*/30 * * * * *)
```

En otra terminal:

```bash
# 2. Health check público.
curl http://localhost:43955/healthz
# → ok

# 3. Notify sin token: 401.
curl http://localhost:43955/notify
# → {"error": "falta header Authorization"}

# 4. Notify con token admin: 202 + webhook fire-and-forget.
TOKEN="<el token que imprimió el boot>"
curl -X POST http://localhost:43955/notify \
     -H "Authorization: Bearer $TOKEN" \
     -H "Content-Type: application/json" \
     -d '{"event":"signup","user":"ada"}'
# → {"status":"accepted","event":"signup","by":"ada"}
```

Mientras tanto, el log de stderr muestra:

```text
{"level":"info","msg":"notify recibido","by":"ada","event":"signup"}
{"level":"info","msg":"dispatching webhook","event":"signup","user":"ada"}
{"level":"info","msg":"webhook dispatched","event":"signup","status":200,"duration_ms":312}
```

Y cada 30 segundos, el cron tickea:

```text
{"level":"info","msg":"upstream health OK","url":"https://httpbin.org/status/200","status":200,"duration_ms":189}
```

## Validación bit-a-bit `fitz run` ↔ `fitz build`

El programa es **standalone-compilable**. Para validar paridad
bit-a-bit:

```bash
# Compilar al binario nativo.
fitz build examples/curso/m5-async-auth-rt/c5-http-client/app.fitz

# Correr el binario (mismo stdout, mismos logs).
./examples/curso/m5-async-auth-rt/c5-http-client/app.exe   # Windows
./examples/curso/m5-async-auth-rt/c5-http-client/app       # Linux/macOS

# El binario standalone tiene reqwest + rustls + tokio + axum
# + jsonwebtoken + argon2 + cron + chrono linkeados estático.
# Cero deps en el sistema destino (ni Python, ni Node, ni JVM).
```

## Variables de entorno

En producción, **no hardcodees** `JWT_SECRET` ni `WEBHOOK_URL`
en el código. Usá `env_or(...)` con defaults para dev:

```fitz
let JWT_SECRET: Str = env_or("JWT_SECRET", "demo-secret-cambialo-en-prod")
let WEBHOOK_URL: Str = env_or("WEBHOOK_URL", "https://httpbin.org/post")
```

Para mantener este ejemplo simple y self-contained, los dejamos
hardcodeados con valores de demo.

## Próximo paso

Cerraste M5. Arrancá [M6 — Postgres + ORM nativo](../../../../docs/curso/m6-postgres-orm/c1-setup-driver-crudo.md).
