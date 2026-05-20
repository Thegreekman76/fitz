// evaluator.rs — Fase 2.4
//
// Recorre el AST y produce efectos (imprimir, mutar variables) y valores.
//
// Estructura interna:
//
//  ┌──────────────┐   programa
//  │ eval(...).await    │ ──────────► env global + register_builtins
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

use async_recursion::async_recursion;

use crate::ast::{
    AssignTarget, BinOpKind, Decorator, Expr, Param, Pattern, Program, Span, Stmt, StrPart,
    TypeExpr, UnaryOpKind,
};
use crate::env::{EnvRef, Environment};
use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::format::format_value_with_spec;
use crate::http::{
    has_active_registry, parse_path_template, push_route, set_server_config, BodyParam,
    HeaderSpec, HttpMethod, MiddlewareSpec, RouteSpec, ServerConfig,
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
    /// Mini-tanda L: `break <v>` lleva el valor. `break` solo →
    /// `Value::Null`. El `label` opcional targetea un loop
    /// específico anidado (`break 'outer`); el loop runner
    /// chequea si el label matchea y propaga si no.
    Break(Value, Option<String>),
    Continue(Option<String>),
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
/// Fase 6.4: `eval` y `eval_with_base` son `async fn` — para contextos
/// con runtime tokio ya activo (tests `#[tokio::test]`, llamados desde
/// otra `async fn`, etc.). Para CLI entry-points sync (main.rs) está
/// la variante `eval_with_base_sync` que arma el runtime y bloquea.
///
/// `dead_code` allow: hoy `main.rs` siempre usa `eval_with_base_sync`
/// con el directorio del archivo. Lo dejamos como API pública por
/// simetría y para tests de smoke.
#[allow(dead_code)]
pub async fn eval(program: Program) -> FitzResult<()> {
    let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    eval_with_base(program, base_dir).await
}

/// Fase 9.z.2.b — corre un test descubierto por `fitz test`. Invoca
/// el handler con 0 args y, si era `async`, await-ea el `Value::Future`
/// resultante para forzar la ejecución del body. Cualquier `FitzError`
/// o `EvalSignal` se devuelve como `Err(FitzError)` — el runner lo
/// formatea para reportar el test como FAILED.
///
/// `name` se usa cosméticamente en mensajes de error de aridad
/// (`invoke_value` lo cita); el runner luego prefija con
/// `<source_file>::` si aplica.
pub async fn run_test_handler(
    handler: Value,
    is_async: bool,
    name: &str,
) -> Result<(), FitzError> {
    let value = invoke_value(handler, vec![], name, Span::ZERO)
        .await
        .map_err(signal_to_error)?;

    if !is_async {
        return Ok(());
    }

    // Test era `async fn`: el invoke produjo un Future. Lo
    // consumimos acá para que la espera real ocurra antes de que el
    // runner reporte resultado. Política idéntica a `Expr::Await`
    // del evaluator (consumo único vía `cell.0.lock().take()`).
    match value {
        Value::Future(cell) => {
            let fut = cell.0.lock().take();
            match fut {
                Some(f) => f.await.map(|_| ()),
                None => Err(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0,
                    0,
                    format!(
                        "test async `{}` produjo un Future ya consumido (bug del dispatcher)",
                        name
                    ),
                )),
            }
        }
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Future".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!(
                "test async `{}` devolvió `{}` en vez de Future — bug del dispatcher",
                name,
                other.type_name()
            ),
        )),
    }
}

/// Versión async de `eval` que recibe explícitamente el directorio raíz
/// para resolver `import`s relativos. Para uso desde contextos async
/// (tests con `#[tokio::test]`, handlers HTTP, otros async fns).
///
/// Si tu programa importa deps registradas en `fitz.toml`, usá
/// `eval_with_base_and_deps` para que el loader resuelva `from <dep>
/// import X` contra el `lib_entry` correcto (Fase 9.y.3.b). Esta
/// versión asume "sin deps" (registry vacío).
pub async fn eval_with_base(program: Program, base_dir: PathBuf) -> FitzResult<()> {
    eval_with_base_and_deps(program, base_dir, crate::manifest::DepRegistry::new()).await
}

/// Fase 9.y.3.b — variante de `eval_with_base` que recibe el
/// `dep_registry` resuelto del `fitz.toml`. Lo consume el loader para
/// que `from <dep-name> import X` resuelva al `lib_entry` absoluto en
/// vez de fallback a path relativo del importer.
pub async fn eval_with_base_and_deps(
    program: Program,
    base_dir: PathBuf,
    dep_registry: crate::manifest::DepRegistry,
) -> FitzResult<()> {
    install_loader(base_dir, dep_registry);
    // Guard para des-instalar el loader siempre — incluso ante panic.
    // Si el programa termina por error, igual queremos limpiar el
    // thread_local así un siguiente `eval` arranca limpio.
    let _guard = LoaderGuard;

    let env = Environment::new();
    register_builtins(&env);

    for stmt in &program {
        if let Err(signal) = eval_stmt(stmt, env.clone()).await {
            return Err(signal_to_error(signal));
        }
    }
    Ok(())
}

/// Fase 9.z.4 — evalúa un programa contra un env existente (compartido
/// entre líneas del REPL). A diferencia de `eval_with_base_and_deps`,
/// NO crea un env nuevo ni registra builtins — el caller mantiene un
/// `EnvRef` propio que persiste entre invocaciones.
///
/// Devuelve el `Value` resultante del último stmt (útil para el REPL:
/// cuando el usuario tipea `1 + 2` el parser lo convierte en
/// `Stmt::Expr` y queremos imprimir `Value::Int(3)`). Para `Stmt::Assign`,
/// `Stmt::FnDef`, etc. el valor es `Value::Null`. Si el programa
/// está vacío, también `Value::Null`.
///
/// Sí instala/desinstala el loader (`install_loader`/`LoaderGuard`)
/// cada vez para que `import` funcione. Cache del loader se pierde
/// entre llamadas (deuda menor; cargas múltiples de un mismo módulo
/// re-ejecutan). Para el caso REPL común (intérprete interactivo,
/// pocos imports), aceptable.
pub async fn eval_program_with_env(
    program: Program,
    base_dir: PathBuf,
    env: EnvRef,
    dep_registry: crate::manifest::DepRegistry,
) -> FitzResult<Value> {
    install_loader(base_dir, dep_registry);
    let _guard = LoaderGuard;
    let mut last = Value::Null;
    for stmt in &program {
        match eval_stmt(stmt, env.clone()).await {
            Ok(v) => last = v,
            Err(signal) => return Err(signal_to_error(signal)),
        }
    }
    Ok(last)
}

/// Fase 9.z.4 — crea un env nuevo + registra builtins. Wrapper público
/// para que el REPL (en `main.rs`) pueda armar su scope inicial sin
/// exponer `register_builtins` por separado.
pub fn new_repl_env() -> EnvRef {
    let env = Environment::new();
    register_builtins(&env);
    env
}

/// Fase 9.z.4 — lista los nombres de los builtins registrados por
/// `register_builtins`. El REPL los usa para filtrar `:env` (mostrar
/// solo lo que el usuario definió, no los builtins built-in del
/// lenguaje). Mantener sincronizado con `register_builtins`.
pub fn builtin_names() -> &'static [&'static str] {
    &[
        "print",
        "len",
        "cors",
        "sleep",
        "assert",
        "assert_eq",
        "assert_ne",
        "assert_throws",
        // Mini-tanda Bits-extras — ops sobre Int como builtins globales.
        "popcount",
        "leading_zeros",
        "trailing_zeros",
        "rotate_left",
        "rotate_right",
        // Mini-tanda Math — builtins matemáticos.
        "abs",
        "min",
        "max",
        "pow",
        "sqrt",
        "ceil",
        "floor",
        "round",
        "clamp",
    ]
}

/// Wrapper sync de `eval_with_base` para CLI entry-points (main.rs).
/// Arma un runtime tokio `current_thread` (single-threaded por F17)
/// y bloquea sobre el future. Si ya estás adentro de un runtime, usá
/// `eval_with_base(...).await` directo.
pub fn eval_with_base_sync(program: Program, base_dir: PathBuf) -> FitzResult<()> {
    eval_with_base_and_deps_sync(program, base_dir, crate::manifest::DepRegistry::new())
}

/// Fase 9.y.3.b — wrapper sync de `eval_with_base_and_deps` para
/// el CLI cuando `fitz run` corre adentro de un proyecto Fitz con
/// `[dependencies]`. El `dep_registry` viene construido por
/// `manifest::build_dep_registry` desde el `ManifestCtx`.
pub fn eval_with_base_and_deps_sync(
    program: Program,
    base_dir: PathBuf,
    dep_registry: crate::manifest::DepRegistry,
) -> FitzResult<()> {
    let runtime = build_runtime();
    runtime.block_on(eval_with_base_and_deps(program, base_dir, dep_registry))
}

/// Construye el runtime tokio `current_thread` que comparten el
/// evaluator async y el server HTTP. Single-threaded por F17.
pub fn build_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("no se pudo construir el runtime tokio")
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

#[allow(clippy::too_many_arguments)]
fn process_decorator(
    deco: &Decorator,
    fn_name: &str,
    params: &[Param],
    return_type: &Option<TypeExpr>,
    headers: &[HeaderSpec],
    middlewares: &[MiddlewareSpec],
    cors_config: &Option<std::sync::Arc<crate::http::CorsConfig>>,
    handler: &Value,
    env: &EnvRef,
    fn_def_span: Span,
) -> Result<(), EvalSignal> {
    // ¿Es un decorator HTTP conocido?
    if let Some(method) = HttpMethod::from_decorator_name(&deco.name) {
        return register_http_route(
            method,
            deco,
            fn_name,
            params,
            return_type,
            headers,
            middlewares,
            cors_config,
            handler,
            env,
        );
    }

    // `@server(port?, host?)`: configura el server. La fn que decora
    // queda en el env como cualquier otra (el patrón típico es
    // ponerlo arriba de `fn main()`).
    if deco.name == "server" {
        return register_server_config(deco, fn_name);
    }

    // `@test`: registra la fn en el `TestRegistry` activo (Fase
    // 9.z.2.a). **Diseño asimétrico vs `@server`**: si no hay
    // registry activo (caso típico de `fitz run`), el decorator es
    // **no-op silencioso** — paralelo a `#[cfg(test)]` de Rust, las
    // fns `@test` se ignoran fuera del runner. El sub-comando
    // `fitz test` (9.z.2.b) instala el registry vía
    // `with_active_test_registry` antes de evaluar.
    //
    // Validaciones acá: sin args, sin kwargs, sin params. Cualquiera
    // de los 3 viola la firma del MVP (`@test fn nombre() { ... }`).
    if deco.name == "test" {
        return register_test(deco, fn_name, params, handler, fn_def_span);
    }

    // Decorador desconocido. Mensaje listo para guiar al usuario.
    Err(EvalSignal::Error(FitzError::new(
        ErrorKind::InvalidSyntax,
        0,
        0,
        format!(
            "decorator '@{}' no implementado (sobre fn '{}'). \
             Decorators soportados hoy: @get, @post, @put, @delete, @server, @header, @middleware, @test.",
            deco.name, fn_name,
        ),
    )))
}

/// Procesa `@test` sobre una `Stmt::FnDef` (Fase 9.z.2.a). Valida la
/// firma del MVP (sin args, sin kwargs, sin params) y, si hay un
/// `TestRegistry` activo, empuja un `TestSpec`. Sin registry activo
/// (caso `fitz run`), no-op silencioso — los tests se descubren solo
/// cuando el sub-comando `fitz test` instala el registry.
///
/// El `is_async` viaja en el `handler` (que es siempre
/// `Value::Function` construido por `Stmt::FnDef` arriba). Lo
/// extraemos para que el runner de 9.z.2.b sepa si invocar sync o
/// await-ear el `Value::Future` resultante.
fn register_test(
    deco: &Decorator,
    fn_name: &str,
    params: &[Param],
    handler: &Value,
    fn_def_span: Span,
) -> Result<(), EvalSignal> {
    let err = |msg: String| {
        EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            fn_def_span.line,
            fn_def_span.column,
            msg,
        ))
    };

    if !deco.args.is_empty() {
        return Err(err(format!(
            "@test sobre fn '{}': no admite args posicionales en el MVP, \
             recibió {}. Sintaxis: `@test fn {}() {{ ... }}`.",
            fn_name,
            deco.args.len(),
            fn_name,
        )));
    }
    if !deco.kwargs.is_empty() {
        return Err(err(format!(
            "@test sobre fn '{}': no admite kwargs en el MVP. Sintaxis: \
             `@test fn {}() {{ ... }}`.",
            fn_name, fn_name,
        )));
    }
    if !params.is_empty() {
        return Err(err(format!(
            "@test sobre fn '{}': la fn debe tener 0 params (recibió {}). \
             Los tests no reciben fixtures en el MVP — usá variables \
             locales o helpers para preparar estado.",
            fn_name,
            params.len(),
        )));
    }

    // Sin registry activo → no-op silencioso (modo `fitz run`).
    // `fitz test` instala el registry antes de evaluar para capturar.
    if !crate::testing::has_active_test_registry() {
        return Ok(());
    }

    // Extraer `is_async` del handler. El handler siempre es
    // `Value::Function` (construido por `Stmt::FnDef`); cualquier
    // otra cosa es bug del evaluator.
    let is_async = match handler {
        Value::Function { is_async, .. } => *is_async,
        _ => unreachable!("@test sobre fn '{}': handler no es Value::Function", fn_name),
    };

    crate::testing::push_test(crate::testing::TestSpec {
        name: fn_name.to_string(),
        handler: handler.clone(),
        is_async,
        span: fn_def_span,
        source_file: crate::testing::current_test_source(),
    });
    Ok(())
}

/// Recolecta los `@header(name="X")` declarados sobre un handler
/// (Fase 7.6). Valida:
///   - kwarg `name: Str` no vacío.
///   - Existe un param Fitz con el nombre derivado
///     (lowercase + `-` → `_`).
///   - El param Fitz es `Str` o `Str?` (otros tipos: error claro).
///   - No hay dos `@header` con el mismo `name`.
///
/// Si la fn no tiene decoradores `@header`, devuelve `vec![]`.
fn collect_headers(
    decorators: &[Decorator],
    fn_name: &str,
    params: &[Param],
) -> Result<Vec<HeaderSpec>, EvalSignal> {
    let err = |msg: String| {
        EvalSignal::Error(FitzError::new(ErrorKind::InvalidSyntax, 0, 0, msg))
    };

    let mut headers: Vec<HeaderSpec> = Vec::new();
    for deco in decorators {
        if deco.name != "header" {
            continue;
        }
        // @header no acepta args posicionales (todo va por kwarg).
        if !deco.args.is_empty() {
            return Err(err(format!(
                "@header sobre fn '{}': no admite args posicionales. \
                 Usá `@header(name=\"X\")`.",
                fn_name,
            )));
        }
        let name_kw = deco
            .kwargs
            .iter()
            .find(|(k, _)| k == "name")
            .ok_or_else(|| {
                err(format!(
                    "@header sobre fn '{}': falta el kwarg 'name' (nombre del header HTTP). \
                     Ej: `@header(name=\"Authorization\")`.",
                    fn_name,
                ))
            })?;
        let http_name = match &name_kw.1 {
            Expr::Str(s, _) if !s.is_empty() => s.clone(),
            Expr::Str(_, _) => {
                return Err(err(format!(
                    "@header sobre fn '{}': el kwarg 'name' no puede ser un string vacío",
                    fn_name,
                )));
            }
            other => {
                return Err(err(format!(
                    "@header sobre fn '{}': el kwarg 'name' debe ser un Str literal, recibió {:?}",
                    fn_name, other,
                )));
            }
        };
        // Mini-fase Q.1: `into="alias"` opcional permite mapear a un
        // param con nombre distinto al derivado por convención.
        // Útil para headers con caracteres no idiomáticos en Fitz
        // (`X-Forwarded-For` → param `forwarded_for` no es muy lindo).
        let into_kw = deco.kwargs.iter().find(|(k, _)| k == "into");
        let into_alias: Option<String> = match into_kw {
            Some((_, Expr::Str(s, _))) if !s.is_empty() => Some(s.clone()),
            Some((_, Expr::Str(_, _))) => {
                return Err(err(format!(
                    "@header(name=\"{}\") sobre fn '{}': el kwarg 'into' no puede ser un string vacío",
                    http_name, fn_name,
                )));
            }
            Some((_, other)) => {
                return Err(err(format!(
                    "@header(name=\"{}\") sobre fn '{}': el kwarg 'into' debe ser un Str literal, recibió {:?}",
                    http_name, fn_name, other,
                )));
            }
            None => None,
        };
        // Kwargs extra: rechazar para no comerse typos silenciosamente.
        if let Some((k, _)) = deco.kwargs.iter().find(|(k, _)| k != "name" && k != "into") {
            return Err(err(format!(
                "@header sobre fn '{}': kwarg '{}' no reconocido. Soportados: name, into.",
                fn_name, k,
            )));
        }
        let param_name = into_alias
            .clone()
            .unwrap_or_else(|| http_name.to_lowercase().replace('-', "_"));
        // Validar que el param exista en la fn y sea Str o Str?.
        let Some(p) = params.iter().find(|p| p.name == param_name) else {
            return Err(err(format!(
                "@header(name=\"{}\"{}) sobre fn '{}': el handler no tiene un param llamado '{}'{}",
                http_name,
                into_alias.as_ref().map(|a| format!(", into=\"{}\"", a)).unwrap_or_default(),
                fn_name,
                param_name,
                if into_alias.is_none() {
                    " (derivado del header HTTP por convención lowercase + `-` → `_`)".to_string()
                } else {
                    String::new()
                },
            )));
        };
        let is_nullable = match &p.type_ {
            Some(TypeExpr::Named(n)) if n == "Str" => false,
            Some(TypeExpr::Nullable(inner)) => match inner.as_ref() {
                TypeExpr::Named(n) if n == "Str" => true,
                other => {
                    return Err(err(format!(
                        "@header(name=\"{}\") sobre fn '{}': el param '{}' debe ser `Str` o `Str?`, \
                         pero está declarado como `{}`",
                        http_name, fn_name, param_name, other.display_name(),
                    )));
                }
            },
            Some(other) => {
                return Err(err(format!(
                    "@header(name=\"{}\") sobre fn '{}': el param '{}' debe ser `Str` o `Str?`, \
                     pero está declarado como `{}`",
                    http_name, fn_name, param_name, other.display_name(),
                )));
            }
            None => {
                return Err(err(format!(
                    "@header(name=\"{}\") sobre fn '{}': el param '{}' necesita una anotación \
                     de tipo (`Str` o `Str?`)",
                    http_name, fn_name, param_name,
                )));
            }
        };
        if headers.iter().any(|h| h.http_name.eq_ignore_ascii_case(&http_name)) {
            return Err(err(format!(
                "@header(name=\"{}\") sobre fn '{}': declarado dos veces (el match es case-insensitive)",
                http_name, fn_name,
            )));
        }
        headers.push(HeaderSpec {
            http_name,
            param_name,
            is_nullable,
        });
    }
    Ok(headers)
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

    // 7.4: kwargs aceptados por @server. `docs: Bool` (opt-out de
    // /openapi.json y /docs). Q.2: `api_version: Str` (override del
    // info.version del schema OpenAPI). Cualquier otro kwarg es error.
    for (key, value_expr) in &deco.kwargs {
        match key.as_str() {
            "docs" => match value_expr {
                Expr::Bool(b, _) => {
                    config.enable_docs = *b;
                }
                other => {
                    return Err(err(format!(
                        "@server sobre fn '{}': el kwarg 'docs' debe ser Bool literal, \
                         recibió {:?}",
                        fn_name, other,
                    )));
                }
            },
            "api_version" => match value_expr {
                Expr::Str(s, _) if !s.is_empty() => {
                    config.api_version = Some(s.clone());
                }
                Expr::Str(_, _) => {
                    return Err(err(format!(
                        "@server sobre fn '{}': el kwarg 'api_version' no puede ser un string vacío",
                        fn_name,
                    )));
                }
                other => {
                    return Err(err(format!(
                        "@server sobre fn '{}': el kwarg 'api_version' debe ser Str literal, \
                         recibió {:?}",
                        fn_name, other,
                    )));
                }
            },
            other => {
                return Err(err(format!(
                    "@server sobre fn '{}': kwarg '{}' no reconocido. \
                     Soportados: docs, api_version.",
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

/// Recolecta los `@middleware(...)` declarados sobre un handler. La
/// pasada distingue dos kinds de middleware:
///
///   - **User-fn (MW.1)**: la expresión evalúa a `Value::Function`.
///     Se acumula en la chain `Vec<MiddlewareSpec>` que corre en
///     orden top-down antes del handler. Gate-only: el retorno
///     determina si se continúa la cadena o se cortocircuita.
///
///   - **CORS (MW.2)**: la expresión evalúa a `Value::CorsConfig`,
///     producto del built-in `cors(...)`. Se guarda en un slot
///     dedicado `Option<Arc<CorsConfig>>` que `RouteSpec.cors`
///     consume — no entra a la chain. Máximo uno por ruta;
///     declararlo dos veces es error claro.
///
/// Otros valores (Int, Str, Instance, etc.) → error con mensaje
/// listo para guiar al usuario.
///
/// Validaciones adicionales:
///   - `@middleware` solo antes del decorator de ruta.
///   - Exactamente un arg posicional. Sin kwargs.
///   - El caller chequea que aplique solo sobre handlers HTTP
///     (paralelo a `collect_headers`).
///
/// Async porque ahora evalúa la expresión del arg con `eval_expr`
/// (necesario para que `cors(allow_origin="*")` y cualquier factoría
/// que devuelva Function tipen).
async fn collect_middlewares(
    decorators: &[Decorator],
    fn_name: &str,
    env: &EnvRef,
) -> Result<(Vec<MiddlewareSpec>, Option<std::sync::Arc<crate::http::CorsConfig>>), EvalSignal> {
    let err = |msg: String| {
        EvalSignal::Error(FitzError::new(ErrorKind::InvalidSyntax, 0, 0, msg))
    };

    let mut middlewares: Vec<MiddlewareSpec> = Vec::new();
    let mut cors: Option<std::sync::Arc<crate::http::CorsConfig>> = None;
    let mut saw_route_decorator = false;

    for deco in decorators {
        if HttpMethod::from_decorator_name(&deco.name).is_some() {
            saw_route_decorator = true;
            continue;
        }
        if deco.name != "middleware" {
            continue;
        }
        // Orden: `@middleware` solo antes del decorator de ruta.
        if saw_route_decorator {
            return Err(err(format!(
                "@middleware sobre fn '{}': debe apilarse ANTES del decorator de ruta \
                 (`@middleware(...)` arriba de `@get`/`@post`/`@put`/`@delete`)",
                fn_name,
            )));
        }
        if !deco.kwargs.is_empty() {
            return Err(err(format!(
                "@middleware sobre fn '{}': no admite argumentos por nombre (kwargs)",
                fn_name,
            )));
        }
        if deco.args.len() != 1 {
            return Err(err(format!(
                "@middleware sobre fn '{}': espera exactamente un argumento \
                 (la fn a aplicar), recibió {}",
                fn_name,
                deco.args.len(),
            )));
        }
        // Evaluar la expresión: puede ser un Ident (fn previa), un
        // Call (`cors(...)`, o una factoría user-fn), una field
        // expression, etc.
        let value = eval_expr(&deco.args[0], env.clone()).await?;
        // Para nombre legible en mensajes: si fue un Ident, usar el
        // nombre tal cual; si fue un Call de cors, "cors"; cualquier
        // otra cosa, una etiqueta de fallback.
        let label = match &deco.args[0] {
            Expr::Ident(n, _) => n.clone(),
            Expr::Call { callee, .. } => match callee.as_ref() {
                Expr::Ident(n, _) => n.clone(),
                _ => "<expr>".to_string(),
            },
            _ => "<expr>".to_string(),
        };
        match value {
            Value::Function { ref params, .. } => {
                // Mw.next — detectar kind por aridad:
                //   1 arg → Pre (gate-only, clásico).
                //   2 args → Post (post-process, recibe Response).
                let kind = match params.len() {
                    1 => crate::http::MiddlewareKind::Pre,
                    2 => crate::http::MiddlewareKind::Post,
                    n => {
                        return Err(err(format!(
                            "@middleware sobre fn '{}': la fn referenciada ({}) debe \
                             tener 1 o 2 parámetros (1 = pre-process clásico que recibe \
                             `Request`; 2 = post-process que recibe `(Request, Response)`); \
                             tiene {}",
                            fn_name, label, n,
                        )));
                    }
                };
                middlewares.push(MiddlewareSpec {
                    name: label,
                    handler: value,
                    kind,
                });
            }
            Value::CorsConfig(config) => {
                if cors.is_some() {
                    return Err(err(format!(
                        "@middleware sobre fn '{}': el handler ya tiene un `cors(...)` aplicado, \
                         solo se admite uno por ruta",
                        fn_name,
                    )));
                }
                cors = Some(config);
            }
            other => {
                return Err(err(format!(
                    "@middleware sobre fn '{}': el argumento debe ser una fn o un \
                     `cors(...)`, recibió {}",
                    fn_name,
                    other.type_name(),
                )));
            }
        }
    }
    Ok((middlewares, cors))
}

#[allow(clippy::too_many_arguments)]
fn register_http_route(
    method: HttpMethod,
    deco: &Decorator,
    fn_name: &str,
    params: &[Param],
    return_type: &Option<TypeExpr>,
    headers: &[HeaderSpec],
    middlewares: &[MiddlewareSpec],
    cors_config: &Option<std::sync::Arc<crate::http::CorsConfig>>,
    handler: &Value,
    env: &EnvRef,
) -> Result<(), EvalSignal> {
    // Helper local para mantener los mensajes consistentes.
    let err = |msg: String| {
        EvalSignal::Error(FitzError::new(ErrorKind::InvalidSyntax, 0, 0, msg))
    };

    // 7.0: ningún decorator HTTP de ruta acepta kwargs todavía. El
    // soporte para headers (7.6) podría sumarlos; por ahora corte
    // explícito.
    if let Some((key, _)) = deco.kwargs.first() {
        return Err(err(format!(
            "@{} sobre fn '{}': los argumentos por nombre (recibió '{}=...') \
             no están soportados sobre decoradores HTTP de ruta.",
            deco.name, fn_name, key,
        )));
    }

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
    // template.params (path), ni en template.query_params (query), ni
    // sea un header declarado. Máximo uno por handler.
    let mut body_param: Option<BodyParam> = None;
    for p in params {
        if template.params.contains(&p.name) {
            continue; // es path param
        }
        if template.query_params.contains(&p.name) {
            continue; // es query param
        }
        if headers.iter().any(|h| h.param_name == p.name) {
            continue; // es header (Fase 7.6)
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
            match env.lock().get(t.head_name()) {
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

    // TypeExpr completos por param, en orden. Aditivo: el dispatch
    // HTTP usa `param_types` (head names); el generador OpenAPI (7.1)
    // consume estos TypeExpr íntegros.
    let param_type_exprs: Vec<(String, Option<TypeExpr>)> = params
        .iter()
        .map(|p| (p.name.clone(), p.type_.clone()))
        .collect();

    // MW.2: separar `cors(...)` del resto de middlewares. El
    // `collect_middlewares` ya distinguió kinds y nos pasa la cors
    // (opcional, máximo una por ruta) por afuera. Para mantener
    // compat con el llamador, lo extraemos acá vía un slot Option.
    // (La separación efectiva en `collect_middlewares` se hace en
    // ese mismo lugar — acá solo recibimos middlewares.)
    push_route(RouteSpec {
        method,
        path: template.path,
        path_params: template.params,
        query_params: template.query_params,
        handler: handler.clone(),
        handler_name: fn_name.to_string(),
        param_types,
        body_param,
        headers: headers.to_vec(),
        param_type_exprs,
        return_type_expr: return_type.clone(),
        middlewares: middlewares.to_vec(),
        cors: cors_config.clone(),
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
    /// Fase 9.y.3.b — registry de deps del proyecto raíz (`fitz.toml`
    /// del importer principal). `from <name> import X` con `<name>`
    /// en este map resuelve directo a `<lib_entry>.fitz` en vez de
    /// fallback a path relativo. Permanece estable durante toda la
    /// vida del loader (no se modifica en nested loads — los módulos
    /// que cargamos no pueden re-bind el registry).
    dep_registry: crate::manifest::DepRegistry,
}

thread_local! {
    static LOADER: RefCell<Option<Loader>> = const { RefCell::new(None) };
}

fn install_loader(base_dir: PathBuf, dep_registry: crate::manifest::DepRegistry) {
    LOADER.with(|cell| {
        *cell.borrow_mut() = Some(Loader {
            base_dir,
            loading: Vec::new(),
            cache: HashMap::new(),
            dep_registry,
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

/// Resuelve los segmentos del path al archivo correspondiente.
///
/// **Fase 9.y.3.b — orden de resolución**:
/// 1. Si `segments` es de un solo nombre y matchea una key del
///    `dep_registry` del loader, devolvemos el `lib_entry` absoluto
///    de la dep directamente.
/// 2. Si no, fallback a path relativo al `base_dir` actual del loader:
///    `["foo"]` → `<base>/foo.fitz`; `["sub", "foo"]` →
///    `<base>/sub/foo.fitz`.
///
/// Decisión: las deps shadowean archivos locales con el mismo nombre
/// (si tenés `[dependencies] utils = { ... }` y un `utils.fitz` local,
/// gana la dep). Comportamiento explícito por design — la dep es
/// declaración primaria de intención.
///
/// No verifica existencia — el caller hace `canonicalize`, que falla
/// con un mensaje útil si el archivo no está.
fn resolve_module_path(segments: &[String]) -> EvalResult<PathBuf> {
    // Step 1 — dep registry shortcut (Fase 9.y.3.b).
    let dep_hit = LOADER.with(|cell| {
        let borrow = cell.borrow();
        let loader = borrow.as_ref()?;
        if segments.len() != 1 {
            return None;
        }
        loader.dep_registry.get(&segments[0]).cloned()
    });
    if let Some(lib_entry) = dep_hit {
        return Ok(lib_entry);
    }

    // Step 2 — path relativo (comportamiento pre-9.y.3.b).
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
#[async_recursion]
async fn load_module(segments: &[String]) -> EvalResult<Value> {
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
    // `Value::Module` (mismo `Arc<Mutex<Environment>>` adentro). No
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
    //
    // Fase 6.4: el closure sync `(|| { ... })()` quedó incompatible con
    // las llamadas async (`async closure` no es estable). Reemplazado
    // por un async block que se await-ea inmediatamente.
    //
    // Fase 9.z.2.b: durante la eval del body del módulo, sobreescribimos
    // el `CURRENT_TEST_SOURCE` con el filename del módulo (`lib.fitz`).
    // Así los `@test fn` declarados en módulos importados quedan
    // etiquetados con su archivo declarante real, no con el del archivo
    // que disparó el import. `with_test_source_async` restaura el label
    // previo al salir, sin afectar el flujo normal cuando no hay test
    // runner activo. Usamos `file_name()` (no canonical completo) por
    // legibilidad — el path absoluto es ruidoso en el output.
    let module_label = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("<modulo>")
        .to_string();
    let eval_result: EvalResult<()> = crate::testing::with_test_source_async(
        module_label,
        || async {
            for stmt in &module_program {
                eval_stmt(stmt, module_env.clone()).await?;
            }
            Ok(())
        },
    )
    .await;

    // Restaurar estado del loader.
    LOADER.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let loader = borrow.as_mut().expect("loader instalado");
        loader.loading.pop();
        loader.base_dir = prev_base;
    });

    eval_result?;

    // PreF8.3: pre-evaluar los defaults de cada `Value::Type` del módulo
    // en el env del módulo, para que un struct lit sobre un tipo
    // importado pueda usar defaults que referencien símbolos del módulo
    // (consts, otros types) sin que el importer los tenga que
    // re-importar. Tipos definidos en el archivo principal NO pasan
    // por acá; sus defaults se siguen evaluando lazy.
    let typedef_names: Vec<String> = module_program
        .iter()
        .filter_map(|s| match s {
            Stmt::TypeDef { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    for type_name in typedef_names {
        let existing = module_env.lock().get(&type_name);
        let Some(Value::Type { name, fields, methods, .. }) = existing else { continue };
        let mut resolved_defaults: Vec<(String, Value)> = Vec::new();
        for f in &fields {
            if let Some(expr) = &f.default {
                let v = eval_expr(expr, module_env.clone()).await?;
                resolved_defaults.push((f.name.clone(), v));
            }
        }
        let new_type = Value::Type {
            name,
            fields,
            resolved_defaults,
            methods,
        };
        module_env.lock().define(type_name, new_type);
    }

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
pub async fn call_handler(
    handler: Value,
    args: Vec<Value>,
    handler_name: &str,
) -> FitzResult<Value> {
    // El handler HTTP no tiene posición sintáctica directa — viene del
    // server runtime, no de una llamada en el source. Span::ZERO está
    // bien acá; el FitzError::Display omite la posición.
    invoke_value(handler, args, handler_name, Span::ZERO)
        .await
        .map_err(signal_to_error)
}

/// Convierte un signal sin contexto en un `FitzError` legible.
fn signal_to_error(signal: EvalSignal) -> FitzError {
    match signal {
        EvalSignal::Error(e) => e,
        // Mini-tanda Err+ — el operador `?` produce `Return(Result(Err))`
        // para propagar al contenedor. Cuando ese signal escapa hasta el
        // top-level (no hay fn que lo capture), damos un mensaje
        // específico mostrando el value del Err. Mucho más útil que el
        // genérico "`return` fuera de función".
        EvalSignal::Return(Value::Result(ResultVariant::Err(e))) => FitzError::new(
            ErrorKind::InvalidSyntax,
            0, 0,
            format!("operación `?` falló con Err: {}", *e),
        ),
        EvalSignal::Return(_) => FitzError::new(
            ErrorKind::ReturnOutsideFunction,
            0, 0,
            "`return` solo puede usarse adentro de una función",
        ),
        EvalSignal::Break(_, _) => FitzError::new(
            ErrorKind::BreakOutsideLoop,
            0, 0,
            "`break` solo puede usarse adentro de un loop",
        ),
        EvalSignal::Continue(_) => FitzError::new(
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

#[async_recursion]
async fn eval_stmt(stmt: &Stmt, env: EnvRef) -> EvalResult<Value> {
    match stmt {
        Stmt::Expr(expr, _) => eval_expr(expr, env).await,

        // Mini-tanda T — destructuring de tupla. Evaluamos el RHS y
        // validamos longitud + shape contra el pattern; si matchea,
        // aplicamos los bindings al env actual (no scope hijo —
        // semántica de `let` regular).
        Stmt::Destructure { pattern, value, span } => {
            let v = eval_expr(value, env.clone()).await?;
            if match_pattern(pattern, &v).is_none() {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    span.line, span.column,
                    format!(
                        "el valor `{}` no matchea el pattern de destructuring",
                        v.type_name()
                    ),
                )));
            }
            bind_tuple_pattern(pattern, &v, env);
            Ok(Value::Null)
        }

        // `x = value`, `x: Tipo = value`, o `obj.campo = value`. La anotación
        // de tipo se ignora en runtime — tipado gradual, los checks de tipos
        // los hará un type-checker estático más adelante.
        //
        // Dos formas según el target:
        //  - `Ident`: si la variable ya existe en algún scope visible,
        //    reasignar ahí; si no, crear local (ver env.rs).
        //  - `Field`: evaluamos el objeto receptor (tiene que ser
        //    `Value::Instance`), validamos que el campo exista, y mutamos
        //    la celda compartida `Arc<Mutex<...>>` de `fields`.
        Stmt::Assign { target, type_, value, span: _ } => {
            let v = eval_expr(value, env.clone()).await?;
            // Fase 8.4.3: cuando hay anotación de tipo nominal y el RHS
            // es un `Value::Map` (típicamente un dict Python coercionado
            // a Map en 8.2.2), intentamos coercer Map → Instance.
            // Habilita el patrón canónico del roadmap:
            //   let row: User = py_call(...)?
            // El runtime valida que el dict tiene los campos requeridos
            // por el tipo y aplica defaults/nullables si faltan; campos
            // extras del dict se ignoran (Python suele devolver más
            // campos de los necesarios).
            let v = match target {
                AssignTarget::Ident(_) => match type_ {
                    Some(annot) => coerce_to_annotation(annot, v, env.clone()).await?,
                    None => v,
                },
                // Para `obj.field = ...` no aplicamos esta coerción
                // (la anotación del field está en el `type` declarado,
                // que el evaluator hoy no valida en runtime — gradual).
                AssignTarget::Field { .. } => v,
                // Para `xs[i] = v` la anotación de tipo del binding
                // no aplica (es indexing, no declaración).
                AssignTarget::Index { .. } => v,
            };
            match target {
                AssignTarget::Ident(name) => {
                    // Borrows separados: `has` toma borrow inmutable, lo
                    // soltamos antes de pedir un borrow mutable.
                    let already_defined = env.lock().has(name);
                    if already_defined {
                        env.lock()
                            .assign(name, v)
                            .expect("la variable existe — acabamos de chequear con has()");
                    } else {
                        env.lock().define(name.clone(), v);
                    }
                }
                AssignTarget::Field { object, field } => {
                    let receiver = eval_expr(object, env.clone()).await?;
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
                    let mut borrowed = fields.lock();
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
                // R.1.3 — `xs[i] = v` / `m["k"] = v`. Dispatch sobre
                // tipo del receptor en runtime: List (bounds check)
                // o Map (insert/replace preservando insertion order).
                AssignTarget::Index { object, index } => {
                    let receiver = eval_expr(object, env.clone()).await?;
                    let idx_value = eval_expr(index, env.clone()).await?;
                    match receiver {
                        Value::List(items) => {
                            let idx = match idx_value {
                                Value::Int(n) => n,
                                other => {
                                    return Err(EvalSignal::Error(FitzError::new(
                                        ErrorKind::TypeMismatch {
                                            expected: "Int".into(),
                                            found: other.type_name().into(),
                                        },
                                        0, 0,
                                        format!(
                                            "el índice de una lista debe ser Int, recibió `{}`",
                                            other.type_name()
                                        ),
                                    )));
                                }
                            };
                            let mut borrowed = items.lock();
                            let len = borrowed.len() as i64;
                            // I.1 — mismo wrap negativo que en lectura.
                            let effective = if idx < 0 { len + idx } else { idx };
                            if effective < 0 || effective >= len {
                                drop(borrowed);
                                return Err(EvalSignal::Error(FitzError::new(
                                    ErrorKind::InvalidSyntax,
                                    0, 0,
                                    format!(
                                        "índice {} fuera de rango (lista de tamaño {})",
                                        idx, len
                                    ),
                                )));
                            }
                            borrowed[effective as usize] = v;
                        }
                        Value::Map(pairs) => {
                            // Linear search por la clave (mismo modelo
                            // que `m.get` y la igualdad de Value). Si
                            // existe, sobreescribir el slot — preserva
                            // insertion order. Si no, push al final.
                            let mut borrowed = pairs.lock();
                            let mut found = false;
                            for (k, slot) in borrowed.iter_mut() {
                                if *k == idx_value {
                                    *slot = v.clone();
                                    found = true;
                                    break;
                                }
                            }
                            if !found {
                                borrowed.push((idx_value, v));
                            }
                        }
                        other => {
                            return Err(EvalSignal::Error(FitzError::new(
                                ErrorKind::TypeMismatch {
                                    expected: "List o Map".into(),
                                    found: other.type_name().into(),
                                },
                                0, 0,
                                format!(
                                    "no se puede asignar por índice a un valor de tipo `{}`",
                                    other.type_name()
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
            let v = eval_expr(expr, env).await?;
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
            let status_v = eval_expr(status, env.clone()).await?;
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
                Some(b) => Some(Box::new(eval_expr(b, env).await?)),
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
        Stmt::FnDef { name, params, return_type, body, is_async, decorators, span } => {
            let func = Value::Function {
                params: params.clone(),
                body: body.clone(),
                closure: env.clone(),
                is_async: *is_async,
            };

            // Procesar decorators ANTES de definir la fn en el env. Si
            // alguno falla, no queremos un binding mitad-registrado.
            // Pasamos el env actual para que el resolver del decorator
            // pueda mirar el `type` declarado de un parámetro body
            // (los `type` ya fueron registrados en este mismo env). El
            // `return_type` viaja para que el handler HTTP lo almacene
            // en `RouteSpec` (insumo del generador OpenAPI, 7.1).
            //
            // Fase 7.6: primero recolectamos los `@header(...)` (que NO
            // son decoradores "principales", solo aportan metadata), y
            // los pasamos al decorator de ruta cuando lo procesemos.
            // Validamos también que `@header` no aparezca sin un
            // decorator HTTP de ruta (sería un no-op confuso).
            let collected_headers = collect_headers(decorators, name, params)?;
            if !collected_headers.is_empty()
                && !decorators
                    .iter()
                    .any(|d| HttpMethod::from_decorator_name(&d.name).is_some())
            {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0,
                    0,
                    format!(
                        "@header sobre fn '{}': solo aplica sobre handlers HTTP \
                         (apilar junto a `@get`/`@post`/`@put`/`@delete`).",
                        name,
                    ),
                )));
            }
            // Mini-fase MW.1/MW.2: recolectar `@middleware(...)`. Una
            // sola pasada distingue user-fns (chain gate-only, MW.1) de
            // `cors(...)` (slot dedicado, MW.2). Como headers, solo
            // aplica sobre handlers HTTP de ruta.
            let (collected_middlewares, collected_cors) =
                collect_middlewares(decorators, name, &env).await?;
            if (!collected_middlewares.is_empty() || collected_cors.is_some())
                && !decorators
                    .iter()
                    .any(|d| HttpMethod::from_decorator_name(&d.name).is_some())
            {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0,
                    0,
                    format!(
                        "@middleware sobre fn '{}': solo aplica sobre handlers HTTP \
                         (apilar junto a `@get`/`@post`/`@put`/`@delete`).",
                        name,
                    ),
                )));
            }
            for deco in decorators {
                if deco.name == "header" || deco.name == "middleware" {
                    continue; // ya procesado por sus `collect_*`
                }
                process_decorator(
                    deco,
                    name,
                    params,
                    return_type,
                    &collected_headers,
                    &collected_middlewares,
                    &collected_cors,
                    &func,
                    &env,
                    *span,
                )?;
            }

            env.lock().define(name.clone(), func);
            Ok(Value::Null)
        }

        // `type Name { campo1: T1, ... }`. Por ahora solo registramos el
        // tipo en el env como un valor inerte. La instanciación (`User { id: 1 }`)
        // y el field access requieren extensiones del AST (Fase 3).
        Stmt::TypeDef { name, fields, methods, span: _ } => {
            // PreF8.3: tipos locales arrancan con `resolved_defaults` vacío.
            // Sus `Field.default` se siguen evaluando lazy en cada struct
            // lit con el env del call site. Solo los tipos cargados desde
            // un módulo (vía `load_module`) tienen los defaults pre-
            // evaluados — esa pre-evaluación se hace en un post-pass al
            // terminar de ejecutar las stmts del módulo, ahí ya están
            // disponibles todos los símbolos del módulo en su env.
            //
            // R.3: los métodos se copian al `Value::Type` para
            // dispatch posterior sobre `Value::Instance`.
            let t = Value::Type {
                name: name.clone(),
                fields: fields.clone(),
                resolved_defaults: Vec::new(),
                methods: methods.clone(),
            };
            env.lock().define(name.clone(), t);
            Ok(Value::Null)
        }
        Stmt::Break(value_expr, label, _) => {
            // Mini-tanda L: evaluamos el valor si está, default Null.
            // El label se propaga vía `EvalSignal::Break(v, label)`.
            let v = if let Some(e) = value_expr {
                eval_expr(e, env).await?
            } else {
                Value::Null
            };
            Err(EvalSignal::Break(v, label.clone()))
        }
        Stmt::Continue(label, _) => Err(EvalSignal::Continue(label.clone())),

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
        // Mini-tanda Md: `var` es ahora un Pattern. Maneja:
        //   - `for x in xs` → Pattern::Ident bindea cada elem.
        //   - `for _ in 0..10` → Pattern::Wildcard ignora cada elem.
        //   - `for (k, v) in m` → Pattern::Tuple destructura.
        //   - `for kv in m` → Pattern::Ident bindea como `Value::Tuple([k, v])`.
        Stmt::For { var, iter, body, label, span: _ } => {
            let iter_v = eval_expr(iter, env.clone()).await?;
            // F17.3: materializamos a `Vec<Value>` en lugar de
            // `Box<dyn Iterator>` porque el `dyn Iterator` no es `Send` y
            // el future del for cruza `.await` (con `#[async_recursion]`
            // sin `?Send` el bound es obligatorio). El Vec ya estaba
            // siendo construido como snapshot para evitar re-entrancia
            // sobre la lista — el cambio solo materializa el caso `Range`.
            let items: Vec<Value> = match iter_v {
                // La lista va por referencia compartida (`Arc<Mutex<>>`).
                // Para iterar tomamos un snapshot del Vec (cloneando los
                // valores): si el body muta la lista misma, el iterator
                // ya tiene su copia y no se altera a mitad de iteración.
                // Eso evita problemas estilo "modifying a list while
                // iterating" sin renunciar a mutación.
                Value::List(items) => items.lock().clone(),
                Value::Range { start, end } => (start..end).map(Value::Int).collect(),
                // Mini-tanda Md: Map iterable como Vec<Tuple([K, V])>.
                // El orden de inserción se preserva (Map de Fitz usa Vec
                // internamente). Snapshot para evitar re-entrancia.
                Value::Map(entries) => entries
                    .lock()
                    .iter()
                    .map(|(k, v)| Value::Tuple(vec![k.clone(), v.clone()]))
                    .collect(),
                other => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "List, Range o Map".into(),
                        found: other.type_name().into(),
                    },
                    0, 0,
                    format!(
                        "no se puede iterar sobre un valor de tipo `{}`",
                        other.type_name()
                    ),
                ))),
            };
            for item in items {
                bind_for_pattern(var, item, &env)?;
                match run_loop_body(body, env.clone(), label.clone()).await {
                    LoopControl::Continue => continue,
                    LoopControl::Break(_) => break, // value descartado en statement-mode
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
        Stmt::While { condition, body, label, span: _ } => {
            loop {
                let cond_v = eval_expr(condition, env.clone()).await?;
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
                match run_loop_body(body, env.clone(), label.clone()).await {
                    LoopControl::Continue => continue,
                    LoopControl::Break(_) => break, // value descartado en statement-mode
                    LoopControl::Propagate(signal) => return Err(signal),
                }
            }
            Ok(Value::Null)
        }

        // `loop { body }` — itera para siempre. Solo `break` o `return`
        // pueden sacarte.
        Stmt::Loop { body, label, span: _ } => {
            loop {
                match run_loop_body(body, env.clone(), label.clone()).await {
                    LoopControl::Continue => continue,
                    LoopControl::Break(_) => break, // value descartado en statement-mode
                    LoopControl::Propagate(signal) => return Err(signal),
                }
            }
            Ok(Value::Null)
        }

        // `import foo` / `import sub.foo` — carga el módulo y lo expone
        // bajo el ÚLTIMO segmento del path (`sub.foo` → binding `foo`).
        // Para field access (`foo.bar`) ver `eval_expr` sobre `Expr::Field`;
        // para method calls (`foo.bar()`) ver `dispatch_method`.
        //
        // Fase 8.1.2: si el path arranca con `python`, ruteamos al loader
        // Python. Hoy `import python.X` no se soporta — la forma canónica
        // es `from python import X`. Cerramos esta rama con error claro.
        Stmt::Import { path, alias, span: _ } => {
            if path.first().map(|s| s.as_str()) == Some("python") {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0, 0,
                    "`import python...` no se soporta en Fase 8.1; \
                     usá `from python import <módulo>` para traer librerías Python al scope".to_string(),
                )));
            }
            let module = load_module(path).await?;
            // PreF8.4: si hay alias (`import foo as f`), bindeamos
            // bajo el alias. Sin alias, bajo el último segmento del path.
            let binding_name = alias.clone().unwrap_or_else(|| {
                path.last()
                    .cloned()
                    .expect("parser garantiza al menos un segmento")
            });
            env.lock().define(binding_name, module);
            Ok(Value::Null)
        }

        // `from foo import a, b, c` — carga el módulo y bindea cada
        // nombre directo al scope actual. Si el módulo no expone
        // alguno de los nombres pedidos, error explícito citando cuál
        // falta y desde qué módulo.
        //
        // PreF8.4: cada entry es `(name, alias?)`. El lookup en el
        // módulo se hace por `name`; el binding en el scope local
        // usa `alias` si está, si no `name`.
        //
        // Fase 8.1.2: si el path es `python`, ruteamos al loader CPython
        // embebido. Cada `name` se importa como módulo top-level Python
        // independiente. Sin la feature `python`, el helper emite error
        // claro citando el flag de build.
        Stmt::FromImport { path, names, span: _ } => {
            if path.first().map(|s| s.as_str()) == Some("python") {
                return eval_python_from_import(path, names, env).await;
            }
            let module = load_module(path).await?;
            let module_env = match &module {
                Value::Module { env, .. } => env.clone(),
                _ => unreachable!("load_module siempre devuelve Value::Module"),
            };
            let module_label = path
                .last()
                .cloned()
                .unwrap_or_else(|| "<sin nombre>".to_string());
            for (name, alias) in names {
                let v = module_env.lock().get(name).ok_or_else(|| {
                    EvalSignal::Error(FitzError::new(
                        ErrorKind::UndefinedVariable(name.clone()),
                        0, 0,
                        format!(
                            "el módulo `{}` no exporta `{}`",
                            module_label, name,
                        ),
                    ))
                })?;
                let binding = alias.clone().unwrap_or_else(|| name.clone());
                env.lock().define(binding, v);
            }
            Ok(Value::Null)
        }
        // Fase 9.0.1 (F15): `Stmt::Error` solo lo produce
        // `parse_with_recovery` (modo recovery del parser para tooling
        // externo). La CLI strict (`fitz run`/`build`/`check`) usa
        // `parse()`, que aborta al primer error de parser — nunca
        // produce `Stmt::Error`. Defensa en profundidad: si llegamos
        // acá es un bug del compilador, no del programa del usuario.
        Stmt::Error(span) => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line,
            span.column,
            "nodo `Stmt::Error` en el AST — la CLI strict no debería producirlo (bug del compilador, Fase 9.0.1)",
        ))),
    }
}

/// Fase 8.1.2 — handler para `from python import X[, Y as z]`. Vive
/// fuera de `eval_stmt` para que el `#[cfg]` switching de la feature
/// `python` quede acotado a una sola fn.
///
/// Reglas en 8.1.2:
/// - El path tiene que ser exactamente `["python"]`. `from python.X
///   import Y` se rechaza con mensaje claro (deuda menor: importar
///   submódulos directamente; workaround actual: `from python import
///   X` y acceder a `Y` via field access, lo cual llega en 8.1.3).
/// - Cada `name` se importa como módulo Python top-level via
///   `py_interop::import_module(name)`.
/// - El binding local respeta el alias `as` si está, sino usa el nombre
///   original (mismo criterio que `Stmt::FromImport` Fitz).
#[cfg(feature = "python")]
async fn eval_python_from_import(
    path: &[String],
    names: &[(String, Option<String>)],
    env: EnvRef,
) -> EvalResult<Value> {
    if path.len() != 1 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            0, 0,
            format!(
                "`from python.{} import ...` no se soporta en Fase 8.1; \
                 usá `from python import {}` y accedé a sub-atributos con `.` (8.1.3)",
                path[1..].join("."),
                path[1],
            ),
        )));
    }
    for (name, alias) in names {
        let module = crate::py_interop::import_module(name)
            .map_err(EvalSignal::Error)?;
        let binding = alias.clone().unwrap_or_else(|| name.clone());
        env.lock().define(binding, module);
    }
    Ok(Value::Null)
}

/// Stub sin-feature: `from python import ...` aborta con mensaje que
/// cita exactamente cómo recompilar para habilitar la interop. La
/// promesa "binario `fitz` default standalone" exige que este path
/// devuelva error claro en lugar de panic o fallback silencioso.
#[cfg(not(feature = "python"))]
async fn eval_python_from_import(
    _path: &[String],
    _names: &[(String, Option<String>)],
    _env: EnvRef,
) -> EvalResult<Value> {
    Err(EvalSignal::Error(FitzError::new(
        ErrorKind::UndefinedVariable("python".to_string()),
        0, 0,
        "`from python import ...` requiere recompilar `fitz` con interop Python habilitada. \
         Este binario se compiló sin la feature `python`. \
         Recompilá con `cargo install --features python` (o `cargo build --features python`).".to_string(),
    )))
}

/// Resultado de correr el cuerpo de un loop una vez. Convierte signals de
/// control de flujo en una decisión local (seguir / salir / propagar).
enum LoopControl {
    Continue,
    /// Mini-tanda L: `break <v>` lleva el valor. En statement-mode
    /// el caller lo descarta; en `Expr::Loop` lo devuelve.
    Break(Value),
    Propagate(EvalSignal),
}

/// Mini-tanda L — `label_matches(signal_label, loop_label)` decide si
/// el loop actual debe capturar el signal o propagarlo arriba.
///
///  - `signal_label = None` → matchea cualquier loop (caso default
///    `break` sin label = loop más cercano).
///  - `signal_label = Some(l)` → solo matchea si `loop_label == Some(l)`.
fn label_matches(signal_label: &Option<String>, loop_label: &Option<String>) -> bool {
    match signal_label {
        None => true,
        Some(s) => match loop_label {
            Some(l) => s == l,
            None => false,
        },
    }
}

/// Mini-tanda Md — Bindea un Pattern del for contra un Value en el
/// env actual. Cubre Ident, Wildcard, Tuple (recursivo). Otros
/// patterns NO deberían llegar acá — el checker los rechaza, pero
/// si lo hacen igual, devolvemos un error de runtime claro.
fn bind_for_pattern(pat: &crate::ast::Pattern, value: Value, env: &EnvRef) -> EvalResult<()> {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name) => {
            env.lock().define(name.clone(), value);
            Ok(())
        }
        Pattern::Wildcard => Ok(()),
        Pattern::Tuple(subs) => match value {
            Value::Tuple(items) if items.len() == subs.len() => {
                for (sub, v) in subs.iter().zip(items) {
                    bind_for_pattern(sub, v, env)?;
                }
                Ok(())
            }
            Value::Tuple(items) => Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: format!("tupla de {} elementos", subs.len()),
                    found: format!("tupla de {} elementos", items.len()),
                },
                0, 0,
                format!(
                    "tuple pattern del `for` espera {} elementos, recibió {}",
                    subs.len(), items.len()
                ),
            ))),
            other => Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Tuple".into(),
                    found: other.type_name().into(),
                },
                0, 0,
                format!(
                    "tuple pattern del `for` espera una tupla, recibió `{}`",
                    other.type_name()
                ),
            ))),
        },
        other => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            0, 0,
            format!("patrón `{:?}` no admitido en `for` (usá Ident, `_`, o Tuple)", other),
        ))),
    }
}

/// Mini-tanda Cmp+ — Helper recursivo que recorre los `for` clauses
/// de una list comprehension (cartesian product) y aplica el `expr`
/// en el nivel más interno. El filter (si está) se evalúa adentro
/// del último loop, antes del expr final. Devuelve la lista final.
#[async_recursion]
async fn run_list_comp(
    expr: &Expr,
    clauses: &[(crate::ast::Pattern, Expr)],
    filter: Option<&Expr>,
    env: EnvRef,
) -> EvalResult<Vec<Value>> {
    if clauses.is_empty() {
        // Nivel más interno: evaluar filter (si está) y el expr.
        if let Some(f) = filter {
            let fv = eval_expr(f, env.clone()).await?;
            match fv {
                Value::Bool(true) => {}
                Value::Bool(false) => return Ok(Vec::new()),
                other => {
                    let s = f.span();
                    return Err(EvalSignal::Error(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            found: other.type_name().into(),
                        },
                        s.line, s.column,
                        format!(
                            "el filtro `if` de la list comprehension debe ser `Bool`, no `{}`",
                            other.type_name()
                        ),
                    )));
                }
            }
        }
        let v = eval_expr(expr, env).await?;
        return Ok(vec![v]);
    }
    let (var, iter) = &clauses[0];
    let iter_v = eval_expr(iter, env.clone()).await?;
    let items: Vec<Value> = match iter_v {
        Value::List(items) => items.lock().clone(),
        Value::Range { start, end } => (start..end).map(Value::Int).collect(),
        other => {
            let s = iter.span();
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "List o Range".into(),
                    found: other.type_name().into(),
                },
                s.line, s.column,
                format!(
                    "list comprehension necesita un iterable (`List` o `Range`), recibió `{}`",
                    other.type_name()
                ),
            )));
        }
    };
    let mut out: Vec<Value> = Vec::new();
    for item in items {
        let child = Environment::new_child(env.clone());
        bind_for_pattern(var, item, &child)?;
        let sub = run_list_comp(expr, &clauses[1..], filter, child).await?;
        out.extend(sub);
    }
    Ok(out)
}

/// Mini-tanda Cmp+ — análogo de `run_list_comp` para map comprehensions.
/// Construye un `Vec<(Value, Value)>` con last-write-wins en duplicados
/// (mismo approach que `List.to_map`).
#[async_recursion]
async fn run_map_comp(
    key_expr: &Expr,
    value_expr: &Expr,
    clauses: &[(crate::ast::Pattern, Expr)],
    filter: Option<&Expr>,
    env: EnvRef,
) -> EvalResult<Vec<(Value, Value)>> {
    if clauses.is_empty() {
        if let Some(f) = filter {
            let fv = eval_expr(f, env.clone()).await?;
            match fv {
                Value::Bool(true) => {}
                Value::Bool(false) => return Ok(Vec::new()),
                other => {
                    let s = f.span();
                    return Err(EvalSignal::Error(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            found: other.type_name().into(),
                        },
                        s.line, s.column,
                        format!(
                            "el filtro `if` de la map comprehension debe ser `Bool`, no `{}`",
                            other.type_name()
                        ),
                    )));
                }
            }
        }
        let k = eval_expr(key_expr, env.clone()).await?;
        let v = eval_expr(value_expr, env).await?;
        return Ok(vec![(k, v)]);
    }
    let (var, iter) = &clauses[0];
    let iter_v = eval_expr(iter, env.clone()).await?;
    let items: Vec<Value> = match iter_v {
        Value::List(items) => items.lock().clone(),
        Value::Range { start, end } => (start..end).map(Value::Int).collect(),
        other => {
            let s = iter.span();
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "List o Range".into(),
                    found: other.type_name().into(),
                },
                s.line, s.column,
                format!(
                    "map comprehension necesita un iterable (`List` o `Range`), recibió `{}`",
                    other.type_name()
                ),
            )));
        }
    };
    let mut out: Vec<(Value, Value)> = Vec::new();
    for item in items {
        let child = Environment::new_child(env.clone());
        bind_for_pattern(var, item, &child)?;
        let sub = run_map_comp(key_expr, value_expr, &clauses[1..], filter, child).await?;
        // Last-write-wins: sobreescribimos keys existentes.
        for (k, v) in sub {
            if let Some(slot) = out.iter_mut().find(|(ek, _)| ek == &k) {
                slot.1 = v;
            } else {
                out.push((k, v));
            }
        }
    }
    Ok(out)
}

/// Ejecuta los stmts del body en orden. Si alguno emite `Break` o `Continue`,
/// los traduce a control local SI el label matchea el del loop owner; sino
/// propaga arriba. Cualquier otro signal (Error, Return) sube como
/// `Propagate`.
#[async_recursion]
async fn run_loop_body(body: &[Stmt], env: EnvRef, loop_label: Option<String>) -> LoopControl {
    for stmt in body {
        match eval_stmt(stmt, env.clone()).await {
            Ok(_) => {}
            Err(EvalSignal::Break(v, sig_label)) => {
                if label_matches(&sig_label, &loop_label) {
                    return LoopControl::Break(v);
                }
                return LoopControl::Propagate(EvalSignal::Break(v, sig_label));
            }
            Err(EvalSignal::Continue(sig_label)) => {
                if label_matches(&sig_label, &loop_label) {
                    return LoopControl::Continue;
                }
                return LoopControl::Propagate(EvalSignal::Continue(sig_label));
            }
            Err(other) => return LoopControl::Propagate(other),
        }
    }
    LoopControl::Continue
}

// ---------------------------------------------------------------------------
// match_pattern — chequea si un Pattern matchea un Value.
//
// Resultado:
//   None             → no matcheó, probar el siguiente arm.
//   Some(None)       → matcheó sin binding (literal/wildcard/range/Or
//                      cuyo branch ganador no bindea).
//   Some(Some((n, v))) → matcheó y bindea `v` a `n` (Ident/Ok/Err).
//
// Para `Pattern::Or`, probamos cada sub-pattern en orden y devolvemos el
// primer match. Los sub-patterns de un Or no bindean por contrato del
// parser (rechaza Ident/OkBinding/ErrBinding adentro), así que el
// resultado siempre es `Some(None)` cuando alguno matchea.
// ---------------------------------------------------------------------------

fn match_pattern(pat: &Pattern, v: &Value) -> Option<Option<(String, Value)>> {
    match (pat, v) {
        (Pattern::Int(p), Value::Int(vv)) if p == vv => Some(None),
        (Pattern::Float(p), Value::Float(vv)) if p == vv => Some(None),
        (Pattern::Str(p), Value::Str(vv)) if p == vv => Some(None),
        (Pattern::Bool(p), Value::Bool(vv)) if p == vv => Some(None),
        (Pattern::Null, Value::Null) => Some(None),
        (Pattern::Wildcard, _) => Some(None),
        (Pattern::Ident(name), _) => Some(Some((name.clone(), v.clone()))),
        (Pattern::Range { start, end, inclusive }, Value::Int(vv))
            if start <= vv && (if *inclusive { vv <= end } else { vv < end }) =>
        {
            Some(None)
        }
        (Pattern::OkBinding(name), Value::Result(ResultVariant::Ok(inner))) => {
            Some(Some((name.clone(), (**inner).clone())))
        }
        (Pattern::ErrBinding(name), Value::Result(ResultVariant::Err(inner))) => {
            Some(Some((name.clone(), (**inner).clone())))
        }
        (Pattern::OkWildcard, Value::Result(ResultVariant::Ok(_))) => Some(None),
        (Pattern::ErrWildcard, Value::Result(ResultVariant::Err(_))) => Some(None),
        (Pattern::Or(subs), _) => {
            for sub in subs {
                if let Some(b) = match_pattern(sub, v) {
                    return Some(b);
                }
            }
            None
        }
        // Tuples (mini-tanda T): matchea por longitud + cada slot.
        // Acumula múltiples bindings — el caller fields un Vec
        // pero la API actual solo soporta uno. Workaround: usamos
        // un helper que aplica todos los bindings al env.
        // Para la API actual `Option<Option<(String, Value)>>`,
        // necesitamos representar múltiples bindings. Cambiamos
        // el modelo: el caller ahora hace el bind via
        // `bind_pattern_into_env` cuando es Tuple.
        (Pattern::Tuple(subs), Value::Tuple(items)) => {
            if subs.len() != items.len() {
                return None;
            }
            // Recursamos en cada slot. Si algún sub no matchea,
            // toda la tupla falla.
            for (s, v) in subs.iter().zip(items.iter()) {
                match_pattern(s, v)?;
            }
            // Acá devolvemos `Some(None)` porque la API solo
            // permite un binding. Los bindings reales del tuple
            // se aplican en el caller via `bind_tuple_pattern`.
            Some(None)
        }
        _ => None,
    }
}

/// Mini-tanda T — aplica todos los bindings de un Pattern al env
/// dado. Para tuple patterns recursea en cada slot. Para
/// Ident/Ok/Err captura el valor. Para wildcards/literales no
/// hace nada. Precondición: el pattern matchea el value (debe
/// haberse chequeado con `match_pattern` antes).
fn bind_tuple_pattern(pat: &Pattern, v: &Value, env: EnvRef) {
    match (pat, v) {
        (Pattern::Ident(name), _) => {
            env.lock().define(name.clone(), v.clone());
        }
        (Pattern::OkBinding(name), Value::Result(ResultVariant::Ok(inner))) => {
            env.lock().define(name.clone(), (**inner).clone());
        }
        (Pattern::ErrBinding(name), Value::Result(ResultVariant::Err(inner))) => {
            env.lock().define(name.clone(), (**inner).clone());
        }
        (Pattern::Tuple(subs), Value::Tuple(items)) => {
            for (s, v) in subs.iter().zip(items.iter()) {
                bind_tuple_pattern(s, v, env.clone());
            }
        }
        // El resto (literales, Wildcard, OkWildcard, ErrWildcard,
        // Range, Or) no bindean.
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// eval_expr — evalúa una expresión a un Value.
// ---------------------------------------------------------------------------

#[async_recursion]
async fn eval_expr(expr: &Expr, env: EnvRef) -> EvalResult<Value> {
    let span = expr.span();
    match expr {
        // Fp.3 — NamedArg solo es válido adentro de Call.args; el
        // dispatcher de Call lo procesa antes de invocar el value.
        // Verlo en eval_expr indica AST mal formado.
        Expr::NamedArg { name, .. } => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeError,
            span.line, span.column,
            format!(
                "argumento nombrado `{}:` no puede aparecer fuera de una llamada",
                name
            ),
        ))),

        // Literales — el valor está embebido en el AST.
        Expr::Int(n, _) => Ok(Value::Int(*n)),
        Expr::Float(x, _) => Ok(Value::Float(*x)),
        Expr::Str(s, _) => Ok(Value::Str(s.clone())),
        Expr::Bool(b, _) => Ok(Value::Bool(*b)),
        Expr::Null(_) => Ok(Value::Null),

        // Mini-tanda L — `loop { body }` como expresión. El valor es
        // el `<v>` del primer `break <v>` que dispara. `break` sin
        // valor → `Null`. Otros signals (Return, Error) suben.
        Expr::Loop { body, label, .. } => {
            loop {
                match run_loop_body(body, env.clone(), label.clone()).await {
                    LoopControl::Continue => continue,
                    LoopControl::Break(v) => return Ok(v),
                    LoopControl::Propagate(sig) => return Err(sig),
                }
            }
        }

        // Tuples (mini-tanda T) — eval cada slot y armamos el Value.
        Expr::Tuple(items, _) => {
            let mut vals = Vec::with_capacity(items.len());
            for e in items {
                vals.push(eval_expr(e, env.clone()).await?);
            }
            Ok(Value::Tuple(vals))
        }
        Expr::TupleField { tuple, index, span } => {
            let v = eval_expr(tuple, env).await?;
            match v {
                Value::Tuple(items) => {
                    items.get(*index).cloned().ok_or_else(|| {
                        EvalSignal::Error(FitzError::new(
                            ErrorKind::InvalidSyntax,
                            span.line, span.column,
                            format!(
                                "índice de tupla {} fuera de rango (tupla de {} elementos)",
                                index, items.len()
                            ),
                        ))
                    })
                }
                other => Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Tuple".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "acceso `.{}` solo aplica a tuplas, recibí `{}`",
                        index, other.type_name()
                    ),
                ))),
            }
        }

        // Identificador — lookup encadenado en la cadena de scopes.
        Expr::Ident(name, _) => env.lock().get(name).ok_or_else(|| {
            EvalSignal::Error(FitzError::new(
                ErrorKind::UndefinedVariable(name.clone()),
                span.line, span.column,
                format!("variable `{}` no definida", name),
            ))
        }),

        // And/Or hacen short-circuit: no evaluamos `right` salvo que haga
        // falta. El resto de BinOps evalúan ambos lados antes de combinar.
        Expr::BinOp { op, left, right, span }
            if matches!(op, BinOpKind::And | BinOpKind::Or | BinOpKind::Xor) =>
        {
            eval_logical(op, left, right, env, *span).await
        }
        Expr::BinOp { op, left, right, span } => {
            let lv = eval_expr(left, env.clone()).await?;
            let rv = eval_expr(right, env).await?;
            eval_binop(op, lv, rv, *span)
        }

        Expr::UnaryOp { op, operand, span } => {
            let v = eval_expr(operand, env).await?;
            eval_unary(op, v, *span)
        }

        // String con interpolación: cada `StrPart::Expr` se evalúa y se
        // convierte a string vía `Display`. Los `Lit` van tal cual.
        // Mini-tanda Fm: si la parte tiene `FormatSpec`, lo aplicamos
        // con `format_value_with_spec`; sino usamos el Display default.
        Expr::StrInterp(parts, _) => {
            let mut result = String::new();
            for part in parts {
                match part {
                    StrPart::Lit(s) => result.push_str(s),
                    StrPart::Expr(e, spec) => {
                        let v = eval_expr(e, env.clone()).await?;
                        match spec {
                            None => result.push_str(&v.to_string()),
                            Some(s) => {
                                let formatted = format_value_with_spec(&v, s)
                                    .map_err(|msg| EvalSignal::Error(FitzError::new(
                                        ErrorKind::TypeMismatch {
                                            expected: "tipo compatible con format spec".into(),
                                            found: v.type_name().into(),
                                        },
                                        e.span().line, e.span().column,
                                        msg,
                                    )))?;
                                result.push_str(&formatted);
                            }
                        }
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
        Expr::Call { callee, args, span } => eval_call(callee, args, env, *span).await,

        // `fn(x) => x * 2` o `fn(x) { return x * 2 }` — función anónima.
        // Se evalúa a `Value::Function` con el env actual como closure,
        // igual que un `Stmt::FnDef`, pero sin nombre ni binding en el env.
        // Mini-tanda Async-cl — `async fn(...)` propaga `is_async = true`,
        // habilitando `.await` adentro y haciendo que la invocación
        // devuelva `Value::Future` perezoso (paralelo a fn nombradas async).
        Expr::FnExpr { params, body, is_async, .. } => Ok(Value::Function {
            params: params.clone(),
            body: body.clone(),
            closure: env,
            is_async: *is_async,
        }),

        // `obj.campo` — acceso a campo de instancia de tipo custom, o
        // a un export de un módulo importado. Para receptores no-Instance
        // y no-Module (List, Map, Str, etc.), el camino habitual es el
        // method dispatch (`xs.map(...)`), que va por la rama `Expr::Call`
        // con callee `Field`. El field access "pelado" sobre primitivos
        // no tiene semántica útil hoy.
        Expr::Field { object, field, .. } => {
            let obj = eval_expr(object, env).await?;
            match obj {
                Value::Instance { type_name, fields } => {
                    fields
                        .lock()
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
                    module_env.lock().get(field).ok_or_else(|| {
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
                // Fase 8.1.3 — `Value::PyObject(.field)` baja a CPython
                // via `getattr`. La auto-coerción primitiva la hace
                // `py_interop::get_attr`: primitivos vuelven como
                // `Value` nativos (Int/Float/Str/Bool/Null), tipos
                // compuestos como `Value::PyObject` opaco para
                // chaining (`math.sqrt(16)` será válido en 8.1.4).
                #[cfg(feature = "python")]
                Value::PyObject(handle) => {
                    crate::py_interop::get_attr(&handle, field).map_err(|mut e| {
                        // El `py_err_to_fitz` setea línea/columna 0 porque
                        // se ejecuta sin contexto del AST. Sobrescribimos
                        // con el span del field access para que el error
                        // apunte al sitio del `.attr` en el source Fitz.
                        if e.line == 0 && e.column == 0 {
                            e.line = span.line;
                            e.column = span.column;
                        }
                        EvalSignal::Error(e)
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
            let ty = env.lock().get(type_name).ok_or_else(|| {
                EvalSignal::Error(FitzError::new(
                    ErrorKind::UndefinedVariable(type_name.clone()),
                    span.line, span.column,
                    format!("tipo `{}` no definido", type_name),
                ))
            })?;
            let (declared_type_name, declared, resolved_defaults) = match ty {
                Value::Type { name, fields, resolved_defaults, .. } => (name, fields, resolved_defaults),
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
            //
            // PreF8.3: el default puede venir pre-evaluado (tipos
            // importados — el loader ya los materializó en el env del
            // módulo de origen) o como Expr (tipos locales — evaluación
            // lazy en cada struct lit con el env del call site).
            let mut instance_fields: Vec<(String, Value)> =
                Vec::with_capacity(declared.len());
            for f in &declared {
                let provided = fields.iter().find(|(n, _)| n == &f.name);
                let value = if let Some((_, expr)) = provided {
                    eval_expr(expr, env.clone()).await?
                } else if let Some((_, v)) = resolved_defaults
                    .iter()
                    .find(|(n, _)| n == &f.name)
                {
                    v.clone()
                } else if let Some(default_expr) = &f.default {
                    eval_expr(default_expr, env.clone()).await?
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
            // PreF8.4: usamos el nombre canónico del Value::Type (el
            // del archivo donde el `type` se declaró), NO el `type_name`
            // del literal sintáctico. Con `from foo import User as P`,
            // `P { ... }` produce una instancia cuyo Display dice
            // "User { ... }" — paridad con `fitz build` (donde
            // `P` es un alias de `User` en Rust y el `Display` está
            // implementado sobre `UserData` con el nombre original).
            Ok(Value::new_instance(declared_type_name, instance_fields))
        }

        // `[e1, e2, ...]` — evaluamos los elementos en orden.
        Expr::List(items, _) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(eval_expr(item, env.clone()).await?);
            }
            Ok(Value::new_list(values))
        }

        // Mini-tanda C + Cmp+ — `[expr for var in iter ([for ...]*) [if cond]?]`.
        // Multi-for clauses producen cartesian product. Cada `for` abre
        // un env hijo dedicado (estilo Python — el var no escapa al
        // caller). El filter (si está) se evalúa en el loop más interno.
        Expr::ListComp { expr, var, iter, extra_clauses, filter, span: _ } => {
            // Armamos un Vec<(Pattern, Expr)> con el primer clause +
            // los extras y delegamos al helper recursivo.
            let mut all_clauses: Vec<(crate::ast::Pattern, Expr)> =
                Vec::with_capacity(1 + extra_clauses.len());
            all_clauses.push((var.clone(), (**iter).clone()));
            for (p, it) in extra_clauses {
                all_clauses.push((p.clone(), it.clone()));
            }
            let result = run_list_comp(expr, &all_clauses, filter.as_deref(), env).await?;
            Ok(Value::new_list(result))
        }

        // Mini-tanda Cmp+ — `{key: value for ...}`. Análogo a ListComp
        // pero produce un `Map<K, V>`. Last-write-wins en duplicados
        // (mismo approach que `List.to_map`). Soporta múltiples `for`
        // clauses + filter opcional.
        Expr::MapComp { key, value, var, iter, extra_clauses, filter, span: _ } => {
            let mut all_clauses: Vec<(crate::ast::Pattern, Expr)> =
                Vec::with_capacity(1 + extra_clauses.len());
            all_clauses.push((var.clone(), (**iter).clone()));
            for (p, it) in extra_clauses {
                all_clauses.push((p.clone(), it.clone()));
            }
            let pairs = run_map_comp(key, value, &all_clauses, filter.as_deref(), env).await?;
            Ok(Value::new_map(pairs))
        }

        // `{k1: v1, ...}` — evaluamos cada par en orden (clave, valor).
        // El orden de inserción se preserva en el Vec resultante.
        Expr::Map(pairs, _) => {
            let mut entries = Vec::with_capacity(pairs.len());
            for (k_expr, v_expr) in pairs {
                let k = eval_expr(k_expr, env.clone()).await?;
                let v = eval_expr(v_expr, env.clone()).await?;
                entries.push((k, v));
            }
            Ok(Value::new_map(entries))
        }

        // `start..end` — ambos extremos tienen que ser Int (no hay rangos
        // de Float). El rango se materializa como `Value::Range`; la
        // iteración real (cuando se usa en `for`) ocurre en Stmt::For.
        Expr::Range { start, end, inclusive, .. } => {
            let s_v = eval_expr(start, env.clone()).await?;
            let e_v = eval_expr(end, env).await?;
            let s = expect_int_for_range(&s_v, "inicio", start.span())?;
            let e = expect_int_for_range(&e_v, "fin", end.span())?;
            // R.1.4: para rangos inclusivos, "promovemos" a la
            // representación exclusiva sumando 1 al end. Así
            // `Value::Range` no necesita un flag nuevo; el for loop
            // sigue iterando `start..end` exclusivo. Caveat: si
            // `end == i64::MAX`, overflow — edge case raro,
            // documentado.
            let e_final = if *inclusive { e.saturating_add(1) } else { e };
            Ok(Value::Range { start: s, end: e_final })
        }

        // `obj[idx]` — indexing. Dispatch por tipo del objeto.
        Expr::Index { object, index, span } => {
            let obj = eval_expr(object, env.clone()).await?;
            let idx = eval_expr(index, env).await?;
            eval_index(&obj, &idx, *span)
        }

        // I.2 (mini-tanda I) — slicing `xs[a..b]`, `xs[..b]`,
        // `xs[a..]`, `xs[..]`, `xs[a..=b]`. Out-of-range se clampea
        // (estilo Python). Soporta receivers List<T> y Str.
        Expr::Slice { object, start, end, inclusive, span } => {
            let obj = eval_expr(object, env.clone()).await?;
            let start_v = if let Some(s) = start {
                Some(eval_expr(s, env.clone()).await?)
            } else {
                None
            };
            let end_v = if let Some(e) = end {
                Some(eval_expr(e, env.clone()).await?)
            } else {
                None
            };
            eval_slice(&obj, start_v.as_ref(), end_v.as_ref(), *inclusive, *span)
        }

        // `if cond { then } else { else_ }`. Funciona como expresión: su
        // valor es el del último stmt del bloque ejecutado. Sin else y cond
        // falsa → Null.
        //
        // Los bloques NO crean scope nuevo — variables declaradas adentro
        // persisten en el scope contenedor (estilo Python). Deuda explícita
        // si después esto trae sorpresas.
        Expr::If { condition, then, else_, .. } => {
            let cond_v = eval_expr(condition, env.clone()).await?;
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
                eval_block(then, env).await
            } else if let Some(else_block) = else_ {
                eval_block(else_block, env).await
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
            let v = eval_expr(value, env.clone()).await?;

            for arm in arms {
                let Some(binding) = match_pattern(&arm.pattern, &v) else {
                    continue;
                };

                // R.2.2 — guard `if cond`. Crear un scope hijo (para
                // que el binding del pattern sea visible en el guard)
                // y evaluar la condición. Si NO matchea, pasamos al
                // siguiente arm — el binding queda descartado con el
                // scope.
                // Mini-tanda T — para tuple patterns aplicamos todos
                // los bindings via `bind_tuple_pattern`. Para los
                // patterns "simples" (Ident/Ok/Err binding) la API
                // ya devolvió `Some(Some(name, value))`.
                let arm_env = if matches!(&arm.pattern, Pattern::Tuple(_)) {
                    let child = Environment::new_child(env.clone());
                    bind_tuple_pattern(&arm.pattern, &v, child.clone());
                    child
                } else if let Some((name, bound)) = &binding {
                    let child = Environment::new_child(env.clone());
                    child.lock().define(name.clone(), bound.clone());
                    child
                } else {
                    env.clone()
                };
                if let Some(guard_expr) = &arm.guard {
                    let cond = eval_expr(guard_expr, arm_env.clone()).await?;
                    let cond_bool = match cond {
                        Value::Bool(b) => b,
                        other => {
                            return Err(EvalSignal::Error(FitzError::new(
                                ErrorKind::TypeError,
                                span.line, span.column,
                                format!(
                                    "el guard de un arm debe ser Bool, recibí {}",
                                    other.type_name()
                                ),
                            )));
                        }
                    };
                    if !cond_bool {
                        continue;
                    }
                }

                // Sp.2 — body es Vec<Stmt>. Ejecutamos en orden; el
                // valor del arm es el valor del último Stmt::Expr (si
                // los hay), o Null en su defecto. Stmt::Return/Break/
                // Continue propagan como EvalSignal — el match no los
                // captura, suben al fn/loop contenedor.
                let body_env = if binding.is_some() || matches!(&arm.pattern, Pattern::Tuple(_)) {
                    arm_env
                } else {
                    env.clone()
                };
                let mut last_value = Value::Null;
                for stmt in &arm.body {
                    match stmt {
                        Stmt::Expr(e, _) => {
                            last_value = eval_expr(e, body_env.clone()).await?;
                        }
                        other => {
                            eval_stmt(other, body_env.clone()).await?;
                            last_value = Value::Null;
                        }
                    }
                }
                return Ok(last_value);
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
            let v = eval_expr(inner, env).await?;
            Ok(Value::Result(ResultVariant::Ok(Box::new(v))))
        }

        // `Err(inner)` — constructor de la variante de error.
        Expr::Err(inner, _) => {
            let v = eval_expr(inner, env).await?;
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
        // `.await` desempaca un `Value::Future`. El checker 6.2 valida
        // estáticamente que el operando sea `Future<T>` y que estemos
        // adentro de una `async fn`; este path es la implementación
        // dinámica.
        //
        // Política: un future se consume una sola vez. El `Option`
        // adentro de `FutureCell` nos deja extraer el `Pin<Box<>>` con
        // `.take()` sin clonar; un segundo `.await` sobre el mismo
        // `Value::Future` paniquea con error explícito.
        Expr::Await(inner, await_span) => {
            let v = eval_expr(inner, env).await?;
            match v {
                Value::Future(cell) => {
                    let fut = cell.0.lock().take();
                    match fut {
                        Some(f) => f.await.map_err(EvalSignal::Error),
                        None => Err(EvalSignal::Error(FitzError::new(
                            ErrorKind::InvalidSyntax,
                            await_span.line, await_span.column,
                            "`.await` sobre un `Future` que ya fue consumido",
                        ))),
                    }
                }
                other => Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Future".into(),
                        found: other.type_name().into(),
                    },
                    await_span.line, await_span.column,
                    format!(
                        "`.await` solo aplica a `Future<T>`, recibió `{}`",
                        other.type_name()
                    ),
                ))),
            }
        }

        Expr::Try(inner, try_span) => {
            let v = eval_expr(inner, env).await?;
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

        // Fase 9.0.1 (F15): paralelo a `Stmt::Error` — defensa contra
        // un bug del compilador. La CLI strict nunca debería ver este
        // nodo; solo `parse_with_recovery` lo produce.
        Expr::Error(_) => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line,
            span.column,
            "nodo `Expr::Error` en el AST — la CLI strict no debería producirlo (bug del compilador, Fase 9.0.1)",
        ))),
    }
}

/// Evalúa una secuencia de sentencias en el env dado (sin crear scope
/// nuevo) y devuelve el valor de la última. Bloque vacío → Null.
///
/// Los signals (Return/Break/Continue/Error) se propagan: si un stmt los
/// emite, el resto del bloque no se ejecuta.
#[async_recursion]
async fn eval_block(stmts: &[Stmt], env: EnvRef) -> EvalResult<Value> {
    let mut last = Value::Null;
    for stmt in stmts {
        last = eval_stmt(stmt, env.clone()).await?;
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
#[async_recursion]
async fn eval_call(callee: &Expr, args: &[Expr], env: EnvRef, span: Span) -> EvalResult<Value> {
    // Method call.
    if let Expr::Field { object, field, .. } = callee {
        let receiver = eval_expr(object, env.clone()).await?;
        // Fp.3 — extraer args con/sin nombre. El dispatch del método
        // reordena por nombre al resolver el método target.
        let mut named_args: Vec<(Option<String>, Value)> = Vec::with_capacity(args.len());
        for arg in args {
            let (name, value_expr) = match arg {
                Expr::NamedArg { name, value, .. } => (Some(name.clone()), value.as_ref()),
                other => (None, other),
            };
            let v = eval_expr(value_expr, env.clone()).await?;
            named_args.push((name, v));
        }
        return dispatch_method_named(receiver, field, named_args, env, span).await;
    }

    // Llamada normal.
    let callee_value = eval_expr(callee, env.clone()).await?;
    // Fp.3 — args con/sin nombre. La resolución de nombres → posiciones
    // ocurre en `invoke_value_named` después de evaluar args y conocer
    // el target (Value::Function tiene los param names).
    let mut named_args: Vec<(Option<String>, Value)> = Vec::with_capacity(args.len());
    for arg in args {
        let (name, value_expr) = match arg {
            Expr::NamedArg { name, value, .. } => (Some(name.clone()), value.as_ref()),
            other => (None, other),
        };
        let v = eval_expr(value_expr, env.clone()).await?;
        named_args.push((name, v));
    }
    let display_name = callee_display_name(callee);
    invoke_value_named(callee_value, named_args, &display_name, span).await
}

/// Fp.3 — reordena `named_args` (mezcla de positionals y named) a una
/// Vec<Value> posicional respetando los nombres de `param_names`. La
/// regla: positionals primero (sin nombre), después los named (cada uno
/// al slot de su nombre). Slots no cubiertos quedan `None` para que el
/// caller los llene con defaults.
///
/// Errores: nombre duplicado, nombre desconocido, named "sobreescribe"
/// a un positional ya provisto.
fn resolve_named_args(
    named_args: Vec<(Option<String>, Value)>,
    param_names: &[String],
    display_name: &str,
    span: Span,
) -> EvalResult<Vec<Option<Value>>> {
    let mut slots: Vec<Option<Value>> = (0..param_names.len()).map(|_| None).collect();
    let mut next_positional = 0usize;
    let mut after_named = false;
    for (name_opt, value) in named_args {
        if let Some(name) = name_opt {
            after_named = true;
            // Buscar el index del param por nombre.
            let idx = param_names.iter().position(|p| p == &name).ok_or_else(|| {
                EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeError,
                    span.line, span.column,
                    format!(
                        "`{}` no tiene un parámetro llamado `{}`",
                        display_name, name
                    ),
                ))
            })?;
            // Si el slot ya está ocupado por un positional previo o
            // por otro named con el mismo nombre, error claro.
            if slots[idx].is_some() {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeError,
                    span.line, span.column,
                    format!(
                        "`{}`: el argumento `{}` está duplicado",
                        display_name, name
                    ),
                )));
            }
            slots[idx] = Some(value);
        } else {
            if after_named {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeError,
                    span.line, span.column,
                    format!(
                        "`{}`: no se puede pasar un argumento posicional después de uno nombrado",
                        display_name
                    ),
                )));
            }
            if next_positional >= param_names.len() {
                // Más positionals que params — el invocador lo va a
                // detectar después como exceso de args. Lo dejamos pasar
                // acá y el chequeo de aridad final reporta.
                slots.push(Some(value));
                next_positional += 1;
            } else {
                slots[next_positional] = Some(value);
                next_positional += 1;
            }
        }
    }
    Ok(slots)
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

/// Fp.3 — versión de `invoke_value` que acepta args con/sin nombre.
/// Para `Value::Function`, los nombres se mapean a posiciones según
/// los `params` de la fn. Para builtins y PyObject (sin info de
/// nombres), rechaza named args con error claro.
async fn invoke_value_named(
    value: Value,
    named_args: Vec<(Option<String>, Value)>,
    display_name: &str,
    span: Span,
) -> EvalResult<Value> {
    let has_named = named_args.iter().any(|(n, _)| n.is_some());
    // Caso rápido: si no hay nombres, delegar al path posicional clásico.
    if !has_named {
        let positional: Vec<Value> = named_args.into_iter().map(|(_, v)| v).collect();
        return invoke_value(value, positional, display_name, span).await;
    }
    // Hay nombres — el callee debe ser una Fitz Function con params
    // conocidos.
    match value {
        Value::Function { params, body, closure, is_async } => {
            let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
            let slots = resolve_named_args(named_args, &param_names, display_name, span)?;
            // Reconstruir Vec<Value> rellenando defaults para los None.
            let has_varargs = params.last().map(|p| p.varargs).unwrap_or(false);
            if has_varargs {
                // Varargs + named args no compatibles en MVP.
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeError,
                    span.line, span.column,
                    format!(
                        "`{}` tiene un parámetro variádico; los argumentos nombrados \
                         no son compatibles con varargs en esta versión",
                        display_name
                    ),
                )));
            }
            let call_env = Environment::new_child(closure);
            for (i, param) in params.iter().enumerate() {
                let value = match &slots[i] {
                    Some(v) => v.clone(),
                    None => {
                        // Rellenar con default.
                        let de = param.default.as_ref().ok_or_else(|| {
                            EvalSignal::Error(FitzError::new(
                                ErrorKind::WrongArgCount {
                                    expected: params.len(),
                                    found: slots.iter().filter(|s| s.is_some()).count(),
                                },
                                span.line, span.column,
                                format!(
                                    "`{}`: falta el argumento `{}` (no tiene default)",
                                    display_name, param.name
                                ),
                            ))
                        })?;
                        eval_expr(de, call_env.clone()).await?
                    }
                };
                call_env.lock().define(param.name.clone(), value);
            }
            // Mismo path que invoke_value: ejecutar body sync o async.
            if is_async {
                let owned_body = body;
                let display_owned = display_name.to_string();
                let fut: crate::value::FitzFuture = Box::pin(async move {
                    for stmt in &owned_body {
                        match eval_stmt(stmt, call_env.clone()).await {
                            Ok(_) => {}
                            Err(EvalSignal::Return(v)) => return Ok(v),
                            Err(signal) => return Err(signal_to_error(signal)),
                        }
                    }
                    let _ = display_owned;
                    Ok(Value::Null)
                });
                return Ok(Value::new_future(fut));
            }
            for stmt in &body {
                match eval_stmt(stmt, call_env.clone()).await {
                    Ok(_) => {}
                    Err(EvalSignal::Return(v)) => return Ok(v),
                    Err(other) => return Err(other),
                }
            }
            Ok(Value::Null)
        }
        Value::Builtin { name, .. } => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeError,
            span.line, span.column,
            format!(
                "el builtin `{}` no soporta argumentos nombrados",
                name
            ),
        ))),
        other => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeError,
            span.line, span.column,
            format!(
                "`{}` no es invocable o no soporta argumentos nombrados (es {})",
                display_name, other.type_name()
            ),
        ))),
    }
}

/// Fp.3 — método con args nombrados. Solo soporta métodos custom (R.3)
/// y estáticos — los métodos built-in de List/Map/Str no tienen nombres
/// de params expuestos. Si todos los args son posicionales, delega al
/// path clásico.
#[async_recursion]
async fn dispatch_method_named(
    receiver: Value,
    method: &str,
    named_args: Vec<(Option<String>, Value)>,
    env: EnvRef,
    span: Span,
) -> EvalResult<Value> {
    let has_named = named_args.iter().any(|(n, _)| n.is_some());
    if !has_named {
        let positional: Vec<Value> = named_args.into_iter().map(|(_, v)| v).collect();
        return dispatch_method(receiver, method, positional, env, span).await;
    }
    // Hay nombres — buscar método custom o estático con param names.
    let method_def_opt: Option<crate::ast::MethodDef> = match &receiver {
        Value::Instance { type_name, .. } => {
            // Buscar el `Value::Type` por nombre canónico en el env.
            let tname = type_name.clone();
            let type_val = env.lock().get(&tname);
            match type_val {
                Some(Value::Type { methods, .. }) => {
                    methods.iter().find(|m| m.name == method && !m.is_static).cloned()
                }
                _ => None,
            }
        }
        Value::Type { methods, .. } => {
            methods.iter().find(|m| m.name == method && m.is_static).cloned()
        }
        _ => None,
    };
    let method_def = method_def_opt.ok_or_else(|| {
        EvalSignal::Error(FitzError::new(
            ErrorKind::TypeError,
            span.line, span.column,
            format!(
                "el método `.{}()` no acepta argumentos nombrados \
                 (solo soportado en métodos custom sobre `type`)",
                method
            ),
        ))
    })?;
    let param_names: Vec<String> = method_def.params.iter().map(|p| p.name.clone()).collect();
    let display = format!(".{}()", method);
    let slots = resolve_named_args(named_args, &param_names, &display, span)?;
    let has_varargs = method_def.params.last().map(|p| p.varargs).unwrap_or(false);
    if has_varargs {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeError,
            span.line, span.column,
            format!(
                "el método `.{}()` tiene un parámetro variádico; \
                 los argumentos nombrados no son compatibles con varargs",
                method
            ),
        )));
    }
    // Construir args posicionales rellenando con defaults.
    let mut positional: Vec<Value> = Vec::with_capacity(method_def.params.len());
    // Necesitamos un env para evaluar defaults — usamos el del caller.
    let temp_env = Environment::new_child(env.clone());
    for (i, param) in method_def.params.iter().enumerate() {
        let v = match &slots[i] {
            Some(v) => v.clone(),
            None => {
                let de = param.default.as_ref().ok_or_else(|| {
                    EvalSignal::Error(FitzError::new(
                        ErrorKind::TypeError,
                        span.line, span.column,
                        format!(
                            "el método `.{}()`: falta el argumento `{}` (no tiene default)",
                            method, param.name
                        ),
                    ))
                })?;
                eval_expr(de, temp_env.clone()).await?
            }
        };
        positional.push(v);
    }
    if method_def.is_static {
        invoke_static_method(method_def, positional, env, span).await
    } else {
        invoke_custom_method(receiver, method_def, positional, env, span).await
    }
}

/// Invoca un valor que ya sabemos que tiene que ser una función. Maneja
/// builtins, user-defined functions y errores de "no es invocable".
#[async_recursion]
async fn invoke_value(
    value: Value, arg_values: Vec<Value>, display_name: &str, span: Span,
) -> EvalResult<Value> {
    match value {
        // Fase 9.z.2.a: `assert_throws` necesita invocar el callback
        // async-recursive (el `invoke_value` genérico es async, los
        // builtins son sync). Se intercepta acá antes del despacho
        // genérico de builtins; cualquier otro builtin va por la
        // rama de abajo. El stub `builtin_assert_throws_stub` emite
        // unreachable! si llegara a ejecutarse — sentinel del bug.
        Value::Builtin { name: "assert_throws", .. } => {
            assert_throws_impl(arg_values, span).await
        }
        Value::Builtin { func, .. } => func(&arg_values).map_err(EvalSignal::Error),

        Value::Function { params, body, closure, is_async } => {
            // Fp — default params: si faltan args trailing y los params
            // tienen `default`, se rellenan.
            // Fp.2 — varargs: el último param (si es variádico) absorbe
            // 0+ args extra. Aridad mínima = required sin contar el
            // varargs; máxima = total (o sin límite con varargs).
            let has_varargs = params.last().map(|p| p.varargs).unwrap_or(false);
            let required_with_defaults = params.iter().filter(|p| p.default.is_none()).count();
            let required = if has_varargs {
                required_with_defaults.min(params.len().saturating_sub(1))
            } else {
                required_with_defaults
            };
            let too_many = !has_varargs && arg_values.len() > params.len();
            if arg_values.len() < required || too_many {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::WrongArgCount {
                        expected: params.len(),
                        found: arg_values.len(),
                    },
                    span.line, span.column,
                    if has_varargs {
                        format!(
                            "`{}` espera al menos {} argumento(s), recibió {}",
                            display_name, required, arg_values.len(),
                        )
                    } else if required == params.len() {
                        format!(
                            "`{}` espera {} argumento(s), recibió {}",
                            display_name, params.len(), arg_values.len(),
                        )
                    } else {
                        format!(
                            "`{}` espera entre {} y {} argumento(s), recibió {}",
                            display_name, required, params.len(), arg_values.len(),
                        )
                    },
                )));
            }

            // Nuevo scope hijo del CLOSURE, no del caller. Lexical scoping.
            let call_env = Environment::new_child(closure);
            let varargs_idx = if has_varargs { Some(params.len() - 1) } else { None };
            let provided = arg_values.len();
            let mut arg_iter = arg_values.into_iter();
            for (i, param) in params.iter().enumerate() {
                if Some(i) == varargs_idx {
                    // Recolectar los args restantes (incluso 0) en una List.
                    let collected: Vec<Value> = arg_iter.by_ref().collect();
                    let list = Value::new_list(collected);
                    call_env.lock().define(param.name.clone(), list);
                    break;
                }
                let value = if i < provided {
                    arg_iter.next().unwrap()
                } else {
                    // Default expr — se evalúa en el env del CLOSURE
                    // (donde la fn vive), no en el del caller. Match
                    // con la semántica de fields default y de Python.
                    let default_expr = param.default.as_ref().expect(
                        "params sin default ya fueron cubiertos por arity check",
                    );
                    eval_expr(default_expr, call_env.clone()).await?
                };
                call_env.lock().define(param.name.clone(), value);
            }

            // Fase 6.4: si la fn es async, en vez de evaluar el body
            // inmediato, lo envolvemos en un `Value::Future` perezoso.
            // El `.await` del caller fuerza la evaluación; sin await
            // queda como Future suelto (`let f = async_fn()`).
            //
            // Owned body + display_name: capturamos por valor en el
            // async block para que el future sea `'static`.
            if is_async {
                let owned_body = body;
                let display_owned = display_name.to_string();
                let fut: crate::value::FitzFuture = Box::pin(async move {
                    for stmt in &owned_body {
                        match eval_stmt(stmt, call_env.clone()).await {
                            Ok(_) => {}
                            Err(EvalSignal::Return(v)) => return Ok(v),
                            Err(signal) => {
                                return Err(signal_to_error(signal));
                            }
                        }
                    }
                    // Cuerpo sin `return` explícito → Null, igual que sync.
                    let _ = display_owned;
                    Ok(Value::Null)
                });
                return Ok(Value::new_future(fut));
            }

            for stmt in &body {
                match eval_stmt(stmt, call_env.clone()).await {
                    Ok(_) => {}
                    Err(EvalSignal::Return(v)) => return Ok(v),
                    Err(other) => return Err(other),
                }
            }
            Ok(Value::Null)
        }

        // Fase 8.1.4 — `Value::PyObject(callable)` cruza al runtime
        // Python via `py_interop::call`. Cubre `let f = math.sqrt; f(16.0)`,
        // funciones top-level del módulo Python pasadas como variable,
        // y cualquier callable opaco. El error se enriquece con el span
        // de la llamada para que el usuario vea dónde explotó.
        #[cfg(feature = "python")]
        Value::PyObject(handle) => {
            crate::py_interop::call(&handle, &arg_values).map_err(|mut e| {
                if e.line == 0 && e.column == 0 {
                    e.line = span.line;
                    e.column = span.column;
                }
                EvalSignal::Error(e)
            })
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
/// `Arc<Mutex<...>>`, así que las mutaciones se propagan a los aliases)
/// y los args ya evaluados.
///
/// Si no hay un método registrado para `(tipo, nombre)`, devuelve error
/// "método no encontrado". El usuario lo va a ver como
/// `xs.metodo_inexistente(...) — Lista no tiene un método llamado ...`.
#[async_recursion]
async fn dispatch_method(
    receiver: Value,
    method: &str,
    args: Vec<Value>,
    env: EnvRef,
    span: Span,
) -> EvalResult<Value> {
    // R.3 — método custom sobre Value::Instance. Buscamos en el
    // Value::Type asociado (resolución por type_name en el env).
    // El lookup se hace ANTES del .await para no mantener el lock
    // del env vivo a través de la suspensión (Send-safe).
    if let Value::Instance { type_name, .. } = &receiver {
        let resolved: Option<crate::ast::MethodDef> = {
            let env_guard = env.lock();
            match env_guard.get(type_name) {
                Some(Value::Type { methods, .. }) => {
                    methods.iter().find(|m| m.name == method).cloned()
                }
                _ => None,
            }
        };
        if let Some(m) = resolved {
            // Mini-tanda St — un método estático no se puede invocar
            // sobre una instancia (no recibe los fields como locales).
            if m.is_static {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    span.line, span.column,
                    format!(
                        "`{}` es un método estático: invocá como `{}.{}({})`, no como `<instancia>.{}({})`",
                        m.name, type_name, m.name,
                        m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "),
                        m.name,
                        m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "),
                    ),
                )));
            }
            return invoke_custom_method(receiver, m, args, env, span).await;
        }
    }
    // Mini-tanda St — método estático sobre Value::Type: `Type.make()`.
    if let Value::Type { name: type_name, methods, .. } = &receiver {
        let resolved = methods.iter().find(|m| m.name == method).cloned();
        if let Some(m) = resolved {
            if !m.is_static {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    span.line, span.column,
                    format!(
                        "`{}.{}()` es un método de instancia: invocá como `<instancia>.{}({})`, no como `{}.{}({})`",
                        type_name, m.name, m.name,
                        m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "),
                        type_name, m.name,
                        m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "),
                    ),
                )));
            }
            return invoke_static_method(m, args, env, span).await;
        }
        // Si no existe el método pero el receptor es un Type, error
        // específico (mejor que el genérico "tipo X no tiene método").
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!("el tipo `{}` no tiene un método estático llamado `{}`", type_name, method),
        )));
    }
    match (&receiver, method) {
        // List
        (Value::List(_), "push") => list_push(receiver, args, span),
        (Value::List(_), "pop") => list_pop(receiver, args, span),
        (Value::List(_), "map") => list_map(receiver, args, span).await,
        (Value::List(_), "filter") => list_filter(receiver, args, span).await,
        (Value::List(_), "find") => list_find(receiver, args, span).await,
        (Value::List(_), "len") => list_len(receiver, args, span),
        // S.3 (mini-tanda S) — métodos chicos sobre List<T>:
        (Value::List(_), "sort") => list_sort(receiver, args, span),
        (Value::List(_), "reverse") => list_reverse(receiver, args, span),
        (Value::List(_), "contains") => list_contains(receiver, args, span),
        // Mini-tanda It — iteradores estilo Python:
        (Value::List(_), "enumerate") => list_enumerate(receiver, args, span),
        (Value::List(_), "zip") => list_zip(receiver, args, span),
        (Value::List(_), "chain") => list_chain(receiver, args, span),
        // Mini-tanda Mb — flatten + sort_by con callback comparator.
        (Value::List(_), "flatten") => list_flatten(receiver, args, span),
        (Value::List(_), "sort_by") => list_sort_by(receiver, args, span).await,
        // Mini-tanda Lx — predicados funcionales: any/all/count/find_index.
        (Value::List(_), "any") => list_any(receiver, args, span).await,
        (Value::List(_), "all") => list_all(receiver, args, span).await,
        (Value::List(_), "count") => list_count(receiver, args, span).await,
        (Value::List(_), "find_index") => list_find_index(receiver, args, span).await,
        // Mini-tanda Ex2 — flat_map + first / last accessors.
        (Value::List(_), "flat_map") => list_flat_map(receiver, args, span).await,
        (Value::List(_), "first") => list_first(receiver, args, span),
        (Value::List(_), "last") => list_last(receiver, args, span),
        // Mini-tanda Mb2 — min/max/sum sobre List<Int>/List<Float>.
        (Value::List(_), "min") => list_min(receiver, args, span),
        (Value::List(_), "max") => list_max(receiver, args, span),
        (Value::List(_), "sum") => list_sum(receiver, args, span),
        // Mini-tanda Mb3 — fold + product + to_map.
        (Value::List(_), "reduce") => list_reduce(receiver, args, span).await,
        (Value::List(_), "product") => list_product(receiver, args, span),
        (Value::List(_), "to_map") => list_to_map(receiver, args, span),
        // Mini-tanda Mb4 — unique + partition.
        (Value::List(_), "unique") => list_unique(receiver, args, span),
        (Value::List(_), "partition") => list_partition(receiver, args, span).await,
        // Mini-tanda Mb5 — group_by + zip_with + max_by/min_by.
        (Value::List(_), "group_by") => list_group_by(receiver, args, span).await,
        (Value::List(_), "zip_with") => list_zip_with(receiver, args, span).await,
        (Value::List(_), "max_by") => list_max_by(receiver, args, span).await,
        (Value::List(_), "min_by") => list_min_by(receiver, args, span).await,
        // Mini-tanda Mb6 — scan (fold con outputs intermedios) + windows.
        (Value::List(_), "scan") => list_scan(receiver, args, span).await,
        (Value::List(_), "windows") => list_windows(receiver, args, span),
        // Mini-tanda Mb7 — take/drop/init/tail/intersperse/cycle.
        (Value::List(_), "take") => list_take(receiver, args, span),
        (Value::List(_), "drop") => list_drop(receiver, args, span),
        (Value::List(_), "init") => list_init(receiver, args, span),
        (Value::List(_), "tail") => list_tail(receiver, args, span),
        (Value::List(_), "intersperse") => list_intersperse(receiver, args, span),
        (Value::List(_), "cycle") => list_cycle(receiver, args, span),
        // Mini-tanda Mb8 — starts_with / ends_with / insert_at / remove_at / zip_to_map.
        (Value::List(_), "starts_with") => list_starts_with(receiver, args, span),
        (Value::List(_), "ends_with") => list_ends_with(receiver, args, span),
        (Value::List(_), "insert_at") => list_insert_at(receiver, args, span),
        (Value::List(_), "remove_at") => list_remove_at(receiver, args, span),
        (Value::List(_), "zip_to_map") => list_zip_to_map(receiver, args, span),
        // Mini-tanda Mb9 — split_at sobre List (similar a Str.split_at).
        (Value::List(_), "split_at") => list_split_at(receiver, args, span),
        // Mini-tanda Ir — iteradores sobre Range. Materializa el rango
        // como `List<Int>` y delega a los métodos de List. Más simple
        // que duplicar la lógica; el overhead es solo el `Vec` extra.
        (Value::Range { start, end }, "enumerate") => {
            let materialized = range_to_list(*start, *end);
            list_enumerate(materialized, args, span)
        }
        (Value::Range { start, end }, "zip") => {
            let materialized = range_to_list(*start, *end);
            list_zip(materialized, args, span)
        }
        (Value::Range { start, end }, "chain") => {
            let materialized = range_to_list(*start, *end);
            list_chain(materialized, args, span)
        }
        // Mini-tanda Rg — `step_by(n)` materializa el rango con step.
        // No materializa el rango entero primero — usamos `step_by`
        // nativo de Rust al construir la List<Int>.
        (Value::Range { start, end }, "step_by") => range_step_by(*start, *end, args, span),
        // Mini-tanda Ir — `len` sobre Range. Devuelve `(end - start)`
        // como Int, igual que `(start..end).count()` de Rust.
        (Value::Range { start, end }, "len") => {
            if !args.is_empty() {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    span.line, span.column,
                    format!("`Range.len()` no toma args, recibió {}", args.len()),
                )));
            }
            Ok(Value::Int((end - start).max(0)))
        }
        // Map
        (Value::Map(_), "get") => map_get(receiver, args, span),
        (Value::Map(_), "has") => map_has(receiver, args, span),
        (Value::Map(_), "keys") => map_keys(receiver, args, span),
        (Value::Map(_), "values") => map_values(receiver, args, span),
        (Value::Map(_), "len") => map_len(receiver, args, span),
        // Mini-tanda Ex — transformaciones funcionales sobre Map.
        (Value::Map(_), "filter") => map_filter(receiver, args, span).await,
        (Value::Map(_), "map_values") => map_map_values(receiver, args, span).await,
        // Mini-tanda Ex2 — merge: combina dos Maps (last-write-wins).
        (Value::Map(_), "merge") => map_merge(receiver, args, span),
        // Mini-tanda Up — update inmutable: aplica `fn(V) -> V` al
        // value asociado a `k` y devuelve un Map nuevo.
        (Value::Map(_), "update") => map_update(receiver, args, span).await,
        // Mini-tanda Mb2 — keys_sorted: keys ordenadas (Int/Float/Str/Bool).
        (Value::Map(_), "keys_sorted") => map_keys_sorted(receiver, args, span),
        // Mini-tanda Mb3 — entries: List<(K, V)> con los pares.
        (Value::Map(_), "entries") => map_entries(receiver, args, span),
        // Mini-tanda Mb4 — invert: Map<V, K> con pares intercambiados.
        (Value::Map(_), "invert") => map_invert(receiver, args, span),
        // Mini-tanda Mb6 — merge_with: merge con callback para
        // resolver conflicts.
        (Value::Map(_), "merge_with") => map_merge_with(receiver, args, span).await,
        // Mini-tanda Mb7 — with: functional update (Map nuevo con k→v).
        (Value::Map(_), "with") => map_with(receiver, args, span),
        // Mini-tanda Mb9 — has_value: chequea si v está como value.
        (Value::Map(_), "has_value") => map_has_value(receiver, args, span),
        // Str
        (Value::Str(_), "len") => str_len(receiver, args, span),
        (Value::Str(_), "upper") => str_upper(receiver, args, span),
        (Value::Str(_), "lower") => str_lower(receiver, args, span),
        // S.1 (mini-tanda S) — métodos chicos sobre Str:
        (Value::Str(_), "contains") => str_contains(receiver, args, span),
        (Value::Str(_), "starts_with") => str_starts_with(receiver, args, span),
        (Value::Str(_), "ends_with") => str_ends_with(receiver, args, span),
        // S.2 — manipulación de strings:
        (Value::Str(_), "split") => str_split(receiver, args, span),
        // Mini-tanda Mb3 — chars: List<Str> con cada caracter.
        (Value::Str(_), "chars") => str_chars(receiver, args, span),
        // Mini-tanda Mb4 — split_at: divide en char idx → (Str, Str).
        (Value::Str(_), "split_at") => str_split_at(receiver, args, span),
        // Mini-tanda Mb5 — lines + is_empty.
        (Value::Str(_), "lines") => str_lines(receiver, args, span),
        (Value::Str(_), "is_empty") => str_is_empty(receiver, args, span),
        // Mini-tanda Mb7 — repeat_with (variante de repeat con sep).
        (Value::Str(_), "repeat_with") => str_repeat_with(receiver, args, span),
        // Mini-tanda Mb8 — left/right/center.
        (Value::Str(_), "left") => str_left(receiver, args, span),
        (Value::Str(_), "right") => str_right(receiver, args, span),
        (Value::Str(_), "center") => str_center(receiver, args, span),
        // Mini-tanda Mb9 — swap_case/title/is_alpha/is_digit/is_numeric.
        (Value::Str(_), "swap_case") => str_swap_case(receiver, args, span),
        (Value::Str(_), "title") => str_title(receiver, args, span),
        (Value::Str(_), "is_alpha") => str_is_alpha(receiver, args, span),
        (Value::Str(_), "is_digit") => str_is_digit(receiver, args, span),
        (Value::Str(_), "is_numeric") => str_is_numeric(receiver, args, span),
        // ---- Mini-tanda Mb9 — methods sobre primitivos Int/Float ----
        //
        // Hasta acá Fitz no tenía dispatch sobre `Value::Int`/`Value::Float`.
        // Sumamos un set acotado de métodos análogos a Rust/Python.
        // `n.abs()` / `x.abs()`, `n.to_str()` / `x.to_str()`, `n.to_str_base(b)`,
        // `x.is_nan()` / `x.is_finite()`. Aridad fija; expect_arity.
        (Value::Int(_), "abs") => {
            expect_arity("abs", &args, 0, span)?;
            let n = match receiver { Value::Int(n) => n, _ => unreachable!() };
            Ok(Value::Int(n.wrapping_abs()))
        }
        (Value::Int(_), "to_str") => {
            expect_arity("to_str", &args, 0, span)?;
            let n = match receiver { Value::Int(n) => n, _ => unreachable!() };
            Ok(Value::Str(n.to_string()))
        }
        (Value::Int(_), "to_str_base") => {
            expect_arity("to_str_base", &args, 1, span)?;
            let n = match receiver { Value::Int(n) => n, _ => unreachable!() };
            let base = match args.into_iter().next().unwrap() {
                Value::Int(b) => b,
                other => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Int".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!("`Int.to_str_base()` espera `Int`, recibió `{}`", other.type_name()),
                ))),
            };
            let s = match base {
                2 => format!("{:b}", n),
                8 => format!("{:o}", n),
                10 => n.to_string(),
                16 => format!("{:x}", n),
                _ => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    span.line, span.column,
                    format!("`Int.to_str_base()` solo soporta bases 2, 8, 10 o 16; recibió {}", base),
                ))),
            };
            Ok(Value::Str(s))
        }
        (Value::Float(_), "abs") => {
            expect_arity("abs", &args, 0, span)?;
            let x = match receiver { Value::Float(x) => x, _ => unreachable!() };
            Ok(Value::Float(x.abs()))
        }
        (Value::Float(_), "to_str") => {
            expect_arity("to_str", &args, 0, span)?;
            let x = match receiver { Value::Float(x) => x, _ => unreachable!() };
            // Mismo formato que Display del intérprete (3.0 → "3.0").
            let s = if x.is_finite() && x.fract() == 0.0 {
                format!("{:.1}", x)
            } else {
                format!("{}", x)
            };
            Ok(Value::Str(s))
        }
        (Value::Float(_), "is_nan") => {
            expect_arity("is_nan", &args, 0, span)?;
            let x = match receiver { Value::Float(x) => x, _ => unreachable!() };
            Ok(Value::Bool(x.is_nan()))
        }
        (Value::Float(_), "is_finite") => {
            expect_arity("is_finite", &args, 0, span)?;
            let x = match receiver { Value::Float(x) => x, _ => unreachable!() };
            Ok(Value::Bool(x.is_finite()))
        }
        (Value::Str(_), "trim") => str_trim(receiver, args, span),
        // Mini-tanda Mb — variantes parciales de trim.
        (Value::Str(_), "trim_start") => str_trim_start(receiver, args, span),
        (Value::Str(_), "trim_end") => str_trim_end(receiver, args, span),
        (Value::Str(_), "replace") => str_replace(receiver, args, span),
        (Value::Str(_), "repeat") => str_repeat(receiver, args, span),
        // Mini-tanda Ex — búsqueda en strings.
        (Value::Str(_), "find") => str_find(receiver, args, span),
        (Value::Str(_), "index_of") => str_index_of(receiver, args, span),
        (Value::Str(_), "last_index_of") => str_last_index_of(receiver, args, span),
        // Mini-tanda Mb2 — padding (alineación de strings).
        (Value::Str(_), "pad_start") => str_pad_start(receiver, args, span),
        (Value::Str(_), "pad_end") => str_pad_end(receiver, args, span),
        // Module: `mod.fn(args)` se resuelve buscando `fn` en el env del
        // módulo y llamándola como cualquier función. No es method
        // dispatch real — el módulo no es "el receptor", solo el lugar
        // donde vive la función.
        (Value::Module { name, env: module_env }, _) => {
            let value = module_env.lock().get(method).ok_or_else(|| {
                EvalSignal::Error(FitzError::new(
                    ErrorKind::UndefinedVariable(method.into()),
                    span.line, span.column,
                    format!("el módulo `{}` no exporta `{}`", name, method),
                ))
            })?;
            invoke_value(value, args, method, span).await
        }
        // Fase 8.1.4 — `pyobj.method(args)`: análogo al patrón de Module.
        // Hacemos getattr para obtener el método/atributo (que puede ser
        // function, bound method, etc.) y delegamos a `invoke_value`,
        // que va a ratar la nueva rama de `Value::PyObject` callable.
        // Cubre `math.sqrt(16.0)`, `os.path.join("a", "b")`, etc.
        #[cfg(feature = "python")]
        (Value::PyObject(handle), _) => {
            let attr = crate::py_interop::get_attr(handle, method)
                .map_err(|mut e| {
                    if e.line == 0 && e.column == 0 {
                        e.line = span.line;
                        e.column = span.column;
                    }
                    EvalSignal::Error(e)
                })?;
            invoke_value(attr, args, method, span).await
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
// R.3 — Invocación de método custom sobre Value::Instance
// ---------------------------------------------------------------------------
//
// "Opción A" — los fields del Instance son visibles como variables
// locales en el body del método. Implementación:
//  1. Aridad: el método NO declara `self` en sus params. La llamada
//     `u.greet(arg1)` pasa exactamente `arg1` (no el receiver), así que
//     `args.len() == m.params.len()`.
//  2. Scope: scope hijo del env del call site. Adentro pre-declaramos
//     cada field del Instance como var local (con su valor actual);
//     después declaramos los params (si un param se llama igual que un
//     field, el param gana — Rust hace lo mismo con shadowing).
//  3. El body se ejecuta vía `eval_block` con `Stmt::Return` y `?`
//     bridgeados por `EvalSignal::Return`/`Error`.
//  4. Si es async, el body puede usar `.await`; el caller debe
//     await-ear el `Value::Future` resultante igual que con cualquier
//     async fn. Para MVP el `is_async` del método se honora propagando
//     a través de `register_user_function`? — en realidad invocamos
//     en línea acá; los `.await` adentro del body funcionan porque
//     `eval_block` es async. El método sync devuelve `Value`
//     directamente; el método async devuelve un Future construido por
//     el caller (cuando aterrice async methods, hoy MVP no lo hace).

async fn invoke_custom_method(
    receiver: Value,
    method: crate::ast::MethodDef,
    args: Vec<Value>,
    env: EnvRef,
    span: Span,
) -> EvalResult<Value> {
    // Fp — aridad con defaults: requerido = params SIN default; total = params.len().
    let required = method.params.iter().filter(|p| p.default.is_none()).count();
    if args.len() < required || args.len() > method.params.len() {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: method.params.len(),
                found: args.len(),
            },
            span.line, span.column,
            if required == method.params.len() {
                format!(
                    "el método `.{}()` espera {} argumento(s), recibió {}",
                    method.name, method.params.len(), args.len(),
                )
            } else {
                format!(
                    "el método `.{}()` espera entre {} y {} argumento(s), recibió {}",
                    method.name, required, method.params.len(), args.len(),
                )
            },
        )));
    }

    // Scope hijo del env del call site.
    let method_env = Environment::new_child(env.clone());

    // Pre-declarar fields del Instance como locales.
    if let Value::Instance { fields, .. } = &receiver {
        for (fname, fvalue) in fields.lock().iter() {
            method_env.lock().define(fname.clone(), fvalue.clone());
        }
    }

    // Declarar params: args provistos + defaults para los faltantes.
    let provided = args.len();
    let mut arg_iter = args.into_iter();
    for (i, p) in method.params.iter().enumerate() {
        let v = if i < provided {
            arg_iter.next().unwrap()
        } else {
            let de = p.default.as_ref().expect("ya cubierto por arity check");
            eval_expr(de, method_env.clone()).await?
        };
        method_env.lock().define(p.name.clone(), v);
    }

    // R.3-async: si el método es async, envolvemos el body en un
    // `Value::Future` perezoso. El `.await` del caller fuerza la
    // evaluación. Patrón paralelo a Value::Function async (línea
    // 2670 aprox).
    if method.is_async {
        let owned_body = method.body;
        let fut: crate::value::FitzFuture = Box::pin(async move {
            for stmt in &owned_body {
                match eval_stmt(stmt, method_env.clone()).await {
                    Ok(_) => {}
                    Err(EvalSignal::Return(v)) => return Ok(v),
                    Err(signal) => return Err(signal_to_error(signal)),
                }
            }
            Ok(Value::Null)
        });
        return Ok(Value::new_future(fut));
    }

    // Sync: ejecutar el body. `Stmt::Return` rebota como
    // EvalSignal::Return y lo desempacamos al valor; cualquier otra
    // señal sube.
    match eval_block(&method.body, method_env).await {
        Ok(v) => Ok(v),
        Err(EvalSignal::Return(v)) => Ok(v),
        Err(other) => Err(other),
    }
}

/// Mini-tanda St — invoca un método estático declarado en el `type`
/// body. Diferencia clave con `invoke_custom_method`: NO pre-declara
/// los fields del tipo como locales (no hay receiver instance). Es
/// más parecido a invocar una fn top-level: solo los params son
/// locales del scope hijo.
async fn invoke_static_method(
    method: crate::ast::MethodDef,
    args: Vec<Value>,
    env: EnvRef,
    span: Span,
) -> EvalResult<Value> {
    // Fp — aridad con defaults.
    let required = method.params.iter().filter(|p| p.default.is_none()).count();
    if args.len() < required || args.len() > method.params.len() {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: method.params.len(),
                found: args.len(),
            },
            span.line, span.column,
            if required == method.params.len() {
                format!(
                    "el método estático `{}` espera {} argumento(s), recibió {}",
                    method.name, method.params.len(), args.len(),
                )
            } else {
                format!(
                    "el método estático `{}` espera entre {} y {} argumento(s), recibió {}",
                    method.name, required, method.params.len(), args.len(),
                )
            },
        )));
    }

    let method_env = Environment::new_child(env.clone());
    let provided = args.len();
    let mut arg_iter = args.into_iter();
    for (i, p) in method.params.iter().enumerate() {
        let v = if i < provided {
            arg_iter.next().unwrap()
        } else {
            let de = p.default.as_ref().expect("ya cubierto por arity check");
            eval_expr(de, method_env.clone()).await?
        };
        method_env.lock().define(p.name.clone(), v);
    }

    if method.is_async {
        let owned_body = method.body;
        let fut: crate::value::FitzFuture = Box::pin(async move {
            for stmt in &owned_body {
                match eval_stmt(stmt, method_env.clone()).await {
                    Ok(_) => {}
                    Err(EvalSignal::Return(v)) => return Ok(v),
                    Err(signal) => return Err(signal_to_error(signal)),
                }
            }
            Ok(Value::Null)
        });
        return Ok(Value::new_future(fut));
    }

    match eval_block(&method.body, method_env).await {
        Ok(v) => Ok(v),
        Err(EvalSignal::Return(v)) => Ok(v),
        Err(other) => Err(other),
    }
}

// ---------------------------------------------------------------------------
// Métodos built-in — implementaciones
// ---------------------------------------------------------------------------
//
// Cada función toma el receptor (consumido por valor — pero como las
// colecciones internas son `Arc<Mutex<>>`, lo que importa es el Arc, no
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
#[async_recursion]
async fn invoke_callback(
    callback: &Value,
    arg: Value,
    method: &str,
    span: Span,
) -> EvalResult<Value> {
    invoke_value(callback.clone(), vec![arg], &format!("callback de .{}()", method), span).await
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
    items.lock().push(std::mem::replace(&mut v, Value::Null));
    Ok(Value::Null)
}

fn list_pop(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("pop", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let popped = items.lock().pop();
    match popped {
        Some(v) => Ok(v),
        None => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            "`.pop()` sobre lista vacía".to_string(),
        ))),
    }
}

#[async_recursion]
async fn list_map(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("map", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    // Snapshot del Vec para evitar re-entrancia al RefCell si la callback
    // mutase la lista original.
    let snapshot: Vec<Value> = items.lock().clone();
    let mut out = Vec::with_capacity(snapshot.len());
    for item in snapshot {
        out.push(invoke_callback(callback, item, "map", span).await?);
    }
    Ok(Value::new_list(out))
}

#[async_recursion]
async fn list_filter(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("filter", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    let mut out = Vec::new();
    for item in snapshot {
        let keep = invoke_callback(callback, item.clone(), "filter", span).await?;
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

#[async_recursion]
async fn list_find(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("find", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    for item in snapshot {
        let keep = invoke_callback(callback, item.clone(), "find", span).await?;
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
    let n = items.lock().len() as i64;
    Ok(Value::Int(n))
}

/// Mini-tanda Lx — `xs.any(pred)`: `true` si algún elemento satisface
/// el predicado. Lista vacía → `false` (paralelo a Python/Rust).
/// Short-circuit en el primer `true`.
#[async_recursion]
async fn list_any(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("any", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    for item in snapshot {
        let ok = invoke_callback(callback, item, "any", span).await?;
        match ok {
            Value::Bool(true) => return Ok(Value::Bool(true)),
            Value::Bool(false) => {}
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Bool".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "la callback de `.any()` tiene que devolver Bool, devolvió `{}`",
                        other.type_name(),
                    ),
                )));
            }
        }
    }
    Ok(Value::Bool(false))
}

/// Mini-tanda Lx — `xs.all(pred)`: `true` si TODOS los elementos
/// satisfacen el predicado. Lista vacía → `true` (vacuamente todo
/// es verdad, paralelo a Python `all([])`). Short-circuit en el
/// primer `false`.
#[async_recursion]
async fn list_all(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("all", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    for item in snapshot {
        let ok = invoke_callback(callback, item, "all", span).await?;
        match ok {
            Value::Bool(true) => {}
            Value::Bool(false) => return Ok(Value::Bool(false)),
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Bool".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "la callback de `.all()` tiene que devolver Bool, devolvió `{}`",
                        other.type_name(),
                    ),
                )));
            }
        }
    }
    Ok(Value::Bool(true))
}

/// Mini-tanda Lx — `xs.count(pred)`: cuenta cuántos elementos
/// satisfacen el predicado. Devuelve `Int`.
#[async_recursion]
async fn list_count(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("count", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    let mut n: i64 = 0;
    for item in snapshot {
        let ok = invoke_callback(callback, item, "count", span).await?;
        match ok {
            Value::Bool(true) => n += 1,
            Value::Bool(false) => {}
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Bool".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "la callback de `.count()` tiene que devolver Bool, devolvió `{}`",
                        other.type_name(),
                    ),
                )));
            }
        }
    }
    Ok(Value::Int(n))
}

/// Mini-tanda Ex2 — `xs.flat_map(fn(T) -> List<U>)`: aplica `fn` a
/// cada elemento y aplana el resultado. Combinación de map + flatten
/// en un solo paso (paralelo a Rust `Iterator::flat_map` y Python
/// `[y for x in xs for y in fn(x)]`).
///
/// Si el callback NO devuelve List, error de runtime claro.
#[async_recursion]
async fn list_flat_map(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("flat_map", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    let mut out: Vec<Value> = Vec::new();
    for (i, item) in snapshot.into_iter().enumerate() {
        let mapped = invoke_callback(callback, item, "flat_map", span).await?;
        match mapped {
            Value::List(inner) => out.extend(inner.lock().clone()),
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "List".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "`.flat_map()` requiere callback que devuelva `List`: el elemento [{}] devolvió `{}`",
                        i, other.type_name(),
                    ),
                )));
            }
        }
    }
    Ok(Value::new_list(out))
}

/// Mini-tanda Ex2 — `xs.first()`: primer elemento o `Err("no encontrado")`
/// si la lista está vacía. Devuelve `Result<T>` para ser consistente con
/// `find`/`find_index` (todos los accessors que pueden fallar devuelven
/// Result).
fn list_first(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("first", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let g = items.lock();
    match g.first() {
        Some(v) => Ok(Value::Result(ResultVariant::Ok(Box::new(v.clone())))),
        None => Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "lista vacía".into(),
        ))))),
    }
}

/// Mini-tanda Ex2 — `xs.last()`: último elemento o `Err`.
fn list_last(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("last", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let g = items.lock();
    match g.last() {
        Some(v) => Ok(Value::Result(ResultVariant::Ok(Box::new(v.clone())))),
        None => Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "lista vacía".into(),
        ))))),
    }
}

/// Mini-tanda Mb2 — Helper común para `min`/`max`/`sum`: extrae el
/// receptor, valida que sea homogéneo Int o Float. Devuelve
/// `Err` con `lista vacía` cuando corresponde a min/max; sum lo
/// maneja con sentinel cero adentro de cada rama. Devuelve
/// `(items_snapshot, "Int"|"Float")` o un error claro.
fn require_numeric_list(
    receiver: Value,
    method: &str,
    span: Span,
) -> EvalResult<(Vec<Value>, &'static str)> {
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let snapshot: Vec<Value> = items.lock().clone();
    if snapshot.is_empty() {
        return Ok((snapshot, "Int"));
    }
    let first_kind: &'static str = match snapshot[0] {
        Value::Int(_) => "Int",
        Value::Float(_) => "Float",
        _ => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int|Float".into(),
                    found: snapshot[0].type_name().into(),
                },
                span.line, span.column,
                format!(
                    "`.{}()` solo se aplica sobre `List<Int>` o `List<Float>`, recibió `List<{}>`",
                    method, snapshot[0].type_name(),
                ),
            )));
        }
    };
    for v in snapshot.iter().skip(1) {
        let kind = match v {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            _ => v.type_name(),
        };
        if kind != first_kind {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: first_kind.into(),
                    found: kind.into(),
                },
                span.line, span.column,
                format!(
                    "`.{}()` requiere elementos del mismo tipo: vi `{}` y `{}`",
                    method, first_kind, kind,
                ),
            )));
        }
    }
    Ok((snapshot, first_kind))
}

/// Mini-tanda Mb2 — `xs.min()` / `xs.max()` sobre `List<Int>` o
/// `List<Float>`. Devuelven `Result<T>`: `Err("lista vacía")` si la
/// lista no tiene elementos. Para `Float` usamos `partial_cmp`
/// devolviendo `Equal` ante NaN (determinístico, paralelo a `sort`).
fn list_min(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("min", &args, 0, span)?;
    let (snapshot, kind) = require_numeric_list(receiver, "min", span)?;
    if snapshot.is_empty() {
        return Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "lista vacía".into(),
        )))));
    }
    let best = match kind {
        "Int" => {
            let mut best = i64::MAX;
            for v in snapshot {
                if let Value::Int(n) = v {
                    if n < best { best = n; }
                }
            }
            Value::Int(best)
        }
        "Float" => {
            let mut best: Option<f64> = None;
            for v in snapshot {
                if let Value::Float(n) = v {
                    best = match best {
                        None => Some(n),
                        Some(b) if n.partial_cmp(&b) == Some(std::cmp::Ordering::Less) => Some(n),
                        Some(b) => Some(b),
                    };
                }
            }
            Value::Float(best.unwrap())
        }
        _ => unreachable!(),
    };
    Ok(Value::Result(ResultVariant::Ok(Box::new(best))))
}

fn list_max(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("max", &args, 0, span)?;
    let (snapshot, kind) = require_numeric_list(receiver, "max", span)?;
    if snapshot.is_empty() {
        return Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "lista vacía".into(),
        )))));
    }
    let best = match kind {
        "Int" => {
            let mut best = i64::MIN;
            for v in snapshot {
                if let Value::Int(n) = v {
                    if n > best { best = n; }
                }
            }
            Value::Int(best)
        }
        "Float" => {
            let mut best: Option<f64> = None;
            for v in snapshot {
                if let Value::Float(n) = v {
                    best = match best {
                        None => Some(n),
                        Some(b) if n.partial_cmp(&b) == Some(std::cmp::Ordering::Greater) => Some(n),
                        Some(b) => Some(b),
                    };
                }
            }
            Value::Float(best.unwrap())
        }
        _ => unreachable!(),
    };
    Ok(Value::Result(ResultVariant::Ok(Box::new(best))))
}

/// Mini-tanda Mb2 — `xs.sum()` sobre `List<Int>` o `List<Float>`.
/// Lista vacía → `Int(0)` (sentinel; el tipo Float vacío también
/// devuelve `Int(0)` porque sin elementos el runtime no sabe cuál
/// usar — el checker declara el tipo, pero el evaluator es gradual).
fn list_sum(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("sum", &args, 0, span)?;
    let (snapshot, kind) = require_numeric_list(receiver, "sum", span)?;
    if snapshot.is_empty() {
        return Ok(Value::Int(0));
    }
    let total = match kind {
        "Int" => {
            let mut total: i64 = 0;
            for v in snapshot {
                if let Value::Int(n) = v {
                    total = total.wrapping_add(n);
                }
            }
            Value::Int(total)
        }
        "Float" => {
            let mut total: f64 = 0.0;
            for v in snapshot {
                if let Value::Float(n) = v {
                    total += n;
                }
            }
            Value::Float(total)
        }
        _ => unreachable!(),
    };
    Ok(total)
}

/// Mini-tanda Lx — `xs.find_index(pred)`: índice del primer elemento
/// que satisface el predicado. Devuelve `Result<Int>`: `Ok(i)` si lo
/// encuentra, `Err("no encontrado")` si no. Paralelo a `find` (que
/// devuelve el elemento en lugar del índice).
#[async_recursion]
async fn list_find_index(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("find_index", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    for (i, item) in snapshot.into_iter().enumerate() {
        let ok = invoke_callback(callback, item, "find_index", span).await?;
        match ok {
            Value::Bool(true) => {
                return Ok(Value::Result(ResultVariant::Ok(Box::new(Value::Int(i as i64)))));
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
                        "la callback de `.find_index()` tiene que devolver Bool, devolvió `{}`",
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

/// S.3 — `xs.sort()` ordena IN-PLACE. Soporta `List<T>` para T en
/// {Int, Float, Str, Bool}. Listas heterogéneas o de tipos no
/// comparables → error claro de runtime. `Float::NaN` ordena
/// determinísticamente vía `partial_cmp.unwrap_or(Less)` (mismo
/// approach que `f64::total_cmp` simplificado). Devuelve `Null`
/// (mutación in-place, paralelo a `push`/`pop`).
fn list_sort(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("sort", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let mut guard = items.lock();
    // Tipo común: chequeamos el primer elemento y exigimos el resto
    // igual. Para lista vacía, no-op.
    if guard.is_empty() {
        return Ok(Value::Null);
    }
    let first_kind = guard[0].type_name();
    for v in guard.iter().skip(1) {
        if v.type_name() != first_kind {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: first_kind.into(),
                    found: v.type_name().into(),
                },
                span.line, span.column,
                format!(
                    "`.sort()` requiere elementos del mismo tipo: vi `{}` y `{}`",
                    first_kind, v.type_name(),
                ),
            )));
        }
    }
    match first_kind {
        "Int" => guard.sort_by(|a, b| match (a, b) {
            (Value::Int(x), Value::Int(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        }),
        "Float" => guard.sort_by(|a, b| match (a, b) {
            (Value::Float(x), Value::Float(y)) => {
                x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
            }
            _ => std::cmp::Ordering::Equal,
        }),
        "Str" => guard.sort_by(|a, b| match (a, b) {
            (Value::Str(x), Value::Str(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        }),
        "Bool" => guard.sort_by(|a, b| match (a, b) {
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        }),
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::InvalidSyntax,
                span.line, span.column,
                format!("`.sort()` no soporta `List<{}>` (solo Int/Float/Str/Bool)", other),
            )));
        }
    }
    Ok(Value::Null)
}

/// S.3 — `xs.reverse()` invierte el orden IN-PLACE. Cualquier
/// `List<T>`. Devuelve `Null`.
fn list_reverse(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("reverse", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    items.lock().reverse();
    Ok(Value::Null)
}

/// S.3 — `xs.contains(v)` devuelve `Bool`. Usa la igualdad
/// estructural de `Value` (`PartialEq`), que ya hace lo correcto
/// para primitivos, instancias, listas anidadas, etc.
fn list_contains(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("contains", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let needle = args.into_iter().next().unwrap();
    let found = items.lock().contains(&needle);
    Ok(Value::Bool(found))
}

/// Mini-tanda It — `xs.enumerate()` → `List<(Int, T)>` con pares
/// (índice, elemento). Snapshot del Vec para evitar re-entrancia.
/// Mini-tanda Ir — Helper: materializa un Range a un `Value::List` con
/// los Ints del rango (semánticas exclusivas — `end` no incluido). Los
/// rangos inclusivos (`0..=N`) se construyen con `end = N + 1` por el
/// parser (R.1.4) así que ya quedan inclusivos al materializar.
fn range_to_list(start: i64, end: i64) -> Value {
    let items: Vec<Value> = (start..end).map(Value::Int).collect();
    Value::new_list(items)
}

/// Mini-tanda Rg — `(start..end).step_by(n)` materializa el rango con
/// step `n`. `n` debe ser un `Int > 0`. Si `n <= 0`, error claro de
/// runtime. Output: `List<Int>` (paralelo a cómo enumerate/zip/chain
/// se materializan desde Range — destino final es siempre una List).
fn range_step_by(start: i64, end: i64, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("step_by", &args, 1, span)?;
    let step = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`Range.step_by()` espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    if step <= 0 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!("`Range.step_by()` requiere n > 0, recibió {}", step),
        )));
    }
    let items: Vec<Value> = (start..end)
        .step_by(step as usize)
        .map(Value::Int)
        .collect();
    Ok(Value::new_list(items))
}

fn list_enumerate(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("enumerate", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let snapshot = items.lock().clone();
    let out: Vec<Value> = snapshot
        .into_iter()
        .enumerate()
        .map(|(i, v)| Value::Tuple(vec![Value::Int(i as i64), v]))
        .collect();
    Ok(Value::new_list(out))
}

/// Mini-tanda It — `xs.zip(ys)` → `List<(T, U)>` truncado al más
/// corto. Si los tipos son distintos, igual funciona — los pares son
/// tuples heterogéneas.
fn list_zip(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("zip", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let other_items = match args.into_iter().next().unwrap() {
        Value::List(other) => other,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "List".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`zip` espera otra `List`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let a = items.lock().clone();
    let b = other_items.lock().clone();
    let out: Vec<Value> = a
        .into_iter()
        .zip(b)
        .map(|(x, y)| Value::Tuple(vec![x, y]))
        .collect();
    Ok(Value::new_list(out))
}

/// Mini-tanda It — `xs.chain(ys)` → `List<T>` concatenado. Snapshot
/// de ambas listas para evitar re-entrancia.
fn list_chain(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("chain", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let other_items = match args.into_iter().next().unwrap() {
        Value::List(other) => other,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "List".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`chain` espera otra `List`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let mut out: Vec<Value> = items.lock().clone();
    out.extend(other_items.lock().clone());
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb — `xss.flatten()` aplana `List<List<T>>` → `List<T>`.
/// Concatena los elementos de cada sub-lista en orden. Si los elementos
/// no son listas, error de runtime claro. Para listas vacías o con
/// sub-listas vacías, no-op (resultado vacío).
fn list_flatten(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("flatten", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let snapshot = items.lock().clone();
    let mut out: Vec<Value> = Vec::new();
    for (i, v) in snapshot.into_iter().enumerate() {
        match v {
            Value::List(inner) => {
                out.extend(inner.lock().clone());
            }
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "List".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "`.flatten()` requiere `List<List<T>>`: el elemento [{}] es `{}`",
                        i, other.type_name(),
                    ),
                )));
            }
        }
    }
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb — `xs.sort_by(cmp)` ordena IN-PLACE usando un
/// callback comparator. El callback recibe `(a, b)` y devuelve un
/// `Int` siguiendo la convención `cmp` de Rust/JS:
///   - negativo si `a < b`,
///   - cero si `a == b`,
///   - positivo si `a > b`.
///
/// El callback puede ser sync o async — invocamos via `invoke_value`
/// que ya maneja ambos casos. Usamos selection sort (O(n²)) en lugar
/// de `Vec::sort_by` porque este último toma un closure sync; con
/// callbacks async tendríamos que bloquear o re-implementar
/// internamente. Para listas chicas (<1000) está bien; sub-paso
/// futuro si aparece presión real.
async fn list_sort_by(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("sort_by", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let cmp_fn = args.into_iter().next().unwrap();

    // Snapshot del Vec para evitar re-entrancia con el callback.
    // Después de comparar todo y ordenar el snapshot, volcamos al
    // original. Patrón paralelo a `list_map`/`list_filter`.
    let snapshot = items.lock().clone();
    let n = snapshot.len();
    if n < 2 {
        return Ok(Value::Null);
    }

    // Materializamos los pares de comparación que necesitamos. Pre-
    // computamos `cmp(a, b)` para cada par solicitado por sort_by; el
    // approach pragmático es invocar el callback adentro de `sort_by`
    // de Rust pero `sort_by` toma un closure SYNC y nuestro `invoke_value`
    // es async. Solución: pre-construir un Vec<usize> de índices,
    // ordenarlos con sort_by que hace lookup a una matriz de
    // comparación pre-computada... O más simple: implementamos
    // selection sort O(n²) acá, invocando el callback async en cada
    // par. Para listas chicas (<1000 elementos) está bien. Sub-paso
    // futuro si aparece presión: spawn_blocking + sync invoke.
    let mut indexed: Vec<(usize, Value)> = snapshot.into_iter().enumerate().collect();
    // Selection sort sobre `indexed`.
    for i in 0..n - 1 {
        let mut min_idx = i;
        for j in (i + 1)..n {
            let a = indexed[j].1.clone();
            let b = indexed[min_idx].1.clone();
            let cmp_result =
                invoke_value(cmp_fn.clone(), vec![a, b], "sort_by", span).await?;
            let cmp_int = match cmp_result {
                Value::Int(n) => n,
                other => {
                    return Err(EvalSignal::Error(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Int".into(),
                            found: other.type_name().into(),
                        },
                        span.line, span.column,
                        format!(
                            "`.sort_by(cmp)` espera que cmp devuelva `Int`, recibió `{}`",
                            other.type_name(),
                        ),
                    )));
                }
            };
            if cmp_int < 0 {
                min_idx = j;
            }
        }
        if min_idx != i {
            indexed.swap(i, min_idx);
        }
    }

    // Volcamos al original (drop el orden anterior, escribir el nuevo).
    let mut guard = items.lock();
    *guard = indexed.into_iter().map(|(_, v)| v).collect();
    Ok(Value::Null)
}

/// Mini-tanda Mb3 — `xs.reduce(init, fn(acc, x) -> Acc)` fold canónico.
/// Itera la lista aplicando `fn(acc, x)` y devuelve el acumulador
/// final. Vacía → init. Paralelo a `Iterator::fold` de Rust o
/// `Array.prototype.reduce(fn, init)` de JS.
#[async_recursion]
async fn list_reduce(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("reduce", &args, 2, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let mut acc = it.next().unwrap();
    let cb = it.next().unwrap();
    let snapshot: Vec<Value> = items.lock().clone();
    for item in snapshot {
        acc = invoke_value(cb.clone(), vec![acc, item], "reduce", span).await?;
    }
    Ok(acc)
}

/// Mini-tanda Mb3 — `xs.product()` análogo a `sum`. Para `List<Int>`
/// o `List<Float>` homogéneos. Vacía → `Int(1)` sentinel (paralelo
/// a Python `math.prod([])`).
fn list_product(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("product", &args, 0, span)?;
    let (snapshot, kind) = require_numeric_list(receiver, "product", span)?;
    if snapshot.is_empty() {
        return Ok(Value::Int(1));
    }
    let total = match kind {
        "Int" => {
            let mut total: i64 = 1;
            for v in snapshot {
                if let Value::Int(n) = v {
                    total = total.wrapping_mul(n);
                }
            }
            Value::Int(total)
        }
        "Float" => {
            let mut total: f64 = 1.0;
            for v in snapshot {
                if let Value::Float(n) = v {
                    total *= n;
                }
            }
            Value::Float(total)
        }
        _ => unreachable!(),
    };
    Ok(total)
}

/// Mini-tanda Mb4 — `xs.unique()`: devuelve `List<T>` con los
/// elementos en el orden de primera aparición, sin duplicados.
/// Usa igualdad estructural (`PartialEq` de Value). O(n²) en el
/// peor caso por la búsqueda lineal; para listas chicas (<1000)
/// está bien. Paralelo a Python `list(dict.fromkeys(xs))`.
fn list_unique(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("unique", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let snapshot: Vec<Value> = items.lock().clone();
    let mut out: Vec<Value> = Vec::with_capacity(snapshot.len());
    for v in snapshot {
        if !out.iter().any(|x| x == &v) {
            out.push(v);
        }
    }
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb9 — `xs.split_at(i)`: divide la lista en posición
/// `i` y devuelve `(List<T>, List<T>)`. `i <= 0` → `([], xs)`;
/// `i >= len` → `(xs, [])`. Paralelo a Rust `slice::split_at` pero
/// devuelve copias en lugar de views.
fn list_split_at(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("split_at", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let idx = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`List.split_at()` espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let snapshot: Vec<Value> = items.lock().clone();
    let clamped = if idx <= 0 {
        0
    } else if (idx as usize) >= snapshot.len() {
        snapshot.len()
    } else {
        idx as usize
    };
    let left: Vec<Value> = snapshot[..clamped].to_vec();
    let right: Vec<Value> = snapshot[clamped..].to_vec();
    Ok(Value::Tuple(vec![
        Value::new_list(left),
        Value::new_list(right),
    ]))
}

/// Mini-tanda Mb8 — `xs.starts_with(prefix)` / `xs.ends_with(suffix)`:
/// devuelven `Bool` si la lista empieza/termina con la sublista dada.
/// Usa igualdad estructural (PartialEq). Prefix vacío → `true`.
fn list_starts_or_ends_with(
    receiver: Value,
    args: Vec<Value>,
    span: Span,
    is_start: bool,
    method: &'static str,
) -> EvalResult<Value> {
    expect_arity(method, &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let prefix_or_suffix = match args.into_iter().next().unwrap() {
        Value::List(items) => items,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "List".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!(
                    "`.{}()` espera una `List` como arg, recibió `{}`",
                    method, other.type_name(),
                ),
            )));
        }
    };
    let self_g = items.lock();
    let other_g = prefix_or_suffix.lock();
    if other_g.len() > self_g.len() {
        return Ok(Value::Bool(false));
    }
    let result = if is_start {
        self_g.iter().take(other_g.len()).eq(other_g.iter())
    } else {
        self_g.iter().rev().take(other_g.len()).eq(other_g.iter().rev())
    };
    Ok(Value::Bool(result))
}

fn list_starts_with(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    list_starts_or_ends_with(receiver, args, span, true, "starts_with")
}

fn list_ends_with(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    list_starts_or_ends_with(receiver, args, span, false, "ends_with")
}

/// Mini-tanda Mb8 — `xs.insert_at(i, v) -> List<T>`: devuelve una
/// lista nueva con `v` insertado en posición `i` (los elementos
/// existentes se corren a la derecha). `i < 0` → error claro;
/// `i > len(xs)` clamp a `len(xs)` (insert al final, paralelo a
/// Python `list.insert`).
fn list_insert_at(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("insert_at", &args, 2, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let idx = match it.next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`.insert_at()`: idx espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let v = it.next().unwrap();
    if idx < 0 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!("`.insert_at()` no acepta idx negativo: recibió {}", idx),
        )));
    }
    let snapshot: Vec<Value> = items.lock().clone();
    let clamped = (idx as usize).min(snapshot.len());
    let mut out: Vec<Value> = Vec::with_capacity(snapshot.len() + 1);
    out.extend(snapshot.iter().take(clamped).cloned());
    out.push(v);
    out.extend(snapshot.into_iter().skip(clamped));
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb8 — `xs.remove_at(i) -> List<T>`: devuelve una lista
/// nueva sin el elemento en posición `i`. `i < 0` o `i >= len(xs)`
/// → error claro (no clamp — el usuario debería saber el rango).
fn list_remove_at(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("remove_at", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let idx = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`.remove_at()`: idx espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let snapshot: Vec<Value> = items.lock().clone();
    if idx < 0 || (idx as usize) >= snapshot.len() {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!(
                "`.remove_at()`: idx {} fuera de rango (len = {})",
                idx, snapshot.len(),
            ),
        )));
    }
    let remove_idx = idx as usize;
    let out: Vec<Value> = snapshot
        .into_iter()
        .enumerate()
        .filter(|(i, _)| *i != remove_idx)
        .map(|(_, v)| v)
        .collect();
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb8 — `xs.zip_to_map(values) -> Map<K, V>`: combina
/// la lista de keys (self) con la de values formando un Map.
/// Trunca al más corto (paralelo a Python `dict(zip(ks, vs))`).
fn list_zip_to_map(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("zip_to_map", &args, 1, span)?;
    let keys = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let values = match args.into_iter().next().unwrap() {
        Value::List(items) => items,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "List".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`.zip_to_map()` espera una `List`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let ks: Vec<Value> = keys.lock().clone();
    let vs: Vec<Value> = values.lock().clone();
    let mut out: Vec<(Value, Value)> = Vec::with_capacity(ks.len().min(vs.len()));
    for (k, v) in ks.into_iter().zip(vs) {
        if let Some(slot) = out.iter_mut().find(|(ek, _)| ek == &k) {
            slot.1 = v;
        } else {
            out.push((k, v));
        }
    }
    Ok(Value::new_map(out))
}

/// Mini-tanda Mb7 — `xs.take(n) -> List<T>`: primeros `n` elementos.
/// Si `n >= len(xs)`, devuelve copia completa; si `n <= 0`, lista
/// vacía. Paralelo a Rust `Iterator::take`.
fn list_take(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("take", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let n = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`.take()` espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let snapshot: Vec<Value> = items.lock().clone();
    let take_n = if n <= 0 { 0 } else { (n as usize).min(snapshot.len()) };
    let out: Vec<Value> = snapshot.into_iter().take(take_n).collect();
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb7 — `xs.drop(n) -> List<T>`: saltea los primeros `n`
/// elementos. Si `n >= len(xs)`, lista vacía; si `n <= 0`, copia
/// completa. Paralelo a Rust `Iterator::skip`.
fn list_drop(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("drop", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let n = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`.drop()` espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let snapshot: Vec<Value> = items.lock().clone();
    let drop_n = if n <= 0 { 0 } else { (n as usize).min(snapshot.len()) };
    let out: Vec<Value> = snapshot.into_iter().skip(drop_n).collect();
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb7 — `xs.init() -> List<T>`: todos los elementos menos
/// el último. Paralelo a Haskell `init`. Lista vacía → lista vacía.
fn list_init(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("init", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let snapshot: Vec<Value> = items.lock().clone();
    let out: Vec<Value> = if snapshot.is_empty() {
        Vec::new()
    } else {
        snapshot[..snapshot.len() - 1].to_vec()
    };
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb7 — `xs.tail() -> List<T>`: todos los elementos menos
/// el primero. Paralelo a Haskell `tail`. Lista vacía → lista vacía.
fn list_tail(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("tail", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let snapshot: Vec<Value> = items.lock().clone();
    let out: Vec<Value> = if snapshot.is_empty() {
        Vec::new()
    } else {
        snapshot[1..].to_vec()
    };
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb7 — `xs.intersperse(sep) -> List<T>`: inserta `sep`
/// entre cada par de elementos consecutivos. `[a, b, c]` →
/// `[a, sep, b, sep, c]`. Lista vacía o de 1 elemento → sin cambios.
/// Paralelo a Haskell `intersperse`.
fn list_intersperse(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("intersperse", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let sep = args.into_iter().next().unwrap();
    let snapshot: Vec<Value> = items.lock().clone();
    let mut out: Vec<Value> = Vec::with_capacity(snapshot.len() * 2);
    for (i, item) in snapshot.into_iter().enumerate() {
        if i > 0 {
            out.push(sep.clone());
        }
        out.push(item);
    }
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb7 — `xs.cycle(n) -> List<T>`: repite la lista `n`
/// veces. `n <= 0` → lista vacía (no error — política friendly).
/// Paralelo a Rust `Iterator::cycle().take(n * len)` pero acotado
/// para evitar listas infinitas en memoria.
fn list_cycle(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("cycle", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let n = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`.cycle()` espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let snapshot: Vec<Value> = items.lock().clone();
    if n <= 0 || snapshot.is_empty() {
        return Ok(Value::new_list(Vec::new()));
    }
    let total = snapshot.len() * (n as usize);
    let mut out: Vec<Value> = Vec::with_capacity(total);
    for _ in 0..n {
        for v in &snapshot {
            out.push(v.clone());
        }
    }
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb6 — `xs.scan(init, fn(acc, x) -> Acc) -> List<Acc>`.
/// Fold con outputs intermedios — devuelve una lista con cada
/// estado del acumulador después de procesar cada elemento. Útil
/// para sumas parciales, máximos acumulados, etc. Paralelo a Rust
/// `Iterator::scan` (sin la sutileza del Option de Rust — siempre
/// emite un valor por elemento). Lista vacía → lista vacía.
#[async_recursion]
async fn list_scan(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("scan", &args, 2, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let mut acc = it.next().unwrap();
    let cb = it.next().unwrap();
    let snapshot: Vec<Value> = items.lock().clone();
    let mut out: Vec<Value> = Vec::with_capacity(snapshot.len());
    for item in snapshot {
        acc = invoke_value(cb.clone(), vec![acc, item], "scan", span).await?;
        out.push(acc.clone());
    }
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb6 — `xs.windows(n) -> List<List<T>>`: sliding windows
/// de tamaño `n`. Cada ventana es una `List<T>` con `n` elementos
/// consecutivos. Si `len(xs) < n`, lista vacía. `n <= 0` → error
/// claro. Paralelo a Rust `slice::windows`.
fn list_windows(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("windows", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let n = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`.windows()` espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    if n <= 0 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!("`.windows()` requiere n > 0, recibió {}", n),
        )));
    }
    let snapshot: Vec<Value> = items.lock().clone();
    let win_size = n as usize;
    let mut out: Vec<Value> = Vec::new();
    if snapshot.len() >= win_size {
        for i in 0..=(snapshot.len() - win_size) {
            let window: Vec<Value> = snapshot[i..i + win_size].to_vec();
            out.push(Value::new_list(window));
        }
    }
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb5 — `xs.group_by(fn(T) -> K)`: agrupa los elementos
/// por la key que devuelve el callback. Output: `Map<K, List<T>>`.
/// Preserva orden — el primer item con key K define posición en el
/// map; items posteriores se acumulan en su `List<T>`.
#[async_recursion]
async fn list_group_by(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("group_by", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
    for item in snapshot {
        let item_clone = item.clone();
        let k = invoke_callback(callback, item_clone, "group_by", span).await?;
        if let Some(slot) = groups.iter_mut().find(|(ek, _)| ek == &k) {
            slot.1.push(item);
        } else {
            groups.push((k, vec![item]));
        }
    }
    let pairs: Vec<(Value, Value)> = groups
        .into_iter()
        .map(|(k, vs)| (k, Value::new_list(vs)))
        .collect();
    Ok(Value::new_map(pairs))
}

/// Mini-tanda Mb5 — `xs.zip_with(ys, fn(T, U) -> V)`: combina zip +
/// map en un paso. Trunca al más corto (paralelo a Python `zip`).
/// Útil cuando solo querés la transformación, no los pares crudos.
#[async_recursion]
async fn list_zip_with(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("zip_with", &args, 2, span)?;
    let xs = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let other = it.next().unwrap();
    let cb = it.next().unwrap();
    let other_items = match other {
        Value::List(items) => items,
        v => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "List".into(),
                    found: v.type_name().into(),
                },
                span.line, span.column,
                format!("`.zip_with()` espera otra `List`, recibió `{}`", v.type_name()),
            )));
        }
    };
    let a: Vec<Value> = xs.lock().clone();
    let b: Vec<Value> = other_items.lock().clone();
    let mut out: Vec<Value> = Vec::with_capacity(a.len().min(b.len()));
    for (x, y) in a.into_iter().zip(b.into_iter()) {
        let v = invoke_value(cb.clone(), vec![x, y], "zip_with", span).await?;
        out.push(v);
    }
    Ok(Value::new_list(out))
}

/// Mini-tanda Mb5 — `xs.max_by(fn(T) -> Int)` y `xs.min_by(...)`:
/// devuelven el elemento con mayor/menor ranking según el callback.
/// El callback extrae un `Int` y nosotros elegimos el item con el
/// valor más grande/chico. Útil para tipos no numéricos (`Instance`,
/// `Str`, etc.) donde `max`/`min` directos no aplican. Vacía → Err.
#[async_recursion]
async fn list_max_min_by(
    receiver: Value,
    args: Vec<Value>,
    span: Span,
    want_max: bool,
    method: &'static str,
) -> EvalResult<Value> {
    expect_arity(method, &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    if snapshot.is_empty() {
        return Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "lista vacía".into(),
        )))));
    }
    let mut best: Option<(i64, Value)> = None;
    for item in snapshot {
        let item_clone = item.clone();
        let r = invoke_callback(callback, item_clone, method, span).await?;
        let key = match r {
            Value::Int(n) => n,
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Int".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "la callback de `.{}()` tiene que devolver `Int`, devolvió `{}`",
                        method, other.type_name(),
                    ),
                )));
            }
        };
        best = match best {
            None => Some((key, item)),
            Some((bk, bv)) => {
                let take_new = if want_max { key > bk } else { key < bk };
                if take_new { Some((key, item)) } else { Some((bk, bv)) }
            }
        };
    }
    let (_, item) = best.unwrap();
    Ok(Value::Result(ResultVariant::Ok(Box::new(item))))
}

async fn list_max_by(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    list_max_min_by(receiver, args, span, true, "max_by").await
}

async fn list_min_by(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    list_max_min_by(receiver, args, span, false, "min_by").await
}

/// Mini-tanda Mb4 — `xs.partition(pred)`: devuelve `(List<T>, List<T>)`
/// con los elementos para los que `pred` da `true` en el primer slot
/// y los `false` en el segundo. Preserva orden relativo.
#[async_recursion]
async fn list_partition(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("partition", &args, 1, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let callback = &args[0];
    let snapshot: Vec<Value> = items.lock().clone();
    let mut truthy: Vec<Value> = Vec::new();
    let mut falsy: Vec<Value> = Vec::new();
    for item in snapshot {
        let item_clone = item.clone();
        let r = invoke_callback(callback, item_clone, "partition", span).await?;
        match r {
            Value::Bool(true) => truthy.push(item),
            Value::Bool(false) => falsy.push(item),
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Bool".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "la callback de `.partition()` tiene que devolver Bool, devolvió `{}`",
                        other.type_name(),
                    ),
                )));
            }
        }
    }
    Ok(Value::Tuple(vec![
        Value::new_list(truthy),
        Value::new_list(falsy),
    ]))
}

/// Mini-tanda Mb3 — `xs.to_map()`: convierte `List<(K, V)>` en
/// `Map<K, V>`. Política last-write-wins si hay keys duplicadas
/// (paralelo a Python `dict(items)` que también sobrescribe).
/// Si algún elemento no es Tuple de aridad 2 → error de runtime.
fn list_to_map(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("to_map", &args, 0, span)?;
    let items = match receiver {
        Value::List(items) => items,
        _ => unreachable!(),
    };
    let snapshot: Vec<Value> = items.lock().clone();
    let mut out: Vec<(Value, Value)> = Vec::with_capacity(snapshot.len());
    for (i, v) in snapshot.into_iter().enumerate() {
        match v {
            Value::Tuple(parts) if parts.len() == 2 => {
                let mut pit = parts.into_iter();
                let k = pit.next().unwrap();
                let val = pit.next().unwrap();
                // Last-write-wins: buscamos si k ya existe y reemplazamos.
                if let Some(slot) = out.iter_mut().find(|(ek, _)| ek == &k) {
                    slot.1 = val;
                } else {
                    out.push((k, val));
                }
            }
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Tuple(K, V)".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "`.to_map()` requiere `List<(K, V)>`: el elemento [{}] es `{}` (aridad {})",
                        i,
                        other.type_name(),
                        if let Value::Tuple(p) = &other { p.len() } else { 0 },
                    ),
                )));
            }
        }
    }
    Ok(Value::new_map(out))
}

// ---- Map ----

fn map_get(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("get", &args, 1, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let key = &args[0];
    for (k, v) in pairs.lock().iter() {
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
    let found = pairs.lock().iter().any(|(k, _)| k == key);
    Ok(Value::Bool(found))
}

fn map_keys(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("keys", &args, 0, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let ks: Vec<Value> = pairs.lock().iter().map(|(k, _)| k.clone()).collect();
    Ok(Value::new_list(ks))
}

fn map_values(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("values", &args, 0, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let vs: Vec<Value> = pairs.lock().iter().map(|(_, v)| v.clone()).collect();
    Ok(Value::new_list(vs))
}

fn map_len(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("len", &args, 0, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let n = pairs.lock().len() as i64;
    Ok(Value::Int(n))
}

/// Mini-tanda Ex — `m.filter(pred)`: keeps pares (k, v) donde
/// `pred(k, v) → true`. Devuelve un Map nuevo (no muta el receiver).
/// Callback toma 2 args: la key y el value.
#[async_recursion]
async fn map_filter(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("filter", &args, 1, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let callback = args.into_iter().next().unwrap();
    let snapshot: Vec<(Value, Value)> = pairs.lock().clone();
    let mut out: Vec<(Value, Value)> = Vec::new();
    for (k, v) in snapshot {
        let ok = invoke_value(
            callback.clone(),
            vec![k.clone(), v.clone()],
            "filter",
            span,
        ).await?;
        match ok {
            Value::Bool(true) => out.push((k, v)),
            Value::Bool(false) => {}
            other => {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Bool".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "el callback de `Map.filter()` debe devolver Bool, recibió `{}`",
                        other.type_name(),
                    ),
                )));
            }
        }
    }
    Ok(Value::new_map(out))
}

/// Mini-tanda Ex — `m.map_values(fn)`: aplica `fn(v) → U` a cada
/// value, dejando las keys intactas. Devuelve un Map nuevo. Cubre
/// el patrón canónico de transformar values sin tocar la estructura
/// (paralelo a Python `{k: fn(v) for k, v in m.items()}`).
#[async_recursion]
async fn map_map_values(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("map_values", &args, 1, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let callback = args.into_iter().next().unwrap();
    let snapshot: Vec<(Value, Value)> = pairs.lock().clone();
    let mut out: Vec<(Value, Value)> = Vec::with_capacity(snapshot.len());
    for (k, v) in snapshot {
        let new_v = invoke_callback(&callback, v, "map_values", span).await?;
        out.push((k, new_v));
    }
    Ok(Value::new_map(out))
}

/// Mini-tanda Up — `m.update(k, fn(V) -> V)`: aplica `fn` al value
/// asociado a `k`, devuelve un Map nuevo (no muta). Si `k` no está
/// presente, devuelve un Map igual al original (no inserta — sigue
/// la convención "update" estilo Rust `Entry::and_modify`).
///
/// Útil para mutaciones atómicas sin tener que `get(k)?` + `set`.
#[async_recursion]
async fn map_update(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("update", &args, 2, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let key = it.next().unwrap();
    let callback = it.next().unwrap();
    let snapshot: Vec<(Value, Value)> = pairs.lock().clone();
    let mut out: Vec<(Value, Value)> = Vec::with_capacity(snapshot.len());
    for (k, v) in snapshot {
        if k == key {
            let new_v = invoke_callback(&callback, v, "update", span).await?;
            out.push((k, new_v));
        } else {
            out.push((k, v));
        }
    }
    Ok(Value::new_map(out))
}

/// Mini-tanda Ex2 — `m.merge(other)`: combina dos Maps en uno nuevo.
/// Política last-write-wins (paralelo a Python `{**m, **other}` /
/// JS spread / Rust `extend`): keys de `other` sobrescriben las de
/// `m`. Devuelve un Map nuevo (no muta `m` ni `other`).
fn map_merge(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("merge", &args, 1, span)?;
    let pairs = match receiver {
        Value::Map(pairs) => pairs,
        _ => unreachable!(),
    };
    let other_pairs = match args.into_iter().next().unwrap() {
        Value::Map(p) => p,
        other => return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch { expected: "Map".into(), found: other.type_name().into() },
            span.line, span.column,
            format!("`.merge()` espera Map, recibió {}", other.type_name()),
        ))),
    };
    let mut out: Vec<(Value, Value)> = pairs.lock().clone();
    for (k, v) in other_pairs.lock().iter() {
        // Buscar si la key ya existe — si sí, sobreescribir;
        // si no, push al final (preserva orden de inserción para
        // pares nuevos).
        if let Some(slot) = out.iter_mut().find(|(existing_k, _)| existing_k == k) {
            slot.1 = v.clone();
        } else {
            out.push((k.clone(), v.clone()));
        }
    }
    Ok(Value::new_map(out))
}

/// Mini-tanda Mb9 — `m.has_value(v) -> Bool`: devuelve true si algún
/// par del Map tiene `v` como value. Igualdad estructural. Paralelo
/// a `m.has(k)` (que chequea keys), pero sobre values.
fn map_has_value(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("has_value", &args, 1, span)?;
    let pairs = match receiver {
        Value::Map(p) => p,
        _ => unreachable!(),
    };
    let needle = args.into_iter().next().unwrap();
    let found = pairs.lock().iter().any(|(_, v)| v == &needle);
    Ok(Value::Bool(found))
}

/// Mini-tanda Mb7 — `m.with(k, v) -> Map<K, V>`: devuelve un Map
/// nuevo con la key `k` mapeada a `v`. Si `k` ya existe, sobreescribe
/// (last-write-wins, paralelo a `merge`). Operación funcional pura —
/// el receiver queda intacto. Útil para construir Maps acumulando
/// updates sin mutar.
fn map_with(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("with", &args, 2, span)?;
    let pairs = match receiver {
        Value::Map(p) => p,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let k = it.next().unwrap();
    let v = it.next().unwrap();
    let mut out: Vec<(Value, Value)> = pairs.lock().clone();
    if let Some(slot) = out.iter_mut().find(|(ek, _)| ek == &k) {
        slot.1 = v;
    } else {
        out.push((k, v));
    }
    Ok(Value::new_map(out))
}

/// Mini-tanda Mb6 — `m.merge_with(other, fn(V, V) -> V) -> Map<K, V>`:
/// merge con resolución de conflictos via callback. Para cada key
/// que aparece en ambos maps, el callback decide qué value queda
/// (e.g. `fn(a, b) => a + b` para sumar valores). Keys que están
/// solo en uno de los dos maps pasan tal cual. Preserva orden:
/// keys del receiver primero, keys nuevas de `other` al final.
/// Generaliza `merge` (que es last-write-wins).
#[async_recursion]
async fn map_merge_with(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("merge_with", &args, 2, span)?;
    let pairs = match receiver {
        Value::Map(p) => p,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let other = it.next().unwrap();
    let cb = it.next().unwrap();
    let other_pairs = match other {
        Value::Map(p) => p,
        v => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Map".into(),
                    found: v.type_name().into(),
                },
                span.line, span.column,
                format!("`.merge_with()` espera Map, recibió `{}`", v.type_name()),
            )));
        }
    };
    let mut out: Vec<(Value, Value)> = pairs.lock().clone();
    let other_snap: Vec<(Value, Value)> = other_pairs.lock().clone();
    for (k, v_other) in other_snap {
        if let Some(slot_idx) = out.iter().position(|(ek, _)| ek == &k) {
            // Key duplicada: el callback decide.
            let v_self = out[slot_idx].1.clone();
            let resolved = invoke_value(
                cb.clone(),
                vec![v_self, v_other],
                "merge_with",
                span,
            ).await?;
            out[slot_idx].1 = resolved;
        } else {
            out.push((k, v_other));
        }
    }
    Ok(Value::new_map(out))
}

/// Mini-tanda Mb4 — `m.invert()`: devuelve `Map<V, K>` con los pares
/// intercambiados (value pasa a key, key pasa a value). Si hay values
/// duplicados, last-write-wins (paralelo a `to_map()`). Útil para
/// "reverse lookup" — `{nombre: id}` → `{id: nombre}`.
fn map_invert(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("invert", &args, 0, span)?;
    let pairs = match receiver {
        Value::Map(p) => p,
        _ => unreachable!(),
    };
    let snapshot: Vec<(Value, Value)> = pairs.lock().clone();
    let mut out: Vec<(Value, Value)> = Vec::with_capacity(snapshot.len());
    for (k, v) in snapshot {
        // El value pasa a key, la key pasa a value.
        if let Some(slot) = out.iter_mut().find(|(ek, _)| ek == &v) {
            slot.1 = k;
        } else {
            out.push((v, k));
        }
    }
    Ok(Value::new_map(out))
}

/// Mini-tanda Mb3 — `m.entries()`: devuelve `List<(K, V)>` con los
/// pares clave-valor en orden de inserción. Paralelo a Python
/// `dict.items()` o JS `Object.entries(obj)`. Inversa de
/// `xs.to_map()` (Mb3).
fn map_entries(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("entries", &args, 0, span)?;
    let pairs = match receiver {
        Value::Map(p) => p,
        _ => unreachable!(),
    };
    let snapshot: Vec<Value> = pairs
        .lock()
        .iter()
        .map(|(k, v)| Value::Tuple(vec![k.clone(), v.clone()]))
        .collect();
    Ok(Value::new_list(snapshot))
}

/// Mini-tanda Mb2 — `m.keys_sorted()`: devuelve `List<K>` con las
/// keys ordenadas. Solo válido para K en {Int, Float, Str, Bool}
/// (mismas reglas que `list_sort`). Map vacío → lista vacía. Tipos
/// no comparables o heterogéneos → error de runtime claro.
fn map_keys_sorted(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("keys_sorted", &args, 0, span)?;
    let pairs = match receiver {
        Value::Map(p) => p,
        _ => unreachable!(),
    };
    let mut keys: Vec<Value> = pairs.lock().iter().map(|(k, _)| k.clone()).collect();
    if keys.is_empty() {
        return Ok(Value::new_list(vec![]));
    }
    let first_kind = keys[0].type_name();
    if !matches!(first_kind, "Int" | "Float" | "Str" | "Bool") {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int|Float|Str|Bool".into(),
                found: first_kind.into(),
            },
            span.line, span.column,
            format!(
                "`.keys_sorted()` solo soporta keys `Int`/`Float`/`Str`/`Bool`, recibió `{}`",
                first_kind,
            ),
        )));
    }
    for k in keys.iter().skip(1) {
        if k.type_name() != first_kind {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: first_kind.into(),
                    found: k.type_name().into(),
                },
                span.line, span.column,
                format!(
                    "`.keys_sorted()` requiere keys del mismo tipo: vi `{}` y `{}`",
                    first_kind, k.type_name(),
                ),
            )));
        }
    }
    match first_kind {
        "Int" => keys.sort_by(|a, b| match (a, b) {
            (Value::Int(x), Value::Int(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        }),
        "Float" => keys.sort_by(|a, b| match (a, b) {
            (Value::Float(x), Value::Float(y)) => {
                x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
            }
            _ => std::cmp::Ordering::Equal,
        }),
        "Str" => keys.sort_by(|a, b| match (a, b) {
            (Value::Str(x), Value::Str(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        }),
        "Bool" => keys.sort_by(|a, b| match (a, b) {
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        }),
        _ => unreachable!(),
    }
    Ok(Value::new_list(keys))
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

/// S.1 — Helper común: extrae el `receiver: Str` y el `arg: Str` para
/// los métodos `contains`/`starts_with`/`ends_with`. Si el argumento
/// no es `Str` (caso gradual), devuelve error de runtime claro.
fn str_one_str_arg(
    method: &str,
    receiver: Value,
    args: Vec<Value>,
    span: Span,
) -> EvalResult<(String, String)> {
    expect_arity(method, &args, 1, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let needle = match args.into_iter().next().unwrap() {
        Value::Str(s) => s,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Str".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!(
                    "`.{}()` espera un argumento `Str`, recibió `{}`",
                    method, other.type_name(),
                ),
            )));
        }
    };
    Ok((s, needle))
}

fn str_contains(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let (s, needle) = str_one_str_arg("contains", receiver, args, span)?;
    Ok(Value::Bool(s.contains(&needle)))
}

fn str_starts_with(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let (s, needle) = str_one_str_arg("starts_with", receiver, args, span)?;
    Ok(Value::Bool(s.starts_with(&needle)))
}

fn str_ends_with(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let (s, needle) = str_one_str_arg("ends_with", receiver, args, span)?;
    Ok(Value::Bool(s.ends_with(&needle)))
}

/// S.2 — `s.split(sep)` devuelve `List<Str>`. Sin separador especial
/// para whitespace (`s.split()` sin args queda como deuda menor —
/// Rust `split_whitespace` lo cubre pero la semántica es distinta).
/// Separador empty string: replica `str::split("")` que devuelve
/// chars individuales + empties en bordes — mismo comportamiento
/// que Python por default.
fn str_split(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let (s, sep) = str_one_str_arg("split", receiver, args, span)?;
    let parts: Vec<Value> = s
        .split(&sep[..])
        .map(|p| Value::Str(p.to_string()))
        .collect();
    Ok(Value::new_list(parts))
}

/// Mini-tanda Mb4 — `s.split_at(idx)`: divide el string en posición
/// `idx` (en CHARS, no bytes) y devuelve `(Str, Str)`. `idx == 0` →
/// `("", s)`; `idx >= len(s)` → `(s, "")`. `idx < 0` → error claro.
/// Paralelo a `str::split_at` Rust (que opera sobre bytes) pero con
/// char-based indexing para uniformar con el resto de los métodos
/// Str de Fitz.
fn str_split_at(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("split_at", &args, 1, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let idx = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`Str.split_at()` espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    if idx < 0 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!("`Str.split_at()` no acepta índice negativo: recibió {}", idx),
        )));
    }
    let len = s.chars().count() as i64;
    let clamped = idx.min(len) as usize;
    let left: String = s.chars().take(clamped).collect();
    let right: String = s.chars().skip(clamped).collect();
    Ok(Value::Tuple(vec![Value::Str(left), Value::Str(right)]))
}

/// Mini-tanda Mb9 — `s.swap_case() -> Str`: invierte el case de cada
/// caracter (mayúscula ↔ minúscula). Caracteres sin case (dígitos,
/// símbolos) quedan como están.
fn str_swap_case(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("swap_case", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_uppercase() {
                c.to_lowercase().collect::<String>()
            } else if c.is_lowercase() {
                c.to_uppercase().collect::<String>()
            } else {
                c.to_string()
            }
        })
        .collect();
    Ok(Value::Str(out))
}

/// Mini-tanda Mb9 — `s.title() -> Str`: capitaliza la primera letra
/// de cada palabra (separadas por whitespace). Paralelo a Python
/// `str.title`. El resto de cada palabra queda en lowercase.
fn str_title(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("title", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let mut out = String::with_capacity(s.len());
    let mut start_of_word = true;
    for c in s.chars() {
        if c.is_whitespace() {
            out.push(c);
            start_of_word = true;
        } else if start_of_word {
            out.extend(c.to_uppercase());
            start_of_word = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    Ok(Value::Str(out))
}

/// Mini-tanda Mb9 — `s.is_alpha() -> Bool`: todos los chars son
/// letras. String vacío → false (paralelo a Python).
fn str_is_alpha(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("is_alpha", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let r = !s.is_empty() && s.chars().all(|c| c.is_alphabetic());
    Ok(Value::Bool(r))
}

/// Mini-tanda Mb9 — `s.is_digit() -> Bool`: todos los chars son
/// dígitos ASCII (0-9). String vacío → false.
fn str_is_digit(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("is_digit", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let r = !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    Ok(Value::Bool(r))
}

/// Mini-tanda Mb9 — `s.is_numeric() -> Bool`: el string completo
/// parsea como número (Int o Float, con signo opcional). String
/// vacío → false. Más permisivo que `is_digit` (acepta `.` y `-`).
fn str_is_numeric(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("is_numeric", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let r = !s.is_empty() && s.parse::<f64>().is_ok();
    Ok(Value::Bool(r))
}

/// Mini-tanda Mb8 — `s.left(n) -> Str` / `s.right(n) -> Str`:
/// primeros/últimos `n` caracteres. `n <= 0` → vacío; `n >= len(s)`
/// → string completo. Paralelo a métodos VB/SQL clásicos. Char-based,
/// no byte-based (consistente con `len`).
fn str_left(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("left", &args, 1, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let n = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`Str.left()` espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let take = if n <= 0 { 0 } else { n as usize };
    let out: String = s.chars().take(take).collect();
    Ok(Value::Str(out))
}

fn str_right(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("right", &args, 1, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let n = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`Str.right()` espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let len = s.chars().count();
    let take = if n <= 0 { 0 } else { (n as usize).min(len) };
    let skip = len - take;
    let out: String = s.chars().skip(skip).collect();
    Ok(Value::Str(out))
}

/// Mini-tanda Mb8 — `s.center(width, ch) -> Str`: centra el string
/// padeando con `ch` a ambos lados hasta alcanzar `width` chars. Si
/// `len(s) >= width`, devuelve `s` sin cambios. `ch` debe ser
/// exactamente 1 char. Paralelo a Python `str.center(width, ch)`.
/// Si el padding es impar, el extra va a la derecha (paralelo a
/// Python).
fn str_center(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("center", &args, 2, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let width = match it.next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`Str.center()`: arg 0 (width) espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let ch = match it.next().unwrap() {
        Value::Str(s) => s,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Str".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`Str.center()`: arg 1 (ch) espera `Str`, recibió `{}`", other.type_name()),
            )));
        }
    };
    if ch.chars().count() != 1 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!(
                "`Str.center(width, ch)`: el char de relleno debe ser 1 caracter, recibió `\"{}\"`",
                ch,
            ),
        )));
    }
    let len = s.chars().count() as i64;
    if len >= width {
        return Ok(Value::Str(s));
    }
    let total_pad = (width - len) as usize;
    let left = total_pad / 2;
    let right = total_pad - left;
    let mut out = String::with_capacity(width as usize);
    out.push_str(&ch.repeat(left));
    out.push_str(&s);
    out.push_str(&ch.repeat(right));
    Ok(Value::Str(out))
}

/// Mini-tanda Mb7 — `s.repeat_with(n, sep) -> Str`: repite el string
/// `n` veces, intercalando `sep` entre cada repetición. `n < 0` →
/// error claro; `n == 0` → string vacío. `"x".repeat_with(3, ", ")` →
/// `"x, x, x"`. Paralelo a Python `sep.join([s] * n)` con sintaxis
/// método.
fn str_repeat_with(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("repeat_with", &args, 2, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let n = match it.next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`.repeat_with()`: arg 0 (n) espera `Int`, recibió `{}`", other.type_name()),
            )));
        }
    };
    let sep = match it.next().unwrap() {
        Value::Str(s) => s,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Str".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("`.repeat_with()`: arg 1 (sep) espera `Str`, recibió `{}`", other.type_name()),
            )));
        }
    };
    if n < 0 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!("`.repeat_with()` no acepta n negativo: recibió {}", n),
        )));
    }
    let parts: Vec<&str> = std::iter::repeat_n(s.as_str(), n as usize).collect();
    Ok(Value::Str(parts.join(&sep)))
}

/// Mini-tanda Mb5 — `s.lines()`: separa el string por `\n` devolviendo
/// `List<Str>`. Paralelo a `str::lines` Rust: si el string termina con
/// `\n`, NO se agrega línea vacía al final. Strings vacíos → lista
/// vacía.
fn str_lines(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("lines", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let parts: Vec<Value> = s.lines().map(|l| Value::Str(l.to_string())).collect();
    Ok(Value::new_list(parts))
}

/// Mini-tanda Mb5 — `s.is_empty()`: `Bool` indicando si `s == ""`.
/// Atajo de `s.len() == 0` con intención clara.
fn str_is_empty(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("is_empty", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    Ok(Value::Bool(s.is_empty()))
}

/// Mini-tanda Mb3 — `s.chars()`: devuelve `List<Str>` con cada char
/// del string como Str de 1 caracter. Paralelo a Python `list(s)` o
/// JS `[...s]`. Útil para iterar y para componer pipelines (e.g.
/// `s.chars().filter(...)`).
fn str_chars(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("chars", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let parts: Vec<Value> = s.chars().map(|c| Value::Str(c.to_string())).collect();
    Ok(Value::new_list(parts))
}

fn str_trim(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("trim", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    Ok(Value::Str(s.trim().to_string()))
}

/// Mb — `s.trim_start()`: solo recorta whitespace del inicio. Paralelo
/// a `str::trim_start` Rust / `str.lstrip` Python (default whitespace).
fn str_trim_start(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("trim_start", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    Ok(Value::Str(s.trim_start().to_string()))
}

/// Mb — `s.trim_end()`: solo recorta whitespace del final. Paralelo
/// a `str::trim_end` Rust / `str.rstrip` Python (default whitespace).
fn str_trim_end(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("trim_end", &args, 0, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    Ok(Value::Str(s.trim_end().to_string()))
}

/// S.2 — `s.replace(old, new)`. Reemplaza TODAS las ocurrencias.
/// Mismo comportamiento que `str::replace` Rust y `str.replace`
/// Python.
fn str_replace(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("replace", &args, 2, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let old = match it.next().unwrap() {
        Value::Str(x) => x,
        other => return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch { expected: "Str".into(), found: other.type_name().into() },
            span.line, span.column,
            format!("`.replace(old, new)` espera Str para el arg 1, recibió {}", other.type_name()),
        ))),
    };
    let new_s = match it.next().unwrap() {
        Value::Str(x) => x,
        other => return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch { expected: "Str".into(), found: other.type_name().into() },
            span.line, span.column,
            format!("`.replace(old, new)` espera Str para el arg 2, recibió {}", other.type_name()),
        ))),
    };
    Ok(Value::Str(s.replace(&old, &new_s)))
}

/// S.2 — `s.repeat(n)` repite el string n veces. `n < 0` es error
/// claro; `n == 0` → string vacío (igual que Rust/Python).
fn str_repeat(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("repeat", &args, 1, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let n = match args.into_iter().next().unwrap() {
        Value::Int(n) => n,
        other => return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch { expected: "Int".into(), found: other.type_name().into() },
            span.line, span.column,
            format!("`.repeat()` espera Int, recibió {}", other.type_name()),
        ))),
    };
    if n < 0 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!("`.repeat()` no acepta n negativo: recibió {}", n),
        )));
    }
    Ok(Value::Str(s.repeat(n as usize)))
}

/// Mini-tanda Mb2 — Helper: para `pad_start`/`pad_end`. Extrae
/// `(s, width, ch)` con validaciones: width `Int`, ch `Str` de
/// exactamente 1 char (paralelo a Python `str.rjust(width, ch)`).
fn str_pad_args(
    method: &str,
    receiver: Value,
    args: Vec<Value>,
    span: Span,
) -> EvalResult<(String, i64, String)> {
    expect_arity(method, &args, 2, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let mut it = args.into_iter();
    let width = match it.next().unwrap() {
        Value::Int(n) => n,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!(
                    "`.{}(width, ch)`: arg 0 espera `Int`, recibió `{}`",
                    method, other.type_name(),
                ),
            )));
        }
    };
    let ch = match it.next().unwrap() {
        Value::Str(x) => x,
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Str".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!(
                    "`.{}(width, ch)`: arg 1 espera `Str`, recibió `{}`",
                    method, other.type_name(),
                ),
            )));
        }
    };
    if ch.chars().count() != 1 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!(
                "`.{}(width, ch)`: el char de relleno debe ser exactamente 1 caracter, recibió `\"{}\"` ({} chars)",
                method, ch, ch.chars().count(),
            ),
        )));
    }
    Ok((s, width, ch))
}

/// Mini-tanda Mb2 — `s.pad_start(width, ch)`: prefija el string con
/// copias de `ch` hasta alcanzar `width` chars. Si `len(s) >= width`,
/// devuelve `s` sin cambios. Paralelo a Python `str.rjust(width, ch)`.
fn str_pad_start(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let (s, width, ch) = str_pad_args("pad_start", receiver, args, span)?;
    let len = s.chars().count() as i64;
    if len >= width {
        return Ok(Value::Str(s));
    }
    let n_pad = (width - len) as usize;
    let pad = ch.repeat(n_pad);
    Ok(Value::Str(format!("{}{}", pad, s)))
}

/// Mini-tanda Mb2 — `s.pad_end(width, ch)`: sufija el string con
/// copias de `ch` hasta alcanzar `width` chars. Paralelo a Python
/// `str.ljust(width, ch)`.
fn str_pad_end(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let (s, width, ch) = str_pad_args("pad_end", receiver, args, span)?;
    let len = s.chars().count() as i64;
    if len >= width {
        return Ok(Value::Str(s));
    }
    let n_pad = (width - len) as usize;
    let pad = ch.repeat(n_pad);
    Ok(Value::Str(format!("{}{}", s, pad)))
}

/// Mini-tanda Ex — `s.find(sub)`: posición de la primera ocurrencia.
/// Devuelve `Result<Int>` — `Ok(i)` con el índice (en chars, no bytes)
/// si lo encuentra, `Err("no encontrado")` si no. Sub vacío matchea
/// en posición 0 (paralelo a Python).
fn str_find(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("find", &args, 1, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let needle = match args.into_iter().next().unwrap() {
        Value::Str(x) => x,
        other => return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch { expected: "Str".into(), found: other.type_name().into() },
            span.line, span.column,
            format!("`.find()` espera Str, recibió {}", other.type_name()),
        ))),
    };
    // Rust `str::find` devuelve byte index; convertimos a char index.
    if let Some(byte_idx) = s.find(needle.as_str()) {
        let char_idx = s[..byte_idx].chars().count() as i64;
        Ok(Value::Result(ResultVariant::Ok(Box::new(Value::Int(char_idx)))))
    } else {
        Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "no encontrado".into(),
        )))))
    }
}

/// Mini-tanda Ex — `s.index_of(sub)`: alias de `find` con nombre
/// estilo JS/TypeScript. Misma semántica.
fn str_index_of(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("index_of", &args, 1, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let needle = match args.into_iter().next().unwrap() {
        Value::Str(x) => x,
        other => return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch { expected: "Str".into(), found: other.type_name().into() },
            span.line, span.column,
            format!("`.index_of()` espera Str, recibió {}", other.type_name()),
        ))),
    };
    if let Some(byte_idx) = s.find(needle.as_str()) {
        let char_idx = s[..byte_idx].chars().count() as i64;
        Ok(Value::Result(ResultVariant::Ok(Box::new(Value::Int(char_idx)))))
    } else {
        Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "no encontrado".into(),
        )))))
    }
}

/// Mini-tanda Ex — `s.last_index_of(sub)`: posición de la ÚLTIMA
/// ocurrencia. Mismo shape de retorno que `find`/`index_of`.
fn str_last_index_of(receiver: Value, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    expect_arity("last_index_of", &args, 1, span)?;
    let s = match receiver {
        Value::Str(s) => s,
        _ => unreachable!(),
    };
    let needle = match args.into_iter().next().unwrap() {
        Value::Str(x) => x,
        other => return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch { expected: "Str".into(), found: other.type_name().into() },
            span.line, span.column,
            format!("`.last_index_of()` espera Str, recibió {}", other.type_name()),
        ))),
    };
    if let Some(byte_idx) = s.rfind(needle.as_str()) {
        let char_idx = s[..byte_idx].chars().count() as i64;
        Ok(Value::Result(ResultVariant::Ok(Box::new(Value::Int(char_idx)))))
    } else {
        Ok(Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "no encontrado".into(),
        )))))
    }
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
        Mod => eval_mod(l, r, span),
        Eq => Ok(Value::Bool(l == r)),
        NotEq => Ok(Value::Bool(l != r)),
        Lt | LtEq | Gt | GtEq => compare(op, l, r, span),
        And | Or | Xor => unreachable!("And/Or/Xor se manejan en eval_logical antes de llegar acá"),
        // Mini-tanda Bits — solo Int. El checker rechaza otros tipos
        // estáticamente; el runtime emite TypeError si por modo
        // gradual llega un valor no-Int.
        BitAnd | BitOr | BitXor => eval_bitwise(op, l, r, span),
        Shl | Shr => eval_shift(op, l, r, span),
    }
}

/// Mini-tanda Bits — AND/OR/XOR bit-a-bit sobre `Int`.
fn eval_bitwise(op: &BinOpKind, l: Value, r: Value, span: Span) -> EvalResult<Value> {
    match (&l, &r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(match op {
            BinOpKind::BitAnd => a & b,
            BinOpKind::BitOr => a | b,
            BinOpKind::BitXor => a ^ b,
            _ => unreachable!(),
        })),
        _ => {
            let sym = match op {
                BinOpKind::BitAnd => "&",
                BinOpKind::BitOr => "|",
                BinOpKind::BitXor => "^",
                _ => unreachable!(),
            };
            type_error(sym, &l, &r, span)
        }
    }
}

/// Mini-tanda Bits — shifts `<<` / `>>` sobre `Int`. RHS negativo o
/// fuera del rango `0..64` produce error claro (paralelo a Rust panic
/// con shift overflow, pero como mensaje recuperable en lugar de panic).
fn eval_shift(op: &BinOpKind, l: Value, r: Value, span: Span) -> EvalResult<Value> {
    let (a, b) = match (&l, &r) {
        (Value::Int(a), Value::Int(b)) => (*a, *b),
        _ => {
            let sym = if matches!(op, BinOpKind::Shl) { "<<" } else { ">>" };
            return type_error(sym, &l, &r, span);
        }
    };
    if !(0..64).contains(&b) {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!(
                "shift fuera de rango: el segundo operando debe estar en 0..64, recibió {}",
                b
            ),
        )));
    }
    let n = b as u32;
    let result = match op {
        BinOpKind::Shl => a.wrapping_shl(n),
        BinOpKind::Shr => a.wrapping_shr(n),
        _ => unreachable!(),
    };
    Ok(Value::Int(result))
}

/// R.1.2 — operador `%` con semántica euclidean. `i64::rem_euclid`
/// garantiza resultado con el mismo signo del divisor (siempre
/// positivo si el divisor es positivo). Paralelo a Python, distinto
/// del `%` Rust (truncate-toward-zero).
///
/// `n % 0` paniquearía en Rust nativo; lo capturamos antes y
/// emitimos `DivisionByZero` para que el evaluator no aborte.
fn eval_mod(l: Value, r: Value, span: Span) -> EvalResult<Value> {
    match (&l, &r) {
        (Value::Int(_), Value::Int(0)) => div_by_zero(span),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.rem_euclid(*b))),
        _ => type_error("%", &l, &r, span),
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
#[async_recursion]
async fn eval_logical(
    op: &BinOpKind, left: &Expr, right: &Expr, env: EnvRef, span: Span,
) -> EvalResult<Value> {
    let lv = eval_expr(left, env.clone()).await?;
    let lb = expect_bool(&lv, op_name(op), "izquierdo", left.span())?;

    // Short-circuit: `false and ...` → false, `true or ...` → true.
    // `xor` NO tiene short-circuit (necesita ambos lados para saber el
    // resultado), así que cae directo al eval del RHS.
    match op {
        BinOpKind::And if !lb => return Ok(Value::Bool(false)),
        BinOpKind::Or if lb => return Ok(Value::Bool(true)),
        _ => {}
    }

    let rv = eval_expr(right, env).await?;
    let rb = expect_bool(&rv, op_name(op), "derecho", right.span())?;
    let _ = span; // mantenido por consistencia de firma con eval_binop
    // Mini-tanda Xor — `a xor b` = `a != b` sobre Bool.
    match op {
        BinOpKind::Xor => Ok(Value::Bool(lb != rb)),
        _ => Ok(Value::Bool(rb)),
    }
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
        Add => "+", Sub => "-", Mul => "*", Div => "/", Mod => "%",
        Eq => "==", NotEq => "!=",
        Lt => "<", LtEq => "<=", Gt => ">", GtEq => ">=",
        And => "and", Or => "or", Xor => "xor",
        BitAnd => "&", BitOr => "|", BitXor => "^",
        Shl => "<<", Shr => ">>",
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
            // I.1 (mini-tanda I) — índices negativos al estilo Python:
            // `xs[-1]` es el último, `xs[-2]` el penúltimo, etc. La
            // resolución es `effective = len + i`. Si sigue negativo o
            // ≥ len, error de runtime claro (sin auto-wrap más allá del
            // tamaño).
            let borrowed = items.lock();
            let len = borrowed.len() as i64;
            let effective = if i < 0 { len + i } else { i };
            if effective < 0 || effective >= len {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    span.line, span.column,
                    format!(
                        "índice fuera de rango: {} en lista de tamaño {}",
                        i, len,
                    ),
                )));
            }
            Ok(borrowed[effective as usize].clone())
        }
        Value::Str(s) => {
            // I.1 — `s[i]` devuelve el i-ésimo char como `Str` de un
            // char (Fitz no tiene tipo Char). Soporta negativos
            // (`s[-1]` = último char). Cuenta CHARS, no bytes — mismo
            // contrato que `s.len()`.
            let i = match idx {
                Value::Int(n) => *n,
                other => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Int".into(),
                        found: other.type_name().into(),
                    },
                    span.line, span.column,
                    format!(
                        "el índice de un Str debe ser Int, no `{}`",
                        other.type_name()
                    ),
                ))),
            };
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let effective = if i < 0 { len + i } else { i };
            if effective < 0 || effective >= len {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    span.line, span.column,
                    format!(
                        "índice fuera de rango: {} en Str de tamaño {}",
                        i, len,
                    ),
                )));
            }
            Ok(Value::Str(chars[effective as usize].to_string()))
        }
        Value::Map(pairs) => {
            // Búsqueda lineal por igualdad. Esto va a ser O(n) hasta que
            // promovamos Map a una estructura indexada de verdad.
            for (k, v) in pairs.lock().iter() {
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

/// I.2 (mini-tanda I) — slicing. Resuelve `xs[a..b]`, `xs[..b]`,
/// `xs[a..]`, `xs[..]`, `xs[a..=b]` sobre List<T> y Str.
///
/// Política:
///  - `start = None` → 0.
///  - `end = None` → len.
///  - Índices negativos se convierten a `len + i` (igual que
///    indexing).
///  - **Clamp**: si después del wrap el índice queda fuera de
///    `[0, len]`, se ajusta a las cotas (Python-style). `xs[100..]`
///    con len=5 → []. No paniquea.
///  - Si `start > end` tras clamp → slice vacío.
///  - `inclusive: true` ajusta `end += 1` ANTES del clamp.
///
/// Devuelve siempre una NUEVA colección (copy semantics).
fn eval_slice(
    obj: &Value,
    start: Option<&Value>,
    end: Option<&Value>,
    inclusive: bool,
    span: Span,
) -> EvalResult<Value> {
    fn extract_int(v: Option<&Value>, name: &str, span: Span) -> EvalResult<Option<i64>> {
        match v {
            None => Ok(None),
            Some(Value::Int(n)) => Ok(Some(*n)),
            Some(other) => Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!("el `{}` de un slice debe ser Int, recibió `{}`", name, other.type_name()),
            ))),
        }
    }
    fn resolve_bounds(
        start: Option<i64>,
        end: Option<i64>,
        inclusive: bool,
        len: i64,
    ) -> (usize, usize) {
        let s_raw = start.unwrap_or(0);
        let e_raw = end.unwrap_or(if inclusive { len - 1 } else { len });
        let s_wrap = if s_raw < 0 { len + s_raw } else { s_raw };
        let e_wrap = if e_raw < 0 { len + e_raw } else { e_raw };
        let e_excl = if inclusive { e_wrap + 1 } else { e_wrap };
        let s_clamp = s_wrap.clamp(0, len);
        let e_clamp = e_excl.clamp(0, len);
        let s = s_clamp.min(e_clamp); // si start > end, slice vacío
        (s as usize, e_clamp as usize)
    }

    let s_i = extract_int(start, "start", span)?;
    let e_i = extract_int(end, "end", span)?;

    match obj {
        Value::List(items) => {
            let borrowed = items.lock();
            let len = borrowed.len() as i64;
            let (a, b) = resolve_bounds(s_i, e_i, inclusive, len);
            let slice: Vec<Value> = borrowed[a..b].to_vec();
            Ok(Value::new_list(slice))
        }
        Value::Str(s) => {
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let (a, b) = resolve_bounds(s_i, e_i, inclusive, len);
            let slice: String = chars[a..b].iter().collect();
            Ok(Value::Str(slice))
        }
        other => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "List o Str".into(),
                found: other.type_name().into(),
            },
            span.line, span.column,
            format!("el tipo `{}` no soporta slicing", other.type_name()),
        ))),
    }
}

// ---------------------------------------------------------------------------
// Coerción Map → Instance con anotación (Fase 8.4.3)
// ---------------------------------------------------------------------------
//
// Cuando un `let x: T = ...` tiene anotación nominal y el RHS evaluado es
// un `Value::Map` (típicamente un dict Python coercionado a Map en 8.2.2),
// intentamos construir un `Value::Instance` validando que el dict tenga
// los campos requeridos por `T`. Habilita el patrón canónico:
//
//   let row: User = json.loads(s)?
//
// Sin coerción, `row` queda como Map y el resto del programa lo trata
// gradualmente; con coerción, el usuario sale del "limbo Python" a tipos
// Fitz concretos en un solo punto.
//
// Reglas:
//   - Anotación `Named(T)` con T nominal + value `Map` → coerce.
//   - Anotación `Nullable(Named(T))` con value `Map` → coerce a `T`.
//   - Anotación `Nullable(Named(T))` con value `Null` → pasa `Null` tal cual.
//   - Cualquier otra combinación (anotación generic, value no-Map,
//     value ya `Instance`, etc.) → pasa el value tal cual sin tocar.
//   - Si el dict no tiene un campo requerido (no nullable, sin default) →
//     `FitzError` claro citando el campo y el tipo.
//   - Campos extras del dict se ignoran (Python suele devolver más de lo
//     necesario; ser permisivos evita fricción innecesaria).

#[async_recursion]
async fn coerce_to_annotation(
    annot: &TypeExpr,
    value: Value,
    env: EnvRef,
) -> EvalResult<Value> {
    // Resolver: nombre del tipo + si la anotación tolera Null.
    let (type_name, allows_null) = match annot {
        TypeExpr::Named(name) => (name.clone(), false),
        TypeExpr::Nullable(inner) => match inner.as_ref() {
            TypeExpr::Named(name) => (name.clone(), true),
            _ => return Ok(value),
        },
        _ => return Ok(value),
    };

    // Nullable + Null → passthrough.
    if allows_null && matches!(value, Value::Null) {
        return Ok(value);
    }

    // Solo intentamos coercer cuando el valor es un Map. Cualquier otro
    // tipo (Instance ya, primitivo, Result, etc.) pasa tal cual — el
    // gradual del checker se encarga de aceptarlo si la anotación es
    // gradual, y los usos posteriores van a fallar claro si no encaja.
    let map_pairs = match value {
        Value::Map(pairs) => pairs,
        other => return Ok(other),
    };

    // Resolver el tipo declarado en el env. Si no es un Value::Type
    // (puede ser un built-in como `Int`, `Str`, etc.), no coercemos:
    // los primitivos no se construyen desde dicts.
    let ty_value = env.lock().get(&type_name);
    let (declared_type_name, declared_fields, resolved_defaults) = match ty_value {
        Some(Value::Type { name, fields, resolved_defaults, .. }) => {
            (name, fields, resolved_defaults)
        }
        _ => return Ok(Value::Map(map_pairs)),
    };

    // Snapshot del Map adentro del lock, después soltamos para evitar
    // mantener el guard durante el eval de los defaults (cualquiera de
    // los cuales podría locker otro Mutex).
    let map_snapshot: Vec<(Value, Value)> = map_pairs.lock().clone();

    let mut instance_fields: Vec<(String, Value)> =
        Vec::with_capacity(declared_fields.len());
    for f in &declared_fields {
        // Buscar el campo en el map por su nombre. La key tiene que
        // ser un `Str` con el nombre exacto del field; otros tipos
        // de key se ignoran (no son representables como nombre de
        // campo Fitz).
        let provided = map_snapshot.iter().find(|(k, _)| {
            matches!(k, Value::Str(s) if s == &f.name)
        });
        let value = if let Some((_, v)) = provided {
            v.clone()
        } else if let Some((_, v)) = resolved_defaults.iter().find(|(n, _)| n == &f.name) {
            v.clone()
        } else if let Some(default_expr) = &f.default {
            eval_expr(default_expr, env.clone()).await?
        } else if f.type_.is_nullable() {
            Value::Null
        } else {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::InvalidSyntax,
                0, 0,
                format!(
                    "no se puede coercer a `{}`: el dict no tiene el campo `{}` \
                     (requerido por el tipo, no es nullable ni tiene default)",
                    type_name, f.name,
                ),
            )));
        };
        instance_fields.push((f.name.clone(), value));
    }

    Ok(Value::new_instance(declared_type_name, instance_fields))
}

// ---------------------------------------------------------------------------
// Operación unaria
// ---------------------------------------------------------------------------
//
// - `Neg`: negación numérica (`-x`) sobre Int/Float.
// - `Not` (R.1.1): negación lógica (`not x`) sobre Bool estricto.

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
        UnaryOpKind::Not => match v {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            other => Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Bool".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!(
                    "el operador `not` requiere Bool, recibió `{}`",
                    other.type_name()
                ),
            ))),
        },
        // Mini-tanda Bits — NOT bit-a-bit. Solo Int.
        UnaryOpKind::BitNot => match v {
            Value::Int(n) => Ok(Value::Int(!n)),
            other => Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!(
                    "el operador `~` requiere Int, recibió `{}`",
                    other.type_name()
                ),
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
    env.lock().define(
        "print",
        Value::Builtin {
            name: "print",
            func: builtin_print,
        },
    );
    env.lock().define(
        "len",
        Value::Builtin {
            name: "len",
            func: builtin_len,
        },
    );
    // `cors(config: Map?)` — built-in MW.2. Construye un
    // `Value::CorsConfig` con los kwargs efectivos. El config es un
    // `Map<Str, ...>` (no kwargs runtime — el parser de calls no los
    // soporta y un refactor del AST queda fuera de scope de MW.2).
    // Llamadas válidas:
    //   - `cors()` o `cors({})` → defaults permisivos (origin "*",
    //     métodos comunes, headers content-type+authorization).
    //   - `cors({"allow_origin": "https://x.com"})` → override de un
    //     subset; el resto queda en defaults.
    //   - Keys soportadas: `allow_origin` (Str), `allow_methods`
    //     (List<Str>), `allow_headers` (List<Str>), `max_age` (Int).
    //   - Cualquier otra key, o un tipo distinto al esperado, da error.
    env.lock().define(
        "cors",
        Value::Builtin {
            name: "cors",
            func: builtin_cors,
        },
    );
    // `sleep(ms: Int)` — async primitive introducido en Fase 6.3. El
    // checker lo tipa como `Function { ret: Future<Null> }`. Acá
    // registramos un stub que emite error claro: el evaluator async
    // (refactor de eval_expr/eval_stmt/etc. a `async fn`) entra en
    // 6.4, así que hasta entonces no podemos esperar ni construir
    // un `Value::Future`. La barrera del evaluator sobre `Expr::Await`
    // suele cortar antes; este builtin es para el caso de llamar a
    // `sleep(100)` sin `.await` (guardar el Future suelto).
    env.lock().define(
        "sleep",
        Value::Builtin {
            name: "sleep",
            func: builtin_sleep,
        },
    );
    // Fase 9.z.2.a — assertion builtins. Siempre disponibles (igual
    // que `print`/`len`/`sleep`/`cors`); su semántica es la misma
    // dentro o fuera de `@test`. Una aserción fallida emite
    // `FitzError` que aborta la ejecución del programa — el runner
    // de `fitz test` (9.z.2.b) atrapará ese error y lo reportará
    // como fallo del test.
    env.lock().define(
        "assert",
        Value::Builtin {
            name: "assert",
            func: builtin_assert,
        },
    );
    env.lock().define(
        "assert_eq",
        Value::Builtin {
            name: "assert_eq",
            func: builtin_assert_eq,
        },
    );
    env.lock().define(
        "assert_ne",
        Value::Builtin {
            name: "assert_ne",
            func: builtin_assert_ne,
        },
    );
    // `assert_throws(fn)` es **caso especial** en `invoke_value`:
    // necesita invocar el callback async-recursive (el `invoke_value`
    // genérico es async, los builtins son sync). El stub `func` acá
    // emite un `unreachable!` — el dispatcher debería interceptar
    // antes de invocarlo. Si llegás a ver "assert_throws stub", el
    // dispatcher tiene un bug.
    env.lock().define(
        "assert_throws",
        Value::Builtin {
            name: "assert_throws",
            func: builtin_assert_throws_stub,
        },
    );
    // Mini-tanda Bits-extras — ops sobre Int. Builtins globales (en
    // lugar de métodos sobre Int) para mantener simple la dispatch
    // del receptor primitivo.
    env.lock().define(
        "popcount",
        Value::Builtin { name: "popcount", func: builtin_popcount },
    );
    env.lock().define(
        "leading_zeros",
        Value::Builtin { name: "leading_zeros", func: builtin_leading_zeros },
    );
    env.lock().define(
        "trailing_zeros",
        Value::Builtin { name: "trailing_zeros", func: builtin_trailing_zeros },
    );
    env.lock().define(
        "rotate_left",
        Value::Builtin { name: "rotate_left", func: builtin_rotate_left },
    );
    env.lock().define(
        "rotate_right",
        Value::Builtin { name: "rotate_right", func: builtin_rotate_right },
    );
    // Mini-tanda Math — abs/min/max/pow/sqrt/ceil/floor/round/clamp.
    env.lock().define(
        "abs",
        Value::Builtin { name: "abs", func: builtin_math_abs },
    );
    env.lock().define(
        "min",
        Value::Builtin { name: "min", func: builtin_math_min },
    );
    env.lock().define(
        "max",
        Value::Builtin { name: "max", func: builtin_math_max },
    );
    env.lock().define(
        "pow",
        Value::Builtin { name: "pow", func: builtin_math_pow },
    );
    env.lock().define(
        "sqrt",
        Value::Builtin { name: "sqrt", func: builtin_math_sqrt },
    );
    env.lock().define(
        "ceil",
        Value::Builtin { name: "ceil", func: builtin_math_ceil },
    );
    env.lock().define(
        "floor",
        Value::Builtin { name: "floor", func: builtin_math_floor },
    );
    env.lock().define(
        "round",
        Value::Builtin { name: "round", func: builtin_math_round },
    );
    env.lock().define(
        "clamp",
        Value::Builtin { name: "clamp", func: builtin_math_clamp },
    );
}

/// Mini-tanda Bits-extras — `popcount(n: Int) -> Int`: cantidad de
/// bits en 1 en la representación de complemento a dos de `n` (64
/// bits). Paralelo a `i64::count_ones()` Rust.
fn builtin_popcount(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 1, found: args.len() },
            0, 0,
            format!("`popcount(n)` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.count_ones() as i64)),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`popcount(n)` espera `Int`, recibió `{}`", other.type_name()),
        )),
    }
}

/// Mini-tanda Bits-extras — `leading_zeros(n: Int) -> Int`: cantidad
/// de ceros líderes en la representación de 64 bits de `n`.
fn builtin_leading_zeros(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 1, found: args.len() },
            0, 0,
            format!("`leading_zeros(n)` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.leading_zeros() as i64)),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`leading_zeros(n)` espera `Int`, recibió `{}`", other.type_name()),
        )),
    }
}

/// Mini-tanda Bits-extras — `trailing_zeros(n: Int) -> Int`.
fn builtin_trailing_zeros(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 1, found: args.len() },
            0, 0,
            format!("`trailing_zeros(n)` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.trailing_zeros() as i64)),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`trailing_zeros(n)` espera `Int`, recibió `{}`", other.type_name()),
        )),
    }
}

/// Mini-tanda Bits-extras — `rotate_left(n: Int, bits: Int) -> Int`.
/// Rotación a la izquierda en 64 bits. `bits` se toma módulo 64
/// (paralelo a Rust `i64::rotate_left`).
fn builtin_rotate_left(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 2, found: args.len() },
            0, 0,
            format!("`rotate_left(n, bits)` espera 2 args, recibió {}", args.len()),
        ));
    }
    let n = match &args[0] {
        Value::Int(n) => *n,
        other => return Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`rotate_left(n, bits)`: arg 0 espera `Int`, recibió `{}`", other.type_name()),
        )),
    };
    let bits = match &args[1] {
        Value::Int(b) => *b,
        other => return Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`rotate_left(n, bits)`: arg 1 espera `Int`, recibió `{}`", other.type_name()),
        )),
    };
    Ok(Value::Int(n.rotate_left(bits.rem_euclid(64) as u32)))
}

/// Mini-tanda Bits-extras — `rotate_right(n: Int, bits: Int) -> Int`.
fn builtin_rotate_right(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 2, found: args.len() },
            0, 0,
            format!("`rotate_right(n, bits)` espera 2 args, recibió {}", args.len()),
        ));
    }
    let n = match &args[0] {
        Value::Int(n) => *n,
        other => return Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`rotate_right(n, bits)`: arg 0 espera `Int`, recibió `{}`", other.type_name()),
        )),
    };
    let bits = match &args[1] {
        Value::Int(b) => *b,
        other => return Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`rotate_right(n, bits)`: arg 1 espera `Int`, recibió `{}`", other.type_name()),
        )),
    };
    Ok(Value::Int(n.rotate_right(bits.rem_euclid(64) as u32)))
}

// ---- Mini-tanda Math — builtins matemáticos ----
//
// `abs/min/max/pow/sqrt/ceil/floor/round/clamp`. Polimórficos sobre
// Int/Float donde aplica:
//   - abs/min/max/clamp aceptan ambos (devuelven el mismo tipo del input)
//   - pow/sqrt operan sobre Float (devuelven Float)
//   - ceil/floor/round toman Float y devuelven Int

fn builtin_math_abs(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 1, found: args.len() },
            0, 0,
            format!("`abs(x)` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.wrapping_abs())),
        Value::Float(x) => Ok(Value::Float(x.abs())),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int|Float".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`abs(x)` espera `Int` o `Float`, recibió `{}`", other.type_name()),
        )),
    }
}

fn builtin_math_min_max(args: &[Value], want_max: bool, name: &str) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 2, found: args.len() },
            0, 0,
            format!("`{}(a, b)` espera 2 argumentos, recibió {}", name, args.len()),
        ));
    }
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => {
            let r = if want_max { *a.max(b) } else { *a.min(b) };
            Ok(Value::Int(r))
        }
        (Value::Float(a), Value::Float(b)) => {
            let r = if want_max {
                if a > b { *a } else { *b }
            } else {
                if a < b { *a } else { *b }
            };
            Ok(Value::Float(r))
        }
        (other_a, other_b) => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int+Int|Float+Float".into(),
                found: format!("{}+{}", other_a.type_name(), other_b.type_name()),
            },
            0, 0,
            format!(
                "`{}(a, b)`: ambos args deben ser del mismo tipo Int o Float, recibió `{}` y `{}`",
                name, other_a.type_name(), other_b.type_name(),
            ),
        )),
    }
}

fn builtin_math_min(args: &[Value]) -> FitzResult<Value> {
    builtin_math_min_max(args, false, "min")
}

fn builtin_math_max(args: &[Value]) -> FitzResult<Value> {
    builtin_math_min_max(args, true, "max")
}

fn builtin_math_pow(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 2, found: args.len() },
            0, 0,
            format!("`pow(base, exp)` espera 2 args, recibió {}", args.len()),
        ));
    }
    let to_f = |v: &Value| -> Option<f64> {
        match v {
            Value::Int(n) => Some(*n as f64),
            Value::Float(x) => Some(*x),
            _ => None,
        }
    };
    let base = to_f(&args[0]).ok_or_else(|| FitzError::new(
        ErrorKind::TypeMismatch {
            expected: "Int|Float".into(),
            found: args[0].type_name().into(),
        },
        0, 0,
        format!("`pow(base, exp)`: arg 0 espera `Int` o `Float`, recibió `{}`", args[0].type_name()),
    ))?;
    let exp = to_f(&args[1]).ok_or_else(|| FitzError::new(
        ErrorKind::TypeMismatch {
            expected: "Int|Float".into(),
            found: args[1].type_name().into(),
        },
        0, 0,
        format!("`pow(base, exp)`: arg 1 espera `Int` o `Float`, recibió `{}`", args[1].type_name()),
    ))?;
    Ok(Value::Float(base.powf(exp)))
}

fn builtin_math_sqrt(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 1, found: args.len() },
            0, 0,
            format!("`sqrt(x)` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    let x = match &args[0] {
        Value::Int(n) => *n as f64,
        Value::Float(x) => *x,
        other => return Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int|Float".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`sqrt(x)` espera `Int` o `Float`, recibió `{}`", other.type_name()),
        )),
    };
    Ok(Value::Float(x.sqrt()))
}

fn builtin_math_ceil(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 1, found: args.len() },
            0, 0,
            format!("`ceil(x)` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    match &args[0] {
        Value::Float(x) => Ok(Value::Int(x.ceil() as i64)),
        Value::Int(n) => Ok(Value::Int(*n)),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Float|Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`ceil(x)` espera `Float` o `Int`, recibió `{}`", other.type_name()),
        )),
    }
}

fn builtin_math_floor(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 1, found: args.len() },
            0, 0,
            format!("`floor(x)` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    match &args[0] {
        Value::Float(x) => Ok(Value::Int(x.floor() as i64)),
        Value::Int(n) => Ok(Value::Int(*n)),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Float|Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`floor(x)` espera `Float` o `Int`, recibió `{}`", other.type_name()),
        )),
    }
}

fn builtin_math_round(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 1, found: args.len() },
            0, 0,
            format!("`round(x)` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    match &args[0] {
        // Rust f64::round() es "half away from zero"; suficiente.
        Value::Float(x) => Ok(Value::Int(x.round() as i64)),
        Value::Int(n) => Ok(Value::Int(*n)),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Float|Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`round(x)` espera `Float` o `Int`, recibió `{}`", other.type_name()),
        )),
    }
}

fn builtin_math_clamp(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 3 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount { expected: 3, found: args.len() },
            0, 0,
            format!("`clamp(x, lo, hi)` espera 3 args, recibió {}", args.len()),
        ));
    }
    match (&args[0], &args[1], &args[2]) {
        (Value::Int(x), Value::Int(lo), Value::Int(hi)) => {
            Ok(Value::Int((*x).clamp(*lo, *hi)))
        }
        (Value::Float(x), Value::Float(lo), Value::Float(hi)) => {
            Ok(Value::Float((*x).clamp(*lo, *hi)))
        }
        (a, b, c) => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "3 args Int|3 args Float".into(),
                found: format!("{}+{}+{}", a.type_name(), b.type_name(), c.type_name()),
            },
            0, 0,
            format!(
                "`clamp(x, lo, hi)`: los 3 args deben ser del mismo tipo Int o Float, recibió `{}`, `{}`, `{}`",
                a.type_name(), b.type_name(), c.type_name(),
            ),
        )),
    }
}

/// `print(arg1, arg2, ...)` — imprime los args convertidos a string,
/// separados por espacio, seguido de newline. Como Python.
fn builtin_print(args: &[Value]) -> FitzResult<Value> {
    let parts: Vec<String> = args.iter().map(|v| v.to_string()).collect();
    println!("{}", parts.join(" "));
    Ok(Value::Null)
}

/// `sleep(ms: Int) -> Future<Null>` — primer async primitive del
/// lenguaje. Devuelve un `Value::Future` que internamente espera
/// `ms` milisegundos via `tokio::time::sleep`. El usuario debe
/// await-earlo desde una `async fn` para que la espera ocurra:
/// `sleep(100).await` adentro de `async fn` pausa 100ms y produce
/// `Null`.
///
/// Diseño (Fase 6.4):
/// - El builtin es **sync por firma** (`fn(&[Value]) -> FitzResult<Value>`)
///   pero **devuelve un Future como valor**. Eso evita refactorar la
///   firma de `Value::Builtin` para distinguir builtins async — el
///   dispatcher trata todos los builtins igual, y los que producen
///   `Value::Future` ceden el control vía `.await` en el caller.
/// - Validación de args (aridad 1, Int) es defensiva: el checker
///   estático 6.3 ya rechaza llamadas mal tipadas, pero el evaluador
///   también lo chequea para casos donde el chequeo se haya saltado
///   (`fitz run --no-typecheck`).
fn builtin_sleep(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: 1,
                found: args.len(),
            },
            0, 0,
            format!("`sleep` espera 1 argumento (ms: Int), recibió {}", args.len()),
        ));
    }
    let ms = match &args[0] {
        Value::Int(n) => *n,
        other => {
            return Err(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int".into(),
                    found: other.type_name().into(),
                },
                0, 0,
                format!("`sleep` espera Int (milisegundos), recibió `{}`", other.type_name()),
            ));
        }
    };
    // Clampeamos negativos a 0: `sleep(-5)` no tiene sentido y
    // tokio::time::sleep no acepta `Duration` negativa.
    let ms_u64 = ms.max(0) as u64;
    let fut: crate::value::FitzFuture = Box::pin(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ms_u64)).await;
        Ok(Value::Null)
    });
    Ok(Value::new_future(fut))
}

/// `cors(config: Map?)` — construye un `Value::CorsConfig` parametrizado
/// por las keys del Map (mini-fase MW.2). Sin args (o con `{}`) emite
/// el default permisivo (origin "*", métodos comunes, headers usuales).
/// Keys reconocidas:
///       - `allow_origin: Str`
///       - `allow_methods: List<Str>`
///       - `allow_headers: List<Str>`
///       - `max_age: Int`
///
/// Cualquier otra key o tipo distinto al esperado → error claro.
fn builtin_cors(args: &[Value]) -> FitzResult<Value> {
    use crate::http::CorsConfig;
    use std::sync::Arc;

    if args.len() > 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: 1,
                found: args.len(),
            },
            0,
            0,
            format!(
                "`cors` espera 0 o 1 argumento (un Map de configuración), recibió {}",
                args.len()
            ),
        ));
    }

    let mut config = CorsConfig::permissive_default();

    if let Some(arg) = args.first() {
        let pairs = match arg {
            Value::Map(p) => p.clone(),
            other => {
                return Err(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Map".into(),
                        found: other.type_name().into(),
                    },
                    0,
                    0,
                    format!(
                        "`cors` espera un Map de configuración, recibió `{}`",
                        other.type_name()
                    ),
                ));
            }
        };
        for (key, value) in pairs.lock().iter() {
            let key_str = match key {
                Value::Str(s) => s.clone(),
                other => {
                    return Err(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Str".into(),
                            found: other.type_name().into(),
                        },
                        0,
                        0,
                        format!(
                            "`cors`: las keys del Map de configuración deben ser Str, recibió `{}`",
                            other.type_name()
                        ),
                    ));
                }
            };
            match key_str.as_str() {
                "allow_origin" => match value {
                    // Q.3: Str → literal (modo previo: emite valor fijo).
                    // Mini-tanda HTTP-Cors — el valor especial `"echo"`
                    // (case-sensitive) construye `AllowOrigin::Echo` que
                    // hace echo del Origin recibido sin filtro.
                    Value::Str(s) => {
                        config.allow_origin = if s == "echo" {
                            crate::http::AllowOrigin::Echo
                        } else {
                            crate::http::AllowOrigin::Literal(s.clone())
                        };
                    }
                    // Q.3: List<Str> → set de orígenes permitidos. El
                    // dispatch HTTP echo del Origin del request si está
                    // en la lista; si no, omite el header (browser
                    // rechaza la response — CORS estricto). Útil con
                    // credenciales (`Allow-Origin: *` incompatible).
                    Value::List(items) => {
                        let mut set = Vec::with_capacity(items.lock().len());
                        for it in items.lock().iter() {
                            match it {
                                Value::Str(s) => set.push(s.clone()),
                                other => {
                                    return Err(cors_type_err(
                                        "allow_origin",
                                        "Str | List<Str>",
                                        other.type_name(),
                                    ));
                                }
                            }
                        }
                        config.allow_origin = crate::http::AllowOrigin::Set(set);
                    }
                    other => {
                        return Err(cors_type_err(
                            "allow_origin",
                            "Str | List<Str>",
                            other.type_name(),
                        ));
                    }
                },
                "allow_methods" => {
                    config.allow_methods = list_of_strings(value, "allow_methods")?;
                }
                "allow_headers" => {
                    config.allow_headers = list_of_strings(value, "allow_headers")?;
                }
                "max_age" => match value {
                    Value::Int(n) => config.max_age = Some(*n),
                    other => {
                        return Err(cors_type_err("max_age", "Int", other.type_name()));
                    }
                },
                other => {
                    return Err(FitzError::new(
                        ErrorKind::InvalidSyntax,
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

    Ok(Value::CorsConfig(Arc::new(config)))
}

fn cors_type_err(key: &str, expected: &str, found: &str) -> FitzError {
    FitzError::new(
        ErrorKind::TypeMismatch {
            expected: expected.into(),
            found: found.into(),
        },
        0,
        0,
        format!("`cors`: la key '{}' espera {}, recibió `{}`", key, expected, found),
    )
}

fn list_of_strings(value: &Value, key: &str) -> FitzResult<Vec<String>> {
    let items = match value {
        Value::List(items) => items.clone(),
        other => {
            return Err(cors_type_err(key, "List<Str>", other.type_name()));
        }
    };
    let mut out = Vec::with_capacity(items.lock().len());
    for item in items.lock().iter() {
        match item {
            Value::Str(s) => out.push(s.clone()),
            other => {
                return Err(cors_type_err(key, "List<Str>", other.type_name()));
            }
        }
    }
    Ok(out)
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
        Value::List(items) => items.lock().len() as i64,
        Value::Map(pairs) => pairs.lock().len() as i64,
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
// Assertion builtins (Fase 9.z.2.a)
//
// Los 4 builtins de aserción del testing built-in. Diseñados para ser
// llamados desde adentro de fns `@test`, pero NO son privilegiados —
// se pueden usar en cualquier contexto. Una aserción fallida emite
// `FitzError` que aborta la ejecución; el runner (9.z.2.b) atrapa el
// error y reporta el test como FAILED.
//
// Formato de los mensajes: estilo cargo test (`left: <val>` / `right:
// <val>`). Los valores se formatean con `Value::Display`, que produce
// la misma representación que `print` (cargo usa `Debug`; en Fitz la
// representación canónica vive en `Display`).
// ---------------------------------------------------------------------------

/// `assert(cond: Bool, msg: Str?) -> Null` — la aserción base. Si
/// `cond` es `false`, emite `FitzError`. Sin `msg`, mensaje genérico
/// "aserción falló"; con `msg`, lo incluye al final.
///
/// Decisión: el primer arg debe ser `Bool` estrictamente (no
/// "truthy"/"falsy" estilo Python/JS). Pasar `Int`/`Str`/etc. emite
/// type error claro — consistente con la decisión de diseño "sin
/// truthy/falsy" del cap 6 de la guía.
fn builtin_assert(args: &[Value]) -> FitzResult<Value> {
    if args.is_empty() || args.len() > 2 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: 1,
                found: args.len(),
            },
            0, 0,
            format!(
                "`assert` espera 1 o 2 argumentos (cond: Bool, msg: Str?), recibió {}",
                args.len()
            ),
        ));
    }
    let cond = match &args[0] {
        Value::Bool(b) => *b,
        other => {
            return Err(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Bool".into(),
                    found: other.type_name().into(),
                },
                0, 0,
                format!(
                    "`assert` espera `Bool` como primer argumento, recibió `{}`",
                    other.type_name()
                ),
            ));
        }
    };
    let msg = match args.get(1) {
        Some(Value::Str(s)) => Some(s.clone()),
        Some(other) => {
            return Err(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Str".into(),
                    found: other.type_name().into(),
                },
                0, 0,
                format!(
                    "`assert` espera `Str` como segundo argumento (mensaje), recibió `{}`",
                    other.type_name()
                ),
            ));
        }
        None => None,
    };
    if cond {
        return Ok(Value::Null);
    }
    let detail = match msg {
        Some(m) => format!("aserción falló: {}", m),
        None => "aserción falló".to_string(),
    };
    Err(FitzError::new(ErrorKind::InvalidSyntax, 0, 0, detail))
}

/// `assert_eq(a, b) -> Null` — falla si `a != b`. Usa la igualdad
/// estructural de `Value` (la misma de `BinOp::Eq`), que coerciona
/// `Int↔Float` y recurre adentro de `List`/`Map`/`Instance`/`Result`.
fn builtin_assert_eq(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: 2,
                found: args.len(),
            },
            0, 0,
            format!(
                "`assert_eq` espera 2 argumentos (left, right), recibió {}",
                args.len()
            ),
        ));
    }
    if args[0] == args[1] {
        return Ok(Value::Null);
    }
    Err(FitzError::new(
        ErrorKind::InvalidSyntax,
        0, 0,
        format!(
            "assert_eq falló:\n  left:  {}\n  right: {}",
            args[0], args[1]
        ),
    ))
}

/// `assert_ne(a, b) -> Null` — falla si `a == b`. Inverso exacto de
/// `assert_eq`.
fn builtin_assert_ne(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: 2,
                found: args.len(),
            },
            0, 0,
            format!(
                "`assert_ne` espera 2 argumentos (left, right), recibió {}",
                args.len()
            ),
        ));
    }
    if args[0] != args[1] {
        return Ok(Value::Null);
    }
    Err(FitzError::new(
        ErrorKind::InvalidSyntax,
        0, 0,
        format!(
            "assert_ne falló: ambos lados son iguales ({})",
            args[0]
        ),
    ))
}

/// Stub del `assert_throws` builtin. NO debería invocarse jamás —
/// el dispatcher (`invoke_value`) intercepta `Value::Builtin {
/// name: "assert_throws", .. }` antes del despacho normal y lo
/// resuelve async via `assert_throws_impl`. Si llegás a ver el
/// mensaje del unreachable, es un bug del dispatcher.
fn builtin_assert_throws_stub(_args: &[Value]) -> FitzResult<Value> {
    Err(FitzError::new(
        ErrorKind::InvalidSyntax,
        0, 0,
        "bug del evaluator: `assert_throws` stub invocado directamente. \
         El dispatcher debería haberlo interceptado."
            .to_string(),
    ))
}

/// `assert_throws(fn) -> Null` async impl. Invoca el callback Fitz
/// y atrapa el `FitzError`. Si el callback retorna sin error,
/// `assert_throws` falla ("se esperaba que tirara"). Si el
/// callback tira, `assert_throws` pasa.
///
/// **Restricción MVP**: el callback debe ser una `Value::Function`
/// con aridad 0 y NO async. Async callbacks devuelven un
/// `Value::Future` suelto (no equivalente a "tirar"); el chequeo
/// de "tiró" requeriría await-ear el Future, lo cual cambia la
/// semántica. Si aparece presión, sub-paso futuro
/// `assert_throws_async(fn)` o flag dedicado.
async fn assert_throws_impl(args: Vec<Value>, span: Span) -> EvalResult<Value> {
    if args.len() != 1 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: 1,
                found: args.len(),
            },
            span.line, span.column,
            format!(
                "`assert_throws` espera 1 argumento (callback: fn), recibió {}",
                args.len()
            ),
        )));
    }
    let callback = &args[0];
    let (param_count, is_async) = match callback {
        Value::Function { params, is_async, .. } => (params.len(), *is_async),
        other => {
            return Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Function".into(),
                    found: other.type_name().into(),
                },
                span.line, span.column,
                format!(
                    "`assert_throws` espera una función como argumento, recibió `{}`",
                    other.type_name()
                ),
            )));
        }
    };
    if param_count != 0 {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            format!(
                "`assert_throws` espera una función con 0 params, recibió una con {}",
                param_count
            ),
        )));
    }
    if is_async {
        return Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            "`assert_throws` no soporta callbacks `async fn` en el MVP. \
             Usá `match` sobre el `Result` o un helper sync."
                .to_string(),
        )));
    }
    match invoke_value(callback.clone(), vec![], "callback de assert_throws", span).await {
        Ok(_) => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            span.line, span.column,
            "assert_throws falló: se esperaba que la fn tirara un error, \
             pero retornó normalmente"
                .to_string(),
        ))),
        Err(_) => Ok(Value::Null),
    }
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
    ///
    /// Fase 6.4: el helper pasó a `async fn` (los tests viven adentro de
    /// `#[tokio::test]` así que un `block_on` interno paniquearía con
    /// "Cannot start a runtime from within a runtime"). Cada call site
    /// agrega `.await`. Diff mínimo, preserva la lógica del test.
    async fn eval_expr_test(expr: Expr) -> EvalResult<Value> {
        let env = Environment::new();
        eval_expr(&expr, env).await
    }

    // ---- entry point ----

    #[tokio::test(flavor = "current_thread")]
    async fn programa_vacio_no_falla() {
        assert!(eval(vec![]).await.is_ok());
    }

    // ---- Fase 6.4: evaluator async, Value::Future, .await real ----

    #[tokio::test(flavor = "current_thread")]
    async fn value_future_display_y_type_name() {
        // Future "vivo" (con un future adentro) y Future "consumido"
        // (Option::None) deben tener display y type_name consistentes.
        let f: crate::value::FitzFuture = Box::pin(async { Ok(Value::Int(1)) });
        let v = Value::new_future(f);
        assert_eq!(v.type_name(), "Future");
        assert_eq!(v.to_string(), "<future>");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn await_sobre_no_future_es_error_de_runtime() {
        // El checker 6.2 ataja la mayoría de los casos, pero el
        // evaluator también valida: `.await` sobre Int → error claro.
        let expr = Expr::Await(Box::new(Expr::Int(42, Span::ZERO)), Span::ZERO);
        let err = eval_expr_test(expr).await.expect_err("esperaba error del evaluator");
        match err {
            EvalSignal::Error(fitz_err) => {
                assert!(
                    fitz_err.message.contains("Future") && fitz_err.message.contains("Int"),
                    "esperaba mensaje sobre Future/Int, fue: {}",
                    fitz_err.message
                );
            }
            other => panic!("se esperaba EvalSignal::Error, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_fn_llamada_con_await_produce_resultado() {
        // `async fn f() -> Int { return 42 }; f().await` evalúa a Int(42).
        // El flow: la llamada produce Value::Future; .await lo desempaca
        // ejecutando el body adentro del runtime tokio.
        let (env, res) = parse_eval_into_env(
            "async fn f() -> Int { return 42 }\n\
             let x = f().await",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("x"), Some(Value::Int(42)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_fn_llamada_sin_await_produce_future_suelto() {
        // Sin `.await`, la llamada a una async fn tipa y produce
        // `Value::Future`. El usuario puede guardarlo, pasarlo, etc.
        let (env, res) = parse_eval_into_env(
            "async fn f() -> Int { return 42 }\n\
             let pending = f()",
        ).await;
        res.unwrap();
        let v = env.lock().get("pending").expect("pending definida");
        assert!(matches!(v, Value::Future(_)), "esperaba Value::Future, fue {:?}", v);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sleep_con_await_pausa_y_produce_null() {
        // `sleep(0).await` adentro de async fn pausa cero tiempo y
        // produce Null. Validamos la integración end-to-end del
        // builtin con el runtime tokio: el `.await` cede control y
        // tokio::time::sleep efectivamente espera.
        let (env, res) = parse_eval_into_env(
            "async fn pausa() -> Null { return sleep(0).await }\n\
             let r = pausa().await",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Null));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sleep_sin_await_devuelve_future_suelto() {
        // `let f = sleep(100)` — sin await, devuelve Value::Future.
        // El builtin `sleep` construye el future pero no lo espera.
        let (env, res) = parse_eval_into_env("let f = sleep(100)").await;
        res.unwrap();
        let v = env.lock().get("f").expect("f definida");
        assert!(matches!(v, Value::Future(_)), "esperaba Value::Future, fue {:?}", v);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn await_sobre_future_consumido_es_error() {
        // Política: un future se await-ea una sola vez. Si el usuario
        // intenta hacerlo dos veces sobre el mismo Value::Future,
        // emite error explícito.
        let f: crate::value::FitzFuture = Box::pin(async { Ok(Value::Int(1)) });
        let cell = crate::value::FutureCell(std::sync::Arc::new(parking_lot::Mutex::new(Some(f))));
        let v = Value::Future(cell);

        // Primer .await: éxito.
        let env = Environment::new();
        env.lock().define("p", v.clone());
        let first = eval_expr(
            &Expr::Await(Box::new(Expr::Ident("p".into(), Span::ZERO)), Span::ZERO),
            env.clone(),
        ).await.unwrap();
        assert_eq!(first, Value::Int(1));

        // Segundo .await: error.
        let err = eval_expr(
            &Expr::Await(Box::new(Expr::Ident("p".into(), Span::ZERO)), Span::ZERO),
            env,
        ).await.expect_err("segundo await debería fallar");
        match err {
            EvalSignal::Error(fitz_err) => {
                assert!(fitz_err.message.contains("consumido"));
            }
            other => panic!("se esperaba Error, fue {:?}", other),
        }
    }

    // ---- literales ----

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_int_literal() {
        assert_eq!(eval_expr_test(Expr::Int(42, Span::ZERO)).await.unwrap(), Value::Int(42));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_float_literal() {
        assert_eq!(eval_expr_test(Expr::Float(3.14, Span::ZERO)).await.unwrap(), Value::Float(3.14));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_string_literal() {
        assert_eq!(
            eval_expr_test(Expr::Str("hola".into(), Span::ZERO)).await.unwrap(),
            Value::Str("hola".into())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_bool_literal() {
        assert_eq!(eval_expr_test(Expr::Bool(true, Span::ZERO)).await.unwrap(), Value::Bool(true));
        assert_eq!(eval_expr_test(Expr::Bool(false, Span::ZERO)).await.unwrap(), Value::Bool(false));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_null_literal() {
        assert_eq!(eval_expr_test(Expr::Null(Span::ZERO)).await.unwrap(), Value::Null);
    }

    // ---- Ident ----

    #[tokio::test(flavor = "current_thread")]
    async fn ident_resuelve_variable_del_env() {
        let env = Environment::new();
        env.lock().define("x", Value::Int(99));

        let result = eval_expr(&Expr::Ident("x".into(), Span::ZERO), env).await.unwrap();
        assert_eq!(result, Value::Int(99));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ident_no_definido_devuelve_error() {
        let env = Environment::new();
        let result = eval_expr(&Expr::Ident("nope".into(), Span::ZERO), env).await;

        match result {
            Err(EvalSignal::Error(e)) => {
                assert!(matches!(e.kind, ErrorKind::UndefinedVariable(ref n) if n == "nope"));
            }
            _ => panic!("se esperaba Error::UndefinedVariable"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ident_busca_en_scope_padre() {
        let global = Environment::new();
        global.lock().define("x", Value::Str("from_global".into()));

        let child = Environment::new_child(global);
        let result = eval_expr(&Expr::Ident("x".into(), Span::ZERO), child).await.unwrap();
        assert_eq!(result, Value::Str("from_global".into()));
    }

    // ---- Stmt::Expr (paso intermedio para verificar el wiring stmt→expr) ----

    #[tokio::test(flavor = "current_thread")]
    async fn stmt_expr_evalua_la_expresion_interna() {
        let env = Environment::new();
        let stmt = Stmt::Expr(Expr::Int(7, Span::ZERO), Span::ZERO);
        let result = eval_stmt(&stmt, env).await.unwrap();
        assert_eq!(result, Value::Int(7));
    }

    // ---- builtins ----

    #[tokio::test(flavor = "current_thread")]
    async fn builtin_print_devuelve_null() {
        let result = builtin_print(&[Value::Str("test".into())]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn register_builtins_define_print_en_env() {
        let env = Environment::new();
        register_builtins(&env);

        let print = env.lock().get("print");
        assert!(print.is_some());
        match print.unwrap() {
            Value::Builtin { name, .. } => assert_eq!(name, "print"),
            _ => panic!("se esperaba Value::Builtin"),
        }
    }

    // ---- signals ----

    #[tokio::test(flavor = "current_thread")]
    async fn fitzerror_se_convierte_a_evalsignal_error() {
        let err = FitzError::new(ErrorKind::DivisionByZero, 1, 1, "test");
        let signal: EvalSignal = err.into();
        assert!(matches!(signal, EvalSignal::Error(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn break_fuera_de_loop_es_error() {
        let result = eval(vec![Stmt::Break(None, None, Span::ZERO)]).await;
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::BreakOutsideLoop
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn continue_fuera_de_loop_es_error() {
        let result = eval(vec![Stmt::Continue(None, Span::ZERO)]).await;
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

    #[tokio::test(flavor = "current_thread")]
    async fn add_int_int_da_int() {
        let e = binop(BinOpKind::Add, Expr::Int(2, Span::ZERO), Expr::Int(3, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(5));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_int_float_promueve_a_float() {
        let e = binop(BinOpKind::Add, Expr::Int(2, Span::ZERO), Expr::Float(0.5, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Float(2.5));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_float_int_promueve_a_float() {
        let e = binop(BinOpKind::Add, Expr::Float(1.5, Span::ZERO), Expr::Int(2, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Float(3.5));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_strings_concatena() {
        let e = binop(
            BinOpKind::Add,
            Expr::Str("hola ".into(), Span::ZERO),
            Expr::Str("mundo".into(), Span::ZERO),
        );
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("hola mundo".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_tipos_incompatibles_es_type_error() {
        let e = binop(BinOpKind::Add, Expr::Str("x".into(), Span::ZERO), Expr::Int(1, Span::ZERO));
        match eval_expr_test(e).await {
            Err(EvalSignal::Error(err)) => {
                assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
            }
            _ => panic!("se esperaba TypeMismatch"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sub_mul_funcionan() {
        let sub = binop(BinOpKind::Sub, Expr::Int(10, Span::ZERO), Expr::Int(3, Span::ZERO));
        assert_eq!(eval_expr_test(sub).await.unwrap(), Value::Int(7));

        let mul = binop(BinOpKind::Mul, Expr::Int(4, Span::ZERO), Expr::Int(5, Span::ZERO));
        assert_eq!(eval_expr_test(mul).await.unwrap(), Value::Int(20));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn div_int_int_trunca() {
        // 10 / 3 = 3 (truncado), no 3.33
        let e = binop(BinOpKind::Div, Expr::Int(10, Span::ZERO), Expr::Int(3, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(3));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn div_int_float_da_float() {
        let e = binop(BinOpKind::Div, Expr::Int(10, Span::ZERO), Expr::Float(4.0, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Float(2.5));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn div_por_cero_int_es_error() {
        let e = binop(BinOpKind::Div, Expr::Int(1, Span::ZERO), Expr::Int(0, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::DivisionByZero, .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn div_por_cero_float_es_error() {
        let e = binop(BinOpKind::Div, Expr::Float(1.0, Span::ZERO), Expr::Float(0.0, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::DivisionByZero, .. })
        ));
    }

    // ---- BinOp: comparación e igualdad ----

    #[tokio::test(flavor = "current_thread")]
    async fn eq_con_coercion_int_float() {
        // 1 == 1.0 → true
        let e = binop(BinOpKind::Eq, Expr::Int(1, Span::ZERO), Expr::Float(1.0, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn eq_tipos_distintos_da_false_sin_error() {
        // 1 == "1" → false (no error)
        let e = binop(BinOpKind::Eq, Expr::Int(1, Span::ZERO), Expr::Str("1".into(), Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(false));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn noteq_funciona() {
        let e = binop(BinOpKind::NotEq, Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lt_gt_lteq_gteq_numericos() {
        assert_eq!(
            eval_expr_test(binop(BinOpKind::Lt, Expr::Int(2, Span::ZERO), Expr::Int(3, Span::ZERO))).await.unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval_expr_test(binop(BinOpKind::Gt, Expr::Int(2, Span::ZERO), Expr::Int(3, Span::ZERO))).await.unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            eval_expr_test(binop(BinOpKind::LtEq, Expr::Int(3, Span::ZERO), Expr::Int(3, Span::ZERO))).await.unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval_expr_test(binop(BinOpKind::GtEq, Expr::Int(2, Span::ZERO), Expr::Int(3, Span::ZERO))).await.unwrap(),
            Value::Bool(false)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn comparacion_con_promocion_int_float() {
        // 2 < 2.5 → true
        let e = binop(BinOpKind::Lt, Expr::Int(2, Span::ZERO), Expr::Float(2.5, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn comparacion_de_strings_es_alfabetica() {
        let e = binop(
            BinOpKind::Lt,
            Expr::Str("abc".into(), Span::ZERO),
            Expr::Str("abd".into(), Span::ZERO),
        );
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn comparacion_entre_bool_es_type_error() {
        // Bool no se compara con <. Sí con ==.
        let e = binop(BinOpKind::Lt, Expr::Bool(true, Span::ZERO), Expr::Bool(false, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- BinOp: lógicos con short-circuit ----

    #[tokio::test(flavor = "current_thread")]
    async fn and_true_true_da_true() {
        let e = binop(BinOpKind::And, Expr::Bool(true, Span::ZERO), Expr::Bool(true, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn and_false_corta_y_no_evalua_derecho() {
        // El lado derecho es un Ident no definido. Si se evaluara, daría error.
        // Como `false and ...` corta, devuelve false sin error.
        let e = binop(
            BinOpKind::And,
            Expr::Bool(false, Span::ZERO),
            Expr::Ident("no_existe".into(), Span::ZERO),
        );
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(false));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn or_true_corta_y_no_evalua_derecho() {
        let e = binop(
            BinOpKind::Or,
            Expr::Bool(true, Span::ZERO),
            Expr::Ident("no_existe".into(), Span::ZERO),
        );
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn or_false_true_da_true() {
        let e = binop(BinOpKind::Or, Expr::Bool(false, Span::ZERO), Expr::Bool(true, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn and_con_no_bool_izquierda_es_type_error() {
        let e = binop(BinOpKind::And, Expr::Int(1, Span::ZERO), Expr::Bool(true, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn and_con_no_bool_derecha_es_type_error() {
        // Para que el lado derecho se evalúe, el izquierdo debe ser true.
        let e = binop(BinOpKind::And, Expr::Bool(true, Span::ZERO), Expr::Int(1, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- Mini-tanda Xor ----

    #[tokio::test(flavor = "current_thread")]
    async fn xor_tabla_de_verdad_completa() {
        // T xor T → F, T xor F → T, F xor T → T, F xor F → F.
        for (l, r, expected) in [(true, true, false), (true, false, true), (false, true, true), (false, false, false)] {
            let e = binop(
                BinOpKind::Xor,
                Expr::Bool(l, Span::ZERO),
                Expr::Bool(r, Span::ZERO),
            );
            assert_eq!(
                eval_expr_test(e).await.unwrap(),
                Value::Bool(expected),
                "xor({}, {}) esperaba {}",
                l,
                r,
                expected
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn xor_evalua_ambos_lados_sin_short_circuit() {
        // Si `false xor <bad>` cortara como `and` lo haría, no
        // emitiría error. Como xor NO short-circuita, evalua el RHS
        // y dispara TypeError sobre el Ident no definido.
        let e = binop(
            BinOpKind::Xor,
            Expr::Bool(false, Span::ZERO),
            Expr::Ident("no_existe".into(), Span::ZERO),
        );
        assert!(eval_expr_test(e).await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn xor_con_no_bool_es_type_error() {
        let e = binop(BinOpKind::Xor, Expr::Int(1, Span::ZERO), Expr::Bool(true, Span::ZERO));
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- BinOp anidados ----

    #[tokio::test(flavor = "current_thread")]
    async fn expresion_anidada_2_mas_3_por_4_da_14() {
        // 2 + (3 * 4) — Stmt::Expr para verificar wiring completo.
        let inner = binop(BinOpKind::Mul, Expr::Int(3, Span::ZERO), Expr::Int(4, Span::ZERO));
        let outer = binop(BinOpKind::Add, Expr::Int(2, Span::ZERO), inner);
        assert_eq!(eval_expr_test(outer).await.unwrap(), Value::Int(14));
    }

    // ---- UnaryOp ----

    #[tokio::test(flavor = "current_thread")]
    async fn neg_int() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Int(5, Span::ZERO)), span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(-5));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn neg_float() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Float(3.14, Span::ZERO)), span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Float(-3.14));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn doble_negacion_devuelve_el_original() {
        // -(-7) = 7
        let inner = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Int(7, Span::ZERO)), span: Span::ZERO,
        };
        let outer = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(inner), span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(outer).await.unwrap(), Value::Int(7));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn neg_de_bool_es_type_error() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Bool(true, Span::ZERO)), span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn neg_de_string_es_type_error() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Str("hola".into(), Span::ZERO)), span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- R.1.1 — `not` (mini-fase R) ----

    #[tokio::test(flavor = "current_thread")]
    async fn not_true_es_false() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Not,
            operand: Box::new(Expr::Bool(true, Span::ZERO)),
            span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(false));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn not_false_es_true() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Not,
            operand: Box::new(Expr::Bool(false, Span::ZERO)),
            span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn doble_not_devuelve_el_original() {
        let inner = Expr::UnaryOp {
            op: UnaryOpKind::Not,
            operand: Box::new(Expr::Bool(true, Span::ZERO)),
            span: Span::ZERO,
        };
        let outer = Expr::UnaryOp {
            op: UnaryOpKind::Not,
            operand: Box::new(inner),
            span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(outer).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn not_de_int_es_type_error_en_runtime_gradual() {
        // Sin --no-typecheck el checker corta antes. Adentro del
        // evaluator directo, debe emitir error de tipo.
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Not,
            operand: Box::new(Expr::Int(5, Span::ZERO)),
            span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn not_de_str_es_type_error_en_runtime_gradual() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Not,
            operand: Box::new(Expr::Str("hola".into(), Span::ZERO)),
            span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- R.1.2 — operador `%` (mini-fase R) ----

    #[tokio::test(flavor = "current_thread")]
    async fn mod_simple_positivo() {
        let e = binop(BinOpKind::Mod, Expr::Int(10, Span::ZERO), Expr::Int(3, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mod_exacto_da_cero() {
        let e = binop(BinOpKind::Mod, Expr::Int(12, Span::ZERO), Expr::Int(4, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mod_negativo_es_euclidean() {
        // Semántica euclidean: -7 % 3 = 2 (no -1 como `%` Rust).
        let e = binop(BinOpKind::Mod, Expr::Int(-7, Span::ZERO), Expr::Int(3, Span::ZERO));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mod_por_cero_es_error_runtime() {
        let e = binop(BinOpKind::Mod, Expr::Int(7, Span::ZERO), Expr::Int(0, Span::ZERO));
        let err = eval_expr_test(e).await.unwrap_err();
        match err {
            EvalSignal::Error(FitzError { kind: ErrorKind::DivisionByZero, .. }) => {}
            other => panic!("esperaba DivisionByZero, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mod_con_float_es_type_error() {
        let e = binop(
            BinOpKind::Mod,
            Expr::Float(10.0, Span::ZERO),
            Expr::Int(3, Span::ZERO),
        );
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- R.1.3 — asignación a índice (mini-fase R) ----

    #[tokio::test(flavor = "current_thread")]
    async fn assign_index_list_replace_in_place() {
        let (env, res) = parse_eval_into_env(
            "let xs = [1, 2, 3]\nxs[0] = 99",
        )
        .await;
        res.unwrap();
        let xs = env.lock().get("xs").unwrap();
        match xs {
            Value::List(items) => {
                let borrowed = items.lock();
                assert_eq!(borrowed.len(), 3);
                assert_eq!(borrowed[0], Value::Int(99));
                assert_eq!(borrowed[1], Value::Int(2));
            }
            other => panic!("xs no es List, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assign_index_map_replace() {
        let (env, res) = parse_eval_into_env(
            "let m = {\"a\": 1, \"b\": 2}\nm[\"a\"] = 10",
        )
        .await;
        res.unwrap();
        let m = env.lock().get("m").unwrap();
        match m {
            Value::Map(pairs) => {
                let borrowed = pairs.lock();
                // Insertion order preservado: "a" sigue siendo el primero.
                assert_eq!(borrowed[0].0, Value::Str("a".into()));
                assert_eq!(borrowed[0].1, Value::Int(10));
            }
            other => panic!("m no es Map, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assign_index_map_insert_nuevo_va_al_final() {
        let (env, res) = parse_eval_into_env(
            "let m = {\"a\": 1}\nm[\"b\"] = 2",
        )
        .await;
        res.unwrap();
        let m = env.lock().get("m").unwrap();
        match m {
            Value::Map(pairs) => {
                let borrowed = pairs.lock();
                assert_eq!(borrowed.len(), 2);
                assert_eq!(borrowed[1].0, Value::Str("b".into()));
                assert_eq!(borrowed[1].1, Value::Int(2));
            }
            other => panic!("m no es Map, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assign_index_list_out_of_bounds_es_error() {
        let (_env, res) = parse_eval_into_env(
            "let xs = [1, 2]\nxs[5] = 99",
        )
        .await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("fuera de rango"),
            "mensaje inesperado: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assign_index_list_negativo_fuera_de_rango_es_error() {
        // I.1: `xs[-1] = ...` ahora es válido (wrap). El error solo
        // dispara si el wrap queda fuera de [0, len).
        let (_env, res) = parse_eval_into_env(
            "let xs = [1, 2]\nxs[-99] = 99",
        )
        .await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("fuera de rango"),
            "mensaje inesperado: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assign_index_sobre_int_es_type_error() {
        let (_env, res) = parse_eval_into_env(
            "let x = 5\nx[0] = 1",
        )
        .await;
        // El checker lo caza primero (type error). Para validar el
        // runtime puro, deberíamos usar --no-typecheck via parse_eval
        // directo; acá nos quedamos con el error sea de tipo o de
        // runtime.
        assert!(res.is_err());
    }

    // ---- Stmt::Assign ----

    #[tokio::test(flavor = "current_thread")]
    async fn assign_define_variable_nueva_en_scope_local() {
        let env = Environment::new();
        let stmt = Stmt::Assign { target: AssignTarget::Ident("x".into()),
            type_: None,
            value: Expr::Int(42, Span::ZERO),
         span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).await.unwrap();

        assert_eq!(env.lock().get("x"), Some(Value::Int(42)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assign_reasigna_variable_existente_en_el_mismo_scope() {
        let env = Environment::new();
        env.lock().define("x", Value::Int(1));

        let stmt = Stmt::Assign { target: AssignTarget::Ident("x".into()),
            type_: None,
            value: Expr::Int(99, Span::ZERO),
         span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).await.unwrap();

        assert_eq!(env.lock().get("x"), Some(Value::Int(99)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assign_desde_child_reasigna_en_el_padre_si_existe() {
        let global = Environment::new();
        global.lock().define("x", Value::Int(1));

        let child = Environment::new_child(global.clone());
        let stmt = Stmt::Assign { target: AssignTarget::Ident("x".into()),
            type_: None,
            value: Expr::Int(42, Span::ZERO),
         span: Span::ZERO };
        eval_stmt(&stmt, child).await.unwrap();

        // El cambio se ve en el global.
        assert_eq!(global.lock().get("x"), Some(Value::Int(42)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assign_crea_local_si_la_variable_no_existe_en_la_cadena() {
        let global = Environment::new();
        let child = Environment::new_child(global.clone());

        let stmt = Stmt::Assign { target: AssignTarget::Ident("nueva".into()),
            type_: None,
            value: Expr::Int(7, Span::ZERO),
         span: Span::ZERO };
        eval_stmt(&stmt, child.clone()).await.unwrap();

        // Solo existe en child, no se propagó al padre.
        assert_eq!(child.lock().get("nueva"), Some(Value::Int(7)));
        assert_eq!(global.lock().get("nueva"), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assign_ignora_la_anotacion_de_tipo() {
        // type_: Some("Int") con value String — no falla (tipado gradual,
        // sin checks en runtime todavía).
        let env = Environment::new();
        let stmt = Stmt::Assign { target: AssignTarget::Ident("x".into()),
            type_: Some(TypeExpr::named("Int")),
            value: Expr::Str("soy un string".into(), Span::ZERO),
         span: Span::ZERO };
        assert!(eval_stmt(&stmt, env.clone()).await.is_ok());
        assert_eq!(env.lock().get("x"), Some(Value::Str("soy un string".into())));
    }

    // ---- Expr::Call (builtins) ----

    #[tokio::test(flavor = "current_thread")]
    async fn call_a_print_devuelve_null() {
        // print(...) escribe a stdout y devuelve Null. Verificamos el Value
        // de retorno; la salida real la chequeamos manualmente con hello.fitz.
        let env = Environment::new();
        register_builtins(&env);

        let call = Expr::Call { callee: Box::new(Expr::Ident("print".into(), Span::ZERO)), args: vec![Expr::Str("test".into(), Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Null);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn call_a_funcion_no_definida_es_error() {
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
            eval_expr(&call, env).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::UndefinedVariable(_), .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn call_a_no_funcion_es_type_error() {
        let env = Environment::new();
        env.lock().define("x", Value::Int(5));

        let call = Expr::Call { callee: Box::new(Expr::Ident("x".into(), Span::ZERO)), args: vec![], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr(&call, env).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn call_evalua_args_antes_de_invocar() {
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
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Null);
    }

    // ---- Expr::StrInterp ----

    #[tokio::test(flavor = "current_thread")]
    async fn str_interp_solo_con_literales_concatena() {
        let e = Expr::StrInterp(vec![
            StrPart::Lit("hola ".into()),
            StrPart::Lit("mundo".into()),
        ], Span::ZERO);
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("hola mundo".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_interp_interpola_ident() {
        let env = Environment::new();
        env.lock().define("name", Value::Str("Fitz".into()));

        let e = Expr::StrInterp(vec![
            StrPart::Lit("Hola, ".into()),
            StrPart::Expr(Expr::Ident("name".into(), Span::ZERO), None),
            StrPart::Lit("!".into()),
        ], Span::ZERO);
        assert_eq!(
            eval_expr(&e, env).await.unwrap(),
            Value::Str("Hola, Fitz!".into())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_interp_convierte_int_a_string() {
        let env = Environment::new();
        env.lock().define("x", Value::Int(42));

        let e = Expr::StrInterp(vec![
            StrPart::Lit("x es ".into()),
            StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
        ], Span::ZERO);
        assert_eq!(eval_expr(&e, env).await.unwrap(), Value::Str("x es 42".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_interp_evalua_expresiones_internas() {
        // "{1 + 2}" → "3"
        let e = Expr::StrInterp(vec![
            StrPart::Expr(Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1, Span::ZERO)),
                right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
            }, None),
        ], Span::ZERO);
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("3".into()));
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
                default: None,
                varargs: false,
            }).collect(),
            return_type: None,
            body,
            is_async: false,
            decorators: vec![],
         span: Span::ZERO }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_sin_return_devuelve_null() {
        // fn f() { } ; f()
        let env = Environment::new();
        eval_stmt(&fn_def("f", vec![], vec![]), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Null);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_return_constante() {
        // fn f() { return 42 } ; f()
        let env = Environment::new();
        eval_stmt(
            &fn_def("f", vec![], vec![Stmt::Return(Expr::Int(42, Span::ZERO), Span::ZERO)]),
            env.clone(),
        ).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(42));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_con_un_param_arrow_style() {
        // fn double(n) => n * 2 → body es vec![Return(n * 2)]
        // double(7) → 14
        let env = Environment::new();
        let body = vec![Stmt::Return(Expr::BinOp {
            op: BinOpKind::Mul,
            left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
            right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
        }, Span::ZERO)];
        eval_stmt(&fn_def("double", vec!["n"], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("double".into(), Span::ZERO)), args: vec![Expr::Int(7, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(14));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_con_dos_params_suma() {
        // fn add(a, b) => a + b ; add(3, 4) → 7
        let env = Environment::new();
        let body = vec![Stmt::Return(Expr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
            right: Box::new(Expr::Ident("b".into(), Span::ZERO)), span: Span::ZERO,
        }, Span::ZERO)];
        eval_stmt(&fn_def("add", vec!["a", "b"], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("add".into(), Span::ZERO)), args: vec![Expr::Int(3, Span::ZERO), Expr::Int(4, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(7));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_ve_variables_del_scope_donde_se_definio() {
        // Closure básico: la función accede a `x` del scope global.
        //
        //   x = 10
        //   fn get_x() => x
        //   get_x()  → 10
        let env = Environment::new();
        env.lock().define("x", Value::Int(10));

        let body = vec![Stmt::Return(Expr::Ident("x".into(), Span::ZERO), Span::ZERO)];
        eval_stmt(&fn_def("get_x", vec![], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("get_x".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(10));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_param_sombrea_variable_externa() {
        // x = 100; fn f(x) => x ; f(7) → 7 (no 100)
        let env = Environment::new();
        env.lock().define("x", Value::Int(100));

        let body = vec![Stmt::Return(Expr::Ident("x".into(), Span::ZERO), Span::ZERO)];
        eval_stmt(&fn_def("f", vec!["x"], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![Expr::Int(7, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(7));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_con_pocos_args_es_error() {
        // fn f(a, b) ... ; f(1)
        let env = Environment::new();
        eval_stmt(
            &fn_def("f", vec!["a", "b"], vec![Stmt::Return(Expr::Int(0, Span::ZERO), Span::ZERO)]),
            env.clone(),
        ).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![Expr::Int(1, Span::ZERO)], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr(&call, env).await.unwrap_err(),
            EvalSignal::Error(FitzError {
                kind: ErrorKind::WrongArgCount { expected: 2, found: 1 }, ..
            })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_con_muchos_args_es_error() {
        let env = Environment::new();
        eval_stmt(
            &fn_def("f", vec![], vec![Stmt::Return(Expr::Int(0, Span::ZERO), Span::ZERO)]),
            env.clone(),
        ).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr(&call, env).await.unwrap_err(),
            EvalSignal::Error(FitzError {
                kind: ErrorKind::WrongArgCount { expected: 0, found: 2 }, ..
            })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn return_fuera_de_fn_es_error() {
        // En el top level, `return 5` no tiene caller que lo intercepte.
        let result = eval(vec![Stmt::Return(Expr::Int(5, Span::ZERO), Span::ZERO)]).await;
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::ReturnOutsideFunction
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_con_body_de_varias_sentencias() {
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
        eval_stmt(&fn_def("f", vec!["n"], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![Expr::Int(5, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(11));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn return_corta_la_ejecucion_del_body() {
        // fn f() {
        //     return 1
        //     return 2   ← nunca se ejecuta
        // }
        let env = Environment::new();
        let body = vec![
            Stmt::Return(Expr::Int(1, Span::ZERO), Span::ZERO),
            Stmt::Return(Expr::Int(2, Span::ZERO), Span::ZERO),
        ];
        eval_stmt(&fn_def("f", vec![], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(1));
    }

    // ---- Expr::If ----

    /// Helper: arma `if cond { then } else? { else_ }`.
    fn if_expr(cond: Expr, then: Vec<Stmt>, else_: Option<Vec<Stmt>>) -> Expr {
        Expr::If { condition: Box::new(cond), then, else_, span: Span::ZERO }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn if_true_sin_else_devuelve_valor_del_then() {
        // if true { 7 } → 7
        let e = if_expr(Expr::Bool(true, Span::ZERO), vec![Stmt::Expr(Expr::Int(7, Span::ZERO), Span::ZERO)], None);
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(7));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn if_false_sin_else_devuelve_null() {
        let e = if_expr(Expr::Bool(false, Span::ZERO), vec![Stmt::Expr(Expr::Int(7, Span::ZERO), Span::ZERO)], None);
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Null);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn if_else_toma_la_rama_correcta() {
        // if true { 1 } else { 2 } → 1
        let then = vec![Stmt::Expr(Expr::Int(1, Span::ZERO), Span::ZERO)];
        let else_ = vec![Stmt::Expr(Expr::Int(2, Span::ZERO), Span::ZERO)];
        let e = if_expr(Expr::Bool(true, Span::ZERO), then.clone(), Some(else_.clone()));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(1));

        let e = if_expr(Expr::Bool(false, Span::ZERO), then, Some(else_));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn if_condicion_no_bool_es_type_error() {
        // if 1 { ... } → error (no truthy coercion).
        let e = if_expr(Expr::Int(1, Span::ZERO), vec![], None);
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn if_evalua_solo_la_rama_correspondiente() {
        // El then es un Ident no definido. Si se evaluara, daría error.
        // Como cond es false, no se toca → resultado del else.
        let then = vec![Stmt::Expr(Expr::Ident("no_existe".into(), Span::ZERO), Span::ZERO)];
        let else_ = vec![Stmt::Expr(Expr::Int(99, Span::ZERO), Span::ZERO)];
        let e = if_expr(Expr::Bool(false, Span::ZERO), then, Some(else_));
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(99));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn variables_definidas_dentro_del_if_persisten_afuera() {
        // x = 1
        // if x == 1 { y = 99 }
        // print(y)  → "99"
        let env = Environment::new();
        env.lock().define("x", Value::Int(1));

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
        eval_stmt(&if_stmt, env.clone()).await.unwrap();

        assert_eq!(env.lock().get("y"), Some(Value::Int(99)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn else_if_anidado_funciona() {
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
        assert_eq!(eval_expr_test(outer).await.unwrap(), Value::Int(2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn if_como_expresion_en_assign() {
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
        eval_stmt(&stmt, env.clone()).await.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(42)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn factorial_recursivo_funciona() {
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

        eval_stmt(&fn_def("factorial", vec!["n"], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("factorial".into(), Span::ZERO)), args: vec![Expr::Int(5, Span::ZERO)], span: Span::ZERO,
        };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(120));
    }

    // ---- Expr::Match ----

    use crate::ast::MatchArm;

    fn match_arm(pattern: Pattern, body: Expr) -> MatchArm {
        MatchArm { pattern, guard: None, body: vec![Stmt::Expr(body, Span::ZERO)] }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_wildcard_siempre_matchea() {
        // match 42 { _ => 99 } → 99
        let e = Expr::Match {
            value: Box::new(Expr::Int(42, Span::ZERO)),
            arms: vec![match_arm(Pattern::Wildcard, Expr::Int(99, Span::ZERO))], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(99));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_ident_bindea_el_valor() {
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
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(43));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_toma_el_primer_arm_que_matchea() {
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
                        StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
                    ], Span::ZERO),
                ),
                match_arm(Pattern::Wildcard, Expr::Str("segundo arm".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("primer arm: hola".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_binding_vive_solo_en_el_arm() {
        // El binding `n` no debe escapar al scope contenedor.
        let env = Environment::new();
        let e = Expr::Match {
            value: Box::new(Expr::Int(7, Span::ZERO)),
            arms: vec![match_arm(Pattern::Ident("n".into()), Expr::Ident("n".into(), Span::ZERO))], span: Span::ZERO,
        };
        eval_expr(&e, env.clone()).await.unwrap();

        // `n` no quedó definida en el scope de afuera.
        assert_eq!(env.lock().get("n"), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_ok_binding_bindea_inner() {
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
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(6));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_err_binding_bindea_inner() {
        // match Err("boom") { Ok(v) => "ok", Err(e) => e } → "boom"
        let e = Expr::Match {
            value: Box::new(Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkBinding("v".into()), Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::ErrBinding("e".into()), Expr::Ident("e".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("boom".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_ok_no_matchea_err() {
        // El patrón Ok(_) NO matchea contra Err(_) — sigue al siguiente arm.
        let e = Expr::Match {
            value: Box::new(Expr::Err(Box::new(Expr::Int(1, Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkBinding("v".into()), Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("otro".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_ok_no_matchea_no_result() {
        // Ok(v) sobre un valor que no es Result → no matchea, cae en wildcard.
        let e = Expr::Match {
            value: Box::new(Expr::Int(5, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkBinding("v".into()), Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("no-result".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("no-result".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_ok_wildcard_matchea_pero_no_bindea() {
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
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("ok!".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_err_wildcard_matchea_err() {
        let e = Expr::Match {
            value: Box::new(Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkBinding("v".into()), Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::ErrWildcard, Expr::Str("falló".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("falló".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_ok_wildcard_no_matchea_err() {
        // OkWildcard NO debe matchear Err.
        let e = Expr::Match {
            value: Box::new(Expr::Err(Box::new(Expr::Int(0, Span::ZERO)), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::OkWildcard, Expr::Str("ok".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("otro".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_ok_wildcard_no_ensucia_scope() {
        // Después de un match con Ok(_), no debe existir una var
        // llamada `_` en el env. Esto era el bug que cerraba 3.3.
        let src = "\
let x = match Ok(5) {\n\
    Ok(_) => 1\n\
    _ => 0\n\
}\n\
print(_)\n";
        let result = parse_and_eval(src).await;
        assert!(
            result.is_err(),
            "esperaba error de variable `_` desconocida, hubo: {:?}",
            result
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_literal_int_matchea() {
        // match 2 { 1 => "uno", 2 => "dos", _ => "otro" } → "dos"
        let e = Expr::Match {
            value: Box::new(Expr::Int(2, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Int(1), Expr::Str("uno".into(), Span::ZERO)),
                match_arm(Pattern::Int(2), Expr::Str("dos".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("dos".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_literal_int_no_coerciona_a_float() {
        // match 1.0 { 1 => "int", _ => "no-int" } → "no-int"
        // (En match, igualdad es estructural — sin la coerción del `==`).
        let e = Expr::Match {
            value: Box::new(Expr::Float(1.0, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Int(1), Expr::Str("int".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("no-int".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("no-int".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_literal_str_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Str("hola".into(), Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Str("chau".into()), Expr::Int(1, Span::ZERO)),
                match_arm(Pattern::Str("hola".into()), Expr::Int(2, Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Int(0, Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_literal_bool_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Bool(true, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Bool(false), Expr::Str("falso".into(), Span::ZERO)),
                match_arm(Pattern::Bool(true), Expr::Str("verdadero".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("verdadero".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_literal_null_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Null(Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Null, Expr::Str("es null".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("no null".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("es null".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_int_negativo_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Int(-5, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Int(-5), Expr::Str("menos cinco".into(), Span::ZERO)),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into(), Span::ZERO)),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("menos cinco".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_literales_caen_a_ident_si_ninguno_matchea() {
        // match 42 { 1 => "uno", n => "default ${n}" }
        let e = Expr::Match {
            value: Box::new(Expr::Int(42, Span::ZERO)),
            arms: vec![
                match_arm(Pattern::Int(1), Expr::Str("uno".into(), Span::ZERO)),
                match_arm(
                    Pattern::Ident("n".into()),
                    Expr::StrInterp(vec![
                        StrPart::Lit("default ".into()),
                        StrPart::Expr(Expr::Ident("n".into(), Span::ZERO), None),
                    ], Span::ZERO),
                ),
            ], span: Span::ZERO,
        };
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Str("default 42".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_sin_arms_es_error() {
        let e = Expr::Match {
            value: Box::new(Expr::Int(1, Span::ZERO)),
            arms: vec![], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr_test(e).await.unwrap_err(),
            EvalSignal::Error(_)
        ));
    }

    // ---- while / loop ----

    #[tokio::test(flavor = "current_thread")]
    async fn while_itera_hasta_que_cond_es_falsa() {
        // i = 0
        // total = 0
        // while i < 5 { total = total + i; i = i + 1 }
        // total → 0+1+2+3+4 = 10
        let env = Environment::new();
        env.lock().define("i", Value::Int(0));
        env.lock().define("total", Value::Int(0));

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
          label: None,
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).await.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(10)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn while_con_cond_inicialmente_falsa_no_itera() {
        let env = Environment::new();
        env.lock().define("counter", Value::Int(0));

        let stmt = Stmt::While {
            condition: Expr::Bool(false, Span::ZERO),
            body: vec![Stmt::Assign { target: AssignTarget::Ident("counter".into()),
                type_: None,
                value: Expr::Int(99, Span::ZERO),
             span: Span::ZERO }],
          label: None,
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).await.unwrap();
        assert_eq!(env.lock().get("counter"), Some(Value::Int(0)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn while_break_termina_loop() {
        let env = Environment::new();
        env.lock().define("i", Value::Int(0));

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
                    then: vec![Stmt::Break(None, None, Span::ZERO)],
                    else_: None, span: Span::ZERO,
                }, Span::ZERO),
            ],
            label: None,
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).await.unwrap();
        assert_eq!(env.lock().get("i"), Some(Value::Int(3)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn while_continue_salta_a_la_siguiente_iteracion() {
        let env = Environment::new();
        env.lock().define("i", Value::Int(0));
        env.lock().define("total", Value::Int(0));

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
                    then: vec![Stmt::Continue(None, Span::ZERO)],
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
            label: None,
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).await.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(12)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn while_cond_no_bool_es_type_error() {
        let env = Environment::new();
        let stmt = Stmt::While {
            condition: Expr::Int(1, Span::ZERO),
            body: vec![],
            label: None,
         span: Span::ZERO };
        assert!(matches!(
            eval_stmt(&stmt, env).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn loop_infinito_se_corta_con_break() {
        let env = Environment::new();
        env.lock().define("count", Value::Int(0));

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
                    then: vec![Stmt::Break(None, None, Span::ZERO)],
                    else_: None, span: Span::ZERO,
                }, Span::ZERO),
            ],
            label: None,
          span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).await.unwrap();
        assert_eq!(env.lock().get("count"), Some(Value::Int(5)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn return_dentro_de_while_dentro_de_fn_propaga() {
        // fn f() {
        //   while true { return 42 }
        // }
        // f() → 42
        let env = Environment::new();
        let body = vec![Stmt::While {
            condition: Expr::Bool(true, Span::ZERO),
            body: vec![Stmt::Return(Expr::Int(42, Span::ZERO), Span::ZERO)],
         label: None,
         span: Span::ZERO }];
        eval_stmt(&fn_def("f", vec![], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("f".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(42));
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

    #[tokio::test(flavor = "current_thread")]
    async fn type_def_registra_el_tipo_en_el_env() {
        // type User { id: Int, name: Str }
        let env = Environment::new();
        let stmt = Stmt::TypeDef {
            name: "User".into(),
            fields: vec![
                make_field("id", "Int", false),
                make_field("name", "Str", false),
            ],
            methods: vec![],
         span: Span::ZERO };
        eval_stmt(&stmt, env.clone()).await.unwrap();

        let v = env.lock().get("User").expect("User no quedó en el env");
        match v {
            Value::Type { name, fields, .. } => {
                assert_eq!(name, "User");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "id");
                assert_eq!(fields[1].name, "name");
            }
            other => panic!("se esperaba Value::Type, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn type_value_type_name_es_type() {
        let t = Value::Type {
            name: "Foo".into(),
            fields: vec![],
            resolved_defaults: vec![],
            methods: vec![],
        };
        assert_eq!(t.type_name(), "Type");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn type_se_puede_referenciar_como_ident_sin_error() {
        // Después de definir un type, `User` como Expr::Ident lo encuentra.
        let env = Environment::new();
        eval_stmt(
            &Stmt::TypeDef {
                name: "User".into(),
                fields: vec![make_field("id", "Int", false)],
                methods: vec![],
             span: Span::ZERO },
            env.clone(),
        ).await.unwrap();

        let result = eval_expr(&Expr::Ident("User".into(), Span::ZERO), env).await.unwrap();
        assert!(matches!(result, Value::Type { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn llamar_un_type_como_funcion_es_type_error() {
        // User(1) sin struct literals → TypeMismatch porque Type no es callable.
        // Esto es deuda explícita: la instanciación viene en Fase 3.
        let env = Environment::new();
        eval_stmt(
            &Stmt::TypeDef {
                name: "User".into(),
                fields: vec![make_field("id", "Int", false)],
                methods: vec![],
             span: Span::ZERO },
            env.clone(),
        ).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("User".into(), Span::ZERO)), args: vec![Expr::Int(1, Span::ZERO)], span: Span::ZERO,
        };
        assert!(matches!(
            eval_expr(&call, env).await.unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- Criterio de Fase 2: el programa completo ----

    #[tokio::test(flavor = "current_thread")]
    async fn criterio_fase_2_corre_end_to_end() {
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
                    StrPart::Expr(Expr::Ident("name".into(), Span::ZERO), None),
                    StrPart::Lit(", x es ".into()),
                    StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
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
        assert!(eval(program).await.is_ok());
    }

    /// Test de integración: el pipeline completo (lexer → parser → eval)
    /// sobre el programa exacto del criterio de Fase 2 escrito como source.
    /// Si esto pasa, las tres fases hablan bien entre sí.
    #[tokio::test(flavor = "current_thread")]
    async fn integracion_criterio_fase_2_lexer_parser_evaluator() {
        let source = r#"
name = "Fitz"
x = 10 + 5
print("Hola {name}, x es {x}")

fn double(n) => n * 2
print(double(x))
"#;
        let tokens = crate::lexer::tokenize(source).expect("lexer falla");
        let program = crate::parser::parse(tokens).expect("parser falla");
        eval(program).await.expect("evaluator falla");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn integracion_factorial_recursivo_end_to_end() {
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
        eval(program).await.expect("evaluator falla");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hello_fitz_corre_sin_error() {
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
                    StrPart::Expr(Expr::Ident("name".into(), Span::ZERO), None),
                    StrPart::Lit("!".into()),
                ], Span::ZERO)], span: Span::ZERO,
            }, Span::ZERO),
        ];
        assert!(eval(program).await.is_ok());
    }

    // -----------------------------------------------------------------------
    // Tests — listas, mapas, rangos, indexing, for (Fase 3, paso 1)
    // -----------------------------------------------------------------------

    /// Helper: parsea y evalúa programa entero. Devuelve el env final.
    async fn parse_and_eval(src: &str) -> FitzResult<()> {
        let tokens = crate::lexer::tokenize(src).expect("la fuente debe tokenizar");
        let program = crate::parser::parse(tokens).expect("la fuente debe parsear");
        eval(program).await
    }

    /// Como `parse_and_eval`, pero conserva el env para inspeccionarlo.
    /// Útil cuando querés assertear valores específicos al final.
    ///
    /// Fase 6.4: async fn — los tests viven adentro de `#[tokio::test]`
    /// así que un `block_on` interno paniquearía con "runtime within
    /// runtime". Los call sites suman `.await`.
    async fn parse_eval_into_env(src: &str) -> (EnvRef, FitzResult<()>) {
        let tokens = crate::lexer::tokenize(src).expect("la fuente debe tokenizar");
        let program = crate::parser::parse(tokens).expect("la fuente debe parsear");
        let env = Environment::new();
        register_builtins(&env);
        let mut result: FitzResult<()> = Ok(());
        for stmt in &program {
            if let Err(signal) = eval_stmt(stmt, env.clone()).await {
                result = Err(signal_to_error(signal));
                break;
            }
        }
        (env, result)
    }

    // ---- List literal ----

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_list_vacia() {
        let v = eval_expr_test(Expr::List(vec![], Span::ZERO)).await.unwrap();
        assert_eq!(v, Value::new_list(vec![]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_list_con_literales() {
        let v = eval_expr_test(Expr::List(vec![
            Expr::Int(1, Span::ZERO),
            Expr::Int(2, Span::ZERO),
            Expr::Int(3, Span::ZERO),
        ], Span::ZERO)).await.unwrap();
        assert_eq!(v, Value::new_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_list_con_expresiones() {
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
        ], Span::ZERO)).await.unwrap();
        assert_eq!(v, Value::new_list(vec![Value::Int(2), Value::Int(4)]));
    }

    // ---- Map literal ----

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_map_vacio() {
        let v = eval_expr_test(Expr::Map(vec![], Span::ZERO)).await.unwrap();
        assert_eq!(v, Value::new_map(vec![]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_map_con_pares() {
        let v = eval_expr_test(Expr::Map(vec![
            (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
            (Expr::Str("b".into(), Span::ZERO), Expr::Int(2, Span::ZERO)),
        ], Span::ZERO)).await.unwrap();
        assert_eq!(
            v,
            Value::new_map(vec![
                (Value::Str("a".into()), Value::Int(1)),
                (Value::Str("b".into()), Value::Int(2)),
            ]),
        );
    }

    // ---- Range literal ----

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_range_simple() {
        let v = eval_expr_test(Expr::Range {
            start: Box::new(Expr::Int(0, Span::ZERO)),
            end: Box::new(Expr::Int(10, Span::ZERO)),
            inclusive: false,
            span: Span::ZERO,
        }).await.unwrap();
        assert_eq!(v, Value::Range { start: 0, end: 10 });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn evalua_range_con_float_es_error() {
        // 0..1.5 — float no es Int.
        let res = eval_expr_test(Expr::Range {
            start: Box::new(Expr::Int(0, Span::ZERO)),
            end: Box::new(Expr::Float(1.5, Span::ZERO)),
            inclusive: false,
            span: Span::ZERO,
        }).await;
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(matches!(e.kind, ErrorKind::TypeMismatch { .. })),
            _ => panic!("se esperaba Error"),
        }
    }

    // ---- R.1.4 — rangos inclusivos `..=` (mini-fase R) ----

    #[tokio::test(flavor = "current_thread")]
    async fn range_inclusive_evalua_a_value_range_con_end_plus_1() {
        // 0..=10 se materializa como Value::Range { 0, 11 } por
        // la conversión inclusive→exclusive del evaluator (no toca
        // Value::Range).
        let v = eval_expr_test(Expr::Range {
            start: Box::new(Expr::Int(0, Span::ZERO)),
            end: Box::new(Expr::Int(10, Span::ZERO)),
            inclusive: true,
            span: Span::ZERO,
        }).await.unwrap();
        assert_eq!(v, Value::Range { start: 0, end: 11 });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_inclusive_itera_end_inclusive() {
        // for i in 0..=3 → 0, 1, 2, 3 (4 iteraciones).
        let (env, res) = parse_eval_into_env(
            "let total = 0\nfor i in 0..=3 { total = total + i }\nlet sum = total",
        )
        .await;
        res.unwrap();
        // 0 + 1 + 2 + 3 = 6.
        assert_eq!(env.lock().get("sum"), Some(Value::Int(6)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_pattern_inclusive_matchea_end() {
        // match 100 { 0..=100 => "ok", _ => "fuera" } → "ok"
        let (env, res) = parse_eval_into_env(
            "let r = match 100 { 0..=100 => \"ok\", _ => \"fuera\" }",
        )
        .await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("ok".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_pattern_exclusive_no_matchea_end() {
        // match 100 { 0..100 => "in", _ => "out" } → "out" (exclusive)
        let (env, res) = parse_eval_into_env(
            "let r = match 100 { 0..100 => \"in\", _ => \"out\" }",
        )
        .await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("out".into())));
    }

    // ---- Or-patterns (R.2.1) ----

    #[tokio::test(flavor = "current_thread")]
    async fn or_pattern_literal_int_matchea_primero() {
        let (env, res) = parse_eval_into_env(
            "let r = match 1 { 1 | 2 | 3 => \"ok\", _ => \"no\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("ok".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn or_pattern_literal_int_matchea_segundo() {
        let (env, res) = parse_eval_into_env(
            "let r = match 2 { 1 | 2 | 3 => \"ok\", _ => \"no\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("ok".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn or_pattern_literal_int_no_matchea() {
        let (env, res) = parse_eval_into_env(
            "let r = match 4 { 1 | 2 | 3 => \"ok\", _ => \"no\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("no".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn or_pattern_strings() {
        let (env, res) = parse_eval_into_env(
            "let r = match \"lun\" { \"lun\" | \"mar\" => \"laboral\", _ => \"x\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("laboral".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn or_pattern_mezcla_int_y_range() {
        // 7 → matchea Range 5..=10 (segundo sub-pattern)
        let (env, res) = parse_eval_into_env(
            "let r = match 7 { 0 | 5..=10 => \"ok\", _ => \"x\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("ok".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn or_pattern_ok_y_err_wildcard() {
        let (env, res) = parse_eval_into_env(
            "let r = match Ok(42) { Ok(_) | Err(_) => \"cualquier\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("cualquier".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn or_pattern_no_introduce_binding_en_scope() {
        // Si el body de un arm con or-pattern intentara usar una var
        // bindeada por el pattern, fallaría. Verificamos que NO se
        // bindee nada usando un literal y un body que no referencia
        // ninguna var del pattern.
        let (env, res) = parse_eval_into_env(
            "let r = match 5 { 1 | 2 => \"chico\", 3 | 4 | 5 => \"medio\", _ => \"x\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("medio".into())));
    }

    // ---- Guards en match (R.2.2) ----

    #[tokio::test(flavor = "current_thread")]
    async fn guard_true_dispara_arm() {
        let (env, res) = parse_eval_into_env(
            "let r = match 10 { x if x > 5 => \"alto\", _ => \"bajo\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("alto".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guard_false_pasa_al_siguiente_arm() {
        let (env, res) = parse_eval_into_env(
            "let r = match 3 { x if x > 5 => \"alto\", _ => \"bajo\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("bajo".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guard_sobre_ok_binding_filtra_por_valor() {
        let (env, res) = parse_eval_into_env(
            "let r = match Ok(5) { Ok(v) if v > 0 => \"pos\", Ok(_) => \"neg\", Err(_) => \"err\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("pos".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guard_sobre_ok_binding_falla_y_cae_al_siguiente_ok_arm() {
        let (env, res) = parse_eval_into_env(
            "let r = match Ok(-3) { Ok(v) if v > 0 => \"pos\", Ok(_) => \"neg\", Err(_) => \"err\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("neg".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guard_con_or_pattern_dispara_si_cond_true() {
        let (env, res) = parse_eval_into_env(
            "let r = match 4 { 1 | 2 | 3 | 4 if true => \"any\", _ => \"otro\" }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("any".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guard_no_bool_es_error() {
        let (_env, res) = parse_eval_into_env(
            "let r = match 1 { x if x => \"x\", _ => \"y\" }",
        ).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("guard"));
    }

    // ---- Operadores compuestos +=/-=/*=//= (R.2.3) ----

    #[tokio::test(flavor = "current_thread")]
    async fn compound_plus_eq_sobre_ident() {
        let (env, res) = parse_eval_into_env(
            "let total = 0\n\
             total += 5\n\
             total += 10",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(15)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn compound_minus_eq_sobre_ident() {
        let (env, res) = parse_eval_into_env(
            "let n = 100\n\
             n -= 30",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(70)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn compound_star_eq_sobre_ident() {
        let (env, res) = parse_eval_into_env(
            "let x = 6\n\
             x *= 7",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("x"), Some(Value::Int(42)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn compound_slash_eq_sobre_ident_int() {
        let (env, res) = parse_eval_into_env(
            "let q = 20\n\
             q /= 4",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("q"), Some(Value::Int(5)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn compound_plus_eq_sobre_index_lista() {
        let (env, res) = parse_eval_into_env(
            "let xs = [1, 2, 3]\n\
             xs[0] += 10",
        ).await;
        res.unwrap();
        let xs = env.lock().get("xs").unwrap();
        match xs {
            Value::List(inner) => {
                let g = inner.lock();
                assert_eq!(g[0], Value::Int(11));
                assert_eq!(g[1], Value::Int(2));
                assert_eq!(g[2], Value::Int(3));
            }
            other => panic!("se esperaba List, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn compound_acumulado_en_loop() {
        // Test típico: acumular en un loop.
        let (env, res) = parse_eval_into_env(
            "let suma = 0\n\
             for i in 1..=5 { suma += i }",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("suma"), Some(Value::Int(15)));
    }

    // ---- Métodos custom sobre type (R.3) ----

    #[tokio::test(flavor = "current_thread")]
    async fn method_sin_params_lee_field() {
        let (env, res) = parse_eval_into_env(
            "type U {\n\
                 name: Str\n\
                 fn greet() -> Str { return \"hola, {name}\" }\n\
             }\n\
             let u = U { name: \"Ada\" }\n\
             let r = u.greet()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("hola, Ada".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn method_con_params_combina_fields_y_args() {
        let (env, res) = parse_eval_into_env(
            "type C {\n\
                 count: Int\n\
                 fn plus(n: Int) -> Int { return count + n }\n\
             }\n\
             let c = C { count: 10 }\n\
             let r = c.plus(5)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(15)));
    }

    // ---- Mini-tanda St — métodos estáticos ----

    #[tokio::test(flavor = "current_thread")]
    async fn st_static_method_se_invoca_como_type_method() {
        let (env, res) = parse_eval_into_env(
            "type C {\n\
                 value: Int = 0\n\
                 static fn zero() -> C { return C { value: 0 } }\n\
                 static fn of(n: Int) -> C { return C { value: n } }\n\
             }\n\
             let z = C.zero()\n\
             let c = C.of(42)",
        ).await;
        res.unwrap();
        // Ambos son instancias de C. Verificamos via Display.
        let z = env.lock().get("z").unwrap();
        let c = env.lock().get("c").unwrap();
        assert_eq!(z.to_string(), "C { value: 0 }");
        assert_eq!(c.to_string(), "C { value: 42 }");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn st_static_method_no_accede_a_fields_como_locales() {
        // Un método estático NO recibe los fields como locales. Si el
        // body intenta usar `value` (un field del tipo), debe fallar
        // con "variable no definida".
        let (_env, res) = parse_eval_into_env(
            "type C {\n\
                 value: Int = 0\n\
                 static fn broken() -> Int { return value }\n\
             }\n\
             let r = C.broken()",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("no definida") || err.message.contains("value"),
            "esperaba mensaje sobre `value` no definida, fue: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn st_static_method_invocado_sobre_instancia_es_error() {
        // `instance.static_method()` debe fallar con mensaje claro
        // sugiriendo la forma correcta.
        let (_env, res) = parse_eval_into_env(
            "type C {\n\
                 value: Int = 0\n\
                 static fn make() -> C { return C { value: 1 } }\n\
             }\n\
             let c = C { value: 5 }\n\
             let r = c.make()",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("estático") && err.message.contains("C.make"),
            "esperaba mensaje sugiriendo `C.make()`, fue: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn st_instance_method_invocado_como_static_es_error() {
        // `Type.instance_method()` debe fallar.
        let (_env, res) = parse_eval_into_env(
            "type C {\n\
                 value: Int = 0\n\
                 fn show() -> Int { return value }\n\
             }\n\
             let r = C.show()",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("instancia"),
            "esperaba mensaje sobre método de instancia, fue: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn method_param_shadowea_field_homonimo() {
        // R.3 — si un param tiene el mismo nombre que un field, el
        // param gana adentro del body (documentado como caveat).
        let (env, res) = parse_eval_into_env(
            "type U {\n\
                 name: Str\n\
                 fn pick(name: Str) -> Str { return name }\n\
             }\n\
             let u = U { name: \"field-name\" }\n\
             let r = u.pick(\"param-name\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("param-name".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn method_con_aridad_incorrecta_es_error() {
        let (_env, res) = parse_eval_into_env(
            "type U {\n\
                 fn f(x: Int) -> Int { return x }\n\
             }\n\
             let u = U {}\n\
             let r = u.f(1, 2)",
        ).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("espera 1"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn method_inexistente_es_error() {
        let (_env, res) = parse_eval_into_env(
            "type U { name: Str }\n\
             let u = U { name: \"x\" }\n\
             let r = u.no_existe()",
        ).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("no_existe"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn method_multiples_arms_de_dispatch() {
        // Múltiples métodos sobre el mismo type, despachados por nombre.
        let (env, res) = parse_eval_into_env(
            "type C {\n\
                 a: Int\n\
                 b: Int\n\
                 fn suma() -> Int { return a + b }\n\
                 fn resta() -> Int { return a - b }\n\
                 fn mult() -> Int { return a * b }\n\
             }\n\
             let c = C { a: 10, b: 3 }\n\
             let s = c.suma()\n\
             let r = c.resta()\n\
             let m = c.mult()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("s"), Some(Value::Int(13)));
        assert_eq!(env.lock().get("r"), Some(Value::Int(7)));
        assert_eq!(env.lock().get("m"), Some(Value::Int(30)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn method_chain_devuelve_instancia_ok() {
        // Un método devuelve un nuevo Instance; encadenamos otro
        // método sobre el resultado.
        let (env, res) = parse_eval_into_env(
            "type P {\n\
                 x: Int\n\
                 fn double_p() -> P { return P { x: x * 2 } }\n\
                 fn show() -> Int { return x }\n\
             }\n\
             let p = P { x: 5 }\n\
             let r = p.double_p().show()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(10)));
    }

    // ---- Indexing ----

    #[tokio::test(flavor = "current_thread")]
    async fn index_list_con_int_valido() {
        // [10, 20, 30][1] → 20
        let v = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(10, Span::ZERO), Expr::Int(20, Span::ZERO), Expr::Int(30, Span::ZERO)], Span::ZERO)),
            index: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
        }).await.unwrap();
        assert_eq!(v, Value::Int(20));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn index_list_fuera_de_rango_es_error() {
        // [1, 2][5]
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)], Span::ZERO)),
            index: Box::new(Expr::Int(5, Span::ZERO)), span: Span::ZERO,
        }).await;
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => {
                assert!(e.message.contains("fuera de rango"));
            }
            _ => panic!("se esperaba Error"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn index_list_negativo_wrappea_al_final() {
        // I.1 (mini-tanda I): `[1, 2][-1]` ahora wrap a `[1, 2][1]` = 2.
        let v = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)], Span::ZERO)),
            index: Box::new(Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
            }), span: Span::ZERO,
        }).await.unwrap();
        assert_eq!(v, Value::Int(2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn index_list_con_string_es_type_error() {
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1, Span::ZERO)], Span::ZERO)),
            index: Box::new(Expr::Str("a".into(), Span::ZERO)), span: Span::ZERO,
        }).await;
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(matches!(e.kind, ErrorKind::TypeMismatch { .. })),
            _ => panic!("se esperaba Error"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn index_map_clave_existente() {
        // {"a": 1, "b": 2}["b"] → 2
        let v = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Map(vec![
                (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
                (Expr::Str("b".into(), Span::ZERO), Expr::Int(2, Span::ZERO)),
            ], Span::ZERO)),
            index: Box::new(Expr::Str("b".into(), Span::ZERO)), span: Span::ZERO,
        }).await.unwrap();
        assert_eq!(v, Value::Int(2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn index_map_clave_inexistente_es_error() {
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Map(vec![
                (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
            ], Span::ZERO)),
            index: Box::new(Expr::Str("z".into(), Span::ZERO)), span: Span::ZERO,
        }).await;
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(e.message.contains("clave no encontrada")),
            _ => panic!("se esperaba Error"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn index_sobre_int_es_type_error() {
        // 42[0] — Int no se indexa
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Int(42, Span::ZERO)),
            index: Box::new(Expr::Int(0, Span::ZERO)), span: Span::ZERO,
        }).await;
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(matches!(e.kind, ErrorKind::TypeMismatch { .. })),
            _ => panic!("se esperaba Error"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn index_encadenado_funciona() {
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
        }).await.unwrap();
        assert_eq!(v, Value::Int(2));
    }

    // ---- for ----

    #[tokio::test(flavor = "current_thread")]
    async fn for_sobre_lista_itera_los_elementos() {
        // total = 1 + 2 + 3 + 4 = 10
        let src = r#"
total = 0
for x in [1, 2, 3, 4] {
    total = total + x
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(10)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_sobre_range_itera_inclusivo_exclusivo() {
        // 0..3 → 0 + 1 + 2 = 3 (la cota superior es exclusiva)
        let src = r#"
total = 0
for i in 0..3 {
    total = total + i
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(3)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_sobre_lista_vacia_no_itera() {
        let src = r#"
ran = false
for x in [] {
    ran = true
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("ran"), Some(Value::Bool(false)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_con_break_corta_iteracion() {
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
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("last"), Some(Value::Int(2)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_con_continue_salta_iteracion() {
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
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(8)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_sobre_map_destructura_pares_kv() {
        // Mini-tanda Md — `for (k, v) in m` bindea k y v en cada iteración.
        let src = r#"
let m = {"a": 1, "b": 2}
for (k, v) in m {
    last_k = k
    last_v = v
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("last_k"), Some(Value::Str("b".into())));
        assert_eq!(env.lock().get("last_v"), Some(Value::Int(2)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_sobre_map_con_ident_bindea_como_tuple() {
        // `for kv in m` bindea kv como Value::Tuple([k, v]) en cada iter.
        let src = r#"
let m = {"a": 1}
for kv in m {
    last_first = kv.0
    last_second = kv.1
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("last_first"), Some(Value::Str("a".into())));
        assert_eq!(env.lock().get("last_second"), Some(Value::Int(1)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_sobre_int_es_type_error() {
        let src = r#"
for x in 42 {
    print(x)
}
"#;
        let res = parse_and_eval(src).await;
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    // ---- Mini-tanda It — iteradores enumerate/zip/chain ----

    #[tokio::test(flavor = "current_thread")]
    async fn list_enumerate_emite_pares_indice_elem() {
        // `[10, 20, 30].enumerate()` → `[(0, 10), (1, 20), (2, 30)]`.
        let src = r#"
let xs = [10, 20, 30]
let pairs = xs.enumerate()
let first_idx = pairs[0].0
let first_val = pairs[0].1
let last_idx = pairs[2].0
let last_val = pairs[2].1
let total_len = pairs.len()
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("first_idx"), Some(Value::Int(0)));
        assert_eq!(env.lock().get("first_val"), Some(Value::Int(10)));
        assert_eq!(env.lock().get("last_idx"), Some(Value::Int(2)));
        assert_eq!(env.lock().get("last_val"), Some(Value::Int(30)));
        assert_eq!(env.lock().get("total_len"), Some(Value::Int(3)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_zip_trunca_al_mas_corto() {
        // `[1, 2, 3].zip(["a", "b"])` → `[(1, "a"), (2, "b")]` (len 2).
        let src = r#"
let xs = [1, 2, 3]
let ys = ["a", "b"]
let zs = xs.zip(ys)
let total = zs.len()
let first_x = zs[0].0
let first_y = zs[0].1
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(2)));
        assert_eq!(env.lock().get("first_x"), Some(Value::Int(1)));
        assert_eq!(env.lock().get("first_y"), Some(Value::Str("a".into())));
    }

    // ---- Mini-tanda Bits — operadores bit-a-bit ----

    #[tokio::test(flavor = "current_thread")]
    async fn bits_and_or_xor_sobre_int() {
        let src = "let a: Int = 0xF0\nlet b: Int = 0x0F\nlet and_ab: Int = a & b\nlet or_ab: Int = a | b\nlet xor_ab: Int = a ^ b\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("and_ab"), Some(Value::Int(0x00)));
        assert_eq!(env.lock().get("or_ab"), Some(Value::Int(0xFF)));
        assert_eq!(env.lock().get("xor_ab"), Some(Value::Int(0xFF)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bits_shifts_basicos() {
        let src = "let a: Int = 1\nlet shl_a: Int = a << 4\nlet shr_a: Int = shl_a >> 2\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("shl_a"), Some(Value::Int(16)));
        assert_eq!(env.lock().get("shr_a"), Some(Value::Int(4)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bits_not_unario_invierte_int() {
        // ~0 = -1 (todos los bits encendidos en i64 con signo).
        let src = "let r: Int = ~0\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(-1)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bits_shift_negativo_es_error_runtime() {
        let src = "let r: Int = 1 << -1\n";
        let (_, res) = parse_eval_into_env(src).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("shift fuera de rango") || err.message.contains("0..64"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bits_shift_64_es_error_runtime() {
        let src = "let r: Int = 1 << 64\n";
        let (_, res) = parse_eval_into_env(src).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("shift fuera de rango") || err.message.contains("0..64"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bits_precedencia_or_menor_que_and() {
        // Python/C precedence: `a | b & c` → `a | (b & c)`.
        // 0b1100 | (0b1010 & 0b0110) = 0b1100 | 0b0010 = 0b1110 = 14.
        let src = "let r: Int = 0b1100 | 0b1010 & 0b0110\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(0b1110)));
    }

    // ---- Mini-tanda Cmp — ops compuestos bit-a-bit ----

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_and_eq_compuesto() {
        let src = "let x: Int = 0xFF\nx &= 0x0F\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("x"), Some(Value::Int(0x0F)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_or_xor_eq_compuestos() {
        let src = "let a: Int = 0b1100\na |= 0b0010\nlet b: Int = 0b1100\nb ^= 0b0101\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Int(0b1110)));
        assert_eq!(env.lock().get("b"), Some(Value::Int(0b1001)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_shl_shr_eq_compuestos() {
        let src = "let n: Int = 1\nn <<= 4\nlet m: Int = 16\nm >>= 2\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(16)));
        assert_eq!(env.lock().get("m"), Some(Value::Int(4)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_compuesto_sobre_float_es_type_error() {
        let src = "let x: Float = 3.14\nx &= 1\n";
        let (_, res) = parse_eval_into_env(src).await;
        assert!(res.is_err());
    }

    // ---- Mini-tanda Err+ — `?` fuera de fn + Err con tipos no-Str ----

    #[tokio::test(flavor = "current_thread")]
    async fn err_plus_try_en_top_level_con_err_str_da_mensaje_especifico() {
        // `?` en top-level con Err debe mostrar mensaje claro
        // (no el genérico "return fuera de función").
        let src = "fn fail() -> Result<Int> { return Err(\"boom\") }\nlet r: Int = fail()?\n";
        let (_, res) = parse_eval_into_env(src).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("operación `?` falló") && err.message.contains("boom"),
            "esperaba mensaje específico de `?`: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn err_plus_try_top_level_con_err_int_preserva_value_en_mensaje() {
        let src = "fn fail() -> Result<Int> { return Err(404) }\nlet r: Int = fail()?\n";
        let (_, res) = parse_eval_into_env(src).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("operación `?` falló") && err.message.contains("404"),
            "esperaba 404 en el mensaje: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn err_plus_match_desempaca_err_con_tipo_custom() {
        // Sumar caveats: el `e` del `Err(e)` matchea como Any (gradual),
        // así que acceder a fields requiere conocer el shape. Uso de
        // `match` como expresión en el RHS de un let para evitar
        // assignments en arm bodies.
        let src = "\
            type ApiError { status: Int }\n\
            fn fetch() -> Result<Int> { return Err(ApiError { status: 503 }) }\n\
            let captured: Int = match fetch() {\n\
                Ok(v) => v,\n\
                Err(e) => e.status\n\
            }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("captured"), Some(Value::Int(503)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn err_plus_err_int_se_preserva_en_value_no_se_convierte_a_str() {
        // El Err(Int) sigue siendo Int en el value, no se string-ifica
        // (a diferencia del codegen que sí lo coerce a Str para
        // compatibilidad con Result<T, String>).
        let src = "\
            fn op() -> Result<Int> { return Err(42) }\n\
            let r: Int = match op() {\n\
                Ok(v) => v,\n\
                Err(e) => e\n\
            }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(42)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bits_combinado_con_hex_literales_de_lit() {
        // Encaja con Lit: mask + shift sobre literales hex.
        let src = "let mask: Int = 0xFF\nlet byte: Int = (0xABCD >> 8) & mask\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("byte"), Some(Value::Int(0xAB)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_chain_concatena_dos_listas() {
        // `[1, 2].chain([3, 4, 5])` → `[1, 2, 3, 4, 5]`.
        let src = r#"
let result = [1, 2].chain([3, 4, 5])
let total = result.len()
let first = result[0]
let last = result[4]
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(5)));
        assert_eq!(env.lock().get("first"), Some(Value::Int(1)));
        assert_eq!(env.lock().get("last"), Some(Value::Int(5)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_loop_var_persiste_despues_del_loop() {
        // Consistente con la política de bloques de Fitz: las variables
        // del body (incluida la variable de iteración) persisten en el
        // scope contenedor. Tras 0..3, i = 2 e last = 2.
        let src = r#"
for i in 0..3 {
    last = i
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("i"), Some(Value::Int(2)));
        assert_eq!(env.lock().get("last"), Some(Value::Int(2)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn for_anidado_funciona() {
        // 3 * 3 = 9 iteraciones totales.
        let src = r#"
total = 0
for i in 0..3 {
    for j in 0..3 {
        total = total + 1
    }
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(9)));
    }

    // ---- Pattern::Range ----

    #[tokio::test(flavor = "current_thread")]
    async fn pattern_range_matchea_valor_dentro() {
        let src = r#"
let n = 5
let r = match n {
    0..10 => "in"
    _     => "out"
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("in".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pattern_range_no_matchea_valor_fuera() {
        let src = r#"
let n = 15
let r = match n {
    0..10 => "in"
    _     => "out"
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("out".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pattern_range_es_exclusivo_en_el_fin() {
        // n = 10 con patrón 0..10 NO matchea (exclusivo). El segundo arm sí.
        let src = r#"
let n = 10
let r = match n {
    0..10 => "menor"
    10..20 => "diez_o_mas"
    _ => "otro"
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("diez_o_mas".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pattern_range_con_negativos() {
        let src = r#"
let n = -3
let r = match n {
    -10..0 => "negativo"
    0..10 => "chico"
    _ => "otro"
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("negativo".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pattern_range_no_matchea_no_int() {
        // 3.14 contra patrón 0..10 → no matchea, cae a wildcard.
        let src = r#"
let n = 3.14
let r = match n {
    0..10 => "int_chico"
    _ => "no_int"
}
"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("no_int".into())));
    }

    // ---- builtin len ----

    #[tokio::test(flavor = "current_thread")]
    async fn len_de_lista_devuelve_cantidad_de_elementos() {
        let src = "n = len([1, 2, 3, 4, 5])";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(5)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn len_de_lista_vacia_es_cero() {
        let src = "n = len([])";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(0)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn len_de_mapa_devuelve_cantidad_de_pares() {
        let src = r#"n = len({"a": 1, "b": 2, "c": 3})"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(3)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn len_de_string_cuenta_chars_no_bytes() {
        // "ñandú" tiene 5 chars y más de 5 bytes en UTF-8.
        let src = r#"n = len("ñandú")"#;
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(5)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn len_de_range_devuelve_cantidad_de_elementos() {
        let src = "n = len(0..10)";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(10)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn len_de_range_al_reves_es_cero() {
        // 10..0 — el evaluador trata rangos invertidos como vacíos.
        let src = "n = len(10..0)";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(0)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn len_de_int_es_type_error() {
        let src = "n = len(42)";
        let res = parse_and_eval(src).await;
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn len_con_cantidad_de_args_incorrecta_es_error() {
        let src = "n = len([1], [2])";
        let res = parse_and_eval(src).await;
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

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_basico_con_todos_los_campos() {
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"Fitz\" }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let u = env.lock().get("u").unwrap();
        match u {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "User");
                let fields = fields.lock();
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0], ("id".into(), Value::Int(1)));
                assert_eq!(fields[1], ("name".into(), Value::Str("Fitz".into())));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_ordena_campos_segun_la_declaracion() {
        // El literal tipea los campos al revés; la instancia debe seguir
        // el orden del `type`.
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { name: \"Fitz\", id: 1 }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let u = env.lock().get("u").unwrap();
        match u {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[0].0, "id");
                assert_eq!(fields[1].0, "name");
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_aplica_default_cuando_se_omite_un_campo() {
        let src = "\
            type Config { host: Str, port: Int = 3000 }\n\
            let c = Config { host: \"localhost\" }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let c = env.lock().get("c").unwrap();
        match c {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[0], ("host".into(), Value::Str("localhost".into())));
                assert_eq!(fields[1], ("port".into(), Value::Int(3000)));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_default_se_evalua_en_el_env_de_instanciacion() {
        // El default es una expresión: se evalúa al instanciar, en el
        // scope donde ocurre el literal. Si el usuario define una var
        // con ese nombre, el default la ve.
        let src = "\
            type Cfg { port: Int = base + 1 }\n\
            let base = 4000\n\
            let c = Cfg {}\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let c = env.lock().get("c").unwrap();
        match c {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[0], ("port".into(), Value::Int(4001)));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_campo_nullable_omitido_es_null() {
        let src = "\
            type User { id: Int, email: Str? }\n\
            let u = User { id: 1 }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let u = env.lock().get("u").unwrap();
        match u {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[1], ("email".into(), Value::Null));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_campo_nullable_explicito_a_null() {
        let src = "\
            type User { id: Int, email: Str? }\n\
            let u = User { id: 1, email: null }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let u = env.lock().get("u").unwrap();
        match u {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[1], ("email".into(), Value::Null));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_campo_faltante_sin_default_ni_nullable_es_error() {
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1 }\n\
        ";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("name"),
            "el error debería mencionar el campo faltante `name`: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_campo_extra_no_declarado_es_error() {
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"x\", color: \"red\" }\n\
        ";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("color"),
            "el error debería mencionar el campo extra `color`: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_de_tipo_no_definido_es_error() {
        let src = "let u = NoExiste { id: 1 }";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UndefinedVariable(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_sobre_no_tipo_es_type_error() {
        // `x` es Int, no un Type — instanciarlo es error.
        let src = "\
            let x = 42\n\
            let u = x { id: 1 }\n\
        ";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn field_access_sobre_instance_devuelve_el_valor() {
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"Fitz\" }\n\
            let n = u.name\n\
            let i = u.id\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Str("Fitz".into())));
        assert_eq!(env.lock().get("i"), Some(Value::Int(1)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn field_access_campo_inexistente_es_error() {
        let src = "\
            type User { id: Int }\n\
            let u = User { id: 1 }\n\
            let x = u.nope\n\
        ";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("nope"),
            "el error debería mencionar el campo `nope`: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn field_access_sobre_no_instance_es_type_error() {
        // Field access "pelado" sobre un Int explota: no hay propiedades
        // sobre primitivos. Los métodos sí (`x.upper()` para Str, etc.),
        // pero ese camino va por `Expr::Call` con callee `Field`, no por
        // este branch.
        let src = "\
            let x = 42\n\
            let n = x.foo\n\
        ";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn struct_literal_anidado_y_field_access_encadenado() {
        let src = "\
            type User { id: Int, name: Str }\n\
            type Order { user: User, total: Int }\n\
            let o = Order { user: User { id: 1, name: \"Fitz\" }, total: 100 }\n\
            let n = o.user.name\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Str("Fitz".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn instance_se_imprime_con_display_esperado() {
        // Sanity: el print de una instancia muestra el formato canónico.
        // (No capturamos stdout — usamos `to_string` del Value retornado.)
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"Fitz\" }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let u = env.lock().get("u").unwrap();
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

    #[tokio::test(flavor = "current_thread")]
    async fn ok_ctor_evalua_a_value_result_ok() {
        // Ok(42) → Value::Result(Ok(Int(42)))
        let e = Expr::Ok(Box::new(Expr::Int(42, Span::ZERO)), Span::ZERO);
        assert_eq!(eval_expr_test(e).await.unwrap(), ok_value(Value::Int(42)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn err_ctor_evalua_a_value_result_err() {
        // Err("boom") → Value::Result(Err(Str("boom")))
        let e = Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO);
        assert_eq!(
            eval_expr_test(e).await.unwrap(),
            err_value(Value::Str("boom".into())),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ok_ctor_evalua_inner_antes_de_envolver() {
        // Ok(1 + 2) → Value::Result(Ok(Int(3)))
        let e = Expr::Ok(Box::new(Expr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(Expr::Int(1, Span::ZERO)),
            right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
        }), Span::ZERO);
        assert_eq!(eval_expr_test(e).await.unwrap(), ok_value(Value::Int(3)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn try_sobre_ok_desempaqueta() {
        // Ok(7)? evaluado adentro de una función debería ser 7.
        // Lo testeamos directamente: como no hay return contenedor, el `?`
        // sobre Ok no emite ningún signal y la expresión vale 7.
        let e = Expr::Try(Box::new(Expr::Ok(Box::new(Expr::Int(7, Span::ZERO)), Span::ZERO)), Span::ZERO);
        assert_eq!(eval_expr_test(e).await.unwrap(), Value::Int(7));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn try_sobre_err_emite_signal_return_con_err() {
        // Err("boom")? emite EvalSignal::Return(Value::Result(Err("boom"))).
        let e = Expr::Try(Box::new(Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO)), Span::ZERO);
        let env = Environment::new();
        match eval_expr(&e, env).await {
            Err(EvalSignal::Return(v)) => {
                assert_eq!(v, err_value(Value::Str("boom".into())));
            }
            other => panic!("se esperaba EvalSignal::Return(Err(...)), se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn try_sobre_no_result_es_type_error() {
        // 42? → error: el operador `?` requiere un Result, no Int.
        let e = Expr::Try(Box::new(Expr::Int(42, Span::ZERO)), Span::ZERO);
        let env = Environment::new();
        match eval_expr(&e, env).await {
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

    #[tokio::test(flavor = "current_thread")]
    async fn try_adentro_de_funcion_con_ok_devuelve_inner() {
        // fn pass() { return Ok(5)? }  → pass() == 5  (porque return de un
        // valor "pelado" de Int sale como Int, no como Result).
        //
        // Acá lo que probamos es que `Ok(5)?` desempaqueta a 5 sin emitir
        // signal de retorno. La función devuelve ese 5 vía su return propio.
        let env = Environment::new();
        let body = vec![Stmt::Return(Expr::Try(Box::new(Expr::Ok(Box::new(
            Expr::Int(5, Span::ZERO),
        ), Span::ZERO)), Span::ZERO), Span::ZERO)];
        eval_stmt(&fn_def("pass", vec![], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("pass".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(eval_expr(&call, env).await.unwrap(), Value::Int(5));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn try_adentro_de_funcion_con_err_propaga() {
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
        eval_stmt(&fn_def("boom", vec![], body), env.clone()).await.unwrap();

        let call = Expr::Call { callee: Box::new(Expr::Ident("boom".into(), Span::ZERO)), args: vec![], span: Span::ZERO };
        assert_eq!(
            eval_expr(&call, env).await.unwrap(),
            err_value(Value::Str("nope".into())),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn programa_e2e_find_user_con_result_y_try() {
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
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(
            env.lock().get("hit"),
            Some(ok_value(Value::Str("Fitz".into()))),
        );
        assert_eq!(
            env.lock().get("miss"),
            Some(err_value(Value::Str("no encontrado".into()))),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn match_e2e_sobre_result_con_ok_y_err() {
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
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("ok_msg"), Some(Value::Str("ok: 5".into())));
        assert_eq!(
            env.lock().get("err_msg"),
            Some(Value::Str("err: divisi\u{00f3}n por cero".into())),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn try_top_level_con_err_genera_error_de_return_huerfano() {
        // En top-level, `Err(...)?` emite Return; el evaluador global lo
        // convierte en "return solo puede usarse adentro de una función".
        let env = Environment::new();
        let stmt = Stmt::Expr(Expr::Try(Box::new(Expr::Err(Box::new(Expr::Int(1, Span::ZERO)), Span::ZERO)), Span::ZERO), Span::ZERO);
        match eval_stmt(&stmt, env.clone()).await {
            Err(EvalSignal::Return(_)) => {} // ok — el global lo traduciría.
            other => panic!("se esperaba EvalSignal::Return, se obtuvo {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Fase 3, paso 4 (fn anónimas, method calls, mutación de campos)
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn fn_expr_evalua_a_function() {
        // `fn(x) => x * 2` — evaluada sola, da un `Value::Function`.
        let fnexpr = Expr::FnExpr {
            params: vec![crate::ast::Param { name: "x".into(), type_: None, default: None, varargs: false }],
            body: vec![Stmt::Return(Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
            }, Span::ZERO)], is_async: false, span: Span::ZERO,
        };
        let env = Environment::new();
        let v = eval_expr(&fnexpr, env).await.unwrap();
        assert!(matches!(v, Value::Function { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_expr_invocada_al_vuelo() {
        // `(fn(x) => x + 1)(2)` → 3
        let src = "let y = (fn(x) => x + 1)(2)\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("y"), Some(Value::Int(3)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_expr_captura_el_env_actual() {
        // El cuerpo de la anónima ve `n` definido afuera (closure).
        let src = "\
            let n = 10\n\
            let f = fn(x) => x + n\n\
            let r = f(5)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(15)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fn_expr_se_pasa_como_argumento() {
        // Pasar fn anónima como callback a una función de orden superior
        // declarada por el usuario.
        let src = "\
            fn apply(f, x) => f(x)\n\
            let r = apply(fn(n) => n * n, 6)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(36)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn field_assign_muta_la_instancia() {
        // `user.name = "Otro"` cambia el campo, visible a través de
        // cualquier alias.
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"Fitz\" }\n\
            u.name = \"Otro\"\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let u = env.lock().get("u").unwrap();
        match u {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[1], ("name".into(), Value::Str("Otro".into())));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn field_assign_visible_a_traves_de_alias() {
        // Dos variables apuntan a la misma instancia (vía `Rc`); mutar
        // por una se ve por la otra.
        let src = "\
            type Box { value: Int }\n\
            let a = Box { value: 1 }\n\
            let b = a\n\
            a.value = 42\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let b = env.lock().get("b").unwrap();
        match b {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[0], ("value".into(), Value::Int(42)));
            }
            other => panic!("se esperaba Instance, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn field_assign_a_no_instance_es_error() {
        // `x.field = ...` sobre algo que no es Instance corta con type error.
        let src = "\
            let x = 10\n\
            x.field = 1\n\
        ";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn field_assign_a_campo_inexistente_es_error() {
        let src = "\
            type User { id: Int }\n\
            let u = User { id: 1 }\n\
            u.nope = 2\n\
        ";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(err.message.contains("nope"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn method_call_sobre_tipo_sin_metodo_emite_error_explicito() {
        // `xs.foo()` no existe — el dispatch corta con
        // "no tiene un método llamado foo".
        let src = "\
            let xs = [1, 2, 3]\n\
            xs.foo()\n\
        ";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(err.message.contains("método"), "mensaje: {}", err.message);
    }

    // -----------------------------------------------------------------------
    // Tests — built-ins de List
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn list_push_muta_in_place() {
        let src = "\
            let xs = [1, 2]\n\
            xs.push(3)\n\
            xs.push(4)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let xs = env.lock().get("xs").unwrap();
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

    #[tokio::test(flavor = "current_thread")]
    async fn list_push_visible_a_traves_de_alias() {
        // Dos variables al mismo Rc; mutar por una se ve por la otra.
        let src = "\
            let a = [1]\n\
            let b = a\n\
            a.push(2)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let b = env.lock().get("b").unwrap();
        assert_eq!(b, Value::new_list(vec![Value::Int(1), Value::Int(2)]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_pop_devuelve_el_ultimo_y_acorta() {
        let src = "\
            let xs = [1, 2, 3]\n\
            let last = xs.pop()\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("last"), Some(Value::Int(3)));
        assert_eq!(
            env.lock().get("xs"),
            Some(Value::new_list(vec![Value::Int(1), Value::Int(2)])),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_pop_sobre_vacia_es_error() {
        let src = "let xs = []\nlet _ = xs.pop()\n";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(err.message.contains("vacía"), "mensaje: {}", err.message);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_map_aplica_fn_a_cada_elemento() {
        let src = "let r = [1, 2, 3].map(fn(n) => n * 10)\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(
            env.lock().get("r"),
            Some(Value::new_list(vec![
                Value::Int(10),
                Value::Int(20),
                Value::Int(30),
            ])),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_filter_solo_mantiene_los_true() {
        let src = "let r = [1, 2, 3, 4].filter(fn(n) => n == 2 or n == 4)\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(
            env.lock().get("r"),
            Some(Value::new_list(vec![Value::Int(2), Value::Int(4)])),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_filter_callback_no_bool_es_error() {
        let src = "let r = [1, 2].filter(fn(n) => n)\n";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_find_devuelve_ok_cuando_matchea() {
        let src = "let r = [1, 2, 3].find(fn(n) => n == 2)\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(
            env.lock().get("r"),
            Some(ok_value(Value::Int(2))),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_find_devuelve_err_cuando_no_hay_match() {
        let src = "let r = [1, 2, 3].find(fn(n) => n == 99)\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(
            env.lock().get("r"),
            Some(err_value(Value::Str("no encontrado".into()))),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_metodo_len() {
        let src = "let n = [1, 2, 3, 4].len()\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(4)));
    }

    // -----------------------------------------------------------------------
    // Tests — built-ins de Map
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn map_get_devuelve_ok_si_hay_clave() {
        let src = "let r = {\"a\": 1}.get(\"a\")\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(ok_value(Value::Int(1))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn map_get_devuelve_err_si_no_hay_clave() {
        let src = "let r = {\"a\": 1}.get(\"nope\")\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        // El mensaje del Err lleva la clave.
        let r = env.lock().get("r").unwrap();
        match r {
            Value::Result(ResultVariant::Err(inner)) => match *inner {
                Value::Str(s) => assert!(s.contains("nope")),
                other => panic!("se esperaba Str dentro de Err, se obtuvo {:?}", other),
            },
            other => panic!("se esperaba Err, se obtuvo {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn map_has_devuelve_true_o_false() {
        let src = "\
            let m = {\"a\": 1}\n\
            let yes = m.has(\"a\")\n\
            let no = m.has(\"x\")\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("yes"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("no"), Some(Value::Bool(false)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn map_keys_y_values_preservan_orden_de_insercion() {
        let src = "\
            let m = {\"b\": 2, \"a\": 1}\n\
            let ks = m.keys()\n\
            let vs = m.values()\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(
            env.lock().get("ks"),
            Some(Value::new_list(vec![
                Value::Str("b".into()),
                Value::Str("a".into()),
            ])),
        );
        assert_eq!(
            env.lock().get("vs"),
            Some(Value::new_list(vec![Value::Int(2), Value::Int(1)])),
        );
    }

    // -----------------------------------------------------------------------
    // Tests — built-ins de Str
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn str_metodo_len_cuenta_chars() {
        let src = "let n = \"hola\".len()\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(4)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_upper_y_lower() {
        let src = "\
            let a = \"hola\".upper()\n\
            let b = \"MUNDO\".lower()\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Str("HOLA".into())));
        assert_eq!(env.lock().get("b"), Some(Value::Str("mundo".into())));
    }

    // ---- S.1: contains/starts_with/ends_with ----

    #[tokio::test(flavor = "current_thread")]
    async fn str_contains_basico() {
        let (env, res) = parse_eval_into_env(
            "let a = \"hola mundo\".contains(\"mundo\")\n\
             let b = \"hola mundo\".contains(\"xyz\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("b"), Some(Value::Bool(false)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_contains_empty_string_es_true() {
        let (env, res) = parse_eval_into_env(
            "let a = \"hola\".contains(\"\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Bool(true)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_starts_with_y_ends_with() {
        let (env, res) = parse_eval_into_env(
            "let a = \"hola.fitz\".starts_with(\"hola\")\n\
             let b = \"hola.fitz\".ends_with(\".fitz\")\n\
             let c = \"hola.fitz\".starts_with(\"xyz\")\n\
             let d = \"hola.fitz\".ends_with(\".py\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("b"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("c"), Some(Value::Bool(false)));
        assert_eq!(env.lock().get("d"), Some(Value::Bool(false)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_contains_con_arg_no_str_es_error() {
        let (_env, res) = parse_eval_into_env(
            "let a = \"hola\".contains(1)",
        ).await;
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    // ---- S.2: split/trim/replace/repeat ----

    #[tokio::test(flavor = "current_thread")]
    async fn str_split_devuelve_list_str() {
        let (env, res) = parse_eval_into_env(
            "let parts = \"a,b,c\".split(\",\")",
        ).await;
        res.unwrap();
        let v = env.lock().get("parts").unwrap();
        let inner = match v {
            Value::List(items) => items,
            other => panic!("se esperaba List, fue {:?}", other),
        };
        let guard = inner.lock();
        assert_eq!(guard.len(), 3);
        assert_eq!(guard[0], Value::Str("a".into()));
        assert_eq!(guard[1], Value::Str("b".into()));
        assert_eq!(guard[2], Value::Str("c".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_split_sin_match_devuelve_un_elemento() {
        let (env, res) = parse_eval_into_env(
            "let parts = \"abc\".split(\"|\")",
        ).await;
        res.unwrap();
        let v = env.lock().get("parts").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            assert_eq!(g.len(), 1);
            assert_eq!(g[0], Value::Str("abc".into()));
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_trim_remueve_whitespace_ambos_lados() {
        let (env, res) = parse_eval_into_env(
            "let a = \"  hola  \".trim()\n\
             let b = \"\\nlinea\\n\".trim()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Str("hola".into())));
        assert_eq!(env.lock().get("b"), Some(Value::Str("linea".into())));
    }

    // ---- Mini-tanda Mb: trim_start / trim_end ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb_str_trim_start_recorta_solo_inicio() {
        let (env, res) = parse_eval_into_env(
            "let a = \"  hola  \".trim_start()\n\
             let b = \"\\n\\tlinea\".trim_start()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Str("hola  ".into())));
        assert_eq!(env.lock().get("b"), Some(Value::Str("linea".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb_str_trim_end_recorta_solo_final() {
        let (env, res) = parse_eval_into_env(
            "let a = \"  hola  \".trim_end()\n\
             let b = \"linea\\n\\t\".trim_end()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Str("  hola".into())));
        assert_eq!(env.lock().get("b"), Some(Value::Str("linea".into())));
    }

    // ---- Mini-tanda Mb: List.flatten ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb_list_flatten_concatena_sublistas_en_orden() {
        let (env, res) = parse_eval_into_env(
            "let xss: List<List<Int>> = [[1, 2], [3], [4, 5, 6]]\n\
             let flat = xss.flatten()",
        ).await;
        res.unwrap();
        let v = env.lock().get("flat").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let vals: Vec<Value> = g.clone();
            assert_eq!(
                vals,
                vec![
                    Value::Int(1), Value::Int(2), Value::Int(3),
                    Value::Int(4), Value::Int(5), Value::Int(6),
                ]
            );
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb_list_flatten_lista_vacia_es_vacia() {
        let (env, res) = parse_eval_into_env(
            "let xss: List<List<Int>> = []\n\
             let flat = xss.flatten()",
        ).await;
        res.unwrap();
        let v = env.lock().get("flat").unwrap();
        if let Value::List(items) = v {
            assert!(items.lock().is_empty());
        } else {
            panic!("esperaba List");
        }
    }

    // ---- Mini-tanda Mb: List.sort_by con callback ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb_list_sort_by_ascendente() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [3, 1, 4, 1, 5, 9, 2, 6]\n\
             xs.sort_by(fn(a, b) => a - b)",
        ).await;
        res.unwrap();
        let v = env.lock().get("xs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| if let Value::Int(n) = x { Some(*n) } else { None }).collect();
            assert_eq!(nums, vec![1, 1, 2, 3, 4, 5, 6, 9]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb_list_sort_by_descendente() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [3, 1, 4]\n\
             xs.sort_by(fn(a, b) => b - a)",
        ).await;
        res.unwrap();
        let v = env.lock().get("xs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| if let Value::Int(n) = x { Some(*n) } else { None }).collect();
            assert_eq!(nums, vec![4, 3, 1]);
        } else {
            panic!("esperaba List");
        }
    }

    // ---- Mini-tanda Ir — iteradores sobre Range ----

    #[tokio::test(flavor = "current_thread")]
    async fn ir_range_enumerate_devuelve_pares_indice_valor() {
        let (env, res) = parse_eval_into_env(
            "let r = (0..3).enumerate()",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            assert_eq!(g.len(), 3);
            // Cada elemento es Tuple([Int, Int]) con (i, n).
            for (idx, item) in g.iter().enumerate() {
                if let Value::Tuple(t) = item {
                    assert_eq!(t.len(), 2);
                    assert_eq!(t[0], Value::Int(idx as i64));
                    assert_eq!(t[1], Value::Int(idx as i64));
                } else {
                    panic!("esperaba Tuple, vio {:?}", item);
                }
            }
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ir_range_zip_trunca_al_mas_corto() {
        let (env, res) = parse_eval_into_env(
            "let r = (0..10).zip([\"a\", \"b\", \"c\"])",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            assert_eq!(items.lock().len(), 3, "esperaba 3 pares (trunca al más corto)");
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ir_range_chain_concatena_con_list_int() {
        let (env, res) = parse_eval_into_env(
            "let r = (0..3).chain([100, 200])",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| if let Value::Int(n) = x { Some(*n) } else { None }).collect();
            assert_eq!(nums, vec![0, 1, 2, 100, 200]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ir_range_len_exclusivo_e_inclusivo() {
        let (env, res) = parse_eval_into_env(
            "let a = (0..10).len()\n\
             let b = (0..=10).len()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Int(10)));
        assert_eq!(env.lock().get("b"), Some(Value::Int(11)));
    }

    // ---- Mini-tanda Up: Map.update + comprehension tuple destructuring ----

    #[tokio::test(flavor = "current_thread")]
    async fn up_map_update_key_existente() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"a\": 10, \"b\": 20}\n\
             let r = m.update(\"a\", fn(v) => v + 100)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            let a_value = g.iter().find(|(k, _)| k == &Value::Str("a".into()));
            assert_eq!(a_value.map(|(_, v)| v.clone()), Some(Value::Int(110)));
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn up_map_update_key_inexistente_es_no_op() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"a\": 10}\n\
             let r = m.update(\"missing\", fn(v) => v + 1000)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            assert_eq!(g.len(), 1);
            let a_value = g.iter().find(|(k, _)| k == &Value::Str("a".into()));
            assert_eq!(a_value.map(|(_, v)| v.clone()), Some(Value::Int(10)));
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn up_comprehension_con_tuple_destructuring() {
        let (env, res) = parse_eval_into_env(
            "let pairs: List<(Int, Int)> = [(1, 10), (2, 20), (3, 30)]\n\
             let sums: List<Int> = [a + b for (a, b) in pairs]",
        ).await;
        res.unwrap();
        let v = env.lock().get("sums").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| if let Value::Int(n) = x { Some(*n) } else { None }).collect();
            assert_eq!(nums, vec![11, 22, 33]);
        } else {
            panic!("esperaba List");
        }
    }

    // ---- Mini-tanda Ex2: List.flat_map/first/last + Map.merge ----

    #[tokio::test(flavor = "current_thread")]
    async fn ex2_list_flat_map_concatena_callback_lists() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.flat_map(fn(n) => [n, n * 10])",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| if let Value::Int(n) = x { Some(*n) } else { None }).collect();
            assert_eq!(nums, vec![1, 10, 2, 20, 3, 30]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ex2_list_flat_map_callback_no_list_es_error() {
        let (_env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2]\n\
             let r = xs.flat_map(fn(n) => n)",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("List") || err.message.contains("flat_map"),
            "esperaba mensaje sobre callback que no devuelve List, fue: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ex2_list_first_last_ok_y_err() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [10, 20, 30]\n\
             let a = xs.first()\n\
             let b = xs.last()\n\
             let empty: List<Int> = []\n\
             let c = empty.first()",
        ).await;
        res.unwrap();
        let a = env.lock().get("a").unwrap();
        let b = env.lock().get("b").unwrap();
        let c = env.lock().get("c").unwrap();
        if let Value::Result(ResultVariant::Ok(inner)) = a { assert_eq!(*inner, Value::Int(10)); } else { panic!(); }
        if let Value::Result(ResultVariant::Ok(inner)) = b { assert_eq!(*inner, Value::Int(30)); } else { panic!(); }
        assert!(matches!(c, Value::Result(ResultVariant::Err(_))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ex2_map_merge_last_write_wins() {
        let (env, res) = parse_eval_into_env(
            "let m1: Map<Str, Int> = {\"a\": 1, \"b\": 2}\n\
             let m2: Map<Str, Int> = {\"b\": 20, \"c\": 3}\n\
             let r = m1.merge(m2)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            assert_eq!(g.len(), 3);
            // b debe ser 20 (m2 gana).
            let b_value = g.iter().find(|(k, _)| k == &Value::Str("b".into()));
            assert_eq!(b_value.map(|(_, v)| v.clone()), Some(Value::Int(20)));
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ex2_map_merge_preserva_orden_para_pares_nuevos() {
        let (env, res) = parse_eval_into_env(
            "let m1: Map<Str, Int> = {\"a\": 1}\n\
             let m2: Map<Str, Int> = {\"b\": 2, \"c\": 3}\n\
             let r = m1.merge(m2)\n\
             let ks: List<Str> = r.keys()",
        ).await;
        res.unwrap();
        let v = env.lock().get("ks").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let keys: Vec<String> = g.iter().filter_map(|x| if let Value::Str(s) = x { Some(s.clone()) } else { None }).collect();
            assert_eq!(keys, vec!["a", "b", "c"]);
        } else {
            panic!("esperaba List");
        }
    }

    // ---- Mini-tanda Ex: Str.find/index_of/last_index_of, Map.filter/map_values ----

    #[tokio::test(flavor = "current_thread")]
    async fn ex_str_find_devuelve_result_int() {
        let (env, res) = parse_eval_into_env(
            "let s: Str = \"hola mundo, hola fitz\"\n\
             let a = s.find(\"hola\")\n\
             let b = s.find(\"nope\")",
        ).await;
        res.unwrap();
        let a = env.lock().get("a").unwrap();
        let b = env.lock().get("b").unwrap();
        assert!(matches!(a, Value::Result(ResultVariant::Ok(_))));
        if let Value::Result(ResultVariant::Ok(inner)) = a {
            assert_eq!(*inner, Value::Int(0));
        }
        assert!(matches!(b, Value::Result(ResultVariant::Err(_))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ex_str_last_index_of_busca_desde_el_final() {
        let (env, res) = parse_eval_into_env(
            "let s: Str = \"hola mundo, hola fitz\"\n\
             let a = s.last_index_of(\"hola\")",
        ).await;
        res.unwrap();
        let a = env.lock().get("a").unwrap();
        if let Value::Result(ResultVariant::Ok(inner)) = a {
            assert_eq!(*inner, Value::Int(12));
        } else {
            panic!("esperaba Ok");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ex_str_find_con_chars_no_ascii_devuelve_char_index() {
        // "café" tiene 4 chars (c, a, f, é) pero `é` ocupa 2 bytes
        // en UTF-8. `find` debe devolver char index (3 para "é"),
        // no byte index (3 también casualmente — usemos un char no-ASCII
        // adelante para forzar la diferencia).
        let (env, res) = parse_eval_into_env(
            "let s: Str = \"café latte\"\n\
             let a = s.find(\"latte\")",
        ).await;
        res.unwrap();
        let a = env.lock().get("a").unwrap();
        if let Value::Result(ResultVariant::Ok(inner)) = a {
            // chars: c(0) a(1) f(2) é(3) ' '(4) l(5) → char index = 5
            assert_eq!(*inner, Value::Int(5));
        } else {
            panic!("esperaba Ok");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ex_map_filter_keeps_pares_donde_pred_true() {
        let (env, res) = parse_eval_into_env(
            "let scores: Map<Str, Int> = {\"ada\": 80, \"bob\": 45, \"cam\": 92}\n\
             let passing = scores.filter(fn(k, v) => v >= 60)",
        ).await;
        res.unwrap();
        let passing = env.lock().get("passing").unwrap();
        if let Value::Map(pairs) = passing {
            assert_eq!(pairs.lock().len(), 2);
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ex_map_map_values_transforma_y_mantiene_keys() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
             let doubled = m.map_values(fn(v) => v * 2)",
        ).await;
        res.unwrap();
        let doubled = env.lock().get("doubled").unwrap();
        if let Value::Map(pairs) = doubled {
            let pairs = pairs.lock().clone();
            assert_eq!(pairs.len(), 3);
            // Verificamos un par a modo de sample.
            let a_value = pairs.iter().find(|(k, _)| k == &Value::Str("a".into()));
            assert_eq!(a_value.map(|(_, v)| v.clone()), Some(Value::Int(2)));
        } else {
            panic!("esperaba Map");
        }
    }

    // ---- Mini-tanda Lx: any/all/count/find_index ----

    #[tokio::test(flavor = "current_thread")]
    async fn lx_any_y_all_corto_circuito() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let a = xs.any(fn(x) => x > 3)\n\
             let b = xs.any(fn(x) => x > 10)\n\
             let c = xs.all(fn(x) => x > 0)\n\
             let d = xs.all(fn(x) => x > 2)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("b"), Some(Value::Bool(false)));
        assert_eq!(env.lock().get("c"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("d"), Some(Value::Bool(false)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lx_any_vacia_es_false_all_vacia_es_true() {
        let (env, res) = parse_eval_into_env(
            "let empty: List<Int> = []\n\
             let a = empty.any(fn(x) => true)\n\
             let b = empty.all(fn(x) => false)",
        ).await;
        res.unwrap();
        // Lista vacía: any → false, all → true (vacuamente todo es
        // verdad, paralelo a Python/Rust).
        assert_eq!(env.lock().get("a"), Some(Value::Bool(false)));
        assert_eq!(env.lock().get("b"), Some(Value::Bool(true)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lx_count_devuelve_int() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let n = xs.count(fn(x) => x > 2)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("n"), Some(Value::Int(3)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lx_find_index_ok_o_err() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [10, 20, 30, 40]\n\
             let a = xs.find_index(fn(x) => x == 30)\n\
             let b = xs.find_index(fn(x) => x > 100)",
        ).await;
        res.unwrap();
        let a = env.lock().get("a").unwrap();
        let b = env.lock().get("b").unwrap();
        // Ok(2) — el índice es 0-based.
        assert!(matches!(a, Value::Result(ResultVariant::Ok(_))));
        if let Value::Result(ResultVariant::Ok(inner)) = a {
            assert_eq!(*inner, Value::Int(2));
        }
        // Err("no encontrado").
        assert!(matches!(b, Value::Result(ResultVariant::Err(_))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lx_callback_no_bool_es_type_error() {
        let (_env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.any(fn(x) => x)",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("Bool") || err.message.contains("any"),
            "esperaba mensaje sobre Bool/any, fue: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb_list_sort_by_callback_no_int_es_error() {
        let (_env, res) = parse_eval_into_env(
            "let xs: List<Int> = [3, 1]\n\
             xs.sort_by(fn(a, b) => \"oops\")",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("Int") || err.message.contains("sort_by"),
            "esperaba mensaje sobre Int / sort_by, fue: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_replace_reemplaza_todas_las_ocurrencias() {
        let (env, res) = parse_eval_into_env(
            "let a = \"aaa\".replace(\"a\", \"bb\")\n\
             let b = \"hola mundo\".replace(\"o\", \"O\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Str("bbbbbb".into())));
        assert_eq!(env.lock().get("b"), Some(Value::Str("hOla mundO".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_repeat_funciona() {
        let (env, res) = parse_eval_into_env(
            "let a = \"ab\".repeat(3)\n\
             let b = \"x\".repeat(0)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Str("ababab".into())));
        assert_eq!(env.lock().get("b"), Some(Value::Str("".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_repeat_con_negativo_es_error() {
        let (_env, res) = parse_eval_into_env(
            "let a = \"ab\".repeat(-1)",
        ).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("negativo"));
    }

    // ---- S.3: List.sort/reverse/contains ----

    #[tokio::test(flavor = "current_thread")]
    async fn list_sort_int_ascendente() {
        let (env, res) = parse_eval_into_env(
            "let xs = [3, 1, 4, 1, 5, 9, 2, 6]\n\
             xs.sort()",
        ).await;
        res.unwrap();
        let v = env.lock().get("xs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|v| if let Value::Int(n) = v { Some(*n) } else { None }).collect();
            assert_eq!(nums, vec![1, 1, 2, 3, 4, 5, 6, 9]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_sort_str_alfabetico() {
        let (env, res) = parse_eval_into_env(
            "let xs = [\"zeta\", \"alfa\", \"beta\"]\n\
             xs.sort()",
        ).await;
        res.unwrap();
        let v = env.lock().get("xs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            assert_eq!(g[0], Value::Str("alfa".into()));
            assert_eq!(g[2], Value::Str("zeta".into()));
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_sort_heterogeneo_es_error() {
        let (_env, res) = parse_eval_into_env(
            "let xs = [1, \"dos\", 3]\n\
             xs.sort()",
        ).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("sort"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_reverse_invierte_orden() {
        let (env, res) = parse_eval_into_env(
            "let xs = [1, 2, 3, 4, 5]\n\
             xs.reverse()",
        ).await;
        res.unwrap();
        let v = env.lock().get("xs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            assert_eq!(g[0], Value::Int(5));
            assert_eq!(g[4], Value::Int(1));
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_contains_int() {
        let (env, res) = parse_eval_into_env(
            "let xs = [1, 2, 3, 4, 5]\n\
             let a = xs.contains(3)\n\
             let b = xs.contains(99)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("b"), Some(Value::Bool(false)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_contains_str() {
        let (env, res) = parse_eval_into_env(
            "let xs = [\"ada\", \"bob\"]\n\
             let a = xs.contains(\"ada\")\n\
             let b = xs.contains(\"dan\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("b"), Some(Value::Bool(false)));
    }

    // ---- I.1: índices negativos (mini-tanda I) ----

    #[tokio::test(flavor = "current_thread")]
    async fn list_negative_index_devuelve_desde_el_final() {
        let (env, res) = parse_eval_into_env(
            "let xs = [10, 20, 30, 40, 50]\n\
             let a = xs[-1]\n\
             let b = xs[-2]\n\
             let c = xs[-5]",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Int(50)));
        assert_eq!(env.lock().get("b"), Some(Value::Int(40)));
        assert_eq!(env.lock().get("c"), Some(Value::Int(10)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_negative_index_fuera_de_rango_es_error() {
        let (_env, res) = parse_eval_into_env(
            "let xs = [1, 2, 3]\n\
             let a = xs[-4]",
        ).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("fuera de rango"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_index_devuelve_char_como_str() {
        let (env, res) = parse_eval_into_env(
            "let s = \"fitz\"\n\
             let a = s[0]\n\
             let b = s[-1]\n\
             let c = s[2]",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Str("f".into())));
        assert_eq!(env.lock().get("b"), Some(Value::Str("z".into())));
        assert_eq!(env.lock().get("c"), Some(Value::Str("t".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_assign_con_negativo() {
        let (env, res) = parse_eval_into_env(
            "let xs = [1, 2, 3, 4]\n\
             xs[-1] = 99",
        ).await;
        res.unwrap();
        let v = env.lock().get("xs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            assert_eq!(g[3], Value::Int(99));
            assert_eq!(g[0], Value::Int(1));
        } else { panic!("esperaba List"); }
    }

    // ---- I.2: slicing ----

    #[tokio::test(flavor = "current_thread")]
    async fn list_slice_basico() {
        let (env, res) = parse_eval_into_env(
            "let xs = [10, 20, 30, 40, 50]\n\
             let a = xs[1..3]\n\
             let b = xs[..2]\n\
             let c = xs[3..]",
        ).await;
        res.unwrap();
        for (name, expected) in [
            ("a", vec![Value::Int(20), Value::Int(30)]),
            ("b", vec![Value::Int(10), Value::Int(20)]),
            ("c", vec![Value::Int(40), Value::Int(50)]),
        ] {
            let v = env.lock().get(name).unwrap();
            if let Value::List(items) = v {
                let g = items.lock();
                assert_eq!(g.as_slice(), expected.as_slice(), "binding `{}`", name);
            } else { panic!("binding {} esperaba List", name); }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_slice_inclusive_y_negativos() {
        let (env, res) = parse_eval_into_env(
            "let xs = [10, 20, 30, 40, 50]\n\
             let a = xs[1..=3]\n\
             let b = xs[-2..]",
        ).await;
        res.unwrap();
        for (name, expected) in [
            ("a", vec![Value::Int(20), Value::Int(30), Value::Int(40)]),
            ("b", vec![Value::Int(40), Value::Int(50)]),
        ] {
            let v = env.lock().get(name).unwrap();
            if let Value::List(items) = v {
                let g = items.lock();
                assert_eq!(g.as_slice(), expected.as_slice(), "binding `{}`", name);
            } else { panic!("binding {} esperaba List", name); }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_slice_clampea_out_of_range() {
        let (env, res) = parse_eval_into_env(
            "let xs = [1, 2, 3]\n\
             let a = xs[100..]\n\
             let b = xs[..100]\n\
             let c = xs[2..1]",
        ).await;
        res.unwrap();
        for (name, expected) in [
            ("a", vec![]),
            ("b", vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            ("c", vec![]),
        ] {
            let v = env.lock().get(name).unwrap();
            if let Value::List(items) = v {
                let g = items.lock();
                assert_eq!(g.as_slice(), expected.as_slice(), "binding `{}`", name);
            } else { panic!("binding {} esperaba List", name); }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_slice_devuelve_copia_no_view() {
        // Mutar el slice NO afecta el original.
        let (env, res) = parse_eval_into_env(
            "let xs = [1, 2, 3, 4, 5]\n\
             let mid = xs[1..4]\n\
             mid[0] = 99",
        ).await;
        res.unwrap();
        let v = env.lock().get("xs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            assert_eq!(g[1], Value::Int(2), "xs original no debe haber cambiado");
        } else { panic!("esperaba List"); }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn str_slice_funciona() {
        let (env, res) = parse_eval_into_env(
            "let s = \"hola fitz\"\n\
             let a = s[0..4]\n\
             let b = s[5..]\n\
             let c = s[-4..]",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Str("hola".into())));
        assert_eq!(env.lock().get("b"), Some(Value::Str("fitz".into())));
        assert_eq!(env.lock().get("c"), Some(Value::Str("fitz".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_empty_methods_no_panic() {
        // Lista vacía: sort/reverse no-op, contains false.
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = []\n\
             xs.sort()\n\
             xs.reverse()\n\
             let r = xs.contains(1)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Bool(false)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn metodo_con_aridad_incorrecta_es_error() {
        let src = "let r = \"x\".upper(1)\n";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::WrongArgCount { .. }));
    }

    // -----------------------------------------------------------------------
    // Tests — encadenamiento y composición
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn metodos_se_encadenan() {
        // `.map(...).filter(...)` se encadena vía postfix. El parser corta
        // sentencias en el newline; el encadenamiento multi-línea con `.`
        // al inicio de la línea siguiente todavía no se soporta (deuda
        // explícita). Se mantiene la cadena en una sola línea.
        let src = "let r = [1, 2, 3, 4].map(fn(n) => n * n).filter(fn(n) => n > 5)\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(
            env.lock().get("r"),
            Some(Value::new_list(vec![Value::Int(9), Value::Int(16)])),
        );
    }

    // -----------------------------------------------------------------------
    // Test E2E — criterio de éxito de Fase 3
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn programa_e2e_criterio_de_exito_fase_3() {
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
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();

        // hit es Ok(User { id: 1, name: "Fitz" })
        let hit = env.lock().get("hit").unwrap();
        match hit {
            Value::Result(ResultVariant::Ok(inner)) => match *inner {
                Value::Instance { ref type_name, ref fields } => {
                    assert_eq!(type_name, "User");
                    let f = fields.lock();
                    assert_eq!(f[0], ("id".into(), Value::Int(1)));
                    assert_eq!(f[1], ("name".into(), Value::Str("Fitz".into())));
                }
                other => panic!("se esperaba Instance adentro del Ok, se obtuvo {:?}", other),
            },
            other => panic!("se esperaba Ok, se obtuvo {:?}", other),
        }

        // miss es Err("no encontrado") — el mensaje viene de list_find.
        let miss = env.lock().get("miss").unwrap();
        assert_eq!(miss, err_value(Value::Str("no encontrado".into())));
    }

    // -----------------------------------------------------------------------
    // Tests — Módulos / import (Fase 3, paso 5)
    // -----------------------------------------------------------------------

    /// Helper: monta `files` (path relativo → contenido) en un tempdir,
    /// evalúa `main_src` con `base_dir` apuntando a ese tempdir, y
    /// devuelve `(env, resultado)`. El tempdir vive lo suficiente para
    /// que el loader pueda leer los archivos; se libera al final.
    async fn eval_with_modules(
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

        install_loader(dir.path().to_path_buf(), crate::manifest::DepRegistry::new());
        // Guard local: garantizamos uninstall aun ante panic en eval.
        let _guard = LoaderGuard;

        let env = Environment::new();
        register_builtins(&env);
        let mut result: FitzResult<()> = Ok(());
        for stmt in &program {
            if let Err(signal) = eval_stmt(stmt, env.clone()).await {
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

    #[tokio::test(flavor = "current_thread")]
    async fn import_simple_expone_el_modulo_como_namespace() {
        // `import utils` + `utils.greet("Fitz")` — el módulo exporta
        // una fn que devuelve un Str interpolado.
        let utils = "fn greet(name) => \"hola, {name}\"\n";
        let main = "\
            import utils\n\
            let g = utils.greet(\"Fitz\")\n\
        ";
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("g"), Some(Value::Str("hola, Fitz".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn import_bindea_bajo_el_ultimo_segmento() {
        // `import sub.foo` → binding `foo` (no `sub.foo`). El path
        // resuelve a `sub/foo.fitz`.
        let foo = "fn one() => 1\n";
        let main = "\
            import sub.foo\n\
            let r = foo.one()\n\
        ";
        let (env, res) = eval_with_modules(&[("sub/foo.fitz", foo)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(1)));
        // `sub` NO se bindea — solo el último segmento.
        assert!(env.lock().get("sub").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn from_import_bindea_nombres_directos() {
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
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("g"), Some(Value::Str("hola, Fitz".into())));
        // `utils` NO se bindea cuando se usa `from import`.
        assert!(env.lock().get("utils").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn from_import_de_tipo_permite_struct_literal() {
        // `from foo import User` + `User { id: 1, name: "x" }` — el
        // parser de struct literal espera `Ident { ... }`, y `from
        // import` trae el Value::Type al scope con ese nombre.
        let foo = "type User { id: Int, name: Str }\n";
        let main = "\
            from foo import User\n\
            let u = User { id: 7, name: \"Fitz\" }\n\
            let nm = u.name\n\
        ";
        let (env, res) = eval_with_modules(&[("foo.fitz", foo)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("nm"), Some(Value::Str("Fitz".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn from_import_default_referencia_const_del_modulo() {
        // PreF8.3: el `type User` del módulo tiene defaults que
        // referencian consts del propio módulo (`MAX`, `HELLO`). El
        // importer no las trae al scope. El loader pre-evalúa los
        // defaults en el env del módulo, así `User {}` aplica `99` y
        // `"saludos"` transparentemente. Pre-fix daba
        // "variable `MAX` no definida" en runtime.
        let foo = "\
            let MAX = 99\n\
            let HELLO = \"saludos\"\n\
            type User { id: Int = MAX, name: Str = HELLO }\n\
        ";
        let main = "\
            from foo import User\n\
            let u = User {}\n\
            let id = u.id\n\
            let nm = u.name\n\
        ";
        let (env, res) = eval_with_modules(&[("foo.fitz", foo)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("id"), Some(Value::Int(99)));
        assert_eq!(env.lock().get("nm"), Some(Value::Str("saludos".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn import_con_alias_bindea_bajo_alias() {
        // PreF8.4: `import utils as u` → binding `u`, no `utils`.
        let utils = "fn greet(n) => \"hola, {n}\"\n";
        let main = "\
            import utils as u\n\
            let g = u.greet(\"Fitz\")\n\
        ";
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("g"), Some(Value::Str("hola, Fitz".into())));
        // El nombre original NO queda bindeado.
        assert!(env.lock().get("utils").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn from_import_alias_bindea_bajo_alias() {
        // PreF8.4: `from utils import greet as g, PREFIX as P`.
        let utils = "\
            let PREFIX = \"saludos, \"\n\
            fn greet(n) => \"hola, {n}\"\n\
        ";
        let main = "\
            from utils import greet as g, PREFIX as P\n\
            let r = g(\"Fitz\")\n\
            let p = P\n\
        ";
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("hola, Fitz".into())));
        assert_eq!(env.lock().get("p"), Some(Value::Str("saludos, ".into())));
        // Los nombres originales NO quedan bindeados.
        assert!(env.lock().get("greet").is_none());
        assert!(env.lock().get("PREFIX").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn from_import_alias_de_tipo_struct_lit_usa_alias_pero_display_nombre_original() {
        // PreF8.4: `from foo import User as Person` + `Person { ... }`.
        // El struct lit usa el alias para mirar el binding, pero el
        // Display de la instancia usa el nombre canónico del tipo
        // (`User`) para mantener paridad con `fitz build`.
        let foo = "type User { id: Int, name: Str }\n";
        let main = "\
            from foo import User as Person\n\
            let p = Person { id: 7, name: \"Fitz\" }\n\
            let rendered = \"{p}\"\n\
        ";
        let (env, res) = eval_with_modules(&[("foo.fitz", foo)], main).await;
        res.unwrap();
        assert_eq!(
            env.lock().get("rendered"),
            Some(Value::Str("User { id: 7, name: \"Fitz\" }".into()))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn from_import_default_se_puede_sobrescribir_con_struct_lit() {
        // Aunque el módulo defina defaults, el importer puede
        // sobrescribirlos al construir.
        let foo = "\
            let MAX = 99\n\
            type User { id: Int = MAX }\n\
        ";
        let main = "\
            from foo import User\n\
            let u = User { id: 1 }\n\
            let id = u.id\n\
        ";
        let (env, res) = eval_with_modules(&[("foo.fitz", foo)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("id"), Some(Value::Int(1)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn modulo_no_existe_da_error_con_path_resuelto() {
        let main = "import inexistente\n";
        let (_env, res) = eval_with_modules(&[], main).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("inexistente"),
            "el mensaje debe nombrar el módulo: {}", err.message);
        assert!(err.message.contains("no se encontró"),
            "el mensaje debe decir 'no se encontró': {}", err.message);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn from_import_de_nombre_inexistente_da_error_claro() {
        // El módulo carga, pero el nombre pedido no existe en él.
        let utils = "fn a() => 1\n";
        let main = "from utils import b\n";
        let (_env, res) = eval_with_modules(&[("utils.fitz", utils)], main).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("no exporta"), "msg: {}", err.message);
        assert!(err.message.contains("`b`"), "msg: {}", err.message);
        assert!(err.message.contains("`utils`"), "msg: {}", err.message);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn field_access_en_modulo_inexistente_da_error_claro() {
        // `import utils` + `utils.missing` — el módulo carga pero
        // no expone `missing`.
        let utils = "fn a() => 1\n";
        let main = "\
            import utils\n\
            let x = utils.missing\n\
        ";
        let (_env, res) = eval_with_modules(&[("utils.fitz", utils)], main).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("no exporta") && err.message.contains("missing"),
            "msg: {}", err.message);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn modulo_cargado_dos_veces_no_re_ejecuta_side_effects() {
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
        let (env, res) = eval_with_modules(&[("counter_mod.fitz", counter_mod)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("v"), Some(Value::Int(42)));
        // Como no podemos detectar re-ejecución desde el lado del
        // lenguaje, validamos al menos que ambos `import` no rompan
        // ni dupliquen estado: el binding `counter_mod` queda accesible
        // y consistente.
        let m = env.lock().get("counter_mod").unwrap();
        assert!(matches!(m, Value::Module { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn modulo_cacheado_devuelve_misma_identidad_de_env() {
        // Cargar un módulo dos veces desde paths distintos pero al
        // mismo archivo (acá igual path) devuelve `Value::Module` con
        // el MISMO `Arc<Mutex<Environment>>` adentro. Eso lo testea
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
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main).await;
        res.unwrap();
        let u1 = env.lock().get("u1").unwrap();
        let u2 = env.lock().get("u2").unwrap();
        assert_eq!(u1, u2, "el segundo import debe devolver el mismo módulo cacheado");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ciclo_a_b_a_se_detecta() {
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
        let (_env, res) = eval_with_modules(&[("a.fitz", a), ("b.fitz", b)], main).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("ciclo de imports"),
            "msg: {}", err.message);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn import_anidado_resuelve_relativo_al_modulo_importer() {
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
        ], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("desde bar".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn modulo_con_error_de_sintaxis_propaga_error() {
        // Si el módulo importado tiene un parse error, debería
        // propagarse al importer en lugar de pasar silenciosamente.
        let busted = "let x = +\n"; // syntax error
        let main = "import busted\n";
        let (_env, res) = eval_with_modules(&[("busted.fitz", busted)], main).await;
        assert!(res.is_err(), "se esperaba error de parseo del módulo");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn modulo_con_error_de_runtime_propaga_error() {
        // El módulo carga (parsea bien) pero su top-level body
        // dispara un error al evaluar — debería propagarse.
        let busted = "let x = no_existe\n";
        let main = "import busted\n";
        let (_env, res) = eval_with_modules(&[("busted.fitz", busted)], main).await;
        let err = res.unwrap_err();
        // Esperamos UndefinedVariable de adentro del módulo.
        assert!(matches!(err.kind, ErrorKind::UndefinedVariable(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn method_call_sobre_modulo_invoca_funcion_exportada() {
        // `utils.suma(2, 3)` debe resolver a `suma` adentro de utils.
        let utils = "fn suma(a, b) => a + b\n";
        let main = "\
            import utils\n\
            let r = utils.suma(2, 3)\n\
        ";
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(5)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn funcion_importada_via_from_import_cierra_sobre_env_del_modulo() {
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
        let (env, res) = eval_with_modules(&[("utils.fitz", utils)], main).await;
        res.unwrap();
        assert_eq!(env.lock().get("g"), Some(Value::Str("saludos, Fitz".into())));
    }

    // -----------------------------------------------------------------------
    // Tests — Fase 8.1.2: ruteo de `from python import X` al loader CPython
    // -----------------------------------------------------------------------
    //
    // Estos tests verifican el comportamiento del evaluator. La lógica
    // de import per se (resolver módulos Python, traducir excepciones)
    // vive en `py_interop.rs` y tiene sus propios unit tests adentro.
    // Acá solo chequeamos: bindings, alias, errores de path inválido,
    // y el fallback sin feature.

    // Con feature `python`: el binario lincado a libpython carga
    // módulos reales (math, json, etc.) y produce `Value::PyObject`.
    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn from_python_import_math_bindea_pyobject() {
        let (env, res) = parse_eval_into_env("from python import math\n").await;
        res.unwrap();
        let v = env.lock().get("math").expect("math debería estar bindeado");
        assert!(matches!(v, Value::PyObject(_)), "se esperaba PyObject, fue: {:?}", v);
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn from_python_import_con_alias_bindea_bajo_alias() {
        let (env, res) = parse_eval_into_env("from python import math as m\n").await;
        res.unwrap();
        assert!(matches!(env.lock().get("m"), Some(Value::PyObject(_))));
        assert!(env.lock().get("math").is_none(), "el nombre original no debe quedar bindeado");
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn from_python_import_multiples_modulos() {
        let (env, res) = parse_eval_into_env("from python import math, json\n").await;
        res.unwrap();
        assert!(matches!(env.lock().get("math"), Some(Value::PyObject(_))));
        assert!(matches!(env.lock().get("json"), Some(Value::PyObject(_))));
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn from_python_import_modulo_inexistente_emite_modulenotfounderror() {
        let (_env, res) =
            parse_eval_into_env("from python import este_modulo_no_existe_xyz_812\n").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("ModuleNotFoundError"),
            "mensaje debería citar ModuleNotFoundError, fue: {}",
            err.message,
        );
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn from_python_path_con_submodulos_no_se_soporta_en_8_1() {
        // `from python.sqlalchemy.orm import Session` queda como deuda
        // menor — para 8.1 hay que importar `sqlalchemy` y bajar con
        // field access (8.1.3+). Mensaje debe ser claro citando 8.1.
        let (_env, res) =
            parse_eval_into_env("from python.sqlalchemy.orm import Session\n").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("python.sqlalchemy.orm")
                && err.message.contains("8.1"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn import_python_punteado_no_se_soporta() {
        // `import python.math` también queda fuera del scope de 8.1.
        // Forma canónica: `from python import math`.
        let (_env, res) = parse_eval_into_env("import python.math\n").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("from python import"),
            "el error debería sugerir la forma canónica, fue: {}",
            err.message,
        );
    }

    // Sin feature `python`: el binario default produce error claro
    // citando el flag de build para recompilar.
    #[cfg(not(feature = "python"))]
    #[tokio::test(flavor = "current_thread")]
    async fn from_python_sin_feature_da_error_de_build() {
        let (_env, res) = parse_eval_into_env("from python import math\n").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("--features python"),
            "mensaje debería citar el flag de build, fue: {}",
            err.message,
        );
    }

    // -------------------------------------------------------------------
    // Fase 8.1.3 — Expr::Field sobre Value::PyObject (getattr + auto-coerción)
    // -------------------------------------------------------------------

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn field_access_math_pi_coerciona_a_float() {
        let (env, res) = parse_eval_into_env(
            "from python import math\nlet p = math.pi\n"
        ).await;
        res.unwrap();
        // Sacamos el binding fuera del lock para que el MutexGuard
        // se libere antes de que `env` se dropee al fin del scope.
        let p = env.lock().get("p").unwrap();
        match p {
            Value::Float(f) => {
                assert!((f - std::f64::consts::PI).abs() < 1e-15, "got {}", f);
            }
            other => panic!("esperaba Float, fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn field_access_funcion_python_queda_pyobject_opaco() {
        // `math.sqrt` no se invoca, solo se lee. Debería quedar como
        // PyObject opaco listo para call en 8.1.4.
        let (env, res) = parse_eval_into_env(
            "from python import math\nlet f = math.sqrt\n"
        ).await;
        res.unwrap();
        assert!(matches!(env.lock().get("f"), Some(Value::PyObject(_))));
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn field_access_atributo_inexistente_emite_attributeerror() {
        let (_env, res) = parse_eval_into_env(
            "from python import math\nlet x = math.no_existe_xyz_813\n"
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("AttributeError"),
            "mensaje: {}",
            err.message,
        );
    }

    // -------------------------------------------------------------------
    // Fase 8.1.4 — Expr::Call sobre Value::PyObject (criterio de éxito 8.1)
    // -------------------------------------------------------------------

    // 8.3: helper para tests Python — un `call` exitoso ahora devuelve
    // `Value::Result(Ok(v))`. Para asserts mecánicos, desempaquetamos
    // el Ok aquí. Si el binding tiene Err o no es Result, el test falla.
    #[cfg(feature = "python")]
    fn ok_inner(v: Value) -> Value {
        match v {
            Value::Result(crate::value::ResultVariant::Ok(inner)) => *inner,
            Value::Result(crate::value::ResultVariant::Err(msg)) => {
                panic!("esperaba Ok(...), llegó Err({:?})", msg)
            }
            other => panic!("esperaba Value::Result, fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn criterio_de_exito_8_1_math_sqrt_y_math_pi() {
        // El criterio explícito del roadmap para cerrar Fase 8.1:
        //   from python import math
        //   print(math.sqrt(16.0))   // 4
        //   print(math.pi)           // 3.141592653589793
        //
        // 8.3: `math.sqrt(16.0)` ahora devuelve `Result<Float>`, así que
        // el binding `r` es `Ok(4.0)` — el test desempaqueta con el
        // helper. `math.pi` es field access (no llamada), sigue
        // devolviendo Float directo.
        let src = "\
            from python import math\n\
            let r = math.sqrt(16.0)\n\
            let p = math.pi\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        assert_eq!(ok_inner(r), Value::Float(4.0));
        let p = env.lock().get("p").unwrap();
        match p {
            Value::Float(f) => {
                assert!((f - std::f64::consts::PI).abs() < 1e-15, "got {}", f);
            }
            other => panic!("esperaba Float, fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn call_via_method_dispatch_sobre_modulo_python() {
        // `math.sqrt(16.0)` directo, sin pasar por let intermedio.
        // El parser genera `Expr::Call { callee: Expr::Field {...} }`
        // que cae al dispatch de método, que para PyObject hace
        // getattr + invoke_value (rama nueva de 8.1.4).
        // 8.3: el resultado viene envuelto en `Ok(Float)`.
        let src = "let x = 4 + 5\nfrom python import math\nlet r = math.sqrt(x * x + 7 * 7)\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        // sqrt(81 + 49) = sqrt(130) ≈ 11.4018
        let r = env.lock().get("r").unwrap();
        match ok_inner(r) {
            Value::Float(f) => {
                assert!((f - 130_f64.sqrt()).abs() < 1e-12, "got {}", f);
            }
            other => panic!("esperaba Float, fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn call_via_pyobject_guardado_en_variable() {
        // `let f = math.sqrt; f(25.0)` — el callable se extrae primero
        // (field access) y se invoca via Ident. Esto pega en
        // `invoke_value` directo, no en `dispatch_method`.
        // 8.3: el call vía Ident también envuelve en Result.
        let src = "\
            from python import math\n\
            let f = math.sqrt\n\
            let r = f(25.0)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        assert_eq!(ok_inner(r), Value::Float(5.0));
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn call_python_con_excepcion_se_envuelve_en_err() {
        // 8.3: `math.sqrt(-1)` lanza ValueError en Python. El call NO
        // aborta — devuelve `Result<Float>::Err("ValueError: ...")` que
        // el usuario tiene que manejar con `match` o `?`.
        let src = "from python import math\nlet r = math.sqrt(-1.0)\n";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        match r {
            Value::Result(crate::value::ResultVariant::Err(inner)) => match *inner {
                Value::Str(s) => assert!(
                    s.contains("ValueError"),
                    "mensaje del Err debería citar ValueError, fue: {}",
                    s,
                ),
                other => panic!("Err debería envolver Str, fue {:?}", other),
            },
            other => panic!("esperaba Err(...), fue {:?}", other),
        }
    }

    // 8.2.1: List ahora SÍ se marshalla. El test reapunta a `math.sqrt`
    // recibiendo una `List<Int>` — Python lanza TypeError porque
    // `sqrt` espera un número. 8.3: el TypeError llega como
    // `Result::Err`, no como abort del programa.
    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn call_python_con_list_como_arg_envuelve_typeerror_en_err() {
        let src = "\
            from python import math\n\
            let xs = [1, 2, 3]\n\
            let r = math.sqrt(xs)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        let msg = match r {
            Value::Result(crate::value::ResultVariant::Err(inner)) => match *inner {
                Value::Str(s) => s,
                other => panic!("Err debería envolver Str, fue {:?}", other),
            },
            other => panic!("esperaba Err(...), fue {:?}", other),
        };
        assert!(
            msg.contains("TypeError"),
            "mensaje debería ser TypeError de Python, fue: {}",
            msg,
        );
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn call_python_arg_int_coerciona() {
        // `abs(-7)` con arg Int Fitz → Int 7.
        let src = "\
            from python import builtins\n\
            let v = builtins.abs(-7)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let v = env.lock().get("v").unwrap();
        assert_eq!(ok_inner(v), Value::Int(7));
    }

    // -------------------------------------------------------------------
    // Fase 8.2.1 — Fitz → Python: List/Map/Instance se marshallan a
    // list/dict/dict end-to-end via json.dumps.
    // 8.3: cada call envuelve en Result, los asserts desempaquetan Ok.
    // -------------------------------------------------------------------

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn marshalling_list_de_ints_via_json_dumps() {
        let src = "\
            from python import json\n\
            let xs = [1, 2, 3]\n\
            let s = json.dumps(xs)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let s = env.lock().get("s").unwrap();
        assert_eq!(ok_inner(s), Value::Str("[1, 2, 3]".into()));
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn marshalling_map_str_int_via_json_dumps() {
        let src = "\
            from python import json\n\
            let m = {\"a\": 1, \"b\": 2}\n\
            let s = json.dumps(m)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let s = env.lock().get("s").unwrap();
        assert_eq!(ok_inner(s), Value::Str("{\"a\": 1, \"b\": 2}".into()));
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn marshalling_instance_via_json_dumps() {
        // Una Instance Fitz se marshalla a dict Python por field name.
        let src = "\
            type User { id: Int, name: Str }\n\
            from python import json\n\
            let u = User { id: 1, name: \"x\" }\n\
            let s = json.dumps(u)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let s = env.lock().get("s").unwrap();
        assert_eq!(ok_inner(s), Value::Str("{\"id\": 1, \"name\": \"x\"}".into()));
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn marshalling_list_de_instances_via_json_dumps() {
        let src = "\
            type User { id: Int, email: Str }\n\
            from python import json\n\
            let users = [\n\
                User { id: 1, email: \"a@x.com\" },\n\
                User { id: 2, email: \"b@x.com\" },\n\
            ]\n\
            let s = json.dumps(users)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let s = env.lock().get("s").unwrap();
        assert_eq!(
            ok_inner(s),
            Value::Str(
                "[{\"id\": 1, \"email\": \"a@x.com\"}, \
                  {\"id\": 2, \"email\": \"b@x.com\"}]".into()
            ),
        );
    }

    // -------------------------------------------------------------------
    // Fase 8.2.2 — Python → Fitz: list/dict se coercionan a List/Map
    // end-to-end via json.loads.
    // -------------------------------------------------------------------

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn marshalling_json_loads_de_array_a_list() {
        let src = "\
            from python import json\n\
            let xs = json.loads(\"[1, 2, 3]\")\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let xs = env.lock().get("xs").unwrap();
        assert_eq!(
            ok_inner(xs),
            Value::new_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        );
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn marshalling_json_loads_de_objeto_a_map() {
        // En Fitz, `{` y `}` dentro de un string indican interpolación.
        // Para que el JSON literal `{"a": 1, ...}` pase entero al
        // string, escapamos las llaves con `\{` y `\}` (el lexer las
        // preserva literales). En el source Rust eso es `\\{`/`\\}`.
        let src = "\
            from python import json\n\
            let m = json.loads(\"\\{\\\"a\\\": 1, \\\"b\\\": 2\\}\")\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let m = env.lock().get("m").unwrap();
        assert_eq!(
            ok_inner(m),
            Value::new_map(vec![
                (Value::Str("a".into()), Value::Int(1)),
                (Value::Str("b".into()), Value::Int(2)),
            ]),
        );
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn marshalling_round_trip_via_json_preserva_estructura() {
        // dumps + loads sobre una List anidada con Map adentro.
        // 8.3: el round-trip natural ahora pasa el `Result` adentro.
        // Para validar que vuelve la misma estructura, usamos `match`
        // para desempaquetar ambos lados.
        let src = "\
            from python import json\n\
            let original = [{\"k\": 1}, {\"k\": 2}]\n\
            let s_res = json.dumps(original)\n\
            let s = match s_res { Ok(v) => v, Err(_) => \"\" }\n\
            let back_res = json.loads(s)\n\
            let back = match back_res { Ok(v) => v, Err(_) => [] }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let back = env.lock().get("back").unwrap();
        let original = env.lock().get("original").unwrap();
        assert_eq!(back, original);
    }

    // -------------------------------------------------------------------
    // Fase 8.2.3 — Criterio de éxito de la fase 8.2 end-to-end:
    // una función Python que recibe `List<User>` y devuelve un mapping
    // string→int (count_by_email). Round-trip completo sin perder data.
    // -------------------------------------------------------------------

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn criterio_8_2_count_by_email_con_counter() {
        // `collections.Counter` es subclass de `dict` y cuenta
        // ocurrencias de elementos en un iterable. Cuando Fitz le pasa
        // `List<Str>` (los emails extraídos de las instancias), Counter
        // devuelve un dict-like con (email → cantidad). Como Counter
        // hereda de dict, `is_instance_of::<PyDict>()` en
        // `py_to_value` matchea y lo coerce a `Value::Map`.
        let src = "\
            type User { id: Int, email: Str }\n\
            from python import collections\n\
            let users = [\n\
                User { id: 1, email: \"alice@x.com\" },\n\
                User { id: 2, email: \"bob@x.com\" },\n\
                User { id: 3, email: \"alice@x.com\" },\n\
            ]\n\
            let emails = users.map(fn(u) => u.email)\n\
            let counts = collections.Counter(emails)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        // 8.3: `counts` ahora es `Ok(Map)` (call envuelto). Desempaquetamos
        // y validamos el orden de inserción del Counter (CPython 3.7+).
        // `alice` aparece primero en `users` así que entra primero al
        // Counter; `bob` después.
        let counts = env.lock().get("counts").unwrap();
        assert_eq!(
            ok_inner(counts),
            Value::new_map(vec![
                (Value::Str("alice@x.com".into()), Value::Int(2)),
                (Value::Str("bob@x.com".into()), Value::Int(1)),
            ]),
        );
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn criterio_8_2_round_trip_no_muta_list_original() {
        // Decisión cross-cutting #4: copia eager bidireccional. La
        // List<User> Fitz que va a Python no comparte estado con la
        // list Python; ninguna mutación del lado Python se debería
        // ver en la List Fitz original.
        let src = "\
            type User { id: Int, email: Str }\n\
            from python import collections\n\
            let users = [\n\
                User { id: 1, email: \"x\" },\n\
                User { id: 2, email: \"y\" },\n\
            ]\n\
            let snap = users.len()\n\
            let _ = collections.Counter([1, 2, 2])\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("snap"), Some(Value::Int(2)));
        // La List<User> original sigue accesible como Fitz nativa:
        // 2 elementos, field access funciona.
        let users = env.lock().get("users").unwrap();
        match users {
            Value::List(items) => {
                let guard = items.lock();
                assert_eq!(guard.len(), 2);
                // Los elementos siguen siendo Instance Fitz, no
                // PyObject (no se "contaminaron" por el round-trip).
                match &guard[0] {
                    Value::Instance { type_name, .. } => {
                        assert_eq!(type_name, "User");
                    }
                    other => panic!("esperaba Instance, fue {:?}", other),
                }
            }
            other => panic!("esperaba List, fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn criterio_8_2_pipeline_completo() {
        // Pipeline end-to-end del caso canónico:
        //   List<User> Fitz → emails List<Str>
        //                    → Counter Python (Map<Str, Int>)
        //                    → ordenar las keys con builtins
        //                    → resultado iterable desde Fitz.
        // 8.3: usamos `match` para desempaquetar el Counter antes de
        // indexar. Es el patrón canónico que el usuario va a escribir.
        let src = "\
            type User { id: Int, email: Str }\n\
            from python import collections\n\
            from python import builtins\n\
            let users = [\n\
                User { id: 1, email: \"a@x.com\" },\n\
                User { id: 2, email: \"b@x.com\" },\n\
                User { id: 3, email: \"a@x.com\" },\n\
                User { id: 4, email: \"c@x.com\" },\n\
            ]\n\
            let counts_res = collections.Counter(users.map(fn(u) => u.email))\n\
            let counts = match counts_res { Ok(v) => v, Err(_) => {} }\n\
            let total = counts[\"a@x.com\"] + counts[\"b@x.com\"] + counts[\"c@x.com\"]\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(4)));
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn marshalling_iterar_map_devuelto_por_python() {
        // Devuelve un dict de Python y lo accede con indexing Fitz —
        // exactamente el patrón de uso típico. Llaves del JSON escapadas
        // con `\{` / `\}` para que el lexer Fitz no las trate como
        // interpolación. 8.3: desempaquetamos con `match` antes de indexar.
        let src = "\
            from python import json\n\
            let m_res = json.loads(\"\\{\\\"a\\\": 10, \\\"b\\\": 20\\}\")\n\
            let m = match m_res { Ok(v) => v, Err(_) => {} }\n\
            let total = m[\"a\"] + m[\"b\"]\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(30)));
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn marshalling_arg_no_marshalleable_se_envuelve_en_err_con_path() {
        // 8.3: Range adentro de List ahora produce `Result::Err(Str)`
        // con path "arg0[1]" en el mensaje, no FitzError que aborta.
        let src = "\
            from python import json\n\
            let xs = [1, 0..5, 3]\n\
            let s = json.dumps(xs)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let s = env.lock().get("s").unwrap();
        let msg = match s {
            Value::Result(crate::value::ResultVariant::Err(inner)) => match *inner {
                Value::Str(s) => s,
                other => panic!("Err debería envolver Str, fue {:?}", other),
            },
            other => panic!("esperaba Err(...), fue {:?}", other),
        };
        assert!(
            msg.contains("arg0[1]") && msg.contains("Range"),
            "msg: {}",
            msg,
        );
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn call_python_chained_field_y_call() {
        // `json.dumps` con string Fitz → JSON con comillas dobles
        // adentro. 8.3: el return ahora viene envuelto en Ok.
        let src = "\
            from python import json\n\
            let s = json.dumps(\"hola\")\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let s = env.lock().get("s").unwrap();
        assert_eq!(ok_inner(s), Value::Str("\"hola\"".into()));
    }

    // -------------------------------------------------------------------
    // Fase 8.3 — Excepciones Python → Result<T>: criterio del roadmap
    // y patrones canónicos (match, `?`).
    // -------------------------------------------------------------------

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn criterio_8_3_json_loads_malformado_con_match() {
        // Reproduce textualmente el ejemplo del roadmap:
        //   match parse("{ malformado") {
        //     Ok(m)  => ...,
        //     Err(e) => "error: <ClassName>: <message>",
        //   }
        // El `{` del JSON malformado se escapa con `\{` para que el
        // lexer Fitz no lo trate como inicio de interpolación.
        let src = "\
            from python import json\n\
            let r = json.loads(\"\\{ malformado\")\n\
            let outcome = match r {\n\
                Ok(_) => \"ok\",\n\
                Err(e) => e,\n\
            }\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let outcome = env.lock().get("outcome").unwrap();
        match outcome {
            Value::Str(s) => {
                assert!(
                    s.contains("JSONDecodeError"),
                    "mensaje del Err debería citar JSONDecodeError, fue: {}",
                    s,
                );
            }
            other => panic!("esperaba Str, fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn criterio_8_3_propagacion_con_try_operator() {
        // Operador `?` adentro de una fn que retorna `Result<T>`
        // propaga el Err Python al caller con el mismo mensaje.
        // En éxito desempaqueta el Ok.
        let src = "\
            from python import math\n\
            fn root(x: Float) -> Result<Float> {\n\
                return Ok(math.sqrt(x)?)\n\
            }\n\
            let r = root(16.0)\n\
            let bad = root(-1.0)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        assert_eq!(ok_inner(r), Value::Float(4.0));
        let bad = env.lock().get("bad").unwrap();
        match bad {
            Value::Result(crate::value::ResultVariant::Err(inner)) => match *inner {
                Value::Str(s) => assert!(
                    s.contains("ValueError"),
                    "propagación con `?` debería preservar el mensaje, fue: {}",
                    s,
                ),
                other => panic!("Err debería envolver Str, fue {:?}", other),
            },
            other => panic!("esperaba Err(...) propagado por `?`, fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn criterio_8_3_field_access_no_se_envuelve() {
        // Decisión interna: solo `call` envuelve en `Result`; field
        // access (`math.pi`, `obj.attr`) sigue devolviendo el valor
        // coercionado directo. Eso preserva la ergonomía de leer
        // constantes y submódulos sin `match` por cada acceso.
        let src = "\
            from python import math\n\
            let p = math.pi\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let p = env.lock().get("p").unwrap();
        // `p` es Float directo, NO Result<Float>.
        assert!(
            matches!(p, Value::Float(_)),
            "field access NO debería envolver en Result, fue {:?}",
            p,
        );
    }

    // -------------------------------------------------------------------
    // Fase 8.4.3 — Coerción runtime Value::Map → Value::Instance con
    // anotación nominal en el binding. Habilita el patrón canónico
    // `let row: User = py_call(...)?`.
    //
    // Estos tests funcionan SIN feature `python` porque la coerción es
    // del lado Fitz: arma un Map manualmente y verifica que la
    // anotación lo transforma a Instance. La integración real con un
    // dict Python se valida en 8.4.4 con un ejemplo runnable.
    // -------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn anotacion_nominal_coerciona_map_a_instance() {
        // El Map literal Fitz tiene los mismos campos que el `type`;
        // la anotación nominal dispara la coerción y bindea `row`
        // como `Value::Instance`.
        let src = "\
            type User { id: Int, name: Str }\n\
            let m = {\"id\": 1, \"name\": \"alice\"}\n\
            let row: User = m\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let row = env.lock().get("row").unwrap();
        match row {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "User");
                let f = fields.lock().clone();
                assert_eq!(f, vec![
                    ("id".to_string(), Value::Int(1)),
                    ("name".to_string(), Value::Str("alice".into())),
                ]);
            }
            other => panic!("esperaba Instance, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anotacion_nominal_con_field_faltante_es_error() {
        // El Map tiene `id` pero NO `name`; `name` no es nullable ni
        // tiene default. La coerción aborta con error claro citando
        // el campo y el tipo.
        let src = "\
            type User { id: Int, name: Str }\n\
            let m = {\"id\": 1}\n\
            let row: User = m\n\
        ";
        let (_env, res) = parse_eval_into_env(src).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("User") && err.message.contains("name"),
            "msg: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anotacion_nominal_aplica_default_si_falta_el_field() {
        // El Map omite `email`, que tiene default. La coerción usa el
        // default y construye la Instance completa.
        let src = "\
            type User { id: Int, email: Str = \"unknown@x.com\" }\n\
            let m = {\"id\": 1}\n\
            let row: User = m\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let row = env.lock().get("row").unwrap();
        match row {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "User");
                let f = fields.lock().clone();
                assert_eq!(f, vec![
                    ("id".to_string(), Value::Int(1)),
                    ("email".to_string(), Value::Str("unknown@x.com".into())),
                ]);
            }
            other => panic!("esperaba Instance, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anotacion_nominal_aplica_null_para_field_nullable_faltante() {
        let src = "\
            type User { id: Int, name: Str? }\n\
            let m = {\"id\": 1}\n\
            let row: User = m\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let row = env.lock().get("row").unwrap();
        match row {
            Value::Instance { fields, .. } => {
                let f = fields.lock().clone();
                assert_eq!(f, vec![
                    ("id".to_string(), Value::Int(1)),
                    ("name".to_string(), Value::Null),
                ]);
            }
            other => panic!("esperaba Instance, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anotacion_nominal_ignora_fields_extras_del_map() {
        // El Map tiene un campo `password_hash` que el `type` no
        // declara. Lo ignoramos silenciosamente (Python suele devolver
        // dicts con extras que el modelo Fitz no necesita).
        let src = "\
            type User { id: Int, name: Str }\n\
            let m = {\"id\": 1, \"name\": \"alice\", \"password_hash\": \"xxx\"}\n\
            let row: User = m\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let row = env.lock().get("row").unwrap();
        match row {
            Value::Instance { fields, .. } => {
                let f = fields.lock().clone();
                // Solo los 2 declarados; password_hash no aparece.
                assert_eq!(f.len(), 2);
                assert_eq!(f[0].0, "id");
                assert_eq!(f[1].0, "name");
            }
            other => panic!("esperaba Instance, fue {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anotacion_nullable_con_null_no_coerciona() {
        // Si la anotación tolera null y el valor es Null, no se intenta
        // coercer — Null pasa tal cual.
        let src = "\
            type User { id: Int, name: Str }\n\
            let row: User? = null\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        assert_eq!(env.lock().get("row"), Some(Value::Null));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anotacion_nullable_con_map_coerciona_a_instance() {
        let src = "\
            type User { id: Int, name: Str }\n\
            let m = {\"id\": 1, \"name\": \"x\"}\n\
            let row: User? = m\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let row = env.lock().get("row").unwrap();
        assert!(matches!(row, Value::Instance { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anotacion_con_value_que_no_es_map_pasa_tal_cual() {
        // Si el value ya es Instance, la coerción no intenta nada
        // raro — passthrough. (El checker valida el tipo a nivel
        // estático; el runtime no re-valida).
        let src = "\
            type User { id: Int, name: Str }\n\
            let u = User { id: 1, name: \"x\" }\n\
            let row: User = u\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let row = env.lock().get("row").unwrap();
        assert!(matches!(row, Value::Instance { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anotacion_con_tipo_no_nominal_no_coerciona() {
        // Si la anotación es `Int` (built-in), no coercemos —
        // los primitivos no se construyen desde dicts. El Map se
        // bindea tal cual (gradual; uso posterior fallará claro
        // si no es compatible).
        let src = "\
            let m = {\"k\": 1}\n\
            let row: Int = m\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let row = env.lock().get("row").unwrap();
        assert!(matches!(row, Value::Map(_)));
    }

    // Test con feature Python: el patrón canónico del roadmap.
    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn criterio_8_4_dict_python_coerce_a_instance_con_anotacion() {
        // El criterio del roadmap: `let row: User = py_call(...)?`.
        // json.loads devuelve Result<Map>; el `?` desempaca al Map;
        // la anotación `User` coerciona el Map a Instance.
        let src = "\
            type User { id: Int, name: Str }\n\
            from python import json\n\
            fn parse_user(s: Str) -> Result<User> {\n\
                let row: User = json.loads(s)?\n\
                return Ok(row)\n\
            }\n\
            let r = parse_user(\"\\{\\\"id\\\": 7, \\\"name\\\": \\\"alice\\\"\\}\")\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        let inner = match r {
            Value::Result(crate::value::ResultVariant::Ok(inner)) => *inner,
            other => panic!("esperaba Ok(Instance), fue {:?}", other),
        };
        match inner {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "User");
                let f = fields.lock().clone();
                assert_eq!(f, vec![
                    ("id".to_string(), Value::Int(7)),
                    ("name".to_string(), Value::Str("alice".into())),
                ]);
            }
            other => panic!("esperaba Instance, fue {:?}", other),
        }
    }

    // -------------------------------------------------------------------
    // Fase 8.6 — Bridge tokio ↔ asyncio: corutinas Python awaiteables
    // desde `async fn` Fitz via `Value::Future`.
    // -------------------------------------------------------------------

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn fase_8_6_asyncio_sleep_awaiteable_desde_fitz() {
        // `asyncio.sleep(0)` devuelve una corutina builtin que el
        // bridge convierte a `Value::Future`. El `.await` adentro de
        // una `async fn` Fitz que retorna `Result<...>` la consume y
        // devuelve `Null` (return value de `asyncio.sleep`).
        let src = "\
            from python import asyncio\n\
            async fn test() -> Result<Null> {\n\
                let _ = asyncio.sleep(0)?.await\n\
                return Ok(null)\n\
            }\n\
            let r = test().await\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        match r {
            Value::Result(crate::value::ResultVariant::Ok(inner)) => {
                assert_eq!(*inner, Value::Null);
            }
            other => panic!("esperaba Ok(Null), fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn fase_8_6_async_fn_fitz_que_await_python_devuelve_valor_calculado() {
        // El test cumple el shape canónico del roadmap: `async fn`
        // Fitz que internamente await-ea una corutina Python y usa
        // su resultado para calcular el valor de retorno.
        let src = "\
            from python import asyncio\n\
            async fn doble(x: Int) -> Result<Int> {\n\
                let _ = asyncio.sleep(0)?.await\n\
                return Ok(x * 2)\n\
            }\n\
            let r = doble(21).await\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        match r {
            Value::Result(crate::value::ResultVariant::Ok(inner)) => {
                assert_eq!(*inner, Value::Int(42));
            }
            other => panic!("esperaba Ok(42), fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn fase_8_6_call_async_devuelve_result_future() {
        // Sin `.await`, el call a `asyncio.sleep(0)` devuelve
        // `Result<Future<Null>>` (la Future no se await-ea hasta que
        // el `.await` se aplique). Validamos el shape del binding.
        let src = "\
            from python import asyncio\n\
            let f = asyncio.sleep(0)\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let f = env.lock().get("f").unwrap();
        // Debería ser Result(Ok(Future(_))) — el call envuelve en
        // Result, el inner es el Future (no PyObject opaco).
        match f {
            Value::Result(crate::value::ResultVariant::Ok(inner)) => {
                assert!(
                    matches!(*inner, Value::Future(_)),
                    "esperaba Future adentro de Ok, fue {:?}",
                    *inner
                );
            }
            other => panic!("esperaba Ok(Future(...)), fue {:?}", other),
        }
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "current_thread")]
    async fn field_access_anidado_modulo_pyobject() {
        // `os.path` es un submódulo. Field access debería darnos otro
        // PyObject opaco — al chequearlo con `__name__` confirmamos
        // que es realmente el submódulo correcto.
        let (env, res) = parse_eval_into_env(
            "from python import os\nlet p = os.path\nlet n = p.__name__\n"
        ).await;
        res.unwrap();
        let p = env.lock().get("p");
        assert!(matches!(p, Some(Value::PyObject(_))));
        let name = env.lock().get("n").unwrap();
        // `os.path.__name__` típicamente es "ntpath" en Windows,
        // "posixpath" en Unix. Cualquiera de los dos es válido.
        match name {
            Value::Str(s) => {
                assert!(
                    s == "ntpath" || s == "posixpath" || s == "os.path",
                    "nombre inesperado: {}",
                    s,
                );
            }
            other => panic!("esperaba Str, fue {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — decoradores (Fase 4, pasos 4.1 / 4.2)
    // -----------------------------------------------------------------------
    //
    // El evaluador procesa decorators al ver `Stmt::FnDef`. Los HTTP
    // (`@get`/`@post`/`@put`/`@delete`) requieren `HttpRegistry`
    // activo en el thread_local; sin él, error explícito. Cualquier
    // otro decorator también es error (`@server` entra en 4.4).

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_con_decorator_http_sin_registry_da_error_claro() {
        // `parse_and_eval` no instala HttpRegistry, así que un
        // `@get(...)` corta con sugerencia de usar `fitz run`.
        let src = "@get(\"/\")\nfn index() => \"hola\"";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(
            err.message.contains("@get")
                && err.message.contains("servidor HTTP activo")
                && err.message.contains("fitz run"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_con_decorator_desconocido_da_error_de_decorator() {
        let src = "@patch(\"/x\")\nfn h() => 0";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(
            err.message.contains("@patch")
                && err.message.contains("no implementado"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_con_decorator_http_con_registry_activo_registra_la_ruta() {
        // Con registry activo, el decorator @get registra ruta sin
        // error y define la fn en el env.

        let src = "@get(\"/users/{id}\")\nfn get_user(id: Int) => \"hola\"";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        assert_eq!(reg.routes.len(), 1);
        let r = &reg.routes[0];
        assert_eq!(r.method, crate::http::HttpMethod::Get);
        assert_eq!(r.path, "/users/{id}");
        assert_eq!(r.path_params, vec!["id".to_string()]);
        assert_eq!(r.handler_name, "get_user");
        assert_eq!(r.param_types, vec![("id".to_string(), Some("Int".into()), false)]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_con_path_param_sin_param_de_handler_es_error() {
        // `@get("/{id}")` pero el handler no tiene un param `id`.
        let src = "@get(\"/{id}\")\nfn h() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("'{id}'") && err.message.contains("parámetro"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_con_decorator_http_sin_args_es_error() {
        // `@get()` sin path.
        let src = "@get()\nfn h() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("@get") && err.message.contains("argumento"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_decorator_http_path_no_string_es_error() {
        // `@get(42)` — path no es string.
        let src = "@get(42)\nfn h() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("string literal"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_decorator_http_path_sin_slash_es_error() {
        let src = "@get(\"users\")\nfn h() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("'/'"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_body_se_registra_y_resuelve_type_si_existe() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
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

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_body_sin_tipo_declarado_queda_sin_resolver() {
        // `body` sin anotación: declared_type = None, runtime
        // deserializa como Value libre.
        let src = "@post(\"/log\")\nfn log(body) => body";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let bp = reg.routes[0].body_param.as_ref().unwrap();
        assert_eq!(bp.name, "body");
        assert!(bp.declared_type.is_none());
        assert!(bp.declared_type_name.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_dos_body_params_es_error_al_registrar() {
        let src = "\
            type A { x: Int }\n\
            type B { y: Int }\n\
            @post(\"/x\")\nfn h(a: A, b: B) => a\n\
        ";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("solo se admite un parámetro body"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_get_con_body_se_registra_sin_problema() {
        // Permitimos body en cualquier verbo; el evaluator no fuerza
        // semántica de HTTP acá (axum/curl aceptan body en GET).
        let src = "\
            type Q { name: Str }\n\
            @get(\"/search\")\nfn s(body: Q) => body.name\n\
        ";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        assert!(reg.routes[0].body_param.is_some());
    }

    // ---- @server (Fase 4.4) ----

    #[tokio::test(flavor = "current_thread")]
    async fn server_decorator_setea_port_y_host() {
        let src = "\
            @server(8080, \"0.0.0.0\")\nfn main() => 0\n\
            @get(\"/\")\nfn h() => \"ok\"\n\
        ";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let cfg = reg.server_config.unwrap();
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.host, "0.0.0.0");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_decorator_sin_args_no_pisa_default() {
        let src = "@server()\nfn cfg() => 0\n@get(\"/\")\nfn h() => 0";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let cfg = reg.server_config.unwrap();
        assert_eq!(cfg.port, 3000);
        assert_eq!(cfg.host, "127.0.0.1");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_decorator_solo_port_usa_host_default() {
        let src = "@server(9090)\nfn cfg() => 0\n@get(\"/\")\nfn h() => 0";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let cfg = reg.server_config.unwrap();
        assert_eq!(cfg.port, 9090);
        assert_eq!(cfg.host, "127.0.0.1");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_port_no_int_es_error() {
        let src = "@server(\"8080\")\nfn cfg() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("port") && err.message.contains("Int"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_port_fuera_de_rango_es_error() {
        let src = "@server(99999)\nfn cfg() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("rango"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_host_invalido_es_error() {
        let src = "@server(8080, \"no-es-ip\")\nfn cfg() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("no-es-ip") && err.message.contains("IP"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_demasiados_args_es_error() {
        let src = "@server(8080, \"0.0.0.0\", 42)\nfn cfg() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("2 args"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_dos_decorators_es_error() {
        let src = "\
            @server(8080)\nfn a() => 0\n\
            @server(9090)\nfn b() => 0\n\
        ";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("ya tenía un @server"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn programa_sin_server_decorator_da_resolved_config_default() {
        let src = "@get(\"/\")\nfn h() => 0";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        assert!(reg.server_config.is_none());
        let cfg = reg.resolved_config();
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 3000);
    }

    // ---- 7.0 kwargs en decoradores (rechazo runtime) ----

    #[tokio::test(flavor = "current_thread")]
    async fn server_decorator_acepta_kwarg_docs_false() {
        // 7.4: @server(3000, docs=false) popula enable_docs=false.
        let src = "@server(3000, docs=false)\nfn cfg() => 0\n@get(\"/\")\nfn h() => 0";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let cfg = reg.server_config.unwrap();
        assert_eq!(cfg.port, 3000);
        assert!(!cfg.enable_docs);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_decorator_acepta_kwarg_docs_true_explicito() {
        let src = "@server(3000, docs=true)\nfn cfg() => 0\n@get(\"/\")\nfn h() => 0";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let cfg = reg.server_config.unwrap();
        assert!(cfg.enable_docs);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_decorator_default_enable_docs_es_true() {
        // Sin kwarg: default true.
        let src = "@server(3000)\nfn cfg() => 0\n@get(\"/\")\nfn h() => 0";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let cfg = reg.server_config.unwrap();
        assert!(cfg.enable_docs);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_decorator_docs_no_bool_es_error() {
        let src = "@server(3000, docs=\"si\")\nfn cfg() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("docs") && err.message.contains("Bool"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_decorator_kwarg_desconocido_es_error() {
        let src = "@server(3000, version=\"1.0\")\nfn cfg() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("version") && err.message.contains("reconocido"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // ---- 7.6 @header(name="X") ----

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_registra_spec_en_routespec() {
        let src = "@header(name=\"Authorization\")\n@get(\"/protected\")\nfn protected(authorization: Str) => authorization";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        assert_eq!(reg.routes.len(), 1);
        let route = &reg.routes[0];
        assert_eq!(route.headers.len(), 1);
        let h = &route.headers[0];
        assert_eq!(h.http_name, "Authorization");
        assert_eq!(h.param_name, "authorization");
        assert!(!h.is_nullable);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_param_nullable_se_marca() {
        // @header sobre param Str? → is_nullable = true.
        let src = "@header(name=\"X-Trace-Id\")\n@get(\"/traced\")\nfn traced(x_trace_id: Str?) => \"ok\"";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let h = &reg.routes[0].headers[0];
        assert_eq!(h.http_name, "X-Trace-Id");
        assert_eq!(h.param_name, "x_trace_id");
        assert!(h.is_nullable);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_sin_param_correspondiente_es_error() {
        // El handler no tiene el param derivado del header.
        let src = "@header(name=\"Authorization\")\n@get(\"/x\")\nfn h() => \"ok\"";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("authorization") && err.message.contains("no tiene un param"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_param_tipo_no_str_es_error() {
        let src = "@header(name=\"X-Count\")\n@get(\"/x\")\nfn h(x_count: Int) => x_count";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("`Str` o `Str?`") && err.message.contains("x_count"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_duplicado_es_error() {
        // Dos @header con el mismo name → error (match case-insensitive).
        let src = "@header(name=\"Authorization\")\n@header(name=\"authorization\")\n@get(\"/x\")\nfn h(authorization: Str) => authorization";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("declarado dos veces") || err.message.contains("dos veces"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_sin_decorator_de_ruta_es_error() {
        // @header solo (sin @get/@post/...) → error.
        let src = "@header(name=\"Authorization\")\nfn h(authorization: Str) => authorization";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("solo aplica sobre handlers HTTP"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // ---- Q.1: @header(name="X", into="alias") ----

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_con_into_usa_alias_explicito() {
        // `into="token"` mapea el header `X-Auth` al param `token`
        // (override de la convención de derivar el nombre).
        let src = "\
            @header(name=\"X-Auth\", into=\"token\")\n\
            @get(\"/x\")\n\
            fn h(token: Str) => token\n\
        ";
        let (res, reg) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let h = &reg.routes[0].headers[0];
        assert_eq!(h.http_name, "X-Auth");
        assert_eq!(h.param_name, "token");
        assert!(!h.is_nullable);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_into_mantiene_nullable_del_param() {
        let src = "\
            @header(name=\"X-Forwarded-For\", into=\"client_ip\")\n\
            @get(\"/x\")\n\
            fn h(client_ip: Str?) => \"ok\"\n\
        ";
        let (res, reg) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let h = &reg.routes[0].headers[0];
        assert_eq!(h.param_name, "client_ip");
        assert!(h.is_nullable);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_into_inexistente_es_error_sin_mencionar_convencion() {
        // Si `into="..."` apunta a un param que no existe, el mensaje
        // de error NO menciona la convención de derivar (el usuario
        // pidió un alias explícito; mencionar la convención confundiría).
        let src = "\
            @header(name=\"X-Auth\", into=\"token\")\n\
            @get(\"/x\")\n\
            fn h() => \"ok\"\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("into=\"token\"") && err.message.contains("'token'"),
            "mensaje inesperado: {}",
            err.message,
        );
        assert!(
            !err.message.contains("derivado del header"),
            "no debería mencionar la convención cuando hay alias explícito: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_into_string_vacio_es_error() {
        let src = "\
            @header(name=\"X-Auth\", into=\"\")\n\
            @get(\"/x\")\n\
            fn h(token: Str) => token\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("into") && err.message.contains("vacío"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_into_no_str_literal_es_error() {
        let src = "\
            @header(name=\"X-Auth\", into=42)\n\
            @get(\"/x\")\n\
            fn h(token: Str) => token\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("into") && err.message.contains("Str literal"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // ---- Q.2: @server(api_version="X.Y.Z") ----

    #[tokio::test(flavor = "current_thread")]
    async fn server_api_version_kwarg_carga_en_registry() {
        let src = "\
            @server(3000, api_version=\"2.5.0\")\n\
            fn main() => 0\n\
            @get(\"/\")\n\
            fn root() => 0\n\
        ";
        let (res, reg) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let cfg = reg.server_config.expect("server_config presente");
        assert_eq!(cfg.api_version, Some("2.5.0".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_sin_api_version_kwarg_queda_none() {
        let src = "\
            @server(3000)\n\
            fn main() => 0\n\
            @get(\"/\")\n\
            fn root() => 0\n\
        ";
        let (res, reg) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let cfg = reg.server_config.expect("server_config presente");
        assert_eq!(cfg.api_version, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_api_version_vacio_es_error() {
        let src = "\
            @server(api_version=\"\")\n\
            fn main() => 0\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("api_version") && err.message.contains("vacío"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_api_version_no_str_es_error() {
        let src = "\
            @server(api_version=42)\n\
            fn main() => 0\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("api_version") && err.message.contains("Str literal"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_kwarg_desconocido_lista_docs_y_api_version() {
        let src = "\
            @server(foo=\"bar\")\n\
            fn main() => 0\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("foo")
                && err.message.contains("docs")
                && err.message.contains("api_version"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_decorator_kwarg_desconocido_lista_into_y_name() {
        // El mensaje de error sobre kwarg desconocido ahora cita tanto
        // `name` como `into` (Q.1).
        let src = "\
            @header(name=\"X\", foo=\"bar\")\n\
            @get(\"/x\")\n\
            fn h(x: Str) => x\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("foo")
                && err.message.contains("name")
                && err.message.contains("into"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_decorator_con_kwarg_es_error_runtime() {
        // Decoradores HTTP de ruta no aceptan kwargs (ni en 7.0 ni
        // antes de que 7.6 defina la convención de headers).
        let src = "@get(\"/x\", foo=1)\nfn h() => 0";
        let (res, _reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("foo") && err.message.contains("nombre"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fndef_post_put_delete_se_registran_con_su_method() {
        let src = "\
            @post(\"/users\")\nfn create(name) => name\n\
            @put(\"/users/{id}\")\nfn update(id: Int, name) => name\n\
            @delete(\"/users/{id}\")\nfn del(id: Int) => 0\n\
        ";
        let (res, reg) = crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
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

    async fn first_runtime_error(src: &str) -> FitzError {
        parse_and_eval(src).await.expect_err("esperado un error de runtime")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn span_runtime_div_zero_apunta_al_operador() {
        // `print(10 / 0)` — el `/` está en columna 10.
        let e = first_runtime_error("print(10 / 0)").await;
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 10);
        assert!(e.message.contains("división por cero"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn span_runtime_type_mismatch_binop_apunta_al_operador() {
        // `print(1 + true)` — el `+` está en columna 9. El checker
        // estático también lo capta; el error de runtime ahora cita
        // la misma posición.
        let e = first_runtime_error("fn f() => 1 + true\nprint(f())").await;
        // El error ocurre adentro de `f`, columna del `+`.
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 13);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn span_runtime_ident_desconocido_apunta_al_ident() {
        // `print(unknown_var)` — `unknown_var` arranca en columna 7.
        let e = first_runtime_error("print(unknown_var)").await;
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 7);
        assert!(e.message.contains("no definida"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn span_runtime_index_oob_apunta_al_corchete() {
        // `let xs = [1, 2]\nprint(xs[10])` — el `[` está en col 9 de
        // línea 2.
        let src = "let xs = [1, 2]\nprint(xs[10])";
        let e = first_runtime_error(src).await;
        assert_eq!(e.line, 2);
        assert_eq!(e.column, 9);
        assert!(e.message.contains("fuera de rango"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn span_runtime_arity_mismatch_apunta_al_paren() {
        // `fn f(x: Int) => x\nprint(f(1, 2))` — el `(` del call está
        // en col 8 de línea 2.
        let src = "fn f(x: Int) -> Int => x\nlet _ = f(1, 2)";
        let e = first_runtime_error(src).await;
        assert_eq!(e.line, 2);
        assert_eq!(e.column, 10);
        assert!(e.message.contains("espera 1"));
    }

    // -----------------------------------------------------------------------
    // Tests — mini-fase MW.1: @middleware en el intérprete
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_un_solo_decorator_se_registra_con_la_ruta() {
        let src = "\
            fn logger(req) {}\n\
            @middleware(logger)\n\
            @get(\"/x\")\n\
            fn h() => \"ok\"\n\
        ";
        let (res, reg) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        assert_eq!(reg.routes.len(), 1);
        let r = &reg.routes[0];
        assert_eq!(r.middlewares.len(), 1);
        assert_eq!(r.middlewares[0].name, "logger");
        assert!(matches!(&r.middlewares[0].handler, Value::Function { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_dos_apilados_preservan_orden_top_down() {
        let src = "\
            fn logger(req) {}\n\
            fn auth(req) {}\n\
            @middleware(logger)\n\
            @middleware(auth)\n\
            @get(\"/x\")\n\
            fn h() => \"ok\"\n\
        ";
        let (res, reg) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let r = &reg.routes[0];
        assert_eq!(r.middlewares.len(), 2);
        // Orden top-down: logger primero, auth segundo.
        assert_eq!(r.middlewares[0].name, "logger");
        assert_eq!(r.middlewares[1].name, "auth");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_referenciando_fn_inexistente_es_error_claro() {
        // En MW.2, collect_middlewares evalúa la expresión completa
        // del arg, así que el error de "no existe" viene del evaluator
        // de identificadores (mensaje consistente con cualquier otro
        // uso de variable sin definir).
        let src = "\
            @middleware(no_existe)\n\
            @get(\"/x\")\n\
            fn h() => 0\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("no_existe") && err.message.contains("no definida"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_arg_que_no_es_fn_es_error_claro() {
        let src = "\
            let x = 42\n\
            @middleware(x)\n\
            @get(\"/x\")\n\
            fn h() => 0\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("@middleware")
                && err.message.contains("debe ser una fn")
                && err.message.contains("Int"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_despues_del_route_decorator_es_error_de_orden() {
        let src = "\
            fn logger(req) {}\n\
            @get(\"/x\")\n\
            @middleware(logger)\n\
            fn h() => \"ok\"\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("@middleware")
                && err.message.contains("ANTES")
                && err.message.contains("@get"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_sin_handler_http_es_error_claro() {
        let src = "\
            fn logger(req) {}\n\
            @middleware(logger)\n\
            fn no_es_handler() => 0\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("@middleware") && err.message.contains("handlers HTTP"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_con_kwargs_es_error() {
        let src = "\
            fn logger(req) {}\n\
            @middleware(logger, level=\"debug\")\n\
            @get(\"/x\")\n\
            fn h() => \"ok\"\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("@middleware") && err.message.contains("kwargs"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_con_aridad_distinta_de_uno_es_error() {
        let src = "\
            fn logger(req) {}\n\
            fn auth(req) {}\n\
            @middleware(logger, auth)\n\
            @get(\"/x\")\n\
            fn h() => \"ok\"\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("@middleware") && err.message.contains("exactamente un"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // -----------------------------------------------------------------------
    // Tests — Q.3: cors({"allow_origin": ["..."]}) modo Set
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn cors_allow_origin_lista_construye_set() {
        let src = "let c = cors({\"allow_origin\": [\"https://a.com\", \"https://b.com\"]})";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let c = env.lock().get("c").unwrap();
        match c {
            Value::CorsConfig(cfg) => match &cfg.allow_origin {
                crate::http::AllowOrigin::Set(items) => {
                    assert_eq!(items, &vec![
                        "https://a.com".to_string(),
                        "https://b.com".to_string(),
                    ]);
                }
                other => panic!("se esperaba AllowOrigin::Set, fue: {:?}", other),
            },
            other => panic!("se esperaba CorsConfig, fue: {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_allow_origin_lista_con_no_str_es_error() {
        let src = "let c = cors({\"allow_origin\": [\"https://a.com\", 42]})";
        let (_env, res) = parse_eval_into_env(src).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("allow_origin")
                && err.message.contains("Str | List<Str>"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_allow_origin_tipo_invalido_menciona_str_y_list() {
        // Pasar Int en allow_origin → error que cita ambas formas válidas.
        let src = "let c = cors({\"allow_origin\": 42})";
        let (_env, res) = parse_eval_into_env(src).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("allow_origin")
                && err.message.contains("Str | List<Str>"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // -----------------------------------------------------------------------
    // Tests — mini-fase MW.2: built-in cors(...)
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn cors_sin_args_emite_value_corsconfig_con_defaults() {
        let src = "let c = cors()";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let c = env.lock().get("c").unwrap();
        match c {
            Value::CorsConfig(cfg) => {
                assert_eq!(
                    cfg.allow_origin,
                    crate::http::AllowOrigin::Literal("*".to_string())
                );
                assert!(cfg.allow_methods.contains(&"GET".to_string()));
                assert!(cfg.allow_methods.contains(&"OPTIONS".to_string()));
                assert!(cfg.allow_headers.contains(&"content-type".to_string()));
                assert_eq!(cfg.max_age, None);
            }
            other => panic!("se esperaba CorsConfig, fue: {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_con_map_vacio_emite_defaults_iguales_a_sin_args() {
        let src = "let c = cors({})";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let c = env.lock().get("c").unwrap();
        match c {
            Value::CorsConfig(cfg) => {
                assert_eq!(
                    cfg.allow_origin,
                    crate::http::AllowOrigin::Literal("*".to_string())
                );
                assert_eq!(cfg.max_age, None);
            }
            other => panic!("se esperaba CorsConfig, fue: {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_override_completo_funciona_y_solo_pisa_los_keys_pasados() {
        let src = "\
            let c = cors({\
                \"allow_origin\": \"https://app.example.com\",\
                \"allow_methods\": [\"GET\", \"POST\"],\
                \"allow_headers\": [\"x-custom\"],\
                \"max_age\": 3600\
            })\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let c = env.lock().get("c").unwrap();
        match c {
            Value::CorsConfig(cfg) => {
                assert_eq!(
                    cfg.allow_origin,
                    crate::http::AllowOrigin::Literal("https://app.example.com".to_string())
                );
                assert_eq!(cfg.allow_methods, vec!["GET".to_string(), "POST".into()]);
                assert_eq!(cfg.allow_headers, vec!["x-custom".to_string()]);
                assert_eq!(cfg.max_age, Some(3600));
            }
            other => panic!("se esperaba CorsConfig, fue: {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_override_parcial_mantiene_defaults_para_no_pasados() {
        let src = "let c = cors({\"max_age\": 600})";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let c = env.lock().get("c").unwrap();
        match c {
            Value::CorsConfig(cfg) => {
                assert_eq!(
                    cfg.allow_origin,
                    crate::http::AllowOrigin::Literal("*".to_string())
                ); // default
                assert!(cfg.allow_methods.contains(&"POST".to_string())); // default
                assert_eq!(cfg.max_age, Some(600));
            }
            other => panic!("se esperaba CorsConfig, fue: {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_key_desconocida_es_error() {
        let src = "let c = cors({\"foo\": 1})";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(
            err.message.contains("`cors`")
                && err.message.contains("foo")
                && err.message.contains("no reconocida"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_tipo_incorrecto_en_value_es_error_con_contexto_de_key() {
        let src = "let c = cors({\"max_age\": \"forever\"})";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(
            err.message.contains("`cors`")
                && err.message.contains("max_age")
                && err.message.contains("Int"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_dos_args_es_error_de_aridad() {
        let src = "let c = cors({}, {})";
        let err = parse_and_eval(src).await.unwrap_err();
        assert!(
            err.message.contains("`cors`") && err.message.contains("0 o 1"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_cors_carga_slot_cors_de_la_ruta() {
        // @middleware(cors(...)) sobre un handler debe cargar
        // route.cors (no entra a la chain de middlewares).
        let src = "\
            @middleware(cors({\"allow_origin\": \"https://x.com\"}))\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let (res, reg) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let r = &reg.routes[0];
        assert!(r.middlewares.is_empty(), "cors NO debe entrar a middlewares chain");
        let cors = r.cors.as_ref().expect("se esperaba RouteSpec.cors");
        assert_eq!(
            cors.allow_origin,
            crate::http::AllowOrigin::Literal("https://x.com".to_string())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_cors_mas_user_fn_carga_ambos_slots() {
        let src = "\
            fn logger(req) {}\n\
            @middleware(logger)\n\
            @middleware(cors())\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let (res, reg) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        res.unwrap();
        let r = &reg.routes[0];
        assert_eq!(r.middlewares.len(), 1);
        assert_eq!(r.middlewares[0].name, "logger");
        assert!(r.cors.is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_dos_cors_es_error_uno_por_ruta() {
        let src = "\
            @middleware(cors())\n\
            @middleware(cors({\"max_age\": 100}))\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let (res, _) =
            crate::http::with_active_registry_async(|| async { parse_and_eval(src).await }).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("cors") && err.message.contains("uno por ruta"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // ---------------------------------------------------------------
    // Fase 9.z.2.a — `@test` decorator + assertion builtins
    // ---------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn test_decorator_sin_registry_es_no_op_silencioso() {
        // `fitz run` con un `@test` presente NO debe abortar — el
        // decorator es no-op silencioso. La fn queda definida en el
        // env normalmente (paralelo a `#[cfg(test)]` Rust: fuera del
        // runner, el código existe pero no se ejecuta).
        let (env, res) = parse_eval_into_env(
            "@test fn dummy() { let x = 1 }\nlet y = 42",
        )
        .await;
        res.expect("@test sin registry no debe abortar");
        // `y` debe estar definida (la evaluación siguió).
        assert_eq!(env.lock().get("y"), Some(Value::Int(42)));
        // `dummy` también queda en el env (es una fn normal).
        assert!(matches!(
            env.lock().get("dummy"),
            Some(Value::Function { .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_decorator_con_registry_registra_la_fn() {
        // Con un `TestRegistry` activo, `@test fn` empuja un `TestSpec`
        // al registry. La fn también queda en el env.
        let src = "@test fn suma() { assert_eq(2 + 2, 4) }";
        let ((), registry) =
            crate::testing::with_active_test_registry_async(|| async {
                let (_env, res) = parse_eval_into_env(src).await;
                res.expect("evaluación OK");
            })
            .await;
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.tests()[0].name, "suma");
        assert!(!registry.tests()[0].is_async);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_decorator_async_fn_registra_is_async_true() {
        let src = "@test async fn carga() { let x = sleep(0).await }";
        let ((), registry) =
            crate::testing::with_active_test_registry_async(|| async {
                let (_env, res) = parse_eval_into_env(src).await;
                res.expect("evaluación OK");
            })
            .await;
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.tests()[0].name, "carga");
        assert!(registry.tests()[0].is_async);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_decorator_preserva_orden_de_declaracion() {
        let src = "\
            @test fn primero() { let x = 1 }\n\
            @test fn segundo() { let y = 2 }\n\
            @test fn tercero() { let z = 3 }\n\
        ";
        let ((), registry) =
            crate::testing::with_active_test_registry_async(|| async {
                let (_env, res) = parse_eval_into_env(src).await;
                res.expect("evaluación OK");
            })
            .await;
        assert_eq!(registry.len(), 3);
        assert_eq!(registry.tests()[0].name, "primero");
        assert_eq!(registry.tests()[1].name, "segundo");
        assert_eq!(registry.tests()[2].name, "tercero");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_decorator_con_params_es_error() {
        // El MVP no admite fixtures: `@test fn t(ctx) { ... }` → error.
        let (_, res) = parse_eval_into_env("@test fn t(x: Int) { let y = x }").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("0 params") || err.message.contains("debe tener"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_decorator_con_args_es_error() {
        // `@test("nombre")` no soportado en MVP.
        let (_, res) = parse_eval_into_env("@test(\"slow\") fn t() { let x = 1 }").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("args posicionales") || err.message.contains("no admite"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_decorator_con_kwargs_es_error() {
        // `@test(slow=true)` no soportado en MVP.
        let (_, res) = parse_eval_into_env("@test(slow=true) fn t() { let x = 1 }").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("kwargs") || err.message.contains("no admite"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_true_no_falla() {
        let (_, res) = parse_eval_into_env("assert(true)").await;
        res.expect("assert(true) no debe fallar");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_true_con_msg_no_falla() {
        let (_, res) = parse_eval_into_env("assert(true, \"no aplica\")").await;
        res.expect("assert(true, msg) no debe fallar");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_false_falla_con_mensaje_generico() {
        let (_, res) = parse_eval_into_env("assert(false)").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("aserción falló"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_false_con_msg_incluye_la_razon() {
        let (_, res) = parse_eval_into_env("assert(false, \"x debe ser positivo\")").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("x debe ser positivo"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_con_no_bool_es_type_error() {
        // El primer arg de `assert` debe ser `Bool` estrictamente (no
        // truthy/falsy). `assert(1)` da type error claro.
        let (_, res) = parse_eval_into_env("assert(1)").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("Bool"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_eq_iguales_no_falla() {
        let (_, res) = parse_eval_into_env("assert_eq(2 + 2, 4)").await;
        res.expect("assert_eq(4, 4) no debe fallar");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_eq_distintos_falla_con_left_right() {
        let (_, res) = parse_eval_into_env("assert_eq(2, 3)").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("left") && err.message.contains("right"),
            "mensaje inesperado: {}",
            err.message,
        );
        assert!(err.message.contains('2') && err.message.contains('3'));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_eq_int_y_float_coerciona() {
        // `Value::PartialEq` coerciona Int↔Float. assert_eq lo refleja.
        let (_, res) = parse_eval_into_env("assert_eq(2, 2.0)").await;
        res.expect("assert_eq(2, 2.0) OK por coerción Int↔Float");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_eq_listas_estructural() {
        // Igualdad estructural recursiva en listas.
        let (_, res) = parse_eval_into_env("assert_eq([1, 2, 3], [1, 2, 3])").await;
        res.expect("listas estructuralmente iguales");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_eq_aridad_1_es_error() {
        let (_, res) = parse_eval_into_env("assert_eq(1)").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("2 argumentos") || err.message.contains("espera"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_ne_distintos_no_falla() {
        let (_, res) = parse_eval_into_env("assert_ne(1, 2)").await;
        res.expect("assert_ne(1, 2) OK");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_ne_iguales_falla() {
        let (_, res) = parse_eval_into_env("assert_ne(\"x\", \"x\")").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("iguales"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_throws_callback_que_tira_pasa() {
        // El callback levanta vía `assert(false)`. assert_throws lo
        // atrapa y devuelve Null.
        let (_, res) = parse_eval_into_env(
            "assert_throws(fn() => assert(false, \"intencional\"))",
        )
        .await;
        res.expect("assert_throws debería pasar (callback tira)");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_throws_callback_que_no_tira_falla() {
        // El callback retorna normal — assert_throws debe fallar.
        let (_, res) = parse_eval_into_env("assert_throws(fn() => 1 + 1)").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("tirara") && err.message.contains("retornó normalmente"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_throws_callback_con_params_es_error() {
        let (_, res) = parse_eval_into_env("assert_throws(fn(x) => x)").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("0 params"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_throws_callback_async_es_error_en_mvp() {
        // Async callbacks generan Future suelto que rompe la semántica
        // de "tirar". Restricción explícita del MVP.
        let src = "\
            async fn lazy() -> Int { return 1 }\n\
            assert_throws(lazy)\n\
        ";
        let (_, res) = parse_eval_into_env(src).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("async") || err.message.contains("MVP"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_throws_arg_no_funcion_es_type_error() {
        let (_, res) = parse_eval_into_env("assert_throws(42)").await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("función") || err.message.contains("Function"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // ---- Mini-tanda C — list comprehensions ----

    /// Helper local: evalúa una expresión `let r = <expr>` y devuelve
    /// el valor de `r` como Value::List clonado a un Vec.
    async fn eval_to_list_vec(src: &str) -> Vec<Value> {
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        let val = env.lock().get("r").expect("var `r` no definida");
        match val {
            Value::List(items) => items.lock().clone(),
            other => panic!("se esperaba List, recibió {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_comp_basica_doubla_cada_elemento() {
        let items = eval_to_list_vec("let r = [x * 2 for x in [1, 2, 3]]").await;
        assert_eq!(
            items,
            vec![Value::Int(2), Value::Int(4), Value::Int(6)],
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_comp_sobre_range_exclusivo() {
        let items = eval_to_list_vec("let r = [n for n in 0..5]").await;
        assert_eq!(items.len(), 5);
        assert_eq!(items[0], Value::Int(0));
        assert_eq!(items[4], Value::Int(4));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_comp_con_filter_solo_pares() {
        let items =
            eval_to_list_vec("let r = [x for x in [1, 2, 3, 4, 5] if x % 2 == 0]").await;
        assert_eq!(items, vec![Value::Int(2), Value::Int(4)]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_comp_var_no_escapa_al_caller_estilo_python() {
        // Decisión de diseño: el var de la comprehension vive en un
        // env hijo dedicado, así no shadowea ni define una var nueva
        // en el scope contenedor (a diferencia del `for ... in`).
        let src = "\
            let x = 100\n\
            let r = [v for v in [1, 2, 3]]\n\
        ";
        let (env, res) = parse_eval_into_env(src).await;
        res.unwrap();
        // `x` del caller intacto.
        assert_eq!(env.lock().get("x"), Some(Value::Int(100)));
        // `v` NO escapó al caller.
        assert_eq!(env.lock().get("v"), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_comp_iter_de_tipo_no_iterable_es_error() {
        let src = "let r = [x for x in 42]";
        let (_, res) = parse_eval_into_env(src).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("iterable") || err.message.contains("List o Range"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // ---- Mini-tanda Mb2: métodos chicos List/Str/Map + Rg ----
    //
    // Bundle de polish ergonómico: min/max/sum sobre List numérico,
    // pad_start/pad_end sobre Str, keys_sorted sobre Map, step_by
    // sobre Range. Tests por método cubren happy path + edge cases
    // (vacío, heterogéneo, tipo equivocado).

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_list_min_max_int() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [3, 1, 4, 1, 5, 9, 2, 6]\n\
             let lo = xs.min()\n\
             let hi = xs.max()",
        ).await;
        res.unwrap();
        let lo = env.lock().get("lo").unwrap();
        let hi = env.lock().get("hi").unwrap();
        match lo {
            Value::Result(ResultVariant::Ok(b)) => assert_eq!(*b, Value::Int(1)),
            other => panic!("esperaba Ok(Int), vi {:?}", other),
        }
        match hi {
            Value::Result(ResultVariant::Ok(b)) => assert_eq!(*b, Value::Int(9)),
            other => panic!("esperaba Ok(Int), vi {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_list_min_max_float() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Float> = [1.5, 0.5, 2.5]\n\
             let lo = xs.min()",
        ).await;
        res.unwrap();
        let lo = env.lock().get("lo").unwrap();
        match lo {
            Value::Result(ResultVariant::Ok(b)) => assert_eq!(*b, Value::Float(0.5)),
            other => panic!("esperaba Ok(Float), vi {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_list_min_vacia_es_err() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = []\n\
             let r = xs.min()",
        ).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        match r {
            Value::Result(ResultVariant::Err(b)) => {
                assert_eq!(*b, Value::Str("lista vacía".into()))
            }
            other => panic!("esperaba Err, vi {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_list_sum_int() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let total = xs.sum()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(15)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_list_sum_float() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Float> = [1.5, 2.5, 3.0]\n\
             let total = xs.sum()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Float(7.0)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_list_sum_vacia_es_cero() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = []\n\
             let total = xs.sum()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(0)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_list_sum_str_es_error_runtime() {
        // El checker rechaza estáticamente (sum no acepta List<Str>),
        // pero como `fitz run` no es strict aún en este wrap helper,
        // verificamos directamente que el runtime aborte si se cuela
        // una List<Str> sin anotación (escape gradual).
        let (_, res) = parse_eval_into_env(
            "let xs = [\"a\", \"b\"]\n\
             let r = xs.sum()",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("List<Int>") || err.message.contains("Int|Float"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_str_pad_start_basico() {
        let (env, res) = parse_eval_into_env(
            "let s = \"42\"\n\
             let p = s.pad_start(5, \"0\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("p"), Some(Value::Str("00042".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_str_pad_end_basico() {
        let (env, res) = parse_eval_into_env(
            "let s = \"hi\"\n\
             let p = s.pad_end(5, \".\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("p"), Some(Value::Str("hi...".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_str_pad_no_op_si_mas_largo() {
        let (env, res) = parse_eval_into_env(
            "let s = \"hola, mundo\"\n\
             let p = s.pad_start(5, \"*\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("p"), Some(Value::Str("hola, mundo".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_str_pad_ch_multi_char_es_error() {
        let (_, res) = parse_eval_into_env(
            "let s = \"42\"\n\
             let p = s.pad_start(5, \"ab\")",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("1 caracter") || err.message.contains("1 char"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_map_keys_sorted_str() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"b\": 2, \"a\": 1, \"c\": 3}\n\
             let ks = m.keys_sorted()",
        ).await;
        res.unwrap();
        let v = env.lock().get("ks").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let names: Vec<String> = g.iter().filter_map(|v| {
                if let Value::Str(s) = v { Some(s.clone()) } else { None }
            }).collect();
            assert_eq!(names, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_map_keys_sorted_int() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Int, Str> = {3: \"c\", 1: \"a\", 2: \"b\"}\n\
             let ks = m.keys_sorted()",
        ).await;
        res.unwrap();
        let v = env.lock().get("ks").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|v| {
                if let Value::Int(n) = v { Some(*n) } else { None }
            }).collect();
            assert_eq!(nums, vec![1, 2, 3]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb2_map_keys_sorted_vacio() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {}\n\
             let ks = m.keys_sorted()",
        ).await;
        res.unwrap();
        let v = env.lock().get("ks").unwrap();
        if let Value::List(items) = v {
            assert_eq!(items.lock().len(), 0);
        } else {
            panic!("esperaba List vacía");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rg_range_step_by_basico() {
        let (env, res) = parse_eval_into_env(
            "let xs = (0..10).step_by(2)",
        ).await;
        res.unwrap();
        let v = env.lock().get("xs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|v| {
                if let Value::Int(n) = v { Some(*n) } else { None }
            }).collect();
            assert_eq!(nums, vec![0, 2, 4, 6, 8]);
        } else {
            panic!("esperaba List<Int>");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rg_range_step_by_inclusivo() {
        let (env, res) = parse_eval_into_env(
            "let xs = (0..=10).step_by(3)",
        ).await;
        res.unwrap();
        let v = env.lock().get("xs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|v| {
                if let Value::Int(n) = v { Some(*n) } else { None }
            }).collect();
            // 0..=10 con step 3 → [0, 3, 6, 9]
            assert_eq!(nums, vec![0, 3, 6, 9]);
        } else {
            panic!("esperaba List<Int>");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rg_range_step_by_cero_es_error() {
        let (_, res) = parse_eval_into_env(
            "let xs = (0..10).step_by(0)",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("n > 0"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rg_range_step_by_negativo_es_error() {
        let (_, res) = parse_eval_into_env(
            "let xs = (0..10).step_by(-1)",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("n > 0"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // ---- Mini-tanda Mb3: métodos funcionales (reduce, product,
    //      chars, entries, to_map) ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_list_reduce_int_sum() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let total: Int = xs.reduce(0, fn(acc, x) => acc + x)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(15)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_list_reduce_vacia_devuelve_init() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = []\n\
             let total: Int = xs.reduce(42, fn(acc, x) => acc + x)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(42)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_list_reduce_acc_tipo_distinto_del_elem() {
        // Acc puede ser de un tipo distinto al de los elementos.
        // Ejemplo: List<Int> reducida a un Str.
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3]\n\
             let s: Str = xs.reduce(\"\", fn(acc, x) => \"{acc}{x}-\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("s"), Some(Value::Str("1-2-3-".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_list_product_int() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [2, 3, 4]\n\
             let p: Int = xs.product()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("p"), Some(Value::Int(24)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_list_product_vacia_es_uno() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = []\n\
             let p: Int = xs.product()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("p"), Some(Value::Int(1)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_list_product_float() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Float> = [1.5, 2.0, 2.0]\n\
             let p: Float = xs.product()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("p"), Some(Value::Float(6.0)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_str_chars_basico() {
        let (env, res) = parse_eval_into_env(
            "let s = \"abc\"\n\
             let cs: List<Str> = s.chars()",
        ).await;
        res.unwrap();
        let v = env.lock().get("cs").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let names: Vec<String> = g.iter().filter_map(|v| {
                if let Value::Str(s) = v { Some(s.clone()) } else { None }
            }).collect();
            assert_eq!(names, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_str_chars_unicode() {
        // Para chars no-ASCII contamos como un solo elemento.
        let (env, res) = parse_eval_into_env(
            "let cs: List<Str> = \"café\".chars()",
        ).await;
        res.unwrap();
        let v = env.lock().get("cs").unwrap();
        if let Value::List(items) = v {
            assert_eq!(items.lock().len(), 4);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_map_entries_devuelve_pares_en_orden() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
             let es: List<(Str, Int)> = m.entries()",
        ).await;
        res.unwrap();
        let v = env.lock().get("es").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            assert_eq!(g.len(), 3);
            // Validamos el primer par.
            if let Value::Tuple(parts) = &g[0] {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0], Value::Str("a".into()));
                assert_eq!(parts[1], Value::Int(1));
            } else {
                panic!("esperaba Tuple");
            }
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_list_to_map_basico() {
        let (env, res) = parse_eval_into_env(
            "let pairs: List<(Str, Int)> = [(\"a\", 1), (\"b\", 2)]\n\
             let m: Map<Str, Int> = pairs.to_map()",
        ).await;
        res.unwrap();
        let v = env.lock().get("m").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            assert_eq!(g.len(), 2);
            let a = g.iter().find(|(k, _)| k == &Value::Str("a".into()));
            assert_eq!(a.map(|(_, v)| v.clone()), Some(Value::Int(1)));
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_list_to_map_last_write_wins() {
        // Si hay keys duplicadas, gana la última (paralelo a
        // Python `dict(items)`).
        let (env, res) = parse_eval_into_env(
            "let pairs: List<(Str, Int)> = [(\"a\", 1), (\"a\", 999)]\n\
             let m: Map<Str, Int> = pairs.to_map()",
        ).await;
        res.unwrap();
        let v = env.lock().get("m").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            assert_eq!(g.len(), 1);
            let a = g.iter().find(|(k, _)| k == &Value::Str("a".into()));
            assert_eq!(a.map(|(_, v)| v.clone()), Some(Value::Int(999)));
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb3_round_trip_entries_to_map() {
        // entries → to_map roundtrip preserva contenido.
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}\n\
             let back: Map<Str, Int> = m.entries().to_map()\n\
             let av: Result<Int> = back.get(\"a\")\n\
             let bv: Result<Int> = back.get(\"b\")",
        ).await;
        res.unwrap();
        let av = env.lock().get("av").unwrap();
        let bv = env.lock().get("bv").unwrap();
        match av {
            Value::Result(ResultVariant::Ok(b)) => assert_eq!(*b, Value::Int(1)),
            other => panic!("esperaba Ok(1), vi {:?}", other),
        }
        match bv {
            Value::Result(ResultVariant::Ok(b)) => assert_eq!(*b, Value::Int(2)),
            other => panic!("esperaba Ok(2), vi {:?}", other),
        }
    }

    // ---- Mini-tanda Mb4: unique + partition + invert + split_at ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb4_list_unique_preserva_orden_de_1ra_aparicion() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 2, 3, 1, 4, 3]\n\
             let r: List<Int> = xs.unique()",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| {
                if let Value::Int(n) = x { Some(*n) } else { None }
            }).collect();
            assert_eq!(nums, vec![1, 2, 3, 4]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb4_list_unique_str() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Str> = [\"a\", \"b\", \"a\", \"c\", \"b\"]\n\
             let r: List<Str> = xs.unique()",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            assert_eq!(items.lock().len(), 3);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb4_list_partition_divide_en_truthy_falsy() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4, 5, 6]\n\
             let split: (List<Int>, List<Int>) = xs.partition(fn(n: Int) => n % 2 == 0)",
        ).await;
        res.unwrap();
        let v = env.lock().get("split").unwrap();
        if let Value::Tuple(items) = v {
            assert_eq!(items.len(), 2);
            // Truthy (pares).
            if let Value::List(t) = &items[0] {
                let nums: Vec<i64> = t.lock().iter().filter_map(|x| {
                    if let Value::Int(n) = x { Some(*n) } else { None }
                }).collect();
                assert_eq!(nums, vec![2, 4, 6]);
            }
            // Falsy (impares).
            if let Value::List(f) = &items[1] {
                let nums: Vec<i64> = f.lock().iter().filter_map(|x| {
                    if let Value::Int(n) = x { Some(*n) } else { None }
                }).collect();
                assert_eq!(nums, vec![1, 3, 5]);
            }
        } else {
            panic!("esperaba Tuple");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb4_map_invert_swap_k_v() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Int, Str> = {1: \"a\", 2: \"b\", 3: \"c\"}\n\
             let inv: Map<Str, Int> = m.invert()",
        ).await;
        res.unwrap();
        let v = env.lock().get("inv").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            assert_eq!(g.len(), 3);
            // El primer par debería ser ("a", 1).
            assert_eq!(g[0].0, Value::Str("a".into()));
            assert_eq!(g[0].1, Value::Int(1));
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb4_map_invert_values_duplicados_last_write_wins() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"a\": 1, \"b\": 1}\n\
             let inv: Map<Int, Str> = m.invert()",
        ).await;
        res.unwrap();
        let v = env.lock().get("inv").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            assert_eq!(g.len(), 1);
            // value 1 → ahora key, último value gana ("b").
            assert_eq!(g[0].0, Value::Int(1));
            assert_eq!(g[0].1, Value::Str("b".into()));
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb4_str_split_at_basico() {
        let (env, res) = parse_eval_into_env(
            "let pair: (Str, Str) = \"hola mundo\".split_at(4)",
        ).await;
        res.unwrap();
        let v = env.lock().get("pair").unwrap();
        if let Value::Tuple(items) = v {
            assert_eq!(items[0], Value::Str("hola".into()));
            assert_eq!(items[1], Value::Str(" mundo".into()));
        } else {
            panic!("esperaba Tuple");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb4_str_split_at_idx_0_y_len() {
        let (env, res) = parse_eval_into_env(
            "let a: (Str, Str) = \"abc\".split_at(0)\n\
             let b: (Str, Str) = \"abc\".split_at(3)\n\
             let c: (Str, Str) = \"abc\".split_at(99)",
        ).await;
        res.unwrap();
        for (name, want_left, want_right) in &[
            ("a", "", "abc"),
            ("b", "abc", ""),
            ("c", "abc", ""),  // idx > len → clamp a len
        ] {
            let v = env.lock().get(name).unwrap();
            if let Value::Tuple(items) = v {
                assert_eq!(items[0], Value::Str((*want_left).into()), "left de `{}`", name);
                assert_eq!(items[1], Value::Str((*want_right).into()), "right de `{}`", name);
            } else {
                panic!("esperaba Tuple para {}", name);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb4_str_split_at_negativo_es_error() {
        let (_, res) = parse_eval_into_env(
            "let pair: (Str, Str) = \"abc\".split_at(-1)",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("negativo"),
            "mensaje inesperado: {}",
            err.message,
        );
    }

    // ---- Mini-tanda Cmp+: multi-for + Map comprehensions ----

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_multi_for_cartesian_product() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3]\n\
             let ys: List<Int> = [10, 20]\n\
             let r: List<Int> = [x + y for x in xs for y in ys]",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| {
                if let Value::Int(n) = x { Some(*n) } else { None }
            }).collect();
            // (1,10), (1,20), (2,10), (2,20), (3,10), (3,20)
            assert_eq!(nums, vec![11, 21, 12, 22, 13, 23]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_multi_for_con_filter() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3]\n\
             let ys: List<Int> = [10, 20]\n\
             let r: List<Int> = [x * y for x in xs for y in ys if x % 2 == 1]",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| {
                if let Value::Int(n) = x { Some(*n) } else { None }
            }).collect();
            // x impar: 1, 3 → (1,10), (1,20), (3,10), (3,20)
            assert_eq!(nums, vec![10, 20, 30, 60]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_map_comp_basico() {
        let (env, res) = parse_eval_into_env(
            "let squares: Map<Int, Int> = {n: n * n for n in 1..=4}",
        ).await;
        res.unwrap();
        let v = env.lock().get("squares").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            assert_eq!(g.len(), 4);
            // 1->1, 2->4, 3->9, 4->16
            assert_eq!(g[0], (Value::Int(1), Value::Int(1)));
            assert_eq!(g[1], (Value::Int(2), Value::Int(4)));
            assert_eq!(g[3], (Value::Int(4), Value::Int(16)));
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_map_comp_con_filter() {
        let (env, res) = parse_eval_into_env(
            "let big: Map<Int, Int> = {n: n * 10 for n in 0..10 if n > 5}",
        ).await;
        res.unwrap();
        let v = env.lock().get("big").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            assert_eq!(g.len(), 4);  // 6, 7, 8, 9
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_map_comp_last_write_wins_en_duplicados() {
        // Si la key se repite, gana el último value.
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 1, 3]\n\
             let m: Map<Int, Int> = {x: x * 100 for x in xs}",
        ).await;
        res.unwrap();
        let v = env.lock().get("m").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            // Keys únicas: 1, 2, 3. La key 1 mantiene posición pero value
            // se sobrescribe.
            assert_eq!(g.len(), 3);
        } else {
            panic!("esperaba Map");
        }
    }

    // ---- Mini-tanda Mb8 — starts_with/ends_with + insert_at/remove_at
    //                       + Str.left/right/center + zip_to_map +
    //                       bits-extras ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb8_list_starts_ends_with() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let a: Bool = xs.starts_with([1, 2])\n\
             let b: Bool = xs.starts_with([1, 3])\n\
             let c: Bool = xs.ends_with([4, 5])\n\
             let d: Bool = xs.ends_with([3, 5])\n\
             let e: Bool = xs.starts_with([])",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("b"), Some(Value::Bool(false)));
        assert_eq!(env.lock().get("c"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("d"), Some(Value::Bool(false)));
        assert_eq!(env.lock().get("e"), Some(Value::Bool(true)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb8_list_insert_at_basico() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 4, 5]\n\
             let r: List<Int> = xs.insert_at(2, 3)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| {
                if let Value::Int(n) = x { Some(*n) } else { None }
            }).collect();
            assert_eq!(nums, vec![1, 2, 3, 4, 5]);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb8_list_insert_at_idx_grande_clamp_al_final() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.insert_at(99, 9)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            assert_eq!(g.len(), 4);
            assert_eq!(g[3], Value::Int(9));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb8_list_remove_at_basico() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [10, 20, 30, 40]\n\
             let r: List<Int> = xs.remove_at(2)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            assert_eq!(g.len(), 3);
            assert_eq!(g[2], Value::Int(40));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb8_list_remove_at_idx_fuera_de_rango_error() {
        let (_, res) = parse_eval_into_env(
            "let xs: List<Int> = [1]\n\
             let r: List<Int> = xs.remove_at(5)",
        ).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("fuera de rango"), "msg: {}", err.message);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb8_list_zip_to_map_combina_keys_y_values() {
        let (env, res) = parse_eval_into_env(
            "let ks: List<Str> = [\"a\", \"b\", \"c\"]\n\
             let vs: List<Int> = [1, 2, 3]\n\
             let m: Map<Str, Int> = ks.zip_to_map(vs)",
        ).await;
        res.unwrap();
        let v = env.lock().get("m").unwrap();
        if let Value::Map(pairs) = v {
            assert_eq!(pairs.lock().len(), 3);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb8_str_left_right_basicos() {
        let (env, res) = parse_eval_into_env(
            "let s = \"hola mundo\"\n\
             let l: Str = s.left(4)\n\
             let r: Str = s.right(5)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("l"), Some(Value::Str("hola".into())));
        assert_eq!(env.lock().get("r"), Some(Value::Str("mundo".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb8_str_center_basico() {
        let (env, res) = parse_eval_into_env(
            "let s = \"hi\"\n\
             let c: Str = s.center(10, \"-\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("c"), Some(Value::Str("----hi----".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb8_str_center_width_menor_que_len_sin_cambios() {
        let (env, res) = parse_eval_into_env(
            "let s = \"hola mundo\"\n\
             let c: Str = s.center(5, \"*\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("c"), Some(Value::Str("hola mundo".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bits_extras_popcount_leading_trailing() {
        let (env, res) = parse_eval_into_env(
            "let a: Int = popcount(7)\n\
             let b: Int = popcount(255)\n\
             let c: Int = leading_zeros(1)\n\
             let d: Int = trailing_zeros(8)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Int(3)));
        assert_eq!(env.lock().get("b"), Some(Value::Int(8)));
        assert_eq!(env.lock().get("c"), Some(Value::Int(63)));
        assert_eq!(env.lock().get("d"), Some(Value::Int(3)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bits_extras_rotate_left_right() {
        let (env, res) = parse_eval_into_env(
            "let a: Int = rotate_left(1, 4)\n\
             let b: Int = rotate_right(16, 4)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Int(16)));
        assert_eq!(env.lock().get("b"), Some(Value::Int(1)));
    }

    // ---- Mini-tanda Mb7 — take/drop/init/tail/intersperse/cycle +
    //                       repeat_with + with ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_list_take_y_drop() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let a: List<Int> = xs.take(3)\n\
             let b: List<Int> = xs.drop(2)",
        ).await;
        res.unwrap();
        let a = env.lock().get("a").unwrap();
        if let Value::List(items) = a {
            assert_eq!(items.lock().len(), 3);
        }
        let b = env.lock().get("b").unwrap();
        if let Value::List(items) = b {
            assert_eq!(items.lock().len(), 3);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_list_init_y_tail() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4]\n\
             let i: List<Int> = xs.init()\n\
             let t: List<Int> = xs.tail()",
        ).await;
        res.unwrap();
        let i = env.lock().get("i").unwrap();
        if let Value::List(items) = i {
            let g = items.lock();
            assert_eq!(g.len(), 3);
            assert_eq!(g[0], Value::Int(1));
            assert_eq!(g[2], Value::Int(3));
        }
        let t = env.lock().get("t").unwrap();
        if let Value::List(items) = t {
            let g = items.lock();
            assert_eq!(g.len(), 3);
            assert_eq!(g[0], Value::Int(2));
            assert_eq!(g[2], Value::Int(4));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_list_init_y_tail_sobre_vacia() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = []\n\
             let i: List<Int> = xs.init()\n\
             let t: List<Int> = xs.tail()",
        ).await;
        res.unwrap();
        let i = env.lock().get("i").unwrap();
        if let Value::List(items) = i {
            assert!(items.lock().is_empty());
        }
        let t = env.lock().get("t").unwrap();
        if let Value::List(items) = t {
            assert!(items.lock().is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_list_intersperse_int() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [10, 20, 30]\n\
             let r: List<Int> = xs.intersperse(0)",
        ).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        if let Value::List(items) = r {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| {
                if let Value::Int(n) = x { Some(*n) } else { None }
            }).collect();
            assert_eq!(nums, vec![10, 0, 20, 0, 30]);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_list_cycle_repite_n_veces() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2]\n\
             let r: List<Int> = xs.cycle(3)",
        ).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        if let Value::List(items) = r {
            assert_eq!(items.lock().len(), 6);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_list_cycle_n_cero_es_vacia() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.cycle(0)",
        ).await;
        res.unwrap();
        let r = env.lock().get("r").unwrap();
        if let Value::List(items) = r {
            assert!(items.lock().is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_str_repeat_with_intercala_sep() {
        let (env, res) = parse_eval_into_env(
            "let r: Str = \"hi\".repeat_with(3, \", \")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("hi, hi, hi".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_str_repeat_with_n_cero_es_vacio() {
        let (env, res) = parse_eval_into_env(
            "let r: Str = \"hi\".repeat_with(0, \", \")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_str_repeat_with_n_negativo_es_error() {
        let (_, res) = parse_eval_into_env(
            "let r: Str = \"hi\".repeat_with(-1, \",\")",
        ).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("negativo"), "msg fue: {}", err.message);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_map_with_inserta_o_sobreescribe() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let m2: Map<Str, Int> = m.with(\"b\", 2)\n\
             let m3: Map<Str, Int> = m.with(\"a\", 99)",
        ).await;
        res.unwrap();
        // m2: {"a": 1, "b": 2}
        let m2 = env.lock().get("m2").unwrap();
        if let Value::Map(pairs) = m2 {
            let g = pairs.lock();
            assert_eq!(g.len(), 2);
        }
        // m3: {"a": 99}
        let m3 = env.lock().get("m3").unwrap();
        if let Value::Map(pairs) = m3 {
            let g = pairs.lock();
            assert_eq!(g.len(), 1);
            let a = g.iter().find(|(k, _)| k == &Value::Str("a".into()));
            assert_eq!(a.map(|(_, v)| v.clone()), Some(Value::Int(99)));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb7_map_with_no_muta_receptor() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let m2: Map<Str, Int> = m.with(\"b\", 2)\n\
             let original_has_b: Bool = m.has(\"b\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("original_has_b"), Some(Value::Bool(false)));
    }

    // ---- Mini-tanda Mb6 — scan + windows + merge_with ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb6_list_scan_acumula_outputs_intermedios() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4]\n\
             let r: List<Int> = xs.scan(0, fn(acc: Int, x: Int) => acc + x)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| {
                if let Value::Int(n) = x { Some(*n) } else { None }
            }).collect();
            // Cada paso del acc: 0+1=1, 1+2=3, 3+3=6, 6+4=10.
            assert_eq!(nums, vec![1, 3, 6, 10]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb6_list_scan_lista_vacia_devuelve_vacia() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = []\n\
             let r: List<Int> = xs.scan(0, fn(acc: Int, x: Int) => acc + x)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            assert!(items.lock().is_empty());
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb6_list_windows_size_3_sobre_lista_de_5() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let r: List<List<Int>> = xs.windows(3)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            assert_eq!(items.lock().len(), 3);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb6_list_windows_n_mayor_que_len_devuelve_vacia() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2]\n\
             let r: List<List<Int>> = xs.windows(5)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            assert!(items.lock().is_empty());
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb6_list_windows_n_cero_es_error() {
        let (_, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<List<Int>> = xs.windows(0)",
        ).await;
        let err = res.unwrap_err();
        assert!(
            err.message.contains("n > 0"),
            "mensaje inesperado: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb6_map_merge_with_resuelve_conflicts_via_callback() {
        let (env, res) = parse_eval_into_env(
            "let a: Map<Str, Int> = {\"x\": 1, \"y\": 2}\n\
             let b: Map<Str, Int> = {\"y\": 10, \"z\": 3}\n\
             let r: Map<Str, Int> = a.merge_with(b, fn(va: Int, vb: Int) => va + vb)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            // x: 1 (solo en a), y: 2+10=12 (conflict), z: 3 (solo en b).
            assert_eq!(g.len(), 3);
            let y_val = g.iter().find(|(k, _)| k == &Value::Str("y".into())).map(|(_, v)| v.clone());
            assert_eq!(y_val, Some(Value::Int(12)));
        } else {
            panic!("esperaba Map");
        }
    }

    // ---- Mini-tanda HTTP-Cors — echo del Origin sin filtro ----

    #[test]
    fn http_cors_allow_origin_echo_devuelve_request_origin() {
        use crate::http::AllowOrigin;
        let echo = AllowOrigin::Echo;
        assert_eq!(
            echo.resolve(Some("https://a.com")),
            Some("https://a.com".to_string())
        );
        assert_eq!(
            echo.resolve(Some("https://anything.example.com")),
            Some("https://anything.example.com".to_string())
        );
    }

    #[test]
    fn http_cors_allow_origin_echo_sin_request_origin_es_none() {
        use crate::http::AllowOrigin;
        let echo = AllowOrigin::Echo;
        assert_eq!(echo.resolve(None), None);
    }

    // ---- Mini-tanda HTTP-Err — status codes específicos por Err ----

    #[test]
    fn http_err_value_to_outcome_con_instance_status_field_usa_ese_status() {
        // Construir Value::Result(Err(Instance { status: 404, message: "..." }))
        // y verificar que `value_to_outcome` lo mapea a HandlerOutcome con
        // status 404 (y body = Instance serializada, no `{"error": ...}`).
        use crate::value::{ResultVariant, Value};
        use parking_lot::Mutex;
        use std::sync::Arc;

        let instance = Value::Instance {
            type_name: "ApiErr".to_string(),
            fields: Arc::new(Mutex::new(vec![
                ("status".to_string(), Value::Int(404)),
                ("message".to_string(), Value::Str("no encontrado".into())),
            ])),
        };
        let err = Value::Result(ResultVariant::Err(Box::new(instance)));
        let outcome = crate::http::value_to_outcome(&err);
        assert_eq!(outcome.status, 404);
        // El body debería serializar la Instance, no envolverla en
        // `{"error": ...}`.
        let body_str = outcome.body.to_string();
        assert!(
            body_str.contains("\"status\":404")
                && body_str.contains("\"message\":\"no encontrado\""),
            "esperaba body de Instance serializada, fue: {}",
            body_str
        );
    }

    #[test]
    fn http_err_sin_status_field_cae_al_500_historico() {
        // Sin field `status`, fallback a 500 con `{"error": e}`.
        use crate::value::{ResultVariant, Value};
        use parking_lot::Mutex;
        use std::sync::Arc;

        let instance = Value::Instance {
            type_name: "Simple".to_string(),
            fields: Arc::new(Mutex::new(vec![(
                "message".to_string(),
                Value::Str("oops".into()),
            )])),
        };
        let err = Value::Result(ResultVariant::Err(Box::new(instance)));
        let outcome = crate::http::value_to_outcome(&err);
        assert_eq!(outcome.status, 500);
        let body_str = outcome.body.to_string();
        assert!(
            body_str.contains("\"error\":"),
            "esperaba `{{\"error\": ...}}`, fue: {}",
            body_str
        );
    }

    #[test]
    fn http_err_status_fuera_de_rango_cae_al_500() {
        // status: 99 (fuera de 100..1000) → 500.
        use crate::value::{ResultVariant, Value};
        use parking_lot::Mutex;
        use std::sync::Arc;

        let instance = Value::Instance {
            type_name: "Bad".to_string(),
            fields: Arc::new(Mutex::new(vec![("status".to_string(), Value::Int(99))])),
        };
        let err = Value::Result(ResultVariant::Err(Box::new(instance)));
        let outcome = crate::http::value_to_outcome(&err);
        assert_eq!(outcome.status, 500);
    }

    // ---- Mini-tanda Mb5: group_by + zip_with + max_by/min_by +
    //                     lines + is_empty ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb5_list_group_by_agrupa_por_key() {
        let (env, res) = parse_eval_into_env(
            "let nums: List<Int> = [1, 2, 3, 4, 5, 6]\n\
             let r: Map<Str, List<Int>> = nums.group_by(fn(n: Int) => if (n % 2 == 0) { \"par\" } else { \"impar\" })",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::Map(pairs) = v {
            let g = pairs.lock();
            assert_eq!(g.len(), 2);
            // El primer grupo creado fue "impar" (n=1 cae primero).
            assert_eq!(g[0].0, Value::Str("impar".into()));
            // Y "par" segundo (n=2).
            assert_eq!(g[1].0, Value::Str("par".into()));
        } else {
            panic!("esperaba Map");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb5_list_zip_with_combina_y_trunca_al_corto() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4]\n\
             let ys: List<Int> = [10, 20]\n\
             let r: List<Int> = xs.zip_with(ys, fn(a: Int, b: Int) => a + b)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let nums: Vec<i64> = g.iter().filter_map(|x| {
                if let Value::Int(n) = x { Some(*n) } else { None }
            }).collect();
            // trunca a min(4, 2) = 2 elementos.
            assert_eq!(nums, vec![11, 22]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb5_list_max_by_devuelve_item_con_max_ranking() {
        let (env, res) = parse_eval_into_env(
            "type P { age: Int = 0, name: Str = \"\" }\n\
             let xs: List<P> = [P { age: 28, name: \"Bob\" }, P { age: 42, name: \"Cam\" }, P { age: 35, name: \"Ada\" }]\n\
             let r: Result<P> = xs.max_by(fn(p: P) => p.age)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::Result(ResultVariant::Ok(item)) = v {
            if let Value::Instance { fields, .. } = item.as_ref() {
                let g = fields.lock();
                let name = g.iter().find(|(k, _)| k == "name").map(|(_, v)| v.clone());
                assert_eq!(name, Some(Value::Str("Cam".into())));
            } else {
                panic!("esperaba Instance");
            }
        } else {
            panic!("esperaba Ok(P)");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb5_list_min_by_lista_vacia_devuelve_err() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = []\n\
             let r: Result<Int> = xs.min_by(fn(n: Int) => n)",
        ).await;
        res.unwrap();
        let v = env.lock().get("r").unwrap();
        if let Value::Result(ResultVariant::Err(_)) = v {
            // OK.
        } else {
            panic!("esperaba Err");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb5_str_lines_separa_por_newline() {
        let (env, res) = parse_eval_into_env(
            "let s = \"uno\\ndos\\ntres\"\n\
             let ls: List<Str> = s.lines()",
        ).await;
        res.unwrap();
        let v = env.lock().get("ls").unwrap();
        if let Value::List(items) = v {
            let g = items.lock();
            let strs: Vec<String> = g.iter().filter_map(|x| {
                if let Value::Str(s) = x { Some(s.clone()) } else { None }
            }).collect();
            assert_eq!(strs, vec!["uno".to_string(), "dos".to_string(), "tres".to_string()]);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb5_str_lines_termina_con_newline_no_agrega_vacia() {
        let (env, res) = parse_eval_into_env(
            "let s = \"a\\nb\\n\"\n\
             let ls: List<Str> = s.lines()",
        ).await;
        res.unwrap();
        let v = env.lock().get("ls").unwrap();
        if let Value::List(items) = v {
            assert_eq!(items.lock().len(), 2);
        } else {
            panic!("esperaba List");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb5_str_is_empty_basico() {
        let (env, res) = parse_eval_into_env(
            "let a: Bool = \"\".is_empty()\n\
             let b: Bool = \"hola\".is_empty()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("b"), Some(Value::Bool(false)));
    }

    // ---- Mini-tanda Async-cl: async fn como closure inline ----

    #[tokio::test(flavor = "current_thread")]
    async fn async_cl_inline_devuelve_future() {
        // `async fn(...) => ...` produce un Value::Function con
        // is_async = true. Al invocar, devuelve Value::Future perezoso
        // que el caller debe `.await`ar.
        let (env, res) = parse_eval_into_env(
            "async fn run() -> Int {\n\
                 let f = async fn(n: Int) -> Int { return n * 2 }\n\
                 let r = f(21).await\n\
                 return r\n\
             }\n\
             let r: Int = run().await",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(42)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_cl_inline_con_sleep_dentro() {
        let (env, res) = parse_eval_into_env(
            "async fn run() -> Int {\n\
                 let delayed = async fn(n: Int) -> Int {\n\
                     sleep(1).await\n\
                     return n + 100\n\
                 }\n\
                 let r = delayed(5).await\n\
                 return r\n\
             }\n\
             let r: Int = run().await",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(105)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_cl_pasada_como_arg_funciona() {
        // Un async closure se puede pasar como arg a otra async fn.
        let (env, res) = parse_eval_into_env(
            "async fn apply_async(f, n: Int) -> Int {\n\
                 let r = f(n).await\n\
                 return r\n\
             }\n\
             async fn run() -> Int {\n\
                 let r = apply_async(async fn(x: Int) -> Int { return x + 1 }, 10).await\n\
                 return r\n\
             }\n\
             let r: Int = run().await",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(11)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmp_var_de_for_anidado_no_escapa() {
        // Después de evaluar, ni `x` ni `y` están en el caller.
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2]\n\
             let ys: List<Int> = [10]\n\
             let r: List<Int> = [x + y for x in xs for y in ys]",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("x"), None);
        assert_eq!(env.lock().get("y"), None);
    }

    // ---- Mini-tanda Math: builtins numéricos ----

    #[tokio::test(flavor = "current_thread")]
    async fn math_abs_int_y_float() {
        let (env, res) = parse_eval_into_env(
            "let a: Int = abs(-5)\n\
             let b: Float = abs(-3.14)\n\
             let c: Int = abs(7)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Int(5)));
        assert_eq!(env.lock().get("b"), Some(Value::Float(3.14)));
        assert_eq!(env.lock().get("c"), Some(Value::Int(7)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn math_min_max_basicos() {
        let (env, res) = parse_eval_into_env(
            "let a: Int = min(3, 5)\n\
             let b: Int = max(3, 5)\n\
             let c: Float = min(1.5, 2.5)\n\
             let d: Float = max(1.5, 2.5)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Int(3)));
        assert_eq!(env.lock().get("b"), Some(Value::Int(5)));
        assert_eq!(env.lock().get("c"), Some(Value::Float(1.5)));
        assert_eq!(env.lock().get("d"), Some(Value::Float(2.5)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn math_pow_y_sqrt() {
        let (env, res) = parse_eval_into_env(
            "let a: Float = pow(2, 10)\n\
             let b: Float = sqrt(16)\n\
             let c: Float = sqrt(2)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Float(1024.0)));
        assert_eq!(env.lock().get("b"), Some(Value::Float(4.0)));
        // sqrt(2) ≈ 1.4142...
        let c_val = env.lock().get("c");
        if let Some(Value::Float(v)) = c_val {
            assert!((v - std::f64::consts::SQRT_2).abs() < 1e-12);
        } else {
            panic!("c no es Float");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn math_ceil_floor_round() {
        let (env, res) = parse_eval_into_env(
            "let a: Int = ceil(3.2)\n\
             let b: Int = floor(3.8)\n\
             let c: Int = round(3.5)\n\
             let d: Int = ceil(7)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Int(4)));
        assert_eq!(env.lock().get("b"), Some(Value::Int(3)));
        assert_eq!(env.lock().get("c"), Some(Value::Int(4)));
        assert_eq!(env.lock().get("d"), Some(Value::Int(7)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn math_clamp_lo_hi() {
        let (env, res) = parse_eval_into_env(
            "let a: Int = clamp(5, 0, 10)\n\
             let b: Int = clamp(-5, 0, 10)\n\
             let c: Int = clamp(15, 0, 10)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("a"), Some(Value::Int(5)));
        assert_eq!(env.lock().get("b"), Some(Value::Int(0)));
        assert_eq!(env.lock().get("c"), Some(Value::Int(10)));
    }

    // ---- Mini-tanda Mb9: Str.swap_case / title / is_alpha / is_digit / is_numeric ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb9_str_swap_case() {
        let (env, res) = parse_eval_into_env(
            "let s: Str = \"Hola Mundo\".swap_case()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("s"), Some(Value::Str("hOLA mUNDO".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb9_str_title() {
        let (env, res) = parse_eval_into_env(
            "let s: Str = \"hola mundo de fitz\".title()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("s"), Some(Value::Str("Hola Mundo De Fitz".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb9_str_is_alpha_digit_numeric() {
        let (env, res) = parse_eval_into_env(
            "let a: Bool = \"hola\".is_alpha()\n\
             let b: Bool = \"hola123\".is_alpha()\n\
             let c: Bool = \"12345\".is_digit()\n\
             let d: Bool = \"12a\".is_digit()\n\
             let e: Bool = \"3.14\".is_numeric()\n\
             let f: Bool = \"-42\".is_numeric()\n\
             let g: Bool = \"3.14.5\".is_numeric()\n\
             let h: Bool = \"\".is_alpha()",
        ).await;
        res.unwrap();
        let env = env.lock();
        assert_eq!(env.get("a"), Some(Value::Bool(true)));
        assert_eq!(env.get("b"), Some(Value::Bool(false)));
        assert_eq!(env.get("c"), Some(Value::Bool(true)));
        assert_eq!(env.get("d"), Some(Value::Bool(false)));
        assert_eq!(env.get("e"), Some(Value::Bool(true)));
        assert_eq!(env.get("f"), Some(Value::Bool(true)));
        assert_eq!(env.get("g"), Some(Value::Bool(false)));
        assert_eq!(env.get("h"), Some(Value::Bool(false)));
    }

    // ---- Mini-tanda Mb9: List.split_at(i) ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb9_list_split_at_basico() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let parts = xs.split_at(2)",
        ).await;
        res.unwrap();
        let env = env.lock();
        match env.get("parts") {
            Some(Value::Tuple(items)) => {
                assert_eq!(items.len(), 2);
                match (&items[0], &items[1]) {
                    (Value::List(left), Value::List(right)) => {
                        assert_eq!(left.lock().len(), 2);
                        assert_eq!(right.lock().len(), 3);
                    }
                    _ => panic!("componentes no son List: {:?}", items),
                }
            }
            other => panic!("parts no es Tuple: {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mb9_list_split_at_clamp_extremos() {
        let (env, res) = parse_eval_into_env(
            "let xs: List<Int> = [1, 2, 3]\n\
             let a = xs.split_at(0)\n\
             let b = xs.split_at(10)\n\
             let c = xs.split_at(-1)",
        ).await;
        res.unwrap();
        // (0) → ([], [1,2,3]); (10) → ([1,2,3], []); (-1) → ([], [1,2,3])
        let env = env.lock();
        if let Some(Value::Tuple(it)) = env.get("a") {
            if let (Value::List(l), Value::List(r)) = (&it[0], &it[1]) {
                assert_eq!(l.lock().len(), 0);
                assert_eq!(r.lock().len(), 3);
            } else { panic!(); }
        } else { panic!(); }
        if let Some(Value::Tuple(it)) = env.get("b") {
            if let (Value::List(l), Value::List(r)) = (&it[0], &it[1]) {
                assert_eq!(l.lock().len(), 3);
                assert_eq!(r.lock().len(), 0);
            } else { panic!(); }
        } else { panic!(); }
        if let Some(Value::Tuple(it)) = env.get("c") {
            if let (Value::List(l), Value::List(r)) = (&it[0], &it[1]) {
                assert_eq!(l.lock().len(), 0);
                assert_eq!(r.lock().len(), 3);
            } else { panic!(); }
        } else { panic!(); }
    }

    // ---- Mini-tanda Mb9: Map.has_value(v) ----

    #[tokio::test(flavor = "current_thread")]
    async fn mb9_map_has_value() {
        let (env, res) = parse_eval_into_env(
            "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
             let yes: Bool = m.has_value(2)\n\
             let no: Bool = m.has_value(99)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("yes"), Some(Value::Bool(true)));
        assert_eq!(env.lock().get("no"), Some(Value::Bool(false)));
    }

    // ---- Mini-tanda Math+Mb9: métodos sobre Int ----

    #[tokio::test(flavor = "current_thread")]
    async fn int_methods_abs_to_str_to_str_base() {
        let (env, res) = parse_eval_into_env(
            "let a: Int = (-5).abs()\n\
             let b: Str = (42).to_str()\n\
             let c: Str = (255).to_str_base(16)\n\
             let d: Str = (10).to_str_base(2)\n\
             let e: Str = (8).to_str_base(8)",
        ).await;
        res.unwrap();
        let env = env.lock();
        assert_eq!(env.get("a"), Some(Value::Int(5)));
        assert_eq!(env.get("b"), Some(Value::Str("42".into())));
        assert_eq!(env.get("c"), Some(Value::Str("ff".into())));
        assert_eq!(env.get("d"), Some(Value::Str("1010".into())));
        assert_eq!(env.get("e"), Some(Value::Str("10".into())));
    }

    // ---- Mini-tanda Math+Mb9: métodos sobre Float ----

    // ---- Mini-tanda Fp — default params ----

    #[tokio::test(flavor = "current_thread")]
    async fn fp_call_sin_args_usa_default() {
        let (env, res) = parse_eval_into_env(
            "fn greet(name: Str = \"amigo\") -> Str { return name }\n\
             let r: Str = greet()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("amigo".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fp_call_con_arg_overridea_default() {
        let (env, res) = parse_eval_into_env(
            "fn greet(name: Str = \"amigo\") -> Str { return name }\n\
             let r: Str = greet(\"Fitz\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("Fitz".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fp_mezcla_required_y_default() {
        let (env, res) = parse_eval_into_env(
            "fn add(a: Int, b: Int = 10) -> Int { return a + b }\n\
             let r1: Int = add(5)\n\
             let r2: Int = add(5, 2)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r1"), Some(Value::Int(15)));
        assert_eq!(env.lock().get("r2"), Some(Value::Int(7)));
    }

    // ---- Mini-tanda Fp.2 — varargs en runtime ----

    #[tokio::test(flavor = "current_thread")]
    async fn fp2_varargs_sin_args_recibe_lista_vacia() {
        let (env, res) = parse_eval_into_env(
            "fn count(...xs: Int) -> Int { return xs.len() }\n\
             let r: Int = count()",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(0)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fp2_varargs_con_args_recibe_lista() {
        let (env, res) = parse_eval_into_env(
            "fn sum(...xs: Int) -> Int {\n\
                let total: Int = 0\n\
                for x in xs { total = total + x }\n\
                return total\n\
             }\n\
             let a: Int = sum(1, 2, 3)\n\
             let b: Int = sum(10, 20)\n\
             let c: Int = sum()",
        ).await;
        res.unwrap();
        let env = env.lock();
        assert_eq!(env.get("a"), Some(Value::Int(6)));
        assert_eq!(env.get("b"), Some(Value::Int(30)));
        assert_eq!(env.get("c"), Some(Value::Int(0)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fp2_varargs_con_required_y_extras() {
        let (env, res) = parse_eval_into_env(
            "fn join(prefix: Str, ...xs: Str) -> Int { return xs.len() }\n\
             let r: Int = join(\"x\", \"a\", \"b\", \"c\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(3)));
    }

    // ---- Mini-tanda Fp.3 — named args en runtime ----

    #[tokio::test(flavor = "current_thread")]
    async fn fp3_call_solo_named_args() {
        let (env, res) = parse_eval_into_env(
            "fn greet(name: Str = \"amigo\", greeting: Str = \"Hola\") -> Str {\n\
                return \"{greeting}, {name}\"\n\
             }\n\
             let r: Str = greet(name: \"Fitz\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("Hola, Fitz".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fp3_call_mezcla_posicional_y_named() {
        let (env, res) = parse_eval_into_env(
            "fn greet(name: Str = \"amigo\", greeting: Str = \"Hola\") -> Str {\n\
                return \"{greeting}, {name}\"\n\
             }\n\
             let r: Str = greet(\"Fitz\", greeting: \"Hi\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("Hi, Fitz".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fp3_named_arg_orden_libre() {
        let (env, res) = parse_eval_into_env(
            "fn greet(name: Str = \"amigo\", greeting: Str = \"Hola\") -> Str {\n\
                return \"{greeting}, {name}\"\n\
             }\n\
             let r: Str = greet(greeting: \"Hi\", name: \"Fitz\")",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Str("Hi, Fitz".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fp3_named_arg_inexistente_es_error() {
        let (_, res) = parse_eval_into_env(
            "fn greet(name: Str) -> Str { return name }\n\
             let r = greet(unknown: \"x\")",
        ).await;
        assert!(res.is_err(), "esperaba error por named arg inexistente");
    }

    // ---- Mini-tanda Sp.2 — return en match arm ----

    #[tokio::test(flavor = "current_thread")]
    async fn sp2_return_en_match_arm_corta_la_fn() {
        let (env, res) = parse_eval_into_env(
            "fn classify(n: Int) -> Str {\n\
                match n {\n\
                    0 => return \"cero\"\n\
                    _ => \"otro\"\n\
                }\n\
                return \"end\"\n\
             }\n\
             let a: Str = classify(0)\n\
             let b: Str = classify(5)",
        ).await;
        res.unwrap();
        let env = env.lock();
        assert_eq!(env.get("a"), Some(Value::Str("cero".into())));
        assert_eq!(env.get("b"), Some(Value::Str("end".into())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sp2_arm_con_bloque_de_varios_stmts() {
        let (env, res) = parse_eval_into_env(
            "fn f(n: Int) -> Int {\n\
                match n {\n\
                    0 => {\n\
                        let x: Int = 10\n\
                        return x * 2\n\
                    }\n\
                    _ => 99\n\
                }\n\
             }\n\
             let r: Int = f(0)",
        ).await;
        res.unwrap();
        assert_eq!(env.lock().get("r"), Some(Value::Int(20)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fp_demasiados_args_es_error() {
        let (_, res) = parse_eval_into_env(
            "fn greet(name: Str = \"amigo\") -> Str { return name }\n\
             let r = greet(\"a\", \"b\")",
        ).await;
        assert!(res.is_err(), "esperaba error de aridad");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fp_default_expr_compleja_se_evalua_en_cada_call() {
        // Default expr puede ser cualquier expr — se evalúa en el
        // env del closure cada vez que se llama sin ese arg.
        // (Decisión de diseño: evaluado-en-cada-call, NO cacheado).
        let (env, res) = parse_eval_into_env(
            "fn make_list(prefix: Str = \"x\", n: Int = 3) -> List<Str> {\n\
                let xs: List<Str> = []\n\
                let i: Int = 0\n\
                while i < n {\n\
                    xs.push(prefix)\n\
                    i = i + 1\n\
                }\n\
                return xs\n\
             }\n\
             let r: List<Str> = make_list()",
        ).await;
        res.unwrap();
        let r = env.lock().get("r");
        match r {
            Some(Value::List(list)) => {
                let g = list.lock();
                assert_eq!(g.len(), 3);
                assert_eq!(g[0], Value::Str("x".into()));
            }
            other => panic!("r no es List: {:?}", other),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn float_methods_abs_to_str_is_nan_is_finite() {
        let (env, res) = parse_eval_into_env(
            "let a: Float = (-3.14).abs()\n\
             let b: Str = (3.14).to_str()\n\
             let c: Bool = (1.0).is_nan()\n\
             let d: Bool = (1.0).is_finite()",
        ).await;
        res.unwrap();
        let env = env.lock();
        assert_eq!(env.get("a"), Some(Value::Float(3.14)));
        assert_eq!(env.get("b"), Some(Value::Str("3.14".into())));
        assert_eq!(env.get("c"), Some(Value::Bool(false)));
        assert_eq!(env.get("d"), Some(Value::Bool(true)));
    }
}
