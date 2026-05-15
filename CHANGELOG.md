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
hito comprometido es **Fase 8.4 — Tipos del lado del checker
(anotaciones y opacidad)**: refinar el tipo de un call Python de
`Any` a `Result<Any>` en el checker estático y permitir
anotaciones explícitas (`let row: User = py_call(...)?`) que el
runtime valida contra el tipo declarado.

## [v0.8.4] — 2026-05-15 — Fase 8.3: Excepciones Python → Result<T>

Tercer sub-paso de la Fase 8 (Interop Python). Cambia la semántica
de las llamadas a funciones Python desde Fitz: **TODA llamada se
envuelve automáticamente en `Result<T>`**. Si Python lanza una
excepción (`ValueError`, `JSONDecodeError`, etc.) o si el marshaling
de args falla (tipo Fitz no representable en Python), el call no
aborta el programa — devuelve `Result::Err(Str("<ClassName>:
<message>"))` que el usuario tiene que manejar con `match` o `?`,
igual que cualquier otra operación que puede fallar (`find`/`get`/
`json.loads` nativos). Preserva la decisión de diseño "sin
excepciones" del lenguaje y evita que excepciones Python escapen
como panics opacos.

- **8.3.1 — `call` envuelve return en Result + tests viejos
  actualizados**: `py_interop::call(handle, args)` ahora SIEMPRE
  devuelve `Ok(Value::Result(...))`. Éxito produce
  `Value::Result(Ok(v))` con el valor coercionado adentro;
  cualquier falla (excepción Python, marshaling de args, marshaling
  del return) produce `Value::Result(Err(Str("<ClassName>:
  <message>")))`. Helper privado `err_value_from_message(msg)`
  construye el wrap. Los ~16 tests viejos del call path (8.1.4 +
  8.2.1 + 8.2.2 + 8.2.3) actualizados con helper `ok_inner(v)` que
  desempaqueta el Ok; los tests que esperaban error
  (`call_excepcion_python_*`, `call_arg_no_marshalleable_*`)
  reescritos con `err_message(v)` que extrae el mensaje del Err.
  4 tests py_interop nuevos sobre el shape: shape `Ok(...)`,
  criterio textual del roadmap (`json.loads("{ malformado")` →
  `JSONDecodeError`), TypeError envuelto, formato `"<Class>:
  <msg>"` estable.
- **8.3.2 — Ejemplos 8.1 y 8.2 actualizados al modelo Result**:
  `examples/python-interop-8.1.fitz` reescrito con
  `match { Ok(v) => v, Err(_) => ... }` para desempaquetar y fns
  helper (`fn floor_x(x: Float) -> Result<Int> { return Ok(math.floor(x)?) }`)
  que propagan con `?`. Sección nueva "Errores Python como
  Result::Err" con caso `math.sqrt(-1.0) → err: ValueError: ...`.
  Idem para `examples/python-interop-8.2.fitz`: helper
  `fn unwrap_str(r: Result<Str>) -> Str`, caso nuevo
  `loads(malformado) → JSONDecodeError: ...`, literales compuestos
  extraídos a variables porque el parser de interpolación no
  acepta `{...}` adentro de strings (caveat documentado en el
  ejemplo).
- **8.3.3 — Ejemplo dedicado + cierre formal**: nuevo
  `examples/python-interop-8.3.fitz` con 6 secciones — criterio
  textual del roadmap, distintas excepciones Python como Err,
  propagación con `?`, marshaling fallido como Err (uniformidad),
  field access sin wrap (decisión interna), chaining con
  desempaquetado intermedio. Validado bit-a-bit. CHANGELOG v0.8.4,
  roadmap actualiza Fase 8.3 a CERRADA, deudas nota de cierre,
  CLAUDE/README refresh.

**Decisiones técnicas tomadas al arrancar**:

- **`call` envuelve siempre, `get_attr` NO envuelve**. Solo
  llamadas pasan por Result; field access (`math.pi`,
  `obj.attr`) sigue devolviendo el valor coercionado directo.
  Matchea la letra del roadmap ("toda **llamada** a una función
  Python") y preserva la ergonomía de leer constantes y submódulos
  sin `match` por cada acceso. AttributeError fallido sigue siendo
  `FitzError` que aborta (es típicamente un error de programación,
  no de runtime esperable).
- **Marshaling de args también va en Err** (uniformidad): el
  usuario ve UN solo punto de error en el path call, independiente
  de qué falló — excepción Python o tipo Fitz no marshalleable.
- **`Err` lleva `Value::Str` con el mensaje** plano. `Value::
  Instance(PyException)` con inspección estructurada (type,
  traceback) queda como deuda menor — si entra demanda real.
- **KeyboardInterrupt/SystemExit también van como `Err`** según
  el roadmap. No hay forma de matar el runtime Fitz desde una
  excepción Python.
- **El checker NO cambia en 8.3**. Sigue tipando call Python como
  `Any`. El refinamiento a `Result<Any>` llega en 8.4.

**Cambio de comportamiento**: técnicamente esto rompe los
ejemplos viejos de 8.1/8.2 que asumían call sin wrap. Se
reescribieron en 8.3.2 (no se publicaron antes de este release,
así que no afecta a usuarios externos).

**Tests al cierre**:
  - Sin feature: **1175 unit** (sin cambios — tests Python son
    `#[cfg(feature = "python")]`) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1252 unit** (1245 baseline 8.2 + 4 py_interop
    + 3 evaluator del criterio canónico/propagación con `?`/field
    access sin wrap) + 80 + 3.
  - Clippy `cargo clippy --all-targets --features python -- -D warnings`
    limpio. Idem sin feature.

Detalle completo: `docs/roadmap.md` → "Fase 8.3".

## [v0.8.3] — 2026-05-15 — Fase 8.2: Marshaling de tipos compuestos

Segundo sub-paso de la Fase 8 (Interop Python). Habilita el
marshaling bidireccional de `List<T>` ↔ `list`, `Map<K, V>` ↔ `dict`,
e `Instance` → `dict` (por field name). Cumple el criterio del
roadmap end-to-end: una función Python que recibe `List<User>` y
devuelve un mapping `email → cantidad` (`collections.Counter`)
funciona sin perder data, con la `List<User>` original Fitz
intacta después del round-trip.

- **8.2.1 — Fitz → Python (`value_to_py`)**: refactor con
  parámetro `path: &str` para breadcrumb informativo en errores
  (ej. `arg0[2].email` apunta al sitio exacto adentro de la
  estructura). Nuevas ramas:
    - `Value::List(items)` → `PyList` con elementos recursivos
      (copia eager).
    - `Value::Map(pairs)` → `PyDict`. Las keys deben ser
      primitivos hashables Python (Int/Float/Str/Bool/Null);
      compuestos como key → error claro citando la restricción.
      Helper `marshal_map_key` valida antes de tocar `dict.__setitem__`.
    - `Value::Instance { type_name, fields }` → `PyDict` con
      field names como keys (traducción nominal). El tipo Fitz
      se "olvida" del lado Python; recuperarlo en el round-trip
      requiere anotación destino (deuda 8.4).
  Política cross-cutting #4 del roadmap: copia eager bidireccional,
  sin aliasing entre los dos GCs. Tipos no marshalleables
  (Range, Function, Future, Type, Module, HttpResponse, CorsConfig,
  Result) → error con path. Test del fallback 8.1.4 reapuntado a
  Range (sigue sin ser marshalleable).
- **8.2.2 — Python → Fitz (`py_to_value`)**: nuevas ramas para
  `PyList` y `PyDict` antes del fallback opaco. Ambas con
  recursión sobre elementos/pares. Resultado semánticamente
  `List<Any>`/`Map<Any, Any>` desde Fitz porque Python no nos da
  tipo estático; refinar a tipos concretos requiere anotación
  destino del lado Fitz (deuda 8.4). CPython 3.7+ garantiza
  orden de inserción para `dict`; preservarlo da paridad bit-a-bit
  con `serde_json::preserve_order` que ya usa el resto del
  proyecto. Decisión explícita: `dict` Python NO se auto-coerce
  a `Instance` Fitz — eso es 8.4. PyO3 0.28 deprecó `downcast`
  en favor de `cast`; usamos `cast`.
- **8.2.3 — Criterio de éxito end-to-end + ejemplo runnable**:
  pipeline canónico `List<User>` Fitz → `Counter` Python →
  `Map<Str, Int>` Fitz funciona sin glue extra porque
  `collections.Counter` es subclass de `dict` y `is_instance_of::
  <PyDict>()` matchea subclases. Validado bit-a-bit. Nuevo
  `examples/python-interop-8.2.fitz` con 5 secciones (Fitz →
  Python, Python → Fitz, round-trip, criterio canónico, copia
  eager). NO entra al smoke `GUIDE_EXAMPLES_COMPILE` (interop
  Python es `fitz run` only — deuda F19).

**Tests al cierre**:
  - Sin feature: **1175 unit** (sin cambios — todos los nuevos
    tests son `#[cfg(feature = "python")]`) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1245 unit** (+ 20 en `py_interop` y + 12 en
    evaluator distribuidos entre 8.2.1/8.2.2/8.2.3, más 2 ajustes
    a tests viejos de 8.1.4 que asumían "List como arg → error
    citando 8.2" — ahora List sí marshalla y Python rechaza con
    TypeError) + 80 + 3.
  - Clippy `-D warnings` limpio en ambos modos.

**Detalles de implementación notables**:

- Breadcrumb de errores con `path: &str` propagado recursivamente:
  un Range adentro de `List<Map<Str, List<Range>>>` reporta
  `arg0[2]["k"][3]` o similar.
- Llaves JSON `{...}` en source Fitz se escapan con `\{`/`\}` para
  evitar interpolación de strings. Documentado en el ejemplo.
- Map keys cuando va a Python: helper `marshal_map_key` con
  validación temprana (mensaje más útil que el `TypeError:
  unhashable type` que Python lanzaría).

Detalle completo: `docs/roadmap.md` → "Fase 8.2".

## [v0.8.2] — 2026-05-15 — Fase 8.1: Embedding básico de CPython

Primer sub-paso de la Fase 8 (Interop Python). Habilita
`from python import <módulo>` desde el intérprete (`fitz run`),
con la feature opt-in `python`. Acceso a atributos, llamadas con
args primitivos, return primitivo coercionado a `Value` Fitz.
Cumple el criterio del roadmap: `math.sqrt(16.0)` → `4.0`,
`math.pi` → `3.141592653589793`.

- **8.1.1 — Dep PyO3 opcional + variante `Value::PyObject`**:
  `Cargo.toml` suma `pyo3 = "0.28"` como dep opcional bajo la
  feature `python`. Features de PyO3: `abi3-py310` (un binario
  corre 3.10+) y `auto-initialize` (boot lazy en el primer
  `Python::attach`). `Value::PyObject(PyObjectHandle)` feature-
  gated; handle envuelve `Arc<Py<PyAny>>` para `clone()` O(1) sin
  tomar el GIL. PartialEq por identidad via `Py::as_ptr()`,
  Display `<python object>`, type_name `"PyObject"`. Binario
  `fitz` default sigue siendo standalone sin link a libpython.
- **8.1.2 — `from python import X` + loader CPython**:
  módulo nuevo `src/py_interop.rs` (feature-gated) con
  `import_module(dotted) -> Value::PyObject` envuelto en
  `Python::attach`. Helper `py_err_to_fitz` traduce excepciones
  Python a `FitzError` con formato `"<ClassName>: <message>"`
  (compatible con el wrap a `Result<T>` que llega en 8.3).
  Evaluator: `Stmt::FromImport` con `path[0] == "python"` rutea
  al loader Python; sin feature, error claro citando el flag
  `cargo build --features python`. Alcance 8.1.2:
  `path == ["python"]` exacto (submódulos profundos quedan deuda
  menor). `import python.X` se rechaza con sugerencia
  `from python import X`.
- **8.1.3 — `Expr::Field` + auto-coerción primitiva**:
  `py_interop::get_attr(handle, name)` toma GIL, hace
  `bound.getattr` y aplica `py_to_value`. Política:
  `None` → `Null`, `bool`/`int`/`float`/`str` → primitivos Fitz,
  resto → PyObject opaco. Chequeo de `bool` ANTES que `int` (en
  Python `bool ⊂ int`). Overflow de `int > i64` → error explícito
  (bignum support queda como deuda menor). Evaluator: `Expr::Field`
  despacha sobre `Value::PyObject` con feature on, enriqueciendo
  el error con el span del field access. Desbloquea `math.pi`,
  `os.path` como submódulo opaco, `math.__name__`.
- **8.1.4 — `Expr::Call` con args primitivos (criterio cerrado)**:
  `py_interop::call(handle, &args)` con `bound.call1(tuple)`
  (positional only — kwargs queda deuda menor). Helper
  `value_to_py` con política simétrica: `Int`/`Float`/`Str`/`Bool`/
  `Null` se marshalla a Python; PyObject passthrough preserva
  identidad. Args compuestos (List/Map/Instance/Range/Function/...)
  → error citando 8.2 como sub-paso futuro. Evaluator:
  `invoke_value` (caso `let f = math.sqrt; f(25.0)`) y
  `dispatch_method` (caso `math.sqrt(16.0)` directo, `json.dumps(
  "hola")` chained) ambos despachan sobre `Value::PyObject`.
  Excepciones Python emiten `FitzError`; el wrap a `Result<T>`
  llega en 8.3.
- **8.1.5 — Guard de codegen + error path completo**:
  `fitz build` con `from python import` aborta con mensaje claro
  sugiriendo `fitz run` (binario con `--features python`). Función
  libre `check_no_python_imports(program)` corre dos veces: al
  inicio de `generate_project` (path real, antes de tocar disk
  para no producir el mensaje confuso "no se encontró
  `python.fitz`") y al inicio de `generate_main_rs` (path de
  tests unit que usan `generate_rust` directo). Deuda comprometida
  F19: soporte real en `fitz build` (emitir Rust con `pyo3`
  linkeado + Cargo.toml condicional) queda como probable sub-paso
  de 8.7 cuando cierre distribución con CPython bundled.

**Tests al cierre**:
  - Sin feature: **1175 unit** (baseline 1172 + 1 fallback de
    "feature off da error claro" + 2 codegen guards) +
    **80 compile_e2e** (baseline 79 + 1 guard E2E) + 3 openapi_e2e.
  - Con feature: **1213 unit** (+ 22 unit en `py_interop` + 11 en
    evaluator + 2 codegen; el test del fallback no-aplica con la
    feature on) + 80 + 3.
  - Clippy `cargo clippy --all-targets --features python -- -D warnings`
    limpio. Idem sin feature.

**Política de venvs** (decisión 2026-05-14): estándar Python sin
magia. El usuario activa su venv antes de `fitz run`
(`source venv/bin/activate` o equivalente en Windows); CPython
embebido lee `VIRTUAL_ENV` al boot y prepende el `site-packages`
del venv a `sys.path`. Cero código nuevo en Fitz. Auto-detect de
`./venv/` y similares queda como deuda menor (revisitable en 8.5
o como flag CLI dedicado).

**Política de errores Python**: en 8.1 cualquier `PyErr` aborta el
programa con `FitzError` ("<ClassName>: <message>"); el wrap
automático a `Result<T>` llega en 8.3 — el formato del mensaje
queda estable, solo cambia el envoltorio.

**Ejemplo runnable**: `examples/python-interop-8.1.fitz` cubre
constantes, funciones con args primitivos, submódulos opacos y
chained call. Se corre con
`cargo run --features python -- run examples/python-interop-8.1.fitz`.
NO entra al smoke `GUIDE_EXAMPLES_COMPILE` porque 8.1 es `fitz run`
only.

Detalle completo: `docs/roadmap.md` → "Fase 8.1".

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
