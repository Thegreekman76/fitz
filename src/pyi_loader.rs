// pyi_loader.rs — Auto-pickup of `.pyi` stubs adjacent to the root .fitz.
//
// Phase 8-pyi.B (v0.9.57): when the program contains `from python
// import foo` (or `from python import foo as bar`), we look for
// `<base_dir>/foo.pyi` adjacent to the `.fitz` file that starts
// execution. If it exists, we parse with `pyi_stub::parse_stub` and
// register the declared nominals in the `TypeEnv` BEFORE invoking
// the checker. Result: the user can write `let u: User =
// requests.fetch(...)?` with `User` declared in `requests.pyi`, and the
// checker resolves the nominal instead of failing with "unknown
// type".
//
// **Error policy** (silent fallback):
// - `foo.pyi` does not exist → no stub, binding stays as
//   opaque `Type::PyAny` (current behavior).
// - `foo.pyi` exists but fails to parse → warning to stderr,
//   binding stays as `Type::PyAny` (does not break the build).
// - `foo.pyi` parses OK but an item has a non-resolvable type →
//   `stub_type_to_fitz_type` already returns `Type::Any` as gradual
//   fallback (zero extra overhead).
//
// **Search policy** (8-pyi.B decision): only adjacent to the
// root `.fitz`. We do NOT walk `PYTHONPATH` or `site-packages`. The idea
// is that the user copies/generates the project-local `.pyi` (via
// `fitz py-stubs <file.py>`) and commits it — reproducible
// parity across machines, zero environment magic.
//
// **MVP coverage**:
// - Nominals (`class Foo:`): registered in `TypeEnv` with fields. ✓
// - Fns (`def name(args) -> ret`): resolved for future use (8-pyi.C
//   typed field access), not yet bound as Function to the checker's
//   scope in this sub-step.
// - Top-level vars (`name: type`): same as fns — resolved but not
//   bound to scope in 8-pyi.B.
//
// The `foo` binding of `from python import foo` still types as
// `Type::PyAny` in 8-pyi.B; typed field access (`foo.bar`) arrives
// in 8-pyi.C.

use crate::ast::{Program, Stmt};
use crate::pyi_stub::{self, ResolvedStubItem, StubItem};
use crate::types::{ResolvedField, Type, TypeEnv};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Stub loaded from disk, parsed and with items resolved
/// against the TypeEnv. The `module_name` is the original
/// import name (`from python import foo` → `module_name = "foo"`). The
/// `alias` is the local binding (`from python import foo as bar` →
/// `module_name = "foo"`, `alias = Some("bar")`).
#[derive(Debug, Clone)]
pub struct LoadedStub {
    /// Name of the Python module (the `foo` of `from python import foo`).
    /// Matches the stem of the `.pyi` file.
    pub module_name: String,
    /// Local alias if the import uses `as` (`from python import foo as bar`
    /// → `Some("bar")`). No alias → `None`.
    pub alias: Option<String>,
    /// Resolved items from the stub, with Fitz types already materialized.
    pub items: Vec<ResolvedStubItem>,
    /// Absolute path to the loaded `.pyi` file. Useful for diagnostics
    /// and future go-to-definition.
    pub stub_path: PathBuf,
}

impl LoadedStub {
    /// Returns the name with which the binding appears in the checker's
    /// local scope (alias if present, otherwise module_name).
    pub fn binding_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.module_name)
    }
}

/// Walks `program` looking for `Stmt::FromImport { path: ["python"],
/// names }`, tries to load `<base_dir>/<name>.pyi` for each name,
/// parses, registers the items in `env`, and returns the list of loaded
/// stubs. Non-critical errors (file does not exist, parse error)
/// degrade silently — the binding stays as `Type::PyAny`.
///
/// `base_dir` is the directory where the root `.fitz` that
/// starts `fitz run`/`fitz build`/`fitz check` lives. For `fitz check`/`run`
/// with manifest mode, it is still the dir of the resolved `.fitz` entry.
///
/// `env` is mutated on each call (each stub nominal is registered
/// as a side-effect). Passing it as `&mut` simplifies ownership;
/// returning the `LoadedStub` listing allows the checker to bind the
/// aliases correctly.
pub fn load_stubs(program: &Program, base_dir: &Path, env: &mut TypeEnv) -> Vec<LoadedStub> {
    // 8-pyi.B: pre-scan the program to identify types that the
    // .fitz already declares. We skip stub classes with that name to
    // avoid redundant `declare_nominal` in `resolve_program` later
    // ("the .fitz wins over the .pyi" policy). Built-ins from the HTTP
    // runtime (`Request`, `Response`, `File`) also go to the skip set:
    // resolve_program registers them unconditionally in round 0 and a
    // `class Request: ...` stub would panic.
    let mut skip_class_names = pre_scan_program_type_names(program);
    skip_class_names.insert("Request".to_string());
    skip_class_names.insert("Response".to_string());
    skip_class_names.insert("File".to_string());

    let mut loaded = Vec::new();
    for stmt in program {
        if let Stmt::FromImport { path, names, .. } = stmt {
            // We only care about `from python import X[, Y as z]`.
            if path.first().map(String::as_str) != Some("python") {
                continue;
            }
            for (name, alias) in names {
                if let Some(stub) =
                    load_one_stub(name, alias.as_deref(), base_dir, env, &skip_class_names)
                {
                    loaded.push(stub);
                }
            }
        }
    }
    loaded
}

/// Returns the set of type names declared by the program
/// (top-level `type X { ... }`). Used by `load_stubs` to skip
/// stub classes with the same name.
fn pre_scan_program_type_names(program: &Program) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in program {
        if let Stmt::TypeDef { name, .. } = stmt {
            names.insert(name.clone());
        }
    }
    names
}

/// Tries to load `<base_dir>/<name>.pyi`. Returns `Some(LoadedStub)`
/// if found and parsed OK; `None` if the file does not exist or the parse
/// fails. On a failed parse it emits a warning to stderr citing
/// the file and the message, but does not abort — silent fallback policy.
///
/// `skip_class_names`: stub classes whose name is in this set are
/// excluded from TypeEnv registration (avoids clashing with the
/// program's `type X` that `resolve_program` declares later).
fn load_one_stub(
    name: &str,
    alias: Option<&str>,
    base_dir: &Path,
    env: &mut TypeEnv,
    skip_class_names: &HashSet<String>,
) -> Option<LoadedStub> {
    let stub_path = base_dir.join(format!("{}.pyi", name));
    if !stub_path.is_file() {
        return None;
    }
    let raw = match std::fs::read_to_string(&stub_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "⚠ pyi-stubs: no se pudo leer `{}`: {} (fallback a PyAny opaco)",
                stub_path.display(),
                e
            );
            return None;
        }
    };
    let items = match pyi_stub::parse_stub(&raw) {
        Ok(items) => items,
        Err(e) => {
            eprintln!(
                "⚠ pyi-stubs: parse error en `{}`: {} (fallback a PyAny opaco)",
                stub_path.display(),
                e
            );
            return None;
        }
    };
    // 8-pyi.B: we only materialize `class` declarations (register
    // the nominals in the TypeEnv). Top-level fns and vars are ignored
    // in this sub-step — their real use is for typed field access
    // (`api.fetch_user(...)`), which arrives in 8-pyi.C. Processing them here
    // has an undesired side-effect: `stub_type_to_fitz_type` over
    // the ret/params of a fn that references a skipped class (declared by the
    // .fitz) would auto-register that nominal in the env, and
    // later `resolve_program` would panic with "type declared more
    // than once".
    let filtered: Vec<StubItem> = items
        .into_iter()
        .filter(|it| match it {
            StubItem::Class(c) => !skip_class_names.contains(&c.name),
            // Skip top-level fns and vars entirely in B; they come in C.
            _ => false,
        })
        .collect();
    let resolved = pyi_stub::register_stub_items_into_env(&filtered, env);
    Some(LoadedStub {
        module_name: name.to_string(),
        alias: alias.map(str::to_string),
        items: resolved,
        stub_path,
    })
}

/// 8-pyi.C: loader pass 2. Processes top-level fns and vars of each
/// stub loaded in pass 1 (`load_stubs`) and registers a synthetic
/// nominal per module with one field per callable or variable.
/// Called by the caller (typically `main.rs`) AFTER
/// `resolve_program` so that the nominals declared in the .fitz
/// (which the stub fns may mention in their ret type) are already
/// available.
///
/// The synthetic nominal is named `__pyi_module_<binding_name>`
/// (`binding_name` = alias if the import has it, else module_name).
/// The mapping is saved in `env.set_pyi_module(binding, id)` so
/// the checker can look it up from `Stmt::FromImport`.
///
/// **Ret type convention for fns**: the stub fns are
/// materialized with their `ret` wrapped in `Result<ret, Str>` to
/// reflect the 8.3 runtime model (Python calls are wrapped
/// automatically in `Result` by the evaluator). This lets
/// the `foo.fn(x)` call site type directly as `Result<T>` without extra
/// refinement.
///
/// **Silent fallback**: if re-reading the stub fails (rare race: the
/// file was deleted between pass 1 and pass 2), we simply skip that
/// module. The binding stays as opaque `Type::PyAny`.
pub fn load_callables(stubs: &[LoadedStub], env: &mut TypeEnv) {
    for stub in stubs {
        load_callables_for_one(stub, env);
    }
}

fn load_callables_for_one(stub: &LoadedStub, env: &mut TypeEnv) {
    let raw = match std::fs::read_to_string(&stub.stub_path) {
        Ok(r) => r,
        Err(_) => return,
    };
    let items = match pyi_stub::parse_stub(&raw) {
        Ok(i) => i,
        Err(_) => return,
    };

    let mut fields: Vec<ResolvedField> = Vec::new();
    for item in &items {
        match item {
            StubItem::Fn(f) => {
                let params: Vec<Type> = f
                    .params
                    .iter()
                    .map(|p| pyi_stub::stub_type_to_fitz_type(&p.ty, env))
                    .collect();
                let ret = pyi_stub::stub_type_to_fitz_type(&f.ret, env);
                // Auto-wrap in Result<ret, Str> — parallel to the 8.3
                // runtime model where EVERY Python call is wrapped in
                // Result. This way the `foo.fn(x)` call site types directly as
                // `Result<T>` and the checker requires exhaustive match/`?`.
                let ret_wrapped = Type::Result {
                    ok: Box::new(ret),
                    err: Box::new(Type::Str),
                };
                fields.push(ResolvedField {
                    name: f.name.clone(),
                    type_: Type::Function {
                        params,
                        ret: Box::new(ret_wrapped),
                    },
                });
            }
            StubItem::Var(v) => {
                let ty = pyi_stub::stub_type_to_fitz_type(&v.ty, env);
                fields.push(ResolvedField {
                    name: v.name.clone(),
                    type_: ty,
                });
            }
            StubItem::Class(_) => continue, // already processed in pass 1
        }
    }

    // Synthetic name of the nominal: `__pyi_module_` prefix to not
    // clash with program types.
    let binding = stub.binding_name();
    let synth_name = format!("__pyi_module_{}", binding);
    // If for some reason the synthetic nominal already exists (re-run
    // of the same loader on the same env, rare but possible in
    // tests), we reuse the existing id and overwrite the fields.
    let id = match env.lookup(&synth_name) {
        Some(existing) => existing,
        None => match env.declare_nominal(synth_name) {
            Ok(id) => id,
            Err(_) => return,
        },
    };
    env.set_fields(id, fields);
    env.set_pyi_module(binding.to_string(), id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::types::Type;
    use std::fs;

    /// Builder helper for tests: parses Fitz source to Program.
    fn parse_program(src: &str) -> Program {
        let tokens = tokenize(src).expect("lex OK");
        parse(tokens).expect("parse OK")
    }

    /// Creates a temp dir with an adjacent `.pyi`. Returns the dir's path
    /// (to use as base_dir); it auto-deletes at the end.
    fn temp_dir_with_stub(stub_name: &str, content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir OK");
        let stub_path = dir.path().join(format!("{}.pyi", stub_name));
        fs::write(&stub_path, content).expect("write OK");
        dir
    }

    #[test]
    fn loader_no_python_imports_returns_empty() {
        let program = parse_program("let x = 42");
        let dir = tempfile::tempdir().unwrap();
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        assert!(stubs.is_empty());
    }

    #[test]
    fn loader_python_import_without_adjacent_pyi_returns_empty() {
        let program = parse_program("from python import math");
        let dir = tempfile::tempdir().unwrap();
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        // No adjacent math.pyi → silent fallback.
        assert!(stubs.is_empty());
    }

    #[test]
    fn loader_python_import_with_pyi_loads_nominals() {
        let stub_src = "\
class User:
    id: int
    name: str
class Order:
    user_id: int
";
        let dir = temp_dir_with_stub("api", stub_src);
        let program = parse_program("from python import api");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        assert_eq!(stubs.len(), 1);
        assert_eq!(stubs[0].module_name, "api");
        assert_eq!(stubs[0].alias, None);
        assert_eq!(stubs[0].binding_name(), "api");
        // `User` and `Order` were registered as nominals with fields.
        let user_id = env.lookup("User").expect("User registrado");
        let user_fields = env
            .info(user_id)
            .fields
            .as_ref()
            .expect("User tiene fields");
        assert_eq!(user_fields.len(), 2);
        assert_eq!(user_fields[0].name, "id");
        assert_eq!(user_fields[0].type_, Type::Int);
        assert_eq!(user_fields[1].name, "name");
        assert_eq!(user_fields[1].type_, Type::Str);
        let order_id = env.lookup("Order").expect("Order registrado");
        let order_fields = env
            .info(order_id)
            .fields
            .as_ref()
            .expect("Order tiene fields");
        assert_eq!(order_fields.len(), 1);
        assert_eq!(order_fields[0].name, "user_id");
        assert_eq!(order_fields[0].type_, Type::Int);
    }

    #[test]
    fn loader_alias_preserves_module_name_and_alias() {
        let stub_src = "class Foo: ...\n";
        let dir = temp_dir_with_stub("mylib", stub_src);
        let program = parse_program("from python import mylib as ml");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        assert_eq!(stubs.len(), 1);
        assert_eq!(stubs[0].module_name, "mylib");
        assert_eq!(stubs[0].alias.as_deref(), Some("ml"));
        assert_eq!(stubs[0].binding_name(), "ml");
    }

    #[test]
    fn loader_pyi_malformado_silent_fallback() {
        // Completely broken stub (invalid Python syntax that the
        // pyi_stub parser should reject).
        let stub_src = "def fn( -> oops\n";
        let dir = temp_dir_with_stub("broken", stub_src);
        let program = parse_program("from python import broken");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        // Silent fallback: does not crash, returns empty.
        assert!(stubs.is_empty());
        // And `broken` did not stay as a nominal contaminating the env.
        assert!(env.lookup("broken").is_none());
    }

    #[test]
    fn loader_pyi_only_fns_skips_fns_in_b_because_they_are_for_c() {
        // 8-pyi.B policy: we only materialize `class`. Stub fns/vars
        // are skipped until 8-pyi.C (typed field access).
        // We verify that `LoadedStub.items` stays empty for a stub
        // that only has fns, and that the fn does NOT contaminate the TypeEnv
        // (auto-registration of ret type nominals, etc.).
        let stub_src = "def add(a: int, b: int) -> int: ...\n";
        let dir = temp_dir_with_stub("calc", stub_src);
        let program = parse_program("from python import calc");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        assert_eq!(stubs.len(), 1);
        assert!(
            stubs[0].items.is_empty(),
            "8-pyi.B skipea fns/vars; LoadedStub.items debería estar vacío"
        );
    }

    // 8-pyi.C: pass-2 tests (load_callables).

    #[test]
    fn callables_registers_synthetic_nominal_with_field_per_fn() {
        let stub_src = "\
def add(a: int, b: int) -> int: ...
def greet(name: str) -> str: ...
";
        let dir = temp_dir_with_stub("calc", stub_src);
        let program = parse_program("from python import calc");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        load_callables(&stubs, &mut env);
        let mod_id = env.pyi_module("calc").expect("pyi_module registrado");
        let fields = env.info(mod_id).fields.as_ref().expect("tiene fields");
        assert_eq!(fields.len(), 2);
        // `add`: Function { params: [Int, Int], ret: Result<Int, Str> }
        let add = fields.iter().find(|f| f.name == "add").expect("add");
        match &add.type_ {
            Type::Function { params, ret } => {
                assert_eq!(params, &[Type::Int, Type::Int]);
                match &**ret {
                    Type::Result { ok, err } => {
                        assert_eq!(**ok, Type::Int);
                        assert_eq!(**err, Type::Str);
                    }
                    other => panic!("ret no es Result: {:?}", other),
                }
            }
            other => panic!("add no es Function: {:?}", other),
        }
    }

    #[test]
    fn callables_top_level_var_registers_direct_field_without_wrap() {
        let stub_src = "VERSION: str\nMAX_SIZE: int\n";
        let dir = temp_dir_with_stub("config", stub_src);
        let program = parse_program("from python import config");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        load_callables(&stubs, &mut env);
        let mod_id = env.pyi_module("config").unwrap();
        let fields = env.info(mod_id).fields.as_ref().unwrap();
        let version = fields.iter().find(|f| f.name == "VERSION").unwrap();
        assert_eq!(version.type_, Type::Str);
        let max_size = fields.iter().find(|f| f.name == "MAX_SIZE").unwrap();
        assert_eq!(max_size.type_, Type::Int);
    }

    #[test]
    fn callables_alias_uses_binding_name_in_pyi_modules() {
        let stub_src = "def hi() -> str: ...\n";
        let dir = temp_dir_with_stub("greetings", stub_src);
        let program = parse_program("from python import greetings as g");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        load_callables(&stubs, &mut env);
        // The checker's binding is `g`, not `greetings`.
        assert!(env.pyi_module("g").is_some());
        assert!(env.pyi_module("greetings").is_none());
    }

    #[test]
    fn callables_fn_returning_class_from_stub_resolves_nominal() {
        let stub_src = "\
class User:
    id: int
def fetch_user(uid: int) -> User: ...
";
        let dir = temp_dir_with_stub("api", stub_src);
        let program = parse_program("from python import api");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        load_callables(&stubs, &mut env);
        let mod_id = env.pyi_module("api").unwrap();
        let fields = env.info(mod_id).fields.as_ref().unwrap();
        let fetch = fields.iter().find(|f| f.name == "fetch_user").unwrap();
        match &fetch.type_ {
            Type::Function { ret, .. } => match &**ret {
                Type::Result { ok, .. } => {
                    // ok side should be Nominal(User)
                    let user_id = env.lookup("User").expect("User registrado");
                    assert_eq!(**ok, Type::Nominal(user_id));
                }
                other => panic!("ret no es Result: {:?}", other),
            },
            other => panic!("fetch_user no es Function: {:?}", other),
        }
    }

    #[test]
    fn loader_multiple_imports_loads_each_pyi() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.pyi"), "class A:\n    x: int\n").unwrap();
        fs::write(dir.path().join("b.pyi"), "class B:\n    y: str\n").unwrap();
        let program = parse_program("from python import a, b");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        assert_eq!(stubs.len(), 2);
        assert!(env.lookup("A").is_some());
        assert!(env.lookup("B").is_some());
    }

    #[test]
    fn loader_fn_referencing_skipped_class_does_not_contaminate_env() {
        // 8-pyi.B regression bug: the stub had `def fn(...) -> User`
        // where User was already in skip_set (the program declares it
        // as `type`). Processing the fn called
        // `stub_type_to_fitz_type(User)` which auto-registered the
        // nominal before resolve_program declared it,
        // producing "type declared more than once". Fix: skip
        // fns/vars in B (do not process them at all).
        let stub_src = "\
class User:
    id: int
def fetch_user(id: int) -> User: ...
def list_users() -> list[User]: ...
";
        let dir = temp_dir_with_stub("api", stub_src);
        let program = parse_program("type User { id: Int, name: Str }\nfrom python import api\n");
        let mut env = TypeEnv::new();
        let _stubs = load_stubs(&program, dir.path(), &mut env);
        // The loader must NOT register User (the .fitz declares it).
        // It must also not have been auto-registered by the fn that
        // references it.
        assert!(
            env.lookup("User").is_none(),
            "User no debe existir en el env hasta que resolve_program lo declare"
        );
    }

    #[test]
    fn loader_no_toca_imports_fitz_normales() {
        // `from utils import X` (without `python` prefix) must be ignored.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("utils.pyi"), "class Foo:\n    z: int\n").unwrap();
        let program = parse_program("from utils import X");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        // It is not from python, so utils.pyi is NOT looked at.
        assert!(stubs.is_empty());
        assert!(env.lookup("Foo").is_none());
    }

    #[test]
    fn loader_fitz_type_wins_over_pyi() {
        // If the program already has `type User { ... }` and the .pyi
        // also declares `class User: ...`, the .fitz fields
        // win (they do not get overwritten).
        let stub_src = "class User:\n    id_from_pyi: int\n";
        let dir = temp_dir_with_stub("api", stub_src);
        let program = parse_program("type User { id: Int, name: Str }\nfrom python import api\n");
        // First we register the Fitz nominal with the program's fields
        // (simulating what `resolve_program` does before the loader).
        let user_id = env_new_with_user_fields(&mut TypeEnv::new());
        let mut env = TypeEnv::new();
        let id = env
            .declare_nominal("User".to_string())
            .expect("declare User");
        env.set_fields(
            id,
            vec![
                ResolvedField {
                    name: "id".into(),
                    type_: Type::Int,
                },
                ResolvedField {
                    name: "name".into(),
                    type_: Type::Str,
                },
            ],
        );
        let _ = (user_id, program); // dummies from the helper scaffold
        let _stubs = load_stubs(
            &parse_program("from python import api\n"),
            dir.path(),
            &mut env,
        );
        // The original .fitz fields remain intact.
        let info = env.info(id);
        let fields = info.fields.as_ref().expect("User tiene fields");
        assert_eq!(fields[0].name, "id");
        assert_eq!(fields[1].name, "name");
        assert_eq!(fields.len(), 2);
    }

    /// Local helper for the previous test: does nothing real, just ensures
    /// that TypeEnv::new() compiles without imports.
    fn env_new_with_user_fields(_env: &mut TypeEnv) -> Option<crate::types::TypeId> {
        None
    }

    use crate::types::ResolvedField;
}
