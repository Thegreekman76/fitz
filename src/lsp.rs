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
    let (_program, _env, _type_info, _def_info, errors) = check_source_with_types(source);
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
    errors
        .iter()
        .map(|e| error_to_diagnostic(e, None))
        .collect()
}

/// Mini-tanda LSPy — variante con source text que computa Range exacto
/// para errores cuya posición coincide con un identificador. Usado
/// por el bin del LSP que tiene el doc text. La signature pública
/// vieja se mantiene como wrapper.
pub fn fitz_errors_to_diagnostics_with_source(
    errors: &[FitzError],
    source: &str,
) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(|e| error_to_diagnostic(e, Some(source)))
        .collect()
}

fn error_to_diagnostic(err: &FitzError, source: Option<&str>) -> Diagnostic {
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
        let line = (err.line.saturating_sub(1)) as u32;
        let col = (err.column.saturating_sub(1)) as u32;
        // Mini-tanda LSPy — si tenemos source y la posición cae sobre
        // un ident, expandir el range al ident completo. Sino fallback
        // a 1 char.
        let fallback = Range {
            start: Position::new(line, col),
            end: Position::new(line, col + 1),
        };
        match source {
            Some(text) => {
                ident_range_at_position(text, line as usize, col as usize).unwrap_or(fallback)
            }
            None => fallback,
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
pub fn hover_for_position(type_info: &TypeInfo, line: u32, character: u32) -> Option<&Type> {
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
/// Variante legacy sin Range. Mantenida para compatibilidad con call
/// sites pre-LSPy. `make_hover_with_range` la reemplaza con el Range
/// computado del ident bajo el cursor.
pub fn make_hover(ty: &Type, env: &TypeEnv) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```fitz\n{}\n```", ty.display(env)),
        }),
        range: None,
    }
}

/// Mini-tanda LSPy — versión con Range computado del símbolo bajo el
/// cursor. El range cubre exactamente el identificador, así VSCode
/// resalta el token en lugar de solo mostrar el tooltip. Si no hay
/// un ident en la posición del cursor, `range = None` (fallback).
///
/// v0.10.32 (Tier D.2) — si el tipo es un `Type::Nominal(id)` con
/// metadata `@table`, agregamos el `CREATE TABLE` SQL emitted al
/// markdown (debajo del display del tipo). Útil para debuggear el
/// shape SQL sin abrir `fitz db diff` o revisar la migration manual.
pub fn make_hover_with_range(
    ty: &Type,
    env: &TypeEnv,
    program: &crate::ast::Program,
    text: &str,
    line: u32,
    character: u32,
) -> Hover {
    let range = ident_range_at_position(text, line as usize, character as usize);
    let mut value = format!("```fitz\n{}\n```", ty.display(env));
    // v0.10.32 (Tier D.2) — append CREATE TABLE SQL si aplica.
    if let Some(sql) = try_table_create_sql(ty, env, program) {
        value.push_str(
            "\n\n---\n\n**`CREATE TABLE` emitted** (vía `fitz db diff/migrate`):\n\n```sql\n",
        );
        value.push_str(&sql);
        value.push_str("\n```");
    }
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range,
    }
}

/// v0.10.32 (Tier D.2) — si `ty` es un `Type::Nominal(id)` con
/// `TableMetadata` registrada, construye el SQL `CREATE TABLE` que
/// `fitz db diff/migrate` emitiría para ese type. Devuelve `None`
/// para tipos que no son `@table` o si la construcción del schema
/// falla (typo en `@belongs_to`, etc.).
///
/// Reusa `migrations::schema_from_program` + `migrations::create_table_sql_for`
/// para producir SQL idéntico al de `fitz db diff` — sin divergencia
/// entre lo que el LSP muestra y lo que el migrator emite.
fn try_table_create_sql(ty: &Type, env: &TypeEnv, program: &crate::ast::Program) -> Option<String> {
    let type_id = match ty {
        Type::Nominal(id) => *id,
        _ => return None,
    };
    // Verificamos que sea @table (sino no tiene SQL para mostrar).
    let table_meta = env.table_metadata(type_id)?;
    let target_sql_name = table_meta.sql_name.clone();
    let target_schema = table_meta.schema.clone();
    // Construir el Schema entero. Si falla (typo en relations, etc.),
    // skipear el augment — devolver None deja el hover con solo el
    // tipo. El user ve los errores del checker en otro lado.
    let schema = crate::migrations::schema_from_program(program, env).ok()?;
    // Buscar la table matching y emitir el CREATE.
    let table = schema
        .tables
        .iter()
        .find(|t| t.name == target_sql_name && t.schema == target_schema)?;
    Some(crate::migrations::create_table_sql_for(table))
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

/// Variante legacy: range de 1 caracter (sin contexto del source).
/// `make_definition_location_with_source` la reemplaza cuando hay
/// source text disponible para computar el end exacto.
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

/// Mini-tanda LSPy — variante con Range exacto. Si `source` está
/// disponible y el `def_span` apunta a un ident, computamos el end
/// leyendo el ident desde la línea del source. Sino, fallback al
/// range de 1 char. Cubre cross-module: el caller pasa el source
/// del archivo target (no del documento abierto).
pub fn make_definition_location_with_source(
    uri: Url,
    def_span: Span,
    source: Option<&str>,
) -> Location {
    let line0 = (def_span.line.saturating_sub(1)) as u32;
    let col0 = (def_span.column.saturating_sub(1)) as u32;
    let range = match source {
        Some(text) => ident_range_from_def(text, def_span).unwrap_or_else(|| Range {
            start: Position::new(line0, col0),
            end: Position::new(line0, col0 + 1),
        }),
        None => Range {
            start: Position::new(line0, col0),
            end: Position::new(line0, col0 + 1),
        },
    };
    Location { uri, range }
}

/// Mini-tanda LSPy — extrae el `Range` LSP del identificador que
/// arranca en la posición `def_span` del source. Lee la línea, busca
/// el primer run de chars ident (alphanum + `_`) que empiece en/cerca
/// de la columna, y devuelve su rango 0-based. Devuelve None si no
/// hay un ident en esa posición (typedef/let/fn keyword en el span,
/// o span de stmt con un Stmt::Expr arbitrario adentro).
fn ident_range_from_def(source: &str, def_span: Span) -> Option<Range> {
    let line_idx = def_span.line.saturating_sub(1);
    let col_idx = def_span.column.saturating_sub(1);
    let line = source.lines().nth(line_idx)?;
    let chars: Vec<char> = line.chars().collect();
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';

    // El def_span puede apuntar al keyword (`let`, `fn`, `type`) o al
    // ident en sí. Buscamos el primer ident a partir de col_idx,
    // skipeando keywords + whitespace.
    let mut i = col_idx.min(chars.len());
    // Skip keyword tokens (`let`, `fn`, `type`, `static`, `async`).
    for kw in ["let ", "fn ", "type ", "static ", "async fn ", "async "] {
        let kw_chars: Vec<char> = kw.chars().collect();
        if chars.len().saturating_sub(i) >= kw_chars.len()
            && chars[i..i + kw_chars.len()] == kw_chars[..]
        {
            i += kw_chars.len();
            break;
        }
    }
    // Skip whitespace.
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    // Encontrar el run de ident.
    if i >= chars.len() || !is_ident_char(chars[i]) {
        return None;
    }
    let start = i;
    while i < chars.len() && is_ident_char(chars[i]) {
        i += 1;
    }
    let end = i;
    if start == end {
        return None;
    }
    Some(Range {
        start: Position::new(line_idx as u32, start as u32),
        end: Position::new(line_idx as u32, end as u32),
    })
}

/// Mini-tanda LSPy — Range LSP del identificador BAJO el cursor (no
/// "starting at" como `ident_range_from_def`). Para hover: el cursor
/// puede estar en medio de un ident; queremos el rango completo del
/// ident, no del que arranca en la columna del cursor.
fn ident_range_at_position(source: &str, line_idx: usize, char_idx: usize) -> Option<Range> {
    let line = source.lines().nth(line_idx)?;
    let chars: Vec<char> = line.chars().collect();
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';
    let cursor = char_idx.min(chars.len());

    let mut start = cursor;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = cursor;
    while end < chars.len() && is_ident_char(chars[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    Some(Range {
        start: Position::new(line_idx as u32, start as u32),
        end: Position::new(line_idx as u32, end as u32),
    })
}

/// Mini-tanda LSPx — cross-module go-to-definition. Si `def_span`
/// apunta a un `Stmt::Import` o `Stmt::FromImport` del `program`,
/// resuelve el archivo target, parsea, y busca la declaración real
/// del símbolo. Devuelve `(Url del módulo target, Span de la decl
/// adentro del módulo)`. Si la resolución falla (path no existe,
/// símbolo no encontrado, etc.), devuelve `None` — el caller usa el
/// Location local como fallback.
///
/// `doc_uri` es el URI del documento abierto; lo usamos solo para
/// resolver el `base_dir` desde el que se buscan los imports
/// relativos. El URI del result es el del módulo target.
///
/// `target_name` es el ident bajo el cursor en el momento del
/// goto-def. Para `import foo` el name puede ser `foo` (namespace);
/// para `from foo import X, Y` debe ser `X` o `Y`.
pub fn resolve_cross_module_definition(
    program: &Program,
    doc_uri: &Url,
    target_span: Span,
    target_name: &str,
) -> Option<(Url, Span)> {
    // Solo tiene sentido si el doc es un `file://`.
    let doc_path = doc_uri.to_file_path().ok()?;
    let base_dir = doc_path.parent()?;

    // Buscar el Stmt::Import / Stmt::FromImport cuyo span coincide.
    // `module_path` = segments del path importado.
    // `target_item` = nombre del símbolo a buscar en el módulo target,
    //                 o None si es un `import` namespace (apunta al
    //                 top del módulo).
    let (module_path, target_item): (Vec<String>, Option<String>) = {
        let mut found: Option<(Vec<String>, Option<String>)> = None;
        for stmt in program {
            match stmt {
                Stmt::Import { path, span, .. } if *span == target_span => {
                    let last = path.last()?;
                    if target_name == last {
                        found = Some((path.clone(), None));
                    }
                    break;
                }
                Stmt::FromImport {
                    path, names, span, ..
                } if *span == target_span => {
                    let item = names.iter().find_map(|(n, alias)| {
                        if alias.as_deref() == Some(target_name) || n == target_name {
                            Some(n.clone())
                        } else {
                            None
                        }
                    });
                    if let Some(it) = item {
                        found = Some((path.clone(), Some(it)));
                    }
                    break;
                }
                _ => {}
            }
        }
        found?
    };

    // Resolver el path a un archivo `.fitz` real. Convención del
    // loader: `path = ["foo", "bar"]` → `<base>/foo/bar.fitz`.
    let mut target_path = base_dir.to_path_buf();
    if module_path.is_empty() {
        return None;
    }
    for (i, comp) in module_path.iter().enumerate() {
        if i + 1 == module_path.len() {
            target_path.push(format!("{}.fitz", comp));
        } else {
            target_path.push(comp);
        }
    }
    let target_path = target_path.canonicalize().ok()?;

    // Parsear el archivo target y buscar la declaración.
    let source = std::fs::read_to_string(&target_path).ok()?;
    let tokens = tokenize(&source).ok()?;
    let (target_program, _errs) = parse_with_recovery(tokens);

    let target_decl_span = match target_item {
        Some(item) => find_top_level_decl(&target_program, &item)?,
        // Import namespace: apuntar al primer stmt del módulo (top).
        None => target_program.first().map(|s| s.span())?,
    };

    let target_uri = Url::from_file_path(&target_path).ok()?;
    Some((target_uri, target_decl_span))
}

/// Busca una declaración top-level con el nombre dado en el AST de un
/// módulo. Cubre `Stmt::FnDef`, `Stmt::TypeDef`, y `Stmt::Assign` con
/// target Ident (consts del módulo). Devuelve el span de la declaración
/// para go-to-def cross-module.
fn find_top_level_decl(program: &Program, name: &str) -> Option<Span> {
    use crate::ast::AssignTarget;
    for stmt in program {
        match stmt {
            Stmt::FnDef { name: n, span, .. } if n == name => return Some(*span),
            Stmt::TypeDef { name: n, span, .. } if n == name => return Some(*span),
            Stmt::Assign {
                target: AssignTarget::Ident(n),
                span,
                ..
            } if n == name => {
                return Some(*span);
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Mini-tanda LSPz (v0.9.47) — Completion en `from <mod> import |`
// ---------------------------------------------------------------------------

/// Enumera los símbolos exportables (fns, tipos, consts) del módulo
/// identificado por `mod_path`, relativos al `doc_uri`. Convención del
/// loader: `mod_path = ["foo"]` → `<base>/foo.fitz`; `["sub", "utils"]`
/// → `<base>/sub/utils.fitz` (1 dir nesting). Devuelve lista vacía si
/// el archivo no existe o no parsea.
pub fn from_import_completions(doc_uri: &Url, mod_path: &[String]) -> Vec<CompletionItem> {
    use crate::ast::AssignTarget;
    if mod_path.is_empty() {
        return Vec::new();
    }
    let doc_path = match doc_uri.to_file_path() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let Some(base_dir) = doc_path.parent() else {
        return Vec::new();
    };
    let mut target_path = base_dir.to_path_buf();
    for (i, comp) in mod_path.iter().enumerate() {
        if i + 1 == mod_path.len() {
            target_path.push(format!("{}.fitz", comp));
        } else {
            target_path.push(comp);
        }
    }
    let source = match std::fs::read_to_string(&target_path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let tokens = match tokenize(&source) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let (program, _errs) = parse_with_recovery(tokens);
    let mut items: Vec<CompletionItem> = Vec::new();
    for stmt in &program {
        match stmt {
            Stmt::FnDef {
                name,
                params,
                return_type,
                is_async,
                ..
            } => {
                let params_str = params
                    .iter()
                    .map(|p| {
                        let ty = p
                            .type_
                            .as_ref()
                            .map(|t| t.display_name())
                            .unwrap_or_else(|| "Any".into());
                        format!("{}: {}", p.name, ty)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let ret_str = return_type
                    .as_ref()
                    .map(|t| t.display_name())
                    .unwrap_or_else(|| "Any".into());
                let prefix = if *is_async { "async fn" } else { "fn" };
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(format!("{}({}) -> {}", prefix, params_str, ret_str)),
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
            Stmt::Assign {
                target: AssignTarget::Ident(name),
                ..
            } => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::CONSTANT),
                    ..CompletionItem::default()
                });
            }
            _ => {}
        }
    }
    items
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
    /// Mini-tanda LSPz (v0.9.47) — `from <mod> import |` o
    /// `from <mod> import X, |` — el cursor está dentro de la lista
    /// de imports de un `from`. Listamos los símbolos exportables del
    /// módulo target. `mod_path` son los segmentos del módulo
    /// (`["foo"]` o `["sub", "utils"]`).
    FromImportList { mod_path: Vec<String> },
    /// v0.10.12 — `@` o `@<prefix>` — el cursor está tipeando un
    /// decorator (después del `@`, antes del `(` o de un newline).
    /// Listamos la lista cerrada de decorators del lenguaje con
    /// snippets útiles. VSCode filtra client-side por `<prefix>`.
    /// Cubre los 4 grupos:
    ///   - HTTP routing: `@get`/`@post`/`@put`/`@delete`/`@server`/`@header`
    ///   - Middleware/CORS: `@middleware`/`@cors`
    ///   - Auth: `@authenticated`/`@admin`/`@auth_provider`
    ///   - WS + Jobs: `@ws`/`@cron`/`@background`/`@test`
    ///   - ORM: `@table`/`@primary`/`@column`/`@unique`/`@index`/
    ///     `@db_default`/`@hidden`/`@belongs_to`/`@has_one`/`@has_many`/
    ///     `@renamed_from` (v0.10.17)
    AfterAt,
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
    completion_at_position_with_uri(text, program, type_info, type_env, line, character, None)
}

/// Mini-tanda LSPz (v0.9.47) — variante con `doc_uri` opcional para
/// resolver el archivo del módulo target en el contexto
/// `from <mod> import |`. El backend del bin (`fitz-lsp.rs`) la usa
/// para pasar el URI del documento abierto; el resto de los
/// consumidores (tests existentes, herramientas externas) pueden
/// usar `completion_at_position` directamente — con `doc_uri = None`,
/// el contexto `FromImportList` devuelve lista vacía (sin URI no
/// podemos resolver el archivo del módulo).
pub fn completion_at_position_with_uri(
    text: &str,
    program: &Program,
    type_info: &TypeInfo,
    type_env: &TypeEnv,
    line: u32,
    character: u32,
    doc_uri: Option<&Url>,
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
        } => after_dot_completions(
            program, type_info, type_env, &recv_name, recv_line, recv_col,
        ),
        CompletionContext::FromImportList { mod_path } => match doc_uri {
            Some(uri) => from_import_completions(uri, &mod_path),
            None => Vec::new(),
        },
        CompletionContext::AfterAt => decorator_completions(),
        CompletionContext::ScopeLevel => {
            // Mini-tanda LSPy.4 — pasar la línea del cursor (1-based)
            // para incluir vars locales/params del scope contenedor.
            let cursor_line_fitz = (line as usize) + 1;
            scope_level_completions(program, type_env, cursor_line_fitz)
        }
    }
}

/// Walkea hacia atrás del cursor en el texto. Si encuentra
/// `<ident>.<partial_prefix?>` devuelve `AfterDot` con la posición del
/// inicio del receiver. Si no, `ScopeLevel`. Devuelve `None` si la
/// posición no es válida (más allá del fin del texto).
fn detect_completion_context(text: &str, line: u32, character: u32) -> Option<CompletionContext> {
    let offset = position_to_offset(text, line, character)?;
    let bytes = text.as_bytes();
    // Saltar el prefix que el usuario ya tipeó (chars de identificador
    // antes del cursor).
    let mut i = offset;
    while i > 0 && is_ident_continue(bytes[i - 1]) {
        i -= 1;
    }
    // v0.10.12 — Si justo antes del prefix hay un `@`, contexto
    // AfterAt (tipeando nombre de decorator). Cubre tanto `@|`
    // (cursor inmediato al `@`, prefix vacío) como `@get|` (prefix
    // "get"). VSCode filtra client-side por el prefix tipeado, así
    // que devolvemos siempre la lista completa de decorators.
    //
    // Tiene prioridad sobre after-dot: el char `@` no puede formar
    // parte de un ident chain (`a.b.c`), así que el después del `@`
    // SIEMPRE es nombre de decorator.
    if i > 0 && bytes[i - 1] == b'@' {
        return Some(CompletionContext::AfterAt);
    }
    // Si justo antes hay un `.`, contexto after-dot.
    if i > 0 && bytes[i - 1] == b'.' {
        let dot_pos = i - 1;
        let mut j = dot_pos;
        // v0.9.47 — chain a.b.c.: walkea back-to-front capturando
        // `<ident>(.<ident>)*` para soportar receivers compuestos.
        // Fase 10 deuda QB — extendido para soportar parens balanceadas
        // dentro de chains: `User.where(fn(u) => true).` captura el
        // chain entero saltando `(...)` cuando aparezcan en el camino.
        // El recv_name resultante NO incluye las parens (capturamos solo
        // los segmentos `<ident>(.<ident>)*` outermost); el lookup de
        // tipo via TypeInfo se hace por la posición del START del
        // primer ident, así que el matching funciona si el TypeInfo
        // tiene un Expr registrado en esa posición.
        while j > 0 {
            let c = bytes[j - 1];
            if is_ident_continue(c) || c == b'.' {
                j -= 1;
            } else if c == b')' {
                // Balanced paren skip — scan back hasta el `(` que matchea.
                let mut depth = 1;
                let mut k = j - 1;
                while k > 0 && depth > 0 {
                    k -= 1;
                    match bytes[k] {
                        b')' => depth += 1,
                        b'(' => depth -= 1,
                        _ => {}
                    }
                }
                if depth != 0 {
                    // No balanceado — abortamos chain walk.
                    break;
                }
                j = k;
            } else {
                break;
            }
        }
        // Validar shape: el receiver no debe empezar con `.` ni tener
        // `..` consecutivos. Si pasa, devolvemos `None` para chain
        // (cae a ScopeLevel).
        if j < dot_pos {
            // Para el recv_name, agarramos la parte ANTES del primer `(`
            // (si hay), porque queremos `User.where` no `User.where(...)`.
            // El TypeInfo lookup va con la posición de inicio del chain.
            let raw = std::str::from_utf8(&bytes[j..dot_pos]).unwrap_or("");
            let recv_name = match raw.find('(') {
                Some(p) => raw[..p].trim_end_matches('.').to_string(),
                None => raw.to_string(),
            };
            if recv_name.starts_with('.') || recv_name.ends_with('.') || recv_name.contains("..") {
                // No es un chain válido — fallback ScopeLevel.
            } else {
                let (recv_line_lsp, recv_col_lsp) = offset_to_position(text, j);
                return Some(CompletionContext::AfterDot {
                    recv_name,
                    recv_line: (recv_line_lsp as usize) + 1,
                    recv_col: (recv_col_lsp as usize) + 1,
                });
            }
        }
    }
    // Mini-tanda LSPz (v0.9.47) — `from <mod> import |` o
    // `from <mod> import X, |`. Walkeamos hacia atrás del cursor
    // saltando whitespace + identifiers + comas hasta el primer
    // token que no encaje. Si lo que precede es `import` (con
    // espacio antes) precedido por `from <mod_path>`, contexto
    // FromImportList con `mod_path` segmentado por `.`.
    if let Some(mod_path) = detect_from_import_list_context(text, line, character) {
        return Some(CompletionContext::FromImportList { mod_path });
    }
    Some(CompletionContext::ScopeLevel)
}

/// Mini-tanda LSPz — detecta el patrón `from <mod_path> import ...|`.
/// Walkea hacia atrás desde la posición del cursor saltando el
/// prefix tipeado + cualquier `<ident>,?\s*` previo, hasta encontrar
/// el keyword `import` y un `from <ident(.<ident>)*>` antes. Devuelve
/// el `mod_path` segmentado por `.` o `None` si el contexto no
/// matchea.
fn detect_from_import_list_context(text: &str, line: u32, character: u32) -> Option<Vec<String>> {
    let offset = position_to_offset(text, line, character)?;
    let bytes = text.as_bytes();
    // Saltar prefix tipeado.
    let mut i = offset;
    while i > 0 && is_ident_continue(bytes[i - 1]) {
        i -= 1;
    }
    // Saltar items previos de la lista: `<ident>,?\s*`.
    loop {
        // Skip whitespace.
        while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
            i -= 1;
        }
        // Skip coma opcional.
        if i > 0 && bytes[i - 1] == b',' {
            i -= 1;
            // Después de coma debe haber whitespace + ident hacia
            // atrás (otro item de la lista).
            while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
                i -= 1;
            }
            let id_end = i;
            while i > 0 && is_ident_continue(bytes[i - 1]) {
                i -= 1;
            }
            if i == id_end {
                // No hay ident antes de la coma — patrón inválido.
                return None;
            }
            continue;
        }
        break;
    }
    // Acá debe haber `import` + whitespace.
    if i < 6 || &bytes[i - 6..i] != b"import" {
        return None;
    }
    i -= 6;
    // Whitespace + módulo path: `<ident>(.<ident>)*`.
    while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
        i -= 1;
    }
    let mod_end = i;
    // Walkea el path back-to-front: chars de ident + `.`.
    while i > 0 {
        let c = bytes[i - 1];
        if is_ident_continue(c) || c == b'.' {
            i -= 1;
        } else {
            break;
        }
    }
    let mod_start = i;
    if mod_start == mod_end {
        return None;
    }
    let mod_str = std::str::from_utf8(&bytes[mod_start..mod_end]).ok()?;
    // Validar shape: ident(.ident)* (no empezar/terminar con `.`,
    // ni dos puntos seguidos).
    if mod_str.starts_with('.') || mod_str.ends_with('.') || mod_str.contains("..") {
        return None;
    }
    // Whitespace + `from`.
    while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
        i -= 1;
    }
    if i < 4 || &bytes[i - 4..i] != b"from" {
        return None;
    }
    // El `from` debe estar al principio de la línea (o precedido de
    // whitespace solo). No imponemos esto estrictamente; en Fitz
    // `from` solo aparece como stmt top-level, así que en práctica
    // siempre matchea.
    let mod_path: Vec<String> = mod_str.split('.').map(|s| s.to_string()).collect();
    Some(mod_path)
}

/// Convierte una `(line, character)` LSP (0-based) a un offset en
/// bytes dentro del `text`. Devuelve `None` si la posición está más
/// allá del fin del texto.
///
/// **v0.9.51** — usa **chars Unicode** (equivalente a `len_utf16 ==
/// 1`) para `character`, alineado con `positionEncoding: "utf-8"`
/// que el server declara en `capabilities.position_encoding`. El
/// cliente respeta esa negociación y manda offsets en bytes UTF-8
/// (chars Unicode para ASCII + BMP; los chars del SMP suman
/// `len_utf8()` bytes pero el cursor del cliente los cuenta como
/// 1 unit en UTF-8 también).
///
/// Decisión técnica: mantener consistencia con `TypeEnv`/`TypeInfo`/
/// `DefinitionInfo` que indexan por chars Unicode 1-based del
/// lexer (`column += 1` por char no-newline en `lexer.rs::advance`).
/// Pre-fix asumía implícitamente UTF-8 sin declararlo en capabilities;
/// clientes que negocian UTF-16 default rompían con multi-byte
/// chars (emoji, etc.). Post-fix: capability explícita + tests
/// con multi-byte chars.
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
            // v0.9.51 — chars Unicode (paralelo a `position_to_offset`,
            // alineado con `positionEncoding: "utf-8"`).
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
/// v0.10.12 — Completions para el contexto `AfterAt` (cursor
/// tipeando un decorator después de `@`). Devuelve la lista cerrada
/// de decorators del lenguaje agrupados por familia, con snippets
/// `${N:label}` donde `${0}` indica el cursor final post-completion.
///
/// VSCode filtra client-side por el prefix tipeado, así que
/// devolvemos siempre la lista completa — el usuario ve `@ge` →
/// `@get`, `@post`, `@put`, `@delete` (filtrado a `@get` por
/// prefix match).
///
/// **Snippets**:
/// - Decorators con un arg típico (`@get("/path")`, `@table("name")`)
///   emiten `nombre("${1:placeholder}")` con tabstop.
/// - Decorators sin args (`@hidden`, `@primary`, `@test`,
///   `@authenticated`, `@admin`, `@background`) emiten el nombre
///   plano sin paréntesis.
/// - Decorators con multiples args opcionales (`@server`, `@cors`)
///   emiten `nombre(${1:args})` con un solo placeholder editable.
/// - Decorators de relation (`@belongs_to`, `@has_one`, `@has_many`)
///   emiten `nombre("${1:Target}", via="${2:fk}")` con dos tabstops.
fn decorator_completions() -> Vec<CompletionItem> {
    use tower_lsp::lsp_types::InsertTextFormat;

    // Cada tuple: (label, snippet, detail, doc)
    // - label es lo que aparece en la lista de VSCode.
    // - snippet usa sintaxis ${N:placeholder} para tabstops.
    // - detail es la firma corta.
    // - doc es la descripción de qué hace.
    let entries: &[(&str, &str, &str, &str)] = &[
        // HTTP routing
        (
            "get",
            "get(\"${1:/path}\")",
            "@get(path) — HTTP GET handler",
            "Registra un handler HTTP GET. Path con `{param}` para path params.",
        ),
        (
            "post",
            "post(\"${1:/path}\")",
            "@post(path) — HTTP POST handler",
            "Registra un handler HTTP POST. Body deserializado al tipo del param leftover.",
        ),
        (
            "put",
            "put(\"${1:/path}\")",
            "@put(path) — HTTP PUT handler",
            "Registra un handler HTTP PUT. Body deserializado al tipo del param leftover.",
        ),
        (
            "delete",
            "delete(\"${1:/path}\")",
            "@delete(path) — HTTP DELETE handler",
            "Registra un handler HTTP DELETE.",
        ),
        (
            "server",
            "server(${1:3000})",
            "@server(port, host?, ws_heartbeat_secs?, ...)",
            "Configura el listener HTTP. Args: port, host (default \"127.0.0.1\"), ws_heartbeat_secs, api_version, docs.",
        ),
        (
            "header",
            "header(\"${1:Header-Name}\")",
            "@header(name) — param del handler bindeado desde header",
            "El param del handler recibe el valor del header HTTP. Solo Str o Str?.",
        ),
        // Middleware / CORS
        (
            "middleware",
            "middleware(${1:fn_name})",
            "@middleware(fn) — apilable antes del decorator de ruta",
            "Cadena de middlewares ejecutados en orden. `return null` continúa, `return <status> {...}` short-circuit.",
        ),
        (
            "cors",
            "cors()",
            "@cors() o @cors({allow_origin: \"...\", ...})",
            "CORS para la ruta. Sin args: defaults permisivos. Con map: override de allow_origin/methods/headers/max_age.",
        ),
        // Auth
        (
            "authenticated",
            "authenticated",
            "@authenticated — handler protegido por el provider",
            "Valida bearer token via el @auth_provider singleton. El primer param leftover recibe el User autenticado.",
        ),
        (
            "admin",
            "admin",
            "@admin — handler protegido + role == \"admin\"",
            "Equivalente a @authenticated + check `user.role == \"admin\"`. Devuelve 403 si no admin.",
        ),
        (
            "auth_provider",
            "auth_provider",
            "@auth_provider — singleton resolutor de tokens",
            "Marca la fn como el provider de auth. Recibe Map<Str,Str> headers, retorna Result<User>.",
        ),
        // WS + Jobs
        (
            "ws",
            "ws(\"${1:/path}\")",
            "@ws(path) — WebSocket endpoint",
            "Async fn con primer param WsConn<T> typed. T es el message type marshalled de/al cliente.",
        ),
        (
            "cron",
            "cron(\"${1:0 */5 * * * *}\")",
            "@cron(expr) — job periódico",
            "Expression cron (5/6/7 fields Unix). Sync o async. Sin params, return Null/Result/Future. \
             Kwargs opcionales (iter2): `tz=\"IANA/Name\"` (default UTC), \
             `retry={max: N, backoff: \"exponential\"|\"linear\"|\"constant\", initial_secs: I, max_secs: M}`, \
             `catch_up=true|false` (default false), \
             `store=db` (persiste runs en fitz_cron_jobs/fitz_cron_runs).",
        ),
        (
            "background",
            "background",
            "@background — marca fn como spawnable via spawn(fn(...))",
            "Marker opt-in. Habilita el call `spawn(fn(args))` fire-and-forget tipado a Future<T>. \
             Kwargs opcionales (iter2): `tz=\"IANA/Name\"`, \
             `retry={...}` (mismo shape que @cron).",
        ),
        (
            "test",
            "test",
            "@test — registra como test unit (fitz test)",
            "Sin params. Bodies pueden usar assert/assert_eq/assert_ne/assert_throws builtins.",
        ),
        // Fase 12.1 (v0.12.0) — Health checks K8s.
        (
            "healthz",
            "healthz",
            "@healthz — liveness probe (auto-mount GET /healthz)",
            "Singleton. Sin params. Return Bool / Result<Null> / Result<Bool> (sync o async). \
             Mapea Bool true / Ok / Null → 200; Bool false / Err → 503. Sin @healthz declarado, \
             el server auto-mounta GET /healthz con respuesta default 200.",
        ),
        (
            "readyz",
            "readyz",
            "@readyz — readiness probe (auto-mount GET /readyz)",
            "Singleton. Sin params. Return Bool / Result<Null> / Result<Bool> (sync o async). \
             Durante SIGTERM/graceful shutdown, retorna 503 inmediato (K8s deja de rutear) sin tocar \
             el handler. Sin @readyz declarado, el server auto-mounta GET /readyz con respuesta \
             default 200.",
        ),
        // v0.11.0 (Fase 13) — CLI builder.
        (
            "command",
            "command(\"${1:name}\", desc=\"${2:descripción}\")",
            "@command(name, desc=) — declara fn como comando CLI",
            "El binario producido por `fitz build` parsea argv y dispatcha. Return type debe ser Int (exit code). Params sin default = positional args; con default = flags. Bool con default false → flag bool.",
        ),
        // ORM
        (
            "table",
            "table(\"${1:tabla}\")",
            "@table(\"name\") — type → tabla Postgres",
            "Habilita los read/write methods del ORM sobre el type. Requiere @primary en algún field.",
        ),
        (
            "primary",
            "primary",
            "@primary — field es la PK",
            "Sobre un field. Exactamente uno por type. Composite PKs no soportadas en MVP.",
        ),
        (
            "column",
            "column(\"${1:sql_name}\")",
            "@column(sql_name) — override del nombre SQL del field",
            "Por default el ORM usa el nombre Fitz del field. Con @column override-eás el SQL.",
        ),
        (
            "unique",
            "unique",
            "@unique — UNIQUE constraint (field-level sin args, o type-level con cols posicionales — v0.10.29)",
            "Sobre un field sin args: marca el field como UNIQUE en el CREATE TABLE. Sobre el `type` (v0.10.29): `@unique(col1, col2, ..., name=\"optional\")` — composite UNIQUE shortcut, alias ergonómico de `@index(unique=true)`. Acepta bare idents o Str con commas.",
        ),
        (
            "check_constraint",
            "check_constraint(\"${1:expr}\")",
            "@check_constraint(\"sql_expr\", name?) — CHECK constraint declarativo (v0.10.29)",
            "Sobre el `type` con `@table`: emite `CHECK (<expr>)` en CREATE TABLE. La expr se pasa literal al SQL — Postgres valida en INSERT/UPDATE. Apilable. Sin drift check del migrator (deuda menor) — usar `db.exec(\"ALTER TABLE ... DROP/ADD CONSTRAINT\")` para cambios.",
        ),
        (
            "index",
            "index",
            "@index(col, ..., unique?, name?, where_?, using?) — índice declarado al type (v0.10.27+)",
            "Sobre el `type` con `@table`: declara índices auto-emitidos por `fitz db diff/migrate`. Composite (multi-col), unique (`unique=true`), partial (`where_=<expr>`), nombre override (`name=\"...\"`), method override (`using=\"gin\"|\"gist\"|\"brin\"|\"hash\"|\"spgist\"` — v0.10.28; btree default).",
        ),
        (
            "db_default",
            "db_default",
            "@db_default — DB asigna el value (skipea INSERT)",
            "ORM skipea el field del INSERT, Postgres aplica su DEFAULT (típico: timestamps, UUIDs gen_random_uuid()). v0.10.16: opcionalmente acepta arg Str con la expresión SQL — `@db_default(\"NOW()\")` — que `fitz db diff` emite automáticamente en CREATE TABLE / ADD COLUMN.",
        ),
        (
            "hidden",
            "hidden",
            "@hidden — field invisible para el JSON HTTP I/O",
            "Skipea de __to_fitz_json (no expone al cliente) y __FromFitzJson (rechaza extras). Útil: password_hash, tokens.",
        ),
        (
            "belongs_to",
            "belongs_to(\"${1:Target}\")",
            "@belongs_to(\"Type\", on_delete?, on_update?)",
            "Sobre un FK field. Soporta kwargs on_delete=\"cascade\"/\"set_null\"/\"restrict\"/\"no_action\".",
        ),
        (
            "has_one",
            "has_one(\"${1:Target}\", via=\"${2:fk}\")",
            "@has_one(\"Type\", via=\"fk_field\", on_delete?)",
            "Virtual field (no va a la DB). El target hospeda el FK. Para `.preload(...)`.",
        ),
        (
            "has_many",
            "has_many(\"${1:Target}\", via=\"${2:fk}\")",
            "@has_many(\"Type\", via=\"fk_field\", on_delete?)",
            "Virtual List<Target>. El target hospeda el FK. Para `.preload(...)`.",
        ),
        (
            "renamed_from",
            "renamed_from(\"${1:old_name}\")",
            "@renamed_from(\"old_name\") — rename seguro (v0.10.17)",
            "Decorator transient para que `fitz db diff` emita `ALTER TABLE ... RENAME COLUMN/TABLE` en vez de DROP + ADD (preserva datos). Sobre un field: rename de column. Sobre el `type` (junto con `@table`): rename de tabla. Borralo después de aplicar la migration.",
        ),
    ];

    entries
        .iter()
        .map(|(label, snippet, detail, doc)| CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some((*detail).to_string()),
            documentation: Some(tower_lsp::lsp_types::Documentation::String(
                (*doc).to_string(),
            )),
            insert_text: Some((*snippet).to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        })
        .collect()
}

fn after_dot_completions(
    program: &Program,
    type_info: &TypeInfo,
    type_env: &TypeEnv,
    recv_name: &str,
    recv_line: usize,
    recv_col: usize,
) -> Vec<CompletionItem> {
    // Fase 9.w.1.b — módulos built-in `jwt` y `hash` (auth nativa).
    // Bypass del type lookup: tipan como `Any` en el checker (decisión
    // del MVP, sin `Type::Module` dedicado), así que el dispatch por
    // tipo no los identifica. Resolvemos por nombre del receiver acá,
    // antes de tocar `type_info`. Si el usuario shadowea `jwt` o `hash`
    // con un `let` propio, igual mostraríamos estos métodos — trade-off
    // aceptado del MVP, refinable post-9.w si pasa a ser problema real.
    match recv_name {
        "jwt" => {
            return method_items(&[
                (
                    "encode",
                    "fn(payload: Map, secret: Str, alg: Str?) -> Str".into(),
                ),
                (
                    "decode",
                    "fn(token: Str, secret: Str, alg: Str?) -> Result<Map>".into(),
                ),
            ]);
        }
        "hash" => {
            return method_items(&[
                ("password", "fn(plain: Str) -> Str".into()),
                ("verify", "fn(plain: Str, hashed: Str) -> Bool".into()),
            ]);
        }
        // Fase 10.1 — módulo built-in `db` para Postgres. Como `jwt`/
        // `hash`, tipa como `Any` en el checker (no hay `Type::Module`
        // dedicado en MVP), así que el dispatch por tipo no lo detecta.
        // Resolvemos por nombre acá.
        "db" => {
            return method_items(&[("connect", "async fn(url: Str) -> Result<DbConn>".into())]);
        }
        _ => {}
    }

    // Fase 10.3 — métodos estáticos ORM sobre `TableName.` cuando el
    // type tiene `@table`. `recv_name` es el identificador del type
    // (`User.`, `Order.`, etc.); resolvemos via `type_env.lookup` +
    // `table_metadata`. Si matchea, devolvemos las 3 estáticos del ORM
    // (all/where/insert) ANTES de caer al type lookup heurístico
    // (que tipa al `User.` como `Value::Type`, no como `Type::Nominal`).
    if let Some(id) = type_env.lookup(recv_name) {
        if type_env.table_metadata(id).is_some() {
            return method_items(&[
                (
                    "all",
                    format!("async fn(db: DbConn) -> Result<List<{}>>", recv_name),
                ),
                (
                    "where",
                    "fn(predicate: fn(row) -> Bool) -> QueryBuilder".into(),
                ),
                (
                    "first",
                    format!("async fn(db: DbConn) -> Result<{}>", recv_name),
                ),
                ("count", "async fn(db: DbConn) -> Result<Int>".into()),
                (
                    "insert",
                    format!(
                        "async fn(db: DbConn, row: {}) -> Result<{}>",
                        recv_name, recv_name
                    ),
                ),
                (
                    "bulk_insert",
                    format!(
                        "async fn(rows: List<{}>, db: DbConn, batch_size?: Int) -> Result<Int>",
                        recv_name
                    ),
                ),
            ]);
        }
    }

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
            // Fields del type + métodos custom (R.3). `info()` panics
            // si el id no existe — no debería pasar (el checker valida).
            //
            // Mini-tanda Vp — los campos privados (`_field`) NO aparecen
            // en `instance.`: solo son accesibles desde adentro del
            // type body, donde el LSP no necesita sugerirlos aparte
            // porque ya son locales de la fn.
            let info = type_env.info(*id);
            let mut items = Vec::new();
            if let Some(fs) = info.fields.as_ref() {
                for f in fs {
                    if f.name.starts_with('_') {
                        continue;
                    }
                    items.push(CompletionItem {
                        label: f.name.clone(),
                        kind: Some(CompletionItemKind::FIELD),
                        detail: Some(f.type_.display(type_env)),
                        ..CompletionItem::default()
                    });
                }
            }
            // Mini-tanda V.5 — métodos custom (R.3) ahora aparecen
            // después de fields. Mini-tanda Up — `NominalMethod` ahora
            // incluye `param_names` paralelo a `params`, así la firma
            // muestra `fn(x: Int, y: Int) -> Float` en lugar de
            // `fn(Int, Int) -> Float` (mejor UX en autocomplete).
            //
            // Mini-tanda St — los métodos estáticos NO aparecen acá:
            // se invocan como `Type.method()`, no como
            // `instance.method()`. Filtramos `is_static`.
            //
            // Mini-tanda Vm — métodos privados (`_method`) tampoco
            // aparecen en `instance.`: solo accesibles desde adentro
            // del type body. Paralelo al filter de fields (Vp).
            for m in info
                .methods
                .iter()
                .filter(|m| !m.is_static && !m.name.starts_with('_'))
            {
                // Combinar param_names con params para formar
                // `x: Int, y: Float`. Si por algún motivo las longitudes
                // no coinciden (defensivo), caemos al format viejo.
                let params_str = if m.param_names.len() == m.params.len() {
                    m.params
                        .iter()
                        .zip(m.param_names.iter())
                        .map(|(t, n)| format!("{}: {}", n, t.display(type_env)))
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    m.params
                        .iter()
                        .map(|t| t.display(type_env))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let prefix = if m.is_async { "async fn" } else { "fn" };
                let detail = format!("{}({}) -> {}", prefix, params_str, m.ret.display(type_env));
                items.push(CompletionItem {
                    label: m.name.clone(),
                    kind: Some(CompletionItemKind::METHOD),
                    detail: Some(detail),
                    ..CompletionItem::default()
                });
            }
            items
        }
        // Mini-tanda Math+Mb9 — métodos sobre primitivos Int/Float.
        // Int: abs/to_str/to_str_base. Float: abs/to_str/is_nan/is_finite.
        Type::Int => method_items(&[
            ("abs", "fn() -> Int".into()),
            ("to_str", "fn() -> Str".into()),
            (
                "to_str_base",
                "fn(base: Int) -> Str  // base ∈ {2, 8, 10, 16}".into(),
            ),
            // v0.10.32 (Tier D.1) — ORM operators numéricos.
            (
                "is_in",
                "fn(values: List<Int>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "between",
                "fn(lo: Int, hi: Int) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
        ]),
        Type::Float => method_items(&[
            ("abs", "fn() -> Float".into()),
            ("to_str", "fn() -> Str".into()),
            ("is_nan", "fn() -> Bool".into()),
            ("is_finite", "fn() -> Bool".into()),
            // v0.10.32 (Tier D.1) — ORM operators numéricos.
            (
                "is_in",
                "fn(values: List<Float>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "between",
                "fn(lo: Float, hi: Float) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
        ]),
        Type::List(t) => method_items(&[
            ("push", format!("fn({}) -> Null", t.display(type_env))),
            ("pop", format!("fn() -> Result<{}>", t.display(type_env))),
            (
                "map",
                format!("fn(fn({}) -> U) -> List<U>", t.display(type_env)),
            ),
            (
                "filter",
                format!(
                    "fn(fn({}) -> Bool) -> List<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            (
                "find",
                format!(
                    "fn(fn({}) -> Bool) -> Result<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            ("len", "fn() -> Int".into()),
            // Mini-tanda S.3: `sort` y `reverse` mutan in-place y
            // devuelven Null. `contains(v)` toma un T y devuelve Bool.
            ("sort", "fn() -> Null".into()),
            ("reverse", "fn() -> Null".into()),
            ("contains", format!("fn({}) -> Bool", t.display(type_env))),
            // Mini-tanda It — iteradores estilo Python.
            (
                "enumerate",
                format!("fn() -> List<(Int, {})>", t.display(type_env)),
            ),
            (
                "zip",
                format!("fn(List<U>) -> List<({}, U)>", t.display(type_env)),
            ),
            (
                "chain",
                format!(
                    "fn(List<{}>) -> List<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            // Mini-tanda Mb — flatten + sort_by.
            (
                "flatten",
                "fn() -> List<U>  // requiere List<List<U>>".to_string(),
            ),
            (
                "sort_by",
                format!(
                    "fn(fn({}, {}) -> Int) -> Null",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            // Mini-tanda Lx — predicados funcionales.
            (
                "any",
                format!("fn(fn({}) -> Bool) -> Bool", t.display(type_env)),
            ),
            (
                "all",
                format!("fn(fn({}) -> Bool) -> Bool", t.display(type_env)),
            ),
            (
                "count",
                format!("fn(fn({}) -> Bool) -> Int", t.display(type_env)),
            ),
            (
                "find_index",
                format!("fn(fn({}) -> Bool) -> Result<Int>", t.display(type_env)),
            ),
            // Mini-tanda Ex2 — flat_map + first / last.
            (
                "flat_map",
                format!("fn(fn({}) -> List<U>) -> List<U>", t.display(type_env),),
            ),
            ("first", format!("fn() -> Result<{}>", t.display(type_env))),
            ("last", format!("fn() -> Result<{}>", t.display(type_env))),
            // Mini-tanda Mb2 — reducciones numéricas.
            (
                "min",
                format!(
                    "fn() -> Result<{}>  // List<Int> o List<Float>",
                    t.display(type_env)
                ),
            ),
            (
                "max",
                format!(
                    "fn() -> Result<{}>  // List<Int> o List<Float>",
                    t.display(type_env)
                ),
            ),
            (
                "sum",
                format!(
                    "fn() -> {}  // List<Int> o List<Float>",
                    t.display(type_env)
                ),
            ),
            // Mini-tanda Mb3 — fold + product + to_map.
            (
                "reduce",
                format!(
                    "fn(init: Acc, fn(Acc, {}) -> Acc) -> Acc",
                    t.display(type_env),
                ),
            ),
            (
                "product",
                format!(
                    "fn() -> {}  // List<Int> o List<Float>",
                    t.display(type_env)
                ),
            ),
            (
                "to_map",
                "fn() -> Map<K, V>  // requiere List<(K, V)>".into(),
            ),
            // Mini-tanda Mb4 — unique + partition.
            (
                "unique",
                format!(
                    "fn() -> List<{}>  // dedup preservando orden",
                    t.display(type_env)
                ),
            ),
            (
                "partition",
                format!(
                    "fn(fn({}) -> Bool) -> (List<{}>, List<{}>)",
                    t.display(type_env),
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
            // Mini-tanda Mb5 — group_by + zip_with + max_by/min_by.
            (
                "group_by",
                format!(
                    "fn(fn({}) -> K) -> Map<K, List<{}>>",
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
            (
                "zip_with",
                format!(
                    "fn(List<U>, fn({}, U) -> V) -> List<V>",
                    t.display(type_env),
                ),
            ),
            (
                "max_by",
                format!(
                    "fn(fn({}) -> Int) -> Result<{}>",
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
            (
                "min_by",
                format!(
                    "fn(fn({}) -> Int) -> Result<{}>",
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
            // Mini-tanda Mb6 — scan + windows.
            (
                "scan",
                format!(
                    "fn(init: Acc, fn(Acc, {}) -> Acc) -> List<Acc>",
                    t.display(type_env),
                ),
            ),
            (
                "windows",
                format!("fn(n: Int) -> List<List<{}>>", t.display(type_env),),
            ),
            // Mini-tanda Mb7 — take/drop/init/tail/intersperse/cycle.
            (
                "take",
                format!("fn(n: Int) -> List<{}>", t.display(type_env)),
            ),
            (
                "drop",
                format!("fn(n: Int) -> List<{}>", t.display(type_env)),
            ),
            (
                "init",
                format!(
                    "fn() -> List<{}>  // todos menos el último",
                    t.display(type_env)
                ),
            ),
            (
                "tail",
                format!(
                    "fn() -> List<{}>  // todos menos el primero",
                    t.display(type_env)
                ),
            ),
            (
                "intersperse",
                format!(
                    "fn(sep: {}) -> List<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            (
                "cycle",
                format!("fn(n: Int) -> List<{}>", t.display(type_env)),
            ),
            // Mini-tanda Mb8 — starts_with/ends_with/insert_at/remove_at/zip_to_map.
            (
                "starts_with",
                format!("fn(prefix: List<{}>) -> Bool", t.display(type_env)),
            ),
            (
                "ends_with",
                format!("fn(suffix: List<{}>) -> Bool", t.display(type_env)),
            ),
            (
                "insert_at",
                format!(
                    "fn(idx: Int, v: {}) -> List<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            (
                "remove_at",
                format!("fn(idx: Int) -> List<{}>", t.display(type_env)),
            ),
            (
                "zip_to_map",
                format!("fn(values: List<V>) -> Map<{}, V>", t.display(type_env)),
            ),
            // Mini-tanda Mb9 — split_at(i): parte la lista en dos en idx.
            (
                "split_at",
                format!(
                    "fn(idx: Int) -> (List<{}>, List<{}>)",
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
        ]),
        Type::Map(k, v) => method_items(&[
            (
                "get",
                format!(
                    "fn({}) -> Result<{}>",
                    k.display(type_env),
                    v.display(type_env)
                ),
            ),
            ("has", format!("fn({}) -> Bool", k.display(type_env))),
            ("keys", format!("fn() -> List<{}>", k.display(type_env))),
            ("values", format!("fn() -> List<{}>", v.display(type_env))),
            ("len", "fn() -> Int".into()),
            // Mini-tanda Ex — transformaciones funcionales.
            (
                "filter",
                format!(
                    "fn(fn({}, {}) -> Bool) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            (
                "map_values",
                format!(
                    "fn(fn({}) -> U) -> Map<{}, U>",
                    v.display(type_env),
                    k.display(type_env),
                ),
            ),
            // Mini-tanda Ex2 — merge (last-write-wins).
            (
                "merge",
                format!(
                    "fn(Map<{}, {}>) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Mini-tanda Up — update inmutable (last-write-wins
            // sobre una sola key).
            (
                "update",
                format!(
                    "fn({}, fn({}) -> {}) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Mini-tanda Mb2 — keys_sorted: keys ordenadas.
            (
                "keys_sorted",
                format!(
                    "fn() -> List<{}>  // K comparable (Int/Float/Str/Bool)",
                    k.display(type_env),
                ),
            ),
            // Mini-tanda Mb3 — entries: pares (K, V) en orden de inserción.
            (
                "entries",
                format!(
                    "fn() -> List<({}, {})>",
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Mini-tanda Mb4 — invert: swap K ↔ V.
            (
                "invert",
                format!(
                    "fn() -> Map<{}, {}>",
                    v.display(type_env),
                    k.display(type_env),
                ),
            ),
            // Mini-tanda Mb6 — merge_with: merge con callback.
            (
                "merge_with",
                format!(
                    "fn(Map<{}, {}>, fn({}, {}) -> {}) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    v.display(type_env),
                    v.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Mini-tanda Mb7 — with: functional update.
            (
                "with",
                format!(
                    "fn({}, {}) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Mini-tanda Mb9 — has_value: chequea si V está presente.
            ("has_value", format!("fn({}) -> Bool", v.display(type_env),)),
            // v0.10.32 (Tier D.1) — ORM operators sobre Map (jsonb).
            // Solo válidos adentro de `.where(closure)` del ORM; el
            // evaluator los intercepta para emitir operadores Postgres
            // jsonb (`?`, `?&`, `?|`, `@>`, `#>`, `#>>`).
            (
                "has_key",
                "fn(k: Str) -> Bool  // (ORM .where) jsonb ? — \"k\" existe en el top-level".into(),
            ),
            (
                "has_all_keys",
                "fn(keys: List<Str>) -> Bool  // (ORM .where) jsonb ?& — todas existen".into(),
            ),
            (
                "has_any_keys",
                "fn(keys: List<Str>) -> Bool  // (ORM .where) jsonb ?| — alguna existe".into(),
            ),
            (
                "contains_json",
                "fn(patch: Map<Str, Any>) -> Bool  // (ORM .where) jsonb @> — superset".into(),
            ),
            (
                "has_path",
                "fn(path: List<Str>) -> Bool  // (ORM .where) jsonb #> — path nested existe".into(),
            ),
            (
                "path_text",
                "fn(path: List<Str>) -> Str  // (ORM .where) jsonb #>> path → text".into(),
            ),
            (
                "path_int",
                "fn(path: List<Str>) -> Int  // (ORM .where) jsonb #>> + cast bigint".into(),
            ),
            (
                "path_float",
                "fn(path: List<Str>) -> Float  // (ORM .where) jsonb #>> + cast float8".into(),
            ),
            (
                "path_bool",
                "fn(path: List<Str>) -> Bool  // (ORM .where) jsonb #>> + cast boolean".into(),
            ),
        ]),
        Type::Str => method_items(&[
            ("upper", "fn() -> Str".into()),
            ("lower", "fn() -> Str".into()),
            ("len", "fn() -> Int".into()),
            // v0.10.32 (Tier D.1) — ORM operators sobre Str. Solo
            // tienen efecto adentro de `.where(closure)` del ORM —
            // el evaluator los intercepta y traduce a SQL. Fuera del
            // ORM, llamarlos sobre un Str arroja error en runtime.
            // Documentamos con `(ORM .where)` en el detail para que
            // el user los distinga de los métodos Str regulares.
            (
                "is_in",
                "fn(values: List<Str>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "like",
                "fn(pattern: Str) -> Bool  // (ORM .where) SQL LIKE — case-sensitive".into(),
            ),
            (
                "ilike",
                "fn(pattern: Str) -> Bool  // (ORM .where) SQL ILIKE — case-insensitive".into(),
            ),
            (
                "matches",
                "fn(query: Str) -> Bool  // (ORM .where) SQL @@ to_tsquery — full-text".into(),
            ),
            (
                "plainto_matches",
                "fn(query: Str) -> Bool  // (ORM .where) SQL @@ plainto_tsquery — plain text".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
            (
                "between",
                "fn(lo: Str, hi: Str) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            // Mini-tanda S.1/S.2: métodos chicos de Str. `contains` y
            // `starts_with`/`ends_with` toman un `Str` y devuelven Bool.
            // `split` devuelve List<Str>. `trim` no toma args. `replace`
            // pide dos Strs. `repeat` un Int.
            ("contains", "fn(s: Str) -> Bool".into()),
            ("starts_with", "fn(s: Str) -> Bool".into()),
            ("ends_with", "fn(s: Str) -> Bool".into()),
            ("split", "fn(sep: Str) -> List<Str>".into()),
            ("trim", "fn() -> Str".into()),
            ("trim_start", "fn() -> Str".into()),
            ("trim_end", "fn() -> Str".into()),
            ("replace", "fn(old: Str, new: Str) -> Str".into()),
            ("repeat", "fn(n: Int) -> Str".into()),
            // Mini-tanda Ex — search.
            ("find", "fn(sub: Str) -> Result<Int>".into()),
            ("index_of", "fn(sub: Str) -> Result<Int>".into()),
            ("last_index_of", "fn(sub: Str) -> Result<Int>".into()),
            // Mini-tanda Mb2 — padding.
            ("pad_start", "fn(width: Int, ch: Str) -> Str".into()),
            ("pad_end", "fn(width: Int, ch: Str) -> Str".into()),
            // Mini-tanda Mb3 — chars: List<Str> con cada char.
            ("chars", "fn() -> List<Str>".into()),
            // Mini-tanda Mb4 — split_at: divide en char idx → (Str, Str).
            ("split_at", "fn(idx: Int) -> (Str, Str)".into()),
            // Mini-tanda Mb5 — lines + is_empty.
            ("lines", "fn() -> List<Str>".into()),
            ("is_empty", "fn() -> Bool".into()),
            // Mini-tanda Mb7 — repeat_with: repeat con separador.
            ("repeat_with", "fn(n: Int, sep: Str) -> Str".into()),
            // Mini-tanda Mb8 — left/right/center.
            ("left", "fn(n: Int) -> Str".into()),
            ("right", "fn(n: Int) -> Str".into()),
            ("center", "fn(width: Int, ch: Str) -> Str".into()),
            // Mini-tanda Mb9 — swap_case/title/is_alpha/is_digit/is_numeric.
            ("swap_case", "fn() -> Str".into()),
            ("title", "fn() -> Str".into()),
            ("is_alpha", "fn() -> Bool".into()),
            ("is_digit", "fn() -> Bool".into()),
            ("is_numeric", "fn() -> Bool".into()),
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
        // Mini-tanda Bytes — métodos del primitivo `Bytes`.
        Type::Bytes => method_items(&[
            ("len", "fn() -> Int".into()),
            ("is_empty", "fn() -> Bool".into()),
            ("to_str", "fn() -> Result<Str>".into()),
        ]),
        // Mini-tanda Ir — Range expone enumerate/zip/chain/len. Es el
        // subset que tiene sentido para un iterable numérico; el resto
        // requiere materializar primero a `List<Int>`.
        Type::Range => method_items(&[
            ("enumerate", "fn() -> List<(Int, Int)>".into()),
            ("zip", "fn(List<U>) -> List<(Int, U)>".into()),
            ("chain", "fn(List<Int>) -> List<Int>".into()),
            ("len", "fn() -> Int".into()),
            // Mini-tanda Rg — step_by(n) materializa con step.
            ("step_by", "fn(n: Int) -> List<Int>".into()),
        ]),
        // F13.D — methods universales sobre `Type::Any` para
        // type-check dinámico en heterogéneos.
        Type::Any => method_items(&[
            ("as_int", "fn() -> Result<Int>".into()),
            ("as_float", "fn() -> Result<Float>".into()),
            ("as_str", "fn() -> Result<Str>".into()),
            ("as_bool", "fn() -> Result<Bool>".into()),
            ("as_bytes", "fn() -> Result<Bytes>".into()),
            ("type_name", "fn() -> Str".into()),
        ]),
        // 9.w.2 — WebSockets tipados. `WsConn<T>` expone 4 métodos:
        // recv/send/broadcast (parametrizados sobre recv/send) + close.
        //
        // 9.w.2-binary-frames: si el tipo = Bytes, recv/send/broadcast
        // operan con frames `Message::Binary` raw (no JSON-marshalled);
        // el detail lo aclara para que el dev no se confunda.
        //
        // 9.w.2-wsconn-bidir (v0.9.38): `recv` y `send` pueden ser
        // tipos distintos para `WsConn<In, Out>`. El detail toma
        // cada uno por separado.
        Type::WsConn { recv, send } => {
            let recv_is_bytes = matches!(recv.as_ref(), Type::Bytes);
            let send_is_bytes = matches!(send.as_ref(), Type::Bytes);
            let recv_disp = recv.display(type_env);
            let send_disp = send.display(type_env);
            let recv_note = if recv_is_bytes {
                "  // espera Message::Binary del cliente"
            } else {
                "  // text frame JSON-marshalled del cliente"
            };
            let send_note = if send_is_bytes {
                "  // emite Message::Binary raw"
            } else {
                ""
            };
            let bcast_note = if send_is_bytes {
                "  // broadcast binario a TODOS los clientes del endpoint"
            } else {
                "  // a TODOS los clientes del endpoint"
            };
            method_items(&[
                (
                    "recv",
                    format!("fn() -> Result<{}>{}", recv_disp, recv_note),
                ),
                (
                    "send",
                    format!("fn(msg: {}) -> Result<Null>{}", send_disp, send_note),
                ),
                (
                    "broadcast",
                    format!("fn(msg: {}) -> Result<Null>{}", send_disp, bcast_note,),
                ),
                ("close", "fn() -> Result<Null>".into()),
            ])
        }
        // Fase 10.1 — `DbConn` (driver Postgres nativo). Métodos query
        // y exec son async; close es idempotente. Fase 10.7 (v0.10.14)
        // suma `transaction(fn(tx) -> Result<T>)` con auto-commit/rollback.
        Type::DbConn => method_items(&[
            (
                "query",
                "async fn(sql: Str, args: List<Any>) -> Result<List<DbRow>>".into(),
            ),
            (
                "exec",
                "async fn(sql: Str, args: List<Any>) -> Result<Int>  // rows affected".into(),
            ),
            ("close", "async fn() -> Result<Null>".into()),
            ("is_closed", "async fn() -> Bool".into()),
            (
                "transaction",
                "async fn(fn(tx: DbConn) -> Result<T>) -> Result<T>  // BEGIN/COMMIT/ROLLBACK auto"
                    .into(),
            ),
        ]),
        // v0.10.22 — `DbRow` (row crudo de `db.query`). Métodos tipados
        // de extracción que devuelven `Result<T>` con error claro si la
        // col no existe, es NULL, o el tipo PG no matchea.
        Type::DbRow => method_items(&[
            ("get_int", "fn(col: Str) -> Result<Int>".into()),
            ("get_str", "fn(col: Str) -> Result<Str>".into()),
            ("get_float", "fn(col: Str) -> Result<Float>".into()),
            ("get_bool", "fn(col: Str) -> Result<Bool>".into()),
            ("len", "fn() -> Int  // número de columnas del row".into()),
        ]),
        // v0.10.24 — `Date` instance methods. Extracción + conversión +
        // formato custom con specifiers chrono (%Y, %m, %d, %A, etc.).
        // v0.10.30 Tier B — aritmética (add_/subtract_) + diff (signed Int).
        Type::Date => method_items(&[
            ("year", "fn() -> Int".into()),
            ("month", "fn() -> Int  // 1..12".into()),
            ("day", "fn() -> Int  // 1..31".into()),
            ("weekday", "fn() -> Int  // ISO 8601: 1=Mon..7=Sun".into()),
            ("to_str", "fn() -> Str  // ISO 8601 YYYY-MM-DD".into()),
            ("to_datetime", "fn() -> DateTime  // 00:00:00 UTC".into()),
            (
                "format",
                "fn(fmt: Str) -> Str  // chrono format (%Y/%m/%d/%A/...)".into(),
            ),
            (
                "add_days",
                "fn(n: Int) -> Date  // v0.10.30 — n signed; panic si overflow".into(),
            ),
            (
                "add_months",
                "fn(n: Int) -> Date  // v0.10.30 — calendar-aware, clampea día".into(),
            ),
            (
                "add_years",
                "fn(n: Int) -> Date  // v0.10.30 — = add_months(n*12)".into(),
            ),
            (
                "subtract_days",
                "fn(n: Int) -> Date  // v0.10.30 — = add_days(-n)".into(),
            ),
            (
                "subtract_months",
                "fn(n: Int) -> Date  // v0.10.30 — = add_months(-n)".into(),
            ),
            (
                "subtract_years",
                "fn(n: Int) -> Date  // v0.10.30 — = add_years(-n)".into(),
            ),
            (
                "diff_days",
                "fn(other: Date) -> Int  // v0.10.30 — días signed; self - other".into(),
            ),
            // v0.10.32 (Tier D.1) — ORM operators temporales.
            (
                "is_in",
                "fn(values: List<Date>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "between",
                "fn(lo: Date, hi: Date) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
        ]),
        // v0.10.24 — `DateTime` instance methods. Mismo set que Date +
        // hour/minute/second/timestamp + extracción `.date()`.
        // v0.10.30 Tier B — sub-second + calendar arithmetic + diff +
        // timezone display (to_local / in_tz IANA).
        Type::DateTime => method_items(&[
            ("year", "fn() -> Int".into()),
            ("month", "fn() -> Int  // 1..12".into()),
            ("day", "fn() -> Int  // 1..31".into()),
            ("hour", "fn() -> Int  // 0..23".into()),
            ("minute", "fn() -> Int  // 0..59".into()),
            ("second", "fn() -> Int  // 0..59".into()),
            ("timestamp", "fn() -> Int  // Unix epoch seconds".into()),
            ("to_str", "fn() -> Str  // ISO 8601 con Z (UTC)".into()),
            ("date", "fn() -> Date  // extrae la parte fecha".into()),
            ("format", "fn(fmt: Str) -> Str  // chrono format".into()),
            (
                "add_seconds",
                "fn(n: Int) -> DateTime  // v0.10.30 — Duration::seconds(n)".into(),
            ),
            (
                "add_minutes",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "add_hours",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "add_days",
                "fn(n: Int) -> DateTime  // v0.10.30 — Duration::days(n)".into(),
            ),
            (
                "add_months",
                "fn(n: Int) -> DateTime  // v0.10.30 — calendar-aware".into(),
            ),
            (
                "add_years",
                "fn(n: Int) -> DateTime  // v0.10.30 — = add_months(n*12)".into(),
            ),
            (
                "subtract_seconds",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_minutes",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_hours",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_days",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_months",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_years",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "diff_seconds",
                "fn(other: DateTime) -> Int  // v0.10.30 — signed; self - other".into(),
            ),
            (
                "diff_minutes",
                "fn(other: DateTime) -> Int  // v0.10.30 — trunc hacia 0".into(),
            ),
            (
                "diff_hours",
                "fn(other: DateTime) -> Int  // v0.10.30 — trunc hacia 0".into(),
            ),
            (
                "diff_days",
                "fn(other: DateTime) -> Int  // v0.10.30 — trunc hacia 0".into(),
            ),
            (
                "to_local",
                "fn() -> Str  // v0.10.30 — ISO 8601 + offset en TZ del sistema".into(),
            ),
            (
                "in_tz",
                "fn(iana: Str) -> Result<Str>  // v0.10.30 — IANA tz name (ej: `America/Argentina/Buenos_Aires`)".into(),
            ),
            // v0.10.32 (Tier D.1) — ORM operators temporales.
            (
                "is_in",
                "fn(values: List<DateTime>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "between",
                "fn(lo: DateTime, hi: DateTime) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
        ]),
        // v0.10.24 — `Uuid` instance methods. MVP acotado.
        Type::Uuid => method_items(&[
            (
                "to_str",
                "fn() -> Str  // canonical xxx-xxx-xxx-xxx-xxx".into(),
            ),
            ("is_nil", "fn() -> Bool".into()),
        ]),
        // Fase 10.3+ — `QueryBuilder<Row>` del ORM. Chain methods
        // preservan QB; terminales devuelven Result<...>.
        Type::QueryBuilder(row) => {
            let row_disp = row.display(type_env);
            method_items(&[
                (
                    "where",
                    format!(
                        "fn(closure: fn({}) -> Bool) -> QueryBuilder<{}>",
                        row_disp, row_disp
                    ),
                ),
                (
                    "order_by",
                    format!(
                        "fn(closure: fn({}) -> Any) -> QueryBuilder<{}>",
                        row_disp, row_disp
                    ),
                ),
                ("limit", format!("fn(n: Int) -> QueryBuilder<{}>", row_disp)),
                (
                    "offset",
                    format!("fn(n: Int) -> QueryBuilder<{}>", row_disp),
                ),
                (
                    "group_by",
                    format!(
                        "fn(closure: fn({}) -> Any) -> QueryBuilder<{}>",
                        row_disp, row_disp
                    ),
                ),
                (
                    "all",
                    format!("async fn(db: DbConn) -> Result<List<{}>>", row_disp),
                ),
                (
                    "first",
                    format!("async fn(db: DbConn) -> Result<{}>", row_disp),
                ),
                ("count", "async fn(db: DbConn) -> Result<Int>".into()),
                (
                    "sum",
                    format!(
                        "async fn(closure: fn({}) -> Float, db: DbConn) -> Result<Float>",
                        row_disp
                    ),
                ),
                (
                    "avg",
                    format!(
                        "async fn(closure: fn({}) -> Float, db: DbConn) -> Result<Float>",
                        row_disp
                    ),
                ),
                (
                    "min",
                    format!(
                        "async fn(closure: fn({}) -> Float, db: DbConn) -> Result<Float>",
                        row_disp
                    ),
                ),
                (
                    "max",
                    format!(
                        "async fn(closure: fn({}) -> Float, db: DbConn) -> Result<Float>",
                        row_disp
                    ),
                ),
                (
                    "update",
                    "async fn(db: DbConn, changes: Map<Str, Any>) -> Result<Int>  // rows affected"
                        .into(),
                ),
                (
                    "delete",
                    "async fn(db: DbConn) -> Result<Int>  // rows affected".into(),
                ),
            ])
        }
        // PyAny y resto: sin info para sugerir.
        _ => Vec::new(),
    }
}

/// Fp — render compacto de una expresión de default param para
/// mostrar en el detail del CompletionItem. Cubre literales primitivos
/// (Int/Float/Str/Bool/Null), listas/maps vacíos, e idents. Para todo
/// lo más complejo (BinOp, FnExpr, struct lits) emite `...` como
/// placeholder — el usuario abre la fn para ver el detalle real.
fn render_default_expr(e: &crate::ast::Expr) -> String {
    use crate::ast::Expr;
    match e {
        Expr::Int(n, _) => n.to_string(),
        Expr::Float(f, _) => f.to_string(),
        Expr::Str(s, _) => format!("\"{}\"", s),
        Expr::Bool(b, _) => b.to_string(),
        Expr::Null(_) => "null".into(),
        Expr::Ident(n, _) => n.clone(),
        Expr::List(items, _) if items.is_empty() => "[]".into(),
        Expr::Map(items, _) if items.is_empty() => "{}".into(),
        Expr::UnaryOp { op, operand, .. } => {
            use crate::ast::UnaryOpKind;
            let pre = match op {
                UnaryOpKind::Neg => "-",
                UnaryOpKind::Not => "not ",
                UnaryOpKind::BitNot => "~",
            };
            format!("{}{}", pre, render_default_expr(operand))
        }
        _ => "...".into(),
    }
}

/// Mini-tanda LSPy.4 — recorre stmts buscando scopes que contengan
/// `cursor_line` y agrega sus bindings como CompletionItems.
///
/// Estrategia: walk recursivo. Para cada body-bearing stmt cuyo span
/// es `<= cursor_line`, asumimos que el cursor puede estar adentro
/// (con o sin slop por `}` de cierre). Recursamos siempre y dejamos
/// que el filtro `cursor_line >= stmt.line` lo controle. Esto es
/// conservador: a veces incluye bindings de scopes que ya cerraron
/// (false-positive aceptable — completion noise pero útil).
fn collect_local_bindings_at(stmts: &[Stmt], cursor_line: usize, items: &mut Vec<CompletionItem>) {
    for stmt in stmts {
        let start = stmt.span().line;
        // Filtro mínimo: el stmt no puede estar después del cursor.
        if start > cursor_line {
            continue;
        }
        match stmt {
            Stmt::FnDef { params, body, .. } => {
                // Params del fn visibles en todo el body. Cursor en
                // o después del `fn` ⇒ los agregamos.
                for p in params {
                    let detail = p.type_.as_ref().map(|t| t.display_name());
                    items.push(CompletionItem {
                        label: p.name.clone(),
                        kind: Some(CompletionItemKind::VARIABLE),
                        detail,
                        ..CompletionItem::default()
                    });
                }
                collect_local_bindings_at(body, cursor_line, items);
                collect_let_bindings_before(body, cursor_line, items);
            }
            Stmt::While { body, .. } | Stmt::Loop { body, .. } => {
                collect_local_bindings_at(body, cursor_line, items);
                collect_let_bindings_before(body, cursor_line, items);
            }
            Stmt::For { var, body, .. } => {
                // El var del for es local al body. Agregamos los
                // idents del pattern (Ident, Wildcard, Tuple).
                use crate::ast::Pattern;
                let add_pat = |pat: &Pattern, out: &mut Vec<CompletionItem>| {
                    if let Pattern::Ident(name) = pat {
                        out.push(CompletionItem {
                            label: name.clone(),
                            kind: Some(CompletionItemKind::VARIABLE),
                            ..CompletionItem::default()
                        });
                    } else if let Pattern::Tuple(subs) = pat {
                        for sub in subs {
                            if let Pattern::Ident(name) = sub {
                                out.push(CompletionItem {
                                    label: name.clone(),
                                    kind: Some(CompletionItemKind::VARIABLE),
                                    ..CompletionItem::default()
                                });
                            }
                        }
                    }
                };
                add_pat(var, items);
                collect_local_bindings_at(body, cursor_line, items);
                collect_let_bindings_before(body, cursor_line, items);
            }
            _ => {}
        }
    }
}

/// Mini-tanda LSPy.4 — agrega bindings de `let` declarados ANTES de
/// la línea del cursor en el mismo bloque. Decimos "antes" en sentido
/// estricto (`let x = ...` en línea 5 es visible desde línea 5 en
/// adelante). Para nested blocks (if/match/loop dentro del body),
/// no recursamos — los maneja `collect_local_bindings_at`.
fn collect_let_bindings_before(
    block: &[Stmt],
    cursor_line: usize,
    items: &mut Vec<CompletionItem>,
) {
    for stmt in block {
        let line = stmt.span().line;
        if line > cursor_line {
            break;
        }
        if let Stmt::Assign {
            target: crate::ast::AssignTarget::Ident(name),
            ..
        } = stmt
        {
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                ..CompletionItem::default()
            });
        }
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
fn scope_level_completions(
    program: &Program,
    type_env: &TypeEnv,
    cursor_line: usize,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // LSPy.4 — scope-aware: si el cursor cae adentro del body de algún
    // stmt anidado (FnDef, While, Loop, For, If, Match), agregamos sus
    // bindings al scope visible. Walkeamos top-down y recursamos solo
    // en blocks que contengan el cursor.
    collect_local_bindings_at(program, cursor_line, &mut items);

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
            Stmt::FnDef {
                name,
                params,
                return_type,
                is_async,
                ..
            } => {
                // Fp — firma de la fn con tipos + defaults (cuando los hay).
                // Fp.2 — varargs prefijado con `...`.
                let params_str = params
                    .iter()
                    .map(|p| {
                        let ty = p
                            .type_
                            .as_ref()
                            .map(|t| t.display_name())
                            .unwrap_or_else(|| "Any".into());
                        let prefix = if p.varargs { "..." } else { "" };
                        let base = format!("{}{}: {}", prefix, p.name, ty);
                        if let Some(default) = &p.default {
                            format!("{} = {}", base, render_default_expr(default))
                        } else {
                            base
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let ret_str = return_type
                    .as_ref()
                    .map(|t| t.display_name())
                    .unwrap_or_else(|| "Any".into());
                let prefix = if *is_async { "async fn" } else { "fn" };
                let detail = format!("{}({}) -> {}", prefix, params_str, ret_str);
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(detail),
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
        // Fase 9.w.3 — `spawn(fn_call)` fire-and-forget. El target del
        // call debe estar marcado con `@background`. Devuelve un
        // `Future<T>` awaitable; ignorar el Future deja la task
        // ejecutándose detached.
        ("spawn", "fn(fn_call) -> Future<T>  // requiere @background"),
        ("cors", "fn(config: Map?) -> CorsConfig"),
        ("bytes", "fn(s: Str) -> Bytes"),
        ("assert", "fn(cond: Bool, msg: Str?) -> Null"),
        ("assert_eq", "fn(a, b) -> Null"),
        ("assert_ne", "fn(a, b) -> Null"),
        ("assert_throws", "fn(callback: fn() -> Any) -> Null"),
        // Mini-tanda Bits-extras — ops sobre Int como builtins globales.
        (
            "popcount",
            "fn(n: Int) -> Int  // population count (bits=1)",
        ),
        (
            "leading_zeros",
            "fn(n: Int) -> Int  // count leading 0-bits",
        ),
        (
            "trailing_zeros",
            "fn(n: Int) -> Int  // count trailing 0-bits",
        ),
        (
            "rotate_left",
            "fn(n: Int, k: Int) -> Int  // rotate bits izq",
        ),
        (
            "rotate_right",
            "fn(n: Int, k: Int) -> Int  // rotate bits der",
        ),
        // Mini-tanda Math — builtins numéricos polimórficos.
        ("abs", "fn(n: Int|Float) -> Int|Float"),
        ("min", "fn(a, b) -> Int|Float  // mismo tipo"),
        ("max", "fn(a, b) -> Int|Float  // mismo tipo"),
        ("pow", "fn(base, exp) -> Float"),
        ("sqrt", "fn(x: Int|Float) -> Float"),
        ("ceil", "fn(x: Int|Float) -> Int"),
        ("floor", "fn(x: Int|Float) -> Int"),
        ("round", "fn(x: Int|Float) -> Int"),
        ("clamp", "fn(x, lo, hi) -> Int|Float  // mismo tipo"),
        // Mini-fase env builtin (2026-05-22, Paso 3 post-boilerplates).
        (
            "env",
            "fn(key: Str) -> Result<Str>  // env var, Err si missing",
        ),
        (
            "env_or",
            "fn(key: Str, default: Str) -> Str  // env var con default",
        ),
        (
            "load_env",
            "fn(path: Str) -> Result<Null>  // parse KEY=VALUE file",
        ),
        // Fase 12.2.a — secret/config builtins.
        (
            "secret",
            "fn(key: Str) -> Result<Secret<Str>>  // env var | /run/secrets/<key>",
        ),
        (
            "config",
            "fn(key: Str, default: T) -> T  // env var con type coercion + default",
        ),
        // 10.8.7 (v0.10.8) — broadcast cross-handler a clientes WS.
        (
            "ws_broadcast",
            "fn(endpoint: Str, msg) -> Null  // broadcast JSON a clientes WS",
        ),
    ] {
        items.push(CompletionItem {
            label: name.into(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(detail.into()),
            ..CompletionItem::default()
        });
    }

    // Fase 9.w.1.b — módulos auth nativa `jwt` y `hash`, siempre
    // disponibles como Value::Module en el env global del evaluator
    // y como bindings `Any` en el checker. Listados como MODULE para
    // que VSCode los muestre con el icono apropiado y los distinga
    // de fns y vars. Fase 10.1 — `db` módulo nativo para Postgres,
    // `db.connect(url) -> DbConn`, métodos en QueryBuilder y Type
    // ORM cuando hay `@table`.
    for (name, detail) in [
        ("jwt", "module: encode, decode"),
        ("hash", "module: password, verify"),
        ("db", "module: connect (Postgres native driver + ORM)"),
    ] {
        items.push(CompletionItem {
            label: name.into(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some(detail.into()),
            ..CompletionItem::default()
        });
    }

    // Tipos built-in: visibles como nombres en posición de anotación.
    for name in [
        "Int", "Float", "Str", "Bool", "Null", "Bytes", "Range", "Any", "List", "Map", "Result",
        "Future", "Request", "Response", "File", "PyAny", "WsConn", "DbConn", "DbRow",
        // v0.10.24 — tipos temporales y UUID nativos.
        "Date", "DateTime", "Uuid",
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
        "continue", "import", "from", "as", "in", "async", "await", "and", "or", "true", "false",
        "null",
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
        let err = err_at(1, 1, "variable no definida").with_hint("¿quisiste decir `name`?");
        let diags = fitz_errors_to_diagnostics(&[err]);
        assert!(
            diags[0].message.contains("variable no definida"),
            "message base: {}",
            diags[0].message,
        );
        assert!(
            diags[0]
                .message
                .contains("Sugerencia: ¿quisiste decir `name`?"),
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
        assert!(
            !errors.is_empty(),
            "lexer debería rechazar string sin cerrar"
        );
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
        assert!(
            ty.is_none(),
            "esperaba None antes del primer token, dio {ty:?}"
        );
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
        let def_span = definition_for_position(&def_info, 1, 8).expect("uso de x debe resolver");
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
            CompletionContext::AfterDot {
                recv_name,
                recv_line,
                recv_col,
            } => {
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

    // ---- v0.9.51 Mini-tanda J — UTF-8 position + F15 recovery sub-stmt ----

    #[test]
    fn position_to_offset_cuenta_chars_unicode_no_utf16_code_units() {
        // El cliente respeta la capability `positionEncoding: utf-8`
        // del server (v0.9.51). Char Unicode = 1 unit en UTF-8.
        // Caso 1: ASCII puro — col 6 apunta al `=`.
        let text = "let x = 42";
        let offset = position_to_offset(text, 0, 6).expect("offset válido");
        assert_eq!(&text[offset..offset + 1], "=", "col 6 en ASCII → `=`");
        // Caso 2: con emoji 😀 (4 bytes UTF-8, 2 code units UTF-16, 1 char
        // Unicode). El comentario empieza en col 0 = `/`, el emoji en col
        // 3. Sumar 1 al cursor pasa por el emoji entero (NO 2).
        let text = "// 😀 hola";
        let offset = position_to_offset(text, 0, 4).expect("offset válido tras emoji");
        // Tras `// `, el emoji ocupa 1 char Unicode pero 4 bytes UTF-8.
        // Cursor en col 4 (post-emoji) → byte offset = 3 + 4 = 7.
        assert_eq!(offset, 7, "offset esperado tras emoji = 7 bytes UTF-8");
    }

    #[test]
    fn offset_to_position_cuenta_chars_unicode_paralelo_a_position_to_offset() {
        // Round-trip: offset → position → offset debe devolver el mismo
        // offset (siempre que el offset esté en char boundary).
        let text = "let x = 1\nlet y: Str = \"😀\"\n";
        let original_offset = text.find('y').unwrap();
        let (line, character) = offset_to_position(text, original_offset);
        let recovered = position_to_offset(text, line, character).unwrap();
        assert_eq!(
            recovered, original_offset,
            "round-trip offset → position → offset debe ser idempotente"
        );
    }

    #[test]
    fn f15_recovery_sub_stmt_preserva_field_access_con_dot_huerfano() {
        // Pre-fix: `user.<EOF>` abortaba el stmt entero (Stmt::Error).
        // Post-fix: `parse_with_recovery` devuelve un AST con
        // `Stmt::Expr(Expr::Field { object: Ident("user"), field: "" })`,
        // permitiendo que el LSP use el tipo del object para completion.
        use crate::ast::{Expr, Stmt};
        use crate::lexer::tokenize;
        use crate::parser::parse_with_recovery;
        let src = "let user = 42\nuser.";
        let tokens = tokenize(src).expect("tokenize OK");
        let (program, errors) = parse_with_recovery(tokens);
        // Hay al menos 1 error reportado (se esperaba field).
        assert!(
            !errors.is_empty(),
            "recovery debe reportar el error del `.` huérfano"
        );
        // El segundo stmt debe ser Expr::Field con `field` vacío,
        // NO Stmt::Error.
        assert!(
            program.len() >= 2,
            "esperaba al menos 2 stmts: el let + el user.<EOF>. Got: {} stmts",
            program.len()
        );
        let last = program.last().expect("último stmt");
        match last {
            Stmt::Expr(Expr::Field { object, field, .. }, _) => {
                assert_eq!(field, "", "field debe ser placeholder vacío");
                assert!(
                    matches!(object.as_ref(), Expr::Ident(name, _) if name == "user"),
                    "object debe ser Ident(\"user\"), got: {:?}",
                    object
                );
            }
            other => panic!(
                "esperaba Stmt::Expr(Expr::Field {{ field: \"\" }}), got: {:?}",
                other
            ),
        }
    }

    #[test]
    fn f15_recovery_sub_stmt_completion_after_dot_funciona_sobre_var_local() {
        // Caso del LSP: cursor en `user.<cursor>` adentro de una fn,
        // con `user: User` declarado localmente. Pre-fix: el stmt entero
        // se descartaba, el completion solo veía vars top-level via
        // fallback. Post-fix: el Expr::Field preserva el `user`, y el
        // lookup en TypeInfo encuentra el tipo del object.
        let src = "type User { id: Int, name: Str }\n\
                   fn process() {\n  \
                     let u: User = User { id: 1, name: \"x\" }\n  \
                     u.\n\
                   }\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor en línea 3 (0-based: línea 3 del fuente, después de `u.`)
        // — col 4 (después del punto).
        let items = completion_at_position(src, &program, &type_info, &env, 3, 4);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"id"),
            "esperaba field `id` de User en completion, labels: {:?}",
            labels
        );
        assert!(
            labels.contains(&"name"),
            "esperaba field `name` de User en completion, labels: {:?}",
            labels
        );
    }

    // ---- v0.9.47 Mini-tanda LSPz — chain a.b.c. + from import ----

    #[test]
    fn detect_context_chain_de_dos_segmentos_captura_recv_completo() {
        // `a.b.|` con cursor justo después del segundo `.` → AfterDot
        // con recv_name = "a.b" (chain, no solo "b").
        let text = "a.b.";
        let ctx = detect_completion_context(text, 0, 4).unwrap();
        match ctx {
            CompletionContext::AfterDot {
                recv_name,
                recv_line,
                recv_col,
            } => {
                assert_eq!(recv_name, "a.b", "recv_name debería ser el chain completo");
                assert_eq!(recv_line, 1);
                assert_eq!(recv_col, 1, "start del chain es col 1 (el `a`)");
            }
            other => panic!("esperaba AfterDot con chain, dio {other:?}"),
        }
    }

    #[test]
    fn detect_context_chain_de_tres_segmentos_con_prefix_partial() {
        // `obj.field.method.upper` con cursor al final — chain de 3
        // segmentos + prefix "upper" tipeado.
        let text = "obj.field.method.upper";
        let ctx = detect_completion_context(text, 0, text.len() as u32).unwrap();
        match ctx {
            CompletionContext::AfterDot { recv_name, .. } => {
                assert_eq!(
                    recv_name, "obj.field.method",
                    "recv_name debería ser chain hasta antes del último `.`"
                );
            }
            other => panic!("esperaba AfterDot, dio {other:?}"),
        }
    }

    #[test]
    fn detect_context_from_import_con_cursor_tras_import_keyword() {
        // `from foo import |` → FromImportList con mod_path = ["foo"].
        let text = "from foo import ";
        let ctx = detect_completion_context(text, 0, 16).unwrap();
        match ctx {
            CompletionContext::FromImportList { mod_path } => {
                assert_eq!(mod_path, vec!["foo".to_string()]);
            }
            other => panic!("esperaba FromImportList, dio {other:?}"),
        }
    }

    #[test]
    fn detect_context_from_import_con_items_previos() {
        // `from foo import X, Y, |` → FromImportList igual (items previos
        // se saltean walkeando back-to-front por coma + ident + ws).
        let text = "from foo import X, Y, ";
        let ctx = detect_completion_context(text, 0, 22).unwrap();
        match ctx {
            CompletionContext::FromImportList { mod_path } => {
                assert_eq!(mod_path, vec!["foo".to_string()]);
            }
            other => panic!("esperaba FromImportList, dio {other:?}"),
        }
    }

    #[test]
    fn detect_context_from_import_con_mod_path_punteado() {
        // `from sub.utils import |` → mod_path = ["sub", "utils"].
        let text = "from sub.utils import ";
        let ctx = detect_completion_context(text, 0, 22).unwrap();
        match ctx {
            CompletionContext::FromImportList { mod_path } => {
                assert_eq!(mod_path, vec!["sub".to_string(), "utils".to_string()]);
            }
            other => panic!("esperaba FromImportList, dio {other:?}"),
        }
    }

    #[test]
    fn from_import_completions_devuelve_exports_del_modulo() {
        // Setup: tempdir con un main.fitz y un utils.fitz. El helper
        // resuelve utils.fitz a partir del URI de main.fitz y lista
        // fns/types/consts del módulo.
        let tmp = tempfile::tempdir().expect("tempdir");
        let main_path = tmp.path().join("main.fitz");
        let utils_path = tmp.path().join("utils.fitz");
        std::fs::write(&main_path, "from utils import \n").unwrap();
        std::fs::write(
            &utils_path,
            "fn double(n: Int) -> Int { return n * 2 }\n\
             type User { id: Int, name: Str }\n\
             let MAX: Int = 100\n",
        )
        .unwrap();
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let items = from_import_completions(&main_uri, &["utils".to_string()]);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"double"), "falta fn `double`: {labels:?}");
        assert!(labels.contains(&"User"), "falta type `User`: {labels:?}");
        assert!(labels.contains(&"MAX"), "falta const `MAX`: {labels:?}");
    }

    #[test]
    fn from_import_completions_modulo_inexistente_devuelve_vacio() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let main_path = tmp.path().join("main.fitz");
        std::fs::write(&main_path, "from no_existe import \n").unwrap();
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let items = from_import_completions(&main_uri, &["no_existe".to_string()]);
        assert!(items.is_empty());
    }

    #[test]
    fn completion_at_position_sin_uri_no_completa_from_import() {
        // El wrapper `completion_at_position` (sin URI) NO puede
        // resolver el archivo del módulo target — para
        // FromImportList devuelve vacío. Solo el wrapper con
        // `_with_uri` lo cubre. Garantía: tests existentes no se
        // rompen porque siguen usando la signature original.
        let src = "from foo import \n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 0, 16);
        assert!(
            items.is_empty(),
            "sin URI, FromImportList debe devolver vacío. Got: {items:?}"
        );
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
        // 9.w.2-binary-frames — `WsConn` ahora aparece en scope-level
        // completions (junto a List/Map/Result/Future/etc.) para que el
        // dev pueda autocompletarlo al escribir handlers `@ws`.
        assert!(
            labels.contains(&"WsConn"),
            "falta tipo built-in `WsConn` en scope-level: {labels:?}"
        );
        assert!(labels.contains(&"Bytes"));
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
        assert!(
            !labels.contains(&"print"),
            "no debería incluir builtins en after-dot"
        );
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

    // 9.w.2-binary-frames — completions sobre `WsConn<Bytes>`.
    // El path después de `conn.` lista los 4 métodos paramétricos
    // sobre T = Bytes (recv/send/broadcast/close) con detail que
    // aclara el modo binary (vs text JSON-marshalled).

    #[test]
    fn after_dot_sobre_wsconn_bytes_lista_4_metodos_modo_binary() {
        // Nota: el `\` al final strippea leading whitespace, así que la
        // línea 2 del src real es `let r = conn.recv()`. Usamos un call
        // válido (no `conn.` huérfano) para que el parser no abandone
        // el body y `Expr::Ident(conn)` quede registrado en TypeInfo.
        // Cursor en col 13 cae entre `.` y `recv`, disparando AfterDot.
        let src = "@ws(\"/raw\")\n\
                   async fn raw(conn: WsConn<Bytes>) -> Null {\n\
                   let r = conn.recv()\n\
                   return null\n\
                   }";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 2, 13);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["recv", "send", "broadcast", "close"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` de WsConn<Bytes>: {labels:?}"
            );
        }
        // `recv` detail debe tipear `Result<Bytes>` y mencionar el
        // modo binary.
        let recv = items.iter().find(|i| i.label == "recv").expect("recv item");
        let detail = recv.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("Result<Bytes>"),
            "recv detail debería tipar Result<Bytes>, fue: {detail}"
        );
        assert!(
            detail.contains("Binary"),
            "recv detail debería mencionar Binary cuando T=Bytes, fue: {detail}"
        );
        // `send` detail debe pedir arg Bytes y mencionar binary raw.
        let send = items.iter().find(|i| i.label == "send").unwrap();
        let send_detail = send.detail.as_deref().unwrap_or("");
        assert!(send_detail.contains("msg: Bytes"));
        assert!(send_detail.contains("Binary"));
    }

    #[test]
    fn after_dot_sobre_wsconn_bidir_recv_send_tipos_distintos() {
        // 9.w.2-wsconn-bidir — `WsConn<Str, ChatMsg>`: recv tipa
        // `Result<Str>`, send espera `msg: ChatMsg`.
        let src = "type ChatMsg { user: Str, text: Str }\n\
                   @ws(\"/c\")\n\
                   async fn c(conn: WsConn<Str, ChatMsg>) -> Null {\n\
                   let r = conn.recv()\n\
                   return null\n\
                   }";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 3, 13);
        let recv = items.iter().find(|i| i.label == "recv").expect("recv item");
        let recv_detail = recv.detail.as_deref().unwrap_or("");
        assert!(
            recv_detail.contains("Result<Str>"),
            "recv detail debería tipar Result<Str> (recv=Str), fue: {recv_detail}"
        );
        let send = items.iter().find(|i| i.label == "send").unwrap();
        let send_detail = send.detail.as_deref().unwrap_or("");
        assert!(
            send_detail.contains("msg: ChatMsg"),
            "send detail debería pedir ChatMsg (send), fue: {send_detail}"
        );
    }

    #[test]
    fn after_dot_sobre_wsconn_str_mantiene_detalle_text() {
        // Sanity: `WsConn<Str>` (camino histórico) no se contamina con
        // el detail de binary. Mismo shape que el test de Bytes — call
        // válido + cursor entre `.` y `recv`.
        let src = "@ws(\"/c\")\n\
                   async fn c(conn: WsConn<Str>) -> Null {\n\
                   let r = conn.recv()\n\
                   return null\n\
                   }";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 2, 13);
        let recv = items.iter().find(|i| i.label == "recv").expect("recv item");
        let detail = recv.detail.as_deref().unwrap_or("");
        assert!(detail.contains("Result<Str>"));
        assert!(
            !detail.contains("Message::Binary"),
            "WsConn<Str>.recv no debería mencionar Binary, fue: {detail}"
        );
    }

    #[test]
    fn after_dot_sobre_receiver_sin_tipo_devuelve_metodos_any() {
        // `desconocido.` — ident no resuelto. v0.9.51 F15 recovery
        // sub-stmt: el parser ahora preserva el stmt como
        // `Expr::Field { object: Ident("desconocido"), field: "" }`
        // (en lugar de descartarlo entero). El checker tipa Ident
        // sin binding como `Type::Any` (gradual escape), y el
        // dispatch `Type::Any` devuelve los 6 métodos universales
        // de F13.D (as_int/as_float/as_str/as_bool/as_bytes/
        // type_name). Pre-fix devolvía vacío porque el stmt entero
        // se descartaba y `TypeInfo` no tenía entry para el ident.
        let src = "desconocido.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 0, 12);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"as_int") && labels.contains(&"type_name"),
            "esperaba métodos universales de Type::Any (F13.D), got: {:?}",
            labels
        );
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
            "trim_start",
            "trim_end",
            "replace",
            "repeat",
        ] {
            assert!(
                labels.contains(&expected),
                "falta método en Str: `{expected}` (S+Mb): {labels:?}"
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
    fn after_dot_sobre_list_incluye_enumerate_zip_y_chain() {
        // Mini-tanda It: enumerate, zip, chain se suman a List.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["enumerate", "zip", "chain"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda It) en List: {labels:?}"
            );
        }
        // El detail de `enumerate` debe reflejar el tipo del elemento.
        let item_enum = items.iter().find(|i| i.label == "enumerate").unwrap();
        assert_eq!(
            item_enum.detail.as_deref(),
            Some("fn() -> List<(Int, Int)>")
        );
    }

    #[test]
    fn up_after_dot_map_incluye_update() {
        let src = "let m = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"update"),
            "falta `update` (mini-tanda Up): {labels:?}"
        );
    }

    #[test]
    fn up_after_dot_nominal_muestra_param_names_en_signature() {
        // Mini-tanda Up: la firma de un método custom debe mostrar
        // `fn(x: Int, y: Int)` en lugar de `fn(Int, Int)`.
        let src = "type Point {\n\
                       x: Int = 0\n\
                       y: Int = 0\n\n\
                       fn distance_to(other_x: Int, other_y: Int) -> Int {\n\
                           return (x - other_x) + (y - other_y)\n\
                       }\n\
                   }\n\
                   let p = Point { x: 1, y: 2 }\n\
                   p.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor justo después del último `.`.
        let lines: Vec<&str> = src.split('\n').collect();
        let last_line = lines.len() as u32 - 2; // -2: descontamos la línea vacía final
        let items = completion_at_position(src, &program, &type_info, &env, last_line, 2);
        let m = items
            .iter()
            .find(|i| i.label == "distance_to")
            .expect("falta distance_to");
        let detail = m.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("other_x: Int") && detail.contains("other_y: Int"),
            "esperaba firma con param names, fue: {detail:?}"
        );
    }

    #[test]
    fn ex2_after_dot_list_incluye_flat_map_first_last() {
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["flat_map", "first", "last"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` (mini-tanda Ex2): {labels:?}"
            );
        }
    }

    #[test]
    fn ex2_after_dot_map_incluye_merge() {
        let src = "let m = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"merge"),
            "falta `merge` (mini-tanda Ex2): {labels:?}"
        );
    }

    #[test]
    fn ex_after_dot_str_incluye_search_methods() {
        let src = "let s = \"hi\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["find", "index_of", "last_index_of"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` (mini-tanda Ex): {labels:?}"
            );
        }
    }

    #[test]
    fn ex_after_dot_map_incluye_filter_y_map_values() {
        let src = "let m = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["filter", "map_values"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` (mini-tanda Ex): {labels:?}"
            );
        }
    }

    #[test]
    fn vm_after_dot_oculta_metodos_privados() {
        // Mini-tanda Vm: métodos `_method` NO aparecen en `instance.`.
        let src = "type C {\n\
                       fn greet() -> Str { return \"hi\" }\n\
                       fn _hidden() -> Str { return \"x\" }\n\
                   }\n\
                   let c = C {}\n\
                   c.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor en línea 5 col 2 (0-based), justo después del `.`.
        let items = completion_at_position(src, &program, &type_info, &env, 5, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"greet"),
            "esperaba `greet` (público), dio: {labels:?}"
        );
        assert!(
            !labels.contains(&"_hidden"),
            "método `_hidden` (privado) NO debería aparecer, dio: {labels:?}"
        );
    }

    #[test]
    fn vp_after_dot_oculta_fields_privados() {
        // Mini-tanda Vp: campos `_field` NO aparecen en `instance.`
        // — son convención de privado y solo accesibles desde
        // métodos del mismo type.
        let src = "type C { name: Str = \"\", _balance: Int = 0 }\n\
                   let c = C {}\n\
                   c.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 2, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"name"),
            "esperaba `name` (público) en completion, dio: {labels:?}"
        );
        assert!(
            !labels.contains(&"_balance"),
            "campo `_balance` (privado) NO debería aparecer en completion, dio: {labels:?}"
        );
    }

    #[test]
    fn after_dot_sobre_list_incluye_any_all_count_find_index() {
        // Mini-tanda Lx: 4 predicados funcionales en List.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["any", "all", "count", "find_index"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Lx): {labels:?}"
            );
        }
        let count = items.iter().find(|i| i.label == "count").unwrap();
        assert!(
            count.detail.as_deref().unwrap_or("").contains("-> Int"),
            "esperaba firma con `-> Int`, dio: {:?}",
            count.detail
        );
    }

    #[test]
    fn after_dot_sobre_list_incluye_flatten_y_sort_by() {
        // Mini-tanda Mb: flatten + sort_by se suman a List.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["flatten", "sort_by"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb) en List: {labels:?}"
            );
        }
        let item_sort_by = items.iter().find(|i| i.label == "sort_by").unwrap();
        assert!(
            item_sort_by
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("fn(Int, Int)"),
            "esperaba firma con `fn(Int, Int)`, dio: {:?}",
            item_sort_by.detail
        );
    }

    #[test]
    fn after_dot_sobre_range_lista_iteradores_y_len() {
        // Mini-tanda Ir: después de `r.` sobre un Range, sugerimos
        // enumerate/zip/chain/len (subset que tiene sentido para un
        // iterable numérico).
        let src = "let r = 0..10\nr.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["enumerate", "zip", "chain", "len"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Ir) en Range: {labels:?}"
            );
        }
        let item_enum = items.iter().find(|i| i.label == "enumerate").unwrap();
        assert_eq!(
            item_enum.detail.as_deref(),
            Some("fn() -> List<(Int, Int)>")
        );
    }

    #[test]
    fn mb2_after_dot_sobre_list_incluye_min_max_sum() {
        // Mini-tanda Mb2: List suma 3 métodos numéricos.
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["min", "max", "sum"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb2) en List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb2_after_dot_sobre_str_incluye_pad_start_y_pad_end() {
        let src = "let s: Str = \"x\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["pad_start", "pad_end"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb2) en Str: {labels:?}"
            );
        }
        let pad_start = items.iter().find(|i| i.label == "pad_start").unwrap();
        assert_eq!(
            pad_start.detail.as_deref(),
            Some("fn(width: Int, ch: Str) -> Str"),
        );
    }

    #[test]
    fn mb2_after_dot_sobre_map_incluye_keys_sorted() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"keys_sorted"),
            "falta `keys_sorted` (mini-tanda Mb2) en Map: {labels:?}",
        );
    }

    #[test]
    fn rg_after_dot_sobre_range_incluye_step_by() {
        let src = "let r = 0..10\nr.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"step_by"),
            "falta `step_by` (mini-tanda Rg) en Range: {labels:?}",
        );
        let step_by = items.iter().find(|i| i.label == "step_by").unwrap();
        assert_eq!(step_by.detail.as_deref(), Some("fn(n: Int) -> List<Int>"));
    }

    #[test]
    fn mb3_after_dot_sobre_list_incluye_reduce_product_to_map() {
        // Mini-tanda Mb3: List suma reduce/product/to_map.
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["reduce", "product", "to_map"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb3) en List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb3_after_dot_sobre_str_incluye_chars() {
        let src = "let s: Str = \"abc\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"chars"),
            "falta `chars` (mini-tanda Mb3) en Str: {labels:?}",
        );
        let chars = items.iter().find(|i| i.label == "chars").unwrap();
        assert_eq!(chars.detail.as_deref(), Some("fn() -> List<Str>"));
    }

    #[test]
    fn mb3_after_dot_sobre_map_incluye_entries() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"entries"),
            "falta `entries` (mini-tanda Mb3) en Map: {labels:?}",
        );
        let entries = items.iter().find(|i| i.label == "entries").unwrap();
        assert_eq!(entries.detail.as_deref(), Some("fn() -> List<(Str, Int)>"));
    }

    #[test]
    fn mb4_after_dot_sobre_list_incluye_unique_y_partition() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["unique", "partition"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb4) en List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb4_after_dot_sobre_map_incluye_invert() {
        let src = "let m: Map<Int, Str> = {1: \"a\"}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"invert"),
            "falta `invert` (mini-tanda Mb4) en Map: {labels:?}",
        );
        let invert = items.iter().find(|i| i.label == "invert").unwrap();
        assert_eq!(invert.detail.as_deref(), Some("fn() -> Map<Str, Int>"));
    }

    #[test]
    fn mb4_after_dot_sobre_str_incluye_split_at() {
        let src = "let s: Str = \"abc\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"split_at"),
            "falta `split_at` (mini-tanda Mb4) en Str: {labels:?}",
        );
    }

    #[test]
    fn mb5_after_dot_sobre_list_incluye_group_by_zip_with_max_min_by() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["group_by", "zip_with", "max_by", "min_by"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb5) en List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb6_after_dot_sobre_list_incluye_scan_y_windows() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["scan", "windows"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb6) en List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb8_after_dot_sobre_list_incluye_starts_ends_with_insert_remove_at_zip_to_map() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in [
            "starts_with",
            "ends_with",
            "insert_at",
            "remove_at",
            "zip_to_map",
        ] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb8) en List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb8_after_dot_sobre_str_incluye_left_right_center() {
        let src = "let s: Str = \"x\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["left", "right", "center"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb8) en Str: {labels:?}"
            );
        }
    }

    #[test]
    fn mb7_after_dot_sobre_list_incluye_take_drop_init_tail_intersperse_cycle() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["take", "drop", "init", "tail", "intersperse", "cycle"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb7) en List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb7_after_dot_sobre_str_incluye_repeat_with() {
        let src = "let s: Str = \"x\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"repeat_with"),
            "falta `repeat_with` (mini-tanda Mb7) en Str: {labels:?}",
        );
    }

    #[test]
    fn mb7_after_dot_sobre_map_incluye_with() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"with"),
            "falta `with` (mini-tanda Mb7) en Map: {labels:?}",
        );
    }

    #[test]
    fn mb6_after_dot_sobre_map_incluye_merge_with() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"merge_with"),
            "falta `merge_with` (mini-tanda Mb6) en Map: {labels:?}",
        );
    }

    #[test]
    fn mb5_after_dot_sobre_str_incluye_lines_y_is_empty() {
        let src = "let s: Str = \"abc\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["lines", "is_empty"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (mini-tanda Mb5) en Str: {labels:?}"
            );
        }
    }

    #[test]
    fn after_dot_sobre_tuple_lista_indices_numericos_con_tipo() {
        // Mini-tanda T.1: después de `t.` sugerimos `0`, `1`, ...
        // como labels, con el tipo del campo en `detail`.
        let src = "let t = (1, \"x\", true)\nt.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["0", "1", "2"],
            "esperaba labels 0/1/2, dio {labels:?}"
        );
        // Cada item es FIELD con detail = tipo del elemento.
        let it0 = &items[0];
        let it1 = &items[1];
        let it2 = &items[2];
        assert_eq!(it0.kind, Some(CompletionItemKind::FIELD));
        assert_eq!(it0.detail.as_deref(), Some("Int"));
        assert_eq!(it1.detail.as_deref(), Some("Str"));
        assert_eq!(it2.detail.as_deref(), Some("Bool"));
    }

    #[test]
    fn after_dot_sobre_nominal_incluye_metodos_custom_r3() {
        // Mini-tanda V.5 + R.3: además de fields, los métodos custom
        // del type aparecen en la lista con kind METHOD y detail con
        // la firma. Test cubre los 3 casos: método sin args, con args,
        // y async fn.
        let src = "type User {\n    id: Int\n    name: Str\n\n    fn greet() -> Str {\n        return \"hi\"\n    }\n\n    fn double(n: Int) -> Int {\n        return n * 2\n    }\n\n    async fn fetch() -> Result<Str> {\n        return Ok(\"x\")\n    }\n}\nlet u = User { id: 1, name: \"Ada\" }\nu.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Conteo de líneas (0-based, una entrada por cada `\n`):
        //   0: type User {
        //   1:     id: Int
        //   2:     name: Str
        //   3: (blank)
        //   4:     fn greet() -> Str {
        //   5:         return "hi"
        //   6:     }
        //   7: (blank)
        //   8:     fn double(n: Int) -> Int {
        //   9:         return n * 2
        //  10:     }
        //  11: (blank)
        //  12:     async fn fetch() -> Result<Str> {
        //  13:         return Ok("x")
        //  14:     }
        //  15: }
        //  16: let u = User { id: 1, name: "Ada" }
        //  17: u.
        let items = completion_at_position(src, &program, &type_info, &env, 17, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Fields (heredados del case original).
        assert!(labels.contains(&"id"), "falta field `id`: {labels:?}");
        assert!(labels.contains(&"name"), "falta field `name`: {labels:?}");
        // Métodos custom (R.3 / V.5).
        assert!(
            labels.contains(&"greet"),
            "falta método `greet`: {labels:?}"
        );
        assert!(
            labels.contains(&"double"),
            "falta método `double`: {labels:?}"
        );
        assert!(
            labels.contains(&"fetch"),
            "falta método async `fetch`: {labels:?}"
        );
        // Kind: fields como FIELD, métodos como METHOD.
        let it_id = items.iter().find(|i| i.label == "id").unwrap();
        let it_greet = items.iter().find(|i| i.label == "greet").unwrap();
        let it_double = items.iter().find(|i| i.label == "double").unwrap();
        let it_fetch = items.iter().find(|i| i.label == "fetch").unwrap();
        assert_eq!(it_id.kind, Some(CompletionItemKind::FIELD));
        assert_eq!(it_greet.kind, Some(CompletionItemKind::METHOD));
        assert_eq!(it_double.kind, Some(CompletionItemKind::METHOD));
        assert_eq!(it_fetch.kind, Some(CompletionItemKind::METHOD));
        // Detail: firma con prefix `fn` o `async fn` y tipos de params.
        assert_eq!(it_greet.detail.as_deref(), Some("fn() -> Str"));
        assert_eq!(it_double.detail.as_deref(), Some("fn(n: Int) -> Int"));
        assert_eq!(
            it_fetch.detail.as_deref(),
            Some("async fn() -> Result<Str>")
        );
    }

    // ---- Mini-tanda Math + Mb9 + Int/Float methods ----

    #[test]
    fn mb9_after_dot_sobre_str_incluye_swap_case_title_is_alpha_is_digit_is_numeric() {
        let src = "let s: Str = \"x\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["swap_case", "title", "is_alpha", "is_digit", "is_numeric"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` (Mb9) en Str: {labels:?}"
            );
        }
    }

    #[test]
    fn mb9_after_dot_sobre_list_incluye_split_at() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"split_at"),
            "falta `split_at` en List: {labels:?}"
        );
    }

    #[test]
    fn mb9_after_dot_sobre_map_incluye_has_value() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"has_value"),
            "falta `has_value` en Map: {labels:?}"
        );
    }

    #[test]
    fn after_dot_sobre_int_incluye_abs_to_str_to_str_base() {
        let src = "let n: Int = 5\nn.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["abs", "to_str", "to_str_base"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` en Int: {labels:?}"
            );
        }
    }

    #[test]
    fn after_dot_sobre_float_incluye_abs_to_str_is_nan_is_finite() {
        let src = "let x: Float = 3.14\nx.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["abs", "to_str", "is_nan", "is_finite"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` en Float: {labels:?}"
            );
        }
    }

    // ---- Mini-tanda LSPy — Range exacto + scope-aware autocomplete ----

    #[test]
    fn lspy_ident_range_at_position_devuelve_run_de_ident() {
        let src = "let foo_bar = 42";
        // Cursor en medio del ident "foo_bar" (col 6 = `o` de "foo").
        let range = ident_range_at_position(src, 0, 6).expect("debería resolver");
        assert_eq!(range.start, Position::new(0, 4)); // start de "foo_bar"
        assert_eq!(range.end, Position::new(0, 11)); // end de "foo_bar"
    }

    #[test]
    fn lspy_ident_range_at_position_devuelve_none_si_no_hay_ident() {
        let src = "let x = 42";
        // Cursor en el `=` (col 6).
        assert!(ident_range_at_position(src, 0, 6).is_none());
    }

    #[test]
    fn lspy_ident_range_from_def_salta_keyword_let() {
        let src = "let foo = 42";
        // def_span apunta al "let" (col 1 = "l"). El helper debe
        // skipear "let " y devolver el range de "foo".
        let span = Span::new(1, 1);
        let range = ident_range_from_def(src, span).expect("debería resolver");
        assert_eq!(range.start, Position::new(0, 4)); // start de "foo"
        assert_eq!(range.end, Position::new(0, 7)); // end de "foo"
    }

    #[test]
    fn lspy_ident_range_from_def_salta_fn_keyword() {
        let src = "fn greet(name: Str) -> Str { return name }";
        let span = Span::new(1, 1);
        let range = ident_range_from_def(src, span).expect("debería resolver");
        assert_eq!(range.start, Position::new(0, 3)); // start de "greet"
        assert_eq!(range.end, Position::new(0, 8)); // end de "greet"
    }

    #[test]
    fn lspy_make_hover_with_range_incluye_range_del_ident() {
        let src = "let count = 42\n";
        let ty = Type::Int;
        let env = TypeEnv::new();
        // v0.10.32 (Tier D.2) — `make_hover_with_range` ahora toma
        // `program: &Program` para augmentar el hover con CREATE TABLE
        // SQL cuando el tipo es un `@table`. Para este test del Range,
        // pasamos un program vacío: ty es Int (no Nominal), entonces el
        // augment se skipea silenciosamente y solo se valida el Range.
        let empty_program: crate::ast::Program = Vec::new();
        // Cursor en col 6 (medio de "count" — "let " = 4 chars + "c" + "o").
        let hover = make_hover_with_range(&ty, &env, &empty_program, src, 0, 6);
        assert!(hover.range.is_some(), "esperaba Range, fue None");
        let r = hover.range.unwrap();
        assert_eq!(r.start, Position::new(0, 4)); // start de "count"
        assert_eq!(r.end, Position::new(0, 9)); // end de "count"
    }

    #[test]
    fn lspy_diagnostics_con_source_extiende_range_a_ident() {
        let src = "let xyz = unknown_var\n";
        // Crear un FitzError sintético apuntando a "unknown_var" (col 11).
        let err = FitzError::new(
            crate::error::ErrorKind::TypeError,
            1,
            11,
            "variable no definida: unknown_var",
        );
        let diagnostics = fitz_errors_to_diagnostics_with_source(&[err], src);
        assert_eq!(diagnostics.len(), 1);
        let r = diagnostics[0].range;
        assert_eq!(r.start, Position::new(0, 10));
        assert_eq!(r.end, Position::new(0, 21)); // 10 + len("unknown_var") = 21
    }

    #[test]
    fn lspy_scope_aware_completion_incluye_params_de_fn() {
        let src = "fn greet(name: Str, age: Int) -> Str {\n    \n    return name\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor en la línea 2 (en el cuerpo de greet). LSP usa 0-based.
        let items = completion_at_position(src, &program, &type_info, &env, 1, 4);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"name"),
            "esperaba param `name`: {labels:?}"
        );
        assert!(labels.contains(&"age"), "esperaba param `age`: {labels:?}");
    }

    #[test]
    fn lspy_scope_aware_completion_incluye_let_locals() {
        let src = "fn f() -> Int {\n    let mi_var: Int = 5\n    \n    return mi_var\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor en línea 3 (después del let).
        let items = completion_at_position(src, &program, &type_info, &env, 2, 4);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"mi_var"),
            "esperaba local `mi_var`: {labels:?}"
        );
    }

    #[test]
    fn lspy_scope_aware_completion_excluye_let_locales_definidos_despues() {
        // Un `let` en línea 3 NO debe aparecer si el cursor está en
        // línea 2 (forward references no se permiten).
        let src = "fn f() -> Int {\n    \n    let posterior: Int = 5\n    return 0\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor línea 2.
        let items = completion_at_position(src, &program, &type_info, &env, 1, 4);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            !labels.contains(&"posterior"),
            "no debería incluir let posterior: {labels:?}"
        );
    }

    #[test]
    fn lspy_scope_aware_completion_incluye_for_var() {
        // Source en una sola línea para evitar problemas de recovery
        // del parser sobre líneas en blanco / `}` huérfanos.
        let src = "fn f() -> Int {\n    for item in [1, 2, 3] {\n        let y: Int = item\n    }\n    return 0\n}\n";
        let (program, env, type_info, _defs, errs) = check_source_with_types(src);
        // Verificamos que el parsing fue limpio (sin Error nodes).
        assert!(
            !program.iter().any(|s| matches!(s, Stmt::Error(_))),
            "parser emitió Error nodes: {errs:?}"
        );
        // Cursor adentro del for body (línea 3, en el let).
        let items = completion_at_position(src, &program, &type_info, &env, 2, 10);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"item"),
            "esperaba `item` del for: {labels:?}"
        );
    }

    // ---- Mini-tanda LSPx — cross-module go-to-definition ----

    #[test]
    fn lspx_cross_module_resuelve_from_import() {
        // Setup: dos archivos temporales en un tmpdir único.
        // `foo.fitz` declara `type User { ... }` y una const.
        // `app.fitz` hace `from foo import User`.
        // Verificamos que `resolve_cross_module_definition` apunte
        // al span de la decl real adentro de foo.fitz.
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("fitz-lspx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let foo_path = dir.join("foo.fitz");
        let app_path = dir.join("app.fitz");
        let mut f = std::fs::File::create(&foo_path).unwrap();
        writeln!(f, "type User {{ id: Int, name: Str }}").unwrap();
        writeln!(f, "let CAP: Int = 100").unwrap();
        drop(f);
        let mut a = std::fs::File::create(&app_path).unwrap();
        writeln!(a, "from foo import User, CAP").unwrap();
        writeln!(a, "let u = User {{ id: 1, name: \"x\" }}").unwrap();
        drop(a);

        let app_src = std::fs::read_to_string(&app_path).unwrap();
        let (program, _env, _ti, _di, _errs) = check_source_with_types(&app_src);
        let doc_uri = Url::from_file_path(&app_path).unwrap();

        // Buscar el span del FromImport (línea 1 col 1).
        let import_span = program
            .iter()
            .find_map(|s| match s {
                Stmt::FromImport { span, .. } => Some(*span),
                _ => None,
            })
            .expect("debería haber FromImport");

        // Resolver `User`: debe apuntar a foo.fitz línea 1.
        let resolved = resolve_cross_module_definition(&program, &doc_uri, import_span, "User")
            .expect("esperaba resolución cross-module");
        let (target_uri, target_span) = resolved;
        // El target_uri es file:// del foo.fitz canonicalizado.
        let target_path = target_uri.to_file_path().unwrap();
        assert_eq!(
            target_path.canonicalize().unwrap(),
            foo_path.canonicalize().unwrap(),
            "esperaba target_uri = foo.fitz, dio: {:?}",
            target_path
        );
        assert_eq!(
            target_span.line, 1,
            "esperaba línea 1 (type User), dio: {}",
            target_span.line
        );

        // Resolver `CAP`: línea 2 (let CAP = 100).
        let resolved_cap = resolve_cross_module_definition(&program, &doc_uri, import_span, "CAP")
            .expect("esperaba resolución de CAP");
        assert_eq!(
            resolved_cap.1.line, 2,
            "esperaba línea 2 (let CAP), dio: {}",
            resolved_cap.1.line
        );

        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lspx_cross_module_name_inexistente_devuelve_none() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("fitz-lspx-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let foo_path = dir.join("foo.fitz");
        let app_path = dir.join("app.fitz");
        std::fs::write(&foo_path, "type User { id: Int }").unwrap();
        let mut a = std::fs::File::create(&app_path).unwrap();
        writeln!(a, "from foo import User").unwrap();
        drop(a);
        let app_src = std::fs::read_to_string(&app_path).unwrap();
        let (program, _env, _ti, _di, _errs) = check_source_with_types(&app_src);
        let doc_uri = Url::from_file_path(&app_path).unwrap();
        let import_span = program
            .iter()
            .find_map(|s| match s {
                Stmt::FromImport { span, .. } => Some(*span),
                _ => None,
            })
            .unwrap();
        // `NotImported` no figura en el import list → None.
        let resolved =
            resolve_cross_module_definition(&program, &doc_uri, import_span, "NotImported");
        assert!(resolved.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fp_scope_level_fn_con_default_incluye_signature_y_default_en_detail() {
        let src = "fn greet(name: Str = \"amigo\") -> Str { return name }\n\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 0);
        let it = items
            .iter()
            .find(|i| i.label == "greet")
            .expect("falta greet");
        let detail = it.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("name: Str = \"amigo\""),
            "esperaba `name: Str = \"amigo\"` en detail, fue: {}",
            detail
        );
        assert!(
            detail.contains("-> Str"),
            "esperaba `-> Str` en detail, fue: {}",
            detail
        );
    }

    #[test]
    fn scope_level_incluye_math_builtins() {
        let src = "let a = 1\n\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in [
            "abs", "min", "max", "pow", "sqrt", "ceil", "floor", "round", "clamp",
        ] {
            assert!(
                labels.contains(&expected),
                "falta builtin `{expected}`: {labels:?}"
            );
        }
    }

    // Fase 10 — completions del ORM/DB en el LSP.

    #[test]
    fn scope_level_incluye_db_module_y_dbconn_dbrow_types() {
        let src = "let a = 1\n\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Módulo db aparece como MODULE.
        let db_item = items
            .iter()
            .find(|i| i.label == "db")
            .expect("falta módulo `db`");
        assert_eq!(db_item.kind, Some(CompletionItemKind::MODULE));
        // Tipos built-in DbConn y DbRow aparecen como CLASS.
        for t in ["DbConn", "DbRow"] {
            assert!(labels.contains(&t), "falta tipo built-in `{t}`: {labels:?}");
        }
    }

    #[test]
    fn after_dot_sobre_db_lista_connect() {
        let src = "let x = db.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor justo después del punto: línea 0, col 11.
        let items = completion_at_position(src, &program, &type_info, &env, 0, 11);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"connect"), "falta `connect`: {labels:?}");
        // No incluye `decode`/`encode` que serían de jwt — confirma el
        // dispatch por nombre del receiver.
        assert!(
            !labels.contains(&"encode"),
            "no debería incluir jwt.encode: {labels:?}"
        );
    }

    #[test]
    fn after_dot_sobre_dbconn_lista_query_exec_close() {
        // Test del dispatch directo `Type::DbConn` → query/exec/close.
        // Mismo patrón que `after_dot_sobre_wsconn_*`: usamos un call
        // completo `conn.close()` para que el parser no abandone el
        // stmt y el Expr::Ident(conn) quede en TypeInfo. Cursor entre
        // `.` y el método dispara AfterDot.
        let src =
            "async fn run(conn: DbConn) -> Null {\n  let _ = conn.close()\n  return null\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor en línea 1, col 15 (justo después de `conn.`).
        let items = completion_at_position(src, &program, &type_info, &env, 1, 15);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["query", "exec", "close", "is_closed", "transaction"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}`: {labels:?}"
            );
        }
    }

    #[test]
    fn after_dot_sobre_dbrow_lista_get_int_get_str_get_float_get_bool_len() {
        // v0.10.22 — dispatch directo `Type::DbRow` → métodos tipados
        // de extracción (get_int/get_str/get_float/get_bool) + len.
        // Patrón: param `r: DbRow` + call completo `r.len()` para que
        // el parser no abandone el stmt y el Ident(r) quede en TypeInfo.
        let src = "fn run(r: DbRow) -> Int {\n  return r.len()\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor en línea 1, col 11 (justo después de `r.`).
        let items = completion_at_position(src, &program, &type_info, &env, 1, 11);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["get_int", "get_str", "get_float", "get_bool", "len"] {
            assert!(
                labels.contains(&expected),
                "falta método `{expected}` sobre DbRow: {labels:?}"
            );
        }
    }

    #[test]
    fn after_dot_sobre_type_con_table_lista_orm_estaticos() {
        let src = "@table(\"users\") type User {\n  @primary\n  id: Int\n  name: Str\n}\nUser.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor después de `User.` en línea 5, col 5.
        let items = completion_at_position(src, &program, &type_info, &env, 5, 5);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["all", "where", "first", "count", "insert", "bulk_insert"] {
            assert!(
                labels.contains(&expected),
                "falta estático ORM `{expected}`: {labels:?}"
            );
        }
        // El detail de `all` debe mencionar `User` (el tipo concreto).
        let all_item = items.iter().find(|i| i.label == "all").unwrap();
        let detail = all_item.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("User"),
            "esperaba `User` en detail de all, fue: {}",
            detail
        );
    }

    #[test]
    fn after_dot_sobre_query_builder_lista_chain_y_terminales() {
        // Fase 10.3+ — el QueryBuilder tipa como `Type::QueryBuilder<Row>`
        // y el after-dot lista los chain methods + terminales. Test
        // simple: `let qb = User.where(...)` separa el binding, después
        // `qb.<cursor>` dispara la heurística TypeInfo limpio (qb es
        // top-level reference). Si el dispatch funciona, todos los
        // métodos del QB están en el resultado.
        let src = "@table(\"users\") type User {\n  @primary\n  id: Int\n  age: Int\n}\nasync fn run(db: DbConn) -> Result<List<User>> {\n  let qb = User.where(fn(u) => u.age > 18)\n  let _r = qb.all(db).await?\n  return Ok([])\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Línea 7 (0-based) contenido: `  let _r = qb.all(db).await?`
        // El `.` entre `qb` y `all` está en col 14; cursor en col 15
        // (justo después del punto).
        let items = completion_at_position(src, &program, &type_info, &env, 7, 15);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Si los completions del QB están funcionando, deben estar:
        for expected in [
            "where", "order_by", "limit", "offset", "all", "first", "count",
        ] {
            assert!(
                labels.contains(&expected),
                "falta método QB `{expected}`: {labels:?}"
            );
        }
        // El detail de `all` debe mencionar el row type `User`.
        let all_item = items.iter().find(|i| i.label == "all").unwrap();
        let detail = all_item.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("User"),
            "esperaba `User` en detail de all, fue: {}",
            detail
        );
    }

    #[test]
    fn after_dot_sobre_type_sin_table_no_lista_orm_estaticos() {
        // Type sin @table NO debe ofrecer all/where/insert.
        let src = "type Plain {\n  id: Int\n}\nPlain.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 3, 6);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // No deben aparecer estos.
        assert!(
            !labels.contains(&"all"),
            "Plain sin @table no debería tener `all`: {labels:?}"
        );
        assert!(
            !labels.contains(&"where"),
            "Plain sin @table no debería tener `where`: {labels:?}"
        );
    }
}
