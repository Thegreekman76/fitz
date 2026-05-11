// ast.rs — Fase 2.2
//
// Define las estructuras de datos que representan el programa en memoria.
// El parser construye este árbol, el evaluador lo recorre.
//
// TODO: implementar en Fase 2

/// Una expresión — produce un valor
#[derive(Debug, Clone)]
pub enum Expr {
    // Literales
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,

    // Identificador (variable, función, etc.)
    Ident(String),

    // Operación binaria: left OP right
    BinOp {
        op: BinOpKind,
        left: Box<Expr>,
        right: Box<Expr>,
    },

    // Llamada a función: name(args...)
    Call {
        name: String,
        args: Vec<Expr>,
    },

    // Acceso a campo: objeto.campo
    Field {
        object: Box<Expr>,
        field: String,
    },

    // Si condition { then } else { else_ }
    If {
        condition: Box<Expr>,
        then: Vec<Stmt>,
        else_: Option<Vec<Stmt>>,
    },

    // match value { pattern => expr, ... }
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
}

/// Una sentencia — no produce valor, tiene efecto
#[derive(Debug, Clone)]
pub enum Stmt {
    // asignación: name = value o name: Type = value
    Assign {
        name: String,
        type_: Option<String>,
        value: Expr,
    },

    // return value
    Return(Expr),

    // expresión como sentencia (ej: una llamada a función)
    Expr(Expr),

    // definición de función
    FnDef {
        name: String,
        params: Vec<Param>,
        return_type: Option<String>,
        body: Vec<Stmt>,
        is_async: bool,
    },

    // definición de tipo
    TypeDef {
        name: String,
        fields: Vec<Field>,
    },

    // endpoint HTTP
    HttpEndpoint {
        method: HttpMethod,
        path: String,
        handler: Box<Stmt>, // FnDef
    },
}

#[derive(Debug, Clone)]
pub enum BinOpKind {
    Add, Sub, Mul, Div,
    Eq, NotEq, Lt, LtEq, Gt, GtEq,
    And, Or,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub type_: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub type_: String,
    pub nullable: bool,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Ident(String),
    Wildcard,           // _
    OkBinding(String),  // Ok(x)
    ErrBinding(String), // Err(e)
}

#[derive(Debug, Clone)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

/// El programa completo es una lista de sentencias
pub type Program = Vec<Stmt>;
