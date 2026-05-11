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

### `let` para variables locales inmutables, `=` para mutables
**Decisión:** TBD — en debate
**Opción A:** Todo con `=`, mutabilidad por defecto
**Opción B:** `let` para inmutable, `let mut` para mutable (como Rust)
**Tendencia:** Opción A para mantener la simplicidad à la Python.

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

### Servidor automático
**Decisión:** Si hay rutas definidas, `fitz run` arranca un servidor HTTP.
No hace falta un `main()` ni configuración extra.
**Razón:** El camino feliz debe ser trivial. Escribís endpoints, corrés, funciona.

---

## Naming

### Nombre del lenguaje: Fitz
**Razón:** Fitz Roy es la montaña más icónica de la Patagonia argentina.
Reconocible internacionalmente, único, memorable. Evoca algo sólido y permanente.

### Extensión de archivos: `.fitz`
**Razón:** Único, claro, descriptivo. No hay colisión con ningún otro lenguaje.

### Comando CLI: `fitz`
```bash
fitz run main.fitz      # ejecutar
fitz build              # compilar
fitz check              # type check y lint
fitz fmt                # formatear
fitz add http           # instalar paquete (futuro)
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
