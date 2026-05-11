# Visión — Por qué existe Fitz

## El problema

Construir y deployar un servicio web en 2025 requiere demasiadas decisiones
antes de escribir una línea de lógica real.

Con Python + FastAPI necesitás:
- Python instalado en el servidor
- Un virtualenv o contenedor
- FastAPI, Pydantic, Uvicorn instalados
- Un Dockerfile para que sea reproducible
- ~50 dependencias transitivas

El resultado es lento, pesado, y frágil en producción.

Con TypeScript + Node:
- Node instalado
- npm/yarn/pnpm (elegí uno, rezá)
- Express/Fastify/Hono + tipos
- El bagaje histórico de JS arrastrando todo
- Tipado que en producción no existe

Con Go:
- Binario nativo, rápido — pero la sintaxis es verborrágica
- El manejo de errores con `if err != nil` es repetitivo hasta el dolor
- No tiene la ergonomía de Python ni el tipado expresivo de TypeScript

Con Rust:
- Performance imbatible, memory safety
- Pero aprender Rust para hacer una API CRUD es matar moscas con un cañón
- La curva de aprendizaje ahuyenta a la mayoría

## La solución que Fitz propone

Un lenguaje que se siente como Python, tipea como TypeScript, compila como Go,
y tiene HTTP en el núcleo del lenguaje — no como biblioteca externa.

```fitz
@get("/hello/{name}")
async fn hello(name: Str) -> Str {
    return "Hola, {name} 🏔️"
}
```

Eso es un servicio web completo. Sin imports. Sin configuración. Sin boilerplate.

```bash
fitz build
./hello
# GET http://localhost:3000/hello/Fitz
# → "Hola, Fitz 🏔️"
```

Un binario. Sin runtime. Sin dependencias en producción.

## Para quién es Fitz

### Desarrolladores de Python
Que aman la sintaxis pero odian el performance y el costo de infraestructura.
Fitz se siente familiar desde el día uno pero el resultado es 10-50x más rápido.

### Desarrolladores de TypeScript
Que quieren tipado gradual sin el bagaje de JavaScript.
Fitz toma el tipado gradual y lo hace ciudadano de primera clase, compilado.

### Desarrolladores que construyen APIs/microservicios
Que están cansados de elegir entre ergonomía y performance.
Fitz no te hace elegir.

### Desarrolladores de IA/ML
Que necesitan performance pero no pueden abandonar el ecosistema Python.
Fitz tiene interop nativo con Python — llamás NumPy, PyTorch, cualquier lib,
directamente desde Fitz.

## Lo que Fitz NO es

- **No es un reemplazo de Rust** — Rust sigue siendo la opción para sistemas
  de bajo nivel, drivers, y código donde cada byte importa.

- **No es otro lenguaje de scripting** — Fitz compila a binario nativo.
  No hay intérprete en producción.

- **No intenta resolver todo** — Fitz está optimizado para servicios web,
  APIs, y tools. Para embedded o sistemas operativos, usá Rust.

## Principios de diseño

1. **Cero fricción hasta el primer deploy** — el camino feliz debe ser trivial
2. **Los tipos ayudan, no molestan** — opcionales, inferidos, progresivos
3. **HTTP es parte del lenguaje** — no una biblioteca más
4. **Los errores son valores** — no excepciones, no panics, `Result` siempre
5. **Un binario es suficiente** — el artefacto de producción es un solo archivo
6. **Interop antes que reescritura** — el ecosistema Python es demasiado valioso

## Origen

Fitz nació de una conversación en El Chaltén, Patagonia, Argentina.
Un desarrollador que amaba Python y FastAPI, aprendiendo Rust, preguntándose
si podía existir un lenguaje que tomara lo mejor de ambos mundos.

El nombre es por el Fitz Roy — la montaña más icónica de la Patagonia.
Imponente, reconocible, única. Como el lenguaje que intenta ser.
