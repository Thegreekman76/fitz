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

## Cómo se compara con otros lenguajes

Esta sección es honesta. Fitz no es mejor en todo y no quiere serlo —
está optimizado para un nicho específico: **servicios web y APIs que
hoy se escriben en Python o TypeScript**. Para todo lo demás, hay
mejores herramientas.

| Dimensión              | Fitz       | Python   | TypeScript | Rust      | Go        |
|------------------------|------------|----------|------------|-----------|-----------|
| Sintaxis ergonómica    | ✓          | ✓        | ✓          | △         | △         |
| Tipado gradual         | ✓          | △ (mypy) | ✓          | ✗ estricto| ✗ estricto|
| Compila a binario      | ✓          | ✗        | ✗          | ✓         | ✓         |
| HTTP en el core        | ✓          | ✗        | ✗          | ✗         | △ (net/http)|
| Errores como valores   | ✓          | ✗        | ✗          | ✓         | △ (idiomático)|
| Sin runtime en prod    | ✓          | ✗        | ✗          | ✓         | ✓         |
| Performance            | alto (compilado) | bajo     | medio      | alto      | alto      |
| Ecosistema             | naciente   | enorme   | enorme     | grande    | grande    |
| Curva de aprendizaje   | suave      | suave    | suave      | empinada  | suave     |

✓ = lo hace bien · △ = parcial · ✗ = no lo hace

### vs. Python

**Por qué Fitz** — si ya escribís FastAPI todos los días, sabés dónde
duele: en producción necesitás Python instalado, un virtualenv, ~50
dependencias transitivas, y aun así el endpoint tarda más de lo que
debería. Fitz toma la ergonomía que te gusta y la compila a un
binario sin runtime; tus anotaciones de tipo se chequean estáticamente
en vez de quedar como decoración; los errores son `Result` en vez de
excepciones que se escapan del scope.

**Por qué Python igual** — el ecosistema. NumPy, pandas, scikit-learn,
PyTorch, Django, Jupyter, 30 años de librerías. Cualquier cosa con
ML, ciencia de datos o scripting de pegamento, Python sigue ganando.
Por eso Fitz va a tener **interop nativo con Python** (Fase 8, vía
PyO3): para que no tengas que elegir entre el ecosistema y la
performance.

```fitz
// Fitz
@get("/users/{id}")
fn get_user(id: Int) -> User {
    return User { id: id, name: "Fitz" }
}
```

```python
# Python + FastAPI
from fastapi import FastAPI
from pydantic import BaseModel

app = FastAPI()

class User(BaseModel):
    id: int
    name: str

@app.get("/users/{id}")
def get_user(id: int) -> User:
    return User(id=id, name="Fitz")
```

Mismo resultado. En Fitz no importás nada, no instalás `fastapi`, no
necesitás `pydantic` ni `uvicorn`. El `fitz build` te da un binario.

### vs. TypeScript

**Por qué Fitz** — TypeScript te da tipado gradual, pero en runtime
no existe: el código que corre es JavaScript con todos sus defectos
históricos (`undefined`, coerciones raras, `==` vs `===`). Y aunque
quieras compilar a algo "rápido", arrastrás V8 o Node. Fitz toma la
idea del tipado gradual y la lleva al binario: lo que ves en el
código es lo que corre.

**Por qué TypeScript igual** — frontend. Fitz no tiene intención de
competir en el browser (al menos no por ahora; WebAssembly aparece
en Fase 9 como visión). Si necesitás un componente UI, TypeScript +
React/Vue es la respuesta correcta. Y el ecosistema npm, con todos
sus problemas, sigue siendo el más grande del mundo dev.

```fitz
// Fitz
type User { id: Int, name: Str, email: Str? }

@post("/users")
async fn create_user(input: UserInput) -> User {
    // tipos se chequean en compile-time; nada de "string | undefined"
}
```

```typescript
// TypeScript + Express
import express from 'express';
const app = express();
app.use(express.json());

interface User { id: number; name: string; email?: string; }
interface UserInput { name: string; email?: string; }

app.post('/users', async (req, res) => {
    const input = req.body as UserInput;  // sin validación real
    res.json({ id: 1, name: input.name });
});
```

Notá las dos diferencias: tipos validados de verdad, y el endpoint
es parte del lenguaje en vez de una llamada `app.post(...)`.

### vs. Rust

**Por qué Fitz** — Rust es brutalmente potente, pero para una API
CRUD es matar moscas a cañonazos. `ownership`, lifetimes, `Arc<Mutex<T>>`,
elegir entre `tokio`/`async-std`, configurar `axum` o `actix-web`...
todo eso es deuda mental para escribir lógica de negocio simple.
Fitz toma decisiones por vos (gestión de memoria implícita, runtime
HTTP integrado, tipado opcional) para que el camino feliz sea
trivial.

**Por qué Rust igual** — todo lo que esté por debajo del nivel de
aplicación web. Sistemas operativos, motores de juego, embedded,
intérpretes de otros lenguajes (¡como el propio Fitz!), código donde
cada byte y cada microsegundo importan. Y la garantía de memory
safety estricta de Rust no la vas a tener en Fitz: el tipado gradual
implica que algunas verificaciones quedan para runtime.

Fitz **está escrito en Rust**, no es un competidor — es lo que
construís cuando ya conocés Rust y querés un lenguaje con menos
fricción para el day-to-day web.

### vs. Go

**Por qué Fitz** — Go gana en simplicidad y compilación rápida, pero
la sintaxis es austera al punto de la verborragia: `if err != nil`
en cada línea, structs sin métodos, sin pattern matching, sin
interpolación de strings. Fitz toma la decisión "lenguaje simple,
binario nativo" de Go y le suma la ergonomía moderna: `Result` con
`?`, `match` con patrones, `f"Hola, {name}"`, tipos genéricos
(cuando lleguen).

**Por qué Go igual** — el ecosistema cloud-native. Kubernetes,
Docker, Terraform, gRPC, etcd, todo el stack de infraestructura
moderna está escrito en Go. Las goroutines + channels son un modelo
de concurrencia limpio y probado en producción a escala enorme.
Para herramientas de infra y servicios distribuidos a gran escala,
Go tiene años de ventaja.

```fitz
// Fitz
fn parse_age(s: Str) -> Result<Int> {
    let n = to_int(s)?
    return Ok(n)
}
```

```go
// Go
func parseAge(s string) (int, error) {
    n, err := strconv.Atoi(s)
    if err != nil {
        return 0, err
    }
    return n, nil
}
```

Tres líneas vs siete. La diferencia se acumula en un codebase real.

### Resumen honesto

Fitz tiene sentido si:

- Estás escribiendo **APIs o servicios web** y la mayoría del trabajo
  es endpoints HTTP + lógica de dominio + persistencia.
- Vas y volvés entre **Python y TypeScript** y sufrís deploys,
  performance, o el tipado que no se valida.
- Querés un **binario** en producción sin runtime ni dependencias.

Fitz **no** tiene sentido (todavía) si:

- Necesitás un **ecosistema enorme y maduro** ya mismo (cualquier
  cosa de ML, frontend, infra de bajo nivel).
- Estás haciendo **sistemas de bajo nivel** o **embedded** — usá Rust.
- Tu equipo o producto necesita **estabilidad de un lenguaje
  consolidado** — Fitz es joven, las APIs pueden cambiar.

El nicho no es "reemplazar todo": es "ser la mejor opción cuando
estás escribiendo el backend de una app moderna".

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

La promesa se extiende más allá del primer endpoint. El roadmap a
mediano plazo (Fase 9) suma **stack web first-class** —
`@authenticated`, `@ws`, `@cron`, `@background` como decoradores
del lenguaje, no como combinación de 5 librerías a integrar — y
**DX completo** (formatter sin config, test runner built-in, hot
reload, REPL, linter, package manager + registry escrito en Fitz
mismo). El norte largo: Fase 10 stack DB nativo + ORM declarativo,
Fase 11 frontend en `.fitz` (componentes con SSR built-in, el
mismo `type` para back y front), Fase 12 deployment de un solo
comando (`fitz deploy` con Dockerfile autogenerado y observability
nativa). Ver [`docs/roadmap.md`](roadmap.md) para el plan
completo.

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

## El logo

El logo de Fitz es un **engranaje de Rust** con la **silueta del
Fitz Roy adentro**. Cuenta exactamente lo que es el proyecto:

- **El engranaje** — en el naranja oficial de Rust (`#CE412B`) —
  representa el lenguaje con el que Fitz está construido. Fitz es,
  en su corazón, un compilador escrito en Rust; el engranaje
  reconoce esa deuda y se inscribe en el ecosistema Rust en lugar
  de fingir ser otra cosa.
- **El Fitz Roy adentro** — tres picos, el central más alto —
  recuerda el origen del nombre y del proyecto. La montaña que se
  ve desde El Chaltén, imponente y reconocible, da el nombre y la
  identidad: un lenguaje nacido en la Patagonia, no en Silicon
  Valley.

La combinación dice "construido con Rust, nacido en una montaña":
**ingeniería sólida sirviendo una identidad clara**. Es la misma
tesis que el lenguaje propone — tomar lo mejor de un stack
existente (Rust como base, Python/TS como ergonomía) para algo
nuevo con personalidad propia.
