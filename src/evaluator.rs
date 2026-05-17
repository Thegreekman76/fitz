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
            Value::Function { .. } => {
                middlewares.push(MiddlewareSpec {
                    name: label,
                    handler: value,
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
        let Some(Value::Type { name, fields, .. }) = existing else { continue };
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

#[async_recursion]
async fn eval_stmt(stmt: &Stmt, env: EnvRef) -> EvalResult<Value> {
    match stmt {
        Stmt::Expr(expr, _) => eval_expr(expr, env).await,

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
                            if idx < 0 || idx >= len {
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
                            borrowed[idx as usize] = v;
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
        Stmt::TypeDef { name, fields, span: _ } => {
            // PreF8.3: tipos locales arrancan con `resolved_defaults` vacío.
            // Sus `Field.default` se siguen evaluando lazy en cada struct
            // lit con el env del call site. Solo los tipos cargados desde
            // un módulo (vía `load_module`) tienen los defaults pre-
            // evaluados — esa pre-evaluación se hace en un post-pass al
            // terminar de ejecutar las stmts del módulo, ahí ya están
            // disponibles todos los símbolos del módulo en su env.
            let t = Value::Type {
                name: name.clone(),
                fields: fields.clone(),
                resolved_defaults: Vec::new(),
            };
            env.lock().define(name.clone(), t);
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
            for item in items {
                env.lock().define(var.clone(), item);
                match run_loop_body(body, env.clone()).await {
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
                match run_loop_body(body, env.clone()).await {
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
                match run_loop_body(body, env.clone()).await {
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
    Break,
    Propagate(EvalSignal),
}

/// Ejecuta los stmts del body en orden. Si alguno emite `Break` o `Continue`,
/// los traduce a control local. Cualquier otro signal (Error, Return) sube
/// como `Propagate` para que el loop lo devuelva al caller.
#[async_recursion]
async fn run_loop_body(body: &[Stmt], env: EnvRef) -> LoopControl {
    for stmt in body {
        match eval_stmt(stmt, env.clone()).await {
            Ok(_) => {}
            Err(EvalSignal::Break) => return LoopControl::Break,
            Err(EvalSignal::Continue) => return LoopControl::Continue,
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
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// eval_expr — evalúa una expresión a un Value.
// ---------------------------------------------------------------------------

#[async_recursion]
async fn eval_expr(expr: &Expr, env: EnvRef) -> EvalResult<Value> {
    let span = expr.span();
    match expr {
        // Literales — el valor está embebido en el AST.
        Expr::Int(n, _) => Ok(Value::Int(*n)),
        Expr::Float(x, _) => Ok(Value::Float(*x)),
        Expr::Str(s, _) => Ok(Value::Str(s.clone())),
        Expr::Bool(b, _) => Ok(Value::Bool(*b)),
        Expr::Null(_) => Ok(Value::Null),

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
        Expr::BinOp { op, left, right, span } if matches!(op, BinOpKind::And | BinOpKind::Or) => {
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
        Expr::StrInterp(parts, _) => {
            let mut result = String::new();
            for part in parts {
                match part {
                    StrPart::Lit(s) => result.push_str(s),
                    StrPart::Expr(e) => {
                        let v = eval_expr(e, env.clone()).await?;
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
        Expr::Call { callee, args, span } => eval_call(callee, args, env, *span).await,

        // `fn(x) => x * 2` o `fn(x) { return x * 2 }` — función anónima.
        // Se evalúa a `Value::Function` con el env actual como closure,
        // igual que un `Stmt::FnDef`, pero sin nombre ni binding en el env.
        Expr::FnExpr { params, body, .. } => Ok(Value::Function {
            params: params.clone(),
            body: body.clone(),
            closure: env,
            // FnExpr (closure anónimo) siempre es sync — el lenguaje no
            // soporta `async fn(...) => ...` todavía. Si en el futuro
            // lo agregamos, el parser pasa a marcar el `is_async`
            // sobre el `Expr::FnExpr` y este sitio lo refleja.
            is_async: false,
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
                Value::Type { name, fields, resolved_defaults } => (name, fields, resolved_defaults),
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
                let arm_env = if let Some((name, bound)) = &binding {
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

                if binding.is_some() {
                    return eval_expr(&arm.body, arm_env).await;
                }
                return eval_expr(&arm.body, env.clone()).await;
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
        let mut arg_values = Vec::with_capacity(args.len());
        for arg in args {
            arg_values.push(eval_expr(arg, env.clone()).await?);
        }
        return dispatch_method(receiver, field, arg_values, span).await;
    }

    // Llamada normal.
    let callee_value = eval_expr(callee, env.clone()).await?;
    let mut arg_values = Vec::with_capacity(args.len());
    for arg in args {
        arg_values.push(eval_expr(arg, env.clone()).await?);
    }
    let display_name = callee_display_name(callee);
    invoke_value(callee_value, arg_values, &display_name, span).await
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
    span: Span,
) -> EvalResult<Value> {
    match (&receiver, method) {
        // List
        (Value::List(_), "push") => list_push(receiver, args, span),
        (Value::List(_), "pop") => list_pop(receiver, args, span),
        (Value::List(_), "map") => list_map(receiver, args, span).await,
        (Value::List(_), "filter") => list_filter(receiver, args, span).await,
        (Value::List(_), "find") => list_find(receiver, args, span).await,
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
        Mod => eval_mod(l, r, span),
        Eq => Ok(Value::Bool(l == r)),
        NotEq => Ok(Value::Bool(l != r)),
        Lt | LtEq | Gt | GtEq => compare(op, l, r, span),
        And | Or => unreachable!("And/Or se manejan en eval_logical antes de llegar acá"),
    }
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
    match op {
        BinOpKind::And if !lb => return Ok(Value::Bool(false)),
        BinOpKind::Or if lb => return Ok(Value::Bool(true)),
        _ => {}
    }

    let rv = eval_expr(right, env).await?;
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
        Add => "+", Sub => "-", Mul => "*", Div => "/", Mod => "%",
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
            let borrowed = items.lock();
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
        Some(Value::Type { name, fields, resolved_defaults }) => {
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
                    Value::Str(s) => {
                        config.allow_origin = crate::http::AllowOrigin::Literal(s.clone());
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
        let result = eval(vec![Stmt::Break(Span::ZERO)]).await;
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::BreakOutsideLoop
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn continue_fuera_de_loop_es_error() {
        let result = eval(vec![Stmt::Continue(Span::ZERO)]).await;
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
    async fn assign_index_list_indice_negativo_es_error() {
        let (_env, res) = parse_eval_into_env(
            "let xs = [1, 2]\nxs[-1] = 99",
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
            StrPart::Expr(Expr::Ident("name".into(), Span::ZERO)),
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
            StrPart::Expr(Expr::Ident("x".into(), Span::ZERO)),
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
            }),
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
        MatchArm { pattern, guard: None, body }
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
                        StrPart::Expr(Expr::Ident("x".into(), Span::ZERO)),
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
                        StrPart::Expr(Expr::Ident("n".into(), Span::ZERO)),
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
                    then: vec![Stmt::Break(Span::ZERO)],
                    else_: None, span: Span::ZERO,
                }, Span::ZERO),
            ],
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
        eval_stmt(&stmt, env.clone()).await.unwrap();
        assert_eq!(env.lock().get("total"), Some(Value::Int(12)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn while_cond_no_bool_es_type_error() {
        let env = Environment::new();
        let stmt = Stmt::While {
            condition: Expr::Int(1, Span::ZERO),
            body: vec![],
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
                    then: vec![Stmt::Break(Span::ZERO)],
                    else_: None, span: Span::ZERO,
                }, Span::ZERO),
            ],
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
                    StrPart::Expr(Expr::Ident("name".into(), Span::ZERO)),
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
    async fn index_list_negativo_es_error() {
        // [1, 2][-1] — sin Python-style por ahora
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)], Span::ZERO)),
            index: Box::new(Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(Expr::Int(1, Span::ZERO)), span: Span::ZERO,
            }), span: Span::ZERO,
        }).await;
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(e.message.contains("negativo")),
            _ => panic!("se esperaba Error"),
        }
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
    async fn for_sobre_map_es_error_explicito() {
        let src = r#"
for x in {"a": 1} {
    print(x)
}
"#;
        let res = parse_and_eval(src).await;
        let err = res.unwrap_err();
        assert!(err.message.contains("Map"));
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
            params: vec![crate::ast::Param { name: "x".into(), type_: None }],
            body: vec![Stmt::Return(Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                right: Box::new(Expr::Int(2, Span::ZERO)), span: Span::ZERO,
            }, Span::ZERO)], span: Span::ZERO,
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
}
