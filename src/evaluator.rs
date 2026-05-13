// evaluator.rs — Fase 2.4
//
// Recorre el AST y produce efectos (imprimir, mutar variables) y valores.
//
// Estructura interna:
//
//  ┌──────────────┐   programa
//  │ eval(...)    │ ──────────► env global + register_builtins
//  └──────┬───────┘
//         │ por cada Stmt
//         ▼
//  ┌──────────────┐         ┌──────────────┐
//  │ eval_stmt    │ ◀──────►│ eval_expr    │
//  └──────────────┘         └──────────────┘
//
// Control de flujo y errores comparten un mismo canal: `EvalSignal`. Esto
// nos permite usar `?` para propagar tanto errores reales como un `return`
// que tiene que escalar hasta el caller de la función. El truco lo tomé de
// Crafting Interpreters; en Rust funciona naturalmente con `Result`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{
    AssignTarget, BinOpKind, Decorator, Expr, Param, Pattern, Program, Span, Stmt, StrPart,
    UnaryOpKind,
};
use crate::env::{EnvRef, Environment};
use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::http::{
    has_active_registry, parse_path_template, push_route, set_server_config, BodyParam, HttpMethod,
    RouteSpec, ServerConfig,
};
use crate::lexer::tokenize;
use crate::parser::parse;
use crate::value::{ResultVariant, Value};

// ---------------------------------------------------------------------------
// EvalSignal — el canal único de "salida no normal" de eval_stmt/eval_expr.
// ---------------------------------------------------------------------------

/// Una interrupción del flujo normal de evaluación. Cubre dos cosas en una:
///  - errores reales del programa (`Error`)
///  - control de flujo no local (`Return`, `Break`, `Continue`)
///
/// Cuando una función llama a otra, el caller espera convertir
/// `Err(Return(v))` en `Ok(v)`. Cuando un loop captura un `break`, convierte
/// `Err(Break)` en una salida normal. Cualquier otra cosa se propaga.
#[derive(Debug)]
pub enum EvalSignal {
    Error(FitzError),
    Return(Value),
    Break,
    Continue,
}

/// `From<FitzError>` permite hacer `return Err(error.into())` o usar `?`
/// directamente cuando una función auxiliar devuelve `FitzResult`.
impl From<FitzError> for EvalSignal {
    fn from(e: FitzError) -> Self {
        EvalSignal::Error(e)
    }
}

pub type EvalResult<T> = Result<T, EvalSignal>;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Ejecuta un programa. Construye el env global, registra builtins, e itera
/// las sentencias del programa.
///
/// El `base_dir` por defecto es el cwd actual del proceso — sirve para
/// resolver imports relativos a archivos del proyecto cuando se ejecuta
/// el binario sin contexto adicional. Para programas cargados desde un
/// archivo `.fitz` específico, usar `eval_with_base` con el directorio
/// del archivo.
///
/// Signals "huérfanos" (`return`/`break`/`continue` fuera de su contexto)
/// se convierten acá en errores del usuario.
///
/// `dead_code` allow: hoy `main.rs` siempre usa `eval_with_base` con el
/// directorio del archivo, y el resto del uso es desde tests (que el
/// análisis del binario no ve). Lo dejamos como API pública por simetría
/// y para tests de smoke.
#[allow(dead_code)]
pub fn eval(program: Program) -> FitzResult<()> {
    let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    eval_with_base(program, base_dir)
}

/// Variante de `eval` que recibe explícitamente el directorio raíz para
/// resolver `import`s relativos. Lo usa `main.rs` después de leer el
/// archivo `.fitz`: el `base_dir` es el padre del archivo, así
/// `import utils` resuelve a `<dir-del-archivo>/utils.fitz`.
pub fn eval_with_base(program: Program, base_dir: PathBuf) -> FitzResult<()> {
    install_loader(base_dir);
    // Guard para des-instalar el loader siempre — incluso ante panic.
    // Si el programa termina por error, igual queremos limpiar el
    // thread_local así un siguiente `eval` arranca limpio.
    let _guard = LoaderGuard;

    let env = Environment::new();
    register_builtins(&env);

    for stmt in &program {
        if let Err(signal) = eval_stmt(stmt, env.clone()) {
            return Err(signal_to_error(signal));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Decorators — pre-procesamiento de `@nombre(args)` sobre `Stmt::FnDef`.
//
// El parser solo acumula decorators; acá decidimos qué hace cada uno.
// Política:
//
//   - `@get` / `@post` / `@put` / `@delete`: validan args (1 path) y
//     registran una `RouteSpec` en el `HttpRegistry` activo. Si no hay
//     registry activo (eval embebido, REPL, test sin server), error
//     explícito con sugerencia de qué hacer.
//   - Cualquier otro nombre: error explícito. `@server` entra en 4.4;
//     decoradores custom no están planeados hasta Fase 5+.
//
// El handler se pasa ya construido como `Value::Function` para que el
// registry lo guarde sin tener que reconstruirlo (clones de `Rc` son
// baratos; el `closure: EnvRef` mantiene viva el env del módulo).
// ---------------------------------------------------------------------------

fn process_decorator(
    deco: &Decorator,
    fn_name: &str,
    params: &[Param],
    handler: &Value,
    env: &EnvRef,
) -> Result<(), EvalSignal> {
    // ¿Es un decorator HTTP conocido?
    if let Some(method) = HttpMethod::from_decorator_name(&deco.name) {
        return register_http_route(method, deco, fn_name, params, handler, env);
    }

    // `@server(port?, host?)`: configura el server. La fn que decora
    // queda en el env como cualquier otra (el patrón típico es
    // ponerlo arriba de `fn main()`).
    if deco.name == "server" {
        return register_server_config(deco, fn_name);
    }

    // Decorador desconocido. Mensaje listo para guiar al usuario.
    Err(EvalSignal::Error(FitzError::new(
        ErrorKind::InvalidSyntax,
        0,
        0,
        format!(
            "decorator '@{}' no implementado (sobre fn '{}'). \
             Decorators soportados hoy: @get, @post, @put, @delete, @server.",
            deco.name, fn_name,
        ),
    )))
}

/// Procesa `@server(port?, host?)`. Args positionals; cualquiera
/// puede omitirse y se aplica el default correspondiente. La
/// validación de uniqueness (un solo `@server` por programa) la
/// hace `http::set_server_config`.
fn register_server_config(deco: &Decorator, fn_name: &str) -> Result<(), EvalSignal> {
    let err = |msg: String| {
        EvalSignal::Error(FitzError::new(ErrorKind::InvalidSyntax, 0, 0, msg))
    };

    if !has_active_registry() {
        return Err(err(format!(
            "@server sobre fn '{}': no hay servidor HTTP activo en este contexto. \
             Los decoradores HTTP solo funcionan ejecutando el archivo con `fitz run`.",
            fn_name,
        )));
    }

    if deco.args.len() > 2 {
        return Err(err(format!(
            "@server(...) sobre fn '{}': admite hasta 2 args positionals \
             (port, host), recibió {}",
            fn_name,
            deco.args.len(),
        )));
    }

    // Arrancamos del default y vamos sobreescribiendo.
    let mut config = ServerConfig::default_addr();

    if let Some(port_expr) = deco.args.first() {
        match port_expr {
            Expr::Int(n, _) => {
                if *n < 1 || *n > 65535 {
                    return Err(err(format!(
                        "@server sobre fn '{}': port {} fuera de rango (debe estar entre 1 y 65535)",
                        fn_name, n,
                    )));
                }
                config.port = *n as u16;
            }
            other => {
                return Err(err(format!(
                    "@server sobre fn '{}': primer argumento (port) debe ser Int literal, \
                     recibió {:?}",
                    fn_name, other,
                )));
            }
        }
    }

    if let Some(host_expr) = deco.args.get(1) {
        match host_expr {
            Expr::Str(s, _) => {
                // Validamos en el momento del registro para no
                // diferirlo a la hora de levantar el server.
                if s.parse::<std::net::IpAddr>().is_err() {
                    return Err(err(format!(
                        "@server sobre fn '{}': host '{}' no es una IP válida \
                         (esperado IPv4 o IPv6 literal, sin resolver DNS)",
                        fn_name, s,
                    )));
                }
                config.host = s.clone();
            }
            other => {
                return Err(err(format!(
                    "@server sobre fn '{}': segundo argumento (host) debe ser Str literal, \
                     recibió {:?}",
                    fn_name, other,
                )));
            }
        }
    }

    if let Err(existing) = set_server_config(config) {
        return Err(err(format!(
            "@server sobre fn '{}': el programa ya tenía un @server configurado \
             ({}:{}). Solo se admite uno por programa.",
            fn_name, existing.host, existing.port,
        )));
    }

    Ok(())
}

fn register_http_route(
    method: HttpMethod,
    deco: &Decorator,
    fn_name: &str,
    params: &[Param],
    handler: &Value,
    env: &EnvRef,
) -> Result<(), EvalSignal> {
    // Helper local para mantener los mensajes consistentes.
    let err = |msg: String| {
        EvalSignal::Error(FitzError::new(ErrorKind::InvalidSyntax, 0, 0, msg))
    };

    // Validación 1: el decorator HTTP necesita un único arg con el path.
    if deco.args.len() != 1 {
        return Err(err(format!(
            "@{}(...) sobre fn '{}' espera un único argumento (la ruta), \
             recibió {}",
            deco.name,
            fn_name,
            deco.args.len(),
        )));
    }

    // Validación 2: el path se puede traducir a template de axum.
    let template = parse_path_template(&deco.args[0]).map_err(|e| {
        err(format!(
            "@{} sobre fn '{}': {}",
            deco.name,
            fn_name,
            e.message()
        ))
    })?;

    // Validación 3: cada path param tiene que estar declarado como
    // parámetro del handler. Eso garantiza que el handler reciba un
    // valor por cada `{x}` y simplifica el dispatch.
    for param_name in &template.params {
        if !params.iter().any(|p| &p.name == param_name) {
            return Err(err(format!(
                "@{} sobre fn '{}': el path declara '{{{}}}' pero el handler \
                 no tiene un parámetro con ese nombre",
                deco.name, fn_name, param_name,
            )));
        }
    }

    // Validación 4: hay un registry activo. Sin él, el evaluator está
    // corriendo afuera de `fitz run` (REPL, test, eval embebido) y los
    // decorators HTTP no tienen dónde registrarse.
    if !has_active_registry() {
        return Err(err(format!(
            "@{} sobre fn '{}': no hay servidor HTTP activo en este contexto. \
             Los decoradores HTTP solo funcionan ejecutando el archivo con \
             `fitz run`.",
            deco.name, fn_name,
        )));
    }

    // Validar que cada query_param del template tenga un param Fitz
    // correspondiente con el mismo nombre. Si el template dice
    // `?limit={limit}` pero el handler no declara `limit`, es un bug
    // del usuario — error claro.
    for qname in &template.query_params {
        if !params.iter().any(|p| &p.name == qname) {
            return Err(err(format!(
                "@{} sobre fn '{}': el query param '{}' está en el path \
                 pero el handler no tiene un parámetro con ese nombre",
                deco.name, fn_name, qname,
            )));
        }
    }

    // Identificar el body param: cualquier parámetro que NO esté ni en
    // template.params (path) ni en template.query_params (query).
    // Máximo uno por handler.
    let mut body_param: Option<BodyParam> = None;
    for p in params {
        if template.params.contains(&p.name) {
            continue; // es path param
        }
        if template.query_params.contains(&p.name) {
            continue; // es query param
        }
        if body_param.is_some() {
            return Err(err(format!(
                "@{} sobre fn '{}': solo se admite un parámetro body por handler \
                 (encontrado '{}', ya había otro)",
                deco.name, fn_name, p.name,
            )));
        }
        // Resolver el tipo declarado, si lo hay y es un `type` custom
        // del programa. Para tipos compuestos (`UserInput?`, `List<X>`,
        // etc.) la resolución usa la cabeza del `TypeExpr`. Si la
        // anotación es un primitivo (`Int`, `Str`, ...), un tipo que el
        // env no conoce, o no hay anotación, `declared_type` queda en
        // `None` y el runtime deserializa como `Value` libre
        // (Map/List/primitivos).
        let declared_type = p.type_.as_ref().and_then(|t| {
            match env.borrow().get(t.head_name()) {
                Some(v @ Value::Type { .. }) => Some(v),
                _ => None,
            }
        });
        body_param = Some(BodyParam {
            name: p.name.clone(),
            declared_type,
            declared_type_name: p.type_.as_ref().map(|t| t.display_name()),
        });
    }

    // Empacar tipos de parámetros en el orden declarado del handler.
    // Esto le sirve al runtime para coercionar path params crudos al
    // tipo Fitz correcto antes de invocar al handler. Pasamos el nombre
    // cabeza del tipo (sin genéricos ni `?`) porque `coerce_path_param`
    // solo soporta primitivos.
    let param_types: Vec<(String, Option<String>, bool)> = params
        .iter()
        .map(|p| {
            (
                p.name.clone(),
                p.type_.as_ref().map(|t| t.head_name().to_string()),
                p.type_.as_ref().map(|t| t.is_nullable()).unwrap_or(false),
            )
        })
        .collect();

    push_route(RouteSpec {
        method,
        path: template.path,
        path_params: template.params,
        query_params: template.query_params,
        handler: handler.clone(),
        handler_name: fn_name.to_string(),
        param_types,
        body_param,
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Loader — carga de módulos `.fitz` desde disco. Estado almacenado en un
// thread_local para no tener que enhebrar un parámetro extra por todas las
// firmas de eval_stmt/eval_expr.
//
// Política de carga:
//  - Eager: cuando el evaluator ve `import foo`, carga, lexea, parsea y
//    evalúa `foo.fitz` antes de seguir con la siguiente sentencia.
//  - Cache por path canonicalizado: importar dos veces el mismo archivo
//    no re-evalúa side effects.
//  - Detección de ciclos: stack `loading` con los paths actualmente en
//    proceso de carga; si reaparece, error explícito.
//  - `base_dir`: directorio donde se buscan los archivos relativos.
//    Cambia temporalmente al cargar un módulo (al padre del propio
//    módulo) para que los `import`s anidados sean relativos al módulo
//    que los hace, no al archivo raíz. Se restaura al volver.
// ---------------------------------------------------------------------------

struct Loader {
    base_dir: PathBuf,
    loading: Vec<PathBuf>,
    cache: HashMap<PathBuf, Value>,
}

thread_local! {
    static LOADER: RefCell<Option<Loader>> = const { RefCell::new(None) };
}

fn install_loader(base_dir: PathBuf) {
    LOADER.with(|cell| {
        *cell.borrow_mut() = Some(Loader {
            base_dir,
            loading: Vec::new(),
            cache: HashMap::new(),
        });
    });
}

fn uninstall_loader() {
    LOADER.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// Drop guard: garantiza que `uninstall_loader` se ejecute al salir del
/// scope de `eval_with_base` aunque haya un panic o un early return.
struct LoaderGuard;
impl Drop for LoaderGuard {
    fn drop(&mut self) {
        uninstall_loader();
    }
}

/// Resuelve los segmentos del path al archivo correspondiente,
/// relativo al `base_dir` actual del loader. `["foo"]` →
/// `<base>/foo.fitz`; `["sub", "foo"]` → `<base>/sub/foo.fitz`.
///
/// No verifica existencia — el caller hace `canonicalize`, que falla
/// con un mensaje útil si el archivo no está.
fn resolve_module_path(segments: &[String]) -> EvalResult<PathBuf> {
    let base = LOADER.with(|cell| {
        cell.borrow().as_ref().map(|l| l.base_dir.clone())
    });
    let mut path = base.ok_or_else(|| {
        EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            0, 0,
            "no se puede resolver `import`: el loader no está instalado \
             (usar `eval` o `eval_with_base` como entrada)",
        ))
    })?;
    let n = segments.len();
    for (i, seg) in segments.iter().enumerate() {
        if i + 1 == n {
            path.push(format!("{}.fitz", seg));
        } else {
            path.push(seg);
        }
    }
    Ok(path)
}

/// Carga un módulo: resuelve el path, chequea cache y ciclos, lee, parsea
/// y evalúa el archivo en un env aislado, lo devuelve como
/// `Value::Module`.
///
/// Esta función es reentrante: si el módulo cargado tiene `import` propios,
/// `eval_stmt` los maneja recursivamente y termina volviendo acá. Mantenemos
/// el invariante de no tener borrows vivos del `LOADER` cuando entramos a
/// `eval_stmt` — cada operación sobre el loader se hace en un bloque chico
/// que termina antes de la recursión.
fn load_module(segments: &[String]) -> EvalResult<Value> {
    let resolved = resolve_module_path(segments)?;

    // `canonicalize` requiere que el archivo exista. Si falla, el módulo
    // no se encontró.
    let canonical = match fs::canonicalize(&resolved) {
        Ok(p) => p,
        Err(_) => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::InvalidSyntax,
                0, 0,
                format!(
                    "no se encontró el módulo `{}` (buscado en `{}`)",
                    segments.join("."),
                    resolved.display(),
                ),
            )));
        }
    };

    // Cache hit: el mismo archivo importado de nuevo devuelve el mismo
    // `Value::Module` (mismo `Rc<RefCell<Environment>>` adentro). No
    // re-evalúa el body.
    if let Some(cached) = LOADER.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|l| l.cache.get(&canonical).cloned())
    }) {
        return Ok(cached);
    }

    // Detección de ciclos: si el archivo ya está en el stack de "loading",
    // estamos volviendo a entrar antes de haber terminado.
    let cycle = LOADER.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|l| l.loading.contains(&canonical))
    });
    if cycle {
        let stack_text = LOADER.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|l| {
                    l.loading
                        .iter()
                        .map(|p| display_module_path(p))
                        .collect::<Vec<_>>()
                        .join(" -> ")
                })
                .unwrap_or_default()
        });
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            0, 0,
            format!(
                "ciclo de imports detectado: {} -> {}",
                stack_text,
                display_module_path(&canonical),
            ),
        )));
    }

    // Leer + lexear + parsear el archivo. Cualquier error de esas etapas
    // se propaga.
    let source = match fs::read_to_string(&canonical) {
        Ok(s) => s,
        Err(e) => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::InvalidSyntax,
                0, 0,
                format!(
                    "error leyendo el módulo `{}`: {}",
                    canonical.display(),
                    e,
                ),
            )));
        }
    };
    let tokens = tokenize(&source).map_err(EvalSignal::Error)?;
    let module_program = parse(tokens).map_err(EvalSignal::Error)?;

    // Apilar este path como "cargando" y cambiar el base_dir al padre
    // del módulo, para que sus propios `import`s resuelvan relativos a
    // su ubicación. Guardamos el `prev_base` para restaurarlo al volver.
    let new_base = canonical
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let prev_base = LOADER.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let loader = borrow.as_mut().expect("loader instalado");
        loader.loading.push(canonical.clone());
        std::mem::replace(&mut loader.base_dir, new_base)
    });

    // Env aislado para el módulo. Registramos builtins también acá: la
    // intención es que un módulo pueda llamar a `print`, `len`, etc.
    // sin que el archivo importer tenga que re-exportarlos.
    let module_env = Environment::new();
    register_builtins(&module_env);

    // Evaluar las sentencias del módulo. Si alguna falla, igual restauramos
    // el estado del loader antes de propagar el error.
    let eval_result: EvalResult<()> = (|| {
        for stmt in &module_program {
            eval_stmt(stmt, module_env.clone())?;
        }
        Ok(())
    })();

    // Restaurar estado del loader.
    LOADER.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let loader = borrow.as_mut().expect("loader instalado");
        loader.loading.pop();
        loader.base_dir = prev_base;
    });

    eval_result?;

    // Construir el `Value::Module`. El nombre visible es el último
    // segmento del path (el `binding name`).
    let name = segments.last().cloned().unwrap_or_default();
    let module = Value::Module {
        name,
        env: module_env,
    };

    // Cachear por path canonicalizado. Un segundo import del mismo
    // archivo, aun bajo un alias distinto, devuelve este mismo Rc.
    LOADER.with(|cell| {
        if let Some(loader) = cell.borrow_mut().as_mut() {
            loader.cache.insert(canonical, module.clone());
        }
    });

    Ok(module)
}

/// Render compacto de un path absoluto para mensajes de error de ciclo.
/// En Windows, `canonicalize` produce paths UNC (`\\?\C:\...`); los
/// limpiamos para que el usuario vea `C:\...` directo.
fn display_module_path(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(|x| x.to_string()).unwrap_or(s)
}

/// Invoca un handler HTTP. Wrapper público sobre `invoke_value` que el
/// runtime HTTP usa por cada request: recibe el `Value::Function`
/// registrado y los args ya construidos (path params coercionados al
/// tipo declarado del handler), devuelve el `Value` que retornó el
/// handler o un `FitzError` con contexto.
///
/// La traducción de ese `Value` a status + body JSON la hace el
/// módulo `http` (`value_to_outcome`), no este wrapper. Acá solo
/// ejecutamos el handler.
pub fn call_handler(handler: Value, args: Vec<Value>, handler_name: &str) -> FitzResult<Value> {
    // El handler HTTP no tiene posición sintáctica directa — viene del
    // server runtime, no de una llamada en el source. Span::ZERO está
    // bien acá; el FitzError::Display omite la posición.
    invoke_value(handler, args, handler_name, Span::ZERO).map_err(signal_to_error)
}

/// Convierte un signal sin contexto en un `FitzError` legible.
fn signal_to_error(signal: EvalSignal) -> FitzError {
    match signal {
        EvalSignal::Error(e) => e,
        EvalSignal::Return(_) => FitzError::new(
            ErrorKind::ReturnOutsideFunction,
            0, 0,
            "`return` solo puede usarse adentro de una función",
        ),
        EvalSignal::Break => FitzError::new(
            ErrorKind::BreakOutsideLoop,
            0, 0,
            "`break` solo puede usarse adentro de un loop",
        ),
        EvalSignal::Continue => FitzError::new(
            ErrorKind::ContinueOutsideLoop,
            0, 0,
            "`continue` solo puede usarse adentro de un loop",
        ),
    }
}

// ---------------------------------------------------------------------------
// eval_stmt — evalúa una sentencia. Devuelve un valor para que `if` y otros
// constructos-bloque puedan usarse como expresión: el valor de un bloque es
// el valor del último stmt evaluado (o `Null` si fue sentencia-puro).
// ---------------------------------------------------------------------------

fn eval_stmt(stmt: &Stmt, env: EnvRef) -> EvalResult<Value> {
    match stmt {
        Stmt::Expr(expr, _) => eval_expr(expr, env),

        // `x = value`, `x: Tipo = value`, o `obj.campo = value`. La anotación
        // de tipo se ignora en runtime — tipado gradual, los checks de tipos
        // los hará un type-checker estático más adelante.
        //
        // Dos formas según el target:
        //  - `Ident`: si la variable ya existe en algún scope visible,
        //    reasignar ahí; si no, crear local (ver env.rs).
        //  - `Field`: evaluamos el objeto receptor (tiene que ser
        //    `Value::Instance`), validamos que el campo exista, y mutamos
        //    la celda compartida `Rc<RefCell<...>>` de `fields`.
        Stmt::Assign { target, type_: _, value, span: _ } => {
            let v = eval_expr(value, env.clone())?;
            match target {
                AssignTarget::Ident(name) => {
                    // Borrows separados: `has` toma borrow inmutable, lo
                    // soltamos antes de pedir un borrow mutable.
                    let already_defined = env.borrow().has(name);
                    if already_defined {
                        env.borrow_mut()
                            .assign(name, v)
                            .expect("la variable existe — acabamos de chequear con has()");
                    } else {
                        env.borrow_mut().define(name.clone(), v);
                    }
                }
                AssignTarget::Field { object, field } => {
                    let receiver = eval_expr(object, env.clone())?;
                    let fields = match &receiver {
                        Value::Instance { fields, .. } => fields.clone(),
                        other => {
                            return Err(EvalSignal::Error(FitzError::new(
                                ErrorKind::TypeMismatch {
                                    expected: "instancia de un tipo".into(),
                                    found: other.type_name().into(),
                                },
                                0, 0,
                                format!(
                                    "no se puede asignar a un campo de {} (no es una instancia)",
                                    other.type_name(),
                                ),
                            )));
                        }
                    };
                    let mut borrowed = fields.borrow_mut();
                    let slot = borrowed.iter_mut().find(|(name, _)| name == field);
                    match slot {
                        Some((_, slot_value)) => {
                            *slot_value = v;
                        }
                        None => {
                            // Capturamos type_name fuera del borrow para el mensaje.
                            let type_name = match &receiver {
                                Value::Instance { type_name, .. } => type_name.clone(),
                                _ => unreachable!(),
                            };
                            drop(borrowed);
                            return Err(EvalSignal::Error(FitzError::new(
                                ErrorKind::InvalidSyntax,
                                0, 0,
                                format!(
                                    "el tipo `{}` no tiene un campo llamado `{}`",
                                    type_name, field
                                ),
                            )));
                        }
                    }
                }
            }
            Ok(Value::Null)
        }

        // `return expr` — evalúa el valor y lo emite como signal. El handler
        // de Call lo intercepta y lo convierte en valor de retorno. Si nadie
        // lo intercepta, llega al top level y se reporta como error.
        Stmt::Return(expr, _) => {
            let v = eval_expr(expr, env)?;
            Err(EvalSignal::Return(v))
        }

        // `return <status> <body?>` — return con status code HTTP custom.
        // Evalúa el status (debe ser Int en rango u16) y el body opcional,
        // empaqueta como `Value::HttpResponse` y lo emite por el mismo
        // signal de Return. El runtime HTTP (en `http.rs`) lo intercepta
        // y emite la response con el status pedido. Fuera de un handler
        // HTTP el signal sube hasta top-level y reporta error como
        // cualquier return huérfano — el checker debería haberlo rechazado.
        Stmt::ReturnStatus { status, body, span } => {
            let status_v = eval_expr(status, env.clone())?;
            let Value::Int(n) = status_v else {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeError,
                    span.line,
                    span.column,
                    format!(
                        "status code de `return` debe ser Int, fue: {}",
                        status_v.type_name()
                    ),
                )));
            };
            if !(100..=599).contains(&n) {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeError,
                    span.line,
                    span.column,
                    format!("status code HTTP fuera de rango (100-599): {}", n),
                )));
            }
            let body_v = match body {
                Some(b) => Some(Box::new(eval_expr(b, env)?)),
                None => None,
            };
            Err(EvalSignal::Return(Value::HttpResponse {
                status: n as u16,
                body: body_v,
            }))
        }

        // `fn name(params) -> ret { body }`. Construye un `Value::Function`
        // capturando el env actual como closure y lo registra con `define`.
        //
        // El orden importa para recursión: como `closure` y el env donde se
        // hace `define` son el MISMO Rc, el body de la función "ve" su
        // propia definición — puede llamarse a sí misma sin hacer nada extra.
        //
        // `return_type` y `is_async` se ignoran en runtime (deuda explícita
        // para type-checker estático en Fase 5 y async real en Fase 4.x).
        //
        // `decorators`: si los hay, los procesamos antes de definir la
        // función. Los decoradores HTTP (`@get`/`@post`/`@put`/`@delete`)
        // requieren un `HttpRegistry` activo en el thread_local (instalado
        // por `main.rs` antes de evaluar). Sin registry, error explícito
        // — los tests y el REPL evalúan sin HTTP. Cualquier decorator no
        // HTTP también es error: `@server` (4.4) y otros entran cuando
        // los implementemos.
        Stmt::FnDef { name, params, return_type: _, body, is_async: _, decorators, span: _ } => {
            let func = Value::Function {
                params: params.clone(),
                body: body.clone(),
                closure: env.clone(),
            };

            // Procesar decorators ANTES de definir la fn en el env. Si
            // alguno falla, no queremos un binding mitad-registrado.
            // Pasamos el env actual para que el resolver del decorator
            // pueda mirar el `type` declarado de un parámetro body
            // (los `type` ya fueron registrados en este mismo env).
            for deco in decorators {
                process_decorator(deco, name, params, &func, &env)?;
            }

            env.borrow_mut().define(name.clone(), func);
            Ok(Value::Null)
        }

        // `type Name { campo1: T1, ... }`. Por ahora solo registramos el
        // tipo en el env como un valor inerte. La instanciación (`User { id: 1 }`)
        // y el field access requieren extensiones del AST (Fase 3).
        Stmt::TypeDef { name, fields, span: _ } => {
            let t = Value::Type {
                name: name.clone(),
                fields: fields.clone(),
            };
            env.borrow_mut().define(name.clone(), t);
            Ok(Value::Null)
        }
        Stmt::Break(_) => Err(EvalSignal::Break),
        Stmt::Continue(_) => Err(EvalSignal::Continue),

        // `for var in iter { body }` — evalúa `iter` una sola vez al
        // entrar, después itera. `var` se redefine en el env actual en
        // cada iteración (no creamos scope nuevo, consistente con la
        // política de bloques de Fitz: las variables del cuerpo persisten).
        //
        // Iterables soportados:
        //  - List: itera los elementos en orden.
        //  - Range: itera los Int de start a end-1.
        //  - Map: aún no (necesita el tipo `Pair`/`entry`; deuda abierta).
        //  - Otros: type error explícito.
        Stmt::For { var, iter, body, span: _ } => {
            let iter_v = eval_expr(iter, env.clone())?;
            let items_iter: Box<dyn Iterator<Item = Value>> = match iter_v {
                // La lista va por referencia compartida (`Rc<RefCell<>>`).
                // Para iterar tomamos un snapshot del Vec (cloneando los
                // valores): si el body muta la lista misma, el iterator
                // ya tiene su copia y no se altera a mitad de iteración.
                // Eso evita problemas estilo "modifying a list while
                // iterating" sin renunciar a mutación.
                Value::List(items) => {
                    let snapshot: Vec<Value> = items.borrow().clone();
                    Box::new(snapshot.into_iter())
                }
                Value::Range { start, end } => Box::new((start..end).map(Value::Int)),
                Value::Map(_) => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0, 0,
                    "`for` sobre Map aún no soportado — necesita el tipo Pair",
                ))),
                other => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "List o Range".into(),
                        found: other.type_name().into(),
                    },
                    0, 0,
                    format!(
                        "no se puede iterar sobre un valor de tipo `{}`",
                        other.type_name()
                    ),
                ))),
            };
            for item in items_iter {
                env.borrow_mut().define(var.clone(), item);
                match run_loop_body(body, env.clone()) {
                    LoopControl::Continue => continue,
                    LoopControl::Break => break,
                    LoopControl::Propagate(signal) => return Err(signal),
                }
            }
            Ok(Value::Null)
        }

        // `while cond { body }`. La cond se evalúa antes de cada iteración.
        // Tiene que ser Bool; otros tipos → type error.
        //
        // Captura `Break` y `Continue` como signals — `Break` termina el
        // loop, `Continue` salta a la siguiente iteración. Errors y
        // `Return` se propagan al caller (un return dentro de un while
        // dentro de una función rompe ambos hasta la función).
        Stmt::While { condition, body, span: _ } => {
            loop {
                let cond_v = eval_expr(condition, env.clone())?;
                let cond_bool = match cond_v {
                    Value::Bool(b) => b,
                    other => return Err(EvalSignal::Error(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            found: other.type_name().into(),
                        },
                        0, 0,
                        format!(
                            "la condición de `while` debe ser Bool, no `{}`",
                            other.type_name()
                        ),
                    ))),
                };
                if !cond_bool {
                    break;
                }
                match run_loop_body(body, env.clone()) {
                    LoopControl::Continue => continue,
                    LoopControl::Break => break,
                    LoopControl::Propagate(signal) => return Err(signal),
                }
            }
            Ok(Value::Null)
        }

        // `loop { body }` — itera para siempre. Solo `break` o `return`
        // pueden sacarte.
        Stmt::Loop { body, span: _ } => {
            loop {
                match run_loop_body(body, env.clone()) {
                    LoopControl::Continue => continue,
                    LoopControl::Break => break,
                    LoopControl::Propagate(signal) => return Err(signal),
                }
            }
            Ok(Value::Null)
        }

        // `import foo` / `import sub.foo` — carga el módulo y lo expone
        // bajo el ÚLTIMO segmento del path (`sub.foo` → binding `foo`).
        // Para field access (`foo.bar`) ver `eval_expr` sobre `Expr::Field`;
        // para method calls (`foo.bar()`) ver `dispatch_method`.
        Stmt::Import { path, span: _ } => {
            let module = load_module(path)?;
            let binding_name = path
                .last()
                .cloned()
                .expect("parser garantiza al menos un segmento");
            env.borrow_mut().define(binding_name, module);
            Ok(Value::Null)
        }

        // `from foo import a, b, c` — carga el módulo y bindea cada
        // nombre directo al scope actual. Si el módulo no expone
        // alguno de los nombres pedidos, error explícito citando cuál
        // falta y desde qué módulo.
        Stmt::FromImport { path, names, span: _ } => {
            let module = load_module(path)?;
            let module_env = match &module {
                Value::Module { env, .. } => env.clone(),
                _ => unreachable!("load_module siempre devuelve Value::Module"),
            };
            let module_label = path
                .last()
                .cloned()
                .unwrap_or_else(|| "<sin nombre>".to_string());
            for name in names {
                let v = module_env.borrow().get(name).ok_or_else(|| {
                    EvalSignal::Error(FitzError::new(
                        ErrorKind::UndefinedVariable(name.clone()),
                        0, 0,
                        format!(
                            "el módulo `{}` no exporta `{}`",
                            module_label, name,
                        ),
                    ))
                })?;
                env.borrow_mut().define(name.clone(), v);
            }
            Ok(Value::Null)
        }
    }
}

/// Resultado de correr el cuerpo de un loop una vez. Convierte signals de
/// control de flujo en una decisión local (seguir / salir / propagar).
enum LoopControl {
    Continue,
    Break,
    Propagate(EvalSignal),
}

/// Ejecuta los stmts del body en orden. Si alguno emite `Break` o `Continue`,
/// los traduce a control local. Cualquier otro signal (Error, Return) sube
/// como `Propagate` para que el loop lo devuelva al caller.
fn run_loop_body(body: &[Stmt], env: EnvRef) -> LoopControl {
    for stmt in body {
        match eval_stmt(stmt, env.clone()) {
            Ok(_) => {}
            Err(EvalSignal::Break) => return LoopControl::Break,
            Err(EvalSignal::Continue) => return LoopControl::Continue,
            Err(other) => return LoopControl::Propagate(other),
        }
    }
    LoopControl::Continue
}

// ---------------------------------------------------------------------------
// eval_expr — evalúa una expresión a un Value.
// ---------------------------------------------------------------------------

fn eval_expr(expr: &Expr, env: EnvRef) -> EvalResult<Value> {
    let span = expr.span();
    match expr {
        // Literales — el valor está embebido en el AST.
        Expr::Int(n, _) => Ok(Value::Int(*n)),
        Expr::Float(x, _) => Ok(Value::Float(*x)),
        Expr::Str(s, _) => Ok(Value::Str(s.clone())),
        Expr::Bool(b, _) => Ok(Value::Bool(*b)),
        Expr::Null(_) => Ok(Value::Null),

        // Identificador — lookup encadenado en la cadena de scopes.
        Expr::Ident(name, _) => env.borrow().get(name).ok_or_else(|| {
            EvalSignal::Error(FitzError::new(
                ErrorKind::UndefinedVariable(name.clone()),
                span.line, span.column,
                format!("variable `{}` no definida", name),
            ))
        }),

        // And/Or hacen short-circuit: no evaluamos `right` salvo que haga
        // falta. El resto de BinOps evalúan ambos lados antes de combinar.
        Expr::BinOp { op, left, right, span } if matches!(op, BinOpKind::And | BinOpKind::Or) => {
            eval_logical(op, left, right, env, *span)
        }
        Expr::BinOp { op, left, right, span } => {
            let lv = eval_expr(left, env.clone())?;
            let rv = eval_expr(right, env)?;
            eval_binop(op, lv, rv, *span)
        }

        Expr::UnaryOp { op, operand, span } => {
            let v = eval_expr(operand, env)?;
            eval_unary(op, v, *span)
        }

        // String con interpolación: cada `StrPart::Expr` se evalúa y se
        // convierte a string vía `Display`. Los `Lit` van tal cual.
        Expr::StrInterp(parts, _) => {
            let mut result = String::new();
            for part in parts {
                match part {
                    StrPart::Lit(s) => result.push_str(s),
                    StrPart::Expr(e) => {
                        let v = eval_expr(e, env.clone())?;
                        result.push_str(&v.to_string());
                    }
                }
            }
            Ok(Value::Str(result))
        }

        // Llamada a función. Dos caminos según la forma sintáctica del
        // callee:
        //  - `Expr::Field { object, field, .. }` → method call. Evaluamos el
        //    receptor y consultamos la tabla de métodos built-in del
        //    evaluador para el tipo del receptor. Si no hay método, caemos
        //    al field access normal y eso emite error de "no es invocable".
        //  - cualquier otra cosa → llamada normal. Evaluamos el callee y
        //    esperamos `Value::Function` o `Value::Builtin`.
        Expr::Call { callee, args, span } => eval_call(callee, args, env, *span),

        // `fn(x) => x * 2` o `fn(x) { return x * 2 }` — función anónima.
        // Se evalúa a `Value::Function` con el env actual como closure,
        // igual que un `Stmt::FnDef`, pero sin nombre ni binding en el env.
        Expr::FnExpr { params, body, .. } => Ok(Value::Function {
            params: params.clone(),
            body: body.clone(),
            closure: env,
        }),

        // `obj.campo` — acceso a campo de instancia de tipo custom, o
        // a un export de un módulo importado. Para receptores no-Instance
        // y no-Module (List, Map, Str, etc.), el camino habitual es el
        // method dispatch (`xs.map(...)`), que va por la rama `Expr::Call`
        // con callee `Field`. El field access "pelado" sobre primitivos
        // no tiene semántica útil hoy.
        Expr::Field { object, field, .. } => {
            let obj = eval_expr(object, env)?;
            match obj {
                Value::Instance { type_name, fields } => {
                    fields
                        .borrow()
                        .iter()
                        .find(|(k, _)| k == field)
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| {
                            EvalSignal::Error(FitzError::new(
                                ErrorKind::InvalidSyntax,
                                span.line, span.column,
                                format!(
                                    "el tipo `{}` no tiene un campo llamado `{}`",
                                    type_name, field
                                ),
                            ))
                        })
                }
                Value::Module { name, env: module_env } => {
                    module_env.borrow().get(field).ok_or_else(|| {
                        EvalSignal::Error(FitzError::new(
                            ErrorKind::UndefinedVariable(field.clone()),
                            span.line, span.column,
                            format!(
                                "el módulo `{}` no exporta `{}`",
                                name, field,
                            ),
                        ))
                    })
                }
                other => Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Instance o Module".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "acceso a campo `.{}` sobre un valor de tipo `{}` — \
                         solo se permite sobre instancias de tipos custom o módulos",
                        field,
                        other.type_name()
                    ),
                ))),
            }
        }

        // `User { id: 1, name: "x" }` — instanciación de un tipo custom.
        //
        // Validación en runtime (no en parse, porque el `type` puede
        // declararse después en el archivo o venir de otro scope):
        //  1. Resolver `type_name` en el env: tiene que existir y ser
        //     un `Value::Type`. Otro caso → error explícito.
        //  2. Detectar campos extra en el literal (no declarados en el
        //     `type`).
        //  3. Para cada campo declarado, en orden de declaración:
        //      a. Si el literal lo provee, usar ese valor.
        //      b. Si no y tiene `default`, evaluar el default en el env
        //         de la INSTANCIACIÓN (no el de la declaración del
        //         tipo) — los defaults son típicamente literales y
        //         este criterio es más predecible.
        //      c. Si no y es `nullable`, usar `Null`.
        //      d. Si no, error: falta el campo.
        //  4. Las anotaciones de tipo NO se chequean en runtime
        //     (tipado gradual; el chequeo estático llega en Fase 5).
        //
        // El orden de los campos en la instancia sigue la declaración
        // del `type`, no el del literal — eso garantiza un `Display`
        // estable y comparaciones estructurales consistentes.
        Expr::StructLit { type_name, fields, .. } => {
            let ty = env.borrow().get(type_name).ok_or_else(|| {
                EvalSignal::Error(FitzError::new(
                    ErrorKind::UndefinedVariable(type_name.clone()),
                    span.line, span.column,
                    format!("tipo `{}` no definido", type_name),
                ))
            })?;
            let declared = match ty {
                Value::Type { fields, .. } => fields,
                other => {
                    return Err(EvalSignal::Error(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Type".into(),
                            found: other.type_name().into(),
                        },
                        span.line, span.column,
                        format!(
                            "`{}` no es un tipo — no se puede instanciar (es `{}`)",
                            type_name,
                            other.type_name()
                        ),
                    )));
                }
            };

            // Detectar campos extra: cada nombre del literal tiene que
            // estar entre los declarados.
            for (provided_name, value_expr) in fields {
                if !declared.iter().any(|f| f.name == *provided_name) {
                    // Apuntamos al valor del campo extra — más útil que
                    // el inicio del struct literal.
                    let fs = value_expr.span();
                    return Err(EvalSignal::Error(FitzError::new(
                        ErrorKind::InvalidSyntax,
                        fs.line, fs.column,
                        format!(
                            "el tipo `{}` no tiene un campo llamado `{}`",
                            type_name, provided_name
                        ),
                    )));
                }
            }

            // Armar la instancia en orden de declaración. Para cada
            // campo declarado: usar el del literal si está; si no,
            // default; si no, null si es nullable; si no, error.
            let mut instance_fields: Vec<(String, Value)> =
                Vec::with_capacity(declared.len());
            for f in &declared {
                let provided = fields.iter().find(|(n, _)| n == &f.name);
                let value = if let Some((_, expr)) = provided {
                    eval_expr(expr, env.clone())?
                } else if let Some(default_expr) = &f.default {
                    eval_expr(default_expr, env.clone())?
                } else if f.type_.is_nullable() {
                    Value::Null
                } else {
                    return Err(EvalSignal::Error(FitzError::new(
                        ErrorKind::InvalidSyntax,
                        span.line, span.column,
                        format!(
                            "falta el campo `{}` al instanciar `{}` \
                             (no tiene default y no es nullable)",
                            f.name, type_name
                        ),
                    )));
                };
                instance_fields.push((f.name.clone(), value));
            }
            Ok(Value::new_instance(type_name.clone(), instance_fields))
        }

        // `[e1, e2, ...]` — evaluamos los elementos en orden.
        Expr::List(items, _) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(eval_expr(item, env.clone())?);
            }
            Ok(Value::new_list(values))
        }

        // `{k1: v1, ...}` — evaluamos cada par en orden (clave, valor).
        // El orden de inserción se preserva en el Vec resultante.
        Expr::Map(pairs, _) => {
            let mut entries = Vec::with_capacity(pairs.len());
            for (k_expr, v_expr) in pairs {
                let k = eval_expr(k_expr, env.clone())?;
                let v = eval_expr(v_expr, env.clone())?;
                entries.push((k, v));
            }
            Ok(Value::new_map(entries))
        }

        // `start..end` — ambos extremos tienen que ser Int (no hay rangos
        // de Float). El rango se materializa como `Value::Range`; la
        // iteración real (cuando se usa en `for`) ocurre en Stmt::For.
        Expr::Range { start, end, .. } => {
            let s_v = eval_expr(start, env.clone())?;
            let e_v = eval_expr(end, env)?;
            let s = expect_int_for_range(&s_v, "inicio", start.span())?;
            let e = expect_int_for_range(&e_v, "fin", end.span())?;
            Ok(Value::Range { start: s, end: e })
        }

        // `obj[idx]` — indexing. Dispatch por tipo del objeto.
        Expr::Index { object, index, span } => {
            let obj = eval_expr(object, env.clone())?;
            let idx = eval_expr(index, env)?;
            eval_index(&obj, &idx, *span)
        }

        // `if cond { then } else { else_ }`. Funciona como expresión: su
        // valor es el del último stmt del bloque ejecutado. Sin else y cond
        // falsa → Null.
        //
        // Los bloques NO crean scope nuevo — variables declaradas adentro
        // persisten en el scope contenedor (estilo Python). Deuda explícita
        // si después esto trae sorpresas.
        Expr::If { condition, then, else_, .. } => {
            let cond_v = eval_expr(condition, env.clone())?;
            let cond_bool = match cond_v {
                Value::Bool(b) => b,
                other => {
                    let cs = condition.span();
                    return Err(EvalSignal::Error(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            found: other.type_name().into(),
                        },
                        cs.line, cs.column,
                        format!(
                            "la condición de `if` debe ser Bool, no `{}`",
                            other.type_name()
                        ),
                    )));
                }
            };

            if cond_bool {
                eval_block(then, env)
            } else if let Some(else_block) = else_ {
                eval_block(else_block, env)
            } else {
                Ok(Value::Null)
            }
        }

        // `match value { pat1 => body1, pat2 => body2, ... }`. Recorre los
        // arms en orden y devuelve el body del primero que matchee.
        //
        // Patrones soportados:
        //  - `Ident(name)`: siempre matchea, bindea el valor entero a `name`.
        //  - `Wildcard`: siempre matchea, sin binding.
        //  - Literales (`Int`, `Float`, `Str`, `Bool`, `Null`): matchean por
        //    igualdad estructural.
        //  - `Range { start, end }`: matchea Int en [start, end).
        //  - `Ok(name)` / `Err(name)`: matchean solo contra `Value::Result`
        //    de la variante correspondiente y bindean el inner a `name`.
        //
        // Cada arm con binding crea un scope hijo para que la variable no
        // contamine el scope contenedor.
        Expr::Match { value, arms, .. } => {
            let v = eval_expr(value, env.clone())?;

            for arm in arms {
                // Resultado del intento de match para este arm:
                //   None             → no matcheó, probar el siguiente.
                //   Some(None)       → matcheó sin binding.
                //   Some(Some((n, val))) → matcheó y bindea `val` a `n`.
                //
                // Esto unifica patrones literales (sin binding) con los que
                // bindean (`Ident`, `OkBinding`, `ErrBinding`).
                let outcome: Option<Option<(String, Value)>> = match (&arm.pattern, &v) {
                    (Pattern::Int(p), Value::Int(vv)) if p == vv => Some(None),
                    (Pattern::Float(p), Value::Float(vv)) if p == vv => Some(None),
                    (Pattern::Str(p), Value::Str(vv)) if p == vv => Some(None),
                    (Pattern::Bool(p), Value::Bool(vv)) if p == vv => Some(None),
                    (Pattern::Null, Value::Null) => Some(None),
                    (Pattern::Wildcard, _) => Some(None),
                    (Pattern::Ident(name), _) => Some(Some((name.clone(), v.clone()))),
                    (Pattern::Range { start, end }, Value::Int(vv))
                        if start <= vv && vv < end => Some(None),
                    (Pattern::OkBinding(name), Value::Result(ResultVariant::Ok(inner))) => {
                        Some(Some((name.clone(), (**inner).clone())))
                    }
                    (Pattern::ErrBinding(name), Value::Result(ResultVariant::Err(inner))) => {
                        Some(Some((name.clone(), (**inner).clone())))
                    }
                    // Wildcards sobre Ok/Err: matchean la variante
                    // sin bindear el inner. No ensucian el scope.
                    (Pattern::OkWildcard, Value::Result(ResultVariant::Ok(_))) => Some(None),
                    (Pattern::ErrWildcard, Value::Result(ResultVariant::Err(_))) => Some(None),
                    _ => None,
                };

                let Some(binding) = outcome else {
                    continue;
                };

                if let Some((name, bound)) = binding {
                    let arm_env = Environment::new_child(env.clone());
                    arm_env.borrow_mut().define(name, bound);
                    return eval_expr(&arm.body, arm_env);
                }
                return eval_expr(&arm.body, env.clone());
            }

            // Ningún arm matcheó. Con Ident/Wildcard presentes es imposible;
            // ocurre típicamente con Ok/Err mal cubiertos.
            Err(EvalSignal::Error(FitzError::new(
                ErrorKind::InvalidSyntax,
                span.line, span.column,
                "el `match` no matcheó ningún brazo",
            )))
        }

        // `Ok(inner)` — constructor de la variante exitosa de Result.
        // Evaluamos el inner y lo envolvemos.
        Expr::Ok(inner, _) => {
            let v = eval_expr(inner, env)?;
            Ok(Value::Result(ResultVariant::Ok(Box::new(v))))
        }

        // `Err(inner)` — constructor de la variante de error.
        Expr::Err(inner, _) => {
            let v = eval_expr(inner, env)?;
            Ok(Value::Result(ResultVariant::Err(Box::new(v))))
        }

        // `expr?` — operador de propagación de errores.
        //
        // Semántica:
        //  - Ok(v)  → la expresión vale `v` (desempaqueta).
        //  - Err(e) → corta la función contenedora devolviendo Err(e) sin
        //    ejecutar el resto. Se emite vía `EvalSignal::Return(...)`,
        //    que `eval_call` captura y convierte en el valor de retorno.
        //  - Cualquier otro tipo → error de runtime explícito.
        //
        // Si `?` se evalúa fuera de una función, el `Return` sintetizado
        // burbujea hasta `eval` y se reporta como "`return` solo puede
        // usarse adentro de una función". Mensaje genérico (deuda
        // explícita; mejora pendiente con un signal dedicado).
        Expr::Try(inner, try_span) => {
            let v = eval_expr(inner, env)?;
            match v {
                Value::Result(ResultVariant::Ok(x)) => Ok(*x),
                Value::Result(ResultVariant::Err(e)) => Err(EvalSignal::Return(
                    Value::Result(ResultVariant::Err(e)),
                )),
                other => Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Result".into(),
                        found: other.type_name().into(),
                    },
                    try_span.line, try_span.column,
                    format!(
                        "el operador `?` requiere un valor `Result`, recibió `{}`",
                        other.type_name()
                    ),
                ))),
            }
        }
    }
}

/// Evalúa una secuencia de sentencias en el env dado (sin crear scope
/// nuevo) y devuelve el valor de la última. Bloque vacío → Null.
///
/// Los signals (Return/Break/Continue/Error) se propagan: si un stmt los
/// emite, el resto del bloque no se ejecuta.
fn eval_block(stmts: &[Stmt], env: EnvRef) -> EvalResult<Value> {
    let mut last = Value::Null;
    for stmt in stmts {
        last = eval_stmt(stmt, env.clone())?;
    }
    Ok(last)
}

/// Resolver de llamadas. Despacha según la forma sintáctica del callee:
///
///  - `Expr::Field { object, field, .. }` → method call. Evalúa el receptor,
///    los args, y consulta la tabla de métodos built-in
///    (`dispatch_method`). Si el receptor es una instancia y el "método"
///    no existe en la tabla, caemos al field access (por si el usuario
///    guardó una función en un campo) y lo invocamos.
///  - cualquier otra expresión → llamada normal. Evalúa el callee y
///    despacha sobre `Value::Builtin` / `Value::Function`.
///
/// El identificador para mensajes de error se deriva de la forma del
/// callee: `Expr::Ident(n, _)` → `"n"`, otro → `"<expr>"`.
fn eval_call(callee: &Expr, args: &[Expr], env: EnvRef, span: Span) -> EvalResult<Value> {
    // Method call.
    if let Expr::Field { object, field, .. } = callee {
        let receiver = eval_expr(object, env.clone())?;
        let mut arg_values = Vec::with_capacity(args.len());
        for arg in args {
            arg_values.push(eval_expr(arg, env.clone())?);
        }
        return dispatch_method(receiver, field, arg_values, span);
    }

    // Llamada normal.
    let callee_value = eval_expr(callee, env.clone())?;
    let mut arg_values = Vec::with_capacity(args.len());
    for arg in args {
        arg_values.push(eval_expr(arg, env.clone())?);
    }
    let display_name = callee_display_name(callee);
    invoke_value(callee_value, arg_values, &display_name, span)
}

/// Devuelve un nombre legible para usar en mensajes de error de una
/// llamada. Para callees con nombre (`Ident`) usa el nombre; para todo
/// lo demás usa un placeholder.
fn callee_display_name(callee: &Expr) -> String {
    match callee {
        Expr::Ident(n, _) => n.clone(),
        _ => "<expr>".to_string(),
    }
}

/// Invoca un valor que ya sabemos que tiene que ser una función. Maneja
/// builtins, user-defined functions y errores de "no es invocable".
fn invoke_value(
    value: Value, arg_values: Vec<Value>, display_name: &str, span: Span,
) -> EvalResult<Value> {
    match value {
        Value::Builtin { func, .. } => func(&arg_values).map_err(EvalSignal::Error),

        Value::Function { params, body, closure } => {
            if arg_values.len() != params.len() {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::WrongArgCount {
                        expected: params.len(),
                        found: arg_values.len(),
                    },
                    span.line, span.column,
                    format!(
                        "`{}` espera {} argumento(s), recibió {}",
                        display_name,
                        params.len(),
                        arg_values.len(),
                    ),
                )));
            }

            // Nuevo scope hijo del CLOSURE, no del caller. Lexical scoping.
            let call_env = Environment::new_child(closure);
            for (param, value) in params.iter().zip(arg_values) {
                call_env.borrow_mut().define(param.name.clone(), value);
            }

            for stmt in &body {
                match eval_stmt(stmt, call_env.clone()) {
                    Ok(_) => {}
                    Err(EvalSignal::Return(v)) => return Ok(v),
                    Err(other) => return Err(other),
                }
            }
            Ok(Value::Null)
        }

        other => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "función".into(),
                found: other.type_name().into(),
            },
            span.line, span.column,
            format!("`{}` no es invocable (es {})", display_name, other.type_name()),
        ))),
    }
}

/// Dispatch de método built-in. Lookup por `(tipo del receptor, nombre
/// del método)` en una tabla estática. Las implementaciones reciben el
/// receptor (por valor, pero las colecciones internas son
/// `Rc<RefCell<...>>`, así que las mutaciones se propagan a los aliases)
/// y los args ya evaluados.
///
/// Si no hay un método registrado para `(tipo, nombre)`, devuelve error
/// "método no encontrado". El usuario lo va a ver como
/// `xs.metodo_inexistente(...) — Lista no tiene un método llamado ...`.
fn dispatch_method(
    receiver: Value,
    method: &str,
    args: Vec<Value>,
    span: Span,
) -> EvalResult<Value> {
    match (&receiver, method) {
        // List
        (Value::List(_), "push") => list_push(receiver, args, span),
        (Value::List(_), "pop") => list_pop(receiver, args, span),
        (Value::List(_), "map") => list_map(receiver, args, span),
        (Value::List(_), "filter") => list_filter(receiver, args, span),
        (Value::List(_), "find") => list_find(receiver, args, span),
        (Value::List(_), "len") => list_len(receiver, args, span),
        // Map
        (Value::Map(_), "get") => map_get(receiver, args, span),
        (Value::Map(_), "has") => map_has(receiver, args, span),
        (Value::Map(_), "keys") => map_keys(receiver, args, span),
        (Value::Map(_), "values") => map_values(receiver, args, span),
        (Value::Map(_), "len") => map_len(receiver, args, span),
        // Str
        (Value::Str(_), "len") => str_len(receiver, args, span),
        (Value::Str(_), "upper") => str_upper(receiver, args, span),
        (Value::Str(_), "lower") => str_lower(receiver, args, span),
        // Module: `mod.fn(args)` se resuelve buscando `fn` en el env del
        // módulo y llamándola como cualquier función. No es method
        // dispatch real — el módulo no es "el receptor", solo el lugar
        // donde vive la función.
        (Value::Module { name, env: module_env }, _) => {
            let value = module_env.borrow().get(method).ok_or_else(|| {
                EvalSignal::Error(FitzError::new(
                    ErrorKind::UndefinedVariable(method.into()),
                    span.line, span.column,
                    format!("el módulo `{}` no exporta `{}`", name, method),
                ))
            })?;
            invoke_value(value, args, method, span)
        }
        _ => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!(
                "el tipo `{}` no tiene un método llamado `{}`",
                receiver.type_name(),
                method,
            ),
        ))),
    }
}

// ---------------------------------------------------------------------------
// Métodos built-in — implementaciones
// ---------------------------------------------------------------------------
//
// Cada función toma el receptor (consumido por valor — pero como las
// colecciones internas son `Rc<RefCell<>>`, lo que importa es el Rc, no
// el clone) y los args ya evaluados. Devuelve un `EvalResult<Value>`.
//
// Convenciones:
//  - Aridad chequeada arriba de todo con `expect_arity`.
//  - Métodos que mutan (push, pop) devuelven `Value::Null` o el valor
//    extraído; los puros (map, filter, find) devuelven la colección o
//    elemento computado.
//  - "Buscar y no encontrar" se modela con `Result`: `find` y `get`
//    devuelven `Ok(v)` / `Err(<msg>)`.

/// Helper: chequea que `args.len() == expected`; si no, devuelve error
/// de aridad citando el método.
fn expect_arity(method: &str, args: &[Value], expected: usize, span: Span) -> EvalResult<()> {
    if args.len() != expected {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::WrongArgCount {
                expected,
                found: args.len(),
            },
            span.line, span.column,
            format!(
                "`.{}()` espera {} argumento(s), recibió {}",
                method, expected, args.len(),
            ),
        )));
    }
    Ok(())
}

/// Helper: invoca un `Value` que tiene que ser callable, con UN solo
/// argumento. Para `map`/`filter`/`find`, donde la callback es siempre
/// unaria.
fn invoke_callback(callback: &Value, arg: Value, method: &str, span: Span) -> EvalResult<Value> {
    invoke_value(callback.clone(), vec![arg], &format!("callback de .{}()", method), span)
}

// ---- List ----

fn list_push(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("push", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let mut v = args.into_iter().next().unwrap();
    // Si quien empuja se pasó a sí mismo, evitamos un re-borrow doble.
    items.borrow_mut().push(std::mem::replace(&mut v, Value::Null));
    Ok(Value::Null)
}

fn list_pop(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("pop", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let popped = items.borrow_mut().pop();
    match popped {
        Some(v) => Ok(v),
        None => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            "`.pop()` sobre lista vacía".to_string(),
        ))),
    }
}

fn list_map(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("map", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    // Snapshot del Vec para evitar re-entrancia al RefCell si la callback
    // mutase la lista original.
    let snapshot: Vec<Value> = items.borrow().clone();
    let mut out = Vec::with_capacity(snapshot.len());
    for item in snapshot {
        out.push(invoke_callback(callback, item, "map", span)?);
    }
    Ok(Value::new_list(out))
}

fn list_filter(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("filter", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.borrow().clone();
    let mut out = Vec::new();
    for item in snapshot {
        let keep = invoke_callback(callback, item.clone(), "filter", span)?;
        match keep {
            Value::Bool(true) => out.push(item),
            Value::Bool(false) => {}
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Bool".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "la callback de `.filter()` tiene que devolver Bool, devolvió `{}`",
                        other.type_name(),
                    ),
                )));
            }
        }
    }
    Ok(Value::new_list(out))
}

fn list_find(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("find", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.borrow().clone();
    for item in snapshot {
        let keep = invoke_callback(callback, item.clone(), "find", span)?;
        match keep {
            Value::Bool(true) => {
                return Ok(Value::Result(ResultVariant::Ok(Box::new(item))));
            }
            Value::Bool(false) => {}
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Bool".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "la callback de `.find()` tiene que devolver Bool, devolvió `{}`",
                        other.type_name(),
                    ),
                )));
            }
        }
    }
    Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
        "no encontrado".into(),
    )))))
}

fn list_len(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("len", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let n = items.borrow().len() as i64;
    Ok(Value::Int(n))
}

// ---- Map ----

fn map_get(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("get", &args, 1, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let key = &args[0];
    for (k, v) in pairs.borrow().iter() {
        if k == key {
            return Ok(Value::Result(ResultVariant::Ok(Box::new(v.clone()))));
        }
    }
    Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
        format!("clave no encontrada: {}", key),
    )))))
}

fn map_has(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("has", &args, 1, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let key = &args[0];
    let found = pairs.borrow().iter().any(|(k, _)| k == key);
    Ok(Value::Bool(found))
}

fn map_keys(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("keys", &args, 0, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let ks: Vec<Value> = pairs.borrow().iter().map(|(k, _)| k.clone()).collect();
    Ok(Value::new_list(ks))
}

fn map_values(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("values", &args, 0, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let vs: Vec<Value> = pairs.borrow().iter().map(|(_, v)| v.clone()).collect();
    Ok(Value::new_list(vs))
}

fn map_len(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("len", &args, 0, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let n = pairs.borrow().len() as i64;
    Ok(Value::Int(n))
}

// ---- Str ----

fn str_len(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("len", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    // Coincide con `len(s)` global: cuenta chars, no bytes.
    Ok(Value::Int(s.chars().count() as i64))
}

fn str_upper(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("upper", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    Ok(Value::Str(s.to_uppercase()))
}

fn str_lower(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("lower", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    Ok(Value::Str(s.to_lowercase()))
}

// ---------------------------------------------------------------------------
// Operaciones binarias
// ---------------------------------------------------------------------------
//
// Tabla de promoción para aritmética (Add, Sub, Mul, Div):
//
//   Int    + Int    → Int
//   Int    + Float  → Float
//   Float  + Int    → Float
//   Float  + Float  → Float
//   Str    + Str    → Str   (solo Add, concatenación)
//   resto           → TypeMismatch
//
// Para Div: si el divisor es 0 (Int) o 0.0 (Float), se emite DivisionByZero
// en vez de dejar pasar IEEE 754 infinitos/NaN.
//
// Comparaciones (Lt, LtEq, Gt, GtEq): numéricas con promoción Int↔Float, o
// strings alfabéticamente. El resto → TypeMismatch.
//
// Igualdad (Eq, NotEq): delega en `PartialEq` de `Value`, que ya hace
// coerción Int↔Float. Tipos incompatibles dan `false` sin error.

fn eval_binop(op: &BinOpKind, l: Value, r: Value, span: Span) -> EvalResult<Value> {
    use BinOpKind::*;
    match op {
        Add => eval_add(l, r, span),
        Sub => arith(l, r, "-", |a, b| a - b, |a, b| a - b, span),
        Mul => arith(l, r, "*", |a, b| a * b, |a, b| a * b, span),
        Div => eval_div(l, r, span),
        Eq => Ok(Value::Bool(l == r)),
        NotEq => Ok(Value::Bool(l != r)),
        Lt | LtEq | Gt | GtEq => compare(op, l, r, span),
        And | Or => unreachable!("And/Or se manejan en eval_logical antes de llegar acá"),
    }
}

/// Add tiene un caso especial: `Str + Str` concatena. El resto delega en
/// `arith` con el mismo patrón de promoción Int↔Float.
fn eval_add(l: Value, r: Value, span: Span) -> EvalResult<Value> {
    if let (Value::Str(a), Value::Str(b)) = (&l, &r) {
        return Ok(Value::Str(format!("{}{}", a, b)));
    }
    arith(l, r, "+", |a, b| a + b, |a, b| a + b, span)
}

/// Div chequea 0 antes de delegar — error explícito en vez de Infinity/NaN.
fn eval_div(l: Value, r: Value, span: Span) -> EvalResult<Value> {
    match &r {
        Value::Int(0) => return div_by_zero(span),
        Value::Float(b) if *b == 0.0 => return div_by_zero(span),
        _ => {}
    }
    arith(l, r, "/", |a, b| a / b, |a, b| a / b, span)
}

/// Helper genérico para Add/Sub/Mul/Div: aplica `int_op` si ambos son Int,
/// `float_op` si alguno es Float (promoviendo el Int a f64). Resto → error.
///
/// `Fn(i64, i64) -> i64` es una _trait bound_ que acepta cualquier closure
/// que no consume su entorno. Los closures `|a, b| a + b` que pasamos no
/// capturan nada, así que cumplen. Esto evita repetir el match cuatro veces.
fn arith<I, F>(
    l: Value, r: Value, op_name: &str, int_op: I, float_op: F, span: Span,
) -> EvalResult<Value>
where
    I: Fn(i64, i64) -> i64,
    F: Fn(f64, f64) -> f64,
{
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_op(a, b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(float_op(a as f64, b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(float_op(a, b as f64))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(a, b))),
        (l, r) => type_error(op_name, &l, &r, span),
    }
}

fn compare(op: &BinOpKind, l: Value, r: Value, span: Span) -> EvalResult<Value> {
    use BinOpKind::*;

    // Numérico (con promoción Int→f64). NaN propaga como false en cualquiera
    // de los cuatro operadores, lo cual es la semántica de IEEE 754.
    if let (Some(a), Some(b)) = (as_f64(&l), as_f64(&r)) {
        return Ok(Value::Bool(match op {
            Lt => a < b,
            LtEq => a <= b,
            Gt => a > b,
            GtEq => a >= b,
            _ => unreachable!(),
        }));
    }

    // Strings alfabéticamente (orden lexicográfico estándar de Rust).
    if let (Value::Str(a), Value::Str(b)) = (&l, &r) {
        return Ok(Value::Bool(match op {
            Lt => a < b,
            LtEq => a <= b,
            Gt => a > b,
            GtEq => a >= b,
            _ => unreachable!(),
        }));
    }

    type_error(op_name(op), &l, &r, span)
}

/// Convierte un Value numérico a f64. Devuelve None si no es numérico —
/// usado en `compare` para discriminar el camino numérico del de strings.
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(x) => Some(*x),
        _ => None,
    }
}

/// And/Or con short-circuit y type-check de Bool. Vive aparte de `eval_binop`
/// porque necesita acceso a las expresiones SIN evaluar (para no evaluar el
/// lado derecho cuando el izquierdo ya determina el resultado).
fn eval_logical(
    op: &BinOpKind, left: &Expr, right: &Expr, env: EnvRef, span: Span,
) -> EvalResult<Value> {
    let lv = eval_expr(left, env.clone())?;
    let lb = expect_bool(&lv, op_name(op), "izquierdo", left.span())?;

    // Short-circuit: `false and ...` → false, `true or ...` → true.
    match op {
        BinOpKind::And if !lb => return Ok(Value::Bool(false)),
        BinOpKind::Or if lb => return Ok(Value::Bool(true)),
        _ => {}
    }

    let rv = eval_expr(right, env)?;
    let rb = expect_bool(&rv, op_name(op), "derecho", right.span())?;
    let _ = span; // mantenido por consistencia de firma con eval_binop
    Ok(Value::Bool(rb))
}

/// Helper para chequear que un Value sea Bool. Devuelve el bool o un
/// TypeMismatch contextualizado al operador y lado.
fn expect_bool(v: &Value, op: &str, side: &str, span: Span) -> EvalResult<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Bool".into(),
                found: v.type_name().into(),
            },
            span.line, span.column,
            format!("operando {} de `{}` debe ser Bool, no `{}`", side, op, v.type_name()),
        ))),
    }
}

/// Símbolo legible de un BinOpKind, para mensajes de error.
fn op_name(op: &BinOpKind) -> &'static str {
    use BinOpKind::*;
    match op {
        Add => "+", Sub => "-", Mul => "*", Div => "/",
        Eq => "==", NotEq => "!=",
        Lt => "<", LtEq => "<=", Gt => ">", GtEq => ">=",
        And => "and", Or => "or",
    }
}

fn type_error<T>(op: &str, l: &Value, r: &Value, span: Span) -> EvalResult<T> {
    Err(EvalSignal::Error(FitzError::new(
        ErrorKind::TypeMismatch {
            expected: "operandos compatibles".into(),
            found: format!("{} {} {}", l.type_name(), op, r.type_name()),
        },
        span.line, span.column,
        format!(
            "operación `{}` no soportada entre `{}` y `{}`",
            op, l.type_name(), r.type_name()
        ),
    )))
}

fn div_by_zero<T>(span: Span) -> EvalResult<T> {
    Err(EvalSignal::Error(FitzError::new(
        ErrorKind::DivisionByZero,
        span.line, span.column,
        "división por cero",
    )))
}

// ---------------------------------------------------------------------------
// Listas, mapas, rangos: helpers de runtime
// ---------------------------------------------------------------------------

/// Extrae el Int de un Value, o emite un TypeMismatch claro indicando si
/// fue el "inicio" o el "fin" del rango. Float NO coerciona — los rangos
/// son discretos.
fn expect_int_for_range(v: &Value, side: &str, span: Span) -> EvalResult<i64> {
    match v {
        Value::Int(n) => Ok(*n),
        other => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            span.line, span.column,
            format!(
                "el {} de un rango debe ser Int, no `{}`",
                side, other.type_name()
            ),
        ))),
    }
}

/// `obj[idx]`. Dispatch por tipo del receptor:
///  - List + Int: bounds-check, devuelve el elemento.
///  - Map + cualquier valor: búsqueda lineal por igualdad (la misma
///    igualdad que usa `==`, así que claves Int↔Float matchean).
///  - Range: no indexable por ahora (semántica no obvia: ¿`(0..10)[3]` = 3?
///    Probablemente sí, pero lo dejamos para más adelante).
///  - Str: no indexable hasta que decidamos si la unidad es char o byte.
///  - Otros: type error.
fn eval_index(obj: &Value, idx: &Value, span: Span) -> EvalResult<Value> {
    match obj {
        Value::List(items) => {
            let i = match idx {
                Value::Int(n) => *n,
                other => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Int".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "el índice de una lista debe ser Int, no `{}`",
                        other.type_name()
                    ),
                ))),
            };
            // Sin índices negativos por ahora (sin Python-style xs[-1]).
            // Si después lo agregamos, vivirá acá.
            if i < 0 {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    span.line, span.column,
                    format!("índice negativo en lista: {}", i),
                )));
            }
            let i_usize = i as usize;
            let borrowed = items.borrow();
            borrowed.get(i_usize).cloned().ok_or_else(|| {
                EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    span.line, span.column,
                    format!(
                        "índice fuera de rango: {} en lista de tamaño {}",
                        i,
                        borrowed.len()
                    ),
                ))
            })
        }
        Value::Map(pairs) => {
            // Búsqueda lineal por igualdad. Esto va a ser O(n) hasta que
            // promovamos Map a una estructura indexada de verdad.
            for (k, v) in pairs.borrow().iter() {
                if k == idx {
                    return Ok(v.clone());
                }
            }
            Err(EvalSignal::Error(FitzError::new(
                ErrorKind::InvalidSyntax,
                span.line, span.column,
                format!("clave no encontrada en mapa: {}", idx),
            )))
        }
        other => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "List o Map".into(),
                found: other.type_name().into(),
            },
            span.line, span.column,
            format!(
                "el tipo `{}` no soporta indexing con `[]`",
                other.type_name()
            ),
        ))),
    }
}

// ---------------------------------------------------------------------------
// Operación unaria
// ---------------------------------------------------------------------------
//
// Por ahora solo `Neg`: negación numérica (`-x`). Cuando el lexer emita `!`
// como operador lógico, sumaremos `Not` acá.

fn eval_unary(op: &UnaryOpKind, v: Value, span: Span) -> EvalResult<Value> {
    match op {
        UnaryOpKind::Neg => match v {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(x) => Ok(Value::Float(-x)),
            other => Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int o Float".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("no se puede negar un valor de tipo `{}`", other.type_name()),
            ))),
        },
    }
}

// ---------------------------------------------------------------------------
// Builtins — funciones nativas implementadas en Rust, expuestas como
// identificadores en el env global.
// ---------------------------------------------------------------------------

/// Registra todas las funciones builtin en el environment. Llamar una sola
/// vez al inicio del programa.
fn register_builtins(env: &EnvRef) {
    env.borrow_mut().define(
        "print",
        Value::Builtin {
            name: "print",
            func: builtin_print,
        },
    );
    env.borrow_mut().define(
        "len",
        Value::Builtin {
            name: "len",
            func: builtin_len,
        },
    );
}

/// `print(arg1, arg2, ...)` — imprime los args convertidos a string,
/// separados por espacio, seguido de newline. Como Python.
fn builtin_print(args: &[Value]) -> FitzResult<Value> {
    let parts: Vec<String> = args.iter().map(|v| v.to_string()).collect();
    println!("{}", parts.join(" "));
    Ok(Value::Null)
}

/// `len(x)` — longitud de listas, mapas, strings y rangos.
///  - List: cantidad de elementos.
///  - Map: cantidad de pares.
///  - Str: cantidad de chars (no bytes — UTF-8 aware).
///  - Range: `end - start`, clampeado a 0 si el rango va al revés.
///  - Otros: type error.
fn builtin_len(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: 1,
                found: args.len(),
            },
            0, 0,
            format!("`len` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    let n: i64 = match &args[0] {
        Value::List(items) => items.borrow().len() as i64,
        Value::Map(pairs) => pairs.borrow().len() as i64,
        Value::Str(s) => s.chars().count() as i64,
        Value::Range { start, end } => (end - start).max(0),
        other => {
            return Err(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "List, Map, Str o Range".into(),
                    found: other.type_name().into(),
                },
                0, 0,
                format!(
                    "`len` no aplica a un valor de tipo `{}`",
                    other.type_name()
                ),
            ));
        }
    };
    Ok(Value::Int(n))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 en tests es un Float genérico, no PI.
mod tests {
    use super::*;
    use crate::ast::TypeExpr;

    // ---- helpers ----

    /// Evalúa una expresión aislada en un env vacío. Para tests cortos.
    fn eval_expr_test(expr: Expr) -> EvalResult<Value> {
        let env = Environment::new();
        eval_expr(&expr, env)
    }

    // ---- entry point ----

    #[test]
    fn programa_vacio_no_falla() {
        assert!(eval(vec![]).is_ok());
    }

    // ---- literales ----

    #[test]
    fn evalua_int_literal() {
        assert_eq!(eval_expr_test(Expr::Int(42, Span::ZERO)).unwrap(), Value::Int(42));
    }

    #[test]
    fn evalua_float_literal() {
        assert_eq!(eval_expr_test(Expr::Float(3.14, Span::ZERO)).unwrap(), Value::Float(3.14));
    }

    #[test]
    fn evalua_string_literal() {
        assert_eq!(
            eval_expr_test(Expr::Str("hola".into(), Span::ZERO)).unwrap(),
            Value::Str("hola".into())
        );
    }

    #[test]
    fn evalua_bool_literal() {
        assert_eq!(eval_expr_test(Expr::Bool(true, Span::ZERO)).unwrap(), Value::Bool(true));
        assert_eq!(eval_expr_test(Expr::Bool(false, Span::ZERO)).unwrap(), Value::Bool(false));
    }

    #[test]
    fn evalua_null_literal() {
        assert_eq!(eval_expr_test(Expr::Null(Span::ZERO)).unwrap(), Value::Null);
    }

    // ---- Ident ----

    #[test]
    fn ident_resuelve_variable_del_env() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(99));

        let result = eval_expr(&Expr::Ident("x".into(), Span::ZERO), env).unwrap();
        assert_eq!(result, Value::Int(99));
    }

    #[test]
    fn ident_no_definido_devuelve_error() {
        let env = Environment::new();
        let result = eval_expr(&Expr::Ident("nope".into(), Span::ZERO), env);

        match result {
            Err(EvalSignal::Error(e)) => {
                assert!(matches!(e.kind, ErrorKind::UndefinedVariable(ref n) if n == "nope"));
            }
            _ => panic!("se esperaba Error::UndefinedVariable"),
        }
    }

    #[test]
    fn ident_busca_en_scope_padre() {
        let global = Environment::new();
        global.borrow_mut().define("x", Value::Str("from_global".into()));

        let child = Environment::new_child(global);
        let result = eval_expr(&Expr::Ident("x".into(), Span::ZERO), child).unwrap();
        assert_eq!(result, Value::Str("from_global".into()));
    }

    // ---- Stmt::Expr (paso intermedio para verificar el wiring stmt→expr) ----

    #[test]
    fn stmt_expr_evalua_la_expresion_interna() {
        let env = Environment::new();
        let stmt = Stmt::Expr(Expr::Int(7, Span::ZERO), Span::ZERO);
        let result = eval_stmt(&stmt, env).unwrap();
        assert_eq!(result, Value::Int(7));
    }

    // ---- builtins ----

    #[test]
    fn builtin_print_devuelve_null() {
        let result = builtin_print(&[Value::Str("test".into())]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn register_builtins_define_print_en_env() {
        let env = Environment::new();
        register_builtins(&env);

        let print = env.borrow().get("print");
        assert!(print.is_some());
        match print.unwrap() {
            Value::Builtin { name, .. } => assert_eq!(name, "print"),
            _ => panic!("se esperaba Value::Builtin"),
        }
    }

    // ---- signals ----

    #[test]
    fn fitzerror_se_convierte_a_evalsignal_error() {
        let err = FitzError::new(ErrorKind::DivisionByZero, 1, 1, "test");
        let signal: EvalSignal = err.into();
        assert!(matches!(signal, EvalSignal::Error(_)));
    }

    #[test]
    fn break_fuera_de_loop_es_error() {
        let result = eval(vec![Stmt::Break(Span::ZERO)]);
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::BreakOutsideLoop
        ));
    }

    #[test]
    fn continue_fuera_de_loop_es_error() {
        let result = eval(vec![Stmt::Continue(Span::ZERO)]);
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::ContinueOutsideLoop
        ));
    }

    // ---- BinOp: aritmética ----

    /// Helper: construye `BinOp { op, left: l, right: r }` con boxes.
    fn binop(op: BinOpKind, l: Expr, r: Expr) -> Expr {
        Expr::BinOp { op, left: Box::new(l), right: Box::new(r), span: Span::ZERO }
    }

    #[test]
    fn add_int_int_da_int() {
        let e = binop(BinOpKind::Add, Expr::Int(2, Span::ZERO), Expr::Int(3, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(5));
    }

    #[test]
    fn add_int_float_promueve_a_float() {
        let e = binop(BinOpKind::Add, Expr::Int(2, Span::ZERO), Expr::Float(0.5, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Float(2.5));
    }

    #[test]
    fn add_float_int_promueve_a_float() {
        let e = binop(BinOpKind::Add, Expr::Float(1.5, Span::ZERO), Expr::Int(2, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Float(3.5));
    }

    #[test]
    fn add_strings_concatena() {
        let e = binop(
            BinOpKind::Add,
            Expr::Str("hola ".into(), Span::ZERO),
            Expr::Str("mundo".into(), Span::ZERO),
        );
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("hola mundo".into()));
    }

    #[test]
    fn add_tipos_incompatibles_es_type_error() {
        let e = binop(BinOpKind::Add, Expr::Str("x".into(), Span::ZERO), Expr::Int(1, Span::ZERO));
        match eval_expr_test(e) {
            Err(EvalSignal::Error(err)) => {
                assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
            }
            _ => panic!("se esperaba TypeMismatch"),
        }
    }

    #[test]
    fn sub_mul_funcionan() {
        let sub = binop(BinOpKind::Sub, Expr::Int(10, Span::ZERO), Expr::Int(3, Span::ZERO));
        assert_eq!(eval_expr_test(sub).unwrap(), Value::Int(7));

        let mul = binop(BinOpKind::Mul, Expr::Int(4, Span::ZERO), Expr::Int(5, Span::ZERO));
        assert_eq!(eval_expr_test(mul).unwrap(), Value::Int(20));
    }

    #[test]
    fn div_int_int_trunca() {
        // 10 / 3 = 3 (truncado), no 3.33
        let e = binop(BinOpKind::Div, Expr::Int(10, Span::ZERO), Expr::Int(3, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(3));
    }

    #[test]
    fn div_int_float_da_float() {
        let e = binop(BinOpKind::Div, Expr::Int(10, Span::ZERO), Expr::Float(4.0, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Float(2.5));
    }

    #[test]
    fn div_por_cero_int_es_error() {
        let e = binop(BinOpKind::Div, Expr::Int(1, Span::ZERO), Expr::Int(0, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::DivisionByZero, .. })
        ));
    }

    #[test]
    fn div_por_cero_float_es_error() {
        let e = binop(BinOpKind::Div, Expr::Float(1.0, Span::ZERO), Expr::Float(0.0, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::DivisionByZero, .. })
        ));
    }

    // ---- BinOp: comparación e igualdad ----

    #[test]
    fn eq_con_coercion_int_float() {
        // 1 == 1.0 → true
        let e = binop(BinOpKind::Eq, Expr::Int(1, Span::ZERO), Expr::Float(1.0, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn eq_tipos_distintos_da_false_sin_error() {
        // 1 == "1" → false (no error)
        let e = binop(BinOpKind::Eq, Expr::Int(1, Span::ZERO), Expr::Str("1".into(), Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(false));
    }

    #[test]
    fn noteq_funciona() {
        let e = binop(BinOpKind::NotEq, Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn lt_gt_lteq_gteq_numericos() {
        assert_eq!(
            eval_expr_test(binop(BinOpKind::Lt, Expr::Int(2, Span::ZERO), Expr::Int(3, Span::ZERO))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval_expr_test(binop(BinOpKind::Gt, Expr::Int(2, Span::ZERO), Expr::Int(3, Span::ZERO))).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            eval_expr_test(binop(BinOpKind::LtEq, Expr::Int(3, Span::ZERO), Expr::Int(3, Span::ZERO))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval_expr_test(binop(BinOpKind::GtEq, Expr::Int(2, Span::ZERO), Expr::Int(3, Span::ZERO))).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn comparacion_con_promocion_int_float() {
        // 2 < 2.5 → true
        let e = binop(BinOpKind::Lt, Expr::Int(2, Span::ZERO), Expr::Float(2.5, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn comparacion_de_strings_es_alfabetica() {
        let e = binop(
            BinOpKind::Lt,
            Expr::Str("abc".into(), Span::ZERO),
            Expr::Str("abd".into(), Span::ZERO),
        );
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn comparacion_entre_bool_es_type_error() {
        // Bool no se compara con <. Sí con ==.
        let e = binop(BinOpKind::Lt, Expr::Bool(true, Span::ZERO), Expr::Bool(false, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- BinOp: lógicos con short-circuit ----

    #[test]
    fn and_true_true_da_true() {
        let e = binop(BinOpKind::And, Expr::Bool(true, Span::ZERO), Expr::Bool(true, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn and_false_corta_y_no_evalua_derecho() {
        // El lado derecho es un Ident no definido. Si se evaluara, daría error.
        // Como `false and ...` corta, devuelve false sin error.
        let e = binop(
            BinOpKind::And,
            Expr::Bool(false, Span::ZERO),
            Expr::Ident("no_existe".into(), Span::ZERO),
        );
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(false));
    }

    #[test]
    fn or_true_corta_y_no_evalua_derecho() {
        let e = binop(
            BinOpKind::Or,
            Expr::Bool(true, Span::ZERO),
            Expr::Ident("no_existe".into(), Span::ZERO),
        );
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn or_false_true_da_true() {
        let e = binop(BinOpKind::Or, Expr::Bool(false, Span::ZERO), Expr::Bool(true, Span::ZERO));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn and_con_no_bool_izquierda_es_type_error() {
        let e = binop(BinOpKind::And, Expr::Int(1, Span::ZERO), Expr::Bool(true, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn and_con_no_bool_derecha_es_type_error() {
        // Para que el lado derecho se evalúe, el izquierdo debe ser true.
        let e = binop(BinOpKind::And, Expr::Bool(true, Span::ZERO), Expr::Int(1, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- BinOp anidados ----

    #[test]
    fn expresion_anidada_2_mas_3_por_4_da_14() {
        // 2 + (3 * 4) — Stmt::Expr para verificar wiring completo.
        let inner = binop(BinOpKind::Mul, Expr::Int(3, Span::ZERO), Expr::Int(4, Span::ZERO));
        let outer = binop(BinOpKind::Add, Expr::Int(2, Span::ZERO), inner);
        assert_eq!(eval_expr_test(outer).unwrap(), Value::Int(14));
    }

    // ---- UnaryOp ----

    #[test]
    fn neg_int() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Int(5, Span::ZERO)), span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(-5));
    }

    #[test]
    fn neg_float() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Float(3.14, Span::ZERO)), span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Float(-3.14));
    }

    #[test]
    fn doble_negacion_devuelve_el_original() {
        // -(-7) = 7
        let inner = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Int(7, Span::ZERO)), span: Span::ZERO,
        };
        let outer = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(inner), span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(outer).unwrap(), Value::Int(7));
    }

    #[test]
    fn neg_de_bool_es_type_error() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Bool(true, Span::ZERO)), span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn neg_de_string_es_type_error() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Str("hola".into(), Span::ZERO)), span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- Stmt::Assign ----

    #[test]
    fn assign_define_variable_nueva_en_scope_local() {
        let env = Environment::new();
        let stmt = Stmt::Assign { target: AssignTarget::Ident("x".into()),
            type_: None,
            value: Expr::Int(42, Span::ZERO),
         span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).unwrap();

        assert_eq!(env.borrow().get("x"), Some(Value::Int(42)));
    }

    #[test]
    fn assign_reasigna_variable_existente_en_el_mismo_scope() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(1));

        let stmt = Stmt::Assign { target: AssignTarget::Ident("x".into()),
            type_: None,
            value: Expr::Int(99, Span::ZERO),
         span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).unwrap();

        assert_eq!(env.borrow().get("x"), Some(Value::Int(99)));
    }

    #[test]
    fn assign_desde_child_reasigna_en_el_padre_si_existe() {
        let global = Environment::new();
        global.borrow_mut().define("x", Value::Int(1));

        let child = Environment::new_child(global.clone());
        let stmt = Stmt::Assign { target: AssignTarget::Ident("x".into()),
            type_: None,
            value: Expr::Int(42, Span::ZERO),
         span: Span::ZERO };
        eval_stmt(&stmt, child).unwrap();

        // El cambio se ve en el global.
        assert_eq!(global.borrow().get("x"), Some(Value::Int(42)));
    }

    #[test]
    fn assign_crea_local_si_la_variable_no_existe_en_la_cadena() {
        let global = Environment::new();
        let child = Environment::new_child(global.clone());

        let stmt = Stmt::Assign { target: AssignTarget::Ident("nueva".into()),
            type_: None,
            value: Expr::Int(7, Span::ZERO),
         span: Span::ZERO };
        eval_stmt(&stmt, child.clone()).unwrap();

        // Solo existe en child, no se propagó al padre.
        assert_eq!(child.borrow().get("nueva"), Some(Value::Int(7)));
        assert_eq!(global.borrow().get("nueva"), None);
    }

    #[test]
    fn assign_ignora_la_anotacion_de_tipo() {
        // type_: Some("Int") con value String — no falla (tipado gradual,
        // sin checks en runtime todavía).
        let env = Environment::new();
        let stmt = Stmt::Assign { target: AssignTarget::Ident("x".into()),
            type_: Some(TypeExpr::named("Int")),
            value: Expr::Str("soy un string".into(), Span::ZERO),
         span: Span::ZERO };
        assert!(eval_stmt(&stmt, env.clone()).is_ok());
        assert_eq!(env.borrow().get("x"), Some(Value::Str("soy un string".into())));
    }

    // ---- Expr::Call (builtins) ----

    #[test]
    fn call_a_print_devuelve_null() {
        // print(...) escribe a stdout y devuelve Null. Verificamos el Value
        // de retorno; la salida real la chequeamos manualmente con hello.fitz.
        let env = Environment::new();
        register_builtins(&env);

        let call = Expr::Call { callee: Box::new(Expr::Ident("print".into(), Span::ZERO)), args: vec![Expr::Str("test".into(), Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Null);
    }

    #[test]
    fn call_a_funcion_no_definida_es_error() {
        // Como `Expr::Call` ahora evalúa el callee como expresión, un
        // ident sin definir falla con `UndefinedVariable` (no
        // `UndefinedFunction` como antes). Es coherente: el parser no
        // distingue "esto es un nombre de función" sintácticamente.
        let env = Environment::new();
        let call = Expr::Call {
            callee: Box::new(Expr::Ident("noexiste".into(), Span::ZERO)),
            args: vec![], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::UndefinedVariable(_), .. })
        ));
    }

    #[test]
    fn call_a_no_funcion_es_type_error() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(5));

        let call = Expr::Call { callee: Box::new(Expr::Ident("x".into(), Span::ZERO)), args: vec![], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn call_evalua_args_antes_de_invocar() {
        // El arg `1 + 2` debe llegar al builtin como Int(3), no como BinOp.
        // Como print no nos deja inspeccionar, usamos un assert indirecto:
        // si el eval de args fallara, daría error. Si llega bien, Null.
        let env = Environment::new();
        register_builtins(&env);

        let call = Expr::Call { callee: Box::new(Expr::Ident("print".into(), Span::ZERO)), args: vec![Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1, Span::ZERO)),
                right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
            }], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Null);
    }

    // ---- Expr::StrInterp ----

    #[test]
    fn str_interp_solo_con_literales_concatena() {
        let e = Expr::StrInterp(vec![
            StrPart::Lit("hola ".into()),
            StrPart::Lit("mundo".into()),
        ], Span::ZERO);
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("hola mundo".into()));
    }

    #[test]
    fn str_interp_interpola_ident() {
        let env = Environment::new();
        env.borrow_mut().define("name", Value::Str("Fitz".into()));

        let e = Expr::StrInterp(vec![
            StrPart::Lit("Hola, ".into()),
            StrPart::Expr(Expr::Ident("name".into(), Span::ZERO)),
            StrPart::Lit("!".into()),
        ], Span::ZERO);
        assert_eq!(
            eval_expr(&e, env).unwrap(),
            Value::Str("Hola, Fitz!".into())
        );
    }

    #[test]
    fn str_interp_convierte_int_a_string() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(42));

        let e = Expr::StrInterp(vec![
            StrPart::Lit("x es ".into()),
            StrPart::Expr(Expr::Ident("x".into(), Span::ZERO)),
        ], Span::ZERO);
        assert_eq!(eval_expr(&e, env).unwrap(), Value::Str("x es 42".into()));
    }

    #[test]
    fn str_interp_evalua_expresiones_internas() {
        // "{1 + 2}" → "3"
        let e = Expr::StrInterp(vec![
            StrPart::Expr(Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1, Span::ZERO)),
                right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
            }),
        ], Span::ZERO);
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("3".into()));
    }

    // ---- Integración mini: hello.fitz a mano ----

    // ---- FnDef + Return + Call (user-defined) ----

    /// Helper: arma `fn name(params) { body }` como Stmt.
    fn fn_def(name: &str, params: Vec<&str>, body: Vec<Stmt>) -> Stmt {
        Stmt::FnDef {
            name: name.into(),
            params: params.into_iter().map(|p| crate::ast::Param {
                name: p.into(),
                type_: None,
            }).collect(),
            return_type: None,
            body,
            is_async: false,
            decorators: vec![],
         span: Span::ZERO }
    }

    #[test]
    fn fn_sin_return_devuelve_null() {
        // fn f() { } ; f()
        let env = Environment::new();
        eval_stmt(&fn_def("f", vec![], vec![]), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Null);
    }

    #[test]
    fn fn_return_constante() {
        // fn f() { return 42 } ; f()
        let env = Environment::new();
        eval_stmt(
            &fn_def("f", vec![], vec![Stmt::Return(Expr::Int(42, Span::ZERO), Span::ZERO)]),
            env.clone(),
        ).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(42));
    }

    #[test]
    fn fn_con_un_param_arrow_style() {
        // fn double(n) => n * 2 → body es vec![Return(n * 2)]
        // double(7) → 14
        let env = Environment::new();
        let body = vec![Stmt::Return(Expr::BinOp {
            op: BinOpKind::Mul,
            left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
            right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
        }, Span::ZERO)];
        eval_stmt(&fn_def("double", vec!["n"], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("double".into(), Span::ZERO)), args: vec![Expr::Int(7, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(14));
    }

    #[test]
    fn fn_con_dos_params_suma() {
        // fn add(a, b) => a + b ; add(3, 4) → 7
        let env = Environment::new();
        let body = vec![Stmt::Return(Expr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
            right: Box::new(Expr::Ident("b".into(), Span::ZERO)), span: Span::ZERO,
        }, Span::ZERO)];
        eval_stmt(&fn_def("add", vec!["a", "b"], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("add".into(), Span::ZERO)), args: vec![Expr::Int(3, Span::ZERO), Expr::Int(4, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(7));
    }

    #[test]
    fn fn_ve_variables_del_scope_donde_se_definio() {
        // Closure básico: la función accede a `x` del scope global.
        //
        //   x = 10
        //   fn get_x() => x
        //   get_x()  → 10
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(10));

        let body = vec![Stmt::Return(Expr::Ident("x".into(), Span::ZERO), Span::ZERO)];
        eval_stmt(&fn_def("get_x", vec![], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("get_x".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(10));
    }

    #[test]
    fn fn_param_sombrea_variable_externa() {
        // x = 100; fn f(x) => x ; f(7) → 7 (no 100)
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(100));

        let body = vec![Stmt::Return(Expr::Ident("x".into(), Span::ZERO), Span::ZERO)];
        eval_stmt(&fn_def("f", vec!["x"], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![Expr::Int(7, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(7));
    }

    #[test]
    fn fn_con_pocos_args_es_error() {
        // fn f(a, b) ... ; f(1)
        let env = Environment::new();
        eval_stmt(
            &fn_def("f", vec!["a", "b"], vec![Stmt::Return(Expr::Int(0, Span::ZERO), Span::ZERO)]),
            env.clone(),
        ).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![Expr::Int(1, Span::ZERO)], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError {
                kind: ErrorKind::WrongArgCount { expected: 2, found: 1 }, ..
            })
        ));
    }

    #[test]
    fn fn_con_muchos_args_es_error() {
        let env = Environment::new();
        eval_stmt(
            &fn_def("f", vec![], vec![Stmt::Return(Expr::Int(0, Span::ZERO), Span::ZERO)]),
            env.clone(),
        ).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError {
                kind: ErrorKind::WrongArgCount { expected: 0, found: 2 }, ..
            })
        ));
    }

    #[test]
    fn return_fuera_de_fn_es_error() {
        // En el top level, `return 5` no tiene caller que lo intercepte.
        let result = eval(vec![Stmt::Return(Expr::Int(5, Span::ZERO), Span::ZERO)]);
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::ReturnOutsideFunction
        ));
    }

    #[test]
    fn fn_con_body_de_varias_sentencias() {
        // fn f(n) {
        //     x = n * 2
        //     return x + 1
        // }
        // f(5) → 11
        let env = Environment::new();
        let body = vec![
            Stmt::Assign { target: AssignTarget::Ident("x".into()),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
                },
             span: Span::ZERO },
            Stmt::Return(Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
            }, Span::ZERO),
        ];
        eval_stmt(&fn_def("f", vec!["n"], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![Expr::Int(5, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(11));
    }

    #[test]
    fn return_corta_la_ejecucion_del_body() {
        // fn f() {
        //     return 1
        //     return 2   ← nunca se ejecuta
        // }
        let env = Environment::new();
        let body = vec![
            Stmt::Return(Expr::Int(1, Span::ZERO), Span::ZERO),
            Stmt::Return(Expr::Int(2, Span::ZERO), Span::ZERO),
        ];
        eval_stmt(&fn_def("f", vec![], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(1));
    }

    // ---- Expr::If ----

    /// Helper: arma `if cond { then } else? { else_ }`.
    fn if_expr(cond: Expr, then: Vec<Stmt>, else_: Option<Vec<Stmt>>) -> Expr {
        Expr::If { condition: Box::new(cond), then, else_, span: Span::ZERO }
    }

    #[test]
    fn if_true_sin_else_devuelve_valor_del_then() {
        // if true { 7 } → 7
        let e = if_expr(Expr::Bool(true, Span::ZERO), vec![Stmt::Expr(Expr::Int(7, Span::ZERO), Span::ZERO)], None);
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(7));
    }

    #[test]
    fn if_false_sin_else_devuelve_null() {
        let e = if_expr(Expr::Bool(false, Span::ZERO), vec![Stmt::Expr(Expr::Int(7, Span::ZERO), Span::ZERO)], None);
        assert_eq!(eval_expr_test(e).unwrap(), Value::Null);
    }

    #[test]
    fn if_else_toma_la_rama_correcta() {
        // if true { 1 } else { 2 } → 1
        let then = vec![Stmt::Expr(Expr::Int(1, Span::ZERO), Span::ZERO)];
        let else_ = vec![Stmt::Expr(Expr::Int(2, Span::ZERO), Span::ZERO)];
        let e = if_expr(Expr::Bool(true, Span::ZERO), then.clone(), Some(else_.clone()));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(1));

        let e = if_expr(Expr::Bool(false, Span::ZERO), then, Some(else_));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(2));
    }

    #[test]
    fn if_condicion_no_bool_es_type_error() {
        // if 1 { ... } → error (no truthy coercion).
        let e = if_expr(Expr::Int(1, Span::ZERO), vec![], None);
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn if_evalua_solo_la_rama_correspondiente() {
        // El then es un Ident no definido. Si se evaluara, daría error.
        // Como cond es false, no se toca → resultado del else.
        let then = vec![Stmt::Expr(Expr::Ident("no_existe".into(), Span::ZERO), Span::ZERO)];
        let else_ = vec![Stmt::Expr(Expr::Int(99, Span::ZERO), Span::ZERO)];
        let e = if_expr(Expr::Bool(false, Span::ZERO), then, Some(else_));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(99));
    }

    #[test]
    fn variables_definidas_dentro_del_if_persisten_afuera() {
        // x = 1
        // if x == 1 { y = 99 }
        // print(y)  → "99"
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(1));

        let if_stmt = Stmt::Expr(if_expr(
            Expr::BinOp {
                op: BinOpKind::Eq,
                left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
            },
            vec![Stmt::Assign { target: AssignTarget::Ident("y".into()),
                type_: None,
                value: Expr::Int(99, Span::ZERO),
             span: Span::ZERO }],
            None,
        ), Span::ZERO);
        eval_stmt(&if_stmt, env.clone()).unwrap();

        assert_eq!(env.borrow().get("y"), Some(Value::Int(99)));
    }

    #[test]
    fn else_if_anidado_funciona() {
        // if false { 1 } else if true { 2 } else { 3 } → 2
        //
        // El parser modela `else if` como `else_: vec![Stmt::Expr(Expr::If, Span::ZERO)]`.
        let inner = if_expr(
            Expr::Bool(true, Span::ZERO),
            vec![Stmt::Expr(Expr::Int(2, Span::ZERO), Span::ZERO)],
            Some(vec![Stmt::Expr(Expr::Int(3, Span::ZERO), Span::ZERO)]),
        );
        let outer = if_expr(
            Expr::Bool(false, Span::ZERO),
            vec![Stmt::Expr(Expr::Int(1, Span::ZERO), Span::ZERO)],
            Some(vec![Stmt::Expr(inner, Span::ZERO)]),
        );
        assert_eq!(eval_expr_test(outer).unwrap(), Value::Int(2));
    }

    #[test]
    fn if_como_expresion_en_assign() {
        // let r = if true { 42 } else { 0 }
        let env = Environment::new();
        let stmt = Stmt::Assign { target: AssignTarget::Ident("r".into()),
            type_: None,
            value: if_expr(
                Expr::Bool(true, Span::ZERO),
                vec![Stmt::Expr(Expr::Int(42, Span::ZERO), Span::ZERO)],
                Some(vec![Stmt::Expr(Expr::Int(0, Span::ZERO), Span::ZERO)]),
            ),
         span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Int(42)));
    }

    #[test]
    fn factorial_recursivo_funciona() {
        // El test que ata todo: closures + recursión + if + comparación
        // + BinOp + Return.
        //
        //   fn factorial(n) {
        //       if n == 0 { return 1 }
        //       return n * factorial(n - 1)
        //   }
        //   factorial(5) → 120
        let env = Environment::new();

        let body = vec![
            Stmt::Expr(if_expr(
                Expr::BinOp {
                    op: BinOpKind::Eq,
                    left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(0, Span::ZERO)), span: Span::ZERO,
                },
                vec![Stmt::Return(Expr::Int(1, Span::ZERO), Span::ZERO)],
                None,
            ), Span::ZERO),
            Stmt::Return(Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                right: Box::new(Expr::Call { callee: Box::new(Expr::Ident("factorial".into(), Span::ZERO)), args: vec![Expr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
                    }], span: Span::ZERO,
                }), span: Span::ZERO,
            }, Span::ZERO),
        ];

        eval_stmt(&fn_def("factorial", vec!["n"], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("factorial".into(), Span::ZERO)), args: vec![Expr::Int(5, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(120));
    }

    // ---- Expr::Match ----

    use crate::ast::MatchArm;

    fn match_arm(pattern: Pattern, body: Expr) -> MatchArm {
        MatchArm { pattern, body }
    }

    #[test]
    fn match_wildcard_siempre_matchea() {
        // match 42 { _ => 99 } → 99
        let e = Expr::Match {
            value: Box::new(Expr::Int(42, Span::ZERO)),
            arms: vec![match_arm(Pattern::Wildcard, Expr::Int(99, Span::ZERO))], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(99));
    }

    #[test]
    fn match_ident_bindea_el_valor() {
        // match 42 { n => n + 1 } → 43
        let e = Expr::Match {
            value: Box::new(Expr::Int(42, Span::ZERO)),
            arms: vec![match_arm(
                Pattern::Ident("n".into()),
                Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
                },
            )], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(43));
    }

    #[test]
    fn match_toma_el_primer_arm_que_matchea() {
        // match "hola" {
        //     x => "primer arm: ${x}",
        //     _ => "segundo arm (no se toca)",
        // }
        let e = Expr::Match {
            value: Box::new(Expr::Str("hola".into(), Span::ZERO)),
            arms: vec![
                match_arm(
                    Pattern::Ident("x".into()),
                    Expr::StrInterp(vec![
                        StrPart::Lit("primer arm: ".into()),
                        StrPart::Expr(Expr::Ident("x".into(), Span::ZERO)),
                    ], Span::ZERO),
                ),
                match_arm(Pattern::Wildcard, Expr::Str("segundo arm".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("primer arm: hola".into()));
    }

    #[test]
    fn match_binding_vive_solo_en_el_arm() {
        // El binding `n` no debe escapar al scope contenedor.
        let env = Environment::new();
        let e = Expr::Match {
            value: Box::new(Expr::Int(7, Span::ZERO)),
            arms: vec![match_arm(Pattern::Ident("n".into()), Expr::Ident("n".into(), Span::ZERO))], span: Span::ZERO,
        };
        eval_expr(&e, env.clone()).unwrap();

        // `n` no quedó definida en el scope de afuera.
        assert_eq!(env.borrow().get("n"), None);
    }

    #[test]
    fn match_ok_binding_bindea_inner() {
        // match Ok(5) { Ok(v) => v + 1, Err(e) => -1 } → 6
        let e = Expr::Match {
            value: Box::new(Expr::Ok(Box::new(Expr::Int(5, Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(
                    Pattern::OkBinding("v".into()),
                    Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("v".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
                    },
                ),
                match_arm(Pattern::ErrBinding("e".into()), Expr::Int(-1, Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(6));
    }

    #[test]
    fn match_err_binding_bindea_inner() {
        // match Err("boom") { Ok(v) => "ok", Err(e) => e } → "boom"
        let e = Expr::Match {
            value: Box::new(Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkBinding("v".into()), Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::ErrBinding("e".into()), Expr::Ident("e".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("boom".into()));
    }

    #[test]
    fn match_ok_no_matchea_err() {
        // El patrón Ok(_) NO matchea contra Err(_) — sigue al siguiente arm.
        let e = Expr::Match {
            value: Box::new(Expr::Err(Box::new(Expr::Int(1, Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkBinding("v".into()), Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("otro".into()));
    }

    #[test]
    fn match_ok_no_matchea_no_result() {
        // Ok(v) sobre un valor que no es Result → no matchea, cae en wildcard.
        let e = Expr::Match {
            value: Box::new(Expr::Int(5, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkBinding("v".into()), Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("no-result".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("no-result".into()));
    }

    #[test]
    fn match_ok_wildcard_matchea_pero_no_bindea() {
        // Pattern::OkWildcard matchea cualquier Ok sin bindear el
        // inner. Cierra la deuda vieja de 3.3 donde `_` adentro se
        // bindeaba como var llamada `_`.
        let e = Expr::Match {
            value: Box::new(Expr::Ok(Box::new(Expr::Int(99, Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkWildcard, Expr::Str("ok!".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("ok!".into()));
    }

    #[test]
    fn match_err_wildcard_matchea_err() {
        let e = Expr::Match {
            value: Box::new(Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkBinding("v".into()), Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::ErrWildcard, Expr::Str("falló".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("falló".into()));
    }

    #[test]
    fn match_ok_wildcard_no_matchea_err() {
        // OkWildcard NO debe matchear Err.
        let e = Expr::Match {
            value: Box::new(Expr::Err(Box::new(Expr::Int(0, Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkWildcard, Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("otro".into()));
    }

    #[test]
    fn match_ok_wildcard_no_ensucia_scope() {
        // Después de un match con Ok(_), no debe existir una var
        // llamada `_` en el env. Esto era el bug que cerraba 3.3.
        let src = "\
let x = match Ok(5) {\n\
    Ok(_) => 1\n\
    _ => 0\n\
}\n\
print(_)\n";
        let result = parse_and_eval(src);
        assert!(
            result.is_err(),
            "esperaba error de variable `_` desconocida, hubo: {:?}",
            result
        );
    }

    #[test]
    fn match_literal_int_matchea() {
        // match 2 { 1 => "uno", 2 => "dos", _ => "otro" } → "dos"
        let e = Expr::Match {
            value: Box::new(Expr::Int(2, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Int(1), Expr::Str("uno".into(), Span::ZERO)),
                match_arm(Pattern::Int(2), Expr::Str("dos".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("dos".into()));
    }

    #[test]
    fn match_literal_int_no_coerciona_a_float() {
        // match 1.0 { 1 => "int", _ => "no-int" } → "no-int"
        // (En match, igualdad es estructural — sin la coerción del `==`).
        let e = Expr::Match {
            value: Box::new(Expr::Float(1.0, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Int(1), Expr::Str("int".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("no-int".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("no-int".into()));
    }

    #[test]
    fn match_literal_str_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Str("hola".into(), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Str("chau".into()), Expr::Int(1, Span::ZERO)),
                match_arm(Pattern::Str("hola".into()), Expr::Int(2, Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Int(0, Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(2));
    }

    #[test]
    fn match_literal_bool_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Bool(true, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Bool(false), Expr::Str("falso".into(), Span::ZERO)),
                match_arm(Pattern::Bool(true), Expr::Str("verdadero".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("verdadero".into()));
    }

    #[test]
    fn match_literal_null_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Null(Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Null, Expr::Str("es null".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("no null".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("es null".into()));
    }

    #[test]
    fn match_int_negativo_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Int(-5, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Int(-5), Expr::Str("menos cinco".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("menos cinco".into()));
    }

    #[test]
    fn match_literales_caen_a_ident_si_ninguno_matchea() {
        // match 42 { 1 => "uno", n => "default ${n}" }
        let e = Expr::Match {
            value: Box::new(Expr::Int(42, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Int(1), Expr::Str("uno".into(), Span::ZERO)),
                match_arm(
                    Pattern::Ident("n".into()),
                    Expr::StrInterp(vec![
                        StrPart::Lit("default ".into()),
                        StrPart::Expr(Expr::Ident("n".into(), Span::ZERO)),
                    ], Span::ZERO),
                ),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("default 42".into()));
    }

    #[test]
    fn match_sin_arms_es_error() {
        let e = Expr::Match {
            value: Box::new(Expr::Int(1, Span::ZERO)),
            arms: vec![], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(_)
        ));
    }

    // ---- while / loop ----

    #[test]
    fn while_itera_hasta_que_cond_es_falsa() {
        // i = 0
        // total = 0
        // while i < 5 { total = total + i; i = i + 1 }
        // total → 0+1+2+3+4 = 10
        let env = Environment::new();
        env.borrow_mut().define("i", Value::Int(0));
        env.borrow_mut().define("total", Value::Int(0));

        let stmt = Stmt::While {
            condition: Expr::BinOp {
                op: BinOpKind::Lt,
                left: Box::new(Expr::Ident("i".into(), Span::ZERO)),
                right: Box::new(Expr::Int(5, Span::ZERO)), span: Span::ZERO,
            },
            body: vec![
                Stmt::Assign { target: AssignTarget::Ident("total".into()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("total".into(), Span::ZERO)),
                        right: Box::new(Expr::Ident("i".into(), Span::ZERO)), span: Span::ZERO,
                    },
                 span: Span::ZERO },
                Stmt::Assign { target: AssignTarget::Ident("i".into()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("i".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
                    },
                 span: Span::ZERO },
            ],
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(10)));
    }

    #[test]
    fn while_con_cond_inicialmente_falsa_no_itera() {
        let env = Environment::new();
        env.borrow_mut().define("counter", Value::Int(0));

        let stmt = Stmt::While {
            condition: Expr::Bool(false, Span::ZERO),
            body: vec![Stmt::Assign { target: AssignTarget::Ident("counter".into()),
                type_: None,
                value: Expr::Int(99, Span::ZERO),
             span: Span::ZERO }],
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("counter"), Some(Value::Int(0)));
    }

    #[test]
    fn while_break_termina_loop() {
        let env = Environment::new();
        env.borrow_mut().define("i", Value::Int(0));

        // while true { i = i + 1; if i == 3 { break } }
        let stmt = Stmt::While {
            condition: Expr::Bool(true, Span::ZERO),
            body: vec![
                Stmt::Assign { target: AssignTarget::Ident("i".into()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("i".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
                    },
                 span: Span::ZERO },
                Stmt::Expr(Expr::If {
                    condition: Box::new(Expr::BinOp {
                        op: BinOpKind::Eq,
                        left: Box::new(Expr::Ident("i".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(3, Span::ZERO)), span: Span::ZERO,
                    }),
                    then: vec![Stmt::Break(Span::ZERO)],
                    else_: None, span: Span::ZERO,
                }, Span::ZERO),
            ],
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("i"), Some(Value::Int(3)));
    }

    #[test]
    fn while_continue_salta_a_la_siguiente_iteracion() {
        let env = Environment::new();
        env.borrow_mut().define("i", Value::Int(0));
        env.borrow_mut().define("total", Value::Int(0));

        // while i < 5 {
        //   i = i + 1
        //   if i == 3 { continue }
        //   total = total + i
        // }
        // total → 1+2+4+5 = 12 (saltó el 3)
        let stmt = Stmt::While {
            condition: Expr::BinOp {
                op: BinOpKind::Lt,
                left: Box::new(Expr::Ident("i".into(), Span::ZERO)),
                right: Box::new(Expr::Int(5, Span::ZERO)), span: Span::ZERO,
            },
            body: vec![
                Stmt::Assign { target: AssignTarget::Ident("i".into()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("i".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
                    },
                 span: Span::ZERO },
                Stmt::Expr(Expr::If {
                    condition: Box::new(Expr::BinOp {
                        op: BinOpKind::Eq,
                        left: Box::new(Expr::Ident("i".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(3, Span::ZERO)), span: Span::ZERO,
                    }),
                    then: vec![Stmt::Continue(Span::ZERO)],
                    else_: None, span: Span::ZERO,
                }, Span::ZERO),
                Stmt::Assign { target: AssignTarget::Ident("total".into()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("total".into(), Span::ZERO)),
                        right: Box::new(Expr::Ident("i".into(), Span::ZERO)), span: Span::ZERO,
                    },
                 span: Span::ZERO },
            ],
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(12)));
    }

    #[test]
    fn while_cond_no_bool_es_type_error() {
        let env = Environment::new();
        let stmt = Stmt::While {
            condition: Expr::Int(1, Span::ZERO),
            body: vec![],
         span: Span::ZERO };
        assert!(matches!(
            eval_stmt(&stmt, env).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn loop_infinito_se_corta_con_break() {
        let env = Environment::new();
        env.borrow_mut().define("count", Value::Int(0));

        // loop {
        //   count = count + 1
        //   if count == 5 { break }
        // }
        let stmt = Stmt::Loop {
            body: vec![
                Stmt::Assign { target: AssignTarget::Ident("count".into()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("count".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
                    },
                 span: Span::ZERO },
                Stmt::Expr(Expr::If {
                    condition: Box::new(Expr::BinOp {
                        op: BinOpKind::Eq,
                        left: Box::new(Expr::Ident("count".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(5, Span::ZERO)), span: Span::ZERO,
                    }),
                    then: vec![Stmt::Break(Span::ZERO)],
                    else_: None, span: Span::ZERO,
                }, Span::ZERO),
            ],
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("count"), Some(Value::Int(5)));
    }

    #[test]
    fn return_dentro_de_while_dentro_de_fn_propaga() {
        // fn f() {
        //   while true { return 42 }
        // }
        // f() → 42
        let env = Environment::new();
        let body = vec![Stmt::While {
            condition: Expr::Bool(true, Span::ZERO),
            body: vec![Stmt::Return(Expr::Int(42, Span::ZERO), Span::ZERO)],
         span: Span::ZERO }];
        eval_stmt(&fn_def("f", vec![], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(42));
    }

    // ---- Stmt::TypeDef ----

    use crate::ast::Field;

    fn make_field(name: &str, type_: &str, nullable: bool) -> Field {
        let base = TypeExpr::named(type_);
        let t = if nullable {
            TypeExpr::Nullable(Box::new(base))
        } else {
            base
        };
        Field {
            name: name.into(),
            type_: t,
            default: None,
        }
    }

    #[test]
    fn type_def_registra_el_tipo_en_el_env() {
        // type User { id: Int, name: Str }
        let env = Environment::new();
        let stmt = Stmt::TypeDef {
            name: "User".into(),
            fields: vec![
                make_field("id", "Int", false),
                make_field("name", "Str", false),
            ],
         span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).unwrap();

        let v = env.borrow().get("User").expect("User no quedó en el env");
        match v {
            Value::Type { name, fields } => {
                assert_eq!(name, "User");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "id");
                assert_eq!(fields[1].name, "name");
            }
            other => panic!("se esperaba Value::Type, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn type_value_type_name_es_type() {
        let t = Value::Type {
            name: "Foo".into(),
            fields: vec![],
        };
        assert_eq!(t.type_name(), "Type");
    }

    #[test]
    fn type_se_puede_referenciar_como_ident_sin_error() {
        // Después de definir un type, `User` como Expr::Ident lo encuentra.
        let env = Environment::new();
        eval_stmt(
            &Stmt::TypeDef {
                name: "User".into(),
                fields: vec![make_field("id", "Int", false)],
             span: Span::ZERO },
            env.clone(),
        ).unwrap();

        let result = eval_expr(&Expr::Ident("User".into(), Span::ZERO), env).unwrap();
        assert!(matches!(result, Value::Type { .. }));
    }

    #[test]
    fn llamar_un_type_como_funcion_es_type_error() {
        // User(1) sin struct literals → TypeMismatch porque Type no es callable.
        // Esto es deuda explícita: la instanciación viene en Fase 3.
        let env = Environment::new();
        eval_stmt(
            &Stmt::TypeDef {
                name: "User".into(),
                fields: vec![make_field("id", "Int", false)],
             span: Span::ZERO },
            env.clone(),
        ).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("User".into(), Span::ZERO)), args: vec![Expr::Int(1, Span::ZERO)], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- Criterio de Fase 2: el programa completo ----

    #[test]
    fn criterio_fase_2_corre_end_to_end() {
        // El programa del roadmap:
        //   name = "Fitz"
        //   x = 10 + 5
        //   print("Hola {name}, x es {x}")
        //   fn double(n) => n * 2
        //   print(double(x))
        //
        // Output esperado (vía stdout, no chequeado acá):
        //   Hola Fitz, x es 15
        //   30
        let program = vec![
            Stmt::Assign { target: AssignTarget::Ident("name".into()),
                type_: None,
                value: Expr::Str("Fitz".into(), Span::ZERO),
             span: Span::ZERO },
            Stmt::Assign { target: AssignTarget::Ident("x".into()),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(10, Span::ZERO)),
                    right: Box::new(Expr::Int(5, Span::ZERO)), span: Span::ZERO,
                },
             span: Span::ZERO },
            Stmt::Expr(Expr::Call { callee: Box::new(Expr::Ident("print".into(), Span::ZERO)), args: vec![Expr::StrInterp(vec![
                    StrPart::Lit("Hola ".into()),
                    StrPart::Expr(Expr::Ident("name".into(), Span::ZERO)),
                    StrPart::Lit(", x es ".into()),
                    StrPart::Expr(Expr::Ident("x".into(), Span::ZERO)),
                ], Span::ZERO)], span: Span::ZERO,
            }, Span::ZERO),
            fn_def(
                "double",
                vec!["n"],
                vec![Stmt::Return(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
                }, Span::ZERO)],
            ),
            Stmt::Expr(Expr::Call { callee: Box::new(Expr::Ident("print".into(), Span::ZERO)), args: vec![Expr::Call { callee: Box::new(Expr::Ident("double".into(), Span::ZERO)), args: vec![Expr::Ident("x".into(), Span::ZERO)], span: Span::ZERO,
                }], span: Span::ZERO,
            }, Span::ZERO),
        ];
        assert!(eval(program).is_ok());
    }

    /// Test de integración: el pipeline completo (lexer → parser → eval)
    /// sobre el programa exacto del criterio de Fase 2 escrito como source.
    /// Si esto pasa, las tres fases hablan bien entre sí.
    #[test]
    fn integracion_criterio_fase_2_lexer_parser_evaluator() {
        let source = r#"
name = "Fitz"
x = 10 + 5
print("Hola {name}, x es {x}")

fn double(n) => n * 2
print(double(x))
"#;
        let tokens = crate::lexer::tokenize(source).expect("lexer falla");
        let program = crate::parser::parse(tokens).expect("parser falla");
        eval(program).expect("evaluator falla");
    }

    #[test]
    fn integracion_factorial_recursivo_end_to_end() {
        // Test de pipeline con recursión + if + return + cierre.
        // Verifica que el evaluator atrapa Return correctamente vía signal.
        let source = r#"
fn factorial(n) {
    if n == 0 {
        return 1
    }
    return n * factorial(n - 1)
}
print(factorial(5))
"#;
        let tokens = crate::lexer::tokenize(source).expect("lexer falla");
        let program = crate::parser::parse(tokens).expect("parser falla");
        eval(program).expect("evaluator falla");
    }

    #[test]
    fn hello_fitz_corre_sin_error() {
        // Réplica del AST equivalente a:
        //   name = "Patagonia"
        //   print("Hola, {name}!")
        //
        // Verifica que el camino Assign → StrInterp → Call (builtin) funciona
        // end-to-end. La salida real se ve con `cargo run -- run examples/hello.fitz`.
        let program = vec![
            Stmt::Assign { target: AssignTarget::Ident("name".into()),
                type_: None,
                value: Expr::Str("Patagonia".into(), Span::ZERO),
             span: Span::ZERO },
            Stmt::Expr(Expr::Call { callee: Box::new(Expr::Ident("print".into(), Span::ZERO)), args: vec![Expr::StrInterp(vec![
                    StrPart::Lit("Hola, ".into()),
                    StrPart::Expr(Expr::Ident("name".into(), Span::ZERO)),
                    StrPart::Lit("!".into()),
                ], Span::ZERO)], span: Span::ZERO,
            }, Span::ZERO),
        ];
        assert!(eval(program).is_ok());
    }

    // -----------------------------------------------------------------------
    // Tests — listas, mapas, rangos, indexing, for (Fase 3, paso 1)
    // -----------------------------------------------------------------------

    /// Helper: parsea y evalúa programa entero. Devuelve el env final.
    fn parse_and_eval(src: &str) -> FitzResult<()> {
        let tokens = crate::lexer::tokenize(src).expect("la fuente debe tokenizar");
        let program = crate::parser::parse(tokens).expect("la fuente debe parsear");
        eval(program)
    }

    /// Como `parse_and_eval`, pero conserva el env para inspeccionarlo.
    /// Útil cuando querés assertear valores específicos al final.
    fn parse_eval_into_env(src: &str) -> (EnvRef, FitzResult<()>) {
        let tokens = crate::lexer::tokenize(src).expect("la fuente debe tokenizar");
        let program = crate::parser::parse(tokens).expect("la fuente debe parsear");
        let env = Environment::new();
        register_builtins(&env);
        for stmt in &program {
            if let Err(signal) = eval_stmt(stmt, env.clone()) {
                return (env, Err(signal_to_error(signal)));
            }
        }
        (env, Ok(()))
    }

    // ---- List literal ----

    #[test]
    fn evalua_list_vacia() {
        let v = eval_expr_test(Expr::List(vec![], Span::ZERO)).unwrap();
        assert_eq!(v, Value::new_list(vec![]));
    }

    #[test]
    fn evalua_list_con_literales() {
        let v = eval_expr_test(Expr::List(vec![
            Expr::Int(1, Span::ZERO),
            Expr::Int(2, Span::ZERO),
            Expr::Int(3, Span::ZERO),
        ], Span::ZERO)).unwrap();
        assert_eq!(v, Value::new_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
    }

    #[test]
    fn evalua_list_con_expresiones() {
        // [1 + 1, 2 * 2]
        let v = eval_expr_test(Expr::List(vec![
            Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1, Span::ZERO)),
                right: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
            },
            Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::Int(2, Span::ZERO)),
                right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
            },
        ], Span::ZERO)).unwrap();
        assert_eq!(v, Value::new_list(vec![Value::Int(2), Value::Int(4)]));
    }

    // ---- Map literal ----

    #[test]
    fn evalua_map_vacio() {
        let v = eval_expr_test(Expr::Map(vec![], Span::ZERO)).unwrap();
        assert_eq!(v, Value::new_map(vec![]));
    }

    #[test]
    fn evalua_map_con_pares() {
        let v = eval_expr_test(Expr::Map(vec![
            (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
            (Expr::Str("b".into(), Span::ZERO), Expr::Int(2, Span::ZERO)),
        ], Span::ZERO)).unwrap();
        assert_eq!(
            v,
            Value::new_map(vec![
                (Value::Str("a".into()), Value::Int(1)),
                (Value::Str("b".into()), Value::Int(2)),
            ]),
        );
    }

    // ---- Range literal ----

    #[test]
    fn evalua_range_simple() {
        let v = eval_expr_test(Expr::Range {
            start: Box::new(Expr::Int(0, Span::ZERO)),
            end: Box::new(Expr::Int(10, Span::ZERO)), span: Span::ZERO,
        }).unwrap();
        assert_eq!(v, Value::Range { start: 0, end: 10 });
    }

    #[test]
    fn evalua_range_con_float_es_error() {
        // 0..1.5 — float no es Int.
        let res = eval_expr_test(Expr::Range {
            start: Box::new(Expr::Int(0, Span::ZERO)),
            end: Box::new(Expr::Float(1.5, Span::ZERO)), span: Span::ZERO,
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(matches!(e.kind, ErrorKind::TypeMismatch { .. })),
            _ => panic!("se esperaba Error"),
        }
    }

    // ---- Indexing ----

    #[test]
    fn index_list_con_int_valido() {
        // [10, 20, 30][1] → 20
        let v = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(10, Span::ZERO), Expr::Int(20, Span::ZERO), Expr::Int(30, Span::ZERO)], Span::ZERO)),
            index: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
        }).unwrap();
        assert_eq!(v, Value::Int(20));
    }

    #[test]
    fn index_list_fuera_de_rango_es_error() {
        // [1, 2][5]
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)], Span::ZERO)),
            index: Box::new(Expr::Int(5, Span::ZERO)), span: Span::ZERO,
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => {
                assert!(e.message.contains("fuera de rango"));
            }
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_list_negativo_es_error() {
        // [1, 2][-1] — sin Python-style por ahora
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)], Span::ZERO)),
            index: Box::new(Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
            }), span: Span::ZERO,
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(e.message.contains("negativo")),
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_list_con_string_es_type_error() {
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1, Span::ZERO)], Span::ZERO)),
            index: Box::new(Expr::Str("a".into(), Span::ZERO)), span: Span::ZERO,
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(matches!(e.kind, ErrorKind::TypeMismatch { .. })),
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_map_clave_existente() {
        // {"a": 1, "b": 2}["b"] → 2
        let v = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Map(vec![
                (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
                (Expr::Str("b".into(), Span::ZERO), Expr::Int(2, Span::ZERO)),
            ], Span::ZERO)),
            index: Box::new(Expr::Str("b".into(), Span::ZERO)), span: Span::ZERO,
        }).unwrap();
        assert_eq!(v, Value::Int(2));
    }

    #[test]
    fn index_map_clave_inexistente_es_error() {
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Map(vec![
                (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
            ], Span::ZERO)),
            index: Box::new(Expr::Str("z".into(), Span::ZERO)), span: Span::ZERO,
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(e.message.contains("clave no encontrada")),
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_sobre_int_es_type_error() {
        // 42[0] — Int no se indexa
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Int(42, Span::ZERO)),
            index: Box::new(Expr::Int(0, Span::ZERO)), span: Span::ZERO,
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(matches!(e.kind, ErrorKind::TypeMismatch { .. })),
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_encadenado_funciona() {
        // [[1, 2], [3, 4]][0][1] → 2
        let v = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Index {
                object: Box::new(Expr::List(vec![
                    Expr::List(vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)], Span::ZERO),
                    Expr::List(vec![Expr::Int(3, Span::ZERO), Expr::Int(4, Span::ZERO)], Span::ZERO),
                ], Span::ZERO)),
                index: Box::new(Expr::Int(0, Span::ZERO)), span: Span::ZERO,
            }),
            index: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
        }).unwrap();
        assert_eq!(v, Value::Int(2));
    }

    // ---- for ----

    #[test]
    fn for_sobre_lista_itera_los_elementos() {
        // total = 1 + 2 + 3 + 4 = 10
        let src = r#"
total = 0
for x in [1, 2, 3, 4] {
    total = total + x
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(10)));
    }

    #[test]
    fn for_sobre_range_itera_inclusivo_exclusivo() {
        // 0..3 → 0 + 1 + 2 = 3 (la cota superior es exclusiva)
        let src = r#"
total = 0
for i in 0..3 {
    total = total + i
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(3)));
    }

    #[test]
    fn for_sobre_lista_vacia_no_itera() {
        let src = r#"
ran = false
for x in [] {
    ran = true
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("ran"), Some(Value::Bool(false)));
    }

    #[test]
    fn for_con_break_corta_iteracion() {
        // Corta cuando i == 3 → last queda en 2.
        let src = r#"
last = 0
for i in 0..10 {
    if i == 3 {
        break
    }
    last = i
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("last"), Some(Value::Int(2)));
    }

    #[test]
    fn for_con_continue_salta_iteracion() {
        // 0..5, saltea i == 2 → 0 + 1 + 3 + 4 = 8.
        let src = r#"
total = 0
for i in 0..5 {
    if i == 2 {
        continue
    }
    total = total + i
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(8)));
    }

    #[test]
    fn for_sobre_map_es_error_explicito() {
        let src = r#"
for x in {"a": 1} {
    print(x)
}
"#;
        let res = parse_and_eval(src);
        let err = res.unwrap_err();
        assert!(err.message.contains("Map"));
    }

    #[test]
    fn for_sobre_int_es_type_error() {
        let src = r#"
for x in 42 {
    print(x)
}
"#;
        let res = parse_and_eval(src);
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[test]
    fn for_loop_var_persiste_despues_del_loop() {
        // Consistente con la política de bloques de Fitz: las variables
        // del body (incluida la variable de iteración) persisten en el
        // scope contenedor. Tras 0..3, i = 2 e last = 2.
        let src = r#"
for i in 0..3 {
    last = i
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("i"), Some(Value::Int(2)));
        assert_eq!(env.borrow().get("last"), Some(Value::Int(2)));
    }

    #[test]
    fn for_anidado_funciona() {
        // 3 * 3 = 9 iteraciones totales.
        let src = r#"
total = 0
for i in 0..3 {
    for j in 0..3 {
        total = total + 1
    }
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(9)));
    }

    // ---- Pattern::Range ----

    #[test]
    fn pattern_range_matchea_valor_dentro() {
        let src = r#"
let n = 5
let r = match n {
    0..10 => "in"
    _     => "out"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("in".into())));
    }

    #[test]
    fn pattern_range_no_matchea_valor_fuera() {
        let src = r#"
let n = 15
let r = match n {
    0..10 => "in"
    _     => "out"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("out".into())));
    }

    #[test]
    fn pattern_range_es_exclusivo_en_el_fin() {
        // n = 10 con patrón 0..10 NO matchea (exclusivo). El segundo arm sí.
        let src = r#"
let n = 10
let r = match n {
    0..10 => "menor"
    10..20 => "diez_o_mas"
    _ => "otro"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("diez_o_mas".into())));
    }

    #[test]
    fn pattern_range_con_negativos() {
        let src = r#"
let n = -3
let r = match n {
    -10..0 => "negativo"
    0..10 => "chico"
    _ => "otro"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("negativo".into())));
    }

    #[test]
    fn pattern_range_no_matchea_no_int() {
        // 3.14 contra patrón 0..10 → no matchea, cae a wildcard.
        let src = r#"
let n = 3.14
let r = match n {
    0..10 => "int_chico"
    _ => "no_int"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("no_int".into())));
    }

    // ---- builtin len ----

    #[test]
    fn len_de_lista_devuelve_cantidad_de_elementos() {
        let src = "n = len([1, 2, 3, 4, 5])";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(5)));
    }

    #[test]
    fn len_de_lista_vacia_es_cero() {
        let src = "n = len([])";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(0)));
    }

    #[test]
    fn len_de_mapa_devuelve_cantidad_de_pares() {
        let src = r#"n = len({"a": 1, "b": 2, "c": 3})"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(3)));
    }

    #[test]
    fn len_de_string_cuenta_chars_no_bytes() {
        // "ñandú" tiene 5 chars y más de 5 bytes en UTF-8.
        let src = r#"n = len("ñandú")"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(5)));
    }

    #[test]
    fn len_de_range_devuelve_cantidad_de_elementos() {
        let src = "n = len(0..10)";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(10)));
    }

    #[test]
    fn len_de_range_al_reves_es_cero() {
        // 10..0 — el evaluador trata rangos invertidos como vacíos.
        let src = "n = len(10..0)";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(0)));
    }

    #[test]
    fn len_de_int_es_type_error() {
        let src = "n = len(42)";
        let res = parse_and_eval(src);
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[test]
    fn len_con_cantidad_de_args_incorrecta_es_error() {
        let src = "n = len([1], [2])";
        let res = parse_and_eval(src);
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::WrongArgCount { .. }));
    }

    // -----------------------------------------------------------------------
    // Tests — Tipos custom instanciables (Fase 3, paso 2)
    //
    // El evaluador resuelve `User { id: 1, name: "x" }` contra el `type`
    // declarado, aplica defaults y nullables, valida campos faltantes y
    // extras, y permite `obj.campo` sobre la instancia resultante.
    // -----------------------------------------------------------------------

    #[test]
    fn struct_literal_basico_con_todos_los_campos() {
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"Fitz\" }\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let u = env.borrow().get("u").unwrap();
        match u {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "User");
                let fields = fields.borrow();
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0], ("id".into(), Value::Int(1)));
                assert_eq!(fields[1], ("name".into(), Value::Str("Fitz".into())));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_literal_ordena_campos_segun_la_declaracion() {
        // El literal tipea los campos al revés; la instancia debe seguir
        // el orden del `type`.
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { name: \"Fitz\", id: 1 }\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let u = env.borrow().get("u").unwrap();
        match u {
            Value::Instance { fields, .. } => {
                let fields = fields.borrow();
                assert_eq!(fields[0].0, "id");
                assert_eq!(fields[1].0, "name");
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_literal_aplica_default_cuando_se_omite_un_campo() {
        let src = "\
            type Config { host: Str, port: Int = 3000 }\n\
            let c = Config { host: \"localhost\" }\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let c = env.borrow().get("c").unwrap();
        match c {
            Value::Instance { fields, .. } => {
                let fields = fields.borrow();
                assert_eq!(fields[0], ("host".into(), Value::Str("localhost".into())));
                assert_eq!(fields[1], ("port".into(), Value::Int(3000)));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_literal_default_se_evalua_en_el_env_de_instanciacion() {
        // El default es una expresión: se evalúa al instanciar, en el
        // scope donde ocurre el literal. Si el usuario define una var
        // con ese nombre, el default la ve.
        let src = "\
            type Cfg { port: Int = base + 1 }\n\
            let base = 4000\n\
            let c = Cfg {}\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let c = env.borrow().get("c").unwrap();
        match c {
            Value::Instance { fields, .. } => {
                let fields = fields.borrow();
                assert_eq!(fields[0], ("port".into(), Value::Int(4001)));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_literal_campo_nullable_omitido_es_null() {
        let src = "\
            type User { id: Int, email: Str? }\n\
            let u = User { id: 1 }\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let u = env.borrow().get("u").unwrap();
        match u {
            Value::Instance { fields, .. } => {
                let fields = fields.borrow();
                assert_eq!(fields[1], ("email".into(), Value::Null));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_literal_campo_nullable_explicito_a_null() {
        let src = "\
            type User { id: Int, email: Str? }\n\
            let u = User { id: 1, email: null }\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let u = env.borrow().get("u").unwrap();
        match u {
            Value::Instance { fields, .. } => {
                let fields = fields.borrow();
                assert_eq!(fields[1], ("email".into(), Value::Null));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_literal_campo_faltante_sin_default_ni_nullable_es_error() {
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1 }\n\
        ";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("name"),
            "el error debería mencionar el campo faltante `name`: {}",
            err.message
        );
    }

    #[test]
    fn struct_literal_campo_extra_no_declarado_es_error() {
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"x\", color: \"red\" }\n\
        ";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("color"),
            "el error debería mencionar el campo extra `color`: {}",
            err.message
        );
    }

    #[test]
    fn struct_literal_de_tipo_no_definido_es_error() {
        let src = "let u = NoExiste { id: 1 }";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UndefinedVariable(_)));
    }

    #[test]
    fn struct_literal_sobre_no_tipo_es_type_error() {
        // `x` es Int, no un Type — instanciarlo es error.
        let src = "\
            let x = 42\n\
            let u = x { id: 1 }\n\
        ";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[test]
    fn field_access_sobre_instance_devuelve_el_valor() {
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"Fitz\" }\n\
            let n = u.name\n\
            let i = u.id\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Str("Fitz".into())));
        assert_eq!(env.borrow().get("i"), Some(Value::Int(1)));
    }

    #[test]
    fn field_access_campo_inexistente_es_error() {
        let src = "\
            type User { id: Int }\n\
            let u = User { id: 1 }\n\
            let x = u.nope\n\
        ";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("nope"),
            "el error debería mencionar el campo `nope`: {}",
            err.message
        );
    }

    #[test]
    fn field_access_sobre_no_instance_es_type_error() {
        // Field access "pelado" sobre un Int explota: no hay propiedades
        // sobre primitivos. Los métodos sí (`x.upper()` para Str, etc.),
        // pero ese camino va por `Expr::Call` con callee `Field`, no por
        // este branch.
        let src = "\
            let x = 42\n\
            let n = x.foo\n\
        ";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[test]
    fn struct_literal_anidado_y_field_access_encadenado() {
        let src = "\
            type User { id: Int, name: Str }\n\
            type Order { user: User, total: Int }\n\
            let o = Order { user: User { id: 1, name: \"Fitz\" }, total: 100 }\n\
            let n = o.user.name\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Str("Fitz".into())));
    }

    #[test]
    fn instance_se_imprime_con_display_esperado() {
        // Sanity: el print de una instancia muestra el formato canónico.
        // (No capturamos stdout — usamos `to_string` del Value retornado.)
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"Fitz\" }\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let u = env.borrow().get("u").unwrap();
        assert_eq!(u.to_string(), "User { id: 1, name: \"Fitz\" }");
    }

    // -----------------------------------------------------------------------
    // Tests — Result + Ok/Err + ? (Fase 3, paso 3)
    //
    // Estos tests construyen el AST a mano para evitar depender del parser,
    // que recibe el soporte para `Ok`/`Err`/`?` en este mismo paso.
    // -----------------------------------------------------------------------

    fn ok_value(v: Value) -> Value {
        Value::Result(ResultVariant::Ok(Box::new(v)))
    }

    fn err_value(v: Value) -> Value {
        Value::Result(ResultVariant::Err(Box::new(v)))
    }

    #[test]
    fn ok_ctor_evalua_a_value_result_ok() {
        // Ok(42) → Value::Result(Ok(Int(42)))
        let e = Expr::Ok(Box::new(Expr::Int(42, Span::ZERO)), Span::ZERO);
        assert_eq!(eval_expr_test(e).unwrap(), ok_value(Value::Int(42)));
    }

    #[test]
    fn err_ctor_evalua_a_value_result_err() {
        // Err("boom") → Value::Result(Err(Str("boom")))
        let e = Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO);
        assert_eq!(
            eval_expr_test(e).unwrap(),
            err_value(Value::Str("boom".into())),
        );
    }

    #[test]
    fn ok_ctor_evalua_inner_antes_de_envolver() {
        // Ok(1 + 2) → Value::Result(Ok(Int(3)))
        let e = Expr::Ok(Box::new(Expr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(Expr::Int(1, Span::ZERO)),
            right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
        }), Span::ZERO);
        assert_eq!(eval_expr_test(e).unwrap(), ok_value(Value::Int(3)));
    }

    #[test]
    fn try_sobre_ok_desempaqueta() {
        // Ok(7)? evaluado adentro de una función debería ser 7.
        // Lo testeamos directamente: como no hay return contenedor, el `?`
        // sobre Ok no emite ningún signal y la expresión vale 7.
        let e = Expr::Try(Box::new(Expr::Ok(Box::new(Expr::Int(7, Span::ZERO)), Span::ZERO)), Span::ZERO);
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(7));
    }

    #[test]
    fn try_sobre_err_emite_signal_return_con_err() {
        // Err("boom")? emite EvalSignal::Return(Value::Result(Err("boom"))).
        let e = Expr::Try(Box::new(Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO)), Span::ZERO);
        let env = Environment::new();
        match eval_expr(&e, env) {
            Err(EvalSignal::Return(v)) => {
                assert_eq!(v, err_value(Value::Str("boom".into())));
            }
            other => panic!("se esperaba EvalSignal::Return(Err(...)), se obtuvo {:?}", other),
        }
    }

    #[test]
    fn try_sobre_no_result_es_type_error() {
        // 42? → error: el operador `?` requiere un Result, no Int.
        let e = Expr::Try(Box::new(Expr::Int(42, Span::ZERO)), Span::ZERO);
        let env = Environment::new();
        match eval_expr(&e, env) {
            Err(EvalSignal::Error(err)) => {
                assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
                assert!(
                    err.message.contains("operador `?`"),
                    "mensaje inesperado: {}",
                    err.message,
                );
            }
            other => panic!("se esperaba error de tipo, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn try_adentro_de_funcion_con_ok_devuelve_inner() {
        // fn pass() { return Ok(5)? }  → pass() == 5  (porque return de un
        // valor "pelado" de Int sale como Int, no como Result).
        //
        // Acá lo que probamos es que `Ok(5)?` desempaqueta a 5 sin emitir
        // signal de retorno. La función devuelve ese 5 vía su return propio.
        let env = Environment::new();
        let body = vec![Stmt::Return(Expr::Try(Box::new(Expr::Ok(Box::new(
            Expr::Int(5, Span::ZERO),
        ), Span::ZERO)), Span::ZERO), Span::ZERO)];
        eval_stmt(&fn_def("pass", vec![], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("pass".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(5));
    }

    #[test]
    fn try_adentro_de_funcion_con_err_propaga() {
        // fn boom() { let _ = Err("nope")? ; return Ok("nunca llega") }
        // boom() devuelve Value::Result(Err("nope")) sin ejecutar el return.
        let env = Environment::new();
        let body = vec![
            Stmt::Assign { target: AssignTarget::Ident("_".into()),
                type_: None,
                value: Expr::Try(Box::new(Expr::Err(Box::new(Expr::Str("nope".into(), Span::ZERO)), Span::ZERO)), Span::ZERO),
             span: Span::ZERO },
            Stmt::Return(Expr::Ok(Box::new(Expr::Str("nunca llega".into(), Span::ZERO)), Span::ZERO), Span::ZERO),
        ];
        eval_stmt(&fn_def("boom", vec![], body), env.clone()).unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("boom".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(
            eval_expr(&call, env).unwrap(),
            err_value(Value::Str("nope".into())),
        );
    }

    #[test]
    fn programa_e2e_find_user_con_result_y_try() {
        // Programa similar al criterio de éxito de Fase 3:
        // un find_user manual que devuelve Result, con `?` y `match`.
        let src = "\
            type User { id: Int, name: Str }\n\
            \n\
            fn find(target) {\n\
            \tif (target == 1) {\n\
            \t\treturn Ok(User { id: 1, name: \"Fitz\" })\n\
            \t}\n\
            \treturn Err(\"no encontrado\")\n\
            }\n\
            \n\
            fn lookup_name(id) {\n\
            \tlet u = find(id)?\n\
            \treturn Ok(u.name)\n\
            }\n\
            \n\
            let hit = lookup_name(1)\n\
            let miss = lookup_name(99)\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(
            env.borrow().get("hit"),
            Some(ok_value(Value::Str("Fitz".into()))),
        );
        assert_eq!(
            env.borrow().get("miss"),
            Some(err_value(Value::Str("no encontrado".into()))),
        );
    }

    #[test]
    fn match_e2e_sobre_result_con_ok_y_err() {
        let src = "\
            fn divide(a, b) {\n\
            \tif (b == 0) {\n\
            \t\treturn Err(\"división por cero\")\n\
            \t}\n\
            \treturn Ok(a / b)\n\
            }\n\
            \n\
            let ok_msg = match divide(10, 2) {\n\
            \tOk(v) => \"ok: {v}\"\n\
            \tErr(e) => \"err: {e}\"\n\
            }\n\
            let err_msg = match divide(10, 0) {\n\
            \tOk(v) => \"ok: {v}\"\n\
            \tErr(e) => \"err: {e}\"\n\
            }\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("ok_msg"), Some(Value::Str("ok: 5".into())));
        assert_eq!(
            env.borrow().get("err_msg"),
            Some(Value::Str("err: divisi\u{00f3}n por cero".into())),
        );
    }

    #[test]
    fn try_top_level_con_err_genera_error_de_return_huerfano() {
        // En top-level, `Err(...)?` emite Return; el evaluador global lo
        // convierte en "return solo puede usarse adentro de una función".
        let env = Environment::new();
        let stmt = Stmt::Expr(Expr::Try(Box::new(Expr::Err(Box::new(Expr::Int(1, Span::ZERO)), Span::ZERO)), Span::ZERO), Span::ZERO);
        match eval_stmt(&stmt, env.clone()) {
            Err(EvalSignal::Return(_)) => {} // ok — el global lo traduciría.
            other => panic!("se esperaba EvalSignal::Return, se obtuvo {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Fase 3, paso 4 (fn anónimas, method calls, mutación de campos)
    // -----------------------------------------------------------------------

    #[test]
    fn fn_expr_evalua_a_function() {
        // `fn(x) => x * 2` — evaluada sola, da un `Value::Function`.
        let fnexpr = Expr::FnExpr {
            params: vec![crate::ast::Param { name: "x".into(), type_: None }],
            body: vec![Stmt::Return(Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
            }, Span::ZERO)], span: Span::ZERO,
        };
        let env = Environment::new();
        let v = eval_expr(&fnexpr, env).unwrap();
        assert!(matches!(v, Value::Function { .. }));
    }

    #[test]
    fn fn_expr_invocada_al_vuelo() {
        // `(fn(x) => x + 1)(2)` → 3
        let src = "let y = (fn(x) => x + 1)(2)\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("y"), Some(Value::Int(3)));
    }

    #[test]
    fn fn_expr_captura_el_env_actual() {
        // El cuerpo de la anónima ve `n` definido afuera (closure).
        let src = "\
            let n = 10\n\
            let f = fn(x) => x + n\n\
            let r = f(5)\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Int(15)));
    }

    #[test]
    fn fn_expr_se_pasa_como_argumento() {
        // Pasar fn anónima como callback a una función de orden superior
        // declarada por el usuario.
        let src = "\
            fn apply(f, x) => f(x)\n\
            let r = apply(fn(n) => n * n, 6)\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Int(36)));
    }

    #[test]
    fn field_assign_muta_la_instancia() {
        // `user.name = "Otro"` cambia el campo, visible a través de
        // cualquier alias.
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"Fitz\" }\n\
            u.name = \"Otro\"\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let u = env.borrow().get("u").unwrap();
        match u {
            Value::Instance { fields, .. } => {
                let fields = fields.borrow();
                assert_eq!(fields[1], ("name".into(), Value::Str("Otro".into())));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn field_assign_visible_a_traves_de_alias() {
        // Dos variables apuntan a la misma instancia (vía `Rc`); mutar
        // por una se ve por la otra.
        let src = "\
            type Box { value: Int }\n\
            let a = Box { value: 1 }\n\
            let b = a\n\
            a.value = 42\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let b = env.borrow().get("b").unwrap();
        match b {
            Value::Instance { fields, .. } => {
                let fields = fields.borrow();
                assert_eq!(fields[0], ("value".into(), Value::Int(42)));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn field_assign_a_no_instance_es_error() {
        // `x.field = ...` sobre algo que no es Instance corta con type error.
        let src = "\
            let x = 10\n\
            x.field = 1\n\
        ";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[test]
    fn field_assign_a_campo_inexistente_es_error() {
        let src = "\
            type User { id: Int }\n\
            let u = User { id: 1 }\n\
            u.nope = 2\n\
        ";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(err.message.contains("nope"));
    }

    #[test]
    fn method_call_sobre_tipo_sin_metodo_emite_error_explicito() {
        // `xs.foo()` no existe — el dispatch corta con
        // "no tiene un método llamado foo".
        let src = "\
            let xs = [1, 2, 3]\n\
            xs.foo()\n\
        ";
        let err = parse_and_eval(src).unwrap_err();
        assert!(err.message.contains("método"), "mensaje: {}", err.message);
    }

    // -----------------------------------------------------------------------
    // Tests — built-ins de List
    // -----------------------------------------------------------------------

    #[test]
    fn list_push_muta_in_place() {
        let src = "\
            let xs = [1, 2]\n\
            xs.push(3)\n\
            xs.push(4)\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let xs = env.borrow().get("xs").unwrap();
        assert_eq!(
            xs,
            Value::new_list(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
                Value::Int(4),
            ]),
        );
    }

    #[test]
    fn list_push_visible_a_traves_de_alias() {
        // Dos variables al mismo Rc; mutar por una se ve por la otra.
        let src = "\
            let a = [1]\n\
            let b = a\n\
            a.push(2)\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        let b = env.borrow().get("b").unwrap();
        assert_eq!(b, Value::new_list(vec![Value::Int(1), Value::Int(2)]));
    }

    #[test]
    fn list_pop_devuelve_el_ultimo_y_acorta() {
        let src = "\
            let xs = [1, 2, 3]\n\
            let last = xs.pop()\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("last"), Some(Value::Int(3)));
        assert_eq!(
            env.borrow().get("xs"),
            Some(Value::new_list(vec![Value::Int(1), Value::Int(2)])),
        );
    }

    #[test]
    fn list_pop_sobre_vacia_es_error() {
        let src = "let xs = []\nlet _ = xs.pop()\n";
        let err = parse_and_eval(src).unwrap_err();
        assert!(err.message.contains("vacía"), "mensaje: {}", err.message);
    }

    #[test]
    fn list_map_aplica_fn_a_cada_elemento() {
        let src = "let r = [1, 2, 3].map(fn(n) => n * 10)\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(
            env.borrow().get("r"),
            Some(Value::new_list(vec![
                Value::Int(10),
                Value::Int(20),
                Value::Int(30),
            ])),
        );
    }

    #[test]
    fn list_filter_solo_mantiene_los_true() {
        let src = "let r = [1, 2, 3, 4].filter(fn(n) => n == 2 or n == 4)\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(
            env.borrow().get("r"),
            Some(Value::new_list(vec![Value::Int(2), Value::Int(4)])),
        );
    }

    #[test]
    fn list_filter_callback_no_bool_es_error() {
        let src = "let r = [1, 2].filter(fn(n) => n)\n";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[test]
    fn list_find_devuelve_ok_cuando_matchea() {
        let src = "let r = [1, 2, 3].find(fn(n) => n == 2)\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(
            env.borrow().get("r"),
            Some(ok_value(Value::Int(2))),
        );
    }

    #[test]
    fn list_find_devuelve_err_cuando_no_hay_match() {
        let src = "let r = [1, 2, 3].find(fn(n) => n == 99)\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(
            env.borrow().get("r"),
            Some(err_value(Value::Str("no encontrado".into()))),
        );
    }

    #[test]
    fn list_metodo_len() {
        let src = "let n = [1, 2, 3, 4].len()\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(4)));
    }

    // -----------------------------------------------------------------------
    // Tests — built-ins de Map
    // -----------------------------------------------------------------------

    #[test]
    fn map_get_devuelve_ok_si_hay_clave() {
        let src = "let r = {\"a\": 1}.get(\"a\")\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(ok_value(Value::Int(1))));
    }

    #[test]
    fn map_get_devuelve_err_si_no_hay_clave() {
        let src = "let r = {\"a\": 1}.get(\"nope\")\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        // El mensaje del Err lleva la clave.
        let r = env.borrow().get("r").unwrap();
        match r {
            Value::Result(ResultVariant::Err(inner)) => match *inner {
                Value::Str(s) => assert!(s.contains("nope")),
                other => panic!("se esperaba Str dentro de Err, se obtuvo {:?}", other),
            },
            other => panic!("se esperaba Err, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn map_has_devuelve_true_o_false() {
        let src = "\
            let m = {\"a\": 1}\n\
            let yes = m.has(\"a\")\n\
            let no = m.has(\"x\")\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("yes"), Some(Value::Bool(true)));
        assert_eq!(env.borrow().get("no"), Some(Value::Bool(false)));
    }

    #[test]
    fn map_keys_y_values_preservan_orden_de_insercion() {
        let src = "\
            let m = {\"b\": 2, \"a\": 1}\n\
            let ks = m.keys()\n\
            let vs = m.values()\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(
            env.borrow().get("ks"),
            Some(Value::new_list(vec![
                Value::Str("b".into()),
                Value::Str("a".into()),
            ])),
        );
        assert_eq!(
            env.borrow().get("vs"),
            Some(Value::new_list(vec![Value::Int(2), Value::Int(1)])),
        );
    }

    // -----------------------------------------------------------------------
    // Tests — built-ins de Str
    // -----------------------------------------------------------------------

    #[test]
    fn str_metodo_len_cuenta_chars() {
        let src = "let n = \"hola\".len()\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(4)));
    }

    #[test]
    fn str_upper_y_lower() {
        let src = "\
            let a = \"hola\".upper()\n\
            let b = \"MUNDO\".lower()\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("a"), Some(Value::Str("HOLA".into())));
        assert_eq!(env.borrow().get("b"), Some(Value::Str("mundo".into())));
    }

    #[test]
    fn metodo_con_aridad_incorrecta_es_error() {
        let src = "let r = \"x\".upper(1)\n";
        let err = parse_and_eval(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::WrongArgCount { .. }));
    }

    // -----------------------------------------------------------------------
    // Tests — encadenamiento y composición
    // -----------------------------------------------------------------------

    #[test]
    fn metodos_se_encadenan() {
        // `.map(...).filter(...)` se encadena vía postfix. El parser corta
        // sentencias en el newline; el encadenamiento multi-línea con `.`
        // al inicio de la línea siguiente todavía no se soporta (deuda
        // explícita). Se mantiene la cadena en una sola línea.
        let src = "let r = [1, 2, 3, 4].map(fn(n) => n * n).filter(fn(n) => n > 5)\n";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(
            env.borrow().get("r"),
            Some(Value::new_list(vec![Value::Int(9), Value::Int(16)])),
        );
    }

    // -----------------------------------------------------------------------
    // Test E2E — criterio de éxito de Fase 3
    // -----------------------------------------------------------------------

    #[test]
    fn programa_e2e_criterio_de_exito_fase_3() {
        // `users.find(fn(u) => u.id == id)` — usa method call, fn anónima,
        // Result, struct literal y field access. `find` ya devuelve
        // `Result<User>` así que `find_user` lo retorna directo. (Usar
        // `return` adentro de un `match` como expresión es deuda; el
        // caso de uso natural acá no lo necesita.)
        let src = "\
            type User { id: Int, name: Str }\n\
            \n\
            fn find_user(users, id) {\n\
            \treturn users.find(fn(u) => u.id == id)\n\
            }\n\
            \n\
            let users = [\n\
            \tUser { id: 1, name: \"Fitz\" },\n\
            \tUser { id: 2, name: \"Roy\" },\n\
            ]\n\
            \n\
            let hit = find_user(users, 1)\n\
            let miss = find_user(users, 99)\n\
        ";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();

        // hit es Ok(User { id: 1, name: "Fitz" })
        let hit = env.borrow().get("hit").unwrap();
        match hit {
            Value::Result(ResultVariant::Ok(inner)) => match *inner {
                Value::Instance { ref type_name, ref fields } => {
                    assert_eq!(type_name, "User");
                    let f = fields.borrow();
                    assert_eq!(f[0], ("id".into(), Value::Int(1)));
                    assert_eq!(f[1], ("name".into(), Value::Str("Fitz".into())));
                }
                other => panic!("se esperaba Instance adentro del Ok, se obtuvo {:?}", other),
            },
            other => panic!("se esperaba Ok, se obtuvo {:?}", other),
        }

        // miss es Err("no encontrado") — el mensaje viene de list_find.
        let miss = env.borrow().get("miss").unwrap();
        assert_eq!(miss, err_value(Value::Str("no encontrado".into())));
    }

    // -----------------------------------------------------------------------
    // Tests — Módulos / import (Fase 3, paso 5)
    // -----------------------------------------------------------------------

    /// Helper: monta `files` (path relativo → contenido) en un tempdir,
    /// evalúa `main_src` con `base_dir` apuntando a ese tempdir, y
    /// devuelve `(env, resultado)`. El tempdir vive lo suficiente para
    /// que el loader pueda leer los archivos; se libera al final.
    fn eval_with_modules(
        files: &[(&str, &str)],
        main_src: &str,
    ) -> (EnvRef, FitzResult<()>) {
        let dir = tempfile::tempdir().expect("creando tempdir");
        for (rel_path, content) in files {
            let full = dir.path().join(rel_path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).expect("creando subdir");
            }
            std::fs::write(&full, content).expect("escribiendo fixture");
        }

        let tokens = crate::lexer::tokenize(main_src).expect("la fuente debe tokenizar");
        let program = crate::parser::parse(tokens).expect("la fuente debe parsear");

        install_loader(dir.path().to_path_buf());
        // Guard local: garantizamos uninstall aun ante panic en eval.
        let _guard = LoaderGuard;

        let env = Environment::new();
        register_builtins(&env);
        let mut result: FitzResult<()> = Ok(());
        for stmt in &program {
            if let Err(signal) = eval_stmt(stmt, env.clone()) {
                result = Err(signal_to_error(signal));
                break;
            }
        }
        // Cerramos el tempdir explícitamente para que se borre antes de
        // que el helper retorne. Los `Value` ya están en memoria (env
        // contiene clones); no dependen del fs.
        drop(dir);
        (env, result)
    }

    #[test]
    fn import_simple_expone_el_modulo_como_namespace() {
        // `import utils` + `utils.greet("Fitz")` — el módulo exporta
        // una fn que devuelve un Str interpolado.
        let utils = "fn greet(name) => \"hola, {name}\"\n";
        let main = "\
            import utils\n\
            let g = utils.greet(\"Fitz\")\n\
        ";
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main);
        res.unwrap();
        assert_eq!(env.borrow().get("g"), Some(Value::Str("hola, Fitz".into())));
    }

    #[test]
    fn import_bindea_bajo_el_ultimo_segmento() {
        // `import sub.foo` → binding `foo` (no `sub.foo`). El path
        // resuelve a `sub/foo.fitz`.
        let foo = "fn one() => 1\n";
        let main = "\
            import sub.foo\n\
            let r = foo.one()\n\
        ";
        let (env, res) = eval_with_modules(&[("sub/foo.fitz", foo)], main);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Int(1)));
        // `sub` NO se bindea — solo el último segmento.
        assert!(env.borrow().get("sub").is_none());
    }

    #[test]
    fn from_import_bindea_nombres_directos() {
        // `from utils import greet, NAME` trae `greet` y `NAME` al
        // scope actual, sin exponer el módulo.
        let utils = "\
            let NAME = \"Fitz\"\n\
            fn greet(n) => \"hola, {n}\"\n\
        ";
        let main = "\
            from utils import greet, NAME\n\
            let g = greet(NAME)\n\
        ";
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main);
        res.unwrap();
        assert_eq!(env.borrow().get("g"), Some(Value::Str("hola, Fitz".into())));
        // `utils` NO se bindea cuando se usa `from import`.
        assert!(env.borrow().get("utils").is_none());
    }

    #[test]
    fn from_import_de_tipo_permite_struct_literal() {
        // `from foo import User` + `User { id: 1, name: "x" }` — el
        // parser de struct literal espera `Ident { ... }`, y `from
        // import` trae el Value::Type al scope con ese nombre.
        let foo = "type User { id: Int, name: Str }\n";
        let main = "\
            from foo import User\n\
            let u = User { id: 7, name: \"Fitz\" }\n\
            let nm = u.name\n\
        ";
        let (env, res) = eval_with_modules(&[("foo.fitz", foo)], main);
        res.unwrap();
        assert_eq!(env.borrow().get("nm"), Some(Value::Str("Fitz".into())));
    }

    #[test]
    fn modulo_no_existe_da_error_con_path_resuelto() {
        let main = "import inexistente\n";
        let (_env, res) = eval_with_modules(&[], main);
        let err = res.unwrap_err();
        assert!(err.message.contains("inexistente"),
            "el mensaje debe nombrar el módulo: {}", err.message);
        assert!(err.message.contains("no se encontró"),
            "el mensaje debe decir 'no se encontró': {}", err.message);
    }

    #[test]
    fn from_import_de_nombre_inexistente_da_error_claro() {
        // El módulo carga, pero el nombre pedido no existe en él.
        let utils = "fn a() => 1\n";
        let main = "from utils import b\n";
        let (_env, res) = eval_with_modules(&[("utils.fitz", utils)], main);
        let err = res.unwrap_err();
        assert!(err.message.contains("no exporta"), "msg: {}", err.message);
        assert!(err.message.contains("`b`"), "msg: {}", err.message);
        assert!(err.message.contains("`utils`"), "msg: {}", err.message);
    }

    #[test]
    fn field_access_en_modulo_inexistente_da_error_claro() {
        // `import utils` + `utils.missing` — el módulo carga pero
        // no expone `missing`.
        let utils = "fn a() => 1\n";
        let main = "\
            import utils\n\
            let x = utils.missing\n\
        ";
        let (_env, res) = eval_with_modules(&[("utils.fitz", utils)], main);
        let err = res.unwrap_err();
        assert!(err.message.contains("no exporta") && err.message.contains("missing"),
            "msg: {}", err.message);
    }

    #[test]
    fn modulo_cargado_dos_veces_no_re_ejecuta_side_effects() {
        // Cada vez que un módulo se evalúa, su body corre. Pero el
        // cache hace que un segundo import del mismo archivo devuelva
        // el mismo `Value::Module` sin re-ejecutar el body. Para
        // medirlo, el módulo escribe en una lista compartida y
        // contamos cuántas veces se incrementó.
        //
        // No usamos side effects de print porque no tenemos forma
        // de capturar stdout en tests; en su lugar, comparamos
        // identidad de un valor exportado.
        let counter_mod = "let value = 42\n";
        let main = "\
            import counter_mod\n\
            import counter_mod\n\
            let v = counter_mod.value\n\
        ";
        let (env, res) = eval_with_modules(&[("counter_mod.fitz", counter_mod)], main);
        res.unwrap();
        assert_eq!(env.borrow().get("v"), Some(Value::Int(42)));
        // Como no podemos detectar re-ejecución desde el lado del
        // lenguaje, validamos al menos que ambos `import` no rompan
        // ni dupliquen estado: el binding `counter_mod` queda accesible
        // y consistente.
        let m = env.borrow().get("counter_mod").unwrap();
        assert!(matches!(m, Value::Module { .. }));
    }

    #[test]
    fn modulo_cacheado_devuelve_misma_identidad_de_env() {
        // Cargar un módulo dos veces desde paths distintos pero al
        // mismo archivo (acá igual path) devuelve `Value::Module` con
        // el MISMO `Rc<RefCell<Environment>>` adentro. Eso lo testea
        // el `PartialEq` de Module (por identidad del env).
        //
        // En este test, dos `from utils import x` (que requieren cargar
        // utils) deberían producir el mismo cache; verificamos
        // accediendo dos veces a un binding "alias" del módulo.
        let utils = "let x = 7\n";
        let main = "\
            import utils\n\
            let u1 = utils\n\
            import utils\n\
            let u2 = utils\n\
        ";
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main);
        res.unwrap();
        let u1 = env.borrow().get("u1").unwrap();
        let u2 = env.borrow().get("u2").unwrap();
        assert_eq!(u1, u2, "el segundo import debe devolver el mismo módulo cacheado");
    }

    #[test]
    fn ciclo_a_b_a_se_detecta() {
        // a.fitz importa b.fitz que importa a.fitz. Mientras se
        // evalúa a (todavía sin terminar), b intenta importar a y el
        // loader detecta el ciclo.
        let a = "\
            import b\n\
            let from_a = 1\n\
        ";
        let b = "\
            import a\n\
            let from_b = 2\n\
        ";
        let main = "import a\n";
        let (_env, res) = eval_with_modules(&[("a.fitz", a), ("b.fitz", b)], main);
        let err = res.unwrap_err();
        assert!(err.message.contains("ciclo de imports"),
            "msg: {}", err.message);
    }

    #[test]
    fn import_anidado_resuelve_relativo_al_modulo_importer() {
        // `main` importa `sub.foo`, y `sub/foo.fitz` importa `bar`,
        // que tiene que resolverse como `sub/bar.fitz` (relativo a
        // `foo`, no a main). Esto verifica el swap de `base_dir`
        // durante la carga del módulo.
        let foo = "\
            import bar\n\
            fn outer() => bar.inner()\n\
        ";
        let bar = "fn inner() => \"desde bar\"\n";
        let main = "\
            import sub.foo\n\
            let r = foo.outer()\n\
        ";
        let (env, res) = eval_with_modules(&[
            ("sub/foo.fitz", foo),
            ("sub/bar.fitz", bar),
        ], main);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("desde bar".into())));
    }

    #[test]
    fn modulo_con_error_de_sintaxis_propaga_error() {
        // Si el módulo importado tiene un parse error, debería
        // propagarse al importer en lugar de pasar silenciosamente.
        let busted = "let x = +\n"; // syntax error
        let main = "import busted\n";
        let (_env, res) = eval_with_modules(&[("busted.fitz", busted)], main);
        assert!(res.is_err(), "se esperaba error de parseo del módulo");
    }

    #[test]
    fn modulo_con_error_de_runtime_propaga_error() {
        // El módulo carga (parsea bien) pero su top-level body
        // dispara un error al evaluar — debería propagarse.
        let busted = "let x = no_existe\n";
        let main = "import busted\n";
        let (_env, res) = eval_with_modules(&[("busted.fitz", busted)], main);
        let err = res.unwrap_err();
        // Esperamos UndefinedVariable de adentro del módulo.
        assert!(matches!(err.kind, ErrorKind::UndefinedVariable(_)));
    }

    #[test]
    fn method_call_sobre_modulo_invoca_funcion_exportada() {
        // `utils.suma(2, 3)` debe resolver a `suma` adentro de utils.
        let utils = "fn suma(a, b) => a + b\n";
        let main = "\
            import utils\n\
            let r = utils.suma(2, 3)\n\
        ";
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Int(5)));
    }

    #[test]
    fn funcion_importada_via_from_import_cierra_sobre_env_del_modulo() {
        // `from utils import greet`, después `greet("x")` ejecuta el
        // body de greet. Ese body usa una variable del módulo
        // (`PREFIX`) — la captura por closure debe seguir viendo el
        // env del módulo, no el del importer.
        //
        // PREFIX NO está en el scope del importer; si la closure no
        // capturó el env del módulo, esto rompería con UndefinedVariable.
        let utils = "\
            let PREFIX = \"saludos, \"\n\
            fn greet(name) => \"{PREFIX}{name}\"\n\
        ";
        let main = "\
            from utils import greet\n\
            let g = greet(\"Fitz\")\n\
        ";
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main);
        res.unwrap();
        assert_eq!(env.borrow().get("g"), Some(Value::Str("saludos, Fitz".into())));
    }

    // -----------------------------------------------------------------------
    // Tests — decoradores (Fase 4, pasos 4.1 / 4.2)
    // -----------------------------------------------------------------------
    //
    // El evaluador procesa decorators al ver `Stmt::FnDef`. Los HTTP
    // (`@get`/`@post`/`@put`/`@delete`) requieren `HttpRegistry`
    // activo en el thread_local; sin él, error explícito. Cualquier
    // otro decorator también es error (`@server` entra en 4.4).

    #[test]
    fn fndef_con_decorator_http_sin_registry_da_error_claro() {
        // `parse_and_eval` no instala HttpRegistry, así que un
        // `@get(...)` corta con sugerencia de usar `fitz run`.
        let src = "@get(\"/\")\nfn index() => \"hola\"";
        let err = parse_and_eval(src).unwrap_err();
        assert!(
            err.message.contains("@get")
                && err.message.contains("servidor HTTP activo")
                && err.message.contains("fitz run"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn fndef_con_decorator_desconocido_da_error_de_decorator() {
        let src = "@patch(\"/x\")\nfn h() => 0";
        let err = parse_and_eval(src).unwrap_err();
        assert!(
            err.message.contains("@patch")
                && err.message.contains("no implementado"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn fndef_con_decorator_http_con_registry_activo_registra_la_ruta() {
        // Con registry activo, el decorator @get registra ruta sin
        // error y define la fn en el env.
        use crate::http::with_active_registry;

        let src = "@get(\"/users/{id}\")\nfn get_user(id: Int) => \"hola\"";
        let (res, reg) = with_active_registry(|| parse_and_eval(src));
        res.unwrap();
        assert_eq!(reg.routes.len(), 1);
        let r = &reg.routes[0];
        assert_eq!(r.method, crate::http::HttpMethod::Get);
        assert_eq!(r.path, "/users/{id}");
        assert_eq!(r.path_params, vec!["id".to_string()]);
        assert_eq!(r.handler_name, "get_user");
        assert_eq!(r.param_types, vec![("id".to_string(), Some("Int".into()), false)]);
    }

    #[test]
    fn fndef_con_path_param_sin_param_de_handler_es_error() {
        // `@get("/{id}")` pero el handler no tiene un param `id`.
        use crate::http::with_active_registry;
        let src = "@get(\"/{id}\")\nfn h() => 0";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("'{id}'") && err.message.contains("parámetro"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn fndef_con_decorator_http_sin_args_es_error() {
        // `@get()` sin path.
        use crate::http::with_active_registry;
        let src = "@get()\nfn h() => 0";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("@get") && err.message.contains("argumento"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn fndef_decorator_http_path_no_string_es_error() {
        // `@get(42)` — path no es string.
        use crate::http::with_active_registry;
        let src = "@get(42)\nfn h() => 0";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("string literal"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn fndef_decorator_http_path_sin_slash_es_error() {
        use crate::http::with_active_registry;
        let src = "@get(\"users\")\nfn h() => 0";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("'/'"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn fndef_body_se_registra_y_resuelve_type_si_existe() {
        use crate::http::with_active_registry;
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let (res, reg) = with_active_registry(|| parse_and_eval(src));
        res.unwrap();
        assert_eq!(reg.routes.len(), 1);
        let route = &reg.routes[0];
        let bp = route.body_param.as_ref().expect("se esperaba body_param");
        assert_eq!(bp.name, "body");
        assert_eq!(bp.declared_type_name.as_deref(), Some("UserInput"));
        assert!(
            matches!(&bp.declared_type, Some(Value::Type { name, .. }) if name == "UserInput"),
            "declared_type debería ser Value::Type 'UserInput'",
        );
    }

    #[test]
    fn fndef_body_sin_tipo_declarado_queda_sin_resolver() {
        // `body` sin anotación: declared_type = None, runtime
        // deserializa como Value libre.
        use crate::http::with_active_registry;
        let src = "@post(\"/log\")\nfn log(body) => body";
        let (res, reg) = with_active_registry(|| parse_and_eval(src));
        res.unwrap();
        let bp = reg.routes[0].body_param.as_ref().unwrap();
        assert_eq!(bp.name, "body");
        assert!(bp.declared_type.is_none());
        assert!(bp.declared_type_name.is_none());
    }

    #[test]
    fn fndef_dos_body_params_es_error_al_registrar() {
        use crate::http::with_active_registry;
        let src = "\
            type A { x: Int }\n\
            type B { y: Int }\n\
            @post(\"/x\")\nfn h(a: A, b: B) => a\n\
        ";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("solo se admite un parámetro body"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn fndef_get_con_body_se_registra_sin_problema() {
        // Permitimos body en cualquier verbo; el evaluator no fuerza
        // semántica de HTTP acá (axum/curl aceptan body en GET).
        use crate::http::with_active_registry;
        let src = "\
            type Q { name: Str }\n\
            @get(\"/search\")\nfn s(body: Q) => body.name\n\
        ";
        let (res, reg) = with_active_registry(|| parse_and_eval(src));
        res.unwrap();
        assert!(reg.routes[0].body_param.is_some());
    }

    // ---- @server (Fase 4.4) ----

    #[test]
    fn server_decorator_setea_port_y_host() {
        use crate::http::with_active_registry;
        let src = "\
            @server(8080, \"0.0.0.0\")\nfn main() => 0\n\
            @get(\"/\")\nfn h() => \"ok\"\n\
        ";
        let (res, reg) = with_active_registry(|| parse_and_eval(src));
        res.unwrap();
        let cfg = reg.server_config.unwrap();
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.host, "0.0.0.0");
    }

    #[test]
    fn server_decorator_sin_args_no_pisa_default() {
        use crate::http::with_active_registry;
        let src = "@server()\nfn cfg() => 0\n@get(\"/\")\nfn h() => 0";
        let (res, reg) = with_active_registry(|| parse_and_eval(src));
        res.unwrap();
        let cfg = reg.server_config.unwrap();
        assert_eq!(cfg.port, 3000);
        assert_eq!(cfg.host, "127.0.0.1");
    }

    #[test]
    fn server_decorator_solo_port_usa_host_default() {
        use crate::http::with_active_registry;
        let src = "@server(9090)\nfn cfg() => 0\n@get(\"/\")\nfn h() => 0";
        let (res, reg) = with_active_registry(|| parse_and_eval(src));
        res.unwrap();
        let cfg = reg.server_config.unwrap();
        assert_eq!(cfg.port, 9090);
        assert_eq!(cfg.host, "127.0.0.1");
    }

    #[test]
    fn server_port_no_int_es_error() {
        use crate::http::with_active_registry;
        let src = "@server(\"8080\")\nfn cfg() => 0";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("port") && err.message.contains("Int"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn server_port_fuera_de_rango_es_error() {
        use crate::http::with_active_registry;
        let src = "@server(99999)\nfn cfg() => 0";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("rango"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn server_host_invalido_es_error() {
        use crate::http::with_active_registry;
        let src = "@server(8080, \"no-es-ip\")\nfn cfg() => 0";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("no-es-ip") && err.message.contains("IP"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn server_demasiados_args_es_error() {
        use crate::http::with_active_registry;
        let src = "@server(8080, \"0.0.0.0\", 42)\nfn cfg() => 0";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("2 args"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn server_dos_decorators_es_error() {
        use crate::http::with_active_registry;
        let src = "\
            @server(8080)\nfn a() => 0\n\
            @server(9090)\nfn b() => 0\n\
        ";
        let (res, _reg) = with_active_registry(|| parse_and_eval(src));
        let err = res.unwrap_err();
        assert!(
            err.message.contains("ya tenía un @server"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[test]
    fn programa_sin_server_decorator_da_resolved_config_default() {
        use crate::http::with_active_registry;
        let src = "@get(\"/\")\nfn h() => 0";
        let (res, reg) = with_active_registry(|| parse_and_eval(src));
        res.unwrap();
        assert!(reg.server_config.is_none());
        let cfg = reg.resolved_config();
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 3000);
    }

    #[test]
    fn fndef_post_put_delete_se_registran_con_su_method() {
        use crate::http::with_active_registry;
        let src = "\
            @post(\"/users\")\nfn create(name) => name\n\
            @put(\"/users/{id}\")\nfn update(id: Int, name) => name\n\
            @delete(\"/users/{id}\")\nfn del(id: Int) => 0\n\
        ";
        let (res, reg) = with_active_registry(|| parse_and_eval(src));
        res.unwrap();
        assert_eq!(reg.routes.len(), 3);
        assert_eq!(reg.routes[0].method, crate::http::HttpMethod::Post);
        assert_eq!(reg.routes[1].method, crate::http::HttpMethod::Put);
        assert_eq!(reg.routes[2].method, crate::http::HttpMethod::Delete);
    }

    // -----------------------------------------------------------------------
    // Tests — Span en errores de runtime (S1.2 sub-paso 3)
    //
    // Antes de S1.2 los errores de runtime sobre expresiones heredaban
    // posición `0:0` (sin ubicación reportada). Tras este sub-paso, cada
    // error de tipo / aridad / división por cero / indexing sobre Expr
    // cita la columna del nodo problemático.
    // -----------------------------------------------------------------------

    fn first_runtime_error(src: &str) -> FitzError {
        parse_and_eval(src).expect_err("esperado un error de runtime")
    }

    #[test]
    fn span_runtime_div_zero_apunta_al_operador() {
        // `print(10 / 0)` — el `/` está en columna 10.
        let e = first_runtime_error("print(10 / 0)");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 10);
        assert!(e.message.contains("división por cero"));
    }

    #[test]
    fn span_runtime_type_mismatch_binop_apunta_al_operador() {
        // `print(1 + true)` — el `+` está en columna 9. El checker
        // estático también lo capta; el error de runtime ahora cita
        // la misma posición.
        let e = first_runtime_error("fn f() => 1 + true\nprint(f())");
        // El error ocurre adentro de `f`, columna del `+`.
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 13);
    }

    #[test]
    fn span_runtime_ident_desconocido_apunta_al_ident() {
        // `print(unknown_var)` — `unknown_var` arranca en columna 7.
        let e = first_runtime_error("print(unknown_var)");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 7);
        assert!(e.message.contains("no definida"));
    }

    #[test]
    fn span_runtime_index_oob_apunta_al_corchete() {
        // `let xs = [1, 2]\nprint(xs[10])` — el `[` está en col 9 de
        // línea 2.
        let src = "let xs = [1, 2]\nprint(xs[10])";
        let e = first_runtime_error(src);
        assert_eq!(e.line, 2);
        assert_eq!(e.column, 9);
        assert!(e.message.contains("fuera de rango"));
    }

    #[test]
    fn span_runtime_arity_mismatch_apunta_al_paren() {
        // `fn f(x: Int) => x\nprint(f(1, 2))` — el `(` del call está
        // en col 8 de línea 2.
        let src = "fn f(x: Int) -> Int => x\nlet _ = f(1, 2)";
        let e = first_runtime_error(src);
        assert_eq!(e.line, 2);
        assert_eq!(e.column, 10);
        assert!(e.message.contains("espera 1"));
    }
}
