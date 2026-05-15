"""Modelos SQLAlchemy del ejemplo CRUD del cap 21.

Definidos sobre SQLite (in-process, sin servidor externo). La idea es
mostrar el mismo patrón conceptual que con Postgres real — las
queries y los models son idénticos; solo cambia la URL de conexión.

Para correr el ejemplo de la guía:

    pip install sqlalchemy
    cargo run --features python -- run examples/guide/21-python-crud.fitz

`fitz py-types` puede generar los `type` Fitz correspondientes a
estos modelos:

    fitz py-types examples/guide/21-python-crud/models.py \\
        --out examples/guide/21-python-crud/models.fitz
"""
from sqlalchemy import Column, Integer, String
from sqlalchemy.orm import declarative_base

Base = declarative_base()


class User(Base):
    __tablename__ = "users"
    id = Column(Integer, primary_key=True)
    name = Column(String, nullable=False)
    email = Column(String, nullable=False)
