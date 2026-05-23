# Curso `Fitz de 0 a experto` — Plan

**Estado**: planificada, sin arrancar. Este documento es el
plan de obra; el contenido real vivirá en `docs/curso/` cuando
arranquemos.

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

### M6 — Capstone: Postgres + SQLAlchemy + Fitz (6 capítulos)

| Cap | Título | Cubre |
|---|---|---|
| C27 | Setup interop | Binario con feature `python` habilitada (o release con interop), venv con SQLAlchemy + psycopg, `from python import math` smoke |
| C28 | Modelos SQLAlchemy | `db/models.py` con `User`/`Post` sobre Postgres real, `docker-compose.yml` para la DB |
| C29 | `fitz py-types` | `fitz py-types db/models.py --out src/models/db_types.fitz`, regenerar cuando cambien los modelos Python |
| C30 | CRUD end-to-end | Handlers Fitz que llaman `db.list_users()` → `List<User>` Fitz con coerción automática, `db.create_user(payload)?` propagando errores via `Result` |
| C31 | DX en producción | `fitz dev` (hot reload) + `fitz test` (integración contra DB de test) + `fitz lint` + GitHub Actions ejemplo |
| C32 | `fitz build` + Docker | Compilar a binario, Dockerfile multi-stage reusando el boilerplate `python-postgres`, deploy local con `docker compose up` |

**Entregable**: app CRUD de blog con auth JWT, Postgres real,
hot reload en dev, binario standalone para prod, Dockerizado.

---

### M7 — Producción y deployment (4 capítulos)

| Cap | Título | Cubre |
|---|---|---|
| C33 | Estructura final | Walkthrough de la convención completa con justificación de cada carpeta — modelos vs services vs handlers vs db |
| C34 | Variables de entorno | `env("DATABASE_URL")?` + `load_env(".env")?`, conventions para dev/staging/prod |
| C35 | CI con GitHub Actions | `fitz check`, `fitz test`, `fitz lint`, `fitz fmt --check`, build matrix |
| C36 | Más allá del curso | Roadmap personal: Fase 10 (ORM nativo), Fase 11 (frontend), cómo seguir aprendiendo, dónde reportar bugs |

**Entregable**: app de M6 con `.env` para configuración, CI
verde en GitHub, lista para deploy real.

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
