# Changelog

Cambios visibles del lenguaje Fitz, agrupados por hito. Sigue el
formato de [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
El detalle técnico de cada sub-paso vive en
[`docs/roadmap.md`](docs/roadmap.md); este archivo es la vista
condensada para alguien que pregunta "¿qué cambió y cuándo?".

Las versiones son retroactivas — Fitz todavía no publica releases
formales; cada bump corresponde al cierre de una Fase del roadmap.

## [Sin publicar]

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
