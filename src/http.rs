// http.rs — Fase 4 (HTTP nativo)
//
// Runtime HTTP de Fitz. Se ensambla en dos pasos:
//
//   1. Durante `eval`, cuando se ve un `Stmt::FnDef` con un decorator
//      `@get`/`@post`/`@put`/`@delete`, se registra una `RouteSpec` en un
//      `HttpRegistry` accesible vía thread_local.
//   2. Al terminar `eval`, si el registry quedó no vacío, `serve()`
//      arranca un runtime tokio + axum y bloquea hasta Ctrl-C.
//
// Threading model (post-F17.5):
//
//   Un único runtime tokio `rt-multi-thread` (F17.4a) corre en el
//   thread que llamó `eval` (`block_on` en `serve()`). Cada request
//   axum dispatchea un handler async en alguno de los workers, que
//   invoca `handle_task(&registry, ...).await` directo sobre el
//   evaluator. `HttpRegistry` se comparte por `Arc` (Send + Sync
//   post-F17.2-3). El paralelismo entre requests es real: N workers
//   procesando handlers simultáneos.
//
// Antes de F17.5 había un bridge mpsc/oneshot + un std::thread aparte
// para tokio. Lo introdujo Fase 4 cuando `Value`/`EnvRef` eran
// `Rc<RefCell<>>` no-Send y los handlers no podían invocarse desde
// axum directo. F17.2 (Arc/Mutex), F17.3 (Send completo) y F17.4a
// (multi-thread) destrabaron la eliminación. Resultado: ~300 LoC
// menos acá y paralelismo HTTP real entre requests.

use std::cell::RefCell;

use crate::ast::{Expr, TypeExpr};
#[cfg(test)]
use crate::ast::Span;
use crate::value::{shared, Value, ResultVariant};

// ---------------------------------------------------------------------------
// Tipos base
// ---------------------------------------------------------------------------

/// Verbo HTTP soportado por un decorator. Vivo solo en runtime del
/// servidor; el AST no lo usa (los decorators son genéricos).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl HttpMethod {
    /// Convierte un nombre de decorator (`"get"`, `"post"`, ...) al
    /// verbo correspondiente. `None` si no es un decorator HTTP.
    pub fn from_decorator_name(name: &str) -> Option<HttpMethod> {
        match name {
            "get" => Some(HttpMethod::Get),
            "post" => Some(HttpMethod::Post),
            "put" => Some(HttpMethod::Put),
            "delete" => Some(HttpMethod::Delete),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
        }
    }
}

/// Una ruta registrada por un decorator. El `handler` es un
/// `Value::Function` clonado del env del intérprete — los `Rc` se
/// clonan barato y la closure mantiene viva la env del módulo.
#[derive(Debug, Clone)]
pub struct RouteSpec {
    pub method: HttpMethod,
    /// Path en formato axum (`/users/{id}`). Ya canonicalizado del
    /// `Expr::Str` o `Expr::StrInterp` del decorator. El query
    /// template (después del `?`) NO entra acá — vive en
    /// `query_params`.
    pub path: String,
    /// Nombres de los path params, en el orden en que aparecen en el
    /// path. Vacío si la ruta no tiene params.
    pub path_params: Vec<String>,
    /// Nombres de los query params declarados con `?key={name}` en el
    /// path del decorator. Cada uno se bindea al param Fitz del mismo
    /// nombre. Vacío si la ruta no declara query.
    pub query_params: Vec<String>,
    /// Handler Fitz. Tiene que ser `Value::Function` — el evaluator
    /// valida esto en registro.
    pub handler: Value,
    /// Nombre del handler para mensajes de error/log.
    pub handler_name: String,
    /// Tipos declarados de los parámetros del handler, en orden. Cada
    /// tupla es `(nombre, head_name_sin_genericos_ni_nullable,
    /// is_nullable)`. `head_name` sirve para `coerce_path_param`
    /// (Int/Float/Str/Bool); `is_nullable` sirve para query params
    /// (un `Int?` faltante en la query queda como `Null` en vez de
    /// 400).
    pub param_types: Vec<(String, Option<String>, bool)>,
    /// Si el handler declara un parámetro que no es path param, lo
    /// tratamos como body. Acá guardamos su nombre y, opcionalmente,
    /// el `Value::Type` declarado (resuelto del env en momento de
    /// registro). Si el tipo no está declarado, deserializamos el
    /// JSON como `Value` libre (Map/List/primitivos).
    ///
    /// Máximo un body por handler. La validación de cuántos hay y
    /// que sean compatibles la hace el evaluator durante el registro.
    pub body_param: Option<BodyParam>,
    /// Headers declarados con `@header(name="X")` sobre el handler
    /// (Fase 7.6). Vacío si el handler no declara ninguno. Cada
    /// entry mapea un nombre HTTP a un param Fitz del handler.
    pub headers: Vec<HeaderSpec>,
    /// TypeExpr completos de los parámetros del handler, en orden.
    /// Aditivo a `param_types` (que carga solo el `head_name` sin
    /// genéricos ni nullables, suficiente para el dispatch). Acá
    /// guardamos el `TypeExpr` íntegro para que la generación de
    /// schema OpenAPI (Fase 7.1) pueda emitir `List<Int>`, `Int?`,
    /// `Result<User>`, etc. sin perder estructura.
    pub param_type_exprs: Vec<(String, Option<TypeExpr>)>,
    /// Return type declarado del handler (si lo declaró). Lo usa el
    /// generador OpenAPI para distinguir `200` solo vs `200` + `500`
    /// (handlers que devuelven `Result<T>` mapean a ambos status).
    /// Sin anotación → `None` y el generador trata el response como
    /// "any" (`200` con schema vacío).
    pub return_type_expr: Option<TypeExpr>,
    /// Middlewares declarados con `@middleware(fn)` apilados antes del
    /// decorator de ruta (mini-fase MW.1). El orden del Vec es el de
    /// aplicación: el primero corre primero, el último justo antes del
    /// handler. Cada uno se invoca con un único arg `Request`. Retornos
    /// soportados (gate-only): `Null`/sin return → continúa la cadena;
    /// `Value::HttpResponse` (vía `return <status> { ... }`) →
    /// short-circuit con ese status code. Cualquier otro tipo → 500
    /// con mensaje claro. Vacío si la ruta no tiene middlewares.
    pub middlewares: Vec<MiddlewareSpec>,
    /// Configuración CORS aplicada con `@middleware(cors(...))`
    /// (mini-fase MW.2). Vive en un slot dedicado, NO entra a la chain
    /// de `middlewares`: CORS necesita inyectar headers en la response
    /// real (no es gate-only) y registrar un handler de preflight
    /// adicional (`OPTIONS`), cosas que el modelo de middleware gate
    /// no expresa. Máximo uno por ruta — dos `cors(...)` aplicados al
    /// mismo handler es un error de registro. `Arc` para evitar clonar
    /// el config por request y para cruzar threads (preflight corre en
    /// el thread tokio).
    pub cors: Option<std::sync::Arc<CorsConfig>>,
}

/// Una entrada del stack de middlewares de una ruta (mini-fase MW.1).
/// El `handler` viene resuelto a `Value::Function` desde el env del
/// importer durante el registro de la ruta; el evaluator garantiza
/// que el value sea callable (clon barato del `Rc` adentro). El
/// `name` es el identificador con el que el usuario lo referenció
/// en `@middleware(...)`, solo para mensajes de error/log.
/// Mini-tanda Mw.next — kind del middleware. Determinado por la
/// aridad del Value::Function en `collect_middlewares`:
///
///   - **Pre (1 arg)**: gate-only clásico. Recibe `Request`, devuelve
///     `null` para continuar o `Response` para short-circuit. NO ve
///     la response final.
///   - **Post (2 args)**: post-process. Corre DESPUÉS del handler.
///     Recibe `(Request, Response)`, devuelve `Response`. Permite
///     agregar headers, modificar el body, etc. Si varios post-mws
///     existen, corren en orden INVERSO al de registración (semántica
///     de wrap: el último registrado es el más interno, ve la
///     response primero).
///
/// Decisión vs wrap-style con `next` callable: el modelo wrap exigiría
/// construir un `next` callable Fitz desde Rust en runtime (refactor de
/// 6-8h con Value variant nuevo). Post-process cubre 80% de los casos
/// reales (timing, headers, logging post-handler) y es self-contained.
/// El caso restante — wrap puro para catch panics o pre+post enlazado
/// en una sola fn — queda como sub-paso futuro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewareKind {
    /// 1 arg, gate-only: `fn mw(req: Request) -> Response?`. Null
    /// → continúa la chain, Response → short-circuit.
    Pre,
    /// 2 args post-process: `fn mw(req: Request, resp: Response) -> Response`.
    /// Corre DESPUÉS del handler.
    Post,
    /// Mini-tanda Mw-Wrap — 2 args wrap-style:
    /// `fn mw(req: Request, next: Fn() -> Response) -> Response`.
    /// El middleware controla la invocación del handler con `next()`.
    /// Habilita timing, observability, response wrapping, decisión
    /// condicional de continuar la chain.
    Wrap,
}

#[derive(Debug, Clone)]
pub struct MiddlewareSpec {
    pub name: String,
    pub handler: Value,
    pub kind: MiddlewareKind,
}

/// Mini-fase Q.3 + mini-tanda HTTP-Cors: la política de
/// `Access-Control-Allow-Origin` admite tres modos: literal (valor
/// fijo, como hasta MW.2), set de orígenes permitidos (echo si está
/// en la lista), y echo sin filtro (acepta cualquier Origin recibido).
///
///       - `Literal("*")` o `Literal("https://x.com")` → emite el valor
///         tal cual (modo previo).
///       - `Set(["https://a.com", "https://b.com"])` → si el header
///         `Origin` del request matchea uno de la lista, emite **ese**
///         valor (no la lista entera). Si no matchea, NO emite el header
///         (el browser rechaza la response — comportamiento estándar de
///         CORS estricto). Útil cuando se necesitan credenciales (cookies/
///         Authorization) sobre múltiples frontends: `Allow-Origin: *`
///         incompatible con credentials, echo del Origin específico sí.
///       - `Echo` → eco del Origin recibido sin filtro. Equivalente a
///         escribir `Set(...)` con todos los frontends posibles. Útil
///         para dev local donde no se conoce la lista a priori. Si la
///         request NO tiene header `Origin`, NO emite el header
///         (mismo comportamiento que Set sin match).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowOrigin {
    /// Valor literal, emitido idéntico en cada response.
    Literal(String),
    /// Set de orígenes permitidos. El runtime echo si el `Origin` del
    /// request está en la lista.
    Set(Vec<String>),
    /// Mini-tanda HTTP-Cors — echo del Origin recibido sin filtro.
    /// Construido vía `allow_origin: "echo"` en el config Map.
    Echo,
}

impl AllowOrigin {
    /// Computa el valor a emitir en `Access-Control-Allow-Origin`
    /// dado el `Origin` del request (si lo hay):
    ///       - Literal → siempre el valor, sin importar el request.
    ///       - Set → el valor del request si está en la lista; `None`
    ///         si no.
    ///       - Echo → el valor del request tal cual (sin filtro);
    ///         `None` si no llega Origin header.
    pub fn resolve(&self, request_origin: Option<&str>) -> Option<String> {
        match self {
            AllowOrigin::Literal(s) => Some(s.clone()),
            AllowOrigin::Set(set) => {
                let req = request_origin?;
                if set.iter().any(|s| s == req) {
                    Some(req.to_string())
                } else {
                    None
                }
            }
            AllowOrigin::Echo => request_origin.map(|s| s.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorsConfig {
    pub allow_origin: AllowOrigin,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
    pub max_age: Option<i64>,
}

impl CorsConfig {
    /// Construye un CorsConfig "default" pensado para uso de browser
    /// frontend SPA: origin "*", métodos comunes, headers `content-type`
    /// + `authorization`. Casos más restrictivos exigen kwargs explícitos.
    pub fn permissive_default() -> Self {
        CorsConfig {
            allow_origin: AllowOrigin::Literal("*".to_string()),
            allow_methods: vec![
                "GET".into(),
                "POST".into(),
                "PUT".into(),
                "DELETE".into(),
                "OPTIONS".into(),
            ],
            allow_headers: vec!["content-type".into(), "authorization".into()],
            max_age: None,
        }
    }

    /// Lista de headers HTTP que el server emite con una response
    /// CORS (real o preflight), resuelta contra el `Origin` del
    /// request. Si la política es `Set` y el origin no está permitido,
    /// el header `Access-Control-Allow-Origin` se OMITE (el browser
    /// rechaza la response, comportamiento CORS estricto correcto).
    /// El resto de los headers (methods/headers/max_age) sí se emiten.
    pub fn response_headers(&self, request_origin: Option<&str>) -> Vec<(String, String)> {
        let mut out = Vec::with_capacity(4);
        if let Some(origin) = self.allow_origin.resolve(request_origin) {
            out.push(("access-control-allow-origin".into(), origin));
        }
        out.push((
            "access-control-allow-methods".into(),
            self.allow_methods.join(", "),
        ));
        out.push((
            "access-control-allow-headers".into(),
            self.allow_headers.join(", "),
        ));
        if let Some(age) = self.max_age {
            out.push(("access-control-max-age".into(), age.to_string()));
        }
        out
    }
}

/// Especificación de un header declarado con `@header(name="X")`
/// sobre un handler (Fase 7.6). El `http_name` es el nombre HTTP
/// canónico declarado por el usuario; `param_name` es el nombre del
/// parámetro Fitz al que se bindea (derivado por convención:
/// lowercase + `-` → `_`). `is_nullable`: si el param Fitz se declaró
/// como `Str?`, el header es opcional (falta → `Null`); si no, es
/// obligatorio (falta → 400).
#[derive(Debug, Clone)]
pub struct HeaderSpec {
    pub http_name: String,
    pub param_name: String,
    pub is_nullable: bool,
}

/// Descripción del parámetro body de un handler: su nombre (para
/// armar args en el orden correcto) y el `Value::Type` esperado, si
/// el usuario lo declaró. Sin tipo declarado, deserializamos como
/// `Value` libre (forma flexible — útil para webhooks o APIs sin
/// schema).
#[derive(Debug, Clone)]
pub struct BodyParam {
    pub name: String,
    /// `Some(Value::Type{...})` si el usuario declaró un tipo custom.
    /// `None` si el parámetro no tiene anotación o si la anotación es
    /// un primitivo (`Int`, `Str`, etc. — soportamos eso también).
    pub declared_type: Option<Value>,
    /// Cuando `declared_type` es `None`, este campo guarda el nombre
    /// del tipo (si lo hay) para mensajes de error. Si tampoco está
    /// declarado, `None`. Lo dejamos como metadata estructural aunque
    /// el lectura actual sea solo por `Debug`.
    #[allow(dead_code)]
    pub declared_type_name: Option<String>,
}

/// Configuración del servidor que un `@server(...)` pudo haber
/// declarado en el programa. Si está en `None`, se usan defaults
/// (127.0.0.1:3000, docs habilitados). Solo se admite un `@server`
/// por programa — la unicidad la enforcea el evaluator durante el
/// registro.
///
/// `enable_docs` (Fase 7.4): cuando `false`, el server NO
/// autoregistra `/openapi.json` ni `/docs`. Default: `true` —
/// el camino feliz entrega docs sin tocar nada. Opt-out con
/// `@server(docs=false)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub enable_docs: bool,
    /// Mini-fase Q.2: override de `info.version` del schema OpenAPI
    /// vía `@server(api_version="1.2.3")`. `None` → el schema usa el
    /// default `"0.1.0"`. Cuando se setea, lo lee `serve()` al
    /// pre-computar el schema y lo pasa a `generate_openapi_with_version`.
    pub api_version: Option<String>,
}

impl ServerConfig {
    /// Defaults aplicados cuando no hay `@server` en el programa.
    pub fn default_addr() -> Self {
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 3000,
            enable_docs: true,
            api_version: None,
        }
    }

    /// Traduce a `SocketAddr`. Falla si el host no parsea como IP
    /// numérica (no resolvemos DNS — para evitar surpresas con un
    /// host literal que no es IP).
    pub fn to_socket_addr(&self) -> Result<std::net::SocketAddr, String> {
        let ip: std::net::IpAddr = self
            .host
            .parse()
            .map_err(|_| format!("host '{}' no es una IP válida (esperado IPv4/IPv6 literal)", self.host))?;
        Ok(std::net::SocketAddr::new(ip, self.port))
    }
}

/// Acumulador de rutas registradas durante `eval`. Construido por
/// `main.rs` antes de evaluar; consultado después para decidir si
/// arrancar el server.
#[derive(Debug, Default)]
pub struct HttpRegistry {
    pub routes: Vec<RouteSpec>,
    /// Configuración del server declarada con `@server(...)`. `None`
    /// si el programa no la declaró — el caller (main.rs) aplica
    /// `ServerConfig::default_addr()`.
    pub server_config: Option<ServerConfig>,
}

impl HttpRegistry {
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            server_config: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        // El registry está "vacío" si no tiene rutas. Un `@server`
        // sin rutas no levanta nada (no hay endpoints a servir);
        // ignorarlo es lo más útil.
        self.routes.is_empty()
    }

    pub fn push(&mut self, route: RouteSpec) {
        self.routes.push(route);
    }

    /// Devuelve el config explícito o el default. Útil para `main.rs`.
    pub fn resolved_config(&self) -> ServerConfig {
        self.server_config
            .clone()
            .unwrap_or_else(ServerConfig::default_addr)
    }
}

// thread_local: el evaluador se entera de si hay un registry activo
// sin pasarlo como parámetro por todos lados. Mismo patrón que el
// loader de módulos en 3.5. `None` → estamos corriendo en un contexto
// sin HTTP (REPL, eval embebido, tests sin server) y los decorators
// dan error explícito.
thread_local! {
    static HTTP_REGISTRY: RefCell<Option<HttpRegistry>> = const { RefCell::new(None) };
}

/// Instala un registry vacío para el thread actual durante la
/// duración del closure. Al terminar lo devuelve. Si el closure
/// retorna `Err`, el registry se descarta junto con el resto del
/// estado. Pensado para `main.rs`: arma, evalúa, recibe el registry,
/// decide si arrancar el server.
pub fn with_active_registry<F, T>(f: F) -> (T, HttpRegistry)
where
    F: FnOnce() -> T,
{
    HTTP_REGISTRY.with(|cell| {
        // Guardamos el registry previo (típicamente `None` — el caso
        // anidado existe solo para tests). Después de `f()` lo
        // restauramos textual, sin reemplazarlo por `HttpRegistry::new()`
        // por error.
        let prev = cell.borrow_mut().take();
        *cell.borrow_mut() = Some(HttpRegistry::new());
        let out = f();
        let registry = cell
            .borrow_mut()
            .take()
            .expect("with_active_registry instaló un registry — debería estar presente");
        *cell.borrow_mut() = prev;
        (out, registry)
    })
}

/// Variante async de `with_active_registry` (Fase 6.4). Misma semántica
/// pero acepta una closure que devuelve un `Future`, para uso desde
/// código async (handlers, tests con `#[tokio::test]`).
///
/// **Invariante de borrow**: NO mantenemos `cell.borrow_mut()` cross
/// await — los borrows se toman/sueltan al entrar y al salir de cada
/// paso atómico. Si la closure paniquea, el guard sigue restaurando
/// el registry previo en el `Drop` implícito (mismo patrón que la
/// versión sync, vía panics propagados después del setup).
///
/// `dead_code` allow: solo lo usan tests por ahora (los handlers HTTP
/// reales aterrizan en 6.5 cuando se elimine el bridge mpsc).
#[allow(dead_code)]
pub async fn with_active_registry_async<F, Fut, T>(f: F) -> (T, HttpRegistry)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let prev = HTTP_REGISTRY.with(|cell| {
        let prev = cell.borrow_mut().take();
        *cell.borrow_mut() = Some(HttpRegistry::new());
        prev
    });
    let out = f().await;
    let registry = HTTP_REGISTRY.with(|cell| {
        let registry = cell
            .borrow_mut()
            .take()
            .expect("with_active_registry_async instaló un registry — debería estar presente");
        *cell.borrow_mut() = prev;
        registry
    });
    (out, registry)
}

/// `true` si hay un registry HTTP activo en el thread actual. El
/// evaluator lo consulta antes de procesar un decorator HTTP: si no
/// hay, sigue cortando con error explícito.
pub fn has_active_registry() -> bool {
    HTTP_REGISTRY.with(|cell| cell.borrow().is_some())
}

/// Empuja una ruta al registry activo. Pánico si no hay uno — el
/// llamador debe haber chequeado con `has_active_registry()` o estar
/// adentro de `with_active_registry`.
pub fn push_route(route: RouteSpec) {
    HTTP_REGISTRY.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let reg = borrow
            .as_mut()
            .expect("push_route llamado sin registry activo");
        reg.push(route);
    });
}

/// Setea la `ServerConfig` del registry activo. Falla si ya había
/// una (mantiene la unicidad de `@server`). Devuelve `Err(())` y el
/// evaluator emite un error explícito.
pub fn set_server_config(config: ServerConfig) -> Result<(), ServerConfig> {
    HTTP_REGISTRY.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let reg = borrow
            .as_mut()
            .expect("set_server_config llamado sin registry activo");
        if let Some(existing) = &reg.server_config {
            return Err(existing.clone());
        }
        reg.server_config = Some(config);
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Path: del decorator a la sintaxis de axum
// ---------------------------------------------------------------------------

/// Resultado de extraer un path declarado en un decorator HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathTemplate {
    /// Path en formato axum: `/users/{id}`, `/`, `/users`. Lo que viene
    /// después de un `?` en el template original NO entra acá — vive
    /// adentro de `query_params`. Axum hace su routing solo con esto.
    pub path: String,
    /// Nombres de los path params en el orden de aparición.
    pub params: Vec<String>,
    /// Nombres de los query params declarados en el template. Cada uno
    /// proviene de un `?key={name}&...` después del path. Por ahora
    /// exigimos que la key del query y el nombre del param Fitz
    /// coincidan (`?limit={limit}`, no `?l={limit}`). El orden de
    /// `query_params` es el de aparición en el template.
    pub query_params: Vec<String>,
}

/// Errores al normalizar el path de un decorator. Mensajes en español
/// para que vayan directo al usuario.
#[derive(Debug, PartialEq, Eq)]
pub enum PathError {
    /// El primer arg del decorator no es un literal string.
    NotAStringLiteral,
    /// El path no arranca con `/`.
    MustStartWithSlash,
    /// Un segmento de interpolación incluyó algo que no es un
    /// identificador simple (`{user.id}`, `{42}`, etc.).
    UnsupportedInterpolation(String),
    /// Algún path param se repitió (`/a/{x}/b/{x}`).
    DuplicateParam(String),
    /// Un query param declarado tiene una key distinta del nombre del
    /// param (`?l={limit}`). Hoy exigimos que coincidan.
    QueryKeyNameMismatch { key: String, name: String },
    /// El template del query no respeta `key={name}` con identificador
    /// simple — ej. `?{limit}`, `?limit=`, `?limit={x.y}`, `?=v`.
    MalformedQueryTemplate(String),
}

impl PathError {
    pub fn message(&self) -> String {
        match self {
            PathError::NotAStringLiteral => {
                "el path de un decorator HTTP debe ser un string literal \
                 (`@get(\"/users\")`)"
                    .to_string()
            }
            PathError::MustStartWithSlash => {
                "el path de un decorator HTTP debe arrancar con '/'".to_string()
            }
            PathError::UnsupportedInterpolation(what) => format!(
                "path param '{{{}}}': solo se admiten identificadores simples \
                 como '{{id}}', no expresiones",
                what
            ),
            PathError::DuplicateParam(name) => format!(
                "path param '{{{}}}' aparece más de una vez en el path",
                name
            ),
            PathError::QueryKeyNameMismatch { key, name } => format!(
                "query param `?{key}={{{name}}}`: la key y el nombre del \
                 param deben coincidir — usá `?{name}={{{name}}}` o renombrá \
                 el parámetro del handler"
            ),
            PathError::MalformedQueryTemplate(t) => format!(
                "template de query mal formado adentro del path: `?{t}` — \
                 esperado `?key={{name}}&otra_key={{otro_name}}` con \
                 identificadores simples"
            ),
        }
    }
}

/// Toma la expresión que el parser dejó como primer arg de un
/// decorator HTTP y la convierte a un `PathTemplate`. Acepta dos
/// formas:
///
///  - `Expr::Str(s, _)`: path sin params. Ej: `"/"`, `"/users"`.
///  - `Expr::StrInterp(parts, _)`: path con params. Cada `StrPart::Expr`
///    tiene que ser un `Ident` simple (`{id}`). Cualquier otra cosa
///    es error.
///
/// Cualquier otra forma de expresión → `PathError::NotAStringLiteral`.
pub fn parse_path_template(expr: &Expr) -> Result<PathTemplate, PathError> {
    use crate::ast::StrPart;

    // Primera pasada: reconstruir el texto del path canonicalizado y
    // recolectar todos los `{name}` en orden (sin distinguir path vs
    // query todavía). El `?` que separa path de query queda como
    // carácter literal en `buf` — lo dividimos abajo.
    let (full, all_params): (String, Vec<String>) = match expr {
        Expr::Str(s, _) => (s.clone(), Vec::new()),
        Expr::StrInterp(parts, _) => {
            let mut buf = String::new();
            let mut params = Vec::new();
            for part in parts {
                match part {
                    StrPart::Lit(s) => buf.push_str(s),
                    StrPart::Expr(Expr::Ident(name, _), _) => {
                        if params.contains(name) {
                            return Err(PathError::DuplicateParam(name.clone()));
                        }
                        params.push(name.clone());
                        buf.push('{');
                        buf.push_str(name);
                        buf.push('}');
                    }
                    StrPart::Expr(other, _) => {
                        return Err(PathError::UnsupportedInterpolation(
                            format!("{:?}", other),
                        ));
                    }
                }
            }
            (buf, params)
        }
        _ => return Err(PathError::NotAStringLiteral),
    };

    if !full.starts_with('/') {
        return Err(PathError::MustStartWithSlash);
    }

    // Separar path de query template por el primer `?`. Si no hay,
    // toda la cadena es path y `query_params` queda vacío.
    let (path, query_template) = match full.find('?') {
        Some(idx) => (full[..idx].to_string(), Some(&full[idx + 1..])),
        None => (full, None),
    };

    // Para distinguir path_params de query_params: los que aparecen
    // adentro del path quedan en `path_params`; los que aparecen
    // adentro del query template (con su key) van a `query_params`.
    let mut path_params: Vec<String> = Vec::new();
    let mut query_params: Vec<String> = Vec::new();

    // Re-escanear el path canonicalizado para extraer los `{name}` que
    // están adentro de él (sin parsear de cero — solo buscamos
    // `{ident}` entre llaves para ordenar correcto).
    extract_brace_idents_into(&path, &mut path_params);

    // Parsear el query template si existe.
    if let Some(q) = query_template {
        // Formato: `key={name}&otra={otra}` con cada pair separado por
        // `&`. Validar que cada pair tenga `key={name}` con key
        // identificador simple y `{name}` también identificador simple,
        // y que key == name.
        if q.is_empty() {
            return Err(PathError::MalformedQueryTemplate(String::new()));
        }
        for pair in q.split('&') {
            let Some(eq_idx) = pair.find('=') else {
                return Err(PathError::MalformedQueryTemplate(pair.to_string()));
            };
            let key = &pair[..eq_idx];
            let value = &pair[eq_idx + 1..];
            if key.is_empty() || !is_simple_ident(key) {
                return Err(PathError::MalformedQueryTemplate(pair.to_string()));
            }
            // El value tiene que ser exactamente `{name}` (un brace
            // pair con un identificador adentro). Cualquier otra cosa
            // (literal, expr, vacío) no se soporta.
            if !(value.starts_with('{') && value.ends_with('}') && value.len() >= 3) {
                return Err(PathError::MalformedQueryTemplate(pair.to_string()));
            }
            let name = &value[1..value.len() - 1];
            if !is_simple_ident(name) {
                return Err(PathError::MalformedQueryTemplate(pair.to_string()));
            }
            if key != name {
                return Err(PathError::QueryKeyNameMismatch {
                    key: key.to_string(),
                    name: name.to_string(),
                });
            }
            if path_params.contains(&name.to_string())
                || query_params.contains(&name.to_string())
            {
                return Err(PathError::DuplicateParam(name.to_string()));
            }
            query_params.push(name.to_string());
        }
    }

    // Sanity check: la suma path + query debería matchear `all_params`
    // (todos los `{name}` que extrajimos en la primera pasada). Si no,
    // hay algo raro en el path (ej. `{name}` adentro del query value
    // sin ser exactamente `={name}`). El parser de query ya lo cazaría
    // pero validamos por defensa.
    let _ = all_params;

    Ok(PathTemplate {
        path,
        params: path_params,
        query_params,
    })
}

/// Extrae nombres entre `{...}` en un path canonicalizado, en orden de
/// aparición, y los empuja a `out`. Asume que el path ya fue
/// reconstruido por `parse_path_template` (las llaves vienen siempre
/// alrededor de identificadores simples).
fn extract_brace_idents_into(path: &str, out: &mut Vec<String>) {
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = path[i + 1..].find('}') {
                let name = &path[i + 1..i + 1 + end];
                out.push(name.to_string());
                i = i + 1 + end + 1;
                continue;
            }
        }
        i += 1;
    }
}

/// Identificador "simple" para keys y param names en query templates:
/// ASCII letras/digits/underscore, primer char no-digit. No usamos
/// `char::is_alphanumeric` para evitar aceptar unicode (Fitz lo
/// rechaza también en idents del lexer).
fn is_simple_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Serialización Value → JSON
// ---------------------------------------------------------------------------

/// Respuesta destilada de un handler: status code + cuerpo serializado.
/// El handler Fitz devuelve un `Value`; esta función decide cómo se
/// traduce a HTTP. La conversión es total (cualquier `Value` produce
/// un `HandlerOutcome`), pero algunos tipos no serializables (Function,
/// Type, Module, Range) generan 500 con un mensaje claro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerOutcome {
    pub status: u16,
    /// JSON ya serializado, listo para mandar como body. Vacío para
    /// 204 (no usado en 4.2; reservado).
    pub body: String,
    /// Content-type del body. Hoy siempre `application/json`; queda
    /// preparado para `text/plain` u otros cuando los necesitemos.
    pub content_type: &'static str,
    /// Headers extra a emitir junto con la response (mini-fase MW.2).
    /// Se popula al final de `handle_task` cuando la ruta tiene
    /// `RouteSpec.cors`: la inyección de `Access-Control-Allow-*`
    /// vive acá. Vacío para responses normales sin CORS.
    pub extra_headers: Vec<(String, String)>,
}

impl HandlerOutcome {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        HandlerOutcome {
            status,
            body: body.to_string(),
            content_type: "application/json",
            extra_headers: Vec::new(),
        }
    }

    /// Atajo para errores del runtime que el handler nunca tendría
    /// que ver: tipo no serializable, decorator mal usado, etc.
    pub fn internal_error(msg: impl Into<String>) -> Self {
        let body = serde_json::json!({ "error": msg.into() });
        HandlerOutcome::json(500, body)
    }
}

/// Convierte el resultado de un handler Fitz a un `HandlerOutcome`.
///
/// Reglas:
///   - `Value::Result(Ok(v))`  → status 200, body = `v` serializado.
///   - `Value::Result(Err(e))` → mini-tanda HTTP-Err: si `e` es
///     `Value::Instance` con field `status: Int`, usa ese status code
///     y el body es la Instance serializada (intacta — el usuario
///     decide el shape). Si no tiene `status`, fallback a 500 con
///     `{"error": e}` (comportamiento histórico).
///   - Cualquier otro `Value`  → status 200, body = ese valor
///     serializado directo (sin envolver). Esto permite handlers que
///     no usan `Result` y devuelven `Str`, `Int`, `Instance`, etc.
///   - Tipos no serializables (Function, Builtin, Type, Module, Range)
///     → status 500, `{"error": "valor no serializable: <tipo>"}`.
pub fn value_to_outcome(value: &Value) -> HandlerOutcome {
    // Status code custom (spec): el handler hizo `return 401 { ... }`
    // y el evaluator emitió `Value::HttpResponse`. Mapeo directo: el
    // status va al outcome, el body (si existe) se serializa con las
    // mismas reglas que cualquier Value. Body ausente → JSON null
    // (HTTP 204 No Content todavía no está implementado, hoy el
    // parser exige body explícito).
    if let Value::HttpResponse { status, body } = value {
        let payload_json = match body {
            Some(b) => match value_to_json(b) {
                Ok(j) => j,
                Err(msg) => return HandlerOutcome::internal_error(msg),
            },
            None => serde_json::Value::Null,
        };
        return HandlerOutcome::json(*status, payload_json);
    }

    // Result auto-handling: peel one layer. El inner se serializa con
    // las mismas reglas que cualquier otro Value.
    let (status, payload) = match value {
        Value::Result(ResultVariant::Ok(inner)) => (200, inner.as_ref()),
        Value::Result(ResultVariant::Err(inner)) => {
            // Mini-tanda HTTP-Err — convención: si el Err lleva una
            // `Instance` con field `status: Int`, usar ese status
            // (e.g. `Err(ApiErr { status: 404, message: "..." })`).
            // El body se serializa íntegro — el usuario decide el
            // shape final. Sin field `status`, fallback al 500 con
            // `{"error": e}` (comportamiento histórico).
            if let Value::Instance { fields, .. } = inner.as_ref() {
                let status_opt = {
                    let g = fields.lock();
                    g.iter()
                        .find(|(k, _)| k == "status")
                        .and_then(|(_, v)| if let Value::Int(n) = v {
                            Some(*n)
                        } else {
                            None
                        })
                };
                if let Some(s) = status_opt {
                    // Mini-tanda HC.1 — status válido `[100, 1000)`
                    // matchea axum y la spec HTTP. Si el usuario
                    // provee un status fuera de rango, ya no caemos
                    // silenciosamente a 500 — emitimos 500 con un
                    // mensaje explícito citando el valor inválido.
                    // Esto destraba debugging cuando el usuario hace
                    // `Err({ status: 999 })` por typo o convención
                    // distinta.
                    if (100..1000).contains(&s) {
                        return match value_to_json(inner) {
                            Ok(j) => HandlerOutcome::json(s as u16, j),
                            Err(msg) => HandlerOutcome::internal_error(msg),
                        };
                    } else {
                        return HandlerOutcome::internal_error(format!(
                            "status code inválido en Err: {} (debe estar en 100..1000)",
                            s
                        ));
                    }
                }
            }
            return match value_to_json(inner) {
                Ok(j) => HandlerOutcome::json(500, serde_json::json!({ "error": j })),
                Err(msg) => HandlerOutcome::internal_error(msg),
            };
        }
        other => (200, other),
    };

    match value_to_json(payload) {
        Ok(j) => HandlerOutcome::json(status, j),
        Err(msg) => HandlerOutcome::internal_error(msg),
    }
}

/// Serializa un `Value` a `serde_json::Value`. Es total para los tipos
/// "de datos" del lenguaje; tipos opacos al usuario (Function, Type,
/// Module, Range, Builtin) devuelven `Err` con mensaje al estilo
/// "valor no serializable: <tipo>".
///
/// Importante: `Result` NO se trata especialmente acá — esa decisión
/// vive en `value_to_outcome` (que mapea Ok→200, Err→500). Si por
/// alguna razón llega un `Result` anidado (un handler que devuelve
/// `Ok(Ok(x))`), serializamos como objeto `{"Ok": ...}` o `{"Err": ...}`
/// para no perder información.
pub fn value_to_json(value: &Value) -> Result<serde_json::Value, String> {
    use serde_json::Value as J;

    Ok(match value {
        Value::Int(n) => J::from(*n),
        Value::Float(f) => {
            // serde_json no admite NaN/Inf — los rechazamos explícito.
            serde_json::Number::from_f64(*f)
                .map(J::Number)
                .ok_or_else(|| format!("float no serializable como JSON: {}", f))?
        }
        Value::Str(s) => J::String(s.clone()),
        Value::Bool(b) => J::Bool(*b),
        Value::Null => J::Null,
        // Mini-tanda Bytes + quick win F13 bundle — Bytes se
        // serializa como base64 string (estándar de facto para
        // bytes en JSON). Antes se emitía como array de Int (cada
        // byte un i64), que funciona pero infla la representación
        // ~4x y es no-estándar. Decodificación implementada manual
        // (alfabeto RFC 4648 sin padding, sin '+' / '/' problemáticos
        // — se usa el `base64-standard`). Para mantener la deuda de
        // dep ligera, no agregamos la crate `base64`; encodeamos
        // inline.
        Value::Bytes(bs) => J::String(b64_encode_standard(bs)),

        Value::List(items) => {
            let mut out = Vec::with_capacity(items.lock().len());
            for v in items.lock().iter() {
                out.push(value_to_json(v)?);
            }
            J::Array(out)
        }

        // Tuples (mini-tanda T): serializamos como Array JSON (no hay
        // tuple type en JSON). Pierde la distinción tuple/list pero
        // es lo razonable para handlers HTTP.
        Value::Tuple(items) => {
            let mut out = Vec::with_capacity(items.len());
            for v in items {
                out.push(value_to_json(v)?);
            }
            J::Array(out)
        }

        Value::Map(pairs) => {
            let mut out = serde_json::Map::new();
            for (k, v) in pairs.lock().iter() {
                let key = match k {
                    Value::Str(s) => s.clone(),
                    other => {
                        return Err(format!(
                            "claves de Map en JSON deben ser Str, se encontró {}",
                            other.type_name()
                        ));
                    }
                };
                out.insert(key, value_to_json(v)?);
            }
            J::Object(out)
        }

        Value::Instance { fields, .. } => {
            let mut out = serde_json::Map::new();
            for (name, v) in fields.lock().iter() {
                out.insert(name.clone(), value_to_json(v)?);
            }
            J::Object(out)
        }

        Value::Result(ResultVariant::Ok(inner)) => {
            // Result anidado (poco común). Lo etiquetamos para no perder
            // la distinción Ok/Err.
            serde_json::json!({ "Ok": value_to_json(inner)? })
        }
        Value::Result(ResultVariant::Err(inner)) => {
            serde_json::json!({ "Err": value_to_json(inner)? })
        }

        // Tipos opacos: no tienen representación JSON sensata.
        Value::Function { .. }
        | Value::Builtin { .. }
        | Value::Type { .. }
        | Value::Module { .. }
        | Value::Range { .. } => {
            return Err(format!(
                "valor no serializable a JSON: {}",
                value.type_name(),
            ));
        }
        // HttpResponse no se serializa directo — vive en
        // `value_to_outcome` (intercepta antes de llegar acá). Si
        // alguien lo serializa fuera de context HTTP, es un bug del
        // codegen/runtime, no del usuario.
        Value::HttpResponse { .. } => {
            return Err(
                "HttpResponse no es serializable a JSON fuera de un handler HTTP".to_string(),
            );
        }
        // Future pendiente: no es serializable. Si llega un Future a un
        // response, el usuario olvidó `.await`. El checker 6.2 lo
        // detecta estáticamente para handlers anotados; este path es
        // defensivo (handlers sin return_type, Future generado por
        // otro camino).
        Value::Future(_) => {
            return Err(
                "Future pendiente no es serializable — falta `.await` en algún lado del handler".to_string(),
            );
        }
        // Mini-tanda Mw-Wrap — `Value::NativeFn` es el callable
        // `next` que se pasa a wrap-style middlewares. Si llega al
        // serializer, el handler lo devolvió por error.
        Value::NativeFn(_) => {
            return Err(
                "función nativa no es serializable — `next` solo se puede invocar, no devolver".to_string(),
            );
        }
        // CorsConfig (MW.2): opaco, no se serializa. Si llega acá,
        // es un bug del registro: el evaluator debió usarlo como
        // arg de `@middleware(cors(...))` y guardarlo en el slot
        // `RouteSpec.cors`, no como valor de retorno del handler.
        Value::CorsConfig(_) => {
            return Err(
                "CorsConfig no es serializable — se usa como argumento de `@middleware(cors(...))`, no como valor".to_string(),
            );
        }
        // PyObject (Fase 8.1+, feature `python`): opaco. El handler
        // debería extraer primitivos (8.1) o usar marshaling explícito
        // (8.2+) antes de devolver. Si llega un PyObject crudo, el
        // usuario olvidó coercionar.
        #[cfg(feature = "python")]
        Value::PyObject(_) => {
            return Err(
                "PyObject no es serializable a JSON — convertí el valor Python a un tipo Fitz antes de devolverlo".to_string(),
            );
        }
    })
}

// ---------------------------------------------------------------------------
// JSON → Value (deserialización del body)
// ---------------------------------------------------------------------------

/// Convierte un `serde_json::Value` a un `Value` de Fitz "libre" —
/// sin chequear contra un schema. Útil cuando el handler declara un
/// body sin anotación de tipo, o con un tipo que no es `type` custom.
///
/// Mapeo:
///   - números enteros → `Int`; con parte fraccional → `Float`.
///   - strings → `Str`. Bools → `Bool`. null → `Null`.
///   - arrays → `List` con cada elemento traducido recursivo.
///   - objects → `Map` con claves `Str` (mantiene orden de inserción
///     del parser de serde_json).
///
/// No falla nunca: cualquier JSON válido produce un `Value`. La
/// validación contra un `type` específico se hace en
/// `json_to_instance`.
pub fn json_to_value(json: &serde_json::Value) -> Value {
    use crate::value::shared;
    use serde_json::Value as J;

    match json {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                // u64 que no entra en i64. Lo guardamos como Float
                // para no perder. Mejor opción hasta que tengamos
                // BigInt o u64 en el lenguaje.
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        J::String(s) => Value::Str(s.clone()),
        J::Array(items) => {
            let vs: Vec<Value> = items.iter().map(json_to_value).collect();
            Value::List(shared(vs))
        }
        J::Object(obj) => {
            let pairs: Vec<(Value, Value)> = obj
                .iter()
                .map(|(k, v)| (Value::Str(k.clone()), json_to_value(v)))
                .collect();
            Value::Map(shared(pairs))
        }
    }
}

/// Convierte un `serde_json::Value` que se espera sea un objeto a un
/// `Value::Instance` validado contra los campos del `type` declarado.
///
/// Reglas (mismas que `StructLit` en el evaluador):
///   - Objeto JSON requerido — array, string o número → error.
///   - Cada campo del type debe estar presente, o tener default, o
///     ser nullable. Campo faltante sin default ni nullable → error.
///   - Campos extra (en el JSON pero no en el type) → error explícito.
///   - El valor de cada campo se convierte recursivamente con
///     `json_to_value` (sin validación adicional contra el tipo
///     declarado del campo — la validación de tipos compuestos llega
///     con el type-checker estático de Fase 5).
///
/// Devuelve `Err(msg)` con un mensaje listo para mandar como 400.
pub fn json_to_instance(json: &serde_json::Value, type_value: &Value) -> Result<Value, String> {
    // 1. El segundo arg tiene que ser un Value::Type.
    let (type_name, fields) = match type_value {
        Value::Type { name, fields, .. } => (name.clone(), fields.clone()),
        other => {
            return Err(format!(
                "json_to_instance recibió un {} en lugar de un Type",
                other.type_name(),
            ));
        }
    };

    // 2. El JSON tiene que ser un objeto.
    let obj = match json {
        serde_json::Value::Object(map) => map,
        other => {
            return Err(format!(
                "body para '{}' debe ser un objeto JSON, se recibió {}",
                type_name,
                json_shape_name(other),
            ));
        }
    };

    // 3. Detectar campos extra antes de construir nada. Mensaje más
    //    útil acumulando todos los extra, no solo el primero.
    let field_names: std::collections::HashSet<&str> =
        fields.iter().map(|f| f.name.as_str()).collect();
    let extras: Vec<&str> = obj
        .keys()
        .filter(|k| !field_names.contains(k.as_str()))
        .map(|k| k.as_str())
        .collect();
    if !extras.is_empty() {
        return Err(format!(
            "body para '{}': campo{} no declarado{}: {}",
            type_name,
            if extras.len() == 1 { "" } else { "s" },
            if extras.len() == 1 { "" } else { "s" },
            extras.join(", "),
        ));
    }

    // 4. Recorrer los campos declarados en orden y construir los
    //    pares. Para cada uno: usar valor del JSON si está, o el
    //    default evaluado en este contexto si no, o Null si es
    //    nullable, o error.
    let mut out: Vec<(String, Value)> = Vec::with_capacity(fields.len());
    for field in &fields {
        if let Some(json_val) = obj.get(&field.name) {
            out.push((field.name.clone(), json_to_value(json_val)));
        } else if let Some(default_expr) = field.default.as_ref() {
            // Los defaults son `Expr` y se evalúan en el env de
            // instanciación. Acá no tenemos env porque el body se
            // valida lejos del eval. Para 4.3, los defaults sólo
            // funcionan si son literales constantes simples; otros
            // casos requieren más cableado. Lo manejamos en
            // `default_to_value` (helper local).
            match default_to_value(default_expr) {
                Ok(v) => out.push((field.name.clone(), v)),
                Err(_) => {
                    return Err(format!(
                        "body para '{}': el campo '{}' tiene un default que no se \
                         puede evaluar sin contexto (Fase 4.3); pasalo explícito \
                         en el body",
                        type_name, field.name,
                    ));
                }
            }
        } else if field.type_.is_nullable() {
            out.push((field.name.clone(), Value::Null));
        } else {
            return Err(format!(
                "body para '{}': falta el campo '{}'",
                type_name, field.name,
            ));
        }
    }

    Ok(Value::new_instance(type_name, out))
}

/// Nombre humano para el shape de un JSON value, útil en mensajes.
fn json_shape_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "número",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Evalúa un default literal del AST a un `Value`. Soporta literales
/// directos (los más comunes en defaults de `type`); cualquier otra
/// cosa devuelve `Err(())` y el caller decide qué hacer.
///
/// No tenemos un env aquí porque corremos del lado del runtime HTTP,
/// no adentro de eval. En 4.x, si necesitamos defaults complejos,
/// evaluamos al momento de registrar la ruta y guardamos el valor
/// resuelto.
fn default_to_value(expr: &Expr) -> Result<Value, ()> {
    match expr {
        Expr::Int(n, _) => Ok(Value::Int(*n)),
        Expr::Float(f, _) => Ok(Value::Float(*f)),
        Expr::Str(s, _) => Ok(Value::Str(s.clone())),
        Expr::Bool(b, _) => Ok(Value::Bool(*b)),
        Expr::Null(_) => Ok(Value::Null),
        _ => Err(()),
    }
}

// ---------------------------------------------------------------------------
// Path params crudos → Value con el tipo declarado
// ---------------------------------------------------------------------------

/// Convierte un path param crudo (lo que axum extrajo como `String`)
/// al `Value` que corresponda según el tipo declarado del parámetro
/// del handler. `None` como tipo → tratamos como `Str` (igual que
/// los parámetros sin anotación en general).
///
/// Tipos soportados: `Int`, `Float`, `Str`, `Bool`. Cualquier otro
/// tipo declarado en el handler para un path param es error: los
/// tipos custom no entran como path params directamente (`Int` para
/// el id; el handler reconstruye el objeto adentro si quiere).
///
/// Devuelve `Err(msg)` cuando el valor crudo no se puede convertir.
/// El runtime traduce ese error a HTTP 400.
pub fn coerce_path_param(raw: &str, declared_type: Option<&str>) -> Result<Value, String> {
    let ty = declared_type.unwrap_or("Str");
    match ty {
        "Str" => Ok(Value::Str(raw.to_string())),
        "Int" => raw
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("se esperaba Int, recibió '{}'", raw)),
        "Float" => raw
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("se esperaba Float, recibió '{}'", raw)),
        "Bool" => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            other => Err(format!(
                "se esperaba Bool ('true' o 'false'), recibió '{}'",
                other
            )),
        },
        other => Err(format!(
            "tipo '{}' no soportado para path params (usá Int/Float/Str/Bool)",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// Runtime async — axum + tokio multi-thread, evaluator directo
// ---------------------------------------------------------------------------
//
// Diseño post-F17.5 (sin bridge):
//
//   thread main = runtime tokio rt-multi-thread (block_on en `serve`)
//   ┌─────────────────────────────────────────────────────────────┐
//   │  axum::serve  →  handler async  →  handle_task(&registry,…) │
//   │                       │                                     │
//   │                       │  Arc<HttpRegistry> compartido        │
//   │                       ▼                                     │
//   │                  call_handler(...).await  (evaluator)        │
//   └─────────────────────────────────────────────────────────────┘
//
// Cada request axum se dispatchea en uno de los N workers tokio. El
// `Arc<HttpRegistry>` se clona barato a cada handler (es solo el
// refcount del Arc); los `Value::Function` adentro se invocan vía
// `handle_task` directo sobre el evaluator async. Paralelismo HTTP
// real: dos requests concurrentes corren simultáneo en workers
// distintos sobre el mismo registry. Lo que cruzaba entre threads
// en el bridge previo (path params, query, body, headers crudos)
// ahora viaja por la stack del handler.
//
// `RouteMeta` se mantiene como vista estructural (`Send + Clone`) de
// `RouteSpec` para que `build_router` arme las routes sin
// quedarse con borrows del registry — los closures de cada handler
// cierran sobre el `Arc<HttpRegistry>` por separado.

use std::collections::HashMap;

use axum::{
    body::Body,
    extract::Path as AxumPath,
    http::{HeaderValue, StatusCode},
    response::Response,
    routing::MethodRouter,
    Router,
};
use crate::evaluator::call_handler;

/// Metadata estructural de una ruta que el thread tokio necesita
/// para configurar el router. Es `Send + Sync + Clone` — no incluye
/// el handler (que vive en el thread del intérprete).
#[derive(Debug, Clone)]
pub struct RouteMeta {
    pub method: HttpMethod,
    pub path: String,
    pub has_path_params: bool,
    /// `true` si el handler declara al menos un query param. Hace que
    /// axum extraiga `Query<HashMap<String, String>>` y lo mande al
    /// intérprete. Cuando es `false`, no extraemos nada (cualquier
    /// query string de la request se ignora).
    pub has_query_params: bool,
    /// `true` si el handler declara un parámetro body. Sirve para
    /// que el handler de axum sepa si extraer el body de la request
    /// y mandarlo al intérprete. Cuando es `false`, ignoramos
    /// cualquier body recibido.
    pub expects_body: bool,
    /// Configuración CORS clonada de `RouteSpec.cors` (mini-fase MW.2).
    /// Si es `Some`, `build_router` registra un handler de preflight
    /// `OPTIONS` para el mismo path. `Arc` se clona barato y atraviesa
    /// la frontera de threads sin moverse del config compartido.
    pub cors: Option<std::sync::Arc<CorsConfig>>,
}

impl HttpRegistry {
    /// Vista del registry que el thread tokio puede consumir sin
    /// llevarse los handlers. Útil para `build_router`.
    pub fn metas(&self) -> Vec<RouteMeta> {
        self.routes
            .iter()
            .map(|r| RouteMeta {
                method: r.method,
                path: r.path.clone(),
                has_path_params: !r.path_params.is_empty(),
                has_query_params: !r.query_params.is_empty(),
                expects_body: r.body_param.is_some(),
                cors: r.cors.clone(),
            })
            .collect()
    }
}

/// Construye un `axum::Router` a partir de la metadata de rutas.
/// Cada handler async cierra sobre un `Arc<HttpRegistry>` clonado y
/// el índice de su ruta, e invoca `handle_task(...).await` directo
/// sobre el registry compartido.
///
/// La metadata (`Vec<RouteMeta>`) basta para configurar todo el
/// routing: verbo + path + flags estructurales (has_path_params /
/// has_query_params / expects_body) que deciden el shape del handler
/// axum (cuáles extractors usar). El `RouteSpec` correspondiente (con
/// el `Value::Function` Fitz) vive dentro del registry y se busca por
/// índice cuando entra una request.
///
/// `openapi_schema` (Fase 7.2): si es `Some`, registra una ruta
/// `GET /openapi.json` que sirve el schema cacheado (precomputado al
/// arrancar el server). Si el usuario ya declaró un handler con ese
/// path en sus rutas, el auto-register cede — la del usuario gana.
/// `None` para programas donde no querramos servir el schema (tests
/// internos, server arrancado en modo opt-out cuando 7.4 cierre).
///
/// **F17.5**: el viejo bridge `mpsc/oneshot` (`InterpTask` + un
/// std::thread aparte para el intérprete, con `run_interpreter_loop`
/// del lado main) desapareció. Post-F17.3 los futures del evaluator
/// son `Send` y `HttpRegistry` también — los handlers axum llaman al
/// evaluator directo y `tokio::spawn` (vía `rt-multi-thread` desde
/// F17.4a) los corre en paralelo entre workers. Eso destraba el
/// paralelismo HTTP real, sin perder ninguna funcionalidad que tenía
/// el bridge.
pub fn build_router(
    metas: &[RouteMeta],
    registry: std::sync::Arc<HttpRegistry>,
    openapi_schema: Option<serde_json::Value>,
) -> Router {
    let mut router = Router::new();
    // Agrupar rutas por path para sumar el `OPTIONS` de preflight al
    // mismo MethodRouter en caso de que varios verbos del mismo path
    // tengan CORS. Hoy `@get`/`@post`/... por path son únicos (no
    // soportamos múltiples handlers por (path, method)), pero pueden
    // existir handlers distintos con métodos distintos sobre el mismo
    // path. axum permite encadenar `.get(...).post(...)` en un mismo
    // MethodRouter, lo cual chocaría con `router.route` dos veces. Por
    // ahora cada (path, method) registra su MethodRouter directo —
    // si hay dos métodos sobre el mismo path con CORS, el preflight
    // termina sumado a la segunda ruta. Aceptable para MW.2; revisitable.
    for (idx, meta) in metas.iter().enumerate() {
        let route_handler = build_method_router(
            meta.method,
            idx,
            registry.clone(),
            meta.has_path_params,
            meta.has_query_params,
            meta.expects_body,
        );
        let route_handler = match &meta.cors {
            Some(cors) => attach_preflight(route_handler, cors.clone()),
            None => route_handler,
        };
        router = router.route(&meta.path, route_handler);
    }

    // Auto-register de /openapi.json (Fase 7.2) y /docs (Fase 7.3).
    // El schema viene precomputado por `serve` (eager, una sola vez
    // al arrancar); cada request lo clona — clone de `serde_json::Value`
    // es lineal en el tamaño del schema, despreciable para APIs
    // típicas. La UI Scalar es HTML estático (incluido al binario
    // como `&'static str`).
    //
    // En ambos casos: si el usuario ya declaró un handler con el
    // mismo path, el auto-register cede.
    if let Some(schema) = openapi_schema {
        if !metas.iter().any(|m| m.path == "/openapi.json") {
            let schema = std::sync::Arc::new(schema);
            router = router.route(
                "/openapi.json",
                axum::routing::get(move || {
                    let schema = schema.clone();
                    async move { axum::Json((*schema).clone()) }
                }),
            );
        }
        if !metas.iter().any(|m| m.path == "/docs") {
            router = router.route(
                "/docs",
                axum::routing::get(|| async {
                    axum::response::Html(crate::openapi::SCALAR_HTML)
                }),
            );
        }
    }

    router
}

/// Convierte el `HeaderMap` de axum a un `HashMap<String, String>`
/// con todas las keys en lowercase (Fase 7.6). El dispatch hace
/// lookup case-insensitive contra esta map. Los headers no-UTF-8 se
/// omiten (HTTP teóricamente permite bytes raros; en la práctica
/// todos los headers usuales son ASCII).
fn headers_to_map(hm: &axum::http::HeaderMap) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for (name, value) in hm.iter() {
        if let Ok(v) = value.to_str() {
            out.insert(name.as_str().to_lowercase(), v.to_string());
        }
    }
    out
}

/// Construye un `MethodRouter` con el handler async correspondiente
/// al verbo. Las ocho combinaciones (path_params × query × body)
/// viven en closures distintos porque los extractors de axum aparecen
/// como argumentos del handler — no se pueden hacer condicionales.
/// `HeaderMap` se extrae **siempre** como argumento extra (Fase 7.6):
/// es zero-cost cuando el handler no declara headers (pasa HashMap
/// vacío y `handle_task` lo ignora).
///
/// **F17.5**: cada closure clona el `Arc<HttpRegistry>` y llama a
/// `handle_task(&registry, ...).await` directo. Antes mandaba un
/// `InterpTask` por mpsc y await-eaba un `oneshot`. La eliminación
/// del bridge destraba el paralelismo HTTP real: con runtime
/// `rt-multi-thread` (F17.4a), N workers procesan handlers en
/// simultáneo sobre el mismo registry compartido (Send + Sync).
fn build_method_router(
    method: HttpMethod,
    route_idx: usize,
    registry: std::sync::Arc<HttpRegistry>,
    has_path_params: bool,
    has_query_params: bool,
    expects_body: bool,
) -> MethodRouter {
    use axum::extract::Query as AxumQuery;
    use axum::http::HeaderMap;
    type Map = HashMap<String, String>;
    match (has_path_params, has_query_params, expects_body) {
        (false, false, false) => {
            let h = move |headers: HeaderMap| {
                let registry = registry.clone();
                async move {
                    let hm = headers_to_map(&headers);
                    dispatch_request(&registry, route_idx, Map::new(), Map::new(), Vec::new(), hm).await
                }
            };
            wrap(method, h)
        }
        (true, false, false) => {
            let h = move |AxumPath(p): AxumPath<Map>, headers: HeaderMap| {
                let registry = registry.clone();
                async move {
                    let hm = headers_to_map(&headers);
                    dispatch_request(&registry, route_idx, p, Map::new(), Vec::new(), hm).await
                }
            };
            wrap(method, h)
        }
        (false, true, false) => {
            let h = move |AxumQuery(q): AxumQuery<Map>, headers: HeaderMap| {
                let registry = registry.clone();
                async move {
                    let hm = headers_to_map(&headers);
                    dispatch_request(&registry, route_idx, Map::new(), q, Vec::new(), hm).await
                }
            };
            wrap(method, h)
        }
        (true, true, false) => {
            let h = move |AxumPath(p): AxumPath<Map>,
                          AxumQuery(q): AxumQuery<Map>,
                          headers: HeaderMap| {
                let registry = registry.clone();
                async move {
                    let hm = headers_to_map(&headers);
                    dispatch_request(&registry, route_idx, p, q, Vec::new(), hm).await
                }
            };
            wrap(method, h)
        }
        (false, false, true) => {
            let h = move |headers: HeaderMap, body: axum::body::Bytes| {
                let registry = registry.clone();
                async move {
                    let hm = headers_to_map(&headers);
                    dispatch_request(&registry, route_idx, Map::new(), Map::new(), body.to_vec(), hm).await
                }
            };
            wrap(method, h)
        }
        (true, false, true) => {
            let h = move |AxumPath(p): AxumPath<Map>,
                          headers: HeaderMap,
                          body: axum::body::Bytes| {
                let registry = registry.clone();
                async move {
                    let hm = headers_to_map(&headers);
                    dispatch_request(&registry, route_idx, p, Map::new(), body.to_vec(), hm).await
                }
            };
            wrap(method, h)
        }
        (false, true, true) => {
            let h = move |AxumQuery(q): AxumQuery<Map>,
                          headers: HeaderMap,
                          body: axum::body::Bytes| {
                let registry = registry.clone();
                async move {
                    let hm = headers_to_map(&headers);
                    dispatch_request(&registry, route_idx, Map::new(), q, body.to_vec(), hm).await
                }
            };
            wrap(method, h)
        }
        (true, true, true) => {
            let h = move |AxumPath(p): AxumPath<Map>,
                          AxumQuery(q): AxumQuery<Map>,
                          headers: HeaderMap,
                          body: axum::body::Bytes| {
                let registry = registry.clone();
                async move {
                    let hm = headers_to_map(&headers);
                    dispatch_request(&registry, route_idx, p, q, body.to_vec(), hm).await
                }
            };
            wrap(method, h)
        }
    }
}

/// Mapea `HttpMethod` al constructor de axum (`get`/`post`/`put`/`delete`)
/// aplicado al handler dado.
fn wrap<H, T>(method: HttpMethod, h: H) -> MethodRouter
where
    H: axum::handler::Handler<T, ()>,
    T: 'static,
{
    use axum::routing::{delete, get, post, put};
    match method {
        HttpMethod::Get => get(h),
        HttpMethod::Post => post(h),
        HttpMethod::Put => put(h),
        HttpMethod::Delete => delete(h),
    }
}

/// Suma un handler `OPTIONS` al MethodRouter dado para responder
/// preflight CORS (mini-fase MW.2). El handler devuelve 204 con los
/// headers `Access-Control-Allow-*` resueltos contra el `Origin` del
/// request — no toca el intérprete, así que es rápido y no usa el
/// bridge mpsc. Q.3: el header `Access-Control-Allow-Origin` puede
/// omitirse si la política `Set` rechaza el origin recibido (browser
/// rechaza el preflight, comportamiento estándar CORS estricto).
fn attach_preflight(
    mr: MethodRouter,
    cors: std::sync::Arc<CorsConfig>,
) -> MethodRouter {
    mr.options(move |headers: axum::http::HeaderMap| {
        let cors = cors.clone();
        async move {
            let request_origin = headers
                .get("origin")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let resolved = cors.response_headers(request_origin.as_deref());
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::NO_CONTENT;
            for (name, value) in resolved {
                let parsed_name = axum::http::HeaderName::try_from(name);
                let parsed_value = HeaderValue::try_from(value);
                if let (Ok(n), Ok(v)) = (parsed_name, parsed_value) {
                    resp.headers_mut().insert(n, v);
                }
            }
            resp
        }
    })
}

/// Punto único donde el handler axum invoca al evaluator y devuelve
/// la `Response`. Post-F17.5: llamada directa a `handle_task` —
/// el bridge mpsc/oneshot que existía en F4.x quedó eliminado al
/// volver `Value`/`EnvRef` `Send` (F17.2-3) y `HttpRegistry`
/// `Send + Sync`.
async fn dispatch_request(
    registry: &HttpRegistry,
    route_idx: usize,
    path_params: HashMap<String, String>,
    query_params: HashMap<String, String>,
    body: Vec<u8>,
    headers: HashMap<String, String>,
) -> Response {
    let outcome = handle_task(
        registry,
        route_idx,
        path_params,
        query_params,
        body,
        headers,
    )
    .await;
    outcome_to_response(outcome)
}

/// Convierte un `HandlerOutcome` a la `Response` de axum. Status,
/// header `content-type`, body como bytes, y los `extra_headers` que
/// hayan inyectado los middlewares (mini-fase MW.2: headers CORS).
///
/// Si un extra_header trae un nombre o un valor no parseable como
/// header HTTP, se omite silenciosamente — preferimos perder un
/// header malformado a hacer panic en una request. En la práctica los
/// CORS headers que emitimos son válidos por construcción.
fn outcome_to_response(outcome: HandlerOutcome) -> Response {
    let mut resp = Response::new(Body::from(outcome.body));
    *resp.status_mut() = StatusCode::from_u16(outcome.status)
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static(outcome.content_type),
    );
    for (name, value) in outcome.extra_headers {
        let parsed_name = axum::http::HeaderName::try_from(name);
        let parsed_value = HeaderValue::try_from(value);
        if let (Ok(n), Ok(v)) = (parsed_name, parsed_value) {
            resp.headers_mut().insert(n, v);
        }
    }
    resp
}

/// Construye el `Value::Instance` de tipo `Request` que el runtime
/// pasa a cada middleware (mini-fase MW.1). El path lleva los path
/// params sustituidos (`/users/{id}` con `id=42` se ve como
/// `/users/42`); la query string del request original NO se concatena
/// para evitar dependencia del orden de `HashMap`. Si aparece presión
/// real por exponer la query string completa, se suma como deuda
/// menor. Los headers se exponen con sus keys en lowercase (consistente
/// con el dispatch case-insensitive de `@header`).
fn build_request_value(
    method: HttpMethod,
    path_template: &str,
    raw_path_params: &HashMap<String, String>,
    headers: &HashMap<String, String>,
) -> Value {
    use crate::value::shared;

    // Sustituir cada `{name}` por su valor real. O(n*m) pero n y m
    // son chicos (un handler típico tiene 0-3 path params); evitable
    // con un parser fino, no vale el costo de mantenimiento.
    let mut path = path_template.to_string();
    for (k, v) in raw_path_params {
        path = path.replace(&format!("{{{}}}", k), v);
    }

    let headers_pairs: Vec<(Value, Value)> = headers
        .iter()
        .map(|(k, v)| (Value::Str(k.clone()), Value::Str(v.clone())))
        .collect();

    Value::new_instance(
        "Request".to_string(),
        vec![
            ("method".to_string(), Value::Str(method.as_str().to_string())),
            ("path".to_string(), Value::Str(path)),
            ("headers".to_string(), Value::Map(shared(headers_pairs))),
        ],
    )
}

/// Ejecuta la cadena de middlewares de una ruta en orden (mini-fase
/// MW.1). Cada middleware recibe un único arg `Request` y se espera
/// que devuelva:
///
///   - `Value::Null` (o nada) → la cadena continúa con el siguiente
///     middleware o el handler.
///   - `Value::HttpResponse` (construido con `return <status> { ... }`)
///     → short-circuit: la cadena corta acá y el outcome se devuelve
///     al cliente.
///   - Cualquier otro valor → 500 con mensaje claro (el middleware
///     tiene que ser gate-only).
///
/// Devuelve `Some(outcome)` si un middleware short-circuita o si algo
/// falló; `None` si la cadena llegó al final y hay que invocar el
/// handler.
async fn run_middleware_chain(
    middlewares: &[MiddlewareSpec],
    request: &Value,
) -> Option<HandlerOutcome> {
    // Mw.next — solo corremos los Pre (gate-only) en este path. Los
    // Post se procesan en `run_post_middlewares` después del handler.
    for mw in middlewares.iter().filter(|m| m.kind == MiddlewareKind::Pre) {
        let args = vec![request.clone()];
        let label = format!("middleware {}", mw.name);
        match call_handler(mw.handler.clone(), args, &label).await {
            Ok(Value::Null) => continue,
            Ok(Value::HttpResponse { status, body }) => {
                let payload_json = match body {
                    Some(b) => match value_to_json(b.as_ref()) {
                        Ok(j) => j,
                        Err(msg) => return Some(HandlerOutcome::internal_error(msg)),
                    },
                    None => serde_json::Value::Null,
                };
                return Some(HandlerOutcome::json(status, payload_json));
            }
            Ok(other) => {
                return Some(HandlerOutcome::internal_error(format!(
                    "middleware '{}' devolvió un valor inesperado ({}); \
                     debe devolver `null` para continuar o `return <status> {{ ... }}` \
                     para cortocircuitar",
                    mw.name,
                    other.type_name(),
                )));
            }
            Err(err) => {
                return Some(HandlerOutcome::internal_error(format!(
                    "middleware '{}' falló: {}",
                    mw.name, err.message,
                )));
            }
        }
    }
    None
}

/// Mw.next — corre los middlewares Post (2 args) en orden INVERSO al
/// de registración (semántica de wrap: el último registrado es el más
/// interno, ve la response primero). Cada Post recibe `(Request,
/// Response)` y devuelve un `Response`. La Response actual se
/// representa como `Value::HttpResponse { status, body }` construido
/// desde el `HandlerOutcome` previo. La response final retorna como
/// HandlerOutcome.
///
/// Errores: si un Post no devuelve `Value::HttpResponse`, error 500
/// claro citando el middleware. Si la chain está vacía o no hay Post
/// mws, devuelve el outcome original sin cambios.
async fn run_post_middlewares(
    middlewares: &[MiddlewareSpec],
    request: &Value,
    mut outcome: HandlerOutcome,
) -> HandlerOutcome {
    let post_mws: Vec<&MiddlewareSpec> = middlewares
        .iter()
        .filter(|m| m.kind == MiddlewareKind::Post)
        .collect();
    if post_mws.is_empty() {
        return outcome;
    }
    // Construir el Value::HttpResponse inicial. El body se parsea desde
    // el JSON del outcome. Si el body no es JSON válido (caso raro), lo
    // pasamos como Str crudo.
    for mw in post_mws.iter().rev() {
        let body_value: Option<Box<Value>> =
            serde_json::from_str::<serde_json::Value>(&outcome.body)
                .ok()
                .map(|j| Box::new(json_to_value(&j)));
        let response_value = Value::HttpResponse {
            status: outcome.status,
            body: body_value,
        };
        let args = vec![request.clone(), response_value];
        let label = format!("middleware post '{}'", mw.name);
        match call_handler(mw.handler.clone(), args, &label).await {
            Ok(Value::HttpResponse { status, body }) => {
                let payload_json = match body {
                    Some(b) => match value_to_json(b.as_ref()) {
                        Ok(j) => j,
                        Err(msg) => return HandlerOutcome::internal_error(msg),
                    },
                    None => serde_json::Value::Null,
                };
                // Preservar headers existentes (CORS, custom ya
                // inyectados); el Post-mw puede sumar headers via
                // un campo adicional `extra_headers` futuro (deuda
                // residual). Por ahora, el post-mw decide status +
                // body, los extra_headers se preservan del outcome
                // previo.
                let prev_extras = std::mem::take(&mut outcome.extra_headers);
                outcome = HandlerOutcome::json(status, payload_json);
                outcome.extra_headers = prev_extras;
            }
            Ok(other) => {
                return HandlerOutcome::internal_error(format!(
                    "middleware post '{}' devolvió un valor inesperado ({}); \
                     debe devolver `Response` (un `return <status> {{ ... }}`)",
                    mw.name,
                    other.type_name(),
                ));
            }
            Err(err) => {
                return HandlerOutcome::internal_error(format!(
                    "middleware post '{}' falló: {}",
                    mw.name, err.message,
                ));
            }
        }
    }
    outcome
}

/// Mini-tanda Mw-Wrap — corre la chain de wrap-style middlewares
/// envolviendo el handler + post chain. Cada Wrap recibe
/// `(request, next)` donde `next` es un `Value::NativeFn` que ejecuta
/// el resto: los wraps restantes + el handler + los post mws.
///
/// El Wrap mw decide cuándo invocar `next()` (antes/después del
/// handler, condicionalmente, midiendo tiempo, etc.). Su return value
/// (`Response`) se convierte al outcome final.
///
/// Estructura recursiva: caso base = sin wraps → invocar handler + post.
/// Caso recursivo = pop primer wrap, construir NativeFn que recursea
/// con los wraps restantes, invocar wrap actual.
fn run_wrap_chain(
    wraps: Vec<MiddlewareSpec>,
    handler: Value,
    handler_args: Vec<Value>,
    handler_name: String,
    request: Value,
    post_mws: Vec<MiddlewareSpec>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = HandlerOutcome> + Send>> {
    Box::pin(async move {
        if wraps.is_empty() {
            // Caso base: invocar handler + post chain.
            let outcome = match call_handler(handler, handler_args, &handler_name).await {
                Ok(value) => value_to_outcome(&value),
                Err(err) => HandlerOutcome::internal_error(err.message),
            };
            return run_post_middlewares(&post_mws, &request, outcome).await;
        }
        // Pop first wrap; el resto va a la closure del NativeFn.
        let mut iter = wraps.into_iter();
        let current = iter.next().unwrap();
        let remaining: Vec<MiddlewareSpec> = iter.collect();

        // Construir el `next` callable. Capturamos por valor (clone)
        // todo lo que la closure va a necesitar la próxima vez.
        let req_clone = request.clone();
        let handler_clone = handler.clone();
        let handler_name_clone = handler_name.clone();
        let handler_args_clone = handler_args.clone();
        let post_clone = post_mws.clone();
        let remaining_clone = remaining.clone();
        let next: crate::value::NativeAsyncFn = crate::value::NativeAsyncFn(
            std::sync::Arc::new(move |_args: Vec<Value>| {
                // Re-clone para cada invocación (puede llamarse 0+ veces).
                let req2 = req_clone.clone();
                let h2 = handler_clone.clone();
                let p2 = post_clone.clone();
                let r2 = remaining_clone.clone();
                let hn2 = handler_name_clone.clone();
                let ha2 = handler_args_clone.clone();
                Box::pin(async move {
                    let outcome = run_wrap_chain(r2, h2, ha2, hn2, req2, p2).await;
                    // Convertir outcome → Value::HttpResponse para que el
                    // mw lo consuma como `Response`.
                    let body = serde_json::from_str::<serde_json::Value>(&outcome.body)
                        .ok()
                        .map(|j| Box::new(json_to_value(&j)));
                    Ok(Value::HttpResponse {
                        status: outcome.status,
                        body,
                    })
                }) as crate::value::FitzFuture
            }),
        );

        // Invocar el Wrap mw con (request, next).
        let args = vec![request.clone(), Value::NativeFn(next)];
        let label = format!("middleware wrap '{}'", current.name);
        match call_handler(current.handler.clone(), args, &label).await {
            Ok(Value::HttpResponse { status, body }) => {
                let payload = match body {
                    Some(b) => match value_to_json(b.as_ref()) {
                        Ok(j) => j,
                        Err(msg) => return HandlerOutcome::internal_error(msg),
                    },
                    None => serde_json::Value::Null,
                };
                HandlerOutcome::json(status, payload)
            }
            Ok(other) => HandlerOutcome::internal_error(format!(
                "middleware wrap '{}' devolvió un valor inesperado ({}); \
                 debe devolver `Response` (un `return <status> {{ ... }}`)",
                current.name,
                other.type_name(),
            )),
            Err(err) => HandlerOutcome::internal_error(format!(
                "middleware wrap '{}' falló: {}",
                current.name, err.message,
            )),
        }
    })
}

/// Procesa un único task. Aislado del loop para testearlo sin canal.
async fn handle_task(
    registry: &HttpRegistry,
    route_idx: usize,
    raw_path_params: HashMap<String, String>,
    raw_query_params: HashMap<String, String>,
    body_bytes: Vec<u8>,
    raw_headers: HashMap<String, String>,
) -> HandlerOutcome {
    let Some(route) = registry.routes.get(route_idx) else {
        return HandlerOutcome::internal_error(format!(
            "ruta {} no existe en el registry",
            route_idx,
        ));
    };

    // MW.1: middlewares apilados sobre la ruta. Corren ANTES de parsear
    // body o coercionar params: si un middleware de auth/CORS cortocircuita,
    // ahorramos el trabajo de validar el resto del request. La cadena
    // recibe un único arg `Request` con method/path/headers; body y
    // query params no se exponen al middleware (deuda explícita).
    if !route.middlewares.is_empty() {
        let request = build_request_value(
            route.method,
            &route.path,
            &raw_path_params,
            &raw_headers,
        );
        if let Some(outcome) = run_middleware_chain(&route.middlewares, &request).await {
            return outcome;
        }
    }

    // Si el handler espera body, parsearlo y prepararlo. Lo hacemos
    // antes de armar args para fallar temprano si el JSON está roto.
    //
    // Mini-tanda Hpx.1 — validación de Content-Type: si el handler
    // declara body param, exigimos `application/json`. Cualquier otro
    // Content-Type (multipart, urlencoded, etc.) → 415 con mensaje
    // claro. Si NO hay header (body crudo), aceptamos (clientes
    // tipo curl sin -H lo emiten así, y Fitz nunca prometió Content-
    // Type estricto). Sub-paso futuro dedicado para multipart/form.
    //
    // Mini-tanda MP — sumamos soporte para `application/x-www-form-urlencoded`:
    // se parsea como `Map<Str, Str>` y se asigna al body param.
    // Multipart con files queda como sub-paso futuro (más complejo).
    let body_value: Option<Value> = if let Some(bp) = &route.body_param {
        let raw_ct = raw_headers.get("content-type").cloned();
        let ct_primary = raw_ct
            .as_ref()
            .map(|ct| ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
            .unwrap_or_default();

        let is_urlencoded = ct_primary == "application/x-www-form-urlencoded";
        let is_json_or_empty = ct_primary.is_empty() || ct_primary == "application/json";
        // Mini-tanda MP2 — multipart/form-data con boundary.
        let is_multipart = ct_primary == "multipart/form-data";

        if !is_json_or_empty && !is_urlencoded && !is_multipart {
            // text/plain, custom, etc. → 415.
            return HandlerOutcome::json(
                415,
                serde_json::json!({
                    "error": format!(
                        "Content-Type no soportado: '{}'. El handler espera JSON \
                         (`application/json`), urlencoded \
                         (`application/x-www-form-urlencoded`) o multipart \
                         (`multipart/form-data`). Otros formatos quedan como \
                         sub-paso futuro.",
                        raw_ct.as_deref().unwrap_or("(sin header)")
                    ),
                }),
            );
        }

        if is_multipart {
            // Mini-tanda MP2 — extraer boundary del Content-Type
            // (`multipart/form-data; boundary=<token>`). Sin boundary
            // → 400 claro.
            let boundary = raw_ct
                .as_deref()
                .and_then(extract_multipart_boundary);
            match boundary {
                None => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({
                            "error": "multipart/form-data: falta el parámetro `boundary` en Content-Type"
                        }),
                    );
                }
                Some(b) => match parse_multipart_body(&body_bytes, &b) {
                    Ok(v) => Some(v),
                    Err(msg) => {
                        return HandlerOutcome::json(
                            400,
                            serde_json::json!({ "error": msg }),
                        );
                    }
                },
            }
        } else if is_urlencoded {
            match parse_urlencoded_body(&body_bytes) {
                Ok(v) => Some(v),
                Err(msg) => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({ "error": msg }),
                    );
                }
            }
        } else {
            match parse_body(&body_bytes, bp) {
                Ok(v) => Some(v),
                Err(msg) => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({ "error": msg }),
                    );
                }
            }
        }
    } else {
        None
    };

    // Armar args en el orden declarado del handler. Para cada
    // parámetro:
    //   - si su nombre está en `path_params`, tomar el valor crudo del
    //     map de path y coercionarlo al tipo declarado;
    //   - si está en `query_params`, idem desde el map de query
    //     (nullable → Null si falta; obligatorio → 400 si falta);
    //   - si es el body param, usar el valor parseado;
    //   - cualquier otro caso (no path, no query, no body) es un bug
    //     del registro: el evaluator no permite registrarlo.
    let mut args = Vec::with_capacity(route.param_types.len());
    for (name, head_type, is_nullable) in &route.param_types {
        if route.path_params.iter().any(|p| p == name) {
            // Path params son siempre obligatorios (axum garantiza que
            // llegan si la ruta matcheó). Coerción al tipo declarado.
            let raw = raw_path_params.get(name).map(|s| s.as_str()).unwrap_or("");
            match coerce_path_param(raw, head_type.as_deref()) {
                Ok(v) => args.push(v),
                Err(msg) => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({
                            "error": format!("path param '{}': {}", name, msg),
                        }),
                    );
                }
            }
        } else if route.query_params.iter().any(|q| q == name) {
            // Query params: si el tipo declarado es nullable (`Int?`),
            // missing → Null. Si es obligatorio, missing → 400.
            let raw = raw_query_params.get(name);
            match (raw, *is_nullable) {
                (Some(s), _) => {
                    match coerce_path_param(s, head_type.as_deref()) {
                        Ok(v) => args.push(v),
                        Err(msg) => {
                            return HandlerOutcome::json(
                                400,
                                serde_json::json!({
                                    "error": format!("query param '{}': {}", name, msg),
                                }),
                            );
                        }
                    }
                }
                (None, true) => args.push(Value::Null),
                (None, false) => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({
                            "error": format!("query param '{}': falta — es obligatorio", name),
                        }),
                    );
                }
            }
        } else if route.body_param.as_ref().map(|bp| bp.name.as_str()) == Some(name) {
            // Body param: ya parseado arriba; tomarlo de `body_value`.
            // unwrap es seguro porque body_value es Some sii hay body_param.
            args.push(body_value.clone().unwrap());
        } else if let Some(hdr) = route.headers.iter().find(|h| &h.param_name == name) {
            // Header (Fase 7.6). Lookup case-insensitive vía lowercase
            // del nombre HTTP. Falta + nullable → Null. Falta +
            // obligatorio → 400.
            let key = hdr.http_name.to_lowercase();
            match (raw_headers.get(&key), hdr.is_nullable) {
                (Some(v), _) => args.push(Value::Str(v.clone())),
                (None, true) => args.push(Value::Null),
                (None, false) => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({
                            "error": format!(
                                "header '{}': falta — es obligatorio",
                                hdr.http_name
                            ),
                        }),
                    );
                }
            }
        } else {
            return HandlerOutcome::internal_error(format!(
                "parámetro '{}' del handler '{}' no es ni path param ni query param ni body ni header — \
                 esto es un bug interno del registro",
                name, route.handler_name,
            ));
        }
    }

    // Mini-tanda Mw-Wrap — si hay wrap-style middlewares, el chain
    // runner los envuelve alrededor del handler + post mws. Si no
    // hay wraps, seguimos con el flujo clásico (handler + post).
    let has_wraps = route
        .middlewares
        .iter()
        .any(|m| m.kind == MiddlewareKind::Wrap);
    let mut outcome = if has_wraps {
        let wraps: Vec<MiddlewareSpec> = route
            .middlewares
            .iter()
            .filter(|m| m.kind == MiddlewareKind::Wrap)
            .cloned()
            .collect();
        let post_mws: Vec<MiddlewareSpec> = route
            .middlewares
            .iter()
            .filter(|m| m.kind == MiddlewareKind::Post)
            .cloned()
            .collect();
        let request = build_request_value(
            route.method,
            &route.path,
            &raw_path_params,
            &raw_headers,
        );
        run_wrap_chain(
            wraps,
            route.handler.clone(),
            args,
            route.handler_name.clone(),
            request,
            post_mws,
        )
        .await
    } else {
        // Flujo clásico: invocar handler + post mws.
        let mut outcome = match call_handler(route.handler.clone(), args, &route.handler_name).await {
            Ok(value) => value_to_outcome(&value),
            Err(err) => HandlerOutcome::internal_error(err.message),
        };

        // Mw.next — correr los post-middlewares (kind = Post, 2-arg)
        // DESPUÉS del handler. Reciben `(Request, Response)` y pueden
        // modificar el body o agregar headers. Si hay middlewares Pre que
        // short-circuit, este path no corre (ya retornamos arriba con la
        // response del Pre).
        if route.middlewares.iter().any(|m| m.kind == MiddlewareKind::Post) {
            let request = build_request_value(
                route.method,
                &route.path,
                &raw_path_params,
                &raw_headers,
            );
            outcome = run_post_middlewares(&route.middlewares, &request, outcome).await;
        }
        outcome
    };

    // MW.2: si la ruta declara CORS, agregar los headers
    // `Access-Control-Allow-*` a la response real. Incluido en
    // responses de error (500/400) — el browser lee CORS antes de
    // parsear el body, así que sin estos headers cualquier error
    // sale como un "CORS error" en consola en vez del 500/400 que
    // de verdad ocurrió.
    // Q.3: pasamos el `Origin` del request al config; si la política
    // es `Set` y matchea, echo del Origin recibido; si no, NO se
    // emite el header (browser rechaza la response — CORS estricto).
    if let Some(cors) = &route.cors {
        let request_origin = raw_headers.get("origin").map(|s| s.as_str());
        outcome
            .extra_headers
            .extend(cors.response_headers(request_origin));
    }
    outcome
}

/// Parsea los bytes del body en un `Value` Fitz según la convención
/// del body param:
///   - JSON inválido → error 400 con mensaje claro.
///   - Si el body param tiene `declared_type: Some(Value::Type)`,
///     validamos contra el type (campos faltantes, extras, etc.) y
///     construimos un `Value::Instance`.
///   - Si no, deserializamos a `Value` libre (Map/List/primitivos).
///
/// Mini-tanda MP — parsea `application/x-www-form-urlencoded` body
/// (formato `key1=value1&key2=value2`) a un `Value::Map<Str, Str>`.
/// URL-decoding aplicado a keys y valores. Body vacío → Map vacío.
/// Duplicados: last-wins (paralelo a la convención de `serde_urlencoded`).
fn parse_urlencoded_body(bytes: &[u8]) -> Result<Value, String> {
    use crate::value::shared;
    let s = std::str::from_utf8(bytes).map_err(|e| {
        format!("body urlencoded inválido (UTF-8): {}", e)
    })?;
    let mut pairs: Vec<(Value, Value)> = Vec::new();
    if s.is_empty() {
        return Ok(Value::Map(shared(pairs)));
    }
    for kv in s.split('&') {
        let mut parts = kv.splitn(2, '=');
        let raw_k = parts.next().unwrap_or("");
        let raw_v = parts.next().unwrap_or("");
        let k = url_decode(raw_k)?;
        let v = url_decode(raw_v)?;
        // Duplicados: last-wins. Eliminamos entry previa con misma key.
        pairs.retain(|(existing_k, _)| {
            !matches!(existing_k, Value::Str(s) if s == &k)
        });
        pairs.push((Value::Str(k), Value::Str(v)));
    }
    Ok(Value::Map(shared(pairs)))
}

/// Mini-tanda MP2 — extrae el `boundary` del Content-Type header de
/// `multipart/form-data` (`multipart/form-data; boundary=<token>` o
/// `boundary="<token>"`). Devuelve `None` si no aparece el parámetro.
/// Trim de espacios + soporte de comillas dobles (RFC 7578).
fn extract_multipart_boundary(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        let lower = part.to_ascii_lowercase();
        if let Some(stripped) = lower.strip_prefix("boundary=") {
            // Stripped es lowercase; necesitamos volver al original
            // para preservar el case del boundary (los boundaries son
            // case-sensitive según RFC 7578).
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

/// Mini-tanda MP2 + File.content Bytes — parser de
/// `multipart/form-data` (RFC 7578) sobre raw bytes.
///
/// Cada part del body viene delimitado por `--<boundary>\r\n` con
/// headers tipo `Content-Disposition: form-data; name="X"; filename="Y"`
/// (filename opcional para text fields). Body de la part separado
/// de los headers por `\r\n\r\n`. La última part termina con
/// `\r\n--<boundary>--`.
///
/// Devuelve `Value::Map<Str, Value>` donde cada entry es:
/// - Text field (sin `filename`) → `Value::Str(content)` (UTF-8;
///   si el content no es UTF-8, error 400).
/// - File field (con `filename`) → `Value::Instance` de `File` con
///   `name`, `content_type`, `content: Bytes`. Files binarios YA
///   funcionan — el content se guarda como `Value::Bytes(Vec<u8>)`
///   sin requerir UTF-8.
///
/// Refactor desde la versión MP2 inicial: ahora trabajamos byte por
/// byte para preservar bytes binarios. Búsqueda de delimitadores
/// usa `slice::windows` o un scan manual; headers se parsean como
/// UTF-8 (ASCII per RFC 7578).
///
/// Duplicados de `name`: last-wins.
fn parse_multipart_body(bytes: &[u8], boundary: &str) -> Result<Value, String> {
    let delimiter = format!("--{}", boundary).into_bytes();
    // Split por la secuencia delimitador.
    let parts_raw: Vec<&[u8]> = split_bytes_by(bytes, &delimiter);
    let mut entries: Vec<(Value, Value)> = Vec::new();
    for raw in parts_raw.iter().skip(1) {
        // Terminator final: `--<boundary>--` produce un raw que
        // empieza con `--` (justo después del delimiter).
        if raw.starts_with(b"--") {
            break;
        }
        // Cada part empieza con `\r\n` (separator entre delimiter y
        // headers). Si no lo tiene, malformado.
        let body = strip_prefix_bytes(raw, b"\r\n").unwrap_or(raw);
        // Cada part puede terminar con `\r\n` antes del próximo
        // delimiter. Trimmealo.
        let body = strip_suffix_bytes(body, b"\r\n").unwrap_or(body);

        // Split headers vs content por la primera ocurrencia de
        // `\r\n\r\n`. Los headers son ASCII; el content puede ser
        // cualquier secuencia de bytes.
        let Some(split_idx) = find_bytes(body, b"\r\n\r\n") else {
            return Err(
                "multipart: part malformada — falta `\\r\\n\\r\\n` entre headers y body"
                    .to_string(),
            );
        };
        let headers_bytes = &body[..split_idx];
        let content_bytes = &body[split_idx + 4..];
        let headers_str = std::str::from_utf8(headers_bytes).map_err(|e| {
            format!("multipart: headers no son ASCII/UTF-8 válido: {}", e)
        })?;

        // Parse de headers de la part. Solo nos interesa
        // `Content-Disposition` (extrae `name` y `filename`) y
        // `Content-Type` (para files).
        let mut name_field: Option<String> = None;
        let mut filename: Option<String> = None;
        let mut content_type_part: Option<String> = None;
        for line in headers_str.split("\r\n") {
            let lower = line.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("content-disposition:") {
                let orig_offset = line.len() - rest.len();
                let value = &line[orig_offset..];
                let params: std::collections::HashMap<String, String> = parse_cd_params(value);
                name_field = params.get("name").cloned();
                filename = params.get("filename").cloned();
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

        let value = match filename {
            None => {
                // Text field: content debe ser UTF-8 válido. Para
                // bytes binarios sin filename, error.
                let s = std::str::from_utf8(content_bytes).map_err(|e| {
                    format!(
                        "multipart: text field '{}' no es UTF-8 válido (use filename= para bytes binarios): {}",
                        name, e
                    )
                })?;
                Value::Str(s.to_string())
            }
            Some(fname) => {
                // File field: content como Bytes (raw). Binary OK.
                let mut fields: Vec<(String, Value)> = Vec::new();
                let name_val = if fname.is_empty() {
                    Value::Null
                } else {
                    Value::Str(fname)
                };
                fields.push(("name".to_string(), name_val));
                fields.push((
                    "content_type".to_string(),
                    content_type_part
                        .map(Value::Str)
                        .unwrap_or(Value::Null),
                ));
                fields.push((
                    "content".to_string(),
                    Value::Bytes(content_bytes.to_vec()),
                ));
                Value::new_instance("File".to_string(), fields)
            }
        };

        let key = Value::Str(name);
        if let Some(idx) = entries.iter().position(|(k, _)| k == &key) {
            entries[idx].1 = value;
        } else {
            entries.push((key, value));
        }
    }

    Ok(Value::Map(shared(entries)))
}

/// File.content Bytes — helpers para split/find sobre `&[u8]`.
fn split_bytes_by<'a>(haystack: &'a [u8], needle: &[u8]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            out.push(&haystack[start..i]);
            i += needle.len();
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(&haystack[start..]);
    out
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn strip_prefix_bytes<'a>(s: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if s.starts_with(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn strip_suffix_bytes<'a>(s: &'a [u8], suffix: &[u8]) -> Option<&'a [u8]> {
    if s.len() >= suffix.len() && &s[s.len() - suffix.len()..] == suffix {
        Some(&s[..s.len() - suffix.len()])
    } else {
        None
    }
}

/// Helper para parsear params del header Content-Disposition:
/// `form-data; name="X"; filename="Y"`. Devuelve un map case-insensitive
/// (keys lowercase) → valor sin comillas.
fn parse_cd_params(s: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    // Skip el primer token (`form-data`).
    for part in s.split(';').skip(1) {
        let part = part.trim();
        let Some(eq_idx) = part.find('=') else {
            continue;
        };
        let key = part[..eq_idx].trim().to_ascii_lowercase();
        let value = part[eq_idx + 1..].trim().trim_matches('"');
        out.insert(key, value.to_string());
    }
    out
}

/// Mini-tanda MP — URL-decode (formato `application/x-www-form-urlencoded`):
/// `+` → espacio, `%XX` → byte hex. Errores de %XX malformado se
/// reportan con offset claro.
/// Quick win F13 bundle — encoder base64 estándar (RFC 4648, sin
/// URL-safe alphabet, con padding). Inline para evitar dep `base64`.
/// Acepta cualquier slice de bytes, devuelve String ASCII.
fn b64_encode_standard(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks(3);
    for c in chunks.by_ref() {
        let b0 = c[0];
        let b1 = if c.len() > 1 { c[1] } else { 0 };
        let b2 = if c.len() > 2 { c[2] } else { 0 };
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if c.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if c.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn url_decode(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut idx: usize = 0;
    while let Some(c) = chars.next() {
        match c {
            '+' => out.push(' '),
            '%' => {
                let h1 = chars.next().ok_or_else(|| {
                    format!("urlencoded: %XX incompleto en offset {}", idx)
                })?;
                let h2 = chars.next().ok_or_else(|| {
                    format!("urlencoded: %XX incompleto en offset {}", idx)
                })?;
                let byte = u8::from_str_radix(&format!("{}{}", h1, h2), 16).map_err(|_| {
                    format!("urlencoded: %{}{} no es hex válido", h1, h2)
                })?;
                // Acumular bytes para chars multi-byte UTF-8.
                out.push(byte as char);
                idx += 3;
                continue;
            }
            other => out.push(other),
        }
        idx += 1;
    }
    Ok(out)
}

fn parse_body(bytes: &[u8], bp: &BodyParam) -> Result<Value, String> {
    // Body vacío para un handler que espera body → error claro. Esto
    // pasa con `POST /users` sin body cuando el handler declara
    // `body: User`.
    if bytes.is_empty() {
        return Err(format!(
            "body requerido para el parámetro '{}' pero la request no trajo body",
            bp.name,
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("body no es JSON válido: {}", e))?;
    match &bp.declared_type {
        Some(t) => json_to_instance(&json, t),
        None => Ok(json_to_value(&json)),
    }
}

/// Arranca el servidor HTTP y bloquea el thread llamador hasta Ctrl-C.
///
/// **F17.5**: modelo simplificado, sin bridge:
///   - Un único runtime tokio `rt-multi-thread` corre acá mismo
///     (`block_on`), N workers según cores.
///   - El `HttpRegistry` se envuelve en `Arc` y se comparte con cada
///     handler axum. Cada worker que recibe una request invoca
///     `handle_task(&registry, ...).await` directo sobre el
///     evaluator — `Send + Sync` lo destrabó F17.2-3.
///   - El thread main bloquea sobre el runtime hasta que axum baja
///     por Ctrl-C (graceful shutdown sigue intacto).
///
/// Antes (Fase 4 → F17.4a) había un std::thread separado para tokio
/// más un loop síncrono en main que recibía `InterpTask`s por mpsc
/// y respondía por `oneshot`s. La eliminación del bridge fue la deuda
/// más grande de F17 — destraba paralelismo HTTP real (~300 LoC
/// menos en este archivo) y deja al evaluator alcanzable desde
/// axum sin glue.
pub fn serve(
    registry: HttpRegistry,
    program: crate::ast::Program,
    addr: std::net::SocketAddr,
) -> std::io::Result<()> {
    let metas = registry.metas();
    let enable_docs = registry.resolved_config().enable_docs;

    // Fase 7.2: precomputar el schema OpenAPI con `program` + `registry`
    // y pasarlo a `build_router`. El auto-register de `/openapi.json`
    // y `/docs` pasa por ahí (y respeta cualquier ruta declarada por
    // el usuario con esos paths).
    //
    // Fase 7.4: si `@server(docs=false)`, ni computamos el schema ni
    // lo pasamos al router — ambas rutas auto-registradas quedan en
    // 404. Trade-off: zero overhead cuando el usuario apaga los docs.
    // Q.2: leer `api_version` del config si se seteó vía
    // `@server(api_version="X.Y.Z")`. None → schema usa default "0.1.0".
    let api_version = registry
        .server_config
        .as_ref()
        .and_then(|c| c.api_version.clone());
    let openapi_schema = if enable_docs {
        let routes = crate::openapi::routes_from_registry(&registry, &program);
        Some(crate::openapi::generate_openapi_with_version(
            &routes,
            &program,
            api_version.as_deref(),
        ))
    } else {
        None
    };

    let registry = std::sync::Arc::new(registry);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let router = build_router(&metas, registry, openapi_schema);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        eprintln!("🏔️  Fitz HTTP escuchando en http://{}", addr);
        for meta in &metas {
            eprintln!("   {} {}", meta.method.as_str(), meta.path);
        }
        if enable_docs {
            eprintln!("   GET /openapi.json  (schema autogenerado)");
            eprintln!("   GET /docs          (UI Scalar)");
        } else {
            eprintln!("   (docs apagadas por @server(docs=false))");
        }
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await
    })?;

    Ok(())
}

/// Escucha SIGINT (Ctrl-C) para graceful shutdown.
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("\nbajando servidor...");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 en tests es un Float genérico, no PI.
mod tests {
    use super::*;
    use crate::ast::StrPart;
    use crate::value::shared;

    // ---- HttpMethod ----

    #[tokio::test(flavor = "current_thread")]
    async fn http_method_desde_nombre_de_decorator() {
        assert_eq!(HttpMethod::from_decorator_name("get"), Some(HttpMethod::Get));
        assert_eq!(HttpMethod::from_decorator_name("post"), Some(HttpMethod::Post));
        assert_eq!(HttpMethod::from_decorator_name("put"), Some(HttpMethod::Put));
        assert_eq!(HttpMethod::from_decorator_name("delete"), Some(HttpMethod::Delete));
        assert_eq!(HttpMethod::from_decorator_name("server"), None);
        assert_eq!(HttpMethod::from_decorator_name("patch"), None);
    }

    // ---- parse_path_template ----

    #[tokio::test(flavor = "current_thread")]
    async fn path_str_simple_sin_params() {
        let t = parse_path_template(&Expr::Str("/".into(), Span::ZERO)).unwrap();
        assert_eq!(t.path, "/");
        assert!(t.params.is_empty());

        let t = parse_path_template(&Expr::Str("/users".into(), Span::ZERO)).unwrap();
        assert_eq!(t.path, "/users");
        assert!(t.params.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_strinterp_con_un_param() {
        // `"/users/{id}"` → StrInterp([Lit("/users/"), Expr(Ident("id"))])
        let e = Expr::StrInterp(vec![
            StrPart::Lit("/users/".into()),
            StrPart::Expr(Expr::Ident("id".into(), Span::ZERO), None),
        ], Span::ZERO);
        let t = parse_path_template(&e).unwrap();
        assert_eq!(t.path, "/users/{id}");
        assert_eq!(t.params, vec!["id".to_string()]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_strinterp_con_varios_params_distintos() {
        // `"/orgs/{org}/users/{id}"`
        let e = Expr::StrInterp(vec![
            StrPart::Lit("/orgs/".into()),
            StrPart::Expr(Expr::Ident("org".into(), Span::ZERO), None),
            StrPart::Lit("/users/".into()),
            StrPart::Expr(Expr::Ident("id".into(), Span::ZERO), None),
        ], Span::ZERO);
        let t = parse_path_template(&e).unwrap();
        assert_eq!(t.path, "/orgs/{org}/users/{id}");
        assert_eq!(t.params, vec!["org".to_string(), "id".to_string()]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_no_arranca_con_slash_es_error() {
        let err = parse_path_template(&Expr::Str("users".into(), Span::ZERO)).unwrap_err();
        assert_eq!(err, PathError::MustStartWithSlash);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_con_expresion_no_ident_es_error() {
        // `"{a+b}"` — interpolación con BinOp.
        let e = Expr::StrInterp(vec![
            StrPart::Lit("/".into()),
            StrPart::Expr(Expr::BinOp {
                op: crate::ast::BinOpKind::Add,
                left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
                right: Box::new(Expr::Ident("b".into(), Span::ZERO)), span: Span::ZERO,
            }, None),
        ], Span::ZERO);
        let err = parse_path_template(&e).unwrap_err();
        assert!(matches!(err, PathError::UnsupportedInterpolation(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_con_params_duplicados_es_error() {
        // `"/a/{x}/b/{x}"`
        let e = Expr::StrInterp(vec![
            StrPart::Lit("/a/".into()),
            StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
            StrPart::Lit("/b/".into()),
            StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
        ], Span::ZERO);
        let err = parse_path_template(&e).unwrap_err();
        assert_eq!(err, PathError::DuplicateParam("x".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_no_string_literal_es_error() {
        // `@get(42)` — Int en lugar de string.
        let err = parse_path_template(&Expr::Int(42, Span::ZERO)).unwrap_err();
        assert_eq!(err, PathError::NotAStringLiteral);
    }

    // ---- Query params en el template ----

    #[tokio::test(flavor = "current_thread")]
    async fn query_template_separa_path_de_query_params() {
        // `"/items?limit={limit}&offset={offset}"` → path solo `/items`,
        // query_params `["limit", "offset"]` en orden.
        let e = Expr::StrInterp(
            vec![
                StrPart::Lit("/items?limit=".into()),
                StrPart::Expr(Expr::Ident("limit".into(), Span::ZERO), None),
                StrPart::Lit("&offset=".into()),
                StrPart::Expr(Expr::Ident("offset".into(), Span::ZERO), None),
            ],
            Span::ZERO,
        );
        let t = parse_path_template(&e).unwrap();
        assert_eq!(t.path, "/items");
        assert!(t.params.is_empty(), "no debería haber path params");
        assert_eq!(
            t.query_params,
            vec!["limit".to_string(), "offset".to_string()]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn query_template_combina_con_path_params() {
        // `"/users/{id}/posts?limit={limit}"` → path `/users/{id}/posts`,
        // path params `["id"]`, query params `["limit"]`.
        let e = Expr::StrInterp(
            vec![
                StrPart::Lit("/users/".into()),
                StrPart::Expr(Expr::Ident("id".into(), Span::ZERO), None),
                StrPart::Lit("/posts?limit=".into()),
                StrPart::Expr(Expr::Ident("limit".into(), Span::ZERO), None),
            ],
            Span::ZERO,
        );
        let t = parse_path_template(&e).unwrap();
        assert_eq!(t.path, "/users/{id}/posts");
        assert_eq!(t.params, vec!["id".to_string()]);
        assert_eq!(t.query_params, vec!["limit".to_string()]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn query_template_key_distinta_del_nombre_es_error() {
        // `"/x?l={limit}"` — key `l` no coincide con nombre `limit`.
        let e = Expr::StrInterp(
            vec![
                StrPart::Lit("/x?l=".into()),
                StrPart::Expr(Expr::Ident("limit".into(), Span::ZERO), None),
            ],
            Span::ZERO,
        );
        let err = parse_path_template(&e).unwrap_err();
        assert!(
            matches!(err, PathError::QueryKeyNameMismatch { .. }),
            "esperaba QueryKeyNameMismatch, fue: {:?}",
            err
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn query_template_malformado_es_error() {
        // `"/x?limit"` — falta `={name}`.
        let e = Expr::Str("/x?limit".into(), Span::ZERO);
        let err = parse_path_template(&e).unwrap_err();
        assert!(
            matches!(err, PathError::MalformedQueryTemplate(_)),
            "esperaba MalformedQueryTemplate, fue: {:?}",
            err
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn query_template_param_duplicado_con_path_es_error() {
        // `"/users/{id}?id={id}"` — `id` aparece en path y query.
        let e = Expr::StrInterp(
            vec![
                StrPart::Lit("/users/".into()),
                StrPart::Expr(Expr::Ident("id".into(), Span::ZERO), None),
                StrPart::Lit("?id=".into()),
                StrPart::Expr(Expr::Ident("id".into(), Span::ZERO), None),
            ],
            Span::ZERO,
        );
        let err = parse_path_template(&e).unwrap_err();
        // El parser dispara DuplicateParam al ver el segundo `{id}` en
        // la primera pasada (antes de separar path de query). Eso es
        // OK — el mensaje es claro al usuario igualmente.
        assert_eq!(err, PathError::DuplicateParam("id".into()));
    }

    // ---- value_to_json ----

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_primitivos() {
        assert_eq!(value_to_json(&Value::Int(42)).unwrap(), serde_json::json!(42));
        assert_eq!(value_to_json(&Value::Float(3.14)).unwrap(), serde_json::json!(3.14));
        assert_eq!(value_to_json(&Value::Str("hola".into())).unwrap(), serde_json::json!("hola"));
        assert_eq!(value_to_json(&Value::Bool(true)).unwrap(), serde_json::json!(true));
        assert_eq!(value_to_json(&Value::Null).unwrap(), serde_json::json!(null));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_lista() {
        let v = Value::List(shared(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ]));
        assert_eq!(value_to_json(&v).unwrap(), serde_json::json!([1, 2, 3]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_mapa_con_claves_string() {
        let v = Value::Map(shared(vec![
            (Value::Str("name".into()), Value::Str("fitz".into())),
            (Value::Str("port".into()), Value::Int(3000)),
        ]));
        assert_eq!(
            value_to_json(&v).unwrap(),
            serde_json::json!({ "name": "fitz", "port": 3000 }),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_mapa_clave_no_string_es_error() {
        let v = Value::Map(shared(vec![(Value::Int(1), Value::Int(10))]));
        let err = value_to_json(&v).unwrap_err();
        assert!(err.contains("claves de Map en JSON"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_instance() {
        // Instance `{id: 1, name: "x"}` → `{"id": 1, "name": "x"}`.
        let inst = Value::new_instance(
            "User".into(),
            vec![
                ("id".into(), Value::Int(1)),
                ("name".into(), Value::Str("x".into())),
            ],
        );
        assert_eq!(
            value_to_json(&inst).unwrap(),
            serde_json::json!({ "id": 1, "name": "x" }),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_result_anidado_se_etiqueta() {
        // `Ok(42)` adentro de otra cosa (no debería pasar en el output
        // directo del handler, pero queremos un comportamiento total).
        let ok = Value::Result(ResultVariant::Ok(Box::new(Value::Int(42))));
        assert_eq!(value_to_json(&ok).unwrap(), serde_json::json!({ "Ok": 42 }));

        let err = Value::Result(ResultVariant::Err(Box::new(Value::Str("boom".into()))));
        assert_eq!(value_to_json(&err).unwrap(), serde_json::json!({ "Err": "boom" }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_function_es_error() {
        // Function no es serializable.
        let env = crate::env::Environment::new();
        let v = Value::Function {
            params: vec![],
            body: vec![],
            closure: env,
            is_async: false,
        };
        let err = value_to_json(&v).unwrap_err();
        assert!(err.contains("Function"));
    }

    // ---- value_to_outcome (handler → status + body) ----

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_de_value_pelado_es_200() {
        let v = Value::Str("hola".into());
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 200);
        assert_eq!(out.body, "\"hola\"");
        assert_eq!(out.content_type, "application/json");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_de_ok_es_200_con_inner() {
        let v = Value::Result(ResultVariant::Ok(Box::new(Value::Int(42))));
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 200);
        assert_eq!(out.body, "42");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_de_err_es_500_con_error_obj() {
        let v = Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "no encontrado".into(),
        ))));
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 500);
        // Body es `{"error":"no encontrado"}` (orden de serde_json).
        assert_eq!(out.body, "{\"error\":\"no encontrado\"}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_de_instance_es_objeto_json() {
        let inst = Value::new_instance(
            "User".into(),
            vec![
                ("id".into(), Value::Int(7)),
                ("name".into(), Value::Str("ana".into())),
            ],
        );
        let out = value_to_outcome(&inst);
        assert_eq!(out.status, 200);
        // serde_json::Map preserva orden de inserción con la feature
        // `preserve_order` activada; sin ella, el orden es indefinido.
        // Acá no asumimos orden: parseamos el body y comparamos.
        let parsed: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "id": 7, "name": "ana" }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_de_tipo_no_serializable_es_500() {
        // Range no es serializable.
        let v = Value::Range { start: 0, end: 10 };
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 500);
        assert!(out.body.contains("Range"));
    }

    // ---- Status codes custom (Value::HttpResponse) ----

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_de_http_response_usa_su_status_y_body() {
        // El evaluator produce `Value::HttpResponse` cuando el usuario
        // hace `return 401 { ... }`. El outcome usa el status del
        // response y serializa el body con las reglas habituales.
        let body = Value::new_instance(
            "Error".into(),
            vec![("message".into(), Value::Str("no autorizado".into()))],
        );
        let v = Value::HttpResponse {
            status: 401,
            body: Some(Box::new(body)),
        };
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 401);
        let parsed: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "message": "no autorizado" }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_de_http_response_sin_body_es_null_json() {
        // `HttpResponse { body: None }` → body JSON null. Reserva para
        // 204 No Content si llega; hoy el parser exige body explícito.
        let v = Value::HttpResponse { status: 204, body: None };
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 204);
        assert_eq!(out.body, "null");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_de_http_response_con_body_map_serializa_a_objeto() {
        // Body = map literal con string keys → objeto JSON.
        let body = Value::new_map(vec![
            (Value::Str("error".into()), Value::Str("falló".into())),
            (Value::Str("code".into()), Value::Int(42)),
        ]);
        let v = Value::HttpResponse {
            status: 500,
            body: Some(Box::new(body)),
        };
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 500);
        let parsed: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "error": "falló", "code": 42 }));
    }

    // ---- coerce_path_param ----

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_default_a_str_sin_anotacion() {
        let v = coerce_path_param("42", None).unwrap();
        assert_eq!(v, Value::Str("42".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_int_se_parsea_a_int() {
        let v = coerce_path_param("42", Some("Int")).unwrap();
        assert_eq!(v, Value::Int(42));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_int_invalido_es_error() {
        let err = coerce_path_param("abc", Some("Int")).unwrap_err();
        assert!(err.contains("Int") && err.contains("abc"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_float_se_parsea() {
        let v = coerce_path_param("3.14", Some("Float")).unwrap();
        assert_eq!(v, Value::Float(3.14));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_bool_true_false() {
        assert_eq!(coerce_path_param("true", Some("Bool")).unwrap(), Value::Bool(true));
        assert_eq!(coerce_path_param("false", Some("Bool")).unwrap(), Value::Bool(false));
        assert!(coerce_path_param("maybe", Some("Bool")).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_tipo_no_soportado_es_error() {
        // Un tipo custom no entra como path param: el handler tiene
        // que recibir el id raw y reconstruir el objeto adentro.
        let err = coerce_path_param("42", Some("User")).unwrap_err();
        assert!(err.contains("User"));
    }

    // ---- registry ----

    #[tokio::test(flavor = "current_thread")]
    async fn registry_arranca_sin_rutas() {
        let r = HttpRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.routes.len(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_active_registry_expone_has_active_para_el_evaluator() {
        // Afuera: no hay registry, los decorators dan error explícito.
        assert!(!has_active_registry());

        let ((), reg) = with_active_registry(|| {
            // Adentro: el evaluator ve registry activo.
            assert!(has_active_registry());
        });

        // Devuelto vacío (nadie pusheó), y afuera sigue sin haber.
        assert!(reg.is_empty());
        assert!(!has_active_registry());
    }

    // ---- handle_task (lado del intérprete, sin tokio) ----

    /// Helper: construye un `HttpRegistry` con una sola ruta a partir
    /// de una fuente Fitz que la registra. Aprovecha el evaluator
    /// real, así no construimos `Value::Function` a mano (que es
    /// frágil — capturar el closure correcto importa).
    ///
    /// Fase 6.4: pasa a `async fn` porque `eval` ahora es async.
    /// Los call sites suman `.await`.
    async fn registry_from_source(src: &str) -> HttpRegistry {
        let (res, registry) = with_active_registry_async(|| async {
            let tokens = crate::lexer::tokenize(src).unwrap();
            let program = crate::parser::parse(tokens).unwrap();
            crate::evaluator::eval(program).await
        })
        .await;
        res.unwrap();
        registry
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_invoca_handler_y_devuelve_outcome() {
        // `@get("/") fn hello() => "hola"`
        let src = "@get(\"/\")\nfn hello() => \"hola\"";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), Vec::new(), HashMap::new()).await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"hola\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_coerciona_path_param_int() {
        let src = "@get(\"/users/{id}\")\nfn h(id: Int) => id * 2";
        let registry = registry_from_source(src).await;
        let mut params = HashMap::new();
        params.insert("id".into(), "21".into());
        let outcome = handle_task(&registry, 0, params, HashMap::new(), Vec::new(), HashMap::new()).await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "42");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_path_param_int_invalido_es_400() {
        let src = "@get(\"/users/{id}\")\nfn h(id: Int) => id";
        let registry = registry_from_source(src).await;
        let mut params = HashMap::new();
        params.insert("id".into(), "no-es-int".into());
        let outcome = handle_task(&registry, 0, params, HashMap::new(), Vec::new(), HashMap::new()).await;
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("Int"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_handler_que_retorna_err_es_500_con_error() {
        // El handler devuelve Err("boom"): runtime lo traduce a 500.
        let src = "@get(\"/\")\nfn h() => Err(\"boom\")";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), Vec::new(), HashMap::new()).await;
        assert_eq!(outcome.status, 500);
        assert!(outcome.body.contains("boom"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_handler_que_retorna_instance_serializa_a_json() {
        let src = "\
            type User { id: Int, name: Str }\n\
            @get(\"/u\")\nfn h() => User { id: 1, name: \"ana\" }\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), Vec::new(), HashMap::new()).await;
        assert_eq!(outcome.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&outcome.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "id": 1, "name": "ana" }));
    }

    // ---- Mini-fase MW.1: middleware chain en handle_task ----

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_middleware_que_retorna_null_continua_al_handler() {
        // Middleware "passthrough": no devuelve nada → la cadena sigue
        // y el handler corre normal.
        let src = "\
            fn pass(req) {}\n\
            @middleware(pass)\n\
            @get(\"/\")\n\
            fn h() => \"ok\"\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"ok\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_middleware_que_short_circuita_con_401() {
        // Middleware corta la cadena con `return 401 { ... }`. El handler
        // NO se invoca y la response es la del middleware.
        let src = "\
            fn auth(req) {\n\
                return 401 {\"error\": \"no autorizado\"}\n\
            }\n\
            @middleware(auth)\n\
            @get(\"/\")\n\
            fn h() => \"NO DEBERIA APARECER\"\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 401);
        assert!(outcome.body.contains("no autorizado"));
        assert!(!outcome.body.contains("NO DEBERIA APARECER"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_dos_middlewares_short_circuita_el_primero_que_corte() {
        // Primero `logger` (pass), después `auth` (corta). El handler
        // no debería correr. Si invertimos el orden y el corte aterriza
        // primero, lo verificamos abajo.
        let src = "\
            fn logger(req) {}\n\
            fn auth(req) {\n\
                return 403 {\"error\": \"forbidden\"}\n\
            }\n\
            @middleware(logger)\n\
            @middleware(auth)\n\
            @get(\"/\")\n\
            fn h() => \"nope\"\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 403);
        assert!(outcome.body.contains("forbidden"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_middleware_lee_method_y_path_del_request() {
        // El middleware inspecciona req.method y req.path. Verifica
        // que el path lleva los path params SUSTITUIDOS, no la
        // template (mini-fase MW.1: `/users/{id}` → `/users/42`).
        let src = "\
            fn debug_mw(req) {\n\
                return 200 {\"method\": req.method, \"path\": req.path}\n\
            }\n\
            @middleware(debug_mw)\n\
            @get(\"/users/{id}\")\n\
            fn h(id: Int) => \"nope\"\n\
        ";
        let registry = registry_from_source(src).await;
        let mut params = HashMap::new();
        params.insert("id".into(), "42".into());
        let outcome = handle_task(
            &registry,
            0,
            params,
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&outcome.body).unwrap();
        assert_eq!(parsed["method"], "GET");
        assert_eq!(parsed["path"], "/users/42");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_middleware_lee_headers_lowercase() {
        // Headers expuestos al middleware con keys en lowercase (mismo
        // criterio que el dispatch de @header).
        let src = "\
            fn auth(req) {\n\
                return 200 {\"token\": req.headers[\"authorization\"]}\n\
            }\n\
            @middleware(auth)\n\
            @get(\"/\")\n\
            fn h() => \"nope\"\n\
        ";
        let registry = registry_from_source(src).await;
        let mut headers = HashMap::new();
        headers.insert("authorization".into(), "bearer-xyz".into());
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            headers,
        )
        .await;
        assert_eq!(outcome.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&outcome.body).unwrap();
        assert_eq!(parsed["token"], "bearer-xyz");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_middleware_que_retorna_valor_invalido_es_500() {
        // Si el middleware devuelve cualquier cosa que no sea Null ni
        // HttpResponse (Int, Str, Instance, ...), el runtime emite 500
        // con mensaje claro citando "gate-only".
        let src = "\
            fn loco(req) => 42\n\
            @middleware(loco)\n\
            @get(\"/\")\n\
            fn h() => \"nope\"\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 500);
        assert!(outcome.body.contains("loco"));
        assert!(outcome.body.contains("valor inesperado") || outcome.body.contains("cortocircuitar"));
    }

    // ---- Mini-fase MW.2: cors built-in + inyección de headers ----

    #[tokio::test(flavor = "current_thread")]
    async fn cors_response_headers_emite_los_tres_headers_basicos() {
        let cfg = CorsConfig::permissive_default();
        let headers = cfg.response_headers(None);
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"access-control-allow-origin"));
        assert!(names.contains(&"access-control-allow-methods"));
        assert!(names.contains(&"access-control-allow-headers"));
        // max_age default es None → no se emite ese header.
        assert!(!names.contains(&"access-control-max-age"));
    }

    // ---- Q.3: AllowOrigin Set + echo del Origin del request ----

    #[tokio::test(flavor = "current_thread")]
    async fn cors_set_echo_si_origin_esta_en_la_lista() {
        let cfg = CorsConfig {
            allow_origin: AllowOrigin::Set(vec![
                "https://a.com".into(),
                "https://b.com".into(),
            ]),
            ..CorsConfig::permissive_default()
        };
        let headers = cfg.response_headers(Some("https://a.com"));
        let origin = headers
            .iter()
            .find(|(n, _)| n == "access-control-allow-origin")
            .map(|(_, v)| v.clone());
        assert_eq!(origin, Some("https://a.com".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_set_omite_origin_header_si_request_no_matchea() {
        let cfg = CorsConfig {
            allow_origin: AllowOrigin::Set(vec!["https://a.com".into()]),
            ..CorsConfig::permissive_default()
        };
        // Origin del request NO está en la lista → el header
        // access-control-allow-origin NO se emite; el browser
        // rechaza la response.
        let headers = cfg.response_headers(Some("https://evil.com"));
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"access-control-allow-origin"));
        // El resto de headers CORS sí se emiten (no son request-aware).
        assert!(names.contains(&"access-control-allow-methods"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_set_omite_origin_si_request_no_trae_origin() {
        // Sin header `Origin` (request same-origin, browser no lo manda),
        // el modo Set tampoco emite — no hay nada que echo. El browser
        // de all modos no lo necesitaría en ese caso.
        let cfg = CorsConfig {
            allow_origin: AllowOrigin::Set(vec!["https://a.com".into()]),
            ..CorsConfig::permissive_default()
        };
        let headers = cfg.response_headers(None);
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"access-control-allow-origin"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_literal_ignora_el_origin_del_request() {
        // Literal emite siempre el mismo valor, sin importar el request.
        let cfg = CorsConfig {
            allow_origin: AllowOrigin::Literal("*".into()),
            ..CorsConfig::permissive_default()
        };
        let headers_with = cfg.response_headers(Some("https://x.com"));
        let headers_without = cfg.response_headers(None);
        assert_eq!(headers_with, headers_without);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn allow_origin_resolve_set_match_y_miss() {
        let any = AllowOrigin::Literal("*".to_string());
        assert_eq!(any.resolve(None), Some("*".to_string()));
        assert_eq!(any.resolve(Some("https://x.com")), Some("*".to_string()));

        let single = AllowOrigin::Literal("https://x.com".to_string());
        assert_eq!(single.resolve(Some("https://y.com")), Some("https://x.com".to_string()));

        let set = AllowOrigin::Set(vec!["https://a.com".into(), "https://b.com".into()]);
        assert_eq!(set.resolve(Some("https://b.com")), Some("https://b.com".to_string()));
        assert_eq!(set.resolve(Some("https://evil.com")), None);
        assert_eq!(set.resolve(None), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_response_headers_emite_max_age_cuando_esta_seteado() {
        let cfg = CorsConfig {
            max_age: Some(3600),
            ..CorsConfig::permissive_default()
        };
        let headers = cfg.response_headers(None);
        let max_age = headers
            .iter()
            .find(|(n, _)| n == "access-control-max-age")
            .map(|(_, v)| v.clone());
        assert_eq!(max_age, Some("3600".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_inyecta_headers_cors_en_response_real() {
        // Handler normal + @middleware(cors()) → la response 200 carga
        // los headers Access-Control-Allow-*.
        let src = "\
            @middleware(cors())\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 200);
        let names: Vec<&str> = outcome.extra_headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"access-control-allow-origin"));
        assert!(names.contains(&"access-control-allow-methods"));
        assert!(names.contains(&"access-control-allow-headers"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_inyecta_headers_cors_incluso_en_500_de_error() {
        // Si el handler devuelve Err(...), la response es 500 PERO igual
        // lleva los headers CORS. Sin esto el browser ve "CORS error" en
        // lugar del 500 que de verdad pasó.
        let src = "\
            @middleware(cors())\n\
            @get(\"/\")\n\
            fn h() => Err(\"boom\")\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 500);
        let names: Vec<&str> = outcome.extra_headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"access-control-allow-origin"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_custom_origin_se_propaga_a_headers() {
        let src = "\
            @middleware(cors({\"allow_origin\": \"https://app.x.com\"}))\n\
            @get(\"/\")\n\
            fn h() => \"ok\"\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        let origin = outcome
            .extra_headers
            .iter()
            .find(|(n, _)| n == "access-control-allow-origin")
            .map(|(_, v)| v.clone());
        assert_eq!(origin, Some("https://app.x.com".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_cors_set_echo_request_origin_si_matchea() {
        // Q.3: cors con lista de orígenes permitidos. Request con
        // `Origin: https://a.com` en la lista → echo del origin.
        let src = "\
            @middleware(cors({\"allow_origin\": [\"https://a.com\", \"https://b.com\"]}))\n\
            @get(\"/\")\n\
            fn h() => \"ok\"\n\
        ";
        let registry = registry_from_source(src).await;
        let mut headers = HashMap::new();
        headers.insert("origin".into(), "https://a.com".into());
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            headers,
        )
        .await;
        let origin = outcome
            .extra_headers
            .iter()
            .find(|(n, _)| n == "access-control-allow-origin")
            .map(|(_, v)| v.clone());
        assert_eq!(origin, Some("https://a.com".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_cors_set_omite_origin_si_no_matchea() {
        let src = "\
            @middleware(cors({\"allow_origin\": [\"https://a.com\"]}))\n\
            @get(\"/\")\n\
            fn h() => \"ok\"\n\
        ";
        let registry = registry_from_source(src).await;
        let mut headers = HashMap::new();
        headers.insert("origin".into(), "https://evil.com".into());
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            headers,
        )
        .await;
        let names: Vec<&str> = outcome.extra_headers.iter().map(|(n, _)| n.as_str()).collect();
        // El header origin NO se emite (browser rechaza la response).
        assert!(!names.contains(&"access-control-allow-origin"));
        // El resto de headers CORS sí.
        assert!(names.contains(&"access-control-allow-methods"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_sin_cors_no_emite_headers_extras() {
        // Sanity: handler sin @middleware(cors(...)) no debe traer
        // headers extras (no contaminación).
        let src = "@get(\"/\")\nfn h() => \"ok\"";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert!(outcome.extra_headers.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_middleware_corta_antes_de_parsear_body() {
        // Si el middleware short-circuita, el body NO se parsea (el
        // 400 por body inválido que normalmente saldría no aparece).
        // Esto chequea que el orden es middlewares → parse body → handler.
        let src = "\
            type Input { x: Int }\n\
            fn deny(req) {\n\
                return 401 {\"error\": \"nope\"}\n\
            }\n\
            @middleware(deny)\n\
            @post(\"/\")\n\
            fn h(body: Input) => body\n\
        ";
        let registry = registry_from_source(src).await;
        // Body inválido (no es JSON) — si llegara al parser, daría 400.
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            b"esto-no-es-json".to_vec(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 401);
        assert!(outcome.body.contains("nope"));
    }

    // ---- ServerConfig (Fase 4.4) ----

    #[tokio::test(flavor = "current_thread")]
    async fn server_config_default_es_localhost_3000() {
        let c = ServerConfig::default_addr();
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.port, 3000);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_config_to_socket_addr_ipv4_ok() {
        let c = ServerConfig {
            host: "0.0.0.0".into(),
            port: 8080,
            enable_docs: true,
            api_version: None,
        };
        let addr = c.to_socket_addr().unwrap();
        assert_eq!(addr.to_string(), "0.0.0.0:8080");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_config_to_socket_addr_host_invalido_es_error() {
        let c = ServerConfig {
            host: "no-es-ip".into(),
            port: 80,
            enable_docs: true,
            api_version: None,
        };
        let err = c.to_socket_addr().unwrap_err();
        assert!(err.contains("no-es-ip"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_server_config_segunda_vez_devuelve_existente() {
        let ((), _reg) = with_active_registry(|| {
            let first = ServerConfig {
                host: "127.0.0.1".into(),
                port: 8080,
                enable_docs: true,
                api_version: None,
            };
            assert!(set_server_config(first.clone()).is_ok());
            let second = ServerConfig {
                host: "0.0.0.0".into(),
                port: 9090,
                enable_docs: true,
                api_version: None,
            };
            let err = set_server_config(second).unwrap_err();
            // El error contiene el config existente, no el nuevo.
            assert_eq!(err, first);
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn registry_resolved_config_devuelve_default_si_no_hay_explicito() {
        let mut reg = HttpRegistry::new();
        assert!(reg.server_config.is_none());
        assert_eq!(reg.resolved_config(), ServerConfig::default_addr());
        // Con config explícito sí.
        reg.server_config = Some(ServerConfig {
            host: "0.0.0.0".into(),
            port: 80,
            enable_docs: true,
            api_version: None,
        });
        let resolved = reg.resolved_config();
        assert_eq!(resolved.port, 80);
        assert_eq!(resolved.host, "0.0.0.0");
    }

    // ---- json_to_value (deserialización libre) ----

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_value_primitivos() {
        assert_eq!(json_to_value(&serde_json::json!(null)), Value::Null);
        assert_eq!(json_to_value(&serde_json::json!(true)), Value::Bool(true));
        assert_eq!(json_to_value(&serde_json::json!(42)), Value::Int(42));
        assert_eq!(json_to_value(&serde_json::json!(3.14)), Value::Float(3.14));
        assert_eq!(
            json_to_value(&serde_json::json!("hola")),
            Value::Str("hola".into())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_value_array_se_vuelve_list() {
        let v = json_to_value(&serde_json::json!([1, 2, "tres"]));
        match v {
            Value::List(items) => {
                let items = items.lock();
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Int(1));
                assert_eq!(items[2], Value::Str("tres".into()));
            }
            _ => panic!("se esperaba List"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_value_object_se_vuelve_map_con_claves_str() {
        let v = json_to_value(&serde_json::json!({ "a": 1, "b": "x" }));
        match v {
            Value::Map(pairs) => {
                let pairs = pairs.lock();
                assert_eq!(pairs.len(), 2);
                // El orden de serde_json::Map depende de la feature
                // `preserve_order`. No la asumimos: convertimos a un
                // map auxiliar para comparar.
                let as_map: std::collections::HashMap<String, Value> = pairs
                    .iter()
                    .map(|(k, v)| {
                        let k = match k {
                            Value::Str(s) => s.clone(),
                            _ => panic!("clave no Str"),
                        };
                        (k, v.clone())
                    })
                    .collect();
                assert_eq!(as_map.get("a"), Some(&Value::Int(1)));
                assert_eq!(as_map.get("b"), Some(&Value::Str("x".into())));
            }
            _ => panic!("se esperaba Map"),
        }
    }

    // ---- json_to_instance (validación contra Value::Type) ----

    /// Helper: arma un `Value::Type` con los campos dados. Cada
    /// campo es `(nombre, tipo, nullable, default)`. El flag `nullable`
    /// se traduce a `TypeExpr::Nullable(Named(t))`.
    fn type_value(name: &str, fields: Vec<(&str, &str, bool, Option<Expr>)>) -> Value {
        use crate::ast::TypeExpr;
        Value::Type {
            name: name.into(),
            fields: fields
                .into_iter()
                .map(|(n, t, nullable, default)| {
                    let base = TypeExpr::named(t);
                    let type_ = if nullable {
                        TypeExpr::Nullable(Box::new(base))
                    } else {
                        base
                    };
                    crate::ast::Field {
                        name: n.into(),
                        type_,
                        default,
                    }
                })
                .collect(),
            resolved_defaults: vec![],
            methods: vec![],
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_caso_feliz() {
        let t = type_value("User", vec![
            ("id", "Int", false, None),
            ("name", "Str", false, None),
        ]);
        let json = serde_json::json!({ "id": 1, "name": "ana" });
        let v = json_to_instance(&json, &t).unwrap();
        match v {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "User");
                let fields = fields.lock();
                assert_eq!(fields[0].0, "id");
                assert_eq!(fields[0].1, Value::Int(1));
                assert_eq!(fields[1].0, "name");
                assert_eq!(fields[1].1, Value::Str("ana".into()));
            }
            _ => panic!("se esperaba Instance"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_campo_faltante_sin_default_ni_nullable_es_error() {
        let t = type_value("User", vec![
            ("id", "Int", false, None),
            ("name", "Str", false, None),
        ]);
        let json = serde_json::json!({ "id": 1 });
        let err = json_to_instance(&json, &t).unwrap_err();
        assert!(err.contains("name"));
        assert!(err.contains("falta"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_campo_extra_es_error() {
        let t = type_value("User", vec![("id", "Int", false, None)]);
        let json = serde_json::json!({ "id": 1, "rogue": "x" });
        let err = json_to_instance(&json, &t).unwrap_err();
        assert!(err.contains("rogue"));
        assert!(err.contains("no declarado"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_campo_nullable_faltante_queda_null() {
        let t = type_value("User", vec![
            ("id", "Int", false, None),
            ("email", "Str", true, None),
        ]);
        let json = serde_json::json!({ "id": 1 });
        let v = json_to_instance(&json, &t).unwrap();
        match v {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[1].0, "email");
                assert_eq!(fields[1].1, Value::Null);
            }
            _ => panic!("se esperaba Instance"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_default_literal_se_usa_si_falta() {
        let t = type_value("User", vec![
            ("id", "Int", false, None),
            ("active", "Bool", false, Some(Expr::Bool(true, Span::ZERO))),
        ]);
        let json = serde_json::json!({ "id": 1 });
        let v = json_to_instance(&json, &t).unwrap();
        match v {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[1].0, "active");
                assert_eq!(fields[1].1, Value::Bool(true));
            }
            _ => panic!("se esperaba Instance"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_body_no_objeto_es_error() {
        let t = type_value("User", vec![("id", "Int", false, None)]);
        let json = serde_json::json!([1, 2, 3]);
        let err = json_to_instance(&json, &t).unwrap_err();
        assert!(err.contains("objeto"));
        assert!(err.contains("array"));
    }

    // ---- handle_task con body ----

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_post_sin_body_pero_handler_lo_espera_es_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), Vec::new(), HashMap::new()).await;
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("body requerido"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_post_con_body_valido_construye_instance() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body.name\n\
        ";
        let registry = registry_from_source(src).await;
        let body = br#"{"name":"fitz"}"#.to_vec();
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), body, HashMap::new()).await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"fitz\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_post_body_json_invalido_es_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), b"not json".to_vec(), HashMap::new()).await;
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("JSON"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_post_body_campo_faltante_es_400() {
        let src = "\
            type UserInput { name: Str, email: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let registry = registry_from_source(src).await;
        let body = br#"{"name":"fitz"}"#.to_vec();
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), body, HashMap::new()).await;
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("email"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_put_con_path_param_y_body() {
        let src = "\
            type UserInput { name: Str }\n\
            @put(\"/users/{id}\")\nfn upd(id: Int, body: UserInput) => body.name\n\
        ";
        let registry = registry_from_source(src).await;
        let mut params = HashMap::new();
        params.insert("id".into(), "7".into());
        let body = br#"{"name":"ana"}"#.to_vec();
        let outcome = handle_task(&registry, 0, params, HashMap::new(), body, HashMap::new()).await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"ana\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_body_sin_anotacion_de_tipo_acepta_libre() {
        // `body` sin tipo → llega como Map<Str,Value>.
        let src = "\
            @post(\"/log\")\nfn log(body) => body[\"name\"]\n\
        ";
        let registry = registry_from_source(src).await;
        let body = br#"{"name":"x"}"#.to_vec();
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), body, HashMap::new()).await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"x\"");
    }

    // ---- build_router + oneshot E2E ----
    //
    // Estos tests arman un router de axum y le mandan requests sin
    // abrir socket TCP, vía `tower::ServiceExt::oneshot`.
    //
    // Post-F17.5: cero glue. El registry se envuelve en `Arc` y se
    // pasa a `build_router`; cada handler axum invoca al evaluator
    // directo. Antes hacía falta un `LocalSet` + un loop tokio::select!
    // sobre `mpsc::recv` para coexistir con el bridge — eso desapareció.

    /// Helper: corre un request contra el router y devuelve
    /// (status, body string). Sin body, sin headers extra.
    async fn run_oneshot(
        src: &str,
        method: axum::http::Method,
        path: &str,
    ) -> (u16, String) {
        run_oneshot_with_body(src, method, path, None).await
    }

    /// Como `run_oneshot_with_body` pero acepta también una lista de
    /// headers `(name, value)` que se agregan a la request. Útil para
    /// los tests de `@header(...)` (Fase 7.6).
    async fn run_oneshot_with_headers(
        src: &str,
        method: axum::http::Method,
        path: &str,
        headers: &[(&str, &str)],
    ) -> (u16, String) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let registry = registry_from_source(src).await;
        let metas = registry.metas();
        let router = build_router(&metas, std::sync::Arc::new(registry), None);

        let mut builder = axum::http::Request::builder().method(method).uri(path);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let req = builder.body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// Como `run_oneshot` pero con body opcional. Si `body` es
    /// `Some(s)`, se manda como `application/json` (aunque el runtime
    /// hoy no valida content-type).
    async fn run_oneshot_with_body(
        src: &str,
        method: axum::http::Method,
        path: &str,
        body: Option<&'static str>,
    ) -> (u16, String) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let registry = registry_from_source(src).await;
        let metas = registry.metas();
        // Tests existentes de routing: schema = None para no contaminar
        // los path lookups con la ruta auto-registrada de 7.2.
        let router = build_router(&metas, std::sync::Arc::new(registry), None);

        let req_body = match body {
            Some(s) => Body::from(s),
            None => Body::empty(),
        };
        let req = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(req_body)
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// Como `run_oneshot` pero devuelve además los headers de la
    /// response (un Vec<(name, value)> en lowercase). Usado por los
    /// tests CORS de MW.2 para verificar `Access-Control-Allow-*`.
    async fn run_oneshot_full(
        src: &str,
        method: axum::http::Method,
        path: &str,
    ) -> (u16, Vec<(String, String)>, String) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let registry = registry_from_source(src).await;
        let metas = registry.metas();
        let router = build_router(&metas, std::sync::Arc::new(registry), None);

        let req = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_lowercase(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn e2e_preflight_options_responde_204_con_headers_cors() {
        // OPTIONS sobre una ruta con @middleware(cors(...)) devuelve 204
        // y los headers Access-Control-Allow-*. El handler real (GET) NO
        // se invoca — axum routea OPTIONS al preflight handler dedicado.
        let src = "\
            @middleware(cors())\n\
            @get(\"/api\")\n\
            fn h() => \"nope\"\n\
        ";
        let (status, headers, body) =
            run_oneshot_full(src, axum::http::Method::OPTIONS, "/api").await;
        assert_eq!(status, 204);
        assert!(body.is_empty());
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"access-control-allow-origin"));
        assert!(names.contains(&"access-control-allow-methods"));
        assert!(names.contains(&"access-control-allow-headers"));
    }

    #[tokio::test]
    async fn e2e_options_sin_cors_es_405_method_not_allowed() {
        // Si la ruta NO tiene @middleware(cors(...)), un OPTIONS responde
        // 405 (axum default — el método no está registrado para ese path).
        // Sanity: sin CORS, no creamos preflight handler.
        let src = "@get(\"/api\")\nfn h() => \"ok\"";
        let (status, _, _) =
            run_oneshot_full(src, axum::http::Method::OPTIONS, "/api").await;
        assert_eq!(status, 405);
    }

    #[tokio::test]
    async fn e2e_response_real_con_cors_lleva_headers_inyectados() {
        // GET normal sobre ruta con cors → 200 + headers Access-Control-Allow-*.
        let src = "\
            @middleware(cors({\"allow_origin\": \"https://x.com\"}))\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let (status, headers, body) =
            run_oneshot_full(src, axum::http::Method::GET, "/api").await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"ok\"");
        let origin = headers
            .iter()
            .find(|(n, _)| n == "access-control-allow-origin")
            .map(|(_, v)| v.clone());
        assert_eq!(origin, Some("https://x.com".to_string()));
    }

    #[tokio::test]
    async fn e2e_preflight_set_echo_si_origin_en_la_lista() {
        // Q.3: preflight con cors({"allow_origin": [...]}) hace echo
        // del Origin si está permitido.
        let src = "\
            @middleware(cors({\"allow_origin\": [\"https://a.com\", \"https://b.com\"]}))\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let (status, headers, _) = run_oneshot_full_with_headers(
            src,
            axum::http::Method::OPTIONS,
            "/api",
            &[("origin", "https://b.com")],
        )
        .await;
        assert_eq!(status, 204);
        let origin = headers
            .iter()
            .find(|(n, _)| n == "access-control-allow-origin")
            .map(|(_, v)| v.clone());
        assert_eq!(origin, Some("https://b.com".to_string()));
    }

    #[tokio::test]
    async fn e2e_preflight_set_sin_match_omite_origin() {
        let src = "\
            @middleware(cors({\"allow_origin\": [\"https://a.com\"]}))\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let (status, headers, _) = run_oneshot_full_with_headers(
            src,
            axum::http::Method::OPTIONS,
            "/api",
            &[("origin", "https://evil.com")],
        )
        .await;
        assert_eq!(status, 204);
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"access-control-allow-origin"));
        assert!(names.contains(&"access-control-allow-methods"));
    }

    /// Variante de `run_oneshot_full` que acepta headers extra para
    /// la request (Q.3: para mandar `Origin: ...` y verificar echo).
    async fn run_oneshot_full_with_headers(
        src: &str,
        method: axum::http::Method,
        path: &str,
        headers: &[(&str, &str)],
    ) -> (u16, Vec<(String, String)>, String) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let registry = registry_from_source(src).await;
        let metas = registry.metas();
        let router = build_router(&metas, std::sync::Arc::new(registry), None);

        let mut builder = axum::http::Request::builder().method(method).uri(path);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let req = builder.body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let response_headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_lowercase(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, response_headers, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn e2e_preflight_max_age_se_emite_solo_si_fue_seteado() {
        let src = "\
            @middleware(cors({\"max_age\": 3600}))\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let (status, headers, _) =
            run_oneshot_full(src, axum::http::Method::OPTIONS, "/api").await;
        assert_eq!(status, 204);
        let max_age = headers
            .iter()
            .find(|(n, _)| n == "access-control-max-age")
            .map(|(_, v)| v.clone());
        assert_eq!(max_age, Some("3600".to_string()));
    }

    #[tokio::test]
    async fn e2e_get_simple_responde_200_con_json() {
        let (status, body) = run_oneshot(
            "@get(\"/\")\nfn index() => \"hola\"",
            axum::http::Method::GET,
            "/",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"hola\"");
    }

    #[tokio::test]
    async fn e2e_get_con_path_param_int() {
        let (status, body) = run_oneshot(
            "@get(\"/users/{id}\")\nfn h(id: Int) => id * 10",
            axum::http::Method::GET,
            "/users/7",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, "70");
    }

    #[tokio::test]
    async fn e2e_get_con_path_param_invalido_devuelve_400() {
        let (status, body) = run_oneshot(
            "@get(\"/users/{id}\")\nfn h(id: Int) => id",
            axum::http::Method::GET,
            "/users/abc",
        )
        .await;
        assert_eq!(status, 400);
        assert!(body.contains("Int"));
    }

    #[tokio::test]
    async fn e2e_handler_que_retorna_instance_serializa_a_json() {
        let src = "\
            type User { id: Int, name: Str }\n\
            @get(\"/me\")\nfn me() => User { id: 1, name: \"fitz\" }\n\
        ";
        let (status, body) = run_oneshot(src, axum::http::Method::GET, "/me").await;
        assert_eq!(status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "id": 1, "name": "fitz" }));
    }

    #[tokio::test]
    async fn e2e_method_no_coincide_devuelve_405() {
        let (status, _body) = run_oneshot(
            "@get(\"/\")\nfn h() => \"ok\"",
            axum::http::Method::POST,
            "/",
        )
        .await;
        assert_eq!(status, 405);
    }

    #[tokio::test]
    async fn e2e_path_no_existe_devuelve_404() {
        let (status, _body) = run_oneshot(
            "@get(\"/foo\")\nfn h() => \"ok\"",
            axum::http::Method::GET,
            "/bar",
        )
        .await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn e2e_handler_err_devuelve_500_con_objeto_error() {
        let (status, body) = run_oneshot(
            "@get(\"/\")\nfn h() => Err(\"boom\")",
            axum::http::Method::GET,
            "/",
        )
        .await;
        assert_eq!(status, 500);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "error": "boom" }));
    }

    #[tokio::test]
    async fn e2e_post_con_body_valido_construye_instance() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body.name\n\
        ";
        let (status, body) = run_oneshot_with_body(
            src,
            axum::http::Method::POST,
            "/users",
            Some(r#"{"name":"fitz"}"#),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"fitz\"");
    }

    #[tokio::test]
    async fn e2e_post_body_invalido_devuelve_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let (status, body) = run_oneshot_with_body(
            src,
            axum::http::Method::POST,
            "/users",
            Some("not json"),
        )
        .await;
        assert_eq!(status, 400);
        assert!(body.contains("JSON"));
    }

    #[tokio::test]
    async fn e2e_put_con_path_param_y_body() {
        let src = "\
            type UserInput { name: Str }\n\
            @put(\"/users/{id}\")\nfn upd(id: Int, body: UserInput) {\n\
                return User { id: id, name: body.name }\n\
            }\n\
            type User { id: Int, name: Str }\n\
        ";
        let (status, body) = run_oneshot_with_body(
            src,
            axum::http::Method::PUT,
            "/users/42",
            Some(r#"{"name":"ana"}"#),
        )
        .await;
        assert_eq!(status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "id": 42, "name": "ana" }));
    }

    #[tokio::test]
    async fn e2e_post_sin_body_pero_handler_espera_es_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let (status, body) =
            run_oneshot(src, axum::http::Method::POST, "/users").await;
        assert_eq!(status, 400);
        assert!(body.contains("body requerido"));
    }

    // ---- 7.6 headers como params del handler ----

    #[tokio::test]
    async fn e2e_header_obligatorio_presente_handler_lo_recibe() {
        let src = "@header(name=\"Authorization\")\n@get(\"/protected\")\nfn protected(authorization: Str) => authorization";
        let (status, body) = run_oneshot_with_headers(
            src,
            axum::http::Method::GET,
            "/protected",
            &[("Authorization", "Bearer xyz")],
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"Bearer xyz\"");
    }

    #[tokio::test]
    async fn e2e_header_obligatorio_falta_es_400() {
        let src = "@header(name=\"Authorization\")\n@get(\"/protected\")\nfn protected(authorization: Str) => authorization";
        let (status, body) = run_oneshot_with_headers(
            src,
            axum::http::Method::GET,
            "/protected",
            &[],
        )
        .await;
        assert_eq!(status, 400);
        assert!(body.contains("Authorization"), "body fue: {}", body);
        assert!(body.contains("obligatorio"), "body fue: {}", body);
    }

    #[tokio::test]
    async fn e2e_header_nullable_falta_handler_recibe_null() {
        let src = "@header(name=\"X-Trace-Id\")\n@get(\"/traced\")\nfn traced(x_trace_id: Str?) -> Str { return \"ok\" }";
        let (status, body) = run_oneshot_with_headers(
            src,
            axum::http::Method::GET,
            "/traced",
            &[],
        )
        .await;
        // Handler corre OK porque el header es opcional.
        assert_eq!(status, 200);
        assert_eq!(body, "\"ok\"");
    }

    #[tokio::test]
    async fn e2e_header_lookup_es_case_insensitive() {
        // HTTP es case-insensitive en nombres de header. Mandamos
        // `authorization` (lowercase) y el handler declara
        // `@header(name="Authorization")` — debe matchear.
        let src = "@header(name=\"Authorization\")\n@get(\"/x\")\nfn h(authorization: Str) => authorization";
        let (status, body) = run_oneshot_with_headers(
            src,
            axum::http::Method::GET,
            "/x",
            &[("authorization", "valor")],
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"valor\"");
    }

    // ---- 7.2 auto-register de /openapi.json ----
    //
    // Helper local: arma router desde un `HttpRegistry` (Arc-wrapped) +
    // schema y le manda GET /openapi.json. Para los casos sin rutas
    // de usuario se pasa `HttpRegistry::new()`. Post-F17.5: cero glue,
    // el router responde directo (no necesita ningún bridge).

    async fn oneshot_get_openapi(
        registry: HttpRegistry,
        openapi_schema: Option<serde_json::Value>,
    ) -> (u16, String) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let metas = registry.metas();
        let router = build_router(&metas, std::sync::Arc::new(registry), openapi_schema);
        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/openapi.json")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_con_schema_some_registra_openapi_json() {
        // Schema mínimo: el router lo sirve como-is en GET /openapi.json.
        let schema = serde_json::json!({
            "openapi": "3.1.0",
            "info": { "title": "Fitz API", "version": "0.1.0" },
            "paths": {},
        });
        let (status, body) = oneshot_get_openapi(HttpRegistry::new(), Some(schema)).await;
        assert_eq!(status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["openapi"], serde_json::json!("3.1.0"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_con_schema_none_no_registra_openapi_json() {
        let (status, _body) = oneshot_get_openapi(HttpRegistry::new(), None).await;
        assert_eq!(status, 404);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_auto_register_convive_con_rutas_del_usuario() {
        // Si el usuario tiene `@get("/")` y el auto-register suma
        // `/openapi.json`, ambas funcionan. Verificamos que el schema
        // sigue disponible aún con rutas declaradas.
        let src = "@get(\"/\")\nfn hello() => \"hola\"";
        let registry = registry_from_source(src).await;
        let schema = serde_json::json!({
            "openapi": "3.1.0",
            "paths": { "/": {} },
        });
        let (status, body) = oneshot_get_openapi(registry, Some(schema)).await;
        assert_eq!(status, 200);
        assert!(body.contains("openapi"));
    }

    #[tokio::test]
    async fn usuario_declara_openapi_json_propio_y_gana_sobre_auto_register() {
        // El usuario declaró su propio `@get("/openapi.json")`. El
        // auto-register debe ceder — la ruta del usuario es la que
        // responde. Verificamos que la respuesta es la del usuario
        // (un string `"mio"`), no el schema cacheado que pasamos.
        let src = "@get(\"/openapi.json\")\nfn custom() => \"mio\"";
        let registry = registry_from_source(src).await;
        let metas = registry.metas();
        let auto_schema = serde_json::json!({
            "openapi": "3.1.0",
            "_marker": "auto-register",
        });

        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let router = build_router(&metas, std::sync::Arc::new(registry), Some(auto_schema));
        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/openapi.json")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();

        assert_eq!(status, 200);
        // El body es el del handler del usuario: `"mio"` (JSON string).
        // NO contiene "_marker" del schema auto-register.
        assert_eq!(body, "\"mio\"");
        assert!(!body.contains("_marker"));
    }

    // ---- 7.3 auto-register de /docs (UI Scalar) ----

    /// Helper local: GET /docs sobre un router armado con o sin schema.
    async fn oneshot_get_docs(
        registry: HttpRegistry,
        openapi_schema: Option<serde_json::Value>,
    ) -> (u16, String) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let metas = registry.metas();
        let router = build_router(&metas, std::sync::Arc::new(registry), openapi_schema);
        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/docs")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_con_schema_some_registra_docs() {
        // GET /docs devuelve el HTML embebido. Verificamos que el
        // body referencia `/openapi.json` (data-url del script de
        // Scalar) — eso garantiza que el HTML está conectado al
        // schema autogenerado.
        let schema = serde_json::json!({ "openapi": "3.1.0", "paths": {} });
        let (status, body) = oneshot_get_docs(HttpRegistry::new(), Some(schema)).await;
        assert_eq!(status, 200);
        assert!(
            body.contains("data-url=\"/openapi.json\""),
            "esperaba que el HTML referenciara /openapi.json, body fue:\n{}",
            body
        );
        assert!(
            body.contains("@scalar/api-reference"),
            "esperaba que el HTML cargara el bundle de Scalar, body fue:\n{}",
            body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_con_schema_none_no_registra_docs() {
        // Sin schema no se registra /docs (paridad con /openapi.json).
        let (status, _body) = oneshot_get_docs(HttpRegistry::new(), None).await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn usuario_declara_docs_propio_y_gana_sobre_auto_register() {
        // El usuario declaró su propio `@get("/docs")`. El auto-register
        // de la UI Scalar cede — la ruta del usuario es la que responde.
        let src = "@get(\"/docs\")\nfn custom() => \"docs-personalizada\"";
        let registry = registry_from_source(src).await;
        let metas = registry.metas();
        let auto_schema = serde_json::json!({ "openapi": "3.1.0" });

        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let router = build_router(&metas, std::sync::Arc::new(registry), Some(auto_schema));
        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/docs")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();

        assert_eq!(status, 200);
        // Body del usuario, no el HTML de Scalar.
        assert_eq!(body, "\"docs-personalizada\"");
        assert!(!body.contains("@scalar/api-reference"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn push_route_acumula_en_el_registry_activo() {
        let ((), reg) = with_active_registry(|| {
            let env = crate::env::Environment::new();
            let handler = Value::Function {
                params: vec![],
                body: vec![],
                closure: env,
                is_async: false,
            };
            push_route(RouteSpec {
                method: HttpMethod::Get,
                path: "/".into(),
                path_params: vec![],
                query_params: vec![],
                handler,
                handler_name: "index".into(),
                param_types: vec![],
                body_param: None,
                headers: vec![],
                param_type_exprs: vec![],
                return_type_expr: None,
                middlewares: vec![],
                cors: None,
            });
        });
        assert_eq!(reg.routes.len(), 1);
        assert_eq!(reg.routes[0].method, HttpMethod::Get);
        assert_eq!(reg.routes[0].handler_name, "index");
    }

    // ---- Mini-tanda HC.1 — status fuera de 100..1000 ----

    fn err_instance_with_status(status: i64) -> Value {
        let fields = vec![
            ("status".into(), Value::Int(status)),
            ("message".into(), Value::Str("test".into())),
        ];
        let instance = Value::Instance {
            type_name: "ApiErr".into(),
            fields: std::sync::Arc::new(parking_lot::Mutex::new(fields)),
        };
        Value::Result(ResultVariant::Err(Box::new(instance)))
    }

    #[test]
    fn hc1_err_con_status_valido_usa_ese_status() {
        let outcome = value_to_outcome(&err_instance_with_status(404));
        assert_eq!(outcome.status, 404);
    }

    #[test]
    fn hc1_err_con_status_fuera_de_rango_emite_500_con_msg_claro() {
        let outcome = value_to_outcome(&err_instance_with_status(50));
        assert_eq!(outcome.status, 500);
        let body_str = outcome.body.to_string();
        assert!(
            body_str.contains("inválido") && body_str.contains("50"),
            "esperaba mensaje claro, fue: {}",
            body_str
        );
    }

    #[test]
    fn hc1_err_con_status_99_es_fuera_de_rango() {
        let outcome = value_to_outcome(&err_instance_with_status(99));
        assert_eq!(outcome.status, 500);
    }

    #[test]
    fn hc1_err_con_status_1500_es_fuera_de_rango() {
        let outcome = value_to_outcome(&err_instance_with_status(1500));
        assert_eq!(outcome.status, 500);
    }

    // ---- Mini-tanda Hpx.1 — Content-Type validation ----

    fn registry_with_post_body_route() -> std::sync::Arc<HttpRegistry> {
        // Setup mínimo: una ruta POST /test que espera body como
        // Value::Map libre (sin schema).
        let mut reg = HttpRegistry::new();
        let handler = Value::Function {
            params: vec![crate::ast::Param {
                name: "body".into(),
                type_: None,
                default: None,
                varargs: false,
            }],
            body: vec![crate::ast::Stmt::Return(
                crate::ast::Expr::Ident("body".into(), crate::ast::Span::ZERO),
                crate::ast::Span::ZERO,
            )],
            closure: crate::env::Environment::new(),
            is_async: false,
        };
        reg.routes.push(RouteSpec {
            method: HttpMethod::Post,
            path: "/test".into(),
            path_params: vec![],
            query_params: vec![],
            handler,
            handler_name: "test".into(),
            param_types: vec![("body".into(), None, false)],
            body_param: Some(BodyParam {
                name: "body".into(),
                declared_type: None,
                declared_type_name: None,
            }),
            headers: vec![],
            param_type_exprs: vec![("body".into(), None)],
            return_type_expr: None,
            middlewares: vec![],
            cors: None,
        });
        std::sync::Arc::new(reg)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hpx1_content_type_json_pasa() {
        let reg = registry_with_post_body_route();
        let body = br#"{"foo": 42}"#.to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert("content-type".into(), "application/json".into());
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        ).await;
        assert_eq!(outcome.status, 200, "esperaba 200, fue {} con body {}", outcome.status, outcome.body);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hpx1_content_type_text_plain_rechaza_con_415() {
        let reg = registry_with_post_body_route();
        let body = b"plain text".to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert("content-type".into(), "text/plain".into());
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        ).await;
        assert_eq!(outcome.status, 415);
        assert!(
            outcome.body.contains("text/plain") && outcome.body.contains("application/json"),
            "esperaba mensaje claro, fue: {}",
            outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp2_content_type_charset_diff_no_oficial_rechaza() {
        // Mini-tanda MP2 — `text/plain` (test viejo asumía
        // multipart-rechaza-con-415; ahora multipart se acepta así
        // que cambié el case). text/plain sigue rechazado: el
        // intérprete acepta JSON, urlencoded y multipart, nada más.
        let reg = registry_with_post_body_route();
        let body = b"raw text content".to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert("content-type".into(), "application/octet-stream".into());
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        ).await;
        assert_eq!(outcome.status, 415);
        assert!(
            outcome.body.contains("octet-stream"),
            "esperaba que el msg cite el CT recibido, fue: {}",
            outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hpx1_content_type_ausente_acepta() {
        // Sin header Content-Type (curl sin -H), aceptamos JSON crudo.
        let reg = registry_with_post_body_route();
        let body = br#"{"foo": 42}"#.to_vec();
        let headers = std::collections::HashMap::new();
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        ).await;
        assert_eq!(outcome.status, 200);
    }

    // ---- Mini-tanda Mw.next — middleware post-process ----

    fn make_mw_post(name: &str) -> MiddlewareSpec {
        // Constructor minimal de un middleware Post (2 args).
        // Body: `return 200 { "wrapped": true }`.
        let handler = Value::Function {
            params: vec![
                crate::ast::Param {
                    name: "req".into(),
                    type_: None,
                    default: None,
                    varargs: false,
                },
                crate::ast::Param {
                    name: "res".into(),
                    type_: None,
                    default: None,
                    varargs: false,
                },
            ],
            body: vec![crate::ast::Stmt::ReturnStatus {
                status: crate::ast::Expr::Int(200, crate::ast::Span::ZERO),
                body: Some(crate::ast::Expr::Map(
                    vec![(
                        crate::ast::Expr::Str("wrapped".into(), crate::ast::Span::ZERO),
                        crate::ast::Expr::Bool(true, crate::ast::Span::ZERO),
                    )],
                    crate::ast::Span::ZERO,
                )),
                span: crate::ast::Span::ZERO,
            }],
            closure: crate::env::Environment::new(),
            is_async: false,
        };
        MiddlewareSpec {
            name: name.into(),
            handler,
            kind: MiddlewareKind::Post,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mwnext_post_middleware_modifica_response() {
        let request = build_request_value(
            HttpMethod::Get,
            "/test",
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        );
        let original = HandlerOutcome::json(200, serde_json::json!({"original": true}));
        let mws = vec![make_mw_post("wrapper")];
        let outcome = run_post_middlewares(&mws, &request, original).await;
        assert_eq!(outcome.status, 200);
        assert!(outcome.body.contains("wrapped"), "esperaba body con `wrapped`, fue: {}", outcome.body);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mwnext_post_middleware_sin_post_no_modifica() {
        let request = build_request_value(
            HttpMethod::Get,
            "/test",
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        );
        let original = HandlerOutcome::json(200, serde_json::json!({"original": true}));
        let mws: Vec<MiddlewareSpec> = vec![]; // empty
        let outcome = run_post_middlewares(&mws, &request, original.clone()).await;
        assert_eq!(outcome.status, original.status);
        assert_eq!(outcome.body, original.body);
    }

    // ---- Mini-tanda MP — urlencoded bodies ----

    #[tokio::test(flavor = "current_thread")]
    async fn mp_urlencoded_basico_parsea_a_map() {
        let reg = registry_with_post_body_route();
        let body = b"name=Fitz&age=25".to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert("content-type".into(), "application/x-www-form-urlencoded".into());
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        ).await;
        assert_eq!(outcome.status, 200, "esperaba 200, fue {} con body {}", outcome.status, outcome.body);
        assert!(
            outcome.body.contains("\"name\":\"Fitz\"") && outcome.body.contains("\"age\":\"25\""),
            "esperaba name/age en body, fue: {}",
            outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp_urlencoded_con_url_encoding() {
        let reg = registry_with_post_body_route();
        // "hola mundo" + "Fitz Roy" con encoding (espacios como +)
        let body = b"greeting=hola+mundo&place=Fitz%20Roy".to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert("content-type".into(), "application/x-www-form-urlencoded".into());
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        ).await;
        assert_eq!(outcome.status, 200);
        assert!(
            outcome.body.contains("\"greeting\":\"hola mundo\""),
            "esperaba `+` decodificado a espacio: {}",
            outcome.body
        );
        assert!(
            outcome.body.contains("\"place\":\"Fitz Roy\""),
            "esperaba `%20` decodificado a espacio: {}",
            outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp_urlencoded_body_vacio_es_map_vacio() {
        let reg = registry_with_post_body_route();
        let mut headers = std::collections::HashMap::new();
        headers.insert("content-type".into(), "application/x-www-form-urlencoded".into());
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            Vec::new(),
            headers,
        ).await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "{}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp2_multipart_sin_boundary_es_400() {
        // Mini-tanda MP2 — `multipart/form-data` sin `boundary=` →
        // 400 con mensaje claro (no 415, ahora SÍ se acepta multipart
        // como CT supported pero el boundary es obligatorio).
        let reg = registry_with_post_body_route();
        let body = b"--boundary\r\n".to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert("content-type".into(), "multipart/form-data".into());
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        ).await;
        assert_eq!(outcome.status, 400);
        assert!(
            outcome.body.contains("boundary"),
            "esperaba mención de boundary, fue: {}",
            outcome.body
        );
    }

    // ---- Quick win F13 bundle — base64 encoder ----

    #[test]
    fn b64_encode_empty() {
        assert_eq!(b64_encode_standard(b""), "");
    }

    #[test]
    fn b64_encode_basico() {
        // RFC 4648 test vectors estándar.
        assert_eq!(b64_encode_standard(b"f"), "Zg==");
        assert_eq!(b64_encode_standard(b"fo"), "Zm8=");
        assert_eq!(b64_encode_standard(b"foo"), "Zm9v");
        assert_eq!(b64_encode_standard(b"foob"), "Zm9vYg==");
        assert_eq!(b64_encode_standard(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64_encode_standard(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn b64_encode_binarios() {
        // Bytes binarios arbitrarios.
        assert_eq!(b64_encode_standard(&[0u8]), "AA==");
        assert_eq!(b64_encode_standard(&[0xff, 0xff, 0xff]), "////");
    }

    #[test]
    fn value_to_json_bytes_emite_base64() {
        // Mini-tanda Bytes + quick win F13: `Value::Bytes` se
        // serializa como base64 string (no como array de Int).
        let v = Value::Bytes(b"hola".to_vec());
        let j = value_to_json(&v).unwrap();
        assert_eq!(j, serde_json::json!("aG9sYQ=="));
    }

    #[test]
    fn mp2_extract_boundary_simple() {
        assert_eq!(
            extract_multipart_boundary("multipart/form-data; boundary=abc"),
            Some("abc".to_string())
        );
    }

    #[test]
    fn mp2_extract_boundary_con_comillas() {
        // RFC 7578 permite boundary entre comillas dobles.
        assert_eq!(
            extract_multipart_boundary(r#"multipart/form-data; boundary="my-boundary""#),
            Some("my-boundary".to_string())
        );
    }

    #[test]
    fn mp2_extract_boundary_case_sensitive_value() {
        // Los boundaries son case-sensitive: `BOUNDARY` minus se trim,
        // pero el valor se preserva tal cual.
        assert_eq!(
            extract_multipart_boundary("multipart/form-data; Boundary=ABC-Def"),
            Some("ABC-Def".to_string())
        );
    }

    #[test]
    fn mp2_extract_boundary_ausente_devuelve_none() {
        assert_eq!(extract_multipart_boundary("multipart/form-data"), None);
    }

    #[test]
    fn mp2_parse_multipart_text_field_basico() {
        // Body con una part de tipo text field (sin filename).
        // Estructura: --<b>\r\n<hdr>\r\n\r\n<body>\r\n--<b>--
        let boundary = "----foo";
        let body = format!(
            "------foo\r\nContent-Disposition: form-data; name=\"msg\"\r\n\r\nhola\r\n------foo--"
        );
        let result = parse_multipart_body(body.as_bytes(), boundary).expect("parse OK");
        match result {
            Value::Map(entries) => {
                let pairs = entries.lock();
                assert_eq!(pairs.len(), 1);
                assert_eq!(pairs[0].0, Value::Str("msg".into()));
                assert_eq!(pairs[0].1, Value::Str("hola".into()));
            }
            other => panic!("esperaba Value::Map, fue: {:?}", other),
        }
    }

    #[test]
    fn mp2_parse_multipart_file_field_construye_instance_file() {
        // Body con file field (con filename) → Value::Instance del
        // tipo built-in `File`.
        let boundary = "----foo";
        let body = "------foo\r\nContent-Disposition: form-data; name=\"upload\"; filename=\"hello.txt\"\r\nContent-Type: text/plain\r\n\r\nfile contents here\r\n------foo--";
        let result = parse_multipart_body(body.as_bytes(), boundary).expect("parse OK");
        let Value::Map(entries) = result else {
            panic!("esperaba Value::Map");
        };
        let pairs = entries.lock();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, Value::Str("upload".into()));
        match &pairs[0].1 {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "File");
                let fld = fields.lock();
                assert_eq!(fld.len(), 3);
                assert_eq!(fld[0].0, "name");
                assert_eq!(fld[0].1, Value::Str("hello.txt".into()));
                assert_eq!(fld[1].0, "content_type");
                assert_eq!(fld[1].1, Value::Str("text/plain".into()));
                assert_eq!(fld[2].0, "content");
                // File.content Bytes — content ahora es Value::Bytes
                // (Vec<u8>), no Value::Str. Habilita files binarios.
                assert_eq!(fld[2].1, Value::Bytes(b"file contents here".to_vec()));
            }
            other => panic!("esperaba Value::Instance(File), fue: {:?}", other),
        }
    }

    #[test]
    fn mp2_parse_multipart_mixto_text_y_file() {
        // Form con un text field + un file field.
        let boundary = "X";
        let body = "--X\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nMi título\r\n--X\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a.txt\"\r\n\r\ncontenido\r\n--X--";
        let result = parse_multipart_body(body.as_bytes(), boundary).expect("parse OK");
        let Value::Map(entries) = result else {
            panic!("esperaba Value::Map");
        };
        let pairs = entries.lock();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, Value::Str("title".into()));
        assert_eq!(pairs[0].1, Value::Str("Mi título".into()));
        assert_eq!(pairs[1].0, Value::Str("doc".into()));
        assert!(matches!(pairs[1].1, Value::Instance { .. }));
    }

    #[test]
    fn mp2_parse_multipart_binary_file_field_funciona() {
        // File.content Bytes — bytes binarios no-UTF8 (0xFF) en un
        // FILE field ya funcionan (antes era 400, ahora se guardan
        // como `Value::Bytes` raw). Habilita uploads binarios.
        let boundary = "X";
        let mut body = b"--X\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n\r\n".to_vec();
        body.push(0xff);
        body.push(0xfe);
        body.extend_from_slice(b"\r\n--X--");
        let result = parse_multipart_body(&body, boundary).expect("parse OK con binary");
        let Value::Map(entries) = result else {
            panic!("esperaba Value::Map");
        };
        let pairs = entries.lock();
        assert_eq!(pairs.len(), 1);
        match &pairs[0].1 {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "File");
                let fld = fields.lock();
                assert_eq!(fld[2].0, "content");
                assert_eq!(fld[2].1, Value::Bytes(vec![0xff, 0xfe]));
            }
            other => panic!("esperaba Instance(File), fue: {:?}", other),
        }
    }

    #[test]
    fn mp2_parse_multipart_text_field_sin_filename_sigue_exigiendo_utf8() {
        // Text field (sin filename) sigue requiriendo UTF-8 — para
        // bytes binarios, el usuario debe usar `filename=`.
        let boundary = "X";
        let mut body = b"--X\r\nContent-Disposition: form-data; name=\"raw\"\r\n\r\n".to_vec();
        body.push(0xff);
        body.extend_from_slice(b"\r\n--X--");
        let err = parse_multipart_body(&body, boundary).expect_err("esperaba error");
        assert!(
            err.contains("UTF-8") && err.contains("filename="),
            "esperaba mención de UTF-8 + workaround filename=, fue: {}",
            err
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp2_multipart_end_to_end_acepta_y_parsea() {
        // E2E del path completo: `handle_task` recibe un body
        // multipart válido y lo enrutea al handler con el body
        // parseado como `Value::Map<Str, Value>`.
        let reg = registry_with_post_body_route();
        let boundary = "----my-boundary";
        let body = format!(
            "------my-boundary\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nFitz\r\n------my-boundary--"
        );
        let mut headers = std::collections::HashMap::new();
        headers.insert(
            "content-type".into(),
            format!("multipart/form-data; boundary={}", boundary),
        );
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body.into_bytes(),
            headers,
        ).await;
        // `registry_with_post_body_route` espera body parseable como
        // `Map`, así que devolverá 200 con el body echo'd.
        assert_eq!(outcome.status, 200, "outcome body: {}", outcome.body);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hpx1_content_type_con_charset_acepta() {
        let reg = registry_with_post_body_route();
        let body = br#"{"foo": 42}"#.to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert("content-type".into(), "application/json; charset=utf-8".into());
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        ).await;
        assert_eq!(outcome.status, 200);
    }

    // ---- Mini-tanda Mw-Wrap — clasificación + chain runner ----

    #[test]
    fn mw_wrap_classifier_param_fn_es_wrap() {
        // Segundo param `Fn() -> Response` → Wrap.
        use crate::ast::{Param, TypeExpr};
        let p = Param {
            name: "next".into(),
            type_: Some(TypeExpr::Function {
                params: vec![],
                ret: Box::new(TypeExpr::Named("Response".into())),
            }),
            default: None,
            varargs: false,
        };
        assert_eq!(
            crate::evaluator::classify_2_arg_middleware(&p),
            MiddlewareKind::Wrap,
        );
    }

    #[test]
    fn mw_wrap_classifier_param_response_es_post() {
        // Segundo param `Response` (nominal) → Post.
        use crate::ast::{Param, TypeExpr};
        let p = Param {
            name: "resp".into(),
            type_: Some(TypeExpr::Named("Response".into())),
            default: None,
            varargs: false,
        };
        assert_eq!(
            crate::evaluator::classify_2_arg_middleware(&p),
            MiddlewareKind::Post,
        );
    }

    #[test]
    fn mw_wrap_classifier_param_sin_anotacion_es_post() {
        // Sin anotación → default Post (preserva semántica histórica).
        use crate::ast::Param;
        let p = Param {
            name: "resp".into(),
            type_: None,
            default: None,
            varargs: false,
        };
        assert_eq!(
            crate::evaluator::classify_2_arg_middleware(&p),
            MiddlewareKind::Post,
        );
    }

    #[test]
    fn mw_wrap_classifier_param_fn_nullable_es_wrap() {
        // `Fn() -> Response?` también clasifica como Wrap.
        use crate::ast::{Param, TypeExpr};
        let p = Param {
            name: "next".into(),
            type_: Some(TypeExpr::Nullable(Box::new(TypeExpr::Function {
                params: vec![],
                ret: Box::new(TypeExpr::Named("Response".into())),
            }))),
            default: None,
            varargs: false,
        };
        assert_eq!(
            crate::evaluator::classify_2_arg_middleware(&p),
            MiddlewareKind::Wrap,
        );
    }
}
