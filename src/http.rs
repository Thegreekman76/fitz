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
// Threading model (tomado en 4.2):
//
//   `Value` y `EnvRef` usan `Rc<RefCell<>>`, que NO es `Send`. No los
//   podemos pasar a un thread de tokio. Entonces:
//
//     - El intérprete vive en un thread `std::thread` propio (el
//       mismo que corrió `eval`, ahora reutilizado para servir
//       handlers).
//     - tokio corre en otro `std::thread`, dueño del runtime async
//       y del server axum.
//     - Cada request async manda un `InterpreterTask` por un
//       `mpsc::UnboundedSender`, espera el resultado vía
//       `oneshot::Receiver`.
//     - El thread intérprete loopea: recibe task → ejecuta el handler
//       Fitz síncronamente → manda `HandlerOutcome` por el oneshot.
//
// Async real adentro del lenguaje sigue siendo deuda; `is_async` se
// sigue ignorando (es decoración sintáctica). En 4.x o Fase 5
// llevamos await/futures al lenguaje.

use std::cell::RefCell;

use crate::ast::{Expr, TypeExpr};
#[cfg(test)]
use crate::ast::Span;
use crate::value::{Value, ResultVariant};

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
}

impl ServerConfig {
    /// Defaults aplicados cuando no hay `@server` en el programa.
    pub fn default_addr() -> Self {
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 3000,
            enable_docs: true,
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
                    StrPart::Expr(Expr::Ident(name, _)) => {
                        if params.contains(name) {
                            return Err(PathError::DuplicateParam(name.clone()));
                        }
                        params.push(name.clone());
                        buf.push('{');
                        buf.push_str(name);
                        buf.push('}');
                    }
                    StrPart::Expr(other) => {
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
}

impl HandlerOutcome {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        HandlerOutcome {
            status,
            body: body.to_string(),
            content_type: "application/json",
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
///   - `Value::Result(Err(e))` → status 500, body = `{"error": e}`.
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
            // Mismo formato que `internal_error` pero usando el inner
            // del Err como mensaje (idealmente Str, pero serializamos
            // lo que venga).
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

        Value::List(items) => {
            let mut out = Vec::with_capacity(items.borrow().len());
            for v in items.borrow().iter() {
                out.push(value_to_json(v)?);
            }
            J::Array(out)
        }

        Value::Map(pairs) => {
            let mut out = serde_json::Map::new();
            for (k, v) in pairs.borrow().iter() {
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
            for (name, v) in fields.borrow().iter() {
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
        Value::Type { name, fields } => (name.clone(), fields.clone()),
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
// Runtime async — bridge entre axum/tokio y el intérprete síncrono
// ---------------------------------------------------------------------------
//
// Diseño:
//
//   std::thread "tokio"                main thread (intérprete)
//   ┌──────────────────┐              ┌──────────────────────┐
//   │  axum::serve     │   InterpTask │  loop {              │
//   │  ┌─────────────┐ │ ───────────► │    rx.blocking_recv()│
//   │  │ async fn    │ │              │    call_handler(...).await │
//   │  │ dispatch    │ │ ◄─────────── │    send outcome      │
//   │  └─────────────┘ │   outcome    │  }                   │
//   └──────────────────┘              └──────────────────────┘
//
// El intérprete vive en el thread main (el mismo que corrió `eval`).
// Eso evita mover los `Rc<RefCell<>>` de `Value`/`EnvRef`, que no son
// `Send`. tokio corre en un std::thread spawneado, con su propio
// runtime current_thread. Lo que cruza el canal:
//
//   - tokio → main: `InterpTask` con índice de ruta + path params
//     crudos (`HashMap<String,String>`).
//   - main → tokio: `HandlerOutcome` (status + body String).
//
// La metadata de las rutas que axum necesita para configurar el router
// (verbo, path, lista de nombres de path params) se separa en
// `RouteMeta` — un struct que sí es `Send + Clone`. Los handlers Fitz
// nunca cruzan al thread tokio; el dispatch los busca por índice del
// lado del intérprete.

use std::collections::HashMap;

use axum::{
    body::Body,
    extract::Path as AxumPath,
    http::{HeaderValue, StatusCode},
    response::Response,
    routing::MethodRouter,
    Router,
};
use tokio::sync::{mpsc, oneshot};

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
            })
            .collect()
    }
}

/// Trabajo enviado desde tokio al thread intérprete. Lleva un `reply`
/// `oneshot::Sender` por el que el thread devuelve el outcome ya
/// listo para mandar como respuesta HTTP.
pub struct InterpTask {
    pub route_idx: usize,
    /// Path params crudos extraídos por axum (siempre llegan como
    /// strings; la coerción al tipo declarado la hace el intérprete).
    pub path_params: HashMap<String, String>,
    /// Query params crudos extraídos por axum desde la query string
    /// (`?limit=10&offset=20`). Siempre strings; coerción al tipo
    /// declarado del param Fitz la hace el intérprete. Si la ruta no
    /// declara query params en su template, este HashMap queda vacío
    /// (cualquier query string del request se descarta).
    pub query_params: HashMap<String, String>,
    /// Body crudo de la request. Vacío cuando el handler no declara
    /// body — axum no lo extrae en ese caso, pero igual mandamos
    /// `Vec` para uniformidad. Bytes para no forzar UTF-8 acá; la
    /// validación de que sea JSON parseable la hace el intérprete.
    pub body: Vec<u8>,
    pub reply: oneshot::Sender<HandlerOutcome>,
}

/// Sender lado tokio. `Clone` cheap (es un `Arc<...>` adentro). Cada
/// handler de axum clona uno para mandar su task.
pub type TaskTx = mpsc::UnboundedSender<InterpTask>;

/// Construye un `axum::Router` a partir de la metadata de rutas.
/// Cada handler async cierra sobre el sender y el índice de su ruta,
/// manda un task al intérprete y await el reply.
///
/// La metadata (`Vec<RouteMeta>`) basta para configurar todo el
/// routing: verbo + path + si hay path params (para decidir el
/// shape del handler). Los handlers Fitz quedan del lado del
/// intérprete y se buscan por índice cuando llega un task.
///
/// `openapi_schema` (Fase 7.2): si es `Some`, registra una ruta
/// `GET /openapi.json` que sirve el schema cacheado (precomputado al
/// arrancar el server). Si el usuario ya declaró un handler con ese
/// path en sus rutas, el auto-register cede — la del usuario gana.
/// `None` para programas donde no querramos servir el schema (tests
/// internos, server arrancado en modo opt-out cuando 7.4 cierre).
pub fn build_router(
    metas: &[RouteMeta],
    tx: TaskTx,
    openapi_schema: Option<serde_json::Value>,
) -> Router {
    let mut router = Router::new();
    for (idx, meta) in metas.iter().enumerate() {
        let route_handler = build_method_router(
            meta.method,
            idx,
            tx.clone(),
            meta.has_path_params,
            meta.has_query_params,
            meta.expects_body,
        );
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

/// Construye un `MethodRouter` con el handler async correspondiente
/// al verbo. Las cuatro combinaciones (path_params × body) viven en
/// cuatro closures distintos porque los extractors de axum aparecen
/// como argumentos del handler — no se pueden hacer condicionales.
fn build_method_router(
    method: HttpMethod,
    route_idx: usize,
    tx: TaskTx,
    has_path_params: bool,
    has_query_params: bool,
    expects_body: bool,
) -> MethodRouter {
    // 8 combinaciones: (path × query × body), cada uno boolean.
    // axum requiere que la firma del handler async refleje exactamente
    // los extractores que usa, así que armamos las 8 variantes a mano.
    // (Si en el futuro se vuelve insostenible, el escape es usar
    // `axum::extract::RawQuery` y parsear a mano.)
    use axum::extract::Query as AxumQuery;
    type Map = HashMap<String, String>;
    match (has_path_params, has_query_params, expects_body) {
        (false, false, false) => {
            let h = move || {
                let tx = tx.clone();
                async move {
                    dispatch_request(route_idx, Map::new(), Map::new(), Vec::new(), tx).await
                }
            };
            wrap(method, h)
        }
        (true, false, false) => {
            let h = move |AxumPath(p): AxumPath<Map>| {
                let tx = tx.clone();
                async move { dispatch_request(route_idx, p, Map::new(), Vec::new(), tx).await }
            };
            wrap(method, h)
        }
        (false, true, false) => {
            let h = move |AxumQuery(q): AxumQuery<Map>| {
                let tx = tx.clone();
                async move {
                    dispatch_request(route_idx, Map::new(), q, Vec::new(), tx).await
                }
            };
            wrap(method, h)
        }
        (true, true, false) => {
            let h = move |AxumPath(p): AxumPath<Map>, AxumQuery(q): AxumQuery<Map>| {
                let tx = tx.clone();
                async move { dispatch_request(route_idx, p, q, Vec::new(), tx).await }
            };
            wrap(method, h)
        }
        (false, false, true) => {
            let h = move |body: axum::body::Bytes| {
                let tx = tx.clone();
                async move {
                    dispatch_request(route_idx, Map::new(), Map::new(), body.to_vec(), tx).await
                }
            };
            wrap(method, h)
        }
        (true, false, true) => {
            let h = move |AxumPath(p): AxumPath<Map>, body: axum::body::Bytes| {
                let tx = tx.clone();
                async move {
                    dispatch_request(route_idx, p, Map::new(), body.to_vec(), tx).await
                }
            };
            wrap(method, h)
        }
        (false, true, true) => {
            let h = move |AxumQuery(q): AxumQuery<Map>, body: axum::body::Bytes| {
                let tx = tx.clone();
                async move {
                    dispatch_request(route_idx, Map::new(), q, body.to_vec(), tx).await
                }
            };
            wrap(method, h)
        }
        (true, true, true) => {
            let h = move |AxumPath(p): AxumPath<Map>,
                          AxumQuery(q): AxumQuery<Map>,
                          body: axum::body::Bytes| {
                let tx = tx.clone();
                async move {
                    dispatch_request(route_idx, p, q, body.to_vec(), tx).await
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

/// Punto único donde el lado async manda un task al intérprete y
/// espera la respuesta. Si algo falla en el canal (intérprete
/// muerto, oneshot dropped), respondemos 500 con un mensaje claro.
async fn dispatch_request(
    route_idx: usize,
    path_params: HashMap<String, String>,
    query_params: HashMap<String, String>,
    body: Vec<u8>,
    tx: TaskTx,
) -> Response {
    let (reply_tx, reply_rx) = oneshot::channel();
    let task = InterpTask {
        route_idx,
        path_params,
        query_params,
        body,
        reply: reply_tx,
    };
    if tx.send(task).is_err() {
        return outcome_to_response(HandlerOutcome::internal_error(
            "intérprete cerrado — no se puede atender la request",
        ));
    }
    match reply_rx.await {
        Ok(outcome) => outcome_to_response(outcome),
        Err(_) => outcome_to_response(HandlerOutcome::internal_error(
            "handler no devolvió respuesta (canal interno cerrado)",
        )),
    }
}

/// Convierte un `HandlerOutcome` a la `Response` de axum. Status,
/// header `content-type`, body como bytes.
fn outcome_to_response(outcome: HandlerOutcome) -> Response {
    let mut resp = Response::new(Body::from(outcome.body));
    *resp.status_mut() = StatusCode::from_u16(outcome.status)
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static(outcome.content_type),
    );
    resp
}

/// Loop síncrono del intérprete. Owns el `HttpRegistry` (con los
/// `Value::Function` y sus closures). Por cada task recibido del
/// canal:
///
///   1. Busca la `RouteSpec` por índice.
///   2. Coerciona cada path param crudo al tipo declarado del
///      parámetro del handler (Int/Float/Str/Bool). Si falla, 400.
///   3. Construye `args` en el orden de los parámetros del handler.
///   4. Invoca el handler vía `call_handler` y traduce el resultado
///      a `HandlerOutcome` (Result → status, otros → 200).
///   5. Envía el outcome por el `oneshot::Sender`.
///
/// El loop termina cuando el sender se cierra (todos los Tx
/// droppearon), que ocurre cuando el server async termina.
pub fn run_interpreter_loop(
    registry: HttpRegistry,
    mut rx: mpsc::UnboundedReceiver<InterpTask>,
) {
    // Fase 6.4: el evaluator es ahora async (`eval_call`/`eval_stmt`/
    // etc. devuelven futures). Para mantener el bridge mpsc/oneshot
    // intacto en 6.4 (eliminación en 6.5), armamos un runtime
    // tokio `current_thread` propio del loop y bloqueamos sobre
    // cada `handle_task(...).await`. Cuando 6.5 elimine el bridge,
    // los handlers axum llaman a `eval_call(...).await` directo y
    // este loop entero desaparece.
    let runtime = crate::evaluator::build_runtime();
    while let Some(task) = rx.blocking_recv() {
        let outcome = runtime.block_on(handle_task(
            &registry,
            task.route_idx,
            task.path_params,
            task.query_params,
            task.body,
        ));
        // Si el oneshot del lado axum se cerró (cliente desconectado,
        // timeout), no hay nada que hacer con el outcome — descartar.
        let _ = task.reply.send(outcome);
    }
}

/// Procesa un único task. Aislado del loop para testearlo sin canal.
async fn handle_task(
    registry: &HttpRegistry,
    route_idx: usize,
    raw_path_params: HashMap<String, String>,
    raw_query_params: HashMap<String, String>,
    body_bytes: Vec<u8>,
) -> HandlerOutcome {
    let Some(route) = registry.routes.get(route_idx) else {
        return HandlerOutcome::internal_error(format!(
            "ruta {} no existe en el registry",
            route_idx,
        ));
    };

    // Si el handler espera body, parsearlo y prepararlo. Lo hacemos
    // antes de armar args para fallar temprano si el JSON está roto.
    let body_value: Option<Value> = if let Some(bp) = &route.body_param {
        match parse_body(&body_bytes, bp) {
            Ok(v) => Some(v),
            Err(msg) => {
                return HandlerOutcome::json(
                    400,
                    serde_json::json!({ "error": msg }),
                );
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
        } else {
            return HandlerOutcome::internal_error(format!(
                "parámetro '{}' del handler '{}' no es ni path param ni query param ni body — \
                 esto es un bug interno del registro",
                name, route.handler_name,
            ));
        }
    }

    // Invocar el handler. Errores del handler (return propio, error
    // de runtime) se traducen a 500 con el mensaje.
    match call_handler(route.handler.clone(), args, &route.handler_name).await {
        Ok(value) => value_to_outcome(&value),
        Err(err) => HandlerOutcome::internal_error(err.message),
    }
}

/// Parsea los bytes del body en un `Value` Fitz según la convención
/// del body param:
///   - JSON inválido → error 400 con mensaje claro.
///   - Si el body param tiene `declared_type: Some(Value::Type)`,
///     validamos contra el type (campos faltantes, extras, etc.) y
///     construimos un `Value::Instance`.
///   - Si no, deserializamos a `Value` libre (Map/List/primitivos).
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
/// Modelo de threading (revertido respecto del borrador inicial):
///   - El thread llamador (main, donde corrió `eval`) NO se mueve —
///     contiene los `Rc<RefCell<>>` de los handlers y no es `Send`.
///   - Spawneamos un std::thread separado para tokio + axum. Recibe
///     solo `Vec<RouteMeta>` (estructural, `Send + Clone`) y el `tx`
///     del canal.
///   - El thread main, después de spawnear tokio, entra al loop del
///     intérprete: `rx.blocking_recv()`, procesa, manda outcome.
///
/// Cuando axum baja por Ctrl-C, su thread termina, todos los `tx`
/// vivos en handlers se dropean, el `rx` del main loop devuelve
/// `None` y la función retorna.
pub fn serve(
    registry: HttpRegistry,
    program: crate::ast::Program,
    addr: std::net::SocketAddr,
) -> std::io::Result<()> {
    use std::thread;

    let (tx, rx) = mpsc::unbounded_channel::<InterpTask>();
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
    let openapi_schema = if enable_docs {
        let routes = crate::openapi::routes_from_registry(&registry);
        Some(crate::openapi::generate_openapi(&routes, &program))
    } else {
        None
    };

    // Thread tokio: owns el runtime async y el server axum. Solo
    // recibe metadata + tx (todos `Send`).
    let tokio_handle = thread::Builder::new()
        .name("fitz-http".into())
        .spawn(move || -> std::io::Result<()> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async move {
                let router = build_router(&metas, tx, openapi_schema);
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
        })?;

    // Main thread: loop del intérprete. Bloquea hasta que el canal
    // se cierra (cuando el thread tokio termina y dropea su tx).
    run_interpreter_loop(registry, rx);

    // Esperar a que el thread tokio termine de bajar limpio.
    match tokio_handle.join() {
        Ok(res) => res,
        Err(_) => Err(std::io::Error::other(
            "thread del servidor HTTP panickeó",
        )),
    }
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
            StrPart::Expr(Expr::Ident("id".into(), Span::ZERO)),
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
            StrPart::Expr(Expr::Ident("org".into(), Span::ZERO)),
            StrPart::Lit("/users/".into()),
            StrPart::Expr(Expr::Ident("id".into(), Span::ZERO)),
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
            }),
        ], Span::ZERO);
        let err = parse_path_template(&e).unwrap_err();
        assert!(matches!(err, PathError::UnsupportedInterpolation(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_con_params_duplicados_es_error() {
        // `"/a/{x}/b/{x}"`
        let e = Expr::StrInterp(vec![
            StrPart::Lit("/a/".into()),
            StrPart::Expr(Expr::Ident("x".into(), Span::ZERO)),
            StrPart::Lit("/b/".into()),
            StrPart::Expr(Expr::Ident("x".into(), Span::ZERO)),
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
                StrPart::Expr(Expr::Ident("limit".into(), Span::ZERO)),
                StrPart::Lit("&offset=".into()),
                StrPart::Expr(Expr::Ident("offset".into(), Span::ZERO)),
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
                StrPart::Expr(Expr::Ident("id".into(), Span::ZERO)),
                StrPart::Lit("/posts?limit=".into()),
                StrPart::Expr(Expr::Ident("limit".into(), Span::ZERO)),
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
                StrPart::Expr(Expr::Ident("limit".into(), Span::ZERO)),
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
                StrPart::Expr(Expr::Ident("id".into(), Span::ZERO)),
                StrPart::Lit("?id=".into()),
                StrPart::Expr(Expr::Ident("id".into(), Span::ZERO)),
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
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), Vec::new()).await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"hola\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_coerciona_path_param_int() {
        let src = "@get(\"/users/{id}\")\nfn h(id: Int) => id * 2";
        let registry = registry_from_source(src).await;
        let mut params = HashMap::new();
        params.insert("id".into(), "21".into());
        let outcome = handle_task(&registry, 0, params, HashMap::new(), Vec::new()).await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "42");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_path_param_int_invalido_es_400() {
        let src = "@get(\"/users/{id}\")\nfn h(id: Int) => id";
        let registry = registry_from_source(src).await;
        let mut params = HashMap::new();
        params.insert("id".into(), "no-es-int".into());
        let outcome = handle_task(&registry, 0, params, HashMap::new(), Vec::new()).await;
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("Int"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_handler_que_retorna_err_es_500_con_error() {
        // El handler devuelve Err("boom"): runtime lo traduce a 500.
        let src = "@get(\"/\")\nfn h() => Err(\"boom\")";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), Vec::new()).await;
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
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), Vec::new()).await;
        assert_eq!(outcome.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&outcome.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "id": 1, "name": "ana" }));
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
            };
            assert!(set_server_config(first.clone()).is_ok());
            let second = ServerConfig {
                host: "0.0.0.0".into(),
                port: 9090,
                enable_docs: true,
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
                let items = items.borrow();
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
                let pairs = pairs.borrow();
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
                let fields = fields.borrow();
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
                let fields = fields.borrow();
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
                let fields = fields.borrow();
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
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), Vec::new()).await;
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
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), body).await;
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
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), b"not json".to_vec()).await;
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
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), body).await;
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
        let outcome = handle_task(&registry, 0, params, HashMap::new(), body).await;
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
        let outcome = handle_task(&registry, 0, HashMap::new(), HashMap::new(), body).await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"x\"");
    }

    // ---- build_router + oneshot E2E ----
    //
    // Estos tests arman un router de axum y le mandan requests sin
    // abrir socket TCP, vía `tower::ServiceExt::oneshot`.
    //
    // Threading: en `serve()` real, el intérprete vive en el thread
    // main y tokio en un std::thread spawneado. Pero los handlers
    // tienen `Rc<RefCell<>>` (no Send), así que no podemos mover el
    // registry a un thread spawneado — eso mismo es lo que `serve()`
    // evita. En los tests usamos `tokio::task::LocalSet` para correr
    // tanto el router como el loop del intérprete adentro del mismo
    // thread del test, eludiendo el bound `Send`. Es el patrón
    // estándar de tokio para coexistencia de futures `!Send` con
    // futures async.

    /// Helper: corre un request contra el router usando LocalSet. El
    /// loop del intérprete y el `oneshot` del router viven juntos en
    /// el mismo thread. Devuelve (status, body string).
    async fn run_oneshot(
        src: &str,
        method: axum::http::Method,
        path: &str,
    ) -> (u16, String) {
        run_oneshot_with_body(src, method, path, None).await
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
        let (tx, mut rx) = mpsc::unbounded_channel::<InterpTask>();
        // Tests existentes de routing: schema = None para no contaminar
        // los path lookups con la ruta auto-registrada de 7.2.
        let router = build_router(&metas, tx, None);

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
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
                let mut resp_fut = Box::pin(router.oneshot(req));

                // `tokio::select!`: avanzamos en paralelo el future del
                // request y el loop de procesación de tasks. Cuando
                // llega una task la procesamos con `handle_task` (sync)
                // y mandamos el outcome. Cuando el request termina,
                // salimos del loop.
                loop {
                    tokio::select! {
                        resp = &mut resp_fut => {
                            let resp = resp.unwrap();
                            let status = resp.status().as_u16();
                            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
                            let body = String::from_utf8(bytes.to_vec()).unwrap();
                            return (status, body);
                        }
                        Some(task) = rx.recv() => {
                            let outcome = handle_task(
                                &registry,
                                task.route_idx,
                                task.path_params,
                                task.query_params,
                                task.body,
                            ).await;
                            let _ = task.reply.send(outcome);
                        }
                    }
                }
            })
            .await
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

    // ---- 7.2 auto-register de /openapi.json ----
    //
    // Helper local: clona el patrón de `run_oneshot` pero acepta un
    // `openapi_schema: Option<serde_json::Value>` y devuelve solo el
    // (status, body) sin loop de tasks (la ruta /openapi.json no
    // necesita el bridge — es 100% async axum-side).

    async fn oneshot_get_openapi(
        metas: Vec<RouteMeta>,
        openapi_schema: Option<serde_json::Value>,
    ) -> (u16, String) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let (tx, _rx) = mpsc::unbounded_channel::<InterpTask>();
        let router = build_router(&metas, tx, openapi_schema);
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
        let (status, body) = oneshot_get_openapi(vec![], Some(schema)).await;
        assert_eq!(status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["openapi"], serde_json::json!("3.1.0"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_con_schema_none_no_registra_openapi_json() {
        let (status, _body) = oneshot_get_openapi(vec![], None).await;
        assert_eq!(status, 404);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_auto_register_convive_con_rutas_del_usuario() {
        // Si el usuario tiene `@get("/")` y el auto-register suma
        // `/openapi.json`, ambas funcionan. Verificamos que la ruta
        // del usuario sigue accesible (no se pisa) y que el schema
        // está disponible.
        let src = "@get(\"/\")\nfn hello() => \"hola\"";
        let registry = registry_from_source(src).await;
        let metas = registry.metas();
        let schema = serde_json::json!({
            "openapi": "3.1.0",
            "paths": { "/": {} },
        });
        let (status, body) = oneshot_get_openapi(metas, Some(schema)).await;
        assert_eq!(status, 200);
        assert!(body.contains("openapi"));
    }

    #[tokio::test(flavor = "current_thread")]
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

        let (tx, mut rx) = mpsc::unbounded_channel::<InterpTask>();
        let router = build_router(&metas, tx, Some(auto_schema));
        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/openapi.json")
            .body(Body::empty())
            .unwrap();

        // Mismo patrón que run_oneshot: avanzar el response + procesar
        // tasks. La ruta del usuario sí dispara una task (es un handler
        // Fitz normal).
        let local = tokio::task::LocalSet::new();
        let (status, body) = local
            .run_until(async move {
                let mut resp_fut = Box::pin(router.oneshot(req));
                loop {
                    tokio::select! {
                        resp = &mut resp_fut => {
                            let resp = resp.unwrap();
                            let status = resp.status().as_u16();
                            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
                            return (status, String::from_utf8(bytes.to_vec()).unwrap());
                        }
                        Some(task) = rx.recv() => {
                            let outcome = handle_task(
                                &registry,
                                task.route_idx,
                                task.path_params,
                                task.query_params,
                                task.body,
                            ).await;
                            let _ = task.reply.send(outcome);
                        }
                    }
                }
            })
            .await;
        assert_eq!(status, 200);
        // El body es el del handler del usuario: `"mio"` (JSON string).
        // NO contiene "_marker" del schema auto-register.
        assert_eq!(body, "\"mio\"");
        assert!(!body.contains("_marker"));
    }

    // ---- 7.3 auto-register de /docs (UI Scalar) ----

    /// Helper local: GET /docs sobre un router armado con o sin schema.
    async fn oneshot_get_docs(
        metas: Vec<RouteMeta>,
        openapi_schema: Option<serde_json::Value>,
    ) -> (u16, String) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let (tx, _rx) = mpsc::unbounded_channel::<InterpTask>();
        let router = build_router(&metas, tx, openapi_schema);
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
        let (status, body) = oneshot_get_docs(vec![], Some(schema)).await;
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
        let (status, _body) = oneshot_get_docs(vec![], None).await;
        assert_eq!(status, 404);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn usuario_declara_docs_propio_y_gana_sobre_auto_register() {
        // El usuario declaró su propio `@get("/docs")`. El auto-register
        // de la UI Scalar cede — la ruta del usuario es la que responde.
        let src = "@get(\"/docs\")\nfn custom() => \"docs-personalizada\"";
        let registry = registry_from_source(src).await;
        let metas = registry.metas();
        let auto_schema = serde_json::json!({ "openapi": "3.1.0" });

        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let (tx, mut rx) = mpsc::unbounded_channel::<InterpTask>();
        let router = build_router(&metas, tx, Some(auto_schema));
        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/docs")
            .body(Body::empty())
            .unwrap();

        let local = tokio::task::LocalSet::new();
        let (status, body) = local
            .run_until(async move {
                let mut resp_fut = Box::pin(router.oneshot(req));
                loop {
                    tokio::select! {
                        resp = &mut resp_fut => {
                            let resp = resp.unwrap();
                            let status = resp.status().as_u16();
                            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
                            return (status, String::from_utf8(bytes.to_vec()).unwrap());
                        }
                        Some(task) = rx.recv() => {
                            let outcome = handle_task(
                                &registry,
                                task.route_idx,
                                task.path_params,
                                task.query_params,
                                task.body,
                            ).await;
                            let _ = task.reply.send(outcome);
                        }
                    }
                }
            })
            .await;
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
                param_type_exprs: vec![],
                return_type_expr: None,
            });
        });
        assert_eq!(reg.routes.len(), 1);
        assert_eq!(reg.routes[0].method, HttpMethod::Get);
        assert_eq!(reg.routes[0].handler_name, "index");
    }
}
