# M6.C5 — Tipos avanzados: jsonb, arrays, Date/DateTime/Uuid

**Pre-requisitos**: [M6.C4 — Relations + navigation](c4-relations-navigation-preload.md).
Tenés el ORM básico cubierto. Faltan los tipos "avanzados" de
Postgres que NO son primitivos clásicos pero que aparecen en
todas las apps reales.

**Objetivo**: dominar los **tipos compuestos** del ORM: jsonb
(`Map<Str, Any>`), arrays (`List<scalar>`), y los **built-in
del lenguaje** `Date` / `DateTime` / `Uuid`. Plus los operadores
de filtrado específicos para cada uno: **JSON operators**
(`.has_key`, `.contains_json`, `.get`, `.path_int`, etc.) y
**array operators** (`.has`, `.contains_all`, `.contained_in`).

**Por qué importa**: en cualquier app moderna hay **metadata
estructurada** que no encaja con columnas relacionales puras —
preferencias del user, payload de un evento, tags de un post,
configuración de un tenant. Postgres lo cubre con `jsonb` +
`text[]`. La mayoría de ORMs (SQLAlchemy/Django/Prisma/etc.) lo
soportan pero con sintaxis incómoda o operadores que se
escriben como strings sueltas. **Fitz lo trae como methods
nativos sobre el field con dispatch tipado**.

**Cross-link**: [DB y ORM — JSONB, arrays y tipos built-in](../../db-orm.md).

---

## Mapa del cap

```mermaid
flowchart LR
    A[Map Str Any] --> B[jsonb column]
    C[List scalar] --> D[T array column]
    E[Date built-in] --> F[date column]
    G[DateTime built-in] --> H[timestamptz UTC]
    I[Uuid built-in] --> J[uuid column]
    B --> K[.has_key .has_all_keys .contains_json .get]
    B --> L[.path_int .path_text .has_path]
    D --> M[.has .contains_all .contained_in]
    G --> N[.now .from_timestamp .add_days .in_tz]
    I --> O[Uuid.v4 .v7 .nil .parse]
```

---

## Por qué Fitz es distinto

| Feature | SQLAlchemy 2.x | Django ORM | Prisma | Diesel | **Fitz** |
|---|---|---|---|---|---|
| jsonb tipado | `JSONB` + acceso dict crudo | `JSONField` + acceso dict | `Json` opaco | `Jsonb` Rust struct | **`Map<Str, Any>` con shape preservado** |
| JSON operators built-in | ⚠ con `.op('?')`/`.op('@>')` strings | ⚠ con `__has_key`/`__contains` lookups | ⚠ con queries crudas | ⚠ con macros | ✅ **methods nativos del lenguaje** |
| arrays Postgres tipados | `ARRAY()` | `ArrayField(...)` | array tipado en `.prisma` | `derive(Queryable)` | ✅ **`List<scalar>` nativo** |
| array ops `.has` / `.contains_all` | ⚠ con `column.any()` / `column.contains()` | ⚠ con `__contains` lookups | sin equivalente directo | ⚠ con macros | ✅ **methods nativos** |
| `Date` / `DateTime` / `Uuid` built-in | con `datetime.date` Python | con `datetime.date` Python | con `Date` / `DateTime` | con `chrono` crate + features | ✅ **built-in del lenguaje** |
| Aritmética de fechas | ⚠ con `timedelta` Python | ⚠ con `timedelta` Python | ⚠ con `Date` JS APIs | ✅ con `chrono` | ✅ **`.add_days(n)` / `.diff_days(other)`** |
| `Uuid.v4()` / `Uuid.v7()` built-in | con `uuid` lib | con `uuid` lib | con lib externa | con `uuid` crate | ✅ **`Uuid.v4()` directo** |
| timezone `.in_tz("IANA")` | con `pytz` | con `pytz` | manual | con `chrono-tz` | ✅ **`.in_tz("America/Buenos_Aires")`** |

El diferencial mayor: **JSON operators y array operators son
methods nativos del lenguaje** que el codegen traduce a SQL
Postgres. `e.data.has_key("foo")` se compila a `"data" ? $1`.
`p.tags.has("rust")` se compila a `$1 = ANY("tags")`. Sin
strings sueltas, sin macros, con autocomplete del LSP.

---

## Paso 1 — `Map<Str, Any>` ↔ `jsonb`

### Declaración

```fitz
@table("events") type Event {
    @primary id: Int = 0
    name: Str
    data: Map<Str, Any>    // → jsonb column
    config: Map<Str, Int>  // → jsonb (shape homogéneo)
}
```

Detalles:

- **`Map<Str, Any>`** acepta values heterogéneos
  (Int/Float/Str/Bool/Null/Map/List nested).
- **`Map<Str, T>`** con `T` concreto (`Map<Str, Int>`,
  `Map<Str, Str>`) también va a jsonb — Postgres no distingue,
  Fitz preserva el shape al SELECT.

### Insert con jsonb

```fitz
let e = Event.insert(db, Event {
    id: 0,
    name: "click",
    data: {
        "page": "/home",
        "ts": 1700000000,
        "active": true,
        "session_id": null,
        "item": {"id": 42, "price": 19.99},     // nested
        "tags": ["nav", "ui"]                    // List adentro de jsonb
    },
    config: {}
}).await?
```

El INSERT serializa el Map con `serde_json` (`preserve_order`
para mantener orden) y cast `::jsonb` antes de mandar a
Postgres.

### Select y consumo

```fitz
let e = Event.where(fn(e) => e.id == 42).first(db).await?
print(e.data["page"])         // "/home"
print(e.data["ts"])           // 1700000000  (Int preservado)
print(e.data["active"])       // true
print(e.data["session_id"])   // null

// Nested.
let item = e.data["item"]
print(item["price"])          // 19.99
```

Los values del Map mantienen su tipo Fitz original. `Null` Fitz
se mapea a `null` JSON real (no la string `"null"`).

---

## Paso 2 — JSON operators en `.where(...)`

5 method calls sobre fields jsonb traducidos a operadores
Postgres nativos:

| Method | SQL emitido | Significado |
|---|---|---|
| `e.data.has_key("foo")` | `"data" ? $1` | la key existe |
| `e.data.has_all_keys(["a", "b"])` | `"data" ?& $1::text[]` | todas las keys existen |
| `e.data.has_any_keys(["a", "b"])` | `"data" ?\| $1::text[]` | cualquier key existe |
| `e.data.contains_json({"k": "v"})` | `"data" @> $1::jsonb` | subset jsonb |
| `e.data.get("foo")` | `"data" ->> $1` | extract text |

Ejemplos:

```fitz
// "events con la key 'page'"
let with_page = Event.where(fn(e) => e.data.has_key("page")).all(db).await?

// "events con la key 'page' Y la key 'user'"
let with_both = Event.where(fn(e) => e.data.has_all_keys(["page", "user"])).all(db).await?

// "events con CUALQUIERA de las keys 'code' o 'extra'"
let either = Event.where(fn(e) => e.data.has_any_keys(["code", "extra"])).all(db).await?

// "events cuyo jsonb contenga AL MENOS {page: '/home'}"
let from_home = Event.where(fn(e) => e.data.contains_json({"page": "/home"})).all(db).await?

// "events del user 'ada' usando .get para extraer text"
let ada_events = Event.where(fn(e) => e.data.get("user") == "ada").all(db).await?
```

**Caveats MVP**:

- `.has_key(s)` / `.get(s)` aceptan **vars externas** como arg.
- `.has_all_keys([...])` / `.has_any_keys([...])` /
  `.contains_json({...})` requieren **literales** (List o Map
  literal directo, no var).
- `.contains_json({...})` solo acepta values primitivos
  (Int/Float/Str/Bool/Null). Para Maps/Lists nested adentro:
  workaround con `db.query(...)` crudo.

---

## Paso 3 — JSON path operators avanzados

Cinco method calls para acceso a **paths anidados** con cast
tipado:

| Method | SQL emitido | Para qué |
|---|---|---|
| `e.data.has_path(["a", "b"])` | `"data" #> $1::text[] IS NOT NULL` | el path existe |
| `e.data.path_text(["a", "b"])` | `("data" #>> $1::text[])` | extract text |
| `e.data.path_int(["a", "b"])` | `(("data" #>> $1::text[])::bigint)` | extract + cast Int |
| `e.data.path_float(["a", "b"])` | `(("data" #>> $1::text[])::float8)` | extract + cast Float |
| `e.data.path_bool(["a", "b"])` | `(("data" #>> $1::text[])::boolean)` | extract + cast Bool |

Ejemplos con jsonb anidado `{"user": {"id": 5, "name": "ada"},
"score": 1.5, "active": true}`:

```fitz
// ¿El path existe?
let with_uid = Event.where(fn(e) => e.data.has_path(["user", "id"])).all(db).await?

// Filtro tipado por Int adentro del jsonb:
let uid_5 = Event.where(fn(e) => e.data.path_int(["user", "id"]) == 5).all(db).await?

// Filtro tipado por Float:
let high_score = Event.where(fn(e) => e.data.path_float(["score"]) > 1.0).all(db).await?

// Filtro Bool directo:
let active = Event.where(fn(e) => e.data.path_bool(["active"])).all(db).await?
```

**Caveats**:

- Path debe ser `List<Str>` **literal** con al menos 1 elemento.
- Cast asume el value adentro es del tipo declarado. Si es
  otro tipo, Postgres lanza error de cast en runtime.
- Path inexistente → extract devuelve NULL → en WHERE el row
  se rechaza (semántica SQL estándar).

---

## Paso 4 — `List<scalar>` ↔ `T[]` arrays Postgres

12 array OIDs soportados (`bool[]`/`int2/4/8[]`/`text[]`/
`varchar[]`/`float4/8[]`/`date[]`/`timestamp[]`/`timestamptz[]`/
`uuid[]`).

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
    title: "Notes on Postgres",
    tags: ["rust", "postgres", "fitz"],
    scores: [100, 85, 92],
    weights: [1.0, 0.8, 1.2],
    flags: [true, false, true]
}).await?
```

Cero glue — `List<Str>` se serializa a `text[]` Postgres
nativo.

### NULL en arrays

`List<scalar?>` permite NULL en elementos:

```fitz
@table("metrics") type Metric {
    @primary id: Int = 0
    name: Str
    measurements: List<Int?>    // bigint[] con NULL aceptable
}

let m = Metric.insert(db, Metric {
    id: 0,
    name: "ping",
    measurements: [10, null, 25, null, 18]
}).await?
```

Sin el `?`, `List<Int>` rechaza nulls en runtime.

---

## Paso 5 — Array operators en `.where(...)`

Tres method calls sobre fields array mapeados a operadores
Postgres:

| Method | SQL emitido | Significado |
|---|---|---|
| `p.tags.has("rust")` | `$1 = ANY("tags")` | el elem está en el array |
| `p.tags.contains_all(["rust", "postgres"])` | `"tags" @> $1::text[]` | TODOS los elems del arg están en el array |
| `p.scores.contained_in([1, 2, 3, 4, 5])` | `"scores" <@ $1::int8[]` | TODOS los elems del array están en el arg |

Ejemplos:

```fitz
// Posts con tag "rust"
let rusty = Post.where(fn(p) => p.tags.has("rust")).all(db).await?

// Posts con TANTO "rust" como "postgres"
let both = Post.where(fn(p) => p.tags.contains_all(["rust", "postgres"])).all(db).await?

// Posts cuyos scores son un subset de [1..5]
let small = Post.where(fn(p) => p.scores.contained_in([1, 2, 3, 4, 5])).all(db).await?

// Combinable con otros operadores
let curated = Post.where(fn(p) =>
    p.tags.has("featured") and
    p.scores.contains_all([100]) and
    not p.archived
).all(db).await?
```

**Caveat MVP**: los args de `.has` / `.contains_all` /
`.contained_in` requieren **literales** del tipo escalar del
array. Vars externas NO se aceptan directo:

```fitz
// ❌ ERROR — var no soportada como arg
let some_tag = "rust"
Post.where(fn(p) => p.tags.has(some_tag)).all(db).await?

// ✅ Workaround — db.query crudo con $param
let rows = db.query("SELECT * FROM posts WHERE $1 = ANY(tags)", [some_tag]).await?
```

---

## Paso 6 — `Date` / `DateTime` / `Uuid` built-in del lenguaje

A diferencia de la deuda histórica (Date/DateTime modelados
como Str ISO 8601), **post-v0.10.24** Fitz tiene tipos
**built-in nativos** para fechas y UUIDs.

### `Date` — fecha sin tiempo

```fitz
let today: Date = Date.today()
print("today: {today}")              // "2026-06-02"

let d = Date.from_ymd(2026, 12, 25)?
let parsed = Date.parse("2026-01-01")?

// Getters.
print("year: {today.year()}")         // 2026
print("month: {today.month()}")       // 6
print("day: {today.day()}")           // 2
print("weekday: {today.weekday()}")   // 0=Lunes..6=Domingo

// Aritmética.
let tomorrow = today.add_days(1)
let next_month = today.add_months(1)
let yesterday = today.subtract_days(1)

// Diff.
let diff: Int = tomorrow.diff_days(today)   // 1

// Comparison nativa.
if (today < tomorrow) {
    print("today < tomorrow")
}

// Format.
let s: Str = today.to_str()           // "2026-06-02"
let custom = today.format("%d/%m/%Y") // "02/06/2026"
```

### `DateTime` — timestamp con timezone

```fitz
let now: DateTime = DateTime.now()
print("now: {now}")                   // "2026-06-02T16:30:00Z" (siempre UTC)

let epoch = DateTime.epoch()          // 1970-01-01T00:00:00Z
let from_ts = DateTime.from_timestamp(1700000000)?

// Getters.
print("hour: {now.hour()}")
print("minute: {now.minute()}")
print("second: {now.second()}")
print("timestamp (Unix): {now.timestamp()}")

// Aritmética.
let next_hour = now.add_hours(1)
let in_5_min = now.add_minutes(5)

// Diff.
let diff_secs = next_hour.diff_seconds(now)   // 3600

// Display con timezone.
let local = now.to_local()                    // formatea en TZ del sistema
let buenos_aires: Str = now.in_tz("America/Argentina/Buenos_Aires")?
// "2026-06-02T13:30:00-03:00"

// Conversión.
let date_only: Date = now.date()
```

### `Uuid` — identificador universal

```fitz
let u: Uuid = Uuid.v4()
print("uuid: {u}")                    // "ff396d09-d71a-44c5-a93a-05e124de236e"

let v7 = Uuid.v7()                    // time-ordered (mejor para PK)
let nil = Uuid.nil()                  // "00000000-0000-0000-0000-000000000000"
let parsed = Uuid.parse("550e8400-e29b-41d4-a716-446655440000")?

// Display.
let s: Str = u.to_str()
```

### Mapping al schema Postgres

```fitz
@table("events") type Event {
    @primary id: Int = 0
    occurred_at: DateTime        // → timestamptz
    occurred_date: Date          // → date
    request_id: Uuid             // → uuid
}
```

**Round-trip transparente con Postgres**: el driver hace el
parsing/serialización con `chrono` + `uuid` automático.

### Cuándo usar Date vs DateTime vs Uuid

- **`Date`** — fecha calendárica sin hora. Cumpleaños,
  fechas de evento, deadlines diarios.
- **`DateTime`** — instante en el tiempo (siempre UTC en
  storage). Created_at, updated_at, logs, métricas.
- **`Uuid`** — IDs que no pueden ser secuenciales por razones
  de seguridad / sharding / merge. Sessions, public IDs de
  recursos, tokens.

### En queries

```fitz
@table("events") type Event {
    @primary id: Int = 0
    occurred_at: DateTime
}

// Filtro por DateTime.
let cutoff = DateTime.from_timestamp(1700000000)?
let recent = Event.where(fn(e) => e.occurred_at > cutoff).all(db).await?

// Filtro por Date (calendar day).
let today = Date.today()
let today_events = Event.where(fn(e) => e.occurred_at.date() == today).all(db).await?
//   ⚠ ↑ esta función `.date()` adentro del closure puede no estar
//     soportada por el translator MVP — bajar a db.query crudo si rompe.
```

---

## Paso 7 — Full-text search con `@@`

Dos method calls sobre fields `Str` que vienen de columnas
`tsvector`:

| Method | SQL emitido |
|---|---|
| `d.body_tsv.matches("query")` | `"body_tsv" @@ to_tsquery($1)` |
| `d.body_tsv.plainto_matches("hola")` | `"body_tsv" @@ plainto_tsquery($1)` |

Diferencia:

- **`matches("...")`** — soporta syntax avanzada de tsquery
  (`'cat & dog'`, `'cat | dog'`, `'cat:*'`). Errores → runtime
  error.
- **`plainto_matches("...")`** — text libre, Postgres convierte
  a tsquery con AND implícito. Seguro para search bars.

```fitz
@table("docs") type Doc {
    @primary id: Int = 0
    body: Str
    @column(sql_type="tsvector") body_tsv: Str
}

// Búsqueda con operators de tsquery
let advanced = Doc.where(fn(d) => d.body_tsv.matches("postgres & (full | text)")).all(db).await?

// Búsqueda del user libre (search bar)
let user_query = "fitz lang"
let user_matches = Doc.where(fn(d) => d.body_tsv.plainto_matches(user_query)).all(db).await?
```

**Ranking** (v0.10.32):

```fitz
let ranked = Doc.where(fn(d) => d.body_tsv.matches("postgres"))
    .order_by(fn(d) => -d.body_tsv.rank("postgres"))
    .limit(10)
    .all(db).await?
```

---

## Paso 8 — Programa end-to-end

```fitz
@table("events") type Event {
    @primary id: Int = 0
    name: Str
    data: Map<Str, Any>
    tags: List<Str>
    occurred_at: DateTime
}

async fn main() -> Result<Str> {
    let db = db.connect(
        env_or("DATABASE_URL", "postgres://postgres:secret@localhost:5432/fitz_curso?sslmode=disable")
    ).await?

    let _ = db.exec("DROP TABLE IF EXISTS events", []).await?
    let _ = db.exec("CREATE TABLE events (id bigserial PRIMARY KEY, name text NOT NULL, data jsonb NOT NULL, tags text[] NOT NULL, occurred_at timestamptz NOT NULL)", []).await?

    // Insert con jsonb + arrays + DateTime.
    let now = DateTime.now()
    let _ = Event.insert(db, Event {
        id: 0,
        name: "click",
        data: {"page": "/home", "ts": 1700000000, "active": true},
        tags: ["nav", "ui"],
        occurred_at: now
    }).await?
    let _ = Event.insert(db, Event {
        id: 0,
        name: "purchase",
        data: {"item": {"id": 42, "price": 19.99}, "qty": 2},
        tags: ["promo", "winter"],
        occurred_at: now
    }).await?

    // SELECT y consumo del jsonb.
    let all = Event.all(db).await?
    for e in all {
        print("{e.name}: tags={e.tags}, data.page={e.data.get(\"page\").get_or(\"N/A\")}")
    }

    // Filtro con JSON operator.
    let with_page = Event.where(fn(e) => e.data.has_key("page")).all(db).await?
    print("con key 'page': {len(with_page)}")

    // Filtro con array operator.
    let promo_events = Event.where(fn(e) => e.tags.has("promo")).all(db).await?
    print("con tag 'promo': {len(promo_events)}")

    // Combinación: JSON + array.
    let homepage_or_promo = Event.where(fn(e) =>
        e.data.contains_json({"page": "/home"}) or e.tags.has("promo")
    ).all(db).await?
    print("homepage o promo: {len(homepage_or_promo)}")

    return Ok("OK")
}

print(main().await)
```

---

## Subset compilable a binario

| Feature | `fitz run` | `fitz build` |
|---|---|---|
| `Map<Str, Any>` ↔ jsonb | ✅ | ✅ |
| `Map<Str, T>` (homogéneo) ↔ jsonb | ✅ | ✅ |
| `List<scalar>` ↔ `T[]` | ✅ | ✅ |
| `List<scalar?>` con NULL ↔ `T[]` con NULLs | ✅ | ✅ |
| `Date` built-in | ✅ | ✅ |
| `DateTime` built-in | ✅ | ✅ |
| `Uuid` built-in | ✅ | ✅ |
| `Date.today()`/`from_ymd`/`parse` | ✅ | ✅ |
| `DateTime.now()`/`epoch`/`from_timestamp` | ✅ | ✅ |
| `Uuid.v4()`/`.v7()`/`.nil()`/`.parse` | ✅ | ✅ |
| `.add_days/.add_months/.diff_days` | ✅ | ✅ |
| `.in_tz("IANA")` | ✅ | ✅ |
| `.has_key`/`.has_all_keys`/`.contains_json`/`.get` | ✅ | ✅ |
| `.has_path`/`.path_int`/`.path_text`/`.path_float`/`.path_bool` | ✅ | ✅ |
| `.has`/`.contains_all`/`.contained_in` (array ops) | ✅ | ✅ |
| `.matches`/`.plainto_matches`/`.rank` (FTS) | ✅ | ✅ |
| Vars externas en `.has`/`.contains_all` | ❌ literales required | ❌ |
| Vars externas en `.contains_json` / `.has_all_keys` | ❌ literales required | ❌ |
| Nested Maps adentro de `.contains_json({...})` | ❌ solo primitivos | ❌ |

---

## Validación

- [ ] `Map<Str, Any>` con shape heterogéneo round-trips a jsonb
      preservando tipos al SELECT.
- [ ] `List<Str>` se mapea a `text[]` y persiste `["a", "b",
      "c"]`.
- [ ] `Date.today()` devuelve la fecha actual como `Date`.
- [ ] `DateTime.now()` devuelve ahora en UTC como `DateTime`.
- [ ] `Uuid.v4()` devuelve un UUID v4 canonical.
- [ ] `.has_key("foo")` traduce a `"data" ? $1` y filtra
      correctamente.
- [ ] `.contains_json({"k": "v"})` con Map literal compila;
      con var falla en compile-time.
- [ ] `.path_int(["a", "b"])` con List literal funciona y casa
      a Int.
- [ ] `.has("rust")` con Str literal funciona; con var falla.
- [ ] `.contains_all([...])` con List literal de Str funciona.
- [ ] `Date.from_ymd(2026, 12, 25)?` parsea correctamente.
- [ ] `dt.in_tz("America/Argentina/Buenos_Aires")?` formatea
      con offset.
- [ ] `Uuid.parse("invalid")` devuelve `Err`.
- [ ] `fitz build` del programa con jsonb + arrays + DateTime
      produce binario standalone funcional contra Postgres.

---

## Troubleshooting

### `Err("invalid input syntax for type jsonb")` al insertar

El value del Map tiene algo no serializable a JSON (Function,
Future, etc.). Verificá que el Map solo contenga primitivos /
nested Maps / Lists.

### `.contains_json({...})` con var falla en compile-time

```fitz
let filter = {"page": "/home"}
Post.where(fn(p) => p.data.contains_json(filter))    // ❌
```

MVP exige Map literal directo. Workaround:

```fitz
// Hardcoded
Post.where(fn(p) => p.data.contains_json({"page": "/home"}))

// O bajar a db.query crudo
let rows = db.query("SELECT * FROM posts WHERE data @> $1::jsonb",
    [json_string]).await?
```

### `.path_int(["a", "b"]) == 0` matchea cuando el path no existe

Semántica: extract devuelve NULL si el path no existe, y en
SQL `NULL = 0` es `NULL` (NO `false`). El row se rechaza
silenciosamente en WHERE.

Para distinguir "no existe" vs "existe con valor 0":

```fitz
let with_zero = Post.where(fn(p) =>
    p.data.has_path(["score"]) and p.data.path_int(["score"]) == 0
).all(db).await?
```

### `Err("invalid input value for type 'date': ...")` al insertar

El value que pasaste no es un `Date` válido. Si estás pasando
un Str, parseálo primero:

```fitz
let d = Date.parse("2026-12-25")?       // ✅ Date típado
let _ = Event.insert(db, Event { ..., occurred_date: d, ... }).await?
```

### `Uuid` aparece como Str en queries

`Uuid` se mapea a `uuid` Postgres pero comparación se hace por
texto canonical. Si querés filtrar por UUID:

```fitz
let token: Uuid = Uuid.parse(input_str)?
let session = Session.where(fn(s) => s.token == token).first(db).await?
```

### `dt.in_tz("InvalidZone")?` falla en runtime

El nombre IANA es inválido. Lista canónica:
`https://en.wikipedia.org/wiki/List_of_tz_database_time_zones`.

### Array operator `.contains_all([1, 2, 3])` con Int falla cuando la columna es `int8[]`

Verificá que los items de la lista son Int (no Float). Postgres
es estricto con tipos de array.

### `Date.parse("2026/06/02")?` falla

`Date.parse` acepta solo ISO 8601 (`YYYY-MM-DD`). Para otros
formatos, parsear manualmente con `Date.from_ymd(y, m, d)?`.

---

## Lo que sigue

Llegaste al final del cap. Lo que cubriste:

- `Map<Str, Any>` ↔ jsonb con shape heterogéneo preservado.
- `List<scalar>` ↔ `T[]` para 12 array OIDs Postgres.
- `Date` / `DateTime` / `Uuid` built-in del lenguaje con
  constructors (`Date.today()`, `DateTime.now()`, `Uuid.v4()`),
  getters (`.year()`, `.hour()`, `.day()`), aritmética
  (`.add_days(n)`, `.diff_days(other)`), comparison nativa, y
  display tz-aware (`.in_tz("IANA")`).
- JSON operators: `.has_key`, `.has_all_keys`, `.has_any_keys`,
  `.contains_json`, `.get` mapeados a `?`, `?&`, `?|`, `@>`,
  `->>`.
- JSON path operators avanzados con cast tipado:
  `.has_path([...])`, `.path_int/.path_text/.path_float/.path_bool`.
- Array operators: `.has(elem)`, `.contains_all([...])`,
  `.contained_in([...])` mapeados a `= ANY`, `@>`, `<@`.
- Full-text search con `.matches("query")` y `.plainto_matches`
  + ranking con `.rank("query")`.

**Próximo cap**: [M6.C6 — Capstone: app CRUD completa con
auth + ORM + WS + cron + Docker](c6-capstone-crud-completo.md).
Vamos a integrar **todo lo de M1-M6** en una app real:
servidor HTTP con handlers tipados + auth con JWT + WebSockets
para notificaciones en tiempo real + cron job de mantenimiento
+ ORM con relations + Postgres en Docker. Cero cargo add, cero
pip install, deploy = un binario.
