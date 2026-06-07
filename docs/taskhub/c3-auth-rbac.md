# C3 — Auth con RBAC custom: 3 roles apilables

**Pre-requisitos**: [C2 — Schema + workflow `fitz db`](c2-schema-migraciones.md)
cerrado. Tenés las 4 tablas creadas + endpoint smoke `GET /api/users`
respondiendo `[]`. El campo `role: Str = "member"` está en el
`@table type User` esperándonos.

**Objetivo**: implementar **registro + login con JWT + Argon2id**,
declarar el `@auth_provider` que valida el bearer token contra la
DB, y aplicar **`@requires("admin")` / `@requires("owner")` /
`@requires("member")`** sobre los handlers para enforzar RBAC.
Demostrar **`@requires` apilable** (semántica OR) en un endpoint
real. Probar end-to-end con `curl` que un member no puede
promote, que el admin sí, y que un owner heredan permisos según
el rol.

**Por qué importa**: el RBAC custom apilable es **un diferencial
fuerte** del lenguaje. Stack típico Python+FastAPI / Node+Express
resuelven esto con **middleware ad-hoc** (`@require_role("admin")`
decorator definido a mano) o **dependency injection runtime**
(`Depends(get_admin_user)`). El checker no valida nada — si te
equivocás de string de rol o referenciás un usuario sin el field
`role`, te enterás en runtime. **Fitz lo valida en compile-time**:
el checker exige `role: Str` no nullable en el `User`, rechaza
`@requires` con string duplicado en apilados, y conoce qué
endpoints están detrás de cada rol — eso vuelve al spec
**verificable estáticamente**.

**Cross-link**: [Cap 28 de la guía — Auth nativa](../guide.md#28-auth-nativa)
para la referencia exhaustiva del subsistema.

---

## Mapa del cap

```mermaid
flowchart LR
    A[POST /api/auth/register] --> B[hash.password Argon2id]
    B --> C[User.insert role member]
    D[POST /api/auth/login] --> E[hash.verify]
    E --> F[jwt.encode HS256]
    F --> G[Token JWT]
    G --> H[GET /api/me Bearer]
    H --> I[@auth_provider valida]
    I --> J[User devuelto]
    K[GET /api/users] --> L[@requires admin]
    L --> M[Lista todos los users]
    N[POST /api/users/promote] --> L
    O[GET /api/stats] --> P[@requires admin + owner apilable]
```

---

## Por qué Fitz es distinto

| Feature | Spring Security | FastAPI + custom decorator | Express + middleware | **Fitz** |
|---|---|---|---|---|
| Setup auth provider | clase `UserDetailsService` + bean wiring | `Depends(get_current_user)` definido a mano | `passport.use(JwtStrategy())` + serialize | **`@auth_provider async fn check_token(...)`** built-in, singleton del programa |
| JWT signing | jjwt o spring-jwt | `python-jose` + manual sign | `jsonwebtoken` package | **`jwt.encode(claims, secret)`** built-in del lenguaje |
| Password hashing | `BCryptPasswordEncoder` | `passlib[bcrypt]` o argon2-cffi | `bcrypt` package | **`hash.password(pw)` / `hash.verify(pw, h)`** Argon2id built-in |
| Role check estático | `@PreAuthorize("hasRole('ADMIN')")` ⚠ string, no checker validation | custom decorator con runtime check | middleware con `if (req.user.role !== ...)` | **`@requires("admin")` validado por el checker estático** — exige `role: Str` no nullable en User |
| Hierarchy de roles | RoleHierarchy bean | manual en code | manual en code | **`@requires` apilable = semántica OR** (`@requires("admin") @requires("owner")` = admin O owner) |
| Hide password en JSON | `@JsonIgnore` annotation | `Field(exclude=True)` en Pydantic | manual `delete user.password` | **`@hidden`** decorator del lenguaje sobre el field |
| 401 / 403 responses | manual ExceptionHandlers | manual exception_handler | manual middleware | **automático** — `@auth_provider` falla → 401, `@requires` rol no matchea → 403 |
| Tipos del provider | manual | manual | manual | **`@auth_provider` exige `fn(Map<Str, Str>) -> Result<User>`**, validado en compile-time |
| Validación apilada | manual | manual | manual | **`@requires("admin") @requires("owner")` apilable**, parsea como OR |

**Diferencial estructural**: el RBAC en Fitz es **parte del
sistema de tipos**. El checker conoce qué endpoints requieren
`role == "admin"`, exige que `User.role` no sea nullable, rechaza
`@requires` con role duplicado, y **valida el shape del provider
en compile-time** (signature exacta `fn(Map<Str, Str>) -> Result<User>`).
En FastAPI/Express/Spring esto es runtime — un typo en el string
del rol o un campo faltante explota cuando llega la primera
request al endpoint.

---

## Paso 1 — Tipos auxiliares + `@hidden` en password_hash

Editás `src/main.fitz`. Primero agregás `@hidden` al
`password_hash` del `User` (el field sigue existiendo en la DB,
pero **no se serializa en JSON responses**):

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    @hidden password_hash: Str = ""    // ← NUEVO @hidden
    role: Str = "member"
    created_at: DateTime
}
```

**`@hidden` es solo cambio de código, no requiere migration** —
el column sigue siendo `text NOT NULL DEFAULT ''` en Postgres. Lo
que cambia es que el codegen del `__ToFitzJson` lo omite cuando
serializa el `User` a la response.

Sumás los tipos de input/output del auth flow:

```fitz
type RegisterInput {
    email: Str
    password: Str
}

type LoginInput {
    email: Str
    password: Str
}

type LoginResponse {
    token: Str
}

type PromoteInput {
    new_role: Str   // "admin" / "owner" / "member"
}
```

**Por qué tipos separados de `User`**: el `User` es el shape DB.
Los inputs HTTP tienen shapes distintos (la `password` viene en
claro del request, el `password_hash` sale al INSERT). Separar
los types **previene mezclas accidentales** (ej. devolver el
`password_hash` en la response porque alguien olvidó proyectar).

---

## Paso 2 — `@auth_provider` que valida el bearer

```fitz
let JWT_SECRET = env_or("JWT_SECRET", "dev-secret-cambiame")

// Helper para lookup por email (el UNIQUE index hace que sea barato).
async fn find_user_by_email(email: Str) -> Result<User> {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }
    return User.where(fn(u) => u.email == email).first(conn).await
}

@auth_provider
async fn check_token(headers: Map<Str, Str>) -> Result<User> {
    let auth: Str = match headers.get("authorization") {
        Ok(v) => v,
        Err(_) => return Err("falta header Authorization"),
    }
    let parts = auth.split(" ")
    if (parts.len() != 2) {
        return Err("Authorization debe ser 'Bearer <token>'")
    }
    if (parts[0] != "Bearer") {
        return Err("scheme debe ser Bearer")
    }
    let token = parts[1]

    let claims = jwt.decode(token, JWT_SECRET)?
    let email: Str = match claims.get("email") {
        Ok(v) => v,
        Err(_) => return Err("token sin claim email"),
    }

    // Lookup contra DB (no contra el claim del token) — si demoteas
    // un admin a member, la próxima request ya ve el nuevo role.
    return find_user_by_email(email).await
}
```

**Detalles importantes**:

- **`@auth_provider`** es un **singleton del programa** — solo
  podés tener uno. Si declarás dos, el checker aborta.
- **Async `fn`** porque tiene `.await` adentro (consulta a la DB).
- **Lookup por DB, no por claim del token**: esto es **importante
  por seguridad**. Si guardás `role` en el JWT y un admin se
  demote a sí mismo, el viejo token sigue valiendo como admin
  hasta que expire. Validando contra DB en cada request, la
  demotion es **inmediata**. Trade-off: más queries (cubierto por
  el UNIQUE index en email — query es O(log n)).
- **`jwt.decode(token, JWT_SECRET)?`** propaga `Err` si el token
  está malformado, signature inválida, o expirado.
- **Mensajes de error específicos** (`falta header Authorization`
  / `scheme debe ser Bearer` / `token sin claim email`) — son lo
  que el cliente recibe en el 401 response. Útiles para debug.

---

## Paso 3 — `POST /api/auth/register`

```fitz
@post("/auth/register")
async fn register(input: RegisterInput) -> Result<User> {
    if (input.email == "" or input.password == "") {
        return Err("email y password son obligatorios")
    }
    if (input.password.len() < 8) {
        return Err("password debe tener al menos 8 caracteres")
    }

    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }

    // Check email único antes de insertar — mensaje user-friendly
    // en lugar del UNIQUE violation crudo del driver.
    let existing = User.where(fn(u) => u.email == input.email).first(conn).await
    match existing {
        Ok(_)  => return Err("email ya registrado"),
        Err(_) => 0,
    }

    let pw_hash = hash.password(input.password)
    let new_user = User.insert(conn, User {
        id: 0,
        email: input.email,
        password_hash: pw_hash,
        role: "member",                   // default — solo admin promueve
        created_at: DateTime.now(),
    }).await?

    return Ok(new_user)
}
```

**Detalles**:

- **Validación temprana** (email vacío / password corto) — antes
  de tocar la DB. Mensaje de error claro al cliente.
- **`hash.password(input.password)`** corre **Argon2id** (built-in
  del lenguaje, sin deps externas). Devuelve un hash con salt
  embebido — guardás en DB tal cual, no hace falta columna
  separada para salt.
- **`role: "member"` hardcodeado** — un endpoint de registro
  público NO permite elegir role. El primer admin se elevará
  manualmente en el Paso 9.
- **`DateTime.now()`** explícito — el `@table` declara
  `created_at: DateTime` sin default. En C4 podríamos sumar
  `@db_default("NOW()")` si queremos hacerlo automático.
- **Response es `User`** pero **NO incluye `password_hash`**
  porque el field tiene `@hidden`. El cliente ve `{id, email,
  role, created_at}` solamente.

---

## Paso 4 — `POST /api/auth/login`

```fitz
@post("/auth/login")
async fn login(creds: LoginInput) -> Result<LoginResponse> {
    let user: User = match find_user_by_email(creds.email).await {
        Ok(u) => u,
        Err(_) => return Err("credenciales inválidas"),
    }
    if (not hash.verify(creds.password, user.password_hash)) {
        return Err("credenciales inválidas")
    }
    let claims = {
        "email": user.email,
        "role": user.role,
    }
    let token = jwt.encode(claims, JWT_SECRET)
    return Ok(LoginResponse { token: token })
}
```

**Detalles**:

- **Mismo mensaje "credenciales inválidas"** tanto si el email no
  existe como si el password no matchea. Esto es **mitigation
  contra timing attacks + enumeración de usuarios** (un atacante
  no puede saber si un email está registrado probando passwords
  random).
- **`hash.verify(plain, hashed)`** devuelve `Bool` (no `Result`).
  Hash malformado → `false` por seguridad (no panic).
- **`claims = {"email": ..., "role": ...}`** es `Map<Str, Str>`.
  El JWT MVP solo acepta este shape (heterogéneos requieren
  `__FitzValue` en codegen, deuda menor). Suficiente para auth.
- **`jwt.encode(claims, secret)`** firma con **HS256** por default.
  HS384/HS512 también disponibles via kwarg.

---

## Paso 5 — `GET /api/me`

```fitz
@authenticated
@get("/me")
fn me(user: User) -> User => user
```

**Una línea**. El `@authenticated` invoca el `@auth_provider`,
inyecta el `user: User` resuelto, el handler simplemente lo
devuelve. **El `@hidden` del `password_hash` aplica** — la response
omite el hash automáticamente.

---

## Paso 6 — `GET /api/users` con `@requires("admin")`

```fitz
@requires("admin")
@get("/users")
async fn list_users_admin(user: User) -> Result<List<User>> {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }
    return User.all(conn).await
}
```

**Diferencias con el endpoint smoke del C2** (`GET /api/users` sin
auth, que vamos a remover):

- **`@requires("admin")`** delante hace dos cosas:
  1. Implica `@authenticated` — corre el provider antes del handler.
  2. Valida que `user.role == "admin"`. Si no, **403 automático**
     con mensaje `forbidden: requires role 'admin', user has 'member'`
     (o el role actual).
- **`user: User`** se inyecta (mismo patrón que `@authenticated`).
- **`User.all(conn)`** devuelve todos los users, sin `password_hash`
  en la response (gracias a `@hidden`).

---

## Paso 7 — `POST /api/users/{id}/promote`

```fitz
@requires("admin")
@post("/users/{id}/promote")
async fn promote_user(id: Int, input: PromoteInput, user: User) -> Result<User> {
    // Validá el role pedido.
    if (input.new_role != "admin" and input.new_role != "owner" and input.new_role != "member") {
        return Err("new_role debe ser 'admin', 'owner' o 'member'")
    }

    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }

    // Update con guard obligatorio (where).
    let updated_count = User.where(fn(u) => u.id == id)
        .update(conn, { "role": input.new_role })
        .await?

    if (updated_count == 0) {
        return Err("user con id={id} no existe")
    }

    // Devolver el user actualizado.
    return User.where(fn(u) => u.id == id).first(conn).await
}
```

**Detalles**:

- **`@requires("admin")`** — solo admin promueve.
- **Validación del role string** antes de tocar DB. Mensaje
  específico al cliente.
- **`.update(conn, { "role": ... })`** con `where(...)` guard
  obligatorio. El ORM rechaza `.update(...)` sin where (cubierto
  en M6.C3 del curso).
- **Devolvemos el user actualizado** (no solo `Ok`) para que el
  cliente pueda verificar el nuevo role sin re-fetch.

---

## Paso 8 — `@requires` apilable: demo `GET /api/stats`

Acá demostramos la **semántica OR del decorator apilable**: admin
**O** owner pueden ver stats agregadas (member no).

```fitz
type StatsResponse {
    total_users: Int
    total_projects: Int
    total_tasks: Int
}

@requires("admin")
@requires("owner")
@get("/stats")
async fn stats(user: User) -> Result<StatsResponse> {
    let conn = match db_result {
        Ok(c) => c,
        Err(_) => return Err("db no disponible"),
    }

    let users = User.count(conn).await?
    let projects = Project.count(conn).await?
    let tasks = Task.count(conn).await?

    return Ok(StatsResponse {
        total_users: users,
        total_projects: projects,
        total_tasks: tasks,
    })
}
```

**Apilable = OR**:

- `@requires("admin") @requires("owner")` = admin **O** owner.
- Member llega → 403.
- Admin llega → 200.
- Owner llega → 200.

**Por qué OR y no AND**: un user tiene **un solo `role` `Str`** (no
lista). Pedir AND (admin Y owner) sería incoherente — nadie es
admin y owner simultáneamente. Para hierarchies más complejas
(membership en N grupos), el modelo es `User has many Roles` y se
maneja con tabla relacional + join — fuera del scope del RBAC
declarativo.

**Por qué NO `@authenticated`** acá: con `@requires("admin") @requires("owner")`,
member queda fuera (no matchea ninguno de los dos). Si quisieras
"todo logueado", usás `@authenticated` solo. Si querés "admin Y
member Y owner explícitos" (equivalente a `@authenticated` pero
declarativo), apilás los tres `@requires`.

---

## Paso 9 — Remover el endpoint smoke + Rebuild

Borrás el endpoint `GET /users` del C2 (sin auth) — está
reemplazado por `GET /users` con `@requires("admin")`. Tu
`src/main.fitz` final tiene:

```text
Endpoints C3:
- GET /healthz                      (no auth)
- POST /auth/register               (no auth)
- POST /auth/login                  (no auth)
- GET /me                           (@authenticated)
- GET /users                        (@requires("admin"))
- POST /users/{id}/promote          (@requires("admin"))
- GET /stats                        (@requires("admin") @requires("owner"))
```

**Rebuild del binario** para incorporar la lógica nueva:

```bash
docker compose up -d --build app
```

Verificación rápida:

```bash
curl http://localhost:8000/healthz
# → {"status":"ok","version":"0.1.0-c3"}

curl http://localhost:8000/api/me
# → 401 Unauthorized — sin token (esperado)

curl http://localhost:8000/api/auth/register \
  -X POST -H 'Content-Type: application/json' \
  -d '{"email":"x","password":"short"}'
# → 500 con {"error":"password debe tener al menos 8 caracteres"}
```

---

## Paso 10 — Bootstrap del primer admin

Acá viene el **chicken-and-egg**: el endpoint `/users/{id}/promote`
está protegido con `@requires("admin")`, pero **no hay admin
todavía** en la DB. Necesitás un admin para crear un admin.

**Solución pragmática**: el primer admin se eleva **manualmente
con psql** después del primer register. Una sola vez.

```bash
# 1. Hacés `source dev-env.sh` si todavía no.
source dev-env.sh

# 2. Registrás el user que va a ser admin.
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}'

# Response:
# → {"id":1,"email":"admin@taskhub.local","role":"member","created_at":"..."}
# (password_hash NO aparece — @hidden funciona)

# 3. Elevás a admin desde psql.
psql "$DATABASE_URL" -c "UPDATE users SET role='admin' WHERE id=1;"
# → UPDATE 1
```

**Otra opción** (refinamiento futuro): una `.fitz` migration nativa
que pregunte por una env var `INITIAL_ADMIN_EMAIL` y eleve ese user
al boot. Lo dejamos como deuda explícita.

---

## Paso 11 — Probar end-to-end con curl

Login como admin:

```bash
ADMIN_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}' \
  | jq -r .token)

echo $ADMIN_TOKEN
# → eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJlbWFpbCI6...
```

`GET /me` como admin:

```bash
curl http://localhost:8000/api/me -H "Authorization: Bearer $ADMIN_TOKEN"
# → {"id":1,"email":"admin@taskhub.local","role":"admin","created_at":"..."}
```

`GET /users` como admin (solo lista todos):

```bash
curl http://localhost:8000/api/users -H "Authorization: Bearer $ADMIN_TOKEN"
# → [{"id":1,"email":"admin@taskhub.local","role":"admin","created_at":"..."}]
```

Registrar un member normal:

```bash
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"bob@taskhub.local","password":"bobpass123"}'
# → {"id":2,"email":"bob@taskhub.local","role":"member","created_at":"..."}

MEMBER_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"bob@taskhub.local","password":"bobpass123"}' \
  | jq -r .token)
```

Intentar `GET /users` como member → 403:

```bash
curl -i http://localhost:8000/api/users -H "Authorization: Bearer $MEMBER_TOKEN"
# → HTTP/1.1 403 Forbidden
#   {"error":"forbidden: requires role 'admin', user has 'member'"}
```

Intentar `GET /stats` como member → 403:

```bash
curl -i http://localhost:8000/api/stats -H "Authorization: Bearer $MEMBER_TOKEN"
# → HTTP/1.1 403 Forbidden
#   {"error":"forbidden: requires role 'admin' or 'owner', user has 'member'"}
```

Admin promueve a Bob a owner:

```bash
curl -X POST http://localhost:8000/api/users/2/promote \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"new_role":"owner"}'
# → {"id":2,"email":"bob@taskhub.local","role":"owner","created_at":"..."}
```

**Re-login de Bob** (el viejo token tiene `role: member` en los
claims, pero el provider revalida contra DB en cada request — ya
es owner desde la próxima llamada):

```bash
# El viejo token sigue funcionando, el provider hace lookup en DB y
# ve el role actualizado.
curl http://localhost:8000/api/stats -H "Authorization: Bearer $MEMBER_TOKEN"
# → {"total_users":2,"total_projects":0,"total_tasks":0}
```

**Esto es la magia del lookup-contra-DB del provider**: los tokens
viejos siguen funcionando con el role actualizado. Cuando expiren,
el cliente re-loguea y obtiene un token nuevo con el claim
actualizado.

---

## Validación del cap

- [ ] `POST /api/auth/register` con email/password OK devuelve
      `User` sin `password_hash`.
- [ ] `POST /api/auth/login` con creds OK devuelve `{token: "..."}`.
- [ ] `GET /api/me` sin token → 401.
- [ ] `GET /api/me` con token → 200 con el user.
- [ ] `GET /api/users` como member → 403.
- [ ] `GET /api/users` como admin → 200 con lista.
- [ ] `POST /api/users/{id}/promote` como member → 403.
- [ ] `POST /api/users/{id}/promote` como admin → 200, user
      cambia de role.
- [ ] `GET /api/stats` como member → 403.
- [ ] `GET /api/stats` como admin → 200.
- [ ] `GET /api/stats` como owner → 200 (apilable funciona).
- [ ] El token viejo sigue funcionando después del promote (lookup
      contra DB).

---

## Troubleshooting

### `401 Unauthorized` con mensaje `token sin claim email`

El JWT no tiene el claim `email`. Causas típicas:

- Estás usando un token de otro proyecto / otro `JWT_SECRET`.
- El `jwt.encode(claims, ...)` no incluyó `email` en los claims.

Decodificá el token en [jwt.io](https://jwt.io) para inspeccionar
los claims.

### `500 Internal Server Error` en `POST /auth/register`

Lo más probable:

- El `db_result` está en estado `Err` (DB no responde). Mirá
  `docker compose logs app`.
- Estás intentando registrar un email que ya existe — el código
  debería devolver `Err("email ya registrado")` antes del INSERT,
  pero si el check falla por alguna razón, el UNIQUE constraint
  del schema tira 500.

### `403 Forbidden` cuando creés que sos admin

El JWT tiene `role: "admin"` en los claims **pero la DB tiene otro
role**. El provider hace lookup contra DB, así que el role
efectivo es el de DB. Verificá:

```bash
psql "$DATABASE_URL" -c "SELECT email, role FROM users WHERE email='admin@taskhub.local';"
```

Si dice `role: member`, hacé el `UPDATE` del Paso 10 de nuevo.

### `fitz check` aborta con `@requires requiere role: Str no nullable en User`

El `User` tiene `role: Str?` (nullable) o le falta el field. El
checker exige `role: Str` no-null. Editá el `@table type User`.

### Tests con `wscat` o frontend dicen `CORS error`

`@requires` no afecta CORS — eso vive en `@middleware(cors(...))`
que vamos a sumar en C4 cuando el frontend pegue desde
`http://localhost:8000` con orígenes diferentes.

---

## Lo que cubriste

- **`@hidden` decorator** en `password_hash` — el field está en DB
  pero no aparece en JSON responses.
- **Tipos auxiliares** separados del shape DB (`RegisterInput`,
  `LoginInput`, `LoginResponse`, `PromoteInput`).
- **`@auth_provider async fn check_token(...)`** singleton que
  valida el Bearer token, decodifica el JWT, y hace lookup
  contra DB por email.
- **`POST /api/auth/register`** con validación + `hash.password`
  Argon2id + INSERT.
- **`POST /api/auth/login`** con `hash.verify` + `jwt.encode`
  HS256 + mismo mensaje "credenciales inválidas" para evitar
  enumeración.
- **`GET /api/me`** con `@authenticated` (handler de una línea).
- **`@requires("admin")`** en handlers admin-only (lista de users
  + promote).
- **`@requires` apilable** (semántica OR) en endpoint stats:
  admin O owner.
- **Bootstrap manual del primer admin** via psql.
- **Tests end-to-end** con curl validando cada rol contra cada
  endpoint.
- **Patrón canónico del lookup contra DB** en el provider — un
  promote surte efecto inmediato sin esperar a que el token expire.

**El sistema de auth está vivo**. TaskHub ahora tiene
register + login + 3 roles + RBAC apilable. Los caps siguientes
construyen sobre esto.

---

## Próximo cap

**[C4 — CRUD + relations + WebSocket en vivo por project](c4-crud-relations-ws.md)**.

Vamos a sumar los CRUD de projects + tasks + comments con
relations (`@belongs_to` / `@has_many` para navigation methods),
eager loading con `.preload(...)`, y `@ws("/ws/projects/{id}")`
para broadcastear cambios en vivo a todos los conectados al mismo
board. El scoping por rol entra en cada handler (un owner solo ve
sus projects, un member solo ve projects donde tiene tasks
asignadas).

Mientras tanto, **commiteá este cap**. Tu repo tiene auth real
con RBAC apilable, los tests por rol pasan end-to-end, y cualquier
cap futuro que sume endpoints solo decoró con el role apropiado.
