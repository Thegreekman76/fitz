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
| Interop Python | ✅ | ❌ | ❌ | ✅ |

**Multiplataforma**: cada [release](https://github.com/Thegreekman76/fitz/releases/latest)
publica binarios + extensión VSCode + imagen Docker
(`ghcr.io/thegreekman76/fitz:latest`) para **4 plataformas**:
Windows x64, Linux x64, Linux ARM64 y macOS Apple Silicon. El mismo
programa Fitz corre en cualquiera; cross-compile gratis vía rustc
targets.

---

## Por dónde arrancar

[Guía completa →](guide.md){ .md-button .md-button--primary }
[DB y ORM →](db-orm.md){ .md-button }
[Boilerplates →](https://github.com/Thegreekman76/fitz/tree/main/boilerplates){ .md-button }
[Ver el roadmap →](roadmap.md){ .md-button }
[GitHub →](https://github.com/Thegreekman76/fitz){ .md-button }

La guía cubre 34 capítulos con ejemplos runnable en
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
[cap 33 de la guía](guide.md#33-plantillas-y-boilerplates).

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

**Próximo norte**: Fase 10 — Stack DB nativo (driver Postgres
puro Fitz + ORM declarativo + migraciones autogeneradas).
Mientras tanto, interop Python con SQLAlchemy cubre el gap
(cap 21 de la guía).

---

## Nombre

Por el [Monte Fitz Roy](https://en.wikipedia.org/wiki/Fitz_Roy)
en El Chaltén, Patagonia, Argentina. Un nombre que no se olvida.
