"""Pygments lexer for the Fitz programming language.

Registered via entry point ``pygments.lexers`` in ``pyproject.toml``,
so any Pygments consumer (MkDocs Material, Sphinx, Jupyter, etc.) that
calls ``pygments.lexers.get_lexer_by_name("fitz")`` will receive the
:class:`FitzLexer` after ``pip install pygments-fitz``.
"""

from .lexer import FitzLexer

__all__ = ["FitzLexer"]
__version__ = "0.1.0"
