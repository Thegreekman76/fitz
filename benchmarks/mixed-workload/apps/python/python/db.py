"""Helpers CRUD del lado Python para apps/python/ del bench mixed-workload.

Fitz invoca estas fns via `from python import db` desde
`src/data/users.fitz` y `src/data/posts.fitz`.

Connection string desde `DATABASE_URL` (lo setea docker-compose).
Formato SQLAlchemy: `postgresql+psycopg2://<user>:<pass>@<host>/<db>`.
"""

import os
from typing import List

from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker

from models import Base, Post, User

# --- Engine + session factory ----------------------------------------

_DATABASE_URL = os.environ.get(
    "DATABASE_URL",
    "postgresql+psycopg2://fitz:fitz@db:5432/fitz",
)

# `pool_pre_ping=True`: chequea conexiones del pool antes de cada
# checkout — robusto contra Postgres restarts (común en bench
# multi-run que hace `down -v` + `up -d` entre stacks).
_engine = create_engine(
    _DATABASE_URL,
    pool_pre_ping=True,
    future=True,
    # Bench-friendly defaults: pool grande para sostener 100 VUs
    # del scenario peak sin saturarse al primer ramp-up.
    pool_size=20,
    max_overflow=10,
)
_SessionLocal = sessionmaker(bind=_engine, expire_on_commit=False, future=True)


# --- Public API consumida por Fitz -----------------------------------

def init_db() -> str:
    """Crea tablas si no existen. Idempotente. Llamado al boot."""
    Base.metadata.create_all(_engine)
    return "ok"


def add_user(name: str, email: str) -> dict:
    """Inserta un user nuevo. Postgres asigna id + created_at."""
    with _SessionLocal() as session:
        user = User(name=name, email=email)
        session.add(user)
        session.commit()
        session.refresh(user)
        return user.to_dict()


def get_user(user_id: int) -> dict:
    """Lookup por id. NoResultFound si no existe → propagado como Err."""
    with _SessionLocal() as session:
        user = session.query(User).filter(User.id == user_id).one()
        return user.to_dict()


def list_users(limit: int = 20) -> List[dict]:
    """Lista paginada de users, ordenada por id ASC."""
    with _SessionLocal() as session:
        users = (
            session.query(User)
            .order_by(User.id.asc())
            .limit(limit)
            .all()
        )
        return [u.to_dict() for u in users]


def update_user(user_id: int, name: str, email: str) -> dict:
    """Update name + email, devuelve el row actualizado."""
    with _SessionLocal() as session:
        user = session.query(User).filter(User.id == user_id).one()
        user.name = name
        user.email = email
        session.commit()
        session.refresh(user)
        return user.to_dict()


def add_post(user_id: int, title: str, body: str) -> dict:
    """Inserta un post asociado a un user. Postgres asigna id + created_at."""
    with _SessionLocal() as session:
        post = Post(user_id=user_id, title=title, body=body)
        session.add(post)
        session.commit()
        session.refresh(post)
        return post.to_dict()


def list_user_posts(user_id: int) -> List[dict]:
    """Lista posts de un user, ordenada por id ASC."""
    with _SessionLocal() as session:
        posts = (
            session.query(Post)
            .filter(Post.user_id == user_id)
            .order_by(Post.id.asc())
            .all()
        )
        return [p.to_dict() for p in posts]
