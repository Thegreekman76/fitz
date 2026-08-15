# M7.C3 — SQLAlchemy interop, bridge async, y cuándo NO usarlo

**Pre-requisitos**: [M7.C2 — numpy + pandas](c2-numpy-pandas-data-analysis.md)
y, fundamental, [M6.C2 — `@table`, `@primary` y ORM nativo](../m6-postgres-orm/c2-table-decoradores-reads.md).
Necesitás conocer los dos paths (ORM nativo Fitz y SQLAlchemy) para
entender el trade-off real.

**Objetivo**: usar SQLAlchemy desde Fitz vía interop para queries
contra Postgres. Cubrir el patrón canónico `<py_async_fn>?.await` para
SQLAlchemy 2.x async. Y, lo más importante, **darte una matriz de
decisión honesta**: dado que Fitz tiene ORM nativo, ¿cuándo conviene
SQLAlchemy?

**Por qué importa**: este cap cierra el círculo del módulo. M6 te
mostró el ORM nativo con todas sus ventajas (codegen-time SQL, paridad
bit-a-bit `run`↔`build`, validación estática, deployment standalone).
SQLAlchemy es el ORM más maduro del mundo Python con 15+ años de
ecosistema (alembic, sqlalchemy-utils, geoalchemy, etc.). Hay casos
legítimos donde SQLAlchemy gana y casos donde el ORM nativo gana.
Si elegís mal, el costo (rewrite, performance, deployment complexity)
es alto. **Este cap te enseña a elegir bien**.

**Cross-link**: [cap 21.8 de la guía — `fitz py-types`](../../guide.md#218-fitz-py-types-auto-mapeo-de-modelos-sqlalchemy)
y [cap 21.9 — Async bridge](../../guide.md#219-async-await-sobre-corutinas-python).

---

## Mapa del cap

```mermaid
flowchart LR
    A["Decisión: ORM nativo vs SQLAlchemy"] --> B{Casos}
    B -->|Legacy schema| C["SQLAlchemy via interop"]
    B -->|Greenfield + sin Python existente| D["ORM nativo Fitz"]
    C --> E["pip install sqlalchemy psycopg2"]
    E --> F["fitz py-types models.py"]
    F --> G["type User Fitz generado"]
    G --> H["from python import db_helpers"]
    H --> I["@get(/users) Fitz"]
    I --> J["users_async_fn() ?.await"]
    J --> K["asyncio bridge"]
    K --> L["SELECT WHERE"]
    L --> M["list of dict"]
    M --> N["coerción a List User"]
```

---

## Por qué Fitz es distinto: matriz de decisión

| Criterio | ORM nativo Fitz | SQLAlchemy via interop | **Cuándo elegir cuál** |
|---|---|---|---|
| **Tu situación: ¿stack greenfield o legacy DB?** | Greenfield, app nueva | Legacy con SQLAlchemy ya modelado | Legacy → SQLAlchemy. Greenfield → ORM nativo. |
| **Performance — SQL build time** | Codegen-time (compile) — comparable a Diesel/sqlx | Runtime building via Python | Latencia crítica → ORM nativo. |
| **Deployment standalone** | ~30 MB binario, cero Python | ~250 MB con venv + libs | Edge/embedded → ORM nativo. K8s/cloud con Python imagen → cualquiera. |
| **Validación estática del schema** | Compile-time (typos detectados en `fitz check`) | Runtime (AttributeError al hacer `user.naem`) | Refactors masivos → ORM nativo. |
| **Schema migrations** | `fitz db diff`/`migrate` built-in | Alembic (más maduro, hooks ricos) | Migrations complejas con data backfill → Alembic. |
| **Tooling alrededor** | Joven (~1 año), MVP completo | 15+ años, ecosistema masivo (geoalchemy, sqlalchemy-utils, etc.) | Necesitás extensions raras (GIS, JSON nested ops, custom types) → SQLAlchemy. |
| **Equipo Python existente** | Curva de aprendizaje Fitz | Familiar para devs Python | Equipo Python que no aprenderá Fitz → SQLAlchemy. |
| **Multi-DB transactions** | Single-DB MVP | Cross-engine con `with_engines(...)` | Si necesitás transaction cross-Postgres-MySQL → SQLAlchemy. |
| **Triggers / stored procedures custom** | `db.exec` SQL crudo manual | SQLAlchemy con Custom Compilers | Stored procs Postgres con tipos custom → SQLAlchemy o `db.exec`. |
| **CTEs y window functions complejas** | `db.query` SQL crudo (escape hatch) | SQLAlchemy `Query.with_cte(...)` ergonómico | Queries analíticas pesadas → SQLAlchemy si el equipo ya las domina. |

**Regla heurística pragmática**:

- **Si arrancás greenfield (app nueva, sin equipo Python legacy) → ORM
  nativo Fitz**. Vas a ahorrar deployment complexity, vas a tener
  binarios chicos, y vas a tener compile-time guarantees.
- **Si llegás a Fitz con SQLAlchemy ya en producción** (legacy DB
  modelado, alembic migrations corriendo, equipo Python que mantiene
  los modelos) → **SQLAlchemy via interop**. Reescribir todo es
  fricción gratuita.
- **Si necesitás features muy específicas que el ORM nativo no cubre
  todavía** (geoalchemy para GIS, multi-DB tx, JSON nested ops Postgres
  no soportados aún) → **SQLAlchemy**.

---

## Paso 1 — Setup: SQLAlchemy + asyncpg en el venv

```bash
(venv) $ pip install "sqlalchemy[asyncio]>=2.0" asyncpg
Collecting sqlalchemy
  Downloading SQLAlchemy-2.0.36-...-manylinux_2_17_x86_64.whl (3.2 MB)
Collecting asyncpg
  Downloading asyncpg-0.30.0-...-manylinux_2_17_x86_64.whl (3.4 MB)
...
Successfully installed sqlalchemy-2.0.36 asyncpg-0.30.0
```

> **`asyncpg` vs `psycopg2`**: para SQLAlchemy 2.x async, `asyncpg` es
> el driver Postgres recomendado (más rápido, async-native). Para
> SQLAlchemy 1.x o sync, `psycopg2-binary` sigue siendo el default.
> Este cap usa el path async porque es donde Fitz brilla con su
> bridge (Fase 8.6).

Necesitás Postgres corriendo (del M6.C1). Si lo perdiste:

```bash
$ docker run -d --name fitz-pg \
    -e POSTGRES_PASSWORD=secret \
    -p 5432:5432 \
    postgres:16
```

---

## Paso 2 — Modelos SQLAlchemy

Crear `models.py` con los `User` y `Order` legacy:

```python
# models.py — modelos SQLAlchemy 2.x.
from sqlalchemy import Integer, String, ForeignKey, DateTime, func
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column, relationship
from datetime import datetime
from typing import List


class Base(DeclarativeBase):
    pass


class User(Base):
    __tablename__ = "users"

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    name: Mapped[str] = mapped_column(String(100))
    email: Mapped[str] = mapped_column(String(200), unique=True)
    created_at: Mapped[datetime] = mapped_column(DateTime, server_default=func.now())

    orders: Mapped[List["Order"]] = relationship(back_populates="user")


class Order(Base):
    __tablename__ = "orders"

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    user_id: Mapped[int] = mapped_column(ForeignKey("users.id"))
    total_cents: Mapped[int] = mapped_column(Integer)
    created_at: Mapped[datetime] = mapped_column(DateTime, server_default=func.now())

    user: Mapped["User"] = relationship(back_populates="orders")
```

Probá standalone:

```bash
(venv) $ python3 -c "from models import User, Order; print(User.__tablename__, Order.__tablename__)"
users orders
```

---

## Paso 3 — `fitz py-types` para generar types Fitz

Acá viene el primer diferencial real de Fitz contra otros lenguajes
con interop: **`fitz py-types` lee tus modelos SQLAlchemy y emite los
`type` Fitz equivalentes**. Vos no escribís el doble-tipado.

```bash
(venv) $ fitz-python py-types models.py
// Generado por `fitz py-types` (Fase 8.5)
// Source: models.py
type User {
    id: Int = 0,
    name: Str = "",
    email: Str = "",
    created_at: Str = ""
}

type Order {
    id: Int = 0,
    user_id: Int = 0,
    total_cents: Int = 0,
    created_at: Str = ""
}
```

Mapping aplicado (cap 21.8 de la guía):

- `Integer`/`BigInteger` → `Int`
- `String`/`Text`/`VARCHAR` → `Str`
- `Float`/`Numeric` → `Float`
- `Boolean` → `Bool`
- `DateTime`/`Date`/`Time` → `Str` (ISO 8601 placeholder — Fitz tiene
  `Date`/`DateTime` nativos desde v0.10.30, pero `py-types` los mapea
  como Str por compatibilidad con el round-trip Python)

Volcalo a un archivo Fitz:

```bash
(venv) $ fitz-python py-types models.py --out models.fitz
✓ types escritos a models.fitz
```

Ahora `models.fitz` está commiteable. Vas a importarlo con
`from models import User, Order` paralelo a cualquier módulo Fitz
nativo.

> **Política "regeneración manual"** (Fase 8.5): NO hay un linter de
> drift entre `models.py` y `models.fitz`. Si cambiás el modelo
> SQLAlchemy, regenerá manualmente con `fitz py-types`. Refinable en
> sub-paso futuro si entra demanda.

---

## Paso 4 — Helpers SQLAlchemy async

SQLAlchemy 2.x con `asyncio` necesita un `AsyncEngine` y un
`AsyncSession`. Creamos `db_helpers.py`:

```python
# db_helpers.py — wrappers async para que Fitz los consuma.
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession
from sqlalchemy.orm import sessionmaker, selectinload
from sqlalchemy import select
from models import Base, User, Order

DATABASE_URL = "postgresql+asyncpg://postgres:secret@localhost:5432/postgres"

_engine = create_async_engine(DATABASE_URL, echo=False)
_SessionFactory = sessionmaker(
    _engine, expire_on_commit=False, class_=AsyncSession
)


async def init_schema():
    """Crea las tablas si no existen. Idempotente."""
    async with _engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)
    return True


async def list_users():
    """SELECT * FROM users. Devuelve list[dict] para marshaling."""
    async with _SessionFactory() as session:
        result = await session.execute(select(User))
        users = result.scalars().all()
        return [
            {
                "id": u.id,
                "name": u.name,
                "email": u.email,
                "created_at": u.created_at.isoformat(),
            }
            for u in users
        ]


async def get_user_with_orders(user_id: int):
    """SELECT + eager load relations. Devuelve dict con orders nested."""
    async with _SessionFactory() as session:
        result = await session.execute(
            select(User).where(User.id == user_id).options(selectinload(User.orders))
        )
        user = result.scalar_one_or_none()
        if user is None:
            raise LookupError(f"user {user_id} not found")
        return {
            "id": user.id,
            "name": user.name,
            "email": user.email,
            "created_at": user.created_at.isoformat(),
            "orders": [
                {"id": o.id, "total_cents": o.total_cents, "created_at": o.created_at.isoformat()}
                for o in user.orders
            ],
        }


async def create_user(name: str, email: str):
    """INSERT a users. Devuelve el id."""
    async with _SessionFactory() as session:
        user = User(name=name, email=email)
        session.add(user)
        await session.commit()
        await session.refresh(user)
        return user.id
```

> **Por qué cada helper devuelve `dict` o `list[dict]`**: SQLAlchemy
> entities (`User`/`Order`) son objetos Python opacos para Fitz. Para
> que Fitz pueda marshalear a `Map<Str, Any>`/`List<...>` y coercionar
> a `type` nominal, convertimos explícito en el lado Python.

Probá standalone:

```bash
(venv) $ python3 -c "import asyncio; from db_helpers import init_schema; print(asyncio.run(init_schema()))"
True

(venv) $ python3 -c "import asyncio; from db_helpers import create_user; print(asyncio.run(create_user('Ada', 'ada@example.com')))"
1
```

---

## Paso 5 — Llamar SQLAlchemy desde Fitz con bridge async

Acá viene el segundo diferencial: **el bridge tokio↔asyncio**. Las
fns async de SQLAlchemy son corutinas Python. Para llamarlas desde
Fitz, usamos el patrón canónico `<py_call>?.await`:

```fitz
// app.fitz
from python import db_helpers
from models import User

async fn ensure_schema() -> Result<Bool> {
    return db_helpers.init_schema()?.await
}

async fn fetch_users() -> Result<List<User>> {
    let raw = db_helpers.list_users()?.await
    return Ok(raw)  // marshaling List<Map> → List<User> automático
}

let _ = ensure_schema().await
let users = fetch_users().await
match users {
    Ok(us) => {
        for u in us {
            print("user {u.id}: {u.name} <{u.email}>")
        }
    },
    Err(msg) => print("falló: {msg}")
}
```

```bash
(venv) $ fitz-python run app.fitz
user 1: Ada <ada@example.com>
```

Qué pasó en el bridge async (Fase 8.6 decisión):

1. `db_helpers.list_users()` — Fitz invoca la fn Python. SQLAlchemy
   devuelve una corutina (porque `list_users` es `async def`).
2. Detección automática: Fitz ve que el return es awaitable, lo
   envuelve en `Value::Future` adentro del `Result::Ok`.
3. El `?` desempaca el `Result` — si falló al construir la corutina,
   `Result::Err` con la excepción Python.
4. El `.await` ejecuta la corutina. Fitz hace `tokio::task::spawn_blocking`
   + `asyncio.new_event_loop().run_until_complete(coro)` adentro del
   worker thread (baseline blocking, Fase 8.6 — refinable con
   `pyo3-async-runtimes` cuando se libere la deuda).
5. El resultado del await es el `list[dict]` SQLAlchemy → marshalled
   a `List<Map<Str, Any>>` → coercionado a `List<User>` por la
   anotación destino.

> **Patrón canónico `<py_call>?.await`**: el `?` antes del `.await` es
> obligatorio en Fitz para construir la corutina (excepciones del
> await llegan adentro del Result). El `.await` después del `?`
> ejecuta la corutina.

---

## Paso 6 — Handler HTTP con SQLAlchemy

Combinamos en un servicio completo:

```fitz
// app.fitz
from python import db_helpers
from models import User

type UserWithOrders {
    id: Int,
    name: Str,
    email: Str,
    created_at: Str,
    orders: List<Order>
}

type Order {
    id: Int,
    total_cents: Int,
    created_at: Str
}

type CreateUserInput {
    name: Str,
    email: Str
}

@get("/users")
async fn list_users() -> Result<List<User>> {
    return db_helpers.list_users()?.await
}

@get("/users/{id}")
async fn get_user(id: Int) -> Result<UserWithOrders> {
    return db_helpers.get_user_with_orders(id)?.await
}

@post("/users")
async fn create_user(body: CreateUserInput) -> Result<Map<Str, Int>> {
    let new_id = db_helpers.create_user(body.name, body.email)?.await
    return Ok({"id": new_id})
}

@server(3000)
fn main() => 0
```

```bash
(venv) $ fitz-python run app.fitz

# en otra terminal:
$ curl -X POST localhost:3000/users \
       -H "Content-Type: application/json" \
       -d '{"name":"Lovelace","email":"ada@compsci.org"}'
{"id":2}

$ curl localhost:3000/users
[{"id":1,"name":"Ada","email":"ada@example.com","created_at":"2026-06-03T18:45:23"},
 {"id":2,"name":"Lovelace","email":"ada@compsci.org","created_at":"2026-06-03T19:12:08"}]

$ curl localhost:3000/users/1
{"id":1,"name":"Ada","email":"ada@example.com","created_at":"...","orders":[]}

$ curl -i localhost:3000/users/999 | head -1
HTTP/1.1 500 Internal Server Error
{"error":"LookupError: user 999 not found"}
```

Tenés:

- API REST tipada sobre Postgres.
- SQLAlchemy 2.x async manejando las queries reales.
- Anotaciones Fitz (`User`, `UserWithOrders`) coerciónan los `dict`
  Python a `type` nominal con validación.
- Excepciones Python (`LookupError`) → 500 con mensaje claro.

---

## Paso 7 — Comparativa side-by-side con ORM nativo

Para el caso de "list users", el ORM nativo Fitz sería:

```fitz
// app_nativo.fitz — usando ORM nativo de Fitz (M6).
@table("users") type User {
    @primary id: Int = 0,
    name: Str = "",
    email: Str = "",
    created_at: Str = ""
}

@get("/users")
async fn list_users() -> Result<List<User>> {
    return User.all(db).await
}

@server(3000)
fn main() {
    let db = db.connect("postgres://postgres:secret@localhost:5432/postgres").await?
}
```

Diferencias:

| Aspecto | SQLAlchemy interop | ORM nativo Fitz |
|---|---|---|
| LoC totales | 4 archivos (models.py + db_helpers.py + models.fitz + app.fitz) ~120 LoC | 1 archivo (app.fitz) ~30 LoC |
| Setup runtime | venv + sqlalchemy + asyncpg | nada — driver embebido |
| Latencia query simple | ~5-10ms (bridge + SQLAlchemy) | ~1-2ms (driver directo) |
| Validación estática del query | ❌ runtime AttributeError | ✅ compile-time |
| Deployment | binario + venv + Python | binario solo |
| Schema migrations | Alembic (rica) | `fitz db diff/migrate` (joven, MVP) |

**Para una app greenfield el ORM nativo gana en casi todo**. Para
legacy con SQLAlchemy modelado, la interop es lo que te evita
re-escribir.

---

## Paso 8 — El veredicto honesto

Tres preguntas para decidir:

1. **¿Tu equipo y tu DB ya usan SQLAlchemy en producción?** Si sí, la
   interop te ahorra una migración masiva. Si no, ORM nativo.

2. **¿Tu deployment target permite Python instalado?**
   - Servers tradicionales (VPS, K8s) → cualquiera.
   - Edge functions (Fly machines, Lambda) → ORM nativo gana
     (deployment más simple).
   - Embedded / appliances → ORM nativo obliga.

3. **¿Necesitás features que el ORM nativo no cubre todavía?**
   - GIS (geoalchemy) → SQLAlchemy.
   - Multi-DB tx, custom types Postgres avanzados → SQLAlchemy.
   - Migrations con data backfill → Alembic suele ganar a `fitz db
     migrate` MVP.
   - JSON ops Postgres complejos → ambos cubren básicas; SQLAlchemy
     más expresiva para nested ops.

Si las 3 respuestas son "no" o "ORM nativo cubre", usá ORM nativo. Si
una sola dice "SQLAlchemy", evaluá interop.

---

## Subset compilable a binario (`fitz run` vs `fitz build`)

- ✅ `fitz run` cubre **todo** lo de este cap.
- ✅ `fitz build` con interop SQLAlchemy compila y produce binario.
  El binario requiere Python instalado en el destino + el venv con
  sqlalchemy y asyncpg. Para distribución sin Python instalado, ver
  M8 (próximo módulo) con `--bundle-python` + `--bundle-pip
  sqlalchemy asyncpg`.

- ⚠ Conocida la deuda residual de coerción Python list/dict →
  List/Map en codegen (mencionada en M7.C1) — para SQLAlchemy
  helpers que devuelven `list[dict]` simples funciona bit-a-bit; si
  llegás a un caso de helper que devuelve un `dict` deeply nested
  con varios niveles, podés terminar con un PyObject opaco en lugar
  de Map coercionado. Workaround: serializar a JSON string en el
  helper Python y `json.loads` en Fitz.

---

## Validación

Checklist completo:

```bash
# 1. Postgres corriendo.
$ docker ps | grep fitz-pg
fitz-pg ... 0.0.0.0:5432->5432/tcp

# 2. Schema inicializado.
(venv) $ python3 -c "import asyncio; from db_helpers import init_schema; print(asyncio.run(init_schema()))"
True

# 3. Fitz lee users (debería estar vacío al principio o tener Ada de Paso 4).
(venv) $ fitz-python run app.fitz &
$ curl -s localhost:3000/users
[]   # o [{"id":1,"name":"Ada",...}]

# 4. POST + GET round-trip.
$ curl -X POST localhost:3000/users -H 'Content-Type: application/json' \
       -d '{"name":"Curie","email":"marie@science.org"}'
{"id":2}
$ curl localhost:3000/users/2
{"id":2,"name":"Curie","email":"marie@science.org","created_at":"...","orders":[]}

# 5. Excepción Python → 500.
$ curl -i localhost:3000/users/9999 | head -1
HTTP/1.1 500 Internal Server Error

# 6. fitz py-types regenerá los types Fitz desde models.py.
(venv) $ fitz-python py-types models.py | head -10
type User {
    id: Int = 0,
    ...
```

Si los 6 puntos pasan, dominás SQLAlchemy interop + bridge async +
coerción tipada.

---

## Troubleshooting

**`ImportError: cannot import name 'asyncpg'`**: olvidaste
`pip install asyncpg`. SQLAlchemy 2.x con async **no usa psycopg2**.

**`sqlalchemy.exc.MissingGreenlet`**: estás usando SQLAlchemy async
sin el patrón `asyncpg`/`AsyncSession`. Verificá que el URL tenga
`postgresql+asyncpg://` (no `postgresql://` ni `postgresql+psycopg2://`).

**`<py_call>?.await` retorna `Future`/`PyObject` opaco en lugar de
desempaquetar**: probablemente olvidaste el `?` antes del `.await`.
El patrón canónico es `?.await` exactamente (`?` primero, después
`.await`). Sin el `?`, te queda el `Future<Result<T>>` envuelto.

**Performance lento con SQLAlchemy** (>50ms por simple SELECT):
SQLAlchemy 2.x async con asyncpg debería ser <10ms. Activá
`echo=True` en el engine para ver el SQL emitido — quizás está
disparando N+1 sin `selectinload` o `joinedload`.

**`fitz py-types` falla con `class without __table__`**: el modelo
SQLAlchemy debe estar registrado como subclase de `DeclarativeBase`
y declarar `__tablename__`. El introspector duck-typea sobre
`__table__.columns` (Fase 8.5 decisión).

**Conexión Postgres** falla intermitente: SQLAlchemy mantiene su
propio pool aparte del de Fitz (`db.connect` nativo). Si combinás
los dos en la misma app, asegurate de cerrar el SQLAlchemy engine
al exit o aceptá el costo de dos pools (~10 conns extra).

---

## Cerraste el módulo M7

Felicitaciones — terminaste el puente al ecosistema Python. Ahora
sabés:

- **Qué es la interop Python en Fitz** y cómo activarla (C1).
- **Cómo aprovechar el ecosistema científico** (numpy + pandas) sin
  microservicios aparte (C2).
- **Cuándo conviene SQLAlchemy vs el ORM nativo** y cómo combinar
  ambos con bridge async (C3).

**Lo que viene en M8**: producción real. Tu app de M6 o M7 ya
funciona en dev. Ahora vas a distribuirla — binarios standalone,
observability con OTel, secrets management, Docker autogenerado.
Y, específicamente para apps con interop Python (de este módulo),
M8.C5 cubre **deploy real**: `fitz build --bundle-python` para
empaquetar CPython adentro del binario y `--bundle-pip` para
empaquetar tus packages pip (pandas, numpy, sqlalchemy, etc.) —
deploy = un solo archivo, sin Python instalado en el destino.

→ [M8.C1 — Distribución avanzada: binarios standalone y cross-compile](../m8-produccion-deploy/c1-distribucion-binarios.md)
