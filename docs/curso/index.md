# Curso `Fitz de 0 a experto`

> Curso pedagógico narrativo en español. Te lleva desde la instalación
> hasta una app real con Postgres + ORM + Docker.

Este curso es **complementario a la [guía](../guide.md)**. La guía es
referencia feature-por-feature; el curso es narrativo, con un proyecto
que crece capítulo a capítulo.

| | [Guía](../guide.md) | Curso |
|---|---|---|
| Estilo | referencia | narrativo |
| Audiencia | ya empezaste con Fitz | desde cero |
| Código | ejemplos aislados | un proyecto que crece |
| Mejor para | "¿cómo se hace X?" | "¿cómo aprendo Fitz?" |

Los dos se complementan. El curso te enseña cómo usar las cosas en
contexto; la guía te muestra el detalle exhaustivo de cada feature.
Cuando un capítulo del curso introduce algo nuevo, te linkea al cap
correspondiente de la guía para que tengas la referencia a mano.

## Antes de empezar

**Pre-requisito único**: sabés programar (Python / JavaScript /
TypeScript / Go / Rust / Java / cualquiera) pero nunca tocaste Fitz.

**No es necesario** saber Rust, Vue, FastAPI, ni nada específico —
el curso explica cada concepto que aparece.

**Editor requerido**: [VSCode](https://code.visualstudio.com/). El
curso muestra el LSP (autocomplete, hover, errores subrayados) desde
el capítulo 3. Otros editores funcionan también pero el material
asume VSCode para los screenshots ASCII.

## Estado del curso

| Módulo | Caps | Estado |
|--------|------|--------|
| M1 — Setup y primer programa | 6 | ✅ cerrado (C1-C6) |
| M2 — Tipos y funciones | 7 | ✅ cerrado (C1-C7) |
| M3 — Módulos y organización | 5 | ✅ cerrado (C1-C5) |
| M4 — HTTP first-class | 5 | ✅ cerrado (C1-C5) |
| M5 — Async, auth, real-time | 5 | ✅ cerrado (C1-C5) |
| M6 — Capstone Postgres + ORM nativo | 7 | ✅ cerrado (C1-C7) |
| M7 — Interop Python | 3 | ✅ cerrado (C1-C3) |
| M8 — Producción y deployment | 5 | ✅ cerrado (C1-C5) |
| M9 — Frontend nativo con `.fitzv` | 4 | ✅ MVP (C1-C3, v0.21.4) + C4 `@rpc` (v0.30.0) |

Total: **8 módulos, 43 capítulos**. Cada módulo es **unidad releasable
independiente** — no hace falta esperar que esté todo para empezar.
M6 creció a 7 caps al sumar **M6.C6 — Migraciones con `fitz db`**
(v0.13.3) que cierra el gap del workflow versionado que antes
quedaba como "out of scope del curso". M7 (Interop Python) se sumó
después de cerrar el plan original M1-M7 para cubrir el ecosistema
Python como puente; el M8 anterior (Producción y deployment) se
renumeró. M5 creció a 5 caps al sumar **M5.C5 — HTTP client
outbound** (v0.17.0) tras cerrar la mini-fase del módulo `http`
builtin del lenguaje.

## M1 — Setup y primer programa

Requisito explícito: VSCode + extensión Fitz instalada.

- **[C1 — Instalación](m1-setup/c1-instalacion.md)** ← empezá acá
- **[C2 — `fitz new` (proyecto skeleton)](m1-setup/c2-fitz-new.md)**
- **[C3 — Hola mundo + LSP visible](m1-setup/c3-hola-lsp.md)**
- **[C4 — CLI esencial (run / check / fmt / lint)](m1-setup/c4-cli-esencial.md)**
- **[C5 — REPL](m1-setup/c5-repl.md)**
- **[C6 — `fitz build` (compilar a binario nativo)](m1-setup/c6-fitz-build.md)**

**Entregable del módulo**: tenés Fitz funcionando en tu máquina,
con la extensión VSCode activa y un proyecto skeleton del que
podés escribir, correr, formatear, debugear, y **compilar a
binario standalone para distribuir** (`fitz build`).

## M2 — Tipos y funciones

Requisito explícito: M1 completo.

- **[C1 — Primitivos, strings e interpolación](m2-tipos-funciones/c1-primitivos-strings.md)**
- **[C2 — Variables, anotaciones e inferencia](m2-tipos-funciones/c2-variables.md)**
- **[C3 — Operadores y control de flujo (`if` / `else`)](m2-tipos-funciones/c3-operadores-if.md)**
- **[C4 — Loops (`while`, `loop`, `for in`)](m2-tipos-funciones/c4-loops.md)**
- **[C5 — Listas, mapas y rangos](m2-tipos-funciones/c5-listas-mapas-rangos.md)**
- **[C6 — Funciones + `fitz test`](m2-tipos-funciones/c6-funciones.md)**
- **[C7 — `type` (tipos custom) + `match`](m2-tipos-funciones/c7-type-match.md)**

**Entregable del módulo**: podés modelar tu dominio con tipos
custom, escribir fns que procesan colecciones, manejar errores
con `Result`+`match`+`?`, y testear todo con `fitz test`. Es el
toolkit base del lenguaje.

## M3 — Módulos y organización

Requisito explícito: M2 completo.

- **[C1 — Módulos + imports (`from X import Y`)](m3-modulos/c1-modulos-imports.md)**
- **[C2 — Lib local + sección `[lib]`](m3-modulos/c2-lib-local.md)**
- **[C3 — Path deps + lockfile (`fitz.lock`)](m3-modulos/c3-path-deps.md)**
- **[C4 — Git deps + cache local](m3-modulos/c4-git-deps.md)**
- **[C5 — `fitz add` / `remove` / `update` + patrones](m3-modulos/c5-add-remove-update.md)**

**Entregable del módulo**: podés partir tu proyecto en módulos,
exponer una lib, consumir deps externas (path o git), y
organizar proyectos serios (monorepo, layered architecture,
shared libs).

## M4 — HTTP first-class

Requisito explícito: M3 completo.

- **[C1 — `@get/@post/@put/@delete` + `@server`](m4-http/c1-verbos-server.md)**
- **[C2 — Body, query params y headers](m4-http/c2-body-query-headers.md)**
- **[C3 — Middleware + CORS](m4-http/c3-middleware-cors.md)**
- **[C4 — OpenAPI 3.1 autogenerado + `/docs`](m4-http/c4-openapi-docs.md)**
- **[C5 — Status codes custom + errores HTTP](m4-http/c5-status-content-errores.md)**

**Entregable del módulo**: podés escribir una API completa
production-ready en Fitz — handlers tipados, validación
automática del input, errores ricos con status code correcto,
middleware reusable, CORS configurado y docs autogenerados. Sin
ningún `pip install`, `npm install`, ni `pom.xml`.

## M5 — Async, auth, real-time

Requisito explícito: M4 completo.

- **[C1 — `async fn` + `.await` + paralelismo HTTP real](m5-async-auth-rt/c1-async-await.md)**
- **[C2 — Auth nativa con `@auth_provider` + JWT + Argon2id](m5-async-auth-rt/c2-auth.md)**
- **[C3 — WebSockets tipados con `@ws` + AsyncAPI auto](m5-async-auth-rt/c3-websockets.md)**
- **[C4 — Jobs sin Celery (`@cron` + `@background` + `spawn`) + persistencia](m5-async-auth-rt/c4-jobs.md)**
- **[C5 — HTTP client outbound (módulo `http` built-in)](m5-async-auth-rt/c5-http-client.md)**

**Entregable del módulo**: tenés todas las features modernas de
producción **integradas en el lenguaje**. Concurrencia real con
N workers tokio multi-thread, login + JWT + Argon2id sin
dependencias externas, canales WebSocket tipados con AsyncAPI 3.0
auto-generado y heartbeat built-in, tareas programadas con tz
IANA + retry con backoff + persistencia opcional sobre Postgres,
y requests HTTP outbound con el módulo `http` builtin integrado
con `@background + spawn` y `@cron` — todo built-in. **Sin
Celery, sin Redis para infra básica, sin passport.js, sin
socket.io, sin `pip install requests` ni `npm install axios`**.
Un binario standalone deployable.

## M6 — Capstone Postgres + ORM nativo

Requisito explícito: M5 completo.

- **[C1 — Setup Postgres + `db.connect` + driver crudo](m6-postgres-orm/c1-setup-driver-crudo.md)**
- **[C2 — `@table`, `@primary` y lecturas tipadas con el ORM](m6-postgres-orm/c2-table-decoradores-reads.md)**
- **[C3 — Writes (`.insert`/`.update`/`.delete`) + QueryBuilder + agregados](m6-postgres-orm/c3-writes-querybuilder-agregados.md)**
- **[C4 — Relations + navigation methods + eager loading](m6-postgres-orm/c4-relations-navigation-preload.md)**
- **[C5 — Tipos avanzados: jsonb, arrays, Date/DateTime/Uuid](m6-postgres-orm/c5-tipos-avanzados.md)**
- **[C6 — Migraciones con `fitz db`](m6-postgres-orm/c6-migraciones-fitz-db.md)**
- **[C7 — Capstone: app CRUD completa con auth + ORM + WS + cron + Docker](m6-postgres-orm/c7-capstone-crud-completo.md)**

**Entregable del módulo**: una **app real production-ready** que
integra TODO lo del curso — auth con JWT + Argon2id, ORM con
relations sobre Postgres, WebSocket para notificaciones,
cron jobs con persistencia, OpenAPI + AsyncAPI auto, todo en un
binario standalone de ~30 MB con su `docker-compose.yml`,
**y workflow versionado de migraciones con `fitz db`** para
cambios de schema en producción. **Sin `pip install`, sin
`npm install`, sin `requirements.txt`, sin `package.json`,
sin `alembic upgrade`** — deploy = un binario.

## M7 — Interop Python

Requisitos explícitos: M6 cerrado (vas a comparar contra el ORM nativo
en el cap C3), Python 3.10+ instalado, y un binario `fitz` compilado
con `cargo build --release --features python` (el default standalone
sigue sin libpython).

- **[C1 — Setup venv + `from python import` + casos simples](m7-python-interop/c1-setup-imports.md)**
- **[C2 — numpy + pandas reales: data analysis](m7-python-interop/c2-numpy-pandas-data-analysis.md)**
- **[C3 — SQLAlchemy interop + bridge async + cuándo NO usarlo](m7-python-interop/c3-sqlalchemy-async-vs-orm-nativo.md)**

**Entregable del módulo**: un servicio HTTP Fitz que combina handlers
tipados nativos con calls a pandas para análisis de datos y SQLAlchemy
para acceso a DB legacy — el puente al ecosistema Python sin renunciar
a la identidad de Fitz (HTTP nativo, tipos, deployment standalone).
**Cierra el círculo con M6**: matriz de decisión honesta para elegir
ORM nativo Fitz vs SQLAlchemy según el contexto.

## M8 — Producción y deployment

Requisito explícito: M6 cerrado (tenés una app real para deployar) o
M7 cerrado (si tu app usa interop Python — el cap M8.C5 te muestra
cómo distribuir esos casos sin Python instalado en destino).

- **[C1 — Distribución avanzada: binarios standalone y cross-compile](m8-produccion-deploy/c1-distribucion-binarios.md)**
- **[C2 — Observability en producción: logs, spans, métricas, OTel](m8-produccion-deploy/c2-observability-otel.md)**
- **[C3 — Secrets management: `secret()`, `config()` y `Secret<T>`](m8-produccion-deploy/c3-secrets-config.md)**
- **[C4 — Deploy avanzado: Docker, healthz/readyz, K8s, 12-factor](m8-produccion-deploy/c4-deploy-docker-k8s.md)**
- **[C5 — Deploy real de apps con interop Python (`--bundle-python` + `--bundle-pip`)](m8-produccion-deploy/c5-bundle-python-pip-deploy.md)**

**Entregable del módulo**: tu app de M6 (puramente Fitz) o M7 (con
interop Python) corriendo en producción real con `fitz docker init` +
monitoring (logs + spans + métricas) + K8s rolling deploys +
healthchecks + SIGTERM drain. Para apps con interop, el cap C5
agrega `fitz build --bundle-python --bundle-pip` para empaquetar
CPython + tus paquetes pip adentro del binario — **deploy = un solo
archivo, sin Python instalado en el destino**. **12-factor compliance
por default** sin instalar nada extra.

## M9 — Frontend nativo con `.fitzv`

Requisito explícito: M4 completo (HTTP first-class). El módulo
opcionalmente encaja post-M5 si querés WebSockets como
transport, pero podés hacer los caps con solo HTTP `@get` +
`fitz-liveviews` como dep.

Phase 11 CERRADA en el compilador (v0.21.0 → v0.21.3): Fitz
tiene una NUEVA extensión de archivo `.fitzv` con componentes
visuales de primera clase, template DSL con directivas, dos
backends de compilación (SSR y WASM client-side), y LSP full
para editar en VSCode con diagnostics + completions + hover +
go-to-def.

- **[C1 — Tu primer `.fitzv` (Counter component)](m9-fitzv-frontend/c1-primer-fitzv.md)**
- **[C2 — Template DSL: interpolación, directivas, composición](m9-fitzv-frontend/c2-template-dsl.md)**
- **[C3 — Full-page SFC: Board.fitzv migration del kanban](m9-fitzv-frontend/c3-full-page-sfc.md)**
- **[C4 — `@rpc`: funciones de servidor fullstack](m9-fitzv-frontend/c4-server-functions-rpc.md)**

**Entregable del módulo**: tenés un componente `.fitzv`
corriendo end-to-end con el runtime `fitz-liveviews` (WebSocket
+ diff/patch), sabés escribir templates con directivas
(`{#if}`/`{#for}`) + interpolaciones + composición (`<Child
prop="v" />`), viste la Board.fitzv full-page migration del
kanban como acceptance criterion, y cerraste el loop **fullstack**
con `@rpc` — llamar una función del server (DB/auth) directo desde
el `.fitzv` como si fuera local, con el mismo `type` compartido
back/front. El pattern de architecture `.fitz` (types + helpers) +
`.fitzv` (SFC) + `main.fitz` (HTTP+WS thin wire-up) queda en tu
toolkit.

Este módulo introduce la **superficie más nueva del lenguaje**
(post-v0.21.0). Las Phase 11.7 (client-side dynamic
capabilities + kanban SPA port) y post-11.9 (companion UI
library) son follow-ups que crecen sobre este módulo.

## Cómo está pensado el curso

- **Un proyecto que crece**: arrancás con un "hola mundo" y al final
  del M6 tenés una app CRUD completa con Postgres + auth + Docker.
- **Cada capítulo es corto** (5-15 min de lectura) y tiene su
  entregable commiteable en [`examples/curso/`](https://github.com/Thegreekman76/fitz/tree/main/examples/curso).
- **Validación al final de cada cap**: comandos exactos para confirmar
  que lo que hiciste funciona.
- **Cross-link a la guía** cuando el cap introduce algo nuevo.

## Si querés saltar a algo específico

- **Ya sabés Fitz, querés referencia**: andá a [guide.md](../guide.md).
- **Querés ver código real de proyectos**: [boilerplates](https://github.com/Thegreekman76/fitz/tree/main/boilerplates)
  (9 boilerplates Dockerizados, desde CLI tools hasta apps fullstack con
  Postgres).
- **Querés el detalle de ORM y DB**: [DB y ORM](../db-orm.md).
