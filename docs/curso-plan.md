# Curso `Fitz de 0 a experto` — Plan

**Estado**: M1 en construcción (arrancó 2026-06-01, post-v0.11.1).
El contenido real vive en [`docs/curso/`](curso/index.md).

> **Actualización 2026-06-01 (al arrancar M1)**: tres ajustes
> sobre el plan original (trazado en v0.9.x):
>
> 1. **M6 cambia capstone**: ORM nativo Fitz (cap 31 de la guía)
>    como camino default; SQLAlchemy via `fitz py-types` queda como
>    cap opcional para users con modelos Python existentes. La Fase
>    10 (cerrada en v0.10.x) volvió a SQLAlchemy un nice-to-have,
>    no la opción principal.
> 2. **M7 espera Fase 12**: el módulo de deployment se documenta
>    pero NO se escribe hasta que `fitz dockerfile`/`fitz compose`/
>    `fitz deploy` (Fase 12) estén implementados. Sin esos
>    sub-comandos, el cap quedaría inconsistente con la promesa
>    "deployment ciudadano primera clase". M1-M6 se releasea como
>    "curso completo backend"; M7 llega después.
> 3. **Tabla nueva "Mapping curso → guide.md"** (ver más abajo).
>    El curso NO duplica el contenido de la guía — la cita como
>    referencia técnica. Cada cap del curso linkea al cap
>    correspondiente de la guía para "el detalle exhaustivo".

---

## Qué es

Serie de tutoriales en español que enseñan Fitz desde la
instalación hasta un proyecto productivo, usando **VSCode como
editor obligatorio** (para que la extensión y el LSP sean parte
visible del aprendizaje).

Se diferencia de `docs/guide.md`:

| | `guide.md` | curso |
|---|---|---|
| Estilo | referencia feature-por-feature | narrativo, proyecto que crece |
| Audiencia | gente que ya empezó | desde cero absoluto |
| Código | ejemplos aislados | un solo proyecto incremental |
| Pre-reqs | sabés programar | sabés programar pero no Fitz |

---

## Posicionamiento y por qué importa

La `guide.md` cubre el lenguaje feature por feature, sirve como
referencia y como introducción a quien ya está mirando Fitz con
intención técnica. Lo que falta es la puerta de entrada
pedagógica para alguien que cae al sitio, lee "HTTP nativo +
async + auth + WebSockets" y necesita verlo construirse paso a
paso desde un `print("hola")` hasta un servicio real con DB.

El curso ocupa ese espacio. También es marketing implícito de
la extensión VSCode — exigir VSCode obliga a mostrar el LSP en
funcionamiento desde el capítulo 3.

---

## Decisiones tomadas (confirmadas 2026-05-23)

1. **Idioma**: español, consistente con `guide.md`.
2. **Ubicación**: `docs/curso/` (paralelo a `guide.md`). Cada
   módulo es una sub-carpeta con sus capítulos en `.md`.
3. **Screenshots**: descripciones ASCII para la mayoría; solo
   screenshots reales para hitos visuales (M1 instalación
   completa, M4 Scalar UI en `/docs`, M6 hot reload corriendo).
4. **M7 incluido como parte mandatoria** (no opcional). Curso
   total = **7 módulos, 36 capítulos**.
5. **Smoke automatizado**: el código de cada capítulo vive en
   `examples/curso/cXX-tema/` y entra al smoke
   `GUIDE_EXAMPLES_COMPILE` para que no se rompa silencioso. Es
   costo de CI conocido (~30 ejemplos extra) a cambio de
   garantía de no-drift.

---

## Convención de carpetas (lo que el curso enseña a construir)

```text
mi-proyecto/
├── fitz.toml
├── fitz.lock
├── src/
│   ├── main.fitz          # entry point (bin)
│   ├── lib.fitz           # opcional, módulo raíz exportable
│   ├── models/            # type definitions (User, Order, ...)
│   ├── services/          # lógica de negocio
│   ├── handlers/          # @get/@post/@ws — HTTP/WS endpoints
│   ├── middleware/        # @middleware reusables
│   └── db/                # acceso a datos (interop Python en M6)
├── tests/                 # @test fns con `fitz test`
├── .env                   # cargado con load_env() (M7)
└── README.md
```

"Namespaces" en Fitz = módulos (`from src.models.user import
User`). El curso lo trata como tal y enseña cuándo separar.
Regla heurística que el curso enseña: **un archivo por concepto
cohesivo**, agrupar por **capa** (models / services / handlers
/ db) antes que por **feature**, hasta que el proyecto crezca
lo suficiente para justificar el cambio.

---

## Módulos y capítulos

### M1 — Setup y primer programa (5 capítulos)

Requisito explícito: VSCode + extensión Fitz instalada.

| Cap | Título | Objetivo | En VSCode se ve | Entregable |
|---|---|---|---|---|
| C1 | Instalación | Bajar binario `fitz`, instalar `.vsix`, validar `fitz --version` | Extension activada, status bar muestra "Fitz LSP" | Terminal con `fitz` funcionando |
| C2 | `fitz new` | Crear `hola-fitz` con `fitz new hola-fitz`, abrir en VSCode | Estructura `fitz.toml` + `src/main.fitz`, syntax highlighting | Proyecto skeleton |
| C3 | Hola mundo + LSP | Editar `main.fitz`, hover sobre `print`, autocomplete, tipear error y borrarlo | Hover tooltip con tipos, autocomplete pop-up, subrayado rojo | `print("Hola, Fitz")` corriendo |
| C4 | CLI esencial | `fitz run` / `check` / `fitz fmt` / `fitz lint` — qué hace cada uno | Comandos desde terminal integrada | Familiaridad con el ciclo edit-run |
| C5 | REPL | `fitz repl`, comandos `:type`, `:help`, `:load`, history | REPL en terminal integrada | Experimentar interactivamente |

---

### M2 — Tipos y funciones (7 capítulos, single-file todavía)

| Cap | Título | Cubre |
|---|---|---|
| C6 | Variables y primitivos | `let`, Int/Float/Str/Bool/Null, reasignación, anotaciones opcionales |
| C7 | Strings e interpolación | `"hola, {name}"`, métodos `upper`/`lower`/`len`, operadores |
| C8 | Funciones | `fn`, params/return tipados, expresión `=>` vs bloque, scope |
| C9 | Control flow | `if`/`while`/`for in 0..10`/`loop`/`match` |
| C10 | Tipos custom | `type User { id: Int, name: Str, email: Str? = null }`, defaults, nullables, igualdad |
| C11 | Errores con Result | `Result<T>`, `Ok`/`Err`, operador `?`, match exhaustivo |
| C12 | Higher-order | `xs.map(fn(x) => x * 2)`, filter, find, FnExpr inline |

**Entregable del módulo**: calculadora CLI single-file con
tipos custom + validaciones via `Result`.

---

### M3 — Módulos y organización (5 capítulos) ★ namespaces y buenas prácticas

| Cap | Título | Objetivo |
|---|---|---|
| C13 | `import` vs `from import` | Cuándo usar cada uno, aliases con `as`, path relativo desde el archivo |
| C14 | Multi-archivo: separar `models/` | Refactor de la calculadora — sacar `type` a `src/models/operacion.fitz`. Mostrar go-to-definition cruzando archivos |
| C15 | `[lib]` y `[bin]` en `fitz.toml` | Convertir una parte en biblioteca, otra en binario. Cuándo conviene |
| C16 | Path deps reusables | `fitz add validador --path ../validador-fitz` — extraer un módulo a un crate aparte y reusarlo |
| C17 | Tests con `@test` | `tests/unit_models.fitz`, `assert_eq`, `fitz test`, filtros |

**Entregable**: calculadora reorganizada en `src/models/` +
`src/services/` + `tests/`, con un `validador-fitz` como path
dep separado, todo lintado y formateado.

---

### M4 — HTTP first-class (5 capítulos)

| Cap | Título | Cubre |
|---|---|---|
| C18 | Primer `@get` | Handler simple, `@server(3000)`, curl, ver `/docs` Scalar en navegador |
| C19 | Path params + body | `@get("/users/{id}")` + `@post("/users")` con `User` body deserializado, `Result<User>` → 200/500 |
| C20 | Organización HTTP | Mover handlers a `src/handlers/users.fitz`. Convención: un archivo por recurso |
| C21 | OpenAPI + headers | `@header(name="Authorization", into="token")`, `@server(api_version="1.2.0")`, `fitz openapi` |
| C22 | Middleware + CORS | `@middleware(log_request)` apilado, `cors(...)` permisivo vs production, gate-only pattern |

**Entregable**: API REST de usuarios con 5 endpoints, OpenAPI
auto en `/docs`, middleware de logging, CORS configurado.

---

### M5 — Async, auth, real-time (4 capítulos)

| Cap | Título | Cubre |
|---|---|---|
| C23 | Async nativo | `async fn`, `.await`, `sleep`, handlers async, paralelismo HTTP real |
| C24 | Auth nativa | `@auth_provider` con JWT + `hash.password`/`verify` (Argon2), `@authenticated`/`@admin`, 401/403 en OpenAPI auto |
| C25 | WebSockets tipados | `@ws("/chat") @authenticated` + `WsConn<Message>` + broadcast + heartbeat, AsyncAPI en `/asyncapi.json` |
| C26 | Jobs sin Celery | `@cron("0 */5 * * * *")` + `@background` + `spawn(track_metric)` |

**Entregable**: chat WebSocket con login (JWT), broadcast, job
cron de limpieza, todo en el mismo binario.

---

### M6 — Capstone: Postgres + ORM nativo Fitz (6 capítulos)

> **Cambio respecto al plan original**: este módulo usaba SQLAlchemy
> via `fitz py-types` como capstone. Con la Fase 10 cerrada (ORM
> nativo Fitz, driver Postgres puro, sin libpq), el camino default
> ahora es el ORM nativo. SQLAlchemy queda como **cap opcional**
> al final del módulo para users con modelos Python existentes.

| Cap | Título | Cubre |
|---|---|---|
| C27 | Setup Postgres | `docker-compose.yml` para Postgres local, `db.connect(env("DATABASE_URL")?)`, primer query con `db.query` |
| C28 | Modelos con `@table` | `type User { ... } @table("users") @primary id: Int` + `@column`/`@belongs_to`/`@has_many`, generación automática del schema |
| C29 | Migraciones | `fitz db diff` para ver el SQL DDL que falta, `fitz db migrate` para aplicarlo, `fitz db status` para ver el estado |
| C30 | CRUD end-to-end | Handlers Fitz con `User.where(fn(u) => ...).all(db)?`, `User.insert(db, user)`, eager loading con `.preload("posts")` |
| C31 | DX en producción | `fitz dev` (hot reload) + `fitz test` (integración contra DB de test) + `fitz lint` + GitHub Actions con Postgres service container |
| C32 | `fitz build` + Docker | Compilar a binario, Dockerfile multi-stage (boilerplate `api-orm-full` como referencia), deploy local con `docker compose up` |

**Cap opcional al final del módulo** (no obligatorio para el
entregable):

| Cap | Título | Cubre |
|---|---|---|
| C32b | Si ya tenés modelos Python | `fitz py-types` para auto-generar types Fitz desde modelos SQLAlchemy, interop con feature `python` habilitada |

**Entregable**: app CRUD de blog con auth JWT (de M5), Postgres
real, migraciones automáticas, hot reload en dev, binario
standalone para prod, Dockerizado. Sin `pip`, sin SQLAlchemy, sin
ORM externo — todo nativo del lenguaje.

---

### M7 — Producción y deployment (4 capítulos) — ⏸ PENDIENTE FASE 12

> **Estado**: este módulo está **suspendido hasta que cierre Fase
> 12** (deployment ciudadano primera clase). Los caps de abajo
> asumen que `fitz dockerfile`/`fitz compose`/`fitz deploy` están
> implementados y producen artefactos canónicos. Escribirlo antes
> de que existan implicaría que el lector aprenda flow manual que
> después cambia — peor pedagógicamente.
>
> Mientras tanto, **M1-M6 se releasea como "curso completo backend"**.
> Los users que llegan al final de M6 ya tienen una app dockerizable
> con el boilerplate `api-orm-full` como referencia (cap 32 del
> módulo M6).

| Cap | Título | Cubre |
|---|---|---|
| C33 | Estructura final | Walkthrough de la convención completa con justificación de cada carpeta — modelos vs services vs handlers vs db |
| C34 | Variables de entorno | `env("DATABASE_URL")?` + `load_env(".env")?`, conventions para dev/staging/prod |
| C35 | `fitz dockerfile` + `fitz compose` | Generación automática del Dockerfile + docker-compose con la DB detectada del `@table` (Fase 12.1+12.2) |
| C36 | CI + deploy + más allá | `fitz check`/`test`/`lint` en GitHub Actions + `fitz deploy` (Fase 12.x), roadmap personal del lector |

**Entregable**: app de M6 con `.env` para configuración, CI verde
en GitHub, `fitz dockerfile` + `fitz deploy` corriendo end-to-end.

---

## Mapping curso → guide.md

El curso NO duplica el contenido de la guía. Cada capítulo cita
la sección correspondiente de [`guide.md`](guide.md) como "detalle
exhaustivo". El curso aporta:

- **Narrativa**: un proyecto que crece capítulo a capítulo.
- **Setup ergonómico**: VSCode + LSP visible desde C3.
- **Organización de carpetas**: `src/models/` + `services/` +
  `handlers/` + `db/` + `tests/` — convención que la guía no cubre.
- **Decisiones del autor**: cuándo elegir qué.

Mapping completo al estado actual de la guía (post-v0.11.1):

| Cap curso | Cubre | Base en guide.md |
|-----------|-------|------------------|
| C1 Instalación | Setup `fitz` + VSCode + .vsix | (nuevo — no está en guía) |
| C2 `fitz new` | Crear proyecto skeleton | §16b Package manager |
| C3 Hola + LSP | Editar + hover + autocomplete | §2 + §22 (editores) |
| C4 CLI esencial | run/check/fmt/lint en terminal | §23 fmt + §24 test + §25 dev + §27 lint |
| C5 REPL | `fitz repl` + `:type` + `:load` | §26 REPL |
| C6-C12 Tipos+fns | Vars/Strings/Fns/Control/Type/Result/HOF | §3-15 |
| C13-C16 Módulos+pkg | `import` + multi-archivo + `[lib]` + path deps + tests | §16 + §16b + §24 |
| C17 Tests | `@test`/`assert_eq`/`fitz test`/filtros | §24 |
| C18-C22 HTTP | `@get`/path params/body/OpenAPI/middleware/CORS | §17 + §18 docs + middleware |
| C23 Async | `async fn`/`.await`/`sleep` | §19 |
| C24 Auth | `@auth_provider`/`@authenticated`/JWT/Argon2 | §28 |
| C25 WS | `@ws`/`WsConn<T>`/broadcast/AsyncAPI | §29 |
| C26 Cron | `@cron`/`@background`/`spawn` | §30 |
| C27-C32 ORM nativo capstone | Postgres + `@table` + migraciones + CRUD + Docker | §31 Postgres + ORM + [DB y ORM exhaustivo](db-orm.md) |
| C32b opcional | Interop SQLAlchemy | §21.8 Interop Python |
| C33-C34 Producción (parcial) | Convención carpetas + env vars | §32 env (parcial) |
| C35-C36 Deployment | `fitz dockerfile`/`compose`/`deploy`/CI | (pendiente Fase 12) |

**Regla operativa del curso**: cada cap arranca diciendo qué
features nuevas introduce y, al final, linkea a las secciones
exhaustivas de la guía. El lector que quiera profundizar tiene
camino claro; el lector que sigue lineal no se desvía.

---

## Template del capítulo

Cada capítulo en `docs/curso/mX/cXX-tema.md` sigue esta
estructura:

```markdown
# CXX — Título corto

**Pre-requisitos**: CYY (...), CZZ (...)

**Objetivo**: una sola oración con qué tiene que saber/hacer
el lector al terminar.

**Por qué importa**: una sola oración con el "por qué" para
que el lector entienda el motivo, no solo el cómo.

## Paso 1 — ...
## Paso 2 — ...
## Paso 3 — ...

(En cada paso: comando, código, qué se ve en VSCode si aplica)

## Código antes / después

(Diff o bloques `antes:` / `después:` cuando hay refactor)

## Validación

(Cómo confirmar que funciona — `fitz run`, `curl`, output esperado)

## Entregable commiteable

`examples/curso/cXX-tema/` — ejecutable con `fitz run` (o `fitz
build` si aplica). Entra al smoke `GUIDE_EXAMPLES_COMPILE`.

## Lo que viene en CXX+1

(Bridge de una oración al próximo capítulo)
```

---

## Cuándo arrancar

Sin fecha. Es **iniciativa paralela** a las fases del lenguaje
— no bloquea ni es bloqueada por Fase 10 / 11 / 12.

Cuando arranquemos, el orden propuesto es:

1. Crear `docs/curso/` con un `index.md` que liste los módulos
2. Escribir M1 entero (5 caps) y validar smoke
3. Releasear M1 públicamente (post en blog, anuncio, etc.) para
   ver si tracciona antes de invertir en M2-M7
4. Iterar el resto según feedback

Cada módulo es una unidad releasable independiente. M1-M3
funcionan como "tutorial corto"; M1-M4 como "tutorial HTTP";
M1-M6 como "curso completo backend"; M7 es polish para los que
quieren llegar a producción.

---

## Riesgos identificados

- **Drift**: cada cambio del lenguaje puede romper código del
  curso. Mitigación: smoke automatizado +
  `feedback_post_changes_smoke_examples_boilerplates`.
- **Mantenimiento de screenshots**: si los hacemos, envejecen.
  Mitigación: descripciones ASCII por default, screenshots solo
  para los 3 hitos visuales.
- **Audiencia diluida**: el curso compite con `guide.md` por
  attention budget. Mitigación: cross-link explícito al inicio
  de ambos ("¿buscás referencia? → guide. ¿buscás aprender desde
  cero? → curso").
- **Tiempo de escritura**: 36 capítulos es mucho. Mitigación:
  releasear por módulo, no esperar al curso entero.

---

## Tiers pre-M5 (acordado 2026-06-01)

M5 del curso cubre Async (C23) + Auth (C24) + WebSockets (C25) +
Jobs (C26). Los caps C24/C25/C26 tienen deudas residuales
documentadas en `docs/roadmap.md` → "Fase 9.w iteración 2" que se
diferían a "post-Fase 10". Fase 10 cerró en v0.10.x, así que el
disparador real ahora es el avance del curso.

**Decisión**: ciertas deudas se cierran ANTES de arrancar a
escribir M5, para no enseñar funcionalidad con gaps obvios (ej:
el cap C26 sin persistencia tendría que decir "ojo, si reinicia
el server perdés los jobs" — justo lo que Fitz vende contra
Celery).

### T1 — bloquean M5 (cerrar antes de arrancar a escribir)

**Estado: CERRADO entero (2026-06-02, v0.11.2)** — bloque
9.w.3.iter2 cubrió los tres items en un release dedicado.
Detalle técnico en `docs/roadmap.md` → "9.w.3.iter2" y en el
CHANGELOG. El cap 30 de la guía documenta el flow nuevo.

| # | Item | Cap impactado | Estado |
|---|---|---|---|
| 1 | **Persistencia de jobs sobre DB nativa** | C26 Jobs | ✅ CERRADO v0.11.2 — `store=db` crea `fitz_cron_jobs` + `fitz_cron_runs` con CREATE TABLE IF NOT EXISTS al boot, persiste cada attempt |
| 2 | **Retry con backoff exponencial** para `@cron` (+ `tz`/`retry` también en `@background`) | C26 Jobs | ✅ CERRADO v0.11.2 — `retry={max, backoff: "exponential"\|"linear"\|"constant", initial_secs, max_secs}` capeado |
| 3 | **Cron timezone configurable** (`@cron(..., tz="America/Argentina/Buenos_Aires")`) | C26 Jobs | ✅ CERRADO v0.11.2 — IANA via `chrono-tz`, default UTC |

**Bonus cerrado en el mismo bloque** (no estaba en T1 original):
`catch_up=true|false` — al boot, si hubo missed runs entre
`last_run_at` y `now`, ejecuta UN run inmediato (no N, evita
spam). Default `false`.

### T2 — evaluar antes de M5 (cierran caso de uso del cap si entran)

| # | Item | Cap impactado | Decisión |
|---|---|---|---|
| 4 | **Rooms/channels en WebSockets** | C25 WS | Depende del ejemplo del cap. Si el chat tiene salas (típico), entra T1. Si es chat global plano, baja a T3 |
| 5 | **Reconnect con state replay** | C25 WS | Acopla con persistencia de T1.1. Si entra T1.1 con storage genérico, agrego mientras estoy ahí |
| 6 | **RBAC con roles custom** (más allá de `@admin`) | C24 Auth | El cap puede vivir con `@admin` único, pero un curso real va a querer enseñar `@requires("editor")` o similar |
| 7 | **Token refresh/revocación server-side** | C24 Auth | Acopla con T1.1 (la blacklist necesita storage). Si está la persistencia, este es chico |

### T3 — post-M5 (mencionar como deuda visible en el cap, no bloquean)

- Coordinación multi-instancia (locks distribuidos) — el curso enseña single-instance, está bien
- `spawn` con coordinación múltiple (Promise.all style) — helper de stdlib eventualmente
- Sessions cookie-based — alternativa a JWT, fuera de alcance del curso
- JWT asimétricos (RS256/ES256) — advanced security
- Backpressure explícito en WS
- Heterogéneos en `jwt.encode/decode` (Map<Str, Any>)

### Orden de ejecución acordado

Mini-fases discretas, una por release, en este orden:

1. **9.w.3.iter2** ✅ **CERRADO 2026-06-02 (v0.11.2)** — Persistencia + retry + timezone + catch_up de jobs (T1.1 + T1.2 + T1.3 + bonus catch_up cerrados juntos).
2. **9.w.1.iter2** — RBAC custom + token refresh (T2.6 + T2.7 — comparten la noción de "roles persistidos" si entra refresh).
3. **9.w.2.iter2** — Rooms + reconnect state replay (T2.4 + T2.5 — solo si el ejemplo del C25 los exige; si no, baja a T3).
4. **Arrancar M5 del curso**.

C23 (Async) no tiene deudas relevantes — sale directo cuando llegue su turno.

Cuando alguno de los tiers cierre, marcarlo en esta tabla como
**CERRADO** con fecha + versión, y actualizar paralelamente en
`docs/roadmap.md` → "Fase 9.w iteración 2".
