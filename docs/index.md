---
hide:
  - navigation
  - toc
---

# Fitz

**Un lenguaje de programación compilado con HTTP, async, auth,
WebSockets, jobs e interop Python como ciudadanos de primera clase
del core del lenguaje.**

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
| HTTP en el core | ❌ | ❌ | ❌ | ✅ |
| Async nativo | ⚠️ | ✅ | ✅ | ✅ |
| Docs HTTP auto | ⚠️ | ❌ | ❌ | ✅ |
| **Auth nativa** | ❌ | ❌ | ❌ | ✅ |
| **WS tipados + AsyncAPI auto** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Jobs sin Celery** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| Interop Python | ✅ | ❌ | ❌ | ✅ |

---

## Por dónde arrancar

[Guía completa →](guide.md){ .md-button .md-button--primary }
[Ver el roadmap →](roadmap.md){ .md-button }
[GitHub →](https://github.com/Thegreekman76/fitz){ .md-button }

La guía cubre 30 capítulos con ejemplos runnable en
[`examples/guide/`](https://github.com/Thegreekman76/fitz/tree/main/examples/guide):
desde `print("hola")` hasta servidores HTTP con auth + WebSockets +
cron jobs en menos de 100 líneas.

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
en El Chaltén, Patagonia, Argentina.

[![Fitz Roy](https://upload.wikimedia.org/wikipedia/commons/thumb/7/79/Cerro_Torre_y_Cerro_Fitz_Roy_o_Chaltén.jpg/640px-Cerro_Torre_y_Cerro_Fitz_Roy_o_Chaltén.jpg)](https://en.wikipedia.org/wiki/Fitz_Roy)
