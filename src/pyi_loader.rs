// pyi_loader.rs — Auto-pickup de stubs `.pyi` adyacentes al .fitz raíz.
//
// Fase 8-pyi.B (v0.9.57): cuando el programa contiene `from python
// import foo` (o `from python import foo as bar`), buscamos
// `<base_dir>/foo.pyi` adyacente al archivo `.fitz` que arranca la
// ejecución. Si existe, parseamos con `pyi_stub::parse_stub` y
// registramos los nominales declarados al `TypeEnv` ANTES de invocar
// al checker. Resultado: el user puede escribir `let u: User =
// requests.fetch(...)?` con `User` declarado en `requests.pyi`, y el
// checker resuelve el nominal en lugar de fallar con "tipo
// desconocido".
//
// **Política de errores** (silent fallback):
// - `foo.pyi` no existe → no hay stub, binding sigue como
//   `Type::PyAny` opaco (comportamiento actual).
// - `foo.pyi` existe pero falla al parsear → warning a stderr,
//   binding sigue como `Type::PyAny` (no rompe el build).
// - `foo.pyi` parsea OK pero un item tiene tipo no resoluble →
//   `stub_type_to_fitz_type` ya devuelve `Type::Any` como fallback
//   gradual (cero overhead extra).
//
// **Política de búsqueda** (decisión 8-pyi.B): solo adyacente al
// `.fitz` raíz. NO recorre `PYTHONPATH` ni `site-packages`. La idea
// es que el user copie/genere el `.pyi` proyecto-local (vía
// `fitz py-stubs <archivo.py>`) y lo commitee — paridad
// reproducible entre máquinas, cero magia ambiente.
//
// **Cobertura del MVP**:
// - Nominales (`class Foo:`): registrados en `TypeEnv` con fields. ✓
// - Fns (`def name(args) -> ret`): resueltos para uso futuro (8-pyi.C
//   field access tipado), no bindeados todavía como Function al scope
//   del checker en este sub-paso.
// - Vars top-level (`name: type`): idem fns — resueltos pero no
//   bindeados a scope en 8-pyi.B.
//
// El binding `foo` del `from python import foo` sigue tipando como
// `Type::PyAny` en 8-pyi.B; field access tipado (`foo.bar`) llega
// en 8-pyi.C.

use crate::ast::{Program, Stmt};
use crate::pyi_stub::{self, ResolvedStubItem, StubItem};
use crate::types::{ResolvedField, Type, TypeEnv};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Stub cargado desde disco, parseado y con sus items resueltos
/// contra el TypeEnv. El `module_name` es el nombre original del
/// import (`from python import foo` → `module_name = "foo"`). El
/// `alias` es el binding local (`from python import foo as bar` →
/// `module_name = "foo"`, `alias = Some("bar")`).
#[derive(Debug, Clone)]
pub struct LoadedStub {
    /// Nombre del módulo Python (el `foo` de `from python import foo`).
    /// Coincide con el stem del archivo `.pyi`.
    pub module_name: String,
    /// Alias local si el import usa `as` (`from python import foo as bar`
    /// → `Some("bar")`). Sin alias → `None`.
    pub alias: Option<String>,
    /// Items resueltos del stub, con tipos Fitz ya materializados.
    pub items: Vec<ResolvedStubItem>,
    /// Path absoluto al archivo `.pyi` cargado. Útil para diagnósticos
    /// y go-to-definition futuro.
    pub stub_path: PathBuf,
}

impl LoadedStub {
    /// Devuelve el nombre con el que el binding aparece en el scope
    /// local del checker (alias si está, sino module_name).
    pub fn binding_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.module_name)
    }
}

/// Walkea el `program` buscando `Stmt::FromImport { path: ["python"],
/// names }`, intenta cargar `<base_dir>/<name>.pyi` por cada nombre,
/// parsea, registra los items en `env`, y devuelve la lista de stubs
/// cargados. Errores no críticos (archivo no existe, parse error)
/// degradan silentemente — el binding sigue como `Type::PyAny`.
///
/// `base_dir` es el directorio donde vive el archivo `.fitz` raíz que
/// arranca `fitz run`/`fitz build`/`fitz check`. Para `fitz check`/`run`
/// con manifest mode, sigue siendo el dir del entry `.fitz` resuelto.
///
/// El `env` se muta en cada call (cada nominal del stub se registra
/// como side-effect). Pasarlo como `&mut` simplifica el ownership;
/// devolver el listado `LoadedStub` permite que el checker bindee los
/// alias correctamente.
pub fn load_stubs(program: &Program, base_dir: &Path, env: &mut TypeEnv) -> Vec<LoadedStub> {
    // 8-pyi.B: pre-scan del programa para identificar tipos que el
    // .fitz ya declara. Skipeamos classes del stub con ese nombre para
    // evitar `declare_nominal` redundante en `resolve_program` después
    // (política "el .fitz gana sobre el .pyi"). Built-ins del runtime
    // HTTP (`Request`, `Response`, `File`) también van al skip set:
    // resolve_program los registra incondicionalmente en vuelta 0 y un
    // stub `class Request: ...` haría panic.
    let mut skip_class_names = pre_scan_program_type_names(program);
    skip_class_names.insert("Request".to_string());
    skip_class_names.insert("Response".to_string());
    skip_class_names.insert("File".to_string());

    let mut loaded = Vec::new();
    for stmt in program {
        if let Stmt::FromImport { path, names, .. } = stmt {
            // Solo nos interesa `from python import X[, Y as z]`.
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

/// Devuelve el set de nombres de tipos declarados por el programa
/// (`type X { ... }` top-level). Usado por `load_stubs` para skipear
/// classes del stub con el mismo nombre.
fn pre_scan_program_type_names(program: &Program) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in program {
        if let Stmt::TypeDef { name, .. } = stmt {
            names.insert(name.clone());
        }
    }
    names
}

/// Intenta cargar `<base_dir>/<name>.pyi`. Devuelve `Some(LoadedStub)`
/// si encuentra y parsea OK; `None` si el archivo no existe o el parse
/// falla. En caso de parse fallido emite un warning a stderr citando
/// el archivo y el mensaje, pero no aborta — política silent fallback.
///
/// `skip_class_names`: classes del stub con nombre en este set se
/// excluyen del registro al TypeEnv (evita choque con `type X` del
/// programa que `resolve_program` declara más adelante).
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
    // 8-pyi.B: solo materializamos `class` declarations (registramos
    // los nominales en el TypeEnv). Fns y vars top-level se ignoran
    // en este sub-paso — su uso real es para field access tipado
    // (`api.fetch_user(...)`), que llega en 8-pyi.C. Procesarlas acá
    // tiene un side-effect indeseado: `stub_type_to_fitz_type` sobre
    // el ret/params de una fn que referencia una class skipeada (la
    // declara el .fitz) auto-registraría ese nominal en el env, y
    // después `resolve_program` haría panic con "tipo declarado más
    // de una vez".
    let filtered: Vec<StubItem> = items
        .into_iter()
        .filter(|it| match it {
            StubItem::Class(c) => !skip_class_names.contains(&c.name),
            // Skip fns y vars top-level por completo en B; llegan en C.
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

/// 8-pyi.C: pase 2 del loader. Procesa fns y vars top-level de cada
/// stub cargado en pase 1 (`load_stubs`) y registra un nominal
/// sintético por módulo con un field por cada callable o variable.
/// Llamado por el caller (típicamente `main.rs`) DESPUÉS de
/// `resolve_program` para que los nominales declarados en el .fitz
/// (que las fns del stub pueden mencionar en su ret type) ya estén
/// disponibles.
///
/// El nominal sintético se llama `__pyi_module_<binding_name>`
/// (`binding_name` = alias si el import lo tiene, sino module_name).
/// El mapeo se guarda en `env.set_pyi_module(binding, id)` para que
/// el checker pueda consultarlo desde `Stmt::FromImport`.
///
/// **Convención de ret type para fns**: las fns del stub se
/// materializan con su `ret` envuelto en `Result<ret, Str>` para
/// reflejar el modelo runtime de 8.3 (calls Python se wrapean
/// automáticamente en `Result` por el evaluator). Esto permite que
/// el call site `foo.fn(x)` tipe directo a `Result<T>` sin extra
/// refinamiento.
///
/// **Silent fallback**: si re-leer el stub falla (race rara: el
/// archivo se borró entre pase 1 y pase 2), simplemente skipea ese
/// módulo. El binding queda como `Type::PyAny` opaco.
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
                // Auto-wrap en Result<ret, Str> — paralelo al modelo
                // runtime de 8.3 donde TODA call Python se envuelve en
                // Result. Así el call site `foo.fn(x)` tipa directo a
                // `Result<T>` y el checker exige match/`?` exhaustivo.
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
            StubItem::Class(_) => continue, // ya procesado en pase 1
        }
    }

    // Nombre sintético del nominal: prefijo `__pyi_module_` para no
    // chocar con types del programa.
    let binding = stub.binding_name();
    let synth_name = format!("__pyi_module_{}", binding);
    // Si por alguna razón el nominal sintético ya existe (re-corrida
    // del mismo loader sobre el mismo env, raro pero posible en
    // tests), reusamos el id existente y reescribimos los fields.
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

    /// Builder helper para los tests: parsea source Fitz a Program.
    fn parse_program(src: &str) -> Program {
        let tokens = tokenize(src).expect("lex OK");
        parse(tokens).expect("parse OK")
    }

    /// Crea un dir temporal con un `.pyi` adyacente. Devuelve el path
    /// del dir (para usarlo como base_dir) y se autoborra al final.
    fn temp_dir_with_stub(stub_name: &str, content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir OK");
        let stub_path = dir.path().join(format!("{}.pyi", stub_name));
        fs::write(&stub_path, content).expect("write OK");
        dir
    }

    #[test]
    fn loader_no_python_imports_devuelve_vacio() {
        let program = parse_program("let x = 42");
        let dir = tempfile::tempdir().unwrap();
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        assert!(stubs.is_empty());
    }

    #[test]
    fn loader_python_import_sin_pyi_adyacente_devuelve_vacio() {
        let program = parse_program("from python import math");
        let dir = tempfile::tempdir().unwrap();
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        // No hay math.pyi adyacente → silent fallback.
        assert!(stubs.is_empty());
    }

    #[test]
    fn loader_python_import_con_pyi_carga_nominales() {
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
        // `User` y `Order` quedaron registrados como nominales con fields.
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
    fn loader_alias_preserva_module_name_y_alias() {
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
        // Stub completamente roto (sintaxis Python inválida que el
        // parser de pyi_stub debería rechazar).
        let stub_src = "def fn( -> oops\n";
        let dir = temp_dir_with_stub("broken", stub_src);
        let program = parse_program("from python import broken");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        // Silent fallback: no crashea, devuelve vacío.
        assert!(stubs.is_empty());
        // Y `broken` no quedó como nominal contaminando el env.
        assert!(env.lookup("broken").is_none());
    }

    #[test]
    fn loader_pyi_solo_fns_skipea_fns_en_b_porque_son_para_c() {
        // 8-pyi.B política: solo materializamos `class`. Fns/vars del
        // stub se skipean hasta 8-pyi.C (field access tipado).
        // Verificamos que `LoadedStub.items` queda vacío para un stub
        // que solo tiene fns, y que la fn NO contamina el TypeEnv
        // (auto-registro de nominales del ret type, etc.).
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

    // 8-pyi.C: tests del pase 2 (load_callables).

    #[test]
    fn callables_registra_nominal_sintetico_con_field_por_fn() {
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
    fn callables_var_top_level_registra_field_directo_sin_wrap() {
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
    fn callables_alias_usa_binding_name_en_pyi_modules() {
        let stub_src = "def hi() -> str: ...\n";
        let dir = temp_dir_with_stub("greetings", stub_src);
        let program = parse_program("from python import greetings as g");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        load_callables(&stubs, &mut env);
        // El binding del checker es `g`, no `greetings`.
        assert!(env.pyi_module("g").is_some());
        assert!(env.pyi_module("greetings").is_none());
    }

    #[test]
    fn callables_fn_que_retorna_class_del_stub_resuelve_nominal() {
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
                    // ok side debería ser Nominal(User)
                    let user_id = env.lookup("User").expect("User registrado");
                    assert_eq!(**ok, Type::Nominal(user_id));
                }
                other => panic!("ret no es Result: {:?}", other),
            },
            other => panic!("fetch_user no es Function: {:?}", other),
        }
    }

    #[test]
    fn loader_multiple_imports_carga_cada_pyi() {
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
    fn loader_fn_que_referencia_class_skipeada_no_contamina_env() {
        // Regresión bug 8-pyi.B: el stub tenía `def fn(...) -> User`
        // donde User ya estaba en skip_set (el programa lo declara
        // como `type`). Procesar la fn hacía
        // `stub_type_to_fitz_type(User)` que auto-registraba el
        // nominal antes de que resolve_program lo declarara,
        // generando "tipo declarado más de una vez". Fix: skipear
        // fns/vars en B (no procesarlas para nada).
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
        // El loader NO debe registrar User (el .fitz lo declara).
        // Tampoco debe haber sido auto-registrado por la fn que lo
        // referencia.
        assert!(
            env.lookup("User").is_none(),
            "User no debe existir en el env hasta que resolve_program lo declare"
        );
    }

    #[test]
    fn loader_no_toca_imports_fitz_normales() {
        // `from utils import X` (sin `python` prefix) debe ignorarse.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("utils.pyi"), "class Foo:\n    z: int\n").unwrap();
        let program = parse_program("from utils import X");
        let mut env = TypeEnv::new();
        let stubs = load_stubs(&program, dir.path(), &mut env);
        // No es from python, así que utils.pyi NO se mira.
        assert!(stubs.is_empty());
        assert!(env.lookup("Foo").is_none());
    }

    #[test]
    fn loader_fitz_type_gana_sobre_pyi() {
        // Si el programa ya tiene `type User { ... }` y el .pyi
        // también declara `class User: ...`, los fields del .fitz
        // ganan (no se sobreescriben).
        let stub_src = "class User:\n    id_from_pyi: int\n";
        let dir = temp_dir_with_stub("api", stub_src);
        let program = parse_program("type User { id: Int, name: Str }\nfrom python import api\n");
        // Primero registramos el nominal Fitz con fields del programa
        // (simulando lo que hace `resolve_program` antes del loader).
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
        let _ = (user_id, program); // dummies del scaffold del helper
        let _stubs = load_stubs(
            &parse_program("from python import api\n"),
            dir.path(),
            &mut env,
        );
        // Los fields originales del .fitz siguen intactos.
        let info = env.info(id);
        let fields = info.fields.as_ref().expect("User tiene fields");
        assert_eq!(fields[0].name, "id");
        assert_eq!(fields[1].name, "name");
        assert_eq!(fields.len(), 2);
    }

    /// Helper local del test anterior: no hace nada real, solo asegura
    /// que TypeEnv::new() compila sin imports.
    fn env_new_with_user_fields(_env: &mut TypeEnv) -> Option<crate::types::TypeId> {
        None
    }

    use crate::types::ResolvedField;
}
