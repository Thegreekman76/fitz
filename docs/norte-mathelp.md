# Norte — fitz (core del lenguaje)

Backlog técnico surgido de la auditoría hecha para **MatHelp** (juego de matemática
mobile-first, i18n es-AR + en, Postgres, 100% Fitz). Primer proyecto real de terceros
apoyado de punta a punta en el stack.

Fecha del análisis: **2026-08-19** · Actualizado: **2026-08-20** (hallazgos de la fase F0)
Versión auditada del core: **v0.48.0** (repo) / **v0.47.0** (binario compilado por el autor).

Documento vivo: marcá los checkboxes al implementar. IDs estables — no renumerar.
El archivo hermano con las tareas del framework es `fitz-liveviews/docs/norte-mathelp.md`.

> **Nota de renumeración (2026-08-20):** en la primera versión yo había usado `FITZ-09`
> (Map.remove) y `FITZ-10` (differ de paridad) como *hallazgos propios*. El autor asignó
> `FITZ-09..12` a hallazgos nuevos de mayor impacto (con repro real sobre el compilador).
> Esos IDs ganan; mis dos hallazgos se renumeraron a **FITZ-13** (Map.remove) y **FITZ-14**
> (differ de paridad). Las referencias cruzadas están actualizadas.

---

## Resumen ejecutivo

Auditoría original (A1–A8): 6 confirmados, 1 muta (A4), 1 refutado a medias (A5).
**Fase F0 sumó cuatro hallazgos que salieron de correr el compilador, no de leer docs** — y
tres de los cuatro son la misma clase de bug. Eso **sube T2 (paridad) por encima de features nuevas**.

- **FITZ-01 (`rand`) sigue siendo el único bloqueante para *empezar* MatHelp.** Nada cambia acá.
- **FITZ-09 (codegen de `T?`) es un ALTO nuevo y urgente.** Una función `-> Str?` con `return null`
  genera Rust roto (`return ()` donde va `return None`, valor sin `Some(...)`). No es evitable en el
  código de la app: **`flv_cookie`, dentro de fitz-liveviews, tiene el patrón exacto** (`lib.fitz:2430`),
  así que **cualquier app que dependa del framework y compile a nativo choca**. Verificado: `examples/admin`
  importa el framework entero → **no compila con `fitz build`**, solo con `fitz run`. Consecuencia en
  MatHelp: el Docker corre el intérprete en vez del binario (se pierde ~9x + distroless).
- **FITZ-10 (`Str + Any`) y FITZ-06 (`preload`) completan la clase check✓/build✗.** Tres divergencias
  interpretado↔compilado en una tarde. No es mala suerte: **no hay nada que las detecte**.
- **FITZ-14 (differ de paridad) sube de Medio a Alto y se mueve al Hito 1.** Es la sugerencia del autor
  y la comparto: un test que corra el corpus de `examples/` + `boilerplates/` por las dos vías y diffee
  la salida hubiera cazado los tres solos. Un lenguaje que a veces se comporta distinto al compilar es un
  problema de *confianza*, no de features.
- **FITZ-11 (la imagen oficial de Docker no trae `git`)** — con `git` deps como única forma de dep
  externa hoy, la imagen no puede construir ningún proyecto con dependencias. Fix de una línea.
- **A4 confirmado empíricamente sobre el binario:** los format specs **compilan** (`{n:,}` → `1,234,567`);
  la doc `guide.md:1266` mentía y ya la corregí. Lo que queda de FITZ-04 es **solo locale** (`1.234.567,00`
  argentino) — y para un juego de matemática, mostrar mal los decimales es enseñar mal.
- **A5 confirmado empíricamente:** `@post` acepta `form-urlencoded` → login zero-JS. Comentario stale de
  `examples/admin/src/auth.fitz` ya corregido.

**Recomendación de arranque:** `FITZ-01 (rand)` para desbloquear el juego. En paralelo, el bloque de
**confianza**: `FITZ-14 (differ)` + `FITZ-09 (T?)` + `FITZ-10 (Str+Any)` — los tres van juntos porque el
differ es la red que protege a los otros dos (y a preload). `FITZ-09` además destraba el binario nativo
de todo lo que use fitz-liveviews, MatHelp incluido.

---

## Tabla priorizada (unificada cross-repo, re-priorizada 2026-08-20)

Misma tabla en los dos archivos; acá se detallan solo las fichas de fitz core.
Filas `FLV-*` viven en `fitz-liveviews/docs/norte-mathelp.md`.

| #  | ID       | Tarea                                   | Estado              | Impacto     | Costo | Riesgo | Depende / desbloquea |
|----|----------|-----------------------------------------|---------------------|-------------|-------|--------|----------------------|
| 1  | FITZ-01  | Módulo `rand` (CSPRNG + seeded)         | Confirmado          | Bloqueante  | M     | Ninguno| —                    |
| 2  | FITZ-09  | Codegen: `-> T?` emite `None`/`Some`    | Confirmado (repro)  | **Alto**    | S     | Bajo   | **desbloquea FLV-10, native builds** |
| 3  | FITZ-14  | Differ de paridad `run`↔`build` `[antes FITZ-10]` | Confirmado| **Alto** (↑)| M     | Ninguno| protege 09/10/06     |
| 4  | FITZ-11  | `git` en la imagen oficial de Docker    | Confirmado (repro)  | Medio-Alto  | S     | Ninguno| —                    |
| 5  | FITZ-10  | `Str + Any`: check✓ / build✗            | Confirmado (repro)  | Medio       | S     | Bajo   | —                    |
| 6  | FITZ-05  | API de cookies (`@cookie` + `Response.cookies`) | Ya resuelto | Alto        | M     | Bajo   | —                    |
| 7  | FITZ-03  | Módulo `fs`                             | Confirmado          | Alto        | M     | Bajo   | habilita **T1**      |
| 8  | FITZ-04  | Formateo de números con locale          | Parcial (paridad OK)| Alto        | M     | Bajo   | **T1**               |
| 11 | FITZ-02  | Servido de estáticos (`@server(static_dir=)`) | Ya resuelto   | Medio       | M     | Bajo   | habilita **T3**      |
| 13 | FITZ-13  | `Map.remove` `[antes FITZ-09]`          | Ya resuelto         | Medio       | S     | Bajo   | **desbloquea FLV-03**|
| 19 | FITZ-06  | `.preload()` en el intérprete (error claro) | Ya resuelto (MVP)| Medio       | M     | Bajo   | **T2**               |
| 20 | FITZ-07  | `.is_in(<var>)` → `= ANY($n)`           | Ya resuelto         | Bajo        | S     | Bajo   | —                    |
| 21 | FITZ-12  | Paréntesis redundantes en el `match` generado | Ya resuelto   | Bajo        | S     | Ninguno| —                    |
| 22 | FITZ-08  | ENUM nativo de Postgres                 | Confirmado          | Bajo        | L     | Medio  | —                    |

Estado ∈ `Confirmado` · `Parcial` · `Refutado` · `Ya resuelto`
Impacto ∈ `Bloqueante` · `Alto` · `Medio` · `Bajo` · Costo ∈ `S` (horas) · `M` (días) · `L` (semana+)
(los `#` saltados son filas `FLV-*` — ver el archivo hermano para la tabla completa)

---

## Fichas

### FITZ-01 · Módulo `rand`

- [x] Implementado (2026-08-20) — módulo `src/rand.rs` (SplitMix64 fijo, no el crate `rand`), `Value::RandGen` + `Type::RandGen`, intérprete + **codegen con paridad bit-a-bit** (`RAND_PRELUDE_CORE`/`_GLOBAL`, getrandom inyectado en Cargo.toml para el path global), checker (`rand` = `Type::Any`, codegen re-deriva `RandGen`), LSP completions, ejemplo runnable `13w-random.fitz`, test E2E de reproducibilidad + smoke verde. **Limitaciones del MVP** (follow-ups, NO divergencias silenciosas): (a) `rand` en codegen solo en el programa principal (cross-module = follow-up; el intérprete ya lo soporta); (b) el receptor de un método `RandGen` debe ser una var local (`let r = rand.seeded(...)`), no un receptor complejo; (c) un `match r.sample(...) { Ok(v) => v, Err(_) => "x" }` heterogéneo en posición de valor cae al gap pre-existente de "bare Any en CLI" (usar matches homogéneos o en posición de statement).
- **Estado:** Confirmado.
- **Evidencia:** `src/evaluator.rs:246-304` (`builtin_names()` — lista cerrada, no hay `rand`);
  `src/lsp.rs:4204-4239` (módulos built-in, no hay `rand`); `src/value.rs:699` (única fuente de azar =
  `Uuid.v4()`). `rand_core` en `Cargo.toml:174` solo para salt de Argon2 / nonce SCRAM / UUID v4.
- **Impacto en un usuario real:** el núcleo de MatHelp es "generar un ejercicio aleatorio según la
  destreza del chico". Sin azar no hay juego. Caso obvio también de simulaciones, sampling, jitter, shuffles.
- **Workaround hoy:** xorshift32 propio sembrado con `Uuid.v4()` hasheado (~40 líneas). Anda, frágil, no
  reusable, y no es CSPRNG (no sirve para tokens).
- **Propuesta (API cerrada):**
  ```fitz
  // Global — CSPRNG (getrandom/OsRng). No reproducible.
  rand.int(min, max) -> Int             // inclusivo ambos extremos
  rand.float() -> Float                 // [0, 1)
  rand.bool() -> Bool
  rand.choice(xs) -> Result<T>          // Err si vacía
  rand.shuffle(xs) -> List<T>           // copia barajada (Fisher-Yates)
  rand.sample(xs, n) -> Result<List<T>> // sin repetir; Err si n > len
  rand.bytes(n) -> Bytes                // CSPRNG, tokens

  // Seeded — PRNG rápido, determinístico, NO cripto.
  let r = rand.seeded(12345)            // -> RandGen (valor con estado, como DbConn)
  r.int(1, 100)  r.float()  r.bool()  r.choice(xs)  r.shuffle(xs)  r.sample(xs, n)
  ```
- **Criterio de aceptación:** `rand.seeded(N)` produce **la misma secuencia** en `fitz run` y `fitz build`,
  para siempre. Por eso el PRNG sembrado **no** debe delegar en `StdRng` del crate `rand` (algoritmo no
  estable entre versiones): fijar uno simple y bien especificado (PCG-XSH-RR 64/32 o SplitMix64),
  implementado idéntico en evaluador y codegen. El CSPRNG global usa `getrandom`. Esta garantía habilita
  guardar `seed + índice` y reconstruir una partida entera desde dos enteros.
- **Archivos a tocar:** nuevo `src/rand.rs` (algoritmo compartido); `src/evaluator.rs:246`
  (módulo + builtins + `Value::RandGen`); `src/value.rs`; `src/types.rs`; `src/lsp.rs:4204`;
  `src/codegen.rs` (preludio `__fitz_rand_*` + dispatch + `Cargo.toml` con `getrandom`); `docs/guide.md`.
- **Tests:** `@test` de secuencia sembrada fija; unit del rango/sample/choice; **E2E de paridad**
  (ver FITZ-14 — el criterio de rand *es* paridad).
- **Docs:** capítulo "Aleatoriedad" en `docs/guide.md` (panorama vecino; CSPRNG vs seeded; patrón replay).
- **Dependencias:** ninguna. Desbloquea el arranque de MatHelp.
- **Notas de diseño:** separar CSPRNG (`rand.*`) de seeded (`rand.seeded()`) es la decisión central.
  Descartado exponer `StdRng` (no reproducible) y un `rand.seed_global()` (estado global).

---

### FITZ-09 · Codegen: las funciones que devuelven `T?` compilan mal

- [x] Implementado (2026-08-20) — fix en `gen_return` + test E2E de paridad. Smoke ~360 ejemplos verde.
- **Estado:** Confirmado con repro mínimo (sobre el binario compilado, no docs).
- **Evidencia:** repro de 20 líneas del autor — `fn primera_parte(s: Str?) -> Str?` con `return null` y
  `return <valor>` pasa `fitz check` ✓, corre en `fitz run` ✓, y **falla `fitz build`** con `E0308`. El
  Rust generado emite dos defectos: `return ()` donde va `return None`, y el valor de retorno sin envolver
  en `Some(...)`. El propio `rustc` sugiere el fix (`help: try wrapping the expression in Some`).
  **La manifestación en el framework:** `flv_cookie(cookie: Str?, name: Str) -> Str?`
  (`fitz-liveviews/src/lib.fitz:2430`) tiene el patrón exacto (`null => { return null }`, `c => c`,
  `return null` de tail) → el Rust generado `fitz_liveviews.rs:1432` no compila. Verificado que
  `examples/admin` importa el framework entero (`auth.fitz:17`, `dashboard.fitz:15`, ...) con dep
  `{ path = "../.." }` → **`examples/admin` no compila con `fitz build`** (solo `fitz run`).
- **Impacto en un usuario real (ALTO, no medio):** no es evitable en el código de la app —
  **cualquier proyecto que dependa de fitz-liveviews y compile a nativo choca**, haga lo que haga. Y
  `flv_cookie` no es marginal: resuelve el locale y la sesión desde la cookie del handshake, lo que
  necesita toda app con i18n o auth. La app insignia del framework no compila a nativo. En MatHelp, el
  Dockerfile corre el intérprete en vez del binario (se pierde ~9x de performance y el runtime distroless;
  la versión compilada quedó comentada).
- **Workaround hoy:** correr el intérprete en Docker (funciona, pero pierde perf + distroless). En el
  código propio, evitar `-> T?` con `return` adentro de un `match` — pero **no evitable** para el código
  de la librería.
- **Propuesta:** aplicar la coerción nullable en **posición de `return`** dentro de una fn cuyo return
  type es `Nullable`. `return null` → `return None`; `return <v>` → `return Some(<coerced v>)`; idem el
  tail. El codegen **ya** tiene la coerción `(T → T?) ⇒ Some(...)` y `(Null → T?) ⇒ None` para
  asignaciones y campos — el bug es que no la invoca en la posición de return.
- **Criterio de aceptación:** (1) el repro compila con `fitz build` y `GET /` devuelve `"a"`;
  (2) `fitz-liveviews` compila entero, `flv_cookie` incluida, y `examples/admin` compila y corre igual que
  interpretado (ese es también el cierre de **FLV-10**); (3) el repro da salida idéntica por `fitz run` y
  `fitz build`.
- **Archivos a tocar:** `src/codegen.rs` — la emisión de `Stmt::Return` (buscar `gen_return`/el brazo de
  `Return`): cuando el frame de return type es `Type::Nullable(_)`, envolver el valor con la coerción
  `Some(...)` y emitir `None` para `return null`. Reusar el helper `coerce(...)` existente.
- **Tests:** el repro como E2E en `tests/compile_e2e.rs`; unit de `gen_return` con ret `Nullable`
  (`return null` → `None`, `return v` → `Some(v)`); **regresión de `flv_cookie`**: un mini programa que
  importe fitz-liveviews y llame `flv_cookie`, que hoy no compila y debe compilar.
- **Docs:** — (bug interno del codegen; sin cambio de superficie).
- **Dependencias:** **desbloquea FLV-10** (flv_cookie native), el native build de `examples/admin`, y el
  Dockerfile compilado de MatHelp. Lo caza **FITZ-14** (differ).
- **Notas de diseño:** `rustc` literalmente sugiere el fix — es la coerción nullable faltante en la única
  posición donde el codegen no la aplica. Fix chico (S), impacto alto. Debe landear **con** FITZ-14 para
  quedar protegido contra regresión.

---

### FITZ-14 · Differ de paridad `fitz run` ↔ `fitz build` `[antes FITZ-10]`

- [x] Implementado (2026-08-20) — MVP: helper `run_interpreter` + `run_build_parity_corpus_fitz14` con 13 ejemplos CLI-puros diffeados `run`↔`build`. Extensible. (Corpus curado, no los 360; ver notas.)
- **Estado:** Confirmado (hallazgo estructural; sube de Medio a Alto con la fase F0).
- **Evidencia:** en `tests/compile_e2e.rs`, todos los `Command::new(&bin)` (`:2288`, `:2322`, `:2378`,
  `:2569`, `:2613`, `:11150`) ejecutan el **binario compilado**; ninguno corre `fitz run` sobre el mismo
  fuente para diffear. El smoke `GUIDE_EXAMPLES_COMPILE` solo valida que compile. La paridad se asegura
  test por test hard-codeando la salida. La fase F0 encontró **tres** divergencias en una tarde
  (FITZ-09, FITZ-10, FITZ-06) — la prueba de que no hay red.
- **Impacto en un usuario real:** los bugs "mismo código, dos comportamientos" son la clase que más
  rápido quema la confianza porque aparecen recién en producción — el usuario dockerizando a las once de la
  noche. Sin differ, el próximo lo encuentra un usuario, no el CI.
- **Workaround hoy:** ninguno — es deuda de infraestructura de tests.
- **Propuesta:** un harness que tome el corpus de `examples/**` + `boilerplates/**` (ya son ~360 archivos),
  corra cada uno CLI-puro por `fitz run` y por `fitz build && ./bin`, y **assertee stdout idéntico
  bit-a-bit**. Allowlist explícita para los que legítimamente difieren o no aplican (HTTP servers, tiempo,
  aleatoriedad no-sembrada).
- **Criterio de aceptación:** un `cargo test` que falla si cualquier ejemplo del corpus produce salida
  distinta entre `run` y `build`. Corre en CI. Habría cazado FITZ-09, FITZ-10 y FITZ-06 solo.
- **Archivos a tocar:** `tests/parity_e2e.rs` nuevo con `run_and_build_diff(fuente)`; corpus + allowlist.
- **Tests:** el harness *es* el test.
- **Docs:** nota en `docs/architecture.md` (testing).
- **Dependencias:** ninguna. Pilar de **T2**. Protege FITZ-01 (criterio = paridad), FITZ-09, FITZ-10,
  FITZ-06.
- **Notas de diseño:** empezar por los ejemplos CLI-puros (deterministas). El corpus crece con cada feature
  nueva. Vale más que cualquier arreglo puntual — es lo que convierte "arreglamos tres bugs" en "no vuelve a
  pasar".

---

### FITZ-11 · La imagen oficial de Docker no trae `git`

- [x] Implementado (2026-08-20) — `git` agregado al `apt-get install` de la imagen estándar (build-capable) en `.github/workflows/release.yml`. Se ejercita en el próximo tag de release. El `--dep-override` queda como enhancement opcional diferido (no era un bug confirmado). El `-python` runtime es run-only (sin cargo), no lo toca.
- **Estado:** Confirmado en un build real.
- **Evidencia:** el build del autor sobre `ghcr.io/thegreekman76/fitz` falla en `RUN fitz build` con
  `could not invoke git (No such file or directory, os error 2)`. La imagen existe y baja bien, pero sin
  `git` adentro `fitz build` no puede resolver ninguna dep `{ git = ... }`. Y hoy `{ git = ... }` es la
  **única** forma de dep externa: el registry público no existe y `{ path = ... }` no sirve en un container.
- **Impacto en un usuario real:** la imagen oficial **no puede construir ningún proyecto con dependencias**
  — justo el caso que la imagen existe para resolver. MatHelp depende de fitz-liveviews por git.
- **Workaround hoy:** etapa `vendor` sobre Alpine que clona fitz-liveviews + un `fitz.docker.toml` paralelo
  con la dep apuntada a `{ path = "/vendor/..." }`. Funciona, pero obliga a **dos manifiestos en sincronía**
  — trampa clásica para un olvido.
- **Propuesta:** (1) una línea en el Dockerfile de la imagen:
  `apt-get install -y --no-install-recommends git ca-certificates` (el `ca-certificates` para clonar por
  https). (2) Evaluar `fitz build --dep-override nombre=ruta`, que evitaría el manifiesto duplicado en
  cualquier build containerizado (y sirve a cualquier CI).
- **Criterio de aceptación:** `docker run ghcr.io/thegreekman76/fitz fitz build` sobre un proyecto con una
  git dep resuelve la dep y compila. Con `--dep-override`, el mismo proyecto compila apuntando la dep a un
  path sin tocar el `fitz.toml`.
- **Archivos a tocar:** el Dockerfile de la imagen oficial (repo `fitz`, `docker/` o el workflow que la
  buildea); opcionalmente `src/main.rs` + `src/manifest.rs` para `--dep-override`.
- **Tests:** smoke de CI que buildee un proyecto con git dep dentro de la imagen; unit de `--dep-override`.
- **Docs:** `docs/guide.md` (cap de deployment / imagen Docker).
- **Dependencias:** ninguna. Alto apalancamiento para cualquiera que dockerice (costo de una línea).
- **Notas de diseño:** el `--dep-override` es la mejora ergonómica de fondo: hoy el registry no existe, así
  que los builds containerizados con deps son inevitablemente incómodos; el override los limpia sin esperar
  el registry.

---

### FITZ-10 · `Str + Any`: lo acepta `check`, lo rechaza `build` `[antes FITZ-13 no — nuevo ID del autor]`

- [x] Implementado (2026-08-20) — `Str + Any` en `gen_binop` (coerce Any→Str, paridad con el intérprete) + detección de `List<Any>`/`Map<_,Any>` vía `TypeInfo` para emitir el preludio `__FitzValue` en CLI. Test E2E + smoke verde.
- **Estado:** Confirmado.
- **Evidencia:** repro del autor — `let chars = []` (infiere `List<Any>`), `chars.push(c)` con `c: Str`,
  y luego `out + chars[0]` → `fitz check` ✓, `fitz build` ✗ con `codegen: operador + no aplicable a Str y
  Any en codegen`. El checker infiere `Any` para el `[]` vacío y no propaga hacia atrás desde el `push`, o
  el codegen es más estricto que el checker.
- **Impacto en un usuario real:** Medio. Otra divergencia check✓/build✗ — el usuario se entera al compilar.
  Misma familia que FITZ-09 y FITZ-06.
- **Workaround hoy:** anotar `let chars: List<Str> = []`.
- **Propuesta:** consistencia — o (a) el checker infiere el tipo del elemento desde el primer `.push()`
  (back-propagation) y así `chars[0]` tipa `Str`, o (b) el codegen coacciona `Str + Any` como el intérprete.
  Preferible (a): que el checker no acepte lo que el build rechaza. Lo que no puede pasar es la divergencia.
- **Criterio de aceptación:** el repro compila sin anotación (inferencia), **o** `fitz check` lo rechaza con
  un mensaje claro. Nunca pasar check y fallar build.
- **Archivos a tocar:** `src/types.rs` (inferencia del elemento de un `List<Any>` vacío desde el `push`)
  y/o `src/codegen.rs` (operador `+` con operando `Any`).
- **Tests:** el repro como caso de checker + E2E; lo caza además **FITZ-14**.
- **Docs:** — (comportamiento interno).
- **Dependencias:** ninguna. Parte de **T2**.
- **Notas de diseño:** la back-propagation del tipo de elemento desde `push`/`insert` es lo más útil (mejora
  la inferencia en general), pero cualquier resolución que elimine la divergencia sirve. Lo importante es la
  consistencia checker↔codegen, no cuál de los dos cede.

---

### FITZ-05 · API de cookies (`@cookie` + `Response.cookies`)

- [x] Implementado (2026-08-20) — **FASE A** (leer con `@cookie(name="X")`, param `Str`/`Str?`,
  opcional `into="alias"`, sobre `@get`/`@post`/`@ws`; `parse_cookie_header`; paridad run↔build;
  OpenAPI `in: cookie`) + **FASE B** (nominal built-in `Cookie` de 8 campos con defaults + campo
  `Response.cookies: List<Cookie>`; cada `Cookie` → un `Set-Cookie`; helper compartido
  `serialize_set_cookie`/`__fitz_serialize_set_cookie` con paridad bit-a-bit; **fix del `.insert`→
  `.append` en `outcome_to_response`** para que múltiples Set-Cookie sobrevivan; LSP + guía cap 17
  "Cookies y sesiones"). Tests: 5 unit http (serialización) + 1 E2E oneshot intérprete (valida el
  `.append`) + 1 E2E codegen raw-TCP (2 Set-Cookie, paridad) + 3 unit codegen del prelude + 1 LSP.
  **Bonus (post-v0.49.0, 2026-08-20):** cerrado el gap de paridad descubierto acá —
  `parse_urlencoded_body` ahora coerce el body `form-urlencoded` al `type` del handler
  (paralelo a `parse_body`/JSON y al `__parse_urlencoded`→`__from_fitz_json` del codegen), así el
  login zero-JS funciona igual en `fitz run` y `fitz build` (2 tests nuevos + smoke bit-a-bit).
- **🟡 RESIDUAL DESCUBIERTO (2026-08-20, durante FLV-09 de fitz-liveviews): `@cookie` NO funciona sobre
  `@ws`.** La ficha dice "sobre `@get`/`@post`/`@ws`" pero el `@ws` está **incompleto**: el checker de
  aridad `check_ws_handler` (`src/types.rs:11532-33`) cuenta `WsConn<T>` + `@header`, pero **no incluye
  `@cookie`** → un `@cookie(name="X") @ws(...)` falla con "expects 1 param (1 WsConn + 1 per @header),
  received 2". Además el binding runtime de cookies (`src/http.rs:4547`, `parse_cookie_header`) vive en el
  path HTTP `dispatch_request`, NO en el path WS del `on_upgrade` — el evaluator `register_ws_route` SÍ
  guarda las cookies (`src/evaluator.rs:2424,2579`) y `collect_cookies` acepta `@ws` (`:3684`), pero el
  handler WS no las lee del handshake. Fix completo (multi-archivo, con verificación end-to-end): (1)
  checker suma `cookie_count` a `expected_params`; (2) runtime WS bindea la cookie del header del upgrade
  (paralelo a cómo bindea `@header` en el WS path); (3) codegen del wrapper `@ws` idem. **Workaround
  documentado en `fitz-liveviews/docs/i18n.md`**: leer el locale con `@header(name="cookie")` +
  `locale_from_cookie` sobre `@ws` (funciona hoy, es lo que hace el admin). NO bloquea el i18n (el
  workaround es completo), solo es menos ergonómico.
- **Estado:** Ya resuelto (FASE A + FASE B + paridad form-urlencoded en el intérprete).
  Form-urlencoded (mitad del A5 original): **REFUTADO — soportado en `fitz build` y ahora
  también en `fitz run` con coerción al `type`.**
- **Evidencia (cookies sin API):** `src/http.rs:1310-1341` (`HandlerOutcome` sin campo `cookies`);
  sin `@cookie`/`Cookie` en `src/parser.rs`/`src/value.rs`. `docs/guide.md:11436` lista sessions
  cookie-based como futuro.
- **Evidencia (form-urlencoded — REFUTADO):** `src/http.rs:4077-4126` + codegen `src/codegen.rs:33359`,
  helper `__parse_urlencoded` (`:36113`). El autor lo verificó en un probe compilado. `<form method=POST>`
  nativo deserializa al mismo `type` que hoy recibe JSON, **sin JS**. (Comentario stale de
  `examples/admin/src/auth.fitz` ya corregido.)
- **Impacto en un usuario real:** la cookie es el único mecanismo viable para el login de familia (el
  browser no manda `Authorization` en navegación normal ni en el handshake WS). Con esto + form-urlencoded
  (ya está), el login de MatHelp es HTML puro. Sin API, cada uno reescribe el parser de `Cookie` — riesgo
  de seguridad.
- **Workaround hoy:** el del ejemplo admin (`auth.fitz` / `i18n.fitz`): concatenar `Set-Cookie` a mano y
  parsear `Cookie` con `.split(";")`/`.split("=")`. Probado, pero es el parser que no debería reescribir cada uno.
- **Propuesta (API cerrada):**
  ```fitz
  @cookie(name="session")
  @get("/")
  fn home(session: Str?) -> Response { ... }   // también sobre @ws (upgrade es HTTP)

  type Cookie {                 // nominal built-in, como Request / File / Response
      name: Str
      value: Str
      path: Str = "/"
      http_only: Bool = false
      secure: Bool = false
      same_site: Str = "Lax"    // "Strict" | "Lax" | "None"
      max_age: Int? = null       // segundos; null = session cookie
      domain: Str? = null
  }
  return Response {
      status: 303,
      cookies: [ Cookie { name: "session", value: token, http_only: true, max_age: 86400 } ],
      headers: { "Location": "/" },
  }
  ```
- **Criterio de aceptación:** `@cookie(name=X)` inyecta el valor parseado (o `null`) en `@get`/`@post`/`@ws`;
  cada `Cookie` del `cookies` se serializa a un `Set-Cookie` con flags correctos; paridad `run` ↔ `build`.
- **Archivos a tocar:** `src/types.rs` (nominal `Cookie` + validación `@cookie`, paralelo a `@header`);
  `src/value.rs`; `src/http.rs:1310` (`cookies` + `parse_cookie_header` + serialización + extractor);
  `src/codegen.rs`; `src/lsp.rs`; `docs/guide.md` cap 17.
- **Tests:** unit de `parse_cookie_header`; E2E de serialización; E2E de login form-urlencoded → `Set-Cookie`
  → `@cookie` de vuelta.
- **Docs:** `docs/guide.md` cap 17. Cruza con FLV-09.
- **Dependencias:** ninguna. Habilita que el admin y `docs/i18n.md` dejen de parsear cookies a mano.
- **Notas de diseño:** lectura por decorador (como `@header`), escritura por campo `cookies` (coherente con
  el `Response` sin estado). La mitad form-urlencoded **no requiere trabajo** — solo se documentó.

---

### FITZ-03 · Módulo `fs`

- [x] Implementado (2026-08-20) — `src/fs.rs` con los 8 builtins + intérprete + **codegen con paridad** (`FS_PRELUDE` sobre `std::fs`, sin deps extra) + checker (`fs` = Any) + LSP + sección de guía "Filesystem" + test E2E de roundtrip run↔build. Detector genérico nuevo `program_calls_module` (reusable). MVP: main-program (cross-module fs = follow-up); sin sandbox; JSON parse nativo no existe (i18n usa formato `clave=valor` con `.split`, o interop Python).
- **Estado:** Confirmado.
- **Evidencia:** `builtin_names()` (`src/evaluator.rs:246-304`) sin `fs`/`read_file`/`write_file`. La única
  lectura de disco expuesta es `load_env(path)` (`src/evaluator.rs:19017`, parser `:19065`) — solo puebla
  env vars. El tipo `File` (`src/types.rs:1618-1648`) es de `multipart/form-data`, no abre archivos.
- **Impacto en un usuario real:** rompe la arquitectura natural de i18n de MatHelp (catálogos `locales/*.json`
  leídos al boot). Sin `fs`, los catálogos son código compilado → agregar un idioma implica recompilar. También:
  templates de mail, seeds, CSV, logs — categoría entera que hoy solo se hace bajando a Python.
- **Workaround hoy:** script externo que compila los `.json` a `.fitz` + `@test` de claves faltantes. Paso de
  build que no debería existir.
- **Propuesta (API cerrada):**
  ```fitz
  fs.read(path) -> Result<Str>          fs.read_bytes(path) -> Result<Bytes>
  fs.write(path, content) -> Result<Null>   fs.append(path, content) -> Result<Null>
  fs.exists(path) -> Bool               fs.list(path) -> Result<List<Str>>
  fs.remove(path) -> Result<Null>       fs.mkdir_all(path) -> Result<Null>
  ```
  Lectura en **runtime** (no compile-time): `fs.read("locales/es-AR.json")` al boot + `json.loads`.
- **Criterio de aceptación:** los 8 builtins con paridad `run` ↔ `build` (mismos Ok/Err, mismos mensajes con
  el path citado); `fs.read` inexistente → `Err`, nunca panic; paths relativos al working dir del proceso.
- **Archivos a tocar:** `src/evaluator.rs:246` (módulo + 8 builtins); `src/types.rs`; `src/lsp.rs:4204`;
  `src/codegen.rs` (helpers `__fitz_fs_*`); `docs/guide.md`.
- **Tests:** round-trip write→read→remove; inexistente → `Err`; `fs.list`; E2E de paridad.
- **Docs:** `docs/guide.md` sección "Filesystem"; nota sobre working-dir en distroless.
- **Dependencias:** ninguna. **Habilita T1** (catálogos desde JSON) y feeds `FLV-09`.
- **Notas de diseño:** MVP sin sandbox (backend nativo, `fs` es esperable); modelo Deno (`--allow-read=`)
  como capa opt-in futura. Streaming de archivos grandes (`open()` con seek) fuera del MVP.

---

### FITZ-04 · Formateo de números con locale

- [x] Implementado (2026-08-20) — (1) doc stale de format specs YA corregida (guide.md:1266, apunta al módulo `num`); (2) módulo `num` (`src/num.rs`): `num.format`/`num.percent`/`num.currency` con es-AR + en-US, **paridad bit-a-bit** (`NUM_PRELUDE` misma lógica de agrupamiento) + checker (Any) + LSP + sección de guía + ejemplo `13x-num-locale.fitz` (smoke + corpus de paridad FITZ-14). MVP: args posicionales (kwargs `locale:` = futuro); tabla de locales embebida (sin ICU).
- **Estado:** Parcial. "Sin paridad run↔build" **REFUTADO** (confirmado empíricamente por el autor: `{n:,}`
  → `1,234,567` en el binario). "Sin locale" **CONFIRMADO**.
- **Evidencia (paridad — REFUTADO):** `src/codegen.rs:40167-40257` + helpers `:13445-13489` implementan
  grouping/exponente/general/char/percent; `docs/guide.md:5042-5078` documenta el cierre "Fm". La tabla
  stale de `docs/guide.md:1266` **ya la corregí** (2026-08-20). Probe del autor: `fitz build` de `{n:,}|{r:.1%}`
  → `"1,234,567|42.0%"`.
- **Evidencia (sin locale — CONFIRMADO):** esa misma salida `1,234,567` / `42.0%` en es-AR debería ser
  `1.234.567` / `42,0 %`. `__fitz_fmt_grouping(n, sep)` inserta cada 3 con el sep elegido, pero el decimal es
  siempre `.`; no hay formato europeo ni moneda.
- **Impacto en un usuario real:** en Argentina `1.234,5` y `$ 1.250`. Un juego de matemática que muestra
  `1,234.5` le está **enseñando mal** al chico. Pedagógico, no cosmético.
- **Workaround hoy:** `fmt_num(locale, x)` con `.replace()`. Default silencioso = número mal formateado.
- **Propuesta (API cerrada):**
  ```fitz
  num.format(1234.5, locale: "es-AR")             // "1.234,5"
  num.percent(0.42, locale: "es-AR", digits: 1)   // "42,0 %"
  num.currency(1250, locale: "es-AR", code: "ARS")// "$ 1.250,00"
  ```
  Separador decimal + de miles + símbolo de moneda + posición cubren el 95%. Precedente: `DateTime.in_tz(iana)`.
- **Criterio de aceptación:** `num.*` con `"es-AR"` da formato argentino, con `"en-US"` inglés; paridad
  `run` ↔ `build`.
- **Archivos a tocar:** `src/evaluator.rs` (módulo `num`); `src/codegen.rs` (`__fitz_num_*` con la misma
  tabla de locales); `src/types.rs`/`src/lsp.rs`. (La doc stale `guide.md:1266` ya está corregida.)
- **Tests:** `@test` es-AR / en-US; E2E de paridad.
- **Docs:** `docs/guide.md` (interpolación) — ya corregido el falso "solo fitz run".
- **Dependencias:** parte de **T1**.
- **Notas de diseño:** tabla de locales embebida (es-AR, en-US, algunos más), sin ICU. Mantener los format
  specs existentes; `num.*` es la vía locale-aware.

---

### FITZ-02 · Servido de archivos estáticos

- [x] Implementado (2026-08-20, v0.51.0) — `@server(port, static_dir="./public", static_prefix="/static")`
  con módulo compartido `src/static_files.rs` (Content-Type por extensión, ETag basado en
  contenido, HTTP-date, `is_safe_relative`), intérprete (`src/http.rs`: handler wildcard bajo el
  prefijo, `If-None-Match` → 304, Cache-Control, Last-Modified, path-traversal bloqueado
  lexical + canonicalize+containment), y **codegen con paridad bit-a-bit** (`STATIC_PRELUDE_*`
  con handler de disco Y de embed, `.merge(__fitz_static_route())`; los `__fitz_static_*` mirror
  literal de `static_files.rs`). Flag `fitz build --embed-static` hornea los assets en el binario
  con `include_bytes!` → sirve su propio frontend sin el dir en disco (distroless). Checker sin
  cambios (`@server` es opaco al checker). LSP completion de `@server` cita los kwargs nuevos.
  Guía cap 17 "Archivos estáticos" + nota deployment distroless + ejemplo `17m-static.fitz`.
  Tests: 8 unit `static_files` + 2 unit http (`resolved_static_prefix`/`if_none_match_matches`)
  + 6 unit codegen (prelude/route/embed/collect) + 2 E2E (`fitz02_static_disk_parity_content_type_etag_304_traversal`
  paridad run↔build + 304 + traversal + missing + user-route; `fitz02_embed_static_serves_without_dir_on_disk`).
  Validado a mano con curl: `fitz run`, `fitz build` (disco, ETag bit-a-bit), `fitz build --embed-static`
  (sin `public/`). **Cierra Hito 3/4 entero. Habilita T3 (PWA instalable).** MVP: sin
  directory index (un dir → 404); embed sin Last-Modified (no hay mtime en memoria); assets
  resueltos relativos al working dir del proceso (runtime) / del build (embed).
- **Estado:** Ya resuelto.
- **Evidencia:** `src/evaluator.rs:1573-1744` (allowlist cerrada de kwargs de `@server`, error en `:1740`);
  grep `ServeDir|static_dir|@static` → 0. Boilerplates fullstack con nginx aparte
  (`boilerplates/taskhub/docker-compose.yml:92-101`).
- **Impacto en un usuario real:** favicon, `manifest.webmanifest` (instalable → **T3**), CSS, sonidos. nginx
  al lado duplica la infra de una app que si no es *un binario + Postgres*.
- **Workaround hoy:** "assets como rutas" (`@get` por archivo con `Response`/`body_bytes` + `bytes_from_b64`).
  Sin caching, sin ETag, llena el binario de rutas.
- **Propuesta (API cerrada):** `@server(3000, static_dir="./public", static_prefix="/static")` con
  `Content-Type` por extensión, `ETag`/`If-None-Match`, `Cache-Control`, `Last-Modified`, path-traversal
  bloqueado. Evaluar `fitz build --embed-static` (assets dentro del binario → encaja con distroless).
- **Criterio de aceptación:** GET a `/static/foo.css` sirve con Content-Type + ETag + 304; `../` rechazado;
  paridad `run` ↔ `build`; con `--embed-static`, sirve sin el dir en disco.
- **Archivos a tocar:** `src/evaluator.rs:1573` (kwargs); `src/http.rs` (`ServeDir`/handler + ETag/traversal);
  `src/codegen.rs` (montaje + `--embed-static` con `include_bytes!`); `src/main.rs` (flag); `docs/guide.md`.
- **Tests:** E2E de estático (Content-Type/ETag/304); traversal rechazado; `--embed-static`.
- **Docs:** `docs/guide.md` cap 17 + nota de deployment distroless.
- **Dependencias:** **habilita T3**. Reduce/elimina el nginx de los boilerplates.
- **Notas de diseño:** `ServeDir` de `tower-http` en runtime; `include_bytes!` en codegen. El embed es el
  diferencial real ("un binario que sirve su propio frontend").

---

### FITZ-13 · `Map.remove(key)` `[antes FITZ-09]`

- [x] Implementado (2026-08-20, post-v0.49.0) — `m.remove(key) -> Bool` (true si existía),
  muta el Map in place (semántica de referencia compartida, visible por cualquier alias).
  Evaluator (`map_remove`, búsqueda lineal + `Vec::remove` preservando orden) + codegen con
  **paridad bit-a-bit** (`gen_map_remove`, `.remove` sobre el `Vec<(K, V)>`; Arc ligado a un
  local para no dropear el `MutexGuard` temporal) + checker (`Map<K,V>.remove(K) -> Bool`) +
  LSP (after-dot + signature catalog) + guía (sección métodos de Map). Tests: 1 unit evaluator
  (mutación vía alias) + 1 E2E de paridad `run`↔`build` (`map_remove_parity_fitz13`).
  **Desbloquea FLV-03** (eviction del store de componentes en fitz-liveviews).
- **Estado:** Confirmado (hallazgo derivado de FLV-03).
- **Evidencia:** `fitz-liveviews/src/lib.fitz:2206-2210` (el store no puede evictar: "`Map` has no `remove`
  yet"); `fitz-liveviews/docs/components.md:398`. Métodos de `Map` en el core: `get`/`has`/`keys`/`values`/`len`,
  sin `remove`.
- **Impacto en un usuario real:** indirecto — **bloquea la eviction de estado de componentes (FLV-03)**. Sin
  `Map.remove`, el store del framework crece para siempre (leak lento). MatHelp, con chicos que abandonan
  partidas, lo siente.
- **Workaround hoy:** ninguno limpio del lado del framework. MatHelp mitiga persistiendo a Postgres.
- **Propuesta (API cerrada):** `m.remove(key) -> Bool` (true si existía).
- **Criterio de aceptación:** borra la entrada y devuelve si existía; paridad `run` ↔ `build`. Habilita
  `flv_drop_instance`/eviction en fitz-liveviews.
- **Archivos a tocar:** `src/evaluator.rs` (dispatch de `Value::Map`); `src/codegen.rs` (`.remove()` sobre el
  `Vec<(K,V)>` cuidando el orden); `src/types.rs`/`src/lsp.rs`; `docs/guide.md`.
- **Tests:** unit del remove; E2E de paridad.
- **Docs:** `docs/guide.md` (métodos de `Map`).
- **Dependencias:** **desbloquea FLV-03**. Prerequisito de menor costo / mayor apalancamiento cross-repo.
- **Notas de diseño:** el `Map` interno es `Vec<(K,V)>` con orden de inserción; `remove` es búsqueda lineal +
  `retain`. Barato.

---

### FITZ-06 · `.preload()` en el intérprete

- [x] Implementado — MVP "error-claro-primero" (2026-08-20, post-v0.49.0). **La premisa
  original del norte ("no-op silencioso") quedó STALE**: desde v0.47.0 el dispatch cambió y
  `.preload()` en `fitz run` ya daba un error genérico ("QueryBuilder has no method `preload`"),
  no un no-op silencioso. Ahora da un **error DEDICADO** en ambos dispatches (QueryBuilder + Type
  directo) que apunta a `fitz build` + workarounds (navigation method `user.posts(db)` o `.where`
  separado). Helper `preload_not_in_interpreter_error`. 1 unit test (`fitz06_preload_in_interpreter_gives_dedicated_error`).
  **El criterio de aceptación se cumple** ("aborta con mensaje claro, nunca no-op silencioso").
  **Follow-up (Costo M, NO bloquea):** implementar el eager loading real en el intérprete para
  paridad con el codegen (cargar las relaciones en `fitz run`).
- **Estado:** Ya resuelto (MVP error-claro; paridad real = follow-up).
- **Evidencia:** `CHANGELOG.md:71-72`; `src/evaluator.rs:14670-14673` (cae al `_ => Ok(None)`, **no-op
  silencioso**, ni error dedicado); implementado en codegen (`src/codegen.rs:23814`, `:24933`).
- **Impacto en un usuario real:** el panel del padre lista perfiles con progreso (`Family → Profile → Mastery`).
  Funciona compilado, falla en `fitz run`. Peor: no-op silencioso (relaciones vacías sin error). Es una trampa
  (T2).
- **Workaround hoy:** en `fitz run`, queries de la relación a mano o desarrollar con `fitz build`.
- **Propuesta:** implementar `.preload()` en el intérprete (paridad), **o** — MVP barato — que `fitz run`
  emita error claro en vez del no-op silencioso.
- **Criterio de aceptación:** `.preload()` carga las relaciones en `fitz run` (paridad), o aborta con mensaje
  claro. Nunca no-op silencioso.
- **Archivos a tocar:** `src/evaluator.rs:14670` (rama `"preload"`).
- **Tests:** E2E `.preload()` en `fitz run` carga o falla con el mensaje esperado. Lo caza **FITZ-14**.
- **Docs:** `docs/db-orm.md`.
- **Dependencias:** parte de **T2**.
- **Notas de diseño:** el error-claro-primero elimina la trampa en horas mientras la implementación real
  espera turno.

---

### FITZ-07 · `.is_in(<var>)` → `= ANY($n)`

- [x] Implementado (2026-08-20, post-v0.49.0) — `is_in(<var>)` con una variable `List<T>`
  del scope externo del closure emite `"col" = ANY($N::<oid>[])`, bindeando la lista entera
  como UN solo parámetro array (el OID sale del tipo escalar de la columna). La lista literal
  sigue emitiendo `IN ($1, $2, ...)`. Evaluator (`scalar_pg_oid` + resolución del var vía `env`
  + `fitz_list_to_pg_array`) + codegen con **paridad bit-a-bit** (`orm_scalar_pg_info_from_type_expr`
  + binding array inline que lockea el Arc del var y mapea cada elem con `__IntoPgValue::into_pg`).
  Doc-comment corregido (ya no miente). Tests: 2 unit del translator (var Int → `= ANY($1::int8[])`,
  var Str → `text[]`) + 2 existentes (literal `IN` + empty `false`) intactos + 1 E2E real Postgres
  (`orm_where_is_in_variable_fitz07`). Validado run↔build bit-a-bit contra Postgres local.
  Doc `db-orm.md` sec is_in reescrita + tabla de var-support actualizada.
- **Estado:** Confirmado.
- **Evidencia:** `src/evaluator.rs:17280-17307` y `src/codegen.rs:20877-20911` — `is_in` solo matchea
  `Expr::List` literal; emite `IN ($1,$2,...)`, no `ANY($n)`. Doc-comment stale en `src/evaluator.rs:17207-17209`
  promete soporte de variable sin cumplirlo.
- **Impacto en un usuario real:** el motor adaptativo necesita
  `Mastery.where(fn(m) => m.skill_code.is_in(pendientes))` con `pendientes: List<Str>` calculada. Hoy no se puede.
- **Workaround hoy:** `db.query("... = ANY($1)", [pendientes])` crudo — pierde type-safety.
- **Propuesta:** aceptar variable `List<T>` → `"col" = ANY($n)` (lista entera a un solo parámetro array). La
  lista literal sigue igual.
- **Criterio de aceptación:** `is_in(pendientes)` compila y emite `= ANY($n)`; literal sin regresión; paridad;
  doc-comment `:17207` corregido.
- **Archivos a tocar:** `src/evaluator.rs:17280`, `src/codegen.rs:20877`, doc-comment `:17207`.
- **Tests:** los existentes + variante `var`; E2E real Postgres.
- **Docs:** `docs/db-orm.md`.
- **Dependencias:** ninguna. Bajo (hay workaround), pero cierra un doc-comment que miente.
- **Notas de diseño:** `= ANY($n)` es la forma canónica; mantener la lista literal por su camino.

---

### FITZ-12 · Paréntesis redundantes en el `match` generado

- [x] Implementado (2026-08-20, post-v0.49.0) — helper `strip_stmt_match_parens` (scanner
  balanceado que skipea strings, fail-safe: solo strippea un `(match … )` completamente
  parentizado) aplicado en `gen_return` y `gen_assign` (las 3 ramas: reasignación, `let _`,
  `let mut`). Un `let x = match …` / `return match …` deja de emitir los paréntesis externos;
  las posiciones de operando/receptor (`(match …).foo()`, `1 + (match …)`) los conservan
  (el helper solo se llama desde los emisores de statement). 2 tests (helper + output sin
  `(match n`) + smoke real (`main.rs` generado con 0 `(match `, binario corre OK).
- **Estado:** Confirmado (cosmético).
- **Evidencia:** `rustc` avisa `unnecessary parentheses around ... match` (ej. `let mut volver: String =
  (match referer.clone() { ... });`). Un build de MatHelp emite **194 warnings**, buena parte de este patrón.
- **Impacto en un usuario real:** Bajo. El ruido tapa los warnings que sí importan (y hoy, con FITZ-09 sin
  arreglar, cuesta ver los errores reales entre 194 warnings).
- **Workaround hoy:** ninguno (es ruido, no rompe).
- **Propuesta:** no envolver el `match` en paréntesis cuando es el RHS de un `let`/`return`/asignación (el
  `match` de Rust ya es una expresión válida en esas posiciones).
- **Criterio de aceptación:** un build de un programa con `match` como valor no emite el warning
  `unnecessary parentheses`.
- **Archivos a tocar:** `src/codegen.rs` — la emisión de `match` como expresión (buscar dónde se envuelve en
  `(...)`).
- **Tests:** unit que el Rust generado para `let x = match ...` no tiene los paréntesis externos.
- **Docs:** — (interno).
- **Dependencias:** ninguna. Barato; limpia el ruido de warnings.
- **Notas de diseño:** cuidar los casos donde el paréntesis sí hace falta (p.ej. `(match ...).método()` o
  dentro de un binop) — solo sacarlo en posición de statement/RHS directo.

---

### FITZ-08 · ENUM nativo de Postgres

- [ ] Implementado
- **Estado:** Confirmado (prioridad baja).
- **Evidencia:** `boilerplates/api-orm-full/src/models.fitz:24-26` (no soportado en ORM MVP).
  **`@check_constraint` SÍ existe** (`src/types.rs:3162-3228`, `src/migrations.rs:916-941`).
- **Impacto en un usuario real:** `mode` (quiz | truefalse | ...) queda como `Str` sin tipo enum. Bajo.
- **Workaround hoy:** `Str` + `@check_constraint("mode IN (...)")` — existe y funciona con migración/drift.
- **Propuesta:** ENUM nativo (`CREATE TYPE ... AS ENUM`) en ORM + migraciones. Costo L. No pedido.
- **Criterio de aceptación:** campo enum materializado como `CREATE TYPE` + columna, con migración/drift.
- **Archivos a tocar:** `src/types.rs`, `src/migrations.rs`, `src/codegen.rs`, `src/db.rs`.
- **Tests:** E2E real Postgres con enum.
- **Docs:** `docs/db-orm.md`.
- **Dependencias:** ninguna. **No prioritario** — `@check_constraint` cubre.
- **Notas de diseño:** documentar el patrón `Str + @check_constraint` como camino recomendado hoy es más
  barato que el enum nativo.

---

## Épicos transversales

### T1 · La cadena del i18n
Del lado de **fitz core**: **FITZ-03** (`fs` → catálogos desde JSON), **FITZ-04** (locale num →
`1.234.567,00`), **FITZ-05** (cookie de idioma con API real). Del lado de fitz-liveviews: `FLV-01`
(`<html lang>`) + `FLV-09` (docs). **Tratarlo como un épico único con dueño.**

### T2 · Paridad `fitz run` ↔ `fitz build` — **SUBE**
La fase F0 confirmó que esta clase es el problema estructural, no una feature suelta:

| Hallazgo | `fitz check` | `fitz run` | `fitz build` |
|---|:---:|:---:|:---:|
| FITZ-09 · `T?` | ✓ | ✓ | ✗ |
| FITZ-10 · `Str + Any` | ✓ | ✓ | ✗ |
| FITZ-04 · format specs | ✓ | ✓ | ✓ *(los docs mentían — corregido)* |
| FITZ-06 · `preload` | ✓ | ✗ *silencioso* | ✓ |

Tres divergencias en una tarde. **FITZ-14 (differ)** sube de Medio a Alto y se mueve al Hito 1: hubiera
cazado las tres solo. Un lenguaje que a veces se comporta distinto al compilar es un problema de
*confianza*, no de features — y la confianza es lo que hace que alguien apueste un proyecto real. Miembros:
FITZ-09, FITZ-10, FITZ-06 (los bugs) + FITZ-14 (la red).

### T3 · Mobile como ciudadano de primera
Del lado de **fitz core**: **FITZ-02** (static → `manifest.webmanifest` → instalable) — **cerrado en
v0.51.0**. El resto (reconnect, viewport, touch, cards) vive en fitz-liveviews (`FLV-04`, `FLV-01`,
`FLV-05`; `FLV-06` ya resuelto).

---

## Orden de ataque sugerido (re-priorizado 2026-08-20)

**Hito 1 — Arranque + confianza.**
`FITZ-01 (rand)` para desbloquear el juego. En paralelo el bloque de paridad: `FITZ-14 (differ)` +
`FITZ-09 (T?)` + `FITZ-10 (Str+Any)`. El differ landea con/antes de los fixes de codegen para dejarlos
protegidos. `FITZ-09` además destraba el binario nativo de todo lo que use fitz-liveviews (MatHelp, admin).

**Hito 2 — Deployment desbloqueado + login zero-JS + i18n correcto.**
`FITZ-11 (git en la imagen, 1 línea)` + `FITZ-05 (cookies)` + `FITZ-03 (fs)` + `FITZ-04 (locale num)`.

**Hito 3 — Mobile + eviction.** ✅ CERRADO (v0.50.0 + v0.51.0)
`FITZ-02 (static → T3)` **[v0.51.0]** + `FITZ-13 (Map.remove → desbloquea FLV-03)` **[v0.50.0]**. (Los
quick wins de liveviews van en su propio hito — ver archivo hermano.)

**Hito 4 — Robustez y ruido.**
`FITZ-06 (preload en intérprete o error claro)` + `FITZ-07 (is_in var)` + `FITZ-12 (paréntesis)`.
`FITZ-08 (enum PG)` fuera salvo demanda concreta.

---

## Refutados / fuera de alcance

| ID | Por qué no va | Evidencia |
|----|---------------|-----------|
| A4 (mitad paridad) | Los format specs `,`/`_`/`%`/`e`/`g`/`c` **compilan en `fitz build`** (confirmado por el autor sobre el binario). Solo queda locale (→ FITZ-04). Doc `guide.md:1266` **corregida 2026-08-20**. | `src/codegen.rs:40167-40257`; probe: `{n:,}` → `1,234,567` |
| A5 (mitad form) | `@post` **acepta `form-urlencoded`** (confirmado por el autor). Login = `<form method=POST>` nativo sin JS. Comentario `auth.fitz` **corregido 2026-08-20**. | `src/http.rs:4077`; `src/codegen.rs:36113` |
| FITZ-08 | Prioridad baja. `Str + @check_constraint` funciona con migración/drift. | `src/types.rs:3162`; `src/migrations.rs:916` |

---

## Qué se puede construir HOY con este repo (v0.47.0/0.48.0, sin cambios)

### ✅ Sale limpio, sin workaround
- HTTP server + rutas + JSON body + **`<form method=POST>` nativo (form-urlencoded, confirmado)** → login sin JS.
- Auth JWT + Argon2id (`jwt.encode`/`hash.password`/`hash.verify`).
- ORM Postgres nativo (`@table`, `.where(closure)`, `.insert/.update/.delete`, agregados).
- WebSockets tipados + LiveViews con diffs por WS.
- `@cron`/`@background`/`spawn`, `env`/`load_env`, `@check_constraint`.
- **Format specs completos** (`{n:,}`, `{r:.1%}`, etc.) — compilan a nativo (la doc que decía lo contrario ya está corregida).

### ⚠️ Sale, pero con workaround — AISLALO detrás de un módulo con la firma futura
| Necesidad | Workaround HOY | Qué tarea lo elimina | Costo de migrar |
|---|---|---|---|
| Ejercicios aleatorios | `rng.fitz` con la firma de `rand.seeded()`. **Guardá `seed + índice` desde el día 1** | FITZ-01 | Trivial |
| **Binario nativo (Docker)** | Correr el **intérprete** en el container (versión compilada comentada en el Dockerfile) | **FITZ-09** | Trivial: descomentás el build cuando `T?` compile |
| Build en la imagen oficial con deps | Etapa `vendor` + `fitz.docker.toml` paralelo (dos manifiestos) | FITZ-11 | Fácil: borrás el manifiesto paralelo cuando la imagen traiga `git` |
| Cookie de sesión/idioma | `cookies.fitz` (`set_cookie`/`read_cookie`) copiado del admin | FITZ-05 | Fácil |
| Números es-AR | `fmt.fitz` con la firma de `num.format`/`num.percent`/`num.currency`. **Ningún número crudo en la UI** | FITZ-04 | Fácil |
| Catálogos i18n | `locale_<code>.fitz` generados por script desde JSON | FITZ-03 | Fácil |
| ~~Assets estáticos~~ ✅ | ~~`assets.fitz` con un `@get` por archivo~~ → `@server(static_dir=)` (v0.51.0) | FITZ-02 | — |
| `is_in` con lista calculada | `db.query(... = ANY($1))` crudo, aislado en una función | FITZ-07 | Trivial |
| `List<Any>` inferido | anotar `let xs: List<Str> = []` | FITZ-10 | Trivial |

### 🚫 No sale hoy de ninguna forma
- **Binario nativo de una app que use fitz-liveviews (FITZ-09/FLV-10).** Hasta que `T?` compile, el Docker
  corre el intérprete. No es bloqueante para *shippear* (el intérprete anda) pero sí pierde ~9x + distroless.
- Nada más es 100% imposible: el único bloqueante para *empezar* MatHelp es **rand (FITZ-01)**, y hasta eso
  tiene workaround frágil.

### 🪤 Trampas conocidas
- **Funciones `-> T?` con `return` no compilan (FITZ-09).** Si tu Dockerfile compila y falla con `E0308`
  sobre `Option`, es esto. Corré el intérprete mientras.
- **`Str + Any` pasa `check` y falla `build` (FITZ-10).** Anotá el tipo del `List` vacío.
- **`.preload()` en `fitz run` = no-op SILENCIOSO.** Usá `fitz build` para esa parte o join manual.
- **La imagen oficial no trae `git` (FITZ-11)** → no resuelve git deps. Vendor stage mientras.
- **194 warnings de paréntesis redundantes (FITZ-12)** tapan los errores reales — ojo al leer el output de build.
- El doc `guide.md:1266` **ya no miente** (corregido): los format specs compilan.

### 📐 Recomendaciones de arquitectura para MatHelp (fitz core)
1. **`rng.fitz`** — todo el azar con la firma de `rand.seeded()`. Guardá `seed + índice` desde el día 1.
2. **Docker: corré el intérprete ahora, dejá el build compilado comentado** — un solo comentario para
   descomentar cuando cierre FITZ-09.
3. **Un solo `fitz.docker.toml` + vendor stage** si dockerizás con deps (hasta FITZ-11); tené un test que
   verifique que el manifiesto paralelo y el `fitz.toml` no divergen.
4. **`cookies.fitz`/`fmt.fitz`/`i18n.fitz`/`assets.fitz`/`db_queries.fitz`** — cada limitación detrás de un
   módulo con la firma futura. Ningún número crudo en la UI. `is_in`/`preload` en funciones con nombre.
5. **Anotá los `List`/`Map` vacíos** (`let xs: List<Str> = []`) para no chocar con FITZ-10 al compilar.

El principio: **workarounds fáciles de borrar.** El más caro de arrastrar es el del intérprete-en-Docker
(FITZ-09) — pero es un comentario; el día que `T?` compile, descomentás y ganás perf + distroless.
