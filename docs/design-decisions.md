# Decisiones de Diseño — Fitz

Este documento registra las decisiones de diseño del lenguaje y el razonamiento
detrás de cada una. Útil para mantener consistencia y recordar el "por qué".

---

## Sintaxis general

### `fn` para funciones (no `def`, no `function`, no `func`)
**Decisión:** `fn`
**Razón:** Corto, claro, consistente. Tomado de Rust. Evita la ambigüedad de
`function` de JS o `def` de Python que muchos developers nuevos asocian a algo
específico.

### Llaves `{}` para bloques (no indentación significativa)
**Decisión:** llaves obligatorias
**Razón:** La indentación significativa de Python es elegante pero problemática
en editores, copiar/pegar, y colaboración. Las llaves son explícitas y no fallan.

### Punto y coma opcional
**Decisión:** punto y coma no requerido (como Go, Kotlin, Swift)
**Razón:** El ruido visual del `;` final no aporta nada semántico. El parser
puede determinar el fin de una sentencia por contexto.

### `let` opcional para declarar variables
**Decisión:** ambas formas conviven — `x = 1` y `let x = 1` declaran la misma var.
**Razón:** la simplicidad à la Python (sin `let`) baja la barrera de entrada;
`let` opcional ayuda a la legibilidad cuando hay muchas declaraciones juntas o
cuando alguien viene de Rust/Swift/JS. El parser acepta ambas sin distinción
semántica — no hay diferencia entre "declarar" y "reasignar" más allá del
scope. Mutabilidad por defecto, como Python.
**Trade-off aceptado:** dos formas para hacer lo mismo. Cuando el formateador
exista (`fitz fmt`), va a elegir una canónica.

---

## Tipos

### Tipado gradual
**Decisión:** Los tipos son opcionales y siempre inferidos cuando no se especifican.
**Razón:** La barrera de entrada de Rust/TypeScript estricto ahuyenta a muchos.
Fitz debe sentirse fácil al principio y seguro cuando lo necesitás.

```fitz
x = 42              // válido — inferido como Int
x: Int = 42         // válido — explícito
```

### Nullable con `?`
**Decisión:** `Str?` indica que puede ser null, `Str` nunca puede ser null.
**Razón:** Tomado de Kotlin/TypeScript. Es el sistema de nullabilidad más
ergonómico conocido. Evita el `NullPointerException` silencioso.

### Nombres de tipos en PascalCase
**Decisión:** `Int`, `Str`, `Float`, `Bool`, `List<T>`, `Map<K,V>`, `User`
**Razón:** Distinción clara entre tipos y valores. Convención universal.

### `Str` en vez de `String`
**Decisión:** `Str`
**Razón:** Más corto. Se escribe decenas de veces. `String` viene del Java y
arrastra connotaciones de mutabilidad que Fitz no necesita.

---

## Manejo de errores

### Result + match, sin excepciones
**Decisión:** No hay `try/catch`. Los errores son valores de tipo `Result<T>`.
**Razón:** Las excepciones rompen el flujo de control de forma invisible.
`Result` hace explícito que algo puede fallar. Tomado de Rust, pero con
la ergonomía del operador `?` para propagación.

```fitz
// explícito
match db.find(id).await {
    Ok(user) => return user
    Err(e)   => return 404 { message: e }
}

// propagación automática con ?
async fn get_name(id: Int) -> Result<Str> {
    let user = db.find(id).await?   // si falla, retorna el Err automáticamente
    return Ok(user.name)
}
```

---

## HTTP

### Decoradores como parte del lenguaje
**Decisión:** `@get`, `@post`, `@put`, `@delete` son keywords del lenguaje.
**Razón:** Esta es la feature definitoria de Fitz. Si HTTP requiere imports
y configuración, perdemos el punto. La magia de FastAPI es exactamente esto:
definís una función, le ponés un decorador, y ya es un endpoint.

### Serialización automática por tipo de retorno
**Decisión:** Si retornás un `type` definido en Fitz, se serializa a JSON automáticamente.
**Razón:** Elimina el boilerplate de `json.dumps`, `jsonify`, etc. El tipo
es el contrato, el runtime se encarga del resto. Igual que FastAPI con Pydantic.

### Servidor automático con `@server(...) fn main()` como patrón canónico
**Decisión:** si hay rutas registradas (`@get`, `@post`, etc.), `fitz run`
arranca un servidor HTTP automáticamente. La configuración del server vive
en `@server(port, host)` y se aplica sobre una `fn main()` placeholder
(`@server(3000) fn main() => 0`). La fn queda definida en el env pero **no
se ejecuta automáticamente** — el server arranca por la presencia de rutas,
no por la presencia de `main`.
**Razón:** el camino feliz sigue siendo trivial (escribir endpoints, correr,
funcionar). El patrón `@server fn main` resuelve dos cosas a la vez: tener
un anchor sintáctico para configurar host/port sin inventar otra keyword, y
dejar la puerta abierta a programas mixtos (CLI + HTTP) sin chocar.
**Trade-off:** `fn main` no es entry point en sentido tradicional cuando hay
HTTP — el "entry point real" es el reactor axum/tokio.

---

## Type checker (Fase 5a)

### Strict por default en `fitz run`, escape vía `--no-typecheck`
**Decisión:** `fitz run` aborta si el checker encuentra errores de tipo.
Para volver al modo gradual (warning + ejecutar), hay que pasar
`--no-typecheck` explícitamente. `fitz build` siempre exige tipos OK,
sin flag de escape.
**Razón:** el valor del tipado gradual está en que los errores se vean
cuando aparecen, no en que se ignoren silenciosamente. Strict por default
empuja a corregir; el flag de escape preserva la posibilidad de ejecutar
prototipos rotos cuando hace falta. Build no debe producir un binario que
el checker considera incorrecto.

### El sistema de tipos no analiza valores
**Decisión:** el checker se preocupa por tipos (forma), no por dominio
(valores). División por cero, indexing fuera de rango, claves faltantes
en `Map.get` — todo eso es runtime, no checker.
**Razón:** un sistema de tipos que intente analizar valores se vuelve un
solver costoso y nunca llega a cero falsos negativos. Fitz prefiere un
checker barato y predecible, y dejar al runtime los errores de dominio
(con `Result` cuando se puede expresar como contrato).

---

## Codegen a binario nativo (Fase 5b)

### Transpile-a-Rust sobre LLVM/Cranelift directos
**Decisión:** `fitz build` traduce el AST tipado de Fitz a código Rust,
y delega el resto del trabajo a `rustc`/`cargo`.
**Razón:** reusamos toda la infraestructura del ecosistema Rust de un
saque — compilador maduro, optimizaciones, cross-compilation gratis,
y las crates que ya usamos en runtime (`axum`, `tokio`, `serde`)
encajan naturalmente cuando llega HTTP en el binario. Construir IR a
LLVM/Cranelift desde cero implicaba reimplementar async, codegen para
HTTP, y mantener dos representaciones de tipos.
**Trade-off:** el tiempo de compilación queda atado a `rustc` (~2s en
programas chicos). El binario producido es standalone y no requiere
runtime de Fitz en producción — eso era el principio de la visión y
hoy es realidad implementada.

### `fitz build` siempre genera un proyecto Cargo
**Decisión:** el output del codegen es `target/fitz-build/<stem>/
{Cargo.toml, src/main.rs, src/<mod>.rs}`, y se invoca `cargo build
--release`. No invocamos `rustc` directo aunque no haya módulos.
**Razón:** los `import` cross-archivo necesitan múltiples `.rs` con
`mod`, que es la abstracción nativa de Cargo. Y cuando HTTP suma
deps (axum/tokio/serde), Cargo las maneja sin reescribir pipeline.
Costo: ~1-2s adicionales en la primera compilación. Aceptable.

### `Arc<Mutex<>>` para tipos custom y colecciones en el binario (post-F17.4b)
**Decisión:** `type Foo { ... }` Fitz → `struct FooData` Rust + alias
`type Foo = Arc<Mutex<FooData>>`. `List<T>` → `Arc<Mutex<Vec<T>>>`.
`Map<K, V>` → `Arc<Mutex<Vec<(K, V)>>>`. `Mutex` es `std::sync::Mutex`
(sin deps extras en el Cargo.toml generado).
**Razón:** preserva la semántica de referencia compartida del intérprete
(mutaciones vía cualquier alias se ven en todos los alias, incluyendo
args de fn) **y** habilita que `Value` y `EnvRef` sean `Send + Sync`
— condición necesaria para que axum pueda invocar al evaluator
sobre un runtime tokio multi-thread. Pre-F17 el codegen emitía
`Rc<RefCell<>>` (single-thread, sin Send) y el runtime HTTP estaba
forzado a `current_thread`. Post-F17 el server compilado responde N
requests en paralelo (validado con 5 reqs concurrentes a un handler
`sleep(1000)` en ~1.2s vs ~5s pre-F17). El intérprete usa
`parking_lot::Mutex` (más rápido, no envenena); el codegen output
usa `std::sync::Mutex` para no agregar deps al binario producido.
**Trade-off:** `u.name` se traduce a un bloque acotado
`{ let __obj = u.clone(); let __g = __obj.lock().unwrap(); __g.name.clone() }`
en lugar de un simple field access. El bloque libera el guard al fin
del bloque (no del statement) — sin esto, dos accesos al mismo
Mutex en una sola expresión (típicamente adentro de `format!(...)`)
producen deadlock porque `std::sync::Mutex` no es reentrante.
`PartialEq` deja de derivarse y se emite manual con helper
`field_eq_expr` (Mutex<T> no impl PartialEq) — espejo del patrón del
intérprete (`Arc::ptr_eq` shortcut + lock+deref).
**Decisión paralela:** state HTTP compartido pasa de `thread_local!`
(F11 pre-F17, asumía current_thread) a
`static X: LazyLock<Arc<Mutex<T>>> = LazyLock::new(|| ...);`
para que un solo Arc se comparta entre todos los workers tokio.

### `Result<T>` Fitz → `Result<T, String>` Rust
**Decisión:** en el codegen, el lado Err de `Result` queda pinned a
`String`. Construir `Err(42)` se coerce a `Err(format!("{}", 42))`.
**Razón:** en la práctica todos los `Err(...)` de los ejemplos y del
runtime construyen mensajes (`find`/`get` devuelven Str, divisiones
por cero también). Pinear E a String habilita el `?` Rust nativo sin
glue extra. La generalización a errores tipados queda como deuda
abierta para cuando aparezca presión real (custom error types).
**Trade-off:** se pierde la posibilidad de matchear sobre `Err` de
tipo arbitrario en el binario. Hoy nadie lo hacía.

---

## Naming

### Nombre del lenguaje: Fitz
**Razón:** Fitz Roy es la montaña más icónica de la Patagonia argentina.
Reconocible internacionalmente, único, memorable. Evoca algo sólido y permanente.

### Extensión de archivos: `.fitz`
**Razón:** Único, claro, descriptivo. No hay colisión con ningún otro lenguaje.

### Comando CLI: `fitz`
```bash
fitz run main.fitz                   # ejecutar (strict por default)
fitz run main.fitz --no-typecheck    # ejecutar ignorando errores de tipo
fitz check main.fitz                 # solo type check
fitz build main.fitz                 # compilar a binario nativo (vía Cargo)
fitz fmt                             # formatear (futuro)
fitz add http                        # instalar paquete (futuro)
```

---

## Lo que Fitz deliberadamente NO tiene

- **Clases** — Fitz usa tipos (structs) y funciones. La OOP clásica con herencia
  genera más problemas de los que resuelve.
- **Herencia** — composición sobre herencia, siempre.
- **Excepciones** — Result y match son suficientes y más explícitos.
- **Punto y coma obligatorio** — ruido visual innecesario.
- **`null` sin marcar** — todo tipo no-nullable es seguro por construcción.
- **Tipado estático obligatorio** — la gradualidad es un feature, no una debilidad.
