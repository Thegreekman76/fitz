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
    /// Fase 9.w.1.e — política de auth de la ruta.
    ///
    /// `AuthSpec::None` (default) — handler público, sin `security` en
    /// el operation. `Authenticated`/`Admin` — emite el security
    /// requirement con bearerAuth y suma 401 (`Admin` también suma 403)
    /// a las `responses`. El scheme top-level
    /// `components.securitySchemes.bearerAuth` se emite cuando al menos
    /// un route tiene `auth != None`.
    pub auth: crate::http::AuthSpec,
}

/// Adapter: del registry runtime a vistas livianas.
///
/// Mini-tanda OAPI — recibe el `&Program` para extraer constantes
/// top-level Int (`let NOT_FOUND = 404`) y resolver Idents en los
/// status codes de Err/ReturnStatus. Cero overhead cuando no hay
/// consts (tabla vacía).
pub fn routes_from_registry(reg: &HttpRegistry, program: &Program) -> Vec<OpenApiRouteInfo> {
    let consts = collect_top_level_int_consts(program);
    reg.routes
        .iter()
        .map(|s| route_info_from_spec(s, &consts))
        .collect()
}

fn route_info_from_spec(
    s: &RouteSpec,
    consts: &std::collections::HashMap<String, i64>,
) -> OpenApiRouteInfo {
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
        // OAPI: usar la tabla de consts para resolver Idents.
        custom_status_codes: match &s.handler {
            crate::value::Value::Function { body, .. } => {
                collect_status_codes_with_consts(body, consts)
            }
            _ => Vec::new(),
        },
        // Fase 9.w.1.e — propagar la política de auth de la ruta.
        auth: s.auth,
    }
}

/// Mini-fase Q.4: recorre un body de fn y devuelve los `Stmt::ReturnStatus`
/// con status literal Int encontrados. Status no literales (variables,
/// expresiones) se omiten — no podemos saberlos estáticamente. Recurse
/// adentro de loops, if/match, etc.; FnExpr inline NO se sigue (otro
/// scope, otra fn). El Vec devuelto está deduplicado y en orden
/// ascendente para que el schema sea determinista.
///
/// Mini-tanda OAPI — wrapper que delega a la versión con tabla de
/// constantes vacía (back-compat con tests y `routes_from_registry`
/// del path runtime, donde no hay AST top-level disponible).
pub fn collect_status_codes(body: &[crate::ast::Stmt]) -> Vec<u16> {
    let empty = std::collections::HashMap::new();
    collect_status_codes_with_consts(body, &empty)
}

/// Mini-tanda OAPI — variante que acepta una tabla `const_name → Int`
/// con las constantes top-level del programa (`let NOT_FOUND = 404`).
/// Cuando el `status` field de un `Err(StructLit { ... })` o el status
/// de un `Stmt::ReturnStatus` es un `Expr::Ident` cuyo nombre matchea
/// una entrada de la tabla, se resuelve al valor literal y se incluye
/// en el schema. Idents que no resuelven (vars locales, expresiones
/// dinámicas) se siguen omitiendo silenciosamente como antes.
pub fn collect_status_codes_with_consts(
    body: &[crate::ast::Stmt],
    consts: &std::collections::HashMap<String, i64>,
) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for s in body {
        collect_status_codes_stmt(s, &mut out, consts);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Mini-tanda OAPI — pre-scan del programa para extraer top-level
/// `let X = <Int literal>` (incluyendo `Expr::UnaryOp::Neg` envolviendo
/// un Int para casos negativos como `let TIMEOUT = -1` — irrelevante
/// para status codes pero coherente). Devuelve una `HashMap<nombre,
/// valor>` lista para pasar a `collect_status_codes_with_consts`. Vars
/// con RHS no literal o tipo distinto se omiten silenciosamente. Solo
/// scope top-level (no const inside fn / inside type).
pub fn collect_top_level_int_consts(
    program: &crate::ast::Program,
) -> std::collections::HashMap<String, i64> {
    use crate::ast::Stmt;
    let mut out = std::collections::HashMap::new();
    for s in program {
        if let Stmt::Assign { target, value, .. } = s {
            // Solo bindings simples `let X = ...` (no field assign).
            let crate::ast::AssignTarget::Ident(name) = target else {
                continue;
            };
            // OAPI-Expr — usa `resolve_status_value` que ahora acepta
            // Int literal, Ident a const previa, UnaryOp::Neg y BinOp
            // simple (Add/Sub/Mul). Walk en orden de declaración para
            // que `let Y = X + 4` resuelva X cuando llega Y.
            let resolved = resolve_status_value(value, &out);
            if let Some(n) = resolved {
                out.insert(name.clone(), n);
            }
        }
    }
    out
}

fn collect_status_codes_stmt(
    stmt: &crate::ast::Stmt,
    out: &mut Vec<u16>,
    consts: &std::collections::HashMap<String, i64>,
) {
    use crate::ast::Stmt;
    match stmt {
        Stmt::ReturnStatus { status, body, .. } => {
            if let Some(n) = resolve_status_value(status, consts) {
                // Status fuera de rango HTTP válido (100-599) lo
                // skipeamos también — el runtime/parser lo cazaría.
                if (100..=599).contains(&n) {
                    out.push(n as u16);
                }
            }
            // El body puede contener otro ReturnStatus anidado vía
            // if/match — recorremos.
            if let Some(b) = body {
                collect_status_codes_expr(b, out, consts);
            }
        }
        Stmt::While { body, .. } | Stmt::Loop { body, .. } | Stmt::For { body, .. } => {
            for s in body {
                collect_status_codes_stmt(s, out, consts);
            }
        }
        Stmt::Assign { value, .. } => collect_status_codes_expr(value, out, consts),
        Stmt::Return(e, _) | Stmt::Expr(e, _) => collect_status_codes_expr(e, out, consts),
        _ => {}
    }
}

/// Mini-tanda OAPI + OAPI-Expr — resuelve un value de `status:` o
/// `Stmt::ReturnStatus.status` a un Int. Acepta:
/// - `Expr::Int(n)` literal directo.
/// - `Expr::Ident(name)` con lookup en la tabla de consts top-level.
/// - `Expr::UnaryOp::Neg` sobre cualquiera de los anteriores.
/// - `Expr::BinOp` con Add/Sub/Mul aritmético simple sobre los
///   anteriores (const-eval). Permite patrones como
///   `status: BASE + 1` o `status: -CODE`.
///
/// Cualquier otra cosa (Div con 0, llamadas, vars locales, etc.)
/// devuelve None — el schema cae al 500 default.
fn resolve_status_value(
    e: &crate::ast::Expr,
    consts: &std::collections::HashMap<String, i64>,
) -> Option<i64> {
    use crate::ast::{BinOpKind, Expr, UnaryOpKind};
    match e {
        Expr::Int(n, _) => Some(*n),
        Expr::Ident(name, _) => consts.get(name).copied(),
        Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand,
            ..
        } => resolve_status_value(operand, consts).map(|n| -n),
        Expr::BinOp {
            op, left, right, ..
        } => {
            let l = resolve_status_value(left, consts)?;
            let r = resolve_status_value(right, consts)?;
            match op {
                BinOpKind::Add => l.checked_add(r),
                BinOpKind::Sub => l.checked_sub(r),
                BinOpKind::Mul => l.checked_mul(r),
                // Div/Mod evitamos por simplicidad (división por 0).
                _ => None,
            }
        }
        _ => None,
    }
}

fn collect_status_codes_expr(
    expr: &crate::ast::Expr,
    out: &mut Vec<u16>,
    consts: &std::collections::HashMap<String, i64>,
) {
    use crate::ast::Expr;
    match expr {
        Expr::If { then, else_, .. } => {
            for s in then {
                collect_status_codes_stmt(s, out, consts);
            }
            if let Some(els) = else_ {
                for s in els {
                    collect_status_codes_stmt(s, out, consts);
                }
            }
        }
        Expr::Match { arms, .. } => {
            for a in arms {
                for s in &a.body {
                    collect_status_codes_stmt(s, out, consts);
                }
            }
        }
        // Mini-tanda HC.2 — detectar `Err(StructLit { status: <Int
        // literal>, ... })` y registrar el status code en el schema.
        // El patrón canónico es `return Err(ApiErr { status: 404, ... })`
        // donde el tipo E del Result tiene un field `status: Int`. El
        // status code se infiere del literal en cada call site.
        //
        // Mini-tanda OAPI — además del Int literal, ahora aceptamos
        // referencias a constantes top-level (`let NOT_FOUND = 404`).
        // El patrón `Err(ApiErr { status: NOT_FOUND, ... })` se
        // resuelve a 404 vía la tabla `consts`. Vars locales o
        // expresiones complejas siguen omitidas (caen al 500 default).
        Expr::Err(inner, _) => {
            if let Expr::StructLit { fields, .. } = inner.as_ref() {
                for (name, val) in fields {
                    if name == "status" {
                        if let Some(n) = resolve_status_value(val, consts) {
                            if (100..=599).contains(&n) {
                                out.push(n as u16);
                            }
                        }
                        break;
                    }
                }
            }
        }
        // Recursión simétrica para que `Ok`/`Err` que envuelven otros
        // exprs no oculten ReturnStatus anidado (no es el caso típico,
        // pero la cobertura cuesta poco).
        Expr::Ok(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
            collect_status_codes_expr(inner, out, consts);
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
    pseudo_routes_from_program_and_modules(program, &[])
}

/// 10.8.5 (v0.10.8) — variante cross-module aware de
/// `pseudo_routes_from_ast`. Combina los handlers del `program`
/// (main) y los `module_http_stmts` (slices preservados por W16 —
/// `LoadedModule.http_fn_stmts`) en un solo Vec antes de extraer.
/// Resultado: el schema OpenAPI 3.1 emitido contiene TODOS los
/// endpoints HTTP, incluyendo los de módulos importados.
///
/// Antes del fix #3 v0.10.8, `pseudo_routes_from_ast` solo miraba
/// el main → schema vacío (`paths: []`) cuando los handlers HTTP
/// vivían cross-module. W16 ya enchufaba las rutas al Router, pero
/// la documentación OpenAPI auto no se actualizaba.
pub fn pseudo_routes_from_program_and_modules(
    program: &crate::ast::Program,
    module_http_stmts: &[&[crate::ast::Stmt]],
) -> Result<Vec<OpenApiRouteInfo>, crate::error::FitzError> {
    use crate::ast::Stmt;
    use crate::http::parse_path_template;

    // Mini-tanda OAPI — pre-scan de constantes top-level Int para
    // resolver `status: NOT_FOUND` adentro de Err({...}) y
    // ReturnStatus dinámicos. Tabla vacía si el programa no tiene
    // consts (caso típico) — cero overhead.
    let consts = collect_top_level_int_consts(program);

    // Concatenar todos los stmts: main primero, después los
    // módulos en orden. Iteramos el slice unificado.
    let mut all_stmts: Vec<&Stmt> = program.iter().collect();
    for module_stmts in module_http_stmts {
        for s in *module_stmts {
            all_stmts.push(s);
        }
    }

    let mut out = Vec::new();
    for s in all_stmts {
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
            // Fase 9.w.1.e — recolectar política de auth del set de
            // decorators. `@admin` gana sobre `@authenticated`.
            let mut auth = crate::http::AuthSpec::None;
            for d2 in decorators {
                match d2.name.as_str() {
                    "authenticated" if auth == crate::http::AuthSpec::None => {
                        auth = crate::http::AuthSpec::Authenticated;
                    }
                    "admin" => auth = crate::http::AuthSpec::Admin,
                    _ => {}
                }
            }
            // El body_param ahora excluye el user param de auth (espejo
            // del codegen 9.w.1.d: "leftover" → auth user en lugar de body).
            let body_param_name = params.iter().find_map(|p| {
                let is_path = template.params.contains(&p.name);
                let is_query = template.query_params.contains(&p.name);
                let is_header = header_params.iter().any(|(_, fitz, _)| fitz == &p.name);
                if is_path || is_query || is_header {
                    return None;
                }
                // Si la ruta tiene auth, el primer leftover es el user
                // (NO body). Esta heurística matchea la regla del codegen.
                if auth != crate::http::AuthSpec::None {
                    return None;
                }
                Some(p.name.clone())
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
                // OAPI: resolver Idents que apunten a consts top-level.
                custom_status_codes: collect_status_codes_with_consts(body, &consts),
                auth,
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
    let mut components = Map::new();
    components.insert("schemas".into(), build_components_schemas(program));
    // Fase 9.w.1.e — security scheme. Si al menos un route tiene
    // `auth != None`, declarar `bearerAuth` en components así los
    // tooling clients (Scalar UI, Swagger UI, generadores de SDK)
    // saben emitir el lock icon + el campo de token. Bearer tokens
    // JWT son el patrón canónico que `@auth_provider` espera (header
    // `Authorization: Bearer <token>`).
    let has_auth = routes.iter().any(|r| r.auth != crate::http::AuthSpec::None);
    if has_auth {
        components.insert(
            "securitySchemes".into(),
            json!({
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                    "description": "Token JWT emitido por el `@auth_provider` del programa. Pasar como `Authorization: Bearer <token>`."
                }
            }),
        );
    }
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Fitz API",
            "version": version.unwrap_or("0.1.0"),
        },
        "paths": build_paths(routes),
        "components": Value::Object(components),
    })
}

// ---------- paths ----------

fn build_paths(routes: &[OpenApiRouteInfo]) -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for route in routes {
        let entry = paths.entry(route.path.clone()).or_insert_with(|| json!({}));
        let obj = entry
            .as_object_mut()
            .expect("entry inicializado como object literal");
        obj.insert(route.method.as_str().to_lowercase(), build_operation(route));
    }
    Value::Object(paths)
}

fn build_operation(route: &OpenApiRouteInfo) -> Value {
    let mut op = Map::new();
    op.insert("operationId".into(), json!(route.handler_name.clone()));
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

    // Fase 9.w.1.e — `security` por operation. Para handlers
    // `@authenticated`/`@admin` declara el requerimiento del bearer
    // token (referencia al scheme global `bearerAuth`). Para handlers
    // públicos NO emitimos `security` (el default OpenAPI es "ninguno"
    // — no necesitamos `security: []` explícito).
    if route.auth != crate::http::AuthSpec::None {
        op.insert("security".into(), json!([{ "bearerAuth": [] }]));
    }

    op.insert(
        "responses".into(),
        build_responses_with_auth(
            &route.return_type_expr,
            &route.custom_status_codes,
            route.auth,
        ),
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

#[cfg(test)]
fn build_responses(return_type: &Option<TypeExpr>, custom_status_codes: &[u16]) -> Value {
    build_responses_with_auth(
        return_type,
        custom_status_codes,
        crate::http::AuthSpec::None,
    )
}

/// Fase 9.w.1.e — variante de `build_responses` que también incluye
/// `401` (handlers `@authenticated`/`@admin`) y `403` (handlers
/// `@admin`) cuando aplica. Documenta al consumidor del schema que
/// el endpoint emite esos status codes — el wrapper auth los
/// produce automáticamente, no son del handler user.
fn build_responses_with_auth(
    return_type: &Option<TypeExpr>,
    custom_status_codes: &[u16],
    auth: crate::http::AuthSpec,
) -> Value {
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
    // Fase 9.w.1.e — sumar 401 (auth) y 403 (admin). El body es
    // siempre `{"error": "<msg>"}` (formato del wrapper auth, paralelo
    // a errores de validación de path params/body). Si el handler
    // también declara `@admin`, 403 sale aparte. NO sobreescribir
    // entries existentes (caso raro: handler que retorna 401 manualmente
    // via `return 401 { ... }`).
    if auth != crate::http::AuthSpec::None && !resp.contains_key("401") {
        resp.insert("401".into(), auth_error_response("Autenticación requerida"));
    }
    if auth == crate::http::AuthSpec::Admin && !resp.contains_key("403") {
        resp.insert(
            "403".into(),
            auth_error_response("Permiso denegado (admin)"),
        );
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

/// Fase 9.w.1.e — shape de las responses de 401/403 emitidas por el
/// wrapper auth. Body siempre `{"error": "<msg>"}` — espejo del
/// `serde_json::json!({"error": ...})` del runtime + codegen.
fn auth_error_response(description: &str) -> Value {
    json!({
        "description": description,
        "content": {
            "application/json": {
                "schema": {
                    "type": "object",
                    "properties": {
                        "error": { "type": "string" }
                    },
                    "required": ["error"]
                }
            }
        }
    })
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
        // 9.w.2-binary-frames — `Bytes` se mapea a `string` con
        // `format: binary` (estándar OpenAPI 3.x / AsyncAPI 3.0 para
        // raw bytes en el wire — el frame WS o el body HTTP es opaque
        // octet-stream, no JSON base64-encoded). Tools como Scalar/
        // AsyncAPI Studio lo renderean como "binary upload"/"binary
        // payload".
        "Bytes" => json!({ "type": "string", "format": "binary" }),
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
        assert_eq!(s, json!({ "type": "string", "nullable": true }));
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
    fn oapi_collect_top_level_int_consts_recolecta_lets_int() {
        // Mini-tanda OAPI + OAPI-Expr — el pre-scan detecta
        // `let X = <Int>`, `let Y = -<Int>` y BinOps simples
        // (`let SUM = 1 + 2` ahora SÍ resuelve a 3, refinamiento
        // de OAPI-Expr). Walk en orden permite referencias a consts
        // previas (`let Y = X + 4`). RHS no resoluble se omite.
        let src = "\
            let NOT_FOUND = 404\n\
            let CUSTOM = -42\n\
            let GREETING = \"hola\"\n\
            let SUM = 1 + 2\n\
            let CHAINED = NOT_FOUND + 1\n\
            @get(\"/\")\n\
            fn h() -> Int => 0\n\
        ";
        let program = parse(tokenize(src).expect("lex")).expect("parse");
        let consts = collect_top_level_int_consts(&program);
        assert_eq!(consts.get("NOT_FOUND").copied(), Some(404));
        assert_eq!(consts.get("CUSTOM").copied(), Some(-42));
        assert!(!consts.contains_key("GREETING"));
        assert_eq!(consts.get("SUM").copied(), Some(3));
        assert_eq!(consts.get("CHAINED").copied(), Some(405));
    }

    #[test]
    fn oapi_returnstatus_con_ident_a_const_top_level_aparece_en_schema() {
        // `return NOT_FOUND { ... }` donde NOT_FOUND es una const Int
        // top-level se resuelve a 404 y entra al schema.
        let src = "\
            let NOT_FOUND = 404\n\
            @get(\"/u/{id}\")\n\
            fn h(id: Int) -> Int {\n\
                return NOT_FOUND {\"error\": \"x\"}\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = &schema["paths"]["/u/{id}"]["get"]["responses"];
        assert!(
            responses.get("404").is_some(),
            "esperaba 404 en el schema, fue: {:?}",
            responses
        );
    }

    #[test]
    fn oapi_err_struct_con_status_ident_aparece_en_schema() {
        // `Err(ApiErr { status: NOT_FOUND, ... })` con NOT_FOUND const
        // top-level se resuelve.
        let src = "\
            let NOT_FOUND = 404\n\
            type ApiErr { status: Int, message: Str }\n\
            @get(\"/u/{id}\")\n\
            fn h(id: Int) -> Result<Int, ApiErr> {\n\
                if (id == 0) {\n\
                    return Err(ApiErr { status: NOT_FOUND, message: \"no\" })\n\
                }\n\
                return Ok(id)\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = &schema["paths"]["/u/{id}"]["get"]["responses"];
        assert!(
            responses.get("404").is_some(),
            "esperaba 404 en el schema, fue: {:?}",
            responses
        );
    }

    #[test]
    fn oapi_ident_no_resuelve_se_omite_silenciosamente() {
        // Si el Ident no apunta a una const top-level Int (var local,
        // fn param, etc.), se omite — schema cae al 500 default.
        let src = "\
            @get(\"/x\")\n\
            fn h(code: Int) -> Int {\n\
                return code {\"error\": \"x\"}\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = &schema["paths"]["/x"]["get"]["responses"];
        // 200 del return type, 500 default, sin codes adicionales.
        // El `return code { ... }` no resuelve estáticamente.
        let codes: Vec<&str> = responses
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert!(
            !codes
                .iter()
                .any(|c| *c == "400" || *c == "404" || *c == "401"),
            "esperaba sin codes específicos del Ident dinámico, fue: {:?}",
            codes
        );
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
                decorators: vec![],
            },
            Field {
                name: "name".into(),
                type_: named("Str"),
                default: None,
                decorators: vec![],
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
                decorators: vec![],
            },
            // Nullable: opcional, no aparece en required.
            Field {
                name: "nickname".into(),
                type_: nullable(named("Str")),
                default: None,
                decorators: vec![],
            },
            // Con default: opcional, no aparece en required.
            Field {
                name: "active".into(),
                type_: named("Bool"),
                default: Some(crate::ast::Expr::Bool(true, crate::ast::Span::ZERO)),
                decorators: vec![],
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
            &routes_from_registry(&registry, &program),
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
        assert_eq!(
            body_schema,
            &json!({ "$ref": "#/components/schemas/UserInput" })
        );
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

    // ---- Mini-tanda HC.2 — status codes de Err({ status: ... }) en schema ----

    #[test]
    fn err_con_status_field_literal_aparece_en_schema_responses() {
        let src = "\
            type User { id: Int, name: Str }\n\
            type ApiErr { status: Int, message: Str }\n\
            @get(\"/users/{id}\")\n\
            fn get_user(id: Int) -> Result<User> {\n\
                if id == 0 { return Err(ApiErr { status: 404, message: \"not found\" }) }\n\
                return Ok(User { id: id, name: \"x\" })\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = schema["paths"]["/users/{id}"]["get"]["responses"]
            .as_object()
            .unwrap();
        assert!(responses.contains_key("200"), "esperaba 200 (Ok)");
        assert!(responses.contains_key("500"), "esperaba 500 (Err fallback)");
        assert!(
            responses.contains_key("404"),
            "esperaba 404 (Err con status literal)"
        );
    }

    #[test]
    fn err_status_codes_varios_aparecen_todos_en_schema() {
        let src = "\
            type User { id: Int, name: Str }\n\
            type ApiErr { status: Int, message: Str }\n\
            @get(\"/users/{id}\")\n\
            fn get_user(id: Int) -> Result<User> {\n\
                if id == 0 { return Err(ApiErr { status: 404, message: \"not found\" }) }\n\
                if id < 0 { return Err(ApiErr { status: 400, message: \"bad\" }) }\n\
                return Ok(User { id: id, name: \"x\" })\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = schema["paths"]["/users/{id}"]["get"]["responses"]
            .as_object()
            .unwrap();
        assert!(responses.contains_key("400"));
        assert!(responses.contains_key("404"));
    }

    // ---- Fase 9.w.1.e — security scheme del OpenAPI ----

    /// Programa base reusado por los tests de auth del schema: un
    /// `@auth_provider` + 3 handlers (público, `@authenticated`, `@admin`).
    const AUTH_SCHEMA_SRC: &str = "\
type User { id: Int, name: Str, role: Str }\n\
@auth_provider\n\
fn check(headers: Map<Str, Str>) -> Result<User> {\n\
    return Err(\"sin auth\")\n\
}\n\
@get(\"/public\")\n\
fn public_route() -> Str => \"sin auth\"\n\
@authenticated\n\
@get(\"/me\")\n\
fn me(user: User) -> Str => user.name\n\
@admin\n\
@get(\"/admin\")\n\
fn admin_route(user: User) -> Str => \"hola admin\"\n\
";

    #[test]
    fn auth_schema_emite_security_schemes_bearer_auth() {
        let schema = schema_for(AUTH_SCHEMA_SRC);
        let security_schemes = schema["components"]["securitySchemes"].as_object();
        assert!(
            security_schemes.is_some(),
            "components.securitySchemes ausente — esperaba bearerAuth",
        );
        let bearer = &schema["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(bearer["type"], json!("http"));
        assert_eq!(bearer["scheme"], json!("bearer"));
        assert_eq!(bearer["bearerFormat"], json!("JWT"));
    }

    #[test]
    fn auth_schema_handler_publico_no_tiene_security() {
        let schema = schema_for(AUTH_SCHEMA_SRC);
        let op = &schema["paths"]["/public"]["get"];
        assert!(
            op.get("security").is_none(),
            "handler público debería NO tener `security`, fue: {:?}",
            op,
        );
        // Tampoco emite 401/403 (no es un caso del wrapper auth).
        let resp = op["responses"].as_object().unwrap();
        assert!(!resp.contains_key("401"));
        assert!(!resp.contains_key("403"));
    }

    #[test]
    fn auth_schema_authenticated_handler_requiere_bearer() {
        let schema = schema_for(AUTH_SCHEMA_SRC);
        let op = &schema["paths"]["/me"]["get"];
        // security: [{ bearerAuth: [] }]
        let sec = op["security"].as_array().expect("security debe ser array");
        assert_eq!(sec.len(), 1);
        assert!(
            sec[0].get("bearerAuth").is_some(),
            "primer requirement debería ser bearerAuth, fue: {:?}",
            sec[0],
        );
        // responses incluye 401 (auth) pero NO 403 (no es admin).
        let resp = op["responses"].as_object().unwrap();
        assert!(resp.contains_key("401"), "@authenticated emite 401");
        assert!(!resp.contains_key("403"), "@authenticated NO emite 403");
        // 200 del happy path debe seguir.
        assert!(resp.contains_key("200"));
    }

    #[test]
    fn auth_schema_admin_handler_emite_401_y_403() {
        let schema = schema_for(AUTH_SCHEMA_SRC);
        let op = &schema["paths"]["/admin"]["get"];
        let sec = op["security"].as_array().expect("security debe ser array");
        assert_eq!(sec.len(), 1);
        assert!(sec[0].get("bearerAuth").is_some());
        let resp = op["responses"].as_object().unwrap();
        assert!(resp.contains_key("401"), "@admin emite 401");
        assert!(resp.contains_key("403"), "@admin emite 403");
        // 401 y 403 son objetos con shape `{"error": <string>}`.
        let r401_schema = &resp["401"]["content"]["application/json"]["schema"];
        assert_eq!(r401_schema["type"], json!("object"));
        assert!(r401_schema["properties"]["error"].is_object());
    }

    #[test]
    fn auth_schema_programa_sin_auth_no_emite_security_schemes() {
        // Sin handlers de auth, components.securitySchemes debe ser
        // omitido (no emitir un objeto vacío — menos ruido en el schema).
        let src = "\
@get(\"/x\")\n\
fn x() -> Str => \"ok\"\n\
";
        let schema = schema_for(src);
        assert!(
            schema["components"].get("securitySchemes").is_none(),
            "programas sin auth NO deberían emitir securitySchemes",
        );
    }
}
