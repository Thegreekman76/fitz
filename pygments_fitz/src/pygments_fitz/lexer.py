"""Fitz language lexer for Pygments.

Mantenimiento: cuando se agregue una keyword nueva al lenguaje
(parser/lexer en Rust), actualizar la tupla correspondiente abajo
(``KEYWORDS``, ``BUILTIN_TYPES``, etc.). Los decoradores ``@nombre``
se matchean con un regex genérico ``@\\w+``, así que no hace falta
listar cada decorator nuevo individualmente.

Convención de tokens:

* ``Keyword``           — fn, let, if, return, async, await, ...
* ``Keyword.Constant``  — null, true, false
* ``Keyword.Type``      — Int, Float, Str, Bool, List, Map, ...
* ``Name.Builtin``      — print, len, sleep, Ok, Err, jwt, hash, ...
* ``Name.Decorator``    — @get, @server, @authenticated, ...
* ``Name.Class``        — identifiers que arrancan con mayúscula
                          (tipo nominal típico: User, Order, ...)
* ``Name``              — identifiers en lowercase / snake_case
* ``Number.*``          — Int/Float/Hex/Bin/Oct (con separador ``_``)
* ``String``            — literales "..." y b"..."
* ``String.Interpol``   — delimitadores ``{`` y ``}`` adentro de un
                          string interpolado
* ``Comment.Single``    — ``// ...``
* ``Comment.Multiline`` — ``/* ... */`` (anidables)
* ``Operator``          — ``==``, ``!=``, ``=>``, ``->``, ``?``, ``..``, ...
* ``Punctuation``       — paréntesis, llaves, comas, dos puntos, etc.
"""

from pygments.lexer import RegexLexer, include, words
from pygments.token import (
    Comment,
    Keyword,
    Name,
    Number,
    Operator,
    Punctuation,
    String,
    Whitespace,
)


# Palabras reservadas del lenguaje. Si agregás una keyword al
# parser de Fitz, sumala acá.
KEYWORDS = (
    "fn", "let", "if", "else", "while", "for", "loop", "match",
    "return", "break", "continue", "async", "await", "type",
    "import", "from", "as", "in",
)

CONSTANTS = ("null", "true", "false")

# Variantes de Result — son constructores, no keywords del parser.
RESULT_VARIANTS = ("Ok", "Err")

# Tipos built-in del lenguaje (singletons reconocidos por el checker
# sin necesidad de import).
BUILTIN_TYPES = (
    "Int", "Float", "Str", "Bool", "Null", "List", "Map",
    "Result", "Future", "Range", "Bytes", "Any", "PyAny",
    "WsConn", "Request", "Response",
)

# Funciones built-in. La lista refleja lo que el checker pre-registra
# como nombres reservados.
BUILTIN_FUNCTIONS = (
    "print", "len", "sleep", "cors", "spawn",
    "assert", "assert_eq", "assert_ne", "assert_throws",
)

# Módulos built-in (no requieren import).
BUILTIN_MODULES = ("jwt", "hash")


class FitzLexer(RegexLexer):
    """Pygments lexer for the Fitz programming language.

    Aliases: ``fitz``. Filename pattern: ``*.fitz``.
    """

    name = "Fitz"
    aliases = ["fitz"]
    filenames = ["*.fitz"]
    mimetypes = ["text/x-fitz"]

    tokens = {
        "root": [
            include("whitespace"),
            include("comments"),
            include("decorators"),
            include("keywords"),
            include("types"),
            include("builtins"),
            include("numbers"),
            include("strings"),
            include("identifiers"),
            include("operators"),
            include("punctuation"),
        ],
        "whitespace": [
            (r"\s+", Whitespace),
        ],
        "comments": [
            (r"//.*?$", Comment.Single),
            (r"/\*", Comment.Multiline, "block_comment"),
        ],
        "block_comment": [
            (r"[^*/]+", Comment.Multiline),
            (r"/\*", Comment.Multiline, "#push"),
            (r"\*/", Comment.Multiline, "#pop"),
            (r"[*/]", Comment.Multiline),
        ],
        "decorators": [
            # `@nombre` — el `@` y el ident van juntos como un solo
            # Name.Decorator. Match genérico, sin lista hardcodeada,
            # para no quedarnos atrás cuando se agreguen decoradores
            # nuevos al lenguaje.
            (r"@\w+", Name.Decorator),
        ],
        "keywords": [
            (words(KEYWORDS, suffix=r"\b"), Keyword),
            (words(CONSTANTS, suffix=r"\b"), Keyword.Constant),
        ],
        "types": [
            (words(BUILTIN_TYPES, suffix=r"\b"), Keyword.Type),
        ],
        "builtins": [
            (words(RESULT_VARIANTS, suffix=r"\b"), Name.Builtin),
            (words(BUILTIN_FUNCTIONS, suffix=r"\b"), Name.Builtin),
            (words(BUILTIN_MODULES, suffix=r"\b"), Name.Builtin.Pseudo),
        ],
        "numbers": [
            # Orden importa: hex/bin/oct antes que int decimal para
            # que `0x` no se coma como `0` + ident `x`.
            (r"0x[0-9a-fA-F_]+", Number.Hex),
            (r"0b[01_]+", Number.Bin),
            (r"0o[0-7_]+", Number.Oct),
            # Float con punto obligatorio o exponente.
            (r"\d[\d_]*\.\d[\d_]*([eE][+-]?\d+)?", Number.Float),
            (r"\d[\d_]*[eE][+-]?\d+", Number.Float),
            # Integer.
            (r"\d[\d_]*", Number.Integer),
        ],
        "strings": [
            # Bytes literal: `b"..."`. NO tiene interpolación.
            (r'b"', String.Affix, "bytes_string"),
            # String regular: `"..."` con interpolación `{expr}`.
            (r'"', String, "string"),
        ],
        "string": [
            (r'"', String, "#pop"),
            # Escapes — el lexer del lenguaje acepta los típicos +
            # `\xHH` byte + `\u{HHHH}` codepoint unicode.
            (r"\\u\{[0-9a-fA-F]+\}", String.Escape),
            (r"\\x[0-9a-fA-F]{2}", String.Escape),
            (r"\\.", String.Escape),
            # `{` abre interpolación. `{{` no existe en Fitz (no hay
            # escape de llave por duplicación); por eso un `{` solo
            # siempre arranca interpolación.
            (r"\{", String.Interpol, "interpolation"),
            (r'[^"\\{]+', String),
        ],
        "bytes_string": [
            (r'"', String.Affix, "#pop"),
            (r"\\x[0-9a-fA-F]{2}", String.Escape),
            (r"\\.", String.Escape),
            (r'[^"\\]+', String),
        ],
        "interpolation": [
            # `}` cierra y vuelve al string.
            (r"\}", String.Interpol, "#pop"),
            # Adentro de la interpolación, casi cualquier expresión
            # Fitz es válida. Reusamos las mismas reglas del root
            # excepto strings (para no entrar a una madeja de
            # estados anidados; las interpolaciones con strings
            # dentro son raras y se ven OK como Name fallback).
            include("whitespace"),
            include("comments"),
            include("keywords"),
            include("types"),
            include("builtins"),
            include("numbers"),
            include("identifiers"),
            include("operators"),
            include("punctuation"),
            # Cualquier otra cosa adentro de `{...}` cae acá.
            (r"[^}]+", Name),
        ],
        "identifiers": [
            # Convención del lenguaje: identifiers que arrancan con
            # mayúscula son nombres de tipo nominal (User, Order, ...).
            # Pygments lo pinta distinto que un binding local.
            (r"[A-Z][a-zA-Z0-9_]*", Name.Class),
            (r"[a-z_][a-zA-Z0-9_]*", Name),
        ],
        "operators": [
            # Multi-char primero para que `==` no se parsea como `=` `=`.
            (r"==|!=|<=|>=|&&|\|\||=>|->|\.\.|\?|<<|>>", Operator),
            # Asignación compuesta y operadores binarios simples.
            (r"[+\-*/%<>!=&|^~]=?", Operator),
        ],
        "punctuation": [
            (r"[(){}\[\],;:.@]", Punctuation),
        ],
    }
