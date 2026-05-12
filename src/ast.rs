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
// `FnExpr` repite el sufijo "Expr" del enum, pero renombrarlo a `Fn`
// chocaría con la keyword visual del lenguaje. Lo dejamos.
#[allow(clippy::enum_variant_names)]
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

    /// Llamada a función: `callee(arg1, arg2, ...)`. El `callee` es cualquier
    /// expresión que en runtime tiene que evaluar a algo invocable: un
    /// `Ident` (`f(1, 2)`), un `Field` (`xs.map(...)` — method call),
    /// una `FnExpr` invocada al vuelo (`(fn(x) => x + 1)(2)`), etc.
    ///
    /// El evaluador despacha por la forma sintáctica del callee:
    ///  - `Expr::Field { object, field }` → method call. Evalúa el receptor
    ///    y busca el método en una tabla por tipo (`(tipo, nombre) → fn`).
    ///  - otra cosa → llamada "normal". Evalúa el callee y espera
    ///    `Value::Function` o `Value::Builtin`.
    ///
    /// `Ok(...)` y `Err(...)` son keywords contextuales: cuando el callee es
    /// literalmente `Expr::Ident("Ok"|"Err")`, el parser los convierte en
    /// `Expr::Ok`/`Expr::Err` antes de construir el `Call`.
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },

    /// Función anónima en posición de expresión: `fn(x) => x * 2` o
    /// `fn(x) { return x * 2 }`. La forma flecha la convierte el parser a
    /// `body: vec![Stmt::Return(expr, Span::ZERO)]` — mismo truco que `Stmt::FnDef`.
    /// No tiene nombre; se evalúa a un `Value::Function` con closure
    /// capturando el env del lugar de definición.
    FnExpr {
        params: Vec<Param>,
        body: Vec<Stmt>,
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

    /// Instanciación de un tipo custom: `User { id: 1, name: "x" }`.
    /// `type_name` es el nombre del `type` declarado; los campos son
    /// pares `(nombre, expresión)` y se evalúan en orden de aparición.
    /// La validación contra los campos declarados (faltantes, extras,
    /// defaults, nullables) la hace el evaluador, no el parser.
    StructLit {
        type_name: String,
        fields: Vec<(String, Expr)>,
    },

    /// Constructor de la variante exitosa del tipo built-in `Result`:
    /// `Ok(expr)`. El parser lo reconoce como keyword contextual cuando
    /// ve el identificador `Ok` seguido de `(`. `Ok` no es una keyword
    /// en el lexer; sigue siendo un `Token::Ident`, pero el parser le
    /// da semántica especial acá (mismo criterio que en `parse_pattern`).
    Ok(Box<Expr>),

    /// Constructor de la variante de error: `Err(expr)`. Mismas reglas
    /// que `Ok`. Convención (no validada en runtime hasta el type checker
    /// de Fase 5): el inner suele ser un `Str` con el mensaje.
    Err(Box<Expr>),

    /// Operador `?` postfix: `expr?`. En runtime, si el operando es
    /// `Ok(v)` la expresión vale `v`; si es `Err(e)` corta la función
    /// contenedora con `return Err(e)`. Aplicable a cualquier expresión;
    /// el chequeo de que sea un `Result` ocurre en el evaluador, no acá.
    Try(Box<Expr>),
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

/// Destino de una asignación: a qué se le está asignando.
///
/// Hasta 3.3 solo soportábamos asignación a un identificador. En 3.4
/// abrimos asignación a campo (`user.name = "x"`) para destrabar mutación
/// de instancias. Asignación a índice (`xs[0] = v`) sigue siendo deuda
/// explícita: cuando entre, se suma una variante acá.
#[derive(Debug, Clone, PartialEq)]
pub enum AssignTarget {
    /// `x = ...` — declaración o reasignación de una variable.
    Ident(String),
    /// `objeto.campo = ...` — mutación de un campo de una `Instance`.
    /// `object` es cualquier expresión que evalúe a `Value::Instance`;
    /// el evaluador chequea esto en runtime y emite error si no.
    Field {
        object: Box<Expr>,
        field: String,
    },
}

/// Posición de un nodo del AST en el archivo fuente. La acompaña a
/// cada `Stmt` desde B.1 — sirve para enriquecer los mensajes de
/// error del checker/evaluador con línea y columna reales (antes
/// quedaban siempre en `0:0`).
///
/// Para nodos sintéticos (construidos por el parser sin un token
/// concreto, p.ej. el cuerpo flecha de `fn f(x) => x * 2` que se
/// convierte a `Return`), o para nodos de tests, usamos
/// `Span::default()` (= `Span::ZERO`, ambos campos a 0). El sitio
/// que reporta error chequea `is_known()` antes de citar posición.
#[derive(Debug, Clone, Copy, Default)]
pub struct Span {
    pub line: usize,
    pub column: usize,
}

impl Span {
    pub const ZERO: Span = Span { line: 0, column: 0 };

    pub fn new(line: usize, column: usize) -> Self {
        Span { line, column }
    }

    /// `true` si el span apunta a una posición real (no fue
    /// completado con `default()`). Lo usan los formatters de error
    /// para decidir si citan línea/columna o solo el mensaje.
    /// Mantenido como API pública para uso futuro en
    /// `FitzError::Display`.
    #[allow(dead_code)]
    pub fn is_known(&self) -> bool {
        self.line != 0 || self.column != 0
    }
}

/// `Span` se compara como **siempre igual** a sí mismo. Razón: los
/// tests del AST/parser/evaluator usan `assert_eq!` sobre `Stmt` y
/// `Expr` construyendo nodos literales con `Span::ZERO`, contra
/// nodos producidos por el parser con spans reales. Si la comparación
/// fuera estructural sobre `line`/`column`, ~30 tests deberían
/// duplicar la lógica del parser para predecir las posiciones — sin
/// valor real (los tests miran estructura, no posición). Cuando hace
/// falta validar la posición, se compara explícitamente `span.line`
/// y `span.column` (ver tests dedicados de span en parser y checker).
impl PartialEq for Span {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}
impl Eq for Span {}

/// Una sentencia: ejecuta un efecto, opcionalmente produce un valor.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// Asignación / declaración. Ej: `x = 42`, `name: Str = "Fitz"`,
    /// `user.name = "Otro"`. En Fitz no diferenciamos `let x = ...` de
    /// `x = ...` a nivel AST. La anotación de tipo `type_` solo es
    /// válida cuando `target` es `Ident` (asignar a un campo no admite
    /// reanotar el tipo); el parser lo enforcea.
    Assign {
        target: AssignTarget,
        type_: Option<TypeExpr>,
        value: Expr,
        span: Span,
    },

    /// `return expr`.
    Return(Expr, Span),

    /// Una expresión usada como sentencia (típicamente una llamada).
    Expr(Expr, Span),

    /// Definición de función. Soporta forma de bloque y forma de flecha
    /// (`fn f(n) => n * 2`) — esta última la convierte el parser a
    /// `body: vec![Stmt::Return(Expr, Span::ZERO)]`.
    ///
    /// `decorators` lista los `@deco(args...)` que envuelven la función,
    /// en el orden en que aparecen en el código fuente. El parser solo
    /// los acumula; la semántica (qué hace cada decorator) la decide el
    /// evaluador despachando por `Decorator.name`. Para una función sin
    /// decoradores el vector está vacío.
    FnDef {
        name: String,
        params: Vec<Param>,
        return_type: Option<TypeExpr>,
        body: Vec<Stmt>,
        is_async: bool,
        decorators: Vec<Decorator>,
        span: Span,
    },

    /// Definición de tipo custom: `type User { id: Int, name: Str }`.
    TypeDef {
        name: String,
        fields: Vec<Field>,
        span: Span,
    },

    /// `break` dentro de loop/while/for.
    Break(Span),

    /// `continue` dentro de loop/while/for.
    Continue(Span),

    /// `while cond { body }`. Itera mientras `cond` evalúe a `Bool(true)`.
    /// `break` corta el loop; `continue` salta a la próxima iteración.
    While {
        condition: Expr,
        body: Vec<Stmt>,
        span: Span,
    },

    /// `loop { body }` — loop infinito. Solo se sale con `break` (o `return`).
    Loop {
        body: Vec<Stmt>,
        span: Span,
    },

    /// `for var in iter { body }`. `iter` se evalúa una vez al entrar
    /// y debe ser iterable (List o Range; Map iterable cuando exista
    /// el tipo `Pair`). `var` se define en el scope del body en cada
    /// iteración. `break`/`continue` funcionan igual que en `while`.
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
        span: Span,
    },

    /// `import foo` o `import foo.bar.baz` — carga un módulo desde
    /// disco y lo expone en el scope actual como `Value::Module` bajo
    /// el ÚLTIMO segmento del path (`import foo.bar` → binding `bar`).
    /// Resolución: relativo al archivo importer, `foo.bar` → `./foo/bar.fitz`.
    ///
    /// Para acceder a algo adentro: `bar.fn(...)`. Para traer nombres
    /// directos al scope, usar `Stmt::FromImport`.
    Import {
        /// Segmentos del path en orden. Siempre tiene al menos un elemento.
        path: Vec<String>,
        span: Span,
    },

    /// `from foo import a, b, c` o `from foo.bar import x` — carga el
    /// módulo y bindea cada nombre listado en el scope actual. El módulo
    /// no queda expuesto como tal.
    ///
    /// El parser garantiza que `names` no esté vacío.
    FromImport {
        path: Vec<String>,
        names: Vec<String>,
        span: Span,
    },
}

impl Stmt {
    /// Devuelve el span de cualquier variante. Helper para los sitios
    /// que reportan errores sobre un stmt sin matchear por variante.
    /// Mantenido como API pública para tests y uso futuro.
    #[allow(dead_code)]
    pub fn span(&self) -> Span {
        match self {
            Stmt::Assign { span, .. } => *span,
            Stmt::Return(_, span) => *span,
            Stmt::Expr(_, span) => *span,
            Stmt::FnDef { span, .. } => *span,
            Stmt::TypeDef { span, .. } => *span,
            Stmt::Break(span) => *span,
            Stmt::Continue(span) => *span,
            Stmt::While { span, .. } => *span,
            Stmt::Loop { span, .. } => *span,
            Stmt::For { span, .. } => *span,
            Stmt::Import { span, .. } => *span,
            Stmt::FromImport { span, .. } => *span,
        }
    }
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
    pub type_: Option<TypeExpr>,
}

/// Campo de un `type`. El tipo es obligatorio dentro de un struct.
/// La nullabilidad (`T?`) se modela adentro del `TypeExpr` como
/// `TypeExpr::Nullable(...)`, no como un flag aparte.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub type_: TypeExpr,
    pub default: Option<Expr>,
}

/// Una expresión de tipo en una anotación. Es el AST que produce el
/// parser cuando ve algo como `Int`, `List<Int>`, `Map<Str, User>`,
/// `Result<List<User>>`, `User?`. No tiene resolución todavía: el
/// `Named(...)` puede referirse a un built-in (`Int`, `Str`, ...),
/// a un tipo declarado por el usuario, o a uno importado. El checker
/// (Fase 5.2) es quien valida que cada nombre exista y que la aridad
/// de los genéricos sea correcta.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// Nombre simple: `Int`, `Str`, `User`.
    Named(String),
    /// Genérico aplicado: `List<Int>`, `Map<Str, User>`, `Result<T>`.
    /// `args` siempre tiene al menos un elemento (`Foo<>` es error de
    /// parser).
    Generic { name: String, args: Vec<TypeExpr> },
    /// Sufijo `?`: el valor puede ser ese tipo o `Null`. `User?` →
    /// `Nullable(Box(Named("User")))`. `List<Int>?` →
    /// `Nullable(Box(Generic { name: "List", args: [Named("Int")] }))`.
    Nullable(Box<TypeExpr>),
    /// Tipo función: `Fn(T1, T2) -> U`. Modela un valor invocable con
    /// los parámetros y retorno indicados. La keyword `Fn` no es un
    /// tipo nominal del lenguaje: el parser la reconoce de forma
    /// dedicada cuando ve `Fn` seguido de `(`. `Fn() -> U` es válido
    /// (cero params). El retorno es obligatorio en la sintaxis para
    /// evitar ambigüedad con `Fn(...)` como expresión.
    Function {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
    },
}

impl TypeExpr {
    /// Atajo para los call sites más comunes (tests, builtins).
    /// Mantenido como API pública para tests futuros.
    #[allow(dead_code)]
    pub fn named(s: impl Into<String>) -> Self {
        TypeExpr::Named(s.into())
    }

    /// Reproduce la forma escrita en el fuente. Usado en errores y en
    /// el runtime HTTP para mostrar el tipo declarado.
    pub fn display_name(&self) -> String {
        match self {
            TypeExpr::Named(name) => name.clone(),
            TypeExpr::Generic { name, args } => {
                let inner: Vec<String> = args.iter().map(|a| a.display_name()).collect();
                format!("{}<{}>", name, inner.join(", "))
            }
            TypeExpr::Nullable(inner) => format!("{}?", inner.display_name()),
            TypeExpr::Function { params, ret } => {
                let ps: Vec<String> = params.iter().map(|p| p.display_name()).collect();
                format!("Fn({}) -> {}", ps.join(", "), ret.display_name())
            }
        }
    }

    /// Nombre "cabeza" del tipo, ignorando nullables y argumentos
    /// genéricos. Útil para el runtime HTTP cuando necesita resolver
    /// un nombre de tipo declarado en el env del importer.
    /// `User?` → `"User"`, `List<Int>` → `"List"`,
    /// `Result<List<User>>` → `"Result"`.
    pub fn head_name(&self) -> &str {
        match self {
            TypeExpr::Named(name) => name,
            TypeExpr::Generic { name, .. } => name,
            TypeExpr::Nullable(inner) => inner.head_name(),
            TypeExpr::Function { .. } => "Fn",
        }
    }

    /// `true` si el tipo es `T?` (admite `Null` además del tipo base).
    pub fn is_nullable(&self) -> bool {
        matches!(self, TypeExpr::Nullable(_))
    }
}

impl std::fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.display_name())
    }
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
    /// `Ok(x)` — matchea cualquier `Result::Ok(...)` y bindea el
    /// inner como `x`.
    OkBinding(String),
    /// `Err(e)` — matchea cualquier `Result::Err(...)` y bindea el
    /// inner como `e`.
    ErrBinding(String),
    /// `Ok(_)` — matchea cualquier `Result::Ok(...)` sin bindear
    /// (no ensucia el scope con una var llamada `_`).
    OkWildcard,
    /// `Err(_)` — matchea cualquier `Result::Err(...)` sin bindear.
    ErrWildcard,
    /// `start..end` — matchea si el valor es Int y `start <= v < end`.
    /// Solo Int por ahora (Float complica la representación discreta).
    Range { start: i64, end: i64 },
}

/// Decorador aplicado a una `Stmt::FnDef`: `@nombre(args...)`.
///
/// En 4.1 el parser solo acumula decoradores; el evaluador es quien
/// despacha por nombre (`@get`/`@post`/`@put`/`@delete` registran rutas
/// HTTP cuando llegue 4.2, `@server` configura el runtime, cualquier
/// otro → error explícito "decorator desconocido"). Args son
/// expresiones cualquiera, validadas en runtime por el decorator
/// específico (ej.: `@get` exige un único `Str` con la ruta).
#[derive(Debug, Clone, PartialEq)]
pub struct Decorator {
    pub name: String,
    pub args: Vec<Expr>,
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
                target: AssignTarget::Ident("name".into()),
                type_: None,
                value: Expr::Str("Fitz".into()),
             span: Span::ZERO },
            // x = 10 + 5
            Stmt::Assign {
                target: AssignTarget::Ident("x".into()),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(10)),
                    right: Box::new(Expr::Int(5)),
                },
             span: Span::ZERO },
            // print("Hola, {name}!")
            Stmt::Expr(Expr::Call {
                callee: Box::new(Expr::Ident("print".into())),
                args: vec![Expr::StrInterp(vec![
                    StrPart::Lit("Hola, ".into()),
                    StrPart::Expr(Expr::Ident("name".into())),
                    StrPart::Lit("!".into()),
                ])],
            }, Span::ZERO),
            // fn double(n) => n * 2
            Stmt::FnDef {
                name: "double".into(),
                params: vec![Param { name: "n".into(), type_: None }],
                return_type: None,
                body: vec![Stmt::Return(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("n".into())),
                    right: Box::new(Expr::Int(2)),
                }, Span::ZERO)],
                is_async: false,
                decorators: vec![],
             span: Span::ZERO },
            // print(double(x))
            Stmt::Expr(Expr::Call {
                callee: Box::new(Expr::Ident("print".into())),
                args: vec![Expr::Call {
                    callee: Box::new(Expr::Ident("double".into())),
                    args: vec![Expr::Ident("x".into())],
                }],
            }, Span::ZERO),
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
        let parts = [
            StrPart::Lit("Edad: ".into()),
            StrPart::Expr(Expr::Ident("age".into())),
        ];
        assert_eq!(parts[0], StrPart::Lit("Edad: ".into()));
        assert!(matches!(parts[1], StrPart::Expr(Expr::Ident(_))));
    }

    #[test]
    fn ast_supports_break_and_continue_inside_loops() {
        // Stmt::Break y Stmt::Continue son sentencias por sí mismas.
        let stmts: Vec<Stmt> = vec![Stmt::Break(Span::ZERO), Stmt::Continue(Span::ZERO)];
        assert_eq!(stmts[0], Stmt::Break(Span::ZERO));
        assert_eq!(stmts[1], Stmt::Continue(Span::ZERO));
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
                callee: Box::new(Expr::Ident("print".into())),
                args: vec![Expr::Ident("x".into())],
            }, Span::ZERO)],
         span: Span::ZERO };
        match f {
            Stmt::For { var, iter, body, .. } => {
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
    fn struct_lit_guarda_tipo_y_campos_en_orden() {
        // `User { id: 1, name: "x" }`
        let lit = Expr::StructLit {
            type_name: "User".into(),
            fields: vec![
                ("id".into(), Expr::Int(1)),
                ("name".into(), Expr::Str("x".into())),
            ],
        };
        match lit {
            Expr::StructLit { type_name, fields } => {
                assert_eq!(type_name, "User");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0, "id");
                assert_eq!(fields[0].1, Expr::Int(1));
                assert_eq!(fields[1].0, "name");
                assert_eq!(fields[1].1, Expr::Str("x".into()));
            }
            _ => panic!("se esperaba StructLit"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Result (Fase 3, paso 3: Result + Ok/Err + `?`)
    // -----------------------------------------------------------------------

    #[test]
    fn ok_ctor_envuelve_inner() {
        // `Ok(42)` → Expr::Ok(Box(Int(42)))
        let e = Expr::Ok(Box::new(Expr::Int(42)));
        match e {
            Expr::Ok(inner) => assert_eq!(*inner, Expr::Int(42)),
            _ => panic!("se esperaba Ok"),
        }
    }

    #[test]
    fn err_ctor_envuelve_inner() {
        // `Err("boom")` → Expr::Err(Box(Str("boom")))
        let e = Expr::Err(Box::new(Expr::Str("boom".into())));
        match e {
            Expr::Err(inner) => assert_eq!(*inner, Expr::Str("boom".into())),
            _ => panic!("se esperaba Err"),
        }
    }

    #[test]
    fn try_expr_envuelve_operando() {
        // `x?` → Expr::Try(Box(Ident("x")))
        let e = Expr::Try(Box::new(Expr::Ident("x".into())));
        match e {
            Expr::Try(inner) => assert_eq!(*inner, Expr::Ident("x".into())),
            _ => panic!("se esperaba Try"),
        }
    }

    #[test]
    fn try_y_ctors_son_componibles() {
        // `Ok(get(id)?)` — un `?` adentro de un constructor `Ok`.
        let e = Expr::Ok(Box::new(Expr::Try(Box::new(Expr::Call {
            callee: Box::new(Expr::Ident("get".into())),
            args: vec![Expr::Ident("id".into())],
        }))));
        if let Expr::Ok(inner) = e {
            assert!(matches!(*inner, Expr::Try(_)));
        } else {
            panic!("se esperaba Ok");
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

    // -----------------------------------------------------------------------
    // Tests — Fase 3, paso 4 (funciones anónimas + method calls + mutación)
    // -----------------------------------------------------------------------

    #[test]
    fn call_admite_callee_como_expresion() {
        // `xs.map(f)` → Call con callee = Field { object: xs, field: "map" }.
        let call = Expr::Call {
            callee: Box::new(Expr::Field {
                object: Box::new(Expr::Ident("xs".into())),
                field: "map".into(),
            }),
            args: vec![Expr::Ident("f".into())],
        };
        match call {
            Expr::Call { callee, args } => {
                assert!(matches!(*callee, Expr::Field { .. }));
                assert_eq!(args.len(), 1);
            }
            _ => panic!("se esperaba Call"),
        }
    }

    #[test]
    fn fn_expr_envuelve_params_y_body() {
        // `fn(x) => x * 2` — versión sin nombre.
        let fnexpr = Expr::FnExpr {
            params: vec![Param { name: "x".into(), type_: None }],
            body: vec![Stmt::Return(Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::Ident("x".into())),
                right: Box::new(Expr::Int(2)),
            }, Span::ZERO)],
        };
        match fnexpr {
            Expr::FnExpr { params, body } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "x");
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0], Stmt::Return(_, _)));
            }
            _ => panic!("se esperaba FnExpr"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Fase 3, paso 5 (módulos / import)
    // -----------------------------------------------------------------------

    #[test]
    fn import_simple_guarda_path_de_un_segmento() {
        // `import utils` → Stmt::Import { path: ["utils"] , span: Span::ZERO }
        let s = Stmt::Import { path: vec!["utils".into()] , span: Span::ZERO };
        match s {
            Stmt::Import { path, .. } => {
                assert_eq!(path, vec!["utils".to_string()]);
            }
            _ => panic!("se esperaba Import"),
        }
    }

    #[test]
    fn import_punteado_guarda_segmentos_en_orden() {
        // `import sub.foo` → Stmt::Import { path: ["sub", "foo"] , span: Span::ZERO }
        let s = Stmt::Import { path: vec!["sub".into(), "foo".into()] , span: Span::ZERO };
        match s {
            Stmt::Import { path, .. } => {
                assert_eq!(path.len(), 2);
                assert_eq!(path[0], "sub");
                assert_eq!(path[1], "foo");
            }
            _ => panic!("se esperaba Import"),
        }
    }

    #[test]
    fn from_import_guarda_path_y_nombres() {
        // `from utils import slugify, parse`
        let s = Stmt::FromImport {
            path: vec!["utils".into()],
            names: vec!["slugify".into(), "parse".into()],
         span: Span::ZERO };
        match s {
            Stmt::FromImport { path, names, .. } => {
                assert_eq!(path, vec!["utils".to_string()]);
                assert_eq!(names, vec!["slugify".to_string(), "parse".to_string()]);
            }
            _ => panic!("se esperaba FromImport"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Fase 4, paso 4.1 (decoradores genéricos sobre FnDef)
    // -----------------------------------------------------------------------

    #[test]
    fn decorator_guarda_nombre_y_args() {
        // `@get("/users/{id}")` — un decorator con un arg string.
        let d = Decorator {
            name: "get".into(),
            args: vec![Expr::Str("/users/{id}".into())],
        };
        assert_eq!(d.name, "get");
        assert_eq!(d.args.len(), 1);
        assert_eq!(d.args[0], Expr::Str("/users/{id}".into()));
    }

    #[test]
    fn fndef_default_sin_decorators_vector_vacio() {
        // Una fn declarada sin `@...` arriba debe tener `decorators` vacío.
        let f = Stmt::FnDef {
            name: "f".into(),
            params: vec![],
            return_type: None,
            body: vec![],
            is_async: false,
            decorators: vec![],
         span: Span::ZERO };
        if let Stmt::FnDef { decorators, .. } = f {
            assert!(decorators.is_empty());
        } else {
            panic!("se esperaba FnDef");
        }
    }

    #[test]
    fn fndef_admite_varios_decorators_en_orden() {
        // `@get("/x") @auth("admin") fn h() {}` — dos decoradores apilados.
        let f = Stmt::FnDef {
            name: "h".into(),
            params: vec![],
            return_type: None,
            body: vec![],
            is_async: false,
            decorators: vec![
                Decorator { name: "get".into(), args: vec![Expr::Str("/x".into())] },
                Decorator { name: "auth".into(), args: vec![Expr::Str("admin".into())] },
            ],
         span: Span::ZERO };
        if let Stmt::FnDef { decorators, .. } = f {
            assert_eq!(decorators.len(), 2);
            assert_eq!(decorators[0].name, "get");
            assert_eq!(decorators[1].name, "auth");
        } else {
            panic!("se esperaba FnDef");
        }
    }

    #[test]
    fn assign_target_admite_ident_y_field() {
        // `x = 1` — target Ident.
        let s1 = Stmt::Assign {
            target: AssignTarget::Ident("x".into()),
            type_: None,
            value: Expr::Int(1),
         span: Span::ZERO };
        if let Stmt::Assign { target, .. } = s1 {
            assert_eq!(target, AssignTarget::Ident("x".into()));
        } else {
            panic!("se esperaba Assign");
        }

        // `user.name = "x"` — target Field.
        let s2 = Stmt::Assign {
            target: AssignTarget::Field {
                object: Box::new(Expr::Ident("user".into())),
                field: "name".into(),
            },
            type_: None,
            value: Expr::Str("x".into()),
         span: Span::ZERO };
        if let Stmt::Assign { target: AssignTarget::Field { object, field }, .. } = s2 {
            assert_eq!(*object, Expr::Ident("user".into()));
            assert_eq!(field, "name");
        } else {
            panic!("se esperaba Assign con target Field");
        }
    }

    // -----------------------------------------------------------------------
    // Tests — TypeExpr (Fase 5, paso 5.1)
    // -----------------------------------------------------------------------

    #[test]
    fn type_expr_named_display_es_el_nombre() {
        assert_eq!(TypeExpr::named("Int").display_name(), "Int");
        assert_eq!(TypeExpr::named("User").display_name(), "User");
    }

    #[test]
    fn type_expr_generic_display_con_args() {
        let t = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int")],
        };
        assert_eq!(t.display_name(), "List<Int>");

        let m = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::named("Str"), TypeExpr::named("User")],
        };
        assert_eq!(m.display_name(), "Map<Str, User>");
    }

    #[test]
    fn type_expr_nullable_display_con_signo_de_pregunta() {
        let t = TypeExpr::Nullable(Box::new(TypeExpr::named("Str")));
        assert_eq!(t.display_name(), "Str?");
    }

    #[test]
    fn type_expr_display_anidado_preserva_estructura() {
        // Result<List<User>?>
        let t = TypeExpr::Generic {
            name: "Result".into(),
            args: vec![TypeExpr::Nullable(Box::new(TypeExpr::Generic {
                name: "List".into(),
                args: vec![TypeExpr::named("User")],
            }))],
        };
        assert_eq!(t.display_name(), "Result<List<User>?>");
    }

    #[test]
    fn type_expr_head_name_ignora_genericos_y_nullables() {
        assert_eq!(TypeExpr::named("User").head_name(), "User");

        let g = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int")],
        };
        assert_eq!(g.head_name(), "List");

        let n = TypeExpr::Nullable(Box::new(g));
        assert_eq!(n.head_name(), "List");

        // Nullable de Nullable cae al fondo.
        let nn = TypeExpr::Nullable(Box::new(TypeExpr::Nullable(Box::new(
            TypeExpr::named("Int"),
        ))));
        assert_eq!(nn.head_name(), "Int");
    }

    #[test]
    fn type_expr_is_nullable_solo_a_nivel_top() {
        assert!(!TypeExpr::named("Int").is_nullable());
        assert!(TypeExpr::Nullable(Box::new(TypeExpr::named("Int"))).is_nullable());
        // `List<Int?>` no es nullable en sí; el campo interno sí.
        let outer = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Nullable(Box::new(TypeExpr::named("Int")))],
        };
        assert!(!outer.is_nullable());
    }
}
