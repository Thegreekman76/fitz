"""Helpers CRUD del boilerplate api-postgres-python.

Adyacente a `models.py`. Fitz invoca estas fns via
`from python import db` + `db.add_user(...)` / `db.get_user(...)`
/ `db.list_users(...)` / `db.init_db()`.

Connection string sale de env vars (DATABASE_URL):
  postgresql+psycopg2://<user>:<pass>@<host>/<dbname>

El docker-compose setea DATABASE_URL al hostname `db` (service
name de Postgres) y los credenciales del .env.
"""

import os
from typing import List

from sqlalchemy import create_engine
from sqlalchemy.exc import NoResultFound
from sqlalchemy.orm import Session, sessionmaker

from models import Base, User

# --- Engine + session factory ----------------------------------------

_DATABASE_URL = os.environ.get(
    "DATABASE_URL",
    "postgresql+psycopg2://fitz:fitz@db:5432/fitz",
)

# `pool_pre_ping=True`: chequea conexiones del pool antes de cada
# checkout — robusto contra Postgres restarts (común en dev local).
_engine = create_engine(_DATABASE_URL, pool_pre_ping=True, future=True)
_SessionLocal = sessionmaker(bind=_engine, expire_on_commit=False, future=True)


# --- Public API consumida por Fitz -----------------------------------

def init_db() -> str:
    """Crea las tablas del modelo (`users`) si no existen. Idempotente.
    Llamado al boot por main.fitz."""
    Base.metadata.create_all(_engine)
    return "ok"


def add_user(name: str, email: str) -> dict:
    """Inserta un user nuevo. Postgres asigna `id` y `created_at`."""
    with _SessionLocal() as session:
        user = User(name=name, email=email)
        session.add(user)
        session.commit()
        session.refresh(user)
        return user.to_dict()


def get_user(user_id: int) -> dict:
    """Lookup por id. Levanta NoResultFound si no existe — el wrap
    de Fitz convierte la excepción a `Err("<class>: <message>")`."""
    with _SessionLocal() as session:
        user = session.query(User).filter(User.id == user_id).one()  # NoResultFound si no hay match
        return user.to_dict()


def list_users() -> List[dict]:
    """Listar todos. Orden por id ASC para output predecible."""
    with _SessionLocal() as session:
        users = session.query(User).order_by(User.id.asc()).all()
        return [u.to_dict() for u in users]
