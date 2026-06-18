# Roadmap — HTTP client builtin (módulo `http`)

> Plan dedicado para implementar el módulo `http` builtin (cliente
> HTTP outbound). Detectado como deuda explícita el 2026-06-18
> durante el desarrollo de **fitzwatch** (status page open-source
> escrito en Fitz puro), que necesita hacer GET/HEAD outbound para
> chequear si las URLs monitoreadas responden 200.
>
> Branch de trabajo: **directo en main** del repo Fitz.
> Tiempo estimado: **6-8 horas focused, ~5-7 commits chicos**.

---

## 1. Contexto y motivación

**Estado actual del lenguaje**: Fitz tiene HTTP **server-side**
nativo y ciudadano de primera (`@get`/`@post`/`@put`/`@delete` +
`@server` + middleware + auth + WebSockets + OpenAPI auto). Pero
**no tiene HTTP client outbound**.

**Builtins disponibles hoy**: `print`/`len`/`bytes`/`cors`/
`sleep`/`ws_broadcast`/`spawn`/`env*`/`secret`/`config`/`flag`/
`flags`/`assert*` + módulos `jwt`/`hash`/`log`/`db`/`auth`/`flags`.

**Casos bloqueados sin `http` builtin**:
- Webhooks que despachan a destinos externos.
- Scraping / integración con APIs externas (Stripe, GitHub, etc.).
- Health checks de servicios externos (use case canónico fitzwatch).
- Proxying / aggregation de upstream APIs.

**Workaround actual**: interop Python con `urllib.request` o pip
`requests`. Funciona pero contradice el modelo "todo nativo en el
core" del lenguaje y agrega ~20-25 MB al binario via
`--bundle-python`.

---

## 2. API target (lo que el usuario va a escribir en Fitz)

```fitz
// Métodos comunes — devuelven Future<Result<HttpClientResponse>>
let r = http.get("https://api.example.com/data").await?
let r = http.head("https://example.com").await?
let r = http.post("https://api.example.com/items", body).await?
let r = http.put("https://api.example.com/items/42", body).await?
let r = http.delete("https://api.example.com/items/42").await?

// body acepta:
//   - Str          → enviado as-is, sin tocar headers
//   - Map<Str,Any> → serializa a JSON + agrega Content-Type: application/json
//   - Bytes        → enviado as-is, sin tocar headers

// Versión low-level con opciones
let r = http.request({
    "method": "GET",                    // obligatorio
    "url": "https://api.example.com",   // obligatorio
    "timeout_ms": 5000,                 // opcional, default 30000
    "headers": {"X-Token": "abc"},      // opcional
    "body": "...",                      // opcional (Str | Map | Bytes)
    "follow_redirects": true,           // opcional, default true
}).await?

// Tipo built-in nuevo
type HttpClientResponse {
    status: Int                         // ej. 200
    body: Str                           // texto plano del body
    headers: Map<Str, Str>              // headers de la response
    duration_ms: Int                    // tiempo total medido en cliente
}
```

### Decisiones de API acordadas con el autor (2026-06-18)

- **Métodos**: API completo desde día 1 (`get + head + post + put
  + delete + request`). **No MVP recortado** a solo `get`/`head`.
- **Body shapes**: 3 desde día 1 (`Str` + `Map` auto-JSON +
  `Bytes`). No empezar solo con `Str`.
- **Errores**: `Result::Err(Str)` con mensaje claro tipo
  `"timeout después de Nms"`, `"DNS no resuelve: <host>"`, etc.
  No estructurar el error con type custom — paralelo a `db.X`.
- **Headers de la response**: `Map<Str, Str>` (no `List<(Str,Str)>`).
  Si un header viene duplicado, gana el último (decisión MVP,
  refinable si entra demanda real).
- **TLS**: rustls (no openssl) — mismo stack que Fase 10.1.b.

---

## 3. Implementación por bloques

Un commit por bloque. Convención `mini-fase HTTP client`.

### Bloque 1 — Evaluator (intérprete primero)

**Archivos**: `src/evaluator.rs`, posiblemente nuevo
`src/http_client.rs`.

- **1.1** Registrar `Value::Module { name: "http", ... }` en
  `register_builtins()` (paralelo a `jwt`/`hash`/`log`/`db`/
  `auth`/`flags`).
- **1.2** Implementar los 6 builtins async:
  - `builtin_http_get(url) -> Future<Result<HttpClientResponse>>`
  - `builtin_http_head(url) -> ...`
  - `builtin_http_post(url, body) -> ...` (body acepta Str/Map/Bytes)
  - `builtin_http_put(url, body) -> ...`
  - `builtin_http_delete(url) -> ...`
  - `builtin_http_request(opts: Map) -> ...` (low-level con
    method/url/timeout_ms/headers/body/follow_redirects)
- **1.3** Pre-registrar tipo `HttpClientResponse` como nominal con
  fields `status: Int`, `body: Str`, `headers: Map<Str, Str>`,
  `duration_ms: Int`. Sigue patrón de `Request`/`Response`
  built-in del HTTP server-side.
- **1.4** Usar `reqwest::Client` (ya viene como dep transitiva de
  `opentelemetry-otlp`). Si hace falta, sumarlo como dep directa
  no condicional en `Cargo.toml` con features
  `["json", "rustls-tls"]`.
- **1.5** Helper privado `body_to_reqwest_body(value)` que
  dispatcha por tipo del Value:
  - `Value::Str(s)` → `body(s)` sin tocar headers.
  - `Value::Map(m)` → `serde_json::to_string(m)?` +
    `Content-Type: application/json`.
  - `Value::Bytes(b)` → `body(b)` sin tocar headers.
  - Otro tipo → `FitzError` claro citando los 3 shapes válidos.
- **1.6** Headers de salida: pasar el `Map<Str, Str>` del `opts`
  a `RequestBuilder::headers()`.
- **1.7** Medir `duration_ms` con `std::time::Instant::now()`
  antes del send y al recibir response.
- **1.8** Errores: timeout, conexión fallida, DNS, TLS handshake
  fallido, etc. → `Result::Err(Value::Str(mensaje))` con prefijo
  identificable. Ejemplos:
  - `"timeout después de 5000ms"`
  - `"no se pudo resolver el host: api.example.com"`
  - `"TLS handshake falló: <detalle>"`
  - `"URL inválida: <detalle>"`
- **1.9** Tests unit: stubear con `wiremock` (Rust) o spawn un
  servidor axum local de prueba para validar:
  - Shape de la response (`HttpClientResponse` con los 4 fields).
  - 200/404/500 propagados como `Ok` con `status` correcto (NO
    como `Err` — solo errores de transporte van a `Err`).
  - Headers de request enviados llegan al server.
  - Body Str/Map/Bytes serializado correctamente.
  - Timeout dispara `Err`.
  - `follow_redirects=false` deja al user ver el 301/302.

### Bloque 2 — Checker estático

**Archivos**: `src/types.rs`, `src/lsp.rs` (parte de imports).

- **2.1** Pre-registrar `http` en `CheckCtx::new()` como
  `Type::Any` en el scope base (paralelo a `jwt`/`hash`/`auth`).
- **2.2** Pre-registrar tipo `HttpClientResponse` en `TypeEnv`
  como Nominal con sus 4 fields.
- **2.3** Tests del checker:
  - `let r = http.get("...").await?` tipa como
    `HttpClientResponse`.
  - `r.status` tipa como `Int`, `r.body` como `Str`,
    `r.headers` como `Map<Str, Str>`, `r.duration_ms` como `Int`.
  - `http.get(123)` (url no-Str) NO falla en el checker en este
    MVP (sigue siendo `Any` el módulo). Refinable post-MVP si
    queremos signatures estrictas (paralelo a deuda de
    `jwt`/`hash`).

### Bloque 3 — Codegen (paridad bit-a-bit)

**Archivos**: `src/codegen.rs`.

- **3.1** Detector `program_uses_http_client(program) -> bool`
  walka AST buscando `Expr::Call` con `callee = Expr::Field {
  obj: Ident("http"), .. }`. Análogo a `program_uses_db` /
  `program_uses_prometheus_export`.
- **3.2** `cargo_toml_for(...)` gana parámetro
  `uses_http_client: bool`. Si true, suma `reqwest = "0.12"`
  con features `["json", "rustls-tls"]` no condicional.
  Propagar a los ~20 call sites de tests (paralelo a lo que se
  hizo en v0.13.1 con `uses_prometheus_export`).
- **3.3** Nuevo preludio `HTTP_CLIENT_PRELUDE` con:
  - `struct __FitzHttpClientResponse { status, body, headers,
    duration_ms }` + impls `Display`, `__ToFitzJson`,
    `__FromFitzJson`, `PartialEq`.
  - `static __FITZ_HTTP_CLIENT: LazyLock<reqwest::Client>` con
    config default (timeout=30s, follow_redirects=true).
  - Helpers async `__fitz_http_get(url) -> Result<...>`,
    `__fitz_http_post(url, body)`, etc.
  - Helper `__fitz_http_body_apply(builder, body_value)` que
    hace el dispatch Str/Map/Bytes igual que el intérprete.
- **3.4** Dispatch en `gen_call(...)`: cuando el callee es
  `http.X(...)`, emitir la llamada al helper correspondiente.
- **3.5** Importación cross-module: `use crate::{__fitz_http_*}`
  en módulos importadores cuando detectan uso.
- **3.6** Tests del codegen:
  - Unit: `program_uses_http_client` detecta los 6 builtins,
    Cargo.toml emite la dep solo cuando true, preludio emitido
    condicionalmente.
  - E2E (`tests/compile_e2e.rs`): paridad bit-a-bit con el
    intérprete contra un servidor axum local de prueba spawneado
    en el test. Cubre GET 200, POST con Map body, error de
    transporte → Err.

### Bloque 4 — LSP

**Archivos**: `src/lsp.rs`, `editors/vscode/syntaxes/fitz.tmLanguage.json` (validación, sin cambios esperados).

- **4.1** `scope_level_completions` suma `http` con descripción
  `"module: get/post/put/delete/head/request (HTTP client async)"`.
- **4.2** `after_dot_completions` (cuando `recv_name == "http"`)
  tira los 6 métodos con signatures completas + ejemplos del hint:
  - `get(url: Str)` → `"GET request. Devuelve Future<Result<HttpClientResponse>>"`
  - `head(url: Str)` → `"HEAD request (sin body). Útil para health checks"`
  - `post(url: Str, body: Str|Map|Bytes)` → `"POST request"`
  - `put(url: Str, body: Str|Map|Bytes)` → `"PUT request"`
  - `delete(url: Str)` → `"DELETE request"`
  - `request(opts: Map)` → `"Low-level con method/url/timeout_ms/headers/body/follow_redirects"`
- **4.3** Tests LSP: completions del módulo + after-dot resuelve
  los 6 métodos (paralelo a tests existentes de `jwt`/`hash`).
- **4.4** Grammar TextMate: validar que no hay cambios necesarios
  (los módulos caen bajo identifier genérico). Si hace falta tocar,
  documentarlo como sub-paso.

### Bloque 5 — Guía + ejemplos runnable

**Archivos**: `docs/guide.md`, `examples/guide/`.

#### 5.1 Sub-sección nueva en cap 17 de `docs/guide.md`

Estructura `"17.X — HTTP client outbound"` (entre la sub-sección
final actual y el cierre del cap), siguiendo política de la memoria
`feedback_guide_emphasize_uniqueness`:

- **Panorama vecino**: tabla comparativa con `requests`/Python
  (lib externa, sync), `axios`/JS (lib externa, Promise),
  `reqwest`/Rust (lib externa, async), `OkHttp`/Java (lib
  externa, sync por default), Fitz (built-in del lenguaje,
  async ciudadano).
- **Por qué Fitz hace esto distinto** (5 diferenciales):
  1. **Built-in del lenguaje**, no lib externa — paralelo a
     HTTP server-side, `db`, `auth`, `log`.
  2. **Paridad bit-a-bit `fitz run` ↔ `fitz build`** — el
     binario standalone tiene el cliente HTTP linkeado, no
     necesita CPython embebido ni runtime de Node.
  3. **Async ciudadano de primera** — devuelve `Future<T>`,
     se integra natural con `@cron`/`@background`/handlers HTTP.
  4. **`Result<T>` automático** — errores de transporte son
     valores; `?` los propaga; el checker exige manejo (regla
     5.3.3).
  5. **Sin deps externas en el binario final** — `reqwest`
     queda linkeado estáticamente con `rustls-tls`, no hace
     falta openssl en el host de runtime.
- **API completo** con los 5 métodos comunes + `request`
  low-level.
- **Body shapes** Str/Map/Bytes con tabla explícita:

  | Shape Fitz | Wire body | Content-Type auto |
  |---|---|---|
  | `Str` | as-is UTF-8 | no se toca |
  | `Map<Str, Any>` | `serde_json::to_string(m)` | `application/json` |
  | `Bytes` | as-is octetos | no se toca |

- **Manejo de errores con `match` o `?`** — ejemplo con un
  match que distingue timeout / DNS fail / status 4xx (este
  último NO es Err, hay que mirar `r.status`).
- **Integración con el resto del lenguaje** — sub-bloques chicos:
  uso adentro de `@background async fn`, `@cron`, handler HTTP
  que proxea a upstream, `spawn(http.get(...))` para
  fire-and-forget.
- **Limitaciones del MVP** (link a sec. 4 de este roadmap):
  no stream del body, no multipart, no cookie jar persistente.

#### 5.2 Ejemplos runnable

Crear los 4 con numeración consistente con el cap 17 (queda por
decidir si `17c`, `17d`, etc. o un sub-dir `examples/guide/17-http-client/`).
Cada uno suma al smoke `GUIDE_EXAMPLES_COMPILE`.

- **`17c-http-client-basico.fitz`** — los 5 métodos comunes
  contra `httpbin.org` (o servidor axum local que el smoke
  arranca antes de compilar): `GET /get`, `HEAD /status/200`,
  `POST /post` con Map body, `PUT /put` con Str body,
  `DELETE /delete`. Muestra `r.status`, `r.duration_ms`,
  inspección del JSON echo. < 60 LoC.
- **`17d-http-client-errores.fitz`** — manejo completo:
  timeout (`timeout_ms=1`), URL inválida, host inexistente,
  status 404/500 que NO son Err (chequear `r.status`).
  Patrón canónico con `match r { Ok(resp) => ..., Err(e) => ... }`
  + chequeo de `resp.status >= 400`. < 80 LoC.
- **`17e-http-client-webhook.fitz`** — handler HTTP `@post
  /events` recibe un payload y lo despacha a un webhook upstream
  con `@background async fn dispatch_webhook(url, payload)` +
  `spawn(...)`. Caso real de webhook dispatcher. Combina HTTP
  server-side + HTTP client + `@background` + `spawn`. < 100 LoC.
- **`17f-http-client-health-checker.fitz`** — `@cron("*/30
  * * * * *") async fn check_all_endpoints()` que recorre una
  lista de URLs y hace `http.head` con timeout corto. Caso real
  fitzwatch-style chiquito. Combina `@cron` + HTTP client +
  `log.info` estructurado. < 80 LoC.

**Decisión de smoke**: si `httpbin.org` no es viable en CI por
red outbound restringida, el smoke arranca un servidor axum local
mínimo (paralelo a lo que ya hace `tests/compile_e2e.rs` para
ejemplos HTTP server-side) y los ejemplos apuntan a
`http://127.0.0.1:<port>`. Documentar la decisión en el cap 17.

### Bloque 6 — Barrida cross-docs

**Archivos**: `CLAUDE.md`, `README.md`, `docs/index.md`,
`docs/architecture.md`, `docs/syntax-spec.md`, `docs/deudas-post-5b.md`,
`mkdocs.yml` (si la sub-sección 17.X aparece en el TOC del sitio).

- **6.1** `CLAUDE.md`: bullet en "Estado actual del proyecto"
  con la mini-fase HTTP client cerrada + bullet en
  "Qué funciona hoy" con la lista de los 6 builtins. Match
  con el patrón de releases anteriores (ej. v0.12.0
  observability).
- **6.2** `README.md`: fila nueva en la tabla feature
  comparativa con marca propia (ej. ♣ o ♥) — "HTTP client
  outbound ✅ built-in del lenguaje" vs "lib externa" para
  Python/JS/Java/Rust. Footnote dedicada con los 5
  diferenciales del cap 17.X.
- **6.3** `docs/index.md`: fila paralela al README en el cuadro
  comparativo del landing del sitio (ya tiene una análoga para
  observability OTel, auth nativa, WS tipados, jobs sin Celery).
- **6.4** `docs/architecture.md`: si documenta builtins, sumar
  bullet del módulo `http` paralelo a `jwt`/`hash`/`log`/`db`/
  `auth`. Si no los enumera, no tocar.
- **6.5** `docs/syntax-spec.md`: si lista módulos built-in en
  algún lugar, sumar `http`. Si no, no tocar.
- **6.6** `docs/deudas-post-5b.md`: nota de cierre paralela a
  las de otras mini-fases (ej. "🟢 Mini-tanda HTTP client builtin
  CERRADA vX.Y.Z") con resumen 1-párrafo + link al CHANGELOG +
  link a este roadmap.
- **6.7** `mkdocs.yml`: si la sub-sección 17.X necesita anchor
  propio en la nav, sumarlo. Probablemente no — sub-secciones
  caen bajo el cap 17 padre.

### Bloque 7 — Curso `Fitz de 0 a experto` (decisión)

Memoria `project_curso_plan.md`: M1-M8 cerrados. El módulo M3
("HTTP, REST y backend") tiene capítulos sobre HTTP server-side
pero no client.

**Opciones**:
1. **Capítulo nuevo M3.CX** dedicado al HTTP client (paralelo
   a los caps de auth/WS/cron). +1 ejemplo en
   `examples/curso/m3-http/`. ~1-2h adicionales.
2. **Mención + ejemplo en M4 o M5** (integración con jobs/DB)
   sin cap dedicado. Más liviano.
3. **Deuda explícita** documentada en `docs/curso-plan.md`
   para sumar después si el curso gana tracción internacional.

**Recomendación**: opción 3 al cerrar el MVP, opción 1 si el
autor confirma que quiere darle peso pedagógico desde día uno.
Decidir al arrancar Bloque 7.

### Bloque 8 — Boilerplates (decisión)

Memoria `project_boilerplates`: 6 boilerplates Dockerizados
cerrados. Ninguno usa HTTP client outbound hoy.

**Opciones**:
1. **Boilerplate nuevo `api-webhook-dispatcher`** que muestre
   el patrón canónico HTTP server-side + HTTP client + jobs
   `@cron`/`@background`. Showcase del stack completo. ~2-3h.
2. **Update de algún boilerplate existente** (ej.
   `api-orm-full`) para sumar 1-2 endpoints que usen
   `http.get/post` (proxying a upstream API). ~30min.
3. **Deuda explícita** post-MVP.

**Recomendación**: opción 2 (update chico) — paralelo a lo que
hicimos cuando cerramos otros builtins. Decidir al arrancar
Bloque 8.

### Bloque 9 — Cierre formal

- **9.1** CHANGELOG nueva versión (probablemente `v0.17.0` por
  feature nueva del lenguaje, o `v0.16.x` si lo encajamos como
  patch — decisión al cerrar).
- **9.2** `docs/roadmap.md`: entrada nueva (`"Mini-tanda — HTTP
  client builtin CERRADA"`) — el grueso ya está en este doc,
  el roadmap solo apunta acá.
- **9.3** Este mismo doc: marcar bloques con ✅ + commit SHA a
  medida que cierren.
- **9.4** Smoke completo: ~360+ ejemplos `GUIDE_EXAMPLES_COMPILE`,
  fmt (`cargo fmt --all --check`), clippy
  (`cargo clippy --all-targets -- -D warnings` en los 3 modos:
  default, `python`, `lsp`), todos los tests (unit + cli_e2e +
  compile_e2e + openapi).
- **9.5** Extensión VSCode: bump de version en `package.json`,
  rebuild `.vsix` con `npm run build:vsix`, validación manual
  de las completions nuevas del módulo `http`.
- **9.6** Verificación pre-bump completa siguiendo memoria
  `feedback_pre_release_verification` (checklist exhaustivo:
  roadmap + guide.md + deudas + CLAUDE + CHANGELOG + README +
  index.md + extensión VSCode grammar+LSP+walkers + examples +
  boilerplates + curso + fmt + clippy + smoke).
- **9.7** **Release con tag**: pasar mensaje de commit al
  usuario para que cree el tag `vX.Y.Z`; el workflow `release.yml`
  multi-plataforma (memoria `project_release_workflow`) compila
  los 3 artefactos juntos (fitz + fitz-lsp + .vsix).
- **9.8** **Entrada nueva para dev.to**: post anunciando el
  HTTP client outbound nativo. Cubre los 5 diferenciales (del
  sub-bloque 5.1), los 4 ejemplos runnable del Bloque 5,
  comparación side-by-side con `requests`/Python y `axios`/JS
  (mismo task en las 3 herramientas), y link al cap 17.X de la
  guía + al repo. Tono casual, código realista, sin marketing
  flowery. Borrador en `docs/blog/<fecha>-http-client-nativo.md`
  para revisión antes de publicar.

---

## 4. Deudas residuales conocidas que NO bloquean este MVP

Documentar como sub-paso futuro si aparece demanda real:

- **Stream del body** (response no se carga entera en memoria,
  para descargar archivos grandes). API: `r.body_stream() ->
  AsyncIterable<Bytes>`. Hoy `body: Str` carga todo en RAM.
- **Multipart form-data** para upload de archivos. Requiere
  feature `multipart` de `reqwest` + API dedicada.
- **Cookie jar** automático (persistencia entre requests).
- **Connection pooling configurable** (max connections per host,
  idle timeout, etc.). Hoy `reqwest::Client` con defaults.
- **HTTP/2 push, HTTP/3 (QUIC)** — `reqwest` los soporta con
  features extra, no MVP.
- **Proxy support** (`HTTP_PROXY`/`HTTPS_PROXY` env vars).
  `reqwest` lo respeta por default pero no está documentado.
- **Custom TLS config** (client cert, CA pinning).
- **Signature estricta del módulo `http`** en el checker (hoy
  `Type::Any`, igual que `jwt`/`hash`). Refinable si entra demanda.
- **Headers como `List<(Str,Str)>`** para preservar orden y
  duplicados (hoy `Map<Str,Str>` gana el último). Refinable.

---

## 5. Cómo retoma fitzwatch después de cerrar esto

Cuando el módulo `http` esté en `main` de Fitz:

1. `cd d:\fitzwatch && fitz check` — debería seguir pasando OK.
2. Crear `src/checks.fitz` con el runner del check HTTP:
   - `async fn run_http_check(monitor) -> CheckResult`:
     `http.head(monitor.target)` con `timeout_ms =
     monitor.timeout_ms`. Si OK + status matchea `expected_status`
     (o ∈ [200,299] si null) → `"up"`. Si no → `"down"` con error.
   - `@background async fn run_check(monitor_id: Int)`: lookup
     del Monitor, dispatch, persiste en DB, actualiza
     `last_check_at` / `last_status`, calcula transition para
     abrir/cerrar `Incident`.
3. `src/scheduler.fitz` con `@cron("*/10 * * * * *", store=db,
   retry={...})` que escanea due monitors y dispara `spawn(
   run_check(m.id))`.
4. `src/public.fitz` — `GET /public/status` sin auth.
5. `src/realtime.fitz` — `@ws("/ws/dashboard")` autenticado.
6. `src/notifications.fitz` — email + webhook (post-MVP básico).
7. Frontend vanilla en `frontend/`.
8. `fitz docker init` + revisar compose.
9. Deploy al VPS siguiendo `d:\fitzwatch\deploy\README.md`.

Detalle completo del plan de fitzwatch en
`d:\fitzwatch\NEXT-SESSION.md` sección 4.

---

## 6. Chequeo de regresiones (regla durante toda la mini-fase)

Memoria `feedback_post_changes_smoke_examples_boilerplates`: toda
mini-fase que toque runtime/codegen/builtins exige sub-paso de
revisión de regresiones ANTES de cerrar el bloque.

**Bloques afectados**: 1 (evaluator), 2 (checker), 3 (codegen).
Los bloques 4-8 son LSP/docs/ejemplos/boilerplates — la regresión
se valida al final del bloque que cierran.

**Checklist por bloque que toca runtime/codegen/builtins**:

- **R1 — Smoke `GUIDE_EXAMPLES_COMPILE`** verde (~360+ ejemplos
  de la guía + curso + TaskHub). Tiempo ~7 min. Es la red de
  seguridad principal.
- **R2 — Tests unit completos** verdes en los 3 modos:
  - `cargo test` (default, sin features).
  - `cargo test --features lsp`.
  - `cargo test --features python` (si la PC tiene CPython
    real linkeable; si no, dejar nota y cubrirlo en CI).
- **R3 — Tests E2E**: `cli_e2e` + `compile_e2e` + `openapi_e2e`
  verdes. Los 8 failures pre-existentes de `compile_e2e`
  (documentados en `docs/deudas-post-5b.md` post-v0.16.0)
  siguen tolerados, pero cero **nuevas** failures.
- **R4 — Lints estrictos**: `cargo fmt --all --check` +
  `cargo clippy --all-targets -- -D warnings` limpios.
- **R5 — Boilerplates** (memoria `project_boilerplates` +
  `project_boilerplates_orm_plan`): los 6 boilerplates base + 9
  ORM siguen compilando con `fitz check` y `fitz build`. Smoke
  rápido: barrida automatizada que recorre
  `boilerplates/*/src/main.fitz` y corre `fitz check`. No hace
  falta el Docker build completo en cada bloque — eso queda
  para el cierre formal (9.4).
- **R6 — Ejemplos del curso** (`examples/curso/m*/*.fitz`) +
  TaskHub (`examples/taskhub/*.fitz`): `fitz check` sobre todo
  el árbol. Cubierto parcialmente por R1 (el smoke gigante los
  incluye).
- **R7 — Validación bit-a-bit `fitz run` ↔ `fitz build`** sobre
  los nuevos ejemplos `17c-17f` del HTTP client. Output idéntico
  entre intérprete y binario nativo, contra el mismo servidor de
  prueba.
- **R8 — Extensión VSCode** (memoria
  `feedback_vscode_extension_workflow`): tras cada cambio del
  lenguaje (mini-tanda / sub-paso / deuda), verificar:
  - **Grammar TextMate** (`editors/vscode/syntaxes/fitz.tmLanguage.json`):
    el módulo `http` cae bajo identifier genérico, no espera
    cambios; confirmar que el highlighting de `http.get(...)`
    se ve correcto adentro del cap 17 de la guía abierto en
    VSCode con la extensión instalada.
  - **LSP autocomplete**: en VSCode con la extensión cargada,
    tipear `http.` dispara las 6 completions con descripciones
    correctas (paralelo a verificación manual hecha en releases
    de `auth`, `flags`, `log`).
  - **Walkers/hover**: hover sobre `http.get(...)` muestra
    signature correcta; go-to-definition sobre `HttpClientResponse`
    funciona; diagnostics aparecen ante uso inválido (ej.
    `http.get(123)` si decidimos refinar el checker a strict).
  - **`.vsix` rebuild**: `cd editors/vscode && npm run build:vsix`
    + instalación local (`code --install-extension fitz-language-*.vsix
    --force`) + smoke manual del archivo nuevo.
  - **Sin cerrar el bloque** hasta que el `.vsix` esté
    regenerado y validado a mano contra los ejemplos
    `17c-17f`.

**Política**: si algún bloque (1/2/3) rompe alguno de R1-R7, NO
se cierra ni se commitea hasta que esté verde. Diagnosticar root
cause, fixear, re-verificar. Sin shortcuts del estilo "lo cierro
y arreglo después" — esa es la regla del proyecto.

**Cierre formal (Bloque 9)** suma encima de esto:
- Smoke con red real outbound (si los ejemplos apuntan a
  `httpbin.org`) o con servidor axum local del smoke.
- Build completo de los 15 boilerplates con `fitz build`
  (no solo `fitz check`).
- TaskHub end-to-end con `docker compose up` y curl manual a
  los endpoints (validación humana, ~5 min).

---

## 7. Estado de bloques

| Bloque | Estado | Commit | Notas |
|--------|--------|--------|-------|
| 1. Evaluator (intérprete) | ⏳ Pendiente | — | + R1-R7, R8 |
| 2. Checker estático | ⏳ Pendiente | — | + R1-R7, R8 |
| 3. Codegen (paridad bit-a-bit) | ⏳ Pendiente | — | + R1-R7, R8 |
| 4. LSP | ⏳ Pendiente | — | + R1-R4, **R8 obligatorio** |
| 5. Guía + ejemplos runnable | ⏳ Pendiente | — | + R1, R7, R8 (smoke manual con `.vsix`) |
| 6. Barrida cross-docs | ⏳ Pendiente | — | docs only |
| 7. Curso (decisión + ejecución si va) | ⏳ Pendiente | — | docs/ejemplos |
| 8. Boilerplates (decisión + ejecución si va) | ⏳ Pendiente | — | + R1-R5 |
| 9. Cierre formal + release | ⏳ Pendiente | — | Verificación pre-bump completa + `.vsix` bump final |

Actualizar a ✅ + commit SHA a medida que cierren.
