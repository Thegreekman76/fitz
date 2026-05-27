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

// Las variantes y sus payloads documentan los tipos de error que el
// compilador/runtime puede emitir. Los campos no se leen vía accesor
// (se ven solo por Debug), pero son parte de la API: distinguen
// `UndefinedVariable("foo")` de `UndefinedFunction("bar")` al
// inspeccionar errores en tests y al imprimir con `{:?}`.
#[derive(Debug)]
#[allow(dead_code)]
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

    // ---- U1 (v0.10.13) — constructores helper para los 3 patterns
    //                    de error más frecuentes ----
    //
    // Antes los call sites del evaluator/checker/codegen formateaban
    // mensajes inconsistentes ("no tiene método X" vs "el tipo X no
    // soporta" vs "espera N args" vs "espera N argumentos"). Estos
    // helpers fijan el wording canónico y reducen duplicación.
    //
    // Migración: en cada nuevo error, preferir uno de estos
    // constructores. Migración de call sites existentes es
    // incremental — los mensajes que ya estaban no cambian de shape
    // (asegurando que tests que matchean substrings sigan verdes).

    /// "el tipo `<type_name>` no tiene un método llamado `<method>`".
    /// Para el receptor `xs.foo()` donde `foo` no existe en el tipo
    /// del receptor (List/Map/Str/Nominal). Ejemplo: dispatch_method
    /// en el evaluator cuando llega un nombre desconocido.
    pub fn method_not_found(line: usize, column: usize, type_name: &str, method: &str) -> Self {
        FitzError::new(
            ErrorKind::TypeError,
            line,
            column,
            format!(
                "el tipo `{}` no tiene un método llamado `{}`",
                type_name, method
            ),
        )
    }

    /// "la función `<name>` espera <expected> argumento(s), recibió
    /// <found>". Pluralización implícita por el `(s)`.
    pub fn wrong_arity(
        line: usize,
        column: usize,
        name: &str,
        expected: usize,
        found: usize,
    ) -> Self {
        FitzError::new(
            ErrorKind::WrongArgCount { expected, found },
            line,
            column,
            format!(
                "la función `{}` espera {} argumento(s), recibió {}",
                name, expected, found
            ),
        )
    }

    /// "<context>: esperaba `<expected>`, recibió `<found>`". `context`
    /// es una etiqueta corta del lugar donde el mismatch ocurrió
    /// (ej. "el arg 1 de `add`", "el campo `email` del struct lit",
    /// "el return de `me`"). Wording uniforme con backticks alrededor
    /// de los tipos.
    pub fn type_mismatch(
        line: usize,
        column: usize,
        context: &str,
        expected: &str,
        found: &str,
    ) -> Self {
        FitzError::new(
            ErrorKind::TypeMismatch {
                expected: expected.to_string(),
                found: found.to_string(),
            },
            line,
            column,
            format!("{}: esperaba `{}`, recibió `{}`", context, expected, found),
        )
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
            write!(
                f,
                "Error en línea {}:{} — {}",
                self.line, self.column, self.message
            )?;
        }
        if let Some(hint) = &self.hint {
            write!(f, "\n  Sugerencia: {}", hint)?;
        }
        Ok(())
    }
}

pub type FitzResult<T> = Result<T, FitzError>;
