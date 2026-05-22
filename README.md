<p align="center">
  <img src="assets/logo.png" alt="Fitz logo — engranaje de Rust con la silueta del Fitz Roy adentro" width="160" />
</p>

<p align="center">
  <em>Engranaje de Rust, Fitz Roy adentro: construido con Rust, nacido en una montaña.<br/>
  Más sobre el porqué del logo en <a href="docs/vision.md#el-logo">docs/vision.md → El logo</a>.</em>
</p>

# Fitz

> Un lenguaje de programación moderno, compilado y orientado a servicios web.
> Nacido en la Patagonia. Construido con Rust.

```fitz
// Un servicio HTTP, compilado a binario nativo, cero dependencias.

type User { id: Int, name: Str, email: Str? }

@get("/users/{id}")
async fn get_user(id: Int) -> User {
    return User { id: id, name: "ada", email: null }
}

@server(3000)
fn main() => 0
```

```bash
fitz run mi_app.fitz       # intérprete + checker estático
fitz build mi_app.fitz     # binario nativo standalone (~5 MB)
./mi_app                   # corre sin Fitz ni Rust en el destino
```

📖 **Documentación completa:** [thegreekman76.github.io/fitz](https://thegreekman76.github.io/fitz/)

## Por qué Fitz

Los lenguajes actuales te obligan a elegir entre ergonomía y performance:

- **Python** — hermoso, pero lento. Deployar es un dolor.
- **TypeScript** — tipado opcional de mentira, arrastra el bagaje de JS.
- **Go** — compilado y rápido, pero sintaxis verborrágica.
- **Rust** — perfecto por dentro, demasiado complejo para APIs.

**Fitz toma lo mejor de cada uno:**

| Feature                | Python | TypeScript | Go  | Fitz  |
| ---------------------- | ------ | ---------- | --- | ----- |
| Sintaxis limpia        | ✅     | ⚠️         | ❌  | ✅    |
| Tipado gradual         | ❌     | ✅         | ❌  | ✅ \* |
| Compilado nativo       | ❌     | ❌         | ✅  | ✅ †  |
| HTTP en el core        | ❌     | ❌         | ❌  | ✅    |
| Async nativo           | ⚠️     | ✅         | ✅  | ✅ ‡  |
| Docs HTTP automáticas  | ⚠️     | ❌         | ❌  | ✅ ◊  |
| **Auth nativa**        | ❌     | ❌         | ❌  | ✅ ♦  |
| **WebSockets tipados** | ⚠️     | ⚠️         | ⚠️  | ✅ ♣  |
| **Jobs sin Celery**    | ⚠️     | ⚠️         | ⚠️  | ✅ ♠  |
| Interop Python         | ✅     | ❌         | ❌  | ✅ §  |

\* **Tipado gradual con chequeo estático**. `fitz check` valida anotaciones en compile time; sin anotación se infiere o se trata como `Any`. → [cap 15 de la guía](https://thegreekman76.github.io/fitz/guide/#15-errores-y-mensajes).

† **Compilado nativo via transpile-a-Rust + Cargo**. Binario standalone ~5 MB, sin runtime de Fitz en el destino. Cross-compile gratis vía targets de rustc. → [cap 20](https://thegreekman76.github.io/fitz/guide/#20-fitz-build--compilar-a-binario-nativo).

‡ **Async nativo + paralelismo HTTP real**. `async fn` y `.await` postfix, `Future<T>` built-in, evaluator async sobre tokio multi-thread. 5 requests concurrentes en ~1.2s en vez de ~5s en serie. → [cap 19](https://thegreekman76.github.io/fitz/guide/#19-async-y-concurrencia).

◊ **OpenAPI 3.1 + UI Scalar automáticos** desde los decoradores. Schema bit-a-bit idéntico entre `fitz run`, `fitz openapi` y `fitz build`. → [cap 18](https://thegreekman76.github.io/fitz/guide/#18-docs-automáticas).

♦ **Auth nativa** con `@auth_provider`/`@authenticated`/`@admin` + built-ins `jwt` (HS256/384/512) y `hash` (Argon2id). Validación en compile-time, 401/403 automáticos en OpenAPI. Cero deps externas. → [cap 28](https://thegreekman76.github.io/fitz/guide/#28-auth-nativa).

♣ **WebSockets tipados** con `@ws("/path")` + `WsConn<T>`. Marshaling JSON automático, AsyncAPI 3.0 auto-generado en `/asyncapi.json`, heartbeat built-in, auth integrada en el handshake. → [cap 29](https://thegreekman76.github.io/fitz/guide/#29-websockets-tipados).

♠ **Jobs sin Celery** con `@cron("expr")` + `@background` + `spawn(fn_call)`. Sin broker externo (Redis/RabbitMQ no son requisito). Cron-only mode systemd-friendly. → [cap 30](https://thegreekman76.github.io/fitz/guide/#30-jobs-sin-celery).

§ **Interop Python via PyO3**. Marshaling bidireccional `List`/`Map`/`Instance` ↔ `list`/`dict`, excepciones Python → `Result<T>`, bridge async tokio ↔ asyncio, `fitz py-types` auto-mapeo SQLAlchemy. Opt-in con feature `python`. → [cap 21](https://thegreekman76.github.io/fitz/guide/#21-interop-python).

## Estabilidad

Fitz está construido sobre Rust, que tiene un compromiso de estabilidad fuerte desde 2015: código que compila en una versión estable sigue compilando en versiones futuras, y los cambios que podrían romper se aíslan en _editions_ opt-in.

Encima de eso, en este repo:

- `rust-toolchain.toml` pinea la versión exacta de Rust con la que Fitz se construye. Cloná el repo y `rustup` baja esa versión sola — no importa qué Rust tengas instalado globalmente.
- `rust-version` en `Cargo.toml` documenta la versión mínima soportada. Cargo da un error claro si alguien intenta con una más vieja.
- `Cargo.lock` fija las versiones exactas de todas las dependencias transitivas, así que builds reproducibles entre máquinas y en el tiempo.

En la práctica: un cambio en Rust o en una dependencia no rompe Fitz hasta que vos decidas subir las versiones de manera explícita.

El estado actual del proyecto (fases cerradas, próximo norte, deudas comprometidas) vive en [`docs/roadmap.md`](docs/roadmap.md) y en el [CHANGELOG](CHANGELOG.md).

## Cómo empezar

**1. Instalar Fitz.** Bajá el binario de tu plataforma desde [releases](https://github.com/Thegreekman76/fitz/releases/latest) (Linux x64/ARM, macOS ARM, Windows x64) y dejalo en cualquier carpeta del `PATH`. O compilá desde fuente:

```bash
git clone https://github.com/Thegreekman76/fitz.git
cd fitz
cargo build --release
# El binario queda en target/release/fitz
```

**2. Tu primer programa.**

```fitz
// hola.fitz
print("Hola desde Fitz 🏔️")

name = "Patagonia"
print("Hola, {name}!")
```

```bash
fitz run hola.fitz
```

**3. Seguir aprendiendo.** La [**guía del lenguaje**](https://thegreekman76.github.io/fitz/guide/) cubre todo lo implementado en español, con ejemplos ejecutables. Empezá por el [cap 2 — Tu primer programa](https://thegreekman76.github.io/fitz/guide/#2-tu-primer-programa). Para la especificación completa (incluye features futuras), ver [docs/syntax-spec.md](docs/syntax-spec.md).

## Boilerplates

Plantillas Dockerizadas listas para arrancar proyectos reales. Cada una tiene README exhaustivo con paso a paso, troubleshooting y plan de producción. Detalle completo en [`boilerplates/README.md`](boilerplates/README.md).

| Boilerplate | Qué demuestra |
|-------------|---------------|
| [`cli-tool`](boilerplates/cli-tool/)                           | CLI puro — sales report con métodos funcionales. Binario nativo distroless ~30 MB.                          |
| [`api-simple`](boilerplates/api-simple/)                       | REST API tipada + OpenAPI 3.1 + UI Scalar autogenerados.                                                    |
| [`api-middleware-cors`](boilerplates/api-middleware-cors/)     | Auth nativa JWT + Argon2 + middleware encadenado + CORS cross-origin + frontend vanilla.                    |
| [`api-websocket`](boilerplates/api-websocket/)                 | WebSockets tipados (`WsConn<T>`) con broadcast + heartbeat + frontend chat vanilla.                         |
| [`api-postgres-python`](boilerplates/api-postgres-python/)     | CRUD multi-archivo con SQLAlchemy + Postgres (compose 2 servicios).                                         |
| [`api-fullstack-postgres`](boilerplates/api-fullstack-postgres/) | **Showcase fullstack** — API + frontend rico (tabla, edit inline, filtros) + Postgres (compose 3 servicios). |

Quickstart genérico:

```bash
cd boilerplates/<nombre>
cp .env.example .env   # si existe
docker compose up --build   # o `docker build .`
```

## CLI

Verificá la instalación con `fitz --version` y `fitz --help`.

**Lenguaje — intérprete y compilador**

| Comando                       | Qué hace                                                                       |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `fitz run [archivo]`          | Ejecuta el archivo (o el `[bin].main` del `fitz.toml`) con checker strict.     |
| `fitz build [archivo]`        | Compila a binario nativo standalone (~5 MB, sin runtime de Fitz en el destino). |
| `fitz check [archivo]`        | Valida tipos y sintaxis sin ejecutar. Exit 1 si hay errores.                   |
| `fitz openapi <archivo>`      | Emite el schema OpenAPI 3.1 del programa a stdout. Útil para CI.               |

**Package manager**

| Comando                       | Qué hace                                                                       |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `fitz new <nombre>`           | Crea un proyecto Fitz nuevo en una carpeta. `--http` para template HTTP.       |
| `fitz init`                   | Inicializa un proyecto en el directorio actual.                                |
| `fitz add <dep> --path/--git` | Agrega una dep al `fitz.toml` y sincroniza el `fitz.lock`.                     |
| `fitz remove <dep>`           | Quita una dep del `fitz.toml`.                                                 |
| `fitz update [dep]`           | Re-resuelve deps. Para git deps invalida cache y re-clona.                     |

**Developer experience**

| Comando                       | Qué hace                                                                       |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `fitz fmt [archivos]`         | Formatea código Fitz a estilo canónico, cero config. `--check` para CI.        |
| `fitz test [filter]`          | Corre fns con `@test`. Output estilo cargo (ok/FAILED + summary + exit code).  |
| `fitz dev [--file]`           | Hot reload — re-arranca el programa cuando un `.fitz` o `fitz.toml` cambia.    |
| `fitz repl`                   | REPL interactivo con env persistente, multi-line, history, comandos `:type`/`:env`/`:load`. |
| `fitz lint [archivos]`        | Linter de patrones (unused_variable, unused_import, useless_match, string_concat). |

**Interop Python** (requiere `cargo build --release --features python`)

| Comando                       | Qué hace                                                                       |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `fitz py-types <archivo.py>`  | Genera `type` Fitz desde modelos SQLAlchemy. Output a stdout o `--out`.        |

## Extensión VSCode

Highlighting + LSP (diagnostics, hover, go-to-definition, autocomplete contextual) con el binario `fitz-lsp` bundleado en cada `.vsix` por plataforma.

**Instalar desde releases (recomendado)**:

1. Bajá el `.vsix` correspondiente a tu OS/arquitectura de [releases](https://github.com/Thegreekman76/fitz/releases/latest):
   - **Windows x64**: `fitz-language-win32-x64-X.Y.Z.vsix`
   - **Windows ARM64**: `fitz-language-win32-arm64-X.Y.Z.vsix`
   - **Linux x64**: `fitz-language-linux-x64-X.Y.Z.vsix`
   - **Linux ARM64**: `fitz-language-linux-arm64-X.Y.Z.vsix`
   - **macOS Apple Silicon (M1/M2/M3)**: `fitz-language-darwin-arm64-X.Y.Z.vsix`
2. En VSCode: `Ctrl+Shift+P` → "Extensions: Install from VSIX..." → seleccioná el archivo bajado.
3. Listo — abrí cualquier `.fitz` y vas a tener errores subrayados al tipear, hover con tipos, F12 para go-to-definition, y autocomplete contextual.

> Nota: macOS Intel (`darwin-x64`) no está en la matriz de release por escasez crónica de runners macos-13 en GitHub Actions. Si lo necesitás, build local (próxima sección) funciona idéntico.
>
> Cuando se cree la cuenta de publisher en el VSCode Marketplace, la extensión va a estar instalable en un clic desde la UI de Extensions buscando `fitz`. Por ahora releases en GitHub es el camino canónico.

**Build local** (alternativa, si querés trackear `main` o no encontrás tu plataforma en releases):

```bash
cd editors/vscode
npm install
npm run build:vsix     # produce un `.vsix` para tu plataforma actual
```

Detalle completo en [cap 22 de la guía](https://thegreekman76.github.io/fitz/guide/#22-soporte-para-editores).

## Nombre

**Fitz** por el Fitz Roy — la montaña más icónica de la Patagonia, en El Chaltén, Argentina. Un nombre que no se olvida.

## Autor

Desarrollado en El Chaltén, Santa Cruz, Argentina 🇦🇷
Por un developer independiente que quería un lenguaje que no tuviera que disculparse por nada.

TheGreekMan (Palopoli Martín)

## Licencia

MIT
