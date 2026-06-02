# M6.C3 — Writes, QueryBuilder chain y agregados

**Pre-requisitos**: [M6.C2 — `@table` y reads](c2-table-decoradores-reads.md).
Sabés declarar `@table` types y hacer `.all/.where/.first/.count`
tipados.

**Objetivo**: cerrar el CRUD básico — **insert**, **update**,
**delete** — y aprender a **encadenar** múltiples métodos del
QueryBuilder (`.order_by` + `.limit` + `.offset` para paginación,
`.where(...).update(db, {...})` para edits con guard, operadores
extendidos como `.between/.is_in/.like/.starts_with`). Plus
**agregados scalar** (`.sum/.avg/.min/.max`) y `.group_by(...)`
para reports.

**Por qué importa**: el 80% del trabajo en una API real es
escribir queries — paginar listados, actualizar campos de un
recurso, borrar rows con un guard correcto, agregar reports.
Saber qué operadores tiene el closure de `.where(...)` te ahorra
horas de Stack Overflow. Y entender que `.update` / `.delete`
exigen `.where(...)` previo te previene el accidente clásico
("olvidé el WHERE y borré todo").

**Cross-link**: [DB y ORM § 6-9](../../db-orm.md#6-querybuilder-reference-chain-y-terminales).

---

## Mapa del cap

```mermaid
flowchart LR
    A[Type.where closure] --> B[QueryBuilder]
    B --> C[.where filtro adicional]
    B --> D[.order_by orden]
    B --> E[.limit offset paginación]
    B --> F[.group_by Aggregated]
    B --> G[.all read]
    B --> H[.first read]
    B --> I[.count int]
    B --> J[.sum avg min max float]
    B --> K[.update changes int rows]
    B --> L[.delete int rows]
    F --> M[List Map Str Any]
    N[Type.insert instance] --> O[RETURNING * - new id]
    P[Type.bulk_insert rows] --> Q[batches multi-tuple]
```

---

## Por qué Fitz es distinto

| Feature | SQLAlchemy 2.x | Rails ActiveRecord | Diesel (Rust) | Prisma | **Fitz** |
|---|---|---|---|---|---|
| Update sin WHERE compila | ✅ peligroso | ✅ peligroso | ❌ macro requiere `filter` | ⚠ runtime check | ✅ **`.where` obligatorio static** |
| Delete sin WHERE compila | ✅ peligroso | ✅ peligroso | ❌ macro requiere `filter` | ⚠ runtime check | ✅ **`.where` obligatorio static** |
| Operators chain tipados | ⚠ con `column.like(...)` | ⚠ con `where(...)` | ✅ macros | ✅ con `prisma generate` | ✅ **closures nativos** |
| Auto-escape `%`/`_` en `starts_with` | ⚠ requiere `escape()` | ⚠ con SQL puro | ❌ manual | ⚠ con regex | ✅ **automático** |
| Var del scope externo en closure | n/a (con queries) | n/a (con queries) | ❌ macro static | ✅ con `prisma generate` | ✅ **soportado** |
| `RETURNING *` automático en `.insert` | ⚠ con `returning()` | ⚠ con `RETURNING` SQL | ⚠ con `.returning()` | ✅ | ✅ **siempre** |
| Bulk insert N rows batch | ⚠ con `bulk_save_objects` | ⚠ con `import` gem | ⚠ con `.values()` | ⚠ con `createMany` | ✅ **`.bulk_insert`** |
| GROUP BY tipado distinto del row | ⚠ runtime tuple | ⚠ runtime | ✅ macro | ⚠ runtime | ✅ **`Aggregated<Row>`** |

El diferencial mayor: **`.update`/`.delete` sin `.where(...)` es
ERROR DE COMPILACIÓN**. En SQLAlchemy / Rails podés hacer
`User.delete_all()` o `session.execute(delete(User))` sin
filtros — UNA línea borra millones de rows. En Fitz, el codegen
exige el guard estáticamente. Para borrar todo intencional:
`.where(fn(_) => true).delete(db)` (explícito).

---

## Paso 1 — `Type.insert(db, row)`: insertar un row

```fitz
let inserted: User = User.insert(db, User {
    id: 0,                     // sentinel — Postgres asigna con bigserial
    name: "Ada",
    email: "ada@x.com",
    age: 35,
    role: "admin"
}).await?
print("nuevo id: {inserted.id}")
```

Detalles:

- **Retorno: `Future<Result<User>>`** — el row **hidratado**
  con el id real (Postgres lo asigna via `bigserial`).
- **SQL emitido**: `INSERT INTO users (...) VALUES (...) RETURNING *`.
  El `RETURNING *` siempre va — por eso podés acceder al `id` y
  cualquier `DEFAULT` de la columna después del insert.
- **`id: 0` es sentinel** del Fitz — detectado en runtime, omite
  la columna del INSERT. Si pasás `id: 42` explícito, va al
  INSERT como tal (y Postgres falla si ya existe).

**Patrón canónico al crear desde HTTP**:

```fitz
type UserInput { name: Str, email: Str, age: Int }

@post("/users")
async fn create_user(input: UserInput) -> Result<User> {
    let u = User { id: 0, name: input.name, email: input.email, age: input.age, role: "user" }
    return User.insert(db, u).await
}
```

El cliente recibe el `User` con `id` asignado. Cero glue extra.

---

## Paso 2 — `Type.bulk_insert(rows, db)`: insertar N en batches

Para seeding inicial o cargas masivas:

```fitz
let rows: List<User> = []
let mut i = 0
while (i < 5000) {
    rows.push(User { id: 0, name: "user_{i}", email: "u{i}@x.com", age: 25, role: "guest" })
    i = i + 1
}

let n = User.bulk_insert(rows, db).await?
print("insertadas: {n}")    // 5000
```

Detalles:

- **Default `batch_size=1000`**. Para customizar:
  `User.bulk_insert(rows, db, batch_size: 500).await?`.
- **SQL emitido**: `INSERT INTO users (...) VALUES (a, b), (c, d), ...`
  en batches de N rows por statement. Comparable en performance
  a `COPY` para cargas medianas.
- **NO emite `RETURNING *`** — devuelve solo el conteo. Si
  necesitás los IDs auto-generados de cada row, usá `.insert` en
  loop.
- **Sentinel `id: 0` detectado de la PRIMERA row** — si la
  primera trae `id: 0`, el SQL omite la columna PK en todas las
  rows del batch. Si la primera trae `id > 0`, todas N rows
  incluyen PK explícito. Shape uniforme exigido (no mezclar).

---

## Paso 3 — `.update(db, changes)`: editar rows con guard obligatorio

```fitz
let updated_rows: Int = User.where(fn(u) => u.id == 42)
    .update(db, {"age": 36, "role": "admin"})
    .await?

print("rows actualizadas: {updated_rows}")    // 1
```

Detalles:

- **`.where(...)` previo OBLIGATORIO**. Sin él, error de
  compilación:
  ```
  ✗ archivo.fitz — 1 error(es) de tipo:
    Error en línea N — .update(...) requiere .where(...) previo.
    Para actualizar TODAS las rows, usá .where(fn(_) => true).update(...).
  ```
- **`changes` es un `Map<Str, Any>`** con `Str` keys (nombres de
  columnas) y values del tipo correspondiente. Acepta:
  - Map literal heterogéneo: `{"age": 36, "active": true}`.
  - List literal para arrays: `{"tags": ["rust", "postgres"]}`.
  - Map nested para jsonb: `{"metadata": {"draft": false}}`.
  - Var Map externa: `let changes = {"age": 40}; .update(db, changes)`.
- **Retorno**: `Future<Result<Int>>` con la **cantidad de rows
  afectadas** (no la instancia actualizada).

**Update masivo con guard explícito** (caso poco común):

```fitz
// Marcar todos los users inactivos como "deleted"
let n = User.where(fn(u) => not u.active).update(db, {"role": "deleted"}).await?
```

**"Actualizar TODAS las rows"** intencional:

```fitz
let n = User.where(fn(_) => true).update(db, {"active": false}).await?
// Explícito → compila. No se confunde con un olvido.
```

---

## Paso 4 — `.delete(db)`: borrar con guard

Mismo modelo de safety que `.update`:

```fitz
let deleted: Int = User.where(fn(u) => u.id == 42).delete(db).await?
print("borradas: {deleted}")    // 1

// ❌ Sin guard — error de codegen
let oops = User.delete(db).await?
//   ↑ error: .delete() requiere .where(...) previo.
```

Para borrar todo:

```fitz
let n = User.where(fn(_) => true).delete(db).await?
```

O preferido (más rápido + resetea `bigserial`):

```fitz
let _ = db.exec("TRUNCATE TABLE users RESTART IDENTITY", []).await?
```

---

## Paso 5 — Encadenar chain methods

El `QueryBuilder<Row>` que devuelve `.where(...)` es
**inmutable** — cada chain method retorna un nuevo builder.
Compone fluido:

```fitz
let top_admins = User
    .where(fn(u) => u.role == "admin")
    .where(fn(u) => u.age >= 30)               // suma AND al WHERE
    .order_by(fn(u) => -u.age)                 // DESC via negación
    .limit(10)
    .offset(0)
    .all(db).await?
```

**SQL emitido**:

```sql
SELECT * FROM users
WHERE "role" = $1 AND "age" >= $2
ORDER BY "age" DESC
LIMIT 10 OFFSET 0
```

Detalles de cada chain method:

### `.where(closure)` adicional

Múltiples `.where(...)` se combinan con AND. Estilísticamente
**preferí un solo closure con `and`** — queda más legible y el
codegen emite el mismo SQL:

```fitz
// Equivalentes — preferí el segundo
User.where(fn(u) => u.role == "admin").where(fn(u) => u.age >= 30)
User.where(fn(u) => u.role == "admin" and u.age >= 30)
```

### `.order_by(closure)` — ASC por default; negación para DESC

```fitz
User.where(fn(u) => true).order_by(fn(u) => u.age)        // ASC
User.where(fn(u) => true).order_by(fn(u) => -u.age)       // DESC (negación)
```

Múltiples `.order_by(...)` se acumulan:

```fitz
let sorted = User.where(fn(u) => true)
    .order_by(fn(u) => u.role)            // primary: ASC
    .order_by(fn(u) => -u.age)            // secondary: DESC
    .all(db).await?
```

SQL: `ORDER BY "role" ASC, "age" DESC`.

### `.limit(n)` + `.offset(n)` — paginación

```fitz
// Página 2 con 20 rows por página: offset = (2-1) * 20 = 20
let page = User.where(fn(u) => u.active).limit(20).offset(20).all(db).await?
```

**Patrón canónico para handler HTTP paginado**:

```fitz
@get("/users")
async fn list(page: Int = 1, per_page: Int = 20) -> Result<List<User>> {
    let offset = (page - 1) * per_page
    return User.where(fn(u) => u.active)
        .order_by(fn(u) => -u.id)
        .limit(per_page)
        .offset(offset)
        .all(db).await
}
```

---

## Paso 6 — Operadores extendidos en `.where(...)`

El closure de `.where(...)` soporta más que comparators básicos.
Acá los más usados:

### Comparators

```fitz
User.where(fn(u) => u.age == 18)        // "age" = $1
User.where(fn(u) => u.age != 18)        // "age" <> $1
User.where(fn(u) => u.age < 18)         // <
User.where(fn(u) => u.age <= 18)        // <=
User.where(fn(u) => u.age > 18)         // >
User.where(fn(u) => u.age >= 18)        // >=
```

### Lógicos `and` / `or` / `not`

```fitz
User.where(fn(u) => u.age >= 18 and u.active)
User.where(fn(u) => u.role == "admin" or u.role == "moderator")
User.where(fn(u) => not u.deleted)

// Agrupado con paréntesis
User.where(fn(u) => (u.age >= 18 and u.role == "admin") or u.id == 1)
```

### Aritméticos (incluye `%` mod)

```fitz
User.where(fn(u) => u.age + 5 > 25)
User.where(fn(u) => u.age * 2 < 50)
User.where(fn(u) => u.age % 2 == 0)     // edades pares
```

### `.between(low, high)` — rangos numéricos

```fitz
User.where(fn(u) => u.age.between(18, 65))
// SQL: "age" BETWEEN $1 AND $2
```

### `.is_in([list])` — IN

```fitz
User.where(fn(u) => u.id.is_in([1, 2, 3]))
// SQL: "id" = ANY($1::int8[])

User.where(fn(u) => u.role.is_in(["admin", "moderator"]))
```

**Caveat MVP**: el arg de `.is_in(...)` debe ser un **List literal
directo** (`.is_in([x, y, z])`). Una var List externa NO funciona
directo (`.is_in(some_var)` falla). Los items adentro SÍ pueden
ser vars (`.is_in([min_id, max_id])` OK).

### Métodos sobre Str

```fitz
User.where(fn(u) => u.email.is_null())               // IS NULL
User.where(fn(u) => u.email.is_not_null())           // IS NOT NULL
User.where(fn(u) => u.email.like("%@example.com"))   // LIKE
User.where(fn(u) => u.email.ilike("%ADA%"))          // ILIKE (case-insens)
User.where(fn(u) => u.email.starts_with("ada"))      // LIKE 'ada%'
User.where(fn(u) => u.email.ends_with("@x.com"))     // LIKE '%@x.com'
User.where(fn(u) => u.email.contains("ada"))         // LIKE '%ada%'
```

**`%`/`_` se escapan automáticamente** en `starts_with` / `ends_with`
/ `contains`. NO se escapan en `like` / `ilike` (ahí vos
controlás el pattern).

### Vars del scope externo

El translator soporta vars del scope exterior al closure:

```fitz
let min_age = 18
let role_filter = "admin"

let adults = User.where(fn(u) => u.age >= min_age and u.role == role_filter)
    .all(db).await?
// SQL: WHERE "age" >= $1 AND "role" = $2  args [18, "admin"]
```

**Útil para handlers HTTP con query params**:

```fitz
@get("/users/by-role/{role}")
async fn by_role(role: Str) -> Result<List<User>> {
    return User.where(fn(u) => u.role == role).all(db).await
}
```

### Tabla resumen — soporte de vars externas

| Operador / Method | Var externa aceptada |
|---|---|
| Comparators (`==`, `<`, etc.) | ✅ ambos lados |
| Lógicos | n/a |
| Aritméticos | ✅ ambos lados |
| `.between(low, high)` | ✅ vars OK |
| `.is_in(list)` | ⚠ List arg literal; items adentro OK |
| `.like(pat)` / `.ilike(pat)` | ✅ pat var OK |
| `.starts_with(s)` / `.ends_with` / `.contains` | ❌ Str literal REQUERIDO |
| `.is_null()` / `.is_not_null()` | n/a (sin args) |

Si necesitás algo fuera de esta tabla, bajá a `db.query(...)`
crudo del cap C1.

---

## Paso 7 — Agregados scalar: `.count` / `.sum` / `.avg` / `.min` / `.max`

Para resúmenes numéricos sobre una sub-tabla:

```fitz
let count: Int = User.where(fn(u) => u.active).count(db).await?
let avg_age: Float = User.where(fn(u) => true).avg(fn(u) => u.age, db).await?
let max_age: Int = User.where(fn(u) => u.active).max(fn(u) => u.age, db).await?
let min_age: Int = User.where(fn(u) => u.active).min(fn(u) => u.age, db).await?
let sum_logins: Float = User.where(fn(u) => u.active).sum(fn(u) => u.login_count, db).await?
```

Detalles:

- **`.count(db)`** devuelve `Int`.
- **`.sum` / `.avg` / `.min` / `.max`** devuelven `Float` por
  default (cast `::float8` automático en el SQL). Para edades
  enteras, `max(age)` devuelve `Int` cuando el cast es
  inocuo.
- **Todos toman `.where(...)` previo** — incluso si querés
  agregar sobre TODA la tabla, usás `.where(fn(_) => true)`:

```fitz
let total = User.where(fn(_) => true).count(db).await?
```

> **Nota**: `User.count(db)` / `User.avg(...)` directos (sin
> `.where(...)`) NO están en MVP. Siempre via QueryBuilder.

---

## Paso 8 — GROUP BY con `Aggregated<Row>`

Para reports con N rows agregadas por una columna:

```fitz
let by_role = User.where(fn(_) => true)
    .group_by(fn(u) => u.role)
    .count(db).await?

print("by_role:")
for r in by_role {
    print("  {r}")
}
```

Output:

```
by_role:
  {"role": "admin", "count": 2}
  {"role": "user", "count": 47}
```

Detalles:

- **`.group_by(closure)`** cambia el tipo retornado de
  `QueryBuilder<Row>` a `Aggregated<Row>`. El checker lo
  distingue estáticamente.
- **El terminal devuelve `List<Map<Str, Any>>`** — no `List<User>`
  ni `List<Row>`. Cada elemento es un row de agregación con la
  columna de GROUP BY + el resultado del agregado (`count`,
  `sum`, etc.).
- **Chain sobre `Aggregated<Row>`**:
  - `.where(...)` — filtro pre-GROUP BY.
  - `.order_by(...)` — ordena el output.
  - `.limit(n)` / `.offset(n)` — pagina.
  - `.group_by(...)` — agrega otra columna al GROUP BY.

**Ejemplo más rico — reporte de edad por rol**:

```fitz
let report = User.where(fn(u) => u.active)
    .group_by(fn(u) => u.role)
    .order_by(fn(u) => u.role)
    .count(db).await?
```

> **Limitación MVP**: `.sum`/`.avg`/`.min`/`.max` sobre
> `Aggregated<Row>` no están en MVP — solo `.count(db)`. Para
> agregados scalar con GROUP BY, bajar a `db.query("SELECT
> ..., AVG(age) FROM ... GROUP BY ...", [])` crudo.

---

## Paso 9 — Programa end-to-end

```fitz
// crud-demo.fitz
@table("users") type User {
    @primary id: Int = 0
    name: Str
    age: Int
    role: Str = "user"
}

async fn main() -> Result<Str> {
    let db = db.connect(
        env_or("DATABASE_URL", "postgres://postgres:secret@localhost:5432/fitz_curso?sslmode=disable")
    ).await?

    let _ = db.exec("DROP TABLE IF EXISTS users", []).await?
    let _ = db.exec("CREATE TABLE users (id bigserial PRIMARY KEY, name text NOT NULL, age bigint NOT NULL, role text NOT NULL DEFAULT 'user')", []).await?

    // CREATE — insert returning *.
    let ada = User.insert(db, User { id: 0, name: "Ada", age: 35, role: "admin" }).await?
    let _ = User.insert(db, User { id: 0, name: "Alan", age: 28, role: "user" }).await?
    let _ = User.insert(db, User { id: 0, name: "Edsger", age: 50, role: "user" }).await?
    let _ = User.insert(db, User { id: 0, name: "Linus", age: 54, role: "admin" }).await?

    // READ — chain con paginación.
    let top2 = User.where(fn(u) => u.age > 0)
        .order_by(fn(u) => -u.age)
        .limit(2)
        .all(db).await?
    print("top 2 por edad desc:")
    for u in top2 {
        print("  {u.name} ({u.age})")
    }

    // READ — operadores extendidos.
    let between = User.where(fn(u) => u.age.between(25, 40)).all(db).await?
    print("edades 25-40: {len(between)}")

    let prefix = User.where(fn(u) => u.name.starts_with("A")).all(db).await?
    print("nombres empezando con A: {len(prefix)}")

    // UPDATE — guard obligatorio.
    let updated = User.where(fn(u) => u.id == ada.id).update(db, {"age": 36}).await?
    print("ada actualizada: {updated} row")

    // AGREGADOS.
    let count = User.where(fn(_) => true).count(db).await?
    print("total: {count}")

    let avg = User.where(fn(_) => true).avg(fn(u) => u.age, db).await?
    print("edad promedio: {avg}")

    // GROUP BY.
    let by_role = User.where(fn(_) => true).group_by(fn(u) => u.role).count(db).await?
    print("por rol:")
    for r in by_role {
        print("  {r}")
    }

    // DELETE — guard obligatorio.
    let deleted = User.where(fn(u) => u.role == "user" and u.age < 30).delete(db).await?
    print("borrados: {deleted}")

    return Ok("OK")
}

print(main().await)
```

Output esperado:

```
top 2 por edad desc:
  Linus (54)
  Edsger (50)
edades 25-40: 2
nombres empezando con A: 2
ada actualizada: 1 row
total: 4
edad promedio: 42.25
por rol:
  {"role": "admin", "count": 2}
  {"role": "user", "count": 2}
borrados: 1
```

---

## Subset compilable a binario

| Feature | `fitz run` | `fitz build` |
|---|---|---|
| `Type.insert(db, row)` con `RETURNING *` | ✅ | ✅ |
| `Type.bulk_insert(rows, db)` | ✅ | ✅ |
| `.update(db, changes)` con guard | ✅ | ✅ |
| `.delete(db)` con guard | ✅ | ✅ |
| Sin guard → error de codegen | ✅ | ✅ |
| `.where(...)` chained con AND | ✅ | ✅ |
| `.order_by(fn(u) => u.field)` ASC | ✅ | ✅ |
| `.order_by(fn(u) => -u.field)` DESC (negación) | ✅ | ✅ |
| `.limit(n)` + `.offset(n)` | ✅ | ✅ |
| Operadores `.between` / `.is_in` / `.like` / etc. | ✅ | ✅ |
| Vars externas en closures | ✅ | ✅ |
| `.count(db)` / `.sum/.avg/.min/.max` con `.where` | ✅ | ✅ |
| `.group_by(closure).count(db)` → `Aggregated<Row>` | ✅ | ✅ |
| `.is_in(var_list)` directo | ❌ List literal REQUERIDO | ❌ |
| `.sum/.avg/.min/.max` sobre `Aggregated` | ❌ usar `db.query` crudo | ❌ |
| `.order_by(closure, ascending: Bool)` kwarg | ❌ usar negación | ❌ |

---

## Validación

- [ ] `User.insert(db, User { id: 0, ... }).await?` devuelve la
      instancia con `id` asignado por Postgres.
- [ ] `User.bulk_insert([row1, row2, ...], db).await?` retorna
      el count, no las instancias.
- [ ] `User.update(db, {...})` SIN `.where(...)` previo es ERROR
      DE COMPILACIÓN.
- [ ] `User.where(fn(_) => true).update(db, {...})` compila y
      actualiza todas las rows.
- [ ] `User.delete(db)` sin guard es ERROR DE COMPILACIÓN.
- [ ] `.order_by(fn(u) => -u.age)` (negación) emite `ORDER BY
      "age" DESC`.
- [ ] `.order_by(closure, ascending: false)` (kwarg) NO existe —
      usar negación.
- [ ] `.where(fn(u) => u.age.between(18, 65))` emite `BETWEEN`.
- [ ] `.where(fn(u) => u.id.is_in([1, 2, 3]))` con List literal
      funciona.
- [ ] `.where(fn(u) => u.id.is_in(some_var))` con var List falla
      en compile-time.
- [ ] `.where(fn(u) => u.name.starts_with("ada"))` escapa `%`/`_`
      automático.
- [ ] `.where(fn(u) => u.role == role_filter)` con var externa
      compila y bindea correctamente.
- [ ] `.where(fn(_) => true).count(db).await?` devuelve `Int`.
- [ ] `.where(fn(_) => true).avg(fn(u) => u.age, db).await?`
      devuelve `Float`.
- [ ] `.group_by(fn(u) => u.role).count(db).await?` devuelve
      `List<Map<Str, Any>>` con un row por grupo.
- [ ] `fitz build` del programa CRUD produce binario standalone
      que ejecuta todo el flow contra Postgres.

---

## Troubleshooting

### `Error en línea N — .update() requiere .where(...) previo`

El checker rechaza updates sin guard. **Si querés actualizar
TODAS las rows intencionalmente**:

```fitz
let n = User.where(fn(_) => true).update(db, {...}).await?
```

Lo mismo con `.delete(db)`.

### `Error en línea N — el método order_by espera 1 argumento(s), recibió 2`

Usaste `.order_by(closure, ascending: false)` (kwarg). En MVP
no existe — usá **negación** en el closure:

```fitz
.order_by(fn(u) => -u.age)    // DESC
```

### `Error en línea N — el tipo `User` no tiene un método estático llamado `count``

`User.count(db)` directo NO existe — siempre via QueryBuilder:

```fitz
let total = User.where(fn(_) => true).count(db).await?
```

Lo mismo con `User.avg(...)`, `User.first(db)`, `User.group_by(...)`.

### `Err("violation of NOT NULL constraint on column 'X'")` en `.update`

El `Map` de changes envió `null` a una columna NOT NULL. Verificá
que las keys del map matcheen con columnas nullables o que el
value NO sea null.

### `Err("invalid input syntax for type integer: ...")` en `.insert`

El field es `Int` en Fitz pero pasaste un value no-numérico.
Causas:

1. Mismatch type entre Fitz y Postgres.
2. El value viene de un body HTTP donde el cliente mandó un
   string en vez de número. Body deserialization debería
   capturarlo antes — verificá los anotaciones del `type` body.

### `.starts_with("var-no-literal")` falla en compile-time

```fitz
let prefix = "ada"
User.where(fn(u) => u.name.starts_with(prefix))    // ❌ var, no literal
```

En MVP, el arg de `.starts_with` / `.ends_with` / `.contains`
**debe ser Str literal**. Workaround:

```fitz
let prefix = "ada%"
User.where(fn(u) => u.name.like(prefix))    // ✅ .like sí acepta var
```

Pero recordá: `like(pat)` NO escapa `%`/`_` — el user controla.

### `.is_in(var_list)` falla en compile-time

```fitz
let ids = [1, 2, 3]
User.where(fn(u) => u.id.is_in(ids))    // ❌ var, no literal
```

Workaround: extender el closure a usar `or`:

```fitz
User.where(fn(u) => u.id == 1 or u.id == 2 or u.id == 3)
```

O bajar a `db.query("SELECT ... WHERE id = ANY($1)", [ids])`
crudo.

### El `.group_by(...).sum(...)` falla con "el método no existe"

En MVP, solo `.count(db)` está disponible como terminal de
`Aggregated<Row>`. Para `SUM`/`AVG`/`MAX`/`MIN` con GROUP BY,
bajar a `db.query("SELECT role, SUM(login_count) FROM users
GROUP BY role", [])` crudo del cap C1.

### `bulk_insert` mete N rows pero el resto del programa no ve los IDs

Esperado. `bulk_insert` NO emite `RETURNING *`. Si querés los
IDs reales, usá `.insert` en loop. Para seeds masivas donde los
IDs no importan, `bulk_insert` es mucho más rápido.

### `Update` aparenta funcionar pero el row no cambia

Verificá:

1. El `.where(...)` matchea con rows reales. `let n = .update(...)`
   devuelve `0` si no matchea ninguno (no Err).
2. El `Map` de changes tiene keys correctas — typo en la key NO
   da error de checker (es `Map<Str, Any>`), pero la columna no
   se actualiza.
3. Estás haciendo `await?` y no descartando el Future.

---

## Lo que sigue

Llegaste al final del cap. Lo que cubriste:

- `Type.insert(db, row)` con `RETURNING *` automático — recibís
  la instancia hidratada con el `id` real.
- `Type.bulk_insert(rows, db)` para cargas masivas en batches
  multi-tuple.
- `.update(db, changes)` y `.delete(db)` con **`.where(...)`
  guard obligatorio** — el checker rechaza statically updates
  sin filtro.
- Chain methods del QueryBuilder: `.where` adicional con AND,
  `.order_by(fn(u) => -u.field)` DESC via negación, `.limit` +
  `.offset` para paginación.
- Operadores extendidos en `.where(...)`: `.between`, `.is_in`,
  `.like`, `.ilike`, `.starts_with`/`.ends_with`/`.contains`
  con escape automático de `%`/`_`, `.is_null`/`.is_not_null`.
- Vars del scope externo en closures, con tabla de soporte por
  operador.
- Agregados scalar: `.count` (Int), `.sum`/`.avg`/`.min`/`.max`
  (Float) sobre QueryBuilder.
- GROUP BY con `.group_by(closure).count(db)` → `List<Map<Str,
  Any>>`.

**Próximo cap**: [M6.C4 — Relations + navigation methods +
eager loading](c4-relations-navigation-preload.md). Vamos a
declarar `@belongs_to`/`@has_many`/`@has_one`, navegar con
`post.user_id(db).await?`, y resolver el problema clásico de
N+1 queries con `.preload("posts")` que el checker valida
estáticamente.
