// pyi_stub.rs — Parser for Python `.pyi` stubs (PEP 484/561).
//
// pyi-stubs quick win (v0.9.39): when a Fitz program does
// `from python import foo` and `foo.pyi` exists alongside, the loader
// parses the stub and registers the types in TypeEnv/module env. The
// checker then sees real types (Int, Str, ChatMsg) instead of
// opaque `PyAny`.
//
// **MVP scope**:
//   - Top-level `def name(args...) -> ret: ...`
//   - Top-level `class Name: ...` with annotated fields (no methods
//     for now — methods stay as minor debt).
//   - Top-level vars `name: type` and `name: type = default`.
//   - Type expressions: `int`, `str`, `float`, `bool`, `bytes`, `None`,
//     `Any`, `list[T]`, `dict[K, V]`, `Optional[T]`, `Union[T, None]`,
//     `T | None` (PEP 604).
//
// **NOT in MVP** (explicit residual debt):
//   - User-defined generics (`class Foo(Generic[T]): ...`).
//   - Protocol / TypedDict / overload.
//   - Class methods with `self`.
//   - Decorators (`@property`, `@staticmethod`, etc.).
//   - Relative imports / re-exports.
//
// **Design**: tokenizer + recursive descent parser over a strict
// subset of the syntax. We do NOT use a complete Python parser (it
// would be ~50× the code of this module) and we do not depend on
// tree-sitter — the scope is small and this bounded approach keeps control.

use crate::types::{ResolvedField, Type, TypeEnv, TypeId};

// ---------------------------------------------------------------------------
// Stub AST
// ---------------------------------------------------------------------------

/// Top-level item of a `.pyi` stub. Minimum subset of PEP 484.
#[derive(Debug, Clone, PartialEq)]
pub enum StubItem {
    /// `def name(args...) -> ret: ...` (sync or `async def`).
    Fn(StubFn),
    /// `class Name: <body>` with annotated fields.
    Class(StubClass),
    /// `name: type` or `name: type = default` at top level.
    Var(StubVar),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StubFn {
    pub name: String,
    pub params: Vec<StubParam>,
    pub ret: StubType,
    pub is_async: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StubParam {
    pub name: String,
    pub ty: StubType,
    pub has_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StubClass {
    pub name: String,
    pub fields: Vec<StubField>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StubField {
    pub name: String,
    pub ty: StubType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StubVar {
    pub name: String,
    pub ty: StubType,
}

/// Python type expression. Subset of PEP 484.
#[derive(Debug, Clone, PartialEq)]
pub enum StubType {
    /// Simple identifier: `int`, `str`, `MyClass`, etc.
    Named(String),
    /// `name[args]`: `list[int]`, `dict[str, T]`, `Optional[int]`.
    Generic(String, Vec<StubType>),
    /// `T | None` (PEP 604) or `Union[T, None]`. We store the
    /// expanded alts in a list.
    Union(Vec<StubType>),
    /// `Any` or `...` or other unsupported types — fall-through.
    Any,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Stub parsing error. We do not use `FitzError` because stubs
/// come from Python (not Fitz) files; they have their own error
/// indexing (original Python line).
#[derive(Debug, Clone, PartialEq)]
pub struct StubParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for StubParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "stub parse error line {}: {}", self.line, self.message)
    }
}

/// Parses the contents of a `.pyi` file. Returns the list of
/// top-level items. Unrecognized items (e.g. `from x import y`,
/// `class Foo(Base)` with bases) are silently skipped — the
/// parser is robust by design (a malformed stub should degrade
/// to opaque `PyAny`, not break the build).
///
/// Imports and other constructs the parser does not understand are discarded;
/// only recognizable items come back to the caller.
pub fn parse_stub(source: &str) -> Result<Vec<StubItem>, StubParseError> {
    let mut items = Vec::new();
    let mut lines = source.lines().enumerate().peekable();
    while let Some((line_no, raw_line)) = lines.next() {
        let trimmed = raw_line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Skip to the first non-whitespace that is identifier, def, class, async.
        if let Some(item) = parse_top_level(raw_line, line_no, &mut lines)? {
            items.push(item);
        }
    }
    Ok(items)
}

/// Tries to parse ONE top-level item from `line` (the current line).
/// Returns `None` if the line does not introduce a recognized item
/// (e.g. `import`, decorator, stray line).
///
/// For `class`, reads the full body (indented lines) until
/// finding a non-indented line — uses the peekable `lines` to
/// consume them.
fn parse_top_level<'a, I>(
    line: &str,
    line_no: usize,
    lines: &mut std::iter::Peekable<I>,
) -> Result<Option<StubItem>, StubParseError>
where
    I: Iterator<Item = (usize, &'a str)>,
{
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if indent > 0 {
        // Indented line at top level: only applies inside a class
        // body. The caller (parse_stub) skips it — we count as
        // "no recognized item here".
        return Ok(None);
    }
    // def fn / async def fn
    if let Some(rest) = trimmed.strip_prefix("async def ") {
        return Ok(Some(StubItem::Fn(parse_fn_sig(rest, line_no, true)?)));
    }
    if let Some(rest) = trimmed.strip_prefix("def ") {
        return Ok(Some(StubItem::Fn(parse_fn_sig(rest, line_no, false)?)));
    }
    // class Name [(Bases)]: ...
    if let Some(rest) = trimmed.strip_prefix("class ") {
        let (cls, _) = parse_class(rest, line_no, lines)?;
        return Ok(Some(StubItem::Class(cls)));
    }
    // Top-level var: `name: type` or `name: type = default`.
    if let Some(var) = try_parse_var(trimmed, line_no)? {
        return Ok(Some(StubItem::Var(var)));
    }
    // Anything else (imports, stray decorators, unusual lines)
    // is silently ignored so the build does not break when the .pyi
    // contains constructs we do not support.
    Ok(None)
}

/// Parses a function signature from after `def `/`async def `.
/// Expects: `name(params) -> ret: ...`.
fn parse_fn_sig(s: &str, line_no: usize, is_async: bool) -> Result<StubFn, StubParseError> {
    let (name, rest) = take_ident(s).ok_or_else(|| StubParseError {
        line: line_no + 1,
        message: format!("expected fn name, found: {}", s),
    })?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('(').ok_or_else(|| StubParseError {
        line: line_no + 1,
        message: format!("expected `(` after `def {}`", name),
    })?;
    let (params_str, after_paren) =
        take_balanced(rest, '(', ')').ok_or_else(|| StubParseError {
            line: line_no + 1,
            message: "unclosed parenthesis in signature".to_string(),
        })?;
    let params = parse_params(params_str, line_no)?;
    // Ret type: `-> type`.
    let after = after_paren.trim_start();
    let ret = if let Some(after_arrow) = after.strip_prefix("->") {
        let after_arrow = after_arrow.trim_start();
        let (ty, _) = parse_type(after_arrow, line_no)?;
        ty
    } else {
        // Without -> ret: malformed or overly compacted stub. We assume `None`.
        StubType::Named("None".to_string())
    };
    Ok(StubFn {
        name: name.to_string(),
        params,
        ret,
        is_async,
    })
}

/// Parses params: CSV list of `name: type` (or `name: type = default`).
fn parse_params(s: &str, line_no: usize) -> Result<Vec<StubParam>, StubParseError> {
    let mut params = Vec::new();
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(params);
    }
    for piece in split_commas_balanced(trimmed) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        // Skip self/cls (class methods). The top-level parser does not
        // enter class methods in the MVP, but defense in depth.
        if piece == "self" || piece == "cls" {
            continue;
        }
        // Skip `*`/`**` (variadic args in stubs) — the MVP does not
        // represent them. Their type is treated as Any.
        if piece.starts_with('*') {
            continue;
        }
        // Split `name: type [= default]`.
        let (name, after_colon) = match piece.split_once(':') {
            Some((n, after)) => (n.trim(), after.trim()),
            None => {
                // Param without annotation — Python allows it; we map to Any.
                params.push(StubParam {
                    name: piece.to_string(),
                    ty: StubType::Any,
                    has_default: false,
                });
                continue;
            }
        };
        // Detect default. `name: type = default` — the `=` is
        // top-level of the param string (not inside an Optional[...]
        // or other generic). We use balanced detection.
        let (ty_str, has_default) = match find_top_eq(after_colon) {
            Some(eq_pos) => (after_colon[..eq_pos].trim(), true),
            None => (after_colon, false),
        };
        let (ty, _) = parse_type(ty_str, line_no)?;
        params.push(StubParam {
            name: name.to_string(),
            ty,
            has_default,
        });
    }
    Ok(params)
}

/// Parses `class Name [(Bases)]: ...` and its body (indented lines
/// with fields). Returns the class + number of consumed lines.
fn parse_class<'a, I>(
    s: &str,
    line_no: usize,
    lines: &mut std::iter::Peekable<I>,
) -> Result<(StubClass, usize), StubParseError>
where
    I: Iterator<Item = (usize, &'a str)>,
{
    let (name, rest) = take_ident(s).ok_or_else(|| StubParseError {
        line: line_no + 1,
        message: format!("expected class name, found: {}", s),
    })?;
    // Skip bases: `(Base, ...)`. We do not use them in the MVP.
    let after_name = rest.trim_start();
    let after_bases = if let Some(rest) = after_name.strip_prefix('(') {
        match take_balanced(rest, '(', ')') {
            Some((_, after)) => after.trim_start(),
            None => {
                return Err(StubParseError {
                    line: line_no + 1,
                    message: "unclosed parenthesis on bases".to_string(),
                });
            }
        }
    } else {
        after_name
    };
    // We expect `:` after the header.
    let _ = after_bases.strip_prefix(':');

    // Body: indented lines until finding a non-indented line.
    let mut fields = Vec::new();
    let mut consumed = 0;
    while let Some(&(next_line_no, raw)) = lines.peek() {
        let trimmed = raw.trim_start();
        let indent = raw.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            lines.next();
            consumed += 1;
            continue;
        }
        if indent == 0 {
            // We leave the class body.
            break;
        }
        // Skip class methods (MVP does not process them). We recognize
        // `def`/`async def` to consume multiple lines if the method's
        // body spans several (rare in stubs but defensive).
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            lines.next();
            consumed += 1;
            continue;
        }
        // Skip field/method decorators.
        if trimmed.starts_with('@') {
            lines.next();
            consumed += 1;
            continue;
        }
        // Skip `...` and `pass`.
        if trimmed == "..." || trimmed == "pass" {
            lines.next();
            consumed += 1;
            continue;
        }
        // Annotated field: `name: type` or `name: type = default`.
        if let Some(field) = try_parse_field(trimmed, next_line_no)? {
            fields.push(field);
        }
        lines.next();
        consumed += 1;
    }
    Ok((
        StubClass {
            name: name.to_string(),
            fields,
        },
        consumed,
    ))
}

fn try_parse_field(s: &str, line_no: usize) -> Result<Option<StubField>, StubParseError> {
    let (name, after_colon) = match s.split_once(':') {
        Some((n, a)) => (n.trim(), a.trim()),
        None => return Ok(None),
    };
    if name.is_empty() || !is_valid_ident(name) {
        return Ok(None);
    }
    let ty_str = match find_top_eq(after_colon) {
        Some(eq_pos) => after_colon[..eq_pos].trim(),
        None => after_colon,
    };
    let (ty, _) = parse_type(ty_str, line_no)?;
    Ok(Some(StubField {
        name: name.to_string(),
        ty,
    }))
}

fn try_parse_var(s: &str, line_no: usize) -> Result<Option<StubVar>, StubParseError> {
    let (name, after_colon) = match s.split_once(':') {
        Some((n, a)) => (n.trim(), a.trim()),
        None => return Ok(None),
    };
    if name.is_empty() || !is_valid_ident(name) {
        return Ok(None);
    }
    let ty_str = match find_top_eq(after_colon) {
        Some(eq_pos) => after_colon[..eq_pos].trim(),
        None => after_colon,
    };
    let (ty, _) = parse_type(ty_str, line_no)?;
    Ok(Some(StubVar {
        name: name.to_string(),
        ty,
    }))
}

/// Parses a Python type expression. Recognizes primitives, generics
/// (`list[T]`, `dict[K, V]`, `Optional[T]`), Union (PEP 604: `T | None`).
fn parse_type(s: &str, line_no: usize) -> Result<(StubType, &str), StubParseError> {
    let s = s.trim_start();
    // Detect Union with `|`: we parse a first term and keep
    // accumulating if we see `|`.
    let (first, mut rest) = parse_type_atom(s, line_no)?;
    let mut alts = vec![first];
    loop {
        let trimmed_rest = rest.trim_start();
        if let Some(after_pipe) = trimmed_rest.strip_prefix('|') {
            let (next, after) = parse_type_atom(after_pipe.trim_start(), line_no)?;
            alts.push(next);
            rest = after;
        } else {
            break;
        }
    }
    let ty = if alts.len() == 1 {
        alts.into_iter().next().unwrap()
    } else {
        StubType::Union(alts)
    };
    Ok((ty, rest))
}

fn parse_type_atom(s: &str, line_no: usize) -> Result<(StubType, &str), StubParseError> {
    let s = s.trim_start();
    // `...` → Any (typical in stubs without a concrete type).
    if let Some(rest) = s.strip_prefix("...") {
        return Ok((StubType::Any, rest));
    }
    // String literal — used in forward refs `"Foo"`. We treat it
    // as Named(content).
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"').ok_or_else(|| StubParseError {
            line: line_no + 1,
            message: "unclosed forward-ref string".to_string(),
        })?;
        let inner = &rest[..end];
        return Ok((StubType::Named(inner.to_string()), &rest[end + 1..]));
    }
    if let Some(rest) = s.strip_prefix('\'') {
        let end = rest.find('\'').ok_or_else(|| StubParseError {
            line: line_no + 1,
            message: "unclosed forward-ref string".to_string(),
        })?;
        let inner = &rest[..end];
        return Ok((StubType::Named(inner.to_string()), &rest[end + 1..]));
    }
    // Identifier (possibly `module.Name`).
    let (name, after_name) = take_ident(s).ok_or_else(|| StubParseError {
        line: line_no + 1,
        message: format!("expected type identifier, found: {:?}", s),
    })?;
    // If there is a `.`, consume `module.Name`. The canonical name is the
    // part after the last `.` (shorthand: module is optional in
    // the Fitz context; we keep the last segment).
    let mut full_name = name.to_string();
    let mut after = after_name;
    while after.starts_with('.') {
        let after_dot = &after[1..];
        if let Some((n2, rest2)) = take_ident(after_dot) {
            full_name = n2.to_string();
            after = rest2;
        } else {
            break;
        }
    }
    // Generic params: `[args]`.
    let after = after.trim_start();
    if let Some(after_bracket) = after.strip_prefix('[') {
        let (inner, after_close) =
            take_balanced(after_bracket, '[', ']').ok_or_else(|| StubParseError {
                line: line_no + 1,
                message: "unclosed generic brackets".to_string(),
            })?;
        let args = parse_type_args(inner, line_no)?;
        return Ok((StubType::Generic(full_name, args), after_close));
    }
    Ok((StubType::Named(full_name), after))
}

fn parse_type_args(s: &str, line_no: usize) -> Result<Vec<StubType>, StubParseError> {
    let mut args = Vec::new();
    for piece in split_commas_balanced(s) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let (ty, _) = parse_type(piece, line_no)?;
        args.push(ty);
    }
    Ok(args)
}

// ---------------------------------------------------------------------------
// Tokenizer helpers (not a complete Python parser)
// ---------------------------------------------------------------------------

fn take_ident(s: &str) -> Option<(&str, &str)> {
    let mut chars = s.char_indices();
    let first = chars.next()?;
    if !is_ident_start(first.1) {
        return None;
    }
    let mut end = first.0 + first.1.len_utf8();
    for (i, c) in chars {
        if is_ident_continue(c) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    Some((&s[..end], &s[end..]))
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if is_ident_start(c) => chars.all(is_ident_continue),
        _ => false,
    }
}

/// Takes the content between `open` and its balanced `close`, returning
/// `(content, rest_after_close)`. Assumes that `s` starts
/// JUST AFTER the `open`.
fn take_balanced(s: &str, open: char, close: char) -> Option<(&str, &str)> {
    let mut depth = 1usize;
    let mut end = None;
    for (i, c) in s.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                end = Some(i);
                break;
            }
        }
    }
    let end = end?;
    Some((&s[..end], &s[end + close.len_utf8()..]))
}

/// CSV split taking balanced parentheses and brackets into account —
/// `list[int, str]` counts as ONE single arg.
fn split_commas_balanced(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start <= s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Finds the first top-level (depth=0) occurrence of `=` in `s`.
/// `==` is discarded (not assignment). Used to separate `type` from
/// `default` in params/vars.
fn find_top_eq(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '=' if depth == 0 => {
                // Skip `==` (equality), not assignment.
                if let Some(&(_, next)) = chars.peek() {
                    if next == '=' {
                        chars.next();
                        continue;
                    }
                }
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Mapper StubType → Fitz Type
// ---------------------------------------------------------------------------

/// Converts a `StubType` (Python syntactic) to a Fitz `Type`.
///
/// `env` is consulted to resolve nominals: if the stub mentions
/// `Foo` and `Foo` is not a known primitive, we look up in `env`.
/// If it does not exist there either, we register it as a new nominal
/// (without fields yet — the `StubItem::Class` later fill them in).
/// Non-representable types (Callable, Protocol, Generic[T] with non-resolvable T)
/// → `Type::Any` as gradual fallback.
pub fn stub_type_to_fitz_type(ty: &StubType, env: &mut TypeEnv) -> Type {
    match ty {
        StubType::Named(name) => named_to_fitz_type(name, env),
        StubType::Generic(name, args) => generic_to_fitz_type(name, args, env),
        StubType::Union(alts) => union_to_fitz_type(alts, env),
        StubType::Any => Type::Any,
    }
}

fn named_to_fitz_type(name: &str, env: &mut TypeEnv) -> Type {
    match name {
        "int" => Type::Int,
        "float" => Type::Float,
        "str" => Type::Str,
        "bool" => Type::Bool,
        "None" | "NoneType" => Type::Null,
        "bytes" | "bytearray" => Type::Bytes,
        "Any" | "object" => Type::Any,
        // List/Dict without args (rare in modern stubs but defensive):
        // we treat them as Any because we do not have the inner type.
        "list" | "List" | "dict" | "Dict" | "tuple" | "Tuple" => Type::Any,
        // Any other identifier is a nominal: look up in the
        // env; if it does not exist, we register it as a new nominal.
        _ => {
            if let Some(id) = env.lookup(name) {
                Type::Nominal(id)
            } else {
                let id = register_unknown_nominal(env, name);
                Type::Nominal(id)
            }
        }
    }
}

fn generic_to_fitz_type(name: &str, args: &[StubType], env: &mut TypeEnv) -> Type {
    match (name, args.len()) {
        ("list" | "List", 1) => Type::List(Box::new(stub_type_to_fitz_type(&args[0], env))),
        ("dict" | "Dict", 2) => Type::Map(
            Box::new(stub_type_to_fitz_type(&args[0], env)),
            Box::new(stub_type_to_fitz_type(&args[1], env)),
        ),
        ("Optional", 1) => Type::Nullable(Box::new(stub_type_to_fitz_type(&args[0], env))),
        ("Union", _) => union_to_fitz_type(args, env),
        // `Callable[...]` — not representable in Fitz today (we cannot
        // recover the precise signature without PEP 484's shape).
        // Fall back to Any.
        ("Callable", _) => Type::Any,
        // Unknown generic: if name is a nominal, lookup; else Any.
        _ => named_to_fitz_type(name, env),
    }
}

fn union_to_fitz_type(alts: &[StubType], env: &mut TypeEnv) -> Type {
    // Typical case: `T | None` or `Union[T, None]` → `T?`.
    let mut non_null: Vec<&StubType> = Vec::new();
    let mut has_null = false;
    for alt in alts {
        match alt {
            StubType::Named(n) if n == "None" || n == "NoneType" => {
                has_null = true;
            }
            _ => non_null.push(alt),
        }
    }
    if non_null.len() == 1 && has_null {
        return Type::Nullable(Box::new(stub_type_to_fitz_type(non_null[0], env)));
    }
    if non_null.len() == 1 && !has_null {
        return stub_type_to_fitz_type(non_null[0], env);
    }
    // Union of several non-nullable types → Any (Fitz has no generic
    // unions; the gradual model covers it).
    Type::Any
}

/// Registers a "stub-only" nominal in the TypeEnv. The fields get
/// filled in later when we process the `StubItem::Class` that
/// defines it. If the class appears later in the same stub, the `id`
/// has empty fields and gets updated afterwards.
///
/// If the name was already registered, returns the existing id
/// (lookup) — `declare_nominal` fails with "type redeclared" but
/// we want resolve-or-create.
fn register_unknown_nominal(env: &mut TypeEnv, name: &str) -> TypeId {
    if let Some(id) = env.lookup(name) {
        return id;
    }
    // If we get here, it was not there — declare_nominal should not fail.
    // If it fails due to a race between lookup and declare, fall back to a
    // dummy id (should not happen in practice — pyi_stub runs
    // sequentially).
    env.declare_nominal(name.to_string())
        .unwrap_or_else(|_| panic!("declare_nominal failed unexpectedly"))
}

// ---------------------------------------------------------------------------
// Public API: register stub items into the TypeEnv
// ---------------------------------------------------------------------------

/// Resolved stub item ready for the checker to consume. Maps each
/// `StubItem` to its Fitz typed counterpart. Top-level fns and vars
/// are exposed as bindings of the typed Python module; classes already
/// live in the TypeEnv as nominals.
///
/// 8-pyi.B: built by `register_stub_items_into_env` and consumed by
/// the checker (via `LoadedStub` in `pyi_loader.rs`) to
/// refine bindings of `from python import foo` with the adjacent `foo.pyi`.
#[derive(Debug, Clone)]
pub enum ResolvedStubItem {
    /// `def name(args...) -> ret` — reflects the signature as `Type::Function`.
    Fn {
        name: String,
        params: Vec<Type>,
        ret: Type,
    },
    /// `class Name: <fields>` — already registered as Nominal in env. We carry the id
    /// for later consumption (e.g. typed field access in 8-pyi.C).
    Class { name: String, id: TypeId },
    /// Top-level stub `name: type` — bind with the declared type.
    Var { name: String, ty: Type },
}

/// Registers all items of a parsed `.pyi` into the TypeEnv:
///
/// 1. Class pre-scan: declares each empty nominal (no fields)
///    so the rest of the items can refer to them forward.
/// 2. Processes each class: sets fields resolving each `StubType`
///    to its corresponding Fitz `Type`.
/// 3. Processes fns and vars: resolves them to `ResolvedStubItem` so
///    the checker can bind the typed Python module.
///
/// **Error policy**: if a class tries to redeclare a nominal that
/// already exists with the same name (e.g. the Fitz program declares
/// `type Foo` and the stub also declares `class Foo`), we reuse the
/// existing nominal without replacing its fields — the Fitz program
/// wins. This preserves compatibility with the pattern
/// `fitz py-stubs requests.pyi --out requests.fitz` + `from python
/// import requests` (the types are already registered from the .fitz).
///
/// Returns the resolved listing in the same order as `items`.
pub fn register_stub_items_into_env(
    items: &[StubItem],
    env: &mut TypeEnv,
) -> Vec<ResolvedStubItem> {
    // Pre-scan: register all classes as empty nominals
    // to unblock forward refs (class A references B in a field, B
    // declared later).
    for item in items {
        if let StubItem::Class(c) = item {
            register_unknown_nominal(env, &c.name);
        }
    }

    // Process items in order.
    let mut resolved = Vec::with_capacity(items.len());
    for item in items {
        match item {
            StubItem::Class(c) => {
                let id = register_unknown_nominal(env, &c.name);
                // Only set fields if the nominal does NOT already have fields
                // (preserves classes declared by the Fitz program —
                // "the .fitz wins over the .pyi" policy).
                if env.info(id).fields.is_none() {
                    let fields: Vec<ResolvedField> = c
                        .fields
                        .iter()
                        .map(|f| ResolvedField {
                            name: f.name.clone(),
                            type_: stub_type_to_fitz_type(&f.ty, env),
                        })
                        .collect();
                    env.set_fields(id, fields);
                }
                resolved.push(ResolvedStubItem::Class {
                    name: c.name.clone(),
                    id,
                });
            }
            StubItem::Fn(f) => {
                let params: Vec<Type> = f
                    .params
                    .iter()
                    .map(|p| stub_type_to_fitz_type(&p.ty, env))
                    .collect();
                let ret = stub_type_to_fitz_type(&f.ret, env);
                resolved.push(ResolvedStubItem::Fn {
                    name: f.name.clone(),
                    params,
                    ret,
                });
            }
            StubItem::Var(v) => {
                let ty = stub_type_to_fitz_type(&v.ty, env);
                resolved.push(ResolvedStubItem::Var {
                    name: v.name.clone(),
                    ty,
                });
            }
        }
    }
    resolved
}

/// Returns the declared `ResolvedField` of a nominal in the TypeEnv,
/// if it has fields set.
pub fn nominal_fields(env: &TypeEnv, id: TypeId) -> Option<&[ResolvedField]> {
    env.info(id).fields.as_deref()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<StubItem> {
        parse_stub(s).expect("parse OK")
    }

    #[test]
    fn parser_fn_simple() {
        let items = parse("def add(a: int, b: int) -> int: ...");
        assert_eq!(items.len(), 1);
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(f.name, "add");
            assert_eq!(f.params.len(), 2);
            assert_eq!(f.params[0].name, "a");
            assert_eq!(f.params[0].ty, StubType::Named("int".into()));
            assert_eq!(f.ret, StubType::Named("int".into()));
            assert!(!f.is_async);
        } else {
            panic!("expected Fn, got {:?}", items[0]);
        }
    }

    #[test]
    fn parser_fn_async() {
        let items = parse("async def fetch(url: str) -> str: ...");
        assert_eq!(items.len(), 1);
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(f.name, "fetch");
            assert!(f.is_async);
        }
    }

    #[test]
    fn parser_fn_with_default() {
        let items = parse("def greet(name: str, prefix: str = \"Hi\") -> str: ...");
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(f.params.len(), 2);
            assert!(!f.params[0].has_default);
            assert!(f.params[1].has_default);
        }
    }

    #[test]
    fn parser_class_with_fields() {
        let src = "class User:\n    id: int\n    name: str\n    age: int = 0\n";
        let items = parse(src);
        assert_eq!(items.len(), 1);
        if let StubItem::Class(c) = &items[0] {
            assert_eq!(c.name, "User");
            assert_eq!(c.fields.len(), 3);
            assert_eq!(c.fields[0].name, "id");
            assert_eq!(c.fields[0].ty, StubType::Named("int".into()));
            assert_eq!(c.fields[2].name, "age");
        } else {
            panic!("expected Class");
        }
    }

    #[test]
    fn parser_class_skip_methods() {
        // Methods do NOT become fields in the MVP — they are ignored
        // without breaking the parse.
        let src = "class User:\n    id: int\n    def greet(self) -> str: ...\n";
        let items = parse(src);
        if let StubItem::Class(c) = &items[0] {
            assert_eq!(c.fields.len(), 1, "the method should not be a field");
        }
    }

    #[test]
    fn parser_var_top_level() {
        let items = parse("VERSION: str");
        if let StubItem::Var(v) = &items[0] {
            assert_eq!(v.name, "VERSION");
            assert_eq!(v.ty, StubType::Named("str".into()));
        } else {
            panic!("expected Var");
        }
    }

    #[test]
    fn parser_var_with_default() {
        let items = parse("DEBUG: bool = False");
        if let StubItem::Var(v) = &items[0] {
            assert_eq!(v.name, "DEBUG");
            assert_eq!(v.ty, StubType::Named("bool".into()));
        }
    }

    #[test]
    fn parser_skip_imports_y_decorators() {
        let src = "from typing import Any\nimport os\n@deprecated\ndef foo() -> int: ...\n";
        let items = parse(src);
        // Only `foo` should be recognized (decorators ignored).
        let fns = items
            .iter()
            .filter(|i| matches!(i, StubItem::Fn(_)))
            .count();
        assert_eq!(fns, 1);
    }

    #[test]
    fn parser_generic_list() {
        let items = parse("def lengths(xs: list[str]) -> list[int]: ...");
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(
                f.params[0].ty,
                StubType::Generic("list".into(), vec![StubType::Named("str".into())])
            );
            assert_eq!(
                f.ret,
                StubType::Generic("list".into(), vec![StubType::Named("int".into())])
            );
        }
    }

    #[test]
    fn parser_generic_dict() {
        let items = parse("def words() -> dict[str, int]: ...");
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(
                f.ret,
                StubType::Generic(
                    "dict".into(),
                    vec![StubType::Named("str".into()), StubType::Named("int".into()),]
                )
            );
        }
    }

    #[test]
    fn parser_optional() {
        let items = parse("def find(x: int) -> Optional[str]: ...");
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(
                f.ret,
                StubType::Generic("Optional".into(), vec![StubType::Named("str".into())])
            );
        }
    }

    #[test]
    fn parser_pep604_union() {
        // `T | None` (PEP 604).
        let items = parse("def find() -> str | None: ...");
        if let StubItem::Fn(f) = &items[0] {
            if let StubType::Union(alts) = &f.ret {
                assert_eq!(alts.len(), 2);
                assert_eq!(alts[0], StubType::Named("str".into()));
                assert_eq!(alts[1], StubType::Named("None".into()));
            } else {
                panic!("expected Union, got {:?}", f.ret);
            }
        }
    }

    #[test]
    fn parser_forward_ref_string() {
        // Stubs with forward ref: `"Foo"` as a type.
        let items = parse("def make() -> \"Foo\": ...");
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(f.ret, StubType::Named("Foo".into()));
        }
    }

    #[test]
    fn parser_module_dotted() {
        // `os.PathLike` → we take the last segment.
        let items = parse("def open(path: os.PathLike) -> int: ...");
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(f.params[0].ty, StubType::Named("PathLike".into()));
        }
    }

    // ---- Mapper StubType → Fitz Type ----

    #[test]
    fn mapper_primitivos() {
        let mut env = TypeEnv::new();
        assert_eq!(
            stub_type_to_fitz_type(&StubType::Named("int".into()), &mut env),
            Type::Int
        );
        assert_eq!(
            stub_type_to_fitz_type(&StubType::Named("str".into()), &mut env),
            Type::Str
        );
        assert_eq!(
            stub_type_to_fitz_type(&StubType::Named("bool".into()), &mut env),
            Type::Bool
        );
        assert_eq!(
            stub_type_to_fitz_type(&StubType::Named("None".into()), &mut env),
            Type::Null
        );
        assert_eq!(
            stub_type_to_fitz_type(&StubType::Named("bytes".into()), &mut env),
            Type::Bytes
        );
    }

    #[test]
    fn mapper_list_int() {
        let mut env = TypeEnv::new();
        let t = StubType::Generic("list".into(), vec![StubType::Named("int".into())]);
        let fitz_ty = stub_type_to_fitz_type(&t, &mut env);
        assert_eq!(fitz_ty, Type::List(Box::new(Type::Int)));
    }

    #[test]
    fn mapper_dict_str_int() {
        let mut env = TypeEnv::new();
        let t = StubType::Generic(
            "dict".into(),
            vec![StubType::Named("str".into()), StubType::Named("int".into())],
        );
        let fitz_ty = stub_type_to_fitz_type(&t, &mut env);
        assert_eq!(fitz_ty, Type::Map(Box::new(Type::Str), Box::new(Type::Int)));
    }

    #[test]
    fn mapper_optional() {
        let mut env = TypeEnv::new();
        let t = StubType::Generic("Optional".into(), vec![StubType::Named("str".into())]);
        let fitz_ty = stub_type_to_fitz_type(&t, &mut env);
        assert_eq!(fitz_ty, Type::Nullable(Box::new(Type::Str)));
    }

    #[test]
    fn mapper_pep604_t_or_none() {
        let mut env = TypeEnv::new();
        let t = StubType::Union(vec![
            StubType::Named("int".into()),
            StubType::Named("None".into()),
        ]);
        let fitz_ty = stub_type_to_fitz_type(&t, &mut env);
        assert_eq!(fitz_ty, Type::Nullable(Box::new(Type::Int)));
    }

    #[test]
    fn mapper_union_no_null_es_any() {
        let mut env = TypeEnv::new();
        let t = StubType::Union(vec![
            StubType::Named("int".into()),
            StubType::Named("str".into()),
        ]);
        let fitz_ty = stub_type_to_fitz_type(&t, &mut env);
        // Non-null Union → Any (gradual fallback).
        assert_eq!(fitz_ty, Type::Any);
    }

    #[test]
    fn mapper_nominal_se_registra_en_env() {
        let mut env = TypeEnv::new();
        let t = StubType::Named("User".into());
        let fitz_ty = stub_type_to_fitz_type(&t, &mut env);
        // Should have been registered and return Nominal.
        assert!(matches!(fitz_ty, Type::Nominal(_)));
        // And `User` now appears in the env.
        assert!(env.lookup("User").is_some());
    }
}
