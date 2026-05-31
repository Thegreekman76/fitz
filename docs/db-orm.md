# DB y ORM — guía exhaustiva

Esta es la guía dedicada al stack DB nativo de Fitz: driver Postgres
puro + ORM declarativo + paridad bit-a-bit `fitz run` ↔ `fitz build`.
A diferencia del [cap 31 de la guía](guide.md#31-postgres--orm-nativo)
que sirve como resumen del lenguaje, este documento es la referencia
completa que cubre cada pieza, cada operador, cada receta y cada
limitación honesta.

**Hito del proyecto (v0.10.0 + v0.10.1 + v0.10.2)** — Fase 10 entera
+ Fase 10.b (paridad bit-a-bit codegen) + cap 31 (documentación)
cierran el bloque "stack web first-class del lado server".

## Índice

- [1. Panorama vecino y diferenciales](#1-panorama-vecino-y-diferenciales)
- [2. Quickstart](#2-quickstart)
- [3. Driver `db`: query/exec crudo](#3-driver-db-queryexec-crudo)
- [4. `@table`, `@primary`, `@column`: declarar el mapping](#4-table-primary-column-declarar-el-mapping)
- [5. Read methods: `.all`, `.first`, `.count`, `.where`](#5-read-methods-all-first-count-where)
- [6. QueryBuilder reference: chain y terminales](#6-querybuilder-reference-chain-y-terminales)
- [7. Operadores extendidos en `.where(...)`](#7-operadores-extendidos-en-where)
- [8. Write methods: `.insert`, `.update`, `.delete`](#8-write-methods-insert-update-delete)
- [9. Aggregates scalar + GROUP BY](#9-aggregates-scalar--group-by)
- [10. Relations: `@belongs_to`, `@has_one`, `@has_many`](#10-relations-belongs_to-has_one-has_many)
- [11. Navigation methods + chain](#11-navigation-methods--chain)
- [12. Eager loading con `.preload(...)`](#12-eager-loading-con-preload)
- [13. JSONB: `Map<Str, Any>` ↔ `jsonb`](#13-jsonb-mapstr-any--jsonb)
- [14. Arrays Postgres: `List<scalar>` ↔ `T[]`](#14-arrays-postgres-listscalar--t)
- [15. NULL en arrays: `List<scalar?>`](#15-null-en-arrays-listscalar)
- [16. `Map<Str, T>` concreto homogéneo](#16-mapstr-t-concreto-homog%C3%A9neo)
- [17. Array ops en `.where(...)`](#17-array-ops-en-where)
- [18. Date / Time / Timestamp / UUID](#18-date--time--timestamp--uuid)
- [19. Recetas — paginación](#19-recetas--paginaci%C3%B3n)
- [20. Recetas — búsqueda](#20-recetas--b%C3%BAsqueda)
- [21. Recetas — search filters combinatorios](#21-recetas--search-filters-combinatorios)
- [22. Recetas — Auth + ORM (queries scoped al user autenticado)](#22-recetas--auth--orm-queries-scoped-al-user-autenticado)
- [23. Recetas — HTTP CRUD completo](#23-recetas--http-crud-completo)
- [24. Recetas — Cron job de limpieza](#24-recetas--cron-job-de-limpieza)
- [25. Recetas — Bulk operations](#25-recetas--bulk-operations)
- [26. Recetas — Schema idempotente al boot](#26-recetas--schema-idempotente-al-boot)
- [26.b. Transactions (v0.10.14)](#26b-transactions-v01014)
- [26.c. Migraciones automáticas (v0.10.16)](#26c-migraciones-autom%C3%A1ticas-v01016)
- [27. Performance](#27-performance)
- [28. Limitaciones honestas y deuda explícita](#28-limitaciones-honestas-y-deuda-expl%C3%ADcita)
- [29. CLI con DB: cómo cada subcomando interactúa](#29-cli-con-db-c%C3%B3mo-cada-subcomando-interact%C3%BAa)
- [30. Ejemplos runnable y boilerplates](#30-ejemplos-runnable-y-boilerplates)

---

## 1. Panorama vecino y diferenciales

En SQLAlchemy/Django ORM/ActiveRecord/Hibernate/Prisma/Diesel la
combinación DB driver + ORM se construye sumando librerías
opcionales al proyecto:

| Stack vecino | Driver | ORM | Cómo se acopla |
|--------------|--------|-----|----------------|
| **Python + FastAPI** | `psycopg2` o `asyncpg` | SQLAlchemy 2.x async o Tortoise | `pip install` ambos. ORM resuelve mapping en runtime con metaclass + reflection. |
| **Python + Django** | `psycopg2` | Django ORM | Tightly coupled con el framework. Migrations via comando aparte. |
| **Ruby + Rails** | `pg` gem | ActiveRecord | Bundler. ActiveRecord es parte del framework. |
| **Java + Spring** | JDBC driver | Hibernate / JPA | Maven. Anotaciones `@Entity`/`@OneToMany` resueltas en runtime con reflection AOP. |
| **Node + Express** | `pg` | Prisma o TypeORM o Sequelize | `npm install`. Prisma genera schema separado (`prisma generate`), TypeORM usa decoradores TS resueltos en runtime. |
| **Rust + Axum** | `tokio-postgres` o `sqlx` | Diesel | Cargo. Diesel pide derives + queries macros (`table!` macro genera código en compile-time). |
| **Go + Gin** | `pgx` | `gorm` o `sqlc` | `go.mod`. `gorm` usa struct tags + reflection runtime. `sqlc` genera código desde queries SQL. |

Cuesta lo siguiente: 3-5 dependencias mínimo por proyecto. Decoradores
"mágicos" que se resuelven en runtime con reflection (Spring AOP /
JPA). Generación de schema separada (Prisma exige `prisma generate`
antes de cada build). Tipado opcional que no respeta el shape real
de la tabla. Y a la hora de compilar a binario nativo: imposible
para Python/Ruby/PHP, parche para Node (con `pkg`/`nexe`/`bun build`
y limitaciones), funciona en Go (`pgx + gorm`) pero arrastra un ORM
separado del compilador, funciona en Rust (Diesel/sqlx) pero pide
macros derive + crates externas.

**En Fitz el DB driver y el ORM son parte del lenguaje**. El módulo
`db` viene con un driver Postgres puro escrito en Fitz/Rust (~2400
LoC en `src/db.rs`, sin link a libpq, sin `tokio-postgres`/`sqlx`/
`diesel`) que habla wire protocol v3.0 + SCRAM-SHA-256 + parser de
los 11 tipos OID core. Encima del driver, 6 decoradores nativos
(`@table`, `@primary`, `@column`, `@belongs_to`, `@has_many`,
`@has_one`) — con kwargs `on_delete`/`on_update`/`fk`/`via` —
declaran el mapping `type` Fitz ↔ tabla Postgres. El type checker valida estáticamente que
`@primary` exista, que `@belongs_to` apunte a un type existente,
que los métodos del `QueryBuilder<Row>` preserven el tipo del row
a lo largo de toda la chain. Y el codegen produce un binario nativo
que ejecuta queries SQL **constantes en compile-time** (cada
`.where(fn(u) => u.age > 18)` se traduce al fragmento `"age" > $1`
DURANTE EL CODEGEN, zero overhead runtime para construir SQL).

### Los 6 diferenciales únicos

1. **DB nativa, no librería**. El driver Postgres + el ORM viven en
   el binario `fitz`. Cero `pip install psycopg2`, cero `gem
   install pg`, cero `cargo add tokio-postgres`, cero `npm install
   pg`. Cuando hacés `fitz build` el binario nativo embebe el
   driver — un `.exe`/ELF/Mach-O standalone que habla wire protocol
   v3.0 + SCRAM-SHA-256 sin link a libpq.
2. **SQL constante en codegen-time**. Cada `.where(closure)` se
   walka del AST DURANTE EL CODEGEN, el fragmento SQL queda
   hard-coded en el binario. Zero overhead runtime para construir
   SQL. Comparable a Diesel/sqlx, **mejor que SQLAlchemy/
   ActiveRecord/Hibernate** que construyen SQL via objetos en
   runtime cada vez.
3. **Paridad bit-a-bit `fitz run` ↔ `fitz build`**. Lo que ves
   funcionar en el intérprete (rapid feedback) funciona idéntico
   en el binario nativo (deploy a prod). Cero "anda en local pero
   no en server". 16 tests E2E de paridad codegen + 27 evaluator
   E2E corren contra `postgres:16` en cada push a `main` via job
   `db-postgres` con service container.
4. **Decorators del lenguaje, no anotaciones**. `@table`/`@primary`/
   `@column`/`@belongs_to`/`@has_many`/`@has_one` son **parte del
   compilador** (lexer + parser + type checker + codegen), no
   anotaciones procesadas por una lib opcional. El checker exige
   `@primary` único, valida que `@belongs_to("X")` apunte a un type
   existente, infiere los signatures de los navigation methods.
   Spring `@Entity`/JPA + Hibernate resuelven esto en runtime con
   reflection — Fitz lo hace en compile-time.
5. **Eager loading con dispatch estático**. `.preload("posts")` con
   el relation name como Str literal en compile-time produce un
   `match` exhaustivo emitido por el codegen. Typos (`.preload(
   "post")` sin la "s" final) detectados en compile-time, no
   runtime. Comparable a Diesel's `belonging_to` macros, mejor que
   SQLAlchemy `joinedload(User.posts)` donde el typo recién
   aparece como `AttributeError` al evaluar.
6. **Integrado con el resto del lenguaje**. Tipos custom +
   `Result<T>` + `?` + `match` + decoradores apilables
   (`@authenticated` + `@get` + handler que llama `Type.where(...)
   .all(db).await?`) + middleware/CORS + body deserialization +
   WebSockets + cron jobs. El ORM no es una "isla" con sus propias
   reglas, encaja exactamente con HTTP nativo + auth + jobs +
   WebSockets.

**Ningún otro lenguaje moderno** combina los 6 puntos en el binario
base sin macros derive ni introspection runtime.

---

## 2. Quickstart

Un programa Fitz minimal que se conecta a Postgres, declara una
tabla, inserta un row, y lo trae de vuelta:

```fitz
@table("users") type User {
    @primary id: Int = 0
    name: Str
    age: Int
}

async fn main() -> Result<Str> {
    let db = db.connect("postgres://postgres:postgres@localhost/demo?sslmode=disable").await?

    // Crear la tabla si no existe (idempotente al boot).
    db.exec("CREATE TABLE IF NOT EXISTS users (
        id bigserial PRIMARY KEY,
        name text NOT NULL,
        age bigint NOT NULL
    )", []).await?

    // Insert: bigserial auto-asigna el id.
    let inserted = User.insert(db, User { id: 0, name: "ada", age: 35 }).await?
    print("nueva user con id = {inserted.id}")

    // SELECT con WHERE.
    let found = User.where(fn(u) => u.id == inserted.id).first(db).await?
    print("encontrada: {found.name} ({found.age})")

    // SELECT all.
    let all = User.all(db).await?
    print("total: {len(all)}")

    return Ok("OK")
}

print(main().await)
```

Salida esperada (con Postgres real corriendo):

```
nueva user con id = 1
encontrada: ada (35)
total: 1
OK
```

Tres piezas:

- `db.connect(url)` devuelve `DbConn` (`Future<Result<DbConn>>`).
- `@table("users")` sobre un `type` con `@primary` lo habilita
  para el ORM.
- `User.insert/all/where/first` son métodos estáticos sobre el
  type (no sobre instancias).

Todo lo demás del documento expande estos tres.

---

## 3. Driver `db`: query/exec crudo

El módulo built-in `db` (siempre disponible, sin import) tiene
cuatro funciones core:

### `db.connect(url) -> Future<Result<DbConn>>`

Establece conexión + abre un pool de conexiones internamente. Lazy:
las conexiones reales se levantan on-demand cuando llega el primer
query.

```fitz
let db = db.connect("postgres://user:pass@host:5432/dbname?sslmode=disable").await?
```

**URL formato estándar Postgres**:

```
postgres://[user[:password]@]host[:port]/dbname[?param=value&...]
```

Parámetros soportados:

- `sslmode=disable|require|verify-ca|verify-full` — controla TLS.
  Default `disable` si no se especifica. Detalle en
  [sub-sección "TLS strict"](#tls-strict-v01023) abajo.
- `sslrootcert=path/to/ca.pem` — custom CA bundle (PEM) para
  `verify-ca`/`verify-full`. Sin esto, el driver usa el Mozilla
  root CA bundle (`webpki-roots`) in-binary. Path relativo al
  CWD del proceso.
- `application_name=mi-app` — passthrough al server.

### TLS strict (v0.10.23)

Habilitado desde v0.10.23 (Fase 10.1.b) — apuntar el driver a
managed Postgres real (Heroku, RDS, Supabase, Neon, Aiven, Render
PG, Crunchy Bridge, etc.) sin downgrade a sslmode=disable.
Implementado con `rustls` puro Rust + `webpki-roots` (Mozilla CA
bundle in-binary). Cero deps system — no requiere OpenSSL/
SChannel/SecTransport instalado.

| `sslmode` | TLS | Cert chain | Hostname | Cuándo usar |
|---|:---:|:---:|:---:|---|
| `disable`     | ❌ | — | — | Local dev sin TLS (Postgres en Docker localhost, etc.) |
| `require`     | ✅ | ❌ | ❌ | Dev/staging contra Postgres internos sin CA pública. NO usar en prod (MITM) |
| `verify-ca`   | ✅ | ✅ | ❌ | Cert custom donde el CN/SAN difiere del hostname (proxies, port forward) |
| `verify-full` | ✅ | ✅ | ✅ | **Recomendado producción** — modo usado por todos los managed Postgres |

```fitz
// Supabase / Neon / RDS — exigen TLS verify-full
let db = db.connect(
    "postgres://user:pass@db.proyecto.supabase.co:5432/postgres?sslmode=verify-full"
).await?

// Postgres interno con cert self-signed firmado por una CA corporativa
let db = db.connect(
    "postgres://user:pass@db.intra:5432/myapp?sslmode=verify-full&sslrootcert=/etc/ssl/corp-ca.pem"
).await?
```

**Combinaciones inválidas** abortan en el parser con mensaje claro
(no esperan a runtime):
- `sslmode=disable&sslrootcert=...` (contradictorio — el cert no se usa).
- `sslmode=require&sslrootcert=...` (inconsistente — require no verifica).
- `sslrootcert=...` sin `sslmode=` (cert sin uso definido).

**Out of scope MVP** (deuda visible, sin presión):
- `sslmode=prefer`/`allow` (negociación dinámica con downgrade). El
  flow correcto requiere reconectar si el server rechaza TLS — los
  drivers Postgres modernos lo desalientan (vulnerable a MITM
  stripping). Usá `disable` o `require`/`verify-full` explícito.
- **Client cert auth** (`sslcert=`/`sslkey=`/`sslpassword=`).
  Patrón enterprise raramente usado vs TLS server-only.

### `db.query(sql, params) -> Future<Result<List<DbRow>>>`

Query crudo. Devuelve cada row como un `DbRow` opaco con las
columnas named (parseo lazy + métodos tipados de extracción).
Los parámetros van como `$1`, `$2`, etc. (positional, NO named).

```fitz
let rows = db.query("SELECT id, email FROM users WHERE active = $1 AND age > $2",
    [true, 18]).await?
// rows: List<DbRow>

let r: DbRow = rows[0]
let id: Int    = r.get_int("id")?           // Result<Int>
let email: Str = r.get_str("email")?        // Result<Str>
```

**Métodos sobre `DbRow`** (desde v0.10.22, paridad bit-a-bit
intérprete↔codegen):

| Método | Retorno | Notas |
|---|---|---|
| `r.get_int(col)`   | `Result<Int>`   | Falla si la col es NULL, no existe, o el tipo PG no es int |
| `r.get_str(col)`   | `Result<Str>`   | Falla si NULL/no existe; acepta text/varchar/uuid/json/etc. |
| `r.get_float(col)` | `Result<Float>` | float8/float4/numeric/etc. |
| `r.get_bool(col)`  | `Result<Bool>`  | bool PG |
| `r.len()`          | `Int`           | número de columnas del row |

Para handlers HTTP que retornan rows crudos sin extracción, podés
devolver `Result<List<DbRow>>` directo — el codegen auto-serializa
cada row a `{col: val, ...}` en el JSON response (ver Boilerplate
`api-multi-tenant` Enfoque B para el patrón canónico).

### `db.exec(sql, params) -> Future<Result<Int>>`

Para statements que no retornan rows (INSERT/UPDATE/DELETE sin
RETURNING, DDL, etc.). Devuelve el número de rows afectadas.

```fitz
let affected = db.exec(
    "UPDATE users SET last_seen = NOW() WHERE id = $1",
    [42]
).await?
print("rows afectadas: {affected}")
```

### `db.close() -> Future<Result<Null>>`

Cierra el pool. Idempotente (llamar 2 veces no es error). Queries
posteriores fallan con error claro.

```fitz
db.close().await?
```

### `db.is_closed() -> Bool`

Sync. Devuelve `true` si la conexión fue cerrada via `.close()`.
Útil para checks defensivos antes de armar una query:

```fitz
if db.is_closed() {
    return Err("db cerrada — reconectar antes de continuar")
}
```

### Tipos pasados como parámetros

Los `params` (segundo arg de `query`/`exec`) es `List<Any>` con
auto-coerción a tipos Postgres según el value Fitz:

| Tipo Fitz       | Tipo Postgres |
|-----------------|---------------|
| `Int`           | `int8` (BIGINT) |
| `Float`         | `float8` (DOUBLE PRECISION) |
| `Str`           | `text` |
| `Bool`          | `bool` |
| `Null`          | `NULL` |
| `List<Int>`     | `int8[]` |
| `List<Str>`     | `text[]` |
| `Map<Str, Any>` | `jsonb` |

Heterogéneos en lista (`List<Any>`) → cada elemento se coerce
individualmente.

---

## 4. `@table`, `@primary`, `@column`: declarar el mapping

Tres decoradores básicos para mapear `type` Fitz a tabla Postgres.

### `@table("nombre_tabla")`

Sobre un `type`. Indica que el ORM debe mapearlo a la tabla
especificada.

```fitz
@table("users") type User { ... }
```

Convención: nombre de tabla en lowercase + plural snake_case
(`users`, `blog_posts`, `order_line_items`). El nombre del `type`
Fitz puede ser cualquier identificador válido (típicamente
PascalCase singular).

**Schemas custom (v0.10.21)**: `@table("schema.name")` mapea a
una tabla en un schema Postgres no-`public`. Útil para
multi-tenant via schemas, separación dev/test, o módulos
aislados.

```fitz
@table("analytics.events") type Event {
    @primary id: Int = 0
    name: Str
    @db_default("NOW()") at: Str = ""
}
```

`fitz db diff` emite `CREATE SCHEMA IF NOT EXISTS "analytics";`
+ `CREATE TABLE "analytics"."events" (...)` automáticamente. El
ORM nativo (SELECT/INSERT/UPDATE/DELETE) usa el qualified name
en TODAS las queries. Sin `.` en el arg, schema=`public`
(comportamiento default — compat con código pre-v0.10.21).

### `@primary`

Sobre un field. **Uno o varios** `@primary` por `type` con
`@table`. Un solo `@primary` Int con default `= 0` → Postgres
`bigserial` auto-asigna. Str UUID → cliente genera.

```fitz
@table("users") type User {
    @primary id: Int = 0    // bigserial PRIMARY KEY (auto)
    email: Str
}

@table("sessions") type Session {
    @primary token: Str     // text PRIMARY KEY (UUID del cliente)
    user_id: Int
}
```

**Composite primary key (v0.10.27)**: N `@primary` arman un PK
tuple. Migrations emite `PRIMARY KEY (col1, col2)` table-level.
Cada columna lleva su tipo individual (sin `bigserial` — los
valores los provee el user explícitos):

```fitz
@table("memberships") type Membership {
    @primary org_id: Int = 0
    @primary user_id: Int = 0
    role: Str = "member"
    joined_at: Str = ""
}

// Insert con valores explícitos del PK tuple
let m = Membership { org_id: 1, user_id: 42, role: "admin" }
let r = Membership.insert(conn, m).await?
```

**Limitaciones composite PK en v0.10.27**:
- Sentinel `id: 0` auto-bigserial NO aplica (es single-PK only).
- Navigation methods (`@belongs_to`, `@has_many`, `@has_one`)
  apuntan al **single PK** del target. BelongsTo a composite PK
  → error de checker explícito.
- `same field declared twice as @primary` → error claro.

### `@column(name="...")`

Sobre un field. Para cuando el nombre del field Fitz difiere del
de la columna en Postgres (camelCase vs snake_case típico):

```fitz
@table("orders") type Order {
    @primary id: Int = 0
    @column(name="customer_id") customer: Int
    @column(name="created_at") created: Str   // ISO 8601
    total_amount: Float       // sin @column: mismo nombre en ambos
}
```

Los kwargs van con `=` (no `:`). Solo `name=` está soportado por
ahora; `sql_type=` (override del tipo Postgres inferido del Fitz)
queda como mini-fase futura.

### Defaults

Los fields pueden tener default literal:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    name: Str
    role: Str = "user"        // default literal
    active: Bool = true
    metadata: Map<Str, Any> = {}
}
```

INSERT desde Fitz: si el field se omite del struct literal, el
default Fitz se usa al construir el value que va a Postgres.

### Mapping de tipos Fitz → Postgres por default

| Tipo Fitz | Postgres column type |
|-----------|----------------------|
| `Int`     | `bigint` (o `bigserial` si es `@primary id: Int = 0`) |
| `Float`   | `double precision` |
| `Str`     | `text` |
| `Bool`    | `boolean` |
| `Str?`    | `text NULL` |
| `Int?`    | `bigint NULL` |
| `List<Int>` | `bigint[]` |
| `List<Str>` | `text[]` |
| `List<Float>` | `double precision[]` |
| `List<Bool>` | `boolean[]` |
| `List<Int?>` | `bigint[]` (con NULL aceptable en elementos) |
| `Map<Str, Any>` | `jsonb` |
| `Map<Str, Int>` (T concreto) | `jsonb` (shape homogéneo) |
| `Date` (v0.10.24) | `date` |
| `DateTime` (v0.10.24) | `timestamptz` (siempre UTC) |
| `Uuid` (v0.10.24) | `uuid` |

`Date`/`DateTime`/`Uuid` son tipos built-in nativos (no `Str` ISO
8601) desde v0.10.24. Tienen constructors estáticos
(`Date.today()`, `DateTime.now()`, `Uuid.v4()`, etc.), métodos
instancia (`.year()`/`.month()`/`.format(fmt)`/`.to_str()`/...) y
round-trip transparente con Postgres. **Paridad bit-a-bit
`fitz run` ↔ `fitz build` desde v0.10.26**: el codegen emite
chrono/uuid en `Cargo.toml`, helpers de marshaling/parsing
condicionales, y `__IntoPgValue`/`__FromFitzDbRow` para los 3
tipos. Cero deuda residual en el bloque temporal/identidad.

Convención de defaults: el user escribe `happens_on: Date = ""`
(sentinel `Str`) y provee el valor real al construir la Instance
con `Date.from_ymd(2026, 12, 25)?` o desde HTTP body JSON (que
deserializa automático `"2026-12-25"` → `Value::Date`).

El user crea las tablas con `CREATE TABLE` (manualmente, via
`db.exec(...)` al boot, o via `fitz db diff/migrate` desde
v0.10.16).

### `@hidden`: ocultar fields de la frontera HTTP

A partir de **v0.10.11**, el decorator `@hidden` marca un field
como invisible para el JSON I/O:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str = ""
    name: Str = ""
    @hidden password_hash: Str = ""   // <-- NUNCA cruza HTTP
    role: Str = "user"
}
```

**Qué cambia**:
- `__to_fitz_json` skipea el field — el cliente HTTP **NUNCA** ve
  `password_hash` en cualquier response que devuelva un `User`
  (directo, como field de `Post.author`, eager-loaded via
  `.preload("author")`, etc.).
- `__FromFitzJson` rechaza el field — si el cliente envía un body
  con `{"password_hash": "..."}`, el server responde 400 con
  `"campo no declarado"`.
- El ORM lo **persiste normalmente** en Postgres — el INSERT lo
  incluye, el SELECT lo trae de vuelta, `.update(...)` lo
  modifica. Solo cambia el boundary HTTP.

**Cuándo usar**:
- Campos sensibles (`password_hash`, tokens internos, claves API).
- Metadata interna que no debe leakearse (timestamps de auditoría
  sin sentido para el cliente, internal_status para flags
  privados, etc.).

**Cuándo NO usar** (alternativas mejores):
- Campos que el cliente envía al **register/login** pero no debe
  recibir de vuelta: usá un type dedicado `RegisterInput` /
  `Credentials` separado del `User` table type. `@hidden` cubre
  el lado "no exponer" pero el flujo input + persistencia separa
  responsabilidades mejor.

**Ortogonal a `@table`**: `@hidden` también funciona en types
plain HTTP sin `@table`:

```fitz
type ResponseEnvelope {
    data: Map<Str, Any>
    @hidden internal_trace_id: Str = ""   // log interno, no al cliente
}
```

### `@index(...)` para composite / unique / partial indexes (v0.10.27)

Decorator a nivel **type** (no a field) que declara índices que
`fitz db diff` y `fitz db migrate` emiten automáticamente:

```fitz
@table("posts")
@index(author_id)                                  // simple
@index(status, published_at)                       // composite
@index(slug, unique=true)                          // unique
@index(status, name="published_idx", where_=status == "published")  // partial
type Post {
    @primary id: Int = 0
    author_id: Int = 0
    slug: Str = ""
    status: Str = "draft"
    published_at: Str = ""
}
```

**Kwargs**:
- `unique=true` — `CREATE UNIQUE INDEX`.
- `name="..."` — override del nombre. Default
  `idx_<table>_<col1>_<col2>...[_uniq]`.
- `where_=...` — partial index. Acepta cualquier expresión
  comparativa simple traducida a SQL (`col == "x"`, `col > N`,
  `col is_in [a, b]`, etc.). El nombre del kwarg es `where_`
  porque `where` es reservada del lenguaje.

**Cuándo aplica**:
- `fitz db diff` muestra los `CREATE INDEX` que faltan.
- `fitz db migrate` los aplica.
- Si la tabla **ya existe** con índices distintos, el diff muestra
  el delta (drop + create cuando cambian columnas, drop puro
  cuando sobran).

**Lo que NO está en v0.10.27**:
- **Expression indexes** sobre llamadas/funciones (`lower(email)`,
  `to_tsvector('english', body)`): no se generan auto. Workaround:
  `db.exec("CREATE INDEX ...")` al boot.
- **Index method override** (`USING hash`, `USING gin`): no
  expresable. Default `btree` (Postgres default).

---

## 5. Read methods: `.all`, `.first`, `.count`, `.where`

Los read methods son **estáticos** sobre el type (Type.method),
no sobre instancias.

### `Type.all(db) -> Future<Result<List<Type>>>`

Devuelve todas las rows de la tabla:

```fitz
let users: List<User> = User.all(db).await?
for u in users {
    print("{u.id}: {u.email}")
}
```

⚠️ Sin paginación, esto trae **TODAS las rows**. Sobre tablas
chicas (<10000 rows) está bien; sobre tablas grandes, usar
`.where(...).limit(...).offset(...)` (ver sección 19 Paginación).

### `Type.where(closure) -> QueryBuilder<Type>`

Empieza una chain de filtros. El closure recibe un nominal del
row y devuelve un `Bool`. El checker valida estáticamente que el
closure referencie fields existentes en el `type`. El translator
DURANTE EL CODEGEN walka el AST del closure y emite SQL
parametrizado constante.

```fitz
// SQL emitido: SELECT ... FROM users WHERE "age" > $1 AND "role" = $2
let admins = User.where(fn(u) => u.age > 18 and u.role == "admin").all(db).await?
```

`.where(...)` NO ejecuta la query. Devuelve un `QueryBuilder<User>`
que se sigue encadenando con chain methods (sección 6) o termina
con `.all(db)` / `.first(db)` / `.count(db)`.

### `Type.first(db) -> Future<Result<Type>>` (sin where = primer row de la tabla)

Atajo equivalente a `Type.where(fn(_) => true).first(db)`. Devuelve
el primer row de la tabla (el server elige el orden — usar
`.order_by(...)` para garantizar determinismo). `Err` si la tabla
está vacía.

```fitz
let user = User.first(db).await?   // primer row, orden indefinido
```

### `Type.count(db) -> Future<Result<Int>>`

Atajo equivalente a `Type.where(fn(_) => true).count(db)`. Total
de rows de la tabla:

```fitz
let total = User.count(db).await?
print("total users: {total}")
```

---

## 6. QueryBuilder reference: chain y terminales

El `QueryBuilder<Row>` retornado por `Type.where(...)` (o
`Type.preload(...)`, ver sección 12) es **inmutable**. Cada chain
method devuelve un nuevo builder con el state acumulado. El SQL
final se compone en el terminal.

### Chain methods (preservan el QueryBuilder<Row>)

#### `.where(closure)`

Suma otro filtro al WHERE. Múltiples `.where(...)` se combinan
con `AND`:

```fitz
// SQL emitido: WHERE "age" >= $1 AND "role" = $2
let qb = User.where(fn(u) => u.age >= 18).where(fn(u) => u.role == "admin")
let result = qb.all(db).await?
```

Equivalente a:

```fitz
let result = User.where(fn(u) => u.age >= 18 and u.role == "admin").all(db).await?
```

(Estilísticamente, **prefiere el AND adentro del mismo closure** —
queda más legible y el codegen emite el mismo SQL).

#### `.order_by(closure, ascending: Bool)`

Ordena por el field referenciado en el closure. `ascending: true`
default; `false` para DESC.

```fitz
// SQL emitido: ORDER BY "age" DESC
let top = User.where(fn(u) => u.active)
    .order_by(fn(u) => u.age, ascending: false)
    .all(db).await?
```

Múltiples `.order_by(...)` se acumulan:

```fitz
// SQL emitido: ORDER BY "role" ASC, "age" DESC
let sorted = User.where(fn(u) => u.active)
    .order_by(fn(u) => u.role)                    // ASC default
    .order_by(fn(u) => u.age, ascending: false)
    .all(db).await?
```

#### `.limit(n: Int)`

LIMIT N. Solo último prevalece si se llama múltiples veces.

```fitz
let first_10 = User.where(fn(u) => u.active).limit(10).all(db).await?
```

#### `.offset(n: Int)`

OFFSET N. Solo último prevalece.

```fitz
let page_2 = User.where(fn(u) => u.active).limit(10).offset(10).all(db).await?
```

#### `.group_by(closure) -> Aggregated<Row>`

Cambia el tipo retornado a `Aggregated<Row>`. Los terminales
disponibles ahora son distintos (sección 9 GROUP BY).

```fitz
let by_role = User.group_by(fn(u) => u.role).count(db).await?
// by_role: List<Map<Str, Any>> con un row por grupo.
```

#### `.preload(relation_name: Str) -> QueryBuilder<Row>`

Eager loading. El relation_name es **Str literal en compile-time**
(no se acepta var). Ver sección 12.

```fitz
let users_with_posts = User.preload("posts").all(db).await?
// Cada user.posts ya está hidratado — cero queries adicionales.
```

### Terminales (ejecutan el SQL final)

#### `.all(db) -> Future<Result<List<Row>>>`

Ejecuta y devuelve todas las rows que matchean:

```fitz
let result: List<User> = qb.all(db).await?
```

#### `.first(db) -> Future<Result<Row>>`

Ejecuta con `LIMIT 1`. `Err` si no hay match:

```fitz
let one: User = User.where(fn(u) => u.id == 42).first(db).await?
```

#### `.count(db) -> Future<Result<Int>>`

Ejecuta `SELECT COUNT(*) ... WHERE ...`. Más eficiente que
`.all(db).await?.len()`:

```fitz
let n: Int = User.where(fn(u) => u.active).count(db).await?
```

#### `.sum(closure, db) / .avg(closure, db) / .min(closure, db) / .max(closure, db)`

Aggregates scalar sobre el field referenciado en el closure. Ver
sección 9.

```fitz
let total_age: Float = User.where(fn(u) => u.active).sum(fn(u) => u.age, db).await?
let avg_age: Float = User.where(fn(u) => u.active).avg(fn(u) => u.age, db).await?
```

Nota: `.sum`/`.avg`/`.min`/`.max` devuelven **`Float`** (cast
`::float8` automático en el SQL para simplificar el wire protocol).
`.count` devuelve `Int`.

#### `.update(db, changes: Map) / .delete(db) -> Future<Result<Int>>`

Write terminales con guard `.where(...)` obligatorio (sección 8):

```fitz
let updated_rows = User.where(fn(u) => u.id == 42).update(db, {"role": "admin"}).await?
let deleted_rows = User.where(fn(u) => u.id == 42).delete(db).await?
```

---

## 7. Operadores extendidos en `.where(...)`

El translator del closure → SQL soporta muchas operaciones más
allá de comparators básicos:

### Comparators

```fitz
User.where(fn(u) => u.age == 18)           // "age" = $1
User.where(fn(u) => u.age != 18)           // "age" <> $1
User.where(fn(u) => u.age < 18)            // "age" < $1
User.where(fn(u) => u.age <= 18)           // "age" <= $1
User.where(fn(u) => u.age > 18)            // "age" > $1
User.where(fn(u) => u.age >= 18)           // "age" >= $1
```

### Lógicos

```fitz
User.where(fn(u) => u.age >= 18 and u.active)        // ... AND ...
User.where(fn(u) => u.age >= 18 or u.role == "vip")  // ... OR ...
User.where(fn(u) => not u.active)                    // NOT ...
```

Asociatividad estándar; usar paréntesis para agrupar explícito:

```fitz
// (age >= 18 AND role = 'admin') OR id = 1
User.where(fn(u) => (u.age >= 18 and u.role == "admin") or u.id == 1)
```

### Aritméticos (incluyendo `%` mod)

```fitz
User.where(fn(u) => u.age + 5 > 25)        // "age" + $1 > $2
User.where(fn(u) => u.age * 2 < 50)        // "age" * $1 < $2
User.where(fn(u) => u.age % 2 == 0)        // "age" % $1 = $2  (pares)
```

### `between(lo, hi)` sobre fields numéricos

```fitz
User.where(fn(u) => u.age.between(18, 65))   // "age" BETWEEN $1 AND $2
```

### `is_in([a, b, c])` sobre cualquier field

```fitz
User.where(fn(u) => u.id.is_in([1, 2, 3]))   // "id" = ANY($1::int8[])
User.where(fn(u) => u.role.is_in(["admin", "moderator"]))
```

Lista vacía → predicado `false` literal (no rompe el query, el
SELECT simplemente no matchea nada). `IN ()` no es SQL válido,
así que el translator emite `false` como predicado equivalente.

⚠️ **Caveat MVP**: el arg de `.is_in(...)` debe ser un **List
literal directo** (`.is_in([1, 2, 3])` o `.is_in([x, y])`). Una
**variable** del scope externo NO funciona como arg directo
(`.is_in(some_var)` → error). Los items adentro de la lista
sí pueden ser variables (`.is_in([min_id, max_id])` OK).

### Métodos sobre columns Str

```fitz
User.where(fn(u) => u.email.is_null())               // "email" IS NULL
User.where(fn(u) => u.email.is_not_null())           // "email" IS NOT NULL
User.where(fn(u) => u.email.like("%@example.com"))   // "email" LIKE $1
User.where(fn(u) => u.email.ilike("%ADA%"))          // "email" ILIKE $1 (case-insensitive)
User.where(fn(u) => u.email.starts_with("ada"))      // "email" LIKE $1 (con "ada%")
User.where(fn(u) => u.email.ends_with("@x.com"))     // "email" LIKE $1 (con "%@x.com")
User.where(fn(u) => u.email.contains("ada"))         // "email" LIKE $1 (con "%ada%")
```

**Patterns con `%`/`_`/`\` se escapan automáticamente** en
`starts_with`/`ends_with`/`contains` (NO en `like`/`ilike` donde
el user controla el pattern manualmente).

### Variables externas al closure

El translator soporta vars del scope exterior al closure como
parámetros:

```fitz
let min_age = 18
let role_filter = "admin"

// SQL emitido: WHERE "age" >= $1 AND "role" = $2  con args [18, "admin"]
let adults = User.where(fn(u) => u.age >= min_age and u.role == role_filter).all(db).await?
```

Útil para handlers HTTP donde el filter viene de un query param:

```fitz
@get("/users/by-role/{role}")
async fn by_role(role: Str) -> Result<List<User>> {
    return User.where(fn(u) => u.role == role).all(db).await
}
```

### Array ops (ver sección 17 para más)

```fitz
Post.where(fn(p) => p.tags.has("rust"))                          // $1 = ANY(tags)
Post.where(fn(p) => p.tags.contains_all(["rust", "postgres"]))   // tags @> $1
Post.where(fn(p) => p.tags.contained_in(["rust", "postgres", "go"]))  // tags <@ $1
```

### Tabla resumen de soporte de variables externas

| Operador / Method | Var externa soportada |
|-------------------|----------------------|
| Comparators (`==`, `!=`, `<`, `<=`, `>`, `>=`) | ✅ ambos lados |
| Lógicos (`and`/`or`/`not`) | n/a |
| Aritméticos (`+`/`-`/`*`/`/`/`%`) | ✅ ambos lados |
| `.between(low, high)` | ✅ low/high vars OK |
| `.is_in(literal_list)` | ⚠️ List arg literal; items adentro OK |
| `.like(pat)` / `.ilike(pat)` | ✅ pat var OK |
| `.starts_with(s)` / `.ends_with(s)` / `.contains(s)` | ❌ Str literal REQUERIDO |
| `.is_null()` / `.is_not_null()` | n/a (sin args) |
| `.has(v)` | ❌ literal escalar REQUERIDO |
| `.contains_all([...])` / `.contained_in([...])` | ❌ literal escalares REQUERIDOS |
| `.has_key(s)` (JSONB) | ✅ var Str OK |
| `.get(s)` (JSONB) | ✅ var Str OK |
| `.has_all_keys([...])` / `.has_any_keys([...])` (JSONB) | ❌ List literal de Str |
| `.contains_json({...})` (JSONB) | ❌ Map literal con values primitivos |

Cuando algo del translator no alcanza, bajar a `db.query(...)`
crudo con SQL escrito a mano.

### Lo que NO soporta el translator

- Field access sobre nested types (no JOINs implícitos): `u.posts.title`
  no funciona. Usar `Post.where(fn(p) => p.user_id == u.id)` aparte.
- Llamadas a fns custom adentro del closure: el closure tiene que
  ser un bloque expresión sobre el row.
- `match` adentro del closure.
- `if/else` adentro del closure (puede agregarse como refinamiento;
  hoy un sólo expr-block sin branches).
- String interpolation adentro del closure: `u.email == "{prefix}@x.com"`
  no se evalúa al SQL — usar concatenación afuera y pasar var.

---

## 8. Write methods: `.insert`, `.update`, `.delete`

Los write methods modifican el state de la DB. Cada uno tiene su
propio safety check.

### `Type.insert(db, row) -> Future<Result<Type>>`

Inserta un row. Si el `@primary` es Int con default `= 0`, Postgres
lo auto-asigna (`bigserial`) y el resultado tiene el id real:

```fitz
let inserted = User.insert(db, User {
    id: 0,                     // auto-asignado por Postgres
    email: "ada@x.com",
    age: 35,
    role: "admin"
}).await?
print("nueva id: {inserted.id}")     // e.g. 42
```

INSERT emite `RETURNING *` internamente, así que el row devuelto
tiene todos los fields hidratados (incluyendo cualquier default
declarado del lado SQL).

### `Type.bulk_insert(rows, db, batch_size=1000) -> Future<Result<Int>>` (v0.10.27)

Inserta múltiples rows en batches multi-tuple `VALUES (...), (...), ...`.
Default `batch_size=1000` (configurable). Devuelve el conteo total
de rows insertadas:

```fitz
let rows: List<User> = []
let mut i = 0
while (i < 5000) {
    rows.push(User { id: 0, name: "user_{i}", role: "guest" })
    i = i + 1
}
let n = User.bulk_insert(rows, db).await?
print("inserted: {n}")  // 5000
```

**Sentinel `id: 0`**: detectado de la PRIMERA row. Si la primera
trae `id: 0`, el SQL omite la columna PK en TODAS las rows del
batch (Postgres genera con `bigserial`). Si la primera trae
`id > 0`, todas las N rows incluyen PK explícito. Shape uniforme
exigido (no mezclar). Composite PK: valores explícitos siempre
(no hay sentinel multi-col).

**No emite `RETURNING *`** (a diferencia de `.insert`): si
necesitás los IDs auto-generados de cada row, usá `.insert` en
loop. `bulk_insert` está optimizado para seeds/migraciones donde
solo importa el conteo.

Ver sección 25 para más patterns bulk.

### `QueryBuilder.update(db, changes: Map) -> Future<Result<Int>>`

Sobre un `QueryBuilder<Row>` con `.where(...)` previo **obligatorio**.
El ORM rechaza estáticamente updates sin guard (`Type.update(db,
{...})` directo sin `.where(...)` → error de codegen). Esto
previene el accidente clásico de "olvidé el WHERE":

```fitz
// ✅ Con guard — actualiza el row específico
let updated_rows = User.where(fn(u) => u.id == 42)
    .update(db, {"age": 36, "role": "admin"})
    .await?
print("rows actualizadas: {updated_rows}")

// ❌ Sin guard — error de codegen
let oops = User.update(db, {"role": "user"}).await?
//   ↑ error: .update() requiere .where(...) previo. Para
//   actualizar TODAS las rows, usá .where(fn(_) => true).update(...).
```

El segundo arg de `.update` es un `Map` con `Str` keys y values
del tipo de la columna. Acepta:

- **Map literal heterogéneo**: `{"age": 36, "role": "admin",
  "active": true}`.
- **List literal**: `{"tags": ["rust", "postgres"]}` (mapea a `text[]`).
- **Map literal nested**: `{"metadata": {"k": 1, "k2": "x"}}` (mapea a `jsonb`).

```fitz
Post.where(fn(p) => p.id == 1)
    .update(db, {
        "title": "nuevo título",
        "tags": ["rust", "postgres", "fitz"],   // List → text[]
        "metadata": {"draft": false, "ts": 1700000000}   // Map → jsonb
    })
    .await?
```

### `QueryBuilder.delete(db) -> Future<Result<Int>>`

Mismo safety pattern: `.where(...)` obligatorio. Devuelve el
número de rows borradas.

```fitz
// ✅ Con guard
let deleted = User.where(fn(u) => u.role == "trial" and u.age < 18)
    .delete(db).await?

// ❌ Sin guard — error de codegen
let oops = User.delete(db).await?
//   ↑ error: .delete() requiere .where(...) previo. Para
//   borrar TODAS las rows, usá .where(fn(_) => true).delete(db).
```

Para "borrar todo" intencionalmente: `.where(fn(_) => true).delete(db)`
es explícito y compila. Pero `db.exec("TRUNCATE TABLE ...", [])`
es generalmente mejor (más rápido + resetea el counter de
`bigserial`).

---

## 9. Aggregates scalar + GROUP BY

Dos paths separados:

- **Scalar**: sobre `QueryBuilder<Row>` → devuelve `Float` (o `Int`
  para `.count`).
- **GROUP BY**: sobre `Aggregated<Row>` (creado con `.group_by`) →
  devuelve `List<Map<Str, Any>>`.

El checker distingue ambos paths estáticamente con la variante
`Type::Aggregated(Box<Type>)`.

### Aggregates scalar sobre QueryBuilder

```fitz
let total: Int = User.count(db).await?
let avg_age: Float = User.avg(fn(u) => u.age, db).await?
let max_age: Float = User.max(fn(u) => u.age, db).await?
let min_age: Float = User.min(fn(u) => u.age, db).await?
let sum_logins: Float = User.where(fn(u) => u.active).sum(fn(u) => u.login_count, db).await?
```

Cast `::float8` automático en `avg`/`sum`/`min`/`max` para que el
wire protocol no necesite parsear `numeric` (deuda menor; el driver
soporta solo OIDs core en MVP).

### GROUP BY

`.group_by(closure)` cambia el tipo retornado del builder a
`Aggregated<Row>`:

```fitz
// SQL emitido: SELECT "role", COUNT(*) AS count FROM users GROUP BY "role"
let by_role = User.group_by(fn(u) => u.role).count(db).await?
// by_role: List<Map<Str, Any>>
// by_role[0] → {"role": "admin", "count": 3}
// by_role[1] → {"role": "user", "count": 47}
```

Sobre `Aggregated<Row>`, los **chain methods** preservan el
tipo (siguen siendo `Aggregated<Row>`):

- `.where(closure)` — agrega un filtro AND al WHERE pre-GROUP BY.
- `.order_by(closure)` — ordena el output post-aggregate.
- `.limit(n)` / `.offset(n)` — pagina el output.
- `.group_by(closure)` — agrega otra columna al GROUP BY.

**Terminales aggregate** (todos devuelven
`Future<Result<List<Map<Str, Any>>>>`):

- `.count(db)`
- `.sum(closure, db)`
- `.avg(closure, db)`
- `.min(closure, db)`
- `.max(closure, db)`

⚠️ `.all`/`.first`/`.update`/`.delete` NO son válidos sobre
`Aggregated<Row>` — error claro en compile-time. Para colapsar
los grupos, usar siempre un aggregate terminal.

Cada row del resultado tiene:

- El field del `group_by` con su value original (e.g. `"role":
  "admin"`).
- Un campo numérico con el resultado del aggregate, named según el
  método y el field:
  - `.count(db)` → `"count": <Int>`
  - `.sum(closure, db)` → `"sum_<field>": <Float>`
  - `.avg(closure, db)` → `"avg_<field>": <Float>`

```fitz
// SQL: SELECT "role", AVG("age")::float8 AS avg_age FROM users GROUP BY "role"
let avg_age_by_role = User.group_by(fn(u) => u.role).avg(fn(u) => u.age, db).await?
// avg_age_by_role[0] → {"role": "admin", "avg_age": 35.5}
```

### GROUP BY combinado con WHERE

`.group_by(...)` se puede encadenar después de `.where(...)`:

```fitz
let active_by_role = User.where(fn(u) => u.active)
    .group_by(fn(u) => u.role)
    .count(db).await?
```

(El order matters semánticamente — `.where` filtra antes del GROUP BY,
equivale a SQL `WHERE ... GROUP BY ...`).

### Limitaciones GROUP BY actuales

- **GROUP BY multi-column**: solo single closure por ahora.
  `GROUP BY a, b` requiere helper futuro.
- **HAVING clause**: no soportado en MVP. Workaround: filtrar el
  resultado del lado Fitz con `.filter(fn(row) => row["count"] > 5)`.
- **Aggregates múltiples en el mismo GROUP BY**: no soportado.
  Workaround: dos queries separadas o `db.query(...)` crudo con
  el SQL completo.
- **`List<Map<Str, Any>>` en HTTP returns** — **CERRADO v0.10.4**.
  El codegen HTTP ahora serializa automáticamente este shape a
  JSON via `impl __MapKey for __FitzValue` en el preludio HTTP.
  Endpoints como `GET /stats` con `User.group_by(...).count(db).await`
  funcionan end-to-end en `fitz build` con paridad bit-a-bit
  contra `fitz run`. Ver `examples/guide/31b-orm-crud-http.fitz`
  endpoint `/stats/by-email`.

---

## 10. Relations: `@belongs_to`, `@has_one`, `@has_many`

Decoradores sobre fields para declarar relations cross-table.

### `@belongs_to("Target")`

El field marcado **ES** la columna FK real en la tabla. Entra al
SELECT, al INSERT, al UPDATE. Mapping clásico:

```fitz
@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    @belongs_to("User") user_id: Int   // FK column real en posts
}
```

El field `user_id` es una columna `bigint REFERENCES users(id)` en
Postgres. El decorator habilita el navigation method `post.user_id(db)`
que resuelve al `User` correspondiente (sección 11).

### `@has_many("Target", via="fk_column")`

El field marcado es **virtual** — NO entra al SELECT/INSERT normal.
El `via` indica cuál columna de la tabla `Target` apunta hacia atrás.

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    @has_many("Post", via="user_id") posts: List<Post>
}
```

El field `posts: List<Post>` no es una columna en `users`; es un
shortcut del ORM que se hidrata con:

- `user.posts(db)` — navigation method (1 SELECT, sección 11).
- `User.preload("posts").all(db)` — eager loading batch (1 SELECT
  para todos los posts, sección 12).

### `@has_one("Target", via="fk_column")`

Igual que `@has_many` pero con cardinalidad 1:1. El field virtual
es `Target?` (nullable) en lugar de `List<Target>`:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    @has_one("Profile", via="user_id") profile: Profile?
}

@table("profiles") type Profile {
    @primary id: Int = 0
    @belongs_to("User") user_id: Int
    bio: Str
}
```

Navigation: `user.profile(db) -> Result<Profile>` (Err si no
existe).

### Kwargs `on_delete` / `on_update` de las relations

Las relations aceptan kwargs `on_delete=...` y `on_update=...`
sobre el MISMO decorator (no son decorators separados). Valores
como **string literal**: `"cascade"`, `"set_null"`, `"restrict"`,
`"no_action"`. El ORM persiste el FK action en la metadata pero
**NO genera la migración** (MVP). El user crea la tabla con
`db.exec("CREATE TABLE ... user_id bigint REFERENCES users(id) ON
DELETE CASCADE", [])` manualmente:

```fitz
@table("comments") type Comment {
    @primary id: Int = 0
    body: Str
    @belongs_to("Post", on_delete="cascade") post_id: Int
}

@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    @belongs_to("User", on_delete="set_null", on_update="cascade")
    author_id: Int
    @has_many("Comment", via="post_id", on_delete="cascade")
    comments: List<Comment>
}
```

Si en el futuro entra `fitz db migrate`, leerá esos kwargs para
generar el DDL apropiado.

### Kwargs `fk` / `via` para nombres FK custom

Por default, el ORM asume convención de nombre. Para overrides:

- **`@belongs_to("Target", fk="custom_fk_field")`** — cuando el
  field FK en este type NO sigue la convención `<target>_id`:

  ```fitz
  @belongs_to("User", fk="created_by_user_id") created_by: Int
  ```

- **`@has_many("Target", via="custom_fk_column")` / `@has_one(...,
  via="...")`** — cuando la columna FK en la tabla Target NO se
  llama `<this>_id`:

  ```fitz
  @has_many("Post", via="author_user_id") posts: List<Post>
  ```

### Cuándo usar cuál

| Cardinalidad | Decorator del side dueño | Field del otro side |
|--------------|-------------------------|---------------------|
| 1 → N (User tiene N Post) | `@has_many("Post", via="user_id") posts: List<Post>` | `@belongs_to("User") user_id: Int` |
| 1 → 1 (User tiene 1 Profile) | `@has_one("Profile", via="user_id") profile: Profile?` | `@belongs_to("User") user_id: Int` |
| N → 1 (cada Order tiene 1 Customer) | (no necesario del side N) | `@belongs_to("Customer") customer_id: Int` |
| M ↔ N (Post ↔ Tag via post_tags) | **Manual** — tabla intermedia con dos `@belongs_to`. Ver receta sección 21. |

---

## 11. Navigation methods + chain

Después de declarar relations, cada field genera un **método de
navegación** sobre la instancia que resuelve la relation a runtime.

### BelongsTo: `instance.fk_field(db) -> Future<Result<Target>>`

```fitz
let post = Post.where(fn(p) => p.id == 1).first(db).await?
let author: User = post.user_id(db).await?
print("autor de '{post.title}': {author.email}")
```

El método se llama **como el field FK** (`user_id`, no `user`).
El runtime ejecuta `SELECT * FROM users WHERE id = $1` con el FK
value como param.

### HasMany: `instance.virtual_field(db) -> Future<Result<List<Target>>>`

```fitz
let user = User.where(fn(u) => u.id == 1).first(db).await?
let user_posts: List<Post> = user.posts(db).await?
print("posts de {user.email}: {len(user_posts)}")
```

El método se llama **como el field virtual declarado** (`posts`).
El runtime ejecuta `SELECT * FROM posts WHERE user_id = $1`.

### HasOne: `instance.virtual_field(db) -> Future<Result<Target>>`

```fitz
let user = User.where(fn(u) => u.id == 1).first(db).await?
let profile: Profile = user.profile(db).await?
// Err si el user no tiene profile (es Result<Profile>, no Result<Profile?>).
```

### Navigation chain: `instance.field()` (sin `db`) devuelve QueryBuilder

Cuando la navigation se llama SIN el `db` arg (`args.is_empty()`),
devuelve un `QueryBuilder<Target>` que sigue encadenando:

```fitz
// Equivale a SELECT * FROM posts WHERE user_id = $1 ORDER BY id DESC LIMIT 5
let latest_5 = user.posts()
    .order_by(fn(p) => p.id, ascending: false)
    .limit(5)
    .all(db).await?
```

Por diseño:

- `user.posts(db)` — terminal directo (ejecuta query).
- `user.posts()` — empieza chain (no ejecuta hasta el terminal).

El segundo es útil para filtros adicionales sobre la relation:

```fitz
let recent_drafts = user.posts()
    .where(fn(p) => p.status == "draft" and p.created_at > "2026-01-01")
    .order_by(fn(p) => p.created_at, ascending: false)
    .limit(10)
    .all(db).await?
```

### N+1 manual (sin .preload)

Si tenés N users y querés sus posts:

```fitz
let users = User.all(db).await?
for u in users {
    let posts = u.posts(db).await?   // 1 query por user (N+1)
    print("{u.email}: {len(posts)} posts")
}
```

Esto hace **N+1 queries** (1 para `User.all` + N para cada
`u.posts(db)`). Para evitarlo, usar `.preload(...)` (sección 12).

---

## 12. Eager loading con `.preload(...)`

Cierra el N+1 con **dispatch estático en compile-time**. El
relation name viaja como Str literal en `.preload(...)`; el
codegen emite un `match` exhaustivo por type con la rama
correspondiente. **Typos detectados en compile-time, no
runtime**.

### Uso básico

```fitz
// 1 query batch para users + 1 query batch para TODOS los posts
// WHERE user_id IN (id_user_1, id_user_2, ...)
let users: List<User> = User.preload("posts").all(db).await?

for u in users {
    print("{u.email}: {len(u.posts)} posts")
    //                ↑ ya está hidratado, cero queries adicionales
    for p in u.posts {
        print("  - {p.title}")
    }
}
```

**Total queries**: 2 (vs N+1 manual).

### Typos en relation name

El relation name como Str literal queda hard-coded en el binario:

```fitz
let users = User.preload("post").all(db).await?
//                       ↑ typo: "post" en lugar de "posts"
// → error de codegen:
//    relation "post" no existe en User. Conocidas: posts.
```

Compile-time, no runtime. SQLAlchemy te dice esto al evaluar
`User.posts` con un typo; Fitz te lo dice al hacer `fitz build`.

### Combinado con .where

`.preload(...)` se puede encadenar con `.where(...)`:

```fitz
let active_with_posts = User.preload("posts")
    .where(fn(u) => u.active)
    .all(db).await?
```

Orden de llamadas no importa funcionalmente:

```fitz
// Misma query final
let r1 = User.where(fn(u) => u.active).preload("posts").all(db).await?
let r2 = User.preload("posts").where(fn(u) => u.active).all(db).await?
```

### Limitaciones del .preload en MVP

- **BelongsTo eager via convention** (deuda #2 cerrada v0.10.5):
  `Post.preload("user").all(db)` ahora funciona cuando el type
  declara el companion field. Convención: `@belongs_to("User")
  user_id: Int` + sibling field `user: User?` (mismo type, name
  derivado stripping `_id`, tipo Nullable<Target>). El checker
  registra el companion como `BelongsToCompanion`; el codegen
  emite el batch SELECT inverso (`target.id IN parent.fk
  DISTINCT`); el field `user` se inicializa `None` por default
  y queda poblado post-preload. Sin el sibling declarado, sigue
  el workaround manual con `is_in(...)`:
  ```fitz
  let posts = Post.all(db).await?
  let user_ids = posts.map(fn(p) => p.user_id)
  let authors = User.where(fn(u) => u.id.is_in(user_ids)).all(db).await?
  ```
- **Single-level**. `.preload("posts").preload("posts.comments")` no
  soportado (cargar posts + comments de cada post en 3 queries).
  Workaround: `.preload("posts")` + N+1 manual sobre `u.posts`.
- **Sin filtrado por relation**: `.preload("posts").where(fn(u) =>
  u.active)` filtra users, no posts. Para filtrar posts pre-load,
  hacer la query separada con `.where(fn(p) => p.user_id.is_in(
  user_ids))`.

---

## 13. JSONB: `Map<Str, Any>` ↔ `jsonb`

Un field `data: Map<Str, Any>` se mapea a columna `jsonb`. INSERT
serializa con `serde_json` (`preserve_order` para mantener orden de
inserción) y cast `::jsonb`. SELECT parsea el text JSON de vuelta
a Map Fitz preservando shape heterogéneo.

### Declaración

```fitz
@table("events") type Event {
    @primary id: Int = 0
    name: Str
    data: Map<Str, Any>    // jsonb column
}
```

### Insert con JSONB heterogéneo

```fitz
let e = Event.insert(db, Event {
    id: 0,
    name: "click",
    data: {
        "page": "/home",
        "ts": 1700000000,
        "user_agent": "Mozilla/5.0",
        "active": true,
        "session_id": null
    }
}).await?
```

### Update incremental sobre JSONB

Para actualizar **todo el JSONB**, pasar el Map completo:

```fitz
Event.where(fn(e) => e.id == 42)
    .update(db, {"data": {"page": "/about", "ts": 1700001000}})
    .await?
```

Para actualizar **una key específica** del JSONB sin re-escribir
todo: bajar a `db.exec` crudo con el JSON operator `||`:

```fitz
db.exec("UPDATE events SET data = data || $1::jsonb WHERE id = $2",
    [json_string, 42]).await?
```

### SELECT y consumo

```fitz
let e = Event.where(fn(e) => e.id == 42).first(db).await?
print(e.data["page"])           // "/home"
print(e.data["ts"])             // 1700000000  (Int adentro del Map<Str, Any>)
print(e.data["session_id"])     // null
```

Los values del `Map<Str, Any>` mantienen su tipo Fitz original:
`Int`/`Float`/`Str`/`Bool`/`Null`/`List<Any>`/`Map<Str, Any>`
nested.

### Nested Maps

JSONB anidado se preserva:

```fitz
let e = Event.insert(db, Event {
    id: 0,
    name: "purchase",
    data: {
        "item": {"id": 42, "name": "T-shirt", "price": 19.99},
        "qty": 2,
        "tags": ["promo", "winter-sale"]
    }
}).await?

let back = Event.where(fn(e) => e.id == e.id).first(db).await?
let item = back.data["item"]
print(item["name"])     // "T-shirt"
```

### JSON operators integrados en `.where(...)` (v0.10.5)

Cinco method calls sobre fields jsonb (`Map<Str, ...>`) se
mapean a operadores nativos Postgres:

| Method                              | SQL emitido          | Postgres operator |
|-------------------------------------|----------------------|-------------------|
| `e.data.has_key("foo")`             | `"data" ? $1`        | key exists        |
| `e.data.has_all_keys(["a", "b"])`   | `"data" ?& $1::text[]` | all keys exist  |
| `e.data.has_any_keys(["a", "b"])`   | `"data" ?| $1::text[]` | any key exists  |
| `e.data.contains_json({"k": "v"})`  | `"data" @> $1::jsonb`  | jsonb contains  |
| `e.data.get("foo")`                 | `("data"->>$1)`       | text extract     |

Ejemplos end-to-end:

```fitz
// "todos los events que tengan la key 'page'"
let with_page = Event.where(fn(e) => e.data.has_key("page")).all(db).await?

// "todos los events que tengan TANTO 'page' como 'user'"
let with_both = Event.where(fn(e) =>
    e.data.has_all_keys(["page", "user"])
).all(db).await?

// "todos los events con CUALQUIERA de las keys 'code' o 'extra'"
let either = Event.where(fn(e) =>
    e.data.has_any_keys(["code", "extra"])
).all(db).await?

// "todos los events cuyo jsonb contenga AL MENOS {page: '/home'}"
let from_home = Event.where(fn(e) =>
    e.data.contains_json({"page": "/home"})
).all(db).await?

// .get(key) devuelve text — comparable contra Str literal:
// "todos los events del usuario 'ada'"
let ada_events = Event.where(fn(e) =>
    e.data.get("user") == "ada"
).all(db).await?
```

**Caveats MVP** (refinable):

- `.has_key(s)` / `.get(s)` aceptan vars externas como arg
  (passan por el translator general).
- `.has_all_keys([...])` / `.has_any_keys([...])` / `.contains_json({...})`
  requieren **literales** (List o Map literal directo, no var).
- `.contains_json({...})` solo acepta values primitivos
  (Int/Float/Str/Bool/Null). Maps/Lists nested adentro: workaround
  con `db.query(...)` crudo.
- `.get(key) == value` compara texto. Para comparar contra Int,
  bajar a `db.query` con cast `(data->>'k')::int`. Refinamiento
  futuro: `.get_int(key)` / `.get_float(key)` / etc.
- Nested path access (`e.data.get("a").get("b")` → `data->'a'->>'b'`)
  no implementado MVP. Workaround: `db.query` crudo.

Para casos no cubiertos, sigue disponible el escape hatch crudo:

```fitz
let promos = db.query(
    "SELECT * FROM events WHERE data->>'tags' LIKE $1",
    ["%promo%"]
).await?
```

### Null Fitz → NULL real (no la string "null")

```fitz
let e = Event.insert(db, Event { id: 0, name: "x", data: {"k": null} }).await?
// Postgres: data = '{"k": null}'::jsonb  → JSONB key con valor JSON null
```

vs `"null"` Str que sería el literal string.

---

## 14. Arrays Postgres: `List<scalar>` ↔ `T[]`

12 array OIDs soportados (`bool[]`/`int2[]`/`int4[]`/`int8[]`/
`text[]`/`varchar[]`/`float4[]`/`float8[]`/`date[]`/`timestamp[]`/
`timestamptz[]`/`uuid[]`).

### Declaración

```fitz
@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    tags: List<Str>           // text[]
    scores: List<Int>          // int8[]
    weights: List<Float>       // float8[]
    flags: List<Bool>          // bool[]
}
```

### Insert con arrays

```fitz
let p = Post.insert(db, Post {
    id: 0,
    title: "Hola",
    tags: ["rust", "postgres", "fitz"],
    scores: [10, 20, 30],
    weights: [0.1, 0.5, 0.9],
    flags: [true, false, true]
}).await?
```

### SELECT round-trip preserva orden

```fitz
let back = Post.where(fn(p) => p.id == p.id).first(db).await?
print(back.tags[0])         // "rust"
print(back.scores[1])       // 20
```

### Update con array literal

```fitz
Post.where(fn(p) => p.id == 1)
    .update(db, {"tags": ["nuevo", "etiquetas"]})
    .await?
```

### Append a un array (workaround crudo)

```fitz
db.exec(
    "UPDATE posts SET tags = array_append(tags, $1) WHERE id = $2",
    ["nueva-tag", 42]
).await?
```

Los operadores `array_append`/`array_remove`/`array_cat` no están
en el translator del ORM en MVP — usar `db.exec` crudo.

---

## 15. NULL en arrays: `List<scalar?>`

Postgres permite NULL adentro de arrays (`int8[] NULL` con
elementos `{1, NULL, 3}`). Fitz lo mapea con `List<scalar?>`:

```fitz
@table("readings") type Reading {
    @primary id: Int = 0
    samples: List<Int?>           // int8[] con elementos nullable
}

let r = Reading.insert(db, Reading {
    id: 0,
    samples: [10, null, 30, null, 50]    // Int?
}).await?

let back = Reading.where(fn(r) => r.id == r.id).first(db).await?
// back.samples: List<Int?> con NULLs preservados
for v in back.samples {
    match v {
        Ok(n)  => print("valor: {n}")
        Err(_) => print("(null)")
    }
}
```

El text format Postgres `{1,NULL,3}` se parsea/encodea
simétricamente. El parser distingue `NULL` (sin quotes) del literal
`"NULL"` (con quotes).

### Cuándo usar arrays nullable

Datos de sensores donde "NaN" o "no medido" es semánticamente
distinto a 0:

```fitz
@table("temperature_readings") type TempReading {
    @primary id: Int = 0
    location_id: Int
    samples: List<Float?>      // null = sensor no reportó esta muestra
    timestamp: Str
}
```

Para arrays donde NULL no tiene sentido (e.g. `tags: List<Str>`),
usar la versión non-nullable: `List<Str>` (no `List<Str?>`).

---

## 16. `Map<Str, T>` concreto homogéneo

Alternativa a `Map<Str, Any>` cuando todos los values son del mismo
tipo primitivo (Int/Float/Str/Bool). El marshaling es directo
(HashMap<String, T> Rust), **sin overhead de enum dispatch**:

```fitz
@table("counters") type CounterSnapshot {
    @primary id: Int = 0
    period: Str
    counts: Map<Str, Int>     // jsonb con shape homogéneo
}

let snap = CounterSnapshot.insert(db, CounterSnapshot {
    id: 0,
    period: "2026-05",
    counts: {"clicks": 1234, "views": 5678, "purchases": 42}
}).await?

let back = CounterSnapshot.where(fn(s) => s.id == s.id).first(db).await?
print(back.counts["clicks"])    // 1234 (Int, no Any)
```

### Restricciones

- **K debe ser Str**. Postgres jsonb keys son strings. `Map<Int, Int>`
  → error claro en codegen.
- **T debe ser primitivo concreto** (Int/Float/Str/Bool). Nested
  Maps (`Map<Str, Map<Str, Int>>`) no soportados en MVP — usar
  `Map<Str, Any>`.

### Cuándo usar `Map<Str, T>` vs `Map<Str, Any>`

| Caso | Preferir |
|------|----------|
| Shape homogéneo conocido (e.g. contadores, métricas) | `Map<Str, T>` (más eficiente, tipo concreto) |
| Shape heterogéneo (e.g. settings dinámicas, metadata libre) | `Map<Str, Any>` (flexible) |
| Anidado en cualquier nivel | `Map<Str, Any>` (MVP no permite T compuesto) |

---

## 17. Array ops en `.where(...)`

Tres operadores Postgres sobre arrays mapeados a method calls Fitz
adentro del closure:

### `.has(elem)` → `$1 = ANY(column)`

¿El elemento está en el array?

```fitz
// SQL emitido: WHERE $1 = ANY("tags")
let rusty = Post.where(fn(p) => p.tags.has("rust")).all(db).await?
```

### `.contains_all([a, b, ...])` → `column @> $1`

¿El array contiene TODOS los elementos especificados?

```fitz
// SQL emitido: WHERE "tags" @> $1::text[]
let both = Post.where(fn(p) => p.tags.contains_all(["rust", "postgres"])).all(db).await?
```

### `.contained_in([a, b, ...])` → `column <@ $1`

¿TODOS los elementos del array están en la lista especificada?

```fitz
// SQL emitido: WHERE "scores" <@ $1::int8[]
let small = Post.where(fn(p) => p.scores.contained_in([1, 2, 3, 4, 5])).all(db).await?
```

### Combinaciones

Como cualquier otro filtro, se combinan con AND/OR:

```fitz
let curated = Post.where(fn(p) =>
    p.tags.has("featured") and
    p.scores.contains_all([100]) and
    not p.archived
).all(db).await?
```

### Caveat MVP

Los array ops (`has`/`contains_all`/`contained_in`) requieren
**args como literales del tipo escalar del array**: Int/Float/Str/
Bool. **Variables del scope externo NO se aceptan** como arg
directo del method. Workaround: bajar a `db.query(...)` crudo:

```fitz
// ❌ ERROR: vars adentro de array ops no soportadas
let some_tag = "rust"
Post.where(fn(p) => p.tags.has(some_tag)).all(db).await?
//                                 ↑ MVP: el arg debe ser literal

// ✅ Workaround: db.query crudo con $param
let rows = db.query(
    "SELECT * FROM posts WHERE $1 = ANY(tags)",
    [some_tag]
).await?
```

Refinamiento futuro probable: permitir vars del scope externo en
los args de array ops, paralelo a lo que ya funciona para
comparators básicos.

---

## 18. Date / Time / Timestamp / UUID

Estos tipos Postgres se modelan como **`Str` ISO 8601** (canonical
para Date/Time) o **`Str` formato canonical UUID** en MVP. El
driver hace el round-trip text↔Postgres correctamente; el `type`
Fitz no tiene primitivos `Date`/`DateTime`/`UUID` dedicados
todavía.

### Date / Time / Timestamp / Timestamptz

```fitz
@table("events") type Event {
    @primary id: Int = 0
    name: Str
    occurred_at: Str           // timestamp / timestamptz
    occurred_date: Str         // date
    occurred_time: Str         // time
}

// Formatos canónicos:
let e = Event.insert(db, Event {
    id: 0,
    name: "alarm",
    occurred_at: "2026-05-26T16:30:00Z",     // RFC 3339 / ISO 8601
    occurred_date: "2026-05-26",              // YYYY-MM-DD
    occurred_time: "16:30:00"                 // HH:MM:SS
}).await?
```

Comparaciones lexicográficas funcionan correctamente para ISO 8601
(las strings se ordenan en orden temporal):

```fitz
let recent = Event.where(fn(e) => e.occurred_at > "2026-01-01T00:00:00Z").all(db).await?
```

### UUID

```fitz
@table("sessions") type Session {
    @primary token: Str         // UUID v4 canonical
    user_id: Int
    expires_at: Str
}

// Generar UUID v4 del lado Fitz (placeholder hasta tener builtin uuid):
let token = "550e8400-e29b-41d4-a716-446655440000"   // hardcoded para el ejemplo

let s = Session.insert(db, Session {
    token: token,
    user_id: 42,
    expires_at: "2026-12-31T23:59:59Z"
}).await?
```

UUID generation built-in (`uuid.v4()` ?) queda como mini-fase
futura.

### Limitaciones

- **Sin Date/DateTime/UUID nativos**: validación de formato es
  responsabilidad del user al insertar. Postgres rechaza con error
  si el formato es inválido (lo cual se propaga como `Result::Err`).
- **Sin aritmética de fechas adentro del translator**: `e.occurred_at
  + interval '1 day'` no funciona. Workaround: `db.query(...)` crudo
  con SQL que usa `INTERVAL`/`AGE`/`EXTRACT`.
- **Timezone handling**: timestamptz se preserva en UTC; conversión
  a local timezone es responsabilidad del cliente Fitz que consume.

---

## 19. Recetas — paginación

### Offset/Limit clásico

El patrón más simple. Para pages chicas y datasets que no cambian
constantemente:

```fitz
@get("/users")
async fn list_users(page: Int, page_size: Int) -> Result<List<User>> {
    let offset = (page - 1) * page_size
    return User.where(fn(u) => u.active)
        .order_by(fn(u) => u.id)         // ⚠️ ORDER BY obligatorio
        .limit(page_size)
        .offset(offset)
        .all(db).await
}
```

**Caveat crítico**: SIN un `ORDER BY` determinístico, Postgres NO
garantiza orden estable entre pages. Siempre incluir
`.order_by(fn(u) => u.id)` (o el field que defina el orden de
display).

**Performance**: para offsets muy grandes (e.g. página 1000 con
page_size=10 = offset 10000), Postgres lee todas las rows hasta el
offset y descarta — costoso. Usar cursor-based para datasets
grandes.

### Cursor-based (más eficiente para datasets grandes)

```fitz
@get("/users")
async fn list_users(after_id: Int, page_size: Int) -> Result<List<User>> {
    return User.where(fn(u) => u.id > after_id and u.active)
        .order_by(fn(u) => u.id)
        .limit(page_size)
        .all(db).await
}

// Cliente llama:
// /users?after_id=0&page_size=20      → primera página
// /users?after_id=20&page_size=20     → segunda (último id de la prev)
```

**Tradeoffs**:

- ✅ Performance constante O(log N) por page (usa el index del `id`).
- ✅ Inmune a inserts/deletes durante la paginación.
- ❌ No permite saltos directos (no hay "página 47", solo
  "siguientes 20 después del último visto").

### Página + total para UI con paginador clásico

```fitz
@get("/users")
async fn list_users(page: Int, page_size: Int) -> Result<Map<Str, Any>> {
    let offset = (page - 1) * page_size

    let users: List<User> = User.where(fn(u) => u.active)
        .order_by(fn(u) => u.id)
        .limit(page_size)
        .offset(offset)
        .all(db).await?

    let total: Int = User.where(fn(u) => u.active).count(db).await?

    return Ok({
        "users": users,
        "total": total,
        "page": page,
        "page_size": page_size,
        "total_pages": (total + page_size - 1) / page_size
    })
}
```

⚠️ **Mismo caveat del cap 31**: `Map<Str, Any>` en HTTP returns
no serializa a JSON automáticamente todavía (gap residual). Para
este caso, definir un `type PaginatedUsers { ... }` concreto.

---

## 20. Recetas — búsqueda

### Búsqueda simple por prefijo / substring

```fitz
@get("/users/search")
async fn search_users(q: Str) -> Result<List<User>> {
    // Match por substring case-insensitive sobre email + name.
    return User.where(fn(u) => u.email.ilike("%{q}%") or u.name.ilike("%{q}%"))
        .order_by(fn(u) => u.id)
        .limit(50)
        .all(db).await
}
```

⚠️ `.ilike(...)` con `%` adelante (e.g. `"%ada%"`) NO usa el index
del field → escaneo lineal. Para tablas grandes, considerar:

- **Trigram index** (`pg_trgm` extension): `CREATE INDEX users_email_trgm ON
  users USING gin (email gin_trgm_ops)`.
- **Full-text search** (`tsvector` + `tsquery`).

### Full-text search con tsvector (workaround crudo)

```fitz
@get("/articles/search")
async fn search_articles(q: Str) -> Result<List<Map<Str, Any>>> {
    // tsquery + ts_rank requieren db.query crudo en MVP.
    return db.query(
        "SELECT id, title, ts_rank(search_vector, query) AS rank
         FROM articles, websearch_to_tsquery($1) query
         WHERE search_vector @@ query
         ORDER BY rank DESC
         LIMIT 20",
        [q]
    ).await
}
```

Pre-requisito: la tabla `articles` tiene una columna
`search_vector tsvector` actualizada (typical via trigger sobre
`title + body`).

### Búsqueda en arrays (tags)

```fitz
@get("/posts/by-tag/{tag}")
async fn by_tag(tag: Str) -> Result<List<Post>> {
    return Post.where(fn(p) => p.tags.has(tag))
        .order_by(fn(p) => p.id, ascending: false)
        .limit(50)
        .all(db).await
}
```

### Búsqueda en JSONB (workaround crudo)

```fitz
@get("/events/by-page/{page}")
async fn by_page(page: Str) -> Result<List<Map<Str, Any>>> {
    return db.query(
        "SELECT id, name, data FROM events
         WHERE data->>'page' = $1
         ORDER BY id DESC
         LIMIT 50",
        [page]
    ).await
}
```

---

## 21. Recetas — search filters combinatorios

Construir queries dinámicas según los filtros que el cliente
envía. **Patrón**: cada filter opcional se aplica condicionalmente.

```fitz
type UserFilters {
    role: Str?
    min_age: Int?
    max_age: Int?
    active_only: Bool
    name_contains: Str?
}

@post("/users/search")
async fn search(filters: UserFilters) -> Result<List<User>> {
    // Empezamos con un base query.
    let qb = User.where(fn(u) => u.id > 0)   // condición trivial inicial

    // Sumar filtros condicionalmente.
    // Nota: la API actual del ORM NO soporta chain dinámico
    // (.where condicional adentro de un if).
    // Workaround: pattern match sobre los filtros y usar
    // múltiples query branches, o usar db.query crudo con
    // SQL construido programáticamente.

    // Patrón con .where múltiples (combinando con AND):
    if filters.active_only {
        // ... aquí necesitaríamos algo como qb = qb.where(...)
    }

    // En MVP, el approach más limpio es:
    return search_dynamic(filters).await
}

async fn search_dynamic(f: UserFilters) -> Result<List<User>> {
    // SQL armado a mano con db.query crudo.
    let where_parts = ["u.id > 0"]

    // ... build dynamic SQL ...
    // (este patrón se documenta más limpio cuando el ORM soporte
    //  chain condicional. Hoy es deuda residual.)
}
```

### Chain dinámico condicional (v0.10.5)

A pesar de que el SQL se construye en compile-time, **el receiver
del chain puede ser una variable mutable** — el codegen emite cada
chain method como `(receiver).with_<x>(...)` y el `QueryBuilder<T>`
runtime es cloneable. Esto habilita el patrón "armar filters
condicionalmente" sin bajar a `db.query` crudo:

```fitz
async fn search(min_age: Int, active_only: Bool, name_like: Str, db: DbConn)
    -> Result<List<User>>
{
    let qb = User.where(fn(u) => u.age >= min_age)

    if (active_only) {
        qb = qb.where(fn(u) => u.active)
    }

    if (name_like != "") {
        qb = qb.where(fn(u) => u.name.like(name_like))
    }

    return qb.order_by(fn(u) => u.id).all(db).await
}
```

Funciona también con `.order_by(...)`, `.limit(n)`, `.offset(n)`:

```fitz
async fn paginated(page: Int, page_size: Int, sort_desc: Bool, db: DbConn)
    -> Result<List<User>>
{
    let qb = User.where(fn(u) => u.age > 0)

    if (sort_desc) {
        qb = qb.order_by(fn(u) => -u.age)
    } else {
        qb = qb.order_by(fn(u) => u.age)
    }

    if (page_size > 0) {
        qb = qb.limit(page_size).offset((page - 1) * page_size)
    }

    return qb.all(db).await
}
```

Caveat: cada chain method genera un fragmento SQL constante (que
respeta las restricciones del closure, ver sec 7). El SHAPE del
chain es dinámico (se decide en runtime cuáles branches del `if`
toman), pero cada fragmento individual sigue siendo compile-time.

### Patrón alternativo: filters fijos con `match`

Si los filtros son pocos y las combinaciones bounded, también
funciona el approach de branches separados sin construir un `qb`
mutable:

```fitz
@get("/users/by-status/{status}")
async fn by_status(status: Str) -> Result<List<User>> {
    return match status {
        "active" => User.where(fn(u) => u.active).all(db).await
        "inactive" => User.where(fn(u) => not u.active).all(db).await
        "admins" => User.where(fn(u) => u.role == "admin").all(db).await
        _ => Err("status no válido")
    }
}
```

Esta forma es más declarativa cuando las combinaciones se conocen
de antemano. La forma dinámica con `qb = qb.where(...)` brilla
cuando hay N filtros opcionales independientes.

---

## 22. Recetas — Auth + ORM (queries scoped al user autenticado)

El ORM se integra naturalmente con auth nativa (cap 28).

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    role: Str
}

@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    body: Str
    @belongs_to("User") author_id: Int
}

@auth_provider
async fn auth(headers: Map<Str, Str>) -> Result<User> {
    let token = headers.get("authorization")?
    let claims = jwt.decode(token, "mi-secret")?
    let email = claims["email"]
    return User.where(fn(u) => u.email == email).first(db).await
}

// GET /my-posts — solo los posts del user autenticado.
@authenticated
@get("/my-posts")
async fn my_posts(user: User) -> Result<List<Post>> {
    return Post.where(fn(p) => p.author_id == user.id)
        .order_by(fn(p) => p.id, ascending: false)
        .all(db).await
}

// POST /posts — crea un post atribuído al user autenticado.
type PostInput { title: Str, body: Str }

@authenticated
@post("/posts")
async fn create_post(user: User, body: PostInput) -> Result<Post> {
    let p = Post {
        id: 0,
        title: body.title,
        body: body.body,
        author_id: user.id     // ← user inyectado por el auth provider
    }
    return Post.insert(db, p).await
}

// DELETE /posts/{id} — solo si el user es el author O es admin.
@authenticated
@delete("/posts/{id}")
async fn delete_post(user: User, id: Int) -> Result<Int> {
    let post = Post.where(fn(p) => p.id == id).first(db).await?

    if post.author_id != user.id and user.role != "admin" {
        return Err("no autorizado para borrar este post")
    }

    return Post.where(fn(p) => p.id == id).delete(db).await
}
```

### Patrón canonical: `user.id` en cada filter

Cada handler protegido que opera sobre datos del user incluye
`u.author_id == user.id` (o equivalente) en el WHERE. **Cero
"olvidé el filter del owner"** porque el `user` lo inyecta el
provider explícitamente al handler.

### Combinar con `@admin`

```fitz
@admin
@delete("/users/{id}")
async fn delete_user(id: Int, admin: User) -> Result<Int> {
    return User.where(fn(u) => u.id == id).delete(db).await
}
```

`@admin` ya valida estáticamente que `user.role == "admin"` —
solo ejecuta el handler si el caller es admin.

---

## 23. Recetas — HTTP CRUD completo

El showcase de combinar todo el stack está en
`examples/guide/31b-orm-crud-http.fitz` (~135 LoC) que el cap 31
referencia. Cubre:

- `GET /` health check
- `GET /users` (list all)
- `GET /users/{id}` (get one)
- `POST /users` (create, body `UserInput`)
- `PUT /users/{id}` (update, body `UserInput`)
- `DELETE /users/{id}`
- `GET /users/{id}/posts` (relation query)
- `POST /posts` (create con FK al user)
- `GET /users-with-posts` (eager loading con `.preload`)
- `GET /user-count` (aggregate scalar)

Patterns demostrados:

- **Types separados para DB shape vs HTTP entrada** (`User` vs
  `UserInput`). El input no incluye `id` (auto-asignado) ni
  `posts` (virtual). Mejor cohesión que reusar `User` para ambos.
- **`env_or("DATABASE_URL", default)`** para configuración via
  env var con fallback.
- **Helper `open_db()`** para reusar la URL en cada handler.
- **`Result<T>` con `?`** para propagar errores ORM hasta el
  cliente (500 con `{"error": "..."}` automático).

### Variante con state shared (1 conn pool global)

El ejemplo del cap 31 abre una conn por request via `open_db()`.
Para usar un pool global compartido entre handlers (más eficiente
en producción):

```fitz
// Top-level: connect una sola vez al boot.
let db = db.connect(env_or("DATABASE_URL", "postgres://...")).await?
db.exec("CREATE TABLE IF NOT EXISTS users (...)", []).await?

@server(3000)
fn main() => 0

// Cada handler usa el `db` top-level.
@get("/users")
async fn list_users() -> Result<List<User>> {
    return User.all(db).await
}
```

Funciona en `fitz run`. En `fitz build` el codegen detecta `db`
como state HTTP compartido y lo emite como `Arc<Mutex<DbConn>>`
(deuda F17 cerrada). Validar bit-a-bit en el ejemplo es deuda
futura — el patrón `open_db()` del ejemplo es la versión segura.

---

## 24. Recetas — Cron job de limpieza

Cron jobs + ORM se combinan naturalmente para tareas batch:

```fitz
@table("sessions") type Session {
    @primary id: Int = 0
    token: Str
    user_id: Int
    expires_at: Str    // ISO 8601
}

// Cada hora, borrar sessions expiradas.
@cron("0 * * * *")
async fn cleanup_expired_sessions() {
    let now_iso = "2026-05-26T16:00:00Z"   // placeholder; hasta tener now() builtin

    let deleted = Session.where(fn(s) => s.expires_at < now_iso)
        .delete(db).await

    match deleted {
        Ok(n)  => print("[cleanup] {n} sessions expiradas borradas")
        Err(e) => print("[cleanup] error: {e}")
    }
}
```

### Drafts viejos sin auto-publish

```fitz
@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    status: Str         // "draft" | "published" | "archived"
    created_at: Str
}

@cron("0 0 * * 0")   // cada domingo a medianoche
async fn archive_old_drafts() {
    let threshold = "2026-01-01T00:00:00Z"   // 6 meses atrás (placeholder)

    let archived = Post.where(fn(p) => p.status == "draft" and p.created_at < threshold)
        .update(db, {"status": "archived"})
        .await

    match archived {
        Ok(n)  => print("[archive] {n} drafts archivados")
        Err(e) => print("[archive] error: {e}")
    }
}
```

### Daily stats compute

```fitz
@cron("5 0 * * *")   // 00:05 cada día
async fn compute_daily_stats() {
    // Total users activos.
    let active = User.where(fn(u) => u.active).count(db).await

    // Promedio de posts por user.
    let avg_posts = db.query(
        "SELECT AVG(c)::float8 AS avg_posts FROM (
             SELECT COUNT(*) AS c FROM posts GROUP BY author_id
         ) sub",
        []
    ).await

    // ... persist stats a una tabla `daily_stats` ...
    print("[stats] daily compute done")
}
```

---

## 25. Recetas — Bulk operations

### Insert múltiple — `Type.bulk_insert(rows, db)` (v0.10.27)

```fitz
async fn seed_users(db: DbConn) -> Result<Int> {
    let rows: List<User> = []
    let mut i = 0
    while (i < 1000) {
        rows.push(User { id: 0, name: "user_{i}", role: "guest" })
        i = i + 1
    }
    let n = User.bulk_insert(rows, db).await?
    print("inserted: {n}")
    return Ok(n)
}
```

Emite `INSERT INTO users ("name", "role") VALUES ($1, $2), ($3, $4), ...`
en batches de 1000 rows por default (configurable con
`User.bulk_insert(rows, db, batch_size=500)`). Devuelve el conteo
total de rows insertadas.

**Sentinel `id: 0` auto-bigserial**: la primera row del batch
decide. Si su PK es `0` (Int sentinel), el SQL omite la columna
PK en TODAS las rows del batch (Postgres genera con
`bigserial`). Si la primera row trae PK > 0, las N rows incluyen
PK explícito. Asunción: shape uniforme — no mezclar rows con y
sin PK en el mismo `bulk_insert`.

**Composite PK** (v0.10.27): el batch lleva los N valores del PK
tuple explícitos en cada row. Sentinel no aplica (no hay
`bigserial` para composite).

**Costos**: 1 round-trip por batch (~1000 rows). Para datasets
mayores (cientos de miles +), `db.copy_in` (COPY FROM STDIN)
sigue como deuda futura.

**Patrón anterior** (loop con `.insert`) sigue válido para casos
de <100 rows o cuando necesitás la `Instance` devuelta por cada
insert con todos los defaults aplicados.

### Update múltiple sobre set de IDs

```fitz
let ids = [1, 2, 3, 4, 5]

// SQL emitido: UPDATE users SET "role" = $1 WHERE "id" = ANY($2::int8[])
let updated = User.where(fn(u) => u.id.is_in(ids))
    .update(db, {"role": "vip"})
    .await?
print("updated rows: {updated}")
```

### Delete por batch

```fitz
let deleted = Post.where(fn(p) => p.status == "spam")
    .delete(db).await?
print("spam posts borrados: {deleted}")
```

---

## 26. Recetas — Schema idempotente al boot

Pattern canonical para que el binario "se auto-bootee" creando las
tablas si no existen:

```fitz
async fn boot_schema(db: DbConn) -> Result<Null> {
    db.exec("CREATE TABLE IF NOT EXISTS users (
        id bigserial PRIMARY KEY,
        email text NOT NULL UNIQUE,
        name text NOT NULL,
        role text NOT NULL DEFAULT 'user',
        active boolean NOT NULL DEFAULT true,
        created_at timestamptz NOT NULL DEFAULT NOW()
    )", []).await?

    db.exec("CREATE TABLE IF NOT EXISTS posts (
        id bigserial PRIMARY KEY,
        title text NOT NULL,
        body text NOT NULL,
        status text NOT NULL DEFAULT 'draft',
        tags text[] NOT NULL DEFAULT '{}',
        metadata jsonb NOT NULL DEFAULT '{}',
        author_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        created_at timestamptz NOT NULL DEFAULT NOW()
    )", []).await?

    // Índices comunes.
    db.exec("CREATE INDEX IF NOT EXISTS posts_author_idx ON posts(author_id)", []).await?
    db.exec("CREATE INDEX IF NOT EXISTS posts_tags_gin ON posts USING gin (tags)", []).await?

    return Ok(null)
}

async fn main() -> Result<Null> {
    let db = db.connect(env_or("DATABASE_URL", "postgres://postgres:postgres@localhost/demo?sslmode=disable")).await?
    boot_schema(db).await?

    // ... resto del programa (HTTP server, jobs, etc.) ...
    return Ok(null)
}
```

### Migraciones manuales versionadas

Hasta tener `fitz db diff`/`migrate` (Fase 10.6+), las migraciones
explícitas se manejan con un patrón simple:

```fitz
async fn run_migrations(db: DbConn) -> Result<Null> {
    db.exec("CREATE TABLE IF NOT EXISTS schema_migrations (
        version text PRIMARY KEY,
        applied_at timestamptz NOT NULL DEFAULT NOW()
    )", []).await?

    let applied = db.query("SELECT version FROM schema_migrations", []).await?
    let applied_versions = applied.map(fn(r) => r["version"])

    // 001_initial.sql
    if not applied_versions.has("001_initial") {
        db.exec("CREATE TABLE users (...)", []).await?
        db.exec("INSERT INTO schema_migrations (version) VALUES ('001_initial')", []).await?
        print("[migrate] aplicada: 001_initial")
    }

    // 002_add_posts.sql
    if not applied_versions.has("002_add_posts") {
        db.exec("CREATE TABLE posts (...)", []).await?
        db.exec("INSERT INTO schema_migrations (version) VALUES ('002_add_posts')", []).await?
        print("[migrate] aplicada: 002_add_posts")
    }

    return Ok(null)
}
```

Workflow CLI dedicado a migrations queda como Fase 10.6+.

---

## 26.b. Transactions (v0.10.14)

Cuando un handler hace **varias escrituras que deben ser
atómicas** (escenario clásico: transferí dinero de cuenta A
a cuenta B; crear un Order + sus N OrderItems; sumar puntos a
un usuario + log de la operación), envolvelas en
`db.transaction(fn(tx) -> Result<T> { ... })`.

### API canónica

```fitz
async fn transfer(db: DbConn, body: TransferInput) -> Result<Int> {
    return db.transaction(fn(tx) -> Result<Int> {
        // `tx: DbConn` es del mismo tipo que `db`, pero pegado a
        // la misma conn física durante toda la tx. Todos los
        // métodos del ORM (`.insert/.update/.delete/.first/.all`)
        // y el escape hatch (`tx.query/.exec`) funcionan sin
        // cambios — pasás `tx` en lugar de `db`.
        let _ = Account
            .where(fn(a) => a.id == body.from_id)
            .update(tx, { "balance": Account.balance - body.amount })
            .await?
        let _ = Account
            .where(fn(a) => a.id == body.to_id)
            .update(tx, { "balance": Account.balance + body.amount })
            .await?
        return Ok(body.amount)
    }).await
}
```

### Garantías

- **Atomicidad** — `BEGIN` antes del callback, `COMMIT` automático
  si retorna `Ok`, `ROLLBACK` automático si retorna `Err` o si
  paniquea. **Imposible quedarse a mitad de camino** — sin
  llamadas manuales a commit/rollback que olvidarse.
- **Aislamiento** — todas las queries adentro del callback usan
  la **misma conexión física**. El acquire del pool sucede una
  sola vez al inicio de la tx; las queries siguientes no compiten
  por slot del semaphore con otras requests concurrentes.
- **Cleanup automático** — la conn vuelve al pool al final
  (sea OK o Err). Sin connection leaks ni transactions
  colgadas server-side.
- **Preserva el `Err` original del callback** — si retornás
  `Err(MyError { ... })`, el caller recibe ese `MyError` intacto
  (no se aplana a un Str del driver).

### Recetas

#### Insert padre + hijos (Order + OrderItems)

```fitz
type OrderItem {
    @primary id: Int = 0
    @belongs_to("Order") order_id: Int = 0
    sku: Str = ""
    qty: Int = 0
}

@table("orders") type Order {
    @primary id: Int = 0
    @belongs_to("User") user_id: Int = 0
    total: Float = 0.0
    @has_many("OrderItem", via="order_id") items: List<OrderItem> = []
    @db_default created_at: Str = ""
}

async fn place_order(
    db: DbConn,
    user_id: Int,
    items: List<NewItem>,
) -> Result<Int> {
    let total = items.map(fn(it) => it.unit_price * it.qty).sum()
    return db.transaction(fn(tx) -> Result<Int> {
        let order = Order.insert(tx, Order {
            id: 0,
            user_id: user_id,
            total: total,
            created_at: "",
        }).await?
        // Si CUALQUIERA de los inserts de items falla, el order
        // tampoco persiste (ROLLBACK auto).
        for it in items {
            let _ = OrderItem.insert(tx, OrderItem {
                id: 0,
                order_id: order.id,
                sku: it.sku,
                qty: it.qty,
            }).await?
        }
        return Ok(order.id)
    }).await
}
```

#### Rollback explícito por validación

```fitz
return db.transaction(fn(tx) -> Result<Int> {
    let account = Account
        .where(fn(a) => a.id == account_id)
        .first(tx)
        .await?

    if (account.balance < amount) {
        // Retornar Err dispara ROLLBACK automático.
        // El balance NO se modificó (todavía).
        return Err("saldo insuficiente")
    }

    let _ = Account
        .where(fn(a) => a.id == account_id)
        .update(tx, { "balance": account.balance - amount })
        .await?
    return Ok(amount)
}).await
```

#### Escape hatch (tx.query / tx.exec)

Adentro de la tx también podés usar SQL crudo via los métodos
del DbConn. Útil para queries que el ORM no expresa (CTEs,
window functions, JSON operators avanzados):

```fitz
return db.transaction(fn(tx) -> Result<Int> {
    let _ = tx.exec(
        "UPDATE accounts SET locked_at = NOW() WHERE id = $1",
        [account_id],
    ).await?
    let _ = tx.exec(
        "INSERT INTO audit_log (user_id, action) VALUES ($1, $2)",
        [account_id, "lock"],
    ).await?
    return Ok(1)
}).await
```

### Diferencias clave vs otros ORMs

- **SQLAlchemy** usa `session.commit()`/`session.rollback()`
  explícitos. Si tu handler retorna antes de commit, la tx queda
  colgada server-side hasta que la conn se cierre. **Fácil de
  olvidarse**. Fitz no permite ese error — el `Ok`/`Err` del
  callback decide commit/rollback automático.
- **Diesel** (Rust) usa `conn.transaction(|tx| {...})` similar
  a Fitz — patrón validado por décadas en el ecosistema Rust.
- **Prisma** (TypeScript) tiene `prisma.$transaction([...])`
  con array de operaciones declarativas (menos flexible que
  closures) Y `prisma.$transaction(async (tx) => {...})`
  (closure-based, equivalente al de Fitz).

### Sintaxis del callback (v0.10.15)

Desde v0.10.15, el callback puede ser tanto FnExpr inline como
fn nombrada — paridad bit-a-bit entre `fitz run` y `fitz build`.

**Inline** (recomendado para closures cortos):

```fitz
return db.transaction(async fn(tx) -> Result<Int> {
    let _ = User.insert(tx, body).await?
    return Ok(1)
}).await
```

**Nombrada** (cuando reusás la misma lógica de tx en varios
endpoints o querés testearla aislada):

```fitz
async fn create_user_tx(tx: DbConn) -> Result<Int> {
    let _ = User.insert(tx, get_default_user()).await?
    return Ok(1)
}

return db.transaction(create_user_tx).await
```

**Captures del outer scope**: el body inline puede usar vars del
handler que envuelve la tx (`body.field`, `user.id`, args del
handler, etc.) sin pasarlos explícitamente. El codegen emite
doble `move` Rust (outer closure + async move inner) para
capturar correctamente.

### Lo que NO soporta el MVP (deuda futura)

- **Nested transactions** (SAVEPOINT). Una `db.transaction(...)`
  adentro de otra es no-op semántico — el inner `BEGIN` lo emite
  igual y Postgres lo ignora con `WARNING`. Para "rollback de
  una parte sin abortar el todo", queda como
  `tx.savepoint(name, fn(sp) => ...)` en Fase 10.7.b.
- **Niveles de aislamiento custom** — `BEGIN READ COMMITTED`,
  `BEGIN SERIALIZABLE`, etc. Usa el default del server (típico
  `READ COMMITTED`). Override via kwarg
  `db.transaction(fn, isolation="serializable")` queda como
  refinamiento futuro.
- **Transacciones read-only** — `BEGIN READ ONLY` para queries
  que solo leen. Hoy todo callback puede escribir.

---

## 26.c. Migraciones automáticas (v0.10.16)

`fitz db diff` introspecciona el schema real de Postgres, lo
compara con los `@table type` declarados en tu programa, y emite
el SQL `ALTER TABLE` necesario para sincronizarlos. `fitz db
migrate` aplica un directorio de migration files versionados
con tracking idempotente en la tabla `_fitz_migrations`. Sin
deps externas (Alembic / Flyway / Liquibase / TypeORM CLI):
todo vive en el binario `fitz`.

### Subcomandos

| Subcomando | Qué hace |
|---|---|
| `fitz db diff [archivo.fitz] [--out file.sql]` | Compara schema declarado (en `archivo.fitz` o `[bin].main` del manifest) con el real, emite SQL al stdout o file. |
| `fitz db migrate [--dry-run]` | Aplica los `.sql` pendientes del dir `./migrations` en orden alfabético, registra cada uno en `_fitz_migrations`. `--dry-run` muestra qué se aplicaría sin tocar la DB. |
| `fitz db status` | Lista cada archivo `.sql` con badge `✓ applied` / `→ PENDING`. |
| `fitz db new <name>` | Crea `migrations/YYYYMMDDHHMMSS_<name>.sql` con stub vacío. |

URL: lee `DATABASE_URL` env var, o pasa `--url
postgres://...`. Dir: `./migrations` por default, override con
`--dir`.

### Workflow canónico

```bash
# 1. Editás `@table type User { ... name: Str = "" }` agregando campo.
# 2. Generás migration vacía con timestamp.
fitz db new add_name_to_users
# → migrations/20260529150000_add_name_to_users.sql

# 3. Generás el SQL automático y lo redirigís a la migration.
fitz db diff > migrations/20260529150000_add_name_to_users.sql

# 4. Aplicás contra la DB.
fitz db migrate
# → ✓ 1 migration(s) aplicada(s): 20260529150000
```

### Política

- **Tracking**: tabla `_fitz_migrations` con `version TEXT
  PRIMARY KEY` + `applied_at TIMESTAMPTZ DEFAULT NOW()`.
- **Idempotente**: re-correr `migrate` con todo aplicado es
  no-op (`✓ todas las migrations ya aplicadas`).
- **Determinístico**: el diff emite cambios en orden seguro
  (CREATE TABLE → ADD/DROP/ALTER COLUMN → CREATE INDEX →
  DROP FK → ADD FK → DROP TABLE).
- **Quoted identifiers**: todos los `CREATE TABLE`/`ALTER
  COLUMN` quotean nombres (`"users"`, `"email"`) — los names
  con reserved words o caracteres especiales no rompen.

### Down migrations + `fitz db rollback` (v0.10.17)

Las migrations soportan secciones explícitas `-- UP` / `-- DOWN`
para permitir rollback. Backward-compatible: archivos sin
marcadores siguen siendo "UP implícito sin DOWN" (no se pueden
revertir, pero `migrate` los aplica igual).

```sql
-- Migration: add_email_to_users
-- Created: 2026-05-29T20:30:00Z

-- UP
ALTER TABLE "users" ADD COLUMN "email" text NOT NULL DEFAULT '';

-- DOWN
ALTER TABLE "users" DROP COLUMN "email";
```

```bash
fitz db migrate     # aplica el UP
fitz db rollback    # ejecuta el DOWN del último applied + borra
                    # registro en _fitz_migrations
fitz db rollback --count 3   # revierte las últimas 3
```

**Política**:

- `fitz db new <name>` (v0.10.17) genera stubs con `-- UP` y
  `-- DOWN` por convención — borrá la sección DOWN solo si la
  migration es genuinamente irreversible.
- El marcador es case-insensitive sobre línea propia: `-- UP`,
  `-- up`, `--UP`, `-- Up` matchean. `-- UP foo` NO (chars
  extra → SQL comment normal).
- Sección DOWN vacía / solo whitespace → tratada como `None`
  (irreversible).
- Si querés revertir N>1 y una de las target NO tiene `-- DOWN`,
  el rollback **aborta ANTES de tocar la DB** con mensaje
  específico citando el filename. Cero estado parcial.
- Cada `revert_migration` corre adentro de su propia tx (atomic
  por migration). Rollback de N>1 es N tx individuales — si la
  k-ésima falla en runtime, las anteriores ya persistieron. Para
  "todo o nada" sobre N migrations, escribí una migration única
  con todo el rollback.
- **No hay limitación de "última N en filename order"** — el
  rollback usa `applied_at DESC` del tracking, no el orden de
  los archivos. Si aplicaste migration A, después B, después
  reaplicaste A (caso raro pero posible vía stamp futuro),
  rollback revierte A primero.

### Renames seguros via `@renamed_from(...)` (v0.10.17)

El diff detecta renames mediante el decorator transient
`@renamed_from("nombre_anterior")` sobre el field o el type
mismo. Sin el decorator, un rename Fitz-side
`name → full_name` se ve como `DROP COLUMN name + ADD COLUMN
full_name`, **perdiendo los datos**. El decorator hace que el
diff emita `ALTER TABLE ... RENAME COLUMN` en su lugar.

```fitz
// Rename de column: campo Fitz `full_name` viene del column
// SQL anterior `name`.
@table("users") type User {
    @primary id: Int = 0
    @renamed_from("name") full_name: Str = ""
}

// Rename de tabla: type Fitz `User` mapea a la tabla SQL
// `users` que ANTES se llamaba `legacy_users`.
@table("users") @renamed_from("legacy_users") type User {
    @primary id: Int = 0
}
```

`fitz db diff` emite:

```sql
ALTER TABLE "legacy_users" RENAME TO "users";
ALTER TABLE "users" RENAME COLUMN "name" TO "full_name";
```

**Orden seguro**: los renames van PRIMERO en el output (antes
de cualquier `ADD/DROP COLUMN` o `ALTER COLUMN`). Esto garantiza
que las acciones siguientes referencian los nombres post-rename
(sin esto, un `ALTER COLUMN "full_name" TYPE varchar` sobre
una tabla recién renombrada fallaría porque el column todavía
no existe con ese nombre).

**Decorator transient** — borralo después de aplicar la
migration. El diff lo ignora silenciosamente cuando ya NO hay
match en current (la migration ya se aplicó):

- Si target tiene `@renamed_from("old")` Y current.columns
  contiene "old" Y NO contiene "new" → emite RENAME COLUMN.
- Si target tiene `@renamed_from("old")` pero current.columns
  ya solo tiene "new" → no-op silencioso (typical caso
  post-migration aplicada).
- Si target tiene `@renamed_from("old")` Y current contiene
  AMBOS "old" y "new" → no-op (sin colisión accidental — el
  diff típico tratará "new" como existente y "old" como a
  droppear según el resto del schema).

**Por qué decorator y no subcomando** (`fitz db rename`):

- Subcomando divorcia el rename del cambio en el code: fácil
  de olvidar uno o el otro.
- Decorator es declarativo, vive temporal en el code, atómico
  con el cambio del nombre del field/type.
- Después de aplicar, el user borra una línea — equivalente a
  cerrar un PR.

### History + offline SQL + squash (v0.10.20)

**`fitz db history`** — audit log de migrations aplicadas, orden
`applied_at DESC`. Cruza tracking con files del dir; si una
version está applied pero el file fue removido (caso típico de
`db stamp <legacy>` o post-squash), aparece como `(file removido)`.

```bash
fitz db history
# version              applied_at                       filename
# -------------------- -------------------------------- ----------
# 20260530120000       2026-05-30 10:53:24.800092-03    create_posts.sql
# 20260530100000       2026-05-30 10:53:24.775132-03    create_users.sql
# 2 migration(s) applied.
```

**`fitz db migrate --sql`** — Offline SQL mode. Emite el SQL
pendiente al stdout en vez de ejecutarlo. Sigue conectándose
para leer `_fitz_migrations` (qué está applied) y skipear esas.
Útil para handoff a un DBA que aplica manual. Rechaza `.fitz`
data migrations (no se materializan como SQL offline).

```bash
fitz db migrate --sql > pending.sql
# 1 migration emitida — pasalo al DBA.
psql -h prod-db -f pending.sql
# Después marcalo como applied:
fitz db stamp 20260530120000
```

**`fitz db squash <from> <to>`** — Combina las migrations del
rango `[from, to]` (inclusive) en una sola. Concatena los UP
en orden + los DOWN en orden inverso (para que el rollback siga
funcionando). Mueve los files originales a `migrations/squashed/`
(no los borra — quedan como histórico). Actualiza el tracking
para apuntar al nuevo squashed.

```bash
fitz db squash 20260101000000 20260301000000
# ✓ tracking actualizado: 47 versions removidas, stamped `20260101000000`
# ✓ 47 migration(s) squashed → migrations/20260101000000_squashed.sql.
#   Originales en migrations/squashed/.
```

Política:

- **Solo `.sql`**: rechaza `.fitz` en el rango (squashing de
  scripts del lenguaje no es semánticamente trivial — abríl
  manualmente).
- **Rango mínimo 2**: squashear 1 migration es no-op.
- **Tracking inteligente**: si alguna del range estaba applied
  en la DB, **borra** todas del rango de `_fitz_migrations` y
  **stampea** solo `from` (el nuevo squashed). Si ninguna estaba
  applied, no toca tracking — `migrate` aplicará el squashed
  como una migration normal.
- **Pre-flight**: aborta antes de tocar files si el squashed ya
  existe (evita sobre-escribir si re-corrés sin pensar).
- **`--no-tracking`**: skipea la actualización del tracking en
  `_fitz_migrations` (para repos sin DB de staging accesible
  desde el dev — útil en CI-only). Quedás responsable de stampear
  manual el squashed en cada DB existente.

Caso de uso típico: repo con 100+ migrations viejas que el equipo
ya aplicó hace meses. Squashear las primeras 80 acelera el
bootstrap de devs nuevos (1 migration en vez de 80) sin afectar
a quienes ya las aplicaron (el tracking las trata como una sola).

### Data migrations en `.fitz` (v0.10.19)

`fitz db migrate` ahora reconoce DOS extensiones en `migrations/`:
**`.sql`** (DDL/DML SQL crudo, splittable en `-- UP`/`-- DOWN`) y
**`.fitz`** (scripts del propio lenguaje con `db.query`/`db.exec`/
`db.transaction` adentro). Se intercalan en orden cronológico por
el prefijo timestamp del filename.

Las `.fitz` migrations habilitan transforms que SQL crudo no
expresa con elegancia: back-fills con lógica condicional, parseo
de JSON viejo a columns nuevas, HTTP calls a un service externo
durante la migración, etc.

**Convención del archivo `.fitz`**: declarar `async fn migrate(db:
DbConn) -> Result<Null>`. Opcionalmente `async fn rollback(db:
DbConn) -> Result<Null>` para que `fitz db rollback` ande.

```fitz
// migrations/20260530150000_backfill_full_name.fitz

async fn migrate(db: DbConn) -> Result<Null> {
    // 1 UPDATE sería suficiente acá, pero el patrón con loop
    // demuestra que tenés acceso completo al lenguaje:
    match db.query("SELECT id, first_name, last_name FROM users WHERE full_name IS NULL", []).await {
        Ok(rows) => {
            for r in rows {
                let id = r.get("id")
                let first = r.get("first_name")
                let last = r.get("last_name")
                let _ = db.exec(
                    "UPDATE users SET full_name = $1 || ' ' || $2 WHERE id = $3",
                    [first, last, id],
                ).await?
            }
            return Ok(null)
        }
        Err(e) => return Err(e),
    }
}

async fn rollback(db: DbConn) -> Result<Null> {
    let _ = db.exec("UPDATE users SET full_name = NULL", []).await?
    return Ok(null)
}
```

**Política**:

- **Discovery**: archivos `.sql` y `.fitz` se mezclan en el dir
  `migrations/`. Orden por prefijo timestamp del filename.
- **Tracking**: misma tabla `_fitz_migrations` que `.sql`. La
  version es el prefix del filename.
- **Atomicidad**: `.fitz` migrations **NO** se envuelven en tx
  automáticamente. El user decide la granularidad: típicamente
  `return db.transaction(fn(tx) -> Result<Null> { ... }).await`
  adentro del cuerpo de `migrate`. Es responsabilidad del script
  ser idempotente o atómico según su semántica.
- **Validación pre-run**: el runner parsea el `.fitz` y verifica
  que declara `async fn migrate(db: DbConn) -> Result<Null>`
  antes de tocar la DB.
- **Rollback opcional**: si el `.fitz` declara `async fn
  rollback(db: DbConn) -> Result<Null>`, `fitz db rollback` la
  invoca + borra el registro de tracking. Si NO la declara,
  rollback aborta pre-flight con mensaje claro (paralelo a `.sql`
  sin `-- DOWN`).
- **`db` está pre-bindeado** al env del script al `Value::DbConn`
  de la conn del CLI. NO requiere `db.connect(url)` adentro
  (el CLI ya conectó por vos via `DATABASE_URL`/`--url`).
- **El env del script tiene los builtins normales** (`print`,
  `len`, `env_or`, `jwt`, `hash`, etc.). Imports relativos al
  dir de la migration funcionan.

**Cuándo usar `.fitz` vs `.sql`**:

- `.sql` — DDL puro (CREATE TABLE / ADD COLUMN / CREATE INDEX),
  back-fills triviales (`UPDATE users SET x = 1 WHERE x IS NULL`),
  fixtures de seed. **80% de las migrations**.
- `.fitz` — back-fills con lógica condicional o loops, parseo de
  JSON viejo a columns nuevas, HTTP calls a un service externo
  durante la migración, transforms que requieren state que SQL
  crudo no expresa elegantemente.

**Caveat**: las `.fitz` migrations corren via **intérprete**
(no codegen), igual que `fitz run`. Para migrations grandes con
miles de iteraciones, considerá hacer el bulk via 1 UPDATE
SQL en una `.sql` separada en lugar de loop iterativo en `.fitz`.

### Drift check + stamping (v0.10.18)

**`fitz db check`** corre el diff y devuelve **exit 0** si el
schema declarado matchea la DB, **exit 1** con el SQL pendiente
al stderr si hay drift. Hook clave para CI bloqueante.

```bash
fitz db check src/main.fitz
# Sin drift:
#   ✓ schema sincronizado — schema declarado matchea la DB
#   (exit 0)
#
# Con drift:
#   ✗ drift detectado — 2 change(s) pendiente(s):
#
#   ALTER TABLE "users" ADD COLUMN "email" text NOT NULL;
#   ...
#
#   💡 corré `fitz db diff > migrations/<file>.sql` + `fitz db migrate`
#   (exit 1)
```

Patrón típico en `.github/workflows/*.yml`:

```yaml
- name: Schema drift check
  run: fitz db check src/main.fitz
  env:
    DATABASE_URL: ${{ secrets.STAGING_DB_URL }}
```

**`fitz db stamp <version>`** marca una migration como
aplicada en `_fitz_migrations` **sin ejecutar el SQL**. Útil
para adoptar Fitz en una DB legacy donde el schema ya está
aplicado manualmente (sin stamp, `migrate` intentaría re-aplicar
y fallaría con duplicados o sobrescribiría seed data).

```bash
# Adoptás Fitz en una DB existente:
# 1. Generás migration que matchea el schema actual:
fitz db diff src/main.fitz > migrations/20260530000000_initial.sql

# 2. Marcás la migration como aplicada SIN ejecutarla (la DB
#    ya tiene las tables):
fitz db stamp 20260530000000

# Output:
#   ✓ stamped: 20260530000000

# 3. A partir de acá, `fitz db migrate` aplica solo las nuevas:
fitz db migrate
```

**`fitz db stamp --all`** marca todas las pending del dir como
aplicadas en una pasada. Útil cuando tenés varias migrations
legacy ya aplicadas manualmente. Idempotente (versions ya
applied → no-op silencioso).

**Idempotencia**: stamp sobre una version ya applied → no-op
silencioso (`✓ no-op: version X ya estaba aplicada`). El
`ON CONFLICT DO NOTHING` interno cubre además el race entre dos
stamps concurrentes sobre la misma version.

**Warning sobre versions inexistentes**: si pasás una version
que NO existe en el dir `migrations/`, el stamp emite warning
pero igual la inserta — patrón "adopto una version legacy que
NUNCA voy a tener como file". El warning evita typos accidentales.

### Defaults SQL via `@db_default("expr")`

Desde v0.10.16, `@db_default` acepta un arg Str opcional con la
expresión SQL del default. Si está, `fitz db diff` emite
`DEFAULT <expr>` en el `CREATE TABLE` / `ADD COLUMN`
automáticamente. Si el field gana o pierde el arg después,
emite `ALTER TABLE ... ALTER COLUMN ... SET/DROP DEFAULT`.

```fitz
@table("events") type Event {
    @primary id: Int = 0
    title: Str = ""

    // Marker-only — el ORM skipea el INSERT, pero la migration
    // NO mete `DEFAULT NOW()` automático. El user lo agrega
    // a mano si lo quiere.
    @db_default created_at: Str = ""

    // Con arg SQL — la migration emite `updated_at timestamp
    // with time zone NOT NULL DEFAULT NOW()` automáticamente.
    @db_default("NOW()") updated_at: Str = ""

    // Otras expresiones válidas: UUID auto-generado, contadores,
    // strings literales, etc. Es SQL literal, no validado por Fitz.
    @db_default("gen_random_uuid()") tracking_id: Str = ""
}
```

El diff es **idempotente**: re-correr `fitz db diff` tras
aplicar la migration no propone cambios. La normalización
maneja:

- **Case-insensitive de função calls**: `NOW()` ↔ `now()` ↔
  `Now()` matchean (Postgres devuelve `now()` lowercase desde
  `information_schema.column_default`).
- **Strip de casts redundantes**: `'public'::text` ↔ `'public'`
  matchean (PG agrega casts automáticos a literales Str).
- **Trim whitespace**.

NO intentamos evaluar expresiones equivalentes — `now()` y
`CURRENT_TIMESTAMP` son ambos válidos para `timestamptz` pero
los tratamos como distintos. Convención: elegí UNO en tu schema
y manteneté consistente.

### Limitaciones explícitas del MVP

- **Renames solo via `@renamed_from`** (v0.10.17): si NO ponés
  el decorator, un rename `name → full_name` se ve como `DROP +
  ADD` perdiendo datos. El decorator hace el rename seguro;
  ver sub-sección "Renames seguros" arriba.
- **`ALTER COLUMN ... TYPE` sin USING**: cambios de tipo
  incompatibles (`text → int`) fallan. Editá la migration para
  agregar `USING (col::int)` o data migration script.
- **Solo schema `public`**: futuras schemas custom requieren
  refinamiento del introspector.
- **Rollback de N>1 NO es atómico** (v0.10.17): cada
  `revert_migration` corre en su propia tx. Para "todo o nada"
  sobre N migrations, escribí una migration única con todo el
  rollback adentro.

### Por qué Fitz hace esto distinto

- **Cero deps externas**: ni `pip install alembic` ni `npm
  install typeorm`. Todo en el binario `fitz`.
- **Schema desde el código tipado**: la fuente de verdad es el
  `@table type User { ... }` del lenguaje, no un YAML aparte
  ni reflection runtime. Si el type cambia, el diff lo detecta.
- **Paridad bit-a-bit con el resto del stack**: usa el mismo
  driver Postgres puro que el ORM (`db::introspect_schema`
  habla wire protocol v3.0 vía `information_schema` + `pg_catalog`).
- **Idempotente por diseño**: el tracking en `_fitz_migrations`
  es estándar; re-correr `migrate` o `diff` es siempre seguro.

---

## 27. Performance

Hay un benchmark publicable contra SQLAlchemy en
[`benchmarks/orm-vs-sqlalchemy/`](../benchmarks/orm-vs-sqlalchemy/):
3 endpoints idénticos (`GET /users`, `GET /users/{id}`, `POST /users`)
sobre dos boilerplates equivalentes (`api-postgres-fitz` con ORM
nativo vs `api-postgres-python` con SQLAlchemy interop). Headline
(corrida v0.10.13, hardware Intel Core Ultra 7 + 64GB + Docker WSL2):

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| Memory peak | **9.2 MB** | 51.0 MB | **5.54x más eficiente** |
| `GET /users` p50 | **4.88 ms** | 37.85 ms | **7.76x** |
| `GET /users/{id}` p50 | **3.60 ms** | 31.87 ms | **8.85x** |
| `GET /users/{id}` RPS | **2604** | 296 | **8.80x** |

Cold start 0.14s vs 0.22s. Image 131MB vs 258MB. POST sequential
queda en empate (~108ms p50 ambos) — bottleneck del bench mismo
(curl loop con subshell overhead), no del server.

Las observaciones conceptuales abajo explican por qué Fitz tiende
a ganar. Para reproducir el bench: `bash benchmarks/orm-vs-sqlalchemy/run.sh`.



### SQL constante en codegen-time

Cada `.where(closure)` se walka del AST DURANTE EL CODEGEN, el
fragmento SQL queda hard-coded en el binario. **Zero overhead
runtime para construir SQL**:

```fitz
User.where(fn(u) => u.age > 18 and u.role == "admin").all(db).await
```

Emite Rust equivalente a:

```rust
// Pseudocódigo del codegen
let qb = __FitzQueryBuilder::<UserData>::new(
    "users",
    "\"id\", \"email\", \"name\", \"age\", \"role\""
);
let qb = qb.with_where(
    "(\"age\" > $1 AND \"role\" = $2)",
    vec![into_pg(18), into_pg("admin")]
);
qb.all(&db).await
```

El fragmento `"(\"age\" > $1 AND \"role\" = $2)"` es un string
literal embebido en el binario. Compare con SQLAlchemy 2.x:

```python
# SQLAlchemy: cada call construye un AST de objetos que se
# evalúa a SQL en runtime.
session.execute(
    select(User).where(
        (User.age > 18) & (User.role == "admin")
    )
).all()
```

Cada `select(...)`, `where(...)`, `&`, etc. construyen objetos
Python en memoria. Compile-time vs runtime construction.

### Pool de conexiones automático

`db.connect(url)` levanta un pool interno (10 conns por default).
Reconnect automático si una conn muere. Health check con
`Weak<DbPool>` para auto-cleanup.

Sin configuración por parte del user. Para apps que necesitan
ajustar tamaño del pool, queda como mini-fase futura
(`db.connect_with(url, pool_size=20)`).

### Driver puro vs libpq

El driver Fitz habla wire protocol v3.0 directamente. Comparable
en perf a `tokio-postgres`/`sqlx-postgres` (también puros en Rust)
y a `pgx` de Go. Mejor que drivers que pasan por libpq (libpq
agrega un layer de copy + GIL en bindings Python).

### Eager loading: 2 queries vs N+1

```fitz
let users = User.preload("posts").all(db).await?
// 2 queries totales: SELECT * FROM users, luego SELECT * FROM posts WHERE user_id IN (...)
```

vs sin preload con 100 users → 101 queries. Diferencia de orden de
magnitud típicamente.

### Optimizaciones cerradas que impactaron el bench

- **B-1 (v0.10.13)** — Driver Postgres `Extended Query Protocol`
  batchea los 5 mensajes (Parse/Bind/Describe/Execute/Sync) en un
  único `write_all_bytes(...)` + `TCP_NODELAY` activo. `GET
  /users/{id}` pasó de **43.70ms p50 → 3.60ms p50** (12x más
  rápido, de "30% más lento que Python" a "8.85x más rápido").
- **Pool singleton per URL (v0.10.9)** — `db.connect(url)` cachea
  el `Arc<DbConnHandle>` global; calls subsiguientes a la misma
  URL devuelven `Arc::clone()` en vez de crear pool nuevo (fix
  connection leak crítico que rompía boilerplates bajo carga
  sostenida).
- **`__to_fitz_json` deadlock fix (v0.10.10)** — el conditional
  emit del field has_many virtual hacía re-lock del mismo Mutex
  no-reentrante. Liberar el guard antes del `__to_fitz_json`
  recursivo destrabó el preload hang.

### Próximas extensiones del bench (no bloqueantes)

- Mixed workload realista (reads + writes intercalados con ratio
  típico OLTP 80/20).
- Bulk inserts (1k+ rows en una transaction).
- Queries con JOINs / preload eager loading (necesita
  `api-orm-full` como bench target).
- Escritura concurrente con saturación del pool.

Quedan como `run-extended.sh` cuando aparezca demanda real para
publicar comparativa más amplia.
- Memory usage bajo carga sostenida.

Detalle en `docs/roadmap.md` → "Plan boilerplates ORM/DB
post-Fase 10".

---

## 28. Limitaciones honestas y deuda explícita

Lo que NO está en el MVP, con plan de cierre y workaround
recomendado:

### Migraciones automáticas — **CERRADO en v0.10.16**

`fitz db diff/migrate/status/new` ya están en el binario. Ver
sección 26.c para el workflow canónico (incluyendo
`@db_default("NOW()")` que emite defaults SQL automáticamente
desde v0.10.16) y las limitaciones del MVP (no detecta renames,
no down migrations).

### Transactions — **CERRADO en v0.10.14 (fn nombrada) + v0.10.15 (FnExpr inline)**

`db.transaction(fn(tx) -> Result<T> { ... })` con
auto-rollback en `Err` y captures del scope outer. Ver
sección 26.b. Deuda futura (NO bloquea uso real): nested
transactions con SAVEPOINT, niveles de aislamiento custom,
read-only transactions.

### Composite primary keys — **CERRADO v0.10.27**

N `@primary` fields por type. Migrations emite `PRIMARY KEY (a, b)`
constraint table-level; SELECT/INSERT/UPDATE/DELETE respetan
composite. Sentinel `id: 0` auto-bigserial NO aplica (el user
provee los N valores explícitos del PK tuple). Navigation
belongs_to/has_many requiere single PK del target.

```fitz
@table("memberships") type Membership {
    @primary org_id: Int = 0
    @primary user_id: Int = 0
    role: Str = ""
}
```

### TLS strict (`sslmode=require` / `verify-ca` / `verify-full`)

- **Status**: MVP soporta solo `sslmode=disable`. TLS llega en
  sub-paso 10.1.b (StartTLS + cert validation).
- **Workaround**: para Postgres detrás de un proxy (e.g. PgBouncer
  + nginx con TLS termination), apuntar Fitz al endpoint sin TLS
  interno. Para conexiones a managed DB (Heroku, RDS, Supabase)
  que exigen TLS, esperar 10.1.b.

### Date / Time / Timestamp / UUID nativos

- **Status**: se modelan como `Str` ISO 8601 / formato canonical
  UUID. El driver hace round-trip correctamente.
- **Workaround**: el user maneja parsing/formatting del lado Fitz.
- **Cuándo**: mini-fase aparte. Decisión: implementar como tipos
  built-in `Date`/`DateTime`/`UUID` con métodos.

### JSON operators avanzados

Los operadores principales (`?`, `?&`, `?|`, `@>`, `->>`) están
disponibles como method calls sobre fields jsonb (ver sección 13).
Lo que queda como deuda menor:

- `.has_all_keys/has_any_keys/contains_json` requieren args
  literales (no vars del scope outer).
- `.contains_json({...})` solo acepta values primitivos (no Maps
  anidados).
- `.get(key)` devuelve text — comparación contra Int requiere
  cast crudo (`db.query(...)`) o helper futuro (`.get_int(key)`).
- Nested path access (`e.data.get("a").get("b")`) no soportado;
  workaround con `db.query` crudo.
- **Operadores faltantes**: `@@` (text search), `#>` / `#>>`
  (path access estructurado), `||` (concat jsonb).

### Refinamientos pendientes del query builder

- **Composite / partial / unique indexes via `@index(...)`** —
  **CERRADO v0.10.27**. Decorator `@index(field1, field2, ...,
  unique=true, name="...", where_=...)` declarado al type emite
  `CREATE INDEX` auto desde `fitz db diff/migrate`. Soporta
  composite (multi-col), unique, partial (`WHERE clause`), y
  override del nombre. Expression indexes (sobre `func(col)`)
  quedan como deuda menor.
- **Bulk insert eficiente** — **CERRADO v0.10.27**.
  `Type.bulk_insert([rows], db, batch_size=1000)` emite multi-row
  VALUES en batches configurables. Paridad bit-a-bit
  `fitz run` ↔ `fitz build`. Sentinel `id: 0` auto-bigserial
  detectado de la PRIMERA row (asume shape uniforme — todas con
  o todas sin PK explícita). Para composite PK los valores van
  explícitos.
- **`db.copy_in(...)` para inserts masivos**: Postgres
  `COPY FROM STDIN` (millones de rows en segundos) no está en el
  driver. Para datasets <100k rows, `bulk_insert` cubre el caso.
  Mini-fase aparte si entra presión real para datasets grandes
  (cientos de miles +).
- **`fitz db inspect` / introspection del schema real**: no
  existe. Probablemente entra junto con `fitz db diff/migrate`.

### Refinamientos menores del sistema de tipos / codegen

- **Nullable refinement en patterns complejos**: `match obj
  { null => x, u => u.field }` con `obj: T?` refina `u` a `T`
  desde v0.10.6. Refinement aplica solo a `Pattern::Ident`
  directo — Tuples / OkBinding / ErrBinding sobre Nullable
  quedan como deuda menor.
- **Escape runtime en LIKE patterns con vars**: `.starts_with(
  var)` no escapa `%`/`_` del input runtime. Si la var tiene
  esos caracteres, se interpretan como wildcards SQL.
  Consistente con `.like(var)` — el user controla el pattern.

---

## 29. CLI con DB: cómo cada subcomando interactúa

Desde v0.10.16, Fitz tiene un subcomando dedicado `fitz db ...`
con cuatro acciones (`diff` / `migrate` / `status` / `new`) — ver
sección 26.c para el workflow canónico y limitaciones del MVP.
El resto de los subcomandos generales del CLI también funcionan
naturalmente con programas que usan el módulo `db` y el ORM.

### `fitz db diff/migrate/status/new/rollback/check/stamp/history/squash` — workflow de migraciones

```bash
export DATABASE_URL="postgres://fitz:fitz@localhost/myapp"
fitz db new add_email_to_users          # crea migration vacía con UP/DOWN
fitz db diff > migrations/<file>.sql    # genera ALTER TABLE auto
fitz db migrate                          # aplica + tracking idempotente
fitz db migrate --sql                    # v0.10.20: emite SQL offline (DBA)
fitz db status                           # lista applied/pending
fitz db history                          # v0.10.20: audit log applied_at DESC
fitz db rollback                         # revierte el último (--count N)
fitz db check                            # CI: exit 0 sync / exit 1 drift
fitz db stamp 20260530000000             # adopt DB legacy: marca sin ejecutar
fitz db stamp --all                      # marca todas las pending
fitz db squash <from> <to>               # v0.10.20: combina migrations en una
```

Detalle completo en sección 26.c.

### `fitz run [archivo]` — con DB

```bash
export DATABASE_URL="postgres://postgres:postgres@localhost/demo?sslmode=disable"
fitz run examples/guide/31-orm.fitz
```

Comportamiento:

- El intérprete carga el módulo `db` built-in al boot (sin
  import explícito necesario).
- `db.connect(url)` levanta el pool de conexiones lazy.
- Todas las queries van contra Postgres real — sin paridad fake,
  el evaluator habla wire protocol v3.0 igual que el binario.
- Si `connect` falla (URL inválida, server down, password
  wrong, sslmode=require sin TLS support), el programa termina
  con `Result::Err` propagado al top-level.
- Hot reload via `fitz dev` (ver abajo) re-arranca el proceso
  conservando el state de la DB.

### `fitz build [archivo]` — con DB

```bash
export DATABASE_URL="postgres://prod-host:5432/myapp?sslmode=disable"
fitz build src/main.fitz
./main
```

Comportamiento:

- El codegen detecta el uso de `db.connect`/`db.query`/`db.exec`
  + cualquier llamada a métodos del ORM (`Type.all`/`.where`/
  etc.) y enciende los flags `uses_db = true`.
- El `Cargo.toml` emitido NO suma deps externas — el driver
  Postgres está embebido en `src/db.rs` que se copia al
  preludio del crate generado. Cero `tokio-postgres` /
  `sqlx` / `diesel` en el binario.
- El binario nativo (`~5-10 MB` standalone) habla wire protocol
  v3.0 directamente — corre en cualquier host que tenga
  Postgres accesible por TCP, sin requerir libpq instalado.
- Paridad bit-a-bit con `fitz run`: los outputs de cada query
  son idénticos (validados en CI con job `db-postgres` +
  service container `postgres:16`).

### `fitz check [archivo]` — con DB

```bash
fitz check src/main.fitz
```

Comportamiento:

- **NO se conecta a Postgres**. El checker valida estáticamente
  sin tocar la DB.
- Valida los decoradores ORM (`@table` con string literal,
  `@primary` único, `@belongs_to("X")` con X siendo un type
  declarado, kwargs de relations con valores válidos).
- Valida el shape de los closures de `.where(...)`/`.order_by(...)`/
  `.preload(...)` contra los fields del type.
- Refina los tipos de las chain methods: `User.where(...)` →
  `QueryBuilder<User>`, `.first(db).await?` → `User`,
  `.group_by(...)` → `Aggregated<User>`, etc.
- Detecta typos en `.preload("post")` vs `"posts"` cuando el
  type tiene `@has_many` declarado.
- Detecta missing `.where(...)` antes de `.update(...)`/
  `.delete(...)` (compile-time error).
- Sin conexión real → no detecta drift entre el shape declarado
  en `type` y el shape real en Postgres (eso requiere
  `fitz db inspect` futuro).

### `fitz openapi <archivo>` — con DB

```bash
fitz openapi src/main.fitz > openapi.json
```

Comportamiento:

- **NO se conecta a Postgres**. Como `fitz check`, es read-only
  sobre el AST.
- Los handlers HTTP que llaman al ORM aparecen en el schema con
  los tipos refinados (e.g. response `200` typed como `List<User>`
  si el handler retorna `Result<List<User>>` desde
  `User.all(db).await`).
- Operaciones que usan `.group_by(...)` retornando `List<Map<Str,
  Any>>` aparecen con schema `additionalProperties: true` (Map
  free-form).

### `fitz test [filter]` — con DB

```bash
export FITZ_TEST_PG_URL="postgres://postgres:postgres@localhost/fitz_test?sslmode=disable"
fitz test  # corre todos los @test del proyecto
fitz test integration  # filter por substring del nombre
```

Comportamiento:

- Los `@test fn` que usan DB siguen las mismas reglas que
  cualquier test: ejecutados serializados (per default), output
  cargo-style.
- Tu test escribe sus fixtures explícitas — el ORM no tiene
  "test factories" built-in. Pattern típico:
  ```fitz
  @test
  async fn test_user_creation() {
      let db = db.connect(env_or("FITZ_TEST_PG_URL", "...")).await?
      db.exec("DROP TABLE IF EXISTS users", []).await?
      db.exec("CREATE TABLE users (id bigserial PRIMARY KEY, email text)", []).await?

      let u = User.insert(db, User { id: 0, email: "ada@x.com" }).await?
      let count = User.count(db).await?
      assert_eq(count, 1)
  }
  ```
- ⚠️ **No hay transaction rollback automático entre tests**
  (deuda residual de transactions). Para isolation real entre
  tests del mismo módulo, drop+recreate la tabla al inicio de
  cada test, o usar prefijos únicos por test (e.g.
  `users_test_xxx`).

### `fitz dev [--file]` — con DB

```bash
export DATABASE_URL="postgres://postgres:postgres@localhost/demo?sslmode=disable"
fitz dev --file src/main.fitz
```

Comportamiento:

- Watcher de archivos: cuando un `.fitz` cambia, el child
  process (tu programa) se mata y se respawnea.
- **El pool de conexiones se cierra y se re-abre** en cada
  reload (no hay continuidad de conexión a través de
  respawns).
- Postgres mantiene los datos entre reloads (es estado en disk,
  no en memoria del binario).
- Útil para iterar handlers HTTP que tocan DB: editás el handler,
  Ctrl+S, y el server se levanta de nuevo en ~1-2s con los datos
  intactos.

### `fitz repl` — con DB

```bash
fitz repl
```

Multi-line input + env persistente entre líneas:

```
fitz> let db = db.connect("postgres://postgres:postgres@localhost/demo?sslmode=disable").await?
fitz> @table("users") type User { @primary id: Int = 0, email: Str }
fitz> let users = User.all(db).await?
fitz> for u in users { print(u.email) }
fitz> :type users
   users : List<User>
```

Comportamiento:

- El `db` definido persiste entre líneas — no hay que reconectar
  cada query.
- `:type <expr>` muestra el tipo refinado del ORM (e.g.
  `User.where(...)` → `QueryBuilder<User>`).
- `:env` lista las vars definidas, incluyendo `db: DbConn` si
  se conectó.
- Útil para explorar una DB existente: definir el `type` con
  `@table`, hacer queries ad-hoc, validar shapes antes de
  meter el código al `src/main.fitz`.

### `fitz fmt [archivos]` — sin interacción con DB

El formatter es puramente sintáctico — no toca el módulo `db`
de manera especial. Los closures de `.where(...)` se formatean
como cualquier otra expresión.

### `fitz lint [archivos]` — sin interacción con DB

Mismo caso: el linter detecta `unused_variable`/`unused_import`/
etc. independiente de si el programa usa DB.

### Subcomandos planeados (NO implementados todavía)

| Subcomando | Función | Status |
|------------|---------|--------|
| `fitz db diff [--from-snapshot]` | Compara el shape de los `type` Fitz con `@table` contra el schema real Postgres + emite el diff DDL. | Roadmap Fase 10.6+ |
| `fitz db migrate [--up/--down]` | Aplica/revierte migrations versionadas. Lee `migrations/NNNN_*.sql` o auto-generadas del diff. | Roadmap Fase 10.6+ |
| `fitz db inspect [--table=X]` | Imprime el schema real de Postgres en formato `type` Fitz, útil para introspeccionar DBs existentes. | Roadmap Fase 10.6+ |
| `fitz db seed [<file>]` | Carga fixtures desde archivos JSON/SQL. | Refinamiento futuro |
| `fitz db console` | Wrapper de `psql` con el `DATABASE_URL` del manifest. | Refinamiento menor |

Detalle planeado en `docs/roadmap.md` → "Fase 10.6+: workflows
DB".

---

## 30. Ejemplos runnable y boilerplates

Dos ejemplos en `examples/guide/` cubren los casos canónicos:

### `examples/guide/31-orm.fitz` (pedagógico, ~100 LoC)

Muestra el **shape canónico del ORM end-to-end**: `@table` con
todos los decoradores, insert, where + first, chain
`order_by`/`limit`/`offset`, operadores extendidos
(`starts_with`/`is_in`/`between`), aggregates scalar
(`count`/`avg`), GROUP BY con `Aggregated<Row>`, navigation
belongs_to/has_many, eager loading con `.preload`, y update/delete
con guard. `fitz build` produce binario que NO requiere Postgres
real al compilar; el `connect` runtime falla con `Err` clara si
la URL es inválida.

### `examples/guide/31b-orm-crud-http.fitz` (CRUD HTTP end-to-end, ~135 LoC)

Combina **todo el stack Fitz**: types `User`/`Post` con
decoradores ORM completos, HTTP nativo (`@get`/`@post`/`@put`/
`@delete` + path params), body deserialization con types custom
dedicados (`UserInput`/`PostInput`), `Result<T>` con `?`,
`env_or(...)` para leer `DATABASE_URL`, y `@server(port)`.
Endpoints: list/get/create/update/delete sobre users, relation
queries, eager loading con `.preload(...)`, aggregate scalar.
Requiere Postgres real para correr; compila con `fitz build`
aunque no haya DB local.

### Boilerplates Dockerizados (post-v0.10.2, planeados)

Dos boilerplates Dockerizados cerrarán el ciclo demostrando el
ORM en proyectos productivos:

- **Boilerplate 6 convertido**: el actual `api-postgres-python`
  (SQLAlchemy + Postgres) se reescribe a Fitz ORM puro sobre el
  mismo dominio `tasks`. LoC counts antes/después + benchmarks
  side-by-side.
- **Boilerplate 7 nuevo dedicado al ORM full**: dominio rico
  (blog/CMS o e-commerce básico) con relations cross-table,
  JSONB, arrays, aggregates + GROUP BY, operadores extendidos,
  auth nativa + ORM, WebSockets + ORM (notificaciones realtime),
  cron jobs (limpieza de drafts). Benchmarks `wrk`/`oha`
  comprometidos vs SQLAlchemy.

Plan detallado en la memoria de proyecto + `docs/roadmap.md` →
"Plan boilerplates ORM/DB post-Fase 10".

---

## Cierre

Este documento es **referencia viva**. Cada release del proyecto
que toque el ORM (refinamiento del translator, nuevos
operadores, codegen de algún caso que faltaba) actualiza la
sección correspondiente con el patrón canonical + cualquier
deuda residual derivada.

**Roadmap del documento**:

- v0.10.2 — este doc creado (todas las secciones cubren MVP de
  Fase 10/10.b).
- Próximo refresh — cuando llegue Fase 10.6 (migraciones
  automáticas) o 10.7 (transactions), las secciones 28
  (limitaciones) marcan deuda como CERRADA y suman las nuevas
  recetas / API.
- Cuando aparezcan benchmarks reales del boilerplate 7, la
  sección 27 (Performance) deja el placeholder y suma números
  concretos vs SQLAlchemy.
- **2026-05-29 (v0.10.13)** — primera corrida publicable del
  bench MVP `api-postgres-fitz` vs `api-postgres-python`. Headline:
  Fitz ORM es **5-10x más rápido** y **5.5x más eficiente en
  memoria** que SQLAlchemy en read workloads sustained. Detalle
  completo en [Benchmarks](benchmarks.md).

Para tirar dudas / proponer recetas nuevas / reportar gaps del
ORM, abrir un issue en
[GitHub](https://github.com/Thegreekman76/fitz/issues).

— [Volver al inicio](#db-y-orm--gu%C3%ADa-exhaustiva)
