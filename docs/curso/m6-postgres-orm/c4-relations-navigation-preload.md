# M6.C4 — Relations, navigation methods y eager loading

**Pre-requisitos**: [M6.C3 — Writes + QueryBuilder](c3-writes-querybuilder-agregados.md).
Tenés el CRUD básico (insert/update/delete) y sabés encadenar
chain methods + agregados.

**Objetivo**: declarar **relaciones entre tablas** —
`@belongs_to` (FK del lado N), `@has_many` (virtual del lado 1),
`@has_one` (1-1). Resolverlas en runtime con **navigation
methods** tipados. Y cerrar el problema clásico de **N+1
queries** con eager loading manual via `.is_in([...])` (más
`.preload(...)` para el codegen path con caveats).

**Por qué importa**: el 90% de los modelos reales tienen
relaciones. Un blog tiene `User has_many Posts`. Un e-commerce
tiene `Order belongs_to Customer`. Un CRM tiene `Account
has_many Contacts`. En SQLAlchemy/Rails/Prisma, declarás esto
con anotaciones que se resuelven en runtime con reflection. En
Fitz, los decoradores son **parte del lenguaje** — el checker
valida en compile-time que `@belongs_to("User")` apunte a un
type existente y el codegen emite navigation methods tipados.

**Cross-link**: [DB y ORM § 10-12](../../db-orm.md#10-relations-belongs_to-has_one-has_many).

---

## Mapa del cap

```mermaid
flowchart LR
    A["@belongs_to FK del lado N"] --> B[columna real bigint REFERENCES]
    C["@has_many virtual del lado 1"] --> D[shortcut del ORM no es columna]
    E["@has_one 1-1"] --> F[Target nullable virtual]
    G[instance.fk_field db] --> H[BelongsTo Result Target]
    I[instance.virtual db] --> J[HasMany Result List Target]
    K[instance.virtual sin db] --> L[QueryBuilder chain]
    M[N+1 problem] --> N[Manual batch is_in fks]
    M --> O[.preload codegen path con caveats]
```

---

## Por qué Fitz es distinto

| Feature | SQLAlchemy 2.x | Rails ActiveRecord | Diesel | Prisma | **Fitz** |
|---|---|---|---|---|---|
| Decorators de relations | `Mapped[User]` + `relationship()` | `belongs_to :user` runtime | `derive(Associations)` + `belongs_to!` macro | `User @relation(...)` en `.prisma` | **`@belongs_to`/`@has_many`/`@has_one`** |
| Validación estática | ❌ runtime | ❌ runtime | ✅ macro | ✅ con `prisma generate` | ✅ **checker** |
| Navigation method tipado | `post.user` (lazy) | `post.user` (lazy SQL) | `Post::belonging_to(user)` macro | `post.user` (gen) | **`post.user_id(db).await?`** |
| Navigation chain (sin ejecutar) | `query(Post).filter_by(user_id=u.id)` | `u.posts.where(...)` | sin equivalente directo | `prisma.user.findUnique({...}).posts()` | **`u.posts().where(...)`** |
| Eager loading típico | `joinedload(User.posts)` | `includes(:posts)` runtime | `BelongsToCompanion::belonging_to(...)` macro | `include: { posts: true }` | **`.preload("posts")`** |
| Typos en relation name | ❌ runtime AttributeError | ❌ runtime NoMethodError | ✅ compile-time | ⚠ TS errors si hay codegen | ✅ **compile-time** |
| Resolución del schema | reflection runtime | reflection runtime | macro `table!` | `.prisma` + `prisma generate` | **decorators en `type` Fitz** |

El diferencial mayor: **navigation chain con `instance.field()` sin
arg `db`**. Te devuelve un `QueryBuilder<Target>` que sigue
encadenando — podés agregar `.where(...)`, `.order_by(...)`,
`.limit(...)` antes de ejecutar. Es como `post.user_id(db)`
pero **componible**, no lazy mágico.

---

## Paso 1 — `@belongs_to("Target")`: el lado N

El lado N de una relación 1→N tiene la columna **FK real** en
su tabla:

```fitz
@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    @belongs_to("User") user_id: Int    // FK real: bigint REFERENCES users(id)
}
```

Detalles:

- **El field marcado ES una columna real** en la tabla — `bigint
  NOT NULL REFERENCES users(id)` (o similar). Entra al SELECT,
  al INSERT, al UPDATE.
- **`"User"` es el nombre del `type` Fitz target**. El checker
  valida que ese `type` exista, sea `@table`, y tenga `@primary`.
- **Convención de nombre**: el field es `<lowercase_target>_id`
  (`user_id`, `customer_id`, `post_id`). El navigation method
  toma el mismo nombre — `post.user_id(db).await?`.

### Override del nombre del FK

Si tu columna FK no sigue la convención:

```fitz
@belongs_to("User", fk="created_by_user_id") created_by: Int
```

Útil para naming legacy o convenciones de equipo distintas.

### Kwargs `on_delete` / `on_update`

```fitz
@belongs_to("User", on_delete="cascade", on_update="cascade") user_id: Int
```

Valores aceptados como string literal: `"cascade"`, `"set_null"`,
`"restrict"`, `"no_action"`. El ORM persiste el FK action en la
metadata. **Pero NO genera la migración** (MVP) — vos creás la
tabla con `CREATE TABLE ... user_id bigint REFERENCES users(id)
ON DELETE CASCADE`.

---

## Paso 2 — `@has_many("Target", via="fk_column")`: el lado 1

El lado 1 declara un field **virtual** — NO es columna en la
tabla:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    @has_many("Post", via="user_id") posts: List<Post>
}
```

Detalles:

- **`posts: List<Post>` NO es una columna en `users`**. Es un
  shortcut del ORM que se hidrata con navigation methods o
  eager loading.
- **`via="user_id"`** indica qué columna en la tabla `posts`
  apunta de vuelta. Sin esto, el ORM no sabría qué FK usar.
- **El field NO entra al SELECT/INSERT/UPDATE** del `User` — el
  ORM lo detecta como virtual y lo skipea automático.

**SQL emitido** para `u.posts(db).await?`:

```sql
SELECT * FROM posts WHERE user_id = $1
```

(donde `$1` es `u.id`).

---

## Paso 3 — `@has_one("Target", via="fk_column")`: 1-1

Mismo modelo que `@has_many` pero con cardinalidad 1:1. El
field virtual es `Target?` (nullable) en lugar de
`List<Target>`:

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

**Patrón canónico**: cada `User` tiene como máximo un
`Profile`. El `profile: Profile?` puede ser `null` si el user
todavía no tiene profile.

Navigation: `user.profile(db).await?` devuelve
`Result<Profile>` — `Err` si no existe.

---

## Paso 4 — Navigation methods con `(db)`

Después de declarar relations, cada field genera un **método de
navegación** sobre la instancia:

### BelongsTo: `post.user_id(db) -> Future<Result<User>>`

```fitz
let post = Post.where(fn(p) => p.id == 1).first(db).await?
let author: User = post.user_id(db).await?
print("autor de '{post.title}': {author.email}")
```

Detalles:

- **El método se llama como el field FK** (`user_id`), no como
  el target (`user`). Esa convención hace explícito que el
  navigation usa el FK column.
- **SQL emitido**: `SELECT * FROM users WHERE id = $1` con el
  FK value como param.
- **Retorno**: `Result<User>` — `Err` si el FK apunta a un id
  que no existe (raro con FK constraint, pero posible si la
  consistencia se rompió).

### HasMany: `user.posts(db) -> Future<Result<List<Post>>>`

```fitz
let user = User.where(fn(u) => u.id == 1).first(db).await?
let user_posts: List<Post> = user.posts(db).await?
print("posts de {user.email}: {len(user_posts)}")
```

- **El método se llama como el field virtual** (`posts`).
- **SQL emitido**: `SELECT * FROM posts WHERE user_id = $1`.

### HasOne: `user.profile(db) -> Future<Result<Profile>>`

```fitz
let user = User.where(fn(u) => u.id == 1).first(db).await?
let profile = user.profile(db).await?
// Err si el user no tiene profile (es Result<Profile>, no Result<Profile?>)
```

---

## Paso 5 — Navigation chain sin `(db)` → `QueryBuilder<Target>`

**Innovación importante**: si llamás la navigation method **SIN**
el arg `db`, recibís un `QueryBuilder<Target>` que sigue
encadenando — equivalente a "empezar un query desde la
relation":

```fitz
// Equivalente a: SELECT * FROM posts WHERE user_id = $1 ORDER BY id DESC LIMIT 5
let latest_5 = user.posts()
    .order_by(fn(p) => -p.id)
    .limit(5)
    .all(db).await?
```

Detalles:

- **`user.posts(db)`** — terminal directo (ejecuta query
  inmediato, devuelve `List<Post>`).
- **`user.posts()`** — empieza chain (NO ejecuta, devuelve
  `QueryBuilder<Post>`).

**Útil para filtros adicionales** sobre la relation:

```fitz
let recent_drafts = user.posts()
    .where(fn(p) => p.status == "draft" and p.published == false)
    .order_by(fn(p) => -p.id)
    .limit(10)
    .all(db).await?
```

Es como `u.posts.where(...).order_by(...).limit(...)` de Rails
pero **componible explícitamente** en vez de lazy mágico.

---

## Paso 6 — El problema N+1 y cómo evitarlo

### El problema

Si tenés N users y querés todos sus posts:

```fitz
let users = User.all(db).await?               // 1 query
for u in users {
    let posts = u.posts(db).await?            // 1 query por user
    print("{u.email}: {len(posts)} posts")
}
```

Con **100 users**, esto hace **101 queries** (1 + 100). Es el
clásico problema **N+1**. Para 1000 users con latencia de 1ms
por query, son 1 segundo de DB time.

### Solución manual: batch con `.is_in([...])`

Patrón canónico para resolver N+1 sin `.preload`:

```fitz
// 1 query para los users.
let users = User.all(db).await?

// 1 query para TODOS los posts, batched por user_id.
let user_ids = users.map(fn(u) => u.id)
let all_posts = Post.where(fn(p) => p.user_id.is_in(user_ids)).all(db).await?

// Agrupar en memoria (no toca la DB).
let posts_by_user: Map<Int, List<Post>> = {}
for p in all_posts {
    let existing = posts_by_user.get(p.user_id).get_or([])
    posts_by_user[p.user_id] = existing.push(p)
}

// Print.
for u in users {
    let user_posts = posts_by_user.get(u.id).get_or([])
    print("{u.email}: {len(user_posts)} posts")
}
```

**Total queries**: 2 (vs 101). Performance comparable a Diesel
y a SQLAlchemy `joinedload`.

> **Caveat MVP**: `.is_in(...)` exige un **List literal**
> en compile-time. Hacé `user_ids = users.map(...)` y pasalo
> como var no funciona directo. Workaround: bajar a `db.query(
> "SELECT * FROM posts WHERE user_id = ANY($1)", [user_ids])`
> crudo del cap C1.

### `.preload(relation_name)` — eager loading nativo

El ORM tiene un método `.preload(...)` que cierra el N+1 con
**dispatch estático en compile-time**:

```fitz
// Conceptualmente: 1 query para users + 1 query batch para todos los posts.
let users = User.preload("posts").all(db).await?

for u in users {
    print("{u.email}: {len(u.posts)} posts")   // u.posts ya hidratado
}
```

**Cómo funciona**:

- `"posts"` es **Str literal en compile-time** (no se acepta
  var).
- El codegen emite un `match` exhaustivo por type con la rama
  correspondiente. **Typos detectados en compile-time** (no
  runtime):
  ```fitz
  User.preload("post").all(db).await?    // ❌ typo: "post"
  // → error de codegen: relation "post" no existe en User.
  //   Conocidas: posts.
  ```

> **Estado en v0.11.2**: el método `.preload(...)` está vivo en
> el **codegen path** (`fitz build`) pero **NO está implementado
> en el intérprete** (`fitz run`). Smoke local con `.preload(...)`
> en `fitz run` da:
> ```
> Error en línea N — el tipo `User` no tiene un método estático
> llamado `preload`
> ```
> Es deuda visible — eventualmente el intérprete lo va a soportar.
> **Por ahora**: usá el **manual batch con `.is_in([...])`**
> documentado arriba para `fitz run` (workaround estándar), y
> `.preload(...)` para programs que deployás con `fitz build`.

### BelongsTo eager con sibling field

Cuando declarás un sibling `user: User?` junto al FK, el ORM
detecta el companion y permite `.preload("user")` desde el lado
N:

```fitz
@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    @belongs_to("User") user_id: Int
    user: User?    // companion auto-detectado
}

let posts_with_authors = Post.preload("user").all(db).await?
for p in posts_with_authors {
    match p.user {
        null => print("{p.title}: (sin author)"),
        _ => print("{p.title}: by {p.user.name}"),
    }
}
```

Mismo caveat — funciona en `fitz build`, no en `fitz run` MVP.

---

## Paso 7 — Programa end-to-end (sin .preload)

Demo realista con `fitz run` usando navigation directa + manual
batch:

```fitz
@table("users") type User {
    @primary id: Int = 0
    name: Str
    @has_many("Post", via="user_id") posts: List<Post>
}

@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    @belongs_to("User") user_id: Int
    user: User?
}

async fn main() -> Result<Str> {
    let db = db.connect(
        env_or("DATABASE_URL", "postgres://postgres:secret@localhost:5432/fitz_curso?sslmode=disable")
    ).await?

    // Schema.
    let _ = db.exec("DROP TABLE IF EXISTS posts", []).await?
    let _ = db.exec("DROP TABLE IF EXISTS users", []).await?
    let _ = db.exec("CREATE TABLE users (id bigserial PRIMARY KEY, name text NOT NULL)", []).await?
    let _ = db.exec("CREATE TABLE posts (id bigserial PRIMARY KEY, title text NOT NULL, user_id bigint NOT NULL REFERENCES users(id))", []).await?

    // Seed.
    let ada = User.insert(db, User { id: 0, name: "Ada", posts: [] }).await?
    let alan = User.insert(db, User { id: 0, name: "Alan", posts: [] }).await?

    let _ = Post.insert(db, Post { id: 0, title: "Notes on the Analytical Engine", user_id: ada.id, user: null }).await?
    let _ = Post.insert(db, Post { id: 0, title: "First mathematician?", user_id: ada.id, user: null }).await?
    let _ = Post.insert(db, Post { id: 0, title: "Turing test", user_id: alan.id, user: null }).await?

    // 1. BelongsTo navigation directa.
    let p1 = Post.where(fn(p) => p.id == 1).first(db).await?
    let author = p1.user_id(db).await?
    print("autor de '{p1.title}': {author.name}")

    // 2. HasMany navigation directa.
    let ada_posts = ada.posts(db).await?
    print("{ada.name} tiene {len(ada_posts)} posts:")
    for p in ada_posts {
        print("  - {p.title}")
    }

    // 3. Navigation chain (QueryBuilder).
    let alan_first = alan.posts().order_by(fn(p) => p.id).limit(1).all(db).await?
    print("primer post de {alan.name}: {alan_first[0].title}")

    return Ok("OK")
}

print(main().await)
```

Output:

```
autor de 'Notes on the Analytical Engine': Ada
Ada tiene 2 posts:
  - Notes on the Analytical Engine
  - First mathematician?
primer post de Alan: Turing test
```

---

## Subset compilable a binario

| Feature | `fitz run` | `fitz build` |
|---|---|---|
| `@belongs_to("Target")` | ✅ | ✅ |
| `@belongs_to(..., fk="...")` override | ✅ | ✅ |
| `@belongs_to(..., on_delete="cascade")` (metadata) | ✅ | ✅ |
| `@has_many("Target", via="...")` | ✅ | ✅ |
| `@has_one("Target", via="...")` | ✅ | ✅ |
| `instance.fk_field(db).await` (BelongsTo) | ✅ | ✅ |
| `instance.virtual_field(db).await` (HasMany/HasOne) | ✅ | ✅ |
| `instance.virtual_field()` (sin db) → QueryBuilder | ✅ | ✅ |
| Manual batch con `.is_in([fks])` | ✅ | ✅ |
| `Type.preload("rel").all(db)` | ❌ deuda intérprete | ✅ |
| `Type.preload("user").all(db)` (BelongsTo companion) | ❌ deuda intérprete | ✅ |
| Compañion field `user: User?` para BelongsTo eager | ✅ declarable | ✅ + activo |
| FK auto-generation en migrations | ❌ (manual `CREATE TABLE`) | ❌ |

---

## Validación

- [ ] `@belongs_to("User")` sobre `user_id: Int` compila con
      `fitz check`.
- [ ] Si `@belongs_to("Userr")` apunta a un type que NO existe,
      el checker da error claro.
- [ ] `@has_many("Post", via="user_id") posts: List<Post>`
      declara un field virtual que NO entra al SELECT del
      `User`.
- [ ] `post.user_id(db).await?` ejecuta `SELECT * FROM users
      WHERE id = $1` y devuelve el `User`.
- [ ] `user.posts(db).await?` ejecuta `SELECT * FROM posts
      WHERE user_id = $1` y devuelve `List<Post>`.
- [ ] `user.posts()` (sin db) devuelve un `QueryBuilder<Post>`
      encadenable con `.where`/`.order_by`/`.limit`.
- [ ] El patrón N+1 manual con `.is_in([fks])` funciona en 2
      queries totales (no N+1).
- [ ] `User.preload("posts").all(db)` ✅ funciona en `fitz build`
      / ❌ falla en `fitz run` con "no tiene método estático
      llamado `preload`".
- [ ] `User.preload("typo")` falla en compile-time (`fitz build`)
      con mensaje claro listando los relation names válidos.
- [ ] El field `user: User?` companion en `Post` permite acceso
      `p.user.name` después de eager loading.

---

## Troubleshooting

### `Error en línea N — el tipo "Y" no existe`

`@belongs_to("Userr")` typo (extra `r`). El checker valida que
el target sea un type declarado en el programa. Verificá la
declaración del target.

### `Error en línea N — el tipo `User` no tiene un método estático llamado `preload``

`.preload(...)` no está implementado en el intérprete (`fitz
run`). **Workaround**:

1. Manual batch con `.is_in([fks])`:
   ```fitz
   let users = User.all(db).await?
   let ids = users.map(fn(u) => u.id)
   let posts = Post.where(fn(p) => p.user_id.is_in(ids)).all(db).await?
   ```
2. Si querés `.preload`, compilá con `fitz build` y corré el
   binario.

### `Err("foreign key violation")` al hacer `Post.insert(...)`

El `user_id` del Post apunta a un user que NO existe en la
tabla `users`. Causas:

1. Hiciste `Post.insert(db, Post { user_id: 999, ... })` con
   un id que no existe.
2. Borraste el User pero no los Posts (si `on_delete="cascade"`
   está en el metadata pero no en el SQL real, no cascadea
   automático).

### `user.posts()` (sin db) no encadena con `.where(...)`

Verificá:

1. Estás llamando SIN el arg `db` literalmente (no `user.posts(db)`
   ni `user.posts({})`).
2. El field `posts` está declarado con `@has_many("Post",
   via="user_id")` correctamente.
3. El target type `Post` tiene `@table` declarado.

### N+1 silencioso — el código compila pero las queries son lentas

Caso típico: olvidaste el batch y estás haciendo `for u in
users: u.posts(db).await?`. Verificá los logs de Postgres
(`log_statement=all`) o usá `EXPLAIN ANALYZE` en una query
representativa. Si ves N queries similares, el problema es
N+1.

**Fix**: convertir a manual batch con `.is_in([fks])`.

### `Err("column "user_id" does not exist")` cuando llamo `user.posts(db)`

El `via="user_id"` no matchea con la columna real de la tabla
`posts`. Si la columna se llama `author_id`, el decorator debe
ser `@has_many("Post", via="author_id")`.

### El `@has_many` con `via` distinto al convencional no compila

Sintaxis: `@has_many("Target", via="column_name")` — `via` es
kwarg con `=`, no `:`.

### `fitz build` con `.preload("typo")` produce error grande

El codegen emite el `match` exhaustivo con typo y rustc abajo
te da un mensaje rojo gigante. **Fix**: corregir el typo. El
mensaje incluye los relation names válidos disponibles para
ese type.

### `Post.preload("user").all(db)` — el companion no se hidrata

Verificá:

1. El field `user: User?` está declarado en `Post` (NO en
   `User`).
2. El decorator `@belongs_to("User")` existe sobre el
   `user_id`.
3. Estás compilando con `fitz build`. El intérprete `fitz run`
   no soporta `.preload(...)`.

---

## Lo que sigue

Llegaste al final del cap. Lo que cubriste:

- `@belongs_to("Target")` declara el FK del lado N — columna
  real en la tabla.
- `@has_many("Target", via="fk_col")` declara un field virtual
  del lado 1 que NO entra al SELECT.
- `@has_one("Target", via="fk_col")` igual que `@has_many` pero
  1-1, con field `Target?` virtual.
- Navigation methods: `instance.fk_field(db)` y
  `instance.virtual_field(db)` devuelven `Result<Target>` o
  `Result<List<Target>>`.
- Navigation chain: `instance.virtual_field()` (sin `db`)
  devuelve `QueryBuilder<Target>` componible.
- El problema N+1 y la solución manual con `.is_in([fks])` —
  funciona en `fitz run` y `fitz build`.
- `.preload(relation_name)` con Str literal en compile-time —
  feature del **codegen path** (`fitz build`). En `fitz run`
  MVP no está implementado todavía.

**Próximo cap**: [M6.C5 — Tipos avanzados: jsonb, arrays, Date/
DateTime/Uuid + operadores extendidos](c5-tipos-avanzados.md).
Vamos a ver cómo Fitz mapea automáticamente `Map<Str, Any>` ↔
`jsonb`, `List<scalar>` ↔ `T[]`, los tipos built-in
`Date`/`DateTime`/`Uuid` (no strings ISO 8601), y los
operadores JSON nativos (`.has_key`, `.contains_json`, `.get`)
mapeados a operadores Postgres (`?`, `@>`, `->>`).
