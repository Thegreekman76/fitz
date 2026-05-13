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
> end-to-end. T1 (tests frágiles del codegen) — primer y segundo
> batch cerrados: infra AST-based con `syn` + `quote`, **65/113**
> unit tests del codegen migrados de string-match a inspección de
> AST. S1.2 (span en Expr) — los 3 sub-pasos cerrados: variantes
> de `Expr` cargan `Span`, parser propaga spans en cada regla,
> checker (`infer_expr` + helpers) y evaluator (`eval_expr` +
> helpers + 14 métodos built-in) citan posición del nodo en
> errores. S1.codegen cerrado — 52 sitios del codegen migrados
> a `err_at` (con span del nodo); los 17 restantes son defensivos
> contra bugs del compilador (checker debió cazar), donde citar
> posición no aporta. **HTTP status codes custom cerrado** —
> sintaxis del spec `return <Int> { ... }` implementada
> end-to-end: AST (`Stmt::ReturnStatus`), parser (detecta el
> patrón después de `return <Int>` cuando viene un `{`), checker
> (acepta solo adentro de handlers HTTP), intérprete
> (`Value::HttpResponse` → outcome con el status pedido), codegen
> (override del return type a `__FitzResponse` cuando la fn HTTP
> contiene `ReturnStatus`, envoltura uniforme de returns
> normales y custom). Polimorfismo del spec: handler `-> User`
> puede mezclar `return user` (200) con `return 404 { ... }`.
> Cap 17 de la guía + ejemplo `examples/guide/17-http.fitz`
> actualizados; intérprete y compilador validados bit-a-bit
> con curl. Ver matriz para ítems pendientes (Pattern/TypeExpr
> sin span, T1 sucesivos batches, query params). **1026 tests
> pasando** (+16 dedicados: parser, checker, http, codegen, E2E).

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

Por **valor/esfuerzo**, en orden:

1. **L1 — Limpiar clippy** (Baja complejidad, alto valor): 12 errores
   + 25 warnings. La mayoría son auto-fixables (`cargo clippy --fix`)
   o triviales (`#[allow(clippy::approx_constant)]` en tests con
   `3.14`, eliminar imports no usados). **Resultado**: `cargo clippy`
   queda limpio, los CI futuros pueden bloquear regresiones nuevas.
2. **L2 — Helper `with_temp_output`** en codegen (Baja): patrón
   `mem::take(&mut self.output)` repetido 6 veces. Refactor a un
   helper genérico que toma una closure. Reduce ~40 líneas, hace
   futuros refactors más seguros.
3. **R1 — Validar `fn main` con decoradores no-`@server`** (Baja):
   hoy el codegen ignora silenciosamente `@get` si está sobre
   `fn main`. Sumar validación explícita: error claro si `fn main`
   tiene cualquier decorador HTTP que no sea `@server`.
4. **T1 — Refactor de tests frágiles a snapshot/AST-based** (Media):
   los unit del codegen usan `code.contains("string literal")`. Es
   poco realista cambiar los 100+ tests, pero un buen 30-40% se
   pueden mover a tests que validen *comportamiento* (compile + run)
   vs *forma textual*. Trabajo de granito incremental.
5. **S1 — Span en AST** (Alta complejidad, alto valor a largo plazo):
   agregar `Span { line: usize, col: usize, len: usize }` a `Expr`
   y `Stmt` (y opcionalmente `TypeExpr`), propagar desde tokens del
   parser, consumir en mensajes de error de checker/evaluator/codegen.
   Esto destraba mensajes de error útiles. Es trabajo grande
   (refactor amplio) pero el roadmap ya lo cita como pospuesto, y es
   condición habilitante para varias mejoras de UX. Sub-paso natural
   para post-5b.

Los otros ~40 hallazgos son **incrementales**: cada uno suma poco
solo, pero entre todos son una mejora de calidad significativa. Lista
completa abajo.

---

## Matriz completa de hallazgos

### Robustez

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| R1 | `codegen.rs:811-849` | `fn main` con decorators no-`@server` se ignora silenciosamente. Falta validación. | Media | Baja |
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
| L2 | `codegen.rs` 6 sitios | Patrón `mem::take(&mut self.output)` + restore repetido. Helper `with_temp_output(f)` lo abstrae. | Baja | Baja |
| M1 | `codegen.rs:779-920` | `generate_main_rs` (~140 líneas) mezcla particionado + validaciones + emisión. Partir en `partition_stmts`, `validate_http`, etc. | Media | Media |
| M2 | `codegen.rs:3529-3688` | `gen_http_handler_wrapper` (~160 líneas) hace todo: resuelve params, categoriza, emite. Extraer sub-fns. | Media | Media |
| M3 | `types.rs:664-1110` | `infer_expr` 446 líneas con mega-match de 30+ branches. Extraer branches grandes. | Baja | Media |
| M4 | `types.rs:1691-1866` | `check_stmt` 175 líneas. Repetición `push_scope`/`pop_scope` en 4 branches. Helper `with_scope(f)`. | Baja | Baja |
| M5 | `parser.rs` 3 sitios | `parse_list_literal`/`parse_call_args`/`parse_struct_lit_fields` parsean listas con coma con código similar pero no factorizado. | Baja | Media |

### Tests

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| T1 | `codegen.rs` tests | **Primer + segundo batch cerrados** — 65/113 unit tests migrados a AST-based con `syn` + `quote` (módulo `ast_test` adentro de `mod tests`). El batch 2 cubrió Listas/Mapas/Indexing/Métodos built-in (literales con vec!, indexing con borrow+clone, for snapshot, len/has/keys/values, map/filter/find/get, print de colecciones) + F12 closures (FnExpr suelta, fn nombrada como valor, fn como param y como return type, captura no-Copy, llamadas a vars Fn, FnExpr inline como arg). Helpers nuevos en `ast_test`: `local_init_expr`, `call_path`, `method_chain_names` (atraviesa Paren+Cast), `find_macro_args`, `count_method_calls_in_expr`, `contains_method_call_in_expr`, `contains_macro_in_expr`, `cast_target_type`. La fragilidad bajó de ~80% (pre-T1) a ~30% sobre los tests del codegen. **Pendientes** los tests de HTTP / Result-detail / módulos / fn-decl varios (~48 tests) y los `assert_err_contains` que son contratos de mensajes al usuario y no necesitan migración. Sumá conversiones cuando muevas `let` o cambies signatures del codegen y rompas tests viejos — es buen momento para reescribirlos AST-based en lugar de actualizar el string. | Media (sucesivos batches) | Media |
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
| F4 | `parser.rs` (Field default) | `type User { id: Int = 0 }` con default a nivel `type` field — el AST tiene `Field.default` pero el parser no lo popula en todos los contextos. Verificar. | Media | Media |
| F5 | `evaluator.rs:751`, `http.rs:27-29` | `is_async` en `FnDef` se ignora silenciosamente (deuda explícita). | Baja | Alta |
| F6 | `evaluator.rs:2098-2113` | Solo 2 builtins (`print`, `len`). Verificar si el syntax-spec promete más (`range`, `type_of`, `to_string`). | Baja | Baja (audit) |
| F7 | `lexer.rs:187-234` | Números: sin soporte `1_000` (separador), `3.14e-2` (notación científica). | Baja | Media |
| F8 | `lexer.rs:319` | Identificadores ASCII-only (`is_alphabetic()` pero después corta con `is_ascii_digit()`). Sin `π`, `función` como nombres. | Baja | Baja |
| F9 | `lexer.rs:252-279` | Escapes en strings limitados: faltan `\u{...}`, `\x..`, `\0`, `\b`. | Baja | Media |
| F10 | `parser.rs` | Encadenamiento multi-línea en method chains (`xs.map(f)\n.filter(g)`). Deuda explícita 3.4 del parser. | Media | Media |
| F11 | ~~`codegen.rs` (state HTTP)~~ **CERRADO** vía `thread_local! { static __FITZ_STATE_X: Rc<RefCell<T>> = ...; }` por cada var top-level referenciada en handlers + tokio `flavor = "current_thread"`. Cada fn que toca state materializa al inicio del body (`let X = __FITZ_STATE_X.with(|s| s.clone());`). Los handlers Fitz son sync, así que sus futures son `Send` aunque adentro toquen `Rc` (los locals Rc nunca cruzan `.await`). `examples/server.fitz` (CRUD completo) y `examples/guide/17-http.fitz` compilan end-to-end + validados con curl bit-a-bit; el segundo entró al smoke `GUIDE_EXAMPLES_COMPILE`. 5 tests nuevos (1 unit + 4 E2E con build + spawn + secuencia de requests). **Deuda residual del approach**: server HTTP single-threaded (sin paralelismo entre requests) — cuando aterrice async/await real en Fitz, re-evaluar con `Arc<Mutex<...>>` + `State` extractor. | — | — |
| F12 | ~~`codegen.rs` (higher-order)~~ **CERRADO** — closures escapadas, fn nombrada como valor, FnExpr asignado a var, fn como param y como tipo de retorno compilan con `fitz build`. `TypeExpr::Function` nueva variante; codegen emite `Rc<dyn Fn(...) -> R>` uniforme. Cap 11 anotado y compilable bit-a-bit con el intérprete. Smoke `GUIDE_EXAMPLES_COMPILE` incluye `11-funciones.fitz`. 24 tests nuevos. | — | — |
| F13 | `codegen.rs` | Listas/mapas heterogéneos: `[1, "dos"]` corre en intérprete, no compila. Requiere `FitzValue` tagged runtime. | Baja | Alta |
| F14 | `codegen.rs` | `let X = <expr>` no-literal a nivel mod top-level. | Baja | Media |

### Docs

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| D1 | `guide.md:4-5` | Header desactualizado: cita Fase 5a / 784 tests. Debe ser Fase 5b cerrada / 949. | Alta | Baja |
| D2 | `guide.md:881-883` | Cita métodos de Str y reenvía a cap 13, pero cap 13 no los desarrolla. Verificar. | Baja | Baja |
| D3 | `syntax-spec.md:1-8` | Header dice "BORRADOR v0.1" sin actualizar a Fase 5 cerrada. Falta marcar features ya implementadas. | Media | Media |
| D4 | Repo root | Sin `CHANGELOG.md`. Con 5 fases cerradas, vale un registro histórico. | Baja | Media |
| D5 | `guide.md:225-226` y otros | Status codes custom (`return 401 { ... }`) citados como "deuda" pero estado real ambiguo entre guía / README / spec. | Media | Media |
| D6 | `guide.md:2725-2738` vs `:4305-4310` | Deudas residuales duplicadas en cap 13 y cap 18 (asignación a índice, state HTTP). Centralizar. | Baja | Baja |
| D7 | `README.md:38` | Tabla async marca `🚧` con nota "se parsea pero runtime sync". Alinea con guía pero la nota podría ser más clara. | Baja | Baja |

### Linter (clippy)

| ID | Ubicación | Descripción | Prio | Comp |
|----|-----------|-------------|------|------|
| L1a | `lexer.rs:539`, `parser.rs:1914`, `value.rs:375` + 9 más | 12 errores: `3.14` literal en tests rechazado por clippy como "approximate value of PI". Falso positivo. | Alta | Baja |
| L1b | varios módulos | Warnings: unused imports (`PathBuf`, `Param`, `delete/get/post/put`), `unused variable: return_type`. | Baja | Baja |
| L1c | varios | Warnings: `if let` colapsables, `map_or` simplificables, `repeat().take()` más conciso, `vec!` innecesario. ~6 sugerencias auto-aplicables con `cargo clippy --fix`. | Baja | Baja |
| L1d | `error.rs` | Variantes `UndefinedFunction`, `NullReference` nunca construidas. Fields `expected`/`found` nunca leídos. Limpiar muertos. | Baja | Baja |
| L1e | `lexer.rs` | `EOF` flaggeado por "name contains capitalized acronym". Cosmético, ignorable con `#[allow]`. | Baja | Baja |
| L1f | `parser.rs` | `.unwrap()` sobre `field.default` después de chequear `.is_some()` → reemplazar por `if let Some(_)`. | Baja | Baja |

---

## Qué NO entró en la auditoría

- **Fase 6/7** (Interop Python, Ecosistema): decisión de roadmap, no
  auditoría.
- **Features del syntax-spec NO implementadas** todavía
  (status codes custom, async/await real, middleware, query params,
  headers): documentadas como dirección, no contrato. La auditoría
  solo señala donde docs/código discrepan sobre el estado actual.
- **Verificación bit-a-bit profunda** de cada feature: el smoke test
  E2E ya cubre los ejemplos compilables; no re-verifiqué cada uno.
- **Benchmarks de performance**: las menciones P1-P5 son
  observaciones sobre el código, no medidas. Si alguna duele, hace
  falta benchmark dedicado.

---

## Próximos pasos sugeridos

Una sesión razonable de cleanup ataca **L1 + L2 + R1 + D1** (todos
prio Alta/Media con complejidad Baja): ~2-3 horas de trabajo, deja
`cargo clippy` limpio, mejora la mantenibilidad puntual, y
sincroniza la guía con el estado actual.

**S1 (span en AST)** está cerrado en sus tres frentes: B.1 (Stmt),
S1.2 (Expr en checker + evaluator), y **S1.codegen** (52 sitios
del codegen con `err_at` + 17 internos con `err()` documentados
como defensivos). Mensajes de error pasan de `0:0` a línea/
columna precisas en cualquier camino del compilador (checker,
runtime, codegen). Pendiente residual menor: `Pattern` y
`TypeExpr` sin span — deuda explícita, baja prioridad porque
los errores de patrones suelen estar en sitios donde el match
contenedor ya provee un span razonable. El próximo trabajo
grande paralelo es **T1 batches sucesivos**: ~48 unit tests
del codegen todavía matchean strings literales del Rust
generado; cada uno migrado a AST-based con `syn` resiste
cambios cosméticos del codegen.

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
- **T1** (tests frágiles del codegen) — **primer + segundo batch
  cerrados**. Infra `ast_test` (módulo adentro de `mod tests`) parsea
  el Rust generado con `syn::parse_file` y expone helpers para buscar
  items, lets, signatures, derives, macro calls, method calls, loops,
  casts, etc. con stringificación normalizada via `quote::ToTokens`.
  **65/113** tests migrados de `assert_contains(code, "literal-string")`
  a inspección de AST. El batch 2 (28 tests) cubrió Listas/Mapas/
  Indexing/Métodos built-in + F12 closures, y sumó helpers nuevos
  (`local_init_expr`, `call_path`, `method_chain_names`,
  `find_macro_args`, `count_method_calls_in_expr`,
  `contains_method_call_in_expr`, `contains_macro_in_expr`,
  `cast_target_type`). Beneficio acumulado: cambios cosméticos del
  codegen (espacios, agrupación de paréntesis, sufijos numéricos
  alternativos) ya no rompen estos tests — solo cambios estructurales
  reales (renaming de tipos generados, eliminación de bindings,
  cambio de semántica) los rompen. Pipeline incremental: cuando rompas
  un test viejo del codegen al refactorizar, considerá reescribirlo
  AST-based en lugar de actualizar el string literal. **Pendientes**
  (~48 tests): HTTP wrappers, Result/`?`/match detail, módulos,
  declaración de fns varias.

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
