"""Tests del FitzLexer.

Usa ``unittest`` del stdlib para que el workflow `docs.yml` los
corra sin necesidad de pip-instalar pytest.

Corrida local:

    python -m unittest discover pygments_fitz/tests
"""

import unittest

from pygments.token import (
    Comment,
    Keyword,
    Name,
    Number,
    Operator,
    Punctuation,
    String,
)

from pygments_fitz import FitzLexer


class FitzLexerTests(unittest.TestCase):
    def setUp(self):
        self.lexer = FitzLexer()

    # ------------------------------------------------------------ #
    # helpers                                                      #
    # ------------------------------------------------------------ #

    def tokens(self, code):
        """Tokens (tipo, valor) sin whitespace puro."""
        return [
            (tok, val)
            for tok, val in self.lexer.get_tokens(code)
            if val.strip() != ""
        ]

    def types(self, code):
        return [tok for tok, _ in self.tokens(code)]

    def assert_has(self, code, expected):
        """Aserta que ``(token, value)`` aparece en el output."""
        self.assertIn(expected, self.tokens(code),
                      msg=f"{expected!r} no apareció en tokens de {code!r}")

    # ------------------------------------------------------------ #
    # keywords                                                     #
    # ------------------------------------------------------------ #

    def test_fn_keyword(self):
        self.assert_has("fn add(x: Int) -> Int", (Keyword, "fn"))

    def test_let_keyword(self):
        self.assert_has("let x = 1", (Keyword, "let"))

    def test_if_else(self):
        toks = self.tokens("if x { 1 } else { 2 }")
        self.assertIn((Keyword, "if"), toks)
        self.assertIn((Keyword, "else"), toks)

    def test_async_await(self):
        toks = self.tokens("async fn foo() => bar().await")
        self.assertIn((Keyword, "async"), toks)
        self.assertIn((Keyword, "await"), toks)

    def test_match_return(self):
        toks = self.tokens("match x { Ok(v) => return v, Err(_) => return 0 }")
        self.assertIn((Keyword, "match"), toks)
        self.assertIn((Keyword, "return"), toks)

    def test_import_from_as(self):
        toks = self.tokens("from python import math as m")
        self.assertIn((Keyword, "from"), toks)
        self.assertIn((Keyword, "import"), toks)
        self.assertIn((Keyword, "as"), toks)

    def test_constants(self):
        self.assert_has("let x = null", (Keyword.Constant, "null"))
        self.assert_has("let x = true", (Keyword.Constant, "true"))
        self.assert_has("let x = false", (Keyword.Constant, "false"))

    # ------------------------------------------------------------ #
    # decorators                                                   #
    # ------------------------------------------------------------ #

    def test_decorator_get(self):
        self.assert_has('@get("/users")', (Name.Decorator, "@get"))

    def test_decorator_server(self):
        self.assert_has("@server(3000)", (Name.Decorator, "@server"))

    def test_decorator_unknown_still_highlighted(self):
        """Decoradores nuevos del lenguaje deben highlightearse aún
        sin estar listados explícitamente."""
        self.assert_has("@new_decorator", (Name.Decorator, "@new_decorator"))

    def test_decorator_authenticated(self):
        self.assert_has("@authenticated", (Name.Decorator, "@authenticated"))

    def test_decorator_ws(self):
        self.assert_has('@ws("/chat")', (Name.Decorator, "@ws"))

    def test_decorator_cron(self):
        self.assert_has('@cron("0 0 * * *")', (Name.Decorator, "@cron"))

    # ------------------------------------------------------------ #
    # builtin types                                                #
    # ------------------------------------------------------------ #

    def test_builtin_types(self):
        for type_name in ("Int", "Float", "Str", "Bool", "List", "Map",
                          "Result", "Future", "Range", "Bytes",
                          "WsConn", "PyAny"):
            self.assert_has(f"let x: {type_name}", (Keyword.Type, type_name))

    # ------------------------------------------------------------ #
    # builtins                                                     #
    # ------------------------------------------------------------ #

    def test_result_variants(self):
        self.assert_has("Ok(42)", (Name.Builtin, "Ok"))
        self.assert_has('Err("nope")', (Name.Builtin, "Err"))

    def test_print_builtin(self):
        self.assert_has('print("hola")', (Name.Builtin, "print"))

    def test_len_builtin(self):
        self.assert_has("len(xs)", (Name.Builtin, "len"))

    def test_jwt_module(self):
        self.assert_has("jwt.encode(claims)", (Name.Builtin.Pseudo, "jwt"))

    def test_hash_module(self):
        self.assert_has("hash.password(pw)", (Name.Builtin.Pseudo, "hash"))

    def test_assert_builtins(self):
        self.assert_has("assert_eq(x, 1)", (Name.Builtin, "assert_eq"))

    # ------------------------------------------------------------ #
    # strings                                                      #
    # ------------------------------------------------------------ #

    def test_simple_string(self):
        toks = self.tokens('"hola"')
        # Hay String para las comillas + contenido
        self.assertTrue(any(tok in (String, String.Affix) for tok, _ in toks))

    def test_string_with_interpolation(self):
        toks = self.tokens('"hola, {name}!"')
        # Tiene que aparecer la marca de interpolación
        self.assertIn(String.Interpol, [t for t, _ in toks],
                      msg=f"interpolation marker no apareció: {toks!r}")

    def test_bytes_literal(self):
        toks = self.tokens('b"abc"')
        # El prefijo bytes debe estar marcado
        self.assertTrue(any(tok == String.Affix for tok, _ in toks),
                        msg=f"bytes prefix no apareció: {toks!r}")

    def test_string_escape(self):
        toks = self.tokens(r'"hola\n"')
        self.assertIn(String.Escape, [t for t, _ in toks])

    # ------------------------------------------------------------ #
    # comments                                                     #
    # ------------------------------------------------------------ #

    def test_single_line_comment(self):
        toks = self.tokens("// hola\nlet x = 1")
        self.assertTrue(any(tok == Comment.Single for tok, _ in toks))

    def test_multi_line_comment(self):
        toks = self.tokens("/* hola\nmundo */ let x = 1")
        self.assertTrue(any(tok == Comment.Multiline for tok, _ in toks))

    def test_nested_block_comment(self):
        toks = self.tokens("/* outer /* inner */ outer */")
        # No tiene que romper. Todo el bloque queda como Comment.Multiline.
        types = [tok for tok, _ in toks]
        # No tiene que haber un Name/Keyword "outer" después del comment
        # (i.e. el comment consumió todo y el cierre interno no rompió).
        self.assertNotIn(Name, types)
        self.assertNotIn(Keyword, types)

    # ------------------------------------------------------------ #
    # numbers                                                      #
    # ------------------------------------------------------------ #

    def test_integer(self):
        self.assert_has("42", (Number.Integer, "42"))

    def test_integer_with_separator(self):
        self.assert_has("1_000_000", (Number.Integer, "1_000_000"))

    def test_float(self):
        self.assert_has("3.14", (Number.Float, "3.14"))

    def test_float_with_exponent(self):
        self.assert_has("1.5e10", (Number.Float, "1.5e10"))

    def test_hex(self):
        self.assert_has("0xFF", (Number.Hex, "0xFF"))

    def test_bin(self):
        self.assert_has("0b1010", (Number.Bin, "0b1010"))

    def test_oct(self):
        self.assert_has("0o777", (Number.Oct, "0o777"))

    # ------------------------------------------------------------ #
    # identifiers                                                  #
    # ------------------------------------------------------------ #

    def test_pascal_case_is_class(self):
        # `User` arranca con mayúscula → Name.Class (tipo nominal)
        self.assert_has("User { id: 1 }", (Name.Class, "User"))

    def test_snake_case_is_name(self):
        self.assert_has("let foo_bar = 1", (Name, "foo_bar"))

    def test_builtin_types_not_class(self):
        # `Int` está en BUILTIN_TYPES → Keyword.Type, NO Name.Class
        toks = self.tokens("let x: Int = 1")
        self.assertIn((Keyword.Type, "Int"), toks)
        self.assertNotIn((Name.Class, "Int"), toks)

    # ------------------------------------------------------------ #
    # operators y punctuation                                      #
    # ------------------------------------------------------------ #

    def test_arrow(self):
        self.assert_has("fn f() -> Int", (Operator, "->"))

    def test_fat_arrow(self):
        self.assert_has("fn f() => 1", (Operator, "=>"))

    def test_try_operator(self):
        self.assert_has("foo()?", (Operator, "?"))

    def test_range_operator(self):
        self.assert_has("for i in 0..10", (Operator, ".."))

    def test_eq_eq(self):
        self.assert_has("x == y", (Operator, "=="))

    def test_punctuation(self):
        toks = self.tokens("foo(x, y)")
        types = [tok for tok, _ in toks]
        self.assertIn(Punctuation, types)

    # ------------------------------------------------------------ #
    # smoke — programa completo                                    #
    # ------------------------------------------------------------ #

    def test_hello_world(self):
        code = '''// hola.fitz
print("Hola desde Fitz 🏔️")

name = "Patagonia"
print("Hola, {name}!")
'''
        toks = self.tokens(code)
        types = [tok for tok, _ in toks]
        # Esperamos al menos: comment + print builtin + interpolation
        self.assertIn(Comment.Single, types)
        self.assertIn(Name.Builtin, [t for t, v in toks if v == "print"])
        self.assertIn(String.Interpol, types)

    def test_http_handler_with_auth(self):
        code = '''type User { id: Int, role: Str }

@auth_provider
fn check_token(headers: Map<Str, Str>) -> Result<User> {
    return Ok(User { id: 1, role: "admin" })
}

@authenticated
@get("/me")
async fn me(user: User) -> User => user

@server(3000)
fn main() => 0
'''
        toks = self.tokens(code)
        types = [tok for tok, _ in toks]
        # Sanity checks de un programa real con casi todo
        self.assertIn(Keyword, types)
        self.assertIn(Keyword.Type, types)
        self.assertIn(Name.Decorator, types)
        self.assertIn(Name.Builtin, [t for t, v in toks if v == "Ok"])
        self.assertIn(Name.Class, [t for t, v in toks if v == "User"])
        self.assertIn(Number.Integer, types)
        self.assertIn(String, types)

    def test_lexer_does_not_raise_on_arbitrary_input(self):
        """Smoke: cualquier input arbitrario no debe romper el lexer."""
        weird = "@@@@ !! 999.0.0 //// /* /* */ \"hola\\"
        # Solo debe terminar sin excepción.
        list(self.lexer.get_tokens(weird))


if __name__ == "__main__":
    unittest.main()
