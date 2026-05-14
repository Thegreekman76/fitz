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
| F15 | `parser.rs` | **Error recovery del parser** — hoy el parser aborta al primer error. Para tooling externo (LSP/IDE) que necesita dar diagnostics y completions sobre código incompleto/roto, el parser tiene que producir un AST parcial y seguir adelante. Refactor mediano: introducir nodos `Stmt::Error`/`Expr::Error`, sync points en `{`/`}`/`;`/newline, recolectar `Vec<FitzError>` en lugar de `Result<_, FitzError>`. Pre-req habilitante para Fase 9 (LSP). | Baja | Alta |
| F16 | `types.rs` (checker) | **IR tipado persistido por nodo** — el checker hoy sintetiza tipos en `infer_expr` y los descarta. Para hover ("¿qué tipo tiene esta expresión?") y completion contextual ("`u.` → mostrar fields de `User`"), hace falta retener `HashMap<SpanKey, Type>` (o un side-table paralelo al AST) con el tipo de cada nodo. Encaja también con el "IR tipado" que el doc ya menciona como sub-paso natural post-5b. Pre-req habilitante para Fase 9 (LSP). | Baja | Media |
| F18 | ~~`parser.rs` + `evaluator.rs` + `codegen.rs` + `types.rs`~~ **CERRADO (2026-05-14, PreF8.4)** — import aliasing con `as` (`import foo as f`, `from foo import bar as b`, alias mixto). Sub-paso adelantado de F8.1 para dejarlo con solo Python interop puro. Lexer suma `Token::As`; AST suma `Stmt::Import.alias: Option<String>` y cambia `Stmt::FromImport.names` a `Vec<(String, Option<String>)>`. Codegen emite `use foo::bar as b;` (fn/const) o `use foo::{T as L, TData as LData};` (type). Evaluator usa el `Value::Type.name` canónico al instanciar (no el alias sintáctico) para paridad bit-a-bit `fitz run` ↔ `fitz build` del Display. 9 unit + 4 E2E nuevos. Cap 16 de la guía documenta. | — | — |
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
