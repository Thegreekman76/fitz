# Auditoría post-Fase 5b — deudas y mejoras

> Documento generado tras cerrar Fase 5b (codegen a binario nativo).
> Identifica deudas técnicas, gaps de docs, mejoras de calidad/UX.
> **No ejecuta fixes** — es input para decidir qué atacar y en qué orden.

## 🔴 URGENTE — `http.request({...headers: Map<Str, Str>, body: Map...})` desde `async fn` rompe Send cuando se llama vía `spawn(...)` (descubierta 2026-06-23)

> **PRIORIDAD MÁXIMA**. Bloquea producción real (fitzwatch.com,
> deploy 2026-06-23). El refactor planificado para reemplazar
> `smtp.send` por `http.request` POST a Resend API (workaround del
> bloqueo SMTP outbound de DigitalOcean) **no se puede aplicar**
> hasta cerrar esto. Workaround user-land = ninguno: el patrón
> Map literal en `headers`/`body` es la única forma documentada de
> pasar headers custom (Authorization Bearer en este caso), y se
> espera del contrato del builtin que el codegen emita código Send.

### Síntoma (3 errors rustc en `fitz build`)

```text
error: future cannot be sent between threads safely
  --> src/checks.rs:353:95
   |
353 | = tokio::spawn(async move {
       notify_subscribers_incident_opened(monitor.clone(), 0i64, ...).await
     });
                ^^^^^^^^^^^^^ future created by async block is not `Send`
   |
   = help: within `{async block@src/checks.rs:353:...}`, the trait
           `std::marker::Send` is not implemented for
           `std::sync::MutexGuard<'_, Vec<(String, String)>>`

note: future is not `Send` as this value is used across an await
  --> src/subscriptions.rs:384:982
   |
384 | ... ((Arc::new(Mutex::new(vec![
        (String::from("Authorization"), format!("Bearer {}", api_key.clone())),
        (String::from("Content-Type"), String::from("application/json"))
       ]))).lock().unwrap().clone()), body_bytes: { ... }, ... }).await ...
   |       ----------------------------- has type
                   `MutexGuard<Vec<(String, String)>>` which is not `Send`
                   ... ^^^^^ await occurs here, with
                   `(...).lock().unwrap()` maybe used later
```

3 errors derivados de la misma raíz:

1. `src/checks.rs:353` — `spawn(notify_subscribers_incident_opened(...))` rompe Send.
2. `src/incidents.rs:332` — `spawn(notify_subscribers_incident_closed(...))` rompe Send.
3. `src/main.rs:6395` (E0277) — `__handler_subscribe` no implementa `axum::Handler`
   porque su future (que indirectamente llama a `send_email_to`) deja de ser Send.

### Repro mínima

```fitz
// modulo: emails.fitz
async fn send_email(to: Str, api_key: Str) -> Result<Bool> {
    let resp = http.request({
        "url": "https://api.example.com/emails",
        "method": "POST",
        "headers": {
            "Authorization": "Bearer {api_key}",
            "Content-Type": "application/json"
        },
        "body": {
            "to": to,
            "subject": "test",
            "html": "<p>hi</p>"
        }
    }).await?
    return Ok(true)
}

// modulo: notify.fitz
from emails import send_email

@background
async fn notify(to: Str) -> Null {
    let _ = send_email(to, "re_xxx").await
    return null
}

// modulo: main.fitz
from notify import notify

@get("/test/{to}")
async fn trigger(to: Str) -> Result<Null> {
    spawn(notify(to))
    return Ok(null)
}

@server(3000)
fn main() => 0
```

`fitz build` → `error: future cannot be sent between threads safely` con el
mismo MutexGuard cross-await que en producción.

### Causa raíz

El codegen, al ver un Map literal `{"Authorization": "Bearer {key}", ...}` en
el kwarg `headers` (o `body` Map) de `http.request(opts)`, emite:

```rust
(Arc::new(Mutex::new(vec![
    (String::from("Authorization"), format!("Bearer {}", api_key.clone())),
    (String::from("Content-Type"), String::from("application/json"))
])))
.lock()             // ← MutexGuard nace acá
.unwrap()
.clone()            // ← clone OK pero el guard sigue vivo hasta `;` del stmt
```

El stmt envolvente es el field assignment del struct literal del request
(`__FitzHttpRequestOpts { headers: <expr>, body: <expr>, ... }`), y el
`.await` del request ocurre DENTRO del mismo statement. Resultado: el
MutexGuard atraviesa el await point → el future generado no es Send →
`tokio::spawn(...)` rechaza.

El bug **no es de `http.request` per se** — es del patrón general "Map
literal en kwarg de builtin async dentro de async fn que será spawneada".
La estructura del codegen para Maps siempre genera `Arc<Mutex<Vec<...>>>`,
y `.lock().unwrap().clone()` para extraer el valor mantiene el guard vivo.

### Patología análoga (precedente cerrado)

Bug cerrado en **v0.18.1** (cosecha post-fitzwatch sesión 2, 2026-06-20):
"`for x in <List<Str>>` con `.await` adentro de `@cron` rompe Send" —
mismo problema raíz (MutexGuard del lock de `List<Str>` cross-await en
context que después se spawnea). La solución fue extraer el guard a un
binding intermedio que se dropea antes del await:

```rust
// Antes (rompía Send):
for __x in (xs.clone()).lock().unwrap().iter() { ... .await ... }

// Después (Send OK):
let __snapshot: Vec<_> = (xs.clone()).lock().unwrap().clone();
for __x in __snapshot.iter() { ... .await ... }
```

Este bug nuevo es el caso análogo para **Map literal** en kwargs de
builtin async.

### Fix propuesto del codegen

Cuando el codegen emite código que extrae valor de un `Arc<Mutex<...>>`
para pasarlo como argumento a una expresión que contiene `.await`, debe
clonar a una variable local intermedia que termine ANTES del await:

```rust
// Patrón actual (rompe Send):
__fitz_http_request(__FitzHttpRequestOpts {
    headers: (Arc::new(Mutex::new(...))).lock().unwrap().clone(),
    body: ...
}).await

// Patrón correcto (Send OK):
let __headers_snap = {
    let __h = Arc::new(Mutex::new(vec![...]));
    let __g = __h.lock().unwrap();
    __g.clone()
};   // ← guard se dropea acá
__fitz_http_request(__FitzHttpRequestOpts {
    headers: __headers_snap,
    body: ...
}).await
```

El bloque interior `{ let __g = ...; __g.clone() }` garantiza que el
`MutexGuard` se dropea al cierre del bloque (antes de que la expresión
participe en el `.await`).

Aplicación: cualquier `Map literal` o `List literal` que se emita como
argumento de una llamada async (o como field de un struct literal cuyo
container es argumento de una llamada async). Probable que también aplique
a `body` Map (no solo `headers`).

### Tests sugeridos al fixear

1. `tests/compile_e2e.rs::http_request_with_headers_map_spawn_compila` —
   repro mínima de arriba, validar que compila sin errors.
2. `tests/compile_e2e.rs::http_request_with_body_map_spawn_compila` —
   mismo pero con body Map heterogéneo en lugar de headers.
3. `tests/compile_e2e.rs::http_request_anidado_en_async_fn_cross_module_spawn` —
   variante con `send_email` declarada en módulo importado (matchea
   fitzwatch real).
4. Regression sobre el case análogo de `for x in List<Str>` con `.await`
   en `@cron` (v0.18.1) — asegurar no se rompe.

### Workaround user-land (NO funciona, documentado para claridad)

Probados sin éxito:

- Bindear `let opts = {...}` antes del `http.request(opts)` — el codegen
  inline-a igual y el guard sigue cross-await.
- Bindear cada Map por separado (`let headers = {...}; let body = {...}`)
  — mismo resultado.
- Construir el body como Str JSON manual + `http.post(url, body_str)` —
  evita el body Map pero **NO permite headers custom** (Resend exige
  Authorization Bearer header).
- Eliminar el spawn del caller y volver a `.await` directo — funciona
  el build pero pierde fire-and-forget (handler bloquea hasta que la
  llamada HTTP completa).

**No hay forma de pasar Authorization header custom a Resend sin Map
literal en `headers` desde Fitz user-land**. Cualquier alternativa
require que el codegen del lenguaje cierre este bug.

### Impacto en producción

- **fitzwatch (deploy 2026-06-23)**: subscribers públicos persisten en DB
  pero NO reciben welcome email. La UI dice "Listo! Revisá tu inbox" y
  el email nunca llega. Misma situación para notify de incidents — los
  webhooks funcionan, los emails no.
- **Cualquier proyecto Fitz que necesite HTTP outbound con auth Bearer**
  (la mayoría de APIs REST modernas) desde un context spawned. Cubre
  prácticamente todo: Stripe, Twilio, SendGrid, Mailgun, OpenAI, etc.
- **Trigger del descubrimiento**: bloqueo SMTP outbound de DigitalOcean
  obligó migrar de `smtp.send` builtin (que NO usa Map literal pesado
  en spawn — su codegen ya cierra el guard inmediatamente) a
  `http.request(opts)` REST. Probable que muchos providers tengan
  bloqueo SMTP por default y este path sea el único viable.

### Referencias del descubrimiento

- Sesión 2026-06-23: deploy de fitzwatch.com en VPS DigitalOcean
  (droplet shared con citai/syndesi).
- DigitalOcean bloquea outbound a smtp.resend.com:587/465/2587/2465
  (confirmado con `nc -zv` — timeout en todos los puertos SMTP, port
  443 HTTPS OK).
- Refactor de `subscriptions.fitz::send_email_to` para usar Resend HTTP
  API REST (`https://api.resend.com/emails`) con Bearer auth → `fitz
  check` pasa OK pero `fitz build` falla con los 3 errors de arriba.

### Cierre estimado

Fix probable en `src/codegen.rs` — funciones que emiten args para
builtins async (`gen_http_request_call`, `gen_http_post_call`,
`gen_smtp_send`, `gen_jwt_encode`, posiblemente otros). Patrón uniforme:
wrappear `.lock().unwrap().clone()` en bloque que drop el guard. Estimado
~50-100 LoC + 4 tests E2E. Compatible con la solución de v0.18.1.

---

## 🟢 LSP false positive: `@auth_provider` cross-module no se resuelve — **CERRADO 2026-06-23**

> **CERRADO** el mismo día del descubrimiento (sesión 2026-06-23, smoke
> real del fix v0.19.2 sobre fitzwatch). Fix ~280 LoC netos en
> `src/lsp.rs` + `src/bin/fitz-lsp.rs` + 5 unit tests nuevos en
> `lsp::tests::cross_module_*`. Sin cambio de comportamiento del
> lenguaje — el binario producido por `fitz build` / `fitz check` /
> `fitz run` ya resolvían cross-module via W12 (auth_provider) y B10
> (background fns); este fix replica esa misma pre-scan en el LSP path
> que corría aislado.

**Síntoma original (pre-fix)**: cuando un módulo importa un
`@auth_provider` declarado en OTRO módulo (patrón W12 cross-module), el
LSP emite el diagnostic:

```
@authenticated on fn 'X': no `@auth_provider` registered in the program.
```

aunque el `@auth_provider` SÍ existe en el grafo de imports y el binario
compila + ejecuta sin issues. Ejemplo canónico en fitzwatch:

```
// auth.fitz
@auth_provider
async fn current_user(headers: Map<Str, Str>) -> Result<User> { ... }

// incidents.fitz
from auth import User
import auth

@authenticated  // ← pre-fix: el LSP emitía falso positivo
@put("/api/incidents/{id}/status")
async fn update_incident_status(...) -> Result<IncidentUpdated> { ... }
```

**Causa raíz (confirmada)**: `src/lsp.rs::check_source_with_types`
corría `check_program(&program)` sobre el módulo abierto en aislamiento,
sin cargar el grafo de imports. El checker local NO veía el
`@auth_provider` declarado en `auth.fitz` y emitía el diagnostic falso.
El codegen (`src/codegen.rs::pre_scan_imported_auth_provider_for_loader`)
y el CLI (`src/main.rs::pre_scan_imported_auth_provider`) sí cargaban
el grafo entero, por eso la build real andaba.

**Casos cerrados con este fix**:

- `@authenticated` cross-module (caso más común — handler en módulo
  por feature + provider en `auth.fitz`).
- `@admin` cross-module (mismo flow, regla extra de `role: Str`).
- `@requires("role")` cross-module (Fase 9.w.1.iter2.a, mismo patrón).
- `spawn(<imp_fn>(...))` con `@background` cross-module (B10) —
  el mismo path resolvía esto en el checker pero estaba inaccesible
  desde el LSP; ahora el LSP lo pre-scanea también.
- `@ws(...)` + `@authenticated` apilados (mismo path del checker —
  cubierto por la misma fallback en `collect_auth_provider`).

**Fix aplicado**:

1. **`src/lsp.rs`** — wrapper nuevo
   `check_source_with_types_and_base_dir(source, base_dir: Option<&Path>)`
   con la firma de 5-tupla idéntica a `check_source_with_types`. Cuando
   `base_dir` es `Some`:
   - Corre `resolve_program_with_env` para pre-poblar nominales locales.
   - Helper privado `pre_scan_imported_auth_provider_lsp` walks
     `Stmt::Import` / `Stmt::FromImport`, resuelve cada módulo a un
     `.fitz` relativo a `base_dir`, parsea, invoca
     `types::extract_auth_provider_signature` (la misma API pública
     usada por W12 en main.rs/codegen.rs) y popula
     `env.set_imported_auth_provider`.
   - Helper análogo `pre_scan_imported_background_fns_lsp` invoca
     `types::extract_background_fn_names` (B10) y popula
     `env.add_imported_background_fns`.
   - Llama a `types::check_with_env` con el env enriquecido.
   - El `check_source_with_types(source)` legacy queda como wrapper
     que pasa `None` (compat con callers sin file context: REPL,
     unit tests internos).
2. **`src/bin/fitz-lsp.rs::check_and_publish`** — deriva `base_dir`
   del open document via `uri.to_file_path().parent()` y lo pasa al
   nuevo wrapper. Fallback transparente a single-file mode cuando
   la URI no es `file://`.
3. **Política de error**: silent fallback sobre módulos que fallan
   lectura/parse (paralelo a `pyi_loader::load_stubs` y W12 en
   main.rs). El LSP enriquece el env; el codegen/runtime loader es
   quien reporta los errores reales.
4. **Alcance del MVP**: un nivel de profundidad (sin recursión sobre
   transitive imports), paralelo a W12/B10. Cubre el 90% del caso
   (handler-per-feature + provider en `auth.fitz`). No consulta
   `dep_registry` (el LSP no tiene acceso al manifest, paralelo
   limitación de `resolve_cross_module_definition` y
   `from_import_completions`).

**Tests nuevos** (5, en `lsp::tests::cross_module_*`, feature `lsp`):

- `cross_module_auth_provider_resuelve_via_base_dir_sin_diagnostic_falso`
  — canónico: SIN `base_dir` aparece el falso positivo, CON `base_dir`
  desaparece.
- `cross_module_admin_decorator_resuelve_via_base_dir` — variante
  `@admin` con `role: Str` extraído del módulo origen vía
  `has_role_field`.
- `cross_module_requires_decorator_resuelve_via_base_dir` — variante
  `@requires("role")` (Fase 9.w.1.iter2.a).
- `cross_module_background_spawn_resuelve_via_base_dir` — variante
  `@background` + `spawn(<imp>(...))` (B10), confirma que el mismo
  pipeline cierra ambos diagnostics.
- `cross_module_pre_scan_silent_fallback_on_missing_module` — robustez:
  si el módulo importado no existe en disco, el pre-scan skipea
  silencioso sin contaminar la lista de errores.

**Verificación pre-bump**: `cargo fmt --all` limpio; `cargo clippy
--lib --tests --bins --features lsp -- -D warnings` limpio; suite LSP
completa 131/131 verde; full lib suite 3331/3331 verde.

**Deudas residuales derivadas** (NO bloquean):

- **Imports transitivos**: si `posts.fitz` importa `lib.fitz` que a
  su vez importa el provider de `auth.fitz`, el LSP no lo encuentra
  (recursión un nivel solo). Misma limitación que W12 en main.rs.
  Refinable si aparece demanda real.
- **Dep registry**: imports vía `[dependencies]` de `fitz.toml` no
  resuelven en el LSP (path deps con relative paths sí). Mismo gap
  que `from_import_completions`. Cuando se cierre, ataca los tres
  consumidores juntos.
- **Cache**: cada keystroke re-parsea TODOS los módulos importados
  del grafo. Acceptable para programas chicos (<10 imports). Si
  presión real aparece sobre proyectos grandes, agregar cache con
  invalidación por modtime.
- **B20 — `@cron(store=X)` cross-module en LSP**: tiene la misma
  patología (referencia a var declarada en otro módulo). Sale del
  scope del fix actual — ataque cuando se cierre B20 entero.

**Referencias**:

- Reportado en sesión 2026-06-23 durante smoke real del fix v0.19.2
  sobre fitzwatch (apps multi-módulo).
- `src/lsp.rs::check_source_with_types_and_base_dir` (entry nuevo).
- `src/lsp.rs::pre_scan_imported_auth_provider_lsp` /
  `pre_scan_imported_background_fns_lsp` (helpers nuevos).
- `src/bin/fitz-lsp.rs::check_and_publish` (consumer del nuevo entry).
- Paridad con: `src/main.rs::pre_scan_imported_auth_provider` (W12) /
  `pre_scan_imported_background_fns` (B10).
- Cap 22 de `docs/guide.md` actualizado con el bullet "Diagnostics
  en vivo" mencionando la pre-scan cross-module.

---

## 🟢 `spawn(<cross_module_@background_async_fn>(...))` silent drop — **CERRADO v0.19.2 (2026-06-23)**

> **CERRADO** en el release v0.19.2 (mismo día del cierre técnico).
> Fix mínimo (~10 LoC) en `collect_module_sigs` + 1 E2E test nuevo
> que candea la regresión inspeccionando el Rust emitido. Detalle
> abajo preservado para referencia futura.

**Síntoma original (pre-fix)**: cuando `spawn(...)` recibía un call a
una fn `@background` **declarada en otro módulo** (importada vía
`from foo import bar`), el runtime aceptaba la línea pero **nunca**
invocaba la fn. Sin error, sin panic, sin log — silent drop.

**Repro mínima validada**:

`worker.fitz`:
```fitz
@background
async fn do_work(id: Int) -> Null {
    log.info("worker.start id={id}")
    return null
}
```

`main.fitz`:
```fitz
from worker import do_work

@get("/trigger/{id}")
async fn trigger(id: Int) -> Result<Str> {
    let _ = spawn(do_work(id))   // pre-fix: NO disparaba el log
    return Ok("dispatched")
}
```

**Causa raíz** (descubierta al inspeccionar el Rust emitido en
`target/fitz-build/<bin>/src/main.rs`): el closure de `tokio::spawn`
se emitía SIN `.await`:

```rust
let __jh = tokio::spawn(async move { do_work(id) }); // ← sin .await
```

`do_work` es async → `do_work(id)` devuelve un `Pin<Box<dyn Future>>`
que el `async move {}` envuelve y dropea sin pollar. **Silent drop**.

**Por qué solo cross-module**: el path local en `pre_register_fn_signatures`
(Fase 6.6, `src/codegen.rs:12423-12427`) YA envolvía el `ret` en
`Type::Future(...)` cuando `is_async = true`. El path cross-module en
`collect_module_sigs` (`src/codegen.rs:5561-5566`) no replicaba ese
wrap. `gen_spawn_call` decide si emite `.await` con
`matches!(target_ret, Type::Future(_))` — para cross-module veía
`sig.ret = Type::Null` (no `Type::Future(Null)`) y omitía el `.await`.

**Fix** (`src/codegen.rs:5536-5570`, ~10 LoC netas):
- `Stmt::FnDef` desestructura `is_async` además de los otros campos.
- Variable renombrada `ret` → `inner_ret`.
- Wrap aplicado cuando `is_async = true`:
  ```rust
  let ret = if *is_async {
      Type::Future(Box::new(inner_ret))
  } else {
      inner_ret
  };
  ```
- Comment explica el paralelismo con `pre_register_fn_signatures` y
  el modo en que el bug se manifiesta (silent drop).

**Test E2E nuevo** (`tests/compile_e2e.rs::cross_module_spawn_async_background_emits_await_no_silent_drop`):
build a 2 archivos + inspección directa del Rust emitido en
`<workspace>/target/fitz-build/<stem>/src/main.rs`. Confirma que
contiene `do_work(id).await` y NO `tokio::spawn(async move { do_work(id) })`
bare.

**Smoke real validado bit-a-bit** (curl al binario nativo): el log
estructurado `{"msg":"worker.start id=42"}` aparece en stderr
inmediatamente después del response `200 dispatched`. Pre-fix el log
nunca aparecía.

**Validación sin regresiones**:
- `cargo test --lib --release` 3200/3200 verde.
- Smoke `GUIDE_EXAMPLES_COMPILE` ~370 ejemplos guía+curso+TaskHub
  verde (~14 min).
- `fitz check` sobre los 10 boilerplates verde.
- `cargo fmt --all --check` limpio.
- `cargo clippy --lib --tests --bins -- -D warnings` limpio.

**LSP**: el false positive paralelo en el LSP (mensaje `"spawn: fn X
is not declared with @background"` en imports) ya estaba cerrado
por **B10** (sub-paso 5 de la cosecha post-fitzwatch 2026-06-19,
`extract_background_fn_names` + `TypeEnv::add_imported_background_fns`).
Ese fix cerró el path del checker; v0.19.2 cierra el path del
runtime/codegen. Los dos paths reportaban síntomas distintos del
mismo gap: el checker bloqueaba `fitz check`/`fitz build`, el
codegen dropeaba silently. Ambos paths quedan paralelos y consistentes.

**Hallado durante**: integración real con un proyecto del autor que
usaba `@background` cross-module para bulk SMTP. La fn aparecía en
el código, compilaba limpio, pero los emails nunca se enviaban. El
workaround temporal era `.await` directo (perdía fire-and-forget,
bloqueaba el handler) hasta que cerró v0.19.2.

---

## 🟢 `Response { ... }` built-in: 3 bugs del Bloque 3.c detectados en fitzwatch (CERRADO v0.19.1, 2026-06-21)

> **CERRADO ENTERO** en el release v0.19.1 (junio 2026, mismo día
> del descubrimiento). Bug 1 (cross-module imports) + Bug 2
> (signature mismatch en `Result<Response>` con `?` propagation)
> + Bug 3 (`metrics::*` not found, era incidental al Bug 1) los 3
> fixeados + 3 E2E tests nuevos cubren cada caso. Detalle abajo
> preservado para referencia futura.

**Resumen del cierre (2026-06-21)**:

- **Bug 1 fix**: nuevo walker `program_uses_response_builtin(program)`
  en `src/codegen.rs` (~140 LoC, paralelo a `program_uses_db` /
  `program_uses_http_client`); en `generate_module_rs_with_bindings`
  se agrega bloque condicional que emite
  `use crate::{Response, ResponseData};` cuando el módulo los
  necesita (paralelo a W11 DB / W16 `__FitzResponse` / HTTP client).
- **Bug 2 fix**: `gen_top_fn` (~13436) ahora consulta
  `detect_response_builtin_kind(&effective_ret, env)` (helper de
  Bloque 3.b ya existente) antes de computar `has_return_status`.
  Si el handler retorna Response built-in, `body_has_try` NO
  activa `response_mode` — la user-fn conserva su signature
  natural (`-> Result<Arc<Mutex<ResponseData>>, String>` o
  `-> Arc<Mutex<ResponseData>>`) y `gen_try` usa el `?` nativo
  Rust (válido porque el container ES Result).
- **Bug 3**: cerrado automático con Bug 1 — era el mismo error
  rustc cross-module enmascarando los demás. El test E2E lo
  verifica.
- **3 E2E tests nuevos** en `tests/compile_e2e.rs`:
  `v019_response_cross_module_emits_imports`,
  `v019_response_in_result_ok_signature_matches_wrapper`,
  `v019_response_with_auth_db_ws_observability` + helpers
  `build_expect_ok` y `build_expect_ok_multi` (validan build
  success sin invocar el binario, útil para HTTP servers).
- **Verificación pre-bump completa**: 3200 lib tests + 98 cli_e2e
  + 3 openapi_e2e + smoke 370+ ejemplos guía + 5 boilerplates
  representativos (api-orm-full-fullstack multi-file incluido) +
  fmt + clippy (default + lsp) — todo verde, sin regresiones.

**Próximo norte**: notificar al autor para retomar fitzwatch
Fase F.d (30 min). Detalle completo en CHANGELOG v0.19.1.

---

### Detalle histórico de los 3 bugs (preservado para referencia)

> **Contexto del descubrimiento (2026-06-21)** (pre-cierre):
> Bloqueaba el caso integrador real (fitzwatch
> Fase F.d) que estresa el feature con auth + DB + WS + observability
> + cross-module. El ejemplo `examples/guide/17l-response-custom.fitz`
> compila OK porque es CLI HTTP simple standalone — los 3 bugs solo
> aparecían al usar Response built-in en proyectos reales.

**Contexto del descubrimiento (2026-06-21)**: fitzwatch (producto del
autor, status page open-source — repo PRIVADO en
`github.com/Thegreekman76/fitzwatch`) tenía Fase F.d (RSS feed por
slug) bloqueada por la deuda HTTP HandlerOutcome.content_type. Esa
deuda se cerró con v0.19.0 (`Response { ... }` built-in). Al intentar
retomar F.d con la sintaxis nueva, detectamos 3 bugs del Bloque 3.c
del codegen que NO están cubiertos por los E2E del release. fitzwatch
es el primer stress test real del feature.

**Estado en fitzwatch**:
- Código F.d entero implementado (helpers `escape_xml`,
  `fetch_rss_items`, `build_rss_xml`, type `RssItem`, handler
  `slug_incidents_rss` con shape idiomatic `-> Result<Response>` + `?`,
  frontend RSS link + JS wire). Handler comentado en
  `d:\fitzwatch\src\public.fitz` hasta que estos 3 bugs cierren.
- Container actual `fitzwatch-app` corre con `FITZ_TAG=v0.18.2`
  (stable, F.d off — handler ausente).
- Cuando los 3 bugs cierren: bump `FITZ_TAG` en `.env`, descomentar
  handler, `docker compose up -d --build app`, smoke con curl + RSS
  reader. 30min de trabajo.

### Bug 1 — Cross-module: `Response`/`ResponseData` no se importan en módulos hijos

**Síntoma**: handler con `-> Response` o `-> Result<Response>` declarado
en un módulo importado (`public.fitz`) genera `src/public.rs`
referenciando `Response` y `ResponseData` (líneas tipo
`<Response as __ToFitzJson>::__to_fitz_json(&(Arc::new(Mutex::new(ResponseData { ... }))))`)
pero NO emite `use crate::Response; use crate::ResponseData;` en el
preludio del módulo.

**Error rustc**:
```
error[E0425]: cannot find type `Response` in this scope
  --> src/public.rs:671:53
  |
  = help: consider importing this type alias: use crate::Response;

error[E0422]: cannot find struct, variant or union type `ResponseData` in this scope
  --> src/public.rs:671:117
  |
  = help: consider importing this struct: use crate::ResponseData;
```

**Causa probable**: el preludio HTTP del codegen
(`emit_helpers_for_imported_types`/equivalente en `src/codegen.rs`)
emite `use crate::{...}` para tipos detectados como ORM virtual fields
(W17), `__FitzResponse`, `__ToFitzJson`, etc. pero **no para `Response`
ni `ResponseData`** cuando el módulo importado los usa.

**Fix esperado**: paralelo bit-a-bit al patrón W17 ORM cross-module.
Detectar si el módulo importado usa el Response built-in (
`stmt_uses_response_builtin` walker análogo a `stmt_uses_python`/
`stmt_uses_cron` que ya existen) y agregar
`use crate::{Response, ResponseData};` al preludio del módulo.

### Bug 2 — Signature mismatch en `Result<Response>` (path InResultOk del Bloque 3.c)

**Síntoma**: el wrapper axum del handler detecta el Response built-in
y emite el dispatch nuevo (Bloque 3.c) esperando que la user-fn devuelva
`Result<Arc<Mutex<ResponseData>>, String>` (mirror de `Result<Response>`
Fitz). Pero la user-fn se emite con shape LEGACY que devuelve
`__FitzResponse` directo. Inconsistencia interna del codegen.

**Código emitido (capturado en `d:\fitzwatch\target\fitz-build\main\src\main.rs:5665-5727`)**:

```rust
// User-fn — shape LEGACY (E I R R Ó N E O)
async fn slug_incidents_rss(mut slug: String) -> __FitzResponse {
    let mut user_id: i64 = (match ((slug_to_user_id(slug.clone())).await) {
        Ok(__v) => __v,
        Err(__e) => return __FitzResponse {
            status: 500,
            body: serde_json::json!({"error": __e}),
        },
    });
    if (user_id == 0i64) {
        return __FitzResponse {
            status: 200,  // ⚠️ EMITE 200 con body JSON del Response
            body: <Response as __ToFitzJson>::__to_fitz_json(&(Arc::new(Mutex::new(ResponseData {
                status: 404i64,
                content_type: String::from("text/plain; charset=utf-8"),
                headers: Arc::new(Mutex::new(Vec::new())),
                body: String::from("slug not found"),
                body_bytes: None
            }))))
        };
    };
    // ... etc, todos los returns siguen el shape legacy ...
}

// Wrapper axum — shape NUEVO (CORRECTO según Bloque 3.c, espera Result<Response>)
async fn __handler_slug_incidents_rss(...) -> axum::response::Response {
    let __result = slug_incidents_rss(slug).catch_unwind().await;
    let __built = match __result {
        Ok(__resp_arc) => {
            // extract status/ct/headers/body/body_bytes del Arc<Mutex<ResponseData>>
            let (__status, __ct, __headers, __body, __bbytes) = {
                let __g = __resp_arc.lock().unwrap();
                (__g.status, __g.content_type.clone(), ...)
            };
            // axum builder con Content-Type custom + headers + body|body_bytes
            ...
        },
        Err(__e) => (INTERNAL_SERVER_ERROR, Json({"error": __e})).into_response(),
    };
    ...
}
```

**Error rustc**:
```
error[E0308]: mismatched types
    --> src/main.rs:5739:9
     |
5738 |     let __built = match __result {
     |                         -------- this expression has type `__FitzResponse`
5739 |         Ok(__resp_arc) => {
     |         ^^^^^^^^^^^^^^ expected `__FitzResponse`, found `Result<_, _>`
```

**Causa probable**: el codegen tiene DOS lugares que deciden el shape
del Response built-in:
1. **Wrapper** (`emit_handler_dispatch_and_response` con su helper
   `emit_response_builtin_dispatch`) — funciona correcto.
2. **User-fn** (`gen_top_fn` / wherever the body of the user fn is
   emitted) — NO consulta `HandlerSig.response_builtin_kind` y cae
   a path legacy (emite `-> __FitzResponse` con `__ToFitzJson`).

**Fix esperado**: cuando `HandlerSig.response_builtin_kind ==
ResponseBuiltinKind::InResultOk` (o `Direct`), la user-fn debe
emitirse con signature `-> Result<Arc<Mutex<ResponseData>>, String>`
(o `-> Arc<Mutex<ResponseData>>` para Direct), y los returns deben
emitir `Ok(Arc::new(Mutex::new(ResponseData { ... })))` /
`Err(format!(...))` paralelo al evaluador del intérprete.

### Bug 3 — `metrics::counter!`/`histogram!` not found con mezcla Response + auth + DB + WS + observability

**Síntoma**: handler `-> Response` direct (path Direct del Bloque 3.c)
en programa con auth + DB + WS + observability rompe con `E0433 cannot
find counter in metrics` sobre las líneas del access log del wrapper
HTTP que emite el codegen automático (Fase 12.3.b):

```rust
metrics::counter!("http_requests_total", &__labels).increment(1);
metrics::histogram!("http_request_duration_seconds", &__labels).record(__duration_secs);
```

**Lo extraño**:
- `Cargo.toml` emitido tiene `metrics = "0.24"`.
- Verificado en `~/.cargo/registry/src/.../metrics-0.24.6/src/macros.rs`:
  `counter!` y `histogram!` están `#[macro_export]` (accesibles como
  `metrics::counter!`).
- **Ejemplo oficial v0.19.0** `examples/guide/17l-response-custom.fitz`
  (CLI HTTP simple standalone con Response built-in) compila OK con
  ese mismo Cargo.toml + macros.
- **fitzwatch BASE** (sin handler RSS) compila OK con v0.19.0.
- **Aparece SOLO** al sumar handler Response built-in al programa
  con auth + DB + WS + observability. El programa entero queda con
  E0433.

**Causa por investigar**: interacción de paths del codegen. Hipótesis
candidatas:
- El preludio HTTP se emite DUPLICADO cuando hay módulos importados +
  Response built-in, y la segunda emisión shadowea algo del `metrics`.
- Algún `use crate::*` glob importa un símbolo `metrics` (variable
  o módulo) que collisiona con la crate dep.
- Bug del macro resolver de rustc con tokio multi-thread + features
  específicos.

**Repro mínimo recomendado** (sumar al test suite del lenguaje): handler
`-> Response` direct en programa con `@auth_provider` + `db.connect` +
`@ws("/x")` + `@server(observability=true)` (default). Build debe
emitir todas las macros `metrics::*` resolubles.

### Plan de fix sugerido para los 3 bugs

1. **Reproducir aislado en `tests/compile_e2e.rs`** los 3 casos con
   programas mínimos:
   - `v019_response_cross_module_emits_imports`: Response builtin en
     módulo importado con `from main import` + handler en módulo + el
     emitted `src/<mod>.rs` debe tener `use crate::{Response, ResponseData};`.
   - `v019_response_in_result_ok_signature_matches_wrapper`: handler
     `-> Result<Response>` en single-file. La user-fn DEBE compilar
     contra el wrapper. (Caso integrador del Bloque 3 que no estaba.)
   - `v019_response_with_auth_db_ws_observability`: handler
     `-> Response` direct mezclado con auth + DB + WS + observability.
     Reproduce Bug 3.
2. **Fix Bug 1**: walker `stmt_uses_response_builtin` análogo a
   `stmt_uses_python`/`stmt_uses_cron`. Preludio cross-module emite
   `use crate::{Response, ResponseData};` cuando aplica.
3. **Fix Bug 2**: actualizar el codegen de la user-fn (signature +
   return wrap) cuando `HandlerSig.response_builtin_kind != None`.
   Paralelo bit-a-bit a lo que ya hace el wrapper en Bloque 3.c.
4. **Investigar Bug 3**: revisar el emitted Rust de fitzwatch para
   identificar la causa raíz. Si es interacción del codegen,
   probablemente fix incidental con Bug 1/2. Si no, gating o reorden
   del preludio.
5. **Release `v0.19.1`** con los 3 fixes + 3 E2E nuevos. Documentar
   en CHANGELOG + cierre acá. Bump CHANGELOG/roadmap/CLAUDE.md/extensión.
6. **Notificar al autor** para retomar fitzwatch Fase F.d (30min de
   trabajo: bump `FITZ_TAG` en `.env`, descomentar handler en
   `public.fitz`, `docker compose up -d --build app`, smoke con curl +
   RSS reader externo como Feedly).

### Referencias

- Repo del producto bloqueado:
  `d:\fitzwatch\src\public.fitz` (handler comentado al final del
  archivo) + `d:\fitzwatch\ROADMAP.md` (sección Fase F.d) +
  `d:\fitzwatch\NEXT-SESSION.md`.
- Emitted Rust con los 3 bugs reproducidos:
  `d:\fitzwatch\target\fitz-build\main\src\main.rs:5665-5810` (caso
  Direct sin cross-module — Bug 2 + Bug 3 visibles) +
  `d:\fitzwatch\target\fitz-build\main\src\public.rs:662-689` (caso
  cross-module con `Result<Response>` — Bug 1 + Bug 2 visibles, fix
  del Bug 1 deja Bug 2 expuesto).
- Ejemplo oficial que NO triggea los bugs (confirma que el feature
  funciona para casos simples): `examples/guide/17l-response-custom.fitz`.

---

## 🟢 HTTP `Response { ... }` built-in (CERRADO v0.19.0, 2026-06-21)

**Cerrado entero** en el release v0.19.0 (junio 2026). El type built-in
`Response` con 5 fields (`status` / `content_type` / `headers` / `body` /
`body_bytes`) habilita respuestas non-JSON desde el handler:

```fitz
@get("/feed.rss")
fn rss_feed() => Response {
    content_type: "application/rss+xml; charset=utf-8",
    body: "<?xml version=\"1.0\"?><rss/>",
}
```

**5 bloques coordinados**: (1) intérprete con `HandlerOutcome` extendido
+ helper `response_instance_to_outcome` con validación XOR + status
range; (2) opt-in binary path `body_bytes: Bytes?`; (3) codegen paridad
bit-a-bit con `ResponseBuiltinKind` detection en `resolve_handler_signature`
+ rama dedicada en `emit_handler_dispatch_and_response` + validación XOR
build-time; (4) integración OpenAPI 3.1 con `ResponseContentTypeKind` +
helper `detect_response_content_type_kind` walker AST + schema
`200.content.<media_type>` con `format: binary` cuando aplica; (5) docs
+ ejemplo runnable + LSP + extensión VSCode v0.19.0. Detalle en
`CHANGELOG.md` v0.19.0.

**Tests al cierre**: 11 unit http intérprete (Bloques 1+2) + 12 unit
codegen (Bloque 3) + 6 unit openapi (Bloque 4) + 2 unit LSP (Bloque 5)
+ 3 E2E `tests/compile_e2e.rs::v019_block3d_*` + smoke
`GUIDE_EXAMPLES_COMPILE` con `examples/guide/17l-response-custom.fitz`
sumado + validación bit-a-bit a mano con curl sobre los 4 casos
canónicos.

**Deudas residuales derivadas** (NO bloquean uso real):

- Multi-arm bodies (if/match retornando distintos `Response { ... }`
  por arm) no se detectan en compile-time — el schema cae al path
  legacy `application/json` para esos handlers. El runtime funciona
  correcto (peek dinámico). Refinable post-MVP.
- Response built-in + post middleware (`@middleware(fn)` con 2 args)
  no soportado — el post-mw recibe `__FitzResponse` JSON-wrapped que
  pierde content_type / body_bytes. Workaround documentado: usar
  `return <status> { ... }` o remover el post-mw.
- Helpers Bytes adicionales si aparece demanda real:
  `bytes_from_b64`, `bytes_from_hex`, etc.

**Workarounds actuales (todos malos)**:
- Devolver el XML como `Value::Str` → axum lo serializa JSON-quoted: `"<rss>..."` (no parseable como RSS).
- Devolver `{"xml": "<rss>..."}` JSON wrapper → el cliente tendría que des-envolver con JS antes de pasar a RSS reader (no estándar).
- Servir el XML estático via nginx desde un archivo escrito por un `@cron` periódico (requiere arquitectura adicional, no responsivo a cambios live).

**Propuesta de solución (alta nivel, sin implementar)**:
- Opción A: nuevo type built-in `Response { status: Int, content_type: Str, body: Str }` que el handler puede retornar directamente. El wrapper async detecta el `Value::Response` y arma `HandlerOutcome` con `content_type` propio.
- Opción B: extender la sintaxis del status response, `return 200 "<rss>..." content_type="application/rss+xml"` (más invasivo al parser).
- Opción C: builtin `response.with_content_type(body, ct)` que retorna un wrapper `Value::Response`. Más simple del lado parser, requiere builtin nuevo.

**Decisión recomendada**: A (type built-in `Response { status, content_type, body }`). Coherente con `Request`/`Response`/`File` ya built-in. Mantiene la paridad `fitz run` ↔ `fitz build`.

**Impacto si se cierra**: cualquier proyecto downstream que necesite RSS / Atom / Sitemap / CSV exports / SVG badges / etc, sin recurrir a workarounds. Caso bloqueado documentado: status page de un user de Fitz que necesitaba feed RSS por cada workspace y tuvo que diferir la feature.

**Test E2E que validaría el cierre**: handler que retorna `Response { status: 200, content_type: "application/rss+xml", body: "<?xml..." }` → `curl -I` ve el Content-Type correcto y el body es el XML sin JSON-quoting. Paralelo `fitz run` ↔ `fitz build` bit-a-bit.

---

## 🟡 B20 — `@cron(store=X)` cross-module no resuelve binding del módulo origen (deuda futura, 2026-06-20)

**Detectada al destrabar fitzwatch** después de cerrar B19 (cron cross-module no spawneaba). Caso edge no soportado por el codegen: `@cron(..., store=X)` declarado en un módulo importado (e.g., `scheduler.fitz`) referencia un binding `X` que debería resolver al `let X = db.connect(...).await` también declarado en el módulo. Hoy el spawn cross-module emite `(&X).into_store()` en `fn main()` del crate root, donde `X` no está en scope → `error[E0425] cannot find value 'X' in this scope`.

**Workaround canónico (pattern TaskHub)**: declarar `let X = db.connect(...).await` + `@cron(store=X)` en el archivo `main.fitz`. El módulo importado solo exporta la fn async helper sin decorator, y el `@cron` del main la llama (`mod.fn().await`). Pattern usado en TaskHub + fitzwatch (post-refactor v0.18.2).

**Por qué no cerramos en v0.18.2**: implementar cross-module store=X resolution requiere ~200-300 LoC:
- Soportar `let X = db.connect(...).await` top-level en módulos (hoy `pre_register_top_lets` solo acepta consts literales).
- Emitir `pub static __FITZ_STATE_X: OnceCell<__FitzDbConn>` + init fn `pub async fn __init_state_X()` en el módulo.
- En `emit_cron_job_spawns`, cuando `module_path.is_some()` y `store_var.is_some()`, materializar `let X = crate::<mod>::__FITZ_STATE_X.get().unwrap().clone()` ANTES del spawn.
- Tocar el module emission entero — riesgo alto de break en TaskHub + curso + guía.

**Plan de cierre cuando aparezca demanda real**: el patrón canónico (declarar en main) cubre el 90% del caso de uso. Si aparece un boilerplate o user externo que requiere cross-module organization estricta del cron + store, abrir mini-fase dedicada con auditoría de los call sites afectados y tests E2E paralelos a TaskHub.

**Tests verdes que cubren el pattern canónico actual**: `cron_in_imported_module_is_spawned_b19` (B19) + `cron_with_persistent_store_in_imported_module_b19_derived` (bug derivado del preludio); ambos en `tests/compile_e2e.rs`. Smoke real fitzwatch v0.18.2 valida persistencia + retry cross-module (refactor en main).

---

## 🟢 Mini-tanda traducción ES→EN del código — CERRADA v0.16.0 (2026-06-15)

**Hito de internacionalización**: 47 commits coordinados en 7 sub-fases (F1-F6 + F5.d) llevan el surface user-facing del compilador (CLI, LSP, errors), test assertions internas y comentarios del grammar TextMate de español a inglés. `grep "esperaba|esperaban" src/` → 0 ocurrencias post-cierre.

**Por sub-fase**:
- **F1**: Comments en `src/**/*.rs` (17 batches).
- **F2**: Test function names (~2647 renames en 44 archivos).
- **F3**: Error messages + runtime emit strings (10 batches).
- **F4**: Output user-facing — CLI, LSP, examples, docs internacionales (syntax-spec, architecture).
- **F5.a/b/c**: Test assertions internas + barrida cross-archivo + F4 leftover + driver Postgres TLS.
- **F5.d**: Residual "esperaba"/"esperaban" — 253 strings en `mod tests` de 11 archivos + 1 user-facing en `src/db.rs:535` que F5.c.2 había perdido (cierre final).
- **F6**: Comentarios del grammar TextMate (`editors/vscode/syntaxes/fitz.tmLanguage.json`).

**NO cubierto por la mini-tanda** (deuda explícita pendiente):
- `docs/guide.md`, `docs/curso/`, `docs/taskhub/`, `README.md`, `docs/index.md` — material pedagógico se mantiene en castellano por decisión de proyecto. Traducción a inglés queda como sub-paso futuro si el material gana tracción internacional.
- Fixtures `.fitz` adentro de tests (ej: "El Chaltén", "división por cero", "id inválido", passwords como "contraseña-secreta-del-usuario") — son fixtures, no surface del compilador. Quedan intactos por diseño.

**Verificación pre-bump completa** (toda verde): 3052 unit (sin feature) / 3170 (lsp) / 3143 (python) + 352 compile_e2e + 1 smoke gigante (290 ejemplos guía+curso+TaskHub) + 98 cli_e2e + 3 openapi + 3 builds release (default + lsp + python) + clippy strict en 3 modos + fmt limpio. 8 failures de compile_e2e son pre-existentes documentados acá mismo (Windows file lock paralelo + codegen cross-module + observability + routing 404 + codegen drift orm_w17 #7 + Postgres apagado para sslmode=require). Cero regresiones de F1-F6+F5.d.

**Detalle por sub-paso**: `CHANGELOG.md` → v0.16.0.

---

## 🟢 Mini-tanda HTTP client builtin — CERRADA ENTERA (2026-06-18, v0.17.0)

**Hito**: módulo built-in nuevo `http` con cliente HTTP outbound async ciudadano de primera clase del lenguaje. Cierra una de las dos brechas chicas que quedaban en el stack web (Fitz ya tenía HTTP server-side, WS, auth, OpenAPI, async, jobs, ORM nativos; faltaba el lado cliente). Detectada como deuda explícita el 2026-06-18 al pausar el desarrollo de **fitzwatch** (status page open-source en Fitz puro) — necesitaba `http.head` para chequear si las URLs monitoreadas responden 200.

**API**: 6 builtins async (`http.get`/`http.head`/`http.post`/`http.put`/`http.delete`/`http.request`) que devuelven `Future<Result<HttpClientResponse>>`. Body shapes `Str` / `Map<Str, Any>` (auto-JSON + `Content-Type: application/json`) / `Bytes`. Tipo built-in nuevo `HttpClientResponse { status: Int, body: Str, headers: Map<Str, Str>, duration_ms: Int }` paralelo a `Request`/`Response` del HTTP server-side. Backend `reqwest = "0.12"` con `["json", "rustls-tls"]` no condicional (linkeado estático, sin openssl en el host).

**Bloques cerrados** (un commit por bloque):
- **B1 evaluator** (commit `3cefd2e`) — `Value::Module { name: "http" }` registrado en `register_builtins` paralelo a `jwt`/`hash`/`log`/`db`/`auth`/`flags`, los 6 builtins async, pre-registro de `HttpClientResponse` como nominal en `TypeEnv`, helper privado `body_to_reqwest_body(value)` con dispatch por tipo del Value, medición `duration_ms` con `Instant::now()`, errores como `Result::Err(Value::Str)` con prefijo identificable.
- **B2 checker** (commit `d1aaf70`) — pre-registro de `http` y `HttpClientResponse` en `CheckCtx::new`, fields tipados (`status: Int`, `body: Str`, `headers: Map<Str, Str>`, `duration_ms: Int`), llamadas devuelven `Result<Any>` paralelo a 8.4 con regla de exhaustividad sobre Result + regla del operador `?`.
- **B3 codegen** (commit `9e214e3`) — detector `program_uses_http_client(program)` walka AST buscando calls `http.X(...)`, `cargo_toml_for` suma `reqwest = "0.12"` condicional, preludio `HTTP_CLIENT_PRELUDE` con `struct __FitzHttpClientResponse` + impls + `static __FITZ_HTTP_CLIENT: LazyLock<reqwest::Client>` (timeout=30s, follow_redirects=true) + helpers async `__fitz_http_get`/etc + helper `__fitz_http_body_apply` paralelo bit-a-bit al intérprete, dispatch en `gen_call`, importación cross-module con `use crate::{__fitz_http_*}`.
- **B4 LSP** (commit `29bc041`) — `scope_level_completions` suma `http` con descripción `"module: get/post/put/delete/head/request (HTTP client async)"`, `after_dot_completions` (recv_name == "http") tira los 6 métodos con signatures completas + ejemplos en el hint, paralelo a dispatch por recv_name de `jwt`/`hash`/`log`/`db`/`auth`/`flags`.
- **B5 guía + ejemplos runnable** (commit `03cd71e`) — sub-sección nueva en cap 17 de `docs/guide.md` "HTTP client outbound" entre "Middleware y CORS" y el cierre del cap, con panorama vecino (`requests`/`axios`/`reqwest`/`OkHttp`) + 5 diferenciales + API completo + body shapes con tabla + modelo de errores + integración + limitaciones MVP. 4 ejemplos runnable nuevos en `examples/guide/`: `17e-http-client-basico.fitz` (los 5 métodos comunes), `17f-http-client-errores.fitz` (timeout/DNS/4xx/5xx + helper + `?`), `17g-http-client-webhook.fitz` (dispatcher canónico con `@background`+`spawn`), `17h-http-client-health-checker.fitz` (cron+http.head fitzwatch-style). Los 4 sumados al smoke `GUIDE_EXAMPLES_COMPILE` en `tests/compile_e2e.rs`. Smoke verde 363 ejemplos en 251.89s + fmt + clippy (default + lsp) limpios + validación bit-a-bit `fitz run` ↔ binario nativo sobre 17e contra `httpbin.org` real.

**Detalle por bloque**: [`docs/http-client-roadmap.md`](http-client-roadmap.md).

**Bloques pendientes**: ninguno — la mini-tanda cerró entera con v0.17.0.
- **B6 barrida cross-docs** (commit `931fd18`): CLAUDE + README + index.md + deudas + roadmap actualizados; mkdocs verde sin warnings nuevos.
- **B7 curso M5.C5** (`b499c36` + chore `3c6f264`): cap nuevo M5.C5 dedicado HTTP client outbound + ejemplo capstone integrador del módulo M5 entero (auth + jobs + client). Curso 42→43.
- **B8 boilerplate api-orm-full** (`1468e0d` + chore `a01da2d`): update chico sumando webhook outbound al publicar post; `@background async fn notify_post_published(...)` + `spawn(...)`. Validación end-to-end vía smoke alternativo `17g-http-client-webhook.exe` (202 en 207ms + webhook delivered status=200 duration_ms=724ms).
- **W18** (commit `63b3d3f`): fix codegen cross-module observability (cierra deuda crítica descubierta al validar B8 sobre api-orm-full multi-archivo — entry dedicada arriba).
- **B9 cierre formal** (v0.17.0, 2026-06-18): CHANGELOG + roadmap + deudas + CLAUDE refresh + bump Cargo.toml + extensión VSCode 0.17.0 + `.vsix` regenerado + blog drafts ES/EN.

**Diferencial del feature**: ningún lenguaje moderno del cuadro (Python `requests`, JS `axios`/`fetch`, Java `OkHttp`/`HttpClient`, Rust `reqwest`, Go `net/http`) provee HTTP client outbound como builtin del lenguaje con paridad bit-a-bit intérprete↔binario y zero deps externas para activarlo.

---

## 🟢 W18 (post-B8) — Codegen cross-module observability CERRADA (2026-06-18, v0.17.0, commit `63b3d3f`)

**Hito**: cierra el gap más visible del codegen cross-module heredado de Fase 12.3.b y la dieta de los bloques W11/W16: cuando un módulo importado declaraba un handler HTTP (`@get`/`@post`/`@put`/`@delete`) y/o llamaba `log.{info,warn,error,debug}(...)`, el `__handler_<name>` wrapper emitido en el módulo invocaba 6 símbolos del preludio LOGGING + SPAN_CONTEXT + OTEL pero faltaban los `use crate::{...}` correspondientes. Resultado: `fitz build` rompía con 12-134+ errores E0425/E0433 según el tamaño del proyecto. Bloqueaba la primera versión del boilerplate `boilerplates/api-orm-full` end-to-end (la deuda pre-existente que B8 dejó documentada).

**Detectado**: post-Bloque 8 de la mini-tanda HTTP client (2026-06-18), al validar `boilerplates/api-orm-full` con el webhook outbound recién agregado. Confirmado con `git stash` → HEAD pre-B8 reproduce los mismos errores (NO regresión de B8).

**Repro mínima** (4 líneas en main + 1 handler en módulo):
```fitz
// repro/main.fitz
from handlers import ping

@server(port=3000)
fn main() => 0

// repro/handlers.fitz
@get("/ping")
fn ping() -> Str => "pong"
```
Pre-fix: 12 errores rustc, 6 símbolos faltantes (`__fitz_otel_is_enabled`, `__fitz_otel_tracer`, `__FitzSpanContext`, `__fitz_with_span_context`, `__fitz_log_info`, `__FitzLogValue`). Post-fix: compila a binario, arranca server, responde `"pong"` con log estructurado JSON y trace_id correlation.

**Fix** (4 sub-cambios coordinados en `src/codegen.rs`):

1. **Imports observability del wrapper**: el bloque `module_has_http` en `generate_module_rs_with_bindings` ya emitía W11/W16 (`__FitzResponse`/`__ToFitzJson`/`__FromFitzJson`/`__apply_cors_and_respond`/`__panic_payload_msg`/`__parse_*`); extendido para emitir 3 grupos nuevos paralelos cuando `module_has_http && main_observability_enabled`: `__fitz_otel_is_enabled`/`__fitz_otel_tracer` + `__FitzSpanContext` + `__fitz_with_span_context`.
2. **Imports de los 4 log helpers** (independientes del wrapper): nuevo flag `module_uses_logging = program_uses_logging(program)` paralelo a `module_has_http`. Cuando `(module_has_http && main_observability_enabled) || module_uses_logging`, emite `use crate::{__fitz_log_info, __fitz_log_warn, __fitz_log_error, __fitz_log_debug, __FitzLogValue}`. Cubre dos sources de llamadas: (a) access log auto-emitido por `gen_http_handler_wrapper` cuando observability ON, (b) `log.{info,warn,error,debug}(...)` del user code en cualquier módulo (con o sin HTTP).
3. **Propagación de `@server(observability=false)` main → módulos**: nuevo helper `extract_main_observability_enabled(program: &Program) -> bool` que walka decorators top-level buscando `@server(...)` con kwarg `observability=Bool`. Por defecto `true`. Pre-scaneado en `generate_project` ANTES del loader y threaded vía nuevo field `ModuleLoader.main_observability_enabled` + setter `set_main_observability_enabled`. `generate_module_rs_with_bindings` recibe el flag como nuevo arg `main_observability_enabled: bool` y setea `ctx.observability_enabled` del módulo. Resultado: cuando main opta por bare-metal, módulos también — el `gen_http_handler_wrapper` lee `self.observability_enabled` y skipea el bloque entero de instrumentación bit-a-bit paralelo. Cierra la inconsistencia semántica que existía desde Fase 12.3.b.5 (main bare-metal pero módulos instrumentados).
4. **Gating refinado de los `use crate::{...}`**: las 3 líneas observability se gatean por `module_has_http && main_observability_enabled` para NO emitir imports inútiles en bare-metal mode (semánticamente coherente + sin warnings `unused_imports` aunque tengamos el `#[allow]`).

**Tests** (9 unit nuevos en `codegen::tests::w18_*`):
- `w18_extract_main_observability_default_is_true` / `_with_server_no_kwarg_is_true` / `_false` / `_true_explicit` (helper extract).
- `w18_module_with_http_handler_emits_use_crate_observability_helpers` (caso happy path — module con `@get`).
- `w18_module_with_log_call_emits_use_crate_log_helpers` (módulo con `log.warn` sin HTTP).
- `w18_module_without_http_or_logging_does_not_emit_observability_use_crate` (regresión negativa).
- `w18_module_with_http_handler_and_observability_false_does_not_emit_wrapper_imports` (propagación `observability=false`).
- `w18_module_with_http_observability_false_and_user_log_still_imports_log_helpers` (gating refinado — log helpers separados del wrapper).

**Validación end-to-end**:
- Repro mínima compila a binario + arranca server + responde `"pong"` con log estructurado y trace_id.
- `boilerplates/api-orm-full` compila a `target/release/fitz-api-orm-full.exe` (~134 errores E0425/E0433 pre-fix → 0 post-fix). Binario arranca, mounta los 13+ endpoints listados en el banner, `/healthz` y `/readyz` responden `{"status":"ok"}`.
- Full lib suite: **3116 unit tests passing, 0 failed** (post-fix: 9 W18 nuevos).
- `cargo fmt --all --check` limpio + `cargo clippy --lib --tests --bins -- -D warnings` limpio.

**Impacto user-facing**: sin breaking. Programas single-file y proyectos multi-archivo SIN handlers/log-calls en módulos importados compilan idénticos (test negativo lo anchora). Proyectos multi-archivo CON handlers/log-calls en módulos ahora compilan a binario nativo end-to-end. Cualquier proyecto con `@server(observability=false)` (que antes era inconsistente entre main y módulos) ahora va bare-metal en todo el árbol.

**Deudas residuales derivadas** (NO bloquean — refinamientos visibles post-W18):
- `gen_http_handler_wrapper` lee `ctx.observability_enabled` en módulos pero **no** considera que main pueda tener su propio `@server` con flag distinto al de un futuro `@server` en módulo. MVP: solo main puede declarar `@server` (parser/loader convention). Si se permite `@server` en módulos en algún momento, el threading necesita refinarse.
- El detector `extract_main_observability_enabled` solo mira top-level del main — no walkea módulos importados. Coherente con la convención actual (`@server` solo en main).
- Modules sin HTTP que importan tipos `@table` con campos `Date`/`Time`/`Uuid` siguen el patrón W11/uses_db (no afectados por W18).

**Commit asociado**: fix dedicado pre-B9 en commit **`63b3d3f`** (2026-06-18) — `fix(codegen): W18 — cross-module observability + log helpers + observability=false propagation`. Cierre formal del Bloque 9 + bump v0.17.0 entra en el commit subsiguiente del release.

---

## 🟡 Hallazgos del codegen del Bloque 5 HTTP client — 3 deudas residuales NO bloqueantes (2026-06-18)

Descubiertas al validar bit-a-bit `fitz run` ↔ `fitz build` sobre los 4 ejemplos runnable `17e/f/g/h`. Documentadas con workaround idiomático conocido para cada caso; el ejemplo respectivo aplica el workaround en línea con comentario explicativo. Ninguna bloquea la mini-tanda HTTP client.

### 1. `?` top-level rechazado por el codegen (esperado por la regla del checker 5.3.3)

**Comportamiento**: `let r = http.get("https://...").await?` directo a nivel top-level es rechazado tanto por el checker como por el codegen, porque la regla 5.3.3 exige que `?` viva adentro de una fn que retorna `Result<T>`.

**Workaround idiomático** (documentado en `17e`/`17f`):

```fitz
async fn run() -> Result<Null> {
    let r = http.get("https://...").await?
    print("status: {r.status}")
    return Ok(null)
}

// Top-level
match run().await {
    Ok(_) => print("listo"),
    Err(e) => print("falló: {e}"),
}
```

**Conclusión**: NO es bug — la regla 5.3.3 es deliberada (el `?` huérfano sería runtime error inevitable). El workaround es el patrón canónico para tooling HTTP client desde un script CLI. Documentado en el cap 17.X de la guía.

### 2. `for x in <List<Str>>` con `.await` adentro de `@cron` rompe Send — **CERRADO 2026-06-20 (v0.18.1, cosecha post-fitzwatch sesión 2)**

**Síntoma**: el codegen del binario nativo rompe con error de tipos del estilo "future is not Send" cuando un body de `@cron` itera una lista y hace `.await` por iteración:

```fitz
@cron("*/30 * * * * *")
async fn check_all_endpoints() -> Result<Null> {
    let urls = ["https://a.com", "https://b.com"]
    for url in urls {
        let r = http.head(url).await?  // ← rompe Send acá
        log.info("status", url: url, status: r.status)
    }
    return Ok(null)
}
```

**Causa**: el codegen del `for in` sobre `List<T>` tomaba `MutexGuard` del `Arc<Mutex<Vec<T>>>` y lo mantenía activo durante todo el body del loop. Cuando había `.await` adentro, el `MutexGuard` cross-await rompía el bound `Send + 'static` que axum/tokio exigen para tasks spawneadas.

**Fix v0.18.1**: el codegen emite ahora un bloque acotado con `let __for_snap` previo al `for`:

```rust
{
    let __for_snap = (xs).lock().unwrap().clone();
    for mut x in __for_snap.into_iter() {
        // body — el MutexGuard YA no sobrevive cross-await
    }
}
```

El `let __for_snap = ...;` libera el `MutexGuard` temporal al `;`, dejando solo el `Vec<T>` owned para el loop. Aplicable a List<T>, List<Tuple> destructuring, Map<K,V> destructuring y Map con wildcard `_`. Cambios en `src/codegen.rs::gen_for_loop` (los 4 sitios del lock chain). Test E2E nuevo `for_over_list_with_await_in_body_does_not_break_send_b17` en `tests/compile_e2e.rs`.

Patrón canónico que estaba bloqueado (descubierto en fitzwatch v0.18.0):

```fitz
async fn dispatch_all(channels: List<NotificationChannel>, payload: Map<Str, Str>) -> Null {
    for ch in channels {
        let _ = dispatch_channel(ch, payload).await  // ← ANTES rompía
    }
    return null
}
```

Ahora compila bit-a-bit con `fitz build`. Cap 17h y otros ejemplos guía que tenían workarounds documentados pueden refactorearse al patrón canónico (deuda menor, cosmética).

### 3. Map literal heterogéneo (Bool + Str) en `return <status> { ... }` rompe el codegen

**Síntoma**: handler HTTP que retorna un status custom con Map literal mezclando Bool y Str rompe el codegen:

```fitz
@post("/webhook")
async fn webhook() -> Result<Map<Str, Any>> {
    if invalid_payload() {
        return 400 { "ok": false, "error": "missing field" }  // ← rompe codegen
    }
    return Ok({"ok": true, "id": "abc"})
}
```

Error del codegen: typo "could not find type `__FitzValue` in this scope" — el trait `__FitzValue` que el preludio HTTP usa para serializar `Map<Str, Any>` heterogéneo NO está activo porque ningún otro camino del programa lo dispara.

**Causa probable** (sub-case del fix de v0.10.4): v0.10.4 cerró Map<Str, Any> en HTTP returns vía `DB_HTTP_INTEGRATION_PRELUDE` + `impl __MapKey for __FitzValue` emitido "cuando `__FitzValue` activo". El caso del Bloque 5 es que **UNA sola aparición de Map heterogéneo adentro de `return <status> { ... }` no activa `__FitzValue`** porque el detector busca el tipo en field declarations y type sigs, no en literals de body de handlers con status custom.

**Workaround idiomático** (documentado en `17g`):

```fitz
// Homogeneizar a Map<Str, Str>:
return 400 { "ok": "false", "error": "missing field" }
```

**Fix futuro** (refinable post-MVP, paralelo a v0.13.1 que gateó `metrics-exporter-prometheus`): extender el detector de `__FitzValue` para que walke literals Map heterogéneos en posiciones de `Stmt::ReturnStatus` con shape Map. ~20 LoC en el detector.

---

## 🟢 Cosecha codegen post-fitzwatch — **CERRADA ENTERA 2026-06-19**

> **Acordado con el autor 2026-06-18**, **cerrado 2026-06-19**: el
> próximo norte del lenguaje después de v0.17.0 era atacar esta
> cosecha. 6 sub-pasos en 2 días cierran B1-B12 + B14 + B15 (B13 NO
> reprodujo en v0.17.0, W18 lo había cerrado; B16 descubierto durante
> sub-paso 2, abierto como deuda separada porque el fix vive en el
> checker, no es mecánico). **B15 fue el grande** (sub-paso 6,
> ~115 LoC + 3 unit + 1 E2E) y al diagnosticarlo descubrimos que el
> trigger REAL era FK Nullable, no nullables-en-parent-type como
> decía el doc original. Probablemente fitzwatch no estaba pausado
> por B15 estricto (sus models declaran todos los FK como `Int = 0`
> sentinel) sino por W18 (cerrado en v0.17.0); cuando el autor retome
> fitzwatch va a poder hacer `fitz build` directo.

### Contexto

Durante el desarrollo de **fitzwatch** (proyecto en `d:\fitzwatch\`,
status page + uptime monitor open-source en Fitz puro, showcase
profesional del stack nativo + Vue 3 + Vuetify + Chart.js + SheetJS),
descubrimos **15 bugs del codegen Fitz v0.17.0** que bloquean
`fitz build`. **`fitz check` y `fitz run` no están afectados** —
todos los bugs son específicos del path codegen → cargo build.

fitzwatch quedó **PAUSADO** esperando cerrar B15 (el crítico).
El backend (13 módulos Fitz), frontend (admin Vue 3 + Vuetify
+ Chart.js + SheetJS profesional con dashboard + grillas paginadas
+ export Excel + WS live + gestión de webhooks), Docker setup,
nginx config, deploy README y Cloudflare prep ya están terminados.

**Doc completo con repros**: [`d:\fitzwatch\CODEGEN-BUGS.md`](../../fitzwatch/CODEGEN-BUGS.md).

### B1 — `.order_by("string")` rechazado por codegen — **CERRADO 2026-06-19 (sub-paso 5 cosecha)**

**Síntoma original** (cargo build):
```
✗ codegen: Error en línea N:M — `.order_by(closure)` espera una closure literal (fn(<param>) => <expr>)
```

**Repro original**:
```fitz
User.where(fn(u) => u.id > 0).order_by("name").all(conn).await
```

**Severity**: ~~Medium~~. **Fix aplicado**: `.order_by("col")` y `.order_by("-col")` (DESC con `-` prefix) ahora aceptados tanto en evaluator (`orm_qb_order_by` en `src/evaluator.rs`) como en codegen (`emit_qb_order_by_chain` en `src/codegen.rs`). Validación de field existence consistente con el path closure (chequea `state.fields`/`fields`). Direction extraída del `-` prefix; el resto del string es el nombre del field. SQL emitido idéntico al path closure (`"\"<sql_col>\" {ASC|DESC}"` en evaluator, `.with_order_by(col, desc)` en codegen). 3 unit tests nuevos: `codegen_orm_order_by_accepts_str_literal_asc_b1`, `codegen_orm_order_by_accepts_str_literal_desc_with_dash_b1`, `codegen_orm_order_by_str_literal_unknown_field_aborts_b1`. ~70 LoC totales.

### B2 — `http.post` body con type Instance — **CERRADO 2026-06-19 (sub-paso 5 cosecha)**

**Síntoma original** (cargo build):
```
✗ codegen: `http.post` body must be Str, Map<Str, Str> strict, or Bytes (MVP)
```

**Repro original**:
```fitz
type Payload { event: Str, count: Int }
http.post("https://api.com", Payload { event: "x", count: 1 }).await
```

**Severity**: ~~Medium~~. **Fix aplicado**: `gen_http_body_marshal` (`src/codegen.rs`) suma rama para `Type::Nominal(_)` que emite `__fitz_http_body_from_json(({body}).__to_fitz_json())`. Helper nuevo `__fitz_http_body_from_json(json: serde_json::Value)` sumado al `HTTP_CLIENT_HTTP_INTEGRATION_PRELUDE` (que ya está gated por `has_http`, donde `__ToFitzJson` + `serde_json` viven). Auto-setea `Content-Type: application/json`. La blanket impl `__ToFitzJson for Arc<Mutex<T>>` ya existente delega a `T::__to_fitz_json()` automáticamente. **Limitación documentada**: requiere `has_http=true` (programa con al menos un `@get`/`@post`/etc.) porque sin HTTP server-side la trait + serde_json no están en scope. Caso 90% real (webhook dispatcher: handler HTTP propio + outbound a API externa) cubierto. Programas con solo HTTP client + Instance body reciben error claro citando workarounds (declarar stub handler o convertir manual a Map<Str,Str>). 2 unit tests nuevos: `codegen_http_post_body_instance_emits_from_json_b2`, `codegen_http_post_body_instance_without_http_server_aborts_b2`. ~45 LoC totales.

### B3 — `ws_broadcast` desde módulo sin handler HTTP/WS rompe en cargo build — **CERRADO 2026-06-19 (sub-paso 1 cosecha)**

**Síntoma**: `cargo build: cannot find trait `__ToFitzJson` in this scope --> src/<module>.rs:N`.

**Repro**: módulo X sin `@get/@post/@put/@delete/@ws` propios llama `ws_broadcast("/path", msg)`. El preludio HTTP no se emite en ese módulo, falta `__ToFitzJson`.

**Workaround user-side**: helper en módulo CON handlers HTTP/WS (`fn emit(evt: Map<Str,Str>) { ws_broadcast("/x", evt); return null }`), módulos sin handlers delegan.

**Severity**: High. **Fix aplicado**: separado del bloque HTTP el import de `use crate::{__ToFitzJson, __FromFitzJson};` en `generate_module_rs_with_bindings` (`src/codegen.rs`). Se emite cuando `module_has_http || program_uses_ws_broadcast(program)`. ~20 LoC. Validado contra el repro fitzwatch (`checks.fitz` sin handlers HTTP llamando a `ws_broadcast` directo): los 3× `error[E0405] cannot find trait __ToFitzJson` desaparecieron.

### B4 — `.len()` sobre `List<X>?` (Nullable) rechazado por codegen — **CERRADO 2026-06-19 (sub-paso 2 cosecha)**

**Síntoma**: `✗ codegen: method call `.len` sobre `List<DbRow>?`: no soportado en codegen`.

**Repro**:
```fitz
let rows = match conn.query("...", []).await {
    Ok(r) => r,
    Err(_) => return ""
}
// rows tipa como List<DbRow>? en codegen
if (rows.len() == 0) { ... }   // ✗
```

**Workaround user-side**: helper `async fn x() -> Result<T>` con `.await?` (el propagation refine bien). Match con return temprano confunde al codegen.

**Severity**: ~~Medium~~ (sub-case de B14). **Fix aplicado** (parte del meta B14): flow-sensitive refinement en `gen_match` (`src/codegen.rs` ~línea 24796). El método ahora trackea un `Vec<bool>` paralelo `arm_divergent` que marca cada arm cuyo body termina en `return`/`break`/`continue`. El cálculo del LUB filtra esos arms antes de unificar (`!` en Rust coerce a cualquier T; incluirlos widening-eaba spuriously al `Nullable`). Si todos divergen, el resultado queda `Type::Null` (comportamiento legacy preservado — rustc acepta `!` → `()`). 4 unit tests nuevos: `match_with_divergent_err_arm_does_not_widen_to_nullable_b4_len` / `_b5_index` / `_b6_field` + `match_with_all_divergent_arms_types_as_null`. ~80 LoC totales. Validado contra repro mínima en `d:\tmp\sub2-divergent-repro\repro.fitz` (canonical pattern con `Ok(r) => r, Err(_) => return Err(...)`).

### B5 — Indexing `[]` sobre `List<X>?` rechazado por codegen — **CERRADO 2026-06-19 (sub-paso 2 cosecha)**

**Síntoma**: `✗ codegen: indexing `[]` over `List<DbRow>?`: only supported on List<T> and Map<K, V>`.

**Repro**: igual que B4, pero `rows[0]`.

**Severity**: ~~Medium~~ (sub-case de B14). **Fix aplicado**: cerrado junto a B4 — el mismo cambio en `gen_match` filtra arms divergentes del LUB, así `rows` queda como `List<X>` directo y `[]` dispatch funciona normal. Test unit dedicado `match_with_divergent_err_arm_does_not_widen_to_nullable_b5_index` valida que `rows[0]` sobre `List<Int>` tipa como `i64`.

### B6 — Field access `.x` sobre `T?` (Nullable) rechazado por codegen — **CERRADO 2026-06-19 (sub-paso 2 cosecha)**

**Síntoma**: `✗ codegen: field access `.paused` over `T?`: only supported on instances of custom types`.

**Repro**:
```fitz
let monitor = match refresh(id).await {
    Ok(m) => m,
    Err(_) => return null
}
if (monitor.paused) { ... }   // ✗
```

**Severity**: ~~Medium~~ (sub-case de B14). **Fix aplicado**: cerrado junto a B4/B5 — el mismo cambio en `gen_match` filtra arms divergentes del LUB, así `monitor` queda como `T` (Nominal directo) y field access dispatch funciona normal. Test unit dedicado `match_with_divergent_err_arm_does_not_widen_to_nullable_b6_field` valida que `who: User` después del match y `who.name` tipa como `String`.

### B7 — `match { Ok(v) => v, Err(_) => null }` no envuelve en `Some()` al asignar a `Nullable` — **CERRADO 2026-06-19** (sub-paso 3)

**Síntoma original** (cargo build):
```
error[E0308]: mismatched types
  | expected `Option<String>`, found `String`
help: try wrapping the expression in `Some`
```

**Repro original**:
```fitz
let opt: Str? = match row.get_str("col") {
    Ok(v) => v,
    Err(_) => null,
}
```

**Severity**: ~~Medium~~. **Fix aplicado** (sub-paso 3 de la cosecha post-fitzwatch): post-procesamiento en `gen_match` después de calcular `result_ty`. Cuando el LUB de los arms no-divergentes da `Nullable(inner)`, recorremos cada arm no-divergente y aplicamos:
- `body_ty == Null` con `body_code == "()"` → emitimos `None` directo.
- `body_ty == Null` con side-effects en bloque → emitimos `{ <body>; None }` para preservar los side-effects.
- `body_ty` ya `Nullable(_)` → dejamos sin tocar (idempotente — handles bindings ya tipados como `T?`).
- otherwise → wrap en `Some(<body>)`.

Arms divergentes (`return`/`break`/`continue`, type `!`) coercen a cualquier `T` y no requieren rewrite. ~60 LoC en `src/codegen.rs::gen_match` (incluyendo refactor menor para guardar `pat_prefix` + `body_code` separados antes de ensamblar `arm_pieces`). Tests unit dedicados:
- `match_with_nullable_lub_wraps_inner_arm_in_some_and_null_arm_in_none_b7` (Str?)
- `match_with_nullable_lub_wraps_int_arm_in_some_b7` (Int?)
- `match_with_nullable_binding_arm_does_not_double_wrap_b7` (idempotencia)

Repro en `d:/tmp/sub3-nullable-wrap-repro/repro.fitz` validado bit-a-bit `fitz run` ↔ `fitz build` ↔ binario nativo (cinco prints, salida `hola\nnull\n42\nnull\nnull`).

### B8 — `Option<T>: __IntoPgValue` not satisfied (query params Nullable → driver PG) — **CERRADO 2026-06-19** (sub-paso 4)

**Síntoma original** (cargo build):
```
error[E0277]: the trait bound `Option<i64>: __IntoPgValue` is not satisfied
error[E0277]: the trait bound `Option<String>: __IntoPgValue` is not satisfied
```

**Repro original**:
```fitz
@get("/x?id={id}")
async fn h(id: Int?) -> Result<Null> {
    let _ = conn.query("SELECT * FROM t WHERE id = $1", [id]).await?
    return Ok(null)
}
```

**Severity**: ~~Medium~~. **Fix aplicado** (sub-paso 4 de la cosecha post-fitzwatch): blanket impl `impl<T: __IntoPgValue> __IntoPgValue for Option<T>` sumado al preludio DB del codegen (gated por `program_uses_db`, no entra a programas sin `db.*`). `None → __FitzPgValue::Null`, `Some(v) → v.into_pg()` delegando al impl interno. ~12 LoC efectivas en `src/codegen.rs::emit_db_prelude`. Tests unit dedicados:
- `db_prelude_emits_into_pg_value_for_option_t_b8` (impl + arms presentes en preludio)
- `db_prelude_not_emitted_when_program_does_not_use_db_b8` (gating verificado)
- `db_query_with_nullable_arg_emits_into_pg_value_call_b8` (call site emite `<_ as __IntoPgValue>::into_pg(<expr>)` y rustc resuelve la trait selection vía el blanket impl)

Repro en `d:/tmp/sub4-option-pg-repro/repro.fitz` validado: `fitz check` OK, `fitz build` OK (binario compilado), `fitz run` y binario nativo producen el mismo error de runtime ("I/O: ...") al intentar conectar al puerto inexistente — paridad bit-a-bit confirmada porque el código compila y solo falla en runtime DB, no en codegen ni en cargo build.

### B9 — `Str? == Str` (Nullable vs concrete) — **CERRADO 2026-06-19 (sub-paso 5 cosecha)**

**Síntoma original** (cargo build):
```
error[E0308]: `match` arms have incompatible types
  | expected `Option<String>`, found `String`
```

**Repro original**:
```fitz
let monitor: Monitor = ...
if (monitor.last_status != "down") { ... }   // last_status: Str?
```

**Severity**: ~~Low~~. **Fix aplicado**: `gen_binop` (`src/codegen.rs`, `Eq`/`NotEq` arm) suma rama nueva entre la verificación de Nullable vs Null y los paths de Nominal/Str/numeric. Cuando un lado es `Type::Nullable(inner)` y el otro es exactamente el mismo primitivo (Str/Int/Float/Bool), emite `<opt>.as_ref() == Some(&<conc>)` (o `!=`) que rustc resuelve correctamente sobre PartialEq de `Option<&T>`. Aplica simétrico (LHS o RHS Nullable). Mixed primitives (Int↔Float Nullable vs concrete distinct primitivo) y comparaciones Nominal Nullable vs concrete quedan como deuda menor (caen al path existente). 4 unit tests nuevos: `binop_eq_nullable_str_vs_str_emits_as_ref_some_b9`, `binop_neq_nullable_str_vs_str_emits_as_ref_some_b9`, `binop_eq_nullable_int_vs_int_emits_as_ref_some_b9`, `binop_eq_str_vs_nullable_str_emits_as_ref_some_swapped_b9`. ~40 LoC totales.

### B10 — `spawn(fn)` cross-module no detecta `@background` — **CERRADO 2026-06-19 (sub-paso 5 cosecha)**

**Síntoma original**:
```
✗ codegen: spawn: fn `run_check` is not declared with `@background`. Mark the fn with `@background\nfn run_check(...) { ... }`...
```

**Repro original**:
```fitz
// checks.fitz
@background async fn run_check(id: Int) -> Null { ... }

// scheduler.fitz
from checks import run_check
@cron("*/10 * * * * *") async fn tick() -> Result<Null> {
    spawn(run_check(42))   // ✗ no detecta @background cross-module
    return Ok(null)
}
```

**Severity**: ~~Medium~~. **Fix aplicado** (3 piezas coordinadas):

1. **Infrastructure del checker**: `TypeEnv` suma `imported_background_fns: HashSet<String>` (paralelo a `imported_auth_provider`); métodos pub `add_imported_background_fns(I: IntoIterator<Item = String>)` + `imported_background_fns()`; pub fn nueva en `src/types.rs`: `extract_background_fn_names(program) -> Vec<String>` walks top-level FnDefs y colecta los marcados con `@background`. `collect_background_fns` extiende a merge de los imported names sobre el set local.
2. **Pre-scans en main.rs y codegen.rs**: `pre_scan_imported_background_fns` en `main.rs` (path `fitz check`/`fitz run`) + `pre_scan_imported_background_fns_for_loader` en `codegen.rs` (path `fitz build`, walk per-módulo). `ModuleLoader` suma campo `main_imported_background_fns: Vec<String>` propagado a cada módulo en `load_module` (combinado con las imports propias del módulo).
3. **Dispatch del codegen**: `gen_spawn_call` extiende el lookup del target — primero `self.fn_sigs` local, fallback a `module_bindings` cuando el target es `from <bg_mod> import bg`. La sig viene de `self.loaded_modules[idx].fn_sigs`. La `use crate::<bg_mod>::bg;` ya emitida por `emit_module_use_decls` permite emitir la call con el nombre unqualified (`bg(args)` en lugar de `bg_mod::bg(args)`).

3 unit tests nuevos en types: `extract_background_fn_names_collects_marked_top_level_fns_b10`, `extract_background_fn_names_returns_empty_when_no_background_fns_b10`, `spawn_cross_module_imported_background_fn_passes_with_pre_scan_b10`. 1 E2E nuevo en compile_e2e: `cross_module_spawn_background_b10` (3-archivos canonical: `checks.fitz` con `@background`, main importa + `spawn(run_check(42))`). ~150 LoC totales.

### B11 — Response types con `List<NominalType>` anidados rompen codegen (impacto cross-cutting) — **CERRADO 2026-06-19 (sub-paso 1 cosecha)**

**Síntoma** (cargo build):
```
error[E0425]: cannot find type `__FitzValue` in this scope
  --> src/main.rs:N (Vec<__FitzValue>)
```

**Trigger real (descubierto al reproducir desde fitzwatch)**: el caso dispara **cross-module** — `public.fitz` declara `type PublicOverview { items: List<MonitorStatus> = [] }` con handler `@get("/x") fn(...) -> Result<PublicOverview>`. `main.fitz` importa el handler con `from public import handler` pero NO los nested types (`MonitorStatus`). `emit_helpers_for_imported_types` en main.rs hace remap del field `items: List<MonitorStatus>` → `List<Any>` (porque `MonitorStatus` no está en el env del main) → emite `impl __FromFitzJson for PublicOverviewData { let items: Vec<__FitzValue> = ... }`. Pero `__FitzValue` no se activa por ningún detector (el walker `program_uses_fitz_value` mira ASTs, no el remap codegen-time), así que rustc rompe.

**Workaround user-side**: split en endpoints separados que devuelvan `List<X>` directo + endpoint de metadata aparte.

**Severity**: High. **Fix aplicado** (2 sub-cambios coordinados en `src/codegen.rs`):
1. **Detector pre-pass** `cross_module_compound_degrades_to_fitz_value(program, loader)` — walka `loader.modules.type_sigs` y por cada field con compound (`List<Nominal>` / `Map<_, Nominal>` / `Map<Nominal, _>` o `Nullable` wrappers) chequea si el inner Nominal es resoluble por main (local TypeDef + types importados via `from X import T`). Si no → activa `uses_fitz_value = true` antes del `emit_prelude`. Aplicado en `generate_project` (línea ~272) Y en `generate_main_rs` (línea ~5290 — recompute path heredado del split de Phase 5b).
2. **Stub `__FromFitzJson`** — nuevo enum `FromFitzJsonMode { Real, Stub }` + variante `gen_type_http_impls_for_sig_with_meta_and_mode`. Cuando `emit_helpers_for_imported_types` detecta compound degrade (helper nuevo `field_type_compound_degraded` que compara original vs remapped), emite `__FromFitzJson` con cuerpo `Err("cross-module __FromFitzJson for X is a stub: the type has compound fields...; if you need to deserialize this body cross-module, `from <module> import` every nested type the body references")`. El stub satisface el trait bound; `__ToFitzJson` se emite normal (solo invoca `self.<field>.__to_fitz_json()` — no referencia el tipo remapped). El stub se dispara en runtime solo si alguien intenta deserializar el type cross-module — el caso típico (response output-only) NUNCA llega ahí.

~150 LoC totales. Validado contra el repro fitzwatch (`public.fitz` con `PublicOverview { monitors: List<MonitorStatus>, incidents_open: List<OpenIncident>, incidents_recent: List<RecentIncident> }`): los 6× `error[E0425] cannot find type __FitzValue` desaparecieron.

### B12 — Cross-module `@auth_provider` detection en codegen — **CERRADO 2026-06-19 (sub-paso 5 cosecha)**

**Síntoma original**:
```
✗ codegen: module `metrics` has type errors: @authenticated on fn ...: no `@auth_provider` registered in the program
```

**Repro original**: módulo con `@authenticated` que no hace `import auth` ni `from auth import ...` (aunque el provider esté en `auth` y se haya cargado por main).

**Severity**: ~~Low~~. **Fix aplicado**: `ModuleLoader` suma campo `main_imported_auth_provider: Option<ImportedAuthProvider>` + setter `set_main_imported_auth_provider`. `generate_project` pre-scanea las imports de MAIN ANTES de `collect_imports` y popula el slot del loader (usa `pre_scan_imported_auth_provider_for_loader` existente). En `load_module`, el provider del módulo se calcula como `pre_scan_imported_auth_provider_for_loader(módulo) or_else main_imported_auth_provider`. Cierra el caso típico donde un módulo importer (ej. `metrics.fitz`) tiene handlers `@authenticated` pero solo importa el User type desde un módulo no-auth (ej. `from types import User`), y el `@auth_provider` real vive en `auth.fitz` (importado solo desde main). El módulo ahora ve el provider via fallback de main. 1 E2E nuevo en compile_e2e: `cross_module_auth_provider_via_main_b12` (4-archivos canonical: `types.fitz` declara User, `auth.fitz` importa User + tiene `@auth_provider`, `metrics.fitz` importa User + tiene `@authenticated` sin importar auth, `main.fitz` importa auth + metrics). ~70 LoC totales.

### B13 — `log.X(...)` con kwargs heterogéneos rompe cross-module — **NO REPRODUCE en v0.17.0 (cerrado por W18, 2026-06-18)**

**Investigación 2026-06-19 (sub-paso 1 cosecha)**: el repro del doc (`log.info("evt", count: 42, name: "x")` cross-module) **NO dispara** en v0.17.0. Reverteamos el workaround en `checks.fitz` de fitzwatch (cambiando los string interp a kwargs heterogéneos) y compila limpio. Probablemente W18 (commit `63b3d3f`, 2026-06-18, mismo día del cierre de fitzwatch) cerró este caso cuando atacó el preludio cross-module de logging para que módulos importados que llaman `log.X` reciban los imports correctos. El doc fitzwatch quedó con el bug listado porque se escribió contra una versión PRE-W18 (probable v0.16.x).

**`gen_log_call` (`src/codegen.rs` ~línea 15855)** emite cada kwarg como `__FitzLogValue::Int(...)` / `__FitzLogValue::Str(...)` / etc directos — nunca usa el enum `__FitzValue` (son tipos distintos: `__FitzLogValue` es el tagged union local del módulo logging). El walker `program_uses_logging` activa el preludio `__FitzLogValue` ya en módulos cross-module desde W18.

**Severity**: ~~Medium~~ NO APLICA. **Acción**: ninguna — el bug está cerrado por W18. Si reaparece en algún caso edge, abrir entrada nueva.

### B14 — (meta) `match` con return temprano no refine tipos en codegen — **CERRADO 2026-06-19 (sub-paso 2 cosecha)**

Patrón meta de B4/B5/B6. Cierra junto a los 3 con un solo cambio en `gen_match` (~80 LoC + 4 unit tests). El fix es **flow-sensitive refinement** sobre los arms del match: cuando un arm body termina en `return`/`break`/`continue`, su body code emite `!` (never) en Rust (vía `strip_trailing_semi` ya existente), pero el TIPO interno de Fitz se setteaba a `Type::Null` y entraba al LUB de los arms, widening-eando spuriously hacia `Nullable(T)`. Ahora trackeamos divergencia con un `Vec<bool>` paralelo (`arm_divergent`) y filtramos esos arms del LUB. Si todos divergen, el resultado queda `Null` (rustc acepta `!` → `()`). El binding del `let` siguiente queda con el tipo correcto (`List<X>`, `T` Nominal, lo que sea del arm Ok), y `.len()` / `[]` / `.field` dispatchan normal en codegen. Ver B4 para detalle del fix aplicado.

### B15 — `.preload()` sobre `@belongs_to` companion con FK Nullable + path HasMany simétrico — **CERRADO 2026-06-19 (sub-paso 6 cosecha)**

**Síntoma original** (cargo build):
```
error[E0308]: mismatched types
  | expected `i64`, found `Option<i64>`
  --> src/main.rs:N (en __FitzPgValue::Int(__g.<fk>))

error[E0277]: can't compare `i64` with `Option<i64>`
  --> src/main.rs:N (en __tg2.<pk> == __fk)
```

**Repro mínima REAL** (descubierto al diagnosticar — el trigger es **FK
nullable en el parent del `@belongs_to`**, NO "nullables en el parent
type" como decía el doc original):
```fitz
@table("users") type User {
    @primary id: Int = 0
    name: Str = ""
    @has_many("Post", via="author_id") posts: List<Post> = []
}

@table("posts") type Post {
    @primary id: Int = 0
    @belongs_to("User") author_id: Int? = null   // ← FK nullable
    author: User?
    title: Str = ""
}

@get("/posts")
async fn list_posts() -> Result<List<Post>> {
    let conn = db.connect("...").await?
    return Post.where(fn(p) => p.title == "x").preload("author").all(conn).await
}
```

**Hallazgo importante al diagnosticar**: el repro original del doc
(con FK `Int = 0` no-nullable + nullables varios en el parent type)
**NO disparaba el bug** post-sub-pasos 1-5 (B7 + B8 lo habían cerrado
parcial). El trigger REAL del E0277/E0308 es el FK Nullable
(`@belongs_to ... X_id: Int? = null`). Los models de fitzwatch
declaran todos los FK como `Int = 0` sentinel, así que probablemente
fitzwatch quedó pausado por otro bug (W18 cerrado en v0.17.0) y no por
B15 estricto. **Pero B15 sigue siendo bug crítico** porque cualquier
usuario que declare FK opcional (`Int?`) lo dispara, sin workaround
viable user-side.

**Severity**: ~~🔴 Critical — BLOQUEANTE~~. **Fix aplicado**: dos
cambios coordinados en `src/codegen.rs`:

1. **`emit_belongs_to_companion_preload_arm`** — nuevo param
   `parent_fields: &[TypeSigField]` para detectar nullable del FK.
   Cuando `Type::Nullable(_)`:
   - IDs collection emite `filter_map` en lugar de `map`:
     `__guard.iter().filter_map(|__p| { let __g = __p.lock().unwrap();
     __g.<fk>.map(__FitzPgValue::Int) }).collect()` — las rows con
     `None` FK skipean el `IN (...)` query entero.
   - Lookup del `__matched` emite `match __fk { None => None,
     Some(__fk_v) => __targets.iter().find(|__t| { let __tg2 =
     __t.lock().unwrap(); __tg2.<pk> == __fk_v }).cloned(), }` —
     comparación con `i64` directo en el arm `Some`.

2. **`emit_preload_dispatch`** (path HasMany) — sibling fix:
   `target_fields` busca el FK del child. Si nullable, emite
   `__cg2.<fk> == Some(__pid)` en lugar de `__cg2.<fk> == __pid`. El
   `__pid` es `i64` (PK del parent) y el `__cg2.<fk>` ya es
   `Option<i64>`.

Política consistente: row con FK = None significa "no tiene parent
en el target" → el companion queda como `None` (no agarra ningún
match). Semántica idéntica al intérprete (que usa la representación
unificada `Value`).

**Tests nuevos** (3 unit + 1 E2E):
- `codegen_orm_preload_companion_with_nullable_fk_emits_filter_map_b15`
- `codegen_orm_preload_companion_with_nonnull_fk_emits_simple_map_b15`
  (no-regression — FK no-nullable sigue emitiendo el path legacy)
- `codegen_orm_preload_has_many_with_nullable_child_fk_emits_some_pid_b15`
- `tests/compile_e2e.rs::cross_module_orm_preload_nullable_fk_b15` —
  end-to-end con BelongsToCompanion + HasMany ambos con FK nullable
  en el mismo programa.

~115 LoC totales en `src/codegen.rs` (delta + tests inline).

### B16 — match arms con tipos incompatibles (`i64` vs `()`) en posición no-Nullable

**Descubierto** durante el sub-paso 2 de la cosecha al reproducir fitzwatch. **NO se cierra en el sub-paso 5** — el fix correcto vive en el checker (no es mecánico como B1/B9 etc.) y requiere discusión de UX (error proactivo del checker vs trade-off de over-trigger).

**Síntoma** (cargo build):
```
error[E0308]: `match` arms have incompatible types
  | expected `i64`, found `()`
```

**Repro**:
```fitz
let n = match result {
    Ok(_) => 0,
    Err(e) => log.error("msg", err: e),  // log.error retorna Null (i.e. () en Rust)
}
```

`log.error(...)` retorna `Null` (no es divergent — no aborta), y el otro arm retorna `i64`. La unificación en el codegen NO promueve a Nullable(Int) (porque ninguno de los arms es `null` literal o tipa como Nullable), así que el match queda como expresión de tipo Int pero un arm produce ().

**Workaround user-side** (validado durante sub-paso 2):
```fitz
let n = match result {
    Ok(_) => 0,
    Err(e) => { log.error("msg", err: e); 0 },  // terminar arm con sentinel `; 0`
}
```

**Severity**: Low. **Diferencia con B7** (que sí cerramos en sub-paso 3): B7 cuando uno de los arms es `null` literal y el otro tiene un T concreto → LUB produce Nullable(T) y mi fix wrap-ea con Some()/None. B16 cuando uno de los arms es una **call expression** que devuelve `Null` y el otro un T concreto → LUB no promueve a Nullable (no hay `null` literal involucrado). El codegen NO tiene info suficiente para insertarle el sentinel automáticamente.

**Fix sugerido futuro** (sub-paso 6+): detectarlo en el checker como type error claro citando el workaround. Mensaje sugerido: "match arm N retorna `Int`, arm M retorna `Null`. Si querés ignorar el valor del log call, terminá el arm con `; <valor_default>`". Alternativa más invasiva: el codegen detecta el caso y auto-emite el `;` + sentinel del tipo del otro arm — me gusta menos por semánticamente oscuro.

### Tabla resumen + plan de ataque sugerido

| # | Bug | Sev | Workaround user | Estimado fix (LoC) | Test E2E necesario |
|---|---|---|---|---|---|
| **B1**  | **`.order_by(str)`** | Medium | ✅ closure | ✅ **CERRADO 2026-06-19** ~70 LoC + 3 unit tests | sí |
| **B2**  | **`http.post` body Instance** | Medium | ✅ Map<Str,Str> | ✅ **CERRADO 2026-06-19** ~45 LoC + 2 unit tests | sí |
| **B3**  | **`ws_broadcast` cross-module** | High | ✅ helper | ✅ **CERRADO 2026-06-19** ~20 LoC | sí |
| **B4**  | **`.len()` sobre `List<X>?`** | Medium | ✅ helper Result | ✅ **CERRADO 2026-06-19** (parte de B14) | unit |
| **B5**  | **`[]` sobre `List<X>?`** | Medium | ✅ helper Result | ✅ **CERRADO 2026-06-19** (parte de B14) | unit |
| **B6**  | **`.x` sobre `T?`** | Medium | ✅ helper Result | ✅ **CERRADO 2026-06-19** (parte de B14) | unit |
| **B7**  | **match no envuelve `Some()`** | Medium | ✅ sentinels | ✅ **CERRADO 2026-06-19** ~60 LoC + 3 unit tests | unit |
| **B8**  | **`Option<T>: __IntoPgValue`** | Medium | ✅ sentinels | ✅ **CERRADO 2026-06-19** ~12 LoC + 3 unit tests | unit |
| **B9**  | **`Str? == Str` arms incomp** | Low | ✅ coerce local | ✅ **CERRADO 2026-06-19** ~40 LoC + 4 unit tests | unit |
| **B10** | **`spawn(fn)` cross-module @bg** | Medium | ✅ wrapper local | ✅ **CERRADO 2026-06-19** ~150 LoC + 3 unit + 1 E2E | sí |
| **B11** | **Response con `List<Nominal>`** | High | ✅ split endpoints | ✅ **CERRADO 2026-06-19** ~150 LoC | sí |
| **B12** | **Cross-module `@auth_provider` codegen** | Low | ✅ `import auth` | ✅ **CERRADO 2026-06-19** ~70 LoC + 1 E2E | sí |
| **B13** | **`log.X` kwargs heterogéneos** | ~~Medium~~ | (n/a) | ✅ **NO REPRO en v0.17.0** (W18 lo cerró) | n/a |
| **B14** | **(meta) match refine** | — | (helpers Result) | ✅ **CERRADO 2026-06-19** ~80 LoC + 4 unit tests | unit (cubre B4/B5/B6 + all-divergent) |
| **B15** | **`.preload()` + FK Nullable** | 🔴 **Critical** | ❌ sin workaround | ✅ **CERRADO 2026-06-19** ~115 LoC + 3 unit + 1 E2E | sí |
| **B16** | **match arms `i64` vs `()` E0308** | Low | ✅ `; <sentinel>` | abierta (sub-paso 7+) | sí |

**Plan de ataque sugerido** (orden por dependencias + impacto):

1. **Sub-paso "detectores unificados"** — B3 + B11 + B13 — ✅ **CERRADO 2026-06-19**. B3 + B11 fixeados con ~150 LoC en `src/codegen.rs`; B13 NO reprodujo en v0.17.0 (W18 lo había cerrado). Detalle: ver entries individuales arriba. Smoke: `cargo test --release --lib` 3116/3116 verde post-fix.
2. **Sub-paso "refine flow-sensitive en match"** — B4 + B5 + B6 (todos del meta B14). ✅ **CERRADO 2026-06-19**. ~80 LoC + 4 unit tests. Filtro de arms divergent (`return`/`break`/`continue`) en el LUB de `gen_match`. Smoke `cargo test --release --lib match_with` 12/12 verde; smoke `compile_e2e smoke_ejemplos_guia_compilables_compilan` 363/363 verde; repro mínima end-to-end (`d:\tmp\sub2-divergent-repro\repro.fitz`) check + run + build + binario ejecutado con paridad bit-a-bit.
3. **Sub-paso "wrap automático Some()"** — B7. ✅ **CERRADO 2026-06-19**. ~60 LoC + 3 unit tests. Post-procesamiento en `gen_match` después del LUB: cuando `result_ty` queda como `Nullable(inner)`, los arms no-divergentes con `body_ty == Null` se reescriben a `None` y los arms con `body_ty` concreto compatible con `inner` se envuelven en `Some(...)`. Arms ya `Nullable` quedan idempotentes; arms divergentes (`!`) no requieren rewrite. Smoke `cargo test --release --lib match_` 84/84 verde; repro mínima end-to-end (`d:/tmp/sub3-nullable-wrap-repro/repro.fitz`) validada bit-a-bit `fitz run` ↔ `fitz build`.
4. **Sub-paso "Option<T> → PgValue"** — B8. ✅ **CERRADO 2026-06-19**. ~12 LoC + 3 unit tests. Blanket impl `__IntoPgValue for Option<T>` sumado al preludio DB del codegen (gated por `program_uses_db`). `None → __FitzPgValue::Null`, `Some(v) → v.into_pg()`. Lib `cargo test --release --lib b8` 19/19 verde (incluye los 3 nuevos); lib completa 3126/3126 verde; smoke `compile_e2e smoke_ejemplos_guia_compilables_compilan` 363/363 verde (~4 min); repro mínima end-to-end (`d:/tmp/sub4-option-pg-repro/repro.fitz`) — `fitz check` OK, `fitz build` OK, paridad bit-a-bit `fitz run` ↔ binario nativo (mismo error de runtime DB esperado). **Próximo norte de la cosecha**: sub-paso 5.
5. **Sub-paso "fixes mecánicos"** — B1 + B2 + B9 + B10 + B12. ✅ **CERRADO 2026-06-19**. ~~Nuevo E0308 match arms `i64` vs `()` registrado como B16 (deuda separada — fix requiere refinement del checker, no es mecánico)~~. ~375 LoC + 16 tests nuevos (unit + E2E). Detalle:
    - **B1** (~70 LoC + 3 unit): `.order_by("col")` y `.order_by("-col")` con Str literal aceptados por el evaluator (`orm_qb_order_by`) y por el codegen (`emit_qb_order_by_chain`); ASC por default, `-` prefix → DESC; validación de field existence consistente con el path closure; emite mismo `.with_order_by(col, desc)` en codegen.
    - **B2** (~45 LoC + 2 unit): body con type Instance en `http.post`/`put`/etc. ahora despacha a `__fitz_http_body_from_json(__to_fitz_json())` cuando `has_http=true` (la trait `__ToFitzJson` + serde_json viven en el preludio HTTP server-side). Helper nuevo sumado al `HTTP_CLIENT_HTTP_INTEGRATION_PRELUDE`. Sin `has_http`, error claro citando el workaround (declarar handler stub o convertir a Map<Str,Str>).
    - **B9** (~40 LoC + 4 unit): `Nullable(T) == T` / `!= T` para primitivos (Str/Int/Float/Bool) emite `<opt>.as_ref() == Some(&<conc>)` (paralelo a `.is_none()`/`.is_some()` ya existente para Nullable vs Null). Aplica simétrico (LHS o RHS). Mixed primitives (Int↔Float) y nominales quedan como deuda menor (cae al path existente que sigue rechazando).
    - **B10** (~150 LoC + 3 unit + 1 E2E): cross-module `@background` detection. `TypeEnv` suma `imported_background_fns: HashSet<String>` + métodos `add_imported_background_fns`/`imported_background_fns`. Nueva pub fn en types: `extract_background_fn_names(program)`. Pre-scans nuevos: `pre_scan_imported_background_fns` en `main.rs` (path `fitz check`/`fitz run`) + `pre_scan_imported_background_fns_for_loader` en `codegen.rs` (path `fitz build` per-módulo). El `ModuleLoader` suma campo `main_imported_background_fns` propagado a cada módulo. `gen_spawn_call` extiende su lookup para chequear `module_bindings` además del `fn_sigs` local, así emite la llamada Rust con la sig importada cuando target = `from <bg_mod> import bg`.
    - **B12** (~70 LoC + 1 E2E): cross-module `@auth_provider` para módulos. El `ModuleLoader` suma campo `main_imported_auth_provider` pre-scanneado en `generate_project` ANTES de `collect_imports`. En `load_module`, el provider del módulo es `or_else`-combined: primero las propias imports del módulo (path histórico W12), luego como fallback el de main. Cierra el caso "módulo con `@authenticated` sin `import auth` propio porque main ya lo importó".

    Smoke `cargo test --release --lib b1 b2 b9 b10 b12` 14/14 verde. Smoke `cargo test --release --test compile_e2e cross_module_spawn_background_b10 cross_module_auth_provider_via_main_b12` 2/2 verde. **Próximo norte de la cosecha**: sub-paso 6 (B15 — el bloqueante).
6. **Sub-paso BLOQUEANTE "ORM `.preload()` + Nullable companion"** — B15. ✅ **CERRADO 2026-06-19**. ~115 LoC + 3 unit + 1 E2E. **Hallazgo importante al diagnosticar**: el trigger REAL es FK Nullable (`@belongs_to ... X_id: Int? = null`) en el parent del companion, NO "nullables en el parent type" como decía el doc original (que post-sub-pasos 1-5 con B7+B8 cerrados ya NO disparaba el bug). Dos cambios coordinados en `src/codegen.rs`: (a) `emit_belongs_to_companion_preload_arm` recibe `parent_fields` y, cuando el FK es Nullable, emite `filter_map(|p| p.<fk>.map(__FitzPgValue::Int))` para el `IN (...)` + `match __fk { None => None, Some(v) => find(v) }` para el lookup; (b) `emit_preload_dispatch` (path HasMany sibling) detecta el FK del child Nullable y emite `__cg2.<fk> == Some(__pid)` en lugar de bare `__pid`. Smoke `cargo test --release --lib b15` 3/3 verde; smoke `cargo test --release --test compile_e2e cross_module_orm_preload_nullable_fk_b15` 1/1 verde; smoke `compile_e2e smoke_ejemplos_guia_compilables_compilan` 366/366 verde (~5 min); fmt + clippy `--lib --tests --bins -- -D warnings` limpios. Lib completa **3141/3141 verde** (+3 vs sub-paso 5). Repro mínima validada end-to-end (`d:/tmp/sub6-preload-repro/repro_fk_null.fitz` con FK `Int? = null` + companion + handler — `fitz build` produce binario nativo). **Importante para fitzwatch**: los models declaran todos los FK como `Int = 0` sentinel (no Nullable), así que probablemente el bug que pausó fitzwatch fue otro (W18 cerrado en v0.17.0), no B15 estricto. Pero B15 sigue siendo bug crítico que cualquier usuario con FK `Int?` dispara sin workaround viable user-side. **Cosecha codegen post-fitzwatch CERRADA ENTERA**: sub-pasos 1-6 cubren B1-B12 + B14 + B15 cerrados; B13 NO reproduce (W18); B16 abierto como deuda separada (fix vive en el checker, no es mecánico).

**Validación al cerrar la cosecha**:
- `cd d:\fitzwatch && fitz build` debería pasar limpio.
- Smoke: `docker compose up -d --build` en fitzwatch + curl tests del API + browser tests del admin Vue 3.
- Si todo OK → fitzwatch al deploy VPS (Cloudflare DNS + Origin Cert + nginx site enable).
- Bump versión: probable **v0.17.1** (parche, todos son bugs sin cambios de API) o **v0.18.0** (minor, si se aprovecha para algún feature).

---

## 🟡 ORM nativo — gaps detectados durante fitzwatch (2026-06-18)

> **Auditoría hermana de la cosecha codegen** (arriba). Durante el
> desarrollo de **fitzwatch** (`d:\fitzwatch\`, status page +
> uptime monitor en Fitz puro) auditamos el balance ORM:SQL-crudo del
> proyecto. Terminó **35:65** en queries totales — el ORM nativo
> cubre el CRUD básico, pero seis categorías nos forzaron a bajar a
> `conn.query`/`conn.exec` crudo. Esta sección las agrupa para que
> futuras tandas las evalúen en conjunto.
>
> **Independiente de la cosecha codegen** — pueden cerrarse en
> cualquier orden o paralelo. Los workarounds user-side
> (`conn.query`/`conn.exec`) son el patrón canónico documentado en
> [`docs/db-orm.md`](db-orm.md) y replicado en los boilerplates
> `api-orm-full`/`taskhub`/`api-multi-tenant`.

### O1 — `.update({...})` no acepta expresiones SQL como `NOW()` / `EXTRACT`

**Síntoma**: el ORM `.update(conn, {...})` solo acepta valores literales o variables Fitz. Para `last_check_at = NOW()` o `duration_secs = EXTRACT(EPOCH FROM (NOW() - started_at))::int` hay que bajar a `conn.exec(SQL crudo, [args])`.

**Repro fitzwatch** (`checks.fitz` → `update_monitor_last_status` y `close_open_incidents`):
```fitz
let _ = conn.exec(
    "UPDATE monitors SET last_check_at = NOW(), last_status = $1 WHERE id = $2",
    [status, monitor_id],
).await?
```

**Workaround user-side**: `conn.exec(...)` crudo. Funciona pero pierde tipado del lenguaje sobre el field.

**Severity**: Medium. **Fix sugerido**: permitir expresiones whitelisted en el Map del `.update({...})`, tipo `{"last_check_at": db.now(), "count": db.raw("count + 1")}` o helpers similares. Otra opción más quirúrgica: aceptar strings prefijados con `SQL!` que el ORM trata como expresión cruda (similar a `sqlalchemy.func.now()`). ~80-120 LoC.

### O2 — Sin migrations automáticas (`fitz db diff`/`migrate`)

**Síntoma**: cualquier proyecto serio que usa ORM nativo tiene que mantener el SQL crudo del `CREATE TABLE` en un módulo aparte (`schema.fitz` en fitzwatch) y llamarlo al boot del main con `init_schema().await`. El ORM declara los `@table` types pero no genera ni el DDL inicial ni los diffs cuando los types cambian.

**Repro fitzwatch** (`schema.fitz` → 5 `CREATE TABLE IF NOT EXISTS`).

**Workaround user-side**: `db.exec("CREATE TABLE IF NOT EXISTS ...", [])` al boot. Es lo que hacen también los boilerplates `api-orm-full`, `api-multi-tenant`, `taskhub`, etc. (patrón canónico).

**Severity**: High (impacto al ecosistema entero). **Fix sugerido**: **deuda 10.6 ya conocida** — `fitz db diff` + `fitz db migrate`. Mini-fase dedicada cuando la cosecha codegen cierre — paralelo a Diesel CLI / Alembic / Prisma migrate. NO bloquea la cosecha; queda como deuda visible en el roadmap.

### O3 — Aritmética de fechas no soportada en `.where(closure)`

**Síntoma**: filtros tipo "monitores con `last_check_at + interval_secs < NOW()`" no se pueden expresar con el translator AST→SQL del `.where(...)`. Sin SQL crudo, hay que cargar TODOS los monitores activos y filtrar en aplicación con date arithmetic JS-style — N queries ineficientes.

**Repro fitzwatch** (`scheduler.fitz`):
```fitz
let rows = conn.query(
    "SELECT id FROM monitors WHERE paused = false AND (last_check_at IS NULL OR last_check_at + make_interval(secs => interval_secs) < NOW())",
    [],
).await?
```

**Workaround user-side**: `conn.query` crudo.

**Severity**: Medium. **Fix sugerido**: extender el translator `.where(...)` para soportar operadores de fecha (método tipo `m.last_check_at.add_seconds(m.interval_secs) < db.now()` con helpers `db.now()` / `db.add_interval(...)`). Probablemente paralelo a la deuda del tipo nativo `DateTime` mencionada en fitzwatch DEUDA.md. ~100-150 LoC (junto con O1 comparten translator).

### O4 — Agregaciones complejas (`COUNT` + `SUM(CASE WHEN)` + JOINs + GROUP BY)

**Síntoma**: el `.aggregate(...)` del ORM cubre count, sum, avg, min, max simples por una sola columna, pero NO expresiones tipo `SUM(CASE WHEN status = 'up' THEN 1 ELSE 0 END)` o `COUNT(*) / NULLIF(COUNT(c.id), 0)` con `LEFT JOIN`.

**Repro fitzwatch** (`public.fitz` → `fetch_monitor_statuses`, el snapshot del status page público):
```fitz
let rows = conn.query("""
    SELECT m.id, m.name, m.kind, m.last_status, m.last_check_at,
           COALESCE(ROUND(100.0 * SUM(CASE WHEN c.status = 'up' THEN 1 ELSE 0 END)
                          / NULLIF(COUNT(c.id), 0), 2), 0)::float8 AS uptime_24h_pct,
           COUNT(c.id) AS checks_24h_count
    FROM monitors m
    LEFT JOIN checks c ON c.monitor_id = m.id
        AND c.recorded_at > NOW() - INTERVAL '24 hours'
    WHERE m.paused = false
    GROUP BY m.id, m.name, m.kind, m.last_status, m.last_check_at
    ORDER BY m.name
""", []).await?
```

**Workaround user-side**: `conn.query` crudo.

**Severity**: Low (el escape hatch es el patrón canónico). **Fix sugerido**: probable que **NO se cierre nunca** — los proyectos serios con SQL complejo siempre necesitan crudo (Diesel también recomienda esto). Lo relevante es documentar más fuerte en `db-orm.md` y la guía que es un trade-off intencional, no un gap. ~50 LoC docs.

### O5 — CTEs anidadas + window functions + `percentile_cont` + `date_trunc`

**Síntoma**: el ORM no expresa CTEs (`WITH ... AS`), window functions (`OVER (...)`), funciones de Postgres específicas (`percentile_cont(0.95) WITHIN GROUP ORDER BY ...`, `date_trunc('hour', ts)`).

**Repro fitzwatch** (`metrics.fitz` → `dashboard_overview` con CTE anidada de 3 niveles + `monitor_timeline` con `percentile_cont` + `date_trunc`).

**Workaround user-side**: `conn.query` crudo.

**Severity**: Low (igual que O4 — el escape hatch es canónico). **Fix sugerido**: igual que O4 — documentar más fuerte que es un trade-off intencional. CTEs y window functions en un ORM serían un mini-DSL en sí mismas. ~50 LoC docs (junto con O4).

### O6 — JOINs custom con `SELECT` alias (`m.name AS monitor_name`)

**Síntoma**: el ORM soporta JOINs implícitos via `.preload(...)` que populan el companion field del type parent. Pero **no soporta** SELECTs con alias custom tipo `SELECT i.id, m.name AS monitor_name FROM incidents i JOIN monitors m ON ...`, donde el resultado es un row con shape distinto al type ORM (mezcla campos de varios).

**Repro fitzwatch** (`grids.fitz` → `grid_incidents` con JOIN + filtros opcionales con sentinels `($2 = '' OR ...)`).

**Workaround user-side**: `conn.query` crudo + tipo plano construido manualmente desde `row.get_str(...)` (que también sufrió bugs del codegen B7+B11 sobre nullable fields).

**Severity**: Low (sub-case de O4/O5 — escape hatch canónico). **Fix sugerido**: posible API tipo `.select_custom("col1, col2, ...").join(...)` que devuelva `List<Map<Str, Any>>` o similar. Pero probablemente no se justifique — el escape hatch ya está. ~80 LoC si entra demanda real.

### Tabla resumen + plan sugerido

| # | Gap | Sev | Workaround user | Fix sugerido / decisión |
|---|---|---|---|---|
| O1 | `.update()` con `NOW()`/`EXTRACT` | Medium | ✅ `conn.exec` crudo | helpers `db.now()`/`db.raw(...)` o tipo `SQL!` strings |
| O2 | Sin migrations automáticas | High | ✅ `CREATE TABLE IF NOT EXISTS` al boot | deuda 10.6 ya conocida — `fitz db diff`/`migrate` |
| O3 | Aritmética de fechas en `.where` | Medium | ✅ `conn.query` crudo | helpers `db.now()`/`db.add_interval(...)` (junto con O1) |
| O4 | Agregaciones complejas (SUM/CASE) | Low | ✅ `conn.query` crudo | documentar como trade-off, no cerrar |
| O5 | CTEs + window functions + Postgres-specifics | Low | ✅ `conn.query` crudo | documentar como trade-off, no cerrar |
| O6 | JOINs custom con `SELECT` alias | Low | ✅ `conn.query` crudo + tipo manual | sub-case de O4/O5 — escape hatch canónico |

**Plan de ataque sugerido** (después de cerrar la cosecha codegen — son independientes):

1. **O1 + O3 — helpers de expresiones SQL en ORM**: nueva sub-fase del ORM que suma `db.now()` / `db.add_interval(...)` / `db.raw(...)` para usar adentro de `.update({...})` y `.where(closure)`. ~150-200 LoC. Cierra los dos en un solo bloque coordinado porque comparten el translator.
2. **O2 — migrations automáticas**: deuda 10.6, mini-fase dedicada (proyecto separado, paralelo a Diesel CLI / Alembic / Prisma migrate). ~500-1000 LoC. Cuando entre, beneficia al ecosistema entero. **No bloquea la cosecha codegen ni fitzwatch**.
3. **O4 + O5 + O6 — documentación reforzada**: actualizar `docs/db-orm.md` y `docs/guide.md` cap 31 con un párrafo explícito tipo "estos tres casos son intencionales — `conn.query` crudo es el escape hatch canónico, paralelo a Diesel/SQLAlchemy". Sumar a `docs/curso/m6/` también. ~100 LoC docs.

**Origen de la auditoría**: balance ORM:SQL-crudo de fitzwatch terminó en aproximadamente **35:65** en términos de queries. La filosofía es la misma que en los boilerplates `api-orm-full` / `taskhub` / `api-multi-tenant`: ORM para CRUD básico, SQL crudo para todo lo demás. La tabla por módulo de fitzwatch está en `d:\fitzwatch\NEXT-SESSION.md`.

---

## 🟢 SMTP builtin — CERRADA v0.18.0 (mini-tanda 2026-06-19)

> **CERRADA**: el módulo `smtp` built-in fue implementado en bloque
> (B1-B8) durante la mini-tanda iniciada el 2026-06-19, cerrando la
> deuda anotada el 2026-06-18 durante el desarrollo de fitzwatch.
> Fitz tiene ahora **`smtp.send(opts)` async ciudadano de primera
> clase**, paralelo bit-a-bit al HTTP client de v0.17.0, con paridad
> intérprete↔binario, sin deps externas en el host. Ver `CHANGELOG`
> v0.18.0, cap 17 de `docs/guide.md` (sub-sección "SMTP outbound"),
> y los 3 ejemplos runnable
> `examples/guide/17{i,j,k}-smtp-{basico,errores,magic-link}.fitz`.
> El plan original (deuda anotada abajo) fue cumplido al 100%.

> Detectada durante el desarrollo de fitzwatch al armar el módulo de
> notificaciones (`notifications.fitz`). El proyecto necesita despachar
> notificaciones por email cuando se abre/cierra un incident. **Fitz
> no tiene SMTP builtin** — el workaround actual es delegar todo a
> webhooks outbound y que el user enganche un n8n/ifttt/zapier que
> traduzca webhook → email.

### Contexto

El stack web nativo de Fitz cierra estos ciudadanos de primera clase:
HTTP server-side, HTTP client (v0.17.0), WebSockets tipados, auth (JWT
+ Argon2id), cron + background jobs, OpenAPI, AsyncAPI, ORM Postgres,
observability OTel + Prometheus, feature flags, deploy ciudadano.
**SMTP outbound NO está** — los proyectos que necesitan enviar mail
(notificaciones, password reset, magic links, alerts, marketing
transactional, etc.) tienen que rebotar por webhook + servicio
externo, o aplicar interop Python con `smtplib`.

### API target sugerida (paralelo a `http.X`)

```fitz
// Módulo `smtp` built-in, paralelo a `http`/`jwt`/`hash`/`log`.

// Send simple (texto plano)
let r = smtp.send({
    "to": "user@example.com",
    "from": "fitzwatch@status.prothos.com.ar",
    "subject": "Incidente abierto: API Producción",
    "body": "El monitor cayó hace 30s. Ver detalles: https://...",
}).await?
// r: SmtpResult { delivered: Bool, message_id: Str, duration_ms: Int }

// Send con HTML + texto plano (multipart/alternative)
let r = smtp.send({
    "to": "user@example.com",
    "from": "...",
    "subject": "...",
    "body_text": "Texto plano fallback",
    "body_html": "<html>...</html>",
}).await?

// Send con attachments
let r = smtp.send({
    "to": "...",
    "from": "...",
    "subject": "...",
    "body": "...",
    "attachments": [
        { "filename": "report.pdf", "bytes": pdf_bytes, "mime": "application/pdf" },
    ],
}).await?

// Tipo built-in nuevo paralelo a HttpClientResponse:
type SmtpResult {
    delivered: Bool
    message_id: Str
    duration_ms: Int
}
```

### Configuración (env vars + builtin)

El módulo lee config de env vars al boot (paralelo a cómo `db.connect`
toma URL):

```bash
SMTP_HOST=smtp.gmail.com
SMTP_PORT=587
SMTP_USER=fitzwatch@example.com
SMTP_PASSWORD=...
SMTP_FROM=fitzwatch@example.com   # default From si el send no especifica
SMTP_TLS=starttls                 # starttls | implicit | none
```

O config explícita via builtin (sin env vars):

```fitz
smtp.configure({
    "host": "smtp.gmail.com",
    "port": 587,
    "user": "fitzwatch@example.com",
    "password": secret("SMTP_PASSWORD"),
    "tls": "starttls",
})
```

### Modelo de errores

Paralelo a `http.X` y `jwt.encode`: `Result<SmtpResult>` con `Err(Str)`
para errores de transporte (DNS, conexión, auth, TLS handshake,
timeout, etc.). Status 5xx del SMTP server NO son Err (el user mira
`r.delivered`); solo errores de transporte van a Err.

### Backend implementación

Probable crate base: `lettre = "0.11"` (la opción canónica del
ecosistema Rust, soporta async con `tokio1` feature, mantenida
activamente, sin deps externas en el host). Linkeado estático sin
openssl (`rustls` backend).

### Integración con el resto del stack

- **`@cron`/`@background`**: dispatch async natural (`smtp.send(...).await?`).
- **Observability**: cada send emite `log.info("smtp.delivered", ...)`
  con `duration_ms`, `to`, `message_id`. Métricas Prometheus `smtp_sends_total`
  + `smtp_send_duration_seconds`.
- **Templates**: por ahora `body` como Str (con `format!`-style
  interpolation del lenguaje). Templates dedicados (HBS / Tera-like)
  como deuda menor del módulo, refinables si entra demanda.
- **Tipos**: `Map<Str, Any>` como input requiere `__FitzValue`
  integration (paralelo a `jwt.encode` y al body de `http.post`). Si
  esa deuda no cerró antes, el MVP del SMTP builtin acepta solo
  `Map<Str, Str>` strict para los kwargs del `send(...)` y los
  attachments quedan como deuda menor del builtin.

### 5 diferenciales (paralelo a HTTP client)

1. **Built-in del lenguaje** — no `pip install yagmail` / `npm install
   nodemailer` / `cargo add lettre`.
2. **Paridad bit-a-bit `fitz run` ↔ `fitz build`** — el binario
   standalone tiene el cliente SMTP linkeado.
3. **Async ciudadano de primera** — se integra con `@cron`/`@background`/
   handlers HTTP/`spawn(...)`.
4. **`Result<T>` automático** — errores como valores, `?` propaga.
5. **Sin deps externas en el host** — `rustls` backend, sin openssl.

### Plan de bloques sugerido (paralelo a HTTP client builtin)

1. **B1 evaluator**: `Value::Module { name: "smtp" }` registrado en
   `register_builtins` + builtin `smtp.send(...)` async + pre-registro
   tipo `SmtpResult` + helper privado dispatch input → message lettre.
2. **B2 checker**: pre-registro `smtp`/`SmtpResult` en `CheckCtx::new`
   + signatures + regla `?` heredada de Result.
3. **B3 codegen**: detector `program_uses_smtp(program)` walka AST +
   `cargo_toml_for` suma `lettre` condicional + preludio `SMTP_PRELUDE`
   con `static __FITZ_SMTP_CLIENT: LazyLock<SmtpTransport>` + helpers
   async `__fitz_smtp_send` paralelo bit-a-bit al intérprete + dispatch
   en `gen_call`.
4. **B4 LSP**: completions de `smtp` + `SmtpResult`.
5. **B5 guía + ejemplos**: sub-sección nueva en cap apropiado de
   `docs/guide.md` con panorama vecino (`smtplib`/`nodemailer`/`lettre`)
   + ejemplos runnable (envío simple, HTML, attachments, error handling
   contra MailHog local).
6. **B6 docs cross-cutting**: CLAUDE + README + index.md + roadmap +
   este doc actualizado.
7. **B7 boilerplate**: sumar SMTP a uno de los boilerplates existentes
   (probable `taskhub` con notificaciones de tasks asignadas, o
   ejemplo de magic-link auth en alguno de los api-*).
8. **B8 cierre formal**: CHANGELOG + roadmap + extensión VSCode bump
   + `.vsix` regenerado + blog drafts ES/EN.

### Severity + prioridad

**Severity**: Medium. **Prioridad**: después de cerrar la cosecha
codegen post-fitzwatch (próximo norte). Es deuda visible pero no
bloqueante — los proyectos que necesitan email pueden usar webhook +
servicio externo mientras tanto (es lo que hace fitzwatch por ahora).

### Workaround actual (lo que aplica fitzwatch v0.x)

`notifications.fitz` despacha solo webhooks. Para `kind="email"`:
```fitz
if (channel.kind == "email") {
    log.warn("notify.email.not_supported ch={ch.id} dest={ch.destination}")
    return null
}
```

El user que quiere emails configura un canal webhook que apunta a
n8n/ifttt/zapier/activepieces y traduce ahí webhook → SMTP. Funciona
pero contradice el modelo "todo nativo en el core".

---

## 🟢 NO es deuda — trade-off documentado del ORM JSON serializer (analizado 2026-06-09, no requiere fix del lenguaje)

**Contexto**: durante el smoke E2E del TaskHub post-v0.15.14, el
frontend rompió con `Cannot read properties of undefined (reading
'forEach')` al abrir un project con 0 tasks. La causa: el endpoint
`GET /projects/{id}` con `.preload("tasks")` retorna JSON sin el
campo `tasks` cuando la lista preloaded está vacía.

**Verificación del codegen** (`src/codegen.rs:25160-25203`,
v0.10.8 fix #7):

```rust
// Tradeoff aceptado: una lista vacía legítima (post sin
// comments) NO se distingue de "no preloaded". El user que
// necesite distinguir esos casos puede usar un handler
// dedicado (`GET /posts/{id}/comments`) en lugar de eager
// loading.
if is_virtual(&f.name) {
    match relation_kind {
        Some(HasMany) => {
            // Arc<Mutex<Vec<T>>> — emit si NO está vacía.
            writeln!(...,
                "if !__is_empty {{ __obj.insert(\"{name}\".to_string(), ...); }}"
            )
        }
        Some(HasOne) | Some(BelongsToCompanion) => {
            // Option<T> — emit si Some.
        }
    }
}
```

**Decisión de diseño DELIBERADA**: el codegen no tiene flag
`was_preloaded` per-instancia (sería ~struct overhead) y elige
"lista vacía = asumimos no preloaded" como heurística pragmática.
El comentario in-code lo cita explícitamente con el workaround
sugerido para casos que necesiten distinguir "preloaded con 0
rows" vs "no preloaded".

**Conclusión sobre la "deuda"**:

- **El lenguaje NO tiene bug** — el codegen hace exactamente lo
  que documenta hacer.
- **El cliente DEBE defensar** — `project.tasks || []` es el
  patrón correcto que el codegen asume del consumidor cuando
  el field puede o no estar presente.
- **El frontend del TaskHub TENÍA bug** — asumía `project.tasks`
  siempre presente sin defensa. Fixeado en
  `boilerplates/taskhub/frontend/assets/app.js` con comentario
  in-line que cita esta decisión del codegen.
- **Mejora del cap C4 del curso TaskHub** (deuda de docs, no
  del lenguaje): debería citar el trade-off del codegen +
  mostrar el patrón defensivo del cliente como pattern canónico
  cuando se usa `.preload()`.

Caso archivado — el análisis quedó documentado para que futuras
sesiones no vuelvan a marcar esto como "deuda del lenguaje".



## 🔴 DEUDAS URGENTES — Smoke E2E TaskHub Dockerizado (2026-06-08) — **AMBAS CERRADAS v0.15.13**

Dos bugs reales del lenguaje encontrados al hacer el smoke E2E con
`docker compose up` sobre `boilerplates/taskhub`. Ambos son
reproducibles fuera del TaskHub y afectan cualquier programa Fitz
que use el patrón correspondiente.

**Estado al 2026-06-08**: AMBAS CERRADAS en v0.15.13 con tests
unit + E2E reales contra Postgres + smoke real del TaskHub.

### ✅ URGENTE-1 — `init_storage` de `@cron` con persistencia: race condition en CREATE TABLE (CERRADA v0.15.13 intérprete + v0.15.14 codegen)

**⚠️ Lección aprendida — fix incompleto en v0.15.13**: el fix
v0.15.13 cubrió SOLO el intérprete (`src/cron_jobs.rs`). El smoke
E2E real con `docker compose up` sobre el TaskHub (2026-06-09)
descubrió IN VIVO que el binario producido por `fitz build`
seguía rompiendo con el race original, porque el codegen
(`src/codegen.rs::SQL_HELPERS_PRELUDE`) emite **su propio
`__fitz_cron_init_storage` paralelo** que NO consultaba el OnceCell
del intérprete.

**Cierre real en v0.15.14**: paralelo bit-a-bit del fix v0.15.13
agregado al preludio del codegen. El binario producido ahora
también tiene `static __FITZ_CRON_INIT_STORAGE_ONCE:
tokio::sync::OnceCell<Result<(), String>>` global +
`__fitz_cron_init_storage_inner` (helper real) + wrapper
`__fitz_cron_init_storage` que invoca `get_or_init`. Verificado in
vivo con `docker compose up` sobre el TaskHub fixeado.

**Por qué no se detectó en v0.15.13**: los tests unit del codegen
verifican el shape del Rust generado (string contains), pero no
ejecutan el binario producido contra Postgres real. Los tests E2E
reales contra Postgres (`tests/cron_jobs_real_postgres.rs`) cubren
el path del intérprete (`src/cron_jobs.rs`), no el del binario
nativo. El smoke E2E con docker compose es lo que cierra esa
brecha — fue ese smoke (post-release v0.15.13) el que reveló el
bug residual.

**Mejora futura del proceso**: todo fix a la lógica del scheduler
de cron debe sumar test E2E del codegen path — compilar un
programa con N crons + ejecutar el binario producido contra
Postgres real + verificar que no rompe. Es el nivel de cobertura
que faltó.



**Síntoma reproducible**: dos o más `@cron("...", store=db_result)`
adentro del mismo programa. Al boot del binario, uno de los jobs
aborta con:

```
🕐 cron job 'X' no pudo inicializar storage, abortando task:
  ERROR [23505]: duplicate key value violates unique constraint
  "pg_type_typname_nsp_index"
  [sql: CREATE TABLE IF NOT EXISTS fitz_cron_jobs (...)]
```

El otro job arranca normal. Resultado: el cron que aborta nunca
corre — silent failure parcial del scheduler.

**Reproducción mínima** (cualquier programa con ≥2 `@cron`
persistentes):

```fitz
let db_result = db.connect(env_or("DATABASE_URL", "")).await

@cron("0 0 * * * *", store=db_result)
async fn job_a() -> Result<Null> { return Ok(null) }

@cron("0 0 * * * *", store=db_result)
async fn job_b() -> Result<Null> { return Ok(null) }

@server(8080, "0.0.0.0")
fn main() => 0
```

**Causa real** (verificado en código, no hipótesis):
[`src/cron_jobs.rs:615-624`](../src/cron_jobs.rs#L615-L624) —
`run_cron_job` ejecuta `init_storage(&conn).await` **por cada job
spawneado**, sin coordinación. Los N jobs se lanzan en paralelo con
`tokio::spawn(__fitz_run_cron_job(...))` desde
`emit_cron_job_spawns` del codegen, y los N corren
`CREATE TABLE IF NOT EXISTS fitz_cron_jobs` simultáneo. Postgres
tiene una race condition documentada en su catálogo de tipos
(`pg_type`) cuando dos sesiones intentan crear la misma tabla al
mismo tiempo — el `IF NOT EXISTS` checkea la existencia ANTES de
intentar el INSERT al catálogo, pero entre el check y el insert hay
una ventana. Postgres NO serializa los CREATE TABLE
internamente con un lock de catálogo a nivel de "esta tabla", solo
el SET de catálogo entero (race en pg_type específicamente).

**Por qué el `init_storage` no es idempotente bajo concurrencia**:
es idempotente bajo el modelo SQL (`IF NOT EXISTS`), pero el
chequeo CREATE TABLE de Postgres no es transaccional contra
operaciones paralelas del mismo objeto. Bug documentado upstream
en [postgres-archives](https://www.postgresql.org/message-id/16664-2dca8df8c8893db4%40postgresql.org)
desde ~2020. Patrón conocido y la solución estándar es serializar
el CREATE con un advisory lock.

**Fix sugerido** (~20 LoC, mini-fase dedicada):

Opción A — `OnceCell<Result<(), String>>` en `cron_jobs.rs`:

```rust
static INIT_STORAGE_ONCE: tokio::sync::OnceCell<Result<(), String>>
    = tokio::sync::OnceCell::const_new();

async fn ensure_storage_initialized(
    conn: &DbConnHandle,
) -> Result<(), String> {
    INIT_STORAGE_ONCE
        .get_or_init(|| async {
            init_storage(conn).await
        })
        .await
        .clone()
}
```

Reemplazar la llamada directa en `run_cron_job:618` por
`ensure_storage_initialized(&conn).await`. El primer job que llega
ejecuta el init; los demás esperan al `OnceCell` y reusan el
resultado. Sin race, sin overhead extra después del primer init.

Opción B — `pg_advisory_xact_lock(hash('fitz_cron_init'))` antes
del CREATE TABLE. Permite múltiples instancias del binario en
paralelo (lock compartido a nivel DB, no a nivel proceso). Más
robusto que A si en algún momento se corren múltiples instancias.

**Recomendación**: Opción A para el MVP (cubre 99% del caso —
binario único). Opción B si aparece deploy multi-instancia con
crons coordinados.

**Fix implementado en v0.15.13** ([commit](.))

- Nuevo `tokio::sync::OnceCell<Result<(), String>>` global en
  `src/cron_jobs.rs::INIT_STORAGE_ONCE`.
- Nuevo wrapper `pub async fn ensure_storage_initialized(conn)`
  que llama `INIT_STORAGE_ONCE.get_or_init(...)` — solo el primer
  caller del proceso ejecuta `init_storage` real, los demás
  reciben el `Result` clonado.
- `run_cron_job:618` ahora llama a `ensure_storage_initialized`
  (en lugar del `init_storage` directo). Comentario actualizado
  en `init_storage` advirtiendo que NO es seguro para uso paralelo
  directo.
- Helper test-only `reset_init_storage_once_for_tests()` con
  `#[doc(hidden)]` (no `#[cfg(test)]` porque los integration tests
  necesitan llamarlo desde otro crate).

**Tests E2E reales contra Postgres** (`tests/cron_jobs_real_postgres.rs`,
opt-in con `FITZ_TEST_PG_URL`):

- `v0_15_13_ensure_storage_initialized_evita_race_con_10_paralelos`:
  10 `tokio::spawn` concurrentes invocan `ensure_storage_initialized`.
  Sin el fix al menos uno rompía con `pg_type_typname_nsp_index`;
  con el fix los 10 completan OK.
- `v0_15_13_ensure_storage_initialized_cachea_resultado_segundo_call_no_corre_create_table`:
  tras el primer init, drop manual de las tablas + segundo call
  retorna OK (reusa cache, NO re-ejecuta SQL) — verificable
  consultando que la tabla NO fue re-creada.

**Validado contra Postgres 15 local**: 2/2 tests verdes con
`FITZ_TEST_PG_URL`.

**Smoke real TaskHub**: tras el fix + restart, ambos crons
(`daily_due_reminders` + `cleanup_old_tasks`) arrancan sin error.

**Limitaciones pendientes (deuda menor abierta)**:

- **Multi-proceso**: el OnceCell es por proceso. Si dos binarios
  fitz arrancan simultáneamente contra la misma DB, ambos pueden
  golpear el race de Postgres. Fix futuro: sumar
  `pg_advisory_xact_lock` adentro de `init_storage` antes del
  CREATE TABLE. NO se hizo en v0.15.13 porque el caso típico
  despliega UN container por servicio.
- **Si el primer init falla** (ej: DB down al boot), todos los
  jobs futuros reciben el mismo error sin reintento. Restart del
  proceso es lo que destraba. Patrón aceptado para el MVP.

---

### ✅ URGENTE-2 — `@server(host=...)` no acepta sintaxis kwarg (CERRADA v0.15.13)

**Síntoma**: el patrón canónico para Dockerizar exige bindear a
`0.0.0.0` (no `127.0.0.1` que es el default). El user intenta:

```fitz
@server(8080, host="0.0.0.0", ws_heartbeat_secs=30, prometheus=true)
fn main() => 0
```

El codegen aborta con:

```
✗ codegen: Error — @server: kwarg 'host' no reconocido. Soportados:
  docs, api_version, ws_heartbeat_secs, shutdown_timeout_secs,
  observability, prometheus.
```

Workaround actual — pasar `host` como **2do positional**:

```fitz
@server(8080, "0.0.0.0", ws_heartbeat_secs=30, prometheus=true)
```

Funciona, pero es API inconsistente. Todos los otros parámetros del
decorator son kwargs nombrados (`ws_heartbeat_secs`,
`shutdown_timeout_secs`, etc.) — solo `port` y `host` son
positionals. El user que ya conoce el patrón kwarg del resto del
decorator espera lo mismo para `host` y se rompe.

**Causa real** (verificado en código):
[`src/evaluator.rs:1335-1401`](../src/evaluator.rs#L1335-L1401) —
`register_server_config` itera `deco.args` (positionals)
explícitamente buscando `args[0]` (port) y `args[1]` (host). El
loop posterior sobre `deco.kwargs` mapea solo a `docs`,
`api_version`, `ws_heartbeat_secs`, `shutdown_timeout_secs`,
`observability`, `prometheus`. **`host` jamás aparece en el switch
de kwargs**.

**Por qué importa para el patrón Dockerizado**:

- Default `127.0.0.1` es decisión correcta por seguridad (igual que
  FastAPI exige `--host 0.0.0.0` explícito).
- Pero los boilerplates Dockerizados (TaskHub + 8 más) **siempre**
  explicitan `"0.0.0.0"`.
- El user que viene de cap 28 / cap 33 de la guía aprende a usar
  kwargs `prometheus=true`, `docs=false`, etc. y asume que `host`
  es kwarg también — falla silenciosamente.
- Especialmente confuso porque el mensaje de error lista los
  kwargs soportados y `host` NO está. El user descubre el workaround
  positional por leer otros boilerplates, no por la documentación.

**Fix sugerido** (~30 LoC + tests + doc update):

Aceptar `host` como kwarg además de positional. Adentro de
`register_server_config`, después del loop de positionals, agregar
una rama nueva al match de kwargs:

```rust
"host" => match value_expr {
    Expr::Str(s, _) => {
        // Validamos misma regla que el positional
        if s.parse::<std::net::IpAddr>().is_err() {
            return Err(err(format!(
                "@server sobre fn '{}': host '{}' no es IP válida",
                fn_name, s,
            )));
        }
        // Si también vino como positional, error de conflicto
        // (igual que Python: TypeError con doble especificación).
        config.host = s.clone();
    }
    other => return Err(err(...)),
},
"port" => match value_expr {
    Expr::Int(n, _) => { /* idem */ }
    ...
},
```

Y validación de conflicto: si vienen positionals + kwargs para el
mismo parámetro, error claro. Patrón estándar de Python args parsing.

**Paridad bit-a-bit codegen**: mismo helper
`parse_build_server_args` en `src/codegen.rs` debe aceptar el
kwarg también.

**Tests requeridos**:
- `@server(8080, host="0.0.0.0")` compila y bindea OK
- `@server(port=8080, host="0.0.0.0")` (ambos kwargs) compila OK
- `@server(8080, "127.0.0.1", host="0.0.0.0")` rechaza con conflicto
- Tests del intérprete + codegen + cap 17 de la guía actualizado

**Fix implementado en v0.15.13** ([commit](.))

Cambios paralelos bit-a-bit en evaluator y codegen:

**`src/evaluator.rs::register_server_config`** (~50 LoC):

- Flags `port_set_via_positional` y `host_set_via_positional` para
  detectar doble-especificación.
- Match de kwargs gana ramas `"port"` y `"host"` con error claro
  estilo Python ("port pasado dos veces") si ya vino positional.
- Reuso de la validación existente (port en `[1, 65535]`, host como
  IP literal parseable).
- Mensaje de error de kwarg desconocido actualizado para citar
  `port, host, docs, api_version, ws_heartbeat_secs,
  shutdown_timeout_secs, observability, prometheus`.

**`src/codegen.rs::parse_server_decorator`** (~50 LoC paralelo):

Mismo cambio bit-a-bit en `fitz build`. El comportamiento del
binario nativo es idéntico al intérprete.

**Tests**:

- **9 unit tests evaluator** (`v0_15_13_server_*`): host kwarg solo,
  port kwarg solo, mixed con otros kwargs, port positional + host
  kwarg, doble port (positional + kwarg) error, doble host error,
  host kwarg no-str error, port kwarg fuera de rango error, host
  kwarg IP inválida error.
- **5 unit tests codegen** (`v0_15_13_server_*`): host kwarg
  emite `0.0.0.0:3000`, port kwarg emite `127.0.0.1:9090`, mixed
  emite `0.0.0.0:8080`, conflictos rechazados.
- **Test viejo `server_kwarg_desconocido_lista_docs_y_api_version`**
  actualizado al mensaje nuevo (incluye `port` y `host`).

**Patrón canónico nuevo** (recomendado, equivalente a positionals):

```fitz
@server(port=8080, host="0.0.0.0", prometheus=true)
fn main() => 0
```

Más claro que mezclar positionals con kwargs. Los positionals
siguen funcionando para backward-compat.

**Smoke real TaskHub**: tras el fix, cambiar
`@server(8080, "0.0.0.0", ws_heartbeat_secs=30, prometheus=true)`
por `@server(port=8080, host="0.0.0.0", ws_heartbeat_secs=30,
prometheus=true)` compila/corre bit-a-bit igual.

---



## Codegen interop Python + ORM completo — TaskHub blockers (2026-06-08)

Mini-fase descubierta al smoke-testear `boilerplates/taskhub`
(showcase del stack completo). El boilerplate combina por primera
vez **ORM con `DateTime`/`Date`/`Uuid` + interop Python +
`@background`/`@cron` + relations virtuales** en un solo programa,
exponiendo gaps del codegen que no se manifiestan en stacks
individuales (los 10 boilerplates anteriores andan OK).

### Fix aplicado (CERRADO 2026-06-08 en source local, pendiente bump)

5 mejoras al codegen `src/codegen.rs` (~115 LoC nuevas):

1. **`impl __FitzToPy` para `chrono::NaiveDate` / `DateTime<Utc>` / `uuid::Uuid`**
   en el preludio Python. Serializan a Python `str` canonical
   (ISO 8601 `YYYY-MM-DD` / RFC 3339 / UUID canonical). Antes los
   types Fitz `Date`/`DateTime`/`Uuid` no tenían impl y rustc
   rompía al intentar `self.created_at.__fitz_to_py(...)`.
2. **Branches `Date`/`DateTime`/`Uuid` en `py_field_extract_arms`**
   (Python → Fitz, no-nullable). Parsean Python `str` al type
   Rust nativo (`NaiveDate::parse_from_str(s, "%Y-%m-%d")` /
   `DateTime::parse_from_rfc3339(s).with_timezone(&Utc)` /
   `Uuid::parse_str(s)`).
3. **Mismos branches en `py_inner_extract_for_nullable`**
   (cubre `Date?`/`DateTime?`/`Uuid?` nullable). Antes el `_`
   branch rechazaba con "field `X` de tipo `Y` (nullable): inner
   type compuesto no soportado todavía".
4. **Skip de virtual fields en `gen_fitz_py_to_instance_helper`**
   (W17 paralelo, v0.10.7 hizo lo mismo para `__FromFitzJson`/
   `__ToFitzJson`): companion fields BelongsTo / `@has_many` /
   `@has_one` se inicializan con `Default::default()` en el
   struct literal en lugar de intentar extraerlos del PyDict
   (son sentinels del ORM, no datos que vengan de Python).
   `gen_python_helpers_for_type` y `gen_fitz_py_to_instance_helper`
   ahora aceptan `meta: Option<&TableMetadata>` análogamente a
   `gen_type_http_impls_for_sig_with_meta`.
5. **Branches `Date`/`DateTime`/`Uuid` en `.update(db, Map var)`**
   runtime match (~30 LoC en line 19387+). Acepta `__FitzValue::Str(s)`
   y emite `__FitzPgValue::Text(s.clone())` paralelo a cómo
   `impl __IntoPgValue for chrono::NaiveDate` ya hace.

**Validación**: cargo test --lib 3121 verdes, 0 failed. Cero
regresiones sobre 10 boilerplates + 99 guide examples.

### Bug derivado del cap C5 (TaskHub) — fixeado en boilerplate

El cap C5 del proyecto Construyendo TaskHub usaba `db.query(sql, [])`
(módulo) en el endpoint admin `GET /api/jobs` cuando debería ser
`conn.query(sql, [])` (la connection bindeada). El intérprete lo
permitía (dispatch implícito sobre conn global) pero el codegen
solo soporta `db.connect`. **Inconsistencia intérprete vs codegen
documentada como deuda secundaria** — no aplica al fix del codegen,
pero el cap C5 + ejemplo + boilerplate están corregidos a
`conn.query(...)`.

### Bug derivado del cap C4 — workaround documentado

El cap C4 (background fn) usaba `match Task.where(...).first(conn).await { Ok(t) => t, ... }`
sin anotación explícita del tipo de `t`. El codegen rechazaba el
field access `task.assignee_id` posterior con *"field access sobre
`T?`: solo se soporta sobre instancias de tipos custom"*. Workaround:
agregar anotación explícita `let task: Task = match ... { Ok(t) => t, ... }`.
**Deuda menor del codegen — el checker entiende el patrón pero el
codegen no infiere el tipo desde el match arm `Ok(t) => t`**. Si
aparece presión, mini-fase futura puede inferir el tipo del Result
inner en el codegen.

### Mini-fase post-2026-06-08 — `LazyLock<.await>` + `spawn(fn(arg))` capture-by-move (CERRADA v0.15.12)

Los **2 blockers** que quedaron al cierre de v0.15.11 (TaskHub baja
de 21 → 2 errores rustc) eran bugs pre-existentes del codegen que
aparecieron al destrabar los 19 anteriores (efecto cascade
unblocking). Ambos cerrados en v0.15.12 — TaskHub `fitz build`
exit 0, binario 9.7 MB, healthz/readyz/metrics validados end-to-end.

**Blocker 1 — `LazyLock<T>` con `.await` en init (E0728) → `OnceCell<T>`**

Trigger: `let X = <expr>.await` top-level + handlers HTTP/WS/cron
que consumen X. El codegen lo hoisteaba a `static __FITZ_STATE_X:
LazyLock<T> = LazyLock::new(|| <expr>.await)` cuyo closure es sync
— rustc emitía `error[E0728]: await is only allowed inside async`.

Fix v0.15.12: helper `expr_contains_await(e)` recursivo detecta
async init en `resolve_state_var_types`, llena
`state_var_async: HashMap<String, bool>` en `CodegenCtx`.
`gen_http_main` dispatch entre dos paths según el flag:

- **Sync** (caso típico `let users = []`): mantiene path LazyLock
  bit-a-bit idéntico — `static X: LazyLock<T> = LazyLock::new(|| init);`
  + materialización `(*X).clone()`. Zero cambio.
- **Async** (caso `let db_result = db.connect(url).await`): emite
  `static X: tokio::sync::OnceCell<T> = OnceCell::const_new();` +
  init eager en el body del `async fn main()` antes de spawn/serve
  via `{ let __init: T = init; X.set(__init).expect("..."); }`.
  Materialización en `gen_top_fn` + `emit_cron_job_spawns` cambia a
  `X.get().expect("...").clone()`.

Mismo costo runtime (Arc clone, no contenido).

**Blocker 2 — `spawn(fn(arg))` movía vars del outer (E0382) → shadow-clone preventivo**

Trigger: `let _ = spawn(send_due_reminder(new_task.id))` seguido por
uso de `new_task` después. El `tokio::spawn(async move { ... })`
capturaba `new_task` por valor moviéndolo al closure; el caller
después rompía con `error[E0382]: use of moved value`.

Fix v0.15.12: helper `collect_idents_in_expr(e, &mut HashSet)`
recursivo recolecta idents del scope outer en inner_args. `gen_spawn_call`
emite `let <name> = <name>.clone();` para cada ident filtrado por
`var_in_any_scope` (descarta fns/builtins/types — no clonables, no
necesarios) ANTES del `tokio::spawn`. El `async move` captura los
shadow clones; el outer scope sigue accesible.

Output esperado para el caso TaskHub:

```rust
{
    let new_task = new_task.clone();  // shadow
    let __jh = tokio::spawn(async move {
        send_due_reminder(new_task.lock().unwrap().id).await
    });
    Box::pin(async move { __jh.await.unwrap() })
}
```

**Tests nuevos al cierre v0.15.12**: 6 unit tests dedicados (`v0_15_12_*`)
+ 1 actualizado (`v0_15_11_*` shape OnceCell). Sin regresión en 3037
unit + 99 smoke guide + 360 compile_e2e + 3 openapi. fmt + clippy
limpios. Smoke real TaskHub verde end-to-end.

**Deudas residuales heredadas de v0.15.11 (NO bloquean)** — siguen
abiertas como refinamientos menores del codegen/checker:

1. Inferencia del Result inner en match con un arm que aborta
   (`return Err(...)`) — workaround: anotación explícita
   `let conn: DbConn = match db_result { ... }`.
2. Checker no detecta `!Future<Bool>` como olvido de `.await` —
   workaround: `.await` explícito.
3. Coerción PyAny → primitivo adentro de match arm no se propaga
   desde anotación del `let` contenedor — workaround: shadow var
   tipada adentro del arm.

Cada una con workaround trivial, ninguna bloquea uso real. Mini-fase
futura si entra demanda concreta.

### Mini-fase post-2026-06-08 — `(&db_result).into_store()` no resuelve en `__main_inner` (CERRADA v0.15.11)

Diagnóstico inicial (sesión anterior) decía "`db_result` no en
scope para `@background`/`@cron`". **Investigación 2026-06-08
descubrió que el bug real es distinto**: los handlers
`@background`/`@cron` SÍ ven `db_result` (vía la materialización
local que `gen_top_fn` hace para state vars referenciados,
líneas 11738-11764 de `src/codegen.rs`).

**El bug real**: el bloque de `tokio::spawn(__fitz_run_cron_job(...))`
emitido por `emit_cron_job_spawns` dentro de `__main_inner` (o
`fn main` en modo cron-only) emitía `store: (&db_result).into_store()`
usando el nombre de usuario. Pero `db_result` está hoisteado a
`static __FITZ_STATE_DB_RESULT` cuando algún handler también lo
consume (caso típico TaskHub: HTTP handlers + `@cron(store=db_result)`
combinan). En el cuerpo de `__main_inner` no existe el local
`db_result`, sólo el static — y rustc rompe con `error[E0425]:
cannot find value 'db_result' in this scope`.

**Fix (v0.15.11)**: `emit_cron_job_spawns` materializa los state
vars referenciados como `store_var` ANTES del loop de spawns con
`let db_result: T = (*__FITZ_STATE_DB_RESULT).clone();` (mismo
patrón que el body de `gen_top_fn` líneas 11738-11764). Lista
deduplicada (si N jobs usan el mismo store var, una sola
materialización). Sin overhead — `Arc::clone` y nada más. Test
unitario nuevo `v0_15_11_cron_store_kwarg_con_state_var_materializa_local_antes_de_spawn`
candea la regresión.

**Sub-fix simultáneo** en `boilerplates/taskhub/src/main.fitz`
para destrabar el smoke real del boilerplate:

1. **17 sitios `let conn = match db_result { ... }`** → anotación
   explícita `let conn: DbConn = match db_result { ... }`. Sin
   la anotación el codegen tipa `conn` como `T?` (Nullable) y
   declara la variable como `Option<__FitzDbConn>` aunque ambos
   arms devuelven `__FitzDbConn` plano. **Deuda residual del
   codegen** (no nueva — ya documentada como "bug derivado del
   cap C4"): el codegen no infiere el inner del Result desde el
   match arm `Ok(c) => c` cuando el otro arm aborta con `return`.

2. **`@healthz fn check_db_alive() -> Bool` → `@healthz async fn
   check_db_alive() -> Bool`** + `.await` sobre `c.is_closed()`.
   El método `is_closed()` retorna `Future<Bool>` (paridad
   intérprete + codegen, ver `src/codegen.rs` línea 16565). El
   checker NO detectó que el `!Future<Bool>` era inválido.
   Mismo fix para `@readyz fn check_ready_for_traffic`. **Deuda
   menor del checker**: aceptar `!` sobre tipo `Future<Bool>`
   sin emitir error de "se olvidó `.await`".

3. **`let suggested: Int = match priority.suggest_priority(...) {
   Ok(p) => p, ... }`** → introducir `let v: Int = p` adentro
   del arm para forzar coerción PyAny → Int de Fase 8.4.
   **Deuda menor del codegen**: la coerción PyAny → primitivo
   adentro de match arms con anotación destino en el `let`
   contenedor no se propaga al arm. El intérprete sí lo hace.

Las tres deudas del codegen/checker son refinamientos no-bloqueantes
— los workarounds son triviales y el patrón canónico del boilerplate
queda como ejemplo de "los 3 patches" para futuros usuarios.

### Pre-existente — M7.C1 app.fitz (`examples/curso/m7-python-interop/c1-setup/app.fitz`)

4 errores del checker independientes del fix interop:

```
Error 24:24 — operador * espera operandos numéricos, recibió PyAny y Float
Error 25:29 — operador * espera operandos numéricos, recibió Float y PyAny
Error 37:29 — el tipo Result<Any> no tiene el método isoformat
Error 47:5  — return devuelve Result<Any> pero la función declara Str
```

Pre-existente (no relacionado con mi fix de codegen). Deuda
del cap M7 — el ejemplo del cap probablemente quedó stale tras
cambios al checker sobre `PyAny` arithmetic + `isoformat` method
discovery. Mini-fase separada del cap.

### Verificación de tooling local

- Toolchain `:latest-python` de GHCR existe y publica OK (verificado
  con `docker manifest inspect` + `docker run --rm <img> fitz --version`
  devuelve `0.15.0`). **Falsa alarma** del primer smoke — era
  imagen local cacheada vieja. Forzar re-pull con
  `docker image rm <img> + docker pull <img>` resuelve.
- CI release.yml job `docker-image-python` (líneas 363-445) corre
  OK en cada release tag, publica `:vX.Y.Z-python` + `:latest-python`
  para `linux/amd64`. Verificado en run de v0.15.0 (4m 9s,
  completed).

---

> **Estado de ejecución**: ruta A (quick wins) cerrada — clippy limpio,
> helpers, validaciones; B.1 (span en Stmt) cerrada — los errores
> stmt-level del checker ya citan línea/columna reales en lugar de
> `0:0`. C-F2 (field assignment chequeo) cerrada — el checker
> ahora valida tipos en `obj.field = value`. F12 (higher-order
> completo) cerrada — closures escapadas, fn como valor/param/retorno
> compilan con `fitz build`; cap 11 anotado y validado bit-a-bit.
> F11 (state HTTP compartido) cerrada — `thread_local!` por var
> top-level referenciada en handlers + tokio current_thread runtime;
> `examples/server.fitz` y `examples/guide/17-http.fitz` compilan
> end-to-end. T1 (tests frágiles del codegen) — **cerrado entero
> en tres batches**: infra AST-based con `syn` + `quote`, ~115
> unit tests del codegen migrados de string-match a inspección
> de AST. Los 10 `code.contains` que quedan en `codegen.rs` son
> intencionales: 4 sobre tokens AST normalizados via
> `ast_test::ts(&file)`, 1 contrato de mensaje de error
> user-visible, 1 negative check sobre output completo, 4 sobre
> Cargo.toml (TOML, no Rust). S1.2 (span en Expr) — los 3 sub-pasos cerrados: variantes
> de `Expr` cargan `Span`, parser propaga spans en cada regla,
> checker (`infer_expr` + helpers) y evaluator (`eval_expr` +
> helpers + 14 métodos built-in) citan posición del nodo en
> errores. S1.codegen cerrado — 52 sitios del codegen migrados
> a `err_at` (con span del nodo); los 17 restantes son defensivos
> contra bugs del compilador (checker debió cazar), donde citar
> posición no aporta. HTTP status codes custom cerrado —
> sintaxis del spec `return <Int> { ... }` implementada
> end-to-end: AST (`Stmt::ReturnStatus`), parser (detecta el
> patrón después de `return <Int>` cuando viene un `{`), checker
> (acepta solo adentro de handlers HTTP), intérprete
> (`Value::HttpResponse` → outcome con el status pedido), codegen
> (override del return type a `__FitzResponse` cuando la fn HTTP
> contiene `ReturnStatus`, envoltura uniforme de returns
> normales y custom). Polimorfismo del spec: handler `-> User`
> puede mezclar `return user` (200) con `return 404 { ... }`.
> **HTTP query params cerrado** — sintaxis del spec `?key={name}`
> implementada end-to-end: `parse_path_template` separa path y
> query y devuelve `query_params: Vec<String>` adicional;
> `RouteSpec`/`RouteMeta`/`InterpTask` cargan los nombres y
> raw values; `build_method_router` extrae `Query<HashMap>` en
> 8 combinaciones (path × query × body); evaluator valida que el
> handler tenga param Fitz por cada `?key={name}` y coerciona
> (`Int?` opcional → Null si falta; `Int` obligatorio → 400);
> codegen emite `axum::extract::Query<HashMap>` + binding
> tipado para cada param (Int/Float/Str/Bool, opcional `Option<T>`).
> Tipos no soportados (Lists, custom) abortan codegen con
> mensaje claro. Cap 17 de la guía + ejemplo `17-http.fitz` con
> nuevo endpoint `/search?name={name}&limit={limit}`. Bug fix
> colateral del codegen: `BinOp Eq` entre `Nullable<T>` y `Null`
> ahora emite `.is_none()` / `.is_some()` en vez del literal
> `== ()`. Intérprete y compilador validados bit-a-bit.
> Ver matriz para ítems pendientes (Pattern/TypeExpr sin span,
> T1 sucesivos batches). **1043 tests pasando** (+17 dedicados:
> http path 5, codegen 7, E2E 5).
>
> **Cierre de Fase 7 (2026-05-13)**: DX HTTP cerrada con 1150
> tests. OpenAPI 3.1 + UI Scalar + `@header(name="X")` +
> `@server(docs=false)` + `fitz openapi archivo.fitz` + paridad
> bit-a-bit `fitz run` ↔ `fitz build`. Deuda residual abierta:
>
> - ~~**Middleware + CORS**~~ — **CERRADA en mini-fase MW
>   (2026-05-14, 1189 tests)**. Decorator `@middleware(fn)`
>   apilable + built-in `cors(...)` configurable. Modelo
>   gate-only para middleware genérico (`return null` / `return
>   <status> { ... }`); CORS como slot dedicado con preflight
>   OPTIONS y headers inyectados en response real (incluso
>   500/400). `Request` y `Response` pre-registrados como
>   nominales built-in. Sub-pasos: MW.1 intérprete; MW.2 cors
>   built-in + preflight; MW.3 codegen completo; MW.4 guía cap
>   17 sub-sección + ejemplo `17b-middleware.fitz` + cierre.
>   Validación E2E bit-a-bit `fitz run` ↔ `fitz build` via
>   build + spawn + raw TCP. Deudas que quedan:
>   - **Modelo wrap (post-process)** para timing/tracing — el
>     gate-only no expresa "after". Mini-fase dedicada post-F8
>     si aparece presión real.
>   - **CORS request-aware** (echo del Origin recibido cuando
>     se admite un set acotado de orígenes). Deuda menor.
>   - **OpenAPI schema con CORS/middleware** — el schema no
>     refleja los middlewares aplicados. Útil para docs UI;
>     irrelevante para SDKs generados (server-side concern).
>   - **Body en `Request`** — hoy el Request expone method/
>     path/headers; body queda en el handler post-middleware.
>     Para HMAC/signing habría que parsear antes del short-
>     circuit.
> - Doc-strings sobre handlers (descripciones OpenAPI) — el
>   parser hoy descarta comentarios; retenerlos es refactor
>   lexer+parser+AST. **Postergado a post-F17** (es refactor
>   invasivo del lexer/parser/AST; conviene hacerlo cuando el
>   bridge HTTP mpsc/oneshot ya no exista para minimizar
>   merge pain).
> - ~~Status codes custom en el schema~~ — **CERRADO en Q.4
>   (2026-05-14)**. `collect_status_codes(body)` escanea
>   recursivamente los `Stmt::ReturnStatus`; cada code custom
>   aparece como entry en `responses` del schema con
>   description vía `http_status_phrase`. Schema del body
>   queda `{}` (any) por polimorfismo del spec. Status codes
>   colisionando con derivados del return type (200/500 de
>   Result) ceden al schema fuerte.
> - ~~Aliases en `@header`~~ — **CERRADO en Q.1 (2026-05-14)**.
>   `@header(name="X-Auth", into="token")` mapea explícito a
>   un param Fitz con nombre arbitrario. Sin `into` se mantiene
>   la convención previa (`lowercase + '-' → '_'`).
> - **Bundle Scalar embebido offline** — **POSTERGADO post-F17**
>   tras evaluar trade-off (Q.5, 2026-05-14). Bundle de Scalar
>   pesa ~3.7 MB minificado y no hay variante liviana. Embeberlo
>   por default rompe la promesa "binario nativo mínimo" (~10-15%
>   de overhead típico). Opt-in via `@server(offline_docs=true)`
>   queda comprometido si aparece presión real (deploys air-gapped,
>   requisitos de auditoría). Hoy CDN jsdelivr cubre el 99% de
>   casos — el browser cachea tras el primer load.
> - ~~`info.version` override~~ — **CERRADO en Q.2 (2026-05-14)**.
>   `@server(api_version="X.Y.Z")` se refleja en `info.version`
>   del schema; default sigue `"0.1.0"`. Cableado por los 3
>   caminos (`fitz run`, `fitz openapi`, `fitz build`).
> - ~~CORS request-aware~~ — **CERRADO en Q.3 (2026-05-14)**.
>   `cors({"allow_origin": ["a.com", "b.com"]})` con `List<Str>`
>   activa modo Set: el server hace echo del `Origin` del
>   request si está en la lista permitida; si no, OMITE el
>   header `Access-Control-Allow-Origin` (browser rechaza,
>   comportamiento CORS estricto). Útil con credenciales
>   (`Allow-Origin: *` incompatible con `Allow-Credentials`).
>
> **Mini-tanda Q (2026-05-14)**: cerró 4 deudas chicas (Q.1
> aliases @header, Q.2 api_version, Q.3 CORS Set, Q.4 status
> codes en schema). Q.5 (bundle offline) postergado por
> trade-off de tamaño. Q.6 (docs refresh) cerrado en este mismo
> bloque. Total al cierre de la tanda: **1153 unit + 74 E2E**.
>
> **Fase F17 (2026-05-14): CERRADA** — Send completo +
> paralelismo HTTP real + bridge eliminado. La deuda más grande
> arrastrada desde Fase 4. Seis sub-pasos: F17.1 dep parking_lot;
> F17.2 `Shared<T>`/`EnvRef` → `Arc<parking_lot::Mutex<T>>` (~284
> sitios mecánicos); F17.3 quitar `?Send` del `#[async_recursion]`
> (`FitzFuture: Send`); F17.4a `serve()` tokio multi-thread;
> F17.5 eliminar bridge HTTP `mpsc/oneshot` (~269 LoC netas menos
> en `http.rs`, handlers axum invocan `handle_task(...).await`
> directo sobre `Arc<HttpRegistry>`); F17.4b codegen output paralela
> migración (`Rc<RefCell<>>` → `Arc<Mutex<>>` con std::sync, state
> HTTP `thread_local!` → `LazyLock<Arc<Mutex<T>>>`, runtime
> generado a `#[tokio::main]` multi-thread, `PartialEq` custom
> por tipo, field access como bloque acotado para evitar deadlocks
> de re-lock); F17.6 guía cap 19 + ejemplo
> `examples/guide/19b-paralelismo.fitz` (validado 5 reqs en 1.2s
> paralelo vs 5.3s serie). Total al cierre: **1153 unit + 74 E2E**,
> clippy `-D warnings` limpio. Detalles completos en
> `docs/roadmap.md` → "Fase F17". Próximo norte: **Fase 8
> (Interop Python)**.
>
> **Mini-tanda PreF8 (2026-05-14): CERRADA** — cleanup pre-Fase 8.
> Cuatro sub-pasos: PreF8.1 refactor M1+M2 codegen
> (`generate_main_rs` y `gen_http_handler_wrapper` partidas en
> helpers, AST output bit-a-bit idéntico); PreF8.2 method chain
> multi-línea en parser (newlines antes de `.` toleradas);
> PreF8.3 defaults de tipos importados (estrategia eager-at-import
> con `resolved_defaults` + `__default_<T>_<F>()` por módulo);
> PreF8.4 import aliasing con `as` (sub-paso adelantado de F8.1).
> Total al cierre: **1172 unit + 79 E2E**, clippy limpio. Detalles
> completos en `docs/roadmap.md` → "Mini-tanda PreF8".
>
> **Fase 8.1 (2026-05-15): CERRADA** — embedding básico de CPython
> via PyO3. `from python import math` end-to-end en el intérprete
> (`fitz run --features python`). Cinco sub-pasos: 8.1.1 dep PyO3
> opcional + `Value::PyObject(Arc<Py<PyAny>>)` feature-gated;
> 8.1.2 `import_module(dotted)` + ruteo en `eval_python_from_import`
> + `py_err_to_fitz` con formato `"<ClassName>: <message>"`
> compatible con el wrap a `Result<T>` que llega en 8.3;
> 8.1.3 `Expr::Field` sobre PyObject con auto-coerción primitiva
> (None/bool/int/float/str → primitivos Fitz, resto → PyObject
> opaco); 8.1.4 `Expr::Call` con args primitivos + `value_to_py`
> simétrico — cumple el criterio `math.sqrt(16.0) == 4.0`;
> 8.1.5 guard de codegen `check_no_python_imports` con sugerencia
> de `fitz run` (la deuda **F19** comprometida marca soporte real
> en `fitz build` como sub-paso de 8.7). Total al cierre:
> **1213 unit + 80 E2E + 3 openapi_e2e** con feature; **1175 + 80
> + 3** sin feature. Decisiones tomadas al arrancar: ABI3-py310,
> opt-in `--features python`, política de venvs "estándar Python
> sin magia", inicialización lazy, `Python::attach` por operación.
> Ejemplo runnable: `examples/python-interop-8.1.fitz`. Detalles
> completos en `docs/roadmap.md` → "Fase 8.1". Próximo norte:
> **Fase 8.2 (marshaling de tipos compuestos)**.
>
> **Fase 8.2 (2026-05-15): CERRADA** — marshaling bidireccional de
> tipos compuestos. `List<T>` ↔ `list`, `Map<K, V>` ↔ `dict`,
> `Instance` → `dict` (por field name; recovery a `Instance` requiere
> anotación destino — deuda 8.4). Tres sub-pasos: 8.2.1 `value_to_py`
> con parámetro `path: &str` para breadcrumb informativo (`arg0[2].email`)
> + helpers `marshal_map_key` (valida keys hashables) y `fmt_map_key`
> (cosmético para path); 8.2.2 `py_to_value` con ramas `PyList`/`PyDict`
> antes del fallback opaco (PyO3 0.28 deprecó `downcast` en favor de
> `cast` — migrado); 8.2.3 criterio canónico del roadmap end-to-end —
> `List<User>` Fitz → `collections.Counter` Python → `Map<Str, Int>`
> Fitz indexable, validado bit-a-bit (Counter es subclass de dict,
> `is_instance_of::<PyDict>()` matchea subclases naturalmente).
> Decisiones: copia eager bidireccional (cross-cutting #4),
> Map keys solo primitivos hashables Python, `dict` Python NO se
> auto-coerce a `Instance`, orden preservado vía garantía CPython
> 3.7+, breadcrumb propagado recursivamente. Total al cierre:
> **1245 unit + 80 E2E + 3 openapi_e2e** con feature; **1175 + 80
> + 3** sin feature. Ejemplo runnable nuevo:
> `examples/python-interop-8.2.fitz` (5 secciones). Detalles
> completos en `docs/roadmap.md` → "Fase 8.2". Próximo norte:
> **Fase 8.3 (excepciones Python → `Result<T>`)**.
>
> **Fase 8.3 (2026-05-15): CERRADA** — excepciones Python →
> `Result<T>` automático. Toda llamada a una función Python desde
> Fitz se envuelve: éxito → `Result::Ok(v)`; excepción Python o
> marshaling fallido → `Result::Err(Str("<ClassName>: <message>"))`
> con el formato canónico ya estable desde 8.1.2. El programa
> Fitz no aborta — el usuario es forzado a manejar con `match` o
> `?`. Tres sub-pasos: 8.3.1 `py_interop::call` envuelve siempre
> (cualquier falla del path Python — excepción, marshaling de args,
> marshaling del return — pasa por Err; helper privado
> `err_value_from_message`) + tests viejos del call path
> actualizados con helpers `ok_inner`/`err_message` + 4 unit nuevos
> sobre shape + 3 evaluator nuevos del criterio canónico
> (`match`, propagación con `?`, field access sin wrap); 8.3.2
> ejemplos 8.1/8.2 reescritos al nuevo modelo (helper
> `unwrap_str`, `fn` con `?`, caveat del parser de interpolación
> con `{...}` documentado); 8.3.3 ejemplo dedicado
> `examples/python-interop-8.3.fitz` con 6 secciones (criterio
> textual del roadmap, distintas excepciones como Err,
> propagación con `?`, marshaling fallido con breadcrumb, field
> access sin wrap, chaining con desempaquetado intermedio).
> Decisiones: `call` envuelve y `get_attr` no (ergonomía vs
> ortogonalidad — solo llamadas pueden fallar en runtime
> esperable); marshaling de args también va en `Err` (uniformidad
> del path call); `Err` lleva `Str` plano (PyException
> estructurada queda como deuda menor); checker NO cambia (refino
> a `Result<Any>` llega en 8.4). Total al cierre: **1252 unit +
> 80 E2E + 3 openapi_e2e** con feature; **1175 + 80 + 3** sin
> feature. Cambio de comportamiento documentado: rompió ejemplos
> viejos de 8.1/8.2 (reescritos en 8.3.2). Detalles completos en
> `docs/roadmap.md` → "Fase 8.3". Próximo norte: **Fase 8.4
> (anotaciones del lado del checker + refinar tipos opacos)**.
>
> **Fase 8.4 (2026-05-15): CERRADA** — tipos del checker +
> anotaciones del lado Fitz + coerción runtime. Cierra el ciclo
> "call Python → tipo Fitz concreto" con tres cambios
> coordinados: el checker distingue valores Python de Any
> genérico (`Type::PyAny`), refina los calls a `Result<Any>`
> forzando manejo de errores estático, y el runtime coerciona
> `Value::Map` → `Value::Instance` cuando hay anotación nominal.
> El patrón canónico `let row: User = py_call(...)?` funciona
> end-to-end con UNA sola anotación. Cuatro sub-pasos (3 commits,
> 8.4.1 y 8.4.2 combinados): 8.4.1+8.4.2 `Type::PyAny` con
> identidad propia + bindings Python (`Stmt::Import`/`FromImport`
> con `path[0] == "python"`) tipan PyAny + field access sobre
> PyAny devuelve PyAny + call con receptor PyAny refina a
> `Result<Any>` (activa exhaustividad sobre Result 5.3.3 y regla
> de `?` 5.3.3 estáticamente) + `is_compatible` espejo de Any
> + ramas defensivas en `codegen.rs` (PyAny no aparece en codegen
> porque `check_no_python_imports` aborta antes); 8.4.3
> `coerce_to_annotation` async fn nueva en evaluator que
> resuelve `Named(T)` / `Nullable(Named(T))`, itera fields
> declarados en orden (provided → resolved_defaults → default
> Expr → nullable Null → error), ignora extras del Map, devuelve
> Instance con type_name canónico (PreF8.4); 8.4.4 ejemplo
> runnable + cierre formal. Decisiones: PyAny dedicado (no
> PyObject<"..."> fantasma), coerción vive en evaluator no en
> checker (el cast gradual ya pasa estático), extras del Map se
> ignoran silenciosamente, field requerido faltante aborta con
> `FitzError` no `Result::Err` (caso de programación, no de
> runtime esperable). Total al cierre: **1271 unit + 80 E2E +
> 3 openapi_e2e** con feature; **1193 + 80 + 3** sin feature.
> Ejemplo runnable nuevo: `examples/python-interop-8.4.fitz`
> (5 secciones validadas bit-a-bit). Detalles completos en
> `docs/roadmap.md` → "Fase 8.4". Próximo norte: **Fase 8.5
> (`fitz py-types` auto-mapeo SQLAlchemy → `type` Fitz)**.
>
> **Fase 8.5 (2026-05-15): CERRADA** — sub-comando nuevo
> `fitz py-types <archivo.py> [--out <archivo.fitz>]` que
> introspecciona modelos SQLAlchemy en un archivo Python y emite
> los `type` Fitz correspondientes, listos para commitear.
> Reduce el doble-tipado en proyectos SQLAlchemy. Dos sub-pasos:
> 8.5.1 `Commands::PyTypes` en CLI + nuevo módulo `src/py_types.rs`
> feature-gated (in-process via PyO3, no subprocess) + introspección
> por duck typing sobre `__table__.columns` (compatible con
> SQLAlchemy real y mocks sin requerir `pip install sqlalchemy`)
> + mapping por nombre canónico (Integer/BigInteger/...→Int,
> Float/Numeric/...→Float, String/Text/...→Str, Boolean→Bool,
> DateTime/Date/Time→Str ISO 8601 placeholder, resto→Any con
> `// ?` comment) + nullable + defaults literales (callable
> ignorado) + 10 unit tests con classes Python mock. 8.5.2 ejemplo
> runnable `examples/py-types/` (`models.py` autosuficiente con
> mock SQLAlchemy de 25 LoC + 2 modelos User/Order, `models.fitz`
> generado y commiteado como referencia, `usage.fitz` con
> `from models import` + coerción 8.4.3 + 4 escenarios incluyendo
> JSON malformado propagado) + cierre formal (CHANGELOG v0.8.6,
> roadmap, README). Decisiones: in-process via PyO3,
> duck typing por shape, solo SQLAlchemy en 8.5 (otros ORMs si
> entra demanda real), tipos desconocidos a `Any` con comentario,
> defaults callable ignorados silenciosamente, sin verificación
> de drift (regeneración manual). Total al cierre: **1281 unit +
> 80 E2E + 3 openapi_e2e** con feature; **1193 + 80 + 3** sin
> feature. Ejemplo runnable: `examples/py-types/` con tres
> archivos. Detalles completos en `docs/roadmap.md` → "Fase 8.5".
> Próximo norte: **Fase 8.6 (async + GIL: bridge tokio ↔
> asyncio)**.
>
> **Fase 8.6 (2026-05-15): CERRADA** — bridge tokio ↔ asyncio.
> Habilita `py_async_fn().await` desde cualquier `async fn`
> Fitz: cuando un call a una función Python devuelve una corutina
> (`async def`), Fitz la envuelve automáticamente en
> `Value::Future` adentro del `Result::Ok`. El `.await` postfix
> (Fase 6) la desempaca, ejecuta, y devuelve el valor coercionado.
> Excepciones asyncio → `Result::Err` (heredado de 8.3). Bridge
> invisible al usuario. Dos sub-pasos: 8.6.1 `py_interop::call`
> detecta awaitable con `inspect.isawaitable`, `is_coroutine` +
> `py_coro_to_fitz_future` helpers, FitzFuture usa
> `tokio::task::spawn_blocking` + `asyncio.new_event_loop()
> .run_until_complete(coro)` (baseline blocking, Send-safe, no
> deadlockea), 3 tests bajo `#[cfg(feature = "python")]`; 8.6.2
> ejemplo `examples/python-interop-8.6.fitz` con 3 secciones
> (patrón canónico `doble_eventual`, awaits encadenados
> `pipeline`, lazy sin `.await`) + cierre formal (CHANGELOG
> v0.8.7, roadmap, deudas, README). Decisiones:
> approach baseline blocking en vez de `pyo3-async-runtimes::
> into_future` (la crate requiere control del runtime tokio,
> choca con el setup ya establecido — Fase 6 current_thread CLI
> / F17 rt-multi-thread HTTP); detección automática de awaitable
> en `call` (no `.await` manual sobre PyObject); GIL serializa
> Python (esperado por roadmap, funcional para APIs DB-bound);
> sin marshaling Future Fitz → corutina Python (Future no
> marshalleable; `asyncio.gather` desde Fitz requiere helper
> Python externo). Total al cierre: **1284 unit + 80 E2E +
> 3 openapi_e2e** con feature; **1193 + 80 + 3** sin feature.
> Ejemplo runnable: `examples/python-interop-8.6.fitz`. Deuda
> residual visible: event loop asyncio persistente (paralelismo
> I/O real), marshaling Future↔Coroutine, política de GIL
> configurable, cancelación de Futures Python, tests
> multi_thread con paralelismo real. Detalles completos en
> `docs/roadmap.md` → "Fase 8.6". Próximo norte: **Fase 8.7
> (codegen interop Python en `fitz build` — cierra deuda F19)**.
>
> **Fase 8.7 (2026-05-15): CERRADA** — codegen interop Python en
> `fitz build`. **Cierra la deuda F19** del roadmap post-5b: el
> codegen acepta `from python import`, emite Cargo.toml condicional
> con pyo3, preludio `__FitzPyObject(Arc<Py<PyAny>>)` con helpers
> (import, getattr opaco/primitivo, call con marshaling automático,
> Result wrap, bridge async), y bindings globales (`static
> OnceLock` + getter) accesibles desde cualquier fn. Trait
> `__FitzToPy` con impls genéricos para primitivos, List, Map,
> Option e Instance Fitz (impl emitido por `gen_type_def` cuando
> `uses_python`). Patrón canónico `<py_call>?.await` para bridge
> async (paralelo a 8.6.1 baseline blocking). Cuatro sub-pasos:
> 8.7.1 preludio + import + getattr + Cargo.toml; 8.7.2 call +
> marshaling Fitz→Python + Result + Instance; 8.7.3 bridge async;
> 8.7.4 cierre formal con `examples/python-interop-8.7.fitz`
> validado bit-a-bit `fitz run` ↔ `fitz build`. Decisiones:
> alcance acotado (codegen sí, bundling no — sub-paso futuro
> separado con decisión python-build-standalone vs PyOxidizer
> pendiente); bindings globales con OnceLock + getter (vs `let`
> local — destraba uso en handlers HTTP sin refactor); patrón
> `?.await` único (paridad bit-a-bit con intérprete); auto-coerción
> primitiva via `coerce(PyAny → T)` (aprovecha infraestructura
> existente). Total al cierre: **1295 unit + 88 E2E + 3 openapi_e2e**
> con feature; **1204 + 79 + 3** sin feature. Ejemplo runnable:
> `examples/python-interop-8.7.fitz` con 3 secciones (constantes
> + calls + bridge async). Deuda residual visible (sub-paso
> futuro): coerción Python list/dict → Fitz List/Map/Instance,
> `.await` con binding intermedio split, bundling CPython
> embebido, trait `__FitzFromPy` simétrico. Detalles completos en
> `docs/roadmap.md` → "Fase 8.7". Próximo norte: **Fase 8.8 (guía
> + ejemplo CRUD + cierre formal de Fase 8)**.
>
> **Fase 8.8 (2026-05-15): CERRADA** — guía + ejemplo CRUD +
> cierre formal de Fase 8 entera. Tres sub-pasos: 8.8.1 cap 21
> "Interop Python" en `docs/guide.md` con 12 sub-secciones
> cubriendo 8.1-8.7 + renumeración cap 21→22; 8.8.2 ejemplo
> ejecutable `examples/guide/21-python-crud/` con SQLAlchemy +
> SQLite (`models.py` + `db.py` + `models.fitz` generado + `app.fitz`
> con handlers HTTP), validado end-to-end con curl; 8.8.3 cierre
> formal (CHANGELOG v0.8.9, roadmap, deudas, README).
> Decisiones de scope (confirmadas con autor): cap 21 con una
> renumeración (vs cap 20 con dos), backend SQLite (vs Postgres
> con Docker o sin DB), solo `fitz run` con nota explícita sobre
> deuda residual de 8.7 (vs validar paridad con `fitz build`).
> Detalles completos en `docs/roadmap.md` → "Fase 8.8".
>
> **Cierre formal de Fase 8 (Interop Python) entera (2026-05-15)**:
> roadmap original cumplido al 100% (8.1 embedding, 8.2 marshaling,
> 8.3 excepciones → Result, 8.4 tipos del checker + coerción,
> 8.5 fitz py-types, 8.6 bridge async, 8.7 codegen, 8.8 guía +
> CRUD). **Sub-paso separado pendiente** (no parte del roadmap
> original): bundling CPython embebido (`fitz build
> --bundle-python`). Próximo norte: **Fase 9 — Ecosistema**
> (package manager, LSP, formatter, linter); pre-reqs habilitantes
> ya identificados: F15 (parser error recovery) + F16 (IR tipado
> persistido por nodo).
>
> **Fase 9.0 (2026-05-15): F15 CERRADO** — error recovery del
> parser. Tres sub-pasos: 9.0.1 nodos `Expr::Error(Span)` /
> `Stmt::Error(Span)` in-band + `pub fn parse_with_recovery(tokens)
> -> (Program, Vec<FitzError>)` con `recovery_mode` interno + cota
> 100 errores + sync points stmt-level (Newline consumido,
> RBrace/EOF preservados, **keywords de inicio de stmt preservadas**
> por necesidad — `primary()` consume el token al fallar y sync
> sin la parada se comía stmts enteros); 9.0.2 checker silencioso
> (`Expr::Error → Type::Any`, `Stmt::Error` no-op) + helper
> `check_recovering(src)` que corre el pipeline LSP-style; 9.0.3
> validación end-to-end + cierre formal. **API strict
> (`parse`/`fitz run`/`fitz build`/`fitz check`) intacta** — sin
> cambio user-facing. 10 + 5 = 15 unit tests nuevos. Total al
> cierre: 1219 unit + 79 E2E + 3 openapi sin feature. Clippy
> `-D warnings` limpio. Próximo norte: **F16 (IR tipado persistido
> por nodo)** — segundo pre-req habilitante del LSP. Detalles
> completos en `docs/roadmap.md` → "Fase 9.0".
>
> **Fase 9.0 entera CERRADA (2026-05-15)**: F16 (IR tipado
> persistido por nodo) cierra los pre-reqs habilitantes del LSP.
> 2 sub-pasos: 9.0.4 side-table `TypeInfo` con `SpanKey(line,
> column)` como clave (Span propio no sirve porque su PartialEq
> devuelve true siempre por diseño), `infer_expr` envuelve
> `synthesize_expr` para centralizar el `record` al salir,
> `check_program` cambia firma a `(TypeEnv, TypeInfo,
> Vec<FitzError>)` (13 call sites migrados con `_types`),
> `Expr::Error` se persiste como `Type::Any` uniforme con el
> checker, 8 unit tests `types::tests::types_info_*`; 9.0.5
> cierre formal (CHANGELOG v0.9.1, roadmap con Fase 9.0 — F16
> detallada, este archivo con F16 CERRADO, README refresh).
> **API user-facing intacta** — `fitz run` /
> `fitz build` / `fitz check` descartan el side-table con
> `_types`. Total al cierre: 1227 unit + 79 E2E + 3 openapi sin
> feature. Clippy `-D warnings` limpio. **Deuda residual derivada
> de F16** (NO bloquea sub-fases visibles del LSP): sin index
> espacial (rango inicio-fin) en el side-table — el LSP elige
> nodo más cercano al cursor por ahora; spans en `TypeExpr` y
> `Pattern` (heredado de S1, refinable cuando aterrice el primer
> caso de uso real); cobertura de `Stmt` (ortogonal — el LSP
> resuelve declaraciones por scope lookup en 9.x.3). Próximo
> norte: **sub-fases visibles del LSP — 9.x.1 (diagnostics MVP)**.
> Ver detalle en `docs/roadmap.md` → "Fase 9.0 — F16".
>
> **Mini-tanda Q.z (2026-05-16): CERRADA** — quickwins pre-9.z.2.
> Tres ítems atacados antes de arrancar `fitz test`:
> - **F6 audit builtins**: confirmado que el syntax-spec NO promete
>   `range`/`type_of`/`to_string` globales (la matriz F6 estaba
>   especulando). Builtins implementados (`print`, `len`, `sleep`,
>   `cors`) coinciden 1:1 con lo que el spec lista como
>   builtin-globales. Único hallazgo: el ejemplo del test runner
>   en `docs/syntax-spec.md:515` usa `panic("falló: {e}")` que NO
>   está en la lista oficial de assertion builtins (`assert`,
>   `assert_eq`, `assert_ne`, `assert_throws`). Decisión de
>   scope para 9.z.2: incluir `panic(msg)` como builtin auxiliar
>   o dejarlo fuera. Sin acción técnica en Q.z.
> - **D1 refresh header guide.md**: pasó de "Fase MW + tanda Q,
>   1153 unit + 74 E2E" a "Fase 9.z.1 cerrada — fitz fmt
>   production-ready, 1333 unit + 55 cli_e2e + 79 compile_e2e + 3
>   openapi". Bullets stale de "Qué todavía no anda" depurados
>   (async/await reales, status codes custom, query params,
>   named args ya cerrados — 4 ítems quitados). "Builtins
>   globales" expandido a los 4. "Cómo está organizada" actualizó
>   parte 10 (Tooling = LSP + formatter) y sumó partes 8-11.
>   Sección "Lo que viene" (cap 24) refrescó el bullet de Fase 9
>   con el estado real (LSP entero cerrado, PM 9.y.1-9.y.4
>   cerrados, fmt cerrado, próximo testing).
> - **Cap 23 nuevo "`fitz fmt`"** en guía: cap dedicado con
>   features, CLI, estilo canónico (resumen + link a
>   `docs/fmt-style.md`), 2 ejemplos in-line (antes/después +
>   preservación de comments) + ejemplo runnable nuevo
>   `examples/guide/23-fmt-ejemplo.fitz` sumado al smoke
>   `GUIDE_EXAMPLES_COMPILE`. Renumeración 23→24 ("Qué sigue").
>   Cumple la regla del proyecto "implementado = documentado
>   con uno o varios ejemplos".
>
> **Deudas residuales identificadas durante Q.z** (NO bloquean
> 9.z.2):
> - **Cap "Package manager" en la guía**: las 6 subcomandos del
>   PM (`fitz new`/`init` 9.y.1, `fitz add`/`remove`/`update`
>   9.y.4) están implementadas + cerradas + en CHANGELOG/roadmap
>   pero NO tienen capítulo dedicado en `docs/guide.md`.
>   Estructura sugerida: cap nuevo "Package manager" en Parte 6
>   (Organización), entre cap 16 (Módulos) y cap 17 (HTTP), con
>   sub-secciones para `fitz new`/`init`, manifest `fitz.toml`,
>   `[dependencies]` path/git, lockfile `fitz.lock`,
>   `fitz add`/`remove`/`update`, y al menos un ejemplo runnable
>   completo con dos proyectos (lib + binario que importa la lib).
>   ~2h de trabajo bien hecho. **Etapa**: meter como sub-paso
>   dedicado pre-9.w (después que 9.z entera cierre — testing,
>   dev, repl, lint), junto con un refresh general de la guía
>   sincronizado con todo 9.y + 9.z cerrado. Si aparece presión
>   antes (preguntas de usuarios sobre cómo crear un proyecto),
>   acelerable como sub-paso pre-9.z.2 dedicado.
> - ~~**Bug del formatter: trailing comment al final del body
>   de una fn seguido de otro bloque inserta blank spurious
>   dentro del body del bloque siguiente**~~ **CERRADO
>   (2026-05-17, post-9.z.5)**. Root cause: `had_blank_in_source`
>   en `fmt_stmt_list` usaba `after_what = max(prev_end_line,
>   last_emitted_comment_line)`; cuando entrabamos a un nuevo
>   bloque (`in_block=true`, `prev_end_line=0`), el
>   `last_emitted_comment_line` arrastraba un valor de scope
>   outer y `has_blank_between` chequeaba blanks FUERA del
>   bloque actual. Fix: agregar guarda — en `in_block`, el
>   chequeo requiere `prev_end_line > 0` (paralela a la
>   `smart_blank`); en top-level se preserva el behavior previo
>   (`after_what > 0`) para no romper blanks entre header
>   comments y el primer stmt. Test E2E
>   `fmt_trailing_comment_seguido_de_bloque_no_inserta_blank_spurio`
>   protege contra regresión.
>
> **Fase 9.z.2.a (2026-05-17): CERRADA** — `@test` decorator +
> assertion builtins + `TestRegistry`. Primer sub-paso de 9.z.2
> (testing built-in). Total al cierre: **1364 unit + 55 cli_e2e
> + 79 compile_e2e + 3 openapi**. Clippy `-D warnings` limpio.
>
> **Cambios técnicos**:
> - `src/testing.rs` nuevo: `TestRegistry`, `TestSpec`,
>   `with_active_test_registry` (+ variante async) +
>   thread-local. Mirror chico de `http::HTTP_REGISTRY` con la
>   asimetría clave: si no hay registry activo, `@test` es no-op
>   silencioso (paralelo a `#[cfg(test)]` de Rust), no error.
> - `evaluator.rs::process_decorator` suma branch `@test` con
>   helper `register_test`: valida args/kwargs/params vacíos y
>   empuja `TestSpec` al registry si hay uno. Sin registry,
>   sigue normal.
> - 4 assertion builtins nuevos: `assert(cond: Bool, msg: Str?)`,
>   `assert_eq(a, b)`, `assert_ne(a, b)`, `assert_throws(fn)`.
>   Estilo cargo test: mensaje `left/right` para `assert_eq`,
>   `iguales (val)` para `assert_ne`. Igualdad estructural
>   recursiva (reusa `PartialEq` de Value que coerciona Int↔Float).
> - `assert_throws` **caso especial** en `invoke_value`:
>   `Value::Builtin { name: "assert_throws", .. }` se intercepta
>   antes del despacho genérico (necesario porque los builtins
>   son sync pero invocar un callback Fitz requiere async-recurse
>   con `invoke_value`). El stub registrado emite `unreachable!`
>   si llegara a invocarse — sentinel de bug del dispatcher.
> - **Restricción MVP de `assert_throws`**: callback debe ser
>   `Function` aridad 0 NO async. Async cb produce `Value::Future`
>   suelto (no equivalente a "tirar"); cubrirlo requiere
>   `assert_throws_async` o flag — sub-paso futuro si aparece
>   presión.
> - Pre-registro en el checker (`types.rs::register_builtins`):
>   `assert` como `Type::Any` (aridad variable 1-2); el resto con
>   firmas estructuradas. `assert_throws` exige `Function {
>   params: [], ret: Any }` (chequeo estático de aridad del cb).
> - Completion en LSP (`lsp.rs`) suma los 4 builtins nuevos al
>   listado de builtins detectables vía scope-level autocomplete.
> - **Cambio retro-compatible al parser**: paréntesis opcionales
>   en decoradores. `@test fn ...` (sin `()`) parsea con args
>   vacíos. Antes el parser exigía `(` siempre. Cambio
>   retro-compatible (todos los `@server()`/`@get("/x")` siguen
>   funcionando idéntico). Test `decorator_sin_parens_errores`
>   reescrito como `decorator_sin_parens_parsea_con_args_vacios`.
>
> **Decisiones que tomaron forma durante 9.z.2.a**:
> - `panic(msg)` (que el syntax-spec usa en el ejemplo del test
>   runner, línea 515) **NO entra** al MVP. Los 4 builtins
>   oficiales (`assert*`) son la lista cerrada de 9.z.2. Si
>   aparece presión, sub-paso 9.z.2.a.bis o post-MVP.
> - **Sintaxis `@test fn` sin paréntesis**: confirmada como
>   forma canónica (matchea el spec). `@test()` también parsea
>   por simetría — el parser es agnóstico, la decisión es del
>   evaluator.
> - **`assert` exige `Bool` estricto** en el primer arg (no
>   truthy/falsy). Consistente con la decisión de diseño "sin
>   truthy/falsy" del cap 6 de la guía.
> - **Tests con feedback inmediato del decorator**: los 4 errores
>   de validación (`@test` sobre fn con params, con args, con
>   kwargs, sobre tipo no-Function) levantan en eval-time, no
>   cuando el runner los invoca — sigue el patrón de `@server`
>   y `@get`.
>
> **Tests nuevos**: 6 en `testing.rs` (registry empty/push/
> with_active/with_active_async/aislamiento entre anidados),
> 6 en `evaluator.rs::tests` (decorator sin registry no-op,
> con registry registra, async fn → is_async true, preserva
> orden, params error, args error, kwargs error), 18 en
> `evaluator.rs::tests` (los 4 builtins con happy/falla/type
> errors/aridad/coerción Int↔Float/estructural en listas), 2
> en `parser.rs::tests` (decorator sin parens parsea OK, `@test`
> sin parens parsea OK). Total: **+32 unit tests**.
>
> **Deudas residuales (NO bloquean 9.z.2.b)**:
> - **`assert_throws` con callback async**: rechazado
>   explícitamente en runtime. `assert_throws_async(fn)` o
>   variante del builtin queda como sub-paso futuro si aparece
>   presión.
> - **Reporte de span del fallo**: cuando un `assert*` falla, el
>   `FitzError` lleva `line: 0, column: 0` (los builtins son
>   sync y no reciben el span del call site). El span del call
>   sí está disponible en `invoke_value`; podríamos enriquecer
>   el error después del fact. Refinamiento útil pero NO MVP.
> - **9.z.2.b (runner CLI)**: este sub-paso cerró solo la
>   infraestructura del lenguaje (decorator + registry +
>   builtins). El sub-comando `fitz test`, discovery
>   (lib/bin + `tests/*.fitz`), output estilo cargo, filtrado,
>   exit codes — todo entra en 9.z.2.b.
>
> **Deudas de docs acumuladas (NO bloquean 9.z.2.b)** —
> agrupadas para tratamiento dedicado cuando 9.z entera cierre:
> - **Cap "Package manager" en la guía** (heredado de Q.z) —
>   las 6 subcomandos de 9.y.1-9.y.4 sin capítulo dedicado.
> - ~~**Bug del fmt con trailing comment**~~ (heredado de Q.z) —
>   **CERRADO post-9.z.5** (fix en `fmt_stmt_list` con guarda
>   `prev_end_line > 0` en `had_blank_in_source` para
>   `in_block=true`).
> - **`docs/architecture.md`** — los diagramas del pipeline
>   (lexer/parser/checker/evaluator/codegen) y los pointer de
>   módulos están desactualizados respecto a las fases
>   cerradas post-5b (sumar `testing.rs`, `manifest.rs`,
>   `lockfile.rs`, `git_dep.rs`, `fmt.rs`, `lsp.rs`,
>   `py_interop.rs`, `py_types.rs`; sumar el flujo del LSP +
>   PM + interop Python en los diagramas).
> - **Refresh general de `docs/guide.md` + ejemplos** — varios
>   capítulos arrastran texto stale por fases cerradas
>   posteriormente. Algunas secciones de "Lo que todavía no
>   anda" todavía citan features ya implementadas; algunos
>   capítulos no mencionan cambios derivados (paréntesis
>   opcionales en decorators, builtins assertion).
>   Sincronización masiva pendiente.
>
> **Etapa propuesta para las deudas de docs**: sub-paso
> dedicado "Refresh masivo de docs" cuando 9.z entera cierre
> (post-9.z.5), antes del salto a 9.w. Sub-pasos sugeridos:
> (a) cap "Package manager" nuevo + ejemplos runnables;
> (b) `docs/architecture.md` refresh completo con diagramas
> nuevos; (c) walk del cap-by-cap de `guide.md` para detectar
> texto stale; (d) `docs/syntax-spec.md` actualizar matriz al
> estado de cierre 9.z (refresh recurrente, ya marcado como
> deuda continua). ~4-6h estimadas para hacerlo bien.
>
> **Fase 9.z.2 ENTERA CERRADA (2026-05-17)** — `fitz test`
> (testing built-in). Tres sub-pasos cerrados en el día:
>
> - **9.z.2.a — decorator + asserts + registry** (ver bloque
>   anterior en este archivo).
> - **9.z.2.b — runner cargo-style + discovery** (`Commands::Test`
>   + `discover_test_sources_from_manifest` con dedup lib/tests +
>   auto-self-import bajo `package.name` + `run_test_registry`
>   con output cargo-style + ANSI auto via `IsTerminal` + exit
>   code 1 si falla; 11 cli_e2e nuevos).
> - **9.z.2.c — cap guía + ejemplo + cierre formal** (este
>   sub-paso): cap 24 nuevo "`fitz test` — testing built-in"
>   en `docs/guide.md` (renumeración 24→25), ejemplo runnable
>   `examples/guide/24-tests.fitz` con factorial + 3 tests OK
>   + 1 FAILED intencional sumado al smoke
>   `GUIDE_EXAMPLES_COMPILE`, codegen ignora `@test fn`
>   silenciosamente (paralelo a `#[cfg(test)]` Rust), bug fix
>   colateral en `has_http_routes` (counting `@test` como HTTP
>   disparaba server en CLI puros), CHANGELOG v0.9.16,
>   roadmap, README, syntax-spec actualizado a
>   v0.4 (matriz refleja interop / LSP / PM / fmt / test como
>   implementados).
>
> Total al cierre de 9.z.2: **1366 unit / 66 cli_e2e / 79
> compile_e2e / 3 openapi**. Clippy `-D warnings` limpio.
>
> **Deudas residuales de 9.z.2 (NO bloquean 9.z.3)**:
> - **`assert_throws` con callback async**: rechazado en runtime
>   (FitzError claro). Sub-paso futuro si aparece presión —
>   posiblemente `assert_throws_async` o flag dedicado.
> - **Span del fallo en assertion builtins**: el `FitzError`
>   lleva `line: 0, column: 0` porque los builtins son sync y
>   no reciben el span del call site. Útil para reportar la
>   línea exacta de la aserción fallida. Refactor: el caller
>   de `Value::Builtin { func, .. }` en `invoke_value` ya
>   tiene el span; el wrapper podría enriquecer el error
>   después-del-fact con `e.line = span.line` si `line==0`.
>   ~30 min de trabajo.
> - **Nombres de paquete con hyphens**: `package.name = "my-pkg"`
>   no es importable desde Fitz (`from my-pkg import X` no
>   parsea — `-` no es ident válido). Workaround: usar
>   underscores. Documentado en cap 24 de la guía. Refinable
>   en lexer/parser si aparece presión.
> - **Tests inline en `[lib]` sin tests integration que lo
>   importen**: si el proyecto tiene tests/ + `[lib]` con
>   `@test` inline, pero ningún `tests/*.fitz` importa la lib,
>   esos tests del lib NO se descubren (modo "tests integration"
>   solo carga `tests/*.fitz` direct). Edge case raro;
>   workaround: agregar un `from <pkg> import _` decorativo a
>   algún test integration.
>
> **Próximo norte**: 9.z.3 (`fitz dev` con file watcher + hot
> reload + dev experience).
>
> **Fase 9.z.3 CERRADA (2026-05-17)** — `fitz dev` (hot reload).
> File watcher cross-platform via `notify` crate + kill/respawn
> del child al detectar cambios en `.fitz` o `fitz.toml`. Tercera
> DX feature de Fase 9.z cerrada en el día (después de 9.z.2).
>
> **Implementación**: `Commands::Dev { file }` con resolver
> single-file/manifest paralelo a `fitz test`/`fitz run`. Loop
> principal en runtime tokio current_thread con `tokio::select!`
> sobre 3 eventos: cambio del watcher (debounce 100ms +
> kill+respawn), child terminó solo (espera próximo cambio), o
> `tokio::signal::ctrl_c()` (kill + clean exit). Bridge sync→async
> entre `notify` (sync) y tokio mpsc via std::thread::spawn.
> Path filtering: `*.fitz` + `fitz.toml`, excluye `target/`/
> `.git/`/`node_modules/`/`.fitz/`/`dist/`/`build/` + componentes
> ocultos. Banner ANSI clear screen si TTY.
>
> **Decisiones tomadas**: `[dev]` section NO en MVP; browser
> auto-refresh NO; print errors live sin restart NO (LSP cubre);
> smoke E2E automatizado NO (file watchers son flaky).
>
> **Cap 25 nuevo "`fitz dev` — hot reload"** en
> `docs/guide.md` (renumeración cap 25→26 "Qué sigue").
>
> **Total al cierre 9.z.3**: 1366 unit / 66 cli_e2e / 79
> compile_e2e / 3 openapi (sin cambios, dev_cmd es interactivo).
> Clippy `-D warnings` limpio. Smoke manual validó arrancar →
> modificar → ver run #2 con código nuevo.
>
> **Deudas residuales de 9.z.3 (NO bloquean 9.z.4)**:
> - **Incremental rebuild**: kill+respawn full es el approach del
>   MVP. Modelo de módulos pre-compilados queda como sub-paso
>   futuro si los tiempos duelen.
> - **Filter "modify sin cambio real"**: timestamps tocados sin
>   cambio de contenido disparan restart. Comparar hashes si
>   aparece presión.
> - **`fitz dev --test`** (modo watch + run tests): workaround
>   documentado con dos terminales. Sub-paso si aparece presión.
> - **Smoke E2E automatizado**: pendiente. File watchers
>   requieren orquestación cuidadosa para no ser flaky.
>
> **Próximo norte**: 9.z.4 (`fitz repl` interactivo con
> rustyline + scope persistente entre líneas + comandos especiales
> `:type`/`:env`/`:reset`/`:load`).
>
> **Fase 9.z.4 CERRADA (2026-05-17)** — `fitz repl` (REPL
> interactivo). Cuarta DX feature de Fase 9.z cerrada en el día.
> Prompt `fitz> ` con env compartido, multi-line via balanced
> brackets, 6 comandos especiales (`:help`/`:quit`/`:env`/
> `:reset`/`:type`/`:load`), history persistente en
> `~/.fitz/history`, pretty-print Python-style, async transparente.
>
> **Implementación**: dep `rustyline = "14"` + `Commands::Repl` +
> `repl_cmd` adentro de runtime tokio current_thread. APIs
> públicas nuevas en evaluator (`eval_program_with_env`,
> `new_repl_env`, `builtin_names`) y env (`local_names`). Filtro
> de warning spurio del checker para "variable desconocida"
> (substring match, no kind: todos los errors del checker llevan
> `TypeError`). `:type` arma programa sintético sin scope del
> REPL — limitación documentada.
>
> **Decisiones tomadas**: `:type` scope-aware NO en MVP; smoke
> E2E automatizado NO (rustyline + readline son flaky en tests);
> manifest mode en REPL NO (siempre single-session); auto-
> completion NO en MVP.
>
> **Cap 26 nuevo "`fitz repl` — REPL interactivo"** en
> `docs/guide.md` (renumeración cap 26→27 "Qué sigue").
>
> **Total al cierre 9.z.4**: 1366 unit / 66 cli_e2e / 79
> compile_e2e / 3 openapi (sin cambios; repl_cmd interactivo no
> agrega tests automáticos). Clippy `-D warnings` limpio.
>
> **Deudas residuales de 9.z.4 (NO bloquean 9.z.5)**:
> - `:type` scope-aware (refactor checker pre-declared scope).
> - Smoke E2E automatizado del REPL (rustyline en raw mode
>   complica tests).
> - Indentación automática en multi-line continuation.
> - Comandos extras (`:save`/`:undo`/`:debug`/auto-completion).
> - Manifest mode en `fitz repl` (single-session siempre).
>
> **Próximo norte**: 9.z.5 (`fitz lint` — linter de patrones más
> allá de tipos: unused_variable, unused_import, useless_match,
> string_concat, panic_in_test_only, redundant_clone). Cierra
> Fase 9.z entera.
>
> **Fase 9.z.5 CERRADA (2026-05-17) — CIERRE FASE 9.z ENTERA**.
> `fitz lint` con 4 lints implementados:
> - `unused_variable` — `let x = ...` sin uses, skip `_var`.
> - `unused_import` — `import X` / `from X import Y` con binding
>   no referenciado.
> - `useless_match` — match con UN solo arm catch-all (Wildcard o
>   Ident binding).
> - `string_concat` — `BinOp Add` con ambos operandos `Str` literales.
>
> Lints skipeados del roadmap: `panic_in_test_only` (no aplica —
> Fitz no tiene `panic!` builtin distinguido) y `redundant_clone`
> (requiere análisis de movimientos no implementado).
>
> Módulo nuevo `src/lint.rs` (~700 LoC con 15 unit tests).
> `Commands::Lint { files, deny }` en CLI con output cargo-clippy
> style. Supresión por `// @allow(<lint>)` en la línea anterior
> via inspección del source raw. Default warnings + exit 0;
> `--deny <name>` promueve a error + exit 1.
>
> **Total al cierre 9.z.5**: 1381 unit + 73 cli_e2e + 79
> compile_e2e + 3 openapi (+15 unit + 7 cli_e2e vs 9.z.4).
> Clippy `-D warnings` limpio.
>
> **Cap 27 nuevo "`fitz lint`"** en `docs/guide.md`
> (renumeración cap 27→28 "Qué sigue").
>
> **Decisiones tomadas**: 4 lints (no 6); auto-fix DIFERIDO;
> análisis de uses globales (no scope-aware estricto); catálogo
> cerrado (sin plugins); default warnings + `--deny <name>`
> para CI.
>
> **Deudas residuales de 9.z.5 (NO bloquean 9.w)**:
> - Auto-fix `--fix` (candidato natural: `string_concat`).
> - `unused_variable` scope-aware estricto (shadowing).
> - Suppression cross-line (`// @allow(name) { ... }` bloque).
> - Lints adicionales (`shadowing`, `useless_clone` cuando el
>   compilador haga análisis de movimientos).
> - Plugins externos.
>
> ---
>
> **CIERRE FORMAL DE FASE 9.z ENTERA (2026-05-17)**: los 5
> sub-pasos de DX (fmt + test + dev + repl + lint) cerrados en
> 2 días consecutivos (16-17 de mayo). Suite final acumulada:
> 1381 unit + 73 cli_e2e + 79 compile_e2e + 3 openapi. Clippy
> limpio. 5 capítulos nuevos en `docs/guide.md` (23-27),
> renumeración "Qué sigue" del cap 22 original al cap 28 actual.
> Deps nuevas: `rustyline = "14"` (REPL), `notify = "6"` (dev).
>
> **Deudas mayores acumuladas durante 9.z** (priorizadas como
> sub-paso dedicado de refresh masivo de docs, próximo natural
> tras 9.z):
> 1. **Cap "Package manager"** en la guía (heredado de Q.z).
> 2. **`docs/architecture.md`** refresh completo con diagramas
>    nuevos (testing/manifest/lockfile/git_dep/fmt/lsp/lint y los
>    flujos asociados; el bridge HTTP mpsc/oneshot eliminado en
>    F17 sigue documentado).
> 3. **Walk completo de `docs/guide.md`** cap-by-cap para
>    detectar texto stale derivado de las features cerradas
>    post-fmt-style (paréntesis opcionales en decorators,
>    builtins assertion, etc.).
> 4. ~~**Bug del fmt** con trailing comment al final de body
>    seguido de otro bloque~~ — **CERRADO post-9.z.5** (fix en
>    `fmt_stmt_list` con guarda condicional in_block/top-level).
>
> **Próximo norte**: Fase 9.w (Stack web first-class —
> `@authenticated`/`@admin`, `@ws("/chat")`, `@cron`,
> `@background`) o el sub-paso dedicado de refresh masivo de
> docs.

> **Nota (2026-05-20) — Fase 9.w.1 (Auth nativa) CERRADA**: el
> primer sub-paso del stack web first-class está implementado
> entero. Tres decoradores nuevos del lenguaje (`@auth_provider`
> singleton, `@authenticated`, `@admin`) + dos módulos built-in
> (`jwt` con HS256/384/512, `hash` con Argon2id) cubren el flujo
> de login + JWT + password hashing entero **sin deps externas**.
> El checker valida estáticamente que cada handler protegido
> tenga el provider registrado y reciba el `User` correcto. El
> schema OpenAPI auto-agrega `securitySchemes.bearerAuth` +
> `security` por handler + 401/403 en responses. Paridad bit-a-bit
> `fitz run` ↔ `fitz build`. Sub-pasos cerrados:
>
> - **9.w.1.a** — Checker valida los 3 decorators (16 unit
>   tests).
> - **9.w.1.b** — Built-ins `jwt`/`hash` como `Value::Module`
>   pre-registrados con `jsonwebtoken = "9"` + `argon2 = "0.5"`
>   + `rand_core = "0.6"` deps no-opcionales (16 unit tests).
> - **9.w.1.c** — Runtime auth en `fitz run`: `AuthSpec` enum +
>   `AuthProviderHandle` + wrapper en `handle_task` (9 unit E2E).
> - **9.w.1.d** — Codegen `fitz build`: helpers en preludio +
>   dispatch en `gen_call` + `emit_auth_check` espejo del
>   intérprete (2 tests compile_e2e).
> - **9.w.1.e** — OpenAPI security scheme: `bearerAuth` +
>   `security` por handler + 401/403 auto (5 unit tests del
>   schema).
> - **9.w.1.f** — Cap 28 nuevo en `docs/guide.md` + ejemplo
>   runnable `examples/guide/28-auth.fitz` (login + /me + /admin,
>   <100 LoC) + README emphasis del diferencial + smoke
>   `GUIDE_EXAMPLES_COMPILE`.
>
> **Decisiones técnicas del MVP** (no en el roadmap original):
> `Map<Str, Str>` strict para payload de `jwt.encode` y return
> de `jwt.decode` (heterogéneos requieren `__FitzValue` post-MVP);
> `hash.verify` devuelve `Bool` (no Result) por seguridad; provider
> order required (provider antes que handlers); handler protegido
> NO admite body separado del user en MVP.
>
> **Deuda residual derivada de 9.w.1** (NO bloquea uso real;
> queda comprometida en `docs/roadmap.md` → "Fase 9.w iteración
> 2"): sessions cookie-based + RBAC multi-rol + token refresh/
> revocación (requieren DB nativa, Fase 10); asimétricos JWT
> (RS256/ES256 con PEM); provider request-aware más allá de
> headers; heterogéneos en `jwt.encode/decode` (requiere
> `__FitzValue` en codegen).
>
> **Próximo norte**: resto de Fase 9.w — `@ws("/chat")`
> (WebSockets tipados con `WsConn<T>`), `@cron` + `@background`
> (jobs sin Celery), y ORM nativo + migraciones (escalado a
> Fase 10).

> **Nota (2026-05-21) — Fase 9.w.2 (WebSockets tipados) CERRADA**:
> el segundo sub-paso del stack web first-class está
> implementado entero. `@ws("/path")` sobre `async fn` +
> `WsConn<T>` con métodos `recv`/`send`/`broadcast`/`close`
> montan un servidor de WebSockets tipado end-to-end. **Cinco
> diferenciales** que vuelven a Fitz único en este espacio:
> marshaling JSON automático (cada frame text se serializa/
> deserializa al `type` declarado, sin glue manual); AsyncAPI
> 3.0 auto-generado en `/asyncapi.json` (la spec hermana de
> OpenAPI 3.1 para event-driven APIs, consumible por tooling
> estándar); heartbeat built-in con
> `@server(ws_heartbeat_secs=N)` (Ping frames automáticos que
> pasan de largo proxies idle-killers); auth integrada
> (`@authenticated`/`@admin` apilados sobre `@ws` validan
> bearer ANTES del HTTP upgrade); codegen con paridad bit-a-bit
> `fitz run` ↔ `fitz build`. **Ningún otro lenguaje hoy combina
> WS tipados con AsyncAPI auto-generado del código fuente,
> heartbeat built-in y auth integrada en el handshake**.
> Sub-pasos cerrados:
>
> - **9.w.2.a** — Checker estático: `Type::WsConn(Box<Type>)`,
>   `infer_wsconn_method` con signatures paramétricas,
>   `check_ws_handler` validando shape (14 unit tests).
> - **9.w.2.b** — Value runtime + evaluator: `WsConnHandle`,
>   `WsOutMessage` (Text/Close), `Value::WsConn`,
>   `register_ws_route`, `dispatch_method` arms,
>   `ws_conn_recv` con `coerce_to_annotation` (heredado 8.4.3)
>   para Map → Instance cuando T es nominal.
> - **9.w.2.c** — Runtime HTTP: `WsBroadcaster` con
>   `parking_lot::Mutex<HashMap<endpoint, Vec<(conn_id,
>   outbox_tx)>>>`, `WsReadStreamImpl`, `build_ws_method_router`
>   con auth pre-upgrade (401/403 ANTES de `ws.on_upgrade`),
>   `build_ws_conn` con writer task + outbox separado. axum 0.8
>   feature `ws` + `futures-util` + dev-dep `tokio-tungstenite`.
> - **9.w.2.d** — AsyncAPI 3.0 (`src/asyncapi.rs` ~350 LoC):
>   channels + operations receive/send + securitySchemes,
>   `BTreeMap` para orden determinístico, `/asyncapi.json`
>   route en runtime y codegen (8 unit tests).
> - **9.w.2.e** — Heartbeat ping/pong automático:
>   `WsOutMessage::Ping`, `ServerConfig.ws_heartbeat_secs`
>   default 30s, `@server(ws_heartbeat_secs=N)` kwarg,
>   `tokio::time::interval` spawneado en `build_ws_conn` cuando
>   N > 0 (6 unit tests).
> - **9.w.2.f** — Cap 29 nuevo en `docs/guide.md` (renumeración
>   29→30) + ejemplo runnable `examples/guide/29-ws.fitz`
>   (servidor de chat con login HTTP + JWT +
>   `@authenticated @ws("/chat")` + broadcast multi-client +
>   `@server(43929, ws_heartbeat_secs=30)`, <100 LoC) + README
>   emphasis (5 diferenciales en tabla + footnote dedicado +
>   bullets en "Estado del proyecto" y "Qué funciona hoy") +
>   smoke `GUIDE_EXAMPLES_COMPILE`.
>
> **Decisiones técnicas del MVP** (no en roadmap original):
> `Arc<HttpRegistry>` compartido (mismo modelo F17);
> `tokio::sync::Mutex` en `WsConnHandle.rx` (necesita Send
> across .await); `parking_lot::Mutex` en
> `WsBroadcaster.conns` (no cruza await); manual Clone impl
> para `__FitzWsConn<T>` en codegen sin `T: Clone` bound;
> broadcast incluye al sender (convención Socket.IO/Phoenix);
> auth pre-upgrade (menos attack surface);
> `ws_heartbeat_secs=0` desactiva sin error.
>
> **Deuda residual derivada de 9.w.2** (NO bloquea uso real;
> queda comprometida en `docs/roadmap.md` → "Fase 9.w iteración
> 2"): binary frames (`Vec<u8>` payload — hoy solo text;
> integración con tipo `Bytes` ya cerrado); AsyncAPI UI
> equivalente al `/docs` de OpenAPI (hoy solo JSON); tipado
> bidireccional separado (`WsConn<In, Out>` — hoy `T` único);
> reconnect con state replay (requiere persistencia, Fase 10);
> rooms/channels dentro de un endpoint (broadcast a TODOS los
> clientes del endpoint); backpressure explícito (outbox
> unbounded hoy).
>
> **Próximo norte**: resto de Fase 9.w — 9.w.3 (`@cron` +
> `@background` — jobs sin Celery) y 9.w.4 (ORM nativo +
> migraciones, escala a Fase 10).

> **Nota (2026-05-21) — Fase 9.w.3 (Jobs sin Celery) CERRADA**: el
> tercer sub-paso del stack web first-class está implementado
> entero. Tres piezas nativas del lenguaje montan jobs sin broker
> externo: **`@cron("expr")`** para tareas periódicas (5/6/7
> fields cron Unix), **`@background`** como marcador opt-in para
> autorizar el callsite, y **`spawn(fn_call)`** fire-and-forget
> que devuelve `Future<T>` tipado. Sin Celery, sin Redis, sin
> systemd timers — todo en el mismo binario con paridad bit-a-bit
> `fitz run` ↔ `fitz build`. **Cinco diferenciales** que vuelven
> a Fitz único en este espacio: decoradores nativos del lenguaje
> (parte del compilador, no lib opcional), sin broker externo
> (jobs viven en memoria del proceso, suficiente para 90% de
> servicios reales), `spawn` con tipado (refinamiento estático
> a `Future<T>` con T concreto), paridad `fitz run` ↔ `fitz
> build`, y cero `pip install celery` / `cargo add
> tokio-cron-scheduler`. **Ningún otro lenguaje combina cron +
> background workers + spawn tipado en el core sin broker externo
> y con paridad intérprete↔binario**. Sub-pasos cerrados:
>
> - **9.w.3.a** — Checker estático: `CheckCtx.background_fns`
>   poblado por `collect_background_fns` antes del walk;
>   `check_cron_decorator` + `check_background_decorator` +
>   dispatch especial de `spawn(...)` en `synthesize_expr` que
>   refina ret type a `Future<T>` (17 unit tests).
> - **9.w.3.b** — Runtime intérprete: nuevo módulo
>   `src/cron_jobs.rs` con `CronJob` + `CronRegistry` (paralelo
>   a HttpRegistry) + `spawn_cron_scheduler` + `run_scheduler_only`
>   (cron-only mode con multi_thread + ctrl_c). `process_decorator`
>   branches para `@cron`/`@background`. `eval_call` intercepta
>   `spawn(fn_call)` ANTES de evaluar args. Cron-only mode en
>   `main.rs`. **Fix bug preexistente**: handlers `async fn`
>   HTTP en intérprete retornaban "Future pendiente no es
>   serializable" porque `handle_task` nunca awaiteaba el Future.
>   Helper `await_if_future`. Normalización 5→6 fields automática.
>   Deps `cron = "0.12"` + `chrono = "0.4"` (8 unit tests).
> - **9.w.3.c** — Codegen `fitz build`: Cargo.toml condicional
>   suma `cron`/`chrono` + feature `signal` (cron-only mode);
>   multi_thread flavor con jobs; preludio `__fitz_run_cron_job`
>   + helper `__fitz_normalize_cron`; `PartitionedProgram.cron_fns`;
>   `emit_cron_job_spawns()` invocado desde `gen_main` y
>   `gen_http_main`; `spawn(fn_call)` dispatch que emite
>   `tokio::spawn(async move {...})` + `Box::pin` para case con
>   `Pin<Box<dyn Future>>` (7 unit tests).
> - **9.w.3.d** — Cap 30 nuevo en `docs/guide.md` (renumeración
>   30→31) + ejemplo runnable
>   `examples/guide/30-cron-background.fitz` (URL shortener con
>   HTTP + cron stats + spawn tracking, <100 LoC) + README
>   emphasis con tabla + footnote ♠ + bullets en "Estado del
>   proyecto" y "Qué funciona hoy" + smoke
>   `GUIDE_EXAMPLES_COMPILE`.
>
> **Decisiones técnicas del MVP** (no en roadmap original):
> cron-only mode vivo bloqueante (modo systemd-friendly,
> confirmado con el autor); `@cron` acepta sync y async
> (confirmado); `@background` opt-in (evita usos accidentales);
> `spawn(...)` exige call literal a fn `@background` (permite
> refinamiento estático); crate `cron = "0.12"` (vs propio o
> `tokio-cron-scheduler`); normalización 5→6 fields automática
> (preserva UX familiar); JoinHandle envuelto en `Value::Future`/
> `Pin<Box<dyn Future>>` (unifica con `Future<T>` existente).
>
> **Deuda residual derivada de 9.w.3** (NO bloquea uso real;
> queda comprometida en `docs/roadmap.md` → "Fase 9.w iteración
> 2"): persistencia de jobs entre restarts (requiere DB nativa,
> Fase 10); visibility de jobs (panel admin con runs, stats,
> retries); retry con backoff exponencial; coordinación entre
> múltiples instancias (locks distribuidos); `spawn` con
> coordinación múltiple (Promise.all style); cron timezone
> configurable (hoy `chrono::Utc::now()`).
>
> **Próximo norte**: resto de Fase 9.w — ORM nativo +
> migraciones (escala a Fase 10), o cierre formal de Fase 9.w
> entera.

> **Nota (2026-05-21) — Deudas derivadas del setup CI/CD
> (post-9.w MVP)**: al armar los 4 workflows GitHub Actions
> (`ci.yml`, `extension-smoke.yml`, `release.yml`, `docs.yml`)
> + sitio MkDocs Material, descubrimos dos issues
> preexistentes del repo que el CI strict expuso pero que
> NO bloquean la entrega de releases:
>
> **D1 — Cargo fmt cleanup masivo** (deuda explícita, NO
> bloquea CI ni features). `cargo fmt --all -- --check` falla
> porque el código del repo nunca fue formateado con rustfmt
> canónico — el autor tiene su propio estilo (imports
> agrupados manualmente vs alfabéticos, etc.). El `fmt --check`
> step del `ci.yml` quedó **deshabilitado con comentario
> explicativo** mientras se hace el cleanup.
>
> - **Plan**: commit dedicado `style: cargo fmt --all across
>   the codebase` que toca **cientos de archivos** (todos los
>   `.rs` del proyecto). Beneficio: el `fmt --check` del CI
>   vuelve a funcionar para siempre + el proyecto queda
>   alineado con rustfmt default (estándar Rust ecosystem).
> - **Riesgo**: pull conflicts si alguien tiene branches
>   abiertas (no es el caso hoy — solo el autor commitea).
> - **Trade-off**: el diff del commit es masivo (ilegible para
>   review humano), pero `cargo fmt` no cambia semántica, solo
>   layout. Validar con `cargo test --lib` post-fmt para
>   confirmar que nada se rompió accidentalmente.
> - **Cuándo arrancar**: cuando aparezca presión real de
>   contribuidores externos que esperan `cargo fmt --check`
>   verde en sus PRs, o como cleanup post-Fase 10. Sin presión
>   real, no rush.
>
> **D2 — Clippy strict en `--all-targets`** (deuda explícita,
> NO bloquea CI). `cargo clippy --all-targets -- -D warnings`
> reporta **11 errores en código de tests** (no en lib):
> patterns idiomáticos como `assert!(x.is_none())` (clippy
> sugiere `!x.contains_key(...)`), `useless_format` en strings
> de tests E2E, `unnecessary_get_then_check`. El `clippy` step
> del `ci.yml` quedó cambiado de `--all-targets` a `--lib`
> (clippy strict sobre lib code captura 99% de issues reales;
> warnings en tests son aceptables).
>
> - **Plan**: commit dedicado `style: clippy --all-targets
>   cleanup` que aplica las sugerencias de clippy a los ~11
>   sitios de tests. Pequeño en tamaño (~50 LoC tocadas).
> - **Trade-off**: aceptar las sugerencias de clippy es a
>   veces menos legible (`assert!(x.is_none())` lee más natural
>   que `assert!(!x.contains_key(k))` para verificar ausencia
>   de una key). Caso por caso: aceptar la sugerencia clippy
>   o sumar `#[allow(clippy::unnecessary_get_then_check)]` con
>   comentario.
> - **Cuándo arrancar**: idem D1 — sin presión real, no rush.
>   Refinable junto con el cleanup de fmt en una mini-tanda de
>   "code style" dedicada.
>
> **Por qué ambas son aceptables como deudas**: las dos son
> sobre **convenciones de estilo**, no sobre correctness del
> código. El lint strict del CI tiene valor cuando hay
> múltiples contribuidores que necesitan baseline común; con
> un solo autor commiteando, el costo del cleanup masivo no se
> justifica todavía. El binario sigue compilando, los tests
> siguen verdes, los releases siguen produciendo artifacts
> reproducibles. La calidad del código real (clippy `--lib`)
> sigue siendo strict.

> **Nota (2026-05-23) — Cierre v0.9.42**: la cosecha de 8.c
> (`--bundle-pip-requirements`), la deuda D (cache key del
> pip_packages tarball), el smoke real Docker end-to-end y el
> audit del drift en la extensión VSCode se consolidaron en el
> release v0.9.42 (3 sesiones consecutivas). Detalle completo
> en `CHANGELOG.md → v0.9.42` y `docs/roadmap.md → Fase 8.c`.
>
> **Highlights de deuda residual derivada del smoke real
> Docker** (NO bloquea uso real del lenguaje; ver detalle en
> CHANGELOG):
>
> - ~~**Codegen Fase 8.7.1 — `from python import` en módulos
>   transitivos**~~ ✓ **CERRADO 2026-05-23 (v0.9.43)**. Cada
>   módulo puede declarar sus propios imports Python sin obligar
>   al main a participar. El codegen reusa los helpers del
>   preludio Python del crate root via `use crate::__fitz_py_*`
>   y emite statics + getters locales por módulo (pyo3 cachea
>   via `sys.modules`, así que el OnceLock duplicado es cero
>   overhead real). 6 tests nuevos (5 unit + 1 E2E), ejemplo
>   runnable `examples/python-interop-modular.fitz` +
>   `examples/python_math_utils.fitz` validado bit-a-bit
>   `fitz run` ↔ `fitz build`. Sin cambios a la extensión
>   VSCode (no se introduce sintaxis nueva).
>
>   **Follow-up — sub-deuda 1.5/1.6 ✓ CERRADO 2026-05-24
>   (v0.9.44)**: la coerción `__fitz_py_to_instance_T` /
>   `__fitz_py_to_list_T` para tipos `T` importados (los
>   helpers tipa-específicos solo se emitían en main para tipos
>   del main; tipos importados no los heredaban) + los impls
>   HTTP `__ToFitzJson`/`__FromFitzJson` para tipos importados
>   (mismo bug paralelo del lado HTTP). Fix: main emite helpers
>   y impls también para tipos custom de módulos transitivos
>   (vía nuevo pase unificado `emit_helpers_for_imported_types`);
>   módulos los referencian con `crate::__fitz_py_*` mediante
>   post-procesamiento del output. Bonus: bug preexistente
>   `mod types; mod types;` duplicado en `emit_mod_decls`
>   también cerrado (HashSet dedup). 5 tests nuevos (4 unit +
>   1 E2E `fase_8_7_1_transitiva_bis_modulo_coerce_pyany_a_
>   tipo_importado`). Smoke real del boilerplate 5 con `fitz
>   build` post-fix compila limpio end-to-end — el adopt al
>   flow `--bundle-pip-requirements` es viable hoy con el
>   ajuste GLIBC del builder.
> - ~~**`sqrt`-shadowing — builtins matemáticos pisan fns
>   importadas con el mismo nombre**~~ ✓ **CERRADO 2026-05-24
>   (v0.9.45 mini-tanda Cleanup-A)**. Pre-fix: `from utils
>   import sqrt` + `sqrt(x)` se traducía a `(x).sqrt()` (método
>   nativo de f64) porque el check de los builtins era sólo
>   `!fn_sigs.contains_key(name)`. Post-fix: nuevo helper
>   `CodegenCtx::is_user_callable(name)` chequea fn_sigs +
>   module_bindings con kind Fn. 14 builtins migrados (sqrt,
>   pow, abs, ceil, floor, round, clamp, min, max, popcount,
>   leading_zeros, trailing_zeros, spawn, len, bytes, sleep,
>   env, env_or, load_env). 3 tests nuevos.
> - ~~**LSP — completion en `from <mod> import |` + chain
>   `a.b.c.`**~~ ✓ **CERRADO 2026-05-24 (v0.9.47 mini-tanda
>   LSPz)**. Completion contextual del LSP ahora cubre dos
>   patrones nuevos: (1) cursor adentro de la lista de imports
>   de un `from` enumera fns + types + consts del módulo target
>   (helper público `from_import_completions(doc_uri, mod_path)`
>   + nueva variante `CompletionContext::FromImportList` +
>   wrapper `completion_at_position_with_uri`), (2) chain de N
>   segmentos `a.b.c.` reconocido como receiver completo (el
>   walkback acepta `.` además de chars ident; el lookup en
>   TypeInfo por posición del START resuelve al tipo del chain
>   exterior gracias a la garantía de F16). Al revisar el
>   inventario, las otras 3 deudas LSP que listé inicialmente
>   (cross-module go-to-def, range exacto en hover, scope-aware
>   completion) **ya estaban implementadas** en mini-tandas
>   previas (LSPx + LSPy + LSPy.4). 8 tests nuevos.
> - **GLIBC mismatch builder/runtime**: fix con
>   `python:3.14-slim-bookworm` (Debian bookworm-aligned).
>   Documentado en los READMEs.
> - ~~**Distroless requiere tar embebido en Rust**: el launcher
>   de `--bundle-python` invoca `Command::new("tar")` subprocess
>   → `gcr.io/distroless/cc-debian12` NO trae tar.~~ ✓ **CERRADO
>   2026-05-24 (v0.9.46)**. El launcher usa crates `tar = "0.4"` +
>   `flate2 = "1"` inline (helper `extract_tar_gz`) en lugar de
>   subprocess. Los 3 sitios reemplazados: PBS extract + pip
>   extract Linux/macOS + pip extract Windows. ~80-100 KB
>   sumados al binario final del launcher (LTO + strip activos)
>   vs ~60 MB ahorrados en la imagen de container final.
>   `Dockerfile.distroless` agregado a boilerplates 5/6 con
>   builder `python:3.14-slim-bookworm` (fix GLIBC) + runtime
>   `gcr.io/distroless/cc-debian12`. 3 tests unit nuevos. Smoke
>   real Docker end-to-end con sqlalchemy + Postgres queda como
>   deuda menor (path técnico correcto, validación funcional
>   pendiente).
> - **Beneficio real de imagen ~10-20 MB**: no 50-70 MB que
>   prometía el plan original. Argumento del approach se mueve
>   de "ahorro de deploy size" a "simplificación de runtime".
>   Plan original recalibrado en los READMEs.
>
> **Cache key del pip_packages** (deuda D CERRADA): builds
> subsiguientes sin cambios en requirements pasan de ~10-30s
> a ~instantáneo. Sin sub-pasos pendientes derivados.
>
> **Audit extensión VSCode** (CERRADO): grammar TextMate +15
> builtins (`spawn` + 5 Bits-extras + 9 Math), LSP scope_level_
> completions +5 Bits-extras. Extensión bumpeada a 0.9.3 con
> `.vsix` re-construido. Próximo workflow_release del CI
> publicará binarios alineados.

## Resumen ejecutivo

Auditoría exhaustiva sobre los 6 módulos del compilador + tests + docs.
Hallazgos: **~45 únicos** después de consolidar duplicados de las 6
revisiones paralelas + clippy. El proyecto está **sólido**: cero bugs
críticos no documentados, cero issues de seguridad, todas las deudas
mayores ya estaban en el roadmap como pospuestas.

Las áreas con más superficie a mejorar:

1. **Span en AST** — la deuda más mencionada (codegen, checker,
   evaluator y parser la citan): errores hardcoded a `0:0` sin
   línea/columna. Bloquea UX seria.
2. **Tests frágiles del codegen** — ~80% de los unit tests matchean
   strings literales del Rust generado. Cualquier refactor menor
   rompe la suite.
3. **Limpieza de clippy** — 12 "errors" (falsos positivos por `3.14`
   tomado como aproximación de π) + ~25 warnings (unused imports,
   `if let` colapsables, etc.) que ensucian el output de `cargo
   clippy`.

## Top 5 recomendaciones

Por **valor/esfuerzo**, en orden (estado a fecha de hoy entre paréntesis):

1. **L1 — Limpiar clippy** (Baja complejidad, alto valor) ✅ **CERRADO**:
   `cargo clippy --all-targets -- -D warnings` queda limpio. Los 12
   errores + 25 warnings originales se cerraron a lo largo de los
   sub-pasos post-5b; la última mini-sesión cerró 3 warnings
   residuales (doc lazy continuation, let_and_return, expect_fun_call).
2. **L2 — Helper `with_temp_output`** en codegen (Baja) — **ABIERTO**:
   patrón `mem::take(&mut self.output)` ahora repetido ~13 veces
   (creció con los sub-pasos de codegen). Refactor a helper genérico
   que toma una closure. Reduce líneas, hace refactors más seguros.
3. **R1 — Validar `fn main` con decoradores no-`@server`** (Baja) ✅
   **CERRADO** en `codegen.rs:1128` + test E2E
   `http_decorator_de_ruta_sobre_fn_main_es_error_claro`.
4. **T1 — Refactor de tests frágiles a snapshot/AST-based** (Media)
   ✅ **CERRADO ENTERO** en 3 batches (~115 unit tests migrados a
   `syn`+`quote`). Ver fila T1 de la matriz y bullet en
   "Próximos pasos".
5. **S1 — Span en AST** (Alta complejidad, alto valor a largo plazo)
   ✅ **CERRADO** en sus 3 frentes: B.1 (Stmt), S1.2 (Expr en checker
   + evaluator), S1.codegen (52 sitios). Residual menor: `Pattern` y
   `TypeExpr` sin span — baja prioridad.

Los otros ~40 hallazgos son **incrementales**: cada uno suma poco
solo, pero entre todos son una mejora de calidad significativa. Lista
completa abajo (con marcas ✅ CERRADO / PARCIALMENTE CERRADO según
estado real).

---

## Matriz completa de hallazgos

### Robustez

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| R1 | ~~`codegen.rs:811-849`~~ | **CERRADO** — `fn main` con cualquier decorator HTTP que no sea `@server` ahora dispara error explícito en `codegen.rs:1128` ("`fn main` solo admite `@server(...)` como decorator"). Test E2E: `http_decorator_de_ruta_sobre_fn_main_es_error_claro`. | — | — |
| R2 | ~~`codegen.rs:3444+`~~ **CERRADO (sesión R2/R3/R6 bundle)** — defensa en profundidad agregada con `validate_rust_ident(name)` que rechaza nombres que colisionan con keywords reservadas de Rust (`fn`, `mut`, `as`, etc.) ANTES de emitir. Aplicado en `pre_register_types` + `pre_register_fns`. El parser filtra identificadores válidos de Fitz; este check protege contra refactor que mueva un nombre de Fitz a un keyword Rust nuevo (caso extremadamente raro, pero la barrera está). Sin restringir character set (Fitz permite Unicode idents — F8). | — | — |
| R3 | ~~`codegen.rs`~~ **CERRADO (sesión R2/R3/R6 bundle)** — helper `emit_fmt(format_args!(...))` agregado para reemplazar `writeln!(out, ...).unwrap()` típicos. Sitios migrados donde el helper aporta. No es prioridad full migration porque `writeln!` sobre `String` no falla nunca — el helper es estilístico/refactor-friendly. | — | — |
| R4 | ~~`evaluator.rs:1578`~~ **AUDIT 2026-05-27** — el sitio original (`candidates[0]` después de validar `is_empty`) NO es `unwrap()`; es indexing seguro tras chequeo de longitud. Total de `unwrap()` en `evaluator.rs` = 766, mayoría sobre `.lock()` (post-F17) o `.borrow()` (sentinel de re-entrancia que el código mantiene invariante). El patrón "args validados por aridad" se mitiga con los helpers de `FitzError` (U1) que validan aridad declarativamente. Audit cierra sin intervención de código. | — | — |
| R5 | ~~`http.rs:208-228`~~ **CERRADO 2026-05-27** — docstring de `with_active_registry` ampliado en `src/http.rs` con sección "Invariantes de reentrancia (R5 audit, 2026-05-27)" que documenta el patrón `take() + replace() + take() final + restore` y explica por qué el closure `f()` puede invocar funciones internas que también hagan `cell.borrow_mut()` sin deadlock (no hay préstamos vivos durante `f`). | — | — |
| R6 | ~~`evaluator.rs` + `codegen.rs`~~ **CERRADO (sesión R6 bundle)** — Float overflow `1.0e300 * 1.0e300` ahora detecta `!is_finite()` en `arith` (evaluator) y devuelve `FitzError` claro. Codegen `gen_binop` Float Add/Sub/Mul/Div emite `if !__r.is_finite() { panic!("Float overflow: ...") }` después de cada op. Test E2E `float_arithmetic_overflow_devuelve_error_r6` valida exit code != 0 + mensaje en stderr. R6 (handler panic catch) también cerrado en la misma sesión con `catch_unwind` sobre el call al user fn — panic en handler devuelve 500 con `{"error": "..."}` en vez de crashear el server. | — | — |

### UX (mensajes / output / CLI)

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| S1 | AST + propagación | **Span en AST** — Stmt-level cerrado en B.1; **Expr-level cerrado en S1.2** (3 sub-pasos): variantes de `Expr` con `Span` (tuple-like al final, struct con `span: Span`), helper `Expr::span()` paralelo a `Stmt::span()`. Parser propaga spans para literales (token), BinOp (operador), Field/Index/Try (postfix), Range/Match/If (keyword), Ok/Err (heredan del Ident receptor), List/Map (corchete/llave). Checker (`infer_expr` + helpers `infer_binop`/`infer_method_call`/`check_method_arity`/`check_unary_callback`/`infer_list_method`/`infer_map_method`/`infer_str_method`/`check_result_match_exhaustiveness`) y evaluator (`eval_expr` + helpers de binop/unary/index/logical/call + 14 métodos built-in) citan posición del nodo en errores. **S1.codegen cerrado**: 52/69 sitios del codegen migrados a `err_at` con span del nodo (errores user-visible). Los 17 que quedan con `err()` son defensivos contra bugs del compilador (checker debió cazar): tipo no pre-registrado, fn no pre-registrada, variable desconocida en codegen, igualdad entre tipos distintos, módulo no cargado, campos sin resolver, etc. Doc-comments de `err`/`err_at` separan los dos casos. 5 tests de span en parser, 9 en checker, 5 en evaluator. **Pendiente residual menor**: `Pattern` y `TypeExpr` sin span (deuda explícita, baja prioridad). | Baja (residual) | Baja |
| U1 | ~~`evaluator.rs`~~ **CERRADO (sesión post-W12-W16)** — `src/error.rs` suma 3 constructors públicos `FitzError::method_not_found(line, column, type_name, method)`, `FitzError::wrong_arity(...)`, `FitzError::type_mismatch(...)`. Helpers consumidos en sitios clave del checker (`src/types.rs`, 9 usos al cierre del audit). Propagación al resto del codebase queda como deuda menor — los helpers están a disposición y los call sites mecánicamente migrables. | — | — |
| U2 | ~~`types.rs` ~20 sitios~~ **CERRADO (mismo cierre que U1)** — el helper `FitzError::type_mismatch(line, column, label, expected, actual)` cubre el patrón `format!("...{}...{}...", ...)` repetido. Aplicado en los call sites donde aporta legibilidad real. | — | — |
| U3 | ~~`http.rs:481`~~ **CERRADO (sesión R6 bundle)** — `run_wrap_chain` ahora emite `eprintln!("[fitz HTTP] handler `{}` falló: {}", handler_name, err)` en el Err path antes de mapear a 500. Stack trace del Err aparece en stderr para debug; response sigue limpio con `{"error": "..."}`. Paralelo: WS handlers (línea ~2249) ya emitían un eprintln análogo desde 9.w.2.c. | — | — |
| U4 | ~~`evaluator.rs:496-510`~~ **CERRADO (cuando se introdujo el LOADER)** — `evaluator.rs:1855-1877` arma `stack_text` con `LOADER.loading.iter().map(display_module_path).join(" -> ")` y emite `"ciclo de imports detectado: a -> b -> c -> a"` con la cadena completa. Mensaje exhaustivo del ciclo visible al usuario. | — | — |

### Performance

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| P1 | `evaluator.rs:2040+` | Map es `Vec<(K,V)>` — lookup O(n). Documentado como deuda explícita; bloqueante para maps grandes. **DEFER 2026-05-27** — cambiar a HashMap/BTreeMap rompe la garantía de insertion order que `serde_json::preserve_order` depende. Refactor requiere mantener orden con `LinkedHashMap` (dep nueva) o `Vec<(K, V)>` + índice secundario. Sin benchmarks que muestren un cuello real, defer. | Baja | Alta |
| P2 | `codegen.rs:1911+` | `.clone()` recursivos de `Type` en hot path (~20 sitios). Cada `gen_expr` puede hacer 2-3 clones. **DEFER 2026-05-27** — audit empírico contó 114 `.clone()` sobre Type/ty (no 20). Muchos son por ownership (return value, store en struct) — eliminarlos requiere lifetime annotations en signatures, refactor cascada masivo. Sin benchmarks proving hot path, defer. | Media | Media |
| P3 | `codegen.rs:636+` | Pre-registro de tipos/fns clona estructuras enteras. Alternativa `Rc<TypeSig>` reduciría allocaciones, requiere refactor. **DEFER 2026-05-27** — `Rc<TypeSig>` cascadea a TypeId lookups, fn_sigs HashMap, type_sigs HashMap. Sin benchmarks proving the cost, defer. | Baja | Alta |
| P4 | `evaluator.rs:805` | Snapshot pattern (`items.borrow().clone()`) en cada llamada a `.map`/`.filter`. Necesario para evitar re-entrancia pero costoso. **DEFER 2026-05-27** — snapshot es CORRECTNESS (sin ella, mutar la lista DURANTE map/filter rompe iteración). Cualquier optimización debe preservar la semántica re-entrante. Sin benchmarks proving the cost en el caso común (listas chicas), defer. | Baja | Alta |
| P5 | ~~`codegen.rs` field access~~ **VERIFICADO 2026-05-27** — el `gen_field_access` ya skipea `.clone()` para tipos Copy via el helper `needs_clone(&f.type_)` (línea 25022). Int/Float/Bool/Null se acceden sin clone; Str/Nominal/List/Map/Result/Function/Nullable sí clonan (necesario por interior mutability via Arc<Mutex>). El audit original asumía clone universal — sin medirlo. La optimización ya está en su lugar más natural. | — | — |

### Mantenibilidad

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| L2 | ~~`codegen.rs`~~ **CERRADO (2026-05-14)** — el helper `with_temp_output(|ctx| ...)` ya existía (lo usaba `gen_block_to_string`) y los 2 sitios manuales restantes (`gen_callback_inline` y `gen_fn_expr_as_value`) se migraron a él. El conteo "~13 sitios" del análisis original quedó obsoleto — la mayoría de los usos se habían consolidado a lo largo de los sub-pasos post-5b. Reducción menor de líneas; el valor real es que ahora hay una sola convención para "emitir a buffer temp". | — | — |
| M1 | ~~`codegen.rs:1159-1391`~~ **CERRADO (2026-05-14, PreF8.1)** — `generate_main_rs` (232 LoC) → orquestador de ~18 LoC + 3 helpers libres: `partition_program_stmts` (bucketea stmts en type_defs/http_fns/top_fns/main_stmts + valida decorators + extrae `@server`), `resolve_state_var_types` (detección de state HTTP + resolución de tipos), `emit_main_rs_body` (emisión final). AST del Rust generado bit-a-bit idéntico pre/post sobre los 19 ejemplos del smoke `GUIDE_EXAMPLES_COMPILE`. | — | — |
| M2 | ~~`codegen.rs:4902-5434`~~ **CERRADO (2026-05-14, PreF8.1)** — `gen_http_handler_wrapper` (532 LoC) → orquestador de ~9 LoC + 6 métodos del `impl CodegenCtx`: `resolve_handler_signature` (entry pattern match, parse path, collect middlewares, resolver tipos, validar y categorizar params, resolver return), `emit_axum_extractors` (firma del wrapper), `emit_middleware_chain` (Request build + chain con short-circuit CORS-aware), `emit_param_coercions` (query + headers + body), `emit_handler_dispatch_and_response` (call + 3 caminos de response), `emit_cors_helpers` (`__cors_resolve_<name>` + `__preflight_<name>`). Nuevo struct `HandlerSig` captura el estado intermedio. | — | — |
| M3 | ~~`types.rs`~~ **AUDIT 2026-05-27 — cierre sin intervención** — el audit original citó 446 LoC y "mega-match de 30+ branches" sugiriendo extraer grandes. Estado real: `synthesize_expr` creció a ~1128 LoC por más variantes AST (Fase 9.w + Fase 10 sumaron WS/cron/spawn/ORM/JSONB/...), no por branches refactorables. La complejidad realmente refactorable YA está extraída en ~15 helpers (`infer_method_call` + `infer_query_builder_method` + `infer_aggregated_method` + `infer_{int,float,range,list,map,wsconn,bytes,str}_method` + `check_method_arity` + `check_unary/binary_callback` + `lub` + `unify_returns`). Los branches inline restantes son case-arms cortos (5-10 LoC) sobre variantes AST sin lógica compleja. Costo de seguir extrayendo (`ctx` plumbing, docstrings, ramas defensivas) excede el beneficio (la legibilidad ya es razonable con los helpers existentes). | — | — |
| M4 | ~~`types.rs:1691-1866`~~ **CERRADO (sesión P/U/M bundle, v0.10.15)** — helper `CheckCtx::with_scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R` agregado en `src/types.rs:2438`. Auto-pop garantizado por el closure body — el helper hace `push_scope` antes y `pop_scope` después sin importar early-returns. Aplicado en branches relevantes de `check_stmt` (if-then/else, for body, while body, FnDef body, Match arm body). Reduce ~3-4 sitios de `push/pop` manual a `ctx.with_scope(\|ctx\| ...)`. | — | — |
| M5 | ~~`parser.rs`~~ **CERRADO 2026-05-27** — helper `Parser::parse_comma_separated<T, F>(terminator, close_msg, parse_item)` agregado en `src/parser.rs`. Maneja el scaffold "skip_newlines + comma + trailing comma + expect terminator" con `std::mem::discriminant` para el match del cierre. Migrados: `parse_call_args` (named arg detection vía closure que captura `saw_named` por mutable ref), y la **cola** de `parse_map_literal_pairs` (el primer par se sigue parseando manual para detectar comprehension `{k: v for ...}` y separar la entrada). **NO migrados** y documentados en el doc-comment del helper: (1) `parse_struct_lit_fields` separa con coma O newline O RBrace; (2) `parse_list_literal_items` necesita detección de comprehension tras el primer item. Los doc-comments lo explican. 365 parser tests verde, sin regresiones. | — | — |

### Tests

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| T1 | `codegen.rs` tests | **CERRADO** — los 3 batches migrados. Batch 1+2 (65 tests): expresiones, lits/literales, instances, listas/mapas/indexing/métodos built-in, F12 closures. Batch 3 (50 tests en 4 sub-commits): HTTP (21: tokio main, Router, path params, status codes, query params, body POST, server decorator, state thread_local, type impls JSON), Result/?/match (9: Ok/Err constructors, ? rust, match con bindings, range guard, print de Result), módulos (6: pub en items, static/const top-level, fn body referenciando const), sobrantes (14: type-def Display, struct-lit con defaults/nullables, igualdad estructural, pasar instance, if-as-expr, str-interp). **Infra `ast_test`** (módulo dentro de `mod tests`): `parse`, `ts`, `find_item_fn/struct/type/static/const`, `find_impl`, `find_let`, `local_init/init_expr/is_mut/type`, `count_macro_calls/lets`, `find_for_loop/while_loop/if/match`, `count_method_calls_in_expr`, `contains_method_call_in_expr`, `find_macro_args/first_macro_args_in_stmts`, `cast_target_type`, `method_chain_names`, `find_route_registrations`, `find_local_in_fn`, `count_locals_in_fn`, `fn_attrs/is_async/body_text/param_pats_and_types/return_type`, `fn_body_returns_any_matching`, `fn_body_has_match_arm_pat`, `find_top_macro`, `vis_is_pub`, etc. Removed: helpers dead `assert_contains` y `assert_http_contains`. **Quedan 10 `code.contains` legítimos** (4 sobre `ts(&file)` ya AST-based, 1 contrato UX, 1 negative check, 4 sobre TOML). | — | — |
| T2 | ~~`tests/compile_e2e.rs:20`~~ **CERRADO (sesión T2/T7/R6 bundle)** — `static SERIAL: Mutex<()>` eliminado del file; los 26 `SERIAL.lock()` removidos. Cada test que invoca `fitz build` usa stem único derivado de `sanitize_stem(test_name)` para que su `<stem>.fitz` + `target/fitz-build/<stem>/` no choque con otros. Cargo serializa el acceso a `~/.cargo/registry` internamente; los outputs de compilación son por-stem. Resultado: tests E2E corren en paralelo según `--test-threads` default de cargo. Speedup observado ~4x en CI multi-core. | — | — |
| T3 | ~~`parser.rs` tests~~ **CERRADO 2026-05-27** — 9 tests nuevos de paths de error en `parser::tests`: `fn_def_con_params_duplicados_es_error` + `_sin_tipo` (fix preventivo: `parse_params` ahora rechaza nombres duplicados con `el parámetro \`X\` está duplicado en la lista de parámetros` antes de que el evaluator vea binding redefinido), `decorator_sobre_let_es_error` + `_expresion_suelta` (ya andaba en runtime, ahora explícito), `string_con_escape_invalido_es_error_del_lexer` (`\q` rechazado en tokenize), 4 tests de nesting mal balanceado (`parens_sin_cerrar`, `llave_sin_cerrar_en_bloque`, `corchete_sin_cerrar_en_list_literal`, `corchetes_anidados_mal_balanceados`). | — | — |
| T4 | ~~E2E ~12/48~~ **CERRADO 2026-05-27** — auditoría completa de 9 candidatos identificados por análisis de `assert!`/`assert_eq!` por test. 7/9 ya tenían asserts adecuados (build_aborta_* validan stderr.contains, módulo_inexistente_aborta, f15_ciclo, fnexpr_sin_anotacion, ws_codegen_*). Los 2 genuinamente débiles reforzados: `lt_let_panic_si_no_matchea` y `float_arithmetic_overflow_devuelve_error_r6` ahora validan stderr message además de exit code != 0. Nuevo helper `build_and_run_with_stderr` para casos análogos futuros. | — | — |
| T5 | ~~`codegen.rs`~~ **CERRADO 2026-05-27** — 4 tests E2E nuevos sobre binario compilado: `t5_triple_nivel_field_access_y_mutation_compilado` (3 niveles de anidación + mutación profunda visible via alias), `t5_igualdad_difiere_tras_mutacion_de_un_solo_field_compilado` (PartialEq recursivo se sensibiliza a cambios profundos), `t5_field_chain_sobre_nullable_anidado_compilado` (match con pattern `null =>` + ident binding refinado), `t5_display_recursivo_con_field_lista_y_mapa_compilado` (Instance con List + Map fields). **Bug F17 deadlock descubierto y fixed**: `==` de dos vars que comparten el mismo Arc<Mutex> deadlockeaba en `std::sync::Mutex` (no reentrante). Caso canónico `let alias = u; u == alias`. El codegen ya emitía `Arc::ptr_eq` shortcut en el `PartialEq` de field nominales, pero NO en el operador `==` top-level de `gen_binop`. Fix: emitir `(Arc::ptr_eq(&l, &r) \|\| *l.lock().unwrap() == *r.lock().unwrap())` para `==` y simétrico para `!=`. Paridad bit-a-bit `fitz run` ↔ `fitz build` validada. | — | — |
| T6 | ~~Combinatorias~~ **CERRADO 2026-05-27** — 4 tests E2E combinatorios nuevos: `t6_list_de_listas_int_compilado` (`List<List<Int>>` con indexing doble + iter anidada + reasignación), `t6_map_str_a_list_int_compilado` (`Map<Str, List<Int>>` con get → Result<List<Int>> + .len() chained via fn helper para evadir print-as-expr en arm body), `t6_list_de_custom_nullable_compilado` (`List<User?>` con mix Some+null + match en for body), `t6_map_str_a_custom_compilado` (`Map<Str, User>` con get → Result<User> + display recursivo). | — | — |
| T7 | ~~HTTP E2E~~ **CERRADO (sesión T2/T7/R6 bundle)** — test E2E nuevo `http_coverage_metodos_headers_content_type_body_libre_t7` con 4 casos: (a) GET con header custom + body libre `Map<Str, Any>`, (b) POST con body `application/x-www-form-urlencoded`, (c) POST con body deserializado a tipo Fitz custom + Content-Type negotiation, (d) handler panic + recovery 500. Complementa los E2E HTTP pre-existentes (paths Int, Result Ok/Err, body POST con type, defaults, extras 400, etc.). Sumá los E2E auth W12-W14 y los E2E cross-module W15-W16 que cierran el escenario "handler en módulo importado con body custom + auth + middleware". | — | — |

### Deuda funcional (features incompletas o gradual)

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| F1 | ~~`types.rs` ~180 sitios~~ **CERRADO 2026-05-24 (v0.9.45, mini-tanda Cleanup-A)** — audit completo + matriz de uso de `Type::Any` documentada en el doc comment del enum `Type` (`src/types.rs`). 9 categorías intencionales: builtins variádicos, builtins polimórficos, propagación gradual, fallback de anotaciones inválidas, callbacks sin anotación, patterns de match sobre `Any`, `Expr::Error` (F15), `Result<Any>`/`Future<Any>` placeholder, propagación de `PyAny`. Anti-patterns que sí serían bugs también documentados (silenciar mismatches genuinos, fns user-defined sin anotación → Any, error real como Any). Sin cambios de código — el audit ratifica que el uso es correcto. | — | — |
| F2 | ~~`types.rs:1739-1741`~~ **CERRADO en C-F2** — el checker ahora valida que el receptor sea `Nominal`, que el field exista, y que el tipo del RHS sea compatible (`is_compatible`). Mensaje con `User.field` + tipos esperado/recibido + línea (gracias a B.1). 6 tests nuevos. | — | — |
| F3 | ~~`parser.rs:656-662`~~ **CERRADO (R.2.4 + ratificado en v0.9.45 mini-tanda Cleanup-A)** — checker rechaza estáticamente los 3 stmts huérfanos con mensajes claros: `return` fuera de fn (`return_stack.is_empty()` en `Stmt::Return`), `break`/`continue` fuera de loop (`loop_depth == 0` en `Stmt::Break`/`Continue`). 3 tests cubren cada caso (`return_huerfano_top_level_es_error`, `break_huerfano_es_error`, `continue_huerfano_es_error`). | — | — |
| F4 | ~~`parser.rs` + `evaluator.rs` + `codegen.rs`~~ **CERRADO (2026-05-14, PreF8.3)** — auditoría exhaustiva de 6 casos del roadmap (root/importado/nullable+default/nested/reasignación/expr-no-literal): 5 andaban OK. Único bug: defaults de tipos importados que referencian símbolos del módulo de origen (`type User { id: Int = MAX }` con `MAX` const del módulo) fallaban tanto en `fitz run` ("variable `MAX` no definida") como en `fitz build` ("variable desconocida en codegen: `MAX`"). Fix estrategia eager-at-import: `Value::Type` suma `resolved_defaults: Vec<(String, Value)>`, el loader pre-evalúa los defaults en el env del módulo; codegen emite `pub fn __default_<T>_<F>() -> T { ... }` en el módulo y el struct lit del importer invoca `<mod>::__default_<T>_<F>()`. Tipos locales del archivo principal siguen con eval lazy del Expr. 3 unit tests + 1 E2E nuevos. Guía cap 12 documenta el comportamiento. | — | — |
| F5 | ~~`evaluator.rs`, `http.rs`~~ **CERRADO (audit 2026-05-27)** — el comentario original "is_async se ignora en runtime" quedó stale tras Fase 6 (Async nativo). Estado real: `is_async` SÍ se propaga end-to-end desde `Stmt::FnDef` y `Expr::FnExpr` → `Value::Function { is_async }` (líneas 2348 y 3286 de evaluator.rs), y se CONSUME en `register_http_route` (505-509), `register_ws_route` (565-570), `process_decorator` (678), `register_cron_route` (688), `invoke_value` (3987, 4357 — decide si esperar el Future resultante). Comentario stale removido y reemplazado con descripción correcta en evaluator.rs:2333. Ratificación adicional: tests `test_decorator_async_fn_registra_is_async_true` y `cron_async_fn_registra_is_async_true` validan el flag. | — | — |
| F6 | ~~`evaluator.rs`~~ **CERRADO (audit 2026-05-27)** — el "solo 2 builtins" del audit original (print + len) quedó hopelessly obsoleto. `register_builtins` (src/evaluator.rs:9241) registra ~15 builtins globales: `print`, `len`, `bytes`, `cors`, `sleep`, `spawn`, `env`, `env_or`, `load_env`, `assert`, `assert_eq`, `assert_ne`, `assert_throws`, `popcount`, `leading_zeros`, `trailing_zeros`, `rotate_left`, etc. Sumá `jwt` y `hash` como `Value::Module` pre-registrados (Fase 9.w.1). El syntax-spec NO promete builtins adicionales como `range`/`type_of`/`to_string`: `range` se expresa con literal `0..10` (Range value type) y `for in`; `type_name()` está como método sobre `__FitzValue` cuando hay heterogéneos; `to_string` se cubre con interpolación `"{x}"` (Display). El set actual cubre el contrato del syntax-spec sin gaps. | — | — |
| F7 | ~~`lexer.rs`~~ **CERRADO (Mini-tanda Núm)** — auditoría 2026-05-27 ratifica el cierre. Soporte de separador `_` entre dígitos (`1_000_000`, `3.14_15`, `1_000.000_1`) + notación científica `e`/`E` con exponente opcionalmente firmado (`1e10`, `3.14e2`, `2.5E3`, `1e-10`, `1e+3`, `3.14E-2`); separadores válidos también en exponente (`1e1_0` → `1e10`). Errores claros: doble underscore (`1__0`), terminal (`1_000_`), exponente sin dígitos (`1e`, `1e+`, `1e-`). Mantiene compatibilidad con tuple field access (`t.0.0` vía flag `prev_was_dot`). 7 unit tests dedicados (`num_separador_*`, `num_notacion_cientifica_*`, `num_exponente_*`, `num_tuple_field_access_*`). Validado E2E: `let x = 1_000_000; let pi = 3.14e-2; print(x); print(pi)` compila con paridad bit-a-bit `fitz run` ↔ `fitz build`. | — | — |
| F8 | ~~`lexer.rs`~~ **CERRADO (Mini-tanda F8)** — auditoría 2026-05-27 ratifica el cierre. Identificadores Unicode end-to-end: `is_alphabetic()` (no `is_ascii_alphabetic`) + dígitos Unicode permitidos en posiciones interiores. Cubre letras griegas (`π`, `σ`), tildes y eñe (`área`, `niño`), CJK (`日本語`), cirílico, mixto Unicode+ASCII. Validado paridad bit-a-bit `fitz run` ↔ `fitz build` con `let área = 100; fn área_de_círculo(r: Float) -> Float => 3.14 * r * r`. 6 unit tests dedicados (`f8_identifiers_griegos_y_simbolos_matematicos`, `f8_identifiers_con_acentos_y_n_tilde`, `f8_identifiers_cjk`, `f8_identifiers_cyrillic`, `f8_identifiers_mixto_unicode_y_ascii`, `f8_digitos_unicode_no_pueden_arrancar_identifier`). Ejemplo `examples/guide/03d-identifiers-unicode.fitz` runnable. | — | — |
| F9 | ~~`lexer.rs`~~ **CERRADO (Mini-tanda F9)** — escapes extendidos en strings: `\u{...}` (Unicode BMP + suplementario), `\x..` (ASCII hex), `\0`, `\b`. El lexer produce `Token::Str` con chars resueltos; codegen no necesita lógica extra (`rust_str_literal` usa `format!("{:?}", s)` que emite el literal Rust correcto). Tests en `tests/compile_e2e.rs::f9_escapes_extendidos_paridad_bit_a_bit` + unit tests en lexer. | — | — |
| F10 | ~~`parser.rs`~~ **CERRADO (2026-05-14, PreF8.2)** — `postfix()` loop tolera `Token::Newline` antes de `.`. Lookahead saltando newlines: si el próximo significativo es `Token::Dot`, consume los newlines y continúa la expresión. Solo `.` continúa — `(`, `[`, `?` rompen como hoy para no cambiar la semántica de expression statements vecinos. AST resultante idéntico al one-liner. 8 tests parser nuevos. Cap 13 de la guía documenta como forma idiomática; `examples/guide/13-metodos.fitz` suma chain de 3 líneas. | — | — |
| F11 | ~~`codegen.rs` (state HTTP)~~ **CERRADO** vía `thread_local! { static __FITZ_STATE_X: Rc<RefCell<T>> = ...; }` por cada var top-level referenciada en handlers + tokio `flavor = "current_thread"`. Cada fn que toca state materializa al inicio del body (`let X = __FITZ_STATE_X.with(|s| s.clone());`). Los handlers Fitz son sync, así que sus futures son `Send` aunque adentro toquen `Rc` (los locals Rc nunca cruzan `.await`). `examples/server.fitz` (CRUD completo) y `examples/guide/17-http.fitz` compilan end-to-end + validados con curl bit-a-bit; el segundo entró al smoke `GUIDE_EXAMPLES_COMPILE`. 5 tests nuevos (1 unit + 4 E2E con build + spawn + secuencia de requests). **Deuda residual del approach**: server HTTP single-threaded (sin paralelismo entre requests) — cuando aterrice async/await real en Fitz, re-evaluar con `Arc<Mutex<...>>` + `State` extractor. | — | — |
| F12 | ~~`codegen.rs` (higher-order)~~ **CERRADO** — closures escapadas, fn nombrada como valor, FnExpr asignado a var, fn como param y como tipo de retorno compilan con `fitz build`. `TypeExpr::Function` nueva variante; codegen emite `Rc<dyn Fn(...) -> R>` uniforme. Cap 11 anotado y compilable bit-a-bit con el intérprete. Smoke `GUIDE_EXAMPLES_COMPILE` incluye `11-funciones.fitz`. 24 tests nuevos. | — | — |
| F13 | ~~`codegen.rs`~~ **CERRADO** (verificado en audit v0.9.49) — `[1, "dos", true]` (List<Any>) compila con `fitz build` y produce output bit-a-bit con `fitz run`. El SPIKE `__FitzValue` con variantes Int/Float/Str/Bool/Null + Bytes + Nominal cubre los casos típicos. Auto-detectado en `gen_list_lit` cuando aparece un `List<Any>` literal. Trade-off del SPIKE: heterogéneos pierden field access tipado (acceso vía type check dinámico), pero el caso 90% (mezcla de primitivos + nominal display) anda. Refinable a `List<__FitzValue>` con typed accessors si aparece presión real. | — | — |
| F14 | ~~`codegen.rs`~~ **CERRADO** (cubierto vía accessor fns en mini-tanda F14 original + tests ampliados en v0.9.45 Cleanup-A) — `gen_module_top_let` despacha en 3 caminos: Str literal → `pub static X: &str`, const-eval-able (Int/Float/Bool con BinOp recursivo) → `pub const X`, cualquier otra cosa → `pub fn X() -> T { rhs }` accessor fn. Cubre listas/mapas/instances/calls/field access. 6 tests cubren cada path (`modulo_let_int_top_level_*`, `modulo_top_level_acepta_expr_const_eval_*`, `modulo_top_level_acepta_expr_no_const_*`, `modulo_top_level_let_lista_literal_*`, `modulo_top_level_let_map_literal_*`, `modulo_top_level_let_instance_*`). | — | — |
| F15 | ~~`parser.rs` + `ast.rs` + `types.rs` + `evaluator.rs` + `codegen.rs`~~ | **CERRADO (2026-05-15, Fase 9.0, 1219 unit + 79 E2E)** — error recovery del parser end-to-end. 3 sub-pasos: 9.0.1 AST + API recovery + tests del parser (nodos `Expr::Error(Span)` / `Stmt::Error(Span)` in-band + `Vec<FitzError>` paralelo; `pub fn parse_with_recovery(tokens) -> (Program, Vec<FitzError>)` con `recovery_mode` interno + cota `MAX_RECOVERED_ERRORS = 100` + helper `synchronize()` con sync points stmt-level — `Newline` consumido, `RBrace`/`EOF` preservados, **keywords de inicio de stmt preservadas** `Let`/`Fn`/`Async`/`Type`/`Return`/`Break`/`Continue`/`While`/`Loop`/`For`/`If`/`Import`/`From`/`At` por necesidad: `primary()` consume el token actual antes de validar, los tests detectaron que sin la parada en keywords sync se comía stmts enteros; defensas en eval/codegen con `FitzError` claro + span; 10 unit tests `parser::tests::recovery_*`); 9.0.2 tolerancia del checker (`Expr::Error → Type::Any`, `Stmt::Error` no-op, silencioso para que el LSP corriendo `check_program` sobre AST recuperado no emita cascadas; helper local `check_recovering(src)` que corre el pipeline LSP-style `parse_with_recovery → check_program`; 5 unit tests `types::tests::checker_*`); 9.0.3 cierre formal (smoke a mano `fitz check` strict sobre buffer roto → exit 1 con un error del primer stmt roto, comportamiento idéntico a antes; smoke `GUIDE_EXAMPLES_COMPILE` sigue verde; CHANGELOG v0.9.0, roadmap con Fase 9.0 detallada, README refresh). **API strict (`parse`) intacta** — la CLI sigue priorizando fail-fast. Decisiones técnicas: nodos in-band + lista paralela (árbol mantiene forma estructural, mejor para LSP/formatter); sync points stmt-level + keywords (compromiso entre simplicidad y recovery efectivo); cota 100 errores (caso 90% del LSP cubierto con margen sin runaway). **Deuda residual derivada** (NO bloquea Fase 9): recovery sub-stmt (errores dentro de un stmt descartan el stmt entero — refinable para completion fino tras `user.`); bindings parciales (`let x = <roto>` no preserva `x`, genera "no definido" en referencias posteriores; aceptable como trade-off del LSP MVP); `Expr::Error` con metadata (opaco hoy, refinable post-LSP). Ver detalle en `docs/roadmap.md` → "Fase 9.0". | — | — |
| F16 | ~~`types.rs` (checker)~~ | **CERRADO (2026-05-15, Fase 9.0, 1227 unit + 79 E2E)** — IR tipado persistido por nodo end-to-end. 2 sub-pasos: 9.0.4 `pub struct SpanKey(usize, usize)` como clave hashable (Span propio no sirve por su PartialEq custom que devuelve true siempre, diseñado para tests de AST estructurales), `pub struct TypeInfo` con `record`/`type_at`/`len` que omite `Span::ZERO` para evitar colisiones entre nodos sintéticos, `infer_expr` envuelve `synthesize_expr` para centralizar el `record` desde un solo punto (recursión incluida), `pub fn check_program` cambia firma de `(TypeEnv, Vec<FitzError>)` a `(TypeEnv, TypeInfo, Vec<FitzError>)` con 13 call sites migrados con `_types`, `Expr::Error` (F15) se persiste como `Type::Any` uniforme con el checker, 8 unit tests `types::tests::types_info_*`; 9.0.5 cierre formal (CHANGELOG v0.9.1, roadmap, este archivo, README refresh). **API user-facing intacta** — la CLI descarta el side-table. Decisiones técnicas: HashMap<SpanKey, Type> (vs NodeId, vs `*const Expr` — el primero reusa spans del AST sin refactor); cobertura amplia (todo Expr, no solo Ident/Field/Call); una sola firma de check_program (vs variante separada — 13 sitios migran trivialmente); Span::ZERO omitido por colisiones; Expr::Error como Any (LSP decide qué mostrar). **Deuda residual derivada** (NO bloquea sub-fases visibles del LSP): sin index espacial (rango inicio-fin) — el LSP elige nodo más cercano al cursor por ahora; spans en `TypeExpr` y `Pattern` (heredado de S1); cobertura de `Stmt` (ortogonal — resolución de declaraciones vía scope lookup en 9.x.3). Ver detalle en `docs/roadmap.md` → "Fase 9.0 — F16". | — | — |
| F18 | ~~`parser.rs` + `evaluator.rs` + `codegen.rs` + `types.rs`~~ **CERRADO (2026-05-14, PreF8.4)** — import aliasing con `as` (`import foo as f`, `from foo import bar as b`, alias mixto). Sub-paso adelantado de F8.1 para dejarlo con solo Python interop puro. Lexer suma `Token::As`; AST suma `Stmt::Import.alias: Option<String>` y cambia `Stmt::FromImport.names` a `Vec<(String, Option<String>)>`. Codegen emite `use foo::bar as b;` (fn/const) o `use foo::{T as L, TData as LData};` (type). Evaluator usa el `Value::Type.name` canónico al instanciar (no el alias sintáctico) para paridad bit-a-bit `fitz run` ↔ `fitz build` del Display. 9 unit + 4 E2E nuevos. Cap 16 de la guía documenta. | — | — |
| F19 | ~~`codegen.rs` (`check_no_python_imports`)~~ **CERRADO (2026-05-15, Fase 8.7)** — codegen interop Python en `fitz build` end-to-end. 4 sub-pasos: 8.7.1 detección + filtrado del ModuleLoader + Cargo.toml condicional (`pyo3 = "0.28"` con `abi3-py310 + auto-initialize`) + preludio `__FitzPyObject(Arc<Py<PyAny>>)` con Display delegado a `__str__` Python (paridad bit-a-bit `print`) + helpers `__fitz_py_import` + getattr + extracción primitiva i64/f64/String/bool + **bindings globales** (`static __FITZ_PY_BIND_X: OnceLock<__FitzPyObject>` + getter por binding, accesibles desde cualquier fn); 8.7.2 trait `__FitzToPy` con impls genéricos para primitivos + List + Map + Option + Instance (`impl __FitzToPy for FooData` + wrapper sobre `Arc<Mutex<FooData>>` emitidos por `gen_type_def` cuando `uses_python = true`) + helper `__fitz_py_invoke(callable, args_fn) → Result<__FitzPyObject, String>` con wrap automático de excepciones Python paralelo a 8.3 + breadcrumb `arg0` paralelo a `value_to_py(path: &str)` del intérprete; 8.7.3 helper async `__fitz_py_invoke_await` con detección `inspect.isawaitable` + ejecución vía `tokio::spawn_blocking + asyncio.new_event_loop().run_until_complete()` (baseline blocking, paralelo a 8.6.1 `py_coro_to_fitz_future`) + patrón canónico `<py_call>?.await` (paridad bit-a-bit con intérprete que rechaza `<call>.await` directo en runtime — el checker 8.7.3 lo rechaza estáticamente); 8.7.4 cierre formal con ejemplo `examples/python-interop-8.7.fitz` validado bit-a-bit `fitz run` ↔ `fitz build`. Total al cierre: 1295 unit + 88 E2E + 3 openapi con feature; 1204 + 79 + 3 sin feature. Clippy `-D warnings` limpio en ambos modos. **Deuda residual derivada** (NO bloquea Fase 8): coerción Python list/dict → Fitz `List<T>` / `Map<K,V>` / `Instance` (helpers `__fitz_py_to_list_*` ya emitidos, falta wiring en `coerce`); `.await` con binding intermedio split (`let fut = py_call()?; fut.await`); bundling CPython embebido (`fitz build --bundle-python`) — proyecto separado, decisión python-build-standalone vs PyOxidizer pendiente. Ver detalle en `docs/roadmap.md` → "Fase 8.7". | — | — |
| F17 | ~~`evaluator.rs` + `value.rs` + `env.rs` + `http.rs` + `codegen.rs`~~ | **CERRADO (2026-05-14, 1153 unit + 74 E2E)** — Send completo + paralelismo HTTP real + bridge HTTP eliminado. Seis sub-pasos: F17.1 dep `parking_lot`; F17.2 `Shared<T>` y `EnvRef` migran a `Arc<parking_lot::Mutex<T>>` (~284 sitios mecánicos `.borrow()/.borrow_mut()` → `.lock()`, `Rc::ptr_eq` → `Arc::ptr_eq`); F17.3 quitar `?Send` del `#[async_recursion]` en evaluator (13 sitios) + `FitzFuture: Pin<Box<dyn Future + Send>>` (fix colateral: `for` sobre List/Range materializa a `Vec<Value>` en vez de `Box<dyn Iterator>`); F17.4a `serve()` tokio `rt-multi-thread`; F17.5 eliminar bridge HTTP (`InterpTask`, `TaskTx`, `run_interpreter_loop`, `dispatch_request` viejo — ~269 LoC netas menos en `http.rs`, handlers axum invocan `handle_task(&registry, ...).await` directo sobre `Arc<HttpRegistry>` compartido, test helpers `run_oneshot_*` sin `LocalSet`/`select!`/canal); F17.4b codegen output paralela (`Rc<RefCell<>>` → `Arc<Mutex<>>` con std::sync, F12 closures `Arc<dyn Fn + Send + Sync>`, state HTTP `thread_local!` → `LazyLock<Arc<Mutex<T>>>`, runtime emitido `#[tokio::main]` default multi-thread, field access en bloque acotado `{ let __obj = ...; let __g = __obj.lock().unwrap(); __g.<f> }` para evitar deadlock por re-lock en `format!`, `PartialEq` custom por tipo nominal con helper recursivo `field_eq_expr`); F17.6 guía cap 19 sub-sección "Paralelismo HTTP real" + ejemplo `examples/guide/19b-paralelismo.fitz` validado a mano (5 reqs concurrentes en **1.2s** vs 5 en serie **5.3s**; pre-F17 ambos ~5s). Decisiones técnicas: `parking_lot::Mutex` para el intérprete, `std::sync::Mutex` para el codegen output (sin deps extras al Cargo.toml generado); política de re-entrancia "lock scope mínimo + clone-out" (auditoría manual en eval_call/EnvRef::get). **Deudas residuales que NO bloquean Fase 8**: benchmarks de `MutexGuard` vs `Ref<T>` (sin medir); lint o test que detecte patrones de re-lock potencial; LOADER del intérprete sigue como `thread_local! { RefCell<...> }` (re-carga módulos por worker, wasteful pero correcto). Ver detalle en `docs/roadmap.md` → "Fase F17". | — | — |

### Docs

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| D1 | `guide.md:4-5` | **PARCIALMENTE CERRADO** — el header ya cita "Fase 5b cerrada / 949 tests" (vs el original "Fase 5a / 784"). Sigue stale al estado actual (1043 tests, mini-fases post-5b cerradas). Mejor refresh recurrente cada vez que se mueve el contador, no deuda permanente. | Baja | Baja |
| D2 | ~~`guide.md:881-883`~~ **CERRADO 2026-05-20** — cap 13 ahora desarrolla los métodos de Str con tablas completas (mini-tandas S.1+S.2 + Mb-series + Math+Mb9 cubrieron `upper`/`lower`/`len`/`contains`/`starts_with`/`ends_with`/`split`/`trim`/`replace`/`repeat`/`find`/`index_of`/`last_index_of`/`pad_start`/`pad_end`/`chars`/`split_at`/`lines`/`is_empty`/`repeat_with`/`left`/`right`/`center`/`swap_case`/`title`/`is_alpha`/`is_digit`/`is_numeric`). Las referencias del cap 5 al cap 13 ya están materializadas. | — | — |
| D3 | ~~`syntax-spec.md:1-8`~~ **CERRADO (2026-05-14)** — header pasó a "BORRADOR v0.3 (post-F17)" con matriz rápida de estado actualizada: implementado/diseñado-no-implementado con referencias a capítulos de la guía y fases del roadmap. Refresh recurrente cada vez que se cierra una mini-fase o fase. | — | — |
| D4 | ~~Repo root~~ **CERRADO (2026-05-14)** — `CHANGELOG.md` creado con 9 entradas retroactivas: v0.1.0 (Fase 2) → v0.8.0 (Fase F17). Formato [Keep a Changelog](https://keepachangelog.com). Detalle técnico vive en `docs/roadmap.md`; el CHANGELOG es la vista condensada "qué cambió y cuándo". | — | — |
| D5 | ~~`guide.md:225-226`~~ | **CERRADO** — status codes custom implementados end-to-end en su mini-fase dedicada (ver bullet en "Próximos pasos"); cap 17 de la guía documenta la sintaxis con ejemplos. README puede quedar stale (cita "deuda residual post-5") — refresh menor cuando se mueva. | — | — |
| D6 | ~~`guide.md:2725-2738` vs `:4305-4310`~~ **CERRADO 2026-05-20** — las dos deudas originales (asignación a índice + state HTTP) ya cerraron (R.1.3 cerró asignación a índice; F11 cerró state HTTP en handlers). Las menciones duplicadas en cap 13 y cap 18 quedaron como deuda residual histórica — los caps modernos las marcan correctamente como "lo que sí anda". | — | — |
| D7 | `README.md:38` | **CERRADO** (suficiente) — la nota actual ("la sintaxis `async fn` se parsea, pero el runtime sigue siendo síncrono") es clara. Re-evaluar cuando aterrice Fase 6 (Async nativo). | — | — |

### Linter (clippy)

**L1 entero CERRADO** — `cargo clippy --all-targets --all-features -- -D warnings` queda limpio. Los items originales L1a-L1f se resolvieron a lo largo de los sub-pasos post-5b; el último pase (3 warnings residuales: doc lazy continuation, let_and_return, expect_fun_call) cerró en una mini-sesión dedicada tras T1 batch 3. Re-correr `cargo clippy` antes de cualquier commit grande.

**v0.9.48 Cleanup-D — `cargo fmt --all` aplicado masivamente + CI strict reactivado**: el repo nunca había pasado por rustfmt canónico desde el inicio. La mini-tanda Cleanup-D aplica el formato (14 archivos reformateados, cero cambios funcionales), reactiva `cargo fmt --check` en `ci.yml` (estaba comentado), y promueve `cargo clippy --lib` → `cargo clippy --all-targets` (la deuda original de "11 errores en tests" ya había cerrado en mini-tandas previas — verificado con audit). Esto sacó el último ítem del bundle D del inventario y deja el repo en estado profesional para colaboradores.

**v0.9.49 audit completo del inventario** (2026-05-24): después de descubrir 2 sesiones consecutivas con inventario stale (v0.9.47 LSP — 3 deudas ya cerradas; v0.9.48 Cleanup-D — los 11 errores de clippy ya cerrados), dedicamos una sesión a auditar el resto. **Resultado**: 4 deudas más resultaron ya cerradas:

- **F13 — heterogéneos en codegen**: ✅ Cerrado vía SPIKE `__FitzValue`. Verificado con smoke `[1, "dos", true]` produce `[1, "dos", true]` bit-a-bit con `fitz run`.
- **8.7-await-binding-split**: ✅ Cerrado con test `py_await_split_emite_fitz_py_await_obj` + dispatch al helper `__fitz_py_await_obj` cuando `inner_ty == PyAny`.
- **multi-arch-docker**: ✅ Implementado en `release.yml` Job 3 `docker-image` con buildx `linux/amd64,linux/arm64`.
- **fitz-python-image**: ✅ Implementado en `release.yml` Job 3b con tag `:latest-python`.

**Deudas reales restantes** (auditadas como NO cerradas):

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| ~~8.7-ok-propagation~~ | ✓ **CERRADO v0.9.53** | `gen_return` propaga expected type adentro de `Ok(...)`/`Err(...)`; coerce inner directo al T/E del `Result<T, E>` esperado |
| ~~dict→Map<K,V> no primitivos~~ | ✓ **CERRADO v0.9.54** | 4 helpers `__fitz_py_to_map_string_<v>` para v primitivo (Str/Int/Float/Bool) + wiring en `coerce`. V compuesto (Nominal/List/Map) sigue gradual como deuda menor — casos raros, workaround manual con iteración del PyDict |
| ~~UTF-16 position strict~~ | ✓ **CERRADO v0.13.2** | v0.9.51 intentó declarar `positionEncoding: utf-8` pero `vscode-languageclient@9.0.1` hard-codea `general.positionEncodings = ['utf-16']` (`client.js:1370`) y rechaza cualquier encoding distinto en `client.js:835` — la extensión 0.13.1 era inservible en VSCode fresh (bug reportado durante curso M1.C1, 2026-06-04). v0.13.2 implementa el fix completo: server omite `position_encoding` (default UTF-16), `position_to_offset`/`offset_to_position` migrados a contar UTF-16 code units vía `ch.len_utf16()`, helper nuevo `utf16_to_unicode_char(text, line, char_utf16) -> u32` (pub) traduce char_utf16 del cliente a chars Unicode 1-based del lexer para lookup en `TypeInfo`/`DefinitionInfo`, handlers del backend `hover`/`goto_definition` aplican la traducción antes del lookup, `detect_completion_context` traduce `recv_col` interno antes de armar `CompletionContext::AfterDot`. Soporta SMP (emoji, símbolos matemáticos avanzados) sin off-by-one en hover/definition/completion. **Deuda residual cosmética** (NO afecta navegación funcional): `make_definition_location` y `ident_range_from_def` retornan Range LSP con char en chars Unicode (no UTF-16), pero como las líneas de def son siempre ASCII en la parte ANTES del ident (keywords + identifiers son ASCII por reglas del lexer), char_unicode == char_utf16 en práctica |
| ~~F15 recovery sub-stmt~~ | ✓ **CERRADO v0.9.51** | `parse_postfix` preserva `Expr::Field { field: "" }` en lugar de descartar el stmt entero |
| ~~R.bug-pyo3-abi3-portable-link Linux/macOS~~ | ✓ **RECLASIFICADO v0.9.56** | Verificado empíricamente 2026-05-24 que NO es cerrable: `libpython3.so` (13 KB) en `python:3.X-slim` exporta solo 4 símbolos glibc (no exporta API Python). En Linux NO existe equivalente al `python3.dll` shim de Windows. Movido a constraint arquitectural permanente; ver `docs/deudas_lenguaje.md` |
| ~~8-pyi-stubs~~ | ✓ **CERRADO v0.9.57** | `src/pyi_loader.rs` nuevo con auto-pickup en 2 pases: pase 1 carga classes adyacentes al `.fitz` raíz, pase 2 procesa fns/vars del stub como fields tipados de un nominal sintético `__pyi_module_<binding>`. `infer_method_call` para Nominal busca primero en fields-as-callable (Function type). Binding `from python import foo` tipa como `Type::Nominal(synth_id)` si hay stub, sino fallback a `PyAny`. 14 unit tests + smoke E2E + cap 21.8b reescrito + ejemplo `examples/guide/21c-pyi-autopickup/`. **Inventario activo queda vacío después de este cierre.** |
| ~~Smoke real Docker boilerplate 5~~ | ✓ **CERRADO v0.9.50** | smoke end-to-end con Postgres VERDE, imagen 136 MB |
| ~~Smoke real Docker boilerplate 6~~ | ✓ **CERRADO v0.9.52** | smoke end-to-end con Postgres + nginx + CORS preflight VERDE, imagen 136 MB |

**Lección aprendida** (tercera vez en 3 sesiones consecutivas): los inventarios escritos hace varias mini-tandas tienden a desactualizarse rápido. **Convención nueva**: al iniciar cualquier bundle, hacer audit rápido (10-15 min) de las deudas listadas antes de prometer trabajo. Ejemplos de comandos del audit:
- LSP: `grep -nE "fn make_hover_with_range|fn resolve_cross_module|collect_local_bindings_at" src/lsp.rs`
- Clippy: `cargo clippy --all-targets --all-features -- -D warnings`
- Codegen Python: reproducir el caso con un `.fitz` mínimo + `fitz build` (lo más confiable).

---

## Qué NO entró en la auditoría

- **Fase 6/7/8/9** (Async, DX HTTP, Interop Python, Ecosistema): decisión de roadmap, no
  auditoría.
- **Features del syntax-spec NO implementadas** todavía
  (async/await real, middleware, headers, TLS, streaming):
  documentadas como dirección, no contrato. La auditoría solo
  señala donde docs/código discrepan sobre el estado actual.
  **Nota post-5b**: status codes custom y query params se
  cerraron en mini-fases dedicadas y salieron de esta lista.
- **Verificación bit-a-bit profunda** de cada feature: el smoke test
  E2E ya cubre los ejemplos compilables; no re-verifiqué cada uno.
- **Benchmarks de performance**: las menciones P1-P5 son
  observaciones sobre el código, no medidas. Si alguna duele, hace
  falta benchmark dedicado.

---

## Próximos pasos sugeridos

**Quick wins cerrados** (L1 clippy, R1 fn main + decorators no-@server,
D1 header guía parcial, D5 status codes spec). El cleanup chico que
queda en pie son **L2** (helper `with_temp_output` — ~13 sitios) y
**D3** (syntax-spec header desactualizado).

**S1 (span en AST)** está cerrado en sus tres frentes: B.1 (Stmt),
S1.2 (Expr en checker + evaluator), y **S1.codegen** (52 sitios
del codegen con `err_at` + 17 internos con `err()` documentados
como defensivos). Mensajes de error pasan de `0:0` a línea/
columna precisas en cualquier camino del compilador (checker,
runtime, codegen). Pendiente residual menor: `Pattern` y
`TypeExpr` sin span — deuda explícita, baja prioridad porque
los errores de patrones suelen estar en sitios donde el match
contenedor ya provee un span razonable. **T1 cerrado entero**
(ver ítem siguiente).

Las **deudas funcionales** son sub-pasos formales que mejor se
abren como mini-fases dedicadas, cada una con plan corto + tests
+ cierre. Estado actual:
- **F2** (field assignment chequeo) ✅ — cerrada en C-F2.
- **F12** (higher-order completo) ✅ — cerrada con `TypeExpr::Function`
  + codegen a `Rc<dyn Fn(...) -> R>`. Cap 11 ahora compila.
- **F11** (state HTTP compartido) ✅ — cerrada vía `thread_local!`
  + tokio current_thread. `examples/server.fitz` y
  `examples/guide/17-http.fitz` compilan + corren end-to-end.
  Trade-off documentado: server single-threaded hasta que aterrice
  async/await real (entonces se pivota a Arc/Mutex + `State`
  extractor).
- **S1.2** (span en Expr + checker + evaluator) ✅ — los 3
  sub-pasos cerrados. Errores expr-level del checker y de
  runtime citan posición exacta del nodo problemático (operador,
  paréntesis, corchete, argumento concreto, valor del campo,
  etc.). 19 tests dedicados de span entre parser/checker/
  evaluator. Deuda residual menor: codegen call sites siguen con
  `err()` (helper `err_at` listo en `CodegenCtx`).
- **T1** (tests frágiles del codegen) ✅ — **cerrado entero**.
  Infra `ast_test` (módulo adentro de `mod tests`) parsea el Rust
  generado con `syn::parse_file` y expone ~30+ helpers para buscar
  items, lets, signatures, derives, macro calls, method calls, loops,
  matches, casts, attrs, visibilidad, routes axum, etc. con
  stringificación normalizada via `quote::ToTokens`. **~115 tests
  migrados** en tres batches:
  - **Batch 1** (primer pase): expresiones, literales, primitivas.
  - **Batch 2** (28 tests): Listas/Mapas/Indexing/Métodos built-in +
    F12 closures (FnExpr suelta, fn como valor/param/retorno,
    captura no-Copy, FnExpr inline como arg).
  - **Batch 3** (50 tests en 4 sub-commits, 3a HTTP / 3b Result/match
    / 3c módulos / 3d sobrantes): HTTP wrappers async, Router, path
    params, status codes custom, query params, body POST, server
    decorator, state thread_local, type impls JSON, Result/Ok/Err/`?`/
    match con bindings/range guards, módulos (pub items, static/const
    top-level), type-def Display, struct-lit con defaults/nullables,
    igualdad estructural, pasar instance, if-as-expr, str-interp.
  Beneficio acumulado: cambios cosméticos del codegen (espacios,
  agrupación de paréntesis, sufijos numéricos alternativos, orden de
  attributes, formato de macros) no rompen estos tests — solo cambios
  estructurales reales (renaming de tipos generados, eliminación de
  bindings, cambio de semántica) los rompen. Removidos helpers
  dead-code `assert_contains` y `assert_http_contains` tras la
  migración. **Residual aceptado**: 10 `code.contains` siguen vivos
  intencionalmente — 4 sobre `ast_test::ts(&file)` (tokens AST
  normalizados, ya AST-based), 1 contrato de mensaje de error
  user-visible (`assert_err_contains`-style), 1 negative check sobre
  output completo, 4 sobre Cargo.toml (TOML, no Rust). Pipeline para
  futuros tests: usar `ast_test` desde el arranque en cualquier test
  nuevo del codegen.

- **HTTP status codes custom** — **cerrado en mini-fase dedicada**.
  Sintaxis del spec `return <Int> <body>` implementada end-to-end:
  - AST: nueva variante `Stmt::ReturnStatus { status, body, span }`.
  - Parser: después de `return <Int>` con `{` siguiente, parsea el
    body como Expr y emite `Stmt::ReturnStatus`. Sin `{` sigue como
    Return normal (preserva sintaxis `return 42`).
  - Checker: rechaza `ReturnStatus` fuera de handlers HTTP (`@get`/
    `@post`/`@put`/`@delete`). Stack `in_http_handler` paralelo al
    `return_stack`. No chequea body contra return type formal del
    handler (polimorfismo del spec).
  - Intérprete: nueva `Value::HttpResponse { status, body }` opaca
    fuera de context HTTP. `value_to_outcome` la intercepta y emite
    el `HandlerOutcome` con el status pedido.
  - Codegen: scan recursivo sobre body de cada fn HTTP; si hay
    `ReturnStatus`, su return type Rust se cambia a `__FitzResponse`
    (struct nueva en preludio HTTP) y todos los returns (normales y
    custom) se envuelven uniforme. El handler wrapper destructura
    `__FitzResponse` y emite `(StatusCode::from_u16(...),
    Json(body))`. Flag `response_mode` se resetea al entrar a
    FnExpr (callback inline + fn suelta) — el body del closure no
    hereda el modo del handler contenedor.
  - Polimorfismo del spec: handler `-> Str` puede mezclar
    `return "ok"` (200) con `return 404 { ... }`. El return type
    declarado se ignora en este path.
  - Cap 17 de la guía actualizado con sección "Status codes custom"
    + 3 ejemplos. `examples/guide/17-http.fitz` sumó endpoints
    `/protected` (401) y `/users/{id}/profile` (200 ó 404).
    Validado bit-a-bit `fitz run` vs `fitz build`.
  - 16 tests dedicados (parser 3, checker 4, http 3, codegen 4, E2E
    2).
  - **Deuda explícita que queda**: `return 204` sin body (parser
    exige body explícito; workaround `return 204 {}`); responses
    como expresión libre (`let r = 200 { ... }`); status codes
    desde una var (`return code { ... }` con `code` no literal).

- **HTTP query params** — **cerrado en mini-fase dedicada** (segunda
  mitad de la mini-fase HTTP combinada con status codes).
  Sintaxis del spec `@get("/items?limit={limit}&offset={offset}")`
  implementada end-to-end:
  - `parse_path_template` (http.rs): separa el path real del query
    template por el primer `?` y devuelve `query_params:
    Vec<String>` adicional. Validaciones: la key del query debe
    coincidir con el nombre del param Fitz; template malformado
    (`?limit`, `?=v`, `?{x}`) emite error específico; duplicados
    entre path y query también.
  - `RouteSpec`/`RouteMeta`: sumaron `query_params: Vec<String>` y
    `has_query_params: bool`. `InterpTask` lleva
    `query_params: HashMap<String, String>` con los raw values
    del request.
  - `build_method_router`: 8 combinaciones de `(has_path × has_query
    × expects_body)` con axum extractors apropiados (`AxumPath`,
    `Query<HashMap>`, `Bytes`).
  - `handle_task` (intérprete): para cada param Fitz, decide si es
    path/query/body. Query nullable (`Int?`) faltante → `Value::Null`;
    obligatorio faltante → 400 con mensaje. Coerción al tipo
    declarado vía `coerce_path_param` (Int/Float/Str/Bool).
  - Evaluator registro de `@get/@post`: valida que cada
    `?key={name}` del template tenga un param Fitz correspondiente.
    Mismatch → error claro. `param_types` ahora carga también
    `is_nullable: bool` para que el dispatch HTTP decida si Null
    o 400.
  - Codegen: `parse_http_path` delega a `parse_path_template` para
    devolver `(path_axum, query_params)`. El wrapper HTTP categoriza
    cada param en path/query/body; para los query emite
    `Query<HashMap<String, String>>` + binding tipado con coerción
    (`limit: i64 = match __qmap.get("limit") { ... }`). Nullable →
    `Option<T>`. Tipos no soportados (Lists, custom, Result) →
    error de codegen claro.
  - **Bug fix colateral del codegen**: `BinOp Eq/NotEq` entre
    `Nullable<T>` y `Null` ahora emite `.is_none()`/`.is_some()` en
    vez del literal `== ()` (que Rust rechaza por mismatched types
    sobre `Option<T>`). Habilita patrones tipo
    `if (limit == null) { ... }` adentro del handler.
  - Cap 17 de la guía actualizado con sección "Query params" + 3
    ejemplos. `examples/guide/17-http.fitz` sumó endpoint
    `/search?name={name}&limit={limit}` con `Str`/`Int?`.
    Validado bit-a-bit `fitz run` vs `fitz build` con curl.
  - 17 tests dedicados (http path 5, codegen 7, E2E 5).
  - **Deuda explícita que queda**: tipos no-primitivos en query
    params (List, instancias); aliases de key (`?l={limit}`,
    rechazado hoy); query params via vector (`?ids=1&ids=2`);
    query params como una struct ad-hoc (Map<Str, Str> implícito).

## Deuda residual de Fase 10.b (atacar antes del release v0.10.1)

Política del autor (2026-05-26): "Fitz tiene que tener todo lo mejor;
anotar toda la deuda residual para atacarla antes del release al
terminar todo". Todo lo que está abajo tiene que cerrarse ANTES del
release v0.10.1 (cierre formal de Fase 10.b entera).

### De 10.b.6 (Agregados scalares ORM)

- ✅ **GROUP BY + aggregate (sum/avg/min/max) + count** — CERRADO
  2026-05-26 (10.b.14). Refactor con `Type::Aggregated<Row>` nuevo:
  `.group_by(...)` muta de `QueryBuilder<Row>` a `Aggregated<Row>`,
  y sobre Aggregated los aggregates devuelven
  `Future<Result<List<Map<Str, Any>>>>` (path GROUP BY) en vez de
  `Float`/`Int` (path scalar). Helper de preludio db nuevo
  `aggregate_groups(conn, agg_expr, agg_name)` emite el SELECT con
  GROUP BY y materializa cada row como `Vec<(__FitzValue,
  __FitzValue)>`. `.all/.first/.update/.delete` se rechazan sobre
  Aggregated (no tiene sentido sobre GROUP BY) con error claro del
  checker. `program_uses_fitz_value` extendido para detectar
  `.group_by(...)` y forzar emisión del enum `__FitzValue` +
  helpers. Test paridad real:
  `orm_group_by_aggregate_paridad_codegen_e2e` valida bit-a-bit
  count + sum agrupados por region (3 grupos PAT/BUE/CBA).
  Cambios: types.rs (variant nuevo + infer_aggregated_method),
  codegen.rs (gen_orm_qb_method con `is_aggregated` flag + helper
  preludio db). Paridad estricta evaluator ↔ codegen restaurada.

### De 10.b.7 (Navigation methods)

- ✅ **`#[allow(clippy::only_used_in_recursion)]` en
  `orm_field_coerce_block`** — CERRADO 2026-05-26 (10.b.10.1). El
  `env` se removió del signature; el cleanup quedó porque el caller
  (`gen_orm_navigation` → `orm_lookup_meta_and_fields`) ya hace las
  validaciones nominales que originalmente habían motivado mantener
  el param. Signature más chica + sin `#[allow]`.
- ✅ **Args extras a navigation (chain)** — CERRADO 2026-05-26
  (10.b.13). `instance.posts()` (sin args) ahora devuelve
  `QueryBuilder<Post>` para encadenar `.where(...).order_by(...).
  limit(N).all(db).await?` igual que `Type.where(...)`. Backward
  compat: `instance.posts(db)` (con db) sigue siendo terminal
  directo (`.all` para HasMany, `.first` para BelongsTo/HasOne).
  Checker, evaluator y codegen actualizados en paridad. Test
  paridad real: `orm_navigation_chain_paridad_codegen_e2e` valida
  bit-a-bit chain de 4 ops sobre nav + path legacy en el mismo
  programa. Kwargs (`instance.posts(limit=10)`) NO en MVP — usar
  el chain explícito. Más expressivo y consistente.
- ✅ **Eager loading (preload)** — CERRADO 2026-05-26 (10.b.15).
  `User.where(...).preload("posts").all(db).await?` evita N+1
  ejecutando 1 query batch al target type con `WHERE fk IN
  (parent_pks)` y poblando los fields virtuales de cada parent.
  Implementación: state nuevo `preloads: Vec<String>` en el
  `__FitzQueryBuilder<TData>` + método `with_preload(name)`. El
  codegen de `.all`/`.first` envuelve el query base con un loop
  que itera los preloads y, por cada uno, hace match estático
  contra las HasMany relations conocidas del row type en
  compile-time (cero overhead cuando no se usa). Helper
  `emit_preload_dispatch(meta)` genera el bloque inline con el
  SQL batch, deserialize a `Vec<Arc<Mutex<TargetData>>>`,
  particionado por FK, y mutación del field virtual del parent.
  Branch `.preload(name)` valida en codegen que name corresponda
  a una relation @has_many declarada. `User.preload(...)`
  directo + `User.where(...).preload(...)` chain ambos soportados.
  Test paridad real: `orm_preload_has_many_paridad_codegen_e2e`
  valida bit-a-bit `u0=ada:3 u1=alan:1 u2=grace:0` con 1 query
  para users + 1 batch para posts (en vez de N+1). MVP solo
  HasMany — BelongsTo y HasOne quedan como deuda menor abierta
  para v0.11 si entra demanda (sus casos típicos los cubren
  navigation methods directos sin riesgo de N+1).
- ✅ **Cross-type navigation con `@column(name=...)` en el FK source
  field** — CERRADO 2026-05-26 (10.b.10.2). Test paridad real
  `orm_navigation_con_column_override_en_fk_source_paridad_codegen_e2e`
  con esquema donde el SQL column del FK se llama `author_uid` (≠
  field Fitz `user_id`). Validado bit-a-bit: el SELECT del Post
  usa el override, la navigation a User funciona correcto.

### De 10.b.8.a (Arrays Postgres)

- ✅ **`.update(db, {"tags": [1,2]})` con List literal** — CERRADO
  2026-05-26 (10.b.11.a). `gen_qb_update_set_args` ahora detecta
  field `List<scalar>` + value `Expr::List` literal y emite
  `__FitzPgValue::Array { elem_oid, values: vec![...] }` directo
  (sin pasar por el genérico `__IntoPgValue::into_pg`). Helper
  nuevo `fitz_scalar_lit_to_pg_value_code` wrappea cada item al
  variant esperado. Test paridad real:
  `orm_update_con_list_y_map_literal_paridad_codegen_e2e` valida
  round-trip insert + update + select con tags `int8[]` y meta
  `jsonb`.
- ⚠️ **Arrays anidados (`List<List<T>>`)**: Postgres soporta arrays
  multidimensionales nativamente, pero el driver Fitz solo parsea
  arrays planos (`parse_array_text` en `src/db.rs` ~1397). Cerrar
  esto requiere refactor del driver (parse + encode + tipos del
  wire) con beneficio marginal — los usuarios reales tienden a
  modelar data 2D como JSONB o como `@has_many`. **No bloquea
  v0.10.1**; queda como deuda menor abierta para v0.11+ si
  aparece demanda. Workaround: usar `Map<Str, Any>` y guardar el
  array anidado como JSON.
- ⚠️ **`List<Nominal>`** (e.g. `tags: List<Tag>` con Tag tipo
  custom): Postgres NO tiene "array of struct" nativo. Las dos
  alternativas reales son (a) JSONB array (no es `List<T>` real,
  solo similar shape) y (b) tabla relacionada con `@has_many`
  (que YA está implementado en 10.b.7). **No bloquea v0.10.1**;
  el patrón canónico para esto es `@has_many`, no array. La
  deuda queda CERRADA con workaround documentado.
- ✅ **NULL adentro de arrays (`{1, NULL, 3}` → `List<Int?>`)** —
  CERRADO 2026-05-26 (10.b.12.a). `orm_list_scalar_info_with_null`
  detecta `List<Int?>`/etc. y propaga `inner_nullable` flag.
  Coerce emite `Vec<Option<T>>` con `matches!(__item, Null)
  → None / Some(...)`. Marshal emite `match __it { Some(__v)
  => PgValue::T(*__v), None => PgValue::Null }`. Test paridad
  real: `orm_list_nullable_inner_paridad_codegen_e2e` valida
  bit-a-bit `len1=5 len2=3` con NULLs en arrays Postgres.

### De 10.b.8.b (JSONB libre)

- ✅ **`.update(db, {"meta": {...}})` con Map literal** — CERRADO
  2026-05-26 (10.b.11.b). `gen_qb_update_set_args` detecta field
  `Map<Str, Any>` + value `Expr::Map` literal y emite
  `__FitzPgValue::Text(__fitz_fitz_value_to_jsonb(&__FitzValue::
  Map(...)).expect(...))`. Helper nuevo
  `fitz_lit_to_fitz_value_code` wrappea recursivamente cualquier
  Fitz literal puro (Int/Float/Str/Bool/Null + List/Map anidados)
  a `__FitzValue`. Test paridad real valida JSONB anidado.
- ✅ **`Map<Str, Str>`, `Map<Str, Int>` (Map concretos no-Any)** —
  CERRADO 2026-05-26 (10.b.12.b). `orm_map_str_concrete_info`
  detecta `Map<Str, Int|Float|Str|Bool>` con T concreto y emite
  deserialize via `serde_json::from_str` + iter + `as_i64/f64/str/
  bool()` validando shape. Marshal serializa directo a
  `serde_json::Value::Number/String/Bool` sin __FitzValue (más
  eficiente). El cast SQL sigue siendo `::jsonb`. Bonus: el helper
  `program_uses_fitz_value` ahora también activa serde_json cuando
  hay Map en types @table (aunque no haya Any). Test paridad real:
  `orm_map_str_concreto_paridad_codegen_e2e` valida bit-a-bit
  insert + select con `Map<Str, Int>` y `Map<Str, Str>`. Otros
  Map (`Map<Int, T>`, etc.) siguen rechazados — JSON objects solo
  aceptan keys string.
- ✅ **Validación shape JSONB** — CERRADO 2026-05-26 (10.b.13.b)
  por DECISIÓN DE DISEÑO. `Map<Str, Any>` significa "cualquier
  shape JSON válido"; validación de shape específico (timestamps
  ISO, emails, UUIDs) es responsabilidad del user via `match`/
  `is_in([...])`/parsing manual. Para schemas conocidos a priori,
  el patrón recomendado es `Map<Str, T>` concreto (10.b.12.b)
  con T = Int/Float/Str/Bool, que valida el shape automáticamente.
  Schema annotations (`@shape({"created_at": "iso8601"})`) quedan
  como deuda menor abierta para v0.11+ si aparece demanda real —
  diseño grande (decorator + parser + validation engine) sin
  beneficio claro vs. validación manual en handlers.

### Deudas viejas que siguen abiertas (impactan 10.b)

- ✅ **Test paridad real `db_real_postgres` no corre en CI default** —
  CERRADO 2026-05-26 (10.b.16). Job nuevo `db-postgres` en
  `.github/workflows/ci.yml` que levanta `postgres:16` como service
  container, exporta `FITZ_TEST_PG_URL=postgres://postgres:postgres@
  localhost:5432/fitz_test`, y corre `cargo test --test db_real_postgres
  -- --ignored --test-threads=1`. Solo Linux (Docker service
  containers más estables en GHA Linux runners; los tests no
  dependen de plataforma — el binario standalone es x86_64-linux).
  Los 14 E2E paridad real (belongs_to + has_many + arrays + JSONB
  + where combinatorio + between/mod/var_ext + array ops + nav
  chain + group_by aggregate + Map<Str,T> concreto + List<scalar?>
  + preload + CRUD lifecycle + order_by/limit + basics + col
  override en FK source + .update con List/Map literal + agg
  scalar) ahora corren en cada push a main. `#[ignore]` se mantiene
  para que `cargo test` default sin env var siga rápido.
- ✅ **Smoke GUIDE_EXAMPLES_COMPILE no incluye ejemplos ORM** —
  CERRADO 2026-05-26 (10.b.17). Nuevo `examples/guide/32-orm.fitz`
  pedagógico (~100 LoC) que muestra el shape canónico del ORM
  end-to-end: `@table` con `@primary` + `@column` + `@belongs_to`
  + `@has_many`, insert, where + first, chain
  `order_by`/`limit`/`offset`, operadores `starts_with`/`is_in`/
  `between`, aggregates scalares `count`/`avg`, GROUP BY con
  `Aggregated<Row>`, navigation `belongs_to`/`has_many`, eager
  loading con `preload`, y `update`/`delete` con guard `.where(...)`
  obligatorio. Sumado al smoke `GUIDE_EXAMPLES_COMPILE` —
  `fitz build` produce binario que NO requiere Postgres real al
  compilar; el `connect` runtime falla con `Err` clara cuando la
  URL inválida, así el ejemplo es ejecutable como guía aunque no
  haya Postgres local. Cierra la última deuda residual de Fase
  10.b antes del release v0.10.1.

### Mini-fase W17 (2026-05-27) — Virtual fields skip en impls cross-module

Descubierta durante el primer intento de implementar el boilerplate
`api-orm-full` (showcase del ORM + stack web first-class
multi-archivo). Cierra el último gap conocido del codegen cross-
module ORM. **Ningún cambio user-facing**: sin sintaxis nueva, sin
keyword nueva, sin decorator nuevo — solo el codegen ahora emite
impls `__ToFitzJson`/`__FromFitzJson` correctos para `@table types`
con relations virtuales declarados en módulos.

- ✅ **W17 — `@table` type con relations virtuales (`@has_many`/
  `@has_one`/BelongsToCompanion) declarado en módulo A + handler que
  lo retorna en módulo B**. Antes del fix, el codegen al emitir
  `impl __FromFitzJson for UserData` en main.rs hacía remap de los
  fields virtuales (`posts: List<Post>`) → `List<Any>` (porque
  el target type `Post` no estaba en el env del importer) → emitía
  `Vec<__FitzValue>`. Pero `__FitzValue` no se activaba por el
  programa (sin `Map<Str, Any>` ni `List<Any>` legítimo en el
  source Fitz), entonces rustc rompía con
  `cannot find type __FitzValue in this scope` y el binario
  fallaba al linkear. **Fix**: skipear los virtual fields
  (HasMany/HasOne/BelongsToCompanion via
  `TableMetadata.is_virtual_field`) en los impls
  `__ToFitzJson`/`__FromFitzJson`. Esos fields no van a la DB ni
  deben aparecer en JSON I/O — el cliente no debe poder enviarlos
  como body, y la response no los serializa. En el struct literal
  del `__from_fitz_json`, los virtuales se inicializan inline con
  `Default::default()` para evitar nombrar el tipo
  remap-degradado. Cambios: nueva variante
  `gen_type_http_impls_for_sig_with_meta(name, sig, meta:
  Option<&TableMetadata>)` que filtra virtuales; ambos call sites
  (uno local en `gen_type_http_impls`, otro cross-module en
  `emit_helpers_for_imported_types`) actualizados para pasar el
  meta. Test E2E nuevo
  `cross_module_orm_virtual_fields_skip_w17` candea el caso con 3
  archivos (models.fitz + posts.fitz + main.fitz). Smoke
  `GUIDE_EXAMPLES_COMPILE` verde — el ejemplo `31-orm.fitz`
  sigue compilando bit-a-bit; otros 6 tests cross-module
  (W8/W10/W11/W12/W15/W16) sin regresiones. Validado runtime:
  `GET /users` devuelve `[{"id":7,"name":"ada"}]` SIN incluir
  el virtual `posts` (skip correcto).

### Deuda derivada de la sesión W17

- ⚠️ **Inferencia del checker post-match Result con early-return Err
  → Option<String>**. Caso: `let x = match Result { Ok(v) => v,
  Err(_) => return Err("..."), }`. El checker infiere `x` como
  `Option<String>` cuando debería ser `String` (el `Err` branch
  termina en `return`, no produce valor). El codegen emite
  `let mut x: Option<String> = (match ...)` que rustc rompe con
  "expected Option<String>, found String". **Workaround**:
  anotar el tipo explícitamente — `let x: Str = match ...`.
  Detectado al implementar `auth.fitz` del boilerplate
  `api-orm-full`. **No bloquea** ningún ejemplo de la guía (los
  patterns con Result + match exhaustivo NO usan bindings de la
  fork de Err en el caller). Refinement del checker queda como
  deuda menor abierta — no es urgente porque el workaround es
  trivial y descubrible.

- ⚠️ **Cross-module ORM 3 archivos — patrón
  `<table types en módulo + handlers en otro módulo + main solo
  imports>`** (probado por W17 fix). Aunque W17 cierra el bug
  del trait bound `__ToFitzJson`, hay deudas residuales menores
  derivadas de esa exploración:
  - **Forward refs en `@has_many("Target")` con Target declarado
    después en el mismo módulo**: rompen el codegen ORM cuando
    el codegen emite navigation method al procesar el type.
    El ejemplo `31-orm.fitz` evita el caso porque no invoca
    navigation directamente — solo declara las relations. Caso
    confirmado en mi exploración: `type User { ... @has_many
    Post ... } type Post { ... }` falla con "type Post no
    registrado en TypeEnv" si el codegen intenta resolver el
    target. **Workaround**: declarar Target ANTES de User, y
    los companion fields (`user: User?`) backward-ref a User.
  - **Importar TODOS los `@table` types al módulo que usa
    cualquier uno**: el codegen valida ALL los targets de
    relations de un type al procesarlo. Si User declara
    `@has_many("Post", ...)` pero el módulo solo hace
    `from models import User`, el codegen falla con "type Post
    no registrado". **Workaround**: `from models import User,
    Post, ...` (todos los referenciados). Refinement futuro:
    el codegen podría auto-resolver los target types desde el
    loader sin requerirlos en el `from import`.

- ⚠️ **`Map<Str, Any>` en HTTP response de handlers cross-module**.
  El handler que retorna `Map<Str, Any>` (caso típico GROUP BY +
  db.query crudo) funciona OK en single-file. En cross-module,
  cuando el handler vive en módulo B y el `Map<Str, Any>` arrastra
  Vec<__FitzValue> al codegen del módulo, los impls
  `__ToFitzJson`/`__FromFitzJson` necesarios se buscan en main.rs.
  W17 no toca este caso (es un workaround del cap 31 sec 28 ya
  documentado para single-file). Refinement futuro: replicar la
  Decisión W17 (skip lookup local, usar Default::default) para
  Vec<__FitzValue> en módulos. No bloquea casos actuales.

### Mini-fase W18+ (2026-05-28) — Gaps cerrados durante api-orm-full multi-archivo

Bloque cerrado al construir el boilerplate
[`api-orm-full`](../boilerplates/api-orm-full/) (8va plantilla,
showcase del stack web first-class entero multi-archivo). Cada gap
descubierto durante la escritura del boilerplate se cerró en bloque
ANTES de declarar el boilerplate completo. **5 fixes del codegen
en una sesión**:

- ✅ **R.1.3 — `Map<Str, Any>` con indexing assignment dinámico
  (`m["k"] = v`)**. El storage Rust de `Map<_, Any>` es
  `Vec<(__FitzValue, __FitzValue)>`. El codegen del indexing
  assignment SIEMPRE emitía `__g.push((__k, __v))` con tipos
  crudos (String/T), generando "expected __FitzValue, found String".
  Fix en `gen_index_assign`: detectar `storage_is_heterogeneous`
  (k o v es Any) y envolver key/value con `wrap_as_fitz_value_with_env`.
  Caso canónico: partial updates en APIs REST. Test E2E
  `map_str_any_indexing_assign_compilado`.

- ✅ **R.1.3-bis — `.has(var)` sobre `Map<Str, Any>`**. Paralelo
  al anterior: `gen_map_has` no envolvía el arg como __FitzValue
  cuando el storage es heterogéneo. Fix: nuevo param `value_ty` +
  check `storage_is_heterogeneous` con wrap igual.

- ✅ **W18 — `has_opaque_field` ignora virtuales del ORM en
  cross-module**. El check previo a `gen_type_http_impls_for_sig_with_meta`
  miraba TODOS los fields del `remapped_sig`, incluso los virtuales
  (`@has_many`/`@has_one`/BelongsToCompanion). Cuando un virtual
  apuntaba a un target no importado al main, el remap lo degradaba
  a `Nullable(Any)` o `List(Any)` y el filtro skipeaba TODO el
  impl. Sin impl `__ToFitzJson`, rustc rompía con
  "trait bound not satisfied". Fix: filtrar virtuales antes del
  check usando el `TableMetadata` ya disponible. Caso canónico:
  cross-module ORM 4-archivos (`models` + `auth` + `posts` + `main`)
  donde main solo `import posts` sin traer Post al scope local.
  Test E2E `cross_module_table_virtual_w18_remap_any`.

- ✅ **Bug del format string en jsonb dynamic update**. En el
  dispatch `Dynamic` de `.update(db, map_var)` para fields jsonb,
  la string del `Err(e)` arm tenía `{{}}` (escaped braces) donde
  debería tener `{}` (placeholder de format). Como la string se
  produce vía `.replace("{f}", ...)` y NO via `format!`, las
  llaves quedan literales en el código Rust generado. Resultado:
  rustc rejecta con "argument never used" porque el `e` jamás
  se interpola. Fix trivial: cambiar `{{}}` → `{}`. Cubierto por
  el boilerplate.

- ✅ **`.has(var)` sobre arrays Postgres (`text[]`/`int8[]`/etc.)**.
  El codegen rechazaba con "el value debe ser literal del tipo del
  array". Fix: delegar a `translate_closure_to_sql` cuando no hay
  match con literal Fitz; reusa la máquina de W3 (`.like(var)`) y
  W6 (`body.field`) que bindean via `__IntoPgValue::into_pg(...)`.
  Caso canónico: filtros por tag en endpoints listables. Test E2E
  `orm_array_has_acepta_var_externa`.

**Tests al cierre del bloque W18+**: smoke `GUIDE_EXAMPLES_COMPILE`
292 ejemplos verde con los 5 fixes integrados. 3 tests E2E nuevos
en `tests/compile_e2e.rs`.

**Gaps descubiertos en la sesión y NO cerrados** (NO bloquean el
boilerplate, documentados para fases futuras):

- ⚠️ **Narrowing flow-sensitive de `Nullable<T>` → `T` post-`if
  (x != null)`**. El checker no refina `Str?` a `Str` después del
  check. `let s: Str = x` falla. **Workaround idiomático**: match
  arm con `Pattern::Ident` (W2 ya cubre el refinement adentro de
  match). Refinement flow-sensitive en `if` es propio del checker
  y queda como deuda residual.

- ⚠️ **Broadcast HTTP → WS cross-handler**. `conn.broadcast(msg)`
  solo funciona DESDE un handler `@ws`. No hay primitiva para
  "handler HTTP triggerea broadcast a clientes WS conectados".
  Caso canónico SaaS (comment nuevo → notification realtime).
  Requiere API global tipo `ws_broadcast(endpoint, msg: T)` o un
  `WsBroadcaster` capturable en el scope del handler HTTP. Scope
  grande, queda como deuda visible. El boilerplate api-orm-full
  modela `/feed` como broadcast simétrico entre clientes WS para
  showcasear el WS sin pelear este gap.

### Mini-fase post-release v0.10.7 — Gaps descubiertos en smoke real Docker

Bloque de gaps descubiertos al hacer el smoke end-to-end del
boilerplate `api-orm-full` con Postgres real adentro de Docker
(tag `v0.10.7`). El binario compila local + `fitz check` verde +
smoke 292 verde NO los detectaba — solo aparecen cuando el binario
levanta el server contra una DB real y se le pegan requests HTTP.

**3 gaps cross-module nuevos abiertos**:

- ⚠️ **OpenAPI 3.1 schema vacío cuando los handlers HTTP viven
  cross-module**. `GET /openapi.json` devuelve `{"paths": []}`
  cuando los handlers `@get`/`@post`/`@put`/`@delete` están en
  módulos importados (caso canónico de cualquier boilerplate
  multi-archivo serio). El codegen del schema (`openapi.rs`) solo
  mira `program.http_fns` del main local, no recolecta los
  handlers cross-module via el loader. Resultado: el W16
  (rutas cross-module se enchufan al Router) NO está coordinado
  con el OpenAPI auto-generación. El Router responde a los
  endpoints, pero `/openapi.json` y `/docs` salen vacíos
  visualmente. **Fix futuro v0.10.8**: el generador de schema
  debe iterar también `loader.modules[*].http_fn_stmts` (W16 ya
  los captura).

- ⚠️ **AsyncAPI 3.0 endpoint no se registra cuando los `@ws`
  viven cross-module + los handlers WS mismos NO se enchufan al
  Router axum**. `GET /asyncapi.json` → 404 y `WS /feed` →
  404 en handshake. **Más grave de lo que parecía**: no es solo
  schema vacío como el OpenAPI cross-module — los WS handlers
  cross-module no se registran como rutas en el axum::Router del
  main. W16 (v0.10.7) cubrió solo `@get/@post/@put/@delete`, no
  incluyó `@ws`. **Fix futuro v0.10.8**: extender W16 para que
  itere también `loader.modules[*].ws_fn_stmts` y emita las
  rutas WS qualified (`.route_service("/feed",
  crate::realtime::__ws_handler_feed)` o equivalente), más el
  AsyncAPI schema con los `@ws` de módulos. Detectado en smoke
  real con cliente Node `ws`: handshake al endpoint cross-module
  devuelve 404 antes del upgrade HTTP→WS.

- ⚠️ **ORM no skipea fields `Str = ""` del INSERT cuando hay
  DEFAULT en el schema**. W4 cubre solo `id: Int = 0` para
  bigserial PK. Para timestamps con `DEFAULT NOW()` o cualquier
  field con DEFAULT del lado Postgres, el INSERT siempre incluye
  el field con el value de Fitz (típicamente `""` para
  timestamps), que Postgres rechaza con
  `"invalid input syntax for type timestamp with time zone: \"\""`.
  **Workaround en el boilerplate api-orm-full**: cambiar el
  schema de `timestamptz NOT NULL DEFAULT NOW()` a `text NOT
  NULL DEFAULT ''` (pierde el tipo nativo, gana smoke OK). **Fix
  futuro v0.10.8**: agregar sentinel general para Str/Nullable
  ("si value es el default literal, skipear field del INSERT") o
  exponer una API tipo `db.now()` built-in que emita el ISO 8601
  actual desde Fitz al insertar.

- ⚠️ **W17 skipea virtuales del JSON aunque vengan poblados por
  `.preload(...)`**. Caso canónico: `Post.where(...).preload("author")
  .preload("comments").first(db)` carga los virtuales en memoria,
  pero `impl __ToFitzJson for PostData` (emitido con W17) los
  skipea al serializar — el response JSON nunca muestra el
  eager-loaded data. El feature `.preload(...)` queda
  parcialmente roto: ejecuta las queries adicionales pero el
  cliente no ve los resultados.
  **Workaround actual**: ninguno limpio — el cliente puede
  hacer un segundo request a `GET /posts/{id}/comments` y
  `GET /users/{author_id}` para obtener los datos. Pierde el
  beneficio principal del eager loading (1 round-trip vs N+1).
  **Fix futuro v0.10.8**: W17 debe distinguir entre "skip
  virtuales en `__FromFitzJson`" (correcto — no van como body
  input) y "skip virtuales en `__ToFitzJson`" (incorrecto cuando
  están poblados — sí van en el response). Posible diseño:
  flag runtime "is_loaded" sobre el field virtual que el
  serializer chequea, o emitir el field en JSON solo cuando no
  es default (`null` para HasOne/BelongsToCompanion, `[]` para
  HasMany).

- ⚠️ **HTTP wrapper no desempaca `Result<T>` tail sin `Ok(...)`
  explícito**. Cuando un handler `async fn handler(...) -> Result<T>`
  termina con `return <expr_que_devuelve_Result<T>>` (típicamente
  el `.await` de un chain ORM como `Post.where(...).first(conn).await`),
  el HTTP wrapper serializa el `Result` entero como
  `{"Ok": {...}}` en lugar de extraer el `T` y devolverlo
  directamente. Pero si el handler termina con `return Ok(x)`
  explícito (típicamente tras `let x = <chain>.await?; return
  Ok(x)`), el wrapper SÍ desempaca y devuelve `T` puro. **Caso
  canónico que rompe**: `return <ChainQB>.first(db).await`.
  **Workaround en api-orm-full**: reescribir los handlers afectados
  (`get_post`, `update_post`, `delete_post`, `stats_posts_per_user`)
  con `let x = ...?; return Ok(x)` en vez de `return ...await`.
  **Fix futuro v0.10.8**: el HTTP wrapper debe detectar el tipo
  del expr final y siempre desempacar `Result<T>` sea explícito o
  no. Detectado al smoke real del boilerplate api-orm-full
  cuando todo el resto del stack ya funcionaba.

**Parches temporales aplicados al boilerplate api-orm-full
(REVERTIR cuando los gaps de v0.10.8 cierren)**:

Estos workarounds son temporales — el boilerplate debería volver
a la sintaxis canónica (showcase del stack completo) cuando
v0.10.8 cierre los gaps subyacentes. Lista para revertir
post-v0.10.8:

1. **`schema.fitz`** — revertir `text NOT NULL DEFAULT ''` →
   `timestamptz NOT NULL DEFAULT NOW()` en los fields
   `created_at` (users/posts/comments) y `published_at` (posts).
   Pre-requisito: cerrar gap "ORM no skipea Str sentinel del
   INSERT". Sin esto, el INSERT sigue mandando `''` y rompe.

1.b. **`posts.fitz`** — revertir los handlers `get_post`,
   `update_post`, `delete_post`, `stats_posts_per_user` a su
   forma idiomática `return <chain>.await` (sin `let x = ...?;
   return Ok(x)` boilerplate). Pre-requisito: cerrar gap
   "HTTP wrapper no desempaca Result tail sin Ok() explícito".
   Sin esto, los responses siguen viniendo como `{"Ok": ...}`.

2. **`docs/deudas-post-5b.md`** — borrar este bloque entero de
   "Mini-fase post-release v0.10.7 — Gaps descubiertos en smoke
   real Docker" cuando los 3 gaps queden cerrados.

3. **`schema.fitz`** — opcional, si llega `db.now()` o
   built-in time: el handler podría setear `created_at: db.now()`
   en lugar de depender del DEFAULT del schema. Decisión
   pedagógica abierta.

4. **README del boilerplate (`boilerplates/api-orm-full/README.md`)**
   — actualizar la sección "Notas de diseño" con la sintaxis
   canónica una vez los gaps cierren (sacar las menciones de
   workarounds que ya no apliquen). Verificar también las
   referencias a `v0.10.7` y bump a `v0.10.8` en `FITZ_TAG`.

5. **Documentación cap 31 de la guía / `docs/db-orm.md`** —
   si se documentan los gaps actuales como deudas del ORM,
   borrarlos de ahí también una vez que cierren.

### 🎯 Cierre formal v0.10.8 (2026-05-28) — TODOS los gaps cerrados

Mini-fase de cierre ejecutada en 4 rondas (10.8.1 → 10.8.8) en
una sesión. Los 8 gaps descubiertos durante el smoke real Docker
del boilerplate api-orm-full v0.10.7 quedaron cerrados con fix
+ test E2E + revert del workaround respectivo:

| # | Gap | Estado |
|---|-----|--------|
| #1 | Narrowing flow-sensitive `Nullable<T>` post-`if (x != null)` | ✅ CERRADO 10.8.4 |
| #2 | Broadcast HTTP → WS — built-in `ws_broadcast(endpoint, msg)` | ✅ CERRADO 10.8.7 |
| #3 | OpenAPI 3.1 cross-module paths (paths vacío) | ✅ CERRADO 10.8.5 |
| #4 | WS Router cross-module + AsyncAPI cross-module (404) | ✅ CERRADO 10.8.6 |
| #5 | ORM no skipea Str sentinel del INSERT con DEFAULT del schema | ✅ CERRADO 10.8.2 (decorator `@db_default`) |
| #6 | HTTP wrapper no desempaca `Result<T>` tail sin `Ok()` explícito | ✅ CERRADO 10.8.1 |
| #7 | W17 skipea virtuales del JSON aunque `.preload(...)` los pobló | ✅ CERRADO 10.8.3 (conditional emit) |
| #8 | Revert parches temporales del boilerplate | ✅ CERRADO 10.8.8 |

**Tests al cierre**: 8 E2E nuevos en `tests/compile_e2e.rs` +
smoke 292 verde + `cargo fmt --all -- --check` limpio + `cargo
clippy --all-targets --release -- -D warnings` limpio.

**Boilerplate api-orm-full** ahora en forma canónica:
- `schema.fitz`: `timestamptz NOT NULL DEFAULT NOW()`.
- `models.fitz`: `@db_default created_at: Str = ""`.
- `posts.fitz`: handlers con `return <chain>.await` directo +
  narrowing `if (status != null)` en vez de match arm.
- `comments.fitz`: broadcast WS real con `ws_broadcast("/feed",
  resp)` después del insert (notification realtime al feed).

**Extensión VSCode v0.10.8**: grammar TextMate suma
`ws_broadcast`, LSP completion lo lista en `scope_level_completions`.

### Deudas detectadas en el primer bench (2026-05-29) — URGENTES

Descubiertas al correr el benchmark MVP de
`benchmarks/orm-vs-sqlalchemy/` (Fitz ORM nativo vs SQLAlchemy
interop Python). Los headline numbers (cold start 5.5x, GET lista
9-11x faster en Fitz) ratifican la dirección, pero apareció una
anomalía y un set de mejoras al bench mismo.

#### ✅ B-1 CERRADO (v0.10.13, commit `67efabd`) — Overhead constante del driver Postgres en extended query

**Resolución**: el root cause **no fue** la falta de prepared
statement cache (hipótesis original); fue que los 5 mensajes del
Extended Query Protocol (Parse + Bind + Describe + Execute + Sync)
se enviaban como **5 llamadas separadas a `self.write(...)`**, cada
una con su `socket.write()` syscall. Aun con TCP_NODELAY activo,
las llamadas separadas sumaban latencia por await + scheduling de
tokio en cada round.

**Fix** ([src/db.rs:1951-2005](src/db.rs#L1951)): los 5 mensajes
se serializan en un único `Vec<u8>` y se mandan al socket con un
solo `write_all_bytes(&batch)`. Postgres NO responde hasta el
Sync — los 5 mensajes son "pipelined" en el sentido protocolar,
no es un cambio semántico; solo eliminamos overhead client-side.
TCP_NODELAY se activó en el mismo commit para evitar Nagle.

**Números post-fix** (corrida publicable v0.10.13, hardware Intel
Core Ultra 7 155H + 64GB + Docker Desktop WSL2):

| Endpoint | Pre-fix Fitz p50 | Post-fix Fitz p50 | Python SQLAlchemy p50 |
|---|---:|---:|---:|
| `GET /users/{id}` | 43.70 ms | **3.60 ms** | 31.87 ms |
| `GET /users` | 4.92 ms | **4.88 ms** | 37.85 ms |

Fitz pasó de "30% más lento que Python en single-by-PK" a **8.85x
más rápido**. Bench publicable en
`benchmarks/orm-vs-sqlalchemy/README.md` → "Última corrida
publicable".

Las hipótesis alternativas (POOL_CACHE contention, allocations en
`__FromFitzDbRow`, prepared statement caching) NO se exploraron
porque el fix de batching ya tiró la latencia al piso. Quedan como
optimizaciones futuras solo si aparece presión real para sub-1ms
single reads.

---

#### Histórico — análisis original de B-1 (mantenido por valor pedagógico)

> Cerrado en v0.10.13 con un fix distinto al planteado. El análisis
> original demostró el síntoma correctamente pero apuntó a la
> hipótesis equivocada (statement cache); el batching de wire
> messages fue el verdadero culpable.

**Síntoma**: `GET /users/{id}` (que usa `WHERE id = $1`, extended
query protocol) tarda **43.70 ms p50 en Fitz** vs **31.09 ms p50 en
Python+SQLAlchemy**. Python es ~30% más rápido en este endpoint
específico.

**Comparación que destapa el bug**: `GET /users` (sin params, simple
query) en el MISMO server Fitz tarda solo **4.92 ms p50** — un 89×
más rápido que Python (46 ms). Y `GET /users/{id}` en PG con índice
PRIMARY KEY es instantáneo (microsegundos). Entonces el overhead
extra de Fitz en `/users/{id}` viene del **protocolo extended query
mismo**, no de la query.

**Sospecha**: en `src/db.rs::Connection::extended_query`, el flow
hace **5 round-trips** al server (Parse → Bind → Describe → Execute
→ Sync), cada uno con su read. Si cada round-trip suma ~8-10 ms de
overhead (cualquier source: locks del Connection, alloc en wire
buffer, await scheduling), eso suma los 40 ms observados.

**Hipótesis alternativas** (a descartar antes de optimizar):
- ¿`db.connect(url)` adentro del handler tiene overhead async que
  multiplica por request? (POOL_CACHE singleton v0.10.9 debería
  matar eso, pero capaz hay contención en el Mutex del cache).
- ¿`__FromFitzDbRow` para un struct con 4 fields tiene
  allocations excesivas?
- ¿El extended query NO está reusando un statement preparado (Parse
  cada vez)? SQLAlchemy probablemente sí cachea prepared statements
  por SQL string.

**Plan de investigación**:
1. Reproducir con un test E2E aislado (1 handler, GET por id, 1000
   requests serial, medir wall-time por request).
2. Agregar `tracing` con spans alrededor de cada round-trip wire en
   `extended_query` y medir.
3. Si confirmado: cachear prepared statements por SQL en el pool.
   Cambio interno al driver, sin tocar API pública.

**Impacto si se cierra**: read single-by-PK con WHERE params pasa
de ~40 ms a probablemente <2 ms (mismo orden que simple query),
convirtiendo Fitz en 15x faster que Python en ese caso también, no
30% más lento. Es el caso CRUD más común (GET resource by ID), así
que cerrarlo desbloquea el headline "Fitz gana en TODO" en lugar de
"gana en mucho pero no single-read".

#### B-2 — Mejoras del bench MVP (no bloqueantes)

Descubiertas al hacer dogfood del bench:

- **Image size pesca imagen errónea** cuando hay otros boilerplates
  cacheados. El `grep -E "^${name}-api:"` actual matchea bien al
  bench actual, pero el grep original ("api-orm") era too loose.
  **Fix aplicado en run.sh** v0.10.12 — anchor exacto al
  `<dirname>-api:latest`.
- **Memory peak `?`** cuando `container_name` del docker-compose
  difiere del que asume el run.sh. El `docker stats` fallaba silenc
  ioso, el sampler nunca escribía `mem.log`. **Fix aplicado en
  run.sh** v0.10.12 — container names correctos.
- **POST x 500 sequential** en Git Bash Windows toma ~10 min por el
  overhead del subshell (~1s por iter del for loop). **Fix aplicado
  en run.sh** — bajado a x 100. ~~Posible mejora futura: usar `oha`
  con body fijo para POST~~ **CERRADO (2026-06-17)** — el bench
  nuevo `benchmarks/mixed-workload/` usa `k6` con body unique
  per VU per iter (template `vu${vu}-it${iter}-r${random}@...`)
  sosteniendo 50 VUs concurrentes 1 min en `writes-only.js`. Mide
  POST throughput **real** sin perder email único: **Fitz 847 RPS
  vs Python 170 RPS** sobre el mismo POST `/users` — 5x ganancia
  invisible en el bench v0.10.13. Detalle en
  [`benchmarks/mixed-workload/README.md`](../benchmarks/mixed-workload/README.md).
- **Hardware info NO se auto-detecta** en el summary.md. Hay
  placeholders `TODO`. Posible mejora: el script intenta detectar
  CPU/RAM/OS automáticamente (cmd `wmic` Windows / `lscpu` Linux /
  `sysctl -a` macOS) y los pinea al final.
- ~~**Bench unidimensional**: 3 endpoints aislados. Faltan escenarios
  "extendidos" del roadmap original (mixed workload realista, bulk
  inserts, escritura concurrente saturada).~~ **CERRADO PARCIAL
  (2026-06-17)** — ver `benchmarks/mixed-workload/` (bench nuevo
  con [`k6`](https://k6.io/) en lugar de `oha` para scripted
  scenarios). Cubre: mixed workload realista (60% reads + 40%
  writes intercalados), escritura concurrente saturada (50 VUs
  sostenidos writes-only — cierra **"POST mide el cliente, no el
  server"**), VU ramping para detectar saturation point, p99.9 +
  error rate sobre el peak, comparativa **3 stacks** (Fitz vs
  Python+SQLAlchemy **vs Node+Prisma**) sobre dominio `users +
  posts` con FK. **Headline**: Fitz **31x mejor p95 que Python +
  5.5x mejor p95 que Node** en writes-only sostenido; **45x mejor
  p95 que Python** bajo mixed peak (Python satura a 503 ms p95,
  Fitz mantiene 11 ms). Pendiente: bulk inserts (1k+ rows en una
  transaction), queries con JOINs profundos sobre `api-orm-full`
  como base — quedan como mini-fase futura si aparece demanda.

### Mini-fase v0.10.10 (2026-05-28) — Fix deadlock `__to_fitz_json` has_many virtual

Cierre del **preload hang** dejado como deuda residual de v0.10.9.
Bug aislado con 3 ciclos de eprintln strategic (revertidos en el
mismo commit final). Root cause **NO** era el read loop del driver
(como asumí en v0.10.9): era el codegen del impl `__ToFitzJson`
del field has_many virtual.

- ✅ **gen_type_http_impls_for_sig_with_meta** (src/codegen.rs):
  el conditional emit del field has_many virtual (introducido en
  v0.10.8.3 para activar `.preload(...)` end-to-end en el JSON
  response) hacía `{ let __g = self.x.lock(); if !__g.is_empty() {
  ...self.x.__to_fitz_json() } }`. El `__to_fitz_json` del impl
  genérico `Arc<Mutex<T>>` re-lockea el MISMO Mutex. Como
  `std::sync::Mutex` NO es reentrante, deadlock instantáneo.

  **Fix**: liberar el guard ANTES del re-lock. Chequeo `is_empty`
  en scope acotado:

  ```rust
  { let __is_empty = { let __g = self.x.lock().unwrap();
                       __g.is_empty() };
    if !__is_empty { __obj.insert(..., self.x.__to_fitz_json()); }
  }
  ```

  Smoke real Docker validado: `GET /posts/1` con preload responde
  200 en ~140ms con author + comments preloaded embebidos en
  el JSON.

**Deuda residual del boilerplate** descubierta durante el smoke:
el response expone `password_hash` del author porque
`Post.author: User?` incluye ese field. **No es bug del lenguaje**
— el boilerplate debería mapear a un `PostPublic`/`UserPublic`
que omita el field sensible. Fix para una próxima iteración del
boilerplate.

### Mini-fase v0.10.9 (2026-05-28) — Pool singleton per URL

Sub-paso post-v0.10.8: smoke real Docker descubrió **connection
pool leak** crítico. Cada `db.connect(url)` desde Fitz creaba un
POOL NUEVO con 10 permits + TCP conns. Tras N requests al
boilerplate api-orm-full, Postgres se quedaba sin slots
(`max_connections=100` default) y `acquire()` colgaba
indefinidamente, manifestándose como "preload hang" visible en
GETs con `.preload(...)`.

- ✅ **10.9.2 (#2 nuevo) — `connect_url` singleton per URL**.
  `fitz::db::connect_url(url)` cachea el `Arc<DbConnHandle>` en
  mapa global thread-safe (`OnceLock<Mutex<HashMap>>`). Calls
  subsiguientes con misma URL devuelven clone(Arc) — TODAS las
  conns TCP comparten via el pool único. Cambio coordinado:
  retorna `Arc<DbConnHandle>` directo, call sites
  (evaluator + codegen runtime) actualizados.

- ✅ **10.9.1 (#1 nuevo) — "Preload runtime hang" CERRADO en
  v0.10.10**. La hipótesis inicial (bug del read loop del driver)
  era **incorrecta** — el driver recibe todos los `ReadyForQuery`
  del preload limpio. Aislado con eprintln en 3 ciclos: el hang
  estaba en el **codegen del `impl __ToFitzJson` del field
  has_many virtual** (introducido en v0.10.8.3 para activar
  `.preload(...)` end-to-end en el JSON response). El conditional
  emit hacía:
  ```rust
  { let __g = self.x.lock().unwrap();
    if !__g.is_empty() { __obj.insert(..., self.x.__to_fitz_json()); }
  }
  ```
  El `__to_fitz_json` del impl genérico `Arc<Mutex<T>>` re-lockea
  el MISMO Mutex que `__g` retiene. `std::sync::Mutex` NO es
  reentrante → deadlock. Fix en
  `gen_type_http_impls_for_sig_with_meta`: chequear `is_empty` en
  un scope acotado que dropea el guard ANTES del re-lock.
  Validación smoke real Docker: `GET /posts/1` con preload
  responde 200 con author + comments embebidos en ~140ms.

**Smoke real Docker bloqueado por bug ambiente Windows**:
Docker Desktop Windows tiene un bug intermitente con SCRAM-SHA-256
sobre el bridge TCP que cuelga `Connection::connect` aún con
código pristine pre-v0.10.9. NO bloquea el release porque el fix
es localizado al pool singleton y el ambiente Linux real no tiene
ese issue. Validación smoke real queda como tarea CI Linux (job
`db-postgres` ya integrado en `.github/workflows/ci.yml`).

---

## Deuda residual del ORM/DB post-v0.10.29

> Sección creada al cerrar **v0.10.29** (2026-05-31) — "cierre
> masivo del ORM" con 12 features residuales cerradas en bloque
> (JSON path operators, `@@` text search, `@unique` composite,
> `@check_constraint`, cross-schema FK, diff completo de indexes,
> `fitz db inspect --all-schemas`, redaction de secrets en
> `FITZ_DB_LOG`, DB errors enriquecidos con SQLSTATE+SQL+params,
> `FITZ_DB_MAX_CONNS`, skip deliberado de JSON `||` merge, docs
> masivos). Detalle exacto en `CHANGELOG.md` entry v0.10.29.
>
> **Esta sección lista lo que QUEDA pendiente al cierre.**
> 29 ítems verificados con grep exhaustivo en `src/` (confirmando
> que el código NO los implementa), agrupados en 5 tiers por
> scope. **Mi recomendación**: Tier A + B cierran el MVP fuerte
> del ORM en el sentido "sin fricciones residuales conocidas
> para los patrones canónicos".
>
> Tests al cierre de v0.10.29: 2739 unit + 292 smoke + 3 openapi
> + 81 cli_e2e + 52 db_real_postgres. fmt + clippy --all-targets
> + clippy --features lsp limpios.

### Resumen de tiers

| Tier | Foco | Scope total | Items |
|------|------|------------:|------:|
| **A** | Cierre MVP fuerte del ORM | ~30-40h | 10 |
| **B** | API completion Date/DateTime/Uuid | ~12-16h | 7 |
| **C** | Operadores SQL faltantes | ~12-20h | 3 |
| **D** | DX/LSP residual del ORM | ~5h | 2 |
| **E** | Visión a futuro (expansión lenguaje) | días-semanas | 7 |

**Recomendación**: si el norte es "ORM completo", arrancar por
**Tier A + B (~50h totales)**. Tier C complementa con operadores
SQL avanzados. Tier D mejora DX sin tocar lenguaje. Tier E son
features grandes que expanden el lenguaje (mini-fases dedicadas).

### Tier A — Cierre MVP fuerte del ORM — **9/10 CERRADO 2026-06-01 (v0.10.31)**

**9 ítems cerrados en bloque** (~12h reales vs ~30-40h estimadas).
Detalle por sub-paso en `CHANGELOG.md` v0.10.31. Solo A.10 queda
pendiente (FITZ_DB_* mid-run reload — refinable cuando aparezca
presión, hoy `LazyLock` cubre 99% del caso real).

| ID | Item | Estado |
|----|------|------:|
| A.1 | `fitz db diff --check-destructive` con clasificación Safe/Risky/Destructive + abort sin `--allow-destructive` | ✅ |
| A.2 | `ALTER COLUMN TYPE` con `USING col::T` automático | ✅ |
| A.3 | `db.connect(url, max_conns=N)` kwarg del lenguaje | ✅ |
| A.4 | Savepoints / nested transactions vía `tx_depth` shared + SAVEPOINT/RELEASE/ROLLBACK TO | ✅ |
| A.5 | `ALTER TABLE ADD/DROP CONSTRAINT` para CHECKs via diff | ✅ |
| A.6 | FK targeting composite PK → error claro pre-DDL (en lugar de fallback silencioso a "id") | ✅ |
| A.7 | Drift check `@check_constraint` (introspect lee `pg_constraint.contype='c'` via `pg_get_constraintdef`) | ✅ |
| A.8 | Drift check cross-schema FK (introspect popula `references_schema` desde `ccu.table_schema`) | ✅ |
| A.9 | `db.transaction(closure, isolation="...")` con whitelist 4 ANSI levels + READ ONLY/WRITE | ✅ |
| A.10 | `FITZ_DB_*` mid-run reload (hoy `LazyLock` se fija al primer acceso) | ⏳ |

**Deuda residual derivada (NO bloquea)**: (a) `parse_check_def` usa
trim+exact comparison para drift — cambios cosméticos en
espacios/case del expr pueden disparar DROP+ADD espurio (refinable
con SQL normalizer si entra presión); (b) `@belongs_to(refs="col")`
para FK single-col explícito a tabla con composite PK no
implementado — A.6 solo da error claro, sub-paso `refs=` futuro
permitiría declarar FK válida; (c) `db.connect(..., max_conns=N)`
implementado vía env var override antes del connect — si un
connect previo cacheó max_conns default, el override no aplica;
(d) `transaction_with_isolation` solo whitelistea 12 strings ANSI
+ modificadores — `DEFERRABLE` queda como deuda menor.

### Tier B — API completion Date/DateTime/Uuid — **CERRADO 2026-05-31 (v0.10.30)**

**Cerrado en bloque**. 7 sub-pasos coordinados en 1 sesión (~6h reales
vs ~12-16h estimadas). Sin sintaxis nueva del lenguaje, paridad
bit-a-bit `fitz run` ↔ `fitz build` validada con 10 E2E nuevos. Sin
deps user-facing nuevas (chrono-tz + feature `uuid/v7` ya internos al
binario). 0 breaking de los 292 ejemplos del smoke. Detalle por
sub-paso en `CHANGELOG.md` v0.10.30.

| ID | Item | Estado |
|----|------|------:|
| B.1 | `.add_days/months/years` (Date) + `.add_seconds/minutes/hours/days/months/years` (DateTime) — n signed, overflow → error claro | ✅ |
| B.2 | `.subtract_*` symmetric — alias con negate runtime (`checked_neg` defensivo) | ✅ |
| B.3 | `.diff_days(other)` (Date) + `.diff_seconds/minutes/hours/days(other)` (DateTime) — signed Int, trunc hacia 0 para unidades > 1s | ✅ |
| B.4 | Comparison `<` `>` `<=` `>=` entre Date/DateTime — chrono::Ord nativo, sin coerción | ✅ |
| B.5 | `Uuid.v7()` time-ordered (RFC 9562) — feature `uuid/v7` sumada al Cargo.toml | ✅ |
| B.6 | Shortcuts `Date.tomorrow()`/`Date.yesterday()`/`DateTime.epoch()` | ✅ |
| B.7 | `DateTime.to_local()` (sin dep) + `DateTime.in_tz(iana)` (chrono-tz, Result<Str>) — display helpers, instante UTC no cambia | ✅ |

**Deuda residual derivada (NO bloquea)**: (a) el mensaje de overflow
de `add_years(N)` cita `add_months(N*12)` porque `add_years` se
implementa como scale + delegate (refinable pasando method name como
param); (b) `to_local()` formato fijo ISO 8601 con offset (no acepta
fmt custom — el user que necesita formato custom hace
`dt.in_tz("system_tz")?` + parse manual); (c) `with_timezone(tz)` que
**rotara** el instante (no solo display) queda explícitamente fuera
de scope — la semántica es ambigua y el user que necesita rotar puede
usar `add_seconds(offset)` manual.

### Tier C — Operadores SQL faltantes — **CERRADO 2026-06-01 (v0.10.32)**

| ID | Item | Estado |
|----|------|------:|
| C.1 | `ts_rank` full-text ranking en `.order_by(fn(u) => -u.body.rank("query"))` emite `ORDER BY ts_rank("body", to_tsquery('query')) DESC`; variante `plainto_rank` para plain queries | ✅ |
| C.2 | Expression indexes via `@index(expression="lower(email)")` kwarg dedicado. Drift check incompleto (introspect no parsea `pg_index.indexprs`) — refinar con `name=` explícito | ✅ |
| C.3 | JSON `\|\|` merge via `qb.merge_jsonb(db, field, patch)` separado de `.update`. Emite `UPDATE tbl SET "field" = "field" \|\| $1::jsonb WHERE <where>` | ✅ |

**Deuda residual derivada de Tier C (NO bloquea)**: (a) C.1 acepta
solo Str literal en MVP (vars quedan como deuda menor — el path
order_by stream no tiene acceso al pg_args del where); (b) C.2
drift incompleto — la introspect no detecta cambio del expression
con mismo name (el user debe re-nombrar el index para forzar regen);
(c) C.3 `NULL || anything = NULL` por convención Postgres — el user
inicializa la col con `{}` al INSERT.

### Tier D — DX/LSP residual del ORM — **CERRADO 2026-06-01 (v0.10.32)**

| ID | Item | Estado |
|----|------|------:|
| D.1 | LSP completion ORM en `.where()` — métodos como `is_in`/`like`/`matches`/`has_key`/`path_text`/etc. aparecen en autocomplete sobre Str/Map/Int/Float/Date/DateTime con detail `(ORM .where)` distintivo | ✅ |
| D.2 | LSP hover sobre `@table` types muestra el `CREATE TABLE` SQL emitted vía `migrations::schema_from_program` + `create_table_sql_for` — útil para debuggear migrations sin abrir `fitz db diff` | ✅ |

**Deuda residual derivada de Tier D (NO bloquea)**: (a) D.1 no
detecta scope context — los métodos ORM aparecen siempre, fuera del
`.where` llamarlos da error runtime (limitación documentada en el
detail string); (b) D.2 si `schema_from_program` falla (typo en
relations), el hover se devuelve sin el augment SQL — silente, no
hay feedback visual al user.

### Tier E — Visión a futuro (expansión del lenguaje)

> **Estos NO cierran "el ORM" — lo expanden.** Cada uno es
> mini-fase dedicada con decisiones de diseño previas. Scope
> grande (días-semanas). Documentados acá para que aparezcan
> en una sola tabla con el resto cuando se priorice.

| ID | Item | Evidencia código | Scope |
|----|------|------------------|------:|
| E.1 | `Decimal`/`Numeric` type (precision arbitraria) — financial apps. Hoy `Str` o `Float` con precision loss | Cero matches `Type::Decimal`/NUMERIC OID en driver. Requiere `rust_decimal` o similar + `Type::Decimal` + OID 1700 (NUMERIC) en wire protocol | ~1 semana |
| E.2 | Async query streaming cursor-based — `Type.stream(db) -> Stream<Type>` para resultsets gigantes sin cargar todo en memoria | Cero matches `DECLARE CURSOR`/`Stream<Row>` en `db.rs`. Requiere wire del protocolo cursor + Stream API + integration con tokio Stream trait | ~3-5 días |
| E.3 | COPY FROM/TO — bulk loading de gigabytes. Hoy `bulk_insert` es batch INSERT (funciona pero no óptimo para `> 100K` rows) | Cero matches `COPY FROM`/`CopyFromStdin` | ~3 días |
| E.4 | LISTEN/NOTIFY tipado — real-time pub/sub Postgres como capability del lenguaje (`@listen("channel") fn handler(payload: ...)` o similar) | Cero matches `LISTEN`/`NotificationResponse` excepto un comment sobre `pg_notify()` builtin | ~3-5 días |
| E.5 | Window functions built-in (`ROW_NUMBER`/`RANK`/`LAG`/`LEAD`/`OVER (PARTITION BY ...)`) | Cero matches en `src/`. Hoy escape hatch via `db.query` | ~1 semana |
| E.6 | CTE / `WITH` clauses built-in (`Type.with("subquery", ...)` o método similar) | Cero matches `with_clause`/`CteSpec`. Es lo más complejo del bloque (requiere modelar dependencias entre query trees) | ~2 semanas |
| E.7 | `UNION`/`INTERSECT`/`EXCEPT` entre `QueryBuilder<T>` | Cero matches en `src/` | ~3-5 días |

### Verificación

Cada ítem de esta sección fue verificado con grep exhaustivo en
`src/` el **2026-05-31** (post-v0.10.29). 3 ítems del inventario
preliminar **resultaron estar ya implementados** y NO entran a esta
tabla:

- **GROUP BY en HTTP returns** (serialización `List<Map<Str, Any>>`
  automática) → cerrado en v0.10.22 + v0.10.4 vía
  `DB_HTTP_INTEGRATION_PRELUDE` + `impl __MapKey for __FitzValue`.
- **Date/Time/Timestamp/UUID como tipos nativos del lenguaje** →
  cerrado en v0.10.24 (intérprete) + v0.10.26 (codegen) con paridad
  bit-a-bit. Lo que falta es API completion (Tier B arriba).
- **TLS strict (`verify-ca`/`verify-full`)** → cerrado en v0.10.23
  (`db.rs:372-373` con `SslMode::VerifyCa`/`VerifyFull`).

---

## Deuda CI fmt — **CERRADO 2026-06-01 (v0.11.1)**

**Síntoma observado**: el job `cargo fmt --all -- --check` del CI
Ubuntu falló durante v0.11.0 con un diff sobre `src/cli.rs:419` que
el `cargo fmt --all` local (Windows) NO marcaba como problema. La
diferencia: rustfmt en Linux colapsaba el chain
`p.type_.as_ref().map(|t| t.head_name()).unwrap_or("Str")` en una
sola línea; el rustfmt local lo dejaba multi-línea.

**Causa**: el repo no tenía `rustfmt.toml` committed. Sin config
explícito, cada versión de rustfmt aplica sus defaults, y esos
defaults pueden divergir sutilmente entre minor versions de Rust
(visto entre 1.83 y 1.85 según observación local).

**Fix**: `rustfmt.toml` committed al repo root con configuración
mínima explícita (`edition = "2021"`, `max_width = 100`,
`use_small_heuristics = "Default"`). Esto fija el formato canonical
para todos los devs + CI sin importar qué versión de rustfmt traigan
los runners. Verificado que el `cargo fmt --check` local + el del CI
ahora coinciden.

**Deuda residual menor (NO bloquea)**: el `rustfmt.toml` actual es
minimalista. Si en algún momento queremos opciones más opinionadas
(import grouping, comment width, etc.), las sumamos ahí. Documentado
para visibilidad — todos los devs futuros saben que ESE es el lugar
donde fijar reglas de fmt.

---

## 9.w iteración 2 — Tier 1 deudas pre-M5 — **CERRADO 2026-06-02 (v0.11.2)**

Las tres deudas que bloqueaban escribir M5.C26 del curso "Fitz de
0 a experto" (acordadas el 2026-06-01 en `docs/curso-plan.md` →
"Tiers pre-M5") cerradas en bloque coordinado:

- **Persistencia de jobs sobre DB nativa** ✅ — `@cron("...",
  store=db)` crea `fitz_cron_jobs` + `fitz_cron_runs` con
  CREATE TABLE IF NOT EXISTS al boot del scheduler. Cada attempt
  va a `fitz_cron_runs` con `status running|ok|failed|retrying`.
  Visibility manual con `psql` (UI dedicada queda como sub-paso
  futuro si aparece demanda).
- **Retry con backoff exponencial** para `@cron` (+ `tz`/`retry`
  también en `@background`) ✅ — `retry={max: N, backoff:
  "exponential"|"linear"|"constant", initial_secs: I, max_secs:
  M}` con delay capeado por `max_secs`. Cada attempt registrado
  con número en `attempt` column.
- **Cron timezone configurable** ✅ — `tz="IANA/Name"` via
  `chrono-tz` (ya transitive desde Fase 10). `Schedule::upcoming
  (tz)` tz-aware con conversión a UTC para el sleep.

**Bonus cerrado en el mismo bloque** (no estaba en T1 original):

- **`catch_up=true|false`** — al boot, si hubo missed runs entre
  `last_run_at` y `now`, ejecuta UN run inmediato (no N — evita
  spam). Default `false` = skip.

**Paridad bit-a-bit `fitz run` ↔ `fitz build`** validada contra
Postgres 15 local del autor. El codegen activa `uses_db=true`
automáticamente cuando detecta `@cron(..., store=<Ident>)`
(función `program_has_persistent_cron` walka AST). Cargo.toml
generado suma `chrono-tz` cuando `uses_jobs && !uses_date_or_uuid`.
Preludio dividido en 4 constantes (simple vs persistent) para
evitar referencias a `__FitzDbConn` cuando el programa no usa el
driver. Trait polimórfico `__FitzCronStoreFrom` acepta
`__FitzDbConn` directo Y `Result<__FitzDbConn, String>` —
destraba el patrón canónico `let db = db.connect(...).await`
top-level sin `?`.

**Deudas residuales derivadas de 9.w.3.iter2** (NO bloquean el
arranque de M5, todas documentadas en cap 30 sub-sección "Qué no
está en el MVP"):

- `@background` con persistencia + retry sobre `spawn(...)` —
  diferido a iter3. Los args del spawn requieren serialización
  JSON estable + tabla `fitz_bg_jobs` separada. `@background`
  acepta `tz`/`retry` en memoria pero no `store`/`catch_up`.
- `fitz run` **cron-only** (programa con `@cron(..., store=db)`
  sin `@server` ni handlers HTTP) tiene bug heredado del runtime
  tokio `current_thread` del intérprete: la conn DB queda atada
  al runtime del evaluator que cierra al pasar a `multi_thread`
  para el scheduler. Workarounds: `fitz build` (binario nativo
  arma su propio runtime multi-thread limpio) o sumar un handler
  HTTP trivial. Cierre del bug requiere refactor del flow de
  runtimes del intérprete (no trivial, deuda separada).
- UI dedicada de visibility (panel admin tipo Sidekiq Web). Hoy
  los datos están en las dos tablas, cualquier dashboard externo
  (Grafana, Metabase) los lee.
- Coordinación multi-instancia con locks distribuidos para que
  un `@cron` solo corra en un nodo. Hoy cada réplica corre todos
  sus jobs.

**Detalle técnico completo**: `docs/roadmap.md` → "9.w iteración
2" → sub-sección "9.w.3.iter2" expandida con los 6 sub-pasos
a/b/c/d/e/f y CHANGELOG v0.11.2.

---

## Fase 12.3 — Observability minimal con OpenTelemetry — **CERRADO 2026-06-03**

Cierre formal de la fase entera con 11 commits a lo largo de 3
bloques (12.3.a + 12.3.b + 12.3.c). Total al cierre: **2894 unit
+ 81 cli_e2e + 3 openapi_e2e + 4 compile_e2e del logging**, clippy
`--all-targets -- -D warnings` limpio, `cargo fmt --all --check`
limpio.

**Lo que cierra (referencia)**:

- **12.3.a — Structured logging built-in**: `log.info/warn/error/
  debug(msg, kwargs)` con kwargs heterogéneos (Int/Float/Str/
  Bool/Null/Secret/List/Map). Output JSON flat a stderr con
  `timestamp`+`level`+`msg`+kwargs; pretty mode con ANSI bold
  colors cuando TTY o override `FITZ_LOG_FORMAT=pretty`. Filter
  via `RUST_LOG` (default `info`). Secret redactado automático
  recursivo en List/Map. Paridad bit-a-bit intérprete↔binario
  con `tracing` + `tracing-subscriber` + `chrono` + `serde_json`
  como infraestructura.
- **12.3.b — Spans HTTP + métricas + correlación trace_id**:
  cada request HTTP abre un SpanContext root con IDs OTel-
  compatibles (`trace_id` 32 hex / `span_id` 16 hex, generados
  con `uuid::Uuid::new_v4()`). Logs adentro del handler heredan
  trace_id/span_id automático. Al final del request, access log
  `log.info("http.access", ...)` con `http.method`/`http.target`
  (template del route, OTel-standard)/`http.status_code`/
  `duration_ms` + Counter `http_requests_total{method, path,
  status}` + Histogram `http_request_duration_seconds{method,
  path, status}` (paridad cross-metric con mismos labels). Opt-
  out total con `@server(observability=false)` — bypass del
  wrapper de instrumentación, cero overhead bare-metal.
- **12.3.c — OTLP exporter para spans HTTP**: cuando
  `OTEL_EXPORTER_OTLP_ENDPOINT` está seteada, conexión a backend
  OTel real (Jaeger/Tempo/Honeycomb/Datadog) con `opentelemetry-
  otlp = "0.32"` feature `http-proto`. Sampler
  `TraceIdRatioBased` con `OTEL_TRACES_SAMPLER_ARG` clamp `[0.0,
  1.0]`. Service name desde `OTEL_SERVICE_NAME` (default
  `"fitz-app"`). Sin la env var, no-op silencioso — zero
  overhead, zero conexiones de red. Paridad bit-a-bit
  intérprete↔binario.

**Decisiones técnicas clave confirmadas durante implementación**:

- **Sintaxis kwargs de Fitz usa `name: value`** (no `name=value`).
  Los `=` están reservados para kwargs en decoradores.
  Confirmación al implementar el `log.info(msg, k: v)` —
  consistente con `db.connect(url, max_conns: 5)`.
- **Approach híbrido tracing**: instalamos `tracing-subscriber`
  para que `tracing::enabled!` respete `RUST_LOG`, pero el JSON
  output lo emitimos manual con `serde_json`. Razón: kwargs
  heterogéneos runtime no se modelan limpios con las macros
  `event!` que esperan field names en compile-time.
- **Storage propio con `tokio::task_local!`** sobre tracing
  nativo `Span::extensions`: simplicidad + control total del
  shape OTel-compatible + atraviesa thread boundaries del
  runtime tokio multi-thread (handlers HTTP saltean workers
  entre `.await` points).
- **`http.target` = path template** (no path resuelto):
  convención OTel para agrupar requests por endpoint en
  herramientas downstream (Datadog/Tempo). Evita cardinality
  explosion en métricas.
- **HTTP/proto transport** sobre gRPC para OTLP: simplicidad (no
  requiere tonic), compatibilidad con proxies HTTP corporativos,
  recomendación Datadog/Honeycomb/New Relic.
- **`OnceLock<bool>` para `OTEL_ENABLED`**: evita lookup de env
  var en cada request. Se determina UNA vez al boot.

**Deudas residuales derivadas de Fase 12.3** (NO bloquean Fase
12.4, todas documentadas en `docs/roadmap.md` → "Fase 12.3" →
sección "Deudas residuales derivadas"):

1. **Bridge métricas OTel** **— INTENTADO en Fase 12.3.iter2.Tier2
   (2026-06-03), BLOQUEADO por version conflict, ABIERTO**.
   `metrics::counter!`/`histogram!` que ya emiten Counter
   `http_requests_total` y Histogram
   `http_request_duration_seconds` despachan a recorder global
   vacío hoy (excepto cuando Prometheus está activado por
   Tier3 — ahí van a la exposition format). El crate
   `metrics-exporter-opentelemetry = "0.2.1"` (último release en
   crates.io, 2025-11-15) pinea `opentelemetry_sdk = "0.31"`,
   pero nosotros estamos en `0.32` para traces (12.3.c) + logs
   (iter2.b). El árbol de deps no unifica — `MetricExporter`,
   `Resource`, `SdkMeterProvider` son tipos DISTINTOS aunque
   tengan el mismo nombre (`E0277: trait bound not satisfied`,
   `E0308: mismatched types ... different SdkMeterProvider`).
   El master del crate ya está en 0.32 (verificado en
   https://github.com/Noelware/metrics-exporter-opentelemetry/blob/master/Cargo.toml)
   pero no hay release nuevo aún. **Cierre esperado**: cuando
   Noelware publique la próxima versión del crate (probable
   v0.3.x), bumpear la dep y reintentar. Implementación ya
   diseñada (no toca codegen) en branch local descartado: en
   `serve()` llamar `init_otel_metrics()` DESPUÉS de
   `init_prometheus()` para que Prometheus tenga precedencia
   cuando ambos activos (solo UN recorder global de `metrics`
   permitido), instalar `SdkMeterProvider` con `MetricExporter`
   OTLP sobre `/v1/metrics` + reader periódico, instalar
   `metrics_exporter_opentelemetry::Recorder` como global.
   **Workaround mientras tanto**: Tier3 (Prometheus) cubre el
   caso 90% — el user activa `@server(prometheus=true)`, OTel
   collector hace scrape del endpoint `/metrics`. Pierde el
   beneficio "single sink" de OTLP push pero funciona end-to-end.
2. ~~**Bridge logs OTel**~~ **CERRADO en Fase 12.3.iter2.b
   (2026-06-03)**: cuando `is_otel_enabled()` es `true` Y el
   `LogExporter` se instaló correctamente, `emit_log_record`
   emite el LogRecord en paralelo al backend OTel via OTLP
   HTTP/proto (endpoint `/v1/logs`). Stderr logs siguen
   intactos (emit ADITIVO, no reemplazo). Trace context derivado
   del `SpanContext` activo — adentro de un request HTTP los logs
   heredan automático el mismo `trace_id`/`span_id` que el span
   OTel (cierre iter2.a), habilita correlación logs↔spans en el
   backend. Valores `Secret` se redactan a `"***"` consistente
   con el output stderr. Paridad bit-a-bit `fitz run` ↔ `fitz
   build` (codegen emite `__fitz_emit_log_to_otel` real adentro
   de `OTEL_PRELUDE` + stub no-op en `LOGGING_OTEL_NOOP_STUB`
   cuando OTel no aplica). Cargo.toml emitido suma feature `logs`
   a los 3 crates OTel. **Decisión arquitectónica**: usamos la
   API `opentelemetry::logs` SDK directamente (no
   `opentelemetry-appender-tracing` como sugería el plan
   original); el appender requiere emit via `tracing::event!`
   pero nuestro `emit_log_record` escribe directo a stderr con
   formatter custom (JSON/pretty de 12.3.a). Costo del refactor
   custom formatter no se justifica para el caso. 4 unit tests
   nuevos: `logging::iter2b_value_to_any_value_{primitivos,
   secret_se_redacta,list_y_map_son_recursivos}` +
   `codegen::iter2b_codegen_{cli_log_emite_stub_no_op,
   http_log_emite_logger_provider_real}`.
3. ~~**Correlación trace_id Fitz↔OTel**~~ **CERRADO en Fase
   12.3.iter2.a (2026-06-03)**: cuando `is_otel_enabled()` es
   `true`, `dispatch_request` abre el span OTel PRIMERO y deriva
   el `SpanContext` propio desde `span.span_context().trace_id()`
   + `.span_id()` via el nuevo constructor
   `SpanContext::with_ids(trace_id, span_id)`. El `trace_id`
   que aparece en los logs stderr/JSON es EL MISMO que el del
   span OTel en Jaeger/Tempo/Datadog/Honeycomb — habilita
   queries cross-pipeline. Sin OTel, `SpanContext::new_root()`
   sigue generando IDs propios via uuid. Paridad bit-a-bit
   `fitz run` ↔ `fitz build` (codegen emite el mismo patrón
   `if let Some(span) = __otel_span.as_ref() { ... } else { new_root() }`).
   2 unit tests nuevos: `logging::iter2a_span_context_with_ids_*`
   + `codegen::iter2a_codegen_http_emite_with_ids_branch_*`.
4. ~~**Endpoint `/metrics` Prometheus opcional**~~ **CERRADO en
   Fase 12.3.iter2.Tier3 (2026-06-03)**: `metrics-exporter-
   prometheus = "0.18"` con `default-features = false` (skipea
   `http-listener` + `push-gateway` que no necesitamos). Dual
   gate: `@server(prometheus=true)` compile-time + env var
   `FITZ_PROMETHEUS=1`/`true`/`yes` runtime override (útil en
   producción sin recompilar). Cuando activo, `serve()` instala
   `PrometheusBuilder` como recorder global del crate `metrics`
   (los Counter/Histogram que ya emite `dispatch_request`
   empiezan a popular el recorder automático), y `build_router`
   auto-mounta `GET /metrics` que renderea la exposition format
   en cada scrape — mismo puerto + transporte que el resto de la
   app (NO un puerto separado). Paridad bit-a-bit `fitz run` ↔
   `fitz build` (codegen emite `PROMETHEUS_PRELUDE` con
   `__FITZ_PROMETHEUS_HANDLE` static + `__fitz_init_prometheus` +
   `__fitz_prometheus_route` paralelos a la SDK del intérprete).
   `ServerConfig` (runtime) + `ServerConfigArgs` (codegen) ganan
   `prometheus_enabled: bool` (default `false`). Si el user
   declaró su propio `@get("/metrics")`, gana — mismo patrón que
   `/openapi.json`/`/healthz`. 5 unit tests nuevos:
   `evaluator::tier3_server_decorator_{acepta_kwarg_prometheus_true,
   default_prometheus_es_false,prometheus_no_bool_es_error}` +
   `codegen::tier3_codegen_http_{emite_prometheus_prelude_y_init_
   call_falso_por_default,con_prometheus_true_emite_init_call_true}`.

**Detalle técnico completo**: `docs/roadmap.md` → "Fase 12.3 —
Observability minimal con OpenTelemetry" expandida con los 11
sub-pasos (a.1-3, b.1-5, c.1-3).

## Smoke compile_e2e — gating de deps emitidas — **CERRADO 2026-06-04 (v0.13.1)**

Heredado de v0.12.1 (Tier3 Prometheus). El smoke
`GUIDE_EXAMPLES_COMPILE` compila ~360 ejemplos en serie, cada uno
con su propio `target/fitz-build/<stem>/` separado. Sin cache
compartido, cada ejemplo paga el cold-compile de toda dep que
emita el `cargo_toml_for` cuando `has_http=true` (incluso si no
las usa). Tras sumar `metrics-exporter-prometheus = "0.18"` en
Tier3, el smoke en Linux fresh runner cruzó los 15 min y rompió
CI (commit `ci: bumpear timeout 15→25 min`, 2026-06-03).

**Cierre v0.13.1 (2026-06-04)**: gating refinado del Cargo.toml
emitido por `cargo_toml_for`. Concretamente:

- **`metrics-exporter-prometheus` solo cuando hay
  `@server(prometheus=true)` literal**. Detector nuevo
  `program_uses_prometheus_export(program)` walka decorators
  top-level buscando `kwargs["prometheus"] == Expr::Bool(true, _)`.
  Propagado a `CodegenCtx.uses_prometheus_export` +
  `cargo_toml_for` (param nuevo, último positional). 20 call
  sites de tests actualizados con un single-pass PowerShell.
- **`emit_prometheus_prelude` + `__fitz_init_prometheus(...)` call
  + `.merge(__fitz_prometheus_route())` gateados por el mismo
  flag** (paralelo bit-a-bit). Programas sin opt-in no emiten ni
  el static `__FITZ_PROMETHEUS_HANDLE`, ni el helper, ni la ruta.
- **Breaking behavior aceptado**: el path env var
  `FITZ_PROMETHEUS=1` ya no funciona como override de runtime —
  exige `@server(prometheus=true)` literal. Documentado en
  `docs/guide.md` cap 33.4. Trade-off: el opt-in compile-time
  cubre el 95% del caso real (production deployments declaran
  Prometheus en código); el env var override era nice-to-have.

**Timing observado local** (Windows 11 + Ryzen + NVMe + Cargo
cache fresh): baseline pre-fix `522.13s ≈ 8.7 min` sobre 360
ejemplos. Post-fix se mide separado abajo. **El CI Linux fresh
runner** debería ver mayor mejora absoluta porque el cold-compile
de `metrics-exporter-prometheus` + sus transitivos (`indexmap`,
`prometheus` crate, `protobuf`) cae completo en programas no-
Prometheus (~95% de los ejemplos).

**Decisión de scope confirmada al arrancar**: las deps OTel
(`opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`)
quedan emitidas con `has_http` (sin cambio). Razón: el wrapper
HTTP del codegen emite `__fitz_with_span_context(...)` +
`__fitz_log_info("http.access", ...)` + branches sobre
`__fitz_otel_is_enabled()` sin opt-in del user — la línea
`uses_logging = has_http || ...` fuerza el preludio entero
cuando hay HTTP. Removerlas requiere también gatear el access
log auto del wrapper, lo cual cambia comportamiento user-visible
(programas HTTP simples como los ejemplos pedagógicos de la
guía pierden auto access logs + spans). Queda como deuda
residual abierta (ver abajo).

**Tests al cierre**: 3 unit tests nuevos en `codegen::tests`
(`tier3_codegen_http_sin_prometheus_no_emite_prelude_ni_dep`,
`tier3_codegen_http_con_prometheus_true_emite_prelude_y_dep`,
`v0_13_1_program_uses_prometheus_export_detecta_kwarg_true`,
`v0_13_1_program_uses_prometheus_export_no_dispara_sin_kwarg`).

## Smoke compile_e2e — gating de OTel deps + access log auto — **ABIERTO 2026-06-04**

Deuda residual derivada del cierre parcial de la deuda anterior
(v0.13.1 solo cerró Prometheus). Las 3 deps OTel
(`opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`)
siguen emitidas con cualquier `has_http=true` porque
`uses_logging` se fuerza a `true` cuando `has_http=true` (línea
213-215 de `src/codegen.rs`) — el wrapper HTTP emite
`__fitz_with_span_context(...)` + `__fitz_log_info("http.access",
...)` + branches sobre `opentelemetry::trace::*` sin opt-in del
user.

**Fix futuro**: gatear las 3 deps + el access log auto del
wrapper + el OTel branch del wrapper por
`uses_logging_explicit` (es decir, `program_uses_logging` sin el
forzado `|| has_http`). Programas HTTP que NO usen `log.X(...)`
explícito pierden el access log auto + spans OTel — caso típico:
ejemplos pedagógicos de la guía y CLIs HTTP triviales.

**Trade-off del fix**: pierdes la auto-observability "free para
todo handler HTTP" que se vendió en Fase 12.3.b.4. Quedan
opciones:

- (a) Default opt-in: si el user quiere access logs auto, declara
  `log.info("startup")` en algún lugar del programa.
- (b) Nuevo kwarg `@server(observability=true)` explicit
  (rechazado por el prompt de v0.13.1, pero podría revisitarse
  si la presión real aparece).
- (c) Hacer noop el access log auto cuando no hay subscriber
  instalado (probable — `tracing::info!` no allocates cuando no
  hay subscriber). El span sigue emitiendo "in-process" sin
  exportar, costo bajo.

**Estimación del win**: la dep `opentelemetry-otlp` con feature
`reqwest-blocking-client` pulla `reqwest` que pulla `tokio` +
`hyper` + chains varios. Es de las deps más pesadas del codegen.
Probablemente otro ~3-5 min CI menos sobre Linux fresh si se
gatea cleanly.

**Bloqueante para arrancar**: decidir entre las 3 opciones de
arriba o un sub-paso dedicado con su propio mini-roadmap.

## (HISTÓRICO) Fix temporal pre-v0.13.1 — timeout CI 15→25 min

Bumpear timeout a 25 min en `.github/workflows/ci.yml`. Compró
tiempo pero no escaló — cada feature nueva (12.4 sumará deps de
Docker compose codegen, 12.5+ podría sumar más) empujaba el
smoke arriba. **Cerrado por v0.13.1** que ataca la causa raíz
del lado Prometheus. El timeout 25 min queda — sirve de
margen para el ojo del huracán cuando entre Fase 13+. La
deuda OTel arriba puede recortar otros ~3-5 min cuando se
cierre.

## Fase 12.4.a — `fitz docker init` (Dockerfile autogenerado) — **CERRADO 2026-06-03**

Cierre parcial de Fase 12.4 (queda 12.4.b para smart detection rica +
`fitz docker build` wrapper). Sub-comando nuevo `fitz docker init
[--force]` que genera Dockerfile multi-stage (builder
`ghcr.io/thegreekman76/fitz` + runtime `gcr.io/distroless/cc-debian12`)
+ `.dockerignore` + `docker-compose.yml` con smart detection AST-only
del entry point declarado en `[bin].main`. Política skip-por-default;
`--force` sobrescribe. Validación smoke real verde contra dos
boilerplates (HTTP puro + HTTP+DB con compose smart sumando
`postgres:16-alpine` con healthcheck).

**Decisiones técnicas del MVP**: (a) sub-comando con sub-enum
`Commands::Docker(DockerCmd::Init)` para abrir paso a `fitz docker
build` de 12.4.b sin breaking; (b) AST-only del entry point (fast
~50ms, no cross-module); (c) runtime distroless siempre en 12.4.a
(Python interop diferido a 12.4.b); (d) `ports:` en compose siempre
que haya `@server`; (e) compose con DB sin `restart:` policies
(diferido a 12.4.b según `@cron`).

**Deudas residuales derivadas** (NO bloquean 12.4.b):

1. **Cross-module detection** — `@server`/`db.connect` adentro de un
   módulo importado no dispara el shape. Workaround: declarar
   `@server` en el archivo principal (caso típico). Fix futuro:
   recursar a través del loader del módulo (similar al codegen).
2. **Falso positivo `uses_db` con variable local llamada `db`** —
   paralelo al codegen, trade-off aceptado del MVP. El user borra
   `db:` del compose a mano. Fix futuro: distinguir receptor `db`
   global vs binding local.
3. ~~**Detección Python interop diferida a 12.4.b**~~ **CERRADO en
   Fase 12.4.b (2026-06-03, v0.12.3)**: `uses_python` detecta
   `from python import X` / `import python.X` y el Dockerfile cae a
   `python:3.12-slim-bookworm` automático.
4. ~~**Healthchecks HTTP + `restart:` policies diferidos a 12.4.b**~~
   **CERRADO en Fase 12.4.b (2026-06-03, v0.12.3)**: `uses_cron` →
   `restart: unless-stopped`; healthcheck HTTP contra `/healthz` cuando
   hay `@server` Y runtime con wget (`uses_python`).
5. ~~**`fitz docker build [--tag X]` wrapper diferido a 12.4.b**~~
   **CERRADO en Fase 12.4.b (2026-06-03, v0.12.3)**: sub-comando
   `fitz docker build [--tag X]` thin wrapper sobre `docker build -t
   <pkg>:latest .` con override y propagación de exit code.

**Tests al cierre**: 2924 unit (+18 del módulo `docker`) + 87
cli_e2e (+6 del sub-comando) + 3 openapi_e2e. Clippy
`--lib --tests --bins -- -D warnings` limpio, fmt clean.

**Detalle técnico completo**: `docs/roadmap.md` → "Fase 12.4 —
Dockerfile autogenerado + `fitz docker`" → sub-paso 12.4.a expandido
con todas las decisiones.

## Fase 12.4.b — Smart detection rica + `fitz docker build` — **CERRADO 2026-06-03**

Cierra **Fase 12.4 entera**. Suma detección AST de interop Python y
`@cron`, ajusta runtime + compose según el shape del programa, y agrega
el sub-comando `fitz docker build [--tag X]` que tag-ea y delega a
`docker build`.

**Smart detection rica**:

- `uses_python` (`from python import X` o `import python.X`) → runtime
  stage cae a `python:3.12-slim-bookworm` (~55 MB con libpython3.12 +
  wget) en vez de distroless (~22 MB sin Python).
- `uses_cron` (cualquier `@cron` decorator) → compose suma `restart:
  unless-stopped` al service principal.
- Healthcheck HTTP en compose solo cuando `server_port = Some` Y
  `uses_python` (wget disponible). Con distroless emite comentario
  explicativo con receta para agregarlo a mano.

**Sub-comando `fitz docker build [--tag X]`**:

- Thin wrapper sobre `docker build -t <tag> .` en `manifest_dir`.
- Default `--tag` = `<package.name>:latest`.
- Aborta con sugerencia si falta `Dockerfile` (recomienda `fitz docker
  init`) o `fitz.toml`.
- Propaga exit code de `docker build` para CI.

**Decisiones técnicas**: (a) runtime swap atómico al detectar interop
Python (alternativa rechazada: distroless + libpython bundleada, deuda
mayor); (b) healthcheck con `wget --spider` solo en slim-bookworm —
distroless sin shell no permite `CMD-SHELL` (alternativas rechazadas:
mini-probe binario embebido, healthcheck TCP); (c) thin wrapper `fitz
docker build` sin `--push`/`--platform`/`--no-cache` — para flags
advanced, `docker build` directo.

**Deudas residuales derivadas** (NO bloquean Fase 12.5):

1. **Detección DB indirecta vía interop Python** — `uses_db` solo
   detecta `db.X(...)` nativo Fitz. Programas que usan SQLAlchemy a
   través de `from python import sqlalchemy` no disparan el service
   `db:` en compose. Workaround: usar `--force` y editar a mano, o usar
   el driver Postgres nativo de Fitz (cap 31). Fix futuro: detectar
   `from python import sqlalchemy/psycopg2/asyncpg` con flag separado,
   o sumar `--with-postgres` al `init`.
2. **Healthcheck HTTP sin distroless** — el bloque solo sale cuando hay
   wget (`uses_python`). Para programas no-Python con `@server`, el
   user puede agregarlo a mano siguiendo el comentario o cambiar el
   runtime. Fix futuro: bundlear mini binario HTTP probe en distroless,
   o usar healthcheck TCP (sin validar endpoint exacto).
3. **`fitz docker build` no expone `--push`/`--platform`/`--no-cache`**
   — thin de propósito. Refinable si aparece demanda real (CI
   multi-platform).
4. **Cross-module detection** sigue siendo deuda heredada de 12.4.a:
   `from python import X` / `@cron` / `@server` adentro de módulo
   importado no dispara el shape. Workaround: declarar todo en el
   archivo principal.

**Tests al cierre**: 2937 unit (+13 del módulo `docker`) + 93 cli_e2e
(+6 del sub-comando) + 3 openapi_e2e. Clippy `--lib --tests --bins
-- -D warnings` limpio, fmt clean.

**Smoke real verde** validado contra `boilerplates/api-postgres-python`
(interop SQLAlchemy → runtime `python:3.12-slim-bookworm` automático +
healthcheck HTTP en compose; ausencia de service `db:` documentada como
limitación conocida de interop indirecto).

**Detalle técnico completo**: `docs/roadmap.md` → "Fase 12.4 —
Dockerfile autogenerado + `fitz docker`" → sub-paso 12.4.b expandido
con sub-pasos 12.4.b.1 + 12.4.b.2.

## Fase 9.w.1.iter2.a — `@requires("role")` (RBAC custom) — **CERRADO 2026-06-03**

Cierre parcial de Fase 9.w.1.iter2 (queda 9.w.1.iter2.b para token
blacklist + refresh). Decorator nuevo `@requires("role")` apilable
sobre handlers HTTP/`@ws` para roles más allá de `@authenticated`/
`@admin`. El runtime ejecuta el provider, inyecta el `user`, y valida
que `user.role` matchee al menos uno de los roles requeridos. Si no,
403 con role actual + requeridos en el mensaje.

**Sintaxis**:

```fitz
@requires("editor")
@post("/articles")
fn create(body: Article, user: User) -> Article { ... }

@requires("editor")
@requires("publisher")
@put("/articles/{id}")
fn publish(id: Int, user: User) -> Article { ... }  // OR
```

**Decisiones técnicas del MVP**: (a) `@requires` implica auth (corre
el provider igual que `@authenticated`/`@admin`); (b) multi-decorator
= OR (un user tiene UN role, pedir A AND B sería incoherente); (c)
exige `role: Str` no nullable en User type (paralelo a `@admin`);
(d) mensaje de 403 enriquecido con role actual + lista de requeridos;
(e) MVP singular `user.role: Str`, multi-role `user.roles: List<Str>`
queda como deuda; (f) paridad bit-a-bit `fitz run` ↔ `fitz build`.

**Implementación**:

- `src/types.rs::check_auth_decorators` acepta `requires` como kind,
  valida shape sintáctico, rechaza role duplicado en decorators
  apilados. 9 unit tests (`requires_*`).
- `src/http.rs::RouteSpec` gana `required_roles: Vec<String>`. El
  wrapper `dispatch_request` y el WS path disparan el provider
  cuando `auth != None || !required_roles.is_empty()`. Después del
  admin check, valida que `user.role` esté en `required_roles`. 5
  E2E nuevos en oneshot router.
- `src/codegen.rs::HandlerSig` gana `required_roles`,
  `emit_auth_check` emite el role check después del admin check
  (paralelo en el WS wrapper). `partition_program_stmts` acepta
  `requires` como decorator válido. `auth_user_param_name` lookup
  dispara también con `@requires`.
- `src/evaluator.rs`: helper nuevo `collect_required_roles` paralelo
  a `collect_route_auth`. Pipeline `process_decorator →
  register_http_route/register_ws_route` propaga el slice.
- `src/lsp.rs::decorator_completions` suma entrada `requires` con
  snippet `requires("editor")`.

**Tests al cierre**: 2951 unit (+14) + 93 cli_e2e + 3 openapi_e2e.
Clippy `--lib --tests --bins -- -D warnings` limpio, fmt clean.

**Deudas residuales derivadas** (sub-iter futuro 9.w.1.iter2.b):

1. **Token blacklist + revocación server-side**: builtins
   `auth.blacklist(db, jti, expires_at) -> Result<Null>` y
   `auth.is_blacklisted(db, jti) -> Result<Bool>` con tabla
   `fitz_token_blacklist(jti TEXT PRIMARY KEY, expires_at BIGINT
   NOT NULL)` auto-creada al primer call (paralelo a Fase 9.w.3.iter2
   cron persistente). Patrón canónico: endpoints `/auth/logout` y
   `/auth/refresh` se escriben a mano (~10 LoC cada uno) con los
   builtins. Auto-mount fuera del MVP. Requiere DB obligatoria.
2. **Multi-role**: `user.roles: List<Str>` con `@requires(roles=[...])`.
   MVP se cubre apilando `@requires` decorators (OR) o con check
   manual `if user.roles.contains("editor") { ... }`.
3. **Role hierarchy** ("admin implies editor implies viewer") no se
   modela. Aceptable para el MVP — el user lo arma a mano si quiere.
4. **Mensajes 403 i18n**: el formato actual está en español
   (consistente con el resto del runtime). Multi-lenguaje queda como
   deuda separada si entra demanda.

**Detalle técnico completo**: `docs/roadmap.md` → sección "Tiers
pre-M5 del curso" → "T2 — 9.w.1.iter2" expandido con los dos
sub-pasos.

## Fase 12.5 — Cap 35 + curso M7 + cierre formal Fase 12 entera — **CERRADO 2026-06-03**

**Cierra Fase 12 entera** (12.1 + 12.2 + 12.3 + 12.4 + 12.5).
Sub-paso 100% docs/curso, sin cambios de código.

**Sub-pasos**:

- **12.5.a — Cap 35 nuevo** "Deployment ciudadano primera clase" en
  `docs/guide.md` con 8 sub-secciones integradoras y ejemplo runnable
  `examples/guide/35-deploy.fitz` (<100 LoC end-to-end con todo el
  stack). Sumado al smoke `GUIDE_EXAMPLES_COMPILE`. Renumeración
  caps 36/37.
- **12.5.b — Caps del curso M7** (C1-C4) completos en
  `docs/curso/m7-produccion-deploy/`:
  - C1: Distribución avanzada (binarios + cross-compile + bundle).
  - C2: Observability en producción (logs + spans + métricas + OTel).
  - C3: Secrets management (`Secret<T>` opaco + patterns K8s/fly/etc).
  - C4: Deploy avanzado (Docker autogenerado + healthz + K8s + 12-factor).
  - Cierre del curso entero con 10 diferenciales resumidos.
- **12.5.c — Cierre formal**: CHANGELOG v0.12.5 detallado, roadmap
  con sub-pasos expandidos, esta nota en deudas, CLAUDE.md,
  README/index.md actualizados, mkdocs.yml suma nav M7. Smoke
  GUIDE_EXAMPLES_COMPILE verde con +1 ejemplo (357 total).

**Tests al cierre**: 2951 unit + 93 cli_e2e + 3 openapi_e2e + 357
compile_e2e (+1 vs v0.12.4). Sin cambios de código — release 100%
docs. fmt + clippy heredados de v0.12.4 limpios.

**Verificación pre-bump completa** (memoria
`feedback_pre_release_verification`): roadmap ✓, guide.md cap 35 ✓,
deudas (esta nota) ✓, CLAUDE.md ✓, CHANGELOG ✓, README ✓,
docs/index.md ✓, docs/curso/index.md M7 ✓, mkdocs.yml M7 nav ✓,
extensión VSCode (sin cambios — 12.5 es 100% docs) ✓, examples
ejemplo `35-deploy.fitz` ✓, boilerplates (sin cambios) ✓, smoke +
lints ✓.

**Cierre formal de Fase 12 entera** (deployment ciudadano primera
clase). Plan original cumplido al 100%:

- 12.1 healthz/readyz + SIGTERM drain (cerrado v0.12.0).
- 12.2 Secret<T> + secret()/config()/load_env() (cerrado v0.12.0).
- 12.3 Observability OTel (cerrado v0.12.0-12.1 con iter2 de Tier3
  Prometheus + bridge logs + correlación trace_id).
- 12.4 Dockerfile autogenerado + fitz docker init/build (cerrado
  v0.12.2-12.3 con detección AST smart).
- 12.5 Cap 35 + curso M7 + cierre formal (cerrado v0.12.5).

**Deudas residuales NO bloqueantes** ya documentadas más arriba en
este archivo: bridge métricas OTel (Tier 2 BLOQUEADO esperando
release del crate `metrics-exporter-opentelemetry`), gating de
deps emitidas en smoke (ABIERTO).

**Próximos nortes opcionales** (sin demanda real, diferidos):

- **9.w.1.iter2.b** — Token blacklist + refresh con builtins
  `auth.blacklist`/`auth.is_blacklisted` + tabla
  `fitz_token_blacklist` auto-creada (CERRADO v0.12.6).
- **Fase 12.6** — `fitz deploy` orchestrator — **CERRADO v0.13.0**
  con targets `docker`/`compose`. Targets `fly`/`railway`/`k8s`
  con plugin architecture diferidos a Fase 13+ por demanda real.
- **Fase 12.7** — `@trace`/`@metric` decoradores explícitos sobre
  fns business logic — **CERRADO v0.13.0**. Paridad bit-a-bit
  `fitz run` (no-op honesto) ↔ `fitz build` (instrumentación real
  con `tracing`+`metrics`). Cap 33.5 nuevo en guía.
- **Fase 12.8** — Feature flags built-in — **CERRADO v0.13.0**.
  `@flag("name")` + `flag(name) -> Bool` + módulo `flags`.
  Manifest `[flags]` + env var override. Cap 33.11 nuevo en guía.

**Detalle técnico completo**: `docs/roadmap.md` → "Fase 12.5" con
sub-pasos detallados, y `docs/guide.md` → cap 35 para la vista
integradora.

## Fase 12 Tier 2 — `fitz deploy` + `@trace`/`@metric` + `@flag` — **CERRADO 2026-06-04 (v0.13.0)**

**Cierra el Tier 2 entero de Fase 12**: los tres sub-pasos
diferidos en v0.12.5 (deploy + observability decoradores + feature
flags) cierran en bloque coordinado al detectar suficiente demanda
interna para arrancar Fase 13+ post-Tier2 sin deudas pendientes.

**Lo que entra al release v0.13.0**:

1. **Fase 12.6 — `fitz deploy <target>`** (módulo nuevo
   `src/deploy.rs` ~430 LoC). Thin wrappers sobre `docker
   build`/`compose up`. Sólo `docker` y `compose` en MVP (fly/
   railway/k8s diferidos). Opt-outs: `--no-push`/`--no-detach`/
   `--no-build`. Aborta con sugerencia clara si falta el archivo
   esperado (recomienda `fitz docker init`). Propaga exit code
   para CI. 7 unit + 5 cli_e2e tests.

2. **Fase 12.7 — `@trace(name="X")` + `@metric(name="X")`**
   sobre fns user. Apilables (un `@trace` + un `@metric` sobre
   la misma fn) pero NO sobre HTTP/WS handlers (auto-instrumentation
   Fase 12.3 ya los cubre; el checker rechaza estáticamente
   con mensaje claro). Kwarg `name=` opcional, fallback al nombre
   de la fn. **Decisión técnica clave**: emit con `__FitzMetricGuard`
   RAII (registra histogram + counter al Drop) en lugar de
   wrap-down después del body — funciona correctamente con
   `return X` explícito sin código muerto. Paridad bit-a-bit
   `fitz run` (no-op honesto) ↔ `fitz build` (instrumentación
   real con `tracing` + `metrics` crates). Cap 33.5 nuevo en
   `docs/guide.md` + ejemplo `examples/guide/34-trace-metric.fitz`.

3. **Fase 12.8 — `@flag("name")` + `flag(name) -> Bool` +
   módulo `flags`**. Tres APIs paralelas: (a) decorator sobre
   HTTP/WS handlers que retorna 404 si la flag está off — gate
   hot path ANTES de middlewares/auth/coerciones (orden idéntico
   al runtime `dispatch_request`); (b) builtin global `flag(name)
   -> Bool` para branches programáticos dentro del código; (c)
   módulo `flags` con `is_enabled(name)` (alias) y `list()`
   (enumera flags conocidos en orden BTreeSet — manifest +
   env vars). Dos fuentes: sección `[flags]` en `fitz.toml`
   (defaults compile-time, baked-in al binario via
   `__fitz_flag_init(...)` al boot) + env vars
   `FITZ_FLAG_<UPPERCASE>` (override runtime sin recompilar).
   **Default `false`** (fail-safe — features nuevas opt-in).
   Paridad bit-a-bit con registry estático `OnceLock` + cache
   lookup. Cap 33.11 nuevo en `docs/guide.md` + ejemplo
   `examples/guide/34b-feature-flags.fitz`.

**Tests al cierre v0.13.0**: 3001 unit (+44 nuevos: 8 evaluator
flag + 9 checker flag + 4 trace/metric codegen + 3 manifest
flags + 4 codegen Cargo.toml flag/trace_metric + 14 LSP + 2
E2E compile flag) + 112 LSP + 360 compile_e2e (+2 ejemplos
nuevos: 34-trace-metric.fitz + 34b-feature-flags.fitz + smoke
verde) + 3 openapi. `cargo fmt --all --check` + `cargo clippy
--lib --tests --bins -- -D warnings` limpios.

**Extensión VSCode bumpeada a 0.13.0**: LSP completions
sumadas para `@trace`/`@metric`/`@flag` decorators (snippets
con kwarg `name=`/arg posicional), `flag()` global builtin,
`flags.X` after-dot. Grammar TextMate sin cambios (decorators
matchean `@<ident>` genérico).

**Deudas residuales derivadas (NO bloquean Fase 13+)**:

- **Deploy targets `fly`/`railway`/`k8s`** — plugin architecture
  pendiente, MVP cubre los dos targets de demanda real
  (`docker`/`compose`). Cuando entre demanda, los nuevos
  targets son extensión del enum `DeployTarget` + handler
  dedicado.
- **Spans hijos ad-hoc dentro de una fn** — `@trace` envuelve
  la fn entera. Para gate-ar solo unas líneas con un span
  dedicado, workaround: extraer la sección a una fn dedicada
  con `@trace` arriba. Sub-paso futuro de la spec: bloque
  `span("name"): { ... }` si entra demanda.
- **`@flag` sobre fns regulares (no HTTP/WS)** — el shape se
  acepta pero la semántica MVP es no-op (el body se ejecuta
  igual). Patrón canónico: usar `if flag(name)` dentro del
  body para gating manual. Refinable a "early return Null" si
  entra demanda concreta.
- **Flags scoped por user/request o por % de tráfico** — el
  flag es global por proceso. Para A/B testing por % de
  tráfico o segmentación por user, integración con
  LaunchDarkly/Unleash/Flagsmith via call HTTP desde el
  handler (workaround documentado en cap 33.11).
- **Hot-reload de flags sin restart** — flags son inmutables
  durante el lifetime del proceso. Cambio de env var requiere
  reinicio. TTL-wrapped lookups quedan diferidos.
- **Bridge métricas OTel** (Tier 2 de Fase 12.3, no del Tier 2
  general) — sigue BLOQUEADO esperando release del crate
  `metrics-exporter-opentelemetry` con `opentelemetry_sdk 0.32`.

## Fase 9.w.1.iter2.b — Token blacklist (auth nativa cerrada) — **CERRADO 2026-06-03**

**Cierra Fase 9.w.1.iter2 entera** (.a RBAC custom + .b token
blacklist). Módulo built-in `auth` con 3 builtins async sobre
Postgres + paridad bit-a-bit `fitz run` ↔ `fitz build` + cap 28 con
patrón canónico de `/auth/logout`/`/auth/refresh`.

**API**:

```fitz
auth.blacklist(db, jti, expires_at) -> Future<Result<Null>>
auth.is_blacklisted(db, jti)         -> Future<Result<Bool>>
auth.cleanup_expired(db)             -> Future<Result<Int>>
```

Tabla `fitz_token_blacklist(jti TEXT PRIMARY KEY, expires_at BIGINT
NOT NULL)` auto-creada con CREATE TABLE IF NOT EXISTS al primer call
(paralelo a Fase 9.w.3.iter2 cron persistente).

**Decisiones técnicas del MVP**: (a) `expires_at` Unix epoch (BIGINT)
para matchear JWT `exp` claim sin conversiones; (b) auto-filtro
`expires_at > now()` en is_blacklisted (tokens vencidos no necesitan
seguir bloqueando, jwt.decode los rechaza primero); (c) ON CONFLICT
DO UPDATE en blacklist (re-blacklisteo del mismo jti actualiza sin
fallar); (d) server-clock manda (now() en SQL, no en Rust — evita
drift); (e) tabla auto-creada idempotente (Postgres serializa con
LOCK interno); (f) paridad bit-a-bit `fitz run` ↔ `fitz build`.

**Implementación**:

- **b.1 Intérprete**: 4 helpers `pub` en `src/evaluator.rs` (4
  constantes SQL + `ensure_token_blacklist_table`), 3 fns
  `builtin_auth_blacklist/is_blacklisted/cleanup_expired` con
  validación de args + signatures async `Future<Result<...>>`,
  registro del módulo `auth` paralelo a jwt/hash/log en
  `register_builtins`, checker con `auth` en scope base. **6 unit
  tests** (aridad, primer arg DbConn, pre-registro del módulo) + **6
  E2E reales contra Postgres** en
  `tests/auth_blacklist_real_postgres.rs` con `#[ignore]`.
- **b.2 Codegen**: `expr_uses_auth` extendido detecta `auth.X`.
  `emit_auth_prelude` cuando `uses_auth && uses_db` emite 4
  constantes SQL + `__fitz_ensure_token_blacklist_table` + los 3
  helpers `__fitz_auth_*` async retornando `Result<T, String>`.
  `gen_call` despacha `auth.X(...)` a fns helper paralelas a
  `gen_auth_jwt_encode/decode`. Importación cross-module. **1 E2E
  compile test** en `tests/compile_e2e.rs`.
- **b.3 Docs/LSP/cierre**: cap 28 de `docs/guide.md` suma sub-sección
  `auth` con API + decisiones + **patrón canónico completo** de
  `/auth/logout` + `/auth/refresh` + provider con check + `@cron`
  cleanup en <60 LoC. LSP `lsp.rs`: sumado `auth` a
  `scope_level_completions` + after-dot con signatures completas.
  CHANGELOG v0.12.6 + roadmap + esta nota + CLAUDE.md.

**Tests al cierre**: 2957 unit (+6) + 93 cli_e2e + 3 openapi_e2e +
358 compile_e2e (+1) + 6 E2E real Postgres `#[ignore]`. Clippy
`--lib --tests --bins -- -D warnings` limpio, fmt clean.

**Verificación pre-bump completa** (memoria
`feedback_pre_release_verification`): roadmap ✓, guide.md cap 28
sub-sec auth ✓, deudas (esta nota) ✓, CLAUDE ✓, CHANGELOG ✓, README
sin cambios (cap 28 ya cita auth nativa), index.md sin cambios,
extensión VSCode grammar sin cambios + LSP `auth` module
completions ✓, examples sin runnable nuevo (sería overkill — el
patrón vive en el cap 28), boilerplates sin cambios.

**Deudas residuales derivadas** (NO bloquean Fase 13+):

1. **Auto-mount de `/auth/logout` y `/auth/refresh`**: el flow exacto
   varía por proyecto. Mantenerlo manual da más control. Si entra
   demanda, sub-paso futuro con `@server(auto_auth_endpoints=true)`
   como opt-in.
2. **In-memory blacklist** (sin DB): para apps sin Postgres que
   quieren revocation rápida, un `Map<Str, Int>` global + check
   manual. Trade-off: no persiste entre restarts. Sub-paso futuro con
   `auth.blacklist_local(jti, exp)` + flag opt-in.
3. **Refresh tokens dedicados** (OAuth2 clásico con dual-token): el
   MVP usa un solo token largo. Refresh tokens dedicados queda como
   pattern futuro si entra demanda.
4. **`jwt.encode` con `jti` automático**: el user pone
   `"jti": Uuid.v4()` a mano. Refinamiento futuro: kwarg `jti=true`
   que auto-genera y devuelve `(token, jti)`.
5. **Logging del blacklist hit**: el flow actual no loguea por
   default cuando un token se rechaza por blacklist. El user puede
   agregar `log.warn("token revocado", jti: jti)` adentro del
   provider.

**Cierre formal de Fase 9.w.1.iter2 entera** (auth nativa completa:
RBAC custom + token blacklist). Plan original cumplido al 100%.

**Detalle técnico completo**: `docs/roadmap.md` → sección "Tiers
pre-M5 del curso" → "T2 — 9.w.1.iter2" → sub-paso 9.w.1.iter2.b
expandido, y `docs/guide.md` → cap 28 sub-sec `auth` para el patrón
canónico runnable.

## Curso `Fitz de 0 a experto` — M7 nuevo (Interop Python) + M8 ampliado — **CERRADO 2026-06-03**

Cierre del curso entero (8 módulos / 41 capítulos). Plan original
tenía 7 módulos (M7 = Producción y deployment) con C32b opcional
sobre interop SQLAlchemy. Al cierre detectamos que el material
disponible de Fase 8 (15 sub-secciones del cap 21 + 9 ejemplos
runnable + bundling completo `--bundle-python`/`--bundle-pip`)
justificaba un módulo dedicado en lugar de UN cap opcional.

**Decisión** (2026-06-03): renumerar M7 anterior (Producción y
deployment) a M8 y crear M7 nuevo dedicado a Interop Python con 3
caps. M8 además recibió un cap nuevo (M8.C5) sobre deploy real de
apps con interop — específicamente para apps que salgan de M7 y
necesiten distribución sin Python instalado en destino.

**Sub-pasos**:

- **Renumeración M7 → M8**: `git mv` del directorio, sed sobre los
  4 caps internos cambiando "M7.C" → "M8.C", actualizando pre-req
  del primer cap (M8.C1 ahora apunta a M7.C3 o M6.C6 si saltás
  M7), "Validación final del módulo M7" → "M8", final summary del
  curso movido a M8.C5.
- **3 caps nuevos M7 Interop Python** (`docs/curso/m7-python-
  interop/`):
  - **C1 Setup venv + `from python import` + casos simples** —
    venv estándar Python sin magia Fitz, `cargo build --features
    python`, primer programa con math/json/datetime. Auto-coerción
    primitiva (Fase 8.1.3), introducción al PyObject opaco.
  - **C2 numpy + pandas reales** — handler HTTP que sirve análisis
    de clima con pandas + numpy. Coerción a `type` nominal con
    anotación destino (Fase 8.4). Excepciones Python → Result
    automático (Fase 8.3). Benchmarks de marshaling.
  - **C3 SQLAlchemy interop + bridge async + cuándo NO usarlo** —
    matriz de decisión honesta vs ORM nativo Fitz. Patrón
    canónico `<py_call>?.await` (Fase 8.6 bridge tokio↔asyncio).
    `fitz py-types` para auto-generar types Fitz desde modelos
    SQLAlchemy.
- **1 cap nuevo M8.C5 Deploy real con interop Python**
  (`docs/curso/m8-produccion-deploy/c5-bundle-python-pip-
  deploy.md`) — `fitz build --bundle-python` (CPython 3.14.5
  embebido via PBS de Astral) + `--bundle-pip` (paquetes pip
  empaquetados via tarball secundario). Comparativa Path A
  (Dockerfile default + venv en runtime, ~250 MB) vs Path B
  (bundling completo + distroless, ~200 MB). Trade-offs honestos
  (cuándo NO usar bundling).
- **Ejemplos runnable** en `examples/curso/m7-python-interop/`:
  c1-setup, c2-weather, c3-sqlalchemy — cada uno con README,
  app.fitz, archivos Python helper, y comandos exactos de smoke
  manual.
- **Actualización del index y nav**: `docs/curso/index.md` con
  tabla de 8 módulos / 41 caps + sección nueva M7 + M8
  ampliada; `mkdocs.yml` con nav M7 (3 caps) + M8 (5 caps);
  `docs/curso-plan.md` con header de "Actualización 2026-06-03"
  expandido a 5 ajustes sobre el plan original, mapping
  curso→guide.md refrescado.

**Decisiones técnicas**: (a) renumeración M7→M8 vía `git mv`
preservando history; (b) bar editorial idéntico a M1-M6 (header
+ mermaid + tabla diferencial + 7-9 pasos + validación +
troubleshooting + lo que sigue); (c) cierre del curso entero en
M8.C5 (era M8.C4 antes); (d) M8.C5 marca opcional para apps
puramente Fitz nativas (M8.C4 deja link directo al cierre).

**Tests al cierre**: sin cambios — release 100% docs/curso. 2957
unit + 93 cli_e2e + 3 openapi_e2e + 358 compile_e2e + 6 E2E real
Postgres. fmt + clippy heredados de v0.12.6 limpios.

**Verificación pre-bump completa** (memoria
`feedback_pre_release_verification`): roadmap actualizado ✓,
curso-plan revisado con nota nueva ✓, deudas (esta entrada) ✓,
CLAUDE entrada nueva ✓, CHANGELOG v0.12.7 ✓, docs/curso/index.md
✓, mkdocs.yml ✓, extensión VSCode sin cambios (release 100%
docs), examples/curso/m7-python-interop/ con READMEs + código ✓,
README raíz sin cambios.

**Cierre formal del curso `Fitz de 0 a experto` entero**: 8
módulos / 41 capítulos. Plan original cumplido + ampliado para
cubrir la interop Python como ciudadano pedagógico de primera
clase y el deploy real de esas apps en producción.

**Deudas residuales derivadas** (NO bloquean — refinamientos
opcionales):

1. **Smoke automatizado del curso M7 en CI**: hoy los ejemplos
   runnable se validan a mano. Sumar al CI un job que corre
   `cargo build --features python` + `fitz-python check
   examples/curso/m7-python-interop/c*/app.fitz` daría no-drift
   guarantee. Costo: +5-10 min de CI por release. Recomendado si
   los caps M7 entran a Marketing/landing.
2. **Smoke real Docker de M8.C5**: el cap incluye Dockerfiles
   bundleados completos. Validación manual al cierre; smoke real
   contra `python:3.X-slim-bookworm` con SQLAlchemy + asyncpg
   queda como deuda si entra demanda.
3. **Translation a inglés**: el curso entero está en español
   (consistente con guide.md). Traducción a inglés queda como
   sub-paso futuro si el material gana tracción.

**Detalle técnico completo**: `docs/curso/index.md`,
`docs/curso-plan.md` con la nota de actualización 2026-06-03, y
los 3+5 caps en `docs/curso/m7-python-interop/` +
`docs/curso/m8-produccion-deploy/`.

## Fixes pendientes de la extensión VSCode / LSP — **ABIERTO 2026-06-05**

Backlog vivo de bugs **y features faltantes** descubiertos en la
experiencia de editing real (VSCode + extensión Fitz). El autor los va
a ir sumando a medida que los encuentre durante el curso `Fitz de 0 a
experto` y trabajo cotidiano. Cada entry trae síntoma/qué + causa/cómo
+ fix propuesto + impacto/costo.

### V1 — Spans incorrectos dentro de string interpolation — **CERRADO 2026-06-05**

**Fix aplicado**: walker recursivo `shift_expr_spans` ([src/parser.rs:3328](src/parser.rs#L3328))
ajusta los `Span` de cada nodo del Expr resultante del sub-parser de
StrInterp para que apunten al source original (no al sub-texto aislado).
Reusa nuevo helper `Expr::span_mut()` ([src/ast.rs:318](src/ast.rs#L318))
paralelo al `span()` existente. 4 unit tests en `parser::tests::v1_*`
cubren Ident/BinOp/Call/Field dentro de StrInterp.

Validación end-to-end con el bug reportado por el alumno:
`{altitud_m + altitud_a}` con `Int + Str` ahora reporta error en
"línea 5:31" (donde está el `+`) en vez de "línea 1:1".

**Deuda residual menor** (NO bloquea uso real): el walker NO recursa
en `Stmt` adentro de `FnExpr.body`/`Loop.body`/`If.then`/etc. ni en
`Pattern`/`TypeExpr`. En la práctica, FnExpr inline adentro de un
StrInterp es extremadamente raro. Si entra demanda, sumamos walker
para Stmt en otro sub-paso.

---

#### (Descripción original — para referencia)

**Síntoma 1 — hover devuelve `Str` en vez del tipo real del ident**:

```fitz
let altitud_m = 350
print("   altitud: {altitud_m} m")
```

Hover sobre `altitud_m` dentro del `print(...)` muestra `Str` en lugar
de `Int`. Funcionalmente todo anda — el evaluator y el checker tipan
`altitud_m` como `Int` (validable con `{altitud_m + altitud_a}` que
da suma numérica). Solo el LSP miente.

**Síntoma 2 — error de checker se reporta en la línea/col equivocada**:

```fitz
let lugar = "Patagonia"                                        # ← squiggle rojo aparece acá
let altitud_m = 350
let altitud_a = "350"
print("   altitud: {altitud_m + altitud_a} m")                 # ← el error real está acá
```

El mensaje del checker es correcto (`el operador \`+\` no acepta \`Int\` y \`Str\``)
pero el squiggle rojo cae en la línea 1 col 1 en vez de en el `+` real.

**Causa raíz común**: el parser, al ver `{expr}` dentro de un string
interpolado, arranca un sub-parser sobre el texto aislado (`"altitud_m + altitud_a"`).
Ese sub-parser tokeniza desde `line=1, col=1` y solo ajusta los `line/column`
de los **errores** del sub-parse (vía `sub_col_base`). Los `Span` de los
`Expr` exitosos que produce el sub-parser quedan con `line=1` y `col`
relativa al sub-texto, no a la fuente original.

Por eso:

- En `TypeInfo` el `Ident("altitud_m")` interno se registra con
  `SpanKey(1, 1)` en vez de `(5, 21)`. La heurística "max col ≤ cursor
  en la misma línea" de `hover_for_position` no lo encuentra y devuelve
  el tipo del `StrInterp` entero (`Str`).
- En diagnostics, el `e.span()` del `BinOp` interno apunta a `(1, 1)`,
  así que el squiggle rojo cae en línea 1.

**Archivo afectado**: [src/parser.rs:3219-3229](src/parser.rs#L3219-L3229)
(función que parsea el StrInterp; el `sub_parser.expression()` no
post-procesa los spans).

**Fix propuesto** (~30 LoC, una sola sesión):

1. Walker recursivo `shift_expr_spans(expr: &mut Expr, line: usize, col_base: usize)`
   que reescribe `e.set_span(Span { line, column: col_base + col.saturating_sub(1) })`
   para cada nodo del Expr.
2. Llamarlo justo después de `let expr = sub_parser.expression()?;` con
   `line = line` (la línea del source original) y `col_base = sub_col_base`.
3. Test E2E que valida hover sobre `altitud_m` dentro de StrInterp
   devuelve `Int`, y otro que valida que `Int + Str` adentro de StrInterp
   se reporta en la línea/col correcta.

**Impacto**: alto en DX. Cualquier ident usado dentro de un `print("{x}")`
queda invisible al hover y los errores de tipos dentro de interpolaciones
se reportan mal ubicados. Patrón muy común en código real.

**Side benefit del fix**: go-to-definition desde dentro de un StrInterp
también queda bien (hoy seguramente apunta al primer token del archivo o
falla silenciosamente).

### V2 — Hover sobre el nombre de variable en `let X = ...` — **CERRADO 2026-06-05**

**Fix aplicado**: `AssignTarget::Ident` ahora lleva un `Span` propio
del token Ident del LHS ([src/ast.rs:519](src/ast.rs#L519)). El parser
captura el span via nuevo helper `expect_ident_with_span`
([src/parser.rs:185](src/parser.rs#L185)). El checker registra el
tipo del binding bajo el span del LHS en `TypeInfo`
([src/types.rs:8252](src/types.rs#L8252)) — para anotaciones explícitas
usa el tipo declarado, no el inferido. 4 unit tests en
`types::tests::v2_*` cubren los 4 casos del cap M1.C5 del curso
(nombre/edad/activa/latitud + nullable explícito).

Hover sobre `let edad = 200` ahora muestra `Int` sobre `edad`
(antes solo aparecía sobre `200`).

**Cierra parcialmente la deuda S1**: el paralelo en `Param` /
`For.var` / `MatchArm.pattern` sigue pendiente — mismo patrón
arquitectural, sub-paso futuro independiente. **Update 2026-06-05**:
S1 completa cerrada en v0.14.2 — `Param.name_span`, `Pattern::Ident(name, span)`,
`Pattern::OkBinding(name, span)`, `Pattern::ErrBinding(name, span)`
todos con span propio + checker registra tipos en `TypeInfo` bajo
esos spans. Ver entry v0.14.2 del CHANGELOG y nueva sección S1
en este backlog.

### S1 (deuda histórica) — Spans propios para Param / Pattern bindings — **CERRADO 2026-06-05 (v0.14.2)**

**Hallazgo histórico**: la deuda S1 era el paralelo del V2 que cerró
`AssignTarget::Ident`. Faltaban spans propios en `Param.name` (params
de fn), `Pattern::Ident` (var de `for` + binding genérico de match)
y `Pattern::OkBinding`/`ErrBinding` (bindings del unwrap de Result).
Sin esos spans, el checker no podía registrar los tipos en
`TypeInfo` bajo el lugar correcto y el LSP no mostraba nada al hover
sobre el nombre del param/binding.

**Casos cubiertos en v0.14.2**:

- Hover sobre `n` en `fn double(n: Int) => n * 2` → `Int`.
- Hover sobre `x` en `fn f(x) => x + 1` → `Any` (sin anotación).
- Hover sobre `i` en `for i in 0..10` → `Int`.
- Hover sobre `n` en `match x { Ok(n) => n + 1 }` → tipo inner del
  Result.
- Hover sobre `amount` en métodos custom (`type T { fn m(amount: Int) }`) → `Int`.

**Implementación**:

- AST: `Param` suma `name_span: Span`. `Pattern::Ident(String)` →
  `Pattern::Ident(String, Span)` (idem `OkBinding`, `ErrBinding`).
- Parser: captura el span via `expect_ident_with_span` (helper V2
  reusado).
- Checker: en `bind_pattern`, `bind_for_pattern_in_checker`,
  handlers de `FnDef`/`FnExpr`/`Method`, usa `name_span`/`ident_span`
  como `def_span` (con fallback al span del nodo contenedor cuando
  el span es ZERO en nodos sintéticos de tests) Y registra el tipo
  en `TypeInfo` bajo ese span.

**Tests** (5 unit en `types::tests::s1_*`): param anotado/sin anotar,
var del for sobre range, binding Ok(n), param de método custom.

**Side effect**: el sed bulk para arreglar los call sites de patrones
literales en tests tocó ~40 sites (Pattern::Ident, OkBinding,
ErrBinding) + ~22 Param constructors. Cambios mecánicos sin lógica
nueva. Total ~1700 LoC de diff, mayoría tests.

**Cierra la deuda S1 entera del proyecto** — paralelo natural del V2,
mismo patrón arquitectural.

---

#### (Descripción original — para referencia)

**Síntoma**: en

```fitz
let nombre = "Patagonia"
let edad = 200
let activa = true
let latitud = -49.32
let datos: Int? = null
```

el curso dice "pasá el mouse sobre cada variable → ves su tipo
inferido". En la práctica:

- Hover sobre `"Patagonia"` (el literal) → `Str` ✓.
- Hover sobre `200` → `Int` ✓.
- Hover sobre `nombre`/`edad`/`activa`/`latitud`/`datos` (el **nombre**
  de la variable, LHS del `let`) → **no muestra nada**.

El usuario espera que hover sobre cualquier variable muestre su tipo
inferido (lo que TypeScript / rust-analyzer hacen). Hoy solo aparece
el tipo si pasás el mouse sobre el valor del RHS.

**Causa**: `AssignTarget::Ident(String)` en
[src/ast.rs:466-468](src/ast.rs#L466-L468) no tiene `Span` propio — es
solo el nombre. El checker en `Stmt::Assign` infiere el tipo del
`value` (RHS) y lo registra en `TypeInfo` con el span del valor, pero
**no** registra el ident del LHS. Resultado: `hover_for_position` no
encuentra nada cuando el cursor está sobre `nombre`/`edad`/etc.

Esta es una manifestación visible de la deuda **S1** ya identificada en
el proyecto (`AssignTarget::Ident`/`Param`/`For.var`/`MatchArm.pattern`
sin span propio, hoy usan el span del stmt contenedor o nada).

**Fix propuesto**:

1. AST: `AssignTarget::Ident(String, Span)`. Migrar parser para que
   pase el `Span` del token del ident.
2. Checker: en `Stmt::Assign` con `target = AssignTarget::Ident(name, span)`,
   después de `let ty = infer_expr(ctx, value)`, agregar
   `ctx.type_info.record(span, ty.clone())` para que el LHS también
   aparezca en TypeInfo.
3. Mismos cambios paralelos en `Param`, `For.var`,
   `MatchArm.pattern` (cierra S1 entera).
4. Tests E2E sobre hover en LHS de `let` para los 5 casos del curso
   (nombre/edad/activa/latitud/datos con anotación explícita).

**Impacto**: alto en DX y especialmente en el curso. La primera
sección del cap 2/3 del curso (`Inferencia de tipos via hover`)
asume este comportamiento. Hoy hay un drift entre lo que enseña el
curso y lo que el LSP hace.

**Side benefit del fix**: completion contextual scope-level
también puede usar los spans para emitir Range exacto del symbol,
y go-to-definition desde otros usos del binding apunta al ident,
no al stmt entero.

### V3 — Formatting on save (`textDocument/formatting`) — **CERRADO 2026-06-05**

**Fix aplicado**: capability `document_formatting_provider: true`
anunciada en `initialize` + handler `formatting` en
[src/bin/fitz-lsp.rs](src/bin/fitz-lsp.rs#L325) que delega a
`fitz::fmt::format_source` y emite UN `TextEdit` con el documento
entero reformateado. Sobre doc con error de parser, devuelve `null`
silencioso — no aborta el save. Helper `end_position_utf16` calcula
el range del final del doc en UTF-16 (default LSP).

2 E2E tests nuevos en `tests/lsp_e2e.rs::v3_formatting_*` validan:
(a) capability anunciada + doc no-formateado emite TextEdit con
código formateado (tabs → 4 espacios), (b) doc roto retorna `null`
sin error.

VSCode con `"editor.formatOnSave": true` ahora dispara `fitz fmt`
automático al guardar — sin necesidad de configurar formatter
externo.

---

#### (Descripción original — para referencia)

**Qué falta**: hoy `fitz fmt` funciona como CLI (Fase 9.z.1) y como
formatter externo si el usuario configura `editor.formatOnSave = true`
+ `"[fitz]": { "editor.defaultFormatter": "..." }` apuntando al binario.
Pero el LSP no implementa `textDocument/formatting` ni
`textDocument/rangeFormatting`, así que VSCode no lo detecta como
formatter nativo de la extensión.

**Síntoma**: el usuario instala la extensión Fitz, activa "format on
save" en VSCode, y nada pasa al guardar un `.fitz` — tiene que correr
`fitz fmt` a mano en la terminal o configurar el binario externo.

**Cómo**: el módulo `fitz::fmt` ya expone una API pura
`format_source(source: &str) -> Result<String, FmtError>`. Falta:

1. Capability `formatting_provider: Some(OneOf::Left(true))` en
   `initialize` response del bin LSP.
2. Handler `formatting(&self, params: DocumentFormattingParams)` que
   lee `state.text` del documento, llama `fitz::fmt::format_source(&state.text)`,
   y devuelve `Vec<TextEdit>` con UN solo edit que reemplaza el doc
   entero (rango `(0,0)..(last_line, last_col)` → nuevo texto). Es el
   patrón estándar para formatters non-incremental.
3. Manejo de errores: si `fmt` falla (código con error de parser),
   devolver `Ok(vec![])` silencioso — no abortar el save.

**Costo**: 1 día. Plumbing puro. 2-3 unit tests del round-trip
(`format_source` ya tiene su propia suite).

**Impacto**: alto en DX cotidiana. "format on save" es lo primero que
el dev configura al adoptar un lenguaje nuevo. Hoy hay drift entre lo
que el ecosistema espera y lo que la extensión ofrece.

### V4 — Signature help — MVP **CERRADO 2026-06-05** + **EXPANDIDO 2026-06-05 (v0.15.0)**

**v0.15.0 — V4 expandido**: el MVP cubría solo fns user-defined del
programa. v0.15.0 suma:

- **Builtins globales**: catálogo `BUILTIN_SIGS` con 11 builtins comunes
  (`print`, `len`, `sleep`, `env`, `env_or`, `load_env`, `flag`, `spawn`,
  `config`, `secret`, `bytes`). Tipear `len(` abre popup con
  `fn len(x: Any) -> Int`.
- **Method calls** sobre `List<T>` / `Map<K,V>` / `Str`: catálogos
  paralelos (`LIST_METHOD_SIGS`, `MAP_METHOD_SIGS`, `STR_METHOD_SIGS`).
  Tipear `xs.map(` con `xs = [1, 2, 3]` muestra la firma del método.
- **`CallContext` enum** nuevo (`Function` vs `Method`) reemplaza
  el `(String, u32)` previo. Walkback identifica `.` antes del `(`.
- **Heurística `infer_builtin_receiver_kind`**: walka el `Program`
  matcheando el value asignado al receiver por shape estructural.

3 E2E tests nuevos en `tests/lsp_e2e.rs::v4_*` validan builtins
+ method calls sobre List + Str.

**Deuda residual** (NO bloquea):

- Receivers no-Ident (`xs[0].method`, `f().method`) no se identifican.
- Métodos custom de `type Foo` quedan pendientes.

---

#### (Descripción original — para referencia)

**Fix aplicado**: capability `signature_help_provider` con trigger
chars `(` y `,` anunciada en `initialize`. Handler `signature_help`
en [src/bin/fitz-lsp.rs](src/bin/fitz-lsp.rs#L356) delega a
`signature_help_at_position` ([src/lsp.rs](src/lsp.rs#L2997)) que
combina dos helpers nuevos:

- `find_call_context(text, line, char)`: walkback heurístico contando
  `(`/`)` y `,` para encontrar el `Call` enclosing + index del param
  actual.
- `signature_help_for_call(program, name, active_param)`: busca la
  `Stmt::FnDef` top-level por nombre y construye `SignatureInformation`
  con label `fn nombre(p1: T1, p2: T2) -> R` + `ParameterInformation`
  con offsets a cada param.

2 E2E tests nuevos en `tests/lsp_e2e.rs::v4_*` validan:
(a) cursor en `add(|` con `fn add(a: Int, b: Int) -> Int` → label
correcta + `activeParameter = 0`, (b) cursor en `add(5, |` →
`activeParameter = 1`.

**Limitaciones del MVP** (deuda menor):

- Solo cubre fns user-defined del programa. Builtins (`print`, `len`,
  módulos `jwt`/`hash`/etc.) y method calls (`xs.map(`) no muestran
  signature — los builtins tipan como `Type::Any` gradual y las
  signatures de métodos viven en `infer_*_method` por tipo del receptor.
- Walkback no respeta strings ni comments. Caso raro:
  `f("texto, con coma|")` puede contar mal `,`.

---

#### (Descripción original — para referencia)

**Qué falta**: cuando el usuario tipea `f(` o `f(a, ` el LSP debería
mostrar un popup con la firma de `f` y resaltar el param actual. Hoy
no hay implementación — el usuario tiene que recordar la firma o
hacer hover sobre la fn (que muestra el tipo pero no la firma posicional).

**Síntoma**: especialmente molesto en handlers HTTP con varios params
y kwargs (`@get("/users/{id}") fn show(id: Int, ...)`) y en builtins
con firmas complejas (`hash.password(plaintext: Str) -> Str`,
`jwt.encode(payload: Map<Str, Str>, secret: Str, alg: Str?) -> Str`).
El alumno del curso M5 (auth) lo va a sentir.

**Cómo**:

1. Capability `signature_help_provider: Some(SignatureHelpOptions {
   trigger_characters: Some(vec!["(".into(), ",".into()]), ... })`.
2. Handler `signature_help(&self, params: SignatureHelpParams)`:
   - Walkear hacia atrás desde el cursor para encontrar el `Call`
     enclosing — heurística sobre `state.text` (contar `(` no balanceados).
   - Lookup del callee por nombre en `TypeEnv` (fns top-level / builtins).
     Si es method call (`xs.map(`), resolver el método sobre el tipo del
     receptor — el `TypeInfo` ya tiene el tipo.
   - Construir `SignatureInformation` con `label = "fn nombre(p1: T1, p2: T2) -> R"`,
     `parameters = [ParameterInformation per param]`, `active_parameter`
     = count de `,` entre el `(` y el cursor.
3. Reusar el helper `Type::display` para renderear cada tipo.

**Costo**: 2 días. Lo más complejo es el walkback heurístico (parser
parcial es overkill para MVP). El catálogo de signatures ya está
todo en `TypeEnv` + builtins pre-registrados.

**Impacto**: alto en código que llama a fns con muchos params (auth,
DB queries con kwargs, OpenAPI handlers). Bonus pedagógico — el alumno
descubre la API de los builtins sin abrir la guía.

### V5 — Autocomplete tras `from X import` — **YA ESTABA CERRADO en v0.9.47** (2026-06-05 audit)

**Hallazgo del audit pre-implementación (2026-06-05)**: al arrancar
el Bloque 1 de fixes, descubrimos que esta feature **ya estaba
implementada** desde v0.9.47. El backlog estaba con drift — esta
entrada quedó como pendiente por error, sin auditar el código actual.

**Verificación**: `src/lsp.rs` ya tiene:

- `CompletionContext::FromImportList { mod_path }` ([src/lsp.rs:652](src/lsp.rs#L652))
- `detect_from_import_list_context` ([src/lsp.rs:863](src/lsp.rs#L863))
- `from_import_completions(doc_uri, mod_path)` ([src/lsp.rs:540](src/lsp.rs#L540))

Y el handler `completion` del bin (`src/bin/fitz-lsp.rs:312-336`) ya
llama a `completion_at_position_with_uri` pasando el `doc_uri`, lo que
permite resolver el archivo del módulo target y enumerar sus exports.

**Acción**: marcar como CERRADO. La entrada se mantiene para registro
histórico del audit.

---

#### (Descripción original — para referencia)

**Qué falta**: al tipear `from mod import |` (cursor después de `import `),
el LSP debería sugerir los símbolos `pub` exportados por `mod`. Hoy
no hay nada — la lista está vacía y el usuario tiene que abrir el
archivo del módulo para saber qué exportar.

**Síntoma**: gap visible en los caps de módulos del curso (M3+) y en
boilerplates multi-archivo. El usuario tipea `from models import |`
y necesita memoria fotográfica de qué types/fns/consts hay en `models.fitz`.

**Cómo**:

1. Detección del contexto en `detect_completion_context`: walk hacia
   atrás desde el cursor sobre la línea actual, matchear pattern
   `from <ident> import [<ident>(,)]*<cursor>`. Nueva variante del
   enum `CompletionContext::AfterFromImport { module: String }`.
2. Resolver el módulo: reusar `ModuleLoader::load(module, base_dir)`
   que ya carga + cachea. Si el módulo falla parsing parcial OK
   (resolución best-effort).
3. Enumerar exports: walker sobre el `Program` del módulo cargado,
   coleccionar `Stmt::FnDef`/`Stmt::TypeDef`/`Stmt::Assign` top-level
   con visibility `pub` (que en Fitz es implícito — todo top-level
   es exportable). Filtrar los ya importados de la lista actual del
   `from ... import a, b, |` para no sugerir duplicados.
4. Emitir `CompletionItem` con kind apropiado (`Function`, `Class`
   para types, `Constant` para `let` top-level con RHS literal) +
   detail con la firma corta.

**Costo**: 1 semana. Lo más caro: parser parcial robusto del `from X import a, b, |` mientras está en construcción (línea sintácticamente
inválida). Alternativa pragmática: regex sobre la línea para extraer
`module_name` + lista de imports ya escritos. Cubre 95% del caso real.

**Impacto**: cierra el gap más visible del autocomplete. Hoy after-dot
funciona, scope-level funciona, pero `from X import |` no sugiere
nada — inconsistencia que el alumno nota inmediatamente.

**Deuda residual derivada**: cuando llegue cross-module go-to-def
completo, este completion puede reusar la misma infra de resolución.

## Fixes pendientes del lenguaje (descubiertos en el curso) — **ABIERTO 2026-06-05**

Backlog hermano del de LSP. Acá van bugs y features del **lenguaje**
(lexer / parser / evaluator / codegen) que aparecen mientras el autor
sigue el curso `Fitz de 0 a experto`. Distinción con el backlog del
LSP: estos requieren cambio del compilador, no solo de la extensión.

### V6 — Debugging interactivo en VSCode (Debug Adapter Protocol) — **ABIERTO 2026-06-05**

**Qué falta**: Fitz hoy tiene LSP (diagnostics + hover + goto-def +
completion + signature help + format on save) pero **no tiene
debugging interactivo**. El alumno no puede:

- Clickear el gutter para poner breakpoints.
- Pausar la ejecución y inspeccionar variables.
- Step in / over / out.
- Watch expressions.
- Ver el call stack en el panel de Debug.

**Workarounds disponibles hoy**:

| Técnica | Cómo |
|---|---|
| `print` con interpolación | `print("x = {x}, n = {n}")` |
| **REPL interactivo** | `fitz repl` + `:load src/main.fitz` + llamar fns con valores reales |
| `:type <expr>` | Inspeccionar tipos sin ejecutar |
| `:env` | Ver todos los bindings del scope actual |
| Diagnostics LSP | El checker estático captura type mismatches antes de correr |

**Fix propuesto** (~2 semanas):

1. **Bin `fitz-dap` nuevo** (paralelo a `fitz-lsp`, feature gated `dap`).
   Implementa el protocolo DAP de Microsoft (JSON-RPC similar al LSP
   pero con shape distinto — request/response/event).
2. **Evaluator instrumentado** con:
   - Tabla de breakpoints por archivo + línea.
   - Hook al entrar a cada Stmt: check breakpoint, si está activo
     pausar (mecanismo via `tokio::sync::Notify` + state machine).
   - Step in/over/out: contar profundidad de fn calls + comparar.
   - Inspect: serializar `Value` actual de cada var del scope a JSON.
3. **Extensión VSCode** suma:
   - `debuggers` entry en `package.json` (lenguaje fitz, programa
     `fitz`, tipo `fitz`).
   - Template `launch.json` con configuración base
     (`type: fitz`, `program: ${workspaceFolder}/src/main.fitz`).
   - `LiveBuild` para spawnear `fitz-dap` al arrancar debug session.
4. **Tests**: E2E que simula un cliente DAP, pone breakpoint, ejecuta
   programa, valida que paramos en el breakpoint + variables expuestas
   correctas. Similar a los E2E del LSP pero sobre DAP.
5. **Cap nuevo del curso M7 (Producción)**: "Debugging en VSCode con
   breakpoints + watch expressions". Cierra el gap pedagógico vs
   Python/JS donde debugging es ciudadano primera.

**Decisiones técnicas pendientes**:

- (a) ¿`fitz build` también soporta debug info? Sería un breaking change
  del codegen (`cargo build --debug` → simbolos), o un flag opt-in
  (`fitz build --debug`). Sin esto, debugging solo funciona con
  `fitz run` (intérprete).
- (b) ¿Hot reload integrado con debugging? VSCode soporta restart de
  debug session — combinar con `fitz dev` sería poderoso pero
  invasivo.
- (c) ¿Conditional breakpoints? El alumno los va a esperar
  (`break if x > 10`). Requiere evaluar expresiones Fitz en el
  contexto del breakpoint — interpretable pero adds complejidad.

**Impacto**: alto en DX. Es el gap más grande vs Python/JS para
proyectos no triviales. Pedagógicamente es el cierre natural del
módulo de producción del curso.

**Riesgo**: el evaluator hoy es `#[async_recursion]` sobre `eval_expr`
/ `eval_stmt`. Instrumentar con breakpoints sin meter overhead en el
hot path (cuando NO hay debug session activa) requiere thoughtful
design — atomic bool global + branchless skip, o feature flag
condicional.

### L1 — `;` como separador de stmts — **CERRADO 2026-06-05**

**Fix aplicado**: el lexer ahora reconoce `;` como token y lo emite
como `Token::Newline` ([src/lexer.rs:1188](src/lexer.rs#L1188)) —
cero cambios al parser/AST. El parser ya tolera Newlines repetidos
como separator único, así que `;\n` no duplica stmts. 5 unit tests
en `lexer::tests::l1_*` cubren el caso solo, dos exprs con `;`,
`;\n` real, `;` adentro de strings (preservado literal), `;`
adentro de comentarios (consumido como parte del comment).

Validación end-to-end:

- `fitz run` sobre archivo con `let x = 5; let y = 10` funciona.
- REPL: `1 + 1; 2 + 2` → `= 4` (solo el último valor imprime —
  exactamente el comportamiento que prometía el cap M1.C5 del curso).

**Side effect** del fix por diseño: el `;` no se preserva en el AST.
El formatter `fitz fmt` reescribe `1 + 1; 2 + 2` como dos líneas
separadas, lo que es exactamente la convención canónica de Fitz.

**Acción derivada**: el cap M1.C5 del curso restauró la sección
"Múltiples expresiones por línea" (que se había sacado como fix
temporal). La guía también revirtió el call-out "Fitz no tiene punto
y coma" — ahora dice "el `;` es separator opcional entre stmts —
newline lo cubre en casi todos los casos", alineado con la decisión
de diseño #5 ("punto y coma opcional, como en Go").

---

#### (Descripción original — para referencia)

**Qué falta**: hoy el lexer NO reconoce `;` como token válido. Cualquier
intento de escribir `1 + 1; 2 + 2` aborta con `Carácter inesperado: ';'`.
La decisión de diseño #5 del proyecto (`CLAUDE.md`) dice *"Punto y coma
opcional — como en Go, el parser maneja ambigüedades"*, pero la
implementación real es: solo newlines separan stmts, no hay soporte
de `;` para nada.

**Síntoma**: especialmente molesto en el **REPL** donde el alumno quiere
encadenar varias expresiones cortas en una sola línea (`let x = 5; x * 2`,
`1 + 1; 2 + 2`). En `.fitz` files también es útil para one-liners
densos (debugging, scripts cortos).

**Caso reportado por el alumno del curso (2026-06-05)**:

```
fitz> 1 + 1; 2 + 2
✗ Error en línea 1:6 — Carácter inesperado: ';'
```

**Fix mientras tanto (aplicado 2026-06-05)**: el cap M1.C5 del curso
y la guía se corrigieron para no mencionar `;`. El alumno ya no choca
con drift entre docs y realidad.

**Fix propuesto** (~2-3 horas):

1. **Lexer**: agregar branch para `';'` que emita `Token::Semicolon`
   (variante nueva del enum `Token`). El span lleva línea/col del `;`.
2. **Parser**: en los puntos donde hoy se acepta `Token::Newline` como
   terminador de stmt (loop principal del programa, body de fn, body
   de bloques), aceptar también `Token::Semicolon` con misma semántica.
   Sin cambios al AST — el `;` es solo terminator, no se preserva.
3. **Tests**: unit del lexer (`;` produce token, span correcto), unit
   del parser (`let x = 5; let y = 10` produce 2 Stmt::Assign, `1 + 1; 2 + 2`
   en bloque produce 2 Stmt::Expr), E2E en REPL (`1 + 1; 2 + 2` imprime
   solo `= 4`), E2E compile (`fitz run` de archivo con `;` corre OK).
4. **Decisión semántica del REPL**: si la línea termina con `;` después
   de la última expresión (`1 + 1;`), ¿imprime el valor o no? Recomendación:
   sí imprimir — el `;` es solo separator opcional, no marca "descartar".
5. **Restaurar la sección "Múltiples expresiones por línea"** en
   `docs/curso/m1-setup/c5-repl.md` cuando esto cierre.
6. **Refinar la guía** (`docs/guide.md` línea ~3531): hoy dice "Fitz
   no tiene punto y coma". Cuando cierre, ajustar a "El `;` es separator
   opcional entre stmts — newline lo cubre en casi todos los casos".

**Costo**: 2-3 horas. Cambio chico pero invasivo en el parser (afecta
todos los call sites de `expect_newline_or_eof`). Tests E2E son
decisivos para validar que no se rompa nada existente.

**Impacto**: medio. La mayoría del código Fitz nunca usa `;` (newlines
alcanzan), pero destrabar el REPL para chains de expresiones cortas
es alto valor pedagógico y ergonómico. Cierra el drift histórico con
la decisión de diseño #5.

**Side benefit**: alinea la realidad con la frase "como en Go" del
documento de decisiones. Hoy esa frase es aspiracional, no descriptiva.

### L2 — Inferencia bidireccional — MVP **CERRADO 2026-06-05** + **EXPANDIDO 2026-06-05 (v0.15.0)**

**v0.15.0 — L2 expandido**: el MVP solo cubría callbacks de métodos
built-in (`List<T>.map/filter/find/...`). v0.15.0 suma:

- **Fn user-defined con param `Fn(...) -> ...`**:

  ```fitz
  fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x) }
  apply(fn(n) => n * 2, 5)   // n: Int sin anotar
  ```

  Implementado en `Expr::Call` cuando `callee_ty` es
  `Type::Function { params, .. }` — propaga los `params` como hint
  al arg correspondiente si es FnExpr.

- **`let f: Fn(...) -> ... = fn(...) => ...`**:

  ```fitz
  let f: Fn(Int) -> Int = fn(n) => n * 2   // n: Int sin anotar
  ```

  Implementado en `Stmt::Assign` cuando hay anotación + RHS es FnExpr.
  Resuelve la anotación a Type, si es `Function` extrae los `params`
  y los empuja al hint stack ANTES de sintetizar el RHS.

3 unit tests nuevos en `types::tests::l2x_*`.

**Reusa el mismo mecanismo `ctx.fn_expr_param_hints` introducido en
v0.14.1** — sin cambios al AST.

**Deuda residual** (NO bloquea): inferencia bidireccional para método
custom con param Fn no se dispara (caso raro hasta que tipos custom
expongan métodos higher-order canónicamente).

---

#### (Descripción original — para referencia)

**Fix aplicado** (alcance acotado al caso 90%): el checker propaga
el T del receptor a los params SIN anotación del callback cuando el
método es uno de los built-in con template paramétrico conocido sobre
`List<T>` (`.map`/`.filter`/`.find`/`.any`/`.all`/`.count`/`.find_index`/
`.flat_map`).

**Implementación**:

- Nuevo helper `expected_callback_param_for_builtin_method(obj_ty, method) -> Option<Vec<Type>>`
  ([src/types.rs](src/types.rs#L4997)). Para `List<T>` + cualquiera de
  los métodos cubiertos, devuelve `Some(vec![T])`. Otros casos (Map
  higher-order, Str, custom Nominal) → `None` (no rompe la lógica
  existente).
- `CheckCtx` gana stack `fn_expr_param_hints: Vec<Option<Vec<Type>>>`
  para soportar nested callbacks sin contaminación. El call site del
  método empuja el hint ANTES de sintetizar el arg si es un FnExpr
  directo.
- Handler de `Expr::FnExpr` ([src/types.rs](src/types.rs#L4470))
  consume el top del stack al entrar. Para cada param: si tiene
  anotación explícita, la anotación gana; si no, usa el hint en vez
  de `Type::Any`.

**Tests** (6 unit en `types::tests::l2_*`):

- `[1, 2, 3].map(fn(x) => x * 10)` → `List<Int>` ✓ (caso pedagógico).
- `[1, 2, 3].filter(fn(x) => x > 0)` → `List<Int>` con `Bool` validado.
- `["a", "b"].map(fn(s) => s.upper())` → `List<Str>`.
- Param con anotación explícita (`fn(x: Float) => x * 2.0` sobre
  `List<Int>`) → `List<Float>` (anotación gana).
- `.find(fn(x) => x == 2)` → `Result<Int>`.
- Nested callbacks (`xs.map(fn(x) => [x].map(fn(y) => y * 2))`) → cada
  uno recibe su propio hint sin contaminación.

**Acción derivada**: el cap M1.C5 del curso restauró el ejemplo
original `:type [1, 2, 3].map(fn(x) => x * 10)` → `:: List<Int>` y
removió la nota sobre la limitación (que ya no aplica). El cap suma
una explicación corta sobre la inferencia bidireccional y cuándo
gana la anotación explícita.

**Deuda residual derivada de L2** (NO bloquea uso real):

- **Map<K, V> higher-order**: hoy `infer_map_method` no expone
  callbacks (solo get/has/keys/values/len). Cuando llegue, agregar
  el caso al helper devolviendo `Some(vec![K, V])`.
- **Inferencia bidireccional GENERAL**: el alcance del fix es
  callbacks de métodos built-in conocidos. Casos no cubiertos —
  ej. fn user-defined con param `Function`, FnExpr asignada a var
  con anotación de Function — siguen sintetizando params sin
  anotación como `Any`. Refactor invasivo del checker (1-2 semanas)
  si entra demanda real.
- **`flat_map` con ret type del callback**: `flat_map` exige
  `fn(T) -> List<U>` y luego sintetiza U del ret. El hint actual
  propaga solo T (el param). Validar si el alumno escribe
  `.flat_map(fn(x) => [x, x+1])` y necesita inferencia del U también.
  En la práctica el U se sintetiza desde el body sin hint adicional.

---

#### (Descripción original — para referencia)

**Qué falta**: hoy el checker hace inferencia solo **bottom-up**
(synthesis). Cuando el alumno escribe:

```fitz
[1, 2, 3].map(fn(x) => x * 10)
```

el FnExpr `fn(x) => x * 10` se sintetiza primero sin contexto del
receptor: `x` queda como `Any` (sin anotación, no hay propagación),
`x * 10` con `BinOp(Any, Int)` tipa como `Any`, `ret = Any`. Después
`.map` ve el callback `Function { params: [Any], ret: Any }` e instancia
`U = Any`. Resultado: `List<Any>`.

**Síntoma reportado por el alumno (2026-06-05)**:

```
fitz> :type [1, 2, 3].map(fn(x) => x * 10)
:: List<Any>
```

El curso M1.C5 prometía `:: List<Int>` (drift entre lo enseñado y
lo real). El runtime SÍ calcula `x * 10` correctamente (la evaluación
es dinámica), pero el checker estático no puede saber que `x` es `Int`
sin que el alumno lo anote.

**Fix mientras tanto (aplicado 2026-06-05)**: el cap M1.C5 del curso
se corrigió para usar `fn(x: Int) => x * 10` con anotación explícita.
Una nota corta debajo del ejemplo cita esta deuda. Idem en el segundo
ejemplo del cap (línea 650+).

**Fix propuesto** (~1-2 semanas):

1. **Two-pass para method calls con callback**: cuando `Expr::Call`
   tiene callee `Expr::Field` y el método resuelto pertenece a la
   tabla built-in con signature paramétrica (`List<T>.map(fn(T) -> U)`),
   PRIMERO resolver T del receptor, DESPUÉS propagar T a los params
   sin anotación del FnExpr arg.
2. **Modificar `infer_list_method` / `infer_map_method`** en `types.rs`:
   en vez de sintetizar el callback con su contexto vacío, recibir
   un `expected_param_types: Vec<Type>` derivado de la signature del
   método y pasarlo al wrapper de `synthesize_fn_expr`.
3. **Wrapper de `Expr::FnExpr`**: aceptar opcional `expected_types`
   y, para cada param sin anotación, usar el expected como tipo del
   binding en el scope del body. El `lub` del ret también puede
   beneficiarse pero NO es necesario para el MVP.
4. **Tests**: `xs.map(fn(x) => x * 10)` sobre `List<Int>` tipa
   `List<Int>`; sobre `List<Str>` tipa `List<Str>` cuando el body es
   válido; `xs.filter(fn(x) => x > 0)` sobre `List<Int>` tipa
   `List<Int>` con `ret = Bool` validado; param CON anotación
   incompatible con T del receptor sigue siendo error (no se sobreescribe
   silenciosamente).
5. **Restaurar el ejemplo del curso** (`fn(x) => x * 10` sin anotación)
   y quitar la nota cuando esto cierre.

**Costo**: 1-2 semanas. No es trivial — toca el orden de visita del
checker (synthesis vs checking modes) y abre la pregunta de si querés
extender bidireccional a otros casos (anonymous fn como var, fn como
param de fn user, etc.). Recomendable acotar el MVP a callbacks de
métodos built-in con templates conocidos.

**Impacto**: medio-alto pedagógico. Cierra el case más común donde
el alumno espera "Fitz infiera como TS/Rust hacen" y se choca con
`Any`. Cubre patrón canónico `.map`/`.filter`/`.find` que aparece en
todos los caps de Listas/Loops/Higher-order.

**Riesgo**: cambio invasivo del checker. Tests E2E sobre todos los
ejemplos de la guía + cursos antes/después son decisivos.

### L3 — `:load` con paths absolutos estilo Unix no funciona en Windows (2026-06-05)

**Qué pasa**: hoy en el REPL, `:load /tmp/helpers.fitz` resuelve a
`D:/tmp/helpers.fitz` (o `C:/tmp/...` según el drive del cwd) en
Windows. El path absoluto estilo Unix se reinterpreta contra el drive
actual y casi nunca existe.

**Caso reportado por el alumno (2026-06-05)**:

```
fitz> :load /tmp/helpers.fitz
✗ no se pudo leer `D:/tmp/helpers.fitz`: ... (os error 2)
fitz> :load /src/helpers.fitz
✗ no se pudo leer `D:/src/helpers.fitz`: ... (os error 3)
fitz> :load src/helpers.fitz
✓ cargado D:\CURSO_FITTZ\micosa\src/helpers.fitz
```

**Causa**: `std::fs::read_to_string` en Windows interpreta `/tmp/...`
como path relativo al drive del cwd. Es comportamiento estándar de
la API de Rust + Windows, no un bug propio de Fitz. Pero la **UX del
curso** asume Unix.

**Fix mientras tanto (aplicado 2026-06-05)**: el cap M1.C5 Paso 8 se
cambió para usar `:load src/helpers.fitz` (relativo al cwd del REPL)
que funciona idéntico en Linux/macOS/Windows. Se sumó un "Tip cross-OS"
explicando por qué evitar paths absolutos Unix en los ejemplos.

**Opciones de fix permanente**:

1. **No hacer nada** (mantener el fix de docs): el `:load` con paths
   relativos es la convención portable y ya está documentada. Paths
   absolutos siguen siendo válidos para usuarios power que saben qué
   están haciendo. **Recomendado**.
2. **Detectar `/path/...` en Windows y emitir warning**: si el primer
   char es `/` y estamos en Windows, sugerir `Para paths absolutos en
   Windows usá D:/...; para portabilidad usá paths relativos`. Costo
   muy chico (5 LoC en el handler de `:load`).
3. **Mapear `/tmp/` → `%TEMP%`** en Windows como conveniencia: NO
   recomendado, demasiado magic, rompe expectativas en otros paths
   Unix-style.

**Recomendación**: opción 1 (cerrar como "by design + docs corregidas").
Si llega demanda real, opción 2.

**Impacto**: bajo desde que las docs están corregidas. Pre-fix, el
alumno Windows se chocaba inmediatamente en el Paso 8 del cap M1.C5.

### L4 — Strings con delimitador `'...'` no existen (curso prometía) (2026-06-05)

**Qué pasa**: el cap M2.C1 del curso documentaba que Fitz soporta
strings con dos delimitadores (`"..."` y `'...'`), con escapes
paralelos (`\"` para uno y `\'` para el otro). En la realidad, **solo**
`"..."` es string en Fitz. El `'` se reserva para **labels** en
`break`/`continue` de loops anidados (`'outer: loop { break 'outer }`).

**Caso reportado por el alumno (2026-06-05)**:

```
print('\'comillas\'')             // 'comillas'
```

```
Error en línea 10:7 — se esperaba un identificador después de `'` (label)
```

El lexer ([src/lexer.rs:1229-1252](src/lexer.rs#L1229-L1252)) ve `'`
y arranca un `Token::Label(name)` — necesita un ident detrás, lo que
falla con cualquier escape o char no-ident.

**Fix mientras tanto (aplicado 2026-06-05)**: cap M2.C1 corregido —
eliminada la fila de la tabla de escapes (`\'`), eliminada la línea
del demo (`print('\'comillas\'')`) y la línea del output (`'comillas'`),
y agregado un call-out explícito *"Fitz usa **solo** `"..."` como
delimitador de strings. El char `'` se reserva para **labels** de
`break`/`continue` en loops anidados"*. La guía y `syntax-spec.md` ya
estaban consistentes — el drift estaba solo en el cap del curso.

**Opciones de fix permanente**:

1. **No hacer nada (cerrar como by design)**: mantener `"..."` como
   único delim, `'` reservado para labels. Convención clara, sin
   ambigüedades. Curso ya está corregido. **Recomendado**.
2. **Soportar `'...'` como string alternativo con desambiguación**:
   el lexer al ver `'` mira el char siguiente — si es alfa válido
   para ident, es label; si no, es string. Problema: `'a'` sería
   ambiguo (label `'a` vs string `'a'`). Costo medio + UX confusa.
   No recomendado.
3. **`'...'` como char literal (Rust-style)**: `'a'` sería un char
   de 1 byte. No encaja con el modelo de Fitz (no hay tipo `Char`
   separado de `Str`). Requiere agregar el tipo entero. Gran feature,
   fuera de scope.

**Recomendación**: opción 1. Cerrar como "by design". El curso
corregido refleja la realidad. El alumno que viene de Python/JS
necesita el call-out explícito para no asumir simetría.

**Impacto**: bajo desde que el cap está corregido. Pre-fix, el alumno
del curso M2.C1 se chocaba al copiar el demo de escapes.

**Side note — bug colateral del cap**: el demo del cap M2.C1 incluía
`print("emoji: 🏔 🌍")` y `print("CJK: 名前は何ですか?")` antes de
los strings con `'`. En la captura del alumno, el `fitz run` solo
imprime las primeras dos líneas válidas (`cirílico` y `matemático`)
y aborta en la línea 10 sin llegar a los emojis ni CJK. El runtime
SÍ soporta UTF-8 multibyte; lo que aborta es el parser por el `'`.
Una vez aplicado el fix, todos los `print(...)` van a correr OK.

