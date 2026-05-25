# Stub Python (PEP 484/561). El loader del checker Fitz lo detecta
# automáticamente cuando ve `from python import users` adyacente.
#
# Convención: el archivo se llama <name>.pyi donde <name> coincide
# con el módulo Python que el código real provee (e.g. `users.py`
# en el venv o en site-packages, o tu propia lib Python). En este
# ejemplo solo nos interesa la parte de typing — no ejecutamos
# código Python real (no hay feature `python` ni `users.py` real).

class User:
    id: int
    name: str
    email: str | None

class Order:
    user_id: int
    items: list[str]
    total: float

# Variables top-level (8-pyi.C): tipo directo, sin wrap.
DEFAULT_USER_NAME: str
MAX_ORDER_ITEMS: int

# Funciones top-level (8-pyi.C): el call site tipa
# automáticamente como `Result<ret>` reflejando el wrap runtime.
def make_user(id: int, name: str) -> User: ...
def make_order(uid: int, items: list[str]) -> Order: ...
def user_summary(u: User) -> str: ...
