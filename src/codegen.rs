// codegen.rs — Fase 5b.1
//
// Transpila el AST de Fitz a código Rust. El binario final lo
// produce `rustc` invocado por el subcomando `fitz build` en
// `main.rs`. No introducimos IR intermedio en 5b.1; un visitor
// sobre el AST tipado por el checker alcanza para el subset
// soportado: literales, BinOp/UnaryOp/StrInterp, asignación,
// `if`/`while`/`loop`/`for in Range`, funciones top-level con
// tipos primitivos, `print`. Cuando entren los tipos compuestos
// (5b.2+) probablemente sumemos un IR pequeño para no acumular
// special cases en este visitor.
//
// Mapping AST de Fitz → Rust:
//
//   Int    → i64
//   Float  → f64
//   Str    → String
//   Bool   → bool
//   Null   → ()
//
// Convenciones:
//   * Variables Fitz se traducen a `let mut x = ...;` en Rust
//     (siempre mut) para simplificar la lógica de reasignación.
//   * Strings se concatenan con `format!("{}{}", a, b)`. Es
//     ineficiente pero evita los juegos de ownership de
//     `String + &str`. Optimizable después.
//   * Coerción Int→Float se inserta como `(x as f64)` en los
//     puntos donde se necesita (BinOp con tipos mixtos,
//     asignación a Float anotado, paso de Int a param Float).
//   * `print(a, b, c)` → `println!("{} {} {}", a, b, c)`. Sin
//     args, `println!()`.
//
// Limitaciones explícitas de 5b.1 (refinar en pasos siguientes):
//   * Solo tipos primitivos. Tipos custom, listas, mapas, Result,
//     módulos, HTTP — fuera de scope.
//   * Funciones anónimas (FnExpr) no se soportan.
//   * Funciones sin `return_type` declarado con cuerpo no vacío
//     que retornan algo → error de codegen. La inferencia desde
//     el body queda para 5b.2.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::ast::{
    AssignTarget, BinOpKind, Decorator, Expr, Field, Program, Stmt, StrPart, TypeExpr,
    UnaryOpKind,
};
use crate::error::{ErrorKind, FitzError};
use crate::types::{check_program, resolve_type_expr, ResolvedField, Type, TypeEnv, TypeId};

// ---------------------------------------------------------------------------
// API pública del codegen
// ---------------------------------------------------------------------------

/// Artefactos para escribir un Cargo project entero. Lo produce
/// `generate_project`; el subcomando `build` de `main.rs` los serializa
/// a disco y dispara `cargo build`.
pub struct ProjectArtifacts {
    /// Nombre del crate y del binario producido por Cargo. Sanitizado
    /// para cumplir las reglas de Cargo (alfanumérico/`-`/`_`, sin
    /// empezar con dígito). Lo usa `main.rs` para encontrar el binario
    /// resultante en `target/release/<bin_name>`.
    pub bin_name: String,
    /// Nombre del binario adyacente al `.fitz` original, sin sanitizar
    /// (matchea el stem del archivo fuente). Ej: `02-hola.fitz` →
    /// `02-hola`. `main.rs` copia `bin_name` a este nombre al final
    /// del build para preservar la convención del usuario.
    pub output_basename: String,
    pub cargo_toml: String,
    pub main_rs: String,
    /// Cero o más mod files. Cada uno apunta a una ruta relativa a `src/`
    /// (p.ej. `guide_utils.rs` o `sub/foo.rs`).
    pub mod_files: Vec<ModFile>,
}

#[derive(Debug, Clone)]
pub struct ModFile {
    pub rel_path: PathBuf,
    pub content: String,
}

/// Pipeline completo para un `fitz build`: arma el Cargo project con
/// `main.rs`, opcionales `mod` files, y `Cargo.toml`. El `src_path` es
/// el archivo `.fitz` que se está compilando — su `parent()` es el
/// `base_dir` para resolver `import`s, y su `file_stem()` es el nombre
/// del crate/binario.
///
/// 5b.5: si el programa tiene `Stmt::Import` / `Stmt::FromImport`,
/// los módulos referenciados se cargan recursivamente (parser + checker
/// + codegen) y producen entries en `mod_files`.
pub fn generate_project(
    src_path: &Path,
    program: &Program,
    env: &TypeEnv,
    dep_registry: crate::manifest::DepRegistry,
) -> Result<ProjectArtifacts, FitzError> {
    let raw_stem = src_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("fitz_build")
        .to_string();
    // Cargo exige que el `[package].name` sea un identificador válido:
    // alfanuméricos, guiones, y guiones bajos, sin empezar por dígito.
    // El stem del archivo `.fitz` puede ser cualquier cosa (ej:
    // `02-hola.fitz`). Sanitizamos: reemplazamos no-alfanuméricos por
    // `_` y prefijamos `fitz_` si empieza con dígito.
    let stem = sanitize_crate_name(&raw_stem);
    let base_dir = src_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Fase 8.7.1 — detectar imports Python ANTES del ModuleLoader.
    // El loader resuelve módulos Fitz (`./foo.fitz`); los imports
    // Python (`from python import X`) no tienen archivo en disk, los
    // procesamos por separado. Si hay un caso que 8.7.1 no soporta
    // todavía, `validate_python_imports_for_codegen` aborta con un
    // mensaje claro citando el sub-paso futuro.
    let python_imports = collect_python_imports(program);
    validate_python_imports_for_codegen(program)?;

    // PASS 1 — Cargar recursivamente todos los módulos Fitz importados
    // desde el main, generar su código Rust y registrarlos. Los
    // imports Python ya están separados en `python_imports`; el loader
    // los ignora (skip por path[0] == "python").
    let mut loader = ModuleLoader::new(base_dir.clone(), dep_registry);
    loader.collect_imports(program)?;

    // 5b.6: detectar si el programa (o algún módulo cargado) usa
    // decoradores HTTP/`@server`. Si sí, el Cargo.toml suma axum +
    // tokio + serde + serde_json. Si no, queda minimalista — los
    // ejemplos no-HTTP no pagan el costo de bajar/compilar axum.
    let has_http = has_http_routes(program);

    // Fase 6.6: idem para async. Habilita `__fitz_sleep`, `tokio::main`
    // sobre CLI, feature `time` en Cargo.toml.
    let uses_async = program_uses_async(program);

    // Fase 8.7.1: idem para interop Python. Habilita preludio
    // `__FitzPyObject` + helpers, suma `pyo3` con `abi3-py310` +
    // `auto-initialize` al Cargo.toml. Los programas sin
    // `from python import` no pagan el costo de bajar/linkear pyo3.
    let uses_python = !python_imports.is_empty();

    // PASS 2 — Generar el main.rs. El loader expone los bindings de
    // módulos (`import foo` / `from foo import X`) para que el codegen
    // del main resuelva `foo.x` como path `foo::x` y los tipos
    // importados con sus fields completos.
    let main_rs = generate_main_rs(program, env, &loader, &python_imports)?;

    Ok(ProjectArtifacts {
        bin_name: stem.clone(),
        output_basename: raw_stem,
        cargo_toml: cargo_toml_for(&stem, has_http, uses_async, uses_python),
        main_rs,
        mod_files: loader.into_mod_files(),
    })
}

/// Fase 8.7.1 — un binding Python detectado en el AST. Cada
/// `from python import math` produce un `PythonImport { binding_name:
/// "math", dotted_path: "math" }`; `from python import math as m`
/// produce `{ binding_name: "m", dotted_path: "math" }`;
/// `import python.os.path` produce `{ binding_name: "path",
/// dotted_path: "os.path" }`.
#[derive(Debug, Clone)]
struct PythonImport {
    /// Nombre visible en el scope Fitz/Rust generado. Es el `as`
    /// si está, o el último segmento del path Python por default.
    binding_name: String,
    /// Path Python "punteado" que `import` consume tal cual (igual
    /// que `import_module(dotted)` en `py_interop`).
    dotted_path: String,
}

/// Recolecta los imports Python del top-level del programa. Top-level
/// es la única posición sintácticamente válida para `Stmt::Import` /
/// `Stmt::FromImport` (el parser lo enforce), así que no recurseamos
/// adentro de fns ni de bodies.
fn collect_python_imports(program: &Program) -> Vec<PythonImport> {
    let mut out: Vec<PythonImport> = Vec::new();
    for stmt in program {
        match stmt {
            Stmt::Import { path, alias, .. }
                if path.first().map(|s| s.as_str()) == Some("python") =>
            {
                // `import python.<dotted>` — el dotted Python son los
                // segmentos del 2do en adelante. El binding default es
                // el último segmento (PreF8.4: alias gana si está).
                let dotted: String = path[1..].join(".");
                let binding_name = alias
                    .clone()
                    .or_else(|| path.last().cloned())
                    .unwrap_or_else(|| "python".to_string());
                out.push(PythonImport { binding_name, dotted_path: dotted });
            }
            Stmt::FromImport { path, names, .. }
                if path.first().map(|s| s.as_str()) == Some("python") =>
            {
                // `from python import a, b as c, ...`. El "módulo
                // base" del lado Python es `path[1..]` (puede ser
                // vacío si es `from python import math` directo —
                // ese caso trata cada `name` como módulo top-level
                // Python, paralelo al evaluator).
                let base_segments = &path[1..];
                for (name, alias) in names {
                    let dotted = if base_segments.is_empty() {
                        name.clone()
                    } else {
                        format!("{}.{}", base_segments.join("."), name)
                    };
                    let binding_name = alias.clone().unwrap_or_else(|| name.clone());
                    out.push(PythonImport { binding_name, dotted_path: dotted });
                }
            }
            _ => {}
        }
    }
    out
}

/// Fase 8.7.1 — valida que los imports Python presentes en el programa
/// caen dentro del alcance soportado por el codegen del sub-paso. Por
/// ahora soportamos solo `from python import X[.Y] [as Z]` e
/// `import python.X[.Y] [as Z]`. Reservamos espacio para sub-pasos
/// futuros que cubran patrones que hoy no andan (typically nada — el
/// shape ya es completo).
fn validate_python_imports_for_codegen(program: &Program) -> Result<(), FitzError> {
    for stmt in program {
        match stmt {
            Stmt::Import { path, .. }
                if path.first().map(|s| s.as_str()) == Some("python") && path.len() < 2 =>
            {
                return Err(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0, 0,
                    "`import python` por sí solo no es un import válido — \
                     usá `import python.<modulo>` o `from python import <modulo>`."
                        .to_string(),
                ));
            }
            Stmt::FromImport { path, names, .. }
                if path.first().map(|s| s.as_str()) == Some("python") && names.is_empty() =>
            {
                return Err(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0, 0,
                    "`from python import ...`: falta especificar al menos un módulo".to_string(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// True si el programa tiene al menos una `Stmt::FnDef` con un
/// decorador HTTP (`@get`/`@post`/`@put`/`@delete`/`@server`). El
/// codegen lo usa para decidir si agregar deps de axum/tokio/serde
/// al Cargo.toml y si emitir un `fn main()` async.
fn has_http_routes(program: &Program) -> bool {
    program.iter().any(|s| {
        matches!(s, Stmt::FnDef { decorators, .. } if !decorators.is_empty())
    })
}

/// Fase 6.6: True si el programa usa async — cualquier `async fn`
/// declarada por el usuario, cualquier `Expr::Await` en algún sitio,
/// o una llamada al builtin `sleep`. El codegen consulta este flag
/// para decidir tres cosas:
///   - Emitir el helper `__fitz_sleep` en el preludio.
///   - Para programas CLI (no-HTTP), usar `#[tokio::main(flavor =
///     "current_thread")]` sobre `fn main()`.
///   - Agregar `tokio` con feature `time` al Cargo.toml generado.
///
/// HTTP ya implica async (los handlers axum corren en tokio), así
/// que el flag es ortogonal: un programa puede ser HTTP sin sleep
/// (no necesita `time`), o CLI con sleep (no necesita axum).
fn program_uses_async(program: &Program) -> bool {
    fn expr_uses_async(e: &Expr) -> bool {
        match e {
            Expr::Await(_, _) => true,
            Expr::Call { callee, args, .. } => {
                if let Expr::Ident(n, _) = callee.as_ref() {
                    if n == "sleep" {
                        return true;
                    }
                }
                expr_uses_async(callee) || args.iter().any(expr_uses_async)
            }
            Expr::BinOp { left, right, .. } => expr_uses_async(left) || expr_uses_async(right),
            Expr::UnaryOp { operand, .. } => expr_uses_async(operand),
            Expr::Field { object, .. } => expr_uses_async(object),
            Expr::Index { object, index, .. } => expr_uses_async(object) || expr_uses_async(index),
            Expr::List(items, _) => items.iter().any(expr_uses_async),
            Expr::Map(pairs, _) => pairs.iter().any(|(k, v)| expr_uses_async(k) || expr_uses_async(v)),
            Expr::Range { start, end, .. } => expr_uses_async(start) || expr_uses_async(end),
            Expr::If { condition, then, else_, .. } => {
                expr_uses_async(condition)
                    || then.iter().any(stmt_uses_async)
                    || else_.as_ref().map(|b| b.iter().any(stmt_uses_async)).unwrap_or(false)
            }
            Expr::Match { value, arms, .. } => {
                expr_uses_async(value) || arms.iter().any(|a| expr_uses_async(&a.body))
            }
            Expr::StructLit { fields, .. } => fields.iter().any(|(_, v)| expr_uses_async(v)),
            Expr::FnExpr { body, .. } => body.iter().any(stmt_uses_async),
            Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) => expr_uses_async(inner),
            Expr::StrInterp(parts, _) => parts.iter().any(|p| match p {
                StrPart::Expr(e) => expr_uses_async(e),
                StrPart::Lit(_) => false,
            }),
            _ => false,
        }
    }
    fn stmt_uses_async(s: &Stmt) -> bool {
        match s {
            Stmt::FnDef { is_async: true, .. } => true,
            Stmt::FnDef { body, .. } => body.iter().any(stmt_uses_async),
            Stmt::Assign { value, .. } => expr_uses_async(value),
            Stmt::Return(e, _) => expr_uses_async(e),
            Stmt::ReturnStatus { status, body, .. } => {
                expr_uses_async(status)
                    || body.as_ref().map(expr_uses_async).unwrap_or(false)
            }
            Stmt::Expr(e, _) => expr_uses_async(e),
            Stmt::While { condition, body, .. } => {
                expr_uses_async(condition) || body.iter().any(stmt_uses_async)
            }
            Stmt::Loop { body, .. } => body.iter().any(stmt_uses_async),
            Stmt::For { iter, body, .. } => {
                expr_uses_async(iter) || body.iter().any(stmt_uses_async)
            }
            _ => false,
        }
    }
    program.iter().any(stmt_uses_async)
}

/// F11 — detección de state HTTP compartido.
///
/// Identifica qué vars top-level (`Stmt::Assign`) son **referenciadas
/// por al menos una fn** del programa cuando el programa tiene HTTP.
/// La detección es directa (no transitiva): si una fn cualquiera
/// (handler o helper) referencia `users` en su body, marcamos a `users`
/// como state. Las fns que tocan state se materializan al inicio del
/// body con `let users = __FITZ_STATE_USERS.with(|s| s.clone());`, así
/// que cada una toma su Rc del thread_local independientemente — no
/// hace falta propagar dependencias por la cadena de llamadas.
///
/// Devuelve:
///   - Vec con los nombres de los state vars, en orden de aparición
///     en el programa (determinista para el output del codegen).
///   - HashMap fn_name → Vec<state_var_names> referenciados (alfabético).
///
/// La función NO valida que la RHS sea compatible — eso lo hace el
/// codegen al emitir cada `Stmt::Assign` top-level dentro del init del
/// thread_local.
fn detect_shared_state(program: &Program) -> (Vec<String>, HashMap<String, Vec<String>>) {
    // Paso 1 — recolectar candidatos (top-level `Stmt::Assign` con
    // target Ident). Solo el Ident; field-assign top-level no aplica.
    let mut candidates: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut order: Vec<String> = Vec::new();
    for s in program {
        if let Stmt::Assign {
            target: AssignTarget::Ident(name),
            ..
        } = s
        {
            if candidates.insert(name.clone()) {
                order.push(name.clone());
            }
        }
    }

    // Paso 2 — para cada fn (top-level cualquiera), recolectar idents
    // referenciados que coincidan con un candidato. Excluimos params
    // de la propia fn y locals declarados en su body. Para detectar
    // locals usamos un mini-visitor con scopes.
    let mut fn_deps: HashMap<String, Vec<String>> = HashMap::new();
    let mut used_globally: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in program {
        if let Stmt::FnDef { name, params, body, .. } = s {
            let mut locals: std::collections::HashSet<String> = params
                .iter()
                .map(|p| p.name.clone())
                .collect();
            let mut refs: std::collections::HashSet<String> = std::collections::HashSet::new();
            for stmt in body {
                walk_stmt_for_state_refs(stmt, &candidates, &mut locals, &mut refs);
            }
            if !refs.is_empty() {
                let mut sorted: Vec<String> = refs.into_iter().collect();
                sorted.sort();
                for r in &sorted {
                    used_globally.insert(r.clone());
                }
                fn_deps.insert(name.clone(), sorted);
            }
        }
    }

    // Paso 3 — filtrar el orden de aparición por las vars realmente
    // referenciadas (las no referenciadas no son "state" — se quedan
    // como main_stmts top-level y se ejecutan/descartan en `fn main`).
    let final_state: Vec<String> = order
        .into_iter()
        .filter(|n| used_globally.contains(n))
        .collect();
    (final_state, fn_deps)
}

/// True si el slice de stmts contiene un `Stmt::ReturnStatus` en
/// cualquier nivel de anidamiento (loops, ifs, etc.). Lo usa
/// `gen_top_fn` para decidir si la fn HTTP debe emitir su return
/// type como `__FitzResponse`.
fn contains_return_status_stmts(stmts: &[Stmt]) -> bool {
    stmts.iter().any(contains_return_status_stmt)
}

fn contains_return_status_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::ReturnStatus { .. } => true,
        Stmt::While { body, .. }
        | Stmt::Loop { body, .. }
        | Stmt::For { body, .. } => contains_return_status_stmts(body),
        Stmt::Assign { value, .. } => contains_return_status_expr(value),
        Stmt::Return(e, _) | Stmt::Expr(e, _) => contains_return_status_expr(e),
        _ => false,
    }
}

/// Idem para expresiones — `if`/`match` contienen bodies de stmts y
/// pueden esconder un ReturnStatus adentro. `FnExpr` no cuenta (es
/// otra fn, su body es otro scope).
fn contains_return_status_expr(expr: &Expr) -> bool {
    match expr {
        Expr::If { then, else_, .. } => {
            contains_return_status_stmts(then)
                || else_.as_deref().is_some_and(contains_return_status_stmts)
        }
        Expr::Match { arms, .. } => arms.iter().any(|a| contains_return_status_expr(&a.body)),
        _ => false,
    }
}

/// Walker recursivo que detecta refs a `candidates` en un stmt,
/// respetando bindings locales nuevos. `locals` se extiende a medida
/// que entran asignaciones / for / params de FnExpr.
fn walk_stmt_for_state_refs(
    stmt: &Stmt,
    candidates: &std::collections::HashSet<String>,
    locals: &mut std::collections::HashSet<String>,
    refs: &mut std::collections::HashSet<String>,
) {
    match stmt {
        Stmt::Assign { target, value, .. } => {
            walk_expr_for_state_refs(value, candidates, locals, refs);
            match target {
                AssignTarget::Ident(name) => {
                    locals.insert(name.clone());
                }
                AssignTarget::Field { object, .. } => {
                    walk_expr_for_state_refs(object, candidates, locals, refs);
                }
            }
        }
        Stmt::Return(e, _) | Stmt::Expr(e, _) => {
            walk_expr_for_state_refs(e, candidates, locals, refs);
        }
        Stmt::ReturnStatus { status, body, .. } => {
            walk_expr_for_state_refs(status, candidates, locals, refs);
            if let Some(b) = body {
                walk_expr_for_state_refs(b, candidates, locals, refs);
            }
        }
        Stmt::While { condition, body, .. } => {
            walk_expr_for_state_refs(condition, candidates, locals, refs);
            for s in body {
                walk_stmt_for_state_refs(s, candidates, locals, refs);
            }
        }
        Stmt::Loop { body, .. } => {
            for s in body {
                walk_stmt_for_state_refs(s, candidates, locals, refs);
            }
        }
        Stmt::For { var, iter, body, .. } => {
            walk_expr_for_state_refs(iter, candidates, locals, refs);
            let was_local = locals.insert(var.clone());
            for s in body {
                walk_stmt_for_state_refs(s, candidates, locals, refs);
            }
            if !was_local {
                // El binding ya estaba — no removemos. Si era nuevo,
                // lo dejamos para mantener la conservadurez del
                // approach (mejor sobre-detectar que sub-detectar).
            }
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::FnDef { .. } | Stmt::TypeDef { .. } | Stmt::Import { .. } | Stmt::FromImport { .. } => {}
        // Fase 9.0.1 (F15): walkers estáticos del codegen ignoran
        // Error nodes — la API strict que llama al codegen nunca los
        // produce, pero defendemos contra panic si entran.
        Stmt::Error(_) => {}
    }
}

fn walk_expr_for_state_refs(
    e: &Expr,
    candidates: &std::collections::HashSet<String>,
    locals: &mut std::collections::HashSet<String>,
    refs: &mut std::collections::HashSet<String>,
) {
    match e {
        Expr::Ident(name, _) => {
            if candidates.contains(name) && !locals.contains(name) {
                refs.insert(name.clone());
            }
        }
        Expr::Int(_, _) | Expr::Float(_, _) | Expr::Str(_, _) | Expr::Bool(_, _) | Expr::Null(_) => {}
        Expr::StrInterp(parts, _) => {
            for p in parts {
                if let StrPart::Expr(inner) = p {
                    walk_expr_for_state_refs(inner, candidates, locals, refs);
                }
            }
        }
        Expr::BinOp { left, right, .. } => {
            walk_expr_for_state_refs(left, candidates, locals, refs);
            walk_expr_for_state_refs(right, candidates, locals, refs);
        }
        Expr::UnaryOp { operand, .. } => {
            walk_expr_for_state_refs(operand, candidates, locals, refs);
        }
        Expr::Call { callee, args, .. } => {
            walk_expr_for_state_refs(callee, candidates, locals, refs);
            for a in args {
                walk_expr_for_state_refs(a, candidates, locals, refs);
            }
        }
        Expr::If { condition, then, else_, .. } => {
            walk_expr_for_state_refs(condition, candidates, locals, refs);
            for s in then {
                walk_stmt_for_state_refs(s, candidates, locals, refs);
            }
            if let Some(else_b) = else_ {
                for s in else_b {
                    walk_stmt_for_state_refs(s, candidates, locals, refs);
                }
            }
        }
        Expr::Range { start, end, .. } => {
            walk_expr_for_state_refs(start, candidates, locals, refs);
            walk_expr_for_state_refs(end, candidates, locals, refs);
        }
        Expr::List(items, _) => {
            for it in items {
                walk_expr_for_state_refs(it, candidates, locals, refs);
            }
        }
        Expr::Map(pairs, _) => {
            for (k, v) in pairs {
                walk_expr_for_state_refs(k, candidates, locals, refs);
                walk_expr_for_state_refs(v, candidates, locals, refs);
            }
        }
        Expr::Index { object, index, .. } => {
            walk_expr_for_state_refs(object, candidates, locals, refs);
            walk_expr_for_state_refs(index, candidates, locals, refs);
        }
        Expr::Field { object, .. } => {
            walk_expr_for_state_refs(object, candidates, locals, refs);
        }
        Expr::StructLit { fields, .. } => {
            for (_, v) in fields {
                walk_expr_for_state_refs(v, candidates, locals, refs);
            }
        }
        Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
            walk_expr_for_state_refs(inner, candidates, locals, refs);
        }
        Expr::Match { value, arms, .. } => {
            walk_expr_for_state_refs(value, candidates, locals, refs);
            for arm in arms {
                // Patterns que introducen bindings extienden locals.
                // Aproximación conservadora: no detallamos cada
                // variante; los Ok(x)/Err(x) bindings no van a chocar
                // con state vars en la práctica (nombres distintos).
                walk_expr_for_state_refs(&arm.body, candidates, locals, refs);
            }
        }
        Expr::FnExpr { params, body, .. } => {
            // Los params del FnExpr son locales adentro del body. El
            // shadowing puede ocultar un state var, pero como no
            // removemos al salir, esto es conservador. En la práctica
            // los params de callbacks (`fn(u) => ...`) no comparten
            // nombre con state vars del scope contenedor.
            for p in params {
                locals.insert(p.name.clone());
            }
            for s in body {
                walk_stmt_for_state_refs(s, candidates, locals, refs);
            }
        }
        // Fase 9.0.1 (F15): walker estático no-op para Error nodes.
        Expr::Error(_) => {}
    }
}

/// F11 — nombre canónico del thread_local que respalda un state var.
/// `users` → `__FITZ_STATE_USERS`. Toda la convención respeta el alfa-
/// numérico ASCII; si el lexer del futuro permite identifiers no-ASCII
/// como nombres de vars, este helper tiene que adaptarse (deuda residual).
fn state_var_static_name(var_name: &str) -> String {
    format!("__FITZ_STATE_{}", var_name.to_ascii_uppercase())
}

/// Normaliza un stem de archivo `.fitz` a un identificador válido
/// para `[package].name` y `[[bin]].name` en Cargo. Reglas:
///   - Caracteres permitidos: ASCII alfanuméricos, `-`, `_`.
///   - No puede empezar con dígito.
///
/// Ejemplos: `02-hola` → `fitz_02-hola`, `mi.app` → `mi_app`,
/// `simple` → `simple`.
fn sanitize_crate_name(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.chars().next().is_none_or(|c| c.is_ascii_digit() || c == '-') {
        s = format!("fitz_{}", s);
    }
    s
}

/// Cargo.toml para el project generado. Si `has_http` es true,
/// suma axum + tokio + serde + serde_json (necesarios para 5b.6).
/// Si no, queda sin `[dependencies]` y la compilación es rápida.
fn cargo_toml_for(stem: &str, has_http: bool, uses_async: bool, uses_python: bool) -> String {
    let header = format!(
        "[package]\n\
         name = \"{stem}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [[bin]]\n\
         name = \"{stem}\"\n\
         path = \"src/main.rs\"\n",
    );
    // Fase 6.6: tokio se necesita con feature `time` cuando el
    // programa usa async (`sleep`/`.await`/`async fn`). HTTP ya pide
    // tokio con macros+rt-multi-thread; combinar las features para no
    // emitir dos entries.
    let tokio_features: &[&str] = match (has_http, uses_async) {
        (true, true) => &["macros", "rt-multi-thread", "time"],
        (true, false) => &["macros", "rt-multi-thread"],
        (false, true) => &["macros", "rt-multi-thread", "time"],
        (false, false) => &[],
    };
    // Fase 8.7.1: pyo3 con `abi3-py310` (un binario corre contra
    // cualquier CPython 3.10+) + `auto-initialize` (boot lazy de
    // CPython en el primer `Python::attach`). Sin feature gate
    // condicional — si el programa usa interop, pyo3 es dep no-opcional
    // del binario generado.
    let pyo3_line = if uses_python {
        "pyo3 = { version = \"0.28\", features = [\"abi3-py310\", \"auto-initialize\"] }\n"
    } else {
        ""
    };
    let needs_deps_section = has_http || uses_async || uses_python;
    if !needs_deps_section {
        return header;
    }
    let tokio_line = if has_http || uses_async {
        format!(
            "tokio = {{ version = \"1\", features = [{}] }}\n",
            tokio_features
                .iter()
                .map(|f| format!("\"{}\"", f))
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        String::new()
    };
    let http_lines = if has_http {
        "axum = \"0.8\"\n\
         serde = { version = \"1\", features = [\"derive\"] }\n\
         serde_json = { version = \"1\", features = [\"preserve_order\"] }\n"
    } else {
        ""
    };
    format!(
        "{}\n[dependencies]\n{}{}{}",
        header, http_lines, tokio_line, pyo3_line
    )
}

// ---------------------------------------------------------------------------
// ModuleLoader — carga recursiva de módulos para el codegen
// ---------------------------------------------------------------------------
//
// El intérprete carga módulos en runtime con un loader instalado en un
// thread_local (ver `evaluator::load_module`). El codegen necesita lo
// mismo pero AOT: leer el archivo, parsearlo, chequearlo, generarlo
// como un `mod` Rust, y guardar metadatos suficientes para resolver
// llamadas y struct literals cross-module.
//
// Alcance 5b.5: single-level imports — el main puede importar módulos,
// pero los módulos NO pueden importar otros módulos. Si llegan
// imports anidados, el loader emite un error explícito y los deja
// como deuda residual.

/// Resultado de cargar un módulo: lo que el main necesita para emitir
/// `mod foo;`, `use foo::{...};`, y resolver `foo.x` o llamadas a
/// items importados.
#[derive(Debug, Clone)]
struct LoadedModule {
    /// Nombre Rust del módulo (= último segmento del path Fitz =
    /// nombre visible en el binding). Ej: `import sub.utils` → `utils`.
    mod_name: String,
    /// Ruta del archivo Rust generado, relativa a `src/` del crate.
    /// Ej: `utils.rs` para `import utils`; `sub/utils.rs` para
    /// `import sub.utils` (junto con `sub/mod.rs`).
    rel_path: PathBuf,
    /// Código Rust completo del módulo (preludio + items con `pub`).
    rust_content: String,
    /// Firmas de tipos exportados, indexadas por nombre.
    type_sigs: HashMap<String, TypeSig>,
    /// Firmas de fns exportadas.
    fn_sigs: HashMap<String, FnSig>,
    /// Constantes / statics top-level: nombre → tipo Fitz resuelto.
    const_sigs: HashMap<String, Type>,
}

/// Binding visible en el archivo importer. Producido por el loader
/// y consumido por el `CodegenCtx` para resolver expresiones que
/// referencian items cross-module.
#[derive(Debug, Clone)]
enum ResolvedBinding {
    /// `import foo` — `foo` queda como namespace. Las expresiones
    /// `foo.greet(...)` y `foo.PREFIX` se traducen a paths Rust
    /// (`foo::greet(...)`, `foo::PREFIX`) consultando esta sig.
    Namespace { module_index: usize },
    /// `from foo import X` — `X` queda como item directo en el scope.
    /// `kind` decide si emitimos `use foo::X;` (fn/const) o
    /// `use foo::{X, XData};` (type).
    Named {
        module_index: usize,
        item: String,
        kind: NamedKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamedKind {
    Type,
    Fn,
    Const,
}

struct ModuleLoader {
    base_dir: PathBuf,
    /// Módulos cargados en orden de descubrimiento. Cada `mod foo;`
    /// del main.rs se emite en este orden.
    modules: Vec<LoadedModule>,
    /// Map de path canonicalizado a índice en `modules` — para cache
    /// (el mismo archivo importado dos veces produce una sola entry).
    by_path: HashMap<PathBuf, usize>,
    /// Bindings nombrados (visibles en el scope del importer).
    /// Mapean cada `import foo` / `from foo import X` al módulo
    /// resuelto y al kind de uso.
    bindings: HashMap<String, ResolvedBinding>,
    /// Fase 9.y.3.b — registry de deps del proyecto raíz. Cuando
    /// `from <dep> import X` aparece, `resolve_path` matchea contra
    /// este map ANTES de fallback al path relativo `<base>/foo.fitz`.
    /// Empty hashmap en single-file mode (pre-9.y.2 behavior).
    dep_registry: crate::manifest::DepRegistry,
}

impl ModuleLoader {
    fn new(base_dir: PathBuf, dep_registry: crate::manifest::DepRegistry) -> Self {
        Self {
            base_dir,
            modules: Vec::new(),
            by_path: HashMap::new(),
            bindings: HashMap::new(),
            dep_registry,
        }
    }

    /// Recorre el AST del programa principal y carga cada módulo
    /// referenciado por `Stmt::Import` / `Stmt::FromImport`.
    ///
    /// Fase 8.7.1: skip los imports Python (`path[0] == "python"`).
    /// Esos no tienen archivo `.fitz` en disk — los procesa
    /// `collect_python_imports` por separado y `generate_main_rs`
    /// los emite como bindings PyO3 directamente.
    fn collect_imports(&mut self, program: &Program) -> Result<(), FitzError> {
        for stmt in program {
            match stmt {
                Stmt::Import { path, .. }
                    if path.first().map(|s| s.as_str()) == Some("python") =>
                {
                    continue;
                }
                Stmt::FromImport { path, .. }
                    if path.first().map(|s| s.as_str()) == Some("python") =>
                {
                    continue;
                }
                Stmt::Import { path, alias, .. } => {
                    let idx = self.load_module(path)?;
                    // PreF8.4: alias gana sobre el último segmento.
                    let binding_name = alias.clone().unwrap_or_else(|| {
                        path.last().cloned().unwrap_or_default()
                    });
                    self.bindings.insert(
                        binding_name,
                        ResolvedBinding::Namespace { module_index: idx },
                    );
                }
                Stmt::FromImport { path, names, .. } => {
                    let idx = self.load_module(path)?;
                    // PreF8.4: cada entry es `(name, alias?)`. El lookup
                    // y `item` siguen usando `name` (el nombre dentro
                    // del módulo); el binding local usa `alias` si está.
                    for (name, alias) in names {
                        let kind = self.classify_named(idx, name)?;
                        let local = alias.clone().unwrap_or_else(|| name.clone());
                        self.bindings.insert(
                            local,
                            ResolvedBinding::Named {
                                module_index: idx,
                                item: name.clone(),
                                kind,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Resuelve los segmentos a un path absoluto.
    ///
    /// **Fase 9.y.3.b — orden de resolución** (paralelo al evaluator):
    /// 1. Si `segments` es de un único nombre y matchea una key del
    ///    `dep_registry`, devolvemos el `lib_entry` absoluto de la
    ///    dep directamente.
    /// 2. Si no, fallback: `["foo"]` → `<base>/foo.fitz`;
    ///    `["sub", "foo"]` → `<base>/sub/foo.fitz`.
    ///
    /// Decisión: las deps shadowean archivos locales con el mismo
    /// nombre (gana la dep si hay conflicto), igual que en el
    /// evaluator. Comportamiento explícito por design.
    fn resolve_path(&self, segments: &[String]) -> PathBuf {
        // Step 1 — dep registry shortcut.
        if segments.len() == 1 {
            if let Some(lib_entry) = self.dep_registry.get(&segments[0]) {
                return lib_entry.clone();
            }
        }

        // Step 2 — path relativo (pre-9.y.3.b behavior).
        let mut path = self.base_dir.clone();
        let n = segments.len();
        for (i, seg) in segments.iter().enumerate() {
            if i + 1 == n {
                path.push(format!("{}.fitz", seg));
            } else {
                path.push(seg);
            }
        }
        path
    }

    /// Carga un módulo: si ya está cacheado por path, devuelve el
    /// índice existente. Si no, lee + parse + check + codegen del
    /// módulo, lo agrega a `modules` y devuelve el nuevo índice.
    fn load_module(&mut self, segments: &[String]) -> Result<usize, FitzError> {
        if segments.is_empty() {
            return Err(loader_err("`import` con path vacío".to_string()));
        }
        let path = self.resolve_path(segments);
        let canonical = std::fs::canonicalize(&path).map_err(|_| {
            loader_err(format!(
                "no se encontró el módulo `{}` (buscado en `{}`)",
                segments.join("."),
                path.display()
            ))
        })?;

        if let Some(&idx) = self.by_path.get(&canonical) {
            return Ok(idx);
        }

        let source = std::fs::read_to_string(&canonical).map_err(|e| {
            loader_err(format!(
                "error leyendo el módulo `{}`: {}",
                canonical.display(),
                e
            ))
        })?;
        let tokens =
            crate::lexer::tokenize(&source).map_err(|e| loader_err(e.message.clone()))?;
        let module_program =
            crate::parser::parse(tokens).map_err(|e| loader_err(e.message.clone()))?;
        let (module_env, _types, _defs, type_errors) = check_program(&module_program);
        if !type_errors.is_empty() {
            return Err(loader_err(format!(
                "el módulo `{}` tiene errores de tipo: {}",
                segments.join("."),
                type_errors[0].message
            )));
        }

        // Restricción 5b.5: imports transitivos no soportados todavía.
        for stmt in &module_program {
            if matches!(stmt, Stmt::Import { .. } | Stmt::FromImport { .. }) {
                return Err(loader_err(format!(
                    "el módulo `{}` usa `import` propio: imports transitivos no soportados \
                     en 5b.5 (deuda residual). Workaround: aplaná los imports al main.",
                    segments.join(".")
                )));
            }
        }

        // Generar el código Rust del módulo (modo Module).
        let rust_content = generate_module_rs(&module_program, &module_env)?;

        let mod_name = segments.last().cloned().unwrap_or_default();
        let rel_path = mod_rel_path_from_segments(segments);

        // Extraer firmas para uso del importer.
        let (type_sigs, fn_sigs, const_sigs) =
            collect_module_sigs(&module_program, &module_env)?;

        let idx = self.modules.len();
        self.modules.push(LoadedModule {
            mod_name,
            rel_path,
            rust_content,
            type_sigs,
            fn_sigs,
            const_sigs,
        });
        self.by_path.insert(canonical, idx);
        Ok(idx)
    }

    /// Decide si un nombre importado vía `from foo import X` es un
    /// type, una fn o una const, inspeccionando las sigs del módulo
    /// cargado en `module_index`. Si el nombre no existe en el
    /// módulo, error.
    fn classify_named(&self, module_index: usize, name: &str) -> Result<NamedKind, FitzError> {
        let m = &self.modules[module_index];
        if m.type_sigs.contains_key(name) {
            Ok(NamedKind::Type)
        } else if m.fn_sigs.contains_key(name) {
            Ok(NamedKind::Fn)
        } else if m.const_sigs.contains_key(name) {
            Ok(NamedKind::Const)
        } else {
            Err(loader_err(format!(
                "el módulo `{}` no exporta `{}`",
                m.mod_name, name
            )))
        }
    }

    fn emit_mod_decls(&self, output: &mut String) {
        for m in &self.modules {
            // Para imports con subdirectorios, agregamos también un
            // `mod.rs` con `pub mod <last>;` en `into_mod_files`.
            // Acá solo declaramos el segmento root en `main.rs`.
            output.push_str(&format!("mod {};\n", root_segment_of(&m.rel_path)));
        }
        if !self.modules.is_empty() {
            output.push('\n');
        }
    }

    fn emit_use_decls(&self, output: &mut String) {
        // Ordenamos las entries por nombre para que el output sea
        // determinista (HashMap no garantiza orden).
        let mut entries: Vec<(&String, &ResolvedBinding)> = self.bindings.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (local, binding) in entries {
            if let ResolvedBinding::Named {
                module_index,
                item,
                kind,
            } = binding
            {
                let mod_name = &self.modules[*module_index].mod_name;
                // PreF8.4: si el local (key del HashMap) difiere del
                // item (nombre dentro del módulo), emitimos `as` para
                // que el Rust generado pueda referenciar el local
                // directamente (sin chocar con consts/types del
                // importer que tengan el mismo nombre que el item).
                let needs_alias = local != item;
                match kind {
                    NamedKind::Type => {
                        if needs_alias {
                            output.push_str(&format!(
                                "use {mod}::{{{item} as {local}, {item}Data as {local}Data}};\n",
                                mod = mod_name,
                                item = item,
                                local = local,
                            ));
                        } else {
                            output.push_str(&format!(
                                "use {mod}::{{{item}, {item}Data}};\n",
                                mod = mod_name,
                                item = item,
                            ));
                        }
                    }
                    NamedKind::Fn | NamedKind::Const => {
                        if needs_alias {
                            output.push_str(&format!(
                                "use {}::{} as {};\n",
                                mod_name, item, local
                            ));
                        } else {
                            output.push_str(&format!(
                                "use {}::{};\n",
                                mod_name, item
                            ));
                        }
                    }
                }
            }
        }
        if self.bindings.values().any(|b| matches!(b, ResolvedBinding::Named { .. })) {
            output.push('\n');
        }
    }

    /// Convierte los módulos cargados a `ModFile`s para que el caller
    /// los escriba a disco. Para imports con subdirectorios, suma un
    /// `mod.rs` en cada parent que faltaba.
    fn into_mod_files(self) -> Vec<ModFile> {
        let mut files: Vec<ModFile> = Vec::new();
        let mut declared_in_parent: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for m in self.modules {
            // Si el rel_path tiene un parent (subdirectorio), anotamos
            // que el parent necesita declarar `pub mod <last>;` en su
            // `mod.rs`.
            if let Some(parent) = m.rel_path.parent() {
                if !parent.as_os_str().is_empty() {
                    let leaf = m
                        .rel_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    declared_in_parent
                        .entry(parent.to_path_buf())
                        .or_default()
                        .push(leaf);
                }
            }
            files.push(ModFile {
                rel_path: m.rel_path,
                content: m.rust_content,
            });
        }
        // Materializar los `mod.rs` por cada parent agregado.
        for (parent, leaves) in declared_in_parent {
            let mut content = String::new();
            for leaf in leaves {
                content.push_str(&format!("pub mod {};\n", leaf));
            }
            files.push(ModFile {
                rel_path: parent.join("mod.rs"),
                content,
            });
        }
        files
    }
}

fn loader_err(msg: String) -> FitzError {
    FitzError::new(ErrorKind::TypeError, 0, 0, msg)
}

/// `["foo"]` → `foo.rs`; `["sub", "foo"]` → `sub/foo.rs`.
fn mod_rel_path_from_segments(segments: &[String]) -> PathBuf {
    let mut p = PathBuf::new();
    let n = segments.len();
    for (i, seg) in segments.iter().enumerate() {
        if i + 1 == n {
            p.push(format!("{}.rs", seg));
        } else {
            p.push(seg);
        }
    }
    p
}

/// Para `mod foo;` en main.rs, queremos `foo` (sin path); para
/// `sub/foo.rs` queremos `sub` (el root segment, que se declara como
/// `mod sub;` y trae el `sub/mod.rs` con `pub mod foo;` adentro).
fn root_segment_of(rel_path: &Path) -> String {
    rel_path
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .map(|s| s.trim_end_matches(".rs").to_string())
        .unwrap_or_default()
}

/// Genera `src/<mod>.rs` para un módulo importado. Modo: `Module`
/// (todo `pub`, sin `fn main()`, top-level `let X = literal` →
/// `pub const`/`pub static`).
fn generate_module_rs(program: &Program, env: &TypeEnv) -> Result<String, FitzError> {
    let mut ctx = CodegenCtx::new_for_module(env);
    ctx.pre_register_types(program)?;
    ctx.pre_register_fns(program)?;
    ctx.pre_register_top_lets(program)?;

    ctx.emit_prelude();

    // Particionar stmts top-level. Para módulos: type / fn / let
    // (con RHS literal). Cualquier otra cosa → error de codegen.
    let mut type_defs: Vec<&Stmt> = Vec::new();
    let mut top_fns: Vec<&Stmt> = Vec::new();
    let mut top_lets: Vec<&Stmt> = Vec::new();
    for s in program {
        match s {
            Stmt::TypeDef { .. } => type_defs.push(s),
            Stmt::FnDef { .. } => top_fns.push(s),
            Stmt::Assign { .. } => top_lets.push(s),
            other => {
                return Err(loader_err(format!(
                    "el módulo no soporta `{}` a nivel top: hoy permitimos solo `type`, \
                     `fn` y `let X = <literal>` (5b.5).",
                    stmt_kind(other)
                )));
            }
        }
    }

    for stmt in &type_defs {
        ctx.gen_type_def(stmt)?;
    }
    for stmt in top_fns {
        ctx.gen_top_fn(stmt)?;
    }
    for stmt in top_lets {
        ctx.gen_module_top_let(stmt)?;
    }
    // PreF8.3: las helpers `__default_<T>_<F>()` se emiten DESPUÉS de
    // los `top_lets` para que sus bodies (que pueden referenciar las
    // consts) las tengan en scope. Los `top_fns` también van antes
    // por consistencia con el patrón de declaración del módulo.
    for stmt in &type_defs {
        ctx.gen_type_default_helpers(stmt)?;
    }

    Ok(ctx.output)
}

fn stmt_kind(s: &Stmt) -> &'static str {
    match s {
        Stmt::Assign { .. } => "asignación",
        Stmt::Expr(..) => "expresión suelta",
        Stmt::Return(..) => "return",
        Stmt::ReturnStatus { .. } => "return con status",
        Stmt::While { .. } => "while",
        Stmt::Loop { .. } => "loop",
        Stmt::For { .. } => "for",
        Stmt::Break(_) => "break",
        Stmt::Continue(_) => "continue",
        Stmt::FnDef { .. } => "fn",
        Stmt::TypeDef { .. } => "type",
        Stmt::Import { .. } | Stmt::FromImport { .. } => "import",
        // Fase 9.0.1 (F15): defensa contra Error nodes — no debería
        // llegar acá porque `fitz build` usa `parse()` strict.
        Stmt::Error(_) => "nodo error",
    }
}

/// Recolecta las firmas exportadas de un módulo: tipos, fns y consts.
/// El loader las usa para resolver llamadas / accesos cross-module.
#[allow(clippy::type_complexity)] // tres maps por categoría es claro
fn collect_module_sigs(
    program: &Program,
    env: &TypeEnv,
) -> Result<
    (
        HashMap<String, TypeSig>,
        HashMap<String, FnSig>,
        HashMap<String, Type>,
    ),
    FitzError,
> {
    let mut type_sigs: HashMap<String, TypeSig> = HashMap::new();
    let mut fn_sigs: HashMap<String, FnSig> = HashMap::new();
    let mut const_sigs: HashMap<String, Type> = HashMap::new();

    for stmt in program {
        match stmt {
            Stmt::TypeDef { name, fields, .. } => {
                let id = match env.lookup(name) {
                    Some(id) => id,
                    None => continue,
                };
                let resolved = match &env.info(id).fields {
                    Some(fs) => fs.clone(),
                    None => continue,
                };
                let mut combined = Vec::with_capacity(resolved.len());
                for r in resolved {
                    let default = fields
                        .iter()
                        .find(|f| f.name == r.name)
                        .and_then(|f| f.default.clone());
                    combined.push(TypeSigField {
                        name: r.name,
                        type_: r.type_,
                        default,
                    });
                }
                type_sigs.insert(
                    name.clone(),
                    TypeSig {
                        id,
                        fields: combined,
                    },
                );
            }
            Stmt::FnDef {
                name,
                params,
                return_type,
                ..
            } => {
                let mut ps: Vec<Type> = Vec::with_capacity(params.len());
                for p in params {
                    let t = match &p.type_ {
                        Some(te) => resolve_type_expr(te, env).map_err(|e| {
                            loader_err(format!(
                                "fn `{}` del módulo: parámetro `{}`: {}",
                                name, p.name, e.message
                            ))
                        })?,
                        None => {
                            return Err(loader_err(format!(
                                "fn `{}` del módulo: parámetro `{}` necesita anotación \
                                 de tipo (deuda 5b.1).",
                                name, p.name
                            )));
                        }
                    };
                    ps.push(t);
                }
                let ret = match return_type {
                    Some(te) => resolve_type_expr(te, env).map_err(|e| {
                        loader_err(format!(
                            "fn `{}` del módulo: return type: {}",
                            name, e.message
                        ))
                    })?,
                    None => Type::Null,
                };
                fn_sigs.insert(name.clone(), FnSig { params: ps, ret });
            }
            Stmt::Assign { target, type_, value, .. } => {
                // Solo bindings simples a un Ident con RHS literal.
                let AssignTarget::Ident(name) = target else {
                    return Err(loader_err(
                        "el módulo no soporta asignación a campo a nivel top \
                         (solo `let X = <literal>`)"
                            .to_string(),
                    ));
                };
                let resolved_ty = match type_ {
                    Some(te) => resolve_type_expr(te, env).map_err(|e| {
                        loader_err(format!(
                            "let `{}` del módulo: anotación: {}",
                            name, e.message
                        ))
                    })?,
                    None => infer_literal_type(value).ok_or_else(|| {
                        loader_err(format!(
                            "let `{}` del módulo: la RHS debe ser un literal \
                             (Int/Float/Str/Bool/Null) o tenés que anotar el tipo (5b.5).",
                            name
                        ))
                    })?,
                };
                if !is_literal_expr(value) {
                    return Err(loader_err(format!(
                        "let `{}` del módulo: la RHS debe ser un literal — \
                         (Int/Float/Str/Bool/Null). Expresiones más complejas \
                         no se soportan a nivel top todavía (5b.5).",
                        name
                    )));
                }
                const_sigs.insert(name.clone(), resolved_ty);
            }
            _ => {}
        }
    }

    Ok((type_sigs, fn_sigs, const_sigs))
}

/// True si la expresión es un literal puro (Int/Float/Str/Bool/Null
/// sin sub-expresiones). El módulo solo permite estos en
/// `let X = ...` top-level.
fn is_literal_expr(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Int(_, _) | Expr::Float(_, _) | Expr::Bool(_, _) | Expr::Null(_) | Expr::Str(_, _)
    )
}

fn infer_literal_type(e: &Expr) -> Option<Type> {
    match e {
        Expr::Int(_, _) => Some(Type::Int),
        Expr::Float(_, _) => Some(Type::Float),
        Expr::Str(_, _) => Some(Type::Str),
        Expr::Bool(_, _) => Some(Type::Bool),
        Expr::Null(_) => Some(Type::Null),
        _ => None,
    }
}

/// Genera código Rust válido a partir de un programa Fitz tipado.
/// El programa debe haber pasado por `check_program` antes (las
/// anotaciones de tipo deben estar resueltas y consistentes).
///
/// Hoy es el path que usan los unit tests del codegen (single-file,
/// sin imports). El subcomando `build` pasa por `generate_project`
/// que es el wrapper multi-archivo.
///
/// Errores acá son de **codegen**: features fuera de scope (FnExpr
/// suelto, top-level no soportado, etc.). No revalidamos lo que el
/// checker ya hizo.
#[allow(dead_code)]
pub fn generate_rust(program: &Program, env: &TypeEnv) -> Result<String, FitzError> {
    let loader = ModuleLoader::new(PathBuf::from("."), crate::manifest::DepRegistry::new());
    let python_imports = collect_python_imports(program);
    generate_main_rs(program, env, &loader, &python_imports)
}

/// Genera el `src/main.rs` del Cargo project. Si hay módulos cargados,
/// emite los `mod foo;` y `use foo::{...};` correspondientes al inicio.
/// Si el programa tiene decoradores HTTP/`@server`, emite un `fn main()`
/// async con el Router + `axum::serve` (modo HTTP); si no, sigue el
/// flujo single-threaded clásico (modo CLI).
fn generate_main_rs(
    program: &Program,
    env: &TypeEnv,
    loader: &ModuleLoader,
    python_imports: &[PythonImport],
) -> Result<String, FitzError> {
    // Fase 8.7.1 — los imports Python se separan acá y se procesan
    // como bindings PyO3 (NO van al loader). El validador ya corrió
    // en `generate_project`; los llamados directos a `generate_main_rs`
    // (tests unit) lo reinvocan para no perder cobertura.
    validate_python_imports_for_codegen(program)?;
    let has_http = has_http_routes(program);
    let uses_async = program_uses_async(program);
    let uses_python = !python_imports.is_empty();

    let mut ctx = CodegenCtx::new(env);
    ctx.uses_async = uses_async;
    ctx.uses_python = uses_python;
    ctx.install_python_bindings(python_imports);
    ctx.install_loader_bindings(loader);
    ctx.pre_register_types(program)?;
    ctx.pre_register_fns(program)?;

    let partitioned = partition_program_stmts(program)?;
    resolve_state_var_types(&mut ctx, program, &partitioned.main_stmts, env, has_http)?;
    emit_main_rs_body(&mut ctx, program, loader, &partitioned, has_http)?;

    Ok(ctx.output)
}

/// Resultado de `partition_program_stmts`: cada stmt top-level cae en
/// una categoría, y los decoradores `@server(...)` quedan parseados a
/// la espera de la emisión del `fn main` HTTP.
struct PartitionedProgram<'a> {
    type_defs: Vec<&'a Stmt>,
    http_fns: Vec<&'a Stmt>,
    top_fns: Vec<&'a Stmt>,
    main_stmts: Vec<&'a Stmt>,
    server_config: Option<ServerConfigArgs>,
}

/// Particiona los stmts top-level del programa por categoría:
/// `type Foo {...}` (structs+alias+Display), `fn ...` con decorator
/// HTTP (handler + wrapper async), `fn ...` normal (pub fn top-level),
/// `Stmt::Import`/`FromImport` (mod/use decls del loader), y el resto
/// (cuerpo de `fn main()` CLI, o se ignora en modo HTTP). `fn main`
/// con decorators es especial: solo procesa decorators (típicamente
/// `@server`); NO se emite como Rust fn (colisiona con `fn main` del
/// crate generado).
///
/// Además valida que los decoradores sobre fns sean los soportados por
/// el codegen y extrae el `@server(...)` config (a lo sumo uno).
fn partition_program_stmts(program: &Program) -> Result<PartitionedProgram<'_>, FitzError> {
    let mut type_defs: Vec<&Stmt> = Vec::new();
    let mut http_fns: Vec<&Stmt> = Vec::new();
    let mut top_fns: Vec<&Stmt> = Vec::new();
    let mut main_stmts: Vec<&Stmt> = Vec::new();
    let mut server_config: Option<ServerConfigArgs> = None;
    for s in program {
        match s {
            Stmt::TypeDef { .. } => type_defs.push(s),
            Stmt::FnDef {
                name,
                decorators,
                ..
            } => {
                if decorators.is_empty() {
                    top_fns.push(s);
                } else {
                    // Separar `@server` de los `@get`/`@post`/etc.
                    let mut http_decos = false;
                    for d in decorators {
                        // 7.5: `@server` acepta kwargs (delegado a
                        // `parse_server_decorator`).
                        // 7.6: `@header(name="X")` también acepta kwargs;
                        // los valida `collect_headers` en runtime y el
                        // wrapper del codegen los procesa. Los
                        // decoradores HTTP de ruta `@get/@post/@put/@delete`
                        // siguen sin aceptar kwargs.
                        if !matches!(d.name.as_str(), "server" | "header") {
                            if let Some((key, _)) = d.kwargs.first() {
                                return Err(FitzError::new(
                                    ErrorKind::TypeError,
                                    0,
                                    0,
                                    format!(
                                        "decorator `@{}` sobre fn `{}`: el argumento por nombre '{}=...' no está soportado",
                                        d.name, name, key,
                                    ),
                                ));
                            }
                        }
                        match d.name.as_str() {
                            "get" | "post" | "put" | "delete" => http_decos = true,
                            "server" => {
                                server_config = Some(parse_server_decorator(&d.args, &d.kwargs)?);
                            }
                            // `@header` (Fase 7.6): no aporta a la
                            // categorización del codegen ni configura
                            // server; el wrapper HTTP lo procesa por
                            // separado vía `headers_from_decorators`.
                            // Acá solo lo aceptamos como decorator válido.
                            "header" => {}
                            // `@middleware(...)` (MW.3): no aporta a la
                            // categorización del codegen acá; el wrapper
                            // HTTP lo procesa por separado al generar el
                            // handler async. Solo validamos que sea un
                            // decorator HTTP de ruta válido en otro lugar.
                            "middleware" => {}
                            other => {
                                return Err(FitzError::new(
                                    ErrorKind::TypeError,
                                    0,
                                    0,
                                    format!(
                                        "decorator `@{}` sobre fn `{}` no soportado en codegen (hoy: @get/@post/@put/@delete/@server/@header)",
                                        other, name
                                    ),
                                ));
                            }
                        }
                    }
                    // 5b.6: `fn main` es especial — el codegen genera
                    // su propia `fn main` async cuando hay HTTP, y la
                    // del usuario solo aporta decorators (típico:
                    // `@server(3000) fn main() => 0` como placeholder).
                    // Si el usuario pone un decorator HTTP de ruta
                    // (`@get`/`@post`/...) sobre `fn main`, lo
                    // ignoraríamos silenciosamente — confuso. Rechazar
                    // explícito.
                    if name == "main" && http_decos {
                        return Err(FitzError::new(
                            ErrorKind::TypeError,
                            0,
                            0,
                            "`fn main` solo admite `@server(...)` como decorator. \
                             Para registrar rutas HTTP, definí los handlers en fns \
                             con nombre distinto a `main` (ej.: `fn index`, `fn get_user`)."
                                .to_string(),
                        ));
                    }
                    if http_decos {
                        http_fns.push(s);
                    } else if name != "main" {
                        // fn con solo `@server` y nombre distinto a main:
                        // raro, pero lo emitimos como pub fn igual.
                        top_fns.push(s);
                    }
                    // Si es `fn main` con solo `@server`: NO se emite.
                }
            }
            Stmt::Import { .. } | Stmt::FromImport { .. } => {}
            _ => main_stmts.push(s),
        }
    }
    Ok(PartitionedProgram {
        type_defs,
        http_fns,
        top_fns,
        main_stmts,
        server_config,
    })
}

/// F11 + F17.4b: state HTTP compartido vía `static LazyLock<Arc<Mutex<T>>>`.
/// Detecta las vars top-level referenciadas por handlers HTTP y resuelve
/// el tipo de cada una (anotación si la tiene; si no, inferencia via
/// `gen_expr` sobre buffer temporal). Llenamos `ctx.fn_state_deps` y
/// `ctx.state_var_types` para que la emisión posterior los consulte.
fn resolve_state_var_types(
    ctx: &mut CodegenCtx,
    program: &Program,
    main_stmts: &[&Stmt],
    env: &TypeEnv,
    has_http: bool,
) -> Result<(), FitzError> {
    let (shared_state_order, fn_deps) = if has_http {
        detect_shared_state(program)
    } else {
        (Vec::new(), HashMap::new())
    };
    ctx.fn_state_deps = fn_deps;
    // Los tipos resueltos de cada state var los inferimos al re-visitar
    // los `Stmt::Assign` correspondientes (necesitamos el `Type` para
    // emitir el alias del thread_local con tipo concreto). Lo hacemos
    // acá porque el ctx ya tiene los tipos custom pre-registrados.
    for s in main_stmts {
        if let Stmt::Assign {
            target: AssignTarget::Ident(name),
            type_,
            value,
            ..
        } = s
        {
            if shared_state_order.contains(name) {
                let resolved = match type_ {
                    Some(te) => resolve_type_expr(te, env).map_err(|e| {
                        FitzError::new(
                            ErrorKind::TypeError,
                            0,
                            0,
                            format!(
                                "state HTTP `{}`: anotación no resuelve: {}",
                                name, e.message
                            ),
                        )
                    })?,
                    None => {
                        // Sin anotación, inferimos del valor inicial.
                        // Hacemos un pre-pass: el ctx aún no tiene los
                        // bodies emitidos, pero `gen_expr` no muta nada
                        // que no podamos descartar (output va a un
                        // buffer temporal). Como atajo, usamos
                        // `with_temp_output` para una emisión "fantasma"
                        // que solo nos da el tipo.
                        let (_out, result) = ctx.with_temp_output(|c| c.gen_expr(value));
                        let (_code, ty) = result?;
                        ty
                    }
                };
                ctx.state_var_types.insert(name.clone(), resolved);
            }
        }
    }
    Ok(())
}

/// Emite el cuerpo del `main.rs`: preludio, mod/use decls del loader,
/// runtime HTTP (si aplica), type defs (+ http impls), fns top-level y
/// handlers HTTP, y finalmente el `fn main()` (CLI o HTTP).
fn emit_main_rs_body(
    ctx: &mut CodegenCtx,
    program: &Program,
    loader: &ModuleLoader,
    p: &PartitionedProgram<'_>,
    has_http: bool,
) -> Result<(), FitzError> {
    ctx.emit_prelude();
    // Fase 8.7.1: el preludio Python va DESPUÉS del preludio base
    // (que ya emitió `use std::sync::{Arc, Mutex};`) y ANTES de los
    // mod decls / use decls de módulos Fitz. El struct
    // `__FitzPyObject` queda en scope global del main.rs para
    // referenciarse desde cualquier `rust_type_for(Type::PyAny)`.
    ctx.emit_python_prelude();
    // Fase 8.7.2: bindings Python globales (static + getter) emitidos
    // al top-level del crate para que cualquier fn los pueda referenciar.
    let py_imports = std::mem::take(&mut ctx.python_imports_ordered);
    ctx.emit_python_bindings_top_level(&py_imports);
    ctx.python_imports_ordered = py_imports;
    loader.emit_mod_decls(&mut ctx.output);
    loader.emit_use_decls(&mut ctx.output);

    // 5b.6: cuando hay HTTP emitimos los helpers de serialización
    // (`__ToFitzJson` / `__FromFitzJson`) antes de los tipos custom,
    // porque los `impl` de cada `type` los referencian.
    if has_http {
        ctx.emit_http_runtime_prelude();
    }

    for stmt in &p.type_defs {
        ctx.gen_type_def(stmt)?;
        if has_http {
            ctx.gen_type_http_impls(stmt)?;
        }
    }
    for stmt in &p.http_fns {
        ctx.gen_top_fn(stmt)?;
    }
    for stmt in &p.top_fns {
        ctx.gen_top_fn(stmt)?;
    }

    if has_http {
        // Emitir un wrapper `async fn __handler_<name>` por cada handler.
        for stmt in &p.http_fns {
            ctx.gen_http_handler_wrapper(stmt)?;
        }
        // `#[tokio::main] async fn main` con Router + serve.
        // Fase 7.5: pasamos `program` para que adentro pueda
        // pre-computar el schema OpenAPI desde el AST.
        ctx.gen_http_main(&p.http_fns, &p.server_config, &p.main_stmts, program)?;
    } else {
        // Modo CLI: cuerpo de `fn main()` con el resto de stmts.
        ctx.gen_main(&p.main_stmts)?;
    }
    Ok(())
}

/// Valores parseados de `@server(port?, host?)`. Defaults aplicados
/// (puerto 3000, host "127.0.0.1", docs habilitados) si los args
/// no están.
#[derive(Debug, Clone)]
struct ServerConfigArgs {
    port: u16,
    host: String,
    /// Fase 7.5: `false` apaga el auto-register de `/openapi.json` y
    /// `/docs` en el binario nativo. Default `true`, opt-out con
    /// `@server(docs=false)`.
    enable_docs: bool,
    /// Mini-fase Q.2: override del `info.version` del schema OpenAPI
    /// generado en build-time. None → "0.1.0". Seteado con
    /// `@server(api_version="X.Y.Z")`.
    api_version: Option<String>,
}

impl Default for ServerConfigArgs {
    fn default() -> Self {
        ServerConfigArgs {
            port: 3000,
            host: "127.0.0.1".to_string(),
            enable_docs: true,
            api_version: None,
        }
    }
}

/// Parsea los args de un decorator `@server(port?, host?, docs=Bool?)`.
/// Validaciones:
///   - Hasta 2 args positionals: `(port: Int)` o `(port: Int, host: Str)`.
///   - Port entre 1 y 65535.
///   - Host parsea como `IpAddr` (sin DNS). Validación delegada al runtime
///     porque acá solo tenemos un literal Str.
///   - Kwargs: solo `docs: Bool` por ahora (Fase 7.5). Otros kwargs son
///     error con el mismo mensaje que el runtime ("kwarg X no reconocido").
fn parse_server_decorator(
    args: &[Expr],
    kwargs: &[(String, Expr)],
) -> Result<ServerConfigArgs, FitzError> {
    let mut cfg = ServerConfigArgs::default();
    if args.len() > 2 {
        return Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "@server(...): admite hasta 2 args positionals (port, host), recibió {}",
                args.len()
            ),
        ));
    }
    if let Some(port_expr) = args.first() {
        let Expr::Int(n, _) = port_expr else {
            return Err(FitzError::new(
                ErrorKind::TypeError,
                0,
                0,
                "@server: el primer arg (port) debe ser un Int literal".to_string(),
            ));
        };
        if *n < 1 || *n > 65535 {
            return Err(FitzError::new(
                ErrorKind::TypeError,
                0,
                0,
                format!("@server: port fuera de rango [1, 65535]: {}", n),
            ));
        }
        cfg.port = *n as u16;
    }
    if let Some(host_expr) = args.get(1) {
        let Expr::Str(s, _) = host_expr else {
            return Err(FitzError::new(
                ErrorKind::TypeError,
                0,
                0,
                "@server: el segundo arg (host) debe ser un Str literal".to_string(),
            ));
        };
        // No validamos IP acá: rustc no puede hacerlo en compile time.
        // El parse se hace en runtime; si falla, axum/tokio reportarán.
        cfg.host = s.clone();
    }
    // Fase 7.5: kwargs `docs: Bool`. Mini-fase Q.2: `api_version: Str`.
    for (key, value_expr) in kwargs {
        match key.as_str() {
            "docs" => {
                let Expr::Bool(b, _) = value_expr else {
                    return Err(FitzError::new(
                        ErrorKind::TypeError,
                        0,
                        0,
                        format!(
                            "@server: el kwarg 'docs' debe ser Bool literal, recibió {:?}",
                            value_expr
                        ),
                    ));
                };
                cfg.enable_docs = *b;
            }
            "api_version" => match value_expr {
                Expr::Str(s, _) if !s.is_empty() => {
                    cfg.api_version = Some(s.clone());
                }
                Expr::Str(_, _) => {
                    return Err(FitzError::new(
                        ErrorKind::TypeError,
                        0,
                        0,
                        "@server: el kwarg 'api_version' no puede ser un string vacío".to_string(),
                    ));
                }
                _ => {
                    return Err(FitzError::new(
                        ErrorKind::TypeError,
                        0,
                        0,
                        format!(
                            "@server: el kwarg 'api_version' debe ser Str literal, recibió {:?}",
                            value_expr
                        ),
                    ));
                }
            },
            other => {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    format!(
                        "@server: kwarg '{}' no reconocido. Soportados: docs, api_version.",
                        other
                    ),
                ));
            }
        }
    }
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// CodegenCtx
// ---------------------------------------------------------------------------

/// Modo del codegen: cambia los detalles de emisión sin duplicar
/// código. `Main` produce `src/main.rs` (con `fn main()` y los
/// items sin `pub`); `Module` produce `src/<nombre>.rs` (con `pub`
/// en todo lo top-level, sin `fn main()`, y `let X = literal` a
/// nivel mod traducido a `pub const`/`pub static`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenMode {
    Main,
    Module,
}

/// Copia portable de las firmas exportadas de un módulo. Se llena en
/// `install_loader_bindings` y vive adentro del CodegenCtx.
#[derive(Debug, Clone)]
struct LoadedModuleSigs {
    mod_name: String,
    type_sigs: HashMap<String, TypeSig>,
    fn_sigs: HashMap<String, FnSig>,
    const_sigs: HashMap<String, Type>,
}

struct CodegenCtx<'a> {
    env: &'a TypeEnv,
    output: String,
    indent: usize,
    mode: GenMode,
    /// Stack de scopes de variables locales: nombre → tipo Fitz.
    /// El codegen usa esto para inferir tipos en expresiones y
    /// para decidir entre `let mut` (primera asignación) y `=`
    /// (reasignación).
    scopes: Vec<HashMap<String, Type>>,
    /// Firmas de las funciones top-level: nombre → (params, ret).
    /// Pre-registrado antes de emitir cuerpos, para que las
    /// llamadas resuelvan el ret type sin importar el orden.
    fn_sigs: HashMap<String, FnSig>,
    /// Firmas de los tipos custom declarados en el programa:
    /// nombre → (TypeId, lista de campos con tipo resuelto + default
    /// AST). Pre-registrado antes de emitir structs, para que las
    /// instancias y los field accesses puedan resolver tipos de
    /// campo sin volver a iterar el AST.
    type_sigs: HashMap<String, TypeSig>,
    /// Fields resueltos por TypeId. Lo usa `gen_field_access` y
    /// `gen_field_assign` para encontrar los campos de un tipo
    /// importado (su TypeEnv del checker tiene el id pero sin
    /// fields — el codegen los enriquece desde el módulo cargado).
    fields_by_id: HashMap<TypeId, Vec<ResolvedField>>,
    /// Consts/statics top-level del propio módulo (5b.5): nombre →
    /// tipo Fitz. Sirven para que el body de una fn del módulo pueda
    /// referenciarlas. En main mode, queda vacío (los `let` top-level
    /// son vars locales adentro de `fn main()`).
    own_consts: HashMap<String, Type>,
    /// Bindings de módulos importados: nombre visible (último segmento
    /// del path para `import foo`, o el identificador en `from foo
    /// import X`) → `ResolvedBinding` con el índice del módulo cargado.
    /// Sirve para resolver `foo.greet(...)` → `foo::greet(...)` Rust,
    /// `foo.PREFIX` → `foo::PREFIX`, y para conocer los fields de
    /// `User` cuando `from foo import User` se usa en `User { ... }`.
    module_bindings: HashMap<String, ResolvedBinding>,
    /// Firmas de módulos cargados, indexadas por `module_index` que
    /// guarda `ResolvedBinding`. El ctx se queda con una copia para no
    /// necesitar una referencia al loader (que viviría con un lifetime
    /// distinto al `&'a TypeEnv` de arriba).
    loaded_modules: Vec<LoadedModuleSigs>,
    /// Stack con el return type esperado de cada fn en curso. Se
    /// pushea al entrar a una fn top-level o a un callback inline
    /// (FnExpr); se consulta desde `gen_expr` para validar que el
    /// operador `?` (`Try`) solo aparezca dentro de fns que
    /// retornen `Result<T>`. El checker 5.3.3 ya valida lo mismo,
    /// pero como defensa en profundidad y para emitir errores
    /// claros del codegen, lo replicamos. Vacío fuera de toda fn
    /// (top-level del archivo, donde `?` no aplica).
    ret_stack: Vec<Type>,
    /// F11: nombres de vars top-level que el codegen detectó como
    /// **state HTTP compartido** (referenciadas desde al menos una fn
    /// con decorator HTTP). Indexado por nombre → tipo resuelto.
    /// Vacío para programas no-HTTP o HTTP sin state compartido.
    /// Las vars se emiten como `thread_local!` con la representación
    /// usual `Arc<Mutex<...>>` (no cambia la repr de tipos) y se
    /// materializan al inicio de cada fn que las referencia.
    /// El tokio runtime queda `flavor = "current_thread"` para que el
    /// thread_local actúe como global de verdad.
    state_var_types: HashMap<String, Type>,
    /// F11: para cada fn (top-level, helper, o handler), los nombres
    /// de los state vars que su body referencia directo. Lo usamos al
    /// inicio del body para emitir `let <name> = __FITZ_STATE_<NAME>
    /// .with(|s| s.clone());` (Rc clone — preserva aliasing). El orden
    /// es alfabético para que el output sea determinista.
    fn_state_deps: HashMap<String, Vec<String>>,
    /// Status codes custom: `true` mientras estamos generando el body
    /// de una fn HTTP que contiene al menos un `Stmt::ReturnStatus`.
    /// El return type Rust se cambia a `__FitzResponse` y todos los
    /// returns (normales y con status) se envuelven en esa struct.
    /// El handler wrapper también lee este flag (vía
    /// `http_handlers_returning_response`) para decidir cómo
    /// destructurar la response. Default false.
    response_mode: bool,
    /// MW.3: `true` mientras estamos emitiendo el body de una fn marcada
    /// como middleware (su nombre está en `middleware_fn_names`). Cambia
    /// la emisión de `Stmt::ReturnStatus` y `Stmt::Return` para
    /// envolver en `Some(__FitzResponse { ... })` y `None`
    /// respectivamente, alineado con el return type
    /// `Option<__FitzResponse>` de la firma. Default false. Combinable
    /// con `response_mode` (el flag general "esta fn produce
    /// __FitzResponse") — middleware tiene ambos en true cuando su
    /// body contiene Stmt::ReturnStatus.
    in_middleware_fn: bool,
    /// Nombres de handlers HTTP que retornan `__FitzResponse` (porque
    /// su body contiene al menos un `Stmt::ReturnStatus`). El handler
    /// wrapper lo consulta para emitir el destructuring apropiado en
    /// vez del path normal de serialización.
    http_handlers_returning_response: std::collections::HashSet<String>,
    /// Mini-fase MW.3: nombres de fns Fitz que aparecen como
    /// `@middleware(name)` en algún FnDef del programa. Pre-scaneado en
    /// `pre_register_fn_signatures` (o equivalente). Esas fns se
    /// codegenan distinto:
    ///   - Return type Rust: `Option<__FitzResponse>` (gate-only:
    ///     `None` = la chain continúa, `Some(resp)` = short-circuit).
    ///   - `return null`/sin return → `None` (default si el body
    ///     cae fuera del último stmt sin Stmt::Return).
    ///   - `return <status> { body }` (Stmt::ReturnStatus) →
    ///     `Some(__FitzResponse { ... })`.
    ///
    /// El handler wrapper invoca cada uno en orden y short-circuita
    /// con la primera response.
    middleware_fn_names: std::collections::HashSet<String>,
    /// Fase 6.6: `true` si el programa usa async — cualquier `async fn`
    /// declarada, `.await` adentro de un body, o llamada al builtin
    /// `sleep`. Habilita el preludio `__fitz_sleep`, el `#[tokio::main]`
    /// sobre `fn main()` CLI, y el feature `time` en el Cargo.toml.
    /// Se setea en `generate_main_rs` antes de emit_prelude.
    uses_async: bool,
    /// Fase 8.7.1: `true` si el programa tiene al menos un import
    /// Python (`from python import X` / `import python.X`). Habilita
    /// el preludio Python (`__FitzPyObject` + helpers PyO3) y la
    /// emisión de bindings como vars locales del main body.
    uses_python: bool,
    /// Fase 8.7.1: bindings Python detectados en el programa. Cada
    /// entry mapea `binding_name` → `dotted_path` Python. Se consulta
    /// desde `gen_expr` (Ident) para tipar el ident como
    /// `Type::PyAny` y desde `emit_python_bindings` para emitir
    /// `let <name> = __fitz_py_import("<dotted>");` al inicio del
    /// main body.
    python_bindings: HashMap<String, String>,
    /// Fase 8.7.1: orden de declaración de los imports Python. El
    /// HashMap `python_bindings` no preserva orden; este Vec sí, para
    /// que `emit_python_bindings` los emita en el mismo orden que el
    /// usuario los escribió (matchea posibles side-effects al import
    /// de un módulo Python que registra hooks globales).
    python_imports_ordered: Vec<PythonImport>,
}

#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<Type>,
    ret: Type,
}

/// Info de un tipo custom durante el codegen. Combina los datos
/// resueltos del checker (tipos por campo) con los defaults del AST
/// (que el checker no conserva): los necesitamos para inline-ar los
/// defaults en cada struct literal que omita el campo.
#[derive(Debug, Clone)]
struct TypeSig {
    #[allow(dead_code)]
    id: TypeId,
    fields: Vec<TypeSigField>,
}

#[derive(Debug, Clone)]
struct TypeSigField {
    name: String,
    type_: Type,
    /// Default expr del campo, tomado del AST de `Stmt::TypeDef`.
    /// `None` si el campo no tenía default declarado.
    default: Option<Expr>,
}

/// Info resuelta de un handler HTTP. La produce
/// `resolve_handler_signature` y la consumen los `emit_*` helpers del
/// codegen del wrapper async. Captura todo el estado intermedio que
/// antes vivía como vars locales adentro de `gen_http_handler_wrapper`.
struct HandlerSig {
    name: String,
    is_async: bool,
    /// "GET" / "POST" / "PUT" / "DELETE" — derivado del decorator HTTP.
    http_method: &'static str,
    /// Path template normalizado tal como llega al runtime axum (con
    /// `{name}` para los path params).
    path: String,
    /// Params categorizados: nombre Fitz + tipo resuelto.
    path_params: Vec<(String, Type)>,
    query_params: Vec<(String, Type)>,
    /// Headers: `(http_name, fitz_param_name, is_nullable)`.
    header_params: Vec<(String, String, bool)>,
    /// Body: `Some(nombre, tipo)` si el handler declara body.
    body_param: Option<(String, Type)>,
    /// Todos los params resueltos en orden original (para la llamada
    /// final `handler(args...)`).
    resolved_params: Vec<(String, Type)>,
    /// `true` si el return type del handler es `Result<T>` — afecta el
    /// dispatch en `emit_handler_dispatch_and_response`.
    returns_result: bool,
    /// MW.3: nombres de las fns user-middleware encadenadas en orden de
    /// declaración. Vacío si no hay `@middleware(fn)`.
    mw_user_fns: Vec<String>,
    /// MW.2/Q.3: config CORS si la ruta declara `@middleware(cors(...))`.
    mw_cors: Option<BuildCorsConfig>,
    has_middleware: bool,
    has_cors: bool,
}

impl<'a> CodegenCtx<'a> {
    fn new(env: &'a TypeEnv) -> Self {
        Self {
            env,
            output: String::new(),
            indent: 0,
            mode: GenMode::Main,
            scopes: vec![HashMap::new()],
            fn_sigs: HashMap::new(),
            type_sigs: HashMap::new(),
            fields_by_id: HashMap::new(),
            own_consts: HashMap::new(),
            module_bindings: HashMap::new(),
            loaded_modules: Vec::new(),
            ret_stack: Vec::new(),
            state_var_types: HashMap::new(),
            fn_state_deps: HashMap::new(),
            response_mode: false,
            in_middleware_fn: false,
            http_handlers_returning_response: std::collections::HashSet::new(),
            middleware_fn_names: std::collections::HashSet::new(),
            uses_async: false,
            uses_python: false,
            python_bindings: HashMap::new(),
            python_imports_ordered: Vec::new(),
        }
    }

    fn new_for_module(env: &'a TypeEnv) -> Self {
        let mut ctx = Self::new(env);
        ctx.mode = GenMode::Module;
        ctx
    }

    fn pub_prefix(&self) -> &'static str {
        match self.mode {
            GenMode::Main => "",
            GenMode::Module => "pub ",
        }
    }

    /// Lee los bindings y las firmas de módulos del loader, dejando al
    /// ctx autocontenido (sin necesidad de mantener viva una referencia
    /// al loader). Las firmas se copian — son pocos KBs por módulo, y
    /// el TypeEnv del importer las usa para resolver tipos / consts /
    /// fns cross-module.
    fn install_loader_bindings(&mut self, loader: &ModuleLoader) {
        for m in &loader.modules {
            self.loaded_modules.push(LoadedModuleSigs {
                mod_name: m.mod_name.clone(),
                type_sigs: m.type_sigs.clone(),
                fn_sigs: m.fn_sigs.clone(),
                const_sigs: m.const_sigs.clone(),
            });
        }
        for (name, binding) in &loader.bindings {
            self.module_bindings.insert(name.clone(), binding.clone());
        }
    }

    /// Resuelve `<ns>.<field>` (namespace access) cuando `ns` es un
    /// módulo importado via `import foo`. Devuelve `(código Rust,
    /// tipo Fitz)`. Si el módulo no exporta `field`, `None`.
    fn resolve_namespace_field(&self, ns: &str, field: &str) -> Option<(String, Type)> {
        let idx = match self.module_bindings.get(ns)? {
            ResolvedBinding::Namespace { module_index } => *module_index,
            _ => return None,
        };
        let m = self.loaded_modules.get(idx)?;
        if let Some(sig) = m.fn_sigs.get(field) {
            Some((
                format!("{}::{}", m.mod_name, field),
                Type::Function {
                    params: sig.params.clone(),
                    ret: Box::new(sig.ret.clone()),
                },
            ))
        } else {
            m.const_sigs
                .get(field)
                .map(|ty| (format!("{}::{}", m.mod_name, field), ty.clone()))
        }
    }

    /// Para `<ns>.<fn_name>(args)`: devuelve `(path Rust, firma)` si
    /// existe. Si `ns` es módulo pero no tiene esa fn, `None`.
    fn resolve_namespace_call(&self, ns: &str, fn_name: &str) -> Option<(String, FnSig)> {
        let idx = match self.module_bindings.get(ns)? {
            ResolvedBinding::Namespace { module_index } => *module_index,
            _ => return None,
        };
        let m = self.loaded_modules.get(idx)?;
        m.fn_sigs
            .get(fn_name)
            .map(|sig| (format!("{}::{}", m.mod_name, fn_name), sig.clone()))
    }

    // --- emit helpers -----------------------------------------------------

    fn emit(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn emit_indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("    ");
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare_var(&mut self, name: String, ty: Type) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, ty);
        }
    }

    fn lookup_var(&self, name: &str) -> Option<&Type> {
        for s in self.scopes.iter().rev() {
            if let Some(t) = s.get(name) {
                return Some(t);
            }
        }
        None
    }

    fn var_in_any_scope(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains_key(name))
    }

    /// Fase 8.7.3 — detecta el patrón `<py_call>.await` y emite
    /// `__fitz_py_invoke_await(&callable, |py| Ok(vec![<args>])).await`.
    /// Devuelve `Some(code)` si el inner del await es un call sobre
    /// receptor `PyAny` (directo o method call); `None` si es un await
    /// regular (Future Fitz nativo, builtin `sleep`, etc.) que sigue
    /// el path normal del `.await` 6.6.
    ///
    /// Si el inner del await es `Expr::Try(<py_call>)`, también lo
    /// despachamos: el `?` se aplica DESPUÉS del await sobre el
    /// `Result<PyAny>` resultante. Sintaxis válida:
    /// `let v: Float = py_async_fn(arg)?.await?`
    fn try_gen_python_await(
        &mut self,
        inner: &Expr,
    ) -> Result<Option<(String, Type)>, FitzError> {
        // Patrón canónico Fitz: `<py_call>?.await`. El AST es
        // `Await(Try(Call con callee/method PyAny))`. El `?` desempaca
        // el `Result<Any>` que el call envuelve (per 8.4 → 8.3); el
        // `.await` ejecuta la corutina vía el bridge tokio ↔ asyncio.
        // En codegen lo emitimos como un helper combinado
        // `__fitz_py_invoke_await(...).await?` (el `?` Rust al final
        // propaga excepciones asyncio del await mismo). Tipo Fitz
        // resultante: `PyAny` (gradual). El sitio destino puede
        // aplicar coerción primitiva si pide un tipo concreto.
        //
        // Paridad bit-a-bit con `fitz run`: el intérprete usa el mismo
        // patrón `?.await` (el evaluator rechaza `<call>.await` directo
        // con "se esperaba Future"). El checker 8.7.3 también rechaza
        // estáticamente.
        if let Expr::Try(try_inner, _) = inner {
            if let Expr::Call { callee, args, .. } = try_inner.as_ref() {
                if let Some(code) =
                    self.try_gen_python_call_await(callee.as_ref(), args.as_slice())?
                {
                    return Ok(Some((code, Type::PyAny)));
                }
            }
        }
        Ok(None)
    }

    /// Helper: si el call `<callee>(<args>)` es sobre receptor PyAny,
    /// emite `__fitz_py_invoke_await(...).await?` que combina call +
    /// await + propagación. Devuelve `None` si no aplica (no es call
    /// PyAny). El sufijo `?` Rust propaga excepciones asyncio del
    /// await mismo — el patrón Fitz `?.await` genera esta forma
    /// porque el operador `?` Fitz del lado izquierdo ya está
    /// implícito en el helper combinado.
    fn try_gen_python_call_await(
        &mut self,
        callee: &Expr,
        args: &[Expr],
    ) -> Result<Option<String>, FitzError> {
        // Method call sobre PyAny: `<obj>.method(args)` con `obj` PyAny.
        if let Expr::Field { object, field, .. } = callee {
            let obj_ty = self.peek_expr_type(object)?;
            if matches!(obj_ty, Type::PyAny) {
                let (obj_code, _) = self.gen_expr(object)?;
                let args_code = self.gen_python_call_args(args)?;
                let code = format!(
                    "__fitz_py_invoke_await(&__fitz_py_get_attr_obj(&{obj}, {name}), |py| {{ Ok(vec![{args}]) }}).await?",
                    obj = obj_code,
                    name = rust_str_literal(field),
                    args = args_code,
                );
                return Ok(Some(code));
            }
        }
        // Call directo sobre Ident PyAny: `<py_callable>(args)`.
        if let Expr::Ident(name, _) = callee {
            if self.python_bindings.contains_key(name)
                || matches!(self.lookup_var(name), Some(Type::PyAny))
            {
                let (callee_code, _) = self.gen_expr(callee)?;
                let args_code = self.gen_python_call_args(args)?;
                let code = format!(
                    "__fitz_py_invoke_await(&{callee}, |py| {{ Ok(vec![{args}]) }}).await?",
                    callee = callee_code,
                    args = args_code,
                );
                return Ok(Some(code));
            }
        }
        Ok(None)
    }

    /// Helper read-only: sintetiza el tipo Fitz que `gen_expr` devolvería
    /// para `e`, sin emitir código. Lo usa `try_gen_python_call_await`
    /// para inspeccionar el tipo del receptor sin "consumirlo" (la
    /// emisión real ocurre en `gen_expr` después).
    fn peek_expr_type(&mut self, e: &Expr) -> Result<Type, FitzError> {
        match e {
            Expr::Ident(name, _) => {
                if self.python_bindings.contains_key(name) {
                    return Ok(Type::PyAny);
                }
                if let Some(t) = self.lookup_var(name).cloned() {
                    return Ok(t);
                }
                Ok(Type::Any)
            }
            Expr::Field { object, .. } => {
                let inner = self.peek_expr_type(object)?;
                if matches!(inner, Type::PyAny) {
                    return Ok(Type::PyAny);
                }
                Ok(Type::Any)
            }
            _ => Ok(Type::Any),
        }
    }

    /// Fase 8.7.2 — emite los args de un call Python como una lista
    /// de expresiones `<arg_code>.__fitz_to_py(py, "arg<i>")?` separadas
    /// por comas, listas para usarse adentro de `vec![...]`. El path
    /// breadcrumb (`arg0`, `arg1`, ...) matchea el del intérprete
    /// (`value_to_py` con `path: &str`).
    ///
    /// Cada arg pasa por `gen_expr` para obtener su código Rust + tipo
    /// Fitz; el trait `__FitzToPy` está impl para todos los tipos
    /// soportados (primitivos, PyObject, List, Map, Option, y los
    /// nominales que el codegen emite con `gen_type_python_impls`).
    fn gen_python_call_args(&mut self, args: &[Expr]) -> Result<String, FitzError> {
        let mut pieces: Vec<String> = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let (code, _ty) = self.gen_expr(a)?;
            // El bind a una var local permite que el `&` de
            // `__fitz_to_py(...)` (que es método `&self`) no requiera
            // un temporary que sobreviva la expression statement.
            pieces.push(format!(
                "{{ let __a = {code}; __a.__fitz_to_py(py, {path})? }}",
                code = code,
                path = rust_str_literal(&format!("arg{}", i)),
            ));
        }
        Ok(pieces.join(", "))
    }

    /// Fase 8.7.1 — registra los bindings Python detectados por
    /// `collect_python_imports`. Guarda el mapeo `binding_name →
    /// dotted_path` para que `emit_python_bindings` lo consuma al
    /// inicio del main body. Los nombres se registran en el scope
    /// global del CodegenCtx con `Type::PyAny` para que `gen_expr`
    /// los trate como opacos PyAny (la auto-coerción primitiva
    /// dispara cuando el contexto destino es concreto).
    fn install_python_bindings(&mut self, imports: &[PythonImport]) {
        for imp in imports {
            self.python_bindings
                .insert(imp.binding_name.clone(), imp.dotted_path.clone());
            self.python_imports_ordered.push(imp.clone());
            // Registrar como var en el scope raíz para que el lookup
            // en `gen_expr::Ident` la encuentre con tipo PyAny.
            if let Some(top) = self.scopes.first_mut() {
                top.insert(imp.binding_name.clone(), Type::PyAny);
            }
        }
    }

    /// Fase 8.7.1 — emite el preludio Python: `use pyo3::prelude::*;`,
    /// `struct __FitzPyObject(Arc<Py<PyAny>>)` con Clone/Display/Debug/
    /// PartialEq, y helpers `__fitz_py_import`, `__fitz_py_get_attr_*`,
    /// `__fitz_py_err_to_string`. Display delega a `__str__` Python
    /// para paridad bit-a-bit con el intérprete cuando se imprime un
    /// PyObject opaco (ej. `print(math.pi)` sin anotación destino
    /// imprime "3.141592653589793" en ambos paths).
    ///
    /// Solo se emite cuando `self.uses_python = true`. Programas sin
    /// imports Python no pagan el costo de incluirlo.
    fn emit_python_prelude(&mut self) {
        if !self.uses_python {
            return;
        }
        self.emit(
            "// Fase 8.7.1 + 8.7.2 — preludio interop Python (PyO3)\n\
             use pyo3::prelude::*;\n\
             use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};\n\
             use pyo3::IntoPyObject;\n\n\
             #[derive(Clone)]\n\
             pub struct __FitzPyObject(pub Arc<pyo3::Py<pyo3::PyAny>>);\n\n\
             impl std::fmt::Display for __FitzPyObject {\n    \
             fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n        \
             let s = Python::attach(|py| {\n            \
             self.0.bind(py).str().map(|v| v.to_string()).unwrap_or_else(|_| \"<python object>\".to_string())\n        \
             });\n        \
             write!(f, \"{}\", s)\n    \
             }\n\
             }\n\n\
             impl std::fmt::Debug for __FitzPyObject {\n    \
             fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n        \
             write!(f, \"<python object>\")\n    \
             }\n\
             }\n\n\
             impl PartialEq for __FitzPyObject {\n    \
             fn eq(&self, other: &Self) -> bool {\n        \
             self.0.as_ptr() == other.0.as_ptr()\n    \
             }\n\
             }\n\n\
             fn __fitz_py_err_to_string(py: Python<'_>, err: PyErr) -> String {\n    \
             let class = err.get_type(py).qualname().ok().map(|s| s.to_string()).unwrap_or_else(|| \"PyError\".to_string());\n    \
             let value = err.value(py).to_string();\n    \
             if value.is_empty() { class } else { format!(\"{}: {}\", class, value) }\n\
             }\n\n\
             fn __fitz_py_import(dotted: &str) -> __FitzPyObject {\n    \
             Python::attach(|py| match py.import(dotted) {\n        \
             Ok(module) => __FitzPyObject(Arc::new(module.into_any().unbind())),\n        \
             Err(err) => panic!(\"error importando módulo Python `{}`: {}\", dotted, __fitz_py_err_to_string(py, err)),\n    \
             })\n\
             }\n\n\
             fn __fitz_py_get_attr_obj(obj: &__FitzPyObject, name: &str) -> __FitzPyObject {\n    \
             Python::attach(|py| {\n        \
             let bound = obj.0.bind(py);\n        \
             match bound.getattr(name) {\n            \
             Ok(attr) => __FitzPyObject(Arc::new(attr.unbind())),\n            \
             Err(err) => panic!(\"error accediendo a `.{}` sobre objeto Python: {}\", name, __fitz_py_err_to_string(py, err)),\n        \
             }\n    \
             })\n\
             }\n\n\
             fn __fitz_py_extract_i64(obj: &__FitzPyObject) -> i64 {\n    \
             Python::attach(|py| {\n        \
             let bound = obj.0.bind(py);\n        \
             if bound.is_instance_of::<PyBool>() {\n            \
             return if bound.extract::<bool>().unwrap_or(false) { 1 } else { 0 };\n        \
             }\n        \
             if !bound.is_instance_of::<PyInt>() {\n            \
             panic!(\"se esperaba un int Python para coercer a Int, llegó otro tipo\");\n        \
             }\n        \
             bound.extract::<i64>().unwrap_or_else(|_| panic!(\"el int Python excede el rango de Int (i64) en Fitz\"))\n    \
             })\n\
             }\n\n\
             fn __fitz_py_extract_f64(obj: &__FitzPyObject) -> f64 {\n    \
             Python::attach(|py| {\n        \
             let bound = obj.0.bind(py);\n        \
             if bound.is_instance_of::<PyFloat>() {\n            \
             return bound.extract::<f64>().unwrap_or(0.0);\n        \
             }\n        \
             if bound.is_instance_of::<PyInt>() {\n            \
             return bound.extract::<i64>().map(|n| n as f64).unwrap_or(0.0);\n        \
             }\n        \
             panic!(\"se esperaba un float Python para coercer a Float, llegó otro tipo\")\n    \
             })\n\
             }\n\n\
             fn __fitz_py_extract_string(obj: &__FitzPyObject) -> String {\n    \
             Python::attach(|py| {\n        \
             let bound = obj.0.bind(py);\n        \
             if !bound.is_instance_of::<PyString>() {\n            \
             panic!(\"se esperaba un str Python para coercer a Str, llegó otro tipo\");\n        \
             }\n        \
             bound.extract::<String>().unwrap_or_default()\n    \
             })\n\
             }\n\n\
             fn __fitz_py_extract_bool(obj: &__FitzPyObject) -> bool {\n    \
             Python::attach(|py| {\n        \
             let bound = obj.0.bind(py);\n        \
             if !bound.is_instance_of::<PyBool>() {\n            \
             panic!(\"se esperaba un bool Python para coercer a Bool, llegó otro tipo\");\n        \
             }\n        \
             bound.extract::<bool>().unwrap_or(false)\n    \
             })\n\
             }\n\n",
        );
        // Fase 8.7.2 — trait `__FitzToPy` + impls para primitivos +
        // List/Map/Option + helpers `__fitz_py_call` con wrap a Result
        // y validación de keys hashables. Paralelo a `value_to_py` del
        // py_interop con breadcrumb (`arg0[2].field`) preservado.
        self.emit(
            "// 8.7.2 — marshaling Fitz → Python (genérico) + call con Result wrap.\n\
             pub trait __FitzToPy {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String>;\n\
             }\n\n\
             impl __FitzToPy for i64 {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, _path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n        \
             let bound = self.into_pyobject(py).map_err(|e| format!(\"{:?}\", e))?;\n        \
             Ok(bound.into_any().unbind())\n    \
             }\n\
             }\n\n\
             impl __FitzToPy for f64 {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, _path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n        \
             let bound = self.into_pyobject(py).map_err(|e| format!(\"{:?}\", e))?;\n        \
             Ok(bound.into_any().unbind())\n    \
             }\n\
             }\n\n\
             impl __FitzToPy for bool {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, _path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n        \
             let bound = self.into_pyobject(py).map_err(|e| format!(\"{:?}\", e))?;\n        \
             Ok(bound.to_owned().into_any().unbind())\n    \
             }\n\
             }\n\n\
             impl __FitzToPy for String {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, _path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n        \
             let bound = self.as_str().into_pyobject(py).map_err(|e| format!(\"{:?}\", e))?;\n        \
             Ok(bound.into_any().unbind())\n    \
             }\n\
             }\n\n\
             impl __FitzToPy for () {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, _path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n        \
             Ok(py.None())\n    \
             }\n\
             }\n\n\
             impl __FitzToPy for __FitzPyObject {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, _path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n        \
             Ok(self.0.clone_ref(py))\n    \
             }\n\
             }\n\n\
             // 8.7.2: los nominales `type Foo = Arc<Mutex<FooData>>`\n    \
             // tienen su impl específico emitido por `gen_type_def`\n    \
             // (impl __FitzToPy for FooData + wrapper sobre Arc<Mutex>).\n    \
             // Eso evita conflicto con `Arc<Mutex<Vec<T>>>` (List) y\n    \
             // `Arc<Mutex<Vec<(K,V)>>>` (Map) que son los impls\n    \
             // genéricos abajo.\n\n\
             impl<T: __FitzToPy> __FitzToPy for Option<T> {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n        \
             match self {\n            \
             Some(v) => v.__fitz_to_py(py, path),\n            \
             None => Ok(py.None()),\n        \
             }\n    \
             }\n\
             }\n\n\
             impl<T: __FitzToPy + Clone> __FitzToPy for Arc<Mutex<Vec<T>>> {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n        \
             let snapshot = self.lock().unwrap().clone();\n        \
             let mut items: Vec<pyo3::Py<pyo3::PyAny>> = Vec::with_capacity(snapshot.len());\n        \
             for (i, v) in snapshot.iter().enumerate() {\n            \
             let item_path = format!(\"{}[{}]\", path, i);\n            \
             items.push(v.__fitz_to_py(py, &item_path)?);\n        \
             }\n        \
             let list = PyList::new(py, items).map_err(|e| format!(\"{:?}\", e))?;\n        \
             Ok(list.into_any().unbind())\n    \
             }\n\
             }\n\n\
             fn __fitz_py_marshal_map_key(py: Python<'_>, k: &(impl __FitzToPy + ?Sized), path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n    \
             // Python `dict` exige `__hash__`. Los primitivos Fitz que pasan acá ya son hashables;\n    \
             // tipos compuestos (List/Map/Instance) como key fueron rechazados en build-time o no\n    \
             // se permiten en el subset Map<K,V> con K primitivo.\n    \
             k.__fitz_to_py(py, path)\n\
             }\n\n\
             impl<K: __FitzToPy + Clone, V: __FitzToPy + Clone> __FitzToPy for Arc<Mutex<Vec<(K, V)>>> {\n    \
             fn __fitz_to_py(&self, py: Python<'_>, path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {\n        \
             let snapshot = self.lock().unwrap().clone();\n        \
             let dict = PyDict::new(py);\n        \
             for (k, v) in snapshot.iter() {\n            \
             let key_path = format!(\"{}.<key>\", path);\n            \
             let py_k = __fitz_py_marshal_map_key(py, k, &key_path)?;\n            \
             let val_path = format!(\"{}[<entry>]\", path);\n            \
             let py_v = v.__fitz_to_py(py, &val_path)?;\n            \
             dict.set_item(py_k, py_v).map_err(|e| format!(\"{:?}\", e))?;\n        \
             }\n        \
             Ok(dict.into_any().unbind())\n    \
             }\n\
             }\n\n\
             /// `__fitz_py_invoke(callable, |py| args)` — paralelo a `call` del\n    \
             /// py_interop del intérprete (8.3): excepción Python → `Err(\"<Class>: <msg>\")`,\n    \
             /// éxito → `Ok(__FitzPyObject)` (sin coerción primitiva — eso ocurre en el\n    \
             /// sitio destino vía `__fitz_py_extract_*` o `__fitz_py_to_*`).\n    \
             /// El closure de args corre adentro de `Python::attach` para que el marshaling\n    \
             /// `__fitz_to_py(py, path)` tenga el GIL disponible.\n\
             fn __fitz_py_invoke<F>(callable: &__FitzPyObject, args_fn: F) -> Result<__FitzPyObject, String>\n\
             where\n    \
             F: FnOnce(Python<'_>) -> Result<Vec<pyo3::Py<pyo3::PyAny>>, String>,\n\
             {\n    \
             Python::attach(|py| {\n        \
             let args = args_fn(py)?;\n        \
             let bound = callable.0.bind(py);\n        \
             let args_tuple = pyo3::types::PyTuple::new(py, args).map_err(|e| format!(\"{:?}\", e))?;\n        \
             match bound.call1(args_tuple) {\n            \
             Ok(ret) => Ok(__FitzPyObject(Arc::new(ret.unbind()))),\n            \
             Err(err) => Err(__fitz_py_err_to_string(py, err)),\n        \
             }\n    \
             })\n\
             }\n\n\
             // (Helper async `__fitz_py_invoke_await` se emite abajo solo cuando\n    \
             // el programa usa async — está condicionado en `emit_python_prelude`.)\n\n",
        );
        // 8.7.3: el helper async vive en un emit aparte, condicionado por
        // `uses_async`. Programas que solo importan Python (getattr,
        // call sync) no pagan el costo de tokio en el Cargo.toml.
        if self.uses_async {
            self.emit(
                "/// 8.7.3 — bridge async tokio ↔ asyncio (baseline blocking, paralelo a\n\
                 /// `py_coro_to_fitz_future` del py_interop 8.6.1). El call se ejecuta\n\
                 /// adentro del GIL; si el return es awaitable (`inspect.isawaitable`),\n\
                 /// se evalúa con `tokio::task::spawn_blocking` + `asyncio.new_event_loop()`\n\
                 /// + `run_until_complete`. Si no es awaitable, se devuelve directo —\n\
                 /// permite `.await` ergonómico aún sobre fns Python sync sin error.\n\
                 async fn __fitz_py_invoke_await<F>(callable: &__FitzPyObject, args_fn: F) -> Result<__FitzPyObject, String>\n\
                 where\n    \
                 F: FnOnce(Python<'_>) -> Result<Vec<pyo3::Py<pyo3::PyAny>>, String>,\n\
                 {\n    \
                 let result_obj = __fitz_py_invoke(callable, args_fn)?;\n    \
                 let is_coro = Python::attach(|py| {\n        \
                 let bound = result_obj.0.bind(py);\n        \
                 let inspect = match py.import(\"inspect\") {\n            \
                 Ok(m) => m,\n            \
                 Err(_) => return false,\n        \
                 };\n        \
                 inspect.call_method1(\"isawaitable\", (bound,)).and_then(|v| v.extract::<bool>()).unwrap_or(false)\n    \
                 });\n    \
                 if !is_coro {\n        \
                 return Ok(result_obj);\n    \
                 }\n    \
                 let coro_owned: pyo3::Py<pyo3::PyAny> = Python::attach(|py| result_obj.0.clone_ref(py));\n    \
                 let join_result = tokio::task::spawn_blocking(move || -> Result<__FitzPyObject, String> {\n        \
                 Python::attach(|py| {\n            \
                 let bound = coro_owned.bind(py);\n            \
                 let asyncio = py.import(\"asyncio\").map_err(|e| __fitz_py_err_to_string(py, e))?;\n            \
                 let event_loop = asyncio.call_method0(\"new_event_loop\").map_err(|e| __fitz_py_err_to_string(py, e))?;\n            \
                 let r = event_loop.call_method1(\"run_until_complete\", (bound,));\n            \
                 let _ = event_loop.call_method0(\"close\");\n            \
                 match r {\n                \
                 Ok(v) => Ok(__FitzPyObject(Arc::new(v.unbind()))),\n                \
                 Err(e) => Err(__fitz_py_err_to_string(py, e)),\n            \
                 }\n        \
                 })\n    \
                 }).await;\n    \
                 match join_result {\n        \
                 Ok(inner) => inner,\n        \
                 Err(join_err) => Err(format!(\"error del blocking pool al ejecutar corutina Python: {}\", join_err)),\n    \
                 }\n\
                 }\n\n",
            );
        }
        self.emit(
            "/// 8.7.2 — `python_list → List<T>`. Convierte un PyList a un `Vec<T>` ya\n    \
             /// adentro de `Arc<Mutex<>>`. T es cualquier tipo Fitz primitivo o nominal:\n    \
             /// el codegen invoca la variante apropiada (`__fitz_py_to_list_i64`, etc.) según\n    \
             /// el tipo destino concreto del binding.\n\
             fn __fitz_py_to_list_i64(obj: &__FitzPyObject) -> Arc<Mutex<Vec<i64>>> {\n    \
             Python::attach(|py| {\n        \
             let bound = obj.0.bind(py);\n        \
             let list = bound.cast::<PyList>().unwrap_or_else(|_| panic!(\"se esperaba list Python para coercer a List<Int>\"));\n        \
             let mut out: Vec<i64> = Vec::with_capacity(list.len());\n        \
             for item in list.iter() {\n            \
             if !item.is_instance_of::<PyInt>() { panic!(\"elemento de list Python no es int — esperado para List<Int>\"); }\n            \
             out.push(item.extract::<i64>().unwrap_or_else(|_| panic!(\"int Python fuera de rango i64 al coercer List<Int>\")));\n        \
             }\n        \
             Arc::new(Mutex::new(out))\n    \
             })\n\
             }\n\n\
             fn __fitz_py_to_list_f64(obj: &__FitzPyObject) -> Arc<Mutex<Vec<f64>>> {\n    \
             Python::attach(|py| {\n        \
             let bound = obj.0.bind(py);\n        \
             let list = bound.cast::<PyList>().unwrap_or_else(|_| panic!(\"se esperaba list Python para coercer a List<Float>\"));\n        \
             let mut out: Vec<f64> = Vec::with_capacity(list.len());\n        \
             for item in list.iter() {\n            \
             let f = if item.is_instance_of::<PyFloat>() {\n                \
             item.extract::<f64>().unwrap_or(0.0)\n            \
             } else if item.is_instance_of::<PyInt>() {\n                \
             item.extract::<i64>().map(|n| n as f64).unwrap_or(0.0)\n            \
             } else {\n                \
             panic!(\"elemento de list Python no es número — esperado para List<Float>\")\n            \
             };\n            \
             out.push(f);\n        \
             }\n        \
             Arc::new(Mutex::new(out))\n    \
             })\n\
             }\n\n\
             fn __fitz_py_to_list_string(obj: &__FitzPyObject) -> Arc<Mutex<Vec<String>>> {\n    \
             Python::attach(|py| {\n        \
             let bound = obj.0.bind(py);\n        \
             let list = bound.cast::<PyList>().unwrap_or_else(|_| panic!(\"se esperaba list Python para coercer a List<Str>\"));\n        \
             let mut out: Vec<String> = Vec::with_capacity(list.len());\n        \
             for item in list.iter() {\n            \
             if !item.is_instance_of::<PyString>() { panic!(\"elemento de list Python no es str — esperado para List<Str>\"); }\n            \
             out.push(item.extract::<String>().unwrap_or_default());\n        \
             }\n        \
             Arc::new(Mutex::new(out))\n    \
             })\n\
             }\n\n",
        );
    }

    /// Fase 8.7.2 — emite los bindings Python como **statics globales
    /// + getters** al top-level del crate generado. Cada `from python
    /// import math` produce:
    ///
    /// ```rust
    /// static __FITZ_PY_BIND_MATH: std::sync::OnceLock<__FitzPyObject> = std::sync::OnceLock::new();
    /// fn __fitz_py_bind_math() -> __FitzPyObject {
    ///     __FITZ_PY_BIND_MATH.get_or_init(|| __fitz_py_import("math")).clone()
    /// }
    /// ```
    ///
    /// Cualquier fn del programa (main, handlers HTTP, user-fns puede
    /// referenciar `math` y el codegen lo traduce a
    /// `__fitz_py_bind_math()`. El boot del módulo Python es lazy: la
    /// primera invocación inicializa el OnceLock (toma el GIL, importa);
    /// las siguientes son lecturas atómicas baratas.
    fn emit_python_bindings_top_level(&mut self, imports: &[PythonImport]) {
        if imports.is_empty() {
            return;
        }
        for imp in imports {
            let upper = sanitize_python_binding_static(&imp.binding_name);
            let lower = sanitize_python_binding_lower(&imp.binding_name);
            self.emit(&format!(
                "static __FITZ_PY_BIND_{upper}: std::sync::OnceLock<__FitzPyObject> = std::sync::OnceLock::new();\n\
                 fn __fitz_py_bind_{lower}() -> __FitzPyObject {{\n    \
                 __FITZ_PY_BIND_{upper}.get_or_init(|| __fitz_py_import(\"{dotted}\")).clone()\n\
                 }}\n\n",
                upper = upper,
                lower = lower,
                dotted = imp.dotted_path,
            ));
        }
    }

    // --- error helpers ----------------------------------------------------

    /// Error sin posición. Reservado para errores **defensivos**: bugs
    /// del compilador, no del código Fitz del usuario (ej. "tipo no
    /// pre-registrado", "fn no estaba pre-registrada", "variable
    /// desconocida en codegen"). El checker debería haberlos cazado,
    /// así que si llegamos acá es un bug y citar una posición del
    /// programa no aporta. Para errores que sí dispara código Fitz
    /// válido pero no soportado por el codegen, usar `err_at`.
    fn err(&self, msg: impl Into<String>) -> FitzError {
        FitzError::new(ErrorKind::TypeError, 0, 0, msg.into())
    }

    /// Variante de `err` que cita la posición real del nodo del AST.
    /// Lo usan los sitios del codegen que tienen un `Expr`/`Stmt` a
    /// mano y quieren que el mensaje apunte al token problemático
    /// (operador, paréntesis, statement entero, etc.). Default para
    /// cualquier error que el usuario podría ver.
    fn err_at(&self, span: crate::ast::Span, msg: impl Into<String>) -> FitzError {
        FitzError::new(ErrorKind::TypeError, span.line, span.column, msg.into())
    }

    // --- prelude + main shell ---------------------------------------------

    fn emit_prelude(&mut self) {
        self.emit("// Código generado por Fitz 5b — no editar a mano.\n");
        // El `#![allow(...)]` es atributo de crate, solo en main.rs.
        if matches!(self.mode, GenMode::Main) {
            self.emit(
                "#![allow(unused_mut, unused_variables, unused_assignments, dead_code)]\n\n",
            );
        } else {
            // En un módulo emitimos los allows como atributos del
            // archivo (`#![...]` también funciona en mods; el efecto
            // se acota al mod).
            self.emit(
                "#![allow(unused_mut, unused_variables, unused_assignments, dead_code)]\n\n",
            );
        }
        // Arc<Mutex<>> es la representación de las instancias de
        // tipos custom — coincide con el modelo del intérprete post-
        // F17.2 (las mutaciones se ven a través de cualquier alias y
        // los binarios HTTP son thread-safe sobre el runtime
        // `rt-multi-thread`). std::sync sobre parking_lot para que el
        // binario generado no arrastre deps extras.
        self.emit("use std::sync::{Arc, Mutex};\n\n");
        // Helper de formato para Float: alinea con `Display` del
        // intérprete (`3.0` se imprime como `\"3.0\"`, no `\"3\"`).
        // Cada archivo (main.rs o mod) trae su propio `__fitz_fmt_float`;
        // no compartimos — es solo unas pocas líneas y nos ahorra una
        // dependencia cross-module.
        self.emit(
            "fn __fitz_fmt_float(v: f64) -> String {\n    \
             if v.is_finite() && v.fract() == 0.0 { format!(\"{:.1}\", v) } else { format!(\"{}\", v) }\n}\n\n",
        );
        // Fase 6.6: builtin `sleep` Fitz → wrapper async sobre
        // `tokio::time::sleep`. Solo se emite cuando el programa lo
        // usa (`uses_async` cubre `sleep`/`.await`/`async fn`) — los
        // programas sync no pagan el costo de incluirlo.
        if self.uses_async {
            self.emit(
                "async fn __fitz_sleep(ms: i64) {\n    \
                 tokio::time::sleep(std::time::Duration::from_millis(\
                 ms.max(0) as u64)).await\n}\n\n",
            );
        }
    }

    fn gen_main(&mut self, stmts: &[&Stmt]) -> Result<(), FitzError> {
        // Fase 6.6: si el programa usa async (sin HTTP — el path
        // HTTP tiene su propio `gen_http_main` con `#[tokio::main]`),
        // emitimos `fn main()` como `#[tokio::main(flavor =
        // "current_thread")] async fn main()`. Eso destraba `.await`
        // top-level y llamadas a `async fn` Fitz que llegan acá vía
        // statements del CLI.
        if self.uses_async {
            self.emit("#[tokio::main(flavor = \"current_thread\")]\n");
            self.emit("async fn main() {\n");
        } else {
            self.emit("fn main() {\n");
        }
        self.indent += 1;
        self.push_scope();
        // Fase 8.7.2: los bindings Python son **globales** (static +
        // getter emitido al top-level del crate por
        // `emit_python_bindings_top_level`). El main body no necesita
        // declararlos como vars locales — `gen_expr::Ident` despacha
        // sobre `python_bindings` y emite el getter inline cuando
        // aparece el nombre.
        for stmt in stmts {
            self.gen_stmt(stmt)?;
        }
        self.pop_scope();
        self.indent -= 1;
        self.emit("}\n");
        Ok(())
    }

    // --- pre-registro de tipos custom -------------------------------------

    /// Recorre los `Stmt::TypeDef` del programa y arma `type_sigs` con
    /// el `TypeId`, los campos con tipo resuelto (vía `TypeEnv`) y la
    /// expresión default (vía AST). El checker ya validó nombres y
    /// recursividad de tipos, así que acá los `lookup`/`info` siempre
    /// resuelven.
    fn pre_register_types(&mut self, program: &Program) -> Result<(), FitzError> {
        // MW.3: pre-registrar los tipos built-in `Request` y `Response`.
        // Existen en TypeEnv (registrados por `register_http_builtin_types`
        // de `types.rs`) y se referencian desde los middlewares del
        // usuario. Necesitamos sus entries en `type_sigs`/`fields_by_id`
        // para que `rust_type_for(Nominal(<Request>))` emita `Request`
        // y que el preludio HTTP los emita como structs Rust legítimos.
        for builtin in &["Request", "Response"] {
            if let Some(id) = self.env.lookup(builtin) {
                let resolved: Vec<ResolvedField> = self
                    .env
                    .info(id)
                    .fields
                    .clone()
                    .unwrap_or_default();
                let combined: Vec<TypeSigField> = resolved
                    .iter()
                    .map(|r| TypeSigField {
                        name: r.name.clone(),
                        type_: r.type_.clone(),
                        default: None,
                    })
                    .collect();
                self.fields_by_id.insert(id, resolved);
                self.type_sigs.insert(
                    (*builtin).to_string(),
                    TypeSig { id, fields: combined },
                );
            }
        }
        for stmt in program {
            let Stmt::TypeDef { name, fields: ast_fields, .. } = stmt else { continue };
            let id = self.env.lookup(name).ok_or_else(|| {
                self.err(format!("tipo `{}` no registrado en el TypeEnv (¿checker no corrió?)", name))
            })?;
            let resolved: Vec<ResolvedField> = match &self.env.info(id).fields {
                Some(fs) => fs.clone(),
                None => {
                    return Err(self.err(format!(
                        "tipo `{}`: campos no resueltos por el checker — no se puede codegen",
                        name
                    )));
                }
            };
            // Combinamos: el orden viene de los `ResolvedField` (que
            // el checker mantiene en orden de declaración). Para cada
            // uno, buscamos el AST por nombre para sacar el default.
            let mut combined = Vec::with_capacity(resolved.len());
            for r in &resolved {
                let default = ast_fields
                    .iter()
                    .find(|f: &&Field| f.name == r.name)
                    .and_then(|f| f.default.clone());
                combined.push(TypeSigField {
                    name: r.name.clone(),
                    type_: r.type_.clone(),
                    default,
                });
            }
            self.fields_by_id.insert(id, resolved);
            self.type_sigs.insert(name.clone(), TypeSig { id, fields: combined });
        }

        // 5b.5: enriquecer con tipos importados via `from foo import User`.
        // El checker del importer ya registró `User` como nominal en el
        // TypeEnv (sin fields). Acá copiamos los fields desde el módulo
        // cargado, asignándolos al `id` del importer — así el TypeId
        // que aparece en `Type::Nominal(id)` sigue siendo coherente
        // entre código del main y los lookups por id.
        let bindings: Vec<(String, ResolvedBinding)> = self
            .module_bindings
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, binding) in bindings {
            let ResolvedBinding::Named {
                module_index,
                item,
                kind,
            } = binding
            else {
                continue;
            };
            if !matches!(kind, NamedKind::Type) {
                continue;
            }
            let importer_id = self.env.lookup(&name).ok_or_else(|| {
                self.err(format!(
                    "tipo importado `{}` no registrado en TypeEnv del importer (¿checker no corrió?)",
                    name
                ))
            })?;
            let module_sig = {
                let m = self.loaded_modules.get(module_index).ok_or_else(|| {
                    self.err(format!("módulo no cargado al registrar `{}`", item))
                })?;
                m.type_sigs.get(&item).cloned().ok_or_else(|| {
                    self.err(format!("el módulo no expone el tipo `{}`", item))
                })?
            };
            let resolved: Vec<ResolvedField> = module_sig
                .fields
                .iter()
                .map(|f| ResolvedField {
                    name: f.name.clone(),
                    type_: f.type_.clone(),
                })
                .collect();
            // Reasignamos el id del módulo al del importer al copiar.
            let combined = module_sig.fields.clone();
            self.fields_by_id.insert(importer_id, resolved);
            self.type_sigs.insert(
                name.clone(),
                TypeSig {
                    id: importer_id,
                    fields: combined,
                },
            );
        }
        Ok(())
    }

    /// Devuelve los fields de un tipo nominal por TypeId. Mira primero
    /// la tabla interna (`fields_by_id`, llenada con tipos locales e
    /// importados) y, como fallback histórico, el TypeEnv. Esto deja
    /// que un tipo importado via `from foo import User` siga andando
    /// aunque el checker no haya resuelto sus fields.
    fn fields_for_id(&self, id: TypeId) -> Option<Vec<ResolvedField>> {
        if let Some(fs) = self.fields_by_id.get(&id) {
            return Some(fs.clone());
        }
        self.env.info(id).fields.clone()
    }

    // --- pre-registro de fns top-level ------------------------------------

    /// Pre-registra los `Stmt::Assign` top-level del módulo como consts
    /// con su tipo, para que el body de las fns del módulo pueda
    /// referenciarlos. Solo aplica en modo `Module`.
    fn pre_register_top_lets(&mut self, program: &Program) -> Result<(), FitzError> {
        for stmt in program {
            let stmt_span = stmt.span();
            let Stmt::Assign { target, type_, value, .. } = stmt else { continue };
            let AssignTarget::Ident(name) = target else { continue };
            let ty = match type_ {
                Some(te) => resolve_type_expr(te, self.env).map_err(|e| {
                    self.err_at(stmt_span, format!(
                        "let `{}` del módulo: anotación: {}",
                        name, e.message
                    ))
                })?,
                None => infer_literal_type(value).unwrap_or(Type::Any),
            };
            self.own_consts.insert(name.clone(), ty);
        }
        Ok(())
    }

    fn pre_register_fns(&mut self, program: &Program) -> Result<(), FitzError> {
        // Mini-fase MW.3: pre-scan de fns referenciadas como
        // `@middleware(name)` en cualquier FnDef del programa. Esas fns
        // mutan su return type Rust a `Option<__FitzResponse>` y sus
        // returns se envuelven (`return null` → `None`, `return <s> { ... }`
        // → `Some(...)`). Lo hacemos antes de pre-registrar las firmas
        // para que `fn_sigs` de cada middleware refleje el override.
        for stmt in program {
            if let Stmt::FnDef { decorators, .. } = stmt {
                for deco in decorators {
                    if deco.name != "middleware" {
                        continue;
                    }
                    for arg in &deco.args {
                        if let Expr::Ident(n, _) = arg {
                            self.middleware_fn_names.insert(n.clone());
                        }
                    }
                }
            }
        }

        for stmt in program {
            if let Stmt::FnDef {
                name,
                params,
                return_type,
                is_async,
                ..
            } = stmt
            {
                let fn_span = stmt.span();
                let params: Vec<Type> = params
                    .iter()
                    .map(|p| self.resolve_param_type(name, &p.name, p.type_.as_ref(), fn_span))
                    .collect::<Result<_, _>>()?;
                let inner_ret = match return_type {
                    Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                        FitzError::new(
                            e.kind,
                            fn_span.line,
                            fn_span.column,
                            format!(
                                "fn `{}`: return type no resuelve: {}",
                                name, e.message
                            ),
                        )
                    })?,
                    None => Type::Null,
                };
                // Fase 6.6: la firma EXTERNA de una `async fn` envuelve
                // su return type en `Future<T>` — espejo de lo que hace
                // el checker (`preregister_fn_signatures` en
                // `types.rs`). Sin esto, `gen_call` sobre una async fn
                // creería que el call site tipa como `T` y el `.await`
                // posterior caería al path gradual (`Any`), perdiendo
                // info de tipo. La emisión Rust del body NO usa esta
                // firma envuelta (Rust auto-envuelve con `async fn`).
                let ret = if *is_async {
                    Type::Future(Box::new(inner_ret))
                } else {
                    inner_ret
                };
                self.fn_sigs.insert(name.clone(), FnSig { params, ret });
            }
        }
        Ok(())
    }

    fn resolve_param_type(
        &self,
        fn_name: &str,
        param_name: &str,
        type_: Option<&TypeExpr>,
        fn_span: crate::ast::Span,
    ) -> Result<Type, FitzError> {
        match type_ {
            Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                FitzError::new(
                    e.kind,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "fn `{}`: parámetro `{}`: {}",
                        fn_name, param_name, e.message
                    ),
                )
            }),
            None => Err(self.err_at(fn_span, format!(
                "fn `{}`: el parámetro `{}` necesita una anotación de tipo para el codegen (5b.1)",
                fn_name, param_name
            ))),
        }
    }

    // --- generación de tipos custom ---------------------------------------

    fn gen_type_def(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::TypeDef { name, .. } = stmt else {
            unreachable!("gen_type_def solo se llama sobre Stmt::TypeDef");
        };
        let sig = self
            .type_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("tipo `{}` no pre-registrado", name)))?;

        let data_name = format!("{}Data", name);

        // struct <Foo>Data { f1: T1, f2: T2, ... }
        //
        // F17.4b: `#[derive(Clone)]` solo. `PartialEq` se emite manual
        // abajo porque `std::sync::Mutex<T>` no impl `PartialEq` (por
        // diseño — comparar a través de un lock tiene semántica sutil).
        // El intérprete usa el mismo patrón en `value.rs` (`Arc::ptr_eq`
        // o lock+deref); replicamos esa lógica por campo según su tipo.
        let pub_kw = self.pub_prefix();
        let field_pub = pub_kw;
        write!(
            &mut self.output,
            "#[derive(Clone)]\n{}struct {} {{\n",
            pub_kw, data_name
        )
        .unwrap();
        for f in &sig.fields {
            writeln!(
                &mut self.output,
                "    {}{}: {},",
                field_pub,
                f.name,
                rust_type_for(&f.type_, self.env)?
            )
            .unwrap();
        }
        self.emit("}\n\n");

        // type Foo = Arc<Mutex<FooData>>;
        write!(
            &mut self.output,
            "{}type {} = Arc<Mutex<{}>>;\n\n",
            pub_kw, name, data_name
        )
        .unwrap();

        // impl PartialEq for FooData — comparación estructural campo a
        // campo. Para nominales/listas/mapas usa el patrón del
        // intérprete: `Arc::ptr_eq` shortcut + lock+deref si los Arc no
        // son el mismo. Para primitivos, `==` directo. Para Option,
        // pattern-match con recursión en el inner.
        write!(
            &mut self.output,
            "impl PartialEq for {} {{\n    fn eq(&self, __other: &Self) -> bool {{\n",
            data_name
        )
        .unwrap();
        if sig.fields.is_empty() {
            self.emit("        true\n");
        } else {
            self.emit("        true");
            for f in &sig.fields {
                let lhs = format!("self.{}", f.name);
                let rhs = format!("__other.{}", f.name);
                let expr = field_eq_expr(&f.type_, &lhs, &rhs, self.env)?;
                writeln!(&mut self.output, "\n            && {}", expr).unwrap();
            }
        }
        self.emit("    }\n}\n\n");

        // impl Display for FooData — reproduce el formato del
        // intérprete: `Foo { f1: v1, f2: v2 }`. Strings con comillas,
        // Floats con `.0` si fracción 0, instancias delegando a su
        // propio Display, Option como `null` cuando None.
        write!(
            &mut self.output,
            "impl std::fmt::Display for {} {{\n    fn fmt(&self, __f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n",
            data_name
        )
        .unwrap();
        if sig.fields.is_empty() {
            // `Foo {}` — sin espacios.
            writeln!(&mut self.output, "        write!(__f, \"{} {{{{}}}}\")\n    }}\n}}\n", name).unwrap();
        } else {
            writeln!(&mut self.output, "        write!(__f, \"{} {{{{\")?;", name).unwrap();
            for (i, f) in sig.fields.iter().enumerate() {
                if i > 0 {
                    self.emit("        write!(__f, \",\")?;\n");
                }
                writeln!(&mut self.output, "        write!(__f, \" {}: \")?;", f.name).unwrap();
                let field_expr = format!("self.{}", f.name);
                let stmt = inline_display_stmt(&field_expr, &f.type_);
                self.emit(&stmt);
            }
            self.emit("        write!(__f, \" }}\")\n");
            self.emit("    }\n}\n\n");
        }
        // Fase 8.7.2: cuando el programa usa interop Python, emit
        // `impl __FitzToPy for FooData` para que `<user>.__fitz_to_py(...)`
        // funcione (paralelo a `Instance → PyDict` del intérprete en
        // `value_to_py`). El path breadcrumb se construye con el
        // nombre del tipo + nombre del campo. Solo en mode Main —
        // mode Module no tiene el preludio Python disponible.
        if self.uses_python && matches!(self.mode, GenMode::Main) {
            write!(
                &mut self.output,
                "impl __FitzToPy for {data} {{\n    \
                 fn __fitz_to_py(&self, py: Python<'_>, path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {{\n        \
                 let dict = PyDict::new(py);\n        \
                 let __prefix: String = if path.is_empty() {{ String::from(\"{name}\") }} else {{ path.to_string() }};\n",
                data = data_name,
                name = name,
            )
            .unwrap();
            for f in &sig.fields {
                writeln!(
                    &mut self.output,
                    "        {{ let __field_path = format!(\"{{}}.{field}\", __prefix); \
                     let __py_v = self.{field}.__fitz_to_py(py, &__field_path)?; \
                     dict.set_item({lit}, __py_v).map_err(|e| format!(\"{{:?}}\", e))?; }}",
                    field = f.name,
                    lit = rust_str_literal(&f.name),
                )
                .unwrap();
            }
            self.emit("        Ok(dict.into_any().unbind())\n    }\n}\n\n");
            // Wrapper: el codegen pasa `Foo` (= Arc<Mutex<FooData>>) como
            // arg de calls Python, no `FooData` directo. Impl sobre el
            // tipo target del alias para que `<user_var>.__fitz_to_py(...)`
            // resuelva. Delega al lock + `FooData::__fitz_to_py`.
            write!(
                &mut self.output,
                "impl __FitzToPy for Arc<Mutex<{data}>> {{\n    \
                 fn __fitz_to_py(&self, py: Python<'_>, path: &str) -> Result<pyo3::Py<pyo3::PyAny>, String> {{\n        \
                 let __g = self.lock().unwrap();\n        \
                 __g.__fitz_to_py(py, path)\n    \
                 }}\n\
                 }}\n\n",
                data = data_name,
            )
            .unwrap();
        }
        Ok(())
    }

    /// PreF8.3: por cada field con default, emite una helper
    /// `pub fn __default_<TypeName>_<FieldName>() -> T { <code del
    /// default> }`. Se invoca solo en modo Module — el `main.rs`
    /// generado para el importer llama a estas helpers en lugar de
    /// inline-ar el `default_expr` (que referenciaría símbolos del
    /// módulo de origen que el importer no tiene visibles).
    ///
    /// Si el tipo no tiene defaults, no emite nada.
    fn gen_type_default_helpers(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::TypeDef { name, .. } = stmt else {
            unreachable!("gen_type_default_helpers solo se llama sobre Stmt::TypeDef");
        };
        let sig = self
            .type_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("tipo `{}` no pre-registrado", name)))?;
        for f in &sig.fields {
            let Some(default_expr) = &f.default else { continue };
            let rust_ty = rust_type_for(&f.type_, self.env)?;
            let (code, ty) = self.gen_expr(default_expr)?;
            let coerced = coerce(&code, &ty, &f.type_);
            writeln!(
                &mut self.output,
                "pub fn __default_{}_{}() -> {} {{ {} }}",
                name, f.name, rust_ty, coerced
            )
            .unwrap();
        }
        if sig.fields.iter().any(|f| f.default.is_some()) {
            self.emit("\n");
        }
        Ok(())
    }

    // --- generación de funciones top-level --------------------------------

    fn gen_top_fn(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::FnDef {
            name,
            params,
            return_type: _,
            body,
            is_async,
            decorators,
            ..
        } = stmt
        else {
            unreachable!("gen_top_fn solo se llama sobre Stmt::FnDef");
        };

        let is_http_handler = decorators.iter().any(|d| {
            matches!(d.name.as_str(), "get" | "post" | "put" | "delete")
        });
        // MW.3: una fn referenciada como `@middleware(name)` en algún
        // FnDef se trata como contexto HTTP (paridad con el checker en
        // types.rs). Su return type Rust override a
        // `Option<__FitzResponse>` y los returns se envuelven igual que
        // un handler con ReturnStatus (gate-only: None = continúa,
        // Some = short-circuit). Ver `gen_middleware_return_*`.
        let is_middleware = self.middleware_fn_names.contains(name);
        let has_return_status_inner = contains_return_status_stmts(body);

        // Status codes custom: si la fn HTTP contiene al menos un
        // `Stmt::ReturnStatus`, su return type Rust pasa a ser
        // `__FitzResponse` (en vez del declarado) y todos los returns
        // se envuelven. El handler wrapper lo detecta vía la tabla
        // `http_handlers_returning_response` para emitir el destructuring
        // apropiado. Para middlewares (MW.3) reusamos el mismo flag
        // `response_mode` para envolver `Stmt::ReturnStatus`, pero la
        // emisión final difiere — los middlewares retornan
        // `Option<__FitzResponse>`, no `__FitzResponse`.
        let has_return_status = (is_http_handler || is_middleware) && has_return_status_inner;
        if has_return_status && is_http_handler {
            self.http_handlers_returning_response.insert(name.clone());
        }

        let sig = self
            .fn_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("fn `{}` no estaba pre-registrada", name)))?;

        // Header: fn <name>(p1: T1, p2: T2, ...) -> Ret {
        // Fase 6.6: `async fn` Fitz → `pub async fn` Rust. Rust auto-
        // envuelve el return type en `impl Future<Output = T>`, así
        // que NO se modifica el ret renderizado abajo.
        // MW.3: middlewares llevan #[allow(unreachable_code)] porque
        // emitimos `None` trailing al cierre del body — si el body
        // termina con un `return Some(...)` (caso típico de auth que
        // siempre rechaza), el `None` queda inalcanzable y rustc
        // emitiría warning. La etiqueta es zero-overhead.
        if is_middleware {
            self.emit("#[allow(unreachable_code)]\n");
        }
        let pub_kw = self.pub_prefix();
        self.emit(pub_kw);
        if *is_async {
            self.emit("async ");
        }
        self.emit("fn ");
        self.emit(name);
        self.emit("(");
        for (i, (param, pty)) in params.iter().zip(sig.params.iter()).enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.emit("mut ");
            self.emit(&param.name);
            self.emit(": ");
            self.emit(&rust_type_for(pty, self.env)?);
        }
        self.emit(")");
        // En response mode el return type generado es `__FitzResponse`,
        // que la struct se define en el preludio HTTP del main. El
        // return type declarado del usuario se ignora (es por la
        // semántica polimórfica del spec — handler puede devolver T o
        // un response builder).
        //
        // Fase 6.6: `sig.ret` en `fn_sigs` carga `Future<T>` cuando la
        // fn es async (alineado con la firma del checker que usan los
        // call sites). Pero Rust auto-envuelve con `async fn`: emitir
        // `-> Pin<Box<dyn Future<...>>>` además daría doble wrapping.
        // Por eso al renderizar el return Rust usamos el INNER si la
        // fn es async, y `sig.ret` directo si no.
        let emit_ret = if *is_async {
            match &sig.ret {
                Type::Future(inner) => (**inner).clone(),
                other => other.clone(),
            }
        } else {
            sig.ret.clone()
        };
        if is_middleware {
            // MW.3: middlewares siempre retornan Option<__FitzResponse>,
            // sin importar el return type declarado por el usuario. El
            // checker ya validó la signatura (Request param, retorno
            // implícito `()` o `Response?` decorativo).
            self.emit(" -> Option<__FitzResponse>");
        } else if has_return_status {
            self.emit(" -> __FitzResponse");
        } else if !matches!(emit_ret, Type::Null) {
            self.emit(" -> ");
            self.emit(&rust_type_for(&emit_ret, self.env)?);
        }
        self.emit(" {\n");

        // Body
        self.indent += 1;
        self.push_scope();
        for (param, pty) in params.iter().zip(sig.params.iter()) {
            self.declare_var(param.name.clone(), pty.clone());
        }
        // F11: si esta fn referencia algún state HTTP shared, lo
        // materializamos como var local al inicio del body. El `(*X).clone()`
        // es Arc clone (barato) y preserva aliasing — mutaciones via
        // `users.push(...)` se ven en todas las llamadas posteriores
        // porque el LazyLock guarda el Arc, no el contenido. F11
        // original usaba `thread_local!` + `.with(|s| s.clone())`;
        // F17.4b migró a LazyLock para destrabar multi-thread.
        if let Some(deps) = self.fn_state_deps.get(name).cloned() {
            for dep_name in &deps {
                let ty = self
                    .state_var_types
                    .get(dep_name)
                    .cloned()
                    .ok_or_else(|| {
                        self.err(format!(
                            "fn `{}` referencia state `{}` pero el tipo no se resolvió",
                            name, dep_name
                        ))
                    })?;
                let static_name = state_var_static_name(dep_name);
                let rust_ty = rust_type_for(&ty, self.env)?;
                self.emit_indent();
                writeln!(
                    &mut self.output,
                    "let mut {}: {} = (*{}).clone();",
                    dep_name, rust_ty, static_name
                )
                .unwrap();
                self.declare_var(dep_name.clone(), ty);
            }
        }
        // Frame de "return esperado" para coerciones y para que `?`
        // (Try) pueda validar que está adentro de una fn Result.
        // Fase 6.6: usamos `emit_ret` (el inner del `Future<T>` para
        // async fn, o `sig.ret` directo) porque adentro del body los
        // `return x` retornan `T` puro — `async` es transparente
        // desde adentro (espejo del checker en `types.rs`).
        self.ret_stack.push(emit_ret.clone());
        let saved_response_mode = self.response_mode;
        let saved_in_middleware = self.in_middleware_fn;
        self.response_mode = has_return_status;
        self.in_middleware_fn = is_middleware;
        for stmt in body {
            self.gen_stmt_in_fn(stmt, &emit_ret)?;
        }
        // MW.3: tail-fall del body de un middleware sin return explícito.
        // El return type es `Option<__FitzResponse>` y el body cae al
        // final sin generar `None;`. Rust quejaría con "expected
        // Option<...>, found ()". Emitimos `None` siempre — si el body
        // ya hizo un return explícito esto es código muerto que rustc
        // elimina sin warning (porque viene después de un `return`).
        if is_middleware {
            self.emit_indent();
            self.emit("None\n");
        }
        self.response_mode = saved_response_mode;
        self.in_middleware_fn = saved_in_middleware;
        self.ret_stack.pop();
        self.pop_scope();
        self.indent -= 1;

        self.emit("}\n\n");
        Ok(())
    }

    // --- generación de statements -----------------------------------------

    fn gen_stmt(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        // En el scope top-level (main), no hay return_type — usamos
        // Null como placeholder (los `return` ahí adentro son raros
        // pero válidos; el evaluator también los emite como huérfanos).
        self.gen_stmt_in_fn(stmt, &Type::Null)
    }

    fn gen_stmt_in_fn(&mut self, stmt: &Stmt, ret_expected: &Type) -> Result<(), FitzError> {
        match stmt {
            Stmt::Assign { target, type_, value, .. } => self.gen_assign(target, type_.as_ref(), value),
            Stmt::Return(e, _) => self.gen_return(e, ret_expected),
            Stmt::ReturnStatus { status, body, span } => {
                self.gen_return_status(status, body.as_ref(), *span)
            }
            Stmt::Expr(e, _) => {
                self.emit_indent();
                self.gen_expr_for_stmt(e)?;
                self.emit(";\n");
                Ok(())
            }
            Stmt::While { condition, body, .. } => self.gen_while(condition, body, ret_expected),
            Stmt::Loop { body, .. } => self.gen_loop(body, ret_expected),
            Stmt::For { var, iter, body, .. } => self.gen_for(var, iter, body, ret_expected),
            Stmt::Break(_) => {
                self.emit_indent();
                self.emit("break;\n");
                Ok(())
            }
            Stmt::Continue(_) => {
                self.emit_indent();
                self.emit("continue;\n");
                Ok(())
            }
            Stmt::FnDef { name, .. } => Err(self.err_at(stmt.span(), format!(
                "fn anidada `{}`: no soportada en 5b.1 — declarala a nivel top",
                name
            ))),
            Stmt::TypeDef { name, .. } => Err(self.err_at(stmt.span(), format!(
                "`type {}`: solo se admite a nivel top, no adentro de funciones u otros bloques",
                name
            ))),
            Stmt::Import { .. } | Stmt::FromImport { .. } => Err(self.err_at(stmt.span(),
                "`import`: solo se admite a nivel top del programa, no adentro de fns u otros bloques",
            )),
            // Fase 9.0.1 (F15): defensa contra Error nodes — `fitz
            // build` usa `parse()` strict; este nodo solo aparece bajo
            // `parse_with_recovery`. Si llegamos acá es un bug del
            // compilador.
            Stmt::Error(span) => Err(self.err_at(*span,
                "nodo `Stmt::Error` en el AST — `fitz build` usa el parser strict, no debería verlo (bug del compilador, Fase 9.0.1)",
            )),
        }
    }

    fn gen_assign(
        &mut self,
        target: &AssignTarget,
        type_: Option<&TypeExpr>,
        value: &Expr,
    ) -> Result<(), FitzError> {
        let name = match target {
            AssignTarget::Ident(n) => n,
            AssignTarget::Field { object, field } => {
                return self.gen_field_assign(object, field, value);
            }
        };

        let (rhs_code, rhs_ty) = self.gen_expr(value)?;
        let declared_ty = match type_ {
            Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                self.err_at(value.span(), format!("anotación de `{}` no resuelve: {}", name, e.message))
            })?,
            None => rhs_ty.clone(),
        };

        let final_rhs = coerce(&rhs_code, &rhs_ty, &declared_ty);
        self.emit_indent();
        // Si la var ya existe en algún scope visible (outer o
        // current), es reasignación: emitimos `name = ...`. Si no,
        // declaración: `let mut name: T = ...`. NOTA: una "primera
        // asignación" adentro de un bloque (while/loop/for body)
        // queda confinada a ese bloque en el Rust generado, mientras
        // que en Fitz persistiría afuera. Es una discrepancia
        // conocida del codegen 5b.1; refinarla pide pre-declarar
        // todas las vars del programa, que llega después.
        if self.var_in_any_scope(name) {
            // Reasignación.
            self.emit(name);
            self.emit(" = ");
            self.emit(&final_rhs);
            self.emit(";\n");
            // El scope ya tiene el tipo del binding original; lo
            // mantenemos.
        } else {
            // Primera vez en este scope — declaración.
            // Caso especial `let _ = ...` (descartar): Rust no admite
            // `let mut _` ni anotación de tipo sobre `_`. Emitimos
            // `let _ = ...;` plano. Útil para llamadas async cuyo
            // resultado no se usa: `let _ = sleep(0).await`.
            if name == "_" {
                self.emit("let _ = ");
                self.emit(&final_rhs);
                self.emit(";\n");
                // No declaramos `_` en el scope — no es un binding.
            } else {
                self.emit("let mut ");
                self.emit(name);
                self.emit(": ");
                self.emit(&rust_type_for(&declared_ty, self.env)?);
                self.emit(" = ");
                self.emit(&final_rhs);
                self.emit(";\n");
                self.declare_var(name.clone(), declared_ty);
            }
        }
        Ok(())
    }

    /// Emite un `let X = <literal>` top-level de un módulo como
    /// `pub const X: T = ...;` (primitivos) o `pub static X: &str =
    /// "...";` (Str). La validación de "es un literal" ya la hizo
    /// `collect_module_sigs` antes; acá asumimos que el value es
    /// una de las variantes literales.
    fn gen_module_top_let(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let stmt_span = stmt.span();
        let Stmt::Assign { target, type_, value, .. } = stmt else {
            unreachable!("gen_module_top_let solo se llama sobre Stmt::Assign");
        };
        let AssignTarget::Ident(name) = target else {
            return Err(self.err_at(stmt_span,
                "asignación a campo a nivel top de módulo: no soportada (solo `let X = <literal>`)",
            ));
        };

        let declared_ty = match type_ {
            Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                self.err_at(value.span(), format!(
                    "let `{}`: anotación: {}",
                    name, e.message
                ))
            })?,
            None => infer_literal_type(value).ok_or_else(|| {
                self.err_at(value.span(), format!(
                    "let `{}` top-level de módulo: la RHS debe ser literal o tenés que anotar el tipo",
                    name
                ))
            })?,
        };

        // Str → `pub static X: &str = "...";`. Los otros → `pub const`.
        match (&declared_ty, value) {
            (Type::Str, Expr::Str(s, _)) => {
                writeln!(
                    &mut self.output,
                    "pub static {}: &str = {};\n",
                    name,
                    rust_str_literal(s)
                )
                .unwrap();
            }
            (Type::Int, Expr::Int(n, _)) => {
                writeln!(&mut self.output, "pub const {}: i64 = {}i64;\n", name, n).unwrap();
            }
            (Type::Float, Expr::Float(f, _)) => {
                writeln!(&mut self.output, "pub const {}: f64 = {}f64;\n", name, f).unwrap();
            }
            (Type::Float, Expr::Int(n, _)) => {
                // Coerción explícita Int → Float, como en el resto del codegen.
                writeln!(
                    &mut self.output,
                    "pub const {}: f64 = {}f64;\n",
                    name, *n as f64
                )
                .unwrap();
            }
            (Type::Bool, Expr::Bool(b, _)) => {
                writeln!(&mut self.output, "pub const {}: bool = {};\n", name, b).unwrap();
            }
            _ => {
                return Err(self.err_at(value.span(), format!(
                    "let `{}`: combinación de tipo/valor no soportada como constante de módulo",
                    name
                )));
            }
        }
        Ok(())
    }

    fn gen_field_assign(
        &mut self,
        object: &Expr,
        field: &str,
        value: &Expr,
    ) -> Result<(), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        let Type::Nominal(id) = &obj_ty else {
            return Err(self.err_at(object.span(), format!(
                "asignación a campo `.{}` sobre `{}`: solo se soporta sobre instancias",
                field,
                type_name(&obj_ty)
            )));
        };
        let info_name = self.env.info(*id).name.clone();
        // Defensivo: el checker garantiza fields resueltos. Si llegamos
        // acá sin fields, es un bug del compilador, no del usuario.
        let declared = self.fields_for_id(*id).ok_or_else(|| {
            self.err(format!(
                "tipo `{}` con campos sin resolver — no se puede generar asignación",
                info_name
            ))
        })?;
        let Some(f) = declared.iter().find(|f| f.name == field) else {
            return Err(self.err_at(object.span(), format!(
                "el tipo `{}` no tiene un campo llamado `{}`",
                info_name, field
            )));
        };
        let (rhs_code, rhs_ty) = self.gen_expr(value)?;
        let coerced = coerce(&rhs_code, &rhs_ty, &f.type_);
        self.emit_indent();
        writeln!(
            &mut self.output,
            "({}).lock().unwrap().{} = {};",
            obj_code, field, coerced
        )
        .unwrap();
        Ok(())
    }

    fn gen_return(&mut self, e: &Expr, ret_expected: &Type) -> Result<(), FitzError> {
        let (code, ty) = self.gen_expr(e)?;
        self.emit_indent();
        // MW.3: en el body de un middleware, `return` sin valor (o
        // `return null`) significa "continuar la cadena". Lo emitimos
        // como `return None;`. Para `return <status> { ... }` se usa
        // `gen_return_status` que ya envuelve en Some/__FitzResponse.
        // Para cualquier otro retorno del middleware (que el checker
        // permite porque el return type Fitz puede ser `Response?`):
        // si es `Null` (sin valor) → None; cualquier otra cosa es un
        // bug del usuario que el checker no caza (todavía) — emitimos
        // None con un comment para que falle bien si el código lo
        // toca.
        if self.in_middleware_fn {
            if matches!(ty, Type::Null) {
                self.emit("return None;\n");
                return Ok(());
            }
            // Si el middleware devuelve algo no-Null sin haber usado
            // `return <status> { ... }`, es un valor que no encaja como
            // gate-only. Por consistencia con MW.1 (donde el runtime da
            // 500 con mensaje claro), abortar con error de codegen:
            return Err(self.err_at(e.span(), format!(
                "middleware: `return` con un valor no-null no es válido — \
                 un middleware debe usar `return null` (o ningún return) \
                 para continuar la cadena, o `return <status> {{ ... }}` \
                 para cortocircuitar. Recibió `{}`",
                type_name(&ty)
            )));
        }
        if self.response_mode {
            // En response mode, todos los returns se envuelven en
            // `__FitzResponse { status: 200, body: <value>.__to_fitz_json() }`.
            // El status default para el path "no error" es 200; el
            // usuario puede pisarlo con `return 401 { ... }` que el
            // codegen de `Stmt::ReturnStatus` maneja aparte.
            // Caso especial: el body `Null` (sin valor de retorno) se
            // emite como `serde_json::Value::Null` para no llamar
            // `__to_fitz_json` sobre `()`.
            let body_code = if matches!(ty, Type::Null) {
                "serde_json::Value::Null".to_string()
            } else {
                format!(
                    "<{rt} as __ToFitzJson>::__to_fitz_json(&({code}))",
                    rt = rust_type_for(&ty, self.env)?,
                    code = code
                )
            };
            self.emit("return __FitzResponse { status: 200, body: ");
            self.emit(&body_code);
            self.emit(" };\n");
            return Ok(());
        }
        let coerced = coerce(&code, &ty, ret_expected);
        self.emit("return ");
        self.emit(&coerced);
        self.emit(";\n");
        Ok(())
    }

    /// `return <status> <body?>` adentro de un handler HTTP. Emite el
    /// wrap `__FitzResponse { status: <s>, body: <b>.__to_fitz_json() }`.
    /// Llamado solo desde `gen_stmt_in_fn`, donde validamos
    /// `response_mode == true` antes (fuera de eso es bug del codegen).
    fn gen_return_status(
        &mut self,
        status: &Expr,
        body: Option<&Expr>,
        span: crate::ast::Span,
    ) -> Result<(), FitzError> {
        if !self.response_mode {
            return Err(self.err_at(
                span,
                "`return <status> { ... }` solo permitido adentro de handlers HTTP — el checker debió haberlo cazado",
            ));
        }
        let (status_code, status_ty) = self.gen_expr(status)?;
        if !matches!(status_ty, Type::Int) {
            return Err(self.err_at(span, format!(
                "el status code de `return` debe ser Int, recibió `{}`",
                type_name(&status_ty)
            )));
        }
        let body_code = match body {
            Some(b) => {
                let (code, ty) = self.gen_expr(b)?;
                if matches!(ty, Type::Null) {
                    "serde_json::Value::Null".to_string()
                } else {
                    format!(
                        "<{rt} as __ToFitzJson>::__to_fitz_json(&({code}))",
                        rt = rust_type_for(&ty, self.env)?,
                        code = code
                    )
                }
            }
            None => "serde_json::Value::Null".to_string(),
        };
        self.emit_indent();
        // MW.3: en una fn middleware, el return type es
        // `Option<__FitzResponse>`. Envolvemos el __FitzResponse en
        // `Some(...)` para que el short-circuit sea visible al wrapper.
        if self.in_middleware_fn {
            self.emit(&format!(
                "return Some(__FitzResponse {{ status: ({}) as u16, body: {} }});\n",
                status_code, body_code
            ));
        } else {
            self.emit(&format!(
                "return __FitzResponse {{ status: ({}) as u16, body: {} }};\n",
                status_code, body_code
            ));
        }
        Ok(())
    }

    fn gen_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
        ret_expected: &Type,
    ) -> Result<(), FitzError> {
        let (cond_code, _) = self.gen_expr(condition)?;
        self.emit_indent();
        self.emit("while ");
        self.emit(&cond_code);
        self.emit(" {\n");
        self.indent += 1;
        self.push_scope();
        for s in body {
            self.gen_stmt_in_fn(s, ret_expected)?;
        }
        self.pop_scope();
        self.indent -= 1;
        self.emit_indent();
        self.emit("}\n");
        Ok(())
    }

    fn gen_loop(&mut self, body: &[Stmt], ret_expected: &Type) -> Result<(), FitzError> {
        self.emit_indent();
        self.emit("loop {\n");
        self.indent += 1;
        self.push_scope();
        for s in body {
            self.gen_stmt_in_fn(s, ret_expected)?;
        }
        self.pop_scope();
        self.indent -= 1;
        self.emit_indent();
        self.emit("}\n");
        Ok(())
    }

    fn gen_for(
        &mut self,
        var: &str,
        iter: &Expr,
        body: &[Stmt],
        ret_expected: &Type,
    ) -> Result<(), FitzError> {
        // Dos iterables soportados hoy:
        //   * `for v in start..end` — rango exclusivo (5b.1).
        //   * `for v in xs` con xs: List<T> — itera sobre snapshot.
        //     Snapshot (clone del Vec interno) para evitar re-entrancia
        //     al RefCell si el body muta la lista original. Mismo patrón
        //     que list_map en el intérprete.
        // Map como iterable directo NO se soporta (alineado con el
        // intérprete, que también lo rechaza).
        if let Expr::Range { start, end, .. } = iter {
            let (start_code, _) = self.gen_expr(start)?;
            let (end_code, _) = self.gen_expr(end)?;
            self.emit_indent();
            writeln!(
                &mut self.output,
                "for mut {var} in ({start_code} as i64)..({end_code} as i64) {{"
            )
            .unwrap();
            self.indent += 1;
            self.push_scope();
            self.declare_var(var.to_string(), Type::Int);
            for s in body {
                self.gen_stmt_in_fn(s, ret_expected)?;
            }
            self.pop_scope();
            self.indent -= 1;
            self.emit_indent();
            self.emit("}\n");
            return Ok(());
        }
        // Caso general: el iter tiene que evaluar a List<T>.
        let (iter_code, iter_ty) = self.gen_expr(iter)?;
        let elem_ty = match &iter_ty {
            Type::List(inner) => (**inner).clone(),
            other => {
                return Err(self.err_at(iter.span(), format!(
                    "`for {} in <expr>`: el iterable es `{}`, solo se soportan Range y List<T>",
                    var,
                    display_type(other, self.env)
                )));
            }
        };
        if matches!(elem_ty, Type::Any) {
            return Err(self.err_at(iter.span(), format!(
                "`for {} in ...` sobre `List<Any>`: el subset compilado exige tipo homogéneo \
                 concreto",
                var
            )));
        }
        self.emit_indent();
        writeln!(
            &mut self.output,
            "for mut {var} in ({iter_code}).lock().unwrap().clone().into_iter() {{"
        )
        .unwrap();
        self.indent += 1;
        self.push_scope();
        self.declare_var(var.to_string(), elem_ty);
        for s in body {
            self.gen_stmt_in_fn(s, ret_expected)?;
        }
        self.pop_scope();
        self.indent -= 1;
        self.emit_indent();
        self.emit("}\n");
        Ok(())
    }

    // --- generación de expresiones ----------------------------------------

    /// Devuelve `(código Rust de la expresión, tipo Fitz)`.
    fn gen_expr(&mut self, e: &Expr) -> Result<(String, Type), FitzError> {
        match e {
            Expr::Int(n, _) => Ok((format!("{}i64", n), Type::Int)),
            Expr::Float(n, _) => {
                // `1.0` ya es f64 literal en Rust; sufijo opcional
                // pero claro. Para evitar `inf`/`-inf` corner cases
                // delegamos al Display de f64 que produce literal
                // válido.
                Ok((format!("{}f64", n), Type::Float))
            }
            Expr::Str(s, _) => Ok((format!("String::from({})", rust_str_literal(s)), Type::Str)),
            Expr::Bool(b, _) => Ok((b.to_string(), Type::Bool)),
            Expr::Null(_) => Ok(("()".to_string(), Type::Null)),

            Expr::Ident(name, _) => {
                // Fase 8.7.2: bindings Python son globales — `math` se
                // traduce a `__fitz_py_bind_math()` (getter sobre
                // `OnceLock<__FitzPyObject>` que lazy-inicializa al
                // primer call). Tipo Fitz: `PyAny`.
                if self.python_bindings.contains_key(name) {
                    let lower = sanitize_python_binding_lower(name);
                    return Ok((format!("__fitz_py_bind_{}()", lower), Type::PyAny));
                }
                // 5b.5: si el nombre está en `module_bindings` como
                // `Named` y es Const, devolvemos el path directo. El
                // `use foo::PREFIX;` ya lo trajo al scope Rust;
                // `PREFIX` con tipo Str → para concat/format hay que
                // pasarlo con `.to_string()` o `&str` — depende del
                // contexto. Simplificamos: si es Str, emitimos
                // `String::from(PREFIX)` para que encaje con el resto
                // del codegen (`String` consistente).
                if let Some(ResolvedBinding::Named { module_index, item, kind }) =
                    self.module_bindings.get(name).cloned()
                {
                    if matches!(kind, NamedKind::Const) {
                        if let Some(m) = self.loaded_modules.get(module_index) {
                            if let Some(ty) = m.const_sigs.get(&item).cloned() {
                                // PreF8.4: el `use foo::PREFIX [as P];`
                                // ya bindeó el const al local `name`
                                // (que es la key del HashMap, no `item`).
                                // Emitimos por `name` para que `as`
                                // funcione transparente.
                                let code = match &ty {
                                    Type::Str => format!("String::from({})", name),
                                    _ => name.to_string(),
                                };
                                return Ok((code, ty));
                            }
                        }
                    }
                }
                // 5b.5: const top-level del propio módulo (emitida como
                // `pub static`/`pub const`). El fn body la referencia
                // por nombre — Rust resuelve.
                if let Some(ty) = self.own_consts.get(name).cloned() {
                    let code = match &ty {
                        Type::Str => format!("String::from({})", name),
                        _ => name.clone(),
                    };
                    return Ok((code, ty));
                }
                // Higher-order (F12): si el ident es una fn top-level
                // referenciada como **valor** (no como callee), emitimos
                // `(Arc::new(<name>) as Arc<dyn Fn(...) -> R>)`. Esto
                // habilita `let f = square` y `apply(square, 7)`. Las
                // fn items de Rust implementan `Fn(...)` así que el
                // `Arc::new(square)` compila directo. El caso "callee
                // de Call" se intercepta antes en `gen_call` (mira la
                // var como callable, no llega acá).
                if !self.var_in_any_scope(name) {
                    if let Some(sig) = self.fn_sigs.get(name).cloned() {
                        let ps_rs: Vec<String> = sig
                            .params
                            .iter()
                            .map(|p| rust_type_for(p, self.env))
                            .collect::<Result<_, _>>()?;
                        let ret_rs = rust_type_for(&sig.ret, self.env)?;
                        let code = format!(
                            "(Arc::new({}) as Arc<dyn Fn({}) -> {} + Send + Sync>)",
                            name,
                            ps_rs.join(", "),
                            ret_rs
                        );
                        let ty = Type::Function {
                            params: sig.params.clone(),
                            ret: Box::new(sig.ret.clone()),
                        };
                        return Ok((code, ty));
                    }
                }
                let ty = self
                    .lookup_var(name)
                    .cloned()
                    .ok_or_else(|| self.err(format!("variable desconocida en codegen: `{}`", name)))?;
                // Para tipos no-Copy (Str, Nominal, Option<...>),
                // generamos `.clone()` porque las expresiones consumen
                // por valor. Es ineficiente pero correcto. Para
                // Nominal el clone es del `Rc`, así que es barato y
                // preserva el aliasing — mutaciones siguen visibles.
                let code = if needs_clone(&ty) {
                    format!("{}.clone()", name)
                } else {
                    name.clone()
                };
                Ok((code, ty))
            }

            Expr::StrInterp(parts, _) => self.gen_str_interp(parts),

            Expr::BinOp { op, left, right, span } => self.gen_binop(op, left, right, *span),
            Expr::UnaryOp { op, operand, .. } => self.gen_unary(op, operand),

            Expr::Call { callee, args, span } => self.gen_call(callee, args, *span),

            Expr::If { condition, then, else_, span } => {
                self.gen_if_expr(condition, then, else_.as_deref(), *span)
            }

            Expr::Range { .. } => Err(self.err_at(e.span(),
                "`Range` solo se acepta como iterable de `for`; otros usos no se generan",
            )),
            Expr::List(items, span) => self.gen_list_lit(items, *span),
            Expr::Map(pairs, span) => self.gen_map_lit(pairs, *span),
            Expr::Index { object, index, span } => self.gen_index(object, index, *span),
            Expr::Field { object, field, span } => self.gen_field_access(object, field, *span),
            Expr::StructLit { type_name, fields, span } => self.gen_struct_lit(type_name, fields, *span),
            Expr::Ok(inner, _) => self.gen_ok(inner),
            Expr::Err(inner, _) => self.gen_err(inner),
            Expr::Try(inner, _) => self.gen_try(inner),
            // `.await` Fitz → `<expr>.await` Rust. Mapping 1:1 — el
            // checker 6.2 garantiza que el operando tipa como
            // `Future<T>` y que estamos adentro de una `async fn`.
            // El tipo resultante es el inner T del `Future<T>`; para
            // operando `Type::Any` (gradual) o desconocido devolvemos
            // `Type::Any` y dejamos que rustc infiera.
            //
            // Fase 8.7.3: si el inner es un call sobre receptor PyAny
            // (`py_async_fn().await`), despachamos a
            // `__fitz_py_invoke_await(...).await` que combina call +
            // detección de awaitable + ejecución vía `spawn_blocking` +
            // `asyncio.run_until_complete`. Paralelo a la detección
            // automática del intérprete 8.6.1 en `py_interop::call`.
            Expr::Await(inner, await_span) => {
                if let Some((code, ty)) = self.try_gen_python_await(inner)? {
                    return Ok((code, ty));
                }
                let _ = await_span;
                let (inner_code, inner_ty) = self.gen_expr(inner)?;
                let result_ty = match inner_ty {
                    Type::Future(t) => *t,
                    _ => Type::Any,
                };
                Ok((format!("({}).await", inner_code), result_ty))
            }
            Expr::Match { value, arms, .. } => self.gen_match(value, arms),
            // FnExpr "suelto" — usado como valor, parámetro o retorno
            // (higher-order, F12). Emite `Arc::new(move |p1: T1, ...|
            // -> R { body }) as Arc<dyn Fn(T1, ...) -> R>`. Los
            // callbacks inline de `.map`/`.filter`/`.find` siguen
            // interceptándose en `gen_method_call` antes de llegar
            // acá — esos no necesitan boxear porque el método los
            // consume directo. Acá llega cualquier FnExpr usado como
            // valor: `let f = fn(n) => ...`, `apply(fn(n) => ..., 7)`,
            // `return fn(y) => x + y`.
            Expr::FnExpr { params, body, span } => self.gen_fn_expr_as_value(params, body, *span),
            // Fase 9.0.1 (F15): defensa — `fitz build` no debería ver
            // `Expr::Error` (strict parser nunca lo produce).
            Expr::Error(span) => Err(self.err_at(*span,
                "nodo `Expr::Error` en el AST — `fitz build` usa el parser strict, no debería verlo (bug del compilador, Fase 9.0.1)",
            )),
        }
    }

    /// Para statements `Stmt::Expr(e, Span::ZERO)`: si `e` es una llamada a
    /// `print(...)`, generamos `println!(...)` (que devuelve `()`).
    /// El resto cae al `gen_expr` normal.
    fn gen_expr_for_stmt(&mut self, e: &Expr) -> Result<(), FitzError> {
        if let Expr::Call { callee, args, .. } = e {
            if let Expr::Ident(name, _) = callee.as_ref() {
                if name == "print" {
                    return self.gen_print(args);
                }
            }
        }
        let (code, _) = self.gen_expr(e)?;
        self.emit(&code);
        Ok(())
    }

    /// Genera el código de un `print(...)` como **String** (no lo emite
    /// directo al output). Usado por contextos donde necesitamos la
    /// llamada al `println!` como expresión adentro de otra estructura,
    /// p.ej. un arm de `match`. `call_expr` debe ser un `Expr::Call`
    /// con callee `print`; el caller ya lo validó vía `is_print_call`.
    fn gen_print_to_string(&mut self, call_expr: &Expr) -> Result<String, FitzError> {
        let Expr::Call { args, .. } = call_expr else {
            return Err(self.err("gen_print_to_string llamada con expr que no es Call"));
        };
        let saved_indent = self.indent;
        self.indent = 0;
        let (out, result) = self.with_temp_output(|ctx| ctx.gen_print(args));
        self.indent = saved_indent;
        result?;
        Ok(out)
    }

    fn gen_print(&mut self, args: &[Expr]) -> Result<(), FitzError> {
        if args.is_empty() {
            self.emit("println!()");
            return Ok(());
        }
        // Para cada arg evaluamos el código y elegimos cómo formatearlo:
        //   * tipos "simples" (Int, Str, Bool) → `{}` con el arg directo,
        //     que es lo que el `println!` nativo ya hace bien;
        //   * tipos que necesitan formato custom (Float con `.0`, Null
        //     como `"null"`, instancias delegando a Display, Options
        //     desempaquetando a `null`) → expresión via `show_expr`
        //     que evalúa a `String`, todavía pasada con `{}`.
        let mut pieces: Vec<String> = Vec::with_capacity(args.len());
        for a in args {
            let (code, ty) = self.gen_expr(a)?;
            let piece = match &ty {
                Type::Int | Type::Bool | Type::Str => code,
                _ => show_expr(&code, &ty),
            };
            pieces.push(piece);
        }
        let format_str: String = std::iter::repeat_n("{}", args.len())
            .collect::<Vec<_>>()
            .join(" ");
        self.emit(&format!(
            "println!(\"{}\", {})",
            format_str,
            pieces.join(", ")
        ));
        Ok(())
    }

    fn gen_str_interp(&mut self, parts: &[StrPart]) -> Result<(String, Type), FitzError> {
        let mut fmt = String::new();
        let mut args: Vec<String> = Vec::new();
        for p in parts {
            match p {
                StrPart::Lit(s) => {
                    // Escapamos `{` y `}` para el format string.
                    for c in s.chars() {
                        match c {
                            '{' => fmt.push_str("{{"),
                            '}' => fmt.push_str("}}"),
                            '\\' => fmt.push_str("\\\\"),
                            '"' => fmt.push_str("\\\""),
                            _ => fmt.push(c),
                        }
                    }
                }
                StrPart::Expr(e) => {
                    fmt.push_str("{}");
                    let (code, ty) = self.gen_expr(e)?;
                    // Para tipos formateables nativos (Int/Bool/Str),
                    // pasamos la expresión directo. Para el resto
                    // (Float con `.0`, Null como `null`, instancias
                    // por Display, Option desempacando) usamos
                    // `show_expr` que devuelve un `String`.
                    let piece = match &ty {
                        Type::Int | Type::Bool | Type::Str => code,
                        _ => show_expr(&code, &ty),
                    };
                    args.push(piece);
                }
            }
        }
        let call = if args.is_empty() {
            format!("String::from(\"{}\")", fmt)
        } else {
            format!("format!(\"{}\", {})", fmt, args.join(", "))
        };
        Ok((call, Type::Str))
    }

    fn gen_binop(
        &mut self,
        op: &BinOpKind,
        left: &Expr,
        right: &Expr,
        span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        let (lc, lt) = self.gen_expr(left)?;
        let (rc, rt) = self.gen_expr(right)?;
        match op {
            BinOpKind::Add => {
                // Str+Str → format!("{}{}", a, b).
                if matches!(lt, Type::Str) && matches!(rt, Type::Str) {
                    return Ok((format!("format!(\"{{}}{{}}\", {}, {})", lc, rc), Type::Str));
                }
                let (l, r, t) = numeric_coerce(&lc, &lt, &rc, &rt)
                    .ok_or_else(|| self.err_at(span, format!(
                        "operador `+` no aplicable a `{}` y `{}` en codegen",
                        type_name(&lt),
                        type_name(&rt)
                    )))?;
                Ok((format!("({} + {})", l, r), t))
            }
            BinOpKind::Sub | BinOpKind::Mul | BinOpKind::Div => {
                let sym = match op {
                    BinOpKind::Sub => "-",
                    BinOpKind::Mul => "*",
                    BinOpKind::Div => "/",
                    _ => unreachable!(),
                };
                let (l, r, t) = numeric_coerce(&lc, &lt, &rc, &rt)
                    .ok_or_else(|| self.err_at(span, format!(
                        "operador `{}` no aplicable a `{}` y `{}` en codegen",
                        sym, type_name(&lt), type_name(&rt)
                    )))?;
                Ok((format!("({} {} {})", l, sym, r), t))
            }
            BinOpKind::Lt | BinOpKind::LtEq | BinOpKind::Gt | BinOpKind::GtEq => {
                let sym = match op {
                    BinOpKind::Lt => "<",
                    BinOpKind::LtEq => "<=",
                    BinOpKind::Gt => ">",
                    BinOpKind::GtEq => ">=",
                    _ => unreachable!(),
                };
                // Para Str: usamos `as_str()` para comparar.
                if matches!(lt, Type::Str) && matches!(rt, Type::Str) {
                    return Ok((
                        format!("({}.as_str() {} {}.as_str())", lc, sym, rc),
                        Type::Bool,
                    ));
                }
                let (l, r, _t) = numeric_coerce(&lc, &lt, &rc, &rt)
                    .ok_or_else(|| self.err_at(span, format!(
                        "comparación entre `{}` y `{}` no aplicable",
                        type_name(&lt), type_name(&rt)
                    )))?;
                Ok((format!("({} {} {})", l, sym, r), Type::Bool))
            }
            BinOpKind::Eq | BinOpKind::NotEq => {
                let sym = match op {
                    BinOpKind::Eq => "==",
                    BinOpKind::NotEq => "!=",
                    _ => unreachable!(),
                };
                // Comparación contra Null sobre un Nullable: el lado
                // Nullable es `Option<T>` en Rust y `null` Fitz es `()`,
                // así que `Option<T> == ()` no compila. Lo traducimos
                // a `.is_none()` / `.is_some()`.
                let null_check = |opt_code: &str, eq: bool| -> String {
                    if eq {
                        format!("({}).is_none()", opt_code)
                    } else {
                        format!("({}).is_some()", opt_code)
                    }
                };
                let is_eq = matches!(op, BinOpKind::Eq);
                if matches!(lt, Type::Nullable(_)) && matches!(rt, Type::Null) {
                    return Ok((null_check(&lc, is_eq), Type::Bool));
                }
                if matches!(rt, Type::Nullable(_)) && matches!(lt, Type::Null) {
                    return Ok((null_check(&rc, is_eq), Type::Bool));
                }
                // Igualdad estructural entre instancias del mismo
                // tipo: borroweamos ambos lados y comparamos por
                // valor — `#[derive(PartialEq)]` sobre `FooData`
                // recursea campo a campo (incluyendo nominales
                // anidados como `Arc<Mutex<T>>`, que comparan por
                // contenido, no identidad).
                if let (Type::Nominal(id_l), Type::Nominal(id_r)) = (&lt, &rt) {
                    if id_l != id_r {
                        return Err(self.err(
                            "igualdad entre instancias de tipos distintos: el checker debería haberlo cazado",
                        ));
                    }
                    return Ok((
                        format!("(*({}).lock().unwrap() {} *({}).lock().unwrap())", lc, sym, rc),
                        Type::Bool,
                    ));
                }
                if matches!(lt, Type::Str) && matches!(rt, Type::Str) {
                    return Ok((format!("({} {} {})", lc, sym, rc), Type::Bool));
                }
                // Numéricos con posible coerción Int↔Float.
                if let Some((l, r, _)) = numeric_coerce(&lc, &lt, &rc, &rt) {
                    return Ok((format!("({} {} {})", l, sym, r), Type::Bool));
                }
                // Bools, Null directos.
                Ok((format!("({} {} {})", lc, sym, rc), Type::Bool))
            }
            BinOpKind::And => Ok((format!("({} && {})", lc, rc), Type::Bool)),
            BinOpKind::Or => Ok((format!("({} || {})", lc, rc), Type::Bool)),
        }
    }

    fn gen_unary(
        &mut self,
        op: &UnaryOpKind,
        operand: &Expr,
    ) -> Result<(String, Type), FitzError> {
        let (code, ty) = self.gen_expr(operand)?;
        match op {
            UnaryOpKind::Neg => Ok((format!("(-{})", code), ty)),
        }
    }

    fn gen_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        // Method call: el callee es `Expr::Field { object, field, .. }`.
        // Despachamos por `(tipo del receptor, nombre del método)`
        // como hace el evaluator. Hoy solo cubrimos métodos built-in
        // sobre Str; List/Map y métodos custom sobre `type` quedan
        // como deuda (llegan en 5b.3 y post-3.2 respectivamente).
        //
        // 5b.5: caso especial — si el object es `Ident(ns)` con `ns`
        // siendo namespace de módulo, traducimos `foo.greet(args)` →
        // `foo::greet(args)` Rust con la firma del módulo.
        if let Expr::Field { object, field, .. } = callee {
            if let Expr::Ident(ns, _) = object.as_ref() {
                if let Some(ResolvedBinding::Namespace { .. }) =
                    self.module_bindings.get(ns).cloned()
                {
                    if let Some((path, sig)) = self.resolve_namespace_call(ns, field) {
                        return self.gen_call_with_sig(&path, &sig, args, call_span);
                    }
                    return Err(self.err_at(call_span, format!(
                        "el módulo `{}` no exporta una función llamada `{}`",
                        ns, field
                    )));
                }
            }
            return self.gen_method_call(object, field, args, call_span);
        }
        let Expr::Ident(name, _) = callee else {
            return Err(self.err_at(callee.span(),
                "llamadas con callee complejo (FnExpr inline u otro Expr): no soportadas",
            ));
        };
        // Fase 8.7.2: si el callee es un binding Python (ident con
        // tipo PyAny), emitimos `__fitz_py_invoke(&<callee>, |py| { ...args... })`
        // con marshaling adentro del closure. El resultado tipa como
        // `Result<PyAny>`; el `?` Fitz / la coerción primitiva al sitio
        // destino se aplica después.
        if self.python_bindings.contains_key(name)
            || matches!(self.lookup_var(name), Some(Type::PyAny))
        {
            let (callee_code, _) = self.gen_expr(callee)?;
            let args_code = self.gen_python_call_args(args)?;
            let code = format!(
                "__fitz_py_invoke(&{callee}, |py| {{ Ok(vec![{args}]) }})",
                callee = callee_code,
                args = args_code,
            );
            return Ok((code, Type::Result(Box::new(Type::PyAny))));
        }
        if name == "print" {
            return Err(self.err_at(call_span,
                "`print(...)` solo puede usarse como sentencia, no como expresión en 5b.1",
            ));
        }
        // Fase 6.6: builtin `sleep(ms: Int) -> Future<Null>`. Si el
        // usuario definió una fn `sleep` propia, `fn_sigs` la captura
        // antes y el builtin no dispara — misma política que `len`.
        if name == "sleep" && !self.fn_sigs.contains_key(name) {
            if args.len() != 1 {
                return Err(self.err_at(call_span, format!(
                    "`sleep` espera 1 argumento (ms: Int), recibió {}",
                    args.len()
                )));
            }
            let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
            let coerced = coerce(&arg_code, &arg_ty, &Type::Int);
            return Ok((
                format!("__fitz_sleep({})", coerced),
                Type::Future(Box::new(Type::Null)),
            ));
        }
        // Builtin global `len(x)`: despacha por tipo del argumento a la
        // misma implementación que el método (`.len()`). Cubre Str, List
        // y Map. Si el usuario tiene una fn `len` definida (raro pero
        // válido), su sig prevalece — chequeamos `fn_sigs` antes del
        // builtin.
        if name == "len" && !self.fn_sigs.contains_key(name) && args.len() == 1 {
            let arg_span = args[0].span();
            let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
            return match arg_ty {
                Type::Str => Ok((
                    format!("(({}).chars().count() as i64)", arg_code),
                    Type::Int,
                )),
                Type::List(_) | Type::Map(_, _) => Ok((
                    format!("(({}).lock().unwrap().len() as i64)", arg_code),
                    Type::Int,
                )),
                other => Err(self.err_at(arg_span, format!(
                    "`len(...)`: no aplica a `{}` — solo Str, List<T> y Map<K, V>",
                    display_type(&other, self.env)
                ))),
            };
        }
        // 5b.5: si el nombre está en `module_bindings` como `Named`
        // (`from foo import greet`), la firma viene del módulo, no
        // de `fn_sigs`. El `use foo::greet;` ya lo agregó al scope
        // Rust, así que el call se emite con el name directo.
        if let Some(ResolvedBinding::Named { module_index, item, kind }) =
            self.module_bindings.get(name).cloned()
        {
            if matches!(kind, NamedKind::Fn) {
                if let Some(m) = self.loaded_modules.get(module_index) {
                    if let Some(sig) = m.fn_sigs.get(&item).cloned() {
                        return self.gen_call_with_sig(name, &sig, args, call_span);
                    }
                }
            }
        }

        // Higher-order (F12): si el name está bindeado en algún
        // scope local como `Type::Function`, es una var que contiene
        // un closure → llamarla con `(*f)(args)` o `f(args)`. Rc<dyn
        // Fn> implementa `Fn` directamente; rustc auto-derefs.
        if let Some(Type::Function { params, ret }) = self.lookup_var(name).cloned() {
            let sig = FnSig { params, ret: *ret };
            return self.gen_call_with_sig(name, &sig, args, call_span);
        }

        let sig = self
            .fn_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("función `{}` desconocida en codegen", name)))?;
        self.gen_call_with_sig(name, &sig, args, call_span)
    }

    /// Emite una llamada con una firma conocida: `<callee>(arg1, arg2, ...)`
    /// con coerciones por parámetro. El `callee_expr` puede ser un
    /// identificador (`greet`) o un path Rust (`foo::greet`); ambos
    /// son válidos como prefijo de `(...)`.
    fn gen_call_with_sig(
        &mut self,
        callee_expr: &str,
        sig: &FnSig,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        if args.len() != sig.params.len() {
            return Err(self.err_at(call_span, format!(
                "`{}` espera {} argumento(s), recibió {}",
                callee_expr,
                sig.params.len(),
                args.len()
            )));
        }
        let mut arg_codes = Vec::with_capacity(args.len());
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            let (code, ty) = self.gen_expr(a)?;
            arg_codes.push(coerce(&code, &ty, expected));
        }
        Ok((
            format!("{}({})", callee_expr, arg_codes.join(", ")),
            sig.ret.clone(),
        ))
    }

    fn gen_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        // Fase 8.7.2: method call sobre PyAny es realmente un call
        // Python (`math.sqrt(16.0)` = `math.sqrt` getattr + invocación).
        // Emitimos `__fitz_py_invoke(&__fitz_py_get_attr_obj(&obj, "name"), ...)`
        // con marshaling de args adentro del closure. Resultado:
        // `Result<PyAny>` que se desempaca en el sitio destino vía `?`
        // o coerción primitiva.
        let _ = call_span; // span queda en err_at adentro si hace falta
        if matches!(obj_ty, Type::PyAny) {
            let args_code = self.gen_python_call_args(args)?;
            let code = format!(
                "__fitz_py_invoke(&__fitz_py_get_attr_obj(&{obj}, {name}), |py| {{ Ok(vec![{args}]) }})",
                obj = obj_code,
                name = rust_str_literal(method),
                args = args_code,
            );
            return Ok((code, Type::Result(Box::new(Type::PyAny))));
        }
        match (&obj_ty, method) {
            // ---- Str ----
            (Type::Str, "len") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("(({}).chars().count() as i64)", obj_code), Type::Int))
            }
            (Type::Str, "upper") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).to_uppercase()", obj_code), Type::Str))
            }
            (Type::Str, "lower") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).to_lowercase()", obj_code), Type::Str))
            }
            (Type::Str, other) => Err(self.err_at(call_span, format!(
                "Str no tiene el método `{}` en el subset compilado (hoy: len/upper/lower)",
                other
            ))),

            // ---- List ----
            (Type::List(t), "push") => self.gen_list_push(&obj_code, t, args),
            (Type::List(t), "pop") => self.gen_list_pop(&obj_code, t, args),
            (Type::List(_), "len") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("(({}).lock().unwrap().len() as i64)", obj_code), Type::Int))
            }
            (Type::List(t), "map") => self.gen_list_map(&obj_code, t, args),
            (Type::List(t), "filter") => self.gen_list_filter(&obj_code, t, args),
            (Type::List(t), "find") => self.gen_list_find(&obj_code, t, args),
            (Type::List(_), other) => Err(self.err_at(call_span, format!(
                "List no tiene el método `{}` en el subset compilado (hoy: push/pop/len/map/filter)",
                other
            ))),

            // ---- Map ----
            (Type::Map(k, _), "has") => self.gen_map_has(&obj_code, k, args),
            (Type::Map(k, _), "keys") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "Arc::new(Mutex::new(({}).lock().unwrap().iter().map(|(__k, _)| __k.clone()).collect::<Vec<_>>()))",
                    obj_code
                );
                Ok((code, Type::List(Box::new((**k).clone()))))
            }
            (Type::Map(_, v), "values") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "Arc::new(Mutex::new(({}).lock().unwrap().iter().map(|(_, __v)| __v.clone()).collect::<Vec<_>>()))",
                    obj_code
                );
                Ok((code, Type::List(Box::new((**v).clone()))))
            }
            (Type::Map(_, _), "len") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("(({}).lock().unwrap().len() as i64)", obj_code), Type::Int))
            }
            (Type::Map(k, v), "get") => self.gen_map_get(&obj_code, k, v, args),
            (Type::Map(_, _), other) => Err(self.err_at(call_span, format!(
                "Map no tiene el método `{}` en el subset compilado (hoy: has/keys/values/len)",
                other
            ))),

            // ---- Tipos custom ----
            (Type::Nominal(_), m) => Err(self.err_at(call_span, format!(
                "métodos custom sobre `type` (`.{}`): primero hay que cerrar la deuda de 3.2 en el parser",
                m
            ))),

            // ---- Otros ----
            (other, m) => Err(self.err_at(call_span, format!(
                "method call `.{}` sobre `{}`: no soportado en codegen",
                m,
                display_type(other, self.env)
            ))),
        }
    }

    // --- métodos List ----------------------------------------------------

    /// `xs.push(x)` → `({xs}).lock().unwrap().push({coerce x → T})`. Devuelve
    /// `()` (Null en Fitz). El stmt-mode agrega el `;` final por encima.
    fn gen_list_push(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("push", args, 1)?;
        let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
        let coerced = coerce(&arg_code, &arg_ty, elem_ty);
        let code = format!("({}).lock().unwrap().push({})", obj_code, coerced);
        Ok((code, Type::Null))
    }

    /// `xs.pop()` → `({xs}).lock().unwrap().pop().expect(...)`. El intérprete
    /// tira error de runtime sobre lista vacía con ese mensaje; el binario
    /// generado paniquea — comportamiento esencial (abortar con mensaje)
    /// equivalente.
    fn gen_list_pop(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("pop", args, 0)?;
        let code = format!(
            "({}).lock().unwrap().pop().expect(\"`.pop()` sobre lista vacía\")",
            obj_code
        );
        Ok((code, elem_ty.clone()))
    }

    /// `xs.map(callback)` → snapshot del Vec + map + collect, envuelto en
    /// `Arc::new(Mutex::new(...))`. El callback debe ser un FnExpr
    /// inline; no admitimos referencias a fns nombradas hoy (eso necesita
    /// higher-order, deuda explícita).
    fn gen_list_map(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("map", args, 1)?;
        let (callback_code, ret_ty) =
            self.gen_callback_inline(&args[0], elem_ty, None, "map")?;
        let code = format!(
            "{{ \
                let __items: Vec<_> = ({}).lock().unwrap().clone(); \
                Arc::new(Mutex::new(__items.into_iter().map({}).collect::<Vec<_>>())) \
            }}",
            obj_code, callback_code
        );
        Ok((code, Type::List(Box::new(ret_ty))))
    }

    /// `xs.filter(callback)` → snapshot + for-loop manual + push. Evitamos
    /// `.filter(...).collect()` porque el `filter` de Iterator pasa `&T`
    /// y el callback de Fitz toma T por valor. El loop manual clona el
    /// item para pasárselo al callback (para Nominal/List/Map es clone
    /// del Rc → barato) y mueve el original al output si el predicado
    /// retorna true.
    fn gen_list_filter(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("filter", args, 1)?;
        let (callback_code, _) =
            self.gen_callback_inline(&args[0], elem_ty, Some(&Type::Bool), "filter")?;
        let code = format!(
            "{{ \
                let __items: Vec<_> = ({}).lock().unwrap().clone(); \
                let __cb = {}; \
                let mut __out: Vec<_> = Vec::new(); \
                for __it in __items.into_iter() {{ \
                    if __cb(__it.clone()) {{ __out.push(__it); }} \
                }} \
                Arc::new(Mutex::new(__out)) \
            }}",
            obj_code, callback_code
        );
        Ok((code, Type::List(Box::new(elem_ty.clone()))))
    }

    /// `xs.find(callback)` → bloque que itera el snapshot y devuelve
    /// `Ok(item)` al primer match, `Err("no encontrado")` si nada matchea.
    /// Devuelve `Result<T, String>`. Habilita el patrón canónico
    /// `users.find(fn(u) => u.id == id)?` (con `?` propagando el Err).
    fn gen_list_find(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("find", args, 1)?;
        let (callback_code, _) =
            self.gen_callback_inline(&args[0], elem_ty, Some(&Type::Bool), "find")?;
        let elem_rs = rust_type_for(elem_ty, self.env)?;
        let code = format!(
            "{{ \
                let __items: Vec<_> = ({}).lock().unwrap().clone(); \
                let __cb = {}; \
                let mut __result: Result<{}, String> = \
                    Err(String::from(\"no encontrado\")); \
                for __it in __items.into_iter() {{ \
                    if __cb(__it.clone()) {{ __result = Ok(__it); break; }} \
                }} \
                __result \
            }}",
            obj_code, callback_code, elem_rs
        );
        Ok((code, Type::Result(Box::new(elem_ty.clone()))))
    }

    // --- métodos Map -----------------------------------------------------

    /// `m.get(k)` → búsqueda lineal por igualdad. Devuelve `Ok(v)` si
    /// la clave existe, `Err("clave no encontrada: <k>")` si no. Mensaje
    /// idéntico al del intérprete. Tipo retornado: `Result<V, String>`.
    fn gen_map_get(
        &mut self,
        obj_code: &str,
        key_ty: &Type,
        val_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("get", args, 1)?;
        let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
        let coerced_key = coerce(&arg_code, &arg_ty, key_ty);
        // Para el mensaje de error, formateamos la clave con el mismo
        // estilo del intérprete (`Display` de Value, **sin** comillas
        // para Str: `clave no encontrada: z`, no `clave no encontrada:
        // "z"`). Eso lo da `show_expr` (modo "print top-level"), no
        // `show_expr_inline` (modo "adentro de lista/mapa", que sí mete
        // comillas).
        let key_show = show_expr("__k", key_ty);
        let val_rs = rust_type_for(val_ty, self.env)?;
        let code = format!(
            "{{ \
                let __map = {}; \
                let __k = {}; \
                let __pairs = __map.lock().unwrap(); \
                let mut __result: Result<{}, String> = Err(format!(\"clave no encontrada: {{}}\", {})); \
                for (__k2, __v) in __pairs.iter() {{ \
                    if __k2 == &__k {{ __result = Ok(__v.clone()); break; }} \
                }} \
                __result \
            }}",
            obj_code, coerced_key, val_rs, key_show
        );
        Ok((code, Type::Result(Box::new(val_ty.clone()))))
    }

    /// `m.has(k)` → búsqueda lineal por igualdad → bool.
    fn gen_map_has(
        &mut self,
        obj_code: &str,
        key_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("has", args, 1)?;
        let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
        let coerced = coerce(&arg_code, &arg_ty, key_ty);
        let code = format!(
            "{{ let __k = {}; ({}).lock().unwrap().iter().any(|(__k2, _)| __k2 == &__k) }}",
            coerced, obj_code
        );
        Ok((code, Type::Bool))
    }

    // --- helpers para callback inline ------------------------------------

    /// Genera el código Rust de un closure inline a partir de un `FnExpr`.
    /// `param_ty` es el tipo que el receptor (List<T>) impone al param;
    /// el FnExpr puede traer anotación propia, pero la del receptor
    /// manda (el checker ya validó compatibilidad).
    ///
    /// `expected_ret_ty = Some(t)` fuerza el tipo de retorno (caso
    /// `filter` que exige `Bool`). `None` infiere desde el primer
    /// `return` del body, o el último `Stmt::Expr` no-print, o `Null`.
    /// La heurística cubre arrow form (`fn(x) => e`) y bodies simples
    /// con un solo return — los casos exóticos (returns adentro de cada
    /// rama de un `if`) caen a `Null` por simplicidad y requieren
    /// reescribir el callback en arrow form si el tipo importa.
    ///
    /// Devuelve `(código del closure, tipo de retorno inferido/forzado)`.
    fn gen_callback_inline(
        &mut self,
        arg: &Expr,
        param_ty: &Type,
        expected_ret_ty: Option<&Type>,
        method: &str,
    ) -> Result<(String, Type), FitzError> {
        let arg_span = arg.span();
        let (params, body) = match arg {
            Expr::FnExpr { params, body, .. } => (params, body),
            _ => {
                return Err(self.err_at(arg_span, format!(
                    "`.{}(...)` exige un callback inline `fn(x) => ...` o `fn(x) {{ ... }}`. \
                     Pasar una fn nombrada como callback (higher-order) llega en un sub-paso \
                     posterior de 5b.",
                    method
                )));
            }
        };
        if params.len() != 1 {
            return Err(self.err_at(arg_span, format!(
                "el callback de `.{}` toma 1 parámetro, recibió {}",
                method,
                params.len()
            )));
        }
        let param_name = params[0].name.clone();

        // Inferimos el ret type en dry-run sobre el primer Stmt::Return
        // del body, o el último Stmt::Expr no-print, o Null.
        let inferred_ret = self.infer_callback_ret_silently(body, &param_name, param_ty)?;
        let ret_ty = expected_ret_ty.cloned().unwrap_or_else(|| inferred_ret.clone());

        let param_ty_rs = rust_type_for(param_ty, self.env)?;
        let ret_ty_rs = rust_type_for(&ret_ty, self.env)?;

        // Emit el body en un buffer aparte, con el param ligado.
        // Pushear el ret_stack del callback para que un `?` adentro
        // del cuerpo del closure pueda chequearse correctamente (en
        // la práctica el checker prohibe `?` adentro de FnExpr inline
        // salvo cuando el return_stack es Any, así que esto suele
        // ser inerte; lo mantenemos por consistencia).
        self.ret_stack.push(ret_ty.clone());
        // Reset response_mode: el body del callback es otra fn (closure
        // Rust), no comparte el retorno del handler contenedor. Si dentro
        // del callback hay un `return`, debe envolver el valor con sus
        // propias reglas (`bool` para filter, T → U para map). Status
        // codes custom (`return 401 { ... }`) NO son válidos adentro de
        // callbacks — el checker los rechazaría porque el FnExpr inline
        // no es un handler HTTP.
        let saved_response_mode = self.response_mode;
        self.response_mode = false;
        self.push_scope();
        self.declare_var(param_name.clone(), param_ty.clone());
        let saved_indent = self.indent;
        self.indent = 0;
        let ret_ty_for_body = ret_ty.clone();
        let (body_str, result) = self.with_temp_output(|ctx| {
            for s in body {
                ctx.gen_stmt_in_fn(s, &ret_ty_for_body)?;
            }
            Ok::<(), FitzError>(())
        });
        self.indent = saved_indent;
        self.pop_scope();
        self.response_mode = saved_response_mode;
        self.ret_stack.pop();
        result?;

        let code = format!(
            "|{}: {}| -> {} {{ {} }}",
            param_name, param_ty_rs, ret_ty_rs, body_str
        );
        Ok((code, ret_ty))
    }

    /// Emite un `FnExpr` "suelto" (no callback inline de
    /// map/filter/find) como **valor** de tipo `Arc<dyn Fn(...) -> R>`.
    /// Cubre `let f = fn(n) => n * 2`, `apply(fn(n) => n * 10, 7)`,
    /// `return fn(y) => x + y` (closure que captura `x` del scope
    /// contenedor). Por uniformidad emitimos siempre con `move` y
    /// el cast a `Arc<dyn Fn(...) -> R>` para que rustc no se queje
    /// de "type annotations needed" cuando el contexto destino es
    /// `Rc<dyn Fn>` (un closure concreto y un trait object son tipos
    /// distintos; el `as` los reconcilia).
    ///
    /// Captura: dejamos que rustc resuelva quién va con `move`. El
    /// closure pide los identifiers del scope contenedor por valor;
    /// si son Copy (i64/f64/bool), la copia es trivial; si son
    /// Str/Rc/...  el `move` los **consume**. Eso rompe si el caller
    /// los necesita después. Mitigación: para vars no-Copy
    /// referenciadas en el body (y no shadowed por param/local), el
    /// helper detecta capturas, las clona afuera y deja que la
    /// closure capture la copia.
    fn gen_fn_expr_as_value(
        &mut self,
        params: &[crate::ast::Param],
        body: &[Stmt],
        fn_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        // Cada param exige anotación de tipo — sin contexto bidireccional
        // no podemos inferir el tipo del param desde su uso. Esta es la
        // misma regla que aplican las fns top-level (deuda 5b.1).
        let mut param_types: Vec<Type> = Vec::with_capacity(params.len());
        for p in params {
            let Some(te) = p.type_.as_ref() else {
                return Err(self.err_at(fn_span, format!(
                    "función anónima `fn({})`: el parámetro `{}` necesita una anotación de \
                     tipo en el subset compilable (deuda 5b.1). Anotalo o usá `fitz run`.",
                    p.name, p.name
                )));
            };
            let t = resolve_type_expr(te, self.env).map_err(|e| {
                self.err_at(fn_span, format!(
                    "función anónima: parámetro `{}`: {}",
                    p.name, e.message
                ))
            })?;
            param_types.push(t);
        }

        // Detectamos capturas: identifiers del scope contenedor
        // referenciados en el body, excluyendo los params del FnExpr.
        // Para cada captura no-Copy, emitimos un binding local
        // `let __cap_<name> = <name>.clone();` y el closure captura
        // **la copia**. Para Copy no hace falta — rustc copia.
        let param_names: std::collections::HashSet<String> =
            params.iter().map(|p| p.name.clone()).collect();
        let mut captures: Vec<(String, Type)> = Vec::new();
        collect_captures(body, &param_names, self, &mut captures);

        // Sintetizamos el ret type del closure desde el body.
        // Reusamos `infer_callback_ret_silently` con el primer param;
        // si no hay params, hacemos un dry-run con scope vacío. Las
        // capturas también deben estar visibles en el dry-run para
        // que `gen_expr` resuelva sus tipos.
        let ret_ty = self.infer_fn_expr_ret_silently(body, params, &param_types, &captures)?;
        let ret_ty_rs = rust_type_for(&ret_ty, self.env)?;

        // Emitimos el body adentro de la closure con scope nuevo +
        // params bindeados + capturas declaradas como vars locales
        // (para que el body las pueda referenciar normalmente).
        self.ret_stack.push(ret_ty.clone());
        // FnExpr suelta (higher-order F12): el body es otra fn con su
        // propio return type. Status codes custom no aplican adentro de
        // FnExpr (el checker los rechaza fuera de handlers HTTP), así
        // que reseteamos response_mode mientras emitimos el body para
        // que cualquier `return` adentro use el camino normal.
        let saved_response_mode = self.response_mode;
        self.response_mode = false;
        self.push_scope();
        for (p, t) in params.iter().zip(param_types.iter()) {
            self.declare_var(p.name.clone(), t.clone());
        }
        for (name, ty) in &captures {
            self.declare_var(name.clone(), ty.clone());
        }
        let saved_indent = self.indent;
        self.indent = 0;
        let ret_ty_for_body = ret_ty.clone();
        let (body_str, result) = self.with_temp_output(|ctx| {
            for s in body {
                ctx.gen_stmt_in_fn(s, &ret_ty_for_body)?;
            }
            Ok::<(), FitzError>(())
        });
        self.indent = saved_indent;
        self.pop_scope();
        self.response_mode = saved_response_mode;
        self.ret_stack.pop();
        result?;

        // Firma del closure: `|p1: T1, p2: T2| -> R { ... }`.
        let params_sig = params
            .iter()
            .zip(param_types.iter())
            .map(|(p, t)| Ok(format!("{}: {}", p.name, rust_type_for(t, self.env)?)))
            .collect::<Result<Vec<_>, FitzError>>()?
            .join(", ");
        let cast_target = {
            let ps: Vec<String> = param_types
                .iter()
                .map(|p| rust_type_for(p, self.env))
                .collect::<Result<_, _>>()?;
            format!("Arc<dyn Fn({}) -> {} + Send + Sync>", ps.join(", "), ret_ty_rs)
        };

        // Si hay capturas no-Copy, emitimos un bloque que cline las
        // capturas afuera del closure y después construye la closure
        // con `move`. Cada captura se rebindea: `let <name> = <name>.clone();`.
        // Esto preserva el aliasing semántico (clone del Rc para
        // List/Map/Nominal/Function) sin consumir la var del caller.
        let mut clones = String::new();
        for (name, ty) in &captures {
            if needs_clone(ty) {
                clones.push_str(&format!("let {0} = {0}.clone(); ", name));
            }
        }

        let closure = format!(
            "|{params_sig}| -> {ret_ty_rs} {{ {body_str} }}",
            params_sig = params_sig,
            ret_ty_rs = ret_ty_rs,
            body_str = body_str
        );
        let code = if clones.is_empty() {
            format!("(Arc::new(move {closure}) as {cast_target})", closure = closure, cast_target = cast_target)
        } else {
            format!(
                "{{ {clones}Arc::new(move {closure}) as {cast_target} }}",
                clones = clones,
                closure = closure,
                cast_target = cast_target
            )
        };

        Ok((
            code,
            Type::Function {
                params: param_types,
                ret: Box::new(ret_ty),
            },
        ))
    }

    /// Dry-run del body de un `FnExpr` para sintetizar el ret type.
    /// Como en `infer_callback_ret_silently`: scope nuevo con params y
    /// capturas bindeados, gen_expr sobre el primer `Stmt::Return` del
    /// body (o último `Stmt::Expr` no-print, o `Null`).
    fn infer_fn_expr_ret_silently(
        &mut self,
        body: &[Stmt],
        params: &[crate::ast::Param],
        param_types: &[Type],
        captures: &[(String, Type)],
    ) -> Result<Type, FitzError> {
        let target: Option<&Expr> = body
            .iter()
            .find_map(|s| if let Stmt::Return(e, _) = s { Some(e) } else { None })
            .or_else(|| {
                body.last().and_then(|s| match s {
                    Stmt::Expr(e, _) if !is_print_call(e) => Some(e),
                    _ => None,
                })
            });
        let Some(e) = target else { return Ok(Type::Null) };

        self.push_scope();
        for (p, t) in params.iter().zip(param_types.iter()) {
            self.declare_var(p.name.clone(), t.clone());
        }
        for (name, ty) in captures {
            self.declare_var(name.clone(), ty.clone());
        }
        let (_discarded, result) = self.with_temp_output(|ctx| ctx.gen_expr(e));
        self.pop_scope();
        result.map(|(_, t)| t)
    }

    /// Dry-run para sintetizar el tipo de retorno de un callback. Pushea
    /// el scope del param, recorre el body buscando el primer
    /// `Stmt::Return(e, Span::ZERO)` (o el último `Stmt::Expr(e, Span::ZERO)` no-print), llama
    /// a `gen_expr` con `self.output` redirigido a un buffer descartable
    /// (no contamina la salida real).
    fn infer_callback_ret_silently(
        &mut self,
        body: &[Stmt],
        param_name: &str,
        param_ty: &Type,
    ) -> Result<Type, FitzError> {
        let target: Option<&Expr> = body
            .iter()
            .find_map(|s| if let Stmt::Return(e, _) = s { Some(e) } else { None })
            .or_else(|| {
                body.last().and_then(|s| match s {
                    Stmt::Expr(e, _) if !is_print_call(e) => Some(e),
                    _ => None,
                })
            });
        let Some(e) = target else { return Ok(Type::Null) };

        self.push_scope();
        self.declare_var(param_name.to_string(), param_ty.clone());
        let (_discarded, result) = self.with_temp_output(|ctx| ctx.gen_expr(e));
        self.pop_scope();
        result.map(|(_, t)| t)
    }

    // --- Result, `?`, match (5b.4) ----------------------------------------

    /// `Ok(e)` → `Ok(<coerced e>)`. El tipo de Fitz es `Result<T>` donde
    /// T es el tipo sintetizado del inner. El Err side queda como `String`
    /// (pinned, ver `rust_type_for`), pero acá no lo materializamos —
    /// rustc lo infiere desde el contexto destino (anotación / return
    /// type / brazo del match opuesto).
    fn gen_ok(&mut self, inner: &Expr) -> Result<(String, Type), FitzError> {
        let (code, ty) = self.gen_expr(inner)?;
        Ok((format!("Ok({})", code), Type::Result(Box::new(ty))))
    }

    /// `Err(e)` → `Err(<e como String>)`. El Err side está pinned a String
    /// en el código generado (decisión 5b.4): si el inner ya es Str, se
    /// usa directo; si no, se coerce con `format!("{}", x)` para preservar
    /// la práctica de "Err con mensaje" del intérprete y de los ejemplos.
    /// El tipo Fitz sintetizado es `Result<Any>` — no conocemos el T del
    /// Ok side, el contexto destino lo refinará.
    fn gen_err(&mut self, inner: &Expr) -> Result<(String, Type), FitzError> {
        let (code, ty) = self.gen_expr(inner)?;
        let as_string = match ty {
            Type::Str => code,
            _ => format!("format!(\"{{}}\", {})", code),
        };
        Ok((format!("Err({})", as_string), Type::Result(Box::new(Type::Any))))
    }

    /// `expr?` — operador de propagación de errores. En Rust, `?` solo
    /// funciona adentro de fns que retornen `Result<_, _>` con E compatible
    /// (acá siempre `String`). Validamos contra `ret_stack` y emitimos
    /// `<expr>?` directo: rustc se encarga de la propagación y del
    /// desempaque del Ok.
    fn gen_try(&mut self, inner: &Expr) -> Result<(String, Type), FitzError> {
        let inner_span = inner.span();
        let (code, ty) = self.gen_expr(inner)?;
        let inner_ty = match &ty {
            Type::Result(t) => (**t).clone(),
            // Any cae a gradual: probablemente vino de un `.find()` u otro
            // call que el checker no pudo tipar concreto. Asumimos que es
            // Result y dejamos que rustc lo confirme. (En la práctica
            // 5b.4 los métodos built-in dan Result concreto, así que este
            // camino no se ejerce mucho.)
            Type::Any => Type::Any,
            other => {
                return Err(self.err_at(inner_span, format!(
                    "operador `?` sobre `{}`: el operando debe ser `Result<T>`",
                    display_type(other, self.env)
                )));
            }
        };
        let ret = self.ret_stack.last().cloned().unwrap_or(Type::Null);
        match &ret {
            Type::Result(_) => {}
            Type::Any => {}
            _ => {
                return Err(self.err_at(inner_span,
                    "operador `?` solo puede usarse adentro de una función que retorne \
                     `Result<...>`",
                ));
            }
        }
        Ok((format!("({})?", code), inner_ty))
    }

    /// `match scrutinee { pat1 => expr1, ... }` → `match` Rust. El match
    /// se emite siempre como **expresión** Rust (`match s { ... }`); cuando
    /// se usa en stmt position, el `;` de `Stmt::Expr` lo cierra; cuando
    /// alimenta una asignación o un return, su valor es el lub de los arms.
    ///
    /// Patrones soportados:
    ///   - Int/Float/Str/Bool/Null literales → patrones Rust directos.
    ///   - Ident (binding) → `name`, captura el scrutinee.
    ///   - Wildcard `_`.
    ///   - Ok(x) / Err(e) → `Ok(x)` / `Err(e)` Rust nativos.
    ///   - Ok(_) / Err(_) → `Ok(_)` / `Err(_)`.
    ///   - Range `a..b` → guard `n if (a..b).contains(&n)` (no patterns
    ///     `a..b` directos para evitar ediciones con exhaustividad).
    ///
    /// Exhaustividad: si los arms no cubren todo (ni Ident/Wildcard ni
    /// las dos variantes de Result), agregamos `_ => panic!(...)` con
    /// el mismo mensaje que el intérprete ("el `match` no matcheó
    /// ningún brazo") para que rustc compile.
    fn gen_match(
        &mut self,
        value: &Expr,
        arms: &[crate::ast::MatchArm],
    ) -> Result<(String, Type), FitzError> {
        let (scrut_code, scrut_ty) = self.gen_expr(value)?;
        let inner_ok_ty = match &scrut_ty {
            Type::Result(t) => Some((**t).clone()),
            _ => None,
        };

        let mut arm_pieces: Vec<String> = Vec::with_capacity(arms.len() + 1);
        let mut arm_tys: Vec<Type> = Vec::with_capacity(arms.len());
        let mut has_catch_all = false;
        let mut has_ok = false;
        let mut has_err = false;

        for arm in arms {
            self.push_scope();
            let pat_code = self.gen_pattern(&arm.pattern, &scrut_ty, &inner_ok_ty)?;
            match &arm.pattern {
                crate::ast::Pattern::Ident(_) | crate::ast::Pattern::Wildcard => {
                    has_catch_all = true;
                }
                crate::ast::Pattern::OkBinding(_) | crate::ast::Pattern::OkWildcard => {
                    has_ok = true;
                }
                crate::ast::Pattern::ErrBinding(_) | crate::ast::Pattern::ErrWildcard => {
                    has_err = true;
                }
                _ => {}
            }
            // `print(...)` adentro del arm no es una expresión Fitz (es
            // statement). Lo emitimos como bloque `{ println!(...); }`
            // que evalúa a `()`. Para el resto delegamos a `gen_expr`.
            let (body_code, body_ty) = if is_print_call(&arm.body) {
                let print_code = self.gen_print_to_string(&arm.body)?;
                (format!("{{ {}; }}", print_code), Type::Null)
            } else {
                self.gen_expr(&arm.body)?
            };
            self.pop_scope();
            arm_pieces.push(format!("{} => {}", pat_code, body_code));
            arm_tys.push(body_ty);
        }

        // Determinar si necesitamos un catch-all artificial para que
        // rustc acepte el match. Casos exhaustivos sin agregar nada:
        //   - hay un Ident/Wildcard arm;
        //   - el scrutinee es Result<T> y tenemos al menos un Ok y un Err.
        let result_exhaustive =
            inner_ok_ty.is_some() && has_ok && has_err;
        if !has_catch_all && !result_exhaustive {
            arm_pieces.push(
                "_ => panic!(\"el `match` no matcheó ningún brazo\")".to_string(),
            );
        }

        // Tipo de salida: lub de los arms; si fallan a unificar, Any.
        let result_ty = if arm_tys.is_empty() {
            Type::Null
        } else {
            let mut acc = arm_tys[0].clone();
            for t in &arm_tys[1..] {
                acc = lub(&acc, t).unwrap_or(Type::Any);
            }
            acc
        };

        let code = format!(
            "(match {} {{ {} }})",
            scrut_code,
            arm_pieces.join(", ")
        );
        Ok((code, result_ty))
    }

    /// Traduce un `Pattern` Fitz a su equivalente como pattern Rust,
    /// declarando en el scope actual cualquier binding que el pattern
    /// introduzca. `scrut_ty` ayuda con literales (negativos, Str con
    /// comillas correctas); `ok_inner_ty` da el tipo del binding `x`
    /// en `Ok(x)` cuando el scrutinee es `Result<T>`.
    fn gen_pattern(
        &mut self,
        pat: &crate::ast::Pattern,
        scrut_ty: &Type,
        ok_inner_ty: &Option<Type>,
    ) -> Result<String, FitzError> {
        use crate::ast::Pattern;
        match pat {
            Pattern::Int(n) => Ok(format!("{}i64", n)),
            Pattern::Float(f) => Ok(format!("{}f64", f)),
            Pattern::Str(s) => {
                // Comparamos con literal `&str` adentro del match
                // contra `String`. Como Rust no acepta `"x"` como
                // pattern contra `String` directo, usamos un guard:
                // `_ if x.as_str() == "..."`. Esto descarta el
                // scrutinee del binding, pero es válido. Emitimos un
                // pattern fresco que matchea y el guard hace el check.
                Ok(format!("ref __s if __s.as_str() == {}", rust_str_literal(s)))
            }
            Pattern::Bool(b) => Ok(b.to_string()),
            Pattern::Null => {
                // `Null` Fitz se mapea a `()` Rust; el pattern `()`
                // matchea sólo `()`. Si el scrutinee no es Null/() el
                // caso es inalcanzable, pero rustc lo acepta.
                if matches!(scrut_ty, Type::Null) {
                    Ok("()".to_string())
                } else {
                    // Para Option<T> u otros: dejamos `_` con guard
                    // estructural — pero el patrón Null sobre tipos
                    // que no son Null es raro y el checker lo veta;
                    // por simplicidad lo dejamos pasar como `_` que
                    // nunca matchea correctamente. (Caso teórico.)
                    Ok("_".to_string())
                }
            }
            Pattern::Ident(name) => {
                self.declare_var(name.clone(), scrut_ty.clone());
                Ok(name.clone())
            }
            Pattern::Wildcard => Ok("_".to_string()),
            Pattern::OkBinding(name) => {
                let bind_ty = ok_inner_ty.clone().unwrap_or(Type::Any);
                self.declare_var(name.clone(), bind_ty);
                Ok(format!("Ok({})", name))
            }
            Pattern::ErrBinding(name) => {
                // El Err side está pinned a `String` en el código
                // generado, así que el binding es siempre String.
                self.declare_var(name.clone(), Type::Str);
                Ok(format!("Err({})", name))
            }
            Pattern::OkWildcard => Ok("Ok(_)".to_string()),
            Pattern::ErrWildcard => Ok("Err(_)".to_string()),
            Pattern::Range { start, end } => {
                // Rust acepta patterns `start..end` como exclusivos en
                // `match` desde 2018, pero solo para tipos primitivos
                // con `PartialOrd`. Para `i64`, funciona — pero el
                // pattern requiere ser exhaustivo o tener catch-all.
                // Para evitar conflictos con la cobertura, emitimos
                // un guard sobre un binding fresco.
                Ok(format!(
                    "__n if ({}i64..{}i64).contains(&__n)",
                    start, end
                ))
            }
        }
    }

    // --- listas, mapas, indexing ------------------------------------------

    /// `[e1, e2, ...]` → `Arc::new(Mutex::new(vec![v1, v2, ...]))` con
    /// coerción de cada elemento al tipo común. Tipo común sintetizado
    /// como en el checker (5.3.1): primer elemento define el tipo, los
    /// demás deben unificar via `lub` (Int↔Float, T↔Null). Mezcla
    /// irrecuperable o lista vacía sin contexto → error claro.
    fn gen_list_lit(&mut self, items: &[Expr], list_span: crate::ast::Span) -> Result<(String, Type), FitzError> {
        if items.is_empty() {
            // Lista vacía: no podemos sintetizar T. Emitimos un código
            // genérico `Vec::new()` y devolvemos `List<Any>`. El
            // contexto (anotación destino, paso a fn tipada) coerciona
            // a un T concreto; si nadie lo restringe, el rustc generado
            // fallará con "type annotations needed", reflejando que el
            // usuario tiene que anotar.
            return Ok((
                "Arc::new(Mutex::new(Vec::new()))".to_string(),
                Type::List(Box::new(Type::Any)),
            ));
        }
        let mut item_codes_tys: Vec<(String, Type)> = Vec::with_capacity(items.len());
        for it in items {
            let (c, t) = self.gen_expr(it)?;
            item_codes_tys.push((c, t));
        }
        let mut common_ty = item_codes_tys[0].1.clone();
        for (_, t) in &item_codes_tys[1..] {
            common_ty = lub(&common_ty, t).map_err(|_| {
                self.err_at(list_span, format!(
                    "lista con elementos de tipos incompatibles (`{}` y `{}`): el subset compilado \
                     exige una lista homogénea (todos del mismo tipo, con coerciones Int→Float y \
                     T→T? permitidas)",
                    display_type(&common_ty, self.env),
                    display_type(t, self.env),
                ))
            })?;
        }
        if matches!(common_ty, Type::Any) {
            return Err(self.err_at(list_span,
                "lista con elementos cuyo tipo común es `Any`: el subset compilado exige tipo \
                 homogéneo concreto. Anotá el tipo o usá `fitz run` para interpretarlo sin restricción.",
            ));
        }
        let coerced: Vec<String> = item_codes_tys
            .iter()
            .map(|(c, t)| coerce(c, t, &common_ty))
            .collect();
        let code = format!(
            "Arc::new(Mutex::new(vec![{}]))",
            coerced.join(", ")
        );
        Ok((code, Type::List(Box::new(common_ty))))
    }

    /// `{k1: v1, k2: v2, ...}` → `Arc::new(Mutex::new(vec![(k1, v1), ...]))`.
    /// Orden de inserción preservado por Vec. K y V deben ser homogéneos
    /// (mismas reglas que List). Para `m["k"]` (Index) y `m.get(k)` la
    /// búsqueda es lineal O(n), pero matchea exactamente lo que hace
    /// el intérprete.
    fn gen_map_lit(&mut self, pairs: &[(Expr, Expr)], map_span: crate::ast::Span) -> Result<(String, Type), FitzError> {
        if pairs.is_empty() {
            return Ok((
                "Arc::new(Mutex::new(Vec::new()))".to_string(),
                Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
            ));
        }
        let mut entries: Vec<((String, Type), (String, Type))> = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let kt = self.gen_expr(k)?;
            let vt = self.gen_expr(v)?;
            entries.push((kt, vt));
        }
        let mut common_k = entries[0].0 .1.clone();
        let mut common_v = entries[0].1 .1.clone();
        for ((_, kt), (_, vt)) in &entries[1..] {
            common_k = lub(&common_k, kt).map_err(|_| {
                self.err_at(map_span, format!(
                    "mapa con claves de tipos incompatibles (`{}` y `{}`): el subset compilado \
                     exige claves homogéneas",
                    display_type(&common_k, self.env),
                    display_type(kt, self.env),
                ))
            })?;
            common_v = lub(&common_v, vt).map_err(|_| {
                self.err_at(map_span, format!(
                    "mapa con valores de tipos incompatibles (`{}` y `{}`): el subset compilado \
                     exige valores homogéneos",
                    display_type(&common_v, self.env),
                    display_type(vt, self.env),
                ))
            })?;
        }
        if matches!(common_k, Type::Any) || matches!(common_v, Type::Any) {
            return Err(self.err_at(map_span,
                "mapa con claves o valores cuyo tipo común es `Any`: el subset compilado exige \
                 tipos homogéneos concretos. Anotá el tipo o usá `fitz run` para interpretarlo \
                 sin restricción.",
            ));
        }
        let pieces: Vec<String> = entries
            .iter()
            .map(|((kc, kt), (vc, vt))| {
                format!(
                    "({}, {})",
                    coerce(kc, kt, &common_k),
                    coerce(vc, vt, &common_v)
                )
            })
            .collect();
        let code = format!(
            "Arc::new(Mutex::new(vec![{}]))",
            pieces.join(", ")
        );
        Ok((code, Type::Map(Box::new(common_k), Box::new(common_v))))
    }

    /// `obj[idx]` — dispatch por tipo del receptor.
    ///
    ///   - `List<T>[Int]`   → `({xs}.lock().unwrap()[idx as usize].clone())`.
    ///     Index out-of-bounds panicea en Rust (igual que el intérprete
    ///     que tira error de runtime).
    ///   - `Map<K, V>[K]`   → búsqueda lineal por igualdad. Si no hay,
    ///     panic con mensaje al estilo del intérprete.
    ///
    /// El clone del item es del Rc para Nominal/List/Map → barato y
    /// preserva el aliasing con la colección original (mutar via
    /// `xs[0].name = "x"` se ve en xs).
    fn gen_index(
        &mut self,
        object: &Expr,
        index: &Expr,
        index_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        let (idx_code, idx_ty) = self.gen_expr(index)?;
        match &obj_ty {
            Type::List(inner) => {
                if !matches!(idx_ty, Type::Int) {
                    return Err(self.err_at(index.span(), format!(
                        "indexing de lista con `{}`: el índice debe ser Int",
                        display_type(&idx_ty, self.env)
                    )));
                }
                let code = format!(
                    "({}).lock().unwrap()[({}) as usize].clone()",
                    obj_code, idx_code
                );
                Ok((code, (**inner).clone()))
            }
            Type::Map(k_ty, v_ty) => {
                let coerced_idx = coerce(&idx_code, &idx_ty, k_ty);
                // Búsqueda lineal por igualdad. `unwrap_or_else(panic)` con
                // mensaje al estilo del intérprete. Ligamos el Rc a una
                // var local antes de `.lock().unwrap()` para extender la vida
                // del temporal — `(m.clone()).lock().unwrap()` solo cuando la
                // expresión completa cabe en una stmt simple; acá usamos
                // un `let __m = ...` y necesitamos el holder.
                let code = format!(
                    "{{ \
                        let __map = {}; \
                        let __m = __map.lock().unwrap(); \
                        let __k = {}; \
                        __m.iter() \
                            .find(|(__k2, _)| __k2 == &__k) \
                            .map(|(_, __v)| __v.clone()) \
                            .unwrap_or_else(|| panic!(\"clave no encontrada en mapa: {{:?}}\", __k)) \
                    }}",
                    obj_code, coerced_idx
                );
                Ok((code, (**v_ty).clone()))
            }
            other => Err(self.err_at(index_span, format!(
                "indexing `[]` sobre `{}`: solo soportado en List<T> y Map<K, V>",
                display_type(other, self.env)
            ))),
        }
    }

    fn gen_struct_lit(
        &mut self,
        type_name: &str,
        provided: &[(String, Expr)],
        struct_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        let sig = self
            .type_sigs
            .get(type_name)
            .cloned()
            .ok_or_else(|| self.err(format!("tipo `{}` desconocido en codegen", type_name)))?;

        // Validamos campos extra. El checker debería haberlo cazado;
        // este chequeo es defensa en profundidad.
        for (provided_name, _) in provided {
            if !sig.fields.iter().any(|f| &f.name == provided_name) {
                return Err(self.err_at(struct_span, format!(
                    "el tipo `{}` no tiene un campo llamado `{}`",
                    type_name, provided_name
                )));
            }
        }

        // PreF8.3: si el tipo viene de un `from foo import T`, los
        // defaults se materializan vía helper fns del módulo
        // (`foo::__default_T_<field>()`). Eso evita resolver Idents del
        // default en el scope del importer, donde tipos referenciados
        // por el default (consts, otros types del módulo de origen) no
        // están visibles.
        // PreF8.4: si el tipo es importado con alias (`from foo import
        // User as MyUser`), `type_name` acá es el alias local; necesitamos
        // el `item` (nombre dentro del módulo) para nombrar la helper
        // `foo::__default_User_<field>()` correctamente.
        let imported_mod_and_item = match self.module_bindings.get(type_name) {
            Some(ResolvedBinding::Named {
                module_index,
                item,
                kind: NamedKind::Type,
            }) => Some((
                self.loaded_modules[*module_index].mod_name.clone(),
                item.clone(),
            )),
            _ => None,
        };

        // Construimos los pares (campo, código Rust) en orden de
        // declaración del `type`. Esto importa para Display y para
        // futuras igualdades.
        let mut field_codes: Vec<String> = Vec::with_capacity(sig.fields.len());
        for f in &sig.fields {
            let supplied = provided.iter().find(|(n, _)| n == &f.name);
            let value_code = if let Some((_, expr)) = supplied {
                let (code, ty) = self.gen_expr(expr)?;
                coerce(&code, &ty, &f.type_)
            } else if let Some(default_expr) = &f.default {
                if let Some((mod_name, item)) = &imported_mod_and_item {
                    // Llamada a la helper fn del módulo de origen. Su
                    // body ya retorna el tipo correcto, sin coerce extra.
                    // Usamos `item` (nombre dentro del módulo), no
                    // `type_name` (alias local).
                    format!("{}::__default_{}_{}()", mod_name, item, f.name)
                } else {
                    let (code, ty) = self.gen_expr(default_expr)?;
                    coerce(&code, &ty, &f.type_)
                }
            } else if matches!(f.type_, Type::Nullable(_)) {
                "None".to_string()
            } else {
                return Err(self.err_at(struct_span, format!(
                    "falta el campo `{}` al instanciar `{}` (no tiene default y no es nullable)",
                    f.name, type_name
                )));
            };
            field_codes.push(format!("{}: {}", f.name, value_code));
        }

        let data_name = format!("{}Data", type_name);
        let code = format!(
            "Arc::new(Mutex::new({} {{ {} }}))",
            data_name,
            field_codes.join(", ")
        );
        let nominal_id = sig.id;
        Ok((code, Type::Nominal(nominal_id)))
    }

    fn gen_field_access(
        &mut self,
        object: &Expr,
        field: &str,
        field_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        // 5b.5: si el objeto es `Ident(ns)` con `ns` siendo un namespace
        // de módulo importado (`import foo`), traducimos `foo.bar` a
        // path Rust `foo::bar`. Lo hacemos ANTES de evaluar el objeto,
        // porque `Ident("foo")` no está en `scopes` (los imports no
        // declaran var en el codegen), pero sí en `module_bindings`.
        if let Expr::Ident(ns, _) = object {
            if let Some(ResolvedBinding::Namespace { .. }) =
                self.module_bindings.get(ns).cloned()
            {
                if let Some((code, ty)) = self.resolve_namespace_field(ns, field) {
                    return Ok((code, ty));
                }
                return Err(self.err_at(field_span, format!(
                    "el módulo `{}` no exporta `{}` (ni fn ni constante)",
                    ns, field
                )));
            }
        }

        let (obj_code, obj_ty) = self.gen_expr(object)?;
        // Fase 8.7.1: field access sobre objeto Python (`math.pi`,
        // `obj.attr`). Emite `__fitz_py_get_attr_obj(&obj, "name")`
        // que devuelve un `__FitzPyObject` opaco. La auto-coerción a
        // tipos primitivos pasa después, en el sitio donde se usa el
        // resultado (vía `coerce(..., PyAny → T)`).
        if matches!(obj_ty, Type::PyAny) {
            return Ok((
                format!(
                    "__fitz_py_get_attr_obj(&{}, {})",
                    obj_code,
                    rust_str_literal(field)
                ),
                Type::PyAny,
            ));
        }
        let Type::Nominal(id) = &obj_ty else {
            return Err(self.err_at(object.span(), format!(
                "field access `.{}` sobre `{}`: solo se soporta sobre instancias de tipos custom",
                field,
                type_name(&obj_ty)
            )));
        };
        let info_name = self.env.info(*id).name.clone();
        // Defensivo: el checker garantiza fields resueltos. Si llegamos
        // acá sin fields, es un bug del compilador, no del usuario.
        let declared = self.fields_for_id(*id).ok_or_else(|| {
            self.err(format!(
                "tipo `{}` con campos sin resolver — no se puede generar acceso",
                info_name
            ))
        })?;
        let Some(f) = declared.iter().find(|f| f.name == field) else {
            return Err(self.err_at(field_span, format!(
                "el tipo `{}` no tiene un campo llamado `{}`",
                info_name, field
            )));
        };
        // F17.4b: emitimos el acceso como bloque con scope acotado.
        // Cada acceso bindea su propio Arc + guard en el bloque, así el
        // guard se libera al fin del bloque (inmediato) y no al fin del
        // statement contenedor. Sin esto, dos accesos al mismo valor
        // adentro de un `format!(...)` o de cualquier expresión con
        // múltiples sub-expresiones intentarían tomar el lock dos veces
        // del mismo thread → deadlock (std::sync::Mutex no es
        // reentrante). El `let __obj` extra evita E0716 (temporary
        // dropped while borrowed) — sin ese binding, el `(u.clone())`
        // temporal moriría mientras el guard lo borrowea.
        let access = if needs_clone(&f.type_) {
            format!(
                "{{ let __obj = {}; let __g = __obj.lock().unwrap(); __g.{}.clone() }}",
                obj_code, field
            )
        } else {
            format!(
                "{{ let __obj = {}; let __g = __obj.lock().unwrap(); __g.{} }}",
                obj_code, field
            )
        };
        Ok((access, f.type_.clone()))
    }

    fn gen_if_expr(
        &mut self,
        condition: &Expr,
        then: &[Stmt],
        else_: Option<&[Stmt]>,
        if_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        let (cond_code, _) = self.gen_expr(condition)?;

        // Si ambas ramas (else incluido) terminan en un `Stmt::Expr`
        // que sea expresable como valor, el `if` es expresión con
        // valor. Si no, lo tratamos como statement con valor `()`
        // (`Type::Null`) y emitimos cada bloque entero como stmts.
        let (then_stmts, then_tail) = split_tail_expr(then);
        let (else_stmts_opt, else_tail) = match else_ {
            Some(body) => {
                let (s, t) = split_tail_expr(body);
                (Some(s), t)
            }
            None => (None, None),
        };

        let want_value = else_stmts_opt.is_some()
            && then_tail.is_some()
            && else_tail.is_some();

        if want_value {
            // Modo expresión: evaluamos los tails y unificamos.
            let (then_block, then_tail_code, then_tail_ty) = {
                self.push_scope();
                let stmts = self.gen_block_to_string(&then_stmts)?;
                let (c, t) = self.gen_expr(then_tail.unwrap())?;
                self.pop_scope();
                (stmts, c, t)
            };
            let (else_block, else_tail_code, else_tail_ty) = {
                self.push_scope();
                let stmts = self.gen_block_to_string(&else_stmts_opt.clone().unwrap())?;
                let (c, t) = self.gen_expr(else_tail.unwrap())?;
                self.pop_scope();
                (stmts, c, t)
            };
            let result_ty = lub(&then_tail_ty, &else_tail_ty).map_err(|_| {
                self.err_at(if_span, format!(
                    "ramas de `if` con tipos incompatibles: `{}` y `{}`",
                    type_name(&then_tail_ty),
                    type_name(&else_tail_ty)
                ))
            })?;
            let then_tail_coerced = coerce(&then_tail_code, &then_tail_ty, &result_ty);
            let else_tail_coerced = coerce(&else_tail_code, &else_tail_ty, &result_ty);
            let code = format!(
                "(if {} {{\n{}{}{}\n{}}} else {{\n{}{}{}\n{}}})",
                cond_code,
                then_block,
                self.indent_str(),
                then_tail_coerced,
                self.indent_str_outer(),
                else_block,
                self.indent_str(),
                else_tail_coerced,
                self.indent_str_outer(),
            );
            Ok((code, result_ty))
        } else {
            // Modo statement: re-emitimos los tails como stmts
            // (`gen_stmt` se encarga del `;` y la indentación).
            let then_block = {
                self.push_scope();
                let mut full = self.gen_block_to_string(&then_stmts)?;
                if let Some(e) = then_tail {
                    full.push_str(&self.gen_stmt_to_string(&Stmt::Expr(e.clone(), crate::ast::Span::ZERO))?);
                }
                self.pop_scope();
                full
            };
            let mut code = format!("if {} {{\n{}{}}}", cond_code, then_block, self.indent_str_outer());
            if let Some(else_stmts) = else_stmts_opt {
                let else_block = {
                    self.push_scope();
                    let mut full = self.gen_block_to_string(&else_stmts)?;
                    if let Some(e) = else_tail {
                        full.push_str(&self.gen_stmt_to_string(&Stmt::Expr(e.clone(), crate::ast::Span::ZERO))?);
                    }
                    self.pop_scope();
                    full
                };
                write!(
                    &mut code,
                    " else {{\n{}{}}}",
                    else_block,
                    self.indent_str_outer()
                )
                .unwrap();
            }
            Ok((code, Type::Null))
        }
    }

    /// Redirige `self.output` a un buffer temporal mientras corre `f`,
    /// y devuelve `(output capturado, valor de retorno de f)`. Restaura
    /// el output original al salir, incluso si `f` retorna `Err`
    /// (vía drop normal del `mem::replace`). Sirve para emitir
    /// fragmentos que después se inyectan en otra estructura (p.ej.
    /// brazos de `match`, bodies de `if`/`else` en modo expresión).
    ///
    /// La indentación NO se gestiona acá — el caller decide si la
    /// modifica adentro de `f` y la restaura.
    fn with_temp_output<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> R,
    ) -> (String, R) {
        let saved = std::mem::take(&mut self.output);
        let result = f(self);
        let captured = std::mem::replace(&mut self.output, saved);
        (captured, result)
    }

    /// Emite los `stmts` redirigiendo `self.output` a un buffer
    /// temporal y devuelve el resultado. Restaura el output original
    /// antes de devolver. La indentación actual se respeta (los
    /// `emit_indent` van con `self.indent + 1` porque entran en un
    /// `if`/`else` body).
    fn gen_block_to_string(&mut self, stmts: &[&Stmt]) -> Result<String, FitzError> {
        self.indent += 1;
        let (out, result) = self.with_temp_output(|ctx| {
            for s in stmts {
                ctx.gen_stmt(s)?;
            }
            Ok::<(), FitzError>(())
        });
        self.indent -= 1;
        result?;
        Ok(out)
    }

    fn gen_stmt_to_string(&mut self, stmt: &Stmt) -> Result<String, FitzError> {
        self.indent += 1;
        let (out, result) = self.with_temp_output(|ctx| ctx.gen_stmt(stmt));
        self.indent -= 1;
        result?;
        Ok(out)
    }

    fn indent_str(&self) -> String {
        "    ".repeat(self.indent + 1)
    }

    fn indent_str_outer(&self) -> String {
        "    ".repeat(self.indent)
    }

    // ------------------------------------------------------------------
    // 5b.6 — HTTP / @server / handlers
    // ------------------------------------------------------------------

    /// Emite el preludio HTTP: traits `__ToFitzJson` y `__FromFitzJson`,
    /// implementaciones para primitivos / List / Map / Option / Result,
    /// helpers de error response. Los impls específicos por `type` se
    /// emiten junto al struct (en `gen_type_http_impls`).
    fn emit_http_runtime_prelude(&mut self) {
        self.emit(HTTP_RUNTIME_PRELUDE);
    }

    /// Emite los `impl __ToFitzJson` y `impl __FromFitzJson` para un
    /// `type Foo` particular. Llamado después de `gen_type_def`.
    fn gen_type_http_impls(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::TypeDef { name, .. } = stmt else {
            return Ok(());
        };
        let sig = self
            .type_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("tipo `{}` no pre-registrado", name)))?;
        let data_name = format!("{}Data", name);

        // impl __ToFitzJson for <Foo>Data
        writeln!(
            &mut self.output,
            "impl __ToFitzJson for {} {{",
            data_name
        )
        .unwrap();
        self.emit("    fn __to_fitz_json(&self) -> serde_json::Value {\n");
        self.emit("        let mut __obj = serde_json::Map::new();\n");
        for f in &sig.fields {
            writeln!(
                &mut self.output,
                "        __obj.insert(\"{}\".to_string(), self.{}.__to_fitz_json());",
                f.name, f.name
            )
            .unwrap();
        }
        self.emit("        serde_json::Value::Object(__obj)\n");
        self.emit("    }\n}\n\n");

        // impl __FromFitzJson for <Foo>Data
        writeln!(
            &mut self.output,
            "impl __FromFitzJson for {} {{",
            data_name
        )
        .unwrap();
        self.emit("    fn __from_fitz_json(__j: &serde_json::Value) -> Result<Self, String> {\n");
        writeln!(
            &mut self.output,
            "        let __obj = __j.as_object().ok_or_else(|| format!(\"body para '{}' debe ser un objeto JSON\"))?;",
            name
        )
        .unwrap();
        // Validar extras
        self.emit("        let __allowed = [");
        for (i, f) in sig.fields.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            write!(&mut self.output, "\"{}\"", f.name).unwrap();
        }
        self.emit("];\n");
        self.emit("        for __k in __obj.keys() {\n");
        self.emit("            if !__allowed.contains(&__k.as_str()) {\n");
        writeln!(
            &mut self.output,
            "                return Err(format!(\"body para '{}': campo no declarado: {{}}\", __k));",
            name
        )
        .unwrap();
        self.emit("            }\n");
        self.emit("        }\n");
        // Cada field: presente en JSON → from_fitz_json; ausente con
        // default → emitir el default; ausente nullable → None; ausente
        // sin default ni nullable → error.
        for f in &sig.fields {
            let rust_ty = rust_type_for(&f.type_, self.env)?;
            writeln!(&mut self.output, "        let {}: {} = match __obj.get(\"{}\") {{", f.name, rust_ty, f.name).unwrap();
            writeln!(
                &mut self.output,
                "            Some(__v) => <{} as __FromFitzJson>::__from_fitz_json(__v)?,",
                rust_ty
            )
            .unwrap();
            // Default o nullable
            if let Some(default_expr) = &f.default {
                let (code, ty) = self.gen_expr(default_expr)?;
                let coerced = coerce(&code, &ty, &f.type_);
                writeln!(&mut self.output, "            None => {},", coerced).unwrap();
            } else if matches!(f.type_, Type::Nullable(_)) {
                self.emit("            None => None,\n");
            } else {
                writeln!(
                    &mut self.output,
                    "            None => return Err(format!(\"body para '{}': falta el campo `{}`\")),",
                    name, f.name
                )
                .unwrap();
            }
            self.emit("        };\n");
        }
        // Construir el struct
        writeln!(&mut self.output, "        Ok({} {{", data_name).unwrap();
        for f in &sig.fields {
            writeln!(&mut self.output, "            {},", f.name).unwrap();
        }
        self.emit("        })\n");
        self.emit("    }\n}\n\n");

        Ok(())
    }

    /// MW.3: recolecta los `@middleware(...)` apilados sobre un handler
    /// y los clasifica en (a) user-fn middlewares (chain gate-only que
    /// el wrapper invocará en orden), y (b) un CorsConfig single-slot
    /// (precomputado build-time desde el arg literal de `cors({...})`).
    ///
    /// Distinción por shape de la expresión del arg:
    ///   - `Expr::Ident(n, _)` que está en `fn_sigs` → user-fn middleware.
    ///   - `Expr::Call { callee: Expr::Ident("cors", _), args, .. }` →
    ///     CORS. Args admitidos: 0 o 1 `Expr::Map` literal con keys
    ///     conocidas (`allow_origin/allow_methods/allow_headers/max_age`).
    ///   - Otra cosa → error de codegen claro. Factories user-defined
    ///     que retornen Function quedan como deuda (post-MW.3).
    ///
    /// Dos `cors(...)` sobre la misma ruta → error "uno por ruta".
    /// El orden de la chain refleja el orden de los decoradores
    /// (top-down igual que MW.1).
    fn collect_route_middlewares(
        &self,
        fn_name: &str,
        decorators: &[Decorator],
    ) -> Result<(Vec<String>, Option<BuildCorsConfig>), FitzError> {
        let mut user_fns: Vec<String> = Vec::new();
        let mut cors: Option<BuildCorsConfig> = None;
        for deco in decorators {
            if deco.name != "middleware" {
                continue;
            }
            if !deco.kwargs.is_empty() {
                return Err(self.err(format!(
                    "@middleware sobre fn `{}`: no admite kwargs",
                    fn_name
                )));
            }
            if deco.args.len() != 1 {
                return Err(self.err(format!(
                    "@middleware sobre fn `{}`: espera exactamente un argumento",
                    fn_name
                )));
            }
            let arg = &deco.args[0];
            match arg {
                // user-fn middleware
                Expr::Ident(n, _) => {
                    if !self.fn_sigs.contains_key(n.as_str()) {
                        return Err(self.err(format!(
                            "@middleware(`{}`) sobre fn `{}`: la fn no está \
                             definida en este programa (build-time check)",
                            n, fn_name
                        )));
                    }
                    user_fns.push(n.clone());
                }
                // cors(...) build-time
                Expr::Call { callee, args, .. } => {
                    let is_cors = matches!(callee.as_ref(), Expr::Ident(n, _) if n == "cors");
                    if !is_cors {
                        return Err(self.err(format!(
                            "@middleware(...) sobre fn `{}`: solo se admite \
                             un Ident o `cors(...)` como argumento en `fitz build` (MW.3)",
                            fn_name
                        )));
                    }
                    if cors.is_some() {
                        return Err(self.err(format!(
                            "@middleware sobre fn `{}`: el handler ya tiene un \
                             `cors(...)` aplicado, solo se admite uno por ruta",
                            fn_name
                        )));
                    }
                    let cfg = parse_build_cors_args(args.as_slice())?;
                    cors = Some(cfg);
                }
                _ => {
                    return Err(self.err(format!(
                        "@middleware sobre fn `{}`: argumento no soportado \
                         en `fitz build` (admitido: Ident o `cors(...)`)",
                        fn_name
                    )));
                }
            }
        }
        Ok((user_fns, cors))
    }

    /// Genera el wrapper `async fn __handler_<name>(...)` para un
    /// handler decorado con `@get/@post/@put/@delete`. Extrae path
    /// params + body (si corresponde), llama a la fn original, y
    /// convierte el resultado en una `axum::response::Response`.
    fn gen_http_handler_wrapper(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let sig = self.resolve_handler_signature(stmt)?;
        self.emit_axum_extractors(&sig)?;
        self.emit_middleware_chain(&sig);
        self.emit_param_coercions(&sig)?;
        self.emit_handler_dispatch_and_response(&sig);
        self.emit_cors_helpers(&sig);
        Ok(())
    }

    /// Resuelve toda la info del handler que `gen_http_handler_wrapper`
    /// necesita para emitir el wrapper async. Pasos: ubicar el decorator
    /// HTTP, parsear el path template, recolectar middlewares (chain +
    /// CORS), resolver tipos de cada param, validar query params del
    /// template, validar header params, categorizar params en
    /// path/query/header/body, resolver el return type.
    fn resolve_handler_signature(&self, stmt: &Stmt) -> Result<HandlerSig, FitzError> {
        let fn_span = stmt.span();
        let Stmt::FnDef {
            name,
            params,
            decorators,
            return_type,
            is_async,
            ..
        } = stmt
        else {
            return Err(self.err("se esperaba Stmt::FnDef en resolve_handler_signature"));
        };

        // Encontrar el decorator HTTP de esta fn (puede haber otros, los
        // ignoramos — el filtrado lo hizo `generate_main_rs`).
        // Defensivo: `generate_main_rs` solo nos llama si la fn tiene
        // decorator HTTP. Si llegamos acá sin uno, es un bug del codegen.
        let http_deco = decorators
            .iter()
            .find(|d| {
                matches!(d.name.as_str(), "get" | "post" | "put" | "delete")
            })
            .ok_or_else(|| self.err(format!("fn `{}`: sin decorator HTTP", name)))?;
        let path_arg = http_deco.args.first().ok_or_else(|| {
            self.err_at(fn_span, format!(
                "fn `{}`: @{} requiere un path como primer arg",
                name, http_deco.name
            ))
        })?;
        let (path, query_template_params) = parse_http_path(path_arg)?;
        let template_params = extract_path_template_names(&path);
        let http_method = match http_deco.name.as_str() {
            "get" => "GET",
            "post" => "POST",
            "put" => "PUT",
            "delete" => "DELETE",
            other => {
                return Err(self.err_at(
                    fn_span,
                    format!("método HTTP no soportado en wrapper: {}", other),
                ));
            }
        };

        // MW.3: recolectar middlewares apilados sobre esta ruta. Separamos
        // user-fn middlewares (chain a invocar en orden) y CorsConfig
        // (slot dedicado). `has_middleware` decide si el wrapper extrae
        // HeaderMap incluso cuando no hay headers declarados (necesario
        // para construir el Request del middleware).
        let (mw_user_fns, mw_cors) = self.collect_route_middlewares(name, decorators)?;
        let has_middleware = !mw_user_fns.is_empty();
        let has_cors = mw_cors.is_some();

        // Resolver tipos resueltos de cada param.
        let mut resolved_params: Vec<(String, Type)> = Vec::with_capacity(params.len());
        for p in params {
            let te = p.type_.as_ref().ok_or_else(|| {
                self.err_at(fn_span, format!(
                    "fn `{}`: parámetro `{}` necesita anotación de tipo",
                    name, p.name
                ))
            })?;
            let t = resolve_type_expr(te, self.env).map_err(|e| {
                self.err_at(fn_span, format!("fn `{}`: parámetro `{}`: {}", name, p.name, e.message))
            })?;
            resolved_params.push((p.name.clone(), t));
        }

        // Validar que cada query_param del template tenga un param Fitz
        // correspondiente con el mismo nombre. Espejo del check del
        // evaluator.
        for qname in &query_template_params {
            if !resolved_params.iter().any(|(n, _)| n == qname) {
                return Err(self.err_at(fn_span, format!(
                    "fn `{}`: el query param `{}` está en el path pero el handler no \
                     tiene un parámetro con ese nombre",
                    name, qname
                )));
            }
        }

        // Headers (Fase 7.6): recolectar desde decorators `@header`.
        // Reusamos la lógica de openapi.rs para mantener el mapping
        // consistente con el schema generado. Validamos que cada
        // param header sea Str o Str?; otros tipos → error de codegen.
        let header_specs = crate::openapi::headers_from_decorators(decorators, params);
        for (http_name, fitz_param, _is_nullable) in &header_specs {
            let p = resolved_params
                .iter()
                .find(|(n, _)| n == fitz_param)
                .map(|(_, t)| t);
            match p {
                Some(Type::Str) | Some(Type::Nullable(_)) => {}
                Some(other) => {
                    return Err(self.err_at(fn_span, format!(
                        "fn `{}`: @header(name=\"{}\") espera un param `Str` o `Str?`, \
                         pero `{}` está declarado como `{}`",
                        name,
                        http_name,
                        fitz_param,
                        type_name(other),
                    )));
                }
                None => {} // ya cazado por evaluator/checker
            }
        }

        // Categorizar: cada param es path / query / header / body.
        let mut path_params: Vec<(String, Type)> = Vec::new();
        let mut query_params: Vec<(String, Type)> = Vec::new();
        let mut header_params: Vec<(String, String, bool)> = Vec::new();
        let mut body_param: Option<(String, Type)> = None;
        for (n, t) in &resolved_params {
            if template_params.iter().any(|tp| tp == n) {
                path_params.push((n.clone(), t.clone()));
            } else if query_template_params.iter().any(|q| q == n) {
                query_params.push((n.clone(), t.clone()));
            } else if let Some((http_name, _, is_nullable)) =
                header_specs.iter().find(|(_, fp, _)| fp == n)
            {
                header_params.push((http_name.clone(), n.clone(), *is_nullable));
            } else if body_param.is_some() {
                return Err(self.err_at(fn_span, format!(
                    "fn `{}`: solo se admite un body param por handler",
                    name
                )));
            } else {
                body_param = Some((n.clone(), t.clone()));
            }
        }

        let resolved_ret = match return_type {
            Some(te) => resolve_type_expr(te, self.env).map_err(|e| {
                self.err_at(fn_span, format!("fn `{}`: return type: {}", name, e.message))
            })?,
            None => Type::Null,
        };
        let returns_result = matches!(resolved_ret, Type::Result(_));

        Ok(HandlerSig {
            name: name.clone(),
            is_async: *is_async,
            http_method,
            path,
            path_params,
            query_params,
            header_params,
            body_param,
            resolved_params,
            returns_result,
            mw_user_fns,
            mw_cors,
            has_middleware,
            has_cors,
        })
    }

    /// Firma del wrapper: `async fn __handler_<name>(...) -> Response`.
    /// Los extractores axum se emiten en el orden declarado por el
    /// usuario: path tuple primero, body al final. HeaderMap se extrae
    /// solo cuando hace falta (headers declarados, middlewares, o CORS).
    fn emit_axum_extractors(&mut self, sig: &HandlerSig) -> Result<(), FitzError> {
        writeln!(&mut self.output, "async fn __handler_{}(", sig.name).unwrap();
        if !sig.path_params.is_empty() {
            if sig.path_params.len() == 1 {
                let (pn, pt) = &sig.path_params[0];
                writeln!(
                    &mut self.output,
                    "    axum::extract::Path({}): axum::extract::Path<{}>,",
                    pn,
                    rust_type_for(pt, self.env)?,
                )
                .unwrap();
            } else {
                // Path<(T1, T2, ...)> con nombres tupleados.
                let names: Vec<String> = sig.path_params.iter().map(|(n, _)| n.clone()).collect();
                let types: Vec<String> = sig
                    .path_params
                    .iter()
                    .map(|(_, t)| rust_type_for(t, self.env))
                    .collect::<Result<_, _>>()?;
                writeln!(
                    &mut self.output,
                    "    axum::extract::Path(({})): axum::extract::Path<({})>,",
                    names.join(", "),
                    types.join(", "),
                )
                .unwrap();
            }
        }
        if !sig.query_params.is_empty() {
            self.emit(
                "    axum::extract::Query(__qmap): axum::extract::Query<std::collections::HashMap<String, String>>,\n",
            );
        }
        // HeaderMap: hace falta cuando el handler declara `@header(...)`
        // (Fase 7.6), hay middlewares (MW.3) que reciben Request, o hay
        // CORS (Q.3) que necesita leer el `Origin` del request para
        // resolver los headers `Access-Control-Allow-*`. Sin ninguno,
        // axum NO extrae el HeaderMap (zero-overhead en handlers simples).
        if !sig.header_params.is_empty() || sig.has_middleware || sig.has_cors {
            self.emit("    __hmap: axum::http::HeaderMap,\n");
        }
        if let Some((bn, _bt)) = &sig.body_param {
            writeln!(
                &mut self.output,
                "    axum::Json({}_raw): axum::Json<serde_json::Value>,",
                bn,
            )
            .unwrap();
        }
        self.emit(") -> axum::response::Response {\n");
        self.emit("    use axum::response::IntoResponse;\n");
        Ok(())
    }

    /// MW.3: si hay middlewares, construir el Request y ejecutar la
    /// chain ANTES de parsear body o coercionar params. Si algún
    /// middleware corta, devolvemos la response (con headers CORS
    /// si la ruta declara `cors(...)`). Sin middlewares, no emite nada.
    fn emit_middleware_chain(&mut self, sig: &HandlerSig) {
        if !sig.has_middleware {
            return;
        }
        // Reconstruir el path con los path params sustituidos.
        // Usamos `format!` con los bindings que axum::extract::Path
        // ya bindeó arriba. Si la ruta no tiene path params, el
        // template está libre de `{...}`.
        let mut fmt_template = String::new();
        let mut fmt_args: Vec<String> = Vec::new();
        for ch in sig.path.chars() {
            if ch == '{' {
                // tomar hasta '}'.
                fmt_template.push_str("{}");
            } else if ch == '}' {
                // skip
            } else {
                fmt_template.push(ch);
            }
        }
        for (pn, _) in &sig.path_params {
            fmt_args.push(pn.clone());
        }
        self.emit("    let __req_headers_vec: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> = std::sync::Arc::new(std::sync::Mutex::new(\n");
        self.emit("        __hmap.iter()\n");
        self.emit("            .filter_map(|(n, v)| v.to_str().ok().map(|s| (n.as_str().to_lowercase(), s.to_string())))\n");
        self.emit("            .collect()\n");
        self.emit("    ));\n");
        if fmt_args.is_empty() {
            writeln!(
                &mut self.output,
                "    let __req_path = String::from(\"{}\");",
                sig.path,
            )
            .unwrap();
        } else {
            writeln!(
                &mut self.output,
                "    let __req_path = format!(\"{}\", {});",
                fmt_template,
                fmt_args.join(", "),
            )
            .unwrap();
        }
        self.emit("    let __req: Request = std::sync::Arc::new(std::sync::Mutex::new(RequestData {\n");
        writeln!(
            &mut self.output,
            "        method: \"{}\".to_string(),",
            sig.http_method,
        )
        .unwrap();
        self.emit("        path: __req_path,\n");
        self.emit("        headers: __req_headers_vec,\n");
        self.emit("    }));\n");
        // Chain de middlewares.
        for mw_name in &sig.mw_user_fns {
            writeln!(
                &mut self.output,
                "    if let Some(__resp) = {}(__req.clone()) {{",
                mw_name,
            )
            .unwrap();
            self.emit("        return __apply_cors_and_respond(\n");
            self.emit("            (\n");
            self.emit("                axum::http::StatusCode::from_u16(__resp.status)\n");
            self.emit("                    .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),\n");
            self.emit("                axum::Json(__resp.body),\n");
            self.emit("            ).into_response(),\n");
            if sig.has_cors {
                // Q.3: resolver headers CORS contra el Origin del
                // request actual. `__cors_resolve_<NAME>(origin)`
                // devuelve un Vec<(&'static str, String)>.
                writeln!(
                    &mut self.output,
                    "            Some({}(__hmap.get(\"origin\").and_then(|v| v.to_str().ok()))),",
                    cors_resolve_fn_name(&sig.name),
                )
                .unwrap();
            } else {
                self.emit("            None,\n");
            }
            self.emit("        );\n");
            self.emit("    }\n");
        }
    }

    /// Pre-call setup: emite las coerciones de query params, lookup de
    /// headers desde HeaderMap, y deserialización del body. Cada bloque
    /// es un "setup pre-llamada" que solo se emite si el handler lo
    /// declara.
    fn emit_param_coercions(&mut self, sig: &HandlerSig) -> Result<(), FitzError> {
        // Query params: para cada uno emitir el binding con coerción
        // desde el HashMap. Si el tipo es nullable (`Int?`), missing →
        // None; si es obligatorio, missing → 400. Tipos soportados en
        // query: Int, Float, Str, Bool (los primitivos que coerciona
        // `coerce_path_param` del intérprete).
        for (qn, qt) in &sig.query_params {
            emit_query_param_coerce(&mut self.output, qn, qt, self.env)?;
        }

        // Headers (Fase 7.6): lookup case-insensitive contra el
        // HeaderMap. Nullable → Option<String>; obligatorio falta → 400.
        for (http_name, fitz_name, is_nullable) in &sig.header_params {
            // Generamos un binding `let <fitz_name>: <ty> = ...`.
            // Para nullable: Option<String>. Para obligatorio: String
            // (con early return 400 si falta).
            let lower = http_name.to_lowercase();
            if *is_nullable {
                writeln!(
                    &mut self.output,
                    "    let {}: Option<String> = __hmap.get(\"{}\").and_then(|v| v.to_str().ok().map(|s| s.to_string()));",
                    fitz_name, lower,
                )
                .unwrap();
            } else {
                writeln!(
                    &mut self.output,
                    "    let {}: String = match __hmap.get(\"{}\").and_then(|v| v.to_str().ok()) {{",
                    fitz_name, lower,
                )
                .unwrap();
                self.emit("        Some(v) => v.to_string(),\n");
                self.emit("        None => return (\n");
                self.emit("            axum::http::StatusCode::BAD_REQUEST,\n");
                writeln!(
                    &mut self.output,
                    "            axum::Json(serde_json::json!({{\"error\": \"header '{}': falta — es obligatorio\"}})),",
                    http_name,
                )
                .unwrap();
                self.emit("        ).into_response(),\n");
                self.emit("    };\n");
            }
        }

        // Si hay body con tipo declarado, deserializar primero. El
        // `__from_fitz_json` genérico para `Arc<Mutex<T>>` ya envuelve
        // el resultado, así que para tipos Nominal el binding queda en
        // la representación correcta (`Foo = Arc<Mutex<FooData>>`).
        if let Some((bn, bt)) = &sig.body_param {
            let rust_ty = rust_type_for(bt, self.env)?;
            writeln!(
                &mut self.output,
                "    let {} = match <{} as __FromFitzJson>::__from_fitz_json(&{}_raw) {{",
                bn, rust_ty, bn
            )
            .unwrap();
            self.emit("        Ok(v) => v,\n");
            self.emit("        Err(e) => return (\n");
            self.emit("            axum::http::StatusCode::BAD_REQUEST,\n");
            self.emit("            axum::Json(serde_json::json!({\"error\": e})),\n");
            self.emit("        ).into_response(),\n");
            self.emit("    };\n");
        }
        Ok(())
    }

    /// Llamada al handler Fitz original y conversión del resultado a
    /// response. Tres caminos: (1) la fn retorna `__FitzResponse`
    /// (status codes custom); (2) la fn retorna `Result<T, String>`:
    /// Ok→200, Err→500; (3) cualquier otro tipo: 200 con el body
    /// serializado. Cierra el cuerpo del wrapper con `}\n\n`.
    fn emit_handler_dispatch_and_response(&mut self, sig: &HandlerSig) {
        // Llamada a la fn original. Si el handler Fitz es `async fn`,
        // su firma Rust (`pub async fn`) devuelve un `Future`; el
        // wrapper await-ea sobre la marcha para obtener el `T` interno
        // y procesarlo igual que un handler sync.
        let call_args: Vec<String> = sig
            .resolved_params
            .iter()
            .map(|(n, _)| n.clone())
            .collect();
        let await_suffix = if sig.is_async { ".await" } else { "" };
        writeln!(
            &mut self.output,
            "    let __result = {}({}){};",
            sig.name,
            call_args.join(", "),
            await_suffix,
        )
        .unwrap();

        // MW.3: si la ruta declara cors, envolvemos el resultado en
        // `__apply_cors_and_respond(...)` para inyectar headers.
        let returns_response = self.http_handlers_returning_response.contains(&sig.name);
        // Q.3: resolver headers CORS contra el Origin del request actual.
        // `__hmap` se extrajo arriba (la condición incluye `has_cors`).
        let cors_arg = if sig.has_cors {
            format!(
                "Some({}(__hmap.get(\"origin\").and_then(|v| v.to_str().ok())))",
                cors_resolve_fn_name(&sig.name)
            )
        } else {
            "None".to_string()
        };
        if returns_response {
            self.emit("    let __resp: __FitzResponse = __result;\n");
            self.emit("    let __built = (\n");
            self.emit("        axum::http::StatusCode::from_u16(__resp.status)\n");
            self.emit("            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),\n");
            self.emit("        axum::Json(__resp.body),\n");
            self.emit("    ).into_response();\n");
            writeln!(
                &mut self.output,
                "    __apply_cors_and_respond(__built, {})",
                cors_arg
            )
            .unwrap();
            self.emit("\n");
        } else if sig.returns_result {
            self.emit("    let __built = match __result {\n");
            self.emit("        Ok(__v) => (\n");
            self.emit("            axum::http::StatusCode::OK,\n");
            self.emit("            axum::Json(__v.__to_fitz_json()),\n");
            self.emit("        ).into_response(),\n");
            self.emit("        Err(__e) => (\n");
            self.emit("            axum::http::StatusCode::INTERNAL_SERVER_ERROR,\n");
            self.emit("            axum::Json(serde_json::json!({\"error\": __e})),\n");
            self.emit("        ).into_response(),\n");
            self.emit("    };\n");
            writeln!(
                &mut self.output,
                "    __apply_cors_and_respond(__built, {})",
                cors_arg
            )
            .unwrap();
            self.emit("\n");
        } else {
            self.emit("    let __built = (\n");
            self.emit("        axum::http::StatusCode::OK,\n");
            self.emit("        axum::Json(__result.__to_fitz_json()),\n");
            self.emit("    ).into_response();\n");
            writeln!(
                &mut self.output,
                "    __apply_cors_and_respond(__built, {})",
                cors_arg
            )
            .unwrap();
            self.emit("\n");
        }
        self.emit("}\n\n");
    }

    /// Q.3: si la ruta declara `cors(...)`, emite la fn
    /// `__cors_resolve_<NAME>(origin: Option<&str>)` (devuelve el Vec
    /// de headers resuelto contra el Origin actual) y el handler
    /// `__preflight_<NAME>` que responde 204 + headers a la request
    /// OPTIONS automática. Sin CORS, no emite nada.
    fn emit_cors_helpers(&mut self, sig: &HandlerSig) {
        let Some(cors) = &sig.mw_cors else { return };
        let resolve_fn = cors_resolve_fn_name(&sig.name);
        let methods = cors.methods_joined();
        let headers_csv = cors.headers_joined();
        let origin = cors.origin_resolved();

        writeln!(
            &mut self.output,
            "fn {}(origin: Option<&str>) -> Vec<(&'static str, String)> {{",
            resolve_fn
        )
        .unwrap();
        self.emit("    let mut __out: Vec<(&'static str, String)> = Vec::with_capacity(4);\n");
        match &origin {
            BuildAllowOrigin::Literal(s) => {
                writeln!(
                    &mut self.output,
                    "    __out.push((\"access-control-allow-origin\", \"{}\".to_string()));",
                    s.replace('\\', "\\\\").replace('"', "\\\"")
                )
                .unwrap();
                // `origin` no se usa en este caso — silenciar warning.
                self.emit("    let _ = origin;\n");
            }
            BuildAllowOrigin::Set(set) => {
                self.emit("    let __set: &[&'static str] = &[");
                for (i, s) in set.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.emit(&format!(
                        "\"{}\"",
                        s.replace('\\', "\\\\").replace('"', "\\\"")
                    ));
                }
                self.emit("];\n");
                self.emit("    if let Some(__req) = origin {\n");
                self.emit("        if __set.iter().any(|s| *s == __req) {\n");
                self.emit("            __out.push((\"access-control-allow-origin\", __req.to_string()));\n");
                self.emit("        }\n");
                self.emit("    }\n");
            }
        }
        writeln!(
            &mut self.output,
            "    __out.push((\"access-control-allow-methods\", \"{}\".to_string()));",
            methods.replace('\\', "\\\\").replace('"', "\\\"")
        )
        .unwrap();
        writeln!(
            &mut self.output,
            "    __out.push((\"access-control-allow-headers\", \"{}\".to_string()));",
            headers_csv.replace('\\', "\\\\").replace('"', "\\\"")
        )
        .unwrap();
        if let Some(age) = cors.max_age {
            writeln!(
                &mut self.output,
                "    __out.push((\"access-control-max-age\", \"{}\".to_string()));",
                age
            )
            .unwrap();
        }
        self.emit("    __out\n");
        self.emit("}\n\n");

        // Handler preflight: lee el Origin del request OPTIONS y
        // emite 204 con los headers resueltos.
        writeln!(
            &mut self.output,
            "async fn __preflight_{}(headers: axum::http::HeaderMap) -> axum::response::Response {{",
            sig.name
        )
        .unwrap();
        self.emit("    use axum::response::IntoResponse;\n");
        self.emit("    let __origin = headers.get(\"origin\").and_then(|v| v.to_str().ok()).map(|s| s.to_string());\n");
        writeln!(
            &mut self.output,
            "    let __headers = {}(__origin.as_deref());",
            resolve_fn
        )
        .unwrap();
        self.emit("    let mut resp = axum::http::StatusCode::NO_CONTENT.into_response();\n");
        self.emit("    for (n, v) in __headers {\n");
        self.emit("        let parsed_n = axum::http::HeaderName::try_from(n);\n");
        self.emit("        let parsed_v = axum::http::HeaderValue::try_from(v);\n");
        self.emit("        if let (Ok(n), Ok(v)) = (parsed_n, parsed_v) {\n");
        self.emit("            resp.headers_mut().insert(n, v);\n");
        self.emit("        }\n");
        self.emit("    }\n");
        self.emit("    resp\n");
        self.emit("}\n\n");
    }

    /// Genera el `#[tokio::main] async fn main()` que construye el
    /// `Router` axum con cada handler registrado, parsea la addr de
    /// `@server(...)` (o usa defaults), e invoca `axum::serve`.
    ///
    /// F11 + F17.4b: si el programa tiene `Stmt::Assign` top-level
    /// (state compartido), emitimos un `static LazyLock<Arc<Mutex<T>>>`
    /// por cada uno antes del `fn main()`. Cada handler (y cada fn
    /// helper) materializa el state al inicio de su body via
    /// `(*X).clone()` — un Arc clone que preserva aliasing entre
    /// requests concurrentes. F11 originalmente usaba `thread_local!`
    /// más tokio `current_thread`; F17.4b cambia a LazyLock + tokio
    /// multi-thread para destrabar paralelismo HTTP real.
    ///
    /// El resto de los `main_stmts` (que no sean `Stmt::Assign`
    /// top-level usados como state) se emiten dentro del `fn main()`
    /// antes del Router — útil para `print(...)` de inicio o setup
    /// auxiliar.
    fn gen_http_main(
        &mut self,
        http_fns: &[&Stmt],
        server_config: &Option<ServerConfigArgs>,
        main_stmts: &[&Stmt],
        program: &Program,
    ) -> Result<(), FitzError> {
        // F11 + F17.4b: emitir un `static LazyLock<Arc<Mutex<T>>>` por
        // cada state var detectado. El init es la expresión RHS del
        // `Stmt::Assign`, evaluada lazy en el primer acceso. Como
        // todas las repr internas (`Arc<Mutex<Vec<...>>>`, etc.) ya son
        // Send + Sync post-F17.4b, el LazyLock se comparte directo
        // entre workers tokio. El `(*X).clone()` que materializa los
        // handlers solo clona el Arc (~ns), no el contenido — alias
        // preservado, mutaciones visibles entre requests.
        if !self.state_var_types.is_empty() {
            for s in main_stmts {
                if let Stmt::Assign {
                    target: AssignTarget::Ident(name),
                    value,
                    ..
                } = s
                {
                    if let Some(ty) = self.state_var_types.get(name).cloned() {
                        let static_name = state_var_static_name(name);
                        let rust_ty = rust_type_for(&ty, self.env)?;
                        let (init_code, init_ty) = self.gen_expr(value)?;
                        let coerced = coerce(&init_code, &init_ty, &ty);
                        writeln!(
                            &mut self.output,
                            "static {}: std::sync::LazyLock<{}> = \
                             std::sync::LazyLock::new(|| {});",
                            static_name, rust_ty, coerced
                        )
                        .unwrap();
                        self.emit("\n");
                    }
                }
            }
        }

        // Fase 7.5: auto-register de /openapi.json y /docs en el
        // binario nativo. Decisión por (enable_docs × paths declarados
        // por el usuario): si el usuario declaró un handler con
        // /openapi.json o /docs, su versión gana — mismo comportamiento
        // que el runtime (7.2/7.3).
        let cfg = server_config.clone().unwrap_or_default();
        let mut user_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
        for stmt in http_fns {
            let Stmt::FnDef { decorators, .. } = stmt else { continue };
            for d in decorators {
                if !matches!(d.name.as_str(), "get" | "post" | "put" | "delete") {
                    continue;
                }
                let Some(path_arg) = d.args.first() else { continue };
                let (path, _q) = parse_http_path(path_arg)?;
                user_paths.insert(path);
            }
        }
        let auto_openapi = cfg.enable_docs && !user_paths.contains("/openapi.json");
        let auto_docs = cfg.enable_docs && !user_paths.contains("/docs");

        // Si alguna ruta auto se va a emitir, pre-computamos el schema
        // OpenAPI desde el AST y lo embebemos como `&'static str` JSON.
        // El HTML de Scalar viene de `crate::openapi::SCALAR_HTML` y
        // se embebe textualmente también.
        if auto_openapi || auto_docs {
            // Schema: armamos las rutas desde AST y serializamos a JSON.
            let routes = crate::openapi::pseudo_routes_from_ast(program)?;
            // Q.2: `@server(api_version=...)` override del info.version.
            let schema = crate::openapi::generate_openapi_with_version(
                &routes,
                program,
                cfg.api_version.as_deref(),
            );
            let schema_str = serde_json::to_string(&schema).map_err(|e| {
                FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0,
                    0,
                    format!("error serializando schema OpenAPI: {}", e),
                )
            })?;
            if auto_openapi {
                self.emit("/// Schema OpenAPI 3.1 generado por `fitz build` (Fase 7.5).\n");
                self.emit("static __FITZ_OPENAPI_SCHEMA: &str = r###\"");
                self.emit(&schema_str);
                self.emit("\"###;\n\n");
                self.emit("async fn __serve_openapi_json() -> axum::response::Response {\n");
                self.emit("    use axum::response::IntoResponse;\n");
                self.emit("    (\n");
                self.emit("        [(axum::http::header::CONTENT_TYPE, \"application/json\")],\n");
                self.emit("        __FITZ_OPENAPI_SCHEMA,\n");
                self.emit("    ).into_response()\n");
                self.emit("}\n\n");
            }
            if auto_docs {
                self.emit("/// HTML de la UI Scalar embebido (Fase 7.5).\n");
                self.emit("static __FITZ_SCALAR_HTML: &str = r###\"");
                self.emit(crate::openapi::SCALAR_HTML);
                self.emit("\"###;\n\n");
                self.emit(
                    "async fn __serve_docs() -> axum::response::Html<&'static str> {\n",
                );
                self.emit("    axum::response::Html(__FITZ_SCALAR_HTML)\n");
                self.emit("}\n\n");
            }
        }

        // F17.4b: tokio default (multi-thread). N workers según cores,
        // paralelismo HTTP real entre requests sobre los handlers.
        self.emit("#[tokio::main]\nasync fn main() {\n");
        self.indent += 1;
        self.push_scope();
        // Emitimos los main_stmts que NO son state (no están en
        // state_var_types). Los que sí son state ya viven en
        // thread_local — re-emitirlos como locales acá sería redundante.
        for s in main_stmts {
            if let Stmt::Assign {
                target: AssignTarget::Ident(name),
                ..
            } = s
            {
                if self.state_var_types.contains_key(name) {
                    continue;
                }
            }
            self.gen_stmt(s)?;
        }

        // Router con cada ruta.
        self.emit_indent();
        self.emit("let __app = axum::Router::new()\n");
        for stmt in http_fns {
            let Stmt::FnDef { name, decorators, .. } = stmt else { continue };
            // MW.3: detectar si esta ruta tiene cors → registrar
            // OPTIONS al mismo path con el preflight handler.
            let route_has_cors = decorators
                .iter()
                .filter(|d| d.name == "middleware")
                .any(|d| {
                    d.args.first().is_some_and(|a| {
                        matches!(
                            a,
                            Expr::Call { callee, .. }
                                if matches!(callee.as_ref(), Expr::Ident(n, _) if n == "cors")
                        )
                    })
                });
            for d in decorators {
                let method = match d.name.as_str() {
                    "get" => "get",
                    "post" => "post",
                    "put" => "put",
                    "delete" => "delete",
                    _ => continue,
                };
                let path_arg = match d.args.first() {
                    Some(p) => p,
                    None => continue,
                };
                let (path, _q) = parse_http_path(path_arg)?;
                self.emit_indent();
                if route_has_cors {
                    writeln!(
                        &mut self.output,
                        "    .route(\"{}\", axum::routing::{}(__handler_{}).options(__preflight_{}))",
                        path, method, name, name,
                    )
                    .unwrap();
                } else {
                    writeln!(
                        &mut self.output,
                        "    .route(\"{}\", axum::routing::{}(__handler_{}))",
                        path, method, name,
                    )
                    .unwrap();
                }
            }
        }
        // Fase 7.5: rutas auto-registradas. Mismo orden que el runtime
        // (7.2/7.3): /openapi.json primero, /docs después.
        if auto_openapi {
            self.emit_indent();
            self.emit("    .route(\"/openapi.json\", axum::routing::get(__serve_openapi_json))\n");
        }
        if auto_docs {
            self.emit_indent();
            self.emit("    .route(\"/docs\", axum::routing::get(__serve_docs))\n");
        }
        self.emit_indent();
        self.emit(";\n");

        // Addr config.
        self.emit_indent();
        writeln!(
            &mut self.output,
            "let __addr: std::net::SocketAddr = \"{}:{}\".parse().expect(\"@server: addr inválida\");",
            cfg.host, cfg.port,
        )
        .unwrap();
        self.emit_indent();
        writeln!(
            &mut self.output,
            "println!(\"Fitz HTTP escuchando en http://{}:{}\");",
            cfg.host, cfg.port,
        )
        .unwrap();
        self.emit_indent();
        self.emit("let __listener = tokio::net::TcpListener::bind(__addr).await.expect(\"bind\");\n");
        self.emit_indent();
        self.emit("axum::serve(__listener, __app).await.expect(\"axum::serve\");\n");

        self.pop_scope();
        self.indent -= 1;
        self.emit("}\n");
        Ok(())
    }
}

/// Extrae el path template de un decorator HTTP, separando el path
/// "axum" (lo que va al router) del query template (`?key={name}&...`)
/// y devolviendo los nombres de query params en orden. Delega a
/// `crate::http::parse_path_template` para mantener la lógica
/// compartida con el intérprete.
fn parse_http_path(expr: &Expr) -> Result<(String, Vec<String>), FitzError> {
    match crate::http::parse_path_template(expr) {
        Ok(t) => Ok((t.path, t.query_params)),
        Err(e) => Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            e.message(),
        )),
    }
}

#[allow(dead_code)]
fn parse_http_path_legacy(expr: &Expr) -> Result<String, FitzError> {
    match expr {
        Expr::Str(s, _) => Ok(s.clone()),
        Expr::StrInterp(parts, _) => {
            use crate::ast::StrPart;
            let mut buf = String::new();
            for part in parts {
                match part {
                    StrPart::Lit(s) => buf.push_str(s),
                    StrPart::Expr(Expr::Ident(name, _)) => {
                        buf.push('{');
                        buf.push_str(name);
                        buf.push('}');
                    }
                    StrPart::Expr(_) => {
                        return Err(FitzError::new(
                            ErrorKind::TypeError,
                            0,
                            0,
                            "el path de un decorator HTTP solo admite literal Str o \
                             interpolación de identificadores: `\"/users/{id}\"`".to_string(),
                        ));
                    }
                }
            }
            Ok(buf)
        }
        _ => Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            "el primer arg de @get/@post/@put/@delete debe ser un Str literal".to_string(),
        )),
    }
}

/// Emite el binding Rust para un query param: lee `__qmap` por su
/// nombre, coerciona al tipo base con el parse correspondiente, y
/// según `nullable` emite `Option<T>` o `T` directo. Errores → 400.
///
/// Tipos soportados en query (los mismos que `coerce_path_param` del
/// intérprete): Int, Float, Str, Bool. Listas/instancias/Result no se
/// soportan — el checker debería rechazarlos pero el codegen aborta
/// con error explícito como defensa.
fn emit_query_param_coerce(
    output: &mut String,
    name: &str,
    ty: &Type,
    env: &TypeEnv,
) -> Result<(), FitzError> {
    use std::fmt::Write;
    // Pelar Nullable para obtener el tipo base.
    let (base_ty, nullable) = match ty {
        Type::Nullable(inner) => ((**inner).clone(), true),
        other => (other.clone(), false),
    };
    // Mapear base_ty a un parser. `Str` no necesita parsear (string es
    // string); los numéricos usan `parse::<T>()`; Bool acepta
    // "true"/"false".
    let (rust_base, parse_expr): (&str, String) = match &base_ty {
        Type::Int => ("i64", format!("__s.parse::<i64>().map_err(|_| format!(\"query param '{}': '{{}}' no es Int\", __s))", name)),
        Type::Float => ("f64", format!("__s.parse::<f64>().map_err(|_| format!(\"query param '{}': '{{}}' no es Float\", __s))", name)),
        Type::Str => ("String", "Ok::<String, String>(__s.clone())".to_string()),
        Type::Bool => ("bool", format!("match __s.as_str() {{ \"true\" => Ok::<bool, String>(true), \"false\" => Ok::<bool, String>(false), _ => Err(format!(\"query param '{}': '{{}}' no es Bool (esperado true/false)\", __s)) }}", name)),
        _ => {
            return Err(FitzError::new(
                ErrorKind::TypeError,
                0,
                0,
                format!(
                    "query param `{}`: tipo `{}` no soportado en codegen — solo Int/Float/Str/Bool (opcionalmente nullable)",
                    name,
                    display_type(&base_ty, env)
                ),
            ));
        }
    };

    if nullable {
        // Opcional: missing → None, presente y coerce OK → Some(v),
        // presente con coerce err → 400.
        writeln!(output, "    let {name}: Option<{rust_base}> = match __qmap.get(\"{name}\") {{").unwrap();
        writeln!(output, "        Some(__s) => {{").unwrap();
        writeln!(output, "            let __r: Result<{rust_base}, String> = {parse_expr};").unwrap();
        writeln!(output, "            match __r {{").unwrap();
        writeln!(output, "                Ok(__v) => Some(__v),").unwrap();
        writeln!(output, "                Err(__e) => return (").unwrap();
        writeln!(output, "                    axum::http::StatusCode::BAD_REQUEST,").unwrap();
        writeln!(output, "                    axum::Json(serde_json::json!({{\"error\": __e}})),").unwrap();
        writeln!(output, "                ).into_response(),").unwrap();
        writeln!(output, "            }}").unwrap();
        writeln!(output, "        }}").unwrap();
        writeln!(output, "        None => None,").unwrap();
        writeln!(output, "    }};").unwrap();
    } else {
        // Obligatorio: missing → 400; presente y coerce OK → v;
        // presente con coerce err → 400.
        writeln!(output, "    let {name}: {rust_base} = match __qmap.get(\"{name}\") {{").unwrap();
        writeln!(output, "        Some(__s) => {{").unwrap();
        writeln!(output, "            let __r: Result<{rust_base}, String> = {parse_expr};").unwrap();
        writeln!(output, "            match __r {{").unwrap();
        writeln!(output, "                Ok(__v) => __v,").unwrap();
        writeln!(output, "                Err(__e) => return (").unwrap();
        writeln!(output, "                    axum::http::StatusCode::BAD_REQUEST,").unwrap();
        writeln!(output, "                    axum::Json(serde_json::json!({{\"error\": __e}})),").unwrap();
        writeln!(output, "                ).into_response(),").unwrap();
        writeln!(output, "            }}").unwrap();
        writeln!(output, "        }}").unwrap();
        writeln!(output, "        None => return (").unwrap();
        writeln!(output, "            axum::http::StatusCode::BAD_REQUEST,").unwrap();
        writeln!(output, "            axum::Json(serde_json::json!({{\"error\": \"query param '{name}': falta — es obligatorio\"}})),").unwrap();
        writeln!(output, "        ).into_response(),").unwrap();
        writeln!(output, "    }};").unwrap();
    }
    Ok(())
}

/// Extrae los nombres de path params de un template axum: `/users/{id}`
/// → `["id"]`; `/users/{id}/posts/{slug}` → `["id", "slug"]`. Acepta
/// `{nombre}` literal (axum 0.8 sintaxis).
fn extract_path_template_names(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            while let Some(&n) = chars.peek() {
                if n == '}' {
                    chars.next();
                    break;
                }
                name.push(n);
                chars.next();
            }
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    out
}

/// Mini-fase MW.3: representación build-time del `cors(...)` aplicado a
/// una ruta vía `@middleware(cors({...}))`. Q.3: `allow_origin` puede ser
/// Literal (valor fijo) o Set (echo del Origin del request si está en la
/// lista). El codegen emite `fn __cors_resolve_<NAME>(origin) -> Vec<(...)>`
/// que el wrapper y el preflight handler llaman por request.
#[derive(Debug, Clone)]
enum BuildAllowOrigin {
    Literal(String),
    Set(Vec<String>),
}

#[derive(Debug, Clone, Default)]
struct BuildCorsConfig {
    /// `None` → default `Literal("*")` cuando se emite el código.
    allow_origin: Option<BuildAllowOrigin>,
    allow_methods: Option<Vec<String>>,
    allow_headers: Option<Vec<String>>,
    max_age: Option<i64>,
}

impl BuildCorsConfig {
    /// Métodos efectivos a emitir. Defaults paralelos a
    /// `crate::http::CorsConfig::permissive_default()`.
    fn methods_joined(&self) -> String {
        self.allow_methods
            .clone()
            .unwrap_or_else(|| {
                vec![
                    "GET".into(),
                    "POST".into(),
                    "PUT".into(),
                    "DELETE".into(),
                    "OPTIONS".into(),
                ]
            })
            .join(", ")
    }

    /// Headers efectivos a emitir.
    fn headers_joined(&self) -> String {
        self.allow_headers
            .clone()
            .unwrap_or_else(|| vec!["content-type".into(), "authorization".into()])
            .join(", ")
    }

    /// `allow_origin` efectivo (con default Literal("*") si no se setó).
    fn origin_resolved(&self) -> BuildAllowOrigin {
        self.allow_origin
            .clone()
            .unwrap_or_else(|| BuildAllowOrigin::Literal("*".to_string()))
    }
}

/// Parsea los args de `cors(...)` en build-time. Soporta:
///       - `cors()` — sin args.
///       - `cors({ "key": value, ... })` — un único Expr::Map literal.
///
/// Validaciones espejo del built-in runtime (en `evaluator.rs`).
fn parse_build_cors_args(args: &[Expr]) -> Result<BuildCorsConfig, FitzError> {
    let err =
        |msg: &str| FitzError::new(ErrorKind::TypeError, 0, 0, msg.to_string());
    if args.len() > 1 {
        return Err(err(
            "`cors` espera 0 o 1 argumento (un Map literal de configuración)",
        ));
    }
    let mut cfg = BuildCorsConfig::default();
    if let Some(arg) = args.first() {
        let pairs = match arg {
            Expr::Map(p, _) => p,
            _ => {
                return Err(err(
                    "`cors`: el argumento debe ser un Map literal en `fitz build` (MW.3)",
                ));
            }
        };
        for (k, v) in pairs {
            let key = match k {
                Expr::Str(s, _) => s.clone(),
                _ => {
                    return Err(err(
                        "`cors`: las keys del Map de configuración deben ser Str literales",
                    ));
                }
            };
            match key.as_str() {
                "allow_origin" => match v {
                    // Q.3: Str → Literal (modo previo, valor fijo).
                    Expr::Str(s, _) => {
                        cfg.allow_origin = Some(BuildAllowOrigin::Literal(s.clone()));
                    }
                    // Q.3: List<Str> → Set, echo del Origin del request.
                    Expr::List(items, _) => {
                        let mut set = Vec::with_capacity(items.len());
                        for it in items {
                            match it {
                                Expr::Str(s, _) => set.push(s.clone()),
                                _ => {
                                    return Err(err(
                                        "`cors`: cada elemento de 'allow_origin' (como lista) debe ser un Str literal",
                                    ));
                                }
                            }
                        }
                        cfg.allow_origin = Some(BuildAllowOrigin::Set(set));
                    }
                    _ => return Err(err(
                        "`cors`: 'allow_origin' debe ser un Str literal o una List<Str> literal",
                    )),
                },
                "allow_methods" => cfg.allow_methods = Some(parse_build_str_list(v, "allow_methods")?),
                "allow_headers" => cfg.allow_headers = Some(parse_build_str_list(v, "allow_headers")?),
                "max_age" => match v {
                    Expr::Int(n, _) => cfg.max_age = Some(*n),
                    _ => return Err(err("`cors`: 'max_age' debe ser un Int literal")),
                },
                other => {
                    return Err(FitzError::new(
                        ErrorKind::TypeError,
                        0,
                        0,
                        format!(
                            "`cors`: key '{}' no reconocida. Soportadas: \
                             allow_origin, allow_methods, allow_headers, max_age.",
                            other
                        ),
                    ));
                }
            }
        }
    }
    Ok(cfg)
}

/// Q.3: nombre de la fn `__cors_resolve_<handler>(origin: Option<&str>)`
/// emitida por el codegen. Cada ruta con `@middleware(cors(...))` la
/// llama por request para obtener el Vec de headers CORS resuelto
/// (incluido el modo `Set` con echo del Origin).
fn cors_resolve_fn_name(handler_name: &str) -> String {
    format!("__cors_resolve_{}", handler_name)
}

fn parse_build_str_list(expr: &Expr, key: &str) -> Result<Vec<String>, FitzError> {
    let err = |msg: String| FitzError::new(ErrorKind::TypeError, 0, 0, msg);
    let items = match expr {
        Expr::List(items, _) => items,
        _ => {
            return Err(err(format!(
                "`cors`: '{}' debe ser una lista literal de Str",
                key
            )));
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Expr::Str(s, _) => out.push(s.clone()),
            _ => {
                return Err(err(format!(
                    "`cors`: cada elemento de '{}' debe ser un Str literal",
                    key
                )));
            }
        }
    }
    Ok(out)
}

/// Preludio HTTP: traits `__ToFitzJson` y `__FromFitzJson` con impls para
/// primitivos y combinadores genéricos (`Option`, `Arc<Mutex<Vec<T>>>`,
/// `Arc<Mutex<Vec<(K, V)>>>`, `Result<T, String>`). Los impls específicos
/// por cada `type Foo` los emite `gen_type_http_impls`.
const HTTP_RUNTIME_PRELUDE: &str = r#"// --- 5b.6: runtime HTTP (serialización JSON) ---

trait __ToFitzJson {
    fn __to_fitz_json(&self) -> serde_json::Value;
}

trait __FromFitzJson: Sized {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String>;
}

/// Status codes custom: response builder con status + body JSON.
/// Producido por `return <status> { ... }` adentro de un handler.
/// El wrapper de cada handler que lo retorna emite `(StatusCode,
/// Json(body))` directo, en vez del path normal Ok/Err.
struct __FitzResponse {
    status: u16,
    body: serde_json::Value,
}

/// Mini-fase MW.3: tipo built-in `Request` que se pasa a cada middleware.
/// Los wrappers de cada handler con `@middleware(...)` lo construyen
/// antes de iterar la chain. La representación matchea cualquier `type`
/// nominal del lenguaje: `Arc<Mutex<RequestData>>` (F17.4b).
#[derive(Clone)]
struct RequestData {
    method: String,
    path: String,
    headers: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}
type Request = std::sync::Arc<std::sync::Mutex<RequestData>>;

impl std::fmt::Display for RequestData {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Request {{ method: \"{}\", path: \"{}\", headers: <map> }}",
            self.method, self.path,
        )
    }
}

impl __ToFitzJson for RequestData {
    fn __to_fitz_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("method".to_string(), serde_json::Value::String(self.method.clone()));
        obj.insert("path".to_string(), serde_json::Value::String(self.path.clone()));
        obj.insert("headers".to_string(), self.headers.__to_fitz_json());
        serde_json::Value::Object(obj)
    }
}

/// `Response` (MW.1) — nominal opaco usable como anotación
/// `-> Response?` en middlewares; el value real lo produce
/// `return <status> { ... }` (= `__FitzResponse` envuelto en
/// `Some(...)`). El struct está vacío por construcción y no se
/// instancia: existe solo para que la anotación tipa.
#[derive(Clone, PartialEq)]
#[allow(dead_code)]
struct ResponseData;
type Response = std::sync::Arc<std::sync::Mutex<ResponseData>>;

impl std::fmt::Display for ResponseData {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "<response>")
    }
}

impl __ToFitzJson for ResponseData {
    fn __to_fitz_json(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

/// MW.3 + Q.3: inyecta los headers CORS resueltos (si los hay) en una
/// `axum::response::Response` ya armada. `cors_headers` viene del helper
/// `__cors_resolve_<NAME>(origin)` emitido por el codegen. Si es `None`,
/// devuelve la response sin cambios. Header inválido → se omite (no
/// panic).
fn __apply_cors_and_respond(
    mut resp: axum::response::Response,
    cors_headers: Option<Vec<(&'static str, String)>>,
) -> axum::response::Response {
    if let Some(hs) = cors_headers {
        for (name, value) in hs {
            let parsed_name = axum::http::HeaderName::try_from(name);
            let parsed_value = axum::http::HeaderValue::try_from(value);
            if let (Ok(n), Ok(v)) = (parsed_name, parsed_value) {
                resp.headers_mut().insert(n, v);
            }
        }
    }
    resp
}

impl __ToFitzJson for i64 {
    fn __to_fitz_json(&self) -> serde_json::Value {
        serde_json::Value::from(*self)
    }
}
impl __ToFitzJson for f64 {
    fn __to_fitz_json(&self) -> serde_json::Value {
        serde_json::Number::from_f64(*self)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    }
}
impl __ToFitzJson for String {
    fn __to_fitz_json(&self) -> serde_json::Value {
        serde_json::Value::String(self.clone())
    }
}
impl __ToFitzJson for bool {
    fn __to_fitz_json(&self) -> serde_json::Value {
        serde_json::Value::Bool(*self)
    }
}
impl __ToFitzJson for () {
    fn __to_fitz_json(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

impl<T: __ToFitzJson> __ToFitzJson for Option<T> {
    fn __to_fitz_json(&self) -> serde_json::Value {
        match self {
            Some(v) => v.__to_fitz_json(),
            None => serde_json::Value::Null,
        }
    }
}

impl<T: __ToFitzJson> __ToFitzJson for std::sync::Arc<std::sync::Mutex<T>> {
    fn __to_fitz_json(&self) -> serde_json::Value {
        self.lock().unwrap().__to_fitz_json()
    }
}

impl<T: __ToFitzJson> __ToFitzJson for Vec<T> {
    fn __to_fitz_json(&self) -> serde_json::Value {
        serde_json::Value::Array(self.iter().map(|v| v.__to_fitz_json()).collect())
    }
}

impl<K: __MapKey, V: __ToFitzJson> __ToFitzJson for Vec<(K, V)> {
    fn __to_fitz_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        for (k, v) in self.iter() {
            obj.insert(k.__as_map_key(), v.__to_fitz_json());
        }
        serde_json::Value::Object(obj)
    }
}

/// Las claves de Map en JSON deben ser strings. Para K = String, usamos
/// la clave tal cual; para otros tipos primitivos (Int/Bool), convertimos
/// con Display. Map con claves nominales/anidadas no es serializable y
/// rustc lo va a flaggear si el codegen lo intenta.
trait __MapKey {
    fn __as_map_key(&self) -> String;
}
impl __MapKey for String {
    fn __as_map_key(&self) -> String { self.clone() }
}
impl __MapKey for i64 {
    fn __as_map_key(&self) -> String { self.to_string() }
}
impl __MapKey for f64 {
    fn __as_map_key(&self) -> String { self.to_string() }
}
impl __MapKey for bool {
    fn __as_map_key(&self) -> String { self.to_string() }
}

impl<T: __ToFitzJson> __ToFitzJson for Result<T, String> {
    /// Result anidado se etiqueta como objeto. El caso principal
    /// (handler que devuelve `Result<T, String>` directo) se maneja
    /// en el wrapper async, NO acá.
    fn __to_fitz_json(&self) -> serde_json::Value {
        match self {
            Ok(v) => serde_json::json!({ "Ok": v.__to_fitz_json() }),
            Err(e) => serde_json::json!({ "Err": e }),
        }
    }
}

// FromFitzJson para primitivos
impl __FromFitzJson for i64 {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        json.as_i64()
            .ok_or_else(|| format!("se esperaba Int, se recibió {}", __json_shape(json)))
    }
}
impl __FromFitzJson for f64 {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        json.as_f64()
            .ok_or_else(|| format!("se esperaba Float, se recibió {}", __json_shape(json)))
    }
}
impl __FromFitzJson for String {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        json.as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| format!("se esperaba Str, se recibió {}", __json_shape(json)))
    }
}
impl __FromFitzJson for bool {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        json.as_bool()
            .ok_or_else(|| format!("se esperaba Bool, se recibió {}", __json_shape(json)))
    }
}

impl<T: __FromFitzJson> __FromFitzJson for Option<T> {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        if json.is_null() {
            Ok(None)
        } else {
            T::__from_fitz_json(json).map(Some)
        }
    }
}

impl<T: __FromFitzJson> __FromFitzJson for std::sync::Arc<std::sync::Mutex<T>> {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        T::__from_fitz_json(json).map(|v| std::sync::Arc::new(std::sync::Mutex::new(v)))
    }
}

fn __json_shape(json: &serde_json::Value) -> &'static str {
    match json {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "Bool",
        serde_json::Value::Number(_) => "Number",
        serde_json::Value::String(_) => "Str",
        serde_json::Value::Array(_) => "Array",
        serde_json::Value::Object(_) => "Object",
    }
}

"#;

/// Si el último stmt del bloque es un `Stmt::Expr(e, Span::ZERO)` que se puede
/// usar como valor (no es un `print(...)`, que solo es stmt), lo
/// devolvemos separado del resto. Caso contrario, el tail va `None`
/// y el bloque queda completo.
fn split_tail_expr(body: &[Stmt]) -> (Vec<&Stmt>, Option<&Expr>) {
    if let Some(Stmt::Expr(e, _)) = body.last() {
        if !is_print_call(e) {
            let stmts: Vec<&Stmt> = body[..body.len() - 1].iter().collect();
            return (stmts, Some(e));
        }
    }
    (body.iter().collect(), None)
}

fn is_print_call(e: &Expr) -> bool {
    matches!(e, Expr::Call { callee, .. }
        if matches!(callee.as_ref(), Expr::Ident(n, _) if n == "print"))
}

/// Recorre el body de un `FnExpr` recolectando capturas: identifiers
/// que no son params, no son locales declarados en el propio body, y
/// existen en algún scope contenedor del codegen. Para cada captura,
/// devolvemos `(name, type)` con el tipo desde el scope contenedor.
/// El orden está deduplicado: cada captura aparece una sola vez.
///
/// La detección es **conservadora**: si una var del scope contenedor
/// aparece referenciada en el body (aunque después esté shadowed por
/// una asignación local), la marcamos como capturada. El binding
/// local va a ganar adentro del closure de todas formas, así que no
/// se rompe; solo terminamos clonando una var que no hacía falta.
fn collect_captures(
    body: &[Stmt],
    params: &std::collections::HashSet<String>,
    ctx: &CodegenCtx,
    out: &mut Vec<(String, Type)>,
) {
    let mut locals: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in body {
        collect_captures_stmt(s, params, &mut locals, ctx, &mut seen, out);
    }
}

fn collect_captures_stmt(
    s: &Stmt,
    params: &std::collections::HashSet<String>,
    locals: &mut std::collections::HashSet<String>,
    ctx: &CodegenCtx,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<(String, Type)>,
) {
    match s {
        Stmt::Assign { target, value, .. } => {
            collect_captures_expr(value, params, locals, ctx, seen, out);
            if let AssignTarget::Ident(name) = target {
                // El binding local se materializa después de evaluar
                // la RHS — el orden importa si el RHS referencia el
                // propio name (shadowing recursivo no soportado, pero
                // por consistencia con el evaluator declaramos
                // después).
                locals.insert(name.clone());
            } else if let AssignTarget::Field { object, .. } = target {
                collect_captures_expr(object, params, locals, ctx, seen, out);
            }
        }
        Stmt::Return(e, _) | Stmt::Expr(e, _) => {
            collect_captures_expr(e, params, locals, ctx, seen, out);
        }
        Stmt::ReturnStatus { status, body, .. } => {
            collect_captures_expr(status, params, locals, ctx, seen, out);
            if let Some(b) = body {
                collect_captures_expr(b, params, locals, ctx, seen, out);
            }
        }
        Stmt::While { condition, body, .. } => {
            collect_captures_expr(condition, params, locals, ctx, seen, out);
            for s in body {
                collect_captures_stmt(s, params, locals, ctx, seen, out);
            }
        }
        Stmt::Loop { body, .. } => {
            for s in body {
                collect_captures_stmt(s, params, locals, ctx, seen, out);
            }
        }
        Stmt::For { var, iter, body, .. } => {
            collect_captures_expr(iter, params, locals, ctx, seen, out);
            locals.insert(var.clone());
            for s in body {
                collect_captures_stmt(s, params, locals, ctx, seen, out);
            }
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::FnDef { .. } | Stmt::TypeDef { .. } | Stmt::Import { .. } | Stmt::FromImport { .. } => {}
        // Fase 9.0.1 (F15): walker estático no-op.
        Stmt::Error(_) => {}
    }
}

fn collect_captures_expr(
    e: &Expr,
    params: &std::collections::HashSet<String>,
    locals: &mut std::collections::HashSet<String>,
    ctx: &CodegenCtx,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<(String, Type)>,
) {
    match e {
        Expr::Ident(name, _) => {
            // Param del propio FnExpr o local declarado dentro:
            // no es captura.
            if params.contains(name) || locals.contains(name) {
                return;
            }
            // Builtins/fn top-level/módulos: no hace falta capturar
            // porque están en scope estático del crate Rust.
            if name == "print" || name == "len" {
                return;
            }
            if ctx.fn_sigs.contains_key(name) || ctx.own_consts.contains_key(name) {
                return;
            }
            if ctx.module_bindings.contains_key(name) {
                return;
            }
            // Tiene que existir en algún scope del codegen para que
            // tenga sentido capturarlo. Si no existe, el error va a
            // saltar en `gen_expr` del body cuando lo emita en serio.
            if let Some(ty) = ctx.lookup_var(name) {
                if seen.insert(name.clone()) {
                    out.push((name.clone(), ty.clone()));
                }
            }
        }
        Expr::Int(_, _) | Expr::Float(_, _) | Expr::Str(_, _) | Expr::Bool(_, _) | Expr::Null(_) => {}
        Expr::StrInterp(parts, _) => {
            for p in parts {
                if let crate::ast::StrPart::Expr(inner) = p {
                    collect_captures_expr(inner, params, locals, ctx, seen, out);
                }
            }
        }
        Expr::BinOp { left, right, .. } => {
            collect_captures_expr(left, params, locals, ctx, seen, out);
            collect_captures_expr(right, params, locals, ctx, seen, out);
        }
        Expr::UnaryOp { operand, .. } => {
            collect_captures_expr(operand, params, locals, ctx, seen, out);
        }
        Expr::Call { callee, args, .. } => {
            collect_captures_expr(callee, params, locals, ctx, seen, out);
            for a in args {
                collect_captures_expr(a, params, locals, ctx, seen, out);
            }
        }
        Expr::FnExpr { params: inner_params, body, .. } => {
            // Closure anidada: sus params introducen un scope nuevo.
            // Para detectar capturas del FnExpr exterior, nos importa
            // todo lo que esa closure interior use desde nuestro
            // contexto — recursivamente, treating sus params como
            // params extra para el cómputo de "no es captura".
            let mut merged: std::collections::HashSet<String> = params.clone();
            for p in inner_params {
                merged.insert(p.name.clone());
            }
            // Las locals de la closure interna son separadas — no las
            // mezclamos con las del outer.
            let mut inner_locals: std::collections::HashSet<String> = std::collections::HashSet::new();
            for s in body {
                collect_captures_stmt(s, &merged, &mut inner_locals, ctx, seen, out);
            }
        }
        Expr::Field { object, .. } => {
            collect_captures_expr(object, params, locals, ctx, seen, out);
        }
        Expr::Index { object, index, .. } => {
            collect_captures_expr(object, params, locals, ctx, seen, out);
            collect_captures_expr(index, params, locals, ctx, seen, out);
        }
        Expr::List(items, _) => {
            for it in items {
                collect_captures_expr(it, params, locals, ctx, seen, out);
            }
        }
        Expr::Map(pairs, _) => {
            for (k, v) in pairs {
                collect_captures_expr(k, params, locals, ctx, seen, out);
                collect_captures_expr(v, params, locals, ctx, seen, out);
            }
        }
        Expr::Range { start, end, .. } => {
            collect_captures_expr(start, params, locals, ctx, seen, out);
            collect_captures_expr(end, params, locals, ctx, seen, out);
        }
        Expr::If { condition, then, else_, .. } => {
            collect_captures_expr(condition, params, locals, ctx, seen, out);
            for s in then {
                collect_captures_stmt(s, params, locals, ctx, seen, out);
            }
            if let Some(els) = else_ {
                for s in els {
                    collect_captures_stmt(s, params, locals, ctx, seen, out);
                }
            }
        }
        Expr::Match { value, arms, .. } => {
            collect_captures_expr(value, params, locals, ctx, seen, out);
            for arm in arms {
                collect_captures_expr(&arm.body, params, locals, ctx, seen, out);
            }
        }
        Expr::StructLit { fields, .. } => {
            for (_, e) in fields {
                collect_captures_expr(e, params, locals, ctx, seen, out);
            }
        }
        Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
            collect_captures_expr(inner, params, locals, ctx, seen, out);
        }
        // Fase 9.0.1 (F15): walker estático no-op.
        Expr::Error(_) => {}
    }
}

/// "Least upper bound" pragmático sobre dos tipos resueltos. Mismo
/// criterio que `types.rs` para FnExpr (5.3.5) y para if-as-expression
/// (5b.2), acotado al subset compilable hoy. Usado además para unificar
/// elementos de listas/mapas literales (5b.3).
///
/// Reglas:
///   - `a == b`               → `a`
///   - `Int` ↔ `Float`        → `Float`
///   - `Null` ↔ `T`           → `T?` (T ≠ Null)
///   - `T?` ↔ `T`             → `T?`
///   - mismo `List<a>`/`List<b>` con `lub(a,b)` recursivo → `List<lub>`
///     (idem `Map`, `Nullable`)
///   - resto                  → `Err(())`
fn lub(a: &Type, b: &Type) -> Result<Type, ()> {
    if a == b {
        return Ok(a.clone());
    }
    match (a, b) {
        (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
        (Type::Null, other) | (other, Type::Null) if !matches!(other, Type::Null) => {
            Ok(Type::Nullable(Box::new(other.clone())))
        }
        (Type::Nullable(inner), other) | (other, Type::Nullable(inner))
            if **inner == *other =>
        {
            Ok(Type::Nullable(inner.clone()))
        }
        (Type::Nullable(a_in), Type::Nullable(b_in)) => {
            lub(a_in, b_in).map(|t| Type::Nullable(Box::new(t)))
        }
        (Type::List(a_in), Type::List(b_in)) => {
            lub(a_in, b_in).map(|t| Type::List(Box::new(t)))
        }
        (Type::Map(ak, av), Type::Map(bk, bv)) => {
            let k = lub(ak, bk)?;
            let v = lub(av, bv)?;
            Ok(Type::Map(Box::new(k), Box::new(v)))
        }
        // Result<a> ↔ Result<b> recursivo. Cubre el caso típico de
        // `match r { Ok(v) => Ok(v + 1), Err(e) => Err(e) }`: ambas
        // ramas son Result<T, String> con el mismo T, lub = T.
        (Type::Result(a_in), Type::Result(b_in)) => {
            lub(a_in, b_in).map(|t| Type::Result(Box::new(t)))
        }
        // Any cede al concreto. Permite que `Err("x")` (Result<Any>)
        // unifique con `Ok(42)` (Result<Int>) → Result<Int>.
        (Type::Any, other) | (other, Type::Any) => Ok(other.clone()),
        _ => Err(()),
    }
}

/// Genera la expresión Rust que compara dos valores del mismo tipo
/// como `bool`. Reemplaza el `==` derivado en `gen_type_def` post-F17.4b:
/// `std::sync::Mutex<T>` no impl PartialEq, así que el derive falla
/// cuando un campo es nominal / List / Map (todos `Arc<Mutex<...>>`).
///
/// Estrategia espejo del intérprete (value.rs `PartialEq for Value`):
///   - Primitivos: `lhs == rhs` directo (impls de stdlib).
///   - Nominal `Arc<Mutex<XData>>`: `Arc::ptr_eq` shortcut + lock+deref.
///     El lock+deref llega a `XData::eq` que también es custom.
///   - List `Arc<Mutex<Vec<T>>>` / Map `Arc<Mutex<Vec<(K,V)>>>`:
///     mismo patrón ptr_eq + lock; comparación interna por elemento
///     con recursión sobre T (resp. K, V).
///   - Option<T>: pattern match (Some+Some recurse, None+None true,
///     mismatch false).
///   - Result<T>: pattern match (Ok+Ok recurse, Err+Err == String).
///   - Function/Future/Any: `false` (Fitz nunca compara funciones, y
///     Future/Any no llegan a tipos de campo de `type`).
fn field_eq_expr(
    ty: &Type,
    lhs: &str,
    rhs: &str,
    _env: &TypeEnv,
) -> Result<String, FitzError> {
    // `_env` se conserva en la firma por simetría con `rust_type_for`
    // y para no romper call sites si en el futuro hace falta resolver
    // un Nominal por TypeId (hoy todos los casos se manejan por la
    // variante de `Type` directamente).
    match ty {
        Type::Int
        | Type::Float
        | Type::Str
        | Type::Bool
        | Type::Null
        | Type::Range => Ok(format!("({} == {})", lhs, rhs)),
        Type::Nominal(_) => Ok(format!(
            "(Arc::ptr_eq(&{lhs}, &{rhs}) \
             || *{lhs}.lock().unwrap() == *{rhs}.lock().unwrap())",
            lhs = lhs,
            rhs = rhs,
        )),
        Type::Nullable(inner) => {
            let inner_eq = field_eq_expr(inner, "__a", "__b", _env)?;
            Ok(format!(
                "(match (&{lhs}, &{rhs}) {{ \
                 (None, None) => true, \
                 (Some(__a), Some(__b)) => {inner_eq}, \
                 _ => false \
                 }})",
                lhs = lhs,
                rhs = rhs,
                inner_eq = inner_eq,
            ))
        }
        Type::List(inner) => {
            let inner_eq = field_eq_expr(inner, "__a", "__b", _env)?;
            Ok(format!(
                "(Arc::ptr_eq(&{lhs}, &{rhs}) || {{ \
                 let __x = {lhs}.lock().unwrap(); \
                 let __y = {rhs}.lock().unwrap(); \
                 __x.len() == __y.len() \
                 && __x.iter().zip(__y.iter()).all(|(__a, __b)| {inner_eq}) \
                 }})",
                lhs = lhs,
                rhs = rhs,
                inner_eq = inner_eq,
            ))
        }
        Type::Map(k, v) => {
            let k_eq = field_eq_expr(k, "__a.0", "__b.0", _env)?;
            let v_eq = field_eq_expr(v, "__a.1", "__b.1", _env)?;
            Ok(format!(
                "(Arc::ptr_eq(&{lhs}, &{rhs}) || {{ \
                 let __x = {lhs}.lock().unwrap(); \
                 let __y = {rhs}.lock().unwrap(); \
                 __x.len() == __y.len() \
                 && __x.iter().zip(__y.iter()).all(|(__a, __b)| ({k_eq} && {v_eq})) \
                 }})",
                lhs = lhs,
                rhs = rhs,
                k_eq = k_eq,
                v_eq = v_eq,
            ))
        }
        Type::Result(inner) => {
            let inner_eq = field_eq_expr(inner, "__a", "__b", _env)?;
            Ok(format!(
                "(match (&{lhs}, &{rhs}) {{ \
                 (Ok(__a), Ok(__b)) => {inner_eq}, \
                 (Err(__a), Err(__b)) => __a == __b, \
                 _ => false \
                 }})",
                lhs = lhs,
                rhs = rhs,
                inner_eq = inner_eq,
            ))
        }
        // Function/Future/Any: no comparables estructuralmente. PyAny
        // (post-8.7.1): tampoco — comparar dos PyObjects por field
        // requiere lock + Python::attach y la semántica "iguales si
        // son el mismo objeto" es lo que ya hace `PartialEq` del
        // newtype (por puntero). Dentro de un field eq derivado, dos
        // PyObjects distintos siempre dan `false`.
        Type::Function { .. } | Type::Future(_) | Type::Any | Type::PyAny => {
            Ok("false".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fase 8.7.2 — sanitiza un nombre de binding Python para usarlo como
/// sufijo de `static __FITZ_PY_BIND_<UPPER>`. Reemplaza caracteres
/// no-alfanuméricos por `_` y pasa a uppercase. `os.path` (que ya está
/// resuelto a binding `path` al llegar acá) → `PATH`.
fn sanitize_python_binding_static(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_uppercase());
        } else {
            out.push('_');
        }
    }
    out
}

/// Variante lowercase para el nombre de la fn getter (`__fitz_py_bind_path`).
fn sanitize_python_binding_lower(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
        } else {
            out.push('_');
        }
    }
    out
}

fn rust_type_for(t: &Type, env: &TypeEnv) -> Result<String, FitzError> {
    match t {
        Type::Int => Ok("i64".to_string()),
        Type::Float => Ok("f64".to_string()),
        Type::Str => Ok("String".to_string()),
        Type::Bool => Ok("bool".to_string()),
        Type::Null => Ok("()".to_string()),
        // Fase 8.7.1: `PyAny` Fitz → `__FitzPyObject` Rust (newtype
        // sobre `Arc<Py<PyAny>>`). El preludio Python ya define el
        // tipo si `uses_python = true`; programas sin imports Python
        // no llegan acá (el checker no produce `Type::PyAny` sin
        // imports Python).
        Type::PyAny => Ok("__FitzPyObject".to_string()),
        Type::Nominal(id) => Ok(env.info(*id).name.clone()),
        Type::Nullable(inner) => Ok(format!("Option<{}>", rust_type_for(inner, env)?)),
        // List<T> y Map<K, V> se modelan con `Arc<Mutex<>>` para
        // preservar la semántica de referencia compartida del intérprete
        // (push/pop/asignación de elementos visibles vía cualquier alias).
        // T = Any (literal mixto sin contexto) → error explícito; el
        // subset compilable exige tipo homogéneo concreto.
        Type::List(inner) => {
            if matches!(**inner, Type::Any) {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    "listas con elementos de tipos mixtos (`List<Any>`): el subset compilado \
                     necesita tipo homogéneo concreto. Anotá el tipo o usá `fitz run` para \
                     interpretarlo sin restricción."
                        .to_string(),
                ));
            }
            Ok(format!("Arc<Mutex<Vec<{}>>>", rust_type_for(inner, env)?))
        }
        Type::Map(k, v) => {
            if matches!(**k, Type::Any) || matches!(**v, Type::Any) {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    "mapas con claves o valores de tipos mixtos (`Map<Any, ...>` o \
                     `Map<..., Any>`): el subset compilado necesita tipos homogéneos \
                     concretos. Anotá el tipo o usá `fitz run` para interpretarlo \
                     sin restricción."
                        .to_string(),
                ));
            }
            Ok(format!(
                "Arc<Mutex<Vec<({}, {})>>>",
                rust_type_for(k, env)?,
                rust_type_for(v, env)?
            ))
        }
        // `Result<T>` Fitz → `Result<T, String>` Rust nativo (5b.4).
        // El Err side está pinned a `String`: matchea la práctica del
        // intérprete (find/get y todos los ejemplos construyen Err con
        // mensajes) y deja que el `?` Rust funcione sin glue. Si T = Any
        // (Err suelto sin contexto), dejamos que el contexto destino
        // (anotación / return type) lo refine; rustc fallará con
        // "type annotations needed" si nadie lo restringe.
        Type::Result(inner) => {
            let inner_rs = if matches!(**inner, Type::Any) {
                "_".to_string()
            } else {
                rust_type_for(inner, env)?
            };
            Ok(format!("Result<{}, String>", inner_rs))
        }
        // Higher-order (F12): tipo función Fitz → `Arc<dyn Fn(...) -> R
        // + Send + Sync>` Rust. F17.4b: el bound `Send + Sync` permite
        // que la closure viaje al runtime tokio multi-thread (handlers
        // HTTP en `fitz build`). Cumple solo si las capturas también
        // lo son — y `Shared<T>` = `Arc<Mutex<T>>` lo es, igual que
        // todos los primitivos Fitz. Trade-off: una indirección por
        // puntero por llamada, uniforme (vars, params, returns todos
        // toman el mismo tipo). Fn (inmutable) cubre todos los ejemplos
        // del cap 11 — FnMut/FnOnce son deuda residual.
        Type::Function { params, ret } => {
            let ps: Vec<String> = params
                .iter()
                .map(|p| rust_type_for(p, env))
                .collect::<Result<_, _>>()?;
            let ret_rs = rust_type_for(ret, env)?;
            Ok(format!("Arc<dyn Fn({}) -> {} + Send + Sync>", ps.join(", "), ret_rs))
        }
        // Fase 6.6: `Future<T>` Fitz → `Pin<Box<dyn Future<Output = T>>>`
        // Rust. Uniforme y compatible con `current_thread` runtime (no
        // exigimos `+ Send`). Aparece cuando el usuario guarda el future
        // suelto en una var o lo pasa como argumento; para return de
        // `async fn` no se usa esta ruta (Rust auto-envuelve con
        // `impl Future`). Si T = Any (gradual escape), emitimos `_` y
        // dejamos que rustc infiera desde el contexto.
        Type::Future(inner) => {
            let inner_rs = if matches!(**inner, Type::Any) {
                "_".to_string()
            } else {
                rust_type_for(inner, env)?
            };
            Ok(format!(
                "std::pin::Pin<Box<dyn std::future::Future<Output = {}>>>",
                inner_rs
            ))
        }
        other => Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "codegen 5b no soporta el tipo `{}` (primitivos + tipos custom + nullables + List<T> + Map<K, V>)",
                type_name(other)
            ),
        )),
    }
}

fn type_name(t: &Type) -> &'static str {
    match t {
        Type::Int => "Int",
        Type::Float => "Float",
        Type::Str => "Str",
        Type::Bool => "Bool",
        Type::Null => "Null",
        Type::Range => "Range",
        Type::Any => "Any",
        Type::PyAny => "PyAny",
        Type::List(_) => "List<...>",
        Type::Map(_, _) => "Map<...>",
        Type::Result(_) => "Result<...>",
        Type::Future(_) => "Future<...>",
        Type::Nullable(_) => "T?",
        Type::Nominal(_) => "<nominal>",
        Type::Function { .. } => "fn(...)",
    }
}

/// Versión "linda" del tipo para mensajes de error, con T concreto
/// (recursa en generics, resuelve nominales). `List<User>` en vez de
/// `List<...>`. Usar `type_name` solo cuando el detalle no importa.
fn display_type(t: &Type, env: &TypeEnv) -> String {
    match t {
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::Str => "Str".into(),
        Type::Bool => "Bool".into(),
        Type::Null => "Null".into(),
        Type::Range => "Range".into(),
        Type::Any => "Any".into(),
        Type::PyAny => "PyAny".into(),
        Type::List(inner) => format!("List<{}>", display_type(inner, env)),
        Type::Map(k, v) => format!("Map<{}, {}>", display_type(k, env), display_type(v, env)),
        Type::Result(inner) => format!("Result<{}>", display_type(inner, env)),
        Type::Future(inner) => format!("Future<{}>", display_type(inner, env)),
        Type::Nullable(inner) => format!("{}?", display_type(inner, env)),
        Type::Nominal(id) => env.info(*id).name.clone(),
        Type::Function { params, ret } => {
            let ps = params
                .iter()
                .map(|p| display_type(p, env))
                .collect::<Vec<_>>()
                .join(", ");
            format!("fn({}) -> {}", ps, display_type(ret, env))
        }
    }
}

/// `true` si el tipo subyacente NO es `Copy` en el Rust generado y por
/// ende necesita `.clone()` cuando se evalúa un `Ident`/`Field` que se
/// va a consumir en otro contexto.
///
/// Para List/Map el clone es del `Rc` envolvente — barato y, lo más
/// importante, **preserva el aliasing**: dos vars que se construyeron
/// a partir de la misma lista comparten contenido y mutaciones vía
/// `push`/asignación se ven en ambas. Mismo criterio que para Nominal.
fn needs_clone(t: &Type) -> bool {
    match t {
        Type::Int | Type::Float | Type::Bool | Type::Null => false,
        Type::Str | Type::Nominal(_) => true,
        // `Option<T>` no es Copy salvo casos extremos; clonamos siempre.
        Type::Nullable(_) => true,
        // `Arc<Mutex<Vec<...>>>` — clone del Rc, barato, alias preservado.
        Type::List(_) | Type::Map(_, _) => true,
        // `Result<T, String>` no es Copy (String tampoco lo es), y el T
        // adentro puede ser Str/Nominal/List/etc. — clonamos por valor.
        Type::Result(_) => true,
        // Funciones-como-valor: `Arc<dyn Fn(...) -> R>` — clone del Rc,
        // barato y comparte el closure (alias semántico, mismo patrón
        // que List/Map/Nominal).
        Type::Function { .. } => true,
        // Fallback conservador: clonamos.
        _ => true,
    }
}

/// Coerciona una expresión Rust (`code`) de tipo Fitz `from` al tipo
/// Fitz `to`. Devuelve la expresión Rust resultante. Si no aplica
/// ninguna coerción, devuelve `code` tal cual.
///
/// Coerciones soportadas:
///   - `Int → Float`           → `(x as f64)`
///   - `T   → T?`               → `Some(x)` (con eventual clone de T)
///   - `Null → T?`              → `None`
fn coerce(code: &str, from: &Type, to: &Type) -> String {
    match (from, to) {
        (Type::Int, Type::Float) => format!("({} as f64)", code),
        (Type::Null, Type::Nullable(_)) => "None".to_string(),
        (from, Type::Nullable(inner)) if !matches!(from, Type::Nullable(_)) => {
            let coerced = coerce(code, from, inner);
            format!("Some({})", coerced)
        }
        // Fase 8.7.1: auto-coerción primitiva desde PyAny. Replica la
        // política `py_to_value` del intérprete (8.1.3): bool → bool,
        // int → i64 (con check de rango), float → f64, str → String.
        // Para tipos no primitivos (List, Map, Nominal), el codegen
        // deja la coerción para sub-pasos futuros (8.7.2 marshaling
        // compuesto, 8.7.3 async).
        (Type::PyAny, Type::Int) => format!("__fitz_py_extract_i64(&{})", code),
        (Type::PyAny, Type::Float) => format!("__fitz_py_extract_f64(&{})", code),
        (Type::PyAny, Type::Str) => format!("__fitz_py_extract_string(&{})", code),
        (Type::PyAny, Type::Bool) => format!("__fitz_py_extract_bool(&{})", code),
        _ => code.to_string(),
    }
}

fn numeric_coerce(
    lc: &str,
    lt: &Type,
    rc: &str,
    rt: &Type,
) -> Option<(String, String, Type)> {
    match (lt, rt) {
        (Type::Int, Type::Int) => Some((lc.into(), rc.into(), Type::Int)),
        (Type::Float, Type::Float) => Some((lc.into(), rc.into(), Type::Float)),
        (Type::Int, Type::Float) => Some((format!("({} as f64)", lc), rc.into(), Type::Float)),
        (Type::Float, Type::Int) => Some((lc.into(), format!("({} as f64)", rc), Type::Float)),
        _ => None,
    }
}

/// Devuelve una **expresión Rust** que evalúa a `String` y representa
/// el valor `code` (de tipo Fitz `ty`) en formato `print` top-level:
/// strings sin comillas, null como `"null"`, floats con `.0` si tienen
/// fracción 0, instancias delegando a su Display, Option como `"null"`
/// cuando None.
fn show_expr(code: &str, ty: &Type) -> String {
    match ty {
        Type::Int | Type::Bool => format!("format!(\"{{}}\", {})", code),
        Type::Float => format!("__fitz_fmt_float({})", code),
        Type::Str => format!("({}).clone()", code),
        Type::Null => "String::from(\"null\")".to_string(),
        // Fase 8.7.1: `PyAny` opaco → delegar al `Display` del newtype
        // `__FitzPyObject`, que adentro hace `Python::attach` + `__str__`.
        // Paridad bit-a-bit con `fitz run`: `print(math.pi)` produce
        // "3.141592653589793" en ambos paths cuando el lado Python
        // tiene un float (su `__str__` coincide con `__fitz_fmt_float`).
        Type::PyAny => format!("format!(\"{{}}\", {})", code),
        Type::Nominal(_) => format!("format!(\"{{}}\", &*({}).lock().unwrap())", code),
        Type::Nullable(inner) => {
            // Capturamos el valor por referencia para no consumirlo.
            // Para `Option<T>`, el match bindea `Some(__v)` y delega a
            // `show_expr` con código `__v` y tipo `*inner`. El `Option`
            // queda intacto.
            let inner_show = show_expr("__v", inner);
            format!(
                "(match &({}) {{ Some(__v) => {}, None => String::from(\"null\") }})",
                code, inner_show
            )
        }
        // List/Map en print top-level usan el formato "inline" (strings
        // con comillas adentro de los items, igual que `write_inline_value`
        // del intérprete). Construimos el string en runtime concatenando
        // sub-shows item por item. Ligamos primero el `Rc` a una `let`
        // antes de hacer `.lock().unwrap()` para extender la vida del temporal
        // — `(xs.clone()).lock().unwrap()` cae con la expresión.
        Type::List(inner) => {
            // Iteramos con `.cloned()` para que `__it` sea por valor
            // (no `&T`) — uniforma el código de `show_expr_inline` con
            // el de `show_expr` general (que asume valor). El clone es
            // barato para `Arc<Mutex<...>>` (Nominal/List/Map) y vivible
            // para `String` en contexto de print.
            let item_show = show_expr_inline("__it", inner);
            format!(
                "{{ \
                    let __list = {}; \
                    let __items = __list.lock().unwrap(); \
                    let mut __s = String::from(\"[\"); \
                    for (__i, __it) in __items.iter().cloned().enumerate() {{ \
                        if __i > 0 {{ __s.push_str(\", \"); }} \
                        __s.push_str(&({})); \
                    }} \
                    __s.push(']'); \
                    __s \
                }}",
                code, item_show
            )
        }
        Type::Map(kt, vt) => {
            let k_show = show_expr_inline("__k", kt);
            let v_show = show_expr_inline("__v", vt);
            format!(
                "{{ \
                    let __map = {}; \
                    let __pairs = __map.lock().unwrap(); \
                    let mut __s = String::from(\"{{\"); \
                    for (__i, (__k, __v)) in __pairs.iter().cloned().enumerate() {{ \
                        if __i > 0 {{ __s.push_str(\", \"); }} \
                        __s.push_str(&({})); \
                        __s.push_str(\": \"); \
                        __s.push_str(&({})); \
                    }} \
                    __s.push('}}'); \
                    __s \
                }}",
                code, k_show, v_show
            )
        }
        // Result<T> → `Ok(<inline T>)` o `Err("<msg>")`. El inner del Ok
        // se formatea con `show_expr_inline` (strings con comillas, igual
        // al intérprete); el Err side está pinned a `String` y siempre se
        // muestra con comillas dobles.
        Type::Result(inner) => {
            let ok_show = show_expr_inline("__v", inner);
            format!(
                "(match &({}) {{ \
                    Ok(__v) => format!(\"Ok({{}})\", {{ let __v = __v.clone(); {} }}), \
                    Err(__e) => format!(\"Err(\\\"{{}}\\\")\", __e) \
                }})",
                code, ok_show
            )
        }
        // Range, Any, Function — fallback. Si el AST cuela algo que llega
        // acá, el error principal viene de otro lado.
        _ => format!("format!(\"{{:?}}\", {})", code),
    }
}

/// Versión "inline" de `show_expr` para items adentro de colecciones:
/// strings van **entre comillas** (igual a `write_inline_value` del
/// intérprete). Llama a `show_expr` para todo lo demás.
fn show_expr_inline(code: &str, ty: &Type) -> String {
    match ty {
        Type::Str => format!("format!(\"\\\"{{}}\\\"\", {})", code),
        _ => show_expr(code, ty),
    }
}

/// Devuelve **una o más sentencias Rust** que escriben `code` (de tipo
/// Fitz `ty`) en el `Formatter` `__f`, en formato "inline" (el que se
/// usa adentro de `Display for FooData`): strings ENTRE COMILLAS,
/// instancias por Display, Option como `"null"` cuando None. Igual a
/// `write_inline_value` del intérprete.
fn inline_display_stmt(code: &str, ty: &Type) -> String {
    match ty {
        Type::Int | Type::Bool => format!("        write!(__f, \"{{}}\", {})?;\n", code),
        Type::Float => format!("        write!(__f, \"{{}}\", __fitz_fmt_float({}))?;\n", code),
        // Para Str adentro de Instance, mostramos con comillas dobles
        // alrededor (igual que el `write_inline_value` del intérprete).
        Type::Str => format!("        write!(__f, \"\\\"{{}}\\\"\", {})?;\n", code),
        Type::Null => "        write!(__f, \"null\")?;\n".to_string(),
        Type::Nominal(_) => format!(
            "        {{ let __t = ({}).lock().unwrap(); write!(__f, \"{{}}\", &*__t)?; }}\n",
            code
        ),
        Type::Nullable(inner) => {
            // Borroweamos el `Option<T>` y matcheamos por referencia.
            // Para Nominal adentro de Some, el match bindea `__v` como
            // `&Arc<Mutex<T>>`, así que necesitamos `(*__v)` o pasar
            // un sub-código. Para tipos primitivos, `&T` también
            // funciona porque Display está implementado para &T.
            let inner_body = match inner.as_ref() {
                Type::Int | Type::Bool => "                write!(__f, \"{}\", __v)?;\n".to_string(),
                Type::Float => "                write!(__f, \"{}\", __fitz_fmt_float(*__v))?;\n".to_string(),
                Type::Str => "                write!(__f, \"\\\"{}\\\"\", __v)?;\n".to_string(),
                Type::Null => "                write!(__f, \"null\")?;\n".to_string(),
                Type::Nominal(_) => {
                    "                { let __t = (*__v).lock().unwrap(); write!(__f, \"{}\", &*__t)?; }\n"
                        .to_string()
                }
                _ => "                write!(__f, \"{:?}\", __v)?;\n".to_string(),
            };
            format!(
                "        match &({}) {{\n            Some(__v) => {{\n{}            }}\n            None => write!(__f, \"null\")?,\n        }}\n",
                code, inner_body
            )
        }
        _ => format!("        write!(__f, \"{{:?}}\", {})?;\n", code),
    }
}

fn check_method_arity(method: &str, args: &[Expr], expected: usize) -> Result<(), FitzError> {
    if args.len() != expected {
        return Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "el método `{}` toma {} argumento(s), recibió {}",
                method,
                expected,
                args.len()
            ),
        ));
    }
    Ok(())
}

fn rust_str_literal(s: &str) -> String {
    // Genera un literal Rust válido escapando comillas y barras.
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::types::check_program;

    fn gen(src: &str) -> Result<String, FitzError> {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (env, _types, _defs, errors) = check_program(&program);
        if !errors.is_empty() {
            panic!("checker errors: {:?}", errors);
        }
        generate_rust(&program, &env)
    }

    fn assert_err_contains(src: &str, needles: &[&str]) {
        let err = gen(src).expect_err("esperaba error de codegen");
        for n in needles {
            assert!(
                err.message.contains(n),
                "esperaba `{}` en el error, fue: {}",
                n,
                err.message
            );
        }
    }

    /// Llama a `generate_rust` ignorando errores del checker. Solo para
    /// probar **barreras defensivas** del codegen sobre features que el
    /// checker también rechaza. El flujo normal aborta en el checker;
    /// este helper salta esa etapa para forzar al codegen a ver el AST
    /// y verificar que su propia barrera está en su lugar. Sin barreras
    /// activas hoy (Fase 6.6 cerró la última, `.await`), pero queda
    /// disponible para barreras futuras.
    #[allow(dead_code)]
    fn gen_ignoring_check(src: &str) -> Result<String, FitzError> {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (env, _types, _defs, _errors) = check_program(&program);
        generate_rust(&program, &env)
    }

    // ---- Fase 6.6: codegen async (async fn / .await / sleep) ----

    #[test]
    fn async_fn_emite_pub_async_fn_rust() {
        // `async fn f() -> Int { return 42 }` → `pub async fn f() -> i64`.
        let code = gen("async fn f() -> Int { return 42 }").unwrap();
        let file = ast_test::parse(&code);
        let f = ast_test::find_item_fn(&file, "f").expect("fn f no emitida");
        assert!(ast_test::fn_is_async(f), "esperaba async fn, no era async");
        // Sanidad: return type es i64.
        let ret = ast_test::fn_return_type(f).unwrap_or_default();
        assert!(ret.contains("i64"), "esperaba return type i64, fue: {}", ret);
    }

    #[test]
    fn sync_fn_no_emite_async() {
        // Programa sync no debe emitir `async fn`.
        let code = gen("fn double(n: Int) -> Int => n * 2").unwrap();
        let file = ast_test::parse(&code);
        let f = ast_test::find_item_fn(&file, "double").expect("fn double");
        assert!(!ast_test::fn_is_async(f), "no debería ser async");
    }

    #[test]
    fn await_emite_dot_await_rust() {
        // `inner().await` → `(inner()).await` Rust.
        let code = gen(
            "async fn inner() -> Int { return 1 }\n\
             async fn outer() -> Int { return inner().await }",
        ).unwrap();
        // Inspeccionamos el body de `outer`: debe contener un
        // `.await` aplicado al call.
        let file = ast_test::parse(&code);
        let outer = ast_test::find_item_fn(&file, "outer").expect("fn outer");
        let body = ast_test::fn_body_text(outer);
        // syn::ToTokens normaliza con espacios: `.await` aparece como
        // `. await` después del `quote!`.
        assert!(
            body.contains(". await") || body.contains(".await"),
            "esperaba `.await` en el body de outer, fue: {}",
            body
        );
    }

    #[test]
    fn sleep_builtin_emite_fitz_sleep_helper_y_call() {
        // `sleep(100)` → `__fitz_sleep(100i64)` + preludio del helper.
        let code = gen(
            "async fn f() -> Int {\n\
                 let _ = sleep(100).await\n\
                 return 0\n\
             }",
        ).unwrap();
        // Helper `__fitz_sleep` debe estar en el preludio del crate.
        assert!(
            code.contains("async fn __fitz_sleep"),
            "esperaba `async fn __fitz_sleep` en el output, no encontrado"
        );
        // El call site usa el helper.
        assert!(
            code.contains("__fitz_sleep(100i64)"),
            "esperaba llamada `__fitz_sleep(100i64)`"
        );
        // Y referencia a `tokio::time::sleep` adentro del helper.
        assert!(
            code.contains("tokio::time::sleep"),
            "esperaba referencia a `tokio::time::sleep`"
        );
    }

    #[test]
    fn cargo_toml_async_sin_http_incluye_tokio_time() {
        // Programa CLI con async → Cargo.toml mínimo + tokio con
        // feature `time` (sin axum).
        let toml = cargo_toml_for("foo", /*has_http=*/false, /*uses_async=*/true, /*uses_python=*/false);
        assert!(toml.contains("tokio"), "esperaba tokio en deps");
        assert!(toml.contains("\"time\""), "esperaba feature `time`");
        assert!(!toml.contains("axum"), "no debería incluir axum");
    }

    #[test]
    fn cargo_toml_async_con_http_incluye_tokio_time_y_axum() {
        let toml = cargo_toml_for("foo", /*has_http=*/true, /*uses_async=*/true, /*uses_python=*/false);
        assert!(toml.contains("axum"));
        assert!(toml.contains("\"time\""));
        assert!(toml.contains("\"macros\""));
    }

    #[test]
    fn cargo_toml_sin_async_sin_http_es_minimal() {
        let toml = cargo_toml_for("foo", false, false, /*uses_python=*/false);
        assert!(!toml.contains("[dependencies]"));
        assert!(!toml.contains("tokio"));
    }

    // ---------------------------------------------------------------
    // Fase 8.7.1 — Cargo.toml condicional con pyo3
    // ---------------------------------------------------------------

    #[test]
    fn cargo_toml_con_python_incluye_pyo3() {
        // Programa CLI con `from python import` → Cargo.toml suma pyo3
        // con `abi3-py310` + `auto-initialize`.
        let toml = cargo_toml_for("foo", /*has_http=*/false, /*uses_async=*/false, /*uses_python=*/true);
        assert!(toml.contains("[dependencies]"), "esperaba sección deps");
        assert!(toml.contains("pyo3"), "esperaba pyo3 en deps");
        assert!(toml.contains("\"abi3-py310\""), "esperaba feature abi3-py310");
        assert!(
            toml.contains("\"auto-initialize\""),
            "esperaba feature auto-initialize"
        );
        assert!(!toml.contains("axum"), "no debería incluir axum");
        assert!(!toml.contains("tokio"), "no debería incluir tokio");
    }

    #[test]
    fn cargo_toml_python_y_http_incluyen_ambos() {
        let toml = cargo_toml_for("foo", true, false, true);
        assert!(toml.contains("axum"));
        assert!(toml.contains("pyo3"));
        assert!(toml.contains("tokio"));
    }

    #[test]
    fn cargo_toml_sin_python_no_incluye_pyo3() {
        let toml = cargo_toml_for("foo", true, false, false);
        assert!(toml.contains("axum"));
        assert!(!toml.contains("pyo3"));
    }

    #[test]
    fn cli_con_async_emite_tokio_main_y_async_main() {
        // Programa CLI sin HTTP con async fn declarada → `fn main()`
        // se emite como `#[tokio::main(...)] async fn main()`.
        let code = gen(
            "async fn pause() -> Int { return 0 }\n\
             print(\"hi\")",
        ).unwrap();
        assert!(
            code.contains("#[tokio::main"),
            "esperaba `#[tokio::main]` en el output"
        );
        assert!(
            code.contains("async fn main"),
            "esperaba `async fn main` para CLI con async"
        );
    }

    #[test]
    fn cli_sync_no_emite_tokio_main() {
        // Programa CLI sync → `fn main()` plano, sin `#[tokio::main]`.
        let code = gen("print(\"hi\")").unwrap();
        assert!(
            !code.contains("#[tokio::main"),
            "no debería tener `#[tokio::main]` en CLI sync"
        );
        assert!(code.contains("fn main()"));
    }

    #[test]
    fn future_t_como_anotacion_de_var_emite_pin_box_dyn_future() {
        // `let f: Future<Int> = async_fn()` (sin await) → tipo
        // `Pin<Box<dyn Future<Output = i64>>>`.
        // Validamos vía `rust_type_for` directamente para evitar
        // chequear todo el flow.
        let env = crate::types::TypeEnv::new();
        let rs = rust_type_for(&Type::Future(Box::new(Type::Int)), &env).unwrap();
        assert!(rs.contains("Pin"), "esperaba Pin, fue: {}", rs);
        assert!(rs.contains("Future<Output = i64>"), "fue: {}", rs);
    }

    // ---- AST-based test helpers (T1 post-5b) ---------------------------
    //
    // Estos helpers parsean el Rust generado por el codegen con `syn` y
    // permiten asertar sobre la estructura del AST en lugar de matchear
    // strings literales del output. La ventaja es que cambios cosméticos
    // del codegen (espacios, sufijos numéricos, agrupación de paréntesis)
    // no rompen los tests — solo cambios estructurales reales lo hacen.
    //
    // Convención: las helpers toman `&str` como código generado y
    // devuelven `syn::File`/`&Item*`/`String` según corresponda. Para
    // stringificar subárboles usamos `quote::ToTokens::to_token_stream()`
    // y normalizamos whitespace, así las comparaciones son robustas.
    //
    // `#[allow(dead_code)]`: la migración de tests a AST-based es
    // incremental; algunas helpers pueden quedar sin uso hasta que se
    // migren más tests.
    #[allow(dead_code)]
    mod ast_test {
        use quote::ToTokens;
        use syn::{File, Item, ItemFn, ItemImpl, ItemStruct, ItemType, Local, Pat, Stmt};

        /// Parsea el Rust generado. Si syn no puede parsearlo, paniquea
        /// con el código completo en el mensaje (ya es una señal: el
        /// codegen está emitiendo Rust inválido).
        pub fn parse(code: &str) -> File {
            syn::parse_file(code).unwrap_or_else(|e| {
                panic!(
                    "syn no pudo parsear el Rust generado: {}\n\nCódigo:\n{}",
                    e, code
                )
            })
        }

        /// Stringifica cualquier nodo `ToTokens` con whitespace
        /// normalizado a un solo espacio entre tokens. Robusto contra
        /// cambios cosméticos de formato.
        pub fn ts<T: ToTokens>(node: &T) -> String {
            let raw = node.to_token_stream().to_string();
            raw.split_whitespace().collect::<Vec<_>>().join(" ")
        }

        pub fn find_item_fn<'a>(file: &'a File, name: &str) -> Option<&'a ItemFn> {
            file.items.iter().find_map(|i| match i {
                Item::Fn(f) if f.sig.ident == name => Some(f),
                _ => None,
            })
        }

        pub fn find_item_struct<'a>(file: &'a File, name: &str) -> Option<&'a ItemStruct> {
            file.items.iter().find_map(|i| match i {
                Item::Struct(s) if s.ident == name => Some(s),
                _ => None,
            })
        }

        pub fn find_item_type<'a>(file: &'a File, name: &str) -> Option<&'a ItemType> {
            file.items.iter().find_map(|i| match i {
                Item::Type(t) if t.ident == name => Some(t),
                _ => None,
            })
        }

        /// Encuentra un `static NAME: T = expr;` top-level por nombre.
        pub fn find_item_static<'a>(file: &'a File, name: &str) -> Option<&'a syn::ItemStatic> {
            file.items.iter().find_map(|i| match i {
                Item::Static(s) if s.ident == name => Some(s),
                _ => None,
            })
        }

        /// Encuentra un `const NAME: T = expr;` top-level por nombre.
        pub fn find_item_const<'a>(file: &'a File, name: &str) -> Option<&'a syn::ItemConst> {
            file.items.iter().find_map(|i| match i {
                Item::Const(c) if c.ident == name => Some(c),
                _ => None,
            })
        }

        /// True si la visibilidad declarada es `pub` (no `pub(crate)` ni
        /// privada).
        pub fn vis_is_pub(vis: &syn::Visibility) -> bool {
            matches!(vis, syn::Visibility::Public(_))
        }

        /// Busca un `impl Trait for Type` por nombre del trait y del
        /// tipo. El matching usa `contains` sobre la representación
        /// tokenizada — sirve para `impl std::fmt::Display for Foo`
        /// (matchea con `trait_name = "Display"`).
        pub fn find_impl<'a>(
            file: &'a File,
            trait_name: &str,
            type_name: &str,
        ) -> Option<&'a ItemImpl> {
            file.items.iter().find_map(|i| match i {
                Item::Impl(im) => {
                    let trait_match = im
                        .trait_
                        .as_ref()
                        .is_some_and(|(_, p, _)| ts(p).contains(trait_name));
                    let type_match = ts(&*im.self_ty).contains(type_name);
                    if trait_match && type_match {
                        Some(im)
                    } else {
                        None
                    }
                }
                _ => None,
            })
        }

        /// Devuelve los stmts del cuerpo de `fn main()`.
        pub fn main_block_stmts(file: &File) -> &[Stmt] {
            let f = find_item_fn(file, "main").expect("no encontré fn main en el código");
            &f.block.stmts
        }

        /// Devuelve el primer `let` cuyo pat bindea `name`.
        pub fn find_let<'a>(stmts: &'a [Stmt], name: &str) -> Option<&'a Local> {
            stmts.iter().find_map(|s| match s {
                Stmt::Local(l) if pat_binds(&l.pat, name) => Some(l),
                _ => None,
            })
        }

        /// Cuenta los `let` que bindean un nombre dado (útil para
        /// validar que una reasignación NO emite un `let` nuevo).
        pub fn count_lets(stmts: &[Stmt], name: &str) -> usize {
            stmts
                .iter()
                .filter(|s| matches!(s, Stmt::Local(l) if pat_binds(&l.pat, name)))
                .count()
        }

        fn pat_binds(p: &Pat, name: &str) -> bool {
            match p {
                Pat::Ident(pi) => pi.ident == name,
                Pat::Type(pt) => pat_binds(&pt.pat, name),
                _ => false,
            }
        }

        /// Devuelve la representación textual del tipo del `let`
        /// (lado derecho del `:`). `None` si el let no tiene tipo
        /// explícito.
        pub fn local_type(local: &Local) -> Option<String> {
            if let Pat::Type(pt) = &local.pat {
                return Some(ts(&*pt.ty));
            }
            None
        }

        /// Devuelve la representación textual del initializer del `let`
        /// (lado derecho del `=`). `None` si el let no tiene init.
        pub fn local_init(local: &Local) -> Option<String> {
            local.init.as_ref().map(|li| ts(&*li.expr))
        }

        /// True si el `let` declara la var como `mut`.
        pub fn local_is_mut(local: &Local) -> bool {
            match &local.pat {
                Pat::Ident(pi) => pi.mutability.is_some(),
                Pat::Type(pt) => match pt.pat.as_ref() {
                    Pat::Ident(pi) => pi.mutability.is_some(),
                    _ => false,
                },
                _ => false,
            }
        }

        /// Cuenta invocaciones de un macro (por nombre del segmento
        /// final, ej. `println` matchea `println!` y `std::println!`).
        pub fn count_macro_calls(stmts: &[Stmt], macro_name: &str) -> usize {
            use syn::visit::Visit;
            struct V<'a> {
                name: &'a str,
                count: usize,
            }
            impl<'a, 'ast> Visit<'ast> for V<'a> {
                fn visit_macro(&mut self, m: &'ast syn::Macro) {
                    if m.path.segments.last().is_some_and(|s| s.ident == self.name) {
                        self.count += 1;
                    }
                    syn::visit::visit_macro(self, m);
                }
            }
            let mut v = V {
                name: macro_name,
                count: 0,
            };
            for s in stmts {
                v.visit_stmt(s);
            }
            v.count
        }

        /// True si en cualquier nivel del AST aparece una llamada a
        /// método con el nombre dado (ej. `borrow`, `clone`).
        pub fn contains_method_call(stmts: &[Stmt], method_name: &str) -> bool {
            use syn::visit::Visit;
            struct V<'a> {
                name: &'a str,
                found: bool,
            }
            impl<'a, 'ast> Visit<'ast> for V<'a> {
                fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
                    if e.method == self.name {
                        self.found = true;
                    }
                    syn::visit::visit_expr_method_call(self, e);
                }
            }
            let mut v = V {
                name: method_name,
                found: false,
            };
            for s in stmts {
                v.visit_stmt(s);
            }
            v.found
        }

        /// Encuentra el primer `for` loop en los stmts (búsqueda
        /// recursiva, no solo top-level).
        pub fn find_for_loop(stmts: &[Stmt]) -> Option<syn::ExprForLoop> {
            use syn::visit::Visit;
            struct V {
                found: Option<syn::ExprForLoop>,
            }
            impl<'ast> Visit<'ast> for V {
                fn visit_expr_for_loop(&mut self, e: &'ast syn::ExprForLoop) {
                    if self.found.is_none() {
                        self.found = Some(e.clone());
                    }
                    syn::visit::visit_expr_for_loop(self, e);
                }
            }
            let mut v = V { found: None };
            for s in stmts {
                v.visit_stmt(s);
            }
            v.found
        }

        /// Encuentra el primer `while` loop en los stmts.
        pub fn find_while_loop(stmts: &[Stmt]) -> Option<syn::ExprWhile> {
            use syn::visit::Visit;
            struct V {
                found: Option<syn::ExprWhile>,
            }
            impl<'ast> Visit<'ast> for V {
                fn visit_expr_while(&mut self, e: &'ast syn::ExprWhile) {
                    if self.found.is_none() {
                        self.found = Some(e.clone());
                    }
                    syn::visit::visit_expr_while(self, e);
                }
            }
            let mut v = V { found: None };
            for s in stmts {
                v.visit_stmt(s);
            }
            v.found
        }

        /// Encuentra el primer `if` (statement o expresión) en los stmts.
        pub fn find_if(stmts: &[Stmt]) -> Option<syn::ExprIf> {
            use syn::visit::Visit;
            struct V {
                found: Option<syn::ExprIf>,
            }
            impl<'ast> Visit<'ast> for V {
                fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
                    if self.found.is_none() {
                        self.found = Some(e.clone());
                    }
                    syn::visit::visit_expr_if(self, e);
                }
            }
            let mut v = V { found: None };
            for s in stmts {
                v.visit_stmt(s);
            }
            v.found
        }

        /// True si la signatura del `fn` declara un derive con el
        /// nombre dado (ej. `PartialEq`).
        pub fn struct_has_derive(s: &ItemStruct, derive: &str) -> bool {
            s.attrs.iter().any(|a| {
                if !a.path().is_ident("derive") {
                    return false;
                }
                let mut found = false;
                let _ = a.parse_nested_meta(|meta| {
                    if meta.path.is_ident(derive) {
                        found = true;
                    }
                    Ok(())
                });
                found
            })
        }

        /// Cuenta el número de parámetros de un `fn`.
        pub fn fn_arity(f: &ItemFn) -> usize {
            f.sig.inputs.len()
        }

        /// Devuelve el tipo de retorno del `fn` como string normalizado,
        /// o `None` si retorna `()`.
        pub fn fn_return_type(f: &ItemFn) -> Option<String> {
            match &f.sig.output {
                syn::ReturnType::Default => None,
                syn::ReturnType::Type(_, ty) => Some(ts(&**ty)),
            }
        }

        /// Devuelve los tipos de los parámetros como strings
        /// normalizados (omite `self`).
        pub fn fn_param_types(f: &ItemFn) -> Vec<String> {
            f.sig
                .inputs
                .iter()
                .filter_map(|arg| match arg {
                    syn::FnArg::Typed(pt) => Some(ts(&*pt.ty)),
                    _ => None,
                })
                .collect()
        }

        /// Devuelve la expresión del initializer del `let` como
        /// `&syn::Expr`. Útil para inspección estructural cuando
        /// `local_init` (que devuelve String) no alcanza.
        pub fn local_init_expr(local: &Local) -> Option<&syn::Expr> {
            local.init.as_ref().map(|li| &*li.expr)
        }

        /// Si la expresión es una llamada (`Expr::Call`), devuelve el
        /// path del callee normalizado. None si no es una llamada.
        pub fn call_path(expr: &syn::Expr) -> Option<String> {
            // Atravesamos paréntesis para que `(Arc::new(...))` matchee.
            let mut e = expr;
            while let syn::Expr::Paren(p) = e {
                e = &*p.expr;
            }
            match e {
                syn::Expr::Call(c) => Some(ts(&*c.func)),
                _ => None,
            }
        }

        /// Devuelve los nombres de los métodos en la cadena de method
        /// calls que termina en `expr`, en orden de aplicación
        /// (receptor → puntas). Ej. `xs.lock().unwrap().clone().into_iter()`
        /// → `["borrow", "clone", "into_iter"]`.
        ///
        /// Atraviesa paréntesis y casts (`expr as T`) — si el `expr`
        /// es `(xs.lock().unwrap().len() as i64)` devuelve la chain de
        /// adentro del cast, ignorando el cast.
        pub fn method_chain_names(expr: &syn::Expr) -> Vec<String> {
            let mut names = Vec::new();
            let mut e = expr;
            loop {
                match e {
                    syn::Expr::MethodCall(mc) => {
                        names.push(mc.method.to_string());
                        e = &*mc.receiver;
                    }
                    syn::Expr::Paren(p) => {
                        e = &*p.expr;
                    }
                    syn::Expr::Cast(c) => {
                        e = &*c.expr;
                    }
                    _ => break,
                }
            }
            names.reverse();
            names
        }

        /// Encuentra el primer macro call con el nombre dado adentro
        /// del expr y devuelve sus tokens normalizados. None si no
        /// hay match.
        pub fn find_macro_args(expr: &syn::Expr, macro_name: &str) -> Option<String> {
            use syn::visit::Visit;
            struct V<'a> {
                name: &'a str,
                found: Option<String>,
            }
            impl<'a, 'ast> Visit<'ast> for V<'a> {
                fn visit_macro(&mut self, m: &'ast syn::Macro) {
                    if self.found.is_none()
                        && m.path.segments.last().is_some_and(|s| s.ident == self.name)
                    {
                        self.found = Some(ts(&m.tokens));
                    }
                    syn::visit::visit_macro(self, m);
                }
            }
            let mut v = V {
                name: macro_name,
                found: None,
            };
            v.visit_expr(expr);
            v.found
        }

        /// Cuenta cuántas veces aparece una llamada a un método con el
        /// nombre dado en cualquier nivel del AST de la expresión.
        pub fn count_method_calls_in_expr(expr: &syn::Expr, method_name: &str) -> usize {
            use syn::visit::Visit;
            struct V<'a> {
                name: &'a str,
                count: usize,
            }
            impl<'a, 'ast> Visit<'ast> for V<'a> {
                fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
                    if e.method == self.name {
                        self.count += 1;
                    }
                    syn::visit::visit_expr_method_call(self, e);
                }
            }
            let mut v = V {
                name: method_name,
                count: 0,
            };
            v.visit_expr(expr);
            v.count
        }

        /// True si en cualquier nivel del expr aparece una llamada a
        /// método con el nombre dado.
        pub fn contains_method_call_in_expr(expr: &syn::Expr, method_name: &str) -> bool {
            count_method_calls_in_expr(expr, method_name) > 0
        }

        /// True si el expr contiene una invocación de macro con el
        /// nombre dado (búsqueda recursiva).
        pub fn contains_macro_in_expr(expr: &syn::Expr, macro_name: &str) -> bool {
            find_macro_args(expr, macro_name).is_some()
        }

        /// Si el expr es un cast `<inner> as <ty>` (opcionalmente
        /// envuelto en paréntesis), devuelve el tipo destino del cast
        /// normalizado. None si la expresión raíz no es un cast.
        pub fn cast_target_type(expr: &syn::Expr) -> Option<String> {
            let mut e = expr;
            while let syn::Expr::Paren(p) = e {
                e = &*p.expr;
            }
            match e {
                syn::Expr::Cast(c) => Some(ts(&*c.ty)),
                _ => None,
            }
        }

        /// Devuelve los attributes de un `fn` como strings tokenizados.
        /// Útil para verificar `#[tokio::main(...)]` y similares.
        pub fn fn_attrs(f: &ItemFn) -> Vec<String> {
            f.attrs.iter().map(ts).collect()
        }

        /// True si el `fn` está marcado como `async`.
        pub fn fn_is_async(f: &ItemFn) -> bool {
            f.sig.asyncness.is_some()
        }

        /// Devuelve el cuerpo de un `fn` tokenizado y normalizado.
        pub fn fn_body_text(f: &ItemFn) -> String {
            ts(&f.block)
        }

        /// Devuelve los pares `(pat, ty)` de los params del `fn` como
        /// strings tokenizados. Útil para verificar que un handler tiene
        /// extractores axum específicos como params.
        pub fn fn_param_pats_and_types(f: &ItemFn) -> Vec<(String, String)> {
            f.sig
                .inputs
                .iter()
                .filter_map(|arg| match arg {
                    syn::FnArg::Typed(pt) => Some((ts(&*pt.pat), ts(&*pt.ty))),
                    _ => None,
                })
                .collect()
        }

        /// Encuentra una invocación de macro top-level por nombre del
        /// segmento final del path (ej. `thread_local`). None si no hay.
        pub fn find_top_macro<'a>(file: &'a File, name: &str) -> Option<&'a syn::ItemMacro> {
            file.items.iter().find_map(|i| match i {
                Item::Macro(im)
                    if im
                        .mac
                        .path
                        .segments
                        .last()
                        .is_some_and(|s| s.ident == name) =>
                {
                    Some(im)
                }
                _ => None,
            })
        }

        /// Cuenta cuántas invocaciones top-level de un macro con el
        /// nombre dado hay en el archivo.
        pub fn count_top_macros(file: &File, name: &str) -> usize {
            file.items
                .iter()
                .filter(|i| {
                    matches!(
                        i,
                        Item::Macro(im) if im.mac.path.segments.last().is_some_and(|s| s.ident == name)
                    )
                })
                .count()
        }

        /// Devuelve todas las invocaciones de `.route(...)` dentro de
        /// `fn main` como `(primer_arg_tokenizado, segundo_arg_tokenizado)`.
        /// Para `.route("/users", axum::routing::get(__handler_x))` da
        /// `("\"/users\"", "axum :: routing :: get ( __handler_x )")`.
        pub fn find_route_registrations(file: &File) -> Vec<(String, String)> {
            let main = match find_item_fn(file, "main") {
                Some(m) => m,
                None => return Vec::new(),
            };
            use syn::visit::Visit;
            struct V {
                out: Vec<(String, String)>,
            }
            impl<'ast> Visit<'ast> for V {
                fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
                    if e.method == "route" && e.args.len() == 2 {
                        let mut iter = e.args.iter();
                        let p = iter.next().unwrap();
                        let h = iter.next().unwrap();
                        self.out.push((ts(p), ts(h)));
                    }
                    syn::visit::visit_expr_method_call(self, e);
                }
            }
            let mut v = V { out: Vec::new() };
            v.visit_block(&main.block);
            v.out
        }

        /// Busca un `let` con un nombre dado adentro de un `fn`,
        /// recursivamente. Devuelve un clon del `Local` o None.
        pub fn find_local_in_fn(f: &ItemFn, name: &str) -> Option<Local> {
            use syn::visit::Visit;
            struct V<'a> {
                name: &'a str,
                found: Option<Local>,
            }
            impl<'a, 'ast> Visit<'ast> for V<'a> {
                fn visit_local(&mut self, l: &'ast Local) {
                    if self.found.is_none() && pat_binds(&l.pat, self.name) {
                        self.found = Some(l.clone());
                    }
                    syn::visit::visit_local(self, l);
                }
            }
            let mut v = V { name, found: None };
            v.visit_block(&f.block);
            v.found
        }

        /// Busca todos los `let` con un nombre dado adentro de un `fn`
        /// (cualquier nivel de anidamiento). Útil para validar
        /// reasignación vs creación múltiple del binding.
        pub fn count_locals_in_fn(f: &ItemFn, name: &str) -> usize {
            use syn::visit::Visit;
            struct V<'a> {
                name: &'a str,
                count: usize,
            }
            impl<'a, 'ast> Visit<'ast> for V<'a> {
                fn visit_local(&mut self, l: &'ast Local) {
                    if pat_binds(&l.pat, self.name) {
                        self.count += 1;
                    }
                    syn::visit::visit_local(self, l);
                }
            }
            let mut v = V { name, count: 0 };
            v.visit_block(&f.block);
            v.count
        }

        /// True si el `fn` body contiene un return cuya expresión
        /// matchea (tokenizada) con todas las needles dadas.
        pub fn fn_body_returns_any_matching(f: &ItemFn, needles: &[&str]) -> bool {
            use syn::visit::Visit;
            struct V<'a, 'b> {
                needles: &'a [&'b str],
                found: bool,
            }
            impl<'a, 'b, 'ast> Visit<'ast> for V<'a, 'b> {
                fn visit_expr_return(&mut self, e: &'ast syn::ExprReturn) {
                    if let Some(ex) = &e.expr {
                        let t = ts(&**ex);
                        if self.needles.iter().all(|n| t.contains(n)) {
                            self.found = true;
                        }
                    }
                    syn::visit::visit_expr_return(self, e);
                }
            }
            let mut v = V {
                needles,
                found: false,
            };
            v.visit_block(&f.block);
            v.found
        }

        /// Encuentra el primer `match` expr en los stmts (búsqueda
        /// recursiva). Devuelve un clon de `ExprMatch` o None.
        pub fn find_match(stmts: &[Stmt]) -> Option<syn::ExprMatch> {
            use syn::visit::Visit;
            struct V {
                found: Option<syn::ExprMatch>,
            }
            impl<'ast> Visit<'ast> for V {
                fn visit_expr_match(&mut self, e: &'ast syn::ExprMatch) {
                    if self.found.is_none() {
                        self.found = Some(e.clone());
                    }
                    syn::visit::visit_expr_match(self, e);
                }
            }
            let mut v = V { found: None };
            for s in stmts {
                v.visit_stmt(s);
            }
            v.found
        }

        /// Devuelve los tokens normalizados del primer macro call con el
        /// nombre dado en los stmts (búsqueda recursiva, atraviesa todos
        /// los anidamientos). None si no hay match.
        pub fn first_macro_args_in_stmts(stmts: &[Stmt], name: &str) -> Option<String> {
            use syn::visit::Visit;
            struct V<'a> {
                name: &'a str,
                found: Option<String>,
            }
            impl<'a, 'ast> Visit<'ast> for V<'a> {
                fn visit_macro(&mut self, m: &'ast syn::Macro) {
                    if self.found.is_none()
                        && m.path.segments.last().is_some_and(|s| s.ident == self.name)
                    {
                        self.found = Some(ts(&m.tokens));
                    }
                    syn::visit::visit_macro(self, m);
                }
            }
            let mut v = V { name, found: None };
            for s in stmts {
                v.visit_stmt(s);
            }
            v.found
        }

        /// True si en el `fn` body aparece un `match` con al menos un
        /// arm cuyo pat contiene la needle dada (tokenizada).
        pub fn fn_body_has_match_arm_pat(f: &ItemFn, pat_needle: &str) -> bool {
            use syn::visit::Visit;
            struct V<'a> {
                needle: &'a str,
                found: bool,
            }
            impl<'a, 'ast> Visit<'ast> for V<'a> {
                fn visit_arm(&mut self, a: &'ast syn::Arm) {
                    if ts(&a.pat).contains(self.needle) {
                        self.found = true;
                    }
                    syn::visit::visit_arm(self, a);
                }
            }
            let mut v = V {
                needle: pat_needle,
                found: false,
            };
            v.visit_block(&f.block);
            v.found
        }
    }

    #[test]
    fn programa_vacio_genera_main_vacio() {
        let code = gen("").unwrap();
        let file = ast_test::parse(&code);
        let main = ast_test::find_item_fn(&file, "main").expect("falta fn main");
        // main vacío: sin params, sin return type, body sin stmts.
        assert_eq!(ast_test::fn_arity(main), 0);
        assert!(ast_test::fn_return_type(main).is_none());
        assert!(
            main.block.stmts.is_empty(),
            "esperaba main body vacío, got: {}",
            ast_test::fn_body_text(main)
        );
    }

    #[test]
    fn let_int_anotado_genera_i64() {
        let file = ast_test::parse(&gen("let x: Int = 42").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        assert!(ast_test::local_is_mut(l));
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        assert_eq!(ast_test::local_init(l).as_deref(), Some("42i64"));
    }

    #[test]
    fn let_int_inferido_genera_i64() {
        let file = ast_test::parse(&gen("let x = 42").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        assert_eq!(ast_test::local_init(l).as_deref(), Some("42i64"));
    }

    #[test]
    fn let_float_anotado_genera_f64_con_coercion_int() {
        let file = ast_test::parse(&gen("let pi: Float = 3").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "pi").expect("falta let pi");
        assert_eq!(ast_test::local_type(l).as_deref(), Some("f64"));
        // El init coerciona Int→Float: `(3i64 as f64)`.
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("3i64") && init.contains("as f64"),
            "esperaba init con coerción Int→Float, fue: {}",
            init
        );
    }

    #[test]
    fn let_str_genera_string() {
        let file = ast_test::parse(&gen("let name = \"Fitz\"").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "name").expect("falta let name");
        assert_eq!(ast_test::local_type(l).as_deref(), Some("String"));
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("String :: from") && init.contains("\"Fitz\""),
            "esperaba init `String::from(\"Fitz\")`, fue: {}",
            init
        );
    }

    #[test]
    fn binop_int_int_es_int() {
        let file = ast_test::parse(&gen("let x = 1 + 2").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("1i64") && init.contains("2i64") && init.contains("+"),
            "esperaba init `1i64 + 2i64`, fue: {}",
            init
        );
    }

    #[test]
    fn binop_int_float_coerciona_a_float() {
        let file = ast_test::parse(&gen("let x = 1 + 2.0").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        // El resultado tipa como f64 (Int+Float promueve a Float).
        assert_eq!(ast_test::local_type(l).as_deref(), Some("f64"));
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("1i64") && init.contains("as f64") && init.contains("2f64"),
            "esperaba coerción Int→Float en el init, fue: {}",
            init
        );
    }

    #[test]
    fn str_interp_genera_format_macro() {
        // Para una var Int adentro de StrInterp, generamos `format!`
        // pasando la var directo (no necesita `.clone()`).
        let code = gen("let n = 5\nlet s = \"x es {n}\"").unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let args = ast_test::first_macro_args_in_stmts(stmts, "format")
            .expect("falta format!");
        assert!(
            args.contains("\"x es {}\""),
            "esperaba template `\"x es {{}}\"`, got: {}",
            args
        );
        // n: Int (Copy) pasa directo sin .clone().
        assert!(
            !args.contains("n . clone") && !args.contains("n .clone"),
            "Int no debería usar .clone(), got: {}",
            args
        );
    }

    #[test]
    fn str_interp_con_var_str_clona() {
        // Para Str, generamos `.clone()` porque format! borrowea
        // pero seguimos pasando el `Ident` evaluado, que sí incluye
        // el clone.
        let code = gen("let name = \"Fitz\"\nlet s = \"hola, {name}\"").unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let args = ast_test::first_macro_args_in_stmts(stmts, "format")
            .expect("falta format!");
        assert!(
            args.contains("\"hola, {}\"") && args.contains("name . clone"),
            "esperaba `format!(\"hola, {{}}\", name.clone())`, got: {}",
            args
        );
    }

    #[test]
    fn print_genera_println_macro() {
        let file = ast_test::parse(&gen("let x: Int = 1\nprint(x)").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        assert_eq!(
            ast_test::count_macro_calls(stmts, "println"),
            1,
            "esperaba exactamente 1 println!"
        );
    }

    #[test]
    fn print_multiples_args_genera_format_string_con_espacios() {
        // El contrato es: print(a, b) emite UN solo println! que
        // contiene tanto `a` como `b` (separación por espacio en el
        // format string es el formato canónico, pero acá nos basta con
        // estructura: 1 println, args contienen ambas vars).
        let file = ast_test::parse(
            &gen("let a: Int = 1\nlet b: Int = 2\nprint(a, b)").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        assert_eq!(ast_test::count_macro_calls(stmts, "println"), 1);
        // Inspeccionamos el último stmt (debería ser el println!).
        let last = ast_test::ts(stmts.last().unwrap());
        assert!(
            last.contains("println !") && last.contains(", a") && last.contains(", b"),
            "esperaba println! con args a y b, fue: {}",
            last
        );
    }

    #[test]
    fn print_sin_args_genera_println_vacio() {
        // `print()` sin args → `println!()` sin tokens internos.
        let file = ast_test::parse(&gen("print()").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        assert_eq!(ast_test::count_macro_calls(stmts, "println"), 1);
        let last = ast_test::ts(stmts.last().unwrap());
        // No debe haber `,` adentro del println!() — sin args.
        assert!(
            last.contains("println ! ()") || last.contains("println !()"),
            "esperaba println!() vacío, fue: {}",
            last
        );
    }

    #[test]
    fn fn_top_level_emite_signature_completa() {
        let file = ast_test::parse(
            &gen("fn double(n: Int) -> Int { return n * 2 }").unwrap(),
        );
        let f = ast_test::find_item_fn(&file, "double").expect("falta fn double");
        assert_eq!(ast_test::fn_arity(f), 1);
        assert_eq!(ast_test::fn_param_types(f), vec!["i64".to_string()]);
        assert_eq!(ast_test::fn_return_type(f).as_deref(), Some("i64"));
    }

    #[test]
    fn fn_arrow_emite_return_implicito() {
        // Tanto el body con `{ return n * 2 }` como la flecha
        // `=> n * 2` deben emitir la misma signatura y un `return` en
        // el body. La diferencia con el test anterior es solo sintáctica
        // del lado de Fitz.
        let file = ast_test::parse(
            &gen("fn double(n: Int) -> Int => n * 2").unwrap(),
        );
        let f = ast_test::find_item_fn(&file, "double").expect("falta fn double");
        assert_eq!(ast_test::fn_arity(f), 1);
        assert_eq!(ast_test::fn_return_type(f).as_deref(), Some("i64"));
        // El body debe contener un `return` (no es solo una expresión
        // de tail — el codegen siempre emite `return ...`).
        let body = ast_test::ts(&f.block);
        assert!(
            body.contains("return"),
            "esperaba `return` en el body de la fn flecha, fue: {}",
            body
        );
    }

    #[test]
    fn llamada_a_fn_top_level_resuelve_return_type() {
        let file = ast_test::parse(
            &gen(
                "fn double(n: Int) -> Int => n * 2\n\
                 let x = double(5)",
            )
            .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        // x debe quedar tipado como i64 (el return type de double).
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("double") && init.contains("5i64"),
            "esperaba init `double(5i64)`, fue: {}",
            init
        );
    }

    #[test]
    fn if_else_genera_estructura_rust() {
        let file = ast_test::parse(
            &gen("let x = 1\nif (x > 0) { print(\"pos\") } else { print(\"neg\") }")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let if_expr = ast_test::find_if(stmts).expect("falta if/else en main");
        // Ambas ramas presentes (else_branch != None).
        assert!(if_expr.else_branch.is_some(), "esperaba else branch");
        // El test del if compara `x > 0`.
        let cond = ast_test::ts(&*if_expr.cond);
        assert!(cond.contains("x") && cond.contains(">"), "cond: {}", cond);
    }

    #[test]
    fn while_genera_estructura_rust() {
        let file = ast_test::parse(
            &gen("let n = 0\nwhile (n < 3) { n = n + 1 }").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let w = ast_test::find_while_loop(stmts).expect("falta while loop");
        let cond = ast_test::ts(&*w.cond);
        assert!(
            cond.contains("n") && cond.contains("<") && cond.contains("3i64"),
            "cond del while: {}",
            cond
        );
        // El body debe reasignar `n` (= no es un `let`).
        let body = ast_test::ts(&w.body);
        assert!(body.contains("n ="), "body del while: {}", body);
    }

    #[test]
    fn for_in_range_genera_rust() {
        let file = ast_test::parse(&gen("for i in 0..3 { print(i) }").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let f = ast_test::find_for_loop(stmts).expect("falta for loop");
        // pat es `mut i`.
        assert!(
            ast_test::ts(&f.pat).contains("i"),
            "pat del for: {}",
            ast_test::ts(&f.pat)
        );
        // expr es un range `0..3` (ambos bordes presentes).
        let expr = ast_test::ts(&*f.expr);
        assert!(
            expr.contains("0i64") && expr.contains("3i64") && expr.contains(".."),
            "expr del for: {}",
            expr
        );
    }

    #[test]
    fn reasignacion_usa_igual_no_let() {
        let file = ast_test::parse(&gen("let x = 1\nx = 2").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        // Solo UN `let x` — la reasignación no es un let.
        assert_eq!(
            ast_test::count_lets(stmts, "x"),
            1,
            "esperaba exactamente 1 `let x`, hubo {}",
            ast_test::count_lets(stmts, "x")
        );
        // Y debe haber un Stmt de asignación `x = 2`.
        let body = ast_test::ts(stmts.last().expect("stmts vacío"));
        assert!(
            body.contains("x =") && body.contains("2i64"),
            "esperaba reasignación `x = 2`, último stmt: {}",
            body
        );
    }

    #[test]
    fn neg_genera_unary_rust() {
        let file = ast_test::parse(&gen("let x = -5").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        let init = ast_test::local_init(l).unwrap();
        // Operator unario `-` aplicado al literal 5i64.
        assert!(
            init.contains("-") && init.contains("5i64"),
            "esperaba unary `-5i64`, fue: {}",
            init
        );
    }

    #[test]
    fn bool_y_logicos_generan_bool_rust() {
        let file = ast_test::parse(&gen("let b = true and false").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "b").expect("falta let b");
        assert_eq!(ast_test::local_type(l).as_deref(), Some("bool"));
        let init = ast_test::local_init(l).unwrap();
        // `and` Fitz → `&&` Rust.
        assert!(
            init.contains("&&") && init.contains("true") && init.contains("false"),
            "esperaba `true && false`, fue: {}",
            init
        );
    }

    #[test]
    fn comparacion_str_usa_as_str() {
        // Comparar Strings con `<`/`>` requiere `.as_str()` (Strings no
        // implementan `PartialOrd<&str>` directamente; sí `&str` con `&str`).
        let file = ast_test::parse(
            &gen("let a = \"hola\"\nlet b = a < \"mundo\"").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        assert!(
            ast_test::contains_method_call(stmts, "as_str"),
            "esperaba alguna call a `.as_str()` en la comparación"
        );
    }

    // ---- features fuera de scope generan errores claros ----

    // ---- 5b.2: tipos custom (sí soportados, salvo igualdad) ----

    #[test]
    fn type_def_emite_struct_y_alias_arc_mutex() {
        let file = ast_test::parse(&gen("type User { id: Int, name: Str }").unwrap());
        // El struct UserData con sus dos campos.
        let s = ast_test::find_item_struct(&file, "UserData").expect("falta UserData");
        let field_names: Vec<String> = s
            .fields
            .iter()
            .filter_map(|f| f.ident.as_ref().map(|i| i.to_string()))
            .collect();
        assert_eq!(field_names, vec!["id".to_string(), "name".to_string()]);
        // El alias `type User = Arc<Mutex<UserData>>;`.
        let t = ast_test::find_item_type(&file, "User").expect("falta type alias User");
        let ty = ast_test::ts(&*t.ty);
        assert!(
            ty.contains("Arc") && ty.contains("Mutex") && ty.contains("UserData"),
            "esperaba alias `Arc<Mutex<UserData>>`, fue: {}",
            ty
        );
    }

    #[test]
    fn type_def_emite_impl_display_canonico() {
        let code = gen("type User { id: Int, name: Str }").unwrap();
        let file = ast_test::parse(&code);
        let im = ast_test::find_impl(&file, "Display", "UserData")
            .expect("falta impl Display for UserData");
        let impl_text = ast_test::ts(im);
        // El Display escribe `User { id: <int>, name: "<str>" }` —
        // strings con comillas adentro de la instancia (igual al
        // intérprete).
        assert!(
            impl_text.contains("\"User {{\""),
            "falta el header del Display `\"User {{{{\"`, got:\n{}",
            impl_text
        );
        assert!(
            impl_text.contains("\"\\\"{}\\\"\""),
            "falta el patrón con comillas para Str, got:\n{}",
            impl_text
        );
    }

    #[test]
    fn struct_lit_emite_arc_new_mutex_new() {
        let file = ast_test::parse(
            &gen("type User { id: Int, name: Str }\nlet u = User { id: 1, name: \"x\" }")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "u").expect("falta let u");
        let init = ast_test::local_init(l).unwrap();
        // El struct lit se emite envuelto en `Arc::new(Mutex::new(UserData { ... }))`.
        assert!(
            init.contains("Arc :: new")
                && init.contains("Mutex :: new")
                && init.contains("UserData"),
            "esperaba envoltorio Arc::new(Mutex::new(UserData {{ ... }})), fue: {}",
            init
        );
        assert!(
            init.contains("1i64") && init.contains("\"x\""),
            "esperaba que el init incluya los valores 1 y \"x\", fue: {}",
            init
        );
    }

    #[test]
    fn struct_lit_aplica_default_inline_si_falta_campo() {
        // `active: Bool = true` debe inyectarse cuando no se pasa.
        let code = gen("type C { port: Int, active: Bool = true }\nlet c = C { port: 8080 }")
            .unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "c").expect("falta let c");
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("active : true"),
            "esperaba que el default `active: true` esté inyectado, fue: {}",
            init
        );
    }

    #[test]
    fn struct_lit_nullable_omitido_se_resuelve_como_none() {
        let code = gen("type U { id: Int, email: Str? }\nlet u = U { id: 1 }").unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "u").expect("falta let u");
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("email : None"),
            "esperaba `email: None` (nullable omitido), fue: {}",
            init
        );
    }

    #[test]
    fn struct_lit_valor_str_a_campo_nullable_se_envuelve_en_some() {
        let code = gen("type U { id: Int, email: Str? }\nlet u = U { id: 1, email: \"a@b\" }")
            .unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "u").expect("falta let u");
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("email : Some")
                && init.contains("String :: from")
                && init.contains("\"a@b\""),
            "esperaba `email: Some(String::from(\"a@b\"))`, fue: {}",
            init
        );
    }

    #[test]
    fn struct_lit_null_literal_a_campo_nullable_es_none() {
        let code = gen("type U { id: Int, email: Str? }\nlet u = U { id: 1, email: null }")
            .unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "u").expect("falta let u");
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("email : None"),
            "esperaba `email: None` (null literal), fue: {}",
            init
        );
    }

    #[test]
    fn field_access_int_emite_block_sin_field_clone() {
        // F17.4b: el field access se emite como bloque con guard
        // acotado para evitar holds del Mutex cross-expression.
        // Para Int (Copy) el block return es `__g.id` (no
        // `__g.id.clone()`). `u.clone()` sí aparece (Arc::clone para
        // compartir el handle); lo que NO debe aparecer es
        // `__g.id.clone()` (clone del value Copy es redundante).
        let code = gen("type U { id: Int }\nlet u = U { id: 1 }\nlet n = u.id").unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("let __g") && init.contains("__g . id"),
            "esperaba bloque `{{ let __g = ...; __g.id }}`, fue: {}",
            init
        );
        assert!(
            !init.contains("__g . id . clone"),
            "no se debe clonar Int (Copy), fue: {}",
            init
        );
    }

    #[test]
    fn field_access_str_emite_lock_clone() {
        let file = ast_test::parse(
            &gen("type U { name: Str }\nlet u = U { name: \"x\" }\nlet s = u.name")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "s").expect("falta let s");
        let init = ast_test::local_init(l).unwrap();
        // Str no es Copy → lock + unwrap + clone (post-F17.4b).
        assert!(
            init.contains("lock") && init.contains("clone"),
            "esperaba `lock` y `clone` en el field access de Str, fue: {}",
            init
        );
    }

    #[test]
    fn field_assign_emite_lock() {
        let file = ast_test::parse(
            &gen("type U { name: Str }\nlet u = U { name: \"x\" }\nu.name = \"y\"")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        // Post-F17.4b: el field assign emite `(obj).lock().unwrap().<f> = ...`.
        assert!(
            ast_test::contains_method_call(stmts, "lock"),
            "esperaba call a `.lock().unwrap()` para field assign"
        );
    }

    #[test]
    fn pasar_instance_a_fn_clona_el_rc() {
        // El Ident `u` de tipo Nominal se evalúa como `u.clone()` al
        // pasarlo a `f(u)`. Esto preserva el aliasing del intérprete.
        let code = gen(
            "type U { id: Int }\nfn f(x: U) -> Int => x.id\nlet u = U { id: 1 }\nlet n = f(u)",
        )
        .unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        let init_expr = ast_test::local_init_expr(l).expect("falta init de n");
        let call = match init_expr {
            syn::Expr::Call(c) => c,
            other => panic!(
                "esperaba init como Call, fue: {}",
                ast_test::ts(other)
            ),
        };
        assert_eq!(ast_test::ts(&*call.func), "f", "callee debería ser `f`");
        assert_eq!(call.args.len(), 1, "esperaba 1 arg");
        let arg = call.args.first().unwrap();
        let arg_text = ast_test::ts(arg);
        assert!(
            arg_text.contains("u") && arg_text.contains(". clone"),
            "esperaba arg `u.clone()`, fue: {}",
            arg_text
        );
    }

    #[test]
    fn print_de_instance_usa_show_expr_con_display() {
        // `print(u)` para u: U → format!("{}", &*u.lock().unwrap()) dentro
        // del println!.
        let code = gen("type U { id: Int }\nlet u = U { id: 1 }\nprint(u)").unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let args = ast_test::first_macro_args_in_stmts(stmts, "println")
            .expect("falta println!");
        assert!(
            args.contains("format !")
                && (args.contains("& *") || args.contains("&*"))
                && args.contains(". lock"),
            "esperaba `format!(\"{{}}\", &*(...).lock().unwrap())` en el println!, got: {}",
            args
        );
    }

    #[test]
    fn tipo_anidado_compila_con_nullable_de_nominal() {
        // `type Order { user: User? }` se traduce a un campo de tipo
        // `Option<User>` (= `Option<Arc<Mutex<UserData>>>`).
        let file = ast_test::parse(
            &gen("type User { name: Str }\ntype Order { user: User? }").unwrap(),
        );
        let s = ast_test::find_item_struct(&file, "OrderData").expect("falta OrderData");
        let user_field = s
            .fields
            .iter()
            .find(|f| f.ident.as_ref().is_some_and(|i| i == "user"))
            .expect("falta field user");
        let ty = ast_test::ts(&user_field.ty);
        assert!(
            ty.contains("Option") && ty.contains("User"),
            "esperaba `Option<User>` para field `user`, fue: {}",
            ty
        );
    }

    #[test]
    fn igualdad_estructural_entre_instancias_emite_lock_eq() {
        let code = gen(
            "type U { id: Int }\nlet a = U { id: 1 }\nlet b = U { id: 1 }\nlet eq = a == b",
        )
        .unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "eq").expect("falta let eq");
        let init = ast_test::local_init(l).unwrap();
        // `*a.lock().unwrap() == *b.lock().unwrap()`: ambos lados aplican
        // .lock().unwrap() y se desreferencian antes de comparar.
        let lock_count = init.matches(". lock").count();
        assert!(
            lock_count >= 2,
            "esperaba al menos 2 `.lock().unwrap()` en la comparación, fue {} en: {}",
            lock_count,
            init
        );
        assert!(
            init.contains("=="),
            "esperaba operador `==`, fue: {}",
            init
        );
    }

    // ---- 5b.2+: if como expresión con valor ----

    #[test]
    fn if_como_expresion_emite_branches_sin_punto_y_coma() {
        let code = gen("let x = if (true) { 1 } else { 2 }").unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("i64"),
            "esperaba `x: i64`"
        );
        // El init es un Expr::If (opcionalmente envuelto en paréntesis).
        let init = ast_test::local_init_expr(l).expect("falta init de x");
        let mut e = init;
        while let syn::Expr::Paren(p) = e {
            e = &*p.expr;
        }
        let if_expr = match e {
            syn::Expr::If(i) => i,
            _ => panic!(
                "esperaba Expr::If como init de x, fue: {}",
                ast_test::ts(init)
            ),
        };
        // then y else terminan en tail (Stmt::Expr sin `;`) — clave
        // para que el if devuelva valor.
        let then_tail = if_expr
            .then_branch
            .stmts
            .last()
            .expect("rama then sin stmts");
        assert!(
            matches!(then_tail, syn::Stmt::Expr(_, None)),
            "rama then debe terminar en tail sin `;`, last stmt: {}",
            ast_test::ts(then_tail)
        );
        let else_block = match if_expr.else_branch.as_ref() {
            Some((_, expr)) => match expr.as_ref() {
                syn::Expr::Block(b) => &b.block,
                _ => panic!("else no es Block, fue: {}", ast_test::ts(expr.as_ref())),
            },
            None => panic!("falta rama else"),
        };
        let else_tail = else_block.stmts.last().expect("rama else sin stmts");
        assert!(
            matches!(else_tail, syn::Stmt::Expr(_, None)),
            "rama else debe terminar en tail sin `;`"
        );
        // Los valores 1 y 2 aparecen como literales i64.
        let body = ast_test::ts(if_expr);
        assert!(
            body.contains("1i64") && body.contains("2i64"),
            "esperaba `1i64` y `2i64` en las ramas, fue: {}",
            body
        );
    }

    #[test]
    fn if_expresion_unifica_int_float_a_float() {
        let code = gen("let x = if (true) { 1 } else { 2.5 }").unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("f64"),
            "esperaba `x: f64`"
        );
        let init = ast_test::local_init(l).unwrap();
        // La rama Int se coerciona explícitamente: `1i64 as f64`.
        assert!(
            init.contains("1i64 as f64"),
            "esperaba coerción Int→Float en rama then, fue: {}",
            init
        );
    }

    #[test]
    fn if_como_sentencia_mantiene_comportamiento_anterior() {
        // Sin asignar y con `print` adentro: el if sigue siendo
        // statement; print no se trata como tail expression
        // (no es una expresión con valor en Fitz).
        let code = gen("if (true) { print(\"a\") } else { print(\"b\") }").unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let if_expr = ast_test::find_if(stmts).expect("falta if en main");
        // Las ramas son Block; cada una emite el print como stmt con `;`.
        let then_text = ast_test::ts(&if_expr.then_branch);
        assert!(
            then_text.contains("println !")
                && then_text.contains("\"a\"")
                && then_text.contains(";"),
            "esperaba `println!(...\"a\"...);` en then, fue: {}",
            then_text
        );
        let else_text = match if_expr.else_branch.as_ref() {
            Some((_, expr)) => match expr.as_ref() {
                syn::Expr::Block(b) => ast_test::ts(&b.block),
                _ => panic!("else no es Block"),
            },
            None => panic!("falta else"),
        };
        assert!(
            else_text.contains("println !")
                && else_text.contains("\"b\"")
                && else_text.contains(";"),
            "esperaba `println!(...\"b\"...);` en else, fue: {}",
            else_text
        );
    }

    #[test]
    fn if_sin_else_no_se_trata_como_expresion() {
        // Sin else, no hay segunda rama → no es expresión con valor.
        // El último stmt del then se emite como statement común
        // (con `;` final, no como tail expression).
        let file = ast_test::parse(&gen("if (true) { 1 }").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let if_expr = ast_test::find_if(stmts).expect("falta if");
        assert!(
            if_expr.else_branch.is_none(),
            "el if no debe tener else"
        );
        // El último stmt del then-block debe ser `Stmt::Expr` con `;`
        // (semicolon presente — sería tail expression sin él).
        let last_then = if_expr.then_branch.stmts.last().expect("then vacío");
        match last_then {
            syn::Stmt::Expr(_, semi) => assert!(
                semi.is_some(),
                "esperaba `1i64;` como stmt con semicolon, fue tail expression"
            ),
            other => panic!(
                "esperaba Stmt::Expr al final del then, fue: {}",
                ast_test::ts(other)
            ),
        }
    }

    // ---- 5b.2+: métodos built-in sobre Str ----

    #[test]
    fn str_len_emite_chars_count_as_i64() {
        let file = ast_test::parse(&gen("let s = \"hola\"\nlet n = s.len()").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        // El init combina `.chars()` y `.count()` (no `.len()` directo,
        // que cuenta bytes en lugar de chars).
        assert!(
            ast_test::contains_method_call(stmts, "chars"),
            "esperaba `.chars()` en el init"
        );
        assert!(
            ast_test::contains_method_call(stmts, "count"),
            "esperaba `.count()` en el init"
        );
    }

    #[test]
    fn str_upper_emite_to_uppercase() {
        let file = ast_test::parse(&gen("let s = \"hola\"\nlet u = s.upper()").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        assert!(
            ast_test::contains_method_call(stmts, "to_uppercase"),
            "esperaba call a `.to_uppercase()`"
        );
    }

    #[test]
    fn str_lower_emite_to_lowercase() {
        let file = ast_test::parse(&gen("let s = \"HOLA\"\nlet l = s.lower()").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        assert!(
            ast_test::contains_method_call(stmts, "to_lowercase"),
            "esperaba call a `.to_lowercase()`"
        );
    }

    // (Métodos desconocidos sobre Str los ataja el checker antes de
    // llegar al codegen, así que no testeamos ese path desde acá.)

    #[test]
    fn type_def_emite_derive_clone_y_impl_partialeq() {
        let code = gen("type U { id: Int }").unwrap();
        let file = ast_test::parse(&code);
        let s = ast_test::find_item_struct(&file, "UData").expect("falta UData");
        assert!(
            ast_test::struct_has_derive(s, "Clone"),
            "esperaba derive(Clone)"
        );
        // F17.4b: PartialEq ya no se deriva porque `std::sync::Mutex<T>`
        // no impl PartialEq. Se emite manual abajo, espejando el patrón
        // del intérprete (`Arc::ptr_eq` shortcut + lock+deref por campo
        // nominal).
        assert!(
            !ast_test::struct_has_derive(s, "PartialEq"),
            "PartialEq NO debe derivarse post-F17.4b (Mutex no impl PartialEq)"
        );
        assert!(
            ast_test::find_impl(&file, "PartialEq", "UData").is_some(),
            "esperaba impl PartialEq for UData (manual post-F17.4b)"
        );
    }

    // ---- 5b.3: listas, mapas, indexing, métodos built-in ----

    #[test]
    fn list_literal_emite_arc_mutex_vec() {
        // `[1, 2, 3]` se modela como `Arc<Mutex<Vec<i64>>>`. Los items
        // se coercen al tipo común (acá Int → i64) y se construye con
        // el macro vec![].
        let file = ast_test::parse(&gen("let xs: List<Int> = [1, 2, 3]").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "xs").expect("falta let xs");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < Mutex < Vec < i64 > > >"),
            "tipo declarado de xs"
        );
        let init = ast_test::local_init_expr(l).expect("falta init de xs");
        // El init es `Arc::new(Mutex::new(vec![...]))`. Confirmamos el
        // outer call al path Rc::new y la presencia del macro vec! con
        // los 3 items con sufijo i64.
        assert_eq!(ast_test::call_path(init).as_deref(), Some("Arc :: new"));
        let vec_args = ast_test::find_macro_args(init, "vec")
            .expect("esperaba un macro vec! adentro del init");
        for n in ["1i64", "2i64", "3i64"] {
            assert!(
                vec_args.contains(n),
                "esperaba item {} en vec!, fue: {}",
                n,
                vec_args
            );
        }
    }

    #[test]
    fn list_literal_homogeneo_int_float_promueve_a_float() {
        // Int+Float en la misma lista → `List<Float>` (mismo lub que
        // if-expression y FnExpr ret).
        let file = ast_test::parse(&gen("let xs = [1, 2.5, 3]").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "xs").expect("falta let xs");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < Mutex < Vec < f64 > > >"),
            "esperaba que xs quede tipado List<Float>"
        );
        let init = ast_test::local_init_expr(l).unwrap();
        let vec_args = ast_test::find_macro_args(init, "vec")
            .expect("esperaba un macro vec! adentro del init");
        // Los Int (1, 3) se coercen a f64 con `(N as f64)`; el Float
        // queda literal.
        assert!(
            vec_args.contains("(1i64 as f64)") && vec_args.contains("(3i64 as f64)"),
            "esperaba coerción Int→Float en los items Int, fue: {}",
            vec_args
        );
    }

    #[test]
    fn list_literal_vacia_es_list_any_a_resolver_por_contexto() {
        // `[]` sin contexto da `List<Any>`. Con anotación, el contexto
        // restringe a List<T> y el `Vec::new()` infiere desde el target.
        let file = ast_test::parse(&gen("let xs: List<Int> = []").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "xs").expect("falta let xs");
        assert!(ast_test::local_is_mut(l), "esperaba `let mut`");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < Mutex < Vec < i64 > > >"),
            "esperaba `List<Int>` por anotación"
        );
        // El init es `Arc::new(Mutex::new(Vec::new()))` — verifico
        // que ningún macro vec! aparezca y que la chain Vec::new exista.
        let init = ast_test::local_init_expr(l).unwrap();
        assert!(
            ast_test::find_macro_args(init, "vec").is_none(),
            "lista vacía no debería emitir macro vec!"
        );
        assert!(
            ast_test::ts(init).contains("Vec :: new"),
            "esperaba `Vec::new()` para lista vacía, fue: {}",
            ast_test::ts(init)
        );
    }

    #[test]
    fn list_literal_heterogeneo_es_error_homogeneo_requerido() {
        // Sin posibilidad de unificar (Int + Str), el codegen aborta
        // con mensaje claro mencionando la heterogeneidad.
        assert_err_contains(
            "let xs = [1, \"dos\"]",
            &["homogénea"],
        );
    }

    #[test]
    fn map_literal_emite_vec_pares() {
        // `{"a": 1, "b": 2}` se modela como
        // `Arc<Mutex<Vec<(String, i64)>>>` con tuplas como items.
        let file = ast_test::parse(
            &gen("let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "m").expect("falta let m");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < Mutex < Vec < (String , i64) > > >"),
            "tipo declarado de m"
        );
        let init = ast_test::local_init_expr(l).unwrap();
        let vec_args = ast_test::find_macro_args(init, "vec")
            .expect("esperaba un macro vec! con los pares");
        // Verifico que ambas tuplas (clave, valor) aparezcan.
        for pair in [
            "(String :: from (\"a\") , 1i64)",
            "(String :: from (\"b\") , 2i64)",
        ] {
            assert!(
                vec_args.contains(pair),
                "esperaba par {} en vec!, fue: {}",
                pair,
                vec_args
            );
        }
    }

    #[test]
    fn map_literal_vacio_resuelto_por_anotacion() {
        let file = ast_test::parse(&gen("let m: Map<Str, Int> = {}").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "m").expect("falta let m");
        assert!(ast_test::local_is_mut(l));
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < Mutex < Vec < (String , i64) > > >"),
            "esperaba `Map<Str, Int>` por anotación"
        );
        // Mapa vacío → sin macro vec!.
        let init = ast_test::local_init_expr(l).unwrap();
        assert!(
            ast_test::find_macro_args(init, "vec").is_none(),
            "mapa vacío no debería emitir macro vec!"
        );
    }

    #[test]
    fn map_literal_valores_heterogeneos_es_error() {
        assert_err_contains(
            "let m = {\"a\": 1, \"b\": \"x\"}",
            &["homogéneos"],
        );
    }

    #[test]
    fn list_indexing_emite_borrow_clone() {
        // `xs[0]` → `(xs.clone()).lock().unwrap()[(0i64) as usize].clone()`.
        // El `.clone()` final es del Rc para Nominal/List/Map o copy
        // para primitivos — siempre seguro.
        let file = ast_test::parse(
            &gen("let xs: List<Int> = [10, 20]\nlet x = xs[0]").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        // El binding `x` debe quedar tipado i64 (List<Int> indexing).
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        // El init debe contener el pipeline borrow + index + clone.
        let init = ast_test::local_init_expr(l).unwrap();
        assert!(
            ast_test::contains_method_call_in_expr(init, "lock"),
            "esperaba .lock().unwrap() en el init de x, fue: {}",
            ast_test::ts(init)
        );
        assert!(
            ast_test::contains_method_call_in_expr(init, "clone"),
            "esperaba .clone() en el init de x, fue: {}",
            ast_test::ts(init)
        );
        // El subscript `[(0i64) as usize]` se preserva tokenizado.
        assert!(
            ast_test::ts(init).contains("(0i64) as usize"),
            "esperaba subscript `[(0i64) as usize]`, fue: {}",
            ast_test::ts(init)
        );
    }

    #[test]
    fn map_indexing_emite_busqueda_lineal_con_panic() {
        // `m["a"]` → bloque que linea la búsqueda y paniquea si falta.
        let file = ast_test::parse(
            &gen("let m: Map<Str, Int> = {\"a\": 1}\nlet n = m[\"a\"]").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        let init = ast_test::local_init_expr(l).unwrap();
        // El pipeline `.iter().find(...).unwrap_or_else(|| panic!(...))`
        // está adentro del init: chequeo presencia de los métodos clave.
        assert!(
            ast_test::contains_method_call_in_expr(init, "find"),
            "esperaba `.find(...)` en el init de n, fue: {}",
            ast_test::ts(init)
        );
        // El panic con el mensaje del intérprete sigue como string
        // (es un contrato bit-a-bit con el evaluator).
        let panic_args = ast_test::find_macro_args(init, "panic")
            .expect("esperaba un panic! adentro del bloque");
        assert!(
            panic_args.contains("clave no encontrada en mapa"),
            "esperaba mensaje del intérprete en panic!, fue: {}",
            panic_args
        );
    }

    #[test]
    fn for_sobre_list_genera_snapshot_iter() {
        // `for v in xs` → snapshot via `lock().unwrap().clone().into_iter()`
        // (evita re-entrancia si el body muta `xs`). Post-F17.4b
        // el lock reemplaza al borrow del RefCell viejo.
        let file = ast_test::parse(
            &gen("let xs: List<Int> = [1, 2, 3]\nfor v in xs { print(v) }").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let fl = ast_test::find_for_loop(stmts).expect("falta el for loop");
        // El iterable es una method chain: receptor → lock → unwrap →
        // clone → into_iter. La inspección estructural ignora
        // paréntesis y formato.
        let chain = ast_test::method_chain_names(&fl.expr);
        assert!(
            chain.windows(3).any(|w| w == ["unwrap", "clone", "into_iter"]),
            "esperaba chain `lock().unwrap().clone().into_iter()` en el for, fue: {:?}",
            chain
        );
    }

    #[test]
    fn for_sobre_list_de_any_es_error() {
        assert_err_contains(
            "let xs = []\nfor v in xs { print(v) }",
            &["List<Any>"],
        );
    }

    #[test]
    fn list_push_emite_lock_push() {
        let file = ast_test::parse(&gen("let xs: List<Int> = []\nxs.push(7)").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        // `.push(...)` se emite como method call sobre `lock().unwrap()`.
        assert!(
            ast_test::contains_method_call(stmts, "lock"),
            "esperaba `lock` antes del push"
        );
        assert!(
            ast_test::contains_method_call(stmts, "push"),
            "esperaba `.push(...)`"
        );
    }

    #[test]
    fn list_pop_emite_lock_pop_con_expect() {
        let file = ast_test::parse(&gen("let xs: List<Int> = [1]\nlet x = xs.pop()").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        let init = ast_test::local_init(l).unwrap();
        // El pop se traduce a `.lock().unwrap().pop().expect("...")` —
        // `.expect(...)` paniquea con el mismo mensaje del intérprete.
        assert!(
            init.contains("lock") && init.contains("pop") && init.contains("expect"),
            "esperaba pipeline lock + pop + expect, fue: {}",
            init
        );
        // El mensaje del expect debe mencionar `pop` y `lista vacía`
        // (matchea el del intérprete).
        assert!(
            init.contains("pop") && init.contains("vac"),
            "esperaba mensaje del expect sobre lista vacía, fue: {}",
            init
        );
    }

    #[test]
    fn list_len_metodo_emite_lock_len_as_i64() {
        let file = ast_test::parse(&gen("let xs: List<Int> = []\nlet n = xs.len()").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        // El binding `n` debe quedar tipado como i64.
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("lock") && init.contains("len") && init.contains("as i64"),
            "esperaba pipeline lock + len + as i64, fue: {}",
            init
        );
    }

    #[test]
    fn len_builtin_global_sobre_list_resuelve_a_lock_len() {
        // `len(xs)` despacha por tipo del argumento — mismo código que
        // `xs.len()` para List/Map; para Str sigue siendo chars().count.
        let file = ast_test::parse(
            &gen("let xs: List<Int> = [1]\nlet n = len(xs)").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        let init = ast_test::local_init_expr(l).unwrap();
        // El init es `(<chain> as i64)` con la chain `xs.clone()
        // .lock().unwrap().len()`. Verifico el cast y los métodos.
        assert_eq!(
            ast_test::cast_target_type(init).as_deref(),
            Some("i64"),
            "esperaba cast final `as i64`"
        );
        let chain = ast_test::method_chain_names(init);
        assert!(
            chain.contains(&"lock".to_string()) && chain.contains(&"len".to_string()),
            "esperaba chain con lock + len, fue: {:?}",
            chain
        );
    }

    #[test]
    fn len_builtin_global_sobre_str_usa_chars_count() {
        let file = ast_test::parse(&gen("let s = \"hola\"\nlet n = len(s)").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        let init = ast_test::local_init_expr(l).unwrap();
        // Para Str el builtin global emite `(s.chars().count() as i64)`.
        assert_eq!(
            ast_test::cast_target_type(init).as_deref(),
            Some("i64"),
            "esperaba cast final `as i64`"
        );
        let chain = ast_test::method_chain_names(init);
        assert!(
            chain.contains(&"chars".to_string()) && chain.contains(&"count".to_string()),
            "esperaba chain con chars + count, fue: {:?}",
            chain
        );
    }

    #[test]
    fn list_map_con_fnexpr_inline_emite_closure() {
        let file = ast_test::parse(
            &gen("let xs: List<Int> = [1, 2, 3]\nlet ys = xs.map(fn(x) => x * 2)")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "ys").expect("falta let ys");
        assert!(ast_test::local_is_mut(l));
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < Mutex < Vec < i64 > > >"),
            "esperaba que `ys` quede tipado List<Int>"
        );
        // El init invoca `.map(|x: i64| -> i64 { ... })` adentro de un
        // Arc::new(Mutex::new(...)). Chequeo estructural: hay un
        // method call `map` y el primer arg del map es una closure
        // tipada `|x: i64| -> i64`.
        let init = ast_test::local_init_expr(l).unwrap();
        assert!(
            ast_test::contains_method_call_in_expr(init, "map"),
            "esperaba `.map(...)` en el init, fue: {}",
            ast_test::ts(init)
        );
        let init_text = ast_test::ts(init);
        assert!(
            init_text.contains("| x : i64 | -> i64"),
            "esperaba closure `|x: i64| -> i64`, fue: {}",
            init_text
        );
        assert!(
            init_text.contains("Arc :: new (Mutex :: new"),
            "esperaba envoltorio Arc::new(Mutex::new(...)), fue: {}",
            init_text
        );
    }

    #[test]
    fn list_filter_con_fnexpr_inline_emite_for_manual() {
        // Filter usa un for manual (no .filter()) porque el callback
        // toma T por valor pero `Iterator::filter` quiere &T.
        let file = ast_test::parse(
            &gen("let xs: List<Int> = [1, 2, 3]\nlet ys = xs.filter(fn(x) => x > 1)")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "ys").expect("falta let ys");
        let init = ast_test::local_init_expr(l).unwrap();
        let init_text = ast_test::ts(init);
        // El bloque del init debe declarar un closure `__cb` tipado
        // `|x: i64| -> bool`.
        assert!(
            init_text.contains("let __cb = | x : i64 | -> bool"),
            "esperaba binding del callback `__cb`, fue: {}",
            init_text
        );
        // El callback se aplica adentro de un for con clone del item.
        assert!(
            init_text.contains("__cb (__it . clone ())"),
            "esperaba aplicación `__cb(__it.clone())` adentro del for, fue: {}",
            init_text
        );
    }

    #[test]
    fn map_method_chaining_funciona() {
        // `xs.map(f).map(g)` debe poder componerse. El test es de
        // estructura: el tipo de salida del primer map alimenta al
        // siguiente sin friction.
        let file = ast_test::parse(
            &gen(
                "let xs: List<Int> = [1, 2]\n\
                 let ys = xs.map(fn(x) => x * 2).map(fn(x) => x + 1)",
            )
            .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "ys").expect("falta let ys");
        let init = ast_test::local_init_expr(l).unwrap();
        // Esperamos dos closures `|x: i64| -> i64` (uno por cada .map)
        // adentro del init de ys.
        let init_text = ast_test::ts(init);
        let n = init_text.matches("| x : i64 | -> i64").count();
        assert!(
            n >= 2,
            "esperaba ≥2 closures `|x: i64| -> i64` en chain, fue {} en: {}",
            n,
            init_text
        );
    }

    #[test]
    fn map_has_emite_iter_any() {
        // `m.has(k)` se traduce a un bloque
        // `{ let __k = k; (m.clone()).lock().unwrap().iter().any(...) }`. El
        // chequeo estructural busca `iter` + `any` adentro del init,
        // sin importar si están envueltos en bloque o expresión.
        let file = ast_test::parse(
            &gen("let m: Map<Str, Int> = {\"a\": 1}\nlet b = m.has(\"a\")").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "b").expect("falta let b");
        let init = ast_test::local_init_expr(l).unwrap();
        assert!(
            ast_test::contains_method_call_in_expr(init, "iter"),
            "esperaba `.iter()` en el init de b, fue: {}",
            ast_test::ts(init)
        );
        assert!(
            ast_test::contains_method_call_in_expr(init, "any"),
            "esperaba `.any(...)` en el init de b, fue: {}",
            ast_test::ts(init)
        );
        // Tipo del binding `b` es bool.
        assert_eq!(ast_test::local_type(l).as_deref(), Some("bool"));
    }

    #[test]
    fn map_keys_emite_lista_nueva_de_claves() {
        let file = ast_test::parse(
            &gen("let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}\nlet ks = m.keys()")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "ks").expect("falta let ks");
        // keys() retorna `List<Str>` envuelto en Rc/RefCell.
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < Mutex < Vec < String > > >"),
            "esperaba que keys retorne List<Str>"
        );
        let init = ast_test::local_init_expr(l).unwrap();
        // El pipeline interno usa `.iter().map(...).collect()`.
        let init_text = ast_test::ts(init);
        assert!(
            init_text.contains(". iter () . map (| (__k , _) | __k . clone ())"),
            "esperaba pipeline `.iter().map(|(__k, _)| __k.clone())`, fue: {}",
            init_text
        );
        assert!(
            init_text.contains("collect :: < Vec < _ > > ()"),
            "esperaba `.collect::<Vec<_>>()`, fue: {}",
            init_text
        );
    }

    #[test]
    fn map_values_emite_lista_nueva_de_valores() {
        let file = ast_test::parse(
            &gen("let m: Map<Str, Int> = {\"a\": 1}\nlet vs = m.values()").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "vs").expect("falta let vs");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < Mutex < Vec < i64 > > >"),
            "esperaba que values retorne List<Int>"
        );
        let init = ast_test::local_init_expr(l).unwrap();
        let init_text = ast_test::ts(init);
        assert!(
            init_text.contains(". iter () . map (| (_ , __v) | __v . clone ())"),
            "esperaba pipeline `.iter().map(|(_, __v)| __v.clone())`, fue: {}",
            init_text
        );
        assert!(
            init_text.contains("collect :: < Vec < _ > > ()"),
            "esperaba `.collect::<Vec<_>>()`, fue: {}",
            init_text
        );
    }

    #[test]
    fn map_len_metodo_emite_borrow_len_as_i64() {
        let file = ast_test::parse(
            &gen("let m: Map<Str, Int> = {\"a\": 1}\nlet n = m.len()").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        let init = ast_test::local_init_expr(l).unwrap();
        assert_eq!(
            ast_test::cast_target_type(init).as_deref(),
            Some("i64"),
            "esperaba cast final `as i64`"
        );
        let chain = ast_test::method_chain_names(init);
        assert!(
            chain.contains(&"lock".to_string()) && chain.contains(&"len".to_string()),
            "esperaba chain con lock + len, fue: {:?}",
            chain
        );
    }

    #[test]
    fn list_find_emite_result_con_loop() {
        // 5b.4: find devuelve `Result<T, String>` con Ok(item) al primer
        // match y `Err("no encontrado")` si nada matchea. Tipado del
        // binding `x` debe ser `Result<i64, String>`.
        let file = ast_test::parse(
            &gen("let xs: List<Int> = [1, 2]\nlet x = xs.find(fn(n) => n > 0)").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Result < i64 , String >"),
            "esperaba `x: Result<i64, String>`"
        );
        // El init es un bloque con un loop manual; chequeo presencia de
        // las piezas clave (el mensaje del Err y la asignación de Ok)
        // como sub-strings de la representación normalizada — son
        // contratos del codegen estables.
        let init = ast_test::local_init_expr(l).unwrap();
        let init_text = ast_test::ts(init);
        assert!(
            init_text.contains("Err (String :: from (\"no encontrado\"))"),
            "esperaba inicializador con `Err(\"no encontrado\")`, fue: {}",
            init_text
        );
        assert!(
            init_text.contains("__result = Ok (__it) ; break ;"),
            "esperaba asignación `__result = Ok(__it); break;`, fue: {}",
            init_text
        );
    }

    #[test]
    fn map_get_emite_result_con_busqueda_lineal() {
        // 5b.4: get devuelve `Result<V, String>`. Mensaje del Err matchea
        // bit-a-bit el del intérprete: `clave no encontrada: <k>` con `<k>`
        // formateado inline (Str con comillas).
        let file = ast_test::parse(
            &gen("let m: Map<Str, Int> = {\"a\": 1}\nlet v = m.get(\"a\")").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "v").expect("falta let v");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Result < i64 , String >"),
            "esperaba `v: Result<i64, String>`"
        );
        let init = ast_test::local_init_expr(l).unwrap();
        // El mensaje del Err lleva el template `clave no encontrada: {}`
        // (contrato bit-a-bit con el intérprete) — lo busco adentro de
        // un format! macro call.
        let fmt = ast_test::find_macro_args(init, "format")
            .expect("esperaba un format! con el mensaje del Err");
        assert!(
            fmt.contains("clave no encontrada: {}"),
            "esperaba template `clave no encontrada: {{}}` en format!, fue: {}",
            fmt
        );
        let init_text = ast_test::ts(init);
        assert!(
            init_text.contains("__result = Ok (__v . clone ()) ; break ;"),
            "esperaba asignación `__result = Ok(__v.clone()); break;`, fue: {}",
            init_text
        );
    }

    #[test]
    fn fnexpr_suelta_emite_arc_dyn_fn() {
        // F12: FnExpr asignado a var emite `Arc::new(move |...| ...) as
        // Arc<dyn Fn(...) -> ...>`. La var queda tipada como
        // `Arc<dyn Fn(i64) -> i64>` y se puede invocar con `f(x)`.
        let file = ast_test::parse(
            &gen("let f: Fn(Int) -> Int = fn(x: Int) => x * 2\nprint(f(3))").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "f").expect("falta let f");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < dyn Fn (i64) -> i64 + Send + Sync >"),
            "esperaba tipo `Arc<dyn Fn(i64) -> i64>`"
        );
        // El init debe contener `Arc::new(move |x: i64| ...)`. Verifico
        // sub-string sobre la representación normalizada.
        let init_text = ast_test::ts(ast_test::local_init_expr(l).unwrap());
        assert!(
            init_text.contains("Arc :: new (move | x : i64 |"),
            "esperaba `Arc::new(move |x: i64| ...)`, fue: {}",
            init_text
        );
    }

    #[test]
    fn fnexpr_sin_anotacion_de_param_da_error_claro() {
        // F12: el subset compilable exige anotación en cada param del
        // FnExpr (deuda 5b.1). Sin anotación → mensaje explícito.
        assert_err_contains(
            "let f: Fn(Int) -> Int = fn(x) => x * 2",
            &["anónima", "anotación de tipo"],
        );
    }

    #[test]
    fn fn_nombrada_como_valor_emite_arc_new() {
        // F12: `let g = square` donde `square` es fn top-level emite
        // `Arc::new(square) as Arc<dyn Fn(...) -> R>` con la firma del
        // fn_sigs.
        let file = ast_test::parse(
            &gen("fn square(n: Int) -> Int => n * n\nlet g: Fn(Int) -> Int = square\nprint(g(7))")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "g").expect("falta let g");
        assert_eq!(
            ast_test::local_type(l).as_deref(),
            Some("Arc < dyn Fn (i64) -> i64 + Send + Sync >"),
            "esperaba tipo `Arc<dyn Fn(i64) -> i64>`"
        );
        let init_text = ast_test::ts(ast_test::local_init_expr(l).unwrap());
        assert!(
            init_text.contains("Arc :: new (square)"),
            "esperaba `Arc::new(square)`, fue: {}",
            init_text
        );
    }

    #[test]
    fn fn_param_de_tipo_funcion_emite_arc_dyn_fn() {
        // F12: param `f: Fn(Int) -> Int` en la firma de la fn top-level
        // debe traducirse a `Arc<dyn Fn(i64) -> i64>` en el header.
        let file = ast_test::parse(
            &gen(
                "fn apply(f: Fn(Int) -> Int, x: Int) -> Int => f(x)\n\
                 fn square(n: Int) -> Int => n * n\n\
                 print(apply(square, 7))",
            )
            .unwrap(),
        );
        // Header de `apply`: tipos de params + return type.
        let apply = ast_test::find_item_fn(&file, "apply").expect("falta fn apply");
        assert_eq!(
            ast_test::fn_param_types(apply),
            vec!["Arc < dyn Fn (i64) -> i64 + Send + Sync >", "i64"],
            "tipos de params de apply"
        );
        assert_eq!(
            ast_test::fn_return_type(apply).as_deref(),
            Some("i64"),
            "return type de apply"
        );
        // La llamada `apply(square, 7)` debe envolver `square` en
        // `Arc::new(square)`. Lo busco como sub-string sobre el `main`
        // tokenizado (la llamada vive adentro del print).
        let main_text = ast_test::ts(
            ast_test::find_item_fn(&file, "main").expect("falta fn main"),
        );
        assert!(
            main_text.contains("apply ((Arc :: new (square)"),
            "esperaba `apply((Arc::new(square) as ...))` en main, fue: {}",
            main_text
        );
    }

    #[test]
    fn fn_como_return_type_emite_arc_dyn_fn() {
        // F12: `-> Fn(Int) -> Int` en una fn top-level emite el header
        // con retorno `Arc<dyn Fn(i64) -> i64>`. La closure interna que
        // captura `x` se traduce con `move`.
        let file = ast_test::parse(
            &gen(
                "fn make_adder(x: Int) -> Fn(Int) -> Int {\n\
                     return fn(y: Int) => x + y\n\
                 }\n\
                 let add5: Fn(Int) -> Int = make_adder(5)\n\
                 print(add5(3))",
            )
            .unwrap(),
        );
        let make_adder = ast_test::find_item_fn(&file, "make_adder")
            .expect("falta fn make_adder");
        assert_eq!(
            ast_test::fn_param_types(make_adder),
            vec!["i64"],
            "tipos de params de make_adder"
        );
        assert_eq!(
            ast_test::fn_return_type(make_adder).as_deref(),
            Some("Arc < dyn Fn (i64) -> i64 + Send + Sync >"),
            "return type de make_adder"
        );
        // El body de make_adder contiene `Arc::new(move |y: i64| ...)`.
        let body_text = ast_test::ts(&make_adder.block);
        assert!(
            body_text.contains("Arc :: new (move | y : i64 |"),
            "esperaba closure con `move` capturando x, fue: {}",
            body_text
        );
    }

    #[test]
    fn closure_que_captura_var_no_copy_clona_afuera() {
        // F12: closure que captura una var no-Copy (Str). El codegen
        // debe emitir `let saludo = saludo.clone();` afuera para
        // preservar el aliasing semántico sin consumir la var del
        // caller.
        let file = ast_test::parse(
            &gen(
                "let saludo = \"hola\"\n\
                 let f: Fn(Str) -> Str = fn(n: Str) => \"{saludo}, {n}!\"\n\
                 print(f(\"Fitz\"))",
            )
            .unwrap(),
        );
        // El clone afuera y el closure interno viven adentro del init
        // de `f` (es un bloque Rust). Tomo el init y verifico sub-strings
        // estructurales.
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "f").expect("falta let f");
        let init_text = ast_test::ts(ast_test::local_init_expr(l).unwrap());
        assert!(
            init_text.contains("let saludo = saludo . clone () ;"),
            "esperaba clone de la captura antes del Rc::new, fue: {}",
            init_text
        );
        assert!(
            init_text.contains("Arc :: new (move | n : String |"),
            "esperaba closure `Arc::new(move |n: String| ...)`, fue: {}",
            init_text
        );
    }

    #[test]
    fn var_de_tipo_funcion_se_llama_con_parens() {
        // F12: `f(x)` sobre una var Fn(Int) -> Int se traduce literal a
        // `f(x)` Rust — el auto-deref de `Rc<dyn Fn>` lo resuelve.
        let file = ast_test::parse(
            &gen("let f: Fn(Int) -> Int = fn(n: Int) => n + 1\nprint(f(10))").unwrap(),
        );
        // El cuerpo del main debe tener una llamada `f(10i64)` adentro
        // del println!. Lo busco sobre la representación tokenizada
        // (es un macro call, el argumento entero se ve serializado).
        let main_text = ast_test::ts(
            ast_test::find_item_fn(&file, "main").expect("falta fn main"),
        );
        assert!(
            main_text.contains("f (10i64)"),
            "esperaba `f(10i64)` en main, fue: {}",
            main_text
        );
    }

    #[test]
    fn fn_anonima_inline_como_arg_emite_closure_directo() {
        // F12: `apply(fn(n: Int) => n * 10, 7)` no envuelve en una var
        // intermedia — emite el `Arc::new(move |n: i64| ...)` inline
        // como argumento.
        let file = ast_test::parse(
            &gen(
                "fn apply(f: Fn(Int) -> Int, x: Int) -> Int => f(x)\n\
                 print(apply(fn(n: Int) => n * 10, 7))",
            )
            .unwrap(),
        );
        let main_text = ast_test::ts(
            ast_test::find_item_fn(&file, "main").expect("falta fn main"),
        );
        assert!(
            main_text.contains("apply ((Arc :: new (move | n : i64 |"),
            "esperaba el FnExpr emitido inline como arg de apply, fue: {}",
            main_text
        );
    }

    #[test]
    fn print_de_lista_emite_iter_inline() {
        // El print/interp construye el string `[a, b, c]` en runtime
        // ligando primero el Rc a una var (vida del temporal).
        let file = ast_test::parse(&gen("let xs: List<Int> = [1, 2]\nprint(xs)").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        // Debe haber un `println!` (es el print de la lista).
        assert!(
            ast_test::count_macro_calls(stmts, "println") >= 1,
            "esperaba al menos un println! para imprimir xs"
        );
        // El bloque inline liga el Rc a `__list` antes del borrow (vida
        // del temporal). Lo verifico chequeando que el código total
        // contiene un `let __list` adentro de algún bloque.
        let code = ast_test::ts(&file);
        assert!(
            code.contains("let __list ="),
            "esperaba binding `let __list = ...` adentro del print, fue:\n{}",
            code
        );
        // El header `[` se emite como literal Str adentro del format.
        assert!(
            code.contains("String :: from (\"[\")"),
            "esperaba `String::from(\"[\")` como header de lista, fue:\n{}",
            code
        );
    }

    #[test]
    fn print_de_mapa_emite_iter_inline_con_llaves() {
        let file = ast_test::parse(
            &gen("let m: Map<Str, Int> = {\"a\": 1}\nprint(m)").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        assert!(
            ast_test::count_macro_calls(stmts, "println") >= 1,
            "esperaba al menos un println! para imprimir m"
        );
        let code = ast_test::ts(&file);
        assert!(
            code.contains("let __map ="),
            "esperaba binding `let __map = ...` adentro del print, fue:\n{}",
            code
        );
        assert!(
            code.contains("String :: from (\"{\")"),
            "esperaba `String::from(\"{{\")` como header de mapa, fue:\n{}",
            code
        );
    }

    // (El test viejo `match_no_soportado` se reemplazó por los tests
    // de match en 5b.4 más abajo.)

    // (El test viejo `imports_no_soportados` se reemplazó en 5b.5;
    // los imports ahora se soportan. Para feature no soportada en
    // codegen single-file, ver `http_decoradores_no_soportados` que
    // sigue apuntando a 5b.6.)

    // (El test viejo `http_decoradores_no_soportados` se reemplazó en
    // 5b.6 por los tests específicos de HTTP más abajo.)

    // ---- 5b.6: HTTP / @server / handlers --------------------------------


    #[test]
    fn http_main_emite_tokio_main_async() {
        // F17.4b: tokio runtime default = `multi_thread` (N workers según
        // cores), paralelismo HTTP real. F11 originalmente lo dejaba en
        // `current_thread` por el `thread_local!` del state; F17.4b lo
        // migró a `LazyLock<Arc<Mutex<T>>>` y destrabó multi-thread.
        let src = "@server(3000) fn main() => 0\n\
                   @get(\"/\") fn index() -> Str => \"ok\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        let main = ast_test::find_item_fn(&file, "main").expect("falta fn main");
        assert!(ast_test::fn_is_async(main), "fn main debería ser async");
        let attrs = ast_test::fn_attrs(main);
        assert!(
            attrs.iter().any(|a| a.contains("tokio :: main")),
            "esperaba #[tokio::main] en fn main, attrs: {:?}",
            attrs
        );
        // El flavor `current_thread` NO debe aparecer (F17.4b switcheó
        // a multi-thread, que es el default — sin override explícito).
        assert!(
            !attrs.iter().any(|a| a.contains("current_thread")),
            "no esperaba `current_thread` en attrs, fue: {:?}",
            attrs
        );
        let body = ast_test::fn_body_text(main);
        assert!(
            body.contains("axum :: Router :: new"),
            "esperaba `axum::Router::new()` en fn main body"
        );
    }

    #[test]
    fn http_router_registra_ruta_get() {
        let src = "@get(\"/users\") fn list_users() -> Str => \"[]\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        let handler = ast_test::find_item_fn(&file, "__handler_list_users")
            .expect("falta async fn __handler_list_users");
        assert!(
            ast_test::fn_is_async(handler),
            "__handler_list_users debería ser async"
        );
        let routes = ast_test::find_route_registrations(&file);
        let users = routes
            .iter()
            .find(|(p, _)| p.contains("/users"))
            .unwrap_or_else(|| panic!("esperaba route /users, got: {:?}", routes));
        assert!(
            users.1.contains("axum :: routing :: get") && users.1.contains("__handler_list_users"),
            "esperaba `axum::routing::get(__handler_list_users)`, got: {}",
            users.1
        );
    }

    #[test]
    fn http_path_param_int_genera_extract_path() {
        let src = "@get(\"/u/{id}\") fn get_user(id: Int) -> Str => \"x\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        let handler = ast_test::find_item_fn(&file, "__handler_get_user")
            .expect("falta __handler_get_user");
        let pats_tys = ast_test::fn_param_pats_and_types(handler);
        assert!(
            pats_tys.iter().any(|(p, t)| {
                p.contains("axum :: extract :: Path")
                    && p.contains("id")
                    && t.contains("axum :: extract :: Path")
                    && t.contains("i64")
            }),
            "esperaba param `axum::extract::Path(id): axum::extract::Path<i64>` en __handler_get_user, got: {:?}",
            pats_tys
        );
    }

    #[test]
    fn http_path_param_str_genera_extract_path_string() {
        let src = "@get(\"/u/{name}\") fn greet(name: Str) -> Str => name";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        let handler =
            ast_test::find_item_fn(&file, "__handler_greet").expect("falta __handler_greet");
        let pats_tys = ast_test::fn_param_pats_and_types(handler);
        assert!(
            pats_tys.iter().any(|(p, t)| {
                p.contains("axum :: extract :: Path")
                    && p.contains("name")
                    && t.contains("axum :: extract :: Path")
                    && t.contains("String")
            }),
            "esperaba param `axum::extract::Path(name): axum::extract::Path<String>` en __handler_greet, got: {:?}",
            pats_tys
        );
    }

    #[test]
    fn http_handler_result_emite_match_ok_err() {
        let src = "@get(\"/d/{n}\") fn divide(n: Int) -> Result<Int> { return Ok(n * 2) }";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let wrapper = ast_test::find_item_fn(&file, "__handler_divide")
            .expect("falta __handler_divide");
        assert!(
            ast_test::fn_body_has_match_arm_pat(wrapper, "Ok (__v)")
                || ast_test::fn_body_has_match_arm_pat(wrapper, "Ok(__v)"),
            "esperaba arm `Ok(__v)` en el wrapper, body:\n{}",
            ast_test::fn_body_text(wrapper)
        );
        assert!(
            ast_test::fn_body_has_match_arm_pat(wrapper, "Err (__e)")
                || ast_test::fn_body_has_match_arm_pat(wrapper, "Err(__e)"),
            "esperaba arm `Err(__e)` en el wrapper, body:\n{}",
            ast_test::fn_body_text(wrapper)
        );
        let body = ast_test::fn_body_text(wrapper);
        assert!(
            body.contains("StatusCode :: OK"),
            "esperaba `StatusCode::OK` (200), got:\n{}",
            body
        );
        assert!(
            body.contains("StatusCode :: INTERNAL_SERVER_ERROR"),
            "esperaba `StatusCode::INTERNAL_SERVER_ERROR` (500), got:\n{}",
            body
        );
    }

    // ---- Status codes custom (return <int> { ... }) ----

    #[test]
    fn status_codes_handler_con_return_status_emite_fitz_response() {
        // El handler `protected` tiene `return 401 { ... }` adentro,
        // así que su return type Rust se vuelve `__FitzResponse` y el
        // body del return se envuelve con `to_fitz_json`.
        let src = "@get(\"/p\") fn protected() -> Str {\n\
                       return 401 {\"msg\": \"no autorizado\"}\n\
                   }";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let protected =
            ast_test::find_item_fn(&file, "protected").expect("falta fn protected");
        assert_eq!(
            ast_test::fn_return_type(protected).as_deref(),
            Some("__FitzResponse"),
            "esperaba que protected retorne __FitzResponse"
        );
        assert!(
            ast_test::fn_body_returns_any_matching(
                protected,
                &["__FitzResponse", "status", "401i64", "as u16"],
            ),
            "esperaba un `return __FitzResponse {{ status: (401i64) as u16, ... }}`, body:\n{}",
            ast_test::fn_body_text(protected)
        );
        assert!(
            ast_test::fn_body_text(protected).contains("__to_fitz_json"),
            "esperaba que el body se serialice con __to_fitz_json"
        );
    }

    #[test]
    fn status_codes_handler_envuelve_returns_normales_en_200() {
        // Spec polimórfico: una fn HTTP que contiene `Stmt::ReturnStatus`
        // tiene su return type sobreescrito a `__FitzResponse`. Los
        // `return user`/`return "x"` normales se envuelven en
        // `__FitzResponse { status: 200, body: ... }`.
        let src = "@get(\"/u/{id}\") fn get_user(id: Int) -> Str {\n\
                       if (id == 1) { return \"alice\" }\n\
                       return 404 {\"msg\": \"no encontrado\"}\n\
                   }";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let get_user =
            ast_test::find_item_fn(&file, "get_user").expect("falta fn get_user");
        // El return de Str "alice" debe envolverse en status 200.
        assert!(
            ast_test::fn_body_returns_any_matching(
                get_user,
                &["__FitzResponse", "status : 200"],
            ),
            "esperaba return con `__FitzResponse {{ status: 200, ... }}`, body:\n{}",
            ast_test::fn_body_text(get_user)
        );
        // El return 404 emite su status custom como cast.
        assert!(
            ast_test::fn_body_returns_any_matching(
                get_user,
                &["__FitzResponse", "404i64", "as u16"],
            ),
            "esperaba return con `status: (404i64) as u16`, body:\n{}",
            ast_test::fn_body_text(get_user)
        );
    }

    #[test]
    fn status_codes_wrapper_destructura_fitz_response() {
        // El wrapper `__handler_X` que llama una fn que retorna
        // `__FitzResponse` debe emitir `from_u16(...)` + `Json(body)`
        // directo en vez del path de Result/value plano.
        let src = "@get(\"/p\") fn p() -> Str {\n\
                       return 403 {\"msg\": \"prohibido\"}\n\
                   }";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let wrapper =
            ast_test::find_item_fn(&file, "__handler_p").expect("falta __handler_p");
        let resp =
            ast_test::find_local_in_fn(wrapper, "__resp").expect("falta let __resp en wrapper");
        assert_eq!(
            ast_test::local_type(&resp).as_deref(),
            Some("__FitzResponse"),
            "esperaba `let __resp: __FitzResponse`"
        );
        assert_eq!(
            ast_test::local_init(&resp).as_deref(),
            Some("__result"),
            "esperaba `let __resp = __result`"
        );
        let body = ast_test::fn_body_text(wrapper);
        assert!(
            body.contains("StatusCode :: from_u16 (__resp . status)")
                || body.contains("StatusCode :: from_u16 (__resp .status)"),
            "esperaba `StatusCode::from_u16(__resp.status)`, got:\n{}",
            body
        );
        assert!(
            body.contains("axum :: Json (__resp . body)")
                || body.contains("axum :: Json (__resp .body)"),
            "esperaba `axum::Json(__resp.body)`, got:\n{}",
            body
        );
    }

    #[test]
    fn status_codes_prelude_define_fitz_response() {
        let src = "@get(\"/p\") fn p() -> Str => \"ok\"";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        // El preludio HTTP siempre incluye `__FitzResponse` (aunque no
        // se use; cuesta poco y permite mezclar status custom con
        // handlers normales en el mismo programa).
        assert!(
            ast_test::find_item_struct(&file, "__FitzResponse").is_some(),
            "esperaba `struct __FitzResponse` en el preludio HTTP, got:\n{}",
            code
        );
    }

    // ---- Query params HTTP ----

    #[test]
    fn query_params_obligatorio_emite_match_some_404_si_falta() {
        // `@get("/x?limit={limit}") fn h(limit: Int)`: el wrapper
        // extrae `Query<HashMap>` y bindea `limit: i64` con coerción.
        // Falta → 400.
        let src = "@get(\"/items?limit={limit}\") fn list_items(limit: Int) -> Int => limit";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let wrapper = ast_test::find_item_fn(&file, "__handler_list_items")
            .expect("falta __handler_list_items");
        let pats_tys = ast_test::fn_param_pats_and_types(wrapper);
        assert!(
            pats_tys.iter().any(|(p, t)| {
                p.contains("axum :: extract :: Query")
                    && p.contains("__qmap")
                    && t.contains("axum :: extract :: Query")
                    && t.contains("HashMap")
                    && t.contains("String")
            }),
            "esperaba extractor Query<HashMap<String, String>>, got: {:?}",
            pats_tys
        );
        let limit = ast_test::find_local_in_fn(wrapper, "limit")
            .expect("falta `let limit` en wrapper");
        assert_eq!(
            ast_test::local_type(&limit).as_deref(),
            Some("i64"),
            "esperaba `limit: i64`"
        );
        let init = ast_test::local_init(&limit).unwrap_or_default();
        assert!(
            init.contains("__qmap . get (\"limit\")")
                || init.contains("__qmap .get (\"limit\")"),
            "esperaba init que matchea __qmap.get(\"limit\"), got: {}",
            init
        );
        // El mensaje de error para query faltante es contrato user-visible,
        // chequeo en el código completo.
        assert!(
            code.contains("query param 'limit': falta — es obligatorio"),
            "esperaba mensaje 400 para query param faltante"
        );
    }

    #[test]
    fn query_params_nullable_emite_option_none_si_falta() {
        // `limit: Int?` → `Option<i64>`. Missing → None, presente OK →
        // Some(v), parse error → 400.
        let src = "@get(\"/items?limit={limit}\") fn list_items(limit: Int?) -> Int => 0";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let wrapper = ast_test::find_item_fn(&file, "__handler_list_items")
            .expect("falta __handler_list_items");
        let limit = ast_test::find_local_in_fn(wrapper, "limit")
            .expect("falta `let limit` en wrapper");
        assert_eq!(
            ast_test::local_type(&limit).as_deref(),
            Some("Option < i64 >"),
            "esperaba `limit: Option<i64>`"
        );
        let init = ast_test::local_init(&limit).unwrap_or_default();
        assert!(
            init.contains("None => None"),
            "esperaba branch `None => None` para query opcional, got init: {}",
            init
        );
    }

    #[test]
    fn query_params_str_no_necesita_parse() {
        // `name: Str` → `String`, sin `.parse::<...>()`.
        let src = "@get(\"/x?name={name}\") fn h(name: Str) -> Str => name";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let wrapper =
            ast_test::find_item_fn(&file, "__handler_h").expect("falta __handler_h");
        let name = ast_test::find_local_in_fn(wrapper, "name")
            .expect("falta `let name` en wrapper");
        assert_eq!(
            ast_test::local_type(&name).as_deref(),
            Some("String"),
            "esperaba `name: String`"
        );
        let init = ast_test::local_init(&name).unwrap_or_default();
        assert!(
            init.contains("Ok ::< String , String > (__s . clone ())")
                || init.contains("Ok ::< String, String > (__s .clone ())")
                || init.contains("Ok :: < String , String > (__s . clone ())"),
            "esperaba coerción `Ok::<String, String>(__s.clone())`, got: {}",
            init
        );
        assert!(
            !init.contains(". parse"),
            "no debería haber `.parse::<...>()` para Str, got: {}",
            init
        );
    }

    #[test]
    fn query_params_bool_acepta_true_false() {
        let src = "@get(\"/x?on={on}\") fn h(on: Bool) -> Bool => on";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let wrapper =
            ast_test::find_item_fn(&file, "__handler_h").expect("falta __handler_h");
        let on = ast_test::find_local_in_fn(wrapper, "on")
            .expect("falta `let on` en wrapper");
        assert_eq!(
            ast_test::local_type(&on).as_deref(),
            Some("bool"),
            "esperaba `on: bool`"
        );
        let init = ast_test::local_init(&on).unwrap_or_default();
        assert!(
            init.contains("\"true\" => Ok") || init.contains("\"true\"=> Ok"),
            "esperaba arm contra `\"true\"`, got init: {}",
            init
        );
    }

    #[test]
    fn query_params_path_y_query_combinados_emiten_ambos_extractores() {
        // `/users/{id}?limit={limit}` con `id: Int, limit: Int?`. Emite
        // ambos extractores: AxumPath<i64> y AxumQuery<HashMap>.
        let src = "@get(\"/users/{id}?limit={limit}\") \
                   fn list(id: Int, limit: Int?) -> Int => id";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let wrapper =
            ast_test::find_item_fn(&file, "__handler_list").expect("falta __handler_list");
        let pats_tys = ast_test::fn_param_pats_and_types(wrapper);
        assert!(
            pats_tys.iter().any(|(p, t)| {
                p.contains("axum :: extract :: Path")
                    && p.contains("id")
                    && t.contains("axum :: extract :: Path")
                    && t.contains("i64")
            }),
            "esperaba extractor Path<i64> para `id`, got: {:?}",
            pats_tys
        );
        assert!(
            pats_tys.iter().any(|(p, t)| {
                p.contains("axum :: extract :: Query")
                    && t.contains("HashMap")
                    && t.contains("String")
            }),
            "esperaba extractor Query<HashMap>, got: {:?}",
            pats_tys
        );
        let limit = ast_test::find_local_in_fn(wrapper, "limit")
            .expect("falta `let limit` en wrapper");
        assert_eq!(
            ast_test::local_type(&limit).as_deref(),
            Some("Option < i64 >"),
            "esperaba `limit: Option<i64>`"
        );
    }

    #[test]
    fn query_params_template_sin_param_correspondiente_es_error() {
        // El template declara `?limit={limit}` pero el handler no
        // tiene un param `limit` → error claro.
        let src = "@get(\"/x?limit={limit}\") fn h() -> Int => 0";
        let err = gen(src).expect_err("esperaba error");
        assert!(
            err.message.contains("query param") && err.message.contains("limit"),
            "esperaba mensaje sobre query param sin param correspondiente, fue: {}",
            err.message
        );
    }

    #[test]
    fn query_params_tipo_no_soportado_es_error_de_codegen() {
        // Listas no se soportan como query param.
        let src = "@get(\"/x?ids={ids}\") fn h(ids: List<Int>) -> Int => 0";
        let err = gen(src).expect_err("esperaba error");
        assert!(
            err.message.contains("query param") && err.message.contains("no soportado"),
            "esperaba mensaje sobre tipo no soportado, fue: {}",
            err.message
        );
    }

    #[test]
    fn http_body_post_con_tipo_emite_from_fitz_json() {
        let src = "type Input { msg: Str }\n\
                   @post(\"/echo\") fn echo(body: Input) -> Input => body";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let wrapper =
            ast_test::find_item_fn(&file, "__handler_echo").expect("falta __handler_echo");
        let pats_tys = ast_test::fn_param_pats_and_types(wrapper);
        assert!(
            pats_tys.iter().any(|(p, t)| {
                p.contains("axum :: Json")
                    && p.contains("body_raw")
                    && t.contains("axum :: Json")
                    && t.contains("serde_json :: Value")
            }),
            "esperaba extractor body_raw: axum::Json<serde_json::Value>, got: {:?}",
            pats_tys
        );
        let body = ast_test::fn_body_text(wrapper);
        assert!(
            body.contains("__FromFitzJson") && body.contains("__from_fitz_json"),
            "esperaba que el body llame __from_fitz_json, got:\n{}",
            body
        );
        assert!(
            body.contains("StatusCode :: BAD_REQUEST"),
            "esperaba 400 si la deserialización falla, got:\n{}",
            body
        );
    }

    #[test]
    fn http_server_decorator_setea_addr() {
        let src = "@server(8080, \"0.0.0.0\") fn main() => 0\n\
                   @get(\"/\") fn index() -> Str => \"ok\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        let main = ast_test::find_item_fn(&file, "main").expect("falta fn main");
        let body = ast_test::fn_body_text(main);
        assert!(
            body.contains("\"0.0.0.0:8080\" . parse")
                || body.contains("\"0.0.0.0:8080\".parse"),
            "esperaba `\"0.0.0.0:8080\".parse()` en fn main, got:\n{}",
            body
        );
    }

    #[test]
    fn http_sin_server_decorator_usa_default_3000() {
        let src = "@get(\"/\") fn index() -> Str => \"ok\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        let main = ast_test::find_item_fn(&file, "main").expect("falta fn main");
        let body = ast_test::fn_body_text(main);
        assert!(
            body.contains("\"127.0.0.1:3000\" . parse")
                || body.contains("\"127.0.0.1:3000\".parse"),
            "esperaba `\"127.0.0.1:3000\".parse()` (default), got:\n{}",
            body
        );
    }

    #[test]
    fn http_75_emite_static_openapi_schema_y_handler() {
        // Fase 7.5: el código generado incluye el schema embebido
        // como static + el handler async + la ruta /openapi.json
        // en el Router.
        let src = "@get(\"/\") fn index() -> Str => \"hola\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        // El static `__FITZ_OPENAPI_SCHEMA` aparece en el archivo
        // como item top-level.
        assert!(
            ast_test::find_item_static(&file, "__FITZ_OPENAPI_SCHEMA").is_some(),
            "esperaba static __FITZ_OPENAPI_SCHEMA en el archivo"
        );
        // El handler async existe.
        assert!(
            ast_test::find_item_fn(&file, "__serve_openapi_json").is_some(),
            "esperaba fn __serve_openapi_json en el archivo"
        );
        // El Router incluye `.route("/openapi.json", ...)`.
        let main = ast_test::find_item_fn(&file, "main").expect("falta fn main");
        let body = ast_test::fn_body_text(main);
        assert!(
            body.contains("\"/openapi.json\""),
            "esperaba .route(\"/openapi.json\", ...) en fn main, got:\n{}",
            body
        );
    }

    #[test]
    fn http_75_emite_scalar_html_y_ruta_docs() {
        let src = "@get(\"/\") fn index() -> Str => \"hola\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        assert!(
            ast_test::find_item_static(&file, "__FITZ_SCALAR_HTML").is_some(),
            "esperaba static __FITZ_SCALAR_HTML en el archivo"
        );
        assert!(
            ast_test::find_item_fn(&file, "__serve_docs").is_some(),
            "esperaba fn __serve_docs en el archivo"
        );
        let main = ast_test::find_item_fn(&file, "main").expect("falta fn main");
        let body = ast_test::fn_body_text(main);
        assert!(
            body.contains("\"/docs\""),
            "esperaba .route(\"/docs\", ...) en fn main, got:\n{}",
            body
        );
    }

    #[test]
    fn http_75_server_docs_false_no_emite_rutas_auto() {
        // @server(docs=false): ni el static schema ni el HTML, ni
        // las rutas autoregistradas. Programa válido sigue compilando.
        let src = "@server(3000, docs=false) fn main() => 0\n\
                   @get(\"/\") fn index() -> Str => \"ok\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        assert!(
            ast_test::find_item_static(&file, "__FITZ_OPENAPI_SCHEMA").is_none(),
            "con docs=false NO debería emitirse __FITZ_OPENAPI_SCHEMA"
        );
        assert!(
            ast_test::find_item_static(&file, "__FITZ_SCALAR_HTML").is_none(),
            "con docs=false NO debería emitirse __FITZ_SCALAR_HTML"
        );
        assert!(
            ast_test::find_item_fn(&file, "__serve_openapi_json").is_none(),
            "con docs=false NO debería emitirse __serve_openapi_json"
        );
        let main = ast_test::find_item_fn(&file, "main").expect("falta fn main");
        let body = ast_test::fn_body_text(main);
        assert!(
            !body.contains("\"/openapi.json\"") && !body.contains("\"/docs\""),
            "con docs=false el Router NO debería tener rutas /openapi.json ni /docs, got:\n{}",
            body
        );
    }

    #[test]
    fn http_75_usuario_declara_openapi_json_propio_no_se_pisa() {
        // Si el usuario declara `@get("/openapi.json")`, el codegen
        // NO debe emitir la ruta auto (su handler gana). El handler
        // del usuario sí se emite normalmente.
        let src = "@get(\"/openapi.json\") fn custom() -> Str => \"mio\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        // /docs sí se sigue auto-registrando (el usuario no la pisó).
        assert!(
            ast_test::find_item_static(&file, "__FITZ_SCALAR_HTML").is_some(),
            "/docs debería seguir auto-registrándose"
        );
        // El schema cacheado NO se emite (la ruta del usuario lo sirve).
        assert!(
            ast_test::find_item_static(&file, "__FITZ_OPENAPI_SCHEMA").is_none(),
            "con /openapi.json del usuario, NO se debería emitir el schema cacheado"
        );
        // El handler del usuario sí.
        let main = ast_test::find_item_fn(&file, "main").expect("falta fn main");
        let body = ast_test::fn_body_text(main);
        assert!(
            body.contains("__handler_custom"),
            "esperaba que el handler del usuario aparezca en el Router, got:\n{}",
            body
        );
    }

    #[test]
    fn http_75_server_decorator_acepta_kwarg_docs() {
        // @server(3000, docs=true) ya no aborta el codegen (el guard
        // de 7.0 se aflojó para @server en 7.5).
        let src = "@server(3000, docs=true) fn main() => 0\n\
                   @get(\"/\") fn h() -> Str => \"ok\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        // Con docs=true (default explícito), sí se emiten las rutas auto.
        assert!(
            ast_test::find_item_static(&file, "__FITZ_OPENAPI_SCHEMA").is_some(),
            "docs=true (explícito) debería emitir el schema embebido"
        );
    }

    #[test]
    fn http_75_server_decorator_kwarg_desconocido_es_error() {
        let src = "@server(3000, version=\"1.0\") fn main() => 0\n\
                   @get(\"/\") fn h() -> Str => \"ok\"";
        let err = gen(src).expect_err("esperaba error de codegen");
        assert!(
            err.message.contains("version") && err.message.contains("reconocido"),
            "esperaba mensaje sobre kwarg desconocido, fue: {}",
            err.message
        );
    }

    #[test]
    fn http_75_decorator_de_ruta_con_kwarg_sigue_siendo_error() {
        // Los decoradores HTTP de ruta (@get/@post/@put/@delete) NO
        // aceptan kwargs hoy. Solo @server y @header.
        let src = "@get(\"/x\", foo=1) fn h() -> Str => \"ok\"";
        let err = gen(src).expect_err("esperaba error de codegen");
        assert!(
            err.message.contains("@get") && err.message.contains("foo"),
            "esperaba mensaje sobre kwarg en decorator HTTP de ruta, fue: {}",
            err.message
        );
    }

    #[test]
    fn http_76_wrapper_extrae_headermap_y_bindea_header_obligatorio() {
        let src = "@header(name=\"Authorization\")\n@get(\"/protected\") fn protected(authorization: Str) -> Str => authorization";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        let handler = ast_test::find_item_fn(&file, "__handler_protected")
            .expect("falta __handler_protected");
        let body = ast_test::fn_body_text(handler);
        // Extractor HeaderMap presente.
        let attrs = ast_test::fn_param_pats_and_types(handler);
        assert!(
            attrs.iter().any(|(_, ty)| ty.contains("HeaderMap")),
            "esperaba `__hmap: HeaderMap` en la firma, got params: {:?}",
            attrs
        );
        // Binding obligatorio con 400 si falta. El body se normaliza
        // vía quote::ToTokens, que separa puntos con espacios; aceptamos
        // ambas formas.
        assert!(
            body.contains("__hmap . get (\"authorization\")")
                || body.contains("__hmap.get(\"authorization\")"),
            "esperaba lookup case-insensitive en lowercase, body:\n{}",
            body
        );
        assert!(
            body.contains("BAD_REQUEST") && body.contains("Authorization") && body.contains("obligatorio"),
            "esperaba branch 400 con mensaje 'obligatorio', body:\n{}",
            body
        );
    }

    #[test]
    fn http_76_wrapper_header_nullable_es_option_string() {
        let src = "@header(name=\"X-Trace-Id\")\n@get(\"/traced\") fn traced(x_trace_id: Str?) -> Str => \"ok\"";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        let file = ast_test::parse(&code);
        let handler = ast_test::find_item_fn(&file, "__handler_traced")
            .expect("falta __handler_traced");
        let body = ast_test::fn_body_text(handler);
        // Option<String> para el binding.
        assert!(
            body.contains("Option < String >") || body.contains("Option<String>"),
            "esperaba `Option<String>` en el binding del header nullable, body:\n{}",
            body
        );
        // Sin branch 400 para este header.
        assert!(
            !body.contains("'X-Trace-Id': falta"),
            "header nullable NO debería tener branch 400, body:\n{}",
            body
        );
    }

    #[test]
    fn http_76_schema_embebido_incluye_header_en_parameters() {
        // Fase 7.5 + 7.6: el schema embebido en el binario incluye
        // el header con in:"header".
        let src = "@header(name=\"Authorization\")\n@get(\"/protected\") fn protected(authorization: Str) -> Str => authorization";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        // El static __FITZ_OPENAPI_SCHEMA contiene el JSON. Buscamos
        // texto literal `"in":"header"` en el output del codegen.
        assert!(
            code.contains("\"in\":\"header\""),
            "esperaba `\"in\":\"header\"` en el schema embebido. \
             Si el formato cambia, ajustá el test."
        );
        assert!(
            code.contains("\"Authorization\""),
            "esperaba el HTTP name del header en el schema embebido"
        );
    }

    #[test]
    fn http_decorator_de_ruta_sobre_fn_main_es_error_claro() {
        // `@get("/") fn main()` no debe ignorarse silenciosamente:
        // 5b.6 generaba la `fn main` async desde el codegen, así que el
        // decorator de ruta sobre `fn main` quedaba mudo. R1 lo ataja.
        let src = "@get(\"/\") fn main() => 0";
        let err = gen(src).expect_err("esperaba error de codegen");
        assert!(
            err.message.contains("`fn main` solo admite `@server"),
            "esperaba mensaje sobre fn main + decorator HTTP, fue: {}",
            err.message
        );
    }

    #[test]
    fn http_state_compartido_emite_lazy_lock() {
        // F11 + F17.4b — el codegen emite un `static LazyLock<Arc<Mutex<T>>>`
        // por cada state var detectado, y cada fn que la referencia
        // materializa el Arc al inicio del body via `(*__FITZ_STATE_X).clone()`.
        // Antes de F17.4b era `thread_local!` + `.with(|s| s.clone())`;
        // el cambio destrabó tokio multi-thread (paralelismo HTTP real).
        let src = "let users = [1, 2, 3]\n\
                   @get(\"/users\") fn list_users() -> List<Int> => users";
        let code = gen(src).unwrap();
        // El static aparece como `static __FITZ_STATE_USERS: ...
        // LazyLock<...> = LazyLock::new(|| ...);`. Validación liviana
        // por sub-string sobre el output (no hay helper `find_top_static`).
        assert!(
            code.contains("static __FITZ_STATE_USERS")
                && code.contains("LazyLock"),
            "esperaba `static __FITZ_STATE_USERS: ... LazyLock<...>`, got:\n{}",
            code
        );
        // Cada fn que toca la state debe materializarla con `(*X).clone()`.
        let file = ast_test::parse(&code);
        let list_users =
            ast_test::find_item_fn(&file, "list_users").expect("falta fn list_users");
        let body = ast_test::fn_body_text(list_users);
        assert!(
            body.contains("__FITZ_STATE_USERS") && body.contains(". clone"),
            "esperaba materialización con `(*__FITZ_STATE_USERS).clone()`, got:\n{}",
            body
        );
    }

    #[test]
    fn http_state_no_referenciado_no_se_promueve_a_lazy_lock() {
        // Si una var top-level NO es referenciada por ninguna fn HTTP,
        // no es state compartido — se queda como var local en `fn main()`.
        let src = "let ignorada = 42\n\
                   @get(\"/\") fn index() -> Str => \"ok\"";
        let code = gen(src).unwrap();
        // El static no debe aparecer (ni como item ni en ningún lado).
        assert!(
            !code.contains("__FITZ_STATE_IGNORADA"),
            "no esperaba el static __FITZ_STATE_IGNORADA, got:\n{}",
            code
        );
    }

    #[test]
    fn http_cargo_toml_incluye_axum_y_tokio() {
        // El Cargo.toml condicional se prueba via `generate_project`,
        // no via `gen` (que solo devuelve main.rs). Pasamos por el API
        // pública para validar.
        use std::path::Path;
        let tokens = crate::lexer::tokenize(
            "@get(\"/\") fn index() -> Str => \"ok\"",
        )
        .unwrap();
        let program = crate::parser::parse(tokens).unwrap();
        let (env, _types, _defs, errs) = crate::types::check_program(&program);
        assert!(errs.is_empty(), "checker errors: {:?}", errs);
        let project = generate_project(Path::new("test.fitz"), &program, &env, crate::manifest::DepRegistry::new()).unwrap();
        assert!(
            project.cargo_toml.contains("axum = \"0.8\""),
            "esperaba axum en Cargo.toml, got:\n{}",
            project.cargo_toml
        );
        assert!(
            project.cargo_toml.contains("tokio"),
            "esperaba tokio en Cargo.toml, got:\n{}",
            project.cargo_toml
        );
        assert!(
            project.cargo_toml.contains("serde_json"),
            "esperaba serde_json en Cargo.toml, got:\n{}",
            project.cargo_toml
        );
    }

    #[test]
    fn no_http_cargo_toml_es_minimalista() {
        use std::path::Path;
        let tokens = crate::lexer::tokenize("print(\"hola\")").unwrap();
        let program = crate::parser::parse(tokens).unwrap();
        let (env, _types, _defs, errs) = crate::types::check_program(&program);
        assert!(errs.is_empty());
        let project = generate_project(Path::new("test.fitz"), &program, &env, crate::manifest::DepRegistry::new()).unwrap();
        assert!(
            !project.cargo_toml.contains("axum"),
            "no debería haber axum en Cargo.toml sin HTTP, got:\n{}",
            project.cargo_toml
        );
    }

    #[test]
    fn http_type_emite_impl_to_fitz_json() {
        let src = "type User { id: Int, name: Str }\n\
                   @get(\"/\") fn index() -> Str => \"ok\"";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        assert!(
            ast_test::find_impl(&file, "__ToFitzJson", "UserData").is_some(),
            "esperaba `impl __ToFitzJson for UserData`, got:\n{}",
            code
        );
        assert!(
            ast_test::find_impl(&file, "__FromFitzJson", "UserData").is_some(),
            "esperaba `impl __FromFitzJson for UserData`, got:\n{}",
            code
        );
    }

    #[test]
    fn fn_sin_anotacion_de_param_es_error() {
        assert_err_contains(
            "fn double(n) -> Int { return n * 2 }",
            &["parámetro", "anotación"],
        );
    }

    // ---- 5b.4: Result, `?`, match ---------------------------------------

    #[test]
    fn result_type_anotacion_emite_result_t_string() {
        // `Result<Int>` Fitz → `Result<i64, String>` Rust.
        let code =
            gen("fn divide(a: Int, b: Int) -> Result<Int> { return Ok(a / b) }").unwrap();
        let file = ast_test::parse(&code);
        let divide = ast_test::find_item_fn(&file, "divide").expect("falta fn divide");
        assert_eq!(
            ast_test::fn_return_type(divide).as_deref(),
            Some("Result < i64 , String >"),
            "esperaba return type `Result<i64, String>`"
        );
    }

    #[test]
    fn ok_constructor_emite_ok_envoltorio() {
        let code = gen("fn ok42() -> Result<Int> { return Ok(42) }").unwrap();
        let file = ast_test::parse(&code);
        let ok42 = ast_test::find_item_fn(&file, "ok42").expect("falta fn ok42");
        assert!(
            ast_test::fn_body_returns_any_matching(ok42, &["Ok", "42i64"]),
            "esperaba `return Ok(42i64)`, body:\n{}",
            ast_test::fn_body_text(ok42)
        );
    }

    #[test]
    fn err_con_str_literal_emite_string_from() {
        let code = gen("fn boom() -> Result<Int> { return Err(\"explotó\") }").unwrap();
        let file = ast_test::parse(&code);
        let boom = ast_test::find_item_fn(&file, "boom").expect("falta fn boom");
        assert!(
            ast_test::fn_body_returns_any_matching(
                boom,
                &["Err", "String :: from", "\"explotó\""],
            ),
            "esperaba `return Err(String::from(\"explotó\"))`, body:\n{}",
            ast_test::fn_body_text(boom)
        );
    }

    #[test]
    fn err_con_no_str_coerciona_via_format() {
        // Err(42): el Err side está pinned a String, así que se coerce
        // con format!. Cambio de comportamiento sutil pero documentado.
        let code = gen("fn boom() -> Result<Str> { return Err(42) }").unwrap();
        let file = ast_test::parse(&code);
        let boom = ast_test::find_item_fn(&file, "boom").expect("falta fn boom");
        assert!(
            ast_test::fn_body_returns_any_matching(boom, &["Err", "format !", "42i64"]),
            "esperaba coerción a String via format!, body:\n{}",
            ast_test::fn_body_text(boom)
        );
    }

    #[test]
    fn try_operador_emite_question_mark_rust() {
        // Adentro de fn que retorna Result, `expr?` → `<expr>?` Rust.
        let code = gen(
            "fn find_user(id: Int) -> Result<Int> { return Ok(id) }\n\
             fn describe(id: Int) -> Result<Str> { let u = find_user(id)?\n return Ok(\"x\") }",
        )
        .unwrap();
        let file = ast_test::parse(&code);
        let describe =
            ast_test::find_item_fn(&file, "describe").expect("falta fn describe");
        let u = ast_test::find_local_in_fn(describe, "u").expect("falta `let u`");
        let init = ast_test::local_init_expr(&u).expect("`let u` sin init");
        // Atraviesa paréntesis externos opcionales y verifica Try.
        let mut e = init;
        while let syn::Expr::Paren(p) = e {
            e = &*p.expr;
        }
        let try_node = match e {
            syn::Expr::Try(t) => t,
            _ => panic!(
                "esperaba `Expr::Try` como init de `u`, got tokens: {}",
                ast_test::ts(init)
            ),
        };
        let inner = ast_test::ts(&*try_node.expr);
        assert!(
            inner.contains("find_user") && inner.contains("(id)"),
            "esperaba `find_user(id)?`, inner del Try: {}",
            inner
        );
    }

    #[test]
    fn try_top_level_es_error_de_codegen() {
        // `?` en top-level (afuera de cualquier fn) pasa el checker
        // (return_stack vacío, sin contexto a chequear) pero el
        // codegen lo ataja: `?` Rust solo funciona adentro de fns
        // que retornen Result. Sin ese contexto, no podemos emitirlo.
        assert_err_contains(
            "let x = Ok(1)?",
            &["?", "Result"],
        );
    }

    #[test]
    fn match_sobre_result_exhaustivo_no_agrega_catch_all() {
        // Ok(v) + Err(e) cubren Result completo — no se agrega panic.
        let file = ast_test::parse(
            &gen(
                "fn divide(a: Int, b: Int) -> Result<Int> { return Ok(a / b) }\n\
                 match divide(10, 2) { Ok(v) => print(\"ok: {v}\"), Err(e) => print(\"err: {e}\") }",
            )
            .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        // Sin catch-all artificial → cero panic! macros en el main.
        assert_eq!(
            ast_test::count_macro_calls(stmts, "panic"),
            0,
            "no esperaba panic! (el match es exhaustivo)"
        );
    }

    #[test]
    fn match_no_exhaustivo_sobre_int_agrega_panic() {
        // Match sobre Int sin catch-all → agregamos `_ => panic!(...)`
        // con el mismo mensaje del intérprete.
        let file = ast_test::parse(
            &gen("let v = 1\nlet s = match v { 0 => \"cero\", 1 => \"uno\" }").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        assert!(
            ast_test::count_macro_calls(stmts, "panic") >= 1,
            "esperaba al menos un panic! (catch-all artificial)"
        );
        // Mensaje del panic matchea el del intérprete.
        let last = ast_test::ts(stmts.last().unwrap());
        assert!(
            last.contains("no matche") && last.contains("ning") && last.contains("brazo"),
            "esperaba mensaje del panic sobre brazo no matcheado, fue: {}",
            last
        );
    }

    #[test]
    fn match_con_wildcard_no_agrega_panic() {
        // El wildcard `_` ya cubre todo — no agregamos catch-all extra.
        let file = ast_test::parse(
            &gen("let v = 1\nlet s = match v { 0 => \"cero\", _ => \"otro\" }").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        assert_eq!(
            ast_test::count_macro_calls(stmts, "panic"),
            0,
            "el wildcard ya es catch-all, no debería sumarse panic"
        );
    }

    #[test]
    fn match_ok_binding_introduce_var_en_scope() {
        // El binding `u` adentro del arm `Ok(u)` debe poder usarse
        // (acceso a `.id`, paso a `print`, etc.).
        let code = gen(
            "type User { id: Int }\n\
             fn find_user(id: Int) -> Result<User> { return Ok(User { id: id }) }\n\
             match find_user(1) { Ok(u) => print(u.id), Err(e) => print(e) }",
        )
        .unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let m = ast_test::find_match(stmts).expect("falta match en main");
        let ok_arm = m
            .arms
            .iter()
            .find(|a| {
                let p = ast_test::ts(&a.pat);
                // `Ok (u)` con binding (no `Ok (__v)` interno ni `Ok (_)`).
                p.contains("Ok") && p.contains("u") && !p.contains("__v") && !p.contains("_")
                    || p == "Ok (u)"
            })
            .expect("falta arm `Ok(u)`");
        let body = ast_test::ts(&*ok_arm.body);
        assert!(
            body.contains(". lock") && body.contains(". id"),
            "esperaba field access tipo `u.lock().unwrap().id` en el arm body, got: {}",
            body
        );
    }

    #[test]
    fn print_de_result_emite_match_inline() {
        // `print(r)` con `r: Result<T>` produce un match inline que
        // formatea `Ok(...)` o `Err("...")` igual al intérprete.
        let code = gen(
            "fn ok42() -> Result<Int> { return Ok(42) }\n\
             let r = ok42()\nprint(r)",
        )
        .unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let println_args = ast_test::first_macro_args_in_stmts(stmts, "println")
            .expect("falta println! en main");
        assert!(
            println_args.contains("Ok (__v)")
                && println_args.contains("format !")
                && println_args.contains("\"Ok({})\""),
            "esperaba arm `Ok(__v) => format!(\"Ok({{}})\", ...)`, println! args:\n{}",
            println_args
        );
        assert!(
            println_args.contains("Err (__e)")
                && println_args.contains("\"Err(\\\"{}\\\")\""),
            "esperaba arm `Err(__e) => format!(\"Err(\\\"{{}}\\\")\", __e)`, println! args:\n{}",
            println_args
        );
    }

    #[test]
    fn list_find_con_question_mark_emite_chain() {
        // Patrón canónico: `users.find(...)?` adentro de fn Result.
        let code = gen(
            "type User { id: Int }\n\
             fn first(us: List<User>) -> Result<User> { let u = us.find(fn(u) => u.id == 1)?\n return Ok(u) }",
        )
        .unwrap();
        let file = ast_test::parse(&code);
        let first = ast_test::find_item_fn(&file, "first").expect("falta fn first");
        let u = ast_test::find_local_in_fn(first, "u").expect("falta `let u` en first");
        let init = ast_test::local_init_expr(&u).expect("`let u` sin init");
        // El init debe ser `<expr>?` — `syn::Expr::Try` (opcionalmente
        // envuelto en paréntesis).
        let mut e = init;
        while let syn::Expr::Paren(p) = e {
            e = &*p.expr;
        }
        assert!(
            matches!(e, syn::Expr::Try(_)),
            "esperaba que el init de `u` sea `<expr>?`, got tokens: {}",
            ast_test::ts(init)
        );
    }

    // ---- 5b.5: módulos / import ----------------------------------------

    /// Genera el código de un programa "main" tratándolo como un módulo
    /// importado (sin loader externo). Útil para validar el codegen
    /// de un módulo independientemente del orquestador.
    fn gen_module(src: &str) -> Result<String, FitzError> {
        let tokens = crate::lexer::tokenize(src).expect("lex OK");
        let program = crate::parser::parse(tokens).expect("parse OK");
        let (env, _types, _defs, errors) = crate::types::check_program(&program);
        if !errors.is_empty() {
            panic!("checker errors: {:?}", errors);
        }
        generate_module_rs(&program, &env)
    }

    #[test]
    fn modulo_emite_pub_en_struct_y_alias() {
        // Un módulo expone tipos custom con `pub` en struct + alias.
        let code = gen_module("type User { id: Int, name: Str }").unwrap();
        let file = ast_test::parse(&code);
        let user_data =
            ast_test::find_item_struct(&file, "UserData").expect("falta struct UserData");
        assert!(
            ast_test::vis_is_pub(&user_data.vis),
            "esperaba `pub struct UserData`"
        );
        let user_alias =
            ast_test::find_item_type(&file, "User").expect("falta type alias User");
        assert!(
            ast_test::vis_is_pub(&user_alias.vis),
            "esperaba `pub type User`"
        );
    }

    #[test]
    fn modulo_emite_pub_en_fn() {
        let code = gen_module("fn add(a: Int, b: Int) -> Int => a + b").unwrap();
        let file = ast_test::parse(&code);
        let add = ast_test::find_item_fn(&file, "add").expect("falta fn add");
        assert!(ast_test::vis_is_pub(&add.vis), "esperaba `pub fn add`");
    }

    #[test]
    fn modulo_let_str_top_level_se_emite_como_pub_static() {
        let code = gen_module("let MSG = \"hola\"").unwrap();
        let file = ast_test::parse(&code);
        let msg = ast_test::find_item_static(&file, "MSG").expect("falta static MSG");
        assert!(ast_test::vis_is_pub(&msg.vis), "esperaba `pub static MSG`");
        assert_eq!(
            ast_test::ts(&*msg.ty),
            "& str",
            "esperaba tipo &str para MSG"
        );
        assert_eq!(
            ast_test::ts(&*msg.expr),
            "\"hola\"",
            "esperaba init literal `\"hola\"` para MSG"
        );
    }

    #[test]
    fn modulo_let_int_top_level_se_emite_como_pub_const() {
        let code = gen_module("let MAX_RETRIES: Int = 5").unwrap();
        let file = ast_test::parse(&code);
        let max = ast_test::find_item_const(&file, "MAX_RETRIES")
            .expect("falta const MAX_RETRIES");
        assert!(
            ast_test::vis_is_pub(&max.vis),
            "esperaba `pub const MAX_RETRIES`"
        );
        assert_eq!(
            ast_test::ts(&*max.ty),
            "i64",
            "esperaba tipo i64 para MAX_RETRIES"
        );
        assert_eq!(
            ast_test::ts(&*max.expr),
            "5i64",
            "esperaba init `5i64` para MAX_RETRIES"
        );
    }

    #[test]
    fn modulo_top_level_no_acepta_expr_compleja() {
        // Una RHS no literal a nivel top de módulo se rechaza con
        // mensaje que cita 5b.5 (deuda residual).
        let r = gen_module("let X = 1 + 1");
        let err = r.expect_err("esperaba error de codegen");
        assert!(
            err.message.contains("literal") || err.message.contains("RHS"),
            "esperaba mensaje sobre literal/RHS, fue: {}",
            err.message
        );
    }

    #[test]
    fn fn_body_de_modulo_puede_referenciar_const_local() {
        // PREFIX como `pub static`, greet la usa adentro de su body.
        // Comprobamos que el codegen del módulo no se queja de
        // "variable desconocida".
        let code = gen_module(
            "let PREFIX = \"hola, \"\nfn greet(name: Str) -> Str => \"{PREFIX}{name}\"",
        )
        .unwrap();
        let file = ast_test::parse(&code);
        let prefix = ast_test::find_item_static(&file, "PREFIX")
            .expect("falta static PREFIX");
        assert!(ast_test::vis_is_pub(&prefix.vis), "esperaba `pub static PREFIX`");
        assert_eq!(ast_test::ts(&*prefix.ty), "& str");
        assert_eq!(ast_test::ts(&*prefix.expr), "\"hola, \"");
        let greet = ast_test::find_item_fn(&file, "greet").expect("falta fn greet");
        assert!(ast_test::vis_is_pub(&greet.vis), "esperaba `pub fn greet`");
        let pats_tys = ast_test::fn_param_pats_and_types(greet);
        assert_eq!(pats_tys.len(), 1, "esperaba 1 param");
        assert!(
            pats_tys[0].0.contains("mut") && pats_tys[0].0.contains("name"),
            "esperaba pat `mut name`, got: {:?}",
            pats_tys
        );
        assert_eq!(pats_tys[0].1, "String", "esperaba tipo String para name");
        assert_eq!(
            ast_test::fn_return_type(greet).as_deref(),
            Some("String"),
            "esperaba return type String"
        );
        let body = ast_test::fn_body_text(greet);
        assert!(
            body.contains("String :: from (PREFIX)"),
            "esperaba `String::from(PREFIX)` en el body, got:\n{}",
            body
        );
    }

    // -----------------------------------------------------------------
    // Fase 8.7.1 — codegen acepta imports Python y emite preludio +
    // bindings + helpers. Tests reapuntados desde 8.1.5 (que rechazaba
    // con mensaje claro) — el shape "emitir + linkear pyo3" reemplaza
    // al guard duro.
    // -----------------------------------------------------------------

    #[test]
    fn build_acepta_from_python_import_emite_preludio() {
        let code = gen("from python import math\n").expect("8.7.1: from python import compila");
        assert!(
            code.contains("use pyo3::prelude::*;"),
            "esperaba `use pyo3::prelude::*;` en el preludio Python"
        );
        assert!(
            code.contains("__FitzPyObject"),
            "esperaba struct __FitzPyObject en preludio"
        );
        assert!(
            code.contains("__fitz_py_import"),
            "esperaba helper __fitz_py_import en preludio"
        );
        // 8.7.2: binding global = static + getter, no `let` local.
        assert!(
            code.contains("static __FITZ_PY_BIND_MATH"),
            "esperaba static __FITZ_PY_BIND_MATH, got:\n{}",
            code
        );
        assert!(
            code.contains("fn __fitz_py_bind_math()"),
            "esperaba getter __fitz_py_bind_math, got:\n{}",
            code
        );
        assert!(
            code.contains("__fitz_py_import(\"math\")"),
            "esperaba `__fitz_py_import(\"math\")` en el getter, got:\n{}",
            code
        );
    }

    #[test]
    fn build_acepta_import_python_punteado_emite_binding_con_ultimo_segmento() {
        let code = gen("import python.os.path\n").expect("8.7.1: import python.X compila");
        // Convención: `import python.os.path` → binding `path` (último
        // segmento), dotted Python `os.path`.
        assert!(
            code.contains("static __FITZ_PY_BIND_PATH"),
            "esperaba static __FITZ_PY_BIND_PATH, got:\n{}",
            code
        );
        assert!(
            code.contains("__fitz_py_import(\"os.path\")"),
            "esperaba `__fitz_py_import(\"os.path\")`, got:\n{}",
            code
        );
    }

    #[test]
    fn build_alias_python_emite_binding_con_alias() {
        // `from python import math as m` → binding `m`, dotted `math`.
        let code = gen("from python import math as m\n").expect("8.7.1: alias compila");
        assert!(
            code.contains("static __FITZ_PY_BIND_M"),
            "esperaba static __FITZ_PY_BIND_M, got:\n{}",
            code
        );
        assert!(
            code.contains("__fitz_py_import(\"math\")"),
            "esperaba `__fitz_py_import(\"math\")`, got:\n{}",
            code
        );
    }

    #[test]
    fn build_sin_python_no_emite_preludio() {
        let code = gen("let x = 1\nprint(x)\n").expect("CLI básico compila");
        assert!(
            !code.contains("__FitzPyObject"),
            "no debería emitir preludio Python para programas sin imports Python"
        );
        assert!(
            !code.contains("use pyo3"),
            "no debería emitir `use pyo3` para programas sin imports Python"
        );
    }

    #[test]
    fn build_python_field_access_emite_get_attr_obj() {
        // `let pi = math.pi` (sin annot) → `__fitz_py_get_attr_obj`
        // opaco. Tipo de `pi` queda como `__FitzPyObject`. 8.7.2: el
        // receptor `math` se traduce al getter `__fitz_py_bind_math()`.
        let code = gen("from python import math\nlet pi = math.pi\n")
            .expect("8.7.1: field access opaco compila");
        assert!(
            code.contains("__fitz_py_get_attr_obj(&__fitz_py_bind_math()")
                && code.contains("\"pi\""),
            "esperaba `__fitz_py_get_attr_obj(&__fitz_py_bind_math(), \"pi\")`, got:\n{}",
            code
        );
    }

    #[test]
    fn build_python_field_access_con_annotacion_float_emite_extract() {
        // `let pi: Float = math.pi` → coerción PyAny→Float aplica
        // `__fitz_py_extract_f64`.
        let code = gen("from python import math\nlet pi: Float = math.pi\n")
            .expect("8.7.1: extracción a Float compila");
        assert!(
            code.contains("__fitz_py_extract_f64"),
            "esperaba __fitz_py_extract_f64 al asignar a Float, got:\n{}",
            code
        );
    }

    #[test]
    fn build_python_field_access_con_annotacion_int_emite_extract() {
        let code = gen("from python import sys\nlet m: Int = sys.maxsize\n")
            .expect("8.7.1: extracción a Int compila");
        assert!(
            code.contains("__fitz_py_extract_i64"),
            "esperaba __fitz_py_extract_i64 al asignar a Int, got:\n{}",
            code
        );
    }

    #[test]
    fn build_call_python_emite_invoke_con_marshaling() {
        // 8.7.2: `math.sqrt(16.0)` → `__fitz_py_invoke(&...,
        // |py| Ok(vec![{ let __a = ...; __a.__fitz_to_py(py, "arg0")? }]))`.
        // Resultado tipa como `Result<PyAny>`.
        let code = gen("from python import math\nlet raw = math.sqrt(16.0)\n")
            .expect("8.7.2: call Python compila");
        assert!(
            code.contains("__fitz_py_invoke"),
            "esperaba __fitz_py_invoke en el call site, got:\n{}",
            code
        );
        assert!(
            code.contains("__fitz_py_get_attr_obj(&__fitz_py_bind_math()"),
            "esperaba `__fitz_py_get_attr_obj(&__fitz_py_bind_math(), \"sqrt\")`, got:\n{}",
            code
        );
        assert!(
            code.contains("__fitz_to_py(py, \"arg0\")"),
            "esperaba marshaling `__fitz_to_py(py, \"arg0\")`, got:\n{}",
            code
        );
    }

    #[test]
    fn build_call_python_devuelve_result_pyany() {
        // El binding `let raw = math.sqrt(...)` (sin annot) sintetiza
        // `Result<PyAny>`. El rust_type_for emite `Result<__FitzPyObject, String>`.
        let code = gen("from python import math\nlet raw = math.sqrt(16.0)\n")
            .expect("8.7.2: binding sin annot");
        assert!(
            code.contains("Result < __FitzPyObject , String >")
                || code.contains("Result<__FitzPyObject, String>"),
            "esperaba tipo Result<__FitzPyObject, String>, got:\n{}",
            code
        );
    }

    #[test]
    fn build_call_python_con_args_primitivos_marshallan() {
        // Cada arg primitivo (Int, Float, Str, Bool) se marshaller via
        // `__fitz_to_py(py, "argN")` con el path numerado.
        let code = gen("from python import json\nlet raw = json.dumps([1, 2, 3])\n")
            .expect("8.7.2: args primitivos compilan");
        assert!(
            code.contains("\"arg0\""),
            "esperaba path \"arg0\", got:\n{}",
            code
        );
    }

    #[test]
    fn match_range_emite_guard_con_contains() {
        // Pattern de rango `0..10` → guard con `(0..10).contains(&__n)`.
        let code =
            gen("let n = 5\nlet s = match n { 0..10 => \"chico\", _ => \"grande\" }").unwrap();
        let file = ast_test::parse(&code);
        let stmts = ast_test::main_block_stmts(&file);
        let m = ast_test::find_match(stmts).expect("falta match en main");
        let guarded = m
            .arms
            .iter()
            .find(|a| a.guard.is_some())
            .expect("falta arm con guard (range pattern)");
        let guard_tokens = ast_test::ts(&guarded.guard.as_ref().unwrap().1);
        assert!(
            guard_tokens.contains("0i64")
                && guard_tokens.contains("10i64")
                && guard_tokens.contains(". contains")
                && guard_tokens.contains("__n"),
            "esperaba guard `(0i64..10i64).contains(&__n)`, got: {}",
            guard_tokens
        );
    }
}
