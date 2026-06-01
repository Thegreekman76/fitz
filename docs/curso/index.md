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
| M5 — Async, auth, real-time | 4 | 📋 planificado |
| M6 — Capstone Postgres + ORM nativo | 6 | 📋 planificado |
| M7 — Producción y deployment | 4 | ⏸ pendiente (espera Fase 12) |

Total previsto: 7 módulos, 36 capítulos. Cada módulo es **unidad
releasable independiente** — no hace falta esperar que esté todo
para empezar.

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
