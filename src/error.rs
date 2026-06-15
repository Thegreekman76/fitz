// error.rs — Compiler / interpreter errors for Fitz.
//
// Errors must be useful. Always include:
// - What went wrong
// - Where (line and column)
// - How to fix it (when possible)

#[derive(Debug)]
pub struct FitzError {
    pub kind: ErrorKind,
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub hint: Option<String>,
}

// The variants and their payloads document the error kinds that the
// compiler/runtime can emit. The fields are not read through accessors
// (only via Debug), but they are part of the API: they distinguish
// `UndefinedVariable("foo")` from `UndefinedFunction("bar")` when
// inspecting errors in tests and when printing with `{:?}`.
#[derive(Debug)]
#[allow(dead_code)]
pub enum ErrorKind {
    // Lexer errors
    UnexpectedChar(char),
    UnterminatedString,
    UnterminatedComment,

    // Parser errors
    UnexpectedToken,
    MissingClosingBrace,
    InvalidSyntax,

    // Evaluator errors
    UndefinedVariable(String),
    UndefinedFunction(String),
    TypeMismatch { expected: String, found: String },
    DivisionByZero,
    NullReference,
    ReturnOutsideFunction,
    BreakOutsideLoop,
    ContinueOutsideLoop,
    WrongArgCount { expected: usize, found: usize },

    // Static checker errors (Phase 5)
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

    // ---- U1 (v0.10.13) — helper constructors for the 3 most
    //                    frequent error patterns ----
    //
    // Previously the evaluator/checker/codegen call sites formatted
    // inconsistent messages ("has no method X" vs "type X does not
    // support" vs "expects N args" vs "expects N arguments"). These
    // helpers pin the canonical wording and reduce duplication.
    //
    // Migration: for every new error, prefer one of these
    // constructors. Migration of existing call sites is incremental
    // — messages already in place keep their shape (so tests
    // matching substrings stay green).

    /// "type `<type_name>` has no method named `<method>`".
    /// For the receiver `xs.foo()` where `foo` does not exist in the
    /// receiver type (List/Map/Str/Nominal). Example: dispatch_method
    /// in the evaluator when an unknown name arrives.
    pub fn method_not_found(line: usize, column: usize, type_name: &str, method: &str) -> Self {
        FitzError::new(
            ErrorKind::TypeError,
            line,
            column,
            format!("type `{}` has no method named `{}`", type_name, method),
        )
    }

    /// "function `<name>` expects <expected> argument(s), received
    /// <found>". Pluralisation implied by the `(s)`.
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
                "function `{}` expects {} argument(s), received {}",
                name, expected, found
            ),
        )
    }

    /// "<context>: expected `<expected>`, received `<found>`". `context`
    /// is a short label of where the mismatch happened (e.g. "arg 1
    /// of `add`", "field `email` of struct lit", "return of `me`").
    /// Uniform wording with backticks around the types.
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
            format!("{}: expected `{}`, received `{}`", context, expected, found),
        )
    }
}

impl std::fmt::Display for FitzError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // line == 0 && column == 0 indicates "no position" — some
        // evaluator errors and all static-checker errors still do
        // not carry a line/column (the AST does not propagate them).
        // In that case we drop the prefix so we do not lie.
        if self.line == 0 && self.column == 0 {
            write!(f, "Error — {}", self.message)?;
        } else {
            write!(
                f,
                "Error at line {}:{} — {}",
                self.line, self.column, self.message
            )?;
        }
        if let Some(hint) = &self.hint {
            write!(f, "\n  Hint: {}", hint)?;
        }
        Ok(())
    }
}

pub type FitzResult<T> = Result<T, FitzError>;
