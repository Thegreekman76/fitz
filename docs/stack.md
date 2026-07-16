# Stack Architecture — Fitz + fitz-liveviews

Este documento fija la **arquitectura target** del stack Fitz a mediano y largo plazo. Sirve como norte para decisiones de "¿este feature va en fitz core o en fitz-liveviews?" y para el diseño de features grandes que atraviesan ambos repos (particularmente Fase 11 del roadmap fitz core, "frontend nativo").

Complementa:
- [`vision.md`](vision.md) — el **por qué** (posicionamiento vs otros lenguajes, marketing, filosofía)
- [`roadmap.md`](roadmap.md) — el **cuándo** (fases + timeline retrospectivo)
- [`architecture.md`](architecture.md) — el **cómo** (pipeline y módulos de `src/` hoy)
- [`design-decisions.md`](design-decisions.md) — el **por qué X sobre Y** (retrospectivo de decisiones ya tomadas)

Este doc es el **qué de mediano plazo** — cómo se ve la plataforma cuando madure.

---

## La visión: dos capas, una plataforma

El stack tiene dos artefactos con roles claros y separados:

**Fitz (lenguaje + compilador)** — plataforma **full-stack** por diseño.

- **Backend**: HTTP + WebSockets + auth + ORM + jobs + interop Python (todo lo que ya vive en fitz core hoy).
- **Frontend**: sintaxis SFC nativa (Fase 11) — `component X { state { ... } event ... <template>... </template> <style scoped>... </style> }`, targets SSR (Rust) y client (WASM/JS).

Fitz por sí solo puede resolver el rango entero: static site generation, SSR sin JS, MPA server-rendered, o SPA compilada a WASM. No requiere librerías externas para escribir UIs.

**fitz-liveviews (librería sobre Fitz)** — capa de **reactividad server-driven opt-in**.

- Consume la Fase 11 de Fitz core para renderizar componentes; no aporta sintaxis nueva al lenguaje.
- Aporta lo suyo: WebSocket bidireccional, patch engine (server-side HTML diff → JSON patches sobre el DOM del cliente), state store per-instance keyed por `(component_name, instance_id)`, dispatch de eventos client → server.
- El day 1 sin fitz-liveviews un user de Fitz **ya puede** hacer UIs completas — sólo pierde la reactividad WS que fitz-liveviews aporta. Es exactamente el mismo modelo de opcionalidad que `axios` sobre `fetch`, o Redux sobre React.

## El paralelo: Elixir + Phoenix + LiveView

La arquitectura target de Fitz es un mirror casi 1:1 del stack Elixir maduro:

| Rol | En el mundo Elixir | En el mundo Fitz |
|---|---|---|
| Lenguaje base | Elixir | Fitz |
| Runtime HTTP + auth + WebSockets + jobs | Phoenix (framework parte del ecosistema Elixir) | Fitz core (todo integrado en el lenguaje) |
| Sintaxis de templates | HEEx (parte de Phoenix, extensión del lenguaje) | Fase 11 SFC (parte de Fitz core) |
| Reactividad server-driven vía WebSocket | Phoenix LiveView (librería sobre Phoenix) | fitz-liveviews (librería sobre Fitz) |
| El componente sin reactividad | Server-rendered normal (HEEx solo) | Fitz SSR normal (Fase 11 solo) |

La diferencia práctica: en Elixir Phoenix es una librería separada; en Fitz decidimos hace tiempo que HTTP + WS + auth + jobs viven **en el core** (feature diferencial del lenguaje). Fase 11 sigue esa misma lógica — las UIs son ciudadano de primera clase.

## Cómo se relacionan

**Dirección de dependencia**: `fitz-liveviews → fitz`. Nunca al revés.

Fitz core no sabe que fitz-liveviews existe. fitz-liveviews es una dependencia declarada por el user en su `fitz.toml`:

```toml
[dependencies]
fitz_liveviews = { git = "https://github.com/Thegreekman76/fitz-liveviews", tag = "v0.4.2" }
```

fitz-liveviews **puede** usar decorators registrados en fitz core (`@live_component`, `@render_for`, `@on` — que viven en fitz core desde v0.20.0 porque son de propósito general para marcar componentes que necesitan reactividad). Pero fitz core NUNCA importa nada de fitz-liveviews.

**Versionado independiente**. Cada uno con su semver:
- Fitz core sigue `Cargo.toml` + `editors/vscode/package.json` (bumpean en lockstep desde v0.20.0).
- fitz-liveviews sigue su `fitz.toml` + `editors/vscode/package.json` (bumpean en lockstep desde v0.4.2 — ver [`fitz-liveviews/CHANGELOG.md`](https://github.com/Thegreekman76/fitz-liveviews/blob/main/CHANGELOG.md)).

Un release de Fitz core puede ocurrir sin release de fitz-liveviews, y viceversa. La compatibilidad entre ambos se documenta en el `CHANGELOG.md` de fitz-liveviews con la línea "Requires Fitz core vX.Y.Z+".

**Evolución paralela con puntos de sincronización naturales**:
- Features de fitz core que fitz-liveviews consume (nuevos decorators, sintaxis SFC de Fase 11) son sync points — fitz-liveviews suma una entrada en su CHANGELOG cuando adopta lo nuevo.
- Features de fitz-liveviews que no tocan el lenguaje (per-instance init payload de la lib, `dispatch_to_all` de la lib, template control flow inline) son independientes del cadence de fitz core.

## Invariantes durante el desarrollo de Fase 11

Fase 11 es el ítem más ambicioso que queda del roadmap fitz core y toca superficie sensible (lexer, parser, checker, codegen). El siguiente contrato es **inviolable** durante su desarrollo:

**Invariant 1** — Los ~370 ejemplos guide+curso+TaskHub del smoke `GUIDE_EXAMPLES_COMPILE` siguen compilando y corriendo idénticos al último release estable (v0.21.0 al momento de escribir este doc — Fase 11.1 → 11.5 + 11.6.a/b/c/d + 11.6.e §9.z/aa/bb shipped; 381 passed / 4 pre-existing failures documentados sin regresiones imputables).

**Invariant 2** — Los boilerplates (11 al momento de escribir esto) siguen pasando `fitz check` + `fitz build`.

**Invariant 3** — La superficie del parser `.fitz` clásico es 100% compatible-hacia-atrás. Código de v0.20.1 compila en la versión con Fase 11 (v0.21.0+) sin cambio ninguno.

**Invariant 4** — El parser nuevo de Fase 11 vive en un módulo dedicado (`src/view/` o similar) aislado del parser clásico. Un bug en el módulo nuevo NO puede romper `.fitz` clásico.

**Invariant 5** — La verificación de "no rompe nada" es la misma que ya se corre pre-bump: `cargo fmt --check` + `cargo clippy --lib --tests --bins -- -D warnings` + `cargo test --lib` + `cargo test --lib --features lsp` + `cargo test --test cli_e2e --release` + `cargo test --test compile_e2e --release`. Cero tests nuevos requeridos para verificar `.fitz` clásico; los tests actuales son el guardrail. Los tests nuevos de Fase 11 (parser + checker + codegen del SFC) se suman en paralelo.

Estos invariantes se referencian en cada PR de Fase 11 con checklist explícito en la descripción del PR.

## Consecuencias para el roadmap

**Fase 11 = fitz core**. La sintaxis SFC (`component X { ... }`, `<template>`, `{#if}`/`{#for}`, `<style scoped>`, `@click`, etc.) es **feature de fitz core**, no de fitz-liveviews. Un user de Fitz que sólo quiere SSG (static site generation) o SSR puro escribe `.fitzv` (o el nombre de extensión que se decida) y no toca fitz-liveviews para nada.

**fitz-liveviews no compite con Fase 11**. Es la capa que suma reactividad WS sobre los componentes que Fase 11 provee. Antes de Fase 11, fitz-liveviews usa el shim actual (`html("""...""")` + `flv()` + `h_when`/`h_join`) — funcional pero ilegible en componentes grandes. Cuando Fase 11 aterrice, fitz-liveviews **refactoriza sus ejemplos y su lib** para consumir la nueva sintaxis; el API público (`component()`, `dispatch_component_events()`, decorators `@live_component`/`@render_for`/`@on`) queda idéntico.

**Fitz-liveviews sigue evolucionando en paralelo mientras Fase 11 se desarrolla**. Los items del ROADMAP de fitz-liveviews que no tocan sintaxis del lenguaje (A.2 per-instance init payload, A.3 `dispatch_to_all(name, event, payload)`, presence primitives, per-user state across connections, `@every(N secs)` para periodic server-driven pushes) pueden avanzar sin bloquear ni ser bloqueados por Fase 11.

## Preguntas frecuentes de arquitectura

**¿Los tres decorators de LiveComponents (`@live_component`, `@render_for`, `@on`) por qué viven en fitz core y no en fitz-liveviews?**

Porque son marcadores de propósito general: "este type carga state reactivo", "este fn renderiza", "este fn maneja un evento". El runtime dispatch específico (crear WebSockets, hacer diff HTML, aplicar patches) sí vive en fitz-liveviews — pero la metadata que anota qué es qué es agnóstica del transporte de reactividad. Un framework hipotético alternativo a fitz-liveviews (uno basado en Server-Sent Events, por ejemplo) podría consumir la misma metadata sin cambios en el lenguaje.

**¿Fase 11 va a soportar múltiples targets frontend (WASM + JS + otros)?**

La visión es sí: WASM primero (más natural desde Rust), JS/vanilla como target secundario para casos donde el bundle WASM sea prohibitivo (edge functions, sites muy simples). El compilador SFC decide qué emitir por target basado en un flag del `fitz build`. El detalle del contrato entre targets se define al arrancar Fase 11.

**¿fitz-liveviews puede seguir vivo si Fase 11 nunca sale?**

Sí. La lib actual (v0.4.2+) es funcional end-to-end con el shim de `html("""...""")` + `flv()`. Los users pueden vivir con eso indefinidamente. Fase 11 es una mejora de DX, no un requisito de existencia de fitz-liveviews.

**¿Un cambio breaking en fitz core podría forzar a bumpear fitz-liveviews sin avisar?**

En teoría sí, en la práctica el modelo de dev del proyecto minimiza breaking changes de fitz core. Cuando ocurren (rarísimo), fitz-liveviews se bumpea con "Requires Fitz core vX.Y.Z+" en su CHANGELOG. La versión mínima de fitz core exigida por cada release de fitz-liveviews queda documentada en `fitz.toml` o README de la lib.

**¿Y si Fase 11 aparece antes de fitz-liveviews A.2/A.3?**

fitz-liveviews puede saltar directamente al refactor con Fase 11 y absorber A.2/A.3 en el mismo release (o postponer). La lib no tiene compromisos de fecha con sus users — cada release documenta qué agrega y qué requiere de fitz core.

**¿Este doc obliga a que Fase 11 arranque ya?**

No. Este doc es la constitución de cómo se ve el stack cuando madure. La decisión de cuándo arrancar Fase 11 depende de prioridades del autor (ver `roadmap.md`). Este doc sirve para que **cuando** se arranque, la arquitectura ya esté decidida y no haya que re-litigar el shape del feature.

---

**Versión de este doc**: v1 — 2026-07-14. Vive en el repo fitz core. Cambios a la visión target de arquitectura del stack pasan por PR contra este archivo.
