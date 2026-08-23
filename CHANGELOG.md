# Changelog

Cambios visibles del lenguaje Fitz, agrupados por hito. Sigue el
formato de [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
El detalle técnico de cada sub-paso vive en
[`docs/roadmap.md`](docs/roadmap.md); este archivo es la vista
condensada para alguien que pregunta "¿qué cambió y cuándo?".

Las versiones son retroactivas — Fitz todavía no publica releases
formales; cada bump corresponde al cierre de una Fase del roadmap.

## [v0.59.0] — 2026-08-23 — Tanda deudas dogfooding: FITZ-21 (loader `fitz test`) + FITZ-22 (aridad cross-módulo en `fitz check`) + residual FITZ-19 (`return Ok(())` en `@ws`)

Tanda de deudas del core encontradas dogfooteando MatHelp + fitz-liveviews.
Detalle en `docs/norte-mathelp.md` → FITZ-21 / FITZ-22 / FITZ-19.

### Fixed
- **FITZ-21 — `fitz test`: un test homónimo del módulo que importa ya no cicla.**
  El loader (`src/evaluator.rs`) gana `Loader.entry_file` (path canónico del
  archivo entry, que se evalúa directo y nunca entra al stack `loading`). En el
  loop de canonicalización de `load_module` se computa `self_path =
  loading.last().or(entry_file)` y se **saltea el candidato que canonicaliza al
  self**, así un `tests/foo.fitz` con `from foo import bar` resuelve al sibling
  `src/foo.fitz` (vía `import_root`) en vez de ciclar. Un ciclo mutuo A→B→A
  sigue detectándose (self=B, candidato=A). Threading:
  `eval_with_base_import_root_and_deps` + `install_loader_with_root` reciben el
  `entry_file`; `eval_test_source` (`src/main.rs`) pasa `fs::canonicalize(path)`.
  1 unit + 1 cli_e2e.
- **FITZ-22 — `fitz check` caza aridad/tipos incorrectos en llamadas a fns
  importadas (erradica check✓/build✗).** Antes el checker tipaba toda fn
  `from m import f` como `Type::Any` → el call site no validaba aridad (recién
  fallaba en `fitz build`). Nuevo pre-scan `pre_scan_imported_fn_signatures`
  (`src/main.rs`, paralelo a W12/B10) resuelve la firma real de cada fn
  importada (`extract_fn_signatures` en `src/types.rs`), la mapea bajo el
  binding local (alias-aware) en `TypeEnv.imported_fn_sigs`, y el handler
  `Stmt::FromImport` la registra como `Type::Function` (con defaults/varargs) →
  la maquinaria de validación existente (regla 5.3.2) valida el call site. Un
  param nominal no importado por el caller resuelve a `Any` (gradual-safe, sin
  falso positivo); la aridad igual se valida. Sin falsos positivos en
  `examples/admin` de fitz-liveviews (importa todo el framework, multi-módulo).
  7 unit + 2 cli_e2e.
- **Residual de FITZ-19 — un `return` explícito en un `@ws` handler emite
  `Ok(())`.** El fix de v0.58 cubrió el tail fall-through de un `@ws` que usa
  `?` (compila a `Result<(), String>`), pero no un `return`/`return null`
  **explícito** intermedio (el auth gate `Err(_) => { return }` de los sockets
  de admin): emitía `return ()` → `E0308`, bloqueando `fitz build
  examples/admin` (que nunca se había buildeado a nativo, solo `fitz run`). Fix
  en `gen_return` (`src/codegen.rs`): cuando `ret_expected == Null && e ==
  Expr::Null && ret_stack.last() == Result` → emite `return Ok(<null→ok>)`.
  Airtight-safe: ese patrón antes no compilaba, así que solo arregla código
  antes roto. 1 compile_e2e + `fitz build examples/admin` verde.

### Verificación
- `cargo test --lib` 4209 · `--features lsp` 4374 · clippy (default + lsp)
  `-D warnings` limpio · fmt limpio · cli_e2e (FITZ-21/22) 3/3 · compile_e2e
  targeted WS/Result/return/B16 12/12 · `fitz build examples/admin` (liveviews)
  → binario nativo. (El smoke completo `GUIDE_EXAMPLES_COMPILE` no se corrió a
  fondo por el estado del disco + caché en frío; el subconjunto targeted +
  admin nativo + cero errores de tipo/codegen reales cubren la regresión de
  `gen_return`.)

## [v0.58.1] — 2026-08-22 — `RandGen` como tipo nombrable: cruza funciones (FITZ-01(b))

Follow-up de v0.58.0 (dogfooding MatHelp). Un `RandGen` ahora puede viajar como
parámetro y retorno de función. Detalle en `docs/norte-mathelp.md` → FITZ-01(b).

### Added
- **`RandGen` como tipo nombrable — cruza fn (FITZ-01(b))** — antes un `RandGen`
  no podía viajar como parámetro ni retorno (`fn draw(g: RandGen, ...)` → el
  checker daba "unknown type" y sin anotar el codegen no lo infería). Ahora
  `resolve_type_expr` mapea `"RandGen" → Type::RandGen` e `infer_randgen_method`
  tipa los 6 métodos (`int→Int`, `float→Float`, `bool→Bool`, `choice→Result<T>`,
  `shuffle→List<T>`, `sample→Result<List<T>>`), así un `g` que llega por
  parámetro tiene métodos tipados. Verificado check/run/build/binario idénticos.
  2 unit tests `randgen_*_v0_58`.
- **Backfill de tests de FITZ-17 (missing-return)** — la fix del checker salió en
  v0.58.0 (`block_always_returns`) pero sus 4 unit tests `missing_return_*_v0_58`
  quedaron sin commitear; entran acá.

## [v0.58.0] — 2026-08-22 — `rand`/`num` cross-módulo en codegen + checker missing-return + `@ws` fall-through + tests importan `src/` (dogfooding MatHelp F2)

Cinco hallazgos del core encontrados construyendo el **primer juego de MatHelp
F2** (contrarreloj de las cuatro operaciones: generadores `rand.seeded`,
formateo `num` por locale, LiveComponent, cronómetro por WebSocket, persistencia
a Postgres). El juego compila a binario nativo y corre en **paridad bit-a-bit
`fitz run` ↔ binario**. Detalle en `docs/norte-mathelp.md` → FITZ-01/17/18/19/20.

- **`rand`/`num` cross-módulo en `fitz build` (FITZ-01(a), FITZ-04)** — un
  módulo IMPORTADO que usa `rand.*` (`rand.seeded`, `rand.int`, métodos de
  `RandGen`) o `num.*` (`num.format`/`percent`/`currency`) fallaba con
  `E0425 cannot find __FitzRng`/`__fitz_num_format` (el prelude se emitía en el
  crate root pero el módulo no recibía el `use crate::{...}`). Ahora el crate
  root emite los preludes si CUALQUIER módulo los usa (OR con `loader.modules`,
  patrón SMTP/Response/DB) y cada módulo importado emite su `use crate::{...}`.
  `LoadedModule` suma `uses_rand`/`uses_rand_global`/`uses_num`.
- **`getrandom` no inyectado con auth + rand global (FITZ-18)** — el chequeo de
  idempotencia `contains("getrandom")` daba falso positivo por el feature
  `["getrandom"]` de `rand_core` (que arrastra el auth) → el dep `getrandom`
  nunca entraba al Cargo.toml. Ahora matchea la línea de dep real.
- **Checker: bloque `{ }` sin `return` con retorno no-nullable (FITZ-17)** — una
  fn con cuerpo de bloque cuyo último statement es una expresión pelada (`fn a()
  -> Int { 5 }`, un `match` sin `return`) pasaba `fitz check` pero devolvía
  `null` en runtime (clase T2: check✓/run-da-null). El bloque `{ }` NO retorna
  su última expresión (solo el arrow `=>`). Ahora el checker lo rechaza. Blast
  radius verificado ≈ 0 (todos los ejemplos + fitz-liveviews usan `return`/`=>`).
- **`@ws` con `?` + fall-through (FITZ-19)** — un handler `@ws` que usa `?` se
  compila `-> Result<(), String>`; si el cuerpo caía por el final (gate con
  `match`, limpieza tras el loop) rompía con E0308. Ahora el codegen cierra con
  `Ok(())` (código muerto cuando el body diverge). Antes solo compilaba un `@ws`
  que terminara en un `loop {}` divergente.
- **Tests de `tests/` importan módulos de `src/` (FITZ-20)** — un `from
  gen_arith import X` en `tests/foo.fitz` no resolvía. Ahora el runner de `fitz
  test` pasa el dir del entry (`src/`) como `import_root` fallback del loader.

## [v0.57.0] — 2026-08-21 — Codegen: nominales cross-módulo de fns importadas + coerción `__FitzValue` en `match` (dogfooding MatHelp F1)

Dos fixes de codegen de la clase **check✓/build✗** (`fitz check` pasa, `fitz build`
rompe) encontrados construyendo la auth de MatHelp (F1 — registro/login/perfiles).
Ambos con paridad `fitz run` ↔ binario. Detalle en `docs/norte-mathelp.md` → FITZ-15/16.

### Fixed
- **FITZ-15 — un nominal retornado por una fn importada, no en scope del módulo
  consumidor, se degradaba a `Any`.** Un `let x = imported_fn().await` (o con `?`,
  o `match`) cuyo ret type es un nominal que el módulo consumidor NO importa hacía
  que el codegen degradara `x` a `Any` (`remap_imported_nominals` devuelve `Any`
  cuando `env.lookup(name)` es `None`), y `x.field` abortaba con `.field over Any` —
  aunque `fitz check` lo resuelve por el `TypeEnv` global del grafo de módulos.
  Nueva `auto_register_imported_fn_ret_nominals` (paralela a
  `auto_register_relation_targets` de v0.45): auto-registra el nominal + copia sus
  fields + emite el `use <mod>::{T, TData}`, recursando en
  `Result`/`List`/`Nullable`/`Map`/`Tuple`/`Function`/`Future`. Converge con W20
  hacia la solución completa (tipo concreto en vez de omitir la anotación).
- **FITZ-16 — el arm `Ok(v) => v` de un `match` sobre `Result<Any>` no coaccionaba
  a un primitivo.** `match m.get("k") { Ok(v) => v, Err(_) => "" }` con
  `m: Map<Str, Any>` (o `jwt.decode` → `Map<Str,__FitzValue>`): el arm `Ok` es
  `__FitzValue`, que no unifica con `String`/`i64`/etc → `E0308`, aunque el arm
  `Err` fija el LUB en un primitivo. `gen_match` ahora coacciona cada arm `Any` con
  `coerce(Any → primitivo)` cuando el LUB es Str/Int/Float/Bool (reusa el
  `__fv_to_*` de `Map<Str,Any>.keys()`). Habilita la forma natural sin el
  workaround `Err(_) => return ""` + anotación.

Verificado: repros + tests E2E (`fitz15_cross_module_fn_ret_nominal_infers_type_without_importing`,
`fitz16_match_result_any_arm_coerces_to_primitive`) + W20 actualizado, 4195 unit +
smoke 290 ejemplos verdes, y **MatHelp F1 compila a binario nativo sin ninguno de
los dos workarounds** (validado end-to-end con curl).

## [v0.56.0] — 2026-08-21 — Codegen: map literal `Map<Str,Any>` no-vacío en top-level de módulo (cierra el residual de v0.55)

Cierra el residual conocido de v0.55 — la última pieza del dogfooding de MatHelp.
Un map literal `Map<Str, Any>` **no-vacío** en el top-level de un módulo
(`let STORE: Map<Str, Any> = { "k": 10 }`) no compilaba con `fitz build`:
`gen_module_top_let` emitía el literal vía `gen_expr` SIN el hint de la
anotación, produciendo `Vec<(String, i64)>` (el tipo inferido del literal), y
después confiaba en `coerce` — que no tiene un arm `Map<K,V> → Map<K,Any>` que
envuelva las entradas → rustc `E0308` (`expected __FitzValue, found String/i64`).
Los `let` locales (vía `gen_assign`) y los maps construidos vacíos + `m[k] = v`
no lo sufrían.

### Fixed
- **`gen_module_top_let`** resuelve la anotación ANTES de generar la RHS y, si es
  `Map<_, Any>` (o `Nullable<Map<_, Any>>`), pasa el hint a
  `gen_map_lit_with_hint` (paralelo a `gen_assign`), que emite
  `Vec<(__FitzValue, __FitzValue)>` con las entradas envueltas. El import
  cross-module de `__FitzValue` lo cubre el content-scan.

### Notes
- Test: `v055_nonempty_map_str_any_literal_at_module_top_level` (el patrón EXACTO
  que falló primero: literal no-vacío top-level de módulo + `.keys()` +
  `.starts_with`, corre y cuenta bien).
- Verificación pre-bump: fmt + clippy (default + lsp) limpios, lib **4195**,
  smoke `GUIDE_EXAMPLES_COMPILE` verde. Sin cambios de LSP/grammar. Bump
  Cargo.toml 0.55.0 → 0.56.0 + extensión VSCode.
- **Cierra el bloque de bugs de codegen encontrados dogfoodeando MatHelp**:
  `@cookie` sobre `@ws` (v0.53), cookies cross-module (v0.54), `Map<Str,Any>.keys()`
  (v0.55) y map literal `Map<Str,Any>` no-vacío (v0.56).

## [v0.55.0] — 2026-08-21 — Codegen: `Map<Str,Any>.keys()`/`.values()` desenvuelve el lado concreto (destraba liveviews v0.50 nativo)

Cierra un bug de codegen (`fitz build`) **descubierto haciendo dogfooding de
MatHelp** al bumpear fitz-liveviews a v0.50.0. Su `dispatch_to_all` (FLV-08,
nuevo en v0.50) hace `for key in COMPONENT_STATE_STORE.keys()` sobre un
`Map<Str, Any>` y llama `key.starts_with(...)`. Como un `Map<Str, Any>` se
representa `Vec<(__FitzValue, __FitzValue)>`, `.keys()` emitía keys `__FitzValue`
en vez de `Str` → rustc `E0599` (`no method starts_with`) / `E0308`. **Rompía el
build nativo de cualquier proyecto con liveviews v0.50** (el codegen compila
todas las fns del módulo, aunque no se usen).

### Fixed
- **`Map<K, Any>.keys()` / `Map<Any, V>.values()`** — cuando la rep del map es
  `Vec<(__FitzValue, __FitzValue)>` (porque el OTRO lado es `Any`) pero ESTE lado
  es un primitivo concreto, `.keys()`/`.values()` ahora desenvuelven cada
  elemento a su tipo declarado (`__fv_to_string`/`_i64`/`_f64`/`_bool`), en vez
  de emitir el `__FitzValue` crudo. Helper nuevo `fv_unwrap_expr`. Un lado `Any`
  mantiene `.clone()` (es correctamente un `__FitzValue`); los maps totalmente
  concretos (`Map<Str, Str>`, etc.) emiten byte-idéntico. Cross-module cubierto
  por el content-scan que ya importa `__fv_to_*`.

### Notes
- Tests: 1 E2E nuevo `v055_map_str_any_keys_string_methods_in_module` (módulo con
  `Map<Str, Any>` construido vacío + index-assign — el patrón de liveviews —
  iterando `.keys()` con `.starts_with`, corre y cuenta bien). **Validado en el
  caso real**: MatHelp compila a binario nativo contra liveviews v0.50.0 (antes
  fallaba con el `E0599`/`E0308` de `dispatch_to_all`).
- Verificación pre-bump: fmt + clippy (default + lsp) limpios, lib **4195**,
  smoke `GUIDE_EXAMPLES_COMPILE` verde. Sin cambios de LSP/grammar. Bump
  Cargo.toml 0.54.0 → 0.55.0 + extensión VSCode.
- **Deuda residual conocida** (follow-up): un map literal `Map<Str, Any>`
  **no-vacío** (`{ "k": 10 }`) no coacciona sus entradas a `__FitzValue`
  (`E0308: expected __FitzValue, found String/i64`). liveviews lo evita
  construyendo el store vacío + index-assign. Se cierra en un release aparte.

## [v0.54.0] — 2026-08-21 — Codegen: cookies cross-module en `fitz build` (destraba el binario nativo de MatHelp)

Cierra un bug de codegen (`fitz build`) **descubierto haciendo dogfooding de
MatHelp**: un módulo importado que construye o serializa cookies no compilaba a
binario nativo, aunque `fitz check`/`fitz run` pasaran. Misma familia que
W23/W18/W11 (helper/tipo emitido en el crate root, módulo que lo usa sin el
`use crate::...`). **No es regresión de v0.53.0** — es el path de *escritura*
(`Response.cookies`), independiente del path de *lectura* WS que cerró v0.53.0.
Destraba la deuda más pesada de MatHelp: correr el **binario nativo** en Docker
(~9x perf + distroless) en vez del intérprete.

### Fixed
- **Cookies cross-module en `fitz build`** — dos gaps del path de escritura,
  ambos en módulos importados que usan el built-in `Response`:
  - **Gap #1 — llamar al serializador**: cualquier módulo cuyo handler retorna
    un `Response` emite la conversión `Response`→axum, cuyo loop de cookies llama
    `__fitz_serialize_set_cookie`. Ese helper vivía en el preludio HTTP del crate
    root como `fn` privado y el `.rs` del módulo no lo importaba → rustc `E0425`
    (`cannot find function`). Fix: el helper pasa a `pub(crate)` y se emite
    `use crate::__fitz_serialize_set_cookie;` en los módulos que usan `Response`
    (gemelo de escritura de `__fitz_parse_cookie`, que ya se importaba).
  - **Gap #2 — construir un `Cookie { ... }` literal** en un módulo (p. ej. un
    handler de login que setea la cookie de sesión) → rustc `E0422`
    (`cannot find struct CookieData`). Fix: `struct CookieData` y `type Cookie`
    (+ sus campos) pasan a `pub(crate)` y los módulos que usan `Response`
    importan `use crate::{Response, ResponseData, Cookie, CookieData};`.

### Notes
- Tests: 1 E2E nuevo (`v053_response_cookies_cross_module_emits_serialize_import`,
  módulo que retorna `Response { cookies: [Cookie {...}] }` importado por main) +
  `v019_response_cross_module_emits_imports` reajustado al nuevo import.
  **Validado end-to-end**: MatHelp (`d:\MathHelp`) ahora compila a binario nativo
  (`fitz build` → `✓ mathelp.exe`), donde antes fallaba con el `E0425` de
  `__fitz_serialize_set_cookie` en `src/assets.rs`.
- Verificación pre-bump: fmt + clippy (default + lsp) limpios, lib verde,
  smoke `GUIDE_EXAMPLES_COMPILE` verde. Sin cambios de LSP/grammar (fix interno
  del codegen). Bump Cargo.toml 0.53.0 → 0.54.0 + extensión VSCode.

## [v0.53.0] — 2026-08-21 — FITZ-05 residual: `@cookie` sobre `@ws` (cierra la última deuda del backlog MatHelp)

Cierra el residual de **FITZ-05** descubierto durante FLV-09 de fitz-liveviews:
un `@cookie(name="X")` sobre un handler `@ws("/...")` fallaba el chequeo de
aridad y, aunque pasara, el binding runtime no leía la cookie del handshake.
La lectura de cookies ahora funciona igual en HTTP y en WebSockets (el upgrade
WS **es** una request HTTP), con paridad bit-a-bit `fitz run` ↔ `fitz build`.
Elimina el workaround del admin (`@header(name="cookie")` + `locale_from_cookie`).

### Added
- **`@cookie(name="X")` sobre `@ws`** — un param del handler WS recibe el valor
  de la cookie nombrada, parseada del header `Cookie` del handshake de upgrade,
  paralelo a `@header`. `Str?` tolera la ausencia (`null`); `Str` requerida que
  falta **rechaza el handshake** (no hay 400 post-upgrade: en runtime cierra la
  conn, en codegen 400 pre-upgrade). Tres cambios coordinados:
  - **Checker** (`src/types.rs`, `check_ws_handler`) — suma `cookie_count` a la
    aridad esperada (`1 WsConn + 1 User + 1 por @header + 1 por @cookie`).
  - **Runtime WS** (`src/http.rs`, `build_ws_method_router`) — clona
    `route.cookies` y bindea la cookie con `parse_cookie_header` desde los
    headers del handshake, con el mismo manejo de error post-upgrade que
    `@header` (unregister + abort de la conn).
  - **Codegen WS** (`src/codegen.rs`, `gen_ws_handler_wrapper`) — emite el
    binding con `__fitz_parse_cookie` desde el `HeaderMap` del upgrade, con OR
    en el gate del `move` de la closure.
- **Completion `@cookie` en la LSP** (`src/lsp.rs`) — faltaba desde FASE A (solo
  estaba el tipo built-in `Cookie`); ahora el decorator se autocompleta con la
  forma `cookie(name="…")`.

### Notes
- Tests: 3 unit del checker (`ws_handler_with_cookie_accepts_extra_param`,
  `ws_handler_cookie_and_header_and_auth_arity`, `ws_handler_missing_cookie_param_is_error`)
  + 1 E2E de compilación (`ws_handler_with_cookie_builds_fitz05`). `lib` **4195**
  verde; smoke `GUIDE_EXAMPLES_COMPILE` verde; paridad run↔build validada con un
  cliente WS que manda `Cookie: lang=es` en el handshake (idéntico en intérprete
  y binario nativo). fmt + clippy (default + lsp) limpios.
- Guía cap 17 "Cookies y sesiones" precisa el matiz de `@ws`. Grammar TextMate
  sin cambios (los decoradores caen bajo la regla genérica `@<ident>`).
- **Con esto el backlog MatHelp queda cerrado entero** — todos los `FITZ-*`
  (core) y `FLV-*` (liveviews) de `docs/norte-mathelp.md`.

## [v0.52.0] — 2026-08-20 — `.fitzv`: cadenas `{#elseif}` (FLV-07) + error claro por `<style>`/`<script>` en el template (FLV-02)

Dos mejoras del pipeline de single-file components `.fitzv` (`src/view/`),
pedidas por el framework [fitz-liveviews](https://github.com/Thegreekman76/fitz-liveviews)
(norte MatHelp). Ambas son cambios del **parser de templates** — sin tocar el
AST, el checker, ni los dos emisores (SSR + client-WASM).

### Added
- **`{#elseif cond}` en templates `.fitzv`** (FLV-07) —
  `{#if a}...{#elseif b}...{#else}...{/if}` aplana las cadenas de condiciones
  (antes había que anidar `{#if}` dentro de `{#else}`). Implementado como
  **azúcar puro en el parser**: `{#elseif b}` desazucara a `{#else}{#if b}...{/if}`,
  un `{#if}` anidado en la rama else. Como el AST ya soporta esa forma y todo el
  pipeline (expand/check/SSR/WASM) recorre `else_children` recursivamente,
  funciona **igual en SSR y en el target client-WASM** sin cambios en los
  emisores. Verificado end-to-end: render SSR (chain A/B/C/F por score) + el
  ejemplo `examples/view/control-flow` compila a **WASM real** con la rama nueva.
  Completion del LSP suma `#elseif`.

### Changed
- **Error claro por `<style>`/`<script>` dentro de un `<template>` de `.fitzv`**
  (FLV-02) — antes daban un error confuso ("unexpected trailing tokens after
  expression (template interpolation)", disparado por el `{` del CSS). Ahora el
  parser los rechaza con un mensaje dirigido que apunta al workaround (CSS en un
  `<style scoped>` a nivel componente, o en el `head_extra` del layout; estilos
  state-dependent con class/style interpolados). Los comentarios HTML
  (`<!-- -->`) ya se descartaban sin romper.

### Notes
- 6 unit tests nuevos del parser (`{#elseif}` desugar / chain / sin else /
  stray error; `<style>`/`<script>` rechazados). `lib` **4192** verde.
- El nivel "warning" original de FLV-02 no aplica al `.fitzv` (nunca funcionó —
  siempre erraba); la mitad "runtime diff engine" (HTML renderizado con
  `<style>` → full-replace silencioso) vive en fitz-liveviews y queda como
  follow-up.

## [v0.51.0] — 2026-08-20 — FITZ-02: servido de archivos estáticos (`@server(static_dir=…)` + `--embed-static`) — cierra Hito 3/4 del norte MatHelp

### Added
- **`@server(port, static_dir="./public", static_prefix="/static")`** — sirve
  archivos estáticos desde el mismo servidor HTTP, sin nginx al lado. `static_dir`
  es el directorio en disco (relativo al working dir del proceso); `static_prefix`
  es el prefijo de URL (default `/static`). Un `GET /static/css/app.css` sirve
  `./public/css/app.css` con:
  - **`Content-Type` por extensión** (html, css, js, json, `webmanifest`, svg,
    png, wasm, woff2, mp3, pdf, … → resto `application/octet-stream`).
  - **`ETag` basado en contenido** (FNV-1a del archivo) + **`If-None-Match` →
    `304 Not Modified`**.
  - **`Cache-Control`** + **`Last-Modified`**.
  - **Path-traversal bloqueado**: `..` (y su forma encodeada `%2e%2e`) → `404`;
    además canonicalize + containment (bloquea escapes por symlink). Nunca sirve
    un archivo de afuera de `static_dir`. Las rutas exactas (tuyas + del sistema)
    ganan sobre el wildcard. `static_prefix="/"` sirve en la raíz.
- **`fitz build --embed-static`** — hornea los assets de `static_dir` **dentro
  del binario** con `include_bytes!` en build-time. El binario producido sirve su
  propio frontend **sin el directorio en disco** — un solo ejecutable
  self-contained, ideal para imágenes Docker `distroless`. Sin el flag, el binario
  lee del disco en runtime (mismo comportamiento que `fitz run`).
- Paridad bit-a-bit `fitz run` ↔ `fitz build`: mismo status, `Content-Type`,
  `ETag` y body en el intérprete y el binario (validado por E2E dedicado).
- Módulo nuevo `src/static_files.rs` con la lógica pura compartida (Content-Type,
  ETag, HTTP-date, `is_safe_relative`), mirroreada **literalmente** en el
  `STATIC_PRELUDE_*` del codegen para garantizar la paridad. Sin deps nuevas
  (`std::fs` + `include_bytes!` + `axum`, ya en scope).
- Guía cap 17 sub-sección "Archivos estáticos" (+ nota de deployment distroless)
  + ejemplo runnable `examples/guide/17m-static.fitz`. LSP: los kwargs
  `static_dir`/`static_prefix` en la completion de `@server`.

### Notes
- **Cierra FITZ-02 y el Hito 3/4 entero del norte MatHelp**
  (`docs/norte-mathelp.md`). Habilita **T3** (PWA instalable: favicon +
  `manifest.webmanifest`).
- Tests: 8 unit `static_files` + 2 unit `http` (`resolved_static_prefix` /
  `if_none_match_matches`) + 6 unit `codegen` (prelude/route/embed/collect) + 2
  E2E (`fitz02_static_disk_parity_content_type_etag_304_traversal`,
  `fitz02_embed_static_serves_without_dir_on_disk`). Ejemplo `17m-static.fitz`
  sumado al smoke `GUIDE_EXAMPLES_COMPILE`.
- MVP: sin directory index (un dir → 404); embed sin `Last-Modified` (no hay
  mtime en memoria, sí ETag idéntico); assets relativos al working dir del proceso
  (runtime) / del build (embed).

## [v0.50.0] — 2026-08-20 — Hito 3/4 del norte MatHelp: `Map.remove`, `is_in(<var>)`, paridad form-urlencoded, y limpieza de codegen

Continuación del backlog de MatHelp (`docs/norte-mathelp.md`) sobre
v0.49.0: cierra los quick wins de Hito 3/4 y el gap de paridad
descubierto al cerrar FITZ-05. Todo con paridad bit-a-bit `fitz run` ↔
`fitz build` (validado contra Postgres local para el ORM).

### Added

- **`Map.remove(key) -> Bool` (FITZ-13)** — borra la entrada `key` y
  devuelve `true` si existía. **Muta el Map in place** (semántica de
  referencia compartida, visible por cualquier alias — a diferencia de
  `with`/`merge` que devuelven un Map nuevo). Evaluator (`map_remove`,
  búsqueda lineal + `Vec::remove` preservando orden) + codegen con
  paridad + checker (`Map<K,V>.remove(K) -> Bool`) + LSP + guía.
  Desbloquea la eviction del store de componentes de fitz-liveviews
  (FLV-03).
- **`is_in(<var>)` con variable `List<T>` en el ORM (FITZ-07)** — un
  `.where(fn(u) => u.col.is_in(pendientes))` con `pendientes` una
  variable `List<T>` del scope externo emite `"col" = ANY($N::<oid>[])`,
  bindeando la lista entera como UN solo parámetro array (el OID sale
  del tipo escalar de la columna). La lista literal sigue emitiendo
  `IN ($1, $2, ...)`. Ideal para listas calculadas en runtime (ej. el
  motor adaptativo de MatHelp con skills pendientes). Paridad bit-a-bit
  validada contra Postgres real. Doc-comment corregido (ya no promete
  soporte sin cumplirlo).

### Fixed

- **Body `form-urlencoded` → tipo declarado en `fitz run`** — un handler
  `@post fn login(creds: Credentials)` que recibe un body
  `application/x-www-form-urlencoded` ahora deserializa al `type`
  declarado en `fitz run` igual que en `fitz build` (antes llegaba como
  `Map` crudo → `creds.user` explotaba con 500). `parse_urlencoded_body`
  construye un `serde_json::Object` all-string y reusa el mismo path que
  el body JSON (`json_to_instance`), paralelo al
  `__parse_urlencoded`→`__from_fitz_json` del codegen. **Cierra el gap de
  paridad** que bloqueaba el login zero-JS (`<form method=POST>`) en el
  intérprete. Descubierto al cerrar FITZ-05 en v0.49.0.
- **`.preload()` en el intérprete: error dedicado (FITZ-06)** — el eager
  loading está implementado en `fitz build` pero no en `fitz run`; ahora
  `.preload(...)` en el intérprete da un mensaje **dedicado** (apunta a
  `fitz build` + workarounds con navigation methods) en vez del genérico
  "QueryBuilder has no method `preload`". (La premisa original del norte
  de un "no-op silencioso" quedó stale — desde v0.47.0 ya erraba.)

### Changed

- **Codegen: sin paréntesis redundantes en el `match` (FITZ-12)** — un
  `let x = match …` / `return match …` deja de emitir `(match …)` (rustc
  avisaba `unnecessary parentheses`). Helper `strip_stmt_match_parens`
  (scanner balanceado fail-safe que skipea strings) aplicado en
  `gen_return`/`gen_assign`; las posiciones de operando/receptor
  (`(match …).foo()`, `1 + (match …)`) conservan los paréntesis. Reduce
  el ruido de warnings en el `cargo build` de los binarios generados.

## [v0.49.0] — 2026-08-20 — Hito 1 + Hito 2 del norte MatHelp: `rand`/`fs`/`num`, cookies, y paridad `run`↔`build`

Release combinado que cierra los dos primeros hitos del backlog surgido
de la auditoría para **MatHelp** (`docs/norte-mathelp.md`): el arranque
del juego (aleatoriedad), la red de confianza (paridad interpretado ↔
compilado) y el desbloqueo de deployment + login zero-JS + i18n. Detalle
por-tarea en `docs/norte-mathelp.md` (fichas `FITZ-*`).

### Added

- **Módulo `rand` (FITZ-01)** — aleatoriedad ciudadana de primera clase.
  CSPRNG global (`rand.int`/`float`/`bool`/`choice`/`shuffle`/`sample`/
  `bytes`, vía `getrandom`) + PRNG **sembrado determinístico**
  (`rand.seeded(N) -> RandGen`, algoritmo SplitMix64 fijo, NO el crate
  `rand`). Criterio de aceptación: `rand.seeded(N)` produce la MISMA
  secuencia en `fitz run` y `fitz build`, para siempre (habilita guardar
  `seed + índice` y reconstruir una partida desde dos enteros). Módulo
  nuevo `src/rand.rs`, `Value::RandGen`/`Type::RandGen`, paridad bit-a-bit
  codegen, checker + LSP, ejemplo `examples/guide/13w-random.fitz`, cap
  "Aleatoriedad" en la guía.
- **Módulo `fs` (FITZ-03)** — filesystem en runtime: `fs.read`/`read_bytes`/
  `write`/`append`/`exists`/`list`/`remove`/`mkdir_all`. `Result<T>` con el
  path citado en el error, nunca panic; paths relativos al working dir.
  Módulo nuevo `src/fs.rs` sobre `std::fs` (sin deps extra), paridad
  bit-a-bit codegen (`FS_PRELUDE`), checker + LSP, sección "Filesystem" en
  la guía. Habilita catálogos i18n desde JSON leídos al boot (T1).
- **Módulo `num` — formateo locale-aware (FITZ-04)** — `num.format`/
  `num.percent`/`num.currency` con `es-AR` (`1.234.567,00` / `42,0 %` /
  `$ 1.250,00`) y `en-US`. Tabla de locales embebida (sin ICU), paridad
  bit-a-bit codegen, ejemplo `examples/guide/13x-num-locale.fitz`. (Los
  format specs `{n:,}`/`{r:.1%}` ya compilaban — solo faltaba locale.)
- **API de cookies (FITZ-05)** — **leer** con `@cookie(name="X")`
  (decorator apilable como `@header`, inyecta el valor parseado en un
  param `Str`/`Str?`, opcional `into="alias"`, sobre `@get`/`@post`/`@ws`;
  `parse_cookie_header`; OpenAPI `in: cookie`) y **escribir** con el campo
  `cookies` de un `Response`. Nominal built-in nuevo `Cookie` (8 campos con
  defaults: `name`/`value` requeridos, `path="/"`, `http_only=false`,
  `secure=false`, `same_site="Lax"`, `max_age: Int?=null`,
  `domain: Str?=null`); cada `Cookie` → un header `Set-Cookie`. Helper de
  serialización compartido (`serialize_set_cookie` intérprete /
  `__fitz_serialize_set_cookie` codegen) con paridad bit-a-bit; LSP + cap
  17 "Cookies y sesiones". Con esto + form-urlencoded (ya soportado en
  `fitz build`), el login de familia es HTML puro, cero JS.
- **`git` + `ca-certificates` en la imagen oficial de Docker (FITZ-11)** —
  la imagen estándar (build-capable) ahora resuelve `{ git = ... }` deps
  (hoy la única forma de dep externa). Fix de una línea en el
  `apt-get install` del `release.yml`.

### Fixed

- **Codegen: `-> T?` con `return` (FITZ-09)** — una fn con return type
  `Nullable` que hace `return null` / `return <valor>` generaba Rust roto
  (`return ()` donde va `return None`; valor sin envolver en `Some(...)`) →
  `E0308` en `fitz build`, aunque pasara `fitz check` y corriera en
  `fitz run`. La coerción nullable ahora se aplica también en **posición de
  return**. Destraba el binario nativo de **todo lo que use
  fitz-liveviews** (`flv_cookie` tiene el patrón exacto), incluidas
  `examples/admin` y las apps que dependen del framework.
- **Codegen: `Str + Any` (FITZ-10)** — `let chars = []` (infiere
  `List<Any>`) + `chars.push(<Str>)` + `out + chars[0]` pasaba `fitz check`
  y fallaba `fitz build`. Ahora el codegen coacciona `Str + Any` como el
  intérprete + detecta `List<Any>`/`Map<_, Any>` para emitir el preludio
  `__FitzValue` en CLI.
- **`Set-Cookie` múltiple (FITZ-05 FASE B)** — `outcome_to_response`
  emitía los headers extra con `.insert` (sobrescribe: solo sobrevivía la
  última cookie); ahora usa `.append` para `Set-Cookie` (todas las cookies
  sobreviven), preservando la semántica de sobrescritura para los demás
  headers.

### Testing

- **Differ de paridad `fitz run` ↔ `fitz build` (FITZ-14)** — harness
  nuevo `run_build_parity_corpus_fitz14` que corre un corpus de ejemplos
  CLI-puros por las dos vías y assertea stdout idéntico bit-a-bit. Es la
  red que hubiera cazado FITZ-09/FITZ-10/FITZ-06 solos; protege el criterio
  de reproducibilidad de `rand`. Corpus curado (extensible).

### Notas

- **Deuda residual descubierta (clase FITZ-14, NO FITZ-05):** en
  `fitz run` un body `form-urlencoded` llega al handler como `Map` en vez
  de deserializar al `type` declarado (en `fitz build` sí tipa). El login
  zero-JS funciona compilado; con `fitz run` usar body JSON mientras tanto.

## [v0.48.0] — 2026-08-18 — Hidratación de composición dinámica keyed (`<Child key>` dentro de `{#for}`) en el target client-WASM de `.fitzv`

Cierra la última deuda de composición del `.fitzv` en hidratación: un
`<Child key="{...}" />` dentro de un `{#for}` en un componente naive
hidratable (`component App hydrate`) ahora ADOPTA el DOM server-pintado
en vez de dejar la lista muerta hasta el primer re-render. Cada ítem se
reconcilia por su keyed instance cache (`__child_map_<n>`), así que el
estado local del hijo (ej. un contador) sobrevive los re-renders naive.

**Diagnóstico** (repro-confirmado): no había un rechazo de build — el
guard `if ctx.in_for` de `emit_child_component_adopt` era código muerto
(inalcanzable), y el `{#for}` en modo adopt solo consumía los anchors
`<!--fr-->`/`<!--/fr-->` sin descender. Los wrappers
`<div class="__fitz-child-<Name>">` server-pintados quedaban visibles
pero muertos al boot (sin listeners), y el primer re-render los
reconstruía perdiendo la identidad del DOM server.

**Fix** (~55 LoC de núcleo, solo `src/view/codegen_wasm.rs`): borrar el
guard muerto (el cuerpo del adopt ya era dual static/dynamic, delega a
`emit_child_get_or_create` que bifurca por `ctx.in_for`) + rutear el
`{#for}` con sitios dinámicos al nuevo helper
`emit_naive_dynamic_region_adopt`, que consume `<!--fr-->`, corre
`emit_for` (snapshot de la lista de state ya restaurada + adopción de un
wrapper por ítem con `__flv_next_element` + reconciliación keyed
`__child_map_<n>`/`__seen_<n>`/`retain` — la exacta maquinaria del build
walk), y consume `<!--/fr-->`. El SSR no cambia (ya pinta los anchors +
los wrappers `__fitz-child-` por ítem). El `{#for}` estático sigue en
`emit_naive_region_skip` byte-idéntico.

**Byte-compat**: aditivo, gated tras el marcador `hydrate` — los 25
ejemplos `examples/view/` previos regeneran byte-idénticos. Validado en
Chrome real 7/7 (adopción cross-boundary del wrapper dinámico + keyed
cache que preserva `taps` a través del re-render + listeners vivos al
boot). Ejemplo nuevo `examples/view/hydrate-keyed-composition/` + smoke
`view_hydrate_keyed_composition_wasm_smoke.rs`. Cierra el catálogo de
composición del `.fitzv` en hidratación entero (estática v0.30.4 +
región estática v0.41.4 + keyed dinámica v0.48.0).

## [v0.47.0] — 2026-08-18 — ORM read/chain methods directos sobre el `@table` type en `fitz run` (paridad con el codegen)

Cierra una discrepancia intérprete↔codegen descubierta al repro-confirmar
v0.46.0: `User.first(db)` (y `User.count/order_by/limit/offset/group_by/sum/avg/
min/max`) fallaban en `fitz run` con `type 'User' has no static method named
'first'`, mientras el codegen (`fitz build`) sí los soporta directo sobre el
type. Antes el intérprete solo aceptaba `User.all(db)` / `User.where(...).first(db)`.

### Arreglado

- **Read/chain methods directos sobre el `@table` type** en el intérprete —
  `orm_dispatch_type_method` ahora expone `first`, `count`, `order_by`, `limit`,
  `offset`, `group_by`, `sum`, `avg`, `min`, `max` (además de los previos `all`/
  `where`/`insert`/`bulk_insert`), construyendo el `QueryBuilderState` base y
  delegando al dispatch del QueryBuilder. `User.first(db)`, `User.order_by(-u.id).all(db)`,
  `User.count(db)`, etc. funcionan directo, paridad con `fitz build`.

### Detalles

- **No toca los mutating**: `update`/`delete` directos sobre el type siguen sin
  exponerse (necesitan un `.where(...)` previo — igual que el codegen los rechaza).
  `preload` sigue sin implementarse en el intérprete (feature grande, paralelo al
  scope de v0.46.0).
- **No shadowea custom methods**: el dispatch de custom methods del `type` corre
  ANTES del ORM (un `fn first(...)` custom gana sobre el `.first` del ORM).
- Test E2E real Postgres `orm_type_static_read_methods_v047` (`#[ignore]`,
  opt-in): `NavxUser.first(db)` / `.count(db)` / `.order_by(...).all(db)` directos.

## [v0.46.0] — 2026-08-18 — ORM cross-module: navigation methods en `fitz run` (paridad con el codegen)

Cierra la deuda paralela documentada en v0.45.0: un navigation method
(`user.posts(db)`) de un `@has_many`/`@has_one`/`@belongs_to` sobre un `@table`
type importado ya funciona en `fitz run` sin exigir importar el target type
explícitamente. Antes `from models import User` + `user.posts(db)` (con
`@has_many("Post")`) fallaba con `navigation: type 'Post' referenced by the
relation is not defined`; el workaround era `from models import User, Post`.
El codegen (`fitz build`) ya lo resolvía desde v0.45.0 — esto trae la paridad
al intérprete.

### Arreglado

- **Navigation ORM cross-module en `fitz run`** — cuando el target de una
  relation no está en el env del módulo llamador (import parcial), el intérprete
  ahora lo resuelve desde un **registro global de `@table` types** (nombre →
  fields + `TableMetadata`) poblado en `Stmt::TypeDef` durante el eval. No-op
  cuando el target SÍ está importado.

### Detalles

- **Por qué global y no el LOADER thread-local**: los handlers HTTP corren en
  **worker threads** de tokio, que NO heredan thread-locals — el `LOADER` es
  `None` ahí. El registro global (`OnceLock<Mutex<...>>`, poblado en el main
  thread durante el eval, visible desde todos los threads) es el mismo patrón
  que el codebase ya usa para `@background` (`install_background_registry`) y
  `ws_broadcast`. Validado a mano: navigation en un handler HTTP (worker thread)
  con import parcial devuelve 200 correcto.
- **Alcance**: navigation methods (`user.posts(db)`, `post.author(db)`). `.preload`
  NO existe en el intérprete (feature grande, sigue fuera de scope). Runtime
  analogue del `auto_register_relation_targets` del codegen (resuelve on-demand
  por navegación en vez de pre-registrar; suficiente porque cada navigation es
  un solo hop).
- **Deuda residual**: el registro es por nombre — dos `@table` types homónimos
  en módulos distintos colisionan (first-writer-wins; raro, los model names
  suelen ser únicos). Paralelo a la limitación del registro del codegen.
- Test E2E real Postgres `orm_navigation_cross_module_v046` (`#[ignore]`,
  opt-in con `FITZ_TEST_PG_URL`): import solo `User`, `u.posts(db)` cross-module.

## [v0.45.0] — 2026-08-18 — ORM cross-module: auto-resolución de relation targets en `fitz build`

Cierra la deuda residual de W17 (v0.10.7): un `.preload("relation")` (o navigation
method) de un `@has_many`/`@has_one`/companion sobre un `@table` type importado ya
NO exige importar el target type explícitamente. Antes `from models import User`
+ `User.preload("posts").all(db)` (con `@has_many("Post")`) fallaba en `fitz build`
con `type 'Post' no registrado en el TypeEnv`; el workaround era `from models
import User, Post` con TODOS los targets referenciados.

### Arreglado

- **Auto-registro de relation targets** — el codegen ahora, antes de emitir,
  recorre las relations de cada `@table` type importado y auto-registra los
  targets que faltan (`Post`) en un **clon local del TypeEnv** usado solo para
  la emisión. Reutiliza toda la maquinaria de imports existente (el loader ya
  tenía `Post` completo: fields + `TableMetadata`; solo faltaba mint-earle un
  `TypeId`). Worklist a fixpoint → cubre targets transitivos (`Post → Tag`).
  Aplica tanto al main (`generate_main_rs`) como a un `.preload` que vive en un
  módulo importado (`generate_module_rs_with_bindings`).

### Detalles

- **Opción B (clon del env)**: el `env` del codegen es inmutable y los `TypeId`
  solo nacen en el checker para los nombres literalmente importados. El fix
  clona el env localmente, hace `declare_nominal` de los targets faltantes, e
  inyecta bindings sintéticos `Named{Type}` en `module_bindings` con
  `entry().or_insert` (un import real del usuario siempre gana → no-op cuando el
  target ya está importado). No toca el checker ni el intérprete.
- Solo registra un target que ALGÚN módulo cargado define en `type_sigs`; un
  target inexistente deja el error pre-existente en pie.
- **Alcance codegen-only**: `fitz run` (intérprete) tiene un gap análogo por
  otra vía (resuelve el target desde el value-env en runtime, y no implementa
  `.preload`) — queda como deuda paralela documentada, fuera de este fix.
- 2 tests E2E nuevos (`cross_module_orm_preload_auto_registers_target_in_{main,module}_v045`).
  El smoke `cross_module_orm_virtual_fields_skip_w17` (importa todos los types)
  sigue verde: con el target ya importado, el auto-registro es no-op.

## [v0.44.0] — 2026-08-18 — Named slots en el emitter SSR (`.fitzv` paridad WASM↔SSR)

Cierra la **Deuda C**: el emitter SSR de `.fitzv` ahora soporta **named slots**
(`<slot name="X" />` + `slot="X"` del padre), que el target client-WASM tenía
desde v0.24.0 pero el SSR **detectaba y rechazaba** ("named slot is client-WASM
only"). El mismo `.fitzv` con named slots ahora compila a los dos targets.

### Agregado

- **`<slot name="X" />` en SSR** — un componente con named slots emite un param
  posicional `__slot_<name>: Str` por cada uno (tras el `__slot` del default, en
  orden de declaración — `ssr_slot_params` fija el orden y lo comparten la firma
  del render fn y la llamada del padre). El padre rutea contenido con
  `<... slot="X">` al named slot correspondiente y el resto al default; el
  atributo `slot="X"` se strippea del DOM emitido. Un slot que el padre no llena
  pasa `""` → el hijo pinta su propio fallback (`<slot name="X">fallback</slot>`).
- Modelo portado 1:1 del WASM (`slot_field_name` / `element_slot_attr` /
  `strip_slot_attr` / `validate_slot_set`), mantenido local en `codegen_ssr.rs`
  con prefijo `ssr_` (misma política que `ssr_slot_shape`).

### Detalles

- **Validación**: `slot="X"` para un slot que el hijo no declara → error que
  nombra el slot desconocido + lista los declarados. Dos named slots que foldean
  al mismo backing param (`side-bar` vs `side_bar`) → error de colisión.
- **Byte-compat**: el path **sin** `slot="X"` (default slot solo, o composición
  sin slots) queda **byte-idéntico** al 11.7.d/SSR-4 — con named routing ausente,
  todo el contenido va al default como antes. Los 6 tests `ssr4_*` siguen verdes;
  el `examples/view/` no regenera (el cambio es SSR, no toca el emitter WASM).
- 5 tests SSR-C nuevos (`ssrc_*`): emisión del param + lectura, fallback,
  ruteo padre named+default, slot desconocido rechazado, colisión de identificador.

## [v0.43.0] — 2026-08-18 — `fitz check` valida los módulos `.fitz` importados (cross-module)

Cierra la **Deuda A**: `fitz check` sobre un proyecto multi-módulo ahora recorre
el grafo de imports y type-checkea **cada módulo `.fitz` clásico importado** — no
solo el entry. Antes un error de sintaxis (o de tipos) en un módulo importado
pasaba `fitz check` limpio y solo explotaba en `fitz run`. Ahora `check` predice
`build`.

### Arreglado

- **`fitz check` cross-module** — el entry `.fitz` ahora walkea sus imports
  (transitivo, dep-aware) y corre lexer + parser + checker sobre cada módulo
  local importado. Un `Err(_) => return,` bare en un `empleados.fitz` importado
  falla `fitz check` con exit 1 + span exacto (`empleados.fitz:4:21`), en vez de
  pasar limpio y romper recién en `fitz run`.
- **Contexto del proyecto heredado** — cada módulo se chequea con el
  `@auth_provider` / `@background` / `@live_component` pre-escaneados del entry
  como fallback (paralelo a los `main_imported_*` del loader del codegen: W12 /
  B10 / §9.bb). Así un handler `@authenticated` en un módulo que no importa el
  provider directamente (lo cablea el entry, patrón B12) no da un falso positivo
  "no `@auth_provider` registered".

### Detalles

- **`.fitzv`** se saltean en el walk (los cubre el sweep a2 + el pipeline de
  view, que hace expand + checks propios). **Deps externas** de `fitz.toml` se
  saltean (`build` las valida y su propio CI también) — `check` se enfoca en el
  código del usuario.
- **Ciclos + dedup**: un `visited` set por path canónico dedupea imports
  repetidos y termina en ciclos (`a → b → a` no cuelga).
- **Single-file**: `fitz check foo.fitz` (sin manifest) también valida los
  imports relativos de `foo.fitz` (registro de deps vacío → resolución
  sibling-only).
- **Limpieza de docs**: la entrada stale de `deudas-post-5b.md` sobre la
  inferencia post-`match Result → Option<String>` queda marcada CERRADA — B16
  (v0.39.1) la cerró como efecto colateral (los arms divergentes se tipan `Any`
  y el LUB del match cede al concreto), verificado contra el binario actual.
- 7 tests cli_e2e nuevos (`deuda_a_check_*`): repro de sintaxis, type error,
  transitivo, ciclo, single-file, dep-skip, happy-path multi-módulo. Smoke verde
  sobre `api-orm-full` / `api-orm-full-fullstack` / `api-fullstack-postgres`
  (multi-módulo, incluye el patrón B12 y subcarpetas).

## [v0.42.2] — 2026-08-17 — `@every` en un módulo importado compila a binario (`fitz build`)

Cierra la deuda D4 de `@every`: paridad `fitz run` ↔ `fitz build` para un
`@every(N)` declarado en un **módulo importado** (antes `fitz build` lo rechazaba
con un guard; `fitz run` sí lo soportaba).

### Arreglado

- **`@every` cross-module en `fitz build`** — igual que `@cron` (B19):
  `LoadedModule` suma `every_fn_stmts`, el codegen puebla `every_jobs_info` desde
  los módulos con `module_path: Some(mod)`, y `emit_every_job_spawns` emite
  `crate::<mod>::<fn>` cuando el `@every` vive en un módulo importado (antes solo
  el bare `<fn>` del main file). Un programa que hace `import worker` de un
  módulo con `@every(1)` ahora compila y tickea desde el binario nativo. Nuevo
  compile_e2e `every_in_imported_module_builds_and_ticks_d4`.

## [v0.42.1] — 2026-08-17 — `ws_broadcast(...)` funciona desde un scheduler (`@every`/`@cron`)

Fix que destraba el patrón canónico de `@every` en fitz-liveviews: un reloj /
heartbeat **global** que `ws_broadcast(...)` a un endpoint LiveView cada segundo.

### Arreglado

- **`ws_broadcast(endpoint, msg)` desde un `@every`/`@cron` fn** — el builtin
  resolvía el broadcaster **solo** por el thread-local `HTTP_REGISTRY`, que NO
  está seteado en los workers tokio donde corren los schedulers → el broadcast
  era un **no-op silencioso**. Ahora hay un **broadcaster global instalado** al
  boot (`install_ws_broadcaster`, paralelo a `install_background_registry`) y
  `ws_broadcast_to_endpoint` cae a él cuando el thread-local no está presente.
  Un `@every(1)` que `ws_broadcast(...)` a `/live/clock` ahora llega a todos los
  clientes conectados. Validado headless: la hora del reloj avanza cada segundo.

## [v0.42.0] — 2026-08-17 — `@every(N)`: decorator de tareas periódicas por intervalo

Decorator nuevo del lenguaje **`@every(N)`**: corre una fn top-level cada N
segundos, server-wide desde el boot — el análogo de intervalo de `@cron`, más
simple (un período, sin expresión cron, así `@every(90)` y `@every(0.5)`
funcionan donde una expresión cron no puede). Paridad bit-a-bit `fitz run` ↔
`fitz build`.

### Añadido

- **`@every(N)`** sobre una fn top-level (sync o async): la corre cada N
  segundos (`N` un literal numérico positivo, Int o Float; mínimo 0.001 s). Sin
  params, sin kwargs (a diferencia de `@cron`; `tz`/`retry`/`catch_up`/`store`
  son cron-only por ahora), return `Null`/`Result`/`Future` (el valor se
  descarta). Primer tick **después** del primer intervalo (no al arrancar); los
  ticks perdidos se saltean. No combinable con `@get`/`@post`/`@ws`/`@cron`/
  `@background`/`@auth_provider`/`@test`/CLI/live-handlers.
- **Runtime (`fitz run`)**: `EveryJob`/`EveryRegistry` + `spawn_every_scheduler`
  en `cron_jobs.rs` (un `tokio::spawn` por job con `tokio::time::interval`,
  `MissedTickBehavior::Skip`); registrado vía `HttpRegistry.every_registry`. El
  every-scheduler arranca junto a axum + el cron-scheduler; un programa
  `@every`-only (sin HTTP) queda vivo bloqueando en Ctrl+C.
- **Codegen (`fitz build`)**: prelude `__fitz_run_every_job` (tokio-only) +
  `emit_every_job_spawns` (un `tokio::spawn` por job, invoke que descarta el
  return, paridad con el intérprete). `docker init` trata `@every` como servicio
  long-running (`restart: unless-stopped`).
- **LSP**: completion de `@every` en la lista de decoradores. **Guía**:
  sub-sección `@every` en el cap 30 (Jobs sin Celery), junto a `@cron`.

### Notas

- **Uso canónico en fitz-liveviews**: un reloj/heartbeat global que
  `ws_broadcast(...)` a un endpoint LiveView cada segundo — un solo ticker para
  todos los clientes (en vez de uno por conexión con `spawn`).
- **MVP**: `@every` debe declararse en el archivo principal — `@every` en un
  módulo importado es soporte de `fitz run` por ahora; `fitz build` lo rechaza
  con un error claro. Deuda menor de codegen: un programa `@every`-only también
  arrastra las deps de cron (el gating se pliega en `uses_jobs`) — optimizable.
- Validado: `fitz check` acepta/rechaza los shapes correctos; `fitz run` y el
  binario de `fitz build` ambos dan la misma cadencia de ticks; regresión
  cron/background verde.

## [v0.41.5] — 2026-08-16 — `.fitzv`: `<style scoped>` en un root que hidrata

Levanta una restricción de autoría de la hidratación: un componente que hidrata
ahora puede llevar `<style scoped>` (o `<style global>`) en su RAÍZ. Antes había
que poner el CSS del componente en el `<head>` del host (o dejaba el árbol
hidratado "muerto" — los eventos nunca se cableaban).

- **Causa raíz**: el emisor SSR prepende el bloque `<style>` scoped como el
  primer hijo del render, y el build cliente inyecta ese CSS al `<head>`. El
  adopt walk arrancaba en `root.first_child()` y — como `<style>` es un Element —
  `__flv_next_element` lo tomaba como la raíz del template, desalineando todo el
  walk un nodo (el `@input`/`@click` nunca se cableaba, sin panic ni error de
  compilación — un árbol hidratado silenciosamente roto).
- **Fix (CHICO)**: `emit_hydrate_method` (la fn compartida por root y child)
  saltea un `<style>` líder al inicio del walk (`tag_name() == "STYLE"` →
  `next_sibling`), **gated por `component.style.is_some()`** — un componente sin
  estilo emite código byte-idéntico. Como la fn es compartida, además vuelve
  **correcto-por-construcción** un child compuesto estilado (antes "funcionaba"
  solo porque el adopt de un child naive es inerte; un child keep-node estilado
  habría roto).
- **Byte-compat**: los `examples/view/*` del core no llevan scoped style en
  componentes hidratables → SSR + WASM byte-idénticos (view suite 736 verde). Los
  demos de composición de fitz-liveviews (Badge estilado compuesto) rebuildean con
  el adopt del child mejorado — mismo DOM runtime, re-validados headless.
- **Validado en Chrome real 6/6**: un root `component App hydrate` con
  `<style scoped>` + un `<input>` vivo → state restaurado, DOM adoptado (witness
  sobrevive), el estilo scoped aplica, **el `@input` se cablea + actualiza** (lo
  que rompía pre-fix), sin errores. Test
  `phase_11_12_hydrating_root_with_scoped_style_skips_leading_style_v0_41_5`.

Sin sintaxis nueva; sin impacto en grammar/LSP (cambio interno del emisor WASM).
Simplifica la autoría: los estilos del componente pueden vivir co-locados
(`<style scoped>`) en vez del `<head>` del host.

## [v0.41.4] — 2026-08-16 — `.fitzv`: hidratación de regiones `{#if}`/`{#for}` en un árbol de composición

Cierra el ítem 2 de las deudas residuales de hidratación de composición (Fase
11.12): un componente naive (composición) con el marcador `hydrate` explícito
que contiene una región `{#if}`/`{#for}` **estática** ahora ADOPTA la región del
DOM server-pintado en vez de abortar con el EmitError "naive-region adopt not
supported". Desbloquea tabs/steppers/accordions compuestos sin un `<input>` vivo
(que hoy no tenían workaround limpio — keep-node exige `@input`).

- Como el `render()` naive tira-y-reconstruye el root entero en cada cambio de
  state, no hay patch in-place que preservar (a diferencia de keep-node): el
  adopt solo **avanza el cursor** más allá de los anchors server-pintados
  `<!--fr-->`/`<!--/fr-->` (nuevo `emit_naive_region_skip`, handle-less, reusa el
  helper `__flv_next_comment` que ya existía), dejando el contenido del primer
  paint en su lugar hasta el primer re-render (que lo reconstruye del state). La
  mitad SSR ya emitía los anchors dentro de un `<div __fitz-child>` hidratable.
- Gate del helper corregido: `any_component_hydratable_with_regions` pasa de
  `component_uses_keep_regions` (exige value-input + sin composición) a
  `component_has_regions` (cualquier componente hidratable con región), así el
  `__flv_next_comment` que el skip referencia se emite también en el caso naive.
- **Conservador**: `tree_auto_hydratable` NO cambia — un árbol composición+región
  SIN marcador sigue fresh-mount (cero cambio byte-compat). Solo el marcador
  `hydrate` explícito habilita el adopt de región.
- **Queda como límite** (deuda 11.12 ítem 3): `<Child />` **dinámico** dentro de
  `{#for}` (reconciliación keyed) sigue rechazado con error claro — es un slice
  grande de bajo valor (choca con el modelo naive de wipe-and-rebuild).
- Validado en Chrome real 7/7 (boot · state restaurado · Badge compuesto adoptado
  · región `{#for}` adoptada del server · naive re-render reconstruye la región ·
  sin errores). Test `phase_11_12_marked_composition_with_region_adopts_via_skip_v0_41_4`.

Sin sintaxis nueva; sin impacto en grammar/LSP (cambio interno del emisor WASM).

## [v0.41.3] — 2026-08-16 — `.fitzv`: composición cross-file `<Child />` en el target SSR

Cierra un gap dual-target de los componentes single-file `.fitzv`: la composición
`<Child />` de un componente declarado en **otro** archivo `.fitzv` funcionaba en
el target **client-WASM** (`fitz build --target wasm-client`, feature CW.8) pero
**no en el target SSR** cuando se compila a través del loader clásico de módulos.
Un programa que hacía `from App import App_render` sobre un `App.fitzv` que
componía `<Badge/>` (importado de otro `.fitzv`) fallaba con `unknown component
<Badge />` — el emisor SSR compilaba el `.fitzv` en aislamiento. Ahora resuelve.

- **La maquinaria ya existía** (checker `check_with_imported_components`, emit de
  `ChildComponent` que llama `<Child>_render(...)`, `merge_imported_components`,
  loaders CW.8 dep-aware); el hueco era puro **threading**: `transform_fitzv_source`
  (el único path del loader clásico que quedó en la variante "aislada") no cargaba
  los `<Child />` importados. Nueva `transform_fitzv_source_with_deps(source, path,
  base_dir, dep_registry)` (molde de `check_view_source`) camina el grafo de imports
  del `.fitzv` (dep-aware) y threadea las surfaces al checker + al emisor SSR nuevo
  `emit_module_ssr_with_components`. El wrapper legacy `transform_fitzv_source`
  resuelve sibling-only (registro vacío). Los 2 call sites que importan (evaluator
  `fitz run`, codegen `fitz build`) pasan su `dep_registry` — así resuelven imports
  punteados por dependencia (`from fitz_liveviews.ui.Badge import badge as Badge`).
- **Imports transitivos del child**: `emit_module_ssr_with_components` une los
  imports transitivos (el `from fitz_liveviews import flv` que usa la companion
  `Badge`, o nominales/fns del child) al módulo merged, podando (a) los imports que
  resuelven a un componente merged (ahora inlineado → evita redeclararlo) y (b)
  `Html`/`html` que el header ya provee (evita doble-import).
- **Byte-compat**: un `.fitzv` sin composición cross-file emite SSR byte-idéntico
  (registro vacío → short-circuit sin clone). Guard con test dedicado; los 734 tests
  de la suite `view` + la gallery `@test` de fitz-liveviews (227) + el Admin ABM
  chequean sin cambios.
- **Tests**: 3 unit (`transform_fitzv_source_with_deps_resolves_sibling_child`,
  `..._unknown_cross_file_child_still_errors`,
  `emit_module_ssr_with_components_empty_is_byte_identical`) + 1 E2E
  (`cross_file_child_composition_ssr_via_classic_loader_v0_41_3`).

Sin sintaxis nueva; sin impacto en grammar/LSP (el `fitz check` de `.fitzv` ya
resolvía cross-file desde v0.39.0 — este cierre alinea `run`/`build` con `check`).
Destraba SSR-componer una companion via `<Child/>` tags (antes solo vía render-fns).

## [v0.41.2] — 2026-08-15 — `jwt.decode` heterogéneo en `fitz build` (cierra la paridad JWT)

Cierra la deuda residual de v0.41.0: `jwt.decode` en `fitz build` devolvía
`Result<Map<Str, Str>>` (stringificaba los claims) mientras el intérprete ya
devolvía heterogéneo. Ahora el binario también devuelve `Result<Map<Str, Any>>`
— los claims vuelven con su tipo JSON nativo (`exp`/`level` → Int, `admin` →
Bool, `roles` → List), **paridad bit-a-bit `fitz run` ↔ `fitz build`** y
round-trip correcto con el `jwt.encode` heterogéneo de v0.41.0.

- **Codegen**: nuevo helper `__fitz_jwt_decode_fv` (gated `uses_fitz_value`)
  que marshaliza cada claim vía `__fitz_json_to_fv` (el mismo conversor que ya
  usaba encode). `gen_auth_jwt_decode` tipa el retorno como
  `Result<Map<Str, Any>>` y dispara el flag `uses_fitz_value`. El detector
  `program_uses_fitz_value` gana una rama `jwt.decode` (cualquier decode
  emite el enum `__FitzValue` + los conversores). El helper viejo Str→Str
  `__fitz_jwt_decode` se elimina — quedaba 100% muerto.
- **Cross-module**: un módulo que extrae claims (`claims["email"]` sobre el
  `Map<Str, Any>`) referencia los extractores `__fv_to_*` + el helper
  `__fitz_jwt_decode_fv`, que viven solo en el preludio del crate root. Se
  extendió el auto-inject por contenido (el mismo que ya inyecta
  `use crate::__FitzValue`) para importarlos. `examples/guide/multi-module-auth`
  compila con los claims extraídos dentro del módulo.
- **Sin cambio del intérprete ni del checker** — el módulo `jwt` ya era
  `Type::Any` (el retorno de decode no se tipaba estáticamente), así que
  ningún programa existente cambia su type-checking.
- **Tests**: unit `codegen::jwt_decode_heterogeneous_uses_fitz_value_path_v0_41_2`
  + E2E `compile_e2e::auth_codegen_jwt_decode_heterogeneous_roundtrip_v0_41_2`
  (encode heterogéneo → decode → aritmética Int sobre claims + Bool, imposible
  si decode aplastara a Str). Los 4 ejemplos con `jwt.decode` (`28-auth`,
  `29-ws`, `multi-module-auth`, taskhub `c3`) compilan a binario.

Sin sintaxis nueva; sin impacto en grammar/LSP (bug interno del codegen).

## [v0.41.1] — 2026-08-15 — LSP: dedup del view-check + cache del project-scan

Dos refinamientos internos del LSP (feature `lsp`) — los dos diferidos que
quedaron de la tanda de v0.41.0. **Sin cambio user-facing del lenguaje ni de
los diagnósticos de tipos.**

- **Dedup del view-check** — `lsp.rs::check_view_source_with_base_dir`
  duplicaba el pipeline parse → expand → check que el CLI
  (`view::check_view_source`, de gotcha #7) ya tenía. Ahora delega: con
  `base_dir` usa el pipeline dep-aware (registro vacío — el LSP no puede
  resolver deps de `fitz.toml` por keystroke: clonaría git deps + haría I/O);
  un documento suelto usa `check_view_source_plain` (nuevo, rama sin
  composición cross-file). El parse/expand se comparte vía el helper
  `parse_and_expand` en `view/mod.rs` (una sola fuente de verdad; `fitz check`
  byte-idéntico). Los mensajes de parse/expand del LSP ganan el prefijo "view
  parse error:" (consistente con el CLI).
- **Cache del project-scan** — `pre_scan_project_middleware_fns_lsp` (que
  camina el árbol del proyecto por keystroke, para el falso positivo
  cross-module de `@middleware`) gana un cache por-archivo keyed por modtime,
  persistido entre llamadas. El walk (readdir) sigue corriendo, pero el read +
  tokenize + parse se saltea para archivos cuyo modtime no cambió. Un write
  bumpea el modtime → re-parse. Documentos grandes dejan de re-parsear todo el
  árbol en cada tecla.

Tests: 13 lib lsp verdes (view + cross-module + `project_scan_cache_returns_
same_result_across_calls` nuevo) + lsp_e2e. clippy `--features lsp` `-D
warnings` limpio (type alias `ProjectMwCache` para el `type_complexity`).
Benchmarks ORM vs SQLAlchemy (el 3er diferido) queda aparte — necesita Docker.
Bump `Cargo.toml` + `editors/vscode/package.json` 0.41.0 → 0.41.1.

## [v0.41.0] — 2026-08-15 — `jwt.encode` heterogéneo + middleware `async` run↔build + docs anchors

Tres cierres coordinados: un feature (payloads JWT heterogéneos en `fitz
build`), un bug de paridad `fitz run` ↔ `fitz build` (middleware `async`),
y una barrida de doc-rot (cross-refs de anchor rotos).

- **`jwt.encode` con payload heterogéneo (`Map<Str, Any>`)** — antes el
  codegen exigía `Map<Str, Str>` estricto y aplastaba todo a strings. Ahora
  un payload con valores no-`Str` (números, booleanos, listas) se serializa
  con su **tipo JSON nativo** (`exp: 1699999999` queda numérico, `admin:
  true` booleano, `roles: [...]` array), con **paridad bit-a-bit `fitz run`
  ↔ `fitz build`** (el intérprete ya era heterogéneo vía `value_to_json`).
  Un payload `Map<Str, Str>` mantiene el **fast-path Str→Str byte-idéntico**
  (`__fitz_jwt_encode`); el hetero marshaliza vía `__FitzValue`
  (`__fitz_jwt_encode_fv` + los conversores `__fitz_fv_to_json` emitidos en
  el preludio para el caso sin-db). `smtp.send` queda **fuera de alcance**
  (no es un payload libre: schema fijo con campos inherentemente `Str`).
  **Residual**: `jwt.decode` en `fitz build` todavía devuelve `Map<Str,
  Str>` (stringifica claims no-Str) — el intérprete ya lo devuelve
  heterogéneo, cerrar la asimetría del codegen es el follow-up.
- **Middleware `async fn` con paridad `fitz run` ↔ `fitz build`** — un
  `@middleware(fn)` `async` devolvía **500** en el intérprete (el evaluator
  no awaiteaba el `Value::Future` del middleware antes de inspeccionar el
  resultado). Ahora el chain de middlewares (Pre/Post/Wrap) awaitea primero
  y matchea `Null`/`HttpResponse`/short-circuit igual que un middleware sync,
  igual que ya hacía `fitz build`. 2 tests nuevos (`handle_task_async_
  middleware_returning_null_continues` + `_short_circuits`).
- **Docs — 67 cross-refs de anchor rotos** en `docs/guide.md` (TOC + cross-
  links), `docs/db-orm.md`, 11 caps del curso, `taskhub`, `index.md`,
  `syntax-spec.md`. Venían de renumeraciones de capítulos + acentos/em-dash
  en los slugs (mkdocs elimina acentos y colapsa el em-dash a un guión) +
  anchors URL-encoded. `mkdocs build --strict` queda sin anchor-links
  internos rotos (los ~92 warnings restantes son links `../` a archivos del
  repo — CHANGELOG/examples/src — que funcionan en GitHub, no bloquean el CI
  non-strict).

Tests: 4 unit nuevos (2 codegen `jwt_encode_*_v0_41_0` + 2 http async-mw) +
2 E2E (`compile_e2e::auth_codegen_jwt_encode_heterogeneous_payload_v0_41_0`
decodifica el JWT y aserta el tipo nativo de cada claim; `_str_str_fast_path`
byte-compat). Verificación: lib 4119 (default) / lsp, smoke ~290 ejemplos
verde (cero regresión en 28-auth pese al detector nuevo que toca todo
`jwt.encode`), cli_e2e, fmt + clippy default+lsp limpios. Bump `Cargo.toml`
+ `editors/vscode/package.json` 0.40.1 → 0.41.0.

## [v0.40.1] — 2026-08-15 — Cierre de la clase B16: `if` divergent-else + literales mixtos

Cierra los **dos últimos residuales** de la clase de bugs checker↔codegen que
abrió B16 ("pasa `fitz check`, rompe `fitz build` con un `E0308` opaco"). Con
esto **no quedan ítems abiertos conocidos de la clase**.

- **R-if-div — `if c { A } else { return B }` usado como valor**. El codegen
  trataba el `if` con rama divergente como statement-mode (`Null`), aunque Rust
  lo tipa como `A` (el `else` es `!`). Dos cambios coordinados: (1) `gen_if_expr`
  gana una rama `asym_value` (+ helper `block_diverges`) que emite expression-mode
  con el tipo de la rama-valor cuando la otra diverge (`return`/`break`); la rama
  `want_value` simétrica queda byte-idéntica. (2) El checker `Expr::If` replica.
  Ahora `let n: Int = if c { A } else { return B }` chequea + buildea + corre.
- **R-list-mix — literales heterogéneos vs anotación**. `Expr::List`/`Expr::Map`
  del checker computan el LUB de los elementos con un **sticky Any** (helper
  `sticky_lub`), replicando `codegen::gen_list_lit`: `[1, 2.5]` → `List<Float>`
  (antes `List<Any>`), `[1, "dos", 2]` → `List<Any>` sticky. Ahora `let xs:
  List<Int> = [1, 2.5]` errora en `fitz check` con mensaje claro en vez del E0308
  de rustc al buildear.

Ambos son la misma clase que `match` (v0.39.1) e `if`-como-expresión (v0.39.3).
5 unit tests (`list_literal_*`/`map_literal_*`/`ifexpr_divergent_else_*`) + 2 E2E
(`compile_e2e::v040_1_*` — divergent-else builds+runs, `[1,2.5]` builds).
Verificación: lib 4115 (default) / 4279 (lsp), smoke ~290 ejemplos verde, cli_e2e
128, fmt + clippy default+lsp limpios. Bump `Cargo.toml` +
`editors/vscode/package.json` 0.40.0 → 0.40.1.

## [v0.40.0] — 2026-08-15 — DX chico: `:load` en Windows (L3) + error de `'...'` string (L4) + `fitz check` barre todo `.fitzv` (a2)

Tres cierres chicos del backlog (L3, L4 del curso + a2 fast-follow de gotcha #7).
**Incluye también el audit de `if`-como-expresión (v0.39.3)**, que se commiteó pero
nunca se tageó — sale como parte de este release.

- **L3 — `:load` con path Unix-absoluto en Windows** (`src/main.rs`): en el REPL,
  `:load /tmp/x.fitz` en Windows resuelve contra el drive actual (`D:\tmp\x.fitz`)
  y casi nunca existe (comportamiento estándar de la API de Windows, no un bug de
  Fitz). Ahora, ante el read-error de un path que empieza con `/`, se emite un
  hint (`nota:`) apuntando a `D:/...` para absolutos o a un path relativo para
  portabilidad. Windows-only (`#[cfg(windows)]`), no cambia nada en Linux/macOS.
- **L4 — mensaje de error de `'...'` como string** (`src/lexer.rs`): `'` está
  reservado para labels de loop (`'outer: loop { break 'outer }`); Fitz no tiene
  `'...'` strings. Antes, `print('comillas')` daba un críptico "expected an
  identifier after `'`". Ahora el error explica que las strings usan comillas
  dobles y el `'` solo abre un loop label. Sin cambio de comportamiento (solo el
  texto del error; la tokenización válida es idéntica).
- **a2 — `fitz check` sin args barre todo `.fitzv` de `src/`** (`src/main.rs`):
  antes chequeaba solo el `[bin].main`. Ahora, sin file arg en un proyecto con
  manifest, además view-checkea todo `.fitzv` bajo `src/` (un componente que no
  es el entry igual se valida — útil para CI + librerías de componentes). Los
  `.fitz` clásicos NO se barren (el checker no resuelve imports cross-module → daría
  ruido de "unknown import"); los `.fitzv` son self-contained (sus imports resuelven
  dep-aware). `check_file`/`check_view_file` pasan a devolver `bool` para agregar el
  exit code; un file arg explícito mantiene el comportamiento single-file.

3 unit tests (`lexer::tests::l4_*`) + 1 E2E (`cli_e2e::a2_check_no_arg_sweeps_non_entry_fitzv`).
Verificación: lib 4112 (default) / lsp, cli_e2e 128, fmt + clippy default+lsp
limpios (el smoke `GUIDE_EXAMPLES_COMPILE` no aplica — ninguno toca el codegen).
Bump `Cargo.toml` + `editors/vscode/package.json` 0.39.3 → 0.40.0. **Ambos L3 y L4
eran "by design" en su deuda** (docs ya corregidas); el cierre acá es la mejora de
DX (hint/mensaje claro), no un cambio de semántica.

## [v0.39.3] — 2026-08-15 — Auditoría de consistencia checker↔codegen: `if`-como-expresión

Continúa la auditoría abierta por B16 (la clase de bugs "pasa `fitz check` pero
rompe `fitz build` con un `E0308` opaco"). Confirmado y cerrado el hermano más
común: **`if`-como-expresión**. El checker devolvía `Type::Any` para todo `if`,
mientras el codegen computa el LUB de las ramas. Así `let n = if (b) { 1 } else
{ print("x") }` (o `{ 2.5 }`) en una fn `-> Int` pasaba `fitz check` (n=`Any`)
pero rompía `fitz build` (codegen: n=`Int?` / `Float`).

**Fix** (`src/types.rs`, `Expr::Match` helper compartido `check_block_tail_type`
+ `Expr::If`): el checker tipa el `if` como el **LUB de sus ramas** cuando es una
expresión-valor (`want_value`: hay `else` Y ambas ramas terminan en una
expresión), replicando exactamente el `gen_if_expr` del codegen. El caso
non-`want_value` (sin `else`, o una rama que no termina en expresión, incluido el
idiom divergente `if c { A } else { return B }`) mantiene su comportamiento
gradual (`Any`) para no introducir regresiones. Ahora el mismatch sale en
`fitz check` con mensaje claro.

4 unit tests `ifexpr_*`. Verificación full: lib 4110 (default) / 4274 (lsp),
smoke ~290 ejemplos verde, cli_e2e 127, fmt + clippy default+lsp limpios. Bump
`Cargo.toml` + `editors/vscode/package.json` 0.39.2 → 0.39.3.

**Residuales de la misma clase** (más raros, requieren cambio de codegen además
del checker — documentados en `docs/deudas-post-5b.md`): (a) `if c { A } else {
return B }` usado como valor — el codegen trackea el statement-mode `if` como
`Null` aunque Rust lo tipa `A`; (b) literales `[...]`/`{...}` heterogéneos
(`[1, 2.5]`) — el checker toma "primer elemento o Any" vs el LUB del codegen.

## [v0.39.2] — 2026-08-14 — Bugfix B16 (residual): `print`/`assert` en match arms

Cierra la deuda residual derivada de B16 (v0.39.1). Los builtins variádicos
`print(...)` y `assert(...)` se tipaban `Type::Any` en el checker (su aridad no
se expresa como `Type::Function`) pero devuelven `Null` en runtime/codegen — el
mismo split checker↔codegen que B16 cerró para `log.X`. Así, `match r { Ok(v) =>
v, Err(_) => print(...) }` en una fn `-> Int` pasaba `fitz check` (match=`Int`)
pero rompía `fitz build` con un `E0308` opaco.

**Fix** (`src/types.rs`, arm de Call con callee `Ident`): `print`/`assert` se
tipan `Null` (respetando shadowing, mismo patrón que `spawn`/`log`). Ahora el
match promueve a `T?` y el mismatch sale en `fitz check` con mensaje claro. Los
otros assertion builtins (`assert_eq`/`assert_ne`/`assert_throws`) ya tenían
`ret: Null` — sin cambio. Uso normal como statement (`print("hola {x}")`)
intacto (nadie usa el retorno de `print`/`assert`).

3 unit tests `b16_print_*` (+9 total del grupo B16). Verificación full: lib 4106
(default) / 4270 (lsp), smoke ~290 ejemplos verde, cli_e2e 127, boilerplates
check, fmt + clippy default+lsp limpios. Bump `Cargo.toml` +
`editors/vscode/package.json` 0.39.1 → 0.39.2.

## [v0.39.1] — 2026-08-14 — Bugfix B16: consistencia checker↔codegen en el tipo de un `match`

Cierra la deuda **B16**. La causa raíz era una inconsistencia: el **checker
tipaba un `match` como el tipo del PRIMER arm**, mientras el **codegen computa
el LUB de los arms**. Con B7 (que ya envuelve en `Option` los arms `Null`) el
síntoma original (`i64` vs `()`) mutó a un nuevo mismatch — `match r { Ok(v) =>
v, Err(e) => log.error(...) }` en una fn `-> Int` pasaba `fitz check` (match =
`Int`) pero rompía `fitz build` con un `E0308` opaco de rustc (`expected i64,
found Option<i64>`, match = `Int?`).

**Tres cambios coordinados en el checker (`src/types.rs`)**: (1) un `match`
tipa como el **LUB de todos sus arms**, no el primero — los arms divergentes
(`return`/`break`/`continue`/`return <status> {...}`) se tipan `Any`, que el
LUB cede al concreto, así el patrón canónico `Ok(r) => r, Err(_) => return`
sigue tipando como `r` (sin regresión); (2) `log.info/warn/error/debug(...)` se
tipa `Null` (su retorno real) en vez de `Any`; (3) `return <status> { ... }` (el
short-circuit HTTP, `Stmt::ReturnStatus`) se trata como divergente, igual que el
codegen — antes era `Null`, latente hasta que el match empezó a LUBear. Más un
fix del `lub` (checker + codegen): `lub(T?, Null)` devolvía `T??` en vez de `T?`
(idempotencia — `Null` está subsumido por `T?`).

Ahora el mismatch sale en **`fitz check` con mensaje claro** (`` `return`
returns `Int?` but the function declares `Int` ``) en vez del `E0308` de rustc
al buildear; el usuario lo arregla con un sentinel (`{ log.error(...); 0 }`) o
anotando `-> Int?`. 6 unit tests `types::tests::b16_*`. Verificación full: lib
4103 (default) / 4267 (lsp), smoke ~290 ejemplos verde, 10 boilerplates check,
cli_e2e 127, fmt + clippy default+lsp limpios. Bump `Cargo.toml` +
`editors/vscode/package.json` 0.39.0 → 0.39.1.

## [v0.39.0] — 2026-08-14 — `fitz check` view-parsea `.fitzv` (gotcha #7)

Cierra el gotcha #7 —el último ítem del catálogo del DSL `.fitzv`. Cuando el
entry (el `[bin].main` del manifest, o `fitz check App.fitzv` explícito) es un
`.fitzv`, `fitz check` corre el pipeline de view (parse → expand → type-check)
en vez del lexer clásico. Antes un `.fitzv` explotaba con un error de lexer
clásico y los errores de view (parse + tipos) solo aparecían en `run`/`build`.

La composición cross-file `<Child />` se resuelve **dep-aware** (mismos loaders
transitivos + `DepRegistry` que `fitz build --target wasm-client`), así que un
resultado de `check` predice el de `build` (`from fitz_liveviews.ui.Badge import
Badge` resuelve por la dependencia, no solo por sibling). Contrato de exit code
idéntico al clásico: `✓ … — no type errors` (exit 0) / `✗ … — N view error(s):`
(exit 1). Alcance MVP: solo el entry (los imports transitivos SÍ se validan);
descubrir TODO `.fitzv` bajo `src/` queda como fast-follow.

Interno: helper NO-gateado `view::check_view_source(source, base_dir,
dep_registry) -> Vec<CheckError>` + constructor `CheckError::syntax(...)` que
pliega parse/expand en el mismo `Vec`; dispatch por extensión en el arm `Check`
de `src/main.rs` (`check_view_file` sibling de `check_file`). Sin cambios al
path clásico `.fitz` ni a los emitters de view (byte-compat: los ~25 ejemplos
`examples/view/` regeneran idénticos). **Cierra el catálogo de gotchas del
`.fitzv` entero** (#1 v0.37.17, #6 v0.38.0, #7 v0.39.0). Verificación: lib 4097
(default, +3) / 4261 (lsp); cli_e2e 127 (+4 `gotcha7_*`); fmt + clippy
`-D warnings` default+lsp limpios; view smoke 25/25 + byte-compat. Bump
`Cargo.toml` + `editors/vscode/package.json` 0.38.0 → 0.39.0.

## [v0.38.0] — 2026-08-13 — `.fitzv`: atributos booleanos condicionales (`checked={expr}`, gotcha #6)

Cierra el gotcha #6 del DSL `.fitzv` (Form B). Un atributo booleano condicional
—`checked={expr}` / `disabled={expr}` con llave SIN comillas— está presente en
el DOM **sii `expr` es truthy** (el modelo de boolean-attribute de HTML).
Distinto del `checked="{expr}"` CON comillas (siempre presente, valor
stringificado). Antes el único camino era emitir las dos variantes con
`{#if checked}<input checked/>{#else}<input/>{/if}`. Sin impacto en el codegen
`.fitz` clásico ni en el LSP; el grammar del `.fitzv` vive en la extensión de
fitz-liveviews.

### Added

- **`attr={boolExpr}` — atributo booleano condicional en `.fitzv`.** Llave sin
  comillas tras `=` → presente-sii-truthy. Cubre `checked`/`disabled`/
  `selected`/`readonly`/`required`/… sin whitelist: la sintaxis + el requisito
  `Bool` en el checker son el gate. Los eventos (`@click=…`) siguen exigiendo
  comillas. Full-stack: **parser** (`Attr::BoolInterpolation`, brace-balanced),
  **expand** (`ExpandedAttr::BoolInterpolation`), **checker** (el expr debe ser
  `Bool`, misma regla que un `{#if}`; ve el scope de un `{#for}` envolvente),
  **SSR** (baja a una if-expresión Fitz `if (cond) { "checked" } else { "" }`),
  **WASM** (build con `set_attribute`, y en un componente keep-node el patch
  reactivo togglea `set_attribute` / `remove_attribute` in-place — único
  primitivo web-sys nuevo). Rechaza el bool attr sobre un `<Child />` (prop
  dinámico, zona de reactividad sin terminar) y sobre un `<slot>` con mensajes
  claros. Byte-compat: los `.fitzv` sin bool attrs emiten idéntico (los ~24
  `examples/view/` regeneran byte-a-byte). Ejemplo nuevo `examples/view/
  bool-attr/` (keep-node, compila a WASM real) + smoke. 15 tests nuevos
  (parser/expand/check/SSR/WASM). Bump 0.37.17 → 0.38.0.

## [v0.37.17] — 2026-08-13 — `.fitzv`: comillas dobles anidadas en valores de atributo

Cierra el gotcha #1 del DSL `.fitzv` (item P4 de la auditoría, máximo DX de
autoría). Sin sintaxis nueva; sin impacto en LSP/grammar (el template es
opaco al lexer de `.fitzv`).

### Fixed

- **`placeholder="{t(locale, "dep.ph")}"` — comillas dobles anidadas en el
  VALOR de un atributo — ahora parsea.** El `"` interno (parte de un string
  literal Fitz dentro de la interpolación `{...}`) se interpretaba como el
  `"` de cierre del atributo → "expected attribute name". El view parser
  (`read_attr_value`) ahora trackea la profundidad de `{...}` + un flag de
  string anidado: solo un `"` a brace-depth 0 cierra el atributo; un `"`
  dentro de una interpolación abre/cierra un string anidado y se preserva
  verbatim (el parser clásico de Fitz lo re-lexea downstream vía
  `parse_expr_at`). **Borra la categoría de helpers `ph_*`/`tip_*`** que
  fitz-liveviews necesitaba para meter i18n (`t(locale, "key")`) en atributos
  — ahora se escribe inline. Byte-compatible: un valor sin `"` anidado parsea
  idéntico (el brace-depth solo importa cuando aparece un `"` anidado). Un `{`
  sin cerrar en un valor ahora se rechaza en PARSE (antes en expand) con
  "unmatched `{`" — más temprano y con mensaje claro. Fix contenido en una
  sola fn (`src/view/parser.rs`), sin tocar el emisor SSR/WASM (consumen
  `ExpandedAttr::Interpolation`/`MixedInterpolation` sin cambios). Tests:
  parser (nested full-interp / mixed / static byte-compat / unmatched),
  expand (→ `Expr::Call` parseado), SSR emit (i18n en atributo emite la
  llamada verbatim).

### Diferido

- **Gotcha #6** (`{expr}` bare en posición de atributo / atributos booleanos
  condicionales `checked`/`disabled`) — requiere una variante de atributo
  nueva + reactividad WASM (`setAttribute`/`removeAttribute` en el patch); el
  workaround `{#if checked}<input checked/>{#else}<input/>{/if}` es barato y
  no genera sprawl de helpers. Y el edge `}` literal dentro de un string en
  la interpolación (`{f("}")}`) sigue mal-contado por `split_mixed_attr_value`
  / `extract_full_interp` (niche, sin uso real).

## [v0.37.16] — 2026-08-13 — Migraciones: red de seguridad para renames de columna sin anotar

Cierra el ítem P1 de la auditoría (seguridad de datos). El path seguro
para renombrar columnas ya existía (`@renamed_from("old")` → `ALTER TABLE
... RENAME COLUMN`, v0.10.17), pero un rename **sin** la anotación se
emite como `DROP COLUMN old` + `ADD COLUMN new`, que pierde los datos de
la columna en silencio. Ahora `fitz db diff` lo detecta y avisa
específicamente. Sin cambios de lenguaje; sin impacto en LSP/grammar.

### Added

- **`fitz db diff` avisa de renames de columna probables.** Cuando el diff
  contiene un `DROP COLUMN` + un `ADD COLUMN` del **mismo tipo SQL** en la
  misma tabla, emite (en TODOS los paths, no solo `--check-destructive`) un
  `⚠ possible column rename(s)` que nombra el par exacto (`old` → `new`,
  tipo) y apunta a `@renamed_from("old")` para un `ALTER TABLE ... RENAME
  COLUMN` que preserva los datos. Es un warning **no bloqueante** (un
  drop+add genuino del mismo tipo es un falso positivo aceptable — el aviso
  se puede ignorar). Nunca auto-renombra: un name-based diff no puede
  distinguir un rename de un drop+add legítimo, así que la señal se
  superficializa para que el usuario decida. Nueva fn pública
  `migrations::detect_probable_renames(changes, current) -> Vec<ProbableRename>`
  (empareja cada columna dropeada con una agregada del mismo `sql_type`,
  cada una en a lo sumo un par); wired en `db_diff_cmd`. Con `@renamed_from`
  presente el diff emite `RenameColumn` (no drop+add) → cero falsos
  positivos. Validado end-to-end contra Postgres real (rename sin anotar →
  ⚠ + DROP/ADD; con `@renamed_from` → RENAME COLUMN sin warning). 4 unit
  tests `migrations::tests::detect_probable_rename_*`.

## [v0.37.15] — 2026-08-13 — Codegen: `for kv in m` sobre Map en `fitz build`

Cierra un gap de **paridad `fitz run` ↔ `fitz build`** encontrado en la
auditoría de deuda: iterar un Map con un solo Ident (`for kv in m`) corría
en el intérprete y pasaba `fitz check`, pero `fitz build` lo rechazaba.
Sin sintaxis nueva; sin impacto en LSP/grammar.

### Fixed

- **`for kv in m` (single Ident sobre un `Map<K, V>`) compila en
  `fitz build`.** Antes el codegen del `for` solo aceptaba el tuple pattern
  `for (k, v) in m` o `for _ in m`, y abortaba con "exige un tuple pattern
  de 2 elementos" ante `for kv in m` — mientras el intérprete lo aceptaba
  (bindeando `kv` a un `Value::Tuple`) y el checker lo tipaba
  (`Type::Tuple([K, V])`, accedido `kv.0`/`kv.1`). Ahora el codegen emite
  `for mut kv in __for_snap.into_iter()` bindeando el par completo como un
  tuple Rust `(K, V)` (iterar el `Vec<(K, V)>` interno del Map lo da
  directo), y `kv.0`/`kv.1` bajan al `TupleField` Rust. Paridad bit-a-bit
  validada (`fitz run` ↔ binario producen el mismo output, orden de
  inserción preservado). Tests `codegen::tests::
  v0_37_15_for_kv_in_map_binds_whole_pair_tuple` +
  `compile_e2e::for_kv_in_map_parity_run_build_v0_37_15`.

### Docs

- `fmt.rs` — doc-header stale corregido (la "CRITICAL LIMITATION de
  9.z.1.a: comments+blank lines se borran" ya no aplica; 9.z.1.b los
  preserva vía `Trivia`). Nota del `is_let` actualizada (el AST ya tiene el
  campo desde v0.28.0; el fmt aún lo lee por el hack de `Span`).
- `deudas-post-5b.md` — nota A.10 (`FITZ_DB_*` mid-run reload) marcada
  CERRADA en v0.37.12 (era "pendiente"); residual menor `HTTP_LOG_MODE`
  anotado.

## [v0.37.14] — 2026-08-13 — Codegen: state compartido de módulo con primitivo `let X` en handlers

Cierra un bug de **paridad `fitz run` ↔ `fitz build`** del codegen de
módulos, descubierto al hacer `fitz build` del showcase Admin ABM de
fitz-liveviews (un `let PAGE_SIZE: Int = 8` de módulo usado por handlers
`@ws`). Sin sintaxis nueva; sin impacto en LSP/grammar; extensión VSCode
sin cambios (bug interno del codegen).

### Fixed

- **Un `let X` de un MÓDULO importado, referenciado por handlers HTTP/WS
  de ese módulo, con RHS primitivo (`Int`/`Float`/`Bool`/`Str`), rompía
  `fitz build`.** El caso es *shared state*: el handler materializa
  `let X = (*__FITZ_STATE_X).clone()` al inicio de su cuerpo, pero
  `gen_module_top_let` emitía el binding como un bare `pub const X`
  (Paths 1a/1b) y retornaba temprano — nunca emitía el static
  `__FITZ_STATE_X` → `E0425 cannot find value __FITZ_STATE_X` +
  `E0530 let bindings cannot shadow constants`. El caso MAIN ya
  funcionaba (`gen_http_main` emite el `LazyLock` para **todo** state
  var, primitivos incluidos); solo el ctx de MÓDULO no corría esa
  maquinaria para primitivos (los contenedores `List`/`Map`/`Nominal` y
  los async `db.connect(...).await` sí desde v0.28.3 / v0.37.6). El
  intérprete (`fitz run`) nunca lo sufrió (captura el env del módulo).
  **Fix** (~50 LoC netas, solo `src/codegen.rs`): `gen_module_top_let`
  computa `is_shared_state` al tope y gatea Paths 1a (Str) y 1b (const
  primitivo) en `!is_shared_state`, así un primitivo shared-state cae a
  una rama nueva que emite `static __FITZ_STATE_X: LazyLock<T>` +
  accessor `pub fn X()` (mirror del main). Los primitivos shared-state se
  registran en `accessor_consts` tras `resolve_state_var_types` para que
  una fn NO-handler del módulo que los referencie bare emita `X()` (las
  fns handler siguen shadoweando con el local materializado). Paridad
  bit-a-bit validada con curl. Test E2E
  `module_shared_state_primitive_const_compiles_v0_37_14`.

## [v0.37.13] — 2026-08-12 — CLI `@arg` / `@flag` (decoradores de parámetro, estilo Click)

Cierra el ítem #6 (el último) del inventario de "deudas residuales
chicas". Los parámetros de un `@command` aceptan decoradores explícitos
para texto de ayuda + short flags elegidos a mano. Fase 13 (CLI builder)
tenía esto diferido como "deuda menor si aparece presión". Sin sintaxis
nueva del lexer; `Param` gana un vector de decoradores.

### Added

- **`@arg(help="...")` y `@flag(short="x", help="...")` sobre parámetros
  de un `@command`.** Overrides opt-in de la convención auto (param sin
  default = positional, con default = flag): `@arg` da texto de ayuda a
  un positional; `@flag` elige la letra del short flag + texto de ayuda.
  Los short flags explícitos son **case-sensitive** (`-V` distinto de un
  auto `-v`, estilo Click) y ganan sobre la primera-letra auto; una
  colisión (dos explícitos, o explícito vs auto) es error de
  compilación. `@flag` es **position-aware**: sobre una fn es el gate de
  feature flags (12.8), sobre un parámetro es el override CLI —
  posiciones disjuntas, no colisionan. Paridad bit-a-bit `fitz run` ↔
  `fitz build` (help + parseo del short). Toca 7 capas:
  `Param.decorators` (AST), `parse_params`, el checker
  (`check_cli_param_decorators`, 10 reglas de validación), `cli.rs`
  (render de help/short + `parse_argv` case-sensitive), el codegen
  (mirror bit-a-bit de los string-templates), `fmt` y el LSP.

### Fixed

- **`fitz fmt` dropeaba el valor por default (y el `...` de varargs) de
  un parámetro** — bug pre-existente que #6 sacó a la luz (los flags CLI
  SON params con default): `fn f(x: Int = 5)` se formateaba como
  `fn f(x: Int)`, convirtiendo un flag en positional. `fmt_param_to_string`
  ahora emite el `= <default>`, el prefijo `...` variadic y los
  decoradores de parámetro.

### Tests

- 7 unit `cli::tests::*_v0_37_13` (help/short accessors + render +
  3 colisiones) + 9 unit `types::tests::cli_*_v0_37_13` (las 10 reglas)
  + 1 parser `param_decorators_parse_into_param_v0_37_13` + 1 E2E
  `compile_e2e::fase_13_cli_arg_flag_decorators_parity_v0_37_13` (help
  ARGS/OPTIONS + short `-L` con paridad run↔build).

### Docs / benchmarks

- Cap 34 del guide (CLI builder) suma la sub-sección `@arg`/`@flag`;
  `examples/guide/33-cli.fitz` suma el comando `deploy` y el boilerplate
  `cli-tool` usa `@flag(help=)` de showcase.
- Benchmarks revalidados contra v0.37.12: mixed-workload (Fitz/Python/
  Node) re-corrido (1 corrida, sin bitrot, Fitz mantiene el liderazgo);
  ORM ya republicado con mediana de 3 en v0.37.12.

## [v0.37.12] — 2026-08-11 — `FITZ_DB_*` mid-run reload + ORM `.preload()` HasOne

Dos deudas residuales chicas del inventario. Sin sintaxis nueva. #6 (CLI
`@arg`/`@flag`) se difirió tras mapearla con un Explore: Fase 13 ya la cerró
como deuda menor, `@flag` colisiona con el decorator de feature flags
(Fase 12.8), y los params no soportan decorators hoy (habría que tocar
AST + parser) — es una mini-feature con forks de diseño, no un commit chico.

### Fixed

- **`FITZ_DB_MAX_CONNS` / `FITZ_DB_LOG` reflejan cambios mid-run.** Ambos se
  leían con `LazyLock` (fijados al primer acceso, cambios posteriores
  ignorados). Ahora `effective_max_conns()` relee `FITZ_DB_MAX_CONNS` fresco
  en cada creación de pool (path frío) y `current_db_log_mode()` relee
  `FITZ_DB_LOG` fresco en cada query (network-bound, costo despreciable).
  Cierra el caveat documentado: un `db.connect(url2, max_conns=20)` emitido
  DESPUÉS de un `db.connect(url1)` previo ahora SÍ aplica el override al pool
  nuevo (el connect previo ya no fija el valor); y `FITZ_DB_LOG=verbose` vía
  `load_env` toma efecto en la siguiente query sin reiniciar. `HTTP_LOG_MODE`
  (`FITZ_HTTP_LOG`) sigue `LazyLock` (fuera de scope de #4).

### Added

- **`.preload("relation")` sobre `@has_one`.** Antes solo `@has_many` y
  BelongsToCompanion; HasOne se rechazaba citando "deuda futura". Ahora
  `User.preload("profile").all(db)` con `@has_one("Profile", via="user_id")
  profile: Profile?` carga eager en la misma dirección que HasMany (el FK vive
  en el child: `WHERE profiles.user_id IN (user_ids)`, reusa el manejo de FK
  nullable de B15) pero, por cardinalidad 1:1, asigna `Option<Profile>` (el
  primer match) en vez de una lista. Codegen-only, en paridad con HasMany/
  companion (el intérprete no soporta `.preload()` de ningún kind — feature
  build-only). **BelongsTo eager ya estaba cerrado** vía companion (v0.10.5);
  `.preload()` sobre el FK directo sigue rechazado a propósito (el companion
  es la forma correcta).

### Tests

- `db::tests::{effective_max_conns,current_db_log_mode}_reflects_midrun_env_change_v0_37_12`
  (2) + `codegen::tests::codegen_orm_preload_has_one_{emits_option_assignment,with_nullable_child_fk_emits_some_pid}_v0_37_12`
  (2) + `compile_e2e::orm_preload_has_one_compiles_to_binary_v0_37_12`
  (1 E2E build a binario nativo).

## [v0.37.11] — 2026-08-11 — `bytes_from_b64`/`bytes_from_hex` + spans de `Stmt` en interpolación

Dos deudas chicas del inventario. Sin sintaxis nueva del lenguaje más allá
de los dos builtins.

### Added

- **`bytes_from_b64(s: Str) -> Result<Bytes>` y `bytes_from_hex(s: Str) ->
  Result<Bytes>`** — decodifican un string base64/hex a `Bytes`, cerrando el
  gap de `Response { body_bytes }` (respuestas HTTP binarias: imágenes,
  PDFs). Antes solo se podían construir bytes con `bytes(s)` (copia UTF-8
  cruda). Builtins globales con paridad bit-a-bit `fitz run` ↔ `fitz build`:
  los decoders son inline idénticos en el evaluator y en el codegen (mismo
  algoritmo → mismo Ok/Err para cualquier input, mismos mensajes de error).
  Un input inválido es `Result::Err(Str)`. **Cero dep nueva** al Cargo.toml
  generado (los decoders son self-contained, no arrastran el crate
  `base64`). LSP completions + grammar TextMate actualizados.

### Fixed

- **Cuelgue del LSP al abrir un `.fitz` fuera de un proyecto (hotfix de
  v0.37.10).** El pre-scan de `@middleware` cross-module (v0.37.10) hacía,
  cuando no encontraba un `fitz.toml` ancestro, un fallback que caminaba
  `base_dir` — y para un documento suelto (URI `file:///x.fitz`, cuyo
  `base_dir` es `/` o `/tmp`) eso escaneaba **el disco entero**, colgando el
  job LSP del CI 1h+ y el LSP de cualquier usuario que abriera un `.fitz`
  suelto. Fix: sin `fitz.toml` ancestro → **no se camina nada** (el patrón
  cross-module de `@middleware` solo existe en proyectos multi-módulo, que
  siempre tienen manifest). Además el walker ya no sigue symlinks (evita
  ciclos) y corta en 2000 archivos. Tests
  `lsp::tests::v0_37_11_project_scan_{without,with}_manifest_*`.
- **Spans de `Stmt` dentro de una interpolación de string.** El walker de
  spans del parser (que corrige los spans de sub-expresiones parseadas en un
  `{...}`) no recursaba en el `Vec<Stmt>` de `FnExpr.body`/`Loop`/`If`/`Match`
  (residual de V1). Un `"r: {xs.map(fn(x) => x * 2)}"` dejaba el `x * 2` con
  spans corridos → hover/go-to-def/diagnósticos mal ubicados. Fix: nuevo
  `parser::shift_stmt_spans` + `Stmt::span_mut()` (ast.rs) + relleno de los 5
  brazos con deuda en `shift_expr_spans`. El caso real alcanzable vía
  interpolación es el arrow `fn(x) => expr` (el lexer de interpolación no
  balancea llaves anidadas, así que los brazos block-form quedan defensivos).

### Tests

- `evaluator::tests::bytes_from_*` (6: Ok/Err b64/hex, empty, odd-length) +
  `codegen::tests::codegen_bytes_*` (3: call + prelude + Cargo.toml sin
  base64) + `parser::tests::v0_37_11_*` (2: FnExpr body con columnas exactas
  + shift_stmt_spans directo sobre un If).

## [v0.37.10] — 2026-08-11 — LSP: falso positivo cross-module de `@middleware` cerrado

El LSP ya no marca un falso positivo de "return con status fuera de
contexto HTTP" sobre una fn middleware DECLARADA en un módulo cuando la
aplicación `@middleware(fn)` vive en otro módulo. Solo diagnostics del
LSP — cero impacto en runtime/build (el `fitz build` ya compilaba bien
desde v0.19.5).

### Fixed

- **Falso positivo cross-module de `@middleware` en el LSP.** Abrir el
  archivo que DECLARA una middleware fn (`rate_limit.fitz` con
  `fn mw_strict() { return 429 {...} }`) mostraba squiggle rojo "only
  allowed inside HTTP handler" cuando la aplicación (`@middleware(mw_strict)`)
  vivía en otro módulo. A diferencia de `@auth_provider`/`@background`
  (v0.19.3), no alcanza seguir las imports del documento abierto: el
  falso positivo está en el DECLARANTE, cuyas imports no llegan al módulo
  aplicante (la dependencia va al revés). El codegen lo resuelve porque
  `main` pre-escanea todo el árbol de imports; el LSP no tiene `main`.
  Fix: `pre_scan_project_middleware_fns_lsp` camina el ÁRBOL del proyecto
  acotado por el `fitz.toml` ancestro (walk-up con `manifest::find_manifest`;
  fallback a `base_dir`), extrae las refs `@middleware(name)` de cada
  `.fitz`/`.fitzv` y las alimenta al checker vía `add_imported_middleware_fns`.
  Silent fallback en read/parse; sin cache en el MVP (barato para
  proyectos típicos). Cero riesgo runtime.

### Tests

- `lsp::tests::cross_module_middleware_declared_fn_no_false_positive`
  (sin `base_dir` el falso positivo aparece; con `base_dir` desaparece).

## [v0.37.9] — 2026-08-11 — `spawn(@background(store=db))` desde un módulo en `fitz build`

Cierra la última sub-deuda de la persistencia de `@background`: un
`spawn(<bg>(...))` ubicado DENTRO de un módulo importado (no en el main)
ahora emite el path persistente en `fitz build`, con paridad bit-a-bit
ante `fitz run`. Sin sintaxis nueva; solo codegen. Aditivo — los
programas sin `@background(store=db)` compilan byte-idéntico.

### Fixed

- **`spawn(...)` persistente desde un módulo caía al path fire-and-forget
  en el binario.** El intérprete (`fitz run`) persiste un spawn desde
  cualquier lado (registry global), pero `fitz build` solo lo persistía
  cuando el `spawn(...)` vivía en el main: el ctx de codegen de cada
  módulo emitía su `.rs` con `bg_persistent_fns` vacío, así que
  `gen_spawn_call` no reconocía el target como persistente y descartaba
  la persistencia en silencio. Fix: (a) nuevo pre-scan
  `pre_scan_imported_background_persist_for_loader` que recolecta la
  config completa (`BgPersistInfo`: store_var/retry/catch_up) de los
  `@background(store=db)` alcanzables — corre ANTES de `collect_imports`,
  mismo patrón que `main_imported_background_fns` (B10); (b) se puebla
  `ctx.bg_persistent_fns` de CADA módulo en
  `generate_module_rs_with_bindings` (imports + defs locales, los locales
  ganan en colisión); (c) la arm persistente de `gen_spawn_call` califica
  con `crate::` los 5 símbolos crate-root (`__fitz_run_persisted_spawn`,
  `__FITZ_BG_STORE_<VAR>`, `__FitzRetryConfig`/`__FitzBackoffKind`,
  `__ToFitzJson`) cuando emite desde un módulo (en el main
  `mod_path_prefix()` → `""`, main.rs sin cambios). Cubre spawn+def
  co-localizados en un módulo y spawn-en-A/def-en-B.

### Tests

- `background_persistent_spawn_in_module_compiles_to_binary_v0_37_9`
  (spawn dentro del módulo, inspecciona `worker.rs`: path persistente +
  símbolos `crate::`-calificados) +
  `background_persistent_spawn_cross_module_def_v0_37_9` (spawn en A, def
  en B). Los 2 de v0.37.7/8 verdes sin regresión.

### Notas

- La persistencia de `@background` (v0.37.7 + cross-module v0.37.8 +
  spawn-desde-módulo v0.37.9) queda completa: paridad bit-a-bit
  `fitz run` ↔ `fitz build` desde cualquier layout de módulos. Restante
  (sin cambio): el valor de retorno de un spawn persistido se descarta
  (fire-and-forget), y el fn debe retornar `Null`/`Result<Null>`.

## [v0.37.8] — 2026-08-10 — `@background(store=db)` cross-module en `fitz build` (port de B20)

Cierra la deuda residual de v0.37.7: un `@background(store=db)` fn + su
`let db = db.connect(...).await` co-localizados en un módulo importado ahora
compilan y persisten con `fitz build` (antes fallaba con `module let 'db':
RHS is not a literal`). Paridad bit-a-bit `fitz run` ↔ `fitz build`. Sin
sintaxis nueva; solo codegen.

### Fixed

- **Cross-module `@background(store=db)` no compilaba a binario.** El
  intérprete (`fitz run`) ya persistía un worker declarado en cualquier
  módulo (el registry es global), pero el binario nativo solo persistía los
  `@background(store=db)` declarados en el main. Es exactamente el gap que
  `@cron(store=db)` tenía antes de **B20 (v0.37.1)**. Fix = port de esa
  maquinaria: `LoadedModule` gana `background_fn_stmts` +
  `hoisted_background_store_vars`; `gen_module_top_let` +
  `collect_module_sigs` hoistean/toleran el `let db = db.connect(...).await`
  co-localizado del módulo (a `crate::<mod>::__FITZ_STATE_DB` +
  `__fitz_init_state_db()`) vía un set `module_background_store_vars`
  paralelo al de cron; `bg_persistent_fns` se puebla cross-module con
  `module_path: Some(mod)`; `emit_background_boot` inicializa +
  materializa el store del módulo y setea el global `__FITZ_BG_STORE_DB`.
- Validado contra Postgres local: un `worker.fitz` con `@background(store=db)`
  + `let db`, importado por un `main.fitz` que hace `spawn(send_email(...))`
  en un handler, compila a binario y persiste + catch_up con paridad ante
  `fitz run`.

### Notas

- **Deuda residual restante**: el `spawn(...)` debe estar en el MAIN. Un
  `spawn(...)` ubicado dentro de un módulo requiere que el ctx del módulo
  también resuelva el store global — diferido. En `fitz run` funciona desde
  cualquier lado.

## [v0.37.7] — 2026-08-10 — `@background(store=db)` persistente: jobs fire-and-forget que sobreviven reinicios

Los background jobs (`@background` + `spawn(...)`) ganan persistencia opt-in sobre
Postgres, paralela a la que `@cron` tiene desde v0.11.2. Sin broker externo. Paridad
bit-a-bit `fitz run` ↔ `fitz build`. Extensión VSCode: LSP completion de `@background`
cita los kwargs nuevos (grammar sin cambios).

### Added

- **`@background(store=db, catch_up=true, retry={...})`.** Con `store=db`, cada
  `spawn(worker(args))` de un `@background` persistido queda registrado en la tabla
  única `fitz_bg_jobs` (auto-creada al boot con `CREATE TABLE IF NOT EXISTS`):
  `fn_name`, `args_json` (los args serializados a JSON — **primitivos + compuestos**
  vía `value_to_json` / `__ToFitzJson`, solo para visibilidad, nunca se deserializan),
  `status` (`running` → `retrying` → `ok`/`failed`), `attempt`, `error`, timestamps.
  Un job = un spawn = una fila (los retries actualizan la misma fila, a diferencia de
  las dos tablas de `@cron`).
- **best-effort**: el INSERT es el primer paso *dentro* de la task spawneada, así que
  `spawn(...)` sigue fire-and-forget no-bloqueante. Si la DB está caída al momento del
  spawn, el job igual corre.
- **`catch_up=true`**: al boot marca los huérfanos (`running`/`retrying` que un crash
  dejó a medias) como `failed` con `error = 'orphaned by restart'`. NO los re-ejecuta
  (menos riesgo de doble-ejecución de jobs no-idempotentes; el operador re-dispara).
- **`retry={max, backoff, initial_secs, max_secs}`**: retry con backoff aplicado en el
  path persistente; un `return Err(...)` del worker cuenta como fallo del job.
- Módulo nuevo `src/background_jobs.rs` (registry + helpers SQL + `run_persisted_spawn`)
  + prelude paralelo bit-a-bit en el codegen (`__fitz_bg_*` + trait `__FitzBgStoreFrom`).
- Ejemplo runnable `examples/guide/30c-background-persistente.fitz`. Sub-sección nueva
  en el cap 30 de la guía + tabla del curso M5.C4 actualizada.

### Notas

- Sin `store`, `@background` sigue siendo fire-and-forget in-memory (backward-compat).
- El valor de retorno de un spawn persistido se descarta (fire-and-forget; resuelve a
  `Null`). El `@background(store=db)` fn debe retornar `Null`/`Result<Null>` (como `@cron`).
- **Deuda residual**: cross-module `@background(store=db)` en `fitz build` — el
  intérprete lo persiste (registry global), el binario solo persiste los declarados en
  el main (paralelo a cómo `@cron` arrancó antes de B20). Documentado en
  `docs/deudas-post-5b.md`.

## [v0.37.6] — 2026-08-08 — State compartido entre handlers HTTP de un MÓDULO (deuda 5b.6) CERRADA

La deuda más impactante del stack web: un `let db = db.connect(...).await` top-level
de un MÓDULO (no el main) referenciado por los handlers HTTP de ese módulo ahora
compila con `fitz build`, con paridad ante `fitz run`. Sin sintaxis nueva. Extensión
VSCode: bump de versión (sin cambio de comportamiento).

### Fixed

- **Módulo con `let db = db.connect(...).await` usado por sus handlers rompía
  `fitz build`.** Los handlers son fns Rust top-level que no capturan el env del
  módulo → el codegen emitía un `pub fn db()` roto (E0728 async en fn sync) o un
  OnceCell sin materialización (E0425). El intérprete sí andaba. Fix: el ctx de
  módulo ahora corre la maquinaria de shared state del caso main
  (`detect_shared_state` + `resolve_state_var_types`); `gen_module_top_let` emite
  `__FITZ_STATE_X` (OnceCell async / LazyLock sync) para shared-state vars, main
  inicializa los async antes de `axum::serve` (de-dup vs cron-stores), y cada
  handler materializa el local vía `gen_top_fn`.
- **Bug de shadowing colateral** (`gen_expr` Ident): un const/accessor de módulo
  ya no tapa a un local del mismo nombre — el check de `own_consts` quedó gateado
  por `!var_in_any_scope` (los locales shadowean los globals de módulo, semántica
  estándar). Era el motivo raíz por el que la materialización no "ganaba" en
  módulos (el shared `db` resolvía al accessor `db()` en vez del local).

### Changed

- **Boilerplate `api-orm-full`**: `jobs.fitz` pasó del workaround (un `cron_db`
  aparte + `db.connect(db_url()).await?` per-request en cada handler) al diseño
  natural: un único `let db = db.connect(db_url()).await` compartido que usan TANTO
  el `@cron(store=db)` COMO el handler `@admin GET /jobs` (que lo desempaca con
  `match`). Sin connect per-request.

### Notes

- Tests nuevos: `compile_e2e::module_async_shared_state_compiles_v0_37_6` +
  `compile_e2e::module_shared_db_both_cron_store_and_handler_no_double_init_v0_37_6`
  (build + grep de una sola init call → sin doble `db.connect`).
- Smoke real Postgres local + smoke guía 290 (guard del cambio de gating de
  `own_consts`, un path core de `gen_expr`) verde.
- Límite: el shared state se comparte por MÓDULO (no cross-module); cada módulo
  declara su `let db`, que POOL_CACHE dedupea a la misma conexión.

## [v0.37.5] — 2026-08-08 — Gating del `use` de observability en módulos + drift de migraciones + `@admin` transitivo (moot)

Tanda del inventario post-v0.37.4. Sin sintaxis nueva. Extensión VSCode: bump de
versión (el LSP recibe la clarificación de los helpers de `@admin` cross-module,
sin cambio de comportamiento).

### Fixed

- **Multi-módulo HTTP sin logging: `fitz build` rompía con E0432.** Un programa
  con un handler HTTP en un módulo importado pero SIN ningún `log.X(...)` fallaba
  con `unresolved import crate::__fitz_otel_*`/`__fitz_log_*`. v0.37.1 volvió la
  observability opt-in (el crate root define esos símbolos solo con `uses_logging`),
  pero el módulo emitía el `use` gated por `module_has_http` — mismatch. Fix: un
  post-process strip en `generate_project` remueve los `use` de observability de los
  módulos cuando `uses_logging` global es false (donde son garantizadamente
  spurios).

### Changed

- **README de `api-orm-full`**: corregido el drift sobre migraciones — decía que
  `fitz db diff`/`migrate` "no existen todavía", pero existen desde Fase 10.6
  (`fitz db diff/migrate/status/new/rollback/check/history/squash/inspect/stamp`).
  El boilerplate usa `schema.fitz` DDL como elección deliberada de showcase; el
  README ahora apunta a `fitz db` + al cap M6.C6 del curso para el flujo real.
- **Doc-comments de los helpers de resolución de `role` cross-module** (codegen +
  LSP + checker): clarifican que solo siguen imports DIRECTOS — Fitz no tiene
  re-export, así que el `User` de un `@auth_provider` es siempre un import directo
  del módulo que lo declara (un `@admin` "transitivo" no puede darse).

### Notes

- El ítem "`@admin` transitivo" del inventario resultó MOOT: Fitz rechaza
  `from A import User` cuando A solo re-importó `User` (no hay re-export), así que
  el fix directo de v0.37.4 ya cubre todos los casos. Detalle en
  `docs/deudas-post-5b.md`.

## [v0.37.4] — 2026-08-08 — Accessors `DbRow` en el intérprete + `@admin`/`@requires` cross-module (parity `fitz run` ↔ `fitz build`)

Dos gaps de paridad `fitz run` ↔ `fitz build` descubiertos al enriquecer el
boilerplate `api-orm-full` con cron persistente + un endpoint de audit log. Sin
sintaxis nueva. Extensión VSCode: bump de versión (el LSP recibe el fix de
`@admin` cross-module; sin cambio de grammar).

### Fixed

- **`conn.query(...)` raw rows: `r.get_str/get_int/get_float/get_bool` ahora
  andan en `fitz run`.** El checker tipa las filas como `List<DbRow>` y el
  codegen emite `__FitzDbRow` con esos accessors tipados (`Result<T>`), pero el
  intérprete devolvía `List<Map>` → `r.get_str("col")` moría en runtime con
  *"`Map` has no method named `get_str`"* aunque `fitz check`/`fitz build` lo
  aceptaran. Fix en el evaluator: nuevo `db_row_get` + 4 arms en el dispatch de
  `Value::Map` (`len` ya existía). Semántica + mensajes de error espejo del
  codegen (col ausente / NULL / tipo PG que no matchea → `Err`).
- **`@admin`/`@requires` sobre un handler cuyo `@auth_provider` devuelve un
  `User` importado a OTRO módulo.** El chequeo del campo `role: Str` sólo miraba
  los `TypeDef` del módulo del provider; en el layout multi-archivo canónico el
  provider importa su `User` (`auth.fitz` hace `from models import User`, `User`
  vive en `models.fitz`), así que `role` era invisible y `@admin` fallaba en
  `fitz build` (aunque `fitz check` en modo manifest pasara). Fix: helper
  `type_decl_has_role_field` en `types.rs` + los tres pre-scans cross-module
  (codegen, LSP, checker single-file) siguen los imports del módulo del provider
  para resolver `role: Str` en el módulo hermano.

### Changed

- **Boilerplate `api-orm-full`**: su `@cron` pasó a persistente — `@cron("0 0 *
  * *", tz="UTC", retry={...}, store=cron_db)` (binding top-level `cron_db` sólo
  para el `store=`; los handlers conectan per-request vía el builtin `db`) — más
  un endpoint nuevo `@admin GET /jobs` que lee `fitz_cron_runs` con los accessors
  `DbRow`. Showcasea la persistencia de cron + el audit log con paridad `fitz
  run` ↔ `fitz build`.

### Notes

- Tests nuevos: `evaluator::tests::db_row_get_typed_accessors_parity_v0_37_3` +
  `compile_e2e::admin_cross_module_role_field_via_provider_module_imports_v0_37_3`.
- Validado end-to-end contra Postgres local: `api-orm-full` boot → register →
  promover a admin → login → `GET /jobs` devuelve el audit log; sin token → 401.

## [v0.37.3] — 2026-08-08 — `fitz run` con `@cron(store=db)` persistente (runtime tokio unificado del intérprete)

Fix del intérprete (`fitz run`). Sin sintaxis nueva. `fitz build` sin cambios.
Extensión VSCode sin cambios (solo bump de versión, sin rebuild del `.vsix`).

### Fixed

- **`fitz run` con `@cron(..., store=db)` persistente ahora funciona** — cierra la
  limitación residual (a) de B20. Antes, un programa con `@cron(store=db)` +
  `let db = db.connect(...).await` fallaba al arrancar el scheduler con
  *"A Tokio 1.x context was found, but it is being shutdown"* al crear
  `fitz_cron_jobs`. Afectaba por igual cron-only y HTTP+cron. `fitz build` (binario
  nativo) nunca lo sufrió. **Root cause**: el intérprete construía dos runtimes
  tokio en secuencia — uno `current_thread` para el eval (que abría el `TcpStream`
  de Postgres y se dropeaba al terminar el eval) y otro `multi_thread` para
  serve/scheduler; la primera query del cron manejaba el `TcpStream` atado al
  reactor del runtime ya cerrado. **Fix**: `run_file` ahora construye UN runtime
  `multi_thread` compartido up-front, corre el eval sobre él (`block_on` en el main
  thread, mismo stack que antes) y drivea serve/scheduler sobre el mismo runtime,
  así el reactor del `TcpStream` vive todo el proceso. Paridad `fitz run` ↔
  `fitz build`.

### Changed

- **`src/http.rs`**: nuevo `build_server_runtime()` (única fuente de la config del
  runtime multi_thread + 16 MB de stack) + `serve_on_runtime(&Runtime, …)`.
  `serve()` queda como wrapper que construye su propio runtime y delega.
- **`src/cron_jobs.rs`**: nuevo `run_scheduler_on_runtime(&Runtime, …)`.
  `run_scheduler_only()` queda como wrapper. Cron-only mode pasa a usar el runtime
  con stacks de 16 MB (antes usaba el default de 2 MB).
- **`src/main.rs` (`run_file`)**: único call site tocado — construye el runtime
  compartido, corre el eval con `shared_runtime.block_on(eval_with_base_and_deps(…))`
  y pasa `&shared_runtime` a las dos ramas. Los ~15 subcomandos restantes
  (check/test/repl/dev/db.*) quedan intactos.

### Notes

- Efecto colateral menor: todo `fitz run` (incluido un CLI puro) construye ahora un
  runtime `multi_thread` para el eval (antes `current_thread`). El eval se sigue
  drivando en el main thread vía `block_on` → comportamiento idéntico; el único
  costo es unos worker threads idle durante la (breve) vida de un CLI puro.
- Test E2E nuevo `tests/cron_run_real_postgres.rs` (`#[ignore]` opt-in con
  `FITZ_TEST_PG_URL`): spawnea `fitz run` real y verifica persistencia de runs en
  cron-only y HTTP+cron contra Postgres.

## [v0.37.2] — 2026-08-07 — B20 residual A (cron store shadowing) + file-lock retry widening

Tanda de quick wins post-v0.37.1. `fitz run` sin cambios; sin sintaxis nueva.
Extensión VSCode sin cambios (solo bump de versión).

### Fixed

- **`@cron(store=X)` con el mismo nombre de store en dos módulos importados
  (B20 residual A).** Dos módulos importados que cada uno declaran
  `let db = db.connect(...).await` + `@cron(store=db)` emitían un
  `let db = crate::alpha::...` seguido de `let db = crate::beta::...` en el mismo
  scope de `main()` — el segundo shadoweaba al primero, así que AMBOS crons
  corrían contra la conexión de beta (bug silencioso de conexión equivocada).
  Ahora cada store hoisteado usa un local module-qualified único
  (`__fitz_cron_store_<mod>_<var>`) y el `store` de cada job resuelve por
  `(module_path, store_var)`. El caso canónico (un módulo / una conexión) no
  cambia.

### Changed

- **`copy_binary_with_retry` — ventana de retry ampliada.** `fitz build` copia el
  binario recién linkeado a destino con retry ante `os error 32`
  (ERROR_SHARING_VIOLATION en Windows: el AV/indexer retiene el handle del
  `.exe`). La ventana pasó de 8 intentos × backoff lineal 25ms (~700ms) a 20
  intentos × backoff exponencial capeado (50→400ms, ~6.7s worst case). Los sleeps
  solo ocurren en un retry; el happy path es instantáneo. Estabiliza dos tests de
  `compile_e2e` que flakeaban en máquinas con AV agresivo.

### Notes

- **`@cron(store=X)` en main con `X` importado (B20 residual B)** falla loud en
  compile-time (nunca conexión silenciosa equivocada); soporte completo diferido
  (topología rara). Test guard agregado.
- **`fitz run` de cron+store persistente** sigue con su limitación de lifecycle
  del runtime tokio del intérprete (diagnóstico completo mapeado esta tanda; fix =
  refactor moderado de unificación de runtimes, diferido a sub-paso dedicado).
- **L2 (`with_temp_output` helper)** ya estaba hecho — deuda stale marcada CERRADA.

## [v0.37.1] — 2026-08-07 — `@cron(store=X)` cross-module + `import_root` fallback en `fitz build` + observability opt-in

Tres cierres de deuda del codegen (`fitz build`), sin sintaxis nueva. `fitz run`
(intérprete) no cambia. La extensión VSCode no cambia (nada toca grammar/LSP).

### Fixed

- **`@cron(..., store=X)` cross-module compila a binario (B20).** Un cron con
  `@cron(store=db)` y su `let db = db.connect(...).await` declarados **ambos en
  un módulo importado** ahora compila con `fitz build` (antes: `E0425 cannot
  find value 'db'` emitido en `main()` del crate root, + accessor async roto en
  el módulo). El codegen hoistea el binding async co-localizado a un `OnceCell`
  crate-visible + `__fitz_init_state_X()`; el spawn del cron drivea el init +
  materializa el local antes de `(&db).into_store()`. Distingue el caso
  co-localizado del binding-en-`main` (b19_derived, sin regresión). Smoke real
  Postgres: el binario crea `fitz_cron_jobs`/`fitz_cron_runs` y persiste runs.
  `fitz run` de cron+store persistente mantiene su limitación de lifecycle del
  runtime tokio del intérprete (pre-existente, v0.11.2). Cierra la deuda B20.
- **`from sub.mod import X` resuelve al `import_root` en `fitz build`
  (loader_absoluto).** Un módulo en subcarpeta (`src/data/users.fitz`) que
  importa un sibling del root del proyecto (`from types.user import User`) ahora
  resuelve a `src/types/user.fitz` — el fallback `import_root` que ya tenía
  `fitz run` se portó al loader de `fitz build` (`ModuleLoader` suma
  `import_root`; `resolve_path` devuelve candidatos base_dir→import_root).
  Paridad run↔build.

### Changed

- **`fitz build` — observability opt-in vía `log.X` (recorta deps OTel).** La
  auto-observability del binario nativo (access-log HTTP + spans + métricas + las
  tres crates OpenTelemetry `opentelemetry`/`_sdk`/`-otlp` + `metrics`/`tracing`)
  se linkea cuando el programa usa **al menos un** builtin `log.X(...)`; un
  servidor HTTP que no loguee compila a un binario **más liviano** sin esas deps
  (recorta binario + ~3-5 min de compilación/CI sobre los ~290 ejemplos del
  smoke). Con una línea `log.info("startup")` recuperás todo el stack.
  **`fitz run` (dev) mantiene el access-log automático siempre** — asimetría
  deliberada dev/prod (el intérprete es un binario fijo que no linkea deps OTel,
  ver los requests en dev no cuesta nada). El comportamiento del programa
  (responses, tus `log.X`) es idéntico en run y build. Cierra la deuda de gating
  de OTel abierta desde v0.13.1.

## [v0.37.0] — 2026-08-06 — CW.9 follow-ups: cierra las 3 deudas residuales del emisor wasm (38/38 componentes de la companion UI dual-targetean)

Cierre de las tres deudas residuales de CW.9 (v0.35.0 / v0.36.0). Cambio
confinado a `src/view/` — no toca el pipeline clásico. Byte-compat: los
`examples/view/` regeneran idéntico (`git status` limpio); sin sintaxis nueva
del `.fitzv`; sin cambio de la extensión VSCode.

### Added

- **Props interpoladas a un target `Nullable<T>`.** `<Badge caption="{note}" />`
  con `caption: Str?` en el hijo ya no difiere en wasm. `lower_child_prop_value`
  recibe los fields del padre (`ctx.state_fields`) y decide el wrap por la
  nulabilidad del SOURCE: un bare state field ya `Nullable<inner>` es
  `Option<inner>` en Rust → clon directo; un source no-nullable (bare `inner`,
  loop var, aritmética, field access de nominal) → wrap `Some(...)`. (Un field
  nominal que sea a su vez `Nullable` doblaría el wrap — un edge raro sin el
  mapa loop-var→nominal, documentado; la falla sale como error claro de rustc.)
- **Relleno de campos omitidos en defaults `List<nominal>`.** `options:
  List<FieldOption> = [ FieldOption { label: "Red", value: "red" } ]` que omite
  `on: Bool = true` ya no rompe con "missing field `on`": el `NominalRegistry`
  ahora carga el default declarado de cada field (`insert_with_defaults`,
  poblado por `load_imported_nominals_with_deps`), y `default_expr_to_rust`
  rellena los campos omitidos con su default en orden declarado — byte-accurate
  con SSR / Fitz clásico. Los campos suministrados mantienen el emit exacto
  previo (byte-idéntico para el caso all-fields). Un campo omitido SIN default
  emite un error claro (no un "missing field" crudo de rustc).
- **Fall-through de `data-flv-click` a un callback del padre (componentes
  controlados).** Un `data-flv-click="page_prev"` cuyo nombre NO es un `event`
  local del componente FALL-THROUGH: si un padre lo bindea (`<Ctrl
  @page_prev="..." />`) el click dispara el slot `__on_page_prev` del componente
  (paralelo al bubbling `@event` de 11.7.c); si nadie lo bindea (mount
  standalone), no se cablea listener (el control queda inerte hasta componerse).
  Nuevo `resolve_event_binding` con `HandlerBinding::{Local, Bubbled, Unbound}`.
  Destraba los componentes controlados (Pager, ConfirmDialog) → **38/38** de la
  companion UI de fitz-liveviews dual-targetean.

### Changed

- **El checker del `.fitzv` acepta `<Child @X="..." />` cuando el hijo EMITE `X`
  vía `data-flv-*="X"`** (no solo cuando lo declara como `event`). Nuevo
  `component_emits_fallthrough_event` en `src/view/check.rs`; un nombre que no
  es ni event declarado ni emisor data-flv sigue siendo un typo y rechaza. Esto
  hace alcanzable el path Bubbled end-to-end (sin la relajación, el checker
  bloqueaba componer un componente controlado). Solo *relaja* — nada que pasaba
  antes ahora falla (los 227 tests SSR de la ui-gallery siguen verdes).

### Verificación

- `cargo test --lib` **4020** (default) / **4181** (`--features lsp`) — 0 fallos.
  fmt + clippy (`--lib --tests`, default y `--features lsp`) `-D warnings`
  limpios. Los 24 view smokes regeneran `examples/view/` byte-idéntico
  (`git status` limpio). `fitz test` de `fitz-liveviews/examples/ui-gallery`
  **227/227** SSR verdes (la relajación del checker no rompe SSR).
- Build wasm **real** (`fitz build --target wasm-client` → `wasm-pack` → `:-)
  Done`) validó los tres fixes en un proyecto scratchpad + el showcase de la
  companion UI (con Pager + ConfirmDialog sumados) compila a WASM real.

## [v0.36.0] — 2026-08-06 — CW.9 iter2: tres fixes del emisor wasm (36/38 componentes de la companion UI dual-targetean)

Continuación de CW.9 (v0.35.0). Tras un barrido de los componentes
restantes de la companion UI de fitz-liveviews (16/18 ya compilaban a
wasm), tres fixes chicos del emisor view (`src/view/codegen_wasm.rs`)
cierran los gaps que quedaban para poblar los componentes lista-driven en
el showcase. Cambio confinado a `src/view/` — no toca el pipeline clásico.
Byte-compat: los `examples/view/` regeneran idéntico (`git status` limpio);
sin sintaxis nueva del `.fitzv`; sin cambio de la extensión VSCode.

### Added
- **`for x in <list>` en cuerpos de helper (`.push`/`.clear` sobre local).**
  `lower_stmt`/`lower_expr_stmt` iteran una lista además de un range, y
  `<local>.push(x)` / `.clear()` en un cuerpo de helper lowerea a
  `Vec::push`/`clear` (el local se declara `let mut`). Destraba helpers
  como `page_range` (`let out = []; for n in 1..X { out.push(n) }`).
- **Props interpoladas no-primitivas (no-nullable).** `<Select
  options="{opts}" />` — un bare state field / loop var / field access
  lowerea a un `.clone()` que sirve para cualquier target `Clone`
  no-nullable (primitivo, `List<T>`, `Map<K,V>`, nominal). El checker
  (`light_check_interpolated_prop`) ya validaba la compatibilidad. Un
  target `Nullable<T>` sigue difiriendo (necesita el wrap `Some(...)` según
  el tipo del field del parent, aún no threadeado).
- **Defaults de state `List<nominal>`.** `default_expr_to_rust` acepta un
  struct-literal nominal (`FieldOption { label: "Red", value: "red", on:
  true }`), típicamente como elemento de `options: List<FieldOption> =
  [ ... ]`. El tipo de cada campo se infiere del kind del literal. MVP:
  todos los campos deben especificarse (rellenar omitidos con los defaults
  del nominal necesita el registry de campos, deuda residual).

### Notas / deuda residual
- Con los tres fixes, **36 de 38** componentes de la companion UI
  dual-targetean a wasm. Los 2 restantes — **Pager** y **ConfirmDialog** —
  son componentes **controlados**: sus botones usan eventos fall-through al
  parent (`data-flv-click="page_prev"` / `confirm_delete`, que no son
  eventos locales del componente), que no tienen equivalente en un mount
  wasm standalone sin wiring de event-bubbling. Quedan SSR-apropiados.
- Deuda: props interpoladas a target `Nullable<T>` (wrap `Some`); rellenar
  campos omitidos en defaults `List<nominal>` (necesita el registry).

## [v0.35.0] — 2026-08-06 — CW.9: expansión del envelope client-WASM (5 componentes markup/list de la companion UI dual-targetean)

Cierre de los gaps del **envelope client-WASM** del `.fitzv` (CW.9, para
fitz-liveviews) en `src/view/`. Sin sintaxis nueva del `.fitzv`; sin
cambio de la extensión VSCode (`raw_html`/`html`/`flv` son helpers del
framework que ya existían). Validado end-to-end a `.wasm` real
(`wasm-pack :-) Done`) **con 5 componentes reales de la companion UI de
fitz-liveviews** que antes eran SSR-only y ahora compilan a wasm desde su
source exacto, con su SSR byte-idéntico (227 tests de la ui-gallery
verdes): **Button** (icono SVG vía `icon() -> Html` → sink), **Select** y
**RadioGroup** (`{#for o in options}` sobre `List<FieldOption>` con
`{#if o.on}`), **GridToolbar** (`{raw_html(actions)}`), y **BarChart**
(`bar_scale` con `for b in bars`). Cambio confinado a `src/view/` — no
toca el pipeline clásico (parser/checker/evaluator/`codegen.rs`).

### Added
- **CW.9 (1a) — `?` / `Result` en cuerpos de helper-fn.** `Result<T>` mapea
  a Rust `Result<T, String>` (Err pineado a `String`, igual que classic
  Fitz + el stub `@rpc`). Bajan los constructores `Ok(v)` / `Err(e)`, la
  propagación con `?`, y los arms de `match` que bindean `Ok(v)` / `Err(e)`
  (con la binding visible en el body del arm — antes el scope del arm no se
  threadeaba). Un helper puede validar y propagar fallas; un caller puede
  `match`earlo.
- **CW.9 (1b) — sink raw-HTML en interpolación.** `{raw_html(x)}` /
  `{html(x)}` como hijo de un elemento inyecta el markup (sin escapar) vía
  `set_inner_html` sobre el padre, en vez de un text node que escapa
  (modelo `dangerouslySetInnerHTML` de React — la interpolación raw-HTML
  debe ser el ÚNICO contenido de su padre). Requiere `from fitz_liveviews
  import raw_html` en scope (paralelo a `flv`). Es el camino para
  dual-targetear los componentes SSR cuyos helpers arman strings de markup
  (`icon`, chart/grid helpers). Los folding helpers
  (`h_join`/`h_when`/`h_either`) siguen SSR-only (sin forma de string
  único). No soportado todavía dentro de componentes keep-node /
  hidratables.
- **CW.9 (1c) — shim `Html` en wasm.** El newtype `Html` de fitz-liveviews
  (`type Html { raw: Str }`) transpila al target wasm vía un shim
  per-bundle: `Html` → `struct __FlvHtml { raw: String }`, con
  constructores `html(x)` / `raw_html(x)` y field access `.raw`. Esto
  destraba los helpers que devuelven `Html` (p.ej. `icon` → un string SVG),
  que el sink 1b luego renderiza como DOM (`{raw_html(icon.raw)}`). El shim
  se emite solo cuando algún fn importado usa `Html` (byte-idéntico si no).
  Los folding helpers (`h_join`/`h_when`/`h_either`) siguen SSR-only.
- **CW.9 — alias de fns en el loader wasm.** `from icon import icon as
  render_icon` ahora registra el fn transpilado también bajo el alias, así
  un template que llama `render_icon(...)` resuelve (paralelo al alias de
  nominales/componentes de CW.8). En `src/view/wasm_build.rs`.
- **CW.9 — marker raw-HTML simétrico en el SSR.** El emitter SSR
  (`src/view/codegen_ssr.rs`) stripea `{raw_html(x)}` / `{html(x)}` en
  interpolación a `{x}` (el `{expr}` clásico ya es crudo — el escape es
  opt-in con `flv()`). Así el MISMO source `{raw_html(icon.raw)}` es raw en
  ambos targets y byte-idéntico al idiomático `{icon.raw}` en SSR.
- **CW.9 — field access booleano en condición `{#if}`.** `lower_cond_expr`
  acepta `{#if o.on}` (un field `Bool` sobre un loop var de `{#for}`, p.ej.
  una `List<FieldOption>`). Destraba `Select` / `RadioGroup` sin re-author.
- **CW.9 — `for x in <list>` en cuerpos de helper.** `lower_stmt` itera una
  lista además de un range (`for b in bars` → `for b in (bars).iter().cloned()`,
  el loop var queda owned y la lista sigue disponible para un `.map(...)`
  posterior). Destraba `BarChart` (`bar_scale`) sin re-author.

### Deuda residual
- **Reactividad fine-grained** (patch-in-place vs naive re-render, para que
  un `@input` de texto conserve el caret) sigue abierta — es un upgrade del
  render model (Fase 11.10), ortogonal a lo de arriba.

## [v0.34.1] — 2026-08-05 — `fitz dev` wasm-client: re-resolución del manifest en vivo (último follow-up de Fase 11.13)

Patch — el modo wasm-client de `fitz dev` ahora **re-resuelve
`fitz.toml` cuando lo guardás**, sin reiniciar. Cierra el último
follow-up abierto de la Fase 11.13 (Approach C). Cambio contenido en
`src/main.rs` — no toca el emisor view (los `examples/view/` regeneran
byte-idéntico) ni la extensión VSCode (sin sintaxis nueva, sin superficie
LSP).

Antes: el loop wasm resolvía el manifest **una sola vez** al arrancar y
reusaba ese `ResolvedEntry` en cada rebuild. Editar el `.fitzv` se tomaba
(el entry se re-lee cada build), pero repuntar `[bin].main`, agregar una
`[dependencies]` o cambiar `[flags]` usaban la resolución vieja. (El path
clásico native/SSR ya era live por kill+respawn del child; no cambia.)

### Changed
- **`fitz dev` (modo wasm-client) re-resuelve `fitz.toml` en vivo.**
  Al guardar el manifest, re-resuelve entry + deps + flags y rebuildea
  con lo nuevo — repuntar `[bin].main`, agregar una dep de la companion
  UI (`from fitz_liveviews.ui.Badge import Badge` + la entry en
  `[dependencies]`) o editar `[flags]` se toman sin reiniciar.
- **Un `fitz.toml` roto a mitad de edición ya no mata el dev loop.** El
  core de resolución se extrajo a `try_resolve_entry_with_bin(...) ->
  Result<ResolvedEntry, String>` (los `exit(1)` → `Err`); el
  `resolve_entry_with_bin` clásico queda como wrapper que sale-en-error,
  así el resto de los subcomandos se comportan byte-idéntico. El loop
  atrapa el `Err`, imprime el error de parse y **sigue sirviendo el
  bundle anterior**, recuperándose al próximo save válido.
- **Nota de reinicio (opción a):** re-resolver solo alimenta los builds
  (entry + deps + flags). Renombrar el bin o cambiar su `mount` cambia el
  output dir `target/wasm/<pkg>/` + la host page del user, que el server
  ya corriendo no puede adoptar — el loop imprime una nota pidiendo
  reiniciar `fitz dev` en vez de driftear en silencio.

### Tests
- +2 unit deterministas (`phase_11_13_change_is_manifest_only_for_fitz_toml`,
  `phase_11_13_try_resolve_single_file_is_ok_without_touching_manifest`).
  Suite: **3999 lib** + **123 cli_e2e** + `view_*` byte-compat todos verdes;
  `fmt --all --check` + clippy `--lib --tests --bins -D warnings` (default y
  `--features lsp`) limpios.
- **Validado en un proyecto multi-bin real** (`fitz dev --bin web`,
  `wasm-pack` real): repuntar `[bin].main` App→App2 se toma en vivo (bundle
  regenerado con el nuevo entry, rebuild ~1.3s, js=200, sin reiniciar); un
  `fitz.toml` roto imprime el error y sigue sirviendo el bundle previo
  (proceso vivo); restaurar el manifest re-resuelve; un edit de `.fitzv`
  rebuildea sin re-resolver (path no-manifest byte-idéntico).

**Follow-ups restantes de 11.13:** el runtime data-driven verdadero
(deuda 🟡, gatillado por la VM de expresiones en WASM); correr ambos bins
(`server` + `web`) en UN `fitz dev` (orquestación dual-process).

## [v0.34.0] — 2026-08-05 — `--bin` en `fitz run` + `fitz dev`: selección de bin en proyectos multi-bin (workflow fullstack)

Minor — `--bin <nombre>` selecciona el bin en un manifest con varios
`[[bin]]`. **`fitz run` gana selección multi-bin** (estaba diferido desde
la Fase 11.5.b) y `fitz dev --bin` extiende el hot reload a proyectos
multi-bin.

### Added
- **`fitz run --bin <nombre>`** — corre un bin específico de un manifest
  multi-bin. Sin `--bin`, un proyecto multi-bin es ambiguo y `fitz run`
  pide que elijas (mismo error que `fitz build`).
- **`fitz dev --bin <nombre>`** — dev-ea un bin específico. Un bin
  `wasm-client` entra al modo wasm (build + serve + live-reload +
  preservación de state); un bin native cae al respawn clásico (child
  `fitz run --bin <nombre>`).
- **Workflow fullstack `@rpc` (`server` + `web`)** — desbloqueado: `fitz
  run --bin server` (backend native) en una terminal + `fitz dev --bin
  web` (frontend wasm con hot reload) en otra.

### Notes
- Documentado donde vive el CLI: README, guía cap 25 (sección fullstack),
  `docs/architecture.md`, curso M1.C4, y los `--help` de `fitz run`/`fitz
  dev`. Es enteramente del lenguaje/CLI — la librería `fitz-liveviews` no
  cambia.
- Follow-up restante de 11.13: re-resolver `fitz.toml` en vivo (editar
  `[bin].main` con `fitz dev` corriendo aún requiere reiniciar); correr
  ambos bins en UN `fitz dev` (orquestación dual-process).

## [v0.33.1] — 2026-08-05 — `fitz dev` hot reload: preservación de state compuesto (Fase 11.13 slice-3, cierra Approach C)

Patch — completa la preservación de state del hot reload wasm-client
(v0.33.0). Ahora el snapshot cubre state **compuesto**, no solo primitivos.
**Sin cambios al path prod** (`fitz build` byte-idéntico; view smokes sin
cambios).

### Added
- **State compuesto sobrevive el hot reload.** El snapshot de `fitz dev`
  ahora serializa/restaura `List<T>` / `Map<Str, V>` / `Nullable<T>` /
  nominales importados (recursivo), además de los primitivos de v0.33.0.
  Nuevo `json_dump_value` (inverso recursivo de `json_restore_value`)
  construye el JSON del snapshot; `__fitz_dev_apply` gana la rama compuesta
  que reusa `json_restore_value`. Editás el template y una lista/mapa/tipo
  custom con el que estabas probando sobrevive el reload. **Cierra la Fase
  11.13 Approach C entera** (slices 1+2+3).

### Notes
- Tipos que no round-trippean por JSON (`Map` con key no-`Str`, tuplas,
  funciones) se omiten del snapshot y resetean a su default en el reload —
  simétrico con el restore.
- Dev-flag gated: `fitz build` sigue emitiendo `lib.rs`/`Cargo.toml`
  byte-idénticos. Validado en Chrome real (puppeteer): un `List<Str>` con
  2 items persiste el reload; el crate wasm con state compuesto compila
  limpio. Sin cambios a la extensión VSCode.

## [v0.33.0] — 2026-08-05 — `fitz dev` modo wasm-client: hot reload de `.fitzv` con auto-refresh + preservación de state (Fase 11.13 Approach C, slices 1+2)

Primera mitad de la Fase 11.13 (hot reload del template). `fitz dev`
gana un **modo wasm-client**: cuando el bin default del manifest apunta
a `wasm-client`, en vez de respawnear `fitz run` buildea el bundle
incremental y lo sirve con auto-refresh en el browser. Approach C
(reload incremental rápido, sin runtime de template en el cliente) — el
runtime data-driven verdadero (diff sin recompilar) queda como norte
futuro, gatillado por una VM de expresiones en WASM (deuda documentada).
**Sin cambios al path prod** (`fitz build`): el `lib.rs`/`Cargo.toml`
emitidos son byte-idénticos; los view smokes regeneran sin cambios.

### Added
- **`fitz dev` — modo wasm-client (slice-1).** Detecta un bin default
  `wasm-client` y, en lugar del respawn clásico:
  - Buildea con `wasm-pack --dev` (sin `wasm-opt`, incremental sobre el
    crate estable `target/wasm-build/` → cache de cargo caliente).
  - Sirve el root del proyecto en `127.0.0.1:<port>` (default `1234`,
    flag nuevo **`--port`**) desde un dev server axum
    (`src/dev_server.rs`): static serving (`Cache-Control: no-store`,
    MIME `application/wasm`, `favicon.ico` faltante → 204) + un
    WebSocket de live-reload en `/__fitz_dev_ws`, inyectando el snippet
    de reload en el `index.html` servido (o generando uno mínimo con el
    elemento del `mount`).
  - Ante cada save de `.fitzv`/`.fitz`/`fitz.toml`: rebuild + push de
    "reload" al browser (que hace `location.reload()`).
  - El watcher ahora reconoce `.fitzv` (antes lo ignoraba — extensión
    `.fitzv` ≠ `.fitz`).
- **Preservación de state a través del reload (slice-2).** En modo dev
  (gated por dev-flag; `fitz build` no lo emite), el codegen agrega en el
  componente root `__fitz_dev_snapshot()`/`__fitz_dev_apply()`, y el
  entry wrapper guarda el snapshot en `sessionStorage` en `beforeunload`
  y lo re-aplica tras `mount()`. Editás el template y el estado vivo
  (contadores, texto tipeado) **sobrevive** el reload. El Cargo.toml dev
  gana `serde_json` + la feature web-sys `Storage`. Cubre state
  **primitivo** (`Int`/`Float`/`Str`/`Bool`); compuesto
  (`List`/`Map`/nominal) resetea a default (follow-up).

### Notes
- **Reencuadre honesto vs. el roadmap original.** Decía *"aplica el diff
  sin recompilar el crate"*; en realidad (a) `fitz dev` ni watcheaba
  `.fitzv` ni invocaba `wasm-pack` (no había dev loop para el path
  client-WASM), y (b) el runtime data-driven verdadero choca con la VM de
  expresiones en WASM. C entrega ~90% del DX sentido a ~5% del costo, sin
  rewrite de codegen ni riesgo byte-compat.
- **Follow-ups abiertos** (no bloquean): state compuesto en el snapshot,
  multi-bin wasm dev (`--bin`), re-resolver `fitz.toml` en vivo, y el
  runtime data-driven verdadero (deuda 🟡 en `docs/deudas-post-5b.md`).
- **Validado en Chrome real** (puppeteer): editar un `<template>`
  auto-recarga el browser; con slice-2, un `count` bumpeado persiste el
  reload. Byte-compat de los view smokes verificado. Sin cambios a la
  extensión VSCode (11.13 no toca grammar ni LSP del `.fitz` clásico).

## [v0.32.0] — 2026-08-04 — ORM: expresiones SQL (`sql.now()`/`sql.raw()`) + aritmética de fechas en `.where` (cierra O1 + O3)

Cierra dos deudas del ORM detectadas construyendo fitzwatch (O1 + O3
de la auditoría 2026-06-18), con paridad bit-a-bit `fitz run` ↔ `fitz
build`. Documenta O4/O5/O6 como trade-off intencional.

### Added
- **Módulo `sql` con helpers de expresiones SQL para el ORM**:
  - **`sql.now()`** → `NOW()`, usable en los valores de `.update({...})`
    (se inlina en el `SET` sin bindear placeholder) y en predicados de
    `.where(...)`.
  - **`sql.raw("<fragmento>")`** → inlina el fragmento SQL verbatim en
    `.update` (p.ej. `sql.raw("streak + 1")`,
    `sql.raw("EXTRACT(EPOCH FROM ...)::int")`). **No parametrizado** —
    mismo modelo de confianza que `db.exec`/`db.query` crudo; en `fitz
    build` el argumento debe ser string literal.
  - Namespace `sql` (no `db`) a propósito: la conexión se liga como
    `let db = db.connect(...)`, que shadowearía el módulo `db`.
    Paralelo a `func.now()`/`text(...)` de SQLAlchemy.
- **Aritmética de fechas en `.where(...)`** sobre columns Date/DateTime:
  `m.col.plus_seconds(n)` → `("col" + make_interval(secs => n))`, más
  `minus_seconds`, `plus_minutes`/`minus_minutes`,
  `plus_hours`/`minus_hours`, `plus_days`/`minus_days`. El argumento
  puede ser otra columna, un literal o una var externa (se parametriza).
- Nueva variante `Value::SqlExpr(String)`; módulo `sql` registrado en
  el checker (`Type::Any`) + completions LSP (scope-level + after-dot).

### Docs
- `docs/db-orm.md`: §8 (`.update` con `sql.now/raw`), §7 (aritmética de
  fechas en `.where`), §18 (limitación de fechas actualizada), §28
  (párrafo nuevo "SQL complejo — trade-off intencional" que cierra
  O4/O5/O6). `docs/guide.md` cap 31 + curso M6.C3 actualizados.
- Deudas O1/O3 marcadas cerradas, O4/O5/O6 como trade-off documentado
  en `docs/deudas-post-5b.md`.
- Blog drafts ES/EN #12 (`12-orm-sql-expressions-fitz-*`).

### Fixed / chore
- **Extensión VSCode al día**: bump 0.27.0 → 0.32.0 con `.vsix`
  reconstruido (cierra la deuda de la extensión rezagada desde v0.28.0;
  bundlea el `fitz-lsp.exe` fresco con las completions nuevas).

### Tests
- 11 unit (translator evaluator + codegen + helpers) + 2 E2E reales
  contra Postgres + 1 compile_e2e (binario nativo, conexión llamada
  `db` para probar el caso de colisión). Suite: **3996 lib** + 161 LSP,
  `fmt` + `clippy --all-targets -D warnings` (default + `lsp`) limpios.

## [v0.31.0] — 2026-08-03 — Phase 11.12: isomorphic SSR render-to-string (closes 11.12)

### Added — the SSR emitter now paints the exact server DOM the client-WASM hydration adopts

Cierra **Fase 11.12 entera**: la mitad server del render isomórfico. Un
`.fitzv` marcado `hydrate` que compila a SSR (`src/view/codegen_ssr.rs`) ahora
emite el HTML byte-por-byte que el adopt walk client-WASM (Fase 11.12 slices
1–4) espera — antes ese HTML se escribía a mano en cada ejemplo. Cuatro sub-pasos
acumulados sobre v0.30.4 (SSR-1/2/3 fueron sin bump; SSR-4 cierra el arco):

- **SSR-1** — `<script type="application/json" id="__flv_state_<Comp>">` con el
  state serializado (`to_json(state)`) al final del render del root.
- **SSR-2** — markers `<!--fi-->` alrededor de cada interpolación en contexto
  mixto (`Hello, {name}!`), para que el browser pinte text nodes distintos.
- **SSR-3** — anchors `<!--fr-->` alrededor de cada región `{#if}`/`{#for}`
  top-level, alineados con `keep_region_index()` del build walk.
- **SSR-4** (este release) — composición: el padre hidratable envuelve cada
  `<Child />` en `<div class="__fitz-child-<Name>">` e inlina el contenido de
  slot provisto por el padre (renderizado en **scope del padre** — sus state
  fields + handlers → `data-flv-*`) en el `<slot>` del hijo, threadeado como el
  argumento `__slot: Str` del render fn del hijo. El root emite su state script;
  un hijo compuesto NO (su state se re-deriva de props en el cliente). Named
  slots en SSR quedan diferidos (puntero claro).

**Sin sintaxis nueva** (el marcador `hydrate` ya existía desde v0.30.4).
Cambio interno del emitter SSR: no-hidratables byte-idénticos; el WASM `lib.rs`
de los ejemplos no cambia. Validado en Chrome real 10/10 sirviendo el HTML
**generado** por el render fn (no el hand-authored): witnesses sobreviven la
adopción cross-boundary, state restaurado, eventos de slot (padre) y de hijo
funcionan, el state del hijo persiste el re-render del padre. 3985 unit
(+6 `ssr4_*`), fmt + clippy `--lib --tests --bins -D warnings` limpios (default
y `--features lsp`).

## [v0.30.3] — 2026-08-02 — Phase 11.12 slice 3: hydration restores composite state

### Added — the `<script>` state payload now restores `List` / `Map` / `Nullable` / nominal fields

Slice 1/2 restored only **primitive scalars** (`Str`/`Int`/`Float`/`Bool`) from
the SSR `<script type="application/json">` payload; a `List`/`Map`/nominal state
field kept its source default. Slice 3 restores **composite state** too, so a
hydrated component holds the server's values, not the source defaults:

- `List<T>` ← a JSON array, `Map<Str, V>` ← a JSON object, `Nullable<T>` ←
  `null` or the inner value, and imported nominals ← a JSON object built field
  by field. Restore is **recursive**, so nestings (`List<Card>`, `Map<Str,
  List<Int>>`, `Nullable<Str>`, …) round-trip.
- A field whose JSON does not match its type — or a type that cannot round-trip
  through JSON at all (tuples, functions, a `Map` with a non-`Str` key) — keeps
  its default, exactly as before.

The mechanism is a recursive `json_restore_value` generator that lowers each
`TypeExpr` to an `Option<T_rust>` expression over a `&serde_json::Value`.
**Scalars keep the byte-identical slice-1/2 accessor form**, so primitive-only
hydratable components regenerate unchanged.

**Byte-compat:** of the 20+ view smokes only `hydrate-regions` and
`search-filter` change (both are hydratable with an `items: List<Str>` field —
they now restore it); the scalar-only hydratable crates (`derived` / `hydrate` /
`live-input`) regenerate byte-identical. The `hydrate-regions` example now paints
an `items` server value that **differs** from the component default, so after an
edit the `{#for}` region rebuilds from the restored **server** list — verified
end-to-end in real Chrome (puppeteer): DOM adoption witnesses survive, primitive
+ composite state restore, live keep-node patch, region rebuild from the server
list, zero page errors. The wasm crate compiled with `wasm-pack`. No syntax /
grammar / LSP change — the VSCode extension stays at 0.27.0 (bump deferred to the
11.12 close). Bump 0.30.2 → 0.30.3.

### Remaining slice limitations

- Only **keep-node** components hydrate; composition (`<slot>` / `<Child />`) is
  the next (larger) slice.
- There is still **no isomorphic SSR string renderer** — the marker + state
  contract is validated with hand-authored HTML (as the slice plan intended).
- An **empty** dynamic run in mixed text (`Hi, {name}!` with `name = ""`) merges
  with its neighbours at the server and is not robustly separated — keep dynamic
  runs non-empty, or wait for the isomorphic renderer.

## [v0.30.2] — 2026-08-02 — Phase 11.12 slice 2: hydrate `{#if}`/`{#for}` regions + mixed text

### Added — hydration now adopts control-flow regions and mixed static/interpolated text

Slice 1 hydrated a **region-free** keep-node component whose interpolations
were each the **sole child** of their element. Slice 2 lifts both restrictions,
so the hydration surface covers the common template shapes:

- **`{#if}` / `{#for}` regions.** A keep-node component with regions is now
  hydratable. The server paints each region's content bounded by tagged comment
  anchors (`<!--fr-->` … `<!--/fr-->`); on boot the client-WASM adopts those
  anchors into the same keep-node region handles (`__astart_<r>` / `__aend_<r>`)
  the build walk declares and **leaves the content in place** — no wipe, no
  rebuild. A later state change patches only the region (rebuild between the
  adopted anchors, `__patch_region_<r>`), so the live `<input>` outside it keeps
  its caret. A new `__flv_next_comment` cursor helper adopts the anchors by
  comment data, stepping over the region content (and any interpolation markers
  inside it).

- **Mixed static + interpolated text** (`Hello, {name}!`). The server separates
  the runs with comment markers (`<!--fi-->` … `<!--/fi-->`) so the browser
  paints distinct text nodes; the skip-based adopt walk steps over the markers
  and maps 1:1 — no sole-child wrapper needed.

There is still no isomorphic SSR string renderer, so the marker contract is
validated with hand-authored HTML (as the slice plan intended). Verified
end-to-end in real Chrome (puppeteer): state restore, DOM adoption (JS-property
witnesses survive), region adoption, live mixed-text patch, region rebuild on
change, and reset — zero page errors. The wasm crate compiled with `wasm-pack`.

**Byte-compat:** of the 20+ view smokes only `search-filter` changes (a
value-input component with regions — now hydratable, so it gains the `hydrate()`
surface + the `serde_json` dep); region-free hydratable crates
(`derived` / `hydrate` / `live-input`) regenerate byte-identical (the
region-anchor helper is gated so they carry no unused function). New example
`examples/view/hydrate-regions/` (regions + mixed text over hand-authored server
HTML) + smoke `tests/view_hydrate_regions_wasm_smoke.rs`. No syntax / grammar /
LSP change — the VSCode extension stays at 0.27.0 (bump deferred to the 11.12
close). Bump 0.30.1 → 0.30.2.

### Remaining slice limitations

- Only **keep-node** components hydrate; composition (`<slot>` / `<Child />`) is
  a later slice.
- Only **primitive** state fields restore from the `<script>` payload; a
  `List`/`Map`/nominal keeps its default.
- An **empty** dynamic run in mixed text (`Hi, {name}!` with `name = ""`) merges
  with its neighbours at the server and is not robustly separated — keep dynamic
  runs non-empty, or wait for the isomorphic renderer that will emit the split.

## [v0.30.1] — 2026-08-02 — Phase 11.12 slice 1: SSR → client hydration (adopt the DOM)

### Added — the client-WASM runtime adopts the server-painted DOM instead of re-creating it

Phase 11.12 slice 1 lets a **keep-node, region-free** `.fitzv` component
(a live `@input`/`@change` over a static template) **hydrate**: the
generated `start()` detects that the mount root already holds
server-painted DOM and calls `hydrate(root)` instead of `mount()` (which
wipes + rebuilds). `hydrate()`:

- restores the serialized state from a
  `<script type="application/json" id="__flv_state_<Comp>">` payload (a
  `__NEXT_DATA__`-style contract),
- walks the existing DOM with a DFS cursor (`__flv_next_element` /
  `__flv_next_text`, skipping whitespace/comment nodes) **adopting** each
  node into the same `__ktext_<n>` / `__kattr_<n>` keep-node handles the
  build walk declared — **no `create_element`, no wipe**,
- wires the `@input`/`@click` listeners onto the adopted elements,
- marks the component built so later state changes patch in place.

An **empty** mount root still fresh-mounts (pre-11.12 behaviour), so the
same bundle works as a standalone SPA. Only keep-node components' emitted
code grows; the naive path is byte-identical. The crate pulls `serde_json`
only when it hydrates (and `@rpc` didn't already).

**Verified end-to-end in real Chrome** (puppeteer-core): a JS property set
on the greeting node *before* `init()` survives hydration (DOM adopted,
not recreated); the greeting shows the server value `"Ada"` (state
restored, not the default `"world"`); typing patches it live over the
adopted node; `reset` restores the default — zero page errors. The crate
built to real `.wasm` (`wasm-pack` → `:-) Done`, 54.1 KB).

New runnable example `examples/view/hydrate/` (`App.fitzv` + `index.html`
with server-painted DOM + state script + README) + smoke
`tests/view_hydrate_wasm_smoke.rs`. 18 unit tests (12 codegen_wasm + 6
wasm_build). Of the 20 view smokes, only `live-input` and `derived`
(now hydratable) change their emitted lib.rs; the rest — including
`search-filter` (has regions) — regenerate byte-identical.

**Slice-1 constraint** (→ slice 2): dynamic text interpolations must be
the sole child of their element (`<span>{name}</span>`); mixed text
(`Hello, {name}!`) needs comment markers. Regions (`{#if}`/`{#for}`),
`<Child />` composition, and a real isomorphic server render-to-string
are later slices.

## [v0.30.0] — 2026-08-02 — `@rpc` server functions (client-WASM ↔ server, fullstack)

### Added — call a server function from a `.fitzv` client, tipada, sin plumbing

Phase 11.11 closes the fullstack loop: a `@rpc async fn` declared in a
classic `.fitz` module is callable **directly** from a client-WASM
`.fitzv` component — `let u = get_user(42).await?` — as if it were a
local async call. The compiler generates **both halves** from one
declaration: a `POST /__rpc/<name>` endpoint on the server and a
`fetch`-based stub on the client. No hand-written HTTP handler, no
`fetch`, no JSON glue, no route strings.

```fitz
// api.fitz (classic — has db + auth)
@rpc
async fn get_user(id: Int) -> Result<User> {
  let conn = db.connect(db_url()).await?
  return User.where(fn(u) => u.id == id).first(conn).await
}

// App.fitzv (client-WASM)
from api import get_user, User
event load() {
  let u = get_user(42).await?   // ← round-trip tipado
  user = u
}
```

- **Server half (`11.11.b`, codegen):** an `@rpc` fn is mounted as
  `POST /__rpc/<name>` (single-file and cross-module). The request body
  is a JSON **object** `{param1: v1, ...}`; each param is deserialized
  from its own field via `__FromFitzJson`, the fn runs, and its
  `Result<T>` becomes `200` + JSON (Ok) or `500` + `{"error": ...}`
  (Err) — reusing the whole `@post` wrapper chain (observability,
  panic-catch, etc.).
- **Client half (`11.11.c`, wasm codegen):** an imported `@rpc` fn is
  emitted as an async `fetch` stub (its server-side body is **not**
  transpiled) that serializes the args, POSTs same-origin (the session
  cookie rides along), and maps the reply back to `Result<T, String>`.
  A shared `__fitz_fetch_post` runtime + `serde` derives on the
  wire-crossing nominals are added only when a crate uses `@rpc`.
- **Async event handlers:** a handler whose body `.await`s an `@rpc`
  stub is split into a sync wrapper that `spawn_local`s an owned-`Rc<Self>`
  async worker returning `Result<(), String>` — so `.await?` propagates
  and state updates + a re-render fire when the reply arrives.

### Notes

- **Verified end-to-end in real Chrome** (puppeteer + a same-origin
  static+proxy server): a two-button SPA fetched a primitive
  (`greet` → "Hello, world!") and a nominal (`get_user(42)` → "Ada")
  from the server binary, updated state, and re-rendered — zero page
  errors. The `web` crate compiled to a real `.wasm` via `wasm-pack`
  (`:-) Done`); the server binary answered the exact `/__rpc/*`
  endpoints (curl: 200 / 500 / 400 all correct).
- Runnable example `examples/view/rpc/` (`api.fitz` + `server.fitz` +
  `App.fitzv`) + smoke `tests/view_rpc_wasm_smoke.rs`.
- The checker piece (`11.11.d`) shipped earlier: bare `@rpc`, `async`
  required, `Result<T>` return, mutually exclusive with the other
  handler decorators.
- **MVP scope:** nominals used across the wire must be imported into the
  `.fitzv` (`from api import ..., User`); stacked auth on the generated
  endpoint (`@authenticated`/`@admin`) is a post-MVP refinement; the
  re-render fires once (a mid-request "loading…" flash is a later
  fine-grained-reactivity slice); `Map<K,V>` payloads serialize as pair
  arrays (documented limitation).
- 14 unit tests (server route/coercion/dispatch, client stub/helper/
  spawn_local/serde/Cargo deps) + the example smoke. Suite: lib 3930
  green; fmt + clippy (default + `lsp`) clean; all 17 prior
  `examples/view/` smokes byte-compatible (non-rpc crates unchanged).

## [v0.29.8] — 2026-08-01 — `.fitzv` → wasm: string methods + logical `and`/`or`

### Added — string methods and logical operators on the client-WASM target

The client-WASM emitter now lowers the common `Str` methods and logical
`and`/`or` in general expression position — closing a CW.9 envelope gap.

- **Str methods** (parity with classic Fitz / the SSR target): `.upper()` →
  `to_uppercase()`, `.lower()` → `to_lowercase()`, `.trim()`, `.contains(x)`,
  `.starts_with(x)`, `.ends_with(x)` (all → `bool`), and `.replace(a, b)`. The
  receiver lowers to an owned `String`, so each maps to a `str` method via
  `Deref`. (`.split` / `.to_int`, which return `List` / `Result`, still defer.)
- **Logical `and` / `or`** in expression position (e.g. a `.filter` closure
  body) lower to Rust `&&` / `||`. Condition position (`{#if}` / `if`-expr) was
  already covered by `lower_cond_expr`.

### Notes

- **Unblocks case-insensitive filters** on client-WASM: a
  `names.filter(fn(x) => q == "" or x.lower().contains(q.lower()))` — the exact
  pattern that was SSR-only — now compiles and runs offline. Verified
  end-to-end: a live filter component compiled to a real `.wasm` (`wasm-pack`
  → `:-) Done`), the generated `lib.rs` carrying `.to_lowercase()` /
  `.contains(...)` / `||`. (The live *text* filter input still re-mounts per
  keystroke under naive re-render — the caret caveat from CW.9 — but the filter
  logic itself now runs client-side.)
- 2 unit tests (`wasm_str_methods_lower`, `wasm_case_insensitive_filter_lowers`).
  Suite: lib 3916 green; fmt + clippy clean; all 17 `examples/view/` smokes
  byte-compatible (the change only *adds* support for previously-rejected
  constructs). Bump 0.29.7 → 0.29.8.

## [v0.29.7] — 2026-07-31 — `.fitzv` → wasm: form-control events `@input` / `@change` (CW.9)

### Added — live value binding on the client-WASM target

`@input` / `@change` on a form control now compile with
`fitz build --target wasm-client`. The emitted DOM listener reads the event
target's live value (casting to `HtmlInputElement` / `HtmlSelectElement` /
`HtmlTextAreaElement`) and calls the handler with a payload carrying it under
the `"value"` key:

```fitzv
event on_name() { name = payload["value"] }
...
<input @input="on_name" value="{name}" />
<select @change="on_color">…</select>
```

This closes the form family beyond file input — a text field echoing as you
type, a `<select>` updating on change, a `<textarea>` binding. It matches the
SSR emitter, which already lowers any `@event` to `data-flv-<event>`, so the
same `.fitzv` targets both.

### Notes

- An `@input`/`@change` handler **must** read `payload["value"]` — the value
  is all the event carries; ignoring it is a compile error with a clear
  pointer. `@click` (with or without payload) is unchanged and byte-identical.
- The emitter adds `HtmlInputElement` / `HtmlSelectElement` /
  `HtmlTextAreaElement` to the generated `Cargo.toml` **only** when a
  component uses `@input`/`@change` — value-free crates keep the base set.
- **Caveat (naive re-render):** a state change rebuilds the whole component
  DOM, so a live text `<input>` re-mounts each keystroke — the value re-binds
  via `value="{name}"` but the caret jumps to the end. `<select> @change` is
  unaffected. Fine-grained reactivity is the ROADMAP's next iteration.
- Validated end-to-end in real headless Chrome: `<select> @change` → the
  swatch updates to the picked colour; `<input> @input` → the greeting echoes
  the typed char; no page errors. New example `examples/view/live-input/` +
  smoke `tests/view_live_input_wasm_smoke.rs` (real `wasm-pack` build). 6 unit
  tests; lib 3914 green; fmt + clippy clean; all 17 view smokes green.

## [v0.29.6] — 2026-07-31 — `.fitzv` → wasm: cross-dir / dependency imports (CW.8)

### Added — dep-aware view import resolution on the client-WASM target

A `.fitzv` compiled with `fitz build --target wasm-client` can now import a
component from a `fitz.toml` **dependency**, not just a flat sibling:

```
from fitz_liveviews.ui.Badge import badge as Badge
```

resolves `Badge.fitzv` under the dependency's root (via the `DepRegistry` the
classic loader already builds from the manifest) and inlines it into the
standalone wasm crate. This unblocks an external wasm app consuming the
companion UI as a library, instead of hand-copying components into the app's
own directory.

The four view loaders (`load_imported_components` / `load_imported_fns` /
`load_imported_nominals` / `collect_transitive_view_imports`) gained
`*_with_deps` variants that resolve each import through the new dep-aware
`resolve_view_import`, mirroring the classic `codegen.rs` / `evaluator.rs`
resolution bit-for-bit (single segment → the dep's lib entry; dotted →
the shared `resolve_dep_subpath_file`). The old two-arg signatures stay as
empty-registry wrappers, so the 16 `examples/view/` smokes are byte-identical.

### Notes

- **Framework builtins never trigger a dep load.** A `from fitz_liveviews
  import flv` line (surfaced from inside a dep component) resolves to `None` —
  `flv`/`html`/`raw_html`/`h_join`/`h_when`/`h_either` are emitter special-cases,
  and loading the framework's lib entry would blow the wasm envelope.
- De-dupe now keys on the **resolved file path** (not `path[0]`), so several
  dep sub-path imports sharing a dep name (`dep.ui.Badge` + `dep.ui.Chip`) are
  each scanned.
- **MVP limit** (residual debt): resolution uses a single flat `base_dir` for
  the sibling fallback, so a dep component composing a *bare-name* sibling
  (`from Icon import Icon`, not `from dep.ui.Icon import Icon`) is not resolved
  against the dep's own directory. The dual-target companion primitives are
  leaves, so this doesn't block them.
- Validated end-to-end: an external consumer with a `fitz_liveviews` path dep
  importing `from fitz_liveviews.ui.Badge import badge as Badge` compiled to a
  real `.wasm` (`wasm-pack` → `:-) Done`); the dep component's `Badge` struct +
  scoped styles inlined, `flv(label)` lowered to identity. 8 unit tests; lib
  3909 green; fmt + clippy clean; all 16 view smokes byte-identical.

## [v0.29.5] — 2026-07-31 — `.fitzv` → wasm: helper-style fn/event bodies (for + match + local reassign + string concat)

### Added — richer statement/expression lowering in wasm fn & event bodies

The client-WASM emitter (`fitz build --target wasm-client`) now lowers four
more constructs inside a `.fitzv` event handler or an imported helper `fn`
body — matching what the SSR target already accepted:

- **Range `for` loops** — `for n in 1..(max + 1) { … }` → a Rust
  `for n in 1i64..(…) { … }`.
- **`match` as a value** — `let x = match n == 2 { true => "a", false => "b" }`.
  The scrutinee gets `.as_str()` when any arm pattern is a string literal;
  arm patterns cover Int/Float/Str/Bool literals, an ident binding, and `_`.
- **Local reassignment** — a `let`-then-reassigned local (`let acc = ""` … then
  `acc = acc + x`) now emits `let mut acc` and a plain reassignment, instead of
  a second shadowing `let`. Uses the AST's `is_let` flag; bodies that never
  reassign a local stay byte-identical.
- **String concatenation** — `a + b` where either side is stringy lowers to
  `format!("{}{}", a, b)` (Rust `String + String` is invalid); numeric `+`
  is unchanged.

### Notes

- **No new working showcase component.** This is a language-capability
  addition, not a visual one: helpers that return **HTML as a string** (e.g. a
  star-rating builder assembling `<input …>` markup) still can't render on the
  wasm target — interpolating that string into the DOM produces a text node
  that escapes the markup (the same intrinsic limit as the CW.6 raw-HTML
  helpers). The constructs above work; a helper's *string* output does not
  become live DOM. Rating stays SSR-only.
- 1 unit test (`wasm_fn_body_for_match_reassign_and_string_concat`) exercises
  all four in one event body. Suite: 3901 lib green; fmt + clippy
  (`--all-targets -D warnings`) clean; all 16 `examples/view/` wasm smokes
  regenerate + build (incl. `list-transform`, which reassigns a local).

## [v0.29.4] — 2026-07-31 — `.fitzv` → wasm: mixed attribute interpolation + negative defaults

### Added — mixed attribute interpolation on the client-WASM target

`style="width: {pct}%"`, `class="toast toast-{kind}"` — an attribute whose
value mixes literal text with `{expr}` interpolation segments — now compiles
on `fitz build --target wasm-client` (the SSR target has had it since
v0.28.7). The emitter lowers it to a `set_attribute(name, &format!("…", …))`
that interleaves the literal segments with a `{}` for each interpolated
expr. Full-value interpolation (`attr="{expr}"`) was already supported; this
covers the mixed case. Unblocks `ProgressBar`, `Spinner`, and any component
that computes an inline style.

### Added — negative numeric state-field defaults

`state { progress: Int = -1 }` / `ratio: Float = -0.5` now compile — a
negated numeric default parses as `UnaryOp{Neg, literal}` (not a bare
literal), which the wasm state-default emitter previously rejected.

### Notes

- Both verified end-to-end in a real headless Chrome (`ProgressBar`'s fill
  renders `style="width: 72%"`; `Spinner` mounts with its `-1` default), no
  page errors. Together they close the client-WASM envelope's
  mixed-attribute-interpolation gap and grow the companion-UI showcase.
- 3 unit tests (`mixed_attr_interpolation_lowers_to_format_set_attribute`,
  `negative_numeric_default_emits_negated_literal`, plus the earlier
  `str_comparison_if_condition_lowers_to_string_eq` regression guard). Suite:
  lib 3900/0, fmt + clippy `-D warnings` clean. Bump 0.29.3 → 0.29.4.

## [v0.29.3] — 2026-07-31 — `.fitzv` → wasm: `data-flv-file` (client-side file/image upload)

### Added — `<input type="file" data-flv-file="handler">` on the client-WASM target

A `.fitzv` compiled with `fitz build --target wasm-client` can now read a
picked file entirely client-side — no server, no network. The new
`data-flv-file="handler"` directive on an `<input type="file">` wires a
`change` listener that reads the first selected file via the browser's
`FileReader` (`read_as_data_url`) and calls the event handler with a
payload map:

- `payload["data"]` — the file as a `data:` URL (drop it straight into an
  `<img src="{img}">` for an instant preview),
- `payload["name"]` — the filename,
- `payload["type"]` — the MIME type.

The handler stores what it needs in state and the template renders it. The
emitter adds the `FileReader` / `File` / `FileList` / `Blob` web-sys
features only when a component uses `data-flv-file`, so file-free crates
stay byte-identical. This closes gap #4 (form/file inputs) of the
client-WASM envelope for the read-a-local-file case.

### Notes

- Verified end-to-end in a real headless Chrome: uploading an image to the
  input renders the preview `<img>` with the `data:` URL and the filename,
  no page errors.
- The handler must read `payload` (file selection always carries one); a
  handler that ignores it is a signature mismatch at compile time.
- Reading is async — the `FileReader.onload` closure is `.forget()`-leaked
  to outlive the read, same discipline as the other event closures.
- 2 unit tests (`data_flv_file_*`) in `src/view/codegen_wasm.rs`. Suite:
  lib 3897/0, fmt + clippy `-D warnings` clean.

## [v0.29.2] — 2026-07-30 — `.fitzv` → wasm: `flv` passthrough (dual-target SSR↔client-WASM)

### Added — el emisor client-WASM tolera el helper de escaping `flv` de fitz-liveviews

Un componente `.fitzv` autoreado en el estilo SSR de fitz-liveviews
—`{flv(label)}` + `from fitz_liveviews import flv`— ahora compila a
`fitz build --target wasm-client` **sin modificarse**. `flv(s: Str) ->
Str` escapa HTML para el string-builder del SSR; en el target
client-WASM un `create_text_node` / `set_attribute` escapa
intrínsecamente, así que `flv` es la **identidad**: el emisor
(`src/view/codegen_wasm.rs`, `lower_call`) lowerea `flv(x)` → `x`, con
salida byte-idéntica a `{x}`. Esto destraba compartir UN source `.fitzv`
entre el target SSR y el client-WASM para el subset presentacional de la
companion UI de fitz-liveviews (research CW.6 de ese repo).

### Guardrail — los helpers raw-HTML hard-errorean como SSR-only

`html` / `raw_html` / `h_join` / `h_when` / `h_either` inyectan markup
deliberadamente sin escapar o pliegan `List<Html>`; tratarlos como
identidad renderizaría el markup como texto escapado (bug silencioso).
El emisor client-WASM ahora **rechaza** con un mensaje claro que los
nombra SSR-only, en lugar de emitir una llamada rota.

### Notas

- **Validado end-to-end**: los componentes SSR `Badge.fitzv` y
  `Chip.fitzv` de la lib fitz-liveviews compilan sin cambios a un
  `.wasm` real (`wasm-pack` → `:-) Done`), con `{flv(label)}` bajando a
  `format!("{}", (*self.label.borrow()))` — cero llamadas a `flv` en el
  bundle.
- **Sin sintaxis nueva, sin cambio de gramática ni LSP.** Cambio interno
  del emisor view (`.fitzv` → wasm). Los ejemplos `examples/view/*`
  (standalone, no usan `flv`) regeneran byte-a-byte. La extensión VSCode
  no se rebuildeó (view-codegen-only, sin impacto en grammar/LSP —
  convención v0.28.1+).
- 2 unit tests nuevos `cw6_*` en `src/view/codegen_wasm.rs`. Suite: lib
  3895/0 (default) + 4055/0 (`--features lsp`), fmt + clippy limpios.

## [v0.29.1] — 2026-07-30 — `.fitzv`: azúcar `{#for x in xs key=x.id}` para keyed diffing

### Added — cláusula `key=<expr>` en `{#for}` de single-file components

Un `{#for}` en un `.fitzv` acepta una cláusula opcional `key=<expr>`
(`{#for row in rows key=row.id}`). Desugarea en `expand` a un atributo
`data-flv-key="{<expr>}"` sobre el **único elemento raíz** del cuerpo del
loop. El motor de keyed diffing de fitz-liveviews (v0.16.0) ya consume
`data-flv-key` para reconciliar listas por identidad (LCS →
`insert_keyed`/`move_keyed`/`remove_keyed`) en vez de por posición — el
keyed diffing deja de necesitar el atributo escrito a mano.

- El `<expr>` se evalúa con la variable del loop en scope, se type-chequea y
  se emite como cualquier interpolación del cuerpo.
- El cuerpo del loop debe tener exactamente un elemento raíz que cargue la
  `key`; cero (solo texto/interpolación o un `<Child />`) o más de uno → error
  claro del expander.
- **Byte-for-byte** para un `{#for}` sin `key=` (los SFC existentes no cambian).
- Target SSR emite `data-flv-key="{<expr>}"`; target WASM lo setea como
  atributo DOM normal (paridad). Validado `fitz run` ↔ binario nativo
  (`<li data-flv-key="...">` idéntico).
- Parser: nuevo split `key=` (con tracking de brackets/paren/brace/strings para
  no confundir un `key=` dentro del iterable). AST: `TemplateNode::For.key_raw:
  Option<String>`. Sin cambios al grammar de la extensión VSCode (`.fitzv` no
  está registrado — convención view-only). +17 tests (parser split + parse +
  expand inject/errors + SSR emit + WASM parity).

## [v0.29.0] — 2026-07-28 — Librerías de componentes por dep-subpath: imports punteados + presupuesto de recursos + fixes de paridad

Reúne el trabajo de core que habilita **librerías de componentes `.fitzv`
distribuibles como dependencia** (la companion UI library de fitz-liveviews):
imports por sub-path punteado, presupuesto de recursos para `fitz test`/`repl`,
y tres fixes que cierran la paridad `fitz run` ↔ `fitz build` end-to-end.

### Added — imports por sub-path punteado dentro de una dependencia

`from <dep>.<sub>.<Mod> import X` resuelve `<Mod>` (`.fitz` o `.fitzv`) bajo la
raíz de la dependencia, no relativo al importador. Habilita consumir una
librería organizada en sub-módulos (`from fitz_liveviews.ui.Pager import
pager`). Paridad `fitz run` ↔ `fitz build` (el codegen emite el módulo de
dep-subpath como un módulo Rust flat saneado). (commit `0e58213`)

### Added — presupuesto de recursos para `fitz test` / `fitz repl`

`fitz test` y `fitz repl` cortan con un error claro al exceder un límite de
pasos o de profundidad de recursión, en vez de colgar el proceso ante un loop
o recursión infinita en código bajo test. (commit `f23777c`)

### Fixed — paridad de las librerías de componentes por dep-subpath

- **codegen** — los imports relativos **transitivos** de un módulo dep-subpath
  ahora resuelven contra el directorio del propio módulo, no contra el
  `base_dir` global del proyecto importador. Sin esto, un `Comp.fitzv` que hace
  `from helpers import x` rompía `fitz build` con `module 'helpers' not found`
  (buscaba `helpers` bajo el src del importador). `fitz run` ya resolvía bien —
  cierra una brecha de paridad run/build. Test:
  `cli_e2e::build_resolves_dotted_dep_subpath_with_transitive_import`.
- **loader/checker** — `fitz run` y `fitz build` pasan el `dep_registry` al
  checker, para que el pre-scan de `@live_component` resuelva imports
  dep-subpath y la auto-registración (`flv_register` inyectado) funcione para
  componentes importados de una dependencia. Sin esto el runtime fallaba con
  `key not found in map: <componente>`.
- **runtime** — el stack de los workers de tokio del servidor HTTP pasa a 16 MB.
  El evaluador tree-walking `#[async_recursion]` consume un frame grande por
  cada llamada Fitz; renderizar una página real (un data-grid con filas
  anidadas + componentes compuestos por WebSocket) overfloweaba el default de
  2 MB. Solo afecta al intérprete — el binario compilado usa frames mucho más
  chicos y anda a 2 MB.

Release de codegen/loader/runtime — sin cambios al LSP ni al grammar, así que la
extensión VSCode del core no se rebuildeó.

## [v0.28.8] — 2026-07-25 — fix: scoped styling de un `class` mixto en `.fitzv`

Cierra una deuda latente introducida por v0.28.7 (mixed attribute
interpolation), descubierta al escribir el `<style scoped>` del Pager del
Admin ABM.

### Fixed — un `class` mixto en un componente con `<style scoped>`

El rewrite de scope de v0.28.7 (`rewrite_class_attrs_in_template`) appendeaba
la scope class a un `class` mixto (`class="badge badge-{kind}"`) como un token
**separado** (` <scope>`), en vez de **sufijar** cada token de clase literal
completo (`badge` → `badge-<scope>`) como hace `rewrite_class_value` para las
clases estáticas. Resultado: en un componente con `<style scoped>`, la parte
estática de un `class` mixto no recibía el estilo scopeado (la scope class
suelta no matcheaba ningún selector). Latente — ningún componente combinaba
`class` mixto con `<style scoped>` (el Pager scopeado usa clases estáticas; el
Toast usa `class` mixto pero CSS global), así que no había síntoma visible.

- Nuevo helper `pure_literal_class_tokens`: extrae los tokens de clase
  puramente literales (bounded por whitespace) de los segmentos del valor
  mixto. Un token pegado a un `{expr}` (`badge-{kind}`) o producido por un expr
  es un valor runtime que no se puede scopear → se excluye. Los tokens puros se
  sufijan con la scope y se appendean; los originales quedan en su lugar.
- Test nuevo `mixed_class_in_scoped_component_suffixes_only_pure_tokens`.
  Byte-a-byte para todo componente que no combine `class` mixto con scoped.

## [v0.28.7] — 2026-07-25 — mixed attribute interpolation en `.fitzv` + retry en la copia del binario

Dos cambios de core surgidos del refactor a LiveComponents del Admin ABM de
fitz-liveviews (slice Toast) y de la familia de flakes de test en Windows.

### Added — mixed attribute interpolation en templates `.fitzv`

El emitter SSR sólo reescribía interpolaciones de atributo de **valor completo**
(`attr="{field}"`); un valor **mixto** (`class="toast toast-{kind}"`,
`style="width: {pct}%"`) caía al path Static y se emitía verbatim, dejando
`{kind}` como una interpolación classic-Fitz de una variable inexistente →
`fitz build` fallaba con `unknown variable kind` (`fitz check`, gradual, lo
aceptaba, así que sólo aparecía al compilar). Las interpolaciones de **texto**
puras (`<span>{msg}</span>`) sí se reescribían — la asimetría rompía cualquier
SFC que interpole state en un atributo (Vue/Svelte/JSX lo soportan).

- Nueva variante `ExpandedAttr::MixedInterpolation` con segmentos
  `Literal`/`Expr`. `expand` detecta los segmentos `{...}` en atributos Static
  (balancea llaves, respeta `\{`/`\}`), parsea cada expr, y el emitter SSR los
  reescribe a `state.<field>` con el mismo walker de scoping que texto e
  interpolación completa. El checker type-chequea los segmentos; el rewrite de
  `<style scoped>` appendea la scope class a un `class` mixto. El target
  client-WASM difiere con error claro (usar interpolación completa ahí).
- Byte-a-byte para los `.fitzv` sin atributos mixtos. 9 tests nuevos (6 emit
  SSR + 3 expand).

### Fixed — retry con backoff en la copia del binario (Windows os error 32)

En Windows el linker/antivirus/indexador puede retener un file handle sobre
`target/release/<stem>.exe` recién linkeado un instante después del build, y un
proceso previo que aún sale puede retener el destino. Una `fs::copy` bare
fallaba con os error 32 (`ERROR_SHARING_VIOLATION`), que se manifestaba como
flakes de test (`hidden_decorator`, `handler_panic_r6`) desde que T2 (v0.10.13)
quitó el mutex `SERIAL` y los builds E2E pasaron a correr en paralelo.

- Nuevo helper `copy_binary_with_retry`: reintenta hasta 8 veces con backoff
  creciente (25ms × intento) sólo ante os error 32; no-op en el happy path.
  Wired en los dos sitios de copia (`build_file` + `build_file_with_bundle`).
- Al eliminar el flake, `hidden_decorator` llegaba por primera vez a su tercer
  assert, que quedó stale tras la traducción ES→EN (v0.16.0): el runtime emite
  `"undeclared field"` pero el test chequeaba `"no declarado"`. Assert corregido.

## [v0.28.6] — 2026-07-24 — import de `__FitzValue` por coerción en módulos (W27): el refactor a LiveComponents del Admin ABM compila a binario

Cierra la deuda **W27**, descubierta en el primer slice del refactor a
LiveComponents del Admin ABM de fitz-liveviews (ConfirmDialog como `.fitzv` con
instancias per-connection). Un módulo que pasa una instancia nominal a un param
`Any` de una fn importada (`component_with(name, id, initial: Any)`) o baja un
retorno `Any` a nominal con anotación (`let st: confirm_dialog =
component_state(...)`) emitía `__FitzValue::Instance`/`__fv_type_name` sin el
`use crate::__FitzValue;` correspondiente → E0433/E0425 en `fitz build`.

### Fixed — gate W23 no cubría `__FitzValue` emitido por coerción

- El gate de v0.28.1 (W23) emite el import cuando el módulo **declara** shapes
  `Any` (`program_uses_fitz_value`) o importa un `@table` jsonb. La coerción
  Instance→`Any` (args de fns importadas) y el downcast `Any`→nominal (let
  anotado) emiten `__FitzValue` sin ninguna declaración detectable en el AST.
- **Fix**: post-scan del Rust generado del módulo — si referencia `__FitzValue`
  y el import falta, se inserta tras el header (`use std::sync::{Arc, Mutex};`)
  con `#[allow(unused_imports)]`; el flag transitivo `module_uses_fitz_value`
  hace OR con el scan para que el crate root emita el `enum __FitzValue`.
- Test nuevo: `compile_e2e::cross_module_any_coercions_emit_fitz_value_import_w27`.
- Validado end-to-end sobre el Admin ABM: build verde + smoke WS 10/10 con
  paridad exacta ante `fitz run` (uuid per-connection, ask/cancel/confirm,
  delete real en Postgres).
- Convenciones confirmadas de paso (no son bugs; detalle en
  `docs/deudas-post-5b.md` → W27): `Uuid.v4().to_str()` para instance ids
  `Str`; el entry file importa `Html` cuando registra SFCs; la
  auto-registración §9.bb pre-scanea solo los imports DIRECTOS del entry.

## [v0.28.5] — 2026-07-23 — nominal transitivo en defaults cross-module (W26): SFCs con `List<Member>` importado compilan a binario

Cierra la deuda **W26** abierta en v0.28.4: un `.fitzv` con
`state { members: List<Member> = [Member { ... }] }` donde `Member` viene de un
`.fitz` hermano ya compila con `fitz build` y corre con paridad ante `fitz run`.
Era el patrón del `examples/course/c3-team-panel-sfc` del curso de fitz-liveviews.
Con esto, las LiveComponents con nominales importados en el estado quedan
completas end-to-end (core en v0.28.3, imports al head en v0.28.4, defaults
transitivos acá).

### Fixed — `impl __FromFitzJson` cross-module inlineaba defaults con nominales transitivos

- El `impl __FromFitzJson for <T>Data` que main.rs emite para tipos de módulos
  cargados (`emit_helpers_for_imported_types`, cuando hay HTTP) inlineaba el
  default expr de cada field ausente vía `gen_expr` **en el ctx de main**. Si el
  default instancia un nominal **transitivo** (importado por el módulo que define
  el tipo, no por main — ej. `members: List<Member> = [Member { ... }]`), el
  build abortaba con `unknown type \`Member\` in codegen`.
- **Fix**: con contexto cross-module (`xmod = Some`, plumbing de W19), los dos
  sitios que materializan defaults (arm `None =>` del field ausente + init de
  fields `@hidden`) delegan al helper `<mod>::__default_<T>_<field>()` que
  PreF8.3 ya emite en el módulo definidor — mismo patrón que el struct lit de
  tipos importados en `gen_struct_lit`. Tipos locales siguen inlineando igual.
- Validado end-to-end sobre el c3-team-panel-sfc: build verde, `GET /` con el
  render inicial correcto, toggle por WS con paridad exacta ante `fitz run`
  (el frame sin `html`/`patches` ejercita en runtime el nuevo path
  `fitz_liveviews::__default_LiveFrame_patches()`).
- Test nuevo:
  `compile_e2e::cross_module_default_with_transitive_nominal_delegates_to_module_helper_w26`.

### Deuda residual derivada

- El path Python espejo (`py_field_extract_code`) tiene el mismo patrón latente
  (inline `gen_expr` del default) — solo dispara con `uses_python` + coerción
  PyDict→Instance de tipos importados con defaults transitivos; misma delegación
  si aparece el caso real. Bonus caracterizado: `return Panel {}` (struct lit
  vacío en return position) parsea como Ident + bloque — gap de parser menor,
  separado. Detalle en `docs/deudas-post-5b.md` → "W26".

## [v0.28.4] — 2026-07-23 — `.fitzv` SSR: imports del usuario al head del módulo

Fix parcial hacia "SFCs con nominales importados en el estado compilan a binario"
(seguimiento de v0.28.3). Descubierto escribiendo el cap C5 del curso de
fitz-liveviews. La pieza que falta (registro del nominal transitivo en main) queda
como deuda **W26**, a cerrar en una tanda dedicada.

### Fixed — el emisor SSR emitía los imports del usuario mid-file

- El emisor SSR de `.fitzv` (`src/view/codegen_ssr.rs`) emitía el helper
  `__fitz_view_str_join` (una `fn`) ANTES de los imports del usuario (`from X
  import Y`), dejando esos imports **en medio del archivo** — clásico Fitz exige
  todo `import` / `from ... import` en el head, antes de cualquier `fn` / `type`.
  Un `.fitzv` cuyo estado referencia un nominal de un `.fitz` hermano
  (`state { xs: List<Member> }`) generaba classic Fitz con el import de `Member`
  mal ubicado.
- **Fix**: `emit_module_header` deja solo el import de framework al head;
  `emit_user_imports` va justo después; y el helper `__fitz_view_str_join` se
  emite en una fn nueva `emit_str_join_helper` **después de todos los imports**.
  View tests 576/0.

### Nota — el gap de `.raw over Any` era user-code, no core

- El "field access `.raw` over `Any`" que aparecía en SFCs con estado `List` NO
  era un bug del compilador: faltaba importar `Html` en el `main.fitz` del
  proyecto. Sin `Html` importado, el retorno de `component(...) -> Html` no
  resolvía y caía a `Any`. Los ejemplos del curso (c2/c3 SFC) suman el import.

### Deuda W26 (v0.28.5) — nominal importado en estado `@live_component`

- Un `.fitzv` con `state { xs: List<Member> }` donde `Member` viene de un `.fitz`
  hermano todavía no compila a binario: el codegen de main gen'ea `Member { ... }`
  (el nominal transitivo del default del estado del componente importado) sin
  registrarlo en su `type_sigs`, justo tras enriquecer el tipo importado del
  componente en `pre_register_types`. El caso plano equivalente (`from models
  import Outer` con `Outer { items: List<Nested> }`, Nested del mismo módulo) sí
  compila — la diferencia es que el nominal del default es **transitivo**. Detalle
  en `docs/deudas-post-5b.md` → "W26".

## [v0.28.3] — 2026-07-23 — LiveComponents compilan a binario nativo (`fitz build`)

Dos fixes de codegen coordinados que destraban las **LiveComponents** de
fitz-liveviews (`@live_component` / `component()` / `flv_register` /
`dispatch_component_events`) en `fitz build`. Antes andaban solo en `fitz run`
(intérprete); ahora compilan a binario nativo con paridad bit-a-bit. Descubierto
escribiendo el cap C5 del curso (dashboard de tiles con estado per-instancia).

### Fixed — `flv_register(...)` con map de funciones no compilaba

- El `flv_register("Name", state, render_fn, {"event": handler, ...})`
  auto-inyectado pasa un `Map<Str, Any>` de funciones. El LUB del map literal lo
  tipaba `Map<Str, Function>`, y al coaccionarlo al parámetro `Map<Str, Any>` el
  codegen no tenía rama `(Map, Map)` → dejaba los `Arc<dyn Fn>` crudos → E0308
  (`expected __FitzValue, found Arc<...>`).
- **Fix**: nuevas ramas `coerce` `(Map<K,Function> → Map<K,Any>)` y
  `(List<Function> → List<Any>)` que reconstruyen el contenedor envolviendo cada
  función en un `__FitzValue::Function(...)` adapter (vía `wrap_fn_as_any`, que
  ya existía) — marshalea `Vec<__FitzValue>` ↔ tipos concretos por la firma de la
  fn. La key también se coacciona a `__FitzValue` (el rep Rust de `Map<K, Any>`
  es `Vec<(__FitzValue, __FitzValue)>`). Guard estrecho: solo cuando el valor es
  una `Function` concreta ensanchando a `Any` (el caso de `flv_register`, que
  vive en main.rs donde los helpers `__fv_*` existen), para no sobre-disparar en
  las coerciones internas del propio módulo lib.

### Fixed — globals de módulo mutables perdían el estado

- Un `let X = {}` a nivel de módulo (Map/List/Nominal mutable) se emitía como
  `pub fn X() -> T { <default fresco> }` — devolvía un valor **nuevo vacío** en
  cada llamada. El estado escrito a un global de módulo se perdía silenciosamente
  entre llamadas y entre threads worker de HTTP. Esto rompía el
  `COMPONENT_REGISTRY` / `COMPONENT_STATE_STORE` de fitz-liveviews (`flv_register`
  poblaba una instancia, `component()` leía otra vacía → "key not found in map").
- **Fix**: los globals de módulo de tipo referencia (List/Map/Nominal, rep Rust
  `Arc<Mutex<...>>`) se emiten como un `LazyLock<Arc<Mutex<T>>>` compartido + un
  getter que clona el Arc (misma instancia). Espejo de cómo el main emite el
  state HTTP (F17.4b). Alinea `fitz build` con `fitz run` (un global de módulo es
  UNA instancia compartida). Primitivos siguen por los paths const/static.

### Validado

- SFC LiveComponent compila a binario + **paridad run↔binario**: counter (C1) y
  el dashboard de tiles (C5, multi-instancia) buildean y corren idénticos
  (aislamiento per-instancia: downloads=2, signups=1, errors=0 en el binario).
- Tests nuevos: `compile_e2e::coerce_map_of_functions_to_map_any_wraps_in_fitzvalue_function_w25`,
  `compile_e2e::module_mutable_global_persists_state_via_shared_lazylock_w25`,
  `codegen::tests::module_let_map_top_level_emits_shared_lazylock_global_w25`.
- **Deuda residual** (v0.28.4+): SFCs con estado `List` (`.raw` sobre `Any` en el
  render de listas del emisor SSR) y con nominales importados (`unknown type X in
  codegen`) pegan gaps de codegen SSR→binario adicionales — el core (registro +
  dispatch + estado per-instancia) ya compila.

## [v0.28.2] — 2026-07-23 — `.fitzv`: operadores de comparación `==` `!=` `<=` `>=` en event bodies

Fix del view lexer de `.fitzv` (single-file components) descubierto escribiendo
el curso de fitz-liveviews. Sin sintaxis nueva; cierra un gap real del round-trip
de operadores.

### Fixed — comparaciones multi-char rotas dentro de `.fitzv`

- El view lexer tokenizaba los operadores char por char, y
  `capture_balanced_body_raw` reconstruía el event body con espaciado que rompía
  las comparaciones multi-char: `==` se reconstruía como `= =`, `>=` como
  `> =`, y `!=` **ni siquiera lexeaba** (`!` era "unexpected character"). El
  resultado: `event go() { if (x != y) { ... } }` fallaba con "view parse error"
  / "expected `)`" al correr o compilar (aunque `fitz check` no lo cazaba).
- **Fix**: el view lexer ahora tokeniza `==`, `!=`, `<=`, `>=` como **tokens
  únicos** (`EqEq`/`Neq`/`Le`/`Ge`), y `append_token_source` los re-emite
  verbatim. Un `!` solo sigue siendo error, espejando el lexer clásico
  (`!` solo vale como parte de `!=`; la negación lógica se escribe `not`). Los
  genéricos (`List<Str>`) no se ven afectados — el `<` de un genérico siempre va
  seguido de una letra, nunca de `=`.
- Tests nuevos: `view::lexer::tests::tokenizes_comparison_operators_as_single_tokens_v0_28_2`,
  `lone_bang_is_a_lex_error_v0_28_2`, `generic_lt_still_distinct_from_le_v0_28_2`.
  Validado end-to-end: un `.fitzv` con `payload["v"] == "yes"` y `!= "yes"` en un
  event handler muta el estado correctamente sobre el WebSocket.

## [v0.28.1] — 2026-07-22 — codegen: `use crate::__FitzValue` independiente del prelude DB

Fix de codegen de **paridad `run`↔`build`** descubierto construyendo el
component gallery de fitz-liveviews (ejemplo del paquete de adopción de la
Companion UI). Sin sintaxis nueva, sin cambio de comportamiento del lenguaje.

### Fixed — módulo con campo `Any` (sin DB) no importaba `__FitzValue`

- Un módulo que declara un `type X { f: Any }` (o `Map<Str, Any>` / `List<Any>`)
  emite ese field con el tipo Rust `__FitzValue`, pero el
  `use crate::__FitzValue;` del módulo vivía **dentro** del bloque del prelude
  DB. Un módulo que referenciaba `__FitzValue` **sin usar Postgres** nunca
  emitía el import → `fitz build` rompía con `cannot find type __FitzValue in
  this scope`. El caso canónico es el `ComponentReg { render_fn: Any, ... }` de
  la lib `fitz_liveviews` cuando lo consume un programa sin ORM (el Admin ABM
  compilaba solo porque su ORM activa `__FitzValue` globalmente).
- **Fix**: el `use crate::__FitzValue;` (y el helper `__fv_type_name`, usado al
  invocar un valor `Any` como callable) se emiten en un bloque **independiente**
  del prelude DB, gobernado por `program_uses_fitz_value(program)`. Los helpers
  JSONB (`__fitz_jsonb_to_fitz_value` / `__fitz_fitz_value_to_jsonb`), que sí son
  de DB, quedan gated como antes. El crate root ya emitía el `enum __FitzValue`
  cuando cualquier módulo lo necesita; el bug era solo el import del módulo.
- Test E2E nuevo `cross_module_any_field_without_db_emits_fitz_value_import_w23`.
  Validado end-to-end: `examples/gallery` de fitz-liveviews compila a binario
  nativo con paridad WS bit-a-bit ante `fitz run`.

## [v0.28.0] — 2026-07-22 — scoping de `let` + `@header` sobre `@ws` (dogfood i18n del Admin ABM)

Dos fixes de core que salieron internacionalizando el showcase Admin ABM de
fitz-liveviews. Sin sintaxis nueva; el AST gana un campo interno.

### Fixed — `let` local que shadowea un import / param (scoping)

- Antes, un `let x = ...` local dentro de una fn PISABA un nombre importado o un
  param del mismo nombre en vez de crear un binding local que lo shadowea. El
  checker no lo cazaba (o lo rechazaba como reasignación incompatible), y el
  codegen mis-infería el tipo (ej: `let cookie = "abc"` con param `cookie: Str?`
  → `Option<String>` → `fitz build` rompía con E0308).
- **La solución**: `Stmt::Assign` gana `is_let: bool`. El parser lo setea —
  `let x` / `x: T =` (declaración) → `true`; `x =` / `x += ` (reasignación) →
  `false`. Una declaración SIEMPRE define un binding fresco en el scope actual
  (shadow), nunca reasigna un import/param/fn de arriba; una reasignación camina
  hacia arriba a reasignar el binding existente. Aplicado en evaluator, checker
  y codegen, con tests. Sin cambio de comportamiento para código que no
  shadowea.

### Fixed — `@header(...)` sobre `@ws` (contexto del handshake)

- Un handler `@ws` ahora puede leer headers del handshake:
  `@header(name="cookie") @ws("/live/x") async fn sock(ws: WsConn<T>, cookie: Str?)`.
  Antes el runtime lo rechazaba (aunque el checker lo aceptaba — mismatch). El
  WS upgrade ES un request HTTP, así que sus headers (cookies incluidas) ahora
  se bindean al param como en los handlers HTTP: nullable ausente → Null,
  requerido ausente → cierra la conn (runtime) / 400 pre-upgrade (binario).
  Paridad `fitz run` ↔ `fitz build` validada. Destraba pasar contexto
  per-conexión (locale, tenant) a un LiveView sin workarounds del lado cliente.
- Queda abierto (menor): query/path params en el path de `@ws` (sigue exigiendo
  un `Str` literal).

## [v0.27.0] — 2026-07-22 — `str.to_int()` + dynamic `.order_by(ascending:)` (Admin ABM Slice 3 dogfood)

**Minor release** — one new builtin (`str.to_int()`) plus an ORM
`run`↔`build` parity fix. Both surfaced while building the Admin ABM
showcase's Slice 3 (a live DataGrid with search + filters + sort +
numbered pages), fixed in the core language rather than worked around.

### Added — `str.to_int() -> Result<Int>` (W22)

- Parses a string (trimmed) into a signed 64-bit integer; `Err(Str)` when
  it isn't a valid integer. Enables turning string payloads into `Int`
  (e.g. a WebSocket event carrying a page number or a filter id from a
  `Map<Str, Str>` payload). Wired through the evaluator, the checker
  (`Result<Int>`), codegen (`.trim().parse::<i64>()` → `Result<i64,
  String>`, so `?`/`match` work), and LSP completions. Unblocks the grid's
  numbered pages and departamento filter.

### Fixed — dynamic `.order_by(closure, ascending: <Bool>)` in `fitz build` (W21)

- `fitz build` rejected `.order_by(fn(e) => e.field, ascending: <var>)`
  when the direction was a runtime `Bool` (it required a literal — the SQL
  direction was baked at compile time), while `fitz run` always accepted
  it. Codegen now passes the descending direction as a runtime expression
  `!(<ascending>)` to the QueryBuilder's `with_order_by(col, descending)`
  (which already took a runtime bool). A Bool literal still bakes
  `true`/`false`; `-u.field` DESC is unchanged; full-text `.rank(...)`
  still needs a literal direction.

### Tests

- `+4` (2 for `str.to_int` across evaluator/codegen, 2 for dynamic
  `ascending:`). lib 3852/0, fmt + clippy clean. The Admin ABM grid (search
  + estado/departamento filters + dynamic sort + numbered pages) compiles
  to a native binary with parity vs `fitz run` (validated against a real
  Postgres).

## [v0.26.1] — 2026-07-21 — Cross-module `List<Nominal>`: WS deser + var inference without importing the nested type

**Patch release** — a codegen `run`↔`build` parity fix. No new syntax,
no grammar/LSP change; `.vsix` regenerated only for version parity.

A `@ws` handler with `WsConn<T>` where `T` lives in a submodule (or a
dep) and has a compound field `List<Nominal>` whose inner nominal is not
imported into main used to hang `ws.recv()` in the native binary while
`fitz run` worked. Two coordinated codegen fixes close it and, as a
bonus, remove the "import every nested type" workaround.

### What changed

- **W19 — real cross-module `__FromFitzJson` instead of a stub.** When
  an imported type's `List<Nominal>` field degraded because the inner
  nominal wasn't in main's env, `impl __FromFitzJson for <T>Data` was
  emitted as a stub returning `Err(...)`. At runtime a `WsConn<T>.recv()`
  over such a type deserialized to that `Err` → the handler ended → the
  connection cleanup blocked on the heartbeat-held writer (half-open
  socket) → the WS client hung. Fix: the nested nominals are already in
  scope in main.rs (`use <mod>::{Name, NameData};` + their real
  `__FromFitzJson` impl), so the body is now emitted with the concrete
  nested type (`Arc<Mutex<Vec<Thing>>>`). The stub is kept only for a
  nominal not defined in any loaded module (genuinely opaque).

- **W20 — infer the concrete type for a `let` bound to an imported fn.**
  `let x = imported_fn()` where `imported_fn` returns `List<Nominal>`
  (nominal not imported) inferred `Vec<__FitzValue>` and failed rustc
  when `__FitzValue` wasn't activated. Fix: omit the type annotation for
  such bindings (concretely-typed accessor RHS) and let Rust infer the
  concrete type from the imported fn's signature.

Together they remove the workaround where the Admin ABM showcase imported
`Patch` only so `WsConn<LiveFrame>` (`LiveFrame.patches: List<Patch>`)
and `let patches = diff_html(...)` would type. Verified end-to-end: the
showcase compiles to a native binary and the WS round-trips **with and
without** the `Patch` import.

### Tests

- `+2` E2E in `tests/compile_e2e.rs` (`ws_cross_module_..._not_stub_w19`,
  `w20_imported_fn_returning_list_nominal_infers_type_...`). lib 3849/0,
  fmt + clippy clean, guide smoke green.

## [v0.26.0] — 2026-07-20 — Cross-file composition refinements: aliasing + transitivity + LSP

**Minor release** — three refinements that finish the cross-file
`<Child />` surface shipped in v0.25.0. WASM-only for the loader/emitter
changes; the LSP change makes editing a `.fitzv` cross-file-aware. Classic
Fitz and the classic `fitz build` path are untouched, and the v0.25.0
same-file view examples regenerate byte-for-byte.

### What changed

- **Aliasing** — `from Card import Card as Row` now resolves `<Row />`.
  `view::load_imported_components` registers a renamed clone under the
  alias (keeping the original name too, so a component's own file-local
  siblings that compose it by its real name still work). Only the
  components reachable from the parent's `<Child />` refs are emitted, so
  the unreached original is not double-emitted next to the alias.
- **Transitivity** — the parent no longer has to import every grandchild
  by hand. New `view::collect_transitive_view_imports` walks the `.fitzv`
  import graph (cycle-safe, one file per step) and feeds the transitive
  union to the three loaders, so a component / nominal / helper `fn` that
  lives in a file the entry does not import directly is discovered.
- **LSP cross-file** — editing a `.fitzv` in VSCode no longer flags a
  cross-file `<Child />` as unknown. The bin derives the document's
  directory from its URI and the new
  `lsp::check_view_source_with_base_dir` loads the imported sibling
  components (over the transitive union, honouring aliases) before running
  `check_with_imported_components` — the structural parallel of the
  classic `.fitz` cross-module pre-scan (v0.19.3). Falls back to
  single-file when there is no file context.

### Example

- `examples/view/cross-file-transitive/` — `App` imports `Card as Row`
  (alias) and composes `<Row />`; `Card` composes `<Badge />` from a third
  file `App` never imports (transitive). Compiles to real WASM (~35 KB).

### Debt closed

- `docs/deudas-post-5b.md` — the three v0.25.0 residual debts (component
  aliasing, one-level transitivity, LSP cross-file) are closed.

## [v0.25.0] — 2026-07-19 — Cross-file `<Child />` composition on the WASM target

**Minor release** — a `<Child />` can now live in a SEPARATE `.fitzv`
file, imported with `from Card import Card`. Before this, `<Child />` was
same-file only (the workaround was the runtime `component("Name", "id")`
API of `fitz-liveviews`). This is the last open piece of the WASM
composition surface: props flow down, events bubble up, and slots fill —
all across a file boundary. WASM-only; classic Fitz and the classic
`fitz build` path are untouched.

### What changed

- `fitz build --target wasm-client` loads every component declared in
  each imported sibling `.fitzv` (new `view::load_imported_components`,
  parallel to the nominal / fn loaders) and inlines its **whole** emit —
  struct + `new` + event handlers + render + `<style scoped>` — into the
  one generated crate.
- The emitter merges the *reachable* imported components (the transitive
  closure of `<Child />` refs from the local components) ahead of the
  local ones into a single synthetic file, so every existing pass —
  bubbled-event collection, per-component emit, and same-file child
  resolution — treats the cross-file child as if it were local.
- The view checker validates the parent's `<Child />` composition (prop
  existence + type, `@event` binding, slot fill) against the imported
  child's **real surface** instead of reporting an unknown component (new
  `view::check_with_imported_components`).
- Each imported component brings its own scoped-style hash
  (`FNV-1a(name::css)`), baked at its own file's expand — so the parent's
  and the child's scoped styles never collide.

### Scope

- Touches checker + emitter + loader + CLI (`src/view/{check,
  codegen_wasm,wasm_build}.rs` + `src/main.rs`). When no cross-file child
  is composed the merge is a structural clone, so the emit and Cargo.toml
  stay byte-for-byte identical — the eight same-file view examples
  regenerate unchanged.
- New example `examples/view/cross-file-child/` (`App.fitzv` imports
  `Card.fitzv` with a static prop, a bubbled `@like`, and named + default
  slots) compiles to real WASM (36.2 KB raw).
- **MVP limits**: one level deep (an imported `.fitzv`'s own imports are
  not loaded transitively — the parent imports everything any child
  needs; the child's file-local siblings ARE available); local wins on a
  name collision; no component aliasing (`from Card import Card as Row`
  registers under the original name); the SSR path still uses the runtime
  `component(...)` API; LSP over a single `.fitzv` still flags a
  cross-file child as unknown (Phase 11.8).

## [v0.24.0] — 2026-07-19 — Named slots: `<slot name="X" />` on the WASM target

**Minor release** — rounds out `<Child />` composition: v0.22.0 shipped
the single default `<slot />`; this adds **multiple named slots** per
child. Entirely additive to the `.fitzv` → WASM emitter (WASM-only — the
SSR target still rejects all slots). Components that only use the default
slot emit byte-for-byte identical; the other view examples regenerate
unchanged.

### What changed

- A child declares several holes — `<slot name="title" />`, a default
  `<slot />`, `<slot name="actions" />` — each with its own fallback.
- The parent fills a named slot by tagging a top-level element inside
  `<Child>...</Child>` with `slot="<name>"` (the native Web Components
  convention). Content **without** a `slot=` attribute fills the default
  slot; a slot the parent doesn't fill renders the child's own fallback.
- Under the hood: the child gains one callback field per slot — `__slot`
  for the default (unchanged since 11.7.d) and `__slot_<name>` per named
  slot (hyphens fold to `_`). The parent partitions its slot content by
  `slot=`, synthesises one `__render_slot_<n>` per bucket (rendered in
  the **parent's** scope → reactive), and wires the matching field. The
  routing `slot` attribute is stripped from the emitted DOM.
- Compile-time validation with clear pointers: a `slot="X"` that targets
  no `<slot name="X" />` in the child, unslotted content when the child
  has no default `<slot />`, or two names colliding on the same backing
  field (`side-bar` vs `side_bar`).
- **No new syntax** — `<slot name="X" />` already parsed; the parent-side
  `slot="X"` is a plain HTML attribute.

### Scope

- **WASM-only.** The change is contained in `src/view/codegen_wasm.rs`;
  classic Fitz and the classic `fitz build` path are untouched. The SSR
  target keeps rejecting slots entirely.
- New example `examples/view/named-slots/` (a `Card` with title/body/
  actions slots + fallbacks) compiles to real WASM (35.2 KB raw).

## [v0.23.0] — 2026-07-19 — Payload bubbling: `<Child @event />` carries data up to the parent

**Minor release** — rounds out the event-bubbling MVP shipped in v0.22.0
(Phase 11.7.c). A `<Child @event="handler" />` bubble now **carries a
payload**, so the parent can tell which child fired and read its
per-item data. Entirely additive to the `.fitzv` → WASM emitter
(WASM-only — the SSR target already rejects `@event` on a child).
Non-bubbled components emit byte-for-byte identical; the nine other view
examples regenerate unchanged.

### What changed

- The child's bubble callback slot goes from `Box<dyn Fn()>` to
  `Box<dyn Fn(&HashMap<String, String>)>`.
- A bubbled event **forwards the payload it received** up to the parent
  — the same `data-flv-value-*` machinery R3.5b uses for plain
  click/form handlers. A bubbled handler always takes a `payload` param
  (so it can forward it), even when its own body never reads it.
- The parent handler receives the payload when it consumes one
  (`payload["k"]` / `payload.has("k")`); a parent handler that ignores
  the payload still works (the closure drops it).
- **No new syntax**: the child chooses what to expose via which
  `data-flv-value-*` attributes it sets on the clickable element.

The `examples/view/event-bubbling` demo is updated: three
`<Item @choose="on_pick" />` children each bubble their own `label`, and
`on_pick` reads `payload["label"]` to know which item was picked.

Change contained to `src/view/codegen_wasm.rs`. Residual (per demand):
the payload is a `Map<Str, Str>` (numbers/bools arrive as strings), and
a typed payload / whole-child-state bubble would be a later slice.

## [v0.22.0] — 2026-07-19 — Phase 11.7 R3.5 + Frente 2: the kanban as a WASM SPA + full `<Child />` composition

**Major release** — closes **Phase 11.7** (client-side dynamic
capabilities) for the `.fitzv` client-WASM target. The headline: the
collaborative-kanban Board — previously an SSR-only single-file
component — now compiles to a **standalone WebAssembly single-page app**
from one `.fitzv` (plus two sibling classic modules), ~57 KB raw /
~21.5 KB gzipped. Everything runs in the browser: add cards, move them
between columns, delete them — no server, no WebSocket.

Nothing in this release changes classic Fitz (`fitz run`/`build`/`check`
of `.fitz` programs are bit-for-bit identical); it is entirely additive
to the `.fitzv` → WASM emitter. The ten pre-existing view examples
regenerate byte-for-byte.

### R3.5 — the kanban path

- **R3.5a.1 — expression machinery on lists**: inline closures,
  `.map`/`.filter`/`.len()` → Rust iterator chains, comparisons, `if`
  as a value, list-literal reassignment (`nums = nums.map(...)` without
  a borrow conflict), and `{#for}` over a call result (not just a bare
  state field). New example `examples/view/list-transform`.
- **R3.5a.2 — imported classic helper fns**: a sibling `.fitz`'s
  top-level `fn`s are transpiled into the WASM bundle (`cards_in`,
  `move_one`, `keep_if_not`, `make_card`, + internal helpers reachable
  through them), so the SFC calls them from templates + event bodies.
  Free-fn calls clone their ident arguments so a captured String/nominal
  survives a `.map`/`.filter` closure. New example
  `examples/view/mini-board`.
- **R3.5b.1 — click payload**: `data-flv-click` + `data-flv-value-*`
  (interpolated attributes) wire a click listener that reads per-item
  data back into a `payload: Map<Str, Str>`; `payload["k"]` /
  `payload.has("k")` lower against it. New example
  `examples/view/click-payload`.
- **R3.5b.2 — form-submit payload**: `data-flv-submit` on a form reads
  its named inputs into the payload and clears `data-flv-clear` fields
  (via the conditional `HtmlInputElement` web-sys feature). New example
  `examples/view/form-input`.
- **R3.5c — the full kanban WASM SPA**: string interpolation
  (`"{next_id}"` → `format!`) + all of the above converge in
  `examples/view/kanban` — nominal `Card`, six transpiled helpers,
  `.map`/`.filter` closures, `{#for c in cards_in(...)}`, per-card click
  payload, form-submit card creation. Builds to real WASM end-to-end.
  The same `data-flv-*` conventions drive both the SSR and WASM targets.

### Frente 2 — `<Child />` composition

- **11.7.c — event bubbling** (`<Child @event="handler" />`): a child
  event fires a parent handler. The child gains one callback slot per
  bound event; only bound events get a slot (byte-for-byte unchanged
  otherwise). MVP: no-payload bubble. New example
  `examples/view/event-bubbling`.
- **11.7.d — slots with fallback** (`<slot>fallback</slot>` +
  `<Child>content</Child>`): a child exposes a `<slot />` hole; the
  parent fills it with content rendered in PARENT scope (parent state +
  event handlers) via a synthesised `__render_slot_<n>` method, or the
  child's fallback shows. MVP: default slot only, no nested `<Child />`
  in slot content. New example `examples/view/slots`.
- Both are client-WASM capabilities: the SSR emitter rejects `@event` on
  a child, `<slot />`, and `<Child>content</Child>` with clear pointers.

## [v0.21.0] — 2026-07-16 — Phase 11: Native frontend `.fitzv` compiled to WASM + SSR emitter for fitz-liveviews

**Major release** — ships **Phase 11 (Native frontend in Fitz
core)** in one coordinated bump. Fitz gains a new file extension
`.fitzv` (single-file components à la Vue/Svelte, hand-rolled
parser + expand + checker + two backend targets: WASM for
client-side interactivity, SSR for server-rendered HTML). The
compiler + module loader route `.fitzv` transparently — a
sibling `.fitz` still wins when both exist, so the migration
from classic to view is opt-in and additive. `fitz build --bin
web --target wasm-client` produces a browser bundle end-to-end
(hand-rolled `wasm-bindgen` under feature `client-wasm`, 11.4 KB
gzipped for a canonical counter demo). The SSR emitter targets
the `fitz-liveviews` framework contract (`@live_component` +
`@render_for` + `@on`), and cross-module auto-inject removes
the manual `flv_register(...)` boot boilerplate for components
declared in imported `.fitzv` sibling modules.

**What's shipped**:

- **Phase 11.1 — POC parser** (`.fitzv` extension, shell
  scaffold, HTML sub-parser). New `crate::view` module with
  `pub fn parse(source) -> ViewParseResult<ViewFile>`.
- **Phase 11.2.a/b/c — Bridge to classic AST + checker**.
  `expand.rs` lowers `.fitzv` raw blobs to classic Fitz AST
  segments; checker validates state field defaults + event
  handler signatures + template interpolations + cross-refs
  (`@click="handler"` must name an event in the same
  component). Template directives `{#if cond}…{/if}`,
  `{#for x in xs}…{/for}`, `{#else}`, and `<slot />` end-to-
  end.
- **Phase 11.3.a/b/c — Scoped styles**. `<style scoped>`
  (default) rewrites class attrs + selectors with a per-
  component scope suffix; `<style global>` opts out. CSS
  mini-parser + `apply_scope(...)` helper. Fully wired into
  `expand`.
- **Phase 11.4.a/b/c/d — WASM emitter (approach A2)**.
  Hand-rolled `wasm-bindgen` + `web-sys` under feature opt-in
  `client-wasm`; gate closed at 11.4 KB gzipped over 40 KB
  budget (28.6 KB headroom). Counter demo runs in Chrome end-
  to-end: `0 → +1 → +2 → -1 → 0` cycle with subtree-scoped
  re-renders (§9.m D1 naive-render policy). Bundle-size gate
  reproducible via `tests/view_counter_wasm_smoke.rs`.
- **Phase 11.5.a/b/c/d/e — CLI wiring + multi-component
  composition**. Manifest gains `[[bin]] name = "…" main =
  "…" target = "wasm-client|native|ssr" mount = "#app"` (with
  legacy `[bin]` auto-migration at parse time — cero breaking
  para 40+ boilerplates + curso; **cierra debt 9.y.8+**). CLI
  `fitz build --bin <name> [--target <t>]` dispatches wasm-
  client to a wasm-pack pipeline that materializes a scaffold
  in `target/wasm-build/<bin>/` and copies `pkg/` to
  `target/wasm/<bin>/`. `<Child prop="v" />` in a template
  mounts a sibling component with static props (`Str`, `Int`,
  `Float`, `Bool`, `Nullable<T>` primitives; nominals/generics/
  functions/tuples deferred to 11.7+).
- **Phase 11.6.a — Research + decision**. Restored the
  original §6 row 11.6 intent (SSR emitter for the 4
  `fitz-liveviews` examples) after §9.t drift; client-side
  dynamic capabilities re-scoped formally to 11.7+.
- **Phase 11.6.b — Skeleton SSR emitter**. New module
  `src/view/codegen_ssr.rs` (~700 LoC + 20 unit tests). Emits
  classic Fitz source (not AST — more debuggable) targeting
  the `fitz-liveviews` contract: `from fitz_liveviews import
  Html, html` + `@live_component("Name") type Name {…}` +
  `@render_for("Name") fn Name_render(state) -> Html` + one
  `@on("Name", "event") fn Name_event(state, payload) ->
  Name` per event block. Event body lowering: mutations
  accumulate → struct-lit return carries every state field
  (mutated fields take assigned RHS, untouched take
  `state.<field>`). `@click="handler"` in the template
  rewrites to `data-flv-click="handler"` in the emitted HTML.
- **Phase 11.6.c partial + continuation — Full expression
  grammar + template directives inline**. RHS walker
  (`format_fitz_expr_scoped`) covers BinOp, UnaryOp, Call,
  Field, Index, StrInterp, List, Map, Range, Ok, Err, arrow
  FnExpr with state-field rewriting + closure-param local-
  scope tracking. `<style scoped>` / `<style global>` blocks
  inline at the top of the emitted render body. `{#if}` /
  `{#for}` lower to Fitz expressions in the template
  (`__fitz_view_str_join` helper). View lexer gains
  `Token::Dot` so `state.count.upper()` reconstructs
  losslessly in event body raw-capture context.
- **Phase 11.6.d — Loader integration + same-file `<Child />`
  composition**. All 5 module loader entry points (evaluator,
  codegen, main CLI pre-scan, LSP definition, LSP from-import)
  try `.fitz` first and fall back to `.fitzv` transparently.
  When a `.fitzv` resolves, `crate::view::transform_fitzv_source
  (source, path)` runs the view pipeline (parse → expand →
  check → emit_module_ssr) and hands classic Fitz source to
  the classic loader. Any view-pipeline failure gets wrapped
  in a `FitzError` naming the offending path + stage. Same-
  file `<Child prop="v" />` composition inline-renders the
  sibling with primitive Fitz-literal props.
- **Phase 11.6.e (partial via §9.z + §9.aa + §9.bb)**.
  - **§9.z**: SSR emitter accepts `payload` in event-body
    scope (`payload["key"]` / `payload.has(…)` / `payload.get(…)`
    natural in RHS). Enriched module-not-found hint for
    `fitz_liveviews` (both `fitz run` + `fitz build` loaders
    detect missing framework dep and suggest the canonical
    git dep snippet in `fitz.toml`).
  - **§9.aa**: Event-body widening. `emit_event_fn` dispatches
    trivial vs widened bodies via `is_trivial_event_body`; wide
    path primes shadow locals + walks recursively accepting
    `Stmt::Assign` to `Ident` (new local or shadow mutation)
    and `Stmt::Expr(Expr::If, _)` guards con arm-scope
    truncation. Walker widened para `Expr::If` (single-expr
    arms) + `Expr::StructLit` (walk fields verbatim). Unblocks
    kanban's `card_editor_save` (`let new_text = if(payload.
    has("text")){payload["text"]} else {text}`) + chat's
    `send_message` (nested `if(payload.has("author")){if(payload.
    has("text")){last_msg = payload["text"]}}`).
  - **§9.bb**: **Cross-module `@live_component` auto-inject**.
    Extends v0.20.1's implicit `flv_register(...)` injection
    (which only scanned top-level program AST) to components
    declared in imported `.fitzv` / `.fitz` sibling modules.
    Paralelo bit-a-bit a W12 (`pre_scan_imported_auth_provider`)
    y B10 (`pre_scan_imported_background_fns`): new
    `TypeEnv.imported_live_components: Vec<ImportedLiveComponent>`
    populated by `pre_scan_imported_live_components` in
    `main.rs`; `inject_live_component_registrations` extended
    con imported loop that emits `flv_register("Counter",
    Counter { }, Counter_render, {…events})` bare-Ident calls
    per imported component. Local-wins-over-imported silent
    skip; missing-names errors list every expected name plus
    the actionable `Add \`from <module> import <TypeName>,
    <TypeName>_render, <TypeName>_<event>...\`` fix. Removes
    the manual boot boilerplate del counter migration draft +
    destraba dashboard/chat/kanban migrations una vez que
    aterricen.

**Tests al cierre v0.21.0**: 3651 unit (default) + 3787 unit
(`--features lsp`) + 115 cli_e2e + 3 openapi_e2e + 381 compile_e2e
(4 pre-existing failures — file-lock Windows races +
`orm_w17` #7 codegen drift + `http_coverage` routing 404 —
todos documentados pre-v0.21.0, cero regresiones imputables al
diff Phase 11 entero). `--features python` env-blocked por
`PYO3_PYTHON` shim del host — no regresión. `cargo fmt --all
--check` limpio. `cargo clippy --lib --tests --bins -- -D
warnings` limpio.

**Extensión VSCode bumpeada a 0.21.0** con `.vsix` regenerado
(`editors/vscode/fitz-language-win32-x64-0.21.0.vsix`)
bundleando el `fitz-lsp.exe` fresh de v0.21.0.

**Files touched (summary)**:

- `src/view/` — nuevo módulo entero (`parse.rs`, `ast.rs`,
  `expand.rs`, `check.rs`, `codegen_wasm.rs`,
  `codegen_ssr.rs`, `wasm_build.rs`, plus lexer). ~4500 LoC
  + ~360 unit tests.
- `src/lexer.rs` — view-dialect keywords + `.` follow-up +
  §7 state annotations con generics.
- `src/manifest.rs` — `[[bin]]` array-of-tables + `target`
  + `mount` fields con legacy `[bin]` auto-migration.
- `src/main.rs` — `--bin <name>` / `--target <t>` CLI flags
  + `pre_scan_imported_live_components` helper +
  `build_wasm_client_cmd` dispatch.
- `src/types.rs` — `ImportedLiveComponent` struct +
  `TypeEnv.imported_live_components` field + accessors +
  `extract_live_components_from_program` public helper +
  `inject_live_component_registrations` extension +
  `collect_names_in_scope` helper.
- `src/evaluator.rs`, `src/codegen.rs`, `src/lsp.rs` —
  `.fitzv` loader integration + enriched module-not-found
  hint for `fitz_liveviews`.
- `examples/view/` — counter demo (WASM-runnable) + showcase
  (multi-component composition fixture).
- `docs/fase-11-plan.md` — plan doc con §1–§10 + §9.a–§9.bb
  sub-sections (~5400 lines).
- `docs/stack.md` — architectural constitution (Invariants
  1–4, WASM-first).
- `docs/roadmap.md`, `README.md`, `CLAUDE.md`, `docs/index.md`
  — Phase 11 status refreshed.
- `editors/vscode/package.json` bumpeada a 0.21.0 + `.vsix`
  regenerado.

**Cierre formal de Phase 11 (parcial)**: 11.1 → 11.5 CLOSED
ENTIRELY. 11.6.a → 11.6.d CLOSED ENTIRELY. 11.6.e PARTIAL
(§9.z + §9.aa + §9.bb) — remaining scope: cross-file
`<Child />` composition (§9.y debt, low priority), migration
commits en `fitz-liveviews` (pending post-release rebase).

**Debt residual (NO bloquea uso real)**:

- **Cross-file `<Child />` composition** (§9.y): threading
  loader's expanded-file cache through checker + emitter.
  None of the 4 `fitz-liveviews` examples need it (all use
  runtime `component(name, id)` API), so low priority.
- **`fitz check` inject-time errors**: cross-module auto-
  inject validation errors surface via `fitz run` / `fitz
  build` only (checker doesn't run inject). Refinable si
  demand real aparece del LSP or CI-only flows.
- **Client-side dynamic capabilities** (Phase 11.7):
  dynamic props (`prop={expr}`), event bubbling, cross-file
  `<Child />` static + dynamic, `<slot />` fallback,
  persistent child state, drag-drop. Deferred hasta demanda
  real (kanban SPA port pinned as the acceptance criterion).
- **LSP support inside `.fitzv`** (Phase 11.8): hover,
  autocomplete, template-attr completion. Deferred.
- **Pedagogic docs** (Phase 11.9): cap dedicado en
  `docs/guide.md` + módulo del curso + `architecture.md`
  refresh. Deferred.
- **`fitz-liveviews` migration commits**: counter draft
  uncommitted en el sibling repo desde §9.z; dashboard
  debería seguir el mismo shape (extract `MetricTile.fitzv`);
  chat + kanban ahora desbloqueados por §9.aa (event-body
  widening) + §9.bb (cross-module auto-inject). Commits
  land post-v0.21.0 release.

**Próximo norte tras v0.21.0**: **atacar deudas** — inventario
completo en `docs/deudas-post-5b.md`. Ver también:
- `docs/roadmap.md` — sub-fases 11.7 / 11.8 / 11.9 y Fase 12+
  status.
- `docs/fase-11-plan.md` §11 — remaining sub-pass scope for
  11.6.e continuations.

## [Unreleased]

*(vacía — próximas entradas van acá antes del siguiente bump)*

## [v0.21.8] — 2026-07-19 — Phase 11.7 (R3): tipos nominales en el target WASM

**Patch release aditivo** — cierra el gap foundational de **nominales
en WASM** (R3 prereq del kanban SPA port). El target **client-WASM**
de `.fitzv` ya acepta un `type` classic importado como ciudadano de
primera clase: `List<Card>` como state, `{#for c in cards}`, field
access `{c.title}`, construcción `Card { ... }` + mutación live de la
lista, y keyed `<Child />` cuyos props primitivos salen de los campos
del nominal. Antes `type_expr_to_rust` rechazaba todo nominal citando
"Phase 11.6+/11.7+".

```fitzv
from card import Card

component App {
  state { cards: List<Card> = [] next_id: Int = 1 }
  event add() { cards.push(Card { id: next_id, title: "Task", done: false }) }
  <template>
    {#for c in cards}
      <CardRow key="{c.id}" n="{c.id}" title="{c.title}" />
    {/for}
  </template>
}
```

**Por qué era el linchpin**: el SSR emitter "hace trampa" — reemite el
`from card import Card` verbatim y difiere toda resolución nominal al
loader classic en un segundo pass. El target WASM produce un crate
`wasm32` standalone sin segundo pass classic, así que **cada
touchpoint nominal tiene que bajar a Rust real inline**: un `struct`
de verdad, field access de verdad, struct literal de verdad. El
pipeline view no carga los bodies de los `.fitz` siblings (los imports
son name-only, el checker registra nominales como stubs opacos), así
que R3 **carga el `type Card` del sibling** (lexer + parser → `Stmt::
TypeDef`) y sintetiza el struct Rust en el bundle.

**Cambios técnicos**:
- **`src/view/codegen_wasm.rs`** (~180 LoC netas): tipo público
  `NominalRegistry` (`BTreeMap<nombre_local, Vec<(campo, TypeExpr)>>`,
  keyed por el binding local para respetar alias); `emit_module_with_
  nominals` (delega desde `emit_module` con registro vacío → los 4
  ejemplos pre-R3 quedan byte-a-byte idénticos); `emit_nominal_structs`
  emite `#[allow(dead_code)] #[derive(Clone)] pub struct Card { ... }`
  (dead_code item-level porque un `type` puede tener campos que el
  template no lee; Clone porque `emit_for` snapshotea con `.iter().
  cloned()`); `type_expr_to_rust` acepta nominales registrados;
  `emit_for` itera `List<nominal>`; `lower_expr` gana arms `Str` /
  `Field` (`obj.field.clone()`) / `StructLit` (`Card { f: v, ... }`);
  `lower_child_prop_value` acepta field access; `lower_stmt` soporta
  `<state_list>.push(<expr>)` + `.clear()` (mutación live → la
  reconciliation de R2b corre de verdad, no sobre una lista constante);
  `RenderCtx` thread-ea el registry.
- **`src/view/wasm_build.rs`** (~90 LoC): `load_imported_nominals`
  (resuelve el `.fitz` sibling single-segment relativo al `.fitzv`,
  lexea + parsea, extrae `Stmt::TypeDef` de los nombres importados,
  respeta alias; best-effort: sibling faltante / fn-only / dotted-path
  se saltean); `compose_lib_rs_with_nominals` (delega desde
  `compose_lib_rs`); `write_wasm_crate_scaffold` gana un param
  `&NominalRegistry`.
- **`src/main.rs`**: `build_wasm_client_cmd` carga los nominales del
  dir del `.fitzv` de entrada antes de scaffoldear.
- **`src/view/mod.rs`**: re-exporta `NominalRegistry`,
  `emit_module_with_nominals`, `load_imported_nominals`,
  `compose_lib_rs_with_nominals`.

**Tests** (+18): 8 unit `phase_11_7_r3_*` (struct emission, `Vec<Card>`
state, `{#for}` snapshot + loop var, field-access interpolación + key,
struct-lit + push, keyed dynamic child, unregistered-nominal reject,
empty-registry no-struct) + 5 unit `load_imported_nominals_*` (lee
fields, alias, sibling faltante, fn-only, solo nombres importados) + 1
test viejo repunteado a R3. **Ejemplo runnable nuevo**
`examples/view/nominal-list/` (`card.fitz` + `App.fitzv` con `App` +
`CardRow`, compila a WASM real end-to-end con cero warnings) + smoke
`tests/view_nominal_list_wasm_smoke.rs`.

**Deuda residual derivada (hacia el kanban completo)**: R3 trae los
*tipos* nominales. El kanban además necesita `{#for}` sobre el
resultado de un fn call (`{#for c in cards_in(cards, "todo")}`),
`.map`/`.filter` + closures en event bodies, e imported classic helper
fns transpiladas al crate WASM (`cards_in`, `move_one`, ...). Ese es el
próximo slice (imported-fn support). El target SSR ya los soporta.

## [v0.21.7] — 2026-07-18 — Phase 11.7 (R2b): keyed `<Child />` composition dentro de `{#for}` en el target WASM

**Patch release aditivo** — el último slice del "rendering core" de
**Phase 11.7** (client-side dynamic capabilities) antes de event
bubbling / slots (11.7.c/d) y nominales + kanban (R3). Trae al
target **client-WASM** de `.fitzv` la composición de **children
dinámicos con identidad estable** dentro de un `{#for}`, sobre
`List<primitive>`:

```fitzv
{#for name in columns}
  <Column key="{name}" title="{name}" />
{/for}
```

**El atributo `key="{expr}"`** — nuevo, reservado — le da a cada
child una identidad estable. NO es un prop: se extrae en expand
time y nunca llega al struct literal del child. El WASM emitter
mantiene una **keyed instance cache** por sitio dinámico
(`__child_map_<n>: RefCell<HashMap<String, Rc<Child>>>`) y hace
get-or-create con `map.entry(key).or_insert_with(|| Child::new())`.
Una key existente **reusa** la instancia (su state local sobrevive
el re-render del parent, igual que 11.7.e para sites estáticos); una
key nueva la crea. Entre renders, un sweep de **reconciliation**
(`retain`) evicta las keys que desaparecieron de la lista.

**Cómo funciona la reconciliation**: el `emit_for` declara un set
`__seen_<n>` por sitio dinámico ANTES del loop; cada child mounteado
inserta su key; después del loop, `self.__child_map_<n>.retain(|k,
_| __seen_<n>.contains(k))` libera las instancias huérfanas. El
`key` lowerea vía `format!("{}", <key_expr>)` (típicamente la loop
var), unificando `List<Str>`/`List<Int>`/etc a una `String` key.

**Cambios técnicos**:
- **`src/view/expand.rs`**: `ExpandedTemplateNode::ChildComponent`
  gana `key: Option<fast::Expr>`; `expand_child_component`
  special-casea el atributo `key` (interpolado → key expr, static
  `key="..."` → error citando que debe ser `{...}`).
- **`src/view/check.rs`**: destructure con `..` (el key se valida en
  emit time — requerido para dynamic, ignorado para static).
- **`src/view/codegen_ssr.rs`**: passthrough (SSR re-renderiza el
  HTML entero, la key es irrelevante) — sin cambio de comportamiento.
- **`src/view/codegen_wasm.rs`** (~250 LoC): `collect_child_site_types`
  clasifica sites STATIC vs DYNAMIC (dentro de `{#for}`) y desciende
  igual que el render walk (Element / If / For); `emit_struct_and_new`
  emite `__child_slot_<n>` (static) o `__child_map_<n>` (dynamic);
  `RenderCtx` gana `static_site_counter` + `dyn_site_counter` +
  `in_for`; `emit_for` pre-escanea sus dynamic child sites, declara
  los seen sets y emite los retains; `emit_child_component` recibe el
  `key` y ramifica static (slot) vs dynamic (keyed map + seen +
  reconciliation). Índices static/dynamic independientes, alineados
  entre collect y render por DFS idéntico.

**Tests** (`codegen_wasm`, +6): `phase_11_7_b_r2b_*` — keyed map
field, seen set + retain, keyless dynamic child rechaza, static
`key="literal"` rechaza en expand, static+dynamic indices alineados,
`{#for}` sin child no reconcilia.

**Ejemplo runnable nuevo**: `examples/view/keyed-composition/` — un
`App` con 3 columnas (`List<Str>`), cada `<Column key="{name}" />`
con su propio contador `taps`; el botón "re-render parent" prueba
que los contadores sobreviven. Compila a WASM real 40 KB. Smoke
`tests/view_keyed_composition_wasm_smoke.rs`.

**Deferido con pointers claros**: `{#for}` sobre `List<nominal>`
(kanban, R3 prereq — nominales en WASM); event bodies que mutan la
lista (push/remove — 11.4.c debt, la lista del ejemplo es constante
así que la reconciliation corre pero nunca evicta live); event
bubbling (11.7.c) + `<slot />` (11.7.d).

**Verificación pre-bump**: 3755 lib (default, +6) + 3911 lib
(`--features lsp`, +6) verde; 115 cli_e2e verde; `cargo fmt --all
--check` + `cargo clippy --all-targets -- -D warnings` (default +
lsp) limpios; smokes de `counter`/`showcase`/`reactive-props`/
`control-flow` regeneran sin cambios (static path bit-a-bit
idéntico); `keyed-composition` compila a WASM real end-to-end.

## [v0.21.6] — 2026-07-18 — Phase 11.7 (R2): control-flow (`{#if}`/`{#for}`) + persistent child state en el target WASM

**Patch release aditivo** — segundo slice de **Phase 11.7**
(client-side dynamic capabilities). Trae al target **client-WASM**
de `.fitzv` tres capacidades que le faltaban: **persistencia de
state del child** (11.7.e) + los directives de control-flow
**`{#if}`** y **`{#for}`** (que estaban deferidos desde 11.4.c y
nunca se habían implementado en el emitter WASM — el counter/
showcase no los usaban, por eso pasó desapercibido).

**11.7.e — Persistent child state (keyed instance cache, sites
estáticos)**: el parent ahora cachea cada `<Child />` en un slot
tipado (`__child_slot_<n>: RefCell<Option<Rc<Child>>>`) y hace
get-or-create en vez de `Child::new()` en cada render. El child se
**reusa** entre re-renders del parent, así que su state local
**sobrevive** (antes de 11.7.e se recreaba y se reseteaba). El
ejemplo `examples/view/reactive-props/` ahora lo demuestra: el
`Badge` tiene un contador `taps` propio que aguanta el "bump" del
parent.

**`{#if}` en WASM**: `{#if cond}...{/if}` / `{#if
cond}...{#else}...{/if}`. `cond` lowerea a un `bool` Rust — bool
state field / loop var usado directo, comparación numérica
(`>`/`<`/`==`/`!=`/`<=`/`>=`), o `&&`/`||`/`!` sobre esos.
Evaluado en render time bajo el modelo dirty-flag.

**`{#for}` en WASM sobre `List<primitive>`**: `{#for x in
<field>}` donde `<field>` es un state field `List<Int|Float|Str|
Bool>`. Snapshotea el `Vec` (`.clone()`), itera por valor, y
bindea `x` como loop var Rust en scope para los children
(usable en `{x}`). El expr lowering (`lower_expr`) gana un
parámetro `locals` para resolver loop vars, y `RenderCtx` gana el
stack de `locals` + los `state_fields` (para resolver el element
type del iterable).

**Cambios técnicos** (`src/view/codegen_wasm.rs`, ~350 LoC + 8
tests): `collect_child_site_types` + slot fields en
`emit_struct_and_new` + get-or-create en `emit_child_component`
(11.7.e); `emit_if` + `emit_for` + `lower_cond_expr` +
`RenderCtx.locals`/`state_fields` (control-flow). Tests
`phase_11_7_e_*` (2) + `phase_11_7_b_*` (4). Los tests stale
`emit_rejects_if_directive`/`emit_rejects_for_directive`
reemplazados por el shape de aceptación.

**Ejemplo runnable nuevo**: `examples/view/control-flow/` (`{#if}`
/`{#else}` + `{#for}` sobre `List<Str>`, botón que flipa el
condicional), compila a WASM real 26 KB. Smoke
`tests/view_control_flow_wasm_smoke.rs`.

**Deferido con pointers claros** (para slices posteriores):
- **`{#for}` sobre `List<nominal>`** (ej `List<Card>`, lo que el
  kanban necesita) — espera soporte de tipos nominales en el
  target WASM (Phase 11.7 R3 prereq).
- **`<Child />` composition dentro de `{#for}`** (keyed dynamic
  children con atributo `key`) — R2b (v0.21.7). Resultó más
  grande de lo estimado: necesita plumbing del `key` cross-module
  (view parser → expand → check) + per-item cache + reconciliation,
  y está limitado a `List<primitive>` hasta que aterricen los
  nominales.
- **Event bubbling** + **`<slot />`** — R2b/posterior.

**Verificación pre-bump**: 3749 lib (default) + 3905 lib
(`--features lsp`) verde; cli_e2e verde; `cargo fmt --all --check`
+ `cargo clippy --all-targets -- -D warnings` limpios; smokes de
`counter`/`showcase`/`reactive-props`/`control-flow` regeneran +
compilan a WASM.

## [v0.21.5] — 2026-07-18 — Phase 11.7.a (R1): reactive interpolated child props en el target WASM

**Patch release aditivo** — primer slice de **Phase 11.7**
(client-side dynamic capabilities), la última sub-fase abierta de
Fase 11. El target **client-WASM** de `.fitzv` ya acepta props
**interpolados** en la composición de componentes:

```fitzv
<Badge heading="{title}" count="{clicks + 1}" />
```

Antes de 11.7.a el WASM emitter rechazaba `<Child prop="{expr}" />`
con un pointer a "Phase 11.7+" (sólo props estáticos string, ver
`examples/view/showcase/`). Ahora acepta el **caso simple**: el
prop es un state field del parent (`{title}`) o aritmética sobre
state numérico (`{n + 1}`), hacia un field **primitivo**
(`Int`/`Float`/`Str`/`Bool`) del child.

**Reactividad — decisión de diseño: dirty-flag + reconciliation**
(vs signals SolidJS-style). El modelo de re-render actual (naive
full re-render por componente) ya provee la propagación reactiva
gratis: el parent muta state → re-renderiza → recomputa el prop
del child → lo re-monta con el valor fresco. Sin signals, sin
VDOM. El upgrade a signals fine-grained queda para si aparece
evidencia de performance real.

**Cambios técnicos** (`src/view/codegen_wasm.rs`, ~110 LoC + 5
tests):
- `emit_child_component` — la rama que rechazaba props
  interpolados en el WASM path ahora computa el valor y lo asigna
  al `RefCell<T>` del child (paralelo al path estático que
  coerciona un literal).
- `lower_child_prop_value(expr, field, target_type, state_names)`
  — lowerea el `Expr` del prop a Rust: bare parent state field →
  `(*self.<name>.borrow()).clone()` (uniforme para todo
  primitivo); aritmética numérica → reusa el event-body lowerer.
- `is_wasm_prop_simple_target` — guard que restringe a targets
  primitivos (nullable / nominal / list defieren con pointer
  claro a un slice posterior / el SSR target).
- 5 unit tests `phase_11_7_a_wasm_*` (bare Str field, aritmética
  Int, target nullable rechaza, ident no-state rechaza, path
  estático intacto). El test stale `k3_interp_wasm_..._rejects`
  reemplazado por el shape de aceptación.

**Ejemplo runnable nuevo**: `examples/view/reactive-props/`
(parent `App` con `title`/`clicks` → child `Badge` con props
`heading`/`count`; botón "bump" incrementa `clicks` → el `count`
del child se actualiza reactivamente). Compila a WASM real (32.2
KB) con `wasm-pack build`. Smoke
`tests/view_reactive_props_wasm_smoke.rs` (regenera `lib.rs` +
`Cargo.toml` en cada `cargo test`, más un build `#[ignore]`).

**Límite R1 → R2 documentado honestamente**: el child se
**recrea** en cada re-render del parent (sin keyed instance cache
todavía), así que un child con state local lo perdería. Por eso el
`Badge` del ejemplo es puro display. Persistent child state +
`{#for}` composition + event bubbling + slots son el trabajo de
**R2 (v0.21.6, Phase 11.7.b/c/d/e)**; drag-drop + kanban SPA port
son **R3 (v0.22.0, Phase 11.7.f/g)**.

**Verificación pre-bump**: 3745 lib (default) + 3901 lib
(`--features lsp`) verde; cli_e2e + openapi_e2e verde; `cargo fmt
--all --check` + `cargo clippy --all-targets -- -D warnings`
limpios; smokes de `counter` + `showcase` regeneran sin cambios
(static path intacto); el ejemplo `reactive-props` compila a WASM
end-to-end.

## [v0.21.4] — 2026-07-18 — Phase 11 Session C: Pedagogic docs (Phase 11.9 CERRADO)

**Docs-only release** que cierra **Phase 11.9 entera** — la
última sub-fase visible de Fase 11. Cero código Rust nuevo,
cero features runtime. Todo es **contenido pedagógico**
cross-doc: guía + curso + architecture. **Con este release
Phase 11 queda cerrada por completo — todas las 3 sub-fases
finales (11.8 LSP, 11.9 docs, y 11.7 client-side reactivity
scheduled next) apuntadas al roadmap**.

### Cap 36 nuevo en `docs/guide.md` — Frontend nativo con `.fitzv` (SFC)

**~1050 LoC** de markdown pedagógico que cubre la superficie
completa del `.fitzv` con el mismo estilo del resto de la guía
(panorama vecino + "En Fitz..." + Las piezas + ejemplos +
cross-links). Secciones:

- **Panorama vecino** — Vue SFC, Svelte, React JSX, Elm,
  HTMX+Jinja, Phoenix LiveView. Contexto competitivo real.
- **En Fitz** — el `.fitzv` como extensión, dos backends
  (SSR + WASM), sin herramientas build externas.
- **Las piezas** — `component <Name> { ... }`, state, event,
  `<template>`, `<style scoped|global>`, imports con `as`
  (S.1 shipped v0.21.2).
- **Interpolación de expresiones** — regla de scoping en 4
  niveles (local scope > state field > imported name > error).
- **Cross-file types** — `.fitzv` importa tipos de `.fitz`
  classic sibling.
- **Composición de components** — `<Child prop="v" />` con las
  4 formas de props (primitives, List<primitive>, Map<Str,Str>,
  interpolated).
- **Estilos scoped** — CSS rewriting automático + `<style
  global>` para reset/global.
- **Los dos backends de compilación** — SSR (fitz-liveviews) +
  WASM client-side (opt-in). Bundle size del counter demo:
  11.4 KB gzipped sobre 40 KB gate.
- **Editor support (LSP)** — cross-link al cap 22.
- **Ejemplo runnable — Contador Fitz-LiveViews** completo con
  `Counter.fitzv` + `main.fitz`.
- **Compatibilidad con classic Fitz** — .fitz + .fitzv en el
  mismo proyecto, `.fitz` gana cuando coexisten.
- **Qué no está en el MVP** — Cross-file `<Child />`, WASM
  interpolated props, event bubbling implícito, `<slot />`
  fallback, persistent child state.

Renumeración: cap 36 (Boilerplates) → 37, cap 37 (Qué sigue)
→ 38.

### Cap 22 (Soporte para editores) refresh

Nueva sub-sección "**En archivos `.fitzv`**" con las 4
capabilities LSP de Phase 11.8 (v0.21.3): diagnostics en vivo
+ 4 clases de completions + hover + go-to-def. Detalle de las
deudas residuales (cross-module symbol lookup, TypeInfo-based
hover, fine-grained context routing, signature help / rename).

### Cap 38 (Qué sigue) refresh

Sección "**Lo que ya bajó de especulativo a REALIDAD**" —
`.fitzv` frontend + Fase 12 deployment + Fase 10.6 migraciones
CERRADAS. Nueva sección "**Lo que sigue**" apunta a Phase
11.7 (client-side dynamic capabilities + kanban SPA port) y
companion UI library como próximos nortes.

### `docs/architecture.md` refresh

Nueva sección **`view/` — `.fitzv` single-file components
(Phase 11)** que describe los 7 módulos del pipeline view:
`lexer.rs` / `parser.rs` / `expand.rs` / `check.rs` /
`codegen_ssr.rs` / `codegen_wasm.rs` / `wasm_build.rs`. Explica
las 2 branches de emit compartiendo 1 check pass, y la
integración con el module loader (`.fitz` gana cuando coexiste
con `.fitzv` del mismo stem).

### Nuevo módulo del curso — M9 Frontend nativo con `.fitzv`

Nuevo folder `docs/curso/m9-fitzv-frontend/` con 3 caps
pedagógicos + entrada en `docs/curso/index.md` + entrada en
`mkdocs.yml` nav:

- **[C1 — Tu primer `.fitzv` (Counter component)](docs/curso/m9-fitzv-frontend/c1-primer-fitzv.md)**
  (~250 LoC) — el "hola mundo" del `.fitzv`. Counter con state
  + 3 events + template + style. Wire-up completo con
  `fitz-liveviews`. Troubleshooting típico.
- **[C2 — Template DSL](docs/curso/m9-fitzv-frontend/c2-template-dsl.md)**
  (~350 LoC) — las 5 features del template en profundidad:
  interpolación de expresiones (bare state + field access +
  method calls + inline math), attribute interpolation,
  directivas `{#if}` / `{#else}` / `{/if}` + `{#for x in xs}`
  / `{/for}`, wire de eventos con `data-flv-*`, composición
  de components. Ejemplo TodoList runnable.
- **[C3 — Full-page SFC: Board.fitzv migration del kanban](docs/curso/m9-fitzv-frontend/c3-full-page-sfc.md)**
  (~350 LoC) — el **acceptance criterion del módulo**. Kanban
  migrado con el pattern canónico: `card.fitz` (types) +
  `board_helpers.fitz` (helpers puros) + `Board.fitzv` (SFC
  full-page con 4 events + template con 3 columnas) +
  `main.fitz` (HTTP + WS wire-up de 30 LoC). Comparación pre/
  post migration numbers.

### `docs/curso/index.md` refresh

Tabla de módulos actualizada: M9 nuevo con "✅ MVP (C1-C3,
v0.21.4)". Sección "M9 — Frontend nativo con `.fitzv`"
completa con requisitos, links a los 3 caps, y entregable
del módulo.

### `mkdocs.yml` nav refresh

Nueva entrada `M9 — Frontend nativo con `.fitzv`` con los 3
caps entre M8 y "Construyendo TaskHub".

### `docs/index.md` refresh

Nueva entry "**Phase 11 Session C — Pedagogic docs**" que
cita el cap 36 nuevo, el M9 nuevo, y la refresh de architecture.

### Verificación pre-bump

- 3741/3741 lib tests default features verde.
- 3897/3897 lib tests con `--features lsp` verde.
- 115/115 cli_e2e verde.
- `cargo fmt --all --check` limpio.
- `cargo clippy --lib --tests --release --features lsp -- -D
  warnings` limpio.
- Docs: cero código Rust nuevo. Todo el diff es markdown +
  mkdocs.yml.
- Smoke real Board.fitzv del kanban (`fitz-liveviews`)
  `fitz check` verde — sin regresión.

### Bump

- `Cargo.toml` 0.21.3 → 0.21.4.
- `editors/vscode/package.json` 0.21.3 → 0.21.4.
- `.vsix` regenerado (no cambios al binario ni al grammar —
  regen para paridad de versión numérica con el resto del
  ecosistema).

### Estado post-Session C

**Phase 11 CERRADA ENTERA post-v0.21.4**: 11.1 → 11.6 shipped
en v0.21.0 (base + SFC pipeline); 11.8 shipped en v0.21.3
(LSP inside `.fitzv`); 11.9 shipped en v0.21.4 (pedagogic
docs). **Solo queda Phase 11.7 (client-side dynamic
capabilities + kanban SPA port) como sub-fase futura de
Phase 11** — schedule TBD, no bloquea uso real (SSR path
cubre el 100% del caso Board y el 95% del caso general).

## [v0.21.3] — 2026-07-18 — Phase 11 Session B: LSP inside `.fitzv` (Phase 11.8 CERRADO)

**Patch release aditivo** que cierra **Phase 11.8 entera** — el
LSP ahora reconoce `.fitzv` como surface de primera clase con las
cuatro capabilities core: diagnostics + completions + hover +
go-to-definition. Editar un `.fitzv` en VSCode ya no es texto
plano — la extensión bundleada con este release muestra errores
de view lexer/parser/expand/check en tiempo real, completa
directivas de template + state fields + event handlers, hover
sobre state fields muestra tipo declarado, y go-to-def salta a
la línea de declaración. Sin cambios breaking.

### 11.8.a — Diagnostics para `.fitzv` (~150 LoC + 8 tests)

- Nuevas fns públicas en `lsp.rs`: `check_view_source(source) ->
  Vec<FitzError>` que routea via view lexer → parser → expand →
  check y mapea los 3 tipos de error (`ViewParseError`,
  `ExpandError`, `CheckError`) a `FitzError` shape; y
  `check_source_by_uri(uri, source)` que dispatch por extensión.
- Nuevo `uri_is_fitzv(uri)` helper.
- LSP bin `check_and_publish` dispatch por `uri_is_fitzv(&uri)`
  — `.fitzv` route al pipeline view, classic Fitz sigue por el
  path original. `DocumentState` para `.fitzv` stashea `Program`/
  `TypeEnv` vacíos (los usa el path classic).
- Error kind mapping: `ViewParseError` → `InvalidSyntax`;
  `ExpandError` → `InvalidSyntax`; `CheckError` → `TypeMismatch`.
- Cross-module (`from X import Y`) NOT walked por este MVP —
  paralelo al gap del path classic pre-W12/B10; refinable si
  entra demanda.

### 11.8.b — Completions inside `.fitzv` (~150 LoC + 5 tests)

- Nueva `completion_at_position_view(source, line, character)`
  con 4 clases de completion:
  1. **Template directives** — `{#if}`, `{#for}`, `{#else}`,
     `{/if}`, `{/for}` cuando el cursor está tras `{` o `{#`.
     SNIPPETs con tabstops (`$1`, `$0`) para navegación de
     placeholders.
  2. **Event decorators** — `click`, `submit` (SNIPPETs con
     `="$1"` tail) tras `@` en attribute position.
  3. **State field names** del enclosing component.
  4. **Event handler names** del enclosing component.
- **Robust to partial parses** — heuristic scan del source raw
  (line-by-line, brace-depth tracking) extrae state field
  names + event names incluso cuando `view::parse` rejects
  (unterminated `{`, mid-typing). Trade-off: false positives
  cosméticos (extra items en la lista). Cero false negatives
  en shapes reales.
- LSP bin `completion` handler routea `.fitzv` via el nuevo
  helper; classic sigue por el path K-4 con `imported_names`.

### 11.8.c — Hover inside `.fitzv` (~200 LoC + 4 tests)

- Nueva `hover_at_position_view(source, line, character) ->
  Option<Hover>` — sobre bare ident del cursor devuelve markdown
  con code fence + label:
  - State field ref → `<name>: <type> — **state field** of
    <Component>`.
  - Event handler ref → `event <name>() — **event handler** of
    <Component>`.
- Keyword filter — `component`/`state`/`event`/`from`/`import`/
  `as`/`template`/`style` return None (evita false positives).
- Ident-under-cursor inline sweep con `is_ident` predicate
  (char-based, no lexer pass).
- Type hint extraído via `extract_field_type_from_line` — trim
  al `=` del default o EOL.
- LSP bin `hover` handler routea `.fitzv` via el nuevo helper;
  classic sigue por TypeInfo.

### 11.8.d — Go-to-definition inside `.fitzv` (~120 LoC + 3 tests)

- Nueva `definition_at_position_view(uri, source, line,
  character) -> Option<Location>` que retorna el Location del
  decl-line del ident bajo el cursor:
  - State field ref → salto a `<name>: <type>` line en el state
    block (columna en el `<name>` position, no en el indent).
  - Event handler ref → salto a `event <name>(...)` line en el
    component.
- Range del ident: 5 chars para `count`, etc. — start.character
  al inicio del ident, end.character + longitud.
- Component boundary respect — si el cursor está en `App` y el
  next `component X { count: ... }` también tiene `count`,
  goto-def apunta al de `App`.
- LSP bin `goto_definition` handler routea `.fitzv` via el
  nuevo helper; classic sigue por `DefinitionInfo`.

### Deudas residuales derivadas (NO bloquean)

- **Fine-grained context routing** — el completion MVP no
  distingue "cursor inside template" vs "inside state block" —
  las suggestions siempre son correctas pero pueden aparecer en
  contexts adjacentes. Refinable si false-positive noise
  aparece en práctica.
- **Cross-module symbol lookup** — hover/go-to-def sobre un
  ident importado por `from X import Y` no salta al target
  module hoy. Refinable con plumbing paralelo al
  `resolve_cross_module_definition` del path classic.
- **TypeInfo-based hover** — hover MVP usa heuristic scan del
  source; una integración full con el classic checker corriendo
  sobre el emitted classic Fitz surface daría más precision
  (para bare ident refs en event body con complex expr shapes).
  Sub-session B.2 si aparece demanda.

### Verificación pre-bump

- **3741/3741** lib tests verde (default features, sin cambios
  vs v0.21.2 — el path `.fitzv` es opt-in via `--features
  lsp`).
- **3897/3897** lib tests con `--features lsp` verde
  (+156 vs default: los 20 tests B11.8 nuevos + 136 tests LSP
  pre-existentes).
- **115/115** cli_e2e verde.
- `cargo fmt --all --check` limpio.
- `cargo clippy --lib --tests --release --features lsp -- -D
  warnings` limpio.
- Smoke real Board.fitzv del kanban (`fitz-liveviews`):
  `fitz check` verde. Sin regresión al SFC de Session A.
- Docs/curso/boilerplates/examples: verificados sin necesitar
  cambios (`.fitzv` LSP support es transparente al user
  editing; docs pedagógicos son scope Session C).

### Bump

- `Cargo.toml` 0.21.2 → 0.21.3.
- `editors/vscode/package.json` 0.21.2 → 0.21.3.
- `.vsix` regenerado bundleando `fitz-lsp.exe` fresh v0.21.3.
- `fitz.exe` global reinstalado en `~/.fitz/bin` +
  `~/.cargo/bin`.

## [v0.21.2] — 2026-07-17 — Phase 11 Session A: Small residual debts (S.1 alias imports + S.2 Map<Str,Str> static props + S.3 type-check interpolated)

**Patch release aditivo** que cierra 3 de las 6 deudas residuales
menores de Phase 11 (memoria `feedback_post_changes_smoke_examples_
boilerplates` verificada — sin ejemplos existentes rotos, sin cursos
afectados). Sin cambios breaking. Descubiertas + fixed en secuencia
en una sola sesión.

### S.1 — Alias en imports SFC (`from X import Y as Z`)

El view lexer aprende `Token::As`; el parser view acepta `<original>
as <alias>` post-ident en las listas de nombres de `from ... import
...`. `ViewImport.names` cambia de `Vec<String>` a `Vec<(String,
Option<String>)>` mirroring el classic Fitz `Stmt::FromImport`
(PreF8.4). El SSR emitter aplana los aliases a `imported_names`
(K-4) usando el ALIAS cuando existe (el binding local en el SFC) y
emite `from X import Y as Z` verbatim en el classic Fitz module
transformado (el loader valida contra `Y` en el módulo origen y
bindea `Z` en local scope). Sin cambio breaking — programas con
`from X import Y` (sin alias) siguen igual. **~120 LoC + 6 tests
nuevos** (3 parser + 3 SSR emitter).

### S.2 — `Map<Str, Str>` static props via `k=v,k=v` convention

`<Child meta="role=admin,scope=full" />` con `meta: Map<Str, Str>`
coerciona a `vec![("role".to_string(), "admin".to_string()), ...]`
(checker + WASM) y a `{"role": "admin", "scope": "full"}` (SSR).
Empty string → `vec![]` / `{}`. Whitespace around `,` AND around
`=` trimmed. Restrict to `Map<Str, Str>` only — richer key/value
types (Int, Bool, etc.) rejected with clear pointer al workaround
(interpolación `<Child meta="{someMap}" />` from K-3 remainder,
que soporta cualquier shape). WASM `type_expr_to_rust` extendido
para aceptar cualquier `Map<K, V>` (emite `Vec<(K, V)>`) — la
restriction Str,Str vive solo en el coerce path del static prop.
`default_expr_to_rust` extendido para `Expr::Map` contra
`Map<K, V>`. **~180 LoC + 7 tests nuevos** (4 check + 3 SSR + 4
WASM; 2 stale rejection tests updated to positive).

### S.3 — Type-check estático del expr interpolado vs field type

`<Child prop="{expr}" />` ya no bypassea silenciosamente el check
cuando `expr` es un bare `Ident` que refiere a un parent state
field. Nueva fn `light_check_interpolated_prop` en `view/check.rs`
que hace type comparison estructural (`type_expr_compatible`) entre
el parent state field type y el child field type. Regla: `T` es
compatible con `T?` (assignment lifts to Some), otherwise
structural equality. Richer expr shapes (BinOp, Call, Field access,
etc.) siguen skipping — full expr type inference is out of scope
para un chico fix. La classic-checker downstream sigue catcheando
mismatches profundos. Con esto un typo como `<Card num="{title}"
/>` con `title: Str` y `num: Int` en el child surface en check
time en vez de propagarse hasta el emitted module. **~85 LoC + 5
tests nuevos** (5 check E2E via check_str).

### S.6 — Cross-file `<Child />` composition — **DIFERIDO**

Design decision needed: convention-based (trust classic checker
para mismatched fields, leaks abstraction) vs proper loader
integration (plumbing bigger). Ninguno de los 4 ejemplos de
`fitz-liveviews` lo usa, Board.fitzv tampoco. **Se ataca cuando
llegue el ejemplo grid + forms del companion UI library como
driver concreto** (memoria `project_liveviews_roadmap_options`
menciona kanban showcase + grid/forms). No bloquea uso real.

### Verificación pre-bump

- 3741/3741 lib tests verde (baseline 3719 + 22 tests nuevos —
  22 additions netos, cuenta:
  6 S.1 + 7 S.2 (4 check + 3 SSR) + 5 S.3 + 4 S.2 WASM = 22).
- 115/115 cli_e2e verde.
- 3/3 openapi_e2e verde.
- `cargo fmt --all --check` limpio.
- `cargo clippy --lib --tests --release -- -D warnings` limpio.
- Verificación docs/curso/boilerplates/examples: `.fitzv` examples
  no usan alias imports ni Map<Str,Str> static props (nothing to
  update); classic Fitz `from X import Y as Z` ya existía
  (documented in curso M3.C1 desde v0.9.x, sin cambios).
- Extensión VSCode grammar TextMate — `as` keyword ya listado
  desde versiones anteriores (classic Fitz feature). Sin cambios
  al grammar, sin snippets afectados.

### Bump

- `Cargo.toml` 0.21.1 → 0.21.2.
- `editors/vscode/package.json` 0.21.1 → 0.21.2.
- `.vsix` regenerado bundleando `fitz-lsp.exe` fresh v0.21.2.
- `fitz.exe` global reinstalado en `~/.fitz/bin` + `~/.cargo/bin`.

## [v0.21.1] — 2026-07-17 — Phase 11 refinements: K-3 (compound + interpolated props) + K-4 (imported fn refs) para SSR

**Patch release** que agrupa tres refinements post-v0.21.0 al SFC
pipeline (SSR path). Todos son aditivos, sin cambios breaking.
Descubiertos + fixed durante la Board.fitzv migration probe en
`fitz-liveviews` — el kanban SFC full-page migration los usa
end-to-end. Con este bundle, la Board migration deja de necesitar
workarounds (top-level fns inlined en cada event body, `<Child />`
composition ugly, no computed values en template) y queda "prolija,
facil, con arquitectura clara" — la triple splittéalo por
responsabilidades: types en `.fitz`, helpers puros en `.fitz`,
component SFC en `.fitzv`, HTTP + WS wire-up thin en `.fitz`.

### K-3 (partial) — `<Child prop />` static props para `List<primitive>` — post-v0.21.0 (2026-07-16)

**Refinement** del feature `<Child prop="v" />` composition
introducido en Phase 11.5.d, sin bump del lenguaje. El path de
prop coercion (checker + WASM + SSR emitters) ahora acepta
`List<T>` donde `T` es un primitivo soportado (o
`Nullable<primitive>`) via **comma-separated raw values**.

- `<Child tags="a,b,c" />` con `tags: List<Str>` en el child
  coerciona a `vec!["a".to_string(), "b".to_string(),
  "c".to_string()]` en el path Rust literal (checker + WASM
  emitter) y a `["a", "b", "c"]` en el path Fitz literal (SSR
  emitter).
- Empty string yields `vec![]` / `[]`. Whitespace around commas
  is trimmed.
- Nested primitives (`List<Int>`, `List<Bool>`,
  `List<Nullable<Int>>`) recurse via el mismo helper.
- WASM state fields con `List<T>` gain Rust type `Vec<T>` +
  default `vec![...]` / `Vec::new()` (wrapped en `RefCell` por
  el existing struct emitter).

**Deudas residuales** de K-3 que siguen abiertas (documentadas
en `docs/deudas-post-5b.md` → K-3): `Map<K, V>` static props,
nominal-type static props (`<Card user="{seedUser}" />`), e
interpolated `<Child prop={expr} />` composition. Ninguna
bloquea el 90% del caso (primitive list props tipo tag arrays,
dropdown options, chart series).

**Sin cambio breaking**: los programas existentes se comportan
bit-a-bit idénticos (el path viejo solo rechazaba estos casos —
ahora los acepta). Cierra la sección "K-3" de las Framework
gaps parcialmente (List<primitive> shipped; nominal +
interpolación siguen abiertas).

**Cambios técnicos**: ~130 LoC netos, 21 unit tests nuevos + 2
E2E via `check_str` reemplazando el test stale que asertaba
rejection (`phase_11_5_d_check_list_prop_rejects_citing_11_6`
→ `k3_check_list_int_prop_end_to_end_accepts_comma_separated_
values` + `k3_check_map_prop_still_rejects_citing_11_6_or_
later`).

### K-3 (remainder) — `<Child />` interpolated props `prop="{expr}"` para SSR — post-v0.21.0 (2026-07-16)

**Cierre completo de K-3** — la segunda de las dos sub-releases
que juntas cierran el gap "compound / interpolated child props"
para el SSR path entero. Con esto **Board.fitzv migration queda
desbloqueada** para el target fitz-liveviews.

- **Parser + expand**: `<Card label="{title}" />` (donde
  `label="{expr}"` es reconocido por `extract_full_interp` en el
  POC parser) ya no aborta con "dynamic prop deferred to 11.6+".
  El expander parsea el `expr_raw` a un `fast::Expr` clásico via
  `parse_expr_at` (mismo helper usado para template interpolations)
  y lo guarda en `ChildComponentProp.expr: Option<fast::Expr>`.
  Helper nuevo `ChildComponentProp::is_interpolated()` discrimina
  el shape.
- **Checker**: cuando `is_interpolated()`, skippea `coerce_child_
  prop_raw_value` (trust runtime). Type-check del expr vs field
  type queda deferrable si false negatives aparecen.
- **SSR emitter (`format_child_composition`)**: dispatch on
  `is_interpolated()`. Static path usa el coerce helper como antes.
  Interpolated path corre el expr por `format_fitz_expr_scoped`
  con el `state_field_names` + `local_scope` del PARENT — bare
  ident `title` en el expr rewritea a `state.title` (la parent's
  state field), no la child's. Closure-parameter locals (`{#for x
  in xs}` alrededor del `<Child />`) también shadow via el
  `local_scope` del parent.
- **WASM emitter**: rechaza interpolated props con mensaje claro
  citando Phase 11.7+ y el workaround (static value o composición
  top-level). Razón: reactive prop propagation from parent state
  to mounted child needs child-lifecycle hooks + prop watchers.

**Patterns unlocked** (SSR target, `fitz-liveviews`):
- `<Card label="{title}" />` — bare state field reference.
- `<Card count="{n + 1}" />` — inline arithmetic (any expression
  the parser accepts as a classic Fitz expr).
- `<Board initial-cards="{cards}" />` — nominal / compound types
  pass through naturally via the interpolation (no static-value
  serialization convention needed).
- Mixed static + interpolated on same child: `<Card
  label="{title}" kind="primary" />`.

**Sin cambio breaking**: el shape `prop="value"` sigue funcionando
idéntico. Solo cambia el shape `prop="{expr}"` — antes fallaba,
ahora funciona en SSR. WASM sigue rechazando con mensaje que
apunta al Phase 11.7+ scope real (reactivity, no un mero
"defer everything").

**Cambios técnicos**: ~120 LoC netos + 5 unit tests nuevos +
1 test flipped from stale rejection (`phase_11_5_d_expand_child_
component_dynamic_prop_rejects_citing_11_6` → `k3_interp_expand_
child_component_accepts_dynamic_prop_and_parses_expr`). Refactor
signature de `format_child_composition` para tomar
`parent_state_field_names` + `parent_local_scope` (data ya
disponible en el caller). 3711 → 3715 lib tests green, fmt +
clippy limpios.

### K-4 — SSR emitter acepta imported top-level fn refs en templates + event bodies — post-v0.21.0 (2026-07-16)

**Discovered + fixed** during Board.fitzv migration probe: el SSR
emitter rechazaba llamadas a top-level fns importadas via `from X
import Y`, forzando workarounds verbose (inline logic repetido o
top-level fns dentro del `.fitzv` que el parser rechaza). K-4
extiende `format_fitz_expr_scoped` para resolver bare Idents
contra la imports table del file (`ExpandedViewFile.imports`, ya
poblado por §9.dd).

- Nuevo param `imported_names: &[&str]` en `format_fitz_expr_
  scoped`. Resolution order: local_scope > state_field >
  imported_name > error.
- Threading: `emit_module_ssr` aplana `file.imports` en un slice
  de nombres y lo pasa por toda la cadena (~30 call sites de
  `emit_component_ssr_into` → template + event body emitters
  → `format_child_composition` → wrapper `format_fitz_expr`).
- Error message cuando el ident no es state field ni imported
  ahora menciona la imports table como fix hint (antes solo citaba
  el generic Phase 11.7+ pointer).
- `lower_event_body_stmts` gana un `#[allow(clippy::too_many_
  arguments)]` — 8 params (stmts + state + local scope + imported
  + component + event + indent + out). Justificado en el comment;
  refactor a `ScopeCtx` struct queda deferrable.

**Patterns unlocked** (SSR target):

```fitzv
from helpers import cards_in, move_one

component Board {
  state { cards: List<Card> = [] }
  event move_right() {
    let target_id = payload["card_id"]
    cards = cards.map(fn(c) => move_one(target_id, "right", c))
  }
  <template>
    {#for c in cards_in(cards, "todo")}
      <li>{c.title}</li>
    {/for}
  </template>
}
```

Board.fitzv migration en `fitz-liveviews` ahora puede separar
helpers puros en un `.fitz` classic sibling e importarlos
naturalmente al SFC — arquitectura limpia (state + event +
template en el SFC, lógica pura en el módulo helper).

**Sin cambio breaking**: el path viejo con state fields + closure
params sigue igual. Solo cambia el rechazo del ident desconocido —
ahora consulta la imports table antes de errorear.

**Cambios técnicos**: ~200 LoC netos + 4 unit tests nuevos
(`k4_ssr_template_can_call_imported_fn_from_from_import`,
`k4_ssr_event_body_can_call_imported_fn_via_closure_arg`,
`k4_ssr_unknown_ident_still_errors_with_updated_hint`,
`k4_ssr_local_shadows_imported_name`). 3715 → 3719 lib tests
green, fmt + clippy limpios.

## [v0.20.1] — 2026-07-13 — Implicit `flv_register(...)` for LiveView components (fitz-liveviews Phase 5, A.1)

Primer sub-paso de la Fase 5 diferida post-Phase-4 de `fitz-liveviews`.
El compilador auto-genera una llamada `flv_register("name", InitialState
{}, render_fn, {"event": handler})` por cada tipo con `@live_component`,
consumiendo la metadata que `resolve_program` ya persiste en `TypeEnv`
(`live_components`, `render_handlers`, `event_handlers`). Sin breaking:
la API pública de `fitz-liveviews` es idéntica; los programas con manual
`flv_register(...)` siguen funcionando y toman precedencia sobre la
inyección implícita.

**Nueva helper** en `src/types.rs`:

- `pub fn inject_live_component_registrations(program: &mut Program,
  env: &TypeEnv) -> Result<(), Vec<FitzError>>` — walker
  determinístico (componentes ordenados por nombre) que appendéa un
  `Stmt::Expr(Call flv_register(...))` sintético por cada
  `@live_component`. Validaciones en tiempo de inyección:
  - `@live_component` sin `@render_for` matching → error claro citando
    el fn que falta.
  - Field sin `default` en un `@live_component` type → error citando
    el field (el `TypeName {}` sintético requiere defaults).
  - `flv_register` no está en scope (ni por `from fitz_liveviews import
    flv_register` ni por `fn flv_register` local en tests) → error
    sugiriendo el import canónico.
  - User ya llamó `flv_register("name", ...)` manualmente → skip
    (idempotencia sin conflicto, respeta intent explícito).
  - Sin `@live_component` en el programa → no-op (return Ok early).

**Wiring en el pipeline**: `run_file`, `build_file` y
`build_file_with_bundle` en `src/main.rs` llaman la helper después de
`check_program_with_pyi_stubs` y antes de eval/codegen. Los errores de
inyección abortan (exit code 1) con el mismo formato que los errores
del checker. `fitz check` no llama la helper — el chequeo es de
build/run time.

**Smoke real**: `examples/kanban/` y `examples/dashboard/` de
`fitz-liveviews` compilan y corren sin el `flv_register(...)` manual
que exigía Phase 4. El dashboard sirve HTML con
`data-flv-component-name="metric_tile"` proving auto-registration
end-to-end en `fitz run`.

**9 unit tests nuevos** en `src/types.rs::tests::implicit_register_*`:
inyección básica, no-op sin componentes, skip cuando manual call
existe, missing `@render_for` error, field sin default error, missing
`flv_register` en scope error, alias de import trata como out-of-scope,
orden alfabético, componente sin events emite Map vacío.

**Deudas residuales derivadas** (NO bloquean uso real):
- Cross-module `@live_component` NO soportado en este MVP — solo
  componentes declarados en el program top-level. Paralelo a
  `imported_auth_provider` / `imported_background_fns` (W12, B10) —
  refinable cuando aparezca demanda real.
- `from fitz_liveviews import flv_register as register` deja
  `flv_register` fuera de scope canónico → error claro con fix; los
  usuarios simplemente no aliasan el nombre.
- Custom initial state con valores distintos de los defaults del type
  requiere manual `flv_register(...)` explícito — el implicit sintetiza
  `TypeName {}` que usa exclusivamente defaults del type.

**Próximo norte tras v0.20.1**: los otros dos items de Phase 5 de
`fitz-liveviews` (per-instance init + `dispatch_to_all`) son features
NUEVAS; se deciden con datos empíricos de un showcase post-cierre.
Sin presión al cierre.

## [v0.20.0] — 2026-07-12 — Language surface para LiveView components (Phase 4 Y-B) + template CLI

Release menor con nueva superficie del lenguaje habilitando componentes
LiveView-style construidos sobre `fitz-liveviews`. Tres decoradores
nuevos (`@live_component`, `@render_for`, `@on`) validados por el
checker, más el sub-comando `fitz new --template <name>` que scaffoldea
proyectos desde un repo git (arranque canónico:
`fitz new my-app --template liveviews`). Sin breaking; los programas
existentes compilan y corren bit-a-bit igual.

**Nuevos decoradores** (validados por el checker en `src/types.rs`;
la ejecución la provee la lib `fitz-liveviews`):

- **`@live_component("name")`** sobre un `type` — registra el type como
  componente stateful. Exactamente 1 arg Str literal, sin kwargs, uno
  por type. El checker persiste `LiveComponentMetadata { name, type_id }`
  en el `TypeEnv` con lookup por `TypeId` y por nombre.
- **`@render_for("name")`** sobre un `fn` — marca la fn como el renderer
  del componente `"name"`. Firma esperada: `fn(state: T) -> Html` (o
  `-> Str`), con T = tipo que declara `@live_component("name")`.
  Persiste `RenderForMetadata { component_name, fn_name }`.
- **`@on("component", "event")`** sobre un `fn` — registra un handler
  para un evento cliente-side (`data-flv-click`/`data-flv-submit`/etc)
  del componente. Firma: `fn(state: T, payload: Map<Str, Str>) -> T`.
  Persiste `EventHandlerMetadata { component_name, event_name, fn_name }`
  con lookup por `(component_name, event_name)`.

**Template CLI** — `fitz new --template <name>` clona un repo git a un
dir temporal, copia el sub-path declarado al nuevo proyecto, sustituye
`{{name}}` en cada archivo UTF-8 con el nombre del proyecto, y corre
`git init` (skippable con `--no-git`). El flag es mutuamente
excluyente con `--http`. Módulo nuevo `src/templates.rs` (~520 LoC)
con:

- **Registry built-in**: hoy solo `liveviews` →
  `https://github.com/Thegreekman76/fitz-liveviews` en `templates/basic`
  sobre `main`. Refinable por PR si aparecen más.
- **Override por env vars**: `FITZ_TEMPLATE_LIVEVIEWS_URL/SUBPATH/REF`
  cambian solo templates conocidos (no permiten registrar nuevos).
  Sirve para tests + power users que apuntan a forks o subpaths
  alternativos.
- **Clone estrategia split** (paralelo a `git_dep::clone_fresh` de
  9.y.3.c): `--depth 1 --branch <ref>` para tags/branches, full clone
  + `git checkout` fallback para commits específicos.
- **Sustitución `{{name}}`**: solo en archivos UTF-8 decodables (los
  binarios se copian byte-por-byte). `.git/` interno del template NO
  se propaga al nuevo proyecto.
- **`TempDir` in-house**: evita promover `tempfile` de dev-dep a runtime
  dep. Auto-cleanup en `Drop`.

**LSP** (v0.20.0) — `decorator_completions()` suma los 3 decoradores
nuevos con snippets tabstop-guided:

- `@live_component("component_name")` con placeholder editable.
- `@render_for("component_name")` con placeholder editable.
- `@on("component_name", "event_name")` con dos placeholders.

Extensión VSCode bumpeada a **0.20.0** con `.vsix` regenerado
bundleando el `fitz-lsp.exe` fresh. Grammar TextMate sin cambios
(`@<ident>` genérico ya cubría los decoradores nuevos por regla).

**Tests** al cierre v0.20.0:

- `cargo test --lib --release` → verde (3229+ tests, incluye 5 tests
  nuevos del `@live_component`, 8+ nuevos de `@render_for`/`@on`, 8
  nuevos de `templates.rs`)
- `cargo test --lib --release --features lsp` → verde
- `cargo test --lib --release --features python` → verde
- `cargo test --test cli_e2e --release` → verde (101 total, +3 nuevos:
  scaffold OK con env var override, error claro sobre template
  desconocido, `--template` + `--http` mutuamente excluyentes)
- `cargo test --test openapi_e2e --release` → verde (3 total)
- `cargo test --test compile_e2e --release` → 377 verde / 8 pre-existentes
  ya documentados (file-lock Windows race sobre `handler_panic_r6` +
  `hidden_decorator_v0_10_11`, codegen cross-module + observability,
  routing 404, orm_w17 #7 drift). Delta de 1 test respecto al último
  run verificado (378/7 en commit `556441c`) es file-lock flake sobre
  Windows entre corridas — sin regresión imputable al diff de v0.20.0
  (LSP-only + fmt-en-cli_e2e; el binary de compile_e2e no toca esos
  paths)
- `cargo fmt --all --check` limpio
- `cargo clippy --lib --tests --bins -- -D warnings` limpio
- `cargo clippy --lib --tests --bins --features lsp -- -D warnings` limpio

**Ejemplo end-to-end**:

```bash
$ fitz new my-app --template liveviews
✓ scaffolded my-app from template `liveviews`

$ cd my-app && fitz run
Server listening on http://127.0.0.1:3000
```

Los tres decoradores son inertes runtime-side en Fitz core (metadata
puro registrada por el checker); la lib `fitz-liveviews` v0.3.1+
provee `component(name, id)` + state store + `dispatch_component_events`
que los consumen para renderizar y despachar eventos.

**Deudas residuales derivadas** (NO bloquean uso real): dispatch
runtime por nombre (`invoke_by_name`) queda como sub-paso futuro de
Fitz core si la lib fitz-liveviews lo pide (hoy resuelve con lookup
en `TypeEnv` + registro estático); imports auto de `fitz-liveviews`
al usar `@live_component` (hoy el user debe declarar la dep en su
`fitz.toml`, comportamiento explícito acorde al modelo del PM);
registro de templates externos por manifest (hoy solo built-in +
env overrides, refinable con `[template.custom]` cuando aparezca
demanda real).

## [v0.19.6] — 2026-06-27 — Bugfix codegen: sub-caso v0.19.5 — wrapper HTTP en módulo importer del middleware

Bugfix release patch que cierra el sub-caso 🔴 URGENTE del bug original
de v0.19.5, descubierto la misma noche del cierre de v0.19.5 durante
el refactor real del patrón canónico "módulo dedicado a rate limiting +
handlers en módulos separados". El fix de v0.19.5 cubrió correctamente
el módulo del middleware (emite `use crate::{Request, RequestData}` en
`rate_limit.rs`), pero NO cubría el módulo IMPORTER del middleware
(donde vive el handler con `@middleware(<imported_fn>)` aplicado). El
wrapper HTTP emitido en `auth.rs`/`subscriptions.rs` construía
`__req: Request = Arc::new(... RequestData { ... })` para pasarlo al
middleware cross-module, pero el detector `program_uses_request_type`
NO disparaba para esos módulos porque ninguna fn local declaraba
`Request` en su firma — solo lo aplicaban como decorator. **14 errores
rustc** al hacer `fitz build` (7 endpoints × 2 errores cada uno: `E0425
cannot find type Request` + `E0422 cannot find struct, variant or
union type RequestData`).

**Fix** (~50 LoC netas, 2 archivos `src/codegen.rs` + `tests/compile_e2e.rs`,
1 E2E test): detector nuevo `program_has_handler_with_middleware`
walka el AST buscando handlers HTTP (`@get/@post/@put/@delete`) que
tienen al menos un decorator `@middleware(...)` aplicado, sin importar
si el ident del middleware resuelve local o cross-module. Cuando
dispara, el call site de `generate_module_rs_with_bindings` que decide
emitir `use crate::{Request, RequestData}` lo agrega como nuevo OR a
los dos predicados existentes (`module_uses_request_local` +
`module_has_imported_middleware_fn`). El costo del `use` extra en
módulos benignos es despreciable (rustc dead-code elimina si no se
usa) y elimina la posibilidad de regresión del bug en el otro sentido
(módulo declara handler con `@middleware(localfn)` y la fn local NO
usa `req: Request` en su firma — pre-fix técnicamente OK porque el
middleware local sí emitía el import via path antiguo, pero ahora doblemente
robusto).

**E2E test nuevo** `v019_6_cross_module_middleware_applied_in_importer_module_emits_request_imports`
(`tests/compile_e2e.rs`) reproduce el shape canónica de fitzwatch con
3 archivos (`mw.fitz` + `handlers.fitz` + `main.fitz`): middleware
declarado en mw, handler con `@middleware(mw_strict)` declarado en
handlers (módulo importer), main solo importa el handler para
mountarlo. Inspecciona el `handlers.rs` emitido confirmando
`use crate::{Request, RequestData}` (o split forms). Paralelo
bit-a-bit al test `v019_5_cross_module_middleware_fn_con_request_arg_compila`
que cubre el otro lado del bug.

**Workaround user-land aplicado en fitzwatch (sesión 2026-06-27 noche)
removible tras v0.19.6**: fn dummy `_codegen_request_anchor(req:
Request) -> Bool => true` declarada al inicio de `auth.fitz` y
`subscriptions.fitz` forzaba al detector `program_uses_request_type`
a disparar (el AST del módulo tenía `Request` en TypeExpr del param).
Costo: 2 fns extra en el `.rs` emitido (zero cost al runtime). Tras
bumpear `FITZ_TAG=v0.19.5 → v0.19.6` en fitzwatch, las dos fns se
quitan y el patrón canónico funciona sin workaround.

**Sin regresiones**: 3201 lib + 98 cli_e2e + 3 openapi_e2e + 14
v019_ E2E (incluyendo el nuevo) + smoke `GUIDE_EXAMPLES_COMPILE`
verde. `cargo fmt --all --check` + `cargo clippy --lib --tests --bins
-- -D warnings` (default + lsp) limpios. Bump Cargo.toml `0.19.5` →
`0.19.6` + extensión VSCode `0.19.5` → `0.19.6` + `.vsix` regenerado
bundleando el `fitz-lsp.exe` fresh (sin cambios al grammar TextMate ni
runtime LSP — bug interno del codegen, cero impacto en surface de
editing). Detalle completo en `docs/deudas-post-5b.md` →
"🟢 Sub-caso v0.19.5 — wrapper HTTP en módulo IMPORTER del middleware
cross-module: CERRADO v0.19.6".

## [v0.19.5] — 2026-06-27 — Bugfix codegen: cross-module `@middleware(fn)` + `Request` destrabados

Bugfix release que cierra una deuda 🔴 URGENTE descubierta el
2026-06-26 al implementar rate limiting en fitzwatch (SaaS privado
del autor) usando el patrón canónico "módulo dedicado a
cross-cutting concerns + handlers en módulos separados". Tres
síntomas distintos del codegen bloqueaban el patrón:

1. **Síntoma 1**: el checker del loader sobre un módulo aislado
   (e.g. `rate_limit.fitz` con `fn mw_block(req: Request) {
   return 429 { ... } }`) rechazaba `return <status> { ... }`
   porque el pre-scan local no veía el `@middleware(mw_block)`
   aplicado desde otro módulo.
2. **Síntoma 2**: cuando módulo declaraba helpers con `req:
   Request` en su firma, el `.rs` emitido referenciaba `Request`
   /`RequestData` sin `use crate::{...}` correspondiente y rustc
   abortaba con E0425/E0422.
3. **Síntoma 3**: en main, `@middleware(mw_strict)` donde
   `mw_strict` venía de `from rate_limit import mw_strict` fallaba
   build-time con "the fn is not defined in this program" porque
   el check solo consultaba `self.fn_sigs` (locales), no
   `module_bindings` (imports).

Bloqueaba el patrón canónico "módulo `rate_limit.fitz` / `audit.fitz`
/ `cors.fitz` que define middlewares reutilizables aplicados desde
varios módulos de handlers" — encarecía todo proyecto Fitz HTTP de
producción. fitzwatch tuvo que abandonar `@middleware` cross-module y
duplicar el check inline en 7 endpoints (~42 LoC duplicadas) como
workaround temporal.

**Fix** (~280 LoC netas, 2 archivos `src/codegen.rs` + `src/types.rs`,
3 E2E tests):

1. **Pre-scan global de `@middleware(fn)` references** —
   `pre_scan_imported_middleware_fns_for_loader` walka main + todos
   los módulos importados (recursivo) y construye el set GLOBAL de
   fn names referenciadas como middleware en cualquier punto del
   árbol del proyecto. Helper público nuevo
   `crate::types::extract_middleware_fn_names`. Paralelo a W12
   (`@auth_provider`) + B10 (`@background`) cross-module pre-scan.
2. **Propagación al checker** — `TypeEnv` suma campo
   `imported_middleware_fns: HashSet<String>` paralelo a
   `imported_background_fns`. Setter `add_imported_middleware_fns`.
   `collect_middleware_fn_names` del checker mergea el set al
   `ctx.middleware_fn_names` antes del walk.
3. **Propagación al codegen del módulo** —
   `generate_module_rs_with_bindings` recibe parámetro nuevo
   `cross_module_middleware_fns: &[String]` y pre-inserta los
   nombres en `ctx.middleware_fn_names` ANTES de `pre_register_fns`.
   La post-scan que clasifica por aridad (1=pre/2=post) ve la unión
   y emite el Rust return type correcto (`Option<__FitzResponse>`)
   + activa `in_middleware_fn=true` para los `Stmt::ReturnStatus`
   del body.
4. **`@middleware(<imported_fn>)` aceptado en main** —
   `collect_route_middlewares` (línea ~28171) ahora consulta tanto
   `self.fn_sigs` (locales) como `self.module_bindings` para
   `ResolvedBinding::Named { kind: NamedKind::Fn }` y resuelve el
   `FnSig` desde `loaded_modules[idx].fn_sigs`. Helper nuevo
   `resolve_fn_sig_anywhere`. Paralelo a `is_user_callable` (v0.9.45).
5. **`use crate::{Request, RequestData}` en módulos** — detector
   nuevo `program_uses_request_type` walka el AST del módulo
   buscando `Request` en TypeExpr (fn params/return, type fields,
   let annotations). Cuando el módulo declara helpers con `req:
   Request` (típico: `fn get_client_ip(req: Request) -> Str` en
   `rate_limit.fitz`), o cuando declara una fn que es referenciada
   como middleware desde otro módulo (todas las middleware fns
   tienen `Request` en su primer param por spec), el codegen emite
   `use crate::{Request, RequestData}` al tope del `.rs`. Paralelo
   a `program_uses_response_builtin` (v0.19.1).
6. **`use crate::{__FitzResponse, __ToFitzJson, ...}` en módulos
   de middleware puro** — cuando el módulo declara solo fns
   middleware (sin `@get`/`@post`/etc), `module_has_http` es
   `false` y el codegen pre-fix no emitía los imports necesarios
   para `Stmt::ReturnStatus`. La condición se extiende a
   `module_has_http || module_has_imported_middleware_fn` para
   `__FitzResponse` + `__apply_cors_and_respond` y para
   `__ToFitzJson`/`__FromFitzJson`.
7. **Async middleware fn cross-module bonus** — `emit_middleware_chain`
   detectaba el callsite como `mw_name(__req.clone())` siempre sync.
   Con cross-module + `async fn mw_strict(req: Request)`, la fn
   devuelve `Future<Option<...>>` y el `if let Some(...) =`
   mismatch. Helper nuevo `middleware_fn_is_async(name)` consulta el
   FnSig (local o imported) y detecta `Type::Future(_)` en el ret.
   El wrapper emite `.await` suffix condicional para los 3 paths
   (pre-mw + post-mw response + post-mw result). Paralelo a
   `gen_call` Phase 6.6. Habilita el patrón canónico
   `async fn mw_strict(req: Request) { match check_rate_limit(req,
   ...).await { ... } }` que fitzwatch necesita.

**3 E2E tests nuevos** en `tests/compile_e2e.rs` cubriendo los 3
síntomas: `v019_5_cross_module_middleware_fn_compila_a_binario_nativo`
(canónico async gate-only `return null`),
`v019_5_cross_module_middleware_fn_con_request_arg_compila`
(helper local con `req: Request` + inspección del `mw.rs` emitido
confirmando los imports), `v019_5_cross_module_middleware_fn_con_return_status_compila`
(con `return 429 { "error": "blocked" }` cross-module).

**Verificación pre-bump completa** (memoria
`feedback_pre_release_verification`): `cargo fmt --all --check` +
`cargo clippy --lib --tests --bins -- -D warnings` (default + lsp)
limpios; `cargo test --lib --release` **3201/3201** verde;
`cargo test --test cli_e2e --release` **98/98** verde;
`cargo test --test openapi_e2e --release` **3/3** verde; smoke
`GUIDE_EXAMPLES_COMPILE` (~370 ejemplos guía+curso+TaskHub) verde
en 687s; 11/11 boilerplates `fitz check` verde; validación bit-a-bit
`fitz build` produce binario que responde HTTP 200 OK al repro
mínimo cross-module.

**Bump Cargo.toml** `0.19.4` → `0.19.5` + extensión VSCode `0.19.4`
→ `0.19.5` + `.vsix` regenerado bundleando el `fitz-lsp.exe` fresh.

**Deudas residuales derivadas** (NO bloquean — documentadas en
`docs/deudas-post-5b.md`):

1. Async middleware fns en `fitz run` (intérprete) devuelven 500
   INCLUSO same-module — bug pre-existente del evaluator (NO
   regresión de v0.19.5). Workaround documentado: validar el flujo
   con `fitz build && ./binario`. El binario ahora soporta async
   middleware cross-module sin problema.
2. LSP cross-module pre-scan de `@middleware` — paralelo a la deuda
   residual que v0.19.3 cerró para `@auth_provider`/`@background`.
   El LSP abriendo un módulo aislado de middleware muestra falso
   positivo. Refinable cuando aparezca presión real.

**Impacto en producción**: fitzwatch puede activar el refactor
`@middleware(rate_limit_strict)` cross-module limpio en los 7
endpoints sensibles cuando bumpee `FITZ_TAG` a v0.19.5+, eliminando
las ~42 LoC duplicadas del workaround inline temporal.

## [v0.19.4] — 2026-06-23 — Bugfix codegen: `http.request` con headers Map literal rompía Send en `spawn`

Bugfix release que cierra una deuda 🔴 URGENTE descubierta el mismo
2026-06-23 durante el deploy real de fitzwatch.com en VPS DigitalOcean.
El bloqueo SMTP outbound del provider obligó migrar de `smtp.send`
builtin a `http.request` POST a Resend API REST con Authorization
Bearer header — el patrón canónico para HTTP outbound auth Bearer
(Stripe, Resend, OpenAI, Mailgun, etc.). El refactor falla en `fitz
build` con `MutexGuard<Vec<(String, String)>>` not `Send` cuando el
caller es `spawn(async fn)`. Mismo root cause que el bug cerrado en
v0.18.1 (`for x in List<Str>` con `.await` en `@cron`).

**Síntoma original (pre-fix)**: `fitz build` sobre un handler que
hace `spawn(notify(...))` donde `notify` es `@background async fn`
que llama `http.request({ headers: { "Authorization": "Bearer x"
}, ... }).await` aborta con 3 errors rustc del estilo
`future cannot be sent between threads safely`. El `MutexGuard` del
lock del Map literal de `headers` cruzaba el await del request porque
el codegen emitía `.lock().unwrap().clone()` inline adentro del struct
literal `__FitzHttpRequestOpts { ... }`. El stmt envolvente NO se
cerraba antes del `.await`.

**Causa raíz**: `src/codegen.rs::gen_http_request_opts` en línea 18569
emitía el field `headers:` como expresión bare
`(({Map literal}).lock().unwrap().clone())`. El `MutexGuard` temporal
vive hasta el `;` del statement enclosing, que en este caso es el
`async {}` block ENTERO (la struct_lit + await + retorno todo es UNA
sola expresión). Mismo patrón de v0.18.1 antes del fix.

**Fix** (~10 LoC del format!):

`src/codegen.rs::gen_http_request_opts` cambia:

```rust
// pre-fix (rompía Send):
format!("(({}).lock().unwrap().clone())", c)
```

por:

```rust
// post-fix (Send OK):
format!(
    "{{ let __headers_snap: Vec<(String, String)> = ({}).lock().unwrap().clone(); __headers_snap }}",
    c
)
```

El `let __headers_snap = ...;` dropea el `MutexGuard` temporal en el
`;` antes de que el block return el clone. El `headers:` field queda
con un `Vec<(String, String)>` puro — sin guard alive cuando el
`.await` del request dispara. Mismo patrón análogo al `let __for_snap
= xs.lock().unwrap().clone();` que cerró v0.18.1.

**Auditoría paralela** (no se detectaron otros sitios afectados):

- **`gen_smtp_send_opts` (smtp.send)**: opts struct lleva solo fields
  `Option<String>`. Sin Map nested. ✓
- **`gen_http_body_marshal` (body Map<Str, Str>)**: el helper
  `__fitz_http_body_from_map_str_str` toma ownership del Arc, lockea
  internamente y dropea el guard antes de retornar bytes. ✓
- **`gen_http_body_marshal` (Bytes)**: representación es `Vec<u8>`
  plano sin Mutex. ✓
- **Resto del codegen**: las ~40 apariciones restantes de
  `.lock().unwrap().clone()` usan binding `let __X = ...;` que dropea
  el guard en el `;`. ✓

**Tests nuevos** (5 — 1 unit + 4 E2E):

- `codegen::tests::v019_4_http_request_headers_map_emits_snapshot_binding_for_send`
  — unit del codegen: asegura `let __headers_snap: Vec<(String,
  String)>` presente en el field `headers:` del callsite del struct
  literal.
- `compile_e2e::v019_4_http_request_with_headers_map_spawn_compila` —
  repro mínima single-file: `@background` + `http.request(headers Map)`
  + handler con `spawn(notify(...))`. Pre-fix: aborta build.
- `compile_e2e::v019_4_http_request_with_body_map_spawn_compila` —
  mismo case pero con `body: Map<Str, Str>`. Asegura paridad bit-a-bit
  (el body Map ya cerraba el guard internamente; este test protege
  contra regresión).
- `compile_e2e::v019_4_http_request_cross_module_spawn_compila` —
  variante con `send_email` declarado en módulo importado. Matchea
  el shape real de fitzwatch (emails.fitz → notify.fitz → main.fitz).
  Combinación del fix de v0.19.2 + v0.19.4.
- `compile_e2e::v019_4_regression_v018_1_for_list_str_await_in_cron_no_send_break`
  — regression sobre el case análogo de v0.18.1. Asegura que el nuevo
  fix no introduce regresión en el path paralelo del for loop con
  `.await`.

**Verificación pre-bump** (zero regresión): `cargo fmt --all --check`
limpio + `cargo clippy --lib --tests --bins -- -D warnings` limpio +
`cargo clippy --lib --tests --bins --features lsp -- -D warnings`
limpio + lib suite 3201/3201 verde + cli_e2e 98/98 verde + openapi_e2e
3/3 verde + 4 E2E nuevos verde.

**Impacto en producción**: fitzwatch deploy 2026-06-23 puede activar
el refactor `smtp.send → http.request` Resend REST. Welcome emails +
incident notify outbound destrabados sin workaround user-land. El
patrón canónico de HTTP outbound con Authorization Bearer (caso 99% de
APIs REST modernas) compila desde context spawned.

**Bump Cargo.toml** `0.19.3` → `0.19.4` + extensión VSCode `0.19.3` →
`0.19.4` + `.vsix` regenerado bundleando el `fitz-lsp.exe` fresh.
Sin cambios al grammar TextMate ni completion del LSP (bug interno del
codegen, cero impacto en surface de editing).

## [v0.19.3] — 2026-06-23 — Bugfix LSP: `@auth_provider` / `@background` cross-module no se resuelven

Bugfix release que cierra una deuda 🟡 descubierta el mismo 2026-06-23
durante el smoke real del fix v0.19.2 sobre fitzwatch (apps
multi-módulo). El LSP corría el checker sobre el archivo abierto en
aislamiento y emitía **falsos positivos** del estilo
`@authenticated on fn 'X': no @auth_provider registered in the program`
cuando el provider vivía en otro módulo (patrón W12 cross-module),
aunque la build real (`fitz check`/`fitz build`/`fitz run`) ya
resolvía correctamente vía el pre-scan de W12/B10. Misma patología
también para `spawn(<imp_fn>(...))` contra `@background` declarado en
módulos hermanos (B10).

**Síntoma original (pre-fix)**: app multi-módulo (provider en
`auth.fitz`, handlers protegidos en `incidents.fitz` / `posts.fitz`
con `import auth`) abierta en VSCode mostraba squiggles rojos +
mensajes `no @auth_provider registered ...` en cada handler decorated.
El `fitz check` desde la terminal y el binario producido por
`fitz build` corrían sin errores — la divergencia diagnostic /
realidad confundía al developer.

**Causa raíz**: `src/lsp.rs::check_source_with_types` llamaba a
`check_program(&program)` directo, sin cargar el grafo de imports.
El codegen (`pre_scan_imported_auth_provider_for_loader`) y el CLI
(`main.rs::pre_scan_imported_auth_provider`) sí cargaban el grafo
entero — el LSP era el único path aislado.

**Fix** (~280 LoC netos, 4 archivos):

1. **`src/lsp.rs`** — wrapper nuevo
   `check_source_with_types_and_base_dir(source, base_dir: Option<&Path>)`
   con firma de 5-tupla idéntica a `check_source_with_types`. Cuando
   `base_dir` es `Some`, corre `resolve_program_with_env` para
   pre-poblar nominales locales, pre-scanea las imports directas via
   nuevos helpers privados `pre_scan_imported_auth_provider_lsp` /
   `pre_scan_imported_background_fns_lsp` (paralelos bit-a-bit a los
   de `main.rs`), enriquece el `TypeEnv` via
   `set_imported_auth_provider` + `add_imported_background_fns`, y
   llama a `check_with_env`. El `check_source_with_types(source)`
   legacy queda como wrapper que pasa `None` (compat con REPL +
   unit tests internos sin file context).
2. **`src/bin/fitz-lsp.rs`** — `check_and_publish` deriva `base_dir`
   del open document URI via `uri.to_file_path().parent()` y lo pasa
   al nuevo wrapper. Fallback transparente a single-file mode cuando
   la URI no es `file://`.
3. **`docs/deudas-post-5b.md`** — deuda 🟡 original reescrita como
   🟢 CERRADO 2026-06-23 con detalle del fix + 5 tests + deudas
   residuales derivadas (transitive imports un nivel solo, sin
   `dep_registry`, sin cache de módulos resueltos, B20 cross-module
   `@cron(store=X)`).
4. **`docs/guide.md` cap 22** — bullet "Diagnostics en vivo" suma
   nota sobre la pre-scan cross-module desde v0.19.3, citando los 4
   patterns cubiertos (auth provider + @admin + @requires +
   @background+spawn) y la limitación MVP (un nivel de profundidad,
   paralelo W12/B10).

**5 unit tests nuevos** en `lsp::tests::cross_module_*` (feature `lsp`):
- `cross_module_auth_provider_resuelve_via_base_dir_sin_diagnostic_falso`
  — canónico: SIN `base_dir` aparece el falso positivo, CON `base_dir`
  desaparece.
- `cross_module_admin_decorator_resuelve_via_base_dir` — `@admin`
  con `role: Str` extraído del módulo origen vía `has_role_field`.
- `cross_module_requires_decorator_resuelve_via_base_dir` —
  `@requires("role")` (Fase 9.w.1.iter2.a).
- `cross_module_background_spawn_resuelve_via_base_dir` —
  `@background` + `spawn(<imp>(...))` (B10), mismo pipeline cierra
  ambos diagnostics.
- `cross_module_pre_scan_silent_fallback_on_missing_module` —
  robustez: módulo importado que no existe en disco no contamina
  errors (el codegen/runtime loader es quien reporta).

**Decisiones técnicas del MVP**:
- **Política de error**: silent fallback sobre módulos que fallan
  lectura/parse (paralelo a `pyi_loader::load_stubs` y W12 en
  main.rs). El LSP enriquece el env; el codegen/runtime loader es
  quien reporta errores reales — duplicar mensajes sería ruido.
- **Alcance MVP**: un nivel de profundidad (sin recursión transitiva),
  paralelo a W12/B10. Cubre el 90% del caso (handler-per-feature +
  provider en `auth.fitz`).
- **Sin dep_registry**: el LSP no tiene acceso al manifest
  (`fitz.toml`); resuelve solo paths relativos (misma limitación
  que `resolve_cross_module_definition` y `from_import_completions`
  ya existentes).
- **Sin cache**: cada keystroke re-parsea los módulos importados.
  Acceptable para programas chicos (<10 imports). Si presión real
  aparece sobre proyectos grandes, agregar cache con invalidación
  por modtime es deuda menor.

**Bonus paralelos en VSCode 0.19.3**: extensión bumpeada (sin cambios
al grammar TextMate ni al runtime del LSP más allá del fix) +
`.vsix` regenerado bundleando el `fitz-lsp.exe` fresh con el fix
incluido. Sin la regeneración, el bundle del `.vsix` de 0.19.2
quedaba pre-fix y los usuarios que instalan via Marketplace o
download del release no recibían la corrección.

**Tests al cierre v0.19.3**: 3331 unit lib con feature `lsp` (sin
cambios numéricos respecto a v0.19.2 — los 5 tests nuevos quedan
adentro del bloque LSP que ya existía + 1 viejo desplazado). `cargo
fmt --all --check` + `cargo clippy --lib --tests --bins --features
lsp -- -D warnings` limpios.

**Próximo norte tras v0.19.3**: ningún ítem crítico abierto.
Candidatos: transitive imports en el LSP (refinamiento del MVP),
cache de módulos resueltos para proyectos grandes, integración con
`dep_registry` para imports declarados en `fitz.toml`. Sin presión
real al cierre — el feedback de fitzwatch (que disparó tanto v0.19.2
como v0.19.3) está integrado.

## [v0.19.2] — 2026-06-23 — Bugfix urgente: `spawn(<cross_module_@background_async_fn>(...))` silent drop

Bugfix release que cierra una deuda 🔴 URGENTE descubierta el
2026-06-22 al integrar un proyecto real del autor con bulk SMTP
outbound vía `@background` cross-module. Sintoma: el handler aceptaba
`spawn(do_work(id))` donde `do_work` vivía en otro módulo
(`from worker import do_work`), compilaba limpio, pero la fn nunca
ejecutaba. Sin error, sin panic, sin log — **silent drop**. Workaround
temporal: `let _ = do_work(id).await` directo (perdía
fire-and-forget, bloqueaba el handler).

**Causa raíz**: `collect_module_sigs` (`src/codegen.rs:5561-5566`)
registraba el `ret` de fns importadas sin envolver en
`Type::Future(...)` cuando `is_async = true`. `gen_spawn_call` veía
`sig.ret = Type::Null` (no `Type::Future(Null)`) y emitía
`tokio::spawn(async move { do_work(id) })` SIN `.await` adentro del
closure — el `async move {}` construía un Future y lo dropeaba sin
pollarlo. Mismo módulo funcionaba OK porque `pre_register_fn_signatures`
ya hacía el wrap (Fase 6.6) — el path cross-module simplemente no
replicaba esa lógica.

**Fix** (`src/codegen.rs:5536-5570`, ~10 LoC netas): replicar el wrap
async/Future en `collect_module_sigs`. `Stmt::FnDef` desestructura
`is_async`, renombra `ret` → `inner_ret`, aplica wrap condicional
`Type::Future(Box::new(inner_ret))` cuando `is_async`. Paralelo
bit-a-bit al path local.

**Test E2E nuevo** (`tests/compile_e2e.rs::cross_module_spawn_async_background_emits_await_no_silent_drop`):
build a 2 archivos (`worker.fitz` + main) + inspección del Rust
emitido en `target/fitz-build/<stem>/src/main.rs`. Valida que el
closure contiene `do_work(id).await` y NO bare `do_work(id)`.

**LSP**: el false positive paralelo en LSP (mensaje
`"spawn: fn X is not declared with @background"` en módulos
importadores) ya estaba cerrado por **B10** (cosecha post-fitzwatch
2026-06-19, `extract_background_fn_names` +
`TypeEnv::add_imported_background_fns`). Ese fix cubría el path del
checker; v0.19.2 cierra el path del runtime/codegen. Ambos paths
quedan consistentes.

**Validación sin regresiones** (memoria `feedback_pre_release_verification`):
- `cargo test --lib --release`: **3200/3200 verde**.
- `cargo test --test compile_e2e --release smoke_ejemplos_guia_compilables_compilan`:
  **~370 ejemplos guía+curso+TaskHub verde** (~14 min).
- `fitz check` sobre los 10 boilerplates (`api-simple`,
  `api-middleware-cors`, `api-multi-tenant`, `api-postgres-fitz`,
  `api-postgres-python`, `api-websocket`, `api-orm-full`,
  `api-orm-full-fullstack`, `api-fullstack-postgres`, `cli-tool`,
  `taskhub`): **10/10 verde** (api-orm-full + taskhub usan
  `@background+spawn` en el MISMO módulo, no afectados por el bug;
  resto sin uso de spawn).
- `cargo fmt --all --check`: limpio.
- `cargo clippy --lib --tests --bins -- -D warnings`: limpio.
- Smoke real bit-a-bit `fitz run` ↔ binario nativo: el log
  estructurado `{"msg":"worker.start id=42"}` aparece en stderr
  inmediatamente después del response `200 dispatched` (pre-fix
  nunca aparecía).

**Bump**: `Cargo.toml` `0.19.1` → `0.19.2` + extensión VSCode `0.19.1`
→ `0.19.2` (sin cambios de grammar TextMate ni LSP; `.vsix` regenerado
para mantener paridad de versiones).

**Próximo norte tras v0.19.2**: la deuda 🔴 URGENTE de `@background`
cross-module queda cerrada entera. fitzwatch (proyecto privado del
autor) puede destrabar sus flows fire-and-forget de email/webhook
con la sintaxis canónica `spawn(<imported_fn>(args))` sin workaround
de `.await`. Ningún otro ítem crítico abierto del stack web — el
paquete completo (HTTP server + client + WS + auth + SMTP + Response
built-in + OpenAPI auto + observability + jobs cron/background) cubre
el caso 99% de proyectos reales.

---

## [v0.19.1] — 2026-06-21 — Bugfix: 3 bugs del Bloque 3.c (`Response { ... }`) detectados en fitzwatch

Bugfix release del feature `Response { ... }` built-in cerrado en
v0.19.0. Los 3 bugs solo aparecían al estresar el feature en
proyectos reales (cross-module + `?` propagation + auth + DB + WS +
observability combinados). El ejemplo oficial
`examples/guide/17l-response-custom.fitz` y los E2E de v0.19.0
seguían verdes — los bugs no estaban cubiertos por la suite.

**Descubrimiento (2026-06-21)**: fitzwatch (status page del autor,
repo privado) tenía Fase F.d (RSS feed por slug) bloqueada por la
deuda HTTP Content-Type. Esa deuda se cerró con v0.19.0; al retomar
F.d con la sintaxis nueva, salieron 3 bugs del codegen.

**Los 3 bugs**:

1. **Cross-module imports faltantes**: handlers `-> Response` o
   `-> Result<Response>` declarados en módulo importado emitían
   `src/<mod>.rs` referenciando `Response`/`ResponseData` sin
   `use crate::{Response, ResponseData};` correspondiente. Rustc
   abortaba con E0425/E0422.
2. **Signature mismatch en `Result<Response>`**: cuando el handler
   declara `-> Result<Response>` Y usa `?` propagation, el codegen
   forzaba la user-fn a emitir `-> __FitzResponse` (path legacy de
   W13 para `?` con returns no-Result). Pero el wrapper axum del
   Bloque 3.c espera la user-fn devolviendo `Result<Arc<Mutex<
   ResponseData>>, String>`. E0308 mismatched types.
3. **`metrics::counter!`/`histogram!` not found**: en programa con
   auth + DB + WS + observability + cross-module + Response. Era
   incidental al Bug 1 — el primer error rustc enmascaraba el resto.
   Con Bug 1 cerrado, Bug 3 desaparece automáticamente.

**Fixes**:

- **Bug 1**: walker nuevo `program_uses_response_builtin(program)`
  recursivo paralelo a `program_uses_db`/`program_uses_http_client`,
  detecta `Response` en TypeExpr (fn signatures, let bindings, type
  fields, params) y `Response { ... }` struct literals. En
  `generate_module_rs_with_bindings` se agrega bloque condicional
  que emite `use crate::{Response, ResponseData};` cuando el módulo
  los necesita.
- **Bug 2**: `gen_top_fn` consulta `detect_response_builtin_kind(
  &effective_ret, env)` antes de computar `has_return_status`. Si
  el handler retorna Response built-in (Direct o InResultOk), el
  `body_has_try` NO activa `response_mode` — la user-fn conserva
  su signature natural (`-> Result<Arc<Mutex<ResponseData>>, String>`
  o `-> Arc<Mutex<ResponseData>>`) y `gen_try` usa el `?` nativo
  Rust (válido porque el container ES Result).
- **Bug 3**: cerrado automático con Bug 1 (no requiere código
  adicional). El test E2E lo verifica: cross-module + Response +
  auth + DB + WS + observability compila limpio post-fix.

**3 E2E tests nuevos** en `tests/compile_e2e.rs` candan cada bug:

- `v019_response_cross_module_emits_imports`: handler `-> Response`
  en módulo importado + inspección del emitted `feed.rs` para
  confirmar el `use crate::{Response, ResponseData};`.
- `v019_response_in_result_ok_signature_matches_wrapper`: handler
  single-file `-> Result<Response>` + `?` propagation que ahora
  compila bit-a-bit.
- `v019_response_with_auth_db_ws_observability`: cross-module con
  auth + DB + WS + observability + Response (paralelo a fitzwatch),
  compila end-to-end.

**Helpers nuevos en `tests/compile_e2e.rs`**: `build_expect_ok` y
`build_expect_ok_multi` validan que `fitz build` succeed sin
invocar el binario (necesarios para HTTP servers que nunca
exitían bajo `output()`).

**Verificación pre-bump completa**: `cargo fmt --all --check` ✓,
`cargo clippy --all-targets -- -D warnings` (default + lsp) ✓,
3200 lib tests ✓, 98 cli_e2e ✓, 3 openapi_e2e ✓, smoke
`GUIDE_EXAMPLES_COMPILE` 370+ ejemplos verde ✓, los 3 E2E nuevos
de v0.19.1 + los 3 E2E `v019_block3d_*` de v0.19.0 ✓ (no
regresiones), `fitz check` + `fitz build` sobre 5 boilerplates
representativos (api-simple, api-orm-full, api-orm-full-fullstack
multi-file, api-websocket, api-middleware-cors crítico porque
usa `Response` opaca en post-mw) ✓.

**Bump**: Cargo.toml `0.19.0` → `0.19.1` + extensión VSCode
`0.19.0` → `0.19.1` + `.vsix` regenerado
(`editors/vscode/fitz-language-win32-x64-0.19.1.vsix`, 1.81 MB
con `server/fitz-lsp.exe` 4.36 MB bundleado fresh).

**Próximo norte tras v0.19.1**: notificar al autor para retomar
fitzwatch Fase F.d (30 min de trabajo: bump `FITZ_TAG` en
`d:\fitzwatch\.env`, descomentar handler `slug_incidents_rss` en
`src/public.fitz`, `docker compose up -d --build app`, smoke con
`curl -i http://localhost:8002/p/<slug>/incidents.rss`). La deuda
🔴 PRIORIDAD MÁXIMA cierra entera con este release.

## [v0.19.0] — 2026-06-21 — `Response { ... }` built-in (Content-Type custom + body crudo + OpenAPI auto)

Type built-in `Response` con 5 fields (`status` / `content_type` /
`headers` / `body` / `body_bytes`) que cubre el caso entero de
respuestas HTTP non-JSON: RSS, Atom, plain text, HTML estático, CSV,
SVG, PDF binario, ZIP. Cierra la deuda residual documentada como
"workaround manual + sintaxis hipotética no implementada todavía" en
cap M4.C5 del curso, y como "🟡 HTTP HandlerOutcome content_type
configurable" en `docs/deudas-post-5b.md`.

**Pitch**:

```fitz
@get("/feed.rss")
fn rss_feed() => Response {
    content_type: "application/rss+xml; charset=utf-8",
    body: "<?xml version=\"1.0\"?><rss/>",
}

// HTTP/1.1 200 OK
// content-type: application/rss+xml; charset=utf-8     ← custom ✓
// <?xml version="1.0"?><rss/>                          ← body crudo ✓
```

**5 diferenciales**:

1. **Built-in del lenguaje, no lib externa**. Validado por el checker
   estático. Cero `pip install` / `npm install` / `import`.
2. **Paridad bit-a-bit `fitz run` ↔ `fitz build`**. Validado a mano
   con `curl` + `xxd` sobre los 4 casos canónicos (RSS / text /
   SVG / PDF binario). Headers custom (Cache-Control,
   Content-Disposition) preservados en ambos paths.
3. **OpenAPI 3.1 auto-documentado**. Schema 200 emite
   `responses.200.content.<media_type>` automático con `format: binary`
   cuando hay `body_bytes`. Cero esfuerzo del user.
4. **Tipo built-in con 5 fields explícitos**. Sin builders ni mutable
   state; un struct literal cubre todo el caso.
5. **Validación XOR build-time**. Si seteás `body` literal no-vacío +
   `body_bytes`, el codegen aborta `fitz build` antes del runtime con
   mensaje claro citando workarounds. UX mejor que esperar el 500.

**5 bloques coordinados (un release coordinado)**:

- **Bloque 1**: `Value::Type` de `Response` registrado en
  `evaluator::register_builtins` + 5 fields en `register_http_builtin_types`
  del checker. `HandlerOutcome` runtime gana `body_bytes: Option<Vec<u8>>`
  + `content_type: String`. Helper `response_instance_to_outcome` con
  validación XOR + status range. axum builder `outcome_to_response` con
  branch text/binary.
- **Bloque 2**: opt-in binary path con `body_bytes: Bytes?`. Detección
  `Result::Ok(Response { ... })` además del Direct case. 11 unit tests
  `http::tests::v019_*`.
- **Bloque 3**: codegen paridad bit-a-bit. `ResponseData` empty marker
  refactorizado a struct con 5 fields. Enum `ResponseBuiltinKind`
  detectado en `resolve_handler_signature`, con fallback inferido por
  `TypeInfo` (consulta del último `Stmt::Return` cuando no hay
  anotación `-> Response`). Rama dedicada en
  `emit_handler_dispatch_and_response` (helper privado
  `emit_response_builtin_dispatch`) que emite axum response con
  Content-Type custom + headers + body|body_bytes. Validación XOR
  build-time. Rechazo de Response + post middleware (combinación de
  borde MVP) con mensaje claro. 12 unit tests `codegen::tests::v019_block3*`
  + 3 E2E `tests/compile_e2e.rs::v019_block3d_*`.
- **Bloque 4**: integración OpenAPI 3.1. Nuevo enum
  `ResponseContentTypeKind { Static, Dynamic }` + helper público
  `detect_response_content_type_kind` walker del AST usado por ambos
  caminos (runtime `route_info_from_spec` + codegen
  `pseudo_routes_from_ast`). `build_responses_with_auth` emite
  `200.content.<media_type>` con `{"type":"string","format":"binary"}`
  cuando aplique. Result<Response> preserva 500 + JSON error legacy.
  Validación bit-a-bit `fitz openapi` ↔ `/openapi.json` del binario.
  6 unit tests `openapi::tests::v019_block4_*`.
- **Bloque 5**: docs + LSP + extensión + cierre formal. Cap 17
  sub-sección nueva "Respuestas con Content-Type custom" en
  `docs/guide.md` con tabla comparativa vs FastAPI/Express/Flask/Spring
  + ejemplos por caso. Ejemplo runnable
  `examples/guide/17l-response-custom.fitz` con los 4 casos canónicos
  + endpoint /health JSON normal para regresión. Cap M4.C5 del curso
  refrescado (sección "Hoy: workaround manual + sintaxis hipotética
  no implementada todavía" reemplazada por "Cerrado en v0.19.0"). LSP
  completions: after-dot sobre Instance Response lista los 5 fields
  como FIELD kind (via path nominal genérico). 2 unit tests
  `lsp::tests::v019_block5_*`. Extensión VSCode v0.19.0 con `.vsix`
  regenerado.

**Tests al cierre v0.19.0**: 3202 lib (+8 neto vs 3194 pre-Bloque-3:
11 http intérprete previos a 3.a + 12 codegen (3+2+4+3) + 6 openapi +
2 LSP — 26 nuevos, varios viejos refactorizados) + 98 cli_e2e + 373
compile_e2e (+3 nuevos v019_block3d_*) + 3 openapi_e2e + 56 openapi
unit. `cargo fmt --all --check` + `cargo clippy --all-targets -- -D
warnings` limpios. Smoke `GUIDE_EXAMPLES_COMPILE` con
`17l-response-custom.fitz` sumado. Validación bit-a-bit a mano con
curl sobre los 4 casos canónicos + `/openapi.json` del binario.

**Deudas residuales derivadas** (NO bloquean uso real):

- Multi-arm bodies (if/match retornando distintos `Response { ... }`
  por arm) no se detectan en compile-time — el schema cae al path
  legacy `application/json` para esos handlers. El runtime funciona
  correcto (peek dinámico). Refinable post-MVP.
- Response built-in + post middleware (`@middleware(fn)` con 2 args)
  no soportado — el post-mw recibe `__FitzResponse` JSON-wrapped que
  pierde content_type / body_bytes. Workaround documentado: usar
  `return <status> { ... }` o remover el post-mw.
- Helper Bytes: `bytes(str)` convierte Str → Bytes para `body_bytes`;
  para leer de archivos / DB el valor llega como Bytes directo. Si
  aparece presión real, podemos sumar `bytes_from_b64`,
  `bytes_from_hex`, etc.

## [v0.18.2] — 2026-06-20 — Codegen: B19 (`@cron` en módulo importado no se spawnea)

Cosecha post-fitzwatch sesión 2, segundo bug del codegen descubierto al
arrancar el deploy real de fitzwatch (status page open-source en Fitz
puro). **Sin sintaxis nueva** — fix interno transparente para programas
con `@cron` declarado en el archivo main; agrega soporte real para el
caso cross-module que antes fallaba silenciosamente.

**El bug**: cuando `@cron("expr") async fn ...` vivía en un módulo
importado (caso canónico: `scheduler.fitz` con `import scheduler` desde
`main.fitz`), el codegen lo **dropeaba silenciosamente**. El usuario
veía el `print("[ready] cron...")` banner (un literal del usuario), pero
ningún `tokio::spawn(__fitz_run_cron_job(...))` se emitía en el `main.rs`
generado. Resultado: el server arrancaba, los endpoints respondían, pero
el scheduler nunca ejecutaba el job — sin error visible en stderr,
imposible de detectar sin inspeccionar el main.rs o esperar que el
trabajo del cron pase y no llegar nunca.

`partition_program_stmts(program)` solo procesa el AST del archivo main,
así que las fns `@cron` de módulos importados nunca aterrizaban en
`cron_jobs_info`.

**El fix**: paralelo a W16 (HTTP handlers cross-module) y 10.8.6 (WS
handlers cross-module):
1. `LoadedModule` suma `cron_fn_stmts: Vec<Stmt>`, populated al cargar
   cada módulo.
2. `CronJobInfo` suma `module_path: Option<String>` que indica el path
   Rust del módulo origen.
3. `generate_project` después de popular `cron_jobs_info` desde
   `p.cron_fns` (local), ITERA sobre `loader.modules` y suma los crons
   de cada uno con `module_path: Some(mod_name)`.
4. `emit_cron_job_spawns` cuando `module_path.is_some()` emite
   `crate::<mod_name>::<fn_name>` en lugar de bare `<fn_name>`.

**Bug derivado** (mismo release): al smoke-testear el fix con fitzwatch,
descubrí que `program_has_persistent_cron(program)` solo miraba el AST
del main, así que un `@cron(..., store=X)` declarado solo en un módulo
emitía el preludio del struct `__FitzCronOptions` en su shape simple
(sin field `store`), pero el spawn cross-module sí emitía con
`store: ...` → `error[E0560] struct '__FitzCronOptions' has no field
named 'store'`. **Fix**: nueva fn `program_or_modules_has_persistent_cron(program, &loader)`
que consulta también `loader.modules[i].cron_fn_stmts`; aplicada en
los 3 call sites (cargo_toml_for de main, decisión cross-module de
uses_db, emit_jobs_prelude). Helper privado `stmt_is_persistent_cron`
extraído para evitar duplicar la lógica.

**Caso edge no cubierto en v0.18.2** (documentado como deuda B20):
cross-module `store=X` resolution cuando el binding `let X = db.connect(...)`
también vive en el módulo importado. Requiere soporte para state vars
async-init en módulos (~200-300 LoC del codegen). Workaround canónico
pattern TaskHub: declarar `let X = ...` + `@cron(store=X)` en main.fitz;
módulo solo exporta la fn helper. Detalle completo en
`docs/deudas-post-5b.md` → "🟡 B20".

**Tests al cierre v0.18.2**: 3171 unit lib + 2 nuevos E2E
(`cron_in_imported_module_is_spawned_b19` que valida runtime via
stderr del binario nativo + `cron_with_persistent_store_in_imported_module_b19_derived`
que valida que el shape del preludio matchea el spawn cross-module
con `store=db`) — ambos verdes. fmt + clippy `--lib --tests --bins -- -D
warnings` limpios.

**Estado fitzwatch al cierre v0.18.2**: el scheduler ejecuta el cron
cada 10s + persistencia en `fitz_cron_jobs` + retry exponencial.
Smoke local end-to-end VERDE: HTTP login + crear monitor + cron tick
chequea el target (`check.down` con `http=503` esperado de httpbin) +
`incident.opened` automático + tabla `fitz_cron_jobs` con
`last_status: 'ok'`. fitzwatch refactoreado al pattern canónico TaskHub
(commit propio del repo fitzwatch): `let db = db.connect(db_url()).await`
+ `@cron(store=db)` en `main.fitz`, scheduler.fitz exporta solo la fn
helper async.

Próximo norte: tag + GHCR build + deploy fitzwatch al VPS.

---

## [v0.18.1] — 2026-06-20 — Codegen: B17 (`for x in <List<T>>` con `.await` en body rompe Send)

Cosecha post-fitzwatch sesión 2. Cierra el último bug del codegen que
quedaba bloqueando `fitz build` de fitzwatch (status page open-source
en Fitz puro). **Sin sintaxis nueva, sin keyword nueva, sin decorator
nuevo** — cambio interno del codegen transparente para programas
existentes (los ~290 ejemplos del smoke `GUIDE_EXAMPLES_COMPILE`
compilan bit-a-bit idéntico).

**El bug**: `for x in <List<T>>` con `.await` adentro del body en
handler `async` rompía Send. El codegen emitía
`for x in (xs).lock().unwrap().clone().into_iter() { ... }`, donde el
`MutexGuard` temporal (resultado de `.lock().unwrap()`) vive hasta el
final del statement del `for`. Cualquier `.await` adentro del body lo
cruzaba. `axum::Handler` exige `Future + Send`, y `MutexGuard` no es
`Send` → falla compilación con `error: future cannot be sent between
threads safely`.

**El fix**: emitir un bloque acotado con `let __for_snap` previo:
```rust
{
    let __for_snap = (xs).lock().unwrap().clone();
    for mut x in __for_snap.into_iter() {
        // body — el guard YA NO sobrevive cross-await
    }
}
```
El `let __for_snap = ...;` libera el `MutexGuard` al `;`, dejando solo
el `Vec<T>` owned. Aplicable a List<T>, List<Tuple> destructuring,
Map<K,V> destructuring y Map con wildcard `_`. Cambios en
`src/codegen.rs` (`gen_for_loop`, los 4 sitios del lock chain).

**Tests al cierre v0.18.1**: **3171 unit + 365/368 compile_e2e
(+1 nuevo `for_over_list_with_await_in_body_does_not_break_send_b17`;
3 fallas pre-existentes documentadas: `hidden_decorator` file lock
Windows + `http_coverage_metodos` routing 404 + `orm_w17_eager_loaded`
codegen drift #7 — heredadas de v0.16.0, cero regresiones por B17 fix)
+ 3 openapi**. Test viejo `for_over_list_generates_snapshot_iter`
actualizado al shape nuevo (snapshot binding + body itera el snap).
Smoke `GUIDE_EXAMPLES_COMPILE` verde ~290 ejemplos en 525s
(~8.7 min). fmt + clippy `--lib --tests --bins -- -D warnings` limpios.

**Estado fitzwatch al cierre v0.18.1**: TODOS los bugs B1-B17 cerrados.
`fitz build` de fitzwatch produce binario nativo end-to-end (validación
manual pendiente en deploy).

Próximo norte: **deploy de fitzwatch al VPS** (el blocker está cerrado).

---

## [v0.18.0] — 2026-06-19 — Mini-tanda SMTP builtin CERRADA + cosecha codegen post-fitzwatch CERRADA

Release coordinado de dos hitos en 1 día: (1) cosecha codegen post-fitzwatch (14 de 15 bugs cerrados, B15 el bloqueante incluido) y (2) mini-tanda SMTP builtin entera (8 sub-bloques). Cierra dos deudas explícitas heredadas del desarrollo de fitzwatch (status page open-source en Fitz puro, pausada el 2026-06-18 esperando ambas).

### Mini-tanda SMTP builtin CERRADA ENTERA (2026-06-19)

Mini-tanda en bloque coordinado (8 sub-bloques B1-B8) que cierra
la deuda "SMTP builtin" anotada el 2026-06-18 durante el desarrollo
de fitzwatch. Fitz suma el módulo built-in `smtp` con `smtp.send(opts)`
async, paralelo bit-a-bit al HTTP client de v0.17.0 y al resto del
stack web nativo (HTTP server, WebSockets, auth, jobs, ORM, observability).

| Bloque | Resumen | Tests nuevos |
|---|---|---|
| **B1 evaluator** | Módulo `Value::Module { name: "smtp" }` + `builtin_smtp_send` async + tipo `SmtpResult` pre-registrado + dispatch input → message lettre | 7 unit |
| **B2 checker** | Pre-registro `smtp` (Type::Any) + `SmtpResult` (Nominal) en `CheckCtx::new` + regla `?` heredada de Result | 5 unit |
| **B3 codegen** | Detector `program_uses_smtp` + Cargo.toml condicional `lettre = "0.11"` + `SMTP_PRELUDE` con `__FITZ_SMTP_TRANSPORT` LazyLock + dispatch `gen_smtp_call` + Map literal strict | 11 unit |
| **B4 LSP** | Completions `scope_level` (`smtp` MODULE) + `after_dot` resuelve `smtp.send` (METHOD) + after-dot sobre `SmtpResult` lista 3 fields | 3 unit LSP |
| **B5 guía + ejemplos** | Sub-sección "SMTP outbound" en cap 17 de `docs/guide.md` con panorama vecino + 5 diferenciales + 3 ejemplos runnable contra MailHog (`17i`/`17j`/`17k`) | smoke `GUIDE_EXAMPLES_COMPILE` (+3 ejemplos) |
| **B6 docs cross-cutting** | CLAUDE + README + `docs/index.md` + roadmap + `deudas-post-5b` marcan CERRADA | — |
| **B7 boilerplate** | `api-orm-full` suma `notify_author_by_email` paralelo a `notify_post_published` (opt-in via `SMTP_ENABLED`) | — |
| **B8 cierre formal** | Bump v0.18.0 + extensión VSCode 0.18.0 + `.vsix` regenerado + blog drafts ES/EN en `docs/blogs/` | — |

**API**: única función — `smtp.send(opts: Map) -> Future<Result<SmtpResult>>`.
Required keys `to`/`subject` + al menos uno de `body`/`body_text`/`body_html`.
`from` opcional si `SMTP_FROM` env var está seteada. Texto plano + HTML
juntos → multipart/alternative automático.

**Configuración**: env vars `SMTP_HOST` (required), `SMTP_PORT`
(default según TLS: 587/465/25), `SMTP_USER` + `SMTP_PASSWORD`
(juntos o ninguno), `SMTP_FROM` (default `From`), `SMTP_TLS`
(`starttls`/`implicit`/`none`). `smtp.configure(...)` programático
queda como deuda menor post-MVP.

**Modelo de errores**: `Result::Err(Str)` con prefijo `"smtp: "` para
errores de transporte (DNS, conexión, auth, TLS, server reject,
address parse, missing config). Status 5xx del SMTP server cuenta
como Err — lettre solo acepta el relay con response 250.

**Tipo built-in nuevo** `SmtpResult { delivered: Bool, message_id: Str,
duration_ms: Int }` pre-registrado en `TypeEnv` paralelo a
`HttpClientResponse`/`Request`/`Response`/`File`. Accesible sin
import, con autocomplete LSP de fields.

**Backend**: `lettre = "0.11"` con features `tokio1-rustls-tls` +
`smtp-transport` + `pool` + `builder` + `hostname`. Linkeado estático
sin openssl. Connection pool reusa la conexión TCP/TLS entre sends
(crítico para handlers HTTP / cron jobs que despachan varios mails
seguidos).

**Diferenciales** (paralelo a HTTP client):

1. Built-in del lenguaje (zero `pip install yagmail` / `npm install nodemailer` / `cargo add lettre`).
2. Paridad bit-a-bit `fitz run` ↔ `fitz build`.
3. Async ciudadano de primera (`@cron`/`@background`/`spawn`/handlers HTTP).
4. `Result<T>` automático con `?` propagation + checker exhaustividad.
5. Sin deps externas en el host (`rustls-tls` no exige openssl).

**Ningún lenguaje moderno del cuadro provee SMTP outbound como
builtin del lenguaje con paridad bit-a-bit intérprete↔binario y zero
deps externas para activarlo** — Python necesita `pip install` para
yagmail (stdlib smtplib es low-level), JS/Node `npm install
nodemailer`, Rust `cargo add lettre`, Java `JavaMail` con Maven. Solo
Go tiene `net/smtp` en stdlib pero es low-level (RFC 5321 a mano);
Fitz `smtp.send(opts)` es 1 línea.

### Cosecha codegen post-fitzwatch CERRADA ENTERA (2026-06-19)

Bloque de 6 sub-pasos en 2 días que cierra 14 de los 15 bugs del codegen descubiertos durante el desarrollo de **fitzwatch** (status page open-source en Fitz puro, pausado el 2026-06-18 esperando estos fixes). B16 abierto como deuda separada porque el fix vive en el checker (no es mecánico). B13 no reprodujo en v0.17.0 — W18 ya lo había cerrado. **`fitz check` y `fitz run` nunca estuvieron afectados** — todos los bugs eran específicos del path codegen → cargo build.

| Sub-paso | Bugs | Resumen | Tests nuevos |
|---|---|---|---|
| **1** | B3 + B11 | Detectores unificados (cross-module `ws_broadcast` + Response con `List<Nominal>`) | ~150 LoC + smoke verde |
| **2** | B4 + B5 + B6 + meta B14 | Flow-sensitive refinement en `gen_match` (filtra arms divergentes del LUB) | ~80 LoC + 4 unit tests |
| **3** | B7 | Wrap automático `Some()`/`None` en `gen_match` cuando el LUB queda Nullable | ~60 LoC + 3 unit tests |
| **4** | B8 | Blanket impl `__IntoPgValue for Option<T>` en el preludio DB del codegen | ~12 LoC + 3 unit tests |
| **5** | B1 + B2 + B9 + B10 + B12 | 5 fixes mecánicos (`.order_by(str)`, `http.post` body Instance, `Str? == Str`, cross-module `@background`/`@auth_provider`) | ~375 LoC + 16 tests |
| **6** | **B15** | **`.preload()` con FK Nullable** en BelongsToCompanion + path HasMany sibling | ~115 LoC + 3 unit + 1 E2E |

**Sub-paso 6 (B15 — el bloqueante)**:

`.preload("companion")` sobre una relation `BelongsToCompanion` cuyo FK en el parent es `Int?` (Nullable) rompía con dos errores rustc:
- `error[E0308]: expected i64, found Option<i64>` en `__FitzPgValue::Int(__g.<fk>)`.
- `error[E0277]: can't compare i64 with Option<i64>` en `__tg2.<pk> == __fk`.

Hallazgo importante al diagnosticar: el doc original de B15 decía "nullables en el parent type" como trigger; **el trigger REAL es FK Nullable**. El repro original (con FK `Int = 0` no-nullable + nullables varios en el parent type) NO disparaba el bug post-sub-pasos 1-5 (B7 + B8 lo habían cerrado parcial). Los models de fitzwatch declaran todos los FK como `Int = 0` sentinel, así que probablemente fitzwatch estaba pausado por W18 (cerrado en v0.17.0) y no por B15 estricto. Pero B15 sigue siendo bug crítico: cualquier usuario con FK opcional (`Int?`) lo dispara sin workaround viable user-side.

Dos cambios coordinados en `src/codegen.rs`:

1. **`emit_belongs_to_companion_preload_arm`** — nuevo param `parent_fields: &[TypeSigField]`. Cuando el FK es `Type::Nullable(_)`:
   - IDs collection emite `filter_map(|p| { let g = p.lock().unwrap(); g.<fk>.map(__FitzPgValue::Int) })` — las rows con `None` FK skipean el `IN (...)`.
   - Lookup del `__matched` emite `match __fk { None => None, Some(__fk_v) => __targets.iter().find(|t| { let tg2 = t.lock().unwrap(); tg2.<pk> == __fk_v }).cloned(), }`.

2. **`emit_preload_dispatch`** (path HasMany sibling) — detecta FK Nullable del child y emite `__cg2.<fk> == Some(__pid)` en lugar de `__cg2.<fk> == __pid`.

Política consistente: row con FK = None significa "no tiene parent en el target" → companion queda como `None`. Semántica idéntica al intérprete.

3 unit tests nuevos (path nullable + no-regression FK no-nullable + HasMany sibling con FK nullable) + 1 E2E (`cross_module_orm_preload_nullable_fk_b15`) con BelongsToCompanion + HasMany ambos con FK nullable en el mismo programa.

### Tests al cierre de la cosecha (sub-paso 6)

- `cargo test --release --lib` → **3141/3141** ✓ (+3 vs sub-paso 5)
- `cargo test --release --test compile_e2e cross_module_orm_preload_nullable_fk_b15` → **1/1** ✓
- `cargo test --release --test compile_e2e smoke_ejemplos_guia_compilables_compilan` → **366/366** ✓ (~5 min)
- `cargo fmt --all --check` ✓
- `cargo clippy --release --lib --tests --bins -- -D warnings` ✓

### Próximo norte

- Cuando el autor retome **fitzwatch**, `fitz build` debería pasar limpio. Si dispara algún error nuevo (improbable — los 14 bugs cerrados cubren todos los patterns reportados), se documenta como nuevo bug numerado.
- **B16** sigue abierto como deuda separada: cuando uno de los arms del `match` es una call expression que retorna `Null` (e.g., `log.error(...)`) y otro arm retorna un T concreto, el LUB no promueve a `Nullable(T)` (no hay `null` literal involucrado). Workaround user-side: terminar el arm con `; <sentinel>`. Fix sugerido: detectar el caso en el checker y emitir error claro citando el workaround. Tracking en `docs/deudas-post-5b.md` → B16.

## [v0.17.0] — 2026-06-18 — Mini-tanda HTTP client builtin CERRADA + W18 fix codegen cross-module observability

Cierre formal de la **mini-tanda HTTP client builtin** (9 bloques + W18 fix coordinado). Habilita HTTP client outbound como **ciudadano de primera clase del lenguaje** — paralelo al HTTP server-side cerrado en Fase 4. Cierra la deuda crítica del codegen cross-module observability detectada al validar B8 sobre `boilerplates/api-orm-full` multi-archivo.

### Nuevo: módulo `http` built-in (6 builtins async)

API completa día 1, devuelve `Future<Result<HttpClientResponse>>` en los 6:

- `http.get(url)` / `http.head(url)` / `http.delete(url)` — sin body.
- `http.post(url, body)` / `http.put(url, body)` — body acepta `Str` / `Map<Str, Any>` (auto-JSON + `Content-Type: application/json`) / `Bytes`.
- `http.request(opts: Map)` — low-level con `method`/`url`/`timeout_ms`/`headers`/`body`/`follow_redirects`.

Tipo built-in nuevo `HttpClientResponse { status: Int, body: Str, headers: Map<Str, Str>, duration_ms: Int }` paralelo a `Request`/`Response` del HTTP server-side.

**Modelo de errores**: `Result::Err(Str)` con mensajes claros (`"timeout después de Nms"`, `"DNS no resuelve: <host>"`). Status 4xx/5xx **NO son Err** (el user mira `r.status`); solo errores de transporte van a Err.

**Backend**: `reqwest = "0.12"` con features `["json", "rustls-tls"]` linkeado estático — sin openssl en el host.

### 5 diferenciales

1. **Built-in del lenguaje** — no `pip install requests` / `npm install axios` / `cargo add reqwest` / import del std.
2. **Paridad bit-a-bit `fitz run` ↔ `fitz build`** — el binario standalone tiene el cliente HTTP linkeado.
3. **Async ciudadano de primera** — se integra natural con `@cron`/`@background`/handlers HTTP/`spawn(...)`.
4. **`Result<T>` automático** — errores como valores, `?` propaga, checker exige manejo (regla 5.3.3).
5. **Sin deps externas en el host** — `rustls-tls` no exige openssl.

Ningún lenguaje moderno del cuadro (Python `requests`, JS `axios`/`fetch`, Java `OkHttp`/`HttpClient`, Rust `reqwest`, Go `net/http`) provee HTTP client outbound como builtin del lenguaje con esta combinación.

### Sub-pasos por bloque

| Bloque | Commit | Resumen |
|---|---|---|
| **B1** evaluator | `3cefd2e` | `Value::Module { name: "http" }` + 6 builtins async + `body_to_reqwest_body` dispatch + tipo `HttpClientResponse` pre-registrado |
| **B2** checker | `d1aaf70` | Pre-registro `http`/`HttpClientResponse` en `CheckCtx::new` + calls tipan `Result<Any>` + regla `?` |
| **B3** codegen | `9e214e3` | `program_uses_http_client` walker + Cargo.toml condicional + `HTTP_CLIENT_PRELUDE` + dispatch en `gen_call` |
| **B4** LSP | `29bc041` | `scope_level_completions` + `after_dot_completions` con 6 métodos + signatures + hints |
| **B5** guía + ejemplos | `03cd71e` | Sub-sección "HTTP client outbound" en cap 17 + 4 ejemplos runnable `17e`/`17f`/`17g`/`17h` |
| **B6** docs cross-cutting | `931fd18` | CLAUDE + README + index.md + deudas + roadmap actualizados |
| **B7** curso M5.C5 | `b499c36` + chore `3c6f264` | Cap nuevo M5.C5 dedicado HTTP client outbound + capstone integrador M5 entero |
| **B8** boilerplate api-orm-full | `1468e0d` + chore `a01da2d` | Update chico sumando webhook outbound al publicar post (`@background async fn notify_post_published(...)` + `spawn(...)`) |
| **W18** fix codegen | `63b3d3f` | Cross-module observability imports + log helpers + `observability=false` propagation (cierra deuda crítica detectada al validar B8) |

### W18 — Fix codegen cross-module observability

Cierra la deuda más grande heredada de Fase 12.3.b + W11/W16. **Sin W18**, programas multi-archivo donde un módulo importado declaraba `@get`/`@post`/etc y/o llamaba `log.{info,warn,error,debug}(...)` rompían `fitz build` con 12-134+ errores rustc (símbolos `__fitz_otel_*`/`__FitzSpanContext`/`__fitz_log_*` faltantes en los módulos). Bloqueaba el primer intento de validar `boilerplates/api-orm-full` end-to-end.

3 sub-cambios coordinados en `src/codegen.rs`:

1. **Imports observability del wrapper HTTP**: extendido `module_has_http` con 3 grupos paralelos a W11/W16 (gateados por `module_has_http && main_observability_enabled`).
2. **Imports de los 4 log helpers** (independientes del wrapper): nuevo flag `module_uses_logging = program_uses_logging(program)`; cubre `log.X(...)` user code en módulos con o sin HTTP.
3. **Propagación de `@server(observability=false)` main → módulos**: nuevo helper `extract_main_observability_enabled(program)` walka top-level del main + threading via `ModuleLoader.main_observability_enabled` + arg nuevo en `generate_module_rs_with_bindings`. Cierra inconsistencia que existía desde Fase 12.3.b.5 (main bare-metal, módulos instrumentados).

9 unit tests nuevos en `codegen::tests::w18_*` (helper extract + happy paths + propagación + regresión negativa). Detalle completo en `docs/deudas-post-5b.md` → "🟢 W18 (post-B8) — Codegen cross-module observability CERRADA".

### Decisión de versión

- **v0.16.0 → v0.17.0** (minor). Justificación: builtin nuevo `http` del lenguaje + tipo built-in nuevo `HttpClientResponse` (user-visible) + cierra deuda crítica del codegen (refactor coordinado de 3 sub-cambios). No breaking de sintaxis del lenguaje.
- Extensión VSCode: **0.16.0 → 0.17.0** + `.vsix` regenerado (LSP completions de B4 ya estaban; bump por alineación con el binario).

### Tests al cierre (verificación pre-bump completa)

- `cargo test --lib` → **3116/3116** ✓ (default mode, +9 tests W18)
- `cargo test --test compile_e2e smoke_ejemplos_guia_compilables_compilan` → **1/1** ✓ (363 ejemplos guía+curso+TaskHub compilan limpios)
- `cargo fmt --all --check` → limpio
- `cargo clippy --all-targets -- -D warnings` (default) → limpio
- `cargo clippy --all-targets --features lsp -- -D warnings` → limpio
- `boilerplates/api-orm-full` con webhook outbound compila a binario nativo end-to-end (134 errores rustc pre-W18 → 0 post-W18) + arranca + `/healthz` y `/readyz` responden `200`
- Smoke alternativo: `17g-http-client-webhook.exe` responde 202 en 207ms + webhook delivered status=200 duration_ms=724ms contra `httpbin.org`
- Validación bit-a-bit `fitz run` ↔ binario nativo sobre `17e` contra `httpbin.org` (paridad estructural perfecta)

### Hallazgos del codegen del Bloque 5 — 3 deudas residuales NO bloqueantes

Documentadas en `docs/deudas-post-5b.md` con workarounds idiomáticos:

1. **`?` top-level rechazado**: regla esperada del checker 5.3.3. Workaround `async fn run() -> Result<Null>` + `match run().await`.
2. **`for x in <List<Str>>` con `.await` adentro de `@cron`**: el codegen mantiene MutexGuard cross-await al iterar. Workaround calls explícitas sin loop.
3. **Map literal heterogéneo en `return <status> { ... }` de handler**: sub-case del fix v0.10.4. Workaround homogeneizar a `Map<Str, Str>`.

### Próximo norte

**Retomar fitzwatch** (status page open-source en Fitz puro, pausado el 2026-06-18 por falta de `http.head` builtin). El blocker `http.head(monitor.target).await?` ya está disponible. Detalle del plan en `d:\fitzwatch\NEXT-SESSION.md`.

Detalle técnico completo de la mini-tanda en [`docs/http-client-roadmap.md`](docs/http-client-roadmap.md) (9 bloques + R1-R8 regresiones + tabla de estado con SHAs).

## [v0.16.0] — 2026-06-15 — Hito: compilador 100% inglés (cierre F1-F6 + F5.d)

Cierre formal de la mini-tanda de traducción del código del compilador
del español al inglés. **47 commits coordinados** en 7 sub-fases
(F1-F6 + F5.d) que llevan el surface user-facing del binario + tests
internos + grammar TextMate de español a inglés.

**Sin cambios de comportamiento del lenguaje** — AST, checker, runtime,
codegen, ORM, package manager y LSP funcionan bit-a-bit idénticos.
Release cosmética + i18n, pero cross-cutting (47 commits) y user-visible
(mensajes de error CLI, hints LSP, descripciones de completion, etc.).

### Surface cubierto

| Sub-fase | Scope |
|---|---|
| **F1** | Comments en src/**/*.rs (17 batches) |
| **F2** | Test function names (~2647 renames en 44 archivos) |
| **F3** | Error messages + runtime emit strings (10 batches) |
| **F4** | Output user-facing — CLI, LSP, examples, docs internacionales (syntax-spec, architecture) |
| **F5.a/b/c** | Test assertions internas + barrida cross-archivo + F4 leftover + driver Postgres TLS |
| **F5.d** | Residual "esperaba"/"esperaban" — 253 strings en `mod tests` de 11 archivos + 1 user-facing en `src/db.rs:535` que F5.c.2 había perdido |
| **F6** | Comentarios del grammar TextMate (`editors/vscode/syntaxes/fitz.tmLanguage.json`) |

### Resultado

- Surface user-facing del compilador (CLI, LSP, errors): **100% inglés**
- Test assertions internas (`mod tests`): **100% inglés**
- Comments Rust + grammar TextMate: **100% inglés**
- `grep "esperaba|esperaban" src/` → **0 ocurrencias** post-cierre

### NO cubierto (deuda explícita, sigue en español)

- `docs/guide.md`, `docs/curso/`, `docs/taskhub/`, `README.md`, `docs/index.md` — material pedagógico se mantiene en castellano por decisión de proyecto. Traducción a inglés queda como sub-paso futuro si el material gana tracción internacional.
- Fixtures `.fitz` dentro de tests (ej: "El Chaltén", "división por cero", "id inválido", passwords como "contraseña-secreta-del-usuario") — son fixtures, no surface del compilador.

### Tests al cierre (verificación pre-bump completa)

- `cargo test --lib` → **3052/3052** ✓ (sin feature)
- `cargo test --lib --features lsp` → **3170/3170** ✓
- `cargo test --lib --features python` → **3143/3143** ✓
- `cargo test --test cli_e2e --features lsp` → **98/98** ✓
- `cargo test --test openapi_e2e --features lsp` → **3/3** ✓
- `cargo test --test compile_e2e --features lsp` (modulo smoke gigante) → **352 passed, 8 failed** — los 8 son **pre-existentes documentados** en `docs/deudas-post-5b.md` (codegen cross-module + observability, Windows file lock paralelo, runtime HTTP routing 404, codegen drift orm_w17 #7, Postgres apagado para sslmode=require). Cero regresiones de F1-F6+F5.d.
- `cargo test --test compile_e2e smoke_ejemplos_guia_compilables_compilan` → **1/1** ✓ (~290 ejemplos guía + curso + TaskHub compilan limpios, 251s)
- `cargo fmt --all --check` → limpio
- `cargo clippy --all-targets -- -D warnings` → limpio
- `cargo clippy --all-targets --features lsp -- -D warnings` → limpio
- `cargo clippy --all-targets --features python -- -D warnings` → limpio
- `cargo build --release` × 3 features (default + lsp + python) → todos verdes
- TypeScript de la extensión (`npx tsc --noEmit`) → limpio

### Versión

- `Cargo.toml`: **v0.15.14 → v0.16.0** (minor — cross-cutting cosmético + i18n con visibilidad en surface user-facing CLI/LSP, no breaking de sintaxis del lenguaje ni de tipos)
- Extensión VSCode: **0.15.0 → 0.16.0** + `.vsix` regenerado (F6 tocó grammar TextMate)

## [v0.15.14] — 2026-06-09 — Codegen: OnceCell paralelo en `__fitz_cron_init_storage` del binario nativo

**Fix de la falla descubierta IN VIVO en el smoke E2E real del TaskHub
con docker compose tras el release v0.15.13** (commit `4761768`).

El v0.15.13 cerró la deuda URGENTE-1 (cron `init_storage` race
condition contra Postgres `pg_type_typname_nsp_index`) pero el fix
cubrió SOLO el camino del intérprete (`src/cron_jobs.rs::INIT_STORAGE_ONCE`).
**El binario producido por `fitz build` corre código emitido por el
codegen (`src/codegen.rs::SQL_HELPERS_PRELUDE`) que tiene su propio
`__fitz_cron_init_storage` paralelo SIN OnceCell** — por lo tanto, los
N jobs persistentes spawneados al boot del binario seguían golpeando
el race de Postgres, y uno de los crons abortaba silenciosamente
(silent partial failure del scheduler).

**Detección**: smoke E2E real con `docker compose up` sobre
`boilerplates/taskhub` el 2026-06-09. Tras el release v0.15.13 y el
re-build del binario taskhub con la imagen `:v0.15.13-python` del CI,
los logs del app al boot mostraron exactamente el mismo error:

```
🕐 Fitz scheduler arrancado con 2 job(s) cron
   @cron  cleanup_old_tasks (0 0 3 * * *)
   @cron  daily_due_reminders (0 0 9 * * *)
🕐 cron job 'daily_due_reminders' init storage falló, abortando:
   ERROR [23505]: duplicate key value violates unique constraint
   "pg_type_typname_nsp_index"
   [sql: CREATE TABLE IF NOT EXISTS fitz_cron_jobs (...)]
```

**Fix v0.15.14**: paralelo bit-a-bit del fix v0.15.13 agregado al
`SQL_HELPERS_PRELUDE` del codegen (`src/codegen.rs`). El binario
producido ahora emite:

- `static __FITZ_CRON_INIT_STORAGE_ONCE: tokio::sync::OnceCell<Result<(), String>>` global.
- `__fitz_cron_init_storage_inner(conn)` — helper real con los 3
  `CREATE TABLE IF NOT EXISTS` (renombrado del viejo `__fitz_cron_init_storage`).
- `__fitz_cron_init_storage(conn)` — wrapper que invoca
  `__FITZ_CRON_INIT_STORAGE_ONCE.get_or_init(|| async { __fitz_cron_init_storage_inner(conn).await }).await.clone()`.

Los call sites en `run_cron_job` emitido siguen invocando
`__fitz_cron_init_storage` (transparente — el wrapper tiene la misma
firma). Sin cambios al cargo_toml emitido: `tokio::sync::OnceCell`
requiere feature `sync`, que ya se incluye cuando `uses_db = true`
(activado por `@cron(store=db)`).

**Lección aprendida del proceso**: todo fix a la lógica del scheduler
de cron debe sumar test E2E del codegen path — compilar un programa
con N crons + ejecutar el binario producido contra Postgres real +
verificar que no rompe. Los tests E2E v0.15.13 cubrían el path del
intérprete (`tests/cron_jobs_real_postgres.rs`), pero el binario
nativo era ciego al test runner. **Esa brecha fue lo que dejó pasar
el bug residual** — documentada en `docs/deudas-post-5b.md` URGENTE-1
como "Mejora futura del proceso".

**Test nuevo del codegen** (sin requerir Postgres real):
`v0_15_14_codegen_cron_persistent_emite_oncecell_para_init_storage`
en `src/codegen.rs::tests`. Verifica que el preludio emitido contiene
`__FITZ_CRON_INIT_STORAGE_ONCE`, `tokio::sync::OnceCell`,
`get_or_init`, y `__fitz_cron_init_storage_inner`.

**Total al cierre v0.15.14**: cargo test --lib 3052 verde (+1 nuevo).
cargo fmt + clippy `--lib --tests --bins -- -D warnings` limpios.

**Smoke E2E real con docker compose pendiente del release CI** —
una vez `:latest-python` actualice a v0.15.14, se re-verifica con
el TaskHub que AMBOS crons arrancan sin el error del catálogo
`pg_type`.

## [v0.15.13] — 2026-06-08 — Lenguaje: cron `init_storage` OnceCell + `@server(host=, port=)` kwargs

**Cierre de las 2 deudas URGENTES** documentadas tras el smoke E2E
del TaskHub Dockerizado (commit `4761768`).

**URGENTE-1 — cron `init_storage` race condition** (parcialmente
cerrada en v0.15.13, completada en v0.15.14):

`tokio::sync::OnceCell<Result<(), String>>` global en
`src/cron_jobs.rs::INIT_STORAGE_ONCE` + wrapper
`ensure_storage_initialized(conn)` que serializa el primer init
del proceso. `run_cron_job` ahora llama al wrapper en lugar de
`init_storage` directo. Helper test-only
`reset_init_storage_once_for_tests` con `#[doc(hidden)]` (no
`#[cfg(test)]` porque los integration tests lo necesitan
desde otro crate).

**Limitación descubierta post-release** (cerrada en v0.15.14): el
fix v0.15.13 cubrió SOLO el camino del intérprete. El codegen
(`src/codegen.rs::SQL_HELPERS_PRELUDE`) tiene su propio
`__fitz_cron_init_storage` paralelo que NO usaba OnceCell —
detectado in vivo en el smoke E2E con docker compose. Cierre
completo en v0.15.14 con el OnceCell paralelo en el preludio del
codegen.

**Tests E2E reales contra Postgres** (`tests/cron_jobs_real_postgres.rs`,
opt-in con `FITZ_TEST_PG_URL`):

- `v0_15_13_ensure_storage_initialized_evita_race_con_10_paralelos`:
  10 tareas concurrentes invocan `ensure_storage_initialized`. Sin
  el fix al menos una rompía con `pg_type_typname_nsp_index`; con
  el fix las 10 completan OK.
- `v0_15_13_ensure_storage_initialized_cachea_resultado_segundo_call_no_corre_create_table`:
  tras el primer init, drop manual de las tablas + segundo call
  retorna OK (reusa cache, no re-ejecuta SQL).

Ambos verdes contra Postgres 15 local.

**URGENTE-2 — `@server(host=..., port=...)` no aceptaba kwarg**:

Cambios paralelos bit-a-bit en evaluator + codegen:

- `src/evaluator.rs::register_server_config`: ramas `"port"` y
  `"host"` nuevas en el match de kwargs con detección de
  doble-especificación estilo Python ("port pasado dos veces").
  Flags `port_set_via_positional` y `host_set_via_positional`
  para detectar conflicto. Mensaje de kwarg desconocido
  actualizado para citar `port, host, ...`.
- `src/codegen.rs::parse_server_decorator`: mismo cambio bit-a-bit.

**Patrón canónico nuevo** (recomendado para Dockerizar):

```fitz
@server(port=8080, host="0.0.0.0", prometheus=true)
fn main() => 0
```

Equivalente a `@server(8080, "0.0.0.0", prometheus=true)` (positionals
siguen funcionando para backward-compat).

**Tests nuevos**:

- 9 unit tests evaluator (`v0_15_13_server_*`): host kwarg solo,
  port kwarg solo, mixed, port positional + host kwarg, doble port
  error, doble host error, host kwarg no-str, port fuera de rango,
  host IP inválida.
- 5 unit tests codegen (`v0_15_13_server_*`): emisión correcta
  para los casos canónicos + conflictos rechazados.
- Test viejo `server_kwarg_desconocido_lista_docs_y_api_version`
  actualizado al mensaje nuevo (incluye `port, host`).

**Docs sincronizadas con el lenguaje + el boilerplate**:

- `docs/deudas-post-5b.md`: ambas a CERRADO con detalle.
- `docs/guide.md` cap 17: sintaxis kwarg + conflicto + nota Docker
  explícita.
- `docs/curso/m4-http/c1-verbos-server.md`: kwarg + ejemplo error.
- `docs/blog/3-deploy-fitz-{one-command-en,un-comando-es}.md`:
  snippet del "real service" actualizado al patrón kwarg canónico.
- `docs/taskhub/c1-c7`: sincronizados con el boilerplate publicado
  (@server kwarg + OTel `:4318` + Jaeger `:1.76.0` + Dockerfile
  multi-stage real + entrypoint.sh con `fitz db migrate` al boot +
  healthcheck `curl /healthz` real).
- `src/lsp.rs`: completion `@server` documenta los kwargs nuevos
  + patrón canónico Docker.
- `boilerplates/taskhub/src/main.fitz`: refactor al patrón kwarg.

**Tests al cierre v0.15.13**:

- cargo test --lib: 3051 verde (+14 nuevos: 9 evaluator + 5 codegen).
- cargo test --test cron_jobs_real_postgres v0_15_13 -- --ignored
  contra Postgres 15 local: 2 verdes.
- cargo test --test compile_e2e --release: 99 ejemplos guide verdes
  (617s).
- cargo fmt + cargo clippy: limpios.
- fitz check sobre TaskHub con sintaxis kwarg: verde.

## [v0.15.12] — 2026-06-08 — Codegen: state vars con `.await` + spawn shadow-clones (TaskHub a CERO errores)

**Cierre completo del boilerplate TaskHub**: `fitz build` baja de los
2 errores rustc remanentes en v0.15.11 a **0** con este release. Los
2 blockers eran bugs pre-existentes del codegen, descubiertos cuando
v0.15.11 destrabó los 19 errores anteriores (efecto cascade
unblocking). Ambos cerrados en una mini-fase coordinada, con paridad
bit-a-bit `fitz run` ↔ `fitz build` preservada y zero regresión en
los 3037 tests del lib + 99 ejemplos guide + 360 compile_e2e.

### Fix 1 — `LazyLock<T>` con `.await` en init → `tokio::sync::OnceCell<T>` (`src/codegen.rs`, ~80 LoC)

**Bug**: cuando un `let X = <expr>.await` top-level está referenciado
por algún handler HTTP/WS/cron, el codegen lo hoistea a
`static __FITZ_STATE_X: LazyLock<T> = LazyLock::new(|| <expr>.await)`.
Pero el closure de `LazyLock::new` es sync — rustc emite
`error[E0728]: await is only allowed inside async functions and blocks`.
Caso canónico TaskHub: `let db_result = db.connect(url).await` que
varios handlers HTTP + el `@cron(store=db_result)` consumen.

**Fix**: detección + dispatch entre dos paths según si la RHS contiene
`.await`. Cambios:

- **Helper nuevo `expr_contains_await(e: &Expr) -> bool`** (recursivo,
  paralelo a `expr_uses_db`/`expr_uses_log`/etc; descende a Try/Await/
  Call/BinOp/UnaryOp/Field/Index/Slice/Range/List/Tuple/Map/StructLit/
  TupleField/StrInterp/Loop/Ok/Err/NamedArg; trata FnExpr como opaque
  scope async/sync propio que no propaga). Test smoke nuevo
  `v0_15_12_expr_contains_await_detecta_await_anidado` cubre 6 casos
  true + 4 false.
- **`state_var_async: HashMap<String, bool>`** nuevo en `CodegenCtx`,
  poblado en `resolve_state_var_types` cuando `expr_contains_await(value)`
  detecta async init.
- **Doble emisión en `gen_http_main`**: para sync se mantiene
  `LazyLock<T> = LazyLock::new(|| init)` (zero cambio); para async se
  emite `static __FITZ_STATE_X: tokio::sync::OnceCell<T> =
  tokio::sync::OnceCell::const_new();` + init eager en el body del
  `async fn main()` antes de cualquier handler/spawn/serve via
  `{ let __init: T = <init>; __FITZ_STATE_X.set(__init).expect("..."); }`.
  El `OnceCell::set` panic-ea si se llama dos veces — sólo se llama
  una vez por var (boot del proceso).
- **Materialización en `gen_top_fn` (líneas 11787-11791) y
  `emit_cron_job_spawns` (líneas 7733-7745)**: dispatch entre
  `(*static).clone()` (sync) y `static.get().expect("...").clone()`
  (async). Mismo costo runtime — clona Arc, no contenido.

Tests nuevos: `v0_15_12_state_var_con_await_usa_oncecell_e_init_eager`
+ `v0_15_12_state_var_sin_await_sigue_usando_lazylock`. El test viejo
`v0_15_11_cron_store_kwarg_con_state_var_materializa_local_antes_de_spawn`
actualizado al shape nuevo (la RHS del state var contiene `.await`,
así que ahora el path OnceCell aplica).

### Fix 2 — `spawn(fn(arg))` movía vars del outer (E0382) — shadow-clone preventivo (`src/codegen.rs`, ~50 LoC)

**Bug**: cuando el arg del inner call de `spawn(...)` referencia un
var del scope outer que el caller también usa después, el
`tokio::spawn(async move { ... })` capturaba el var por valor,
moviéndolo al closure. Rust rompe con `error[E0382]: use of moved
value`. Caso canónico TaskHub:

```fitz
let _ = spawn(send_due_reminder(new_task.id))  // <- mueve new_task
return new_task  // E0382
```

**Fix**: shadow-clone preventivo de cada ident del scope local
referenciado por los inner_args. Cambios:

- **Helper nuevo `collect_idents_in_expr(e, &mut HashSet<String>)`**
  recursivo (paralelo a `expr_contains_await`; descende a Call/BinOp/
  UnaryOp/Field/Index/Slice/Range/List/Tuple/Map/StructLit/StrInterp/
  Ok/Err/Try/Await/NamedArg).
- **`gen_spawn_call`** ahora recolecta los idents de los inner_args,
  filtra por `var_in_any_scope` (descarta fns/builtins/types/módulos
  no clonables), y emite `let <name> = <name>.clone();` para cada uno
  (orden determinista, deduplicado) ANTES del `tokio::spawn(async
  move ...)`. El `async move` captura los shadow clones por valor;
  el outer scope sigue accesible.

Tests nuevos: `v0_15_12_spawn_clona_vars_outer_antes_del_async_move`
+ `v0_15_12_spawn_sin_args_no_emite_shadow_clones` + `v0_15_12_spawn_no_clona_idents_que_son_fn_nombradas`.

### Sin breaking changes user-visible

- Programas sin state vars async siguen path LazyLock — output Rust
  idéntico bit-a-bit.
- Programas con `spawn(fn())` sin args del outer no emiten clones
  extras.
- `cargo fmt --all --check` + `cargo clippy --lib --tests --bins --
  -D warnings` limpios.

### Smoke real TaskHub (cierre)

`fitz build` sobre `boilerplates/taskhub/` produce `taskhub.exe` de
9.7 MB **exit 0**. El binario arranca contra Postgres local sin
panic, `GET /healthz` → 200 `{"status":"ok","version":"0.1.0-c7"}`,
`GET /readyz` → 200 `{"status":"ok"}`, `GET /metrics` expone counters
Prometheus con el state var async (`db_result`) materializado
correctamente en cada handler async. Los 2 blockers heredados de
v0.15.11 están cerrados end-to-end.

### Deudas residuales menores del TaskHub (NO bloquean)

Heredadas de v0.15.11, siguen abiertas como refinamientos del
codegen/checker — workarounds triviales documentados en main.fitz:
(1) match con un arm `return Err(...)` no propaga inferencia del
inner Result al binding; (2) `!Future<Bool>` requiere `.await`
explícito (checker no detecta); (3) coerción PyAny → Int adentro de
match arm no se propaga desde anotación destino. Cada una con
workaround claro; ninguna bloquea uso real.

## [v0.15.11] — 2026-06-08 — Codegen: state vars materializados en `emit_cron_job_spawns` + workarounds TaskHub

**Avance grande pero NO cierre completo** del boilerplate TaskHub:
`fitz build` baja de **21 errores rustc a 2** con este release. Los 2
restantes son bugs pre-existentes del codegen que aparecieron al
destrabar los 19 anteriores (efecto cascade unblocking) y quedan
documentados como deuda crítica próxima en `docs/deudas-post-5b.md`
(LazyLock init con `.await` + spawn arg captura por move).

### Fix del codegen (`src/codegen.rs`, +~30 LoC)

**`emit_cron_job_spawns`** materializa state vars referenciados como
kwarg `store=<ident>` en `@cron(...)` ANTES del loop de spawns. Cuando
un `let X = ...` top-level está referenciado por algún handler HTTP/
WS/cron, el codegen lo hoistea a `static __FITZ_STATE_X` y materializa
el local dentro del body de cada fn vía `gen_top_fn` líneas 11738-11764.
Pero el bloque de `tokio::spawn(...)` del cron emitido por
`emit_cron_job_spawns` corre adentro de `__main_inner` (no de una
top fn), donde NO se materializaba el local. Resultado: `(&X).into_store()`
rompía con `error[E0425]: cannot find value 'X' in this scope`.

El fix recolecta los `store_var` de cada `CronJobInfo`, filtra los
que también están en `state_var_types` (hoisteados), deduplica y
emite `let X: T = (*__FITZ_STATE_X).clone();` antes del loop. Sin
overhead extra — `Arc::clone` y nada más. Test unitario nuevo
`v0_15_11_cron_store_kwarg_con_state_var_materializa_local_antes_de_spawn`
candea la regresión (verifica orden: materialización antes del spawn).

### Workarounds en `boilerplates/taskhub/src/main.fitz`

3 patches sobre el `.fitz` del TaskHub para destrabar el smoke real,
documentados como deudas residuales menores del codegen/checker:

1. **17 sitios `let conn = match db_result { ... }`** → anotación
   explícita `let conn: DbConn = match db_result { ... }`. Sin la
   anotación el codegen tipa `conn: Option<__FitzDbConn>` (Nullable)
   aunque ambos arms devuelven `__FitzDbConn` plano (el otro arm
   aborta con `return Err(...)`). Deuda residual del codegen.
2. **`@healthz fn check_db_alive() -> Bool` → `@healthz async fn ...`**
   con `.await` sobre `c.is_closed()`. Idem `@readyz`. El método
   `is_closed()` retorna `Future<Bool>` (paridad intérprete + codegen);
   sin `.await` rustc rompe con `cannot apply unary operator '!' to
   type 'impl Future<Output = bool>'`. Deuda menor del checker (no
   detecta `!Future<Bool>` como error temprano).
3. **`let suggested: Int = match priority.suggest_priority(...) {
   Ok(p) => p, Err(_) => 3 }`** → introducir `let v: Int = p` adentro
   del arm Ok para forzar coerción PyAny → Int de Fase 8.4. Deuda
   menor del codegen (coerción adentro de match arms no se propaga
   desde la anotación destino del `let` contenedor).

Documentación: `docs/deudas-post-5b.md` reescribe la sección "Blocker
REMAINING" como "Mini-fase post-2026-06-08 CERRADA v0.15.11" con
diagnóstico correcto + las 3 deudas residuales menores listadas
para futuras mini-fases si entra demanda real.

## [v0.15.10] — 2026-06-08 — Codegen interop Python + ORM: Date/DateTime/Uuid + skip virtuales (deuda TaskHub parcial)

**Mini-fase del codegen** descubierta al smoke-testear el
boilerplate `taskhub` (showcase del stack completo). El
boilerplate combina por primera vez **ORM con Date/DateTime/Uuid +
interop Python + relations virtuales + cron/background** en un
solo programa, exponiendo gaps del codegen que no se manifiestan
en los 10 boilerplates anteriores. **5 fixes coordinados al
codegen + 1 fix de cap doc**.

### Fix del codegen (`src/codegen.rs`, ~115 LoC nuevas)

1. **`impl __FitzToPy` para `chrono::NaiveDate` / `DateTime<Utc>` /
   `uuid::Uuid`** en el preludio Python (línea ~7128+). Serializan
   a Python `str` canonical (ISO 8601 `YYYY-MM-DD` / RFC 3339 /
   UUID canonical). Antes el preludio solo tenía `i64`/`f64`/
   `bool`/`String`/`()`/`__FitzPyObject` — los types Fitz
   `Date`/`DateTime`/`Uuid` no tenían impl y rustc rompía al
   intentar `self.created_at.__fitz_to_py(...)` adentro del
   `gen_python_helpers_for_type`.
2. **Branches `Date`/`DateTime`/`Uuid` en `py_field_extract_arms`**
   (Python → Fitz, no-nullable). Parsean Python `str` al type
   Rust nativo (`NaiveDate::parse_from_str(&s, "%Y-%m-%d")` /
   `DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc))` /
   `Uuid::parse_str(&s)`).
3. **Mismas branches en `py_inner_extract_for_nullable`**
   (cubre `Date?`/`DateTime?`/`Uuid?`). Antes el `_ => Err(...)`
   rechazaba con *"field `X` de tipo `Y` (nullable): inner type
   compuesto no soportado todavía"*.
4. **Skip de virtual fields en `gen_fitz_py_to_instance_helper`**
   (W17 paralelo — v0.10.7 hizo lo mismo para `__FromFitzJson`/
   `__ToFitzJson`). Companion fields BelongsTo / `@has_many` /
   `@has_one` se inicializan con `Default::default()` en el
   struct literal en lugar de intentar extraerlos del PyDict
   (son sentinels del ORM, no datos que vengan de Python).
   `gen_python_helpers_for_type` + `gen_fitz_py_to_instance_helper`
   ahora aceptan `meta: Option<&TableMetadata>` análogamente a
   `gen_type_http_impls_for_sig_with_meta`. Dos call sites
   actualizados (local types en `gen_type_def` + cross-module
   en `emit_python_helpers_for_imported_types`).
5. **Branches `Date`/`DateTime`/`Uuid` en `.update(db, Map var)`**
   runtime match (~30 LoC en líneas 19387+). Acepta
   `__FitzValue::Str(s)` y emite `__FitzPgValue::Text(s.clone())`
   paralelo a cómo `impl __IntoPgValue for chrono::NaiveDate`
   ya hace en `emit_date_uuid_db_prelude`. Cubre tanto Date/
   DateTime/Uuid no-nullable como `Date?`/`DateTime?`/`Uuid?`.

### Fix M7.C1 app.fitz (pre-existente roto descubierto en smoke)

`examples/curso/m7-python-interop/c1-setup/app.fitz` tenía 4
errores del checker post-Fase 8.3 que no se actualizaron cuando
el wrap de `Result<T>` automático sobre calls Python aterrizó.
Reescrito con los patrones canónicos:

- `let pi: Float = math.pi` para coerción auto-primitive PyAny → Float.
- `let now = datetime.datetime.now()?` + `now.isoformat()?` para
  propagar excepciones Python (handler retorna `Result<Map<...>>`).
- `return json.dumps(payload)` directo (handler retorna `Result<Str>`).

### Validación

- `cargo test --release --lib --features python -- --test-threads=2`:
  **3121 unit tests verdes, 0 failed**.
- `fitz check` sobre **10 boilerplates** existentes: **10/10
  verdes**.
- `fitz check` sobre **99 guide examples**: **99/99 verdes**.
- `fitz check` sobre **5 curso examples**: **5/5 verdes** (después
  del fix M7.C1).
- `fitz build` sobre `boilerplates/taskhub`: sigue bloqueado por
  **otra deuda independiente** (`db_result` no en scope para
  `@background`/`@cron` handlers — error `E0425` de rustc cuando
  el handler async fn referencia el binding top-level). **No es
  regresión de este fix** — preexistía y se manifiesta porque
  TaskHub es el primer programa que combina @background/@cron con
  state global. Documentado en
  [`docs/deudas-post-5b.md`](docs/deudas-post-5b.md) como blocker
  separado.

### Verificación del CI ghcr.io (falsa alarma inicial)

Primer smoke local sugería que `ghcr.io/thegreekman76/fitz:latest-python`
estaba desactualizada (rechazaba sintaxis de v0.10.30+). **Era
imagen Docker local cacheada vieja**. Forzar `docker image rm` +
`docker pull` resuelve — la imagen GHCR está al día con v0.15.0
(verificado con `docker run --rm <img> fitz --version`).
CI release.yml job `docker-image-python` corre OK en cada release
tag (4m 9s en v0.15.0). No requiere fix del CI.

### Bug doc de TaskHub C5 — fixeado en boilerplate

El cap C5 del proyecto Construyendo TaskHub usaba `db.query(sql, [])`
(módulo `db`) en el endpoint admin `GET /api/jobs` cuando debería
ser `conn.query(sql, [])` (la connection bindeada). El intérprete
lo permitía pero el codegen solo soporta `db.connect`. Fixeado en
`boilerplates/taskhub/src/main.fitz`. **Deuda secundaria
documentada**: inconsistencia intérprete vs codegen sobre
`module.method` dispatch.

### Bug doc de TaskHub C4 — workaround documentado

Background fn usaba `match Task.where(...).first(conn).await { Ok(t) => t, ... }`
sin anotación explícita del tipo. El codegen rechaza el field
access posterior con *"field access sobre `T?`: solo se soporta
sobre instancias de tipos custom"*. Workaround: anotación
explícita `let task: Task = match ...`. Fixeado en el boilerplate.
**Deuda menor del codegen**: el checker entiende el patrón
`Ok(t) => t` pero el codegen no infiere el tipo desde el match
arm.

### Sin breaking changes

100% aditivo. Cero regresiones sobre tests + boilerplates + guide
+ curso. Sin cambios al lenguaje user-facing (los types Date/
DateTime/Uuid + interop Python siguen igual — solo se amplían
los casos que el codegen acepta).

## [v0.15.9] — 2026-06-07 — TaskHub C7 + cierre del proyecto entero + boilerplates/taskhub publicado

**Cap final del proyecto Construyendo TaskHub** (7 caps cerrados
entre C1-C7) + publicación del boilerplate descargable
`boilerplates/taskhub/` (11mo boilerplate del repo, el más
completo, showcase del stack único de Fitz integrado).

### Cap C7 entregado

- **[`docs/taskhub/c7-observability-frontend-deploy.md`](docs/taskhub/c7-observability-frontend-deploy.md)**
  (~1100 LoC) — 8 pasos cubriendo: activar
  `@server(prometheus=true)` para `/metrics` scrapeable +
  Prometheus UI target UP, validar OTel tracing en Jaeger UI
  (spans automáticos por request HTTP + handlers cron/background
  con `user.id`/`user.role` extraído del `@auth_provider`),
  `@healthz fn check_db_alive() -> Bool` + `@readyz fn
  check_ready_for_traffic() -> Bool` auto-mount-ed con check
  real contra `db_result.is_closed()` + SIGTERM drain
  automático (readyz pasa a 503 al recibir SIGTERM, K8s deja de
  rutear), frontend vanilla JS funcional (~500 LoC sin
  frameworks ni build): `index.html` + `style.css` + `api.js`
  (wrappers fetch con Bearer) + `ws.js` (cliente WebSocket con
  reconexión auto) + `app.js` (router por hash + login screen
  + projects list + board kanban con drag&drop HTML5 + live
  updates WS), `nginx.conf` con `try_files $uri $uri/
  /index.html` para SPA routing, Dockerfile final con Path A
  (`fitz build --bundle-python --bundle-pip openai` + base
  distroless ~50 MB) y Path B fallback documentado
  (python:3.12-slim ~250 MB), publicación al boilerplate como
  cierre del proyecto. Bar editorial: header de cierre +
  mermaid completo + tabla comparativa con Python+FastAPI /
  Node+Express+pm2 / Spring Boot (10 rows incluyendo image
  size, cold start, memory idle, prometheus auto, OTel auto,
  healthz/readyz, SIGTERM drain, frontend bundling,
  single-binary, 12-factor), validación checklist con 8 items,
  troubleshooting con 7 casos, cierre del proyecto con stack
  integrado en TaskHub (7 caps en tabla), comparativa final vs
  stack típico (10 métricas), ideas para fork.

### Ejemplo runnable C7

`examples/taskhub/c7-observability-frontend-deploy/` — estado
final del proyecto. Copia de C6 con:

- **`src/main.fitz`**: `@healthz`/`@readyz` con check real de DB
  + `@server(8080, ws_heartbeat_secs=30, prometheus=true)`.
  ~720 LoC integrando el stack completo final.
- **`frontend/`** completo: `index.html` + `assets/style.css`
  + `assets/api.js` + `assets/ws.js` + `assets/app.js` (~500
  LoC total vanilla JS). Login + projects + board kanban
  drag&drop + WS reconnect + live refresh.
- **`Dockerfile`** con Path A activo (bundling + distroless ~50
  MB) y Path B comentado como fallback (~250 MB).

### Boilerplate publicado

**`boilerplates/taskhub/`** — copia del C7 con README dedicado
de ~250 LoC enfocado en "clonalo y arrancá" (sin material
pedagógico):

- Tabla del stack incluido (11 piezas: HTTP, Auth, RBAC, ORM,
  Migrations, WS, Cron, Interop Python, Observability,
  Frontend, Deploy).
- Quickstart con `git clone` + `cp .env.example .env` +
  `docker compose up -d --build` + bootstrap admin = TaskHub
  corriendo en ~30s.
- Tabla del compose con 5 services + endpoints accesibles.
- Modelo de datos con FK constraints + indexes.
- 3 roles (admin/owner/member) con flow de promote manual.
- Cron jobs incluidos + GET /api/jobs audit log.
- Interop Python sweet spot (LLM con fallback heurística).
- Ideas para fork (cambio de dominio, más roles, otros LLMs,
  email real, billing).
- Links al material pedagógico Construyendo TaskHub para
  entender cada pieza.
- Limitaciones honestas del MVP (path params en @ws, broadcast
  HTTP→WS, bundling alternativos, sync Python).
- Comparativa con FastAPI+Celery / Express+bull / Spring Boot
  (10 métricas).

`boilerplates/README.md` actualizado de "10 boilerplates" a "11
boilerplates" con entry `taskhub 🏔️` marcado como **"Showcase
final: stack único de Fitz completo"**.

### Cierre del proyecto Construyendo TaskHub

**7 caps cerrados entre 2026-06-07** (todos en una sesión
intensiva):

- C1 — Setup Docker-first (5 services del compose).
- C2 — Schema + workflow `fitz db` versionado.
- C3 — Auth con RBAC custom (3 roles apilables).
- C4 — CRUD + relations + WebSocket en vivo.
- C5 — Cron + background jobs con persistencia.
- C6 — Interop Python con LLM.
- C7 — Observability + frontend + deploy production + boilerplate.

**Total proyecto**: ~7000 LoC docs/caps + ~3000 LoC ejemplos
runnable + boilerplate completo. **Único showcase pedagógico
del stack entero de Fitz** integrado en una app real
production-ready (~50 MB image con bundling, ~330 MB compose
total).

### Cross-refs actualizados

- **`mkdocs.yml`**: nav del tab suma entry C7. Tab
  "Construyendo TaskHub" ahora completo con index + 7 caps.
- **`docs/taskhub/index.md`**: tabla del roadmap C7 + Post-C7
  marcados como CERRADOS, con link real al boilerplate.
- **`docs/taskhub/c6-interop-python-llm.md`**: "Próximo cap"
  linkea al C7 real.
- **`examples/taskhub/c6-interop-python-llm/README.md`**: "Qué
  viene" linkea al C7.
- **`boilerplates/README.md`**: tabla actualizada de "10
  boilerplates" a "11 boilerplates" con entry taskhub destacado
  con 🏔️.

### Validación

- `mkdocs build` non-strict: 16.62s, sin warnings nuevos sobre
  los archivos TaskHub (una warning sobre anchor stale a
  `guide.md#33-observability` — pre-existente del patrón del
  resto del sitio).
- `fitz check examples/taskhub/c7-observability-frontend-deploy/src/main.fitz`
  → "sin errores de tipo". El programa con
  `@server(prometheus=true)` + `@healthz`/`@readyz` + todo el
  stack acumulado de los 6 caps anteriores pasa el checker
  estático.
- Smoke real end-to-end NO automatizado — requiere Docker +
  Path A del bundling funcionando (toolchain con
  `--features python` + `--bundle-python` + `--bundle-pip`).
  Path B fallback documentado para entornos donde Path A falla.

### Sin cambios

Sin cambios de código del lenguaje, sin cambios al stack —
release v0.15.9 patch 100% docs/material pedagógico + boilerplate
nuevo.

## [v0.15.8] — 2026-06-07 — TaskHub C6: Interop Python con LLM (priorización IA)

Sexto cap de **Construyendo TaskHub**. **El cap más diferenciador
del ecosistema**: integra un módulo Python real (`priority.py`)
desde el binario nativo Fitz, demostrando el patrón **`from
python import` + `match Result<Int>`** para handle de errores
Python en compile-time. El módulo decide internamente entre LLM
real (OpenAI gpt-4o-mini) si `OPENAI_API_KEY` está set, o
heurística por keywords como fallback puro Python — la complejidad
del fallback vive donde están las libs (Python), Fitz solo consume
el resultado tipado.

### Cap C6 entregado

- **[`docs/taskhub/c6-interop-python-llm.md`](docs/taskhub/c6-interop-python-llm.md)**
  (~1000 LoC) — 10 pasos cubriendo: pre-requisitos
  (Python 3.10+ local + `cargo build --release --features python`
  para el binario fitz, OpenAI API key opcional), `python/priority.py`
  con `suggest_priority(title, description) -> int` que decide
  LLM-vs-heurística adentro de Python, `python/requirements.txt`
  con `openai>=1.0` opcional, `from python import priority` al
  tope de `main.fitz` + handler `POST /api/tasks/{id}/suggest-priority`
  con scope check (admin / owner / assignee) y `match Result<Int>
  { Ok(p) => p, Err(_) => 3 }` como fallback de emergencia, cache
  en `task.ai_suggested_priority` (field ya declarado desde C2 sin
  migration nueva), Dockerfile actualizado a
  `python:3.12-slim-bookworm` (image ~150 MB → ~250 MB, C7
  optimiza con `--bundle-python`), docker-compose con
  `OPENAI_API_KEY` env opcional, validación local + en Docker con
  curl. Bar editorial: header + mermaid + tabla comparativa con
  Python+FastAPI / Node+Express / Spring+langchain4j / Rails+ruby-openai
  (rows sobre ecosistema libs LLM, llamar lib externa, manejo de
  excepciones, compile-time check del shape, distribución binario,
  latency, async support, fallback), validación checklist con 10
  items, troubleshooting con 6 casos.

### Ejemplo runnable

`examples/taskhub/c6-interop-python-llm/` — estado del proyecto al
cerrar C6. Copia de C5 con:

- **`python/priority.py`** + **`python/requirements.txt`** —
  módulo nuevo con LLM + heurística + decisión interna.
- **`src/main.fitz`** extendido con: `from python import priority`
  + handler `suggest_task_priority` con scope check + `match
  Result<Int>` + cache en DB. `main.fitz` ahora ~670 LoC.
- **`Dockerfile`** actualizado a `python:3.12-slim-bookworm` con
  `PYTHONPATH=/app/python` + `pip install -r requirements.txt`.
- **`docker-compose.yml`** suma `OPENAI_API_KEY: ${OPENAI_API_KEY:-}`
  con default vacío al service `app`.
- **`.env.example`** documenta `OPENAI_API_KEY` opcional con
  comentarios sobre costo (~$0.0001/call) y latency (~500ms-1s).
- **`.gitignore`** suma `.venv/`, `__pycache__/`, `*.pyc`, `*.pyo`.
- README dedicado con setup local (venv) + setup Docker +
  validación con dos tasks de prioridades distintas (urgent → 5,
  refactor → 2) + trade-offs honestos.

### Decisiones técnicas tomadas

- **Decisión LLM vs heurística dentro de Python**: el módulo
  `priority.py` chequea `OPENAI_API_KEY` internamente. Si está,
  intenta el LLM; si falla (network, rate limit, parse), cae a
  heurística. Fitz solo conoce el entry point `suggest_priority(...)`.
  Esto evita refactorear el handler Fitz a `?.await` async
  (limitación heredada del MVP — `let fut = py_call()?; fut.await`
  no compila por la deuda residual de Fase 8.7).
- **`match Result<Int>` con fallback de emergencia**: si Python
  mismo falla (módulo no instalado, ImportError, etc.), el match
  cae a `3` (medium). Esto significa que **el endpoint nunca
  rompe** — el peor caso es una sugerencia subóptima.
- **Sin migration nueva**: el field `ai_suggested_priority: Int?`
  fue declarado en C2 previendo este cap.
- **Sync Python (no async)**: el cap simplifica el patrón usando
  `def` en lugar de `async def`. La versión async con `?.await`
  queda documentada como refinamiento futuro (refactor del
  módulo a `async def` con `import httpx` en lugar de openai
  sync).
- **Image size 150 → 250 MB**: trade-off honesto del cap.
  Documentado upfront + referenciado el C7 para `--bundle-python`
  que optimiza a ~50 MB con base distroless.
- **Builder image `ghcr.io/thegreekman76/fitz:latest-python`** asumido:
  si la variante no existe pre-built, workaround documentado es
  build local + COPY del binario al Dockerfile.
- **Fallback heurístico por keywords**: 5 niveles (urgent/asap/
  critical/blocker/p0 → 5, bug/fix/error/crash/broken → 4,
  refactor/cleanup/test/docs/comment → 2, default → 3). Simple
  pero funcional para el cap.

### Cross-refs actualizados

- **`mkdocs.yml`**: nav del tab suma entry C6.
- **`docs/taskhub/index.md`**: tabla del roadmap C6 con link real
  + descripción ampliada (diferenciador único).
- **`docs/taskhub/c5-cron-jobs-persistencia.md`**: "Próximo cap"
  linkea al C6 real.
- **`examples/taskhub/c5-cron-jobs-persistencia/README.md`**: "Qué
  viene" linkea al C6.

### Validación

- `mkdocs build` non-strict: 19.76s, sin warnings nuevos sobre
  los archivos TaskHub.
- `fitz check examples/taskhub/c6-interop-python-llm/src/main.fitz`
  → "sin errores de tipo". El programa con `from python import
  priority` + handler con `match Result<Int>` pasa el checker
  estático **incluso sin feature python habilitado en el binario**
  (el checker es estático; el guard real es en runtime/codegen
  per Fase 8.1.2).
- Smoke real end-to-end NO automatizado — requiere Docker +
  Python feature en el binario + venv local con openai instalado
  (opcional). Documentado paso a paso en README + cap.

### Sin cambios

Sin cambios de código del lenguaje, sin cambios al stack —
release v0.15.8 patch 100% docs/material pedagógico.

## [v0.15.7] — 2026-06-07 — TaskHub C5: Cron + background jobs con persistencia

Quinto cap de **Construyendo TaskHub**. Sobre el CRUD + WS del
C4 sumamos **cron + background + spawn + persistencia** — el cap
que **borra Celery del compose**. Sin Celery, sin Redis, sin
worker separados: todo vive en el binario con `tokio` scheduler
+ tablas `fitz_cron_jobs` + `fitz_cron_runs` auto-creadas en
Postgres. Diferenciador estructural fuerte vs stacks típicos
Python+Celery+Redis (que suman ~450 MB al compose).

### Cap C5 entregado

- **[`docs/taskhub/c5-cron-jobs-persistencia.md`](docs/taskhub/c5-cron-jobs-persistencia.md)**
  (~950 LoC) — 6 pasos cubriendo: `@cron("0 0 3 * * *", tz="UTC",
  retry={max:3, backoff:"exponential", initial_secs:30,
  max_secs:300}, store=db_result) async fn cleanup_old_tasks()`
  (cron handler sin params, accede `db_result` del closure scope),
  `@cron("0 0 9 * * *", tz="UTC", retry={..., backoff:"linear"},
  store=db_result) async fn daily_due_reminders()` (busca tasks
  con `due_date` próxima + dispara `spawn(send_due_reminder(...))`
  por cada una), `@background async fn send_due_reminder(task_id:
  Int) -> Null` (mock email envío con print + integración futura
  con SendGrid/Postmark/SES), `spawn(send_due_reminder(new_task.id))`
  desde `POST /api/projects/{id}/tasks` cuando `due_date != null`,
  endpoint admin `GET /api/jobs` con `db.query` crudo +
  typed accessors (`r.get_str(...)?` / `r.get_int(...)?`) leyendo
  `fitz_cron_runs`, tests end-to-end con forzar el cron a
  schedule corto para validación. Bar editorial: header + mermaid
  + tabla comparativa con Python+Celery / Node+bull / Spring+Quartz
  / Rails+Sidekiq (rows sobre services compose, imagen total,
  broker externo, setup scheduler, workers, persistencia retry
  TZ catch-up cron-only mode), validación checklist con 9 items,
  troubleshooting con 5 casos (incluido el `fitz_cron_runs` crece
  sin parar — soluciona con otro cron de cleanup del audit log).

### Ejemplo runnable

`examples/taskhub/c5-cron-jobs-persistencia/` — estado del proyecto
al cerrar C5. Copia de C4 con:

- **`src/main.fitz`** extendido con: 2 `@cron` handlers,
  `@background async fn send_due_reminder`, `JobRun` type +
  `GET /jobs` admin endpoint con SQL crudo, modificación al
  `create_task` handler para disparar `spawn(...)` cuando
  `due_date != null`. Total `main.fitz` ahora ~600 LoC integrando
  el stack completo.
- **Sin cambios al schema ni migration**. Las tablas
  `fitz_cron_jobs` + `fitz_cron_runs` se crean automáticamente al
  boot del scheduler (idempotente con `CREATE TABLE IF NOT EXISTS`).
- README dedicado con setup + validación + sección "Forzar
  ejecución de cron para testing" + 3 descubrimientos del checker
  documentados.

### 3 descubrimientos del checker durante la implementación

El cap C5 obligó a aprender 3 restricciones del MVP no obvias —
documentadas en el cap doc + README:

1. **`@cron` handlers no aceptan params**. Signature exacta es
   `async fn () -> Result<Null>`. El kwarg `store=db_result` es
   para PERSISTENCIA DE METADATA DE RUNS, NO para inyectar `db`
   en el handler. Para acceder a la DB, el handler hace
   `let conn = match db_result { Ok(c) => c, Err(_) => return ... }`
   desde el closure scope.
2. **Strings multi-línea no compilan**. SQL queries largas van
   en una sola línea o se concatenan con `+` (también en una
   sola línea — concatenación multi-línea tampoco funciona).
3. **Closures multi-línea no compilan**. `fn(t) => predicate`
   debe estar en una sola línea. WHERE complejos como
   `t.due_date != null and t.due_date >= today and ...` van
   inline con `and`/`or`.

### Cross-refs actualizados

- **`mkdocs.yml`**: nav del tab suma entry C5.
- **`docs/taskhub/index.md`**: tabla del roadmap C5 con link real
  + descripción ampliada (sin Celery, sin Redis).
- **`docs/taskhub/c4-crud-relations-ws.md`**: "Próximo cap" linkea
  al C5.
- **`examples/taskhub/c4-crud-relations-ws/README.md`**: "Qué
  viene" linkea al C5.

### Validación

- `mkdocs build` non-strict: 15.25s, sin warnings nuevos sobre
  los archivos TaskHub.
- `fitz check examples/taskhub/c5-cron-jobs-persistencia/src/main.fitz`
  → "sin errores de tipo". El programa con 2 `@cron` + 1
  `@background` + `spawn(...)` + endpoint admin con SQL crudo +
  typed accessors pasa el checker en bloque.
- Smoke real end-to-end NO automatizado — requiere Docker +
  scheduler corriendo + esperar a las 3am o forzar schedule
  corto. Documentado paso a paso en README + cap.

### Sin cambios

Sin cambios de código del lenguaje, sin cambios al stack —
release v0.15.7 patch 100% docs/material pedagógico.

## [v0.15.6] — 2026-06-07 — TaskHub C4: CRUD + relations + WebSocket en vivo

Cuarto cap de **Construyendo TaskHub**. Sobre el auth + RBAC del
C3 sumamos el **corazón funcional del producto**: CRUD HTTP para
projects + tasks con `@belongs_to` / `@has_many` decorators +
companion fields + `.preload("tasks")` para eager loading sin
N+1, y un canal WebSocket global tipado con `WsConn<TaskEvent>` +
broadcast simétrico que demuestra updates en vivo entre múltiples
clientes. El cap documenta **dos limitaciones honestas del MVP**
del lenguaje y muestra los workarounds canónicos.

### Cap C4 entregado

- **[`docs/taskhub/c4-crud-relations-ws.md`](docs/taskhub/c4-crud-relations-ws.md)**
  (~1000 LoC) — 10 pasos cubriendo: sumar `@has_many("Task",
  via="project_id", on_delete="cascade") tasks: List<Task> = []`
  en Project + `@belongs_to("Project", on_delete="cascade")
  project_id: Int` + companion `project: Project?` en Task (sin
  migration — las FK constraints ya están con ON DELETE CASCADE
  en `initial_schema.sql` del C2), tipos auxiliares con
  **sentinels** (`Str = ""`, `Int = 0`) en `UpdateTaskInput`,
  `POST /api/projects` con `owner_id = user.id`, `GET /api/projects`
  con scope por rol (admin bypass, otros con WHERE `owner_id =
  user.id`), `GET /api/projects/{id}` con `.preload("tasks")` para
  eager load, `POST /api/projects/{project_id}/tasks` con
  verificación de ownership, `PUT /api/tasks/{id}` con triple
  scope (admin OR owner OR assignee) + nullable refinement en
  match arm (patrón `null` + bare ident, NO `Some(a)`), canal
  WebSocket global `@authenticated @ws("/ws/events")` con
  `WsConn<TaskEvent>` + filtrado client-side por `project_id`,
  tests end-to-end con curl + wscat.
- **Dos limitaciones del MVP documentadas honestamente**:
  1. **HTTP handlers no triggerean broadcasts WS**: `conn.broadcast`
     solo funciona desde dentro de un handler `@ws`. Patrón
     canónico: cliente que mutó por HTTP **también** emite un
     frame WS para informar a otros. Futura API global
     `Ws.broadcast(endpoint, event)` cubrirá el caso.
  2. **`@ws` no acepta path params**: `@ws("/path")` exige Str
     literal. Patrón canónico: canal global + cada frame incluye
     identificador (`project_id`) + filtrado client-side. Futuro
     soporte de path params o subscription tracking server-side.
- Bar editorial: header + mermaid + tabla comparativa con
  Rails ActiveRecord + ActionCable / Django ORM + Channels /
  Sequelize + Socket.IO / TypeORM + ws (con rows específicos
  sobre relations declarativas + eager loading + WS auth en
  handshake + frame typing + AsyncAPI auto + heartbeat),
  validación checklist con 11 items, troubleshooting con 6
  casos.

### Ejemplo runnable

`examples/taskhub/c4-crud-relations-ws/` — estado del proyecto al
cerrar C4. Copia de C3 con:

- **`src/main.fitz`** extendido con: `@has_many` + `@belongs_to` +
  companion fields en Project/Task, 4 tipos auxiliares nuevos
  (`CreateProjectInput`, `CreateTaskInput`, `UpdateTaskInput`,
  `TaskEvent` con `project_id`), 5 handlers HTTP nuevos
  (create_project, list_projects, get_project, create_task,
  update_task), 1 handler WS (`task_events`) con marshaling
  automático JSON ↔ TaskEvent. `@server(8080, ws_heartbeat_secs=30)`
  activa ping/pong automático.
- **Sin cambios al schema** — relations son decisión de código.
- README dedicado con setup + validación end-to-end (curl + wscat)
  + nota sobre la limitación del MVP.

### Descubrimientos hechos durante la implementación

Tres ajustes al cap doc + ejemplo después de correr `fitz check`:

1. **Match arms con side effects + `;` separator no compilan**.
   El boilerplate `api-orm-full/src/posts.fitz` evita el problema
   usando sentinels (`Str = ""`, `Int = 0`) en lugar de nullables
   en `UpdateInput`, con `if (field != "") { changes[...] = ... }`
   adentro del handler. Adoptado el patrón.
2. **`@ws("/path")` exige Str literal**: el checker rechaza
   `@ws("/ws/projects/{id}")` con mensaje *"el argumento debe ser
   un Str literal (path)"*. Refactorizado a canal global
   `@ws("/ws/events")` + `project_id` en el frame + filtrado
   client-side.
3. **Nullable match pattern: `null` + bare ident, NO `Some(a)`**.
   El refinement de nullables en match arms (heredado del
   CLAUDE.md Pattern::Ident sobre Nullable) usa
   `match x { null => ..., a => ... }` donde `a` está refinada
   al inner `T`. `Some(a)` es para Result, no para nullable.

### Cross-refs actualizados

- **`mkdocs.yml`**: nav del tab suma entry C4.
- **`docs/taskhub/index.md`**: tabla del roadmap actualiza C4 con
  link real + descripción ampliada mencionando la limitación
  honesta.
- **`docs/taskhub/c3-auth-rbac.md`**: "Próximo cap" linkea al C4.
- **`examples/taskhub/c3-auth-rbac/README.md`**: "Qué viene"
  linkea al C4.

### Validación

- `mkdocs build` non-strict: 22.57s, sin warnings nuevos sobre
  los archivos TaskHub.
- `fitz check examples/taskhub/c4-crud-relations-ws/src/main.fitz`
  → "sin errores de tipo". El programa con `@has_many` +
  `@belongs_to` + companion fields + `.preload(...)` + canal WS
  global + 5 handlers HTTP + 1 handler WS + nullable refinement
  pasa el checker en bloque.
- Smoke real end-to-end NO automatizado — requiere Docker +
  migrations + admin bootstrap manual + 3 users + multi-terminal
  wscat. Documentado paso a paso en el README + cap.

### Sin cambios

Sin cambios de código del lenguaje, sin cambios al stack —
release v0.15.6 patch 100% docs/material pedagógico.

## [v0.15.5] — 2026-06-07 — TaskHub C3: Auth con RBAC custom de 3 roles apilables

Tercer cap de **Construyendo TaskHub**. Sobre el schema del C2
sumamos auth nativa con JWT + Argon2id + `@auth_provider`
singleton + `@authenticated` / `@requires("admin"|"owner"|"member")`
apilable (semántica OR) — todo demostrado end-to-end con tests por
rol usando curl. El RBAC custom es **un diferencial fuerte del
lenguaje** vs Spring Security / FastAPI / Express donde se resuelve
con middleware ad-hoc o dependency injection runtime.

### Cap C3 entregado

- **[`docs/taskhub/c3-auth-rbac.md`](docs/taskhub/c3-auth-rbac.md)**
  (~880 LoC) — 11 pasos cubriendo: `@hidden` decorator sobre
  `User.password_hash` (column sigue en DB pero no aparece en JSON
  responses), tipos auxiliares (`RegisterInput`, `LoginInput`,
  `LoginResponse`, `PromoteInput`, `StatsResponse`) separados del
  shape DB, `@auth_provider async fn check_token(headers)` que
  valida el Bearer JWT contra la DB (lookup por email contra el
  UNIQUE index — un demote surte efecto inmediato sin esperar al
  expiry del token), `POST /api/auth/register` con `hash.password`
  Argon2id + validación temprana + check de unicidad antes del
  INSERT, `POST /api/auth/login` con `hash.verify` + `jwt.encode`
  HS256 + mismo mensaje "credenciales inválidas" para evitar
  enumeración de usuarios, `GET /api/me` con `@authenticated`
  (handler de una línea), `GET /api/users` y
  `POST /api/users/{id}/promote` con `@requires("admin")`,
  `GET /api/stats` con `@requires("admin") @requires("owner")`
  apilable (demo de semántica OR), bootstrap manual del primer
  admin via `psql UPDATE`, tests end-to-end con curl validando
  cada rol contra cada endpoint. Bar editorial: header + mermaid
  + tabla comparativa con Spring Security / FastAPI custom decorator
  / Express + middleware (incluye rows específicos sobre validación
  estática del checker, hide password, hierarchies de roles),
  validación checklist con 12 items, troubleshooting con 5 casos.

### Ejemplo runnable

`examples/taskhub/c3-auth-rbac/` — estado del proyecto al cerrar
C3. Copia de C2 con:

- **`src/main.fitz`** extendido con: `@hidden` en password_hash,
  6 tipos auxiliares nuevos, helper `find_user_by_email`,
  `@auth_provider async fn check_token`, 6 handlers nuevos
  (register / login / me / list_users_admin / promote_user /
  stats) más el `/healthz` existente. **Sin cambios al schema**
  — `@hidden` es decisión de código.
- README dedicado con setup desde cero + validación end-to-end
  con curl (registro de admin + bootstrap manual + login + tests
  por cada rol + demo del apilable `@requires("admin") @requires("owner")`).

### Decisiones técnicas tomadas

- **`@auth_provider async fn`** (no bare `fn`) — el provider hace
  query a DB con `.await` adentro. Singleton del programa
  (checker rechaza dos providers).
- **JWT claims `Map<Str, Str>` con `email` + `role` snapshot**.
  El provider revalida `role` contra DB en cada request
  (security against stale claims) — el snapshot del claim sirve
  para debug del JWT en jwt.io, no para decidir RBAC.
- **`@hidden` en `password_hash`** — el field sigue en DB
  (column `text NOT NULL DEFAULT ''`) pero el codegen del
  `__ToFitzJson` lo omite. Sin migration necesaria.
- **Bootstrap del primer admin manual via `psql UPDATE`** — el
  chicken-and-egg de "necesitás admin para crear admin" se
  resuelve una sola vez en la vida del proyecto. Refinamiento
  futuro: `.fitz` migration nativa que lee `INITIAL_ADMIN_EMAIL`
  env var y eleva ese user al boot.
- **`@requires("admin") @requires("owner")` apilable = OR**. Un
  user tiene **un solo `role: Str`** (no lista), pedir AND
  sería incoherente. Para hierarchies más complejas (membership
  en N grupos) → tabla relacional `user_roles` + JOIN, fuera del
  scope del RBAC declarativo.
- **Mismo mensaje "credenciales inválidas"** en login para email
  inexistente y password wrong — mitigation contra enumeración
  de usuarios (estándar en stacks production).

### Cross-refs actualizados

- **`mkdocs.yml`**: nav del tab suma entry C3.
- **`docs/taskhub/index.md`**: tabla del roadmap actualiza C3 de
  "(próximamente)" a link al cap + descripción ampliada.
- **`docs/taskhub/c2-schema-migraciones.md`**: "Próximo cap"
  linkea al C3 real.
- **`examples/taskhub/c2-schema-migraciones/README.md`**: "Qué
  viene" linkea al cap C3 + descripción.

### Validación

- `mkdocs build` non-strict: 17.99s, sin warnings nuevos sobre
  los archivos TaskHub.
- `fitz check examples/taskhub/c3-auth-rbac/src/main.fitz` →
  "sin errores de tipo". El programa con auth completo +
  `@requires` apilable + tipos auxiliares + helpers + 7 handlers
  pasa el checker en bloque. **El checker valida el shape exacto
  del `@auth_provider`** (signature `Map<Str, Str> -> Result<User>`),
  exige `role: Str` no nullable en `User`, rechaza duplicados de
  `@requires("admin")` en handlers apilados, y conoce qué endpoints
  están detrás de cada rol.
- Smoke real end-to-end NO automatizado — requiere Docker + las
  migrations aplicadas + admin bootstrap manual. Documentado paso
  a paso en el README + el cap.

### Sin cambios

Sin cambios de código del lenguaje, sin cambios al stack —
release v0.15.5 patch 100% docs/material pedagógico.

## [v0.15.4] — 2026-06-07 — TaskHub C2: Schema + workflow `fitz db` end-to-end

Segundo cap de **Construyendo TaskHub** (proyecto integrador
post-curso). Sobre el setup Docker-first del C1 sumamos el schema
del dominio (4 `@table type`) + el workflow versionado completo
de `fitz db` aplicado en un setup Docker-first real + CI con
drift check.

### Cap C2 entregado

- **[`docs/taskhub/c2-schema-migraciones.md`](docs/taskhub/c2-schema-migraciones.md)**
  (~880 LoC) — 9 pasos cubriendo: declaración de los 4 `@table type`
  (User / Project / Task / Comment con sus tipos primitivos +
  nullables + defaults), conexión a la DB con `db.connect()`
  top-level + endpoint smoke `GET /api/users`, helper `dev-env.sh`
  para exportar `DATABASE_URL` apuntando al `db` expuesto en
  `localhost:5432`, workflow canónico (`fitz db new` → editar
  `@table` → `fitz db diff` → editar UP/DOWN + agregar FK constraints
  + indexes manualmente → `fitz db migrate`), verificación contra
  Postgres con `psql`, **rebuild del binario** con `docker compose
  up -d --build app` para incorporar el schema declarado, cambio
  de schema posterior (agregar `Task.estimated_hours: Int = 0`) +
  segunda migration + rollback, `fitz db history` para audit log,
  **drift check en CI con GitHub Actions** (job con service
  container Postgres + `fitz db migrate` + `fitz db check`).
- Bar editorial del curso: header con pre-reqs + objetivo + por qué
  importa, mermaid del flujo, tabla "Por qué Fitz es distinto" vs
  Alembic / TypeORM en setup Docker-first (incluye row específico
  sobre rebuild del binario tras schema change), validación
  checklist con 8 items, troubleshooting con 5 casos típicos
  (drift falso, `connection refused`, 500 en `/api/users`, rollback
  sin DOWN, builds lentos).

### Ejemplo runnable

`examples/taskhub/c2-schema-migraciones/` — estado del proyecto al
cerrar el cap C2. Copia de `examples/taskhub/c1-setup/` con:

- **`src/main.fitz`** extendido (4 `@table type` + `db.connect()`
  top-level + endpoint smoke `GET /users` con `User.all(db)`).
- **`migrations/20260607130000_initial_schema.sql`** — primera
  migration con `CREATE TABLE` para los 4 types + FK constraints
  (`REFERENCES ... ON DELETE CASCADE` / `SET NULL`) + 5 indexes
  recomendados + sección `-- DOWN` con `DROP TABLE` en orden inverso.
- **`dev-env.sh`** — helper script que source-ás para exportar
  `DATABASE_URL` desde el `.env` del compose hacia tu shell del host.
- **`.github/workflows/ci.yml`** — workflow de drift check con
  service container Postgres + `fitz db migrate` + `fitz db check`.
- README dedicado con setup desde cero + validación end-to-end +
  demo del workflow de cambio de schema + cross-link al cap.

### Decisiones técnicas tomadas

- **`fitz db` corre desde el host, no adentro del container**
  (distroless sin shell). Requiere `DATABASE_URL` apuntando a
  `localhost:5432`. El compose ya expone ese puerto del `db`.
- **4 `@table type` declarados SIN `@belongs_to`** todavía. Las
  navigation methods + `@has_many` + `@belongs_to` llegan en C4.
  En C2 las FK constraints van en SQL manual de las migrations
  (`REFERENCES ... ON DELETE CASCADE`).
- **Workflow Docker-first explícito**: después de cada
  `fitz db migrate` hace falta `docker compose up -d --build app`
  para que el binario incorpore el schema declarado. El binario
  no tiene runtime reflection — el schema está hard-coded al
  compile time del `fitz build`.
- **`created_at: DateTime` sin `@db_default("NOW()")`** — los
  INSERTs van a tener que pasar `DateTime.now()` explícito en C4.
  Decisión pragmática para mantener el cap C2 enfocado en el
  workflow de migrations.

### Cross-refs actualizados

- **`mkdocs.yml`**: nav del tab "Construyendo TaskHub" suma entry
  C2.
- **`docs/taskhub/index.md`**: tabla del roadmap actualiza C2 de
  "(próximamente)" a link al cap.
- **`docs/taskhub/c1-setup-docker-first.md`**: sección "Próximo cap"
  ahora linkea al C2 real (en vez del placeholder "(próximamente —
  en desarrollo)").
- **`examples/taskhub/c1-setup/README.md`**: "Qué viene" linkea al
  cap C2 + describe brevemente.

### Validación

- `mkdocs build` non-strict: 17.83s, sin warnings nuevos sobre los
  archivos TaskHub.
- `fitz check examples/taskhub/c2-schema-migraciones/src/main.fitz`
  → "sin errores de tipo". El schema declarado con los 4
  `@table type` + `db.connect()` async top-level + endpoint con
  `User.all(db).await` pasa el checker.
- Smoke real end-to-end NO automatizado — requiere Docker + las
  migrations aplicadas. Documentado paso a paso en el README del
  ejemplo + el cap para reproducir manual.

### Sin cambios

Sin cambios de código del lenguaje, sin cambios al stack —
release v0.15.4 patch 100% docs/material pedagógico.

## [v0.15.3] — 2026-06-07 — Construyendo TaskHub: nuevo tab proyecto integrador + cap C1 (Setup Docker-first)

Iniciativa nueva en docs/. **TaskHub** es un **proyecto integrador
post-curso** — un Trello colaborativo en vivo Dockerizado desde el
día 1 que demuestra el stack único de Fitz funcionando junto
(auth + RBAC custom + ORM + migraciones reales + WebSocket + cron
persistente + interop Python para IA + observability completa con
Prometheus/Jaeger + frontend vanilla JS). Pensado para tres
audiencias: quien terminó el curso M1-M8 y quiere ver todo
integrado, quien ya conoce Fitz y necesita un proyecto serio
end-to-end, y quien evalúa Fitz para producción.

**Decisión estructural confirmada con el autor**: tab top-level
**separado del curso** (no M9 del curso) — el curso es producto
cerrado de 8 módulos / 42 caps; TaskHub es producto distinto con
identidad propia. Mejor marketing en la home (tres rutas: aprender
desde cero / ver proyecto real / referencia exhaustiva).

### Cap C1 — Setup Docker-first (entregado)

- **[`docs/taskhub/index.md`](docs/taskhub/index.md)** (~280 LoC)
  — overview del proyecto, mermaid del stack, comparativa contra
  M6.C7 capstone (qué pieza extra integra TaskHub), descripción
  del modelo de datos (4 tipos: User/Project/Task/Comment),
  funcionalidades end-to-end, roadmap de los 7 caps + post-C7
  (publicación como boilerplate descargable), pre-requisitos,
  cómo seguir el proyecto, comparativa final con stacks típicos.
- **[`docs/taskhub/c1-setup-docker-first.md`](docs/taskhub/c1-setup-docker-first.md)**
  (~880 LoC) — 10 pasos cubriendo: estructura del proyecto,
  `fitz.toml` + `main.fitz` minimal que responde `/healthz`,
  Dockerfile multi-stage con distroless (~30 MB final),
  `docker-compose.yml` con 5 services (app + Postgres +
  Prometheus + Jaeger + nginx) con healthchecks y `depends_on`,
  `nginx.conf` con proxy `/api/*` (HTTP) + `/ws/*` (WebSocket
  upgrade) + serve estático del frontend, `prometheus.yml` con
  scrape config (taskhub target DOWN hasta C7 — esperado),
  Jaeger `all-in-one` con OTel collector built-in (sin config
  file), frontend `index.html` placeholder validable visual,
  `.env.example` + `.gitignore`, primera vuelta + validación
  end-to-end de cada service, validación checklist,
  troubleshooting con 5 casos típicos. Bar editorial del curso
  (header con pre-reqs + objetivo + por qué importa, tabla
  comparativa "Por qué Fitz es distinto", troubleshooting,
  cierre con "Lo que cubriste" + próximo cap).
- **Ejemplo runnable** `examples/taskhub/c1-setup/` con 11
  archivos: `fitz.toml`, `Dockerfile`, `docker-compose.yml`,
  `src/main.fitz`, `nginx/nginx.conf`, `prometheus/prometheus.yml`,
  `frontend/index.html`, `.env.example`, `.gitignore`,
  `migrations/.gitkeep`, `README.md`. Listo para `cp .env.example
  .env` + editar passwords + `docker compose up -d --build` y
  los 5 services arriba en ~30s.

### Roadmap de caps siguientes (declarado en el index)

- **C2** — Schema + workflow `fitz db` end-to-end (declarás
  `@table type` para User/Project/Task/Comment + workflow real
  con cambios de schema posteriores + `rollback` + CI drift
  check).
- **C3** — Auth con RBAC custom (3 roles apilables: admin /
  owner / member) + JWT + Argon2id + tests por rol.
- **C4** — CRUD + relations + WebSocket en vivo por project.
- **C5** — Cron + background jobs con `store=db` persistente.
- **C6** — Interop Python para priorización IA (LLM + fallback
  heurístico).
- **C7** — Observability completa + frontend + deploy
  production.
- **Post-C7** — Extracción del estado final a
  `boilerplates/taskhub/` (al lado de los 9 boilerplates
  existentes) con README dedicado, para que cualquiera pueda
  probar TaskHub sin pasar por los 7 caps. **Decisión del autor**:
  publicar como boilerplate descargable hace que la app sea
  testeable sin el curriculum completo.

### Estructura docs/

- `docs/taskhub/` — nuevo directorio paralelo a `docs/curso/`
  con `index.md` + `c1-setup-docker-first.md`. Los 6 caps
  restantes (C2-C7) se agregan en sesiones siguientes.
- `examples/taskhub/c1-setup/` — primer ejemplo runnable.
  Convención `examples/taskhub/cN-tema/` igual que
  `examples/curso/`.

### Integración con el resto del sitio

- **`mkdocs.yml`**: entry nuevo top-level **"Construyendo
  TaskHub"** entre el curso y la sección "DB y ORM". El tab
  tiene `index.md` + `c1-setup-docker-first.md` (los caps
  futuros se suman a medida que se cierran).
- **`docs/index.md`** (home del sitio): suma botón
  "Construyendo TaskHub" entre el curso y la guía. Párrafo
  descriptivo después del bloque del curso explica el
  posicionamiento (proyecto integrador post-curso, comparativa
  con M6.C7 capstone, audiencias).
- **`docs/curso/m8-produccion-deploy/c5-bundle-python-pip-deploy.md`**
  (último cap del curso): sección "¿Qué sigue?" suma TaskHub
  como primer ítem ("el siguiente paso natural" después del
  curso entero).

### Validación

- `mkdocs build` non-strict: 35.82s, sin warnings nuevos sobre
  los archivos de TaskHub (después de fixear placeholders al C2
  que aún no existe como link).
- `fitz check examples/taskhub/c1-setup/src/main.fitz` → "sin
  errores de tipo".
- `docker compose config -f examples/taskhub/c1-setup/docker-compose.yml`
  (con env vars dummy) → parsea correcto los 5 services con
  todas las env vars resueltas.
- Smoke real `docker compose up -d --build` NO automatizado —
  requiere Docker + ~30s + descarga de imágenes la primera vez.
  Documentado en el README del ejemplo para reproducir manual.

### Sin cambios

Sin cambios de código del lenguaje, sin cambios al stack — esta
entrega es 100% docs/material pedagógico. Release v0.15.3 patch.

## [v0.15.2] — 2026-06-07 — Drift cleanup: API real de `DbRow` en docs/curso M6.C1 + docs/db-orm.md

Follow-up de v0.15.1. Mientras escribíamos el cap nuevo M6.C6 detectamos
que **el snippet de `.fitz` migration en `docs/db-orm.md` § 26.c usaba
`r.get(...)` genérico** (no soportado por el checker). Una barrida más
amplia descubrió que **el cap M6.C1 enseñaba dos APIs inexistentes**:
indexing `r["col"]` y `r.get(col)?` con `Result<Any>`. Ambas son
rechazadas por el checker hace tiempo (error claro: *"el tipo `DbRow`
no tiene el método `get` (soportados: get_int, get_str, get_float,
get_bool, len)"*). La API real, ya consolidada en `docs/guide.md`
y `docs/db-orm.md` §3, es **typed accessors con `?`**:
`r.get_int("col")?` / `r.get_str("col")?` / `r.get_float("col")?` /
`r.get_bool("col")?` / `r.len()`.

### Archivos actualizados

- **[`docs/curso/m6-postgres-orm/c1-setup-driver-crudo.md`](docs/curso/m6-postgres-orm/c1-setup-driver-crudo.md)**
  (8 referencias stale corregidas):
  - Comentario L275: *"Cada row es Map<Str, Any>"* → *"Cada row es
    `DbRow` — accesos tipados con `.get_int/str/...`"*.
  - Sección "Acceso a campos del row" L292-323 reescrita entera: tabla
    de métodos con shape (`get_int`/`get_str`/`get_float`/`get_bool`/`len`),
    razón de diseño explícita ("tipos PG estrictos en runtime, el `?` te
    obliga a manejar NULL/missing"), cuándo NO hace falta (el patrón
    `List<Map<Str, Any>>` para handlers HTTP sigue siendo válido — el
    codegen serialize cada DbRow al JSON).
  - Loop sub-sección L335-339: `row.get_str("name")?` / `row.get_int("age")?`.
  - COUNT example L450 (`/users/count` handler): `r.get_int("n")?`.
  - Validation script L484: `r.get_str("msg")?`.
  - Troubleshooting "Acceso `r['col']` no compila" L547+: ahora documenta
    el error real del checker + por qué la API es tipada (decisión de
    diseño consciente, no limitación).
- **[`docs/db-orm.md`](docs/db-orm.md)** (2 referencias stale corregidas):
  - L2820 (sección "Migraciones manuales versionadas"):
    `applied.map(fn(r) => r["version"])` → `applied.map(fn(r) => r.get_str("version")?)`.
    El `?` propaga adentro del callback de `.map` — patrón limpio.
  - L3305-3320 (sección 26.c — snippet de `.fitz` migration con backfill):
    `r.get("id")` / `r.get("first_name")` / `r.get("last_name")` →
    `r.get_int("id")?` / `r.get_str("first_name")?` / `r.get_str("last_name")?`
    con anotaciones de tipo explícitas (`let id: Int = ...`).

### Verificación de cobertura

- `grep` sobre `r["` + `r.get(` en `docs/` y `examples/` confirma cero
  ocurrencias stale post-fix. Las que quedan son **Map.get** legítimo
  (`headers.get("authorization")?` en M5.C2 / M5.C2 troubleshooting,
  `posts_by_user.get(p.user_id)` en M6.C4 — todos sobre `Map<K,V>`, no
  DbRow) o **JSONB column `.get(key)`** (operador de extracción text que
  el ORM emite como `column->>$1`, NO es DbRow — vive en db-orm.md
  §13 y siguientes).
- `docs/guide.md` ya usaba el patrón correcto (L11233-11234 — typed
  accessors con `?`). Sin cambios.
- `examples/curso/m6-postgres-orm/c6-migrations/` (creado en v0.15.1)
  ya usa el patrón correcto. Sin cambios.
- Snippet sintético con los 5 patrones corregidos pasa `fitz check`
  sin errores de tipo.

### Validación

- `mkdocs build` non-strict: 30.13s, sin warnings nuevos sobre los
  archivos tocados (los pre-existentes de 98 sobre anchors stale son
  independientes).
- `fitz check examples/curso/m6-postgres-orm/c6-migrations/src/main.fitz`
  + el `.fitz` migration → ambos verdes.

### Sin cambios

Sin cambios de código del lenguaje, sin cambios al checker, sin cambios
a la API real — esta entrega solo **alinea los docs con lo que el
binario `fitz` ya hace desde v0.10.22**. Release v0.15.2 patch 100%
docs.

## [v0.15.1] — 2026-06-07 — Curso M6.C6 nuevo: Migraciones con `fitz db`

Release 100% docs/curso sin cambios de código del lenguaje. **Cierra
un gap pedagógico conocido del curso**: M6.C1 y M6.C2 levantaban un
disclaimer explícito *"para migrations reales con diff/apply hay un
subcomando `fitz db diff/migrate` (out of scope del curso)"* aunque
el feature está 100% implementado (10 sub-comandos: `diff`, `migrate`,
`status`, `new`, `rollback`, `check`, `history`, `squash`, `stamp`,
`inspect`) y exhaustivamente documentado en
[`docs/db-orm.md` § 26.c](docs/db-orm.md#26c-migraciones-automaticas-v01016).

### Cap nuevo

- **[M6.C6 — Migraciones con `fitz db`](docs/curso/m6-postgres-orm/c6-migraciones-fitz-db.md)**
  (~900 LoC) — walk-through del workflow completo end-to-end:
  - Setup proyecto + Postgres en compose
  - Workflow canónico: `new` → editar `@table` → `diff > file.sql`
    → `migrate` → `status`
  - Cambio de schema + segunda migration con `ALTER TABLE`
  - `fitz db rollback` con sección `-- DOWN`
  - `fitz db history` (audit log)
  - `fitz db check` con ejemplo de GitHub Actions para CI
  - Migraciones nativas en `.fitz` con `async fn migrate(db)` para
    backfills condicionales
  - `fitz db inspect` + `stamp` para adoptar DBs legacy
  - `fitz db migrate --sql` para handoff a DBA (offline SQL)
  - `fitz db squash <from> <to>` para limpieza histórica
  - Renames seguros con `@renamed_from` decorator transient
- Bar editorial de M6 (header con pre-requisitos + objetivo + por
  qué importa, mermaid del flujo, tabla comparativa con Alembic /
  TypeORM / Flyway / Diesel, 12 pasos numerados, troubleshooting,
  cheat sheet, validación checklist).
- **Ejemplo runnable** `examples/curso/m6-postgres-orm/c6-migrations/`
  con `fitz.toml` + `docker-compose.yml` + `src/main.fitz` con schema
  final + 3 migrations (2 `.sql` con UP/DOWN + 1 `.fitz` con backfill
  condicional usando typed accessors `r.get_int(...)?` /
  `r.get_str(...)?`) + README con setup + smoke completo del
  workflow + caveats.

### Cambios estructurales del módulo M6

- **Capstone renombrado**: `c6-capstone-crud-completo.md` →
  `c7-capstone-crud-completo.md` (con `git mv` para preservar
  history). M6 crece de 6 a 7 caps; curso entero de 41 a 42.
- **Disclaimers levantados** en
  [M6.C1#L228-230](docs/curso/m6-postgres-orm/c1-setup-driver-crudo.md#L228)
  y [M6.C2#L206-210](docs/curso/m6-postgres-orm/c2-table-decoradores-reads.md#L206):
  ya no dicen "out of scope" — apuntan al cap C6 nuevo como
  referencia.
- **Nota explicativa en el capstone (M6.C7)** sobre por qué mantiene
  `CREATE TABLE IF NOT EXISTS` al boot deliberadamente (foco en
  integración del stack web; el workflow versionado vive en C6).
- **Cross-refs actualizados**:
  - [M5.C4#L800](docs/curso/m5-async-auth-rt/c4-jobs.md#L800): lista
    de M6 ahora 7 caps con C6 (migraciones) + C7 (capstone)
    renumerado.
  - [M7.C1#L3](docs/curso/m7-python-interop/c1-setup-imports.md#L3)
    y [M8.C1#L5](docs/curso/m8-produccion-deploy/c1-distribucion-binarios.md#L5):
    pre-requisito apunta a M6.C7 (no C6).
  - "Capstone integra todo el curso" (M6.C7) suma bullet de M6.C6.
  - "Cerraste el módulo M6" (M6.C7) suma bullet ✅ de C6 + renumera
    el "← acá" a C7.
- **Sección "Qué viene M7"** del capstone reescrita: M7 ahora es
  Interop Python y M8 es Producción/deployment (la versión vieja
  apuntaba a M7 = Producción, stale post-v0.12.5).

### Docs estructurales

- `docs/curso/index.md`: tabla "Estado del curso" actualiza M6 de 6
  a 7 caps, total de 41 a 42; sección M6 suma entry C6 (migraciones)
  + renombra C6 → C7 capstone; entregable del módulo menciona el
  workflow versionado.
- `mkdocs.yml`: nav del M6 suma entry C6 (migraciones) + renombra
  C6 → C7.
- `docs/curso-plan.md`: header del estado refleja M6 7 caps + total
  42; nota nueva "Actualización 2026-06-07" detalla la decisión + por
  qué se omitió en el delivery original; tabla "Mapping curso →
  guide.md" suma fila M6.C6 + reagrupa C27-C31 (sin migraciones).

### Notas técnicas detectadas durante el cierre

- **API de `DbRow`**: el ejemplo del backfill `.fitz` originalmente
  usaba `r.get("col")` genérico — `fitz check` lo rechaza con error
  claro citando que la API tipada es `r.get_int(col)?` / `r.get_str(col)?`
  / etc. Fix aplicado al ejemplo y al cap. **Deuda residual menor**:
  [`docs/db-orm.md` § 26.c](docs/db-orm.md) (la referencia
  exhaustiva del ORM) tiene en una sub-sección el snippet usando
  `r.get(...)` — no es esta entrega, queda como deuda para una
  pasada de barrida sobre db-orm.md.

### Validación

- `mkdocs build` non-strict: completa en 18.15s sin warnings nuevos
  sobre el cap C6 nuevo ni sobre el C7 renombrado. Los 98 warnings
  pre-existentes (anchors stale de `guide.md` / `db-orm.md` / otros
  caps de cursos) son independientes.
- `fitz check examples/curso/m6-postgres-orm/c6-migrations/src/main.fitz`
  → "sin errores de tipo".
- `fitz check examples/curso/m6-postgres-orm/c6-migrations/migrations/20260607130000_backfill_full_name.fitz`
  → "sin errores de tipo".
- Smoke real contra Postgres NO automatizado en CI (requiere
  `FITZ_TEST_PG_URL` y workflow de varios pasos). Documentado en el
  README del ejemplo para reproducir manual.

## [v0.15.0] — 2026-06-05 — Bloque 4: V4 expandido (signature help para builtins + method calls) + L2 expandido (inferencia bidireccional para `Fn`)

**Mini-fase combinada** que extiende dos features previas a sus
casos pedagógicos completos. Cierra los gaps visibles del LSP en
método chains (`xs.map(...)`) y de la inferencia para callbacks
asignados a vars o pasados a fns user-defined con `Fn(...) -> ...`.

### LSP — V4 expandido

Antes (v0.14.0): signature help solo cubría fns user-defined del
programa. Builtins y method calls quedaban sin popup.

Ahora (v0.15.0):

- **Builtins globales**: catálogo cerrado de signatures en
  [`src/lsp.rs::BUILTIN_SIGS`](src/lsp.rs) cubre `print`, `len`,
  `sleep`, `env`, `env_or`, `load_env`, `flag`, `spawn`, `config`,
  `secret`, `bytes`. Tipear `len(` abre popup con `fn len(x: Any) -> Int`.
- **Method calls** sobre `List<T>` / `Map<K,V>` / `Str`: catálogo
  paralelo (`LIST_METHOD_SIGS`, `MAP_METHOD_SIGS`, `STR_METHOD_SIGS`)
  con shapes genéricos (`fn map(f: fn(T) -> U) -> List<U>`, etc.).
  Tipear `xs.map(` con `xs = [1, 2, 3]` abre popup con la firma del método.
- **Heurística del receiver**: `infer_builtin_receiver_kind` walka
  el `Program` buscando un `Stmt::Assign` top-level con el nombre
  del receiver, y matchea por shape estructural del value (Expr::List
  → "List", Expr::Map → "Map", Expr::Str → "Str"). MVP simple sin
  pasar por el checker entero.
- **`CallContext` enum** nuevo (`Function` vs `Method`) en
  [`src/lsp.rs`](src/lsp.rs) reemplaza el `(String, u32)` previo.
  El walkback de `find_call_context` detecta el `.` antes del `(`
  para identificar method calls vs function calls.

### Lenguaje — L2 expandido

Antes (v0.14.1): inferencia bidireccional solo en callbacks de
métodos built-in con templates paramétricos (`List<T>.map/filter/...`).

Ahora (v0.15.0):

- **Fn user-defined con param `Fn(...) -> ...`**: si el callee es un
  `Stmt::FnDef` con un param que tipa como `Type::Function { params, .. }`,
  el call site propaga los `params` como hint al arg correspondiente
  si es FnExpr. Caso canónico:

  ```fitz
  fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x) }
  apply(fn(n) => n * 2, 5)   // n se infiere como Int desde el param `f`
  ```

- **`let f: Fn(...) -> ... = fn(...) => ...`**: si la anotación del
  let resuelve a `Type::Function { params, .. }` y el RHS es un FnExpr,
  los `params` se propagan como hint:

  ```fitz
  let f: Fn(Int) -> Int = fn(n) => n * 2   // n se infiere como Int
  ```

- **Implementación**: reusa el stack `ctx.fn_expr_param_hints` introducido
  en v0.14.1. Push al entrar al arg/value FnExpr, pop al entrar al
  handler de `Expr::FnExpr`. Sin cambios al AST.

### Tests

- **+3 LSP E2E** en `tests/lsp_e2e.rs::v4_*`:
  - `len(` → popup con `fn len(x: Any) -> Int`.
  - `xs.map(` con `xs = [1, 2, 3]` → popup con `fn map(f: fn(T) -> U) -> List<U>`.
  - `s.len(` con `s = "hola"` → popup con `fn len() -> Int`.
- **+3 unit tests** en `types::tests::l2x_*`:
  - `let f: Fn(Int) -> Int = fn(n) => n * 2` compila sin errores.
  - `apply(fn(n) => n * 2, 5)` con `apply(f: Fn(Int) -> Int, x: Int)` compila sin errores.
  - Anotación explícita incompatible (`fn(n: Str) =>` sobre `Fn(Int) -> Int`) emite error.
- **Total al cierre**: 3030 unit + 13 LSP E2E + smoke 360 verde.
- Clippy `--lib --tests --bins --features lsp -- -D warnings` limpio.

### Backlog

- **V4 expandido** marcado como CERRADO en
  [`docs/deudas-post-5b.md`](docs/deudas-post-5b.md).
- **L2 expandido** marcado como CERRADO.
- **V6 — Debug Adapter Protocol (DAP)** sumado al backlog como deuda
  futura (~2 semanas). Sin DAP, debugging interactivo en VSCode
  (breakpoints, step in/over/out, watch) no está disponible —
  workarounds: `print`, REPL con `:type`/`:env`, diagnostics LSP.

### Deuda residual derivada

- **V4 — receivers no-Ident**: hoy `infer_builtin_receiver_kind` solo
  cubre `<ident>.method(...)`. Para `<expr>.method(...)` (ej.
  `xs[0].method`, `f().method`) el receiver no se identifica y no hay
  signature help. Refinable con walkback más profundo o uso de
  TypeInfo del LSP.
- **V4 — métodos de tipos custom**: solo cubre List/Map/Str
  built-in. Métodos custom de `type Foo` quedan pendientes.
- **L2 — método custom con param Fn**: la inferencia bidireccional
  no se dispara cuando el callee es un método custom de `type` que
  recibe un callback. Caso raro hasta que tipos custom expongan
  método higher-order canónicamente.

### Versión

- `Cargo.toml`: **v0.14.2 → v0.15.0** (minor — features nuevas
  significativas en LSP + checker; aprovechamos para reconciliar el
  release confuso de v0.14.0/v0.14.2 donde los binarios del workflow
  tenían el código de uno con el tag del otro).
- Extensión VSCode: **0.14.2 → 0.15.0** + `.vsix` regenerado con
  `server/fitz-lsp.exe` v0.15.0.

## [v0.14.2] — 2026-06-05 — Bloque 3: deuda S1 cerrada — hover sobre params, vars de for y bindings de match

**Cierra la deuda histórica S1 del proyecto** — paralelo natural del
V2 que cerró `AssignTarget::Ident`. Sin S1, el LSP no mostraba nada
al hover sobre nombres de params (`fn f(x: Int)` → hover sobre `x`),
vars de loops (`for i in 0..10` → hover sobre `i`), o bindings de
match (`match x { Ok(n) => ... }` → hover sobre `n`). El alumno del
curso lo iba a chocar inmediatamente al entrar a M2 (funciones) y
M3 (loops + match).

### Lenguaje — AST extendido

- `Param` suma `name_span: Span` (paralelo a `AssignTarget::Ident(name, span)` del V2).
- `Pattern::Ident(String)` → `Pattern::Ident(String, Span)`.
- `Pattern::OkBinding(String)` → `Pattern::OkBinding(String, Span)`.
- `Pattern::ErrBinding(String)` → `Pattern::ErrBinding(String, Span)`.

`Span::PartialEq` sigue siendo siempre-true, así que los tests
estructurales del AST no se rompen.

### Implementación

- **Parser**: reusa el helper `expect_ident_with_span` introducido
  en V2 para capturar el span del token Ident de cada param/binding.
- **Checker**: los 4 sitios donde se bindean params/patterns ahora
  registran el tipo en `TypeInfo` bajo el span propio:
  - `infer_expr` para `Expr::FnExpr` (callback inline).
  - Handler de `Stmt::FnDef` (fns top-level).
  - Handler de método custom de `type` con `m.params`.
  - `bind_pattern` (match arms).
  - `bind_for_pattern_in_checker` (var del for).
- **Fallback ZERO**: si el span del nodo es `Span::ZERO` (param /
  pattern sintético en tests), usa el span del nodo contenedor como
  antes — backwards-compat con tests viejos.

### Tests

- **+5 unit tests** en `types::tests::s1_*`:
  - `fn double(n: Int) => n * 2` → hover sobre `n` da `Int`.
  - `fn f(x) => x + 1` → hover sobre `x` sin anotación da `Any`.
  - `for i in 0..10` → hover sobre `i` da `Int`.
  - `match Ok(42) { Ok(n) => n }` → hover sobre `n` da `Int` (inner).
  - Método custom `type T { fn m(amount: Int) }` → hover sobre `amount`
    da `Int`.
- **Total al cierre**: 3027 unit + 10 LSP E2E + smoke 360 verde.
- Clippy `--lib --tests --bins --features lsp -- -D warnings` limpio.

### Side effects del cambio de AST

- Los call sites de patrones literales en tests del parser/evaluator/
  http/cli sumaron ~40 actualizaciones (Pattern::Ident, OkBinding,
  ErrBinding con `Span::default()`) y los constructores de `Param`
  ~22 (campo nuevo `name_span: Span::default()`). Cambios mecánicos
  con sed/perl, sin lógica nueva.

### Deuda residual derivada

Ninguna mayor. La deuda S1 era el complemento de V2 y queda cerrada
por completo. Casos visibles del LSP que siguen pendientes:

- **`Pattern::Tuple` recursivo**: hover sobre `a` en `for (a, b) in xs`
  funciona porque el walker llama `bind_pattern` recursivamente sobre
  cada slot. ✓
- **`Pattern::Or` sin bindings**: por contrato del parser, los
  or-patterns rechazan bindings adentro. No hay nada que registrar.

### Versión

- `Cargo.toml`: **v0.14.1 → v0.14.2** (patch — cambio invasivo del
  AST sin breaking del lenguaje del usuario, solo de tests internos).
- Extensión VSCode: **0.14.1 → 0.14.2** + `.vsix` regenerado.

## [v0.14.1] — 2026-06-05 — Bloque 2: inferencia bidireccional en callbacks de métodos built-in (L2)

**Mini-fase acotada al caso 90%**: el plan original de L2 estimaba
1-2 semanas para inferencia bidireccional GENERAL, pero el caso
pedagógico del curso (`xs.map(fn(x) => x * 10)` → `List<Int>` sin
anotar) se cierra con un cambio puntual en el dispatch de método
built-in. Costo real: ~2-3h, ~140 LoC + 6 unit tests.

### Lenguaje — feature

- **L2 — Inferencia bidireccional de callbacks en métodos built-in**.
  El checker propaga el `T` del receptor a los params SIN anotación
  del callback cuando el método es uno de los built-in con template
  paramétrico conocido sobre `List<T>` (`.map`/`.filter`/`.find`/
  `.any`/`.all`/`.count`/`.find_index`/`.flat_map`).

  **Antes** (v0.14.0):
  ```
  fitz> :type [1, 2, 3].map(fn(x) => x * 10)
  :: List<Any>     ← x quedaba como Any sin anotación
  ```

  **Después** (v0.14.1):
  ```
  fitz> :type [1, 2, 3].map(fn(x) => x * 10)
  :: List<Int>     ← x se infiere como Int del receptor
  ```

  Param CON anotación explícita gana siempre — `fn(x: Float) =>` sobre
  `List<Int>` mantiene el comportamiento previo.

### Implementación

- Nuevo helper `expected_callback_param_for_builtin_method(obj_ty, method) -> Option<Vec<Type>>`
  en [`src/types.rs`](src/types.rs).
- `CheckCtx` gana stack `fn_expr_param_hints: Vec<Option<Vec<Type>>>`
  para soportar nested callbacks sin contaminación.
- El call site del método empuja el hint ANTES de sintetizar cada
  arg si es un FnExpr directo. El handler de `Expr::FnExpr` consume
  el top del stack al entrar.

### Docs

- Cap M1.C5 del curso ([docs/curso/m1-setup/c5-repl.md](docs/curso/m1-setup/c5-repl.md))
  restauró el ejemplo original `fn(x) => x * 10` (sin anotación) y
  removió la nota sobre la limitación (que ya no aplica). Suma una
  explicación corta sobre la inferencia bidireccional y cuándo gana
  la anotación explícita.

### Tests

- **+6 unit tests** en `types::tests::l2_*`:
  - Caso pedagógico (`map` sobre `List<Int>` sin anotar).
  - `filter` con `ret: Bool` validado.
  - Propagación a métodos del param (`s.upper()` requiere `s: Str`).
  - Anotación explícita gana sobre hint.
  - `find` devuelve `Result<T>` con T inferido.
  - Nested callbacks sin contaminación (`xs.map(fn(x) => [x].map(fn(y) => y*2))`).
- **Total al cierre**: 3022 unit + 10 LSP E2E + smoke 360 verde.
- Clippy `--lib --tests --bins --features lsp -- -D warnings` limpio.

### Deuda residual derivada de L2

- **Map<K, V> higher-order**: hoy `infer_map_method` no expone
  callbacks (solo get/has/keys/values/len). Cuando lleguen
  filter/find sobre entries, agregar el caso al helper devolviendo
  `Some(vec![K, V])`.
- **Inferencia bidireccional GENERAL**: el alcance del fix es
  callbacks de métodos built-in conocidos. Casos no cubiertos —
  fn user-defined con param `Function`, FnExpr asignada a var con
  anotación de Function — siguen sintetizando params sin anotación
  como `Any`. Refactor invasivo del checker (1-2 semanas) si entra
  demanda real.

### Versión

- `Cargo.toml`: **v0.14.0 → v0.14.1** (patch — feature acotada,
  estrictamente backward-compat: lo que antes tipaba `List<Any>` ahora
  tipa `List<T>`, y `Any` es compatible con cualquier `T`).
- Extensión VSCode: **0.14.0 → 0.14.1** + `.vsix` regenerado.

## [v0.14.0] — 2026-06-05 — Bloque 1 LSP+lenguaje: format on save + `;` separator + spans StrInterp + hover LHS + signature help

**Mini-fase intensiva descubierta durante el curso M1**: el alumno
(el autor) reportó 5 bugs/features faltantes al recorrer los caps
M1.C1, M1.C5, M2.C1. Cada uno generó una entrada en el backlog
nuevo de
[`docs/deudas-post-5b.md`](docs/deudas-post-5b.md#fixes-pendientes-de-la-extensión-vscode--lsp)
("Fixes pendientes de la extensión VSCode / LSP" + "Fixes pendientes
del lenguaje (descubiertos en el curso)"). Esta release cierra
**4 de los 5** en una sola pasada coordinada de tests + commit.

### LSP — features nuevas

- **V3 — Format on save** (`textDocument/formatting`). Capability
  `document_formatting_provider: true` + handler en
  [`src/bin/fitz-lsp.rs`](src/bin/fitz-lsp.rs) que delega a
  `fitz::fmt::format_source` y emite UN `TextEdit` con el doc
  reformateado. Sobre doc con error de parser retorna `null`
  silencioso. Helper `end_position_utf16` calcula range del final
  del doc. VSCode con `editor.formatOnSave: true` ahora dispara
  `fitz fmt` automático.
- **V4 — Signature help** (`textDocument/signatureHelp`).
  Capability con trigger chars `(` y `,`. Tipear `f(` o `f(a, `
  muestra popup con label `fn f(p1: T1, p2: T2) -> R` y resalta el
  param actual. MVP cubre fns user-defined del programa; builtins
  y method calls quedan como deuda menor. Helpers nuevos en
  [`src/lsp.rs`](src/lsp.rs) — `find_call_context` (walkback heurístico
  contando `(`/`)` y `,`) y `signature_help_for_call`.

### LSP — bugs cerrados

- **V1 — Spans incorrectos dentro de string interpolation**. Hover
  sobre identificador en `print("{x}")` mostraba `Str` (el tipo del
  StrInterp entero) en vez del tipo del identificador. Errores de
  checker dentro de `{...}` se reportaban en línea 1 col 1.
  Causa: el sub-parser de StrInterp ajustaba columnas de errores via
  `sub_col_base` pero NO los `Span` de los `Expr` exitosos.
  Fix: walker recursivo `shift_expr_spans` en
  [`src/parser.rs`](src/parser.rs) + nuevo helper
  `Expr::span_mut()` en [`src/ast.rs`](src/ast.rs) paralelo al
  `span()` existente.
- **V2 — Hover sobre el nombre de variable en `let X = ...` no
  mostraba nada**. Solo aparecía el tipo si el mouse caía sobre el
  RHS. Causa: `AssignTarget::Ident(String)` no tenía `Span` propio
  — el checker no podía registrar el tipo del LHS en `TypeInfo`.
  Fix: `AssignTarget::Ident` ahora lleva `Span` propio del token
  Ident ([`src/ast.rs`](src/ast.rs)). Parser captura via nuevo
  helper `expect_ident_with_span`. Checker registra el tipo del
  binding bajo el span del LHS — para anotaciones explícitas usa
  el tipo declarado. Cierra parcialmente la deuda S1
  (`Param`/`For.var`/`MatchArm.pattern` siguen pendientes —
  sub-pasos futuros independientes).
- **V5 — Autocomplete tras `from X import` — audit**. Marcada
  como pendiente en el backlog por error; el audit pre-implementación
  confirmó que ya estaba implementada desde v0.9.47
  (`CompletionContext::FromImportList` + `from_import_completions`
  + handler `completion` con `doc_uri`). Sin código nuevo —
  solo cleanup del backlog.

### Lenguaje — features nuevas

- **L1 — `;` como separador opcional de stmts**. Cierra el drift
  histórico con la decisión de diseño #5 del proyecto ("punto y
  coma opcional, como en Go"): el lenguaje pre-v0.14.0 rechazaba
  `;` como `Carácter inesperado`. Fix mínimo: lexer emite
  `Token::Newline` cuando ve `;` ([`src/lexer.rs`](src/lexer.rs))
  — cero cambios al parser/AST. Strings y comments preservan `;`
  literal sin interpretarlo. El formatter `fitz fmt` reescribe
  `1 + 1; 2 + 2` como dos líneas separadas (convención canónica).
  Validación end-to-end:
  ```
  fitz> 1 + 1; 2 + 2
  = 4
  ```
  Exactamente el comportamiento que el cap M1.C5 del curso prometía.

### Docs

- Cap M1.C5 del curso restauró la sección "Múltiples expresiones
  por línea" (que se había sacado como fix temporal mientras L1
  estaba abierto).
- Cap M2.C1 del curso corregido en sesión anterior: la fila
  `\'` de la tabla de escapes y el demo `print('\'comillas\'')`
  fueron removidos, y se agregó un call-out *"Fitz usa **solo**
  `"..."` como delimitador. El `'` se reserva para labels de
  `break`/`continue`"* (deuda **L4** marcada como **by design**).
- Guía: la frase ambigua *"No hace falta `;`"* en el cap de tipos
  custom se actualizó a *"el `;` es separator opcional entre
  stmts — newline lo cubre en casi todos los casos"*, alineado
  con la decisión #5 ahora que L1 está cerrado.
- Cap M1.C5 corregido en sesión anterior: dos ejemplos de
  `:type [1, 2, 3].map(fn(x) => ...)` ahora usan `fn(x: Int) =>`
  con nota corta explicando la limitación del checker (deuda
  **L2** documentada). El Paso 8 (`:load`) ahora usa
  `:load src/helpers.fitz` consistentemente (portable cross-OS;
  deuda **L3** marcada como **by design**).
- [`docs/deudas-post-5b.md`](docs/deudas-post-5b.md) sumó dos
  secciones nuevas: *"Fixes pendientes de la extensión VSCode /
  LSP"* (V1-V5) y *"Fixes pendientes del lenguaje (descubiertos
  en el curso)"* (L1-L4). Backlog vivo para próximos hallazgos.

### Tests

- **+9 unit tests nuevos**: 4 en `parser::tests::v1_*` (spans
  StrInterp), 4 en `types::tests::v2_*` (hover LHS de `let`), 5 en
  `lexer::tests::l1_*` (`;` semantics).
- **+4 E2E LSP nuevos**: 2 V3 (format on save happy path + doc
  roto silencioso), 2 V4 (signature help con active_param=0 y =1).
- **Total al cierre**: 3134 unit + 8 LSP E2E (+13 vs v0.13.2).

### Deuda residual derivada (NO bloquea Bloque 2)

- **L2 — Inferencia bidireccional de tipos en callbacks**
  (`xs.map(fn(x) => x * 10)` tipa como `List<Any>` en vez de
  `List<Int>`). Workaround: anotar `fn(x: Int) =>`. Costo del fix:
  1-2 semanas, cambio invasivo del checker (synthesis vs
  checking modes). Documentada en backlog.
- **V1 walker no recursa en Stmts** adentro de `FnExpr.body` /
  `Loop.body` / `If.then`. Caso raro en interpolaciones.
- **V2 cierra solo `AssignTarget::Ident`**; `Param` / `For.var`
  / `MatchArm.pattern` siguen sin span propio (deuda S1, paralelo).
- **V4 cubre solo fns user-defined**. Builtins (`print`, `len`,
  módulos `jwt`/`hash`/etc.) y method calls (`xs.map(`) quedan
  pendientes — la mayoría de los builtins tipan como `Type::Any`
  gradual, las signatures concretas viven en `infer_*_method`
  por tipo del receptor.

### Versión

- `Cargo.toml`: **v0.13.2 → v0.14.0** (minor bump por features
  nuevas significativas + cambio invasivo de `AssignTarget::Ident`).
- Extensión VSCode: **0.13.2 → 0.14.0** + `.vsix` regenerado para
  exponer las nuevas capabilities (`signatureHelpProvider`,
  `documentFormattingProvider`).

## [v0.13.2] — 2026-06-04 — Bugfix LSP: positionEncoding UTF-16 (extensión 0.13.1 inservible)

**Bug crítico** descubierto durante el curso M1.C1: la extensión
VSCode 0.13.1 no podía conectarse al language server en VSCode
fresh. Síntoma reportado por el usuario:

```
Error: Unsupported position encoding (utf-8) received from server
Fitz Language Server
[Error] The Fitz Language Server crashed 5 times in 3 minutes.
```

**Causa**: v0.9.51 declaró `position_encoding: utf-8` en
`capabilities` del initialize response del server. El cliente
`vscode-languageclient@9.0.1` hard-codea
`generalCapabilities.positionEncodings = ['utf-16']` en
`client.js:1370` y rechaza cualquier encoding del server distinto
de `utf-16` o `undefined` en `client.js:835`. El handshake fallaba
ANTES de poder hablar JSON-RPC. El binario `fitz.exe` no estaba
afectado — `fitz run`/`build`/`check` funcionaban normal; solo
rompía la extensión.

**Fix completo** (Opción B — no quedó deuda):

- **Server**: omite `position_encoding` en `capabilities`
  (`src/bin/fitz-lsp.rs:124`). Cliente asume UTF-16 default.
- **`position_to_offset` / `offset_to_position`** (`src/lsp.rs`):
  migrados de contar chars Unicode a contar **UTF-16 code units**
  vía `ch.len_utf16()`. Tolerancia defensiva con `>=` para
  cliente mal comportado que mande position en medio de surrogate
  pair (VSCode no genera ese caso pero queremos defensa).
- **Helper nuevo `utf16_to_unicode_char(text, line, char_utf16) -> u32`**
  (pub, exportado): traduce el `character` del cliente (UTF-16)
  a chars Unicode 1-based del lexer. Necesario porque
  `TypeInfo`/`DefinitionInfo` siguen indexados por chars Unicode
  (`lexer.rs::advance` cuenta así).
- **Handlers del backend** (`src/bin/fitz-lsp.rs`): `hover` y
  `goto_definition` traducen `pos.character` con el helper antes
  de llamar a `hover_for_position` / `definition_for_position` /
  `ident_under_cursor`.
- **`detect_completion_context`**: traduce `recv_col` interno
  después de `offset_to_position` antes de armar
  `CompletionContext::AfterDot`, así el lookup en TypeInfo (que
  indexa por chars Unicode) funciona cuando hay SMP en la línea
  antes del receiver.

**Tests nuevos** (`src/lsp.rs::tests`):

- `position_to_offset_cuenta_utf16_code_units_no_chars_unicode`
  (reemplaza al test legacy `position_to_offset_cuenta_chars_unicode_no_utf16_code_units`).
- `position_to_offset_tolera_mid_surrogate`.
- `offset_to_position_emoji_retorna_utf16_units`.
- `utf16_to_unicode_char_identidad_para_ascii`.
- `utf16_to_unicode_char_colapsa_smp`.
- `utf16_to_unicode_char_multilinea`.
- `detect_context_after_dot_traduce_recv_col_con_smp_antes`.

Soporta chars del Supplementary Multilingual Plane (emoji,
símbolos matemáticos avanzados) sin off-by-one en hover/
definition/completion. **Deuda "UTF-16 position strict" CERRADA
completa** (no se re-abre).

**Deuda residual cosmética** (NO afecta navegación funcional):

- `make_definition_location` y `ident_range_from_def` retornan
  `Range` LSP con char en chars Unicode (no UTF-16). En la
  práctica char_unicode == char_utf16 en líneas de def porque
  keywords + identifiers son ASCII por reglas del lexer. Solo
  difiere si hay SMP en la parte ANTES del identifier (raro:
  comment con emoji + un `let` en la misma línea, lo cual no es
  sintaxis válida porque `//` desencadena comentario hasta fin de
  línea).

**Docs**:

- Cap C1 del curso M1 (`docs/curso/m1-setup/c1-instalacion.md`)
  suma DOS entradas de troubleshooting:
  - `vcruntime140.dll no se encuentra` (VC++ Redistributable) —
    afecta tanto `fitz.exe` como `fitz-lsp.exe` en Windows
    fresh.
  - `Unsupported position encoding (utf-8)` — bug específico
    de 0.13.1, fix en 0.13.2, instruye actualizar.

**Tests al cierre v0.13.2**: 3121 lib tests (+8 vs v0.13.1: 7
nuevos del UTF-16 + 1 invertido) + 112 LSP + 360 compile_e2e + 3
openapi. `cargo fmt --all --check` + `cargo clippy --lib --tests
--bins --features lsp -- -D warnings` limpios.

## [v0.13.1] — 2026-06-04 — Smoke gating de deps emitidas (deuda boring cerrada)

**Cierre de la deuda residual "Smoke compile_e2e — gating de deps
emitidas"** (abierta en v0.12.1 cuando Tier3 sumó
`metrics-exporter-prometheus` al codegen). El smoke
`GUIDE_EXAMPLES_COMPILE` compila ~360 ejemplos en serie sin cache
compartido; cada uno pagaba cold-compile de `metrics-exporter-prometheus`
aunque no usara Prometheus. CI Linux ya tuvo que bumpear timeout
15→25 min en commit `94e97c5`. Esta release refina el gating del
Cargo.toml emitido por `cargo_toml_for`:

- **`metrics-exporter-prometheus` solo se emite cuando hay
  `@server(prometheus=true)` literal en código.** Detector nuevo
  `program_uses_prometheus_export(program)` paralelo a
  `program_uses_trace_metric`. Propagado a `CodegenCtx` +
  `cargo_toml_for` (param nuevo `uses_prometheus_export`).
- **`emit_prometheus_prelude` gateado por el mismo flag**
  (paralelo bit-a-bit). El static `__FITZ_PROMETHEUS_HANDLE`, el
  helper `__fitz_init_prometheus(...)`, y el helper
  `__fitz_prometheus_route()` ya no se emiten en programas sin
  opt-in. La call `__fitz_init_prometheus(...)` en `gen_main_with_http`
  + el `.merge(__fitz_prometheus_route())` en el Router también
  gateados.
- **Breaking behavior**: el path env var `FITZ_PROMETHEUS=1` ya no
  funciona como override de runtime — production deployments
  declaran Prometheus en código con `@server(prometheus=true)`.
  Trade-off aceptado: el opt-in compile-time cubre el 95% del caso
  real (CI/CD pipelines saben en compile time si Prometheus va o
  no); el env var override era nice-to-have que no justifica
  ~5 min CI extra por release.

**Decisiones de scope confirmadas al arrancar**:

- Solo se gatea Prometheus. Las deps OTel (`opentelemetry`,
  `opentelemetry_sdk`, `opentelemetry-otlp`) siguen emitidas
  con `has_http` porque `uses_logging` se fuerza a `true` cuando
  `has_http=true` (línea 213-215 de `src/codegen.rs`) — el
  wrapper HTTP emite `__fitz_log_info("http.access", ...)` +
  `__fitz_with_span_context(...)` + branches sobre
  `__fitz_otel_is_enabled()` sin opt-in del user. Removerlas
  exige también gatear el access log automático del wrapper, lo
  cual cambia comportamiento user-visible (programas HTTP
  simples pierden auto access logs). Queda como deuda residual
  separada en `docs/deudas-post-5b.md`.
- El crate `metrics` (liviano, no pulla deps grandes) queda
  emitido cuando `has_http || uses_trace_metric` (sin cambio).
- Sin cross-module detection del `@server(prometheus=true)` — MVP
  solo detecta top-level del main. Workaround: declarar `@server`
  en el archivo principal (caso típico ya).

**Cambios concretos en el codegen**:

- Nueva función pure-function `program_uses_prometheus_export`
  walka decorators top-level buscando `name=="server"` con
  `kwargs["prometheus"] == Expr::Bool(true, _)` literal.
- `cargo_toml_for` recibe nuevo param `uses_prometheus_export:
  bool` (positional, último). 20 call sites actualizados (1
  producción + 19 tests).
- `CodegenCtx` gana field `uses_prometheus_export: bool` (default
  `false`), seteado en `generate_main_rs` desde el detector +
  `generate_project` propagado a `cargo_toml_for`.
- `emit_prometheus_prelude` early-return si
  `!has_http || !uses_prometheus_export`.
- `gen_main_with_http` skipea la línea `__fitz_init_prometheus(...)`
  cuando `!uses_prometheus_export`.
- `gen_router_with_routes` skipea `.merge(__fitz_prometheus_route())`
  cuando `!uses_prometheus_export`.

**Tests al cierre v0.13.1**:

- 3 tests nuevos en `codegen::tests`:
  - `tier3_codegen_http_sin_prometheus_no_emite_prelude_ni_dep`
    (reescritura del antiguo
    `tier3_codegen_http_emite_prometheus_prelude_y_init_call_falso_por_default`):
    sin `@server(prometheus=true)` no se emite ni el preludio ni
    la dep `metrics-exporter-prometheus`.
  - `tier3_codegen_http_con_prometheus_true_emite_prelude_y_dep`
    (refinado del antiguo
    `tier3_codegen_http_con_prometheus_true_emite_init_call_true`):
    con `@server(prometheus=true)` se emite todo (handle + init +
    route + dep).
  - `v0_13_1_program_uses_prometheus_export_detecta_kwarg_true` +
    `..._no_dispara_sin_kwarg`: cubren el detector pure-function
    sobre 4 casos (con kwarg true, sin kwarg, con `false`
    explícito, con otros kwargs como `observability=true`).
- Total al cierre: **3003 unit (+2 vs v0.13.0; 2 tests refactor de los Tier3 viejos + 2 detectores nuevos) + 112 LSP + 360
  compile_e2e + 3 openapi** verde. `cargo fmt --all --check` +
  `cargo clippy --lib --tests --bins -- -D warnings` limpios.

**Documentación actualizada**:

- `docs/guide.md` cap 33.4 documenta el breaking behavior del env
  var `FITZ_PROMETHEUS=1` con nota explícita.
- `docs/deudas-post-5b.md` marca la deuda "Smoke compile_e2e —
  gating de deps emitidas" como CERRADO 2026-06-04 con timing
  before/after.
- `docs/roadmap.md` nota breve en la entrada v0.13.x.
- README + extensión VSCode no afectados (deuda interna del CI,
  no user-visible más allá del breaking del env var override).

**Próximo norte**: el grueso de Fase 12 entera cerrado (Tier 1 +
Tier 2). Próximas direcciones según demanda real:

- Cerrar la deuda residual OTel deps gating (requiere también
  gatear access log auto del wrapper).
- Fase 13+ (orquestación distribuida, multi-tenant) si aparece
  demanda concreta.

## [v0.13.0] — 2026-06-04 — Fase 12 Tier 2: `fitz deploy` (12.6) + `@trace`/`@metric` (12.7) + `@flag` / `flag()` (12.8)

**Release coordinado de las 3 sub-fases del Tier 2 de Fase 12**:

- **Fase 12.6 — `fitz deploy <target>` orchestrator**. Sub-comando
  nuevo en CLI, thin wrappers sobre `docker build`/`compose up`.
  Soporta `docker` (build + push opt-out con `--no-push`) y
  `compose` (up local; `--no-detach`/`--no-build` opt-outs).
  Requiere `Dockerfile` (o `docker-compose.yml`) en el manifest
  dir — sugiere `fitz docker init` si falta. Propaga exit codes
  para CI. Módulo nuevo `src/deploy.rs` (~430 LoC + 7 unit + 5
  cli_e2e tests).

- **Fase 12.7 — `@trace(name="X")` y `@metric(name="X")` sobre
  fns user**. Decorators apilables sobre funciones business
  logic (rechazados sobre HTTP/WS — la auto-instrumentation
  Fase 12.3 cubre esos casos). `@trace` abre un
  `tracing::info_span!` que envuelve cada call; `@metric`
  registra `<name>_duration_seconds` (histogram) +
  `<name>_calls_total` (counter) al Drop del scope vía
  `__FitzMetricGuard` RAII (funciona con `return X` explícito sin
  código muerto). Kwarg `name=` opcional sobre cada uno
  (fallback al nombre de la fn). Paridad bit-a-bit `fitz run`
  (no-op honesto — el evaluator ignora los decorators) ↔
  `fitz build` (instrumentación real con `tracing` + `metrics`
  crates linkeados). Cap 33.5 nuevo en la guía + ejemplo
  runnable `examples/guide/34-trace-metric.fitz`.

- **Fase 12.8 — `@flag("name")` + `flag(name) -> Bool` +
  módulo `flags`**. Feature flags built-in con dos fuentes:
  sección `[flags]` en `fitz.toml` (defaults compile-time) +
  env vars `FITZ_FLAG_<UPPERCASE>` (override runtime). Default
  `false` (fail-safe). El decorator `@flag` sobre HTTP/WS
  handlers retorna 404 si la flag está off — gate hot path
  ANTES de middlewares/auth. `flag(name)` y `flags.is_enabled`
  para branches dentro del código; `flags.list()` enumera
  flags conocidos (manifest + env vars). Paridad bit-a-bit
  `fitz run` ↔ `fitz build` (registry estático con `OnceLock`
  en codegen + cache lookup). Cap 33.11 nuevo en la guía +
  ejemplo runnable `examples/guide/34b-feature-flags.fitz`.

**Decisiones técnicas del Tier 2**:
- Deploy targets MVP: solo docker + compose (no fly/railway —
  diferidos a Fase 13+ por demanda real).
- `@trace`/`@metric` exclusivos para fns user; HTTP/WS handlers
  ya tienen auto-instrumentation Fase 12.3 (rechazo estático
  con mensaje claro citando el sub-paso anterior).
- `@flag` como gate de fn entera. Default `false` opt-in (todos
  los flags requieren opt-in explícito). Defaults compile-time
  baked-in al binario via `__fitz_flag_init(...)` al boot.

**Builtins nuevos disponibles globalmente**: `flag(name)`,
`flags.is_enabled(name)`, `flags.list()`. Pre-registrados en el
scope del checker como `Type::Any` (mismo patrón que `jwt`/
`hash`/`auth`).

**Extensión VSCode v0.13.0**: LSP completions sumadas
(`@trace`/`@metric`/`@flag` decorators, `flag()` builtin,
`flags.X` after-dot). Grammar TextMate sin cambios (decorators
matchean `@<ident>` genérico).

**Tests al cierre**: 3001 unit (+44 nuevos: 8 evaluator flag +
9 checker flag + 4 trace/metric codegen + 3 manifest flags + 4
codegen Cargo.toml flag/trace_metric + 14 LSP + 2 E2E compile
flag) + 112 LSP + 360 compile_e2e (+2 nuevos: 34b-feature-flags
+ 34-trace-metric + smoke verde) + 3 openapi. Total acumulado
+ unit. `cargo fmt --all --check` + `cargo clippy --lib --tests
--bins -- -D warnings` limpios.

**Curso `Fitz de 0 a experto` cerrado entero (8 módulos / 41 caps)**
(de v0.12.7): M7 dedicado a Interop Python (3 caps) + M8
(Producción y deployment) con M8.C5 sobre deploy real de apps
con interop. **Fase 12 ENTERA + 9.w.1.iter2 ENTERA** CERRADAS.

**Próximo norte**: Fase 13+ (orquestación distribuida, multi-tenant,
o demanda real concreta). Tier 2 de Fase 12.3 (bridge métricas
OTel) sigue bloqueado por release del crate.

## [v0.12.7] — 2026-06-03 — Curso M7 nuevo (Interop Python) + M8 ampliado (M7→M8 renumber + C5 nuevo)

Release 100% docs/curso, sin cambios de código. Cierra el curso `Fitz
de 0 a experto` entero (8 módulos / 41 capítulos). Plan original tenía
7 módulos (M7 = Producción y deployment) pero al cierre detectamos un
gap: la Interop Python (Fase 8) era módulo de feature densa
documentado en cap 21 de la guía (~800 LoC, 15 sub-secciones) pero
**sin presencia pedagógica** en el curso. El plan original lo trataba
como UN cap opcional (C32b) dentro de M6, lo cual subutilizaba el
material disponible (9 ejemplos runnable + bundling completo).

**Decisión confirmada con el autor** (2026-06-03): renumerar el M7
anterior (Producción y deployment) a M8, y crear un M7 nuevo dedicado
a Interop Python con 3 caps. M8 además recibió un cap nuevo (M8.C5)
sobre deploy real de apps con interop Python — específicamente para
las apps que salgan de M7 que necesiten distribución sin Python
instalado en destino.

**M7 nuevo — Interop Python** (`docs/curso/m7-python-interop/`):

- **M7.C1 — Setup venv + `from python import` + casos simples**
  (`c1-setup-imports.md`). Setup del venv estándar Python (sin magia
  Fitz, lee `VIRTUAL_ENV` al boot vía CPython), compilación de `fitz`
  con `--features python`, primer programa con `math`/`json`/
  `datetime`. Auto-coerción primitiva (Fase 8.1.3), introducción al
  concepto de PyObject opaco para tipos complejos. Tabla diferencial
  vs subprocess/IPC, Node `child_process`, Rust+PyO3 manual, Julia
  PyCall, Java JNI. Ejemplo runnable handler HTTP combinando los 3
  módulos stdlib.
- **M7.C2 — numpy + pandas reales: data analysis**
  (`c2-numpy-pandas-data-analysis.md`). El sweet spot real de la
  interop: leer CSV con pandas, calcular con numpy, devolver
  `list[dict]` que Fitz marshalea automático a
  `List<Map<Str, Any>>`. Coerción a `type` nominal Fitz con
  anotación destino (Fase 8.4). Excepciones Python →
  `Result::Err` automático (Fase 8.3). Benchmarks de marshaling
  (~12ms para 1000 filas + agg). Tabla diferencial vs FastAPI
  dedicado, subprocess, FastAPI sidecar, Rust+PyO3 manual.
- **M7.C3 — SQLAlchemy interop + bridge async + cuándo NO usarlo**
  (`c3-sqlalchemy-async-vs-orm-nativo.md`). Cierre del módulo:
  **matriz de decisión honesta** entre ORM nativo Fitz y SQLAlchemy
  interop. Cubre el patrón canónico `<py_call>?.await` para
  SQLAlchemy 2.x async (Fase 8.6 bridge tokio↔asyncio). `fitz
  py-types` para auto-generar `type` Fitz desde modelos SQLAlchemy.
  9 criterios de decisión (greenfield vs legacy, performance, sin
  Python en runtime, validación estática, migrations, tooling,
  equipo, multi-DB, triggers).

**M8 — Producción y deployment** (renombrado de M7, `docs/curso/
m8-produccion-deploy/`):

- **M8.C1-C4** intactos (renumerados): distribución avanzada,
  observability OTel, secrets management, deploy con Docker +
  healthz + K8s + 12-factor.
- **M8.C5 NUEVO — Deploy real de apps con interop Python**
  (`c5-bundle-python-pip-deploy.md`). Cubre `fitz build
  --bundle-python` (CPython 3.14.5 embebido vía python-build-
  standalone de Astral) y `--bundle-pip` (paquetes pip embebidos
  via venv temporal + tarball secundario). Comparativa Path A
  (Dockerfile default + venv en runtime, ~250 MB) vs Path B
  (bundling completo + distroless runtime, ~200 MB).
  Trade-offs honestos (cuándo NO usar bundling: CI rápido, C
  extensions con deps de sistema, layers cacheables, paquetes
  >100 MB). Cuándo USAR: distribución a usuarios finales, edge
  functions, reproducibilidad estricta. Smoke real validado con
  pandas y SQLAlchemy bundleados.

**Decisiones técnicas del nuevo M7/M8**:

- **Bar editorial idéntico a M1-M6**: header con pre-requisitos +
  objetivo + por qué importa + cross-link a guide.md cap 21,
  mermaid map, tabla "Por qué Fitz es distinto" comparativa,
  pasos numerados con código + outputs reales, subset compilable
  a binario, validación checklist, troubleshooting, lo que sigue.
- **Pre-req de M7**: M6 cerrado (necesario para entender la
  matriz vs ORM nativo en M7.C3) + Python 3.10+ + `cargo build
  --features python`.
- **Cierre del curso** en M8.C5 (era M8.C4 antes). Si la app NO
  usa interop Python, M8.C5 es opcional — M8.C4 deja link al
  cierre.

**Estructura final del curso** (8 módulos / 41 capítulos):

| Módulo | Caps | Total |
|---|---|---|
| M1 — Setup y primer programa | C1-C6 | 6 |
| M2 — Tipos y funciones | C1-C7 | 7 |
| M3 — Módulos y organización | C1-C5 | 5 |
| M4 — HTTP first-class | C1-C5 | 5 |
| M5 — Async, auth, real-time | C1-C4 | 4 |
| M6 — Capstone Postgres + ORM nativo | C1-C6 | 6 |
| **M7 — Interop Python (NUEVO)** | C1-C3 | 3 |
| **M8 — Producción y deployment (ex-M7 + C5 nuevo)** | C1-C5 | 5 |
| **Total** | | **41** |

**Sub-pasos del release**:

- **Renumeración M7 → M8**: `git mv docs/curso/m7-produccion-
  deploy/ → docs/curso/m8-produccion-deploy/`. Sed sobre los 4
  caps existentes: "M7.C" → "M8.C", header del primer cap
  pre-req actualizado, "Validación final del módulo M7" → "M8",
  final summary del curso updateado. Renumeración interna 8 refs
  en headers + cross-links.
- **3 caps nuevos del M7 Interop Python** escritos siguiendo el
  bar editorial M6: ~1100 LoC de markdown por cap.
- **1 cap nuevo M8.C5**: ~800 LoC + el cierre del curso entero
  movido a este cap.
- **Ejemplos runnable** en `examples/curso/m7-python-interop/`:
  C1 (`c1-setup/app.fitz` + README) — handler HTTP con math/
  json/datetime; C2 (`c2-weather/` con app.fitz + weather.py +
  generate_data.py + README) — análisis de clima con pandas
  +numpy; C3 (`c3-sqlalchemy/` con app.fitz + models.py +
  db_helpers.py + README) — SQLAlchemy 2.x async + bridge.
- **`docs/curso/index.md`**: tabla de estado con 8 módulos / 41
  caps. Sección nueva "M7 — Interop Python" con links a los 3
  caps. Sección "M8 — Producción y deployment" actualizada con
  cap M8.C5.
- **`mkdocs.yml`**: nav nueva con M7 (3 caps) + M8 (5 caps).
- **`docs/curso-plan.md`**: header de "Actualización 2026-06-03"
  con 5 ajustes sobre el plan original, mapping curso → guide.md
  refrescado.

**Tests al cierre v0.12.7**: 2957 unit + 93 cli_e2e + 3 openapi_e2e
+ 358 compile_e2e + 6 E2E real Postgres (sin cambios — release 100%
docs). Clippy + fmt heredados de v0.12.6 limpios.

**Verificación pre-bump completa** (memoria
`feedback_pre_release_verification`): roadmap actualizado, curso-plan
revisado con la nota de actualización, deudas-post-5b nota de cierre
del curso entero, CLAUDE entrada nueva, CHANGELOG (esta entrada),
docs/curso/index.md ✓, mkdocs.yml ✓, extensión VSCode sin cambios
(release 100% docs), examples sumados a `examples/curso/m7-python-
interop/`, README sin cambios (link a curso index ya está).

**Cierre formal del curso `Fitz de 0 a experto` entero**: 8 módulos
/ 41 capítulos cubren desde "`print('hola')`" hasta apps production-
ready con interop Python distribuidas como binarios bundleados. Plan
original cumplido + ampliado.

**Próximos nortes**: Fase 13+ (visión post-Fase 12 — `fitz deploy`
orchestrator, feature flags, etc.) según demanda real, smoke
automatizado del curso M7 con `cargo build --features python` en CI
(deuda residual menor), o release dedicado a feedback de los users
reales que terminen el curso.

## [v0.12.6] — 2026-06-03 — Fase 9.w.1.iter2.b: Token blacklist (auth nativa cerrada)

Módulo built-in nuevo `auth` con 3 builtins async sobre Postgres para
revocación de tokens. **Cierra Fase 9.w.1.iter2 entera** (.a RBAC custom
+ .b blacklist).

**API**:

```fitz
auth.blacklist(db, jti, expires_at) -> Future<Result<Null>>
auth.is_blacklisted(db, jti)         -> Future<Result<Bool>>
auth.cleanup_expired(db)             -> Future<Result<Int>>  // rows borradas
```

Tabla `fitz_token_blacklist(jti TEXT PRIMARY KEY, expires_at BIGINT
NOT NULL)` auto-creada con `CREATE TABLE IF NOT EXISTS` al primer call
(paralelo a Fase 9.w.3.iter2 cron persistente).

**Decisiones técnicas**:

- **`expires_at` como Unix epoch (BIGINT)** — el JWT `exp` claim ya
  viene como timestamp Unix, evita conversiones.
- **Auto-filtro de tokens vencidos** en `is_blacklisted`: SQL usa
  `expires_at > extract(epoch from now())`. Tokens con `exp` pasado
  cuentan como NO blacklisted — el `jwt.decode` los rechaza primero
  por expirado, no necesitan seguir bloqueando.
- **`ON CONFLICT DO UPDATE`** en blacklist: re-blacklistear el mismo
  jti actualiza `expires_at` sin fallar (caso raro de token re-emitido).
- **Server-clock manda** (`now()` en SQL, no en Rust): evita drift
  entre el binario y la DB.
- **Auto-creación de tabla idempotente** — Postgres serializa CREATE
  TABLE IF NOT EXISTS con LOCK interno.
- **Paridad bit-a-bit `fitz run` ↔ `fitz build`** — el codegen emite
  helpers `__fitz_auth_*` con el mismo SQL que el intérprete.

**Patrón canónico documentado en cap 28**:

- **`/auth/logout`** se escribe a mano (~10 LoC): `jwt.decode` + extraer
  jti/exp + `auth.blacklist(db, jti, exp).await?`.
- **`/auth/refresh`** se escribe a mano (~15 LoC): revocar token actual
  + emitir uno nuevo con `jti` fresco.
- **`@auth_provider`** chequea `auth.is_blacklisted(db, jti).await?`
  antes de devolver el user.
- **`@cron` periódico** llama `auth.cleanup_expired(db)` (típico
  `@cron("0 0 3 * * *")` daily 3 AM).

Auto-mount de logout/refresh queda **fuera del MVP** — el flow exacto
varía por proyecto, mantenerlo manual da más control y honestidad.

**Sub-pasos**:

- **9.w.1.iter2.b.1 — Builtins intérprete**: 4 helpers `pub` en
  `src/evaluator.rs` (`SQL_*` constantes + `ensure_token_blacklist_table`),
  3 fns `builtin_auth_blacklist/is_blacklisted/cleanup_expired` con
  validación de args + signatures async devolviendo `Future<Result<...>>`,
  registro del módulo `auth` paralelo a `jwt`/`hash`/`log` en
  `register_builtins`. Checker: `auth` registrado como `Type::Any` en
  el scope base (paralelo a jwt/hash). **6 unit tests** sobre validación
  de args (aridad incorrecta, primer arg debe ser DbConn, etc.) y
  pre-registro del módulo. **6 E2E reales contra Postgres** en
  `tests/auth_blacklist_real_postgres.rs` con `#[ignore]`: blacklist +
  is_blacklisted devuelve true, jti inexistente devuelve false,
  expires_at pasado cuenta como no-blacklisted (auto-filtro), cleanup
  borra solo vencidas, re-blacklist actualiza expires_at (ON CONFLICT),
  ensure_table es idempotente.
- **9.w.1.iter2.b.2 — Paridad codegen**: `expr_uses_auth` extendido
  para detectar `auth.{blacklist,is_blacklisted,cleanup_expired}`.
  `emit_auth_prelude` cuando `uses_auth && uses_db` emite 4 constantes
  SQL + `__fitz_ensure_token_blacklist_table` + los 3 helpers
  `__fitz_auth_*` async retornando `Result<T, String>`. `gen_call`
  despacha `auth.X(...)` a `gen_auth_blacklist/is_blacklisted/cleanup_expired`
  análogos a `gen_auth_jwt_encode/decode`. Importación cross-module
  cuando un módulo importer usa `auth.X` con db en scope. **1 E2E
  test** en `tests/compile_e2e.rs::auth_blacklist_codegen_compila_los_3_builtins_y_emite_helpers`
  valida que el programa con los 3 builtins compila a binario nativo
  sin errores.
- **9.w.1.iter2.b.3 — Docs + LSP + cierre formal**: cap 28 de
  `docs/guide.md` suma sub-sección **`auth`** (después de `hash`)
  con la API, decisiones de diseño, patrón canónico completo de
  `/auth/logout` + `/auth/refresh` + provider + cleanup `@cron`, y lo
  que NO está en el MVP (auto-mount, in-memory, refresh tokens
  dedicados). LSP `lsp.rs`: sumado `auth` a `scope_level_completions`
  + after-dot resuelve `auth.X` con signatures completas de los 3
  builtins. CHANGELOG v0.12.6 (esta entrada) + roadmap actualizado +
  deudas-post-5b nota de cierre + CLAUDE.md.

**Tests al cierre v0.12.6**: 2957 unit (+6) + 93 cli_e2e + 3
openapi_e2e + 358 compile_e2e (+1 codegen test) + 6 E2E real Postgres
(`#[ignore]`, opt-in con `FITZ_TEST_PG_URL`). Clippy `--lib --tests
--bins -- -D warnings` limpio, `cargo fmt --all --check` limpio.

**Verificación pre-bump completa** (memoria
`feedback_pre_release_verification`): roadmap ✓, guide.md cap 28
sub-sec `auth` ✓, deudas-post-5b nota cierre 9.w.1.iter2 entera ✓,
CLAUDE.md ✓, CHANGELOG ✓, README sin cambios (cap 28 ya cita auth
nativa), docs/index.md sin cambios (`@requires` ya estaba listado),
extensión VSCode grammar sin cambios (decoradores caen bajo regla
genérica) — LSP `auth` module completions añadido, examples sin
ejemplo runnable nuevo (sería un programa con DB persistente que
requiere `FITZ_TEST_PG_URL` — overkill para smoke), boilerplates
sin cambios.

**Cierre formal de Fase 9.w.1.iter2 entera** (auth completa: RBAC
custom + token blacklist). Plan original cumplido al 100%.

**Deudas residuales derivadas de 9.w.1.iter2.b** (NO bloquean Fase
13+):

- **Auto-mount de `/auth/logout` y `/auth/refresh`**: el flow exacto
  varía por proyecto; mantenerlo manual da más control. Si entra
  demanda real, sub-paso futuro con `@server(auto_auth_endpoints=true)`
  como opt-in.
- **In-memory blacklist** (sin DB): para apps sin Postgres que
  quieren revocation rápida, un `Map<Str, Int>` global + check
  manual. Trade-off: no persiste entre restarts. Sub-paso futuro con
  `auth.blacklist_local(jti, exp)` + flag opt-in.
- **Refresh tokens dedicados** (OAuth2 clásico): el MVP usa un solo
  token largo. Dual-token model queda como pattern futuro si entra
  demanda.
- **`jwt.encode` con `jti` automático**: el user pone `"jti":
  uuid.v4()` a mano. Refinamiento futuro: kwarg `jti=true` que
  auto-genera y devuelve `(token, jti)`.
- **Logging del blacklist**: el flow actual no loguea por default
  cuando un token se rechaza por blacklist. El user puede agregar
  `log.warn("token revocado", jti: jti)` adentro del provider.

**Próximo norte**: **Fase 13+** (visión post-Fase 12 — `fitz deploy`
orchestrator, `@trace`/`@metric` decoradores explícitos, feature
flags built-in) según demanda real. O release dedicado a feedback de
los usuarios reales del curso M7 antes de seguir con código nuevo.

## [v0.12.5] — 2026-06-03 — Fase 12.5: Cap 35 + curso M7 + cierre formal Fase 12 entera

**Cierre formal de Fase 12 entera** (deployment ciudadano primera
clase). Tres sub-pasos coordinados de pura documentación + curso:

- **12.5.a — Cap 35 nuevo "Deployment ciudadano primera clase"** en
  `docs/guide.md`. Vista integradora de los 4 sub-pasos de Fase 12
  (healthz/readyz auto-mount + Secret<T> + observability OTel + Docker
  autogenerado). Sub-secciones: 35.1 panorama del stack en una mirada,
  35.2 healthz/readyz, 35.3 Secret<T> + config/secret built-ins, 35.4
  referencia a cap 33 observability, 35.5 fitz docker init + build,
  35.6 ejemplo runnable end-to-end (`examples/guide/35-deploy.fitz`,
  <100 LoC con @server + @auth_provider + @admin + @requires +
  @healthz + secret() + config() + log.info estructurado), 35.7 12-
  factor compliance built-in con tabla, 35.8 deudas honestas. Cap
  validado contra smoke `GUIDE_EXAMPLES_COMPILE` (357 ejemplos en
  serie, ~5 min local). Caps subsiguientes renumerados (36 Plantillas
  + 37 Qué sigue).

- **12.5.b — Caps M7 del curso "Fitz de 0 a experto"** completos:
  - **M7.C1 — Distribución avanzada** (`c1-distribucion-binarios.md`)
    — binarios standalone, cross-compile gratis vía rustc targets,
    `--bundle-python`/`--bundle-pip`, optimización (strip/LTO/UPX),
    inspección con ldd/dumpbin.
  - **M7.C2 — Observability en producción**
    (`c2-observability-otel.md`) — `log.*` estructurado + redacción
    de Secret, spans HTTP auto con trace_id propagado, métricas
    Prometheus, bridge OTLP con Jaeger local, patterns (service name
    por env, sampling, debug en prod).
  - **M7.C3 — Secrets management** (`c3-secrets-config.md`) — distinción
    config()/secret(), tipo opaco Secret<T>, .expose() explícito,
    redacción recursiva en List/Map/Instance, load_env(.env) para dev,
    K8s secrets + Sealed/External Operators, fly/Railway/Heroku
    patterns.
  - **M7.C4 — Deploy avanzado** (`c4-deploy-docker-k8s.md`) —
    `fitz docker init`/`build`, healthz/readyz auto-mount + custom
    overrides, SIGTERM drain (30s grace), aplicar a K8s con rolling
    deploy, tabla 12-factor compliance, patterns (multi-stage compose,
    sidecar log shipping, CDN, read replicas), cierre del curso entero.
  - `docs/curso/index.md` marca M7 ✅ cerrado (C1-C4); `mkdocs.yml`
    suma la nav del módulo.

- **12.5.c — Cierre formal** (este release): CHANGELOG v0.12.5 +
  roadmap actualizado con sub-pasos detallados + deudas-post-5b.md +
  CLAUDE.md + verificación pre-bump completa (extensión VSCode sin
  cambios, examples runnable validado, smoke verde).

**Decisiones técnicas del cap 35**: (a) panorama vecino con tabla
comparativa explícita (Python/TS/Go/Spring/**Fitz**) cubriendo las 4
piezas; (b) ejemplo runnable mostrando @auth_provider + @admin +
@requires + @healthz custom + secret() + config() + log.info — TODO
el stack web first-class + Fase 12 trabajando junto en un solo
archivo; (c) explicación honesta de limitaciones (healthcheck HTTP
sin distroless, cross-module detection, etc); (d) cross-link al cap
33 (Observability) para detalle exhaustivo en lugar de duplicar.

**Decisiones técnicas del curso M7**: (a) 4 caps consistentes con el
pattern de M6 (pre-requisitos + objetivo + por qué importa + tabla
diferencial vs otros lenguajes + paso a paso + validación end-to-end +
patrones de producción); (b) M7.C1 sale del curso lineal para abrir
con distribución (no escala del binario que ya tenían de M6) — es la
puerta a los demás caps "saliendo del laptop"; (c) M7.C4 cierra el
curso entero con resumen de los 10 diferenciales aprendidos y
sugerencias de siguiente paso (boilerplates + contribución).

**Tests al cierre v0.12.5**: 2951 unit + 93 cli_e2e + 3 openapi_e2e
+ 357 compile_e2e (smoke verde local, +1 vs v0.12.4). Sin cambios de
código — esta release es 100% docs/curso. fmt + clippy clean
heredado de v0.12.4.

**Verificación pre-bump (checklist según memoria)**:

- ✓ `docs/roadmap.md` — Fase 12.5 marcada CERRADA con sub-pasos
  detallados.
- ✓ `docs/guide.md` — cap 35 nuevo + renumeración 36/37 + ejemplo
  runnable agregado al smoke.
- ✓ `docs/deudas-post-5b.md` — nota de cierre Fase 12.5 + cierre
  formal de Fase 12 entera.
- ✓ `CLAUDE.md` — entrada v0.12.5.
- ✓ `CHANGELOG.md` — esta entrada.
- ✓ `README.md` — bullet actualizado en Estado del proyecto.
- ✓ `docs/index.md` — Fase 12 marcada cerrada en lista.
- ✓ `docs/curso/index.md` — M7 ✅ cerrado.
- ✓ `mkdocs.yml` — nav M7 sumada.
- ✓ Extensión VSCode (grammar + LSP) — sin cambios (12.5 es 100%
  docs, no toca lenguaje).
- ✓ `examples/guide/35-deploy.fitz` — agregado y validado.
- ✓ `boilerplates/` — sin cambios (cap 35 referencia los existentes).
- ✓ Smoke + lints — verdes.

**Cierre formal de Fase 12 entera** — el plan original
"healthz/readyz + Secret + observability + Docker" + caps del curso
+ documentación integrada está cumplido al 100%. Próximos nortes:
9.w.1.iter2.b (token blacklist + refresh) o Fase 13+ (visión
post-Fase 12 — `fitz deploy` orchestrator, feature flags built-in,
`@trace`/`@metric` explícitos, etc.) según demanda real.

**Próximo norte**: **9.w.1.iter2.b** (sub-iter Token blacklist +
refresh) — completar la pieza faltante de auth con tabla `fitz_token_blacklist`
auto-creada y endpoints `/auth/logout`/`/auth/refresh` documentados
como pattern canónico. O Fase 13+ si aparece demanda concreta.

## [v0.12.4] — 2026-06-03 — Fase 9.w.1.iter2.a: `@requires("role")` (RBAC custom)

Decorator nuevo `@requires("role")` apilable para roles más allá de
`@admin`. El runtime ejecuta el provider e inyecta el `user`; después
verifica que `user.role` matchee al menos uno de los roles requeridos.
Si no, 403 con el role actual y los requeridos en el mensaje.

**Sintaxis**:

```fitz
@requires("editor")
@post("/articles")
fn create(body: Article, user: User) -> Article {
    // Solo si user.role == "editor"
}

@requires("editor")
@requires("publisher")
@put("/articles/{id}")
fn publish(id: Int, user: User) -> Article {
    // OR — matchea cualquiera de los dos
}
```

**Decisiones técnicas**:

- **`@requires` implica auth** — el wrapper corre el `@auth_provider`
  igual que `@authenticated`/`@admin`. No requiere apilar
  `@authenticated` explícito.
- **Multi-decorator = OR**, no AND. Razón: un user tiene UN role; pedir
  "role == 'a' AND role == 'b'" sería incoherente. OR cubre "este
  endpoint permite editor O publisher".
- **Exige `role: Str` en `User`** (no nullable) — paralelo a `@admin`.
  El checker rechaza tipos sin el field.
- **Mensaje de 403 enriquecido** con el role del user y la lista de
  requeridos: `"acceso prohibido — role 'viewer' no autorizado
  (requeridos: editor, publisher)"`. Útil para debug y observability
  (los logs lo capturan via Fase 12.3 OTel spans).
- **MVP solo singular** — `user.role: Str`, no `user.roles: List<Str>`.
  Multi-role queda como deuda residual visible (`@requires(roles=[...])`
  o el patrón canónico `if user.roles.contains("editor") { ... }`).

**Implementación**:

- **Checker** (`src/types.rs`): `check_auth_decorators` acepta `requires`
  como kind, valida shape (1 arg Str literal, sin kwargs, sólo sobre
  handlers HTTP, exige `role: Str` en User), rechaza role duplicado en
  decorators apilados. 9 unit tests nuevos.
- **Runtime** (`src/http.rs`): `RouteSpec` gana `required_roles:
  Vec<String>`. El wrapper de `dispatch_request` y el WS path dispara
  el provider cuando `auth != None || !required_roles.is_empty()`.
  Después del admin check, valida que `user.role` esté en
  `required_roles`. 5 E2E nuevos en oneshot router (role correcto =
  200, role incorrecto = 403, apilado acepta cualquiera, apilado
  rechaza viewer, sin header = 401 ANTES de evaluar role).
- **Codegen** (`src/codegen.rs`): `HandlerSig` gana `required_roles`,
  `emit_auth_check` emite el role check después del admin check,
  paralelo en el WS wrapper. `partition_program_stmts` acepta
  `requires` como decorator válido. `auth_user_param_name` lookup
  dispara también con `@requires` (no solo con `auth != None`).
- **Evaluator** (`src/evaluator.rs`): nuevo helper
  `collect_required_roles` paralelo a `collect_route_auth`. Pipeline
  `process_decorator → register_http_route/register_ws_route` propaga
  el slice nuevo. La detección del leftover `user param` dispara con
  `has_auth_decorator` (auth O required_roles).
- **LSP** (`src/lsp.rs`): nueva entrada en `decorator_completions()`
  con snippet `requires("editor")` y descripción.

**Tests al cierre v0.12.4**: 2951 unit (+14: 9 checker + 5 runtime) +
93 cli_e2e + 3 openapi_e2e. Clippy `--lib --tests --bins -- -D warnings`
limpio, `cargo fmt --all --check` limpio. (Nota: el test
`logging::tests::detect_format_respeta_override_env_pretty` es flaky
con `--test-threads=multiple` por race condition de env vars; passa
con `--test-threads=1`. Pre-existente, NO de 9.w.1.iter2.a.)

**Deuda residual derivada de 9.w.1.iter2.a** (sub-iter futuro):

- **9.w.1.iter2.b — Token blacklist + refresh**: builtins
  `auth.blacklist(db, jti, expires_at) -> Result<Null>` y
  `auth.is_blacklisted(db, jti) -> Result<Bool>` con tabla
  `fitz_token_blacklist(jti TEXT PRIMARY KEY, expires_at BIGINT
  NOT NULL)` auto-creada al primer call (paralelo a Fase 9.w.3.iter2
  cron persistente). Endpoints `/auth/logout` y `/auth/refresh` se
  escriben a mano por el user (~10 LoC cada uno) con los builtins
  disponibles; auto-mount queda fuera del MVP. Requiere DB
  obligatoria (Postgres).
- **Multi-role**: `user.roles: List<Str>` con `@requires(roles=
  [...])`. Para el MVP, multi-role se cubre apilando `@requires`
  decorators (OR) o con check manual en el handler.
- **Role hierarchy**: "admin implies editor implies viewer" no se
  modela. Aceptable para el MVP — el user lo arma a mano si quiere
  (`@requires("admin") @requires("editor") @requires("viewer")` en
  todos los handlers editor; o usa una fn helper `has_role(user, min)`
  con la lógica de jerarquía).
- **Auto-401 cuando `@requires` está apilado sin handler HTTP**: el
  checker exige `@get`/`@post`/etc apilado; sin él, error claro.

**Próximo norte**: **Fase 12.5** (cap "Deployment ciudadano primera
clase" + caps del curso M7) y luego 9.w.1.iter2.b si la demanda real
aparece.

## [v0.12.3] — 2026-06-03 — Fase 12.4.b: smart detection rica + `fitz docker build`

Cierra **Fase 12.4 entera**. Suma detección AST de interop Python y
`@cron`, ajusta runtime + compose según el shape del programa, y agrega
el sub-comando `fitz docker build [--tag X]` que tag-ea y delega a
`docker build`.

**Smart detection rica**:

- **`uses_python`** (`from python import X` o `import python.X`) → el
  runtime stage del Dockerfile cae a `python:3.12-slim-bookworm` (~55
  MB) en vez de `gcr.io/distroless/cc-debian12` (~22 MB). El binario
  producido por `fitz build` con interop dynamic-linkea
  `libpython3.12.so` que distroless no incluye; con slim-bookworm ya
  está + wget para healthcheck HTTP.
- **`uses_cron`** (cualquier `@cron` decorator) → compose suma
  `restart: unless-stopped` al service principal para que el scheduler
  sobreviva crashes/redeploys.
- **Healthcheck HTTP condicional** — si hay `@server(port)` Y el runtime
  tiene wget disponible (`uses_python` → slim-bookworm), el compose suma
  bloque `healthcheck:` que pega contra `/healthz` (auto-mounteado por
  Fase 12.1.b). Con distroless (default, sin wget), el healthcheck NO
  se emite — comentario explicativo en el compose con la receta para
  agregarlo a mano si el user cambia el runtime.

**Sub-comando nuevo `fitz docker build [--tag X]`**:

- Thin wrapper sobre `docker build -t <tag> .` en el manifest_dir.
- Default `--tag` = `<package.name>:latest`. Override con `--tag mi/app:v1`.
- Aborta con mensaje claro si no hay `Dockerfile` (sugiere `fitz docker
  init` primero) o si no hay `fitz.toml`.
- Propaga el exit code de `docker build` para que CI lo capture igual.

**Sub-pasos**:

- **12.4.b.1** — `DockerShape` gana 2 campos (`uses_python`, `uses_cron`)
  + `Default` derive (simplifica los literales de tests). Nuevos helpers
  `stmt_uses_python` (mira `Stmt::Import`/`FromImport` con `path[0] ==
  "python"`) y `stmt_uses_cron` (mira `Stmt::FnDef.decorators` con
  `name == "cron"`). `render_dockerfile` consulta `runtime_image(shape)`
  que devuelve `"python:3.12-slim-bookworm"` o `"gcr.io/distroless/cc-
  debian12"`. `render_compose` suma `restart: unless-stopped` cuando
  `uses_cron` y healthcheck HTTP cuando `server_port = Some` + `uses_python`
  (comentario explicativo cuando es distroless). Handler `docker_init_cmd`
  reporta los nuevos detectados. 13 unit tests nuevos en `docker::tests::*`
  + 4 E2E nuevos en `cli_e2e` (init con python, con cron, healthcheck
  HTTP en compose, comentario distroless).
- **12.4.b.2** — Sub-enum `DockerCmd::Build { tag }` + handler
  `docker_build_cmd(tag)` que reusa `resolve_entry(None)`, valida
  `Dockerfile` existe, invoca `std::process::Command::new("docker") build
  -t <tag> . ` en `manifest_dir` con propagación de exit code. 2 E2E
  nuevos (`build sin dockerfile aborta`, `build sin manifest aborta`).

**Tests al cierre v0.12.3**: 2937 unit (+13) + 93 cli_e2e (+6) + 3
openapi_e2e. Clippy `--lib --tests --bins -- -D warnings` limpio,
`cargo fmt --all --check` limpio.

**Smoke real verde** validado a mano contra
`boilerplates/api-postgres-python` (interop Python con SQLAlchemy):
runtime cae a `python:3.12-slim-bookworm` automático, healthcheck HTTP
emitido con wget, sin postgres en compose porque la DB se accede vía
Python (limitación conocida — el helper detecta `db.X(...)` nativo Fitz,
no interop indirecto).

**Deudas residuales derivadas de 12.4.b** (NO bloquean cierre de Fase
12):

- **Detección DB indirecta vía interop Python**: el helper `uses_db`
  solo detecta `db.X(...)` nativo Fitz. Programas que acceden a Postgres
  con `from python import sqlalchemy` no disparan el service `db` en
  compose. Workaround: usar `--force` y editar el compose, o usar el
  driver Postgres nativo de Fitz (cap 31 de la guía). Fix futuro:
  detectar `from python import sqlalchemy/psycopg2/asyncpg` con su
  propio flag separado, o sumar flag `--with-postgres` al init.
- **Healthcheck HTTP sin distroless**: el bloque healthcheck solo sale
  cuando el runtime tiene wget (`uses_python`). Para programas no-Python
  con `@server`, el user puede agregarlo a mano siguiendo el comentario
  o cambiar el runtime. Fix futuro: bundlear un mini binario HTTP probe
  en distroless, o usar healthcheck TCP (sin requerir wget) — TCP no
  valida el endpoint exacto.
- **`fitz docker build` no expone `--push`/`--platform`/`--no-cache`**:
  el wrapper es thin de propósito. Para flags avanzados, correr `docker
  build` directo. Refinable si aparece demanda real.
- **Cross-module detection** sigue siendo deuda heredada de 12.4.a:
  `@server`/`db.X(...)`/`@cron`/`from python import X` adentro de módulo
  importado no dispara el shape. Workaround: declarar todo en el archivo
  principal (caso típico).

**Próximo norte**: **Fase 12.5** — cap nuevo "Deployment ciudadano
primera clase" en `docs/guide.md` + caps del curso M7. O salto a
**9.w.1.iter2** (RBAC custom + token refresh) si el avance del curso lo
demanda.

## [v0.12.2] — 2026-06-03 — Fase 12.4.a: `fitz docker init` (Dockerfile + compose autogenerados)

Sub-comando nuevo `fitz docker init [--force]` que genera tres archivos
en el directorio del manifest:

- **`Dockerfile`** multi-stage: builder
  `ghcr.io/thegreekman76/fitz:${FITZ_TAG}` con `RUN fitz build` → runtime
  `gcr.io/distroless/cc-debian12` (~22 MB base + binario standalone).
- **`.dockerignore`** con `target/`, `.git/`, `.env*`, `__pycache__/`,
  etc.
- **`docker-compose.yml`** smart por defecto.

**Smart por defecto** (detección AST-only del entry point declarado en
`[bin].main`):

- `@server(N)` con N Int literal → `EXPOSE N` en Dockerfile + `ports:`
  en compose.
- `db.X(...)` en cualquier nodo del AST → compose suma service
  `postgres:16-alpine` con healthcheck, volume `pgdata`, y
  `DATABASE_URL: "postgres://${POSTGRES_USER:-fitz}:..."` inyectada al
  service principal con `depends_on: service_healthy`.

**Política de skip**: si un archivo ya existe, se skipea y se sugiere
`--force` para sobrescribir. Cero overwrite accidental del Dockerfile
hand-tuned de un boilerplate existente.

**Decisiones técnicas del MVP (12.4.a)**:

- **Sub-comando con sub-enum** `Commands::Docker(DockerCmd::Init {
  force })` deja la puerta abierta a `fitz docker build` de 12.4.b sin
  cambio breaking.
- **AST-only del entry point** — fast (~50ms vs ~2s del eval), no
  recursa en módulos importados (el caso típico tiene los decoradores
  en el archivo principal; deuda residual visible).
- **`uses_db` heurística generosa** paralela al `program_uses_db` del
  codegen: cualquier `db.X(...)` cuenta. Falso positivo si el usuario
  nombra una variable local `db`; trade-off aceptable, el user borra el
  service `db:` a mano.
- **Runtime distroless siempre** — programas con interop Python no
  funcionan con distroless (necesita libpython.so); 12.4.b suma
  detección automática + fallback a `debian:bookworm-slim`.
- **Sin `restart:` policies ni healthchecks HTTP** en 12.4.a — eso lo
  cubre 12.4.b según `@cron` / `@healthz`/`@readyz`.

**Sub-paso (un solo sub-paso de 12.4.a)**:

- **12.4.a** — nuevo módulo `src/docker.rs` (~520 LoC con tests) con la
  API pública (`DockerShape`, `detect_shape`, `render_dockerfile`,
  `render_dockerignore`, `render_compose`, `init` con `InitResult`).
  `src/lib.rs` exporta el módulo. `src/main.rs` suma el sub-enum +
  handler `docker_init_cmd` (~95 LoC) que reusa `resolve_entry(None)`
  para walkear al manifest. 18 unit tests en el módulo + 6 E2E en
  `tests/cli_e2e.rs` (CLI puro, HTTP con `@server`, con `db.connect`,
  skip sin force, sobrescribe con force, abort sin manifest). Smoke
  real validado contra `boilerplates/api-simple` (HTTP, no DB) y
  `boilerplates/api-postgres-fitz` (HTTP + DB con compose smart).

**Tests al cierre v0.12.2**: 2924 unit (+18) + 87 cli_e2e (+6) + 3
openapi_e2e. Clippy `--lib --tests --bins -- -D warnings` limpio,
`cargo fmt --all --check` limpio.

**Deuda residual derivada de 12.4.a** (NO bloquea 12.4.b):

- Cross-module detection — `@server`/`db.connect` adentro de un módulo
  importado no dispara el shape. Workaround: declarar `@server` en el
  archivo principal (caso típico).
- Falso positivo `uses_db` si hay variable local llamada `db`. User
  edita el compose a mano (deuda menor, paralela al codegen).
- Detección Python interop diferida a 12.4.b (cuando dispara, fallback
  a `debian:bookworm-slim` automático).
- Healthchecks HTTP + `restart:` policies en compose diferidos a 12.4.b
  (depende de `@healthz/@readyz` + `@cron`).
- `fitz docker build [--tag X]` wrapper diferido a 12.4.b.

**Próximo norte**: **Fase 12.4.b** — smart detection rica (Python
fallback + healthchecks + cron restart) + `fitz docker build` wrapper.

## [v0.12.1] — 2026-06-03 — Fase 12.3.iter2: cierre de deudas residuales

Mini-tanda dedicada a cerrar las deudas residuales de Fase 12.3. Tier 1
(correlación trace_id + bridge logs) + Tier 3 (Prometheus) cerrados;
Tier 2 (bridge métricas OTel) INTENTADO y BLOQUEADO por version
conflict del crate (esperando release nuevo). Sumado **cap 33 nuevo
"Observability"** en la guía con renumeración 33→34, 34→35, 35→36.

**Deudas residuales de 12.3 al cierre**:

- ✓ #2 Bridge logs OTel → CERRADO (iter2.b)
- ✓ #3 Correlación trace_id Fitz↔OTel → CERRADO (iter2.a)
- ✓ #4 Endpoint /metrics Prometheus → CERRADO (Tier3)
- ✓ #5 Cap dedicado en guide.md → CERRADO
- ⚠ #1 Bridge métricas OTel → BLOQUEADO (crate
  `metrics-exporter-opentelemetry 0.2.1` pinea `opentelemetry_sdk 0.31`
  mientras usamos 0.32; master del crate ya está en 0.32, esperando
  release oficial). Workaround end-to-end: Tier3 (Prometheus scrape)
  cubre 90%.

**iter2.a — Correlación trace_id Fitz↔OTel**: `dispatch_request` y el
wrapper HTTP del codegen abren el span OTel ANTES del SpanContext
propio; nuevo constructor `SpanContext::with_ids(trace_id, span_id)`
deriva los IDs del span OTel. El `trace_id` en logs stderr matchea
exactamente el del backend OTel (Jaeger/Tempo/Datadog) →
cross-pipeline queries habilitadas. Paridad bit-a-bit codegen.

**iter2.b — Bridge logs OTel**: `emit_log_record` emite en paralelo a
stderr Y al backend OTel via OTLP HTTP/proto sobre `/v1/logs` cuando
el provider está activo. Trace context derivado del SpanContext →
correlación logs↔spans automática en el backend. Decisión
arquitectónica: SDK `opentelemetry::logs` directa (no
`opentelemetry-appender-tracing` — refactorizar el formatter custom
JSON/pretty de 12.3.a no se justifica).

**Tier3 — Endpoint `/metrics` Prometheus**: `@server(prometheus=true)`
compile-time + env var `FITZ_PROMETHEUS=1`/`true`/`yes` runtime
override. Cuando activo, `serve()` instala `PrometheusBuilder` como
recorder global del crate `metrics` y `build_router` auto-mounta
`GET /metrics` con exposition format. Mismo puerto + transporte que
el resto de la app (NO un puerto separado). Si Prometheus + OTel
ambos activos, Prometheus gana (solo UN recorder global de `metrics`
permitido).

**Cap 33 nuevo "Observability — logs, spans, métricas, OTel"**:
~300 LoC markdown end-to-end — structured logging con kwargs y
redacción Secret, spans HTTP automáticos + correlación trace_id,
OTel exporter (TracerProvider + LoggerProvider) opt-in via env var,
`/metrics` Prometheus opt-in, patrón canónico stack completo,
recetas comunes, panorama vecino (FastAPI/Express/Go/Rust), y
honestidad sobre lo que NO hace.

**Validación al cierre**: 2906 unit + 81 cli_e2e + 3 openapi_e2e + 4
compile_e2e log codegen + smoke ~290 ejemplos verde en 833s.
Clippy `--lib --tests --bins -- -D warnings` + fmt limpios.

## [v0.12.0] — 2026-06-03 — Fase 12.3 entera: Observability minimal con OpenTelemetry

Cierre formal de Fase 12.3 en 3 bloques + 11 sub-pasos. Observability
ciudadana de primera clase en el core del compilador, con
OpenTelemetry collector compatible (Jaeger, Tempo, Honeycomb, Datadog,
etc.).

**12.3.a — Structured logging built-in** (3 sub-pasos):
`log.info/warn/error/debug(msg, kwargs)` con kwargs heterogéneos
(`Int`/`Float`/`Str`/`Bool`/`Null`/`Secret`/`List`/`Map`/nominal),
output JSON flat a stderr por default con `timestamp` + `level` +
`msg` + kwargs; pretty mode con ANSI colors cuando TTY o
`FITZ_LOG_FORMAT=pretty`. Filter via `FITZ_LOG=info|debug|warn|error`
(default `info`). Redacción recursiva de `Value::Secret` en
`List`/`Map`. Stack: `tracing` + `tracing-subscriber` + `chrono` +
`serde_json`. Paridad bit-a-bit `fitz run` ↔ `fitz build`.

**12.3.b — Spans HTTP + métricas + correlación trace_id** (5
sub-pasos): cada request HTTP abre un `SpanContext` root con IDs
OTel-compatibles (`trace_id` 32 hex / `span_id` 16 hex generados con
`uuid::Uuid::new_v4()`). Logs del handler heredan automático
`trace_id`/`span_id` via `tokio::task_local!` (atraviesa thread
boundaries multi-thread). Access log `log.info("http.access", ...)`
con `http.method`/`http.target` (template del route)/
`http.status_code`/`duration_ms`. Counter `http_requests_total{method,
path, status}` + Histogram `http_request_duration_seconds{method,
path, status}` con labels iguales para correlación cross-metric.
Opt-out total con `@server(observability=false)` que bypassa el
wrapper de instrumentación entero.

**12.3.c — OTLP exporter** (3 sub-pasos): cuando
`OTEL_EXPORTER_OTLP_ENDPOINT` está seteada, conexión a backend OTel
real con `opentelemetry-otlp = "0.32"` feature `http-proto` (sobre
gRPC por simplicidad + compat proxy + recomendación
Datadog/Honeycomb). Sampler `TraceIdRatioBased` con
`OTEL_TRACES_SAMPLER_ARG` clamp `[0.0, 1.0]`. Service name desde
`OTEL_SERVICE_NAME` (default `"fitz-app"`). Sin la env var, no-op
silencioso — zero overhead, zero conexiones de red. Paridad bit-a-bit
intérprete↔binario.

**Deudas residuales derivadas** (NO bloquean Fase 12.4): bridge
métricas OTel, bridge logs OTel, correlación trace_id Fitz↔OTel,
endpoint `/metrics` Prometheus opt-in, cap dedicado en guide.md.
**Las 5 cerradas en v0.12.1** excepto #1 (bridge métricas OTel) que
quedó bloqueada por version conflict del crate
`metrics-exporter-opentelemetry` 0.31 vs nuestro 0.32 — esperando
release del crate.

**Validación al cierre**: 2894 unit + 81 cli_e2e + 3 openapi_e2e + 4
compile_e2e log codegen + smoke ~290 ejemplos verde. Clippy
`--all-targets -- -D warnings` + `cargo fmt --all --check` limpios.

## [v0.11.2] — 2026-06-02 — 9.w.3.iter2: Persistencia + retry + timezone + catch_up en `@cron`

**Cierre formal de Tier 1 de las deudas pre-M5** acordadas el
2026-06-01. Las tres deudas que bloqueaban escribir M5.C26 del
curso (Jobs sin Celery) cerradas en bloque, paralelo bit-a-bit
entre intérprete y codegen.

`@cron("expr")` acepta **4 kwargs opcionales nuevos** — programas
viejos siguen funcionando idénticos:

```fitz
let db = db.connect(env_or("DATABASE_URL", "postgres://...")).await

@cron("0 9 * * *",
      tz="America/Argentina/Buenos_Aires",
      retry={max: 3, backoff: "exponential",
             initial_secs: 1, max_secs: 30},
      catch_up=true,
      store=db)
async fn cleanup() -> Result<Null> { ... }
```

- **`tz="IANA/Name"`** — interpreta el schedule en huso indicado
  (vía `chrono_tz`). Default `"UTC"`.
- **`retry={max, backoff, initial_secs, max_secs}`** — hasta N
  reintentos con backoff (`"exponential"`/`"linear"`/`"constant"`),
  cada delay capeado por `max_secs`. Default: sin retry.
- **`catch_up=true`** — al boot, si hubo missed runs entre
  `last_run_at` y `now`, ejecuta UN run inmediato (no N — evita
  spam). Default `false` = skip.
- **`store=<binding>`** — persiste el registry + cada attempt en
  `fitz_cron_jobs` / `fitz_cron_runs` (auto-creadas con
  `CREATE TABLE IF NOT EXISTS`). Visibility manual con `psql`.

`@background` acepta los mismos `tz` y `retry` (sin `store` ni
`catch_up` — persistencia de `spawn(...)` diferida a iter3).

### Sub-paso a — Checker estático de kwargs

Helpers libres `check_job_kwargs` + `check_retry_map` en
`src/types.rs` parametrizados por allowed-list. Valida shape
sintáctico (Str/Bool/Map literal según kwarg), rechaza
duplicados y desconocidos con la lista de aceptados.
`extract_int_literal` reconoce `Int(N)` y `UnaryOp { Neg, Int(N) }`
para negativos. **+20 unit tests** (15 `cron_*` + 5
`background_*`); total 24 + 8 al cierre.

### Sub-paso b — Runtime intérprete: tipos extendidos

`src/cron_jobs.rs`: `enum BackoffKind` (default `Exponential`) +
`struct RetryConfig` con `Default` (max=0) + `delay_for_attempt`
capeado + `struct CronJobOptions { tz, retry, catch_up, store }`
con `Default`. `CronJob` gana los 4 campos; `register` acepta
`CronJobOptions` como parámetro final.

`src/evaluator.rs`: `register_cron_job` parsea kwargs del
`Decorator` vía `parse_cron_job_options` + sub-helpers
(`parse_retry_kwarg`, `resolve_store_kwarg`). El IANA real lo
valida `chrono_tz::Tz::from_str` con error claro si falla.
**+11 unit tests del registry + +7 unit tests del evaluator**.

### Sub-paso c — Scheduler intérprete + tests E2E reales

`src/cron_jobs.rs`: 7 helpers SQL (`init_storage` /
`upsert_job_row` / `record_run_start` / `record_run_finish` /
`update_job_last_run` / `read_last_run_at` /
`parse_pg_timestamptz` — el último normaliza offset Postgres sin
minutos). `run_cron_job` boot con init storage + upsert + catch_up
(`Schedule::after(last)` en la tz). Loop tz-aware con
`Schedule::upcoming(job.tz)` + `invoke_with_retry`.

Schema:

```sql
fitz_cron_jobs(
    name PK, schedule, tz,
    last_run_at, last_status, last_error, next_run_at
)
fitz_cron_runs(
    id BIGSERIAL, job_name, started_at, finished_at,
    status, attempt, error
)
-- status: 'running' | 'ok' | 'failed' | 'retrying'
-- attempt: 1-indexed; retry máx N produce hasta N+1 rows
```

**+6 tests E2E reales** contra Postgres en
`tests/cron_jobs_real_postgres.rs` (`#[ignore]`, requieren
`FITZ_TEST_PG_URL`).

### Sub-paso d — Codegen `fitz build` paridad bit-a-bit

`src/codegen.rs` (~720 LoC nuevas): `CronJobInfo` extendido con
los 4 campos parseados build-time. `program_has_persistent_cron`
walka AST y fuerza `uses_db=true` cuando encuentra
`store=<ident>`. Preludio dividido en 4 constantes
(`JOBS_COMMON_PRELUDE` + `JOBS_RUN_PRELUDE_SIMPLE` cuando no hay
persistencia, o `SQL_HELPERS_PRELUDE` +
`JOBS_RUN_PRELUDE_PERSISTENT` cuando sí). Trait
`__FitzCronStoreFrom` polimórfico acepta `__FitzDbConn` directo
o `Result<__FitzDbConn, String>` (caso idiomático `let db =
db.connect(...).await` sin `?` top-level). `gen_main` reordena:
stmts del usuario van ANTES de `emit_cron_job_spawns` para que
bindings top-level estén en scope.

`src/evaluator.rs::resolve_store_kwarg` también acepta
`Value::Result(Ok(DbConn))` (paridad bit-a-bit con el trait del
codegen).

Validado contra Postgres 15 local: binario nativo con
`@cron("*/2 * * * * *", store=db)` crea las dos tablas, persiste
3 runs `status='ok' attempt=1` en 6s; `last_status='ok'` en
`fitz_cron_jobs`.

### Sub-paso e — Cap 30 + ejemplo + LSP refresh

`docs/guide.md` cap 30 "Jobs sin Celery" — sub-sección nueva
**"Persistencia, retry y timezone (iter2)"** documenta los 4
kwargs con shape, defaults, schema DDL, queries de visibility
con `psql`, semántica del binding `Result<DbConn>` top-level +
`__FitzCronStoreFrom`. Limitación conocida documentada
(`fitz run` cron-only con `store=db`). Sub-sección "Qué no está
en el MVP" reescrita: salen los 3 items cerrados; entra
`@background` con persistencia + retry (diferido a iter3).

`examples/guide/30b-cron-persistente.fitz` (~50 LoC) — HTTP+cron
con los 4 kwargs combinados. Sumado al smoke
`GUIDE_EXAMPLES_COMPILE` (~290 ejemplos, todos verde en ~7 min).

`src/lsp.rs` — descripciones de `@cron`/`@background` mencionan
los kwargs nuevos. Grammar TextMate sin cambios. **112 tests LSP
verdes**.

### Cierre formal — Tier 1 del curso

Total al cierre: **2792 unit + 6 E2E real Postgres + 1
compile_e2e smoke + 112 LSP**. `cargo fmt --all` + `cargo clippy
--all-targets -- -D warnings` limpios. mkdocs build sin
warnings nuevos.

Próximo norte: **9.w.1.iter2** (Tier 2 — RBAC custom + token
refresh) o saltar a Fase 12 según necesidad del curso.

---

## [v0.11.1] — 2026-06-01 — Fase 13 polish: short flags + Bool=true negation + List<Str> variadic + fix CI fmt drift

**4 sub-pasos coordinados (~5h reales)** cerrando las 3 deudas
residuales de Fase 13 (v0.11.0) + un fix CI permanente.

### Sub-paso 1 — rustfmt.toml committed + fix CI fmt drift

**Fix permanente** del fail CI de v0.11.0 (`src/cli.rs:419` se
formateaba distinto entre Windows local y Ubuntu CI). Causa: el
repo no tenía `rustfmt.toml` committed, cada versión de rustfmt
aplicaba defaults sutilmente distintos.

- `rustfmt.toml` nuevo en repo root con `edition = "2021"`,
  `max_width = 100`, `use_small_heuristics = "Default"`. Fija el
  formato canonical para todos los devs + CI sin importar la
  versión de rustfmt del runner.
- Deuda documentada en `docs/deudas-post-5b.md` como CERRADA con
  contexto del incidente y la decisión técnica.

### Sub-paso 2 — Short flags auto-inferidos

`-l` como atajo de `--loud` se infiere de la primera letra del
nombre del flag. **Sin sintaxis extra del lado del user**:

```fitz
@command("greet")
fn greet(name: Str, loud: Bool = false, count: Int = 1) -> Int {
    // Auto: --loud / -l, --count / -c
    ...
}
```

```bash
$ ./mybin Ada -l -c 3
HELLO, Ada!
HELLO, Ada!
HELLO, Ada!
```

- Helper nuevo `compute_short_flags(params) -> Result<HashMap<char,
  String>, String>` en `src/cli.rs` que infiere los mappings.
- **Detección de colisiones en compile-time**: dos flags con misma
  primera letra (`loud` + `level`) → error claro al `fitz build`
  con sugerencia ("Renombrá uno de los dos").
- Parser de argv (`parse_argv` en intérprete + dispatcher generado
  en `gen_cli_command_helpers`) normaliza `-x` → `--<long>` antes
  del match flag. Same path para ambos. Soporta solo `-x` single en
  MVP — combo `-xyz` y `-x=v` quedan como deuda menor.
- Help text muestra `-l, --loud` cuando hay short asignado.

### Sub-paso 3 — `Bool = true` con `--no-<flag>` negation

Lifted la restricción MVP `Bool = true rechazado`. Ahora:

```fitz
@command("go")
fn go(verbose: Bool = true) -> Int {
    if verbose { print("verbose mode ON") } else { print("quiet") }
    return 0
}
```

```bash
$ ./go                 # default → true
verbose mode ON

$ ./go --no-verbose    # negación explícita → false
quiet
```

- Checker actualizado para aceptar `Bool = true` defaults.
- Parser de argv reconoce `--no-<name>` para Bool flags: si el
  resto matchea un flag Bool del comando, set a false. Si el nombre
  empieza con `no-` pero no matchea (caso raro `--noisy` legítimo),
  cae al path normal.
- Help text emite `--no-<name>` para Bool con default true (paralelo
  a Cargo `--no-default-features`).
- Codegen emite arms `"no-<name>"` antes de los arms de flag normales
  para el match parser.

### Sub-paso 4 — `List<Str>` variadic positional

Último param de tipo `List<Str>` con default `= []` absorbe N
tokens posicionales restantes:

```fitz
@command("run")
fn run(mode: Str, verbose: Bool = true, files: List<Str> = []) -> Int {
    if verbose { print("mode: {mode}") }
    for f in files {
        print("  - {f}")
    }
    return 0
}
```

```bash
$ ./run fast a.txt b.txt c.txt
mode: fast
  - a.txt
  - b.txt
  - c.txt

$ ./run fast --no-verbose
# mode no impreso, files vacía
```

- Checker permite `List<Str>` como ÚLTIMO param de todos. Variadic
  posicionado en otra ubicación → error.
- Convención del `= []` default: requerido porque el parser Fitz
  exige "después del primer default, todos los siguientes también".
  El `[]` es semánticamente redundante (variadic siempre empieza
  vacío y acumula) pero satisface el shape sintáctico.
- Parser de argv: detecta variadic por type+posición, acumula
  tokens restantes en `Vec<String>` → wrappea como
  `Value::List(Arc<Mutex<...>>)`.
- Codegen emite `__cli_variadic_<name>: Vec<String>` accumulator +
  wrap final a `Arc<Mutex<Vec<String>>>` (mismo shape que los List
  del runtime Fitz post-F17).
- Variadic excluido de short flag auto (no es flag), de OPTIONS
  section del help, y aparece en USAGE como `[<files>...]`.

### Decisiones técnicas

- **Short flags auto vs explícito**: optamos por auto-inferir
  (primera letra) en vez de `@flag(short="l")` decorator porque
  evita AST change en `Param` y matchea la convención CLI estándar
  (POSIX, GNU). El override manual queda como deuda futura si entra
  presión.
- **Variadic requiere `= []` default**: violación menor de
  semántica (variadic no necesita default conceptualmente) a cambio
  de no tocar el parser Fitz. Trade-off aceptado por scope.
- **`--no-<flag>` solo para Bool con default true**: técnicamente
  podríamos soportar `--no-<flag>` para Bool con default false
  también (negaría a false redundante), pero no aporta y agrega
  ruido al help. Si el user lo quiere, escribe `--<flag>=false`.
- **Smoke negation tiene priority sobre flag literal `no-foo`**: si
  hay un Bool flag llamado `no-foo` Y un Bool flag llamado `foo`, la
  arm `"no-foo"` matchea PRIMERO (case-sensitive exact). Documentado
  pero raro en práctica.

### Tests

- **7 E2E nuevos** en `tests/compile_e2e.rs`:
  `fase_13_short_flags_auto_inferidos`,
  `fase_13_short_flag_desconocida_es_error`,
  `fase_13_bool_default_true_se_niega_con_no_flag`,
  `fase_13_list_str_variadic_absorbe_positionals`,
  `fase_13_list_str_variadic_vacio_aceptado`,
  `fase_13_paridad_run_vs_build_polish` (paridad bit-a-bit con
  short flags + variadic + Bool=true combinados),
  `fase_13_short_flag_collision_es_error_compile`.
- Total Fase 13 E2E al cierre: **17/17 verdes** (10 de v0.11.0 + 7
  nuevos de v0.11.1).
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (293 ejemplos).
- Clippy `--all-targets -D warnings` + `--features lsp` limpios.
  fmt `--all --check` ahora consistente entre Windows y Linux gracias
  al `rustfmt.toml` committed.

### Ejemplo `examples/guide/33-cli.fitz` actualizado

Cap intro del ejemplo guide actualizado con las 3 features nuevas
(short flags, Bool=true negation, variadic) documentadas en el
header comment.

### Total al cierre v0.11.1

**2754 unit + 293 smoke + 81 cli_e2e + 341 compile_e2e (+7 nuevos
Fase 13 polish) + 3 openapi + 61 db_real_postgres**.

### Próximo norte

Mismo que v0.11.0: **Fase 12** (Deployment) o **Tier E del ORM**.

## [v0.11.0] — 2026-06-01 — Fase 13: CLI builder nativo (`@command`)

**Bump menor → 0.11.0** porque Fase 13 cierra entera una nueva
ciudadana primera del lenguaje. Funcionalmente backward-compatible
con v0.10.32 (los programas existentes siguen funcionando), pero
suma una capacidad core del lenguaje que justifica el salto de
minor.

**5 sub-pasos coordinados (~10h reales vs ~12h estimadas)**.
`@command("name", desc="...")` sobre una `fn` la declara como comando
CLI; el binario producido por `fitz build` parsea
`std::env::args()` y dispatcha al comando matching, con **help
auto-generado** y **parser de positional args + flags** con **zero
deps externas**. Convención sin decorators en params: positional vs
flag se infiere del `default = ...` del param.

### Sintaxis canónica

```fitz
@command("greet", desc="Greet a person")
fn greet(name: Str, loud: Bool = false, count: Int = 1) -> Int {
    let n = count
    while n > 0 {
        if loud { print("HELLO, {name}!") } else { print("hello, {name}") }
        n = n - 1
    }
    return 0
}
```

```bash
$ ./mybin greet Ada --loud --count 3
HELLO, Ada!
HELLO, Ada!
HELLO, Ada!

$ ./mybin --help
USAGE:
    mybin <command> [ARGS] [OPTIONS]
COMMANDS:
    greet    Greet a person
...
```

### Sub-pasos

- **13.1 — Parser/AST/Checker**: nueva fn `check_command_decorator`
  en `src/types.rs` valida shape (arg Str literal con nombre del
  comando, opcional kwarg `desc=`, return type `Int`, params
  CLI-marshallables `Str/Int/Float/Bool/Str?`, sin varargs, sin
  conflictos con decorators de servidor/job/test/middleware). Bool
  con `default = true` rechazado en MVP (requiere convención
  `--no-flag` para negar — documentado).
- **13.2 — Evaluator (intérprete)**: nuevo módulo `src/cli.rs` con
  `CliRegistry` (paralelo a `CronRegistry`), `CliCommand`,
  `parse_argv()` con multi-command detection + dispatch. Helper
  `with_active_cli_registry`/`install_cli_registry` thread-local
  en `src/evaluator.rs`. `process_decorator` branch para
  `@command` que pushea al registry activo. `src/main.rs::run_file`
  instala el registry pre-eval y, post-eval, dispatcha CLI si
  `count > 0` (skip HTTP/cron en ese caso).
- **13.3 — Help autogeneration**: funciones puras
  `render_global_help`/`render_command_help`/`usage_line`/
  `render_args_section`/`render_options_section` en `src/cli.rs`.
  El help se construye desde los specs registrados (no requiere
  doc-strings). Padding consistente con clap.
- **13.4 — Codegen (`fitz build`)**: `gen_cli_main` emite `fn main()`
  (o `#[tokio::main]` si algún @command es async) con dispatch
  estático: detecta modo single vs multi, parsea `--help` global,
  matchea el subcomando contra arms generados. Per-command:
  `__fitz_cli_run_<cmd>` parsea positional + flags con
  type-coerciones (`parse::<i64>`/`parse::<f64>` + error claro
  con exit 2). Help string emitida como const en build-time.
  **Paridad bit-a-bit `fitz run` ↔ `fitz build`** validada con E2E
  (`fase_13_cli_paridad_run_vs_build`).
- **13.5 — Tests + ejemplo + docs**: 10 E2E nuevos en
  `tests/compile_e2e.rs` (single command positional, multi-command
  dispatch, Bool + Int flags, help global, help per-command,
  comando desconocido exit 2, missing positional exit 2, bad Int
  flag exit 2, exit code from handler, paridad run↔build). Ejemplo
  runnable `examples/guide/33-cli.fitz` con 3 commands (greet/add/
  status) sumado al smoke `GUIDE_EXAMPLES_COMPILE`.

### Decisiones técnicas

- **Convención sin `@arg`/`@flag` decorators**: la presencia de
  `default = ...` en el param determina si es positional o flag.
  Reduce verbosidad vs Click (Python) que exige `@click.argument`
  por cada param. Trade-off: NO se puede tener positional
  optional args (los con default son flags). Para casos límite, usá
  `Str?` (nullable) que mantiene shape pero requiere `match` en el
  body.
- **Exit codes POSIX**: `0` éxito, `1+` retornado por el handler,
  `2` errores de parsing del CLI. Convención estándar Linux.
- **Detección de modo automática**: el binario tiene un único
  "modo" determinado por los decorators presentes (`@get*` → HTTP,
  `@cron` → cron-only, `@command` → CLI, ninguno → script plain).
  Mutuamente excluyentes — el checker rechaza combinaciones con
  error claro.
- **Help string emitida en build-time como const**: en lugar de
  construirla en runtime, el codegen emite las strings inline para
  cada comando. Trade-off: binario más grande (~50 bytes por
  comando) pero startup más rápido y sin allocs en el path normal.
- **Boolean flag presence semantics**: `--loud` sin valor activa
  el flag a `true` (idiomático CLI). `--loud=false` también funciona
  para override explícito. Sin valor con flag no-Bool → error claro.
- **`Bool = true` rechazado en MVP**: requiere convención
  `--no-<flag>` para negar (paralelo a Cargo `--no-default-features`).
  Implementable, deuda menor — el user invierte la lógica por ahora.

### Tests

- **10 E2E** en `tests/compile_e2e.rs::fase_13_cli_*` cubriendo
  todo el path codegen + paridad bit-a-bit con el intérprete.
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos + el nuevo
  `33-cli.fitz` = 293).
- Clippy `--all-targets -D warnings` + `--features lsp` limpios.
  fmt `--all --check` limpio.

### Boilerplate `cli-tool` actualizado

`boilerplates/cli-tool/` ahora usa `@command` idiomático: 3
comandos (`report`/`count`/`regions`) con help auto-generado,
positional args y flags. README actualizado con demo completa de
la nueva sintaxis. El binario compilado sigue siendo ~5 MB Linux
standalone, imagen Docker `distroless/cc` ~22 MB.

### Otros cambios incluidos en este release

- **Fix CI LSP**: `make_hover_with_range` cambió signature en v0.10.32
  (Tier D.2 — sumó `program: &Program`) pero el test unit
  `lspy_make_hover_with_range_incluye_range_del_ident` no se
  actualizó. Compilaba con `cargo test --lib` (sin `--features lsp`)
  pero rompía en el job CI `cargo test --features lsp --lib lsp::`.
  Fixed con `&Vec::new()` (Program vacío — el test valida solo el
  Range, no el augment).

### Total al cierre v0.11.0

**2739 unit + 293 smoke + 3 openapi + 81 cli_e2e + 334 compile_e2e
+ 61 db_real_postgres** (+10 nuevos Fase 13 en compile_e2e).
Acumulado de los `src/cli.rs` unit tests: ~15 directos.

### Por qué Fase 13 importa

Hace de Fitz **el único lenguaje moderno** que combina HTTP nativo
+ WebSockets tipados + ORM + jobs + **CLI builder nativo** en el
core del compilador, con paridad bit-a-bit intérprete↔binario, **zero
deps externas** para todas estas features intrínsecas. Cualquier
otro stack requiere `clap`/`argparse`/`click`/`commander` separado.

### Próximo norte

**Fase 12** (Deployment ciudadano primera clase) o resto del **Tier E
del ORM**. Detalle de cada uno en `docs/roadmap.md`.

## [v0.10.32] — 2026-06-01 — Tier C + D del cierre ORM/DB (operadores SQL + DX/LSP residual)

**5 features coordinadas en bundle (~8h reales vs ~20h estimadas)**
cerrando los 2 últimos tiers no-visión del ORM/DB. Sin breaking
changes, paridad bit-a-bit `fitz run` ↔ `fitz build` mantenida. 3
E2E nuevos contra Postgres real + 0 regresiones en 2739 unit + 292
smoke + 81 cli_e2e + 324 compile_e2e + 3 openapi.

### Tier C — Operadores SQL faltantes

- **C.1 — `ts_rank` full-text ranking** en `.order_by(...)`. Sintaxis:
  `.order_by(fn(u) => -u.body.rank("query"))` emite
  `ORDER BY ts_rank("body", to_tsquery('query')) DESC`. Combinable con
  `.where(fn(u) => u.body.matches("query"))` para ordenar resultados
  full-text por relevancia. Variante `plainto_rank` para queries del
  estilo "plain text" (`plainto_tsquery`). El query string se inlina
  como SQL literal en MVP — vars quedan como deuda menor. Habilita el
  pattern canonical de search ranking sin escape hatch a `db.query`.
- **C.2 — Expression indexes** con `@index(expression="lower(email)")`.
  El user pasa la SQL expression raw como kwarg dedicado; el codegen
  emite `CREATE INDEX ... ON tbl (<expression>)` literal. Habilita
  case-insensitive UNIQUE (`lower(email)`), full-text setup tsvector
  (`to_tsvector('english', body)`), totals computados
  (`(price * quantity)`), etc. **Drift check incompleto** documentado:
  la introspect lee el index del catálogo pero NO parsea
  `pg_index.indexprs` para detectar el expression — el user nombra
  el index explícito con `name=` para drift name-based reliable.
- **C.3 — JSON `||` merge en `.merge_jsonb`**. Sintaxis:
  `User.where(fn(u) => u.id == 5).merge_jsonb(db, "data", {"new": "v"}).await?`.
  Emite `UPDATE tbl SET "data" = "data" || $1::jsonb WHERE id = $2`
  preservando las keys existentes del objeto jsonb y aplicando el
  patch (overwrite si existen, agregar si no). Limitación Postgres
  documentada: `NULL || anything = NULL` — el user inicializa la col
  con `{}` al INSERT para que el merge funcione. Field debe ser
  `Map<...>` (jsonb); error de checker si se intenta sobre otro tipo.

### Tier D — DX/LSP residual

- **D.1 — LSP completion ORM en `.where()`**. Los métodos ORM
  (intercepted por el evaluator solo en `.where(closure)` context)
  ahora aparecen en autocomplete del LSP cuando tipás `u.email.` o
  `u.data.`. Los detail muestran `(ORM .where)` para distinguirlos
  de métodos regulares. Cobertura: **Str** (`is_in`, `like`, `ilike`,
  `matches`, `plainto_matches`, `between`, `is_null`, `is_not_null`),
  **Map** (`has_key`, `has_all_keys`, `has_any_keys`, `contains_json`,
  `has_path`, `path_text/int/float/bool`), **Int/Float**
  (`is_in`, `between`, `is_null`, `is_not_null`), **Date/DateTime**
  (idem). Fuera del `.where`, llamarlos genera error en runtime —
  el LSP no detecta el contexto (limitación documentada).
- **D.2 — LSP hover sobre `@table` types**. Al hover sobre un
  identificador que tipa como `Type::Nominal(id)` con `@table`
  metadata, el tooltip ahora incluye el `CREATE TABLE` SQL emitted
  bajo el tipo declarado. Implementado vía
  `migrations::schema_from_program` + `create_table_sql_for`. Útil
  para debuggear migrations sin abrir `fitz db diff` — el LSP
  muestra exactamente el shape SQL que el migrator emite. Si
  `schema_from_program` falla (typo en relations, FK target no
  existente), el augment se skipea (hover sigue mostrando solo el
  tipo, sin error visual).

### Decisiones técnicas

- **C.1 inline el query string**: en lugar de bindear `$N`, el
  query string se inlina al SQL literal (`to_tsquery('query')`). El
  trade-off: aceptar solo Str literal en MVP, vars quedan como
  deuda menor. Razón: el path order_by stream del runtime no tiene
  acceso al pg_args store del where; cambiarlo requeriría refactor
  cross-method.
- **C.2 kwarg `expression=` exclusivo**: `@index(expression="...")`
  NO acepta arg posicional simultáneo (`@index("col", expression="...")`).
  Forzar la elección "cols o expression" evita ambigüedad semántica.
- **C.3 `.merge_jsonb` separado de `.update`**: en lugar de embedded
  semantics en `.update` con flags, una method dedicada. API más
  explícita en el call site, signatura simple `(db, field, patch)`.
- **D.1 completion sin scope detection**: el LSP NO detecta si el
  cursor está adentro de un `.where(...)` closure — los métodos
  ORM aparecen sobre Str/Map/etc. en todos los contextos. El detail
  `(ORM .where)` informa al user.
- **D.2 hover augment idempotente**: si `schema_from_program` falla,
  el hover devuelve solo el tipo display (sin SQL).

### Tests

- **3 E2E reales contra Postgres** en `tests/db_real_postgres.rs`:
  `tier_c1_ts_rank_order_by_works`, `tier_c2_expression_index_creates_lowercase_unique`,
  `tier_c3_jsonb_merge_preserves_existing_keys`. Todos verdes
  contra Postgres local.
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos).
- Clippy `--all-targets -D warnings` + `--features lsp` limpios.
  fmt `--all --check` limpio.

### Total al cierre v0.10.32

**2739 unit + 292 smoke + 3 openapi + 81 cli_e2e + 324 compile_e2e
+ 61 db_real_postgres** (+3 nuevos del Tier C).

## [v0.10.31] — 2026-06-01 — Tier A del cierre ORM/DB: MVP fuerte (3 bloques en bundle)

**9 features en bloque (~12h reales vs ~30-40h estimadas)** que
llevan el ORM/DB del estado "funcional con fricciones residuales" a
"MVP fuerte sin caveats conocidos" para el caso de uso real. Sin
sintaxis nueva mayor (solo kwargs sobre 2 built-ins existentes),
zero deps externas, paridad bit-a-bit `fitz run` ↔ `fitz build`
mantenida. 6 E2E nuevos contra Postgres real + 0 regresiones en
2739 unit + 292 smoke + 81 cli_e2e + 324 compile_e2e + 3 openapi.

### Bloque 1 — Diff seguro + ALTER + CHECK constraints (A.1 + A.2 + A.5)

- **A.1 — `fitz db diff --check-destructive`**: clasifica cada
  change como `Safe` / `Risky` / `Destructive` y aborta con exit 1
  si hay destructive sin `--allow-destructive` explícito. El SQL
  emitido suma comentarios `-- [SAFE]` / `-- [RISKY]` /
  `-- [DESTRUCTIVE]` por change. Política:
  - **Destructive**: `DropTable`, `DropColumn`
  - **Risky**: `AddColumn NOT NULL sin default`, `AlterColumnType`,
    `AlterColumnNullable false`, `AlterColumnDefault`, `DropIndex`,
    `AddCheckConstraint`
  - **Safe**: el resto (CreateTable, CreateIndex, AddForeignKey,
    DropForeignKey, RenameTable/Column, AlterColumnNullable true,
    DropCheckConstraint, AddColumn nullable/con default)
- **A.2 — `ALTER COLUMN TYPE` con `USING` automático**: el SQL emit
  pasa de `ALTER TABLE t ALTER COLUMN c TYPE T;` a
  `ALTER TABLE t ALTER COLUMN c TYPE T USING c::T;`. Postgres acepta
  el cast explicit incluso para auto-castable (`int → bigint`), y es
  required para casts non-auto (`text → int`, `varchar → int`).
  Para casts que `::` no soporta (bytea ↔ text con encoding custom,
  etc.), el user edita el SQL emitido manualmente.
- **A.5 — `ALTER TABLE ADD/DROP CONSTRAINT` para CHECKs via diff**:
  nuevas variantes `Change::AddCheckConstraint` y
  `Change::DropCheckConstraint`. `diff_check_constraints()` compara
  `current.check_constraints` vs `target.check_constraints` por
  `name`; mismo name + expr distinto → DROP + ADD. Habilita la
  evolución de `@check_constraint("...")` sin recrear la tabla —
  drift detect completo en combinación con A.7.

### Bloque 2 — Transacciones avanzadas (A.4 + A.9 + A.3)

- **A.4 — Nested transactions vía SAVEPOINT**: `db.transaction(fn(tx)
  { ... tx.transaction(fn(inner) { ... }) ... })` ahora funciona
  correctamente. `DbConnHandle` suma `tx_depth: Arc<AtomicI32>`
  shared entre outer y todos los handles de sub-pool. La outer tx
  (depth=0) emite `BEGIN/COMMIT/ROLLBACK`; las nested (depth>0)
  emiten `SAVEPOINT fitz_sp_<N>/RELEASE SAVEPOINT/ROLLBACK TO
  SAVEPOINT`. Inner Err deja el outer intacto (rollback parcial).
- **A.9 — Isolation levels custom**: `db.transaction(closure,
  isolation="SERIALIZABLE")` (kwarg). Whitelist defensiva con 4
  niveles ANSI (`READ UNCOMMITTED` / `READ COMMITTED` /
  `REPEATABLE READ` / `SERIALIZABLE`) opcionalmente combinados con
  `READ ONLY` / `READ WRITE` (`"SERIALIZABLE READ ONLY"`). Outer
  tx emite `BEGIN ISOLATION LEVEL <...>`. Nested ignora el kwarg
  (Postgres no permite ISOLATION en SAVEPOINT — el nivel lo fija
  el outer BEGIN). Nuevo public method
  `transaction_with_isolation(Option<&str>, closure)` en
  `DbConnHandle` (call directo en Rust para tests).
- **A.3 — `db.connect(url, max_conns=N)` kwarg**: pool size opt-in
  del lado del lenguaje (antes solo via env var
  `FITZ_DB_MAX_CONNS`). Validación `1 ≤ N ≤ 1000` con error claro.
  Implementado vía override de la env var antes del connect (deuda
  menor: si un connect previo ya cacheó `max_conns`, el override
  no aplica — documentado).

### Bloque 3 — FK + Drift completo (A.6 + A.7 + A.8)

- **A.6 — FK composite PK del target con error claro**: antes de
  v0.10.31, `@belongs_to user_id: Int` apuntando a un `@table` con
  composite PK hacía fallback silencioso a `"id"` (típicamente no
  existente) → error críptico de Postgres en `fitz db migrate`. Ahora
  `schema_from_program` aborta con mensaje específico citando los
  fields de la composite PK + sugiriendo workarounds (declarar
  UNIQUE constraint single-column en el target, o usar single PK
  surrogate). El sub-paso `refs=` para single-FK explícito queda
  como deuda menor.
- **A.7 — Drift de `@check_constraint` (introspect lee
  `pg_constraint.contype='c'`)**: nueva fn
  `introspect_check_constraints()` que pulla desde `pg_constraint`
  con `pg_get_constraintdef(con.oid)` y canonicaliza la expr via
  `parse_check_def()` (recorta `CHECK ` + paréntesis externos
  balanceados — PG a veces emite 1 o 2 niveles). El diff ahora
  detecta cambios reales del expr y DROP CHECK funciona end-to-end.
- **A.8 — Drift cross-schema FK (introspect popula
  `references_schema`)**: el SQL del FK introspect pulla también
  `ccu.table_schema AS ref_schema`. Si el ref_schema difiere del
  schema local → `references_schema = Some(...)`; mismo schema →
  `None` (paridad con la convención de `schema_from_program`).
  Habilita drift end-to-end para FKs declarados con
  `@belongs_to("schema.User")` cross-schema.

### Decisiones técnicas

- **Severity opinionada, conservadora**: `DropIndex` es Risky (no
  hay pérdida de data, pero performance impact); `DropForeignKey`
  es Safe (solo remueve constraint). Refinable si entra presión.
- **USING `col::T` siempre**: en lugar de detectar casos que no
  necesitan USING, lo emitimos siempre. Postgres es permisivo con
  el cast redundante. Beneficio: menos código + mensajes de error
  más informativos en runtime.
- **Composite PK FK error claro vs. fallback**: la antigua semántica
  de fallback a `"id"` ocultaba el problema hasta el último momento.
  Mejor abortar al `schema_from_program` con mensaje específico.
- **Severity bloquea solo Destructive**: Risky se reporta como
  warning pero no bloquea. La razón: Risky cubre cambios que el user
  típicamente QUIERE hacer (`ALTER TYPE`), solo necesitan revisión.
  Destructive es la línea roja real (data loss garantizada).
- **`parse_check_def` con balance check**: detecta cuando los
  paréntesis externos NO son envolventes (`(a) AND (b)`) y NO los
  recorta. Esto evita corromper exprs composite donde el primer
  `(` cierra en posición interna.
- **Whitelist de isolation levels**: 4 ANSI x opcional READ ONLY/
  WRITE = 12 strings válidos. Rechaza otros con error claro. Más
  estricto que dejar pasar y que Postgres responda con error.

### Tests

- **6 E2E reales contra Postgres** en `tests/db_real_postgres.rs`
  (`#[ignore]` por default, corren con `FITZ_TEST_PG_URL +
  --ignored`): A.4 SAVEPOINT inner rollback + inner commit (2),
  A.9 SERIALIZABLE + READ COMMITTED/REPEATABLE READ (2), A.7
  introspect CHECK constraint, A.8 introspect cross-schema FK.
  Todos verdes contra `postgres:16` local + dev.
- **0 unit nuevos directos** — los helpers nuevos
  (`severity()`/`count_by_severity`/`changes_to_sql_with_severity`/
  `parse_check_def`/`dispatch_builtin_kwargs`) se cubren vía los
  E2E que ejercitan todo el path.
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos).
- Clippy `--all-targets -D warnings` + `--features lsp` limpios.
  fmt `--all --check` limpio.

### Total al cierre v0.10.31

**2739 unit + 292 smoke + 3 openapi + 81 cli_e2e + 324 compile_e2e
+ 58 db_real_postgres** (+6 nuevos del Tier A).

### Próximo norte

**Tier C** (operadores SQL faltantes — ts_rank, expression indexes,
JSON `||` merge — ~12-20h) o **Tier D** (DX/LSP residual del ORM
— completion en `.where(...)`, hover sobre `@table` — ~5h, quick
win). Tier E es visión a futuro (cada ítem mini-fase dedicada).

## [v0.10.30] — 2026-05-31 — Tier B del cierre ORM/DB: API completion Date/DateTime/Uuid

**Tier B entero cerrado en bloque (~12-16h estimadas, ~6h reales)**.
7 sub-pasos coordinados que llevan los tipos nativos
`Date`/`DateTime`/`Uuid` del estado "funcionales con getters" a
"API completa con aritmética, diff, comparison y timezone display".
Sin sintaxis nueva del lenguaje (todos métodos sobre tipos
existentes), zero deps user-facing nuevas (chrono-tz + feature
`uuid/v7` ya internos al binario), paridad bit-a-bit `fitz run` ↔
`fitz build` para los 7 sub-pasos validada con 10 E2E nuevos.
Ningún breaking en los 292 ejemplos del smoke `GUIDE_EXAMPLES_COMPILE`.

### Sub-paso B.1 — Aritmética add_* sobre Date y DateTime

- **Date**: `.add_days(n)`, `.add_months(n)`, `.add_years(n)`
- **DateTime**: `.add_seconds(n)`, `.add_minutes(n)`, `.add_hours(n)`,
  `.add_days(n)`, `.add_months(n)`, `.add_years(n)`

`n: Int` signed (negativos OK). Sub-second units van vía
`chrono::Duration::seconds(n * factor)`; calendar units (months/years)
preservan day-of-month con clamping (`Jan 31 + 1 mes → Feb 28/29`)
vía `chrono::Months` + `checked_add_months`/`checked_sub_months`.
Overflow del rango interno (NaiveDate ±262143 / DateTime ±i64 secs)
emite `FitzError` claro citando el método + el valor que rompió;
en codegen panic con mismo formato.

### Sub-paso B.2 — Subtract symmetric

Aliases con negate runtime:
- **Date**: `.subtract_days/months/years(n)` ≡ `.add_*(-n)`
- **DateTime**: `.subtract_seconds/minutes/hours/days/months/years(n)`

Misma semántica que B.1 con `n` invertido vía `checked_neg`
(`i64::MIN` sin opuesto → error claro). Útil para legibilidad cuando
`n` es un valor literal positivo (`d.subtract_days(7)` lee mejor que
`d.add_days(-7)`).

### Sub-paso B.3 — Diff entre fechas (signed Int)

- **Date**: `d1.diff_days(d2)` → `Int` días entre d1 y d2 (negativo
  si d2 posterior a d1)
- **DateTime**: `.diff_seconds/minutes/hours/days(other)` con
  truncamiento hacia 0 para unidades > 1 segundo (paralelo a
  `Duration::num_seconds() / factor`)

Patrón `dt2.diff_seconds(dt1)` se mapea a `dt2.signed_duration_since(dt1).num_seconds()`.

### Sub-paso B.4 — Comparison operators `<` `>` `<=` `>=` Date/DateTime

`chrono::NaiveDate` y `chrono::DateTime<Utc>` impl `Ord` nativo →
mapping directo a los operadores Fitz. El checker suma
`(Date, Date) | (DateTime, DateTime)` a las parejas permitidas
(antes solo numéricos y Str), el evaluator suma dos arms en `compare()`,
codegen emite `({lhs} < {rhs})` literal sin coerción. Workaround viejo
`d1.timestamp() < d2.timestamp()` ya no necesario.

### Sub-paso B.5 — `Uuid.v7()` time-ordered UUIDs

`Uuid.v7()` constructor estático sobre el módulo `Uuid`. UUIDv7
(RFC 9562, mayo 2024) codifica Unix millis en los primeros 48 bits
→ ordenan cronológicamente en btree indexes, muy útil para PKs
sortables por created_at (vs v4 random que produce index scattering).
Implementado vía `uuid::Uuid::now_v7()` con feature `uuid/v7`
añadida al `Cargo.toml` del binario y al Cargo.toml emitido por
`fitz build`.

### Sub-paso B.6 — Shortcuts

- `Date.tomorrow()` ≡ `Date.today().add_days(1)`
- `Date.yesterday()` ≡ `Date.today().add_days(-1)`
- `DateTime.epoch()` ≡ `DateTime.from_timestamp(0).unwrap()`
  (1970-01-01T00:00:00Z)

Cubren patrones cortos sin necesidad de armar el chain
manualmente.

### Sub-paso B.7 — Timezone display (chrono-tz + IANA)

- `DateTime.to_local()` → `Str`: formatea el instante UTC en la
  zona local del sistema como ISO 8601 con offset
  (`%Y-%m-%dT%H:%M:%S%:z`). Sin deps extras (`chrono::Local` ya
  viene activo via feature `clock`).
- `DateTime.in_tz(name: Str)` → `Result<Str>`: formatea en cualquier
  IANA timezone name (`"America/Argentina/Buenos_Aires"`,
  `"Europe/Paris"`, `"UTC"`, etc.). Name desconocido → `Err(Str)`
  con sugerencia de ejemplos.

**El instante UTC interno NO cambia** — son helpers de display, no
aritmética. Dep nueva `chrono-tz = "0.10"` (sin features extras,
~250KB compiled-in con la DB IANA completa); paralela en el binario
y en el Cargo.toml emitido por `fitz build` (sumado al bloque
`uses_date_or_uuid`).

### Decisiones técnicas

- **B.7 IANA names sobre enum built-in**: el caso real "convertir
  DB-UTC al huso del user" requiere IANA strings (`"America/...",
  `"Europe/..."`); enum dedicado quedaba expresivamente corto.
  `chrono-tz` pesa ~250KB, costo aceptable.
- **B.4 chrono nativo `Ord`**: en lugar de añadir un caso especial al
  evaluator `compare()` que parsee fechas a Int, reusamos el
  `PartialOrd` de chrono. Performance y semántica idénticas al
  approach manual con menos código.
- **`add_years` = `add_months * 12`**: simplifica al reusar
  `checked_add_months`/`checked_sub_months` (chrono no expone
  `add_years` directo). Trade-off menor: el mensaje de overflow cita
  `add_months` con el N pre-escalado (`add_years(100M)` →
  `add_months(1.2B)`). Refinable pasando el método como param si entra
  presión.
- **Negativos como `add_*(-n)` runtime**: `subtract_*` no son alias
  léxicos del parser sino dispatchers separados que negan el arg.
  Coste: dos arms más en el match (~30 LoC). Benefit: el método
  citado en error siempre coincide con el que llamó el user
  (`subtract_days` reporta `subtract_days`, no `add_days`).
- **`?` requiere fn-Result wrapper**: las nuevas API constructoras
  que retornan Result (`Date.from_ymd`, `DateTime.from_timestamp`,
  `Uuid.parse`) se usan con `?` adentro de una fn `-> Result<T>`,
  consistente con el resto del lenguaje (deuda menor: el codegen
  no acepta `?` top-level aunque el intérprete sí — paralelo al
  resto de programas del proyecto).

### Tests

- 10 E2E nuevos en `tests/compile_e2e.rs` cubriendo paridad bit-a-bit
  `fitz run` ↔ `fitz build` para cada sub-paso B.1-B.7 + 1 runtime
  overflow + 1 checker rejection (acumulado: 81 cli_e2e + **324
  compile_e2e**).
- 0 unit nuevos directos (los helpers `date_add_days`/`date_add_months`/
  `datetime_add_duration`/`datetime_diff`/`datetime_in_tz` se cubren
  vía los E2E que ejercitan todo el path eval → dispatch_method →
  helper).
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos).
- Clippy `--all-targets -D warnings` limpio. Clippy `--features lsp`
  limpio. fmt `--all --check` limpio.

### Total al cierre v0.10.30

**2739 unit + 292 smoke + 3 openapi + 81 cli_e2e + 324 compile_e2e
+ 52 db_real_postgres** (Tier B no toca DB — sin cambios en ese
test set).

### Próximo norte

**Tier A** (cierre MVP fuerte del ORM): `fitz db diff
--check-destructive`, `ALTER COLUMN TYPE` con `USING` automático,
`db.connect(url, max_conns=N)` kwarg, savepoints / nested
transactions, `ALTER TABLE ADD/DROP CONSTRAINT` para CHECKs, FK
targeting composite PK, drift check de `@check_constraint` +
cross-schema FK, isolation levels, `FITZ_DB_*` mid-run reload.
Estimado ~30-40h (10 ítems independientes). Detalle en
`docs/deudas-post-5b.md` → sección Tier A.

## [v0.10.29] — 2026-05-31 — Cierre masivo del ORM: JSON path + text search + @unique/@check + cross-schema FK + 6 cierres residuales más

**Release dedicado al cierre masivo de deudas residuales del ORM**.
12 features nuevas + 1 skip deliberado en bloque que llevan el ORM
de "funcional con caveats" a "completo + observable + ergonómico"
para el caso de uso real de aplicaciones full-stack contra
Postgres. Sin sintaxis nueva del lenguaje (la mayoría son
extensiones de decoradores y métodos existentes), zero deps
externas adicionales, paridad bit-a-bit `fitz run` ↔ `fitz build`
mantenida. Ningún breaking change para los 292 ejemplos del smoke
(`GUIDE_EXAMPLES_COMPILE` verde end-to-end).

### Sub-paso 1 — JSON path operators (nested + cast tipado)

Cinco method calls nuevos sobre fields jsonb (`Map<Str, ...>`) en
closures de `.where(...)`. Cierran el agujero del `.get("k")`
single-level habilitando acceso a paths anidados con cast tipado:

- `e.data.has_path([k1, k2, ...])` → `"data" #> $N::text[] IS NOT NULL`
- `e.data.path_text([k1, k2, ...])` → `("data" #>> $N::text[])`
- `e.data.path_int([k1, k2, ...])` → `(("data" #>> $N::text[])::bigint)`
- `e.data.path_float([k1, k2, ...])` → `(("data" #>> $N::text[])::float8)`
- `e.data.path_bool([k1, k2, ...])` → `(("data" #>> $N::text[])::boolean)`

Filtros tipados al estilo `e.data.path_int(["user", "id"]) == 5`
reemplazan el workaround de `db.query(...)` con cast crudo.

### Sub-paso 2 — Full-text search via `@@`

Dos method calls sobre fields `Str` (típicamente columna tsvector
via `@column(sql_type="tsvector")`):

- `body_tsv.matches("query")` → `"body_tsv" @@ to_tsquery($1)` (syntax avanzada)
- `body_tsv.plainto_matches(input)` → `"body_tsv" @@ plainto_tsquery($1)` (search bar libre)

Combinable con `@index(body_tsv, using="gin")` v0.10.28 para
performance de full-text search end-to-end sin bajar a SQL crudo.

### Sub-paso 3 — `@unique(col1, col2, ...)` composite shortcut

Decorator type-level nuevo, alias ergonómico de
`@index(unique=true)`. Acepta bare idents o Str con commas. Solo
soporta `name="..."` como kwarg (para `where_=`/`using=` usar
`@index(...)` directo). Apilable.

```fitz
@table("users")
@unique(email, tenant_id)
@unique(slug, name="users_slug_unique")
type User { ... }
```

### Sub-paso 4 — `@check_constraint("expr", name="optional")` decorator

Emite `CHECK (<expr>)` en `CREATE TABLE`. La expresión se pasa
literal al SQL. Apilable. Auto-naming `chk_<table>_<idx>`.

```fitz
@table("users")
@check_constraint("age >= 0 AND age <= 150")
@check_constraint("status IN ('active', 'pending', 'deleted')")
type User { ... }
```

Limitación MVP: sin drift check (introspect no lee
`pg_constraint.contype = 'c'`), sin diff automático de cambios.
Workaround: `db.exec("ALTER TABLE ... DROP/ADD CONSTRAINT")` o
recrear la tabla con `name=` distinto.

### Sub-paso 5 — Cross-schema FK transparente

Cuando un type referencia con `@belongs_to("User")` un type que
vive en un schema distinto al actual, el FK SQL emit usa
`REFERENCES "schema"."table"(col)` qualified automáticamente.
**Sin cambio de sintaxis** — Fitz resuelve el schema desde el
`@table` del target.

```fitz
@table("public.users") type User { ... }
@table("tenants.memberships") type Membership {
  @belongs_to("User") user_id: Int   // FK cross-schema transparente
  ...
}
// Emite: REFERENCES "public"."users" ("id")
```

Same-schema → SQL sin qualifier (compat con boilerplates que
asumen `public`).

### Sub-paso 6 — Diff completo de indexes

El migrator detecta cambios en `using` / `where_clause` / `unique`
/ `columns` cuando los nombres matchean, emitiendo `DROP INDEX +
CREATE INDEX` para regenerar con el shape nuevo. Antes era
name-based puro y el user tenía que renombrar el índice para
forzar regen. El comparator de `where_clause` normaliza whitespace
+ case para evitar regens espurios; `using` trata `None` y
`Some("btree")` como equivalentes.

### Sub-paso 7 — `fitz db inspect --all-schemas`

Flag nuevo para listar TODOS los schemas user-defined a la vez
(incluyendo `public`), agrupados con su propia sub-vista.
Mutuamente excluyente con `--schema`. Combinable con `--table X`
para filtrar un nombre puntual en todos los schemas. JSON shape:
`{"schemas": [{"schema": "ops", "tables": [...]}, ...]}` con sort
alfabético determinístico.

### Sub-paso 8 — Redaction de secrets en `FITZ_DB_LOG=verbose`

Los params correspondientes a campos sensibles (`password`/
`passwd`/`passphrase`/`secret`/`api_key`/`apikey`/`api_token`/
`auth_token`/`access_token`/`refresh_token`/`id_token`/
`private_key`/`privkey`/`credential`/`session_key`/`session_token`/
`csrf_token`) se enmascaran automáticamente como `<redacted>` en
el output verbose. Heurística best-effort: mira ~50 chars antes
del placeholder, descarta matches separados por `WHERE`/`AND`/
`OR`/etc. Sobre-redacta en bordes ambiguos por seguridad.

### Sub-paso 9 — DB errors enriquecidos con SQLSTATE + SQL + params

`DbError::Server` Display ahora muestra `<severity> [<SQLSTATE>]: <msg>`.
Las queries que fallan pasan por `enrich_db_error_with_context`
que suma `[sql: <query truncado> params=[...]]` con la misma
redaction de secrets que `FITZ_DB_LOG=verbose`.

Antes:
```
ERROR: duplicate key value violates unique constraint "users_email_key"
```

Después:
```
ERROR [23505]: duplicate key value violates unique constraint "users_email_key"
    [sql: INSERT INTO users (email, password) VALUES ($1, $2)
     params=[$1="ada@x.com", $2=<redacted>]]
```

### Sub-paso 10 — `FITZ_DB_MAX_CONNS` pool tuning

Env var opt-in para overridear el pool size del driver. Default
10 conexiones simultáneas máximas por URL. Clamp `[1, 200]`.
Aplica global al proceso (no per URL). Útil para apps con mucho
concurrent load (`FITZ_DB_MAX_CONNS=50`) o cron jobs con poco load
(`FITZ_DB_MAX_CONNS=3`). Kwarg dedicado del lenguaje
(`db.connect(url, max_conns=N)`) queda como deuda menor.

### Sub-paso 11 — Skip deliberado: JSON `||` merge

Decisión documentada: el operador `||` jsonb (typical UPDATE `SET
data = data || $1`) NO se modela en `.where(...)` (read-only).
Caso de uso dominante cubierto por escape hatch:

```fitz
db.exec(
    "UPDATE foo SET data = data || $1::jsonb WHERE id = $2",
    [patch_json, id]
).await?
```

### Tests

- **+39 unit tests nuevos**: 17 evaluator (9 path methods + 3
  matches + 6 @unique + helpers), 13 codegen (7 path codegen
  paralelo + 2 matches codegen + 4 SQL emit), 17 migrations (5
  diff indexes + 4 all-schemas + 6 @check + 2 cross-schema FK),
  17 db (6 redaction parsing + 4 enrichment + 2 SQLSTATE Display
  + 2 max_conns parser + 3 misc), 6 types (@check_constraint
  decorator), 6 types (@unique decorator).
- **1 E2E nuevo en `tests/db_real_postgres.rs`** contra Postgres
  real: `orm_jsonb_path_operators_in_where_paridad_codegen_e2e`
  (paridad bit-a-bit `fitz run` ↔ `fitz build` con table jsonb +
  seed via `seed.exec` con literales nested + queries con los 5
  path methods).

Al cierre: **2739 unit + 292 smoke + 3 openapi + 81 cli_e2e + 52
db_real_postgres** (51 viejos + 1 nuevo). `cargo fmt --all --
--check` + `cargo clippy --all-targets -- -D warnings` + `cargo
clippy --lib --features lsp -- -D warnings` todos limpios.

### Cross-impact

- `editors/vscode/package.json` bump 0.10.28 → 0.10.29.
- `src/lsp.rs` descripción de `@unique` actualizada (single col
  field-level + composite type-level v0.10.29) + nuevo entry
  `@check_constraint` con snippet `@check_constraint("${1:expr}")`.
- `docs/db-orm.md`: bloques nuevos para JSON path operators (sec
  13), full-text search `@@` (sec 13 sub-bloque), `@unique`
  composite + `@check_constraint` + cross-schema FK (sec 4 sub-
  bloques), redaction de secrets en `FITZ_DB_LOG` (sec 29),
  `FITZ_DB_MAX_CONNS` pool tuning (sec 29), DB errors con SQL
  contexto (sec 29).
- `docs/guide.md` cap 31 (Postgres + ORM): bullet "Cierre masivo
  de v0.10.29 — ORM completo" con todos los items + cap 32 (env
  vars) sumando `FITZ_DB_MAX_CONNS` paralelo a `FITZ_DB_LOG`.
- `README.md` Estado del proyecto: bullet de v0.10.29 con todos
  los items.
- `docs/architecture.md`: conteo de sub-comandos actualizado (15
  → 27 efectivos), Familia 5 DB nueva con los 10 sub-comandos
  `fitz db ...` documentados, secciones nuevas para `db.rs`
  (driver Postgres puro) y `migrations.rs` (schema diff +
  introspect + DDL emit), diagramas mermaid + ASCII con path
  `fitz db ...` → `migrations.rs` → `db.rs` → Postgres.

### Deuda residual derivada (NO bloquea uso real)

- `@check_constraint` sin drift check del migrator (introspect no
  lee `pg_constraint.contype = 'c'`). Workaround: drop + recreate
  manual via `db.exec`.
- Cross-schema FK no popula `references_schema` desde la
  introspect (deja siempre `None`), por lo que el drift no detecta
  cambios cross-schema off-Fitz.
- Chain estilo `e.data.get("a").get("b")` (azúcar sobre
  `path_text(["a", "b"])`) sigue como deuda menor.
- JSON `||` merge en `.where(...)` (skipeado deliberadamente —
  caso UPDATE cubierto por escape hatch).
- Ranking full-text (`ts_rank`) — bajar a `db.query` con `ORDER
  BY ts_rank(...)`.
- `db.connect(url, max_conns=N)` kwarg del lenguaje (hoy via env
  var). Requiere wire del kwarg desde evaluator + codegen.
- Cambios mid-run de `FITZ_DB_MAX_CONNS` NO se reflejan (LazyLock
  igual que `FITZ_DB_LOG`). Workaround: reiniciar el proceso.

## [v0.10.28] — 2026-05-31 — Tier S del ORM: introspect + @index using + DB log + HTTP access log

Cierre del **Tier S del ORM**: 4 sub-pasos coordinados que cierran
el ORM como herramienta operativa + observable end-to-end. Sin
sintaxis nueva del lenguaje; tres features nuevas (sub-paso 1
subcomando CLI, sub-paso 2 kwarg nuevo de un decorator existente,
sub-pasos 3/4 env vars opt-in) que cubren el gap entre "tengo el
ORM funcionando" y "tengo el ORM funcionando + sé qué está
pasando + puedo auditar la DB sin abrir psql".

### Sub-paso 1 — `fitz db inspect` (introspect del schema real)

Subcomando nuevo `fitz db inspect [--url URL] [--schema name]
[--table name] [--json]` que se conecta a Postgres y emite una
vista legible del schema actual (tables, columnas con tipos +
nullability + defaults, primary keys, indexes con WHERE de partial,
foreign keys con ON DELETE). Sin tocar tu programa Fitz — pura
introspección. Útil para auditar antes de migrar, descubrir tables
legacy, comparar dev vs prod, o generar reportes machine-readable
con `--json` (shape lockeada, parseable por scripts externos).

Implementación: ensamblar + formatear sobre los helpers existentes
(`introspect_columns`/`introspect_indexes`/`introspect_foreign_keys`/
`list_user_tables_qualified`) — la query infra ya estaba lista
desde v0.10.16. Nuevas APIs públicas `migrations::format_inspection_text`
y `migrations::format_inspection_json`; filtrado in-memory post-
introspect según `--schema` y `--table`.

### Sub-paso 2 — `@index(col, using="gin")` method override

El decorator `@index(...)` a nivel **type** acepta el kwarg nuevo
`using=<method>` con whitelist Postgres oficial: `btree` (default,
no se emite `USING` redundante), `hash`, `gin`, `gist`, `brin`,
`spgist`. Habilita full-text search (`gin` sobre tsvector), range
queries (`gist`), large time-series resumidas (`brin`) sin tener
que bajar a `db.exec("CREATE INDEX ... USING gin")`.

```fitz
@table("docs")
@index(body_tsv, using="gin")
@index(price_range, using="gist")
@index(created_at, using="brin")
type Doc { ... }
```

Implementación: `IndexSpec.using: Option<String>` + `Index.using`
en migrations + processor del kwarg con whitelist + propagación
end-to-end (resolved_indexes → schema_from_program → CREATE INDEX
SQL emit + introspect via `pg_am.amname` para round-trip + format
text/json para que aparezca en `fitz db inspect`). Method
inválido → error claro del checker en compile-time citando los
soportados. Field-level `@index` (sobre un field individual) se
mantiene SIN args (default btree, mismo comportamiento) — el
`using=` solo aplica a nivel type.

**Limitación heredada**: diff name-based — si cambiás SOLO el
`using=` con mismo nombre + cols, el migrator NO detecta el
cambio. Workaround: pasar `name=` distinto para forzar regen.
Mismo patrón que `where_clause` desde v0.10.27.

### Sub-paso 3 — `FITZ_DB_LOG` (query logging del driver)

Env var opt-in que loguea cada query del driver Postgres a
stderr post-ejecución. Zero overhead si no está seteada (single
atomic load + match al inicio de cada call).

- `FITZ_DB_LOG=1` o `=true` → mode Simple: `[fitz-db Nms] <sql>`.
- `FITZ_DB_LOG=verbose` → además params: `params=[$1="ada", $2=42]`
  (strings truncados a 80 chars con `…` final por seguridad — no
  se vuelca un BLOB entero al log).
- Vacío / `=0` / no seteado → Off, silencio total.

Hook en `DbConnHandle::query` (punto único — `exec` delega ahí).
SQL multi-línea se colapsa a una sola línea para grep. Loguea
también las queries que fallan. Cubre tanto `fitz run` como el
binario producido por `fitz build` (paridad bit-a-bit gratis —
mismo crate `fitz::db` via `pub use`). Validado end-to-end
contra Postgres local.

### Sub-paso 4 — `FITZ_HTTP_LOG` (access log estilo uvicorn)

Pieza paralela a `FITZ_DB_LOG` para el stack HTTP. Loguea per-
request a stderr con method + path + status + elapsed.

- `FITZ_HTTP_LOG=1` o `=true` → mode Simple: `[fitz HTTP Nms]
  GET /users/42 → 200`.
- `FITZ_HTTP_LOG=verbose` → además `(UA="curl/8.0" len=1234)`.
- Vacío / `=0` / no seteado → Off, el layer middleware ni se
  monta (literalmente zero overhead, no la indirection del wrapper).

Implementación: `axum::middleware::from_fn(http_log_layer)`
montado condicionalmente sobre el `Router` al final de
`build_router_with_asyncapi` cuando `HTTP_LOG_MODE != Off`. Cubre
**todas** las requests que pasan por el router: handlers
matcheados, preflight OPTIONS de CORS, rutas auto `/openapi.json`/
`/docs`/`/asyncapi.json`, WebSocket handshake (loguea como 101
Switching Protocols), y respuestas 401/403/400/500 de auth/
middleware/handler.

Paridad bit-a-bit codegen: el binario producido por `fitz build`
reusa el mismo `src/http.rs` via `fitz::http` re-export — el
hook + LazyLock se heredan automáticamente sin wiring extra.

### Tests

- **+19 unit tests nuevos**: 5 de migrations (`create_index_con_using`/
  `create_index_sin_using`/`create_index_combina_unique_using_y_where`/
  `format_inspection_text_muestra_using`/`format_inspection_json_incluye_using`),
  más 5 de migrations sobre el formatter base (`format_inspection_text_*`
  + `format_inspection_json_*`), 3 de types (`checker_at_index_using_*`),
  7 de db (`format_db_log_line_*` + `truncate_for_log_utf8_safe`), 6 de
  http (`format_http_log_line_*`).
- **2 E2E nuevos en `tests/db_real_postgres.rs`** contra Postgres
  real: `inspect_schema_text_y_json_contienen_todo_el_shape` (PK
  + partial unique index + FK CASCADE round-trip), y
  `introspect_y_diff_round_trip_using_gin_method` (CREATE INDEX
  USING gin aplicado + introspect lo devuelve correctamente).

Al cierre: **2677 unit + smoke 325** (sin cambios — los ejemplos
existentes siguen pasando bit-a-bit) **+ 51 db_real_postgres**
(49 viejos + 2 nuevos). `cargo fmt --all -- --check` + `cargo
clippy --all-targets -- -D warnings` + `cargo clippy --lib
--features lsp -- -D warnings` todos limpios.

### Cross-impact

- `editors/vscode/package.json` bump 0.10.27 → 0.10.28.
- `src/lsp.rs` descripción de `@index` suma `using=` con
  whitelist al hover/completion.
- `docs/db-orm.md` sec 4 (`@index`) suma bullet `using=` con
  ejemplos canonicales; sec 29 (CLI con DB) suma bloque
  dedicado a `fitz db inspect` con vista texto + JSON shape +
  notas; sec nueva sobre `FITZ_DB_LOG` con formato + ejemplos.
- `docs/guide.md` cap 31 (Postgres + ORM) suma bullet "Tier S"
  con los 3 features visibles; cap 32 (env vars) suma sub-sección
  "Observabilidad — `FITZ_DB_LOG` y `FITZ_HTTP_LOG`" con
  formato + ejemplos + dónde aplica.

### Deuda residual derivada (NO bloquea uso real)

- Diff de indexes name-based no detecta cambios SOLO en
  `where_clause` ni en `using` cuando nombre y cols son iguales.
  Workaround documentado: `name=` distinto para forzar regen.
- `fitz db inspect` cross-schema solo muestra el schema pasado
  por `--schema` (default `public`). Listar TODOS los schemas
  user-defined a la vez es trivial — sumar `--all-schemas` si
  aparece demanda.
- `FITZ_DB_LOG=verbose` trunca strings a 80 chars con `…` — sin
  escape de chars no-imprimibles ni redaction de secrets visibles
  en `$1="password_aqui"`. Caveat documentado en `docs/db-orm.md`.
- Cambios mid-run de `FITZ_DB_LOG`/`FITZ_HTTP_LOG` NO se reflejan
  (LazyLock se fija al primer acceso). Workaround: reiniciar el
  proceso.

## [v0.10.27] — 2026-05-30 — Bulk insert + composite PK + @index decorator

Tres features ortogonales del ORM cerradas en bloque: `Type.bulk_insert(
rows, db, batch_size=1000)` con paridad bit-a-bit run↔build, N `@primary`
fields por type (composite PK) con `TableMetadata.primary_fields: Vec<String>`
+ helpers `single_pk()`/`has_pk()`, y `@index(col1, col2, ..., unique=true,
name="...", where_=<expr>)` decorator a nivel type emitido por `fitz db
diff`/`migrate` con auto-naming `idx_<table>_<col1>_<col2>...[_uniq]` y
partial via WHERE clause. Detalles en el commit b07a36d y `docs/roadmap.md`.

## [v0.10.26] — 2026-05-30 — Codegen Date/DateTime/Uuid: paridad bit-a-bit `fitz run` ↔ `fitz build`

Cierre de la deuda comprometida en CHANGELOG v0.10.24 — los 3 tipos
temporales y de identidad ahora compilan a binario nativo con
`fitz build`. **Paridad bit-a-bit completa con `fitz run`**: mismos
constructors, métodos, ORM mapping, driver wire protocol, HTTP body
in/out, migrations, defaults sentinel.

### Cambios codegen (~700 LoC netas)

**Detector + Cargo.toml condicional**:
- `program_uses_date_or_uuid(program)` paralelo a `program_uses_db`,
  walkea AST + TypeExpr buscando `Ident("Date"|"DateTime"|"Uuid")`
  y annotations.
- Transitivo via `LoadedModule.uses_date_or_uuid`.
- `cargo_toml_for` suma `uuid = { version = "1", features = ["v4"] }`
  + `chrono` (si no estaba ya por uses_jobs) al `Cargo.toml` del crate
  generado cuando `uses_date_or_uuid = true`.
- `CodegenCtx.uses_date_or_uuid` propagado para gateo de helpers.

**Tipos + Display**:
- `rust_type_for`: `Type::Date → chrono::NaiveDate`, `Type::DateTime
  → chrono::DateTime<chrono::Utc>`, `Type::Uuid → uuid::Uuid`.
- `show_expr` (str interpolation + print): Display canonical para
  matchear el intérprete bit-a-bit:
  - Date → `d.format("%Y-%m-%d").to_string()`
  - DateTime → `dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()`
    (sin micros — diferente al Display default de chrono).
  - Uuid → `u.to_string()` (canonical hyphenated lowercase).
- `field_eq_expr` + `type_name` + `display_type`: 3 nuevos arms.

**Constructors (9 funcs)**:
`gen_temporal_module_call(recv, field, args)` dispatch paralelo a
`gen_db_module_call`. Cada constructor emite la llamada chrono/uuid
correspondiente, envolviendo en Result cuando puede fallar:
- `Date.today()` → `chrono::Local::now().date_naive()`
- `Date.parse(s)` → `chrono::NaiveDate::parse_from_str(...).map_err(...)`
- `Date.from_ymd(y, m, d)` → `chrono::NaiveDate::from_ymd_opt(...).ok_or_else(...)`
- `DateTime.now()` → `chrono::Utc::now()`
- `DateTime.parse(s)` → `chrono::DateTime::parse_from_rfc3339(...).map(...).map_err(...)`
- `DateTime.from_timestamp(secs)` → `chrono::DateTime::<Utc>::from_timestamp(secs, 0).ok_or_else(...)`
- `Uuid.v4()` → `uuid::Uuid::new_v4()`
- `Uuid.parse(s)` → `uuid::Uuid::parse_str(...).map_err(...)`
- `Uuid.nil()` → `uuid::Uuid::nil()`

**Instance methods (13)**:
Dispatch sobre `Type::Date`/`DateTime`/`Uuid` en `gen_method_call`:
- Date: `year/month/day/weekday/to_str/to_datetime/format` (7).
- DateTime: `year/month/day/hour/minute/second/timestamp/to_str/date/format` (10).
- Uuid: `to_str/is_nil` (2).
- Total: 13 (algunos comparten name pero distinto receiver).

**HTTP JSON ser/de**:
Nueva const `DATE_UUID_HTTP_INTEGRATION_PRELUDE` emitida cuando
`uses_date_or_uuid && has_http` con impls de `__ToFitzJson` y
`__FromFitzJson` para los 3 tipos. JSON shape canonical (JSON Schema
"date"/"date-time"/"uuid" formats). `__FromFitzJson` rechaza con
error claro si el string no parsea (→ 400 Bad Request al cliente
con mensaje específico).

**Driver wire protocol (ORM + raw query)**:
- `emit_date_uuid_db_prelude()` método nuevo del CodegenCtx, emite
  cuando `uses_date_or_uuid && uses_db`:
  - `impl __IntoPgValue for chrono::NaiveDate/DateTime<Utc>/uuid::Uuid`
    (param marshaling: `PgValue::Text` en formato canonical PG).
  - `__fitz_pg_to_date/datetime/uuid(v, col) -> Result<T, String>`
    (row reading: parse de `PgValue::Text` a chrono/uuid).
  - `__fitz_pg_normalize_timestamptz(s)`: paralelo a
    `parse_pg_timestamptz` del evaluator (`YYYY-MM-DD HH:MM:SS±TZ`
    → RFC 3339).
- `orm_marshal_field_to_pg` (INSERT path): nuevos arms para
  Date/DateTime/Uuid via `__IntoPgValue::into_pg(...)`.
- `orm_field_coerce_block` (SELECT path): nuevos arms via
  `__fitz_pg_to_date/datetime/uuid(__v, col)?`.

**Field default sentinel `Str = ""`**:
Cuando un field `Date`/`DateTime`/`Uuid` tiene default `""` (Str
literal sentinel, paralelo a `id: Int = 0`), el codegen emite el
`Default::default()` correspondiente al tipo destino:
- `Date → chrono::NaiveDate::default()` (1970-01-01)
- `DateTime → chrono::DateTime::<chrono::Utc>::default()`
- `Uuid → uuid::Uuid::nil()`

Aplica tanto al path `__from_fitz_json` (None → default) como al path
de fields hidden.

### Cambios complementarios

- **`__fitz_pg_to_date/datetime/uuid` gateados condicionalmente**:
  los helpers se emiten SOLO cuando `uses_date_or_uuid && uses_db`.
  Programas con `@table` que NO usan los 3 tipos no pagan el peso
  de chrono/uuid ni de los helpers. La `use crate::{...}` de los
  módulos no incluye los nuevos helpers en el import condicional
  por defecto (los módulos que los necesiten resuelven via
  inferencia del cross-impl).
- **Error block removido**: el error claro de v0.10.24 que decía
  "Date/DateTime/Uuid no soportado en `fitz build` — sub-paso
  comprometido v0.10.26" se eliminó. Si el user escribe
  `Date`/`DateTime`/`Uuid` solo (sin `.method()`), error nuevo
  citando el patrón canonical de uso (siempre `.method()`).

### Smoke E2E verde

- `examples/guide/31-orm.fitz` (ya usaba Date en field) compila
  ahora con `fitz build` sin error.
- Smoke nuevo: `@table type Event { happens_on: Date, starts_at:
  DateTime, external_id: Uuid }` + INSERT + readback via
  `Event.all(conn)` preserva tipos. Métodos instancia
  (`.year()`, `.hour()`, `.is_nil()`) funcionan sobre la Instance
  recuperada del PG.
- Smoke HTTP body in/out: POST con
  `{"happens_on":"2026-12-25","starts_at":"...","external_id":"..."}`
  → handler recibe `body.happens_on` como `Date`, `body.starts_at`
  como `DateTime`. Date inválida en JSON → 400 con mensaje claro
  citando el formato esperado.
- `fitz db diff` emite `CREATE TABLE ... (happens_on date NOT NULL,
  starts_at timestamptz NOT NULL, external_id uuid NOT NULL)` (ya
  estaba desde v0.10.24 vía `migrations::fitz_typeexpr_to_sql_type`).

### Validación final

- `cargo test --lib`: **2647 verde**.
- `compile_e2e::smoke_ejemplos_guia_compilables` (325 ejemplos):
  verde (incluye 31-orm.fitz que antes fallaba con la deuda).
- `tests::db_real_postgres` (49 ignored): **49/49 verde** contra PG
  local.
- `cargo fmt --all -- --check`: verde.
- `cargo clippy --all-targets -- -D warnings`: verde.
- Smoke real `fitz build` + ejecución contra PG local: 3
  endpoints HTTP retornando Date/DateTime/Uuid + INSERT/SELECT en
  tabla con los 3 tipos + body deserialization con error claro
  para Dates inválidas.

### Cross-impact docs

- `docs/db-orm.md` sec 4 "Mapping de tipos Fitz → Postgres":
  caveat "v0.10.24" reemplazado por "paridad bit-a-bit `fitz run`
  ↔ `fitz build` desde v0.10.26".
- VSCode extension bump 0.10.25 → 0.10.26.

### Deps nuevas

- `uuid` (re-emitido condicionalmente al Cargo.toml del crate
  generado). `chrono` ya estaba (cron jobs); ahora también se
  emite cuando `uses_date_or_uuid && !uses_jobs`.

### Out of scope (deuda residual, sin presión)

- **Aritmética de fechas** (`dt + Duration`): `Duration` es otro
  tipo built-in, mini-fase aparte si entra demanda.
- **Time standalone** (Postgres `time` OID 1083).
- **DateTime con TZ parametrizado** (`DateTime<TZ>`).
- **Métodos extra Uuid**: `version()`, `variant()`, `bytes()`.

## [v0.10.25] — 2026-05-30 — Hotfix v0.10.24: array elem_oid solo refina si caller pidió

Hotfix del CI release v0.10.24 — 33 tests E2E del driver
Postgres fallaron en cascada en GitHub Actions tras el push del
tag v0.10.24, descubierto por el job `db-postgres` del workflow
CI sobre `postgres:16`. Hot-issue resuelto antes de que llegue a
ningún user real.

### Síntoma

Cascada que arrancó en `orm_uuid_array_e2e`:

```
thread 'orm_uuid_array_e2e' panicked at tests/db_real_postgres.rs:4528:26:
    esperaba Str(UUID), fue Uuid(...)
```

Tras ese panic, los 32 tests siguientes fallaron con
`Io(Custom { kind: Other, error: "A Tokio 1.x context was found,
but it is being shutdown." })`. Test runner perdió el runtime
tokio del primer test, y el pool singleton per-URL (desde v0.10.9)
cacheaba un handle con tasks ligadas al runtime cerrado.

### Root cause

En `pg_value_to_fitz_with_oid`, el arm `PgValue::Array` siempre
propagaba `elem_oid` a la recursión sobre items, ignorando el
`oid_hint` del caller:

```rust
// ANTES (bug v0.10.24)
crate::db::PgValue::Array { elem_oid, values } => {
    let items: Vec<Value> = values
        .iter()
        .map(|item| pg_value_to_fitz_with_oid(item, Some(*elem_oid)))
        .collect();
    Value::new_list(items)
}
```

Resultado: `db.query(...)` raw sobre una columna `uuid[]` /
`date[]` / `timestamptz[]` devolvía `List<Uuid>` / `List<Date>` /
`List<DateTime>` en vez de `List<Str>` (comportamiento
pre-v0.10.24). Programas legacy que iteraban con
`match v { Value::Str(s) => ..., _ => panic!() }` quebraban.

### Fix

Array recursion ahora hace `oid_hint.map(|_| *elem_oid)` — solo
propaga `elem_oid` si el caller pasó `Some(_)`:

```rust
// DESPUÉS (fix v0.10.25)
crate::db::PgValue::Array { elem_oid, values } => {
    let elem_hint = oid_hint.map(|_| *elem_oid);
    let items: Vec<Value> = values
        .iter()
        .map(|item| pg_value_to_fitz_with_oid(item, elem_hint))
        .collect();
    Value::new_list(items)
}
```

Si `oid_hint` es `None` (default backward-compat de
`pg_row_to_fitz_map` y de `pg_value_to_fitz`), los elementos
vuelven como `Str`. El path ORM @table-typed (annotation-aware,
ya corregido en commit previo del v0.10.24) sí pasa `Some(_)` y
refina cuando el field declara explícitamente `Date`/`DateTime`/
`Uuid` o `List<T>` con esos tipos.

### Validación

Local smoke contra Postgres real:

```
$ FITZ_TEST_PG_URL="postgres://...@localhost:5432/postgres?sslmode=disable" \
    cargo test --release --test db_real_postgres -- --ignored --test-threads=1
test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
                 ^^^^^^^^^^^^^^^^^^^^^^^ era 16/49 con la cascade
```

- `cargo test --lib`: **2647 verde** (sin cambios).
- `cargo fmt --all -- --check`: verde.
- `cargo clippy --all-targets -- -D warnings`: verde.

### Backward compat preservada

- **Programas pre-v0.10.24** usando `db.query(...)` sobre columnas
  `date`/`timestamptz`/`uuid` (o arrays de esos): siguen recibiendo
  `Str` / `List<Str>` con formato ISO 8601 / canonical Postgres.
- **Programas pre-v0.10.24** usando `@table type X { d: Str }`
  para columnas date/timestamptz/uuid: siguen recibiendo `Str`
  en el field tras `Type.all(db)`.
- **Programas v0.10.24+ opt-in**: declaran `@table type X { d: Date }`
  con anotación explícita → la refinación annotation-aware dispara
  y devuelven `Value::Date` tipado.

### Cero impacto en feature surface

Esta release es PURO hotfix del bug introducido en v0.10.24. Toda
la API user-facing (constructors, métodos, ORM mapping, JSON,
LSP, grammar) queda idéntica a v0.10.24. El extensión VSCode
bump 0.10.24 → 0.10.25 es solo para alinear versiones.

## [v0.10.24] — 2026-05-30 — Date / DateTime / Uuid tipos nativos (intérprete)

Cierre del bloque comprometido post-TLS — los 3 tipos temporales
y de identidad más usados pasan de `Str` ISO 8601 a tipos
built-in con constructors, métodos, integración driver Postgres
y mapping ORM. **Soporte completo en `fitz run`**; `fitz build`
queda como deuda explícita comprometida v0.10.25 (codegen emite
error claro citando el sub-paso).

### Tipos nuevos

| Tipo | Wrapper interno | Postgres | JSON |
|---|---|---|---|
| `Date` | `chrono::NaiveDate` | `date` (OID 1082) | string ISO 8601 `YYYY-MM-DD` |
| `DateTime` | `chrono::DateTime<chrono::Utc>` | `timestamptz` (OID 1184) | string RFC 3339 `YYYY-MM-DDTHH:MM:SSZ` |
| `Uuid` | `uuid::Uuid` | `uuid` (OID 2950) | string canonical `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` |

Decisiones de diseño:
- **Naming**: `Uuid` (consistente con `DbConn`/`DbRow`/`PyAny`).
- **TZ**: `DateTime` siempre UTC en MVP. `DateTime<TZ>` parametrizado
  queda como deuda futura (usado por <5% de apps reales).
- **Sin `Time` standalone**: caso de uso raro, suma deuda futura
  si pide.
- **Sin aritmética** (`dt + Duration`): `Duration` es otro tipo
  built-in que sumamos si entra demanda.

### API user-facing

```fitz
// Constructors estáticos (Value::Module global por tipo)
let today: Date = Date.today()
let dt: DateTime = DateTime.now()
let id: Uuid = Uuid.v4()
let nil_id: Uuid = Uuid.nil()
let d: Date = Date.from_ymd(2026, 12, 25)?
let dt2: DateTime = DateTime.parse("2026-12-25T18:00:00Z")?
let u: Uuid = Uuid.parse("550e8400-e29b-41d4-a716-446655440000")?

// Métodos instancia (dispatch sobre Value::Date/DateTime/Uuid)
print(d.year())              // 2026
print(d.month())             // 12
print(d.weekday())           // 5 (ISO 8601, Friday)
print(d.format("%A %B %d"))  // "Friday December 25" (chrono format)
print(d.to_datetime())       // 2026-12-25T00:00:00Z

print(dt.hour())             // 18
print(dt.timestamp())        // 1766685600 (Unix epoch)
print(dt.date())             // 2026-12-25 (extrae solo la fecha)

print(u.to_str())            // canonical hyphenated
print(u.is_nil())            // false (Uuid.nil() devuelve true)
```

### Integración driver Postgres (D.5)

- `pg_value_to_fitz_with_oid(value, oid_hint)` refina
  `PgValue::Text` a `Value::Date`/`DateTime`/`Uuid` cuando el OID
  identifica `date` (1082) / `timestamptz` (1184) / `timestamp`
  (1114) / `uuid` (2950).
- `parse_pg_timestamptz` normaliza el formato Postgres
  `YYYY-MM-DD HH:MM:SS±TZ` a RFC 3339 (espacio→T, offsets `+00`/
  `+0530` → `+00:00`/`+05:30`).
- `fitz_value_to_pg`: `Value::Date/DateTime/Uuid` → `PgValue::Text`
  en el formato canonical que Postgres acepta via cast implícito.
- `Row::get_with_oid(col)` devuelve `(PgValue, oid)` para que el
  caller pueda hacer la refinación.
- `pg_row_to_instance` y `pg_row_to_fitz_map` propagan el OID al
  converter — el ORM read-back devuelve `Instance` con
  `Value::Date`/`DateTime`/`Uuid` en los fields declarados como
  tales.

### ORM + migrations mapping (D.4 + D.7)

`migrations::fitz_typeexpr_to_sql_type` mapea:
- `Type::Date` → `date`
- `Type::DateTime` → `timestamptz`
- `Type::Uuid` → `uuid`

Habilita el flujo canónico:

```fitz
@table("events") type Event {
    @primary id: Int = 0
    name: Str = ""
    happens_on: Date = ""       // sentinel; user provee Date.from_ymd(...)
    starts_at: DateTime = ""    // idem
    external_id: Uuid = ""      // idem
}
```

El checker acepta `Str` literal como default para fields
Date/DateTime/Uuid (sentinel paralelo a `id: Int = 0`); el
evaluator coerce el `Str` cuando se construye la Instance via
`coerce_to_annotation`.

### JSON serialization (D.6)

- `value_to_json` emite Date/DateTime/Uuid como JSON string
  canonical (estándar de la industria — JSON Schema
  `"date"`/`"date-time"`/`"uuid"` formats).
- `coerce_to_annotation` con annot `Date`/`DateTime`/`Uuid` sobre
  `Value::Str` deserializa al tipo correspondiente. Caso típico:
  HTTP body JSON con `"happens_on": "2026-12-25"` deserializado a
  Instance con `Value::Date` en el field. Errores claros si el
  string no matchea el formato esperado.
- Para `DateTime`, acepta tanto RFC 3339 como el formato Postgres
  timestamptz con espacio.

### LSP + extensión VSCode

- `lsp::scope_level_completions` lista `Date`/`DateTime`/`Uuid`
  como built-in types.
- `lsp::after_dot_completions` dispatches sobre `Type::Date`,
  `Type::DateTime`, `Type::Uuid` con method_items dedicado
  (year/month/.../format/to_str/etc.).
- Grammar TextMate suma `Date`/`DateTime`/`Uuid` a
  `support.type.builtin.fitz`.
- Extensión VSCode bump 0.10.23 → 0.10.24.

### Codegen — deuda explícita v0.10.25

El codegen emite error claro cuando encuentra `Date`/`DateTime`/
`Uuid` en el AST:

```
✗ codegen: Error — `Date` (tipo built-in v0.10.24) todavía no
soportado en `fitz build` — sub-paso comprometido v0.10.25 (deuda
explícita). Usá `fitz run` mientras tanto.
```

Cerrar la deuda v0.10.25 requiere ~500+ LoC adicionales en
codegen: helpers de preludio emitiendo chrono/uuid + dispatch de
cada constructor/método + `__IntoPgValue`/`__FromFitzDbRow` para
los 3 tipos + `__ToFitzJson`/`__FromFitzJson` + Cargo.toml
emisión condicional de `chrono` (ya parte del workspace) y
`uuid` (dep nueva).

### Smoke E2E real Postgres

Validó el ciclo completo: `@table` con Date/DateTime/Uuid fields
→ `Event.insert(conn, row)` con valores `Date.from_ymd(2026, 12, 25)?`/
`DateTime.parse("2026-12-25T18:00:00Z")?`/`Uuid.v4()` → wire
format text round-trip → `Event.all(conn)` → readback preserva
tipos (`.year()`, `.hour()`, `.is_nil()` funcionan sobre la
Instance recuperada).

### Validación final

- `cargo test --lib`: **2647 verde**.
- `compile_e2e::smoke_ejemplos_guia_compilables` (325 ejemplos):
  verde.
- `cargo fmt --all -- --check`: verde.
- `cargo clippy --all-targets -- -D warnings`: verde.

### Deps nuevas

- `uuid = { version = "1", features = ["v4"] }` — generación
  random + parsing. Pure Rust ~50KB.
- `chrono` ya era dep no-opcional desde Fase 9.w.3 (cron jobs);
  reusado sin pulls nuevos.

### Out of scope (deuda explícita v0.10.25+)

- **Codegen completo** para `fitz build`: ~500+ LoC, próximo release.
- **`Duration`** + aritmética (`dt + 1.days()`, `dt2 - dt1`).
- **`Time` standalone** sin fecha (Postgres `time` OID 1083).
- **`DateTime<TZ>`** parametrizado por timezone.
- **Métodos adicionales en `Uuid`** (`version()`, `variant()`,
  `bytes()`).

## [v0.10.23] — 2026-05-30 — Fase 10.1.b: TLS strict para el driver Postgres

Cierre del sub-paso comprometido desde Fase 10.1. El driver ahora
soporta los 4 modos `sslmode` estándar Postgres (`disable`,
`require`, `verify-ca`, `verify-full`) + custom CA via
`sslrootcert=path/to/ca.pem`. **Habilita apuntar el driver Fitz a
managed Postgres real** (Heroku, RDS, Supabase, Neon, Aiven,
Render PG, Crunchy Bridge, etc.) sin downgrade a `sslmode=disable`.

### Modos soportados

| `sslmode` | TLS | Cert chain | Hostname | Cuándo usar |
|---|:---:|:---:|:---:|---|
| `disable`     | ❌ | — | — | Local dev sin TLS (Postgres en Docker localhost) |
| `require`     | ✅ | ❌ | ❌ | Dev/staging contra Postgres interno sin CA pública. NO usar en prod |
| `verify-ca`   | ✅ | ✅ | ❌ | Cert con CN/SAN distinto al hostname (proxies, port forward) |
| `verify-full` | ✅ | ✅ | ✅ | **Recomendado producción** — usado por todos los managed PG |

```fitz
// Managed PG real con TLS strict
let db = db.connect(
    "postgres://user:pass@db.proyecto.supabase.co:5432/postgres?sslmode=verify-full"
).await?

// Postgres interno con CA corporativa custom
let db = db.connect(
    "postgres://user:pass@db.intra:5432/myapp?sslmode=verify-full&sslrootcert=/etc/ssl/corp-ca.pem"
).await?
```

### Implementación

- Deps nuevas (no opcionales, parte del core del driver): `rustls
  0.23` + `tokio-rustls 0.26` + `webpki-roots 0.26` + `rustls-pemfile 2`.
- `rustls` con feature `ring` como crypto provider (puro Rust +
  assembly, **sin deps system tipo CMake/clang/OpenSSL**). Mantiene
  la promesa "binario standalone sin libs system".
- `webpki-roots` trae el **Mozilla CA bundle in-binary** — cubre
  Heroku/RDS/Neon/Aiven/Render/etc. sin que el user instale nada.
- `Connection.stream` migrado de `TcpStream` a `Box<dyn DbReadWrite>`
  con helper trait `DbReadWrite: AsyncRead + AsyncWrite + Send +
  Unpin`. Costo: una vtable lookup por read/write (~3ns), irrelevante
  vs el round-trip TCP. **Sin impacto en el bench v0.10.13** (los
  números B-1 se mantienen).
- `read_message` migrado de hard-coded `TcpStream` a genérico
  `<R: AsyncRead + Unpin>`.
- 3 `ServerCertVerifier` custom:
  - `NoVerifier` (sslmode=require): acepta cualquier cert.
  - `NoHostnameVerifier` (sslmode=verify-ca): wrapper sobre
    `WebPkiServerVerifier` que catchea `CertificateError::NotValidForName`
    (y `NotValidForNameContext` en rustls 0.23+) y lo trata como
    Ok. Mantiene chain validation + skip hostname.
  - Default `WebPkiServerVerifier` (sslmode=verify-full).
- `SSLRequest` dance (8-byte magic 80877103) + server response
  parsing ('S' = TLS supported, 'N' = no TLS, 'E' = error con
  body drenado). Errores específicos según cada caso.
- `ensure_rustls_provider()` instala el `ring` provider de rustls
  via `std::sync::Once` la primera vez que se intenta un TLS upgrade.
- Validación cruzada de combinaciones inválidas en el parser
  (`sslmode=disable&sslrootcert=...`, `sslrootcert=` sin sslmode,
  etc.) — fail-fast con mensaje claro en vez de runtime confuso.

### URL parser

- `SslMode` enum extendido: `Disable` / `Require` / `VerifyCa`
  / `VerifyFull`.
- `ConnectionConfig.sslrootcert: Option<PathBuf>` nuevo.
- `sslmode=prefer|allow` siguen como `NotImplemented` con mensaje
  claro (negociación dinámica con downgrade es vulnerable a MITM;
  los drivers modernos lo desalientan).
- `sslrootcert=` URL-decoded (paths con spaces o caracteres
  especiales funcionan).

### `DbError::Tls` variant nueva

Fallos del path TLS (SSLRequest rechazado, handshake roto, sslrootcert
ilegible/malformado, hostname mismatch en verify-full, etc.) ahora
tienen variant dedicada con Display `"TLS: <msg>"`. Diferencia
limpia de `DbError::Io` (TCP genérico) y `DbError::Auth` (credentials).

### Validación end-to-end contra Supabase real

Smoke E2E corrido contra el pooler de Supabase
(`aws-1-us-west-2.pooler.supabase.com`):

- `sslmode=disable`: SELECT 1 OK ✓
- `sslmode=require`: TLS handshake completo + SELECT 1 OK ✓
- `sslmode=verify-ca`/`verify-full`: UnknownIssuer — el verifier
  funciona correctamente (Supabase pooler usa su propia CA fuera
  del Mozilla bundle). El user puede bajar la CA cert del dashboard
  Supabase y usarla como `sslrootcert=path/to/prod-ca-2021.crt`
  para validación end-to-end.

Para managed PG con cert público (Neon usa Let's Encrypt, RDS usa
Amazon Root CA — ambos en `webpki-roots`), `verify-full` funciona
sin custom CA.

### Tests + suite

- 10 unit tests nuevos en `db::tests`:
  - `url_sslmode_require_parsea_ok`
  - `url_sslmode_verify_ca_parsea_ok`
  - `url_sslmode_verify_full_parsea_ok`
  - `url_sslmode_prefer_sigue_no_implementado`
  - `url_sslmode_allow_sigue_no_implementado`
  - `url_sslmode_desconocido_es_error`
  - `url_sslrootcert_con_verify_ca_parsea_ok`
  - `url_sslrootcert_url_encoded_se_decodifica`
  - `url_sslrootcert_con_sslmode_disable_es_error`
  - `url_sslrootcert_con_sslmode_require_es_error`
  - `url_sslrootcert_sin_sslmode_es_error`
- 1 test refresh en `evaluator::tests` (`db_connect_url_con_sslmode_require_resuelve_y_falla_en_red`):
  antes esperaba `NotImplemented`; ahora verifica que el flow
  llega al I/O step (sslmode=require ya no rechaza early).
- `cargo test --lib`: **2647 verde** (era 2637, +10 parser tests).
- `cargo clippy --all-targets -- -D warnings`: verde.
- `cargo fmt --all -- --check`: verde.

### Cross-impact docs

- `docs/db-orm.md` sección 3: sub-sección nueva "TLS strict
  (v0.10.23)" con tabla de los 4 modos + ejemplos + combinaciones
  inválidas + out of scope (`prefer`/`allow` + client cert auth).
- `docs/guide.md` cap 31: ejemplo del driver `db` muestra dos
  flavors (local sin TLS + managed con verify-full).
- Cargo.toml: 4 deps nuevas con comentario justificando elección
  (`rustls` sobre `native-tls` para mantener "binario standalone
  sin deps system"; `ring` como crypto provider; `webpki-roots`
  in-binary).

## [v0.10.22] — 2026-05-30 — Cierre 2 deudas residuales del codegen del driver DB

Cierra las 2 deudas heredadas que el Boilerplate 10
(`api-multi-tenant`) destapó: queries con shape dinámico
retornadas crudas como JSON desde un handler HTTP, y extracción
tipada de columnas individuales sobre `DbRow` desde `fitz build`
(antes solo intérprete).

### Deuda A — `Result<List<DbRow>>` como retorno de handler HTTP

Los handlers ahora pueden devolver `Result<List<DbRow>>` directo
y el codegen auto-serializa cada row a `{col: val, ...}` en el
JSON response. Útil para queries cuyo shape no se puede
representar como `type` (CTEs, multi-tenant con schema dinámico,
queries ad-hoc retornadas a frontends que aceptan shape libre).

```fitz
@get("/products/dynamic")
async fn products() -> Result<List<DbRow>> {
    let conn = db.connect(db_url).await?
    return conn.query("SELECT id, name FROM acme.products", []).await
}
// HTTP 200 → [{"id":1,"name":"foo"},{"id":2,"name":"bar"}]
```

Implementación: nuevo `DB_HTTP_INTEGRATION_PRELUDE` emitido
condicionalmente cuando `uses_db = true`, con `impl __ToFitzJson
for __fitz_db_runtime::Row` que mapea cada `PgValue` al `Value`
JSON correspondiente (incluye auto-detección de JSON/array
strings como `jsonb` → JSON anidado real, no como string).

### Deuda B — Métodos tipados sobre `DbRow` en codegen

5 métodos nuevos vivos en `fitz build` con paridad bit-a-bit
intérprete↔codegen:

| Método | Retorno | Notas |
|---|---|---|
| `r.get_int(col)`   | `Result<Int>`   | Falla si NULL, no existe, o el tipo PG no es int |
| `r.get_str(col)`   | `Result<Str>`   | Falla si NULL/no existe; acepta text/varchar/uuid/json/etc. |
| `r.get_float(col)` | `Result<Float>` | float8/float4/numeric/etc. |
| `r.get_bool(col)`  | `Result<Bool>`  | bool PG |
| `r.len()`          | `Int`           | número de columnas del row |

```fitz
let rows = conn.query("SELECT id, name FROM users LIMIT 1", []).await?
let r: DbRow = rows[0]
let id: Int    = r.get_int("id")?       // Result<Int>
let name: Str  = r.get_str("name")?     // Result<Str>
```

Sintaxis dedicada (`get_int` en vez de `get` polimórfico) por
elección de diseño — el checker refina el ret type del call al
`Result<T>` correcto sin requerir anotación en la lhs.

### Boilerplate 10 (`api-multi-tenant`) — Enfoque B real, no demo

El handler `GET /products/dynamic` con header `X-Tenant: <slug>`
+ validación whitelist contra `public.tenants` + SQL dinámico
**ahora compila con `fitz build`** y se expone como endpoint
nativo. El frontend `/dynamic.html` deja de ser solo-texto y
suma un selector interactivo con 4 valores demo (acme/beta
válidos, zeta no registrado, SQL injection rechazada por
whitelist) + área de resultado en vivo.

### Tests + smoke + LSP

- 4 unit tests nuevos en `types::tests::checker_db_row_*`
  (Result<Int> / Result<Str> / annotation-mismatch /
  unknown-method).
- 1 unit test nuevo en `lsp::tests::after_dot_sobre_dbrow_*`
  (autocomplete tras `r.` lista get_int/get_str/get_float/
  get_bool/len).
- LSP `Type::DbRow` ahora aparece en `after_dot_completions`
  con method_items dedicado.
- Refresh signature del autocomplete `DbConn.query` (era
  `Result<List<Map>>`, ahora `Result<List<DbRow>>`).
- Smoke .fitz dedicado validado bit-a-bit `fitz run` ↔ `fitz
  build` contra Postgres local (3 endpoints verde).
- Smoke E2E Boilerplate 10 contra Postgres local: 7 endpoints
  verde (3 Enfoque A + 4 Enfoque B incluido el caso injection).
- `GUIDE_EXAMPLES_COMPILE` smoke (325 ejemplos) verde.
- `cargo test --lib`: 2637 verde (sin feature) / 2749 verde
  (con feature `lsp`).
- `cargo fmt --all -- --check` + `cargo clippy --all-targets
  -- -D warnings` verde.

### Cross-impact docs

- `docs/db-orm.md` sección 3: signature de `db.query` corregida
  a `Future<Result<List<DbRow>>>` + sub-sección nueva sobre los
  métodos `r.get_*`.
- `docs/guide.md` cap 31: ejemplo del driver `db` actualizado
  con DbRow + extracción tipada + nota sobre handlers HTTP
  retornando `Result<List<DbRow>>`.
- Boilerplate 10 README: bloque "Enfoque B" reescrito (deja
  de citar deudas residuales) + curl ejemplo del caso injection
  como demo de validación.
- Extension VSCode bump 0.10.21 → 0.10.22 (grammar ya tenía
  `DbRow`; el delta real es LSP completion + signatures).

## [v0.10.21] — 2026-05-30 — Fase 10.6.e.3: schemas custom (cierra Fase 10.6 entera)

Última feature del Tier 2 del plan vs Alembic. **Cierra la Fase
10.6 completa**: el paquete `fitz db ...` cubre ahora migrations
generation + apply/rollback + drift check + stamping + history
+ offline SQL + squash + data migrations en `.fitz` + schemas
custom Postgres. Equivalente funcional a Alembic con cero deps
externas.

### Sintaxis: `@table("schema.name")`

`@table` ahora acepta opcionalmente un nombre de schema separado
por `.`. Sin `.` (compat pre-v0.10.21), schema = `public`
(default Postgres).

```fitz
@table("users") type User {              // public.users (default)
    @primary id: Int = 0
    email: Str = ""
}

@table("analytics.events") type Event {  // analytics.events (custom)
    @primary id: Int = 0
    name: Str = ""
    @db_default("NOW()") at: Str = ""
}
```

Validación del checker: ambos segmentos no-vacíos, sin
whitespace, máximo 1 `.`. Strings inválidos (`""`, `"a.b.c"`,
`"foo bar"`) → error de tipo claro.

### Multi-schema end-to-end

`fitz db check` con `analytics.events` + `users` (mixed):

```sql
CREATE SCHEMA IF NOT EXISTS "analytics";

CREATE TABLE "analytics"."events" (
    "id" bigserial PRIMARY KEY,
    "name" text NOT NULL,
    "at" text NOT NULL DEFAULT NOW()
);

CREATE TABLE "users" (
    "id" bigserial PRIMARY KEY,
    "email" text NOT NULL
);
```

El ORM nativo usa qualified everywhere: `INSERT INTO
"analytics"."events" (...)`, `SELECT ... FROM "analytics"."events"`,
`UPDATE "analytics"."events" SET ... WHERE ...`, etc.

### Casos de uso

- **Multi-tenant via schemas**: `@table("tenant_acme.users")`,
  `@table("tenant_beta.users")` aisla data por cliente.
- **Separación dev/test/staging**: `@table("staging.events")`
  vs `@table("prod.events")` en el mismo cluster.
- **Módulos aislados**: `@table("auth.sessions")`,
  `@table("billing.invoices")`, `@table("analytics.events")`
  para namespacing en monolitos grandes.
- **Naming conflict resolution**: dos modules con tabla
  `events` viven en schemas distintos sin colisión.

### Cambios técnicos

- **src/types.rs**:
  - `TableMetadata.schema: Option<String>` nuevo field. `None`
    = `public`.
  - Parser del decorator `@table("...")` splitea por `.` via
    helper `split_schema_qualified_table(s)` con validación.
  - Nuevo método `TableMetadata::qualified_sql_name()` —
    returns `"schema"."name"` o `"name"` (ya quoteado).
- **src/migrations.rs**:
  - `Table.schema: Option<String>` + `qualified_id()` method.
  - Nueva struct `TableRef { schema, name }` para identidad
    cross-schema. Constructores `public()`, `qualified()`,
    `from_table()`.
  - `Change` enum refactorizado: todas las variants con `table`
    ahora usan `TableRef` en vez de `String`. Nueva variant
    `CreateSchema { name }` emitida primero en el diff.
    `DropIndex` ahora tiene `schema: Option<String>` (PG needs
    qualified DROP INDEX para non-public).
  - `introspect_schema` ahora itera TODAS las user schemas
    (excluye `pg_catalog`, `information_schema`, `pg_toast*`,
    `pg_temp_*`, `_fitz_migrations`). `list_user_tables_qualified`
    devuelve `(schema, name)` tuples. `introspect_columns`/
    `indexes`/`foreign_keys` parametrizados por schema.
  - `diff_schemas` compara por `qualified_id` (no por name).
    Emite `CreateSchema` para schemas en target que no existen
    en current. `apply_renames_from_target` es schema-aware
    (renames dentro del mismo schema; cross-schema rename queda
    como deuda menor).
  - `change_to_sql` usa nuevo helper `quote_qualified(TableRef)`
    everywhere. Bare names para `public`, `"schema"."name"` para
    custom.
- **src/codegen.rs**:
  - `__FitzQueryBuilder.table` (preludio) ahora almacena la
    forma ya-quoteada qualified (`"users"` o `"public"."x"`).
    Los `format!` SQL del preludio cambian de `\"{}\"` a `{}`
    (~5 sitios en `build_select_sql`/`count`/`update`/
    `delete`/agg).
  - `qb_constructor` pasa `meta.qualified_sql_name()` (already
    quoted) en lugar de `meta.sql_name` (plain).
  - `target_table` en preload arms (HasMany + BelongsToCompanion)
    usa `qualified_sql_name()`; el format runtime `{table_lit}`
    sin extra quotes + escape `replace('"', "\\\"")` para que
    el embed funcione.
- **src/evaluator.rs**:
  - `SELECT ... FROM`, `INSERT INTO`, `UPDATE`, `DELETE FROM`,
    aggregates: todos usan `state.meta.qualified_sql_name()`
    (5 sitios refactorizados).
- **editors/vscode/package.json**: 0.10.20 → 0.10.21.

### Decisiones técnicas

- **Sintaxis con `.` en string del `@table`**: minimal change vs
  kwarg `@table("name", schema="...")`. Postgres usa la misma
  convención (`schema.table`).
- **`schema=None` = `public`**: backward compat 100% con código
  pre-v0.10.21. Tables sin schema explícito se comportan
  exactamente igual que antes.
- **Already-quoted-qualified en el field `table` del QB**: el
  preludio almacena `"public"."x"` ya quoteado y los `format!`
  interpolan con `{}` directo. Más simple que un campo
  `schema: Option<String>` paralelo + reconstruir en cada uso.
- **Cross-schema FK references**: en MVP, el FK del `@belongs_to`
  asume same-schema (la convención canonical "una table apunta
  a otra del mismo módulo"). Cross-schema FK queda como deuda
  menor si entra demanda.
- **Cross-schema rename**: no soportado en MVP. `@renamed_from`
  se interpreta dentro del schema actual de la table.

### Tests

- **0 unit tests nuevos** (existentes 60/60 cubren shape con
  `schema: None` default; el path schema custom se valida vía
  smoke E2E real).
- **2633/2633 lib tests verde** sin regresiones.
- **Smoke E2E real Postgres local validado bit-a-bit**:
  - 2 `@table` mixed (`users` public + `analytics.events`):
  - `db check` emite `CREATE SCHEMA IF NOT EXISTS "analytics";`
    + 2 CREATE TABLE qualified correctamente.
  - `db migrate` aplica todo OK.
  - `db check` post-migrate → `✓ schema sincronizado`.
  - ORM nativo: `User.insert(...)` (public) → id=1.
    `Event.insert(...)` (analytics) → id=1. `User.all(...)`
    + `Event.all(...)` SELECT contra `"analytics"."events"`
    devuelve rows correctas.

### Cierre formal Fase 10.6 — paquete migrations completo vs Alembic

Las 4 features del Tier 2 del plan original están cerradas
(v0.10.20 + v0.10.21). El stack `fitz db ...` ahora cubre:

| Feature | Versión | Equivalente Alembic |
|---|---|---|
| Auto-generate diff desde código tipado | v0.10.16 | ✓ |
| Apply pending + tracking idempotente | v0.10.16 | ✓ |
| Defaults SQL `@db_default("expr")` | v0.10.16 | ✓ |
| Down migrations + rollback | v0.10.17 | ✓ |
| Renames seguros via `@renamed_from` | v0.10.17 | ✓ |
| Drift check (CI bloqueante) | v0.10.18 | ✓ |
| Stamping (adoptar DB legacy) | v0.10.18 | ✓ |
| Data migrations en `.fitz` (Python-like) | v0.10.19 | ✓ |
| History (audit log) | v0.10.20 | ✓ |
| Offline SQL mode (DBA handoff) | v0.10.20 | ✓ |
| Squash (compactar migrations viejas) | v0.10.20 | ✓ |
| Schemas custom (multi-tenant) | v0.10.21 | ✓ |

**Diferenciales que Alembic NO tiene**:
- Cero deps externas (binario `fitz` solo vs `pip install
  alembic + sqlalchemy + psycopg2`).
- Schema desde código tipado del propio lenguaje (Alembic genera
  desde SQLAlchemy models, otro layer).
- Paridad bit-a-bit con el resto del stack (mismo driver en
  `fitz run`, `fitz build`, `fitz db ...`).

### Por qué importa

Cierra el último item del Tier 2 del plan. Equipos pueden ahora
modelar multi-tenant via PG schemas sin salir del lenguaje (cada
tenant en su schema con `@table("tenant_X.users")`). El paquete
completo de migrations queda al nivel funcional de Alembic con
diferenciales reales (cero deps, paridad, schema desde el code).

## [v0.10.20] — 2026-05-30 — Fase 10.6.e.1+.2: history + offline SQL + squash

Cierra 3 de las 4 features del Tier 2 del plan vs Alembic:
auditoría (`history`), handoff-a-DBA (`migrate --sql`), y
compactación de migrations viejas (`squash`). Schemas custom
(10.6.e.3) se difiere a v0.10.21 separada — la pre-eval reveló
cross-cutting con el ORM más grande de lo estimado.

### `fitz db history` — audit log de migrations applied

Lista las migrations aplicadas con `version` + `applied_at` +
filename. Orden `applied_at DESC` (más reciente primero). Si una
version está applied pero el file fue removido del dir
(post-squash o post-`stamp <legacy>`), aparece como
`(file removido)`.

```bash
fitz db history
# version              applied_at                       filename
# -------------------- -------------------------------- ----------
# 20260530120000       2026-05-30 10:53:24.800092-03    create_posts.sql
# 20260530100000       2026-05-30 10:53:24.775132-03    create_users.sql
# 2 migration(s) applied.
```

### `fitz db migrate --sql` — offline SQL mode (DBA handoff)

En vez de ejecutar las migrations pendientes, emite el SQL
concatenado al stdout (1 archivo por migration con header
`-- migration <version>: <filename>`). Útil para pasarle el SQL
a un DBA que aplica manual contra DBs prod sin exponer
credenciales al CLI.

```bash
fitz db migrate --sql > pending.sql
# 3 migrations emitidas al stdout
# Pasalas al DBA → psql -h prod-db -f pending.sql
# Marcalas como applied:
fitz db stamp --all
```

- Sigue conectándose para leer `_fitz_migrations` (skipea
  applied).
- Rechaza `.fitz` data migrations (no se materializan como SQL
  offline; usar `fitz db migrate` directo).
- Incompatible con `--dry-run` (clap valida).

### `fitz db squash <from> <to>` — compactar migrations viejas

Combina migrations del rango `[from, to]` (inclusive) en un
`<from>_squashed.sql`. Concatena los UP en orden + los DOWN en
orden INVERSO (para que el rollback siga funcionando). Mueve los
files originales a `migrations/squashed/` (no los borra). Si
alguna del range estaba applied en la DB, actualiza el tracking
para apuntar al nuevo squashed.

```bash
fitz db squash 20260101000000 20260301000000
# ✓ tracking actualizado: 47 versions removidas, stamped `20260101000000`
# ✓ 47 migration(s) squashed → migrations/20260101000000_squashed.sql.
#   Originales en migrations/squashed/.
```

Política:

- Solo `.sql` (rechaza `.fitz` en el rango — squashing de
  scripts del lenguaje no es semánticamente trivial).
- Rango mínimo 2 (squash de 1 = no-op rechazado).
- Tracking inteligente: si alguna applied, borra todas + stampea
  `from`. Si ninguna applied, no toca tracking.
- Pre-flight: aborta si el squashed ya existe.
- Flag `--no-tracking` para CI-only (skipea la actualización del
  tracking; user responsable de stampear manual en cada DB).
- Caso típico: repo con 100+ migrations viejas que el equipo ya
  aplicó. Squashear las primeras 80 acelera el bootstrap de devs
  nuevos sin afectar a quienes ya las aplicaron.

### Cambios técnicos

- **src/migrations.rs**:
  - Nueva struct `HistoryEntry { version, applied_at, filename }`
    + nueva `pub async fn history(conn, dir) -> DbResult<Vec<HistoryEntry>>`.
- **src/main.rs**:
  - Nueva variante `DbCmd::History { url, dir }` + handler
    `db_history_cmd` (output tabular).
  - Nueva variante `DbCmd::Squash { from, to, url, dir, no_tracking }`
    + handler `db_squash_cmd` (read range + pre-flight + emit
    squashed + move originals + update tracking).
  - `DbCmd::Migrate` suma flag `--sql`; `db_migrate_cmd` branchea
    en modo offline (lee tracking + emite SQL al stdout sin
    ejecutar).
- **editors/vscode/package.json**: 0.10.19 → 0.10.20.

### Tests

- **2 unit tests nuevos** en `src/migrations.rs::tests`:
  `history_entry_shape` + `history_signature_compila`.
- **60/60 migrations tests verde** (58 anteriores + 2 nuevos).
- **Smoke E2E real Postgres local validado bit-a-bit**:
  - 3 migrations `.sql` (create_users + add_name + create_posts)
    → `--sql` emite las 3 al stdout con header correcto + no
    toca DB → `migrate` aplica las 3 → `history` lista las 3 en
    orden cronológico inverso con `applied_at`.
  - `squash 20260530100000 20260530110000` combina users +
    add_name → emite `20260530100000_squashed.sql` con UP en
    orden + DOWN en orden inverso → mueve los 2 originales a
    `migrations/squashed/` → tracking borra las 2 versions y
    stampea solo `20260530100000` → `history` post-squash
    muestra 2 entradas (squashed + create_posts) con el
    squashed apuntando al filename nuevo.

### Schemas custom — DIFERIDO a v0.10.21

La pre-eval reveló:
- ~45 sitios entre evaluator/codegen/migrations que usan
  `meta.sql_name` directo sin concept de schema.
- Cross-cutting con el ORM (SELECT/INSERT/UPDATE/DELETE
  qualified, FK refs qualified, etc.).
- Estimación realista ~5-6 hs + risk de bugs ORM downstream.

Merece su propio commit + tag para que el smoke amplio cubra el
ORM. Plan en `docs/roadmap.md` → "Fase 10.6.e.3".

### Por qué importa

`fitz db history` cierra el último gap de visibility ("¿qué se
aplicó cuando?"). `migrate --sql` destraba el caso enterprise de
DBA-handoff (ops separadas de devs). `squash` evita que el dir
`migrations/` crezca sin techo en repos longevos — patrón
estándar de Alembic/Django/Rails que ahora Fitz también ofrece
con cero deps externas.

## [v0.10.19] — 2026-05-30 — Fase 10.6.d: data migrations en `.fitz`

`fitz db migrate` ahora reconoce DOS extensiones en `migrations/`:
`.sql` (DDL/DML crudo, splittable en `-- UP`/`-- DOWN`) y **`.fitz`**
(scripts del propio lenguaje con acceso completo a `db.query`,
`db.exec`, `db.transaction`, etc.). Se intercalan en orden
cronológico por el prefijo timestamp del filename.

Habilita transforms que SQL crudo NO expresa con elegancia:
back-fills con lógica condicional, parseo de JSON viejo a columns
nuevas, HTTP calls a un service externo durante la migración,
etc. — el caso típico que en Alembic / Rails se resuelve con
"data migration en Python/Ruby".

### Convención del `.fitz` migration

```fitz
// migrations/20260530150000_backfill_full_name.fitz

async fn migrate(db: DbConn) -> Result<Null> {
    // Acceso completo al lenguaje: loops, match, builtins,
    // db.transaction(...) para granularidad atómica.
    let _ = db.exec(
        "UPDATE users SET full_name = first_name || ' ' || last_name WHERE full_name IS NULL",
        [],
    ).await?
    return Ok(null)
}

// Opcional: si la declarás, `fitz db rollback` la invoca.
async fn rollback(db: DbConn) -> Result<Null> {
    let _ = db.exec("UPDATE users SET full_name = NULL", []).await?
    return Ok(null)
}
```

### Cambios técnicos

- **src/migrations.rs**:
  - `MigrationFile` refactorizado: ahora tiene `kind:
    MigrationKind` en vez de `up_sql/down_sql` directos.
  - Nueva enum `MigrationKind { Sql { up_sql, down_sql }, Fitz {
    path, source } }` + helpers `is_fitz()`.
  - `read_migrations_dir` acepta extensiones `.sql` y `.fitz`,
    detecta por sufijo, construye la variante correcta.
  - `apply_migration` rechaza migrations `.fitz` con error
    específico (el caller debe despachar al runner del lenguaje).
  - `revert_migration` y `rollback_n` paralelos rechazan
    `.fitz` con guards explícitos.
  - Nuevos helpers públicos `track_fitz_migration_applied(conn,
    version)` y `untrack_fitz_migration(conn, version)` para que
    el CLI inserte/borre el tracking después de invocar la
    callback del lenguaje.
- **src/main.rs**:
  - `db_migrate_cmd` ahora itera con dispatch per-kind:
    `.sql` → `apply_migration`, `.fitz` →
    `apply_fitz_migration_async`.
  - Nuevo `rollback_n_dispatch` paralelo a `migrations::rollback_n`
    pero con dispatch per-kind. `db_rollback_cmd` lo usa.
  - Nueva fn async `apply_fitz_migration_async(conn, version,
    filename, path, source)`: invoca el runner + trackea.
  - Nueva fn async `revert_fitz_migration_async`: invoca runner
    sobre `rollback` + untrackea.
  - Nueva fn async `run_fitz_migration_callback(conn, path,
    source, fn_name)`: parsea el `.fitz`, verifica que la fn
    está declarada como `async`, crea env vía
    `evaluator::new_repl_env()`, bindea `db` al `Value::DbConn`
    de la conn, appendea stmt sintético `let __fitz_mig_result =
    <fn_name>(db).await`, eval con `eval_program_with_env`,
    inspecciona el binding del env para extraer `Result::Ok(_)`
    vs `Result::Err(msg)`.
  - Nueva fn `fitz_migration_has_rollback(source)`: parsea
    source-only (sin tocar DB) para pre-flight del rollback.
- **editors/vscode/package.json**: 0.10.18 → 0.10.19.

### Decisiones técnicas

- **Convención `async fn migrate(db: DbConn) -> Result<Null>`**
  (paralelo al patrón `@test fn ...` del test runner). El user
  no escribe top-level code que dependa de un global `db` mágico;
  declara una fn explícita. Validable estáticamente, inspectable
  por el LSP, paralelo a cómo `fitz run` y `fitz test` ya
  modelan entry points.
- **`db` pre-bindeado al env del script**: la conn la maneja el
  CLI (lee `DATABASE_URL` o `--url`); el `.fitz` NO necesita
  llamar `db.connect(url)`. Inyectamos el `Value::DbConn` directo
  via `env.lock().define("db", ...)` antes del eval.
- **Atomicidad opt-in vs auto**: `.sql` migrations las envuelve
  el código en `db.transaction` automático. `.fitz` NO — el user
  decide granularidad (típicamente `return db.transaction(fn(tx)
  => ...).await` adentro del cuerpo de `migrate`). Más flexible:
  permite back-fills en chunks, retry parcial, multi-tx por
  diseño cuando el dataset es grande.
- **Rollback opcional**: si la `.fitz` declara `async fn
  rollback(db)`, el rollback la usa + borra registro. Si NO la
  declara, pre-flight aborta con mensaje claro (paralelo a `.sql`
  sin `-- DOWN`).
- **Eval, no codegen**: las `.fitz` migrations corren via
  intérprete (`evaluator::eval_program_with_env`). Para
  migrations con miles de iteraciones, el doc recomienda
  delegar el bulk a 1 UPDATE SQL en una `.sql` aparte.
- **Stmt sintético append**: en vez de invocar `migrate(db)`
  desde Rust directo (complica el path de invoke_value/dispatch),
  appendamos `let __fitz_mig_result = migrate(db).await` al AST
  parseado antes del eval. El `__fitz_mig_result` queda en el
  env, lo leemos vía `env.lock().get(...)` post-eval. Simple +
  reusa todo el path de evaluación normal.

### Tests

- **1 unit test nuevo** en `src/migrations.rs::tests`:
  `read_migrations_dir_detecta_fitz_files` valida que `.fitz` y
  `.sql` se intercalan en orden alfabético + la variante `kind`
  es la correcta + el `source` del `.fitz` queda cacheado.
- **58/58 migrations tests verde** (57 anteriores + 1 nuevo).
- **Smoke E2E real Postgres local validado bit-a-bit**:
  - 2 migrations mixtas (1 `.sql` create_users + 1 `.fitz`
    backfill_names) → `db status` lista ambas pending → `db
    migrate` aplica ambas en orden → DB rows con `name`
    rellenado por la `.fitz` → `_fitz_migrations` con ambas.
  - `db rollback` revierte solo la `.fitz` (ejecuta su `async
    fn rollback`) → `name` vacío + tracking de la `.fitz`
    eliminado.
  - Re-`db migrate` re-aplica solo la `.fitz` (la `.sql` sigue
    applied) → idempotencia OK.
  - Pre-flight error: `.fitz` SIN `async fn rollback` declarada
    → `db rollback` aborta antes de tocar la DB con mensaje
    específico ("no declara `async fn rollback(db: DbConn) ->
    Result<Null>`. Agregá la fn al archivo y reintentá").

### Cuándo usar `.fitz` vs `.sql`

- **`.sql`** — DDL puro (CREATE TABLE / ADD COLUMN / CREATE
  INDEX), back-fills triviales (`UPDATE users SET x = 1 WHERE x
  IS NULL`), seed fixtures. **~80% de las migrations**.
- **`.fitz`** — back-fills con lógica condicional o loops,
  parseo de JSON viejo a columns nuevas, HTTP calls durante la
  migración, transforms que requieren state que SQL crudo no
  expresa elegantemente.

### Limitaciones explícitas del MVP

- **Sin auto-wrap en tx**: el user decide granularidad. Si la
  `.fitz` migrate fallise a la mitad sin tx explícita, queda en
  estado parcial — escribí `return db.transaction(...).await`
  para garantizar atómico.
- **Stmt sintético con var pública**: `let __fitz_mig_result`
  contamina el env del script con un nombre interno. En
  práctica no choca (`__` prefix convención), pero un script
  que defina `__fitz_mig_result` por su cuenta tendría
  comportamiento sorprendente.
- **Eval-only**: las `.fitz` corren via intérprete, NO codegen.
  Para bulk-loads masivos preferí SQL crudo (1 UPDATE >> N
  iteraciones del intérprete).

### Por qué importa

Cierra el último gap funcional Tier 1 del plan vs Alembic.
Equipos pueden ahora hacer transforms reales (no solo DDL) sin
salir a Python/Ruby scripts externos. Combinado con drift check
(v0.10.18), rollback + renames (v0.10.17), y el ORM nativo
(v0.10.x), el stack DB de Fitz cubre el flujo completo de
desarrollo + CI/CD de schema management.

## [v0.10.18] — 2026-05-29 — Fase 10.6.c: drift check + stamping (+ driver fix OID `name`)

Cierra el Tier 1 más solicitado en surveys de Alembic: drift
check para CI bloqueante. Más adopción de Fitz en DB legacy via
stamping. Más un driver fix crítico descubierto durante el smoke
real con Postgres local.

### `fitz db check` — drift detection para CI

Corre el diff del schema declarado vs la DB real:
- **Exit 0** + `✓ schema sincronizado` si sin cambios.
- **Exit 1** + SQL pendiente al stderr si hay drift, con sugerencia
  de cómo sincronizar.

```bash
fitz db check src/main.fitz
# ✓ schema sincronizado — schema declarado matchea la DB
# (exit 0)
```

Patrón canónico en CI:
```yaml
- name: Schema drift check
  run: fitz db check src/main.fitz
  env:
    DATABASE_URL: ${{ secrets.STAGING_DB_URL }}
```

### `fitz db stamp <version>` / `--all` — adoptar Fitz en DB legacy

Marca migrations como aplicadas en `_fitz_migrations` **sin
ejecutar el SQL**. Caso de uso típico: adoptar Fitz en un
proyecto que ya tiene el schema aplicado manualmente.

```bash
# 1. Generás migration que matchea el schema actual:
fitz db diff src/main.fitz > migrations/20260530000000_initial.sql

# 2. Marcás como aplicada SIN ejecutarla:
fitz db stamp 20260530000000
#   ✓ stamped: 20260530000000

# 3. A partir de acá, `migrate` aplica solo nuevas.
```

`--all` marca todas las pending del dir en una pasada (caso
adopción inicial). Idempotente — ya-applied → no-op silencioso.
Warning sobre versions que no existen en el dir (typo guard).

### Driver fix — OID 19 (`name` type de `pg_catalog`) → Text

**Descubierto durante el smoke real**: las queries de introspect
de `migrations` consultan `information_schema.columns` cuyos
campos `column_name` y `udt_name` son tipados como
`sql_identifier` (alias de `name` interno de Postgres con OID 19).
El driver no manejaba OID 19 → error
`tipo Postgres OID 19 no soportado en MVP (10.5)` rompía TODO
`fitz db ...` que introspectara.

Fix trivial: `oid::NAME = 19` agregado al match `parse_text_value`
(treat as Text, equivalente a `text`/`varchar`). Cambio de 6 LoC
+ desbloqueador crítico de toda la sub-fase 10.6.

### Cambios técnicos

- **src/migrations.rs**:
  - Nueva fn `stamp_version(conn, version) -> DbResult<bool>` con
    `ON CONFLICT DO NOTHING` para race-safety. Devuelve true si
    insertó, false si ya estaba.
  - Nueva fn `stamp_all_pending(conn, migrations) -> DbResult<Vec<String>>`
    que itera el dir y stampea solo las no-applied.
- **src/main.rs**:
  - Nuevas variantes `DbCmd::Check { file, url }` y
    `DbCmd::Stamp { version, all, url, dir }` con clap
    `conflicts_with` entre version y all.
  - Nuevos handlers `db_check_cmd` (reusa diff + decide exit
    code) y `db_stamp_cmd` (wrap stamp_version / stamp_all_pending
    + warning sobre versions no en dir).
- **src/db.rs**:
  - `oid::NAME = 19` agregado al módulo `oid`.
  - Branch `oid::NAME` en `parse_text_value` (treat as Text junto
    con `oid::TEXT` / `oid::VARCHAR`).
- **editors/vscode/package.json**: 0.10.17 → 0.10.18.

### Tests

- **2 unit tests nuevos** en `src/migrations.rs::tests`:
  - `check_es_verde_cuando_diff_es_vacio` + `check_falla_cuando_hay_drift`:
    valida la decisión de exit code basada en `diff_schemas`.
  - `stamp_version_y_stamp_all_pending_estan_exportadas`: smoke
    estructural (rompe a compilar si renombran o cambian firmas).
- **57/57 migrations tests verde** (54 anteriores + 3 nuevos).
- **Smoke end-to-end real Postgres local validado**: create DB
  → `db check` (drift detected, exit 1 con SQL) → `db new` +
  `db diff --out` → `db migrate` → `db check` (sincronizado,
  exit 0) → `db stamp <version>` (no-op) → `db stamp 19990101000000`
  (warning + stamped) → `db stamp --all` (no-op) → `db stamp`
  sin args (error claro).

### Por qué importa

`fitz db check` cierra el último gap visible para uso CI/CD
profesional: equipos pueden bloquear PRs que diverjan del schema
de staging. `fitz db stamp` destraba la adopción de Fitz en
proyectos legacy (caso típico: equipo con SQLAlchemy quiere
migrar a Fitz manteniendo la DB). El driver fix OID 19 era un
landmine — sin él, **ninguna** corrida de `fitz db diff/check/migrate`
funcionaba contra una DB que ya tuviera tables (porque la
introspect failearía después de la primera). Lo descubrimos a
las primeras corridas del smoke real con Postgres local.

## [v0.10.17] — 2026-05-29 — Fase 10.6.b: rollback + renames seguros

Cierra los dos gaps Tier 1 más visibles de migraciones contra
Alembic: forward-only (sin rollback) y renames perdiendo datos.

### Rollback (`fitz db rollback [--count N]`)

Las migrations soportan secciones explícitas `-- UP` / `-- DOWN`
para revertir. Backward-compatible: archivos sin marcadores
siguen siendo "UP implícito sin DOWN" (no se pueden revertir,
pero `migrate` los aplica igual).

```sql
-- Migration: add_email_to_users

-- UP
ALTER TABLE "users" ADD COLUMN "email" text NOT NULL DEFAULT '';

-- DOWN
ALTER TABLE "users" DROP COLUMN "email";
```

```bash
fitz db rollback              # revierte el último
fitz db rollback --count 3    # revierte los últimos 3
```

Política:
- `fitz db new` genera stubs con `-- UP` / `-- DOWN` por
  convención.
- Marcador case-insensitive sobre línea propia (`-- UP`, `--up`,
  `-- Up` matchean). `-- UP foo` NO (chars extra → SQL comment
  normal).
- Sección DOWN vacía / solo whitespace → `None` (irreversible).
- Si querés revertir N>1 y alguna target NO tiene `-- DOWN`, el
  rollback **aborta ANTES de tocar la DB** con mensaje específico
  citando filename. Cero estado parcial pre-flight.
- Cada `revert_migration` es atómico individual (1 tx). Rollback
  de N>1 son N tx — si la k-ésima falla en runtime, las anteriores
  ya persistieron. Para "todo o nada" sobre N, usar 1 migration
  única con todo el rollback adentro.
- Orden de rollback: `applied_at DESC` del tracking (más reciente
  primero), NO orden de filename.

### Renames seguros (`@renamed_from("old_name")`)

Decorator transient sobre field o `@table` para que el diff
emita `ALTER TABLE ... RENAME COLUMN/TABLE` en vez de `DROP +
ADD`, preservando datos.

```fitz
// Rename column.
@table("users") type User {
    @primary id: Int = 0
    @renamed_from("name") full_name: Str = ""
}

// Rename tabla.
@table("users") @renamed_from("legacy_users") type User {
    @primary id: Int = 0
}
```

`fitz db diff` emite:

```sql
ALTER TABLE "legacy_users" RENAME TO "users";
ALTER TABLE "users" RENAME COLUMN "name" TO "full_name";
```

Política:
- Orden seguro en el output: renames PRIMERO, después
  ADD/DROP/ALTER COLUMN sobre el nombre nuevo.
- No-op silencioso cuando el rename ya se aplicó (target tiene
  `@renamed_from("old")` pero current ya solo tiene "new" —
  caso típico post-migration). El user borra el decorator
  cuando quiera.
- Por qué decorator y no subcomando: el subcomando divorcia
  rename del cambio en el code (fácil olvidar uno); decorator
  es declarativo + atómico con el código.

### Cambios técnicos

- **src/migrations.rs**:
  - `MigrationFile` reemplaza `sql: String` por `up_sql: String`
    + `down_sql: Option<String>`.
  - Nueva fn `split_up_down(raw)` con parser line-anchored case-
    insensitive de marcadores `-- UP` / `-- DOWN`.
  - Nuevas variantes `Change::RenameTable` + `Change::RenameColumn`.
  - Nueva fn `apply_renames_from_target(current, target, changes)`
    que pre-procesa los hints `renamed_from` del target: emite
    rename Changes al frente y devuelve una versión renombrada
    de current para que el resto del diff compare por nombres
    post-rename.
  - Nueva fn `revert_migration(conn, migration)`: ejecuta el
    `-- DOWN` adentro de tx + borra registro de
    `_fitz_migrations`. Atomic. Error específico si `down_sql`
    es None.
  - Nueva fn `rollback_n(conn, migrations, n)`: pre-flight
    valida que TODAS las versiones target tienen file + DOWN,
    después revierte una por una (atomic individual).
  - Nueva fn `applied_versions_desc` (orden por `applied_at
    DESC`) para `rollback_n`.
- **src/types.rs**:
  - `TableMetadata.renamed_from: Option<String>` paralelo al
    `sql_name`. Parsea `@renamed_from("old")` a nivel type.
  - `ColumnMetadata.renamed_from: Option<String>` para fields.
    Parsea `@renamed_from("old")` a nivel field.
  - Validación: solo arg Str literal no vacío, rechaza otros
    con mensaje claro.
  - Error del decorator inválido sobre `type` actualizado para
    listar `@renamed_from` también.
- **src/main.rs**:
  - Nueva variante `DbCmd::Rollback { url, dir, count }` +
    dispatch + handler `db_rollback_cmd`.
  - `db_new_cmd` genera stub con secciones `-- UP` / `-- DOWN`
    por convención.
- **src/lsp.rs**: nuevo completion item snippet para
  `@renamed_from("${1:old_name}")`. Doc del `@db_default` y la
  lista de decorators ORM en `AfterAt` actualizada.
- **src/migrations.rs**: `Table.renamed_from` y
  `Column.renamed_from` agregados (poblados solo en target
  schema desde `schema_from_program`; `None` en introspect).
- **editors/vscode/package.json**: 0.10.16 → 0.10.17.

### Tests

- **15 unit tests nuevos** en `src/migrations.rs::tests`:
  - 6 sobre `split_up_down` (sin marcadores, ambos, case-
    insensitive, sección vacía, sin UP solo DOWN, marker con
    chars extra que NO es marker).
  - 6 sobre renames (RenameTable + RenameColumn emit + SQL del
    output + no-op silencioso cuando no hay match + orden
    seguro rename-antes-de-alter + cargar `renamed_from` a
    Column/Table desde program).
  - 1 sobre `read_migrations_dir` que preserva up/down.
  - 2 sobre `schema_from_program` con `@renamed_from` field y
    table.
- **54/54 migrations tests verde** (39 anteriores + 15 nuevos).
- **2627/2627 lib tests verde** (sin regresiones).
- **Smoke `GUIDE_EXAMPLES_COMPILE`** verde (292 ejemplos).
- **Smoke manual**: `fitz check` y `fitz run` aceptan
  `@renamed_from(...)` en field y type; `fitz db rollback
  --help` documenta la nueva subcomando; `fitz db new` emite
  stub con secciones `-- UP` / `-- DOWN`.

### Limitaciones explícitas del MVP

- **Rollback de N>1 NO es atómico transversal**: cada `revert`
  es 1 tx aislada. Para "todo o nada" sobre N migrations, una
  migration única con todo el rollback adentro.
- **`@renamed_from` no detecta renames cíclicos** (A → B → A):
  caso degenerado, el user lo resuelve manualmente.
- **`ALTER COLUMN ... TYPE` sin USING** sigue siendo deuda
  (cambios incompatibles fallan — editar migration con USING).

### Por qué importa

Hasta v0.10.16 Fitz tenía migraciones forward-only sin rollback
y renames que perdían datos — los dos gaps más visibles vs
Alembic. v0.10.17 los cierra. El siguiente Tier 1 del plan
roadmap es **Fase 10.6.c (drift check + stamp)** para CI
bloqueante y adopción en DB legacy.

## [v0.10.15] — 2026-05-29 — `db.transaction` acepta FnExpr inline (paridad `fitz run` ↔ `fitz build`)

Cierre de la deuda más visible de v0.10.14 — el codegen MVP solo
aceptaba fn nombrada como callback de `db.transaction(...)`. Ahora
acepta también FnExpr inline (`async fn(tx) -> Result<T> { ... }`)
con captures del outer scope.

### Cambio user-facing

```fitz
@post("/transfer/{from_id}/{to_id}/{amount}")
async fn transfer(from_id: Int, to_id: Int, amount: Float) -> Result<Account> {
    let conn = db.connect(db_url).await?
    return conn.transaction(async fn(tx) -> Result<Account> {
        let from = Account.where(fn(a) => a.id == from_id).first(tx).await?
        let to = Account.where(fn(a) => a.id == to_id).first(tx).await?
        // ... transferí dinero, balance check, etc.
        return Ok(to)
    }).await
}
```

Antes (v0.10.14) el codegen forzaba extraer el callback a una fn
nombrada por restricción del MVP. Ahora la sintaxis inline natural
funciona idéntica a `fitz run`.

### Implementación

`gen_db_conn_method_call` arm `"transaction"` suma Path 2 nuevo
para FnExpr inline:
- Ret type sacado del `TypeInfo` del checker via
  `type_info.type_at(args[0].span())` (NO `infer_callback_ret_silently`
  que hace dry-run sin scope).
- Push `unwrapped` (`Result<T>`) al `ret_stack`, no `inferred`
  (`Future<Result<T>>`) — el body interno del async closure es
  código cuyo ret natural es Result<T>; sin esto, `?` rechazaba
  con "solo en fn que retorna Result".
- Emit: `__fitz_db_transaction(&{db}, move |{param}: __FitzDbConn|
  async move {{ {body} }})`. Doble `move` (outer FnOnce + inner
  async Send).

Path 3 (otro Expr) sigue dando error claro listando los 2 patterns
válidos.

### Cambios complementarios

- `examples/guide/31c-transactions.fitz` revertido a la sintaxis
  inline natural (era el fix forzado en v0.10.14 que extrajo las
  3 closures a fns nombradas).
- Extension VSCode bump 0.10.14 → 0.10.15.

## [v0.10.14] — 2026-05-29 — Transactions ORM con `db.transaction(fn)` closure-based

**Cierre formal Fase 10.7**. Escrituras atómicas multi-step con
BEGIN/COMMIT/ROLLBACK automático según el `Result` del callback.
Imposible olvidarse el commit/rollback — el control de flujo del
Result garantiza la atomicidad.

### API user-facing

```fitz
let result = db.transaction(async fn(tx) -> Result<Int> {
    let user = User.insert(tx, User { ... }).await?
    Order.insert(tx, Order { user_id: user.id, ... }).await?
    return Ok(user.id)
}).await?
```

El `tx: DbConn` es del mismo tipo que `db`, pero internamente
pegado a la misma conn física durante toda la tx. Todos los métodos
del ORM (`.insert`/`.update`/`.delete`/`.first`/`.all`) y escape
hatch (`.query`/`.exec`) funcionan sin cambios.

### Sub-pasos

1. **`src/db.rs` — `Connection::begin/commit/rollback`** primitivos.
   Wrappers simples sobre `simple_query`. Sin niveles de aislamiento
   explícitos (usa default del server, típico READ COMMITTED).
2. **`src/db.rs` — `DbConnHandle::transaction<F, Fut, T>(self:
   &Arc<Self>, f)`** orquestador con auto-rollback en Err/panic +
   cleanup de la conn al pool. Single-conn pool interno
   (`max_conns=1`) garantiza isolation físico — todas las queries
   del callback usan la misma conn.
3. **`tests/db_real_postgres.rs`** — 3 tests E2E nuevos:
   - `tx_happy_path_commit_persiste`
   - `tx_rollback_explicito_nada_persiste`
   - `tx_conn_vuelve_al_pool_despues_de_tx` (5 iter consecutivos sin leak)
4. **`src/evaluator.rs`** — builtin `db_conn_transaction` + dispatch.
   Preserva el `Value` original del Err callback via cell compartido
   (`Arc<Mutex<Option<Value>>>`) — el `Err` Fitz no se aplana al
   `DbError::Protocol` del driver.
5. **`src/codegen.rs`** — `gen_db_conn_method_call` arm `"transaction"`
   + `__fitz_db_transaction` helper genérico en el preludio. MVP
   soporta SOLO fn nombrada como callback (no FnExpr inline); error
   de codegen explícito sugiere el workaround. El intérprete sí
   permite inline. Refinable a futuro → **cerrado en v0.10.15**.

### Limitaciones MVP

- Sin niveles de aislamiento custom (READ UNCOMMITTED, SERIALIZABLE,
  etc.).
- Sin nested transactions con SAVEPOINT.
- Sin read-only transactions.

Todos quedan como deuda menor (revisable si entra presión).

## [v0.10.13] — 2026-05-29 — Driver Postgres B-1 fix (Extended Query batching + TCP_NODELAY) + bench fixes

Bloque grande agrupando mini-fases relacionadas con la calidad del
bench Fitz ORM vs SQLAlchemy + sus hallazgos.

### B-1 — Extended Query Protocol optimization

Root cause identificado en el benchmark v2: `GET /users/{id}` (que
usa `WHERE id = $1`, extended query) tardaba 43ms p50 vs 4ms del
simple query. El driver hacía 5 `self.write(...).await?` separados
para Parse/Bind/Describe/Execute/Sync, sumando ~30-40ms de overhead
por Nagle + 5 syscalls write() + 5 awaits.

**Fix doble en [src/db.rs](src/db.rs)**:

1. **`TCP_NODELAY`** al construir el TcpStream (deshabilita Nagle).
   Crítico porque mandamos 5 mensajes consecutivos sin esperar
   respuesta del server entre ellos — sin esto el kernel TCP
   retrasaba cada paquete chico esperando ACK del previo.
2. **Batch los 5 mensajes en UN solo `write_all_bytes(...)`** —
   `Vec<u8>` con concat de los 5 `encode()`. Server Postgres NO
   responde hasta `Sync`; es pipelining protocolar legítimo, no
   cambio semántico.

**Resultado** (bench publicable v0.10.13):

| Endpoint | Pre-fix Fitz p50 | Post-fix Fitz p50 | Python SQLAlchemy p50 |
|---|---:|---:|---:|
| `GET /users/{id}` | 43.70 ms | **3.60 ms** | 31.87 ms |
| `GET /users` | 4.92 ms | **4.88 ms** | 37.85 ms |

Fitz pasó de "30% más lento que Python en single-by-PK" a **8.85x
más rápido**. Headline del bench: **5-10x speedup + 5.5x menos
memory** en read workloads.

### Bench fixes

- **Image size grep**: `^${boilerplate}-api:latest ` (anchor exacto)
  para no pescar otros boilerplates cacheados.
- **Memory peak sampler**: container names correctos según el
  `container_name:` del docker-compose de cada boilerplate.
- **POST x500 → x100**: en Git Bash Windows el overhead del subshell
  (~1s/iter) hace que x500 tarde ~10min. x100 es suficiente para
  p50/p95/p99 representativos.
- **PID via archivo** para el memory sampler (fix Git Bash:
  capturar PID via `$()` espera todo el subshell, hace hang).

### Migración Python boilerplates → ghcr `fitz:latest-python`

`api-postgres-python` + `api-fullstack-postgres`: Dockerfile migrado
de `cargo install --git` (~8-12 min build inicial) al patrón
pre-built `ghcr.io/thegreekman76/fitz:latest-python` (~30-60s).
Reducción ~10x del build time. Trade-off: dependencia de la
imagen publicada por CI release (default `latest-python`, override
con `--build-arg FITZ_TAG=v0.10.13-python`).

## [v0.10.16] — 2026-05-29 — Fase 10.6: migraciones automáticas ORM + `@db_default("expr")`

`fitz db diff/migrate/status/new` — el binario ahora introspecciona
el schema real de Postgres, lo compara con los `@table type`
declarados, y emite el SQL `ALTER TABLE` / `CREATE TABLE` necesario
para sincronizar. Las migrations versionadas se aplican con
tracking idempotente en `_fitz_migrations`. **Cero deps externas**:
ni Alembic ni Flyway ni Liquibase ni TypeORM CLI. La fuente de
verdad es el código tipado del lenguaje.

### Subcomandos nuevos

| Subcomando | Qué hace |
|---|---|
| `fitz db diff [archivo.fitz] [--out file.sql]` | Compara schema declarado vs real, emite SQL al stdout o file. |
| `fitz db migrate [--dry-run]` | Aplica los `.sql` pendientes del dir `./migrations` en orden alfabético. |
| `fitz db status` | Lista cada archivo `.sql` con badge `✓ applied` / `→ PENDING`. |
| `fitz db new <name>` | Crea `migrations/YYYYMMDDHHMMSS_<name>.sql` con stub vacío. |

URL: lee `DATABASE_URL` env var, o pasa `--url postgres://...`.
Dir: `./migrations` por default, override con `--dir`. Entry:
explícito o `[bin].main` del manifest.

### `@db_default("expr")` — defaults SQL en el `type`

El decorator `@db_default` (introducido en v0.10.8 como marker
"skip INSERT") ahora acepta un arg Str opcional con la expresión
SQL del default. Si está, `fitz db diff` emite `DEFAULT <expr>`
automáticamente en `CREATE TABLE` / `ADD COLUMN`. Si cambia, el
diff emite `ALTER TABLE ... ALTER COLUMN ... SET/DROP DEFAULT`.
Sin arg, comportamiento original (marker-only).

```fitz
@table("events") type Event {
    @primary id: Int = 0
    @db_default("NOW()") created_at: Str = ""
    @db_default("gen_random_uuid()") tracking_id: Str = ""
}
```

**Idempotencia del diff** — la normalización es tolerante a
variaciones cosméticas que Postgres aplica automáticamente:

- Case-insensitive en function calls (`NOW()` ↔ `now()`).
- Strip de casts redundantes (`'foo'::text` ↔ `'foo'`).
- Trim whitespace.

NO intenta evaluar expresiones equivalentes (`now()` ≠
`CURRENT_TIMESTAMP` desde el lado del diff aunque ambos sean
válidos para `timestamptz`). El user elige una y la mantiene.

### Cambios técnicos

- **src/migrations.rs** (~1260 LoC nuevas): módulo dedicado con
  `Schema`/`Table`/`Column`/`Index`/`ForeignKey` structs,
  `introspect_schema(conn)` via `information_schema` +
  `pg_catalog`, `schema_from_program(program, type_env)` que
  walka el AST + `TableMetadata`, `diff_schemas(current, target)`
  con orden seguro (CREATE TABLE → ADD/DROP/ALTER COLUMN →
  CREATE INDEX → DROP FK → ADD FK → DROP TABLE),
  `changes_to_sql(changes)` con quoted identifiers, helpers de
  tracking (`ensure_tracking_table`, `applied_versions`,
  `read_migrations_dir`, `apply_migration`,
  `apply_pending_migrations`, `status`).
- **src/lib.rs**: nueva `pub mod migrations`.
- **src/main.rs**: nueva variante `Commands::Db(DbCmd)` con 4
  subcomandos vía clap. Handlers `db_diff_cmd`/`db_migrate_cmd`/
  `db_status_cmd`/`db_new_cmd` con helpers `resolve_db_url`,
  `resolve_migrations_dir`, `resolve_db_entry`,
  `load_program_for_db`. Todos los handlers usan una sola
  runtime tokio para connect + work (evita que health_check_task
  muera con un runtime que se dropea entre connect y query).
- **Cargo.toml**: `chrono = "0.4"` reusado (ya dep para
  jobs/cron) para timestamps `YYYYMMDDHHMMSS_<name>.sql`.
- **src/types.rs**: `ColumnMetadata.db_default_sql:
  Option<String>` paralelo al flag `db_default` existente. El
  parser del decorator acepta 0 args (marker-only, backward
  compat con v0.10.8) o 1 arg Str literal (nueva semántica
  v0.10.16) — rechaza arg no-Str con mensaje específico.
- **src/migrations.rs**: nueva variante `Change::AlterColumnDefault`
  + helper `normalize_default_for_diff` (lowercase + strip
  trailing PG cast). `introspect_columns` strippea `nextval(...)`
  del default de PK bigserial para evitar falso positivo.
- **src/lsp.rs**: doc del completion item `@db_default` actualizado
  para mencionar el arg Str opcional con ejemplos.
- **editors/vscode/package.json**: version `0.10.15` → `0.10.16`.
  Grammar TextMate sin cambios (ya matchea `@db_default` y
  cualquier decorator con args via la rule de strings).

### Decisiones técnicas

- **Quoted identifiers everywhere** (`"users"`, `"email"`): los
  CREATE TABLE / ALTER COLUMN emitidos quotean cada nombre para
  que reserved words o caracteres especiales no rompan.
- **Filesystem-based, no DSL custom**: migrations son `.sql`
  planos editables a mano. Patrón estándar de Flyway/Rails. El
  diff genera el SQL, el user lo redirige al file y lo edita
  si necesita refinos manuales.
- **Forward-only** (sin `down` migrations): para revertir,
  escribís una nueva migration con el cambio inverso. Patrón
  Rails sin `down`, Alembic sin `downgrade`. Menos código,
  menos drift posible entre `up` y `down` que se desincronizan.
- **Tracking en tabla dedicada** (`_fitz_migrations` con
  `version TEXT PRIMARY KEY` + `applied_at TIMESTAMPTZ DEFAULT
  NOW()`): patrón estándar. Re-correr `migrate` es siempre
  no-op si todo está aplicado.
- **Schema diff determinístico**: orden estable de categorías +
  sort alfabético dentro de cada categoría — re-correr `diff`
  con los mismos inputs produce siempre el mismo output (clave
  para grep/sed/CI checks contra diffs esperados).
- **Solo schema `public`** en el MVP: refinamiento futuro si
  entra demanda multi-schema.

### Tests

- **39 unit tests nuevos** en `src/migrations.rs::tests`
  cubriendo: diff de schemas vacíos/iguales (idempotente);
  CREATE/DROP table; ADD/DROP/ALTER column con type + nullable;
  CREATE/DROP index; ADD foreign key; orden seguro (CREATE
  antes que DROP); determinismo cross-runs; emission de SQL
  para cada `Change`; round-trip `schema_from_program(src)`
  con types Fitz reales + `diff` contra sí mismo es vacío;
  dos versiones del schema yield `AddColumn`. **+13 tests del
  `@db_default("expr")`**: parse + emission CREATE TABLE/ADD
  COLUMN/SET DEFAULT/DROP DEFAULT + idempotencia diff
  case-insensitive + strip PG casts + round-trip schema con
  default normalizado.
- **2612/2612 unit tests verde** post-cambios (sin regresiones).
- **Smoke real Postgres**: validable vía CI (job `db-postgres`).
  En Windows host contra Docker-mapped Postgres reproduce un
  bug pre-existente del driver wire protocol ("cstr no es
  UTF-8") que NO bloquea uso desde Linux/CI.

### Limitaciones explícitas del MVP

- **No detecta renames** (column ni table): un rename Fitz-side
  `name` → `full_name` se ve como `DROP COLUMN + ADD COLUMN`,
  **perdiendo datos**. Editá la migration a mano (`ALTER TABLE
  ... RENAME COLUMN`) cuando el caso lo justifique.
- **`ALTER COLUMN ... TYPE` sin USING**: cambios incompatibles
  (`text → int`) fallan. Editá la migration para agregar
  `USING (col::int)`.
- **`@db_default` sin arg sigue siendo marker-only**: el
  comportamiento de v0.10.8 se preserva (skip INSERT, sin
  default en migration). Para que el diff emita el default,
  pasale la expresión SQL explícita (`@db_default("NOW()")`).
- **Solo schema `public`** (no multi-schema).
- **Forward-only** (sin `down`/`downgrade`).

### Docs actualizados

- `docs/db-orm.md`: nueva sección 26.c "Migraciones automáticas
  (v0.10.16)" con workflow canónico + política + limitaciones +
  por qué Fitz lo hace distinto. Sección 28 actualizada
  (migraciones + transactions movieron de "deuda" a "CERRADO").
  Sección 29 (CLI con DB) suma sub-sección con los 4 nuevos
  subcomandos.
- `docs/guide.md` cap 31 (Postgres + ORM nativo): nueva
  sub-sección "Migraciones automáticas (v0.10.16)" con el
  workflow básico y link a `docs/db-orm.md` para detalles.
- `CHANGELOG.md`: esta entrada.

### Por qué importa

Hasta v0.10.15, el schema se escribía a mano en `db.exec(
"CREATE TABLE IF NOT EXISTS ...", [])` al boot del programa
(idiomatic en los ejemplos de la guía pero manual y no
versionado). Equipos serios necesitan: cambios versionados, CI
checks contra drift schema vs código, rollouts ordenados, y
visibilidad de "qué migrations corren en cada deploy". `fitz db
diff/migrate/status/new` resuelve esto en el binario, sin
levantar deps externas. Combinado con Transactions ORM
(v0.10.14-15), el stack DB de Fitz es ahora self-contained
end-to-end.

## [v0.10.12] — 2026-05-29 — LSP completion tras `@` + 9no boilerplate fullstack

Dos cambios paralelos en una sola release:

### LSP completion tras `@` (DX del editor)

Cerrada la deuda LSP heredada de v0.10.11 ("hoy grammar destaca
cualquier `@name` pero el LSP no sugiere la lista cerrada de
decorators"). Al escribir `@` o `@<prefix>` en el editor, el LSP
sugiere ahora los 23 decorators del lenguaje con snippets útiles.

**Cambios técnicos**:

- **src/lsp.rs**: nuevo `CompletionContext::AfterAt` detectado
  cuando el char antes del prefix ident es `@`. Tiene prioridad
  sobre `AfterDot` (el char `@` no forma parte de un ident chain).
- **src/lsp.rs**: nueva fn `decorator_completions()` que devuelve
  los 23 decorators agrupados en 5 familias:
  - HTTP routing: `@get`/`@post`/`@put`/`@delete`/`@server`/`@header`
  - Middleware/CORS: `@middleware`/`@cors`
  - Auth: `@authenticated`/`@admin`/`@auth_provider`
  - WS + Jobs: `@ws`/`@cron`/`@background`/`@test`
  - ORM: `@table`/`@primary`/`@column`/`@unique`/`@index`/
    `@db_default`/`@hidden`/`@belongs_to`/`@has_one`/`@has_many`
- **Snippets con tabstops** (`${N:placeholder}`): decorators con
  args típicos (`@get("/path")`, `@table("name")`) emiten un
  placeholder editable. Decorators sin args (`@hidden`,
  `@primary`, `@test`, etc.) emiten el nombre plano. Decorators
  de relation emiten dos tabstops (`@belongs_to("Target",
  via="fk")`).
- **src/bin/fitz-lsp.rs**: `CompletionOptions.trigger_characters`
  expandido a `[".", "@"]` — VSCode invoca completion
  automáticamente cuando el usuario tipea `@`.
- **tests/lsp_e2e.rs**: nuevo test
  `completion_after_at_lista_decorators_v0_10_12` que valida la
  capability, la lista de 17 decorators core, kind=SNIPPET (15) e
  insertTextFormat=2 (snippet). Test viejo
  `completion_after_dot_sobre_str_lista_metodos_built_in`
  actualizado para tolerar el nuevo trigger char `@`.

### 9no boilerplate: `api-orm-full-fullstack` ⭐⭐⭐

Replica el backend de `api-orm-full` (HTTP + auth + WS + cron +
Postgres ORM) **sumando un frontend vanilla** en nginx que consume
todo el stack desde un browser real. Cubre el ciclo "browser →
server → DB" end-to-end.

**Estructura**:
- `src/` — idéntica a `api-orm-full` (ningún cambio al backend).
- `frontend/` — Dockerfile + nginx.conf + 7 pantallas HTML/CSS/JS
  vanilla (sin build step, sin node_modules, <100 KB total).
- `docker-compose.yml` — 3 services: db (Postgres 16) + api (Fitz
  binario standalone) + frontend (nginx-alpine).

**Pantallas**:
| URL | Endpoint(s) | Qué ejercita |
|---|---|---|
| `/login.html` | `POST /auth/login`, `POST /auth/register` | JWT en localStorage |
| `/posts.html` | `GET /posts?status=&tag=` | listado con filtros |
| `/post-detail.html?id=N` | `GET /posts/{id}` + preload, `POST /posts/{id}/comments` | eager loading inline |
| `/new-post.html` | `POST /posts` | tags array + jsonb desde browser |
| `/edit-post.html?id=N` | `PUT /posts/{id}` | partial update con `Map<Str, Any>` |
| `/stats.html` | `GET /stats/posts-per-user` | GROUP BY con Chart.js |
| `/feed.html` | `WS /feed` | WS auth realtime |

**Decisión técnica clave — nginx proxy same-origin**: el frontend
hace `fetch("/api/...")` y `new WebSocket("/ws/feed?token=...")`.
nginx proxy-ea ambos al backend en `:3000`. Esto resuelve **dos
limitaciones de los browsers** sin tocar el backend:
- **Sin CORS**: requests same-origin desde la perspectiva del
  browser.
- **WS auth header injection**: los browsers NO permiten custom
  headers en `new WebSocket(url)`; el frontend pasa el token JWT
  como `?token=...` y nginx lo transforma a header
  `Authorization: Bearer ...` antes del proxy.

```nginx
location /ws/ {
    proxy_pass http://api:3000/;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    set $auth_token "";
    if ($arg_token) { set $auth_token "Bearer $arg_token"; }
    proxy_set_header Authorization $auth_token;
}
```

**Stack frontend**:
- HTML/CSS/JS vanilla, sin build step.
- Pico.css (classless) via CDN — sin clases CSS, HTML semántico
  se ve bien.
- Chart.js via CDN para `/stats.html`.
- localStorage para JWT.

`boilerplates/README.md` actualizado de "8 boilerplates" a "9
boilerplates" con entrada nueva entre `api-orm-full` y
`api-postgres-python`.

### Validación

- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets --features lsp -- -D warnings` limpio.
- `cargo test --release --features lsp --test lsp_e2e` 6 tests verde
  (5 anteriores + 1 nuevo de AfterAt).
- `cargo test --release --test compile_e2e smoke_ejemplos_guia` 292
  ejemplos verde (sin regresiones).
- Backend del 9no boilerplate: `fitz check src/main.fitz` limpio.
- Smoke real Docker pendiente al release CI verde.

## [v0.10.11] — 2026-05-29 — `@hidden` field decorator + boilerplates con LATEST

Cierre de la **deuda menor del boilerplate** detectada en smoke real
v0.10.10: el response de `GET /posts/{id}` con `.preload("author")`
exponía `password_hash` del User embebido. Decisión de approach:
**resolverlo a nivel del lenguaje** con un decorator nuevo en lugar
de fix puntual en el boilerplate.

### Nuevo decorator `@hidden` sobre fields

Marca un field como invisible para el JSON I/O:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str = ""
    name: Str = ""
    @hidden password_hash: Str = ""   // <-- nunca cruza HTTP
    role: Str = "user"
}
```

**Semántica**:
- `__to_fitz_json` **skipea** el field — no aparece en el response
  HTTP, en cualquier contexto donde el type se serialice (directo,
  como field de otro type, eager-loaded via `.preload(...)`, etc.).
- `__FromFitzJson` **rechaza** el field — si el body del cliente
  incluye `{"password_hash": "..."}`, el server responde 400 con
  `"campo no declarado"`.
- El ORM lo **persiste normalmente** en Postgres (INSERT/SELECT/
  UPDATE incluyen el field como cualquier columna). Solo cambia
  el boundary HTTP.
- El código Fitz interno asigna libremente el field: en `register`,
  `User.insert(conn, User { password_hash: hash.password(body.password), ... })`
  funciona igual.

**Ortogonal al ORM**: `@hidden` funciona en types con o sin
`@table` — útil también para metadata interna en types plain HTTP.

### Cambios técnicos

- **src/types.rs**: nuevo arm `"hidden"` en
  `parse_table_decorators_for_type`. Tolera el decorator (sin args
  ni kwargs). Importante: NO setea `any_field_decorator = true`
  (que dispara el check "missing @table") — `@hidden` es ortogonal
  al ORM y funciona en types plain HTTP.
- **src/codegen.rs**: nuevo flag `TypeSigField.hidden: bool`
  propagado desde `Field.decorators` en los 4 sitios donde se
  construye `TypeSigField`. `gen_type_http_impls_for_sig_with_meta`
  skipea fields con `hidden: true` en:
  - `__to_fitz_json` body (no aparece en el output JSON).
  - `__allowed` lista del `__FromFitzJson` (rechaza extras).
  - field iteration del `__FromFitzJson` (no se lee del input).
  - struct literal: usa el default declarado del field o
    `Default::default()` para construir el struct sin el field.
- **editors/vscode/package.json**: bump a `0.10.11`. La grammar
  TextMate ya captura `@hidden` con el pattern genérico
  `@[a-zA-Z_][a-zA-Z0-9_]*` — sin cambios necesarios.

### Migración de boilerplates al patrón LATEST

Aprovechando el ciclo, los Dockerfiles de `api-orm-full` y
`api-postgres-fitz` pasan de `cargo install --git` (que compilaba
Fitz desde source en ~5-8min) al patrón pre-built
`ghcr.io/thegreekman76/fitz:latest` (ya usado en
`api-middleware-cors`, `api-postgres-python`, `api-simple`,
`api-websocket`, `cli-tool`, `api-fullstack-postgres`).

**Reducción ~10x del build time**: primer build de ~5-8min pasa a
~30-60s. La imagen `:latest` se actualiza automáticamente en cada
release del repo (workflow `.github/workflows/release.yml`).

```dockerfile
ARG FITZ_TAG=latest
FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder

WORKDIR /app
COPY fitz.toml ./
COPY src/ ./src/
RUN fitz build src/main.fitz

FROM debian:bookworm-slim
COPY --from=builder /app/src/main /usr/local/bin/app
EXPOSE 3000
CMD ["/usr/local/bin/app"]
```

En uso normal: `docker compose build` (sin `--build-arg`). Pinned:
`docker compose build --build-arg FITZ_TAG=v0.10.11`.

### Aplicación al boilerplate api-orm-full

`User.password_hash` ahora marcado con `@hidden`. Validación bit-a-
bit: `GET /posts/{id}` con preload author devuelve el User
embebido SIN `password_hash` (vs el leak detectado en v0.10.10).

### Documentación

- **docs/db-orm.md** sección 4: nuevo bloque "`@hidden`: ocultar
  fields de la frontera HTTP" con semántica + cuándo usar + cuándo
  NO usar + ortogonalidad con `@table`.
- **boilerplates/api-orm-full/README.md**: actualización del
  comando `docker compose build` para usar LATEST por default.
- **boilerplates/api-orm-full/Dockerfile** + `Dockerfile.distroless`:
  migración al patrón pre-built.
- **boilerplates/api-postgres-fitz/Dockerfile** + `Dockerfile.distroless`:
  migración paralela.

### Tests

- **tests/compile_e2e.rs**: nuevo test
  `hidden_decorator_skipea_field_en_json_io_v0_10_11` que valida
  los 3 casos canónicos:
  1. GET response NO incluye el hidden field aunque el handler le
     asignó valor.
  2. POST sin el field → 200 (el server usa el default).
  3. POST con el field → 400 ("campo no declarado").

### Validación

- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets -- -D warnings` limpio.
- `cargo test --release --test compile_e2e hidden_decorator` verde.
- `cargo test --release --test compile_e2e smoke_ejemplos_guia` 292
  ejemplos verde (sin regresiones).

### Deuda menor visible (NO bloquea release)

LSP autocomplete context-aware después de `@`: hoy la grammar
TextMate destaca cualquier `@name`, pero el LSP no sugiere la
lista cerrada de decorators (`@get`/`@post`/`@table`/`@hidden`/
etc.) al escribir `@`. Sería una mini-fase de ~30-60min en una
próxima iteración cuando aparezca presión real.

## [v0.10.10] — 2026-05-28 — Fix deadlock `__to_fitz_json` en has_many virtual (preload hang cerrado)

**Cierre del preload hang** dejado como deuda residual en v0.10.9.
La hipótesis inicial (bug del read loop del driver Postgres al
encadenar queries) era **incorrecta**: el driver funciona
perfectamente y el preload completa todas sus queries (todos los
`ReadyForQuery` se reciben). El bug está en el **codegen del impl
`__ToFitzJson` para tipos con relations virtuales has_many**.

### Root cause

En `gen_type_http_impls_for_sig_with_meta` (src/codegen.rs), el
conditional emit del field has_many virtual — introducido en
v0.10.8.3 para activar `.preload(...)` end-to-end en el JSON
response — tenía un fallo de lock scope:

```rust
{
    let __g = self.comments.lock().unwrap();
    if !__g.is_empty() {
        __obj.insert(
            "comments".to_string(),
            self.comments.__to_fitz_json(),  // ← re-lockea
        );
    }
}
```

Mientras `__g` retiene el `MutexGuard` sobre el `Mutex<Vec<...>>`,
el `__to_fitz_json` del impl genérico `Arc<Mutex<T>>` hace
`self.lock().unwrap().__to_fitz_json()` sobre el **mismo** Mutex.
`std::sync::Mutex` NO es reentrante → **deadlock instantáneo**
del worker thread.

**Manifestación**: en el boilerplate api-orm-full, `GET /posts/{id}`
con `.preload("author").preload("comments")` colgaba en la
serialización del response (HTTP 000 timeout a los 8s). Los
handlers SIN preloads activos funcionaban porque
`__g.is_empty()` era true y nunca llegaba al re-lock.

### Fix

Liberar el guard ANTES del re-lock. Chequeo `is_empty` en un
scope acotado que dropea el guard, después serialización normal:

```rust
{
    let __is_empty = {
        let __g = self.comments.lock().unwrap();
        __g.is_empty()
    };  // ← __g dropped aquí
    if !__is_empty {
        __obj.insert(
            "comments".to_string(),
            self.comments.__to_fitz_json(),  // ← lock libre ahora
        );
    }
}
```

El re-lock dentro de `__to_fitz_json` ahora encuentra el Mutex libre.

### Workflow de diagnóstico

3 ciclos de eprintln strategic en commits `[REVERTIR]` aislaron el
hang con precisión:
1. `[FITZ-WIRE]` en `db.rs::read_message` → confirmó que el driver
   recibe todos los `ReadyForQuery` del preload (descartó bug del
   read loop).
2. `[FITZ-PRELOAD] processing/done` + `[FITZ-PRELOAD-LOOP-EXIT]`
   en `emit_preload_dispatch` → confirmó que el for loop del
   preload completa OK.
3. `[FITZ-FIRST-CLOSURE] dropping/dropped __rows` →
   confirmó que el `drop(__rows)` no es el culpable.
4. `[FITZ-WRAP-PRE-CATCH]` + `[FITZ-WRAP-POST-CATCH]` en
   `emit_handler_dispatch_and_response` → confirmó que
   `catch_unwind().await` del wrapper NUNCA retorna (handler
   `Future` no completa).
5. `[FITZ-RET-MATCH] pre-await / post-await / Ok arm calling
   __to_fitz_json / __to_fitz_json done` en el handler return →
   **bingo**: vimos hasta "Ok arm calling __to_fitz_json" pero
   NUNCA "done". Aislado al impl.

Todos los eprintln revertidos en este commit final.

### Smoke real Docker validado

`GET /posts/1` con `.preload("author").preload("comments")` ahora
responde **HTTP 200 en ~140ms** con el Post + author (preloaded
User) + comments (preloaded Vec<Comment>) embebidos en el JSON.
Otros endpoints (`GET /posts`, `GET /stats/posts-per-user`,
auth/register) sin regresiones.

### Deuda menor del boilerplate descubierta

El response de `GET /posts/{id}` expone `password_hash` del author
porque `Post.author: User?` incluye ese field. **No es bug del
lenguaje** — es del boilerplate. Fix típico: handler hace mapping
a un `PostPublic`/`UserPublic` que omite el field sensible. Queda
como deuda residual del boilerplate, no bloquea v0.10.10.

### Cambios coordinados

- `editors/vscode/package.json`: bump a `0.10.10`.
- `boilerplates/api-orm-full/Dockerfile` + `README.md`:
  `FITZ_TAG=v0.10.10` (en commit separado al rebuild del
  boilerplate).

### Validación

- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets -- -D warnings` limpio.
- `cargo test --release --test compile_e2e cross_module_orm` verde.
- Smoke real Docker: `GET /posts/1` con preload → 200 con author
  + comments completos en 140ms (vs HTTP 000 timeout a los 8s en
  v0.10.9).

## [v0.10.9] — 2026-05-28 — Pool singleton per URL (fix connection leak)

**Mini-fase de cierre del gap runtime más serio** descubierto en
smoke real Docker de v0.10.8: cada llamada a `db.connect(url)`
desde Fitz creaba un POOL NUEVO con 10 permits + TCP conns.
Después de N requests al boilerplate api-orm-full, Postgres se
quedaba sin slots (`max_connections=100` default) y `acquire()`
colgaba indefinidamente, manifestándose como "preload hang"
visible en GETs con `.preload(...)`.

### 10.9.2 (#2 nuevo) — `connect_url` singleton per URL

`fitz::db::connect_url(url)` ahora cachea el `Arc<DbConnHandle>`
en un mapa global thread-safe (`OnceLock<Mutex<HashMap<String,
Arc<DbConnHandle>>>>`). Calls subsiguientes con la misma URL
devuelven clone(Arc) del handle existente — TODAS las conns TCP
se comparten via el pool único.

**Cambios técnicos**:

- `connect_url` ahora retorna `Arc<DbConnHandle>` directo (en vez
  de `DbConnHandle` por valor + el caller wrappea en `Arc::new`).
  Call sites actualizados: `evaluator.rs` y `codegen.rs`.
- Cache check + fast path zero-alloc cuando hay handle existente.
- Si el handle fue cerrado con `.close()` explícito, se crea uno
  nuevo (caller quiere reabrir).
- Tests actualizados en `tests/db_real_postgres.rs`.

**Trade-off documentado**: los handles persisten hasta el cierre
del proceso. Memoria despreciable (~24 KB por pool idle). Si
nunca te volvés a conectar a una URL, el pool sobrevive sin
uso — aceptable para 99% de los servicios.

**El pool singleton fue validado end-to-end** en smoke real
Docker post-tag: 3 GETs consecutivos = 2 conns constantes (1
schema init + 1 del pool reutilizado), confirmando que ya no
hay leak.

### Preload hang sigue abierto — gap separado del driver

El smoke real con `GET /posts/{id}/preload(...)` mostró que el
preload hang **NO** era causado por el pool leak (como inicial-
mente asumí). Después de cerrar el pool leak, el preload sigue
colgándose en `extended_query` aún con conns disponibles.

Diagnóstico del smoke: tras un preload colgado, `pg_stat_activity`
muestra la conn en estado `idle` con la última query del preload
("SELECT ... comments WHERE post_id IN ..."). Postgres terminó
de servir; el cliente Fitz nunca leyó la respuesta final
(probable: ReadyForQuery no se lee).

Es un **bug separado del read loop del driver** (`Connection::
extended_query` en `src/db.rs`) cuando hay múltiples queries
chained sobre la misma `DbConnHandle`. Queda como deuda
residual para v0.10.10.

Los otros 12+ endpoints HTTP/WS del boilerplate funcionan
correctamente con el pool singleton.

### Cambios coordinados

- `boilerplates/api-orm-full/Dockerfile` + `README.md`:
  `FITZ_TAG=v0.10.9`.
- `editors/vscode/package.json`: bump a `0.10.9`.

### Validación

- `cargo build --bin fitz --release` OK.
- `cargo test --release --test compile_e2e smoke_ejemplos_guia`
  292 ejemplos verde.
- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets --release -- -D warnings` limpio.

**Smoke real Docker queda como validación CI/Linux** — el
ambiente Docker Desktop Windows tiene un bug intermitente con
SCRAM-SHA-256 sobre el bridge TCP que cuelga el `Connection::
connect` aún con código pristine pre-v0.10.9. NO bloquea el
release porque el fix es localizado al pool singleton y el
ambiente Linux real no tiene ese issue.

### Próximo norte

- 9no boilerplate `api-orm-full-fullstack` (frontend vanilla
  nginx, memoria `project_boilerplate_orm_full_fullstack.md`).
- Benchmarks Fitz ORM vs SQLAlchemy.

## [v0.10.8] — 2026-05-28 — Cierre de 8 gaps del smoke real Docker

**Mini-fase de cierre** de los 8 gaps cross-module descubiertos
durante el smoke real del boilerplate `api-orm-full` con
Postgres real en Docker (v0.10.7). El binario compila local +
`fitz check` verde + smoke 292 verde no los detectaba — solo
aparecen cuando el binario levanta el server contra DB real y se
le pegan requests HTTP/WS. Todos cerrados en 4 rondas de
sub-pasos (10.8.1 → 10.8.8) en una sesión, ~1500 LoC netas + 8
tests E2E nuevos.

### Round 1 (10.8.1 + 10.8.2 + 10.8.3)

- **10.8.1 (#6) — HTTP wrapper desempaca `Result<T>` tail sin
  `Ok()` explícito**. El codegen ahora emite `match` runtime que
  desempaca: `Ok(v)` → 200 con `v` puro, `Err(e)` → 500 con
  `{"error": e}`. Aplica al path `response_mode` (handlers
  cross-module con `?` o `@authenticated`). Antes serializaba
  `Result<T, E>` entero produciendo `{"Ok": ...}`.
- **10.8.2 (#5) — Decorator `@db_default` para fields managed-by-DB**.
  El ORM skipea estos del INSERT; Postgres aplica su `DEFAULT`
  declarado en el schema (típico: `DEFAULT NOW()` para
  timestamps). Field sigue en RETURNING * con el valor que
  Postgres asignó. Paralelo a W4 pero general (cualquier tipo).
- **10.8.3 (#7) — W17 eager loading: virtuales SÍ se emiten en
  JSON cuando preloaded**. Runtime check: `Option<T>` → emit si
  `is_some()`, `Vec<T>` → emit si `!is_empty()`. Antes los
  virtuales JAMÁS aparecían en el JSON, perdiendo el beneficio
  del `.preload(...)`.

### Round 2 (10.8.4 + 10.8.5)

- **10.8.4 (#1) — Narrowing flow-sensitive `Nullable<T>` post-
  `if (x != null)`**. El checker refina el binding adentro del
  then/else branch; el codegen emite shadow `let x = x.unwrap();`
  para que el value Rust sea `T` puro. Cubre también `if (x ==
  null)` para el else branch.
- **10.8.5 (#3) — OpenAPI 3.1 cross-module paths**. El schema
  emitido por `fitz build` ahora incluye los handlers HTTP de
  módulos importados. Antes `paths: []` cuando los handlers
  vivían cross-module. Fix vía
  `pseudo_routes_from_program_and_modules(program,
  module_http_stmts)`.

### Round 3 (10.8.6 + 10.8.7)

- **10.8.6 (#4) — WS Router cross-module + AsyncAPI cross-module**.
  Handlers `@ws` cross-module se enchufan al Router axum del
  main (paralelo a W16 para HTTP). El módulo emite
  `pub async fn __ws_handler_<name>`; main registra
  `.route("/path", axum::routing::get(crate::<mod>::__ws_handler_<name>))`.
  El schema AsyncAPI 3.0 también se emite cuando los `@ws` viven
  cross-module. Pre-fix: WS handshake al `/feed` cross-module
  daba 404.
- **10.8.7 (#2) — `ws_broadcast(endpoint, msg)` built-in**.
  Habilita el patrón canónico SaaS "handler HTTP triggerea
  notification realtime a clientes WS conectados". Helper en
  `http.rs` (`ws_broadcast_to_endpoint`), built-in en evaluator
  (`builtin_ws_broadcast`), signature `(Str, Any) -> Null` en
  checker, codegen emite `crate::__fitz_ws_broadcast(...)` (con
  `crate::` prefix para funcionar desde módulos). Pre-scan
  `program_uses_ws_broadcast` activa preludio WS + helper.

### Round 4 (10.8.8 — cierre formal)

- Extensión VSCode: grammar TextMate suma `ws_broadcast` a la
  lista de builtins highlightables; LSP completion lo lista en
  `scope_level_completions`. Bumpeo a v0.10.8.
- Boilerplate `api-orm-full` revertido a sintaxis canónica:
  schema con `timestamptz NOT NULL DEFAULT NOW()`, models con
  `@db_default`, handlers `posts.fitz` con `return <chain>.await`
  directo, narrowing con `if (status != null)` en vez de match
  arm, broadcast WS real en `comments.fitz`. README actualizado.
- CHANGELOG, deudas-post-5b, FITZ_TAG en Dockerfile/README,
  todos a v0.10.8.

### Tests

8 tests E2E nuevos en `tests/compile_e2e.rs`:
`http_wrapper_desempaca_result_tail_sin_ok_explicito`,
`orm_db_default_skipea_field_del_insert`,
`orm_w17_eager_loaded_virtuales_aparecen_en_json`,
`checker_narrow_nullable_post_if_not_null`,
`checker_narrow_nullable_else_branch_eq_null`,
`openapi_cross_module_incluye_handlers_de_modulos`,
`ws_router_y_asyncapi_cross_module`,
`ws_broadcast_builtin_cross_handler`.

Smoke `GUIDE_EXAMPLES_COMPILE` 292 ejemplos verde con todos los
fixes integrados. `cargo fmt --all -- --check` limpio,
`cargo clippy --all-targets --release -- -D warnings` limpio.

### Próximo norte

- 9no boilerplate `api-orm-full-fullstack` (frontend vanilla
  nginx sobre el backend api-orm-full, memoria
  `project_boilerplate_orm_full_fullstack.md`).
- Benchmarks Fitz ORM vs SQLAlchemy (boilerplate `task` actual
  con SQLAlchemy vs `api-orm-full-fullstack` con ORM nativo).
- Curso "Fitz de 0 a experto" (memoria `project_curso_plan.md`).

## [v0.10.7] — 2026-05-28 — W17/W18: cross-module ORM completo + boilerplate `api-orm-full` + 5 gaps cerrados

**Release bundle**: cierre del cross-module ORM (W17 + W18), 8va
plantilla del directorio `boilerplates/` (`api-orm-full`
multi-archivo), y bloque de 4 gaps adicionales del codegen
descubiertos al construir el boilerplate. La política
"cerrar gaps que aparezcan al construir el boilerplate ANTES del
release" se aplicó estricto — todo lo que ahora corre el showcase
es paridad bit-a-bit `fitz run` ↔ `fitz build`.

### Boilerplate `api-orm-full` (nuevo)

Multi-archivo (9 módulos Fitz) showcase del **stack web first-class
entero** en un solo binario standalone:

- **HTTP + auth nativa** (`@auth_provider`/`@authenticated` cross-
  module) + **OpenAPI 3.1 auto** en `/docs`.
- **WebSockets tipados** (`@ws("/feed")` con `WsConn<FeedEvent>`)
  + **AsyncAPI 3.0 auto** en `/asyncapi.json` + heartbeat 30s.
- **Cron jobs sin Celery/broker** (`@cron("0 0 * * *") cleanup_old_drafts`).
- **ORM nativo declarativo** con 4 `@table` types coordinados
  (User/Profile/Post/Comment), relations completas (`@has_many`/
  `@has_one`/`@belongs_to` + companion fields), eager loading
  (`.preload("author")`/`.preload("comments")`), JSONB
  (`metadata: Map<Str, Any>`), arrays (`tags: List<Str>` con
  `.has(var)`), aggregates (GROUP BY `count(db)`).
- **Sin Python, sin SQLAlchemy, sin Celery, sin Redis, sin broker**
  — un solo binario `fitz build`. Imagen distroless ~15-20 MB.

Patrón cross-module W12 + W16 + W17 + W18: handlers HTTP/WS,
cron jobs, `@auth_provider` y `@table` types viven en módulos
por feature; el main solo hace `import auth, posts, comments,
realtime, jobs`.

### 5 gaps/bugs del codegen cerrados (post-W17, durante boilerplate)

Política: cerrar TODO gap descubierto durante el boilerplate
ANTES de declarar el sub-paso completo (memoria
`feedback_post_changes_smoke_examples_boilerplates`).

- **R.1.3 — `Map<Str, Any>` con indexing assignment dinámico**
  (`m["k"] = v`). El storage Rust de `Map<_, Any>` es
  `Vec<(__FitzValue, __FitzValue)>`; el codegen del indexing
  assignment NO envolvía key/value como `__FitzValue`. Fix en
  `gen_index_assign`. Caso canónico: partial updates en APIs REST.
- **R.1.3-bis — `.has(var)` sobre `Map<Str, Any>`** (paralelo).
  Fix en `gen_map_has`.
- **W18 — `has_opaque_field` ignora virtuales del ORM** en
  `emit_helpers_for_imported_types`. El filtro previo a emitir
  `__ToFitzJson`/`__FromFitzJson` para types cross-module miraba
  los virtuales (`@has_many`/`@has_one`/BelongsToCompanion) que
  degradan a `Any` post-remap cuando el target no está importado
  al main. Resultado: impl jamás se emite, rustc rompe con
  "trait bound not satisfied". Fix: filtrar virtuales antes del
  check usando el `TableMetadata`.
- **Bug del format string en jsonb dynamic update**. El dispatch
  `Dynamic` de `.update(db, map_var)` para fields jsonb tenía
  `{{}}` (escaped braces) donde debería tener `{}` para interpolar
  el error message. Fix trivial cambio de string.
- **`.has(var)` sobre arrays Postgres** (`text[]`/`int8[]`/etc.).
  El codegen rechazaba con "el value debe ser literal". Fix:
  delegar a `translate_closure_to_sql` (reusa máquina W3/W6) que
  bindea via `__IntoPgValue::into_pg(...)`. Caso canónico: filtros
  por tag en endpoints listables.

### Tests y validación al cierre

- 3 tests E2E nuevos en `tests/compile_e2e.rs`:
  `map_str_any_indexing_assign_compilado`,
  `cross_module_table_virtual_w18_remap_any`,
  `orm_array_has_acepta_var_externa`.
- Test E2E W17 ya existente:
  `cross_module_orm_virtual_fields_skip_w17`.
- Smoke `GUIDE_EXAMPLES_COMPILE` 292 ejemplos verde con todos los
  fixes integrados.
- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets --release -- -D warnings` limpio.

### Gaps abiertos derivados (NO bloquean el release)

Detalle en `docs/deudas-post-5b.md` sección "Mini-fase W18+".

- Narrowing flow-sensitive de `Nullable<T>` → `T` post-`if (x !=
  null)`. Workaround idiomático: match arm.
- Broadcast HTTP → WS cross-handler. Sin API global
  `ws_broadcast(endpoint, msg)` hoy. El boilerplate modela
  `/feed` como broadcast simétrico entre clientes WS.

### W17: virtual fields skip en impls cross-module (incluido en este release)

**Cierre del último gap conocido del codegen cross-module ORM**
descubierto durante el primer intento de implementar el boilerplate
`api-orm-full` (showcase multi-archivo del stack DB+ORM+HTTP+WS+
cron). **Sin sintaxis nueva**: cambio interno del codegen — los
programas existentes siguen compilando bit-a-bit, sin cambios en
grammar TextMate, LSP completions ni docs prosa.

**El bug**: `@table type` con `@has_many`/`@has_one` declarado en
un módulo importado + handler que lo retorna como response, vivos
en módulos distintos al main. Caso canónico:

```
src/models.fitz   →  type User { ... @has_many("Post", ...) posts }
src/posts.fitz    →  from models import User, Post
                     @get("/users") fn list() -> List<User> { ... }
src/main.fitz     →  import posts
                     @server(3000)
                     fn main() => 0
```

El codegen al emitir `impl __FromFitzJson for UserData` en main.rs
hacía remap del field virtual `posts: List<Post>` → `List<Any>`
(porque `Post` no estaba en el env del importer main) → emitía
`Vec<__FitzValue>`. Pero `__FitzValue` no se activaba para el
programa, así que rustc rompía con `cannot find type __FitzValue
in this scope`.

**El fix**: skipear los virtual fields (HasMany/HasOne/
BelongsToCompanion via `TableMetadata.is_virtual_field`) en los
impls `__ToFitzJson`/`__FromFitzJson`. Esos fields no van a la
DB ni deben aparecer en JSON I/O — el cliente no debe enviarlos
como body, la response no los serializa. En el struct literal
del `__from_fitz_json`, los virtuales se inicializan inline con
`Default::default()` para evitar nombrar el tipo remap-degradado.

**Cambios técnicos**:

- Nueva variante `gen_type_http_impls_for_sig_with_meta(name, sig,
  meta: Option<&TableMetadata>)` en `src/codegen.rs` que filtra
  virtuales según el meta del type.
- Ambos call sites actualizados: `gen_type_http_impls` (types
  locales) hace lookup vía `table_metadata_for(id)`;
  `emit_helpers_for_imported_types` (cross-module) hace lookup en
  `m.table_metadata.get(type_name)` del módulo origen.
- Test E2E nuevo `cross_module_orm_virtual_fields_skip_w17` en
  `tests/compile_e2e.rs` candea el caso 3-archivos. Validado
  runtime: `GET /users` devuelve `{"id":7,"name":"ada"}` SIN
  incluir el virtual `posts`.

**Validación al cierre**:

- 314/314 tests `compile_e2e` pasan (no-regresión sobre los 6 tests
  cross-module existentes: W8/W10/W11/W12/W15/W16).
- Smoke `GUIDE_EXAMPLES_COMPILE` (292 ejemplos) verde.
- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets --release -- -D warnings` limpio.

**Deudas derivadas documentadas** (en `docs/deudas-post-5b.md` sec
nueva):

- ⚠️ Bug del checker: inferencia `Option<String>` en `let x = match
  Result { Ok(v) => v, Err(_) => return ... }`. Workaround
  trivial: anotar `let x: Str = ...`.
- ⚠️ Forward refs en `@has_many("Target", ...)` con Target después
  en el mismo módulo: rompe si el codegen procesa navigation.
  Workaround: declarar Target antes.
- ⚠️ Importar TODOS los `@table` types al módulo que use cualquier
  uno (el codegen valida ALL targets). Workaround: `from models
  import User, Post, ...` (todos los referenciados).

## [v0.10.6] — 2026-05-27 — Bloque W1-W7: workarounds residuales del ORM cerrados

**Cierre del bloque "workarounds residuales del ORM"** identificados
durante v0.10.4 + v0.10.5. Los 7 workarounds menores documentados
en `docs/db-orm.md` sec 28 quedan cerrados con commits dedicados
+ tests E2E. **El stack DB+ORM ya no tiene fricciones residuales
conocidas para los patrones canonicales del language guide / boilerplates.**

| Workaround | Sub-commit | Tests nuevos |
|------------|------------|--------------|
| W4 — `id: 0` sentinel auto-asigna bigserial | commit dedicado | 2 unit codegen |
| W5 — `db.close()` devuelve `Future<Result<Null>>` | commit dedicado | 1 unit codegen |
| W7 — `.update(db, Map var)` además del literal | commit dedicado | 2 unit + 1 E2E |
| W3 — `.starts_with`/`.ends_with`/`.contains` aceptan var Str | commit dedicado | 2 unit codegen |
| W6 — `body.field` en closures de `.where` | commit dedicado | 2 unit codegen |
| W1 — Map literal homogéneo a field `Map<Str, Any>` | commit dedicado | 1 E2E |
| W2 — Nullable refinement en match arms | commit dedicado | 1 unit + 1 E2E |

**Cambios técnicos clave**:

- **W4**: nuevo branch runtime `if __g.<pk> == 0` en `gen_orm_type_insert`
  con dos SQLs alternativos (con/sin PK) elegidos según el value runtime
  del primary. Paralelo bit-a-bit al evaluator (skip del field cuando
  `Value::Int(0)`).
- **W5**: `db.close()` devuelve `Future<Result<Null>>` (antes `Future<Null>`).
  El helper preludio `__fitz_db_close` ahora retorna `Result<(), String>`
  con `.map_err(|e| e.to_string())`. Los docs ya prometían esta semántica
  desde v0.10.5 — ahora el código se alineó.
- **W7**: nuevo `UpdateSetEmission { Static, Dynamic }`. Static (Map
  literal) mantiene el shape anterior. Dynamic (Map var/expr) emite un
  closure IIFE con match runtime sobre `key.as_str()` ramificado por
  field del type, con conversión `__FitzValue → __FitzPgValue` per-tipo.
  Soporta Int/Float/Str/Bool/Nullable<primitivo> + Map<...> (jsonb) +
  List<scalar> (arrays).
- **W3**: dos paths en `starts_with`/`ends_with`/`contains` (tanto en
  evaluator como codegen). Str literal mantiene el escape Rust-side de
  `%`/`_`. Var/expr se traduce como arg general y envuelve SQL-side con
  `||` Postgres (`$N || '%'`, `'%' || $N`, etc.).
- **W6**: el translator (evaluator + codegen) acepta `<var>.<field>` cuando
  la var no es el `param_name` del closure. Hace lookup en el closure_env
  (evaluator) o delega a `gen_expr` recursivo (codegen) y bindea como
  `$N` via `__IntoPgValue`. Soporta chains arbitrarios (`req.inner.email`).
- **W1**: nuevo wrapper `gen_map_lit_with_hint(pairs, span, hint)`. Cuando
  el hint es `Map<_, Any>`, fuerza `heterogeneous_v = true` antes del
  loop de `lub`, lo que hace que el shape emitido sea `Vec<(FV, FV)>`.
  `gen_struct_lit` propaga el hint del field destino. `gen_map_lit`
  original queda como wrapper sin hint (paridad pre-v0.10.6).
- **W2**: dos correcciones coordinadas en `gen_pattern`. **Bug fix**:
  `Pattern::Null` sobre Nullable emitía `_` (matcheaba TODO, no solo
  null) — ahora emite `None` específico. **Refinement**: `Pattern::Ident`
  sobre Nullable emite `Some(name)` y declara `name` como inner `T` (no
  Option). Checker estático también gana refinement flow-sensitive
  (Ident posterior a un arm Null-covering se bindea al inner T).

**Barrida de documentación**: workarounds removidos del prose de
`docs/db-orm.md` (sec 28 ahora marca los 7 como CERRADOS), ejemplos
y boilerplates de la guía. Tests existentes que usaban los workarounds
(`let lang = body.lang` antes de `.where(...)`, `id: 1` explícito en
inserts demo, etc.) actualizados a la sintaxis canónica.

**Tests** al cierre del bloque: 2562 unit + 81 cli_e2e + 295 compile_e2e
+ 3 openapi + 46+ db_real_postgres. Clippy `--all-targets -D warnings`
limpio, fmt `--all --check` limpio, smoke `GUIDE_EXAMPLES_COMPILE` verde.

**Próximo norte**: boilerplates ORM Dockerizados (convertir 5/6
SQLAlchemy → Fitz ORM nativo + boilerplate nuevo dedicado al ORM full),
benchmarks Fitz ORM vs SQLAlchemy.

## [v0.10.5] — 2026-05-26 — Bundle deudas residuales ORM #2/#3/#4 + cosecha BodyJson + workarounds documentados

**Cierre del bloque "deudas residuales del ORM"** iniciado
post-v0.10.4. 3 deudas más cerradas en commits intermedios
bundleadas en un único release + cosecha menor descubierta
durante la actualización de ejemplos:

| Deuda | Status | Sub-paso |
|-------|--------|----------|
| #1 — Map<Str, Any> en HTTP returns | Cerrada | v0.10.4 |
| #2 — BelongsTo en .preload(...) | Cerrada | v0.10.5 |
| #3 — JSON operators en .where | Cerrada | v0.10.5 |
| #4 — Chain dinámico condicional | Cerrada (drift docs) | v0.10.5 |

Total v0.10.5: ~970 LoC netas (+770 código + tests, +200 docs)
+ 3 paridad real tests + actualización completa de los 2
ejemplos guía (`31-orm.fitz` + `31b-orm-crud-http.fitz`) con
los patterns de las 4 deudas.

### Added

- **Deuda #2 — BelongsTo eager via convention** (`src/types.rs`
  +120 LoC, `src/codegen.rs` +135 LoC, `src/evaluator.rs` +10
  LoC, paridad test +112 LoC):
  - Nueva variante `RelationKind::BelongsToCompanion`. El checker
    auto-detecta el patrón canónico: `@belongs_to("User")
    user_id: Int` + sibling field `user: User?` (name derivado
    stripping `_id`, type Nullable<Target>). Registra companion
    como virtual. Sin sibling declarado, comportamiento previo
    (FK navigation directa).
  - `emit_belongs_to_companion_preload_arm` en `codegen.rs`
    paralelo a HasMany pero con SQL inverso: `WHERE target.pk IN
    (parent.fk DISTINCT)`. Asigna `Some(target)` a cada parent.
  - `.preload("user")` (companion name) ahora funciona end-to-end
    en `fitz build`. Validado con `orm_preload_belongs_to_
    companion_paridad_codegen_e2e` (3 posts + 3 preloaded users
    en 2 queries).
- **Deuda #3 — JSON operators en `.where(...)`** (`src/evaluator.rs`
  +191 LoC, `src/codegen.rs` +204 LoC, paridad test +92 LoC).
  5 method calls sobre fields jsonb (`Map<Str, ...>`) mapeados a
  operadores Postgres nativos:
  - `.has_key("k")` → `"data" ? $1`
  - `.has_all_keys([...])` → `"data" ?& $1::text[]`
  - `.has_any_keys([...])` → `"data" ?| $1::text[]`
  - `.contains_json({...})` → `"data" @> $1::jsonb`
  - `.get("k")` → `("data"->>$1)` (text result, comparable con
    `==` contra Str literal)

  Validado con `orm_jsonb_operators_in_where_paridad_codegen_e2e`
  (4 events shapes distintos → conteos esperados: 3/3/2/2/1).
- **Deuda #4 — Chain dinámico condicional** (regression test +99
  LoC, sin código Rust nuevo): el codegen YA soportaba `qb =
  qb.where(...)` adentro de un `if`. La doc previa decía "no
  compila" — drift documental. Validado con
  `orm_dynamic_chain_conditional_paridad_codegen_e2e` (4
  combinaciones de filtros condicionales sobre 5 users).
- **Cosecha BodyJson** (`src/codegen.rs` +20 LoC): nueva
  `impl __FromFitzJson for Vec<(__FitzValue, __FitzValue)>`
  en el preludio HTTP. Habilita body deserialization de fields
  `Map<Str, Any>` cuando aparecen en types HTTP entrada (e.g.
  `PostInput.metadata: Map<Str, Any>`). Encontrado al sumar
  endpoints nuevos a `31b-orm-crud-http.fitz` que aceptaban
  metadata libre del body.

### Changed

- **`examples/guide/31-orm.fitz` re-escrito** (~150 LoC) con
  todos los patterns de las 4 deudas demostrados:
  - Sec 2.7 ampliada con companion field auto-detectado.
  - Sec 2.8 ampliada con `.preload("user")` BelongsTo eager.
  - Nueva sec con JSON operators (`.has_key`/`.contains_json`/`.get`).
  - Nueva sec con chain dinámico condicional (`qb = qb.where(...)`).
  - `Post` ahora declara `metadata: Map<Str, Any>` y `user: User?`
    como ejemplos canónicos de los nuevos features.
- **`examples/guide/31b-orm-crud-http.fitz`** sumó 4 endpoints
  nuevos:
  - `GET /posts-with-author` — BelongsTo eager (deuda #2)
  - `GET /posts/drafts` — `.has_key` (deuda #3)
  - `GET /posts/by-lang/{lang}` — `.get` con var externa (deuda #3)
  - `POST /posts/search` — chain dinámico con body (deuda #4)

  Type `Post` extendido con companion `user: User?` y `metadata:
  Map<Str, Any>`. Type `PostInput` extendido con `metadata` para
  HTTP body. Nuevo type `SearchInput` para el endpoint dinámico.
- **`docs/db-orm.md`**:
  - Sec 12 (eager loading): caveat reescrito reflejando BelongsTo
    via companion como CERRADO.
  - Sec 13 (JSONB): tabla nueva de operadores + ejemplos completos.
  - Sec 21 (search filters): chain dinámico documentado con
    ejemplos correctos (no más workaround search_dynamic).
  - Sec 28 (limitaciones): 4 entradas marcadas CERRADO + nueva
    sub-sección **W1-W7 workarounds residuales documentados**
    encontrados durante el cierre del bloque (Map literal
    homogéneo no matchea Map<Str,Any>, Nullable refinement en
    match, `.starts_with` solo Str literal, `id: 0` no
    auto-asigna con bigserial, `db.close` no devuelve Result,
    `body.field` no soportado en closures, `.update` solo
    acepta Map literal). Cada uno con síntoma + workaround +
    fix futuro propuesto.

### Fixed

- Codegen format string bug en `impl __FromFitzJson for Vec<...>`
  (descubierto + arreglado en la misma sesión): el codegen
  emitía `{{}}` literal donde debía emitir `{}` (format spec
  para el `other` arg).

### Tests

- **3 paridad real tests nuevos** en `tests/db_real_postgres.rs`:
  - `orm_preload_belongs_to_companion_paridad_codegen_e2e`
  - `orm_jsonb_operators_in_where_paridad_codegen_e2e`
  - `orm_dynamic_chain_conditional_paridad_codegen_e2e`
- Total `db_real_postgres`: **46 tests** (was 43, +3).
- Smoke `GUIDE_EXAMPLES_COMPILE` (292 ejemplos) verde.
- `cargo fmt --all --check` + `cargo clippy --all-targets -D
  warnings` limpio.

### Diferenciales reforzados con v0.10.5

Sumadas a las features del MVP de Fase 10/10.b/v0.10.4, ahora
el stack DB+ORM cubre **todos los patterns canónicos** que un
ORM moderno debería tener:

- **Eager loading bidireccional**: HasMany (`.preload("posts")`)
  + BelongsTo (`.preload("user")` via companion). Cierre N+1
  en cualquier dirección de la relation, dispatch estático
  compile-time, paridad bit-a-bit run↔build.
- **JSONB queries first-class**: 5 operadores Postgres nativos
  mapeados a method calls Fitz ergonómicos. Sin bajar a SQL
  crudo para casos comunes (key exists, contains subset,
  text extract).
- **Dynamic search filters**: chain condicional con `qb =
  qb.where(...)` funciona sin compromisos de perf — el SQL
  por fragmento sigue siendo constante en compile-time, solo
  el SHAPE del chain es dinámico.
- **Workarounds documentados**: cuando un patron requiere
  workaround conocido (W1-W7), está documentado con síntoma
  reproducible + plan de fix. Sin "magia" — el user sabe
  exactamente qué funciona y qué no.

## [v0.10.4] — 2026-05-26 — Deuda residual #1 cerrada: Map<Str, Any> en HTTP returns

**Primer cierre del bloque "deudas residuales del ORM"** (decidido
post-v0.10.3). 4 deudas planeadas para atacar en orden de scope
creciente: (1) Map<Str, Any> en HTTP, (2) BelongsTo eager loading,
(3) JSON operators en `.where`, (4) chain dinámico condicional.

### Added

- **`impl __MapKey for __FitzValue`** en el preludio HTTP del
  codegen (cuando `__FitzValue` está activo). Cierra la cadena
  de trait bounds que hacía fallar la serialización de
  `Map<Str, Any>` en HTTP returns:
  - Pre-fix: `Arc<Mutex<Vec<Arc<Mutex<Vec<(__FitzValue,
    __FitzValue)>>>>>>` → trait bound `Vec<(__FitzValue,
    __FitzValue)>: __ToFitzJson` no satisfecho (porque exige
    `K: __MapKey` y `__FitzValue` no lo implementaba).
  - Post-fix: el impl convierte `__FitzValue::Str(s)` a
    `s.clone()` (caso típico de keys de JSONB y GROUP BY),
    resto via Display (matchea la lógica de
    `__fitz_fv_to_json`). El chain de trait bounds queda
    satisfecho y el codegen emite el handler correcto.
- **`examples/guide/31b-orm-crud-http.fitz` ahora incluye
  endpoint `/stats/by-email`** que llama `User.group_by(fn(u)
  => u.email).count(db).await` y devuelve `Result<List<Map<Str,
  Any>>>` serializado a JSON automáticamente. Pre-v0.10.4 este
  endpoint era el caveat documentado del ejemplo; ahora forma
  parte del showcase.

### Changed

- `examples/guide/31b-orm-crud-http.fitz` — comentario del
  header refleja el cierre de la deuda (nota histórica vs
  caveat activo).
- `docs/guide.md` cap 31 — descripción del ejemplo HTTP CRUD
  incluye el nuevo endpoint GROUP BY y la referencia a v0.10.4.
- `docs/db-orm.md`:
  - Sección 28 (limitaciones) marca la deuda
    `Map<Str, Any>` en HTTP returns como **✅ CERRADO v0.10.4**
    con explicación del fix.
  - Sección 12 (eager loading) reescrita el caveat del
    `List<Map<Str, Any>>` en HTTP returns para reflejar que
    ahora funciona end-to-end.

### Behavior

- Handlers HTTP que retornan `Result<List<Map<Str, Any>>>`
  (típicamente desde `Type.group_by(...).count/sum/avg/min/max(db)`)
  ahora compilan y corren con paridad bit-a-bit `fitz run` ↔
  `fitz build`.
- Keys del Map: `__FitzValue::Str` (caso típico GROUP BY +
  JSONB) → string original; otros variantes → Display formatted.
- Empty list → array JSON vacío `[]` (sin cambios).
- Smoke `GUIDE_EXAMPLES_COMPILE` valida 292 ejemplos verdes
  (sin nuevos archivos; el ejemplo existente ahora compila
  más endpoints).

### Tests

- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos).
- 2552 unit tests verdes (sin cambios en el count — el fix es
  puramente codegen-side, no rompe ningún test existente).
- Lint `cargo fmt --all --check` + `cargo clippy --all-targets
  -D warnings` verdes.

## [v0.10.3] — 2026-05-26 — Guía exhaustiva DB y ORM (docs/db-orm.md)

**Hito de documentación**. Cierra la promesa hecha al diseñar
v0.10.2: el cap 31 de la guía es el RESUMEN del stack DB; la
guía exhaustiva vive aparte. Tab dedicado **"DB y ORM"** en el
nav de MkDocs entre "Guía" y "Roadmap" — `docs/db-orm.md`
nuevo con ~2600 LoC cubriendo cada operador, cada receta, cada
limitación honesta del MVP.

Decisión registrada en la memoria del proyecto (2026-05-25):
"el ORM merece su propia entrada de navegación porque (a) es un
dominio aparte del lenguaje base, (b) la gente que viene a
aprender el ORM específicamente no quiere scrollear por 30 caps
de la guía, (c) showcase del diferencial vs SQLAlchemy/Prisma/
Diesel". Cierre formal de esa decisión con este release.

### Added

- **`docs/db-orm.md`** nuevo (~2600 LoC, 30 secciones):
  - **1.** Panorama vecino (comparación side-by-side con stacks
    Python/Ruby/Java/Node/Rust/Go) + 6 diferenciales únicos.
  - **2.** Quickstart end-to-end (db.connect + @table + insert +
    where + all).
  - **3.** Driver `db`: query/exec/close/is_closed crudo +
    auto-coerción de tipos params.
  - **4.** `@table`, `@primary`, `@column(name=...)`, mapping de
    tipos Fitz → Postgres por default.
  - **5.** Read methods estáticos (`Type.all`/`first`/`count`/
    `where`).
  - **6.** QueryBuilder reference completo: chain (where /
    order_by / limit / offset / group_by / preload) + terminales
    (all / first / count / sum / avg / min / max / update /
    delete).
  - **7.** Operadores extendidos en `.where(...)` (comparators,
    lógicos, aritméticos + mod, between, is_in, like/ilike,
    starts_with/ends_with/contains, is_null/is_not_null, has/
    contains_all/contained_in) + **tabla resumen de soporte de
    variables externas por operador**.
  - **8.** Write methods + guard `.where(...)` obligatorio.
  - **9.** Aggregates scalar + GROUP BY (`Aggregated<Row>`
    separado de `QueryBuilder<Row>`).
  - **10.** Relations `@belongs_to`/`@has_one`/`@has_many` +
    kwargs `on_delete`/`on_update`/`fk`/`via`.
  - **11.** Navigation methods + chain (QueryBuilder<Target>
    cuando args vacía, terminal directo con db).
  - **12.** Eager loading `.preload(...)` con dispatch estático.
  - **13.** JSONB (`Map<Str, Any>`) con shape heterogéneo +
    JSON operators del lado SQL (workaround crudo).
  - **14.** Arrays Postgres (12 OIDs).
  - **15.** NULL en arrays (`List<scalar?>`).
  - **16.** `Map<Str, T>` concreto homogéneo vs `Map<Str, Any>`.
  - **17.** Array ops (`.has`/`.contains_all`/`.contained_in`)
    + caveat literales requeridos.
  - **18.** Date / Time / Timestamp / UUID como Str ISO 8601.
  - **19-26.** **8 recetas** runnable: paginación (offset/limit
    + cursor-based + paginado con total), búsqueda (prefijo +
    full-text con tsvector + arrays + JSONB), search filters
    combinatorios, **auth + ORM (queries scoped al user
    autenticado)** end-to-end, HTTP CRUD completo, cron jobs
    de limpieza, bulk operations (insert múltiple, update por
    set de IDs), schema idempotente al boot + migraciones
    manuales versionadas.
  - **27.** Performance: arquitectura del driver puro + SQL
    constante en codegen-time vs runtime construction (SQLAlchemy
    comparison) + placeholder para benchmarks futuros.
  - **28.** Limitaciones honestas y deuda explícita
    (migraciones automáticas, transactions, composite PKs, TLS
    strict, Date/UUID nativos, JSON operators en .where,
    BelongsTo eager, `Map<Str, Any>` en HTTP returns, chain
    dinámico, bulk insert eficiente, `db.copy_in`, `fitz db
    inspect`).
  - **29.** **CLI con DB: cómo cada subcomando interactúa** —
    `fitz run`/`build`/`check`/`openapi`/`test`/`dev`/`repl`/
    `fmt`/`lint` documentados con behavior específico sobre
    programas que usan el módulo `db` y el ORM. Subcomandos
    planeados `fitz db diff`/`migrate`/`inspect`/`seed`/`console`
    documentados como roadmap.
  - **30.** Ejemplos runnable (`31-orm.fitz` + `31b-orm-crud-
    http.fitz`) + boilerplates planeados (6 convertido + 7 nuevo).
- **`mkdocs.yml`** — entrada nueva `'DB y ORM': db-orm.md` en
  el nav entre "Guía" y "Roadmap" (decisión 2026-05-25 en
  memoria del proyecto formalizada).

### Changed

- **`docs/guide.md` cap 31** — sumada sección "Guía exhaustiva"
  con link al nuevo `docs/db-orm.md` antes del cierre. El cap
  31 sigue siendo el resumen del stack para lectores secuenciales
  de la guía; el doc dedicado es la referencia para lectores
  buscando ORM específico.
- **`README.md`** — footnote ◈ Postgres+ORM extendido con link
  al `docs/db-orm.md` ("guía exhaustiva ~2500 LoC con todos los
  operadores, recetas, CLI integration y limitaciones").
- **`docs/index.md`** — botón nuevo "DB y ORM →" al lado de
  "Guía completa →" en la sección "Por dónde arrancar". Tabla
  feature comparison suma row Postgres+ORM nativo. Texto
  introductorio actualizado a "34 capítulos" + mención del
  link a la guía exhaustiva.

### Fixed (correcciones de drift entre código y docs)

Auditoría exhaustiva durante la creación de `db-orm.md` reveló
desfasajes entre los docs/memoria y la implementación real.
Cerrados en este release:

- **Sintaxis de `on_delete`/`on_update`**: el cap 31 de la guía
  (v0.10.2) y los CHANGELOG entries de v0.10.0/v0.10.1
  describían estos como **decoradores separados**
  (`@on_delete=cascade`) con valores como **bare identifiers**.
  La realidad: son **kwargs del MISMO decorator** de relation
  (`@belongs_to`/`@has_one`/`@has_many`) con valores como
  **string literals**: `"cascade"`/`"set_null"`/`"restrict"`/
  `"no_action"`. Ejemplo correcto: `@belongs_to("User", on_delete="cascade") user_id: Int`.
  Cap 31 corregido + `db-orm.md` documenta la sintaxis correcta
  + sección 10 detalla los 4 valores válidos como string
  literals.
- **`.is_in([])` empty list**: docs decían "error claro en
  compile-time". La realidad: emite predicado `false` literal
  (no rompe el query, simplemente no matchea nada — `IN ()` no
  es SQL válido, el translator lo evita). Cap 31 corregido +
  `db-orm.md` documenta el comportamiento real.
- **Var externa support por operador**: documentación previa
  no clarificaba qué operadores aceptan vars externas vs solo
  literales. `db-orm.md` suma tabla resumen explícita: comparators
  + aritméticos + `.like(pat)` + `.ilike(pat)` + `.between(low,
  high)` aceptan vars; `.is_in([...])` arg debe ser list literal
  (items adentro pueden ser vars); `.has`/`.contains_all`/
  `.contained_in` requieren literales escalares; `.starts_with`/
  `.ends_with`/`.contains` requieren Str literal.
- **`Aggregated<Row>` chain capabilities**: la sección original
  de v0.10.2 solo mencionaba terminales (`count`/`sum`/`avg`/
  `min`/`max`). En realidad también soporta chain methods
  (`where`/`order_by`/`limit`/`offset`/`group_by`) que preservan
  el tipo. `db-orm.md` documenta ambos sets.
- **`db.is_closed()` faltante en docs**: el método existe en el
  evaluator (`Value::DbConn` arm `is_closed`) pero no estaba
  documentado. `db-orm.md` lo cubre en sección 3.

### Dependencies

Sin deps nuevas. Cambios 100% documentales.

### Hito

Con v0.10.3 el bloque DB+ORM tiene:

- **Cap 31 de la guía** — resumen para lectores secuenciales.
- **`docs/db-orm.md`** — referencia exhaustiva (~2600 LoC) con
  todos los operadores, 8 recetas runnable, CLI integration,
  limitaciones honestas, drift entre código y docs cerrado.
- **Entrada propia en MkDocs nav** — visibilidad equivalente
  a la guía principal del lenguaje.
- **2 ejemplos runnable** en `examples/guide/` (pedagógico +
  CRUD HTTP) sumados al smoke CI (292 ejemplos verdes).
- **CI con Postgres real** corriendo 44 tests en cada push.

Próximo norte: boilerplates ORM Dockerizados (convertir 6 +
crear 7 nuevo dedicado) + benchmarks Fitz ORM vs SQLAlchemy.

## [v0.10.2] — 2026-05-26 — Cap 31 guía: "Postgres + ORM nativo" + hito stack server completo

**Hito mayor del proyecto.** Cierra la documentación del bloque
"stack web first-class" del lado server con cap nuevo en
`docs/guide.md` ("Postgres + ORM nativo", cap 31) + dos ejemplos
runnable end-to-end. Con este release, las features ciudadanas
de primera clase del stack server quedan documentadas, ejemplificadas,
y vivas en CI:

| Feature           | Cap | Ejemplo                          | Status      |
| ----------------- | --- | -------------------------------- | ----------- |
| HTTP nativo       | 17  | `17-http.fitz`                   | ✅          |
| Middleware + CORS | 17  | `17b-middleware.fitz`            | ✅          |
| OpenAPI auto      | 18  | `18-docs.fitz`                   | ✅          |
| Async             | 19  | `19-async.fitz`                  | ✅          |
| `fitz build`      | 20  | `20-build.fitz`                  | ✅          |
| Interop Python    | 21  | `21-python-crud/`                | ✅          |
| Auth nativa       | 28  | `28-auth.fitz`                   | ✅          |
| WebSockets        | 29  | `29-ws.fitz`                     | ✅          |
| Jobs sin Celery   | 30  | `30-cron-background.fitz`        | ✅          |
| **Postgres + ORM** | **31** | **`31-orm.fitz` + `31b-orm-crud-http.fitz`** | **✅ NUEVO** |

Todo en el binario `fitz`, todo con paridad bit-a-bit
`fitz run` ↔ `fitz build`, todo validado en CI multi-plataforma
con Postgres real en cada push.

### Added

- **Cap 31 nuevo en `docs/guide.md`** — "Postgres + ORM nativo"
  (~550 LoC de markdown). Cubre las piezas (`db`, `@table`,
  `@primary`, `@column`, relations), read methods + QueryBuilder
  chain, write methods + guard `.where(...)` obligatorio,
  aggregates scalar + GROUP BY (`Aggregated<Row>` separado de
  `QueryBuilder<Row>`), relations + navigation methods, eager
  loading con `.preload(...)` y dispatch estático en compile-
  time, tipos avanzados (JSONB, arrays, `List<scalar?>`,
  `Map<Str, T>` concreto), operadores extendidos en `.where(...)`
  (`between`/`is_in`/`like`/`starts_with`/array ops), escape
  hatch `db.query`/`db.exec` para CTEs/window functions/JSON
  operators crudos. Sección "Por qué Fitz hace esto distinto" con
  5 diferenciales (DB nativa no lib, SQL constante codegen-time,
  paridad bit-a-bit, decorators del lenguaje no anotaciones,
  eager loading con dispatch estático). Sección "Qué no está
  en el MVP" con deuda explícita (migraciones, transactions,
  composite PKs, TLS strict, Date/Time/UUID nativos, JSON ops
  Postgres, BelongsTo en `.preload`). Cierre con callout de
  hito.
- **`examples/guide/31-orm.fitz`** (renombrado de `32-orm.fitz`)
  — el ejemplo pedagógico de 10.b.17, cap reference actualizada
  de "cap 32" a "cap 31".
- **`examples/guide/31b-orm-crud-http.fitz`** nuevo (~135 LoC)
  — showcase del stack completo: CRUD HTTP real (GET/POST/PUT/
  DELETE sobre users + posts), body deserialization con types
  custom dedicados (`UserInput`/`PostInput` separan el shape DB
  del shape HTTP), relations queries (`GET /users/{id}/posts`),
  eager loading (`GET /users-with-posts` con `.preload(...)`),
  aggregate scalar (`GET /user-count`), `env_or(...)` para leer
  `DATABASE_URL` con default, `@server(port)`. Requiere Postgres
  real para correr; compila con `fitz build` aunque no haya DB
  local. Documenta el setup pre-condición (`createdb` + `CREATE
  TABLE`) al inicio del archivo.
- **Smoke `GUIDE_EXAMPLES_COMPILE`** ahora valida 292 ejemplos
  (291 + `31b-orm-crud-http.fitz`). Garantiza que el ejemplo
  CRUD HTTP + ORM no regresione.

### Changed

- **Renumeración cap 31 → 32 / 32 → 33 / 33 → 34** en
  `docs/guide.md`:
  - Cap 31 anterior "Variables de entorno" → cap 32
  - Cap 32 anterior "Plantillas y boilerplates" → cap 33
  - Cap 33 anterior "Qué sigue" → cap 34
  - TOC actualizado, cross-refs internos al cap 31 viejo (env
    builtin) reapuntados a cap 32.
- **Rename de archivos de ejemplos** con `git mv` (preserva
  history):
  - `examples/guide/32-orm.fitz` → `examples/guide/31-orm.fitz`
  - `examples/guide/31-env.fitz` → `examples/guide/32-env.fitz`
- **`docs/index.md`** — link stale a `guide.md#31-plantillas-y-
  boilerplates` (que ya estaba roto pre-v0.10.2 — cap 31 era
  "Variables de entorno", no "Plantillas") reapuntado a
  `guide.md#33-plantillas-y-boilerplates` post-renumeración.

### Fixed

- **`up_map_update_compila` pre-existente** (regresión Windows
  UAC heredada de Mini-tanda Up): stem `up_map_update`
  gatillaba el heurístico installer-detection de Windows que
  exige elevación (`ERROR_ELEVATION_REQUIRED` 740). Renombrado
  a `up_map_upd`. Mismo workaround que aplicamos en 10.b.11
  (`orm_upd_list_map_codegen`). El test corría OK en Linux CI
  pero fallaba en local de Windows en parallel.

### Diferenciales únicos (reforzados con cap 31)

**Ningún otro lenguaje moderno** combina lo siguiente en el
binario base + cero deps externas:

- **HTTP nativo + auth + WebSockets + jobs + ORM + DB nativa**
  todo en el compilador. FastAPI/Spring/Express requieren
  ~5-10 librerías opcionales por cada uno.
- **Paridad bit-a-bit `fitz run` ↔ `fitz build`** para todas
  estas features (verificado en CI con Postgres real en cada
  push via job `db-postgres` con service container).
- **SQL constante en codegen-time**: cada `.where(closure)` se
  walka del AST DURANTE EL CODEGEN, el fragmento SQL queda
  hard-coded en el binario. Comparable a Diesel/sqlx, mejor
  que SQLAlchemy/ActiveRecord (runtime SQL construction).
- **Decorators del lenguaje**: `@table`/`@primary`/`@column`/
  `@belongs_to`/`@has_many`/`@has_one`/`@on_delete`/`@on_update`
  son parte del compilador (lexer + parser + checker + codegen),
  no anotaciones procesadas por libs en runtime (vs Spring
  `@Entity` + JPA reflection / SQLAlchemy declarative meta).
- **Eager loading con dispatch estático**: `.preload("posts")`
  con el relation name como Str literal en compile-time produce
  un `match` exhaustivo emitido por el codegen. Typos
  (`.preload("post")` sin la "s") detectados en compile-time,
  no runtime.
- **Binario standalone deployable**: `fitz build` produce un
  `.exe`/ELF/Mach-O ~5-10 MB con todo embebido — driver
  Postgres, JWT signing, Argon2 hashing, ORM, axum, tokio.
  Cero `requirements.txt`/`Cargo.toml`/`package.json` que
  mantener en el destino.

### Hito del proyecto

Con v0.10.2 cierra el bloque **"stack web first-class del lado
server"** entero, documentado y ejemplificado en la guía. La
promesa del proyecto — "escribir una API tipada con auth + DB
+ jobs + WebSockets que deploye como un binario standalone" —
está viva en un solo lenguaje, con cero deps externas para
features intrínsecas, en `fitz run` (rapid feedback) y en
`fitz build` (deploy a prod) idénticamente.

Próximo norte: Fase 11+ (frontend en `.fitz`, deployment
ciudadano primera clase, CLI builder) y refinamientos
opcionales sobre el stack ya vivo (migraciones automáticas,
transactions, TLS strict, JSON operators del lado SQL,
`Map<Str, Any>` en HTTP returns para GROUP BY).

## [v0.10.1] — 2026-05-26 — Fase 10.b: paridad bit-a-bit codegen del ORM

Hito de cierre de la deuda más grande heredada de v0.10.0. **Fase 10.b
ENTERA CERRADA**: el codegen del ORM declarativo ahora tiene paridad
bit-a-bit con el evaluator. Todo lo que `fitz run` soporta del ORM —
read methods, write methods, QueryBuilder chain, agregados, relations,
navigation, JSONB, arrays nullables, Map<Str,T> concretos, GROUP BY,
eager loading con `.preload`, operadores extendidos en `.where(...)` —
ahora también compila a binario nativo con `fitz build`.

23 commits, ~9580 LoC netas, 2552 unit + 81 cli_e2e + 291 compile_e2e
(smoke incluye `32-orm.fitz` pedagógico) + 3 openapi + 44 db_real_postgres
(`#[ignore]` opt-in, 16 son paridad codegen E2E nuevos vs evaluator).
Clippy `--all-targets -D warnings` limpio, fmt `--all --check` limpio.
**Paridad real Postgres corre en cada push a `main`** (job nuevo
`db-postgres` en `.github/workflows/ci.yml` con service container
`postgres:16`).

### Added

- **Fase 10.b.1 — Fixes preludio runtime + smoke `fitz build` con
  `db.connect` solo**. Tres bugs base cerrados: `Box::pin(...)` wrap
  del Future de `__fitz_db_connect`, imports condicionales `Arc,
  Mutex` según `has_http`/`uses_db`/`uses_python`, feature `time` de
  tokio cuando `uses_db = true`.
- **Fase 10.b.2 — Closure → SQL translator en codegen** (~400 LoC).
  Port del `translate_expr_to_sql` del evaluator al codegen. Helper
  `gen_closure_to_sql(closure, table_meta) -> (String, Vec<RustExpr>)`
  emite SQL parametrizado constante en codegen-time + Vec<Rust> de
  bindings que se evalúa en runtime. BinOp (Eq/NotEq/Lt/Gt/Lte/Gte/
  And/Or), UnaryOp (not), field access sobre el param de la closure.
  Cero overhead runtime para construir SQL.
- **Fase 10.b.3 — ORM read methods en `gen_call`**: `Type.all(db)`,
  `Type.first(db)`, `Type.count(db)`. Emit del SQL constante +
  deserializer per-type `impl __FromFitzDbRow for FooData` con
  conversión field-por-field desde `__FitzPgValue` (paralelo a
  `__FromFitzJson` para JSON HTTP).
- **Fase 10.b.4 — ORM write methods**: `Type.insert(db, record)`.
  RETURNING * round-trip al row Fitz, RETURNING id para auto-asignar
  serial. INSERT serializa fields según `TableMetadata` con casts
  apropiados (`::int8`, `::text`, `::jsonb`, etc.).
- **Fase 10.b.5 — QueryBuilder chain en codegen**: `.where(closure)`,
  `.order_by(closure, asc/desc)`, `.limit(n)`, `.offset(n)`,
  `.group_by(closure)`, terminales `.update(db, changes)` y
  `.delete(db)` con guard obligatorio `.where(...)` previo (safety
  check). Struct `__FitzQueryBuilder<Row>` con state mínimo (Vec de
  WHERE fragments + Vec de ORDER BY + Option<i64> de limit/offset
  + Vec<String> de GROUP BY), métodos accumulan al state, terminales
  componen SQL final + ejecutan via `__fitz_db_runtime`.
- **Fase 10.b.6 — Agregados scalares en codegen**: `.sum(closure, db)`,
  `.avg(closure, db)`, `.min(closure, db)`, `.max(closure, db)` sobre
  `QueryBuilder<Row>`. Helper `aggregate_f64` para path scalar.
- **Fase 10.b.7 — Navigation methods en codegen + refinement del
  checker**. Navigation `post.user_id(db).await?` → `Result<User>`
  (BelongsTo), `user.posts(db).await?` → `Result<List<Post>>`
  (HasMany), `user.profile(db).await?` → `Result<Profile>` (HasOne).
  Convención del field name: el método se nombra como el field FK
  (BelongsTo) o como el field virtual declarado en el `type`. Checker
  refinado para devolver `Type::Future(Result<Target>)` cuando args
  contiene `db`, y `Type::QueryBuilder(Target)` cuando args vacía
  (habilita chain post-navigation).
- **Fase 10.b.8.a — Arrays Postgres en codegen** (List<scalar>).
  `List<Int>` ↔ `int8[]`, `List<Str>` ↔ `text[]`, `List<Float>` ↔
  `float8[]`, `List<Bool>` ↔ `bool[]`. Marshaling directo sin pasar
  por `__FitzValue` — `Vec<T>` Rust en el row Fitz, INSERT/UPDATE
  detectan List<T> y emiten cast apropiado (`::int8[]`/etc.).
- **Fase 10.b.8.b — JSONB libre en codegen** (Map<Str, Any>). `Map<Str,
  Any>` ↔ `jsonb`. INSERT serializa Map → JSON via `serde_json` con
  `preserve_order` + cast `::jsonb`. SELECT parsea text JSON con
  `__FitzValue` (enum tagged ya existente del F13 SPIKE) preservando
  shape heterogéneo. Null Fitz → NULL real (no la string "null").
- **Fase 10.b.9.a — Validación exhaustiva del translator `.where(...)`
  en codegen**. Refinamiento de helpers que detectaban casos no
  cubiertos del AST y los rechazaban con error claro citando el shape
  esperado.
- **Fase 10.b.9.b — Operadores extendidos en `.where(...)`**: between,
  `%` (Mod), var externa al body de la closure (lookup en el scope
  del codegen para emitir como binding `$N`).
- **Fase 10.b.9.c — Operadores sobre arrays Postgres en `.where(...)`**:
  `.has(elem)` (cualquiera de los elementos del array column matchea),
  `.contains_all([...])` (`@>`), `.contained_in([...])` (`<@`).
- **Fase 10.b.10 — Cleanup + cobertura paridad real Postgres
  exhaustiva**. Helper de reuso `run_paridad_program(src, stem,
  assert)` que reduce duplicación en E2E. 14 paridad real E2E nuevos
  contra Postgres instalado: navigation, arrays + JSONB roundtrip,
  where combinatorio, between/mod/var externa, array ops, nav chain,
  GROUP BY aggregate, Map<Str,T> concreto, List<scalar?>, preload,
  CRUD lifecycle, order_by/limit/offset, basics all/first/count,
  aggregates scalar, col override en FK source.
- **Fase 10.b.11 — `.update` con List literal + Map literal**. Branches
  nuevos en `gen_qb_update_set_args` para que `.update(db, {"tags":
  ["a", "b"]})` y `.update(db, {"data": {"k": 1}})` emitan los casts
  apropiados (`::text[]`/`::jsonb`) y serialicen los valores.
- **Fase 10.b.12.a — NULL en arrays Postgres**. `List<Int?>` ↔ `int8[]
  NULL`. `__FitzPgValue::Array { elem_oid, values }` ahora codifica
  `NULL` sin quotes en el text format `{a,NULL,c}`. Parser/encoder
  simétricos. Branches específicos en `orm_field_coerce_block` y
  `orm_marshal_field_to_pg` para arrays nullable inner.
- **Fase 10.b.12.b — Map<Str, T> concretos en codegen**. `Map<Str,
  Int>` ↔ `jsonb` con shape homogéneo `HashMap<String, i64>` (vs
  `Map<Str, Any>` que usa `__FitzValue`). Marshaling directo sin
  enum dispatch — solo aplica cuando T es primitivo concreto
  (Int/Float/Str/Bool).
- **Fase 10.b.13 — Navigation chain + JSONB shape (by design)**.
  Decisión: las navigations siempre devuelven `QueryBuilder<Target>`
  cuando `args.is_empty()`, permitiendo `user.posts().order_by(...).
  all(db)`. Terminales obligatorios para ejecutar. JSONB conserva el
  shape libre del Map<Str, Any> — no se valida shape (by design: el
  user opera el dict de retorno con `.get(...)?`).
- **Fase 10.b.14 — GROUP BY + aggregate (Type::Aggregated)**. Nueva
  variante `Type::Aggregated(Box<Type>)` separada de
  `Type::QueryBuilder(Box<Type>)` para el path GROUP BY. El checker
  refina `.group_by(closure)` a `Aggregated<Row>` y los métodos
  agregados (`.count(db)` / `.sum(closure, db)` / etc.) sobre
  `Aggregated` devuelven `Result<List<Map<Str, Any>>>` (vs scalar
  sobre `QueryBuilder`). Helper `aggregate_groups` paralelo al
  scalar.
- **Fase 10.b.15 — Eager loading (.preload sobre HasMany)**. `User.
  preload("posts").all(db)` resuelve N+1 con 1 query batch
  (`SELECT * FROM posts WHERE user_id IN (1, 2, 3)`) + dispatch
  estático del relation name en compile-time vía match. Helper
  `emit_preload_dispatch` por type con `@has_many`. El relation name
  como Str literal queda hard-coded en el binario — typos detectados
  en compile-time, no runtime.
- **Fase 10.b.16 — Postgres en CI default**. Job nuevo `db-postgres`
  en `.github/workflows/ci.yml` que levanta `postgres:16` como
  service container, exporta `FITZ_TEST_PG_URL=postgres://postgres:
  postgres@localhost:5432/fitz_test`, y corre `cargo test --test
  db_real_postgres -- --ignored --test-threads=1`. Solo Linux
  (Docker service containers más estables en GHA Linux runners).
  Los 16 paridad codegen E2E + los 27 evaluator E2E ahora corren
  en cada push. **`#[ignore]` se mantiene** para que `cargo test`
  default sin env var siga rápido.
- **Fase 10.b.17 — Ejemplo guía `32-orm.fitz` pedagógico + smoke
  GUIDE**. Nuevo `examples/guide/32-orm.fitz` (~100 LoC) que muestra
  el shape canónico del ORM end-to-end: `@table` con `@primary` +
  `@column` + `@belongs_to` + `@has_many`, insert, where + first,
  chain order_by/limit/offset, operadores starts_with/is_in/between,
  aggregates scalares count/avg, GROUP BY con `Aggregated<Row>`,
  navigation belongs_to/has_many, eager loading con preload, y
  update/delete con guard. Sumado al smoke `GUIDE_EXAMPLES_COMPILE`
  (291 ejemplos compilan en cada push). `fitz build` produce binario
  aunque no haya Postgres real — el `connect` runtime falla con
  `Err` clara si la URL es inválida, así el ejemplo es ejecutable
  como guía sin Postgres local.

### Changed

- `Type::Aggregated(Box<Type>)` variante nueva, paralela a
  `Type::QueryBuilder(Box<Type>)`. Separación necesaria porque el
  path GROUP BY devuelve `List<Map<Str, Any>>` con shape heterogéneo
  vs el path scalar de `QueryBuilder.sum/avg/min/max` que devuelve
  `Float`.
- `evaluator::translate_expr_to_sql` ahora `translate_expr_to_sql_
  with_env(closure, table_meta, env: Option<&EnvRef>)` para soportar
  var externa al body de la closure (lookup en el scope del
  evaluator cuando el codegen lo necesita).
- `Value::Type` ya cacheaba `table_metadata: Option<Box<TableMetadata>>`
  desde v0.10.0; ahora el codegen además persiste el TypeEnv para
  resolver field types de relations cross-table.
- Sentinel test lock: `static ENV_VAR_LOCK: parking_lot::Mutex<()>`
  en `src/pbs.rs` para serializar tests que mutan `FITZ_TEST_PG_URL`/
  env vars globales y romper race con `cache_root_usa_env_override`.

### Fixed

- **PostData PartialEq derive faltante**: bug latente de 10.b.3 donde
  `inline_display_stmt` caía a `{:?}` para tipos compuestos y exigía
  `Debug` sobre `Arc<Mutex<NominalData>>`. Fix: branches específicos
  en `inline_display_stmt` para List/Map/Tuple/Any delegando a
  `show_expr` (paralelo al modo Display del intérprete).
- **E0507 cannot move out of self.posts en Display impl**: navigation
  fields virtuales (`@has_many posts: List<Post>`) generaban
  `Display::fmt` que movía el `Vec` adentro del receiver. Fix:
  `.clone()` explícito al pasar a `show_expr`.
- **`emit_qb_where_chain` perdía TypeExpr al usar `TypeExpr::Named
  ("Any")` placeholder**: array ops como `.has(elem)` necesitan
  conocer el inner type para emitir el cast apropiado. Fix: helper
  `type_to_type_expr_for_translator` convierte `Type` resuelto del
  checker a `TypeExpr` AST.
- **Map<Int, Int> previamente aceptado por accidente**: 10.b.12.b
  habilitó `Map<Str, T>` con T concreto, pero el codegen aceptaba
  cualquier K. Fix: K se restringe a Str (Postgres jsonb keys son
  strings). Map<Int, Int> ahora rechazado con error claro.
- **NULL en arrays E0308**: `__v` viene como `&T` en
  `for __v in __values.iter()`. Fix: `*__v` para primitivos Copy en
  some_wrap.
- **GROUP BY codegen emitía `.count` en lugar de `.aggregate_groups`**:
  10.b.14 lo separó por `Type::Aggregated` en lugar de mezclar
  paths en `gen_orm_qb_method`.
- **Windows UAC bloqueaba `orm_update_list_map_codegen` por "update"
  in stem**: ERROR_ELEVATION_REQUIRED code 740. Fix: renombre del
  helper a `orm_upd_list_map_codegen`.
- **Test paridad real `db_real_postgres` no corría en CI default**:
  ahora corre en cada push via job `db-postgres` (10.b.16). Pre-fix
  estos E2E solo corrían en local del autor.

### Dependencies

Sin deps nuevas — el driver Postgres sigue siendo **puro Fitz/Rust**.
`parking_lot` ya estaba para el intérprete (F17), reusado para el
mutex de env vars de tests.

### Diferenciales únicos (refrescados post-10.b)

Lo que sigue siendo único de Fitz tras Fase 10.b:

- **Único lenguaje moderno** con driver Postgres puro + ORM declarativo
  + paridad bit-a-bit `fitz run` ↔ `fitz build` + LSP completo
  **sin macros derive ni introspection runtime**. La paridad codegen
  cierra la última brecha que separaba el intérprete del binario.
- **Decorators del lenguaje** (`@table`/`@primary`/`@column`/
  `@belongs_to`/`@has_many`/`@has_one`/`@on_delete`/`@on_update`)
  son parte del compilador, no anotaciones procesadas en runtime.
- **SQL constante en codegen-time**: cada `.where(closure)` se walka
  del AST DURANTE EL CODEGEN, el fragmento SQL queda hard-coded
  en el binario. Zero overhead runtime para construir SQL —
  comparable a Diesel/sqlx, mejor que SQLAlchemy/ActiveRecord
  que construyen SQL via objetos en runtime.
- **Eager loading con dispatch estático**: `.preload("posts")` con
  el relation name como Str literal en compile-time → match
  exhaustivo emitido por el codegen. Typos detectados en compile-
  time, no runtime.
- **CI paridad real**: job dedicado `db-postgres` corre 27 evaluator
  E2E + 16 paridad codegen E2E contra `postgres:16` en cada push,
  cubriendo todo el ORM end-to-end sobre datos reales.

## [v0.10.0] — 2026-05-25 — Fase 10 entera: Postgres nativo + ORM declarativo

Hito mayor. Cierra **Fase 10 entera** (driver Postgres puro + pool +
ORM declarativo + relations + tipos avanzados) — la última fase del
stack web first-class. Ningún otro lenguaje moderno combina driver
Postgres puro + ORM sobre `type` + paridad bit-a-bit `fitz run` ↔
`fitz build` + LSP completo sin macros derive ni introspection runtime.

20 commits, ~7400 LoC nuevas, 2463 unit + 2574 LSP + 27 E2E reales
contra Postgres instalado. Clippy `--all-targets -D warnings` limpio,
fmt `--all --check` limpio.

### Added

- **Fase 10.1 — Driver Postgres puro en Fitz (sin libpq)**.
  - **10.1.a**: módulo nuevo `src/db.rs` (~2400 LoC) — protocolo wire
    v3.0 hand-rolled. `ConnectionConfig` con parser de URL postgres://,
    SCRAM-SHA-256 (RFC 7677) + PBKDF2-HMAC-SHA-256, Simple Query +
    Extended Query con `Parse`/`Bind`/`Describe`/`Execute`. 11 tipos
    OID core: BOOL, INT2/4/8, FLOAT4/8, TEXT/VARCHAR, BYTEA, DATE/TIME/
    TIMESTAMP/TIMESTAMPTZ, UUID, JSON/JSONB, VOID.
  - **10.1.b**: integración con evaluator — `Value::DbConn(Arc<DbConnHandle>)`,
    builtin module `db` con `db.connect(url).await`, métodos `query/
    exec/close` async sobre `DbConn`.
  - **10.1.c**: codegen del driver en `fitz build` (paridad bit-a-bit
    intérprete↔binario para programas que usan `db.*`).
- **Fase 10.2 — Pool de conexiones + reconnect + health check**.
  Pool con `Arc<DbPool>` + `OwnedSemaphorePermit`, RAII Drop pattern,
  health check con `Weak<DbPool>` para auto-cleanup, reconnect
  automático cuando una conn muere.
- **Fase 10.3 — ORM declarativo sobre `type`**.
  - **10.3.a**: decorators ORM (`@table("name")`, `@primary`, `@column(name=, sql_type=)`)
    + checker que persiste `TableMetadata` en el `TypeEnv`. Validación
    estática del shape (`@primary` sobre exactamente un field, etc.).
  - **10.3.b1**: `Type.all(db) -> Result<List<Type>>` end-to-end +
    cache de metadata en `Value::Type` para evitar re-lookup en cada
    call.
  - **10.3.b2**: `Type.where(closure) -> QueryBuilder<Row>` con
    translator AST → SQL parametrizado. Traduce BinOp comparators
    (==, !=, <, <=, >, >=), BinOp lógicos (and, or), UnaryOp (not),
    field access sobre el param de la closure. Args van como `$N`
    parametrizados, sin SQL injection.
  - **10.3.b3**: chain methods `.order_by(closure, ascending: Bool)`,
    `.limit(n)`, `.offset(n)`, `.first(db)`, `.count(db)`. Builder
    pattern con `QueryBuilderState` cloneable inmutable.
  - **10.3.c**: terminales `.insert(db, row)`, `.update(db, changes)`,
    `.delete(db)`. UPDATE refuses sin `.where(...)` previo (safety
    check). RETURNING * round-trip al row Fitz.
- **Fase 10.4 — Relations cross-table**.
  - **10.4.a**: decorators `@belongs_to("Author")`, `@has_one("Profile")`,
    `@has_many("Comment")` con `@on_delete=cascade/setnull/restrict/noaction`
    y `@on_update=...`. Persistidos en `TableMetadata.relations`. Fields
    virtuales para `has_*` (no aparecen en SQL columns).
  - **10.4.b**: navigation methods. `post.author(db) -> Result<User>`
    (BelongsTo), `user.posts(db) -> Result<List<Post>>` (HasMany),
    `user.profile(db) -> Result<Profile>` (HasOne). Lazy (1 query
    por navegación, sin N+1 eager hasta 10.6).
- **Fase 10.5 — Tipos avanzados**.
  - **10.5.a**: **JSONB**. Field `data: Map<Str, Any>` → columna
    `jsonb`. INSERT serializa Map → JSON con cast `::jsonb`. SELECT
    parsea text JSON de vuelta a Map Fitz. Nested Maps preservados.
    Null Fitz → NULL real (no la string "null").
  - **10.5.b**: **Arrays nativos** (List<T> ↔ Postgres T[]). 12 array
    OIDs (bool/int2/4/8/text/varchar/float4/8/date/timestamp/uuid).
    `PgValue::Array { elem_oid, values }` con parser/encoder del text
    format `{a,b,c}` que maneja escapes `\\`/`\"`, NULL sin quotes,
    arrays vacíos. INSERT/UPDATE detectan `List<T>` y emiten cast
    apropiado (`::int8[]`/`::text[]`/etc.). SELECT round-trip a
    `Value::List`.
  - **10.5.c**: **Date / Time / Timestamp / Timestamptz / UUID**.
    Round-trip como `Str` con formato ISO 8601 / UUID canonical. Sin
    tipos Fitz dedicados en MVP — `let d: Str = ...`. Cross-feature
    test con uuid[] valida `Array<UUID>` end-to-end.
  - **10.5.f1**: **Agregados sobre QueryBuilder** — `.sum(closure, db)`,
    `.avg(closure, db)`, `.min(closure, db)`, `.max(closure, db)`.
    Cast `::float8` automático en avg para evitar Numeric OID.
  - **10.5.f2**: **GROUP BY**. `.group_by(closure).all(db)` devuelve
    `List<Map>` con `{group_field: value, count: N, sum_x: N, ...}`.
    Auto-detección scalar vs grouped path según `state.group_by_clauses`.
  - **10.5.g**: **Operadores y filtros extendidos en `.where(...)`**.
    Method calls sobre `<param>.<col>`: `is_null()`, `is_not_null()`,
    `is_in([a, b, c])`, `like(p)`, `ilike(p)`, `starts_with(s)`,
    `ends_with(s)`, `contains(s)`. `escape_like` para escapar `%`/`_`/`\`
    en patterns. is_in con lista vacía → error claro.
- **`Type::QueryBuilder(Box<Type>)` paramétrico en el checker**. Para
  que el LSP entienda la cadena `User.where(...).order_by(...).all(db)`
  con tipos refinados (chain methods preservan QB, terminales devuelven
  `Result<List<Row>>`/`Result<Row>`/`Result<Int>` apropiado).
- **LSP refresh (post-ORM)**.
  - Grammar TextMate: `DbConn` y `DbRow` highlighted como built-in
    types. Decorators (`@table`/`@primary`/`@belongs_to`/etc.) ya cubiertos
    por pattern genérico.
  - Scope-level completions: módulo `db` como MODULE, `DbConn`/`DbRow`
    como CLASS.
  - After-dot completions:
    - `db.` → `connect`
    - `DbConn.` → `query/exec/close`
    - `TableName.` con `@table` → `all/where/insert` estáticos
    - `QueryBuilder<Row>.` → 14 chain methods + terminales con detail
      tipado al row concreto
  - Chain detection con parens balanceadas — captura `User.where(fn(u)
    => u.id > 0).` como recv válido (antes se rompía en el `)`).
  - Resolver: `DbConn` y `DbRow` aceptados como tipos primitivos en
    `resolve_named` (antes producían "tipo desconocido" en anotaciones).

### Changed

- `Value::Type` ahora cachea `table_metadata: Option<Box<TableMetadata>>`
  para que el dispatch ORM no re-lookee el env en cada call.
- `Value::QueryBuilder(Arc<dyn Any + Send + Sync>)` opaco — evita
  ciclo de dependencia entre `evaluator` y `value`.
- `Value::Instance` ahora se forma con `{ type_name, fields }` (struct
  variant) — tests E2E reformateados.
- Tests E2E del driver (15 archivos en `tests/db_real_postgres.rs`)
  con setup canonical `DROP TABLE IF EXISTS` + `CREATE TABLE` para
  re-runs limpios. Opt-in via `FITZ_TEST_PG_URL` env var.

### Fixed

- Driver: 2 bug fixes críticos durante 10.3.b1 — (a) Extended Query
  protocol fallaba silente sin `Describe(P, "")` entre `Bind` y
  `Execute` (server no enviaba `RowDescription`), (b) OID 2278 (void)
  no soportado rompía `pg_sleep` en el test del pool — mapeado a
  `PgValue::Null`.
- Driver: `Numeric` (OID 1700) sin soporte rompía AVG — fix con cast
  `::float8` automático en el SQL emit de aggregates.
- Codegen: `Value::Type` boxed (`Option<Box<TableMetadata>>`) para
  evitar `result_large_err` clippy de 117 errores tras agregar la
  metadata al enum.
- Lock scope: 9 instancias de `await_holding_lock` en los tests E2E
  refactoreadas con scopes `{ ... }` que dropean el guard antes de
  los `.await` del driver.

### Dependencies

Sin deps nuevas — driver Postgres es **puro Fitz/Rust** (sin `tokio-
postgres`, `sqlx`, `diesel`, ni libpq).

### Diferenciales únicos

- **Único lenguaje moderno** con driver Postgres puro + ORM declarativo
  + paridad bit-a-bit `fitz run` ↔ `fitz build` + LSP completo
  (autocomplete del ORM end-to-end) **sin macros derive ni
  introspection runtime**.
- Decorators del lenguaje (no lib externa): `@table`/`@primary`/
  `@belongs_to`/`@has_many` son parte del compilador.
- Validación estática: el checker exige `@primary` único, `role: Str`
  no nullable para `@admin`, etc.
- Zero deps externas: SCRAM-SHA-256 + PBKDF2 + protocolo wire v3.0
  todo hand-rolled.
- Type system aware: `QueryBuilder<Row>` paramétrico, chain refina al
  tipo concreto, LSP sugiere las 14 métodos del builder con detail
  específico al row.



En curso: ver `docs/roadmap.md` para el plan vigente. **Package
manager (9.y.1 + 9.y.2 + 9.y.3 entera + 9.y.4) CERRADOS**, **9.z
(DX) ENTERA CERRADA**, **refresh masivo de docs ENTERO CERRADO**,
y **bloque entero de mini-tandas post-Fase 8 cerrado**: ~25
mini-tandas en 4 días (2026-05-17 → 2026-05-20) llevaron el
lenguaje + LSP + HTTP a estado pulido. Highlights:

- **R-series, S, Mb-series, Math+Mb9**: ~40+ métodos chicos sobre
  primitivos y colecciones.
- **Bits/Núm/Lit/F8/F9/Fmt-build**: operadores de bit, separadores
  numéricos, hex/bin/oct, identifiers Unicode, escapes extendidos,
  format specs en codegen.
- **Cd/F11-F19**: codegen polish completo — higher-order, state
  HTTP shared, módulos transitivos, error recovery del parser,
  IR tipado per nodo, codegen interop Python.
- **HTTP polish bundle** (HC, Hpx.1/2, Mw.next, RP/MP/P1, UC/HA):
  status codes custom, Content-Type 415, return type inference,
  post-process middleware, urlencoded body completo, msg alignment.
- **DZ/CT/OAPI**: paridad chica run↔build (división por cero,
  comparar tipos distintos) + status codes con consts.
- **MP2/MP-Build + File.content Bytes**: multipart con files
  binarios end-to-end (paridad bit-a-bit).
- **Bytes**: sexto primitivo del lenguaje (`b"..."` con escapes
  `\xHH`, métodos `.len()`/`.is_empty()`/`.to_str()`, builtin
  `bytes(s)`, base64 en JSON).
- **Mw-Wrap**: wrap-style middleware con `next` callable
  (intérprete; codegen es la única deuda visible restante).
- **F13 entero**: heterogéneos en `fitz build` con
  `__FitzValue` tagged runtime — primitivos, Bytes, Nominales,
  List/Map heterogéneo, anidados con mix interno, HTTP body
  `List<Any>`/`Map<Str, Any>`, method dispatch dinámico
  (`.as_int()`/`.as_str()`/`.type_name()`). 95%+ del lenguaje
  compila a binario nativo con paridad bit-a-bit.
- **OAPI-Expr**: status codes con const-eval recursivo (BinOp +
  UnaryOp::Neg sobre consts encadenadas).
- **LSPx/LSPy + cross-module go-to-def + scope-aware completion**.

Total al cierre del bloque: **2045 unit sin feature, 2135 con
--features lsp, 250+ compile_e2e**, 77 ejemplos guía. Clippy
`-D warnings` limpio. Detalle exhaustivo en
[`docs/deudas_lenguaje.md`](docs/deudas_lenguaje.md) y
[`docs/design-fitzvalue.md`](docs/design-fitzvalue.md) (F13).

Próximo norte: **boilerplates Dockerizados** (memoria
`project_boilerplates`) — 4 boilerplates showcase del stack
cerrado en 9.w (api-simple, api-postgres-python con SQLAlchemy
via interop, api-middleware-cors, cli-tool). Luego repo público
+ sitio docs MkDocs Material. ORM nativo + migraciones
(9.w.4 / Fase 10) cuando aparezca proyecto real que lo necesite.

## [v0.9.57] — 2026-05-24 — Cierre 8-pyi-stubs: auto-pickup loader + field access tipado + race fix compile_e2e

### Added

Cierre de la última deuda activa del proyecto: **8-pyi-stubs**.
Auto-pickup loader de archivos `.pyi` adyacentes al `.fitz` raíz
+ field access tipado sobre los stubs cargados. Después de
v0.9.57, queda **cero deudas activas cerrables** — el inventario
post-boilerplates está vacío.

- **`src/pyi_loader.rs`** (módulo nuevo, ~400 LoC):
  - `load_stubs(program, base_dir, env)` — **pase 1 (8-pyi.B)**:
    walkea el programa buscando `Stmt::FromImport { path:
    ["python"], names }`, intenta cargar `<base_dir>/<name>.pyi`
    por cada nombre, parsea con `pyi_stub::parse_stub`, y
    registra solo las `class` declarations en el `TypeEnv`.
    Skipea classes con nombre en pre-scan del programa
    (`type X { ... }`) y built-ins HTTP (`Request`, `Response`,
    `File`) — política "el .fitz gana sobre el .pyi". Fns/vars
    del stub se posponen a pase 2 (8-pyi.C).
  - `load_callables(stubs, env)` — **pase 2 (8-pyi.C)**: procesa
    fns/vars top-level de cada stub cargado y crea un nominal
    sintético `__pyi_module_<binding>` con un field por
    callable/var. Fns se materializan como `Type::Function {
    params, ret: Result<ret, Str> }` (auto-wrap a Result
    paralelo al runtime 8.3 donde toda call Python se envuelve).
    Vars se materializan con su tipo directo (sin wrap). Registra
    mapping `binding → synth_id` en `env.pyi_modules`.

- **`src/pyi_stub.rs`**: nuevas APIs públicas
  `register_stub_items_into_env(items, env) -> Vec<ResolvedStubItem>`
  y `nominal_fields(env, id)`. Política "el .fitz gana":
  `register_stub_items_into_env` solo setea fields si el nominal
  todavía no tiene fields (no sobreescribe declaraciones del
  programa Fitz).

- **`src/types.rs`**:
  - `TypeEnv.pyi_modules: HashMap<String, TypeId>` + métodos
    `set_pyi_module(name, id)` / `pyi_module(name) -> Option<TypeId>`
    para mapear binding name → nominal sintético.
  - Nuevas APIs públicas `resolve_program_with_env(program,
    initial_env, errors_init)` y `check_with_env(program, env,
    errors)` que permiten partir de un env pre-llenado (típicamente
    por el loader). `resolve_program(program)` y `check_program(program)`
    quedan como wrappers para backward compat de los 11+ call sites
    sin contexto de archivo.
  - `Stmt::FromImport` from_python: si hay stub cargado (lookup
    en `pyi_modules`), bindea el nombre con `Type::Nominal(id)`
    sintético; sino fallback a `Type::PyAny` opaco.
  - `infer_method_call` para `Type::Nominal(id)`: **8-pyi.C
    field-as-callable** — antes del lookup de métodos custom
    (R.3), busca en `info.fields` un field con `type_:
    Type::Function`. Si matchea, valida arity + tipos de args
    y devuelve el ret. Mensajes de error recortan el prefijo
    `__pyi_module_` para mostrar el binding original (e.g.
    `api.fetch_user espera 1 argumento(s), recibió 3`).

- **`src/main.rs`**:
  - `base_dir_for_stub_lookup(path) -> PathBuf` — calcula el
    base dir del lookup (parent del path, fallback a cwd).
  - `check_program_with_pyi_stubs(program, path)` — wrapper
    que orquesta los dos pases del loader alrededor de
    `resolve_program_with_env` + `check_with_env`. Llamado por
    todos los call sites con path (`run`, `build`, `check`,
    `openapi`, `bundle_python`, `test`).

### Fixed

**Race condition Windows preexistente en `compile_e2e`**: pre-fix
todos los tests del harness escribían `prog.fitz` → compartían
`target/fitz-build/prog/` (cache global per-stem). Bajo
`SERIAL`, los tests corrían secuenciales, pero Windows mantenía
file handles del `.exe` un instante después de `Child.wait()`;
el siguiente test sobreescribía el mismo path y `fitz build`
fallaba con `OS error 32 — being used by another process`. Flake
real intermitente, no puro.

Fix: helper `sanitize_stem(test_name)` (lowercase + chars
no-`[a-z0-9_-]` → `_`) usado por `build_and_run`,
`build_expect_fail` y `build_and_run_with_env`. Cada test escribe
`<sanitized>.fitz` → cada uno va a `target/fitz-build/<sanitized>/`.
Cero choque de handles entre runs. Tests inline que no usan
helpers (~31 sitios) siguen vulnerables como deuda menor.

### Notes

- **8-pyi.D (codegen paridad)**: cierra "gratis". El codegen
  consume el `TypeInfo` del checker, y el checker ya usa los
  stubs vía B/C. Programas sin `from python import` siguen
  idénticos (validado smoke con `buildtest.fitz`).
- **Cap 21.8b** de `docs/guide.md` reescrito: documenta los dos
  modos (manual `fitz py-stubs` + auto-pickup), tabla de cuándo
  usar cada uno, sub-set cubierto incluyendo callables y vars
  del stub (no solo classes como pre-v0.9.57).
- **Ejemplo runnable nuevo**: `examples/guide/21c-pyi-autopickup/`
  con `users.pyi` adyacente + `app.fitz` que demuestra el
  pipeline tipado end-to-end via auto-pickup (valida con
  `fitz check`).
- **Decisiones técnicas**: lookup local-only (adyacente al
  `.fitz`, NO PYTHONPATH/site-packages) — máxima reproducibilidad
  + cero magia ambiente, diferencial vs typecheckers Python que
  dependen del venv. Silent fallback en parse error (warning a
  stderr, binding cae a PyAny). Política "el .fitz gana sobre
  el .pyi" via skip set en pase 1. Nominal sintético prefijado
  `__pyi_module_<binding>` para evitar colisiones con tipos del
  programa; prefix se recorta en mensajes de error.
- **14 unit tests nuevos en `pyi_loader::tests`** (4 del pase 1
  ya existían en v0.9.57.B + 4 nuevos del pase 2 8-pyi.C +
  regresiones). Suite total: **2304 unit (default) / 2395 lsp**.
- **Smoke E2E manual VERDE**: programa con `from python import
  api` + `api.pyi` adyacente valida tipado completo de classes,
  fns con auto-wrap a Result, vars top-level, arity check, y
  type check de args (todos producen errores precisos del
  checker con mensajes user-friendly).
- **Próximo norte**: **Fase 10 — Stack DB nativo + ORM
  declarativo**. Driver Postgres en Fitz puro + ORM sobre `type`
  + migraciones autogeneradas. Sesión de diseño primero (sin
  código), después implementación incremental. **El inventario
  de deudas activas queda vacío después de v0.9.57** — el
  proyecto entra a fase "todo lo prometido implementado" antes
  de la próxima fase grande.

---

## [v0.9.56] — 2026-05-24 — Re-investigación R.bug-pyo3-abi3-portable-link Linux: reclasificado como constraint arquitectural permanente

### Changed

Retomado el bug R.bug-pyo3-abi3-portable-link Linux/macOS con el
plan documentado el 2026-05-23 ("combinación correcta del fix sin
validar"). El experimento empírico **invalidó la hipótesis del
fix** y reveló que el bug **no es cerrable en Linux**. Se
reclasifica de "deuda activa cerrable" a **constraint
arquitectural permanente**. **Cero cambios de código del lenguaje**;
solo documentación + comentarios en Dockerfiles.

- **Experimento Docker en `d:\tmp\fitz-pyo3-test\`** (descartado,
  no va al repo):
  - Builder: `FROM python:3.13-slim`
  - Runtime: `FROM python:3.10-slim` (cross-version intencional)
  - Env vars: `PYO3_NO_PYTHON=1` + `PYO3_CONFIG_FILE` con
    `lib_name=python3` + `abi3=true` + `version=3.10`
  - RUSTFLAGS: `-L /usr/local/lib`
  - Cargo build OK hasta el link final; `rust-lld` falló con
    ~10+ símbolos undefined (`PyDict_Next`, `PyObject_Str`,
    `PyLong_AsLong`, `PyBool_Type`, `PyFloat_Type`, etc.).

- **Verificación con `nm -D /usr/local/lib/libpython3.so`** en
  `python:3.10-slim` y `python:3.13-slim`:
  - El archivo (13992 bytes en ambas imágenes) exporta **solo 4
    símbolos glibc** (`_ITM_*`, `__cxa_finalize`, `__gmon_start__`).
  - **NO exporta ningún símbolo del API Python**.
  - La asunción del 2026-05-23 de que ese archivo era el "abi3
    shim" era falsa — es un dummy/placeholder.

- **Conclusión**: en Linux NO existe equivalente al `python3.dll`
  stable-ABI shim de Windows. Los símbolos abi3 viven solo en
  `libpython3.X.so.1.0` (versioned). El bug requiere uno de:
  - (a) Cambio upstream en PyO3 (modo "skip-link + dlopen
    runtime", pyo3#5043 abierto).
  - (b) Cambio arquitectural en Fitz (CPython como subprocess).
  - (c) Distribuir Fitz como wheel Python (modelo invertido).
  
  Ninguna razonable en corto/medio plazo.

- **Reclasificación**: el bug pasa de "deuda activa" a
  **constraint arquitectural documentado**. El workaround
  "match builder=runtime Python version" es la **solución
  permanente** en Linux, no temporal.

### Files updated

- **`docs/deudas_lenguaje.md`** sección
  R.bug-pyo3-abi3-portable-link: nueva sub-sección
  "Re-investigación 2026-05-24 — hallazgo definitivo" con tabla
  empírica de `nm -D` y razonamiento de las 3 opciones
  arquitecturales descartadas.
- **`docs/roadmap.md`**: "Estado actual del proyecto" pasa a
  v0.9.56; queda solo 1 deuda activa restante (`8-pyi-stubs`);
  sección de Fase 8.b actualizada con cierre formal del cierre
  parcial Windows + reclasificación Linux/macOS.
- **`docs/guide.md`** cap 21.11: nota "Constraint conocido"
  reescrita como "Constraint arquitectural permanente" con
  referencia al experimento empírico y a `deudas_lenguaje.md`.
  Cap 33 "Qué sigue" pierde la deuda de la lista de "Deudas
  reales restantes".
- **`docs/deudas-post-5b.md`**: la fila de la tabla pasa a
  `~~CERRADO~~` con etiqueta "RECLASIFICADO v0.9.56".
- **`boilerplates/api-postgres-python/Dockerfile`** +
  **`boilerplates/api-fullstack-postgres/Dockerfile`**:
  comentarios actualizados de "deuda residual" a "constraint
  arquitectural permanente" con referencia a deudas_lenguaje.md.

### Notes

- **Cero cambios de código del lenguaje**. Suite intacta: 2290
  default / 2381 python / 2395 lsp. Clippy + fmt limpios.
- El experimento Docker en `d:\tmp\fitz-pyo3-test\` queda
  descartado (no va al repo). El hallazgo empírico está
  documentado en `docs/deudas_lenguaje.md`.
- **Deudas reales restantes después de v0.9.56**: solo
  `8-pyi-stubs` (1-2 días, post-Fase 9). El proyecto queda en
  estado "una sola deuda activa cerrable".
- **Próximo norte**: **Fase 10 — Stack DB nativo + ORM
  declarativo**. Sesión de diseño primero (sin código),
  después implementación incremental.

---

## [v0.9.55] — 2026-05-24 — Hito de consolidación: refresh masivo de docs macro

### Changed

Tras 14 releases consecutivos cerrando deudas (v0.9.43 → v0.9.54),
release de consolidación que refresca las docs macro al estado
actual. **Cero cambios funcionales**.

- **`README.md`** raíz:
  - Sección "Interop Python via PyO3" (footnote §): actualizada
    para reflejar que **distroless está habilitado desde v0.9.46**
    (launcher con `tar`+`flate2` inline) y **smoke real Docker
    validado end-to-end con Postgres** en v0.9.50/52 (imagen
    ~136 MB). Pre-fix el README citaba distroless como "deuda
    menor del launcher" — ya no es cierto.
  - Tabla de boilerplates: los 2 con Python+Postgres
    (`api-postgres-python`, `api-fullstack-postgres`) ahora
    documentan la variante `Dockerfile.distroless` validada,
    incluyendo CORS preflight desde otro origin para el fullstack.

- **`docs/roadmap.md`**:
  - Nueva sección "Estado actual del proyecto (v0.9.55)" al
    inicio. Resume las fases 1-9 entera CERRADAS, el cierre del
    bundle B/I (Python interop codegen), las métricas de tests
    actuales (2290 default / 2381 python / 2395 lsp), las 2
    deudas reales restantes (R.bug-pyo3-abi3 L/M, 8-pyi-stubs),
    y el próximo norte grande (Fase 10 — Stack DB nativo + ORM).

- **`docs/guide.md` cap 33 "Qué sigue"**:
  - Sección "Lo que viene" actualizada para reflejar que Fase 9
    está entera CERRADA (era listada "en curso"). Suma mención
    de bundling Python `--bundle-python`/`--bundle-pip*` (Fase
    8.b/8.c) con smoke distroless validado, env builtin (cap
    31), y los 4 caps de stack web first-class (28-30).
  - "Deuda residual comprometida" actualizada: las 3 que
    listaba (coerción list/dict, heterogéneos compilados,
    deuda menor F7) **ya cerraron** (v0.9.44/49/54).
    Reemplazadas por las 2 reales restantes.
  - Suma Fase 11 (Frontend en `.fitz`) y Fase 12 (Deployment
    ciudadano primera clase) como nortes especulativos
    siguientes al post-Fase 10.

### Notes

- **Cero cambios de código del lenguaje**. Suite intacta: 2290
  default / 2381 python / 2395 lsp. Clippy + fmt limpios.
- **Hito**: este release marca el cierre del bloque de 15
  releases consecutivos (v0.9.43 → v0.9.55) que llevaron el
  proyecto desde "Fase 9 + bundling con caveats" hasta
  "production-ready en patrones canónicos + repo profesional
  con CI strict + 6 boilerplates validados end-to-end".
- **Próximo norte**: **Fase 10 — Stack DB nativo + ORM
  declarativo**. Sesión de diseño primero (sin código),
  después implementación incremental.

## [v0.9.54] — 2026-05-24 — Cierre dict→Map<K,V> primitivo: coerción `PyAny → Map<Str, V>`

### Added

- **Coerción `PyAny → Map<Str, V>` para V primitivo**
  (Str/Int/Float/Bool). Pre-fix: `let m: Map<Str, Str> = json.
  loads(raw)?` adentro de fn `-> Result<Map<Str, Str>>` fallaba
  en rustc con `expected Arc<Mutex<Vec<(String, String)>>>,
  found __FitzPyObject` — el `coerce()` no tenía caso para
  `(PyAny, Map<K, V>)`. Post-fix: 4 helpers nuevos en el preludio
  Python + caso wireado en `coerce()`. Cubre el caso típico de
  `json.loads` de objects con shapes simples.

  Implementación:
  - 4 helpers `pub(crate) fn __fitz_py_to_map_string_<v>` con
    v ∈ {string, i64, f64, bool} emitidos en el bloque
    `emit_python_prelude` (paralelo a los `__fitz_py_to_list_<v>`
    existentes). Cada helper: itera el PyDict, valida que las
    keys son `PyString` + cada value es del tipo esperado, y
    devuelve `Arc<Mutex<Vec<(String, V)>>>`. Preserva el orden
    de inserción del dict Python (CPython 3.7+ garantía nativa).
  - Caso nuevo en `coerce()`: `(Type::PyAny, Type::Map(k, v))`
    despacha por `(k, v)`. K=Str + V primitivo → helper
    dedicado. K no-Str u otros V (Nominal/List/Map/Any) →
    gradual (`code` tal cual; el caller se queja en build si
    necesita coerción concreta).

  **Cobertura de combinaciones**:
  - ✅ Map<Str, Str>, Map<Str, Int>, Map<Str, Float>, Map<Str, Bool>.
  - ❌ Map con K no-Str (raro en JSON), Map<Str, Nominal>,
    Map<Str, List<...>>, Map<Str, Map<...>>, Map<Str, Any> —
    quedan como deuda menor (caso 90%+ cubierto; los compuestos
    son raros y el usuario puede destrabar iterando manualmente
    el PyDict si necesita).

### Notes

- **Tests nuevos**: 5 unit en `codegen::tests::map_coerce_*`
  (4 helpers verificados + 1 test que confirma que Map<Str,
  List<...>> queda gradual sin emitir helper inexistente).
- **Smoke real validado**: `fn parse(raw) -> Result<Map<Str,
  Str>>` con `json.loads(raw)?` compila y produce
  `Ok({"a": "x", "b": "y"})` con `json.dumps` + round-trip
  (validado a mano con `fitz build` + ejecutar binario).
- Suite total: **2290 default** (era 2285 + 5), **2381 python**
  (era 2376 + 5), **2395 lsp** (era 2390 + 5). Clippy
  `--all-targets -D warnings` + `cargo fmt --check` limpios en
  los 3 modos.
- **Sin cambios a la extensión VSCode** — fix puramente del
  codegen.

### Bundle B/I (Python interop codegen) ENTERO CERRADO

Con v0.9.54, las 3 deudas originales del bundle B/I (Python
interop codegen) cierran:

| Deuda | Estado |
|-------|--------|
| ~~8.7-ok-propagation~~ | ✓ CERRADO v0.9.53 |
| ~~8.7-await-binding-split~~ | ✓ CERRADO mini-tandas previas (verificado v0.9.49) |
| ~~dict→Map<K,V> no primitivos~~ | ✓ CERRADO v0.9.54 (variantes primitivas) |

**Inventario depurado** post-v0.9.54: **2 deudas reales
restantes**:

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| R.bug-pyo3-abi3 Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |

## [v0.9.53] — 2026-05-24 — Cierre 8.7-ok-propagation + fix fmt regression v0.9.51

### Fixed

- **8.7-ok-propagation — codegen propaga expected type adentro
  de `Ok(...)`/`Err(...)` en `return`** ✓. Deuda residual de
  Fase 8.7 que ya era blocker concreto del boilerplate 6
  (v0.9.52 aplicó workaround temporal con binding intermedio
  anotado). Pre-fix: `return Ok(json.dumps(raw)?)` adentro de
  fn `-> Result<Str>` fallaba en rustc con `expected String,
  found __FitzPyObject` porque `gen_ok` devolvía
  `Result<PyAny>` sin coerción al expected `Str` y el `coerce`
  general no maneja `Result<A> → Result<B>`. Post-fix:
  `gen_return` detecta `Expr::Ok(inner)` / `Expr::Err(inner)`
  cuando `ret_expected` es `Result<T, E>` y coerce `inner`
  directo al T (Ok) o E (Err) ANTES de envolver. El gate
  `!self.response_mode && !self.in_middleware_fn` lo aísla de
  los paths HTTP que ya manejan Ok/Err específicamente. (`src/
  codegen.rs::gen_return`)

  Casos cubiertos:
  - `return Ok(json.dumps(...)?)` con `-> Result<Str>` →
    coerce PyAny → Str via `__fitz_py_extract_string`.
  - `return Ok(math.floor(...)?)` con `-> Result<Int>` →
    coerce PyAny → Int via `__fitz_py_extract_i64`.
  - `return Ok(T { ... })` con `-> Result<T>` → no emite
    coerce innecesario (inner ya tipa T).

  **Workaround removido en boilerplate 6**: los 5 helpers
  (`create_raw`/`find_raw`/`list_raw`/`update_raw`/
  `delete_raw`) vuelven al patrón inline original
  `return Ok(json.dumps(raw)?)`. El v0.9.52 los había
  modificado a binding intermedio anotado como workaround
  explícito.

- **Fmt regression de v0.9.51 (`src/parser.rs`)** ✓. El cambio
  del F15 recovery sub-stmt en v0.9.51 introdujo formato
  no-canonical en el `match self.expect_ident(...)` que
  `cargo fmt --check` (activado en v0.9.48) detectó en CI.
  Aplicado `cargo fmt` al archivo. CI strict ahora pasa.

### Notes

- **Tests nuevos**: 3 unit en `codegen::tests`:
  - `ok_propagation_coerce_pyany_a_str_adentro_de_return_ok`
  - `ok_propagation_coerce_pyany_a_int_adentro_de_return_ok`
  - `ok_propagation_inner_ya_correcto_no_emite_coerce_innecesario`
- Suite total: **2285 default** (era 2282 + 3), **2376 python**
  (era 2373 + 3), **2390 lsp** (era 2387 + 3). Clippy
  `--all-targets -D warnings` limpio en los 3 modos.
  `cargo fmt --check` ahora pasa (la regresión de v0.9.51
  estaba bloqueando CI desde v0.9.52).
- **Sin cambios a la extensión VSCode** — fix puramente del
  codegen.

### Bundle B parcialmente cerrado

Con v0.9.53, 1 de las 2 deudas restantes del bundle B (Python
interop codegen) cierra:

| Deuda | Estado |
|-------|--------|
| ~~8.7-ok-propagation~~ | ✓ **CERRADO v0.9.53** |
| dict→Map<K,V> no primitivos | sigue pendiente (4-6h) |

**Inventario depurado** post-v0.9.53: **3 deudas reales
restantes**:

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| dict→Map<K,V> no primitivos | Python interop | 4-6h |
| R.bug-pyo3-abi3 Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |

## [v0.9.52] — 2026-05-24 — Smoke real Docker boilerplate 6 (Dockerfile.distroless) end-to-end VERDE

### Added

- **Smoke real Docker boilerplate 6 (`Dockerfile.distroless`)
  validado END-TO-END** ✓. La deuda menor que v0.9.50 dejó como
  "paralela al boilerplate 5" cierra acá. Stack completo de 3
  servicios:
  - **api** (distroless con binario standalone — `fitz build
    --bundle-pip-requirements`): **imagen final 136 MB real**
    (igual que boilerplate 5 — CPython 3.14.5 + sqlalchemy +
    psycopg2-binary embebidos).
  - **frontend** (nginx alpine) sirviendo el SPA estático
    desde port 8080.
  - **db** (postgres 16-alpine) con healthcheck.
  - **CORS preflight** OPTIONS desde `Origin:
    http://localhost:8080` responde HTTP 204 con
    `access-control-allow-origin` + `access-control-allow-methods`
    + `access-control-allow-headers` correctos (`@middleware
    (cors({...}))` del api funciona en runtime distroless).
  - **HTTP smoke**: POST `/tasks` crea (devuelve task tipado
    desde Postgres), GET `/tasks?filter=all` lista, frontend
    SPA HTTP 200 con 20679 bytes.

- **`docker-compose.distroless.yml`** sumado al boilerplate 6
  con los 3 servicios (api distroless + nginx + postgres).
  Listo para `docker compose -f docker-compose.distroless.yml
  up --build` directo.

### Changed

- **`Dockerfile.distroless` del boilerplate 6**: fix bug
  preexistente (intentaba `COPY web/` que no existe — el dir
  real es `frontend/`). Ahora copia solo lo necesario para el
  api (sin frontend assets — el SPA vive en el container nginx
  separado, consistente con el `Dockerfile` actual).
- **`src/data/tasks.fitz` del boilerplate 6**: workaround
  v0.9.52 para el bug **8.7-ok-propagation** (deuda residual
  del codegen Python que SIGUE abierta). Los 5 helpers
  `create_raw`/`find_raw`/`list_raw`/`update_raw`/`delete_raw`
  ahora usan binding intermedio anotado `let s: Str = json.
  dumps(raw)?` en lugar de `return Ok(json.dumps(raw)?)`
  inline. Sin esto, `fitz build` falla con `expected String,
  found __FitzPyObject` adentro del `Ok(...)`. Cuando
  8.7-ok-propagation cierre, los 5 helpers vuelven al patrón
  inline original. NO afecta `fitz run` (el intérprete tipa
  correctamente).

### Notes

- **Sin cambios de código del lenguaje** — solo nuevo
  `docker-compose.distroless.yml` + workaround del bug
  8.7-ok-propagation en el boilerplate + actualizaciones de
  docs. Suite intacta: 2282 default / 2373 python / 2387 lsp.
- **Inventario depurado** post-v0.9.52: **4 deudas reales
  restantes**:

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| 8.7-ok-propagation | Python interop | 3-5h |
| dict→Map<K,V> no primitivos | Python interop | 4-6h |
| R.bug-pyo3-abi3 Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |

  El bundle G del inventario original (3 deudas: smoke 5 +
  multi-arch + python-image) entera CERRADA con los releases
  v0.9.49 (audit + 2 ya cerradas) + v0.9.50 (smoke 5) +
  v0.9.52 (smoke 6). Bundle B sigue como el más obvio para
  destrabar el flow `--bundle-pip-requirements` sin workarounds.

## [v0.9.51] — 2026-05-24 — Mini-tanda J: LSP polish (UTF-8 capability + F15 recovery sub-stmt)

### Added

- **Capability `positionEncoding: utf-8` declarada en el LSP
  server** (`fitz-lsp`). Pre-fix asumía implícitamente UTF-8 sin
  declararlo en `capabilities`; clientes que negocian UTF-16
  default (spec LSP por defecto) rompían con chars multi-byte
  (emoji, símbolos matemáticos, scripts del SMP). Post-fix
  explicit. VSCode + tower-lsp soportan UTF-8 desde LSP 3.17
  (julio 2022). Decisión técnica: mantener consistencia con
  `TypeEnv`/`TypeInfo`/`DefinitionInfo` que indexan por chars
  Unicode 1-based del lexer (`column += 1` por char no-newline
  en `lexer.rs::advance`).

- **F15 recovery sub-stmt — `Expr::Field` con field vacío**
  cuando el parser encuentra `<expr>.<EOF|Newline|otro>` en
  modo recovery. Pre-fix: el stmt entero se descartaba como
  `Stmt::Error` y el LSP solo podía recuperar completion vía
  el fallback "walk top-level por nombre" (cubría vars
  top-level, NO locales/params). Post-fix: el `Expr::Field
  { object, field: "", span }` queda en el AST, el checker lo
  tipa via TypeInfo, y el completion ve el tipo del `object`
  directamente — funciona para vars locales/params/cualquier
  scope.

  Impacto en completion contextual:
  - `user.<EOF>` con `let user: User = ...` dentro de una fn →
    el completion muestra los fields/métodos de `User`
    (pre-fix solo funcionaba si `user` era top-level).
  - `desconocido.` (ident sin binding) → tipa `Type::Any`
    (gradual escape del checker) → muestra los 6 métodos
    universales de F13.D (`as_int`/`as_float`/`as_str`/
    `as_bool`/`as_bytes`/`type_name`). Pre-fix devolvía vacío
    porque el stmt entero se descartaba.

### Changed

- **`position_to_offset` y `offset_to_position`** (`src/lsp.rs`):
  doc actualizado para reflejar `positionEncoding: utf-8`
  declarada en capabilities. Sin cambio funcional (ya contaban
  chars Unicode, ahora documentado correctamente).
- **`parse_postfix`** (`src/parser.rs`): branch `Token::Dot`
  ahora maneja `expect_ident` fallido bajo `recovery_mode`
  preservando el `Expr::Field` con `field: ""` en lugar de
  propagar el error que descartaba el stmt entero.

### Notes

- **Tests nuevos**: 4 unit en `lsp::tests`
  (`position_to_offset_cuenta_chars_unicode_no_utf16_code_units`,
  `offset_to_position_cuenta_chars_unicode_paralelo_a_position_to_offset`,
  `f15_recovery_sub_stmt_preserva_field_access_con_dot_huerfano`,
  `f15_recovery_sub_stmt_completion_after_dot_funciona_sobre_var_local`).
- **Tests ajustados**: 1 unit
  (`after_dot_sobre_receiver_sin_tipo_devuelve_metodos_any`,
  renombrado de `..._devuelve_vacio`) — cambia las expectativas
  para reflejar el nuevo comportamiento (F15 + F13.D
  combinados): ident sin binding ahora tipa Any y muestra los
  6 métodos universales en lugar de devolver vacío.
- Suite total: **2387 unit con lsp** (era 2383 + 4),
  **2373 con python**, **2282 sin features**. Clippy
  `--all-targets -D warnings` limpio en los 3 modos.
- **Sin cambios a la extensión VSCode** — la capability LSP
  negocia automáticamente al conectar.

### Bundle J cerrado

Con v0.9.51, el bundle J del inventario está completo. Las 2
deudas reales del LSP residuales (UTF-16 position strict + F15
recovery sub-stmt) cierran. **Inventario depurado** ahora baja
a **5 deudas reales restantes**:

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| 8.7-ok-propagation | Python interop codegen | 3-5h |
| dict→Map<K,V> no primitivos | Python interop codegen | 4-6h |
| R.bug-pyo3-abi3-portable-link Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |
| Smoke real Docker boilerplate 6 | Validación | 1-2h |

## [v0.9.50] — 2026-05-24 — Smoke real Docker boilerplate 5 (Dockerfile.distroless) validado end-to-end con Postgres

### Added

- **Smoke real Docker boilerplate 5 (`Dockerfile.distroless`)
  validado END-TO-END con Postgres** ✓. La deuda menor que
  v0.9.46 dejó pendiente ("path técnico correcto; validación
  funcional pendiente") y que v0.9.49 documentó como abortada
  por tiempo, **cierra finalmente acá** (el build se completó
  en background mientras avanzamos con docs):
  - Build con `Dockerfile.distroless` + `--bundle-pip-requirements`
    completó OK (~10 min cargo install desde source con
    `python:3.14-slim-bookworm` builder).
  - **Imagen final: 136 MB real** (vs ~80-100 MB esperado por
    el plan original — el binario standalone con CPython 3.14.5
    + sqlalchemy + psycopg2-binary embebidos pesa más de lo
    estimado en abstracto). Sigue siendo **15% más chica que
    los ~155 MB del Dockerfile actual** con `python:3.12-slim`
    + `fitz run`.
  - Runtime `gcr.io/distroless/cc-debian12` arranca limpio,
    boot logs `[boot] DB conectada y schema inicializado` +
    `[ready] Server arrancando en :3000` correctos.
  - **Smoke con curl end-to-end OK**: POST `/users` + GET `/users`
    (devuelve `[{"id":1,"name":"Ada","email":...},{...}]`
    tipado) + GET `/users/1` (instance individual tipada). Toda
    la cadena Fitz HTTP + SQLAlchemy + psycopg2 + Postgres
    funcional adentro del runtime distroless.
- **`docker-compose.distroless.yml`** sumado al boilerplate 5
  con la imagen + Postgres listos para `docker compose -f
  docker-compose.distroless.yml up --build` directo.

### Notes

- **Boilerplate 6 (fullstack)** sigue pendiente como deuda menor
  más chica — el patrón del Dockerfile.distroless es paralelo
  al 5 (mismo structure + frontend SPA estático). Smoke real
  con docker-compose tomaría ~10-15 min adicionales de build.
  Path técnico ya validado; queda como ~1-2h de trabajo
  paralelo, no bloqueante.
- **Sin cambios de código del lenguaje** — solo nuevo
  `docker-compose.distroless.yml` + actualizaciones de docs.
  Suite intacta: 2282 default / 2373 python / 2383 lsp.

## [v0.9.49] — 2026-05-24 — Audit-G: audit completo del inventario + 4 deudas confirmadas como ya cerradas

### Changed

- **Dockerfiles distroless**: `FITZ_TAG` default actualizado de
  `v0.9.46` → `v0.9.48` (boilerplates 5 + 6) — usar el release
  más reciente con CI strict (`cargo fmt --check` + `cargo
  clippy --all-targets`) ya activado en `ci.yml`.

### Audit del inventario (sin cambios funcionales)

Después de descubrir 2 sesiones consecutivas con deudas stale
(v0.9.47 — 3 LSP ya cerradas; v0.9.48 — 11 errores clippy ya
cerrados), Audit-G dedicó la sesión a verificar el resto del
inventario. **4 deudas más confirmadas como YA cerradas**:

- **F13 — heterogéneos en codegen** (Baja): SPIKE `__FitzValue`
  con variantes Int/Float/Str/Bool/Null + Bytes + Nominal. Smoke
  `[1, "dos", true]` (List<Any>) compila con `fitz build` y
  produce `[1, "dos", true]` bit-a-bit con `fitz run`.
  Auto-detectado en `gen_list_lit` cuando aparece un `List<Any>`
  literal.
- **8.7-await-binding-split** (Python interop): cerrado con
  dispatch al helper `__fitz_py_await_obj` cuando el inner del
  `.await` tiene `inner_ty == PyAny`. Tiene test
  `py_await_split_emite_fitz_py_await_obj`.
- **multi-arch-docker**: ya implementado en `release.yml` Job 3
  `docker-image` con buildx `--platform linux/amd64,linux/arm64`.
- **fitz-python-image**: ya implementado en `release.yml` Job 3b
  con tag `:latest-python`.

**Inventario depurado**: deudas reales restantes ahora bajan a
7 (de 13+ que figuraban en los documentos):

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| 8.7-ok-propagation | Python interop codegen | 3-5h |
| dict→Map<K,V> no primitivos | Python interop codegen | 4-6h |
| UTF-16 position strict | LSP | 2-3h |
| F15 recovery sub-stmt | LSP | 1-2h |
| R.bug-pyo3-abi3-portable-link Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |
| Smoke real Docker boilerplate 5/6 | Validación | 2-3h |

### Notes

- **Convención nueva** (tercera vez consecutiva que aparece
  inventario stale — pattern claro): antes de prometer trabajo
  en un bundle de deudas, hacer audit rápido (10-15 min) con
  comandos directos (`grep` por nombres de fns/features, `cargo
  clippy --all-targets`, reproducir con `.fitz` mínimo +
  `fitz build`). Documentado en `docs/deudas-post-5b.md`.
- **Sin cambios de código del lenguaje** en este release. Suite
  intacta: 2282 default / 2373 python / 2383 lsp. Único cambio:
  bump del `FITZ_TAG` default en los 2 Dockerfile.distroless
  (`v0.9.46` → `v0.9.48`) + audit/documentación del inventario.
- **Smoke real Docker boilerplate 5 (`Dockerfile.distroless`)**:
  arrancado al final de la sesión pero abortado por tiempo
  (build con `cargo install fitz --features python` desde
  source toma 10+ min). Queda como deuda menor explícita. (El
  build se completó en background después del commit — cierre
  efectivo en v0.9.50.)

## [v0.9.48] — 2026-05-24 — Mini-tanda Cleanup-D: cargo fmt --all masivo + clippy --all-targets reactivado en CI

### Changed

- **`cargo fmt --all` aplicado masivamente** (14 archivos
  reformateados: `src/asyncapi.rs`, `src/codegen.rs`,
  `src/evaluator.rs`, `src/http.rs`, `src/launcher_template.rs`,
  `src/lib.rs`, `src/lsp.rs`, `src/main.rs`, `src/pbs.rs`,
  `src/pyi_stub.rs`, `src/types.rs`, `tests/bundle_python_e2e.rs`,
  `tests/cli_e2e.rs`, `tests/compile_e2e.rs`). El repo nunca había
  pasado por rustfmt canónico desde el inicio del proyecto; el
  CI lo tenía deshabilitado con nota de "preferencias del autor
  difieren del default". Cleanup-D aplica el formato canónico
  para alinear con la convención del ecosistema Rust y desbloquear
  el step `fmt --check` en CI.
- **`ci.yml` actualizado**:
  - `cargo fmt --check` reactivado (bloquea diff a futuro).
  - `cargo clippy --all-targets` reactivado (era `--lib` solo).
    La deuda original de "11 errores en tests" cerró a lo largo
    de mini-tandas previas; al verificar con `cargo clippy
    --all-targets --all-features -- -D warnings` la suite pasa
    limpia en los 3 modos (default, `python`, `lsp`).

### Notes

- **Cero cambios funcionales**: `cargo fmt` solo modifica
  whitespace/line breaks. Toda la lógica del lenguaje +
  comportamiento generado es idéntico bit-a-bit.
- **Suite verde post-fmt** en los 3 modos:
  - Sin features: **2282 unit** (igual que antes del fmt).
  - Con `python`: **2373 unit**.
  - Con `lsp`: **2383 unit**.
  - Clippy `-D warnings` limpio en los 3 modos + `--all-targets`.
- **`cargo outdated` skipeado**: el plugin `cargo-outdated` no
  está instalado en la máquina dev. Sin presión real de
  vulnerabilidad, dejamos el audit de bumps para una sesión
  futura cuando aparezca caso de uso concreto (ej. CVE en una
  dep transitiva). Las deps principales del repo (`pyo3`,
  `axum`, `tokio`, `serde`) están en versiones recientes según
  Cargo.toml.
- **Mini-tanda Cleanup-D — cierre del último ítem del bundle D
  del inventario de deudas post-v0.9.46**: junto con los cierres
  parciales de v0.9.45 (4 deudas chicas del lenguaje) y v0.9.47
  (LSP completion + chain), el repo queda en estado profesional
  para colaboradores. Bundle D estaba siendo pospuesto release
  tras release ("sin presión") — su cierre saca ruido del
  inventario y permite enfocar las próximas mini-fases en
  features reales.

## [v0.9.47] — 2026-05-24 — Mini-tanda LSPz: completion en `from mod import` + chain `a.b.c.`

### Added

- **Completion en `from <mod> import |`** (LSP). El cursor adentro
  de la lista de imports de un `from <mod> import` ahora sugiere
  los símbolos exportables del módulo target (fns con firma
  completa, types, consts/let top-level). Funciona también con
  items previos (`from foo import X, Y, |`) y módulos con path
  punteado (`from sub.utils import |`).

  Implementación:
  - Nueva variante `CompletionContext::FromImportList { mod_path:
    Vec<String> }` en `src/lsp.rs`.
  - Helper nuevo `detect_from_import_list_context(text, line,
    character)` que walkea back-to-front del cursor, saltando
    items previos (`<ident>,?\s*`), y matchea el patrón `from
    <mod_path> import`. Devuelve `mod_path` segmentado por `.`.
  - Helper público nuevo `from_import_completions(doc_uri,
    mod_path)` que resuelve el archivo target relativo al doc URI
    (convención del loader: `["foo"]` → `<base>/foo.fitz`),
    parsea con `parse_with_recovery`, y enumera fns + types +
    consts top-level. Tolera módulos inexistentes (devuelve vacío).
  - Nueva variante pública `completion_at_position_with_uri` que
    acepta `doc_uri: Option<&Url>` y la pasa al contexto
    `FromImportList`. La firma original `completion_at_position`
    se mantiene como wrapper (`doc_uri = None`) para
    backward-compat de tests/herramientas externas.
  - El backend del LSP (`src/bin/fitz-lsp.rs`) ahora invoca el
    wrapper `_with_uri` pasando el URI del documento abierto.

- **Chain `a.b.c.` en after-dot completion**. El completion
  contextual tras un punto ahora reconoce chains de N segmentos
  (no solo `<ident>.`). Pre-fix: `obj.field.|` interpretaba el
  receiver como `field` (último ident) y buscaba sus métodos
  como si fuera Str/List/etc. Post-fix: el receiver es el chain
  entero `obj.field`, y el lookup en TypeInfo por la posición del
  START del primer ident resuelve al tipo del Expr::Field más
  exterior (el chain completo). El comportamiento se apoya en la
  garantía de TypeInfo (F16) de que el último `record` por
  posición es el tipo del nodo más externo.

  Implementación: en `detect_completion_context`, el walkback
  desde el `.` ahora acepta `is_ident_continue(c) || c == b'.'`
  (antes solo `is_ident_continue`). Validación de shape:
  rechaza chains que empiecen/terminen con `.` o tengan `..`
  consecutivos.

### Changed

- **Doc comment de `completion_at_position`**: actualizado para
  reflejar que la deuda visible "Chain `a.b.c.`" cerró en v0.9.47.

### Notes

- **Tests nuevos**: 8 unit en `lsp::tests` (5 sobre
  `detect_context_chain_*`/`from_import_*` + 2 sobre
  `from_import_completions_*` + 1 sobre el backward-compat
  `completion_at_position_sin_uri_no_completa_from_import`).
  Suite total: **2383 unit con lsp** (era 2375 + 8), **2282 sin
  features**. Clippy `-D warnings` limpio en los 3 modos (default,
  `python`, `lsp`).
- **Descubrimiento del bundle**: las otras 3 deudas LSP del
  inventario original ya estaban implementadas en mini-tandas
  previas (LSPx para cross-module go-to-def, LSPy.4 para
  scope-aware completion, LSPy para hover con range exacto via
  `make_hover_with_range`/`ident_range_at_position`). El bundle
  E redujo a 2 deudas reales (completion en imports + chain) y se
  cerró en una sola sesión.
- **Sin cambios a la extensión VSCode** — solo cambios al
  backend del LSP. Los clientes existentes (extensión VSCode,
  vim-lsp, helix, etc.) reciben las nuevas completions
  automáticamente al conectar al `fitz-lsp` actualizado.

### Deuda residual LSP (NO bloquea uso real)

- **UTF-16 position strict**: el LSP por default usa UTF-16 para
  `character` en `Position`. Fitz LSP usa UTF-8 (asume programas
  ASCII-dominantes). Refinable post-MVP si aparece presión real
  con código en idiomas no-latin.
- **F15 recovery sub-stmt**: errores adentro de un stmt
  descartan el stmt entero — refinable para completion fino
  tras `user.<typo>`.

## [v0.9.46] — 2026-05-24 — Bundling Docker end-to-end: distroless habilitado + Dockerfile.distroless en boilerplates 5/6

### Added

- **distroless-tar-embedded — launcher con `tar` + `flate2` inline**.
  Cierre de la deuda residual de Fase 8.b/8.c documentada al cerrar
  el bundling de Python: el launcher del binario standalone
  (generado por `fitz build --bundle-python` / `--bundle-pip*`)
  invocaba `Command::new("tar")` subprocess para extraer los
  tarballs PBS + pip a `$TMPDIR/fitz-py-<hash>/`. Esto requería
  `tar` instalado en el runtime de Docker — `gcr.io/distroless/cc-
  debian12` no lo trae, así que el runtime mínimo viable era
  `debian:bookworm-slim` (~85 MB base). Post-fix: el launcher usa
  crates `tar = "0.4"` + `flate2 = "1"` para extraer en memoria,
  sin subprocess. Distroless ahora es viable como runtime.

  Implementación:
  - **`extract_tar_gz(tarball_path, dest)`** — nuevo helper inline
    en `LAUNCHER_MAIN_RS_TEMPLATE` con `flate2::read::GzDecoder` +
    `tar::Archive::unpack`. El crate `tar` valida paths contra
    `../` escapes (CVE protection automática).
  - **`LAUNCHER_CARGO_TOML_TEMPLATE`** suma `tar = "0.4"` +
    `flate2 = "1"` a `[dependencies]`. Los 2 crates suman ~80-100
    KB al binario final del launcher con LTO + strip activos
    (perfil `opt-level = "z"` se mantiene minimalista). Trade-off
    aceptable vs el ahorro de ~60 MB en la imagen de container
    final.
  - **3 sitios reemplazados**: PBS tarball extract (extracción de
    CPython embebido) + pip tarball extract en Linux/macOS (path
    `python/lib/python3.X/site-packages/`) + pip tarball extract
    en Windows (path `python/Lib/site-packages/`). Los 3 ahora
    usan `extract_tar_gz` en lugar de `Command::new("tar")`.
  - El binario final del launcher en `fitz build --bundle-python`
    pesa ahora ~80-100 KB más que pre-fix; sin diferencia
    observable en el tamaño total del binario standalone
    (`examples/python-interop-8.b.exe` mantiene ~22 MB en
    Windows x64).

- **`Dockerfile.distroless` en boilerplates 5 (api-postgres-python)
  y 6 (api-fullstack-postgres)**: variante alternativa al
  `Dockerfile` actual con el flow `fitz build --bundle-pip-
  requirements` + runtime `gcr.io/distroless/cc-debian12`.
  Builder pineado a `python:3.14-slim-bookworm` (fix GLIBC: la
  variante `slim` default es trixie con GLIBC 2.39, incompatible
  con el runtime bookworm GLIBC 2.36). Imagen final esperada:
  **~80-100 MB** (vs ~155 MB del Dockerfile actual con
  `fitz run` + Python en runtime). El Dockerfile actual queda
  sin cambios — es el path "seguro" mientras la validación smoke
  funcional con Postgres real avanza.

### Notes

- **Tests nuevos**: 3 unit en `launcher_template::tests`
  (`template_cargo_toml_incluye_deps_tar_y_flate2`,
  `template_main_rs_define_extract_tar_gz_y_no_invoca_tar_subprocess`,
  `gen_launcher_main_rs_pip_block_usa_extract_tar_gz`). Los 2 E2E
  existentes (`template_launcher_compila_con_paths_dummies`,
  `template_launcher_compila_con_path_windows_y_espacios`) siguen
  verdes y validan que el template Rust resultante compila con
  las nuevas deps.
- **Smoke real bundling**: `fitz build --bundle-python
  examples/python-interop-8.b.fitz` produce el binario standalone
  (~22 MB Windows x64) y el binario corre limpio bit-a-bit con la
  versión pre-fix. Validado a mano con cache TMP vacía + cache hit.
- Suite total: **2373 unit con python** (era 2370 + 3),
  **2282 sin python** (era 2279 + 3). Clippy `-D warnings` limpio
  en ambos modos. Sin cambios a la extensión VSCode (cambio del
  codegen del launcher, no del lenguaje).
- **Deuda residual derivada** (NO bloquea, queda como sub-paso
  futuro): smoke real Docker end-to-end del `Dockerfile.distroless`
  con sqlalchemy + psycopg2 + Postgres cliente. El path técnico
  está correcto (todos los blockers documentados cerrados); la
  validación funcional completa requiere ~30 min de setup Docker
  + Postgres y queda como tarea independiente cuando aparezca
  presión real de adopt.

## [v0.9.45] — 2026-05-24 — Mini-tanda Cleanup-A: 4 deudas chicas del lenguaje cerradas

### Added

- **Fix sqrt-shadowing**: las fns importadas con el mismo nombre
  que un builtin matemático del codegen (`sqrt`, `pow`, `abs`,
  `ceil`, `floor`, `round`, `clamp`, `min`, `max`, `popcount`,
  `leading_zeros`, `trailing_zeros`, `spawn`, `len`, `bytes`,
  `sleep`, `env`, `env_or`, `load_env`) ahora tienen precedencia
  correctamente. Pre-fix: el codegen chequeaba sólo
  `fn_sigs.contains_key(name)` para decidir si emitir el método
  nativo (`(x).sqrt()`) o respetar el user override. Las fns
  importadas vivían en `module_bindings`, no en `fn_sigs`, así que
  `from utils import sqrt` + `sqrt(x)` se traducía
  incorrectamente al método f64. Post-fix: nuevo helper
  `CodegenCtx::is_user_callable(name)` chequea ambos. 14 call
  sites migrados. 3 tests nuevos (`build_fn_importada_con_nombre_
  de_builtin_matematico_no_es_shadeada`, `build_fn_importada_con_
  nombre_pow_no_es_shadeada`, `build_fn_local_con_nombre_de_
  builtin_sigue_funcionando_como_antes`).

- **F14 ampliado — `let X = <expr no literal>` a nivel top de
  módulo**: el caso ya estaba cubierto vía accessor fns desde la
  mini-tanda F14 original (v0.9.x), pero no había tests para
  literales compuestos (List/Map/Instance). 3 tests nuevos
  (`modulo_top_level_let_lista_literal_se_emite_como_accessor_fn`,
  `modulo_top_level_let_map_literal_se_emite_como_accessor_fn`,
  `modulo_top_level_let_instance_se_emite_como_accessor_fn`)
  sellan la cobertura.

- **F3 ampliado — `return`/`break`/`continue` huérfanos**: el
  check estático ya estaba implementado (`return_stack`/
  `loop_depth` en `CheckCtx`) con 3 tests cubriendo cada caso
  (`return_huerfano_top_level_es_error`, `break_huerfano_es_
  error`, `continue_huerfano_es_error`). Sin cambios al código —
  solo actualizamos la documentación de deudas-post-5b para
  marcar F3 como cerrado.

### Documented

- **F1 — Matriz de uso de `Type::Any` (audit)**: los ~180 sitios
  donde aparece se clasificaron en 9 categorías intencionales
  (builtins variádicos, builtins polimórficos, propagación
  gradual, fallback de anotaciones, callbacks sin anotación,
  patterns de match, `Expr::Error` F15, `Result<Any>`/
  `Future<Any>` placeholder, propagación de `PyAny`). La doc
  del enum `Type` en `src/types.rs` describe cada categoría +
  qué NO debe aparecer (anti-patterns que sí serían bugs). Sin
  cambios de código — audit ratifica que el uso actual es
  correcto.

### Notes

- **Tests nuevos**: 6 unit (3 sqrt-shadowing + 3 F14 ampliado).
  Suite total: 2370 unit con python (era 2364 + 6), 2279 sin
  python (era 2273 + 6). Smoke `GUIDE_EXAMPLES_COMPILE` verde.
  Clippy `-D warnings` limpio en ambos modos.
- **Cierre formal de la mini-tanda Cleanup-A**: bundle pragmático
  de 4 deudas chicas relacionadas como "limpieza de lenguaje
  menor". F1 (audit/docs), F3 (ya cerrado, solo docs), F14
  (ampliado con tests), sqrt-shadowing (fix real). Decisión de
  scope (vs bundle más grande con `8.7-ok-propagation`,
  `8.7-await-binding-split`, `dict→Map<K,V>`, F13): mantener
  cada deuda mediana/grande como mini-fase dedicada para no
  acumular riesgo en un solo cierre.
- **Sin cambios a la extensión VSCode** — no se introduce sintaxis
  nueva.

## [v0.9.44] — 2026-05-24 — Cierre sub-deuda 1.5/1.6: coerción + impls HTTP para tipos importados en `fitz build`

### Added

- **Codegen — helpers Python para tipos custom definidos en módulos
  transitivos**. Cierre de la **sub-deuda 1.5** que emergió al cerrar
  v0.9.43 (cuando el smoke real del boilerplate 5 con `fitz build`
  pasó el rechazo del 8.7.1 transitiva y reveló que los helpers
  `__fitz_py_to_instance_<T>`/`__fitz_py_to_list_<T>` solo se emitían
  para tipos definidos en main, no para tipos importados desde otros
  módulos). El error pre-fix era `cannot find function
  __fitz_py_to_instance_User in this scope`.

  Implementación:
  - **`pub(crate)`** sobre los helpers `__fitz_py_to_instance_<T>` y
    `__fitz_py_to_list_<T>` en main (antes `fn` privadas) — necesario
    para que módulos los referencien con `crate::__fitz_py_to_*`.
  - **`gen_python_helpers_for_type(name, &sig)`** — nuevo método del
    `CodegenCtx` que extrae las 2 `impl __FitzToPy` + helpers
    Python→Fitz desde `gen_type_def`. Reusable para tipos del main Y
    para tipos importados.
  - **`emit_helpers_for_imported_types(loader, do_python, do_http)`**
    — pase unificado nuevo invocado desde `emit_main_rs_body` después
    de emitir los tipos locales. Para cada tipo custom de cada módulo
    cargado del proyecto: emite `#[allow(unused_imports)] use
    crate::<qualifier>::{T, TData};` (si no está ya importado al
    main) + opcionalmente los impls HTTP (`__ToFitzJson`/
    `__FromFitzJson`) + opcionalmente los Python helpers
    (`__FitzToPy` + `__fitz_py_to_instance_<T>` +
    `__fitz_py_to_list_<T>`). Dedup por nombre para evitar emitir
    helpers duplicados si dos módulos definen tipos con el mismo
    nombre.
  - **Post-procesamiento del output de cada módulo**
    (`prefix_module_py_nominal_helpers`): pasada lineal sin regex que
    prefija `crate::` a las referencias `__fitz_py_to_instance_<Cap>(`
    y `__fitz_py_to_list_<Cap>(` (con `<Cap>` capitalizado = Nominal).
    Los helpers primitivos `__fitz_py_to_list_i64/f64/string/bool`
    (lowercase) NO se tocan — ya se importan via `use crate::{...}`.
    Idempotente (no duplica `crate::crate::`).
  - **`emit_module_python_use_decls`** ahora suma `use crate::
    {__fitz_py_to_list_i64, __fitz_py_to_list_f64,
    __fitz_py_to_list_string, __fitz_py_to_list_bool}` (helpers
    primitivos del crate root) además de los helpers Python ya
    importados antes.

- **Codegen — impls HTTP (`__ToFitzJson`/`__FromFitzJson`) para
  tipos importados de módulos**. Cierre de la **sub-deuda 1.6**
  paralela a la 1.5 pero del lado HTTP: handlers que aceptan o
  devuelven tipos `T` importados (e.g. `fn create(u: NewUser) ->
  Result<User>`) fallaban en `fitz build` con `the trait bound
  NewUserData: __FromFitzJson is not satisfied` y `method
  __to_fitz_json exists for struct Arc<Mutex<UserData>>, but its
  trait bounds were not satisfied`.

  Implementación:
  - **`gen_type_http_impls_for_sig(name, &sig)`** — extraído de
    `gen_type_http_impls` para reusabilidad. Emite los 2 impls
    `__ToFitzJson` + `__FromFitzJson` dado nombre + sig.
  - El pase unificado `emit_helpers_for_imported_types` lo invoca
    cuando `has_http = true` (sin requerir `uses_python`). Cubre
    boilerplates HTTP que importan tipos de módulos sin tocar
    Python.

- **Bug fix preexistente — `mod types; mod types;` duplicado**.
  `ModuleLoader::emit_mod_decls` emitía `mod <root>;` por cada
  módulo cargado; cuando dos módulos compartían parent dir
  (`types/user.rs` + `types/api.rs`), `mod types;` aparecía dos
  veces y rustc fallaba con `E0428: the name 'types' is defined
  multiple times`. Fix: dedup por root segment con `HashSet`. El
  bug bloqueaba `fitz build` de cualquier proyecto multi-archivo
  con dos módulos en la misma carpeta — descubierto al validar la
  sub-deuda 1.5/1.6.

### Changed

- **READMEs de boilerplates 5 (api-postgres-python) y 6
  (api-fullstack-postgres)**: blocker #1.5 (coerción
  `__fitz_py_to_*_T` para tipos importados) marcado como CERRADO.
  Quedan 2 caveats menores (GLIBC mismatch + tamaño real ~10-20 MB
  vs 50-70 MB del plan original) — ambos no-bloqueantes para el
  adopt real. Validado a mano: `fitz build` del boilerplate 5
  produce un binario que compila limpio, bootea Python y falla
  solo por `psycopg2` no instalado (config runtime, no del fix).
- **Guía cap 21.10 (Interop Python en `fitz build`)**: nota
  actualizada — la coerción `dict → Instance<T>` y `list →
  List<T>` para tipos `T` importados de otro módulo ya funciona;
  el caveat residual queda en tipos primitivos `Map<K,V>` opacos.

### Notes

- **Tests nuevos**: 4 unit en `codegen::tests::build_main_emite_
  helpers_py_*`/`build_modulo_referencia_helper_*`/`build_modulo_
  importa_helpers_py_*`/`build_emit_mod_decls_deduplica_*` + 1 E2E
  `fase_8_7_1_transitiva_bis_modulo_coerce_pyany_a_tipo_importado`
  (con feature `python`). El E2E valida end-to-end: módulo
  `parser.fitz` define `type User` + `from python import json` +
  `fn parse_default_user() -> Result<User>` que hace `let u: User
  = json.loads(raw)?`, main importa la fn + el tipo, matchea sobre
  `Ok(u)`, imprime `name=Fitz role=admin`.
- **Sin cambios a la extensión VSCode** — no se introduce sintaxis
  nueva.
- **Sub-deuda residual descubierta** (no bloqueante, queda como
  deuda menor del codegen): cuando una fn de módulo retorna
  `Result<Str>` y el body hace `let s = json.dumps(...)?` + `return
  Ok(s)`, el codegen infiere `s: PyAny` y no propaga la expectativa
  de `Str` adentro de `Ok(...)`. Workaround usado en el E2E nuevo:
  binding intermedio anotado `let raw: Str = raw_py`. Refinable
  con propagación de expected type adentro de `Ok(...)`.

## [v0.9.43] — 2026-05-23 — Cierre deuda codegen 8.7.1: `from python import` en módulos transitivos

### Added

- **Codegen — `from python import` adentro de módulos Fitz
  transitivos**. Cierre de la deuda residual de Fase 8.7.1
  documentada al cerrar la fase ("imports Python adentro de
  módulos transitivos no se soportan todavía. Workaround: poné el
  `from python import` en el main"). Cada módulo puede declarar
  sus propios imports Python sin obligar al main a participar.
  Patrón canónico: librerías Fitz que delegan operaciones a Python
  (numpy/scipy/sqlalchemy/redis-py) sin filtrar el detalle a quien
  las usa.

  Implementación:
  - `LoadedModule` gana `python_imports: Vec<PythonImport>`.
  - `ModuleLoader::load_module_inner` recolecta los imports
    Python del módulo con `collect_python_imports` antes del loop
    de procesado de imports Fitz.
  - `generate_module_rs_with_bindings` recibe los python_imports
    como nuevo parámetro, llama a `install_python_bindings`,
    emite `use crate::{__FitzPyObject, __fitz_py_*}` (reusa los
    helpers del preludio Python del crate root) y emite sus
    propios statics + getters locales con
    `emit_python_bindings_top_level`.
  - Nuevo método `emit_module_python_use_decls` orquesta los
    `use crate::__fitz_py_*` que el módulo necesita; gated por
    `uses_async` para `__fitz_py_invoke_await`/`__fitz_py_await_obj`.
  - Los helpers del preludio Python pasan de `fn` a `pub(crate)
    fn` (`__fitz_py_import`, `__fitz_py_get_attr_obj`,
    `__fitz_py_extract_*`, `__fitz_py_err_to_string`,
    `__fitz_py_invoke`, `__fitz_py_marshal_map_key`,
    `__fitz_py_invoke_await`, `__fitz_py_await_obj`,
    `__fitz_py_to_list_*`) para ser accesibles desde módulos
    del crate generado. El prefix `__` mantiene la convención de
    privacidad visual.
  - `uses_python` global = `main OR cualquier módulo transitivo`.
    Si solo módulos transitivos usan Python, el main igual emite
    el preludio entero (los `use crate::__fitz_py_*` lo requieren)
    y Cargo.toml suma pyo3 igual.

  pyo3 cachea via `sys.modules`, así que dos módulos importando
  el mismo módulo Python (`from python import math` en main y en
  utils) no pagan doble inicialización — solo el OnceLock
  duplicado (casi cero overhead real).

  6 tests nuevos cubren el comportamiento (5 unit + 1 E2E con
  feature `python`): no falla cuando antes fallaba, emite
  `use crate::__fitz_py_*` + statics locales en el módulo, main
  emite preludio + Cargo.toml suma pyo3 cuando solo módulos
  transitivos usan Python, y `fase_8_7_1_transitiva_build_from_
  python_en_modulo_compila_y_corre` valida el caso completo
  end-to-end (programa con `pymath.fitz` que importa
  `python.math`, main que lo importa, paridad bit-a-bit
  `fitz run` ↔ `fitz build`).

  Ejemplo runnable nuevo:
  [examples/python-interop-modular.fitz](examples/python-interop-modular.fitz)
  + [examples/python_math_utils.fitz](examples/python_math_utils.fitz)
  (validado a mano: `área(r=2) = 12.566370614359172`,
  `sqrt(16) = 4.0`, `sqrt(-1) → ValueError: ...`).

### Changed

- **Guía cap 16 (Módulos) — `from python import` transitivo**:
  la sección "Detalles del loader" pasa de listar la restricción
  como deuda a documentar el patrón canónico con pointer al
  ejemplo runnable.
- **Guía cap 21.10 (Interop Python en `fitz build`)**: sumada
  nota explícita sobre el caso transitivo.
- **READMEs de los boilerplates 5 (api-postgres-python) y 6
  (api-fullstack-postgres)**: blocker #1 (rechazo del codegen
  transitiva) marcado como CERRADO. Smoke real del boilerplate 5
  con `fitz build` reveló que la coerción `dict → Instance<T>` y
  `list → List<T>` para tipos `T` importados (helpers
  `__fitz_py_to_instance_T`/`__fitz_py_to_list_T` que hoy solo
  se emiten para tipos del main) es la próxima sub-deuda concreta
  para destrabar el adopt — documentada como blocker #1.5 en
  ambos READMEs. Ya estaba mencionada en el roadmap como deuda
  residual derivada de Fase 8.

### Notes

- **Smoke validado**: `cargo test --lib --features python` →
  2359 unit (+5 nuevos) sin regresiones; `cargo test --test
  compile_e2e fase_8_7_1_transitiva --features python` → 1 E2E
  nuevo OK; smoke `GUIDE_EXAMPLES_COMPILE` (sin feature python)
  → verde. Clippy `-D warnings` limpio. Sin cambios a la
  extensión VSCode (no se introduce sintaxis nueva — solo se
  levanta una restricción semántica del codegen).
- **Deuda residual derivada**: la coerción `__fitz_py_to_*_T`
  para tipos importados (blocker #1.5 de los boilerplates 5/6)
  queda como próxima prioridad concreta para destrabar el adopt
  real del flow `--bundle-pip-requirements` en proyectos con
  data layer separado. NO bloquea el caso de `fitz build` con
  programas simples donde el binding del tipo retornado por
  Python vive en el mismo archivo o solo es PyAny opaco.

## [v0.9.42] — 2026-05-23 — Cosecha 8.c + cache key del pip_packages tarball + smoke real Docker + VSCode drift audit

Release consolida cuatro piezas trabajadas en sesiones
consecutivas:

1. **Cosecha 8.c**: nuevo flag CLI `--bundle-pip-requirements
   <FILE>` (la sub-tanda original cerrada el mismo día con
   v0.9.42 commit).
2. **Cache key del pip_packages tarball** (deuda D documentada
   en el roadmap como menor de Fase 8.c): hash determinístico
   sobre los inputs del pip install (`--bundle-pip` positionals
   ordenados + bytes de los requirements files). Cache hit reusa
   el tarball existente sin re-correr pip install + tar — builds
   subsiguientes sin cambios en paquetes pasan de ~10-30s a
   ~instantáneo. Sidecar `<bin>_pip_packages.inputs_hash`
   adyacente al tarball.
3. **Smoke real Docker end-to-end** del flow `--bundle-pip-
   requirements` + Docker multi-stage + runtime debian-slim
   (cerrado VERDE con smoke alternativo flat).
4. **Audit de la extensión VSCode** vs el lenguaje actual: 15
   builtins faltaban en grammar TextMate, 5 en LSP completion.
   `.vsix` re-construido a 0.9.3.

Quick win continuando 8.c en la misma sesión. Nuevo flag
`--bundle-pip-requirements <FILE>` repetible que lee paquetes
desde un `requirements.txt` estándar en lugar de listarlos uno
por uno con `--bundle-pip`. Implica `--bundle-python` igual que
el flag hermano y es combinable con `--bundle-pip` (pip acumula
positionals + contenido del file).

Sin parsing del lado de Fitz: el archivo se pasa directo a
`pip install -r <file>`, así que toda la sintaxis nativa
funciona sin cambios — comentarios con `#`, includes
`-r other.txt`, version pins, `--hash`, índices alternos, etc.

### Cambios

- **Nuevo flag CLI `--bundle-pip-requirements <FILE>`**
  repetible en `Commands::Build`:
  ```bash
  # Equivalente a --bundle-pip sqlalchemy --bundle-pip ...
  fitz build --bundle-pip-requirements requirements.txt mi_app.fitz

  # Combinable con --bundle-pip
  fitz build \
    --bundle-pip-requirements requirements.txt \
    --bundle-pip "psycopg2-binary==2.9.10" \
    mi_app.fitz

  # Repetible (caso multi-stage típico)
  fitz build \
    --bundle-pip-requirements requirements.txt \
    --bundle-pip-requirements requirements-prod.txt \
    mi_app.fitz
  ```

- **Validación temprana**: cada path se canonicaliza y se lee
  antes de tocar lex/parse/PBS. Si el archivo no existe o no es
  legible, `fitz build` aborta con mensaje claro citando el
  path inválido. Cero overhead en el pipeline real.

- **Conteo combinado** para el log: `pip_total_count =
  bundle_pip.len() + líneas no-blank/no-comment del file`.
  El summary `pip install --target ({} paquete(s))…` y el
  banner final reflejan el total.

- **`pip_args` extendido**: por cada requirements file, se
  agregan `["-r", "<abs_path>"]` antes de los positionals.
  Pip los acumula naturalmente; no hay parsing del lado de
  Fitz (toda la sintaxis del archivo la maneja pip).

- **Hash combinado preservado**: si hay pip packages
  (de cualquier fuente — positionals o requirements files),
  el hash del extract TMP incluye los bytes del pip tarball
  resultante. Dos proyectos con distintos paquetes siguen
  teniendo distintos extract dirs (sin colisión).

### Tests

- **3 E2E tests nuevos** en `bundle_python_e2e.rs`:
  - `bundle_pip_requirements_implica_bundle_python_y_aborta_sin_from_python_import`
  - `bundle_pip_requirements_archivo_inexistente_aborta_con_mensaje_claro`
  - `bundle_pip_requirements_combinable_con_bundle_pip`

Tests E2E del bundling: 7/7 (4 previos + 3 nuevos). El happy
path real (build + run del binario standalone con
requirements.txt embebido) sigue siendo validación manual
porque requiere PBS tarball + red + tar + Python 3.14.x
en el builder (constraint heredado de 8.b en Linux/macOS).

### Cache key del pip_packages tarball (deuda D de Fase 8.c)

Antes: cada `fitz build --bundle-pip` o `--bundle-pip-
requirements` re-corría `pip install --target` + `tar -czf`
desde cero, aunque la lista de paquetes no hubiera cambiado.
Costo: 10-30s por build, peor en Docker layer rebuilds.

Ahora: helper `pip_inputs_hash(bundle_pip, requirements_
contents) -> String` computa hash determinístico FNV-1a 64-bit
sobre:
- Positionals `--bundle-pip` ordenados alfabéticamente
  (reordenar args NO invalida cache).
- Bytes de cada requirements file en orden CLI (reordenar
  archivos SÍ invalida — pip los procesa en orden con
  potenciales conflicts/overrides).
- Separador `\n---\n` entre las dos secciones.

Sidecar `<bin>_pip_packages.inputs_hash` adyacente al tarball.
En la próxima corrida, si tarball + sidecar existen y el
hash matchea el nuevo, se reusa todo (skip de PBS extract +
pip install + tar). Mensaje informativo: `→ pip cache hit
({N} paquete(s), hash {8 chars}…) — reusando tarball`.

### Smoke real Docker (findings)

Smoke alternativo en workspace temp con programa flat (`from
python import` solo en main, sin módulos transitivos):
binario standalone de 37.4 MB con CPython 3.14.5 + `requests`
embebido, ejecutado adentro de container `debian:bookworm-
slim`, GET `/version` devuelve `"2.34.2"` (versión de
`requests`) end-to-end. Cadena `--bundle-pip-requirements` +
Docker multi-stage + runtime debian-slim **VERDE**.

3 blockers descubiertos en el path original (boilerplates
5/6 con módulos transitivos):

1. **Deuda del codegen Fase 8.7.1**: `from python import` en
   módulos transitivos NO soportado. Boilerplates
   `api-postgres-python` y `api-fullstack-postgres` usan
   `from python import db` adentro de `src/data/*.fitz`
   (transitivos del main). Workaround del codegen actual:
   "poné el `from python import` en el main" — implica
   refactor invasivo del boilerplate (rompe separation of
   concerns del data layer wrapper Python).
2. **GLIBC mismatch**: `python:3.14-slim` (Debian trixie,
   GLIBC 2.39) ↔ `debian:bookworm-slim` (GLIBC 2.36) →
   binario linkea contra GLIBC del builder y crashea en
   runtime con "version 'GLIBC_2.39' not found". Fix:
   pinear builder a `python:3.14-slim-bookworm`. Documentado
   en los READMEs de los boilerplates afectados.
3. **Beneficio de imagen menor del esperado**: ~10-20 MB
   real (no 50-70 MB que prometía el plan original). El
   binario standalone con CPython embebido pesa ~37 MB que
   compensa el ahorro de no tener Python en runtime. El
   argumento se vuelve "simplificación de runtime" (sin pip,
   sin Python, sin libpq instalados) más que "ahorro de
   deploy size".

Dockerfiles de los boilerplates 5/6 NO simplificados —
mantienen su approach actual con `python:3.12-slim` + `fitz
run`. READMEs actualizados con los 3 blockers documentados +
plan concreto del Dockerfile para cuando cierren.

### Audit de la extensión VSCode

Drift detectado vs el lenguaje actual al revisar grammar
TextMate y LSP completion contra la lista canónica de
builtins del evaluator (`builtin_names()`):

**Faltaban en grammar TextMate** (15):
- `spawn` (Fase 9.w.3 — fire-and-forget de fns `@background`)
- 5 ops de Bits-extras: `popcount`, `leading_zeros`,
  `trailing_zeros`, `rotate_left`, `rotate_right`
- 9 Math: `abs`, `min`, `max`, `pow`, `sqrt`, `ceil`,
  `floor`, `round`, `clamp`

**Faltaban en LSP scope_level_completions** (5): los mismos
Bits-extras (los Math + spawn ya estaban).

Ambos fixeados. Extensión bumpeada a 0.9.3 y `.vsix` re-
construido. Próximo workflow_release del CI multi-platform
publicará binarios alineados.

### Docs

- **cap 21.12 de `docs/guide.md`** suma sub-bloque dedicado
  al flag con ejemplo combinado y nota sobre que la sintaxis
  del file es la nativa de pip.
- **READMEs de `boilerplates/api-postgres-python/` y
  `boilerplates/api-fullstack-postgres/`** actualizados con
  los 3 blockers del smoke real Docker (codegen Fase 8.7.1,
  GLIBC mismatch fix, beneficio realista de imagen).
- **`CHANGELOG.md`** v0.9.42 expandido con las 4 piezas.
- **`CLAUDE.md`** sección Fase 8.c actualizada.
- **`docs/roadmap.md`** Fase 8.c sección final actualizada.

### Deuda residual derivada

- **Codegen `from python import` en módulos transitivos
  (Fase 8.7.1)** — pasó de deuda menor genérica a blocker
  explícito de los boilerplates 5/6. Cerrarla destraba la
  simplificación de los Dockerfiles a `--bundle-pip-
  requirements` + binario standalone.
- **GLIBC mismatch fix** — el plan de simplificación tiene
  que pinear `python:3.14-slim-bookworm` (no `python:3.14-
  slim` que es trixie). Documentado.
- **Distroless requiere `tar` embebido en Rust** — el
  launcher de `--bundle-python` invoca `Command::new("tar")`
  para extraer el PBS. `gcr.io/distroless/cc-debian12` NO
  trae tar → forzados a `debian:bookworm-slim` como runtime.
  Mover a distroless requiere un crate de tar inline (sub-
  paso futuro de la deuda menor del launcher).

## [v0.9.41] — 2026-05-23 — Fase 8.c: `fitz build --bundle-pip` (paquetes pip embebidos)

Nuevo flag `--bundle-pip <paquete>` repetible para `fitz build`.
Empaqueta paquetes pip junto al CPython base de `--bundle-python`
(implica este flag automáticamente). El binario resultante embebe
CPython 3.14.5 + los paquetes pip pedidos, todo en un solo
archivo standalone. NO requiere `pip install` en el destino.

Continuación natural de Fase 8.b. Sub-paso separado en una sesión
con momentum del feature anterior. Destraba boilerplates 5/6
(api-postgres-python, api-fullstack-postgres) para pasar de
`FROM python:3.X-slim` a `FROM gcr.io/distroless/cc-debian12`
con un solo binario embebido (imagen ~150 MB → ~80-100 MB).

### Cambios

- **Nuevo flag CLI `--bundle-pip <PACKAGE>`** repetible en
  `Commands::Build`:
  ```bash
  fitz build \
    --bundle-pip sqlalchemy \
    --bundle-pip psycopg2-binary \
    --bundle-pip "redis==5.0.0" \
    mi_app.fitz
  ```
  Acepta version pin nativo de pip (`==`, `>=`, `<`, etc.).
  Implica `--bundle-python` automáticamente.

- **`launcher_template.rs` extendido** con 2 placeholders nuevos:
  - `PLACEHOLDER_PIP_DECL_BLOCK`: donde se inyecta
    `const PIP_PACKAGES: &[u8] = include_bytes!("...");` si hay
    `--bundle-pip`, o string vacío si no.
  - `PLACEHOLDER_PIP_EXTRACT_BLOCK`: donde se inyecta el bloque
    de extracción del tarball pip adentro de
    `python/Lib/site-packages/` (Windows) o
    `python/lib/python3.X/site-packages/` (Unix).
  - `gen_launcher_main_rs(...)` suma param
    `pip_packages_path: Option<&str>`. None = backward compat
    con 8.b (template bit-a-bit idéntico).

- **Pipeline de build extendido** (`main::build_file_with_bundle`):
  1. Build del real binary (igual que 8.b).
  2. Descarga PBS tarball (igual).
  3. **NUEVO** si `--bundle-pip` no vacío: extraer PBS al cache
     local del proyecto (`target/fitz-build/<bin>_pbs_extract/`),
     correr `<pbs>/python -m pip install --target <dir> <pkgs>`,
     empacar el resultado en `<bin>_pip_packages.tar.gz`.
  4. **NUEVO**: hash combinado (PBS bytes + pip bytes) para que
     dos proyectos con paquetes distintos no compartan TMP dir.
  5. Generar launcher con ambos paths (Some(pip_tarball)).
  6. Build del launcher (cargo).
  7. Copia al destino del usuario.

### Tests

- **2 unit tests nuevos** en `launcher_template::tests`:
  - `gen_launcher_main_rs_con_pip_packages_inyecta_bloques`
  - `gen_launcher_main_rs_pip_packages_escapa_windows_path`
- **2 E2E tests nuevos** en `bundle_python_e2e.rs`:
  - `bundle_pip_implica_bundle_python_y_aborta_sin_from_python_import`
  - `bundle_pip_repetible_acepta_varios_paquetes`
- **Total Fase 8.c**: 4 tests nuevos. Acumulado con 8.b: 29
  tests específicos del bundling.
- Smoke `GUIDE_EXAMPLES_COMPILE` sigue verde (sin regresión).

### Smoke manual end-to-end (Windows)

```
$ fitz build --bundle-pip requests examples/python-interop-8.c.fitz
→ compilando real binary…
→ asegurando PBS tarball (cpython 3.14.5 / x86_64-pc-windows-msvc)…
→ extrayendo PBS al cache local para correr pip (1 paquete(s))…
→ pip install --target (1 paquete(s))…
→ empacando pip_packages.tar.gz…
→ compilando launcher…
✓ binario standalone (CPython 3.14.5 + 1 pip pkg(s) embebidos):
  python-interop-8.c.exe (22.9 MB)

# Sin Python en PATH:
$ ./python-interop-8.c.exe
Módulo requests cargado desde el bundle pip:
requests
2.34.2
```

### Tamaños observados

| Bundle | Tamaño bin | Cold first run | Warm |
|--------|------------|----------------|------|
| `--bundle-python` (stdlib) | ~22 MB | ~3-5s | ~50-100ms |
| `+ --bundle-pip requests` | ~23 MB | ~5-7s | ~50-100ms |
| `+ --bundle-pip sqla+psycopg2` (estimado) | ~50 MB | ~8-12s | ~50-100ms |

### Ejemplo + docs

- `examples/python-interop-8.c.fitz` runnable con comentarios
  exhaustivos (cuándo usar, caveats, tamaños).
- **Cap 21.12 nuevo** "`fitz build --bundle-pip` — empaquetar
  paquetes pip" en `docs/guide.md`. Renumeración:
  21.12 (CRUD)→21.13, 21.12 (Limitaciones)→21.14 (fix de bug
  de renumeración previo en 8.b.7 donde había dos 21.12).
- **README footnote § actualizado** con el nuevo flag y los
  casos de uso reales.
- **READMEs boilerplates 5/6 actualizados** con plan concreto
  de simplificación a `FROM gcr.io/distroless/cc-debian12` +
  `--bundle-pip sqlalchemy psycopg2-binary`. Imagen ~150 MB →
  ~80-100 MB. Dockerfiles actuales mantenidos sin cambios
  (smoke real Docker como deuda — el primer user que pruebe
  confirma).

### Deudas residuales (NO bloquean uso real)

- **Smoke real Docker de boilerplates 5/6 con --bundle-pip**:
  validado solo en Windows con programa simple (`requests`).
  La combinación `--bundle-pip sqlalchemy + psycopg2-binary`
  adentro de un Dockerfile Linux multi-stage es deuda nueva.
- **Constraint Linux/macOS heredado**: builder requiere Python
  3.14.x (R.bug-pyo3-abi3-portable-link componente Linux/macOS
  pendiente). Cuando cierre, `--bundle-pip` es independiente
  del Python del builder en las 3 plataformas.
- **C extensions cross-platform**: `pip install` al build time
  baja wheels específicos del triple del builder. Buildear
  Linux desde Windows requiere `cross` o Docker (igual que
  todo cross-compile Rust).
- **Re-pip-install al cambiar paquetes**: hoy el pip install
  corre cada build si `<bin>_pip_packages` no existe. Cuando
  cambiás `--bundle-pip <pkgs>`, el cache stale se borra
  automático (rm -rf antes de instalar). Optimizable con hash
  de la lista de pkgs como cache key.
- **`--bundle-pip` con requirements.txt**: hoy hay que listar
  paquetes uno por uno. `--bundle-pip-requirements <file>`
  futuro para leer requirements.txt automático.

## [v0.9.40] — 2026-05-23 — Fase 8.b: `fitz build --bundle-python` (binario standalone con CPython embebido)

Nuevo flag `--bundle-python` para `fitz build`. Produce un binario
standalone con CPython 3.14.5 embebido (vía
[python-build-standalone](https://github.com/astral-sh/python-build-standalone)
de Astral). El binario resultante **NO requiere Python instalado
en el destino** — corre en cualquier máquina del triple soportado,
en frío. Es el único lenguaje moderno que ofrece esto activamente
mantenido (PyOxidizer hizo algo parecido pero está ralentizado
desde 2023).

### Cambios

- **Nuevo flag CLI `--bundle-python`** (`Commands::Build`):
  ```bash
  fitz build --bundle-python mi_app.fitz
  ./mi_app   # corre sin Python en el PATH
  ```
- **Nuevo módulo `src/pbs.rs`** — descarga + cache local del
  tarball PBS. Release pinned `20260510` con CPython `3.14.5`,
  sabor `install_only_stripped` (~70% más chico que
  `install_only`). Cache en `~/.fitz/cache/pbs/` (override con
  `FITZ_CACHE_DIR`, mismo patrón que `git_dep`). Subprocess
  `curl`, cero deps Rust nuevas.
- **Nuevo módulo `src/launcher_template.rs`** — template Rust del
  launcher Datasette-style con placeholders `__FITZ_REPLACE_*__`.
  El launcher (~200 KB Rust standalone, sin pyo3) embebe vía
  `include_bytes!` el tarball PBS y el "real binary". En primer
  run extrae a `$TMPDIR/fitz-py-<hash>/` (subprocess `tar -xzf`,
  bsdtar nativo en Win11/macOS/Linux moderno), setea
  `PYTHONHOME` + `LD_LIBRARY_PATH`/`DYLD_FALLBACK_LIBRARY_PATH`/
  `PATH` según OS, y `exec` (Unix) / `spawn+wait` (Windows) del
  real binary. Hash FNV-1a 16-char para nomenclatura
  determinística del cache TMP.
- **Nueva función `main::build_file_with_bundle()`** — pipeline
  paralelo a `build_file()` cuando hay `--bundle-python`:
  validaciones tempranas (host triple soportado, programa usa
  `from python import`), build del real binary (reusa
  `codegen::generate_project` sin cambios), descarga PBS,
  generación + build del launcher en
  `target/fitz-build/<bin>_launcher/`, copia del launcher al
  destino.
- **Modelo arquitectónico**: launcher pattern (Datasette Desktop
  desde 2021). Descartamos:
  - **Extract-on-first-run naive**: no funciona, el OS resuelve
    libpython ANTES de `main()` (Linux: `DT_NEEDED` vía ld.so;
    macOS: `LC_LOAD_DYLIB` vía dyld; Windows: import table).
  - **Linking estático con PBS "full"** (PyOxidizer-style):
    "multi-month rabbit hole", PyOxidizer es el único proof y
    está ralentizado.
  - **Delay-load/dlopen manual**: sin soporte documentado en
    PyO3, brittle entre versiones.

### Tests

- **10 unit tests** en `src/pbs.rs::tests` (constantes pinned,
  URL builder, host triple detection, cache path, error display).
- **11 unit tests** en `src/launcher_template.rs::tests` (template
  sustitución, escape Windows paths, hash FNV-1a determinístico).
- **2 E2E tests** en `tests/launcher_template_e2e.rs` (template
  procesado compila como Rust válido, con paths Windows + paths
  con espacios).
- **2 E2E tests** en `tests/bundle_python_e2e.rs` (validation
  temprana: aborta con mensaje claro sin `from python import`,
  aborta antes de bundling si hay error de parse).
- **Total nuevo: 25 tests**. El smoke
  `GUIDE_EXAMPLES_COMPILE` sigue verde (sin regresión del
  codegen normal).

### Smoke manual validado

Sobre Windows 11 SSD con programa `from python import math`:

- Build: `→ compilando real binary → asegurando PBS tarball →
  compilando launcher → ✓ binario standalone (21.8 MB)`
- Run sin Python en PATH: output bit-a-bit con el real binary
  (`math.pi = 3.141592653589793`, `math.sqrt(81.0) = Ok(9.0)`).
- Cold first run: ~5.3s (extract tar + boot CPython).
- Warm subsequent runs: ~50-100ms (cache TMP hit).

### Tamaños observados

| Triple | Binario final | Extract dir TMP |
|--------|---------------|-----------------|
| `x86_64-pc-windows-msvc` | ~22 MB | ~61 MB |
| `x86_64-unknown-linux-gnu` | ~35 MB | ~75 MB |
| `aarch64-apple-darwin` | ~24 MB | ~62 MB |

### Ejemplo + docs

- `examples/python-interop-8.b.fitz` — programa runnable que
  demuestra el flag con comentarios detallados sobre cuándo
  usarlo y cuándo no, tamaños y timing observados.
- **Cap 21.11 nuevo** "`fitz build --bundle-python` — binario
  standalone" en `docs/guide.md` (renumeración 21.11→21.12,
  21.12→21.13). Incluye cuándo usar, tamaños, timing,
  arquitectura interna del launcher, constraint del builder, y
  pendientes.
- **README footnote § actualizado** con emphasis del feature
  como diferencial único en el cuadro de comparación
  Python/TS/Go/Fitz.
- **Cierre parcial** de la deuda
  `R.bug-pyo3-abi3-portable-link` (componente bundling): el
  modelo launcher pattern bypasea el bug en Windows
  completamente (real binary linkea contra `python3.dll`
  stable ABI, no contra `python314.dll` específica). En
  Linux/macOS el constraint sigue (builder = bundle version).

### Deudas residuales (NO bloquean uso real)

- **Bundling de pip packages** (sub-paso futuro). Hoy
  `--bundle-python` embebe CPython base + stdlib. Programas que
  usan SQLAlchemy/numpy/etc. necesitan `pip install` adicional
  en el destino. Una extensión `--bundle-pip <pkg>` podría
  empaquetar paquetes pip junto al CPython base.
- **Boilerplates 5/6 simplificación**: con `--bundle-pip` los
  Dockerfiles podrían `FROM scratch` o `FROM distroless` en
  lugar de `FROM python:3.X-slim`. Ahorro estimado: imagen
  ~150 MB → ~40 MB.
- **Linux/macOS smoke end-to-end**: hoy validado solo en
  Windows. Los primeros usuarios en Linux/macOS confirman que
  el pipeline funciona ahí también.
- **Bundle más chico vía stdlib stripping**: ~30% reducción
  posible eliminando módulos no usados (similar al
  `py-spy --strip` de PyOxidizer).
- **Hash SHA256** en lugar de FNV-1a para defender contra
  cambios silenciosos del PBS upstream (FNV-1a es suficiente
  hoy porque el release está pinned).

### Cómo retomar la deuda residual

Para `--bundle-pip`: agregar campo `bundle_pip: Vec<String>`
al `Commands::Build`; cuando hay `--bundle-pip <pkg>`, después
de extraer el tarball ejecutar `<extract-dir>/python/python.exe
-m pip install --target <extract-dir>/python/Lib/site-packages
<pkg>` adentro del launcher (en primer run, mismo flujo de
extract). Trade-off: primera ejecución del launcher con pip
puede tardar varios segundos. Diseño detallado pendiente.



Nuevo sub-comando `fitz py-stubs <archivo.pyi> [--out <archivo.fitz>]`
paralelo a `fitz py-types` (que ya hacía SQLAlchemy). Parsea stubs
Python PEP 484/561 y emite los `type` Fitz equivalentes para cada
`class` top-level. Cierra parcialmente la deuda `8-pyi-stubs`.

### Cambios

- **Nuevo módulo `src/pyi_stub.rs`** — parser .pyi ad-hoc (no
  parser Python completo). Tokenizer line-based, recursive descent
  sobre subset PEP 484:
  - Top-level `def name(args) -> ret: ...` (parsed pero no
    emitido al output — deuda menor).
  - Top-level `class Name: ...` con fields anotados.
  - Top-level `name: type = default` (parsed pero no emitido).
  - Type exprs: primitivos, `list[T]`, `dict[K, V]`,
    `Optional[T]`, `T | None` (PEP 604), forward refs string
    `"Foo"`, dotted names `module.Name` (toma el último segmento).
- **Mapper StubType → Fitz Type** (`stub_type_to_fitz_type`):
  - `int/float/str/bool/None/bytes` → primitivos Fitz.
  - `list[T]/dict[K,V]/Optional[T]` → `List<T>/Map<K,V>/T?`.
  - `T | None` (Union[T, None]) → `T?` (caso típico nullable).
  - Union no-null → `Any` (Fitz no tiene unions arbitrarias).
  - Nominal desconocido → registrado en TypeEnv.
- **CLI `Commands::PyStubs { source, out }`** — disponible **sin
  feature `python`** (el parser .pyi no usa PyO3). Sigue el mismo
  patrón del `py-types`: lee el .pyi, parsea, emite `.fitz` por
  stdout o archivo.
- **Renderer `render_stub_items_as_fitz`** (`src/main.rs`) — sólo
  emite `class` → `type` (def/var top-level se ignoran porque el
  evaluator runtime los maneja via `PyAny` opaco).

### Tests

- 21 unit tests en `pyi_stub::tests` (parser + mapper exhaustivo).
- 5 cli_e2e tests del comando `fitz py-stubs` (class básica, tipos
  compuestos + Optional, output a archivo, archivo inexistente,
  skip fns/vars).

### Ejemplo + docs

- `examples/guide/21b-pyi-stubs.fitz` con dos types generados +
  programa que los usa (paralelo al ejemplo de cap 21.7).
- Cap 21.8b nuevo en `docs/guide.md` con workflow, sub-set
  cubierto, restricciones, y nota sobre integración automática
  como deuda residual.
- Smoke `GUIDE_EXAMPLES_COMPILE` suma `21b-pyi-stubs.fitz`.

### Deuda residual (documentada)

- **Integración automática con el checker** — cuando `from python
  import foo` y `<base>/foo.pyi` existe, hidratar el TypeEnv
  directamente. Requiere `Type::PyModule` + refactor signature
  de `check_program(base_dir: Option<&Path>)`. Sin presión real
  hoy — el flow `fitz py-stubs --out` cubre el 80% del valor.
- **`def` top-level del stub al `.fitz`** — hoy se ignoran porque
  el runtime las trata como PyAny. Materializar las signatures
  como Fitz fns que tipan los calls Python al .py real es
  refactor mayor.
- **Métodos de class** — solo fields hoy. Materializar métodos
  custom requiere registro `type Foo { ... } fn Foo.method(...)`
  + decisiones sobre `self`.

## [v0.9.38] — 2026-05-23 — 9.w.2-wsconn-bidir: `WsConn<In, Out>` con tipos asimétricos

Cierra la deuda residual del MVP 9.w.2 (WebSockets) sobre tipos
bidireccionales separados. Habilita canales asimétricos donde el
cliente envía un tipo (e.g. comandos `Str`) y el server emite otro
(e.g. eventos `ChatMsg` estructurados). Backward-compat con todo el
código pre-bidir.

### Cambios

- **AST + type system**: `Type::WsConn(Box<Type>)` →
  `Type::WsConn { recv: Box<Type>, send: Box<Type> }`. Cuando el
  usuario declara `WsConn<T>` (aridad 1), `recv == send == T`
  (simétrico, identical to pre-bidir). Cuando declara
  `WsConn<In, Out>` (aridad 2), `recv = In`, `send = Out` difieren.
  `Type::WsConn::display` emite `WsConn<T>` para simétricos,
  `WsConn<In, Out>` para asimétricos.
- **Checker**: `infer_wsconn_method` recibe `recv_ty` y `send_ty`
  separados. `recv() → Result<RECV>`, `send/broadcast(msg: SEND) →
  Result<Null>`. Mensajes de error con tipo correcto en cada
  dirección.
- **Runtime intérprete**: `WsConnHandle` gana `send_type` (paralelo
  a `msg_type` que ahora documenta explícitamente "recv type").
  `ws_conn_send`/`ws_conn_broadcast` usan `send_type` para
  decidir modo binary vs text JSON. `ws_conn_recv` sigue con
  `msg_type` (recv). `build_ws_conn` toma `send_type` como
  parámetro adicional.
- **RouteSpec**: nuevo campo `ws_send_type: Option<TypeExpr>`. El
  evaluator lo popula al registrar el handler `@ws`.
- **Codegen `fitz build`**: preludio refactored —
  `struct __FitzWsConn<RECV: __FitzWsMessage, SEND: __FitzWsMessage>`
  con dos type params; `recv` usa `RECV`, `send/broadcast` usan
  `SEND`. `__fitz_ws_setup<RECV, SEND>` también con dos params. El
  wrapper del handler emite el setup con ambos tipos resueltos.
  Monomorfismo garantiza que `WsConn<T>` simétrico produzca un
  binario idéntico al pre-bidir.
- **AsyncAPI 3.0**: cuando `recv != send`, el schema emite **dos
  messages distintos** — `msg_in` (referenciado por la operation
  `receive`) y `msg_out` (referenciado por la operation `send`).
  Cuando son iguales, sigue emitiendo el único `msg` (sin romper
  consumers existentes del schema simétrico).
- **LSP**: el `detail` del completion sobre `WsConn<In, Out>` ahora
  muestra `recv() -> Result<In>` y `send(msg: Out)` con tipos
  correctos.

### Restricción binary mixto

Si `recv` o `send` es `Bytes` pero el otro no (`WsConn<Bytes, Str>`),
el codegen rechaza con error explícito. El wrapper del handler
detecta `recv_is_bytes != send_is_bytes` y aborta antes de emitir
el setup. Soporte de canales binary-mixed queda como deuda
residual menor.

### Tests

- 3 unit tests del checker (aridad 2 resuelve recv/send distintos,
  display asimétrico, aridad >2 es error).
- 1 E2E intérprete (`WsConn<Str, ChatMsg>`: cliente envía Str,
  server emite ChatMsg JSON-marshalled).
- 3 unit tests AsyncAPI (asimétrico emite dos messages, operations
  apuntan a messages distintos, simétrico sigue con `msg` único).
- 1 unit test LSP (detail correcto para `WsConn<Str, ChatMsg>`).

Total: +8 unit tests. 2215 → ~2223 verdes.

### Ejemplo + docs

- `examples/guide/29c-ws-bidir.fitz`: canal `WsConn<Str, ChatMsg>`
  con welcome message + loop recv/send.
- Cap 29 de la guía: sección "Canales asimétricos con `WsConn<In, Out>`"
  con explicación del modelo, AsyncAPI asimétrico, restricción
  binary mixto, paridad bit-a-bit.
- Smoke `GUIDE_EXAMPLES_COMPILE` suma `29c-ws-bidir.fitz`.

## [v0.9.36] — 2026-05-23 — Bloque C: imagen `:latest-python` + auth WS desde browsers

Segundo bloque de quick wins del día. Dos features autocontenidas
con valor inmediato para usuarios browser y CI/distribución.

### Quick win #1 — `ghcr.io/<owner>/fitz:latest-python` (fitz-python-image)

Nuevo job `docker-image-python` en `release.yml` que builda y
publica una imagen Docker dedicada con `--features python` activo,
lista para usar como base de boilerplates 5/6:

```dockerfile
# Antes (boilerplate 5/6) — ~5-8 min de build inicial:
FROM python:3.12-slim AS builder
RUN curl ... rustup ... && \
    cargo install --git https://github.com/Thegreekman76/fitz --features python ...

# Después — pull en segundos:
FROM ghcr.io/thegreekman76/fitz:latest-python AS builder
```

Single-arch (`linux/amd64`) inicial. ARM64 con `--features python`
queda como deuda explícita hasta que `R.bug-pyo3-abi3-portable-link`
se cierre (cross-compile PyO3 abi3 requiere setup adicional).

Tags publicados:
- `ghcr.io/<owner>/fitz:v0.9.36-python`
- `ghcr.io/<owner>/fitz:latest-python`

Patrón del Dockerfile sigue el del boilerplate `api-postgres-python`
(builder y runtime con `python:3.12-slim`, builder agrega Rust con
rustup, runtime descarta Rust). Los Dockerfiles de boilerplates
5/6 actualizados con nota sobre la alternativa rápida (sin
migrarlos todavía — el cambio queda como opt-in del usuario).

### Quick win #2 — Auth WS desde browsers (9.w.2-ws-auth-browser)

Workaround estándar para autenticar WebSockets desde código de
browser. `new WebSocket(url)` NO permite setear headers HTTP
arbitrarios; el segundo argumento sí acepta una lista de
subprotocols. Convención (Socket.IO, Phoenix, varios proyectos
Node): pasar el token via subprotocol `bearer.<token>`.

Desde v0.9.36, el runtime y el codegen Fitz extraen el token del
header `Sec-WebSocket-Protocol` y lo inyectan como
`authorization: Bearer <token>` al map de headers que ve el
`@auth_provider`. Sin cambios del lado user — el mismo provider
funciona para HTTP y WS browser.

Implementación:
- Nuevo helper público `extract_ws_bearer_subprotocol` en
  `src/http.rs` (runtime) + helper paralelo
  `__fitz_ws_extract_bearer_subprotocol` en preludio WS de codegen
  (`src/codegen.rs`).
- `build_ws_method_router` (runtime) y `gen_ws_handler_wrapper`
  (codegen): antes de invocar al `@auth_provider`, inyectan
  `authorization: Bearer <token>` al map si no hay header
  `Authorization` previo.
- Echo del subprotocol seleccionado en el handshake response via
  `ws.protocols([proto])` (RFC 6455 §4.1 — sin echo, el browser
  rechaza el upgrade).
- Compatibilidad: si el cliente envía AMBOS Authorization header
  Y subprotocol bearer, el header gana (preserva el caso wscat/
  curl/clientes no-browser).

Tests:
- 6 unit tests del helper (single proto, CSV con varios, ausente,
  sin match, token vacío, JWT con dots internos).
- 2 E2E intérprete (acepta token válido + echo del subprotocol;
  rechaza con 401 si el token es inválido).
- 2 unit codegen (output emite el helper + la inyección).
- 1 E2E codegen (binario nativo + cliente tokio-tungstenite con
  subprotocol — handshake + auth + echo end-to-end).

Cap 28 (Auth nativa) actualizado con la sección "Auth WS desde
browsers" en el cap 29: ejemplo cliente JavaScript + server Fitz
+ explicación del flujo + compatibilidad con header.

### Acumulado al cierre

+11 unit (6 helper + 2 codegen + 6 runtime + 1 docker workflow) +
2 E2E intérprete + 1 E2E codegen. Clippy `-D warnings` limpio.
Smoke `GUIDE_EXAMPLES_COMPILE` verde. Sin breaking changes — el
header `Authorization` original sigue funcionando idéntico para
clientes que pueden setearlo.

Boilerplates revisados — 6/6 verdes con `fitz check`. Boilerplate
api-websocket podría aprovechar el subprotocol en su frontend HTML
(deuda menor, queda opcional).

VSCode review: NO requiere update. Ninguno de los quick wins
toca AST/grammar/LSP. El helper de extracción es runtime/codegen
puro; la API del `@auth_provider` no cambia para el user.

Próximo norte: vaciar resto del backlog (`9.w.2-wsconn-bidir`,
`8-pyi-stubs`, `8-bundling-cpython`) o saltar a Fase 10 (Stack DB
nativo).

## [v0.9.35] — 2026-05-23 — Bloque triple de quick wins: split await + AsyncAPI UI + inferencia params

Bloque coordinado de tres quick wins del backlog post-v0.9.34, en
una sola tanda. Sin breaking changes — sólo features nuevas y
mejor inferencia.

### Quick win #1 — `let fut = py_call()?; fut.await` (8.7-await-binding-split)

`fitz build` ahora compila el patrón "split" del await Python:
binding intermedio del coroutine y `.await` después.

Antes solo aceptaba `<py_call>?.await` inline (`Await(Try(Call PyAny))`
en el AST). Con un binding intermedio, el inner del await era un
`Expr::Ident` con tipo `PyAny`, y el codegen emitía `.await` directo
sobre `__FitzPyObject` — Rust fallaba con "is not a future".

Fix: nuevo helper Rust `__fitz_py_await_obj(coro: &__FitzPyObject)`
emitido al preludio cuando `uses_async + uses_python`. Cuando
`Expr::Await(inner)` tiene `inner_ty == Type::PyAny` y NO matchea
el patrón inline, el codegen despacha al helper dedicado. El
intérprete ya lo soportaba (envuelve el coroutine en `Value::Future`
en `py_interop::call`) — ahora el codegen tiene paridad.

3 unit tests del codegen. Cap 21 de la guía actualizado con el
nuevo patrón documentado.

### Quick win #2 — UI HTML para AsyncAPI 3.0 (9.w.2-asyncapi-ui)

Cuando hay handlers `@ws`, además de `/asyncapi.json`, el server
auto-registra `/asyncapi` con una UI HTML embebida que renderea
channels + operations + messages + securitySchemes. Mismo patrón
que `/docs` para OpenAPI/Scalar.

Bundle: `@asyncapi/react-component@2.6.5` vía CDN (unpkg). Carga
liviana (~liviana después de cache del navegador).

- `src/templates/asyncapi.html`: HTML wrapper que hace fetch del
  schema y lo pasa a `AsyncApiStandalone.render(...)`.
- `src/asyncapi.rs`: `pub const ASYNCAPI_HTML` con `include_str!`.
- Runtime (`build_router_with_asyncapi` en `src/http.rs`):
  auto-registra `/asyncapi` cuando hay schema, cede si el user
  declaró `@get("/asyncapi")` propio.
- Codegen (`src/codegen.rs`): emite `static __FITZ_ASYNCAPI_HTML`
  + `async fn __serve_asyncapi()` + `.route("/asyncapi", ...)` en
  el router builder. Mismo cede-si-user-gana.
- Opt-out global: `@server(docs=false)` apaga AMBAS (OpenAPI +
  AsyncAPI UI/JSON).
- `eprintln!` del banner del runtime suma "GET /asyncapi (UI AsyncAPI)".

3 unit tests del runtime (HTML correcto, 404 sin schema, JSON sigue
funcionando) + 3 unit tests del codegen (handler/route emitidos,
cede sobre user, no se emite sin @ws). Cap 29 de la guía
actualizado.

### Quick win #3 — Inferencia de params/return sin anotar (cierre 5b.1)

La deuda 5b.1 (inferencia de tipos de params en fns sin anotación)
ya tenía implementación parcial (`fill_inferred_param_types` en
codegen) pero `type_to_type_expr` saltaba el caso `Nominal` con
un comentario "Skip Nominal por ahora — necesitamos el nombre real".

Fix: `type_to_type_expr` ahora recibe el `TypeEnv` y resuelve
`Nominal(id)` consultando `env.info(*id).name` para obtener el
nombre canónico. También suma `Type::Bytes`. Cubre:
Int/Float/Str/Bool/Null/Bytes/Nullable/List/Map/Result/Nominal.

Casos confirmados que ahora compilan sin anotaciones:
- `fn double(n) { return n * 2 }` (Int inferido).
- `fn greet(u) { return "hola {u.name}" }` con `User` (Nominal inferido).
- `fn shout(s) { return s.upper() }` con `"hola"` (Str inferido).
- Funciones recursivas con anotación de return + param sin anotar.
- Múltiples call sites del mismo fn.

5 unit tests del codegen (4 path #2 `resolve_param_type` + 1 path
#1 `fill_inferred_param_types` validando que Nominal se resuelve
a `TypeExpr::Named("User")`). Cap 11, cap 14 y cap 18 de la guía
actualizados — anotaciones siguen recomendadas pero NO obligatorias.

### Acumulado al cierre

+11 unit tests (3 await + 6 asyncapi + 5 infer). Clippy
`-D warnings` limpio. Smoke `GUIDE_EXAMPLES_COMPILE` verde. Sin
breaking changes — programas existentes compilan idéntico.

Boilerplates revisados — ninguno toca los paths cambiados (todos
usan anotaciones explícitas + handlers @ws con marshaling text +
sin call patterns split de Python). Sin necesidad de update.

Próximo norte: Fase 10 (Stack DB nativo) o seguir vaciando el
backlog (`9.w.2-wsconn-bidir`, `fitz-python-image`, `8-pyi-stubs`,
`8-bundling-cpython`).

## [v0.9.34] — 2026-05-23 — Quick win: 9.w.2-binary-frames — `WsConn<Bytes>` end-to-end

Cierra la deuda más visible del MVP de WebSockets (9.w.2): el
soporte para frames binarios raw vía `WsConn<Bytes>`. Hoy el wire
de un `WsConn` puede ser **text JSON-marshalled** (T = Str /
nominal / etc.) o **binary opaco** (T = Bytes); el modo lo elige
el T declarado y el lenguaje rechaza el mismatch con mensaje
claro. Cero deps nuevas, paridad bit-a-bit `fitz run` ↔ `fitz
build`.

**Lo que entra**:

- **Checker** — `WsConn<Bytes>` aceptado en `@ws` handlers como
  cualquier otro T concreto; `recv()` tipa `Result<Bytes>`,
  `send/broadcast` exigen arg `Bytes`. 4 unit tests blindean el
  contrato.
- **Runtime intérprete** — `Value::WsOutMessage::Binary(Vec<u8>)`,
  `IncomingFrame::{Text, Binary}` enum reemplaza el filtro
  text-only del read stream, `WsBroadcasterTrait::broadcast_binary`
  paralelo a `broadcast_text`. El evaluator discrimina por
  `ws_msg_is_bytes(msg_type)` en `recv/send/broadcast`. 3 E2E con
  tokio-tungstenite: echo round-trip, broadcast multi-cliente,
  mismatch (cliente manda text con T=Bytes → Err).
- **Runtime HTTP** (`src/http.rs`) — `WsReadStreamImpl::next_frame`
  expone Binary en lugar de rechazarlo; writer task gana rama
  `Binary(bs)` → `Message::Binary(bs.into())`.
- **AsyncAPI 3.0** — payload schema cuando T=Bytes emite
  `{"type":"string","format":"binary"}` + `contentType:
  application/octet-stream`. 3 tests del schema.
- **Codegen `fitz build`** — struct dedicado `__FitzWsConnBytes`
  (no genérico — specialization sobre `Vec<u8>` chocaría con el
  blanket impl del trait interno que lo trataría como
  `List<Int>` JSON); helper `__fitz_ws_setup_bytes`; writer
  task del preludio drena Binary también; ramaje en
  `gen_ws_handler_wrapper`. 1 E2E del codegen con cliente
  binary verificado bit-a-bit.
- **Guía cap 29** — sub-sección nueva "Frames binarios con
  `WsConn<Bytes>`" con ejemplo runnable + AsyncAPI schema
  emitido + trade-off documentado (text XOR binary por
  endpoint). Ejemplo `examples/guide/29b-ws-binary.fitz`
  agregado al smoke `GUIDE_EXAMPLES_COMPILE`.

**Decisión de diseño**: opción A — un endpoint es text-only XOR
binary-only, según el T declarado. Más simple que un canal mixto
y alineado con el modelo "T determina el frame type" que ya
tiene el lenguaje. Si aparece presión por endpoints mixtos,
queda como sub-paso futuro.

**Acumulado al cierre**: +10 unit (4 checker + 3 AsyncAPI + 3
E2E intérprete = todos via `cargo test --lib`) + 1 E2E codegen
(`cargo test --test compile_e2e`). Clippy `-D warnings` limpio.
Sin breaking changes — handlers `WsConn<Str>` / `WsConn<Nominal>`
existentes siguen funcionando idéntico.

Próximo norte (mismo bloque de quick wins post-boilerplates):
investigación + cierre de `R.bug-pyo3-abi3-portable-link` —
binarios Linux con `--features python` corren en cualquier
Python 3.10+ del runtime sin rebuild.

## [v0.9.32] — 2026-05-22 — Patch: mini-tanda Cleanup-Residual+ — limpiezas mecánicas + pyo3-abi3 cerrado + multi-arch Docker

Bloque grande de cleanup post-Cleanup-Residual. 4 sub-tandas
coordinadas: auditoría de deudas (4 más marcadas CERRADAS de
facto), cleanups mecánicos (clippy + fmt), fix `pyo3-abi3-autoinit`
con CI multi-Python, multi-arch Docker image.

### Sub-tanda A — Auditoría (4 deudas RESIDUALES marcadas CERRADAS)

- **5b.5-imports-transitivos** ✓ — ya cerrada por F15 (Fase 9.0);
  test `f15_module_loader_acepta_imports_transitivos_en_modulo`
  vive en codegen tests.
- **F13-listas-heterogeneas-compiladas** ✓ — implementado vía
  `__FitzValue` tagged runtime. `uses_fitz_value` se setea en
  literales heterogéneos.
- **8.7-fromfitzpy-symmetric** ✓ — subsumida por mini-fase 8.7.bis
  (v0.9.28). Dirección Python→Fitz está en helpers
  `__fitz_py_to_instance_*` / `__fitz_py_to_list_*` per-tipo —
  equivalente funcional al trait simétrico propuesto.
- **5b.5-let-expr-top-mod** ✓ — cerrada por F14. `gen_module_top_let`
  tiene 3 caminos: const literal → `pub static`/`pub const`,
  const-eval → `pub const`, runtime → accessor `pub fn X() -> T`.

### Sub-tanda B — Mecánicos (alto valor, bajo riesgo)

- **clippy-all-targets** ✓ — fix de 9 errores que bloqueaban
  `cargo clippy --all-targets -- -D warnings`:
  - 2× `useless_format!` en tests de `http.rs`
  - 1× `unused_import` (`futures_util::SinkExt` en test WS)
  - 2× `cloned_ref_to_slice_refs` en `hash_password` tests
  - 2× `MutexGuard held across await` (intencional — SERIAL Mutex
    para serializar tests E2E; ahora marcado `#[allow]` explícito)
  - 1× `non_snake_case` (`fmt_G_uppercase_compila` → `fmt_g_...`)
  - 1× `unnecessary_get_then_check` (`get().is_none()` →
    `!contains_key()`)
- **fmt-cleanup-codebase** ✓ — `cargo fmt --all` aplicado. 28
  archivos modificados (2246 diffs canónicos de rustfmt).
  Pendiente: activar `cargo fmt --check` en CI — sub-paso futuro
  separado para que el reactivar quede en commit limpio.

### Sub-tanda D.1 — multi-arch-docker ✓

`release.yml` job `docker-image` ahora emite imagen multi-arch
`linux/amd64,linux/arm64`. Cambios:

- Descarga ambos artefactos `binaries-linux-x64` + `binaries-linux-arm64`.
- Dockerfile usa `ARG TARGETARCH` para copiar el binario
  pre-compilado correcto (`fitz-amd64` o `fitz-arm64`).
- `docker/setup-buildx-action@v3` habilita el multi-arch build.
- `docker/build-push-action@v6` con `platforms: linux/amd64,linux/arm64`
  push manifest que Docker resuelve por host.

Habilita Mac M-series (arm64), Raspberry Pi 4+ (arm64), AWS
Graviton (arm64) sin emulación QEMU. Imagen GHCR pasa de single-arch
amd64 a multi-arch transparente.

### Sub-tanda D.3 + E — R.bug-pyo3-abi3-autoinit CERRADO + boilerplates simplificados

Fix de la deuda más vieja del backlog Python interop. Antes:
`Cargo.toml` tenía `features = ["abi3-py310", "auto-initialize"]`
que eran INCOMPATIBLES — auto-initialize ganaba, el binario
linkeaba contra libpython específica del builder, perdíamos la
portabilidad abi3.

Fix:
- `Cargo.toml`: removido `auto-initialize`. Solo `abi3-py310` activo
  → binario corre contra cualquier Python 3.10+.
- `src/py_interop.rs`: nuevo helper `ensure_python_initialized()`
  que llama `Python::initialize()` adentro de un `std::sync::Once`.
  Lazy init en el primer `import_module`. Idempotente, sin overhead
  perceptible.
- `.github/workflows/ci.yml`: job `python` ahora corre con matriz
  `python-version: [3.10, 3.11, 3.12, 3.13]` para validar el
  contrato cross-Python.
- `boilerplates/api-postgres-python/Dockerfile` simplificado:
  builder pasa de `python:3.12-slim` + rustup manual a `rust:slim`
  con Rust pre-instalado. Ahorro de ~2-3 min por docker build.
  Runtime `python:3.12-slim` queda intacto.
- `boilerplates/api-fullstack-postgres/Dockerfile` mismo refactor.

46 py_interop tests verdes localmente con feature python. El binario
default sin feature python no toca su path de compilación.

### Sub-tanda C + D.2 — DIFERIDAS

- **9.w.2-binary-frames** (WS Bytes payload): scope ~1-2h con
  refactor del trait `WsReadStreamTrait` + nuevo
  `WsOutMessage::Binary` + dispatch en evaluator/codegen. Sin
  presión real, defer a sesión dedicada.
- **fitz-python-image** (`ghcr.io/.../fitz:latest-python`): requiere
  compilar `--features python` adentro del Dockerfile en buildx
  multi-arch — ~25 min de CI compute por release. El workaround
  actual del boilerplate (cargo install --git) toma ~6-9 min y
  solo corre por boilerplate, no por release. Trade-off no
  justifica.

### Validación

- **2178 unit tests** verdes localmente.
- **46 py_interop tests** verdes localmente con `--features python`
  (Python 3.14 en local).
- **clippy `--all-targets -- -D warnings`** verde.
- **cargo fmt --all -- --check** verde.

CI multi-Python matrix correrá en la próxima push a `main` —
valida 3.10/3.11/3.12/3.13 automáticamente. Si alguna versión falla,
abrimos deuda específica.

---

## [v0.9.31] — 2026-05-22 — Patch: mini-tanda Cleanup-Residual — 2 deudas FUNCIONALES cerradas

Cierre de 2 deudas medias documentadas en
`docs/deudas_lenguaje.md` después del cierre del plan
post-boilerplates. Una tercera (R.bug-pyo3-abi3-autoinit) quedó
diferida con plan claro porque su validación necesita Docker
multi-Python que no tengo en local — el workaround del
boilerplate funciona correctamente y no urge.

**R.bug-13i-stack-overflow-debug** — CERRADO

`13i-campos-privados.fitz` desbordaba el stack al compilar con
`fitz build` debug-mode en Windows (1 MB stack default). Fix:
`.cargo/config.toml` con linker flag `/STACK:8388608` bajo
`[target.x86_64-pc-windows-msvc]`. El main thread del binario
`fitz` ahora tiene 8 MB de stack en Windows (paridad con Unix
default). Smoke `GUIDE_EXAMPLES_COMPILE` verde con 13i incluido.
Clippy `-D warnings` verde.

**R.bug-result-status** — CERRADO

Handler HTTP con return type `Result<T>` + `return <status> { ... }`
adentro serializaba con wrapper `{"Ok":{...}}` en lugar de
desempacar el inner. Fix en `src/codegen.rs::gen_return`:

```rust
// Antes (en response_mode):
return __FitzResponse {
    status: 200,
    body: <Result<Item, String> as __ToFitzJson>::__to_fitz_json(&(Ok(found.clone()))),
    // ↑ serializa con wrapper {"Ok":{...}}
};

// Ahora (response_mode con Expr::Ok detectado):
return __FitzResponse {
    status: 200,
    body: <ItemData as __ToFitzJson>::__to_fitz_json(&(found.clone())),
    // ↑ desempaca el inner, serializa Item directo
};
```

Semántica paralela al runtime:
- `return Ok(v)` → 200 + body = v serializado.
- `return Err(e)` → 500 + body = `{"error": e}` serializado.
- `return <status> { ... }` sin cambios.

2 E2E verdes en `compile_e2e::r_bug_result_status_handler_*`
(unwrap Ok + path 404). Boilerplate `api-simple::get_item`
simplificado a `Result<Item>` con `return Ok(it)` semánticamente
prolijo (era workaround a `Item` directo).

**R.bug-pyo3-abi3-autoinit** — DIFERIDO con plan claro

Cargo.toml de Fitz tiene `pyo3 = { features = ["abi3-py310",
"auto-initialize"] }`, incompatibles entre sí: auto-initialize
gana y el binario linkea contra libpython específica del builder,
perdiendo la promesa "binario portable" de abi3. Workaround actual:
match builder/runtime Python en los Dockerfiles de boilerplates
5/6, funciona OK pero agrega ~30s al build (apt-get build-essential
+ rustup en lugar de `rust:slim`).

Fix planificado (no ejecutado en esta mini-tanda):
1. Quitar `auto-initialize` del Cargo.toml.
2. Emitir `pyo3::prepare_freethreaded_python()` en el preludio
   del codegen y al boot del intérprete cuando `uses_python = true`.
3. Validar cross-Python en Docker (build 3.13 + run 3.10/3.11/
   3.12/3.14). El paso 3 requiere Docker runner con múltiples
   Pythons que no tengo en local — bloqueante.

Próxima acción: cuando aterrice un sub-paso "CI multi-Python"
con GitHub Actions matrix, cerrar este fix ahí.

Total al cierre: **2178 unit + 277 compile_e2e + 3 openapi**.
Smoke `GUIDE_EXAMPLES_COMPILE` verde. Clippy `-D warnings` verde.

---

## [v0.9.30] — 2026-05-22 — Feature: mini-fase loader-absoluto — imports nested cross-folder

Cuarto y último paso del plan post-boilerplates. Cierra deuda
**R.bug-loader-relative-only** (descubierta 2026-05-22 al armar
el 6to boilerplate, documentada en `docs/deudas_lenguaje.md`).

El loader de módulos ahora resuelve imports en DOS estrategias
encadenadas:

1. **Relativo al importer** (comportamiento previo). Si el archivo
   buscado existe en `<importer_dir>/<segments>`, se usa.
2. **Relativo al import_root** (nuevo). Si el archivo NO existe
   relativo al importer, se prueba relativo al "import root" =
   parent del entry file (estable durante toda la vida del loader).

Caso canónico que ahora funciona — proyecto con módulos en
subcarpetas hermanas:

```
src/
├── main.fitz          → from data.users import create
├── types/
│   └── user.fitz      → type User { ... }
└── data/
    └── users.fitz     → from types.user import User
                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                        // Antes: buscaba `src/data/types/user.fitz` y fallaba.
                        // Ahora: relativo falla → fallback a `src/types/user.fitz`. ✓
```

Backward compat preservada: el patrón `import bar` desde un módulo
nested que resuelve a un sibling (`sub/bar.fitz`) sigue ganando vía
la búsqueda relativa, sin pasar por el fallback.

Cambios:

- `src/evaluator.rs`:
  - `Loader` suma `import_root: PathBuf` (estable durante toda la
    vida del loader, fijado al `base_dir` inicial = parent del
    entry file).
  - `resolve_module_path` devuelve `Vec<PathBuf>` con candidatos
    ordenados (relativo primero, después import_root si difiere).
  - `load_module` itera los candidatos; el primero que
    canonicalize OK gana.
- `src/codegen.rs`:
  - Nuevo helper `mod_qualifier_of(rel_path)` que convierte
    `types/user.rs` → `types::user`. `LoadedModuleSigs` suma
    field `mod_qualifier` (computed at construction).
  - `emit_use_decls`, `emit_module_use_decls`,
    `resolve_namespace_field`, `resolve_namespace_call`, y el
    `imported_mod_and_item` en `gen_struct_lit` ahora usan
    `mod_qualifier` (path completo) en lugar de `mod_name`
    (último segmento). Antes el codegen emitía
    `use crate::user::User` para `from types.user import User`
    y rustc fallaba con "unresolved import `crate::user`".
- 2 unit tests verdes en `evaluator::tests::loader_absoluto_*`:
  - `data_sibling_import_resuelve_via_import_root` — caso canónico.
  - `no_rompe_imports_relativos_legacy` — backward compat.
- 1 E2E codegen verde en
  `compile_e2e::loader_absoluto_data_sibling_import_compila_en_fitz_build`
  — proyecto con `src/main.fitz` + `src/types/user.fitz` +
  `src/data/users.fitz` compila a binario y corre OK.
- Boilerplate `api-postgres-python` refactorizado a usar el
  patrón limpio:
  - `data/users.fitz` ahora hace `from types.user import User` +
    `from types.api import NewUser` y devuelve `Result<User>` /
    `Result<List<User>>` tipado (no más JSON crudo).
  - `main.fitz` simplificado a delegar a `data.users::create`/
    `find`/`list_all` directo (sin coerción intermedia).
- VSCode extension: SIN cambios necesarios (fix es runtime/codegen
  puro, no toca grammar/sintaxis/types/LSP). Documentado en cierre
  formal.

Bug residual detectado durante la validación (NO regresión, NO
bloquea):

- **R.bug-13i-stack-overflow-debug**: `13i-campos-privados.fitz`
  desborda el stack en `fitz build` debug-mode en Windows
  (1 MB stack). Verificado con `git stash` que el overflow es
  pre-existente, NO disparado por esta mini-fase. Release build
  compila el ejemplo sin problema. Fix lean propuesto en
  `docs/deudas_lenguaje.md`: linker flag `/STACK:8388608` en
  `.cargo/config.toml` para Windows target. Sin presión real.

Total al cierre: **2178 unit + 275 compile_e2e + 3 openapi**.
Smoke `GUIDE_EXAMPLES_COMPILE` con asterisco — 13i flake (deuda
ya documentada).

**CIERRE FORMAL DEL PLAN POST-BOILERPLATES**: los 4 pasos
(coerción recursiva runtime + 8.7 codegen + env builtin +
loader-absoluto) cerrados entre el 2026-05-22 y el 2026-05-22.
Ningún paso requirió cambios al checker estático (la mayoría
extensiones al evaluator y codegen). Boilerplates simplificados
gracias a los fixes. Próximo norte: definir el siguiente bloque
con el autor (probablemente algo del backlog "deudas residuales
sin presión real" o una mini-fase de features nuevas).

---

## [v0.9.29] — 2026-05-22 — Feature: mini-fase env builtin — `env`/`env_or`/`load_env`

Tercer paso del plan post-boilerplates. Tres builtins nuevos para
leer variables de entorno desde Fitz, paridad bit-a-bit
intérprete↔codegen. Cierra deuda documentada en
`project_env_builtin.md` (memoria).

Builtins agregados:

- **`env(key: Str) -> Result<Str>`** — lee `std::env::var`. Si la
  var existe → `Ok(value)`, si no → `Err("env var X no definida")`.
  Fuerza al usuario a manejar el caso missing con `?` o `match`
  (paralelo a `find`/`get`/`json.loads`). Modelo "sin excepciones"
  del lenguaje respetado.
- **`env_or(key: Str, default: Str) -> Str`** — mismo lookup pero
  con default. Nunca falla. Paralelo a `Option::unwrap_or` de Rust.
- **`load_env(path: Str) -> Result<Null>`** — parser KEY=VALUE
  simple. Líneas vacías y `#` comments ignoradas, comillas dobles
  wrapping strippeadas. Sin variable expansion (`$VAR`/`${VAR}`),
  sin multi-line, sin escape chars. **Sin auto-load por diseño**:
  el usuario explícitamente llama `load_env(".env")?` en el boot
  ("explicit > magic").

Cambios:

- `src/evaluator.rs`: 3 builtins nuevos (`builtin_env`,
  `builtin_env_or`, `builtin_load_env`) + helper `parse_env_file`
  con parser KEY=VALUE simple. Registrados en `register_builtins`;
  agregados a `builtin_names()` del REPL.
- `src/types.rs::register_builtins`: 3 firmas nuevas registradas
  en el checker (`env: Function([Str]) -> Result<Str>`,
  `env_or: Function([Str, Str]) -> Str`,
  `load_env: Function([Str]) -> Result<Null>`).
- `src/codegen.rs`: 3 arms nuevos en `gen_call` que delegan a
  helpers `__fitz_env`/`__fitz_env_or`/`__fitz_load_env` emitidos
  siempre en el preludio (son fns chicas; Rust hace dead-code elim
  si no se usan).
- 8 unit tests verdes en `evaluator::tests::env_builtin_*`/
  `env_or_builtin_*`/`load_env_builtin_*` cubriendo:
  var existente como Ok, var missing como Err con mensaje
  específico, var vacía como Ok(""), propagación con `?`,
  env_or con default vs valor real, load_env de archivo con
  comments + comillas + líneas vacías, load_env de archivo
  inexistente como Err.
- 5 tests E2E verdes en `compile_e2e::env_builtin_*`/`env_or_*`/
  `load_env_*` con nuevo helper `build_and_run_with_env` que
  inyecta env vars al child via `Command::env`. Confirma paridad
  bit-a-bit `fitz run` ↔ `fitz build`.
- VSCode extension actualizada:
  - Grammar TextMate (`syntaxes/fitz.tmLanguage.json`): los 3
    builtins sumados al pattern `support.function.builtin.fitz`.
  - LSP autocomplete (`src/lsp.rs::scope_level_completions`):
    los 3 builtins listados con sus firmas en el detail.
- Cap nuevo 31 "Variables de entorno" en `docs/guide.md`
  (renumeración 31→32 Plantillas, 32→33 Qué sigue). Cubre las 3
  builtins con patrones canónicos, formato `.env`, razón del
  `Result<Str>` en `env()`, política de no-auto-load.
- Ejemplo runnable nuevo `examples/guide/31-env.fitz` agregado al
  smoke `GUIDE_EXAMPLES_COMPILE` (verde).
- Boilerplate `api-middleware-cors`: el `JWT_SECRET` hardcoded
  reemplazado por `env_or("JWT_SECRET", "demo-cambiame-...")`.
  README refrescado: la nota "env builtin es deuda futura"
  reemplazada por ejemplo de uso real. Roadmap del boilerplate
  marca esa deuda como ✓ CERRADA.

Total al cierre: **2176 unit + 274 compile_e2e + 3 openapi**.
Smoke `GUIDE_EXAMPLES_COMPILE` verde con el nuevo cap 31.

**Próximo paso del plan post-boilerplates**: Paso 4 — Loader Fitz
con imports absolutos desde manifest root (deuda
R.bug-loader-relative-only — bloquea organización multi-archivo).

---

## [v0.9.28] — 2026-05-22 — Patch: paridad codegen — coerción `PyAny → List<T>`/`Nominal`/`List<Nominal>` en `fitz build`

Cierra la deuda **R.bug-8.7-coercion-list-codegen** documentada al
cierre formal de Fase 8.7 (CHANGELOG v0.8.8 de 2026-05-15). Paso 2
del plan post-boilerplates, paridad codegen del Paso 1 (v0.9.27)
que cerró el equivalente runtime.

Antes (en `fitz build`):

```fitz
type User { id: Int, name: Str }
from python import json

fn list_users(raw: Str) -> Result<List<User>> {
    let users: List<User> = json.loads(raw)?
    // ERROR Rust: expected Arc<Mutex<Vec<UserData>>>, found __FitzPyObject
    return Ok(users)
}
```

Ahora:

```fitz
fn list_users(raw: Str) -> Result<List<User>> {
    let users: List<User> = json.loads(raw)?
    // OK — compila a binario nativo, coerce el PyList item-por-item
    // a `Arc<Mutex<Vec<Arc<Mutex<UserData>>>>>` bit-a-bit como el runtime.
    return Ok(users)
}
```

Cambios:

- `src/codegen.rs::coerce(code, from, to, env)` ahora despacha:
  - `(PyAny, List<Int>)` → `__fitz_py_to_list_i64(&{code})`
  - `(PyAny, List<Float>)` → `__fitz_py_to_list_f64(&{code})`
  - `(PyAny, List<Str>)` → `__fitz_py_to_list_string(&{code})`
  - `(PyAny, List<Bool>)` → `__fitz_py_to_list_bool(&{code})` (helper nuevo)
  - `(PyAny, Nominal(T))` → `__fitz_py_to_instance_<T>(&{code})` (helper per-tipo)
  - `(PyAny, List<Nominal(T)>)` → `__fitz_py_to_list_<T>(&{code})` (helper per-tipo)
  - Signatura cambió: añadido param `env: &TypeEnv` (89 call sites
    actualizados via sed automático).
- Nuevos métodos en `CodegenCtx`:
  - `gen_fitz_py_to_instance_helper(name, sig)` — emite
    `fn __fitz_py_to_instance_<Name>(obj: &__FitzPyObject) -> Arc<Mutex<<Name>Data>>`
    con extracción field-por-field, defaults inline, manejo de
    Nullable (`None` cuando dict missing o Python None), error
    claro cuando field requerido falta. Llamado desde
    `gen_type_def` cuando `uses_python = true`.
  - `gen_fitz_py_to_list_helper(name)` — emite
    `fn __fitz_py_to_list_<Name>(obj: &__FitzPyObject) -> Arc<Mutex<Vec<Arc<Mutex<<Name>Data>>>>>`
    iterando un PyList y delegando al helper de instance.
  - `py_field_extract_code` + `py_field_extract_arms` +
    `py_inner_extract_for_nullable` — sub-helpers para emitir
    el extract code por field según tipo (Int/Float/Str/Bool/
    Nullable<primitive>/Nominal/List<primitive>).
- 3 E2E tests verdes en `compile_e2e::fase_8_7_bis_*`:
  - `pyany_a_list_int_via_anotacion` — patrón list primitivo.
  - `pyany_a_instance_via_anotacion` — patrón single dict, con
    default field aplicado cuando falta key.
  - `pyany_a_list_de_instances` — patrón canónico del boilerplate
    `api-postgres-python::list_users`.
- READMEs de boilerplates 5/6 actualizados: la nota "deuda 8.7
  bloquea `fitz build`" reemplazada por nota técnica que cita
  el cierre 2026-05-22 (mini-fase 8.7.bis) y explica que `fitz
  build` ahora soporta el patrón end-to-end. Dockerfiles
  intencionalmente quedan con `fitz run` por boot rápido en
  containers (build desde source toma 8-12 min); usuarios que
  quieran binario standalone solo cambian `CMD`.
- VSCode extension revisada: grammar + LSP autocomplete + walkers
  + diagnostics SIN cambios (el fix es codegen puro, no toca
  sintaxis ni types estáticos). Documento confirmado en cierre
  formal.

Total al cierre: **2168 unit + 269 compile_e2e + 3 openapi**.
Smoke `GUIDE_EXAMPLES_COMPILE` verde.

**Deuda residual del scope acotado** (NO bloquea uso real):
- `Map<K, V>` coerción desde PyDict no implementada (poco común
  en práctica — `let m: Map<Str, V> = json.loads(s)?` es el caso
  raro).
- `List<List<T>>` o nominales anidados que contienen `List<Nominal>`
  como field también pendientes (deuda menor — el subset cubierto
  destraba el 90% del caso real).

---

## [v0.9.27] — 2026-05-22 — Patch: coerción recursiva `Map → Instance` sobre `List<T>`/`Map<K,V>` en runtime

Fix de la deuda **R.missing-recursive-instance-coercion** (descubierta
el 2026-05-22 al armar el 6to boilerplate `api-fullstack-postgres`).
La coerción 8.4.3 (`Map → Instance`) ahora recursa sobre `List<T>` y
`Map<K, V>` cuando el inner es nominal o `Nullable(nominal)`.

Antes:

```fitz
let users: List<User> = json.loads(raw)?
// users es List<Map>, NO List<User>. El binding pasa el checker
// gradual pero `users.find(fn(u) => u.name == "x")` falla con
// "Map no tiene field name".
```

Workaround anterior (loop manual):

```fitz
let maps: List<Any> = json.loads(raw)?
let users: List<User> = []
for m in maps {
    let u: User = m   // ← acá disparaba la coerción Map → User
    users.push(u)
}
```

Ahora:

```fitz
let users: List<User> = json.loads(raw)?
// users es List<User> directamente, cada item coercionado item-por-item.
```

Cambios:

- `src/evaluator.rs::coerce_to_annotation` con dos casos recursivos
  nuevos al inicio (List + Map). Solo dispara si el inner es nominal
  (filtra `List<Int>`, `List<Any>`, etc. — passthrough).
- Helper `is_nominal_target(ty, env)` chequea contra el env si el
  ident apunta a un `Value::Type`.
- 8 unit tests verdes en `evaluator::tests::coerce_recursive_*`
  cubriendo: caso canónico, lista vacía, lista de primitivos no
  dispara, Nullable nominal con `Null` pasando, `Map<Str, User>`,
  error claro con field requerido faltante, default aplicado,
  passthrough sin coerción si value no es List.
- 2 boilerplates simplificados:
  - `api-fullstack-postgres::list_tasks` de loop manual a 1 línea.
  - `api-postgres-python::list_users` de `Result<Str>` con JSON
    crudo a `Result<List<User>>` tipado.

Total al cierre: **2168 unit + 257 compile_e2e + 3 openapi**. Smoke
`GUIDE_EXAMPLES_COMPILE` verde.

**Deuda derivada que sigue abierta**: 8.7 (codegen) — `fitz build`
todavía necesita wiring de `coerce(PyAny → List<T>)` para paridad
bit-a-bit. Es el siguiente paso del plan post-boilerplates.

---

## [v0.9.26] — 2026-05-22 — Patch: fix OPTIONS preflight duplicado al compartir path con CORS

Fix de la deuda **R.bug-options-preflight-shared-path** (descubierta
el 2026-05-22 al validar el 6to boilerplate end-to-end con frontend).

Cuando dos o más handlers HTTP compartían el mismo path con
`@middleware(cors(...))` declarado en cada uno (caso típico CRUD:
`/tasks` con `@get` + `@post`, o `/tasks/{id}` con
`@get`/`@put`/`@delete`), axum hacía panic al construir el `Router`:

```
Overlapping method route. Handler for `OPTIONS /tasks` already exists
```

Cada handler intentaba registrar su propio OPTIONS preflight para
el mismo path. Fix coordinado runtime + codegen:

- **Intérprete (`src/http.rs::build_router_with_asyncapi`)**:
  pre-cómputo de `CorsConfig` merged por path (unión de
  `allow_methods` preservando orden, `allow_headers` case-insensitive,
  max `max_age`, primer `allow_origin` gana). Solo el OWNER del
  path emite el preflight con la config merged.
- **Codegen (`src/codegen.rs`)**: mismo patrón. Nuevos campos en
  `CodegenCtx`: `cors_merged_per_path` + `cors_preflight_owner`.
  Pre-scan `precompute_cors_merge(http_fns)` corre antes del loop de
  wrappers. `emit_cors_helpers` solo emite el preflight para el
  owner; nuevo método `cors_resolve_fn_for(sig)` para que los
  wrappers no-owner referencien el resolver compartido del owner.
- 4 unit tests verdes en
  `http::tests::bug_options_preflight_duplicado_*` (no-panic,
  methods merged, 3 verbos con `{id}`, headers case-insensitive
  dedup).
- 1 E2E verde en
  `compile_e2e::r_bug_options_preflight_duplicado_en_fitz_build_paridad_con_fitz_run`.

Acompañado por:

- **6 boilerplates Dockerizados** live en `boilerplates/` con README
  general comparativo (`cli-tool`, `api-simple`,
  `api-middleware-cors`, `api-websocket`, `api-postgres-python`,
  `api-fullstack-postgres`). El 6to es el showcase fullstack —
  frontend rico vanilla + API Fitz + Postgres en 3 containers.
- Mención de boilerplates en README + cap 31 nuevo de
  `docs/guide.md` (renumeración 31→32 Qué sigue) + `docs/index.md`.
- Naming real de `.vsix` corregido en docs
  (`fitz-lang-<plataforma>.vsix`, 4 plataformas: Win x64, Linux
  x64/ARM, macOS Apple Silicon).
- Multiplataforma resaltado como diferencial en README + index +
  cap 1 de la guía.
- Bug entry como CERRADO en `docs/deudas_lenguaje.md`.

---

## [v0.9.25] — 2026-05-21 — Patch: fix codegen deadlock en string interp con re-locks

**Bug fix crítico** descubierto al validar el primer boilerplate
Dockerizado (`boilerplates/cli-tool`).

**Bug**: `gen_str_interp` emitía `format!(fmt, arg1, arg2, ...)` donde
los temporales (MutexGuards de `.lock().unwrap()`) vivían hasta el
final de la statement. Si dos args lockeaban el mismo `Arc<Mutex<>>`
(caso típico: `print("{xs.len()} - {total(xs)}")`), el segundo
`.lock()` desde el mismo thread quedaba esperando que el primero
libere → **deadlock silencioso del binario** (std::sync::Mutex no es
re-entrant). El programa terminaba sin panic ni error visible — solo
output truncado.

**Fix** en `src/codegen.rs::gen_str_interp`: cuando hay ≥2 args, emitir
cada arg como `let __aN = <code>;` adentro de un bloque ANTES del
`format!`. Cada `let` cierra una statement → dropea el MutexGuard
inmediatamente. El siguiente arg evalúa sin guards vivos del anterior.
0 args mantiene `String::from`, 1 arg mantiene `format!` directo.

**Regression test**: `tests/compile_e2e.rs::r_bug_deadlock_str_interp_re_lock_mismo_arc_no_cuelga`.
Si el deadlock vuelve, el test falla por timeout/exit code.

**Boilerplate cli-tool** (`boilerplates/cli-tool/`) incluido en este
release como showcase del fix funcionando end-to-end: Dockerfile +
README exhaustivo + .gitignore + .dockerignore + fitz.toml con
`edition = "2026"` + main.fitz con report generator usando el
patrón problemático que el fix arregla.

**Validado**: 2156 unit + 256 compile_e2e (255 + 1 nuevo) +
smoke con 78 ejemplos guide verde. Sin breaking changes — solo
bug fix.

**Por qué importa este release**: la imagen Docker `ghcr.io/<owner>/
fitz:v0.9.24` tenía el binario con el bug. Boilerplates posteriores
van a usar `FROM ghcr.io/<owner>/fitz:latest` que ahora apunta a
v0.9.25 con el fix. Sin este release, los boilerplates con patterns
típicos de print interp se cuelgan.

Deuda **R.bug-deadlock** cerrada el mismo día del descubrimiento.

## [v0.9.24] — 2026-05-21 — Cierre formal Fase 9.w MVP entera (Stack web first-class)

**Cierre formal del bloque entero "Stack web first-class"** —
9.w.1 + 9.w.2 + 9.w.3 cerradas entre 2026-05-20 y 2026-05-21.
9.w.4 (ORM nativo + migraciones) diferida a **Fase 10** por
scope técnico justificado.

**Diferenciales validados del bloque** (con caps + ejemplos
runnable end-to-end):

1. **Auth como decoradores del lenguaje** (`@auth_provider` +
   `@authenticated` + `@admin`) con built-ins `jwt`/`hash`
   (HS256/HS384/HS512 + Argon2id) — checker estático en
   compile-time + OpenAPI auto-documentado con `securitySchemes.
   bearerAuth` + paridad bit-a-bit `fitz run` ↔ `fitz build` +
   cero `pip install jsonwebtoken passlib`. Cap 28 +
   `examples/guide/28-auth.fitz` (<100 LoC: login + /me +
   /admin con JWT real).
2. **WebSockets tipados** (`@ws("/path")` + `WsConn<T>`) con
   **marshaling JSON automático** + **AsyncAPI 3.0
   auto-generado** en `/asyncapi.json` + **heartbeat built-in**
   con `@server(ws_heartbeat_secs=N)` + **auth integrada** en
   el handshake (`@authenticated`/`@admin` apilados ANTES del
   HTTP upgrade) + paridad bit-a-bit. Cap 29 +
   `examples/guide/29-ws.fitz` (<100 LoC: chat broadcast con
   login + JWT + heartbeat).
3. **Jobs sin Celery** (`@cron("expr")` + `@background` +
   `spawn(fn_call)`) sin broker externo (Redis/RabbitMQ no son
   requisito) + checker estático del callsite `spawn(...)` que
   refina el ret type a `Future<T>` con T concreto +
   cron-only mode systemd-friendly (`signal::ctrl_c` automático
   sin `@server`) + paridad bit-a-bit. Cap 30 +
   `examples/guide/30-cron-background.fitz` (<100 LoC: URL
   shortener con HTTP + cron stats + spawn tracking async).

**Ningún otro lenguaje combina** auth + JWT/Argon2 +
WebSockets tipados + AsyncAPI auto + cron + spawn tipado en el
core del compilador, sin broker externo, con paridad bit-a-bit
intérprete↔binario nativo, cero deps externas para features
intrínsecas.

**Decisión de scope de 9.w.4 (ORM nativo)**: difetida a
**Fase 10**. El driver Postgres puro en Fitz es un proyecto
del tamaño de todo Fase 5-9 combinado. Implementar el
protocolo binario desde cero (handshake + SCRAM-SHA-256 +
prepared statements + ~40 tipos OID + cursors + transacciones
+ COPY + LISTEN/NOTIFY + pool + retry) sin via libpq es
comparable a `tokio-postgres`/`sqlx` que llevaron años de
desarrollo. Más ORM declarativo + migraciones autogeneradas +
decisiones de diseño abiertas (Postgres-first vs multi-DB,
async-first vs sync-first). **Gap cubierto por interop
Python**: cap 21 documenta SQLAlchemy desde Fitz con `fitz
py-types` y CRUD runnable. Fase 10 arranca cuando aparezca
proyecto real en Fitz que choque con las limitaciones de
interop Python.

**Acumulado al cierre del bloque 9.w MVP**:

- **2156 unit tests** sin feature (~80 unit tests nuevos del
  bloque: 33 de 9.w.1 + N de 9.w.2 + 32 de 9.w.3).
- **90 LSP unit tests** con `--features lsp` (incluye
  completion de `jwt`/`hash`/`WsConn`/`spawn`).
- **76 cli_e2e + 3 openapi**.
- **255 compile_e2e** con smoke ejemplos guía (incluye
  `28-auth.fitz`, `29-ws.fitz`, `30-cron-background.fitz`).
- Clippy `-D warnings` limpio en ambos modos (con y sin
  features).
- **3 caps nuevos** en `docs/guide.md` (28, 29, 30) + 3
  ejemplos runnable end-to-end.
- **Deps nuevas** del binario: `jsonwebtoken = "9"` +
  `argon2 = "0.5"` + `rand_core = "0.6"` (9.w.1); axum
  feature `ws` + `futures-util` + dev-dep
  `tokio-tungstenite` (9.w.2); `cron = "0.12"` +
  `chrono = "0.4"` (9.w.3).

**Próximo norte**: boilerplates Dockerizados (memoria
`project_boilerplates`) — showcase del stack cerrado en 4
boilerplates listos para `git clone` + `fitz run`. Después
repo público + sitio docs MkDocs Material.

## [v0.9.23] — 2026-05-21 — Fase 9.w.3 CERRADA — Jobs sin Celery (`@cron` + `@background` + `spawn`)

**Cierre del tercer sub-paso de Fase 9.w (stack web first-class).**
Tres piezas nativas del lenguaje montan jobs sin broker externo:
**`@cron("expr")`** para tareas periódicas (5/6/7 fields cron
Unix), **`@background`** como marcador opt-in para autorizar el
callsite, y **`spawn(fn_call)`** fire-and-forget que devuelve
`Future<T>` tipado. Sin Celery, sin Redis, sin systemd timers —
todo en el mismo binario con paridad bit-a-bit `fitz run` ↔
`fitz build`.

**Sub-pasos (4 commits)**:

- **9.w.3.a** — Checker estático: `Type::Future<T>` ya existe;
  acá refinamos el ret de `spawn(...)` cuando el target es una
  fn `@background` (lookup via `CheckCtx.background_fns`).
  Nuevas validaciones:
  - `@cron`: 1 arg Str, sin params, return Null/Result/Future.
    No combinable con `@get`/`@post`/`@ws`/`@background`/
    `@auth_provider`/`@test`.
  - `@background`: sin args/kwargs. No combinable con otros
    decorators "handler" (mismo set que @cron).
  - `spawn(...)`: 1 arg que es `Expr::Call` literal a fn
    `@background`. El callsite retorna `Future<T>` (T del target
    o Future<T> si target ya es async, sin doble wrap).
  - Dispatch en `synthesize_expr` solo dispara cuando el binding
    "spawn" no fue shadowed (sigue siendo `Type::Any` builtin).
  - LSP completion list `spawn` con detail
    `fn(fn_call) -> Future<T>  // requiere @background`.
  17 unit tests.

- **9.w.3.b** — Runtime intérprete: nuevo módulo
  `src/cron_jobs.rs` con `CronJob` (handler + Schedule parseado)
  + `CronRegistry` (paralelo a HttpRegistry, vive adentro)
  + `spawn_cron_scheduler` (un `tokio::spawn` por job)
  + `run_scheduler_only` (cron-only mode con multi_thread +
  ctrl_c). `process_decorator` branches para `@cron` (parsea
  expression via crate `cron`, registra job) y `@background`
  (no-op runtime). `eval_call` intercepta `spawn(fn_call)` ANTES
  de evaluar args para capturar el AST del inner call; ejecuta
  `tokio::spawn(invoke)` con await del Future si async, envuelve
  el JoinHandle en `Value::Future`. Cron-only mode en `main.rs`:
  cuando NO hay rutas HTTP pero SÍ jobs `@cron`, llama
  `cron_jobs::run_scheduler_only` que bloquea hasta Ctrl+C
  (decisión confirmada con el autor). **Fix bug preexistente**:
  handlers `async fn` HTTP en intérprete retornaban "Future
  pendiente no es serializable" porque `handle_task` nunca
  awaiteaba el Future. Solo afectaba `fitz run` (codegen lo
  hacía bien). Detectado al validar 9.w.3.b con un POST handler
  async que llama `spawn(...)`. Helper `await_if_future` en
  `http.rs` para extraer el Value final. Normalización 5→6
  fields del cron expression: si el usuario provee Unix clásico
  (5 fields), el runtime prependa `"0 "` (segundo 0). Deps
  nuevas: `cron = "0.12"` y `chrono = "0.4"` (no opcionales).
  8 unit tests.

- **9.w.3.c** — Codegen `fitz build`: Cargo.toml condicional
  suma `cron`/`chrono` cuando `uses_jobs = true`. Tokio con
  feature `signal` adicional en cron-only mode. Multi_thread
  flavor por default cuando hay jobs. Preludio
  `__fitz_run_cron_job(name, schedule, handler)` análogo al
  intérprete + helper `__fitz_normalize_cron`. `PartitionedProgram`
  gana `cron_fns` paralelo a `http_fns`/`ws_fns`. `gen_main`
  (CLI) y `gen_http_main` ambos invocan `emit_cron_job_spawns()`
  que itera `ctx.cron_jobs_info` y emite por job:
  ```
  tokio::spawn(__fitz_run_cron_job(
      "name".to_string(),
      cron::Schedule::from_str(&__fitz_normalize_cron("expr"))?,
      || async { name().await; },
  ));
  ```
  CLI cron-only mode añade `signal::ctrl_c().await` al final
  del main. HTTP + cron arranca el scheduler ANTES de
  `axum::serve`. `spawn(fn_call)` dispatch en `gen_call` solo
  dispara cuando `spawn` no fue shadowed; emite
  `tokio::spawn(async move { target(args...).await })` con
  `.await` solo si target es async; envuelve el JoinHandle en
  `Box::pin(async move { jh.await.unwrap() })` para case con
  `Pin<Box<dyn Future>>` del codegen. 7 unit tests.

- **9.w.3.d** — Cap 30 nuevo "Jobs sin Celery" en
  `docs/guide.md` (renumeración 30→31 "Qué sigue") + ejemplo
  runnable `examples/guide/30-cron-background.fitz` (URL
  shortener con `type Link`, HTTP + cron stats cada 5 seg +
  `spawn(track_click)` de tracking async sin bloquear la
  response, <100 LoC) + README emphasis con los 5 diferenciales
  en tabla feature comparison + footnote dedicado ♠ + bullets en
  "Estado del proyecto" y "Qué funciona hoy". Smoke en
  `GUIDE_EXAMPLES_COMPILE`.

**Decisiones técnicas del MVP** (no en roadmap original):

- **Cron-only mode vivo bloqueante** (vs run-once o flag opt-in):
  modo systemd-friendly drop-in. Confirmado con el autor.
- **`@cron` acepta sync y async** (vs solo async): ergonomía
  consistente con el resto del lenguaje. Confirmado con el autor.
- **`@background` como marcador opt-in** (vs cualquier fn
  spawneable): evita usos accidentales sobre fns regulares cuyo
  retorno el caller espera consumir.
- **`spawn(...)` exige call literal a fn `@background`** (vs var
  o expression compuesta): permite refinamiento estático del ret
  type y validación clara en compile-time.
- **Crate `cron` para parsing** (vs parser propio): liviano,
  audit history limpio, soporta 5/6/7 fields.
- **Normalización 5→6 fields automática**: preserva UX familiar
  del cron Unix sin reescribir la sintaxis aceptada por el crate.
- **JoinHandle envuelto en `Value::Future`/`Pin<Box<dyn Future>>`**:
  unifica la API con `Future<T>` Fitz existente — descartar el
  Future deja la task detached (fire-and-forget natural).

**Por qué importa**:

- **Sin broker externo**: para 90% de servicios reales (tareas
  de mantenimiento, scripts periódicos, fire-and-forget de
  notificaciones), los jobs en memoria del proceso son
  suficientes. Persistencia entre restarts llega con Fase 10 +
  DB nativa, sin cambiar la sintaxis.
- **Checker estático**: validación en compile-time del callsite
  `spawn(...)` (target con `@background` Y refinamiento del ret
  type) vs `tokio::spawn` sin marcador, `asyncio.create_task`
  sin tipos, Celery con string-based task names.
- **Paridad bit-a-bit**: el flow corre idéntico en intérprete
  (rapid dev) y binario nativo (deploy a prod).
- **Cero deps externas**: `cron` + `chrono` van en el binario
  `fitz`. No hay `pip install celery`, `npm install bull`,
  `cargo add tokio-cron-scheduler`.
- **Ningún otro lenguaje** combina cron + background workers +
  spawn tipado en el core sin broker externo y con paridad
  intérprete↔binario.

**Deuda residual derivada de 9.w.3** (no bloquea uso real; abre
items para iteración 2 post-Fase 10):

- Persistencia de jobs entre restarts (requiere DB nativa, Fase
  10) o backend de queue (Redis, post-MVP).
- Visibility de jobs (panel admin con runs, stats, retries).
- Retry con backoff exponencial cuando un job falla.
- Coordinación entre múltiples instancias (locks distribuidos
  para que un cron solo corra en un nodo).
- `spawn` con coordinación múltiple (Promise.all style requiere
  agregación manual con vectores de futures).
- Cron timezone configurable (hoy todos los jobs usan
  `chrono::Utc::now()`).

**Próximo norte**: resto de Fase 9.w — ORM nativo + migraciones
(escala a Fase 10), o cierre formal de Fase 9.w entera.

## [v0.9.22] — 2026-05-21 — Fase 9.w.2 CERRADA — WebSockets tipados (`@ws` + `WsConn<T>` + AsyncAPI 3.0 + heartbeat + auth integrada)

**Cierre del segundo sub-paso de Fase 9.w (stack web first-class).**
`@ws("/path")` sobre `async fn` + `WsConn<T>` con métodos
`recv`/`send`/`broadcast`/`close` montan un servidor de WebSockets
tipado end-to-end. Cinco diferenciales que vuelven a Fitz único
en este espacio: **marshaling JSON automático** del frame al
`type` declarado, **AsyncAPI 3.0 auto-generado** en
`/asyncapi.json`, **heartbeat built-in** con
`@server(ws_heartbeat_secs=N)`, **auth integrada**
(`@authenticated`/`@admin` apilados sobre `@ws` validan bearer
ANTES del HTTP upgrade) y **codegen con paridad** bit-a-bit
`fitz run` ↔ `fitz build`.

**Sub-pasos (6 commits)**:

- **9.w.2.a** — Checker: `Type::WsConn(Box<Type>)` variant,
  `resolve_type_expr` para `WsConn<T>` aridad 1,
  `infer_wsconn_method` con signatures paramétricas
  (`recv() -> Result<T>`, `send(T) -> Result<Null>`,
  `broadcast(T) -> Result<Null>`, `close() -> Result<Null>`),
  `check_ws_handler` validando shape del handler (async fn, primer
  param `WsConn<T>`, return `Null`, compatibilidad con auth). 14
  unit tests.
- **9.w.2.b** — Value runtime: `WsConnHandle`,
  `WsBroadcasterTrait`, `WsReadStreamTrait`, `WsOutMessage`
  (Text/Close), `Value::WsConn(Arc<WsConnHandle>)`. Manual Debug.
  `register_ws_route` en evaluator paralelo a
  `register_http_route`; `process_decorator` branch para `@ws`;
  `dispatch_method` arms para `(Value::WsConn, recv/send/broadcast/
  close)`; `ws_conn_recv` usa `coerce_to_annotation` (8.4.3) para
  Map → Instance cuando T es nominal.
- **9.w.2.c** — Runtime HTTP: `WsBroadcaster` con
  `parking_lot::Mutex<HashMap<endpoint, Vec<(conn_id, outbox_tx)>>>` +
  `AtomicU64` next_id. `WsReadStreamImpl` wrapping `SplitStream`
  con filtrado de ping/pong/binary. `RouteSpec.is_ws/
  ws_conn_param_name/ws_msg_type`. `HttpRegistry.ws_broadcaster:
  Arc<WsBroadcaster>`. `build_ws_method_router` emite axum GET
  handler con `WebSocketUpgrade` extractor + auth pre-upgrade
  (devuelve 401/403 vía HTTP Response ANTES de `ws.on_upgrade`).
  `build_ws_conn` spawnea writer task (mpsc::UnboundedReceiver →
  sink) + opcional heartbeat task. axum 0.8 con feature `ws` +
  `futures-util` + dev-dep `tokio-tungstenite`.
- **9.w.2.d** — AsyncAPI 3.0 schema (`src/asyncapi.rs` nuevo,
  ~350 LoC). `AsyncApiChannelInfo`,
  `channels_from_registry` (runtime),
  `pseudo_channels_from_ast` (codegen). `generate_asyncapi_with_version`
  emite channels (uno por endpoint `@ws`), operations
  receive/send por channel, `components.securitySchemes.bearerAuth`
  cuando hay auth. `BTreeMap` para orden determinístico.
  `build_router_with_asyncapi` registra `/asyncapi.json`. En
  codegen, `auto_asyncapi` gate emite `__FITZ_ASYNCAPI_SCHEMA` +
  handler `__serve_asyncapi_json` + route. 8 unit tests.
- **9.w.2.e** — Heartbeat ping/pong automático.
  `WsOutMessage::Ping` + `ServerConfig.ws_heartbeat_secs: u64`
  (default 30). Parsing de `@server(ws_heartbeat_secs=N)` con
  validación (`Int` literal, no negativo). Si N > 0,
  `build_ws_conn` spawnea `tokio::time::interval(N segundos)` que
  envía Ping frames por el outbox; si el cliente no responde
  Pong, el sink falla en el próximo write y el writer task
  termina limpio (no requiere tracking explícito de Pong).
  `CodegenCtx.ws_heartbeat_secs` capturado ANTES de emitir WS
  wrappers (gen_ws_handler_wrapper corre antes de gen_http_main).
  6 unit tests.
- **9.w.2.f** — Cap 29 "WebSockets tipados" en `docs/guide.md`
  (renumeración 29→30) + ejemplo runnable
  `examples/guide/29-ws.fitz` (servidor de chat con login HTTP
  + JWT + `@authenticated @ws("/chat")` + broadcast multi-client
  + `@server(43929, ws_heartbeat_secs=30)`, <100 líneas) +
  README emphasis con los 5 diferenciales en la tabla feature
  comparison + footnote dedicado + bullets en "Estado del
  proyecto" y "Qué funciona hoy". Smoke en
  `GUIDE_EXAMPLES_COMPILE`.

**Por qué importa**:

- **Marshaling JSON automático**: declarás `WsConn<ChatMsg>` y
  cada frame text se serializa/deserializa al `type` sin
  `json.loads` + Pydantic / `JSON.parse` + Zod manual. El
  mismo trait que sirve HTTP (`__ToFitzJson`/`__FromFitzJson`)
  cubre WS.
- **AsyncAPI auto-generado**: el schema sale del código fuente
  (vs Socket.IO/Phoenix/SignalR/FastAPI WebSocket donde vive en
  un README que se atrasa). Tooling estándar (AsyncAPI Studio,
  generadores de clientes JS/TS/Python/Java) lo consume directo.
- **Heartbeat built-in**: `@server(ws_heartbeat_secs=N)` y
  listo. Pasa de largo Nginx (60s idle), Cloudflare (~100s) y
  AWS ALB (60s) sin código del usuario.
- **Auth integrada**: `@authenticated`/`@admin` apilados sobre
  `@ws` validan el bearer token ANTES del HTTP upgrade. El
  cliente recibe 401/403 sin abrir el socket — menos attack
  surface, menos recursos consumidos.
- **Codegen con paridad**: el flow WS funciona idéntico en
  `fitz run` y en el binario nativo de `fitz build`.
- **Ningún otro lenguaje hoy combina** WS tipados con AsyncAPI
  auto-generado del código fuente, heartbeat built-in y auth
  integrada en el handshake.

**Deuda residual derivada de 9.w.2** (no bloquea uso real):
binary frames (`Vec<u8>` payload — hoy solo text), AsyncAPI UI
equivalente al `/docs` de OpenAPI (hoy solo el JSON), tipado
bidireccional separado (`WsConn<In, Out>` — hoy `T` único),
reconnect con state replay (requiere persistencia, Fase 10),
rooms/channels dentro de un endpoint (broadcast es a TODOS los
clientes del endpoint), backpressure explícito (outbox unbounded
hoy).

**Próximo norte**: resto de Fase 9.w — `@cron` + `@background`
(jobs sin Celery), y ORM nativo + migraciones (escalado a Fase
10).

## [v0.9.21] — 2026-05-20 — Fase 9.w.1 CERRADA — Auth nativa (`@auth_provider`/`@authenticated`/`@admin` + `jwt`/`hash`)

**Cierre del primer sub-paso de Fase 9.w (stack web first-class).**
Tres decoradores nuevos del lenguaje + dos módulos built-in
montan un flujo de auth + JWT + password hashing entero sin
deps externas. El checker valida estáticamente; OpenAPI 3.1
auto-documenta los requirements y los 401/403; paridad bit-a-bit
`fitz run` ↔ `fitz build`.

**Sub-pasos (6 commits)**:

- **9.w.1.a** — Checker: `collect_auth_provider` pre-scan
  (singleton; signature `fn(Map<Str,Str>) -> Result<T-nominal>`)
  + `check_auth_decorators` por handler (exige provider + handler
  HTTP + param compatible con `User`; `@admin` exige campo
  `role: Str`). 16 unit tests.
- **9.w.1.b** — Built-ins `jwt` y `hash` como `Value::Module`
  pre-registrados. `jwt.encode/decode` (HS256/384/512 con
  `jsonwebtoken = "9"`), `hash.password/verify` (Argon2id con
  `argon2 = "0.5"` + `rand_core` para `OsRng`). Sin kwargs en
  builtins; `alg` como positional opcional al final.
  `decode` siempre devuelve `Result<Map>`; `verify` siempre
  devuelve `Bool` (hash malformado → `false` por seguridad).
  Checker tipa como `Any` (deuda de `Type::Function` sin
  opcionales); LSP completions agregan `jwt`/`hash` como
  `MODULE` kind con after-dot shortcut. 16 unit tests.
- **9.w.1.c** — Runtime auth en `fitz run`. Wrapper en
  `handle_task` después de middlewares y antes de body parsing:
  construye `Map<Str,Str>` de headers, invoca al provider (con
  `.await` si es async), match `Result<User>` → 401/200 o 403
  (admin). `AuthSpec`/`AuthProviderHandle` en `http.rs`;
  `register_auth_provider` + `collect_route_auth` en evaluator.
  Provider singleton con order requirement (provider antes que
  handlers que lo usan). 9 unit E2E.
- **9.w.1.d** — Codegen `fitz build`. Helpers `__fitz_jwt_*`/
  `__fitz_hash_*` en preludio gated por `uses_auth`; Cargo.toml
  condicional suma `jsonwebtoken`/`argon2`/`rand_core` cuando
  aplica. Dispatch en `gen_call` para `jwt.encode/decode/hash.
  password/verify`. `HandlerSig` suma
  `auth + auth_user_param_name`; `emit_auth_check` (paralelo al
  wrapper del intérprete); `emit_axum_extractors` agarra
  `HeaderMap` cuando hay auth. 2 tests compile_e2e (CLI puros +
  HTTP end-to-end).
- **9.w.1.e** — OpenAPI security scheme.
  `OpenApiRouteInfo.auth` + propagación;
  `components.securitySchemes.bearerAuth` (type=http,
  scheme=bearer, bearerFormat=JWT) cuando hay auth; `security:
  [{bearerAuth: []}]` por handler protegido; 401 (auth) y 403
  (admin) auto en responses con shape `{"error": Str}`. 5 unit
  tests del schema.
- **9.w.1.f** — Cap 28 "Auth nativa" en `docs/guide.md`
  (renumeración 28→29) + ejemplo runnable
  `examples/guide/28-auth.fitz` (login + /me + /admin con
  JWT real, <100 líneas) + README emphasis del diferencial vs
  FastAPI/Spring/ASP.NET (cero deps, checker estático, OpenAPI
  auto, paridad run↔build). Suma a `GUIDE_EXAMPLES_COMPILE`.
  Refresh oportunista del marcador de Interop Python en la
  tabla feature comparison del README (de 🚧 a ✅ con footnote
  honesta sobre deuda residual derivada).

**Por qué importa**:

- **Estático, no reflection**: el checker valida en compile-time
  que cada `@authenticated`/`@admin` tenga provider registrado
  y reciba el `User` correcto. Spring AOP / ASP.NET
  `[Authorize]` resuelven en runtime con reflection; cuando
  rompe, rompe en prod.
- **Zero dependencies**: JWT signing + Argon2id password hashing
  vienen en el binario `fitz`. No hay `requirements.txt` /
  `package.json` / `Cargo.toml` extra que mantener. Deploy es
  un binario.
- **OpenAPI auto-documentado**: `bearerAuth` + `security` por
  operation + 401/403 — sin escribir specs OpenAPI a mano.
- **Paridad bit-a-bit**: el flow funciona idéntico en
  intérprete y binario nativo.

**Deuda residual derivada de 9.w.1** (no bloquea uso real):
sessions cookie-based + RBAC multi-rol + token refresh/revocación
(requieren DB nativa, Fase 10); asimétricos JWT (RS256/ES256 con
PEM); provider request-aware más allá de headers (body, método).

**Próximo norte**: resto de Fase 9.w — `@ws("/chat")` (WebSockets
tipados con `WsConn<T>`), `@cron` + `@background` (jobs sin
Celery), y ORM nativo + migraciones (escalado a Fase 10).

## [v0.9.20] — 2026-05-17 — Refresh masivo de docs + cap 16b Package manager + fix bug fmt

Sub-paso dedicado de refresh general de docs acumulado durante
Fase 9.z entera. Cuatro sub-tareas (A + B + C + D) cerradas en
una tanda:

**A — Caps stale en `docs/guide.md`** refrescados:

- **Cap 12 "Tipos con `type`"**: removido "Chequeo de tipos en
  runtime" + "Tipos compuestos en campos no se validan" (ambos
  cerrados post-Fase 5a/5.1). Sumado bloque "Lo que SÍ anda y
  antes era deuda" con referencias a Fase 5a / 5.1 / PreF8.3.
- **Cap 13 "Métodos"**: removido "Encadenamiento multi-línea"
  (cerrado en PreF8.2). Sumado ejemplo idiomático.
- **Cap 17 "HTTP nativo"**: reescrita sección "Qué pasa adentro"
  (era stale — describía el bridge mpsc/dos-threads que F17
  eliminó). Removidos 6 ítems de "Qué todavía no anda" todos
  cerrados: async/await reales, status codes custom, query
  params, headers de request, named args en decoradores,
  middleware. Sumado bloque con referencias a sub-secciones
  existentes del mismo cap.
- **Cap 20 "fitz build"**: removido "Server HTTP multi-threaded
  como deuda" (cerrado F17 — runtime tokio default multi-thread
  con state HTTP como `LazyLock<Arc<Mutex<T>>>`). Sumado bloque
  con state HTTP compartido + paralelismo HTTP real + interop
  Python como features cerradas que antes eran deuda.

**B — Cap 16b nuevo "Package manager"** en `docs/guide.md`:

- Posición: entre cap 16 "Módulos" y cap 17 "HTTP nativo" en
  Parte 6 "Organización" (convención `16b` paralela a
  `17b-middleware`, `19b-paralelismo`).
- Cubre: anatomía de `fitz.toml` (`[package]`/`[bin]`/`[lib]`/
  `[dependencies]`), `fitz new`/`fitz init` con scaffolding,
  manifest mode de `fitz run`/`build`/`check` (walk-up Cargo-style),
  deps path (`{ path = "../foo" }`), deps git con tag o rev
  (`{ git = "...", tag = "v1.0.0" }`), lockfile `fitz.lock` con
  formato Cargo-style, `fitz add`/`remove`/`update`. "Lo que NO
  anda todavía" lista registry público, dev-dependencies,
  workspaces, branches en git deps, transitive deps.
- **Ejemplo runnable** `examples/guide/16b-pkg-manager/` con
  dos proyectos: `greetings/` (lib con dos fns) + `greeter/`
  (bin que importa via `[dependencies] greetings = { path =
  "../greetings" }`). README en el ejemplo explica el flujo
  end-to-end.
- **2 cli_e2e tests nuevos**:
  - `cap_16b_ejemplo_greeter_corre_y_genera_lockfile` valida
    `fitz run` + output esperado + lockfile auto-generado.
  - `cap_16b_fitz_build_compila_greeter_a_binario_nativo`
    valida `fitz build` + binario producido + paridad de output
    con `fitz run`.

**C — `docs/architecture.md` refresh completo**:

- Reescrito de cero (287 → ~470 líneas).
- Diagrama mermaid + ASCII fallback actualizados: muestran los
  **15 sub-comandos** del CLI en lugar de los 3 originales
  (check/run/build).
- Agrupados en 5 familias: pipeline core, package manager, DX,
  interop Python, editor support.
- **12 módulos** nuevos documentados que faltaban: `lib.rs`,
  `manifest.rs`, `lockfile.rs`, `git_dep.rs`, `testing.rs`,
  `fmt.rs`, `lint.rs`, `lsp.rs`, `py_interop.rs`, `py_types.rs`,
  `openapi.rs`. Cada uno cita su Fase de origen + APIs públicas
  relevantes.
- Removidas referencias stale: "tres subcomandos" (línea 90),
  "axum + tokio en thread separado" (línea 24 del diagrama —
  F17 lo eliminó), "Rc<RefCell<>>" en value.rs (post-F17 es
  `Arc<parking_lot::Mutex<>>`).
- Sumada nota explicando features opcionales (`python`, `lsp`)
  como cargo features con bin separado para `fitz-lsp`.
- "Por qué este orden y no otro" actualizado para reflejar
  decisiones recientes (TypeInfo side-table en lugar de IR,
  package manager y DX como módulos hermanos no parte del
  pipeline core).

**D — Fix bug del fmt** (deuda residual de 9.z.1.b):

Bug: trailing comment al final del body de una fn seguido de
otro bloque insertaba blank spurio adentro del body del segundo
bloque. MRE:
```fitz
fn greet(name: Str) -> Str {
    return "Hola, {name}!" // inline
}

for n in ["Ada"] {
    print(greet(n))   // ← antes del fix, había blank line antes acá
}
```

Root cause: `had_blank_in_source` en `fmt_stmt_list` usaba
`after_what = max(prev_end_line, last_emitted_comment_line)`.
Al entrar a un nuevo bloque (`in_block=true`,
`prev_end_line=0`), `last_emitted_comment_line` arrastraba un
valor de scope outer (el trailing comment del stmt anterior al
bloque) y `has_blank_between` chequeaba blanks FUERA del bloque
actual.

Fix: guarda condicional. En `in_block=true`, el chequeo requiere
`prev_end_line > 0` (paralela a la `smart_blank`). En top-level
se preserva el behavior previo (`after_what > 0`) para no romper
blanks legítimas entre header comments y el primer stmt del
file.

Test E2E nuevo `fmt_trailing_comment_seguido_de_bloque_no_inserta_blank_spurio`
en `tests/cli_e2e.rs` protege contra regresión.

`docs/fmt-style.md` actualizado con entry de "Historia"
documentando el fix.

**Tests al cierre del refresh**:
- 1381 unit / 76 cli_e2e (+3 vs 9.z.5: 2 del cap 16b + 1 del
  fix fmt) / 79 compile_e2e / 3 openapi.
- Clippy `-D warnings` limpio.

**Deudas residuales actualizadas en `docs/deudas-post-5b.md`**:
- Bug del fmt: marcado CERRADO en los 3 lugares donde se mencionaba.
- Cap "Package manager" en la guía: marcado CERRADO.
- `docs/architecture.md` refresh: marcado CERRADO.
- Walk completo de guide.md: parcialmente CERRADO (caps stale
  refrescados; pueden quedar referencias menores).

Próximo norte: Fase 9.w (Stack web first-class: `@authenticated`,
`@ws`, `@cron`, `@background`).

Sub-paso separado pendiente sin presión: bundling CPython embebido
(`fitz build --bundle-python`).

## [v0.9.19] — 2026-05-17 — Fase 9.z.5 CERRADA + cierre Fase 9.z entera — `fitz lint`

Quinta y última DX feature de Fase 9.z. Linter de patrones más
allá de tipos. **Cierra Fase 9.z entera** — los 5 sub-pasos (fmt
+ test + dev + repl + lint) cerrados en 2 días (16-17 de mayo).

**Implementación**:

- Módulo nuevo `src/lint.rs` (~700 LoC incluyendo 15 unit tests):
  framework `LintFinding` con `name`/`message`/`line`/`column`/
  `hint`/`fix` opcional, walkers `collect_uses_in_*` y
  `walk_exprs_in_stmt` para visit recursivo del AST, supresión
  via inspección del source raw.
- **4 lints**:
  - `unused_variable`: detecta `let x = ...` (target Ident) cuyo
    nombre no aparece en `Expr::Ident` del programa. Skipea
    prefijo `_` (convención "intencional"). Walkea fns, while,
    loop, for. Params de fn NO se flaguean en MVP (típicamente
    handlers HTTP / callbacks reciben params no usados, sería
    ruido).
  - `unused_import`: `import X` y `from X import Y` cuyo binding
    no se referencia. Maneja alias (`import foo as f` → binding
    `f`).
  - `useless_match`: `match expr { _ => body }` con UN solo arm
    catch-all (Wildcard o Ident binding) = equivalente a un
    `let`. NO flaguea matches con múltiples arms.
  - `string_concat`: `Expr::BinOp { op: Add, left, right }` con
    AMBOS operandos `Expr::Str` literales. Sugiere interpolación.
    Concat con var queda OK (puede ser intencional).
- **Lints skipeados del roadmap original**:
  - `panic_in_test_only`: NO aplica — Fitz no tiene `panic!`
    builtin distinguido (los asserts son builtins normales).
  - `redundant_clone`: requiere análisis de movimientos que el
    compilador no hace.
- `Commands::Lint { files, deny }` en CLI:
  - Sin args: manifest mode, descubre `.fitz` del proyecto via
    `discover_project_fitz_files` (heredado de `fitz fmt`).
  - Con archivos: lintea solo esos.
  - `--deny <name>` (repetible): trata ese lint como error
    (exit 1 si aparece).
- **Output cargo-clippy style**: `warning:` amarillo / `error:`
  rojo con `--deny`, `--> <file>:<line>:<col>`, hint con `= nota:`,
  summary final con conteo de findings + denied. ANSI colors auto
  via `std::io::IsTerminal`.
- **Supresión**: `// @allow(<lint>)` en la línea inmediatamente
  anterior al stmt offending. Lookup directo sobre el source raw
  (no trivia stream del lexer): pragmático y suficiente.
- **Default exit code**: 0 con findings normales (warnings no
  rompen build). Exit 1 si: error de lectura de archivo, parse
  error, o `--deny` matchea algún finding.

**Decisiones tomadas**:

- 4 lints en el MVP (no 6 del roadmap original).
- Auto-fix (`--fix`) **diferido** a sub-paso futuro: todos los
  lints emiten sugerencias textuales pero no modifican código.
  `string_concat` es el candidato natural a auto-fix.
- Supresión solo en la línea INMEDIATAMENTE anterior (no
  multi-línea, no inline).
- Análisis de uses globales (no scope-aware estricto): shadowing
  (`let x = 5; let x = 10; x`) no se detecta. Refinamiento si
  aparece presión.
- Catálogo cerrado (sin plugins externos).
- Lints emiten warnings por default; CI usa `--deny <name>`.

**Tests**:
- 15 unit tests en `src/lint.rs::tests`: 1 caso por lint en
  forma básica (smoke), 1 con `_var` ignorado, 1 con uso real
  no flagueado, 1 supresión con `@allow` funciona, 1 supresión
  solo aplica a línea inmediata anterior, 1 con fn body, 1 con
  alias en imports, 1 con dos arms (no flaguea), 1 programa
  limpio (cero findings), 1 ordenamiento por línea+columna.
- 7 cli_e2e nuevos: detecta unused_variable + unused_import,
  `--deny` exit 1, suppression silencia, archivo inexistente
  exit 1, string_concat detecta literales, código limpio cero
  findings, useless_match detecta un-solo-arm.
- **Total al cierre 9.z.5**: 1381 unit (+15) / 73 cli_e2e (+7)
  / 79 compile_e2e / 3 openapi. Clippy `-D warnings` limpio.

**Cap 27 nuevo "`fitz lint` — linter de patrones"** en
`docs/guide.md`: los 4 lints con tabla, CLI, supresión, output
cargo-clippy, integración con CI, limitaciones (sin auto-fix,
sin plugins, sin shadowing detection). Renumeración cap 27→28
("Qué sigue"). Bullet sumado en "Lo que ya sabés" + "DX 9.z"
del cap 28 marca la fase entera como CERRADA.

**Cierre formal de Fase 9.z entera**: los 5 sub-pasos cerrados.
Próximo norte: Fase 9.w (stack web first-class: `@authenticated`,
`@ws`, `@cron`, `@background`) o sub-paso dedicado de refresh
masivo de docs (cap "Package manager" + `architecture.md` +
walk completo de la guía).

**Deudas residuales de 9.z.5 (NO bloquean siguientes pasos)**:
- Auto-fix `--fix` (especialmente para `string_concat`).
- Lints adicionales si aparece demanda (`shadowing`,
  `useless_clone` cuando el compilador haga análisis de
  movimientos, etc.).
- `unused_variable` scope-aware estricto (shadowing detection).
- Suppression cross-line (`// @allow(name) { ... }` bloque).
- Plugins externos para catálogo extensible.

## [v0.9.18] — 2026-05-17 — Fase 9.z.4 CERRADA — `fitz repl` (REPL interactivo)

Cuarta DX feature de Fase 9.z. Prompt interactivo donde cada línea
se evalúa contra un env compartido, con multi-line continuation,
comandos especiales `:nombre`, history persistente, y async
transparente.

**Implementación**:

- Dep nueva: `rustyline = "14"` para terminal handling
  (arrow keys, history Ctrl-R, line editing, Ctrl+C/D
  diferenciados). Mismo crate que cargo-edit. Default features
  traen file history.
- `Commands::Repl` (sin args) en CLI. Manifest mode/single-file
  no aplica — el REPL es siempre single-session.
- `repl_cmd` corre adentro de `evaluator::build_runtime()`
  (current_thread) para que `sleep(100).await` y similares
  funcionen desde el prompt.
- `read_complete_input` lee líneas hasta que el buffer esté
  "completo" según heurística de balanced brackets
  (`input_is_complete`): cuenta `{`/`(`/`[` skip-eando strings
  literales y comments `//` y `/* */`. Si no balancea, prompt
  cambia a `... `. Es heurística (no parser real); el parser
  puede aún emitir un error sintáctico distinto que se muestra
  y vuelve al prompt.
- 6 comandos especiales (`handle_special_command`): `:help`,
  `:quit`/`:q`/`:exit`, `:env`, `:reset`, `:type <expr>`,
  `:load <archivo>`.
- `:env` lista los bindings del scope raíz filtrando builtins
  (`evaluator::builtin_names()` — array nuevo con los 8
  builtins actuales).
- `:type <expr>` arma un programa sintético
  `let __repl_type = <expr>`, lo pasa por el checker, y lee el
  tipo del span del value. Limitación conocida: no es
  scope-aware (no ve vars previas del REPL). Documentado.
- `:load <archivo>` lee + parsea + chequea + evalúa el archivo
  contra el env del REPL. Los `let`/`fn` del archivo quedan
  disponibles para las próximas líneas del prompt.
- History persistente: `~/.fitz/history` (Linux/macOS) o
  `%USERPROFILE%\.fitz\history` (Windows). Se carga al inicio y
  se guarda al salir. `rustyline` maneja arrow up/down + Ctrl+R
  + line editing nativo.
- **Pretty-print Python-style del último valor**: cuando el
  último stmt del input es `Stmt::Expr` y devuelve un `Value`
  no-Null, se imprime con `= <value>`. Para `let`/`fn`/`print`
  el output es silencioso (`print` devuelve Null y ya imprime
  por su cuenta).

**APIs nuevas en el evaluator/env** (pub):
- `evaluator::eval_program_with_env(program, base_dir, env,
  dep_registry) -> FitzResult<Value>`: evalúa contra un env
  externo que persiste entre invocaciones (a diferencia de
  `eval_with_base_and_deps`). Devuelve el `Value` del último
  stmt para que el REPL pueda imprimir.
- `evaluator::new_repl_env() -> EnvRef`: wrapper público que
  crea env + registra builtins, sin exponer la fn privada.
- `evaluator::builtin_names() -> &'static [&'static str]`:
  lista de nombres de builtins para que el REPL los filtre del
  `:env`. Mantener sincronizado con `register_builtins`.
- `Environment::local_names() -> Vec<String>`: lista los nombres
  definidos en el scope actual (sin recursar al padre).

**Decisiones tomadas**:

- Filtrar warning spurio del checker para "variable desconocida"
  por **substring del mensaje** (no kind): todos los errores del
  checker llevan `ErrorKind::TypeError` (`UndefinedVariable` es
  kind del evaluator), y el string "variable desconocida" está
  estable en `types::infer_expr`. Sin el filtro, cada `let x =
  5; x + 1` emitía warning spurio del checker para `x` en la
  segunda línea (el checker arma scope desde cero por
  invocación).
- `:type` scope-aware: NO en MVP. Refinable feedeando el env del
  REPL al checker como pre-declaraciones — sub-paso futuro si
  aparece presión real.
- `panic(msg)` u otros builtins extras: NO en MVP. Lista
  oficial es la de 9.z.2 (4 asserts).
- Smoke E2E automatizado: NO — el REPL es interactivo, los tests
  serían flaky. Smoke manual con stdin scripted valida.

**Smoke manual validado**:
- `1 + 2` → `= 3`
- `let x = 5; x + 1` → `= 6` (sin warnings spurios)
- `fn doble(n: Int) -> Int { return n * 2 }; doble(21)` → `= 42`
- `async fn pausa() -> Int { return 42 }; pausa().await` → `= 42`
- `:env` lista user-defined vars + filtra builtins
- `:reset` limpia scope
- `:load <archivo.fitz>` carga + define todo en el env actual
- typo real (`xyz_typo`) → error claro del evaluator
- Multi-line con `{` abierto cambia prompt a `... `

**Cap 26 nuevo "`fitz repl` — REPL interactivo"** en
`docs/guide.md`: features, comandos especiales con tabla, history
persistente, async, decisión "expresiones vs statements",
limitaciones (`:type` no scope-aware, no manifest mode, sin
auto-completion de paths). Renumeración cap 26→27 ("Qué sigue").

**Cierre formal**:
- CHANGELOG v0.9.18.
- `docs/roadmap.md`: 9.z.4 marcado CERRADO con detalle.
- `docs/deudas-post-5b.md`: bloque "Fase 9.z.4 CERRADA" +
  deudas residuales (`:type` scope-aware, smoke E2E, etc.).
- README.md: bloque 9.z.4 + conteo final.
- CLAUDE.md: bloque "Próximo norte" actualizado.
- `docs/syntax-spec.md`: `fitz repl` cae adentro de "implementado".

**Tests al cierre**:
- 1366 unit / 66 cli_e2e / 79 compile_e2e / 3 openapi (sin cambios
  — repl_cmd es interactivo, smoke E2E automatizado pendiente).
- Clippy `-D warnings` limpio.

**Deudas residuales (NO bloquean 9.z.5)**:
- `:type` scope-aware (no ve vars previas del REPL).
- Smoke E2E automatizado del REPL (file watchers + readline son
  flaky en tests; el smoke manual con stdin scripted cubre el
  caso 90%).
- Indentación automática en multi-line continuation.
- Comandos `:save`/`:undo`/`:debug` si aparece demanda.
- Auto-completion de paths en `:load`.
- Manifest mode en `fitz repl` (hoy es single-session siempre).

## [v0.9.17] — 2026-05-17 — Fase 9.z.3 CERRADA — `fitz dev` (hot reload)

Modo desarrollo con file watcher + kill/respawn al detectar cambio.
Tercera DX feature de Fase 9.z. El loop iterativo del developer
(editar → save → ver efecto) sin re-tipear `fitz run` en cada save.

**Implementación**:
- Dep nueva: `notify = "6"` (file watcher cross-platform: FSEvents
  en macOS, inotify en Linux, ReadDirectoryChangesW en Windows).
  Sin layer de debouncer — el debounce 100ms lo hacemos manual con
  un `tokio::time::timeout` + drain del canal en el loop.
- `Commands::Dev { file }` en CLI. Sin args, manifest mode
  (busca `fitz.toml`, watch su dir, corre `fitz run`). Con
  `--file <archivo.fitz>`, single-file mode (watch parent del
  archivo).
- `dev_cmd` corre adentro de un runtime tokio current_thread
  (reusa `evaluator::build_runtime`). El loop principal
  `run_dev_loop` usa `tokio::select!` sobre 3 eventos: cambio
  detectado por el watcher, exit del child, o `tokio::signal::ctrl_c()`.
- Bridge sync→async para `notify`: un `std::thread::spawn` lee del
  `std::sync::mpsc` (sync) y re-envía al `tokio::sync::mpsc`
  (async). El watcher es sync; este patrón evita feature
  `tokio` del crate `notify` para no inflar el dep tree.
- Spawn del child con `tokio::process::Command`: `current_exe()`
  + `target.child_args` + `current_dir(&target.watch_dir)` para
  que `fitz run` (manifest mode, sin args) encuentre el
  `fitz.toml` correcto. Single-file mode usa path absoluto del
  archivo, así el cwd no importa.
- **Path filtering** (`path_is_relevant`): sólo `*.fitz` y
  `fitz.toml`. Excluye en cualquier nivel `target/`, `.git/`,
  `node_modules/`, `.fitz/`, `dist/`, `build/`, y cualquier
  componente oculto (`.algo`).
- **Debounce 100ms**: tras detectar un evento, drain del canal
  con `tokio::time::timeout` para colapsar saves múltiples del
  editor (VSCode emite write tmp + rename + chmod en un save).
- **Banner UX** (`clear_screen_and_banner`): `\x1b[2J\x1b[H` para
  clear+home si stdout es TTY (`std::io::IsTerminal`), sino
  separa con líneas. Cada arranque muestra "▶ fitz dev (run #N)
  — <target>".
- **Ctrl+C**: `tokio::signal::ctrl_c()` en el `select!` mata el
  child + waits antes de retornar. Sin esto, en uso real
  quedarían procesos zombie del child.
- Caso "child terminó solo" (programa CLI corto, error de tipo):
  no salimos del loop — esperamos un cambio en filesystem para
  reiniciar. Pedagógicamente útil: el user fixea el error, save,
  retry automático.

**Decisiones tomadas**:
- `[dev]` config en `fitz.toml` para customizar paths watched /
  debounce / etc.: NO en MVP, solo defaults. Sumar si aparece
  demanda concreta.
- Browser auto-refresh para HTTP: NO en MVP. Quien edite HTML/CSS
  junto puede usar Live Server o similar.
- Print de errors del checker mientras tipeás sin disparar
  restart: NO — el child mismo imprime los errores en arranque.
  El LSP (cap 22) ya hace diagnostics in-editor para feedback
  continuo.
- `fitz dev --test` (modo "watch + run tests"): sub-paso futuro
  si aparece presión. Workaround documentado en el cap 25
  con dos terminales.

**Tests**:
- Smoke manual validado: arrancar `fitz dev --file`, modificar
  archivo, observar run #2 con código nuevo. ANSI clear screen
  + banner funcionando.
- 1366 unit / 66 cli_e2e / 79 compile_e2e / 3 openapi (sin
  cambios — el dev_cmd es interactivo, los tests automáticos
  serían flaky). Clippy `-D warnings` limpio.

**Bug fix colateral**: en el smoke confirmé que el child del
`fitz dev --file` re-evalúa el archivo modificado correctamente
(no hay cache stale).

**Deudas residuales (NO bloquean 9.z.4)**:
- Incremental rebuild (solo el archivo cambiado se re-carga):
  hoy es kill+respawn full. Mejora futura cuando aparezca
  modelo de módulos pre-compilados.
- Filtrar "modify sin cambio real" (timestamps tocados sin
  cambio de contenido): hoy cualquier evento `Modify` dispara.
  Refinable comparando hashes si duele.
- Auto-test mode (`fitz dev --test`): workaround documentado
  con dos terminales.
- Smoke E2E automatizado: por interactividad del dev_cmd y
  flakeyness de los file watchers, los tests son manuales por
  ahora.

**Cap 25 nuevo "`fitz dev` — hot reload"** en `docs/guide.md`:
features, CLI single-file/manifest, qué dispara restart, output
típico, limitaciones, integración con `fitz test`. Renumeración
cap 25→26 ("Qué sigue").

**Cierre formal**: CHANGELOG v0.9.17, roadmap (9.z.3 CERRADA con
detalle), `docs/deudas-post-5b.md` (bloque "Fase 9.z.3 CERRADA"),
README, CLAUDE, `docs/syntax-spec.md` (nota implementado).

## [v0.9.16] — 2026-05-17 — Fase 9.z.2 entera CERRADA — `fitz test` (testing built-in)

Test runner integrado al lenguaje. Sin librerías, sin glue, sin
elegir entre 3 frameworks. Tres sub-pasos (a + b + c) cerrados en
el día:

**9.z.2.a — `@test` decorator + assertion builtins + TestRegistry**:
- `src/testing.rs` nuevo: `TestRegistry` + thread-local +
  `with_active_test_registry` (sync/async). Mirror chico de
  `http::HTTP_REGISTRY` con la asimetría clave: sin registry
  activo, `@test` es no-op silencioso (paralelo a `#[cfg(test)]`).
- Evaluator: branch `@test` en `process_decorator` con `register_test`.
  Valida args/kwargs/params vacíos; empuja `TestSpec` si hay registry.
- 4 assertion builtins: `assert(cond, msg?)`, `assert_eq(a, b)`,
  `assert_ne(a, b)`, `assert_throws(fn)`. Estilo cargo
  (`left`/`right`). `assert_throws` con callback async: rechazado
  en MVP — caso especial en `invoke_value` invoca async-recursive.
- Pre-registro en checker (`types.rs`) + completion en LSP (`lsp.rs`).
- **Cambio retro-compatible al parser**: paréntesis opcionales en
  decoradores (necesario para `@test fn ...`). Los demás
  decorators siguen funcionando idéntico con/sin paréntesis.

**9.z.2.b — `fitz test` runner**:
- `Commands::Test { filter, file }` en CLI.
- **Single-file mode** (`fitz test --file archivo.fitz`): carga
  el archivo, descubre `@test`, los corre.
- **Manifest mode** (`fitz test`): discovery automático. Si hay
  `tests/*.fitz` top-level: solo carga esos (el `[lib]` se carga
  vía import auto-self-registrado bajo `package.name` —
  paralelo a `use my_crate::*` Rust). Si no hay tests integration:
  carga el `[lib].entry` directo para tests inline.
- Filtrado por **substring** del nombre del test (cargo default).
- Output estilo cargo: `test <file>::<name> ... ok/FAILED` +
  sección `failures:` con detalle + summary `test result: ...
  passed; ... failed; finished in ...s`. ANSI colors auto cuando
  stdout es TTY (`std::io::IsTerminal`, cero deps nuevas).
- **Async tests** funcionan: `evaluator::run_test_handler`
  encapsula invoke + await del `Future`.
- Exit code 1 si ≥1 falla, 0 si todos pasan.
- Loader sobrescribe `CURRENT_TEST_SOURCE` al cargar módulos
  importados: los `@test` quedan etiquetados con su archivo
  declarante real (no con el del importer).
- Dedup en discovery: si hay tests integration, no se carga
  `[lib]` direct para evitar duplicar tests inline del lib que
  los tests importan.

**9.z.2.c — guía + ejemplo + cierre formal**:
- Cap 24 nuevo **"`fitz test` — testing built-in"** en
  `docs/guide.md`: features, CLI single-file / manifest mode,
  filtrado, output cargo-style, async tests, estructura típica
  de proyecto, limitaciones. Renumeración cap 24→25 ("Qué sigue").
- Ejemplo runnable `examples/guide/24-tests.fitz` con `factorial`
  + 3 tests OK + 1 FAILED intencional. Sumado al smoke
  `GUIDE_EXAMPLES_COMPILE` (compila con `fitz build` porque
  codegen ignora `@test`).
- Codegen: `@test fn` se **ignora silenciosamente** en `fitz build`
  (paralelo a `#[cfg(test)]`). Bug fix colateral en
  `has_http_routes` (counting `@test` como HTTP disparaba
  servidor en CLI puro — refinado a solo
  `get`/`post`/`put`/`delete`/`server`).
- CHANGELOG v0.9.16, roadmap (9.z.2 a/b/c marcado CERRADO),
  `docs/deudas-post-5b.md` (bloque "Fase 9.z.2 entera CERRADA"),
  README, CLAUDE, `docs/syntax-spec.md` (sección "Testing"
  pasa de "futuro" a "implementado").

**Decisiones tomadas durante 9.z.2**:
- `panic(msg)` (que el syntax-spec usa en su ejemplo) **fuera de
  scope** del MVP. Los 4 oficiales bastan; refinable si aparece
  presión.
- `assert_throws` solo SYNC callbacks. Async cb queda como sub-paso
  futuro si aparece presión.
- Discovery dedup pragmática: lib vs tests integration.
- Auto-self-import bajo `package.name`: requiere nombre usable
  como ident Fitz (sin hyphens). Deuda visible.

**Tests al cierre**:
- 1366 unit (+33 vs Fase 9.z.1) — `+6 testing`, `+25 evaluator
  (decorator + asserts)`, `+2 parser regression`.
- 66 cli_e2e (+11 vs Fase 9.z.1) — runner end-to-end.
- 79 compile_e2e (igual cuenta que 9.z.1; `24-tests.fitz` se sumó
  a la lista del smoke `GUIDE_EXAMPLES_COMPILE` que es 1 `#[test]`
  único iterando, no a tests individuales).
- 3 openapi.
- Clippy `-D warnings` limpio.

## [v0.9.14] — 2026-05-16 — Fase 9.z.1.b + cierre de 9.z.1 entera: comment + blank preservation

Cierra la deuda crítica de 9.z.1.a: el formatter ahora **preserva
comentarios y blank lines del usuario** al reescribir archivos.
`fitz fmt` es production-ready — el warning loud del modo write
fue removido. **Cierra 9.z.1 entera** (a + b).

Lexer:
- `Trivia` struct nueva: `Vec<Comment>` (con `kind: Line | Block`,
  `text`, `line`, `column`) + `Vec<usize>` con líneas blank.
- `tokenize_with_trivia(src) -> (Vec<TokenWithPos>, Trivia)`
  paralela a `tokenize` (que sigue zero-overhead — parser/LSP/
  resto no se ven afectados). AST sin cambios.
- `Lexer.collect_trivia` flag + `line_had_code` /
  `line_had_comment` para distinguir líneas blank (sin nada) de
  líneas comment-only (no son blank).

Formatter:
- `format_source` ahora invoca `tokenize_with_trivia` y threadea
  la trivia en el output.
- `fmt_stmt_list` emit leading comments + blank lines preservadas
  + trailing comments por stmt.
- `end_line_of_stmt`/`end_line_of_expr` recursivos para detectar
  trailing comments en stmts multi-línea.
- Smart blank entre fn/type defs **suprimida** si hay leading
  comment recién emitido (el comment se ata al stmt siguiente).
- Comments normalizados: `//foo` → `// foo` (espacio post-`//`).
- Trailing comments emitidos con 2 espacios de separación.
- Múltiples blank lines consecutivas colapsadas a 1.

Decisiones cerradas: lexer side-stream vs token kind (lean
side-stream porque parser no se contamina); fmt_stmt_list con
`in_block` flag (blocks no emiten footer comments — caso raro de
"comment entre último stmt y `}`" es deuda menor documentada);
smart blank suprimida por leading comment.

CLI:
- Removido el warning loud del modo write (deuda 9.z.1.a cerrada).
- Docstring de `Commands::Fmt` reescrita reflejando
  production-ready.

Limitaciones residuales (NO bloquean 9.z.2):
- Comments entre último stmt de un bloque y el `}` terminan
  saliendo del bloque al re-formatear (caso raro).
- Multi-línea de listas/maps/method chains se colapsa a
  single-line (auto-wrap line-aware es deuda futura).
- Comments adentro de expresiones (`f(x, // foo\n y)`) no
  soportados.

- 8 unit tests nuevos en `lexer::tests` (trivia capture, blank
  detection, comment-only lines, mixto).
- 10 unit tests nuevos en `fmt::tests` (preservación de leading/
  trailing/blanks/multiline, normalización de espacios,
  idempotencia con comments, smoke con 02-hola).
- 2 cli_e2e nuevos / actualizados.
- Total: 1333 unit + 55 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

Smoke a mano: `examples/guide/02-hola.fitz` round-trip exacto
bit-a-bit (2 comments + 2 blank lines preservados).

Ver `docs/fmt-style.md` para la referencia completa de
convenciones del formatter.

## [v0.9.13] — 2026-05-16 — Fase 9.z.1.a: `fitz fmt` (sin comment preservation)

Primer slice del formatter. Pretty-printer escrito a mano sobre
el AST, cero config (4 espacios indent, comillas dobles, blank
line solo entre fn/type top-level consecutivos). Cubre >20 nodos
del AST: literales, let, fn (con/sin async/decorators),
if/while/for/loop, match, struct lit, list/map, BinOp/UnaryOp,
Call/Field/Index, Range, Ok/Err/Try/Await, FnExpr (preserva
flecha si body es Return único), TypeDef con defaults, Decorator,
Import/FromImport.

**⚠ LIMITACIÓN CRÍTICA** — el lexer strippea comentarios antes
de llegar al AST. Modo write (`fitz fmt`) borra comments y blank
lines del usuario. Modo `--check` (read-only) es safe. Para
hacer al formatter usable en código real, comment preservation
llega en **9.z.1.b** (lexer side stream + parser side-table +
threading en el formatter). Mientras tanto, el CLI emite warning
loud explicando la pérdida + sugiriendo `--check`.

CLI:
- `fitz fmt <files...>` — formatea archivos explícitos.
- `fitz fmt` (sin args) — descubre `.fitz` del proyecto via
  manifest (walk recursivo de `src/`).
- `fitz fmt --check` — modo CI, read-only, exit 1 si hay diffs.

Decisiones cerradas: indent 4 espacios, comillas dobles, sin
auto-wrap de líneas largas (deuda futura); `is_let` recuperado
del source via Span (AST no preserva `let x = ...` vs `x = ...`);
`fn f() => expr` se normaliza a bloque (AST no preserva flecha
en defs); `if` con paréntesis obligatorios en condición;
warning loud solo en write mode (`--check` silencioso).

- 21 unit tests nuevos en `fmt::tests` (incl. idempotencia
  sobre programas complejos).
- 7 E2E nuevos en `tests/cli_e2e.rs` (file/check/sin args/error
  de sintaxis/warning emission/project discovery).
- Total: 1315 unit + 55 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.12] — 2026-05-16 — Fase 9.y.4: `fitz add` / `fitz remove` / `fitz update`

Cuarto sub-paso del package manager. Automatiza la edición del
manifest + lockfile que hasta 9.y.3 era manual. Tres subcomandos
nuevos con UX cargo-style. Hoy editás el `fitz.toml` con un
comando, no a mano.

- `fitz add <name> --path <p>` — agrega path dep.
- `fitz add <name> --git <url> --tag <t>` (o `--rev <r>`) —
  agrega git dep. clap valida conflicts entre `path`/`git` y
  entre `tag`/`rev`.
- `fitz add <name>` sin flags — error claro citando 9.y.5
  (registry futuro).
- `fitz remove <name>` — quita entry + sync lockfile. Si la dep
  era la única, borra `fitz.lock` entero (deps vacías).
- `fitz update [name]` — invalida cache de git deps (force
  re-clone). Path deps son no-op (siempre fresh). Sin name
  actualiza todas; con name solo esa (error si no existe).

Decisiones cerradas: dep nueva `toml_edit = "0.22"` (preserva
comentarios + formatting al modificar `fitz.toml`); persist eager
incluso si la resolución posterior falla (cargo-style, usuario
revierte con `fitz remove`); validación cruzada delegada a clap
(`conflicts_with` + `requires`) — mensajes limpios sin código
custom; `fitz add` sobreescribe sin warning si la dep existía;
`fitz remove` borra `fitz.lock` cuando deps queda vacío para no
dejar stale state; `fitz update no-existe` da error claro (no
silent no-op); dev deps `[dev-dependencies]` diferidas a 9.z.2.

- 11 unit tests nuevos en `manifest::tests` (add path/git,
  sobreescribe, sin `[dependencies]`, preserva comentarios;
  remove existente/inexistente/borra sección vacía;
  add+remove inversa).
- 11 E2E tests nuevos en `tests/cli_e2e.rs` cubriendo todos los
  caminos del CLI + errores (sin flags, sin tag/rev, conflicts,
  fuera de proyecto, dep inexistente, cache busted con marker
  file).
- Total: 1294 unit + 48 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.11] — 2026-05-16 — Fase 9.y.3.c + cierre de 9.y.3 entera: git deps + cache local

Tercer y último slice del tercer sub-paso del package manager.
Habilita `[dependencies] foo = { git = "https://...", tag = "v1.0.0" }`
en `fitz.toml`. El primer acceso clona el repo a `<cache>/git/
<sanitized-url>@<ref>/` (cache global, default `~/.fitz/cache/`,
override con `FITZ_CACHE_DIR`) y reusa el dir en accesos
siguientes — sin re-clone automático. El lockfile registra el
commit hash exacto Cargo-style: `source = "git+<url>#<commit>"`.

**Cierra 9.y.3 entera**: path deps (a) + loader integration (b)
+ git deps (c) están todos vivos. El package manager Fitz puede
hoy declarar, resolver, bloquear y CONSUMIR deps tanto locales
como de repos git remotos, sin registry todavía. Próximo norte:
9.y.4 (`fitz add`/`remove`/`update`).

Decisiones cerradas: subprocess `git` sobre crate (zero deps);
`tag` XOR `rev` mutuamente exclusivos; `branch` NO soportado
intencionalmente (no reproducible); cache naming determinístico
sin hashing (`github.com_foo_bar@v1.0.0/`, trunca a 200 chars);
cache reuse sin re-clone automático; estrategia split (`--depth 1
--branch <tag>` para tags, full clone + checkout para revs porque
git no acepta SHAs en `--branch`); `FITZ_CACHE_DIR` env var
override para tests E2E aislados.

Validaciones cruzadas con mensajes accionables: `path` + `git`,
`tag` + `rev` juntos, `tag`/`rev` sin `git`, `git` sin `tag`/`rev`
(cita reproducibilidad), `tag`/`rev` vacíos.

Smoke end-to-end: `myutils` con `[lib]` + git repo + tag `v0.1.0`;
`myapp` con `[dependencies] myutils = { git = "file:///...", tag
= "v0.1.0" }`. `fitz run` clona, lockfile correcto, output ok;
segunda corrida sin re-clone (verificado con marker file); `fitz
build` produce binario ejecutable bit-a-bit idéntico.

- 8 unit tests nuevos en `git_dep::tests` (sanitize_url,
  cache_path_for, lockfile_source_string, GitRef shape).
- 6 unit tests nuevos en `manifest::tests` (parse_git_ref +
  validaciones de shape: sin tag/rev, tag+rev juntos, tag vacío,
  path+git, tag sin git).
- 4 E2E tests nuevos en `tests/cli_e2e.rs` con bare git repo
  local + `FITZ_CACHE_DIR` aislado.
- Total: 1283 unit + 37 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

Deuda residual visible (NO bloquea 9.y.4): drift entre lockfile
commit y cache borrado (re-clone fresh no detecta si upstream
movió el tag); `fitz cache clean` sub-comando (borrar cache es
manual hoy); auth para repos privados (delegado al git del
sistema); shallow clone con `--filter` para revs (optimización
de performance); verificación de integridad (commit signature).

## [v0.9.10] — 2026-05-16 — Fase 9.y.3.b: loader integration (deps usables desde código)

Segundo slice del tercer sub-paso del package manager. El loader
del evaluator (`fitz run`) y el del codegen (`fitz build`) consultan
ahora el `dep_registry` resuelto del manifest ANTES de fallback a
paths relativos del importer. `from <dep-name> import X` resuelve
al `lib_entry` absoluto de la dep — las deps declaradas en 9.y.3.a
son finalmente **usables desde código**.

Smoke end-to-end: con un proyecto `myutils` (con `[lib] entry =
"src/lib.fitz"` exponiendo `double`/`greet`) y un proyecto `myapp`
con `[dependencies] myutils = { path = "../myutils" }`, el código
`from myutils import double, greet` en `myapp/src/main.fitz`
funciona tanto en `fitz run` como en `fitz build`, produciendo el
output esperado bit-a-bit.

Decisiones cerradas: `DepRegistry` como `HashMap<String, PathBuf>`
alias en `manifest.rs`; resolución con shortcut single-segment +
fallback path-relativo (paralelo en evaluator y codegen); deps
shadowean archivos locales con el mismo nombre; transitive deps
(deps de deps) NO soportadas en este slice (refactor mayor, deuda
futura); hyphens en dep names aceptados al parse pero no
importables porque el parser Fitz no acepta `-` en identifiers
(deuda 9.y.4 para auto-translation); `fitz check` no consume el
dep_registry (los nombres importados se tipan como Any/nominal
placeholder, validación real ocurre en run/build).

API del evaluator: `eval_with_base_and_deps(_sync)` nuevas pub APIs;
`eval_with_base(_sync)` quedan como wrappers con registry vacío
(backward compat para callers sin manifest awareness).

- 5 E2E nuevos en `tests/cli_e2e.rs` (deps en run + build, no
  ref no falla, fallback path-relativo, dep shadowea local).
- Total: 1270 unit + 33 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.9] — 2026-05-16 — Fase 9.y.3.a: path deps + sección `[lib]` + `fitz.lock`

Primer slice del tercer sub-paso del package manager. Habilita
declarar `[dependencies] foo = { path = "../foo" }` en el manifest;
el `fitz.lock` se emite/sincroniza automáticamente en cada
`fitz run`/`build`/`check` (manifest mode). **NO toca el loader
del lenguaje** todavía — las deps quedan declaradas y bloqueadas
en el lockfile pero `from foo import X` no las resuelve aún. Esa
promesa es 9.y.3.b.

Sintaxis: `[dependencies] utils-lib = { path = "../utils-lib" }`
en el importer + sección nueva `[lib] entry = "src/lib.fitz"` en
la dep (paralelo a `[bin] main`). Path deps son librerías por
definición — si la dep solo tiene `[bin]`, el resolver aborta
con la sección `[lib]` sugerida inline.

`fitz.lock` formato TOML Cargo-style: `version = 1` + `[[package]]`
con `name`/`version`, sin campo `source` para path deps (convención
Cargo: implícitas). El lockfile se regenera idempotentemente —
sin cambios = sin escritura (no spam de mtime).

Decisiones cerradas: lockfile TOML, `Dependency` enum
`Version(String) | Detailed(...)` con `serde(untagged)`,
`Lib.entry` obligatorio sin defaults mágicos, path deps son libs
por definición, lockfile siempre regenerado idempotente, sin
emisión si no hay deps. Versiones sueltas (`foo = "1.0.0"`) y
git deps se aceptan al parse pero el resolver las rechaza con
errores accionables citando 9.y.5 (registry) y 9.y.3.c (git)
respectivamente.

- 10 unit tests nuevos en `manifest::tests` (Dependency parse
  forms, Lib parse, resolve_dependencies happy + 5 error paths).
- 14 unit tests nuevos en `lockfile::tests` (parse/serialize/
  round-trip, from_resolved ordering, idempotencia de write).
- 8 E2E tests nuevos en `tests/cli_e2e.rs` (lockfile emitido,
  idempotencia, regen en cambio de versión, sin deps no emite,
  errores: version/git/path inexistente/sin `[lib]`).
- Total: 1270 unit + 28 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.8] — 2026-05-16 — Fase 9.y.2: `fitz run`/`build`/`check` leen el manifest

Segundo sub-paso del package manager. Sin archivo explícito, los
tres subcomandos detectan `fitz.toml` en el cwd o ancestros
(Cargo-style) y usan `[bin].main` como entry point. En manifest
mode, `fitz build` emite el binario a
`<manifest_dir>/target/release/<pkg-name>(.exe)` con el nombre del
paquete (no el stem del fuente).

**Sin breaking**: los ejemplos de la guía siguen corriendo
idénticos con `fitz run examples/guide/02-hola.fitz`. Los 79 tests
de `compile_e2e` (single-file mode) verdes sin cambio.

Decisiones cerradas: `target/release/<pkg-name>(.exe)` adyacente
al manifest hardcodeado (configurable post-MVP), `fitz check`
chequea solo el `[bin].main` (loader walks imports
transitivamente), compat single-file silenciosa sin warning,
manifest sin `[bin]` aborta con la sección sugerida inline,
multi-bin (`[[bin]]` array) sigue deuda 9.y.8+.

- 9 E2E tests nuevos en `tests/cli_e2e.rs`: run/check sin args,
  walk-up desde subdir, single-file mode compat, errores (sin
  manifest + sin archivo, sin `[bin]`, TOML corrupto), build sin
  args produce binario con pkg-name en `target/release/`.
- Total: 1246 unit + 20 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.7] — 2026-05-16 — Fase 9.y.1: manifest + `fitz new` / `fitz init`

Primer sub-paso del package manager (Fase 9.y). Define el formato
`fitz.toml` (TOML, Cargo-style) y suma dos subcomandos para crear
proyectos: `fitz new <name>` (carpeta nueva con `git init`
automático) y `fitz init` (directorio actual). Templates `--http`
(server con `@get`/`@server`) y default (`print` top-level estilo
cap 2 de la guía).

**Sin cambio breaking**: el modo single-file (`fitz run
archivo.fitz`) sigue funcionando idéntico. La integración del
manifest con `fitz run`/`build`/`check` llega en 9.y.2.

Decisiones cerradas: TOML para el manifest, `src/main.fitz` como
entry default, `edition = "2026"` (Cargo-style year), bin único
en MVP (multi-bin queda 9.y.8+), validación de nombre
`^[a-z][a-z0-9_-]{0,63}$` (política crates.io), `git init`
automático con flag `--no-git` para opt-out, `.gitignore` excluye
`target/` + binarios (no `fitz.lock` — el lockfile se commitea).

- 13 unit tests nuevos en `manifest::tests`.
- 11 E2E tests nuevos en `tests/cli_e2e.rs` cubriendo estructura
  completa, ambos templates, git init opt-in/out, errores
  (nombre inválido, carpeta ya existe, manifest existente).
- Total: 1246 unit + 11 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

Dep nueva no-opcional: `toml = "0.8"`.

## [v0.9.6] — 2026-05-16 — Fase 9.x.5: distribución VSCode multi-platform + logo

Quinta y última sub-fase visible del LSP **completa el plan LSP
entero**. Deja la extensión lista para publicar al VSCode Marketplace:
binarios pre-compilados por plataforma bundleados en el `.vsix`,
logo oficial del proyecto, script reproducible de build local.

La publicación real al Marketplace queda como acción del autor
(requiere cuenta de publisher + decisión sobre hacer el repo
público), no commit técnico.

Sub-pasos coordinados:

- **9.x.5.0 — Logo de Fitz**:
  - Diseño: engranaje estilo Rust (color naranja `#CE412B`, 12
    dientes) con silueta del monte Fitz Roy adentro del hueco
    (3 picos, central más alto, los dos laterales escalonados;
    confinada vía `clipPath` circular).
  - `assets/logo.svg` — single source of truth (256×256).
  - `assets/logo.png` — generado para README + propósitos generales.
  - `assets/logo-social.svg` + `.png` (1280×640) — Social preview
    de GitHub (se sube manual a Settings → Social preview).
  - `editors/vscode/icon.png` — copia para el .vsix de la extensión.
  - `editors/vscode/scripts/build-icon.mjs` — regenera los 3 PNGs
    desde los SVGs vía `@resvg/resvg-js` (puro JS bindings de
    resvg, Rust SVG renderer; más confiable que cairosvg en Windows).
  - `npm run build:icon` desde editors/vscode/ regenera todo.
  - `editors/vscode/package.json` declara `"icon": "icon.png"` →
    Marketplace usa el icon en el listing.
  - README raíz suma hero image centrada al inicio.

- **9.x.5.a — Extensión multi-platform aware + script `build-vsix`**:
  - `src/extension.ts` refactorizado con `resolveServerPath`
    siguiendo prioridad:
    (a) Override del user (`fitz.lspPath` ≠ default `"fitz-lsp"`)
        → respeta.
    (b) Bundled: busca `<extensionPath>/server/fitz-lsp[.exe]`
        (caso típico del .vsix de Marketplace).
    (c) Fallback al PATH del sistema (flujo alfa de 9.x.1.c —
        `cargo install` + setting default).
  - Helpers privados nuevos: `bundledBinaryPath`, `resolveUserPath`.
  - `scripts/build-vsix.mjs` orquesta: cargo build (con opcional
    `--target <triple>`) → copia binario a `server/` → tsc compile
    → `vsce package --target <vsce>` → produce `.vsix` con sufijo
    `-<platform>-<arch>`. Args: `--target <vsce>`, `--rust-target
    <triple>`. Default: plataforma actual via `process.platform`+
    `process.arch`. 6 plataformas soportadas con mapping a Rust
    triples: win32-x64/arm64, linux-x64/arm64, darwin-x64/arm64.
  - Estructura `editors/vscode/server/` con `.gitignore` que excluye
    los binarios (se regeneran cada build, no se versionan).
  - `.vscodeignore` actualizado para excluir `**/.gitignore` del
    .vsix final.
  - `activationEvents` removido del package.json (auto-derived por
    VSCode ≥1.74 desde `contributes.languages`).
  - `npm run build:vsix` desde editors/vscode/ corre todo.

**Decisiones técnicas tomadas al arrancar**:

- **Logo**: engranaje Rust + Fitz Roy (la inspiración del nombre
  del lenguaje + el lenguaje de implementación). Color `#CE412B`
  (Rust orange).
- **SVG single source of truth en `assets/`** (raíz del repo, no
  enterrado en `editors/vscode/`). Script regenera múltiples PNGs.
- **`@resvg/resvg-js` para SVG→PNG**: puro JS bindings, sin
  compilación nativa pesada, confiable en Windows. Alternativas
  rechazadas: `sharp` (compilación nativa), `cairosvg` Python
  (problemas en Windows).
- **Per-plataforma `.vsix`** (estándar rust-analyzer/Marketplace):
  un .vsix por target, cada uno con SU binario en `server/`.
  Alternativa rechazada: mega-.vsix con los 5 binarios (~50 MB).
- **Resolución del binario en orden** (override > bundled > PATH):
  backward-compatible con flujo 9.x.1.c.
- **`activationEvents` removido**: auto-derived por VSCode ≥1.74.
- **CI multi-platform y publicación al Marketplace fuera de scope**:
  acciones del autor (decisión sobre repo público, cuenta de
  publisher, PAT). Documentadas como pasos manuales en la guía.

**Cierre formal del plan LSP (9.x.1 → 9.x.5)**:

| Sub-fase | Feature | Cerrada |
|---|---|---|
| 9.x.1 | Diagnostics MVP + extensión VSCode base | 2026-05-15 |
| 9.x.2 | Hover (tipo del nodo bajo el cursor) | 2026-05-16 |
| 9.x.3 | Go-to-definition (uso → declaración) | 2026-05-16 |
| 9.x.4 | Autocomplete contextual (scope-level + after-dot) | 2026-05-16 |
| 9.x.5 | Distribución multi-platform + logo | 2026-05-16 |

El LSP MVP cubre la experiencia core de editing — diagnostics,
hover, go-to-def, autocomplete — más la infraestructura de
distribución. Lo que falta es decisión del autor (publicar) +
features avanzadas refinables post-MVP (rename, refactoring,
semantic highlighting, inlay hints).

**Acciones manuales pendientes del autor** (no commit técnico):

1. **GitHub Social Preview**: Settings → General → Social preview
   → upload `assets/logo-social.png`.
2. **Hacer el repo público** (cuando decida): pre-requisito para
   publicar al Marketplace + para que el Social Preview se
   renderice en link previews.
3. **Crear publisher en VSCode Marketplace**: Microsoft account +
   Azure DevOps + Personal Access Token.
4. **Publicar al Marketplace**: `vsce publish --packagePath
   editors/vscode/fitz-language-X.Y.Z-<target>.vsix` por cada
   plataforma.
5. **CI multi-platform** (opcional): GitHub Actions workflow con
   jobs Windows/macOS/Linux que corren `npm run build:vsix` y
   publican post release tag.

**Total al cierre**: 1233 unit + 79 E2E + 3 openapi sin cambios;
36 unit + 5 E2E LSP sin cambios. Logo + script no agregan tests
Rust. Validación local Windows: ✅ `fitz-language-win32-x64-0.9.2.vsix`
(1.49 MB, 211 archivos, `server/fitz-lsp.exe` bundleado).

**Próximo norte (técnico)**: resto de Fase 9 — **package manager
+ registry**, **formatter**, **linter**. Plan a definir al arrancar.

**Deuda residual derivada (NO bloquea próximas fases)**:

- CI multi-platform (GitHub Actions workflow).
- Publicación automática al Marketplace post-CI build.
- Cross-compile local desde una plataforma (requiere `cross` crate
  o Docker). Hoy: cada plataforma genera su propio .vsix nativo.
- Logo: versiones adicionales (favicon 32×32, app icon 512×512,
  monochrome para temas dark) si aparece demanda.

## [v0.9.5] — 2026-05-16 — Fase 9.x.4: LSP autocomplete contextual

Cuarta sub-fase visible del LSP — completa el MVP del language
server. El cliente VSCode (o cualquier otro cliente LSP) ahora puede
pedir `textDocument/completion` con una posición y recibe una lista
de `CompletionItem` apropiados al contexto: tras un `.` muestra los
fields/métodos del tipo del receiver, en cualquier otra posición
muestra los símbolos top-level del programa + builtins + tipos +
keywords. Cierra el loop "errores subrayados + hover + go-to-def
+ autocomplete" — el LSP MVP ya cubre la experiencia core de
editing.

Dos sub-pasos coordinados (un commit por sub-paso):

- **9.x.4.a — Persistir Program + helper `completion_at_position`**:
  - `check_source_with_types` retorna 5-tupla incluyendo `Program`:
    `(Program, TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>)`.
    El AST es necesario para que el LSP enumere top-level
    declarations en scope-level y resuelva receivers por nombre en
    after-dot (fallback cuando el parser abandona stmts rotos).
    Call sites del LSP actualizados.
  - `fitz::lsp::completion_at_position(text, program, type_info,
    type_env, line, character) -> Vec<CompletionItem>` (pure
    function, unit-testeable). Despacha por contexto detectado:

    - **Scope-level**: enumera top-level del Program
      (let/fn/type/import) + builtins (print/len/sleep/cors) +
      tipos built-in (Int/Float/Str/.../PyAny) + keywords del
      lenguaje. **NO scope-aware**: no enumera vars locales/params
      como función del cursor (deuda MVP — requiere refactor del
      checker para exponer scopes por stmt). VSCode filtra por
      prefix client-side; el usuario puede tipear vars locales
      aunque no aparezcan en la lista.

    - **After-dot**: identifica el receiver (un solo ident antes
      del `.`), resuelve el tipo con **dos fallbacks**:
      1. TypeInfo lookup heurístico (max col <= recv_col en la
         misma línea).
      2. Walk del Program por nombre — busca `Stmt::Assign`
         top-level con `target == recv_name` y mira el tipo del
         value en TypeInfo. Cubre el caso típico `obj.<cursor>`
         al final del buffer donde el parser abandona el stmt
         entero por el `.` huérfano (deuda F15 recovery sub-stmt).

      Tipos cubiertos: `Nominal` (fields del TypeEnv), `List` (6
      métodos), `Map` (5 métodos), `Str` (3 métodos). Otros (Any,
      PyAny, primitivos) devuelven lista vacía.

  - Helpers internos: `CompletionContext` enum (AfterDot con
    `recv_name`+`recv_line`+`recv_col` / ScopeLevel),
    `detect_completion_context` (walk hacia atrás del cursor),
    `position_to_offset` / `offset_to_position` (UTF-8 char-based;
    UTF-16 LSP default queda como refinamiento si aparece presión
    real con código no-ASCII), `is_ident_continue` (ASCII
    alphanumeric + `_`), `method_items` (factory para METHOD kind),
    `after_dot_completions`, `scope_level_completions`.

  - `DocumentState` del backend suma `program: Program` con
    `#[allow(dead_code)]` puntual hasta 9.x.4.b.

  - 10 unit tests nuevos en `fitz::lsp::tests`: round-trip
    `position_to_offset`/`offset_to_position`; 4 casos de
    `detect_context` (vacío, después de ident, after-dot, after-dot
    con prefix); scope-level lista top-level+builtins+tipos+kws;
    after-dot Nominal lista fields del type (FIELD kind); after-dot
    List lista 6 métodos (METHOD kind); after-dot Str lista 3
    métodos (cubre el fallback walk-del-Program); after-dot
    receiver sin tipo devuelve vacío.

- **9.x.4.b — Handler `completion` + capability + E2E**:
  - Capability `completion_provider: Some(CompletionOptions {
    trigger_characters: Some(vec![".".into()]), resolve_provider:
    Some(false), ... })` anunciada en `initialize`. El trigger char
    `.` hace que VSCode invoque automáticamente completion tras un
    punto; para typing normal, el cliente invoca por su cuenta.
    `resolve_provider: false` porque mandamos toda la info en el
    item (no usamos `completionItem/resolve` para lazy details).
  - Handler `Backend::completion` lee state bajo lock, delega al
    helper pure-function, devuelve `CompletionResponse::Array(items)`.
    Sin awaits dentro del lock.
  - `#[allow(dead_code)]` removido de `DocumentState.text` y
    `DocumentState.program` (ya tienen consumidor).
  - 1 E2E nuevo `completion_after_dot_sobre_str_lista_metodos_built_in`:
    valida capability anunciada con `triggerCharacters: ["."]`,
    after-dot sobre `s.` con `s: Str` lista `upper`/`lower` y NO
    `push` (no es método de Str), scope-level lista `s` (var
    top-level) + `print` (builtin) + `Int` (tipo built-in) + `let`
    (keyword).

**Decisiones técnicas tomadas al arrancar**:

- **Alcance**: MVP cubre (1) scope-level y (2) after-dot. **(3)
  imports** (`from mod import `) queda como deuda visible — requiere
  cargar el módulo remoto y enumerar sus exports, complejidad del
  loader que pertenece a sub-paso futuro.
- **Scope-level no scope-aware**: enumeramos top-level del Program
  + builtins + tipos + keywords. NO enumeramos vars locales/params
  según la posición del cursor. Scope-aware requiere refactor del
  checker. Trade-off MVP aceptado: VSCode filtra por prefix client-
  side, el usuario puede tipear vars locales igual.
- **After-dot solo `<ident>.`**: chain `a.b.c.` queda como deuda
  — requeriría parser parcial.
- **After-dot con dos fallbacks**: TypeInfo lookup heurístico +
  walk del Program por nombre. El walk cubre el caso típico donde
  el parser abandona el stmt entero por el `.` huérfano (deuda F15
  recovery sub-stmt). Sin el fallback, `obj.<cursor>` al final del
  buffer no funcionaría.
- **Persistir `Program` en `DocumentState`**: el AST es necesario
  en cada completion request (scope-level enumera top-level; after-
  dot fallback walkea por nombre). Re-walkar es barato vs re-parsear.
- **`CompletionItem` shape**: label, kind (Variable/Function/Field/
  Method/Keyword/Class/Module), detail opcional (firma de fn/método
  o tipo de field). VSCode renderea kind con íconos distintivos.
- **`UTF-8 char-based` para position↔offset**: LSP default es UTF-16,
  pero el MVP asume programas mayormente ASCII. Refinable post-MVP
  si aparece presión real con código no-ASCII.

**Total al cierre**: 1233 unit (default) + 79 E2E + 3 openapi sin
cambios respecto a Fase 9.x.3. **10 unit nuevos + 1 E2E nuevo** con
`--features lsp` (acumulado 36 unit + 5 E2E en LSP). Clippy
`-D warnings` limpio sobre lib + ambos bins + tests.

**Cierre formal del LSP MVP**: con 9.x.4 cerrada, el LSP cubre
la experiencia core de editing — diagnostics, hover, go-to-def,
autocomplete. Lo que sigue (9.x.5) es distribución (publicar al
VSCode Marketplace con binarios bundleados por plataforma).

**Próximo norte**: **9.x.5 (distribución VSCode Marketplace)** —
publicar la extensión con binarios pre-compilados (Windows x64,
macOS x64+ARM, Linux x64+ARM) bundleados en el `.vsix`, al estilo
rust-analyzer. Alternativa de alfa: `.vsix` manual + `fitz-lsp` en
PATH (lo que ya tenemos en 9.x.1.c).

**Deuda residual derivada (NO bloquea 9.x.5)**:

- **Completion para imports** (`from mod import `): listar
  símbolos exportados por el módulo. Requiere cargar el módulo
  remoto y mapearlo a CompletionItems. Sub-paso futuro.
- **Scope-aware en scope-level**: enumerar vars locales y params
  según la posición del cursor. Requiere refactor del checker para
  exponer scopes por stmt. Refinable cuando el usuario lo pida.
- **Chain `a.b.c.`** en after-dot: solo soportamos `<ident>.`.
  Requiere parser parcial para resolver el tipo del FieldAccess
  intermedio.
- **Position UTF-16 strict** (LSP default): hoy UTF-8 char-based.
  Programas mayormente ASCII funcionan; con muchos caracteres
  no-latin puede haber off-by-one. Refinable.
- **Completion en posiciones context-sensitive del parser**: tras
  `@`, sugerir decoradores (`@get`/`@server`/`@middleware`); tras
  `import `, sugerir paths de módulos. Hoy todo eso cae en scope-
  level genérico.

## [v0.9.4] — 2026-05-16 — Fase 9.x.3: LSP go-to-definition

Tercera sub-fase visible del LSP. El cliente VSCode (o cualquier
otro cliente LSP) ahora puede pedir `textDocument/definition` con
una posición y recibe la `Location` de la declaración del ident bajo
el cursor — desbloquea la experiencia "F12 sobre un nombre te lleva
a su definición", core del workflow de exploración del código.

Dos sub-pasos coordinados (un commit por sub-paso):

- **9.x.3.a — Side-table `DefinitionInfo` + populación en el checker**:
  - `VarBinding` suma `def_span: Span`: cada binding recuerda dónde
    se declaró. Builtins (`print`/`len`/`sleep`/`cors`) usan
    `Span::ZERO` y el LSP los filtra (no hay archivo donde saltar).
  - `declare_var`/`declare_var_annotated` reciben `def_span` como
    nuevo parámetro. 12 call sites actualizados con el span
    apropiado (Stmt::Assign, FnDef body params, For.var, Import,
    FromImport, FnExpr params, match patterns vía
    `bind_pattern(...arm_span)`, `preregister_fn_signatures`).
    Aproximaciones documentadas donde el AST no tiene span propio
    del binding (Param, AssignTarget::Ident, For.var,
    MatchArm.pattern — deuda S1). VSCode salta al stmt contenedor;
    el usuario ve la línea de declaración.
  - `pub struct DefinitionInfo` paralelo a `TypeInfo` (F16). Side-
    table `HashMap<SpanKey (use), Span (def)>` con `record`,
    `definition_at`, `len`, `is_empty`, `iter`. Política: omite
    `Span::ZERO` en use y def (sintéticos y builtins).
  - El wrapper `infer_expr` para `Expr::Ident` resuelve vía
    `lookup_binding`, clona los fields para liberar el préstamo
    inmutable de `ctx.scopes`, y registra `(use_span, def_span)`
    antes de retornar.
  - `check_program` retorna 4-tupla `(TypeEnv, TypeInfo,
    DefinitionInfo, Vec<FitzError>)`. 18 call sites internos
    actualizados (CLI + codegen + LSP + tests).
  - `check_source_with_types` del LSP también retorna la 4-tupla.
  - `DocumentState` del backend suma `def_info` con
    `#[allow(dead_code)]` puntual hasta 9.x.3.b.
  - Limpieza colateral: `lookup_var` (que duplicaba `lookup_binding`)
    eliminado — el único caller pasó a usar `lookup_binding`
    directamente para acceder al `def_span`.
  - 6 unit tests nuevos en `types::tests::def_info_*`: registra var
    local, NO registra builtins (Span::ZERO filtra), registra fn
    top-level, registra param de fn (aproximación al span del FnDef),
    `definition_at` devuelve None para spans ausentes o ZERO, ident
    no definido no agrega entry.

- **9.x.3.b — Handler `definition` + helpers + capability**:
  - `definition_for_position(&DefinitionInfo, line, character) -> Option<Span>`
    en `fitz::lsp` (pure function). Misma heurística que
    `hover_for_position`: max col <= cursor en la misma línea sobre
    `DefinitionInfo.iter()`.
  - `make_definition_location(Url, Span) -> Location` arma la
    respuesta LSP. Convierte 1-based Fitz a 0-based LSP; range de
    1 carácter. `uri` es el del documento abierto.
  - Capability `definition_provider: Some(OneOf::Left(true))`
    anunciada en `initialize`.
  - Handler `Backend::goto_definition` lee state bajo lock, delega
    a los helpers, devuelve `GotoDefinitionResponse::Scalar(loc)`
    (un solo Location — Fitz no tiene overloading).
  - 5 unit tests nuevos cubriendo: var local resuelve a def_span,
    línea sin idents devuelve None, builtin filtrado, conversión
    1-based → 0-based correcta, smoke pipeline end-to-end.
  - 1 E2E nuevo `goto_definition_sobre_uso_de_var_local_devuelve_location_de_let`:
    valida capability anunciada, definition sobre uso de `x` en
    `let x = 42\nlet y = x\n` devuelve Location con line:0,
    definition sobre `print` (builtin) devuelve `result: null`.

**Decisiones técnicas tomadas al arrancar**:

- **Side-table dedicado vs reuso de TypeInfo**: dedicado
  `DefinitionInfo`. Mismo patrón que F16; semánticas distintas no se
  mezclan. El checker ya hace el lookup; solo agregamos la captura
  del span al wrapper.
- **`VarBinding` gana `def_span: Span`**: refactor mecánico — los 12
  call sites de `declare_var*` pasan el span apropiado. Compiler
  ayuda a no olvidar ningún call site. Builtins usan `Span::ZERO`
  y se filtran.
- **Granularidad del span de def**: aproximaciones pragmáticas dado
  el AST actual (`Param`, `AssignTarget::Ident`, `For.var`,
  `MatchArm.pattern` no tienen span propio — deuda S1). Para
  `Stmt::Assign` reasignaciones, el `def_span` se sobreescribe con
  el del último binding stmt (semántica simplificada del MVP —
  refinable a "primera declaración" con tracking adicional).
- **Lookup heurístico igual que hover** (max col <= cursor en la
  misma línea): consistente con 9.x.2, identidad sobre idents.
- **`range` de 1 carácter en la respuesta** (sin `end_span`):
  paralelo a Diagnostics y Hover.
- **`uri` = documento abierto** (vs resolución cross-module): cross-
  module def requiere mapear paths del loader a URIs — agrega
  complejidad del loader que pertenece a 9.x.4 o post-MVP.
- **`OneOf::Left(true)` para `definition_provider`** (vs
  `DefinitionOptions`): forma simple del LSP. Fitz no tiene
  overloading, no necesitamos múltiples Locations por nombre.

**Total al cierre**: 1233 unit (default) + 79 E2E + 3 openapi sin
cambios respecto a Fase 9.x.2 (+6 unit nuevos en `types::tests::def_info_*`).
**5 unit nuevos + 1 E2E nuevo** con `--features lsp` (acumulado 26
unit + 4 E2E en LSP). Clippy `-D warnings` limpio sobre lib + ambos
bins + tests.

**Próximo norte**: **9.x.4 (autocomplete contextual)** —
`textDocument/completion` con cuatro contextos: símbolos en scope
visible (typing en cualquier posición), fields tras `obj.` (mirar
el tipo del receptor), métodos built-in tras `xs.`/`m.`/`s.` (List/
Map/Str), símbolos importados tras `from mod import `. Después: 9.x.5
distribución VSCode Marketplace. Ver `docs/roadmap.md` → "Fase 9.x".

**Deuda residual derivada (NO bloquea 9.x.4)**:

- Cross-module go-to-def: `from foo import X` apunta al span del
  Stmt::Import local, no al módulo remoto. Requiere mapear paths
  del loader a URIs.
- `def_span` granular por nombre (vs por Stmt contenedor): el AST
  no tiene `Span` propio en `Param`, `AssignTarget::Ident`,
  `For.var`, `MatchArm.pattern`. Refinable con S1.deuda.
- Reasignaciones sobrescriben `def_span` con el último let stmt
  (semántica simplificada). TypeScript salta a la primera
  declaración; Fitz salta a la última. Refinable con tracking
  adicional si pinta corto.
- Cross-method tipos (definición de método built-in `xs.map`):
  no aplica — los métodos built-in no tienen "definición" en el
  código fuente Fitz.

## [v0.9.3] — 2026-05-16 — Fase 9.x.2: LSP hover — tipo del nodo bajo el cursor

Segunda sub-fase visible del LSP. El cliente VSCode (o cualquier
otro cliente LSP) ahora puede preguntar `textDocument/hover` con
una posición y recibe el tipo del nodo bajo el cursor — desbloquea
la experiencia "pasá el mouse y ve qué tipo tiene esta expresión",
equivalente al hover que TypeScript provee desde hace años.

Dos sub-pasos coordinados (un commit por sub-paso):

- **9.x.2.a — Persistencia de TypeInfo por documento**:
  - Nueva API `fitz::lsp::check_source_with_types(src) -> (TypeEnv,
    TypeInfo, Vec<FitzError>)` que retiene el side-table de tipos
    poblado por F16. La fn vieja `check_source` se mantiene como
    wrapper que descarta env + types (consumidores que solo
    necesitan diagnostics).
  - `DocumentState { text, type_env, type_info }` reemplaza el
    `String` plano en el `documents` map del backend. `did_open`/
    `did_change` corren la pipeline y persisten los tres; `did_close`
    limpia.
  - 4 unit tests nuevos: programa válido devuelve TypeInfo no vacío,
    error de lexer aborta antes del checker (TypeInfo vacío), error
    de tipo no borra TypeInfo (Exprs válidos quedan), sanity check
    de equivalencia entre las dos APIs.

- **9.x.2.b — Hover handler + lookup heurístico + capability**:
  - `hover_for_position(&TypeInfo, line, character) -> Option<&Type>`
    en `fitz::lsp` (pure function, unit-testeable). Heurística "max
    col <= cursor en la misma línea" sobre el TypeInfo iterado.
    Convierte 0-based LSP a 1-based Fitz. Sin `end_span` en los
    nodos (deuda S1), asume que el último Expr iniciado antes del
    cursor en la misma línea es el más probable — cubre 90% del
    caso (cursor sobre o inmediatamente después de un identificador
    /literal). Refinable cuando los nodos tengan span completo.
  - `make_hover(&Type, &TypeEnv) -> Hover` arma la respuesta LSP
    con `MarkupContent::Markdown` y bloque ```fitz<tipo>```. VSCode
    renderea con syntax highlighting nativo. `range: None` porque
    sin `end_span` no podemos devolver el rango exacto del nodo —
    el tooltip funciona, el token no se resalta.
  - Capability `hover_provider: Some(HoverProviderCapability::Simple(true))`
    anunciada en `initialize`.
  - `Backend::hover` lee el state bajo lock, delega al helper y
    formatea con `make_hover`. Sin awaits dentro del lock.
  - Exposición de `pub fn iter()` sobre `TypeInfo` — necesario para
    que el LSP haga lookup heurístico (sin esto, solo `type_at`
    para lookup exacto). Mínimo y backward-compatible.
  - 8 unit tests nuevos cubriendo: posición exacta sobre literal,
    medio de Ident usado como Expr, línea sin spans, cursor antes
    del primer token, no cruce de líneas, markdown format, tipos
    compuestos (`List<Int>` se formatea OK), smoke end-to-end.
  - 1 E2E nuevo `hover_sobre_literal_int_devuelve_tipo_en_markdown`:
    valida capability anunciada, hover sobre `42` en col 8 devuelve
    `Int` en markdown fitz, hover en posición sin spans devuelve
    `result: null`.

**Decisiones técnicas tomadas al arrancar**:

- **Persistencia de TypeInfo por URI** (vs re-correr el pipeline en
  cada hover): re-correr sería lento sobre buffers grandes; el
  TypeInfo es solo un `HashMap<SpanKey, Type>` — pesa nada.
- **Heurística de lookup "max col <= cursor en la misma línea"** (vs
  lookup exacto que casi nunca funcionaría sin que el cursor esté
  en el inicio exacto del span). Limitación heredada de F16: sin
  `end_span` no podemos hacer "está adentro del nodo". El 90% del
  caso (cursor sobre token corto) funciona; tokens largos pueden
  fallar si el cursor está muy al final.
- **Colisiones en TypeInfo aceptadas como están**: cuando dos `Expr`
  comparten span (típicamente un `BinOp` y su primer operando),
  TypeInfo guarda solo el último escrito (heredado de F16). En la
  práctica el tipo del Expr más "grande" es lo que el usuario
  quiere ver al hover.
- **Persistir TypeEnv junto con TypeInfo**: `Type::display` necesita
  el env para resolver nombres de tipos nominales. Sin el env, el
  hover sobre un `User` mostraría `Nominal(TypeId(3))` en vez de
  `User`. Cambio chico de firma en `check_source_with_types`.
- **`MarkupContent::Markdown` con bloque ```fitz``` (vs PlainText)**:
  VSCode aplica syntax highlighting si reconoce el lenguaje. Más
  bonito sin costo.
- **`range: None` en la respuesta Hover**: sin `end_span` no podemos
  devolver el rango exacto. El tooltip se muestra igual, solo el
  highlighting del token no aparece.

**Total al cierre**: 1227 unit (default) + 79 E2E + 3 openapi sin
cambios. **12 unit nuevos + 1 E2E nuevo** con `--features lsp`
(acumulado 21 unit + 3 E2E en LSP). Clippy `-D warnings` limpio.

**Próximo norte**: **9.x.3 (go-to-definition)** —
`textDocument/definition` resuelve `Ident` → span de la declaración
(`let x = ...`, `fn f(...)`, `type T { ... }`). Requiere mantener
una tabla de resolución de scopes desde el checker. Después: 9.x.4
autocomplete contextual, 9.x.5 distribución VSCode Marketplace.
Ver `docs/roadmap.md` → "Fase 9.x".

## [v0.9.2] — 2026-05-15 — Fase 9.x.1: LSP MVP — diagnostics + extensión VSCode

Primera sub-fase visible del LSP. Habilita la experiencia "escribir
Fitz en VSCode con errores subrayados al tipear" — equivalente al
nivel de servicio que ofrece TypeScript en sus primeros segundos.

Tres componentes coordinados (un commit por sub-paso):

- **9.x.1.a — Server skeleton** (bin nuevo `fitz-lsp`, feature opt-in
  `lsp`, handshake initialize/shutdown):
  - Dep `tower-lsp = "0.20"` opcional; feature `lsp = ["dep:tower-lsp"]`
    paralela a `python = ["dep:pyo3"]`. Bin `[[bin]] name = "fitz-lsp"`
    con `required-features = ["lsp"]`. El bin `fitz` default sigue
    standalone, sin pagar el peso de tower-lsp + lsp-types en el dep tree.
  - `src/bin/fitz-lsp.rs` con `Backend` impl `LanguageServer`:
    `initialize` → response con `serverInfo` + `textDocumentSync: FULL`,
    `initialized` (log via `client.log_message`), `shutdown`.
    `#[tokio::main(flavor = "current_thread")]` (LSP es I/O-bound).
  - 1 test E2E `tests/lsp_e2e.rs` que spawnea el bin y valida el
    handshake. Frames JSON-RPC construidos a mano via Content-Length,
    sin deps extras. `#![cfg(feature = "lsp")]`.

- **9.x.1.b — Lib refactor + helper diagnostics + lifecycle hooks**:
  - **Lib refactor**: `src/lib.rs` nuevo expone los módulos como
    `pub mod`. `src/main.rs` migra de `mod X;` a `use fitz::{...};`.
    Habilita que `fitz-lsp` reuse `lexer`/`parser`/`types` sin
    compilación duplicada.
  - **`src/lsp.rs` (nuevo, lib, feature-gated)**: dos APIs públicas
    pure-function unit-testeables:
    - `check_source(&str) -> Vec<FitzError>` — pipeline LSP-style:
      tokenize → `parse_with_recovery` (F15) → `check_program`
      (descarta el `TypeInfo` que llega 9.x.2 hover).
    - `fitz_errors_to_diagnostics(&[FitzError]) -> Vec<Diagnostic>` —
      mapea 1-based Fitz → 0-based LSP. Range 1-char (refinable a
      span completo cuando S1.Pattern/TypeExpr sume `end_span`).
      `hint` concatenado al `message`. Severity ERROR, source "fitz".
      Sentinel `(0, 0)` → range degenerado al inicio del documento.
  - **Backend con DocumentStore**: `documents: Arc<parking_lot::Mutex
    <HashMap<Url, String>>>`. `did_open`/`did_change`/`did_close`
    disparan `check_source → fitz_errors_to_diagnostics →
    publish_diagnostics`. Cierre limpia diagnósticos.
  - 9 unit tests + 1 test E2E nuevo (`did_open` con buffer roto valida
    la notification `textDocument/publishDiagnostics`).
  - **Deuda nueva visible**: `#[allow(clippy::result_unit_err)]`
    puntual sobre `Environment::assign` en `src/env.rs`. Lint apareció
    en clippy 1.95 + expuesto por el refactor lib (antes silencioso).
    El `Result<(), ()>` ahí es sentinel intencional. Refactor a
    newtype error queda como deuda menor.

- **9.x.1.c — Extensión VSCode** (`editors/vscode/`, paquete TypeScript):
  - Grammar TextMate (`syntaxes/fitz.tmLanguage.json`): comments,
    strings con interpolación `{...}` recursiva, números, decoradores
    `@nombre`, keywords (control + declaración + lógicos), tipos
    built-in (Int/Float/Str/Bool/Null/Range/Any/List/Map/Result/
    Future/Request/Response/PyAny) + nominales `[A-Z]…`, constantes
    `true`/`false`/`null`/`Ok`/`Err`, built-ins `print`/`len`/`sleep`/
    `cors`, operadores y defs/calls de funciones.
  - `language-configuration.json`: comments, brackets, autoClose
    (con `notIn` string/comment), surrounding, indent rules.
  - `src/extension.ts`: capa fina sobre `vscode-languageclient/node`.
    `resolveServerPath` distingue absoluto / relativo-a-workspace /
    nombre-suelto-en-PATH. Error visible al usuario si el binario no
    spawnea, citando el path intentado.
  - Settings: `fitz.lspPath` (default `"fitz-lsp"`) y
    `fitz.trace.server` (off/messages/verbose).
  - Activation `onLanguage:fitz`.
  - Validaciones build: 4 manifestos JSON OK; `npm install` (12
    packages, 0 vulns); `npm run compile` (tsc strict, sin warnings);
    `npx @vscode/vsce package` produce `.vsix` 294 KB con 209 archivos.
    `node_modules/`, `out/`, `*.vsix` excluidos por `.gitignore` local.

**Decisiones técnicas tomadas al arrancar**:

- **bin `fitz-lsp` separado del CLI principal** (vs subcomando):
  convención ecosistema (rust-analyzer/gopls/tsserver). Bin `fitz`
  queda chico, release ciclo independiente.
- **`tower-lsp` sobre `lsp-server` crudo**: async-first, framing
  JSON-RPC automático. Cientos de LoC menos para el MVP.
- **Grammar TextMate sobre tree-sitter**: TextMate son ~120 LoC JSON,
  suficiente para colores. Tree-sitter más preciso pero requiere
  build chain extra. Refinable post-MVP.
- **Descubrimiento via setting `fitz.lspPath`** (vs bundling): alfa
  simple. Bundling rust-analyzer-style llega en 9.x.5.
- **`textDocumentSync: FULL`** (vs `INCREMENTAL`): default razonable
  para MVP. Migración es decisión de perf si aparece presión.
- **`tokio::main(flavor = "current_thread")`** para LSP: I/O-bound,
  sin work-stealing necesario. Decisión ortogonal a la del CLI HTTP
  (multi-thread, F17).

**Total al cierre**: 1227 unit + 79 E2E + 3 openapi sin cambios
(default). **9 unit + 2 E2E nuevos** con `--features lsp`. Clippy
`-D warnings` limpio sobre lib + ambos bins + tests.

**Próximo norte**: **9.x.2 (hover)** — `textDocument/hover` devuelve
el tipo del nodo bajo el cursor. Consume el `TypeInfo` (F16) que
hoy `check_source` descarta. Después: 9.x.3 go-to-definition,
9.x.4 autocomplete contextual, 9.x.5 distribución Marketplace.
Ver `docs/roadmap.md` → "Fase 9.x".

## [v0.9.1] — 2026-05-15 — Fase 9.0: F16 cierre — IR tipado persistido por nodo

Segundo y último sub-paso de Fase 9.0. Cierra la deuda F16
identificada post-5b: **segundo pre-requisito habilitante del LSP**
(hover y completion contextual). El checker ahora retiene los tipos
sintetizados de cada nodo `Expr` en un side-table devuelto junto al
`TypeEnv`.

**Sin cambio de comportamiento user-facing**: `fitz run` / `fitz
build` / `fitz check` siguen ignorando el side-table. La API nueva
(`TypeInfo`, retornada por `check_program`) está pensada para los
consumidores del LSP que llegan en sub-fases siguientes.

- **9.0.4 — Side-table TypeInfo + populación + tests** (8 unit nuevos):
  - Nuevo `pub struct SpanKey(usize, usize)` como clave hashable.
    Necesario porque `Span` propio no sirve: su `PartialEq` devuelve
    `true` siempre (intencional para que los tests de AST comparen
    estructura sin re-derivar posiciones del parser).
  - Nuevo `pub struct TypeInfo` con `record(span, ty)`,
    `type_at(span)` y `len()`. Omite `Span::ZERO` (sintéticos /
    tests) para evitar colisiones bajo la misma clave `(0, 0)`.
  - `infer_expr` pasa a ser wrapper sobre `synthesize_expr`: la
    lógica del match queda igual, y el wrapper centraliza el
    `record` al salir. Cobertura amplia desde un solo punto, sin
    "olvidé tal caso".
  - `pub fn check_program` cambia firma de `(TypeEnv,
    Vec<FitzError>)` a `(TypeEnv, TypeInfo, Vec<FitzError>)`. Los
    13 call sites internos (main.rs, codegen.rs, tests) migrados
    descartando el segundo elemento con `_types` — la CLI no
    consume el side-table todavía.
  - `Expr::Error` (F15) se persiste como `Type::Any` uniforme con
    el comportamiento del checker. El LSP decide qué mostrar en
    hover sobre Error nodes.
  - Tests del side-table (`types::tests::types_info_*`): literales,
    ident + BinOp, call + field, match arms, omisión de Span::ZERO,
    Error nodes como Any, lookup ausente devuelve None, smoke
    sobre programa real (`info.len() >= 10`).

- **9.0.5 — Cierre formal**: este CHANGELOG, `docs/roadmap.md`
  con Fase 9.0 — F16 documentada paso a paso, `docs/deudas-post-5b.md`
  con F16 marcado CERRADO, README + CLAUDE refresh.

**Decisiones técnicas tomadas al arrancar**:

- **`HashMap<SpanKey, Type>` (vs NodeId asignado al nodo, vs
  `*const Expr`)**: simple, reusa los spans que ya tiene cada
  `Expr` post-S1.2, zero refactor del AST. La colisión potencial
  por `Span::ZERO` se resuelve omitiendo esos nodos.
- **Cobertura amplia (todo `Expr` que pasa por `infer_expr`)**:
  un solo `record` en el wrapper en lugar de un insert por
  brazo del match. Futuro-proof contra nuevos tipos de Expr.
- **API: una sola** (no variante `check_program_with_types`):
  los 13 call sites son triviales (`let (env, _types, errors) =
  ...`), una sola API es más limpia que dos en paralelo.
- **`Span::ZERO` omitido**: sintéticos del parser y nodos de
  tests colisionarían entre sí bajo la misma clave; ninguno es
  user-visible para hover.
- **Solo `Expr` (no `Stmt` / `TypeExpr` / `Pattern`)**: el LSP
  obtiene info de variables y fns por scope lookup; persistir
  Stmt es ortogonal. Spans en `TypeExpr` y `Pattern` siguen
  como deuda residual menor de S1 — refinable post-LSP MVP si
  aparece presión real.

**Total al cierre**: 1227 unit + 79 E2E + 3 openapi sin feature.
Clippy `-D warnings` limpio.

**Próximo norte**: las sub-fases visibles del LSP — **9.x.1
(diagnostics MVP)**, 9.x.2 (hover, ya consume `TypeInfo`), 9.x.3
(go-to-definition), 9.x.4 (autocomplete), 9.x.5 (distribución
VSCode Marketplace). Ver `docs/roadmap.md` → "Fase 9.x".

## [v0.9.0] — 2026-05-15 — Fase 9.0: F15 cierre — error recovery del parser

Primer sub-paso de Fase 9 (Ecosistema). Cierra la deuda F15
identificada post-5b: **pre-requisito habilitante del LSP** que
permitirá que herramientas externas (LSP, formatter, futuros
analizadores) reciban un AST parcial y la lista paralela de
errores sobre buffers en construcción.

**Sin cambio de comportamiento user-facing**: `fitz run` / `fitz
build` / `fitz check` siguen usando `parse()` strict y abortando al
primer error de parser, exactamente como antes. La API nueva
(`parse_with_recovery`) está pensada para los consumidores
internos del lenguaje que llegan en sub-fases siguientes.

- **9.0.1 — AST + API recovery + tests del parser** (10 unit nuevos):
  - Nuevas variantes `Expr::Error(Span)` y `Stmt::Error(Span)` en
    el AST. Su único productor es `parse_with_recovery`; mantienen
    la forma estructural del árbol cuando hay errores recuperados
    (un body de fn con un stmt roto sigue siendo un `Vec<Stmt>`
    válido).
  - Parser: flag interno `recovery_mode` + cota dura
    `MAX_RECOVERED_ERRORS = 100` + helper `synchronize()` que
    avanza hasta sync points stmt-level. Sync points: `Newline`
    (consumido), `RBrace`/`EOF` (preservados), y keywords que
    típicamente arrancan stmt — `Let`, `Fn`, `Async`, `Type`,
    `Return`, `Break`, `Continue`, `While`, `Loop`, `For`, `If`,
    `Import`, `From`, `At` — preservadas. La regla de keywords
    fue necesaria porque `primary()` consume el token actual antes
    de validar: un `Newline` inesperado se consume y el cursor
    termina parado en el `Let` del próximo stmt; sin la parada en
    keywords, sync se comía stmts enteros.
  - API pública nueva:
    `pub fn parse_with_recovery(tokens) -> (Program, Vec<FitzError>)`.
    Nunca retorna `Err`: los errores se acumulan en la lista
    paralela. Marcada con `#[allow(dead_code)]` justificado
    porque hasta que aterricen los consumidores (LSP / formatter)
    solo la ejercitan los tests.
  - Defensas en evaluator/codegen: si un nodo Error llega ahí
    (no debería — la CLI strict nunca los produce), emiten un
    `FitzError` claro con span, no panic.
  - Checker silencioso (ya entró en 9.0.1, tests en 9.0.2):
    `Expr::Error` sintetiza `Type::Any`, `Stmt::Error` no-op.
  - Tests del parser (`parser::tests::recovery_*`): programa
    válido sin errores, stmt roto top-level, dos errores
    consecutivos, recovery dentro de `if`/`fn` body, span del
    Error node apunta al inicio del stmt, posición del error
    apunta al token problemático, EOF inesperado se acumula,
    cota de 100 errores se respeta, fn con body roto preserva
    estructura, parse strict sigue abortando al primer error.

- **9.0.2 — Tests del checker sobre AST recuperado** (5 unit nuevos):
  - Helper local `check_recovering(src)` que corre el pipeline
    LSP-style (`parse_with_recovery` → `check_program`) y devuelve
    solo los errores del checker. Es el pipeline que usará el LSP
    MVP para producir diagnostics.
  - Tests (`types::tests::checker_stmt_error_*` y
    `checker_pipeline_recovering_*` y `checker_expr_error_*`):
    Stmt::Error no agrega errores derivados; el silencio sobre
    Error nodes no afecta detección de errores genuinos en stmts
    vecinos válidos; Error nodes en fn body no abortan el check
    del resto del programa; smoke con 3 stmts rotos no panic;
    Expr::Error directo en AST tipa como Type::Any.

- **9.0.3 — Validación end-to-end + cierre formal**:
  - Smoke a mano: `fitz check d:/tmp/recovery_smoke.fitz` (buffer
    con 3 stmts rotos intercalados con código válido) → exit 1
    con un error reportado del primer stmt roto. Comportamiento
    strict idéntico a antes.
  - Smoke `GUIDE_EXAMPLES_COMPILE` sigue verde sobre los 13
    ejemplos de la guía compilables.
  - Docs: este CHANGELOG, `docs/roadmap.md` con Fase 9.0
    documentada paso a paso, `docs/deudas-post-5b.md` con F15
    marcado CERRADO, README + CLAUDE refresh.

**Decisiones técnicas tomadas al arrancar**:

- **Representación de errores**: nodos `Expr::Error(Span)` /
  `Stmt::Error(Span)` in-band en el AST + `Vec<FitzError>`
  paralelo. Razón: el árbol mantiene su forma estructural (mejor
  para LSP/formatter que recorren el AST sin chequear cada nodo),
  y la lista paralela lleva los mensajes ricos sin tener que
  desempaquetar wrappers en cada visita.
- **Sync points stmt-level + keywords de inicio**: la primera
  iteración tenía solo Newline/RBrace/EOF; los tests detectaron
  que `primary()` consume el token al fallar y el cursor podía
  saltar al próximo stmt. Agregar keywords como sync points cierra
  el caso sin complicar la lógica de recovery.
- **API strict intacta**: `parse()` no cambia su firma ni su
  comportamiento. Razón: la CLI sigue priorizando fail-fast con
  un error claro; recovery es feature de tooling externo.
- **Cota 100**: cubre el caso 90% del LSP (~5-20 errores en un
  buffer real) con margen amplio sin permitir cascadas runaway
  sobre buffers de tests bizarros.
- **Recovery solo stmt-level en 9.0**: errores DENTRO de un stmt
  (paréntesis sin cerrar adentro de un arg, expresión incompleta
  como RHS) descartan el stmt entero. Recovery sub-stmt
  (preservar bindings parciales, args parciales) queda como
  sub-paso futuro post-LSP MVP si aparece presión.
- **Cascadas "variable no definida" del checker**: aceptables como
  trade-off del LSP MVP. Cuando un Stmt::Error reemplaza un
  `let x = ...` roto, referencias posteriores a `x` pueden
  generar "no definida". El error real del parser apunta al lugar
  del problema; los IDEs muestran ambos diagnostics. Refinar
  requiere preservar bindings parciales (post-9.0).

**Trade-offs reconocidos**:

- `Expr::Error` solo se construye desde tests en 9.0 (el parser en
  9.0 produce Stmt::Error pero nunca Expr::Error suelto — recovery
  sub-stmt llega después). El nodo existe en el AST porque
  agregarlo después rompe match exhaustivos en 11 sitios
  (eval/checker/codegen).
- La parada en keywords como sync points es un compromiso: en
  expression-statements largos con keywords adentro (raros pero
  posibles), recovery podría sub-sincronizar. Aceptable para el
  90% del uso real; revisable si aparece presión.

**Total al cierre**: 1219 unit + 79 E2E + 3 openapi sin feature
(1310 + 88 + 3 con `--features python`). Clippy `-D warnings`
limpio en ambos modos.

**Próximo norte**: **Fase 9.0 — F16 (IR tipado persistido por
nodo)** — segundo pre-req habilitante del LSP. Después del cierre
de F16, las sub-fases visibles del LSP (9.x.1 → 9.x.5) pueden
arrancar.

## [v0.8.9] — 2026-05-15 — Fase 8.8: Guía + ejemplo CRUD + cierre de Fase 8

Octavo y último sub-paso de la Fase 8 (Interop Python). **Cierra
la fase entera**: la guía gana un capítulo dedicado a interop, el
ejemplo CRUD demuestra el flujo end-to-end (SQLAlchemy + SQLite +
HTTP + tipos Fitz), y la Fase 8 queda con todas las features del
roadmap original cubiertas — embedding (8.1), marshaling
compuesto (8.2), excepciones → `Result<T>` (8.3), tipos del
checker + coerción (8.4), `fitz py-types` (8.5), bridge async
(8.6), codegen en `fitz build` (8.7), y este cierre formal con
docs + ejemplo (8.8).

- **8.8.1 — Capítulo 21 "Interop Python" en `docs/guide.md`** + renumeración:
  - Capítulo nuevo con 12 secciones cubriendo todo lo de 8.1-8.7:
    setup (`cargo build --features python` + venvs estándar),
    sintaxis (`from python import X` + alias + path punteado),
    constantes y atributos con coerción primitiva, llamadas con
    Result wrap automático, propagación con `?`, marshaling de
    tipos compuestos (List/Map/Instance Fitz → list/dict
    Python), recuperación con anotaciones (`let row: User =
    py_call(...)?`), `fitz py-types` para SQLAlchemy, bridge
    async (`<py_call>?.await`), `fitz build` con interop (qué
    anda y qué es deuda residual), CRUD ejecutable referenciado,
    y limitaciones honestas (GIL, numpy C extensions, herencia
    Python, `asyncio.gather` con futures Fitz).
  - Renumeración: cap 21 viejo "Qué sigue" → cap 22; índice
    actualizado con la parte 10 nueva ("Cerrando" ahora vive
    en parte 10 mientras la 9 es "Interop").
  - Cap 22 ("Qué sigue") refrescado: la sección "Lo que ya sabés"
    suma el bullet de interop Python con todas las features; la
    sección "Lo que viene" pasa de "más allá de Fase 7" a "más
    allá de Fase 8" + próximo norte Fase 9, sub-paso futuro
    separado de bundling CPython, y stack DB nativo (Fase 10+).
- **8.8.2 — Ejemplo CRUD ejecutable**:
  - `examples/guide/21-python-crud/` con:
    - `models.py` — modelo SQLAlchemy `User` sobre SQLite.
    - `db.py` — helpers DB (`init_db`, `add_user`, `list_users`,
      `get_user`) que devuelven dicts/lists nativos Python para
      marshaling directo a Fitz.
    - `models.fitz` — output de `fitz py-types models.py`
      (versionado para que el ejemplo funcione sin requerir
      `sqlalchemy` instalado solo para regenerar).
    - `app.fitz` — programa Fitz principal con 3 handlers HTTP
      (`POST /users`, `GET /users`, `GET /users/{id}`) que
      combinan HTTP nativo + tipos Fitz + interop Python.
  - Helper `user_from_py(raw)` — round-trip por JSON
    (`json.dumps` + `json.loads`) para disparar la coerción
    `Map → Instance` de 8.4.3 sobre dicts Python opacos.
  - Setup: `pip install sqlalchemy` + `PYTHONPATH=...` antes
    del comando (el cap 21 explica el porqué — preferimos el
    estándar Python sobre magia de Fitz para sys.path).
  - `.gitignore` suma reglas para `__pycache__/`, `*.pyc`, y el
    SQLite local `crud.db` que el ejemplo crea al boot.
  - Validado end-to-end con curl: POST inserta con id auto-asignado
    por SQLite, GET lista, GET por id devuelve `User` Fitz tipado.

**Decisiones técnicas tomadas al arrancar**:
- **Posición del capítulo nuevo**: cap 21 (entre `fitz build` y
  "Qué sigue"). Una sola renumeración (21→22), lectura lineal —
  el cap 20 (`fitz build`) menciona limitaciones que cierra
  interop, así que conviene leerlos en ese orden.
- **Backend de DB**: SQLite + SQLAlchemy in-process. Setup
  mínimo (`pip install sqlalchemy`) sin Docker ni Postgres.
  Cubre el mismo patrón conceptual que Postgres (sesiones,
  models, queries) — el código Fitz es idéntico salvo la URL
  de conexión. Demuestra el caso canónico sin pesos extras.
- **Modo de ejecución del ejemplo**: solo `fitz run` (intérprete)
  con nota explícita sobre 8.7. El intérprete ya tiene la
  coerción `Map → Instance` (8.4.3) que el ejemplo necesita;
  `fitz build` cierra el codegen interop (8.7) pero la coerción
  de compuestos sigue siendo deuda residual. Documentado
  honestamente en el cap.

**Cierre formal de Fase 8 entera (Interop Python)**:

Roadmap original cumplido al 100%:
- ✅ Embedding básico de CPython (8.1)
- ✅ Marshaling List/Map/Instance bidireccional (8.2)
- ✅ Excepciones Python → `Result<T>` (8.3)
- ✅ Tipos del checker + coerción runtime (8.4)
- ✅ `fitz py-types` auto-mapeo SQLAlchemy (8.5)
- ✅ Bridge async tokio ↔ asyncio (8.6)
- ✅ Codegen interop en `fitz build` (8.7 — cierra deuda F19)
- ✅ Guía + ejemplo CRUD + cierre formal (8.8)

**Tests al cierre**: 1204 unit + 79 E2E + 3 openapi sin feature;
**1295 unit + 88 E2E + 3 openapi con `--features python`**.
Clippy `-D warnings` limpio en ambos modos.

**Sub-paso separado pendiente** (no parte del roadmap original
de Fase 8): bundling CPython embebido (`fitz build
--bundle-python`). Decisión python-build-standalone vs
PyOxidizer pendiente; sin presión real al cierre.

**Próximo norte**: Fase 9 — Ecosistema (package manager, LSP
con autocomplete + hover + go-to-def, formatter, linter). Pre-reqs
habilitantes ya identificados: parser con error recovery (F15) +
IR tipado persistido por nodo (F16).

## [v0.8.8] — 2026-05-15 — Fase 8.7: Codegen interop Python en `fitz build` (cierra F19)

Séptimo sub-paso de la Fase 8 (Interop Python). Cierra la deuda
**F19** del roadmap post-5b: `fitz build` ahora compila programas
con `from python import` a binario nativo standalone con pyo3
linkeado, con paridad bit-a-bit ante `fitz run`. El binario asume
Python instalado en el destino (`PYO3_PYTHON` o `python3` en
PATH) — bundling de CPython queda como sub-paso futuro separado.

**Decisión de alcance al arrancar 8.7** — separar codegen
(deuda F19, alcance medible) de bundling (decisión de herramienta
pendiente, proyecto más grande). Carta blanca del autor confirma:
F19 cierra ahora, bundling queda explícito como deuda residual
con dos opciones reales evaluadas (python-build-standalone,
mantenida activamente por Astral para `uv`; PyOxidizer,
ralentizado en 2024-2025).

- **8.7.1 — Preludio Python + import + getattr + Cargo.toml condicional**:
  - `collect_python_imports(program)` separa imports Python del
    AST top-level; el `ModuleLoader` Fitz los skipea (no hay
    archivo `.fitz` que cargar).
  - Cargo.toml generado suma `pyo3 = { version = "0.28",
    features = ["abi3-py310", "auto-initialize"] }` cuando
    `uses_python`. Programas sin interop no pagan el costo de
    bajar/linkear pyo3 — sigue siendo binario libre como Fase 5b.
  - Preludio Python emitido en `emit_python_prelude` (solo
    cuando `uses_python`): `struct __FitzPyObject(Arc<Py<PyAny>>)`
    con Clone/Debug/PartialEq (por puntero)/Display que delega
    a `__str__` Python (paridad bit-a-bit con `print(math.pi)`).
    Helpers `__fitz_py_import`, `__fitz_py_get_attr_obj`,
    `__fitz_py_extract_{i64,f64,string,bool}`,
    `__fitz_py_err_to_string` (formato canónico `<Class>: <msg>`
    paralelo a 8.1.2).
  - **Bindings globales**: cada `from python import X` se emite
    como `static __FITZ_PY_BIND_X: OnceLock<__FitzPyObject>` +
    getter `__fitz_py_bind_x()`. Lazy init en el primer
    `Python::attach`. Cualquier fn (main, handlers HTTP,
    helpers) referencia el binding via el getter.
  - `Type::PyAny` → `__FitzPyObject` en `rust_type_for`.
    `gen_field_access` despacha sobre receptor PyAny.
    `coerce(PyAny → T)` con T primitivo emite extracción
    directa (`let pi: Float = math.pi` →
    `__fitz_py_extract_f64(...)`).
- **8.7.2 — Call + marshaling Fitz → Python + Result wrap**:
  - `gen_call` / `gen_method_call` aceptan receptor PyAny y
    emiten `__fitz_py_invoke(&<callable>, |py| Ok(vec![<args
    marshalled>]))` con resultado `Result<__FitzPyObject,
    String>`. Excepciones Python aparecen como
    `Err(Str("<Class>: <msg>"))` paralelo a 8.3.
  - Trait `__FitzToPy` con impls genéricos para primitivos
    (i64/f64/bool/()/String), `__FitzPyObject` (passthrough con
    clone_ref), `Option<T>`, `Arc<Mutex<Vec<T>>>` (List → list
    con breadcrumb `arg0[i]`), `Arc<Mutex<Vec<(K,V)>>>` (Map →
    dict con `__fitz_py_marshal_map_key` para primitivos
    hashables).
  - **Marshaling Instance Fitz → Python dict**: `gen_type_def`
    emite `impl __FitzToPy for FooData` (PyDict con fields en
    orden) + wrapper `impl __FitzToPy for Arc<Mutex<FooData>>`
    cuando `uses_python`. Destraba el caso canónico 8.5: pasar
    `User { id: 1, name: "Ada" }` a `json.dumps(user)`.
  - `gen_python_call_args(args)` emite cada arg como
    `<code>.__fitz_to_py(py, "arg<i>")?` paralelo a
    `value_to_py(path: &str)` del intérprete 8.2.
- **8.7.3 — Bridge async tokio ↔ asyncio**:
  - Helper `async fn __fitz_py_invoke_await<F>(callable, args_fn)`
    en el preludio (solo cuando `uses_async`): combina call sync
    + detección `inspect.isawaitable` + ejecución vía
    `tokio::task::spawn_blocking + asyncio.new_event_loop().
    run_until_complete()`. Paralelo a `py_coro_to_fitz_future`
    8.6.1 (mismo baseline blocking, mismo trade-off).
  - Patrón canónico Fitz único: `<py_call>?.await`. El AST es
    `Await(Try(Call PyAny))`. El codegen detecta el patrón
    (`try_gen_python_await` + `try_gen_python_call_await`) y
    emite `__fitz_py_invoke_await(&callable, |py| Ok(vec![<args>])).
    await?` con el `?` Rust al final para propagar excepciones
    asyncio. Tipo Fitz resultante: `PyAny`.
  - Checker (`Type::PyAny.await → Any`) acepta el patrón
    estáticamente; rechaza `<call>.await` directo sin `?`
    (paridad bit-a-bit con evaluator del intérprete que también
    rechaza en runtime).
- **8.7.4 — Cierre formal**:
  - `examples/python-interop-8.7.fitz` con 3 secciones
    (constantes + coerción primitiva, calls + Result + marshaling
    List/Instance, bridge async con patrón `?.await`). Validado
    bit-a-bit `fitz run` ↔ `fitz build` + binario standalone
    ejecutado.
  - CHANGELOG v0.8.8, roadmap 8.7 actualizado a CERRADA,
    deudas-post-5b marca **F19 CERRADO** con nota detallada,
    README + CLAUDE refresh.

**Decisiones técnicas tomadas al arrancar**:
- **Alcance acotado (codegen sí, bundling no)** — F19 era deuda
  medible; bundling es proyecto separado.
- **Bindings globales con OnceLock + getter** — destraba uso
  adentro de handlers HTTP y user-fns sin refactor.
- **Trait `__FitzToPy` con impls condicionales por nominal** —
  estático, sin mini-Value runtime. Disjunto con List/Map
  genéricos.
- **Patrón canónico `?.await` único** — paridad bit-a-bit con
  intérprete + un solo camino que mantener.
- **Auto-coerción primitiva via `coerce(PyAny → T)`** —
  aprovecha la infraestructura existente; dispara solo con
  anotación destino concreta.

**Deuda residual visible** (sub-paso futuro): coerción Python
list/dict → Fitz `List<T>`/`Map<K,V>`/`Instance` (helpers
`__fitz_py_to_list_*` ya emitidos, falta wiring en `coerce`);
`.await` split con binding intermedio; bundling CPython
embebido (proyecto separado).

**Cierre formal**: 1295 unit + 88 E2E + 3 openapi con feature
python; 1204 + 79 + 3 sin feature. Clippy `-D warnings` limpio
en ambos modos. Paridad bit-a-bit `fitz run` ↔ `fitz build`
validada con `examples/python-interop-8.7.fitz`.

## [v0.8.7] — 2026-05-15 — Fase 8.6: Bridge tokio ↔ asyncio

Sexto sub-paso de la Fase 8 (Interop Python). Habilita
`py_async_fn().await` desde cualquier `async fn` Fitz: cuando un
call a una función Python devuelve una corutina (caso típico de
`async def`), Fitz la envuelve automáticamente en `Value::Future`
adentro del `Result::Ok`. El `.await` postfix existente (Fase 6)
desempaca el Future, ejecuta la corutina, y devuelve el valor
coercionado a `Value`. Excepciones asyncio bajan como
`Result::Err` con el formato canónico ya estable desde 8.1.2.

- **8.6.1 — Bridge baseline + tests**:
  - `py_interop::call` detecta cuando el return Python es awaitable
    (via `inspect.isawaitable`) y lo envuelve automáticamente en
    `Value::Future` adentro del `Result::Ok`. El usuario no necesita
    glue manual; el `.await` postfix lo desempaca naturalmente.
  - Helpers nuevos: `is_coroutine(py, obj)` (introspección defensiva
    con fallback a `false`) y `py_coro_to_fitz_future(coro)` que
    construye el `FitzFuture`.
  - **Implementación "baseline blocking"**: el FitzFuture envuelve
    `tokio::task::spawn_blocking` + `asyncio.new_event_loop()
    .run_until_complete(coro)`. El `Py<PyAny>` (Send-safe) viaja
    al worker; el `Bound` derivado solo existe adentro del
    `Python::attach` del worker. El blocking pool de tokio aísla
    el bloqueo del scheduler async.
  - **Tests** (3 nuevos en evaluator bajo `#[cfg(feature = "python")]`):
    - `fase_8_6_asyncio_sleep_awaiteable_desde_fitz`:
      `asyncio.sleep(0)?.await` adentro de async fn Fitz → Null.
    - `fase_8_6_async_fn_fitz_que_await_python_devuelve_valor_calculado`:
      `async fn doble(x) -> Result<Int> { sleep; return Ok(x*2) }`
      con `doble(21).await` → 42.
    - `fase_8_6_call_async_devuelve_result_future`: shape lazy —
      sin `.await`, el binding es `Result::Ok(Future(_))`.
- **8.6.2 — Ejemplo runnable + cierre formal**:
  - `examples/python-interop-8.6.fitz` con 3 secciones: patrón
    canónico (`doble_eventual(x)` con `sleep + return Ok(x*2)`),
    awaits encadenados (`pipeline(start)` con 3 sleeps + cálculo),
    lazy sin `.await` (`Result<Future>` no ejecutado). Notas
    extensas sobre el modelo de errores asyncio (heredado de 8.3),
    el trade-off baseline blocking y por qué no hacemos un caso
    runnable de excepción asyncio (definir `async def` custom
    requiere un archivo Python helper aparte). Validado bit-a-bit
    con `cargo run --features python -- run
    examples/python-interop-8.6.fitz`.
  - CHANGELOG v0.8.7, roadmap a CERRADA, deudas nota de cierre,
    CLAUDE + README refresh.

**Decisiones técnicas tomadas al arrancar**:

- **Detección automática de awaitable en `call`** (no `.await`
  manual sobre PyObject): el usuario escribe `py_async_fn().await`
  natural, sin pensar "esto es coroutine". La detección usa
  `inspect.isawaitable` (canónica en Python stdlib).
- **Approach baseline blocking** (vs `pyo3-async-runtimes::tokio::
  into_future`): la crate `pyo3-async-runtimes` 0.28 requiere
  control del runtime tokio (`init_with_runtime`/`run`), lo que
  choca con el tokio que Fitz ya tiene corriendo
  (current_thread CLI / rt-multi-thread HTTP). `spawn_blocking` +
  `run_until_complete` es Send-safe, no deadlockea con el runtime
  existente, y suficiente para el criterio. La versión future-based
  real (event loop asyncio persistente compartido) queda como
  deuda menor — el `Value::Future` shape ya es estable, sólo
  cambia la implementación interna.
- **El GIL serializa Python** (esperado por roadmap): N awaits
  concurrentes a corutinas distintas se serializan en el GIL. Para
  APIs DB-bound (caso típico SQLAlchemy/asyncpg con queries
  cortas), la DB es el cuello de botella, no el GIL. Para APIs
  CPU-intensivas con NumPy o long-running asyncio.gather, deuda
  menor.
- **Sin marshaling Future Fitz → corutina Python**: pasar un
  `Value::Future` Fitz como arg a una función Python no se
  soporta (Future no es marshalleable, igual que Range/Function).
  Caso típico afectado: `asyncio.gather(fut1, fut2)` desde Fitz no
  funciona si los futs vienen de calls Python anteriores. Trade-off
  documentado en el ejemplo.
- **No incluimos caso runnable de excepción asyncio** en el
  ejemplo: definir una `async def` Python custom desde Fitz
  requiere un archivo helper Python aparte (el `from python
  import` carga módulos top-level, no archivos del usuario). El
  patrón de manejo de errores es idéntico al de calls sync (8.3) —
  documentado en notas del ejemplo.

**Cierre formal**:

  - Sin feature: **1193 unit** (sin cambios — bridge async es
    feature-gated) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1284 unit** (1281 + 3 del bridge async) + 80 + 3.
  - Clippy `-D warnings` limpio en ambos modos.

Detalle completo: `docs/roadmap.md` → "Fase 8.6".

## [v0.8.6] — 2026-05-15 — Fase 8.5: `fitz py-types` auto-mapeo SQLAlchemy → type Fitz

Quinto sub-paso de la Fase 8 (Interop Python). Cierra la
ergonomía del caso canónico SQLAlchemy: un comando nuevo
`fitz py-types <archivo.py> [--out <archivo.fitz>]` introspecciona
un archivo Python con modelos SQLAlchemy (o mocks equivalentes)
y emite los `type` Fitz correspondientes, listo para commitear.
Reduce el doble-tipado (Python + Fitz) — escribís los modelos UNA
vez en Python y Fitz los importa con sus tipos resueltos.

- **8.5.1 — Sub-comando + introspección + mapping**:
  - `Commands::PyTypes { source, out }` en el CLI con flag opcional
    `--out` (default: stdout).
  - Nuevo módulo `src/py_types.rs` feature-gated. Usa PyO3
    in-process (no subprocess) reusando el GIL + dep ya disponible
    con `--features python`. Sin la feature, el sub-comando aborta
    con error claro citando `cargo install --features python`.
  - `generate_from_file(source) -> Result<String, String>`:
    canonicaliza el path, importa el archivo Python via
    `importlib.util.spec_from_file_location` + `module_from_spec`
    + `loader.exec_module`, itera el `__dict__` filtrando clases
    definidas en ESE módulo (filtra re-exports SQLA como `Base`,
    `Column`, `Integer`, etc.). Duck typing: clase tiene que tener
    `__table__.columns` para contar como modelo — compatible con
    SQLAlchemy real Y con mocks que cumplan el contract.
  - Mapping por nombre canónico de la clase de `Column.type`:
      Integer/BigInteger/SmallInteger/INTEGER/...   → Int
      Float/Numeric/Double/REAL/FLOAT/NUMERIC       → Float
      String/Text/Unicode/VARCHAR/TEXT/CHAR/CLOB    → Str
      Boolean/BOOLEAN                               → Bool
      DateTime/Date/Time/TIMESTAMP/DATE/TIME        → Str (ISO 8601)
      resto                                         → Any + `// ?` comment
  - `nullable=True` → sufijo `?`. `default=<literal>` (Int/Float/
    Str/Bool/None) → inline `= valor`. Defaults callable
    (`datetime.utcnow`) se ignoran silenciosamente — emitir
    `= func()` no aporta.
  - 10 tests con classes Python mock que cumplen el shape SQLA sin
    requerir `pip install sqlalchemy` (modelo simple, mapping
    completo de tipos, nullable, default literal, default callable
    ignorado, tipo desconocido con comentario, múltiples modelos,
    archivo sin modelos error claro, clases sin `__table__`
    filtradas, header cita fuente).
- **8.5.2 — Ejemplo runnable + cierre formal**:
  - `examples/py-types/models.py` autosuficiente: 25 LoC de mock
    SQLAlchemy (clases `Column`, `_Table`, `Integer`, etc.) +
    modelos `User` (6 campos: int, str, str, int?, bool=false,
    str-datetime) y `Order` (5 campos: bigint, int, float,
    str="USD", str?). Comentario explica cómo reemplazar el mock
    con `from sqlalchemy import ...` para uso real.
  - `examples/py-types/models.fitz` (generado y commiteado): el
    output de `fitz py-types models.py --out models.fitz`. Sirve
    como referencia del output esperado.
  - `examples/py-types/usage.fitz`: `from models import User, Order`
    + dos fns `parse_user`/`parse_order` que demuestran coerción
    runtime 8.4.3 sobre dicts JSON (`json.loads` Python → Map →
    User Instance). Cubre happy path con todos los campos,
    default `currency="USD"` aplicado en Order, nullable `notes`
    como Null, y JSON malformado propagado como `Result::Err`.
    Validado bit-a-bit con
    `cargo run --features python -- run examples/py-types/usage.fitz`.
  - CHANGELOG v0.8.6, roadmap.md actualiza 8.5 a CERRADA con
    sub-pasos detallados, deudas-post-5b.md nota de cierre,
    CLAUDE + README refresh.

**Decisiones técnicas tomadas al arrancar**:

- **In-process via PyO3** (no subprocess): reusa el GIL + dep
  PyO3 ya disponible. Más simple que armar subprocess management
  + parseo de output. Requiere `--features python`; sin la feature
  el sub-comando aborta antes.
- **Duck typing sobre `__table__.columns`** (no `isinstance(cls,
  DeclarativeBase)`): permite tests con mocks sin requerir
  SQLAlchemy real instalado. Funciona igual con SQLAlchemy real.
- **Solo SQLAlchemy en 8.5**: Django, Tortoise, peewee,
  dataclasses quedan como sub-comandos futuros si entra demanda
  (`fitz py-types-django`, etc.). La arquitectura es reusable —
  el dispatch va por shape del object, no por ORM específico.
- **Defaults callable ignorados** silenciosamente: emitir
  `= datetime.utcnow()` confunde más de lo que ayuda (no es
  evaluable estáticamente desde Fitz).
- **Tipos desconocidos → `Any` con comentario** `// ?` citando el
  nombre original SQLA. Permite al usuario detectar y refinar a
  mano (ej. `JSON` → `Map<Str, Any>`).
- **Output a stdout por default**; `--out <archivo>` opcional.
  El archivo generado lleva header `// Generado por fitz py-types
  — no editar a mano` + cita de la fuente — facilita el flujo
  "commitear el .fitz, regenerar si cambia el schema".
- **Sin verificación de drift** entre `.py` y `.fitz` generado
  (regeneración manual cuando el schema cambia). Linter de drift
  queda para Fase 9+ si entra demanda.

**Cierre formal**:

  - Sin feature: **1193 unit** (sin cambios — `py_types` es
    feature-gated) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1281 unit** (1271 + 10 nuevos en `py_types`)
    + 80 + 3.
  - Clippy `-D warnings` limpio en ambos modos.

Detalle completo: `docs/roadmap.md` → "Fase 8.5".

## [v0.8.5] — 2026-05-15 — Fase 8.4: Tipos del checker + anotaciones del lado Fitz

Cuarto sub-paso de la Fase 8 (Interop Python). Cierra el ciclo
"call Python → tipo Fitz concreto" con tres cambios coordinados:
el checker estático ahora distingue valores Python de Any
genérico (`Type::PyAny`), refina los calls a `Result<Any>`
forzando manejo de errores estático, y el runtime coerciona
`Value::Map` → `Value::Instance` cuando hay anotación nominal en
el binding. Habilita el patrón canónico del roadmap:

```fitz
fn fetch_user(s: Str) -> Result<User> {
    let row: User = json.loads(s)?
    return Ok(row)
}
```

Una sola anotación (`: User`) basta para salir del "limbo Python"
a tipos Fitz concretos. El runtime valida que el dict tenga los
campos requeridos.

- **8.4.1+8.4.2 — `Type::PyAny` en el checker + calls Python tipan
  `Result<Any>`** (combinados en un commit, ~5 LoC del refinamiento
  del call ya estaban listos): nueva variante `Type::PyAny` con
  identidad propia (vs `Any` genérico), bidireccionalmente
  compatible con cualquier tipo igual que Any. `Stmt::Import` y
  `Stmt::FromImport` con `path[0] == "python"` tipan los bindings
  como `PyAny`; imports normales siguen como `Any`. Field access
  sobre PyAny devuelve PyAny (permite chaining como `os.path`).
  `Expr::Call` con receptor PyAny (callee o `Field.object`) refina
  el ret type a `Type::Result(Box::new(Type::Any))` — activa
  estáticamente la regla de exhaustividad sobre Result (5.3.3) y
  la regla del operador `?` (5.3.3). 9 tests nuevos del checker.
- **8.4.3 — Coerción runtime Map → Instance con anotación**:
  `Stmt::Assign` con `target: Ident` y anotación dispara
  `coerce_to_annotation(annot, value, env)` antes de bindear.
  Si la anotación es `Named(T)` o `Nullable(Named(T))` con T
  nominal, y el value es `Value::Map`, construye una `Value::
  Instance` validando que los fields matcheen el `type` declarado.
  Reglas: nullable + Null → passthrough; value no-Map (Instance
  ya, primitivo, etc.) → passthrough; resuelve fields en orden
  (`provided` → `resolved_defaults` PreF8.3 → `default` Expr →
  nullable Null → error claro). Campos extras del Map se ignoran
  silenciosamente (Python suele devolver dicts con más data de la
  necesaria; ser permisivo evita fricción). Field requerido
  faltante (no nullable, sin default) → `FitzError` que aborta
  con mensaje citando type + field. 9 tests nuevos (8 sin feature,
  1 con feature validando el criterio canónico end-to-end via
  json.loads).
- **8.4.4 — Ejemplo runnable + cierre formal**: nuevo
  `examples/python-interop-8.4.fitz` con 5 secciones (happy path,
  nullable faltante → Null, extras ignorados, JSON malformado
  propagado por `?`, default aplicado) más comentario explícito
  sobre el caso "field requerido faltante" que aborta por
  diseño. CHANGELOG v0.8.5, roadmap actualiza Fase 8.4 a CERRADA,
  deudas-post-5b nota de cierre, CLAUDE + README refresh.

**Decisiones técnicas tomadas al arrancar**:

- **`Type::PyAny` dedicado** (no `Type::Any` genérico ni
  `Type::PyObject<"...">`). Empezar simple, refinar a fantasma si
  entra demanda (roadmap recomienda).
- **Coerción Map → Instance vive en el evaluator**, no en el
  checker. El checker ya acepta el cast (gradual Any → T). El
  runtime hace la coerción real con validación de fields.
- **Campos extras del dict se ignoran silenciosamente**. Python
  suele devolver más data de la necesaria; ser permisivos evita
  fricción. Documentado en el ejemplo.
- **Field requerido faltante → FitzError que aborta** (no
  `Result::Err`). Diseño: este caso indica datos malformados a
  nivel de fuente (DB schema desalineado, API contract roto), no
  un error de runtime esperable como una excepción Python. El
  programador debe validar el dict antes o declarar el campo
  nullable/con default.
- **El test `?` operator solo se chequea adentro de fn que
  retorna `Result<...>`** (regla heredada de 5.3.3). `?` a top-
  level se reporta en runtime, no en el checker — comportamiento
  consistente con calls nativas Fitz.

**Cierre formal**:

  - Sin feature: **1193 unit** (+ 9 checker + 8 coerción + 1 fix
    test 8.3 sin contar baseline) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1271 unit** (+ 1 criterio canónico end-to-end
    via json.loads) + 80 + 3.
  - Clippy `cargo clippy --all-targets --features python -- -D warnings`
    limpio. Idem sin feature.

Detalle completo: `docs/roadmap.md` → "Fase 8.4".

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
