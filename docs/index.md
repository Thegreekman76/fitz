---
hide:
  - navigation
  - toc
---

<div class="fitz-hero" markdown="0">
  <img src="assets/logo.png" alt="Fitz logo — engranaje de Rust con la silueta del Fitz Roy adentro" />
  <h1>Fitz</h1>
  <p class="fitz-tagline">
    Un lenguaje de programación compilado con HTTP, async, auth, WebSockets,
    jobs e interop Python como ciudadanos de primera clase del core del lenguaje.
  </p>
</div>

Sintaxis inspirada en Python/TypeScript, compilado a binario nativo
standalone (sin runtime en el destino), tipado gradual con checker
estático en compile-time.

```fitz
type User { id: Int, email: Str, name: Str, role: Str }

@auth_provider
fn check_token(headers: Map<Str, Str>) -> Result<User> {
    let auth = headers.get("authorization")?
    let claims = jwt.decode(auth, "secret")?
    return Ok(User { id: 1, email: claims["email"], name: "Ada", role: "admin" })
}

@authenticated
@get("/me")
fn me(user: User) -> User => user

@server(3000)
fn main() => 0
```

```bash
$ fitz build server.fitz
$ ./server
🏔️  Fitz HTTP escuchando en http://127.0.0.1:3000
   GET /me  🔒 (bearerAuth)
   GET /openapi.json  (schema autogenerado)
   GET /docs          (UI Scalar)
```

---

## ¿Por qué Fitz?

| | Python | TypeScript | Go | **Fitz** |
|---|---|---|---|---|
| Sintaxis limpia | ✅ | ⚠️ | ❌ | ✅ |
| Tipado gradual | ❌ | ✅ | ❌ | ✅ |
| Compilado nativo | ❌ | ❌ | ✅ | ✅ |
| **Multiplataforma** | ⚠️ | ⚠️ | ✅ | ✅ |
| HTTP en el core | ❌ | ❌ | ❌ | ✅ |
| Async nativo | ⚠️ | ✅ | ✅ | ✅ |
| Docs HTTP auto | ⚠️ | ❌ | ❌ | ✅ |
| **Auth nativa** | ❌ | ❌ | ❌ | ✅ |
| **WS tipados + AsyncAPI auto** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Jobs sin Celery** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Postgres + ORM nativo** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Observability OTel** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| Interop Python | ✅ | ❌ | ❌ | ✅ |

**Multiplataforma**: cada [release](https://github.com/Thegreekman76/fitz/releases/latest)
publica binarios + extensión VSCode + imagen Docker
(`ghcr.io/thegreekman76/fitz:latest`) para **4 plataformas**:
Windows x64, Linux x64, Linux ARM64 y macOS Apple Silicon. El mismo
programa Fitz corre en cualquiera; cross-compile gratis vía rustc
targets.

### Benchmark Fitz ORM vs SQLAlchemy

Para validar la promesa "binario nativo sin overhead" del ORM,
mantenemos un **bench reproducible cabeza-a-cabeza** entre los dos
boilerplates equivalentes
([`api-postgres-fitz`](https://github.com/Thegreekman76/fitz/tree/main/boilerplates/api-postgres-fitz)
vs [`api-postgres-python`](https://github.com/Thegreekman76/fitz/tree/main/boilerplates/api-postgres-python))
— mismo Postgres, mismos endpoints, misma firma. **Headline numbers
en v0.10.13** (Intel Core Ultra 7 155H, Docker 29.2.1, sustained
30s c=10):

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| Memory peak | **9.2 MB** | 51 MB | **5.5x más eficiente** |
| GET /users p50 | **4.88 ms** | 37.85 ms | **7.76x** |
| GET /users RPS | **1944** | 246 | **7.91x** |
| GET /users/{id} p50 | **3.60 ms** | 31.87 ms | **8.85x** |
| GET /users/{id} RPS | **2604** | 296 | **8.80x** |
| Cold start | **0.14 s** | 0.22 s | 1.57x |
| Image size | 131 MB | 258 MB | 2x más liviano |

Detalle, metodología y "cómo reproducir" en
[Benchmarks](benchmarks.md).

---

## Por dónde arrancar

[Curso de 0 a experto →](curso/index.md){ .md-button .md-button--primary }
[Guía completa →](guide.md){ .md-button }
[DB y ORM →](db-orm.md){ .md-button }
[Benchmarks →](benchmarks.md){ .md-button }
[Boilerplates →](https://github.com/Thegreekman76/fitz/tree/main/boilerplates){ .md-button }
[Ver el roadmap →](roadmap.md){ .md-button }
[GitHub →](https://github.com/Thegreekman76/fitz){ .md-button }

El **[curso `Fitz de 0 a experto`](curso/index.md)** es la entrada
recomendada si arrancás de cero. Te lleva paso a paso desde la
instalación hasta una app real con HTTP + auth + Postgres, con un
proyecto que crece capítulo a capítulo. M1 (setup) y M2 (tipos y
funciones) están cerrados (13 caps, ~9k LoC); M3 (módulos) recién
cerrado; M4-M7 en construcción.

La **guía** cubre 34 capítulos con ejemplos runnable en
[`examples/guide/`](https://github.com/Thegreekman76/fitz/tree/main/examples/guide):
desde `print("hola")` hasta servidores HTTP con auth + WebSockets +
cron jobs + Postgres + ORM nativo en menos de 100 líneas. Para el
stack DB completo (driver puro + ORM declarativo + relations +
JSONB/arrays + GROUP BY + eager loading + recetas), ver la
[guía exhaustiva DB y ORM](db-orm.md) (~2500 LoC dedicados al
diferencial con SQLAlchemy/Prisma/Diesel).

Para ver el stack completo en acción, los **6 boilerplates
Dockerizados** del repo cubren CLI puro, REST API, auth + frontend,
WebSockets con chat, CRUD multi-archivo con SQLAlchemy + Postgres,
y un showcase fullstack con frontend rico + Postgres en 3
containers. Cada uno con README exhaustivo. Ver el
[cap 35 de la guía](guide.md#35-plantillas-y-boilerplates).

### Extensión VSCode

La extensión con LSP (highlighting + diagnostics + hover +
go-to-def + autocomplete) viene en `.vsix` per-plataforma como
asset de cada [release](https://github.com/Thegreekman76/fitz/releases/latest).
Bajá el de tu OS/arquitectura (`fitz-lang-win32-x64.vsix`,
`fitz-lang-linux-x64.vsix`, `fitz-lang-linux-arm64.vsix` o
`fitz-lang-darwin-arm64.vsix`) e instalá desde VSCode:
`Ctrl+Shift+P` → "Extensions: Install from VSIX...". El binario
`fitz-lsp` viene bundleado adentro — no necesitás compilar nada
local.

Cuando la cuenta de publisher en el VSCode Marketplace esté lista,
la extensión va a estar instalable en un clic desde la UI de
Extensions. Detalle en
[cap 22 de la guía](guide.md#22-soporte-para-editores).

---

## Estado del proyecto

**Fases cerradas** (cierre formal de cada bloque en
[`CHANGELOG.md`](https://github.com/Thegreekman76/fitz/blob/main/CHANGELOG.md)):

- **Fase 2-3** — Lexer + parser + AST + intérprete + tipos
  custom + `Result` + módulos.
- **Fase 4** — HTTP nativo (`@get`/`@post`/`@put`/`@delete` +
  `@server`).
- **Fase 5a** — Type checker estático.
- **Fase 5b** — Codegen `fitz build` a binario nativo standalone.
- **Fase 6** — Async nativo (`async fn` + `.await` + `sleep` +
  `Future<T>`).
- **Fase 7** — OpenAPI 3.1 auto-generado + UI Scalar.
- **Fase 8** — Interop Python end-to-end (PyO3 + marshaling
  bidireccional + bridge async + `fitz py-types`).
- **Fase 9.x** — LSP MVP (diagnostics + hover + go-to-def +
  autocomplete) + distribución multi-platform.
- **Fase 9.y** — Package manager (`fitz new`/`add`/`remove` +
  `fitz.toml` + `fitz.lock` + path/git deps).
- **Fase 9.z** — DX completo (`fitz fmt`/`test`/`dev`/`repl`/
  `lint`).
- **Fase 9.w MVP** — Stack web first-class (auth + JWT/Argon2 +
  WebSockets tipados + AsyncAPI auto + cron + spawn).
- **Fase 10** — Stack DB nativo (driver Postgres puro Fitz + ORM
  declarativo + migraciones automáticas + transactions + composite
  PK + indexes + tipos avanzados Date/DateTime/Uuid).
- **Fase 11** (v0.11.0) — CLI builder nativo (`@command` con
  `--help` autogenerado, sin `clap`/`argparse`/`click`).
- **Fase 12.1-12.3** — Healthz/readyz + Secret + Observability
  minimal con OpenTelemetry (logs + spans + métricas + OTLP
  exporter).
- **Fase 12.4** — Dockerfile autogenerado (`fitz docker init` +
  `fitz docker build` con detección AST de @server/db/python/cron
  y runtime + compose adaptativos).

**Próximo norte**: Fase 12.5 (cap "Deployment ciudadano primera
clase" en la guía + caps del curso M7) y/o Fase 9.w.1.iter2 (RBAC
custom + token refresh).

---

## Nombre

Por el [Monte Fitz Roy](https://en.wikipedia.org/wiki/Fitz_Roy)
en El Chaltén, Patagonia, Argentina. Un nombre que no se olvida.
