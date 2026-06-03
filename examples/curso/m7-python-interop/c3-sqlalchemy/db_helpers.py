"""
db_helpers.py — wrappers async sobre SQLAlchemy 2.x para que Fitz
consuma con el patrón <py_async_fn>?.await.

Devuelven dict / list[dict] explícito porque SQLAlchemy entities
(User/Order) son objetos opacos para Fitz.
"""
import os
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession
from sqlalchemy.orm import sessionmaker, selectinload
from sqlalchemy import select
from models import Base, User, Order

DATABASE_URL = os.environ.get(
    "DATABASE_URL",
    "postgresql+asyncpg://postgres:secret@localhost:5432/postgres",
)

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
    """SELECT * FROM users. Devuelve list[dict]."""
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
    """SELECT + eager load. Devuelve dict con orders nested."""
    async with _SessionFactory() as session:
        result = await session.execute(
            select(User)
            .where(User.id == user_id)
            .options(selectinload(User.orders))
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
                {
                    "id": o.id,
                    "total_cents": o.total_cents,
                    "created_at": o.created_at.isoformat(),
                }
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
