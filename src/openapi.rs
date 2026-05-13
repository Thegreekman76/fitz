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
use crate::http::{HttpRegistry, RouteSpec};

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
///   - `registry`: rutas registradas durante `eval` (con TypeExpr
///     completos por param y return desde 7.1).
///   - `program`: AST original — necesario para recorrer los
///     `Stmt::TypeDef` y emitir `components.schemas`.
///
/// Salida: un `Value` que serializado con `serde_json::to_string_pretty`
/// es un OpenAPI 3.1 válido.
pub fn generate_openapi(registry: &HttpRegistry, program: &Program) -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Fitz API",
            "version": "0.1.0",
        },
        "paths": build_paths(registry),
        "components": {
            "schemas": build_components_schemas(program),
        },
    })
}

// ---------- paths ----------

fn build_paths(registry: &HttpRegistry) -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for route in &registry.routes {
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

fn build_operation(route: &RouteSpec) -> Value {
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

    if let Some(body_param) = &route.body_param {
        let body_type = route
            .param_type_exprs
            .iter()
            .find(|(n, _)| n == &body_param.name)
            .and_then(|(_, t)| t.as_ref());
        op.insert("requestBody".into(), build_request_body(body_type));
    }

    op.insert("responses".into(), build_responses(&route.return_type_expr));
    Value::Object(op)
}

fn build_parameters(route: &RouteSpec) -> Vec<Value> {
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
    out
}

fn lookup_param_type<'a>(route: &'a RouteSpec, name: &str) -> Option<&'a TypeExpr> {
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

fn build_responses(return_type: &Option<TypeExpr>) -> Value {
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
    Value::Object(resp)
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
        let r = build_responses(&None);
        let obj = r.as_object().unwrap();
        assert!(obj.contains_key("200"));
        assert!(!obj.contains_key("500"));
        // Schema vacío (any).
        let schema = obj["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema, json!({}));
    }

    #[test]
    fn responses_con_return_type_concreto_emite_solo_200() {
        let r = build_responses(&Some(named("Int")));
        let obj = r.as_object().unwrap();
        assert!(obj.contains_key("200"));
        assert!(!obj.contains_key("500"));
        let schema = obj["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema, json!({ "type": "integer", "format": "int64" }));
    }

    #[test]
    fn responses_con_result_emite_200_y_500() {
        let r = build_responses(&Some(generic("Result", vec![named("User")])));
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
        generate_openapi(&registry, &program)
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
