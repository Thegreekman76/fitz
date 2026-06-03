# M6.C2 — `@table`, `@primary` y lecturas tipadas con el ORM

**Pre-requisitos**: [M6.C1 — Setup Postgres + driver crudo](c1-setup-driver-crudo.md).
Tenés Postgres corriendo, sabés abrir conn con `db.connect` y
hacer `db.exec`/`db.query` crudos.

**Objetivo**: subir un nivel de abstracción. Declarar el mapping
`type` Fitz ↔ tabla Postgres con **3 decoradores** (`@table`,
`@primary`, `@column`), y reemplazar `db.query("SELECT ...
WHERE")` por **expresiones tipadas** que el checker valida en
compile-time: `User.where(fn(u) => u.age > 18).all(db).await?`.

**Por qué importa**: el SQL crudo del cap anterior funciona pero
es **stringly typed** — typos en el nombre de columna recién
salen como `Err("column 'X' does not exist")` cuando llega la
query al server. Con el ORM declarativo, `u.aje` (typo) NO
compila. Y el SQL no se construye en runtime — el codegen lo
emite **constante en compile-time**. Performance comparable a
Diesel/sqlx, mejor que SQLAlchemy/ActiveRecord/Hibernate.

**Cross-link**: [DB y ORM § 4-5](../../db-orm.md#4-table-primary-column-declarar-el-mapping).

---

## Mapa del cap

```mermaid
flowchart LR
    A["@table users type User"] --> B[checker valida fields contra type]
    B --> C[codegen emite SQL constante]
    C --> D["User.all db .await"]
    C --> E["User.where closure.first db"]
    C --> F["User.where closure.count db"]
    G["@primary id"] --> H[bigserial auto-asigna]
    I["@column name"] --> J["mapea snake_case ↔ camelCase"]
```

---

## Por qué Fitz es distinto

| Feature | SQLAlchemy 2.x | Diesel (Rust) | TypeORM (Node) | Prisma | **Fitz ORM** |
|---|---|---|---|---|---|
| Setup deps | `pip install sqlalchemy` | `cargo add diesel` + features | `npm install typeorm` + driver | `npm install prisma` + `prisma generate` | **builtin** |
| Mapping tipo ↔ tabla | `class User(Base)` + reflection | `derive(Queryable)` + `table!` macro | decoradores TS runtime | schema file `.prisma` separado | **`@table` en `type`** |
| Schema separado del código | ❌ inline | ❌ inline | ❌ inline | ✅ `.prisma` aparte | ❌ **inline en `type`** |
| `prisma generate` antes de cada build | ❌ | ❌ | ❌ | ✅ obligatorio | ❌ **no** |
| Typos en column name caen en runtime | ✅ típicamente | ❌ compile-time (macro) | ✅ runtime | ⚠ runtime para queries dinámicas | ❌ **compile-time** |
| SQL constante (no construido en runtime) | ❌ objetos runtime | ✅ macro | ❌ objetos runtime | ⚠ depende del engine | ✅ **codegen-time** |
| Compila a binario standalone | ❌ Python | ✅ con deps | ⚠ pkg hack | ❌ requiere Node + engine | ✅ **`fitz build`** |

El diferencial mayor: **el checker estático y el codegen son uno**.
Cuando escribís `User.where(fn(u) => u.aje > 18)`:

1. El checker resuelve `User` al `type` con `@table`.
2. Walka el closure y exige que cada `u.<field>` exista en
   `User`. Typo `aje` → error en compile-time, no en runtime.
3. El codegen DURANTE EL BUILD traduce el closure al fragmento
   SQL `"age" > $1` y lo deja hard-coded en el binario. Cero
   overhead runtime para construir SQL.

Diesel hace esto con macros derive + macro `table!`. Fitz lo
hace **sin macros** — son decoradores nativos del lenguaje.

---

## Paso 1 — `@table("nombre")`: mapear un `type` a una tabla

```fitz
@table("users") type User {
    @primary id: Int = 0
    name: Str
    email: Str
    age: Int
}
```

Detalles:

- **`@table("nombre")`** sobre un `type`. El arg es la tabla
  Postgres real. Convención: lowercase + snake_case + plural
  (`users`, `blog_posts`, `order_line_items`).
- **El nombre del `type` Fitz** es libre (`User`, `BlogPost`,
  `OrderLineItem`). Típico: PascalCase singular.
- **No hace falta archivo schema separado**. La definición Fitz
  ES el schema. Vs Prisma que exige un `.prisma` aparte +
  `prisma generate` antes de cada build.

**Schemas Postgres custom** (multi-tenant, separación dev/test):

```fitz
@table("analytics.events") type Event {
    @primary id: Int = 0
    name: Str
}
```

El qualified name `schema.tabla` se respeta en TODAS las
queries del ORM. Si el `.` no aparece, schema = `public`.

---

## Paso 2 — `@primary`: la clave primaria

```fitz
@table("users") type User {
    @primary id: Int = 0    // bigserial PRIMARY KEY (auto-asigna)
    email: Str
}
```

Reglas:

- **Uno o varios `@primary`** por `type`. El más común es uno
  solo.
- **`@primary id: Int = 0`** es el patrón canónico — el `= 0`
  funciona como **sentinel** para que el `INSERT` deje que
  Postgres asigne con `bigserial`. El runtime detecta el `0` y
  omite el field de la query INSERT.
- **Tipo `Str` para UUID**: el cliente genera el valor
  (`Uuid.v4().to_str()` o similar).

```fitz
@table("sessions") type Session {
    @primary token: Str    // UUID generado por el cliente
    user_id: Int
}
```

### Composite primary key (varios `@primary`)

Para tablas join (membership, junction tables):

```fitz
@table("memberships") type Membership {
    @primary org_id: Int = 0
    @primary user_id: Int = 0
    role: Str = "member"
}

// Insert con valores explícitos del PK tuple (NO bigserial).
let m = Membership { org_id: 1, user_id: 42, role: "admin" }
let _ = Membership.insert(db, m).await?
```

Notas:

- El sentinel `id: 0 → bigserial` **NO aplica** en composite —
  los valores los pasás vos.
- `@belongs_to` apuntando a un type con composite PK → error
  del checker explícito. Las relations (cap C4) requieren
  single PK en el target.

---

## Paso 3 — `@column(name="...")`: mapear nombre Fitz ↔ Postgres

A veces el nombre del field Fitz difiere del nombre de la
columna real (camelCase vs snake_case, naming legacy):

```fitz
@table("orders") type Order {
    @primary id: Int = 0
    @column(name="customer_id") customer: Int
    @column(name="created_at") created: Str    // ISO 8601
    total_amount: Float    // mismo nombre, sin @column
}
```

Detalles:

- **Kwargs van con `=`** (no `:`).
- **Solo `name=` está soportado por ahora**. Otros (`sql_type=`,
  override del tipo Postgres) quedan como mini-fase futura.
- **Sin `@column`**, el ORM usa el nombre del field tal cual.
  Vs Django/Rails que pluralizan/snake-casean por convención —
  Fitz no inventa nombres.

---

## Paso 4 — Mapping default Fitz → Postgres

Cuando creás la tabla con `CREATE TABLE`, los tipos Fitz se
mapean así:

| Tipo Fitz | Columna Postgres |
|---|---|
| `Int` | `bigint` (o `bigserial` si es `@primary id: Int = 0`) |
| `Float` | `double precision` |
| `Str` | `text` |
| `Bool` | `boolean` |
| `Str?` | `text NULL` |
| `Int?` | `bigint NULL` |
| `List<Int>` | `bigint[]` |
| `List<Str>` | `text[]` |
| `List<Float>` | `double precision[]` |
| `Map<Str, Any>` | `jsonb` |
| `Map<Str, Int>` (T concreto) | `jsonb` |
| `Date` | `date` |
| `DateTime` | `timestamptz` (siempre UTC) |
| `Uuid` | `uuid` |

`Date`/`DateTime`/`Uuid` son tipos built-in del lenguaje (no
strings ISO 8601 envueltos) — los vemos en M6.C5.

**Importante**: el ORM **no crea la tabla automáticamente** en
MVP. Vos corrés `CREATE TABLE` con `db.exec(...)` al boot
(patrón idempotente del cap C1) o `fitz db diff`/`migrate` (
features avanzadas, no cubiertas en M6).

---

## Paso 5 — `Type.all(db)`: traer todo

El método estático más simple. Trae todas las rows del table
como `List<Type>`:

```fitz
@table("users") type User {
    @primary id: Int = 0
    name: Str
    age: Int
}

async fn main() -> Result<Str> {
    let db = db.connect("postgres://...").await?

    let users: List<User> = User.all(db).await?
    for u in users {
        print("{u.id}: {u.name} ({u.age})")
    }

    return Ok("OK")
}
```

Detalles:

- **Tipo de retorno**: `Future<Result<List<User>>>` — uso con
  `.await?`.
- **Cada elemento es un `User`** tipado, no un Map opaco. Acceso
  via `u.<field>` directo con autocomplete del LSP.
- **SQL emitido (en compile-time, hard-coded en el binario)**:
  ```sql
  SELECT "id", "name", "age" FROM users
  ```

**Cuándo NO usar `.all()`**: tablas grandes. Sin `WHERE` ni
`LIMIT` traés TODO. Para `User` con 10k+ rows, usá `.where(...).limit(...)`
(cap C3 paginación).

---

## Paso 6 — `Type.where(closure)`: empezar un QueryBuilder

`.where(...)` toma un **closure** `fn(row) => Bool` que el
codegen traduce a un `WHERE` SQL durante el build:

```fitz
let adults = User.where(fn(u) => u.age >= 18).all(db).await?
```

**SQL emitido en compile-time**:

```sql
SELECT "id", "name", "age" FROM users WHERE "age" >= $1
```

El `$1` se bindea a `18` en runtime. Cero overhead — el SQL
queda hard-coded en el binario nativo.

### El checker valida los fields del closure

```fitz
let bug = User.where(fn(u) => u.aje >= 18).all(db).await?
//                              ^^^ typo "aje" en vez de "age"
```

Error del checker (compile-time):

```
Error en línea N — fn `where`: el field `aje` no existe en
`User`. Fields disponibles: `id`, `name`, `age`.
```

**No esperás a runtime**. SQLAlchemy/Hibernate/Prisma típico
sería:
- SQLAlchemy: `AttributeError: type object 'User' has no
  attribute 'aje'` cuando la query se construya.
- Prisma: `Unknown arg 'aje' in where.aje`. Runtime engine call.
- Diesel: ✅ también compile-time, pero requiere `derive(
  Queryable)` + macro `table!` separada.

### Operadores en el closure

```fitz
// Comparaciones.
User.where(fn(u) => u.age > 18)
User.where(fn(u) => u.age >= 18 and u.age < 65)
User.where(fn(u) => u.role == "admin")
User.where(fn(u) => u.email != "spam@x.com")

// Lógicos and / or / not.
User.where(fn(u) => u.age >= 18 and (u.role == "admin" or u.role == "moderator"))
User.where(fn(u) => not u.deleted)
```

**Lo que el closure NO puede hacer en MVP**:

- Llamar a otras funciones Fitz arbitrarias adentro.
- Crear instancias o hacer cálculos complejos.
- Acceder a vars cuyo tipo el checker no resuelve.

El closure tiene que ser una **expresión predicado pura** —
field access + literales + operadores. Eso permite al codegen
traducirlo determinísticamente a SQL.

### `.where(...)` NO ejecuta la query

```fitz
let qb = User.where(fn(u) => u.age >= 18)
// qb es QueryBuilder<User> — todavía no se hizo nada.

// Recién al llamar un terminal (.all/.first/.count), el SQL se
// ejecuta:
let users = qb.all(db).await?
```

Esto te deja **componer la query** antes de ejecutarla (Paso 6
del cap C3 con `.order_by` / `.limit` / `.offset`).

---

## Paso 7 — Terminales del QueryBuilder: `.first`, `.count`, `.all`

Después de `.where(...)`, tenés un `QueryBuilder<Type>`. Tres
métodos lo cierran:

### `.all(db) -> Future<Result<List<Type>>>`

Devuelve todas las rows que matchean:

```fitz
let adults = User.where(fn(u) => u.age >= 18).all(db).await?
print("{len(adults)} adults found")
```

### `.first(db) -> Future<Result<Type>>`

Devuelve el primer row. Si no hay matches → `Err`:

```fitz
let ada = User.where(fn(u) => u.email == "ada@x.com").first(db).await?
print("hola {ada.name}")
```

Para no abortar si no existe:

```fitz
match User.where(fn(u) => u.email == "ada@x.com").first(db).await {
    Ok(u) => print("hola {u.name}"),
    Err(_) => print("no encontrada"),
}
```

### `.count(db) -> Future<Result<Int>>`

Devuelve la cantidad de rows que matchean:

```fitz
let total_adults = User.where(fn(u) => u.age >= 18).count(db).await?
print("adults: {total_adults}")
```

**SQL emitido**: `SELECT COUNT(*) FROM users WHERE "age" >= $1`.

### Atajo: "todos los rows" sin where

Si querés contar TODOS los users (sin filtro), pasás un closure
trivial:

```fitz
let total = User.where(fn(u) => true).count(db).await?
```

> **Nota**: hoy `Type.count(db)` directo (sin `.where(...)`)
> NO está disponible — siempre va via el QueryBuilder. Lo mismo
> con `Type.first(db)`. Es deuda menor — para "primer row de la
> tabla", usá `User.where(fn(u) => true).first(db)`.

---

## Paso 8 — Defaults en fields y `@hidden`

### Defaults literales

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    name: Str
    role: Str = "user"        // default literal
    active: Bool = true
    metadata: Map<Str, Any> = {}    // jsonb default empty
}
```

**Comportamiento**:

- Al crear instancia con struct literal, podés omitir fields
  con default:
  ```fitz
  let u = User { id: 0, email: "ada@x.com", name: "Ada" }
  // u.role == "user", u.active == true, u.metadata == {}
  ```
- En `INSERT`, los fields con default que NO se modificaron NO
  se omiten del SQL — se mandan a Postgres con el valor del
  default Fitz. Vs Postgres column default que solo aplica si
  el field se omite del INSERT.

### `@hidden`: ocultar fields de la frontera HTTP

Caso típico: password hashes que NUNCA deben crossar HTTP.

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str = ""
    name: Str = ""
    @hidden password_hash: Str = ""    // <-- NUNCA cruza JSON
    role: Str = "user"
}
```

Qué cambia:

- **`__to_fitz_json` skipea el field**. Cuando un handler HTTP
  devuelve un `User` (directo o como part de un `Post.author`
  vía eager loading), el cliente **nunca ve** `password_hash`.
- **`__from_fitz_json` rechaza el field**. Si el cliente manda
  `{"password_hash": "..."}` en un body POST/PUT, el server
  responde 400 con `"campo no declarado"`.
- **El ORM lo persiste normal**. INSERT lo incluye, SELECT lo
  trae, `.update(...)` lo modifica.

**Cuándo usar**:

- Campos sensibles (`password_hash`, `api_key_hash`, tokens
  internos).
- Metadata privada (`internal_status`, IDs internos, etc.).

**Cuándo NO usar**:

- Para validar input de register/login, usá un type
  **separado** (`Credentials`/`RegisterInput`) — separación de
  responsabilidades es más clara que apoyarse solo en `@hidden`.

---

## Paso 9 — Programa end-to-end

```fitz
// users-demo.fitz
@table("users") type User {
    @primary id: Int = 0
    name: Str
    email: Str
    age: Int
    role: Str = "user"
}

async fn main() -> Result<Str> {
    let db = db.connect(
        env_or("DATABASE_URL", "postgres://postgres:secret@localhost:5432/fitz_curso?sslmode=disable")
    ).await?

    // Schema idempotente al boot.
    let _ = db.exec("CREATE TABLE IF NOT EXISTS users (id bigserial PRIMARY KEY, name text NOT NULL, email text NOT NULL, age bigint NOT NULL, role text NOT NULL DEFAULT 'user')", []).await?

    // Insert 3 users con bigserial auto-asignado (id: 0 sentinel).
    let _ = User.insert(db, User { id: 0, name: "Ada", email: "ada@x.com", age: 35, role: "admin" }).await?
    let _ = User.insert(db, User { id: 0, name: "Alan", email: "alan@x.com", age: 28, role: "user" }).await?
    let _ = User.insert(db, User { id: 0, name: "Edsger", email: "edsger@x.com", age: 50, role: "user" }).await?

    // Type.all(db) - TODOS.
    let all = User.all(db).await?
    print("total users: {len(all)}")

    // Type.where(closure).count(db) - cuántos cumplen filtro.
    let admins_count = User.where(fn(u) => u.role == "admin").count(db).await?
    print("admins: {admins_count}")

    // Type.where(closure).all(db) - filtrados.
    let adults = User.where(fn(u) => u.age >= 30).all(db).await?
    print("adults (age >= 30):")
    for u in adults {
        print("  {u.name} ({u.age})")
    }

    // Type.where(closure).first(db) - primer match (o Err).
    let ada = User.where(fn(u) => u.email == "ada@x.com").first(db).await?
    print("ada role: {ada.role}")

    return Ok("OK")
}

print(main().await)
```

Output esperado:

```
total users: 3
admins: 1
adults (age >= 30):
  Ada (35)
  Edsger (50)
ada role: admin
```

---

## Subset compilable a binario

| Feature | `fitz run` | `fitz build` |
|---|---|---|
| `@table("nombre")` | ✅ | ✅ |
| `@table("schema.nombre")` (schema custom) | ✅ | ✅ |
| `@primary` single | ✅ | ✅ |
| `@primary` composite (varios) | ✅ | ✅ |
| `@column(name="...")` | ✅ | ✅ |
| `@hidden` | ✅ | ✅ |
| Defaults literales en fields | ✅ | ✅ |
| `Type.insert(db, instance)` | ✅ | ✅ |
| `Type.all(db)` | ✅ | ✅ |
| `Type.where(closure).first(db)` | ✅ | ✅ |
| `Type.where(closure).count(db)` | ✅ | ✅ |
| `Type.where(closure).all(db)` | ✅ | ✅ |
| Checker valida fields del closure | ✅ | ✅ |
| `Type.count(db)` directo (sin `.where`) | ❌ usar `.where(fn(u) => true)` | ❌ |
| `Type.first(db)` directo (sin `.where`) | ❌ usar `.where(fn(u) => true)` | ❌ |
| Closures con llamadas a otras fns | ❌ | ❌ |
| Operadores `like` / `is_in` / etc. | (cap C3) | (cap C3) |

---

## Validación

- [ ] `@table("users") type User { @primary id: Int = 0, ... }`
      compila con `fitz check`.
- [ ] El `INSERT` con `id: 0` deja que Postgres asigne con
      `bigserial`.
- [ ] `User.all(db).await?` devuelve `List<User>` con cada
      elemento tipado (acceso `.name`/`.age` con autocomplete).
- [ ] `User.where(fn(u) => u.<typo>).all(db)` falla en
      `fitz check` con error citando los fields disponibles.
- [ ] `User.where(fn(u) => u.role == "admin").count(db).await?`
      devuelve `Int` con la cantidad real.
- [ ] `User.where(fn(u) => u.email == "X").first(db).await?`
      devuelve el `User` o `Err` si no existe.
- [ ] `@column(name="customer_id") customer: Int` emite SQL con
      la columna `customer_id`, no `customer`.
- [ ] `@hidden password_hash: Str` skipea el field en JSON I/O
      pero lo persiste en la DB.
- [ ] `fitz build` del programa de demo produce binario
      standalone que conecta a Postgres y corre el flow.

---

## Troubleshooting

### `Error en línea N — el field `X` no existe en `Y``

Typo en el closure de `.where(...)`. El checker compara contra
los fields declarados en el `type` con `@table`. Verificá la
declaración del `type`.

### `Err("column 'X' does not exist")` en runtime

El SQL emitido nombra una columna que no existe en la tabla
real. Causas comunes:

1. El `type` Fitz no matchea con la tabla real (alguien hizo
   `ALTER TABLE ... DROP COLUMN`).
2. Olvidaste `@column(name="...")` cuando el field Fitz difiere
   del nombre real (`customer` field → columna `customer_id`).
3. Fuiste por `@table("schema.tabla")` y el schema no existe.

### `Err("relation 'users' does not exist")`

La tabla no se creó. **Fitz NO crea la tabla automáticamente**.
Patrón canónico: `CREATE TABLE IF NOT EXISTS users (...)` con
`db.exec(...)` al boot del programa (cap C1).

### `Error en línea N — el tipo `User` no tiene un método estático llamado `count``

`User.count(db)` directo NO está disponible — siempre va via
`.where(...)`. Usá:

```fitz
let total = User.where(fn(u) => true).count(db).await?
```

Lo mismo aplica a `User.first(db)`. Es deuda menor del MVP.

### `Err("attempt to insert NULL into column 'X'")`

El field es NOT NULL en Postgres pero el Fitz lo envió como
`null`. Causas:

1. El field es `Str?` en Fitz pero la columna es `text NOT NULL`
   en Postgres (mismatch).
2. El field NO tiene default Fitz y vos no lo seteaste al crear
   la instancia.

**Fix**: alinear el shape Fitz con el de Postgres, o agregar
default Fitz al field.

### `Err("bigint out of range")` al hacer `User.where(fn(u) => u.id == X).first(db)`

Postgres `bigint` acepta hasta `2^63 - 1`. Si pasás un literal
mayor (caso raro), overflow. Para tablas con IDs realistas
(< billones), no debería pasar.

### `INSERT` no respeta `id: 0` como sentinel — Postgres da error de unique constraint

Estás insertando con `id: 0` pero la tabla NO tiene la columna
`id` como `bigserial`. **Fix**: revisar el `CREATE TABLE` —
debe ser `id bigserial PRIMARY KEY`, no `id bigint`.

### El `User.all(db)` devuelve ZERO rows aunque tengo data en la tabla

Verificá:

1. La URL de la conn apunta a la DB correcta (no a `postgres`
   sino a `fitz_curso`).
2. El `@table("users")` matchea con la tabla real (case
   sensitivo en Postgres).
3. La tabla está en el schema correcto. Si en Postgres está en
   `tenant_a.users` y el Fitz dice `@table("users")` (= `public.
   users`), apuntan a tablas distintas.

### `fitz build` falla con error de codegen en el closure

Causas comunes:

1. El closure llama a una fn que el codegen no puede traducir
   (`User.where(fn(u) => is_palindrome(u.name))` — fn user-
   defined).
2. El closure usa una var de scope externo que no es primitivo
   (`User.where(fn(u) => u.role == role_filter)` con `role_filter:
   Map`).

**Fix**: simplificar el predicado, o construir el filtro con
QueryBuilder dinámico (cap C3).

---

## Lo que sigue

Llegaste al final del cap. Lo que cubriste:

- `@table("nombre")` mapea un `type` a una tabla Postgres, con
  soporte para schemas custom (`@table("analytics.events")`).
- `@primary` declara la PK — single con `id: Int = 0` sentinel
  para `bigserial`, o composite con múltiples `@primary`.
- `@column(name="...")` mapea nombre Fitz ↔ columna real.
- Mapping default Fitz → Postgres (Int → `bigint`, `Map<Str,
  Any>` → `jsonb`, etc.).
- `Type.all(db)` para traer todo.
- `Type.where(closure).all/first/count(db)` para queries
  filtradas. El checker valida los fields del closure en
  compile-time; el codegen emite SQL constante hard-coded en
  el binario.
- Defaults literales en fields + `@hidden` para campos
  sensibles que no deben cruzar HTTP.

**Próximo cap**: [M6.C3 — Writes (`.insert`/`.update`/`.delete`)
+ QueryBuilder chain + agregados](c3-writes-querybuilder-agregados.md).
Vamos a ver cómo modificar data, cómo encadenar `.order_by` /
`.limit` / `.offset` / `.group_by`, y cómo usar agregados
(`.sum` / `.avg` / `.min` / `.max`) tipados.
