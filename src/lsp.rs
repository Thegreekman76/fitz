// lsp.rs — Lógica del Language Server Protocol (Fase 9.x.1.b).
//
// Vive en la lib (`src/lib.rs` la expone como `pub mod lsp` detrás de
// la feature `lsp`) en lugar de adentro del bin para que sea
// unit-testeable: cargo no soporta bien `#[cfg(test)]` en `src/bin/*.rs`.
// El bin `src/bin/fitz-lsp.rs` consume esto vía `use fitz::lsp::...`.

use crate::ast::{Program, Span, Stmt};
use crate::error::FitzError;
use crate::lexer::tokenize;
use crate::parser::parse_with_recovery;
use crate::types::{check_program, DefinitionInfo, Type, TypeEnv, TypeInfo};

use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, Hover, HoverContents,
    Location, MarkupContent, MarkupKind, Position, Range, Url,
};

/// Pipeline LSP-style sobre `source`: tokeniza, parsea con recovery,
/// chequea tipos, y devuelve la lista combinada de errores. El nombre
/// "lsp-style" lo distingue de `fitz check`/`fitz run` que usan
/// `parser::parse` strict y abortan al primer error de parser.
///
/// Los errores del lexer abortan la pipeline (no hay AST sobre el cual
/// chequear). Parser y checker siempre devuelven sus errores en
/// paralelo a lo que pudieron recuperar.
///
/// Esta variante descarta los side-tables y el AST retornados por
/// `check_program` (consumidores que solo necesitan diagnostics). Para
/// hover / go-to-definition / completion, usar `check_source_with_types`.
pub fn check_source(source: &str) -> Vec<FitzError> {
    let (_program, _env, _type_info, _def_info, errors) =
        check_source_with_types(source);
    errors
}

/// Pipeline LSP-style + `Program`, `TypeEnv`, `TypeInfo` y
/// `DefinitionInfo` retenidos. Variante de `check_source` para
/// consumidores que necesitan el AST (Fase 9.x.4 — autocomplete
/// scope-level enumera top-level), el env para resolver nombres
/// nominales (hover/completion), el side-table de tipos por nodo
/// (hover, autocomplete after-dot), y el side-table de definiciones
/// por uso (go-to-definition).
///
/// Si la pipeline aborta antes del checker (error de lexer), Program
/// queda vacío y los side-tables también.
pub fn check_source_with_types(
    source: &str,
) -> (Program, TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>) {
    let tokens = match tokenize(source) {
        Ok(t) => t,
        Err(e) => {
            return (
                Vec::new(),
                TypeEnv::default(),
                TypeInfo::new(),
                DefinitionInfo::new(),
                vec![e],
            );
        }
    };
    let (program, mut errors) = parse_with_recovery(tokens);
    let (env, type_info, def_info, mut type_errors) = check_program(&program);
    errors.append(&mut type_errors);
    (program, env, type_info, def_info, errors)
}

/// Convierte una lista de `FitzError` en `Diagnostic`s LSP. Pure
/// function — no toca el server ni I/O. El test suite cubre las
/// reglas de mapeo (1-based Fitz → 0-based LSP, sin posición → range
/// degenerado, hint → concatenado al message).
pub fn fitz_errors_to_diagnostics(errors: &[FitzError]) -> Vec<Diagnostic> {
    errors.iter().map(error_to_diagnostic).collect()
}

fn error_to_diagnostic(err: &FitzError) -> Diagnostic {
    // Fitz usa convención 1-based para line/column (el lexer empieza
    // en `line: 1, column: 1`). LSP usa 0-based en `Position`. Cuando
    // line == 0 && column == 0 es el sentinel "sin posición" (ver
    // `FitzError::Display` en `error.rs`); lo mapeamos a un range
    // degenerado al inicio del documento.
    let range = if err.line == 0 && err.column == 0 {
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        }
    } else {
        // saturating_sub por defensa contra `line == 0 && column != 0`
        // (no debería ocurrir, pero evita underflow). El range es de
        // 1 carácter — refinable a span completo cuando S1.Pattern/
        // TypeExpr sume `end_span` a los nodos del AST.
        let line = (err.line.saturating_sub(1)) as u32;
        let col = (err.column.saturating_sub(1)) as u32;
        Range {
            start: Position::new(line, col),
            end: Position::new(line, col + 1),
        }
    };

    // Concatenamos el hint al message porque LSP no tiene un campo
    // dedicado para sugerencias. VSCode renderea `\n` en el tooltip;
    // el formato matchea cómo `FitzError::Display` lo emite en CLI.
    let mut message = err.message.clone();
    if let Some(hint) = &err.hint {
        message.push_str("\n  Sugerencia: ");
        message.push_str(hint);
    }

    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("fitz".into()),
        message,
        ..Diagnostic::default()
    }
}

// ---------------------------------------------------------------------------
// Hover (Fase 9.x.2)
// ---------------------------------------------------------------------------

/// Encuentra el tipo del nodo "bajo el cursor" según una posición LSP
/// (0-based). Heurística pragmática para el MVP: filtra las entries de
/// `TypeInfo` cuya línea coincide con el cursor y cuya columna es
/// menor o igual a la del cursor, y devuelve el `Type` cuya columna
/// es máxima (el `Expr` más cercano a la izquierda en la misma línea).
///
/// **Por qué heurística y no rango exacto**: los `Span` de Fitz hoy
/// solo guardan el inicio del nodo (deuda S1.Pattern/TypeExpr). Sin
/// `end_span`, no podemos decir "el cursor está adentro del nodo X";
/// asumimos que el último Expr iniciado antes del cursor en la misma
/// línea es el más probable. Cubre el 90% del caso (cursor sobre o
/// inmediatamente después de un identificador/literal). Refinable
/// cuando los nodos tengan span completo.
///
/// **Colisiones en `TypeInfo`**: cuando dos `Expr` distintos comparten
/// span (típicamente un `BinOp` y su primer operando), `TypeInfo`
/// guarda solo el último escrito — heredado de F16. En la práctica
/// el tipo del Expr más "grande" suele ser lo que el usuario quiere
/// ver al hover.
pub fn hover_for_position(
    type_info: &TypeInfo,
    line: u32,
    character: u32,
) -> Option<&Type> {
    // LSP 0-based → Fitz 1-based.
    let target_line = (line as usize) + 1;
    let target_col = (character as usize) + 1;
    type_info
        .iter()
        .filter(|(key, _)| key.0 == target_line && key.1 <= target_col)
        .max_by_key(|(key, _)| key.1)
        .map(|(_, ty)| ty)
}

/// Construye la respuesta `Hover` LSP a partir del `Type` encontrado
/// bajo el cursor. El tipo se renderea como bloque de código Fitz en
/// markdown — VSCode lo muestra con syntax highlighting nativo.
///
/// `range: None` porque sin `end_span` en los nodos no podemos
/// devolver el rango exacto al cliente (deuda S1.Pattern/TypeExpr).
/// Sin range, VSCode no resalta el token bajo el cursor, pero el
/// tooltip funciona igual.
pub fn make_hover(ty: &Type, env: &TypeEnv) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```fitz\n{}\n```", ty.display(env)),
        }),
        range: None,
    }
}

// ---------------------------------------------------------------------------
// Go-to-definition (Fase 9.x.3)
// ---------------------------------------------------------------------------

/// Encuentra el `Span` de la declaración del ident bajo el cursor según
/// una posición LSP (0-based). Misma heurística que `hover_for_position`:
/// filtra entries de `DefinitionInfo` cuya línea coincide con el cursor
/// y cuya columna es menor o igual a la del cursor, y devuelve el
/// `def_span` cuya columna es máxima (el ident más cercano a la
/// izquierda en la misma línea).
///
/// El span devuelto apunta a la posición de la declaración (1-based
/// Fitz). El caller lo convierte a `Range` LSP (0-based) vía
/// `make_definition_location`.
pub fn definition_for_position(
    def_info: &DefinitionInfo,
    line: u32,
    character: u32,
) -> Option<Span> {
    // LSP 0-based → Fitz 1-based.
    let target_line = (line as usize) + 1;
    let target_col = (character as usize) + 1;
    def_info
        .iter()
        .filter(|(key, _)| key.0 == target_line && key.1 <= target_col)
        .max_by_key(|(key, _)| key.1)
        .map(|(_, def_span)| *def_span)
}

/// Construye la respuesta `Location` LSP a partir del `Span` de
/// declaración. Convierte 1-based Fitz → 0-based LSP; range de 1
/// caracter porque sin `end_span` no podemos devolver el rango exacto
/// del identificador declarado (paralelo a `error_to_diagnostic`).
///
/// `uri` es el del documento actual: cross-module def queda como deuda
/// visible del MVP — los imports `from foo import X` registran el def
/// como el span del `Stmt::Import` local, no como la declaración real
/// en el módulo `foo`. Mapear paths del loader a URIs requiere
/// resolución cross-file que pertenece a una sub-fase posterior.
pub fn make_definition_location(uri: Url, def_span: Span) -> Location {
    let line = (def_span.line.saturating_sub(1)) as u32;
    let col = (def_span.column.saturating_sub(1)) as u32;
    Location {
        uri,
        range: Range {
            start: Position::new(line, col),
            end: Position::new(line, col + 1),
        },
    }
}

// ---------------------------------------------------------------------------
// Autocomplete contextual (Fase 9.x.4)
// ---------------------------------------------------------------------------

/// Contexto resuelto en `detect_completion_context`. Determina qué tipo
/// de completion devolver.
#[derive(Debug, PartialEq)]
enum CompletionContext {
    /// `obj.` o `obj.partial` — el receiver es un identificador cuyo
    /// tipo buscamos. Llevamos:
    /// - `recv_name`: para el fallback "buscar en top-level por nombre"
    ///   cuando TypeInfo no tiene el ident (caso típico: el parser
    ///   abortó el stmt entero por el `.` huérfano, deuda F15 recovery
    ///   sub-stmt).
    /// - `recv_line`/`recv_col`: posición Fitz 1-based del START del
    ///   receiver, para lookup en TypeInfo cuando sí está.
    AfterDot {
        recv_name: String,
        recv_line: usize,
        recv_col: usize,
    },
    /// Cualquier otro contexto — listamos top-level + builtins + keywords.
    ScopeLevel,
}

/// Endpoint principal de completion (Fase 9.x.4). Inspecciona el texto
/// para detectar si el cursor está después de un `.` (after-dot) o no
/// (scope-level), y devuelve la lista de `CompletionItem` apropiada.
///
/// **Scope-level**: enumera top-level del Program (`let`, `fn`, `type`,
/// `import` bindings) + builtins (`print`/`len`/`sleep`/`cors`) +
/// keywords del lenguaje. NO scope-aware: no enumeramos vars locales
/// y params como función de la posición del cursor (deuda MVP —
/// requiere refactor del checker para exponer scopes por stmt).
/// VSCode filtra por prefix client-side; el usuario puede tipear vars
/// locales aunque no aparezcan en la lista.
///
/// **After-dot**: identifica el receiver (un solo identificador antes
/// del `.`), busca su tipo en `TypeInfo` por la posición del start del
/// receiver, despacha por tipo:
/// - `Nominal(id)` → fields del type via `TypeEnv.info(id)`.
/// - `List<T>` → 6 métodos built-in.
/// - `Map<K, V>` → 5 métodos built-in.
/// - `Str` → 3 métodos.
/// - `Any`/`PyAny`/otros → lista vacía.
///
/// Chain `a.b.c.` queda como deuda visible — solo soporta
/// `<ident>.<prefix?>`.
pub fn completion_at_position(
    text: &str,
    program: &Program,
    type_info: &TypeInfo,
    type_env: &TypeEnv,
    line: u32,
    character: u32,
) -> Vec<CompletionItem> {
    let ctx = match detect_completion_context(text, line, character) {
        Some(c) => c,
        None => return Vec::new(),
    };
    match ctx {
        CompletionContext::AfterDot {
            recv_name,
            recv_line,
            recv_col,
        } => after_dot_completions(program, type_info, type_env, &recv_name, recv_line, recv_col),
        CompletionContext::ScopeLevel => scope_level_completions(program, type_env),
    }
}

/// Walkea hacia atrás del cursor en el texto. Si encuentra
/// `<ident>.<partial_prefix?>` devuelve `AfterDot` con la posición del
/// inicio del receiver. Si no, `ScopeLevel`. Devuelve `None` si la
/// posición no es válida (más allá del fin del texto).
fn detect_completion_context(
    text: &str,
    line: u32,
    character: u32,
) -> Option<CompletionContext> {
    let offset = position_to_offset(text, line, character)?;
    let bytes = text.as_bytes();
    // Saltar el prefix que el usuario ya tipeó (chars de identificador
    // antes del cursor).
    let mut i = offset;
    while i > 0 && is_ident_continue(bytes[i - 1]) {
        i -= 1;
    }
    // Si justo antes hay un `.`, contexto after-dot.
    if i > 0 && bytes[i - 1] == b'.' {
        let dot_pos = i - 1;
        let mut j = dot_pos;
        while j > 0 && is_ident_continue(bytes[j - 1]) {
            j -= 1;
        }
        if j < dot_pos {
            // Receiver: bytes[j..dot_pos]. Convertimos j a (line, col)
            // Fitz 1-based para lookup en TypeInfo.
            let recv_name = std::str::from_utf8(&bytes[j..dot_pos])
                .unwrap_or("")
                .to_string();
            let (recv_line_lsp, recv_col_lsp) = offset_to_position(text, j);
            return Some(CompletionContext::AfterDot {
                recv_name,
                recv_line: (recv_line_lsp as usize) + 1,
                recv_col: (recv_col_lsp as usize) + 1,
            });
        }
    }
    Some(CompletionContext::ScopeLevel)
}

/// Convierte una `(line, character)` LSP (0-based) a un offset en
/// bytes dentro del `text`. Devuelve `None` si la posición está más
/// allá del fin del texto. Asume que el cliente usa la misma convención
/// de "character" que nosotros (chars UTF-8 — LSP por default usa
/// UTF-16, pero el MVP asume programas mayormente ASCII; refinable
/// post-MVP si aparece presión real con código en idiomas no-latin).
fn position_to_offset(text: &str, line: u32, character: u32) -> Option<usize> {
    let mut offset = 0usize;
    let mut current_line = 0u32;
    let mut current_char = 0u32;
    for ch in text.chars() {
        if current_line == line && current_char == character {
            return Some(offset);
        }
        if ch == '\n' {
            current_line += 1;
            current_char = 0;
        } else {
            current_char += 1;
        }
        offset += ch.len_utf8();
    }
    if current_line == line && current_char == character {
        return Some(offset);
    }
    None
}

/// Inverso de `position_to_offset` — usado para localizar la posición
/// LSP de un punto en el texto dado en bytes (típicamente el start de
/// un receiver para hacer lookup en TypeInfo).
fn offset_to_position(text: &str, offset: usize) -> (u32, u32) {
    let mut current_line = 0u32;
    let mut current_char = 0u32;
    let mut current_offset = 0usize;
    for ch in text.chars() {
        if current_offset >= offset {
            break;
        }
        if ch == '\n' {
            current_line += 1;
            current_char = 0;
        } else {
            current_char += 1;
        }
        current_offset += ch.len_utf8();
    }
    (current_line, current_char)
}

/// Caracteres válidos a mitad de un identificador Fitz: alfanuméricos
/// ASCII + underscore. Coincide con la definición del lexer.
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Genera completions para after-dot: busca el tipo del receiver y
/// despacha por tipo.
///
/// **Resolución del tipo del receiver** con dos fallbacks:
/// 1. **TypeInfo lookup heurístico**: filtra entries cuya línea es la
///    misma del receiver y cuya col es <= recv_col, devuelve la de col
///    máxima. Funciona cuando el `Expr::Ident(recv_name, span)` quedó
///    en el AST (el caso `let r = foo.<cursor>` donde foo se parsea
///    bien aunque sea Field roto).
/// 2. **Walk del Program por nombre**: si TypeInfo no devolvió tipo,
///    walkeamos `Stmt::Assign` top-level buscando `target == recv_name`
///    y miramos el tipo del `value` en TypeInfo. Cubre el caso típico
///    del usuario tipeando `obj.` al final del buffer — el parser
///    abandona el stmt entero por el `.` huérfano (deuda F15 recovery
///    sub-stmt), entonces el Expr::Ident no llega a TypeInfo, pero el
///    `let obj = ...` previo sí tiene su value tipado.
///
/// Tipos cubiertos: `Nominal` (fields), `List` (6 métodos), `Map`
/// (5 métodos), `Str` (3 métodos). Otros devuelven lista vacía.
fn after_dot_completions(
    program: &Program,
    type_info: &TypeInfo,
    type_env: &TypeEnv,
    recv_name: &str,
    recv_line: usize,
    recv_col: usize,
) -> Vec<CompletionItem> {
    // Fallback 1: TypeInfo lookup heurístico (max col <= recv_col en la
    // misma línea).
    let recv_type = type_info
        .iter()
        .filter(|(key, _)| key.0 == recv_line && key.1 <= recv_col)
        .max_by_key(|(key, _)| key.1)
        .map(|(_, ty)| ty.clone());

    // Fallback 2: walk de top-level por nombre, mirar el tipo del value
    // del let con `target == recv_name`. Cubre el caso del parser
    // abandonando el stmt entero por `.` huérfano.
    let recv_type = recv_type.or_else(|| {
        program.iter().find_map(|stmt| {
            if let Stmt::Assign {
                target: crate::ast::AssignTarget::Ident(name),
                value,
                ..
            } = stmt
            {
                if name == recv_name {
                    return type_info.type_at(value.span()).cloned();
                }
            }
            None
        })
    });

    let Some(ty) = recv_type else {
        return Vec::new();
    };
    match &ty {
        Type::Nominal(id) => {
            // Fields del type. `info()` panics si el id no existe —
            // no debería pasar (el checker valida).
            let info = type_env.info(*id);
            info.fields
                .as_ref()
                .map(|fs| {
                    fs.iter()
                        .map(|f| CompletionItem {
                            label: f.name.clone(),
                            kind: Some(CompletionItemKind::FIELD),
                            detail: Some(f.type_.display(type_env)),
                            ..CompletionItem::default()
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        Type::List(t) => method_items(
            &[
                ("push", format!("fn({}) -> Null", t.display(type_env))),
                ("pop", format!("fn() -> Result<{}>", t.display(type_env))),
                ("map", format!("fn(fn({}) -> U) -> List<U>", t.display(type_env))),
                ("filter", format!(
                    "fn(fn({}) -> Bool) -> List<{}>",
                    t.display(type_env),
                    t.display(type_env)
                )),
                ("find", format!(
                    "fn(fn({}) -> Bool) -> Result<{}>",
                    t.display(type_env),
                    t.display(type_env)
                )),
                ("len", "fn() -> Int".into()),
                // Mini-tanda S.3: `sort` y `reverse` mutan in-place y
                // devuelven Null. `contains(v)` toma un T y devuelve Bool.
                ("sort", "fn() -> Null".into()),
                ("reverse", "fn() -> Null".into()),
                ("contains", format!("fn({}) -> Bool", t.display(type_env))),
            ],
        ),
        Type::Map(k, v) => method_items(
            &[
                ("get", format!(
                    "fn({}) -> Result<{}>",
                    k.display(type_env),
                    v.display(type_env)
                )),
                ("has", format!("fn({}) -> Bool", k.display(type_env))),
                ("keys", format!("fn() -> List<{}>", k.display(type_env))),
                ("values", format!("fn() -> List<{}>", v.display(type_env))),
                ("len", "fn() -> Int".into()),
            ],
        ),
        Type::Str => method_items(&[
            ("upper", "fn() -> Str".into()),
            ("lower", "fn() -> Str".into()),
            ("len", "fn() -> Int".into()),
            // Mini-tanda S.1/S.2: métodos chicos de Str. `contains` y
            // `starts_with`/`ends_with` toman un `Str` y devuelven Bool.
            // `split` devuelve List<Str>. `trim` no toma args. `replace`
            // pide dos Strs. `repeat` un Int.
            ("contains", "fn(s: Str) -> Bool".into()),
            ("starts_with", "fn(s: Str) -> Bool".into()),
            ("ends_with", "fn(s: Str) -> Bool".into()),
            ("split", "fn(sep: Str) -> List<Str>".into()),
            ("trim", "fn() -> Str".into()),
            ("replace", "fn(old: Str, new: Str) -> Str".into()),
            ("repeat", "fn(n: Int) -> Str".into()),
        ]),
        // Mini-tanda T (tuples): después de `t.` sugerimos los índices
        // de los campos como labels numéricos (`0`, `1`, ...) con el
        // tipo del elemento como detail. Estilo rust-analyzer. VSCode
        // muestra los labels en la lista; el usuario tipea el número
        // para insertarlo.
        Type::Tuple(items) => items
            .iter()
            .enumerate()
            .map(|(i, ty)| CompletionItem {
                label: i.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(ty.display(type_env)),
                ..CompletionItem::default()
            })
            .collect(),
        // Any, PyAny y resto: sin info para sugerir.
        _ => Vec::new(),
    }
}

/// Construye una lista de `CompletionItem` de tipo Method desde un
/// slice de `(nombre, firma)`.
fn method_items(items: &[(&str, String)]) -> Vec<CompletionItem> {
    items
        .iter()
        .map(|(name, detail)| CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(detail.clone()),
            ..CompletionItem::default()
        })
        .collect()
}

/// Genera completions para scope-level: walkea top-level del Program +
/// builtins + keywords. NO scope-aware (ver doc en
/// `completion_at_position`).
fn scope_level_completions(program: &Program, type_env: &TypeEnv) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Top-level del Program: let/fn/type/import.
    for stmt in program {
        match stmt {
            Stmt::Assign {
                target: crate::ast::AssignTarget::Ident(name),
                ..
            } => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    ..CompletionItem::default()
                });
            }
            Stmt::FnDef { name, .. } => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    ..CompletionItem::default()
                });
            }
            Stmt::TypeDef { name, .. } => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::CLASS),
                    ..CompletionItem::default()
                });
            }
            Stmt::Import { path, alias, .. } => {
                let label = alias.clone().or_else(|| path.last().cloned());
                if let Some(name) = label {
                    items.push(CompletionItem {
                        label: name,
                        kind: Some(CompletionItemKind::MODULE),
                        ..CompletionItem::default()
                    });
                }
            }
            Stmt::FromImport { names, .. } => {
                for (n, alias) in names {
                    let label = alias.clone().unwrap_or_else(|| n.clone());
                    items.push(CompletionItem {
                        label,
                        kind: Some(CompletionItemKind::VARIABLE),
                        ..CompletionItem::default()
                    });
                }
            }
            _ => {}
        }
    }

    // Builtins del lenguaje (matchea `register_builtins` del checker).
    for (name, detail) in [
        ("print", "fn(args...)"),
        ("len", "fn(x) -> Int"),
        ("sleep", "fn(Int) -> Future<Null>"),
        ("cors", "fn(config: Map?) -> CorsConfig"),
        ("assert", "fn(cond: Bool, msg: Str?) -> Null"),
        ("assert_eq", "fn(a, b) -> Null"),
        ("assert_ne", "fn(a, b) -> Null"),
        ("assert_throws", "fn(callback: fn() -> Any) -> Null"),
    ] {
        items.push(CompletionItem {
            label: name.into(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(detail.into()),
            ..CompletionItem::default()
        });
    }

    // Tipos built-in: visibles como nombres en posición de anotación.
    for name in [
        "Int", "Float", "Str", "Bool", "Null", "Range", "Any", "List", "Map", "Result",
        "Future", "Request", "Response", "PyAny",
    ] {
        items.push(CompletionItem {
            label: name.into(),
            kind: Some(CompletionItemKind::CLASS),
            ..CompletionItem::default()
        });
    }

    // Keywords del lenguaje. VSCode los renderiza con ícono distinto y
    // los promueve cuando el usuario tipea sus primeras letras.
    for kw in [
        "let", "fn", "if", "else", "while", "for", "loop", "match", "type", "return", "break",
        "continue", "import", "from", "as", "in", "async", "await", "and", "or", "true",
        "false", "null",
    ] {
        items.push(CompletionItem {
            label: kw.into(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..CompletionItem::default()
        });
    }

    // Nominales declarados por el usuario aparecen ya via TypeDef top-
    // level (los walkeamos arriba). Si el programa importa nominales
    // via `from foo import User`, también aparecen via FromImport.
    // No duplicamos desde `type_env.nominals` (sería redundante con
    // los emitidos arriba — y mezclaríamos con el orden de declaración
    // del Program, que es lo que el usuario probablemente quiere
    // primero).
    let _ = type_env; // silencia warning hasta que use type_env aquí.

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn err_at(line: usize, column: usize, msg: &str) -> FitzError {
        FitzError::new(ErrorKind::TypeError, line, column, msg)
    }

    #[test]
    fn error_con_posicion_mapea_a_range_0_based_de_1_caracter() {
        let errs = vec![err_at(3, 5, "tipo incompatible")];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.range.start, Position::new(2, 4)); // 1-based → 0-based
        assert_eq!(d.range.end, Position::new(2, 5));
        assert_eq!(d.message, "tipo incompatible");
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(d.source.as_deref(), Some("fitz"));
    }

    #[test]
    fn error_sin_posicion_mapea_a_range_degenerado_al_inicio() {
        let errs = vec![err_at(0, 0, "sin línea ni columna")];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert_eq!(diags[0].range.start, Position::new(0, 0));
        assert_eq!(diags[0].range.end, Position::new(0, 0));
    }

    #[test]
    fn error_con_hint_concatena_sugerencia_al_message() {
        let err = err_at(1, 1, "variable no definida")
            .with_hint("¿quisiste decir `name`?");
        let diags = fitz_errors_to_diagnostics(&[err]);
        assert!(
            diags[0].message.contains("variable no definida"),
            "message base: {}",
            diags[0].message,
        );
        assert!(
            diags[0].message.contains("Sugerencia: ¿quisiste decir `name`?"),
            "message con hint: {}",
            diags[0].message,
        );
    }

    #[test]
    fn error_sin_hint_no_agrega_la_palabra_sugerencia() {
        let errs = vec![err_at(1, 1, "tipo incompatible")];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert!(!diags[0].message.contains("Sugerencia"));
    }

    #[test]
    fn lista_vacia_devuelve_vec_vacio() {
        let diags = fitz_errors_to_diagnostics(&[]);
        assert!(diags.is_empty());
    }

    #[test]
    fn multiples_errores_preservan_orden() {
        let errs = vec![
            err_at(1, 1, "primero"),
            err_at(5, 3, "segundo"),
            err_at(0, 0, "tercero sin posición"),
        ];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert_eq!(diags.len(), 3);
        assert_eq!(diags[0].message, "primero");
        assert_eq!(diags[1].message, "segundo");
        assert_eq!(diags[2].message, "tercero sin posición");
    }

    // Tests sobre `check_source` — pipeline LSP-style entero.

    #[test]
    fn check_source_programa_valido_no_emite_errores() {
        let src = "let x = 1\nlet y = 2\nprint(x + y)";
        let errs = check_source(src);
        assert!(errs.is_empty(), "errores inesperados: {errs:?}");
    }

    #[test]
    fn check_source_error_de_tipo_sale_del_checker() {
        let src = "let x: Int = \"texto\"";
        let errs = check_source(src);
        assert!(!errs.is_empty(), "checker debería rechazar Int = Str");
        assert!(
            errs.iter().any(|e| matches!(e.kind, ErrorKind::TypeError)),
            "esperaba al menos un TypeError: {errs:?}",
        );
    }

    #[test]
    fn check_source_recovery_no_aborta_ante_stmts_rotos() {
        // El parser con recovery debería darnos AST parcial + errores;
        // el checker chequea lo que recuperó. Sin recovery, la pipeline
        // abortaría en el primer error. El smoke acá es que `check_source`
        // devuelve algo (no panic) sobre una entrada con sintaxis rota.
        let src = "let x = ???\nlet y = 1\nlet z: Int = \"mal\"";
        let errs = check_source(src);
        assert!(
            !errs.is_empty(),
            "debería haber al menos un error de parser",
        );
    }

    // Tests sobre `check_source_with_types` — variante para hover (Fase
    // 9.x.2). Lo nuevo respecto a `check_source` es que retiene el
    // `TypeInfo` poblado por F16.

    #[test]
    fn check_source_with_types_programa_valido_devuelve_type_info_no_vacio() {
        let src = "let x = 42\nlet y = x + 1";
        let (_program, _env, type_info, _defs, errors) = check_source_with_types(src);
        assert!(errors.is_empty(), "errores inesperados: {errors:?}");
        assert!(
            !type_info.is_empty(),
            "TypeInfo no debería estar vacío sobre un programa con Exprs",
        );
    }

    #[test]
    fn check_source_with_types_error_lexer_devuelve_type_info_vacio() {
        // String sin cerrar — lexer aborta antes del parser/checker,
        // entonces el `TypeInfo` no se puede poblar.
        let src = "let x = \"sin cerrar";
        let (_program, _env, type_info, _defs, errors) = check_source_with_types(src);
        assert!(!errors.is_empty(), "lexer debería rechazar string sin cerrar");
        assert!(
            type_info.is_empty(),
            "TypeInfo debería estar vacío si la pipeline aborta en el lexer",
        );
    }

    #[test]
    fn check_source_with_types_error_de_tipo_no_borra_type_info() {
        // El checker chequea lo que pudo aún cuando hay errores: los
        // Exprs válidos quedan en TypeInfo, los inválidos también con
        // el tipo "best-effort".
        let src = "let x = 42\nlet y: Int = \"mal\"";
        let (_program, _env, type_info, _defs, errors) = check_source_with_types(src);
        assert!(!errors.is_empty(), "debería haber un TypeError");
        assert!(
            !type_info.is_empty(),
            "TypeInfo debería retener tipos de los Exprs válidos pese al error",
        );
    }

    // Tests sobre `hover_for_position` y `make_hover` (Fase 9.x.2.b).

    #[test]
    fn hover_for_position_devuelve_tipo_en_posicion_exacta_de_literal() {
        // `let x = 42` — el literal `42` empieza en col 9 (1-based),
        // que es col 8 LSP (0-based). El cursor en (line=0, char=8)
        // debería matchear el Int.
        let src = "let x = 42";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 0, 8);
        assert!(matches!(ty, Some(Type::Int)), "esperaba Int, dio {ty:?}");
    }

    #[test]
    fn hover_for_position_devuelve_tipo_en_medio_de_un_ident_usado_como_expr() {
        // El lado izquierdo de un `let` es un AssignTarget, no un Expr
        // — esos idents NO entran a TypeInfo. Para testear el caso del
        // cursor "en medio de un identificador" necesitamos que el
        // ident sea Expr (uso, no declaración):
        //
        //   let nombre = 42         (línea 0)
        //   let x = nombre + 1      (línea 1)
        //
        // `nombre` en línea 1 empieza en col 9 (1-based) = col 8 (0-based).
        // Cursor a la mitad (línea 1, col 11 LSP / col 12 Fitz) cae
        // dentro del Ident. La heurística "max col <= cursor en la
        // misma línea" debe devolver Some(_) (el tipo del Ident o el
        // del BinOp que comparte span — ambos Int).
        let src = "let nombre = 42\nlet x = nombre + 1";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 1, 11);
        assert!(matches!(ty, Some(Type::Int)), "esperaba Int, dio {ty:?}");
    }

    #[test]
    fn hover_for_position_linea_sin_spans_devuelve_none() {
        // Programa de una línea; cursor en línea 5 → no hay spans.
        let src = "let x = 1";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 5, 0);
        assert!(ty.is_none(), "esperaba None en línea sin spans, dio {ty:?}");
    }

    #[test]
    fn hover_for_position_cursor_antes_del_primer_token_devuelve_none() {
        // `   let x = 1` — cursor en col 0 está antes de cualquier
        // Expr (el primer Expr es `1` en col 13 (1-based)).
        let src = "   let x = 1";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 0, 0);
        assert!(ty.is_none(), "esperaba None antes del primer token, dio {ty:?}");
    }

    #[test]
    fn hover_for_position_dos_lineas_no_cruza_la_linea() {
        // Aseguramos que la heurística no se "escapa" a la línea
        // anterior cuando la línea del cursor está vacía de spans.
        let src = "let x = 42\n   ";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 1, 0);
        assert!(ty.is_none(), "no debería cruzar líneas, dio {ty:?}");
    }

    #[test]
    fn make_hover_emite_markdown_con_bloque_fitz() {
        let env = TypeEnv::default();
        let hover = make_hover(&Type::Int, &env);
        match &hover.contents {
            HoverContents::Markup(MarkupContent { kind, value }) => {
                assert_eq!(*kind, MarkupKind::Markdown);
                assert_eq!(value, "```fitz\nInt\n```");
            }
            other => panic!("esperaba Markup, dio {other:?}"),
        }
        assert!(hover.range.is_none(), "range debe ser None hasta end_span");
    }

    #[test]
    fn make_hover_formatea_tipos_compuestos_con_display() {
        let env = TypeEnv::default();
        let list_int = Type::List(Box::new(Type::Int));
        let hover = make_hover(&list_int, &env);
        if let HoverContents::Markup(MarkupContent { value, .. }) = &hover.contents {
            assert_eq!(value, "```fitz\nList<Int>\n```");
        } else {
            panic!("esperaba Markup");
        }
    }

    #[test]
    fn hover_end_to_end_pipeline_devuelve_int_para_un_literal() {
        // Smoke combinado: pipeline + hover sobre el literal `42`.
        let src = "let x = 42";
        let (_program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 0, 8).expect("debería matchear");
        let hover = make_hover(ty, &env);
        if let HoverContents::Markup(MarkupContent { value, .. }) = &hover.contents {
            assert_eq!(value, "```fitz\nInt\n```");
        } else {
            panic!("esperaba Markup");
        }
    }

    #[test]
    fn check_source_y_with_types_devuelven_la_misma_lista_de_errores() {
        // Sanity check: ambas APIs comparten pipeline, los errores
        // deberían ser equivalentes (mismo orden, misma cantidad,
        // mismos mensajes).
        let src = "let x: Int = \"mal\"\nlet y: Str = 42";
        let errs_solo = check_source(src);
        let (_program, _env, _type_info, _defs, errs_with) = check_source_with_types(src);
        assert_eq!(errs_solo.len(), errs_with.len());
        for (a, b) in errs_solo.iter().zip(errs_with.iter()) {
            assert_eq!(a.message, b.message);
            assert_eq!(a.line, b.line);
            assert_eq!(a.column, b.column);
        }
    }

    // Tests sobre `definition_for_position` y `make_definition_location`
    // (Fase 9.x.3.b).

    #[test]
    fn definition_for_position_devuelve_span_de_declaracion_de_var_local() {
        // `let x = 1` en línea 0, `let y = x` en línea 1. El uso de
        // `x` está en línea 1, col 8 (0-based) — el `def_span`
        // devuelto debe ser de línea 1 (1-based, el Stmt::Assign de `x`).
        let src = "let x = 1\nlet y = x\n";
        let (_program, _env, _type_info, def_info, _errs) = check_source_with_types(src);
        let def_span = definition_for_position(&def_info, 1, 8)
            .expect("uso de x debe resolver");
        assert_eq!(def_span.line, 1, "def en línea 1 (1-based)");
    }

    #[test]
    fn definition_for_position_linea_sin_idents_devuelve_none() {
        let src = "let x = 1\n";
        let (_program, _env, _type_info, def_info, _errs) = check_source_with_types(src);
        assert!(definition_for_position(&def_info, 5, 0).is_none());
    }

    #[test]
    fn definition_for_position_no_resuelve_uso_de_builtin() {
        // `print(42)` — `print` es builtin con def_span Span::ZERO.
        // No debe aparecer en DefinitionInfo (filtrado por política),
        // así que el lookup devuelve None.
        let src = "print(42)\n";
        let (_program, _env, _type_info, def_info, _errs) = check_source_with_types(src);
        // Cursor sobre `print` (línea 0, col 0).
        assert!(definition_for_position(&def_info, 0, 0).is_none());
    }

    #[test]
    fn make_definition_location_convierte_1_based_a_0_based() {
        let uri = Url::parse("file:///test.fitz").unwrap();
        // def_span en línea 3, col 5 (1-based) → LSP línea 2, col 4 (0-based).
        let loc = make_definition_location(uri.clone(), Span::new(3, 5));
        assert_eq!(loc.uri, uri);
        assert_eq!(loc.range.start, Position::new(2, 4));
        assert_eq!(loc.range.end, Position::new(2, 5));
    }

    #[test]
    fn definition_end_to_end_pipeline_devuelve_location_de_def() {
        // Smoke combinado: pipeline + definition_for_position +
        // make_definition_location.
        let src = "let x = 1\nlet y = x\n";
        let (_program, _env, _type_info, def_info, _errs) = check_source_with_types(src);
        let def_span = definition_for_position(&def_info, 1, 8).expect("matchea");
        let uri = Url::parse("file:///t.fitz").unwrap();
        let loc = make_definition_location(uri, def_span);
        // El Stmt::Assign de `x` está en línea 1 (1-based) → línea 0
        // (0-based). Su columna depende del parser; asumimos col 1
        // (1-based, primer caracter de `let`).
        assert_eq!(loc.range.start.line, 0);
    }

    // Tests sobre `completion_at_position` y helpers privados
    // (Fase 9.x.4.a). Cubren detección de contexto (after-dot vs
    // scope-level), conversión de offset, y los dos paths de
    // completions.

    #[test]
    fn position_to_offset_y_back_son_inversas() {
        // Sanity: el inverso compuesto recupera la posición.
        let text = "abc\nde\nfghi";
        for (line, ch) in [(0, 0), (0, 2), (1, 0), (1, 1), (2, 3)] {
            let off = position_to_offset(text, line, ch).unwrap();
            let (l, c) = offset_to_position(text, off);
            assert_eq!((l, c), (line, ch), "round-trip falla en ({line},{ch})");
        }
    }

    #[test]
    fn detect_context_scope_level_en_documento_vacio() {
        let ctx = detect_completion_context("", 0, 0).unwrap();
        assert_eq!(ctx, CompletionContext::ScopeLevel);
    }

    #[test]
    fn detect_context_after_dot_tras_ident_y_punto() {
        // `obj.` con cursor justo después del `.`.
        let text = "obj.";
        let ctx = detect_completion_context(text, 0, 4).unwrap();
        match ctx {
            CompletionContext::AfterDot { recv_name, recv_line, recv_col } => {
                // Receiver `obj` empieza en line 1, col 1 (Fitz 1-based).
                assert_eq!(recv_name, "obj");
                assert_eq!(recv_line, 1);
                assert_eq!(recv_col, 1);
            }
            other => panic!("esperaba AfterDot, dio {other:?}"),
        }
    }

    #[test]
    fn detect_context_after_dot_con_prefix_partial() {
        // `obj.fo` con cursor al final → el usuario ya tipeó "fo" del
        // método. El context sigue siendo AfterDot; VSCode filtra por
        // el prefix client-side.
        let text = "obj.fo";
        let ctx = detect_completion_context(text, 0, 6).unwrap();
        assert!(matches!(ctx, CompletionContext::AfterDot { .. }));
    }

    #[test]
    fn detect_context_scope_level_en_medio_de_ident() {
        // `obj` sin `.` adelante → scope-level. Cursor en mitad del
        // ident; el prefix ya tipeado lo filtra VSCode.
        let text = "obj";
        let ctx = detect_completion_context(text, 0, 3).unwrap();
        assert_eq!(ctx, CompletionContext::ScopeLevel);
    }

    #[test]
    fn scope_level_completion_incluye_top_level_y_builtins_y_keywords() {
        // Cursor en línea 3 col 0 — fuera de cualquier stmt declarado,
        // contexto scope-level.
        let src = "let x = 1\nfn foo() => 0\ntype Bar { id: Int }\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 3, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Top-level (let, fn, type).
        assert!(labels.contains(&"x"), "falta var top-level `x`: {labels:?}");
        assert!(labels.contains(&"foo"), "falta fn `foo`: {labels:?}");
        assert!(labels.contains(&"Bar"), "falta type `Bar`: {labels:?}");
        // Builtins.
        assert!(labels.contains(&"print"), "falta builtin `print`");
        assert!(labels.contains(&"len"));
        // Tipos built-in.
        assert!(labels.contains(&"Int"));
        assert!(labels.contains(&"List"));
        // Keywords.
        assert!(labels.contains(&"let"));
        assert!(labels.contains(&"match"));
    }

    #[test]
    fn after_dot_sobre_nominal_lista_fields_del_type() {
        // `type Point { x: Int, y: Int }` + `let p = Point { x: 1, y: 2 }`
        // + ident `p` en línea 2 col 0 (1-based: line 3, col 1).
        // After-dot sobre `p.` debería listar x, y.
        let src = "type Point { x: Int, y: Int }\nlet p = Point { x: 1, y: 2 }\np.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor en línea 2, col 2 (0-based LSP), justo después del `.`.
        let items = completion_at_position(src, &program, &type_info, &env, 2, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"x"), "falta field `x`: {labels:?}");
        assert!(labels.contains(&"y"), "falta field `y`: {labels:?}");
        // No debe incluir top-level: ya estamos en after-dot.
        assert!(!labels.contains(&"print"), "no debería incluir builtins en after-dot");
        // El kind debe ser FIELD.
        let item_x = items.iter().find(|i| i.label == "x").unwrap();
        assert_eq!(item_x.kind, Some(CompletionItemKind::FIELD));
    }

    #[test]
    fn after_dot_sobre_list_lista_metodos_built_in() {
        // `let xs = [1, 2, 3]` + `xs.` en línea 1.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["push", "pop", "map", "filter", "find", "len"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` de List: {labels:?}"
            );
        }
        let item_map = items.iter().find(|i| i.label == "map").unwrap();
        assert_eq!(item_map.kind, Some(CompletionItemKind::METHOD));
    }

    #[test]
    fn after_dot_sobre_str_lista_3_metodos() {
        // Caso del usuario tipeando `obj.` al final del buffer: el
        // parser abandona el stmt entero por el `.` huérfano (deuda
        // F15 recovery sub-stmt), el Expr::Ident no llega a TypeInfo.
        // El fallback "walk top-level por nombre" resuelve el tipo
        // mirando el `let s = "hola"` previo.
        let src = "let s = \"hola\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"upper"));
        assert!(labels.contains(&"lower"));
        assert!(labels.contains(&"len"));
        // Sin métodos de List.
        assert!(!labels.contains(&"push"));
    }

    #[test]
    fn after_dot_sobre_receiver_sin_tipo_devuelve_vacio() {
        // `desconocido.` — ident no resuelto → TypeInfo no tiene
        // entry → lista vacía.
        let src = "desconocido.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 0, 12);
        assert!(items.is_empty(), "esperaba vacío, dio {items:?}");
    }

    // Mini-tanda V.2 (VSCode catch-up) — los métodos nuevos de Str
    // (S.1/S.2), de List (S.3), y el tuple field access (T.1).

    #[test]
    fn after_dot_sobre_str_incluye_metodos_de_mini_tanda_s() {
        // Los 7 métodos nuevos sumados en S.1/S.2 deben aparecer en la
        // lista de completion para receptores `Str`.
        let src = "let s = \"hola\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in [
            "contains",
            "starts_with",
            "ends_with",
            "split",
            "trim",
            "replace",
            "repeat",
        ] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda S) en Str: {labels:?}"
            );
        }
        // Sanity: los 3 originales siguen.
        assert!(labels.contains(&"upper"));
        assert!(labels.contains(&"lower"));
        assert!(labels.contains(&"len"));
    }

    #[test]
    fn after_dot_sobre_list_incluye_sort_reverse_y_contains() {
        // Mini-tanda S.3: sort, reverse, contains se suman a la lista
        // canónica de métodos de List.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["sort", "reverse", "contains"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda S.3) en List: {labels:?}"
            );
        }
        // El detail de `contains` debe reflejar el tipo del elemento.
        let item_contains = items.iter().find(|i| i.label == "contains").unwrap();
        assert_eq!(item_contains.detail.as_deref(), Some("fn(Int) -> Bool"));
    }

    #[test]
    fn after_dot_sobre_tuple_lista_indices_numericos_con_tipo() {
        // Mini-tanda T.1: después de `t.` sugerimos `0`, `1`, ...
        // como labels, con el tipo del campo en `detail`.
        let src = "let t = (1, \"x\", true)\nt.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["0", "1", "2"], "esperaba labels 0/1/2, dio {labels:?}");
        // Cada item es FIELD con detail = tipo del elemento.
        let it0 = &items[0];
        let it1 = &items[1];
        let it2 = &items[2];
        assert_eq!(it0.kind, Some(CompletionItemKind::FIELD));
        assert_eq!(it0.detail.as_deref(), Some("Int"));
        assert_eq!(it1.detail.as_deref(), Some("Str"));
        assert_eq!(it2.detail.as_deref(), Some("Bool"));
    }
}
