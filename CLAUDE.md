# CLAUDE.md — Contexto del proyecto Fitz

Este archivo es para Claude Code. Contiene todo el contexto necesario para
continuar el desarrollo de Fitz sin perder el hilo.

---

## Qué es Fitz

Fitz es un lenguaje de programación nuevo, compilado, con tipado gradual,
sintaxis inspirada en Python/TypeScript, y HTTP/async como ciudadanos de
primera clase en el core del lenguaje. Está siendo construido en Rust.

El nombre es por el Fitz Roy, en El Chaltén, Patagonia, Argentina.

## Por qué existe

El autor usa Python, FastAPI, TypeScript y Vue.js. Ama la ergonomía de esos
lenguajes pero sufre sus limitaciones: Python es lento, TypeScript arrastra
el bagaje de JS, ninguno compila a binario nativo, ninguno tiene HTTP en el
core del lenguaje.

Fitz intenta resolver exactamente eso.

## Stack técnico

- **Implementación:** Rust
- **Paradigma del lenguaje:** imperativo con elementos funcionales
- **Tipado:** gradual (opcional, inferido cuando no se especifica)
- **Compilación:** objetivo final es binario nativo via LLVM, por ahora intérprete
- **Interop:** Python (via PyO3 a futuro)

## Estado actual

Fase 1 — Aprender Rust (en progreso)
Fase 2 — Intérprete base (pendiente)

Ver docs/roadmap.md para detalle completo.

## Sintaxis del lenguaje

Ver docs/syntax-spec.md para la especificación completa de sintaxis.

## Estructura del proyecto

```
fitz/
├── CLAUDE.md              # este archivo
├── README.md              # presentación pública
├── Cargo.toml             # proyecto Rust (cuando arranque Fase 2)
├── src/
│   ├── main.rs            # entry point
│   ├── lexer.rs           # tokenización
│   ├── parser.rs          # construcción del AST
│   ├── ast.rs             # definición del AST
│   ├── evaluator.rs       # ejecución del AST
│   └── error.rs           # manejo de errores
├── examples/              # programas de ejemplo en Fitz
│   ├── hello.fitz
│   ├── types.fitz
│   └── server.fitz
└── docs/
    ├── vision.md          # por qué y para quién
    ├── syntax-spec.md     # especificación de sintaxis
    ├── roadmap.md         # fases de desarrollo
    ├── naming.md          # decisiones de diseño
    └── references.md      # recursos y referencias
```

## Decisiones de diseño importantes

1. **HTTP nativo** — `@get`, `@post`, etc. son parte del lenguaje, no de una lib
2. **Sin excepciones** — manejo de errores via `Result` + `match`, como Rust
3. **Tipado gradual** — podés omitir tipos, el compilador infiere
4. **Strings con interpolación** — `"Hola, {name}"` nativo
5. **Punto y coma opcional** — como en Go, el parser maneja ambigüedades
6. **`fn` para funciones** — consistente, sin `def`/`function`/`func` distintos

## Convenciones de código (para el compilador en Rust)

- Cada fase del compilador en su propio módulo
- Tests unitarios en cada módulo (`#[cfg(test)]`)
- Errores descriptivos con línea y columna siempre
- Nombres en inglés en el código Rust, comentarios en español está bien

## Contexto del autor

- Developer full-stack independiente en El Chaltén, Patagonia, Argentina
- Stack principal: Python, FastAPI, Vue.js, Docker, PostgreSQL
- Proyectos activos: citai.ai (RAG SaaS), fisicainteractivaweb, juegoslogicaweb
- Aprendiendo Rust para construir Fitz
- Prefiere tono directo y casual, explicaciones concretas con código

## Cómo ayudar

Cuando el autor pida ayuda con Fitz, siempre:
1. Tener en cuenta las decisiones de diseño de arriba
2. Respetar la sintaxis definida en docs/syntax-spec.md
3. Explicar el código Rust porque está aprendiendo el lenguaje
4. Sugerir tests para cada componente nuevo
5. Pensar en la experiencia del usuario final del lenguaje
