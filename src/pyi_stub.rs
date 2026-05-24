// pyi_stub.rs — Parser de `.pyi` stubs Python (PEP 484/561).
//
// Quick win pyi-stubs (v0.9.39): cuando un programa Fitz hace
// `from python import foo` y existe `foo.pyi` adyacente, el loader
// parsea el stub y registra los tipos en el TypeEnv/module env. El
// checker entonces ve tipos reales (Int, Str, ChatMsg) en lugar de
// `PyAny` opaco.
//
// **Scope MVP**:
//   - Top-level `def name(args...) -> ret: ...`
//   - Top-level `class Name: ...` con fields anotados (sin métodos
//     por ahora — los métodos quedan como deuda menor).
//   - Top-level vars `name: type` y `name: type = default`.
//   - Type expressions: `int`, `str`, `float`, `bool`, `bytes`, `None`,
//     `Any`, `list[T]`, `dict[K, V]`, `Optional[T]`, `Union[T, None]`,
//     `T | None` (PEP 604).
//
// **NO en el MVP** (deuda residual explícita):
//   - Generics genéricos definidos por el user (`class Foo(Generic[T]): ...`).
//   - Protocol / TypedDict / overload.
//   - Métodos de clase con `self`.
//   - Decorators (`@property`, `@staticmethod`, etc.).
//   - Imports relativos / re-exports.
//
// **Diseño**: tokenizer + recursive descent parser sobre un subset
// estricto de la sintaxis. NO usamos un parser Python completo (sería
// ~50× el código de este módulo) ni dependemos de tree-sitter — el
// scope es chico y este enfoque acotado mantiene el control.

use crate::types::{Type, TypeEnv, TypeId};

// ---------------------------------------------------------------------------
// AST de stubs
// ---------------------------------------------------------------------------

/// Item top-level de un stub `.pyi`. Sub-set mínimo del PEP 484.
#[derive(Debug, Clone, PartialEq)]
pub enum StubItem {
    /// `def name(args...) -> ret: ...` (sync o `async def`).
    Fn(StubFn),
    /// `class Name: <body>` con fields anotados.
    Class(StubClass),
    /// `name: type` o `name: type = default` a nivel top-level.
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

/// Type expression de Python. Sub-set del PEP 484.
#[derive(Debug, Clone, PartialEq)]
pub enum StubType {
    /// Identifier simple: `int`, `str`, `MyClass`, etc.
    Named(String),
    /// `name[args]`: `list[int]`, `dict[str, T]`, `Optional[int]`.
    Generic(String, Vec<StubType>),
    /// `T | None` (PEP 604) o `Union[T, None]`. Almacenamos los alts
    /// expandidos en una lista.
    Union(Vec<StubType>),
    /// `Any` o `...` u otros tipos no soportados — fall-through.
    Any,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Error de parsing del stub. No usamos `FitzError` porque los stubs
/// vienen de archivos Python (no Fitz), tienen su propia indexación
/// de errores (línea Python original).
#[derive(Debug, Clone, PartialEq)]
pub struct StubParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for StubParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "stub parse error línea {}: {}", self.line, self.message)
    }
}

/// Parsea el contenido de un archivo `.pyi`. Devuelve el listado de
/// items top-level. Items no reconocidos (ej. `from x import y`,
/// `class Foo(Base)` con bases) se skipean silenciosamente — el
/// parser es robust por diseño (un stub malformado debería degradar
/// a `PyAny` opaco, no romper el build).
///
/// Imports y otros constructs que el parser no entiende se descartan;
/// solo los items reconocibles vuelven al caller.
pub fn parse_stub(source: &str) -> Result<Vec<StubItem>, StubParseError> {
    let mut items = Vec::new();
    let mut lines = source.lines().enumerate().peekable();
    while let Some((line_no, raw_line)) = lines.next() {
        let trimmed = raw_line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Skip al primer no-whitespace que sea identificador, def, class, async.
        if let Some(item) = parse_top_level(raw_line, line_no, &mut lines)? {
            items.push(item);
        }
    }
    Ok(items)
}

/// Intenta parsear UN top-level item desde `line` (la línea actual).
/// Devuelve `None` si la línea no introduce un item reconocido
/// (e.g. `import`, decorator, línea suelta).
///
/// Para `class`, lee el body completo (líneas indentadas) hasta
/// encontrar una línea no indentada — usa el peekable `lines` para
/// consumirlas.
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
        // Línea indentada a nivel top: solo aplica adentro de un class
        // body. El caller (parse_stub) la skipea — nos contamos como
        // "no item reconocido aquí".
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
    // Var top-level: `name: type` o `name: type = default`.
    if let Some(var) = try_parse_var(trimmed, line_no)? {
        return Ok(Some(StubItem::Var(var)));
    }
    // Cualquier otra cosa (imports, decorators sueltos, líneas raras)
    // se ignora silenciosamente para no romper el build cuando el .pyi
    // tiene construcciones que no soportamos.
    Ok(None)
}

/// Parsea la signature de una función desde después del `def `/`async def `.
/// Espera: `name(params) -> ret: ...`.
fn parse_fn_sig(s: &str, line_no: usize, is_async: bool) -> Result<StubFn, StubParseError> {
    let (name, rest) = take_ident(s).ok_or_else(|| StubParseError {
        line: line_no + 1,
        message: format!("esperaba nombre de fn, fue: {}", s),
    })?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('(').ok_or_else(|| StubParseError {
        line: line_no + 1,
        message: format!("esperaba `(` después de `def {}`", name),
    })?;
    let (params_str, after_paren) =
        take_balanced(rest, '(', ')').ok_or_else(|| StubParseError {
            line: line_no + 1,
            message: "paréntesis sin cerrar en signature".to_string(),
        })?;
    let params = parse_params(params_str, line_no)?;
    // Ret type: `-> type`.
    let after = after_paren.trim_start();
    let ret = if let Some(after_arrow) = after.strip_prefix("->") {
        let after_arrow = after_arrow.trim_start();
        let (ty, _) = parse_type(after_arrow, line_no)?;
        ty
    } else {
        // Sin -> ret: stub mal formado o muy compactado. Asumimos `None`.
        StubType::Named("None".to_string())
    };
    Ok(StubFn {
        name: name.to_string(),
        params,
        ret,
        is_async,
    })
}

/// Parsea params: lista CSV de `name: type` (o `name: type = default`).
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
        // Skip self/cls (métodos de clase). El parser top-level no
        // entra a class methods en el MVP, pero defensa de profundidad.
        if piece == "self" || piece == "cls" {
            continue;
        }
        // Skip `*`/`**` (variadic args en stubs) — el MVP no los
        // representa. Su tipo se trata como Any.
        if piece.starts_with('*') {
            continue;
        }
        // Split `name: type [= default]`.
        let (name, after_colon) = match piece.split_once(':') {
            Some((n, after)) => (n.trim(), after.trim()),
            None => {
                // Param sin anotación — Python lo permite, lo mapeamos a Any.
                params.push(StubParam {
                    name: piece.to_string(),
                    ty: StubType::Any,
                    has_default: false,
                });
                continue;
            }
        };
        // Detectar default. `name: type = default` — el `=` es
        // top-level del param string (no adentro de un Optional[...]
        // u otro generic). Usamos detección balanceada.
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

/// Parsea `class Name [(Bases)]: ...` y su body (líneas indentadas
/// con fields). Devuelve el class + número de líneas consumidas.
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
        message: format!("esperaba nombre de class, fue: {}", s),
    })?;
    // Skip bases: `(Base, ...)`. No las usamos en el MVP.
    let after_name = rest.trim_start();
    let after_bases = if let Some(rest) = after_name.strip_prefix('(') {
        match take_balanced(rest, '(', ')') {
            Some((_, after)) => after.trim_start(),
            None => {
                return Err(StubParseError {
                    line: line_no + 1,
                    message: "paréntesis de bases sin cerrar".to_string(),
                });
            }
        }
    } else {
        after_name
    };
    // Esperamos `:` después del header.
    let _ = after_bases.strip_prefix(':');

    // Body: líneas indentadas hasta encontrar una línea no indentada.
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
            // Salimos del class body.
            break;
        }
        // Skip métodos del class (MVP no los procesa). Reconocemos
        // `def`/`async def` para consumir múltiples líneas si el body
        // del método ocupa varias (raro en stubs pero defensivo).
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            lines.next();
            consumed += 1;
            continue;
        }
        // Skip decorators de field/method.
        if trimmed.starts_with('@') {
            lines.next();
            consumed += 1;
            continue;
        }
        // Skip `...` y `pass`.
        if trimmed == "..." || trimmed == "pass" {
            lines.next();
            consumed += 1;
            continue;
        }
        // Field anotado: `name: type` o `name: type = default`.
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

/// Parsea una expresión de tipo Python. Reconoce primitivos, generics
/// (`list[T]`, `dict[K, V]`, `Optional[T]`), Union (PEP 604: `T | None`).
fn parse_type(s: &str, line_no: usize) -> Result<(StubType, &str), StubParseError> {
    let s = s.trim_start();
    // Detectar Union con `|`: parseamos un primer término y vamos
    // acumulando si vemos `|`.
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
    // `...` → Any (típico en stubs sin tipo concreto).
    if let Some(rest) = s.strip_prefix("...") {
        return Ok((StubType::Any, rest));
    }
    // String literal — usado en forward refs `"Foo"`. Lo tratamos
    // como Named(contenido).
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"').ok_or_else(|| StubParseError {
            line: line_no + 1,
            message: "string forward-ref sin cerrar".to_string(),
        })?;
        let inner = &rest[..end];
        return Ok((StubType::Named(inner.to_string()), &rest[end + 1..]));
    }
    if let Some(rest) = s.strip_prefix('\'') {
        let end = rest.find('\'').ok_or_else(|| StubParseError {
            line: line_no + 1,
            message: "string forward-ref sin cerrar".to_string(),
        })?;
        let inner = &rest[..end];
        return Ok((StubType::Named(inner.to_string()), &rest[end + 1..]));
    }
    // Identifier (posiblemente `module.Name`).
    let (name, after_name) = take_ident(s).ok_or_else(|| StubParseError {
        line: line_no + 1,
        message: format!("esperaba identifier de tipo, fue: {:?}", s),
    })?;
    // Si hay `.`, consume `module.Name`. El nombre canónico es la
    // parte después del último `.` (corrida: módulo es opcional en
    // el contexto Fitz; nos quedamos con el last segment).
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
                message: "brackets de generic sin cerrar".to_string(),
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
// Tokenizer helpers (no parser Python completo)
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

/// Toma el contenido entre `open` y su `close` balanceado, devolviendo
/// `(contenido, resto_después_del_close)`. Asume que `s` empieza
/// JUSTO DESPUÉS del `open`.
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

/// Split CSV teniendo en cuenta paréntesis y brackets balanceados —
/// `list[int, str]` cuenta como UN solo arg.
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

/// Encuentra la primera ocurrencia top-level (depth=0) de `=` en `s`.
/// `==` se descarta (no es asignación). Usado para separar `type` de
/// `default` en params/vars.
fn find_top_eq(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '=' if depth == 0 => {
                // Skip `==` (igualdad), no asignación.
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
// Mapper StubType → Type Fitz
// ---------------------------------------------------------------------------

/// Convierte un `StubType` (sintáctico Python) a un `Type` Fitz.
///
/// `env` se consulta para resolver nominales: si el stub menciona
/// `Foo` y `Foo` no es un primitivo conocido, lookupeamos en `env`.
/// Si tampoco existe ahí, lo registramos como nominal nuevo (sin
/// fields todavía — los rellenan los `StubItem::Class` más adelante).
/// Tipos no representables (Callable, Protocol, Generic[T] con T no
/// resoluble) → `Type::Any` como fallback gradual.
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
        // List/Dict sin args (raros en stubs modernos pero defensivos):
        // los tratamos como Any porque no tenemos el inner type.
        "list" | "List" | "dict" | "Dict" | "tuple" | "Tuple" => Type::Any,
        // Cualquier otro identifier es un nominal: lookupeamos en el
        // env; si no existe, lo registramos como nominal nuevo.
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
        // `Callable[...]` — no representable en Fitz hoy (no podemos
        // recuperar la signature precisa sin el shape de PEP 484).
        // Fallback a Any.
        ("Callable", _) => Type::Any,
        // Generic desconocido: si name es nominal, lookup; sino Any.
        _ => named_to_fitz_type(name, env),
    }
}

fn union_to_fitz_type(alts: &[StubType], env: &mut TypeEnv) -> Type {
    // Caso típico: `T | None` o `Union[T, None]` → `T?`.
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
    // Union de varios tipos no nullable → Any (Fitz no tiene unions
    // genéricas; el modelo gradual lo cubre).
    Type::Any
}

/// Registra un nominal "stub-only" en el TypeEnv. Los fields se
/// rellenan después cuando procesamos los `StubItem::Class` que lo
/// definen. Si el class aparece después en el mismo stub, el `id`
/// queda con fields vacíos y se actualiza posteriormente.
///
/// Si el nombre ya estaba registrado, devuelve el id existente
/// (lookup) — `declare_nominal` falla con "tipo redeclarado" pero
/// nosotros queremos resolver-o-crear.
fn register_unknown_nominal(env: &mut TypeEnv, name: &str) -> TypeId {
    if let Some(id) = env.lookup(name) {
        return id;
    }
    // Si llegó acá, no estaba — declare_nominal no debería fallar.
    // Si falla por race entre lookup y declare, fallback a un id
    // dummy (no debería pasar en práctica — pyi_stub corre
    // secuencialmente).
    env.declare_nominal(name.to_string())
        .unwrap_or_else(|_| panic!("declare_nominal falló inesperadamente"))
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
            panic!("esperaba Fn, fue {:?}", items[0]);
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
    fn parser_fn_con_default() {
        let items = parse("def greet(name: str, prefix: str = \"Hi\") -> str: ...");
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(f.params.len(), 2);
            assert!(!f.params[0].has_default);
            assert!(f.params[1].has_default);
        }
    }

    #[test]
    fn parser_class_con_fields() {
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
            panic!("esperaba Class");
        }
    }

    #[test]
    fn parser_class_skip_methods() {
        // Métodos NO entran a fields en el MVP — se ignoran sin
        // romper el parse.
        let src = "class User:\n    id: int\n    def greet(self) -> str: ...\n";
        let items = parse(src);
        if let StubItem::Class(c) = &items[0] {
            assert_eq!(c.fields.len(), 1, "el método no debería ser field");
        }
    }

    #[test]
    fn parser_var_top_level() {
        let items = parse("VERSION: str");
        if let StubItem::Var(v) = &items[0] {
            assert_eq!(v.name, "VERSION");
            assert_eq!(v.ty, StubType::Named("str".into()));
        } else {
            panic!("esperaba Var");
        }
    }

    #[test]
    fn parser_var_con_default() {
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
        // Solo `foo` debería ser reconocido (decoradores ignorados).
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
                panic!("esperaba Union, fue {:?}", f.ret);
            }
        }
    }

    #[test]
    fn parser_forward_ref_string() {
        // Stubs con forward ref: `"Foo"` como tipo.
        let items = parse("def make() -> \"Foo\": ...");
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(f.ret, StubType::Named("Foo".into()));
        }
    }

    #[test]
    fn parser_module_dotted() {
        // `os.PathLike` → tomamos el último segmento.
        let items = parse("def open(path: os.PathLike) -> int: ...");
        if let StubItem::Fn(f) = &items[0] {
            assert_eq!(f.params[0].ty, StubType::Named("PathLike".into()));
        }
    }

    // ---- Mapper StubType → Type Fitz ----

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
        // Union no-null → Any (fallback gradual).
        assert_eq!(fitz_ty, Type::Any);
    }

    #[test]
    fn mapper_nominal_se_registra_en_env() {
        let mut env = TypeEnv::new();
        let t = StubType::Named("User".into());
        let fitz_ty = stub_type_to_fitz_type(&t, &mut env);
        // Debería haberse registrado y devolver Nominal.
        assert!(matches!(fitz_ty, Type::Nominal(_)));
        // Y `User` ahora aparece en el env.
        assert!(env.lookup("User").is_some());
    }
}
