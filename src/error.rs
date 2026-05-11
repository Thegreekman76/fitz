// error.rs — Errores del compilador/intérprete de Fitz
//
// Los errores deben ser útiles. Siempre incluir:
// - Qué salió mal
// - Dónde (línea y columna)
// - Cómo arreglarlo (cuando sea posible)

#[derive(Debug)]
pub struct FitzError {
    pub kind: ErrorKind,
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub hint: Option<String>,
}

#[derive(Debug)]
pub enum ErrorKind {
    // Errores de lexer
    UnexpectedChar(char),
    UnterminatedString,
    UnterminatedComment,

    // Errores de parser
    UnexpectedToken,
    MissingClosingBrace,
    InvalidSyntax,

    // Errores de evaluador
    UndefinedVariable(String),
    UndefinedFunction(String),
    TypeMismatch { expected: String, found: String },
    DivisionByZero,
    NullReference,
    ReturnOutsideFunction,
    BreakOutsideLoop,
    ContinueOutsideLoop,
    WrongArgCount { expected: usize, found: usize },

    // Errores del checker estático (Fase 5)
    TypeError,
}

impl FitzError {
    pub fn new(kind: ErrorKind, line: usize, column: usize, message: impl Into<String>) -> Self {
        FitzError {
            kind,
            line,
            column,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl std::fmt::Display for FitzError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // line == 0 && column == 0 indica "sin posición" — algunos
        // errores del evaluator y todos los del checker estático
        // todavía no llevan línea/columna (el AST no las propaga).
        // En ese caso, omitimos el prefijo para no mentir.
        if self.line == 0 && self.column == 0 {
            write!(f, "Error — {}", self.message)?;
        } else {
            write!(f, "Error en línea {}:{} — {}", self.line, self.column, self.message)?;
        }
        if let Some(hint) = &self.hint {
            write!(f, "\n  Sugerencia: {}", hint)?;
        }
        Ok(())
    }
}

pub type FitzResult<T> = Result<T, FitzError>;
