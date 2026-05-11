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

use crate::ast::Expr;
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
    /// `Expr::Str` o `Expr::StrInterp` del decorator.
    pub path: String,
    /// Nombres de los path params, en el orden en que aparecen en el
    /// path. Vacío si la ruta no tiene params.
    pub path_params: Vec<String>,
    /// Handler Fitz. Tiene que ser `Value::Function` — el evaluator
    /// valida esto en registro.
    pub handler: Value,
    /// Nombre del handler para mensajes de error/log.
    pub handler_name: String,
    /// Tipos declarados de los parámetros del handler, en orden. Cada
    /// uno es `Option<String>` igual que en el AST. Sirve para
    /// convertir path params crudos (siempre llegan como string desde
    /// axum) al tipo Fitz correspondiente antes de invocar al handler.
    pub param_types: Vec<(String, Option<String>)>,
    /// Si el handler declara un parámetro que no es path param, lo
    /// tratamos como body. Acá guardamos su nombre y, opcionalmente,
    /// el `Value::Type` declarado (resuelto del env en momento de
    /// registro). Si el tipo no está declarado, deserializamos el
    /// JSON como `Value` libre (Map/List/primitivos).
    ///
    /// Máximo un body por handler. La validación de cuántos hay y
    /// que sean compatibles la hace el evaluator durante el registro.
    pub body_param: Option<BodyParam>,
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
    /// declarado, `None`.
    pub declared_type_name: Option<String>,
}

/// Configuración del servidor que un `@server(...)` pudo haber
/// declarado en el programa. Si está en `None`, se usan defaults
/// (127.0.0.1:3000). Solo se admite un `@server` por programa —
/// la unicidad la enforcea el evaluator durante el registro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl ServerConfig {
    /// Defaults aplicados cuando no hay `@server` en el programa.
    pub fn default_addr() -> Self {
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 3000,
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
    /// Path en formato axum: `/users/{id}`, `/`, `/users`.
    pub path: String,
    /// Nombres de los path params en el orden de aparición.
    pub params: Vec<String>,
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
        }
    }
}

/// Toma la expresión que el parser dejó como primer arg de un
/// decorator HTTP y la convierte a un `PathTemplate`. Acepta dos
/// formas:
///
///  - `Expr::Str(s)`: path sin params. Ej: `"/"`, `"/users"`.
///  - `Expr::StrInterp(parts)`: path con params. Cada `StrPart::Expr`
///    tiene que ser un `Ident` simple (`{id}`). Cualquier otra cosa
///    es error.
///
/// Cualquier otra forma de expresión → `PathError::NotAStringLiteral`.
pub fn parse_path_template(expr: &Expr) -> Result<PathTemplate, PathError> {
    use crate::ast::StrPart;

    let (path, params): (String, Vec<String>) = match expr {
        Expr::Str(s) => (s.clone(), Vec::new()),
        Expr::StrInterp(parts) => {
            let mut buf = String::new();
            let mut params = Vec::new();
            for part in parts {
                match part {
                    StrPart::Lit(s) => buf.push_str(s),
                    StrPart::Expr(Expr::Ident(name)) => {
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

    if !path.starts_with('/') {
        return Err(PathError::MustStartWithSlash);
    }

    Ok(PathTemplate { path, params })
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
        } else if field.default.is_some() {
            // Los defaults son `Expr` y se evalúan en el env de
            // instanciación. Acá no tenemos env porque el body se
            // valida lejos del eval. Para 4.3, los defaults sólo
            // funcionan si son literales constantes simples; otros
            // casos requieren más cableado. Lo manejamos en
            // `default_to_value` (helper local).
            match default_to_value(field.default.as_ref().unwrap()) {
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
        } else if field.nullable {
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
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(f) => Ok(Value::Float(*f)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),
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
//   │  │ async fn    │ │              │    call_handler(...) │
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
pub fn build_router(metas: &[RouteMeta], tx: TaskTx) -> Router {
    let mut router = Router::new();
    for (idx, meta) in metas.iter().enumerate() {
        let route_handler = build_method_router(
            meta.method,
            idx,
            tx.clone(),
            meta.has_path_params,
            meta.expects_body,
        );
        router = router.route(&meta.path, route_handler);
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
    expects_body: bool,
) -> MethodRouter {
    use axum::routing::{delete, get, post, put};

    match (has_path_params, expects_body) {
        (false, false) => {
            let h = move || {
                let tx = tx.clone();
                async move {
                    dispatch_request(route_idx, HashMap::new(), Vec::new(), tx).await
                }
            };
            wrap(method, h)
        }
        (true, false) => {
            let h = move |AxumPath(params): AxumPath<HashMap<String, String>>| {
                let tx = tx.clone();
                async move { dispatch_request(route_idx, params, Vec::new(), tx).await }
            };
            wrap(method, h)
        }
        (false, true) => {
            let h = move |body: axum::body::Bytes| {
                let tx = tx.clone();
                async move {
                    dispatch_request(route_idx, HashMap::new(), body.to_vec(), tx).await
                }
            };
            wrap(method, h)
        }
        (true, true) => {
            let h = move |AxumPath(params): AxumPath<HashMap<String, String>>,
                          body: axum::body::Bytes| {
                let tx = tx.clone();
                async move {
                    dispatch_request(route_idx, params, body.to_vec(), tx).await
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
    body: Vec<u8>,
    tx: TaskTx,
) -> Response {
    let (reply_tx, reply_rx) = oneshot::channel();
    let task = InterpTask {
        route_idx,
        path_params,
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
    while let Some(task) = rx.blocking_recv() {
        let outcome = handle_task(&registry, task.route_idx, task.path_params, task.body);
        // Si el oneshot del lado axum se cerró (cliente desconectado,
        // timeout), no hay nada que hacer con el outcome — descartar.
        let _ = task.reply.send(outcome);
    }
}

/// Procesa un único task. Aislado del loop para testearlo sin canal.
fn handle_task(
    registry: &HttpRegistry,
    route_idx: usize,
    raw_path_params: HashMap<String, String>,
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
    //   - si su nombre coincide con un path param, coercionar el
    //     valor crudo al tipo declarado;
    //   - si es el body param, usar el valor parseado;
    //   - cualquier otro caso (no path, no body) es un bug del
    //     registro: el evaluator no permite registrarlo.
    let mut args = Vec::with_capacity(route.param_types.len());
    for (name, declared_type) in &route.param_types {
        if let Some(raw) = raw_path_params.get(name) {
            match coerce_path_param(raw, declared_type.as_deref()) {
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
        } else if route.body_param.as_ref().map(|bp| bp.name.as_str()) == Some(name) {
            // Body param: ya parseado arriba; tomarlo de `body_value`.
            // unwrap es seguro porque body_value es Some sii hay body_param.
            args.push(body_value.clone().unwrap());
        } else {
            return HandlerOutcome::internal_error(format!(
                "parámetro '{}' del handler '{}' no es ni path param ni body — \
                 esto es un bug interno del registro",
                name, route.handler_name,
            ));
        }
    }

    // Invocar el handler. Errores del handler (return propio, error
    // de runtime) se traducen a 500 con el mensaje.
    match call_handler(route.handler.clone(), args, &route.handler_name) {
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
pub fn serve(registry: HttpRegistry, addr: std::net::SocketAddr) -> std::io::Result<()> {
    use std::thread;

    let (tx, rx) = mpsc::unbounded_channel::<InterpTask>();
    let metas = registry.metas();

    // Thread tokio: owns el runtime async y el server axum. Solo
    // recibe metadata + tx (todos `Send`).
    let tokio_handle = thread::Builder::new()
        .name("fitz-http".into())
        .spawn(move || -> std::io::Result<()> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async move {
                let router = build_router(&metas, tx);
                let listener = tokio::net::TcpListener::bind(addr).await?;
                eprintln!("🏔️  Fitz HTTP escuchando en http://{}", addr);
                for meta in &metas {
                    eprintln!("   {} {}", meta.method.as_str(), meta.path);
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
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::Other,
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
mod tests {
    use super::*;
    use crate::ast::StrPart;
    use crate::value::shared;

    // ---- HttpMethod ----

    #[test]
    fn http_method_desde_nombre_de_decorator() {
        assert_eq!(HttpMethod::from_decorator_name("get"), Some(HttpMethod::Get));
        assert_eq!(HttpMethod::from_decorator_name("post"), Some(HttpMethod::Post));
        assert_eq!(HttpMethod::from_decorator_name("put"), Some(HttpMethod::Put));
        assert_eq!(HttpMethod::from_decorator_name("delete"), Some(HttpMethod::Delete));
        assert_eq!(HttpMethod::from_decorator_name("server"), None);
        assert_eq!(HttpMethod::from_decorator_name("patch"), None);
    }

    // ---- parse_path_template ----

    #[test]
    fn path_str_simple_sin_params() {
        let t = parse_path_template(&Expr::Str("/".into())).unwrap();
        assert_eq!(t.path, "/");
        assert!(t.params.is_empty());

        let t = parse_path_template(&Expr::Str("/users".into())).unwrap();
        assert_eq!(t.path, "/users");
        assert!(t.params.is_empty());
    }

    #[test]
    fn path_strinterp_con_un_param() {
        // `"/users/{id}"` → StrInterp([Lit("/users/"), Expr(Ident("id"))])
        let e = Expr::StrInterp(vec![
            StrPart::Lit("/users/".into()),
            StrPart::Expr(Expr::Ident("id".into())),
        ]);
        let t = parse_path_template(&e).unwrap();
        assert_eq!(t.path, "/users/{id}");
        assert_eq!(t.params, vec!["id".to_string()]);
    }

    #[test]
    fn path_strinterp_con_varios_params_distintos() {
        // `"/orgs/{org}/users/{id}"`
        let e = Expr::StrInterp(vec![
            StrPart::Lit("/orgs/".into()),
            StrPart::Expr(Expr::Ident("org".into())),
            StrPart::Lit("/users/".into()),
            StrPart::Expr(Expr::Ident("id".into())),
        ]);
        let t = parse_path_template(&e).unwrap();
        assert_eq!(t.path, "/orgs/{org}/users/{id}");
        assert_eq!(t.params, vec!["org".to_string(), "id".to_string()]);
    }

    #[test]
    fn path_no_arranca_con_slash_es_error() {
        let err = parse_path_template(&Expr::Str("users".into())).unwrap_err();
        assert_eq!(err, PathError::MustStartWithSlash);
    }

    #[test]
    fn path_con_expresion_no_ident_es_error() {
        // `"{a+b}"` — interpolación con BinOp.
        let e = Expr::StrInterp(vec![
            StrPart::Lit("/".into()),
            StrPart::Expr(Expr::BinOp {
                op: crate::ast::BinOpKind::Add,
                left: Box::new(Expr::Ident("a".into())),
                right: Box::new(Expr::Ident("b".into())),
            }),
        ]);
        let err = parse_path_template(&e).unwrap_err();
        assert!(matches!(err, PathError::UnsupportedInterpolation(_)));
    }

    #[test]
    fn path_con_params_duplicados_es_error() {
        // `"/a/{x}/b/{x}"`
        let e = Expr::StrInterp(vec![
            StrPart::Lit("/a/".into()),
            StrPart::Expr(Expr::Ident("x".into())),
            StrPart::Lit("/b/".into()),
            StrPart::Expr(Expr::Ident("x".into())),
        ]);
        let err = parse_path_template(&e).unwrap_err();
        assert_eq!(err, PathError::DuplicateParam("x".into()));
    }

    #[test]
    fn path_no_string_literal_es_error() {
        // `@get(42)` — Int en lugar de string.
        let err = parse_path_template(&Expr::Int(42)).unwrap_err();
        assert_eq!(err, PathError::NotAStringLiteral);
    }

    // ---- value_to_json ----

    #[test]
    fn value_to_json_primitivos() {
        assert_eq!(value_to_json(&Value::Int(42)).unwrap(), serde_json::json!(42));
        assert_eq!(value_to_json(&Value::Float(3.14)).unwrap(), serde_json::json!(3.14));
        assert_eq!(value_to_json(&Value::Str("hola".into())).unwrap(), serde_json::json!("hola"));
        assert_eq!(value_to_json(&Value::Bool(true)).unwrap(), serde_json::json!(true));
        assert_eq!(value_to_json(&Value::Null).unwrap(), serde_json::json!(null));
    }

    #[test]
    fn value_to_json_lista() {
        let v = Value::List(shared(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ]));
        assert_eq!(value_to_json(&v).unwrap(), serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn value_to_json_mapa_con_claves_string() {
        let v = Value::Map(shared(vec![
            (Value::Str("name".into()), Value::Str("fitz".into())),
            (Value::Str("port".into()), Value::Int(3000)),
        ]));
        assert_eq!(
            value_to_json(&v).unwrap(),
            serde_json::json!({ "name": "fitz", "port": 3000 }),
        );
    }

    #[test]
    fn value_to_json_mapa_clave_no_string_es_error() {
        let v = Value::Map(shared(vec![(Value::Int(1), Value::Int(10))]));
        let err = value_to_json(&v).unwrap_err();
        assert!(err.contains("claves de Map en JSON"));
    }

    #[test]
    fn value_to_json_instance() {
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

    #[test]
    fn value_to_json_result_anidado_se_etiqueta() {
        // `Ok(42)` adentro de otra cosa (no debería pasar en el output
        // directo del handler, pero queremos un comportamiento total).
        let ok = Value::Result(ResultVariant::Ok(Box::new(Value::Int(42))));
        assert_eq!(value_to_json(&ok).unwrap(), serde_json::json!({ "Ok": 42 }));

        let err = Value::Result(ResultVariant::Err(Box::new(Value::Str("boom".into()))));
        assert_eq!(value_to_json(&err).unwrap(), serde_json::json!({ "Err": "boom" }));
    }

    #[test]
    fn value_to_json_function_es_error() {
        // Function no es serializable.
        let env = crate::env::Environment::new();
        let v = Value::Function {
            params: vec![],
            body: vec![],
            closure: env,
        };
        let err = value_to_json(&v).unwrap_err();
        assert!(err.contains("Function"));
    }

    // ---- value_to_outcome (handler → status + body) ----

    #[test]
    fn outcome_de_value_pelado_es_200() {
        let v = Value::Str("hola".into());
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 200);
        assert_eq!(out.body, "\"hola\"");
        assert_eq!(out.content_type, "application/json");
    }

    #[test]
    fn outcome_de_ok_es_200_con_inner() {
        let v = Value::Result(ResultVariant::Ok(Box::new(Value::Int(42))));
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 200);
        assert_eq!(out.body, "42");
    }

    #[test]
    fn outcome_de_err_es_500_con_error_obj() {
        let v = Value::Result(ResultVariant::Err(Box::new(Value::Str(
            "no encontrado".into(),
        ))));
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 500);
        // Body es `{"error":"no encontrado"}` (orden de serde_json).
        assert_eq!(out.body, "{\"error\":\"no encontrado\"}");
    }

    #[test]
    fn outcome_de_instance_es_objeto_json() {
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

    #[test]
    fn outcome_de_tipo_no_serializable_es_500() {
        // Range no es serializable.
        let v = Value::Range { start: 0, end: 10 };
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 500);
        assert!(out.body.contains("Range"));
    }

    // ---- coerce_path_param ----

    #[test]
    fn path_param_default_a_str_sin_anotacion() {
        let v = coerce_path_param("42", None).unwrap();
        assert_eq!(v, Value::Str("42".into()));
    }

    #[test]
    fn path_param_int_se_parsea_a_int() {
        let v = coerce_path_param("42", Some("Int")).unwrap();
        assert_eq!(v, Value::Int(42));
    }

    #[test]
    fn path_param_int_invalido_es_error() {
        let err = coerce_path_param("abc", Some("Int")).unwrap_err();
        assert!(err.contains("Int") && err.contains("abc"));
    }

    #[test]
    fn path_param_float_se_parsea() {
        let v = coerce_path_param("3.14", Some("Float")).unwrap();
        assert_eq!(v, Value::Float(3.14));
    }

    #[test]
    fn path_param_bool_true_false() {
        assert_eq!(coerce_path_param("true", Some("Bool")).unwrap(), Value::Bool(true));
        assert_eq!(coerce_path_param("false", Some("Bool")).unwrap(), Value::Bool(false));
        assert!(coerce_path_param("maybe", Some("Bool")).is_err());
    }

    #[test]
    fn path_param_tipo_no_soportado_es_error() {
        // Un tipo custom no entra como path param: el handler tiene
        // que recibir el id raw y reconstruir el objeto adentro.
        let err = coerce_path_param("42", Some("User")).unwrap_err();
        assert!(err.contains("User"));
    }

    // ---- registry ----

    #[test]
    fn registry_arranca_sin_rutas() {
        let r = HttpRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.routes.len(), 0);
    }

    #[test]
    fn with_active_registry_expone_has_active_para_el_evaluator() {
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
    fn registry_from_source(src: &str) -> HttpRegistry {
        let (res, registry) = with_active_registry(|| {
            let tokens = crate::lexer::tokenize(src).unwrap();
            let program = crate::parser::parse(tokens).unwrap();
            crate::evaluator::eval(program)
        });
        res.unwrap();
        registry
    }

    #[test]
    fn handle_task_invoca_handler_y_devuelve_outcome() {
        // `@get("/") fn hello() => "hola"`
        let src = "@get(\"/\")\nfn hello() => \"hola\"";
        let registry = registry_from_source(src);
        let outcome = handle_task(&registry, 0, HashMap::new(), Vec::new());
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"hola\"");
    }

    #[test]
    fn handle_task_coerciona_path_param_int() {
        let src = "@get(\"/users/{id}\")\nfn h(id: Int) => id * 2";
        let registry = registry_from_source(src);
        let mut params = HashMap::new();
        params.insert("id".into(), "21".into());
        let outcome = handle_task(&registry, 0, params, Vec::new());
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "42");
    }

    #[test]
    fn handle_task_path_param_int_invalido_es_400() {
        let src = "@get(\"/users/{id}\")\nfn h(id: Int) => id";
        let registry = registry_from_source(src);
        let mut params = HashMap::new();
        params.insert("id".into(), "no-es-int".into());
        let outcome = handle_task(&registry, 0, params, Vec::new());
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("Int"));
    }

    #[test]
    fn handle_task_handler_que_retorna_err_es_500_con_error() {
        // El handler devuelve Err("boom"): runtime lo traduce a 500.
        let src = "@get(\"/\")\nfn h() => Err(\"boom\")";
        let registry = registry_from_source(src);
        let outcome = handle_task(&registry, 0, HashMap::new(), Vec::new());
        assert_eq!(outcome.status, 500);
        assert!(outcome.body.contains("boom"));
    }

    #[test]
    fn handle_task_handler_que_retorna_instance_serializa_a_json() {
        let src = "\
            type User { id: Int, name: Str }\n\
            @get(\"/u\")\nfn h() => User { id: 1, name: \"ana\" }\n\
        ";
        let registry = registry_from_source(src);
        let outcome = handle_task(&registry, 0, HashMap::new(), Vec::new());
        assert_eq!(outcome.status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&outcome.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "id": 1, "name": "ana" }));
    }

    // ---- ServerConfig (Fase 4.4) ----

    #[test]
    fn server_config_default_es_localhost_3000() {
        let c = ServerConfig::default_addr();
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.port, 3000);
    }

    #[test]
    fn server_config_to_socket_addr_ipv4_ok() {
        let c = ServerConfig {
            host: "0.0.0.0".into(),
            port: 8080,
        };
        let addr = c.to_socket_addr().unwrap();
        assert_eq!(addr.to_string(), "0.0.0.0:8080");
    }

    #[test]
    fn server_config_to_socket_addr_host_invalido_es_error() {
        let c = ServerConfig {
            host: "no-es-ip".into(),
            port: 80,
        };
        let err = c.to_socket_addr().unwrap_err();
        assert!(err.contains("no-es-ip"));
    }

    #[test]
    fn set_server_config_segunda_vez_devuelve_existente() {
        let ((), _reg) = with_active_registry(|| {
            let first = ServerConfig {
                host: "127.0.0.1".into(),
                port: 8080,
            };
            assert!(set_server_config(first.clone()).is_ok());
            let second = ServerConfig {
                host: "0.0.0.0".into(),
                port: 9090,
            };
            let err = set_server_config(second).unwrap_err();
            // El error contiene el config existente, no el nuevo.
            assert_eq!(err, first);
        });
    }

    #[test]
    fn registry_resolved_config_devuelve_default_si_no_hay_explicito() {
        let mut reg = HttpRegistry::new();
        assert!(reg.server_config.is_none());
        assert_eq!(reg.resolved_config(), ServerConfig::default_addr());
        // Con config explícito sí.
        reg.server_config = Some(ServerConfig {
            host: "0.0.0.0".into(),
            port: 80,
        });
        let resolved = reg.resolved_config();
        assert_eq!(resolved.port, 80);
        assert_eq!(resolved.host, "0.0.0.0");
    }

    // ---- json_to_value (deserialización libre) ----

    #[test]
    fn json_to_value_primitivos() {
        assert_eq!(json_to_value(&serde_json::json!(null)), Value::Null);
        assert_eq!(json_to_value(&serde_json::json!(true)), Value::Bool(true));
        assert_eq!(json_to_value(&serde_json::json!(42)), Value::Int(42));
        assert_eq!(json_to_value(&serde_json::json!(3.14)), Value::Float(3.14));
        assert_eq!(
            json_to_value(&serde_json::json!("hola")),
            Value::Str("hola".into())
        );
    }

    #[test]
    fn json_to_value_array_se_vuelve_list() {
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

    #[test]
    fn json_to_value_object_se_vuelve_map_con_claves_str() {
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
    /// campo es `(nombre, tipo, nullable, default)`.
    fn type_value(name: &str, fields: Vec<(&str, &str, bool, Option<Expr>)>) -> Value {
        Value::Type {
            name: name.into(),
            fields: fields
                .into_iter()
                .map(|(n, t, nullable, default)| crate::ast::Field {
                    name: n.into(),
                    type_: t.into(),
                    nullable,
                    default,
                })
                .collect(),
        }
    }

    #[test]
    fn json_to_instance_caso_feliz() {
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

    #[test]
    fn json_to_instance_campo_faltante_sin_default_ni_nullable_es_error() {
        let t = type_value("User", vec![
            ("id", "Int", false, None),
            ("name", "Str", false, None),
        ]);
        let json = serde_json::json!({ "id": 1 });
        let err = json_to_instance(&json, &t).unwrap_err();
        assert!(err.contains("name"));
        assert!(err.contains("falta"));
    }

    #[test]
    fn json_to_instance_campo_extra_es_error() {
        let t = type_value("User", vec![("id", "Int", false, None)]);
        let json = serde_json::json!({ "id": 1, "rogue": "x" });
        let err = json_to_instance(&json, &t).unwrap_err();
        assert!(err.contains("rogue"));
        assert!(err.contains("no declarado"));
    }

    #[test]
    fn json_to_instance_campo_nullable_faltante_queda_null() {
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

    #[test]
    fn json_to_instance_default_literal_se_usa_si_falta() {
        let t = type_value("User", vec![
            ("id", "Int", false, None),
            ("active", "Bool", false, Some(Expr::Bool(true))),
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

    #[test]
    fn json_to_instance_body_no_objeto_es_error() {
        let t = type_value("User", vec![("id", "Int", false, None)]);
        let json = serde_json::json!([1, 2, 3]);
        let err = json_to_instance(&json, &t).unwrap_err();
        assert!(err.contains("objeto"));
        assert!(err.contains("array"));
    }

    // ---- handle_task con body ----

    #[test]
    fn handle_task_post_sin_body_pero_handler_lo_espera_es_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let registry = registry_from_source(src);
        let outcome = handle_task(&registry, 0, HashMap::new(), Vec::new());
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("body requerido"));
    }

    #[test]
    fn handle_task_post_con_body_valido_construye_instance() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body.name\n\
        ";
        let registry = registry_from_source(src);
        let body = br#"{"name":"fitz"}"#.to_vec();
        let outcome = handle_task(&registry, 0, HashMap::new(), body);
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"fitz\"");
    }

    #[test]
    fn handle_task_post_body_json_invalido_es_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let registry = registry_from_source(src);
        let outcome = handle_task(&registry, 0, HashMap::new(), b"not json".to_vec());
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("JSON"));
    }

    #[test]
    fn handle_task_post_body_campo_faltante_es_400() {
        let src = "\
            type UserInput { name: Str, email: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let registry = registry_from_source(src);
        let body = br#"{"name":"fitz"}"#.to_vec();
        let outcome = handle_task(&registry, 0, HashMap::new(), body);
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("email"));
    }

    #[test]
    fn handle_task_put_con_path_param_y_body() {
        let src = "\
            type UserInput { name: Str }\n\
            @put(\"/users/{id}\")\nfn upd(id: Int, body: UserInput) => body.name\n\
        ";
        let registry = registry_from_source(src);
        let mut params = HashMap::new();
        params.insert("id".into(), "7".into());
        let body = br#"{"name":"ana"}"#.to_vec();
        let outcome = handle_task(&registry, 0, params, body);
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"ana\"");
    }

    #[test]
    fn handle_task_body_sin_anotacion_de_tipo_acepta_libre() {
        // `body` sin tipo → llega como Map<Str,Value>.
        let src = "\
            @post(\"/log\")\nfn log(body) => body[\"name\"]\n\
        ";
        let registry = registry_from_source(src);
        let body = br#"{"name":"x"}"#.to_vec();
        let outcome = handle_task(&registry, 0, HashMap::new(), body);
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

        let registry = registry_from_source(src);
        let metas = registry.metas();
        let (tx, mut rx) = mpsc::unbounded_channel::<InterpTask>();
        let router = build_router(&metas, tx);

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
                                task.body,
                            );
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

    #[test]
    fn push_route_acumula_en_el_registry_activo() {
        let ((), reg) = with_active_registry(|| {
            let env = crate::env::Environment::new();
            let handler = Value::Function {
                params: vec![],
                body: vec![],
                closure: env,
            };
            push_route(RouteSpec {
                method: HttpMethod::Get,
                path: "/".into(),
                path_params: vec![],
                handler,
                handler_name: "index".into(),
                param_types: vec![],
                body_param: None,
            });
        });
        assert_eq!(reg.routes.len(), 1);
        assert_eq!(reg.routes[0].method, HttpMethod::Get);
        assert_eq!(reg.routes[0].handler_name, "index");
    }
}
