"""Helpers de DB para el ejemplo CRUD del cap 21.

Encapsulan el setup de SQLAlchemy (engine, sessionmaker, create_all)
para que el código Fitz solo invoque fns con shape simple:

    init_db()                       -> None
    add_user(name, email)           -> dict (la fila insertada con id real)
    list_users()                    -> list[dict] (todas las filas)
    get_user(uid)                   -> dict | None

Cada fn devuelve dicts/lists nativos Python (no instancias del modelo
SQLAlchemy) para que el marshaling a Fitz sea directo: List<dict> y
dict ↔ Map. El cap 21.7 muestra cómo subir esos dicts a `Instance`
del lado Fitz con anotaciones.

Si SQLAlchemy no está instalado, init_db() lanza ImportError que
llega al Fitz como `Err(Str("ImportError: ..."))`.
"""
import os

from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker

from models import Base, User

# El DB file vive adyacente a este módulo. `os.path.dirname(__file__)`
# resuelve a `examples/guide/21-python-crud/` independientemente del
# cwd desde donde se ejecute Fitz.
_DB_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "crud.db")
_engine = create_engine(f"sqlite:///{_DB_PATH}", echo=False, future=True)
_SessionLocal = sessionmaker(bind=_engine, autoflush=False, autocommit=False)


def init_db():
    """Crea las tablas si no existen. Idempotente."""
    Base.metadata.create_all(_engine)


def _to_dict(user: User) -> dict:
    """Serializa una fila User a dict con orden de declaración. Es
    lo que Fitz va a recibir como `Map<Str, Any>` (coercible a
    `Instance User` con anotación del lado Fitz, cap 21.7)."""
    return {"id": user.id, "name": user.name, "email": user.email}


def add_user(name: str, email: str) -> dict:
    """Inserta una fila User y devuelve el dict con el id real
    asignado por SQLite. Si name/email vienen vacíos, SQLAlchemy
    no falla — la validación de input vive del lado Fitz."""
    with _SessionLocal() as session:
        user = User(name=name, email=email)
        session.add(user)
        session.commit()
        session.refresh(user)
        return _to_dict(user)


def list_users() -> list:
    """Devuelve todas las filas como lista de dicts en orden de id."""
    with _SessionLocal() as session:
        rows = session.query(User).order_by(User.id).all()
        return [_to_dict(u) for u in rows]


def get_user(uid: int):
    """Busca por id. Devuelve dict o None."""
    with _SessionLocal() as session:
        user = session.query(User).filter_by(id=uid).first()
        return _to_dict(user) if user is not None else None


def reset():
    """Borra todas las filas (útil para tests). No se invoca desde
    el ejemplo Fitz; queda como utilidad si querés correrlo varias
    veces sin dejar data acumulada."""
    with _SessionLocal() as session:
        session.query(User).delete()
        session.commit()
