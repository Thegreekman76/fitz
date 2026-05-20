// openapi.rs — Fase 7.1: generador de schema OpenAPI 3.1.
//
// Consume el `HttpRegistry` poblado durante `eval` y el AST del
// programa, y produce un `serde_json::Value` listo para serializar.
// El generador no tiene estado; recibe todo por parámetro y devuelve
// el JSON. Lo invocan:
//   - el subcomando `fitz openapi archivo.fitz` (escupe el JSON a
//     stdout, útil para CI / pipeline de SDKs).
//   - el endpoint `/openapi.json` autoregistrado en `fitz run` (7.2).
//   - el codegen del binario nativo (7.5) — el schema se emite en
//     build time y se embebe como `&'static str`.
//
// Decisión de diseño: usamos OpenAPI 3.1 (incluye JSON Schema 2020-12
// completo). Es lo que consume Scalar, Postman, Insomnia, openapi-generator.
//
// Limitaciones aceptadas en 7.1 (documentadas en el roadmap):
//   - `info.description` y `paths.*.*.description` vacíos: los doc-strings
//     sobre handlers son deuda post-F7 (el lexer hoy descarta comentarios).
//   - `info.version` fijo en "0.1.0".
//   - Status codes custom (`return 404 { ... }`): el schema solo emite
//     200 (caso feliz) + 500 (Err si return es Result). Códigos custom
//     específicos quedan como deuda menor — la info vive en `Stmt::ReturnStatus`
//     pero requiere análisis del body del handler para enumerarlos.

use serde_json::{json, Map, Value};

use crate::ast::{Field, Program, Stmt, TypeExpr};
use crate::http::{HttpMethod, HttpRegistry, RouteSpec};

/// Vista liviana de una ruta HTTP — solo los campos que el generador
/// OpenAPI necesita. Se construye desde un `RouteSpec` del runtime
/// (`routes_from_registry`) o desde el AST en build-time del codegen
/// (`pseudo_routes_from_ast` en codegen.rs).
///
/// Desacopla el generator del `Value` del runtime: el codegen no
/// necesita inventar `Value::Function` dummies para alimentar el
/// schema.
#[derive(Debug, Clone)]
pub struct OpenApiRouteInfo {
    pub method: HttpMethod,
    pub path: String,
    pub handler_name: String,
    pub path_params: Vec<String>,
    pub query_params: Vec<String>,
    /// Nombre del param que el handler interpreta como body, si existe.
    /// El tipo del body se mira en `param_type_exprs` por nombre.
    pub body_param_name: Option<String>,
    /// Headers declarados con `@header(name="X")` sobre el handler
    /// (Fase 7.6). Cada entry es `(http_name, fitz_param_name,
    /// is_nullable)`. El schema OpenAPI los emite como `parameters`
    /// con `in: "header"`.
    pub header_params: Vec<(String, String, bool)>,
    pub param_type_exprs: Vec<(String, Option<TypeExpr>)>,
    pub return_type_expr: Option<TypeExpr>,
    /// Mini-fase Q.4: status codes custom (`return <Int> { ... }`)
    /// detectados en el body del handler. Cada uno genera un entry en
    /// `responses` del schema OpenAPI además de los derivados del
    /// return type. Vec ordenado ascendente y deduplicado para schema
    /// determinista. Status no literales (variable, expr) se omiten —
    /// no son inferibles estáticamente.
    pub custom_status_codes: Vec<u16>,
}

/// Adapter: del registry runtime a vistas livianas.
pub fn routes_from_registry(reg: &HttpRegistry) -> Vec<OpenApiRouteInfo> {
    reg.routes.iter().map(route_info_from_spec).collect()
}

fn route_info_from_spec(s: &RouteSpec) -> OpenApiRouteInfo {
    OpenApiRouteInfo {
        method: s.method,
        path: s.path.clone(),
        handler_name: s.handler_name.clone(),
        path_params: s.path_params.clone(),
        query_params: s.query_params.clone(),
        body_param_name: s.body_param.as_ref().map(|b| b.name.clone()),
        header_params: s
            .headers
            .iter()
            .map(|h| (h.http_name.clone(), h.param_name.clone(), h.is_nullable))
            .collect(),
        param_type_exprs: s.param_type_exprs.clone(),
        return_type_expr: s.return_type_expr.clone(),
        // Q.4: extraer los status codes custom del body del handler.
        // El handler runtime es un `Value::Function { body, ... }`; si
        // por alguna razón no lo es (registro inconsistente), tratamos
        // como sin status codes (defensivo).
        custom_status_codes: match &s.handler {
            crate::value::Value::Function { body, .. } => collect_status_codes(body),
            _ => Vec::new(),
        },
    }
}

/// Mini-fase Q.4: recorre un body de fn y devuelve los `Stmt::ReturnStatus`
/// con status literal Int encontrados. Status no literales (variables,
/// expresiones) se omiten — no podemos saberlos estáticamente. Recurse
/// adentro de loops, if/match, etc.; FnExpr inline NO se sigue (otro
/// scope, otra fn). El Vec devuelto está deduplicado y en orden
/// ascendente para que el schema sea determinista.
pub fn collect_status_codes(body: &[crate::ast::Stmt]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for s in body {
        collect_status_codes_stmt(s, &mut out);
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn collect_status_codes_stmt(stmt: &crate::ast::Stmt, out: &mut Vec<u16>) {
    use crate::ast::Stmt;
    match stmt {
        Stmt::ReturnStatus { status, body, .. } => {
            if let crate::ast::Expr::Int(n, _) = status {
                // Status fuera de rango HTTP válido (100-599) lo
                // skipeamos también — el runtime/parser lo cazaría.
                if (100..=599).contains(n) {
                    out.push(*n as u16);
                }
            }
            // El body puede contener otro ReturnStatus anidado vía
            // if/match — recorremos.
            if let Some(b) = body {
                collect_status_codes_expr(b, out);
            }
        }
        Stmt::While { body, .. } | Stmt::Loop { body, .. } | Stmt::For { body, .. } => {
            for s in body {
                collect_status_codes_stmt(s, out);
            }
        }
        Stmt::Assign { value, .. } => collect_status_codes_expr(value, out),
        Stmt::Return(e, _) | Stmt::Expr(e, _) => collect_status_codes_expr(e, out),
        _ => {}
    }
}

fn collect_status_codes_expr(expr: &crate::ast::Expr, out: &mut Vec<u16>) {
    use crate::ast::Expr;
    match expr {
        Expr::If { then, else_, .. } => {
            for s in then {
                collect_status_codes_stmt(s, out);
            }
            if let Some(els) = else_ {
                for s in els {
                    collect_status_codes_stmt(s, out);
                }
            }
        }
        Expr::Match { arms, .. } => {
            for a in arms {
                for s in &a.body {
                    collect_status_codes_stmt(s, out);
                }
            }
        }
        // Los demás Expr no tienen bodies anidados con stmts (calls,
        // literales, binops, etc.).
        _ => {}
    }
}

/// Extrae los `@header(name="X")` del set de decorators de una fn
/// (Fase 7.6). Devuelve `Vec<(http_name, param_fitz, is_nullable)>`.
/// Replica la lógica de `collect_headers` del evaluator. Asume que
/// el programa pasó el evaluator (los decorators son válidos), así
/// que silenciosamente skipea casos malformados — los errores los
/// caza el runtime al evaluar.
pub(crate) fn headers_from_decorators(
    decorators: &[crate::ast::Decorator],
    params: &[crate::ast::Param],
) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for deco in decorators {
        if deco.name != "header" {
            continue;
        }
        let Some(name_kw) = deco.kwargs.iter().find(|(k, _)| k == "name") else {
            continue;
        };
        let crate::ast::Expr::Str(http_name, _) = &name_kw.1 else {
            continue;
        };
        if http_name.is_empty() {
            continue;
        }
        // Mini-fase Q.1: `into="alias"` permite que el param Fitz tenga
        // un nombre distinto al derivado por convención. Si no está,
        // se mantiene la convención previa (lowercase + `-` → `_`).
        let param_name = match deco.kwargs.iter().find(|(k, _)| k == "into") {
            Some((_, crate::ast::Expr::Str(alias, _))) if !alias.is_empty() => alias.clone(),
            _ => http_name.to_lowercase().replace('-', "_"),
        };
        let Some(p) = params.iter().find(|p| p.name == param_name) else {
            continue;
        };
        let is_nullable = matches!(&p.type_, Some(t) if t.is_nullable());
        out.push((http_name.clone(), param_name, is_nullable));
    }
    out
}

/// Construye `OpenApiRouteInfo` desde el AST en build-time, sin
/// evaluar el programa (Fase 7.5). Se usa desde `codegen.rs` para que
/// `fitz build` pueda emitir el schema OpenAPI sin pasar por el
/// runtime HTTP.
///
/// La detección de body param replica la regla del evaluator: cualquier
/// param del handler que NO esté en el template del path ni en los
/// query params se considera body. Máximo uno por handler (validación
/// en el evaluator durante registro; acá no la repetimos — si el
/// programa pasa el evaluator, este AST es consistente).
///
/// Falla solo si `parse_path_template` rechaza el path (template
/// malformado).
pub fn pseudo_routes_from_ast(
    program: &crate::ast::Program,
) -> Result<Vec<OpenApiRouteInfo>, crate::error::FitzError> {
    use crate::ast::Stmt;
    use crate::http::parse_path_template;

    let mut out = Vec::new();
    for s in program {
        let Stmt::FnDef {
            name,
            params,
            return_type,
            decorators,
            body,
            ..
        } = s
        else {
            continue;
        };
        for d in decorators {
            let Some(method) = HttpMethod::from_decorator_name(&d.name) else {
                continue;
            };
            let Some(path_arg) = d.args.first() else {
                continue;
            };
            let template = parse_path_template(path_arg).map_err(|e| {
                crate::error::FitzError::new(
                    crate::error::ErrorKind::InvalidSyntax,
                    0,
                    0,
                    format!("@{} sobre fn '{}': {}", d.name, name, e.message()),
                )
            })?;
            // Fase 7.6: recolectar headers del mismo set de
            // decorators. Mismas reglas que `collect_headers` del
            // evaluator (derivación lowercase + `-` → `_`, validación
            // de tipos `Str` / `Str?`). En build-time replicamos la
            // lógica acá; si la fn pasa el evaluator, esta vista es
            // consistente.
            let header_params = headers_from_decorators(decorators, params);
            let body_param_name = params.iter().find_map(|p| {
                if !template.params.contains(&p.name)
                    && !template.query_params.contains(&p.name)
                    && !header_params.iter().any(|(_, fitz, _)| fitz == &p.name)
                {
                    Some(p.name.clone())
                } else {
                    None
                }
            });
            let param_type_exprs = params
                .iter()
                .map(|p| (p.name.clone(), p.type_.clone()))
                .collect();
            out.push(OpenApiRouteInfo {
                method,
                path: template.path,
                handler_name: name.clone(),
                path_params: template.params,
                query_params: template.query_params,
                body_param_name,
                header_params,
                param_type_exprs,
                return_type_expr: return_type.clone(),
                // Q.4: escanear el body del FnDef por ReturnStatus.
                custom_status_codes: collect_status_codes(body),
            });
        }
    }
    Ok(out)
}

/// HTML embebido para la UI de docs (Fase 7.3). Carga el bundle de
/// Scalar desde el CDN de jsdelivr y le apunta al `/openapi.json`
/// que el server sirve adyacente. ~10 líneas; el peso del binario
/// no se mueve (el bundle de Scalar baja en el browser, primera vez
/// que se visita `/docs`).
///
/// Trade-off documentado: la primera carga necesita red. Después el
/// navegador cachea. Si en el futuro queremos un bundle local
/// embebido (offline), se reemplaza el `<script src>` por
/// `include_bytes!` de un asset. Hoy es deuda post-F7.
pub const SCALAR_HTML: &str = include_str!("templates/scalar.html");

/// Genera el schema OpenAPI 3.1 del programa.
///
/// Entradas:
///   - `routes`: vistas livianas de las rutas HTTP (ver
///     `OpenApiRouteInfo`). En `fitz run` vienen del registry vía
///     `routes_from_registry`; en `fitz build` se construyen desde
///     el AST en build-time.
///   - `program`: AST original — necesario para recorrer los
///     `Stmt::TypeDef` y emitir `components.schemas`.
///
/// Salida: un `Value` que serializado con `serde_json::to_string_pretty`
/// es un OpenAPI 3.1 válido.
#[allow(dead_code)]
pub fn generate_openapi(routes: &[OpenApiRouteInfo], program: &Program) -> Value {
    generate_openapi_with_version(routes, program, None)
}

/// Variante de `generate_openapi` que acepta un `info.version` override
/// (mini-fase Q.2). Si `version` es `Some(v)`, el schema emite ese valor;
/// si es `None`, default `"0.1.0"` (compat con uso pre-Q.2). El runtime
/// lo lee de `HttpRegistry.server_config.api_version`; el codegen lo
/// lee del `Stmt::FnDef` decorado con `@server(api_version=...)`.
pub fn generate_openapi_with_version(
    routes: &[OpenApiRouteInfo],
    program: &Program,
    version: Option<&str>,
) -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Fitz API",
            "version": version.unwrap_or("0.1.0"),
        },
        "paths": build_paths(routes),
        "components": {
            "schemas": build_components_schemas(program),
        },
    })
}

// ---------- paths ----------

fn build_paths(routes: &[OpenApiRouteInfo]) -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for route in routes {
        let entry = paths
            .entry(route.path.clone())
            .or_insert_with(|| json!({}));
        let obj = entry
            .as_object_mut()
            .expect("entry inicializado como object literal");
        obj.insert(route.method.as_str().to_lowercase(), build_operation(route));
    }
    Value::Object(paths)
}

fn build_operation(route: &OpenApiRouteInfo) -> Value {
    let mut op = Map::new();
    op.insert(
        "operationId".into(),
        json!(route.handler_name.clone()),
    );
    op.insert(
        "summary".into(),
        json!(format!("Handler `{}`", route.handler_name)),
    );

    let params = build_parameters(route);
    if !params.is_empty() {
        op.insert("parameters".into(), Value::Array(params));
    }

    if let Some(body_name) = &route.body_param_name {
        let body_type = route
            .param_type_exprs
            .iter()
            .find(|(n, _)| n == body_name)
            .and_then(|(_, t)| t.as_ref());
        op.insert("requestBody".into(), build_request_body(body_type));
    }

    op.insert(
        "responses".into(),
        build_responses(&route.return_type_expr, &route.custom_status_codes),
    );
    Value::Object(op)
}

fn build_parameters(route: &OpenApiRouteInfo) -> Vec<Value> {
    let mut out = Vec::new();
    for name in &route.path_params {
        let t = lookup_param_type(route, name);
        out.push(json!({
            "name": name,
            "in": "path",
            "required": true,
            "schema": type_expr_to_schema_or_any(t),
        }));
    }
    for name in &route.query_params {
        let t = lookup_param_type(route, name);
        // Query params nullables (Int?) son opcionales; el resto
        // obligatorios. Sin anotación → required = true (el handler
        // espera el valor).
        let required = !t.map(|x| x.is_nullable()).unwrap_or(false);
        let schema = type_expr_to_schema_or_any(t);
        out.push(json!({
            "name": name,
            "in": "query",
            "required": required,
            "schema": schema,
        }));
    }
    // Fase 7.6: headers como parameters con in: "header". El name es
    // el HTTP name canónico (lo que el cliente debe mandar); el
    // schema siempre es `string` (HTTP headers son strings; los tipos
    // ricos son deuda explícita).
    for (http_name, _fitz_name, is_nullable) in &route.header_params {
        out.push(json!({
            "name": http_name,
            "in": "header",
            "required": !is_nullable,
            "schema": { "type": "string" },
        }));
    }
    out
}

fn lookup_param_type<'a>(route: &'a OpenApiRouteInfo, name: &str) -> Option<&'a TypeExpr> {
    route
        .param_type_exprs
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, t)| t.as_ref())
}

fn build_request_body(t: Option<&TypeExpr>) -> Value {
    json!({
        "required": true,
        "content": {
            "application/json": {
                "schema": type_expr_to_schema_or_any(t),
            },
        },
    })
}

fn build_responses(return_type: &Option<TypeExpr>, custom_status_codes: &[u16]) -> Value {
    let mut resp: Map<String, Value> = Map::new();
    match return_type {
        Some(TypeExpr::Generic { name, args }) if name == "Result" && args.len() == 1 => {
            resp.insert("200".into(), success_response(Some(&args[0])));
            resp.insert("500".into(), error_response());
        }
        Some(other) => {
            resp.insert("200".into(), success_response(Some(other)));
        }
        None => {
            resp.insert("200".into(), success_response(None));
        }
    }
    // Q.4: sumar entries por cada status code custom detectado en el
    // body. El body de un ReturnStatus es polimórfico (un handler
    // `-> User` puede mandar `return 404 { "error": "..." }` con un
    // shape distinto), así que el schema de cada response custom queda
    // como "any" (`{}`). Si ya hay un entry con el mismo status (caso
    // raro: `return 200 { ... }`), gana el del return type (no se
    // sobreescribe). Los codes vienen ordenados ascendente y deduplicados.
    for code in custom_status_codes {
        let key = code.to_string();
        if resp.contains_key(&key) {
            continue;
        }
        resp.insert(key, custom_status_response(*code));
    }
    Value::Object(resp)
}

fn custom_status_response(code: u16) -> Value {
    json!({
        "description": http_status_phrase(code),
        "content": {
            "application/json": {
                // Body polimórfico: no fijamos schema. El usuario lo
                // describe en docs externas si lo necesita.
                "schema": {},
            },
        },
    })
}

/// Mapeo mínimo de status code → reason phrase para la `description`
/// del schema. Cubre los codes comunes; los demás caen a "Response".
/// El schema sigue siendo válido sin importar el texto.
fn http_status_phrase(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Response",
    }
}

fn success_response(t: Option<&TypeExpr>) -> Value {
    json!({
        "description": "OK",
        "content": {
            "application/json": {
                "schema": type_expr_to_schema_or_any(t),
            },
        },
    })
}

fn error_response() -> Value {
    json!({
        "description": "Internal error",
        "content": {
            "application/json": {
                "schema": {
                    "type": "object",
                    "properties": {
                        "error": { "type": "string" },
                    },
                    "required": ["error"],
                },
            },
        },
    })
}

// ---------- TypeExpr → JSON Schema ----------

fn type_expr_to_schema_or_any(t: Option<&TypeExpr>) -> Value {
    match t {
        Some(t) => type_expr_to_schema(t),
        // Sin anotación: schema vacío = "cualquier valor JSON".
        None => json!({}),
    }
}

/// Traduce un `TypeExpr` a un schema JSON Schema 2020-12 (subset).
/// Mapping:
///   - `Int`           → `{"type":"integer","format":"int64"}`
///   - `Float`         → `{"type":"number"}`
///   - `Str`           → `{"type":"string"}`
///   - `Bool`          → `{"type":"boolean"}`
///   - `Null`          → `{"type":"null"}`
///   - `T?`            → schema de T + `"nullable": true`
///   - `List<T>`       → `{"type":"array","items":<T>}`
///   - `Map<Str, V>`   → `{"type":"object","additionalProperties":<V>}`
///   - `Map<K, V>` con K ≠ Str → object con description (no es
///     serializable como JSON object con claves ≠ Str).
///   - `Result<T>`     → schema de T (en posición de valor; en return
///     se procesa especial en `build_responses`).
///   - `Foo` (nominal) → `{"$ref":"#/components/schemas/Foo"}`.
///   - `Fn(...) -> R`  → description (no serializable).
pub fn type_expr_to_schema(t: &TypeExpr) -> Value {
    match t {
        TypeExpr::Named(name) => named_to_schema(name),
        TypeExpr::Generic { name, args } => generic_to_schema(name, args),
        TypeExpr::Nullable(inner) => {
            let mut s = type_expr_to_schema(inner);
            if let Some(obj) = s.as_object_mut() {
                obj.insert("nullable".into(), json!(true));
            }
            s
        }
        TypeExpr::Function { .. } => json!({
            "description": format!("{} (función Fitz, no serializable)", t.display_name()),
        }),
        // Tuples (mini-tanda T): JSON no tiene tuples, serializamos
        // como array prefix-typed. Schema OpenAPI 3.1 admite
        // `prefixItems` para esto.
        TypeExpr::Tuple(items) => {
            let schemas: Vec<Value> = items.iter().map(type_expr_to_schema).collect();
            json!({
                "type": "array",
                "prefixItems": schemas,
                "minItems": items.len(),
                "maxItems": items.len(),
            })
        }
    }
}

fn named_to_schema(name: &str) -> Value {
    match name {
        "Int" => json!({ "type": "integer", "format": "int64" }),
        "Float" => json!({ "type": "number" }),
        "Str" => json!({ "type": "string" }),
        "Bool" => json!({ "type": "boolean" }),
        "Null" => json!({ "type": "null" }),
        // Nominal: ref a components.schemas. Si el tipo no fue declarado
        // en este programa, la ref queda dangling — el ajuste lo hace
        // la herramienta consumidora del schema (Scalar, generador de
        // SDKs). No abortamos por eso para no acoplar el generator al
        // checker.
        _ => json!({ "$ref": format!("#/components/schemas/{}", name) }),
    }
}

fn generic_to_schema(name: &str, args: &[TypeExpr]) -> Value {
    match name {
        "List" if args.len() == 1 => json!({
            "type": "array",
            "items": type_expr_to_schema(&args[0]),
        }),
        "Map" if args.len() == 2 => {
            if let TypeExpr::Named(k) = &args[0] {
                if k == "Str" {
                    return json!({
                        "type": "object",
                        "additionalProperties": type_expr_to_schema(&args[1]),
                    });
                }
            }
            json!({
                "type": "object",
                "description": format!(
                    "Map<{}, {}> (claves no-Str no son serializables como objeto JSON)",
                    args[0].display_name(),
                    args[1].display_name()
                ),
            })
        }
        "Result" if args.len() == 1 => type_expr_to_schema(&args[0]),
        _ => json!({ "type": "object" }),
    }
}

// ---------- components.schemas ----------

fn build_components_schemas(program: &Program) -> Value {
    let mut schemas: Map<String, Value> = Map::new();
    for stmt in program {
        if let Stmt::TypeDef { name, fields, .. } = stmt {
            schemas.insert(name.clone(), type_def_to_schema(fields));
        }
    }
    Value::Object(schemas)
}

fn type_def_to_schema(fields: &[Field]) -> Value {
    let mut properties: Map<String, Value> = Map::new();
    let mut required: Vec<Value> = Vec::new();
    for field in fields {
        properties.insert(field.name.clone(), type_expr_to_schema(&field.type_));
        // "required" en JSON Schema = no nullable y sin default.
        // Si el campo tiene default → el server lo completa cuando falta.
        // Si es nullable → puede ser explícito `null`.
        let is_required = !field.type_.is_nullable() && field.default.is_none();
        if is_required {
            required.push(json!(field.name.clone()));
        }
    }
    let mut schema = json!({
        "type": "object",
        "properties": properties,
    });
    if !required.is_empty() {
        schema
            .as_object_mut()
            .expect("schema inicializado como object literal")
            .insert("required".into(), Value::Array(required));
    }
    schema
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TypeExpr;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    // Helpers de construcción.
    fn named(s: &str) -> TypeExpr {
        TypeExpr::Named(s.into())
    }
    fn nullable(t: TypeExpr) -> TypeExpr {
        TypeExpr::Nullable(Box::new(t))
    }
    fn generic(name: &str, args: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr::Generic {
            name: name.into(),
            args,
        }
    }

    // -------- type_expr_to_schema --------

    #[test]
    fn schema_de_int() {
        let s = type_expr_to_schema(&named("Int"));
        assert_eq!(s, json!({ "type": "integer", "format": "int64" }));
    }

    #[test]
    fn schema_de_float_str_bool_null() {
        assert_eq!(
            type_expr_to_schema(&named("Float")),
            json!({ "type": "number" })
        );
        assert_eq!(
            type_expr_to_schema(&named("Str")),
            json!({ "type": "string" })
        );
        assert_eq!(
            type_expr_to_schema(&named("Bool")),
            json!({ "type": "boolean" })
        );
        assert_eq!(
            type_expr_to_schema(&named("Null")),
            json!({ "type": "null" })
        );
    }

    #[test]
    fn schema_de_nullable_agrega_flag_nullable() {
        let s = type_expr_to_schema(&nullable(named("Str")));
        assert_eq!(
            s,
            json!({ "type": "string", "nullable": true })
        );
    }

    #[test]
    fn schema_de_list_es_array_con_items() {
        let s = type_expr_to_schema(&generic("List", vec![named("Int")]));
        assert_eq!(
            s,
            json!({
                "type": "array",
                "items": { "type": "integer", "format": "int64" }
            })
        );
    }

    #[test]
    fn schema_de_map_str_es_object_con_additional_properties() {
        let s = type_expr_to_schema(&generic("Map", vec![named("Str"), named("Int")]));
        assert_eq!(
            s,
            json!({
                "type": "object",
                "additionalProperties": { "type": "integer", "format": "int64" }
            })
        );
    }

    #[test]
    fn schema_de_map_no_str_lleva_description() {
        let s = type_expr_to_schema(&generic("Map", vec![named("Int"), named("Str")]));
        let obj = s.as_object().unwrap();
        assert_eq!(obj.get("type"), Some(&json!("object")));
        // El description explica que las claves no son Str.
        let desc = obj.get("description").unwrap().as_str().unwrap();
        assert!(desc.contains("Map<Int, Str>"), "description fue: {}", desc);
    }

    #[test]
    fn schema_de_nominal_es_ref_a_components_schemas() {
        let s = type_expr_to_schema(&named("User"));
        assert_eq!(s, json!({ "$ref": "#/components/schemas/User" }));
    }

    #[test]
    fn schema_de_result_en_posicion_de_valor_es_el_inner() {
        // En posición de valor (no return), Result<T> se aplana al inner T.
        let s = type_expr_to_schema(&generic("Result", vec![named("Int")]));
        assert_eq!(s, json!({ "type": "integer", "format": "int64" }));
    }

    #[test]
    fn schema_de_list_de_nominales_anida_ref() {
        let s = type_expr_to_schema(&generic("List", vec![named("User")]));
        assert_eq!(
            s,
            json!({
                "type": "array",
                "items": { "$ref": "#/components/schemas/User" }
            })
        );
    }

    // -------- build_responses --------

    #[test]
    fn responses_sin_return_type_solo_emite_200_any() {
        let r = build_responses(&None, &[]);
        let obj = r.as_object().unwrap();
        assert!(obj.contains_key("200"));
        assert!(!obj.contains_key("500"));
        // Schema vacío (any).
        let schema = obj["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema, json!({}));
    }

    #[test]
    fn responses_con_return_type_concreto_emite_solo_200() {
        let r = build_responses(&Some(named("Int")), &[]);
        let obj = r.as_object().unwrap();
        assert!(obj.contains_key("200"));
        assert!(!obj.contains_key("500"));
        let schema = obj["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema, json!({ "type": "integer", "format": "int64" }));
    }

    // ---- Q.4: status codes custom en schema ----

    #[test]
    fn responses_suma_entries_por_status_codes_custom() {
        let r = build_responses(&Some(named("Str")), &[401, 404]);
        let obj = r.as_object().unwrap();
        // 200 sigue (del return type Str).
        assert!(obj.contains_key("200"));
        // 401 y 404 sumados con schema vacío.
        assert!(obj.contains_key("401"));
        assert!(obj.contains_key("404"));
        assert_eq!(
            obj["401"]["content"]["application/json"]["schema"],
            json!({})
        );
        // Description usa la reason phrase HTTP.
        assert_eq!(obj["401"]["description"], json!("Unauthorized"));
        assert_eq!(obj["404"]["description"], json!("Not Found"));
    }

    #[test]
    fn responses_status_custom_no_pisa_200_existente() {
        // Si un handler hace `return 200 { ... }` y además tiene
        // return type `Str`, el entry 200 del return type gana —
        // mantenemos el schema fuerte sobre el polimórfico.
        let r = build_responses(&Some(named("Str")), &[200]);
        let schema = r["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema, json!({ "type": "string" }));
    }

    #[test]
    fn responses_status_custom_no_pisa_500_de_result() {
        // Result<T> genera 200+500. Un `return 500 { ... }` custom no
        // debe duplicarlos.
        let r = build_responses(&Some(generic("Result", vec![named("Int")])), &[500]);
        let obj = r.as_object().unwrap();
        // El 500 sigue siendo el "error" del Result, no el custom any.
        let schema = obj["500"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema["type"], json!("object"));
    }

    #[test]
    fn responses_status_custom_desconocido_usa_response_phrase_default() {
        let r = build_responses(&None, &[418]);
        assert_eq!(r["418"]["description"], json!("Response"));
    }

    #[test]
    fn collect_status_codes_simple_extraccion_y_orden() {
        use crate::ast::Span;
        // body: `return 404 { ... }; return 401 { ... }; return 404 { ... }`
        let body = vec![
            crate::ast::Stmt::ReturnStatus {
                status: crate::ast::Expr::Int(404, Span::ZERO),
                body: None,
                span: Span::ZERO,
            },
            crate::ast::Stmt::ReturnStatus {
                status: crate::ast::Expr::Int(401, Span::ZERO),
                body: None,
                span: Span::ZERO,
            },
            crate::ast::Stmt::ReturnStatus {
                status: crate::ast::Expr::Int(404, Span::ZERO),
                body: None,
                span: Span::ZERO,
            },
        ];
        // Ordenado ascendente + dedup.
        assert_eq!(collect_status_codes(&body), vec![401u16, 404u16]);
    }

    #[test]
    fn collect_status_codes_status_no_literal_se_omite() {
        use crate::ast::Span;
        // `return <ident> { ... }` no es inferible — se skipea.
        let body = vec![crate::ast::Stmt::ReturnStatus {
            status: crate::ast::Expr::Ident("code".into(), Span::ZERO),
            body: None,
            span: Span::ZERO,
        }];
        assert!(collect_status_codes(&body).is_empty());
    }

    #[test]
    fn collect_status_codes_status_fuera_de_rango_se_omite() {
        use crate::ast::Span;
        // 1000 no es un status HTTP válido → skipear (parser/runtime
        // lo cazarían pero el schema no debería emitir códigos que no
        // pueden aparecer).
        let body = vec![crate::ast::Stmt::ReturnStatus {
            status: crate::ast::Expr::Int(1000, Span::ZERO),
            body: None,
            span: Span::ZERO,
        }];
        assert!(collect_status_codes(&body).is_empty());
    }

    #[test]
    fn schema_para_handler_con_returnstatus_emite_codes() {
        let src = "\
            @get(\"/p\")\n\
            fn protected() -> Str {\n\
                return 401 {\"msg\": \"no autorizado\"}\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = &schema["paths"]["/p"]["get"]["responses"];
        assert!(responses.get("200").is_some()); // del return type Str
        assert!(responses.get("401").is_some()); // del ReturnStatus custom
        assert_eq!(responses["401"]["description"], json!("Unauthorized"));
    }

    #[test]
    fn schema_codes_dentro_de_if_else_se_detectan() {
        // `Stmt::ReturnStatus` adentro de un `if`/`else` se detecta
        // recursivamente. El walker baja por el branch then/else.
        let src = "\
            @get(\"/u/{id}\")\n\
            fn h(id: Int) -> Str {\n\
                if (id == 0) {\n\
                    return 404 {\"msg\": \"no encontrado\"}\n\
                }\n\
                return \"ok\"\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = &schema["paths"]["/u/{id}"]["get"]["responses"];
        assert!(responses.get("404").is_some());
    }

    #[test]
    fn responses_con_result_emite_200_y_500() {
        let r = build_responses(&Some(generic("Result", vec![named("User")])), &[]);
        let obj = r.as_object().unwrap();
        assert!(obj.contains_key("200"));
        assert!(obj.contains_key("500"));
        // 200 lleva el inner.
        let ok_schema = obj["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(ok_schema, json!({ "$ref": "#/components/schemas/User" }));
        // 500 lleva `{error: string}`.
        let err_schema = obj["500"]["content"]["application/json"]["schema"].clone();
        assert_eq!(err_schema["type"], json!("object"));
        assert_eq!(err_schema["required"], json!(["error"]));
    }

    // -------- type_def_to_schema --------

    #[test]
    fn type_def_emite_object_con_properties_y_required() {
        let fields = vec![
            Field {
                name: "id".into(),
                type_: named("Int"),
                default: None,
            },
            Field {
                name: "name".into(),
                type_: named("Str"),
                default: None,
            },
        ];
        let s = type_def_to_schema(&fields);
        assert_eq!(s["type"], json!("object"));
        assert!(s["properties"]["id"].is_object());
        assert!(s["properties"]["name"].is_object());
        // Required incluye ambos (sin default y no nullable).
        let req = s["required"].as_array().unwrap();
        assert!(req.contains(&json!("id")));
        assert!(req.contains(&json!("name")));
    }

    #[test]
    fn type_def_excluye_de_required_los_nullables_y_con_default() {
        let fields = vec![
            Field {
                name: "id".into(),
                type_: named("Int"),
                default: None,
            },
            // Nullable: opcional, no aparece en required.
            Field {
                name: "nickname".into(),
                type_: nullable(named("Str")),
                default: None,
            },
            // Con default: opcional, no aparece en required.
            Field {
                name: "active".into(),
                type_: named("Bool"),
                default: Some(crate::ast::Expr::Bool(true, crate::ast::Span::ZERO)),
            },
        ];
        let s = type_def_to_schema(&fields);
        let req = s["required"].as_array().unwrap();
        assert_eq!(req.len(), 1);
        assert_eq!(req[0], json!("id"));
    }

    // -------- generate_openapi (integradores) --------

    /// Helper: parsea + evalúa el src adentro de un registry activo, y
    /// devuelve el schema. Útil para tests de extremo a extremo del
    /// generator que verifican el cableado completo (TypeExpr → RouteSpec
    /// → schema).
    fn schema_for(src: &str) -> Value {
        let program = parse(tokenize(src).expect("lex OK")).expect("parse OK");
        let (res, registry) = crate::http::with_active_registry(|| {
            crate::evaluator::eval_with_base_sync(program.clone(), std::env::current_dir().unwrap())
        });
        res.expect("eval OK");
        // Q.2: replica el cableado real de main.rs / http.rs — si el
        // programa declara `@server(api_version=...)`, el schema lo
        // refleja. Sin el override, default "0.1.0".
        let api_version = registry
            .server_config
            .as_ref()
            .and_then(|c| c.api_version.clone());
        generate_openapi_with_version(
            &routes_from_registry(&registry),
            &program,
            api_version.as_deref(),
        )
    }

    #[test]
    fn generador_emite_estructura_top_level_openapi_3_1() {
        let src = "@get(\"/\")\nfn root() => \"hola\"";
        let schema = schema_for(src);
        assert_eq!(schema["openapi"], json!("3.1.0"));
        assert_eq!(schema["info"]["title"], json!("Fitz API"));
        assert_eq!(schema["info"]["version"], json!("0.1.0"));
        assert!(schema["paths"].is_object());
        assert!(schema["components"]["schemas"].is_object());
    }

    // ---- Q.2: @server(api_version="X.Y.Z") ----

    #[test]
    fn api_version_override_se_refleja_en_info_version() {
        let src = "\
            @server(api_version=\"1.2.3\")\n\
            fn main() => 0\n\
            @get(\"/\")\n\
            fn root() => \"hola\"\n\
        ";
        let schema = schema_for(src);
        assert_eq!(schema["info"]["version"], json!("1.2.3"));
    }

    #[test]
    fn sin_api_version_kwarg_default_sigue_siendo_0_1_0() {
        let src = "\
            @server(3000)\n\
            fn main() => 0\n\
            @get(\"/\")\n\
            fn root() => \"hola\"\n\
        ";
        let schema = schema_for(src);
        assert_eq!(schema["info"]["version"], json!("0.1.0"));
    }

    #[test]
    fn generate_openapi_with_version_some_y_none() {
        // Test directo del generador: con override y sin override.
        use crate::ast::Stmt;
        let program: Vec<Stmt> = vec![];
        let s1 = generate_openapi_with_version(&[], &program, Some("9.9.9"));
        assert_eq!(s1["info"]["version"], json!("9.9.9"));
        let s2 = generate_openapi_with_version(&[], &program, None);
        assert_eq!(s2["info"]["version"], json!("0.1.0"));
    }

    #[test]
    fn ruta_get_simple_aparece_en_paths_con_operation_id() {
        let src = "@get(\"/health\")\nfn ping() => \"ok\"";
        let schema = schema_for(src);
        let get = &schema["paths"]["/health"]["get"];
        assert_eq!(get["operationId"], json!("ping"));
        assert!(get["responses"]["200"].is_object());
    }

    #[test]
    fn ruta_con_path_param_emite_parameter_in_path() {
        let src = "@get(\"/users/{id}\")\nfn get_user(id: Int) => id";
        let schema = schema_for(src);
        let params = schema["paths"]["/users/{id}"]["get"]["parameters"]
            .as_array()
            .unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], json!("id"));
        assert_eq!(params[0]["in"], json!("path"));
        assert_eq!(params[0]["required"], json!(true));
        assert_eq!(
            params[0]["schema"],
            json!({ "type": "integer", "format": "int64" })
        );
    }

    #[test]
    fn ruta_con_query_param_nullable_es_no_requerido() {
        let src = "@get(\"/search?limit={limit}\")\nfn search(limit: Int?) => limit";
        let schema = schema_for(src);
        let params = schema["paths"]["/search"]["get"]["parameters"]
            .as_array()
            .unwrap();
        assert_eq!(params[0]["name"], json!("limit"));
        assert_eq!(params[0]["in"], json!("query"));
        assert_eq!(params[0]["required"], json!(false));
        // El schema lleva nullable: true.
        assert_eq!(params[0]["schema"]["nullable"], json!(true));
    }

    #[test]
    fn ruta_post_con_body_de_type_custom_emite_request_body_y_ref() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let schema = schema_for(src);
        let post = &schema["paths"]["/users"]["post"];
        assert_eq!(post["requestBody"]["required"], json!(true));
        let body_schema = &post["requestBody"]["content"]["application/json"]["schema"];
        assert_eq!(body_schema, &json!({ "$ref": "#/components/schemas/UserInput" }));
        // Y `UserInput` está en components.schemas.
        let user_input_schema = &schema["components"]["schemas"]["UserInput"];
        assert_eq!(user_input_schema["type"], json!("object"));
        assert!(user_input_schema["properties"]["name"].is_object());
    }

    #[test]
    fn ruta_con_header_obligatorio_aparece_en_parameters() {
        let src = "@header(name=\"Authorization\")\n@get(\"/protected\")\nfn protected(authorization: Str) -> Str => authorization";
        let schema = schema_for(src);
        let params = schema["paths"]["/protected"]["get"]["parameters"]
            .as_array()
            .unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], json!("Authorization"));
        assert_eq!(params[0]["in"], json!("header"));
        assert_eq!(params[0]["required"], json!(true));
        assert_eq!(params[0]["schema"], json!({ "type": "string" }));
    }

    #[test]
    fn ruta_con_header_nullable_es_no_requerido() {
        let src = "@header(name=\"X-Trace-Id\")\n@get(\"/traced\")\nfn traced(x_trace_id: Str?) -> Str => \"ok\"";
        let schema = schema_for(src);
        let params = schema["paths"]["/traced"]["get"]["parameters"]
            .as_array()
            .unwrap();
        assert_eq!(params[0]["name"], json!("X-Trace-Id"));
        assert_eq!(params[0]["in"], json!("header"));
        assert_eq!(params[0]["required"], json!(false));
    }

    #[test]
    fn return_result_user_emite_200_user_y_500_error() {
        let src = "\
            type User { id: Int, name: Str }\n\
            @get(\"/users/{id}\")\nfn get_user(id: Int) -> Result<User> => Ok(User { id: id, name: \"x\" })\n\
        ";
        let schema = schema_for(src);
        let get = &schema["paths"]["/users/{id}"]["get"];
        let responses = get["responses"].as_object().unwrap();
        assert!(responses.contains_key("200"));
        assert!(responses.contains_key("500"));
        let ok_schema = &responses["200"]["content"]["application/json"]["schema"];
        assert_eq!(ok_schema, &json!({ "$ref": "#/components/schemas/User" }));
    }
}
