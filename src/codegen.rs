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
use crate::types::{check_program, is_compatible, resolve_type_expr, ResolvedField, Type, TypeEnv, TypeId};

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
    type_info: &crate::types::TypeInfo,
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

    // Fase 9.w.1.d — auth nativa. Habilita el preludio de helpers
    // `__fitz_jwt_*` / `__fitz_hash_*` y suma `jsonwebtoken` + `argon2`
    // + `rand_core` al Cargo.toml cuando el programa usa el módulo
    // built-in `jwt`/`hash` o cualquier decorator de auth
    // (`@auth_provider`/`@authenticated`/`@admin`).
    let uses_auth = program_uses_auth(program);
    // Fase 9.w.2.c — detección de uso de WebSockets. Habilita el
    // preludio `__FitzWsConn<T>` + broadcaster global, suma la feature
    // `ws` de axum + `futures-util` + `tokio-tungstenite` (transitivo
    // de la feature ws) al Cargo.toml generado.
    let uses_ws = program_uses_ws(program);

    // PASS 2 — Generar el main.rs. El loader expone los bindings de
    // módulos (`import foo` / `from foo import X`) para que el codegen
    // del main resuelva `foo.x` como path `foo::x` y los tipos
    // importados con sus fields completos.
    let main_rs = generate_main_rs(program, env, type_info, &loader, &python_imports)?;

    Ok(ProjectArtifacts {
        bin_name: stem.clone(),
        output_basename: raw_stem,
        cargo_toml: cargo_toml_for(
            &stem, has_http, uses_async, uses_python, uses_auth, uses_ws,
        ),
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
        matches!(
            s,
            Stmt::FnDef { decorators, .. }
                if decorators.iter().any(|d| matches!(
                    d.name.as_str(),
                    // Fase 9.w.2.c — `@ws` cuenta como HTTP también:
                    // el handshake es HTTP, y los wrappers WS necesitan
                    // toda la infraestructura HTTP (axum + tokio +
                    // serde + __ToFitzJson/__FromFitzJson para el
                    // marshaling JSON del T).
                    "get" | "post" | "put" | "delete" | "server" | "ws"
                ))
        )
    })
}

/// Fase 9.w.1.d — `true` si el programa usa al menos uno de:
/// - El módulo built-in `jwt` (`jwt.encode(...)` / `jwt.decode(...)`).
/// - El módulo built-in `hash` (`hash.password(...)` / `hash.verify(...)`).
/// - Algún decorator de auth (`@auth_provider`/`@authenticated`/`@admin`).
///
/// El codegen lo usa para decidir si agregar deps `jsonwebtoken` +
/// `argon2` + `rand_core` al Cargo.toml y si emitir el preludio de
/// helpers `__fitz_jwt_*` / `__fitz_hash_*`. Programas sin auth no
/// pagan el costo de bajar/compilar esas crates.
/// Fase 9.w.2.c — `true` si el programa tiene al menos un handler
/// con `@ws("/path")`. Habilita el preludio WS (`__FitzWsConn<T>` +
/// broadcaster global), suma la feature `ws` de axum + `futures-util`
/// al Cargo.toml generado, y emite los wrappers axum WS específicos
/// en lugar del HTTP dispatcher normal. Programas sin `@ws` no
/// pagan el costo.
fn program_uses_ws(program: &Program) -> bool {
    program.iter().any(|s| {
        matches!(
            s,
            Stmt::FnDef { decorators, .. }
                if decorators.iter().any(|d| d.name == "ws")
        )
    })
}

fn program_uses_auth(program: &Program) -> bool {
    use crate::ast::StrPart;
    fn expr_uses_auth(e: &Expr) -> bool {
        match e {
            Expr::Call { callee, args, .. } => {
                // jwt.X(...) o hash.X(...).
                if let Expr::Field { object, field, .. } = callee.as_ref() {
                    if let Expr::Ident(recv, _) = object.as_ref() {
                        if (recv == "jwt"
                            && matches!(field.as_str(), "encode" | "decode"))
                            || (recv == "hash"
                                && matches!(field.as_str(), "password" | "verify"))
                        {
                            return true;
                        }
                    }
                }
                expr_uses_auth(callee) || args.iter().any(expr_uses_auth)
            }
            Expr::BinOp { left, right, .. } => {
                expr_uses_auth(left) || expr_uses_auth(right)
            }
            Expr::UnaryOp { operand, .. } => expr_uses_auth(operand),
            Expr::Field { object, .. } => expr_uses_auth(object),
            Expr::Index { object, index, .. } => {
                expr_uses_auth(object) || expr_uses_auth(index)
            }
            Expr::Slice { object, start, end, .. } => {
                expr_uses_auth(object)
                    || start.as_ref().is_some_and(|s| expr_uses_auth(s))
                    || end.as_ref().is_some_and(|x| expr_uses_auth(x))
            }
            Expr::Range { start, end, .. } => {
                expr_uses_auth(start) || expr_uses_auth(end)
            }
            Expr::List(items, _) => items.iter().any(expr_uses_auth),
            Expr::ListComp { expr, iter, extra_clauses, filter, .. } => {
                expr_uses_auth(expr)
                    || expr_uses_auth(iter)
                    || extra_clauses.iter().any(|(_, it)| expr_uses_auth(it))
                    || filter.as_ref().is_some_and(|f| expr_uses_auth(f))
            }
            Expr::MapComp { key, value, iter, extra_clauses, filter, .. } => {
                expr_uses_auth(key)
                    || expr_uses_auth(value)
                    || expr_uses_auth(iter)
                    || extra_clauses.iter().any(|(_, it)| expr_uses_auth(it))
                    || filter.as_ref().is_some_and(|f| expr_uses_auth(f))
            }
            Expr::Map(pairs, _) => pairs
                .iter()
                .any(|(k, v)| expr_uses_auth(k) || expr_uses_auth(v)),
            Expr::StructLit { fields, .. } => {
                fields.iter().any(|(_, v)| expr_uses_auth(v))
            }
            Expr::Tuple(items, _) => items.iter().any(expr_uses_auth),
            Expr::TupleField { tuple, .. } => expr_uses_auth(tuple),
            Expr::If { condition, then, else_, .. } => {
                expr_uses_auth(condition)
                    || then.iter().any(stmt_uses_auth)
                    || else_
                        .as_ref()
                        .is_some_and(|b| b.iter().any(stmt_uses_auth))
            }
            Expr::Match { value, arms, .. } => {
                expr_uses_auth(value)
                    || arms.iter().any(|a| a.body.iter().any(stmt_uses_auth))
            }
            Expr::FnExpr { body, .. } => body.iter().any(stmt_uses_auth),
            Expr::StrInterp(parts, _) => parts.iter().any(|p| match p {
                StrPart::Lit(_) => false,
                StrPart::Expr(inner, _) => expr_uses_auth(inner),
            }),
            Expr::Loop { body, .. } => body.iter().any(stmt_uses_auth),
            Expr::Ok(inner, _)
            | Expr::Err(inner, _)
            | Expr::Try(inner, _)
            | Expr::Await(inner, _) => expr_uses_auth(inner),
            _ => false,
        }
    }
    fn stmt_uses_auth(s: &Stmt) -> bool {
        match s {
            Stmt::FnDef { decorators, body, .. } => {
                decorators.iter().any(|d| {
                    matches!(
                        d.name.as_str(),
                        "auth_provider" | "authenticated" | "admin"
                    )
                }) || body.iter().any(stmt_uses_auth)
            }
            Stmt::Assign { value, .. } => expr_uses_auth(value),
            Stmt::Expr(e, _) => expr_uses_auth(e),
            Stmt::Return(e, _) => expr_uses_auth(e),
            Stmt::ReturnStatus { status, body, .. } => {
                expr_uses_auth(status)
                    || body.as_ref().is_some_and(expr_uses_auth)
            }
            Stmt::While { condition, body, .. } => {
                expr_uses_auth(condition) || body.iter().any(stmt_uses_auth)
            }
            Stmt::Loop { body, .. } => body.iter().any(stmt_uses_auth),
            Stmt::For { iter, body, .. } => {
                expr_uses_auth(iter) || body.iter().any(stmt_uses_auth)
            }
            _ => false,
        }
    }
    program.iter().any(stmt_uses_auth)
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
/// Mini-tanda Fmt-build — detecta si el programa usa format specs
/// que requieren helpers custom: `,`/`_` grouping, `%` percent, `c`
/// char. Cuando es `true`, el preludio emite los helpers
/// `__fitz_fmt_grouping`/`__fitz_fmt_percent`/`__fitz_fmt_char`.
/// F13 SPIKE — detecta si el programa tiene al menos un literal de
/// lista con items de tipos AST distintos (heurística sintáctica
/// conservadora: mira solo los tipos directos de los items del
/// literal, sin tipar). Cubre el caso canónico `[1, "dos", true]`.
/// Listas con elementos calculados (`[f(), g()]`) donde el tipo se
/// resuelve solo en el checker pueden no triggerear el preludio
/// FitzValue — limitación aceptada del SPIKE.
fn program_uses_fitz_value(program: &Program) -> bool {
    use crate::ast::StrPart;
    fn item_class(e: &Expr) -> Option<u8> {
        // Clasifica un literal AST en buckets primitivos. None si no
        // es un literal directo (el tipo viene del checker — se asume
        // que `lub` lo resuelve).
        match e {
            Expr::Int(_, _) => Some(0),
            Expr::Float(_, _) => Some(1),
            Expr::Str(_, _) | Expr::StrInterp(_, _) => Some(2),
            Expr::Bool(_, _) => Some(3),
            Expr::Null(_) => Some(4),
            _ => None,
        }
    }
    fn list_is_heterogeneous(items: &[Expr]) -> bool {
        let mut seen: Option<u8> = None;
        for it in items {
            if let Some(c) = item_class(it) {
                // Int↔Float coerciona vía lub, no es heterogéneo.
                let c = if c == 1 { 0 } else { c };
                match seen {
                    None => seen = Some(c),
                    Some(prev) if prev == c => {}
                    Some(_) => return true,
                }
            }
        }
        false
    }
    /// F13.A — detecta heterogeneidad en pares de un Map literal.
    /// Triggerea si las keys son de tipos AST distintos O los values
    /// son de tipos AST distintos.
    fn map_is_heterogeneous(pairs: &[(Expr, Expr)]) -> bool {
        let keys: Vec<&Expr> = pairs.iter().map(|(k, _)| k).collect();
        let values: Vec<&Expr> = pairs.iter().map(|(_, v)| v).collect();
        let keys_owned: Vec<Expr> = keys.iter().map(|e| (**e).clone()).collect();
        let vals_owned: Vec<Expr> = values.iter().map(|e| (**e).clone()).collect();
        list_is_heterogeneous(&keys_owned) || list_is_heterogeneous(&vals_owned)
    }
    fn expr_uses_fv(e: &Expr) -> bool {
        match e {
            Expr::List(items, _) => {
                list_is_heterogeneous(items) || items.iter().any(expr_uses_fv)
            }
            Expr::StrInterp(parts, _) => parts.iter().any(|p| match p {
                StrPart::Expr(inner, _) => expr_uses_fv(inner),
                StrPart::Lit(_) => false,
            }),
            Expr::BinOp { left, right, .. } => expr_uses_fv(left) || expr_uses_fv(right),
            Expr::UnaryOp { operand, .. } => expr_uses_fv(operand),
            Expr::Call { callee, args, .. } => {
                expr_uses_fv(callee) || args.iter().any(expr_uses_fv)
            }
            Expr::Field { object, .. } => expr_uses_fv(object),
            Expr::Index { object, index, .. } => {
                expr_uses_fv(object) || expr_uses_fv(index)
            }
            Expr::Slice { object, start, end, .. } => {
                expr_uses_fv(object)
                    || start.as_ref().is_some_and(|s| expr_uses_fv(s))
                    || end.as_ref().is_some_and(|e| expr_uses_fv(e))
            }
            Expr::Map(pairs, _) => {
                map_is_heterogeneous(pairs)
                    || pairs.iter().any(|(k, v)| expr_uses_fv(k) || expr_uses_fv(v))
            }
            Expr::Tuple(items, _) => items.iter().any(expr_uses_fv),
            Expr::TupleField { tuple, .. } => expr_uses_fv(tuple),
            Expr::Range { start, end, .. } => expr_uses_fv(start) || expr_uses_fv(end),
            Expr::If { condition, then, else_, .. } => {
                expr_uses_fv(condition)
                    || then.iter().any(stmt_uses_fv)
                    || else_.as_ref().is_some_and(|e| e.iter().any(stmt_uses_fv))
            }
            Expr::Match { value, arms, .. } => {
                expr_uses_fv(value)
                    || arms.iter().any(|a| {
                        a.guard.as_ref().is_some_and(expr_uses_fv)
                            || a.body.iter().any(stmt_uses_fv)
                    })
            }
            Expr::FnExpr { body, .. } => body.iter().any(stmt_uses_fv),
            Expr::Loop { body, .. } => body.iter().any(stmt_uses_fv),
            Expr::ListComp {
                expr,
                iter,
                extra_clauses,
                filter,
                ..
            } => {
                expr_uses_fv(expr)
                    || expr_uses_fv(iter)
                    || extra_clauses.iter().any(|(_, it)| expr_uses_fv(it))
                    || filter.as_ref().is_some_and(|f| expr_uses_fv(f))
            }
            Expr::MapComp {
                key,
                value,
                iter,
                extra_clauses,
                filter,
                ..
            } => {
                expr_uses_fv(key)
                    || expr_uses_fv(value)
                    || expr_uses_fv(iter)
                    || extra_clauses.iter().any(|(_, it)| expr_uses_fv(it))
                    || filter.as_ref().is_some_and(|f| expr_uses_fv(f))
            }
            Expr::StructLit { fields, .. } => fields.iter().any(|(_, e)| expr_uses_fv(e)),
            Expr::NamedArg { value, .. } => expr_uses_fv(value),
            Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
                expr_uses_fv(inner)
            }
            _ => false,
        }
    }
    /// F13.C — detecta `Any` adentro de un TypeExpr de anotación.
    /// Triggerea sobre `List<Any>`, `Map<Str, Any>`, `Map<Any, V>`,
    /// `T?` con T conteniendo Any, etc.
    fn type_expr_has_any(t: &crate::ast::TypeExpr) -> bool {
        use crate::ast::TypeExpr;
        match t {
            TypeExpr::Named(n) => n == "Any",
            TypeExpr::Generic { name, args } => {
                name == "Any" || args.iter().any(type_expr_has_any)
            }
            TypeExpr::Nullable(inner) => type_expr_has_any(inner),
            TypeExpr::Function { params, ret } => {
                params.iter().any(type_expr_has_any) || type_expr_has_any(ret)
            }
            TypeExpr::Tuple(items) => items.iter().any(type_expr_has_any),
        }
    }
    fn stmt_uses_fv(s: &Stmt) -> bool {
        match s {
            Stmt::Assign { value, type_, .. } => {
                expr_uses_fv(value) || type_.as_ref().is_some_and(type_expr_has_any)
            }
            Stmt::Destructure { value, .. } => expr_uses_fv(value),
            Stmt::Return(e, _) | Stmt::Expr(e, _) => expr_uses_fv(e),
            Stmt::ReturnStatus { status, body, .. } => {
                expr_uses_fv(status) || body.as_ref().is_some_and(expr_uses_fv)
            }
            Stmt::While { condition, body, .. } => {
                expr_uses_fv(condition) || body.iter().any(stmt_uses_fv)
            }
            Stmt::Loop { body, .. } => body.iter().any(stmt_uses_fv),
            Stmt::For { iter, body, .. } => {
                expr_uses_fv(iter) || body.iter().any(stmt_uses_fv)
            }
            Stmt::FnDef { params, return_type, body, .. } => {
                params.iter().any(|p| p.type_.as_ref().is_some_and(type_expr_has_any))
                    || return_type.as_ref().is_some_and(type_expr_has_any)
                    || body.iter().any(stmt_uses_fv)
            }
            _ => false,
        }
    }
    program.iter().any(stmt_uses_fv)
}

fn program_uses_fmt_helpers(program: &Program) -> bool {
    use crate::ast::{FormatKind, StrPart};
    fn spec_needs_helper(spec: &crate::ast::FormatSpec) -> bool {
        if spec.grouping.is_some() {
            return true;
        }
        matches!(
            spec.kind,
            Some(FormatKind::Char)
                | Some(FormatKind::Percent)
                | Some(FormatKind::GeneralLower)
                | Some(FormatKind::GeneralUpper)
        )
    }
    fn expr_uses_fmt(e: &Expr) -> bool {
        match e {
            Expr::StrInterp(parts, _) => parts.iter().any(|p| match p {
                StrPart::Expr(inner, Some(spec)) => {
                    spec_needs_helper(spec) || expr_uses_fmt(inner)
                }
                StrPart::Expr(inner, None) => expr_uses_fmt(inner),
                StrPart::Lit(_) => false,
            }),
            Expr::BinOp { left, right, .. } => expr_uses_fmt(left) || expr_uses_fmt(right),
            Expr::UnaryOp { operand, .. } => expr_uses_fmt(operand),
            Expr::Call { callee, args, .. } => {
                expr_uses_fmt(callee) || args.iter().any(expr_uses_fmt)
            }
            Expr::Field { object, .. } => expr_uses_fmt(object),
            Expr::Index { object, index, .. } => {
                expr_uses_fmt(object) || expr_uses_fmt(index)
            }
            Expr::Slice { object, start, end, .. } => {
                expr_uses_fmt(object)
                    || start.as_ref().is_some_and(|s| expr_uses_fmt(s))
                    || end.as_ref().is_some_and(|e| expr_uses_fmt(e))
            }
            Expr::List(items, _) => items.iter().any(expr_uses_fmt),
            Expr::ListComp { expr, iter, extra_clauses, filter, .. } => {
                expr_uses_fmt(expr)
                    || expr_uses_fmt(iter)
                    || extra_clauses.iter().any(|(_, it)| expr_uses_fmt(it))
                    || filter.as_ref().is_some_and(|f| expr_uses_fmt(f))
            }
            Expr::MapComp { key, value, iter, extra_clauses, filter, .. } => {
                expr_uses_fmt(key)
                    || expr_uses_fmt(value)
                    || expr_uses_fmt(iter)
                    || extra_clauses.iter().any(|(_, it)| expr_uses_fmt(it))
                    || filter.as_ref().is_some_and(|f| expr_uses_fmt(f))
            }
            Expr::Map(pairs, _) => pairs.iter().any(|(k, v)| expr_uses_fmt(k) || expr_uses_fmt(v)),
            Expr::Tuple(items, _) => items.iter().any(expr_uses_fmt),
            Expr::TupleField { tuple, .. } => expr_uses_fmt(tuple),
            Expr::Range { start, end, .. } => expr_uses_fmt(start) || expr_uses_fmt(end),
            Expr::If { condition, then, else_, .. } => {
                expr_uses_fmt(condition)
                    || then.iter().any(stmt_uses_fmt)
                    || else_.as_ref().is_some_and(|b| b.iter().any(stmt_uses_fmt))
            }
            Expr::Match { value, arms, .. } => {
                expr_uses_fmt(value) || arms.iter().any(|a| a.body.iter().any(stmt_uses_fmt))
            }
            Expr::Loop { body, .. } => body.iter().any(stmt_uses_fmt),
            Expr::StructLit { fields, .. } => fields.iter().any(|(_, v)| expr_uses_fmt(v)),
            Expr::FnExpr { body, .. } => body.iter().any(stmt_uses_fmt),
            Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
                expr_uses_fmt(inner)
            }
            _ => false,
        }
    }
    fn stmt_uses_fmt(s: &Stmt) -> bool {
        match s {
            Stmt::FnDef { body, .. } => body.iter().any(stmt_uses_fmt),
            Stmt::Assign { value, .. } => expr_uses_fmt(value),
            Stmt::Return(e, _) => expr_uses_fmt(e),
            Stmt::ReturnStatus { status, body, .. } => {
                expr_uses_fmt(status) || body.as_ref().is_some_and(expr_uses_fmt)
            }
            Stmt::Expr(e, _) => expr_uses_fmt(e),
            Stmt::While { condition, body, .. } => {
                expr_uses_fmt(condition) || body.iter().any(stmt_uses_fmt)
            }
            Stmt::Loop { body, .. } => body.iter().any(stmt_uses_fmt),
            Stmt::For { iter, body, .. } => {
                expr_uses_fmt(iter) || body.iter().any(stmt_uses_fmt)
            }
            _ => false,
        }
    }
    program.iter().any(stmt_uses_fmt)
}

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
            Expr::Slice { object, start, end, .. } => {
                expr_uses_async(object)
                    || start.as_ref().is_some_and(|s| expr_uses_async(s))
                    || end.as_ref().is_some_and(|e| expr_uses_async(e))
            }
            Expr::List(items, _) => items.iter().any(expr_uses_async),
            Expr::ListComp { expr, iter, extra_clauses, filter, .. } => {
                expr_uses_async(expr)
                    || expr_uses_async(iter)
                    || extra_clauses.iter().any(|(_, it)| expr_uses_async(it))
                    || filter.as_ref().is_some_and(|f| expr_uses_async(f))
            }
            Expr::MapComp { key, value, iter, extra_clauses, filter, .. } => {
                expr_uses_async(key)
                    || expr_uses_async(value)
                    || expr_uses_async(iter)
                    || extra_clauses.iter().any(|(_, it)| expr_uses_async(it))
                    || filter.as_ref().is_some_and(|f| expr_uses_async(f))
            }
            Expr::Map(pairs, _) => pairs.iter().any(|(k, v)| expr_uses_async(k) || expr_uses_async(v)),
            Expr::Tuple(items, _) => items.iter().any(expr_uses_async),
            Expr::TupleField { tuple, .. } => expr_uses_async(tuple),
            Expr::Loop { body, .. } => body.iter().any(stmt_uses_async),
            Expr::Range { start, end, .. } => expr_uses_async(start) || expr_uses_async(end),
            Expr::If { condition, then, else_, .. } => {
                expr_uses_async(condition)
                    || then.iter().any(stmt_uses_async)
                    || else_.as_ref().map(|b| b.iter().any(stmt_uses_async)).unwrap_or(false)
            }
            Expr::Match { value, arms, .. } => {
                expr_uses_async(value) || arms.iter().any(|a| a.body.iter().any(stmt_uses_async))
            }
            Expr::StructLit { fields, .. } => fields.iter().any(|(_, v)| expr_uses_async(v)),
            Expr::FnExpr { body, .. } => body.iter().any(stmt_uses_async),
            Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) => expr_uses_async(inner),
            Expr::StrInterp(parts, _) => parts.iter().any(|p| match p {
                StrPart::Expr(e, _) => expr_uses_async(e),
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

// Mini-tanda Cd (F12) — identifica los `let X = <expr>` top-level del
// archivo principal que el codegen debe "hoistar" a const/static Rust
// global para que fns top-level puedan referenciarlos. Reglas:
//   - El value es const-eval (literal Int/Float/Bool/Null + ops puros,
//     o Str literal directo).
//   - Tiene una sola ocurrencia en main_stmts (no se reasigna).
//   - Es referenciado por al menos UNA fn top-level (sin esa ref no
//     hace falta hoistar — el let queda como local de main()).
// Devuelve los stmts en el orden de aparición original (necesario
// para que un hoist que referencia otro const ya esté declarado).
fn collect_f12_hoists<'a>(
    program: &'a Program,
    main_stmts: &[&'a Stmt],
) -> Vec<&'a Stmt> {
    // Paso 1: candidatos = `Stmt::Assign(Ident(name), hoistable_value)`
    // únicos en main_stmts.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for s in main_stmts {
        if let Stmt::Assign {
            target: AssignTarget::Ident(name),
            ..
        } = s
        {
            *counts.entry(name.clone()).or_insert(0) += 1;
        }
    }
    let mut candidates: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in main_stmts {
        if let Stmt::Assign {
            target: AssignTarget::Ident(name),
            value,
            ..
        } = s
        {
            let hoistable = is_const_eval_expr(value) || matches!(value, Expr::Str(_, _));
            if hoistable && counts.get(name).copied().unwrap_or(0) == 1 {
                candidates.insert(name.clone());
            }
        }
    }
    if candidates.is_empty() {
        return Vec::new();
    }

    // Paso 2: filtrar por referencia desde alguna fn top-level. Reusamos
    // `walk_stmt_for_state_refs` que ya hace skip de locales/params.
    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in program {
        if let Stmt::FnDef { params, body, .. } = s {
            let mut locals: std::collections::HashSet<String> = params
                .iter()
                .map(|p| p.name.clone())
                .collect();
            let mut refs = std::collections::HashSet::new();
            for stmt in body {
                walk_stmt_for_state_refs(stmt, &candidates, &mut locals, &mut refs);
            }
            referenced.extend(refs);
        }
    }

    // Paso 3: emitir en orden original solo los names que sobrevivieron.
    let mut out: Vec<&'a Stmt> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in main_stmts {
        if let Stmt::Assign {
            target: AssignTarget::Ident(name),
            ..
        } = s
        {
            if referenced.contains(name) && seen.insert(name.clone()) {
                out.push(s);
            }
        }
    }
    out
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
        Expr::Match { arms, .. } => arms.iter().any(|a| contains_return_status_stmts(&a.body)),
        _ => false,
    }
}

/// Mini-tanda Md — extrae todos los `Ident` de un Pattern. Usado por
/// los walkers de codegen y el codegen del `for` con tuple destructuring.
/// Solo cubre los patterns aceptados en `Stmt::For` (Ident, Wildcard,
/// Tuple). Otros patterns devuelven Vec vacío (el checker los rechaza
/// antes).
fn collect_pattern_idents(pat: &crate::ast::Pattern) -> Vec<String> {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name) => vec![name.clone()],
        Pattern::Wildcard => Vec::new(),
        Pattern::Tuple(subs) => subs.iter().flat_map(collect_pattern_idents).collect(),
        _ => Vec::new(),
    }
}

/// Mini-tanda Md — convierte un Pattern simple (Ident o Wildcard) en
/// `(binding_string, vec![(name, type)])` para emitir en un `for ... in`
/// y declarar el var en el scope del codegen. Tuple no se admite acá
/// porque los call sites del `for` lo descomponen ANTES de llamar a
/// este helper. Otros patterns (literales, Ok/Err, Range) devuelven
/// error claro.
fn pattern_to_simple_binding(
    pat: &crate::ast::Pattern,
    ty: &Type,
) -> Result<(String, Vec<(String, Type)>), String> {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name) => Ok((name.clone(), vec![(name.clone(), ty.clone())])),
        Pattern::Wildcard => Ok(("_".into(), Vec::new())),
        Pattern::Tuple(_) => Err(
            "el codegen del `for` con tuple pattern requiere un Pattern::Tuple manejado por el caller"
                .into(),
        ),
        other => Err(format!(
            "patrón `{:?}` no admitido como variable de `for` en `fitz build`",
            other
        )),
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
        // Mini-tanda T — destructuring. Walkear el value y registrar
        // los names del pattern como locals.
        Stmt::Destructure { pattern, value, .. } => {
            walk_expr_for_state_refs(value, candidates, locals, refs);
            collect_pattern_names(pattern, locals);
        }
        Stmt::Assign { target, value, .. } => {
            walk_expr_for_state_refs(value, candidates, locals, refs);
            match target {
                AssignTarget::Ident(name) => {
                    locals.insert(name.clone());
                }
                AssignTarget::Field { object, .. } => {
                    walk_expr_for_state_refs(object, candidates, locals, refs);
                }
                AssignTarget::Index { object, index } => {
                    walk_expr_for_state_refs(object, candidates, locals, refs);
                    walk_expr_for_state_refs(index, candidates, locals, refs);
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
            // Mini-tanda Md: extraemos todos los bindings del Pattern.
            let names = collect_pattern_idents(var);
            for n in &names {
                locals.insert(n.clone());
            }
            for s in body {
                walk_stmt_for_state_refs(s, candidates, locals, refs);
            }
            // Mantenemos los bindings (igual que antes, conservador).
        }
        Stmt::Break(_, _, _) | Stmt::Continue(_, _) => {}
        Stmt::FnDef { .. } | Stmt::TypeDef { .. } | Stmt::Import { .. } | Stmt::FromImport { .. } => {}
        // Fase 9.0.1 (F15): walkers estáticos del codegen ignoran
        // Error nodes — la API strict que llama al codegen nunca los
        // produce, pero defendemos contra panic si entran.
        Stmt::Error(_) => {}
    }
}

/// Mini-tanda T — recolecta los names bindeados por un pattern
/// (recursivo en Tuple). Usado por los walkers que mantienen un
/// set de locals para distinguir captures vs locales.
fn collect_pattern_names(
    pat: &crate::ast::Pattern,
    locals: &mut std::collections::HashSet<String>,
) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name) | Pattern::OkBinding(name) | Pattern::ErrBinding(name) => {
            locals.insert(name.clone());
        }
        Pattern::Tuple(subs) => {
            for s in subs {
                collect_pattern_names(s, locals);
            }
        }
        _ => {}
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
        Expr::Int(_, _) | Expr::Float(_, _) | Expr::Str(_, _) | Expr::Bool(_, _) | Expr::Null(_) | Expr::Bytes(_, _) => {}
        Expr::StrInterp(parts, _) => {
            for p in parts {
                if let StrPart::Expr(inner, _) = p {
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
        // Mini-tanda C — list comprehension. El `var` introducido es
        // local adentro del expr/filter, lo sumamos a locals para no
        // marcar falso positivo si shadowea un state var del scope.
        Expr::ListComp { expr, var, iter, extra_clauses, filter, .. } => {
            walk_expr_for_state_refs(iter, candidates, locals, refs);
            // Mini-tanda Up — `var` ahora es Pattern. Recolectamos todos
            // los nombres del pattern (Ident/Tuple recursivo) y los
            // agregamos a `locals` mientras walkeamos el cuerpo.
            let mut added: Vec<String> = Vec::new();
            collect_pattern_bindings(var, &mut added);
            // Mini-tanda Cmp+ — extra clauses: walkeamos cada iter
            // (que puede referenciar vars del clause anterior) y
            // sumamos los nombres de su pattern a locals.
            for (extra_var, extra_iter) in extra_clauses {
                for name in &added {
                    if !locals.contains(name) {
                        locals.insert(name.clone());
                    }
                }
                walk_expr_for_state_refs(extra_iter, candidates, locals, refs);
                collect_pattern_bindings(extra_var, &mut added);
            }
            for name in &added {
                if !locals.contains(name) {
                    locals.insert(name.clone());
                }
            }
            if let Some(f) = filter {
                walk_expr_for_state_refs(f, candidates, locals, refs);
            }
            walk_expr_for_state_refs(expr, candidates, locals, refs);
            // Quitamos solo los que NO estaban antes (preserva outer).
            for name in &added {
                locals.remove(name);
            }
        }
        // Mini-tanda Cmp+ — map comprehension análoga a ListComp.
        Expr::MapComp { key, value, var, iter, extra_clauses, filter, .. } => {
            walk_expr_for_state_refs(iter, candidates, locals, refs);
            let mut added: Vec<String> = Vec::new();
            collect_pattern_bindings(var, &mut added);
            for (extra_var, extra_iter) in extra_clauses {
                for name in &added {
                    if !locals.contains(name) {
                        locals.insert(name.clone());
                    }
                }
                walk_expr_for_state_refs(extra_iter, candidates, locals, refs);
                collect_pattern_bindings(extra_var, &mut added);
            }
            for name in &added {
                if !locals.contains(name) {
                    locals.insert(name.clone());
                }
            }
            if let Some(f) = filter {
                walk_expr_for_state_refs(f, candidates, locals, refs);
            }
            walk_expr_for_state_refs(key, candidates, locals, refs);
            walk_expr_for_state_refs(value, candidates, locals, refs);
            for name in &added {
                locals.remove(name);
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
        Expr::Slice { object, start, end, .. } => {
            walk_expr_for_state_refs(object, candidates, locals, refs);
            if let Some(s) = start { walk_expr_for_state_refs(s, candidates, locals, refs); }
            if let Some(e) = end { walk_expr_for_state_refs(e, candidates, locals, refs); }
        }
        Expr::Tuple(items, _) => {
            for it in items {
                walk_expr_for_state_refs(it, candidates, locals, refs);
            }
        }
        Expr::TupleField { tuple, .. } => {
            walk_expr_for_state_refs(tuple, candidates, locals, refs);
        }
        Expr::Loop { body, .. } => {
            for s in body {
                walk_stmt_for_state_refs(s, candidates, locals, refs);
            }
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
                for stmt in &arm.body {
                    walk_stmt_for_state_refs(stmt, candidates, locals, refs);
                }
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
        // Fp.3 — NamedArg solo aparece adentro de Call.args; el caller
        // recurse hacia adentro del value. Tratamos el wrapper como
        // passthrough.
        Expr::NamedArg { value, .. } => {
            walk_expr_for_state_refs(value, candidates, locals, refs);
        }
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
fn cargo_toml_for(
    stem: &str,
    has_http: bool,
    uses_async: bool,
    uses_python: bool,
    uses_auth: bool,
    uses_ws: bool,
) -> String {
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
    // Fase 9.w.1.d — auth deps. Si el programa usa `jwt.*`/`hash.*` o
    // decorators de auth, sumar `jsonwebtoken` (JWT HS256/384/512),
    // `argon2` (Argon2id password hashing) y `rand_core` (con feature
    // `getrandom` para `OsRng` del salt). Paralelo a las deps no
    // opcionales del binario `fitz` principal (ver Cargo.toml del
    // workspace).
    let auth_lines = if uses_auth {
        "jsonwebtoken = \"9\"\n\
         argon2 = { version = \"0.5\", features = [\"std\"] }\n\
         rand_core = { version = \"0.6\", features = [\"getrandom\"] }\n\
         serde_json = { version = \"1\", features = [\"preserve_order\"] }\n"
    } else {
        ""
    };
    let needs_deps_section = has_http || uses_async || uses_python || uses_auth;
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
    // `serde_json` ya está en http_lines cuando hay HTTP; si solo hay
    // auth (sin HTTP), `auth_lines` lo trae. Evitar duplicación: si
    // ambos están, omitir el de auth.
    let auth_lines_final = if has_http {
        // serde_json ya está en http_lines; omitirlo de auth.
        if uses_auth {
            "jsonwebtoken = \"9\"\n\
             argon2 = { version = \"0.5\", features = [\"std\"] }\n\
             rand_core = { version = \"0.6\", features = [\"getrandom\"] }\n"
        } else {
            ""
        }
    } else {
        auth_lines
    };
    // Fase 9.w.2.c — `axum` con feature `ws` cuando hay `@ws` handlers.
    // La feature `ws` agrega `dep:tokio-tungstenite` + `dep:sha1` +
    // `dep:base64` transitivamente — el codegen no las menciona por
    // separado, axum las arrastra. `futures-util` para los
    // combinadores Sink/Stream que usa el wrapper WS generado.
    let http_lines = if has_http {
        if uses_ws {
            "axum = { version = \"0.8\", features = [\"ws\"] }\n\
             futures-util = { version = \"0.3\", default-features = false, features = [\"std\"] }\n\
             serde = { version = \"1\", features = [\"derive\"] }\n\
             serde_json = { version = \"1\", features = [\"preserve_order\"] }\n"
        } else {
            "axum = \"0.8\"\n\
             serde = { version = \"1\", features = [\"derive\"] }\n\
             serde_json = { version = \"1\", features = [\"preserve_order\"] }\n"
        }
    } else {
        ""
    };
    format!(
        "{}\n[dependencies]\n{}{}{}{}",
        header, http_lines, tokio_line, pyo3_line, auth_lines_final
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
    /// Mini-tanda F14 — set de consts que se emitieron como accessor
    /// fn `pub fn X() -> T` (en lugar de `pub const X`). El importer
    /// necesita saberlo para emitir `X()` en lugar de `X`.
    accessor_consts: std::collections::HashSet<String>,
    /// Mini-tanda F15 — bindings de los `import` propios del módulo.
    /// El codegen del módulo los emite como `use crate::<other>::...`
    /// y los usa al resolver expresiones que referencian items
    /// cross-module. Vacío para módulos sin imports.
    #[allow(dead_code)]
    local_bindings: HashMap<String, ResolvedBinding>,
    /// Mini-tanda CM — métodos custom (R.3) de cada tipo exportado.
    /// El importer los copia a su `type_methods` para que el dispatch
    /// `instance.method()` resuelva sobre tipos importados. Sin esto,
    /// `from foo import User` + `u.greet()` falla en `fitz build` con
    /// "el tipo `User` no tiene un método llamado `greet`".
    type_methods: HashMap<String, Vec<crate::ast::MethodDef>>,
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
    /// Mini-tanda F15 — stack de paths en curso de carga, para
    /// detectar ciclos durante recursión transitiva. Paralelo al
    /// `loader_stack` del evaluator (`evaluator::load_module`).
    loading_stack: Vec<PathBuf>,
}

impl ModuleLoader {
    fn new(base_dir: PathBuf, dep_registry: crate::manifest::DepRegistry) -> Self {
        Self {
            base_dir,
            modules: Vec::new(),
            by_path: HashMap::new(),
            bindings: HashMap::new(),
            dep_registry,
            loading_stack: Vec::new(),
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

        // F15 — cycle detection paralelo al evaluator. Si el módulo
        // que vamos a cargar ya está en curso de carga (más arriba en
        // la cadena de imports transitivos), reportamos el ciclo
        // completo y abortamos.
        if self.loading_stack.contains(&canonical) {
            let mut cycle: Vec<String> = self
                .loading_stack
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            cycle.push(canonical.display().to_string());
            return Err(loader_err(format!(
                "ciclo de imports detectado: {}",
                cycle.join(" -> ")
            )));
        }
        self.loading_stack.push(canonical.clone());

        let load_result = self.load_module_inner(segments, &canonical);
        self.loading_stack.pop();
        load_result
    }

    /// F15 — Cuerpo de `load_module` separado para garantizar que
    /// `loading_stack.pop()` corra incluso ante errores intermedios.
    fn load_module_inner(
        &mut self,
        segments: &[String],
        canonical: &Path,
    ) -> Result<usize, FitzError> {
        let source = std::fs::read_to_string(canonical).map_err(|e| {
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

        // F15 — carga recursiva de imports transitivos. Antes del codegen
        // del módulo, resolvemos cada `Stmt::Import` / `Stmt::FromImport`
        // del módulo y armamos su tabla de bindings locales. Si alguno
        // dispara un ciclo, `load_module` lo detecta vía `loading_stack`.
        // Imports Python adentro de módulos transitivos: NO soportados
        // todavía (deuda residual menor — se rechaza explícito).
        let mut local_bindings: HashMap<String, ResolvedBinding> = HashMap::new();
        for stmt in &module_program {
            match stmt {
                Stmt::Import { path, .. }
                    if path.first().map(|s| s.as_str()) == Some("python") =>
                {
                    return Err(loader_err(format!(
                        "el módulo `{}` usa `from python import ...`: imports Python \
                         dentro de módulos transitivos no se soportan todavía. \
                         Workaround: poné el `from python import` en el main.",
                        segments.join(".")
                    )));
                }
                Stmt::FromImport { path, .. }
                    if path.first().map(|s| s.as_str()) == Some("python") =>
                {
                    return Err(loader_err(format!(
                        "el módulo `{}` usa `from python import ...`: imports Python \
                         dentro de módulos transitivos no se soportan todavía. \
                         Workaround: poné el `from python import` en el main.",
                        segments.join(".")
                    )));
                }
                Stmt::Import { path: nested, alias, .. } => {
                    let idx = self.load_module(nested)?;
                    let binding_name = alias.clone().unwrap_or_else(|| {
                        nested.last().cloned().unwrap_or_default()
                    });
                    local_bindings.insert(
                        binding_name,
                        ResolvedBinding::Namespace { module_index: idx },
                    );
                }
                Stmt::FromImport { path: nested, names, .. } => {
                    let idx = self.load_module(nested)?;
                    for (name, alias) in names {
                        let kind = self.classify_named(idx, name)?;
                        let local = alias.clone().unwrap_or_else(|| name.clone());
                        local_bindings.insert(
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

        // Generar el código Rust del módulo (modo Module). En F15 el
        // codegen recibe los bindings locales + las firmas de todos los
        // módulos ya cargados, así puede emitir `use crate::<other>::...`
        // y resolver expresiones cross-module adentro del módulo.
        let rust_content =
            generate_module_rs_with_bindings(&module_program, &module_env, &local_bindings, &self.modules)?;

        let mod_name = segments.last().cloned().unwrap_or_default();
        let rel_path = mod_rel_path_from_segments(segments);

        // Extraer firmas para uso del importer.
        let (type_sigs, fn_sigs, const_sigs, accessor_consts) =
            collect_module_sigs(&module_program, &module_env)?;

        // Mini-tanda CM — recolectar métodos custom de cada `type`
        // exportado. El importer los necesita para dispatch
        // `instance.method()` sobre tipos importados.
        let mut type_methods: HashMap<String, Vec<crate::ast::MethodDef>> = HashMap::new();
        for stmt in &module_program {
            if let Stmt::TypeDef { name, methods, .. } = stmt {
                if !methods.is_empty() {
                    type_methods.insert(name.clone(), methods.clone());
                }
            }
        }

        let idx = self.modules.len();
        self.modules.push(LoadedModule {
            mod_name,
            rel_path,
            rust_content,
            type_sigs,
            fn_sigs,
            const_sigs,
            accessor_consts,
            local_bindings,
            type_methods,
        });
        self.by_path.insert(canonical.to_path_buf(), idx);
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
/// F15 — Versión completa del codegen del módulo. Si el módulo tiene
/// imports propios (`local_bindings` no-vacío), se emite el bloque
/// `use crate::<other>::...` al inicio y las firmas de los módulos
/// referenciados se instalan en el ctx para que las expresiones
/// puedan resolver `<ns>.<field>` y nombres directos a items
/// cross-module.
///
/// `loaded_modules`: slice de los módulos ya cargados, en el mismo
/// orden que el `ModuleLoader`. Los `module_index` de
/// `local_bindings` son índices a este slice.
fn generate_module_rs_with_bindings(
    program: &Program,
    env: &TypeEnv,
    local_bindings: &HashMap<String, ResolvedBinding>,
    loaded_modules: &[LoadedModule],
) -> Result<String, FitzError> {
    // Hpx.2 — para módulos compilados via loader, computar TypeInfo
    // fresco. El loader corrió `resolve_program` antes pero no
    // `check_program`. Hacemos un check rápido solo para tener el
    // side-table; los errores ya fueron reportados arriba.
    let (_e, type_info, _d, _errs) = crate::types::check_program(program);
    let mut ctx = CodegenCtx::new_for_module(env, &type_info);
    // F15 — instalar firmas + bindings ANTES del pre-registro, porque
    // los pre-pases pueden tener que resolver tipos cross-module al
    // armar las firmas locales de fns/types/consts.
    for m in loaded_modules {
        ctx.loaded_modules.push(LoadedModuleSigs {
            mod_name: m.mod_name.clone(),
            type_sigs: m.type_sigs.clone(),
            fn_sigs: m.fn_sigs.clone(),
            const_sigs: m.const_sigs.clone(),
            accessor_consts: m.accessor_consts.clone(),
            type_methods: m.type_methods.clone(),
        });
    }
    for (name, binding) in local_bindings {
        ctx.module_bindings.insert(name.clone(), binding.clone());
    }
    ctx.pre_register_types(program)?;
    ctx.pre_register_fns(program)?;
    ctx.pre_register_top_lets(program)?;

    ctx.emit_prelude();

    // F15 — `use crate::<other>::...` lines para cada Named binding del
    // módulo. Para Namespace bindings (`import foo`) no hace falta
    // emitir `use crate::foo;` porque el módulo `foo` es declarado
    // como `mod foo;` en main.rs, que vive en crate root accesible
    // como `crate::foo`. Las referencias se emiten con prefix
    // `crate::` en módulos (ver `mod_path_prefix`).
    ctx.emit_module_use_decls(local_bindings, loaded_modules);

    // Particionar stmts top-level. Para módulos: type / fn / let.
    // F15: `Stmt::Import` / `Stmt::FromImport` se ignoran acá — el
    // loader ya los procesó recursivamente antes de invocar el codegen
    // y registró los bindings locales. Cualquier otra cosa → error
    // de codegen.
    let mut type_defs: Vec<&Stmt> = Vec::new();
    let mut top_fns: Vec<&Stmt> = Vec::new();
    let mut top_lets: Vec<&Stmt> = Vec::new();
    for s in program {
        match s {
            Stmt::TypeDef { .. } => type_defs.push(s),
            Stmt::FnDef { .. } => top_fns.push(s),
            Stmt::Assign { .. } => top_lets.push(s),
            Stmt::Import { .. } | Stmt::FromImport { .. } => {
                // F15: ya procesados por el loader.
            }
            other => {
                return Err(loader_err(format!(
                    "el módulo no soporta `{}` a nivel top: hoy permitimos solo `type`, \
                     `fn`, `let` e `import`.",
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
        Stmt::Destructure { .. } => "destructuring",
        Stmt::Expr(..) => "expresión suelta",
        Stmt::Return(..) => "return",
        Stmt::ReturnStatus { .. } => "return con status",
        Stmt::While { .. } => "while",
        Stmt::Loop { .. } => "loop",
        Stmt::For { .. } => "for",
        Stmt::Break(_, _, _) => "break",
        Stmt::Continue(_, _) => "continue",
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
///
/// Mini-tanda F14 — también devuelve `accessor_consts`: los nombres
/// de `let X = <expr>` cuya RHS NO es const-eval (StrInterp/Call/
/// StructLit/etc.). El codegen del módulo los emite como `pub fn X()
/// -> T` y el importer los referencia como `X()`.
#[allow(clippy::type_complexity)]
fn collect_module_sigs(
    program: &Program,
    env: &TypeEnv,
) -> Result<
    (
        HashMap<String, TypeSig>,
        HashMap<String, FnSig>,
        HashMap<String, Type>,
        std::collections::HashSet<String>,
    ),
    FitzError,
> {
    let mut type_sigs: HashMap<String, TypeSig> = HashMap::new();
    let mut fn_sigs: HashMap<String, FnSig> = HashMap::new();
    let mut const_sigs: HashMap<String, Type> = HashMap::new();
    let mut accessor_consts: std::collections::HashSet<String> =
        std::collections::HashSet::new();

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
                let defaults: Vec<Option<Expr>> = params.iter().map(|p| p.default.clone()).collect();
                let has_varargs = params.last().map(|p| p.varargs).unwrap_or(false);
                let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                fn_sigs.insert(name.clone(), FnSig { params: ps, ret, defaults, has_varargs, param_names });
            }
            Stmt::Assign { target, type_, value, .. } => {
                // Solo bindings simples a un Ident.
                let AssignTarget::Ident(name) = target else {
                    return Err(loader_err(
                        "el módulo no soporta asignación a campo a nivel top \
                         (solo `let X = <expr>`)"
                            .to_string(),
                    ));
                };
                // Mini-tanda F14 — sin anotación, el tipo se infiere
                // solo si la RHS es un literal puro. Para RHS más
                // complejas (BinOp, StrInterp, etc.) exigimos anotación
                // porque `collect_module_sigs` no hace inferencia
                // completa (sin codegen context).
                let resolved_ty = match type_ {
                    Some(te) => resolve_type_expr(te, env).map_err(|e| {
                        loader_err(format!(
                            "let `{}` del módulo: anotación: {}",
                            name, e.message
                        ))
                    })?,
                    None => infer_literal_type(value).ok_or_else(|| {
                        loader_err(format!(
                            "let `{}` del módulo: la RHS no es literal — anotá el tipo (`let {}: T = <expr>`).",
                            name, name
                        ))
                    })?,
                };
                const_sigs.insert(name.clone(), resolved_ty);
                // Mini-tanda F14 — Str-literal sigue siendo `pub static &str`,
                // los const-eval Rust → `pub const`, los demás → accessor fn.
                if !is_literal_expr(value) && !is_const_eval_expr(value) {
                    accessor_consts.insert(name.clone());
                }
            }
            _ => {}
        }
    }

    Ok((type_sigs, fn_sigs, const_sigs, accessor_consts))
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

/// Mini-tanda F14 — `true` si la expresión puede emitirse como
/// `pub const` Rust. Rust evalúa const expressions en compile-time:
/// los literales primitivos, los `BinOp` aritméticos/lógicos/bit-a-bit
/// sobre operandos const, y los `UnaryOp` Neg/Not/BitNot sobre const
/// son válidos.
///
/// No const-eval: Str (porque `String::from` no es const fn aún —
/// `&str` literal sí lo es pero hace falta type diferente), calls
/// a fns, StringInterp, StructLit, List/Map literals (necesitan
/// `Arc::new`), Idents (sin resolución estática).
/// Mini-tanda HTTP-Err — detecta si un type es `Nominal` con un field
/// `status: Int`. Lo usa el codegen del handler wrapper para decidir
/// si emitir lookup dinámico del status code del Err o caer al 500
/// histórico. El field debe llamarse exactamente `status` y tener
/// tipo `Int` (no Nullable<Int>).
fn err_type_has_status_field(ty: &Type, env: &TypeEnv) -> bool {
    let Type::Nominal(id) = ty.base() else {
        return false;
    };
    let info = env.info(*id);
    let Some(fields) = info.fields.as_ref() else {
        return false;
    };
    fields
        .iter()
        .any(|f| f.name == "status" && matches!(f.type_, Type::Int))
}

fn is_const_eval_expr(e: &Expr) -> bool {
    match e {
        Expr::Int(_, _) | Expr::Float(_, _) | Expr::Bool(_, _) | Expr::Null(_) => true,
        // Str literal NO es const-eval para `String` (la lógica de
        // gen_module_top_let lo maneja aparte como `pub static &str`).
        Expr::Str(_, _) => false,
        Expr::BinOp { op, left, right, .. } => {
            use crate::ast::BinOpKind::*;
            // Operadores que Rust acepta como const sobre primitivos.
            matches!(
                op,
                Add | Sub | Mul | Div | Mod | Eq | NotEq | Lt | LtEq | Gt | GtEq
                | And | Or | Xor | BitAnd | BitOr | BitXor | Shl | Shr
            ) && is_const_eval_expr(left)
                && is_const_eval_expr(right)
        }
        Expr::UnaryOp { op, operand, .. } => {
            use crate::ast::UnaryOpKind::*;
            matches!(op, Neg | Not | BitNot) && is_const_eval_expr(operand)
        }
        _ => false,
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
    // Para tests unit que llaman directamente a `generate_rust`,
    // computamos TypeInfo fresco. En el path real (`generate_project`)
    // se reusa el que ya computó el checker en main.rs.
    let (_env_ignored, type_info, _defs, _errs) = crate::types::check_program(program);
    let _ = _env_ignored;
    generate_main_rs(program, env, &type_info, &loader, &python_imports)
}

/// Genera el `src/main.rs` del Cargo project. Si hay módulos cargados,
/// emite los `mod foo;` y `use foo::{...};` correspondientes al inicio.
/// Si el programa tiene decoradores HTTP/`@server`, emite un `fn main()`
/// async con el Router + `axum::serve` (modo HTTP); si no, sigue el
/// flujo single-threaded clásico (modo CLI).
fn generate_main_rs(
    program: &Program,
    env: &TypeEnv,
    type_info: &crate::types::TypeInfo,
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
    let uses_fmt_helpers = program_uses_fmt_helpers(program);
    let uses_fitz_value = program_uses_fitz_value(program);
    let uses_auth = program_uses_auth(program);
    let uses_ws = program_uses_ws(program);

    let mut ctx = CodegenCtx::new(env, type_info);
    ctx.uses_async = uses_async;
    ctx.uses_python = uses_python;
    ctx.uses_fmt_helpers = uses_fmt_helpers;
    ctx.uses_fitz_value = uses_fitz_value;
    ctx.has_http = has_http;
    ctx.uses_auth = uses_auth;
    ctx.uses_ws = uses_ws;
    // Fase 9.w.1.d — pre-scan del `@auth_provider`. Singleton; el checker
    // (9.w.1.a) ya validó. Lo guardamos por nombre + is_async para que
    // cada handler con `@authenticated`/`@admin` emita la invocación
    // correcta.
    for stmt in program {
        if let Stmt::FnDef { name, decorators, is_async, .. } = stmt {
            if decorators.iter().any(|d| d.name == "auth_provider") {
                ctx.auth_provider_name = Some(name.clone());
                ctx.auth_provider_is_async = *is_async;
                break;
            }
        }
    }
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
    /// Fase 9.w.2.c — handlers `@ws("/path")`. Separados de
    /// `http_fns` porque el codegen del wrapper es distinto (axum
    /// `WebSocketUpgrade` + `on_upgrade` closure en lugar del HTTP
    /// dispatcher).
    ws_fns: Vec<&'a Stmt>,
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
    let mut ws_fns: Vec<&Stmt> = Vec::new();
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
                    // Fase 9.z.2.c: `@test` se ignora silenciosamente en
                    // codegen. La fn no se emite al output Rust (paralelo
                    // a `#[cfg(test)]` de Rust: las fns marcadas con `@test`
                    // pertenecen al runner de `fitz test`, no al binario
                    // final). Si una fn tiene cualquier decorator `@test`,
                    // saltamos el resto del análisis y NO la agregamos a
                    // `top_fns` ni `http_fns`.
                    if decorators.iter().any(|d| d.name == "test") {
                        continue;
                    }
                    // Separar `@server` de los `@get`/`@post`/etc.
                    let mut http_decos = false;
                    let mut ws_decos = false;
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
                            // Fase 9.w.1.d — `@auth_provider` marca una
                            // fn como el provider de auth singleton. El
                            // pre-scan de `generate_main_rs` ya capturó
                            // su nombre + is_async en `ctx.auth_provider_name`.
                            // Acá la fn sigue siendo top_fn (se emite
                            // como `pub async fn`/`pub fn` normal); el
                            // wrapper de cada handler protegido la
                            // invoca por nombre.
                            "auth_provider" => {}
                            // Fase 9.w.1.d — `@authenticated`/`@admin`
                            // sobre handlers HTTP. El wrapper del handler
                            // (gen_http_handler_wrapper + emit_auth_check)
                            // los procesa; acá solo los aceptamos como
                            // decorators válidos. NO setean http_decos
                            // por sí solos — debe haber un `@get`/`@post`/
                            // `@put`/`@delete` apilado (validado por el
                            // checker 9.w.1.a y por el evaluator MVP).
                            "authenticated" | "admin" => {}
                            // Fase 9.w.2.c — `@ws("/path")` marca un
                            // handler WebSocket. Va a `ws_fns` (no a
                            // `http_fns`) porque el wrapper generado
                            // es distinto: `WebSocketUpgrade` extractor
                            // + `on_upgrade` closure en lugar del HTTP
                            // dispatcher normal. Sin kwargs (validado
                            // por el branch general arriba).
                            "ws" => ws_decos = true,
                            other => {
                                return Err(FitzError::new(
                                    ErrorKind::TypeError,
                                    0,
                                    0,
                                    format!(
                                        "decorator `@{}` sobre fn `{}` no soportado en codegen (hoy: @get/@post/@put/@delete/@ws/@server/@header/@middleware/@auth_provider/@authenticated/@admin)",
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
                    if ws_decos {
                        // `@ws` wins over `@get/...` si por alguna razón
                        // estuvieran apilados (no es un caso válido, pero
                        // defensivo). El wrapper WS no consulta http_decos.
                        ws_fns.push(s);
                    } else if http_decos {
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
        ws_fns,
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
    // Fase 9.w.1.d: preludio de auth — helpers `__fitz_jwt_encode/
    // decode` y `__fitz_hash_password/verify`. Solo se emiten si el
    // programa usa el módulo built-in `jwt`/`hash` o cualquier
    // decorator de auth. Va después del preludio base y antes de los
    // mod/use decls para que las fns top-level del usuario lo
    // referencien sin necesidad de `use` extra.
    ctx.emit_auth_prelude();
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

    // Mini-tanda Cd (F12 fix) — hoistar los `let X = <const-eval>` top-level
    // del archivo principal que fns top-level referencian. Solo aplica en
    // modo CLI: en modo HTTP, el mecanismo de state compartido
    // (`detect_shared_state` + thread_local) ya cubre el caso.
    if !has_http {
        let hoists = collect_f12_hoists(program, &p.main_stmts);
        for stmt in hoists {
            ctx.gen_main_hoisted_let(stmt)?;
        }
    }
    for stmt in &p.http_fns {
        ctx.gen_top_fn(stmt)?;
    }
    // Fase 9.w.2.c — emitir handlers `@ws` como pub fn normales (con
    // signature `async fn h(conn: __FitzWsConn<T>, ...) -> ...`). El
    // wrapper axum los invoca tras el upgrade.
    for stmt in &p.ws_fns {
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
        // Fase 9.w.2.c — wrappers WS análogos (extractor
        // `WebSocketUpgrade`, auth pre-upgrade, on_upgrade closure).
        for stmt in &p.ws_fns {
            ctx.gen_ws_handler_wrapper(stmt)?;
        }
        // `#[tokio::main] async fn main` con Router + serve.
        // Fase 7.5: pasamos `program` para que adentro pueda
        // pre-computar el schema OpenAPI desde el AST.
        ctx.gen_http_main(
            &p.http_fns,
            &p.ws_fns,
            &p.server_config,
            &p.main_stmts,
            program,
        )?;
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
    /// Mini-tanda F14 — set de consts que se emitieron como
    /// accessor fn `pub fn X() -> T` (en lugar de `pub const`).
    /// El importer necesita saberlo para emitir `X()` en lugar de
    /// `X` al referenciarlas.
    accessor_consts: std::collections::HashSet<String>,
    /// Mini-tanda CM — métodos custom (R.3) por nombre de tipo
    /// exportado. Copiados al importer en `install_loader_bindings`
    /// y enriquecidos en `type_methods` al procesar imports.
    type_methods: HashMap<String, Vec<crate::ast::MethodDef>>,
}

struct CodegenCtx<'a> {
    env: &'a TypeEnv,
    /// Mini-tanda Hpx.2 — TypeInfo del checker para inferir return
    /// types de fns sin anotación (`fn greet(name: Str) { return name }`
    /// pre-Hpx.2: codegen error; post-Hpx.2: infiere `Str` via TypeInfo).
    type_info: &'a crate::types::TypeInfo,
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
    /// R.3 — métodos custom declarados por tipo (key = type name).
    /// Pre-registrado durante el walk inicial de typedefs; consumido
    /// por `gen_method_call` para resolver `instance.metodo(args)` y
    /// por `gen_type_def` para emitir el `impl FooData { ... }`.
    type_methods: HashMap<String, Vec<crate::ast::MethodDef>>,
    /// Consts/statics top-level del propio módulo (5b.5): nombre →
    /// tipo Fitz. Sirven para que el body de una fn del módulo pueda
    /// referenciarlas. En main mode, queda vacío (los `let` top-level
    /// son vars locales adentro de `fn main()`).
    own_consts: HashMap<String, Type>,
    /// Mini-tanda Cd — set de `let X = <const-eval>` top-level del
    /// archivo principal que el codegen "hoisteó" a `pub const`/
    /// `pub static` Rust porque alguna fn top-level los referencia.
    /// El check de `gen_expr Ident` los trata como bindings globales
    /// (resuelve al nombre Rust directo sin error de "variable
    /// desconocida"). Vacío en módulo mode (módulos siempre emiten
    /// todos sus `let` top-level como consts/accessors, no hace
    /// falta el set extra).
    hoisted_main_lets: HashMap<String, Type>,
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
    /// Mini-tanda L — stack paralelo a `Expr::Loop` actualmente
    /// siendo emitido. Cada frame recolecta los tipos de los
    /// `break <v>` adentro. `Expr::Loop` consume el frame para
    /// devolver el tipo unificado. Statement-mode loops NO empujan
    /// — sus `break <v>` se descartan.
    break_value_stack: Vec<Vec<Type>>,
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
    /// Mini-tanda P1 (Mw.next codegen) — nombres de fns usadas como
    /// `@middleware(fn)` con aridad 2 (post-process). El return type
    /// emitido para estas fns es `__FitzResponse` (no `Option<...>`)
    /// porque siempre devuelven una Response.
    middleware_post_fn_names: std::collections::HashSet<String>,
    /// Fase 6.6: `true` si el programa usa async — cualquier `async fn`
    /// declarada, `.await` adentro de un body, o llamada al builtin
    /// `sleep`. Habilita el preludio `__fitz_sleep`, el `#[tokio::main]`
    /// sobre `fn main()` CLI, y el feature `time` en el Cargo.toml.
    /// Se setea en `generate_main_rs` antes de emit_prelude.
    uses_async: bool,
    /// Mini-tanda Fmt-build — `true` si el programa usa al menos un
    /// format spec que requiere helpers custom (`,`/`_` grouping,
    /// `%` percent, `c` char). Habilita la emisión de los helpers
    /// `__fitz_fmt_grouping`/`__fitz_fmt_percent`/`__fitz_fmt_char`
    /// en el preludio. Detectado en una pre-pasada (no afecta el
    /// resto del codegen si es false). Se setea durante `gen_str_interp`
    /// cuando aparece un FormatSpec con grouping o kind Char/Percent.
    uses_fmt_helpers: bool,
    /// F13 SPIKE — `true` si el programa usa al menos un literal
    /// heterogéneo (`List<Any>` o equivalente). Habilita la emisión
    /// del enum `__FitzValue` en el preludio + el mapping de
    /// `Type::List(Any)` a `Arc<Mutex<Vec<__FitzValue>>>`. Solo se
    /// setea cuando el codegen necesita emitir el wrapper.
    uses_fitz_value: bool,
    /// F13.C — `true` si el programa tiene al menos un decorator HTTP.
    /// Habilita emit de `__FromFitzJson` / `__ToFitzJson` for
    /// `__FitzValue` (requieren serde_json en scope, que solo se
    /// emite cuando hay HTTP).
    has_http: bool,
    /// Fase 8.7.1: `true` si el programa tiene al menos un import
    /// Python (`from python import X` / `import python.X`). Habilita
    /// el preludio Python (`__FitzPyObject` + helpers PyO3) y la
    /// emisión de bindings como vars locales del main body.
    uses_python: bool,
    /// Fase 9.w.1.d: `true` si el programa usa el módulo built-in
    /// `jwt`/`hash` (`jwt.encode(...)` / `hash.password(...)` etc.) o
    /// cualquier decorator de auth (`@auth_provider`/`@authenticated`/
    /// `@admin`). Habilita la emisión de los helpers
    /// `__fitz_jwt_encode/decode` / `__fitz_hash_password/verify` en el
    /// preludio y el wiring del wrapper auth alrededor de cada handler
    /// HTTP con `@authenticated`/`@admin`.
    uses_auth: bool,
    /// Fase 9.w.2.c: `true` si el programa tiene al menos un handler
    /// con `@ws("/path")`. Habilita la emisión de `WS_RUNTIME_PRELUDE`
    /// (struct `__FitzWsConn<T>` + broadcaster global + trait
    /// `__FitzWsMessage`) y el wiring del wrapper WS para cada `@ws`
    /// handler. Cuando es `false`, programas HTTP regulares no pagan
    /// el costo del bloque WS adicional en el binario.
    uses_ws: bool,
    /// Fase 9.w.1.d: nombre Rust de la fn marcada con `@auth_provider`
    /// (singleton). `None` si el programa no la tiene. El wrapper de
    /// cada handler con `@authenticated`/`@admin` la invoca antes del
    /// handler. Pre-scaneado en `generate_main_rs`.
    auth_provider_name: Option<String>,
    /// Fase 9.w.1.d: `true` si la fn `@auth_provider` es `async fn`.
    /// El wrapper la invoca con `.await` si es así, sync caller-side
    /// si no.
    auth_provider_is_async: bool,
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
    /// Mini-tanda Rt — contador para nombres únicos de bindings
    /// sintéticos en patterns (`__s_<n>`/`__n_<n>`/`__or_v_<n>`).
    /// Cada vez que `gen_pattern` necesita uno, lo incrementa. Sin
    /// reset entre arms — los nombres son scope-local del arm de
    /// todas formas, lo que importa es que sean únicos dentro del
    /// pattern del arm (especialmente cuando dos sub-patterns de un
    /// Tuple necesitan bindings sintéticos a la vez).
    pattern_slot_counter: usize,
    /// Mini-tanda F14 — set de consts top-level del propio módulo que
    /// se emitieron como `pub fn X() -> T` (accessor function) en
    /// lugar de `pub const X`. El codegen de Ident emite `X()` para
    /// estos nombres y `X` para los consts reales.
    accessor_consts: std::collections::HashSet<String>,
}

#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<Type>,
    ret: Type,
    /// Fp — defaults trailing por param. `defaults[i]` es la expr del
    /// default del param `i`, o `None` si no tiene. La regla del parser
    /// garantiza que los defaults son consecutivos al final, así que
    /// los `None`s aparecen antes que los `Some`s.
    defaults: Vec<Option<Expr>>,
    /// Fp.2 — si el último param es variádico (`...xs`). En el codegen
    /// se emite como `Vec<T>` por valor — el call site lo arma con
    /// `vec![]` macro a partir de los args extras.
    has_varargs: bool,
    /// Fp.3 — nombres de los params, en orden. Para el reorder de named
    /// args en el call site del codegen. Vacío para vars de tipo
    /// `Type::Function` (no llevan nombres).
    param_names: Vec<String>,
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
    /// Mini-tanda HTTP-Err — `true` si el `E` del `Result<T, E>` es un
    /// Nominal con field `status: Int`. Cuando es así, el wrapper emite
    /// código que lee `.status` del Instance Err y lo usa como HTTP
    /// status code (con fallback a 500 si está fuera de 100..1000).
    /// Sin status field → 500 histórico.
    err_has_status_field: bool,
    /// MW.3: nombres de las fns user-middleware Pre (gate-only, 1 arg)
    /// encadenadas en orden de declaración. Vacío si no hay
    /// `@middleware(fn)` con 1 param.
    mw_user_fns: Vec<String>,
    /// Mini-tanda P1 (Mw.next codegen): nombres de las fns user-middleware
    /// Post (post-process, 2 args `(Request, Response)`) en orden de
    /// declaración. Corren DESPUÉS del handler (en reverse) modificando
    /// la response final. Vacío si no hay middlewares Post.
    mw_user_fns_post: Vec<String>,
    /// MW.2/Q.3: config CORS si la ruta declara `@middleware(cors(...))`.
    mw_cors: Option<BuildCorsConfig>,
    has_middleware: bool,
    has_cors: bool,
    /// Fase 9.w.1.d — política de auth de la ruta (`@authenticated` /
    /// `@admin` o ninguno). `AuthSpec::None` (default) es ruta pública.
    auth: crate::http::AuthSpec,
    /// Fase 9.w.1.d — nombre del param del handler donde se inyecta el
    /// `user` retornado por el `@auth_provider`. `Some(name)` cuando
    /// `auth != None`; `None` cuando no hay auth. Identificado por
    /// regla "leftover" (el param que no es path/query/header).
    auth_user_param_name: Option<String>,
}

impl<'a> CodegenCtx<'a> {
    fn new(env: &'a TypeEnv, type_info: &'a crate::types::TypeInfo) -> Self {
        Self {
            env,
            type_info,
            output: String::new(),
            indent: 0,
            mode: GenMode::Main,
            scopes: vec![HashMap::new()],
            fn_sigs: HashMap::new(),
            type_sigs: HashMap::new(),
            fields_by_id: HashMap::new(),
            type_methods: HashMap::new(),
            own_consts: HashMap::new(),
            module_bindings: HashMap::new(),
            loaded_modules: Vec::new(),
            ret_stack: Vec::new(),
            break_value_stack: Vec::new(),
            state_var_types: HashMap::new(),
            fn_state_deps: HashMap::new(),
            response_mode: false,
            in_middleware_fn: false,
            http_handlers_returning_response: std::collections::HashSet::new(),
            middleware_fn_names: std::collections::HashSet::new(),
            middleware_post_fn_names: std::collections::HashSet::new(),
            uses_async: false,
            uses_fmt_helpers: false,
            uses_fitz_value: false,
            has_http: false,
            uses_python: false,
            uses_auth: false,
            auth_provider_name: None,
            auth_provider_is_async: false,
            uses_ws: false,
            python_bindings: HashMap::new(),
            python_imports_ordered: Vec::new(),
            pattern_slot_counter: 0,
            accessor_consts: std::collections::HashSet::new(),
            hoisted_main_lets: HashMap::new(),
        }
    }

    fn new_for_module(env: &'a TypeEnv, type_info: &'a crate::types::TypeInfo) -> Self {
        let mut ctx = Self::new(env, type_info);
        ctx.mode = GenMode::Module;
        ctx
    }

    fn pub_prefix(&self) -> &'static str {
        match self.mode {
            GenMode::Main => "",
            GenMode::Module => "pub ",
        }
    }

    /// F15 — Prefix Rust para referenciar otros módulos por path
    /// absoluto desde adentro del archivo actual. En `main.rs` los
    /// módulos están declarados como `mod foo;` y son accesibles
    /// directo (`foo::greet`). En un módulo del crate (`src/foo.rs`),
    /// los demás módulos viven al lado en crate root, así que hay
    /// que prefijarlos con `crate::` (`crate::bar::greet`).
    fn mod_path_prefix(&self) -> &'static str {
        match self.mode {
            GenMode::Main => "",
            GenMode::Module => "crate::",
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
                accessor_consts: m.accessor_consts.clone(),
                type_methods: m.type_methods.clone(),
            });
        }
        for (name, binding) in &loader.bindings {
            self.module_bindings.insert(name.clone(), binding.clone());
        }
    }

    /// F15 — Emite `use crate::<other>::<item>` para cada Named binding
    /// del módulo. Paralelo a `ModuleLoader::emit_use_decls` pero usa
    /// el prefix `crate::` y itera sobre los bindings locales del
    /// módulo en vez de los del importer principal. Para Namespace
    /// bindings (`import foo`) no emitimos `use` — las referencias
    /// `foo::greet` ya funcionan con `crate::foo::greet` (ver
    /// `resolve_namespace_field`).
    fn emit_module_use_decls(
        &mut self,
        local_bindings: &HashMap<String, ResolvedBinding>,
        loaded_modules: &[LoadedModule],
    ) {
        let mut entries: Vec<(&String, &ResolvedBinding)> = local_bindings.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut emitted_any = false;
        for (local, binding) in entries {
            if let ResolvedBinding::Named { module_index, item, kind } = binding {
                let mod_name = &loaded_modules[*module_index].mod_name;
                let needs_alias = local != item;
                match kind {
                    NamedKind::Type => {
                        if needs_alias {
                            self.output.push_str(&format!(
                                "use crate::{mod}::{{{item} as {local}, {item}Data as {local}Data}};\n",
                                mod = mod_name,
                                item = item,
                                local = local,
                            ));
                        } else {
                            self.output.push_str(&format!(
                                "use crate::{mod}::{{{item}, {item}Data}};\n",
                                mod = mod_name,
                                item = item,
                            ));
                        }
                    }
                    NamedKind::Fn | NamedKind::Const => {
                        if needs_alias {
                            self.output.push_str(&format!(
                                "use crate::{}::{} as {};\n",
                                mod_name, item, local
                            ));
                        } else {
                            self.output.push_str(&format!(
                                "use crate::{}::{};\n",
                                mod_name, item
                            ));
                        }
                    }
                }
                emitted_any = true;
            }
        }
        if emitted_any {
            self.output.push('\n');
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
        let prefix = self.mod_path_prefix();
        if let Some(sig) = m.fn_sigs.get(field) {
            Some((
                format!("{}{}::{}", prefix, m.mod_name, field),
                Type::Function {
                    params: sig.params.clone(),
                    ret: Box::new(sig.ret.clone()),
                },
            ))
        } else {
            m.const_sigs.get(field).map(|ty| {
                // F14: si el const es accessor fn, lo invocamos en el
                // call site (`mod::X()`). Para `pub const`/`pub static`
                // emitimos `mod::X` directo.
                let code = if m.accessor_consts.contains(field) {
                    format!("{}{}::{}()", prefix, m.mod_name, field)
                } else {
                    format!("{}{}::{}", prefix, m.mod_name, field)
                };
                (code, ty.clone())
            })
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
        let prefix = self.mod_path_prefix();
        m.fn_sigs
            .get(fn_name)
            .map(|sig| (format!("{}{}::{}", prefix, m.mod_name, fn_name), sig.clone()))
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

    // Mini-tanda Cd — resuelve un ident a una firma de callback
    // `(params, ret)` cuando se usa como higher-order arg de un
    // método (map/filter/find/reduce/etc.). Busca primero las fns
    // top-level del archivo principal (`fn_sigs`); como fallback,
    // variables locales con tipo `Function { params, ret }` (caso
    // `let f = fn(n) => ...; xs.map(f)`). Devuelve `None` si no es
    // callable bajo ninguna fuente.
    fn resolve_named_callback(&self, name: &str) -> Option<(Vec<Type>, Type)> {
        if let Some(sig) = self.fn_sigs.get(name) {
            return Some((sig.params.clone(), sig.ret.clone()));
        }
        if let Some(Type::Function { params, ret }) = self.lookup_var(name) {
            return Some((params.clone(), (**ret).clone()));
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

    /// Fase 9.w.1.d — Emite el preludio de helpers para auth nativa:
    /// `__fitz_jwt_encode/__fitz_jwt_decode/__fitz_hash_password/
    /// __fitz_hash_verify`. Solo se emite cuando `uses_auth` es true
    /// (programa usa `jwt.*`/`hash.*` o algún decorator de auth). Sin
    /// uses_auth, los helpers no se emiten y los Cargo.toml deps
    /// quedan fuera — programas sin auth no pagan el costo.
    ///
    /// Política de los helpers (paralela a `register_builtins` del
    /// intérprete, 9.w.1.b):
    /// - Encode: payload `Map<Str, Str>` strict por MVP (heterogéneos
    ///   en codegen requieren `__FitzValue` integration, post-MVP).
    ///   Secret/alg como Str. Default alg HS256, también HS384/HS512.
    ///   Encode panic en fallo (error en build-time, shouldn't happen
    ///   con args válidos).
    /// - Decode: token+secret+alg Str. Devuelve `Result<Map<Str, Str>,
    ///   String>` con claims serializados como Str. Cualquier falla
    ///   (token malformado, signature inválida, expirado) → `Err(msg)`.
    /// - hash.password/verify igual que intérprete: Argon2id con
    ///   params default OWASP, output PHC string, verify devuelve Bool
    ///   (no Result — hash malformado → false por seguridad).
    fn emit_auth_prelude(&mut self) {
        if !self.uses_auth {
            return;
        }
        // jwt.encode: payload `Arc<Mutex<Vec<(String, String)>>>` →
        // `serde_json::Value::Object` → JWT firmado.
        self.emit(
            "/// 9.w.1.d — `jwt.encode(payload, secret, alg)` codegen helper.\n\
             /// payload restringido a `Map<Str, Str>` en MVP — heterogéneos\n\
             /// requieren `__FitzValue` integration, post-MVP.\n\
             fn __fitz_jwt_encode(\n    \
                 payload: Arc<Mutex<Vec<(String, String)>>>,\n    \
                 secret: String,\n    \
                 alg: Option<String>,\n\
             ) -> String {\n    \
                 let mut claims = serde_json::Map::new();\n    \
                 {\n        \
                     let guard = payload.lock().unwrap();\n        \
                     for (k, v) in guard.iter() {\n            \
                         claims.insert(k.clone(), serde_json::Value::String(v.clone()));\n        \
                         }\n    \
                 }\n    \
                 let alg_str = alg.as_deref().unwrap_or(\"HS256\");\n    \
                 let algorithm = match alg_str {\n        \
                     \"HS256\" => jsonwebtoken::Algorithm::HS256,\n        \
                     \"HS384\" => jsonwebtoken::Algorithm::HS384,\n        \
                     \"HS512\" => jsonwebtoken::Algorithm::HS512,\n        \
                     other => panic!(\"`jwt.encode`: alg `{}` no soportado en MVP. Soportados: HS256, HS384, HS512.\", other),\n    \
                 };\n    \
                 let header = jsonwebtoken::Header::new(algorithm);\n    \
                 let key = jsonwebtoken::EncodingKey::from_secret(secret.as_bytes());\n    \
                 jsonwebtoken::encode(&header, &serde_json::Value::Object(claims), &key)\n        \
                     .unwrap_or_else(|e| panic!(\"`jwt.encode`: fallo al firmar: {}\", e))\n\
             }\n\n",
        );
        // jwt.decode: verifica + decodifica → `Result<Map<Str, Str>,
        // String>`. Los claims se serializan a Str (numbers/bools
        // pasan por `to_string()`).
        self.emit(
            "/// 9.w.1.d — `jwt.decode(token, secret, alg)` codegen helper.\n\
             /// Devuelve `Result<Map<Str, Str>, String>` con claims como Str.\n\
             /// Cualquier falla → `Err` con mensaje del crate jsonwebtoken.\n\
             fn __fitz_jwt_decode(\n    \
                 token: String,\n    \
                 secret: String,\n    \
                 alg: Option<String>,\n\
             ) -> Result<Arc<Mutex<Vec<(String, String)>>>, String> {\n    \
                 let alg_str = alg.as_deref().unwrap_or(\"HS256\");\n    \
                 let algorithm = match alg_str {\n        \
                     \"HS256\" => jsonwebtoken::Algorithm::HS256,\n        \
                     \"HS384\" => jsonwebtoken::Algorithm::HS384,\n        \
                     \"HS512\" => jsonwebtoken::Algorithm::HS512,\n        \
                     other => return Err(format!(\"`jwt.decode`: alg `{}` no soportado en MVP\", other)),\n    \
                 };\n    \
                 let key = jsonwebtoken::DecodingKey::from_secret(secret.as_bytes());\n    \
                 let mut validation = jsonwebtoken::Validation::new(algorithm);\n    \
                 validation.required_spec_claims.clear();\n    \
                 match jsonwebtoken::decode::<serde_json::Value>(&token, &key, &validation) {\n        \
                     Ok(data) => {\n            \
                         let mut out: Vec<(String, String)> = Vec::new();\n            \
                         if let serde_json::Value::Object(obj) = data.claims {\n                \
                             for (k, v) in obj.iter() {\n                    \
                                 let s = match v {\n                        \
                                     serde_json::Value::String(s) => s.clone(),\n                        \
                                     other => other.to_string(),\n                    \
                                 };\n                    \
                                 out.push((k.clone(), s));\n                \
                             }\n            \
                         }\n            \
                         Ok(Arc::new(Mutex::new(out)))\n        \
                     }\n        \
                     Err(e) => Err(e.to_string()),\n    \
                 }\n\
             }\n\n",
        );
        // hash.password: Argon2id con salt random + params default.
        self.emit(
            "/// 9.w.1.d — `hash.password(plain)` codegen helper. Argon2id\n\
             /// con salt random + params default (OWASP). Output PHC string.\n\
             fn __fitz_hash_password(plain: String) -> String {\n    \
                 use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};\n    \
                 let salt = SaltString::generate(&mut OsRng);\n    \
                 let argon2 = argon2::Argon2::default();\n    \
                 argon2.hash_password(plain.as_bytes(), &salt)\n        \
                     .map(|h| h.to_string())\n        \
                     .unwrap_or_else(|e| panic!(\"`hash.password`: fallo al hashear: {}\", e))\n\
             }\n\n",
        );
        // hash.verify: devuelve Bool (no Result). Hash malformado → false.
        self.emit(
            "/// 9.w.1.d — `hash.verify(plain, hashed)` codegen helper.\n\
             /// Hash malformado o mismatch → false (no panic) por seguridad.\n\
             fn __fitz_hash_verify(plain: String, hashed: String) -> bool {\n    \
                 use argon2::password_hash::{PasswordHash, PasswordVerifier};\n    \
                 let parsed = match PasswordHash::new(&hashed) {\n        \
                     Ok(p) => p,\n        \
                     Err(_) => return false,\n    \
                 };\n    \
                 argon2::Argon2::default()\n        \
                     .verify_password(plain.as_bytes(), &parsed)\n        \
                     .is_ok()\n\
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
        // Mini-tanda Bytes — formato `b\"...\"` paralelo al Display de
        // `Value::Bytes` del intérprete. Cada archivo emite su propio
        // `__fitz_fmt_bytes`; no compartimos cross-module por simetría
        // con `__fitz_fmt_float`.
        self.emit(
            "fn __fitz_fmt_bytes(bs: &[u8]) -> String {\n    \
             let mut out = String::from(\"b\\\"\");\n    \
             for &b in bs.iter() {\n        \
                 match b {\n            \
                     b'\\\\' => out.push_str(\"\\\\\\\\\"),\n            \
                     b'\\\"' => out.push_str(\"\\\\\\\"\"),\n            \
                     b'\\n' => out.push_str(\"\\\\n\"),\n            \
                     b'\\r' => out.push_str(\"\\\\r\"),\n            \
                     b'\\t' => out.push_str(\"\\\\t\"),\n            \
                     0x20..=0x7e => out.push(b as char),\n            \
                     _ => out.push_str(&format!(\"\\\\x{:02x}\", b)),\n        \
                 }\n    \
             }\n    \
             out.push('\\\"');\n    \
             out\n}\n\n",
        );
        // F13 SPIKE — enum `__FitzValue` para listas/mapas
        // heterogéneos. Solo se emite cuando el programa usa al
        // menos un literal `List<Any>` (auto-detectado en
        // `gen_list_lit`). Variantes mínimas del SPIKE:
        // Int/Float/Str/Bool/Null. Bytes/List/Map/Nominal quedan
        // como follow-up dedicado. Display paralelo al
        // `fmt::Display for Value` del intérprete (strings con
        // comillas adentro de colecciones via `__fitz_fmt_value_inline`,
        // Float con `.0` si fract=0 via `__fitz_fmt_float`).
        if self.uses_fitz_value {
            // F13.A + F13.B — variantes extendidas:
            //   - Bytes(Vec<u8>): Bytes adentro de heterogéneos.
            //   - Nominal(String): captura el Display del nominal
            //     como String. Trade-off SPIKE/F13.B: pierde field
            //     access tipado en heterogéneos pero evita la
            //     dependencia en `serde_json` (que solo se emite
            //     cuando hay rutas HTTP). El usuario que necesite
            //     acceso tipado debe sacar el item con type check
            //     dinámico (follow-up F13.D) o serializar/
            //     deserializar via JSON manual.
            self.emit(
                "#[derive(Clone, Debug)]\n\
                 #[allow(dead_code)]\n\
                 enum __FitzValue {\n    \
                     Int(i64),\n    \
                     Float(f64),\n    \
                     Str(String),\n    \
                     Bool(bool),\n    \
                     Null,\n    \
                     Bytes(Vec<u8>),\n    \
                     Nominal(String),\n    \
                     List(Vec<__FitzValue>),\n    \
                     Map(Vec<(__FitzValue, __FitzValue)>),\n\
                 }\n\n\
                 impl std::fmt::Display for __FitzValue {\n    \
                     fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {\n        \
                         match self {\n            \
                             Self::Int(n) => write!(f, \"{}\", n),\n            \
                             Self::Float(x) => write!(f, \"{}\", __fitz_fmt_float(*x)),\n            \
                             Self::Str(s) => write!(f, \"\\\"{}\\\"\", s),\n            \
                             Self::Bool(b) => write!(f, \"{}\", b),\n            \
                             Self::Null => write!(f, \"null\"),\n            \
                             Self::Bytes(bs) => write!(f, \"{}\", __fitz_fmt_bytes(bs)),\n            \
                             Self::Nominal(repr) => write!(f, \"{}\", repr),\n            \
                             Self::List(items) => {\n                \
                                 write!(f, \"[\")?;\n                \
                                 for (i, it) in items.iter().enumerate() {\n                    \
                                     if i > 0 { write!(f, \", \")?; }\n                    \
                                     write!(f, \"{}\", it)?;\n                \
                                 }\n                \
                                 write!(f, \"]\")\n            \
                             }\n            \
                             Self::Map(pairs) => {\n                \
                                 write!(f, \"{{\")?;\n                \
                                 for (i, (k, v)) in pairs.iter().enumerate() {\n                    \
                                     if i > 0 { write!(f, \", \")?; }\n                    \
                                     write!(f, \"{}: {}\", k, v)?;\n                \
                                 }\n                \
                                 write!(f, \"}}\")\n            \
                             }\n        \
                         }\n    \
                     }\n\
                 }\n\n\
                 impl PartialEq for __FitzValue {\n    \
                     fn eq(&self, other: &Self) -> bool {\n        \
                         match (self, other) {\n            \
                             (Self::Int(a), Self::Int(b)) => a == b,\n            \
                             (Self::Float(a), Self::Float(b)) => a == b,\n            \
                             (Self::Int(a), Self::Float(b)) => (*a as f64) == *b,\n            \
                             (Self::Float(a), Self::Int(b)) => *a == (*b as f64),\n            \
                             (Self::Str(a), Self::Str(b)) => a == b,\n            \
                             (Self::Bool(a), Self::Bool(b)) => a == b,\n            \
                             (Self::Null, Self::Null) => true,\n            \
                             (Self::Bytes(a), Self::Bytes(b)) => a == b,\n            \
                             (Self::Nominal(a), Self::Nominal(b)) => a == b,\n            \
                             (Self::List(a), Self::List(b)) => a == b,\n            \
                             (Self::Map(a), Self::Map(b)) => a == b,\n            \
                             _ => false,\n        \
                         }\n    \
                     }\n\
                 }\n\n\
                 #[allow(dead_code)]\n\
                 fn __fv_type_name(v: &__FitzValue) -> &'static str {\n    \
                     match v {\n        \
                         __FitzValue::Int(_) => \"Int\",\n        \
                         __FitzValue::Float(_) => \"Float\",\n        \
                         __FitzValue::Str(_) => \"Str\",\n        \
                         __FitzValue::Bool(_) => \"Bool\",\n        \
                         __FitzValue::Null => \"Null\",\n        \
                         __FitzValue::Bytes(_) => \"Bytes\",\n        \
                         __FitzValue::Nominal(_) => \"Instance\",\n        \
                         __FitzValue::List(_) => \"List\",\n        \
                         __FitzValue::Map(_) => \"Map\",\n    \
                     }\n\
                 }\n\n",
            );
            // F13.C — `__FromFitzJson` y `__ToFitzJson` para
            // `__FitzValue`. Solo si HTTP está activo (serde_json
            // y los traits requieren el preludio HTTP). Habilita
            // `body: List<Any>` / `Map<Str, Any>` deserializando
            // desde JSON entrante. Conversión por shape:
            //   - Null → Null
            //   - Bool → Bool
            //   - Number → Int (sin frac) o Float (con frac)
            //   - String → Str
            //   - Array → List recursivo
            //   - Object → Map recursivo
            // Bytes/Nominal NO se decodean desde JSON acá (Bytes
            // requeriría hint de base64; Nominal requiere type
            // knowledge — sub-paso futuro).
            if self.has_http {
                self.emit(
                    "#[allow(dead_code)]\n\
                     impl __FromFitzJson for __FitzValue {\n    \
                         fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {\n        \
                             match json {\n            \
                                 serde_json::Value::Null => Ok(__FitzValue::Null),\n            \
                                 serde_json::Value::Bool(b) => Ok(__FitzValue::Bool(*b)),\n            \
                                 serde_json::Value::Number(n) => {\n                \
                                     if let Some(i) = n.as_i64() { Ok(__FitzValue::Int(i)) }\n                \
                                     else if let Some(f) = n.as_f64() { Ok(__FitzValue::Float(f)) }\n                \
                                     else { Err(format!(\"número JSON fuera de rango: {}\", n)) }\n            \
                                 }\n            \
                                 serde_json::Value::String(s) => Ok(__FitzValue::Str(s.clone())),\n            \
                                 serde_json::Value::Array(arr) => {\n                \
                                     let items: Result<Vec<_>, _> = arr.iter().map(__FitzValue::__from_fitz_json).collect();\n                \
                                     items.map(__FitzValue::List)\n            \
                                 }\n            \
                                 serde_json::Value::Object(obj) => {\n                \
                                     let mut pairs = Vec::with_capacity(obj.len());\n                \
                                     for (k, v) in obj.iter() {\n                    \
                                         let k_fv = __FitzValue::Str(k.clone());\n                    \
                                         let v_fv = __FitzValue::__from_fitz_json(v)?;\n                    \
                                         pairs.push((k_fv, v_fv));\n                \
                                     }\n                \
                                     Ok(__FitzValue::Map(pairs))\n            \
                                 }\n        \
                             }\n    \
                         }\n\
                     }\n\n\
                     #[allow(dead_code)]\n\
                     impl __ToFitzJson for __FitzValue {\n    \
                         fn __to_fitz_json(&self) -> serde_json::Value {\n        \
                             match self {\n            \
                                 __FitzValue::Int(n) => serde_json::Value::from(*n),\n            \
                                 __FitzValue::Float(x) => serde_json::Number::from_f64(*x).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),\n            \
                                 __FitzValue::Str(s) => serde_json::Value::String(s.clone()),\n            \
                                 __FitzValue::Bool(b) => serde_json::Value::Bool(*b),\n            \
                                 __FitzValue::Null => serde_json::Value::Null,\n            \
                                 __FitzValue::Bytes(bs) => serde_json::Value::String(__fv_b64_encode(bs)),\n            \
                                 __FitzValue::Nominal(s) => serde_json::Value::String(s.clone()),\n            \
                                 __FitzValue::List(items) => serde_json::Value::Array(items.iter().map(__ToFitzJson::__to_fitz_json).collect()),\n            \
                                 __FitzValue::Map(pairs) => {\n                \
                                     let mut obj = serde_json::Map::new();\n                \
                                     for (k, v) in pairs.iter() {\n                    \
                                         let key = match k {\n                        \
                                             __FitzValue::Str(s) => s.clone(),\n                        \
                                             other => format!(\"{}\", other),\n                    \
                                         };\n                    \
                                         obj.insert(key, v.__to_fitz_json());\n                \
                                     }\n                \
                                     serde_json::Value::Object(obj)\n            \
                                 }\n        \
                             }\n    \
                         }\n\
                     }\n\n\
                     #[allow(dead_code)]\n\
                     fn __fv_b64_encode(bytes: &[u8]) -> String {\n    \
                         const T: &[u8; 64] = b\"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\";\n    \
                         let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);\n    \
                         for c in bytes.chunks(3) {\n        \
                             let b0 = c[0];\n        \
                             let b1 = if c.len() > 1 { c[1] } else { 0 };\n        \
                             let b2 = if c.len() > 2 { c[2] } else { 0 };\n        \
                             let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);\n        \
                             out.push(T[((triple >> 18) & 0x3f) as usize] as char);\n        \
                             out.push(T[((triple >> 12) & 0x3f) as usize] as char);\n        \
                             if c.len() > 1 { out.push(T[((triple >> 6) & 0x3f) as usize] as char); } else { out.push('='); }\n        \
                             if c.len() > 2 { out.push(T[(triple & 0x3f) as usize] as char); } else { out.push('='); }\n    \
                         }\n    \
                         out\n\
                     }\n\n",
                );
            }
        }
        // Mini-tanda Fmt-build — helpers para los format specs que
        // Rust no soporta nativamente: `,`/`_` grouping, `%` percent,
        // `c` char. Solo se emiten cuando el programa los usa (gating
        // via `uses_fmt_helpers`).
        if self.uses_fmt_helpers {
            self.emit(
                "fn __fitz_fmt_grouping(n: i64, sep: char) -> String {\n    \
                 let abs = (n as i128).unsigned_abs();\n    \
                 let s = abs.to_string();\n    \
                 let mut out: Vec<char> = Vec::with_capacity(s.len() + s.len() / 3);\n    \
                 for (i, c) in s.chars().rev().enumerate() {\n        \
                     if i > 0 && i % 3 == 0 { out.push(sep); }\n        \
                     out.push(c);\n    \
                 }\n    \
                 let mut rev: String = out.into_iter().rev().collect();\n    \
                 if n < 0 { rev.insert(0, '-'); }\n    \
                 rev\n}\n\n",
            );
            self.emit(
                "fn __fitz_fmt_percent(x: f64, precision: usize) -> String {\n    \
                 format!(\"{:.*}%\", precision, x * 100.0)\n}\n\n",
            );
            self.emit(
                "fn __fitz_fmt_char(n: i64) -> String {\n    \
                 char::from_u32(n as u32).map(|c| c.to_string()).unwrap_or_else(|| format!(\"\\\\u{{{:x}}}\", n))\n}\n\n",
            );
            // Mini-tanda Fmt-g — general format `g`/`G`: bit-a-bit con
            // `src/format.rs::general_format` del intérprete.
            //   - precision = 0 → 1 (paralelo a Python).
            //   - exp = floor(log10(abs(x))) — categoriza la magnitud.
            //   - use_exp = exp < -4 || exp >= precision.
            //   - exp branch: precision - 1 después del punto, NO strip.
            //   - fixed branch: precision - 1 - exp dígitos después,
            //     CON strip de ceros trailing.
            //   - upper → uppercase ('e' → 'E').
            self.emit(
                "fn __fitz_fmt_general(x: f64, precision: usize, upper: bool) -> String {\n    \
                 let p = precision.max(1);\n    \
                 if x == 0.0 { return if upper { \"0\".to_string() } else { \"0\".to_string() }; }\n    \
                 let abs = x.abs();\n    \
                 let exp = abs.log10().floor() as i32;\n    \
                 let use_exp = exp < -4 || exp >= p as i32;\n    \
                 let result = if use_exp {\n        \
                     let after = p.saturating_sub(1);\n        \
                     format!(\"{:.*e}\", after, x)\n    \
                 } else {\n        \
                     let after = (p as i32 - 1 - exp).max(0) as usize;\n        \
                     let s = format!(\"{:.*}\", after, x);\n        \
                     if s.contains('.') {\n            \
                         let t = s.trim_end_matches('0').trim_end_matches('.');\n            \
                         if t.is_empty() || t == \"-\" { \"0\".to_string() } else { t.to_string() }\n        \
                     } else { s }\n    \
                 };\n    \
                 if upper { result.to_uppercase() } else { result }\n}\n\n",
            );
        }
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
        //
        // Mini-tanda Cd (F12 fix) — skipear los `Stmt::Assign(Ident(name))`
        // cuyos names fueron hoisteados a const/static top-level. El
        // hoist los emitió antes del `fn main()`, así que ya están
        // accesibles como bindings Rust globales. Re-emitirlos como
        // locales sería redundante (e incompatible cuando declared con
        // un tipo Rust distinto).
        for stmt in stmts {
            if let Stmt::Assign { target: AssignTarget::Ident(name), .. } = stmt {
                if self.hoisted_main_lets.contains_key(name) {
                    continue;
                }
            }
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
            let Stmt::TypeDef { name, fields: ast_fields, methods, .. } = stmt else { continue };
            // R.3 — pre-registrar métodos custom por nombre del tipo.
            if !methods.is_empty() {
                self.type_methods.insert(name.clone(), methods.clone());
            }
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
            let (module_sig, module_type_methods) = {
                let m = self.loaded_modules.get(module_index).ok_or_else(|| {
                    self.err(format!("módulo no cargado al registrar `{}`", item))
                })?;
                let sig = m.type_sigs.get(&item).cloned().ok_or_else(|| {
                    self.err(format!("el módulo no expone el tipo `{}`", item))
                })?;
                let methods = m.type_methods.get(&item).cloned();
                (sig, methods)
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
            // Mini-tanda CM — copiar métodos del tipo importado. El
            // dispatch `instance.method()` busca en `type_methods` por
            // el nombre LOCAL del tipo (que puede ser un alias via
            // `from foo import User as Person`); ahí los registramos.
            if let Some(methods) = module_type_methods {
                self.type_methods.insert(name.clone(), methods);
            }
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
            // Mini-tanda F14 — flagear como accessor fn los consts cuya
            // RHS NO es const-eval (StrInterp, Call, StructLit, etc.).
            // El codegen del Ident emite `X()` para esos vs `X` para
            // pub const.
            if !is_literal_expr(value) && !is_const_eval_expr(value) {
                self.accessor_consts.insert(name.clone());
            }
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
                body,
                is_async,
                ..
            } = stmt
            {
                let fn_span = stmt.span();
                let params_tys: Vec<Type> = params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| self.resolve_param_type(
                        name, &p.name, p.type_.as_ref(), fn_span, i, program,
                    ))
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
                    // Mini-tanda Hpx.2 — sin anotación: inferir del body
                    // walkeando los `Stmt::Return` y consultando TypeInfo
                    // del checker. Si no hay returns explícitos, fallback
                    // a Null (igual al comportamiento histórico).
                    None => infer_return_type_from_body(body, self.type_info)
                        .unwrap_or(Type::Null),
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
                let defaults: Vec<Option<Expr>> = params.iter().map(|p| p.default.clone()).collect();
                let has_varargs = params.last().map(|p| p.varargs).unwrap_or(false);
                let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                self.fn_sigs.insert(
                    name.clone(),
                    FnSig { params: params_tys, ret, defaults, has_varargs, param_names },
                );
            }
        }

        // Mini-tanda P1 — post-scan: clasificar middlewares por aridad.
        // Si un middleware tiene 2 params, lo movemos de
        // `middleware_fn_names` a `middleware_post_fn_names` para que
        // `gen_top_fn` emita la firma correcta (__FitzResponse en lugar
        // de Option<__FitzResponse>).
        let post_mws: Vec<String> = self
            .middleware_fn_names
            .iter()
            .filter(|n| {
                self.fn_sigs
                    .get(n.as_str())
                    .map(|s| s.params.len() == 2)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        for n in post_mws {
            self.middleware_fn_names.remove(&n);
            self.middleware_post_fn_names.insert(n);
        }
        Ok(())
    }

    fn resolve_param_type(
        &self,
        fn_name: &str,
        param_name: &str,
        type_: Option<&TypeExpr>,
        fn_span: crate::ast::Span,
        param_idx: usize,
        program: &Program,
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
            // Mini-tanda 5b.1 — inferencia de tipos de params para fns
            // sin anotación. Estrategia "first call site": scan el
            // programa por `fn_name(args)` calls; del primer call
            // encontrado, consultar el tipo del arg `param_idx` en
            // TypeInfo. Si el tipo es concreto, usarlo. Sino, error
            // claro con sugerencia.
            None => {
                if let Some(inferred) = infer_param_type_from_call_sites(
                    program, fn_name, param_idx, self.type_info,
                ) {
                    return Ok(inferred);
                }
                Err(self.err_at(fn_span, format!(
                    "fn `{}`: el parámetro `{}` necesita una anotación de tipo (5b.1) — \
                     el codegen no pudo inferirlo desde call sites. Workaround: anotar \
                     manualmente (`{}: Str`, `{}: Int`, etc.).",
                    fn_name, param_name, param_name, param_name,
                )))
            }
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

        // R.3 — `impl <Foo>Data { pub fn metodo(&self, ...) ... }`
        // con los métodos custom declarados. Los fields del tipo se
        // pre-bindean como `let <field> = self.<field>.clone();`
        // dentro del body de cada método, para que las referencias a
        // fields sin `self.` funcionen ("opción A": fields como
        // locales).
        let methods = self.type_methods.get(name).cloned().unwrap_or_default();
        if !methods.is_empty() {
            writeln!(&mut self.output, "impl {} {{", data_name).unwrap();
            for m in &methods {
                self.emit_custom_method(name, &sig, m)?;
            }
            self.emit("}\n\n");
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
        // Mini-tanda P1 (Mw.next codegen) — middleware Post: return
        // type es `__FitzResponse` directo (no Option). El body siempre
        // termina en `Stmt::ReturnStatus` (devuelve Response).
        let is_middleware_post = self.middleware_post_fn_names.contains(name);
        let has_return_status_inner = contains_return_status_stmts(body);

        // Status codes custom: si la fn HTTP contiene al menos un
        // `Stmt::ReturnStatus`, su return type Rust pasa a ser
        // `__FitzResponse` (en vez del declarado) y todos los returns
        // se envuelven. El handler wrapper lo detecta vía la tabla
        // `http_handlers_returning_response` para emitir el destructuring
        // apropiado. Para middlewares Pre (MW.3) reusamos el mismo flag
        // `response_mode` para envolver `Stmt::ReturnStatus`, pero la
        // emisión final difiere — los middlewares Pre retornan
        // `Option<__FitzResponse>`, no `__FitzResponse`. Para Post mws
        // (P1) el wrapping también aplica, pero la return type emitida
        // es __FitzResponse directo.
        let has_return_status =
            (is_http_handler || is_middleware || is_middleware_post) && has_return_status_inner;
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
            // Mini-tanda P1 (Mw.next codegen) — post mw segundo param
            // (`res: Response`) se emite como `__FitzResponse` para que
            // el call site del wrapper pueda pasarlo directo. El body
            // no debería acceder a `res.field` — el caso típico es
            // `return <status> { ... }` que reconstruye la response.
            let rust_ty = if is_middleware_post && i == 1 {
                "__FitzResponse".to_string()
            } else if param.varargs {
                // Fp.2 — varargs: el tipo en `sig.params` es el elemento T
                // (para que el call-site valide cada arg contra T), pero la
                // fn recibe `List<T>` (`Arc<Mutex<Vec<T>>>`). Wrap en List
                // al emitir el tipo Rust del param.
                rust_type_for(&Type::List(Box::new(pty.clone())), self.env)?
            } else {
                rust_type_for(pty, self.env)?
            };
            self.emit(&rust_ty);
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
            // MW.3: middlewares Pre retornan Option<__FitzResponse>,
            // sin importar el return type declarado por el usuario. El
            // checker ya validó la signatura (Request param, retorno
            // implícito `()` o `Response?` decorativo).
            self.emit(" -> Option<__FitzResponse>");
        } else if is_middleware_post {
            // Mini-tanda P1 (Mw.next codegen) — middleware Post retorna
            // __FitzResponse directo. Siempre tiene un Stmt::ReturnStatus
            // que construye la response final.
            self.emit(" -> __FitzResponse");
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
            // Fp.2 — varargs param se ve como `List<T>` adentro del body.
            let bind_ty = if param.varargs {
                Type::List(Box::new(pty.clone()))
            } else {
                pty.clone()
            };
            self.declare_var(param.name.clone(), bind_ty);
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
        // P1 — Post mws: response_mode true para wrappear ReturnStatus
        // como __FitzResponse, pero in_middleware_fn false para que
        // el wrap NO use Some(...). (Pre mws usan in_middleware_fn=true
        // que envuelve en Some(...).)
        self.response_mode = has_return_status || is_middleware_post;
        self.in_middleware_fn = is_middleware;
        for stmt in body {
            self.gen_stmt_in_fn(stmt, &emit_ret)?;
        }
        // MW.3: tail-fall del body de un middleware Pre sin return
        // explícito. El return type es `Option<__FitzResponse>` y el
        // body cae al final sin generar `None;`. Rust quejaría con
        // "expected Option<...>, found ()". Emitimos `None` siempre —
        // si el body ya hizo un return explícito esto es código muerto
        // que rustc elimina sin warning. Post mws (P1) NO necesitan
        // tail-fall — siempre tienen return explícito por construcción
        // (response_mode true exige ReturnStatus).
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
            Stmt::Destructure { pattern, value, .. } => self.gen_destructure(pattern, value),
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
            Stmt::While { condition, body, label, .. } => self.gen_while(condition, body, label.as_deref(), ret_expected),
            Stmt::Loop { body, label, .. } => self.gen_loop(body, label.as_deref(), ret_expected),
            Stmt::For { var, iter, body, label, .. } => self.gen_for(var, iter, body, label.as_deref(), ret_expected),
            Stmt::Break(value, label, _) => {
                // Mini-tanda L: `break ['label] [<expr>]` emite
                // `break ['label] [code];` Rust nativo. Rust soporta
                // ambas formas con label antes del valor (igual que
                // nuestra sintaxis). Adicionalmente alimentamos el
                // `break_value_stack` del frame activo (si hay) para
                // que el `Expr::Loop` contenedor sepa el tipo del v.
                self.emit_indent();
                let label_str = label
                    .as_ref()
                    .map(|l| format!(" '{}", l))
                    .unwrap_or_default();
                if let Some(e) = value {
                    let (code, ty) = self.gen_expr(e)?;
                    if let Some(frame) = self.break_value_stack.last_mut() {
                        frame.push(ty);
                    }
                    writeln!(&mut self.output, "break{} {};", label_str, code).unwrap();
                } else {
                    if let Some(frame) = self.break_value_stack.last_mut() {
                        frame.push(Type::Null);
                    }
                    writeln!(&mut self.output, "break{};", label_str).unwrap();
                }
                Ok(())
            }
            Stmt::Continue(label, _) => {
                self.emit_indent();
                if let Some(l) = label {
                    writeln!(&mut self.output, "continue '{};", l).unwrap();
                } else {
                    self.emit("continue;\n");
                }
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

    /// Mini-tanda T + Lt — `let <pattern> = expr`. Dos caminos:
    ///
    /// - **Pure irrefutable** (solo Ident/Wildcard/Tuple recursivo):
    ///   emite `let <pat> = <expr>;` directo, sin match wrapper. Cero
    ///   overhead — Rust irrefutable pattern.
    /// - **Rico/refutable** (literales/ranges/Or/Ok/Err en algún
    ///   slot): envuelve en `match <expr> { <pat>[ if <guard>] =>
    ///   <bindings>, _ => panic!(...) }`. Permite `let (1, x) = ...`,
    ///   `let (Ok(v), tag) = ...`, `let ("ada", n) = ...`, etc.
    ///   Mini-tanda Lt habilita este path (antes era error de codegen).
    fn gen_destructure(
        &mut self,
        pattern: &crate::ast::Pattern,
        value: &Expr,
    ) -> Result<(), FitzError> {
        // Inferir tipo del value para registrar bindings en el scope.
        let (val_code, val_ty) = self.gen_expr(value)?;

        // Mini-tanda Lt — si el pattern es irrefutable y "puro" (solo
        // Ident/Wildcard/Tuple), emitimos `let pat = value` directo. Si
        // contiene literales/ranges/or/Ok/Err (que son refutables en
        // Rust), envolvemos en `match` con un brazo catch-all que
        // paniquea, paralelo a cómo `gen_pattern` lo trata para match.
        if pattern_is_pure_irrefutable(pattern) {
            let pat_code = self.destructure_pattern_to_rust(pattern, &val_ty)?;
            self.emit_indent();
            writeln!(&mut self.output, "let {} = {};", pat_code, val_code).unwrap();
            return Ok(());
        }

        // Camino "rico": pattern refutable. Estrategia:
        //   1. Bindeamos el scrutinee a un local con anotación de tipo
        //      explícita: `let __destr_scrut: <rust_ty> = <val_code>;`.
        //      Esto resuelve ambigüedades de inferencia tipo `Ok(99)`
        //      sin contexto del E (Rust necesita la anotación para
        //      saber `Result<i64, String>` vs `Result<i64, _>`).
        //   2. Recolectar los nombres bindeados por el pattern.
        //   3. Generar el Rust pattern + guard via `gen_pattern`
        //      (reutiliza la lógica del match — declara las vars en el
        //      scope del codegen).
        //   4. Emitir `let (n1, n2) = match __destr_scrut { pat[ if
        //      guard] => (n1, n2), _ => panic!("...") };`. Si hay un
        //      solo binding, sin paréntesis. Si hay cero, statement
        //      `match` con `()` en cada brazo.
        let scrut_rust_ty = rust_type_for(&val_ty, self.env)?;
        let mut names: Vec<String> = Vec::new();
        collect_pattern_bindings(pattern, &mut names);
        let (rust_pat, guard_opt) = self.gen_pattern(pattern, &val_ty, &None)?;
        let guard_clause = match &guard_opt {
            Some(g) => format!(" if {}", g),
            None => String::new(),
        };

        self.emit_indent();
        writeln!(
            &mut self.output,
            "let __destr_scrut: {} = {};",
            scrut_rust_ty, val_code
        )
        .unwrap();

        self.emit_indent();
        match names.len() {
            0 => {
                // Sin bindings: el `let` no hace falta — emitimos un
                // `match` stmt que paniquea si no matchea. Sirve para
                // chequear shape sin extraer valores.
                writeln!(
                    &mut self.output,
                    "match __destr_scrut {{ {}{} => {{}}, _ => panic!(\"destructuring no matcheó el valor\") }};",
                    rust_pat, guard_clause
                )
                .unwrap();
            }
            1 => {
                let n = &names[0];
                writeln!(
                    &mut self.output,
                    "let mut {} = match __destr_scrut {{ {}{} => {}, _ => panic!(\"destructuring no matcheó el valor\") }};",
                    n, rust_pat, guard_clause, n
                )
                .unwrap();
            }
            _ => {
                let joined = names.join(", ");
                writeln!(
                    &mut self.output,
                    "let ({}) = match __destr_scrut {{ {}{} => ({}), _ => panic!(\"destructuring no matcheó el valor\") }};",
                    joined, rust_pat, guard_clause, joined
                )
                .unwrap();
            }
        }
        Ok(())
    }

    /// Helper para `gen_destructure`. Recursea en el pattern y va
    /// registrando bindings en el scope. Devuelve el código Rust
    /// del pattern: `(a, _, b)`, `((x, y), z)`, etc.
    ///
    /// Solo cubre patterns puros irrefutables (Ident/Wildcard/Tuple).
    /// Para patterns ricos, `gen_destructure` toma otro camino (match
    /// wrapper). Este helper retorna error si recibe algo más, como
    /// guarda defensiva — no debería llamarse así.
    fn destructure_pattern_to_rust(
        &mut self,
        pat: &crate::ast::Pattern,
        ty: &Type,
    ) -> Result<String, FitzError> {
        use crate::ast::Pattern;
        match pat {
            Pattern::Ident(name) => {
                self.declare_var(name.clone(), ty.clone());
                Ok(name.clone())
            }
            Pattern::Wildcard => Ok("_".to_string()),
            Pattern::Tuple(subs) => {
                let slot_tys: Vec<Type> = match ty {
                    Type::Tuple(items) if items.len() == subs.len() => items.clone(),
                    _ => (0..subs.len()).map(|_| Type::Any).collect(),
                };
                let mut parts: Vec<String> = Vec::with_capacity(subs.len());
                for (s, st) in subs.iter().zip(slot_tys.iter()) {
                    parts.push(self.destructure_pattern_to_rust(s, st)?);
                }
                if parts.is_empty() {
                    Ok("()".to_string())
                } else if parts.len() == 1 {
                    Ok(format!("({},)", parts[0]))
                } else {
                    Ok(format!("({})", parts.join(", ")))
                }
            }
            _ => Err(self.err(
                "destructure_pattern_to_rust: pattern no puro pasó al camino puro \
                 (bug del codegen)".to_string(),
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
            // R.1.3 — `xs[i] = v` / `m["k"] = v` (mini-fase R).
            AssignTarget::Index { object, index } => {
                return self.gen_index_assign(object, index, value);
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

    /// Mini-tanda F14 — Emite `let X = <expr>` top-level de un módulo.
    ///
    /// Tres caminos:
    ///   1. **Literal puro** (`Int`/`Float`/`Bool`/`Str` directo) →
    ///      `pub const X: T = ...;` o `pub static X: &str = "...";`.
    ///      Comportamiento histórico de 5b.5.
    ///   2. **Expresión const-eval** (BinOp aritmético/lógico/bit-a-bit
    ///      sobre literales y otros consts top-level const-eval) →
    ///      `pub const X: T = <rhs>;`. Rust valida la const-eval en
    ///      compile-time.
    ///   3. **Expresión runtime** (StrInterp, Call, StructLit, etc.) →
    ///      `pub fn X() -> T { <rhs> }` — accessor function. Cada
    ///      referencia re-evalúa la RHS. Para inmutables (Str/Int/
    ///      etc.) la diferencia es invisible; para StructLit el clone
    ///      del Arc/Mutex es barato.
    // Mini-tanda Cd, F12 fix: emite `let X = <const-eval>` top-level
    // del archivo principal como `const X: T = ...;` o `static X: &str
    // = ...;` para que las fns top-level lo puedan referenciar; el name
    // se registra en `hoisted_main_lets` para que el `Stmt::Assign`
    // original no se emita como local de `main()` después.
    fn gen_main_hoisted_let(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let stmt_span = stmt.span();
        let Stmt::Assign { target, type_, value, .. } = stmt else {
            unreachable!("gen_main_hoisted_let solo se llama sobre Stmt::Assign");
        };
        let AssignTarget::Ident(name) = target else {
            unreachable!("collect_f12_hoists ya filtró por Ident");
        };

        let (rhs_code, rhs_ty) = self.gen_expr(value)?;
        let declared_ty = match type_ {
            Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                self.err_at(value.span(), format!(
                    "let `{}` (hoist): anotación: {}",
                    name, e.message
                ))
            })?,
            None => rhs_ty.clone(),
        };

        // Str literal → `static NAME: &str = "...";`.
        if matches!(&declared_ty, Type::Str) {
            if let Expr::Str(s, _) = value {
                writeln!(
                    &mut self.output,
                    "static {}: &str = {};\n",
                    name,
                    rust_str_literal(s),
                ).unwrap();
                self.hoisted_main_lets.insert(name.clone(), declared_ty);
                return Ok(());
            }
        }

        // const-eval (Int/Float/Bool con BinOp/UnaryOp puros).
        if is_const_eval_expr(value) {
            match &declared_ty {
                Type::Int => {
                    let coerced = coerce(&rhs_code, &rhs_ty, &Type::Int);
                    writeln!(&mut self.output, "const {}: i64 = {};\n", name, coerced).unwrap();
                    self.hoisted_main_lets.insert(name.clone(), declared_ty);
                    return Ok(());
                }
                Type::Float => {
                    let coerced = coerce(&rhs_code, &rhs_ty, &Type::Float);
                    writeln!(&mut self.output, "const {}: f64 = {};\n", name, coerced).unwrap();
                    self.hoisted_main_lets.insert(name.clone(), declared_ty);
                    return Ok(());
                }
                Type::Bool => {
                    writeln!(&mut self.output, "const {}: bool = {};\n", name, rhs_code).unwrap();
                    self.hoisted_main_lets.insert(name.clone(), declared_ty);
                    return Ok(());
                }
                _ => {}
            }
        }

        // Caso defensive: el filtro `collect_f12_hoists` no debería
        // dejar pasar otros tipos. Si llega acá es un bug; reportamos
        // claro en lugar de panic.
        Err(self.err_at(stmt_span, format!(
            "F12 hoist: tipo `{}` no soportado para `let {}` top-level (esperaba Int/Float/Bool/Str literal o const-eval)",
            display_type(&declared_ty, self.env),
            name,
        )))
    }

    fn gen_module_top_let(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let stmt_span = stmt.span();
        let Stmt::Assign { target, type_, value, .. } = stmt else {
            unreachable!("gen_module_top_let solo se llama sobre Stmt::Assign");
        };
        let AssignTarget::Ident(name) = target else {
            return Err(self.err_at(stmt_span,
                "asignación a campo a nivel top de módulo: no soportada (solo `let X = <expr>`)",
            ));
        };

        // Generar el código Rust de la RHS sin emitirlo todavía. Esto
        // nos da el tipo Fitz inferido y el código Rust de la expresión
        // para usar en la const/fn.
        let (rhs_code, rhs_ty) = self.gen_expr(value)?;

        let declared_ty = match type_ {
            Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                self.err_at(value.span(), format!(
                    "let `{}`: anotación: {}",
                    name, e.message
                ))
            })?,
            None => rhs_ty.clone(),
        };

        // Camino 1a: Str literal directo → `pub static X: &str = "...";`
        // (Rust no acepta `String` en const, pero sí `&'static str`.
        // El call site se encarga de `String::from(X)` cuando hace falta.)
        if matches!(&declared_ty, Type::Str) {
            if let Expr::Str(s, _) = value {
                writeln!(
                    &mut self.output,
                    "pub static {}: &str = {};\n",
                    name,
                    rust_str_literal(s)
                )
                .unwrap();
                return Ok(());
            }
        }

        // Camino 1b+2: const-eval-able (Int/Float/Bool con BinOp/UnaryOp
        // aritmético/lógico/bit recursivo) → emit como `pub const`.
        if is_const_eval_expr(value) {
            match &declared_ty {
                Type::Int => {
                    let coerced = coerce(&rhs_code, &rhs_ty, &Type::Int);
                    writeln!(
                        &mut self.output,
                        "pub const {}: i64 = {};\n",
                        name, coerced
                    )
                    .unwrap();
                    return Ok(());
                }
                Type::Float => {
                    let coerced = coerce(&rhs_code, &rhs_ty, &Type::Float);
                    writeln!(
                        &mut self.output,
                        "pub const {}: f64 = {};\n",
                        name, coerced
                    )
                    .unwrap();
                    return Ok(());
                }
                Type::Bool => {
                    writeln!(
                        &mut self.output,
                        "pub const {}: bool = {};\n",
                        name, rhs_code
                    )
                    .unwrap();
                    return Ok(());
                }
                _ => {
                    // Para otros tipos (Nominal, List, etc.) const-eval
                    // raramente aplica — caemos al accessor fn.
                }
            }
        }

        // Camino 3: runtime — accessor function `pub fn X() -> T { ... }`.
        let ret_rs = rust_type_for(&declared_ty, self.env).map_err(|_| {
            self.err_at(value.span(), format!(
                "let `{}`: tipo `{}` no soportado a nivel top de módulo",
                name,
                display_type(&declared_ty, self.env)
            ))
        })?;
        let final_rhs = coerce(&rhs_code, &rhs_ty, &declared_ty);
        writeln!(
            &mut self.output,
            "pub fn {}() -> {} {{ {} }}\n",
            name, ret_rs, final_rhs
        )
        .unwrap();
        Ok(())
    }

    /// R.1.3 — `xs[i] = v` y `m["k"] = v` (mini-fase R).
    ///
    /// Para `List<T>`: bounds check explícito + index como `usize`.
    /// Si index < 0 o >= len, emite panic con mensaje claro (paralelo
    /// al runtime del intérprete).
    ///
    /// Para `Map<K,V>`: linear search (preserva insertion order); si
    /// la clave existe se sobreescribe, si no, push al final.
    fn gen_index_assign(
        &mut self,
        object: &Expr,
        index: &Expr,
        value: &Expr,
    ) -> Result<(), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        let (idx_code, _idx_ty) = self.gen_expr(index)?;
        let (rhs_code, rhs_ty) = self.gen_expr(value)?;

        match &obj_ty {
            Type::List(item_ty) => {
                let coerced = coerce(&rhs_code, &rhs_ty, item_ty);
                // **Importante**: evaluar `__idx` y `__val` ANTES de
                // tomar el lock outer. Si `<<RHS>>` o `<<index>>`
                // contienen un access al mismo Mutex (ej. `nums[i] =
                // nums[i] * 10`), un `__g.lock()` outer + un
                // `nums.lock()` adentro del RHS produciría DEADLOCK
                // (`std::sync::Mutex` no es reentrante). El patrón
                // "compute first, lock last" lo evita.
                self.emit_indent();
                writeln!(
                    &mut self.output,
                    "{{ \
                     let __coll = {}.clone(); \
                     let __idx: i64 = {}; \
                     let __val = {}; \
                     let mut __g = __coll.lock().unwrap(); \
                     let __len = __g.len() as i64; \
                     let __eff = if __idx < 0 {{ __len + __idx }} else {{ __idx }}; \
                     if __eff < 0 || __eff >= __len {{ \
                     panic!(\"índice {{}} fuera de rango (lista de tamaño {{}})\", __idx, __len); \
                     }} \
                     __g[__eff as usize] = __val; \
                     }}",
                    obj_code, idx_code, coerced
                )
                .unwrap();
                Ok(())
            }
            Type::Map(_k_ty, v_ty) => {
                let coerced = coerce(&rhs_code, &rhs_ty, v_ty);
                // Mismo patrón compute-first, lock-last que List.
                self.emit_indent();
                writeln!(
                    &mut self.output,
                    "{{ \
                     let __coll = {}.clone(); \
                     let __k = {}; \
                     let __v = {}; \
                     let mut __g = __coll.lock().unwrap(); \
                     let mut __found = false; \
                     for (__ek, __ev) in __g.iter_mut() {{ \
                     if *__ek == __k {{ *__ev = __v.clone(); __found = true; break; }} \
                     }} \
                     if !__found {{ __g.push((__k, __v)); }} \
                     }}",
                    obj_code, idx_code, coerced
                )
                .unwrap();
                Ok(())
            }
            other => Err(self.err_at(object.span(), format!(
                "asignación a índice `[...] = v` no soportada sobre `{}` (solo List y Map)",
                type_name(other)
            ))),
        }
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
        label: Option<&str>,
        ret_expected: &Type,
    ) -> Result<(), FitzError> {
        let (cond_code, _) = self.gen_expr(condition)?;
        self.emit_indent();
        if let Some(l) = label {
            self.emit(&format!("'{}: ", l));
        }
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

    fn gen_loop(&mut self, body: &[Stmt], label: Option<&str>, ret_expected: &Type) -> Result<(), FitzError> {
        self.emit_indent();
        if let Some(l) = label {
            self.emit(&format!("'{}: ", l));
        }
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
        var: &crate::ast::Pattern,
        iter: &Expr,
        body: &[Stmt],
        label: Option<&str>,
        ret_expected: &Type,
    ) -> Result<(), FitzError> {
        // Tres iterables soportados:
        //   * `for v in start..end` — rango exclusivo (5b.1).
        //   * `for v in xs` con xs: List<T> — itera sobre snapshot.
        //   * `for (k, v) in m` con m: Map<K, V> — itera sobre snapshot
        //     con destructuring nativo Rust (mini-tanda Md).
        // Snapshot para evitar re-entrancia si el body muta el container.
        //
        // Patrones aceptados como `var`:
        //   - Pattern::Ident(name) — bindea cada elemento.
        //   - Pattern::Wildcard — ignora cada elemento (emite `_`).
        //   - Pattern::Tuple(subs) — destructura cada elemento como tupla.
        use crate::ast::Pattern;
        let label_prefix = label.map(|l| format!("'{}: ", l)).unwrap_or_default();

        // Range case — solo Pattern::Ident y Wildcard tienen sentido.
        if let Expr::Range { start, end, inclusive, .. } = iter {
            let (start_code, _) = self.gen_expr(start)?;
            let (end_code, _) = self.gen_expr(end)?;
            let op = if *inclusive { "..=" } else { ".." };
            let (binding, declared) = pattern_to_simple_binding(var, &Type::Int)
                .map_err(|msg| self.err_at(iter.span(), msg))?;
            let mut_prefix = if binding == "_" { "" } else { "mut " };
            self.emit_indent();
            writeln!(
                &mut self.output,
                "{label_prefix}for {mut_prefix}{binding} in ({start_code} as i64){op}({end_code} as i64) {{"
            )
            .unwrap();
            self.indent += 1;
            self.push_scope();
            for (name, ty) in declared {
                self.declare_var(name, ty);
            }
            for s in body {
                self.gen_stmt_in_fn(s, ret_expected)?;
            }
            self.pop_scope();
            self.indent -= 1;
            self.emit_indent();
            self.emit("}\n");
            return Ok(());
        }

        // Generic case — iter es List<T> o Map<K, V>.
        let (iter_code, iter_ty) = self.gen_expr(iter)?;
        match &iter_ty {
            Type::List(inner) => {
                let elem_ty = (**inner).clone();
                if matches!(elem_ty, Type::Any) {
                    return Err(self.err_at(iter.span(),
                        "`for ... in xs` sobre `List<Any>`: el subset compilado exige tipo homogéneo concreto"
                            .to_string(),
                    ));
                }
                // Mini-tanda It — si el elem es Tuple y el var es Pattern::Tuple
                // del mismo aridad, emitimos destructuring nativo Rust (paralelo
                // a Map). Caso canónico: `for (i, x) in xs.enumerate()`.
                if let (Pattern::Tuple(subs), Type::Tuple(item_tys)) = (var, &elem_ty) {
                    if subs.len() == item_tys.len() {
                        let mut bindings: Vec<(String, Vec<(String, Type)>)> = Vec::with_capacity(subs.len());
                        for (sub, ty) in subs.iter().zip(item_tys.iter()) {
                            let bnd = pattern_to_simple_binding(sub, ty)
                                .map_err(|msg| self.err_at(iter.span(), msg))?;
                            bindings.push(bnd);
                        }
                        let parts: Vec<String> = bindings
                            .iter()
                            .map(|(name, _)| {
                                let prefix = if name == "_" { "" } else { "mut " };
                                format!("{prefix}{name}")
                            })
                            .collect();
                        self.emit_indent();
                        writeln!(
                            &mut self.output,
                            "{label_prefix}for ({}) in ({iter_code}).lock().unwrap().clone().into_iter() {{",
                            parts.join(", ")
                        )
                        .unwrap();
                        self.indent += 1;
                        self.push_scope();
                        for (_, declared) in &bindings {
                            for (name, ty) in declared {
                                self.declare_var(name.clone(), ty.clone());
                            }
                        }
                        for s in body {
                            self.gen_stmt_in_fn(s, ret_expected)?;
                        }
                        self.pop_scope();
                        self.indent -= 1;
                        self.emit_indent();
                        self.emit("}\n");
                        return Ok(());
                    }
                }
                let (binding, declared) = pattern_to_simple_binding(var, &elem_ty)
                    .map_err(|msg| self.err_at(iter.span(), msg))?;
                let mut_prefix = if binding == "_" { "" } else { "mut " };
                self.emit_indent();
                writeln!(
                    &mut self.output,
                    "{label_prefix}for {mut_prefix}{binding} in ({iter_code}).lock().unwrap().clone().into_iter() {{"
                )
                .unwrap();
                self.indent += 1;
                self.push_scope();
                for (name, ty) in declared {
                    self.declare_var(name, ty);
                }
                for s in body {
                    self.gen_stmt_in_fn(s, ret_expected)?;
                }
                self.pop_scope();
                self.indent -= 1;
            }
            Type::Map(k_ty, v_ty) => {
                // Mini-tanda Md — destructuring nativo Rust sobre el
                // Vec<(K, V)> interno del Map. Aceptamos:
                //   - Pattern::Tuple([Ident a, Ident b]) → `(a, b)`
                //     destructuring directo.
                //   - Pattern::Wildcard → `_` (ignora todo el par).
                // Pattern::Ident solo (`for kv in m`) NO está soportado
                // en codegen porque emitir un binding tipo `(K, V)` que
                // se use luego como Tuple Rust requiere helpers que
                // hoy no tenemos.
                match var {
                    Pattern::Tuple(subs) if subs.len() == 2 => {
                        let (kname, ktdecl) = pattern_to_simple_binding(&subs[0], k_ty)
                            .map_err(|msg| self.err_at(iter.span(), msg))?;
                        let (vname, vtdecl) = pattern_to_simple_binding(&subs[1], v_ty)
                            .map_err(|msg| self.err_at(iter.span(), msg))?;
                        // `mut _` no es válido en Rust. Detectar
                        // wildcards y omitir el `mut`.
                        let k_prefix = if kname == "_" { "" } else { "mut " };
                        let v_prefix = if vname == "_" { "" } else { "mut " };
                        self.emit_indent();
                        writeln!(
                            &mut self.output,
                            "{label_prefix}for ({k_prefix}{kname}, {v_prefix}{vname}) in ({iter_code}).lock().unwrap().clone().into_iter() {{"
                        )
                        .unwrap();
                        self.indent += 1;
                        self.push_scope();
                        for (name, ty) in ktdecl.into_iter().chain(vtdecl) {
                            self.declare_var(name, ty);
                        }
                        for s in body {
                            self.gen_stmt_in_fn(s, ret_expected)?;
                        }
                        self.pop_scope();
                        self.indent -= 1;
                    }
                    Pattern::Wildcard => {
                        self.emit_indent();
                        writeln!(
                            &mut self.output,
                            "{label_prefix}for _ in ({iter_code}).lock().unwrap().clone().into_iter() {{"
                        )
                        .unwrap();
                        self.indent += 1;
                        self.push_scope();
                        for s in body {
                            self.gen_stmt_in_fn(s, ret_expected)?;
                        }
                        self.pop_scope();
                        self.indent -= 1;
                    }
                    _ => {
                        return Err(self.err_at(iter.span(),
                            "`for ... in m` sobre Map en `fitz build` exige un tuple pattern de 2 elementos: `for (k, v) in m { ... }`. `for kv in m` queda como deuda residual del codegen.",
                        ));
                    }
                }
            }
            other => {
                return Err(self.err_at(iter.span(), format!(
                    "`for <pat> in <expr>`: el iterable es `{}`, solo se soportan Range, List<T> y Map<K, V>",
                    display_type(other, self.env)
                )));
            }
        }
        self.emit_indent();
        self.emit("}\n");
        Ok(())
    }

    // --- generación de expresiones ----------------------------------------

    /// Devuelve `(código Rust de la expresión, tipo Fitz)`.
    fn gen_expr(&mut self, e: &Expr) -> Result<(String, Type), FitzError> {
        match e {
            // Fp.3 — NamedArg solo es válido adentro de Call.args; el
            // dispatcher de calls lo procesa antes de llegar acá. Verlo
            // en gen_expr indica AST mal formado (bug interno).
            Expr::NamedArg { name, span, .. } => Err(self.err_at(*span, format!(
                "argumento nombrado `{}:` no puede aparecer fuera de una llamada",
                name
            ))),
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
            // Mini-tanda Bytes — literal `b"..."` → `vec![<byte>, ...]`
            // en Rust. Para el caso vacío emitimos `Vec::<u8>::new()` en
            // lugar de `vec![]` para evitar que rustc pida type
            // annotations (E0282).
            Expr::Bytes(bs, _) => {
                if bs.is_empty() {
                    return Ok(("Vec::<u8>::new()".to_string(), Type::Bytes));
                }
                let mut s = String::from("vec![");
                for (i, b) in bs.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&format!("{}u8", b));
                }
                s.push(']');
                Ok((s, Type::Bytes))
            }
            // Mini-tanda L — `loop { body }` como expresión. Rust
            // nativo soporta `break <value>` adentro de `loop` y
            // produce el valor de la expresión. Emitimos el body
            // dentro de un bloque `loop { ... }`. El tipo lo
            // sintetizamos como `Any` por simplicidad MVP (rustc
            // infiere desde el `break <v>` adentro).
            Expr::Loop { body, label, .. } => {
                // Mini-tanda L — emitir body con un frame de
                // `break_value_stack` activo para que los
                // `Stmt::Break(Some(v), _)` adentro reporten su
                // tipo. Después unificar con `lub` para el tipo
                // del Expr::Loop. `label` opcional emite
                // `'name: loop { ... }` Rust nativo.
                self.break_value_stack.push(Vec::new());
                let stmt_refs: Vec<&Stmt> = body.iter().collect();
                let body_code = self.gen_block_to_string(&stmt_refs)?;
                let values = self.break_value_stack.pop().unwrap_or_default();
                let result_ty = if values.is_empty() {
                    Type::Null
                } else {
                    let mut acc = values[0].clone();
                    for t in &values[1..] {
                        acc = lub(&acc, t).unwrap_or(Type::Any);
                    }
                    acc
                };
                let label_prefix = label
                    .as_ref()
                    .map(|l| format!("'{}: ", l))
                    .unwrap_or_default();
                let code = format!("({}loop {{ {} }})", label_prefix, body_code);
                Ok((code, result_ty))
            }
            // Tuples (mini-tanda T) — Rust nativo `(a, b, c)`. Tupla
            // vacía `()` y de 1 elemento `(x,)`.
            Expr::Tuple(items, _) => {
                let mut codes: Vec<String> = Vec::with_capacity(items.len());
                let mut tys: Vec<Type> = Vec::with_capacity(items.len());
                for e in items {
                    let (c, t) = self.gen_expr(e)?;
                    codes.push(c);
                    tys.push(t);
                }
                let code = if codes.is_empty() {
                    "()".to_string()
                } else if codes.len() == 1 {
                    format!("({},)", codes[0])
                } else {
                    format!("({})", codes.join(", "))
                };
                Ok((code, Type::Tuple(tys)))
            }
            Expr::TupleField { tuple, index, span } => {
                let (obj_code, obj_ty) = self.gen_expr(tuple)?;
                match &obj_ty {
                    Type::Tuple(items) => {
                        if let Some(t) = items.get(*index) {
                            // `.0`, `.1`, etc. Rust nativo. Para
                            // primitivos `Copy` no hace falta clone;
                            // para String/Arc/etc. agregamos `.clone()`
                            // por seguridad.
                            let needs_clone = matches!(
                                t,
                                Type::Str | Type::List(_) | Type::Map(_, _) | Type::Nominal(_)
                                    | Type::Tuple(_) | Type::Result { .. } | Type::Nullable(_)
                            );
                            let code = if needs_clone {
                                format!("({}).{}.clone()", obj_code, index)
                            } else {
                                format!("({}).{}", obj_code, index)
                            };
                            Ok((code, t.clone()))
                        } else {
                            Err(self.err_at(*span, format!(
                                "tupla de {} elementos no tiene índice `{}`",
                                items.len(), index
                            )))
                        }
                    }
                    other => Err(self.err_at(*span, format!(
                        "acceso `.{}` solo aplica a tuplas, recibí `{}`",
                        index, display_type(other, self.env)
                    ))),
                }
            }
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
                                // Mini-tanda F14: si el const es accessor
                                // fn en el módulo origen, referenciamos
                                // como `name()`.
                                let is_accessor = m.accessor_consts.contains(&item);
                                let access = if is_accessor {
                                    format!("{}()", name)
                                } else {
                                    name.to_string()
                                };
                                let code = match &ty {
                                    Type::Str if !is_accessor => format!("String::from({})", name),
                                    _ => access,
                                };
                                return Ok((code, ty));
                            }
                        }
                    }
                }
                // Mini-tanda Cd (F12 fix) — `let X = <const-eval>` del
                // archivo principal hoisteado a const/static top-level.
                // Si no hay binding local con ese nombre, resolvemos al
                // ident Rust directo (Rust hace el lookup global).
                if !self.var_in_any_scope(name) {
                    if let Some(ty) = self.hoisted_main_lets.get(name).cloned() {
                        // Str hoisteado vive como &str — los call sites
                        // necesitan String, así que envolvemos como
                        // `String::from(NAME)` por uniformidad (paralelo
                        // a `own_consts` para Str pub static).
                        let code = if matches!(&ty, Type::Str) {
                            format!("String::from({})", name)
                        } else {
                            name.clone()
                        };
                        return Ok((code, ty));
                    }
                }
                // 5b.5: const top-level del propio módulo (emitida como
                // `pub static`/`pub const`). El fn body la referencia
                // por nombre — Rust resuelve. Mini-tanda F14: si es
                // accessor fn (RHS no const-eval), referenciamos
                // como `name()` en lugar de `name`.
                if let Some(ty) = self.own_consts.get(name).cloned() {
                    let is_accessor = self.accessor_consts.contains(name);
                    let access = if is_accessor {
                        format!("{}()", name)
                    } else {
                        name.clone()
                    };
                    let code = match &ty {
                        // Para Str pub static (literal puro) sigue
                        // siendo &str → String. Para Str accessor fn,
                        // la fn ya retorna String.
                        Type::Str if !is_accessor => format!("String::from({})", name),
                        _ => access,
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
            Expr::ListComp { expr, var, iter, extra_clauses, filter, span } => {
                self.gen_list_comp(expr, var, iter, extra_clauses, filter.as_deref(), *span)
            }
            Expr::MapComp { key, value, var, iter, extra_clauses, filter, span } => {
                self.gen_map_comp(key, value, var, iter, extra_clauses, filter.as_deref(), *span)
            }
            Expr::Map(pairs, span) => self.gen_map_lit(pairs, *span),
            Expr::Index { object, index, span } => self.gen_index(object, index, *span),
            Expr::Slice { object, start, end, inclusive, span } => {
                self.gen_slice(object, start.as_deref(), end.as_deref(), *inclusive, *span)
            }
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
            Expr::FnExpr { params, body, is_async, span } => {
                // Mini-tanda Async-cl build — async closures inline
                // ahora compilan. El closure se emite como
                // `move |...| -> Pin<Box<dyn Future<Output=R> + Send>>
                // { Box::pin(async move { ... }) }`.
                self.gen_fn_expr_as_value(params, body, *is_async, *span)
            }
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
                StrPart::Expr(e, spec) => {
                    let (code, ty) = self.gen_expr(e)?;
                    // Mini-tanda Fm — si hay spec, traducimos a `format!`
                    // de Rust nativo. Para los casos que Rust no soporta
                    // directo (g/G, c, %, grouping `,`/`_`) emitimos error
                    // de codegen claro citando `fitz run` como workaround.
                    match spec {
                        None => {
                            fmt.push_str("{}");
                            let piece = match &ty {
                                Type::Int | Type::Bool | Type::Str => code,
                                _ => show_expr(&code, &ty),
                            };
                            args.push(piece);
                        }
                        Some(s) => {
                            let (rust_spec, coerced) = format_spec_to_rust(s, &code, &ty)
                                .map_err(|msg| self.err_at(e.span(), msg))?;
                            fmt.push_str(&format!("{{{}}}", rust_spec));
                            args.push(coerced);
                        }
                    }
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
            BinOpKind::Sub | BinOpKind::Mul => {
                let sym = match op {
                    BinOpKind::Sub => "-",
                    BinOpKind::Mul => "*",
                    _ => unreachable!(),
                };
                let (l, r, t) = numeric_coerce(&lc, &lt, &rc, &rt)
                    .ok_or_else(|| self.err_at(span, format!(
                        "operador `{}` no aplicable a `{}` y `{}` en codegen",
                        sym, type_name(&lt), type_name(&rt)
                    )))?;
                Ok((format!("({} {} {})", l, sym, r), t))
            }
            // Mini-tanda DZ — chequeo explícito de divisor 0 para
            // emitir el mismo mensaje `"división por cero"` del
            // intérprete (`eval_div` en evaluator.rs). Sin este
            // wrap: (a) `10 / 0` literal hace rustc rechazar con
            // `unconditional_panic` en const-eval, y (b) `a / 0`
            // dinámico paniquearía con el msg crudo de Rust
            // (`attempt to divide by zero`) para Int, o produciría
            // `inf`/`NaN` silencioso para Float.
            BinOpKind::Div => {
                let (l, r, t) = numeric_coerce(&lc, &lt, &rc, &rt)
                    .ok_or_else(|| self.err_at(span, format!(
                        "operador `/` no aplicable a `{}` y `{}` en codegen",
                        type_name(&lt), type_name(&rt)
                    )))?;
                let (ty_rs, zero_lit) = match &t {
                    Type::Int => ("i64", "0"),
                    Type::Float => ("f64", "0.0"),
                    _ => unreachable!(),
                };
                Ok((
                    format!(
                        "{{ let __a: {ty} = {l}; let __b: {ty} = {r}; \
                         if __b == {z} {{ panic!(\"división por cero\"); }} \
                         (__a / __b) }}",
                        ty = ty_rs, z = zero_lit, l = l, r = r
                    ),
                    t,
                ))
            }
            // R.1.2 — operador `%` con semántica euclidean. Emitimos
            // `i64::rem_euclid` para paridad bit-a-bit con el
            // intérprete (mismo signo del divisor). El checker
            // garantiza ambos lados Int.
            BinOpKind::Mod => {
                if !matches!(lt, Type::Int) || !matches!(rt, Type::Int) {
                    return Err(self.err_at(span, format!(
                        "operador `%` requiere Int en ambos lados (recibió `{}` y `{}`)",
                        type_name(&lt),
                        type_name(&rt)
                    )));
                }
                // El `{}.rem_euclid({})` paniquea si `b == 0`. Lo
                // envolvemos en un check explícito para emitir el
                // mismo error que el intérprete ("división por
                // cero") en lugar de un panic crudo de Rust.
                Ok((
                    format!(
                        "{{ let __a: i64 = {}; let __b: i64 = {}; \
                         if __b == 0 {{ panic!(\"división por cero\"); }} \
                         __a.rem_euclid(__b) }}",
                        lc, rc
                    ),
                    Type::Int,
                ))
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
                // Mini-tanda CT — comparación entre tipos primitivos
                // incompatibles (Int vs Str, Bool vs Int, Str vs Null
                // sin Nullable, etc.). El intérprete devuelve `false`
                // sin error (`Value::PartialEq` distingue por variant).
                // Codegen: emite el literal (`false` para `==`,
                // `true` para `!=`) evaluando ambos lados con
                // `let _` para preservar side effects (calls, prints).
                // Rustc rechazaría `Int == String` con E0308; este
                // wrap alinea con la semántica del intérprete sin
                // panicar.
                if ct_incompatible_eq(&lt, &rt) {
                    let result_lit = if is_eq { "false" } else { "true" };
                    return Ok((
                        format!(
                            "{{ let _ = {}; let _ = {}; {} }}",
                            lc, rc, result_lit
                        ),
                        Type::Bool,
                    ));
                }
                // Bools, Null directos.
                Ok((format!("({} {} {})", lc, sym, rc), Type::Bool))
            }
            BinOpKind::And => Ok((format!("({} && {})", lc, rc), Type::Bool)),
            BinOpKind::Or => Ok((format!("({} || {})", lc, rc), Type::Bool)),
            // Mini-tanda Xor — `a xor b` = `a != b` sobre Bool.
            // Sin short-circuit (paralelo al evaluator); emite `!=`
            // directo entre dos `bool` Rust.
            BinOpKind::Xor => Ok((format!("({} != {})", lc, rc), Type::Bool)),
            // Mini-tanda Bits — operadores bit-a-bit sobre Int. Emit
            // Rust nativo. Para shifts, el RHS de Rust requiere `u32`
            // (i64 no implementa Shl<i64>), así que cast explícito.
            // El check de "shift en rango 0..64" lo hace el runtime
            // (`wrapping_shl`/`wrapping_shr` sobre i64) — el codegen
            // emite `wrapping_shl((rhs as u32))` para mantener paridad
            // bit-a-bit con el evaluator.
            BinOpKind::BitAnd => Ok((format!("({} & {})", lc, rc), Type::Int)),
            BinOpKind::BitOr => Ok((format!("({} | {})", lc, rc), Type::Int)),
            BinOpKind::BitXor => Ok((format!("({} ^ {})", lc, rc), Type::Int)),
            BinOpKind::Shl => Ok((
                format!(
                    "({{ let __rhs: i64 = {}; if !(0..64).contains(&__rhs) {{ panic!(\"shift fuera de rango: {{}}\", __rhs); }} (({}).wrapping_shl(__rhs as u32)) }})",
                    rc, lc
                ),
                Type::Int,
            )),
            BinOpKind::Shr => Ok((
                format!(
                    "({{ let __rhs: i64 = {}; if !(0..64).contains(&__rhs) {{ panic!(\"shift fuera de rango: {{}}\", __rhs); }} (({}).wrapping_shr(__rhs as u32)) }})",
                    rc, lc
                ),
                Type::Int,
            )),
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
            // R.1.1 — `not <expr>` emite `!` Rust nativo. El checker
            // garantiza que el operando tipa `Bool` (o `Any` gradual),
            // así que `!<bool_expr>` es válido Rust.
            UnaryOpKind::Not => Ok((format!("(!{})", code), Type::Bool)),
            // Mini-tanda Bits — `~x` también emite `!` Rust (sirve
            // para Int por las reglas del operador en Rust).
            UnaryOpKind::BitNot => Ok((format!("(!{})", code), Type::Int)),
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
            // Fase 9.w.1.d — built-ins `jwt`/`hash`. Dispatch antes de
            // cualquier otra cosa: el receiver es un Ident con nombre
            // exacto `jwt`/`hash`, y los helpers del preludio aterrizan
            // por nombre. Si el usuario shadowea con un `let jwt = ...`,
            // el lookup de var local gana — pero el codegen del MVP no
            // hace esa verificación (todos los programas razonables no
            // shadowean los módulos del lenguaje). Refinable si pasa a
            // ser problema real.
            if let Expr::Ident(recv, _) = object.as_ref() {
                match (recv.as_str(), field.as_str()) {
                    ("jwt", "encode") => return self.gen_auth_jwt_encode(args, call_span),
                    ("jwt", "decode") => return self.gen_auth_jwt_decode(args, call_span),
                    ("hash", "password") => {
                        return self.gen_auth_hash_password(args, call_span);
                    }
                    ("hash", "verify") => return self.gen_auth_hash_verify(args, call_span),
                    _ => {}
                }
            }
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
            return Ok((code, Type::Result { ok: Box::new(Type::PyAny), err: Box::new(Type::Str) }));
        }
        if name == "print" {
            return Err(self.err_at(call_span,
                "`print(...)` solo puede usarse como sentencia, no como expresión en 5b.1",
            ));
        }
        // Mini-tanda Bits-extras — builtins globales sobre Int. Si
        // el usuario define una fn con el mismo nombre, `fn_sigs` la
        // toma antes (paralelo a `sleep`/`len`). Emite el método Rust
        // nativo equivalente sobre `i64`.
        if matches!(
            name.as_str(),
            "popcount" | "leading_zeros" | "trailing_zeros"
        ) && !self.fn_sigs.contains_key(name)
        {
            if args.len() != 1 {
                return Err(self.err_at(call_span, format!(
                    "`{}` espera 1 argumento, recibió {}",
                    name, args.len()
                )));
            }
            let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
            let coerced = coerce(&arg_code, &arg_ty, &Type::Int);
            let rust_method = match name.as_str() {
                "popcount" => "count_ones",
                "leading_zeros" => "leading_zeros",
                "trailing_zeros" => "trailing_zeros",
                _ => unreachable!(),
            };
            return Ok((
                format!("(({}).{}() as i64)", coerced, rust_method),
                Type::Int,
            ));
        }
        if matches!(name.as_str(), "rotate_left" | "rotate_right")
            && !self.fn_sigs.contains_key(name)
        {
            if args.len() != 2 {
                return Err(self.err_at(call_span, format!(
                    "`{}` espera 2 argumentos (n, bits), recibió {}",
                    name, args.len()
                )));
            }
            let (n_code, n_ty) = self.gen_expr(&args[0])?;
            let (b_code, b_ty) = self.gen_expr(&args[1])?;
            let n_c = coerce(&n_code, &n_ty, &Type::Int);
            let b_c = coerce(&b_code, &b_ty, &Type::Int);
            let rust_method = if name == "rotate_left" {
                "rotate_left"
            } else {
                "rotate_right"
            };
            return Ok((
                format!(
                    "(({n_c}).{method}((({b_c}).rem_euclid(64)) as u32))",
                    method = rust_method
                ),
                Type::Int,
            ));
        }
        // Mini-tanda Math — builtins matemáticos polimórficos.
        // abs/min/max/clamp aceptan Int|Float (mismo tipo en todos los
        // args, devuelven ese tipo). pow/sqrt devuelven Float.
        // ceil/floor/round devuelven Int.
        if matches!(name.as_str(), "abs") && !self.fn_sigs.contains_key(name) {
            if args.len() != 1 {
                return Err(self.err_at(call_span, format!(
                    "`abs(x)` espera 1 argumento, recibió {}", args.len()
                )));
            }
            let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
            return match arg_ty {
                Type::Int => Ok((format!("({}).wrapping_abs()", arg_code), Type::Int)),
                Type::Float => Ok((format!("({}).abs()", arg_code), Type::Float)),
                other => Err(self.err_at(call_span, format!(
                    "`abs(x)` espera `Int` o `Float`, recibió `{}`",
                    display_type(&other, self.env)
                ))),
            };
        }
        if matches!(name.as_str(), "min" | "max") && !self.fn_sigs.contains_key(name) {
            if args.len() != 2 {
                return Err(self.err_at(call_span, format!(
                    "`{}(a, b)` espera 2 args, recibió {}", name, args.len()
                )));
            }
            let (a_code, a_ty) = self.gen_expr(&args[0])?;
            let (b_code, b_ty) = self.gen_expr(&args[1])?;
            let is_max = name == "max";
            // Misma rama para Int+Int y Float+Float; rechazamos mix.
            return match (&a_ty, &b_ty) {
                (Type::Int, Type::Int) => {
                    let rust = if is_max { "max" } else { "min" };
                    Ok((format!("({a_code}).{rust}({b_code})"), Type::Int))
                }
                (Type::Float, Type::Float) => {
                    let cmp = if is_max { ">" } else { "<" };
                    Ok((
                        format!("{{ let __a: f64 = {a_code}; let __b: f64 = {b_code}; if __a {cmp} __b {{ __a }} else {{ __b }} }}"),
                        Type::Float,
                    ))
                }
                (a, b) => Err(self.err_at(call_span, format!(
                    "`{}(a, b)`: args deben ser ambos Int o ambos Float, recibió `{}` y `{}`",
                    name,
                    display_type(a, self.env),
                    display_type(b, self.env)
                ))),
            };
        }
        if matches!(name.as_str(), "pow") && !self.fn_sigs.contains_key(name) {
            if args.len() != 2 {
                return Err(self.err_at(call_span, format!(
                    "`pow(base, exp)` espera 2 args, recibió {}", args.len()
                )));
            }
            let (a_code, a_ty) = self.gen_expr(&args[0])?;
            let (b_code, b_ty) = self.gen_expr(&args[1])?;
            let a_f = match a_ty {
                Type::Int => format!("({} as f64)", a_code),
                Type::Float => a_code,
                other => return Err(self.err_at(call_span, format!(
                    "`pow(base, exp)`: base debe ser Int o Float, recibió `{}`",
                    display_type(&other, self.env)
                ))),
            };
            let b_f = match b_ty {
                Type::Int => format!("({} as f64)", b_code),
                Type::Float => b_code,
                other => return Err(self.err_at(call_span, format!(
                    "`pow(base, exp)`: exp debe ser Int o Float, recibió `{}`",
                    display_type(&other, self.env)
                ))),
            };
            return Ok((format!("({a_f}).powf({b_f})"), Type::Float));
        }
        if matches!(name.as_str(), "sqrt") && !self.fn_sigs.contains_key(name) {
            if args.len() != 1 {
                return Err(self.err_at(call_span, format!(
                    "`sqrt(x)` espera 1 argumento, recibió {}", args.len()
                )));
            }
            let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
            let coerced = match arg_ty {
                Type::Int => format!("({} as f64)", arg_code),
                Type::Float => arg_code,
                other => return Err(self.err_at(call_span, format!(
                    "`sqrt(x)` espera `Int` o `Float`, recibió `{}`",
                    display_type(&other, self.env)
                ))),
            };
            return Ok((format!("({}).sqrt()", coerced), Type::Float));
        }
        if matches!(name.as_str(), "ceil" | "floor" | "round")
            && !self.fn_sigs.contains_key(name)
        {
            if args.len() != 1 {
                return Err(self.err_at(call_span, format!(
                    "`{}(x)` espera 1 argumento, recibió {}", name, args.len()
                )));
            }
            let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
            return match arg_ty {
                Type::Int => Ok((arg_code, Type::Int)),
                Type::Float => Ok((
                    format!("({}).{}() as i64", arg_code, name),
                    Type::Int,
                )),
                other => Err(self.err_at(call_span, format!(
                    "`{}(x)` espera `Float` o `Int`, recibió `{}`",
                    name, display_type(&other, self.env)
                ))),
            };
        }
        if matches!(name.as_str(), "clamp") && !self.fn_sigs.contains_key(name) {
            if args.len() != 3 {
                return Err(self.err_at(call_span, format!(
                    "`clamp(x, lo, hi)` espera 3 args, recibió {}", args.len()
                )));
            }
            let (x_code, x_ty) = self.gen_expr(&args[0])?;
            let (lo_code, lo_ty) = self.gen_expr(&args[1])?;
            let (hi_code, hi_ty) = self.gen_expr(&args[2])?;
            return match (&x_ty, &lo_ty, &hi_ty) {
                (Type::Int, Type::Int, Type::Int) => Ok((
                    format!("({x_code}).clamp({lo_code}, {hi_code})"),
                    Type::Int,
                )),
                (Type::Float, Type::Float, Type::Float) => Ok((
                    format!("({x_code}).clamp({lo_code}, {hi_code})"),
                    Type::Float,
                )),
                (a, b, c) => Err(self.err_at(call_span, format!(
                    "`clamp(x, lo, hi)`: los 3 args deben ser del mismo tipo Int o Float, recibió `{}`, `{}`, `{}`",
                    display_type(a, self.env),
                    display_type(b, self.env),
                    display_type(c, self.env),
                ))),
            };
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
                Type::Bytes => Ok((
                    format!("(({}).len() as i64)", arg_code),
                    Type::Int,
                )),
                Type::List(_) | Type::Map(_, _) => Ok((
                    format!("(({}).lock().unwrap().len() as i64)", arg_code),
                    Type::Int,
                )),
                other => Err(self.err_at(arg_span, format!(
                    "`len(...)`: no aplica a `{}` — solo Str, Bytes, List<T> y Map<K, V>",
                    display_type(&other, self.env)
                ))),
            };
        }
        // Mini-tanda Bytes — constructor builtin `bytes(s: Str) -> Bytes`.
        // Convierte un Str a Vec<u8> Rust usando `as_bytes().to_vec()`.
        if name == "bytes" && !self.fn_sigs.contains_key(name) && args.len() == 1 {
            let arg_span = args[0].span();
            let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
            if !matches!(arg_ty, Type::Str) {
                return Err(self.err_at(arg_span, format!(
                    "`bytes(...)`: el argumento debe ser Str, recibió `{}`",
                    display_type(&arg_ty, self.env)
                )));
            }
            return Ok((
                format!("({}).as_bytes().to_vec()", arg_code),
                Type::Bytes,
            ));
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
            // Higher-order: vars `Type::Function` no llevan info de
            // defaults (la signature paramétrica no conserva los Expr).
            // Defaults solo aplican a callees por nombre resolubles en
            // `fn_sigs`. Estricta aridad acá.
            let arity = params.len();
            let sig = FnSig { params, ret: *ret, defaults: vec![None; arity], has_varargs: false, param_names: Vec::new() };
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
        // Fp.3 — si hay named args, reordenar a posicionales primero.
        // Requiere `sig.param_names` poblado; en su ausencia (FnSig
        // sintetizada para Type::Function de var), error claro.
        let has_named = args.iter().any(|a| matches!(a, Expr::NamedArg { .. }));
        if has_named {
            if sig.param_names.is_empty() {
                return Err(self.err_at(call_span, format!(
                    "`{}` no soporta argumentos nombrados (callee indirecto sin info de nombres)",
                    callee_expr
                )));
            }
            if sig.has_varargs {
                return Err(self.err_at(call_span, format!(
                    "`{}` tiene un parámetro variádico; los argumentos nombrados \
                     no son compatibles con varargs en esta versión",
                    callee_expr
                )));
            }
            // Reordenar a posicional. Cada slot se rellena con un Expr
            // (el value del NamedArg, el positional original, o el
            // default expr).
            let mut slots: Vec<Option<Expr>> = (0..sig.param_names.len()).map(|_| None).collect();
            let mut next_pos = 0usize;
            let mut after_named = false;
            for arg in args {
                if let Expr::NamedArg { name, value, .. } = arg {
                    after_named = true;
                    let idx = sig.param_names.iter().position(|p| p == name).ok_or_else(|| {
                        self.err_at(call_span, format!(
                            "`{}` no tiene un parámetro llamado `{}`",
                            callee_expr, name
                        ))
                    })?;
                    if slots[idx].is_some() {
                        return Err(self.err_at(call_span, format!(
                            "`{}`: el argumento `{}` está duplicado",
                            callee_expr, name
                        )));
                    }
                    slots[idx] = Some((**value).clone());
                } else {
                    if after_named {
                        return Err(self.err_at(call_span, format!(
                            "`{}`: no se puede pasar un argumento posicional después de uno nombrado",
                            callee_expr
                        )));
                    }
                    if next_pos >= sig.param_names.len() {
                        return Err(self.err_at(call_span, format!(
                            "`{}` espera {} argumento(s), recibió más",
                            callee_expr, sig.param_names.len()
                        )));
                    }
                    slots[next_pos] = Some(arg.clone());
                    next_pos += 1;
                }
            }
            // Rellenar None con default.
            let mut reordered: Vec<Expr> = Vec::with_capacity(sig.param_names.len());
            for (i, slot) in slots.into_iter().enumerate() {
                match slot {
                    Some(e) => reordered.push(e),
                    None => {
                        let de = sig.defaults[i].as_ref().ok_or_else(|| {
                            self.err_at(call_span, format!(
                                "`{}`: falta el argumento `{}` (no tiene default)",
                                callee_expr, sig.param_names[i]
                            ))
                        })?;
                        reordered.push(de.clone());
                    }
                }
            }
            // Recursar con args posicionales puros.
            return self.gen_call_with_sig(callee_expr, sig, &reordered, call_span);
        }
        // Fp.2 — varargs: aridad mínima excluye el varargs (puede recibir
        // 0 args); máxima = ilimitada. Sin varargs: aridad mínima =
        // params SIN default; máxima = total.
        let required_without_defaults = sig.defaults.iter().filter(|d| d.is_none()).count();
        let required = if sig.has_varargs {
            required_without_defaults.min(sig.params.len().saturating_sub(1))
        } else {
            required_without_defaults
        };
        let too_many = !sig.has_varargs && args.len() > sig.params.len();
        if args.len() < required || too_many {
            return Err(self.err_at(call_span, if sig.has_varargs {
                format!(
                    "`{}` espera al menos {} argumento(s), recibió {}",
                    callee_expr, required, args.len(),
                )
            } else if required == sig.params.len() {
                format!(
                    "`{}` espera {} argumento(s), recibió {}",
                    callee_expr, sig.params.len(), args.len(),
                )
            } else {
                format!(
                    "`{}` espera entre {} y {} argumento(s), recibió {}",
                    callee_expr, required, sig.params.len(), args.len(),
                )
            }));
        }
        let varargs_idx = if sig.has_varargs { Some(sig.params.len() - 1) } else { None };
        let mut arg_codes: Vec<String> = Vec::with_capacity(sig.params.len());

        // Args posicionales hasta el varargs (si hay).
        let positional_count = if let Some(i) = varargs_idx { i } else { sig.params.len() };
        for (i, expected) in sig.params.iter().enumerate().take(positional_count) {
            if i < args.len() {
                let (code, ty) = self.gen_expr(&args[i])?;
                arg_codes.push(coerce(&code, &ty, expected));
            } else {
                // Fill con default.
                let default_expr = sig.defaults[i].as_ref().ok_or_else(|| {
                    self.err_at(call_span, format!(
                        "`{}`: el parámetro {} no tiene default y no fue provisto (bug interno)",
                        callee_expr, i + 1,
                    ))
                })?;
                let (code, ty) = self.gen_expr(default_expr)?;
                arg_codes.push(coerce(&code, &ty, &sig.params[i]));
            }
        }

        // Fp.2 — args del varargs: se empaquetan en una `List<T>` Fitz
        // (`Arc<Mutex<Vec<T>>>`). Construimos un `vec![item1, item2, ...]`
        // y lo envolvemos. Si no hay args extra, queda como List vacío.
        if let Some(varargs_idx) = varargs_idx {
            let elem_ty = &sig.params[varargs_idx];
            let mut extras: Vec<String> = Vec::new();
            for arg in args.iter().skip(positional_count) {
                let (code, ty) = self.gen_expr(arg)?;
                extras.push(coerce(&code, &ty, elem_ty));
            }
            // `Arc::new(Mutex::new(vec![...]))` — paralelo al codegen
            // estándar de literales de lista (`Expr::List`).
            let list_code = format!(
                "std::sync::Arc::new(std::sync::Mutex::new(vec![{}]))",
                extras.join(", ")
            );
            arg_codes.push(list_code);
        }

        Ok((
            format!("{}({})", callee_expr, arg_codes.join(", ")),
            sig.ret.clone(),
        ))
    }

    // -----------------------------------------------------------------
    // Fase 9.w.1.d — Built-ins jwt + hash en codegen.
    //
    // Dispatchers de `jwt.encode/decode`, `hash.password/verify` desde
    // `gen_call`. Cada uno valida aridad y tipos de los args al call
    // site, emite la llamada al helper Rust correspondiente del
    // preludio (`__fitz_jwt_encode`/etc.), y devuelve el tipo Fitz
    // adecuado para que el resto del codegen lo coercione si hace
    // falta. Política MVP:
    //
    // - `jwt.encode`: payload restringido a `Map<Str, Str>` strict
    //   (heterogéneos requieren `__FitzValue`, post-MVP). Devuelve
    //   `Type::Str`.
    // - `jwt.decode`: devuelve `Type::Result { ok: Map<Str, Str>, err: Str }`.
    // - `hash.password`: devuelve `Type::Str`.
    // - `hash.verify`: devuelve `Type::Bool`.
    // -----------------------------------------------------------------

    fn gen_auth_jwt_encode(
        &mut self,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        if args.len() < 2 || args.len() > 3 {
            return Err(self.err_at(call_span, format!(
                "`jwt.encode` espera 2 o 3 argumentos (payload: Map<Str, Str>, secret: Str, alg: Str?), recibió {}",
                args.len()
            )));
        }
        let (payload_code, payload_ty) = self.gen_expr(&args[0])?;
        let payload_ok = matches!(
            &payload_ty,
            Type::Map(k, v) if matches!(k.as_ref(), Type::Str) && matches!(v.as_ref(), Type::Str)
        );
        if !payload_ok {
            return Err(self.err_at(args[0].span(), format!(
                "`jwt.encode` en `fitz build` MVP: el payload debe ser `Map<Str, Str>` strict. \
                 Heterogéneos (`Map<Str, Any>`) son deuda post-MVP. Recibió `{}`.",
                payload_ty.display(self.env)
            )));
        }
        let (secret_code, secret_ty) = self.gen_expr(&args[1])?;
        let secret_c = coerce(&secret_code, &secret_ty, &Type::Str);
        let alg_code = if args.len() == 3 {
            let (a_code, a_ty) = self.gen_expr(&args[2])?;
            let a_c = coerce(&a_code, &a_ty, &Type::Str);
            format!("Some({})", a_c)
        } else {
            "None".to_string()
        };
        Ok((
            format!(
                "__fitz_jwt_encode({}, {}, {})",
                payload_code, secret_c, alg_code
            ),
            Type::Str,
        ))
    }

    fn gen_auth_jwt_decode(
        &mut self,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        if args.len() < 2 || args.len() > 3 {
            return Err(self.err_at(call_span, format!(
                "`jwt.decode` espera 2 o 3 argumentos (token: Str, secret: Str, alg: Str?), recibió {}",
                args.len()
            )));
        }
        let (token_code, token_ty) = self.gen_expr(&args[0])?;
        let token_c = coerce(&token_code, &token_ty, &Type::Str);
        let (secret_code, secret_ty) = self.gen_expr(&args[1])?;
        let secret_c = coerce(&secret_code, &secret_ty, &Type::Str);
        let alg_code = if args.len() == 3 {
            let (a_code, a_ty) = self.gen_expr(&args[2])?;
            let a_c = coerce(&a_code, &a_ty, &Type::Str);
            format!("Some({})", a_c)
        } else {
            "None".to_string()
        };
        Ok((
            format!(
                "__fitz_jwt_decode({}, {}, {})",
                token_c, secret_c, alg_code
            ),
            Type::Result {
                ok: Box::new(Type::Map(Box::new(Type::Str), Box::new(Type::Str))),
                err: Box::new(Type::Str),
            },
        ))
    }

    fn gen_auth_hash_password(
        &mut self,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        if args.len() != 1 {
            return Err(self.err_at(call_span, format!(
                "`hash.password` espera 1 argumento (plain: Str), recibió {}",
                args.len()
            )));
        }
        let (code, ty) = self.gen_expr(&args[0])?;
        let coerced = coerce(&code, &ty, &Type::Str);
        Ok((
            format!("__fitz_hash_password({})", coerced),
            Type::Str,
        ))
    }

    fn gen_auth_hash_verify(
        &mut self,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        if args.len() != 2 {
            return Err(self.err_at(call_span, format!(
                "`hash.verify` espera 2 argumentos (plain: Str, hashed: Str), recibió {}",
                args.len()
            )));
        }
        let (plain_code, plain_ty) = self.gen_expr(&args[0])?;
        let plain_c = coerce(&plain_code, &plain_ty, &Type::Str);
        let (hashed_code, hashed_ty) = self.gen_expr(&args[1])?;
        let hashed_c = coerce(&hashed_code, &hashed_ty, &Type::Str);
        Ok((
            format!("__fitz_hash_verify({}, {})", plain_c, hashed_c),
            Type::Bool,
        ))
    }

    fn gen_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        // Fase 9.w.2.c — Métodos sobre `WsConn<T>`. Detectamos por tipo
        // sintetizado del receiver. `recv`/`send`/`broadcast` son async
        // — emitimos `.await` automático para que el call site Fitz
        // (`let r = conn.recv()`) se traduzca a Rust válido. `close`
        // es sync. El checker (9.w.2.a) ya validó aridad y tipos de
        // args; acá nos limitamos a emitir el Rust.
        if let Ok((obj_code, Type::WsConn(inner_t_box))) = self.gen_expr(object) {
            let inner_t: &Type = &inner_t_box;
            {
                match method {
                    "recv" => {
                        if !args.is_empty() {
                            return Err(self.err_at(call_span,
                                format!("`WsConn.recv()` no acepta args, recibió {}", args.len()),
                            ));
                        }
                        return Ok((
                            format!("({}).recv().await", obj_code),
                            Type::Result {
                                ok: Box::new(inner_t.clone()),
                                err: Box::new(Type::Str),
                            },
                        ));
                    }
                    "send" | "broadcast" => {
                        if args.len() != 1 {
                            return Err(self.err_at(call_span, format!(
                                "`WsConn.{}(msg)` espera 1 arg, recibió {}",
                                method, args.len()
                            )));
                        }
                        let (msg_code, msg_ty) = self.gen_expr(&args[0])?;
                        let coerced = coerce(&msg_code, &msg_ty, inner_t);
                        return Ok((
                            format!("({}).{}({}).await", obj_code, method, coerced),
                            Type::Result {
                                ok: Box::new(Type::Null),
                                err: Box::new(Type::Str),
                            },
                        ));
                    }
                    "close" => {
                        if !args.is_empty() {
                            return Err(self.err_at(call_span,
                                format!("`WsConn.close()` no acepta args, recibió {}", args.len()),
                            ));
                        }
                        return Ok((
                            format!("({{ ({}).close(); () }})", obj_code),
                            Type::Null,
                        ));
                    }
                    _ => {
                        return Err(self.err_at(call_span, format!(
                            "`WsConn<T>` no tiene método `{}` (soportados: recv, send, broadcast, close)",
                            method,
                        )));
                    }
                }
            }
        }
        // Mini-tanda St — static method dispatch: `Type.method(args)`.
        // El object es `Expr::Ident("Type")` que NO es un valor sino un
        // tipo. Lo detectamos antes que `gen_expr(object)` falle y
        // emitimos `<Type>::<method>(args)` Rust nativo si el método
        // existe y es estático.
        if let Expr::Ident(name, _) = object {
            if let Some(methods) = self.type_methods.get(name).cloned() {
                if let Some(m) = methods.iter().find(|md| md.name == method).cloned() {
                    if !m.is_static {
                        return Err(self.err_at(call_span, format!(
                            "`{}.{}()` es método de instancia; invocá como `<instancia>.{}(...)`, no como `{}.{}(...)`",
                            name, method, method, name, method,
                        )));
                    }
                    return self.gen_static_method_call(name, &m, args, call_span);
                }
            }
        }

        // Mini-tanda Rg — `(start..end).step_by(n)` se emite SIN
        // materializar el rango primero (eso desperdiciaría memoria
        // y derrotaría el sentido de `step_by`). Detectamos el caso
        // antes que el bloque general de Range que materializa a
        // `Vec<i64>` para enumerate/zip/chain.
        if method == "step_by" {
            if let Expr::Range { start, end, inclusive, .. } = object {
                check_method_arity(method, args, 1)?;
                let (start_code, start_ty) = self.gen_expr(start)?;
                let (end_code, end_ty) = self.gen_expr(end)?;
                let start_c = coerce(&start_code, &start_ty, &Type::Int);
                let end_c = coerce(&end_code, &end_ty, &Type::Int);
                let end_final = if *inclusive {
                    format!("({}) + 1i64", end_c)
                } else {
                    end_c
                };
                let (n_code, n_ty) = self.gen_expr(&args[0])?;
                let n_c = coerce(&n_code, &n_ty, &Type::Int);
                let code = format!(
                    "{{ let __step: i64 = {n_c}; \
                       if __step <= 0 {{ panic!(\"`Range.step_by()` requiere n > 0, recibió {{}}\", __step); }} \
                       Arc::new(Mutex::new(({start_c}..{end_final}).step_by(__step as usize).collect::<Vec<i64>>())) }}"
                );
                return Ok((code, Type::List(Box::new(Type::Int))));
            }
        }

        // Mini-tanda Ir — `(start..end).enumerate()`/`zip()`/`chain()`/
        // `len()`. El `Range` NO está soportado como valor general en
        // codegen (`gen_expr` lo rechaza), pero como receptor de un
        // método podemos materializarlo inline a un `Vec<i64>` y dejar
        // que el resto del dispatch lo trate como `List<Int>`. Habilita
        // el patrón canónico `for (i, n) in (0..10).enumerate()`.
        let (obj_code, obj_ty) = if let Expr::Range { start, end, inclusive, .. } = object {
            let (start_code, start_ty) = self.gen_expr(start)?;
            let (end_code, end_ty) = self.gen_expr(end)?;
            let start_c = coerce(&start_code, &start_ty, &Type::Int);
            let end_c = coerce(&end_code, &end_ty, &Type::Int);
            // Inclusivo: sumamos 1 al end (paralelo a parser R.1.4 que
            // materializa el rango inclusivo así).
            let end_final = if *inclusive {
                format!("({}) + 1i64", end_c)
            } else {
                end_c
            };
            let code = format!(
                "Arc::new(Mutex::new(({}..{}).collect::<Vec<i64>>()))",
                start_c, end_final
            );
            (code, Type::List(Box::new(Type::Int)))
        } else {
            self.gen_expr(object)?
        };
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
            return Ok((code, Type::Result { ok: Box::new(Type::PyAny), err: Box::new(Type::Str) }));
        }
        match (&obj_ty, method) {
            // ---- F13.D — methods universales sobre cualquier tipo
            //       para type-check dinámico ----
            //
            // Sobre `Type::Any` (típico: items de heterogéneos), el
            // receiver es `__FitzValue` y dispatcha por variant.
            // Sobre tipos concretos (`Int`, `Str`, etc.), match
            // estático y emit constante.
            // F13.D — helper para emitir el match común "as_<tipo>"
            // con mensaje alineado al intérprete. Ya que el match
            // recorre las variantes de FitzValue, también extrae el
            // type_name desde la variant.
            (Type::Any, "as_int") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "(match &({}) {{ \
                        __FitzValue::Int(__n) => Ok::<i64, String>(*__n), \
                        __other => Err::<i64, String>(format!(\"as_int: el valor es {{}}, no Int\", __fv_type_name(__other))), \
                    }})",
                    obj_code
                );
                Ok((code, Type::Result { ok: Box::new(Type::Int), err: Box::new(Type::Str) }))
            }
            (Type::Any, "as_float") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "(match &({}) {{ \
                        __FitzValue::Float(__x) => Ok::<f64, String>(*__x), \
                        __FitzValue::Int(__n) => Ok::<f64, String>(*__n as f64), \
                        __other => Err::<f64, String>(format!(\"as_float: el valor es {{}}, no Float\", __fv_type_name(__other))), \
                    }})",
                    obj_code
                );
                Ok((code, Type::Result { ok: Box::new(Type::Float), err: Box::new(Type::Str) }))
            }
            (Type::Any, "as_str") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "(match &({}) {{ \
                        __FitzValue::Str(__s) => Ok::<String, String>(__s.clone()), \
                        __other => Err::<String, String>(format!(\"as_str: el valor es {{}}, no Str\", __fv_type_name(__other))), \
                    }})",
                    obj_code
                );
                Ok((code, Type::Result { ok: Box::new(Type::Str), err: Box::new(Type::Str) }))
            }
            (Type::Any, "as_bool") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "(match &({}) {{ \
                        __FitzValue::Bool(__b) => Ok::<bool, String>(*__b), \
                        __other => Err::<bool, String>(format!(\"as_bool: el valor es {{}}, no Bool\", __fv_type_name(__other))), \
                    }})",
                    obj_code
                );
                Ok((code, Type::Result { ok: Box::new(Type::Bool), err: Box::new(Type::Str) }))
            }
            (Type::Any, "as_bytes") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "(match &({}) {{ \
                        __FitzValue::Bytes(__bs) => Ok::<Vec<u8>, String>(__bs.clone()), \
                        __other => Err::<Vec<u8>, String>(format!(\"as_bytes: el valor es {{}}, no Bytes\", __fv_type_name(__other))), \
                    }})",
                    obj_code
                );
                Ok((code, Type::Result { ok: Box::new(Type::Bytes), err: Box::new(Type::Str) }))
            }
            (Type::Any, "type_name") => {
                check_method_arity(method, args, 0)?;
                let code = format!("__fv_type_name(&({})).to_string()", obj_code);
                Ok((code, Type::Str))
            }

            // ---- Mini-tanda Bytes — métodos sobre `Type::Bytes` ----
            (Type::Bytes, "len") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("(({}).len() as i64)", obj_code), Type::Int))
            }
            (Type::Bytes, "is_empty") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).is_empty()", obj_code), Type::Bool))
            }
            (Type::Bytes, "to_str") => {
                check_method_arity(method, args, 0)?;
                // String::from_utf8(Vec<u8>) → Result<String, FromUtf8Error>.
                // Lo wrapeamos al shape de Fitz `Result<Str>` (Err=String).
                // `obj_code` ya viene como `Vec<u8>` por valor (clone),
                // así que NO agregamos otro `.clone()`.
                let code = format!(
                    "{{ let __r: Result<String, String> = match String::from_utf8({}) {{ \
                        Ok(__s) => Ok(__s), \
                        Err(__e) => Err(format!(\"Bytes.to_str(): contenido no es UTF-8 válido en offset {{}}\", __e.utf8_error().valid_up_to())) \
                    }}; __r }}",
                    obj_code,
                );
                Ok((code, Type::Result { ok: Box::new(Type::Str), err: Box::new(Type::Str) }))
            }
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
            // S.1 — `contains`/`starts_with`/`ends_with` toman 1 arg
            // `Str` y devuelven `Bool`. Rust `str::contains/starts_with/
            // ends_with` aceptan `&str` directo. Coercionamos el arg
            // a `&str` para uniformar.
            (Type::Str, "contains") => {
                check_method_arity(method, args, 1)?;
                let (a_code, a_ty) = self.gen_expr(&args[0])?;
                let coerced = coerce(&a_code, &a_ty, &Type::Str);
                Ok((format!("({}).contains({}.as_str())", obj_code, coerced), Type::Bool))
            }
            (Type::Str, "starts_with") => {
                check_method_arity(method, args, 1)?;
                let (a_code, a_ty) = self.gen_expr(&args[0])?;
                let coerced = coerce(&a_code, &a_ty, &Type::Str);
                Ok((format!("({}).starts_with({}.as_str())", obj_code, coerced), Type::Bool))
            }
            (Type::Str, "ends_with") => {
                check_method_arity(method, args, 1)?;
                let (a_code, a_ty) = self.gen_expr(&args[0])?;
                let coerced = coerce(&a_code, &a_ty, &Type::Str);
                Ok((format!("({}).ends_with({}.as_str())", obj_code, coerced), Type::Bool))
            }
            // Mini-tanda Mb3 — `chars()` devuelve `List<Str>` con
            // cada char del string. Rust `str::chars()` devuelve un
            // iterator de `char`; lo materializamos como Vec<String>
            // con 1 char cada uno via `to_string()`.
            (Type::Str, "chars") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "Arc::new(Mutex::new(({}).chars().map(|__c| __c.to_string()).collect::<Vec<String>>()))",
                    obj_code
                );
                Ok((code, Type::List(Box::new(Type::Str))))
            }
            // Mini-tanda Mb5 — `lines()` → `List<Str>`. Reusa
            // `str::lines` Rust (separa por `\n` y descarta `\n` final
            // sin agregar línea vacía).
            (Type::Str, "lines") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "Arc::new(Mutex::new(({}).lines().map(String::from).collect::<Vec<String>>()))",
                    obj_code
                );
                Ok((code, Type::List(Box::new(Type::Str))))
            }
            // Mini-tanda Mb5 — `is_empty()` → `Bool`.
            (Type::Str, "is_empty") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).is_empty()", obj_code), Type::Bool))
            }
            // Mini-tanda Mb4 — `split_at(idx)`: divide en char idx →
            // `(Str, Str)`. idx negativo → panic; idx > len → ("s", "").
            (Type::Str, "split_at") => {
                check_method_arity(method, args, 1)?;
                let (i_code, i_ty) = self.gen_expr(&args[0])?;
                let i_c = coerce(&i_code, &i_ty, &Type::Int);
                let code = format!(
                    "{{ let __s: String = {obj_code}; \
                       let __idx: i64 = {i_c}; \
                       if __idx < 0 {{ panic!(\"`Str.split_at()` no acepta índice negativo: recibió {{}}\", __idx); }} \
                       let __len: i64 = __s.chars().count() as i64; \
                       let __clamped: usize = __idx.min(__len) as usize; \
                       let __left: String = __s.chars().take(__clamped).collect(); \
                       let __right: String = __s.chars().skip(__clamped).collect(); \
                       (__left, __right) }}"
                );
                Ok((code, Type::Tuple(vec![Type::Str, Type::Str])))
            }
            // S.2 — `split` devuelve `List<Str>` = `Arc<Mutex<Vec<String>>>`.
            // Rust `str::split` devuelve un iterator de `&str`; lo
            // materializamos a `Vec<String>` via `.map(String::from)`.
            (Type::Str, "split") => {
                check_method_arity(method, args, 1)?;
                let (a_code, a_ty) = self.gen_expr(&args[0])?;
                let coerced = coerce(&a_code, &a_ty, &Type::Str);
                let code = format!(
                    "Arc::new(Mutex::new(({}).split({}.as_str()).map(String::from).collect::<Vec<_>>()))",
                    obj_code, coerced
                );
                Ok((code, Type::List(Box::new(Type::Str))))
            }
            (Type::Str, "trim") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).trim().to_string()", obj_code), Type::Str))
            }
            // Mini-tanda Mb — trim_start / trim_end.
            (Type::Str, "trim_start") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).trim_start().to_string()", obj_code), Type::Str))
            }
            (Type::Str, "trim_end") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).trim_end().to_string()", obj_code), Type::Str))
            }
            (Type::Str, "replace") => {
                check_method_arity(method, args, 2)?;
                let (old_code, old_ty) = self.gen_expr(&args[0])?;
                let (new_code, new_ty) = self.gen_expr(&args[1])?;
                let old_c = coerce(&old_code, &old_ty, &Type::Str);
                let new_c = coerce(&new_code, &new_ty, &Type::Str);
                Ok((format!("({}).replace({}.as_str(), {}.as_str())", obj_code, old_c, new_c), Type::Str))
            }
            // S.2 — `s.repeat(n)`: Rust `str::repeat` toma `usize`. El
            // intérprete chequea `n < 0` y emite error claro; el
            // binario también: si `n < 0`, panicamos con el mismo
            // mensaje. Conversión `i64 → usize` directa (cast usize
            // truncaría — usamos try_into con expect).
            (Type::Str, "repeat") => {
                check_method_arity(method, args, 1)?;
                let (n_code, n_ty) = self.gen_expr(&args[0])?;
                let coerced = coerce(&n_code, &n_ty, &Type::Int);
                let code = format!(
                    "({{ let __n: i64 = {}; if __n < 0 {{ panic!(\"`.repeat()` no acepta n negativo: recibió {{}}\", __n); }} ({}).repeat(__n as usize) }})",
                    coerced, obj_code
                );
                Ok((code, Type::Str))
            }
            // Mini-tanda Ex — Str search (find / index_of / last_index_of).
            // Rust `str::find` devuelve byte index; convertimos a char
            // index via `s[..idx].chars().count()`. Output:
            // `Result<i64, String>`.
            (Type::Str, "find") | (Type::Str, "index_of") => {
                check_method_arity(method, args, 1)?;
                let (a_code, a_ty) = self.gen_expr(&args[0])?;
                let coerced = coerce(&a_code, &a_ty, &Type::Str);
                let code = format!(
                    "{{ let __s: String = {}; let __needle: String = {}; \
                     match __s.find(__needle.as_str()) {{ \
                         Some(__b) => Ok(__s[..__b].chars().count() as i64), \
                         None => Err(String::from(\"no encontrado\")) \
                     }} }}",
                    obj_code, coerced,
                );
                Ok((code, Type::Result { ok: Box::new(Type::Int), err: Box::new(Type::Str) }))
            }
            (Type::Str, "last_index_of") => {
                check_method_arity(method, args, 1)?;
                let (a_code, a_ty) = self.gen_expr(&args[0])?;
                let coerced = coerce(&a_code, &a_ty, &Type::Str);
                let code = format!(
                    "{{ let __s: String = {}; let __needle: String = {}; \
                     match __s.rfind(__needle.as_str()) {{ \
                         Some(__b) => Ok(__s[..__b].chars().count() as i64), \
                         None => Err(String::from(\"no encontrado\")) \
                     }} }}",
                    obj_code, coerced,
                );
                Ok((code, Type::Result { ok: Box::new(Type::Int), err: Box::new(Type::Str) }))
            }
            // Mini-tanda Mb2 — `pad_start(width, ch)` / `pad_end(width, ch)`.
            // El padding usa `repeat` en runtime; `ch.chars().count() != 1`
            // panicamos con mensaje claro (paralelo al evaluator). Output:
            // `String`. Si `len(s) >= width`, devolvemos `s` sin cambios.
            (Type::Str, "pad_start") | (Type::Str, "pad_end") => {
                check_method_arity(method, args, 2)?;
                let at_start = method == "pad_start";
                let (w_code, w_ty) = self.gen_expr(&args[0])?;
                let (ch_code, ch_ty) = self.gen_expr(&args[1])?;
                let w_c = coerce(&w_code, &w_ty, &Type::Int);
                let ch_c = coerce(&ch_code, &ch_ty, &Type::Str);
                let pad_concat = if at_start {
                    "format!(\"{}{}\", __pad, __s)"
                } else {
                    "format!(\"{}{}\", __s, __pad)"
                };
                let code = format!(
                    "{{ let __s: String = {obj_code}; \
                       let __width: i64 = {w_c}; \
                       let __ch: String = {ch_c}; \
                       if __ch.chars().count() != 1 {{ \
                           panic!(\"`.{method}(width, ch)`: el char de relleno debe ser exactamente 1 caracter, recibió `\\\"{{}}\\\"` ({{}} chars)\", __ch, __ch.chars().count()); \
                       }} \
                       let __len: i64 = __s.chars().count() as i64; \
                       if __len >= __width {{ __s }} else {{ \
                           let __pad = __ch.repeat((__width - __len) as usize); \
                           {pad_concat} \
                       }} }}"
                );
                Ok((code, Type::Str))
            }
            // Mini-tanda Mb9 — swap_case / title / is_alpha / is_digit /
            // is_numeric. Reusan métodos `char::is_*` de Rust.
            (Type::Str, "swap_case") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "({}).chars().map(|c| if c.is_uppercase() {{ c.to_lowercase().collect::<String>() }} \
                       else if c.is_lowercase() {{ c.to_uppercase().collect::<String>() }} \
                       else {{ c.to_string() }}).collect::<String>()",
                    obj_code,
                );
                Ok((code, Type::Str))
            }
            (Type::Str, "title") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "{{ let __s: String = {obj_code}; \
                       let mut __out = String::with_capacity(__s.len()); \
                       let mut __start = true; \
                       for __c in __s.chars() {{ \
                           if __c.is_whitespace() {{ __out.push(__c); __start = true; }} \
                           else if __start {{ __out.extend(__c.to_uppercase()); __start = false; }} \
                           else {{ __out.extend(__c.to_lowercase()); }} \
                       }} \
                       __out }}"
                );
                Ok((code, Type::Str))
            }
            (Type::Str, "is_alpha") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "{{ let __s: String = {obj_code}; !__s.is_empty() && __s.chars().all(|c| c.is_alphabetic()) }}"
                );
                Ok((code, Type::Bool))
            }
            (Type::Str, "is_digit") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "{{ let __s: String = {obj_code}; !__s.is_empty() && __s.chars().all(|c| c.is_ascii_digit()) }}"
                );
                Ok((code, Type::Bool))
            }
            (Type::Str, "is_numeric") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "{{ let __s: String = {obj_code}; !__s.is_empty() && __s.parse::<f64>().is_ok() }}"
                );
                Ok((code, Type::Bool))
            }
            // Mini-tanda Mb8 — `left(n)` / `right(n)`: primeros/últimos
            // n chars. n <= 0 → vacío; n >= len → string completo.
            (Type::Str, "left") => {
                check_method_arity(method, args, 1)?;
                let (n_code, n_ty) = self.gen_expr(&args[0])?;
                let n_c = coerce(&n_code, &n_ty, &Type::Int);
                let code = format!(
                    "{{ let __s: String = {obj_code}; \
                       let __n: i64 = {n_c}; \
                       let __take = if __n <= 0 {{ 0 }} else {{ __n as usize }}; \
                       __s.chars().take(__take).collect::<String>() }}"
                );
                Ok((code, Type::Str))
            }
            (Type::Str, "right") => {
                check_method_arity(method, args, 1)?;
                let (n_code, n_ty) = self.gen_expr(&args[0])?;
                let n_c = coerce(&n_code, &n_ty, &Type::Int);
                let code = format!(
                    "{{ let __s: String = {obj_code}; \
                       let __n: i64 = {n_c}; \
                       let __len = __s.chars().count(); \
                       let __take = if __n <= 0 {{ 0 }} else {{ (__n as usize).min(__len) }}; \
                       let __skip = __len - __take; \
                       __s.chars().skip(__skip).collect::<String>() }}"
                );
                Ok((code, Type::Str))
            }
            // Mini-tanda Mb8 — `center(width, ch)`: padding bilateral.
            (Type::Str, "center") => {
                check_method_arity(method, args, 2)?;
                let (w_code, w_ty) = self.gen_expr(&args[0])?;
                let (ch_code, ch_ty) = self.gen_expr(&args[1])?;
                let w_c = coerce(&w_code, &w_ty, &Type::Int);
                let ch_c = coerce(&ch_code, &ch_ty, &Type::Str);
                let code = format!(
                    "{{ let __s: String = {obj_code}; \
                       let __width: i64 = {w_c}; \
                       let __ch: String = {ch_c}; \
                       if __ch.chars().count() != 1 {{ \
                           panic!(\"`Str.center(width, ch)`: el char de relleno debe ser 1 caracter, recibió `\\\"{{}}\\\"`\", __ch); \
                       }} \
                       let __len: i64 = __s.chars().count() as i64; \
                       if __len >= __width {{ __s }} else {{ \
                           let __total = (__width - __len) as usize; \
                           let __left = __total / 2; \
                           let __right = __total - __left; \
                           let mut __out = String::with_capacity(__width as usize); \
                           __out.push_str(&__ch.repeat(__left)); \
                           __out.push_str(&__s); \
                           __out.push_str(&__ch.repeat(__right)); \
                           __out \
                       }} }}"
                );
                Ok((code, Type::Str))
            }
            // Mini-tanda Mb7 — `repeat_with(n, sep)`: repite intercalando
            // sep. n < 0 → panic; n == 0 → vacío.
            (Type::Str, "repeat_with") => {
                check_method_arity(method, args, 2)?;
                let (n_code, n_ty) = self.gen_expr(&args[0])?;
                let (sep_code, sep_ty) = self.gen_expr(&args[1])?;
                let n_c = coerce(&n_code, &n_ty, &Type::Int);
                let sep_c = coerce(&sep_code, &sep_ty, &Type::Str);
                let code = format!(
                    "{{ let __s: String = {obj_code}; \
                       let __n: i64 = {n_c}; \
                       let __sep: String = {sep_c}; \
                       if __n < 0 {{ panic!(\"`.repeat_with()` no acepta n negativo: recibió {{}}\", __n); }} \
                       let __parts: Vec<&str> = std::iter::repeat(__s.as_str()).take(__n as usize).collect(); \
                       __parts.join(__sep.as_str()) }}"
                );
                Ok((code, Type::Str))
            }
            (Type::Str, other) => Err(self.err_at(call_span, format!(
                "Str no tiene el método `{}` en el subset compilado (hoy: len/upper/lower/contains/starts_with/ends_with/split/trim/trim_start/trim_end/replace/repeat/find/index_of/last_index_of/pad_start/pad_end/chars/split_at/lines/is_empty/repeat_with/left/right/center/swap_case/title/is_alpha/is_digit/is_numeric)",
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
            // S.3 (mini-tanda S) — métodos chicos sobre List.
            (Type::List(t), "sort") => self.gen_list_sort(&obj_code, t, args, call_span),
            (Type::List(_), "reverse") => {
                check_method_arity(method, args, 0)?;
                Ok((
                    format!("({}).lock().unwrap().reverse()", obj_code),
                    Type::Null,
                ))
            }
            (Type::List(t), "contains") => self.gen_list_contains(&obj_code, t, args),
            // Mini-tanda It — iteradores enumerate/zip/chain.
            (Type::List(t), "enumerate") => {
                check_method_arity(method, args, 0)?;
                let elem_rust = rust_type_for(t, self.env)?;
                // Emite Vec<(i64, T)> y lo envuelve en Arc<Mutex<>> igual
                // que cualquier List literal.
                let code = format!(
                    "Arc::new(Mutex::new(({obj_code}).lock().unwrap().iter().cloned().enumerate().map(|(__i, __v)| (__i as i64, __v)).collect::<Vec<(i64, {elem_rust})>>()))"
                );
                Ok((
                    code,
                    Type::List(Box::new(Type::Tuple(vec![Type::Int, (**t).clone()]))),
                ))
            }
            (Type::List(t), "zip") => {
                check_method_arity(method, args, 1)?;
                let (other_code, other_ty) = self.gen_expr(&args[0])?;
                let u_ty = match &other_ty {
                    Type::List(inner) => (**inner).clone(),
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`zip` espera `List<U>`, recibió `{}`",
                            display_type(other, self.env)
                        )));
                    }
                };
                let t_rust = rust_type_for(t, self.env)?;
                let u_rust = rust_type_for(&u_ty, self.env)?;
                let code = format!(
                    "Arc::new(Mutex::new(({obj_code}).lock().unwrap().iter().cloned().zip(({other_code}).lock().unwrap().iter().cloned()).collect::<Vec<({t_rust}, {u_rust})>>()))"
                );
                Ok((
                    code,
                    Type::List(Box::new(Type::Tuple(vec![(**t).clone(), u_ty]))),
                ))
            }
            (Type::List(t), "chain") => {
                check_method_arity(method, args, 1)?;
                let (other_code, other_ty) = self.gen_expr(&args[0])?;
                match &other_ty {
                    Type::List(inner) if lub(t, inner).is_ok() => {}
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`chain` espera `List<{}>`, recibió `{}`",
                            display_type(t, self.env),
                            display_type(other, self.env)
                        )));
                    }
                }
                let t_rust = rust_type_for(t, self.env)?;
                let code = format!(
                    "Arc::new(Mutex::new(({obj_code}).lock().unwrap().iter().cloned().chain(({other_code}).lock().unwrap().iter().cloned()).collect::<Vec<{t_rust}>>()))"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            // Mini-tanda Mb — `flatten()` requiere `List<List<U>>`.
            // Emite `Vec::iter().cloned().flat_map(|inner|
            // inner.lock().unwrap().clone()).collect()`.
            (Type::List(t), "flatten") => {
                check_method_arity(method, args, 0)?;
                let inner = match &**t {
                    Type::List(inner) => inner.clone(),
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.flatten()` requiere `List<List<U>>`, el receptor es `List<{}>`",
                            display_type(other, self.env)
                        )));
                    }
                };
                let inner_rs = rust_type_for(&inner, self.env)?;
                // Cada elemento del outer es `Arc<Mutex<Vec<U>>>`; clonamos
                // el lock para extraer los elementos sin colgar el guard
                // afuera del map. Output: `Arc<Mutex<Vec<U>>>`.
                let code = format!(
                    "Arc::new(Mutex::new(({obj_code}).lock().unwrap().iter().cloned().flat_map(|__sub| __sub.lock().unwrap().clone()).collect::<Vec<{inner_rs}>>()))"
                );
                Ok((code, Type::List(Box::new(*inner))))
            }
            // Mini-tanda Mb — `sort_by(cmp)` con callback `fn(T, T) -> Int`.
            // Muta IN-PLACE. El callback se invoca via FnExpr inline (igual
            // que map/filter/find); Rust `sort_by` espera `FnMut(&T, &T) ->
            // Ordering`, así que envolvemos el Fn de Fitz convirtiendo el
            // Int devuelto a `std::cmp::Ordering`.
            (Type::List(t), "sort_by") => {
                check_method_arity(method, args, 1)?;
                // El callback de Fitz toma DOS args (a, b). Hay que pasarlo
                // distinto de `gen_callback_inline` (que asume 1 arg).
                // Estrategia: si es FnExpr inline con 2 params, emitimos
                // una closure Rust idiomática con los nombres tal cual.
                let cb_code = self.gen_binary_callback_inline(&args[0], t, t, "sort_by")?;
                let t_rust = rust_type_for(t, self.env)?;
                // Bindeamos el `obj_code` a un local antes de tomar el
                // lock — sin esto, `(xs.clone()).lock()` produce un
                // temporario que rustc dropea al fin del stmt, fallando
                // E0716 "borrow later used here".
                let code = format!(
                    "{{ \
                        let __cb = {cb_code}; \
                        let __list = {obj_code}; \
                        let mut __guard = __list.lock().unwrap(); \
                        let mut __vec: Vec<{t_rust}> = std::mem::take(&mut *__guard); \
                        __vec.sort_by(|__a, __b| {{ \
                            let __r: i64 = __cb(__a.clone(), __b.clone()); \
                            if __r < 0 {{ std::cmp::Ordering::Less }} \
                            else if __r > 0 {{ std::cmp::Ordering::Greater }} \
                            else {{ std::cmp::Ordering::Equal }} \
                        }}); \
                        *__guard = __vec; \
                    }}"
                );
                Ok((code, Type::Null))
            }
            // Mini-tanda Lx — predicados funcionales any/all/count/find_index.
            // Rust `Iterator::any`/`all` toman `FnMut(T) -> bool`. Nuestro
            // callback Fitz es `fn(T) -> Bool` y se emite como closure
            // sync via `gen_callback_inline`. La firma encaja directo.
            (Type::List(t), "any") => {
                check_method_arity(method, args, 1)?;
                let (cb_code, _) = self.gen_callback_inline(&args[0], t, Some(&Type::Bool), "any")?;
                let code = format!(
                    "({obj_code}).lock().unwrap().iter().cloned().any({cb_code})"
                );
                Ok((code, Type::Bool))
            }
            (Type::List(t), "all") => {
                check_method_arity(method, args, 1)?;
                let (cb_code, _) = self.gen_callback_inline(&args[0], t, Some(&Type::Bool), "all")?;
                let code = format!(
                    "({obj_code}).lock().unwrap().iter().cloned().all({cb_code})"
                );
                Ok((code, Type::Bool))
            }
            (Type::List(t), "count") => {
                check_method_arity(method, args, 1)?;
                let (cb_code, _) = self.gen_callback_inline(&args[0], t, Some(&Type::Bool), "count")?;
                // Manual loop: `Iterator::filter` toma `FnMut(&T)`, no
                // `FnMut(T)`, así que no encaja con nuestra callback
                // sync que toma `T` por valor. Snapshot + for-loop
                // paralelo a `gen_list_filter`.
                let code = format!(
                    "{{ \
                        let __items: Vec<_> = ({obj_code}).lock().unwrap().clone(); \
                        let __cb = {cb_code}; \
                        let mut __n: i64 = 0; \
                        for __it in __items.into_iter() {{ \
                            if __cb(__it) {{ __n += 1; }} \
                        }} \
                        __n \
                    }}"
                );
                Ok((code, Type::Int))
            }
            (Type::List(t), "find_index") => {
                check_method_arity(method, args, 1)?;
                let (cb_code, _) = self.gen_callback_inline(&args[0], t, Some(&Type::Bool), "find_index")?;
                // Manual loop (mismo motivo que count + acceso al
                // índice). Devuelve `Result<i64, String>`.
                let code = format!(
                    "{{ \
                        let __items: Vec<_> = ({obj_code}).lock().unwrap().clone(); \
                        let __cb = {cb_code}; \
                        let mut __result: Result<i64, String> = \
                            Err(String::from(\"no encontrado\")); \
                        for (__i, __it) in __items.into_iter().enumerate() {{ \
                            if __cb(__it) {{ __result = Ok(__i as i64); break; }} \
                        }} \
                        __result \
                    }}"
                );
                Ok((code, Type::Result { ok: Box::new(Type::Int), err: Box::new(Type::Str) }))
            }
            // Mini-tanda Ex2 — flat_map: map + flatten en un paso.
            // El callback devuelve `List<U>`; el output es `List<U>`
            // (concatenación de todas las sub-listas).
            (Type::List(t), "flat_map") => {
                check_method_arity(method, args, 1)?;
                let (cb_code, cb_ret_ty) = self.gen_callback_inline(&args[0], t, None, "flat_map")?;
                let u_ty = match &cb_ret_ty {
                    Type::List(u) => (**u).clone(),
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.flat_map()`: el callback debe retornar `List<U>`, retorna `{}`",
                            display_type(other, self.env)
                        )));
                    }
                };
                let u_rs = rust_type_for(&u_ty, self.env)?;
                let code = format!(
                    "{{ let __items: Vec<_> = ({obj_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       let mut __out: Vec<{u_rs}> = Vec::new(); \
                       for __it in __items.into_iter() {{ \
                           let __sub = __cb(__it); \
                           __out.extend(__sub.lock().unwrap().clone()); \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new(u_ty))))
            }
            // Mini-tanda Ex2 — first() / last() devuelven `Result<T>`.
            // Bindeamos el `obj_code` a un local antes del lock para
            // evitar E0716 con temporaries de `(xs.clone()).lock()`.
            (Type::List(t), "first") => {
                check_method_arity(method, args, 0)?;
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __list = {obj_code}; let __g = __list.lock().unwrap(); \
                       match __g.first().cloned() {{ \
                           Some(__v) => Ok::<{t_rs}, String>(__v), \
                           None => Err(String::from(\"lista vacía\")) \
                       }} }}"
                );
                Ok((code, Type::Result { ok: Box::new((**t).clone()), err: Box::new(Type::Str) }))
            }
            (Type::List(t), "last") => {
                check_method_arity(method, args, 0)?;
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __list = {obj_code}; let __g = __list.lock().unwrap(); \
                       match __g.last().cloned() {{ \
                           Some(__v) => Ok::<{t_rs}, String>(__v), \
                           None => Err(String::from(\"lista vacía\")) \
                       }} }}"
                );
                Ok((code, Type::Result { ok: Box::new((**t).clone()), err: Box::new(Type::Str) }))
            }
            // Mini-tanda Mb2 — `min` / `max` sobre `List<Int>` o
            // `List<Float>`. Devuelven `Result<T>`; lista vacía → Err.
            // Para Float usamos `partial_cmp` con NaN handling (paralelo
            // a evaluator y `sort`). Para Int, usamos `iter().min()` /
            // `iter().max()` directos (Ord trait).
            (Type::List(t), "min") | (Type::List(t), "max") => {
                check_method_arity(method, args, 0)?;
                let is_min = method == "min";
                let t_rs = rust_type_for(t, self.env)?;
                let code = match &**t {
                    Type::Int => {
                        let cmp_fn = if is_min { "min" } else { "max" };
                        format!(
                            "{{ let __list = {obj_code}; let __g = __list.lock().unwrap(); \
                               match __g.iter().{cmp_fn}().copied() {{ \
                                   Some(__v) => Ok::<{t_rs}, String>(__v), \
                                   None => Err(String::from(\"lista vacía\")) \
                               }} }}"
                        )
                    }
                    Type::Float => {
                        let cmp_branch = if is_min {
                            "Some(std::cmp::Ordering::Less) => Some(__v)"
                        } else {
                            "Some(std::cmp::Ordering::Greater) => Some(__v)"
                        };
                        format!(
                            "{{ let __list = {obj_code}; let __g = __list.lock().unwrap(); \
                               let mut __best: Option<{t_rs}> = None; \
                               for __v in __g.iter().copied() {{ \
                                   __best = match __best {{ \
                                       None => Some(__v), \
                                       Some(__b) => match __v.partial_cmp(&__b) {{ \
                                           {cmp_branch}, \
                                           _ => Some(__b), \
                                       }}, \
                                   }}; \
                               }} \
                               match __best {{ \
                                   Some(__v) => Ok::<{t_rs}, String>(__v), \
                                   None => Err(String::from(\"lista vacía\")) \
                               }} }}"
                        )
                    }
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.{}()` solo se aplica sobre `List<Int>` o `List<Float>`, recibió `List<{}>`",
                            method, display_type(other, self.env)
                        )));
                    }
                };
                Ok((code, Type::Result { ok: Box::new((**t).clone()), err: Box::new(Type::Str) }))
            }
            // Mini-tanda Mb2 — `sum` sobre `List<Int>` o `List<Float>`.
            // Lista vacía → 0/0.0 (Rust `Iterator::sum` lo hace nativo).
            (Type::List(t), "sum") => {
                check_method_arity(method, args, 0)?;
                let t_rs = match &**t {
                    Type::Int | Type::Float => rust_type_for(t, self.env)?,
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.sum()` solo se aplica sobre `List<Int>` o `List<Float>`, recibió `List<{}>`",
                            display_type(other, self.env)
                        )));
                    }
                };
                let code = format!(
                    "({obj_code}).lock().unwrap().iter().copied().sum::<{t_rs}>()"
                );
                Ok((code, (**t).clone()))
            }
            // Mini-tanda Mb3 — `product` análogo a `sum`. Usa
            // `Iterator::product` (Rust nativo, vacío → 1/1.0).
            (Type::List(t), "product") => {
                check_method_arity(method, args, 0)?;
                let t_rs = match &**t {
                    Type::Int | Type::Float => rust_type_for(t, self.env)?,
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.product()` solo se aplica sobre `List<Int>` o `List<Float>`, recibió `List<{}>`",
                            display_type(other, self.env)
                        )));
                    }
                };
                let code = format!(
                    "({obj_code}).lock().unwrap().iter().copied().product::<{t_rs}>()"
                );
                Ok((code, (**t).clone()))
            }
            // Mini-tanda Mb3 — `reduce(init, fn(acc, x) -> Acc) -> Acc`.
            // Snapshot del receiver + for loop con el callback binario.
            // El init y Acc tienen el mismo tipo; el item tiene tipo T.
            (Type::List(t), "reduce") => {
                check_method_arity(method, args, 2)?;
                let (init_code, init_ty) = self.gen_expr(&args[0])?;
                // `Acc` es el tipo del init (concreto o Any si gradual).
                // Si init es literal con tipo inferible, lo tomamos como
                // Acc; sino fallback Any.
                let acc_ty = init_ty.clone();
                // Pasamos Acc como expected_ret porque el callback de
                // reduce produce el siguiente acc — mismo tipo que el
                // inicial, no Any genérico.
                let cb_code = self.gen_binary_callback_inline_with_ret(
                    &args[1], &acc_ty, t, &acc_ty, "reduce",
                )?;
                let acc_rs = rust_type_for(&acc_ty, self.env)?;
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __items: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       let mut __acc: {acc_rs} = {init_code}; \
                       for __it in __items.into_iter() {{ \
                           __acc = __cb(__acc, __it); \
                       }} \
                       __acc }}"
                );
                Ok((code, acc_ty))
            }
            // Mini-tanda Mb4 — `unique()`: dedup preservando orden de
            // 1ra aparición. Cualquier T. O(n²) por linear scan (mismo
            // approach que el evaluator). Para listas chicas (<1000)
            // es aceptable.
            (Type::List(t), "unique") => {
                check_method_arity(method, args, 0)?;
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __items: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let mut __out: Vec<{t_rs}> = Vec::with_capacity(__items.len()); \
                       for __v in __items.into_iter() {{ \
                           if !__out.iter().any(|__x| *__x == __v) {{ __out.push(__v); }} \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            // Mini-tanda Mb4 — `partition(pred)`: divide en (truthy, falsy).
            // Callback `fn(T) -> Bool`. Devuelve `(List<T>, List<T>)`.
            (Type::List(t), "partition") => {
                check_method_arity(method, args, 1)?;
                let (cb_code, _) = self.gen_callback_inline(&args[0], t, Some(&Type::Bool), "partition")?;
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __items: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       let mut __t: Vec<{t_rs}> = Vec::new(); \
                       let mut __f: Vec<{t_rs}> = Vec::new(); \
                       for __it in __items.into_iter() {{ \
                           if __cb(__it.clone()) {{ __t.push(__it); }} else {{ __f.push(__it); }} \
                       }} \
                       (Arc::new(Mutex::new(__t)), Arc::new(Mutex::new(__f))) }}"
                );
                Ok((
                    code,
                    Type::Tuple(vec![
                        Type::List(Box::new((**t).clone())),
                        Type::List(Box::new((**t).clone())),
                    ]),
                ))
            }
            // Mini-tanda Mb3 — `to_map()`: convierte `List<(K, V)>` →
            // `Map<K, V>`. Política last-write-wins (paralelo a Python
            // `dict(items)`). El tipo `T` del receptor debe ser
            // `Tuple` de aridad 2.
            (Type::List(t), "to_map") => {
                check_method_arity(method, args, 0)?;
                let (k_ty, v_ty) = match &**t {
                    Type::Tuple(items) if items.len() == 2 => {
                        (items[0].clone(), items[1].clone())
                    }
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.to_map()` requiere `List<(K, V)>` (Tuple aridad 2), recibió `List<{}>`",
                            display_type(other, self.env)
                        )));
                    }
                };
                let k_rs = rust_type_for(&k_ty, self.env)?;
                let v_rs = rust_type_for(&v_ty, self.env)?;
                // Last-write-wins: por cada par, si la key ya está, la
                // sobreescribimos; si no, push al final.
                let code = format!(
                    "{{ let __items: Vec<({k_rs}, {v_rs})> = ({obj_code}).lock().unwrap().clone(); \
                       let mut __out: Vec<({k_rs}, {v_rs})> = Vec::new(); \
                       for (__k, __v) in __items.into_iter() {{ \
                           if let Some(__slot) = __out.iter_mut().find(|(__ek, _)| *__ek == __k) {{ \
                               __slot.1 = __v; \
                           }} else {{ \
                               __out.push((__k, __v)); \
                           }} \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::Map(Box::new(k_ty), Box::new(v_ty))))
            }
            // Mini-tanda Mb5 — group_by(fn(T) -> K) → Map<K, List<T>>.
            // Emite snapshot + for loop con find-or-push estilo
            // last-write-wins paralelo a `to_map`.
            (Type::List(t), "group_by") => {
                check_method_arity(method, args, 1)?;
                let (cb_code, k_ty) = self.gen_callback_inline(&args[0], t, None, "group_by")?;
                let t_rs = rust_type_for(t, self.env)?;
                let k_rs = rust_type_for(&k_ty, self.env)?;
                let code = format!(
                    "{{ let __items: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       let mut __groups: Vec<({k_rs}, Vec<{t_rs}>)> = Vec::new(); \
                       for __it in __items.into_iter() {{ \
                           let __k: {k_rs} = __cb(__it.clone()); \
                           if let Some(__slot) = __groups.iter_mut().find(|__p| __p.0 == __k) {{ \
                               __slot.1.push(__it); \
                           }} else {{ \
                               __groups.push((__k, vec![__it])); \
                           }} \
                       }} \
                       Arc::new(Mutex::new(__groups.into_iter().map(|(__k, __vs)| (__k, Arc::new(Mutex::new(__vs)))).collect::<Vec<({k_rs}, Arc<Mutex<Vec<{t_rs}>>>)>>())) }}"
                );
                Ok((
                    code,
                    Type::Map(Box::new(k_ty), Box::new(Type::List(Box::new((**t).clone())))),
                ))
            }
            // Mini-tanda Mb5 — zip_with(ys, fn(T, U) -> V) → List<V>.
            // Combina zip + map en un paso. Trunca al más corto.
            (Type::List(t), "zip_with") => {
                check_method_arity(method, args, 2)?;
                let (other_code, other_ty) = self.gen_expr(&args[0])?;
                let u_ty = match &other_ty {
                    Type::List(inner) => (**inner).clone(),
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.zip_with()` espera `List<U>` como primer arg, recibió `{}`",
                            display_type(other, self.env)
                        )));
                    }
                };
                // Inferimos V (ret del callback) ANTES de emitir el
                // closure binario, así pasamos el tipo concreto al
                // helper (necesario para que el `-> V` Rust no sea
                // `_` que rustc no puede inferir en algunos casos).
                let v_ty = match &args[1] {
                    Expr::FnExpr { params, body, .. } => self
                        .infer_callback_ret_silently_binary_named(params, body, t, &u_ty)
                        .unwrap_or(Type::Any),
                    Expr::Ident(name, _) => {
                        if let Some(sig) = self.fn_sigs.get(name).cloned() {
                            sig.ret
                        } else if let Some(Type::Function { ret, .. }) = self.lookup_var(name) {
                            (**ret).clone()
                        } else {
                            Type::Any
                        }
                    }
                    _ => Type::Any,
                };
                if matches!(v_ty, Type::Any) {
                    return Err(self.err_at(call_span,
                        "`.zip_with()`: el ret type del callback es `Any` (anotalo o usá tipos concretos)".to_string(),
                    ));
                }
                let cb_code = self.gen_binary_callback_inline_with_ret(
                    &args[1], t, &u_ty, &v_ty, "zip_with",
                )?;
                let t_rs = rust_type_for(t, self.env)?;
                let u_rs = rust_type_for(&u_ty, self.env)?;
                let v_rs = rust_type_for(&v_ty, self.env)?;
                let code = format!(
                    "{{ let __a: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __b: Vec<{u_rs}> = ({other_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       let mut __out: Vec<{v_rs}> = Vec::with_capacity(__a.len().min(__b.len())); \
                       for (__x, __y) in __a.into_iter().zip(__b.into_iter()) {{ \
                           __out.push(__cb(__x, __y)); \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new(v_ty))))
            }
            // Mini-tanda Mb5 — max_by/min_by(fn(T) -> Int) → Result<T>.
            // Extrae ranking Int por elemento, devuelve el item con
            // max/min ranking. Vacía → Err.
            (Type::List(t), "max_by") | (Type::List(t), "min_by") => {
                check_method_arity(method, args, 1)?;
                let is_max = method == "max_by";
                let (cb_code, _) = self.gen_callback_inline(&args[0], t, Some(&Type::Int), method)?;
                let t_rs = rust_type_for(t, self.env)?;
                let cmp_op = if is_max { "__rank > __bk" } else { "__rank < __bk" };
                let code = format!(
                    "{{ let __items: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       let mut __best: Option<(i64, {t_rs})> = None; \
                       for __it in __items.into_iter() {{ \
                           let __rank: i64 = __cb(__it.clone()); \
                           __best = match __best {{ \
                               None => Some((__rank, __it)), \
                               Some((__bk, __bv)) if {cmp_op} => Some((__rank, __it)), \
                               Some(__keep) => Some(__keep), \
                           }}; \
                       }} \
                       match __best {{ \
                           Some((_, __v)) => Ok::<{t_rs}, String>(__v), \
                           None => Err(String::from(\"lista vacía\")) \
                       }} }}"
                );
                Ok((
                    code,
                    Type::Result {
                        ok: Box::new((**t).clone()),
                        err: Box::new(Type::Str),
                    },
                ))
            }
            // Mini-tanda Mb6 — scan(init, fn(acc, x) -> Acc) -> List<Acc>.
            // Loop manual con snapshot que va acumulando outputs
            // intermedios. Reusa `gen_binary_callback_inline_with_ret`.
            (Type::List(t), "scan") => {
                check_method_arity(method, args, 2)?;
                let (init_code, init_ty) = self.gen_expr(&args[0])?;
                let acc_ty = init_ty.clone();
                let cb_code = self.gen_binary_callback_inline_with_ret(
                    &args[1], &acc_ty, t, &acc_ty, "scan",
                )?;
                let acc_rs = rust_type_for(&acc_ty, self.env)?;
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __items: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       let mut __acc: {acc_rs} = {init_code}; \
                       let mut __out: Vec<{acc_rs}> = Vec::with_capacity(__items.len()); \
                       for __it in __items.into_iter() {{ \
                           __acc = __cb(__acc, __it); \
                           __out.push(__acc.clone()); \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new(acc_ty))))
            }
            // Mini-tanda Mb6 — windows(n) -> List<List<T>>. Sliding
            // windows de tamaño n. Si len < n, lista vacía. n <= 0 →
            // panic.
            (Type::List(t), "windows") => {
                check_method_arity(method, args, 1)?;
                let (n_code, n_ty) = self.gen_expr(&args[0])?;
                let n_c = coerce(&n_code, &n_ty, &Type::Int);
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __n: i64 = {n_c}; \
                       if __n <= 0 {{ panic!(\"`.windows()` requiere n > 0, recibió {{}}\", __n); }} \
                       let __items: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __w = __n as usize; \
                       let mut __out: Vec<Arc<Mutex<Vec<{t_rs}>>>> = Vec::new(); \
                       if __items.len() >= __w {{ \
                           for __i in 0..=(__items.len() - __w) {{ \
                               let __win: Vec<{t_rs}> = __items[__i..__i + __w].to_vec(); \
                               __out.push(Arc::new(Mutex::new(__win))); \
                           }} \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((
                    code,
                    Type::List(Box::new(Type::List(Box::new((**t).clone())))),
                ))
            }
            // Mini-tanda Mb9 — split_at(i) → (List<T>, List<T>). Clamp
            // safe en ambos extremos.
            (Type::List(t), "split_at") => {
                check_method_arity(method, args, 1)?;
                let (idx_code, idx_ty) = self.gen_expr(&args[0])?;
                let idx_c = coerce(&idx_code, &idx_ty, &Type::Int);
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __snap: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __idx: i64 = {idx_c}; \
                       let __clamped = if __idx <= 0 {{ 0 }} \
                           else if (__idx as usize) >= __snap.len() {{ __snap.len() }} \
                           else {{ __idx as usize }}; \
                       let __left: Vec<{t_rs}> = __snap[..__clamped].to_vec(); \
                       let __right: Vec<{t_rs}> = __snap[__clamped..].to_vec(); \
                       (Arc::new(Mutex::new(__left)), Arc::new(Mutex::new(__right))) }}"
                );
                Ok((
                    code,
                    Type::Tuple(vec![
                        Type::List(Box::new((**t).clone())),
                        Type::List(Box::new((**t).clone())),
                    ]),
                ))
            }
            // Mini-tanda Mb8 — starts_with(prefix) / ends_with(suffix):
            // arg `List<T>`, devuelve `Bool`.
            (Type::List(t), "starts_with") | (Type::List(t), "ends_with") => {
                check_method_arity(method, args, 1)?;
                let is_start = method == "starts_with";
                let (other_code, other_ty) = self.gen_expr(&args[0])?;
                match &other_ty {
                    Type::List(_) => {}
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.{}()` espera una `List`, recibió `{}`",
                            method,
                            display_type(other, self.env)
                        )));
                    }
                }
                let t_rs = rust_type_for(t, self.env)?;
                let chain = if is_start {
                    "__self.iter().take(__other.len()).eq(__other.iter())"
                } else {
                    "__self.iter().rev().take(__other.len()).eq(__other.iter().rev())"
                };
                let code = format!(
                    "{{ let __self: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __other: Vec<{t_rs}> = ({other_code}).lock().unwrap().clone(); \
                       if __other.len() > __self.len() {{ false }} else {{ {chain} }} }}"
                );
                Ok((code, Type::Bool))
            }
            // Mini-tanda Mb8 — insert_at(i, v): functional, idx clamp
            // a [0, len]. Negativo → panic con mensaje claro.
            (Type::List(t), "insert_at") => {
                check_method_arity(method, args, 2)?;
                let (idx_code, idx_ty) = self.gen_expr(&args[0])?;
                let (v_code, v_ty) = self.gen_expr(&args[1])?;
                let idx_c = coerce(&idx_code, &idx_ty, &Type::Int);
                let v_c = coerce(&v_code, &v_ty, t);
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __idx: i64 = {idx_c}; \
                       if __idx < 0 {{ panic!(\"`.insert_at()` no acepta idx negativo: recibió {{}}\", __idx); }} \
                       let __snap: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __clamped = (__idx as usize).min(__snap.len()); \
                       let mut __out: Vec<{t_rs}> = Vec::with_capacity(__snap.len() + 1); \
                       __out.extend(__snap.iter().take(__clamped).cloned()); \
                       __out.push({v_c}); \
                       __out.extend(__snap.into_iter().skip(__clamped)); \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            // Mini-tanda Mb8 — remove_at(i): functional, idx fuera de
            // rango → panic.
            (Type::List(t), "remove_at") => {
                check_method_arity(method, args, 1)?;
                let (idx_code, idx_ty) = self.gen_expr(&args[0])?;
                let idx_c = coerce(&idx_code, &idx_ty, &Type::Int);
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __idx: i64 = {idx_c}; \
                       let __snap: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       if __idx < 0 || (__idx as usize) >= __snap.len() {{ \
                           panic!(\"`.remove_at()`: idx {{}} fuera de rango (len = {{}})\", __idx, __snap.len()); \
                       }} \
                       let __remove = __idx as usize; \
                       let __out: Vec<{t_rs}> = __snap.into_iter().enumerate() \
                           .filter(|(__i, _)| *__i != __remove) \
                           .map(|(_, __v)| __v) \
                           .collect(); \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            // Mini-tanda Mb8 — zip_to_map(values) -> Map<K, V>.
            (Type::List(t), "zip_to_map") => {
                check_method_arity(method, args, 1)?;
                let (other_code, other_ty) = self.gen_expr(&args[0])?;
                let v_ty = match &other_ty {
                    Type::List(inner) => (**inner).clone(),
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.zip_to_map()` espera `List<V>`, recibió `{}`",
                            display_type(other, self.env)
                        )));
                    }
                };
                let k_rs = rust_type_for(t, self.env)?;
                let v_rs = rust_type_for(&v_ty, self.env)?;
                let code = format!(
                    "{{ let __ks: Vec<{k_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __vs: Vec<{v_rs}> = ({other_code}).lock().unwrap().clone(); \
                       let mut __out: Vec<({k_rs}, {v_rs})> = Vec::with_capacity(__ks.len().min(__vs.len())); \
                       for (__k, __v) in __ks.into_iter().zip(__vs.into_iter()) {{ \
                           if let Some(__slot) = __out.iter_mut().find(|__p| __p.0 == __k) {{ \
                               __slot.1 = __v; \
                           }} else {{ \
                               __out.push((__k, __v)); \
                           }} \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::Map(Box::new((**t).clone()), Box::new(v_ty))))
            }
            // Mini-tanda Mb7 — take(n) / drop(n) / cycle(n) / init() /
            // tail() / intersperse(sep). Snapshot + slice/iter Rust.
            (Type::List(t), "take") => {
                check_method_arity(method, args, 1)?;
                let (n_code, n_ty) = self.gen_expr(&args[0])?;
                let n_c = coerce(&n_code, &n_ty, &Type::Int);
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __snap: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __n: i64 = {n_c}; \
                       let __take = if __n <= 0 {{ 0 }} else {{ (__n as usize).min(__snap.len()) }}; \
                       Arc::new(Mutex::new(__snap.into_iter().take(__take).collect::<Vec<{t_rs}>>())) }}"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            (Type::List(t), "drop") => {
                check_method_arity(method, args, 1)?;
                let (n_code, n_ty) = self.gen_expr(&args[0])?;
                let n_c = coerce(&n_code, &n_ty, &Type::Int);
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __snap: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __n: i64 = {n_c}; \
                       let __drop = if __n <= 0 {{ 0 }} else {{ (__n as usize).min(__snap.len()) }}; \
                       Arc::new(Mutex::new(__snap.into_iter().skip(__drop).collect::<Vec<{t_rs}>>())) }}"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            (Type::List(t), "init") => {
                check_method_arity(method, args, 0)?;
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __snap: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __out: Vec<{t_rs}> = if __snap.is_empty() {{ Vec::new() }} \
                           else {{ __snap[..__snap.len() - 1].to_vec() }}; \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            (Type::List(t), "tail") => {
                check_method_arity(method, args, 0)?;
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __snap: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __out: Vec<{t_rs}> = if __snap.is_empty() {{ Vec::new() }} \
                           else {{ __snap[1..].to_vec() }}; \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            (Type::List(t), "intersperse") => {
                check_method_arity(method, args, 1)?;
                let (sep_code, sep_ty) = self.gen_expr(&args[0])?;
                let sep_c = coerce(&sep_code, &sep_ty, t);
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __snap: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __sep: {t_rs} = {sep_c}; \
                       let mut __out: Vec<{t_rs}> = Vec::with_capacity(__snap.len() * 2); \
                       for (__i, __v) in __snap.into_iter().enumerate() {{ \
                           if __i > 0 {{ __out.push(__sep.clone()); }} \
                           __out.push(__v); \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            (Type::List(t), "cycle") => {
                check_method_arity(method, args, 1)?;
                let (n_code, n_ty) = self.gen_expr(&args[0])?;
                let n_c = coerce(&n_code, &n_ty, &Type::Int);
                let t_rs = rust_type_for(t, self.env)?;
                let code = format!(
                    "{{ let __snap: Vec<{t_rs}> = ({obj_code}).lock().unwrap().clone(); \
                       let __n: i64 = {n_c}; \
                       let __out: Vec<{t_rs}> = if __n <= 0 || __snap.is_empty() {{ Vec::new() }} \
                           else {{ \
                               let mut __r: Vec<{t_rs}> = Vec::with_capacity(__snap.len() * (__n as usize)); \
                               for _ in 0..__n {{ for __v in &__snap {{ __r.push(__v.clone()); }} }} \
                               __r \
                           }}; \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::List(Box::new((**t).clone()))))
            }
            (Type::List(_), other) => Err(self.err_at(call_span, format!(
                "List no tiene el método `{}` en el subset compilado (hoy: push/pop/len/map/filter/find/sort/sort_by/reverse/contains/enumerate/zip/chain/flatten/any/all/count/find_index/flat_map/first/last/min/max/sum/product/reduce/to_map/unique/partition/group_by/zip_with/max_by/min_by/scan/windows/take/drop/init/tail/intersperse/cycle/starts_with/ends_with/insert_at/remove_at/zip_to_map/split_at)",
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
            // Mini-tanda Ex — Map.filter(pred): callback `fn(K, V) -> Bool`.
            // Emit manual loop con snapshot — `Vec::filter` toma `FnMut(&T)`
            // que no encaja con nuestro closure.
            (Type::Map(k, v), "filter") => {
                check_method_arity(method, args, 1)?;
                let cb_code = self.gen_binary_callback_inline(&args[0], k, v, "filter")?;
                let k_rust = rust_type_for(k, self.env)?;
                let v_rust = rust_type_for(v, self.env)?;
                let code = format!(
                    "{{ let __pairs: Vec<({k_rs}, {v_rs})> = ({obj_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       let mut __out: Vec<({k_rs}, {v_rs})> = Vec::new(); \
                       for (__k, __v) in __pairs.into_iter() {{ \
                           if __cb(__k.clone(), __v.clone()) {{ __out.push((__k, __v)); }} \
                       }} \
                       Arc::new(Mutex::new(__out)) }}",
                    k_rs = k_rust, v_rs = v_rust,
                );
                Ok((code, Type::Map(k.clone(), v.clone())))
            }
            // Mini-tanda Ex — Map.map_values(fn): callback `fn(V) -> U`,
            // output `Map<K, U>`. Reusa `gen_callback_inline` (1-arg).
            (Type::Map(k, v), "map_values") => {
                check_method_arity(method, args, 1)?;
                let (cb_code, u_ty) = self.gen_callback_inline(&args[0], v, None, "map_values")?;
                let k_rust = rust_type_for(k, self.env)?;
                let u_rust = rust_type_for(&u_ty, self.env)?;
                let code = format!(
                    "{{ let __pairs = ({obj_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       Arc::new(Mutex::new(__pairs.into_iter().map(|(__k, __v)| ((__k as {k_rs}), __cb(__v) as {u_rs})).collect::<Vec<({k_rs}, {u_rs})>>())) }}",
                    k_rs = k_rust, u_rs = u_rust,
                );
                Ok((code, Type::Map(k.clone(), Box::new(u_ty))))
            }
            // Mini-tanda Up — update(k, fn(V) -> V) → Map<K, V> nuevo.
            // Snapshot del receiver, busca la key, aplica fn solo si
            // está presente; cualquier otra entry queda igual.
            (Type::Map(k, v), "update") => {
                check_method_arity(method, args, 2)?;
                let (key_code, key_ty) = self.gen_expr(&args[0])?;
                let coerced_key = coerce(&key_code, &key_ty, k);
                let (cb_code, _) = self.gen_callback_inline(&args[1], v, Some(v), "update")?;
                let k_rs = rust_type_for(k, self.env)?;
                let v_rs = rust_type_for(v, self.env)?;
                let code = format!(
                    "{{ let __key: {k_rs} = {key_code}; \
                       let __cb = {cb_code}; \
                       let __pairs: Vec<({k_rs}, {v_rs})> = ({obj_code}).lock().unwrap().clone(); \
                       let __out: Vec<({k_rs}, {v_rs})> = __pairs.into_iter().map(|(__k, __v)| {{ \
                           if __k == __key {{ (__k, __cb(__v)) }} else {{ (__k, __v) }} \
                       }}).collect(); \
                       Arc::new(Mutex::new(__out)) }}",
                    key_code = coerced_key,
                );
                Ok((code, Type::Map(k.clone(), v.clone())))
            }
            // Mini-tanda Mb2 — `keys_sorted()`: devuelve `List<K>` con
            // las keys ordenadas. Solo válido para K en {Int, Float,
            // Str, Bool} (mismas reglas que `list_sort`). Map vacío →
            // lista vacía. Para Float usamos `partial_cmp` con NaN
            // handling (paralelo al evaluator).
            (Type::Map(k, _), "keys_sorted") => {
                check_method_arity(method, args, 0)?;
                let k_rs = rust_type_for(k, self.env)?;
                let sort_line = match &**k {
                    Type::Int | Type::Str | Type::Bool => "__keys.sort();".to_string(),
                    Type::Float => "__keys.sort_by(|__a, __b| __a.partial_cmp(__b).unwrap_or(std::cmp::Ordering::Equal));".to_string(),
                    other => {
                        return Err(self.err_at(call_span, format!(
                            "`.keys_sorted()` solo soporta keys `Int`/`Float`/`Str`/`Bool`, recibió `Map<{}, _>`",
                            display_type(other, self.env)
                        )));
                    }
                };
                // Bindeamos `obj_code` a un local primero (paralelo a
                // first/last) — sin esto, `(m.clone()).lock()` produce
                // un temporario que rustc dropea al fin del stmt.
                let code = format!(
                    "{{ let __map = {obj_code}; \
                       let mut __keys: Vec<{k_rs}> = __map.lock().unwrap().iter().map(|(__k, _)| __k.clone()).collect(); \
                       {sort_line} \
                       Arc::new(Mutex::new(__keys)) }}"
                );
                Ok((code, Type::List(Box::new((**k).clone()))))
            }
            // Mini-tanda Mb6 — merge_with(other, fn(V, V) -> V) -> Map<K, V>.
            // Generaliza merge: callback decide qué value queda cuando
            // hay conflict. Útil para "sumar values" en duplicados.
            (Type::Map(k, v), "merge_with") => {
                check_method_arity(method, args, 2)?;
                let (other_code, other_ty) = self.gen_expr(&args[0])?;
                if !matches!(&other_ty, Type::Map(_, _) | Type::Any) {
                    return Err(self.err_at(call_span, format!(
                        "`.merge_with()` espera otro `Map`, recibió `{}`",
                        display_type(&other_ty, self.env)
                    )));
                }
                let cb_code = self.gen_binary_callback_inline_with_ret(
                    &args[1], v, v, v, "merge_with",
                )?;
                let k_rs = rust_type_for(k, self.env)?;
                let v_rs = rust_type_for(v, self.env)?;
                let code = format!(
                    "{{ let mut __out: Vec<({k_rs}, {v_rs})> = ({obj_code}).lock().unwrap().clone(); \
                       let __other = ({other_code}).lock().unwrap().clone(); \
                       let __cb = {cb_code}; \
                       for (__k, __v_other) in __other.into_iter() {{ \
                           if let Some(__idx) = __out.iter().position(|__p| __p.0 == __k) {{ \
                               let __v_self = __out[__idx].1.clone(); \
                               __out[__idx].1 = __cb(__v_self, __v_other); \
                           }} else {{ \
                               __out.push((__k, __v_other)); \
                           }} \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::Map(k.clone(), v.clone())))
            }
            // Mini-tanda Ex2 — merge: combina dos Maps last-write-wins.
            // Iteramos los pares de `other` y actualizamos/insertamos en
            // un clone del `obj`. Devuelve Map nuevo.
            (Type::Map(k, v), "merge") => {
                check_method_arity(method, args, 1)?;
                let (other_code, other_ty) = self.gen_expr(&args[0])?;
                if !matches!(&other_ty, Type::Map(_, _) | Type::Any) {
                    return Err(self.err_at(call_span, format!(
                        "`.merge()` espera otro `Map`, recibió `{}`",
                        display_type(&other_ty, self.env)
                    )));
                }
                let k_rs = rust_type_for(k, self.env)?;
                let v_rs = rust_type_for(v, self.env)?;
                let code = format!(
                    "{{ let mut __out: Vec<({k_rs}, {v_rs})> = ({obj_code}).lock().unwrap().clone(); \
                       let __other = ({other_code}).lock().unwrap().clone(); \
                       for (__k, __v) in __other.into_iter() {{ \
                           if let Some(__slot) = __out.iter_mut().find(|(__ek, _)| *__ek == __k) {{ \
                               __slot.1 = __v; \
                           }} else {{ \
                               __out.push((__k, __v)); \
                           }} \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::Map(k.clone(), v.clone())))
            }
            // Mini-tanda Mb4 — `invert()`: swap K ↔ V. Last-write-wins
            // si hay values duplicados. Devuelve `Map<V, K>`.
            (Type::Map(k, v), "invert") => {
                check_method_arity(method, args, 0)?;
                let k_rs = rust_type_for(k, self.env)?;
                let v_rs = rust_type_for(v, self.env)?;
                let code = format!(
                    "{{ let __snapshot: Vec<({k_rs}, {v_rs})> = ({obj_code}).lock().unwrap().clone(); \
                       let mut __out: Vec<({v_rs}, {k_rs})> = Vec::with_capacity(__snapshot.len()); \
                       for (__k, __v) in __snapshot.into_iter() {{ \
                           if let Some(__slot) = __out.iter_mut().find(|(__ek, _)| *__ek == __v) {{ \
                               __slot.1 = __k; \
                           }} else {{ \
                               __out.push((__v, __k)); \
                           }} \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::Map(v.clone(), k.clone())))
            }
            // Mini-tanda Mb3 — `entries()`: devuelve `List<(K, V)>`.
            // Inversa de `xs.to_map()`. Emite snapshot del Vec<(K, V)>
            // del Map adentro de un Arc<Mutex<>>.
            (Type::Map(k, v), "entries") => {
                check_method_arity(method, args, 0)?;
                let k_rs = rust_type_for(k, self.env)?;
                let v_rs = rust_type_for(v, self.env)?;
                let code = format!(
                    "Arc::new(Mutex::new(({obj_code}).lock().unwrap().iter().map(|(__k, __v)| (__k.clone(), __v.clone())).collect::<Vec<({k_rs}, {v_rs})>>()))"
                );
                Ok((code, Type::List(Box::new(Type::Tuple(vec![(**k).clone(), (**v).clone()])))))
            }
            // Mini-tanda Mb9 — has_value(v) -> Bool: chequea si v está
            // como value en algún par del Map.
            (Type::Map(_, v), "has_value") => {
                check_method_arity(method, args, 1)?;
                let (val_code, val_ty) = self.gen_expr(&args[0])?;
                let val_c = coerce(&val_code, &val_ty, v);
                let v_rs = rust_type_for(v, self.env)?;
                let code = format!(
                    "{{ let __target: {v_rs} = {val_c}; \
                       ({obj_code}).lock().unwrap().iter().any(|__p| __p.1 == __target) }}"
                );
                Ok((code, Type::Bool))
            }
            // Mini-tanda Mb7 — with(k, v) -> Map<K, V>: functional update.
            (Type::Map(k, v), "with") => {
                check_method_arity(method, args, 2)?;
                let (key_code, key_ty) = self.gen_expr(&args[0])?;
                let (val_code, val_ty) = self.gen_expr(&args[1])?;
                let key_c = coerce(&key_code, &key_ty, k);
                let val_c = coerce(&val_code, &val_ty, v);
                let k_rs = rust_type_for(k, self.env)?;
                let v_rs = rust_type_for(v, self.env)?;
                let code = format!(
                    "{{ let mut __out: Vec<({k_rs}, {v_rs})> = ({obj_code}).lock().unwrap().clone(); \
                       let __k: {k_rs} = {key_c}; \
                       let __v: {v_rs} = {val_c}; \
                       if let Some(__slot) = __out.iter_mut().find(|__p| __p.0 == __k) {{ \
                           __slot.1 = __v; \
                       }} else {{ \
                           __out.push((__k, __v)); \
                       }} \
                       Arc::new(Mutex::new(__out)) }}"
                );
                Ok((code, Type::Map(k.clone(), v.clone())))
            }
            (Type::Map(_, _), other) => Err(self.err_at(call_span, format!(
                "Map no tiene el método `{}` en el subset compilado (hoy: has/keys/values/len/get/filter/map_values/merge/update/keys_sorted/entries/invert/merge_with/with/has_value)",
                other
            ))),

            // ---- Mini-tanda Mb9 — métodos sobre primitivos Int/Float ----
            (Type::Int, "abs") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).wrapping_abs()", obj_code), Type::Int))
            }
            (Type::Int, "to_str") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).to_string()", obj_code), Type::Str))
            }
            (Type::Int, "to_str_base") => {
                check_method_arity(method, args, 1)?;
                let (base_code, base_ty) = self.gen_expr(&args[0])?;
                let base_c = coerce(&base_code, &base_ty, &Type::Int);
                let code = format!(
                    "{{ let __n: i64 = {obj_code}; let __base: i64 = {base_c}; \
                       match __base {{ \
                           2 => format!(\"{{:b}}\", __n), \
                           8 => format!(\"{{:o}}\", __n), \
                           10 => __n.to_string(), \
                           16 => format!(\"{{:x}}\", __n), \
                           _ => panic!(\"`Int.to_str_base()` solo soporta bases 2, 8, 10 o 16; recibió {{}}\", __base), \
                       }} }}"
                );
                Ok((code, Type::Str))
            }
            (Type::Float, "abs") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).abs()", obj_code), Type::Float))
            }
            (Type::Float, "to_str") => {
                check_method_arity(method, args, 0)?;
                // Mismo formato que `__fitz_fmt_float` del preludio.
                Ok((format!("__fitz_fmt_float({})", obj_code), Type::Str))
            }
            (Type::Float, "is_nan") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).is_nan()", obj_code), Type::Bool))
            }
            (Type::Float, "is_finite") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).is_finite()", obj_code), Type::Bool))
            }
            // ---- Tipos custom (R.3 mini-fase R) ----
            (Type::Nominal(id), m) => {
                let type_name = self.env.info(*id).name.clone();
                let method_def = self
                    .type_methods
                    .get(&type_name)
                    .and_then(|ms| ms.iter().find(|md| md.name == m))
                    .cloned();
                let method_def = match method_def {
                    Some(md) => md,
                    None => {
                        return Err(self.err_at(call_span, format!(
                            "el tipo `{}` no tiene un método llamado `{}`",
                            type_name, m
                        )));
                    }
                };
                self.gen_custom_method_call(&obj_code, &type_name, &method_def, args, call_span)
            }

            // ---- Otros ----
            (other, m) => Err(self.err_at(call_span, format!(
                "method call `.{}` sobre `{}`: no soportado en codegen",
                m,
                display_type(other, self.env)
            ))),
        }
    }

    // --- R.3: emitir método custom como `impl FooData { fn ... }` --------
    //
    // El método se emite como `pub [async] fn <name>(&self, p1: T1, ...) -> R { ... }`.
    // El body se prepara pre-bindeando los fields del tipo como locales
    // (`let <field> = self.<field>.clone();`) — "opción A". Esto hace
    // que las referencias `name` (sin prefijo `self.`) adentro del body
    // resuelvan al binding local, replicando la semántica del evaluator.
    //
    // Caveats:
    //  - El método debe tener `return_type` declarado (deuda 5b.1 —
    //    inferencia de tipos no soportada). Si falta → error claro.
    //  - Params sin anotación → error claro (mismo motivo).
    //  - async methods (`async fn ...` adentro de `type`) — habilitados
    //    post-R.3. Emite `pub async fn`; el caller hace el `.await`
    //    explícito como con cualquier async fn Fitz. El call site usa
    //    el patrón "clone-out" para no holdear el MutexGuard a través
    //    del await (ver `gen_custom_method_call`).
    fn emit_custom_method(
        &mut self,
        type_name: &str,
        sig: &TypeSig,
        method: &crate::ast::MethodDef,
    ) -> Result<(), FitzError> {
        // Resolver firmas de params + return type. Exigimos anotación
        // (igual que fns top-level — 5b.1).
        let mut rust_params: Vec<String> = Vec::with_capacity(method.params.len());
        let mut param_types: Vec<Type> = Vec::with_capacity(method.params.len());
        for p in &method.params {
            let pty_expr = p.type_.as_ref().ok_or_else(|| {
                self.err(format!(
                    "método `{}.{}`: el parámetro `{}` necesita una anotación de tipo para el codegen (5b.1)",
                    type_name, method.name, p.name
                ))
            })?;
            let pty = crate::types::resolve_type_expr(pty_expr, self.env)
                .map_err(|e| self.err(format!(
                    "método `{}.{}`: tipo del parámetro `{}` no resoluble: {:?}",
                    type_name, method.name, p.name, e
                )))?;
            rust_params.push(format!("{}: {}", p.name, rust_type_for(&pty, self.env)?));
            param_types.push(pty);
        }
        let ret_ty_expr = method.return_type.as_ref().ok_or_else(|| {
            self.err(format!(
                "método `{}.{}`: falta anotación de tipo de retorno para el codegen (5b.1)",
                type_name, method.name
            ))
        })?;
        let ret_ty = crate::types::resolve_type_expr(ret_ty_expr, self.env)
            .map_err(|e| self.err(format!(
                "método `{}.{}`: tipo de retorno no resoluble: {:?}",
                type_name, method.name, e
            )))?;
        // Para async methods, el ret type declarado por el usuario es
        // `T`, pero el `pub async fn` Rust auto-envuelve en
        // `impl Future<Output = T>`. El interior del body sigue
        // produciendo `T` puro (no envuelto).
        let ret_rust = rust_type_for(&ret_ty, self.env)?;

        // Cabecera Rust del método.
        // R.3-async: los métodos async toman `self` por valor (no
        // `&self`). El Future devuelto captura `self` adentro y vive
        // a través de `.await` sin lifetime issues. Sync sigue
        // tomando `&self` (más barato, no clona).
        //
        // Mini-tanda St: los métodos estáticos NO toman receiver — se
        // emiten como `pub fn <name>(params...) -> R` (associated fn).
        let async_kw = if method.is_async { "async " } else { "" };
        let header = if method.is_static {
            // Sin self: associated function. Si no hay params, omitimos
            // la coma intermedia para no producir `fn foo(, )`.
            if rust_params.is_empty() {
                format!(
                    "    pub {async_kw}fn {name}() -> {ret} {{",
                    async_kw = async_kw,
                    name = method.name,
                    ret = ret_rust,
                )
            } else {
                format!(
                    "    pub {async_kw}fn {name}({params}) -> {ret} {{",
                    async_kw = async_kw,
                    name = method.name,
                    params = rust_params.join(", "),
                    ret = ret_rust,
                )
            }
        } else {
            let self_kw = if method.is_async { "self" } else { "&self" };
            if rust_params.is_empty() {
                format!(
                    "    pub {async_kw}fn {name}({self_kw}) -> {ret} {{",
                    async_kw = async_kw,
                    self_kw = self_kw,
                    name = method.name,
                    ret = ret_rust,
                )
            } else {
                format!(
                    "    pub {async_kw}fn {name}({self_kw}, {params}) -> {ret} {{",
                    async_kw = async_kw,
                    self_kw = self_kw,
                    name = method.name,
                    params = rust_params.join(", "),
                    ret = ret_rust,
                )
            }
        };
        writeln!(&mut self.output, "{}", header).unwrap();

        // Pre-bindear cada field como local. Usamos `clone()` para
        // sacar el valor del struct y dejarlo en una var mutable
        // (el body puede reasignar la var local sin afectar al
        // receiver — semántica de "opción A": local gana).
        //
        // R.3 shadowing: si un param tiene el mismo nombre que un
        // field, el param gana. Skipeamos esos fields para que el
        // `let <field>` no shadowee al param (que ya está en el
        // scope de la fn).
        //
        // Mini-tanda St: los métodos estáticos NO tienen receiver, así
        // que NO pre-bindean fields (no hay `self.field` que clonar).
        let param_names: std::collections::HashSet<&str> =
            method.params.iter().map(|p| p.name.as_str()).collect();
        self.push_scope();
        if !method.is_static {
        for f in &sig.fields {
            if param_names.contains(f.name.as_str()) {
                continue;
            }
            let rty = rust_type_for(&f.type_, self.env)?;
            writeln!(
                &mut self.output,
                "        let mut {name}: {rty} = self.{name}.clone();",
                name = f.name,
                rty = rty
            )
            .unwrap();
            // Suppress unused-var warnings cuando el método no usa el field.
            writeln!(&mut self.output, "        let _ = &{};", f.name).unwrap();
            self.declare_var(f.name.clone(), f.type_.clone());
        }
        } // cierre de `if !method.is_static`
        // Registrar params en el scope del codegen (sobreescriben
        // homónimos por el `declare_var`).
        for (p, pty) in method.params.iter().zip(param_types.iter()) {
            self.declare_var(p.name.clone(), pty.clone());
        }

        // Push del return type al stack para `Stmt::Return` chequeo.
        self.ret_stack.push(ret_ty.clone());

        // Generar el body. Reusamos `gen_block_to_string` que walkea los
        // stmts y emite el código. Indent +1 (estamos adentro de
        // `impl { fn { ... } }`).
        let stmt_refs: Vec<&Stmt> = method.body.iter().collect();
        let body_code = self.gen_block_to_string(&stmt_refs)?;
        self.emit(&body_code);

        self.ret_stack.pop();
        self.pop_scope();
        self.emit("    }\n");
        Ok(())
    }

    // --- R.3: método custom sobre nominal ---------------------------------
    //
    // El método se emite como `pub [async] fn <name>(&self, p1: T1, ...) -> R`
    // dentro de un `impl <Type>Data` (ver `gen_type_def`). El call
    // sobre `Arc<Mutex<Data>>` se traduce de dos formas según el
    // método:
    //
    //  - **Sync**: `{ let __recv = obj.clone(); let __g = __recv.lock().unwrap();
    //    __g.<method>(args) }`. El lock dura solo lo que dura el call;
    //    el `&Data` no escapa el bloque.
    //
    //  - **Async**: el lock NO puede cruzar el `.await` porque
    //    `MutexGuard<std::sync::Mutex<_>>` no es `Send`. Patrón
    //    "clone-out": clonamos el `Data` adentro del lock y soltamos
    //    el guard antes del call:
    //    `{ let __recv = obj.clone(); let __data: Data = { let __g =
    //    __recv.lock().unwrap(); __g.clone() }; __data.<method>(args) }`.
    //    El Future devuelto captura `__data` por valor; las colecciones
    //    Arc<Mutex<...>> adentro del Data siguen siendo refs compartidas,
    //    así que mutaciones a listas/maps siguen visibles desde el
    //    receiver original (semántica idéntica al evaluator).
    //    El `.await` lo emite el caller (Expr::Await en el AST).
    /// Mini-tanda St — emite `<Type>Data::<method>(args)` para una
    /// invocación de método estático. Sin receiver (sin `&self`), sin
    /// lock — paralelo a una fn top-level pero scope-namespaced.
    fn gen_static_method_call(
        &mut self,
        type_name: &str,
        method_def: &crate::ast::MethodDef,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        // Fp — aridad con defaults para métodos estáticos.
        let required = method_def.params.iter().filter(|p| p.default.is_none()).count();
        if args.len() < required || args.len() > method_def.params.len() {
            return Err(self.err_at(call_span, if required == method_def.params.len() {
                format!(
                    "el método estático `{}.{}` espera {} argumento(s), recibió {}",
                    type_name, method_def.name, method_def.params.len(), args.len(),
                )
            } else {
                format!(
                    "el método estático `{}.{}` espera entre {} y {} argumento(s), recibió {}",
                    type_name, method_def.name, required, method_def.params.len(), args.len(),
                )
            }));
        }

        let mut arg_codes: Vec<String> = Vec::with_capacity(method_def.params.len());
        for (i, arg) in args.iter().enumerate() {
            let (a_code, a_ty) = self.gen_expr(arg)?;
            let target_ty = method_def.params[i]
                .type_
                .as_ref()
                .and_then(|t| crate::types::resolve_type_expr(t, self.env).ok())
                .unwrap_or(Type::Any);
            arg_codes.push(coerce(&a_code, &a_ty, &target_ty));
        }
        // Fp — fill con defaults para los faltantes.
        for i in args.len()..method_def.params.len() {
            let default_expr = method_def.params[i].default.as_ref().expect("ya cubierto");
            let (d_code, d_ty) = self.gen_expr(default_expr)?;
            let target_ty = method_def.params[i]
                .type_
                .as_ref()
                .and_then(|t| crate::types::resolve_type_expr(t, self.env).ok())
                .unwrap_or(Type::Any);
            arg_codes.push(coerce(&d_code, &d_ty, &target_ty));
        }

        let ret_ty = method_def
            .return_type
            .as_ref()
            .and_then(|t| crate::types::resolve_type_expr(t, self.env).ok())
            .unwrap_or(Type::Null);
        let value_ty = if method_def.is_async {
            Type::Future(Box::new(ret_ty.clone()))
        } else {
            ret_ty.clone()
        };

        let _ = call_span;
        let code = format!(
            "{type_name}Data::{name}({args})",
            type_name = type_name,
            name = method_def.name,
            args = arg_codes.join(", "),
        );
        Ok((code, value_ty))
    }

    fn gen_custom_method_call(
        &mut self,
        obj_code: &str,
        type_name: &str,
        method_def: &crate::ast::MethodDef,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        let _ = type_name; // disponible para mensajes de error futuros
        // Fp — aridad con defaults para custom methods.
        let required = method_def.params.iter().filter(|p| p.default.is_none()).count();
        if args.len() < required || args.len() > method_def.params.len() {
            return Err(self.err_at(call_span, if required == method_def.params.len() {
                format!(
                    "el método `.{}()` espera {} argumento(s), recibió {}",
                    method_def.name, method_def.params.len(), args.len(),
                )
            } else {
                format!(
                    "el método `.{}()` espera entre {} y {} argumento(s), recibió {}",
                    method_def.name, required, method_def.params.len(), args.len(),
                )
            }));
        }

        // Resolver tipos de params y return (con anotaciones del MethodDef).
        let mut arg_codes: Vec<String> = Vec::with_capacity(method_def.params.len());
        for (i, arg) in args.iter().enumerate() {
            let (a_code, a_ty) = self.gen_expr(arg)?;
            let target_ty = method_def.params[i]
                .type_
                .as_ref()
                .and_then(|t| crate::types::resolve_type_expr(t, self.env).ok())
                .unwrap_or(Type::Any);
            arg_codes.push(coerce(&a_code, &a_ty, &target_ty));
        }
        // Fp — fill con defaults para los params faltantes.
        for i in args.len()..method_def.params.len() {
            let default_expr = method_def.params[i].default.as_ref().expect("ya cubierto");
            let (d_code, d_ty) = self.gen_expr(default_expr)?;
            let target_ty = method_def.params[i]
                .type_
                .as_ref()
                .and_then(|t| crate::types::resolve_type_expr(t, self.env).ok())
                .unwrap_or(Type::Any);
            arg_codes.push(coerce(&d_code, &d_ty, &target_ty));
        }

        let ret_ty = method_def
            .return_type
            .as_ref()
            .and_then(|t| crate::types::resolve_type_expr(t, self.env).ok())
            .unwrap_or(Type::Null);
        // Si el método es async, el "tipo de retorno" desde el punto
        // de vista del checker Fitz es `Future<T>`. El caller hace
        // `.await` para desempacar a T.
        let value_ty = if method_def.is_async {
            Type::Future(Box::new(ret_ty.clone()))
        } else {
            ret_ty.clone()
        };

        let _ = call_span;
        let code = if method_def.is_async {
            // Patrón clone-out: lock corto adentro del bloque inner,
            // call al método sobre la copia (sin lock vivo).
            let data_type = format!("{}Data", type_name);
            format!(
                "{{ let __recv = ({obj}).clone(); \
                 let __data: {data} = {{ let __g = __recv.lock().unwrap(); __g.clone() }}; \
                 __data.{name}({args}) }}",
                obj = obj_code,
                data = data_type,
                name = method_def.name,
                args = arg_codes.join(", "),
            )
        } else {
            format!(
                "{{ let __recv = ({obj}).clone(); let __g = __recv.lock().unwrap(); __g.{name}({args}) }}",
                obj = obj_code,
                name = method_def.name,
                args = arg_codes.join(", "),
            )
        };
        Ok((code, value_ty))
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
        Ok((code, Type::Result { ok: Box::new(elem_ty.clone()), err: Box::new(Type::Str) }))
    }

    /// S.3 — `xs.sort()` IN-PLACE. Soporta `List<T>` para T en
    /// {Int, Float, Str, Bool}. Para Float usamos `partial_cmp`
    /// con fallback `Equal` (NaN-tolerant). Tipos no soportados →
    /// error claro de codegen. List<Any> → error (no podemos
    /// generar el comparator estático; el intérprete lo chequea
    /// en runtime, pero el codegen necesita un tipo concreto).
    fn gen_list_sort(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
        call_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("sort", args, 0)?;
        let cmp = match elem_ty {
            Type::Int | Type::Str | Type::Bool => "a.cmp(b)".to_string(),
            Type::Float => "a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)".to_string(),
            other => {
                return Err(self.err_at(call_span, format!(
                    "`.sort()` no soporta `List<{}>` en `fitz build` (hoy: Int/Float/Str/Bool)",
                    display_type(other, self.env),
                )));
            }
        };
        let code = format!(
            "({}).lock().unwrap().sort_by(|a, b| {})",
            obj_code, cmp
        );
        Ok((code, Type::Null))
    }

    /// S.3 — `xs.contains(v)` lineal sobre el `Vec`. Usa la
    /// `PartialEq` derivada del tipo del elemento (que para
    /// nominales/listas/maps es la custom impl emitida por el
    /// codegen; para primitivos es la stdlib).
    fn gen_list_contains(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("contains", args, 1)?;
        let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
        let coerced = coerce(&arg_code, &arg_ty, elem_ty);
        let elem_rs = rust_type_for(elem_ty, self.env)?;
        let code = format!(
            "{{ let __needle: {} = {}; ({}).lock().unwrap().iter().any(|__v| __v == &__needle) }}",
            elem_rs, coerced, obj_code
        );
        Ok((code, Type::Bool))
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
        Ok((code, Type::Result { ok: Box::new(val_ty.clone()), err: Box::new(Type::Str) }))
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
        // Mini-tanda Cd — higher-order: si el arg es `Expr::Ident(name)`
        // y refiere a una fn top-level (`fn_sigs`) o local con tipo
        // `Function`, lo emitimos como referencia directa al ident Rust.
        // Habilita `xs.map(double)` cuando `double` es `fn double(n: Int)`.
        if let Expr::Ident(name, _) = arg {
            if let Some((sig_params, sig_ret)) = self.resolve_named_callback(name) {
                if sig_params.len() != 1 {
                    return Err(self.err_at(arg_span, format!(
                        "el callback de `.{}` toma 1 parámetro, la fn `{}` declara {}",
                        method, name, sig_params.len(),
                    )));
                }
                if !is_compatible(param_ty, &sig_params[0]) {
                    return Err(self.err_at(arg_span, format!(
                        "el callback `{}` espera `{}`, pero el elemento es `{}`",
                        name,
                        display_type(&sig_params[0], self.env),
                        display_type(param_ty, self.env),
                    )));
                }
                if let Some(want) = expected_ret_ty {
                    if !is_compatible(&sig_ret, want) {
                        return Err(self.err_at(arg_span, format!(
                            "el callback `{}` debe retornar `{}`, retorna `{}`",
                            name,
                            display_type(want, self.env),
                            display_type(&sig_ret, self.env),
                        )));
                    }
                }
                return Ok((name.clone(), sig_ret));
            }
        }
        let (params, body) = match arg {
            Expr::FnExpr { params, body, .. } => (params, body),
            _ => {
                return Err(self.err_at(arg_span, format!(
                    "`.{}(...)` exige un callback inline `fn(x) => ...` o `fn(x) {{ ... }}` \
                     o el nombre de una fn top-level (`fn(...) -> ...`).",
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

    /// Mini-tanda Mb — variante de `gen_callback_inline` para
    /// callbacks de 2 parámetros (caso canónico: `sort_by(cmp)`).
    /// Devuelve el código Rust del closure binario con tipo
    /// inferido para el ret. Espera que ambos params tipen como T
    /// (el `sort_by` de `List<T>` los pasa con el mismo tipo).
    fn gen_binary_callback_inline(
        &mut self,
        arg: &Expr,
        param0_ty: &Type,
        param1_ty: &Type,
        method: &str,
    ) -> Result<String, FitzError> {
        // Mini-tanda Mb3: caller pasa `expected_ret_ty` via
        // `gen_binary_callback_inline_with_ret`; para callers
        // existentes el fallback por nombre de método sigue siendo
        // válido.
        let expected_ret = match method {
            "sort_by" => Type::Int,
            "filter" => Type::Bool, // Map.filter
            _ => Type::Any,
        };
        self.gen_binary_callback_inline_with_ret(
            arg, param0_ty, param1_ty, &expected_ret, method,
        )
    }

    /// Mini-tanda Mb3 — versión explícita: el caller pasa el
    /// `expected_ret_ty` que el callback debe satisfacer. Útil para
    /// `reduce` donde Acc puede ser cualquier tipo declarado por el
    /// usuario (no se infiere por nombre de método).
    fn gen_binary_callback_inline_with_ret(
        &mut self,
        arg: &Expr,
        param0_ty: &Type,
        param1_ty: &Type,
        expected_ret_ty: &Type,
        method: &str,
    ) -> Result<String, FitzError> {
        let expected_ret = expected_ret_ty.clone();
        let arg_span = arg.span();
        // Mini-tanda Cd — higher-order: aceptamos también fn nombrada
        // como callback binario (`xs.reduce(0, sumar)`, `xs.sort_by(cmp)`).
        if let Expr::Ident(name, _) = arg {
            if let Some((sig_params, sig_ret)) = self.resolve_named_callback(name) {
                if sig_params.len() != 2 {
                    return Err(self.err_at(arg_span, format!(
                        "el callback de `.{}` toma 2 parámetros, la fn `{}` declara {}",
                        method, name, sig_params.len(),
                    )));
                }
                if !is_compatible(param0_ty, &sig_params[0]) {
                    return Err(self.err_at(arg_span, format!(
                        "el callback `{}` espera `{}` en el param[0], recibe `{}`",
                        name,
                        display_type(&sig_params[0], self.env),
                        display_type(param0_ty, self.env),
                    )));
                }
                if !is_compatible(param1_ty, &sig_params[1]) {
                    return Err(self.err_at(arg_span, format!(
                        "el callback `{}` espera `{}` en el param[1], recibe `{}`",
                        name,
                        display_type(&sig_params[1], self.env),
                        display_type(param1_ty, self.env),
                    )));
                }
                if !is_compatible(&sig_ret, &expected_ret) {
                    return Err(self.err_at(arg_span, format!(
                        "el callback `{}` debe retornar `{}`, retorna `{}`",
                        name,
                        display_type(&expected_ret, self.env),
                        display_type(&sig_ret, self.env),
                    )));
                }
                return Ok(name.clone());
            }
        }
        let (params, body) = match arg {
            Expr::FnExpr { params, body, .. } => (params, body),
            _ => {
                return Err(self.err_at(arg_span, format!(
                    "`.{}(...)` exige un callback inline `fn(a, b) => ...` o `fn(a, b) {{ ... }}` \
                     o el nombre de una fn top-level (`fn(...) -> ...`).",
                    method
                )));
            }
        };
        if params.len() != 2 {
            return Err(self.err_at(arg_span, format!(
                "el callback de `.{}` toma 2 parámetros, recibió {}",
                method,
                params.len()
            )));
        }
        let p0_name = params[0].name.clone();
        let p1_name = params[1].name.clone();

        let ret_ty = expected_ret;
        let p0_rs = rust_type_for(param0_ty, self.env)?;
        let p1_rs = rust_type_for(param1_ty, self.env)?;
        let ret_rs = rust_type_for(&ret_ty, self.env)?;

        self.ret_stack.push(ret_ty.clone());
        let saved_response_mode = self.response_mode;
        self.response_mode = false;
        self.push_scope();
        self.declare_var(p0_name.clone(), param0_ty.clone());
        self.declare_var(p1_name.clone(), param1_ty.clone());
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

        Ok(format!(
            "|{}: {}, {}: {}| -> {} {{ {} }}",
            p0_name, p0_rs, p1_name, p1_rs, ret_rs, body_str
        ))
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
    /// Mini-tanda Async-cl build — emite un FnExpr como valor.
    /// Cuando `is_async = true`, el closure devuelve un `Pin<Box<dyn
    /// Future<Output=T> + Send>>` (boxing requerido porque `async move`
    /// produce un opaque type que no se puede nombrar). El body se
    /// envuelve en `Box::pin(async move { ... })`. Para el caso sync
    /// (`is_async = false`), comportamiento idéntico al de pre-Async-cl.
    fn gen_fn_expr_as_value(
        &mut self,
        params: &[crate::ast::Param],
        body: &[Stmt],
        is_async: bool,
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
        // Mini-tanda Async-cl build: para async closures, el closure
        // devuelve `Pin<Box<dyn Future<Output=R> + Send>>` (boxing
        // requerido porque `async move` produce un opaque type sin
        // nombre). El body se envuelve en `Box::pin(async move { ... })`.
        // El Type Fitz también se envuelve: `Function { ret: Future<R> }`.
        let (closure_ret_ty_rs, closure_body, fitz_ret_ty) = if is_async {
            let pinned = format!(
                "std::pin::Pin<Box<dyn std::future::Future<Output = {ret_ty_rs}> + Send>>",
            );
            // Rust requiere que el body del closure esté entre `{}`
            // cuando hay return type explícito. Por eso envolvemos:
            // `|...| -> Pin<...> { Box::pin(async move { ... }) }`.
            let body_wrapped = format!(
                "{{ Box::pin(async move {{ {body_str} }}) }}"
            );
            (pinned, body_wrapped, Type::Future(Box::new(ret_ty.clone())))
        } else {
            (
                ret_ty_rs.clone(),
                format!("{{ {body_str} }}"),
                ret_ty.clone(),
            )
        };
        let cast_target = {
            let ps: Vec<String> = param_types
                .iter()
                .map(|p| rust_type_for(p, self.env))
                .collect::<Result<_, _>>()?;
            format!("Arc<dyn Fn({}) -> {} + Send + Sync>", ps.join(", "), closure_ret_ty_rs)
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
            "|{params_sig}| -> {closure_ret_ty_rs} {closure_body}",
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
                ret: Box::new(fitz_ret_ty),
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

    /// Mini-tanda Mb5 — variante binaria de `infer_callback_ret_silently`.
    /// Usa los nombres reales del FnExpr para declarar los params en
    /// el scope antes del dry-run del primer `Stmt::Return` (o último
    /// `Stmt::Expr` no-print) del body. Necesario para `zip_with`
    /// donde V se determina del callback y no por nombre del método.
    fn infer_callback_ret_silently_binary_named(
        &mut self,
        params: &[crate::ast::Param],
        body: &[Stmt],
        p0_ty: &Type,
        p1_ty: &Type,
    ) -> Option<Type> {
        if params.len() != 2 {
            return None;
        }
        let target: Option<&Expr> = body
            .iter()
            .find_map(|s| if let Stmt::Return(e, _) = s { Some(e) } else { None })
            .or_else(|| {
                body.last().and_then(|s| match s {
                    Stmt::Expr(e, _) if !is_print_call(e) => Some(e),
                    _ => None,
                })
            });
        let e = target?;
        self.push_scope();
        self.declare_var(params[0].name.clone(), p0_ty.clone());
        self.declare_var(params[1].name.clone(), p1_ty.clone());
        let (_discarded, result) = self.with_temp_output(|ctx| ctx.gen_expr(e));
        self.pop_scope();
        result.ok().map(|(_, t)| t)
    }

    // --- Result, `?`, match (5b.4) ----------------------------------------

    /// `Ok(e)` → `Ok(<coerced e>)`. El tipo de Fitz es `Result<T>` donde
    /// T es el tipo sintetizado del inner. El Err side queda como `String`
    /// (pinned, ver `rust_type_for`), pero acá no lo materializamos —
    /// rustc lo infiere desde el contexto destino (anotación / return
    /// type / brazo del match opuesto).
    fn gen_ok(&mut self, inner: &Expr) -> Result<(String, Type), FitzError> {
        let (code, ty) = self.gen_expr(inner)?;
        Ok((format!("Ok({})", code), Type::Result { ok: Box::new(ty), err: Box::new(Type::Str) }))
    }

    /// `Err(e)` → `Err(<e como String>)`. El Err side está pinned a String
    /// en el código generado (decisión 5b.4): si el inner ya es Str, se
    /// usa directo; si no, se coerce con `format!("{}", x)` para preservar
    /// la práctica de "Err con mensaje" del intérprete y de los ejemplos.
    /// El tipo Fitz sintetizado es `Result<Any>` — no conocemos el T del
    /// Ok side, el contexto destino lo refinará.
    ///
    /// Mini-tanda Re+ — `Err(value)` ahora tipa como `Result<Any, E>`
    /// donde E es el tipo del value. El codegen NO coerce el value a
    /// String — emite `Err(<code>)` directo. El tipo Rust final del
    /// `Result<T, E>` se resuelve cuando el contexto destino lo
    /// determina (anotación del let, return de fn, etc.).
    ///
    /// Mini-tanda El — `Err(List<T>)` / `Err(Map<K,V>)` ahora también
    /// se soportan. El codegen emite `Err(<code>)` directo con el E
    /// inferido como `List<T>` o `Map<K,V>`; el binding `Err(e)` en
    /// match arms recupera el tipo concreto y permite usar métodos
    /// `.len()`, `.get(k)`, etc. sobre el value. El `print` de un
    /// `Result<T, List<U>>` ya pasaba por `show_expr` recursivo, que
    /// maneja Result/List/Map nativamente.
    fn gen_err(&mut self, inner: &Expr) -> Result<(String, Type), FitzError> {
        let (code, ty) = self.gen_expr(inner)?;
        match &ty {
            // Primitivos + nominal + List + Map — emit `Err(<code>)`
            // directo, sin coerción a String. El tipo del Result
            // resultante lleva el E real para que el binding `Err(e)`
            // pueda tipar como ese E.
            Type::Str | Type::Int | Type::Float | Type::Bool | Type::Null
            | Type::Nominal(_)
            | Type::List(_) | Type::Map(_, _) => {
                Ok((
                    format!("Err({})", code),
                    Type::Result { ok: Box::new(Type::Any), err: Box::new(ty) },
                ))
            }
            other => {
                Err(self.err_at(inner.span(), format!(
                    "`Err({})` no soportado en `fitz build` — el tipo del Err debe ser primitivo, nominal, List o Map. Usá `fitz run` para preservar el value, o convertí explícitamente con interpolación: `Err(\"...{{x}}...\")`",
                    display_type(other, self.env)
                )))
            }
        }
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
            Type::Result { ok: t, err: _ } => (**t).clone(),
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
            Type::Result { .. } => {}
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
            Type::Result { ok: t, err: _ } => Some((**t).clone()),
            _ => None,
        };

        let mut arm_pieces: Vec<String> = Vec::with_capacity(arms.len() + 1);
        let mut arm_tys: Vec<Type> = Vec::with_capacity(arms.len());
        let mut has_catch_all = false;
        let mut has_ok = false;
        let mut has_err = false;
        // R.2.1: or-patterns emiten `ref __or_v if cond1 || cond2`.
        // Rust no infiere exhaustividad a partir de guards, así que
        // si HAY algún or-pattern, forzamos un catch-all artificial
        // al final, aunque conceptualmente esté cubierto.
        let mut has_or_arm = false;

        for arm in arms {
            self.push_scope();
            let (pat_code, inner_guard) = self.gen_pattern(&arm.pattern, &scrut_ty, &inner_ok_ty)?;
            if matches!(&arm.pattern, crate::ast::Pattern::Or(_)) {
                has_or_arm = true;
            }
            // R.2.2: arms con guard NO cuentan para coverage (el
            // guard puede fallar en runtime). Forzamos catch-all
            // artificial igual que con or-patterns y no actualizamos
            // las flags.
            if arm.guard.is_some() {
                has_or_arm = true; // reuso el flag "forzar catch-all"
            } else {
                update_arm_coverage(
                    &arm.pattern,
                    &mut has_catch_all,
                    &mut has_ok,
                    &mut has_err,
                );
            }
            // R.2.2 — el guard explícito del arm se gen-expr-ea en el
            // scope con los bindings del pattern visibles.
            let outer_guard = if let Some(guard_expr) = &arm.guard {
                let (g_code, _g_ty) = self.gen_expr(guard_expr)?;
                Some(g_code)
            } else {
                None
            };
            // Sp.2 — body es Vec<Stmt>. Emitimos como bloque Rust
            // `{ <stmts> }`. El "valor" es el del último Stmt::Expr;
            // los demás stmts (Return/Break/Continue/Assign) se emiten
            // sin trailing semicolon en el último si es Expr (modo
            // expr-block Rust). Si hay Return/Break/Continue, queda
            // como `!` (never), Rust acepta.
            // Sp.2 — body es Vec<Stmt>. Casos:
            //   - 1 Stmt::Expr (print o regular) → emit como expression.
            //   - 1 Stmt::Return/Break/Continue → emit como `{ return X }`
            //     SIN `;` trailing para que Rust trate el block como
            //     `!` (never type, coercible a cualquier T del match).
            //   - >1 stmts → block con stmts y último expr-tail.
            //
            // Helper: strip trailing `;\n?` de un código de stmt cuando
            // sabemos que es la cola de un block que debe tipar como `!`.
            fn strip_trailing_semi(s: &str) -> String {
                let trimmed = s.trim_end();
                trimmed.strip_suffix(';').unwrap_or(trimmed).to_string()
            }
            let (body_code, body_ty) = if arm.body.len() == 1 {
                match &arm.body[0] {
                    Stmt::Expr(e, _) => {
                        if is_print_call(e) {
                            let print_code = self.gen_print_to_string(e)?;
                            (format!("{{ {}; }}", print_code), Type::Null)
                        } else {
                            self.gen_expr(e)?
                        }
                    }
                    other @ (Stmt::Return(..) | Stmt::Break(..) | Stmt::Continue(..)) => {
                        // `return X`/`break`/`continue` sin trailing `;`
                        // — type `!` que coerce a cualquier T del match.
                        let stmt_code = self.gen_stmt_to_string(other)?;
                        let stripped = strip_trailing_semi(&stmt_code);
                        (format!("{{ {} }}", stripped), Type::Null)
                    }
                    other => {
                        // Otros stmts: emisión normal con `;`.
                        let stmt_code = self.gen_stmt_to_string(other)?;
                        (format!("{{ {} }}", stmt_code), Type::Null)
                    }
                }
            } else {
                // Múltiples stmts → bloque Rust con todos los stmts y
                // último como expr-tail si es Stmt::Expr o como `!` si
                // es Return/Break/Continue (sin trailing `;`).
                let mut block = String::from("{ ");
                let mut tail_ty = Type::Null;
                for (i, stmt) in arm.body.iter().enumerate() {
                    let is_last = i + 1 == arm.body.len();
                    if is_last {
                        match stmt {
                            Stmt::Expr(e, _) => {
                                let (code, ty) = self.gen_expr(e)?;
                                block.push_str(&code);
                                tail_ty = ty;
                                continue;
                            }
                            Stmt::Return(..) | Stmt::Break(..) | Stmt::Continue(..) => {
                                let code = self.gen_stmt_to_string(stmt)?;
                                block.push_str(&strip_trailing_semi(&code));
                                tail_ty = Type::Null;
                                continue;
                            }
                            _ => {}
                        }
                    }
                    let code = self.gen_stmt_to_string(stmt)?;
                    block.push_str(&code);
                    block.push(' ');
                }
                block.push_str(" }");
                (block, tail_ty)
            };
            self.pop_scope();

            // Combinar inner_guard (Str/Range/Or) con outer_guard (R.2.2)
            // usando `&&`. Si los dos están, paréntesis envuelven cada uno.
            let guard_combined = match (inner_guard, outer_guard) {
                (None, None) => String::new(),
                (Some(g), None) | (None, Some(g)) => format!(" if {}", g),
                (Some(g1), Some(g2)) => format!(" if ({}) && ({})", g1, g2),
            };
            arm_pieces.push(format!("{}{} => {}", pat_code, guard_combined, body_code));
            arm_tys.push(body_ty);
        }

        // Determinar si necesitamos un catch-all artificial para que
        // rustc acepte el match. Casos exhaustivos sin agregar nada:
        //   - hay un Ident/Wildcard arm;
        //   - el scrutinee es Result<T> y tenemos al menos un Ok y un Err.
        let result_exhaustive =
            inner_ok_ty.is_some() && has_ok && has_err;
        if !has_catch_all && (!result_exhaustive || has_or_arm) {
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
    /// introduzca. Devuelve `(pattern_code, optional_inner_guard)`:
    /// algunos patterns Fitz (Str/Range/Or) no se traducen 1:1 a un
    /// pattern Rust puro y necesitan guard adicional. El caller
    /// combina ese guard con el guard explícito del arm (R.2.2)
    /// usando `&&`.
    fn gen_pattern(
        &mut self,
        pat: &crate::ast::Pattern,
        scrut_ty: &Type,
        ok_inner_ty: &Option<Type>,
    ) -> Result<(String, Option<String>), FitzError> {
        use crate::ast::Pattern;
        match pat {
            Pattern::Int(n) => Ok((format!("{}i64", n), None)),
            Pattern::Float(f) => Ok((format!("{}f64", f), None)),
            Pattern::Str(s) => {
                // Rust no acepta literal `&str` como pattern contra
                // `String`. Bindeamos como `ref __s_<n>` y comparamos
                // en el guard. Mini-tanda Rt: counter en CodegenCtx
                // garantiza nombres únicos cuando hay varios Str
                // patterns adentro de un Tuple.
                let id = self.pattern_slot_counter;
                self.pattern_slot_counter += 1;
                Ok((
                    format!("ref __s_{}", id),
                    Some(format!("__s_{}.as_str() == {}", id, rust_str_literal(s))),
                ))
            }
            Pattern::Bool(b) => Ok((b.to_string(), None)),
            Pattern::Null => {
                if matches!(scrut_ty, Type::Null) {
                    Ok(("()".to_string(), None))
                } else {
                    Ok(("_".to_string(), None))
                }
            }
            Pattern::Ident(name) => {
                self.declare_var(name.clone(), scrut_ty.clone());
                Ok((name.clone(), None))
            }
            Pattern::Wildcard => Ok(("_".to_string(), None)),
            Pattern::OkBinding(name) => {
                let bind_ty = ok_inner_ty.clone().unwrap_or(Type::Any);
                self.declare_var(name.clone(), bind_ty);
                Ok((format!("Ok({})", name), None))
            }
            Pattern::ErrBinding(name) => {
                // Mini-tanda Re+ — `Err(e)` tipa con el E del
                // `Result { ok, err }` del scrutinee. Pre-Re+ siempre
                // era Str (default); ahora puede ser Int/Instance/etc.
                let bind_ty = match scrut_ty {
                    Type::Result { err, .. } => (**err).clone(),
                    _ => Type::Str,
                };
                self.declare_var(name.clone(), bind_ty);
                Ok((format!("Err({})", name), None))
            }
            Pattern::OkWildcard => Ok(("Ok(_)".to_string(), None)),
            Pattern::ErrWildcard => Ok(("Err(_)".to_string(), None)),
            Pattern::Range { start, end, inclusive } => {
                // Mini-tanda Rt — counter para nombre único.
                let op = if *inclusive { "..=" } else { ".." };
                let id = self.pattern_slot_counter;
                self.pattern_slot_counter += 1;
                Ok((
                    format!("__n_{}", id),
                    Some(format!("({}i64{}{}i64).contains(&__n_{})", start, op, end, id)),
                ))
            }
            Pattern::Or(subs) => {
                // R.2.1 — or-pattern como `ref __or_v_<n>` + guard que
                // es la OR de cada condición. Mini-tanda Rt: counter
                // para nombre único cuando hay Or adentro de Tuple.
                let id = self.pattern_slot_counter;
                self.pattern_slot_counter += 1;
                let bind_name = format!("__or_v_{}", id);
                let mut conds: Vec<String> = Vec::with_capacity(subs.len());
                for sub in subs {
                    conds.push(self.pattern_to_or_cond(sub, scrut_ty, &bind_name)?);
                }
                Ok((format!("ref {}", bind_name), Some(conds.join(" || "))))
            }
            // Mini-tanda Rt — Tuple patterns con sub-patterns que
            // requieren guards (Str/Range/Or) ahora se soportan
            // combinando los inner_guards de cada slot con `&&`.
            // Antes de Rt esto era error de codegen explícito.
            Pattern::Tuple(subs) => {
                let slot_tys: Vec<Type> = match scrut_ty {
                    Type::Tuple(items) if items.len() == subs.len() => items.clone(),
                    _ => (0..subs.len()).map(|_| Type::Any).collect(),
                };
                let mut codes: Vec<String> = Vec::with_capacity(subs.len());
                let mut guards: Vec<String> = Vec::new();
                for (sub, ty) in subs.iter().zip(slot_tys.iter()) {
                    let (code, inner_guard) = self.gen_pattern(sub, ty, ok_inner_ty)?;
                    codes.push(code);
                    if let Some(g) = inner_guard {
                        guards.push(g);
                    }
                }
                let pat_code = if codes.is_empty() {
                    "()".to_string()
                } else if codes.len() == 1 {
                    format!("({},)", codes[0])
                } else {
                    format!("({})", codes.join(", "))
                };
                let combined_guard = if guards.is_empty() {
                    None
                } else if guards.len() == 1 {
                    Some(guards.remove(0))
                } else {
                    Some(guards.join(" && "))
                };
                Ok((pat_code, combined_guard))
            }
        }
    }

    /// Helper para `Pattern::Or` (R.2.1). Traduce cada sub-pattern
    /// a una expresión Bool que checkea si `bind_name` matchea. Los
    /// patrones con binding están vetados por el parser, así que
    /// acá no hay que declarar nada en el scope.
    ///
    /// Mini-tanda Rt: `bind_name` ahora es parámetro (en lugar de
    /// `__or_v` hardcoded) para que la versión sintetizada por
    /// `gen_pattern` (con counter `__or_v_<n>`) pueda llegar acá.
    fn pattern_to_or_cond(
        &self,
        pat: &crate::ast::Pattern,
        scrut_ty: &Type,
        bind_name: &str,
    ) -> Result<String, FitzError> {
        use crate::ast::Pattern;
        match pat {
            Pattern::Int(n) => Ok(format!("*{} == {}i64", bind_name, n)),
            Pattern::Float(n) => Ok(format!("*{} == {}f64", bind_name, n)),
            Pattern::Str(s) => Ok(format!("{}.as_str() == {}", bind_name, rust_str_literal(s))),
            Pattern::Bool(b) => Ok(format!("*{} == {}", bind_name, b)),
            Pattern::Null => {
                if matches!(scrut_ty, Type::Null) {
                    Ok("true".to_string())
                } else {
                    Ok("false".to_string())
                }
            }
            Pattern::Wildcard => Ok("true".to_string()),
            Pattern::Range { start, end, inclusive } => {
                let op = if *inclusive { "..=" } else { ".." };
                Ok(format!("({}i64{}{}i64).contains({})", start, op, end, bind_name))
            }
            Pattern::OkWildcard => Ok(format!("matches!({}, Ok(_))", bind_name)),
            Pattern::ErrWildcard => Ok(format!("matches!({}, Err(_))", bind_name)),
            // Los siguientes están vetados por el parser para
            // or-patterns. Si llegan acá, es un bug del parser.
            Pattern::Ident(_) | Pattern::OkBinding(_) | Pattern::ErrBinding(_) => {
                Err(FitzError::new(
                    crate::error::ErrorKind::InvalidSyntax,
                    0, 0,
                    "or-patterns no admiten bindings (bug interno: el parser debería haberlo rechazado)",
                ))
            }
            Pattern::Or(_) => {
                // Or anidado: el parser lo aplana en un solo Vec, pero
                // por seguridad, recursamos.
                Err(FitzError::new(
                    crate::error::ErrorKind::InvalidSyntax,
                    0, 0,
                    "or-patterns anidados no soportados (caso degenerado)",
                ))
            }
            Pattern::Tuple(_) => {
                // Tuple patterns no admitidos como sub-pattern de Or
                // (bindings prohibidos en Or, y tuples casi siempre
                // bindean). Si llegan acá, error.
                Err(FitzError::new(
                    crate::error::ErrorKind::InvalidSyntax,
                    0, 0,
                    "tuple patterns no admitidos adentro de or-patterns",
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
    fn gen_list_lit(&mut self, items: &[Expr], _list_span: crate::ast::Span) -> Result<(String, Type), FitzError> {
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
        // F13 SPIKE — si `lub` falla entre dos items, caemos a
        // `Type::Any` y emitimos como `List<__FitzValue>` (tagged
        // union). El sticky bit es crítico: la regla `Any + T = T`
        // del lub existente colapsaría el Any de vuelta a concreto,
        // así que una vez que detectamos heterogeneidad, lockeamos
        // el common_ty a Any sin volver a llamar lub.
        let mut common_ty = item_codes_tys[0].1.clone();
        let mut heterogeneous = false;
        for (_, t) in &item_codes_tys[1..] {
            if heterogeneous {
                break;
            }
            match lub(&common_ty, t) {
                Ok(joined) => common_ty = joined,
                Err(_) => {
                    heterogeneous = true;
                    common_ty = Type::Any;
                }
            }
        }
        if matches!(common_ty, Type::Any) {
            // F13 SPIKE + F13.A + F13.B — el tipo común no se puede
            // resolver homogéneo; emitimos `Vec<__FitzValue>` con cada
            // item wrapeado en su variante. Cubre Int/Float/Str/Bool/
            // Null/Bytes/Nominales. List/Map/Function anidados como
            // items siguen siendo follow-up.
            self.uses_fitz_value = true;
            let env = self.env;
            let wrapped: Vec<String> = item_codes_tys
                .iter()
                .map(|(c, t)| wrap_as_fitz_value_with_env(c, t, env))
                .collect::<Result<Vec<_>, FitzError>>()?;
            let code = format!(
                "Arc::new(Mutex::new(vec![{}]))",
                wrapped.join(", ")
            );
            return Ok((code, Type::List(Box::new(Type::Any))));
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

    /// Mini-tanda C — `[expr for var in iter [if filter]]` →
    /// emite un bloque Rust que arma un `Vec` con un for + push y lo
    /// envuelve en `Arc::new(Mutex::new(...))` igual que un List literal.
    /// Soporta `Range` y `List<T>` como iter (paralelo a `gen_for`).
    fn gen_list_comp(
        &mut self,
        expr: &Expr,
        var: &crate::ast::Pattern,
        iter: &Expr,
        extra_clauses: &[(crate::ast::Pattern, Expr)],
        filter: Option<&Expr>,
        span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        // Scope dedicado para todas las clauses; pop al salir.
        self.push_scope();
        // Primer clause + extras: emitir loops anidados de afuera hacia
        // adentro. El expr y filter se evalúan adentro del más interno.
        let mut loop_headers: Vec<String> = Vec::with_capacity(1 + extra_clauses.len());
        loop_headers.push(self.gen_comp_clause_header(var, iter)?);
        for (extra_var, extra_iter) in extra_clauses {
            loop_headers.push(self.gen_comp_clause_header(extra_var, extra_iter)?);
        }
        // Filter (opcional). Adentro del scope para que vea bindings.
        let filter_code = if let Some(f) = filter {
            let (fc, ft) = self.gen_expr(f)?;
            if !matches!(ft, Type::Bool) {
                self.pop_scope();
                return Err(self.err_at(f.span(), format!(
                    "el filtro `if` de la list comprehension debe ser `Bool`, recibió `{}`",
                    display_type(&ft, self.env)
                )));
            }
            Some(fc)
        } else {
            None
        };
        let (expr_code, expr_ty) = self.gen_expr(expr)?;
        self.pop_scope();
        if matches!(expr_ty, Type::Any) {
            return Err(self.err_at(span,
                "la expresión de la list comprehension tipa como `Any`: el subset compilado exige tipo concreto"
                    .to_string(),
            ));
        }
        // Cuerpo más interno: push del expr (con o sin filter).
        let inner = if let Some(fc) = filter_code {
            format!("if {fc} {{ __fitz_comp.push({expr_code}); }}")
        } else {
            format!("__fitz_comp.push({expr_code});")
        };
        // Anidamos los loops desde afuera hacia adentro.
        let mut nested = inner;
        for header in loop_headers.iter().rev() {
            nested = format!("{header} {{ {nested} }}");
        }
        let block = format!(
            "{{ let mut __fitz_comp = Vec::new(); {nested} Arc::new(Mutex::new(__fitz_comp)) }}"
        );
        Ok((block, Type::List(Box::new(expr_ty))))
    }

    /// Mini-tanda Cmp+ — codegen análogo para map comprehensions.
    /// Emite loops anidados como `gen_list_comp`, pero el cuerpo interno
    /// construye pares `(k, v)` y los inserta en un `Vec<(K, V)>` con
    /// last-write-wins en duplicados.
    #[allow(clippy::too_many_arguments)]
    fn gen_map_comp(
        &mut self,
        key: &Expr,
        value: &Expr,
        var: &crate::ast::Pattern,
        iter: &Expr,
        extra_clauses: &[(crate::ast::Pattern, Expr)],
        filter: Option<&Expr>,
        span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        self.push_scope();
        let mut loop_headers: Vec<String> = Vec::with_capacity(1 + extra_clauses.len());
        loop_headers.push(self.gen_comp_clause_header(var, iter)?);
        for (extra_var, extra_iter) in extra_clauses {
            loop_headers.push(self.gen_comp_clause_header(extra_var, extra_iter)?);
        }
        let filter_code = if let Some(f) = filter {
            let (fc, ft) = self.gen_expr(f)?;
            if !matches!(ft, Type::Bool) {
                self.pop_scope();
                return Err(self.err_at(f.span(), format!(
                    "el filtro `if` de la map comprehension debe ser `Bool`, recibió `{}`",
                    display_type(&ft, self.env)
                )));
            }
            Some(fc)
        } else {
            None
        };
        let (k_code, k_ty) = self.gen_expr(key)?;
        let (v_code, v_ty) = self.gen_expr(value)?;
        self.pop_scope();
        if matches!(k_ty, Type::Any) || matches!(v_ty, Type::Any) {
            return Err(self.err_at(span,
                "key/value de la map comprehension tipan como `Any`: el subset compilado exige tipos concretos"
                    .to_string(),
            ));
        }
        let k_rs = rust_type_for(&k_ty, self.env)?;
        let v_rs = rust_type_for(&v_ty, self.env)?;
        // Body interno: last-write-wins push.
        let push_pair = format!(
            "{{ let __k: {k_rs} = {k_code}; let __v: {v_rs} = {v_code}; \
               if let Some(__slot) = __fitz_comp.iter_mut().find(|__pair| __pair.0 == __k) {{ \
                   __slot.1 = __v; \
               }} else {{ \
                   __fitz_comp.push((__k, __v)); \
               }} }}"
        );
        let inner = if let Some(fc) = filter_code {
            format!("if {fc} {push_pair}")
        } else {
            push_pair
        };
        let mut nested = inner;
        for header in loop_headers.iter().rev() {
            nested = format!("{header} {{ {nested} }}");
        }
        let block = format!(
            "{{ let mut __fitz_comp: Vec<({k_rs}, {v_rs})> = Vec::new(); {nested} Arc::new(Mutex::new(__fitz_comp)) }}"
        );
        Ok((block, Type::Map(Box::new(k_ty), Box::new(v_ty))))
    }

    /// Mini-tanda Cmp+ — helper que emite el header de un loop Rust
    /// `for <binding> in <iter_code>` para una clause de comprehension.
    /// Declara los bindings del pattern en el scope actual del ctx.
    /// El caller envuelve el body con `{ ... }`.
    fn gen_comp_clause_header(
        &mut self,
        var: &crate::ast::Pattern,
        iter: &Expr,
    ) -> Result<String, FitzError> {
        // Resolver el iter: Range o List<T>.
        let (iter_code, elem_ty) = if let Expr::Range { start, end, inclusive, .. } = iter {
            let (s_code, _) = self.gen_expr(start)?;
            let (e_code, _) = self.gen_expr(end)?;
            let op = if *inclusive { "..=" } else { ".." };
            (
                format!("({s_code} as i64){op}({e_code} as i64)"),
                Type::Int,
            )
        } else {
            let (ic, it_ty) = self.gen_expr(iter)?;
            let elem_ty = match &it_ty {
                Type::List(inner) => (**inner).clone(),
                other => {
                    return Err(self.err_at(iter.span(), format!(
                        "comprehension necesita un iterable (`Range` o `List<T>`), recibió `{}`",
                        display_type(other, self.env)
                    )));
                }
            };
            if matches!(elem_ty, Type::Any) {
                return Err(self.err_at(iter.span(),
                    "comprehension sobre `List<Any>`: el subset compilado exige tipo homogéneo concreto"
                        .to_string(),
                ));
            }
            (
                format!("({ic}).lock().unwrap().clone().into_iter()"),
                elem_ty,
            )
        };
        // Pattern → binding Rust. Reusa la lógica de Up (Ident/Wildcard/Tuple).
        let var_binding = match var {
            crate::ast::Pattern::Tuple(subs) => {
                let slot_tys: Vec<Type> = match &elem_ty {
                    Type::Tuple(items) if items.len() == subs.len() => items.clone(),
                    _ => (0..subs.len()).map(|_| Type::Any).collect(),
                };
                let mut parts: Vec<String> = Vec::with_capacity(subs.len());
                for (sub, st) in subs.iter().zip(slot_tys.iter()) {
                    let (b, declared) = pattern_to_simple_binding(sub, st)
                        .map_err(|msg| self.err_at(iter.span(), msg))?;
                    let mut_prefix = if b == "_" { "" } else { "mut " };
                    parts.push(format!("{}{}", mut_prefix, b));
                    for (n, t) in declared {
                        self.declare_var(n, t);
                    }
                }
                format!("({})", parts.join(", "))
            }
            _ => {
                let (b, declared) = pattern_to_simple_binding(var, &elem_ty)
                    .map_err(|msg| self.err_at(iter.span(), msg))?;
                for (n, t) in declared {
                    self.declare_var(n, t);
                }
                let mut_prefix = if b == "_" { "" } else { "mut " };
                format!("{}{}", mut_prefix, b)
            }
        };
        Ok(format!("for {var_binding} in {iter_code}"))
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
        // F13.A — sticky bit para Map (paralelo a List): si `lub`
        // falla entre dos keys o values, lockeamos a `Type::Any` y
        // emitimos Vec<(__FitzValue, __FitzValue)>.
        let _ = map_span;
        let mut common_k = entries[0].0 .1.clone();
        let mut common_v = entries[0].1 .1.clone();
        let mut heterogeneous_k = false;
        let mut heterogeneous_v = false;
        for ((_, kt), (_, vt)) in &entries[1..] {
            if !heterogeneous_k {
                match lub(&common_k, kt) {
                    Ok(j) => common_k = j,
                    Err(_) => {
                        heterogeneous_k = true;
                        common_k = Type::Any;
                    }
                }
            }
            if !heterogeneous_v {
                match lub(&common_v, vt) {
                    Ok(j) => common_v = j,
                    Err(_) => {
                        heterogeneous_v = true;
                        common_v = Type::Any;
                    }
                }
            }
        }

        if matches!(common_k, Type::Any) || matches!(common_v, Type::Any) {
            // F13.A — Map heterogéneo. Emitimos Vec<(FV, FV)>.
            // Cuando solo UNO de K o V es Any, igual wrapeamos AMBOS
            // lados como `__FitzValue` (el rust_type_for emite
            // `Vec<(FV, FV)>` para ambos casos — la cara más simple
            // del Map heterogéneo). Esto pierde info estática del
            // lado homogéneo pero simplifica el path.
            self.uses_fitz_value = true;
            let env = self.env;
            let pieces: Vec<String> = entries
                .iter()
                .map(|((kc, kt), (vc, vt))| {
                    let k_wrap = wrap_as_fitz_value_with_env(kc, kt, env)?;
                    let v_wrap = wrap_as_fitz_value_with_env(vc, vt, env)?;
                    Ok::<_, FitzError>(format!("({}, {})", k_wrap, v_wrap))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let code = format!(
                "Arc::new(Mutex::new(vec![{}]))",
                pieces.join(", ")
            );
            return Ok((
                code,
                // El tipo resultante es Map<Any, Any> sintáctico, que
                // mapea a Vec<(FV, FV)> en rust_type_for.
                Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
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
                // I.1 (mini-tanda I) — índices negativos: `xs[-1]`
                // = último. Convertimos a `effective = len + i` si
                // `i < 0`. Out-of-range → panic con mensaje claro
                // (paralelo al intérprete).
                let code = format!(
                    "{{ \
                        let __recv = ({}).clone(); \
                        let __i: i64 = {}; \
                        let __g = __recv.lock().unwrap(); \
                        let __len = __g.len() as i64; \
                        let __e = if __i < 0 {{ __len + __i }} else {{ __i }}; \
                        if __e < 0 || __e >= __len {{ \
                            panic!(\"índice fuera de rango: {{}} en lista de tamaño {{}}\", __i, __len); \
                        }} \
                        __g[__e as usize].clone() \
                    }}",
                    obj_code, idx_code
                );
                Ok((code, (**inner).clone()))
            }
            // I.1 — `s[i]` devuelve el i-ésimo char como `String`.
            // Cuenta CHARS, no bytes. Soporta negativos. Out-of-range
            // → panic. Bindeamos el `String` a una var antes de
            // tomar `.as_str()` para que no se dropee como temporary.
            Type::Str => {
                if !matches!(idx_ty, Type::Int) {
                    return Err(self.err_at(index.span(), format!(
                        "indexing de Str con `{}`: el índice debe ser Int",
                        display_type(&idx_ty, self.env)
                    )));
                }
                let code = format!(
                    "{{ \
                        let __s_owned: String = ({}); \
                        let __i: i64 = {}; \
                        let __chars: Vec<char> = __s_owned.chars().collect(); \
                        let __len = __chars.len() as i64; \
                        let __e = if __i < 0 {{ __len + __i }} else {{ __i }}; \
                        if __e < 0 || __e >= __len {{ \
                            panic!(\"índice fuera de rango: {{}} en Str de tamaño {{}}\", __i, __len); \
                        }} \
                        __chars[__e as usize].to_string() \
                    }}",
                    obj_code, idx_code
                );
                Ok((code, Type::Str))
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

    /// I.2 (mini-tanda I) — slicing. Genera Rust que clamp+slice
    /// inline. Política idéntica al evaluator:
    ///  - `start=None` → 0; `end=None` → len.
    ///  - Negativos wrap por `len + i`.
    ///  - `inclusive` ajusta `end_excl = end + 1` antes de clamp.
    ///  - Clamp ambos extremos a `[0, len]`; si start > end, vacío.
    ///  - Devuelve copia: `Arc::new(Mutex::new(slice.to_vec()))`
    ///    para List, `String` nuevo para Str.
    fn gen_slice(
        &mut self,
        object: &Expr,
        start: Option<&Expr>,
        end: Option<&Expr>,
        inclusive: bool,
        slice_span: crate::ast::Span,
    ) -> Result<(String, Type), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        // start/end codes: si None, "None". Si Some, "Some(<code>)".
        let start_code = match start {
            None => "None".to_string(),
            Some(e) => {
                let (c, t) = self.gen_expr(e)?;
                let coerced = coerce(&c, &t, &Type::Int);
                format!("Some({})", coerced)
            }
        };
        let end_code = match end {
            None => "None".to_string(),
            Some(e) => {
                let (c, t) = self.gen_expr(e)?;
                let coerced = coerce(&c, &t, &Type::Int);
                format!("Some({})", coerced)
            }
        };
        let incl_lit = if inclusive { "true" } else { "false" };

        match &obj_ty {
            Type::List(inner) => {
                let elem_rs = rust_type_for(inner, self.env)?;
                let code = format!(
                    "{{ \
                        let __recv = ({}).clone(); \
                        let __s_opt: Option<i64> = {}; \
                        let __e_opt: Option<i64> = {}; \
                        let __incl: bool = {}; \
                        let __g = __recv.lock().unwrap(); \
                        let __len = __g.len() as i64; \
                        let __s_raw = __s_opt.unwrap_or(0); \
                        let __e_raw = __e_opt.unwrap_or(if __incl {{ __len - 1 }} else {{ __len }}); \
                        let __s_wrap = if __s_raw < 0 {{ __len + __s_raw }} else {{ __s_raw }}; \
                        let __e_wrap = if __e_raw < 0 {{ __len + __e_raw }} else {{ __e_raw }}; \
                        let __e_excl = if __incl {{ __e_wrap + 1 }} else {{ __e_wrap }}; \
                        let __s_clamp = __s_wrap.clamp(0, __len); \
                        let __e_clamp = __e_excl.clamp(0, __len); \
                        let __a = __s_clamp.min(__e_clamp) as usize; \
                        let __b = __e_clamp as usize; \
                        let __slice: Vec<{}> = __g[__a..__b].to_vec(); \
                        Arc::new(Mutex::new(__slice)) \
                    }}",
                    obj_code, start_code, end_code, incl_lit, elem_rs
                );
                Ok((code, Type::List(Box::new((**inner).clone()))))
            }
            Type::Str => {
                let code = format!(
                    "{{ \
                        let __s_owned: String = ({}); \
                        let __s_opt: Option<i64> = {}; \
                        let __e_opt: Option<i64> = {}; \
                        let __incl: bool = {}; \
                        let __chars: Vec<char> = __s_owned.chars().collect(); \
                        let __len = __chars.len() as i64; \
                        let __s_raw = __s_opt.unwrap_or(0); \
                        let __e_raw = __e_opt.unwrap_or(if __incl {{ __len - 1 }} else {{ __len }}); \
                        let __s_wrap = if __s_raw < 0 {{ __len + __s_raw }} else {{ __s_raw }}; \
                        let __e_wrap = if __e_raw < 0 {{ __len + __e_raw }} else {{ __e_raw }}; \
                        let __e_excl = if __incl {{ __e_wrap + 1 }} else {{ __e_wrap }}; \
                        let __s_clamp = __s_wrap.clamp(0, __len); \
                        let __e_clamp = __e_excl.clamp(0, __len); \
                        let __a = __s_clamp.min(__e_clamp) as usize; \
                        let __b = __e_clamp as usize; \
                        __chars[__a..__b].iter().collect::<String>() \
                    }}",
                    obj_code, start_code, end_code, incl_lit
                );
                Ok((code, Type::Str))
            }
            other => Err(self.err_at(slice_span, format!(
                "slicing `[..]` sobre `{}`: solo soportado en List<T> y Str",
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
                    // `type_name` (alias local). F15: prefix `crate::`
                    // cuando el codegen es de un módulo.
                    format!("{}{}::__default_{}_{}()", self.mod_path_prefix(), mod_name, item, f.name)
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
        // Fase 9.w.2.c — preludio adicional cuando hay handlers @ws.
        // Vive separado de HTTP_RUNTIME_PRELUDE para que programas
        // HTTP sin WS no paguen el costo del bloque extra (~150 LoC
        // generados).
        if self.uses_ws {
            self.emit(WS_RUNTIME_PRELUDE);
        }
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
    #[allow(clippy::type_complexity)]
    fn collect_route_middlewares(
        &self,
        fn_name: &str,
        decorators: &[Decorator],
    ) -> Result<(Vec<String>, Vec<String>, Option<BuildCorsConfig>), FitzError> {
        // Mini-tanda P1 — devolvemos (pre, post, cors). Pre y Post se
        // distinguen por aridad de la fn middleware (1 vs 2 args).
        let mut user_fns_pre: Vec<String> = Vec::new();
        let mut user_fns_post: Vec<String> = Vec::new();
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
                    // Mini-tanda P1 — detectar aridad para clasificar
                    // Pre (1 arg) vs Post (2 args). Pre corre antes
                    // del handler con semántica gate-only; Post corre
                    // después con (Request, Response) → Response.
                    //
                    // Mini-tanda Mw-Wrap — wrap-style mw (2 args con
                    // segundo param `Fn() -> Response`) corre solo en
                    // `fitz run` por ahora. El codegen lo rechaza con
                    // un mensaje claro citando el workaround.
                    let sig = self.fn_sigs.get(n.as_str()).cloned();
                    let mw_arity = sig.as_ref().map(|s| s.params.len()).unwrap_or(1);
                    let is_wrap = sig
                        .as_ref()
                        .and_then(|s| s.params.get(1))
                        .map(|p| matches!(p, Type::Function { .. }))
                        .unwrap_or(false);
                    if is_wrap {
                        return Err(self.err(format!(
                            "@middleware(`{}`) sobre fn `{}`: wrap-style middleware \
                             (segundo param `Fn() -> Response`) corre solo en \
                             `fitz run` por ahora. Codegen es deuda residual \
                             menor — refinable si entra demanda. Para `fitz build`, \
                             usá post-process (segundo param tipo `Response`) o \
                             pre-process (1 arg).",
                            n, fn_name
                        )));
                    }
                    match mw_arity {
                        1 => user_fns_pre.push(n.clone()),
                        2 => user_fns_post.push(n.clone()),
                        n_args => {
                            return Err(self.err(format!(
                                "@middleware(`{}`) sobre fn `{}`: aridad inválida ({} args). \
                                 Aceptados: 1 (pre-process gate-only) o 2 (post-process \
                                 con `(Request, Response)`).",
                                n, fn_name, n_args
                            )));
                        }
                    }
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
        Ok((user_fns_pre, user_fns_post, cors))
    }

    /// Genera el wrapper `async fn __handler_<name>(...)` para un
    /// handler decorado con `@get/@post/@put/@delete`. Extrae path
    /// params + body (si corresponde), llama a la fn original, y
    /// convierte el resultado en una `axum::response::Response`.
    fn gen_http_handler_wrapper(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let sig = self.resolve_handler_signature(stmt)?;
        self.emit_axum_extractors(&sig)?;
        self.emit_middleware_chain(&sig);
        // Fase 9.w.1.d — auth check después de middlewares y antes de
        // body parsing. Si la ruta tiene `@authenticated`/`@admin`,
        // invoca al provider con un `Map<Str,Str>` de headers, valida
        // `Result<User>` → 401/403 según corresponda, y bindea el
        // `user` para que el handler lo reciba como arg. No-op si
        // `sig.auth == AuthSpec::None`.
        self.emit_auth_check(&sig);
        self.emit_param_coercions(&sig)?;
        self.emit_handler_dispatch_and_response(&sig);
        self.emit_cors_helpers(&sig);
        Ok(())
    }

    /// Fase 9.w.2.c — Wrapper async para handlers `@ws("/path")`.
    /// Paralelo a `gen_http_handler_wrapper` pero con dispatch axum
    /// distinto: `WebSocketUpgrade` extractor + `on_upgrade` closure.
    ///
    /// Estructura del Rust emitido:
    ///
    /// ```rust
    /// async fn __ws_handler_<name>(
    ///     ws: axum::extract::ws::WebSocketUpgrade,
    ///     __hmap: axum::http::HeaderMap,
    /// ) -> axum::response::Response {
    ///     // [auth check si aplica — return 401/403 pre-upgrade]
    ///     ws.on_upgrade(move |socket| async move {
    ///         let endpoint = "/<path>".to_string();
    ///         let (__conn, __writer) = __fitz_ws_setup::<T>(socket, endpoint.clone());
    ///         let __conn_id = __conn.conn_id;
    ///         let _ = <user_handler>(__conn, ...optional user...).await;
    ///         __fitz_ws_unregister(&endpoint, __conn_id);
    ///         let _ = __writer.await;
    ///     }).into_response()
    /// }
    /// ```
    fn gen_ws_handler_wrapper(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::FnDef {
            name,
            params,
            decorators,
            ..
        } = stmt
        else {
            return Err(self.err("gen_ws_handler_wrapper: esperaba Stmt::FnDef"));
        };
        // Path del decorator @ws.
        let ws_deco = decorators
            .iter()
            .find(|d| d.name == "ws")
            .ok_or_else(|| self.err(format!("fn `{}`: sin decorator @ws", name)))?;
        let path_arg = ws_deco
            .args
            .first()
            .ok_or_else(|| self.err(format!("@ws sobre `{}`: falta path arg", name)))?;
        let path = match path_arg {
            Expr::Str(s, _) => s.clone(),
            _ => {
                return Err(self.err(format!(
                    "@ws sobre `{}`: el path debe ser Str literal",
                    name
                )));
            }
        };

        // Resolver auth desde decorators (paralelo a HandlerSig).
        let mut auth = crate::http::AuthSpec::None;
        for d in decorators {
            match d.name.as_str() {
                "authenticated" if auth == crate::http::AuthSpec::None => {
                    auth = crate::http::AuthSpec::Authenticated;
                }
                "admin" => auth = crate::http::AuthSpec::Admin,
                _ => {}
            }
        }

        // Identificar el param `WsConn<T>` y (si hay auth) el param
        // `user: T_user`. Resolver tipos para la signature Rust del
        // user handler.
        let mut ws_conn_param: Option<(String, Type)> = None;
        let mut user_param: Option<(String, Type)> = None;
        for p in params {
            let te = p.type_.as_ref().ok_or_else(|| {
                self.err_at(
                    stmt.span(),
                    format!("@ws fn `{}`: param `{}` necesita anotación de tipo", name, p.name),
                )
            })?;
            let ty = resolve_type_expr(te, self.env).map_err(|e| {
                self.err_at(
                    stmt.span(),
                    format!("@ws fn `{}`: param `{}`: {}", name, p.name, e.message),
                )
            })?;
            if matches!(ty, Type::WsConn(_)) {
                ws_conn_param = Some((p.name.clone(), ty));
            } else if auth != crate::http::AuthSpec::None && user_param.is_none() {
                user_param = Some((p.name.clone(), ty));
            }
        }
        let (conn_name, conn_ty) = ws_conn_param.ok_or_else(|| {
            self.err_at(
                stmt.span(),
                format!(
                    "@ws fn `{}`: falta param `WsConn<T>` (validado por checker, defensivo en codegen)",
                    name
                ),
            )
        })?;
        let conn_ty_rs = rust_type_for(&conn_ty, self.env)?;

        // Emitir signature del wrapper.
        writeln!(&mut self.output, "async fn __ws_handler_{}(", name).unwrap();
        self.emit("    ws: axum::extract::ws::WebSocketUpgrade,\n");
        self.emit("    __hmap: axum::http::HeaderMap,\n");
        self.emit(") -> axum::response::Response {\n");
        self.emit("    use axum::response::IntoResponse;\n");

        // Auth pre-upgrade. Paralelo a `emit_auth_check` pero adaptado
        // para el contexto WS (return Response directo si falla).
        if auth != crate::http::AuthSpec::None {
            let provider = self.auth_provider_name.clone().ok_or_else(|| {
                self.err_at(stmt.span(), format!(
                    "@ws fn `{}` con `@authenticated`/`@admin`: falta `@auth_provider` (validado por checker, defensivo)",
                    name,
                ))
            })?;
            let provider_await = if self.auth_provider_is_async {
                ".await"
            } else {
                ""
            };
            self.emit("    let __auth_headers: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> = {\n");
            self.emit("        let mut __pairs: Vec<(String, String)> = Vec::with_capacity(__hmap.len());\n");
            self.emit("        for (k, v) in __hmap.iter() {\n");
            self.emit("            if let Ok(vs) = v.to_str() {\n");
            self.emit("                __pairs.push((k.as_str().to_string(), vs.to_string()));\n");
            self.emit("            }\n");
            self.emit("        }\n");
            self.emit("        std::sync::Arc::new(std::sync::Mutex::new(__pairs))\n");
            self.emit("    };\n");
            writeln!(
                &mut self.output,
                "    let __auth_result = {}(__auth_headers){};",
                provider, provider_await,
            )
            .unwrap();
            self.emit("    let __user = match __auth_result {\n");
            self.emit("        Ok(u) => u,\n");
            self.emit("        Err(__msg) => return (\n");
            self.emit("            axum::http::StatusCode::UNAUTHORIZED,\n");
            self.emit("            axum::Json(serde_json::json!({\"error\": __msg})),\n");
            self.emit("        ).into_response(),\n");
            self.emit("    };\n");
            if auth == crate::http::AuthSpec::Admin {
                self.emit("    {\n        let __guard = __user.lock().unwrap();\n        if __guard.role != \"admin\" {\n");
                self.emit("            drop(__guard);\n");
                self.emit("            return (\n");
                self.emit("                axum::http::StatusCode::FORBIDDEN,\n");
                self.emit("                axum::Json(serde_json::json!({\"error\": \"acceso prohibido — se requiere rol admin\"})),\n");
                self.emit("            ).into_response();\n");
                self.emit("        }\n    }\n");
            }
        }

        // Upgrade closure.
        // El `move` captura `__user` si aplica.
        if auth != crate::http::AuthSpec::None {
            self.emit("    ws.on_upgrade(move |__socket| async move {\n");
        } else {
            self.emit("    ws.on_upgrade(|__socket| async move {\n");
        }
        writeln!(
            &mut self.output,
            "        let __endpoint = \"{}\".to_string();",
            path,
        )
        .unwrap();
        // `T` para el setup viene del WsConn<T> del param: rust_type_for
        // ya devuelve `__FitzWsConn<T_rust>`. Necesitamos extraer T para
        // pasarlo al setup. Lo extraemos del TypeExpr resuelto.
        let t_rs = match &conn_ty {
            Type::WsConn(inner) => rust_type_for(inner, self.env)?,
            _ => unreachable!("conn_ty siempre es WsConn por construcción"),
        };
        writeln!(
            &mut self.output,
            "        let (__conn, __writer) = __fitz_ws_setup::<{}>(__socket, __endpoint.clone());",
            t_rs,
        )
        .unwrap();
        self.emit("        let __conn_id = __conn.conn_id;\n");
        // Llamar al handler. Si hay user, pasarlo además del conn.
        let _ = conn_name;
        if let Some((user_name, _user_ty)) = &user_param {
            let _ = user_name;
            writeln!(
                &mut self.output,
                "        let _ = {}(__conn, __user).await;",
                name,
            )
            .unwrap();
        } else {
            writeln!(&mut self.output, "        let _ = {}(__conn).await;", name)
                .unwrap();
        }
        // Cleanup.
        self.emit("        __fitz_ws_unregister(&__endpoint, __conn_id);\n");
        self.emit("        let _ = __writer.await;\n");
        self.emit("    }).into_response()\n");
        self.emit("}\n\n");

        // El user handler `<name>` ya se emitió con `gen_top_fn` —
        // tiene la signature `async fn <name>(conn: __FitzWsConn<T>, ...)`.
        let _ = conn_ty_rs;
        Ok(())
    }

    /// Fase 9.w.1.d — emite el código de auth check del wrapper. No-op
    /// si la ruta es pública. Llamado entre `emit_middleware_chain` y
    /// `emit_param_coercions` para que: middlewares short-circuit antes
    /// que auth (CORS preflight, logging genérico no necesitan auth);
    /// pero body parsing y args coercion no se hagan si auth falla
    /// (fail-fast).
    ///
    /// Emite:
    /// 1. `let __auth_headers = ...` — construye `Arc<Mutex<Vec<(String, String)>>>`
    ///    a partir del `__hmap` HeaderMap.
    /// 2. `let __auth_result = <provider>(__auth_headers)[.await]` —
    ///    invoca al provider singleton.
    /// 3. `match __auth_result { Ok(u) => ..., Err(msg) => 401 }`.
    /// 4. Para `@admin`: chequea `user.role == "admin"` → 403.
    /// 5. Bindea `let <user_param_name> = u;` para el handler.
    fn emit_auth_check(&mut self, sig: &HandlerSig) {
        if sig.auth == crate::http::AuthSpec::None {
            return;
        }
        let user_name = sig
            .auth_user_param_name
            .clone()
            .expect("auth_user_param_name siempre Some cuando auth != None");
        let provider = self
            .auth_provider_name
            .clone()
            .expect("auth_provider_name pre-scaneado en generate_main_rs");
        let provider_await = if self.auth_provider_is_async {
            ".await"
        } else {
            ""
        };
        // Build Map<Str,Str> de headers.
        self.emit("    let __auth_headers: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> = {\n");
        self.emit("        let mut __pairs: Vec<(String, String)> = Vec::with_capacity(__hmap.len());\n");
        self.emit("        for (k, v) in __hmap.iter() {\n");
        self.emit("            if let Ok(vs) = v.to_str() {\n");
        self.emit("                __pairs.push((k.as_str().to_string(), vs.to_string()));\n");
        self.emit("            }\n");
        self.emit("        }\n");
        self.emit("        std::sync::Arc::new(std::sync::Mutex::new(__pairs))\n");
        self.emit("    };\n");
        // Invoke provider.
        writeln!(
            &mut self.output,
            "    let __auth_result = {}(__auth_headers){};",
            provider, provider_await,
        )
        .unwrap();
        // Match result.
        writeln!(
            &mut self.output,
            "    let {} = match __auth_result {{",
            user_name,
        )
        .unwrap();
        self.emit("        Ok(u) => u,\n");
        self.emit("        Err(__msg) => return (\n");
        self.emit("            axum::http::StatusCode::UNAUTHORIZED,\n");
        self.emit("            axum::Json(serde_json::json!({\"error\": __msg})),\n");
        self.emit("        ).into_response(),\n");
        self.emit("    };\n");
        // Admin role check.
        if sig.auth == crate::http::AuthSpec::Admin {
            writeln!(
                &mut self.output,
                "    {{\n        let __guard = {}.lock().unwrap();\n        if __guard.role != \"admin\" {{",
                user_name,
            )
            .unwrap();
            self.emit("            drop(__guard);\n");
            self.emit("            return (\n");
            self.emit("                axum::http::StatusCode::FORBIDDEN,\n");
            self.emit("                axum::Json(serde_json::json!({\"error\": \"acceso prohibido — se requiere rol admin\"})),\n");
            self.emit("            ).into_response();\n");
            self.emit("        }\n");
            self.emit("    }\n");
        }
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
        let (mw_user_fns, mw_user_fns_post, mw_cors) =
            self.collect_route_middlewares(name, decorators)?;
        let has_middleware = !mw_user_fns.is_empty() || !mw_user_fns_post.is_empty();
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

        // Fase 9.w.1.d — recolectar política de auth de la ruta antes de
        // categorizar params. Si la ruta tiene `@authenticated`/`@admin`,
        // el "leftover" (el param que no es path/query/header) se trata
        // como `auth_user_param_name` (no como body). Política MVP
        // espejo del intérprete: handler protegido NO admite body
        // separado del user.
        let mut auth = crate::http::AuthSpec::None;
        for d in decorators {
            match d.name.as_str() {
                "authenticated" if auth == crate::http::AuthSpec::None => {
                    auth = crate::http::AuthSpec::Authenticated;
                }
                "admin" => auth = crate::http::AuthSpec::Admin,
                _ => {}
            }
        }
        if auth != crate::http::AuthSpec::None && self.auth_provider_name.is_none() {
            return Err(self.err_at(fn_span, format!(
                "fn `{}`: `@authenticated`/`@admin` exige declarar un \
                 `@auth_provider` antes en el archivo.",
                name,
            )));
        }
        let auth_user_param_name: Option<String> =
            if auth != crate::http::AuthSpec::None {
                let candidates: Vec<&str> = resolved_params
                    .iter()
                    .filter(|(n, _)| {
                        !template_params.iter().any(|tp| tp == n)
                            && !query_template_params.iter().any(|q| q == n)
                            && !header_specs.iter().any(|(_, fp, _)| fp == n)
                    })
                    .map(|(n, _)| n.as_str())
                    .collect();
                if candidates.is_empty() {
                    return Err(self.err_at(fn_span, format!(
                        "fn `{}`: falta param del tipo `User` (inyectado por auth).",
                        name,
                    )));
                }
                if candidates.len() > 1 {
                    return Err(self.err_at(fn_span, format!(
                        "fn `{}`: hay {} params que no son path/query/header. \
                         En MVP, un handler protegido por auth admite solo el \
                         param `user` y NO body separado.",
                        name,
                        candidates.len(),
                    )));
                }
                Some(candidates[0].to_string())
            } else {
                None
            };

        // Categorizar: cada param es path / query / header / body / auth_user.
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
            } else if auth_user_param_name.as_deref() == Some(n.as_str()) {
                // Auth-injected user — NO es body, lo maneja el wrapper auth.
                continue;
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
        let returns_result = matches!(resolved_ret, Type::Result { .. });
        // Mini-tanda HTTP-Err — detectar si el E del Result<T, E> es
        // Nominal con field `status: Int`. Habilita la convención de
        // status codes específicos por kind de Err.
        let err_has_status_field = match &resolved_ret {
            Type::Result { err, .. } => err_type_has_status_field(err, self.env),
            _ => false,
        };

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
            err_has_status_field,
            mw_user_fns,
            mw_user_fns_post,
            mw_cors,
            has_middleware,
            has_cors,
            auth,
            auth_user_param_name,
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
        // (Fase 7.6), hay middlewares (MW.3) que reciben Request, hay
        // CORS (Q.3) que necesita leer el `Origin` del request para
        // resolver los headers `Access-Control-Allow-*`, o hay body
        // (mini-tanda UC: leemos Content-Type para dispatch entre
        // JSON / urlencoded / 415), o el handler exige auth nativa
        // (9.w.1.d: el provider recibe Map<Str,Str> de headers). Sin
        // ninguno, axum NO extrae el HeaderMap (zero-overhead en
        // handlers simples).
        if !sig.header_params.is_empty()
            || sig.has_middleware
            || sig.has_cors
            || sig.body_param.is_some()
            || sig.auth != crate::http::AuthSpec::None
        {
            self.emit("    __hmap: axum::http::HeaderMap,\n");
        }
        if let Some((bn, _bt)) = &sig.body_param {
            // Mini-tanda UC — extraemos Bytes en lugar de Json para
            // poder dispatchar por Content-Type (JSON vs urlencoded)
            // en `emit_param_coercions`. axum::body::Bytes acepta
            // cualquier cuerpo, sin imponer parseo.
            writeln!(
                &mut self.output,
                "    {}_body_bytes: axum::body::Bytes,",
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

        // Si hay body con tipo declarado, dispatchar primero por
        // Content-Type (mini-tanda UC + Hpx.1): JSON o vacío → parsear
        // como JSON; urlencoded → parsear con `__parse_urlencoded` que
        // devuelve un `serde_json::Value::Object`; otro → 415 con el
        // mismo mensaje del intérprete (`http::handle_task`). El
        // `__from_fitz_json` genérico consume el `serde_json::Value`
        // intermedio (Map<Str, Str> para urlencoded; estructura libre
        // para JSON), así que el resto del path queda intacto.
        if let Some((bn, bt)) = &sig.body_param {
            writeln!(
                &mut self.output,
                "    let {}_ct_primary: String = __hmap.get(\"content-type\")",
                bn,
            )
            .unwrap();
            self.emit("        .and_then(|v| v.to_str().ok())\n");
            self.emit("        .map(|ct| ct.split(';').next().unwrap_or(\"\").trim().to_ascii_lowercase())\n");
            self.emit("        .unwrap_or_default();\n");
            writeln!(
                &mut self.output,
                "    let {}_raw: serde_json::Value = if {}_ct_primary.is_empty() || {}_ct_primary == \"application/json\" {{",
                bn, bn, bn,
            )
            .unwrap();
            writeln!(
                &mut self.output,
                "        match serde_json::from_slice(&{}_body_bytes) {{",
                bn,
            )
            .unwrap();
            self.emit("            Ok(v) => v,\n");
            self.emit("            Err(e) => return (\n");
            self.emit("                axum::http::StatusCode::BAD_REQUEST,\n");
            self.emit("                axum::Json(serde_json::json!({\"error\": format!(\"body JSON inválido: {}\", e)})),\n");
            self.emit("            ).into_response(),\n");
            self.emit("        }\n");
            writeln!(
                &mut self.output,
                "    }} else if {}_ct_primary == \"application/x-www-form-urlencoded\" {{",
                bn,
            )
            .unwrap();
            writeln!(
                &mut self.output,
                "        match __parse_urlencoded(&{}_body_bytes) {{",
                bn,
            )
            .unwrap();
            self.emit("            Ok(v) => v,\n");
            self.emit("            Err(e) => return (\n");
            self.emit("                axum::http::StatusCode::BAD_REQUEST,\n");
            self.emit("                axum::Json(serde_json::json!({\"error\": e})),\n");
            self.emit("            ).into_response(),\n");
            self.emit("        }\n");
            // Mini-tanda MP-Build — multipart/form-data ahora también
            // funciona en `fitz build` (paridad bit-a-bit con el
            // intérprete). El helper `__parse_multipart` devuelve un
            // `serde_json::Value::Object` con cada part como text
            // (Value::String) o file (Value::Object con shape de
            // FileData), que `__FromFitzJson` puede consumir.
            writeln!(
                &mut self.output,
                "    }} else if {}_ct_primary == \"multipart/form-data\" {{",
                bn,
            )
            .unwrap();
            self.emit("        let __ct_full = __hmap.get(\"content-type\").and_then(|v| v.to_str().ok()).unwrap_or(\"\");\n");
            self.emit("        let __boundary = match __extract_multipart_boundary(__ct_full) {\n");
            self.emit("            Some(b) => b,\n");
            self.emit("            None => return (\n");
            self.emit("                axum::http::StatusCode::BAD_REQUEST,\n");
            self.emit("                axum::Json(serde_json::json!({\"error\": \"multipart/form-data: falta el parámetro `boundary` en Content-Type\"})),\n");
            self.emit("            ).into_response(),\n");
            self.emit("        };\n");
            writeln!(
                &mut self.output,
                "        match __parse_multipart(&{}_body_bytes, &__boundary) {{",
                bn,
            )
            .unwrap();
            self.emit("            Ok(v) => v,\n");
            self.emit("            Err(e) => return (\n");
            self.emit("                axum::http::StatusCode::BAD_REQUEST,\n");
            self.emit("                axum::Json(serde_json::json!({\"error\": e})),\n");
            self.emit("            ).into_response(),\n");
            self.emit("        }\n");
            self.emit("    } else {\n");
            self.emit("        let __ct_display = __hmap.get(\"content-type\").and_then(|v| v.to_str().ok()).unwrap_or(\"(sin header)\").to_string();\n");
            self.emit("        return (\n");
            self.emit("            axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,\n");
            // Mini-tanda MP-Build — paridad bit-a-bit con `fitz run`:
            // los tres CTs soportados, otros formatos → 415.
            self.emit("            axum::Json(serde_json::json!({\"error\": format!(\"Content-Type no soportado: '{}'. El handler espera JSON (`application/json`), urlencoded (`application/x-www-form-urlencoded`) o multipart (`multipart/form-data`). Otros formatos quedan como sub-paso futuro.\", __ct_display)})),\n");
            self.emit("        ).into_response();\n");
            self.emit("    };\n");

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
        let has_post_mws = !sig.mw_user_fns_post.is_empty();
        if returns_response {
            self.emit("    let mut __resp: __FitzResponse = __result;\n");
            // Mini-tanda P1 (Mw.next codegen) — emit post-mw chain.
            // Cada post mw recibe `(Request, Response)` y devuelve un
            // nuevo `__FitzResponse`. Reverse order (semántica de wrap:
            // último registrado ve la response primero).
            if has_post_mws {
                for mw_name in sig.mw_user_fns_post.iter().rev() {
                    writeln!(
                        &mut self.output,
                        "    __resp = {}(__req.clone(), __resp);",
                        mw_name,
                    )
                    .unwrap();
                }
            }
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
        } else if sig.returns_result && has_post_mws {
            // Mini-tanda RP — Result<T> + post mws: construir
            // `__resp: __FitzResponse` via match Ok/Err, correr post
            // mws en reverse, y convertir a axum Response. Mismo set
            // de casos que el path Result regular (Ok+200, Err con/sin
            // status field, status fuera de rango), pero produciendo
            // __FitzResponse en lugar de axum Response inline.
            self.emit("    let mut __resp: __FitzResponse = match __result {\n");
            self.emit("        Ok(__v) => __FitzResponse {\n");
            self.emit("            status: 200,\n");
            self.emit("            body: __v.__to_fitz_json(),\n");
            self.emit("        },\n");
            if sig.err_has_status_field {
                self.emit("        Err(__e) => {\n");
                self.emit("            let __raw_status = __e.lock().unwrap().status;\n");
                self.emit("            if (100i64..1000i64).contains(&__raw_status) {\n");
                self.emit("                __FitzResponse {\n");
                self.emit("                    status: __raw_status as u16,\n");
                self.emit("                    body: __e.__to_fitz_json(),\n");
                self.emit("                }\n");
                self.emit("            } else {\n");
                self.emit("                let __msg = format!(\"status code inválido en Err: {} (debe estar en 100..1000)\", __raw_status);\n");
                self.emit("                __FitzResponse {\n");
                self.emit("                    status: 500,\n");
                self.emit("                    body: serde_json::json!({\"error\": __msg}),\n");
                self.emit("                }\n");
                self.emit("            }\n");
                self.emit("        },\n");
            } else {
                self.emit("        Err(__e) => __FitzResponse {\n");
                self.emit("            status: 500,\n");
                self.emit("            body: serde_json::json!({\"error\": __e}),\n");
                self.emit("        },\n");
            }
            self.emit("    };\n");
            // Post mws en reverse order — semántica wrap.
            for mw_name in sig.mw_user_fns_post.iter().rev() {
                writeln!(
                    &mut self.output,
                    "    __resp = {}(__req.clone(), __resp);",
                    mw_name,
                )
                .unwrap();
            }
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
            // Mini-tanda HTTP-Err — convención: si el E del Result es
            // un Nominal con field `status: Int`, leemos ese field y
            // usamos su valor como HTTP status code (validado a 100..1000).
            // El body es el Instance serializado (sin envolver en
            // `{"error": ...}`). Sin status field → 500 histórico.
            //
            // Mini-tanda HC.1 — status fuera de 100..1000 ya NO cae
            // silenciosamente a 500. Emitimos 500 + body con un msg
            // claro citando el status inválido. Paridad con el path
            // del intérprete (`value_to_outcome` en `http.rs`).
            if sig.err_has_status_field {
                self.emit("        Err(__e) => {\n");
                self.emit("            let __raw_status = __e.lock().unwrap().status;\n");
                self.emit("            if (100i64..1000i64).contains(&__raw_status) {\n");
                self.emit("                let __status_code = axum::http::StatusCode::from_u16(__raw_status as u16)\n");
                self.emit("                    .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);\n");
                self.emit("                (__status_code, axum::Json(__e.__to_fitz_json())).into_response()\n");
                self.emit("            } else {\n");
                self.emit("                let __msg = format!(\"status code inválido en Err: {} (debe estar en 100..1000)\", __raw_status);\n");
                self.emit("                (\n");
                self.emit("                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,\n");
                self.emit("                    axum::Json(serde_json::json!({\"error\": __msg})),\n");
                self.emit("                ).into_response()\n");
                self.emit("            }\n");
                self.emit("        },\n");
            } else {
                self.emit("        Err(__e) => (\n");
                self.emit("            axum::http::StatusCode::INTERNAL_SERVER_ERROR,\n");
                self.emit("            axum::Json(serde_json::json!({\"error\": __e})),\n");
                self.emit("        ).into_response(),\n");
            }
            self.emit("    };\n");
            writeln!(
                &mut self.output,
                "    __apply_cors_and_respond(__built, {})",
                cors_arg
            )
            .unwrap();
            self.emit("\n");
        } else {
            // Mini-tanda P1 — emit post mws sobre el response del
            // handler plain-T. Construimos __FitzResponse intermedio
            // si hay post mws para encadenar.
            if has_post_mws {
                self.emit("    let mut __resp = __FitzResponse {\n");
                self.emit("        status: 200,\n");
                self.emit("        body: __result.__to_fitz_json(),\n");
                self.emit("    };\n");
                for mw_name in sig.mw_user_fns_post.iter().rev() {
                    writeln!(
                        &mut self.output,
                        "    __resp = {}(__req.clone(), __resp);",
                        mw_name,
                    )
                    .unwrap();
                }
                self.emit("    let __built = (\n");
                self.emit("        axum::http::StatusCode::from_u16(__resp.status)\n");
                self.emit("            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),\n");
                self.emit("        axum::Json(__resp.body),\n");
                self.emit("    ).into_response();\n");
            } else {
                self.emit("    let __built = (\n");
                self.emit("        axum::http::StatusCode::OK,\n");
                self.emit("        axum::Json(__result.__to_fitz_json()),\n");
                self.emit("    ).into_response();\n");
            }
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
            // Mini-tanda HTTP-Cors — echo del Origin sin filtro.
            // Si la request no manda Origin, no emitimos el header
            // (mismo comportamiento que Set sin match).
            BuildAllowOrigin::Echo => {
                self.emit("    if let Some(__req) = origin {\n");
                self.emit("        __out.push((\"access-control-allow-origin\", __req.to_string()));\n");
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
    #[allow(clippy::too_many_arguments)]
    fn gen_http_main(
        &mut self,
        http_fns: &[&Stmt],
        ws_fns: &[&Stmt],
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
        // Fase 9.w.2.c — registrar rutas WS. Cada `@ws("/path")` se
        // monta como axum GET (el handshake HTTP es GET) que internamente
        // hace el upgrade.
        for stmt in ws_fns {
            let Stmt::FnDef { name, decorators, .. } = stmt else { continue };
            for d in decorators {
                if d.name != "ws" {
                    continue;
                }
                let path_arg = match d.args.first() {
                    Some(p) => p,
                    None => continue,
                };
                let (path, _q) = parse_http_path(path_arg)?;
                self.emit_indent();
                writeln!(
                    &mut self.output,
                    "    .route(\"{}\", axum::routing::get(__ws_handler_{}))",
                    path, name,
                )
                .unwrap();
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
                    StrPart::Expr(Expr::Ident(name, _), _) => {
                        buf.push('{');
                        buf.push_str(name);
                        buf.push('}');
                    }
                    StrPart::Expr(_, _) => {
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
    /// Mini-tanda HTTP-Cors — echo del Origin recibido sin filtro.
    Echo,
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
                    // Mini-tanda HTTP-Cors — el valor especial `"echo"`
                    // construye `BuildAllowOrigin::Echo` para echo
                    // sin filtro del Origin recibido.
                    Expr::Str(s, _) => {
                        cfg.allow_origin = Some(if s == "echo" {
                            BuildAllowOrigin::Echo
                        } else {
                            BuildAllowOrigin::Literal(s.clone())
                        });
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

/// Mini-tanda MP2 + File.content Bytes — `File` built-in para
/// multipart bodies. `content` ahora es `Vec<u8>` (Bytes en Fitz)
/// para soportar files binarios. El path multipart en codegen
/// HTTP también extrae bytes raw (`parse_multipart` del runtime
/// emite array de Int al JSON intermedio que se deserializa a
/// Vec<u8> via __FromFitzJson — refactor pendiente para usar
/// formato más eficiente).
#[derive(Clone, PartialEq)]
#[allow(dead_code)]
struct FileData {
    name: Option<String>,
    content_type: Option<String>,
    content: Vec<u8>,
}
#[allow(dead_code)]
type File = std::sync::Arc<std::sync::Mutex<FileData>>;

impl std::fmt::Display for FileData {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "File {{ name: {}, content_type: {}, content: {} }}",
            self.name.as_ref().map(|s| format!("\"{}\"", s)).unwrap_or_else(|| "null".to_string()),
            self.content_type.as_ref().map(|s| format!("\"{}\"", s)).unwrap_or_else(|| "null".to_string()),
            __fitz_fmt_bytes(&self.content),
        )
    }
}

impl __ToFitzJson for FileData {
    fn __to_fitz_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("name".to_string(), self.name.__to_fitz_json());
        obj.insert("content_type".to_string(), self.content_type.__to_fitz_json());
        // File.content Bytes — serialize as base64 string (RFC 4648),
        // estándar de facto. Paralelo a `value_to_json(Value::Bytes)`
        // del intérprete.
        obj.insert(
            "content".to_string(),
            serde_json::Value::String(b64_encode_for_file(&self.content)),
        );
        serde_json::Value::Object(obj)
    }
}

impl __FromFitzJson for FileData {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        let obj = json
            .as_object()
            .ok_or_else(|| format!("File: se esperaba Object, se recibió {}", __json_shape(json)))?;
        let name = obj
            .get("name")
            .map(|v| <Option<String> as __FromFitzJson>::__from_fitz_json(v))
            .transpose()?
            .unwrap_or(None);
        let content_type = obj
            .get("content_type")
            .map(|v| <Option<String> as __FromFitzJson>::__from_fitz_json(v))
            .transpose()?
            .unwrap_or(None);
        // File.content Bytes — decodificar desde base64 string (output
        // de __ToFitzJson) o, como fallback, desde array de Int para
        // round-trips legacy.
        let content = match obj.get("content") {
            Some(serde_json::Value::String(s)) => b64_decode_for_file(s)?,
            Some(serde_json::Value::Array(arr)) => {
                let mut bytes = Vec::with_capacity(arr.len());
                for v in arr.iter() {
                    let n = v.as_i64().ok_or_else(|| {
                        format!("File.content array: se esperaba Int en cada item")
                    })?;
                    bytes.push(n as u8);
                }
                bytes
            }
            Some(_) => return Err("File.content: se esperaba String (base64) o Array<Int>".to_string()),
            None => Vec::new(),
        };
        Ok(FileData { name, content_type, content })
    }
}

/// File.content Bytes — base64 encoder/decoder inline para FileData
/// (paralelo a los helpers en `http.rs` del intérprete).
#[allow(dead_code)]
fn b64_encode_for_file(bytes: &[u8]) -> String {
    const T: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b0 = c[0];
        let b1 = if c.len() > 1 { c[1] } else { 0 };
        let b2 = if c.len() > 2 { c[2] } else { 0 };
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(T[((triple >> 18) & 0x3f) as usize] as char);
        out.push(T[((triple >> 12) & 0x3f) as usize] as char);
        if c.len() > 1 { out.push(T[((triple >> 6) & 0x3f) as usize] as char); } else { out.push('='); }
        if c.len() > 2 { out.push(T[(triple & 0x3f) as usize] as char); } else { out.push('='); }
    }
    out
}

#[allow(dead_code)]
fn b64_decode_for_file(s: &str) -> Result<Vec<u8>, String> {
    fn dec_char(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity((s.len() * 3) / 4);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let a = dec_char(bytes[i]).ok_or_else(|| format!("base64: char inválido '{}'", bytes[i] as char))?;
        let b = dec_char(bytes[i + 1]).ok_or_else(|| format!("base64: char inválido '{}'", bytes[i + 1] as char))?;
        out.push((a << 2) | (b >> 4));
        if i + 2 < bytes.len() {
            let c = dec_char(bytes[i + 2]).ok_or_else(|| format!("base64: char inválido '{}'", bytes[i + 2] as char))?;
            out.push((b << 4) | (c >> 2));
            if i + 3 < bytes.len() {
                let d = dec_char(bytes[i + 3]).ok_or_else(|| format!("base64: char inválido '{}'", bytes[i + 3] as char))?;
                out.push((c << 6) | d);
            }
        }
        i += 4;
    }
    Ok(out)
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

/// Mini-tanda UC — parsea `application/x-www-form-urlencoded` body
/// como un `serde_json::Value::Object` con todas las keys/values como
/// strings. Esto permite reusar el path `__from_fitz_json` para
/// deserializar a tipos Fitz (`Map<Str, Str>` o structs con fields
/// todos `Str`). URL-decoding: `+` → espacio, `%XX` → byte hex.
/// Duplicados: last-wins. Body vacío → Object vacío.
fn __parse_urlencoded(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let s = std::str::from_utf8(bytes).map_err(|e| {
        format!("body urlencoded inválido (UTF-8): {}", e)
    })?;
    let mut map = serde_json::Map::new();
    if s.is_empty() {
        return Ok(serde_json::Value::Object(map));
    }
    for kv in s.split('&') {
        let mut parts = kv.splitn(2, '=');
        let raw_k = parts.next().unwrap_or("");
        let raw_v = parts.next().unwrap_or("");
        let k = __url_decode(raw_k)?;
        let v = __url_decode(raw_v)?;
        map.insert(k, serde_json::Value::String(v));
    }
    Ok(serde_json::Value::Object(map))
}

fn __url_decode(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '+' => out.push(' '),
            '%' => {
                let h1 = chars.next().ok_or_else(|| "urlencoded: %XX incompleto".to_string())?;
                let h2 = chars.next().ok_or_else(|| "urlencoded: %XX incompleto".to_string())?;
                let byte = u8::from_str_radix(&format!("{}{}", h1, h2), 16)
                    .map_err(|_| format!("urlencoded: %{}{} no es hex válido", h1, h2))?;
                out.push(byte as char);
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Mini-tanda MP-Build — extracta el `boundary=<token>` del
/// Content-Type para `multipart/form-data`. Paralelo a
/// `extract_multipart_boundary` del intérprete.
fn __extract_multipart_boundary(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        let lower = part.to_ascii_lowercase();
        if let Some(stripped) = lower.strip_prefix("boundary=") {
            let orig_offset = part.len() - stripped.len();
            let value = &part[orig_offset..];
            let trimmed = value.trim_matches('"');
            if trimmed.is_empty() {
                return None;
            }
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Mini-tanda MP-Build — parser de `multipart/form-data`. Paralelo a
/// `parse_multipart_body` del intérprete. Devuelve un
/// `serde_json::Value::Object` con cada entry como text field
/// (Value::String) o file field (Value::Object con name/content_type/
/// content). Files binarios no-UTF8 → Err.
fn __parse_multipart(bytes: &[u8], boundary: &str) -> Result<serde_json::Value, String> {
    let delimiter = format!("--{}", boundary);
    let s = std::str::from_utf8(bytes)
        .map_err(|e| format!("multipart: body no es UTF-8 válido: {}", e))?;
    let parts_raw: Vec<&str> = s.split(&delimiter).collect();
    let mut map = serde_json::Map::new();
    for raw in parts_raw.iter().skip(1) {
        if raw.starts_with("--") {
            break;
        }
        let body = raw.strip_prefix("\r\n").unwrap_or(raw);
        let body = body.strip_suffix("\r\n").unwrap_or(body);
        let Some((headers_str, content)) = body.split_once("\r\n\r\n") else {
            return Err(
                "multipart: part malformada — falta `\\r\\n\\r\\n` entre headers y body"
                    .to_string(),
            );
        };
        let mut name_field: Option<String> = None;
        let mut filename: Option<String> = None;
        let mut content_type_part: Option<String> = None;
        for line in headers_str.split("\r\n") {
            let lower = line.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("content-disposition:") {
                let orig_offset = line.len() - rest.len();
                let value = &line[orig_offset..];
                for part in value.split(';').skip(1) {
                    let part = part.trim();
                    let Some(eq_idx) = part.find('=') else { continue; };
                    let key = part[..eq_idx].trim().to_ascii_lowercase();
                    let val = part[eq_idx + 1..].trim().trim_matches('"');
                    if key == "name" {
                        name_field = Some(val.to_string());
                    } else if key == "filename" {
                        filename = Some(val.to_string());
                    }
                }
            } else if let Some(rest) = lower.strip_prefix("content-type:") {
                let orig_offset = line.len() - rest.len();
                let value = &line[orig_offset..];
                content_type_part = Some(value.trim().to_string());
            }
        }
        let Some(name) = name_field else {
            return Err(
                "multipart: part sin `name` en Content-Disposition".to_string(),
            );
        };
        let entry = match filename {
            None => serde_json::Value::String(content.to_string()),
            Some(fname) => {
                // Construir un objeto que matchea el shape de `FileData`
                // (`name`, `content_type`, `content`). `__FromFitzJson for
                // FileData` lo consume tal cual.
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "name".to_string(),
                    if fname.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::String(fname)
                    },
                );
                obj.insert(
                    "content_type".to_string(),
                    content_type_part
                        .map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null),
                );
                obj.insert(
                    "content".to_string(),
                    serde_json::Value::String(content.to_string()),
                );
                serde_json::Value::Object(obj)
            }
        };
        map.insert(name, entry);
    }
    Ok(serde_json::Value::Object(map))
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

/// Mini-tanda UC — `List<T>` body deserialization desde JSON Array.
impl<T: __FromFitzJson> __FromFitzJson for Vec<T> {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        let arr = json
            .as_array()
            .ok_or_else(|| format!("se esperaba Array, se recibió {}", __json_shape(json)))?;
        arr.iter()
            .map(|v| T::__from_fitz_json(v))
            .collect()
    }
}

/// Mini-tanda UC — `Map<K, V>` body deserialization desde JSON Object.
/// Las claves de JSON Object son siempre `String`, así que para `K`
/// distinto de `String` parseamos la clave desde el string. Habilita
/// el case canónico de urlencoded: `Map<Str, Str>` con un Object
/// `{"k": "v"}` desde `__parse_urlencoded`.
impl<K: __MapKeyFromStr, V: __FromFitzJson> __FromFitzJson for Vec<(K, V)> {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        let obj = json
            .as_object()
            .ok_or_else(|| format!("se esperaba Object, se recibió {}", __json_shape(json)))?;
        let mut out = Vec::with_capacity(obj.len());
        for (k_str, v) in obj.iter() {
            let k = K::__from_map_key(k_str)?;
            let v = V::__from_fitz_json(v)?;
            out.push((k, v));
        }
        Ok(out)
    }
}

/// Parseo de la clave de Map desde el string del JSON Object. Espejo
/// de `__MapKey` (que va al revés).
trait __MapKeyFromStr: Sized {
    fn __from_map_key(s: &str) -> Result<Self, String>;
}
impl __MapKeyFromStr for String {
    fn __from_map_key(s: &str) -> Result<Self, String> { Ok(s.to_string()) }
}
impl __MapKeyFromStr for i64 {
    fn __from_map_key(s: &str) -> Result<Self, String> {
        s.parse::<i64>()
            .map_err(|_| format!("se esperaba Int en la clave del Map, se recibió '{}'", s))
    }
}
impl __MapKeyFromStr for f64 {
    fn __from_map_key(s: &str) -> Result<Self, String> {
        s.parse::<f64>()
            .map_err(|_| format!("se esperaba Float en la clave del Map, se recibió '{}'", s))
    }
}
impl __MapKeyFromStr for bool {
    fn __from_map_key(s: &str) -> Result<Self, String> {
        s.parse::<bool>()
            .map_err(|_| format!("se esperaba Bool en la clave del Map, se recibió '{}'", s))
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

/// Fase 9.w.2.c — Preludio adicional para WebSockets.
///
/// Define:
///   - Enum `__FitzWsOutMsg` (Text|Close) que viaja por el outbox.
///   - Global `__FITZ_WS_BROADCASTER` (LazyLock<Arc<Mutex<HashMap>>>)
///     que mantiene endpoint → list de outbox txs. Paralelo al
///     `WsBroadcaster` del intérprete (http.rs).
///   - Global `__FITZ_WS_NEXT_ID` (AtomicU64) para asignar conn_ids.
///   - Struct `__FitzWsConn<T>` con `recv/send/broadcast/close` async.
///     `T: __FitzWsMessage` permite que cualquier tipo
///     serializable a JSON pueda usarse.
///   - Trait `__FitzWsMessage` + blanket impl sobre cualquier `T:
///     __ToFitzJson + __FromFitzJson` (lo cual cubre primitivos y
///     todos los `type` Fitz custom porque ya emitimos esos impls
///     en `gen_type_http_impls`).
///   - Helpers `__fitz_ws_register/unregister/broadcast` que el
///     wrapper de cada `@ws` handler invoca.
///
/// Solo se emite cuando `ctx.uses_ws == true`. Sin uses_ws, programas
/// HTTP regulares no pagan los ~150 LoC extra ni el costo de runtime
/// del global broadcaster.
const WS_RUNTIME_PRELUDE: &str = r#"// --- 9.w.2.c: runtime WebSocket (broadcaster + struct + trait) ---

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering as __FitzOrdering};

/// Outbox message. Lo emiten `send`/`broadcast` desde el handler; lo
/// drena el writer task del conn y lo empuja al sink WebSocket.
#[derive(Clone, Debug)]
enum __FitzWsOutMsg {
    Text(String),
    Close,
}

/// Tipo del map interno del broadcaster. Por endpoint, una lista de
/// `(conn_id, outbox_tx)`. Cleanup lazy: retain elimina txs cerrados
/// al broadcast.
type __FitzWsConnList = Vec<(u64, tokio::sync::mpsc::UnboundedSender<__FitzWsOutMsg>)>;

static __FITZ_WS_BROADCASTER: OnceLock<
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, __FitzWsConnList>>>,
> = OnceLock::new();

fn __fitz_ws_broadcaster() -> &'static std::sync::Arc<
    std::sync::Mutex<std::collections::HashMap<String, __FitzWsConnList>>,
> {
    __FITZ_WS_BROADCASTER.get_or_init(|| {
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
    })
}

static __FITZ_WS_NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn __fitz_ws_register(
    endpoint: String,
    tx: tokio::sync::mpsc::UnboundedSender<__FitzWsOutMsg>,
) -> u64 {
    let conn_id = __FITZ_WS_NEXT_ID.fetch_add(1, __FitzOrdering::Relaxed);
    let b = __fitz_ws_broadcaster();
    let mut conns = b.lock().unwrap();
    conns.entry(endpoint).or_default().push((conn_id, tx));
    conn_id
}

fn __fitz_ws_unregister(endpoint: &str, conn_id: u64) {
    let b = __fitz_ws_broadcaster();
    let mut conns = b.lock().unwrap();
    if let Some(list) = conns.get_mut(endpoint) {
        list.retain(|(id, _)| *id != conn_id);
        if list.is_empty() {
            conns.remove(endpoint);
        }
    }
}

fn __fitz_ws_broadcast_payload(endpoint: &str, payload: String) {
    let b = __fitz_ws_broadcaster();
    let mut conns = b.lock().unwrap();
    if let Some(list) = conns.get_mut(endpoint) {
        list.retain(|(_, tx)| tx.send(__FitzWsOutMsg::Text(payload.clone())).is_ok());
        if list.is_empty() {
            conns.remove(endpoint);
        }
    }
}

/// Trait que cualquier `T` debe satisfacer para viajar por un
/// `WsConn<T>`. Blanket impl sobre `__ToFitzJson + __FromFitzJson`
/// (que ya emitimos para todos los `type` Fitz custom + primitivos)
/// asegura que el usuario no tiene que escribir impls manuales.
trait __FitzWsMessage: Sized {
    fn __ws_to_payload(&self) -> Result<String, String>;
    fn __ws_from_payload(payload: &str) -> Result<Self, String>;
}

impl<T> __FitzWsMessage for T
where
    T: __ToFitzJson + __FromFitzJson,
{
    fn __ws_to_payload(&self) -> Result<String, String> {
        serde_json::to_string(&self.__to_fitz_json()).map_err(|e| e.to_string())
    }
    fn __ws_from_payload(payload: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(payload).map_err(|e| e.to_string())?;
        Self::__from_fitz_json(&v)
    }
}

/// WebSocket conn tipado. El runtime construye uno por upgrade y lo
/// pasa al handler como argumento `conn: WsConn<T>`. Métodos espejo
/// del intérprete (9.w.2.b): `recv/send/broadcast/close`.
struct __FitzWsConn<T: __FitzWsMessage> {
    endpoint: String,
    conn_id: u64,
    rx: std::sync::Arc<
        tokio::sync::Mutex<
            futures_util::stream::SplitStream<axum::extract::ws::WebSocket>,
        >,
    >,
    outbox_tx: tokio::sync::mpsc::UnboundedSender<__FitzWsOutMsg>,
    closed: std::sync::Arc<AtomicBool>,
    _phantom: std::marker::PhantomData<T>,
}

// Clone manual sin bound `T: Clone`: el conn solo carga Arcs/atomics/
// strings/ids, todos clone-ables independientemente de T. PhantomData<T>
// clona libremente sin tocar T. Esto permite que el codegen Fitz haga
// `let x = conn.clone()` natural cuando el handler usa `conn` varias
// veces (cada uso necesita un clone porque needs_clone() devuelve true
// para tipos opacos).
impl<T: __FitzWsMessage> Clone for __FitzWsConn<T> {
    fn clone(&self) -> Self {
        Self {
            endpoint: self.endpoint.clone(),
            conn_id: self.conn_id,
            rx: std::sync::Arc::clone(&self.rx),
            outbox_tx: self.outbox_tx.clone(),
            closed: std::sync::Arc::clone(&self.closed),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T: __FitzWsMessage> __FitzWsConn<T> {
    /// Lee el próximo frame text y lo deserializa a T. `Err` para
    /// conn cerrada, frame binario, o JSON inválido.
    async fn recv(&self) -> Result<T, String> {
        use futures_util::StreamExt;
        if self.closed.load(__FitzOrdering::Relaxed) {
            return Err("WsConn cerrada".to_string());
        }
        loop {
            let next = {
                let mut g = self.rx.lock().await;
                g.next().await
            };
            match next {
                Some(Ok(axum::extract::ws::Message::Text(t))) => {
                    return T::__ws_from_payload(t.as_str())
                        .map_err(|e| format!("WsConn.recv(): {}", e));
                }
                Some(Ok(axum::extract::ws::Message::Binary(_))) => {
                    return Err("WsConn.recv(): frame binario no soportado en MVP".to_string());
                }
                Some(Ok(axum::extract::ws::Message::Ping(_)))
                | Some(Ok(axum::extract::ws::Message::Pong(_))) => continue,
                Some(Ok(axum::extract::ws::Message::Close(_))) | None => {
                    self.closed.store(true, __FitzOrdering::Relaxed);
                    return Err("WsConn cerrada por el peer".to_string());
                }
                Some(Err(e)) => return Err(format!("WsConn.recv(): {}", e)),
            }
        }
    }

    async fn send(&self, msg: T) -> Result<(), String> {
        if self.closed.load(__FitzOrdering::Relaxed) {
            return Err("WsConn cerrada".to_string());
        }
        let payload = msg.__ws_to_payload()?;
        self.outbox_tx
            .send(__FitzWsOutMsg::Text(payload))
            .map_err(|_| {
                self.closed.store(true, __FitzOrdering::Relaxed);
                "WsConn.send(): outbox cerrado (conn caída)".to_string()
            })
    }

    async fn broadcast(&self, msg: T) -> Result<(), String> {
        let payload = msg.__ws_to_payload()?;
        __fitz_ws_broadcast_payload(&self.endpoint, payload);
        Ok(())
    }

    fn close(&self) {
        if self.closed.load(__FitzOrdering::Relaxed) {
            return;
        }
        let _ = self.outbox_tx.send(__FitzWsOutMsg::Close);
        self.closed.store(true, __FitzOrdering::Relaxed);
    }
}

/// Construye un `__FitzWsConn<T>` a partir del axum WebSocket + el
/// endpoint, y lanza el writer task. El handler recibe el conn como
/// `&__FitzWsConn<T>` (por ref para evitar moves).
fn __fitz_ws_setup<T: __FitzWsMessage + Send + 'static>(
    socket: axum::extract::ws::WebSocket,
    endpoint: String,
) -> (__FitzWsConn<T>, tokio::task::JoinHandle<()>) {
    use futures_util::{SinkExt, StreamExt};
    let (mut sink, stream) = socket.split();
    let (outbox_tx, mut outbox_rx) = tokio::sync::mpsc::unbounded_channel();
    let conn_id = __fitz_ws_register(endpoint.clone(), outbox_tx.clone());
    let closed = std::sync::Arc::new(AtomicBool::new(false));
    let closed_w = closed.clone();
    let writer = tokio::spawn(async move {
        while let Some(m) = outbox_rx.recv().await {
            match m {
                __FitzWsOutMsg::Text(t) => {
                    if sink
                        .send(axum::extract::ws::Message::Text(t.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                __FitzWsOutMsg::Close => {
                    let _ = sink.close().await;
                    break;
                }
            }
        }
        closed_w.store(true, __FitzOrdering::Relaxed);
    });
    let conn = __FitzWsConn {
        endpoint,
        conn_id,
        rx: std::sync::Arc::new(tokio::sync::Mutex::new(stream)),
        outbox_tx,
        closed,
        _phantom: std::marker::PhantomData,
    };
    (conn, writer)
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
        Stmt::Destructure { pattern, value, .. } => {
            collect_captures_expr(value, params, locals, ctx, seen, out);
            collect_pattern_names(pattern, locals);
        }
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
            // Mini-tanda Md: var es Pattern, extraemos todos los idents.
            for name in collect_pattern_idents(var) {
                locals.insert(name);
            }
            for s in body {
                collect_captures_stmt(s, params, locals, ctx, seen, out);
            }
        }
        Stmt::Break(_, _, _) | Stmt::Continue(_, _) => {}
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
        Expr::Int(_, _) | Expr::Float(_, _) | Expr::Str(_, _) | Expr::Bool(_, _) | Expr::Null(_) | Expr::Bytes(_, _) => {}
        Expr::StrInterp(parts, _) => {
            for p in parts {
                if let crate::ast::StrPart::Expr(inner, _) = p {
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
        Expr::Slice { object, start, end, .. } => {
            collect_captures_expr(object, params, locals, ctx, seen, out);
            if let Some(s) = start { collect_captures_expr(s, params, locals, ctx, seen, out); }
            if let Some(e) = end { collect_captures_expr(e, params, locals, ctx, seen, out); }
        }
        Expr::Tuple(items, _) => {
            for it in items {
                collect_captures_expr(it, params, locals, ctx, seen, out);
            }
        }
        Expr::TupleField { tuple, .. } => {
            collect_captures_expr(tuple, params, locals, ctx, seen, out);
        }
        Expr::Loop { body, .. } => {
            for s in body {
                collect_captures_stmt(s, params, locals, ctx, seen, out);
            }
        }
        Expr::List(items, _) => {
            for it in items {
                collect_captures_expr(it, params, locals, ctx, seen, out);
            }
        }
        // Mini-tanda C — list comprehension. El `var` es local
        // adentro del expr/filter (paralelo a walk_expr_for_state_refs).
        // Mini-tanda Up — `var` ahora es Pattern; recolectamos sus
        // nombres y los marcamos locals para el walk del body.
        Expr::ListComp { expr, var, iter, extra_clauses, filter, .. } => {
            collect_captures_expr(iter, params, locals, ctx, seen, out);
            let mut added: Vec<String> = Vec::new();
            collect_pattern_bindings(var, &mut added);
            for (extra_var, extra_iter) in extra_clauses {
                for name in &added {
                    if !locals.contains(name) {
                        locals.insert(name.clone());
                    }
                }
                collect_captures_expr(extra_iter, params, locals, ctx, seen, out);
                collect_pattern_bindings(extra_var, &mut added);
            }
            for name in &added {
                if !locals.contains(name) {
                    locals.insert(name.clone());
                }
            }
            if let Some(f) = filter {
                collect_captures_expr(f, params, locals, ctx, seen, out);
            }
            collect_captures_expr(expr, params, locals, ctx, seen, out);
            for name in &added {
                locals.remove(name);
            }
        }
        // Mini-tanda Cmp+ — map comprehension.
        Expr::MapComp { key, value, var, iter, extra_clauses, filter, .. } => {
            collect_captures_expr(iter, params, locals, ctx, seen, out);
            let mut added: Vec<String> = Vec::new();
            collect_pattern_bindings(var, &mut added);
            for (extra_var, extra_iter) in extra_clauses {
                for name in &added {
                    if !locals.contains(name) {
                        locals.insert(name.clone());
                    }
                }
                collect_captures_expr(extra_iter, params, locals, ctx, seen, out);
                collect_pattern_bindings(extra_var, &mut added);
            }
            for name in &added {
                if !locals.contains(name) {
                    locals.insert(name.clone());
                }
            }
            if let Some(f) = filter {
                collect_captures_expr(f, params, locals, ctx, seen, out);
            }
            collect_captures_expr(key, params, locals, ctx, seen, out);
            collect_captures_expr(value, params, locals, ctx, seen, out);
            for name in &added {
                locals.remove(name);
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
                for stmt in &arm.body {
                    collect_captures_stmt(stmt, params, locals, ctx, seen, out);
                }
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
        // Fp.3 — NamedArg passthrough al value.
        Expr::NamedArg { value, .. } => {
            collect_captures_expr(value, params, locals, ctx, seen, out);
        }
    }
}

/// Mini-tanda P2 (5b.1/Hpx.2 chained fix) — `true` si el program
/// tiene alguna fn top-level con al menos un param sin anotar. Usado
/// por `main.rs::build_file` para decidir si correr la segunda pasada
/// del checker tras inferir params via 5b.1.
pub fn has_unannotated_fn_params(program: &Program) -> bool {
    program.iter().any(|s| {
        matches!(
            s,
            Stmt::FnDef { params, .. } if params.iter().any(|p| p.type_.is_none())
        )
    })
}

/// Mini-tanda P2 — muta el AST en-place: para cada Stmt::FnDef con
/// params sin anotar, intenta inferir el tipo via call sites
/// (`infer_param_type_from_call_sites`). Si tiene éxito, fillea
/// `Param.type_` con un TypeExpr sintetizado desde el Type resuelto.
/// Si la inferencia falla, deja el Param como estaba — el codegen
/// reportará error con sugerencia (resolve_param_type fallback).
pub fn fill_inferred_param_types(
    program: &mut Program,
    type_info: &crate::types::TypeInfo,
) {
    // Iterar sobre una copia de los fn names porque vamos a mutar el
    // program y necesitamos buscar call sites sobre el program ORIGINAL.
    let inferences: Vec<(String, usize, Type)> = {
        let mut out = Vec::new();
        for stmt in program.iter() {
            if let Stmt::FnDef { name, params, .. } = stmt {
                for (i, p) in params.iter().enumerate() {
                    if p.type_.is_none() {
                        if let Some(ty) =
                            infer_param_type_from_call_sites(program, name, i, type_info)
                        {
                            out.push((name.clone(), i, ty));
                        }
                    }
                }
            }
        }
        out
    };
    // Aplicar las inferencias.
    for (fn_name, param_idx, ty) in inferences {
        if let Some(type_expr) = type_to_type_expr(&ty) {
            for stmt in program.iter_mut() {
                if let Stmt::FnDef { name, params, .. } = stmt {
                    if name == &fn_name && param_idx < params.len() {
                        params[param_idx].type_ = Some(type_expr.clone());
                    }
                }
            }
        }
    }
}

/// Mini-tanda P2 — convierte un `Type` resuelto a su `TypeExpr`
/// sintáctico equivalente, para fillear en Param.type_ tras inferencia.
/// Cubre primitivos, Nullable, List<T>, Map<K,V>, Result<T> y Nominal
/// por nombre. Para tipos no representables sintácticamente (Function,
/// Any, Range, etc.) devuelve None — el caller deja el param sin anotar
/// y el codegen falla con su error histórico.
fn type_to_type_expr(ty: &Type) -> Option<TypeExpr> {
    Some(match ty {
        Type::Int => TypeExpr::named("Int"),
        Type::Float => TypeExpr::named("Float"),
        Type::Str => TypeExpr::named("Str"),
        Type::Bool => TypeExpr::named("Bool"),
        Type::Null => TypeExpr::named("Null"),
        Type::Nominal(id) => {
            // El TypeEnv no se expone acá; usamos el TypeId como
            // sentinel. En la práctica, el codegen consulta TypeEnv
            // via su propio `env`. Para el nombre, dependemos de que
            // el id se traduzca al nombre canónico via Type::display.
            // Aproximación: usar el formato "NominalN" no funcionaría
            // — necesitamos el nombre real. Skip Nominal por ahora;
            // el caller falla y el user anota a mano.
            let _ = id;
            return None;
        }
        Type::List(inner) => {
            let inner_te = type_to_type_expr(inner)?;
            TypeExpr::Generic {
                name: "List".into(),
                args: vec![inner_te],
            }
        }
        Type::Map(k, v) => {
            let k_te = type_to_type_expr(k)?;
            let v_te = type_to_type_expr(v)?;
            TypeExpr::Generic {
                name: "Map".into(),
                args: vec![k_te, v_te],
            }
        }
        Type::Result { ok, .. } => {
            let ok_te = type_to_type_expr(ok)?;
            TypeExpr::Generic {
                name: "Result".into(),
                args: vec![ok_te],
            }
        }
        Type::Nullable(inner) => {
            let inner_te = type_to_type_expr(inner)?;
            TypeExpr::Nullable(Box::new(inner_te))
        }
        _ => return None,
    })
}

/// Mini-tanda 5b.1 — infiere el tipo de un param de una fn sin
/// anotación, buscando el primer call site `fn_name(...)` en el
/// programa y consultando el tipo del arg en posición `param_idx` via
/// `TypeInfo`. Si el tipo es concreto (no Any), devuelve Some. Si no
/// hay call site o el tipo es Any, devuelve None.
///
/// Estrategia simple "first call site": cubre 80% del caso real
/// (fns helper que se llaman con literales o vars tipadas). Casos no
/// cubiertos: fns sin call site, args dinámicos, recursión sin caso
/// base — el codegen reporta error con sugerencia de anotar.
fn infer_param_type_from_call_sites(
    program: &Program,
    fn_name: &str,
    param_idx: usize,
    type_info: &crate::types::TypeInfo,
) -> Option<Type> {
    fn walk_stmts(
        stmts: &[Stmt],
        fn_name: &str,
        param_idx: usize,
        type_info: &crate::types::TypeInfo,
    ) -> Option<Type> {
        for stmt in stmts {
            if let Some(t) = walk_stmt(stmt, fn_name, param_idx, type_info) {
                return Some(t);
            }
        }
        None
    }
    fn walk_stmt(
        stmt: &Stmt,
        fn_name: &str,
        param_idx: usize,
        type_info: &crate::types::TypeInfo,
    ) -> Option<Type> {
        match stmt {
            Stmt::Expr(e, _)
            | Stmt::Return(e, _)
            | Stmt::Assign { value: e, .. } => walk_expr(e, fn_name, param_idx, type_info),
            Stmt::While { body, .. }
            | Stmt::Loop { body, .. }
            | Stmt::For { body, .. } => walk_stmts(body, fn_name, param_idx, type_info),
            Stmt::FnDef { body, .. } => walk_stmts(body, fn_name, param_idx, type_info),
            _ => None,
        }
    }
    fn walk_expr(
        expr: &Expr,
        fn_name: &str,
        param_idx: usize,
        type_info: &crate::types::TypeInfo,
    ) -> Option<Type> {
        match expr {
            Expr::Call { callee, args, .. } => {
                if let Expr::Ident(n, _) = callee.as_ref() {
                    if n == fn_name && param_idx < args.len() {
                        // Found a call site. Get type of arg at param_idx.
                        let arg_expr = &args[param_idx];
                        if let Some(t) = type_info.type_at(arg_expr.span()) {
                            if !matches!(t, Type::Any) {
                                return Some(t.clone());
                            }
                        }
                    }
                }
                // Recursar en args y callee.
                if let Some(t) = walk_expr(callee, fn_name, param_idx, type_info) {
                    return Some(t);
                }
                for arg in args {
                    if let Some(t) = walk_expr(arg, fn_name, param_idx, type_info) {
                        return Some(t);
                    }
                }
                None
            }
            Expr::If { condition, then, else_, .. } => {
                if let Some(t) = walk_expr(condition, fn_name, param_idx, type_info) {
                    return Some(t);
                }
                if let Some(t) = walk_stmts(then, fn_name, param_idx, type_info) {
                    return Some(t);
                }
                if let Some(els) = else_ {
                    return walk_stmts(els, fn_name, param_idx, type_info);
                }
                None
            }
            Expr::Match { value, arms, .. } => {
                if let Some(t) = walk_expr(value, fn_name, param_idx, type_info) {
                    return Some(t);
                }
                for arm in arms {
                    if let Some(t) = walk_stmts(&arm.body, fn_name, param_idx, type_info) {
                        return Some(t);
                    }
                }
                None
            }
            Expr::BinOp { left, right, .. } => {
                walk_expr(left, fn_name, param_idx, type_info)
                    .or_else(|| walk_expr(right, fn_name, param_idx, type_info))
            }
            Expr::List(items, _) => {
                for it in items {
                    if let Some(t) = walk_expr(it, fn_name, param_idx, type_info) {
                        return Some(t);
                    }
                }
                None
            }
            _ => None,
        }
    }
    walk_stmts(program, fn_name, param_idx, type_info)
}

/// Mini-tanda Hpx.2 — infiere el return type de una fn walkeando el
/// body, buscando todos los `Stmt::Return(expr)` y consultando el tipo
/// del expr en `TypeInfo` (poblado por el checker). Unifica con `lub`.
/// Si el body no tiene return explícito, devuelve `None` (caller usa
/// fallback). Si algún return da `Type::Any` o no tiene entry en
/// TypeInfo, también devuelve None para que el caller decida.
///
/// El walker es shallow para Stmts comunes (Return, ReturnStatus, Expr,
/// Assign) y descende en Stmts con cuerpos (If/Match/While/Loop/For)
/// para capturar returns anidados.
fn infer_return_type_from_body(
    body: &[Stmt],
    type_info: &crate::types::TypeInfo,
) -> Option<Type> {
    let mut collected: Vec<Type> = Vec::new();
    collect_return_types(body, type_info, &mut collected);
    if collected.is_empty() {
        return None;
    }
    // Unificar con `lub`. Si alguna unificación falla, fallback a None.
    let mut current = collected[0].clone();
    for ty in collected.iter().skip(1) {
        match lub(&current, ty) {
            Ok(unified) => current = unified,
            Err(_) => return None,
        }
    }
    // Si terminamos en Any (gradual), descartamos — el caller fallback
    // a Null mantiene el comportamiento pre-Hpx.2 para signaturas
    // demasiado vagas.
    if matches!(current, Type::Any) {
        return None;
    }
    Some(current)
}

fn collect_return_types(
    stmts: &[Stmt],
    type_info: &crate::types::TypeInfo,
    out: &mut Vec<Type>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Return(e, _) => {
                if let Some(t) = type_info.type_at(e.span()) {
                    out.push(t.clone());
                }
            }
            Stmt::While { body, .. }
            | Stmt::Loop { body, .. }
            | Stmt::For { body, .. } => collect_return_types(body, type_info, out),
            Stmt::Expr(e, _) | Stmt::Assign { value: e, .. } => {
                // Algunos Expr llevan stmts adentro (If/Match/Loop body).
                collect_returns_in_expr(e, type_info, out);
            }
            _ => {}
        }
    }
}

fn collect_returns_in_expr(
    expr: &Expr,
    type_info: &crate::types::TypeInfo,
    out: &mut Vec<Type>,
) {
    match expr {
        Expr::If { then, else_, .. } => {
            collect_return_types(then, type_info, out);
            if let Some(els) = else_ {
                collect_return_types(els, type_info, out);
            }
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                collect_return_types(&arm.body, type_info, out);
            }
        }
        Expr::Loop { body, .. } | Expr::FnExpr { body, .. } => {
            collect_return_types(body, type_info, out);
        }
        _ => {}
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
        (Type::Result { ok: a_in, err: _ }, Type::Result { ok: b_in, err: _ }) => {
            lub(a_in, b_in).map(|t| Type::Result { ok: Box::new(t), err: Box::new(Type::Str) })
        }
        // Any cede al concreto. Permite que `Err("x")` (Result<Any>)
        // unifique con `Ok(42)` (Result<Int>) → Result<Int>.
        (Type::Any, other) | (other, Type::Any) => Ok(other.clone()),
        // Tuples (mini-tanda T): mismo lub elemento por elemento si
        // misma longitud. Distintas longitudes → Err.
        (Type::Tuple(xs), Type::Tuple(ys)) if xs.len() == ys.len() => {
            let mut combined = Vec::with_capacity(xs.len());
            for (x, y) in xs.iter().zip(ys.iter()) {
                combined.push(lub(x, y)?);
            }
            Ok(Type::Tuple(combined))
        }
        _ => Err(()),
    }
}

/// Actualiza las flags de cobertura de arm para el chequeo
/// "necesitamos catch-all artificial?" en `gen_match`. Recursea
/// en `Pattern::Or` para que `Ok(_) | Err(_)` cuente como
/// ambos lados del Result (R.2.1).
fn update_arm_coverage(
    pat: &crate::ast::Pattern,
    has_catch_all: &mut bool,
    has_ok: &mut bool,
    has_err: &mut bool,
) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(_) | Pattern::Wildcard => *has_catch_all = true,
        Pattern::OkBinding(_) | Pattern::OkWildcard => *has_ok = true,
        Pattern::ErrBinding(_) | Pattern::ErrWildcard => *has_err = true,
        Pattern::Or(subs) => {
            for sub in subs {
                update_arm_coverage(sub, has_catch_all, has_ok, has_err);
            }
        }
        // Tuples: NO cuentan para cobertura de Result. El codegen
        // pondrá catch-all artificial si hace falta.
        Pattern::Tuple(_) => {}
        _ => {}
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
        | Type::Bytes
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
        Type::Result { ok: inner, err: _ } => {
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
        // Fase 9.w.2: `WsConn<T>` tampoco es comparable — el conn lleva
        // handles a streams Mutex<>'eados, dos conns distintos jamás
        // son "iguales" estructuralmente.
        Type::Function { .. }
        | Type::Future(_)
        | Type::WsConn(_)
        | Type::Any
        | Type::PyAny => Ok("false".to_string()),
        // Tuples (mini-tanda T): comparación element-wise. Rust ya
        // implementa PartialEq para tuples si cada slot lo hace,
        // así que `lhs == rhs` funciona directamente para tipos
        // primitivos. Para tuples con nominales/listas/maps
        // adentro, los elementos individuales ya tienen su impl
        // custom, así que `==` recursea bien.
        Type::Tuple(_) => {
            Ok(format!("({}) == ({})", lhs, rhs))
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

/// F13 SPIKE + F13.A + F13.B — envuelve una expresión Fitz en su
/// variante de `__FitzValue`. Usado para emitir items de listas/
/// mapas heterogéneos.
///
/// Cobertura post-F13.A+B:
/// - Primitivos (Int/Float/Str/Bool/Null): variantes directas.
/// - Bytes: `__FitzValue::Bytes(Vec<u8>)` con clone explícito.
/// - Nominales: `__FitzValue::Nominal(type_name, json)`. El nombre
///   se preserva para Display bit-a-bit con el intérprete; los
///   fields van como `serde_json::Value::Object` via
///   `__ToFitzJson`. Round-trip implícito a JSON pierde field
///   access tipado adentro del heterogéneo — el usuario que
///   necesite acceso tipado debe sacar el item con `as_<T>()` o
///   serializar/deserializar (follow-up F13.D para method dispatch).
///
/// Tipos NO cubiertos (follow-up de F13): List/Map/Function/Tuple/
/// Range/Future. La lista anidada `[[1, 2], "x"]` y el callable en
/// heterogéneo siguen abortando con error claro.
fn wrap_as_fitz_value(code: &str, ty: &Type) -> Result<String, FitzError> {
    let wrapped = match ty {
        Type::Int => format!("__FitzValue::Int({})", code),
        Type::Float => format!("__FitzValue::Float({})", code),
        Type::Str => format!("__FitzValue::Str({})", code),
        Type::Bool => format!("__FitzValue::Bool({})", code),
        Type::Null => "__FitzValue::Null".to_string(),
        // F13.A — Bytes en heterogéneos. El `code` ya es `Vec<u8>`
        // (rust_type_for(Bytes)); clone es O(n) pero correcto.
        Type::Bytes => format!("__FitzValue::Bytes({})", code),
        // F13.B — Nominales: capturamos el Display del instance
        // como String. El Display nativo de cada tipo nominal ya
        // formatea `User { id: 1, name: "x" }` bit-a-bit con el
        // intérprete (codegen emite `impl Display for FooData`).
        // El `code` típicamente es un `Arc<Mutex<FooData>>` así
        // que primero hacemos lock para llegar al Data.
        Type::Nominal(_) => format!(
            "__FitzValue::Nominal(format!(\"{{}}\", &*({}).lock().unwrap()))",
            code
        ),
        // F13.E — Listas anidadas adentro de heterogéneos. El
        // `code` es `Arc<Mutex<Vec<T>>>`; lockeamos + cloneamos +
        // wrapeamos cada item recursivamente. Bindeamos el `Arc`
        // a una `let` antes del `.lock()` para extender la vida
        // del temporal (paralelo al patrón del show_expr para List).
        Type::List(inner) => {
            let inner_wrap = wrap_as_fitz_value("__it", inner)?;
            format!(
                "__FitzValue::List({{ \
                    let __list = {}; \
                    let __l = __list.lock().unwrap(); \
                    __l.iter().cloned().map(|__it| {}).collect::<Vec<__FitzValue>>() \
                }})",
                code, inner_wrap
            )
        }
        // F13.E — Mapas anidados adentro de heterogéneos.
        Type::Map(kt, vt) => {
            let k_wrap = wrap_as_fitz_value("__k", kt)?;
            let v_wrap = wrap_as_fitz_value("__v", vt)?;
            format!(
                "__FitzValue::Map({{ \
                    let __map = {}; \
                    let __m = __map.lock().unwrap(); \
                    __m.iter().cloned().map(|(__k, __v)| ({}, {})).collect::<Vec<(__FitzValue, __FitzValue)>>() \
                }})",
                code, k_wrap, v_wrap
            )
        }
        // F13.E — `Type::Any` adentro de heterogéneo significa
        // que el item YA es un FitzValue (lista anidada de un mix
        // que ya disparó FitzValue). Pasthrough sin re-wrap.
        Type::Any => code.to_string(),
        _ => {
            return Err(FitzError::new(
                ErrorKind::TypeError,
                0,
                0,
                format!(
                    "F13: el tipo `{}` adentro de un literal heterogéneo \
                     todavía no es soportado en `fitz build` — F13.A+B+E \
                     cubren Int/Float/Str/Bool/Null/Bytes/Nominales/List/Map. \
                     Functions/Tuples/Range/Future en heterogéneos siguen \
                     como follow-up. Workaround: usar `fitz run`.",
                    type_name(ty)
                ),
            ));
        }
    };
    Ok(wrapped)
}

/// F13.A + F13.B — alias para mantener compatibilidad con call sites
/// que pasaban `env`. Tras simplificar el Nominal a capturar Display
/// directo, `wrap_as_fitz_value` no necesita el env, pero los call
/// sites lo pasan por uniformidad — el wrapper ignora el env.
fn wrap_as_fitz_value_with_env(
    code: &str,
    ty: &Type,
    _env: &TypeEnv,
) -> Result<String, FitzError> {
    wrap_as_fitz_value(code, ty)
}

fn rust_type_for(t: &Type, env: &TypeEnv) -> Result<String, FitzError> {
    match t {
        Type::Int => Ok("i64".to_string()),
        Type::Float => Ok("f64".to_string()),
        Type::Str => Ok("String".to_string()),
        Type::Bool => Ok("bool".to_string()),
        Type::Null => Ok("()".to_string()),
        // Mini-tanda Bytes — `Bytes` Fitz → `Vec<u8>` Rust. Clone es
        // O(n) pero correcto. PartialEq directo.
        Type::Bytes => Ok("Vec<u8>".to_string()),
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
            // F13 SPIKE — `List<Any>` se mapea a `Vec<__FitzValue>`
            // (tagged union). El helper `__FitzValue` se emite en
            // el preludio cuando `uses_fitz_value = true`. El caller
            // que decida usar este tipo debe setear el flag (`gen_expr`
            // sobre `Expr::List` heterogéneo lo hace automáticamente).
            // List<T> con T concreto sigue como `Vec<T>` (sin overhead).
            if matches!(**inner, Type::Any) {
                return Ok("Arc<Mutex<Vec<__FitzValue>>>".to_string());
            }
            Ok(format!("Arc<Mutex<Vec<{}>>>", rust_type_for(inner, env)?))
        }
        Type::Map(k, v) => {
            // F13.A — `Map<Any, V>` o `Map<K, Any>` (o ambos) →
            // `Vec<(__FitzValue, __FitzValue)>`. El caller que decida
            // usar este tipo debe setear `uses_fitz_value` (`gen_map_lit`
            // sobre `Map` con keys/values heterogéneos lo hace
            // automáticamente). Mapas homogéneos siguen como
            // `Vec<(K, V)>` (sin overhead).
            if matches!(**k, Type::Any) || matches!(**v, Type::Any) {
                return Ok("Arc<Mutex<Vec<(__FitzValue, __FitzValue)>>>".to_string());
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
        Type::Result { ok: ok_t, err: err_t } => {
            // Mini-tanda Re+ — Result<T, E> tipado. Default Err = Str
            // mantiene compat con código pre-Re+. Cuando el checker
            // infiere E concreto (Int/Instance/etc.), el binding
            // `Err(e)` puede tipar como ese E en lugar de String.
            let ok_rs = if matches!(**ok_t, Type::Any) {
                "_".to_string()
            } else {
                rust_type_for(ok_t, env)?
            };
            let err_rs = match err_t.as_ref() {
                // Default histórico — Str se mapea a String.
                Type::Str => "String".to_string(),
                // Any se mapea a `_` para que rustc lo infiera.
                Type::Any => "_".to_string(),
                other => rust_type_for(other, env)?,
            };
            Ok(format!("Result<{}, {}>", ok_rs, err_rs))
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
            // Mini-tanda Async-cl build — `+ Send` requerido para que
            // `Future<T>` como ret type de un closure async pueda
            // vivir adentro de `Arc<dyn Fn(...) -> Pin<...> + Send +
            // Sync>` (el bound `+ Send + Sync` del Arc<dyn Fn> exige
            // que el Output sea Send). Los `async move` Rust producen
            // futures Send siempre que sus capturas sean Send (que en
            // Fitz post-F17 lo son: Arc/Mutex everywhere).
            Ok(format!(
                "std::pin::Pin<Box<dyn std::future::Future<Output = {}> + Send>>",
                inner_rs
            ))
        }
        // Fase 9.w.2.c — `WsConn<T>` Fitz → `__FitzWsConn<T>` Rust.
        // El struct se emite en `WS_RUNTIME_PRELUDE` cuando
        // `uses_ws = true`. El handler recibe `__FitzWsConn<T>` por
        // valor (move), pero como solo aparece como param de un
        // handler `@ws`, nunca cruza el sitio donde haría falta clone.
        Type::WsConn(inner) => {
            let inner_rs = rust_type_for(inner, env)?;
            Ok(format!("__FitzWsConn<{}>", inner_rs))
        }
        // Tuples (mini-tanda T) → Rust tuple type nativo.
        // `()` (vacía) → `()` (unit). `(T,)` (un slot) → `(T,)`.
        Type::Tuple(items) => {
            if items.is_empty() {
                return Ok("()".to_string());
            }
            let parts: Vec<String> = items
                .iter()
                .map(|t| rust_type_for(t, env))
                .collect::<Result<_, _>>()?;
            if parts.len() == 1 {
                Ok(format!("({},)", parts[0]))
            } else {
                Ok(format!("({})", parts.join(", ")))
            }
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
        Type::Bytes => "Bytes",
        Type::Range => "Range",
        Type::Any => "Any",
        Type::PyAny => "PyAny",
        Type::List(_) => "List<...>",
        Type::Map(_, _) => "Map<...>",
        Type::Result { .. } => "Result<...>",
        Type::Future(_) => "Future<...>",
        Type::WsConn(_) => "WsConn<...>",
        Type::Nullable(_) => "T?",
        Type::Nominal(_) => "<nominal>",
        Type::Function { .. } => "fn(...)",
        Type::Tuple(_) => "(...)",
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
        Type::Bytes => "Bytes".into(),
        Type::Range => "Range".into(),
        Type::Any => "Any".into(),
        Type::PyAny => "PyAny".into(),
        Type::List(inner) => format!("List<{}>", display_type(inner, env)),
        Type::Map(k, v) => format!("Map<{}, {}>", display_type(k, env), display_type(v, env)),
        Type::Result { ok: inner, err: _ } => format!("Result<{}>", display_type(inner, env)),
        Type::Future(inner) => format!("Future<{}>", display_type(inner, env)),
        Type::WsConn(inner) => format!("WsConn<{}>", display_type(inner, env)),
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
        Type::Tuple(items) => {
            let parts: Vec<String> = items.iter().map(|t| display_type(t, env)).collect();
            if parts.len() == 1 {
                format!("({},)", parts[0])
            } else {
                format!("({})", parts.join(", "))
            }
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
        Type::Result { .. } => true,
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

/// Mini-tanda CT — detecta cuando dos tipos primitivos son
/// incompatibles para `==`/`!=`. Llamado en `gen_binop` después de
/// que las ramas estructuradas (Str==Str, num coerce Int↔Float,
/// Nominal==Nominal, Nullable==Null) ya fallaron. El intérprete
/// devuelve `false` sin error para estas combinaciones; el codegen
/// debe alinearse para no producir Rust E0308. Lista exhaustiva
/// sobre primitivos: Int/Float/Str/Bool/Null. Tipos no primitivos
/// (Nominal, List, Map, Function, Any, PyAny, Range, etc.) NO
/// se consideran incompatibles acá — caen al fallback del `==`
/// directo y rustc decide. Any se trata como gradual escape:
/// nunca incompatible.
fn ct_incompatible_eq(lt: &Type, rt: &Type) -> bool {
    use Type::*;
    fn is_primitive(t: &Type) -> bool {
        matches!(t, Int | Float | Str | Bool | Null)
    }
    if !is_primitive(lt) || !is_primitive(rt) {
        return false;
    }
    match (lt, rt) {
        // Mismos primitivos → no incompatibles (manejado upstream).
        (Int, Int) | (Float, Float) | (Str, Str) | (Bool, Bool) | (Null, Null) => false,
        // Int↔Float ya coerciona vía `numeric_coerce`.
        (Int, Float) | (Float, Int) => false,
        // Resto: combinaciones entre distintos primitivos → incompatible.
        _ => true,
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
        // Mini-tanda Bytes — formato `b"..."` paralelo al Display de
        // Value::Bytes. Delegamos al helper `__fitz_fmt_bytes` que se
        // emite en el preludio.
        Type::Bytes => format!("__fitz_fmt_bytes(&({}))", code),
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
        Type::Result { ok: inner, err: _ } => {
            let ok_show = show_expr_inline("__v", inner);
            format!(
                "(match &({}) {{ \
                    Ok(__v) => format!(\"Ok({{}})\", {{ let __v = __v.clone(); {} }}), \
                    Err(__e) => format!(\"Err(\\\"{{}}\\\")\", __e) \
                }})",
                code, ok_show
            )
        }
        // Tuple → `(<inline T1>, <inline T2>, ...)`. Cada componente
        // usa `show_expr_inline` para que los strings vayan entre
        // comillas igual que adentro de listas/mapas — paridad bit-a-bit
        // con `write_inline_value` del intérprete.
        Type::Tuple(items) => {
            let mut s = String::from("{ let __t = ");
            s.push_str(&format!("({}).clone()", code));
            s.push_str("; let mut __s = String::from(\"(\"); ");
            for (i, it_ty) in items.iter().enumerate() {
                if i > 0 {
                    s.push_str("__s.push_str(\", \"); ");
                }
                let pick = format!("(__t.{}).clone()", i);
                let inline = show_expr_inline("__x", it_ty);
                s.push_str(&format!(
                    "{{ let __x = {pick}; __s.push_str(&({inline})); }} "
                ));
            }
            s.push_str("__s.push(')'); __s }");
            s
        }
        // F13 SPIKE — `Type::Any` con código `__FitzValue` (el wrapper
        // que `wrap_as_fitz_value` produce). Display ya está
        // implementado en el preludio, formato bit-a-bit con el
        // intérprete (strings con comillas adentro de colecciones).
        Type::Any => format!("format!(\"{{}}\", {})", code),
        // Range, Function — fallback. Si el AST cuela algo que llega
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

/// Mini-tanda Fm — traduce un `FormatSpec` Fitz a un format spec Rust
/// (el contenido adentro de `{:...}` en `format!`). Devuelve también
/// el código del valor coercionado al tipo que el spec exige.
///
/// Casos no soportados en `fitz build`:
///
/// - `,`/`_` grouping (Rust no tiene equivalente nativo).
/// - `g`/`G` general format.
/// - `c` char codepoint.
/// - `%` percent.
///
/// Para esos, error claro citando `fitz run` como workaround.
fn format_spec_to_rust(
    spec: &crate::ast::FormatSpec,
    code: &str,
    ty: &Type,
) -> Result<(String, String), String> {
    use crate::ast::FormatKind;
    // Mini-tanda Fmt-build — `,`/`_` grouping, `%` percent, y `c`
    // char usan helpers custom emitidos en el preludio. Acá los
    // detectamos y construimos un wrapper que pre-formatea el value
    // como String; el resto del spec (width/align/precision) se
    // aplica encima con `{:>5}` por ejemplo. La señal de "wrapper
    // helper" se hace devolviendo el coerced ya envuelto + kind_str
    // forzado a "" (es un String al final).
    let mut helper_wrapper: Option<String> = None;
    if let Some(grouping_char) = spec.grouping {
        // Grouping requiere Int.
        if !matches!(ty, Type::Int) {
            return Err(format!(
                "format spec con grouping (`,`/`_`) requiere Int, recibió `{}`",
                display_type(ty, &crate::types::TypeEnv::new())
            ));
        }
        helper_wrapper = Some(format!(
            "__fitz_fmt_grouping(({}) as i64, '{}')",
            code, grouping_char
        ));
    }
    let kind_str = match spec.kind {
        None => "",
        Some(FormatKind::String) => "",
        Some(FormatKind::Decimal) => "",
        Some(FormatKind::FixedLower) | Some(FormatKind::FixedUpper) => "",
        Some(FormatKind::Binary) => "b",
        Some(FormatKind::Octal) => "o",
        Some(FormatKind::HexLower) => "x",
        Some(FormatKind::HexUpper) => "X",
        Some(FormatKind::ExponentLower) => "e",
        Some(FormatKind::ExponentUpper) => "E",
        Some(FormatKind::GeneralLower) | Some(FormatKind::GeneralUpper) => {
            // Mini-tanda Fmt-g — `g` lower / `G` upper. El helper
            // decide entre fixed vs exponente según magnitud y
            // precision; quita ceros trailing.
            let coerced_f = match ty {
                Type::Int => format!("(({}) as f64)", code),
                Type::Float => code.to_string(),
                other => {
                    return Err(format!(
                        "format spec `g`/`G` requiere Float o Int, recibió `{}`",
                        display_type(other, &crate::types::TypeEnv::new())
                    ));
                }
            };
            // Default precision Python: 6.
            let precision = spec.precision.unwrap_or(6);
            let upper = matches!(spec.kind, Some(FormatKind::GeneralUpper));
            helper_wrapper = Some(format!(
                "__fitz_fmt_general({}, {}, {})",
                coerced_f, precision, upper
            ));
            ""
        }
        Some(FormatKind::Char) => {
            // Mini-tanda Fmt-build — `c` char: Int → Str (codepoint).
            if !matches!(ty, Type::Int) {
                return Err(format!(
                    "format spec `c` (codepoint) requiere Int, recibió `{}`",
                    display_type(ty, &crate::types::TypeEnv::new())
                ));
            }
            helper_wrapper = Some(format!("__fitz_fmt_char(({}) as i64)", code));
            ""
        }
        Some(FormatKind::Percent) => {
            // Mini-tanda Fmt-build — `%` percent: Float (o Int) →
            // Str con valor multiplicado x100 + sufijo %. Precision
            // del spec se pasa al helper (default 6 paralelo a Python).
            let coerced_f = match ty {
                Type::Int => format!("(({}) as f64)", code),
                Type::Float => code.to_string(),
                other => {
                    return Err(format!(
                        "format spec `%` (percent) requiere Float o Int, recibió `{}`",
                        display_type(other, &crate::types::TypeEnv::new())
                    ));
                }
            };
            let precision = spec.precision.unwrap_or(6);
            helper_wrapper = Some(format!(
                "__fitz_fmt_percent({}, {})",
                coerced_f, precision
            ));
            ""
        }
    };

    // Coerción del value según el kind. Float kinds exigen f64; los
    // otros aceptan el value tal cual.
    let needs_float = matches!(
        spec.kind,
        Some(
            FormatKind::FixedLower
                | FormatKind::FixedUpper
                | FormatKind::ExponentLower
                | FormatKind::ExponentUpper
        )
    );
    let coerced = if needs_float {
        match ty {
            Type::Int => format!("({} as f64)", code),
            Type::Float => code.to_string(),
            other => {
                return Err(format!(
                    "format spec `{}` espera Float o Int, recibió `{}`",
                    spec.kind.unwrap().to_char(),
                    display_type(other, &crate::types::TypeEnv::new())
                ));
            }
        }
    } else {
        code.to_string()
    };

    // Armado del rust spec.
    let mut out = String::new();
    // Mini-tanda Fmt-build — cuando hay helper_wrapper, el resultado
    // del helper ya es un String con todo aplicado (grouping ya
    // formateado, precision ya consumida por el `%`, kind por `c`).
    // El único spec extra Rust acepta encima es fill/align/width.
    // Si NO hay wrapper, comportamiento Fm clásico.
    if let Some(wrapped) = helper_wrapper {
        if let (Some(fill), Some(align)) = (spec.fill, spec.align) {
            out.push(fill);
            out.push(align.to_char());
        } else if let Some(align) = spec.align {
            out.push(align.to_char());
        }
        if let Some(w) = spec.width {
            out.push_str(&w.to_string());
        }
        // Ignoramos sign/alternate/zero_pad/precision/kind — el helper
        // ya hizo lo suyo. El coerced final es el wrapper (String).
        return Ok((format!(":{}", out), wrapped));
    }
    if let (Some(fill), Some(align)) = (spec.fill, spec.align) {
        out.push(fill);
        out.push(align.to_char());
    } else if let Some(align) = spec.align {
        out.push(align.to_char());
    }
    if let Some(sign) = spec.sign {
        match sign {
            crate::ast::FormatSign::Plus => out.push('+'),
            crate::ast::FormatSign::Space => out.push(' '),
            crate::ast::FormatSign::Minus => {}
        }
    }
    if spec.alternate {
        out.push('#');
    }
    if spec.zero_pad {
        out.push('0');
    }
    if let Some(w) = spec.width {
        out.push_str(&w.to_string());
    }
    if let Some(p) = spec.precision {
        out.push_str(&format!(".{}", p));
    }
    if !kind_str.is_empty() {
        out.push_str(kind_str);
    }
    Ok((format!(":{}", out), coerced))
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

/// Mini-tanda Lt — predicado: el pattern es "puro irrefutable",
/// usable en `let pat = value` Rust directo. Solo Ident/Wildcard/
/// Tuple recursivamente. Patterns ricos (literal, range, Or, Ok,
/// Err, OkBinding, ErrBinding, OkWildcard, ErrWildcard) son
/// refutables y requieren el camino `match` wrapper.
fn pattern_is_pure_irrefutable(pat: &crate::ast::Pattern) -> bool {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(_) | Pattern::Wildcard => true,
        Pattern::Tuple(subs) => subs.iter().all(pattern_is_pure_irrefutable),
        _ => false,
    }
}

/// Mini-tanda Lt — recolecta los nombres de los bindings que el
/// pattern introduce al matchear. Ident/OkBinding/ErrBinding aportan
/// un nombre; Tuple recursa; el resto (literales, ranges, Or,
/// wildcards) no aportan nombres.
fn collect_pattern_bindings(pat: &crate::ast::Pattern, out: &mut Vec<String>) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(n) | Pattern::OkBinding(n) | Pattern::ErrBinding(n) => {
            out.push(n.clone());
        }
        Pattern::Tuple(subs) => {
            for s in subs {
                collect_pattern_bindings(s, out);
            }
        }
        _ => {}
    }
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
        let toml = cargo_toml_for("foo", false, true, false, false, false);
        assert!(toml.contains("tokio"), "esperaba tokio en deps");
        assert!(toml.contains("\"time\""), "esperaba feature `time`");
        assert!(!toml.contains("axum"), "no debería incluir axum");
    }

    #[test]
    fn cargo_toml_async_con_http_incluye_tokio_time_y_axum() {
        let toml = cargo_toml_for("foo", true, true, false, false, false);
        assert!(toml.contains("axum"));
        assert!(toml.contains("\"time\""));
        assert!(toml.contains("\"macros\""));
    }

    #[test]
    fn cargo_toml_sin_async_sin_http_es_minimal() {
        let toml = cargo_toml_for("foo", false, false, false, false, false);
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
        let toml = cargo_toml_for("foo", false, false, true, false, false);
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
        let toml = cargo_toml_for("foo", true, false, true, false, false);
        assert!(toml.contains("axum"));
        assert!(toml.contains("pyo3"));
        assert!(toml.contains("tokio"));
    }

    #[test]
    fn cargo_toml_sin_python_no_incluye_pyo3() {
        let toml = cargo_toml_for("foo", true, false, false, false, false);
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
    fn list_literal_heterogeneo_emite_fitz_value() {
        // F13 SPIKE — `[1, "dos"]` heterogéneo ya no es error;
        // emite `Vec<__FitzValue>` con cada item envuelto en su
        // variante (`__FitzValue::Int(1)`, `__FitzValue::Str("dos")`).
        // Antes del SPIKE el codegen abortaba "homogénea requerida".
        let code = gen("let xs = [1, \"dos\"]").unwrap();
        assert!(
            code.contains("Vec<__FitzValue>"),
            "esperaba tipo `Vec<__FitzValue>` para lista heterogénea, código:\n{}",
            code
        );
        assert!(
            code.contains("__FitzValue::Int(1i64)"),
            "esperaba wrap `__FitzValue::Int(1i64)` para Int en lista heterogénea"
        );
        assert!(
            code.contains("__FitzValue::Str(String::from"),
            "esperaba wrap `__FitzValue::Str(...)` para Str en lista heterogénea"
        );
    }

    #[test]
    fn f13_spike_preludio_emite_fitz_value_enum() {
        // F13 SPIKE — el enum `__FitzValue` aparece en el preludio
        // cuando el programa tiene listas heterogéneas.
        let code = gen("let xs = [1, \"dos\", true]").unwrap();
        assert!(
            code.contains("enum __FitzValue"),
            "esperaba definición de `enum __FitzValue` en el preludio"
        );
        assert!(
            code.contains("impl PartialEq for __FitzValue"),
            "esperaba `impl PartialEq for __FitzValue`"
        );
        assert!(
            code.contains("impl std::fmt::Display for __FitzValue"),
            "esperaba `impl Display for __FitzValue`"
        );
    }

    #[test]
    fn f13_spike_lista_homogenea_no_emite_fitz_value() {
        // Sanity: listas homogéneas siguen sin emitir el enum
        // (cero overhead para el 90% del caso).
        let code = gen("let xs = [1, 2, 3]").unwrap();
        assert!(
            !code.contains("enum __FitzValue"),
            "lista homogénea NO debe gatillar emisión de `__FitzValue`"
        );
    }

    // ---- F13.A — Bytes y Map heterogéneo ----

    #[test]
    fn f13_a_bytes_en_lista_heterogenea_se_envuelve() {
        // F13.A — Bytes adentro de lista heterogénea se envuelve
        // como `__FitzValue::Bytes(_)`.
        let code = gen("let xs = [1, b\"raw\"]").unwrap();
        assert!(
            code.contains("__FitzValue::Bytes("),
            "esperaba wrap `__FitzValue::Bytes(...)` para Bytes en heterogéneo"
        );
    }

    #[test]
    fn f13_a_map_heterogeneo_emite_vec_fv_fv() {
        // F13.A — Map<Str, Any> emite Vec<(FV, FV)>: ambos lados
        // wrapeados (incluso el lado homogéneo se wrappea para
        // uniformar el tipo).
        let code = gen("let m = {\"a\": 1, \"b\": \"x\"}").unwrap();
        assert!(
            code.contains("Vec<(__FitzValue, __FitzValue)>"),
            "esperaba Vec<(__FitzValue, __FitzValue)> para mapa heterogéneo"
        );
        assert!(
            code.contains("__FitzValue::Str(String::from(\"a\"))"),
            "esperaba wrap del lado homogéneo de keys"
        );
    }

    #[test]
    fn f13_a_mapa_homogeneo_no_emite_fitz_value() {
        // Sanity: mapas homogéneos siguen sin overhead.
        let code = gen("let m = {\"a\": 1, \"b\": 2}").unwrap();
        assert!(
            !code.contains("__FitzValue"),
            "mapa homogéneo NO debe gatillar emisión de `__FitzValue`"
        );
    }

    // ---- F13.B — Nominales en heterogéneos ----

    #[test]
    fn f13_b_nominal_en_lista_heterogenea_captura_display() {
        // F13.B — Nominal adentro de lista heterogénea se captura
        // como String via Display del Data.
        let code = gen(
            "type User { id: Int }\n\
             let u = User { id: 1 }\n\
             let xs = [u, 42]",
        )
        .unwrap();
        assert!(
            code.contains("__FitzValue::Nominal(format!"),
            "esperaba wrap `__FitzValue::Nominal(format!(...))` para Nominal en heterogéneo"
        );
        assert!(
            code.contains(".lock().unwrap()"),
            "esperaba lock del Arc<Mutex<UserData>> antes de Display"
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
    fn map_literal_valores_heterogeneos_emite_fitz_value() {
        // F13.A — `{"a": 1, "b": "x"}` con values heterogéneos ya no
        // es error; emite Vec<(__FitzValue, __FitzValue)> con cada
        // par envuelto. Antes del F13.A el codegen abortaba "valores
        // homogéneos requeridos".
        let code = gen("let m = {\"a\": 1, \"b\": \"x\"}").unwrap();
        assert!(
            code.contains("Vec<(__FitzValue, __FitzValue)>"),
            "esperaba tipo `Vec<(__FitzValue, __FitzValue)>` para mapa heterogéneo, código:\n{}",
            code
        );
        assert!(
            code.contains("__FitzValue::Int(1i64)"),
            "esperaba wrap `__FitzValue::Int(1i64)` para value Int"
        );
    }

    #[test]
    fn list_indexing_emite_borrow_clone() {
        // I.1 (mini-tanda I): el indexing ahora emite un bloque
        // con bounds check + wrap negativo + clone. Verificamos
        // que las piezas clave estén presentes.
        let file = ast_test::parse(
            &gen("let xs: List<Int> = [10, 20]\nlet x = xs[0]").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        // El binding `x` debe quedar tipado i64 (List<Int> indexing).
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        let init = ast_test::local_init_expr(l).unwrap();
        let ts = ast_test::ts(init);
        // El nuevo emit usa `lock().unwrap()` adentro del bloque.
        assert!(ts.contains("lock"), "esperaba .lock(), fue: {}", ts);
        // Y el `clone()` final del elemento.
        assert!(ts.contains("clone"), "esperaba .clone(), fue: {}", ts);
        // Verificamos el wrap negativo (signature del nuevo emit).
        assert!(
            ts.contains("__len + __i"),
            "esperaba wrap negativo `__len + __i`, fue: {}",
            ts
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
        // Mini-tanda UC: el extractor pasó de `axum::Json<serde_json::Value>`
        // a `axum::body::Bytes` para poder dispatchar por Content-Type
        // (JSON vs urlencoded vs 415) adentro del wrapper.
        let src = "type Input { msg: Str }\n\
                   @post(\"/echo\") fn echo(body: Input) -> Input => body";
        let code = gen(src).unwrap();
        let file = ast_test::parse(&code);
        let wrapper =
            ast_test::find_item_fn(&file, "__handler_echo").expect("falta __handler_echo");
        let pats_tys = ast_test::fn_param_pats_and_types(wrapper);
        assert!(
            pats_tys.iter().any(|(p, t)| {
                p.contains("body_body_bytes")
                    && t.contains("axum :: body :: Bytes")
            }),
            "esperaba extractor body_body_bytes: axum::body::Bytes, got: {:?}",
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
    fn uc_http_body_extrae_bytes_no_json() {
        // Mini-tanda UC: confirmamos que el extractor del body es
        // Bytes, no Json<Value>. Esto es lo que habilita el dispatch
        // por Content-Type adentro del wrapper.
        let src = "type Input { msg: Str }\n\
                   @post(\"/echo\") fn echo(body: Input) -> Input => body";
        let code = gen(src).unwrap();
        assert!(
            code.contains("body_body_bytes: axum::body::Bytes"),
            "esperaba extractor `body_body_bytes: axum::body::Bytes`, no se encontró"
        );
        assert!(
            !code.contains("axum::Json(body_raw):"),
            "no esperaba el viejo extractor `axum::Json(body_raw): axum::Json<serde_json::Value>`"
        );
    }

    #[test]
    fn uc_http_body_dispatch_por_content_type() {
        // Mini-tanda UC: el wrapper computa ct_primary y dispatcha
        // entre JSON, urlencoded y 415.
        let src = "type Input { msg: Str }\n\
                   @post(\"/echo\") fn echo(body: Input) -> Input => body";
        let code = gen(src).unwrap();
        assert!(
            code.contains("body_ct_primary"),
            "esperaba bind `body_ct_primary` para Content-Type"
        );
        assert!(
            code.contains("\"application/json\""),
            "esperaba branch para `application/json`"
        );
        assert!(
            code.contains("\"application/x-www-form-urlencoded\""),
            "esperaba branch para urlencoded"
        );
        assert!(
            code.contains("__parse_urlencoded"),
            "esperaba llamada a `__parse_urlencoded` para urlencoded"
        );
        assert!(
            code.contains("UNSUPPORTED_MEDIA_TYPE"),
            "esperaba 415 para Content-Type no soportado"
        );
    }

    #[test]
    fn uc_http_body_415_msg_matchea_interprete() {
        // Mini-tanda HA + MP-Build — el msg del 415 del codegen
        // contiene las frases clave que el usuario espera ver, y
        // cita los 3 CTs soportados (JSON, urlencoded, multipart)
        // tal cual el intérprete (`http::handle_task`).
        let src = "type Input { msg: Str }\n\
                   @post(\"/echo\") fn echo(body: Input) -> Input => body";
        let code = gen(src).unwrap();
        let key_phrases = [
            "Content-Type no soportado",
            "application/json",
            "application/x-www-form-urlencoded",
            "multipart/form-data",
            "sub-paso futuro",
        ];
        for phrase in &key_phrases {
            assert!(
                code.contains(phrase),
                "esperaba que el msg del 415 contenga `{}`, no se encontró",
                phrase
            );
        }
    }

    #[test]
    fn mp_build_codegen_emite_helpers_multipart() {
        // Mini-tanda MP-Build — los helpers `__parse_multipart` y
        // `__extract_multipart_boundary` se emiten en el preludio
        // junto con los de urlencoded.
        let src = "@get(\"/\") fn index() -> Str => \"ok\"";
        let code = gen(src).unwrap();
        assert!(
            code.contains("fn __parse_multipart(bytes: &[u8]"),
            "esperaba helper `__parse_multipart` en el preludio HTTP"
        );
        assert!(
            code.contains("fn __extract_multipart_boundary("),
            "esperaba helper `__extract_multipart_boundary` en el preludio HTTP"
        );
    }

    #[test]
    fn mp_build_codegen_dispatch_incluye_multipart_branch() {
        // El wrapper del handler con body ahora dispatcha entre
        // JSON, urlencoded y multipart (3 branches), con 415 al final.
        let src = "type Input { msg: Str }\n\
                   @post(\"/echo\") fn echo(body: Input) -> Input => body";
        let code = gen(src).unwrap();
        assert!(
            code.contains("\"multipart/form-data\""),
            "esperaba branch para multipart en el dispatch"
        );
        assert!(
            code.contains("__parse_multipart"),
            "esperaba llamada a `__parse_multipart` en el dispatch"
        );
    }

    #[test]
    fn uc_http_preludio_emite_helpers_urlencoded() {
        // Los helpers `__parse_urlencoded` y `__url_decode` se emiten
        // siempre que haya rutas HTTP — son parte del preludio.
        let src = "@get(\"/\") fn index() -> Str => \"ok\"";
        let code = gen(src).unwrap();
        assert!(
            code.contains("fn __parse_urlencoded(bytes: &[u8])"),
            "esperaba helper `__parse_urlencoded` en el preludio HTTP"
        );
        assert!(
            code.contains("fn __url_decode(s: &str)"),
            "esperaba helper `__url_decode` en el preludio HTTP"
        );
    }

    #[test]
    fn uc_http_body_fuerza_hmap_extraction() {
        // Mini-tanda UC: cuando hay body_param, fuerza la extracción
        // del HeaderMap (lo necesitamos para leer Content-Type) aun
        // sin @header / middleware / cors.
        let src = "type Input { msg: Str }\n\
                   @post(\"/echo\") fn echo(body: Input) -> Input => body";
        let code = gen(src).unwrap();
        assert!(
            code.contains("__hmap: axum::http::HeaderMap"),
            "esperaba que se extraiga el HeaderMap cuando hay body_param"
        );
    }

    // ---- Mini-tanda DZ — división por cero con msg alineado al intérprete ----

    #[test]
    fn dz_div_int_emite_check_de_cero() {
        // `a / b` para Int emite un bloque con check explícito de 0
        // que panica con "división por cero" — paralelo a `eval_div`
        // del intérprete y antes de que rustc rechace `10/0` literal
        // con `unconditional_panic`.
        let src = "let x = 10 / 2\nprint(x)";
        let code = gen(src).unwrap();
        assert!(
            code.contains("__b == 0") && code.contains("división por cero"),
            "esperaba check `__b == 0` + panic `división por cero`, got:\n{}",
            code
        );
    }

    #[test]
    fn dz_div_float_emite_check_de_cero_float() {
        // Float division por 0.0 también chequea — sin este wrap,
        // rustc emite `inf`/`NaN` silencioso.
        let src = "let x = 10.0 / 2.0\nprint(x)";
        let code = gen(src).unwrap();
        assert!(
            code.contains("__b == 0.0") && code.contains("división por cero"),
            "esperaba check `__b == 0.0` + panic, got:\n{}",
            code
        );
    }

    #[test]
    fn dz_division_literal_por_cero_compila_aunque_paniquea_en_runtime() {
        // El wrap del check de cero evita que rustc rechace
        // `10 / 0` con `unconditional_panic`. El programa compila;
        // el panic ocurre en runtime con el msg alineado.
        let src = "print(10 / 0)";
        let code = gen(src).unwrap();
        assert!(
            code.contains("división por cero"),
            "esperaba panic msg `división por cero` en el output, got:\n{}",
            code
        );
    }

    // ---- Mini-tanda CT — comparar tipos distintos: codegen emite literal ----

    #[test]
    fn ct_int_vs_str_eq_emite_false_literal() {
        // `1 == "1"` en el intérprete devuelve false; el codegen
        // debe alinearse y emitir false literal en lugar de un Rust
        // `==` entre tipos distintos (E0308).
        let src = "print(1 == \"1\")";
        let code = gen(src).unwrap();
        // Esperamos el patrón `{ let _ = ...; let _ = ...; false }`.
        assert!(
            code.contains("let _ = ") && code.contains("false }"),
            "esperaba wrap CT con `let _` + literal false, got:\n{}",
            code
        );
    }

    #[test]
    fn ct_int_vs_str_neq_emite_true_literal() {
        let src = "print(1 != \"1\")";
        let code = gen(src).unwrap();
        assert!(
            code.contains("let _ = ") && code.contains("true }"),
            "esperaba wrap CT con `let _` + literal true para `!=`, got:\n{}",
            code
        );
    }

    #[test]
    fn ct_bool_vs_int_eq_emite_false_literal() {
        let src = "print(true == 1)";
        let code = gen(src).unwrap();
        assert!(
            code.contains("let _ = ") && code.contains("false }"),
            "esperaba wrap CT con `let _` + literal false, got:\n{}",
            code
        );
    }

    #[test]
    fn ct_str_vs_null_eq_emite_false_literal() {
        // `"x" == null` (Str no es Nullable, así que no cae al
        // `is_none/is_some` path) → CT incompatible → false.
        let src = "print(\"x\" == null)";
        let code = gen(src).unwrap();
        assert!(
            code.contains("let _ = ") && code.contains("false }"),
            "esperaba wrap CT con `let _` + literal false, got:\n{}",
            code
        );
    }

    #[test]
    fn ct_str_eq_str_sigue_emitiendo_comparacion_normal() {
        // Para Str==Str (mismo tipo) NO aplica el wrap CT — debe
        // emitir `==` directo entre &String.
        let src = "print(\"a\" == \"a\")";
        let code = gen(src).unwrap();
        assert!(
            !code.contains("let _ ="),
            "Str==Str no debe disparar el wrap CT, got:\n{}",
            code
        );
    }

    #[test]
    fn ct_int_eq_float_sigue_coercionando_no_dispara_wrap() {
        // Int↔Float coerciona vía numeric_coerce, NO es incompatible.
        let src = "print(1 == 1.0)";
        let code = gen(src).unwrap();
        assert!(
            !code.contains("let _ ="),
            "Int==Float coerce, no debe disparar el wrap CT, got:\n{}",
            code
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
        let (env, types_info, _defs, errs) = crate::types::check_program(&program);
        assert!(errs.is_empty(), "checker errors: {:?}", errs);
        let project = generate_project(Path::new("test.fitz"), &program, &env, &types_info, crate::manifest::DepRegistry::new()).unwrap();
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
        let (env, types_info, _defs, errs) = crate::types::check_program(&program);
        assert!(errs.is_empty());
        let project = generate_project(Path::new("test.fitz"), &program, &env, &types_info, crate::manifest::DepRegistry::new()).unwrap();
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
    fn err_con_no_str_emite_value_directo_post_re_plus() {
        // Mini-tanda Re+ — `Err(42)` ya NO coerce a String. Emit
        // `Err(42i64)` directo y el tipo del Result es
        // `Result<T, i64>` con E concreto. Cambio respecto al
        // comportamiento pre-Re+ donde se hacía `format!("{}", 42)`.
        let code = gen("fn boom() -> Result<Str, Int> { return Err(42) }").unwrap();
        let file = ast_test::parse(&code);
        let boom = ast_test::find_item_fn(&file, "boom").expect("falta fn boom");
        assert!(
            ast_test::fn_body_returns_any_matching(boom, &["Err", "42i64"]),
            "esperaba Err(42i64) directo sin format!, body:\n{}",
            ast_test::fn_body_text(boom)
        );
        assert!(
            !ast_test::fn_body_text(boom).contains("format !"),
            "NO debería haber format! ya que el Err es Int (no Str): {}",
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
        generate_module_rs_with_bindings(&program, &env, &HashMap::new(), &[])
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
    fn modulo_top_level_acepta_expr_const_eval_como_pub_const() {
        // F14: una RHS const-eval-able (BinOp aritmético sobre literales)
        // a nivel top de módulo ahora se acepta y se emite como `pub const`.
        let code = gen_module("let X = 1 + 1").unwrap();
        let file = ast_test::parse(&code);
        let x = ast_test::find_item_const(&file, "X").expect("falta const X");
        assert!(ast_test::vis_is_pub(&x.vis), "esperaba `pub const X`");
        assert_eq!(ast_test::ts(&*x.ty), "i64", "esperaba tipo i64 para X");
    }

    #[test]
    fn modulo_top_level_acepta_expr_no_const_como_pub_fn() {
        // F14: una RHS no const-eval (call a fn, field access, etc.) a
        // nivel top de módulo se emite como accessor fn `pub fn X() -> T`.
        let code = gen_module(
            "fn make() -> Int => 42\nlet X: Int = make()",
        )
        .unwrap();
        let file = ast_test::parse(&code);
        let x = ast_test::find_item_fn(&file, "X").expect("falta fn X");
        assert!(ast_test::vis_is_pub(&x.vis), "esperaba `pub fn X`");
        assert_eq!(
            ast_test::fn_return_type(x).as_deref(),
            Some("i64"),
            "esperaba return type i64 para X()"
        );
    }

    // ---- Mini-tanda Lt — let-destructure con sub-patterns ricos ----

    #[test]
    fn lt_let_pure_irrefutable_emite_path_directo() {
        // Caso clásico `let (a, b) = ...`: el codegen NO usa match
        // wrapper. Emite `let (a, b) = ...;` directo (path pre-Lt).
        let code = gen("let (a, b) = (1, 2)\nprint(a)\nprint(b)\n").unwrap();
        assert!(
            !code.contains("__destr_scrut"),
            "esperaba path puro (sin __destr_scrut) para Ident/Tuple, got:\n{}",
            code
        );
        assert!(
            code.contains("let (a, b)"),
            "esperaba `let (a, b)` directo, got:\n{}",
            code
        );
    }

    #[test]
    fn lt_let_literal_int_subpattern_emite_match_wrapper() {
        // `let (1, x) = ...`: refutable → match wrapper con catch-all
        // panic. El nombre `x` queda declarado en el scope outer.
        let code = gen("let (1, x) = (1, 42)\nprint(x)\n").unwrap();
        assert!(
            code.contains("__destr_scrut"),
            "esperaba match wrapper (con __destr_scrut), got:\n{}",
            code
        );
        assert!(
            code.contains("panic!(\"destructuring no matcheó"),
            "esperaba catch-all panic, got:\n{}",
            code
        );
        assert!(
            code.contains("(1i64, x)"),
            "esperaba pattern `(1i64, x)`, got:\n{}",
            code
        );
    }

    #[test]
    fn lt_let_ok_binding_subpattern_extrae_resultado() {
        // `let (Ok(v), tag) = ...`: bindings = [v, tag].
        let code = gen(
            "let (Ok(v), tag) = (Ok(99), \"result\")\nprint(v)\nprint(tag)\n",
        )
        .unwrap();
        assert!(
            code.contains("__destr_scrut"),
            "esperaba match wrapper, got:\n{}",
            code
        );
        assert!(
            code.contains("(Ok(v), tag)"),
            "esperaba pattern `(Ok(v), tag)`, got:\n{}",
            code
        );
        // Los dos bindings se emiten en el tuple de retorno del brazo.
        assert!(
            code.contains("(v, tag)"),
            "esperaba retorno `(v, tag)`, got:\n{}",
            code
        );
    }

    #[test]
    fn lt_let_str_literal_subpattern_usa_guard_inline() {
        // `let ("ada", n) = ...`: el Str literal genera un guard
        // `__s_X.as_str() == "ada"` en el brazo del match.
        let code = gen(
            "let (\"ada\", n) = (\"ada\", 7)\nprint(n)\n",
        )
        .unwrap();
        assert!(
            code.contains("__destr_scrut"),
            "esperaba match wrapper, got:\n{}",
            code
        );
        assert!(
            code.contains("if __s_") && code.contains(".as_str() == \"ada\""),
            "esperaba guard sobre __s_X.as_str(), got:\n{}",
            code
        );
    }

    #[test]
    fn lt_let_range_subpattern_usa_guard_contains() {
        // `let (0..100, y) = ...`: Range emite guard `(0..100).contains(&__n_X)`.
        let code = gen(
            "let (0..100, y) = (50, \"yes\")\nprint(y)\n",
        )
        .unwrap();
        assert!(
            code.contains(".contains(&__n_"),
            "esperaba guard `(0..100).contains(&__n_X)`, got:\n{}",
            code
        );
    }

    #[test]
    fn lt_let_single_binding_no_emite_paren() {
        // Con un solo binding, el `let mut <name> = match ...` evita
        // la tupla degenerada `(x,)` que requeriría un trailing comma.
        let code = gen("let (1, x) = (1, 42)\nprint(x)\n").unwrap();
        assert!(
            code.contains("let mut x = match"),
            "esperaba `let mut x = match ...` (sin paréntesis), got:\n{}",
            code
        );
    }

    // ---- Mini-tanda El — Err(List<T>) / Err(Map<K,V>) en codegen ----

    #[test]
    fn el_err_list_se_emite_sin_coercion_a_string() {
        // Pre-El: `Err([1,2,3])` con `Result<Int, List<Int>>` se
        // rechazaba con error de codegen. Post-El: emite `Err(<list>)`
        // directo con el `Arc<Mutex<Vec<i64>>>` intacto, sin coerción
        // a String.
        let code = gen(
            "fn fail() -> Result<Int, List<Int>> {\n\
                 return Err([1, 2, 3])\n\
             }\n\
             fail()",
        )
        .unwrap();
        assert!(
            !code.contains("Err(format!"),
            "esperaba que el List se emita directo, no via format!; got:\n{}",
            code
        );
        assert!(
            code.contains("Err(Arc::new(Mutex::new(vec!["),
            "esperaba `Err(Arc::new(Mutex::new(vec![...])))`, got:\n{}",
            code
        );
        assert!(
            code.contains("Result<i64, Arc<Mutex<Vec<i64>>>>"),
            "esperaba return type `Result<i64, Arc<Mutex<Vec<i64>>>>`, got:\n{}",
            code
        );
    }

    #[test]
    fn el_err_map_se_emite_directo() {
        let code = gen(
            "fn fail() -> Result<Int, Map<Str, Int>> {\n\
                 return Err({\"a\": 1, \"b\": 2})\n\
             }\n\
             fail()",
        )
        .unwrap();
        assert!(
            !code.contains("Err(format!"),
            "Map no debería ir por format!; got:\n{}",
            code
        );
        assert!(
            code.contains("Result<i64, Arc<Mutex<Vec<(String, i64)>>>>"),
            "esperaba return type `Result<i64, Map>`, got:\n{}",
            code
        );
    }

    #[test]
    fn f15_module_loader_acepta_imports_transitivos_en_modulo() {
        // F15: un módulo con su propio `import` ya no se rechaza al
        // codegen-time. El test usa `generate_project` indirectamente
        // via la suite e2e; acá solo validamos a nivel unit que el
        // `gen_module` (que NO toma loader) sigue funcionando con
        // imports stmts en el AST — los stmts se ignoran (ya los
        // procesó el loader).
        let code = gen_module(
            "from segundo import dos\nfn x() -> Int => dos()",
        );
        assert!(
            code.is_err(),
            "gen_module sin loader: el cuerpo de `x` referencia `dos` \
             que no está en scope; debe ser error"
        );
        // El mensaje NO debería citar 5b.5 ni "imports transitivos".
        let msg = code.unwrap_err().message;
        assert!(
            !msg.contains("transitivos") && !msg.contains("5b.5"),
            "el error ya no debe citar deuda transitiva post-F15; fue: {}",
            msg
        );
    }

    #[test]
    fn modulo_top_level_str_concat_se_emite_como_pub_fn() {
        // F14: `let X = "a" + "b"` no es const-eval (Rust no acepta
        // `String + String` en const) → accessor fn `pub fn X() -> String`.
        let code = gen_module(
            "let GREETING: Str = \"hola, \" + \"Fitz\"",
        )
        .unwrap();
        let file = ast_test::parse(&code);
        let greeting = ast_test::find_item_fn(&file, "GREETING")
            .expect("falta fn GREETING (esperaba accessor para Str concat)");
        assert!(ast_test::vis_is_pub(&greeting.vis), "esperaba `pub fn GREETING`");
        assert_eq!(
            ast_test::fn_return_type(greeting).as_deref(),
            Some("String"),
            "esperaba return type String para GREETING()"
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

    // ---- Mini-tanda Cd ----
    //
    // HO callbacks: pasar fn nombrada (`xs.map(double)`) y F12 fix:
    // hoist de `let X = <const-eval>` top-level a `const X` cuando
    // alguna fn top-level lo referencia.

    #[test]
    fn cd_ho_pasar_fn_nombrada_a_map() {
        let src = "fn double(n: Int) -> Int { return n * 2 }\n\
                   let xs: List<Int> = [1, 2, 3]\n\
                   let ys: List<Int> = xs.map(double)";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        // El callback se emite como referencia a la fn nombrada `double`.
        assert!(
            code.contains(". map (double)") || code.contains(".map(double)"),
            "esperaba `.map(double)` con fn nombrada, got:\n{}",
            code
        );
    }

    #[test]
    fn cd_ho_pasar_fn_nombrada_a_filter() {
        let src = "fn is_even(n: Int) -> Bool { return n % 2 == 0 }\n\
                   let xs: List<Int> = [1, 2, 3, 4]\n\
                   let ys: List<Int> = xs.filter(is_even)";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        assert!(
            code.contains("let __cb = is_even"),
            "esperaba `let __cb = is_even`, got:\n{}",
            code
        );
    }

    #[test]
    fn cd_ho_pasar_fn_nombrada_a_reduce() {
        // Callback binario: `xs.reduce(0, sumar)`.
        let src = "fn sumar(acc: Int, x: Int) -> Int { return acc + x }\n\
                   let xs: List<Int> = [1, 2, 3]\n\
                   let total: Int = xs.reduce(0, sumar)";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        assert!(
            code.contains("let __cb = sumar"),
            "esperaba `let __cb = sumar` (callback binario nombrado), got:\n{}",
            code
        );
    }

    // Los siguientes 3 casos los **detecta el checker** ANTES del codegen
    // (las firmas de las fns nombradas tipan como `Function { params, ret }`
    // y la validación del callback usa `check_unary_callback`). Los tests
    // verifican que el pipeline completo aborta con mensaje claro — el
    // codegen ni siquiera llega a correr porque el checker corta antes.

    #[test]
    fn cd_ho_fn_inexistente_aborta_en_checker() {
        let src = "let xs: List<Int> = [1, 2, 3]\n\
                   let ys: List<Int> = xs.map(no_existe)";
        // El checker detecta `no_existe` como variable desconocida.
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (_env, _types, _defs, errors) = check_program(&program);
        assert!(
            !errors.is_empty()
                && errors.iter().any(|e| e.message.contains("no_existe")),
            "esperaba error del checker sobre `no_existe`, fue: {:?}",
            errors
        );
    }

    #[test]
    fn cd_ho_fn_nombrada_con_aridad_incorrecta_aborta_en_checker() {
        let src = "fn binaria(a: Int, b: Int) -> Int { return a + b }\n\
                   let xs: List<Int> = [1, 2, 3]\n\
                   let ys: List<Int> = xs.map(binaria)";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (_env, _types, _defs, errors) = check_program(&program);
        assert!(
            !errors.is_empty()
                && errors.iter().any(|e| e.message.contains("1 ar")),
            "esperaba error del checker sobre aridad, fue: {:?}",
            errors
        );
    }

    #[test]
    fn cd_ho_fn_nombrada_con_ret_incompatible_aborta_en_checker() {
        let src = "fn to_str(n: Int) -> Str { return \"{n}\" }\n\
                   let xs: List<Int> = [1, 2, 3]\n\
                   let ys: List<Int> = xs.filter(to_str)";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (_env, _types, _defs, errors) = check_program(&program);
        assert!(
            !errors.is_empty()
                && errors.iter().any(|e| e.message.contains("Bool")),
            "esperaba error del checker sobre ret type, fue: {:?}",
            errors
        );
    }

    #[test]
    fn cd_f12_let_int_const_referenciado_por_fn_se_hoistea() {
        let src = "let MAX = 100\n\
                   fn cap(n: Int) -> Int {\n\
                       if (n > MAX) { return MAX }\n\
                       return n\n\
                   }\n\
                   print(cap(50))\n\
                   print(cap(200))";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        // El hoist emite `const MAX: i64 = 100;` antes de fns.
        assert!(
            code.contains("const MAX : i64 = 100") || code.contains("const MAX: i64 = 100"),
            "esperaba `const MAX: i64 = 100;` hoisteado, got:\n{}",
            code
        );
        // La fn body referencia MAX directo (con o sin paréntesis del if).
        assert!(
            code.contains("n > MAX") && code.contains("return MAX"),
            "esperaba la fn `cap` referenciando MAX, got:\n{}",
            code
        );
    }

    #[test]
    fn cd_f12_let_str_referenciado_por_fn_se_hoistea_a_static() {
        let src = "let GREETING = \"hola\"\n\
                   fn greet(name: Str) -> Str { return \"{GREETING}, {name}\" }\n\
                   print(greet(\"Ada\"))";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        assert!(
            code.contains("static GREETING : & str") || code.contains("static GREETING: &str"),
            "esperaba `static GREETING: &str = ...;` hoisteado, got:\n{}",
            code
        );
    }

    #[test]
    fn cd_f12_let_no_referenciado_por_fn_no_se_hoistea() {
        // Si nadie lo referencia, queda como local de main(); no hoist.
        let src = "let X = 42\nprint(X)";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        // NO debería haber `const X: i64 = 42;` top-level.
        assert!(
            !code.contains("const X : i64 = 42") && !code.contains("const X: i64 = 42"),
            "X no debería hoistar (no es referenciado por fn), got:\n{}",
            code
        );
        // Sigue como let local.
        assert!(
            code.contains("let mut X : i64 = 42") || code.contains("let mut X: i64"),
            "X debería seguir como local de main(), got:\n{}",
            code
        );
    }

    #[test]
    fn cd_f12_let_reasignado_no_se_hoistea() {
        // Reasignación rompe el hoist (const Rust no se puede mutar).
        let src = "let X = 10\nX = 20\nfn read() -> Int { return X }\nprint(read())";
        let err = gen(src).expect_err("esperaba error de codegen (X no hoisteable, reasignado)");
        assert!(
            err.message.contains("desconocida") && err.message.contains("X"),
            "esperaba error sobre `X` desconocida (no hoisteada), fue: {}",
            err.message
        );
    }

    #[test]
    fn cd_f12_const_eval_con_binop_se_hoistea() {
        let src = "let LIMIT = 10 * 2 + 5\n\
                   fn check(n: Int) -> Bool { return n < LIMIT }\n\
                   print(check(20))";
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        // 10 * 2 + 5 sigue siendo const-eval (BinOp puros).
        assert!(
            code.contains("const LIMIT : i64") || code.contains("const LIMIT: i64"),
            "esperaba `const LIMIT: i64` hoisteado, got:\n{}",
            code
        );
    }
}
