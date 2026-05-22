"""CRUD helpers del boilerplate api-fullstack-postgres.

Invocados por Fitz vía `from python import db` desde
`src/data/tasks.fitz`. Cada helper retorna un `dict` (o `list[dict]`)
que el wrapper Fitz round-trip-ea por JSON al `type Task`.

Connection string desde env var DATABASE_URL (seteado por
docker-compose a partir del .env)."""

import os
from typing import List

from sqlalchemy import create_engine
from sqlalchemy.exc import NoResultFound
from sqlalchemy.orm import sessionmaker

from models import Base, Task

_DATABASE_URL = os.environ.get(
    "DATABASE_URL",
    "postgresql+psycopg2://fitz:fitz@db:5432/fitz",
)

_engine = create_engine(_DATABASE_URL, pool_pre_ping=True, future=True)
_SessionLocal = sessionmaker(bind=_engine, expire_on_commit=False, future=True)


def init_db() -> str:
    """Crear tablas si no existen. Idempotente."""
    Base.metadata.create_all(_engine)
    return "ok"


def add_task(title: str, description: str, priority: str) -> dict:
    """Crear task nuevo. id y created_at los asigna Postgres."""
    with _SessionLocal() as session:
        task = Task(
            title=title,
            description=description or "",
            priority=priority or "med",
        )
        session.add(task)
        session.commit()
        session.refresh(task)
        return task.to_dict()


def get_task(task_id: int) -> dict:
    """Lookup por id. NoResultFound si no existe → Fitz lo wrappea en Err."""
    with _SessionLocal() as session:
        task = session.query(Task).filter(Task.id == task_id).one()
        return task.to_dict()


def list_tasks(filter_kind: str = "all") -> List[dict]:
    """Lista filtrada. `filter_kind` ∈ {"all", "pending", "done"}.
    Orden: pending primero (DONE=false), después por id ASC."""
    with _SessionLocal() as session:
        query = session.query(Task)
        if filter_kind == "pending":
            query = query.filter(Task.done.is_(False))
        elif filter_kind == "done":
            query = query.filter(Task.done.is_(True))
        # "all" o cualquier otro → sin filtro.
        tasks = query.order_by(Task.done.asc(), Task.id.asc()).all()
        return [t.to_dict() for t in tasks]


def update_task(task_id: int, title: str, description: str, priority: str, done: bool) -> dict:
    """Update completo. NoResultFound si no existe."""
    with _SessionLocal() as session:
        task = session.query(Task).filter(Task.id == task_id).one()
        task.title = title
        task.description = description or ""
        task.priority = priority or "med"
        task.done = bool(done)
        session.commit()
        session.refresh(task)
        return task.to_dict()


def delete_task(task_id: int) -> dict:
    """Delete por id. NoResultFound si no existe.
    Devuelve `{"deleted": true, "id": <id>}` para confirmar al frontend."""
    with _SessionLocal() as session:
        task = session.query(Task).filter(Task.id == task_id).one()
        session.delete(task)
        session.commit()
        return {"deleted": True, "id": task_id}
