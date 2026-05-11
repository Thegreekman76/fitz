# Fitz 🏔️

> Un lenguaje de programación moderno, compilado y orientado a servicios web.  
> Nacido en la Patagonia. Construido con Rust.

```fitz
@get("/users/{id}")
async fn get_user(id: Int) -> User {
    let user = db.find(id).await
    return user
}
```

## Por qué Fitz

Los lenguajes actuales te obligan a elegir entre ergonomía y performance:

- **Python** — hermoso, pero lento. Deployar es un dolor.
- **TypeScript** — tipado opcional de mentira, arrastra el bagaje de JS.
- **Go** — compilado y rápido, pero sintaxis verborrágica.
- **Rust** — perfecto por dentro, demasiado complejo para APIs.

**Fitz toma lo mejor de cada uno:**

| Feature | Python | TypeScript | Go | Fitz |
|---|---|---|---|---|
| Sintaxis limpia | ✅ | ⚠️ | ❌ | ✅ |
| Tipado gradual | ❌ | ✅ | ❌ | ✅ |
| Compilado nativo | ❌ | ❌ | ✅ | ✅ |
| HTTP en el core | ❌ | ❌ | ❌ | ✅ |
| Async nativo | ⚠️ | ✅ | ✅ | ✅ |
| Interop Python | ✅ | ❌ | ❌ | ✅ |

## Ejemplo rápido

```fitz
// main.fitz — un servicio completo, un archivo, cero dependencias

type User {
    id: Int
    name: Str
    email: Str?
}

@get("/")
async fn index() -> Str {
    return "Fitz corriendo 🏔️"
}

@get("/users/{id}")
async fn get_user(id: Int) -> User {
    let user = db.find(id).await
    match user {
        Ok(u)  => return u
        Err(e) => return 404 { message: e }
    }
}
```

```bash
fitz build && ./main
# Servidor en http://localhost:3000
```

Un binario. Sin dependencias en producción.

## Estado del proyecto

🚧 **En desarrollo activo — Fase 2: Intérprete base** (lexer y AST completos, parser en camino)

Ver [roadmap](docs/roadmap.md) para el estado detallado.

## Nombre

**Fitz** por el Fitz Roy — la montaña más icónica de la Patagonia, en El Chaltén, Argentina.  
Un nombre que no se olvida.

## Autor

Desarrollado en El Chaltén, Santa Cruz, Argentina 🇦🇷  
Por un developer independiente que quería un lenguaje que no tuviera que disculparse por nada.

## Licencia

MIT
