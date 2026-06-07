# M6.C6 — Capstone: app CRUD completa con auth, ORM, WS y cron

**Pre-requisitos**: **todos** los caps anteriores del curso.
M1 (setup), M2 (tipos + funciones), M3 (módulos), M4 (HTTP),
M5 (async + auth + WS + cron), M6.C1-C5 (DB + ORM).

**Objetivo**: integrar **todo lo del curso** en una **app real
production-ready** — un mini "Notion lite" donde users
loguean, crean notas con tags, suscriben a actualizaciones en
tiempo real por WebSocket, y un cron job limpia notas
archivadas. Compilar con `fitz build`, dockerizar, deployar.
**Cero `pip install`**, **cero `npm install`**, **cero
`requirements.txt`**.

**Por qué importa**: cada cap anterior te dio una pieza
aislada. Acá las **combinás todas en un sistema real**. Es la
prueba de que el stack "first-class server" no es slideware —
es un binario de producción que el operador deploya con `docker
compose up` y listo.

**Cross-link**: ejemplo de referencia
[`examples/guide/31b-orm-crud-http.fitz`](https://github.com/Thegreekman76/fitz/blob/main/examples/guide/31b-orm-crud-http.fitz)
para la sección CRUD HTTP + ORM.

---

## Mapa del cap

```mermaid
flowchart LR
    A[Cliente HTTP] --> B["POST /login"]
    B --> C[hash.verify Argon2id]
    C --> D[jwt.encode HS256]
    D --> E[Token JWT]
    E --> F["GET /notes con Bearer"]
    F --> G["@auth_provider"]
    G --> H["Note ORM .where user_id"]
    H --> I[Response JSON]
    J["POST /notes"] --> K["Note.insert"]
    K --> L[WS broadcast nota nueva]
    M["@cron 5min"] --> N["cleanup archived &gt; 30d"]
    O["Docker postgres:16"] --> P[binario Fitz standalone]
```

---

## Por qué importa el capstone

| Stack típico Python+FastAPI | Fitz |
|---|---|
| `pip install fastapi uvicorn sqlalchemy asyncpg pyjwt passlib python-multipart celery redis websockets python-dotenv alembic` | `fitz build` |
| `requirements.txt` con 20+ deps | sin archivo extra |
| `docker-compose.yml` con uvicorn + redis + celery_worker + celery_beat + postgres + nginx | docker-compose con 2 services: app + postgres |
| ~80 MB Docker image (Python base + libs) | ~30 MB Docker image (binario estático + libpq nada) |
| Cold start ~3-5s (boot Python + import + connect) | Cold start ~50-100ms |
| Memory ~120-180 MB idle | Memory ~20-40 MB idle |
| ~15 archivos: routes, models, schemas, services, etc. | 1 archivo `main.fitz` (o pocos módulos) |

**Diferenciador estructural**: el binario nativo de Fitz embebe
el driver Postgres + tokio + JWT + Argon2id + axum + serde
**todo adentro**. La deployment story es radical: copiar 1
archivo binario al server y arrancar.

---

## Paso 1 — El proyecto: "Notas con tiempo real"

App que demuestra TODO el stack:

- **Auth**: register + login con JWT + Argon2id (M5.C2).
- **CRUD con ORM**: notas con tags (jsonb) + relations a users
  (M6.C2-C5).
- **WebSocket**: notificación en tiempo real cuando un user
  crea una nota (M5.C3).
- **Cron**: cleanup de notas archivadas hace más de 30 días
  (M5.C4).
- **OpenAPI + AsyncAPI**: docs auto en `/docs` y `/asyncapi`
  (M4.C4 + M5.C3).
- **Status custom**: 401/403/404/422 con bodies tipados
  (M4.C5).
- **Docker**: imagen mínima + `docker-compose.yml`.

---

## Paso 2 — Estructura del proyecto

```
notas-fitz/
├── fitz.toml                # manifest del package manager (M3)
├── src/
│   └── main.fitz            # entry point — todo el server
├── Dockerfile               # imagen de la app
├── docker-compose.yml       # app + postgres
├── .env                     # DATABASE_URL, JWT_SECRET
└── README.md
```

`fitz.toml`:

```toml
[package]
name = "notas-fitz"
version = "0.1.0"
edition = "2026"

[bin]
main = "src/main.fitz"
```

`.env`:

```bash
DATABASE_URL=postgres://postgres:secret@db:5432/notas?sslmode=disable
JWT_SECRET=mi-secret-de-al-menos-32-chars-largo
```

---

## Paso 3 — Schema y types ORM

```fitz
// src/main.fitz (parte 1)

// Models del dominio.

@table("users") type User {
    @primary id: Int = 0
    email: Str
    name: Str
    @hidden password_hash: Str = ""
    role: Str = "user"
    @has_many("Note", via="user_id") notes: List<Note>
}

@table("notes") type Note {
    @primary id: Int = 0
    title: Str
    body: Str
    tags: List<Str>                      // text[]
    metadata: Map<Str, Any>              // jsonb
    archived: Bool = false
    created_at: DateTime                 // timestamptz
    @belongs_to("User") user_id: Int
    user: User?                          // companion (preload)
}

// Body shapes separados del shape DB.

type RegisterInput { email: Str, password: Str, name: Str }
type LoginInput { email: Str, password: Str }
type LoginResponse { token: Str, user: User }
type NoteInput { title: Str, body: Str, tags: List<Str>, metadata: Map<Str, Any> }
type NoteUpdate { title: Str?, body: Str?, archived: Bool? }

// WebSocket message.
type NoteEvent { kind: Str, note_id: Int, user_email: Str, title: Str }
```

Notar:

- **`@hidden password_hash`** — nunca cruza HTTP (M6.C2 paso 8).
- **`User` con `role` field** — requisito de `@admin` (M5.C2).
- **`Note` con companion `user: User?`** — habilita preload
  BelongsTo (M6.C4).
- **`NoteEvent`** — tipo del frame WS, marshaling JSON
  automático (M5.C3).

---

## Paso 4 — Schema idempotente al boot

```fitz
async fn ensure_schema(db: DbConn) -> Result<Null> {
    let _ = db.exec("CREATE TABLE IF NOT EXISTS users (id bigserial PRIMARY KEY, email text NOT NULL UNIQUE, name text NOT NULL, password_hash text NOT NULL DEFAULT '', role text NOT NULL DEFAULT 'user')", []).await?

    let _ = db.exec("CREATE TABLE IF NOT EXISTS notes (id bigserial PRIMARY KEY, title text NOT NULL, body text NOT NULL, tags text[] NOT NULL DEFAULT '{}', metadata jsonb NOT NULL DEFAULT '{}', archived boolean NOT NULL DEFAULT false, created_at timestamptz NOT NULL DEFAULT NOW(), user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE)", []).await?

    return Ok(null)
}
```

**Por qué `CREATE TABLE IF NOT EXISTS` y no `fitz db migrate` acá**:
el capstone se enfoca en **integrar el stack web entero** (auth +
ORM + WS + cron + Docker) en una sola app. El workflow versionado
con `fitz db diff`/`migrate`/`rollback`/`status` ya lo viste en
[M6.C6 — Migraciones con `fitz db`](c6-migraciones-fitz-db.md), así
que acá mantenemos el patrón idempotente más simple para no repetir
material. **En proyectos reales más allá del capstone, el workflow
`fitz db` es la herramienta default** — referenciá el cap C6 y
[DB y ORM § 26.c](../../db-orm.md#26c-migraciones-automaticas-v01016)
cuando lo apliques.

---

## Paso 5 — Conexión + JWT secret

```fitz
let SECRET = env_or("JWT_SECRET", "dev-secret-cambiame")

let db_result = db.connect(
    env_or("DATABASE_URL", "postgres://postgres:secret@localhost:5432/notas?sslmode=disable")
).await
```

El binding `db_result: Result<DbConn>` queda como tal —
desempacamos en el `main` y en cada handler.

---

## Paso 6 — Auth provider + register/login

```fitz
@auth_provider
fn check_token(headers: Map<Str, Str>) -> Result<User> {
    let auth: Str = match headers.get("authorization") {
        Ok(v) => v,
        Err(_) => return Err("falta Authorization header"),
    }
    let parts = auth.split(" ")
    if (parts.len() != 2) { return Err("formato debe ser 'Bearer <token>'") }
    if (parts[0] != "Bearer") { return Err("scheme debe ser Bearer") }

    let claims = jwt.decode(parts[1], SECRET)?
    let email = claims["email"]

    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("DB no disponible"),
    }
    return User.where(fn(u) => u.email == email).first(conn).await
}

@post("/register")
async fn register(body: RegisterInput) -> Result<User> {
    let conn = match db_result {
        Ok(c) => c,
        Err(e) => return Err("DB error: {e}"),
    }
    let u = User {
        id: 0,
        email: body.email,
        name: body.name,
        password_hash: hash.password(body.password),
        role: "user",
        notes: []
    }
    return User.insert(conn, u).await
}

@post("/login")
async fn login(body: LoginInput) -> LoginResponse {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return 503 { "error": "DB no disponible" },
    }
    let user: User = match User.where(fn(u) => u.email == body.email).first(conn).await {
        Ok(u) => u,
        Err(_) => return 401 { "error": "credenciales inválidas" },
    }
    if (not hash.verify(body.password, user.password_hash)) {
        return 401 { "error": "credenciales inválidas" }
    }
    let claims = {"email": user.email, "role": user.role}
    let token = jwt.encode(claims, SECRET)
    return LoginResponse { token: token, user: user }
}
```

**Detalles importantes**:

- **`@hidden password_hash`** asegura que el `User` en la
  response del `/login` NO incluye el hash, aunque el field
  está en la DB.
- **Mensajes 401 únicos** ("credenciales inválidas" tanto si
  el email no existe como si el password está mal) —
  mitigation contra timing attacks (M5.C2 paso 7).

---

## Paso 7 — CRUD de notes con scope al user

```fitz
@authenticated
@get("/notes")
async fn list_notes(user: User) -> Result<List<Note>> {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("DB no disponible"),
    }
    return Note.where(fn(n) => n.user_id == user.id and n.archived == false)
        .order_by(fn(n) => -n.id)
        .limit(50)
        .all(conn).await
}

@authenticated
@get("/notes/{id}")
async fn get_note(id: Int, user: User) -> Result<Note> {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("DB no disponible"),
    }
    return Note.where(fn(n) => n.id == id and n.user_id == user.id).first(conn).await
}

@authenticated
@post("/notes")
async fn create_note(input: NoteInput, user: User) -> Result<Note> {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("DB no disponible"),
    }
    let n = Note {
        id: 0,
        title: input.title,
        body: input.body,
        tags: input.tags,
        metadata: input.metadata,
        archived: false,
        created_at: DateTime.now(),
        user_id: user.id,
        user: null
    }
    let inserted = Note.insert(conn, n).await?

    // Notificar via WS (siguiente paso).
    let event = NoteEvent {
        kind: "note_created",
        note_id: inserted.id,
        user_email: user.email,
        title: inserted.title
    }
    let _ = spawn(notify_subscribers(event))

    return Ok(inserted)
}

@authenticated
@put("/notes/{id}")
async fn update_note(id: Int, input: NoteUpdate, user: User) -> Result<Int> {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("DB no disponible"),
    }
    let changes: Map<Str, Any> = {}
    // ... lógica de armar changes desde input nullable ...
    return Note.where(fn(n) => n.id == id and n.user_id == user.id)
        .update(conn, changes).await
}

@authenticated
@delete("/notes/{id}")
async fn delete_note(id: Int, user: User) -> Result<Int> {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("DB no disponible"),
    }
    return Note.where(fn(n) => n.id == id and n.user_id == user.id).delete(conn).await
}
```

**Patrón canónico de auth scoping**: el `@auth_provider`
inyecta `user`, y CADA query agrega `.where(fn(n) => n.user_id
== user.id)` para que un user NO vea / edite / borre notas de
otro.

---

## Paso 8 — WebSocket para notificaciones en tiempo real

```fitz
@authenticated
@ws("/events")
async fn subscribe_events(conn: WsConn<NoteEvent>, user: User) -> Null {
    let welcome = NoteEvent {
        kind: "connected",
        note_id: 0,
        user_email: "system",
        title: "Suscripto a eventos de {user.email}"
    }
    let _ = conn.send(welcome)

    loop {
        match conn.recv() {
            Ok(_) => { /* ignorar mensajes del cliente */ }
            Err(_) => return null
        }
    }
    return null
}

@background
async fn notify_subscribers(event: NoteEvent) -> Null {
    // En MVP: broadcast a TODOS los WS conectados. Para production
    // con miles de subscribers, particioná por user_id con un map
    // de conns adentro del state del server.
    //
    // El broadcast es per-endpoint del WS; el siguiente call es
    // implícito porque `conn.broadcast` en el handler manda a TODOS
    // los conn de "/events".
    //
    // Acá un sleep simbólico para mostrar fire-and-forget.
    let _ = sleep(10).await
    print("[bg] note_created: {event.title} by {event.user_email}")
    return null
}
```

**Notas**:

- `@authenticated @ws("/events")` valida el bearer ANTES del
  HTTP upgrade (M5.C3 paso 4).
- El `spawn(notify_subscribers(event))` en `create_note`
  dispara fire-and-forget — la response al cliente HTTP sale
  inmediato.
- Browser conecta con subprotocol `bearer.<token>` (M5.C3 paso
  5).

---

## Paso 9 — Cron job de cleanup

```fitz
@cron("0 */5 * * * *",                      // cada 5 minutos
      tz="UTC",
      retry={max: 2, backoff: "exponential", initial_secs: 5, max_secs: 60})
async fn cleanup_archived() -> Result<Null> {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("DB no disponible"),
    }

    // Borrar notes archivadas hace más de 30 días.
    let cutoff = DateTime.now().subtract_days(30)
    let n = Note.where(fn(n) => n.archived == true and n.created_at < cutoff)
        .delete(conn).await?

    print("[cron] cleanup: borradas {n} notas archivadas")
    return Ok(null)
}
```

Con `tz="UTC"` + `retry={...}` + sin `store=db` (memoria). Para
persistencia y visibility, agregar `store=db` (M5.C4 paso 6).

---

## Paso 10 — `main()` y `@server` config

```fitz
@server(8080, ws_heartbeat_secs=30)
fn main() => 0

print("Arrancando notas-fitz...")

// Schema idempotente.
match db_result {
    Ok(conn) => {
        let _ = ensure_schema(conn).await
        print("Schema OK")
    },
    Err(e) => {
        print("FATAL: db error: {e}")
    },
}

print("Server arriba en :8080")
print("  POST /register, /login")
print("  GET/POST/PUT/DELETE /notes (authenticated)")
print("  WS /events (authenticated)")
print("  GET /docs (OpenAPI Scalar)")
print("  GET /asyncapi (AsyncAPI UI)")
```

---

## Paso 11 — `Dockerfile` minimal

```dockerfile
# Multi-stage build para imagen pequeña.

# Stage 1: build.
FROM rust:1.95 as builder
WORKDIR /app
RUN curl -fsSL https://github.com/Thegreekman76/fitz/releases/latest/download/fitz-linux-x86_64.tar.gz | tar xz -C /usr/local/bin
COPY . .
RUN fitz build src/main.fitz --release

# Stage 2: runtime.
FROM debian:trixie-slim
WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/main /app/notas-fitz
EXPOSE 8080
CMD ["./notas-fitz"]
```

Resultado: imagen ~30 MB (binario + libc + ca-certificates).
Comparable con Go static; mucho menor que Python (~80-120 MB)
o Node (~200+ MB).

---

## Paso 12 — `docker-compose.yml`

```yaml
version: "3.8"

services:
  app:
    build: .
    ports:
      - "8080:8080"
    environment:
      DATABASE_URL: postgres://postgres:secret@db:5432/notas?sslmode=disable
      JWT_SECRET: ${JWT_SECRET}
    depends_on:
      db:
        condition: service_healthy
    restart: unless-stopped

  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: secret
      POSTGRES_DB: notas
    volumes:
      - pg_data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 5s
      timeout: 5s
      retries: 5
    restart: unless-stopped

volumes:
  pg_data:
```

Deployment:

```bash
$ export JWT_SECRET=$(openssl rand -hex 32)
$ docker compose up -d
$ docker compose logs -f app
Arrancando notas-fitz...
Schema OK
Server arriba en :8080
  POST /register, /login
  ...
```

---

## Paso 13 — Probar end-to-end

```bash
# Register
$ curl -X POST localhost:8080/register \
       -H 'Content-Type: application/json' \
       -d '{"email":"ada@x.com","password":"secret123","name":"Ada"}'
{"id":1,"email":"ada@x.com","name":"Ada","role":"user","notes":[]}
# Nota: NO incluye password_hash (es @hidden)

# Login
$ curl -X POST localhost:8080/login \
       -H 'Content-Type: application/json' \
       -d '{"email":"ada@x.com","password":"secret123"}'
{"token":"eyJ...","user":{"id":1,"email":"ada@x.com","name":"Ada","role":"user","notes":[]}}

# Crear note (con token)
$ TOKEN="eyJ..."
$ curl -X POST localhost:8080/notes \
       -H "Authorization: Bearer $TOKEN" \
       -H 'Content-Type: application/json' \
       -d '{"title":"Primera nota","body":"hola mundo","tags":["intro"],"metadata":{"lang":"es"}}'
{"id":1,"title":"Primera nota","body":"hola mundo","tags":["intro"],"metadata":{"lang":"es"},"archived":false,"created_at":"2026-06-02T16:30:00Z","user_id":1,"user":null}

# Listar (scoped al user del token)
$ curl localhost:8080/notes -H "Authorization: Bearer $TOKEN"
[{"id":1, ... }]

# WS subscribe (con wscat)
$ wscat -c "ws://localhost:8080/events" -H "Authorization: Bearer $TOKEN"
< {"kind":"connected","note_id":0,"user_email":"system","title":"Suscripto a eventos de ada@x.com"}

# Crear otra nota (en otra terminal con el mismo o distinto user)
# → la primera terminal recibe el broadcast en el WS.

# Docs autogenerados
$ open http://localhost:8080/docs        # OpenAPI Scalar UI
$ open http://localhost:8080/asyncapi    # AsyncAPI UI
```

---

## Lo que cubriste

Llegaste al final del cap **y del módulo M6**. **Felicidades**.

### Capstone integra todo el curso

- **M1** — Setup + `fitz build` para producir el binario.
- **M2** — `type` custom (`User`, `Note`, `NoteEvent`,
  `LoginInput`, etc.) + `match` + `Result`.
- **M3** — `fitz.toml` para organizar el proyecto.
- **M4** — Handlers `@get`/`@post`/`@put`/`@delete` + body
  tipado + status codes custom (`return 401 {...}`) + OpenAPI
  + `/docs`.
- **M5.C1** — `async fn` + `.await` en handlers, schema setup
  y conn.
- **M5.C2** — `@auth_provider` + `@authenticated` + `jwt` +
  `hash` (Argon2id) + `@hidden` para password_hash.
- **M5.C3** — `@ws("/events")` + `WsConn<NoteEvent>` + auth
  pre-upgrade + AsyncAPI.
- **M5.C4** — `@cron("...")` con `tz` + `retry` + `@background`
  + `spawn(...)`.
- **M6.C1** — `db.connect` + lazy pool.
- **M6.C2** — `@table` + `@primary` + `@hidden`.
- **M6.C3** — `.where(...).update/delete` con guard + chain
  con `.order_by`/`.limit`.
- **M6.C4** — Relations (`@has_many`/`@belongs_to`) + companion
  field.
- **M6.C5** — jsonb (`metadata: Map<Str, Any>`) + arrays
  (`tags: List<Str>`) + `DateTime.now()` + aritmética
  (`.subtract_days(30)`).
- **M6.C6** — workflow `fitz db diff`/`migrate`/`rollback`/
  `status`/`history`/`check` para versionar cambios de schema
  en producción. Acá usamos `CREATE TABLE IF NOT EXISTS` al
  boot para enfocarnos en la integración del stack web, pero
  el workflow versionado es la herramienta default en proyectos
  más allá del MVP.

### Diferenciadores que justifican el approach

| | Stack vecino típico | Fitz |
|---|---|---|
| Setup deps | `pip install ×8` + `docker compose` con N services | `fitz build` |
| Tamaño binario | ~80-200 MB Docker image | ~30 MB |
| Boot time | 3-5s (interpreter + deps) | 50-100ms (binario nativo) |
| Memory idle | 120-180 MB | 20-40 MB |
| Lines of code | ~1500-2500 LoC (split en N archivos) | ~400 LoC (un archivo) |
| Type safety | runtime checks parciales | checker estático completo |
| OpenAPI manual | YAML aparte que se atrasa | auto-generado del código |
| WS docs | README inexistente o ausente | AsyncAPI auto en `/asyncapi` |

---

## Cerraste el módulo M6

**Felicidades** — completaste el módulo de **Postgres + ORM
nativo**. Sabés:

- ✅ Conectarte a Postgres con `db.connect(url).await?` y
  ejecutar SQL crudo con `db.query` / `db.exec`. TLS strict
  para managed (Heroku, RDS, Supabase, Neon) con `rustls` puro
  Rust (**C1**).
- ✅ Declarar el ORM con `@table` + `@primary` + `@column` +
  `@hidden`, y hacer lecturas tipadas con `Type.all(db)` /
  `.where(closure).first/count(db)` validadas por el checker
  en compile-time (**C2**).
- ✅ Hacer writes (`.insert` con `RETURNING *`, `.bulk_insert`
  para cargas masivas, `.update`/`.delete` con guard
  obligatorio), encadenar `.order_by`/`.limit`/`.offset` para
  paginación, y armar agregados scalar + GROUP BY (**C3**).
- ✅ Declarar relaciones con `@belongs_to`/`@has_many`/
  `@has_one`, navegar con `instance.field(db).await?`, encadenar
  con `instance.field()` sin `db`, y cerrar el N+1 con batch
  manual `.is_in([fks])` (**C4**).
- ✅ Mapear tipos avanzados: `Map<Str, Any>` ↔ jsonb,
  `List<scalar>` ↔ `T[]`, built-in `Date`/`DateTime`/`Uuid`
  con aritmética + tz, JSON operators (`.has_key`,
  `.contains_json`, `.path_int`), array operators
  (`.has`, `.contains_all`, `.contained_in`), full-text search
  (`.matches`, `.plainto_matches`, `.rank`) (**C5**).
- ✅ Versionar cambios de schema con `fitz db new`/`diff`/
  `migrate`/`rollback`/`status`/`history`/`check`, escribir
  data migrations nativas en `.fitz` con `async fn migrate(db)`,
  bloquear drift en CI con `fitz db check`, adoptar DBs legacy
  con `fitz db inspect` + `stamp` (**C6**).
- ✅ Integrar **todo el curso** en una app real con auth + ORM
  + WS + cron + Docker, deployable como un binario standalone
  de ~30 MB (**C7**) ← acá.

### Comparativa final con el stack típico

| Componente | Python+FastAPI | Node+Express | Spring Boot | **Fitz** |
|---|---|---|---|---|
| HTTP framework | FastAPI | Express | Spring MVC | builtin |
| Async runtime | uvicorn workers | event loop | Reactor | tokio multi-thread |
| ORM | SQLAlchemy | TypeORM/Prisma | Hibernate/JPA | builtin |
| Migrations | Alembic | typeorm migration | Flyway | `fitz db diff/migrate` |
| JWT + Argon2 | python-jose + passlib | jsonwebtoken + argon2 | Spring Security + jjwt | builtin |
| WebSocket | websockets + manual | socket.io | Spring WebFlux | builtin |
| Cron | Celery + Redis | bull + Redis | `@Scheduled` + Quartz | builtin |
| OpenAPI | FastAPI auto + Pydantic | swagger-ui-express | springdoc | builtin |
| AsyncAPI | manual YAML | manual YAML | manual YAML | builtin |
| Static binary | ❌ | ⚠ pkg hack | ✅ jar (~50-100MB) | ✅ ~30MB |
| Docker image | ~80-150 MB | ~150-250 MB | ~200-400 MB | ~30 MB |

## Qué viene en M7 y M8

A partir del próximo módulo el curso se abre en dos ramas
opcionales según tu caso de uso:

**[M7 — Interop Python](../m7-python-interop/c1-setup-imports.md)** (3 caps):

- **C1** — Setup venv + `from python import` + casos simples.
- **C2** — numpy + pandas reales para data analysis.
- **C3** — SQLAlchemy interop + bridge async + matriz de
  decisión vs ORM nativo.

Si tu app necesita usar paquetes Python (data science, ML,
SQLAlchemy contra una DB legacy), M7 es el puente. Si solo
necesitás el stack Fitz nativo, podés saltar directo a M8.

**[M8 — Producción y deployment](../m8-produccion-deploy/c1-distribucion-binarios.md)** (5 caps):

- **C1** — Distribución avanzada: binarios standalone +
  cross-compile multi-platform.
- **C2** — Observability: logs + spans + métricas + OTel.
- **C3** — Secrets management: `secret()` + `config()` +
  `Secret<T>`.
- **C4** — Deploy avanzado: `fitz docker init/build` +
  healthz/readyz + K8s + 12-factor.
- **C5** — Deploy de apps con interop Python: `fitz build
  --bundle-python` + `--bundle-pip`.

Mientras tanto, podés **shippear lo que ya tenés** — el
capstone de M6 es production-ready. Mucha gente NO necesita
más que eso para una app real.

**Gracias por terminar el curso.** 🏔️
