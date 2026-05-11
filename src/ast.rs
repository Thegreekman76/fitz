// ast.rs — Fase 2.2
//
// Define las estructuras de datos que representan un programa en memoria.
// El parser construye este árbol a partir de los tokens; el evaluador lo
// recorre para ejecutar.
//
// Convenciones:
//  - `Expr` produce un valor (tiene tipo).
//  - `Stmt` produce un efecto (no necesariamente tiene valor).
//  - Las recursiones se hacen con `Box<Expr>` porque Rust necesita tamaño
//    conocido en compile-time para los enums.

/// Una expresión: produce un valor.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    // ---------- literales ----------
    Int(i64),
    Float(f64),
    /// String literal sin interpolación. Ej: `"Hola"`.
    Str(String),
    /// String con interpolación. Ej: `"Hola, {name}!"` → partes literales y
    /// expresiones intercaladas. El parser elige `Str` vs `StrInterp` según
    /// el contenido. La razón de tener ambas en vez de solo `StrInterp` es
    /// claridad y una mínima optimización en el evaluador.
    StrInterp(Vec<StrPart>),
    Bool(bool),
    Null,

    /// Referencia a un identificador (variable, parámetro, función, etc.).
    Ident(String),

    /// Operación binaria: `left <op> right`.
    BinOp {
        op: BinOpKind,
        left: Box<Expr>,
        right: Box<Expr>,
    },

    /// Operación unaria prefijo: `<op> operand`. Por ahora solo
    /// negación numérica (`-x`). Cuando el lexer emita `!` como
    /// operador lógico, sumaremos `UnaryOpKind::Not`.
    UnaryOp {
        op: UnaryOpKind,
        operand: Box<Expr>,
    },

    /// Llamada a función: `name(arg1, arg2, ...)`.
    /// Por ahora solo soporta llamadas con nombre simple (no expresiones que
    /// resulten en función). Cuando agreguemos closures como valores de
    /// primera clase esto cambia a `callee: Box<Expr>`.
    Call {
        name: String,
        args: Vec<Expr>,
    },

    /// Acceso a campo: `objeto.campo`.
    Field {
        object: Box<Expr>,
        field: String,
    },

    /// Indexing postfix: `objeto[indice]`. Aplica a listas (`xs[0]`),
    /// mapas (`m["clave"]`), y strings (lectura por índice si lo
    /// agregamos). El receptor y el índice son expresiones cualquiera.
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },

    /// Lista literal: `[1, 2, 3]`, `[]`, `[x, y + 1]`. Anidable.
    List(Vec<Expr>),

    /// Mapa literal: `{"k": v, "otra": 42}`, `{}`. Preserva orden de
    /// inserción (por eso `Vec<(Expr, Expr)>` y no `HashMap`). Las
    /// claves son expresiones (típicamente strings, pero permitido
    /// cualquier expresión por simetría con valores).
    Map(Vec<(Expr, Expr)>),

    /// Rango exclusivo: `start..end`. Itera `start, start+1, ..., end-1`.
    /// Por ahora solo exclusivo; `..=` inclusive llega si hace falta.
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
    },

    /// `if condition { then } else { else_ }`. Puede usarse en posición de
    /// expresión: `let x = if cond { 1 } else { 2 }`.
    If {
        condition: Box<Expr>,
        then: Vec<Stmt>,
        else_: Option<Vec<Stmt>>,
    },

    /// `match value { pat1 => expr1, pat2 => expr2, _ => default }`.
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
}

/// Pieza de un string con interpolación.
/// Ej: `"Hola, {name}!"` se descompone en
/// `[Lit("Hola, "), Expr(Ident("name")), Lit("!")]`.
#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    /// Texto literal.
    Lit(String),
    /// Expresión cuyo resultado se convierte a string e inserta.
    Expr(Expr),
}

/// Una sentencia: ejecuta un efecto, opcionalmente produce un valor.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// Asignación / declaración. Ej: `x = 42` o `name: Str = "Fitz"`.
    /// En Fitz no diferenciamos `let x = ...` de `x = ...` a nivel AST.
    Assign {
        name: String,
        type_: Option<String>,
        value: Expr,
    },

    /// `return expr`.
    Return(Expr),

    /// Una expresión usada como sentencia (típicamente una llamada).
    Expr(Expr),

    /// Definición de función. Soporta forma de bloque y forma de flecha
    /// (`fn f(n) => n * 2`) — esta última la convierte el parser a
    /// `body: vec![Stmt::Return(Expr)]`.
    FnDef {
        name: String,
        params: Vec<Param>,
        return_type: Option<String>,
        body: Vec<Stmt>,
        is_async: bool,
    },

    /// Definición de tipo custom: `type User { id: Int, name: Str }`.
    TypeDef {
        name: String,
        fields: Vec<Field>,
    },

    /// Endpoint HTTP: `@get("/path") async fn handler(...) -> T { ... }`.
    /// TODO Fase 4: reemplazar por un esquema de decoradores genérico
    /// (`decorators: Vec<Decorator>` adentro de `FnDef`) para soportar
    /// `@server(...)` y futuros decoradores custom.
    HttpEndpoint {
        method: HttpMethod,
        path: String,
        handler: Box<Stmt>,
    },

    /// `break` dentro de loop/while/for.
    Break,

    /// `continue` dentro de loop/while/for.
    Continue,

    /// `while cond { body }`. Itera mientras `cond` evalúe a `Bool(true)`.
    /// `break` corta el loop; `continue` salta a la próxima iteración.
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },

    /// `loop { body }` — loop infinito. Solo se sale con `break` (o `return`).
    Loop {
        body: Vec<Stmt>,
    },

    /// `for var in iter { body }`. `iter` se evalúa una vez al entrar
    /// y debe ser iterable (List o Range; Map iterable cuando exista
    /// el tipo `Pair`). `var` se define en el scope del body en cada
    /// iteración. `break`/`continue` funcionan igual que en `while`.
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOpKind {
    Add, Sub, Mul, Div,
    Eq, NotEq, Lt, LtEq, Gt, GtEq,
    And, Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOpKind {
    /// Negación numérica: `-x`.
    Neg,
}

/// Parámetro formal de una función. El tipo es opcional (tipado gradual).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub type_: Option<String>,
}

/// Campo de un `type`. El tipo es obligatorio dentro de un struct.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub type_: String,
    pub nullable: bool,
    pub default: Option<Expr>,
}

/// Brazo de un `match`: patrón → expresión.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
}

/// Patrones para `match`.
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// `42` — matchea si el valor es ese int exacto. Igual para float/str/bool.
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// `null` — matchea si el valor es Null.
    Null,
    /// `nombre` — siempre matchea, bindea el valor a ese nombre.
    Ident(String),
    /// `_` — siempre matchea, sin binding.
    Wildcard,
    /// `Ok(x)` — bloqueado hasta tener tipo Result (Fase 3).
    OkBinding(String),
    /// `Err(e)` — bloqueado hasta tener tipo Result (Fase 3).
    ErrBinding(String),
    /// `start..end` — matchea si el valor es Int y `start <= v < end`.
    /// Solo Int por ahora (Float complica la representación discreta).
    Range { start: i64, end: i64 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

/// Un programa Fitz es una lista de sentencias.
pub type Program = Vec<Stmt>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Construye a mano el AST equivalente al programa:
    ///
    /// ```fitz
    /// name = "Fitz"
    /// x = 10 + 5
    /// print("Hola, {name}!")
    /// fn double(n) => n * 2
    /// print(double(x))
    /// ```
    ///
    /// Sirve como prueba de que el AST puede representar el criterio de
    /// éxito de la Fase 2, y como referencia de qué tiene que producir el
    /// parser cuando lo implementemos.
    #[test]
    fn can_represent_phase2_success_program() {
        let program: Program = vec![
            // name = "Fitz"
            Stmt::Assign {
                name: "name".into(),
                type_: None,
                value: Expr::Str("Fitz".into()),
            },
            // x = 10 + 5
            Stmt::Assign {
                name: "x".into(),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(10)),
                    right: Box::new(Expr::Int(5)),
                },
            },
            // print("Hola, {name}!")
            Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::StrInterp(vec![
                    StrPart::Lit("Hola, ".into()),
                    StrPart::Expr(Expr::Ident("name".into())),
                    StrPart::Lit("!".into()),
                ])],
            }),
            // fn double(n) => n * 2
            Stmt::FnDef {
                name: "double".into(),
                params: vec![Param { name: "n".into(), type_: None }],
                return_type: None,
                body: vec![Stmt::Return(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("n".into())),
                    right: Box::new(Expr::Int(2)),
                })],
                is_async: false,
            },
            // print(double(x))
            Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::Call {
                    name: "double".into(),
                    args: vec![Expr::Ident("x".into())],
                }],
            }),
        ];

        assert_eq!(program.len(), 5);

        // Verificación puntual: la 4ta sentencia es la fn def de `double`.
        match &program[3] {
            Stmt::FnDef { name, params, body, .. } => {
                assert_eq!(name, "double");
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "n");
                assert_eq!(body.len(), 1);
            }
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn strpart_distinguishes_literal_from_expression() {
        let parts = vec![
            StrPart::Lit("Edad: ".into()),
            StrPart::Expr(Expr::Ident("age".into())),
        ];
        assert_eq!(parts[0], StrPart::Lit("Edad: ".into()));
        assert!(matches!(parts[1], StrPart::Expr(Expr::Ident(_))));
    }

    #[test]
    fn ast_supports_break_and_continue_inside_loops() {
        // Stmt::Break y Stmt::Continue son sentencias por sí mismas.
        let stmts: Vec<Stmt> = vec![Stmt::Break, Stmt::Continue];
        assert_eq!(stmts[0], Stmt::Break);
        assert_eq!(stmts[1], Stmt::Continue);
    }

    #[test]
    fn list_literal_holds_arbitrary_exprs() {
        // `[1, x, 2 + 3]`
        let list = Expr::List(vec![
            Expr::Int(1),
            Expr::Ident("x".into()),
            Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(2)),
                right: Box::new(Expr::Int(3)),
            },
        ]);
        match list {
            Expr::List(items) => assert_eq!(items.len(), 3),
            _ => panic!("se esperaba List"),
        }
    }

    #[test]
    fn map_literal_preserva_orden_de_pares() {
        // `{"a": 1, "b": 2}`
        let map = Expr::Map(vec![
            (Expr::Str("a".into()), Expr::Int(1)),
            (Expr::Str("b".into()), Expr::Int(2)),
        ]);
        match map {
            Expr::Map(pairs) => {
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0].0, Expr::Str("a".into()));
                assert_eq!(pairs[1].1, Expr::Int(2));
            }
            _ => panic!("se esperaba Map"),
        }
    }

    #[test]
    fn range_expr_envuelve_extremos() {
        // `0..10`
        let r = Expr::Range {
            start: Box::new(Expr::Int(0)),
            end: Box::new(Expr::Int(10)),
        };
        match r {
            Expr::Range { start, end } => {
                assert_eq!(*start, Expr::Int(0));
                assert_eq!(*end, Expr::Int(10));
            }
            _ => panic!("se esperaba Range"),
        }
    }

    #[test]
    fn index_expr_envuelve_objeto_e_indice() {
        // `xs[0]`
        let ix = Expr::Index {
            object: Box::new(Expr::Ident("xs".into())),
            index: Box::new(Expr::Int(0)),
        };
        match ix {
            Expr::Index { object, index } => {
                assert_eq!(*object, Expr::Ident("xs".into()));
                assert_eq!(*index, Expr::Int(0));
            }
            _ => panic!("se esperaba Index"),
        }
    }

    #[test]
    fn for_stmt_envuelve_var_iter_y_body() {
        // `for x in xs { print(x) }`
        let f = Stmt::For {
            var: "x".into(),
            iter: Expr::Ident("xs".into()),
            body: vec![Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::Ident("x".into())],
            })],
        };
        match f {
            Stmt::For { var, iter, body } => {
                assert_eq!(var, "x");
                assert_eq!(iter, Expr::Ident("xs".into()));
                assert_eq!(body.len(), 1);
            }
            _ => panic!("se esperaba For"),
        }
    }

    #[test]
    fn pattern_range_guarda_extremos_como_int() {
        // `match n { 0..10 => "chico", _ => "grande" }` — solo el patrón.
        let p = Pattern::Range { start: 0, end: 10 };
        match p {
            Pattern::Range { start, end } => {
                assert_eq!(start, 0);
                assert_eq!(end, 10);
            }
            _ => panic!("se esperaba Range"),
        }
    }

    #[test]
    fn unary_op_negation_wraps_operand() {
        // -x → UnaryOp { op: Neg, operand: Ident("x") }
        let expr = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Ident("x".into())),
        };
        match expr {
            Expr::UnaryOp { op, operand } => {
                assert_eq!(op, UnaryOpKind::Neg);
                assert_eq!(*operand, Expr::Ident("x".into()));
            }
            _ => panic!("se esperaba UnaryOp"),
        }
    }
}
