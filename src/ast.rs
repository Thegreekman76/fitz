// ast.rs — Phase 2.2
//
// Defines the data structures that represent a program in memory.
// The parser builds this tree from the tokens; the evaluator walks
// it to execute.
//
// Conventions:
//  - `Expr` produces a value (has a type).
//  - `Stmt` produces an effect (does not necessarily have a value).
//  - Recursion uses `Box<Expr>` because Rust needs compile-time
//    known sizes for enums.

/// An expression: produces a value.
///
/// Every variant carries its `Span` (source position) as the last
/// component. For synthetic nodes produced by the parser or tests,
/// the span may be `Span::ZERO`. `Span` comparison is trivial
/// (always equal) — see `impl PartialEq for Span` — so `assert_eq!`s
/// on `Expr` do not break because of position differences.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum Expr {
    // ---------- literals ----------
    Int(i64, Span),
    Float(f64, Span),
    Str(String, Span),
    StrInterp(Vec<StrPart>, Span),
    Bool(bool, Span),
    Null(Span),
    /// Mini-batch Bytes — bytes literal `b"..."`.
    Bytes(Vec<u8>, Span),

    /// Reference to an identifier (variable, parameter, function, etc.).
    Ident(String, Span),

    /// Binary operation. `span` points at the operator.
    BinOp {
        op: BinOpKind,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },

    /// Numeric negation `-x`. `span` points at the `-`.
    UnaryOp {
        op: UnaryOpKind,
        operand: Box<Expr>,
        span: Span,
    },

    /// Call `callee(arg1, ...)`. `span` points at the `(`.
    /// Method calls: `callee` is `Expr::Field`. `Ok(...)`/`Err(...)`
    /// are rewritten by the parser to `Expr::Ok`/`Expr::Err`.
    ///
    /// Mini-batch Fp.3 — named args: named args (`name: value`)
    /// appear as `Expr::NamedArg { name, value }` inside `args`.
    /// Positionals are any other variant. Canonical rule: positionals
    /// first, named after. The parser validates this.
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },

    /// Mini-batch Fp.3 — named argument in a call
    /// (`greet(name: "Fitz")`). Only valid inside `Call.args`. The
    /// checker/evaluator/codegen desugar it by mapping `name` to the
    /// matching param position; the `value`s get reordered.
    NamedArg {
        name: String,
        value: Box<Expr>,
        span: Span,
    },

    /// Anonymous function `fn(x) => ...` or `fn(x) { ... }`. The
    /// arrow form is rewritten by the parser to
    /// `body: vec![Stmt::Return(expr, ...)]`. Mini-batch Async-cl —
    /// the `async fn(...)` prefix marks the closure as async; the
    /// body can use `.await` and the fn returns a `Future<T>` that
    /// the caller must `.await`.
    FnExpr {
        params: Vec<Param>,
        body: Vec<Stmt>,
        is_async: bool,
        span: Span,
    },

    /// Field access `object.field`. `span` points at the `.`.
    Field {
        object: Box<Expr>,
        field: String,
        span: Span,
    },

    /// Postfix indexing `object[index]`. `span` points at the `[`.
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },

    /// Slicing `xs[a..b]`, `xs[..b]`, `xs[a..]`, `xs[..]`,
    /// `xs[a..=b]` (I.2, mini-batch I). Returns a copy.
    /// `start: None` → from the start; `end: None` → to the end.
    /// Out-of-range gets clamped (Python style, no panic). Supports
    /// `List<T>` receivers (returns `List<T>`) and `Str` (returns
    /// `Str`).
    Slice {
        object: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
        span: Span,
    },

    /// List literal `[1, 2, 3]`, `[]`. Nestable.
    List(Vec<Expr>, Span),

    /// List comprehension (mini-batch C). `[expr for var in iter]` or
    /// `[expr for var in iter if filter]`. Dedicated AST node (not
    /// desugared to `.map()`) so fmt preserves the original syntax
    /// and checker errors point at the actual `for`.
    ///
    /// Post-Cmp+ coverage:
    ///   - `iter` may be `List<T>` or `Range` (same semantics as
    ///     `for ... in`).
    ///   - `var` is a Pattern (Ident, Wildcard, Tuple for destructure).
    ///   - Inline `if cond` filter is optional.
    ///   - Multiple `for` clauses (cartesian product) via `extra_clauses`.
    ListComp {
        /// Expression evaluated for each iteration and accumulated
        /// into the result list.
        expr: Box<Expr>,
        /// Binding of the FIRST for. Pattern (Ident/Wildcard/Tuple).
        var: crate::ast::Pattern,
        /// Iterable of the first for: `List<T>` or `Range`.
        iter: Box<Expr>,
        /// Mini-batch Cmp+ — additional `for` clauses for cartesian
        /// product: `[expr for a in xs for b in ys]`. Each element
        /// is `(pattern, iter)`. Empty for the single-for case.
        extra_clauses: Vec<(crate::ast::Pattern, Expr)>,
        /// Optional `if cond` filter at the end. Evaluated inside
        /// the innermost loop; if it returns false, that combination
        /// is skipped.
        filter: Option<Box<Expr>>,
        span: Span,
    },

    /// Mini-batch Cmp+ — Map comprehension `{k: v for x in xs}`.
    /// Analogous to ListComp but produces a `Map<K, V>` with
    /// separate expressions for key and value. Supports multiple
    /// `for` clauses (cartesian product) and an optional filter.
    /// Last-write-wins on duplicates (parallel to the `List.to_map`
    /// conversion).
    MapComp {
        /// Expression evaluated for the KEY of the pair.
        key: Box<Expr>,
        /// Expression evaluated for the VALUE of the pair.
        value: Box<Expr>,
        /// Binding of the first for.
        var: crate::ast::Pattern,
        /// Iterable of the first for.
        iter: Box<Expr>,
        /// Additional clauses (cartesian product).
        extra_clauses: Vec<(crate::ast::Pattern, Expr)>,
        /// Optional filter.
        filter: Option<Box<Expr>>,
        span: Span,
    },

    /// Map literal `{"k": v, ...}`, `{}`. Preserves insertion order.
    Map(Vec<(Expr, Expr)>, Span),

    /// Tuple literal (mini-batch T post-I). `(e1, e2, e3, ...)`.
    /// Special cases in the parser:
    ///   - `()` → empty tuple (unit).
    ///   - `(e,)` → 1-element tuple (trailing comma required).
    ///   - `(e)` → grouping parens only, NOT a tuple.
    ///   - `(e1, e2)` and beyond → tuple.
    ///
    /// Heterogeneous by nature: each slot has its own type.
    Tuple(Vec<Expr>, Span),

    /// Access to a tuple field by index: `t.0`, `t.1`. The parser
    /// detects `<expr>.<int_literal>` in postfix and emits this
    /// instead of `Expr::Field` (which requires an identifier).
    TupleField {
        tuple: Box<Expr>,
        index: usize,
        span: Span,
    },

    /// `loop { body }` as an expression (mini-batch L). The value of
    /// the expression is the `<v>` of the first `break <v>` that
    /// fires. `break` without a value → `Null`. Useful for retry
    /// loops and polling: `let result = loop { if cond { break value } }`.
    /// Distinct from `Stmt::Loop` (statement) — the evaluator
    /// discards the value in statement mode.
    Loop {
        body: Vec<Stmt>,
        /// Mini-batch L — optional label `'outer:` before `loop`.
        /// If present, `break 'outer` inside targets it
        /// specifically. Without a label, `break` matches this loop
        /// only if it is the nearest one.
        label: Option<String>,
        span: Span,
    },

    /// Range `start..end` (exclusive) or `start..=end` (inclusive).
    /// `span` points at `..` or `..=`. The `inclusive` flag was
    /// added by R.1.4 (mini-phase R) — default `false` keeps the
    /// existing call sites compatible.
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
        span: Span,
    },

    /// `if cond { then } else { else_ }`. Usable as an expression.
    If {
        condition: Box<Expr>,
        then: Vec<Stmt>,
        else_: Option<Vec<Stmt>>,
        span: Span,
    },

    /// `match value { pat => expr, ... }`.
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },

    /// Instantiation `User { id: 1, name: "x" }`.
    StructLit {
        type_name: String,
        fields: Vec<(String, Expr)>,
        span: Span,
    },

    /// Constructor `Ok(expr)`. Contextual keyword.
    Ok(Box<Expr>, Span),
    /// Constructor `Err(expr)`. Contextual keyword.
    Err(Box<Expr>, Span),
    /// Postfix operator `expr?`. `span` points at the `?`.
    Try(Box<Expr>, Span),

    /// Postfix operator `expr.await`. Introduced in Phase 6.1.
    /// `span` points at the `.` (parallel to `Field`).
    ///
    /// The `await` keyword is already tokenised by the lexer as
    /// `Token::Await`. Only legal inside an `async fn` — the
    /// validation lands in 6.2 (checker). In 6.1 the evaluator and
    /// codegen emit an explicit error pointing at the sub-step
    /// that will complete it.
    Await(Box<Expr>, Span),

    /// Marker for "an expression that could not be parsed here".
    /// Only produced by `parse_with_recovery` (Phase 9.0.1, F15);
    /// the strict `parse` API never emits it. The span points at
    /// the token where the problem was detected. The real errors
    /// land in the parallel `Vec<FitzError>` returned by
    /// `parse_with_recovery`; this node exists only so the AST
    /// keeps its structural shape (a fn body with a broken stmt is
    /// still a valid `Vec<Stmt>`, a call with a broken arg is still
    /// a `Call` with the expected number of args). Checker,
    /// evaluator and codegen treat this node as silenced: the
    /// checker emits no derived errors (synthesises `Type::Any`);
    /// evaluator and codegen abort with a defensive `FitzError`
    /// (no panic) because the strict CLI should never see them.
    ///
    /// `#[allow(dead_code)]` because in 9.0.1 it is only built
    /// from the parser (via `Stmt::Error`); `Expr::Error` at the
    /// sub-stmt level arrives with sub-expression recovery later.
    #[allow(dead_code)]
    Error(Span),
}

impl Expr {
    /// Returns the span of any variant. Parallel to `Stmt::span()`.
    #[allow(dead_code)]
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s) => *s,
            Expr::Float(_, s) => *s,
            Expr::Str(_, s) => *s,
            Expr::StrInterp(_, s) => *s,
            Expr::Bool(_, s) => *s,
            Expr::Null(s) => *s,
            Expr::Bytes(_, s) => *s,
            Expr::Ident(_, s) => *s,
            Expr::BinOp { span, .. } => *span,
            Expr::UnaryOp { span, .. } => *span,
            Expr::Call { span, .. } => *span,
            Expr::NamedArg { span, .. } => *span,
            Expr::FnExpr { span, .. } => *span,
            Expr::Field { span, .. } => *span,
            Expr::Index { span, .. } => *span,
            Expr::Slice { span, .. } => *span,
            Expr::Tuple(_, span) => *span,
            Expr::TupleField { span, .. } => *span,
            Expr::Loop { span, .. } => *span,
            Expr::List(_, s) => *s,
            Expr::ListComp { span, .. } => *span,
            Expr::MapComp { span, .. } => *span,
            Expr::Map(_, s) => *s,
            Expr::Range { span, .. } => *span,
            Expr::If { span, .. } => *span,
            Expr::Match { span, .. } => *span,
            Expr::StructLit { span, .. } => *span,
            Expr::Ok(_, s) => *s,
            Expr::Err(_, s) => *s,
            Expr::Try(_, s) => *s,
            Expr::Await(_, s) => *s,
            Expr::Error(s) => *s,
        }
    }

    /// V1 (2026-06-05) — mutable version of `span()`. Returns a
    /// mutable reference to the `Span` of any variant. Used by
    /// `shift_expr_spans` in `parser.rs` to fix up the spans of the
    /// Expr produced by the string-interpolation sub-parser (see
    /// V1 in `docs/deudas-post-5b.md`).
    pub fn span_mut(&mut self) -> &mut Span {
        match self {
            Expr::Int(_, s) => s,
            Expr::Float(_, s) => s,
            Expr::Str(_, s) => s,
            Expr::StrInterp(_, s) => s,
            Expr::Bool(_, s) => s,
            Expr::Null(s) => s,
            Expr::Bytes(_, s) => s,
            Expr::Ident(_, s) => s,
            Expr::BinOp { span, .. } => span,
            Expr::UnaryOp { span, .. } => span,
            Expr::Call { span, .. } => span,
            Expr::NamedArg { span, .. } => span,
            Expr::FnExpr { span, .. } => span,
            Expr::Field { span, .. } => span,
            Expr::Index { span, .. } => span,
            Expr::Slice { span, .. } => span,
            Expr::Tuple(_, span) => span,
            Expr::TupleField { span, .. } => span,
            Expr::Loop { span, .. } => span,
            Expr::List(_, s) => s,
            Expr::ListComp { span, .. } => span,
            Expr::MapComp { span, .. } => span,
            Expr::Map(_, s) => s,
            Expr::Range { span, .. } => span,
            Expr::If { span, .. } => span,
            Expr::Match { span, .. } => span,
            Expr::StructLit { span, .. } => span,
            Expr::Ok(_, s) => s,
            Expr::Err(_, s) => s,
            Expr::Try(_, s) => s,
            Expr::Await(_, s) => s,
            Expr::Error(s) => s,
        }
    }
}

/// Piece of a string with interpolation. E.g. `"Hola, {name}!"`
/// decomposes into
/// `[Lit("Hola, "), Expr(Ident("name"), None), Lit("!")]`.
///
/// Mini-batch Fm — the second field of `Expr` is the optional
/// `FormatSpec` extracted from the `:spec` after the expr in
/// `{x:.2f}`. `None` means "use the default Display format".
#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    /// Literal text.
    Lit(String),
    /// Interpolated expression with optional format.
    Expr(Expr, Option<FormatSpec>),
}

/// Mini-batch Fm — Format spec inspired by Python's
/// `{x:[fill]align[sign][#][0][width][grouping][.precision][type]}`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormatSpec {
    pub fill: Option<char>,
    pub align: Option<FormatAlign>,
    pub sign: Option<FormatSign>,
    pub alternate: bool,
    pub zero_pad: bool,
    pub width: Option<usize>,
    pub grouping: Option<char>,
    pub precision: Option<usize>,
    pub kind: Option<FormatKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatAlign {
    Left,
    Right,
    Center,
    Pad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatSign {
    Plus,
    Minus,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    Binary,
    Char,
    Decimal,
    ExponentLower,
    ExponentUpper,
    FixedLower,
    FixedUpper,
    GeneralLower,
    GeneralUpper,
    Octal,
    String,
    HexLower,
    HexUpper,
    Percent,
}

impl FormatSpec {
    /// Reconstructs the spec's source syntax:
    /// `[fill]align[sign][#][0][width][grouping][.prec][type]`. Used
    /// by `fitz fmt` to preserve the spec in the output.
    pub fn to_source(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        if let (Some(fill), Some(align)) = (self.fill, self.align) {
            out.push(fill);
            out.push(align.to_char());
        } else if let Some(align) = self.align {
            out.push(align.to_char());
        }
        if let Some(sign) = self.sign {
            out.push(sign.to_char());
        }
        if self.alternate {
            out.push('#');
        }
        if self.zero_pad {
            out.push('0');
        }
        if let Some(w) = self.width {
            let _ = write!(out, "{}", w);
        }
        if let Some(g) = self.grouping {
            out.push(g);
        }
        if let Some(p) = self.precision {
            let _ = write!(out, ".{}", p);
        }
        if let Some(k) = self.kind {
            out.push(k.to_char());
        }
        out
    }
}

impl FormatAlign {
    pub fn to_char(self) -> char {
        match self {
            FormatAlign::Left => '<',
            FormatAlign::Right => '>',
            FormatAlign::Center => '^',
            FormatAlign::Pad => '=',
        }
    }
}

impl FormatSign {
    pub fn to_char(self) -> char {
        match self {
            FormatSign::Plus => '+',
            FormatSign::Minus => '-',
            FormatSign::Space => ' ',
        }
    }
}

impl FormatKind {
    pub fn to_char(self) -> char {
        match self {
            FormatKind::Binary => 'b',
            FormatKind::Char => 'c',
            FormatKind::Decimal => 'd',
            FormatKind::ExponentLower => 'e',
            FormatKind::ExponentUpper => 'E',
            FormatKind::FixedLower => 'f',
            FormatKind::FixedUpper => 'F',
            FormatKind::GeneralLower => 'g',
            FormatKind::GeneralUpper => 'G',
            FormatKind::Octal => 'o',
            FormatKind::String => 's',
            FormatKind::HexLower => 'x',
            FormatKind::HexUpper => 'X',
            FormatKind::Percent => '%',
        }
    }
}

/// Assignment target: what is being assigned to.
///
/// Until 3.3 we only supported assignment to an identifier. In 3.4
/// we opened assignment to a field (`user.name = "x"`) to unlock
/// instance mutation.
#[derive(Debug, Clone, PartialEq)]
pub enum AssignTarget {
    /// `x = ...` — variable declaration or reassignment.
    ///
    /// V2 (2026-06-05) — the second field is the `Span` of the LHS
    /// Ident token (the variable name in `let name = ...`). The
    /// checker uses it to register the binding type in `TypeInfo`
    /// and enable hover over the variable name, not just over the
    /// RHS. Partially closes the S1 debt (`AssignTarget::Ident` /
    /// `Param` / `For.var` / `MatchArm.pattern` without their own
    /// span). `Span::PartialEq` is always-true so this does not
    /// break the AST structural tests.
    Ident(String, Span),
    /// `object.field = ...` — mutation of an `Instance` field.
    /// `object` is any expression that evaluates to a
    /// `Value::Instance`; the evaluator checks this at runtime and
    /// emits an error if not.
    Field { object: Box<Expr>, field: String },
    /// `object[index] = ...` — assignment to a `List` or `Map`
    /// index (R.1.3, mini-phase R). For `List`, the index must be
    /// an `Int` in range; out-of-bounds → runtime error. For `Map`,
    /// the key can be any hashable type; if it already exists it is
    /// overwritten, otherwise it is inserted preserving insertion
    /// order.
    Index { object: Box<Expr>, index: Box<Expr> },
}

/// Position of an AST node in the source file. Attached to every
/// `Stmt` since B.1 — it enriches checker/evaluator error messages
/// with real line and column (before that they always showed up as
/// `0:0`).
///
/// For synthetic nodes (built by the parser without a concrete
/// token, e.g. the arrow body of `fn f(x) => x * 2` rewritten into
/// `Return`), or for test nodes, we use `Span::default()`
/// (= `Span::ZERO`, both fields at 0). The site reporting the error
/// checks `is_known()` before quoting the position.
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

    /// `true` if the span points at a real position (was not filled
    /// in via `default()`). Used by error formatters to decide
    /// whether to quote line/column or just the message. Kept as a
    /// public API for future use in `FitzError::Display`.
    #[allow(dead_code)]
    pub fn is_known(&self) -> bool {
        self.line != 0 || self.column != 0
    }
}

/// `Span` compares as **always equal** to itself. Reason: the AST /
/// parser / evaluator tests use `assert_eq!` on `Stmt` and `Expr`
/// building literal nodes with `Span::ZERO`, against nodes produced
/// by the parser with real spans. If the comparison were structural
/// over `line`/`column`, ~30 tests would have to duplicate the
/// parser logic to predict positions — with no real value (the
/// tests look at structure, not position). When the position needs
/// to be validated, we compare `span.line` and `span.column`
/// explicitly (see the dedicated span tests in parser and checker).
impl PartialEq for Span {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}
impl Eq for Span {}

/// A statement: runs an effect, optionally produces a value.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// Assignment / declaration. E.g. `x = 42`, `name: Str = "Fitz"`,
    /// `user.name = "Otro"`. In Fitz we do not differentiate `let x
    /// = ...` from `x = ...` at the AST level. The `type_`
    /// annotation is only valid when `target` is `Ident` (assigning
    /// to a field does not allow re-annotating the type); the
    /// parser enforces that.
    Assign {
        target: AssignTarget,
        type_: Option<TypeExpr>,
        value: Expr,
        span: Span,
    },

    /// `let (a, b) = expr` — tuple destructuring (mini-batch T).
    /// Only applies with `let` (declaration). The evaluator declares
    /// `a` and `b` in the current env; every name can be `_`
    /// (ignore). Pattern supports nesting:
    /// `let ((x, y), z) = ...`.
    Destructure {
        pattern: Pattern,
        value: Expr,
        span: Span,
    },

    /// `return expr`.
    Return(Expr, Span),

    /// `return <status> <body?>` — return with a custom HTTP status
    /// code. Only valid inside HTTP handlers (fn with a decorator
    /// `@get`/`@post`/`@put`/`@delete`); the checker rejects the
    /// rest. `body` is optional for cases like `return 204` (No
    /// Content). The span points at the `return` keyword.
    ///
    /// Spec syntax: `return 401 { message: "no autorizado" }`,
    /// `return 204`, `return 200 user`.
    ReturnStatus {
        status: Expr,
        body: Option<Expr>,
        span: Span,
    },

    /// An expression used as a statement (typically a call).
    Expr(Expr, Span),

    /// Function definition. Supports the block form and the arrow
    /// form (`fn f(n) => n * 2`) — the parser rewrites the arrow
    /// into `body: vec![Stmt::Return(Expr, Span::ZERO)]`.
    ///
    /// `decorators` lists the `@deco(args...)` wrapping the
    /// function, in the order they appear in the source. The
    /// parser only accumulates them; the semantics (what each
    /// decorator does) is the evaluator's job, dispatching on
    /// `Decorator.name`. A function without decorators has an
    /// empty vector.
    FnDef {
        name: String,
        params: Vec<Param>,
        return_type: Option<TypeExpr>,
        body: Vec<Stmt>,
        is_async: bool,
        decorators: Vec<Decorator>,
        span: Span,
    },

    /// Definition of a custom type: `type User { id: Int, name: Str }`.
    /// R.3 (mini-phase R) adds `methods: Vec<MethodDef>` — custom
    /// methods on the type. Fields and methods mix in any order
    /// inside the `type`'s `{}`.
    ///
    /// Phase 10.3.a adds `decorators` on the type (`@table("users")`
    /// for the ORM) and `decorators` per field (`@primary`,
    /// `@column(...)`, `@unique`, `@index`). Decorators are always
    /// parsed — the checker decides which are valid by context.
    /// Programs without ORM use no decorators and the field stays
    /// empty.
    TypeDef {
        name: String,
        decorators: Vec<Decorator>,
        fields: Vec<Field>,
        methods: Vec<MethodDef>,
        span: Span,
    },

    /// `break [label] [<expr>]` inside loop/while/for.
    /// Mini-batch L:
    ///   - `value` optional for `loop` as an expression
    ///     (`break v`).
    ///   - `label` optional to target a specific nested loop
    ///     (`break 'outer`). Without a label → matches the nearest
    ///     loop.
    Break(Option<Expr>, Option<String>, Span),

    /// `continue [label]` inside loop/while/for. Without a label →
    /// continues the nearest loop.
    Continue(Option<String>, Span),

    /// `while cond { body }`. Iterates while `cond` evaluates to
    /// `Bool(true)`. `break` exits the loop; `continue` jumps to
    /// the next iteration. `label` (mini-batch L) optional for
    /// `break 'outer`.
    While {
        condition: Expr,
        body: Vec<Stmt>,
        label: Option<String>,
        span: Span,
    },

    /// `loop { body }` — infinite loop. Exits only with `break` (or
    /// `return`). `label` (mini-batch L) optional for `break
    /// 'outer`.
    Loop {
        body: Vec<Stmt>,
        label: Option<String>,
        span: Span,
    },

    /// `for var in iter { body }`. `iter` is evaluated once on
    /// entry and must be iterable (List, Range, or Map). `var` is a
    /// `Pattern` matched against every element of the iter on each
    /// iteration. Mini-batch Md: the Pattern unlocks
    /// `for (k, v) in m` over Map (Pattern::Tuple), `for _ in 0..10`
    /// (Pattern::Wildcard), in addition to the classic `for x in
    /// xs` (Pattern::Ident). `break`/`continue` behave like in
    /// `while`. `label` (mini-batch L) optional for `break 'outer`.
    For {
        var: Pattern,
        iter: Expr,
        body: Vec<Stmt>,
        label: Option<String>,
        span: Span,
    },

    /// `import foo` or `import foo.bar.baz` — loads a module from
    /// disk and exposes it in the current scope as a `Value::Module`
    /// under the LAST segment of the path (`import foo.bar` →
    /// binding `bar`). Resolution: relative to the importer's file,
    /// `foo.bar` → `./foo/bar.fitz`.
    ///
    /// To access something inside: `bar.fn(...)`. To bring names
    /// directly into the scope, use `Stmt::FromImport`.
    Import {
        /// Path segments in order. Always has at least one element.
        path: Vec<String>,
        /// PreF8.4: `import foo as f` — the namespace is bound as
        /// `f` instead of the last path segment. `None` if no
        /// alias.
        alias: Option<String>,
        span: Span,
    },

    /// `from foo import a, b, c` or `from foo.bar import x` — loads
    /// the module and binds every listed name in the current scope.
    /// The module is not exposed as such.
    ///
    /// The parser guarantees `names` is non-empty.
    ///
    /// PreF8.4: every `names` entry is `(original_name,
    /// optional_alias)`. `from foo import bar as b` →
    /// `[("bar", Some("b"))]`. Without an alias, the second
    /// component is `None` and the binding uses the original name.
    FromImport {
        path: Vec<String>,
        names: Vec<(String, Option<String>)>,
        span: Span,
    },

    /// Marker for "a statement that could not be parsed here".
    /// Parallel to `Expr::Error`: only produced by
    /// `parse_with_recovery` (Phase 9.0.1, F15). The span points at
    /// the token where the problem was detected. The error details
    /// live in the parallel `Vec<FitzError>` returned by
    /// `parse_with_recovery`. This variant keeps the shape of the
    /// `Program` / block body when there are recovered errors.
    Error(Span),
}

impl Stmt {
    /// Returns the span of any variant. Helper for the sites that
    /// report errors on a stmt without matching by variant. Kept as
    /// a public API for tests and future use.
    #[allow(dead_code)]
    pub fn span(&self) -> Span {
        match self {
            Stmt::Assign { span, .. } => *span,
            Stmt::Destructure { span, .. } => *span,
            Stmt::Return(_, span) => *span,
            Stmt::ReturnStatus { span, .. } => *span,
            Stmt::Expr(_, span) => *span,
            Stmt::FnDef { span, .. } => *span,
            Stmt::TypeDef { span, .. } => *span,
            Stmt::Break(_, _, span) => *span,
            Stmt::Continue(_, span) => *span,
            Stmt::While { span, .. } => *span,
            Stmt::Loop { span, .. } => *span,
            Stmt::For { span, .. } => *span,
            Stmt::Import { span, .. } => *span,
            Stmt::FromImport { span, .. } => *span,
            Stmt::Error(span) => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOpKind {
    Add,
    Sub,
    Mul,
    Div,
    /// R.1.2 — modulo operator (`%`). Only valid on `Int` in MVP.
    /// Euclidean semantics: the result always has the same sign as
    /// the divisor (parallel to Python, different from Rust's `%`
    /// which is truncate-toward-zero). `n % 0` → clear runtime
    /// error.
    Mod,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    /// Mini-batch Xor — logical `a xor b` (parallel to `and`/`or`).
    /// Only valid on `Bool`. Equivalent to `a != b` on Bool but
    /// more declarative.
    Xor,
    /// Mini-batch Bits — bitwise operators on `Int`. The checker
    /// rejects any other type. Shifts `<<`/`>>` with negative RHS
    /// or >= 64 → runtime error.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOpKind {
    /// Numeric negation: `-x`.
    Neg,
    /// Logical negation: `not x` (R.1.1, mini-phase R). Only valid
    /// on `Bool`; the checker rejects any other type.
    Not,
    /// Mini-batch Bits — bitwise NOT `~x`. Only `Int`.
    BitNot,
}

/// Formal parameter of a function. The type is optional (gradual
/// typing). `default` (mini-batch Fp): expression to use when the
/// caller does not provide this arg. If a param has a default, every
/// later one must too (Python rule). The parser and the checker
/// validate this.
///
/// `varargs` (mini-batch Fp.2): if `true`, the param is variadic
/// (`fn sum(...xs: Int)`). Collects every extra call arg into a
/// `List<T>`. Only the LAST param can be varargs; mutually exclusive
/// with `default` (a varargs cannot have a default). The parser
/// validates this.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub type_: Option<TypeExpr>,
    pub default: Option<Expr>,
    pub varargs: bool,
    /// S1 (2026-06-05) — span of the Ident token of the param name.
    /// `Span::ZERO` for synthetic params (tests / hand-built nodes
    /// with no position). The checker consumes it to register the
    /// binding's type under this span in `TypeInfo` and enable
    /// hover over the param name inside a fn signature.
    ///
    /// `Span::PartialEq` is always-true, so this does not affect
    /// the AST structural tests that compare `Param`s.
    pub name_span: Span,
}

/// Field of a `type`. The type is mandatory inside a struct.
/// Nullability (`T?`) is modelled inside `TypeExpr` as
/// `TypeExpr::Nullable(...)`, not as a separate flag.
///
/// Phase 10.3.a — `decorators` enables `@primary`,
/// `@column(name=...)`, `@unique`, `@index` per field. For non-ORM
/// types it stays empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub type_: TypeExpr,
    pub default: Option<Expr>,
    pub decorators: Vec<Decorator>,
}

/// Custom method inside a `type` (R.3, mini-phase R).
///
/// "Option A" design — the type's **fields** are visible as local
/// variables in the method body (without a `self` prefix). Closer
/// to Python/Ruby/Crystal than to Rust. Trade-off: if the method
/// declares a local with the same name as a field, the local wins
/// (documented as a caveat).
///
/// MVP:
///  - No decorators (`@get`/`@server`/etc. are for top-level fns
///    with HTTP dispatch; methods do not fit).
///  - Visibility: all public in MVP. `pub fn` is left as debt.
///  - No operator overloading.
///
/// **Mini-batch St**: `is_static` distinguishes static methods
/// (`static fn make() -> X` invoked as `X.make()`) from instance
/// methods (`fn greet()` invoked as `instance.greet()`). Static
/// methods do not receive the fields as locals — they are
/// constructors / factories / type utilities.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodDef {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub body: Vec<Stmt>,
    pub is_async: bool,
    /// Mini-batch St — `true` if the method is static
    /// (`static fn ...` declared in the `type` body).
    pub is_static: bool,
    pub span: Span,
}

/// A type expression in an annotation. The AST the parser produces
/// when it sees something like `Int`, `List<Int>`, `Map<Str, User>`,
/// `Result<List<User>>`, `User?`. No resolution yet: `Named(...)`
/// may refer to a built-in (`Int`, `Str`, ...), a user-declared
/// type, or an imported one. The checker (Phase 5.2) validates that
/// each name exists and that the generic arities are correct.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// Simple name: `Int`, `Str`, `User`.
    Named(String),
    /// Applied generic: `List<Int>`, `Map<Str, User>`, `Result<T>`.
    /// `args` always has at least one element (`Foo<>` is a parser
    /// error).
    Generic { name: String, args: Vec<TypeExpr> },
    /// `?` suffix: the value can be that type or `Null`. `User?` →
    /// `Nullable(Box(Named("User")))`. `List<Int>?` →
    /// `Nullable(Box(Generic { name: "List", args: [Named("Int")] }))`.
    Nullable(Box<TypeExpr>),
    /// Function type: `Fn(T1, T2) -> U`. Models a value that can be
    /// invoked with the given parameters and return. The `Fn`
    /// keyword is not a nominal type of the language: the parser
    /// recognises it in a dedicated path when it sees `Fn` followed
    /// by `(`. `Fn() -> U` is valid (zero params). The return is
    /// mandatory in the syntax to avoid ambiguity with `Fn(...)` as
    /// an expression.
    Function {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
    },
    /// Tuple type `(T1, T2, ...)` (mini-batch T). Heterogeneous by
    /// definition. Empty Vec = the "unit" type `()` (empty tuple).
    Tuple(Vec<TypeExpr>),
}

impl TypeExpr {
    /// Shortcut for the most common call sites (tests, builtins).
    /// Kept as a public API for future tests.
    #[allow(dead_code)]
    pub fn named(s: impl Into<String>) -> Self {
        TypeExpr::Named(s.into())
    }

    /// Reproduces the form written in source. Used in error messages
    /// and in the HTTP runtime to show the declared type.
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
            TypeExpr::Tuple(items) => {
                let parts: Vec<String> = items.iter().map(|t| t.display_name()).collect();
                if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                }
            }
        }
    }

    /// "Head" name of the type, ignoring nullables and generic
    /// arguments. Useful for the HTTP runtime when it needs to
    /// resolve a declared type name in the importer's env.
    /// `User?` → `"User"`, `List<Int>` → `"List"`,
    /// `Result<List<User>>` → `"Result"`.
    pub fn head_name(&self) -> &str {
        match self {
            TypeExpr::Named(name) => name,
            TypeExpr::Generic { name, .. } => name,
            TypeExpr::Nullable(inner) => inner.head_name(),
            TypeExpr::Function { .. } => "Fn",
            TypeExpr::Tuple(_) => "Tuple",
        }
    }

    /// `true` if the type is `T?` (accepts `Null` in addition to the
    /// base type).
    pub fn is_nullable(&self) -> bool {
        matches!(self, TypeExpr::Nullable(_))
    }
}

impl std::fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.display_name())
    }
}

/// Arm of a `match`: pattern → expression.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    /// Optional guard `if <cond>` after the pattern (R.2.2). The
    /// arm matches if the pattern matches AND the guard evaluates
    /// to `true`. Arms with a guard do NOT count for Result
    /// exhaustiveness (parallel to Rust) — the checker requires an
    /// explicit catch-all.
    pub guard: Option<Expr>,
    /// Arm body. Sp.2 (post-Fp+Sp) — `Vec<Stmt>` instead of `Expr`,
    /// parallel to `Expr::If.then`. The arm's "value" is the last
    /// `Stmt::Expr` (if any) or `Null`. `return`/`break`/`continue`
    /// inside an arm propagate to the containing fn/loop like any
    /// other statement.
    ///
    /// Parser syntax:
    ///   - `pat => expr` desugars to `vec![Stmt::Expr(expr)]` (1 elem).
    ///   - `pat => { stmts }` desugars to the block's stmt list.
    ///   - `pat => return X` / `break` / `continue` desugars to
    ///     `Stmt::Return(X)` / `Stmt::Break` / `Stmt::Continue`
    ///     directly.
    pub body: Vec<Stmt>,
}

/// Patterns for `match`.
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// `42` — matches if the value is that exact int. Same for
    /// float/str/bool.
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// `null` — matches if the value is Null.
    Null,
    /// `name` — always matches, binds the value to that name.
    ///
    /// S1 (2026-06-05) — the `Span` is the pattern's Ident token,
    /// used by the checker to register the binding type in
    /// `TypeInfo` (hover over `i` in `for i in 0..10`, `n` in
    /// `match x { Ok(n) => n }`, etc.). `Span::PartialEq` is
    /// always-true so this does not affect structural tests.
    Ident(String, Span),
    /// `_` — always matches, no binding.
    Wildcard,
    /// `Ok(x)` — matches any `Result::Ok(...)` and binds the inner
    /// as `x`. S1 (2026-06-05) — `Span` of the Ident token inside
    /// the parens.
    OkBinding(String, Span),
    /// `Err(e)` — matches any `Result::Err(...)` and binds the
    /// inner as `e`. S1 (2026-06-05) — `Span` of the Ident token
    /// inside the parens.
    ErrBinding(String, Span),
    /// `Ok(_)` — matches any `Result::Ok(...)` without binding
    /// (does not pollute the scope with a var named `_`).
    OkWildcard,
    /// `Err(_)` — matches any `Result::Err(...)` without binding.
    ErrWildcard,
    /// `start..end` (exclusive) or `start..=end` (inclusive) —
    /// matches if the value is Int and `start <= v < end` (or `<=
    /// end`). Int-only for now (Float complicates the discrete
    /// representation). `inclusive` was added by R.1.4 (mini-phase
    /// R).
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
    },
    /// `(p1, p2, ...)` — matches if the value is a tuple of the
    /// same length and every sub-pattern matches its slot
    /// (mini-batch T). The empty tuple `()` matches only
    /// `Tuple([])`. Each sub-pattern can be any `Pattern`
    /// (including another `Tuple` for nested destructuring).
    Tuple(Vec<Pattern>),
    /// `pat1 | pat2 | pat3` — matches if ANY of the sub-patterns
    /// matches. R.2.1 (mini-phase R).
    ///
    /// MVP restrictions (parallel to Rust):
    ///  - **No bindings** inside or-patterns. Neither `Ident(x)`,
    ///    nor `Ok(x)`, nor `Err(e)` — the parser rejects them with
    ///    a clear error. `Pattern::Wildcard`, `OkWildcard` and
    ///    `ErrWildcard` are allowed.
    ///  - Guarantee: the list has 2+ elements; a single `pat`
    ///    stays as a plain pattern, not wrapped in `Or`.
    Or(Vec<Pattern>),
}

/// Decorator applied to a `Stmt::FnDef`: `@name(args..., key=value...)`.
///
/// In 4.1 the parser only accumulates decorators; the evaluator
/// dispatches by name (`@get`/`@post`/`@put`/`@delete` register HTTP
/// routes when 4.2 lands, `@server` configures the runtime, any
/// other → explicit "unknown decorator" error). Args are arbitrary
/// expressions, validated at runtime by the specific decorator
/// (e.g. `@get` requires a single `Str` with the path).
///
/// In 7.0 we added `kwargs` to support `@deco(pos1, key=value)`.
/// Kwargs go **after** the positionals; the parser rejects the
/// reverse order. Each decorator decides whether it accepts them —
/// today (while 7.4 has not landed)
/// `@get/@post/@put/@delete/@server` emit an error on kwargs. That
/// changes once 7.4 closes and `@server` accepts `docs: Bool`.
#[derive(Debug, Clone, PartialEq)]
pub struct Decorator {
    pub name: String,
    pub args: Vec<Expr>,
    pub kwargs: Vec<(String, Expr)>,
}

/// A Fitz program is a list of statements.
pub type Program = Vec<Stmt>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds by hand the AST equivalent to the program:
    ///
    /// ```fitz
    /// name = "Fitz"
    /// x = 10 + 5
    /// print("Hola, {name}!")
    /// fn double(n) => n * 2
    /// print(double(x))
    /// ```
    ///
    /// Acts as proof that the AST can represent the Phase 2 success
    /// criterion, and as a reference for what the parser has to
    /// produce once it is implemented.
    #[test]
    fn can_represent_phase2_success_program() {
        let program: Program = vec![
            // name = "Fitz"
            Stmt::Assign {
                target: AssignTarget::Ident("name".into(), Span::default()),
                type_: None,
                value: Expr::Str("Fitz".into(), Span::ZERO),
                span: Span::ZERO,
            },
            // x = 10 + 5
            Stmt::Assign {
                target: AssignTarget::Ident("x".into(), Span::default()),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(10, Span::ZERO)),
                    right: Box::new(Expr::Int(5, Span::ZERO)),
                    span: Span::ZERO,
                },
                span: Span::ZERO,
            },
            // print("Hola, {name}!")
            Stmt::Expr(
                Expr::Call {
                    callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                    args: vec![Expr::StrInterp(
                        vec![
                            StrPart::Lit("Hola, ".into()),
                            StrPart::Expr(Expr::Ident("name".into(), Span::ZERO), None),
                            StrPart::Lit("!".into()),
                        ],
                        Span::ZERO,
                    )],
                    span: Span::ZERO,
                },
                Span::ZERO,
            ),
            // fn double(n) => n * 2
            Stmt::FnDef {
                name: "double".into(),
                params: vec![Param {
                    name: "n".into(),
                    type_: None,
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                }],
                return_type: None,
                body: vec![Stmt::Return(
                    Expr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(2, Span::ZERO)),
                        span: Span::ZERO,
                    },
                    Span::ZERO,
                )],
                is_async: false,
                decorators: vec![],
                span: Span::ZERO,
            },
            // print(double(x))
            Stmt::Expr(
                Expr::Call {
                    callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                    args: vec![Expr::Call {
                        callee: Box::new(Expr::Ident("double".into(), Span::ZERO)),
                        args: vec![Expr::Ident("x".into(), Span::ZERO)],
                        span: Span::ZERO,
                    }],
                    span: Span::ZERO,
                },
                Span::ZERO,
            ),
        ];

        assert_eq!(program.len(), 5);

        // Spot check: the 4th statement is the fn def of `double`.
        match &program[3] {
            Stmt::FnDef {
                name, params, body, ..
            } => {
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
            StrPart::Expr(Expr::Ident("age".into(), Span::ZERO), None),
        ];
        assert_eq!(parts[0], StrPart::Lit("Edad: ".into()));
        assert!(matches!(parts[1], StrPart::Expr(Expr::Ident(_, _), _)));
    }

    #[test]
    fn ast_supports_break_and_continue_inside_loops() {
        // Stmt::Break and Stmt::Continue are statements in their
        // own right.
        let stmts: Vec<Stmt> = vec![
            Stmt::Break(None, None, Span::ZERO),
            Stmt::Continue(None, Span::ZERO),
        ];
        assert_eq!(stmts[0], Stmt::Break(None, None, Span::ZERO));
        assert_eq!(stmts[1], Stmt::Continue(None, Span::ZERO));
    }

    #[test]
    fn list_literal_holds_arbitrary_exprs() {
        // `[1, x, 2 + 3]`
        let list = Expr::List(
            vec![
                Expr::Int(1, Span::ZERO),
                Expr::Ident("x".into(), Span::ZERO),
                Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(2, Span::ZERO)),
                    right: Box::new(Expr::Int(3, Span::ZERO)),
                    span: Span::ZERO,
                },
            ],
            Span::ZERO,
        );
        match list {
            Expr::List(items, _) => assert_eq!(items.len(), 3),
            _ => panic!("se esperaba List"),
        }
    }

    #[test]
    fn map_literal_preserva_orden_de_pares() {
        // `{"a": 1, "b": 2}`
        let map = Expr::Map(
            vec![
                (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
                (Expr::Str("b".into(), Span::ZERO), Expr::Int(2, Span::ZERO)),
            ],
            Span::ZERO,
        );
        match map {
            Expr::Map(pairs, _) => {
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0].0, Expr::Str("a".into(), Span::ZERO));
                assert_eq!(pairs[1].1, Expr::Int(2, Span::ZERO));
            }
            _ => panic!("se esperaba Map"),
        }
    }

    #[test]
    fn range_expr_envuelve_extremos() {
        // `0..10`
        let r = Expr::Range {
            start: Box::new(Expr::Int(0, Span::ZERO)),
            end: Box::new(Expr::Int(10, Span::ZERO)),
            inclusive: false,
            span: Span::ZERO,
        };
        match r {
            Expr::Range { start, end, .. } => {
                assert_eq!(*start, Expr::Int(0, Span::ZERO));
                assert_eq!(*end, Expr::Int(10, Span::ZERO));
            }
            _ => panic!("se esperaba Range"),
        }
    }

    #[test]
    fn index_expr_envuelve_objeto_e_indice() {
        // `xs[0]`
        let ix = Expr::Index {
            object: Box::new(Expr::Ident("xs".into(), Span::ZERO)),
            index: Box::new(Expr::Int(0, Span::ZERO)),
            span: Span::ZERO,
        };
        match ix {
            Expr::Index { object, index, .. } => {
                assert_eq!(*object, Expr::Ident("xs".into(), Span::ZERO));
                assert_eq!(*index, Expr::Int(0, Span::ZERO));
            }
            _ => panic!("se esperaba Index"),
        }
    }

    #[test]
    fn for_stmt_envuelve_var_iter_y_body() {
        // `for x in xs { print(x) }`
        let f = Stmt::For {
            var: Pattern::Ident("x".into(), Span::default()),
            iter: Expr::Ident("xs".into(), Span::ZERO),
            body: vec![Stmt::Expr(
                Expr::Call {
                    callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                    args: vec![Expr::Ident("x".into(), Span::ZERO)],
                    span: Span::ZERO,
                },
                Span::ZERO,
            )],
            label: None,
            span: Span::ZERO,
        };
        match f {
            Stmt::For {
                var, iter, body, ..
            } => {
                assert_eq!(var, Pattern::Ident("x".into(), Span::default()));
                assert_eq!(iter, Expr::Ident("xs".into(), Span::ZERO));
                assert_eq!(body.len(), 1);
            }
            _ => panic!("se esperaba For"),
        }
    }

    #[test]
    fn pattern_range_guarda_extremos_como_int() {
        // `match n { 0..10 => "chico", _ => "grande" }` — the pattern only.
        let p = Pattern::Range {
            start: 0,
            end: 10,
            inclusive: false,
        };
        match p {
            Pattern::Range {
                start,
                end,
                inclusive,
            } => {
                assert_eq!(start, 0);
                assert_eq!(end, 10);
                assert!(!inclusive);
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
                ("id".into(), Expr::Int(1, Span::ZERO)),
                ("name".into(), Expr::Str("x".into(), Span::ZERO)),
            ],
            span: Span::ZERO,
        };
        match lit {
            Expr::StructLit {
                type_name, fields, ..
            } => {
                assert_eq!(type_name, "User");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0, "id");
                assert_eq!(fields[0].1, Expr::Int(1, Span::ZERO));
                assert_eq!(fields[1].0, "name");
                assert_eq!(fields[1].1, Expr::Str("x".into(), Span::ZERO));
            }
            _ => panic!("se esperaba StructLit"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Result (Phase 3, step 3: Result + Ok/Err + `?`)
    // -----------------------------------------------------------------------

    #[test]
    fn ok_ctor_envuelve_inner() {
        // `Ok(42)` → Expr::Ok(Box(Int(42)), Span::ZERO)
        let e = Expr::Ok(Box::new(Expr::Int(42, Span::ZERO)), Span::ZERO);
        match e {
            Expr::Ok(inner, _) => assert_eq!(*inner, Expr::Int(42, Span::ZERO)),
            _ => panic!("se esperaba Ok"),
        }
    }

    #[test]
    fn err_ctor_envuelve_inner() {
        // `Err("boom")` → Expr::Err(Box(Str("boom")), Span::ZERO)
        let e = Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO);
        match e {
            Expr::Err(inner, _) => assert_eq!(*inner, Expr::Str("boom".into(), Span::ZERO)),
            _ => panic!("se esperaba Err"),
        }
    }

    #[test]
    fn try_expr_envuelve_operando() {
        // `x?` → Expr::Try(Box(Ident("x")), Span::ZERO)
        let e = Expr::Try(Box::new(Expr::Ident("x".into(), Span::ZERO)), Span::ZERO);
        match e {
            Expr::Try(inner, _) => assert_eq!(*inner, Expr::Ident("x".into(), Span::ZERO)),
            _ => panic!("se esperaba Try"),
        }
    }

    #[test]
    fn try_y_ctors_son_componibles() {
        // `Ok(get(id)?)` — a `?` inside an `Ok` constructor.
        let e = Expr::Ok(
            Box::new(Expr::Try(
                Box::new(Expr::Call {
                    callee: Box::new(Expr::Ident("get".into(), Span::ZERO)),
                    args: vec![Expr::Ident("id".into(), Span::ZERO)],
                    span: Span::ZERO,
                }),
                Span::ZERO,
            )),
            Span::ZERO,
        );
        if let Expr::Ok(inner, _) = e {
            assert!(matches!(*inner, Expr::Try(_, _)));
        } else {
            panic!("se esperaba Ok");
        }
    }

    #[test]
    fn unary_op_negation_wraps_operand() {
        // -x → UnaryOp { op: Neg, operand: Ident("x") }
        let expr = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Ident("x".into(), Span::ZERO)),
            span: Span::ZERO,
        };
        match expr {
            Expr::UnaryOp { op, operand, .. } => {
                assert_eq!(op, UnaryOpKind::Neg);
                assert_eq!(*operand, Expr::Ident("x".into(), Span::ZERO));
            }
            _ => panic!("se esperaba UnaryOp"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Phase 3, step 4 (anonymous functions + method calls + mutation)
    // -----------------------------------------------------------------------

    #[test]
    fn call_admite_callee_como_expresion() {
        // `xs.map(f)` → Call with callee = Field { object: xs, field: "map" }.
        let call = Expr::Call {
            callee: Box::new(Expr::Field {
                object: Box::new(Expr::Ident("xs".into(), Span::ZERO)),
                field: "map".into(),
                span: Span::ZERO,
            }),
            args: vec![Expr::Ident("f".into(), Span::ZERO)],
            span: Span::ZERO,
        };
        match call {
            Expr::Call { callee, args, .. } => {
                assert!(matches!(*callee, Expr::Field { .. }));
                assert_eq!(args.len(), 1);
            }
            _ => panic!("se esperaba Call"),
        }
    }

    #[test]
    fn fn_expr_envuelve_params_y_body() {
        // `fn(x) => x * 2` — nameless version.
        let fnexpr = Expr::FnExpr {
            params: vec![Param {
                name: "x".into(),
                type_: None,
                default: None,
                varargs: false,
                name_span: Span::default(),
            }],
            body: vec![Stmt::Return(
                Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(2, Span::ZERO)),
                    span: Span::ZERO,
                },
                Span::ZERO,
            )],
            is_async: false,
            span: Span::ZERO,
        };
        match fnexpr {
            Expr::FnExpr { params, body, .. } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "x");
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0], Stmt::Return(_, _)));
            }
            _ => panic!("se esperaba FnExpr"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Phase 3, step 5 (modules / import)
    // -----------------------------------------------------------------------

    #[test]
    fn import_simple_guarda_path_de_un_segmento() {
        // `import utils` → Stmt::Import { path: ["utils"], alias: None }
        let s = Stmt::Import {
            path: vec!["utils".into()],
            alias: None,
            span: Span::ZERO,
        };
        match s {
            Stmt::Import { path, alias, .. } => {
                assert_eq!(path, vec!["utils".to_string()]);
                assert!(alias.is_none());
            }
            _ => panic!("se esperaba Import"),
        }
    }

    #[test]
    fn import_punteado_guarda_segmentos_en_orden() {
        // `import sub.foo` → Stmt::Import { path: ["sub", "foo"], alias: None }
        let s = Stmt::Import {
            path: vec!["sub".into(), "foo".into()],
            alias: None,
            span: Span::ZERO,
        };
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
        // `from utils import slugify, parse` — no aliases.
        let s = Stmt::FromImport {
            path: vec!["utils".into()],
            names: vec![("slugify".into(), None), ("parse".into(), None)],
            span: Span::ZERO,
        };
        match s {
            Stmt::FromImport { path, names, .. } => {
                assert_eq!(path, vec!["utils".to_string()]);
                assert_eq!(names.len(), 2);
                assert_eq!(names[0].0, "slugify");
                assert!(names[0].1.is_none());
                assert_eq!(names[1].0, "parse");
            }
            _ => panic!("se esperaba FromImport"),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Phase 4, step 4.1 (generic decorators over FnDef)
    // -----------------------------------------------------------------------

    #[test]
    fn decorator_guarda_nombre_y_args() {
        // `@get("/users/{id}")` — a decorator with a string arg.
        let d = Decorator {
            name: "get".into(),
            args: vec![Expr::Str("/users/{id}".into(), Span::ZERO)],
            kwargs: vec![],
        };
        assert_eq!(d.name, "get");
        assert_eq!(d.args.len(), 1);
        assert_eq!(d.args[0], Expr::Str("/users/{id}".into(), Span::ZERO));
        assert!(d.kwargs.is_empty());
    }

    #[test]
    fn fndef_default_sin_decorators_vector_vacio() {
        // A fn declared without `@...` on top should have an empty
        // `decorators` vector.
        let f = Stmt::FnDef {
            name: "f".into(),
            params: vec![],
            return_type: None,
            body: vec![],
            is_async: false,
            decorators: vec![],
            span: Span::ZERO,
        };
        if let Stmt::FnDef { decorators, .. } = f {
            assert!(decorators.is_empty());
        } else {
            panic!("se esperaba FnDef");
        }
    }

    #[test]
    fn fndef_admite_varios_decorators_en_orden() {
        // `@get("/x") @auth("admin") fn h() {}` — two stacked decorators.
        let f = Stmt::FnDef {
            name: "h".into(),
            params: vec![],
            return_type: None,
            body: vec![],
            is_async: false,
            decorators: vec![
                Decorator {
                    name: "get".into(),
                    args: vec![Expr::Str("/x".into(), Span::ZERO)],
                    kwargs: vec![],
                },
                Decorator {
                    name: "auth".into(),
                    args: vec![Expr::Str("admin".into(), Span::ZERO)],
                    kwargs: vec![],
                },
            ],
            span: Span::ZERO,
        };
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
        // `x = 1` — Ident target.
        let s1 = Stmt::Assign {
            target: AssignTarget::Ident("x".into(), Span::default()),
            type_: None,
            value: Expr::Int(1, Span::ZERO),
            span: Span::ZERO,
        };
        if let Stmt::Assign { target, .. } = s1 {
            assert_eq!(target, AssignTarget::Ident("x".into(), Span::default()));
        } else {
            panic!("se esperaba Assign");
        }

        // `user.name = "x"` — Field target.
        let s2 = Stmt::Assign {
            target: AssignTarget::Field {
                object: Box::new(Expr::Ident("user".into(), Span::ZERO)),
                field: "name".into(),
            },
            type_: None,
            value: Expr::Str("x".into(), Span::ZERO),
            span: Span::ZERO,
        };
        if let Stmt::Assign {
            target: AssignTarget::Field { object, field },
            ..
        } = s2
        {
            assert_eq!(*object, Expr::Ident("user".into(), Span::ZERO));
            assert_eq!(field, "name");
        } else {
            panic!("se esperaba Assign con target Field");
        }
    }

    // -----------------------------------------------------------------------
    // Tests — TypeExpr (Phase 5, step 5.1)
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

        // Nullable of Nullable falls to the bottom.
        let nn = TypeExpr::Nullable(Box::new(TypeExpr::Nullable(Box::new(TypeExpr::named(
            "Int",
        )))));
        assert_eq!(nn.head_name(), "Int");
    }

    #[test]
    fn type_expr_is_nullable_solo_a_nivel_top() {
        assert!(!TypeExpr::named("Int").is_nullable());
        assert!(TypeExpr::Nullable(Box::new(TypeExpr::named("Int"))).is_nullable());
        // `List<Int?>` is not nullable itself; the inner field is.
        let outer = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Nullable(Box::new(TypeExpr::named("Int")))],
        };
        assert!(!outer.is_nullable());
    }
}
