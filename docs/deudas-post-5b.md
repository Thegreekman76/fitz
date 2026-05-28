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
| ~~UTF-16 position strict~~ | ✓ **CERRADO v0.9.51** | LSP capability `positionEncoding: utf-8` declarada explícita |
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
  viven cross-module**. `GET /asyncapi.json` → 404 (bug paralelo
  al anterior). El codegen del runtime HTTP solo detecta `has_ws`
  mirando `program.ws_fns` local. Si `realtime.fitz` tiene los
  `@ws` pero el main solo hace `import realtime`, el flag
  `has_ws=false` en el main → ni el endpoint `/asyncapi.json` ni
  el schema se emiten. **Fix futuro v0.10.8**: paralelo al
  OpenAPI fix, mirar `loader.modules[*].ws_fn_stmts`.

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
