# M6.C1 — Setup Postgres, `db.connect` y queries crudas

**Pre-requisitos**: [M5.C1 — async](../m5-async-auth-rt/c1-async-await.md).
Todo el módulo M6 usa `async fn` + `.await` para hablar con la
DB. Si te quedó alguna duda con `Future<T>`, repasalo antes de
seguir.

**Objetivo**: dejar un Postgres real corriendo, conectarse desde
Fitz con `db.connect(URL).await`, y ejecutar SQL crudo
parametrizado con `db.query` y `db.exec`. Es la base sobre la
que se monta el ORM (caps C2-C6).

**Por qué importa**: el ORM declarativo del próximo cap es
azúcar **sobre** este driver crudo. Vas a usar el ORM el 95% del
tiempo, pero el 5% restante (CTEs complejas, window functions,
JSON operators no soportados todavía, queries dinámicas
arbitrarias) lo resolvés bajando a `db.query`. Saber el shape
crudo te ahorra debugging cuando el ORM no cubre exactamente lo
que necesitás. Y conocer el driver es ver el **diferencial real
de Fitz**: el driver Postgres + el ORM viven adentro del binario,
**no son librerías externas que sumás al proyecto**.

**Cross-link**: [DB y ORM — guía exhaustiva](../../db-orm.md).

---

## Mapa del cap

```mermaid
flowchart LR
    A[Postgres corriendo] --> B["URL postgres://user:pass@host:port/db"]
    B --> C["db.connect URL .await"]
    C -->|Ok| D[DbConn pool interno]
    C -->|Err| E["Result Err connection refused"]
    D --> F["db.exec SQL params"]
    D --> G["db.query SQL params"]
    F --> H[Int rows affected]
    G --> I["List Map Str Any"]
    D --> J[db.close .await]
    D --> K[db.is_closed sync Bool]
```

---

## Por qué Fitz es distinto

| Feature | Python + asyncpg | Node + pg | Go + pgx | Rust + tokio-postgres | **Fitz** |
|---|---|---|---|---|---|
| Setup driver | `pip install asyncpg` | `npm install pg` | `go get jackc/pgx` | `cargo add tokio-postgres` | **builtin `db.connect`** |
| Sin deps externas | ❌ | ❌ | ❌ | ❌ | ✅ **driver puro Fitz/Rust** |
| Wire protocol nativo | ✅ pure Python | ⚠ libpq opcional | ✅ pure Go | ✅ pure Rust | ✅ **wire v3.0 + SCRAM-SHA-256** |
| TLS rustls (sin OpenSSL system) | ❌ libssl | ❌ libssl | ❌ libssl | ⚠ feature flag | ✅ **`rustls` puro + webpki-roots in-binary** |
| URL parsing strict | ⚠ runtime | ⚠ runtime | ⚠ runtime | ⚠ runtime | ✅ **combinaciones inválidas abortan parser** |
| Pool de conexiones built-in | ⚠ con `asyncpg.Pool` | ⚠ con `pg.Pool` | ✅ | ⚠ con `bb8`/`deadpool` | ✅ **lazy auto** |
| Compila a binario standalone | ❌ | ⚠ pkg hack | ✅ | ✅ con deps | ✅ **`fitz build` cero deps** |
| Validación estática `Result<DbConn>` | ❌ runtime | ❌ runtime | ⚠ con check | ✅ con `?` | ✅ **checker + `?` propagación** |

El diferencial mayor: **el driver es parte del binario `fitz`**.
Cuando hacés `fitz build`, el binario nativo embebe el driver
Postgres entero — wire protocol v3.0, SCRAM-SHA-256, parsers de
tipos OID, TLS con `rustls` + webpki-roots Mozilla. **Cero
`apt install libpq`, cero `cargo add tokio-postgres`, cero
runtime de Python con `pip install asyncpg`**. Deploy: copiar el
`.exe`/ELF/Mach-O al server.

---

## Paso 1 — Levantar un Postgres local

Si ya tenés Postgres corriendo (instalación nativa, Docker,
managed), saltá al Paso 2. Si no, dos caminos rápidos:

### Camino A — Docker (cross-platform, ideal para dev)

```bash
$ docker run -d --name fitz-pg \
    -e POSTGRES_PASSWORD=secret \
    -p 5432:5432 \
    postgres:16

$ docker logs fitz-pg --tail 5
... database system is ready to accept connections
```

URL para conectar:
`postgres://postgres:secret@localhost:5432/postgres?sslmode=disable`

Para parar/borrar:

```bash
$ docker stop fitz-pg && docker rm fitz-pg
```

### Camino B — Instalación nativa (Windows / macOS / Linux)

- **Windows**: instalador oficial en
  [postgresql.org/download/windows](https://www.postgresql.org/download/windows/).
  Te pregunta password al instalar.
- **macOS**: `brew install postgresql@16 && brew services start postgresql@16`.
- **Linux** (Debian/Ubuntu): `sudo apt install postgresql && sudo systemctl start postgresql`.

URL típica con password local:
`postgres://postgres:<tu-pass>@localhost:5432/postgres?sslmode=disable`.

### Crear una DB de trabajo

Vas a querer una DB separada del default `postgres` para no
mezclar con otras herramientas. Desde `psql`:

```sql
CREATE DATABASE fitz_curso;
```

O en una sola línea desde shell:

```bash
$ psql -U postgres -h localhost -c "CREATE DATABASE fitz_curso;"
```

A partir de acá la URL es
`postgres://postgres:<pass>@localhost:5432/fitz_curso?sslmode=disable`.

---

## Paso 2 — `db.connect(url).await`: abrir conexión

`db` es un **módulo built-in** del lenguaje — siempre disponible,
sin import. Su entry point es `db.connect(url)`:

```fitz
async fn main() -> Result<Str> {
    let db = db.connect(
        "postgres://postgres:secret@localhost:5432/fitz_curso?sslmode=disable"
    ).await?

    print("conectado!")
    return Ok("OK")
}

print(main().await)
```

Detalles:

- **Tipo**: `db.connect(url: Str) -> Future<Result<DbConn>>`.
- **`Result`** — la conexión puede fallar (host wrong, credenciales
  inválidas, DB no existe). Patrón canónico: `?` adentro de
  `async fn` que retorna `Result<T>`.
- **Lazy**: el `connect` arma el pool interno pero **no abre
  socket todavía**. La primera conexión TCP se levanta cuando
  llega el primer query. Esto hace que el `connect` sea barato
  incluso si después no hacés queries.
- **Pool automático**: el pool maneja N conexiones físicas a
  Postgres bajo el capot. No configurás N — el driver decide
  según la carga.

Si la URL es inválida, el `connect` falla al boot:

```bash
$ fitz run mi_app.fitz
Err("connection refused (10.0.0.99:5432)")
```

Patrón típico para apps de larga vida (server HTTP, scheduler
con cron): conectar en el top del `main()` con `?` y dejar que
el server panee si falla — ahí el operador ve el error claro al
arrancar.

---

## Paso 3 — `db.exec(sql, params)`: statements sin rows

`db.exec` es para SQL que NO retorna rows: `INSERT`, `UPDATE`,
`DELETE` sin `RETURNING`, `CREATE TABLE`, `ALTER`, `DROP`.
Devuelve la cantidad de **rows afectadas**:

```fitz
async fn main() -> Result<Str> {
    let db = db.connect("postgres://postgres:secret@localhost:5432/fitz_curso?sslmode=disable").await?

    // DDL — crear tabla si no existe.
    let _ = db.exec(
        "CREATE TABLE IF NOT EXISTS users (id bigserial PRIMARY KEY, name text NOT NULL, age bigint NOT NULL)",
        []
    ).await?

    // INSERT con parámetros $1, $2.
    let _ = db.exec(
        "INSERT INTO users (name, age) VALUES ($1, $2)",
        ["ada", 35]
    ).await?

    // UPDATE — devuelve N rows afectadas.
    let affected = db.exec(
        "UPDATE users SET age = age + 1 WHERE name = $1",
        ["ada"]
    ).await?
    print("filas actualizadas: {affected}")

    return Ok("OK")
}

print(main().await)
```

**Tres detalles importantes**:

1. **Parámetros positional `$1`, `$2`, ...** — Postgres usa este
   sintaxis (no `?` como MySQL ni `:name` como Oracle). El
   segundo arg de `exec`/`query` es la `List<Any>` con valores
   en orden.

2. **Strings SQL en una sola línea**. El lexer de Fitz NO acepta
   strings multilínea sin escape. Si el SQL es largo, podés
   romperlo con concatenación:

   ```fitz
   let sql = "CREATE TABLE users ("
       + "id bigserial PRIMARY KEY, "
       + "name text NOT NULL, "
       + "age bigint NOT NULL"
       + ")"
   let _ = db.exec(sql, []).await?
   ```

   O guardarlo como `let` separado de varios bloques.

3. **`CREATE TABLE IF NOT EXISTS` al boot** es el patrón
   idempotente más simple y cubre dev local + apps chicas. Para
   cambios de schema versionados (agregar/borrar columns en
   producción, rollback, audit log, drift check para CI), Fitz
   tiene un subcomando dedicado `fitz db` con workflow completo
   — lo vemos en [M6.C6 — Migraciones con `fitz db`](c6-migraciones-fitz-db.md).

### Tipos de parámetros aceptados

Lo que pasás en la lista de params se auto-coerciona a tipo
Postgres:

| Tipo Fitz | Tipo Postgres |
|---|---|
| `Int` | `bigint` (`int8`) |
| `Float` | `double precision` (`float8`) |
| `Str` | `text` |
| `Bool` | `bool` |
| `Null` | `NULL` |
| `List<Int>` | `int8[]` |
| `List<Str>` | `text[]` |
| `Map<Str, Any>` | `jsonb` |

Heterogéneos en la lista van bien — cada elemento se coerce
individualmente:

```fitz
let _ = db.exec(
    "INSERT INTO eventos (user_id, name, payload, tags) VALUES ($1, $2, $3, $4)",
    [42, "click", {"page": "home"}, ["nav", "ui"]]
).await?
```

---

## Paso 4 — `db.query(sql, params)`: SELECT con rows

`db.query` es la lectura cruda. Devuelve cada fila como un
`Map<Str, Any>` con las columnas como keys:

```fitz
async fn main() -> Result<Str> {
    let db = db.connect("postgres://postgres:secret@localhost:5432/fitz_curso?sslmode=disable").await?

    let rows = db.query("SELECT id, name, age FROM users WHERE age > $1", [30]).await?
    print("rows = {len(rows)}")

    // Cada row es `DbRow` — accesos tipados con `.get_int/str/...`.
    let r = rows[0]
    print("primero: {r}")

    return Ok("OK")
}

print(main().await)
```

Salida típica:

```
rows = 1
primero: {"id": 1, "name": "ada", "age": 35}
```

**Acceso a campos del row** — typed accessors con `?` para
desempacar el `Result<T>`:

```fitz
let r = rows[0]

let id: Int    = r.get_int("id")?
let name: Str  = r.get_str("name")?
let age: Int   = r.get_int("age")?
```

Métodos disponibles sobre `DbRow` (v0.10.22+):

| Método | Retorna | Cuándo falla |
|---|---|---|
| `r.get_int(col)` | `Result<Int>` | NULL, columna no existe, o el tipo PG no es int |
| `r.get_str(col)` | `Result<Str>` | NULL, no existe; acepta `text`/`varchar`/`uuid`/`json`/etc. |
| `r.get_float(col)` | `Result<Float>` | NULL, no existe, o el tipo PG no es float |
| `r.get_bool(col)` | `Result<Bool>` | NULL, no existe, o el tipo PG no es bool |
| `r.len()` | `Int` | nunca falla — cantidad de cols |

**Por qué tipado**: en runtime Postgres distingue `int8` vs `text`
vs `bool`. Si pedís `r.get_int("name")` cuando `name` es text, el
`?` propaga `Err("col 'name' no es int")` y queda explícito en el
flujo. Sin las typed accessors tendrías que castear a mano (y
si te equivocás, panic runtime opaco).

**Cuándo NO necesitás esto**: el patrón normal para handlers HTTP
es declarar el response como `List<Map<Str, Any>>` directo y dejar
que el codegen serialize a JSON automático — cada row pasa a
`{"col": val, ...}` sin que toques los typed accessors:

```fitz
@get("/users")
async fn list_users() -> Result<List<Map<Str, Any>>> {
    return db.query("SELECT id, name, age FROM users", []).await
}
```

### Loop sobre filas

```fitz
let rows = db.query("SELECT name, age FROM users ORDER BY age", []).await?
for row in rows {
    let name: Str = row.get_str("name")?
    let age: Int = row.get_int("age")?
    print("{name} tiene {age}")
}
```

---

## Paso 5 — Cerrar y chequear estado

Dos métodos extra para gestionar el ciclo de vida de la conn:

### `db.close() -> Future<Result<Null>>`

Cierra el pool y libera las conexiones físicas a Postgres:

```fitz
let _ = db.close().await?
```

Detalles:

- **Idempotente** — llamar 2 veces no es error.
- Queries posteriores fallan con `Err("db cerrada")`.
- Para apps de larga vida (server HTTP), **no llames close**
  manualmente — el shutdown del proceso libera el pool. Lo usás
  típicamente en scripts CLI cortos.

### `db.is_closed() -> Bool`

Sync. Devuelve `true` si la conn fue cerrada con `.close()`.
Útil para checks defensivos:

```fitz
if (db.is_closed()) {
    return Err("db cerrada — reconectar antes de continuar")
}
```

---

## Paso 6 — TLS strict para Postgres managed (Heroku/RDS/Supabase/Neon)

Cuando deployás contra un Postgres administrado (no localhost),
el server **exige TLS**. La URL acepta el parámetro `sslmode`
estándar de Postgres:

| `sslmode` | TLS | Verifica cert | Verifica hostname | Cuándo |
|---|---|---|---|---|
| `disable` | ❌ | — | — | Local dev sin TLS |
| `require` | ✅ | ❌ | ❌ | Dev/staging interno (NO prod — vulnerable a MITM) |
| `verify-ca` | ✅ | ✅ | ❌ | Cert custom con CN/SAN diferente al hostname |
| `verify-full` | ✅ | ✅ | ✅ | **Recomendado para producción** |

```fitz
// Supabase / Neon / RDS — TLS verify-full
let db = db.connect(
    "postgres://user:pass@db.proyecto.supabase.co:5432/postgres?sslmode=verify-full"
).await?

// Postgres interno con CA corporativa
let db = db.connect(
    "postgres://user:pass@db.intra:5432/myapp?sslmode=verify-full&sslrootcert=/etc/ssl/corp-ca.pem"
).await?
```

**Detalles técnicos**:

- TLS implementado con `rustls` puro Rust + `webpki-roots`
  (Mozilla CA bundle in-binary). **Cero deps del sistema** — no
  requiere `libssl`, `SecTransport` ni `SChannel`.
- `sslrootcert` acepta un PEM custom (CA corporativa). Path
  relativo al CWD del proceso.
- **Combinaciones inválidas abortan el parser de URL** (no
  esperan a runtime):
  - `sslmode=disable&sslrootcert=...` (cert sin uso).
  - `sslmode=require&sslrootcert=...` (require no verifica
    nada).
  - `sslrootcert=...` sin `sslmode=` (cert sin contexto).

**Out of scope MVP** (deuda visible):

- `sslmode=prefer`/`allow` (negociación dinámica). Vulnerable a
  TLS-stripping MITM — usá `disable` o `verify-full` explícito.
- Client cert auth (`sslcert`/`sslkey`/`sslpassword`). Patrón
  enterprise raramente usado vs TLS server-only.

---

## Paso 7 — Patrón típico en handler HTTP

Combinando con M4-M5, así se ve un handler que lee de la DB:

```fitz
let db = db.connect(
    env_or("DATABASE_URL", "postgres://postgres:secret@localhost:5432/fitz_curso?sslmode=disable")
).await

@server(3000)
fn main() => 0

@get("/users")
async fn list_users() -> Result<List<Map<Str, Any>>> {
    let conn = match db {
        Ok(c) => c,
        Err(e) => return Err("db no disponible: {e}"),
    }
    let rows = conn.query("SELECT id, name, age FROM users ORDER BY id", []).await?
    return Ok(rows)
}

@get("/users/count")
async fn count() -> Result<Int> {
    let conn = match db {
        Ok(c) => c,
        Err(e) => return Err("db no disponible: {e}"),
    }
    let rows = conn.query("SELECT COUNT(*)::bigint AS n FROM users", []).await?
    let r = rows[0]
    let n: Int = r.get_int("n")?
    return Ok(n)
}
```

**Lo que pasa**:

1. La conn se abre **una vez** al boot del programa (top-level
   `let db = ...`).
2. Los handlers la **reusan en paralelo** — el pool interno
   soporta N requests simultáneas.
3. `env_or("DATABASE_URL", "default")` lee la env var en runtime
   con fallback a la URL local. Convención 12-factor.

Para el ORM declarativo (caps C2+), no vas a escribir SQL crudo
así — vas a hacer `User.where(...).all(db).await?`. Pero el
shape de "abrir db top-level, pasar a cada handler" es el mismo.

---

## Validación

Probá un programa minimal end-to-end:

```fitz
// hello-db.fitz
async fn main() -> Result<Str> {
    let db = db.connect("postgres://postgres:secret@localhost:5432/fitz_curso?sslmode=disable").await?

    let _ = db.exec("CREATE TABLE IF NOT EXISTS hello (id bigserial PRIMARY KEY, msg text NOT NULL)", []).await?
    let _ = db.exec("INSERT INTO hello (msg) VALUES ($1)", ["primer row desde Fitz"]).await?

    let rows = db.query("SELECT id, msg FROM hello ORDER BY id DESC LIMIT 1", []).await?
    let r = rows[0]
    let msg: Str = r.get_str("msg")?
    print("último row: {msg}")

    return Ok("OK")
}

print(main().await)
```

```bash
$ fitz run hello-db.fitz
último row: primer row desde Fitz
Ok("OK")
```

Si esto anda, el setup está OK. Si no:

- [ ] Postgres está corriendo (`psql -U postgres -h localhost -c "SELECT 1;"`).
- [ ] La URL tiene el password correcto.
- [ ] La DB `fitz_curso` existe (`CREATE DATABASE fitz_curso;`).
- [ ] El `sslmode` matchea con la config del server (`disable`
      para local; `verify-full` para managed).
- [ ] El programa está adentro de un `async fn main()` que
      retorna `Result<...>` — `?` no funciona top-level.

---

## Troubleshooting

### `Err("connection refused (...)")`

El driver no pudo abrir socket al host:port. Verificá:

1. Postgres está corriendo — `psql -U postgres -h localhost -c "SELECT 1;"`.
2. El puerto correcto. Default 5432, pero containers a veces
   exponen otros.
3. Si es Docker, el container está running:
   `docker ps | grep postgres`.

### `Err("password authentication failed for user 'X'")`

El user/password de la URL no matchea con la config de
Postgres. Cuidado con:

- Caracteres especiales en el password (URL-encode `@`, `:`, `/`
  como `%40`, `%3A`, `%2F`).
- Postgres `pg_hba.conf` con auth method distinto (`peer` solo
  desde el user del SO, etc.).

### `Err("database 'X' does not exist")`

Le pasaste un nombre de DB que no existe. Creala con
`CREATE DATABASE X;` o cambiá la URL.

### `Err("relation 'X' does not exist")` al hacer query

La tabla no existe. `CREATE TABLE IF NOT EXISTS X (...)` al boot
es el patrón idempotente — agregalo antes del primer query.

### Error del lexer "String sin cerrar"

Tenés un string SQL con saltos de línea sin escape. Fitz NO
soporta strings multilínea raw — opciones:

1. SQL en una sola línea (verbose pero claro).
2. Concatenación con `+`:
   ```fitz
   let sql = "CREATE TABLE x ("
       + "id bigserial PRIMARY KEY,"
       + "name text"
       + ")"
   ```
3. Escape `\n` adentro del string (raro para SQL — el lexer no
   parseará el `\n` como newline real, lo dejará literal).

### `db.query` retorna rows pero `r["col"]` no compila

El checker reporta `el tipo `DbRow` no soporta indexing con `[]``.
A diferencia de `Map<Str, Any>`, `DbRow` es un wrapper opaco con
**accessors tipados** explícitos para forzarte a pensar el tipo
de cada columna en compile-time:

```fitz
// ❌ NO compila
let name = r["name"]

// ✅ Patrón correcto
let name: Str = r.get_str("name")?
let age: Int  = r.get_int("age")?
```

Razón de diseño: en Postgres los tipos son estrictos en runtime
(int8 ≠ text ≠ jsonb). Forzando el accessor tipado el checker
te avisa cuando el tipo declarado del binding no matchea el del
accessor, y el `?` te obliga a manejar el caso `Err` cuando la
columna es NULL, falta, o tiene tipo PG distinto.

### Connect lento en el primer query (no en `connect`)

Esperado. `db.connect` es **lazy** — solo arma el pool. La
primera conexión TCP + handshake SCRAM se levanta cuando llega
el primer `query`/`exec`. Si querés "calentar" el pool, hacé
un `db.query("SELECT 1", []).await?` justo después del connect.

### `Err("SSL connection error: ...")` con managed Postgres

Cuando deployás contra Heroku/RDS/Supabase/Neon, exigen TLS.
Cambiá la URL a `sslmode=verify-full`:

```fitz
let db = db.connect(
    "postgres://user:pass@host:5432/db?sslmode=verify-full"
).await?
```

Si el server tiene cert auto-firmado, podés bajar a `require`
(NO seguro para prod) o pasar `sslrootcert=path/to/ca.pem`.

### `fitz build` falla buscando libpq

Esto NO debería pasar — el driver de Fitz no linkea contra
libpq. Si ves el error, abrir issue: es un bug del codegen.

---

## Lo que sigue

Llegaste al final del cap. Lo que cubriste:

- Levantar Postgres local con Docker o instalación nativa.
- Conectarse con `db.connect(url).await?` — URL formato estándar
  Postgres + pool lazy automático.
- `db.exec(sql, params)` para statements sin rows (INSERT/UPDATE/
  DELETE/DDL) — devuelve rows afectadas.
- `db.query(sql, params)` para SELECT — devuelve `List<Map<Str,
  Any>>` con acceso por nombre de columna.
- Parámetros positional `$1`, `$2` con auto-coerción de tipos
  Fitz → Postgres (incluye `List<scalar>` → `T[]` y `Map<Str,
  Any>` → `jsonb`).
- `db.close()` + `db.is_closed()` para gestión de ciclo de vida.
- TLS strict con `sslmode=verify-full` para Postgres managed,
  implementado con `rustls` + webpki-roots sin deps del sistema.

**Próximo cap**: [M6.C2 — Mapping con `@table`, `@primary` y
métodos del ORM](c2-table-decoradores-reads.md). Vamos a montar
el ORM declarativo sobre `type`: `@table("users") type User
{...}` + `User.all(db)`/`first`/`where(closure)`/`count`. El
SQL deja de ser strings y empieza a ser **expresiones tipadas**
que el checker valida en compile-time.
