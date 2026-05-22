"""SQLAlchemy models del boilerplate api-postgres-python.

Adyacente a `db.py` (helpers CRUD). Ambos archivos se copian al
container Docker y `PYTHONPATH=/app/python` los expone a Fitz
via `from python import models` / `from python import db`.

El `type User` Fitz en `src/types/user.fitz` refleja este modelo.
En proyectos reales, `fitz py-types python/models.py --out
src/types/user.fitz` automatiza la sincronización.
"""

from datetime import datetime
from sqlalchemy import Column, DateTime, Integer, String
from sqlalchemy.orm import declarative_base

Base = declarative_base()


class User(Base):
    """User table — id auto-asignado, email único, timestamp del insert."""

    __tablename__ = "users"

    id = Column(Integer, primary_key=True, autoincrement=True)
    name = Column(String(120), nullable=False)
    email = Column(String(180), nullable=False, unique=True)
    created_at = Column(DateTime, nullable=False, default=datetime.utcnow)

    def to_dict(self) -> dict:
        """Serialización a dict — el wrapper Fitz lo round-trip-ea
        por JSON al `type User` via `let u: User = json.loads(s)?`."""
        return {
            "id": self.id,
            "name": self.name,
            "email": self.email,
            "created_at": self.created_at.isoformat() if self.created_at else "",
        }
