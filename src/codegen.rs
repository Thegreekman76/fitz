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
    AssignTarget, BinOpKind, Expr, Field, Program, Stmt, StrPart, TypeExpr, UnaryOpKind,
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

    // PASS 1 — Cargar recursivamente todos los módulos importados desde
    // el main, generar su código Rust y registrarlos.
    let mut loader = ModuleLoader::new(base_dir.clone());
    loader.collect_imports(program)?;

    // 5b.6: detectar si el programa (o algún módulo cargado) usa
    // decoradores HTTP/`@server`. Si sí, el Cargo.toml suma axum +
    // tokio + serde + serde_json. Si no, queda minimalista — los
    // ejemplos no-HTTP no pagan el costo de bajar/compilar axum.
    let has_http = has_http_routes(program);

    // PASS 2 — Generar el main.rs. El loader expone los bindings de
    // módulos (`import foo` / `from foo import X`) para que el codegen
    // del main resuelva `foo.x` como path `foo::x` y los tipos
    // importados con sus fields completos.
    let main_rs = generate_main_rs(program, env, &loader)?;

    Ok(ProjectArtifacts {
        bin_name: stem.clone(),
        output_basename: raw_stem,
        cargo_toml: cargo_toml_for(&stem, has_http),
        main_rs,
        mod_files: loader.into_mod_files(),
    })
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
    }
}

fn walk_expr_for_state_refs(
    e: &Expr,
    candidates: &std::collections::HashSet<String>,
    locals: &mut std::collections::HashSet<String>,
    refs: &mut std::collections::HashSet<String>,
) {
    match e {
        Expr::Ident(name) => {
            if candidates.contains(name) && !locals.contains(name) {
                refs.insert(name.clone());
            }
        }
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => {}
        Expr::StrInterp(parts) => {
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
        Expr::Call { callee, args } => {
            walk_expr_for_state_refs(callee, candidates, locals, refs);
            for a in args {
                walk_expr_for_state_refs(a, candidates, locals, refs);
            }
        }
        Expr::If { condition, then, else_ } => {
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
        Expr::Range { start, end } => {
            walk_expr_for_state_refs(start, candidates, locals, refs);
            walk_expr_for_state_refs(end, candidates, locals, refs);
        }
        Expr::List(items) => {
            for it in items {
                walk_expr_for_state_refs(it, candidates, locals, refs);
            }
        }
        Expr::Map(pairs) => {
            for (k, v) in pairs {
                walk_expr_for_state_refs(k, candidates, locals, refs);
                walk_expr_for_state_refs(v, candidates, locals, refs);
            }
        }
        Expr::Index { object, index } => {
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
        Expr::Ok(inner) | Expr::Err(inner) | Expr::Try(inner) => {
            walk_expr_for_state_refs(inner, candidates, locals, refs);
        }
        Expr::Match { value, arms } => {
            walk_expr_for_state_refs(value, candidates, locals, refs);
            for arm in arms {
                // Patterns que introducen bindings extienden locals.
                // Aproximación conservadora: no detallamos cada
                // variante; los Ok(x)/Err(x) bindings no van a chocar
                // con state vars en la práctica (nombres distintos).
                walk_expr_for_state_refs(&arm.body, candidates, locals, refs);
            }
        }
        Expr::FnExpr { params, body } => {
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
fn cargo_toml_for(stem: &str, has_http: bool) -> String {
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
    if has_http {
        format!(
            "{}\n\
             [dependencies]\n\
             axum = \"0.8\"\n\
             tokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n\
             serde = {{ version = \"1\", features = [\"derive\"] }}\n\
             serde_json = {{ version = \"1\", features = [\"preserve_order\"] }}\n",
            header
        )
    } else {
        header
    }
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
}

impl ModuleLoader {
    fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            modules: Vec::new(),
            by_path: HashMap::new(),
            bindings: HashMap::new(),
        }
    }

    /// Recorre el AST del programa principal y carga cada módulo
    /// referenciado por `Stmt::Import` / `Stmt::FromImport`.
    fn collect_imports(&mut self, program: &Program) -> Result<(), FitzError> {
        for stmt in program {
            match stmt {
                Stmt::Import { path, .. } => {
                    let idx = self.load_module(path)?;
                    let binding_name = path.last().cloned().unwrap_or_default();
                    self.bindings.insert(
                        binding_name,
                        ResolvedBinding::Namespace { module_index: idx },
                    );
                }
                Stmt::FromImport { path, names, .. } => {
                    let idx = self.load_module(path)?;
                    for name in names {
                        let kind = self.classify_named(idx, name)?;
                        self.bindings.insert(
                            name.clone(),
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

    /// Resuelve los segmentos a un path absoluto. `["foo"]` →
    /// `<base>/foo.fitz`; `["sub", "foo"]` → `<base>/sub/foo.fitz`.
    fn resolve_path(&self, segments: &[String]) -> PathBuf {
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
        let (module_env, type_errors) = check_program(&module_program);
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
        for (_, binding) in entries {
            if let ResolvedBinding::Named {
                module_index,
                item,
                kind,
            } = binding
            {
                let mod_name = &self.modules[*module_index].mod_name;
                match kind {
                    NamedKind::Type => {
                        output.push_str(&format!(
                            "use {mod}::{{{item}, {item}Data}};\n",
                            mod = mod_name,
                            item = item,
                        ));
                    }
                    NamedKind::Fn | NamedKind::Const => {
                        output.push_str(&format!(
                            "use {}::{};\n",
                            mod_name, item
                        ));
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

    Ok(ctx.output)
}

fn stmt_kind(s: &Stmt) -> &'static str {
    match s {
        Stmt::Assign { .. } => "asignación",
        Stmt::Expr(..) => "expresión suelta",
        Stmt::Return(..) => "return",
        Stmt::While { .. } => "while",
        Stmt::Loop { .. } => "loop",
        Stmt::For { .. } => "for",
        Stmt::Break(_) => "break",
        Stmt::Continue(_) => "continue",
        Stmt::FnDef { .. } => "fn",
        Stmt::TypeDef { .. } => "type",
        Stmt::Import { .. } | Stmt::FromImport { .. } => "import",
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
        Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Null | Expr::Str(_)
    )
}

fn infer_literal_type(e: &Expr) -> Option<Type> {
    match e {
        Expr::Int(_) => Some(Type::Int),
        Expr::Float(_) => Some(Type::Float),
        Expr::Str(_) => Some(Type::Str),
        Expr::Bool(_) => Some(Type::Bool),
        Expr::Null => Some(Type::Null),
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
    let loader = ModuleLoader::new(PathBuf::from("."));
    generate_main_rs(program, env, &loader)
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
) -> Result<String, FitzError> {
    let has_http = has_http_routes(program);

    let mut ctx = CodegenCtx::new(env);
    ctx.install_loader_bindings(loader);
    ctx.pre_register_types(program)?;
    ctx.pre_register_fns(program)?;

    // Particionar stmts top-level. Categorías:
    //   * `type Foo { ... }`              → structs + alias + impl Display.
    //   * `fn ...` con decorators HTTP    → handler: emitirla como pub fn
    //                                       + generar wrapper async.
    //   * `fn main` con decorators        → solo procesar decorators
    //                                       (típicamente `@server`); NO
    //                                       emitir como Rust fn (colisión
    //                                       con `fn main` del crate).
    //   * `fn ...` normal                 → pub fn top-level.
    //   * `Stmt::Import` / `FromImport`   → mod/use decls del loader.
    //   * el resto                        → cuerpo de `fn main()` (modo
    //                                       CLI) o se ignora (modo HTTP).
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
                        match d.name.as_str() {
                            "get" | "post" | "put" | "delete" => http_decos = true,
                            "server" => {
                                server_config = Some(parse_server_decorator(&d.args)?);
                            }
                            other => {
                                return Err(FitzError::new(
                                    ErrorKind::TypeError,
                                    0,
                                    0,
                                    format!(
                                        "decorator `@{}` sobre fn `{}` no soportado en codegen (5b.6 cubre @get/@post/@put/@delete/@server)",
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

    // F11 (post-5b): state HTTP compartido vía `thread_local!`.
    // Detectamos las vars top-level `let X = ...` referenciadas por las
    // fns del programa. Cada una se materializa como un `thread_local!`
    // estático y las fns que las usan emiten al inicio del body
    // `let X = __FITZ_STATE_X.with(|s| s.clone());` — un Rc clone que
    // preserva aliasing. El tokio runtime se configura como
    // `flavor = "current_thread"` para que el thread_local actúe como
    // global (caso contrario, cada worker thread tendría su propia
    // copia y los handlers no compartirían state).
    //
    // Trade-off: el server HTTP queda single-threaded. Para el subset
    // de Fitz HTTP de hoy (handlers sync, sin async externo, sin
    // workloads CPU-bound) es irrelevante. Cuando Fitz sume async/await
    // reales, este approach se reemplaza por `Arc<Mutex<...>>` con
    // `State` extractor (deuda residual documentada).
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
    for s in &main_stmts {
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

    ctx.emit_prelude();
    loader.emit_mod_decls(&mut ctx.output);
    loader.emit_use_decls(&mut ctx.output);

    // 5b.6: cuando hay HTTP emitimos los helpers de serialización
    // (`__ToFitzJson` / `__FromFitzJson`) antes de los tipos custom,
    // porque los `impl` de cada `type` los referencian.
    if has_http {
        ctx.emit_http_runtime_prelude();
    }

    for stmt in &type_defs {
        ctx.gen_type_def(stmt)?;
        if has_http {
            ctx.gen_type_http_impls(stmt)?;
        }
    }
    for stmt in &http_fns {
        ctx.gen_top_fn(stmt)?;
    }
    for stmt in top_fns {
        ctx.gen_top_fn(stmt)?;
    }

    if has_http {
        // Emitir un wrapper `async fn __handler_<name>` por cada handler.
        for stmt in &http_fns {
            ctx.gen_http_handler_wrapper(stmt)?;
        }
        // `#[tokio::main] async fn main` con Router + serve.
        ctx.gen_http_main(&http_fns, &server_config, &main_stmts)?;
    } else {
        // Modo CLI: cuerpo de `fn main()` con el resto de stmts.
        ctx.gen_main(&main_stmts)?;
    }

    Ok(ctx.output)
}

/// Valores parseados de `@server(port?, host?)`. Defaults aplicados
/// (puerto 3000, host "127.0.0.1") si los args no están.
#[derive(Debug, Clone)]
struct ServerConfigArgs {
    port: u16,
    host: String,
}

impl Default for ServerConfigArgs {
    fn default() -> Self {
        ServerConfigArgs {
            port: 3000,
            host: "127.0.0.1".to_string(),
        }
    }
}

/// Parsea los args de un decorator `@server(port?, host?)`. Validaciones:
///   - Hasta 2 args positionals: `(port: Int)` o `(port: Int, host: Str)`.
///   - Port entre 1 y 65535.
///   - Host parsea como `IpAddr` (sin DNS). Validación delegada al runtime
///     porque acá solo tenemos un literal Str.
fn parse_server_decorator(args: &[Expr]) -> Result<ServerConfigArgs, FitzError> {
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
        let Expr::Int(n) = port_expr else {
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
        let Expr::Str(s) = host_expr else {
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
    /// usual `Rc<RefCell<...>>` (no cambia la repr de tipos) y se
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

    // --- error helpers ----------------------------------------------------

    fn err(&self, msg: impl Into<String>) -> FitzError {
        FitzError::new(ErrorKind::TypeError, 0, 0, msg.into())
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
        // Rc<RefCell<>> es la representación de las instancias de
        // tipos custom — coincide con el modelo del intérprete (las
        // mutaciones se ven a través de cualquier alias).
        self.emit("use std::rc::Rc;\n");
        self.emit("use std::cell::RefCell;\n\n");
        // Helper de formato para Float: alinea con `Display` del
        // intérprete (`3.0` se imprime como `\"3.0\"`, no `\"3\"`).
        // Cada archivo (main.rs o mod) trae su propio `__fitz_fmt_float`;
        // no compartimos — es solo unas pocas líneas y nos ahorra una
        // dependencia cross-module.
        self.emit(
            "fn __fitz_fmt_float(v: f64) -> String {\n    \
             if v.is_finite() && v.fract() == 0.0 { format!(\"{:.1}\", v) } else { format!(\"{}\", v) }\n}\n\n",
        );
    }

    fn gen_main(&mut self, stmts: &[&Stmt]) -> Result<(), FitzError> {
        self.emit("fn main() {\n");
        self.indent += 1;
        self.push_scope();
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
            let Stmt::Assign { target, type_, value, .. } = stmt else { continue };
            let AssignTarget::Ident(name) = target else { continue };
            let ty = match type_ {
                Some(te) => resolve_type_expr(te, self.env).map_err(|e| {
                    self.err(format!(
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
        for stmt in program {
            if let Stmt::FnDef {
                name,
                params,
                return_type,
                ..
            } = stmt
            {
                let params: Vec<Type> = params
                    .iter()
                    .map(|p| self.resolve_param_type(name, &p.name, p.type_.as_ref()))
                    .collect::<Result<_, _>>()?;
                let ret = match return_type {
                    Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                        FitzError::new(
                            e.kind,
                            0,
                            0,
                            format!(
                                "fn `{}`: return type no resuelve: {}",
                                name, e.message
                            ),
                        )
                    })?,
                    None => Type::Null,
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
    ) -> Result<Type, FitzError> {
        match type_ {
            Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                FitzError::new(
                    e.kind,
                    0,
                    0,
                    format!(
                        "fn `{}`: parámetro `{}`: {}",
                        fn_name, param_name, e.message
                    ),
                )
            }),
            None => Err(self.err(format!(
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
        // `PartialEq` derivado compara campo a campo. Para campos
        // `Rc<RefCell<T>>` (instancias anidadas) `PartialEq` de
        // `Rc<T>` compara por **contenido** (no identidad), y
        // `RefCell<T>` compara borroweando — matchea exacto la
        // semántica estructural del intérprete.
        let pub_kw = self.pub_prefix();
        let field_pub = pub_kw;
        write!(
            &mut self.output,
            "#[derive(Clone, PartialEq)]\n{}struct {} {{\n",
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

        // type Foo = Rc<RefCell<FooData>>;
        write!(
            &mut self.output,
            "{}type {} = Rc<RefCell<{}>>;\n\n",
            pub_kw, name, data_name
        )
        .unwrap();

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
        Ok(())
    }

    // --- generación de funciones top-level --------------------------------

    fn gen_top_fn(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::FnDef {
            name,
            params,
            return_type: _,
            body,
            decorators,
            ..
        } = stmt
        else {
            unreachable!("gen_top_fn solo se llama sobre Stmt::FnDef");
        };

        // 5b.6: las fns con decoradores HTTP (`@get`/`@post`/etc.) se
        // emiten como `pub fn` normales — el wrapper `async fn
        // __handler_<name>` (`gen_http_handler_wrapper`) las llama
        // adentro de la response builder. Los decoradores en sí no
        // afectan el codegen del cuerpo, solo los pre-categoriza
        // `generate_main_rs`. Acá los ignoramos.
        let _ = decorators;

        let sig = self
            .fn_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("fn `{}` no estaba pre-registrada", name)))?;

        // Header: fn <name>(p1: T1, p2: T2, ...) -> Ret {
        let pub_kw = self.pub_prefix();
        self.emit(pub_kw);
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
        if !matches!(sig.ret, Type::Null) {
            self.emit(" -> ");
            self.emit(&rust_type_for(&sig.ret, self.env)?);
        }
        self.emit(" {\n");

        // Body
        self.indent += 1;
        self.push_scope();
        for (param, pty) in params.iter().zip(sig.params.iter()) {
            self.declare_var(param.name.clone(), pty.clone());
        }
        // F11: si esta fn referencia algún state HTTP shared, lo
        // materializamos como var local al inicio del body. El `clone()`
        // sobre el contenido del thread_local es Rc clone (barato) y
        // preserva aliasing — mutaciones via `users.push(...)` se ven en
        // todas las llamadas posteriores porque el thread_local guarda
        // el Rc, no el contenido.
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
                    "let mut {}: {} = {}.with(|__s| __s.clone());",
                    dep_name, rust_ty, static_name
                )
                .unwrap();
                self.declare_var(dep_name.clone(), ty);
            }
        }
        // Frame de "return esperado" para coerciones y para que `?`
        // (Try) pueda validar que está adentro de una fn Result.
        self.ret_stack.push(sig.ret.clone());
        for stmt in body {
            self.gen_stmt_in_fn(stmt, &sig.ret)?;
        }
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
            Stmt::FnDef { name, .. } => Err(self.err(format!(
                "fn anidada `{}`: no soportada en 5b.1 — declarala a nivel top",
                name
            ))),
            Stmt::TypeDef { name, .. } => Err(self.err(format!(
                "`type {}`: solo se admite a nivel top, no adentro de funciones u otros bloques",
                name
            ))),
            Stmt::Import { .. } | Stmt::FromImport { .. } => Err(self.err(
                "`import`: módulos no soportados en 5b.1 — llegan en 5b.5",
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
                self.err(format!("anotación de `{}` no resuelve: {}", name, e.message))
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
            self.emit("let mut ");
            self.emit(name);
            self.emit(": ");
            self.emit(&rust_type_for(&declared_ty, self.env)?);
            self.emit(" = ");
            self.emit(&final_rhs);
            self.emit(";\n");
            self.declare_var(name.clone(), declared_ty);
        }
        Ok(())
    }

    /// Emite un `let X = <literal>` top-level de un módulo como
    /// `pub const X: T = ...;` (primitivos) o `pub static X: &str =
    /// "...";` (Str). La validación de "es un literal" ya la hizo
    /// `collect_module_sigs` antes; acá asumimos que el value es
    /// una de las variantes literales.
    fn gen_module_top_let(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::Assign { target, type_, value, .. } = stmt else {
            unreachable!("gen_module_top_let solo se llama sobre Stmt::Assign");
        };
        let AssignTarget::Ident(name) = target else {
            return Err(self.err(
                "asignación a campo a nivel top de módulo: no soportada (solo `let X = <literal>`)",
            ));
        };

        let declared_ty = match type_ {
            Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                self.err(format!(
                    "let `{}`: anotación: {}",
                    name, e.message
                ))
            })?,
            None => infer_literal_type(value).ok_or_else(|| {
                self.err(format!(
                    "let `{}` top-level de módulo: la RHS debe ser literal o tenés que anotar el tipo",
                    name
                ))
            })?,
        };

        // Str → `pub static X: &str = "...";`. Los otros → `pub const`.
        match (&declared_ty, value) {
            (Type::Str, Expr::Str(s)) => {
                writeln!(
                    &mut self.output,
                    "pub static {}: &str = {};\n",
                    name,
                    rust_str_literal(s)
                )
                .unwrap();
            }
            (Type::Int, Expr::Int(n)) => {
                writeln!(&mut self.output, "pub const {}: i64 = {}i64;\n", name, n).unwrap();
            }
            (Type::Float, Expr::Float(f)) => {
                writeln!(&mut self.output, "pub const {}: f64 = {}f64;\n", name, f).unwrap();
            }
            (Type::Float, Expr::Int(n)) => {
                // Coerción explícita Int → Float, como en el resto del codegen.
                writeln!(
                    &mut self.output,
                    "pub const {}: f64 = {}f64;\n",
                    name, *n as f64
                )
                .unwrap();
            }
            (Type::Bool, Expr::Bool(b)) => {
                writeln!(&mut self.output, "pub const {}: bool = {};\n", name, b).unwrap();
            }
            _ => {
                return Err(self.err(format!(
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
            return Err(self.err(format!(
                "asignación a campo `.{}` sobre `{}`: solo se soporta sobre instancias",
                field,
                type_name(&obj_ty)
            )));
        };
        let info_name = self.env.info(*id).name.clone();
        let declared = self.fields_for_id(*id).ok_or_else(|| {
            self.err(format!(
                "tipo `{}` con campos sin resolver — no se puede generar asignación",
                info_name
            ))
        })?;
        let Some(f) = declared.iter().find(|f| f.name == field) else {
            return Err(self.err(format!(
                "el tipo `{}` no tiene un campo llamado `{}`",
                info_name, field
            )));
        };
        let (rhs_code, rhs_ty) = self.gen_expr(value)?;
        let coerced = coerce(&rhs_code, &rhs_ty, &f.type_);
        self.emit_indent();
        writeln!(
            &mut self.output,
            "({}).borrow_mut().{} = {};",
            obj_code, field, coerced
        )
        .unwrap();
        Ok(())
    }

    fn gen_return(&mut self, e: &Expr, ret_expected: &Type) -> Result<(), FitzError> {
        let (code, ty) = self.gen_expr(e)?;
        let coerced = coerce(&code, &ty, ret_expected);
        self.emit_indent();
        self.emit("return ");
        self.emit(&coerced);
        self.emit(";\n");
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
        if let Expr::Range { start, end } = iter {
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
                return Err(self.err(format!(
                    "`for {} in <expr>`: el iterable es `{}`, solo se soportan Range y List<T>",
                    var,
                    display_type(other, self.env)
                )));
            }
        };
        if matches!(elem_ty, Type::Any) {
            return Err(self.err(format!(
                "`for {} in ...` sobre `List<Any>`: el subset compilado exige tipo homogéneo \
                 concreto",
                var
            )));
        }
        self.emit_indent();
        writeln!(
            &mut self.output,
            "for mut {var} in ({iter_code}).borrow().clone().into_iter() {{"
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
            Expr::Int(n) => Ok((format!("{}i64", n), Type::Int)),
            Expr::Float(n) => {
                // `1.0` ya es f64 literal en Rust; sufijo opcional
                // pero claro. Para evitar `inf`/`-inf` corner cases
                // delegamos al Display de f64 que produce literal
                // válido.
                Ok((format!("{}f64", n), Type::Float))
            }
            Expr::Str(s) => Ok((format!("String::from({})", rust_str_literal(s)), Type::Str)),
            Expr::Bool(b) => Ok((b.to_string(), Type::Bool)),
            Expr::Null => Ok(("()".to_string(), Type::Null)),

            Expr::Ident(name) => {
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
                                let code = match &ty {
                                    Type::Str => format!("String::from({})", item),
                                    _ => item.clone(),
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
                // `(Rc::new(<name>) as Rc<dyn Fn(...) -> R>)`. Esto
                // habilita `let f = square` y `apply(square, 7)`. Las
                // fn items de Rust implementan `Fn(...)` así que el
                // `Rc::new(square)` compila directo. El caso "callee
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
                            "(Rc::new({}) as Rc<dyn Fn({}) -> {}>)",
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

            Expr::StrInterp(parts) => self.gen_str_interp(parts),

            Expr::BinOp { op, left, right } => self.gen_binop(op, left, right),
            Expr::UnaryOp { op, operand } => self.gen_unary(op, operand),

            Expr::Call { callee, args } => self.gen_call(callee, args),

            Expr::If { condition, then, else_ } => {
                self.gen_if_expr(condition, then, else_.as_deref())
            }

            Expr::Range { .. } => Err(self.err(
                "`Range` solo se acepta como iterable de `for`; otros usos no se generan",
            )),
            Expr::List(items) => self.gen_list_lit(items),
            Expr::Map(pairs) => self.gen_map_lit(pairs),
            Expr::Index { object, index } => self.gen_index(object, index),
            Expr::Field { object, field } => self.gen_field_access(object, field),
            Expr::StructLit { type_name, fields } => self.gen_struct_lit(type_name, fields),
            Expr::Ok(inner) => self.gen_ok(inner),
            Expr::Err(inner) => self.gen_err(inner),
            Expr::Try(inner) => self.gen_try(inner),
            Expr::Match { value, arms } => self.gen_match(value, arms),
            // FnExpr "suelto" — usado como valor, parámetro o retorno
            // (higher-order, F12). Emite `Rc::new(move |p1: T1, ...|
            // -> R { body }) as Rc<dyn Fn(T1, ...) -> R>`. Los
            // callbacks inline de `.map`/`.filter`/`.find` siguen
            // interceptándose en `gen_method_call` antes de llegar
            // acá — esos no necesitan boxear porque el método los
            // consume directo. Acá llega cualquier FnExpr usado como
            // valor: `let f = fn(n) => ...`, `apply(fn(n) => ..., 7)`,
            // `return fn(y) => x + y`.
            Expr::FnExpr { params, body } => self.gen_fn_expr_as_value(params, body),
        }
    }

    /// Para statements `Stmt::Expr(e, Span::ZERO)`: si `e` es una llamada a
    /// `print(...)`, generamos `println!(...)` (que devuelve `()`).
    /// El resto cae al `gen_expr` normal.
    fn gen_expr_for_stmt(&mut self, e: &Expr) -> Result<(), FitzError> {
        if let Expr::Call { callee, args } = e {
            if let Expr::Ident(name) = callee.as_ref() {
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
                    .ok_or_else(|| self.err(format!(
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
                    .ok_or_else(|| self.err(format!(
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
                    .ok_or_else(|| self.err(format!(
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
                // Igualdad estructural entre instancias del mismo
                // tipo: borroweamos ambos lados y comparamos por
                // valor — `#[derive(PartialEq)]` sobre `FooData`
                // recursea campo a campo (incluyendo nominales
                // anidados como `Rc<RefCell<T>>`, que comparan por
                // contenido, no identidad).
                if let (Type::Nominal(id_l), Type::Nominal(id_r)) = (&lt, &rt) {
                    if id_l != id_r {
                        return Err(self.err(
                            "igualdad entre instancias de tipos distintos: el checker debería haberlo cazado",
                        ));
                    }
                    return Ok((
                        format!("(*({}).borrow() {} *({}).borrow())", lc, sym, rc),
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
    ) -> Result<(String, Type), FitzError> {
        // Method call: el callee es `Expr::Field { object, field }`.
        // Despachamos por `(tipo del receptor, nombre del método)`
        // como hace el evaluator. Hoy solo cubrimos métodos built-in
        // sobre Str; List/Map y métodos custom sobre `type` quedan
        // como deuda (llegan en 5b.3 y post-3.2 respectivamente).
        //
        // 5b.5: caso especial — si el object es `Ident(ns)` con `ns`
        // siendo namespace de módulo, traducimos `foo.greet(args)` →
        // `foo::greet(args)` Rust con la firma del módulo.
        if let Expr::Field { object, field } = callee {
            if let Expr::Ident(ns) = object.as_ref() {
                if let Some(ResolvedBinding::Namespace { .. }) =
                    self.module_bindings.get(ns).cloned()
                {
                    if let Some((path, sig)) = self.resolve_namespace_call(ns, field) {
                        return self.gen_call_with_sig(&path, &sig, args);
                    }
                    return Err(self.err(format!(
                        "el módulo `{}` no exporta una función llamada `{}`",
                        ns, field
                    )));
                }
            }
            return self.gen_method_call(object, field, args);
        }
        let Expr::Ident(name) = callee else {
            return Err(self.err(
                "llamadas con callee complejo (FnExpr inline u otro Expr): no soportadas",
            ));
        };
        if name == "print" {
            return Err(self.err(
                "`print(...)` solo puede usarse como sentencia, no como expresión en 5b.1",
            ));
        }
        // Builtin global `len(x)`: despacha por tipo del argumento a la
        // misma implementación que el método (`.len()`). Cubre Str, List
        // y Map. Si el usuario tiene una fn `len` definida (raro pero
        // válido), su sig prevalece — chequeamos `fn_sigs` antes del
        // builtin.
        if name == "len" && !self.fn_sigs.contains_key(name) && args.len() == 1 {
            let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
            return match arg_ty {
                Type::Str => Ok((
                    format!("(({}).chars().count() as i64)", arg_code),
                    Type::Int,
                )),
                Type::List(_) | Type::Map(_, _) => Ok((
                    format!("(({}).borrow().len() as i64)", arg_code),
                    Type::Int,
                )),
                other => Err(self.err(format!(
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
                        return self.gen_call_with_sig(name, &sig, args);
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
            return self.gen_call_with_sig(name, &sig, args);
        }

        let sig = self
            .fn_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("función `{}` desconocida en codegen", name)))?;
        self.gen_call_with_sig(name, &sig, args)
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
    ) -> Result<(String, Type), FitzError> {
        if args.len() != sig.params.len() {
            return Err(self.err(format!(
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
    ) -> Result<(String, Type), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
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
            (Type::Str, other) => Err(self.err(format!(
                "Str no tiene el método `{}` en el subset compilado (hoy: len/upper/lower)",
                other
            ))),

            // ---- List ----
            (Type::List(t), "push") => self.gen_list_push(&obj_code, t, args),
            (Type::List(t), "pop") => self.gen_list_pop(&obj_code, t, args),
            (Type::List(_), "len") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("(({}).borrow().len() as i64)", obj_code), Type::Int))
            }
            (Type::List(t), "map") => self.gen_list_map(&obj_code, t, args),
            (Type::List(t), "filter") => self.gen_list_filter(&obj_code, t, args),
            (Type::List(t), "find") => self.gen_list_find(&obj_code, t, args),
            (Type::List(_), other) => Err(self.err(format!(
                "List no tiene el método `{}` en el subset compilado (hoy: push/pop/len/map/filter)",
                other
            ))),

            // ---- Map ----
            (Type::Map(k, _), "has") => self.gen_map_has(&obj_code, k, args),
            (Type::Map(k, _), "keys") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "Rc::new(RefCell::new(({}).borrow().iter().map(|(__k, _)| __k.clone()).collect::<Vec<_>>()))",
                    obj_code
                );
                Ok((code, Type::List(Box::new((**k).clone()))))
            }
            (Type::Map(_, v), "values") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "Rc::new(RefCell::new(({}).borrow().iter().map(|(_, __v)| __v.clone()).collect::<Vec<_>>()))",
                    obj_code
                );
                Ok((code, Type::List(Box::new((**v).clone()))))
            }
            (Type::Map(_, _), "len") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("(({}).borrow().len() as i64)", obj_code), Type::Int))
            }
            (Type::Map(k, v), "get") => self.gen_map_get(&obj_code, k, v, args),
            (Type::Map(_, _), other) => Err(self.err(format!(
                "Map no tiene el método `{}` en el subset compilado (hoy: has/keys/values/len)",
                other
            ))),

            // ---- Tipos custom ----
            (Type::Nominal(_), m) => Err(self.err(format!(
                "métodos custom sobre `type` (`.{}`): primero hay que cerrar la deuda de 3.2 en el parser",
                m
            ))),

            // ---- Otros ----
            (other, m) => Err(self.err(format!(
                "method call `.{}` sobre `{}`: no soportado en codegen",
                m,
                display_type(other, self.env)
            ))),
        }
    }

    // --- métodos List ----------------------------------------------------

    /// `xs.push(x)` → `({xs}).borrow_mut().push({coerce x → T})`. Devuelve
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
        let code = format!("({}).borrow_mut().push({})", obj_code, coerced);
        Ok((code, Type::Null))
    }

    /// `xs.pop()` → `({xs}).borrow_mut().pop().expect(...)`. El intérprete
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
            "({}).borrow_mut().pop().expect(\"`.pop()` sobre lista vacía\")",
            obj_code
        );
        Ok((code, elem_ty.clone()))
    }

    /// `xs.map(callback)` → snapshot del Vec + map + collect, envuelto en
    /// `Rc::new(RefCell::new(...))`. El callback debe ser un FnExpr
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
                let __items: Vec<_> = ({}).borrow().clone(); \
                Rc::new(RefCell::new(__items.into_iter().map({}).collect::<Vec<_>>())) \
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
                let __items: Vec<_> = ({}).borrow().clone(); \
                let __cb = {}; \
                let mut __out: Vec<_> = Vec::new(); \
                for __it in __items.into_iter() {{ \
                    if __cb(__it.clone()) {{ __out.push(__it); }} \
                }} \
                Rc::new(RefCell::new(__out)) \
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
                let __items: Vec<_> = ({}).borrow().clone(); \
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
                let __pairs = __map.borrow(); \
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
            "{{ let __k = {}; ({}).borrow().iter().any(|(__k2, _)| __k2 == &__k) }}",
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
        let (params, body) = match arg {
            Expr::FnExpr { params, body } => (params, body),
            _ => {
                return Err(self.err(format!(
                    "`.{}(...)` exige un callback inline `fn(x) => ...` o `fn(x) {{ ... }}`. \
                     Pasar una fn nombrada como callback (higher-order) llega en un sub-paso \
                     posterior de 5b.",
                    method
                )));
            }
        };
        if params.len() != 1 {
            return Err(self.err(format!(
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
        self.push_scope();
        self.declare_var(param_name.clone(), param_ty.clone());
        let saved = std::mem::take(&mut self.output);
        let saved_indent = self.indent;
        self.indent = 0;
        let mut body_str = String::new();
        for s in body {
            self.gen_stmt_in_fn(s, &ret_ty)?;
            body_str.push_str(&std::mem::take(&mut self.output));
        }
        self.output = saved;
        self.indent = saved_indent;
        self.pop_scope();
        self.ret_stack.pop();

        let code = format!(
            "|{}: {}| -> {} {{ {} }}",
            param_name, param_ty_rs, ret_ty_rs, body_str
        );
        Ok((code, ret_ty))
    }

    /// Emite un `FnExpr` "suelto" (no callback inline de
    /// map/filter/find) como **valor** de tipo `Rc<dyn Fn(...) -> R>`.
    /// Cubre `let f = fn(n) => n * 2`, `apply(fn(n) => n * 10, 7)`,
    /// `return fn(y) => x + y` (closure que captura `x` del scope
    /// contenedor). Por uniformidad emitimos siempre con `move` y
    /// el cast a `Rc<dyn Fn(...) -> R>` para que rustc no se queje
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
    ) -> Result<(String, Type), FitzError> {
        // Cada param exige anotación de tipo — sin contexto bidireccional
        // no podemos inferir el tipo del param desde su uso. Esta es la
        // misma regla que aplican las fns top-level (deuda 5b.1).
        let mut param_types: Vec<Type> = Vec::with_capacity(params.len());
        for p in params {
            let Some(te) = p.type_.as_ref() else {
                return Err(self.err(format!(
                    "función anónima `fn({})`: el parámetro `{}` necesita una anotación de \
                     tipo en el subset compilable (deuda 5b.1). Anotalo o usá `fitz run`.",
                    p.name, p.name
                )));
            };
            let t = resolve_type_expr(te, self.env).map_err(|e| {
                self.err(format!(
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
        self.push_scope();
        for (p, t) in params.iter().zip(param_types.iter()) {
            self.declare_var(p.name.clone(), t.clone());
        }
        for (name, ty) in &captures {
            self.declare_var(name.clone(), ty.clone());
        }
        let saved = std::mem::take(&mut self.output);
        let saved_indent = self.indent;
        self.indent = 0;
        let mut body_str = String::new();
        for s in body {
            self.gen_stmt_in_fn(s, &ret_ty)?;
            body_str.push_str(&std::mem::take(&mut self.output));
        }
        self.output = saved;
        self.indent = saved_indent;
        self.pop_scope();
        self.ret_stack.pop();

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
            format!("Rc<dyn Fn({}) -> {}>", ps.join(", "), ret_ty_rs)
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
            format!("(Rc::new(move {closure}) as {cast_target})", closure = closure, cast_target = cast_target)
        } else {
            format!(
                "{{ {clones}Rc::new(move {closure}) as {cast_target} }}",
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
    /// Como en `infer_callback_ret_silently`: scope nuevo con params
    /// + capturas bindeados, gen_expr sobre el primer `Stmt::Return`
    /// del body (o último `Stmt::Expr` no-print, o `Null`).
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
                return Err(self.err(format!(
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
                return Err(self.err(
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

    /// `[e1, e2, ...]` → `Rc::new(RefCell::new(vec![v1, v2, ...]))` con
    /// coerción de cada elemento al tipo común. Tipo común sintetizado
    /// como en el checker (5.3.1): primer elemento define el tipo, los
    /// demás deben unificar via `lub` (Int↔Float, T↔Null). Mezcla
    /// irrecuperable o lista vacía sin contexto → error claro.
    fn gen_list_lit(&mut self, items: &[Expr]) -> Result<(String, Type), FitzError> {
        if items.is_empty() {
            // Lista vacía: no podemos sintetizar T. Emitimos un código
            // genérico `Vec::new()` y devolvemos `List<Any>`. El
            // contexto (anotación destino, paso a fn tipada) coerciona
            // a un T concreto; si nadie lo restringe, el rustc generado
            // fallará con "type annotations needed", reflejando que el
            // usuario tiene que anotar.
            return Ok((
                "Rc::new(RefCell::new(Vec::new()))".to_string(),
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
                self.err(format!(
                    "lista con elementos de tipos incompatibles (`{}` y `{}`): el subset compilado \
                     exige una lista homogénea (todos del mismo tipo, con coerciones Int→Float y \
                     T→T? permitidas)",
                    display_type(&common_ty, self.env),
                    display_type(t, self.env),
                ))
            })?;
        }
        if matches!(common_ty, Type::Any) {
            return Err(self.err(
                "lista con elementos cuyo tipo común es `Any`: el subset compilado exige tipo \
                 homogéneo concreto. Anotá el tipo o usá `fitz run` para interpretarlo sin restricción.",
            ));
        }
        let coerced: Vec<String> = item_codes_tys
            .iter()
            .map(|(c, t)| coerce(c, t, &common_ty))
            .collect();
        let code = format!(
            "Rc::new(RefCell::new(vec![{}]))",
            coerced.join(", ")
        );
        Ok((code, Type::List(Box::new(common_ty))))
    }

    /// `{k1: v1, k2: v2, ...}` → `Rc::new(RefCell::new(vec![(k1, v1), ...]))`.
    /// Orden de inserción preservado por Vec. K y V deben ser homogéneos
    /// (mismas reglas que List). Para `m["k"]` (Index) y `m.get(k)` la
    /// búsqueda es lineal O(n), pero matchea exactamente lo que hace
    /// el intérprete.
    fn gen_map_lit(&mut self, pairs: &[(Expr, Expr)]) -> Result<(String, Type), FitzError> {
        if pairs.is_empty() {
            return Ok((
                "Rc::new(RefCell::new(Vec::new()))".to_string(),
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
                self.err(format!(
                    "mapa con claves de tipos incompatibles (`{}` y `{}`): el subset compilado \
                     exige claves homogéneas",
                    display_type(&common_k, self.env),
                    display_type(kt, self.env),
                ))
            })?;
            common_v = lub(&common_v, vt).map_err(|_| {
                self.err(format!(
                    "mapa con valores de tipos incompatibles (`{}` y `{}`): el subset compilado \
                     exige valores homogéneos",
                    display_type(&common_v, self.env),
                    display_type(vt, self.env),
                ))
            })?;
        }
        if matches!(common_k, Type::Any) || matches!(common_v, Type::Any) {
            return Err(self.err(
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
            "Rc::new(RefCell::new(vec![{}]))",
            pieces.join(", ")
        );
        Ok((code, Type::Map(Box::new(common_k), Box::new(common_v))))
    }

    /// `obj[idx]` — dispatch por tipo del receptor.
    ///
    ///   - `List<T>[Int]`   → `({xs}.borrow()[idx as usize].clone())`.
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
    ) -> Result<(String, Type), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        let (idx_code, idx_ty) = self.gen_expr(index)?;
        match &obj_ty {
            Type::List(inner) => {
                if !matches!(idx_ty, Type::Int) {
                    return Err(self.err(format!(
                        "indexing de lista con `{}`: el índice debe ser Int",
                        display_type(&idx_ty, self.env)
                    )));
                }
                let code = format!(
                    "({}).borrow()[({}) as usize].clone()",
                    obj_code, idx_code
                );
                Ok((code, (**inner).clone()))
            }
            Type::Map(k_ty, v_ty) => {
                let coerced_idx = coerce(&idx_code, &idx_ty, k_ty);
                // Búsqueda lineal por igualdad. `unwrap_or_else(panic)` con
                // mensaje al estilo del intérprete. Ligamos el Rc a una
                // var local antes de `.borrow()` para extender la vida
                // del temporal — `(m.clone()).borrow()` solo cuando la
                // expresión completa cabe en una stmt simple; acá usamos
                // un `let __m = ...` y necesitamos el holder.
                let code = format!(
                    "{{ \
                        let __map = {}; \
                        let __m = __map.borrow(); \
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
            other => Err(self.err(format!(
                "indexing `[]` sobre `{}`: solo soportado en List<T> y Map<K, V>",
                display_type(other, self.env)
            ))),
        }
    }

    fn gen_struct_lit(
        &mut self,
        type_name: &str,
        provided: &[(String, Expr)],
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
                return Err(self.err(format!(
                    "el tipo `{}` no tiene un campo llamado `{}`",
                    type_name, provided_name
                )));
            }
        }

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
                let (code, ty) = self.gen_expr(default_expr)?;
                coerce(&code, &ty, &f.type_)
            } else if matches!(f.type_, Type::Nullable(_)) {
                "None".to_string()
            } else {
                return Err(self.err(format!(
                    "falta el campo `{}` al instanciar `{}` (no tiene default y no es nullable)",
                    f.name, type_name
                )));
            };
            field_codes.push(format!("{}: {}", f.name, value_code));
        }

        let data_name = format!("{}Data", type_name);
        let code = format!(
            "Rc::new(RefCell::new({} {{ {} }}))",
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
    ) -> Result<(String, Type), FitzError> {
        // 5b.5: si el objeto es `Ident(ns)` con `ns` siendo un namespace
        // de módulo importado (`import foo`), traducimos `foo.bar` a
        // path Rust `foo::bar`. Lo hacemos ANTES de evaluar el objeto,
        // porque `Ident("foo")` no está en `scopes` (los imports no
        // declaran var en el codegen), pero sí en `module_bindings`.
        if let Expr::Ident(ns) = object {
            if let Some(ResolvedBinding::Namespace { .. }) =
                self.module_bindings.get(ns).cloned()
            {
                if let Some((code, ty)) = self.resolve_namespace_field(ns, field) {
                    return Ok((code, ty));
                }
                return Err(self.err(format!(
                    "el módulo `{}` no exporta `{}` (ni fn ni constante)",
                    ns, field
                )));
            }
        }

        let (obj_code, obj_ty) = self.gen_expr(object)?;
        let Type::Nominal(id) = &obj_ty else {
            return Err(self.err(format!(
                "field access `.{}` sobre `{}`: solo se soporta sobre instancias de tipos custom",
                field,
                type_name(&obj_ty)
            )));
        };
        let info_name = self.env.info(*id).name.clone();
        let declared = self.fields_for_id(*id).ok_or_else(|| {
            self.err(format!(
                "tipo `{}` con campos sin resolver — no se puede generar acceso",
                info_name
            ))
        })?;
        let Some(f) = declared.iter().find(|f| f.name == field) else {
            return Err(self.err(format!(
                "el tipo `{}` no tiene un campo llamado `{}`",
                info_name, field
            )));
        };
        // `code.borrow().field` es válido cuando el accesor consume
        // el valor en una expresión que se evalúa inmediatamente.
        // Como devolvemos una expresión Rust que puede entrar en
        // arbitrary contextos, agregamos `.clone()` cuando el tipo lo
        // requiere (Str, Nominal, Option de cualquier cosa). Para
        // tipos `Copy` (Int/Float/Bool/Null), el borrow basta.
        let access = if needs_clone(&f.type_) {
            format!("({}).borrow().{}.clone()", obj_code, field)
        } else {
            format!("({}).borrow().{}", obj_code, field)
        };
        Ok((access, f.type_.clone()))
    }

    fn gen_if_expr(
        &mut self,
        condition: &Expr,
        then: &[Stmt],
        else_: Option<&[Stmt]>,
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
                self.err(format!(
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

    /// Genera el wrapper `async fn __handler_<name>(...)` para un
    /// handler decorado con `@get/@post/@put/@delete`. Extrae path
    /// params + body (si corresponde), llama a la fn original, y
    /// convierte el resultado en una `axum::response::Response`.
    fn gen_http_handler_wrapper(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::FnDef {
            name,
            params,
            decorators,
            return_type,
            ..
        } = stmt
        else {
            return Ok(());
        };

        // Encontrar el decorator HTTP de esta fn (puede haber otros, los
        // ignoramos — el filtrado lo hizo `generate_main_rs`).
        let http_deco = decorators
            .iter()
            .find(|d| {
                matches!(d.name.as_str(), "get" | "post" | "put" | "delete")
            })
            .ok_or_else(|| self.err(format!("fn `{}`: sin decorator HTTP", name)))?;
        let path_arg = http_deco.args.first().ok_or_else(|| {
            self.err(format!(
                "fn `{}`: @{} requiere un path como primer arg",
                name, http_deco.name
            ))
        })?;
        let path = parse_http_path(path_arg)?;

        let template_params = extract_path_template_names(&path);

        // Resolver tipos resueltos de cada param.
        let mut resolved_params: Vec<(String, Type)> = Vec::with_capacity(params.len());
        for p in params {
            let te = p.type_.as_ref().ok_or_else(|| {
                self.err(format!(
                    "fn `{}`: parámetro `{}` necesita anotación de tipo",
                    name, p.name
                ))
            })?;
            let t = resolve_type_expr(te, self.env).map_err(|e| self.err(e.message.clone()))?;
            resolved_params.push((p.name.clone(), t));
        }

        // Categorizar: cada param es path o body.
        let mut path_params: Vec<(String, Type)> = Vec::new();
        let mut body_param: Option<(String, Type)> = None;
        for (n, t) in &resolved_params {
            if template_params.iter().any(|tp| tp == n) {
                path_params.push((n.clone(), t.clone()));
            } else if body_param.is_some() {
                return Err(self.err(format!(
                    "fn `{}`: solo se admite un body param por handler",
                    name
                )));
            } else {
                body_param = Some((n.clone(), t.clone()));
            }
        }

        let resolved_ret = match return_type {
            Some(te) => resolve_type_expr(te, self.env).map_err(|e| self.err(e.message.clone()))?,
            None => Type::Null,
        };
        let returns_result = matches!(resolved_ret, Type::Result(_));

        // Firma del wrapper. Construimos los extractores axum en orden
        // declarado por el usuario: path tuple primero, body al final.
        writeln!(&mut self.output, "async fn __handler_{}(", name).unwrap();
        if !path_params.is_empty() {
            if path_params.len() == 1 {
                let (pn, pt) = &path_params[0];
                writeln!(
                    &mut self.output,
                    "    axum::extract::Path({}): axum::extract::Path<{}>,",
                    pn,
                    rust_type_for(pt, self.env)?,
                )
                .unwrap();
            } else {
                // Path<(T1, T2, ...)> con nombres tupleados.
                let names: Vec<String> = path_params.iter().map(|(n, _)| n.clone()).collect();
                let types: Vec<String> = path_params
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
        if let Some((bn, _bt)) = &body_param {
            writeln!(
                &mut self.output,
                "    axum::Json({}_raw): axum::Json<serde_json::Value>,",
                bn,
            )
            .unwrap();
        }
        self.emit(") -> axum::response::Response {\n");
        self.emit("    use axum::response::IntoResponse;\n");

        // Si hay body con tipo declarado, deserializar primero. El
        // `__from_fitz_json` genérico para `Rc<RefCell<T>>` ya envuelve
        // el resultado, así que para tipos Nominal el binding queda en
        // la representación correcta (`Foo = Rc<RefCell<FooData>>`).
        if let Some((bn, bt)) = &body_param {
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

        // Llamada a la fn original.
        let call_args: Vec<String> = resolved_params
            .iter()
            .map(|(n, _)| n.clone())
            .collect();
        writeln!(
            &mut self.output,
            "    let __result = {}({});",
            name,
            call_args.join(", ")
        )
        .unwrap();

        // Convertir a response según retorne Result o no.
        if returns_result {
            self.emit("    match __result {\n");
            self.emit("        Ok(__v) => (\n");
            self.emit("            axum::http::StatusCode::OK,\n");
            self.emit("            axum::Json(__v.__to_fitz_json()),\n");
            self.emit("        ).into_response(),\n");
            self.emit("        Err(__e) => (\n");
            self.emit("            axum::http::StatusCode::INTERNAL_SERVER_ERROR,\n");
            self.emit("            axum::Json(serde_json::json!({\"error\": __e})),\n");
            self.emit("        ).into_response(),\n");
            self.emit("    }\n");
        } else {
            self.emit("    (\n");
            self.emit("        axum::http::StatusCode::OK,\n");
            self.emit("        axum::Json(__result.__to_fitz_json()),\n");
            self.emit("    ).into_response()\n");
        }
        self.emit("}\n\n");

        Ok(())
    }

    /// Genera el `#[tokio::main(flavor = "current_thread")] async fn main()`
    /// que construye el `Router` axum con cada handler registrado,
    /// parsea la addr de `@server(...)` (o usa defaults), e invoca
    /// `axum::serve`.
    ///
    /// F11: si el programa tiene `Stmt::Assign` top-level (state
    /// compartido), emitimos un `thread_local!` por cada uno antes del
    /// `fn main()`. Cada handler (y cada fn helper) materializa el
    /// state al inicio de su body via `state.with(|s| s.clone())` — un
    /// Rc clone que preserva aliasing. El tokio runtime queda en
    /// `flavor = "current_thread"` para que el thread_local funcione
    /// como global (sin él, cada worker thread tendría su propia copia).
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
    ) -> Result<(), FitzError> {
        // F11: emitir un `thread_local!` por cada state var detectado.
        // El init es la expresión RHS original del `Stmt::Assign`. El
        // `Rc::new(RefCell::new(...))` está embebido en gen_expr para
        // List/Map/Nominal — la representación matchea exactamente la
        // del intérprete y la del CLI compilado, por eso el Rc clone
        // del `.with(|s| s.clone())` preserva aliasing across handler
        // calls. Orden determinista por orden de aparición.
        if !self.state_var_types.is_empty() {
            // Reconstruimos el orden caminando main_stmts (que vienen
            // en orden de aparición). Solo emitimos para los que están
            // en state_var_types (los referenciados por al menos una
            // fn). Los `Stmt::Assign` sin referencia se ignoran — se
            // re-emiten como locales en `fn main()` (caso raro).
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
                        self.emit("thread_local! {\n");
                        writeln!(
                            &mut self.output,
                            "    static {}: {} = {};",
                            static_name, rust_ty, coerced
                        )
                        .unwrap();
                        self.emit("}\n\n");
                    }
                }
            }
        }

        // F11: tokio current_thread. Justificación arriba en el doc-
        // comment. Los `tokio::spawn` que axum hace internamente siguen
        // pidiendo `Send` sobre los futures, pero los wrappers
        // `__handler_<name>` que generamos son sync (sus locals Rc no
        // cruzan ningún `.await`), así que cumplen el bound.
        self.emit("#[tokio::main(flavor = \"current_thread\")]\nasync fn main() {\n");
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
                let path = parse_http_path(path_arg)?;
                self.emit_indent();
                writeln!(
                    &mut self.output,
                    "    .route(\"{}\", axum::routing::{}(__handler_{}))",
                    path, method, name,
                )
                .unwrap();
            }
        }
        self.emit_indent();
        self.emit(";\n");

        // Addr config.
        let cfg = server_config.clone().unwrap_or_default();
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

/// Extrae el path template de un decorator HTTP. Acepta tanto un
/// literal puro (`Expr::Str("/users/static")`) como una interpolación
/// (`Expr::StrInterp` con partes Lit + Ident: `"/users/{id}"`). En el
/// segundo caso, reconstruye el path y devuelve los nombres de los
/// params en orden. Si la interpolación tiene expresiones complejas
/// (no-Ident), error.
fn parse_http_path(expr: &Expr) -> Result<String, FitzError> {
    match expr {
        Expr::Str(s) => Ok(s.clone()),
        Expr::StrInterp(parts) => {
            use crate::ast::StrPart;
            let mut buf = String::new();
            for part in parts {
                match part {
                    StrPart::Lit(s) => buf.push_str(s),
                    StrPart::Expr(Expr::Ident(name)) => {
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

/// Preludio HTTP: traits `__ToFitzJson` y `__FromFitzJson` con impls para
/// primitivos y combinadores genéricos (`Option`, `Rc<RefCell<Vec<T>>>`,
/// `Rc<RefCell<Vec<(K, V)>>>`, `Result<T, String>`). Los impls específicos
/// por cada `type Foo` los emite `gen_type_http_impls`.
const HTTP_RUNTIME_PRELUDE: &str = r#"// --- 5b.6: runtime HTTP (serialización JSON) ---

trait __ToFitzJson {
    fn __to_fitz_json(&self) -> serde_json::Value;
}

trait __FromFitzJson: Sized {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String>;
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

impl<T: __ToFitzJson> __ToFitzJson for std::rc::Rc<std::cell::RefCell<T>> {
    fn __to_fitz_json(&self) -> serde_json::Value {
        self.borrow().__to_fitz_json()
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

impl<T: __FromFitzJson> __FromFitzJson for std::rc::Rc<std::cell::RefCell<T>> {
    fn __from_fitz_json(json: &serde_json::Value) -> Result<Self, String> {
        T::__from_fitz_json(json).map(|v| std::rc::Rc::new(std::cell::RefCell::new(v)))
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
        if matches!(callee.as_ref(), Expr::Ident(n) if n == "print"))
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
        Expr::Ident(name) => {
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
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => {}
        Expr::StrInterp(parts) => {
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
        Expr::Call { callee, args } => {
            collect_captures_expr(callee, params, locals, ctx, seen, out);
            for a in args {
                collect_captures_expr(a, params, locals, ctx, seen, out);
            }
        }
        Expr::FnExpr { params: inner_params, body } => {
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
        Expr::Index { object, index } => {
            collect_captures_expr(object, params, locals, ctx, seen, out);
            collect_captures_expr(index, params, locals, ctx, seen, out);
        }
        Expr::List(items) => {
            for it in items {
                collect_captures_expr(it, params, locals, ctx, seen, out);
            }
        }
        Expr::Map(pairs) => {
            for (k, v) in pairs {
                collect_captures_expr(k, params, locals, ctx, seen, out);
                collect_captures_expr(v, params, locals, ctx, seen, out);
            }
        }
        Expr::Range { start, end } => {
            collect_captures_expr(start, params, locals, ctx, seen, out);
            collect_captures_expr(end, params, locals, ctx, seen, out);
        }
        Expr::If { condition, then, else_ } => {
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
        Expr::Match { value, arms } => {
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
        Expr::Ok(inner) | Expr::Err(inner) | Expr::Try(inner) => {
            collect_captures_expr(inner, params, locals, ctx, seen, out);
        }
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rust_type_for(t: &Type, env: &TypeEnv) -> Result<String, FitzError> {
    match t {
        Type::Int => Ok("i64".to_string()),
        Type::Float => Ok("f64".to_string()),
        Type::Str => Ok("String".to_string()),
        Type::Bool => Ok("bool".to_string()),
        Type::Null => Ok("()".to_string()),
        Type::Nominal(id) => Ok(env.info(*id).name.clone()),
        Type::Nullable(inner) => Ok(format!("Option<{}>", rust_type_for(inner, env)?)),
        // List<T> y Map<K, V> se modelan con `Rc<RefCell<>>` para
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
            Ok(format!("Rc<RefCell<Vec<{}>>>", rust_type_for(inner, env)?))
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
                "Rc<RefCell<Vec<({}, {})>>>",
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
        // Higher-order (F12): tipo función Fitz → `Rc<dyn Fn(...) -> R>`
        // Rust. Decisión simplificadora: siempre Rc<dyn Fn>, no fn
        // pointer ni impl Fn ni Box<dyn Fn>. Trade-off: una
        // indirección por puntero por llamada, pero uniforme (vars,
        // params, returns todos toman el mismo tipo). Rc (no Box)
        // porque las funciones-como-valor se clonan al referenciarse
        // (mismo patrón que List/Map/Nominal que también van por Rc).
        // Fn (inmutable) cubre todos los ejemplos del cap 11 —
        // FnMut/FnOnce son deuda residual.
        Type::Function { params, ret } => {
            let ps: Vec<String> = params
                .iter()
                .map(|p| rust_type_for(p, env))
                .collect::<Result<_, _>>()?;
            let ret_rs = rust_type_for(ret, env)?;
            Ok(format!("Rc<dyn Fn({}) -> {}>", ps.join(", "), ret_rs))
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
        Type::List(_) => "List<...>",
        Type::Map(_, _) => "Map<...>",
        Type::Result(_) => "Result<...>",
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
        Type::List(inner) => format!("List<{}>", display_type(inner, env)),
        Type::Map(k, v) => format!("Map<{}, {}>", display_type(k, env), display_type(v, env)),
        Type::Result(inner) => format!("Result<{}>", display_type(inner, env)),
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
        // `Rc<RefCell<Vec<...>>>` — clone del Rc, barato, alias preservado.
        Type::List(_) | Type::Map(_, _) => true,
        // `Result<T, String>` no es Copy (String tampoco lo es), y el T
        // adentro puede ser Str/Nominal/List/etc. — clonamos por valor.
        Type::Result(_) => true,
        // Funciones-como-valor: `Rc<dyn Fn(...) -> R>` — clone del Rc,
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
        Type::Nominal(_) => format!("format!(\"{{}}\", &*({}).borrow())", code),
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
        // antes de hacer `.borrow()` para extender la vida del temporal
        // — `(xs.clone()).borrow()` cae con la expresión.
        Type::List(inner) => {
            // Iteramos con `.cloned()` para que `__it` sea por valor
            // (no `&T`) — uniforma el código de `show_expr_inline` con
            // el de `show_expr` general (que asume valor). El clone es
            // barato para `Rc<RefCell<...>>` (Nominal/List/Map) y vivible
            // para `String` en contexto de print.
            let item_show = show_expr_inline("__it", inner);
            format!(
                "{{ \
                    let __list = {}; \
                    let __items = __list.borrow(); \
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
                    let __pairs = __map.borrow(); \
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
            "        {{ let __t = ({}).borrow(); write!(__f, \"{{}}\", &*__t)?; }}\n",
            code
        ),
        Type::Nullable(inner) => {
            // Borroweamos el `Option<T>` y matcheamos por referencia.
            // Para Nominal adentro de Some, el match bindea `__v` como
            // `&Rc<RefCell<T>>`, así que necesitamos `(*__v)` o pasar
            // un sub-código. Para tipos primitivos, `&T` también
            // funciona porque Display está implementado para &T.
            let inner_body = match inner.as_ref() {
                Type::Int | Type::Bool => "                write!(__f, \"{}\", __v)?;\n".to_string(),
                Type::Float => "                write!(__f, \"{}\", __fitz_fmt_float(*__v))?;\n".to_string(),
                Type::Str => "                write!(__f, \"\\\"{}\\\"\", __v)?;\n".to_string(),
                Type::Null => "                write!(__f, \"null\")?;\n".to_string(),
                Type::Nominal(_) => {
                    "                { let __t = (*__v).borrow(); write!(__f, \"{}\", &*__t)?; }\n"
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
        let (env, errors) = check_program(&program);
        if !errors.is_empty() {
            panic!("checker errors: {:?}", errors);
        }
        generate_rust(&program, &env)
    }

    fn assert_contains(src: &str, fragments: &[&str]) {
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        for f in fragments {
            assert!(
                code.contains(f),
                "esperaba `{}` en la salida, no estaba.\nSalida:\n{}",
                f,
                code
            );
        }
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
    }

    #[test]
    fn programa_vacio_genera_main_vacio() {
        let code = gen("").unwrap();
        assert!(code.contains("fn main()"));
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
        assert_contains(
            "let n = 5\nlet s = \"x es {n}\"",
            &["format!(\"x es {}\", n)"],
        );
    }

    #[test]
    fn str_interp_con_var_str_clona() {
        // Para Str, generamos `.clone()` porque format! borrowea
        // pero seguimos pasando el `Ident` evaluado, que sí incluye
        // el clone.
        assert_contains(
            "let name = \"Fitz\"\nlet s = \"hola, {name}\"",
            &["format!(\"hola, {}\", name.clone())"],
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
    fn type_def_emite_struct_y_alias_rc_refcell() {
        let file = ast_test::parse(&gen("type User { id: Int, name: Str }").unwrap());
        // El struct UserData con sus dos campos.
        let s = ast_test::find_item_struct(&file, "UserData").expect("falta UserData");
        let field_names: Vec<String> = s
            .fields
            .iter()
            .filter_map(|f| f.ident.as_ref().map(|i| i.to_string()))
            .collect();
        assert_eq!(field_names, vec!["id".to_string(), "name".to_string()]);
        // El alias `type User = Rc<RefCell<UserData>>;`.
        let t = ast_test::find_item_type(&file, "User").expect("falta type alias User");
        let ty = ast_test::ts(&*t.ty);
        assert!(
            ty.contains("Rc") && ty.contains("RefCell") && ty.contains("UserData"),
            "esperaba alias `Rc<RefCell<UserData>>`, fue: {}",
            ty
        );
    }

    #[test]
    fn type_def_emite_impl_display_canonico() {
        let code = gen("type User { id: Int, name: Str }").unwrap();
        assert!(
            code.contains("impl std::fmt::Display for UserData"),
            "falta impl Display, got:\n{}",
            code
        );
        // El Display escribe `User { id: <int>, name: "<str>" }` —
        // strings con comillas adentro de la instancia (igual al
        // intérprete).
        assert!(code.contains("\"User {{\""), "falta el header del Display");
        assert!(code.contains("\"\\\"{}\\\"\""), "falta el patrón con comillas para Str");
    }

    #[test]
    fn struct_lit_emite_rc_new_refcell_new() {
        let file = ast_test::parse(
            &gen("type User { id: Int, name: Str }\nlet u = User { id: 1, name: \"x\" }")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "u").expect("falta let u");
        let init = ast_test::local_init(l).unwrap();
        // El struct lit se emite envuelto en `Rc::new(RefCell::new(UserData { ... }))`.
        assert!(
            init.contains("Rc :: new")
                && init.contains("RefCell :: new")
                && init.contains("UserData"),
            "esperaba envoltorio Rc::new(RefCell::new(UserData {{ ... }})), fue: {}",
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
        let code = gen(
            "type C { port: Int, active: Bool = true }\nlet c = C { port: 8080 }",
        )
        .unwrap();
        assert!(
            code.contains("active: true"),
            "esperaba que el default `true` esté inyectado, got:\n{}",
            code
        );
    }

    #[test]
    fn struct_lit_nullable_omitido_se_resuelve_como_none() {
        let code = gen(
            "type U { id: Int, email: Str? }\nlet u = U { id: 1 }",
        )
        .unwrap();
        assert!(
            code.contains("email: None"),
            "esperaba `email: None`, got:\n{}",
            code
        );
    }

    #[test]
    fn struct_lit_valor_str_a_campo_nullable_se_envuelve_en_some() {
        let code = gen(
            "type U { id: Int, email: Str? }\nlet u = U { id: 1, email: \"a@b\" }",
        )
        .unwrap();
        assert!(
            code.contains("email: Some(String::from(\"a@b\"))"),
            "esperaba `Some(String::from(...))`, got:\n{}",
            code
        );
    }

    #[test]
    fn struct_lit_null_literal_a_campo_nullable_es_none() {
        let code = gen(
            "type U { id: Int, email: Str? }\nlet u = U { id: 1, email: null }",
        )
        .unwrap();
        assert!(
            code.contains("email: None"),
            "esperaba `email: None`, got:\n{}",
            code
        );
    }

    #[test]
    fn field_access_int_emite_borrow_sin_clone() {
        // El receptor del field access SÍ se clona (Rc::clone, barato:
        // refcount). Lo que NO se clona es el VALOR del field — para
        // Int (Copy) no hace falta. La forma del init es entonces:
        // `(u.clone()).borrow().id` (acaba con field access, no con
        // `.clone()` al final).
        let file = ast_test::parse(
            &gen("type U { id: Int }\nlet u = U { id: 1 }\nlet n = u.id").unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        let init_expr = &l.init.as_ref().unwrap().expr;
        // Tope del init es Expr::Field (`<algo>.id`), no MethodCall a
        // `.clone()` envolviendo todo. Si fuera `<algo>.clone()`, sería
        // un `Expr::MethodCall` con method == "clone".
        match &**init_expr {
            syn::Expr::Field(fld) => match &fld.member {
                syn::Member::Named(ident) => assert_eq!(ident, "id"),
                _ => panic!("esperaba member named `id`, fue tuple-index"),
            },
            other => panic!(
                "esperaba init como Expr::Field acabando en `.id`, fue: {}",
                ast_test::ts(other)
            ),
        }
    }

    #[test]
    fn field_access_str_emite_borrow_clone() {
        let file = ast_test::parse(
            &gen("type U { name: Str }\nlet u = U { name: \"x\" }\nlet s = u.name")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "s").expect("falta let s");
        let init = ast_test::local_init(l).unwrap();
        // Str no es Copy → borrow + clone.
        assert!(
            init.contains("borrow") && init.contains("clone"),
            "esperaba `borrow` y `clone` en el field access de Str, fue: {}",
            init
        );
    }

    #[test]
    fn field_assign_emite_borrow_mut() {
        let file = ast_test::parse(
            &gen("type U { name: Str }\nlet u = U { name: \"x\" }\nu.name = \"y\"")
                .unwrap(),
        );
        let stmts = ast_test::main_block_stmts(&file);
        // Buscamos un `borrow_mut()` en el árbol — lo emite el field assign.
        assert!(
            ast_test::contains_method_call(stmts, "borrow_mut"),
            "esperaba call a `.borrow_mut()` para field assign"
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
        assert!(
            code.contains("f(u.clone())"),
            "esperaba `f(u.clone())`, got:\n{}",
            code
        );
    }

    #[test]
    fn print_de_instance_usa_show_expr_con_display() {
        // `print(u)` para u: U → format!("{}", &*u.borrow()) dentro
        // del println!.
        let code = gen(
            "type U { id: Int }\nlet u = U { id: 1 }\nprint(u)",
        )
        .unwrap();
        assert!(
            code.contains("format!(\"{}\", &*"),
            "esperaba `format!(\"{{}}\", &*(...).borrow())`, got:\n{}",
            code
        );
        assert!(
            code.contains(".borrow())"),
            "esperaba `.borrow())` en el print, got:\n{}",
            code
        );
    }

    #[test]
    fn tipo_anidado_compila_con_nullable_de_nominal() {
        // `type Order { user: User? }` se traduce a un campo de tipo
        // `Option<User>` (= `Option<Rc<RefCell<UserData>>>`).
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
    fn igualdad_estructural_entre_instancias_emite_borrow_eq() {
        let code = gen(
            "type U { id: Int }\nlet a = U { id: 1 }\nlet b = U { id: 1 }\nlet eq = a == b",
        )
        .unwrap();
        assert!(
            code.contains(").borrow() == *(") || code.contains(".borrow() == *"),
            "esperaba comparación con `*x.borrow() == *y.borrow()`, got:\n{}",
            code
        );
    }

    // ---- 5b.2+: if como expresión con valor ----

    #[test]
    fn if_como_expresion_emite_branches_sin_punto_y_coma() {
        let code = gen("let x = if (true) { 1 } else { 2 }").unwrap();
        // El bloque del if tiene su última expresión sin `;` para que
        // el `if` evalúe a un valor (`1` o `2`).
        assert!(
            code.contains("(if true {") || code.contains("(if (true)"),
            "esperaba un if-expression envuelto en paréntesis, got:\n{}",
            code
        );
        assert!(
            code.contains("1i64\n") && code.contains("2i64\n"),
            "esperaba `1i64` y `2i64` como tail sin `;`, got:\n{}",
            code
        );
        // x debe quedar como i64.
        assert!(
            code.contains("let mut x: i64 = "),
            "esperaba `let mut x: i64 = ...`, got:\n{}",
            code
        );
    }

    #[test]
    fn if_expresion_unifica_int_float_a_float() {
        let code = gen("let x = if (true) { 1 } else { 2.5 }").unwrap();
        assert!(
            code.contains("let mut x: f64 = "),
            "esperaba `x: f64`, got:\n{}",
            code
        );
        // La rama Int se coerciona explícitamente: `(1i64 as f64)`.
        assert!(
            code.contains("(1i64 as f64)"),
            "esperaba coerción Int→Float en la rama then, got:\n{}",
            code
        );
    }

    #[test]
    fn if_como_sentencia_mantiene_comportamiento_anterior() {
        // Sin asignar y con `print` adentro: el if sigue siendo
        // statement; print no se trata como tail expression
        // (no es una expresión con valor en Fitz).
        let code = gen("if (true) { print(\"a\") } else { print(\"b\") }").unwrap();
        // Cada print queda emitido con `;` final (terminator de stmt).
        assert!(
            code.contains("println!(\"{}\", String::from(\"a\"));"),
            "esperaba print como stmt con `;`, got:\n{}",
            code
        );
        assert!(
            code.contains("println!(\"{}\", String::from(\"b\"));"),
            "esperaba print como stmt con `;` en else, got:\n{}",
            code
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
    fn type_def_emite_derive_partialeq() {
        let file = ast_test::parse(&gen("type U { id: Int }").unwrap());
        let s = ast_test::find_item_struct(&file, "UData").expect("falta UData");
        assert!(
            ast_test::struct_has_derive(s, "Clone"),
            "esperaba derive(Clone)"
        );
        assert!(
            ast_test::struct_has_derive(s, "PartialEq"),
            "esperaba derive(PartialEq)"
        );
    }

    // ---- 5b.3: listas, mapas, indexing, métodos built-in ----

    #[test]
    fn list_literal_emite_rc_refcell_vec() {
        // `[1, 2, 3]` se modela como `Rc<RefCell<Vec<i64>>>`. Los items
        // se coercen al tipo común (acá Int → i64) y se construye con
        // el macro vec![].
        let file = ast_test::parse(&gen("let xs: List<Int> = [1, 2, 3]").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "xs").expect("falta let xs");
        let ty = ast_test::local_type(l).unwrap();
        assert!(
            ty.contains("Rc") && ty.contains("RefCell") && ty.contains("Vec") && ty.contains("i64"),
            "esperaba tipo `Rc<RefCell<Vec<i64>>>`, fue: {}",
            ty
        );
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("Rc :: new") && init.contains("RefCell :: new") && init.contains("vec !"),
            "esperaba `Rc::new(RefCell::new(vec![...]))`, fue: {}",
            init
        );
        // Los 3 items con sufijo `i64`.
        assert!(
            init.contains("1i64") && init.contains("2i64") && init.contains("3i64"),
            "esperaba items 1i64, 2i64, 3i64, fue: {}",
            init
        );
    }

    #[test]
    fn list_literal_homogeneo_int_float_promueve_a_float() {
        // Int+Float en la misma lista → `List<Float>` (mismo lub que
        // if-expression y FnExpr ret).
        let code = gen("let xs = [1, 2.5, 3]").unwrap();
        assert!(
            code.contains("Rc<RefCell<Vec<f64>>>"),
            "esperaba List<f64>, got:\n{}",
            code
        );
        assert!(
            code.contains("(1i64 as f64)") && code.contains("(3i64 as f64)"),
            "esperaba coerción Int→Float en los items, got:\n{}",
            code
        );
    }

    #[test]
    fn list_literal_vacia_es_list_any_a_resolver_por_contexto() {
        // `[]` sin contexto da `List<Any>`. Con anotación, el contexto
        // restringe a List<T> y el `Vec::new()` infiere desde el target.
        let code = gen("let xs: List<Int> = []").unwrap();
        assert!(
            code.contains("let mut xs: Rc<RefCell<Vec<i64>>>"),
            "esperaba `List<Int>` por anotación, got:\n{}",
            code
        );
        assert!(
            code.contains("Rc::new(RefCell::new(Vec::new()))"),
            "esperaba `Rc::new(RefCell::new(Vec::new()))` para lista vacía, got:\n{}",
            code
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
        assert_contains(
            "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}",
            &[
                "let mut m: Rc<RefCell<Vec<(String, i64)>>>",
                "(String::from(\"a\"), 1i64)",
                "(String::from(\"b\"), 2i64)",
            ],
        );
    }

    #[test]
    fn map_literal_vacio_resuelto_por_anotacion() {
        let code = gen("let m: Map<Str, Int> = {}").unwrap();
        assert!(
            code.contains("let mut m: Rc<RefCell<Vec<(String, i64)>>>"),
            "esperaba `Map<Str, Int>` por anotación, got:\n{}",
            code
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
        // `xs[0]` → `(xs.clone()).borrow()[(0i64) as usize].clone()`.
        // El `.clone()` final es del Rc para Nominal/List/Map o copy
        // para primitivos — siempre seguro.
        let code = gen("let xs: List<Int> = [10, 20]\nlet x = xs[0]").unwrap();
        assert!(
            code.contains(".borrow()[(0i64) as usize].clone()"),
            "esperaba acceso por borrow + index + clone, got:\n{}",
            code
        );
        assert!(
            code.contains("let mut x: i64 ="),
            "esperaba que x quede tipado como i64, got:\n{}",
            code
        );
    }

    #[test]
    fn map_indexing_emite_busqueda_lineal_con_panic() {
        // `m["a"]` → bloque que linea la búsqueda y paniquea si falta.
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet n = m[\"a\"]",
        )
        .unwrap();
        assert!(
            code.contains(".find(|(__k2, _)| __k2 == &__k)"),
            "esperaba búsqueda lineal en map, got:\n{}",
            code
        );
        assert!(
            code.contains("clave no encontrada en mapa"),
            "esperaba mensaje de panic con texto del intérprete, got:\n{}",
            code
        );
    }

    #[test]
    fn for_sobre_list_genera_snapshot_iter() {
        // `for v in xs` → snapshot via `borrow().clone().into_iter()`
        // (evita re-entrancia si el body muta `xs`).
        let code = gen(
            "let xs: List<Int> = [1, 2, 3]\nfor v in xs { print(v) }",
        )
        .unwrap();
        assert!(
            code.contains(".borrow().clone().into_iter()"),
            "esperaba snapshot iter, got:\n{}",
            code
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
    fn list_push_emite_borrow_mut_push() {
        let file = ast_test::parse(&gen("let xs: List<Int> = []\nxs.push(7)").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        // `.push(...)` se emite como method call sobre `borrow_mut()`.
        assert!(
            ast_test::contains_method_call(stmts, "borrow_mut"),
            "esperaba `borrow_mut` antes del push"
        );
        assert!(
            ast_test::contains_method_call(stmts, "push"),
            "esperaba `.push(...)`"
        );
    }

    #[test]
    fn list_pop_emite_borrow_mut_pop_con_expect() {
        let file = ast_test::parse(&gen("let xs: List<Int> = [1]\nlet x = xs.pop()").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "x").expect("falta let x");
        let init = ast_test::local_init(l).unwrap();
        // El pop se traduce a `.borrow_mut().pop().expect("...")` —
        // `.expect(...)` paniquea con el mismo mensaje del intérprete.
        assert!(
            init.contains("borrow_mut") && init.contains("pop") && init.contains("expect"),
            "esperaba pipeline borrow_mut + pop + expect, fue: {}",
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
    fn list_len_metodo_emite_borrow_len_as_i64() {
        let file = ast_test::parse(&gen("let xs: List<Int> = []\nlet n = xs.len()").unwrap());
        let stmts = ast_test::main_block_stmts(&file);
        let l = ast_test::find_let(stmts, "n").expect("falta let n");
        // El binding `n` debe quedar tipado como i64.
        assert_eq!(ast_test::local_type(l).as_deref(), Some("i64"));
        let init = ast_test::local_init(l).unwrap();
        assert!(
            init.contains("borrow") && init.contains("len") && init.contains("as i64"),
            "esperaba pipeline borrow + len + as i64, fue: {}",
            init
        );
    }

    #[test]
    fn len_builtin_global_sobre_list_resuelve_a_borrow_len() {
        // `len(xs)` despacha por tipo del argumento — mismo código que
        // `xs.len()` para List/Map; para Str sigue siendo chars().count.
        let code = gen("let xs: List<Int> = [1]\nlet n = len(xs)").unwrap();
        assert!(
            code.contains(".borrow().len() as i64"),
            "esperaba `.borrow().len() as i64` desde el builtin global, got:\n{}",
            code
        );
    }

    #[test]
    fn len_builtin_global_sobre_str_usa_chars_count() {
        let code = gen("let s = \"hola\"\nlet n = len(s)").unwrap();
        assert!(
            code.contains(".chars().count() as i64"),
            "esperaba `.chars().count() as i64`, got:\n{}",
            code
        );
    }

    #[test]
    fn list_map_con_fnexpr_inline_emite_closure() {
        let code = gen(
            "let xs: List<Int> = [1, 2, 3]\nlet ys = xs.map(fn(x) => x * 2)",
        )
        .unwrap();
        assert!(
            code.contains(".into_iter().map(|x: i64| -> i64"),
            "esperaba closure inline `|x: i64| -> i64`, got:\n{}",
            code
        );
        assert!(
            code.contains("Rc::new(RefCell::new"),
            "esperaba envoltorio Rc::new(RefCell::new(...)), got:\n{}",
            code
        );
        assert!(
            code.contains("let mut ys: Rc<RefCell<Vec<i64>>>"),
            "esperaba que `ys` quede tipado `List<Int>`, got:\n{}",
            code
        );
    }

    #[test]
    fn list_filter_con_fnexpr_inline_emite_for_manual() {
        // Filter usa un for manual (no .filter()) porque el callback
        // toma T por valor pero `Iterator::filter` quiere &T.
        let code = gen(
            "let xs: List<Int> = [1, 2, 3]\nlet ys = xs.filter(fn(x) => x > 1)",
        )
        .unwrap();
        assert!(
            code.contains("let __cb = |x: i64| -> bool"),
            "esperaba binding del callback como `|x: i64| -> bool`, got:\n{}",
            code
        );
        assert!(
            code.contains("if __cb(__it.clone())"),
            "esperaba aplicación del cb con clone, got:\n{}",
            code
        );
    }

    #[test]
    fn map_method_chaining_funciona() {
        // `xs.map(f).map(g)` debe poder componerse. El test es de
        // estructura: el tipo de salida del primer map alimenta al
        // siguiente sin friction.
        let code = gen(
            "let xs: List<Int> = [1, 2]\n\
             let ys = xs.map(fn(x) => x * 2).map(fn(x) => x + 1)",
        )
        .unwrap();
        assert!(
            code.matches(".into_iter().map(|x: i64| -> i64").count() >= 2,
            "esperaba dos map closures encadenados, got:\n{}",
            code
        );
    }

    #[test]
    fn map_has_emite_iter_any() {
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet b = m.has(\"a\")",
        )
        .unwrap();
        assert!(
            code.contains(".iter().any(|(__k2, _)| __k2 == &__k)"),
            "esperaba `.iter().any(...)`, got:\n{}",
            code
        );
    }

    #[test]
    fn map_keys_emite_lista_nueva_de_claves() {
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}\nlet ks = m.keys()",
        )
        .unwrap();
        assert!(
            code.contains(".iter().map(|(__k, _)| __k.clone()).collect::<Vec<_>>()"),
            "esperaba pipeline de keys, got:\n{}",
            code
        );
        assert!(
            code.contains("let mut ks: Rc<RefCell<Vec<String>>>"),
            "esperaba que keys retorne List<Str>, got:\n{}",
            code
        );
    }

    #[test]
    fn map_values_emite_lista_nueva_de_valores() {
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet vs = m.values()",
        )
        .unwrap();
        assert!(
            code.contains(".iter().map(|(_, __v)| __v.clone()).collect::<Vec<_>>()"),
            "esperaba pipeline de values, got:\n{}",
            code
        );
        assert!(
            code.contains("let mut vs: Rc<RefCell<Vec<i64>>>"),
            "esperaba que values retorne List<Int>, got:\n{}",
            code
        );
    }

    #[test]
    fn map_len_metodo_emite_borrow_len_as_i64() {
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet n = m.len()",
        )
        .unwrap();
        assert!(
            code.contains(".borrow().len() as i64"),
            "esperaba `.borrow().len() as i64`, got:\n{}",
            code
        );
    }

    #[test]
    fn list_find_emite_result_con_loop() {
        // 5b.4: find devuelve `Result<T, String>` con Ok(item) al primer
        // match y `Err("no encontrado")` si nada matchea. Tipado del
        // binding `x` debe ser `Result<i64, String>`.
        let code = gen(
            "let xs: List<Int> = [1, 2]\nlet x = xs.find(fn(n) => n > 0)",
        )
        .unwrap();
        assert!(
            code.contains("let mut x: Result<i64, String>"),
            "esperaba `x: Result<i64, String>`, got:\n{}",
            code
        );
        assert!(
            code.contains("Err(String::from(\"no encontrado\"))"),
            "esperaba inicializador con `Err(\"no encontrado\")`, got:\n{}",
            code
        );
        assert!(
            code.contains("__result = Ok(__it); break;"),
            "esperaba la asignación de Ok + break en el loop, got:\n{}",
            code
        );
    }

    #[test]
    fn map_get_emite_result_con_busqueda_lineal() {
        // 5b.4: get devuelve `Result<V, String>`. Mensaje del Err matchea
        // bit-a-bit el del intérprete: `clave no encontrada: <k>` con `<k>`
        // formateado inline (Str con comillas).
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet v = m.get(\"a\")",
        )
        .unwrap();
        assert!(
            code.contains("let mut v: Result<i64, String>"),
            "esperaba `v: Result<i64, String>`, got:\n{}",
            code
        );
        assert!(
            code.contains("clave no encontrada: {}"),
            "esperaba mensaje `clave no encontrada: {{}}`, got:\n{}",
            code
        );
        assert!(
            code.contains("__result = Ok(__v.clone()); break;"),
            "esperaba asignación de Ok + break, got:\n{}",
            code
        );
    }

    #[test]
    fn fnexpr_suelta_emite_rc_dyn_fn() {
        // F12: FnExpr asignado a var emite `Rc::new(move |...| ...) as
        // Rc<dyn Fn(...) -> ...>`. La var queda tipada como
        // `Rc<dyn Fn(i64) -> i64>` y se puede invocar con `f(x)`.
        let code = gen("let f: Fn(Int) -> Int = fn(x: Int) => x * 2\nprint(f(3))").unwrap();
        assert!(
            code.contains("Rc<dyn Fn(i64) -> i64>"),
            "esperaba tipo Rc<dyn Fn> en el código, got:\n{}",
            code
        );
        assert!(
            code.contains("Rc::new(move |x: i64|"),
            "esperaba `Rc::new(move |x: i64| ...)` para el closure, got:\n{}",
            code
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
    fn fn_nombrada_como_valor_emite_rc_new() {
        // F12: `let g = square` donde `square` es fn top-level emite
        // `Rc::new(square) as Rc<dyn Fn(...) -> R>` con la firma del
        // fn_sigs.
        let code = gen(
            "fn square(n: Int) -> Int => n * n\nlet g: Fn(Int) -> Int = square\nprint(g(7))",
        )
        .unwrap();
        assert!(
            code.contains("Rc::new(square)"),
            "esperaba `Rc::new(square)`, got:\n{}",
            code
        );
        assert!(
            code.contains("Rc<dyn Fn(i64) -> i64>"),
            "esperaba tipo Rc<dyn Fn(i64) -> i64>, got:\n{}",
            code
        );
    }

    #[test]
    fn fn_param_de_tipo_funcion_emite_rc_dyn_fn() {
        // F12: param `f: Fn(Int) -> Int` en la firma de la fn top-level
        // debe traducirse a `Rc<dyn Fn(i64) -> i64>` en el header.
        let code = gen(
            "fn apply(f: Fn(Int) -> Int, x: Int) -> Int => f(x)\n\
             fn square(n: Int) -> Int => n * n\n\
             print(apply(square, 7))",
        )
        .unwrap();
        assert!(
            code.contains("fn apply(mut f: Rc<dyn Fn(i64) -> i64>, mut x: i64) -> i64"),
            "esperaba header de apply con `Rc<dyn Fn>`, got:\n{}",
            code
        );
        // La llamada `apply(square, 7)` debe envolver `square` en Rc::new.
        assert!(
            code.contains("apply((Rc::new(square)"),
            "esperaba `apply((Rc::new(square) as ...)`, got:\n{}",
            code
        );
    }

    #[test]
    fn fn_como_return_type_emite_rc_dyn_fn() {
        // F12: `-> Fn(Int) -> Int` en una fn top-level emite el header
        // con retorno `Rc<dyn Fn(i64) -> i64>`. La closure interna que
        // captura `x` se traduce con `move`.
        let code = gen(
            "fn make_adder(x: Int) -> Fn(Int) -> Int {\n\
                 return fn(y: Int) => x + y\n\
             }\n\
             let add5: Fn(Int) -> Int = make_adder(5)\n\
             print(add5(3))",
        )
        .unwrap();
        assert!(
            code.contains("fn make_adder(mut x: i64) -> Rc<dyn Fn(i64) -> i64>"),
            "esperaba header de make_adder con retorno Rc<dyn Fn>, got:\n{}",
            code
        );
        assert!(
            code.contains("Rc::new(move |y: i64|"),
            "esperaba closure con `move` capturando x, got:\n{}",
            code
        );
    }

    #[test]
    fn closure_que_captura_var_no_copy_clona_afuera() {
        // F12: closure que captura una var no-Copy (Str). El codegen
        // debe emitir `let saludo = saludo.clone();` afuera para
        // preservar el aliasing semántico sin consumir la var del
        // caller.
        let code = gen(
            "let saludo = \"hola\"\n\
             let f: Fn(Str) -> Str = fn(n: Str) => \"{saludo}, {n}!\"\n\
             print(f(\"Fitz\"))",
        )
        .unwrap();
        assert!(
            code.contains("let saludo = saludo.clone();"),
            "esperaba clone de la captura antes del Rc::new, got:\n{}",
            code
        );
        assert!(
            code.contains("Rc::new(move |n: String|"),
            "esperaba closure con `move`, got:\n{}",
            code
        );
    }

    #[test]
    fn var_de_tipo_funcion_se_llama_con_parens() {
        // F12: `f(x)` sobre una var Fn(Int) -> Int se traduce literal a
        // `f(x)` Rust — el auto-deref de `Rc<dyn Fn>` lo resuelve.
        let code = gen(
            "let f: Fn(Int) -> Int = fn(n: Int) => n + 1\nprint(f(10))",
        )
        .unwrap();
        assert!(
            code.contains("f(10i64)"),
            "esperaba `f(10i64)`, got:\n{}",
            code
        );
    }

    #[test]
    fn fn_anonima_inline_como_arg_emite_closure_directo() {
        // F12: `apply(fn(n: Int) => n * 10, 7)` no envuelve en una var
        // intermedia — emite el `Rc::new(move |n: i64| ...)` inline
        // como argumento.
        let code = gen(
            "fn apply(f: Fn(Int) -> Int, x: Int) -> Int => f(x)\n\
             print(apply(fn(n: Int) => n * 10, 7))",
        )
        .unwrap();
        assert!(
            code.contains("apply((Rc::new(move |n: i64|"),
            "esperaba el FnExpr emitido inline como arg, got:\n{}",
            code
        );
    }

    #[test]
    fn print_de_lista_emite_iter_inline() {
        // El print/interp construye el string `[a, b, c]` en runtime
        // ligando primero el Rc a una var (vida del temporal).
        let code = gen("let xs: List<Int> = [1, 2]\nprint(xs)").unwrap();
        assert!(
            code.contains("let __list = "),
            "esperaba binding del Rc antes del borrow, got:\n{}",
            code
        );
        assert!(
            code.contains("String::from(\"[\")"),
            "esperaba header `[` para lista, got:\n{}",
            code
        );
    }

    #[test]
    fn print_de_mapa_emite_iter_inline_con_llaves() {
        let code = gen("let m: Map<Str, Int> = {\"a\": 1}\nprint(m)").unwrap();
        assert!(
            code.contains("let __map = "),
            "esperaba binding del Rc antes del borrow, got:\n{}",
            code
        );
        assert!(
            code.contains("String::from(\"{\")"),
            "esperaba header `{{` para mapa, got:\n{}",
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

    /// Helper: genera el código de un programa con HTTP y verifica que
    /// los fragmentos esperados estén presentes. Replica `assert_contains`
    /// pero pasa por `generate_main_rs` (que decide el modo HTTP).
    fn assert_http_contains(src: &str, fragments: &[&str]) {
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        for f in fragments {
            assert!(
                code.contains(f),
                "esperaba `{}` en la salida, no estaba.\nSalida:\n{}",
                f,
                code
            );
        }
    }

    #[test]
    fn http_main_emite_tokio_main_async() {
        // F11: tokio runtime queda en `flavor = "current_thread"` para
        // que el `thread_local!` del state compartido funcione como
        // global. Los handlers Fitz son sync (sus locals Rc no cruzan
        // ningún `.await`), así que cumplen el bound `Send` que axum
        // exige sobre los futures.
        let src = "@server(3000) fn main() => 0\n\
                   @get(\"/\") fn index() -> Str => \"ok\"";
        assert_http_contains(
            src,
            &[
                "#[tokio::main(flavor = \"current_thread\")]",
                "async fn main()",
                "axum::Router::new()",
            ],
        );
    }

    #[test]
    fn http_router_registra_ruta_get() {
        let src = "@get(\"/users\") fn list_users() -> Str => \"[]\"";
        assert_http_contains(
            src,
            &[
                ".route(\"/users\", axum::routing::get(__handler_list_users))",
                "async fn __handler_list_users(",
            ],
        );
    }

    #[test]
    fn http_path_param_int_genera_extract_path() {
        let src = "@get(\"/u/{id}\") fn get_user(id: Int) -> Str => \"x\"";
        assert_http_contains(
            src,
            &["axum::extract::Path(id): axum::extract::Path<i64>"],
        );
    }

    #[test]
    fn http_path_param_str_genera_extract_path_string() {
        let src = "@get(\"/u/{name}\") fn greet(name: Str) -> Str => name";
        assert_http_contains(
            src,
            &["axum::extract::Path(name): axum::extract::Path<String>"],
        );
    }

    #[test]
    fn http_handler_result_emite_match_ok_err() {
        let src = "@get(\"/d/{n}\") fn divide(n: Int) -> Result<Int> { return Ok(n * 2) }";
        let code = gen(src).unwrap();
        assert!(
            code.contains("Ok(__v)") && code.contains("Err(__e)"),
            "esperaba match Ok/Err en handler, got:\n{}",
            code
        );
        assert!(
            code.contains("StatusCode::OK") && code.contains("StatusCode::INTERNAL_SERVER_ERROR"),
            "esperaba status codes 200/500, got:\n{}",
            code
        );
    }

    #[test]
    fn http_body_post_con_tipo_emite_from_fitz_json() {
        let src = "type Input { msg: Str }\n\
                   @post(\"/echo\") fn echo(body: Input) -> Input => body";
        let code = gen(src).unwrap();
        assert!(
            code.contains("axum::Json(body_raw): axum::Json<serde_json::Value>"),
            "esperaba extractor body_raw, got:\n{}",
            code
        );
        assert!(
            code.contains("__FromFitzJson>::__from_fitz_json(&body_raw)"),
            "esperaba __from_fitz_json para deserializar, got:\n{}",
            code
        );
        assert!(
            code.contains("StatusCode::BAD_REQUEST"),
            "esperaba 400 si la deserialización falla, got:\n{}",
            code
        );
    }

    #[test]
    fn http_server_decorator_setea_addr() {
        let src = "@server(8080, \"0.0.0.0\") fn main() => 0\n\
                   @get(\"/\") fn index() -> Str => \"ok\"";
        assert_http_contains(src, &["\"0.0.0.0:8080\".parse()"]);
    }

    #[test]
    fn http_sin_server_decorator_usa_default_3000() {
        let src = "@get(\"/\") fn index() -> Str => \"ok\"";
        assert_http_contains(src, &["\"127.0.0.1:3000\".parse()"]);
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
    fn http_state_compartido_emite_thread_local() {
        // F11 — antes (5b.6) este caso abortaba con "state compartido
        // no soportado". Ahora el codegen emite un `thread_local!` por
        // cada state var detectado, y cada fn que la referencia
        // materializa la Rc al inicio del body via `.with(|s| s.clone())`.
        let src = "let users = [1, 2, 3]\n\
                   @get(\"/users\") fn list_users() -> List<Int> => users";
        let code = gen(src).unwrap();
        assert!(
            code.contains("thread_local!"),
            "esperaba bloque `thread_local!`, got:\n{}",
            code
        );
        assert!(
            code.contains("__FITZ_STATE_USERS"),
            "esperaba el static __FITZ_STATE_USERS, got:\n{}",
            code
        );
        assert!(
            code.contains("__FITZ_STATE_USERS.with(|__s| __s.clone())"),
            "esperaba la materialización con `.with(|__s| __s.clone())`, got:\n{}",
            code
        );
    }

    #[test]
    fn http_state_no_referenciado_no_se_promueve_a_thread_local() {
        // Si una var top-level NO es referenciada por ninguna fn HTTP,
        // no es state compartido — se queda como var local en `fn main()`.
        let src = "let ignorada = 42\n\
                   @get(\"/\") fn index() -> Str => \"ok\"";
        let code = gen(src).unwrap();
        assert!(
            !code.contains("thread_local!"),
            "no esperaba thread_local para una var sin refs, got:\n{}",
            code
        );
        assert!(
            !code.contains("__FITZ_STATE_IGNORADA"),
            "no esperaba el static, got:\n{}",
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
        let (env, errs) = crate::types::check_program(&program);
        assert!(errs.is_empty(), "checker errors: {:?}", errs);
        let project = generate_project(Path::new("test.fitz"), &program, &env).unwrap();
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
        let (env, errs) = crate::types::check_program(&program);
        assert!(errs.is_empty());
        let project = generate_project(Path::new("test.fitz"), &program, &env).unwrap();
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
        assert!(
            code.contains("impl __ToFitzJson for UserData"),
            "esperaba impl ToFitzJson para UserData, got:\n{}",
            code
        );
        assert!(
            code.contains("impl __FromFitzJson for UserData"),
            "esperaba impl FromFitzJson para UserData, got:\n{}",
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
        let code = gen(
            "fn divide(a: Int, b: Int) -> Result<Int> { return Ok(a / b) }",
        )
        .unwrap();
        assert!(
            code.contains("-> Result<i64, String>"),
            "esperaba return type `Result<i64, String>`, got:\n{}",
            code
        );
    }

    #[test]
    fn ok_constructor_emite_ok_envoltorio() {
        let code = gen(
            "fn ok42() -> Result<Int> { return Ok(42) }",
        )
        .unwrap();
        assert!(
            code.contains("return Ok(42i64);"),
            "esperaba `return Ok(42i64);`, got:\n{}",
            code
        );
    }

    #[test]
    fn err_con_str_literal_emite_string_from() {
        let code = gen(
            "fn boom() -> Result<Int> { return Err(\"explotó\") }",
        )
        .unwrap();
        assert!(
            code.contains("return Err(String::from(\"explotó\"));"),
            "esperaba `return Err(String::from(\"explotó\"));`, got:\n{}",
            code
        );
    }

    #[test]
    fn err_con_no_str_coerciona_via_format() {
        // Err(42): el Err side está pinned a String, así que se coerce
        // con format!. Cambio de comportamiento sutil pero documentado.
        let code = gen(
            "fn boom() -> Result<Str> { return Err(42) }",
        )
        .unwrap();
        assert!(
            code.contains("Err(format!(\"{}\", 42i64))"),
            "esperaba coerción a String via format!, got:\n{}",
            code
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
        assert!(
            code.contains("(find_user(id))?"),
            "esperaba `(find_user(id))?` en describe, got:\n{}",
            code
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
        assert!(
            code.contains("Ok(u) =>"),
            "esperaba arm `Ok(u) =>`, got:\n{}",
            code
        );
        // El cuerpo del arm debe poder hacer `u.id` (lo que requiere
        // que `u` esté declarado con tipo `User`).
        assert!(
            code.contains(".borrow().id"),
            "esperaba field access sobre el binding `u`, got:\n{}",
            code
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
        assert!(
            code.contains("Ok(__v) => format!(\"Ok({})\""),
            "esperaba match inline con `Ok(__v) => format!(\"Ok({{}})\", ...)`, got:\n{}",
            code
        );
        assert!(
            code.contains("Err(__e) => format!(\"Err(\\\"{}\\\")\", __e)"),
            "esperaba arm Err con comillas dobles alrededor del mensaje, got:\n{}",
            code
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
        // El find emite un bloque Result<User, String>; el `?` Rust se
        // aplica al bloque entero.
        assert!(
            code.contains("})?"),
            "esperaba que el bloque del find termine con `}})?` (operador ? sobre Result), got:\n{}",
            code
        );
    }

    // ---- 5b.5: módulos / import ----------------------------------------

    /// Genera el código de un programa "main" tratándolo como un módulo
    /// importado (sin loader externo). Útil para validar el codegen
    /// de un módulo independientemente del orquestador.
    fn gen_module(src: &str) -> Result<String, FitzError> {
        let tokens = crate::lexer::tokenize(src).expect("lex OK");
        let program = crate::parser::parse(tokens).expect("parse OK");
        let (env, errors) = crate::types::check_program(&program);
        if !errors.is_empty() {
            panic!("checker errors: {:?}", errors);
        }
        generate_module_rs(&program, &env)
    }

    #[test]
    fn modulo_emite_pub_en_struct_y_alias() {
        // Un módulo expone tipos custom con `pub` en struct + alias.
        let code = gen_module("type User { id: Int, name: Str }").unwrap();
        assert!(
            code.contains("pub struct UserData"),
            "esperaba `pub struct UserData`, got:\n{}",
            code
        );
        assert!(
            code.contains("pub type User = "),
            "esperaba `pub type User = ...`, got:\n{}",
            code
        );
    }

    #[test]
    fn modulo_emite_pub_en_fn() {
        let code = gen_module("fn add(a: Int, b: Int) -> Int => a + b").unwrap();
        assert!(
            code.contains("pub fn add("),
            "esperaba `pub fn add(`, got:\n{}",
            code
        );
    }

    #[test]
    fn modulo_let_str_top_level_se_emite_como_pub_static() {
        let code = gen_module("let MSG = \"hola\"").unwrap();
        assert!(
            code.contains("pub static MSG: &str = \"hola\""),
            "esperaba `pub static MSG: &str = \"hola\"`, got:\n{}",
            code
        );
    }

    #[test]
    fn modulo_let_int_top_level_se_emite_como_pub_const() {
        let code = gen_module("let MAX_RETRIES: Int = 5").unwrap();
        assert!(
            code.contains("pub const MAX_RETRIES: i64 = 5i64"),
            "esperaba `pub const MAX_RETRIES: i64 = 5i64`, got:\n{}",
            code
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
        assert!(
            code.contains("pub static PREFIX: &str = \"hola, \""),
            "esperaba `pub static PREFIX`, got:\n{}",
            code
        );
        assert!(
            code.contains("pub fn greet(mut name: String) -> String"),
            "esperaba `pub fn greet(mut name: String) -> String`, got:\n{}",
            code
        );
        assert!(
            code.contains("String::from(PREFIX)"),
            "esperaba que el body use `String::from(PREFIX)`, got:\n{}",
            code
        );
    }

    #[test]
    fn match_range_emite_guard_con_contains() {
        // Pattern de rango `0..10` → guard con `(0..10).contains(&__n)`.
        let code = gen(
            "let n = 5\nlet s = match n { 0..10 => \"chico\", _ => \"grande\" }",
        )
        .unwrap();
        assert!(
            code.contains("__n if (0i64..10i64).contains(&__n)"),
            "esperaba guard de rango, got:\n{}",
            code
        );
    }
}
