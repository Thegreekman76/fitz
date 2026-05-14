# Changelog

Cambios visibles del lenguaje Fitz, agrupados por hito. Sigue el
formato de [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
El detalle técnico de cada sub-paso vive en
[`docs/roadmap.md`](docs/roadmap.md); este archivo es la vista
condensada para alguien que pregunta "¿qué cambió y cuándo?".

Las versiones son retroactivas — Fitz todavía no publica releases
formales; cada bump corresponde al cierre de una Fase del roadmap.

## [Sin publicar]

En curso: ver `docs/roadmap.md` para el plan vigente. El próximo
hito comprometido es **Fase 8 — Interop Python** (embedding CPython
via PyO3, importar SQLAlchemy/NumPy/etc. desde handlers Fitz).

## [v0.8.1] — 2026-05-14 — Mini-tanda PreF8: cleanup antes de Interop Python

Cuatro sub-pasos chicos antes del salto a Fase 8 para no entremezclar
deuda existente con la parte real de Python interop.

- **PreF8.1 — Refactor M1+M2 codegen**: `generate_main_rs` (232 LoC)
  → orquestador de ~18 LoC + 3 helpers libres (`partition_program_stmts`,
  `resolve_state_var_types`, `emit_main_rs_body`). `gen_http_handler_wrapper`
  (532 LoC) → orquestador de ~9 LoC + 6 métodos del `impl CodegenCtx`
  (`resolve_handler_signature` que devuelve `HandlerSig`,
  `emit_axum_extractors`, `emit_middleware_chain`,
  `emit_param_coercions`, `emit_handler_dispatch_and_response`,
  `emit_cors_helpers`). Cero cambio funcional: AST del Rust generado
  bit-a-bit idéntico pre/post sobre los 19 ejemplos del smoke
  `GUIDE_EXAMPLES_COMPILE`. F8 va a hacer crecer ambas fns con Python
  imports + wrappers; mejor partirlas antes.
- **PreF8.2 — Method chain multi-línea en parser**: el `postfix()`
  loop tolera `Token::Newline` antes de `.`. Habilita el patrón
  idiomático de chains largos partidos por línea
  (`users\n.filter(...)\n.map(...)`); AST resultante idéntico al
  one-liner. Caso de uso central: chains de SQLAlchemy/pandas en F8.
- **PreF8.3 — Defaults de tipos importados**: auditoría de 6 casos
  de `Field.default` detectó un único bug — defaults que referencian
  consts del módulo de origen fallaban en `fitz run` y `fitz build`.
  Fix con estrategia eager-at-import: `Value::Type` suma
  `resolved_defaults`, el loader pre-evalúa los defaults en el env
  del módulo; codegen emite `pub fn __default_<T>_<F>()` en el
  módulo. Habilita el patrón `from foo import User` con
  `type User { name: Str = DEFAULT_NAME }` sin re-importar
  `DEFAULT_NAME`.
- **PreF8.4 — Import aliasing**: `import foo as f`, `from foo import
  bar as b`, alias mixto. Sub-paso adelantado de F8.1. Lexer suma
  `Token::As`; AST suma `Stmt::Import.alias` y cambia
  `Stmt::FromImport.names` a `Vec<(String, Option<String>)>`.
  Evaluator usa el `Value::Type.name` canónico al instanciar
  (`Person { ... }` con alias produce instancia cuyo Display dice
  `User`, paridad bit-a-bit). Codegen emite `use foo::bar as b;`.

**Tests**: 1172 unit (baseline 1153 + 19 nuevos) + 79 compile_e2e
(baseline 74 + 5 nuevos) + 3 openapi_e2e verdes. Clippy
`-D warnings` limpio. Paridad bit-a-bit `fitz run` ↔ `fitz build`
validada en todos los sub-pasos.

Detalle completo: `docs/roadmap.md` → "Mini-tanda PreF8".

## [v0.8.0] — 2026-05-14 — Fase F17: Send completo + paralelismo HTTP real

- **Paralelismo HTTP real**: el server (tanto `fitz run` como el
  binario de `fitz build`) corre tokio `rt-multi-thread` con N
  workers según cores. 5 requests concurrentes a un handler
  `sleep(1000).await` responden en ~1.2s (pre-F17 eran ~5s).
- **Bridge HTTP eliminado**: el modelo de dos threads + canal
  `mpsc/oneshot` introducido en Fase 4 desapareció. Los handlers
  axum invocan al evaluator directo sobre un `Arc<HttpRegistry>`
  compartido. ~269 LoC netas menos en `src/http.rs`.
- **Tipos `Send`**: `Value` y `EnvRef` migran de `Rc<RefCell<>>` a
  `Arc<parking_lot::Mutex<>>` (intérprete) y a `Arc<std::sync::Mutex<>>`
  (codegen output). Habilitó la eliminación del bridge y el runtime
  multi-thread.
- **State HTTP compartido**: pasa de `thread_local!` a
  `LazyLock<Arc<Mutex<T>>>` en el codegen, para que un solo Arc
  se comparta entre workers.
- **Guía cap 19**: sub-sección nueva "Paralelismo HTTP real" con
  ejemplo `examples/guide/19b-paralelismo.fitz`.

Subdivisión en 6 sub-pasos: F17.1 (dep `parking_lot`), F17.2
(migración atómica Shared/EnvRef), F17.3 (Send completo en
evaluator), F17.4a (`serve()` multi-thread), F17.5 (eliminar
bridge), F17.4b (codegen multi-thread + tipos), F17.6 (guía +
cierre formal).

Detalle completo: `docs/roadmap.md` → "Fase F17".

## [v0.7.1] — 2026-05-14 — Mini-tanda Q: quick wins post-MW

- **Q.1**: `@header(into="alias")` para mapping explícito de un
  header HTTP a un nombre arbitrario de param Fitz.
- **Q.2**: `@server(api_version="X.Y.Z")` override del campo
  `info.version` del schema OpenAPI.
- **Q.3**: CORS request-aware. `cors({"allow_origin": ["a.com",
  "b.com"]})` con `List<Str>` activa modo Set — el server hace
  echo del `Origin` recibido si está en la lista permitida.
- **Q.4**: status codes custom aparecen en `responses` del schema
  OpenAPI con description de la frase HTTP estándar.
- **Q.5** (postergado): bundle Scalar embebido offline. ~3.7 MB de
  overhead no justifica romper "binario mínimo". Pendiente como
  opt-in via `@server(offline_docs=true)` cuando aparezca presión
  real (deploys air-gapped).
- **Q.6**: refresh de docs (guide header, syntax-spec v0.2,
  deudas-post-5b).

## [v0.7.0] — 2026-05-14 — Mini-fase MW: middleware + CORS

- **`@middleware(fn)`** apilable antes de cualquier handler HTTP.
  Modelo gate-only: `return null` o sin return continúa la cadena;
  `return <status> { ... }` corta con ese status code.
- **`@middleware(cors(...))`** con built-in `cors(...)` configurable
  (`allow_origin`, `allow_methods`, `allow_headers`, `max_age`).
  Preflight `OPTIONS` automático con 204 + headers; inyección de
  `Access-Control-Allow-*` en la response real (incluso 500/400).
- **Built-in `Request`** (`method`, `path`, `headers`) y `Response`
  opaco como tipos del lenguaje, pre-registrados en `TypeEnv`.
- **Paridad bit-a-bit** `fitz run` ↔ `fitz build` validada con E2E
  build + spawn + raw TCP request.

## [v0.6.0] — 2026-05-13 — Fase 7: DX HTTP

- **OpenAPI 3.1 autogenerado** desde los decoradores. Path/query
  params, body, headers, return type (`Result<T>` → 200 + 500)
  todos reflejados en el schema. Subcomando nuevo
  `fitz openapi archivo.fitz`.
- **UI Scalar embebida** en `/docs`. Bundle via CDN jsdelivr.
- **Headers como params del handler** con `@header(name="X")`. Lookup
  case-insensitive. Solo `Str` o `Str?`.
- **`@server(docs=false)`** opt-out de las rutas auto `/docs` y
  `/openapi.json`. Zero overhead cuando se apagan.
- **Paridad bit-a-bit** entre `fitz run`, `fitz openapi` y el
  schema embebido por `fitz build`.

## [v0.5.0] — 2026-05-13 — Fase 6: Async nativo

- **`async fn`** declarable, retorna `Future<T>` al llamarse.
- **`.await`** postfix para desempacar futures. Permitido adentro
  de `async fn` y a nivel top-level del archivo.
- **`Future<T>`** como tipo built-in genérico (igual que `List<T>`,
  `Result<T>`).
- **Builtin `sleep(ms: Int) -> Future<Null>`** que pausa N
  milisegundos sin bloquear el runtime.
- **Handlers HTTP async**: cualquier `@get`/`@post`/etc. puede ser
  `async fn`. axum invoca con `.await` automático.
- **`fitz build`** emite `#[tokio::main(flavor = "current_thread")]`
  para programas con async, y compila `.await` 1:1 a Rust.

## [v0.4.0] — 2026-05-12 — Fase 5: Compilador estático

- **Fase 5a — Type checker estático**. `fitz check` valida tipos
  en todo el programa: resolución de `TypeExpr`, inferencia de
  ret type para `FnExpr`, chequeo de aridad/tipos de calls,
  exhaustividad de `match` sobre `Result`, métodos built-in
  paramétricos. `fitz run` corre en modo strict por default;
  `--no-typecheck` para escape gradual.
- **Fase 5b — Codegen a binario nativo**. `fitz build archivo.fitz`
  transpila Fitz → Rust → binario standalone (via `cargo build
  --release`). Subset cubierto: primitivos, tipos custom, listas/
  mapas homogéneos, `Result`/`?`/`match`, módulos, HTTP nativo,
  higher-order (closures escapadas + fn como valor/param/retorno),
  state HTTP compartido.
- **Guía cap 20 nuevo** "fitz build" con mapping de tipos Fitz → Rust.

## [v0.3.0] — 2026-05-11 — Fase 4: HTTP nativo

- **Decoradores HTTP**: `@get`, `@post`, `@put`, `@delete` registran
  rutas en un `HttpRegistry` durante `eval`. `serve()` arranca
  axum + tokio cuando hay rutas y bloquea hasta Ctrl-C.
- **Path params tipados**: `@get("/users/{id}")` con `fn h(id: Int)`
  coerciona el path param crudo al tipo declarado; falla → 400.
- **Body JSON**: cada parámetro que no es path se trata como body.
  Con `type` declarado, validación + defaults + extras → 400.
- **`@server(port, host)`** configurable. Default `127.0.0.1:3000`.
- **`Result<T>` auto-handling**: `Ok(v)` → 200 + JSON(v),
  `Err(e)` → 500 + `{"error": e}`.

## [v0.2.0] — 2026-05-11 — Fase 3: El lenguaje crece

- **Listas, mapas, rangos**: `[1, 2, 3]`, `{"k": v}`, `0..10`.
  Indexing postfix `xs[i]`, `m["k"]`. `for var in iter`.
- **Tipos custom**: `type User { id: Int, name: Str }`. Struct
  literal `User { id: 1, name: "x" }`. Field access `obj.campo`.
  Defaults y nullables.
- **Funciones anónimas + higher-order**: `fn(x) => x * 2`, callbacks
  `xs.map(fn)`. Métodos sobre List/Map/Str.
- **`Result<T>` + `Ok`/`Err` + `?`**: sum type built-in para errores.
  Patrón `Ok(x)`/`Err(e)` en match; operador `?` postfix propaga.
- **Módulos**: `import foo`, `from foo import User`. Cache por path
  canonicalizado + detección de ciclos.

## [v0.1.0] — 2026-05-11 — Fase 2: Intérprete base

- **Lexer + parser + AST** completos para la sintaxis core.
- **Evaluator** que recorre el AST y produce efectos
  (`print`, asignaciones, control de flujo).
- **Variables**, primitivos (`Int`, `Float`, `Str`, `Bool`, `Null`),
  strings con interpolación (`"hola, {name}"`).
- **Operadores**: aritmética con promoción Int↔Float, comparación,
  lógicos, unario negativo.
- **Control de flujo**: `if`/`else`, `while`, `for`, `loop`/`break`,
  `match` con patrones literales, wildcard, rangos.
- **Funciones**: `fn nombre(params) -> ret { ... }` o
  `fn nombre(p) => expr`. Closures con captura por referencia.
- **CLI**: `fitz run archivo.fitz` ejecuta un programa.
- **Guía v0.1** publicada (`docs/guide.md`).
