"""SQLAlchemy model del boilerplate api-fullstack-postgres."""

from datetime import datetime
from sqlalchemy import Boolean, Column, DateTime, Integer, String
from sqlalchemy.orm import declarative_base

Base = declarative_base()


class Task(Base):
    """Task — 5 fields que matchean el `type Task` del lado Fitz
    (`src/types/task.fitz`)."""

    __tablename__ = "tasks"

    id = Column(Integer, primary_key=True, autoincrement=True)
    title = Column(String(200), nullable=False)
    description = Column(String(1000), nullable=False, default="")
    priority = Column(String(10), nullable=False, default="med")  # low / med / high
    done = Column(Boolean, nullable=False, default=False)
    created_at = Column(DateTime, nullable=False, default=datetime.utcnow)

    def to_dict(self) -> dict:
        """Serialización a dict — el wrapper Fitz lo round-trip-ea
        por JSON al `type Task` via `let t: Task = json.loads(s)?`."""
        return {
            "id": self.id,
            "title": self.title,
            "description": self.description or "",
            "priority": self.priority,
            "done": self.done,
            "created_at": self.created_at.isoformat() if self.created_at else "",
        }
