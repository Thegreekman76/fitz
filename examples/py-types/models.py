# examples/py-types/models.py — fuente para `fitz py-types`.
#
# Demuestra el patrón canónico de Fase 8.5: clases con `__table__.columns`
# se traducen automáticamente a `type` Fitz.
#
# Este archivo es **autosuficiente** — define un mock mínimo del shape
# de SQLAlchemy para que el ejemplo corra sin requerir
# `pip install sqlalchemy`. En un proyecto real, reemplazás el bloque
# del mock con:
#
#   from sqlalchemy.orm import DeclarativeBase
#   from sqlalchemy import Column, Integer, String, Float, Boolean, DateTime
#
# y todo lo demás funciona igual — la introspección usa duck typing
# sobre `__table__.columns`, así que solo importa el shape (que las
# Column tengan `name`, `type`, `nullable`, `default`).


# -----------------------------------------------------------------------
# Mock pedagógico de SQLAlchemy (~25 LoC; en el caso real, esto se
# reemplaza con `from sqlalchemy import ...`).
# -----------------------------------------------------------------------

class Column:
    def __init__(self, type_, nullable=False, default=None):
        self.name = None  # SQLAlchemy lo setea via descriptor; el helper
                          # `_named` lo hace explícito acá.
        self.type = type_
        self.nullable = nullable
        self.default = default

class _Columns:
    def __init__(self, items):
        self._items = items
    def __iter__(self):
        return iter(self._items)

class _Table:
    def __init__(self, columns):
        self.columns = _Columns(columns)

def _named(name, col):
    col.name = name
    return col

class Integer: pass
class BigInteger: pass
class Float: pass
class String: pass
class Boolean: pass
class DateTime: pass


# -----------------------------------------------------------------------
# Modelos — esto SÍ es lo que escribirías en un proyecto real
# (excepto que `Column`, `Integer`, etc. vendrían de SQLAlchemy).
# -----------------------------------------------------------------------

class User:
    """Usuario registrado en el sistema."""
    __table__ = _Table([
        _named("id", Column(Integer())),
        _named("email", Column(String())),
        _named("name", Column(String())),
        _named("age", Column(Integer(), nullable=True)),
        _named("is_admin", Column(Boolean(), default=False)),
        _named("created_at", Column(DateTime())),
    ])


class Order:
    """Pedido asociado a un User."""
    __table__ = _Table([
        _named("id", Column(BigInteger())),
        _named("user_id", Column(Integer())),
        _named("total", Column(Float())),
        _named("currency", Column(String(), default="USD")),
        _named("notes", Column(String(), nullable=True)),
    ])
