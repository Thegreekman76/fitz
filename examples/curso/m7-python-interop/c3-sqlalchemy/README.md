# M7.C3 — SQLAlchemy interop + bridge async

Ejemplo runnable del cap [M7.C3 del curso](../../../../docs/curso/m7-python-interop/c3-sqlalchemy-async-vs-orm-nativo.md).

## Setup

```bash
# 1. Activar venv + instalar deps.
$ source venv/bin/activate
(venv) $ pip install "sqlalchemy[asyncio]>=2.0" asyncpg

# 2. Levantar Postgres (si no tenés uno corriendo).
$ docker run -d --name fitz-pg \
    -e POSTGRES_PASSWORD=secret \
    -p 5432:5432 \
    postgres:16

# 3. Generar models.fitz desde models.py.
(venv) $ fitz-python py-types models.py --out models.fitz
✓ types escritos a models.fitz
```

## Run

```bash
(venv) $ fitz-python run app.fitz
{"timestamp":"...","level":"INFO","msg":"server listo"}
```

## Probar

```bash
# Lista vacía al principio.
$ curl localhost:3000/users
[]

# Crear un user.
$ curl -X POST localhost:3000/users \
       -H 'Content-Type: application/json' \
       -d '{"name":"Ada","email":"ada@example.com"}'
{"id":1}

# Listar otra vez.
$ curl localhost:3000/users
[{"id":1,"name":"Ada","email":"ada@example.com","created_at":"..."}]

# Get user con orders nested.
$ curl localhost:3000/users/1
{"id":1,"name":"Ada","email":"ada@example.com","created_at":"...","orders":[]}

# Excepción → 500.
$ curl -i localhost:3000/users/9999 | head -1
HTTP/1.1 500 Internal Server Error
```

## Qué cubre

- SQLAlchemy 2.x async con `asyncpg`.
- `fitz py-types models.py` para auto-generar `type` Fitz desde
  modelos SQLAlchemy.
- Patrón canónico `<py_call>?.await` para corutinas Python (Fase 8.6
  bridge async).
- Coerción `List<Map>` → `List<User>` con anotación destino.
- Eager loading con `selectinload(User.orders)` desde Python.
- Excepciones Python → `Result::Err` → 500 con mensaje claro.

## Variables de entorno

- `DATABASE_URL` (default
  `postgresql+asyncpg://postgres:secret@localhost:5432/postgres`).

Para producción, override con tu connection string real
(idealmente desde un `secret()` Fitz nativo en M8 — este ejemplo
mantiene hardcoded para simplicidad).

## Archivos

- `app.fitz` — handlers HTTP Fitz.
- `models.py` — modelos SQLAlchemy.
- `models.fitz` — auto-generado con `fitz py-types models.py`.
- `db_helpers.py` — wrappers async (sessions + queries).
