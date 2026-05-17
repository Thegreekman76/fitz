# Auditoría post-Fase 5b — deudas y mejoras

> Documento generado tras cerrar Fase 5b (codegen a binario nativo).
> Identifica deudas técnicas, gaps de docs, mejoras de calidad/UX.
> **No ejecuta fixes** — es input para decidir qué atacar y en qué orden.

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
> roadmap, README, CLAUDE). Decisiones: in-process via PyO3,
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
> v0.8.7, roadmap, deudas, CLAUDE, README). Decisiones:
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
> formal (CHANGELOG v0.8.9, roadmap, deudas, README, CLAUDE).
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
> detallada, este archivo con F16 CERRADO, README + CLAUDE
> refresh). **API user-facing intacta** — `fitz run` /
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
> - **Bug del formatter: trailing comment al final del body de
>   una fn seguido de otro bloque inserta blank spurious dentro
>   del body del bloque siguiente**. MRE preciso (descubierto
>   redactando el ejemplo del cap 23):
>   ```fitz
>   fn greet(name: Str) -> Str {
>       return "Hola, {name}!" // inline
>   }
>
>   for n in ["Ada"] {
>       print(greet(n))
>   }
>   ```
>   Tras `fitz fmt`, queda blank line spurious entre el `{` del
>   `for` y `print(greet(n))`. Variante del caso edge ya
>   documentado en `docs/fmt-style.md` ("Comments entre último
>   stmt de un bloque y el `}` ... pueden terminar fuera del
>   bloque al re-formatear"), pero acá el comment está EN LA
>   MISMA LÍNEA que el último stmt (trailing), no entre stmt y
>   `}`. El bug afecta la enseñanza del cap 23 (el ejemplo
>   runnable tuvo que removerse el trailing comment final).
>   **Etapa**: sub-paso de fix-up de 9.z.1 (deuda residual
>   reconocida del closing de 9.z.1.b). Pre-9.z.2 dedicado si
>   el fix es chico (~30 min — probablemente bookkeeping del
>   estado "just emitted blank" en `fmt.rs`); si requiere
>   refactor del trivia stream, post-9.z.5 cuando 9.z entera
>   cierre. Auditoría rápida del módulo antes de decidir.
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
> - **Bug del fmt con trailing comment** (heredado de Q.z) —
>   blank spurious dentro del body del bloque siguiente.
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
>   roadmap, README, CLAUDE, syntax-spec actualizado a
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
| R2 | `codegen.rs:3444+` | Nombres de variables/campos del usuario se inyectan en strings Rust sin sanitizar. Defensa en profundidad: agregar sanity check. Teórico hoy (parser filtra), pero frágil. | Media | Baja |
| R3 | `codegen.rs` (múltiples) | `write!`/`writeln!` con `.unwrap()` ~36 sitios. No falla sobre `String` pero acopla a la representación de output. | Media | Media |
| R4 | `evaluator.rs:1578` y otros | `unwrap()` sobre args ya validados por aridad. Seguro hoy, pero fragiliza ante refactor. | Baja | Baja |
| R5 | `http.rs:208-228` | `with_active_registry` con `take()`/restore — patrón correcto pero documentación no aclara invariantes de reentrancia. | Baja | Baja |
| R6 | `evaluator.rs` | Float `1.0/0.0` → `Float(inf)` sin warning, después falla en serialización JSON. Atajable en aritmética. | Baja | Media |

### UX (mensajes / output / CLI)

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| S1 | AST + propagación | **Span en AST** — Stmt-level cerrado en B.1; **Expr-level cerrado en S1.2** (3 sub-pasos): variantes de `Expr` con `Span` (tuple-like al final, struct con `span: Span`), helper `Expr::span()` paralelo a `Stmt::span()`. Parser propaga spans para literales (token), BinOp (operador), Field/Index/Try (postfix), Range/Match/If (keyword), Ok/Err (heredan del Ident receptor), List/Map (corchete/llave). Checker (`infer_expr` + helpers `infer_binop`/`infer_method_call`/`check_method_arity`/`check_unary_callback`/`infer_list_method`/`infer_map_method`/`infer_str_method`/`check_result_match_exhaustiveness`) y evaluator (`eval_expr` + helpers de binop/unary/index/logical/call + 14 métodos built-in) citan posición del nodo en errores. **S1.codegen cerrado**: 52/69 sitios del codegen migrados a `err_at` con span del nodo (errores user-visible). Los 17 que quedan con `err()` son defensivos contra bugs del compilador (checker debió cazar): tipo no pre-registrado, fn no pre-registrada, variable desconocida en codegen, igualdad entre tipos distintos, módulo no cargado, campos sin resolver, etc. Doc-comments de `err`/`err_at` separan los dos casos. 5 tests de span en parser, 9 en checker, 5 en evaluator. **Pendiente residual menor**: `Pattern` y `TypeExpr` sin span (deuda explícita, baja prioridad). | Baja (residual) | Baja |
| U1 | `evaluator.rs` | Mensajes de error inconsistentes en estilo: "no tiene método X" vs "el tipo X no soporta" vs "espera Y arg(s)". Falta helper unificado. | Media | Baja |
| U2 | `types.rs` ~20 sitios | Mismo patrón `ctx.error(format!("...{}...{}...", ...))` repetido. Helper `type_mismatch_error(label, expected, actual)` reduce repetición. | Baja | Baja |
| U3 | `http.rs:481` | El handler-mapping `Ok→200/Err→500` no incluye stack trace del Err en log (solo en response). Útil para debug. | Baja | Baja |
| U4 | `evaluator.rs:496-510` | Detección de ciclos de import no incluye stack en mensaje. El `LOADER.loading` lo tiene; agregar al error. | Baja | Baja |

### Performance

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| P1 | `evaluator.rs:2040+` | Map es `Vec<(K,V)>` — lookup O(n). Documentado como deuda explícita; bloqueante para maps grandes. | Baja | Alta |
| P2 | `codegen.rs:1911+` | `.clone()` recursivos de `Type` en hot path (~20 sitios). Cada `gen_expr` puede hacer 2-3 clones. | Media | Media |
| P3 | `codegen.rs:636+` | Pre-registro de tipos/fns clona estructuras enteras. Alternativa `Rc<TypeSig>` reduciría allocaciones, requiere refactor. | Baja | Alta |
| P4 | `evaluator.rs:805` | Snapshot pattern (`items.borrow().clone()`) en cada llamada a `.map`/`.filter`. Necesario para evitar re-entrancia pero costoso. | Baja | Alta |
| P5 | `codegen.rs` field access | `u.field` → `(u).borrow().field.clone()`. Optimizable a borrow sin clone en casos seguros, pero requiere análisis. | Baja | Alta |

### Mantenibilidad

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| L2 | ~~`codegen.rs`~~ **CERRADO (2026-05-14)** — el helper `with_temp_output(|ctx| ...)` ya existía (lo usaba `gen_block_to_string`) y los 2 sitios manuales restantes (`gen_callback_inline` y `gen_fn_expr_as_value`) se migraron a él. El conteo "~13 sitios" del análisis original quedó obsoleto — la mayoría de los usos se habían consolidado a lo largo de los sub-pasos post-5b. Reducción menor de líneas; el valor real es que ahora hay una sola convención para "emitir a buffer temp". | — | — |
| M1 | ~~`codegen.rs:1159-1391`~~ **CERRADO (2026-05-14, PreF8.1)** — `generate_main_rs` (232 LoC) → orquestador de ~18 LoC + 3 helpers libres: `partition_program_stmts` (bucketea stmts en type_defs/http_fns/top_fns/main_stmts + valida decorators + extrae `@server`), `resolve_state_var_types` (detección de state HTTP + resolución de tipos), `emit_main_rs_body` (emisión final). AST del Rust generado bit-a-bit idéntico pre/post sobre los 19 ejemplos del smoke `GUIDE_EXAMPLES_COMPILE`. | — | — |
| M2 | ~~`codegen.rs:4902-5434`~~ **CERRADO (2026-05-14, PreF8.1)** — `gen_http_handler_wrapper` (532 LoC) → orquestador de ~9 LoC + 6 métodos del `impl CodegenCtx`: `resolve_handler_signature` (entry pattern match, parse path, collect middlewares, resolver tipos, validar y categorizar params, resolver return), `emit_axum_extractors` (firma del wrapper), `emit_middleware_chain` (Request build + chain con short-circuit CORS-aware), `emit_param_coercions` (query + headers + body), `emit_handler_dispatch_and_response` (call + 3 caminos de response), `emit_cors_helpers` (`__cors_resolve_<name>` + `__preflight_<name>`). Nuevo struct `HandlerSig` captura el estado intermedio. | — | — |
| M3 | `types.rs:664-1110` | `infer_expr` 446 líneas con mega-match de 30+ branches. Extraer branches grandes. | Baja | Media |
| M4 | `types.rs:1691-1866` | `check_stmt` 175 líneas. Repetición `push_scope`/`pop_scope` en 4 branches. Helper `with_scope(f)`. | Baja | Baja |
| M5 | `parser.rs` 3 sitios | `parse_list_literal`/`parse_call_args`/`parse_struct_lit_fields` parsean listas con coma con código similar pero no factorizado. | Baja | Media |

### Tests

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| T1 | `codegen.rs` tests | **CERRADO** — los 3 batches migrados. Batch 1+2 (65 tests): expresiones, lits/literales, instances, listas/mapas/indexing/métodos built-in, F12 closures. Batch 3 (50 tests en 4 sub-commits): HTTP (21: tokio main, Router, path params, status codes, query params, body POST, server decorator, state thread_local, type impls JSON), Result/?/match (9: Ok/Err constructors, ? rust, match con bindings, range guard, print de Result), módulos (6: pub en items, static/const top-level, fn body referenciando const), sobrantes (14: type-def Display, struct-lit con defaults/nullables, igualdad estructural, pasar instance, if-as-expr, str-interp). **Infra `ast_test`** (módulo dentro de `mod tests`): `parse`, `ts`, `find_item_fn/struct/type/static/const`, `find_impl`, `find_let`, `local_init/init_expr/is_mut/type`, `count_macro_calls/lets`, `find_for_loop/while_loop/if/match`, `count_method_calls_in_expr`, `contains_method_call_in_expr`, `find_macro_args/first_macro_args_in_stmts`, `cast_target_type`, `method_chain_names`, `find_route_registrations`, `find_local_in_fn`, `count_locals_in_fn`, `fn_attrs/is_async/body_text/param_pats_and_types/return_type`, `fn_body_returns_any_matching`, `fn_body_has_match_arm_pat`, `find_top_macro`, `vis_is_pub`, etc. Removed: helpers dead `assert_contains` y `assert_http_contains`. **Quedan 10 `code.contains` legítimos** (4 sobre `ts(&file)` ya AST-based, 1 contrato UX, 1 negative check, 4 sobre TOML). | — | — |
| T2 | `tests/compile_e2e.rs:20` | Mutex `SERIAL` serializa los 48 E2E. Cada uno usa tempdir único — paralelizables con `CARGO_TARGET_DIR` per-test. ~4x speedup. | Media | Media |
| T3 | `parser.rs` tests | Solo 4 tests de paths de error. Sin tests para: `fn f(a, a)` (params duplicados), decorator fuera de fn, escapes raros. | Media | Media |
| T4 | E2E ~12/48 | Tests E2E que solo verifican que `build` no falle, sin validar stdout/body/status. | Media | Baja |
| T5 | `codegen.rs` | Tipos custom compilados sin tests E2E sobre el binario: field access, instancias anidadas, igualdad estructural. (Sí hay en intérprete.) | Media | Media |
| T6 | Combinatorias | Cero tests para `List<List<Int>>`, `Map<Str, List<Int>>`, `List<Custom?>`. | Media | Alta |
| T7 | HTTP E2E (7/48) | Cobertura HTTP E2E muy limitada. Sin tests para: múltiples rutas mismo path, headers, Content-Type negociation, body sin tipo declarado. | Media | Alta |

### Deuda funcional (features incompletas o gradual)

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| F1 | `types.rs` ~30 sitios | `Type::Any` como gradual escape — silencia errores intencionalmente. Falta documentar matriz de cobertura: qué casos son legítimos vs cuáles podrían tipar mejor. | Media | Media |
| F2 | ~~`types.rs:1739-1741`~~ **CERRADO en C-F2** — el checker ahora valida que el receptor sea `Nominal`, que el field exista, y que el tipo del RHS sea compatible (`is_compatible`). Mensaje con `User.field` + tipos esperado/recibido + línea (gracias a B.1). 6 tests nuevos. | — | — |
| F3 | `parser.rs:656-662` | `return`/`break`/`continue` huérfanos aceptados por parser, captados en runtime con mensaje genérico. El checker podría rechazarlos estáticamente. | Media | Media |
| F4 | ~~`parser.rs` + `evaluator.rs` + `codegen.rs`~~ **CERRADO (2026-05-14, PreF8.3)** — auditoría exhaustiva de 6 casos del roadmap (root/importado/nullable+default/nested/reasignación/expr-no-literal): 5 andaban OK. Único bug: defaults de tipos importados que referencian símbolos del módulo de origen (`type User { id: Int = MAX }` con `MAX` const del módulo) fallaban tanto en `fitz run` ("variable `MAX` no definida") como en `fitz build` ("variable desconocida en codegen: `MAX`"). Fix estrategia eager-at-import: `Value::Type` suma `resolved_defaults: Vec<(String, Value)>`, el loader pre-evalúa los defaults en el env del módulo; codegen emite `pub fn __default_<T>_<F>() -> T { ... }` en el módulo y el struct lit del importer invoca `<mod>::__default_<T>_<F>()`. Tipos locales del archivo principal siguen con eval lazy del Expr. 3 unit tests + 1 E2E nuevos. Guía cap 12 documenta el comportamiento. | — | — |
| F5 | `evaluator.rs:751`, `http.rs:27-29` | `is_async` en `FnDef` se ignora silenciosamente (deuda explícita). | Baja | Alta |
| F6 | `evaluator.rs:2098-2113` | Solo 2 builtins (`print`, `len`). Verificar si el syntax-spec promete más (`range`, `type_of`, `to_string`). | Baja | Baja (audit) |
| F7 | `lexer.rs:187-234` | Números: sin soporte `1_000` (separador), `3.14e-2` (notación científica). | Baja | Media |
| F8 | `lexer.rs:319` | Identificadores ASCII-only (`is_alphabetic()` pero después corta con `is_ascii_digit()`). Sin `π`, `función` como nombres. | Baja | Baja |
| F9 | `lexer.rs:252-279` | Escapes en strings limitados: faltan `\u{...}`, `\x..`, `\0`, `\b`. | Baja | Media |
| F10 | ~~`parser.rs`~~ **CERRADO (2026-05-14, PreF8.2)** — `postfix()` loop tolera `Token::Newline` antes de `.`. Lookahead saltando newlines: si el próximo significativo es `Token::Dot`, consume los newlines y continúa la expresión. Solo `.` continúa — `(`, `[`, `?` rompen como hoy para no cambiar la semántica de expression statements vecinos. AST resultante idéntico al one-liner. 8 tests parser nuevos. Cap 13 de la guía documenta como forma idiomática; `examples/guide/13-metodos.fitz` suma chain de 3 líneas. | — | — |
| F11 | ~~`codegen.rs` (state HTTP)~~ **CERRADO** vía `thread_local! { static __FITZ_STATE_X: Rc<RefCell<T>> = ...; }` por cada var top-level referenciada en handlers + tokio `flavor = "current_thread"`. Cada fn que toca state materializa al inicio del body (`let X = __FITZ_STATE_X.with(|s| s.clone());`). Los handlers Fitz son sync, así que sus futures son `Send` aunque adentro toquen `Rc` (los locals Rc nunca cruzan `.await`). `examples/server.fitz` (CRUD completo) y `examples/guide/17-http.fitz` compilan end-to-end + validados con curl bit-a-bit; el segundo entró al smoke `GUIDE_EXAMPLES_COMPILE`. 5 tests nuevos (1 unit + 4 E2E con build + spawn + secuencia de requests). **Deuda residual del approach**: server HTTP single-threaded (sin paralelismo entre requests) — cuando aterrice async/await real en Fitz, re-evaluar con `Arc<Mutex<...>>` + `State` extractor. | — | — |
| F12 | ~~`codegen.rs` (higher-order)~~ **CERRADO** — closures escapadas, fn nombrada como valor, FnExpr asignado a var, fn como param y como tipo de retorno compilan con `fitz build`. `TypeExpr::Function` nueva variante; codegen emite `Rc<dyn Fn(...) -> R>` uniforme. Cap 11 anotado y compilable bit-a-bit con el intérprete. Smoke `GUIDE_EXAMPLES_COMPILE` incluye `11-funciones.fitz`. 24 tests nuevos. | — | — |
| F13 | `codegen.rs` | Listas/mapas heterogéneos: `[1, "dos"]` corre en intérprete, no compila. Requiere `FitzValue` tagged runtime. | Baja | Alta |
| F14 | `codegen.rs` | `let X = <expr>` no-literal a nivel mod top-level. | Baja | Media |
| F15 | ~~`parser.rs` + `ast.rs` + `types.rs` + `evaluator.rs` + `codegen.rs`~~ | **CERRADO (2026-05-15, Fase 9.0, 1219 unit + 79 E2E)** — error recovery del parser end-to-end. 3 sub-pasos: 9.0.1 AST + API recovery + tests del parser (nodos `Expr::Error(Span)` / `Stmt::Error(Span)` in-band + `Vec<FitzError>` paralelo; `pub fn parse_with_recovery(tokens) -> (Program, Vec<FitzError>)` con `recovery_mode` interno + cota `MAX_RECOVERED_ERRORS = 100` + helper `synchronize()` con sync points stmt-level — `Newline` consumido, `RBrace`/`EOF` preservados, **keywords de inicio de stmt preservadas** `Let`/`Fn`/`Async`/`Type`/`Return`/`Break`/`Continue`/`While`/`Loop`/`For`/`If`/`Import`/`From`/`At` por necesidad: `primary()` consume el token actual antes de validar, los tests detectaron que sin la parada en keywords sync se comía stmts enteros; defensas en eval/codegen con `FitzError` claro + span; 10 unit tests `parser::tests::recovery_*`); 9.0.2 tolerancia del checker (`Expr::Error → Type::Any`, `Stmt::Error` no-op, silencioso para que el LSP corriendo `check_program` sobre AST recuperado no emita cascadas; helper local `check_recovering(src)` que corre el pipeline LSP-style `parse_with_recovery → check_program`; 5 unit tests `types::tests::checker_*`); 9.0.3 cierre formal (smoke a mano `fitz check` strict sobre buffer roto → exit 1 con un error del primer stmt roto, comportamiento idéntico a antes; smoke `GUIDE_EXAMPLES_COMPILE` sigue verde; CHANGELOG v0.9.0, roadmap con Fase 9.0 detallada, README + CLAUDE refresh). **API strict (`parse`) intacta** — la CLI sigue priorizando fail-fast. Decisiones técnicas: nodos in-band + lista paralela (árbol mantiene forma estructural, mejor para LSP/formatter); sync points stmt-level + keywords (compromiso entre simplicidad y recovery efectivo); cota 100 errores (caso 90% del LSP cubierto con margen sin runaway). **Deuda residual derivada** (NO bloquea Fase 9): recovery sub-stmt (errores dentro de un stmt descartan el stmt entero — refinable para completion fino tras `user.`); bindings parciales (`let x = <roto>` no preserva `x`, genera "no definido" en referencias posteriores; aceptable como trade-off del LSP MVP); `Expr::Error` con metadata (opaco hoy, refinable post-LSP). Ver detalle en `docs/roadmap.md` → "Fase 9.0". | — | — |
| F16 | ~~`types.rs` (checker)~~ | **CERRADO (2026-05-15, Fase 9.0, 1227 unit + 79 E2E)** — IR tipado persistido por nodo end-to-end. 2 sub-pasos: 9.0.4 `pub struct SpanKey(usize, usize)` como clave hashable (Span propio no sirve por su PartialEq custom que devuelve true siempre, diseñado para tests de AST estructurales), `pub struct TypeInfo` con `record`/`type_at`/`len` que omite `Span::ZERO` para evitar colisiones entre nodos sintéticos, `infer_expr` envuelve `synthesize_expr` para centralizar el `record` desde un solo punto (recursión incluida), `pub fn check_program` cambia firma de `(TypeEnv, Vec<FitzError>)` a `(TypeEnv, TypeInfo, Vec<FitzError>)` con 13 call sites migrados con `_types`, `Expr::Error` (F15) se persiste como `Type::Any` uniforme con el checker, 8 unit tests `types::tests::types_info_*`; 9.0.5 cierre formal (CHANGELOG v0.9.1, roadmap, este archivo, README + CLAUDE refresh). **API user-facing intacta** — la CLI descarta el side-table. Decisiones técnicas: HashMap<SpanKey, Type> (vs NodeId, vs `*const Expr` — el primero reusa spans del AST sin refactor); cobertura amplia (todo Expr, no solo Ident/Field/Call); una sola firma de check_program (vs variante separada — 13 sitios migran trivialmente); Span::ZERO omitido por colisiones; Expr::Error como Any (LSP decide qué mostrar). **Deuda residual derivada** (NO bloquea sub-fases visibles del LSP): sin index espacial (rango inicio-fin) — el LSP elige nodo más cercano al cursor por ahora; spans en `TypeExpr` y `Pattern` (heredado de S1); cobertura de `Stmt` (ortogonal — resolución de declaraciones vía scope lookup en 9.x.3). Ver detalle en `docs/roadmap.md` → "Fase 9.0 — F16". | — | — |
| F18 | ~~`parser.rs` + `evaluator.rs` + `codegen.rs` + `types.rs`~~ **CERRADO (2026-05-14, PreF8.4)** — import aliasing con `as` (`import foo as f`, `from foo import bar as b`, alias mixto). Sub-paso adelantado de F8.1 para dejarlo con solo Python interop puro. Lexer suma `Token::As`; AST suma `Stmt::Import.alias: Option<String>` y cambia `Stmt::FromImport.names` a `Vec<(String, Option<String>)>`. Codegen emite `use foo::bar as b;` (fn/const) o `use foo::{T as L, TData as LData};` (type). Evaluator usa el `Value::Type.name` canónico al instanciar (no el alias sintáctico) para paridad bit-a-bit `fitz run` ↔ `fitz build` del Display. 9 unit + 4 E2E nuevos. Cap 16 de la guía documenta. | — | — |
| F19 | ~~`codegen.rs` (`check_no_python_imports`)~~ **CERRADO (2026-05-15, Fase 8.7)** — codegen interop Python en `fitz build` end-to-end. 4 sub-pasos: 8.7.1 detección + filtrado del ModuleLoader + Cargo.toml condicional (`pyo3 = "0.28"` con `abi3-py310 + auto-initialize`) + preludio `__FitzPyObject(Arc<Py<PyAny>>)` con Display delegado a `__str__` Python (paridad bit-a-bit `print`) + helpers `__fitz_py_import` + getattr + extracción primitiva i64/f64/String/bool + **bindings globales** (`static __FITZ_PY_BIND_X: OnceLock<__FitzPyObject>` + getter por binding, accesibles desde cualquier fn); 8.7.2 trait `__FitzToPy` con impls genéricos para primitivos + List + Map + Option + Instance (`impl __FitzToPy for FooData` + wrapper sobre `Arc<Mutex<FooData>>` emitidos por `gen_type_def` cuando `uses_python = true`) + helper `__fitz_py_invoke(callable, args_fn) → Result<__FitzPyObject, String>` con wrap automático de excepciones Python paralelo a 8.3 + breadcrumb `arg0` paralelo a `value_to_py(path: &str)` del intérprete; 8.7.3 helper async `__fitz_py_invoke_await` con detección `inspect.isawaitable` + ejecución vía `tokio::spawn_blocking + asyncio.new_event_loop().run_until_complete()` (baseline blocking, paralelo a 8.6.1 `py_coro_to_fitz_future`) + patrón canónico `<py_call>?.await` (paridad bit-a-bit con intérprete que rechaza `<call>.await` directo en runtime — el checker 8.7.3 lo rechaza estáticamente); 8.7.4 cierre formal con ejemplo `examples/python-interop-8.7.fitz` validado bit-a-bit `fitz run` ↔ `fitz build`. Total al cierre: 1295 unit + 88 E2E + 3 openapi con feature; 1204 + 79 + 3 sin feature. Clippy `-D warnings` limpio en ambos modos. **Deuda residual derivada** (NO bloquea Fase 8): coerción Python list/dict → Fitz `List<T>` / `Map<K,V>` / `Instance` (helpers `__fitz_py_to_list_*` ya emitidos, falta wiring en `coerce`); `.await` con binding intermedio split (`let fut = py_call()?; fut.await`); bundling CPython embebido (`fitz build --bundle-python`) — proyecto separado, decisión python-build-standalone vs PyOxidizer pendiente. Ver detalle en `docs/roadmap.md` → "Fase 8.7". | — | — |
| F17 | ~~`evaluator.rs` + `value.rs` + `env.rs` + `http.rs` + `codegen.rs`~~ | **CERRADO (2026-05-14, 1153 unit + 74 E2E)** — Send completo + paralelismo HTTP real + bridge HTTP eliminado. Seis sub-pasos: F17.1 dep `parking_lot`; F17.2 `Shared<T>` y `EnvRef` migran a `Arc<parking_lot::Mutex<T>>` (~284 sitios mecánicos `.borrow()/.borrow_mut()` → `.lock()`, `Rc::ptr_eq` → `Arc::ptr_eq`); F17.3 quitar `?Send` del `#[async_recursion]` en evaluator (13 sitios) + `FitzFuture: Pin<Box<dyn Future + Send>>` (fix colateral: `for` sobre List/Range materializa a `Vec<Value>` en vez de `Box<dyn Iterator>`); F17.4a `serve()` tokio `rt-multi-thread`; F17.5 eliminar bridge HTTP (`InterpTask`, `TaskTx`, `run_interpreter_loop`, `dispatch_request` viejo — ~269 LoC netas menos en `http.rs`, handlers axum invocan `handle_task(&registry, ...).await` directo sobre `Arc<HttpRegistry>` compartido, test helpers `run_oneshot_*` sin `LocalSet`/`select!`/canal); F17.4b codegen output paralela (`Rc<RefCell<>>` → `Arc<Mutex<>>` con std::sync, F12 closures `Arc<dyn Fn + Send + Sync>`, state HTTP `thread_local!` → `LazyLock<Arc<Mutex<T>>>`, runtime emitido `#[tokio::main]` default multi-thread, field access en bloque acotado `{ let __obj = ...; let __g = __obj.lock().unwrap(); __g.<f> }` para evitar deadlock por re-lock en `format!`, `PartialEq` custom por tipo nominal con helper recursivo `field_eq_expr`); F17.6 guía cap 19 sub-sección "Paralelismo HTTP real" + ejemplo `examples/guide/19b-paralelismo.fitz` validado a mano (5 reqs concurrentes en **1.2s** vs 5 en serie **5.3s**; pre-F17 ambos ~5s). Decisiones técnicas: `parking_lot::Mutex` para el intérprete, `std::sync::Mutex` para el codegen output (sin deps extras al Cargo.toml generado); política de re-entrancia "lock scope mínimo + clone-out" (auditoría manual en eval_call/EnvRef::get). **Deudas residuales que NO bloquean Fase 8**: benchmarks de `MutexGuard` vs `Ref<T>` (sin medir); lint o test que detecte patrones de re-lock potencial; LOADER del intérprete sigue como `thread_local! { RefCell<...> }` (re-carga módulos por worker, wasteful pero correcto). Ver detalle en `docs/roadmap.md` → "Fase F17". | — | — |

### Docs

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| D1 | `guide.md:4-5` | **PARCIALMENTE CERRADO** — el header ya cita "Fase 5b cerrada / 949 tests" (vs el original "Fase 5a / 784"). Sigue stale al estado actual (1043 tests, mini-fases post-5b cerradas). Mejor refresh recurrente cada vez que se mueve el contador, no deuda permanente. | Baja | Baja |
| D2 | `guide.md:881-883` | Cita métodos de Str y reenvía a cap 13, pero cap 13 no los desarrolla. Verificar. | Baja | Baja |
| D3 | ~~`syntax-spec.md:1-8`~~ **CERRADO (2026-05-14)** — header pasó a "BORRADOR v0.3 (post-F17)" con matriz rápida de estado actualizada: implementado/diseñado-no-implementado con referencias a capítulos de la guía y fases del roadmap. Refresh recurrente cada vez que se cierra una mini-fase o fase. | — | — |
| D4 | ~~Repo root~~ **CERRADO (2026-05-14)** — `CHANGELOG.md` creado con 9 entradas retroactivas: v0.1.0 (Fase 2) → v0.8.0 (Fase F17). Formato [Keep a Changelog](https://keepachangelog.com). Detalle técnico vive en `docs/roadmap.md`; el CHANGELOG es la vista condensada "qué cambió y cuándo". | — | — |
| D5 | ~~`guide.md:225-226`~~ | **CERRADO** — status codes custom implementados end-to-end en su mini-fase dedicada (ver bullet en "Próximos pasos"); cap 17 de la guía documenta la sintaxis con ejemplos. README puede quedar stale (cita "deuda residual post-5") — refresh menor cuando se mueva. | — | — |
| D6 | `guide.md:2725-2738` vs `:4305-4310` | Deudas residuales duplicadas en cap 13 y cap 18 (asignación a índice, state HTTP). Centralizar. | Baja | Baja |
| D7 | `README.md:38` | **CERRADO** (suficiente) — la nota actual ("la sintaxis `async fn` se parsea, pero el runtime sigue siendo síncrono") es clara. Re-evaluar cuando aterrice Fase 6 (Async nativo). | — | — |

### Linter (clippy)

**L1 entero CERRADO** — `cargo clippy --all-targets --all-features -- -D warnings` queda limpio. Los items originales L1a-L1f se resolvieron a lo largo de los sub-pasos post-5b; el último pase (3 warnings residuales: doc lazy continuation, let_and_return, expect_fun_call) cerró en una mini-sesión dedicada tras T1 batch 3. Re-correr `cargo clippy` antes de cualquier commit grande.

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
