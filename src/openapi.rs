// openapi.rs — Phase 7.1: OpenAPI 3.1 schema generator.
//
// Consumes the `HttpRegistry` populated during `eval` and the
// program's AST, and produces a `serde_json::Value` ready to
// serialize. The generator is stateless; it receives everything as
// parameters and returns the JSON. Invoked by:
//   - the `fitz openapi archivo.fitz` sub-command (spits the JSON to
//     stdout, useful for CI / SDK pipeline).
//   - the `/openapi.json` endpoint auto-registered in `fitz run` (7.2).
//   - the native binary's codegen (7.5) — the schema is emitted at
//     build time and embedded as `&'static str`.
//
// Design decision: we use OpenAPI 3.1 (includes full JSON Schema
// 2020-12). It's what Scalar, Postman, Insomnia, openapi-generator
// consume.
//
// Limitations accepted in 7.1 (documented in the roadmap):
//   - `info.description` and `paths.*.*.description` empty: handler
//     doc-strings are post-F7 debt (the lexer currently discards
//     comments).
//   - `info.version` fixed at "0.1.0".
//   - Custom status codes (`return 404 { ... }`): the schema only
//     emits 200 (happy case) + 500 (Err if return is Result).
//     Specific custom codes are minor debt — the info lives in
//     `Stmt::ReturnStatus` but requires analyzing the handler's body
//     to enumerate them.

use serde_json::{json, Map, Value};

use crate::ast::{Field, Program, Stmt, TypeExpr};
use crate::http::{HttpMethod, HttpRegistry, RouteSpec};

/// Lightweight view of an HTTP route — only the fields the OpenAPI
/// generator needs. Built from a runtime `RouteSpec`
/// (`routes_from_registry`) or from the AST at codegen build-time
/// (`pseudo_routes_from_ast` in codegen.rs).
///
/// Decouples the generator from the runtime `Value`: codegen doesn't
/// need to invent dummy `Value::Function`s to feed the schema.
#[derive(Debug, Clone)]
pub struct OpenApiRouteInfo {
    pub method: HttpMethod,
    pub path: String,
    pub handler_name: String,
    pub path_params: Vec<String>,
    pub query_params: Vec<String>,
    /// Name of the param the handler interprets as the body, if any.
    /// The body's type is looked up in `param_type_exprs` by name.
    pub body_param_name: Option<String>,
    /// Headers declared with `@header(name="X")` on the handler
    /// (Phase 7.6). Each entry is `(http_name, fitz_param_name,
    /// is_nullable)`. The OpenAPI schema emits them as `parameters`
    /// with `in: "header"`.
    pub header_params: Vec<(String, String, bool)>,
    pub param_type_exprs: Vec<(String, Option<TypeExpr>)>,
    pub return_type_expr: Option<TypeExpr>,
    /// Phase Q.4 mini-batch: custom status codes (`return <Int> { ... }`)
    /// detected in the handler body. Each one generates an entry in
    /// the OpenAPI schema's `responses` in addition to those derived
    /// from the return type. Ascending-ordered and deduplicated Vec
    /// for a deterministic schema. Non-literal statuses (variable,
    /// expr) are omitted — they are not statically inferable.
    pub custom_status_codes: Vec<u16>,
    /// Phase 9.w.1.e — route's auth policy.
    ///
    /// `AuthSpec::None` (default) — public handler, no `security` on
    /// the operation. `Authenticated`/`Admin` — emits the security
    /// requirement with bearerAuth and adds 401 (`Admin` also adds
    /// 403) to the `responses`. The top-level
    /// `components.securitySchemes.bearerAuth` scheme is emitted when
    /// at least one route has `auth != None`.
    pub auth: crate::http::AuthSpec,
    /// v0.19.0 Block 4 — kind of `Response` built-in content_type
    /// detected in the handler's body (when the handler returns
    /// `Response` or `Result<Response>` and the body is a literal
    /// `Response { content_type: "X", body_bytes: ... }` struct lit
    /// at the last return). Drives the schema's 200.content key
    /// (custom media type instead of "application/json") and the
    /// `format: binary` marker for binary payloads.
    ///
    /// `None` = handler does NOT return `Response` built-in (or
    /// returns one but indirectly through a helper) → schema falls
    /// back to "application/json" path (legacy behaviour).
    pub response_content_type: Option<ResponseContentTypeKind>,
}

/// v0.19.0 Block 4 — shape of the `Response` built-in detected
/// statically in the handler's body. Populated by
/// `detect_response_content_type_kind` walking the AST.
///
/// Variants:
///   - `Static { media_type, is_binary }`: the body's last return
///     is a literal `Response { content_type: "<str_literal>",
///     body_bytes: <expr|null> }` struct lit (directly or wrapped
///     in `Ok(...)` for `Result<Response>`). The schema emits
///     `200.content.<media_type>` with `format: binary` when
///     `is_binary == true`.
///   - `Dynamic`: handler returns Response built-in but the
///     content_type is NOT statically inferable (ident, call,
///     branching with different content_type per arm). The schema
///     defaults to `application/octet-stream` (catch-all "any
///     binary or text" — preserves the contract that the response
///     is NOT JSON, but cannot pin the exact media type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseContentTypeKind {
    Static { media_type: String, is_binary: bool },
    Dynamic,
}

/// Adapter: from runtime registry to lightweight views.
///
/// OAPI mini-batch — receives the `&Program` to extract top-level
/// Int constants (`let NOT_FOUND = 404`) and resolve Idents in
/// Err/ReturnStatus status codes. Zero overhead when there are no
/// consts (empty table).
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
        // Q.4: extract custom status codes from the handler's body.
        // The runtime handler is a `Value::Function { body, ... }`;
        // if for some reason it isn't (inconsistent registration), we
        // treat it as no status codes (defensive).
        // OAPI: use the const table to resolve Idents.
        custom_status_codes: match &s.handler {
            crate::value::Value::Function { body, .. } => {
                collect_status_codes_with_consts(body, consts)
            }
            _ => Vec::new(),
        },
        // Phase 9.w.1.e — propagate the route's auth policy.
        auth: s.auth,
        // v0.19.0 Block 4 — detect `Response` built-in content_type
        // from the handler's body (same walker used by the codegen
        // path `pseudo_routes_from_ast`).
        response_content_type: match &s.handler {
            crate::value::Value::Function { body, .. } => detect_response_content_type_kind(body),
            _ => None,
        },
    }
}

/// v0.19.0 Block 4 — walks a handler's body looking for a literal
/// `Response { content_type: "X", body_bytes: ... }` struct lit at
/// the last return, and returns the corresponding
/// `ResponseContentTypeKind`. Used by both runtime
/// (`routes_from_registry`) and codegen build-time
/// (`pseudo_routes_from_ast`) paths so the emitted OpenAPI schema
/// is bit-by-bit identical.
///
/// Heuristic (covers the 90% case):
///   1. Look at the body's last `Stmt::Return(expr, _)`.
///   2. Unwrap `Ok(...)` if present (Result<Response> case).
///   3. If `expr` is `Expr::StructLit("Response", fields, _)`,
///      extract `content_type` and `body_bytes` from the fields.
///   4. `content_type`:
///      - Str literal → `Static { media_type, ... }`.
///      - Not provided (default applies) → `Static { media_type:
///        "application/json", ... }` (canonical default of the
///        built-in).
///      - Other expr (Ident, Call) → `Dynamic`.
///   5. `body_bytes`:
///      - Not provided or `Null` literal → `is_binary = false`.
///      - Any other expr → `is_binary = true` (the user opted
///        into the binary path).
///   6. Anything else (helper fn returning Response, branching
///      with multiple Response { ... } per arm, etc.) → `None`
///      (the schema falls back to "application/json" — the legacy
///      behaviour).
///
/// Multi-arm bodies (if/match returning different `Response { ... }`
/// in each arm) are NOT handled in MVP — they keep the legacy
/// `application/json` path. Documented as minor debt for iter 2.
pub fn detect_response_content_type_kind(
    body: &[crate::ast::Stmt],
) -> Option<ResponseContentTypeKind> {
    use crate::ast::{Expr, Stmt};
    // Look at the last statement; ignore trailing empty stmts.
    let last = body
        .iter()
        .rev()
        .find(|s| !matches!(s, Stmt::Expr(Expr::Null(_), _)))?;
    let Stmt::Return(expr, _) = last else {
        return None;
    };
    // Unwrap `Ok(...)` wrapper for Result<Response>. `Ok(...)` is
    // a dedicated AST variant (NOT a Call), so we match it
    // directly.
    let inner: &Expr = match expr {
        Expr::Ok(boxed, _) => boxed.as_ref(),
        other => other,
    };
    let Expr::StructLit {
        type_name, fields, ..
    } = inner
    else {
        return None;
    };
    if type_name != "Response" {
        return None;
    }
    let mut content_type_kind: Option<String> = None;
    let mut dynamic_content_type = false;
    let mut is_binary = false;
    for (name, value) in fields {
        match name.as_str() {
            "content_type" => match value {
                Expr::Str(s, _) => content_type_kind = Some(s.clone()),
                _ => dynamic_content_type = true,
            },
            "body_bytes" if !matches!(value, Expr::Null(_)) => {
                is_binary = true;
            }
            _ => {}
        }
    }
    if dynamic_content_type {
        Some(ResponseContentTypeKind::Dynamic)
    } else {
        // Default content_type when not supplied is the built-in's
        // canonical default (matches `builtin_default_for("Response",
        // "content_type")` in codegen.rs).
        let media_type = content_type_kind.unwrap_or_else(|| "application/json".to_string());
        Some(ResponseContentTypeKind::Static {
            media_type,
            is_binary,
        })
    }
}

/// Phase Q.4 mini-batch: walks a fn body and returns the
/// `Stmt::ReturnStatus` with literal Int status that were found.
/// Non-literal statuses (variables, expressions) are omitted — we
/// cannot know them statically. Recurses inside loops, if/match,
/// etc.; inline FnExpr is NOT followed (different scope, different
/// fn). The returned Vec is deduplicated and in ascending order so
/// the schema is deterministic.
///
/// OAPI mini-batch — wrapper that delegates to the variant with an
/// empty const table (back-compat with tests and
/// `routes_from_registry` on the runtime path, where no top-level
/// AST is available).
pub fn collect_status_codes(body: &[crate::ast::Stmt]) -> Vec<u16> {
    let empty = std::collections::HashMap::new();
    collect_status_codes_with_consts(body, &empty)
}

/// OAPI mini-batch — variant that accepts a `const_name → Int` table
/// with the program's top-level constants (`let NOT_FOUND = 404`).
/// When the `status` field of an `Err(StructLit { ... })` or the
/// status of a `Stmt::ReturnStatus` is an `Expr::Ident` whose name
/// matches an entry in the table, it resolves to the literal value
/// and is included in the schema. Idents that don't resolve (local
/// vars, dynamic expressions) are still silently omitted as before.
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

/// OAPI mini-batch — program pre-scan to extract top-level
/// `let X = <Int literal>` (including `Expr::UnaryOp::Neg` wrapping
/// an Int for negative cases like `let TIMEOUT = -1` — irrelevant
/// for status codes but consistent). Returns a `HashMap<name, value>`
/// ready to pass to `collect_status_codes_with_consts`. Vars with a
/// non-literal or different-typed RHS are silently omitted. Only
/// top-level scope (no const inside fn / inside type).
pub fn collect_top_level_int_consts(
    program: &crate::ast::Program,
) -> std::collections::HashMap<String, i64> {
    use crate::ast::Stmt;
    let mut out = std::collections::HashMap::new();
    for s in program {
        if let Stmt::Assign { target, value, .. } = s {
            // Only simple bindings `let X = ...` (no field assign).
            let crate::ast::AssignTarget::Ident(name, _) = target else {
                continue;
            };
            // OAPI-Expr — uses `resolve_status_value` which now
            // accepts Int literal, Ident to a previous const,
            // UnaryOp::Neg and simple BinOp (Add/Sub/Mul). Walks in
            // declaration order so that `let Y = X + 4` resolves X
            // when Y is reached.
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
                // Status outside the valid HTTP range (100-599) we
                // also skip — the runtime/parser would catch it.
                if (100..=599).contains(&n) {
                    out.push(n as u16);
                }
            }
            // The body can contain another nested ReturnStatus via
            // if/match — we walk it.
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

/// OAPI + OAPI-Expr mini-batch — resolves a `status:` value or
/// `Stmt::ReturnStatus.status` to an Int. Accepts:
/// - `Expr::Int(n)` direct literal.
/// - `Expr::Ident(name)` with lookup in the top-level const table.
/// - `Expr::UnaryOp::Neg` over any of the above.
/// - `Expr::BinOp` with simple arithmetic Add/Sub/Mul over the
///   above (const-eval). Allows patterns like `status: BASE + 1` or
///   `status: -CODE`.
///
/// Anything else (Div by 0, calls, local vars, etc.) returns None —
/// the schema falls back to 500 default.
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
                // Div/Mod avoided for simplicity (division by 0).
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
        // HC.2 mini-batch — detect `Err(StructLit { status: <Int
        // literal>, ... })` and register the status code in the
        // schema. The canonical pattern is
        // `return Err(ApiErr { status: 404, ... })` where the
        // Result's E type has a `status: Int` field. The status code
        // is inferred from the literal at each call site.
        //
        // OAPI mini-batch — in addition to the Int literal, we now
        // accept references to top-level constants (`let NOT_FOUND
        // = 404`). The pattern `Err(ApiErr { status: NOT_FOUND, ... })`
        // resolves to 404 via the `consts` table. Local vars or
        // complex expressions are still omitted (fall back to the
        // 500 default).
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
        // Symmetric recursion so that `Ok`/`Err` wrapping other exprs
        // don't hide a nested ReturnStatus (not the typical case, but
        // coverage costs little).
        Expr::Ok(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
            collect_status_codes_expr(inner, out, consts);
        }
        // The other Exprs don't have bodies with nested stmts (calls,
        // literals, binops, etc.).
        _ => {}
    }
}

/// Extracts the `@header(name="X")` from a fn's decorator set
/// (Phase 7.6). Returns `Vec<(http_name, param_fitz, is_nullable)>`.
/// Replicates the evaluator's `collect_headers` logic. Assumes the
/// program passed the evaluator (the decorators are valid), so it
/// silently skips malformed cases — those errors are caught by the
/// runtime at eval time.
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
        // Phase Q.1 mini-batch: `into="alias"` lets the Fitz param
        // have a different name than the one derived by convention.
        // If absent, the previous convention is kept (lowercase +
        // `-` → `_`).
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

/// FITZ-05 — build-time replica of `collect_cookies`. Returns
/// `(cookie_name, param_name, is_nullable)` per `@cookie(name="X")`. Default
/// param name is the cookie name AS-IS (cookie names are case-sensitive), or the
/// `into="alias"` override.
pub(crate) fn cookies_from_decorators(
    decorators: &[crate::ast::Decorator],
    params: &[crate::ast::Param],
) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for deco in decorators {
        if deco.name != "cookie" {
            continue;
        }
        let Some(name_kw) = deco.kwargs.iter().find(|(k, _)| k == "name") else {
            continue;
        };
        let crate::ast::Expr::Str(cookie_name, _) = &name_kw.1 else {
            continue;
        };
        if cookie_name.is_empty() {
            continue;
        }
        let param_name = match deco.kwargs.iter().find(|(k, _)| k == "into") {
            Some((_, crate::ast::Expr::Str(alias, _))) if !alias.is_empty() => alias.clone(),
            _ => cookie_name.clone(),
        };
        let Some(p) = params.iter().find(|p| p.name == param_name) else {
            continue;
        };
        let is_nullable = matches!(&p.type_, Some(t) if t.is_nullable());
        out.push((cookie_name.clone(), param_name, is_nullable));
    }
    out
}

/// Builds `OpenApiRouteInfo` from the AST at build-time, without
/// evaluating the program (Phase 7.5). Used from `codegen.rs` so
/// that `fitz build` can emit the OpenAPI schema without going
/// through the HTTP runtime.
///
/// Body param detection replicates the evaluator's rule: any param
/// of the handler that is NOT in the path template nor in the query
/// params is considered body. At most one per handler (validation
/// in the evaluator during registration; we don't repeat it here —
/// if the program passes the evaluator, this AST is consistent).
///
/// Only fails if `parse_path_template` rejects the path (malformed
/// template).
pub fn pseudo_routes_from_ast(
    program: &crate::ast::Program,
) -> Result<Vec<OpenApiRouteInfo>, crate::error::FitzError> {
    pseudo_routes_from_program_and_modules(program, &[])
}

/// 10.8.5 (v0.10.8) — cross-module aware variant of
/// `pseudo_routes_from_ast`. Combines the handlers from the
/// `program` (main) and the `module_http_stmts` (slices preserved
/// by W16 — `LoadedModule.http_fn_stmts`) into a single Vec before
/// extracting. Result: the emitted OpenAPI 3.1 schema contains ALL
/// HTTP endpoints, including those from imported modules.
///
/// Before the v0.10.8 fix #3, `pseudo_routes_from_ast` only looked
/// at main → empty schema (`paths: []`) when HTTP handlers lived
/// cross-module. W16 already plugged the routes into the Router,
/// but the auto OpenAPI documentation was not updated.
pub fn pseudo_routes_from_program_and_modules(
    program: &crate::ast::Program,
    module_http_stmts: &[&[crate::ast::Stmt]],
) -> Result<Vec<OpenApiRouteInfo>, crate::error::FitzError> {
    use crate::ast::Stmt;
    use crate::http::parse_path_template;

    // OAPI mini-batch — pre-scan of top-level Int constants to
    // resolve `status: NOT_FOUND` inside Err({...}) and dynamic
    // ReturnStatus. Empty table if the program has no consts
    // (typical case) — zero overhead.
    let consts = collect_top_level_int_consts(program);

    // Concatenate all stmts: main first, then the modules in order.
    // We iterate over the unified slice.
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
            // Phase 7.6: collect headers from the same decorator
            // set. Same rules as the evaluator's `collect_headers`
            // (lowercase + `-` → `_` derivation, `Str` / `Str?`
            // type validation). At build-time we replicate the logic
            // here; if the fn passes the evaluator, this view is
            // consistent.
            let header_params = headers_from_decorators(decorators, params);
            // Phase 9.w.1.e — collect auth policy from the decorator
            // set. `@admin` wins over `@authenticated`.
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
            // body_param now excludes the auth user param (mirror of
            // codegen 9.w.1.d: "leftover" → auth user instead of body).
            let body_param_name = params.iter().find_map(|p| {
                let is_path = template.params.contains(&p.name);
                let is_query = template.query_params.contains(&p.name);
                let is_header = header_params.iter().any(|(_, fitz, _)| fitz == &p.name);
                if is_path || is_query || is_header {
                    return None;
                }
                // If the route has auth, the first leftover is the user
                // (NOT body). This heuristic matches the codegen rule.
                if auth != crate::http::AuthSpec::None {
                    return None;
                }
                Some(p.name.clone())
            });
            let param_type_exprs = params
                .iter()
                .map(|p| (p.name.clone(), p.type_.clone()))
                .collect();
            // v0.19.0 Block 4 — detect `Response` built-in
            // content_type from the handler's AST body (same walker
            // used in `route_info_from_spec` runtime path).
            let response_content_type = detect_response_content_type_kind(body);
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
                // Q.4: scan the FnDef body for ReturnStatus.
                // OAPI: resolve Idents pointing to top-level consts.
                custom_status_codes: collect_status_codes_with_consts(body, &consts),
                auth,
                response_content_type,
            });
        }
    }
    Ok(out)
}

/// Embedded HTML for the docs UI (Phase 7.3). Loads the Scalar
/// bundle from the jsdelivr CDN and points it at the
/// `/openapi.json` that the server serves alongside. ~10 lines; the
/// binary's weight doesn't move (the Scalar bundle is downloaded by
/// the browser, the first time `/docs` is visited).
///
/// Documented trade-off: the first load requires network. After
/// that the browser caches. If in the future we want a local
/// embedded bundle (offline), the `<script src>` is replaced with
/// `include_bytes!` of an asset. Today it's post-F7 debt.
pub const SCALAR_HTML: &str = include_str!("templates/scalar.html");

/// Generates the program's OpenAPI 3.1 schema.
///
/// Inputs:
///   - `routes`: lightweight views of the HTTP routes (see
///     `OpenApiRouteInfo`). In `fitz run` they come from the
///     registry via `routes_from_registry`; in `fitz build` they
///     are built from the AST at build-time.
///   - `program`: original AST — needed to walk the `Stmt::TypeDef`
///     and emit `components.schemas`.
///
/// Output: a `Value` that serialized with
/// `serde_json::to_string_pretty` is valid OpenAPI 3.1.
#[allow(dead_code)]
pub fn generate_openapi(routes: &[OpenApiRouteInfo], program: &Program) -> Value {
    generate_openapi_with_version(routes, program, None)
}

/// Variant of `generate_openapi` that accepts an `info.version`
/// override (Q.2 mini-batch). If `version` is `Some(v)`, the schema
/// emits that value; if `None`, defaults to `"0.1.0"` (back-compat
/// with pre-Q.2 use). The runtime reads it from
/// `HttpRegistry.server_config.api_version`; the codegen reads it
/// from the `Stmt::FnDef` decorated with `@server(api_version=...)`.
pub fn generate_openapi_with_version(
    routes: &[OpenApiRouteInfo],
    program: &Program,
    version: Option<&str>,
) -> Value {
    let mut components = Map::new();
    components.insert("schemas".into(), build_components_schemas(program));
    // Phase 9.w.1.e — security scheme. If at least one route has
    // `auth != None`, declare `bearerAuth` in components so tooling
    // clients (Scalar UI, Swagger UI, SDK generators) know how to
    // emit the lock icon + the token field. JWT bearer tokens are
    // the canonical pattern that `@auth_provider` expects (header
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

    // Phase 9.w.1.e — `security` per operation. For
    // `@authenticated`/`@admin` handlers declare the bearer token
    // requirement (reference to the global `bearerAuth` scheme). For
    // public handlers we do NOT emit `security` (OpenAPI's default
    // is "none" — we don't need explicit `security: []`).
    if route.auth != crate::http::AuthSpec::None {
        op.insert("security".into(), json!([{ "bearerAuth": [] }]));
    }

    op.insert(
        "responses".into(),
        build_responses_with_auth(
            &route.return_type_expr,
            &route.custom_status_codes,
            route.auth,
            route.response_content_type.as_ref(),
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
        // Nullable query params (Int?) are optional; the rest are
        // mandatory. No annotation → required = true (the handler
        // expects the value).
        let required = !t.map(|x| x.is_nullable()).unwrap_or(false);
        let schema = type_expr_to_schema_or_any(t);
        out.push(json!({
            "name": name,
            "in": "query",
            "required": required,
            "schema": schema,
        }));
    }
    // Phase 7.6: headers as parameters with in: "header". The name
    // is the canonical HTTP name (what the client must send); the
    // schema is always `string` (HTTP headers are strings; rich
    // types are explicit debt).
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
        None,
    )
}

/// Phase 9.w.1.e — variant of `build_responses` that also includes
/// `401` (handlers `@authenticated`/`@admin`) and `403` (handlers
/// `@admin`) when applicable. Documents to the schema's consumer
/// that the endpoint emits those status codes — the auth wrapper
/// produces them automatically, they are not from the user handler.
fn build_responses_with_auth(
    return_type: &Option<TypeExpr>,
    custom_status_codes: &[u16],
    auth: crate::http::AuthSpec,
    response_content_type: Option<&ResponseContentTypeKind>,
) -> Value {
    let mut resp: Map<String, Value> = Map::new();
    // v0.19.0 Block 4 — Response built-in shortcut. When the
    // handler returns `Response` (or `Result<Response>` with the
    // Ok arm being a literal `Response { ... }`), the 200 response
    // uses the custom content_type (or "application/octet-stream"
    // when dynamic) instead of the legacy "application/json" path.
    // The Err arm of `Result<Response>` still emits 500 with JSON
    // body (parallel to the codegen wrapper of Block 3.c).
    if let Some(kind) = response_content_type {
        let (media_type, is_binary) = match kind {
            ResponseContentTypeKind::Static {
                media_type,
                is_binary,
            } => (media_type.as_str(), *is_binary),
            ResponseContentTypeKind::Dynamic => ("application/octet-stream", true),
        };
        resp.insert(
            "200".into(),
            response_built_in_success(media_type, is_binary),
        );
        // Preserve 500 when the return type is `Result<Response>`
        // (the Err arm goes through the legacy 500 + JSON error).
        if matches!(
            return_type,
            Some(TypeExpr::Generic { name, args })
                if name == "Result" && args.len() == 1
        ) {
            resp.insert("500".into(), error_response());
        }
    } else {
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
    }
    // Phase 9.w.1.e — add 401 (auth) and 403 (admin). The body is
    // always `{"error": "<msg>"}` (auth wrapper format, parallel to
    // path param/body validation errors). If the handler also
    // declares `@admin`, 403 is emitted separately. Do NOT overwrite
    // existing entries (rare case: handler that returns 401 manually
    // via `return 401 { ... }`).
    if auth != crate::http::AuthSpec::None && !resp.contains_key("401") {
        resp.insert("401".into(), auth_error_response("Authentication required"));
    }
    if auth == crate::http::AuthSpec::Admin && !resp.contains_key("403") {
        resp.insert(
            "403".into(),
            auth_error_response("Permission denied (admin)"),
        );
    }
    // Q.4: add entries for each custom status code detected in the
    // body. A ReturnStatus body is polymorphic (a handler `-> User`
    // can emit `return 404 { "error": "..." }` with a different
    // shape), so the schema of each custom response stays as "any"
    // (`{}`). If there is already an entry with the same status
    // (rare case: `return 200 { ... }`), the return type's wins
    // (we don't overwrite). Codes come ascending-ordered and
    // deduplicated.
    for code in custom_status_codes {
        let key = code.to_string();
        if resp.contains_key(&key) {
            continue;
        }
        resp.insert(key, custom_status_response(*code));
    }
    Value::Object(resp)
}

/// Phase 9.w.1.e — shape of the 401/403 responses emitted by the
/// auth wrapper. Body always `{"error": "<msg>"}` — mirror of the
/// `serde_json::json!({"error": ...})` from runtime + codegen.
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
                // Polymorphic body: we don't pin a schema. The user
                // describes it in external docs if needed.
                "schema": {},
            },
        },
    })
}

/// Minimal mapping status code → reason phrase for the schema's
/// `description`. Covers the common codes; the rest fall back to
/// "Response". The schema is still valid regardless of the text.
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

/// v0.19.0 Block 4 — 200 response when the handler returns the
/// `Response` built-in. The content key is the user-supplied
/// media_type (or "application/octet-stream" when dynamic). The
/// schema is `{"type":"string","format":"binary"}` for binary
/// payloads (body_bytes set) and `{"type":"string"}` for text
/// payloads (body Str). The text variant is intentionally loose
/// — XML / HTML / CSV / plain text are all "strings" from JSON
/// Schema's point of view; pinning a finer schema would lie about
/// the contract (the user controls the bytes verbatim).
fn response_built_in_success(media_type: &str, is_binary: bool) -> Value {
    let schema = if is_binary {
        json!({ "type": "string", "format": "binary" })
    } else {
        json!({ "type": "string" })
    };
    json!({
        "description": "OK",
        "content": {
            media_type: {
                "schema": schema,
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
        // No annotation: empty schema = "any JSON value".
        None => json!({}),
    }
}

/// Translates a `TypeExpr` to a JSON Schema 2020-12 schema (subset).
/// Mapping:
///   - `Int`           → `{"type":"integer","format":"int64"}`
///   - `Float`         → `{"type":"number"}`
///   - `Str`           → `{"type":"string"}`
///   - `Bool`          → `{"type":"boolean"}`
///   - `Null`          → `{"type":"null"}`
///   - `T?`            → schema of T + `"nullable": true`
///   - `List<T>`       → `{"type":"array","items":<T>}`
///   - `Map<Str, V>`   → `{"type":"object","additionalProperties":<V>}`
///   - `Map<K, V>` with K ≠ Str → object with description (not
///     serializable as a JSON object with non-Str keys).
///   - `Result<T>`     → schema of T (in value position; in return
///     it is processed specially in `build_responses`).
///   - `Foo` (nominal) → `{"$ref":"#/components/schemas/Foo"}`.
///   - `Fn(...) -> R`  → description (not serializable).
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
            "description": format!("{} (Fitz function, not serializable)", t.display_name()),
        }),
        // Tuples (T mini-batch): JSON has no tuples, we serialize as
        // a prefix-typed array. OpenAPI 3.1 schema supports
        // `prefixItems` for this.
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
        // 9.w.2-binary-frames — `Bytes` maps to `string` with
        // `format: binary` (OpenAPI 3.x / AsyncAPI 3.0 standard for
        // raw bytes on the wire — the WS frame or HTTP body is an
        // opaque octet-stream, not base64-encoded JSON). Tools like
        // Scalar/AsyncAPI Studio render it as "binary upload"/
        // "binary payload".
        "Bytes" => json!({ "type": "string", "format": "binary" }),
        // Nominal: ref to components.schemas. If the type was not
        // declared in this program, the ref stays dangling — the
        // schema's consumer tool (Scalar, SDK generator) handles
        // adjustment. We don't abort over this so the generator
        // stays decoupled from the checker.
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
        // "required" in JSON Schema = not nullable and no default.
        // If the field has a default → the server fills it in when
        // missing. If nullable → can be explicit `null`.
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

    // Construction helpers.
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
    fn schema_for_int() {
        let s = type_expr_to_schema(&named("Int"));
        assert_eq!(s, json!({ "type": "integer", "format": "int64" }));
    }

    #[test]
    fn schema_for_float_str_bool_null() {
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
    fn schema_for_nullable_adds_nullable_flag() {
        let s = type_expr_to_schema(&nullable(named("Str")));
        assert_eq!(s, json!({ "type": "string", "nullable": true }));
    }

    #[test]
    fn schema_for_list_is_array_with_items() {
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
    fn schema_for_map_str_is_object_with_additional_properties() {
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
    fn schema_for_map_non_str_has_description() {
        let s = type_expr_to_schema(&generic("Map", vec![named("Int"), named("Str")]));
        let obj = s.as_object().unwrap();
        assert_eq!(obj.get("type"), Some(&json!("object")));
        // The description explains that the keys are not Str.
        let desc = obj.get("description").unwrap().as_str().unwrap();
        assert!(desc.contains("Map<Int, Str>"), "description was: {}", desc);
    }

    #[test]
    fn schema_for_nominal_is_ref_to_components_schemas() {
        let s = type_expr_to_schema(&named("User"));
        assert_eq!(s, json!({ "$ref": "#/components/schemas/User" }));
    }

    #[test]
    fn schema_for_result_in_value_position_is_the_inner() {
        // In value position (not return), Result<T> flattens to the inner T.
        let s = type_expr_to_schema(&generic("Result", vec![named("Int")]));
        assert_eq!(s, json!({ "type": "integer", "format": "int64" }));
    }

    #[test]
    fn schema_for_list_of_nominals_nests_ref() {
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
    fn responses_without_return_type_only_emit_200_any() {
        let r = build_responses(&None, &[]);
        let obj = r.as_object().unwrap();
        assert!(obj.contains_key("200"));
        assert!(!obj.contains_key("500"));
        // Empty schema (any).
        let schema = obj["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema, json!({}));
    }

    #[test]
    fn responses_with_concrete_return_type_emit_only_200() {
        let r = build_responses(&Some(named("Int")), &[]);
        let obj = r.as_object().unwrap();
        assert!(obj.contains_key("200"));
        assert!(!obj.contains_key("500"));
        let schema = obj["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema, json!({ "type": "integer", "format": "int64" }));
    }

    // ---- Q.4: status codes custom en schema ----

    #[test]
    fn responses_adds_entries_for_custom_status_codes() {
        let r = build_responses(&Some(named("Str")), &[401, 404]);
        let obj = r.as_object().unwrap();
        // 200 still there (from the return type Str).
        assert!(obj.contains_key("200"));
        // 401 and 404 added with an empty schema.
        assert!(obj.contains_key("401"));
        assert!(obj.contains_key("404"));
        assert_eq!(
            obj["401"]["content"]["application/json"]["schema"],
            json!({})
        );
        // Description uses the HTTP reason phrase.
        assert_eq!(obj["401"]["description"], json!("Unauthorized"));
        assert_eq!(obj["404"]["description"], json!("Not Found"));
    }

    #[test]
    fn responses_custom_status_does_not_overwrite_existing_200() {
        // If a handler does `return 200 { ... }` and also has a
        // `Str` return type, the return type's 200 entry wins — we
        // keep the strong schema over the polymorphic one.
        let r = build_responses(&Some(named("Str")), &[200]);
        let schema = r["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema, json!({ "type": "string" }));
    }

    #[test]
    fn responses_custom_status_does_not_overwrite_500_from_result() {
        // Result<T> generates 200+500. A custom `return 500 { ... }`
        // must not duplicate them.
        let r = build_responses(&Some(generic("Result", vec![named("Int")])), &[500]);
        let obj = r.as_object().unwrap();
        // The 500 stays as the Result's "error", not the custom any.
        let schema = obj["500"]["content"]["application/json"]["schema"].clone();
        assert_eq!(schema["type"], json!("object"));
    }

    #[test]
    fn responses_unknown_custom_status_uses_default_response_phrase() {
        let r = build_responses(&None, &[418]);
        assert_eq!(r["418"]["description"], json!("Response"));
    }

    #[test]
    fn collect_status_codes_simple_extraction_and_order() {
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
        // Ascending order + dedup.
        assert_eq!(collect_status_codes(&body), vec![401u16, 404u16]);
    }

    #[test]
    fn collect_status_codes_non_literal_status_is_omitted() {
        use crate::ast::Span;
        // `return <ident> { ... }` is not inferable — skipped.
        let body = vec![crate::ast::Stmt::ReturnStatus {
            status: crate::ast::Expr::Ident("code".into(), Span::ZERO),
            body: None,
            span: Span::ZERO,
        }];
        assert!(collect_status_codes(&body).is_empty());
    }

    #[test]
    fn collect_status_codes_status_out_of_range_is_omitted() {
        use crate::ast::Span;
        // 1000 is not a valid HTTP status → skip (parser/runtime
        // would catch it but the schema shouldn't emit codes that
        // can never appear).
        let body = vec![crate::ast::Stmt::ReturnStatus {
            status: crate::ast::Expr::Int(1000, Span::ZERO),
            body: None,
            span: Span::ZERO,
        }];
        assert!(collect_status_codes(&body).is_empty());
    }

    #[test]
    fn oapi_collect_top_level_int_consts_recolecta_lets_int() {
        // OAPI + OAPI-Expr mini-batch — the pre-scan detects
        // `let X = <Int>`, `let Y = -<Int>` and simple BinOps
        // (`let SUM = 1 + 2` now DOES resolve to 3, OAPI-Expr
        // refinement). Walking in order allows references to
        // previous consts (`let Y = X + 4`). Unresolvable RHS is
        // omitted.
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
    fn oapi_returnstatus_with_ident_to_top_level_const_appears_in_schema() {
        // `return NOT_FOUND { ... }` where NOT_FOUND is a top-level
        // Int const resolves to 404 and lands in the schema.
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
            "expected 404 in the schema, was: {:?}",
            responses
        );
    }

    #[test]
    fn oapi_err_struct_with_status_ident_appears_in_schema() {
        // `Err(ApiErr { status: NOT_FOUND, ... })` with NOT_FOUND as
        // a top-level const resolves.
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
            "expected 404 in the schema, was: {:?}",
            responses
        );
    }

    #[test]
    fn oapi_ident_unresolved_is_silently_omitted() {
        // If the Ident doesn't point at a top-level Int const (local
        // var, fn param, etc.), it's omitted — schema falls back to
        // the 500 default.
        let src = "\
            @get(\"/x\")\n\
            fn h(code: Int) -> Int {\n\
                return code {\"error\": \"x\"}\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = &schema["paths"]["/x"]["get"]["responses"];
        // 200 from the return type, 500 default, no extra codes.
        // The `return code { ... }` does not resolve statically.
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
            "expected no specific codes from dynamic Ident, was: {:?}",
            codes
        );
    }

    #[test]
    fn schema_for_handler_with_returnstatus_emits_codes() {
        let src = "\
            @get(\"/p\")\n\
            fn protected() -> Str {\n\
                return 401 {\"msg\": \"unauthorized\"}\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = &schema["paths"]["/p"]["get"]["responses"];
        assert!(responses.get("200").is_some()); // from the return type Str
        assert!(responses.get("401").is_some()); // from the custom ReturnStatus
        assert_eq!(responses["401"]["description"], json!("Unauthorized"));
    }

    #[test]
    fn schema_codes_inside_if_else_are_detected() {
        // `Stmt::ReturnStatus` inside an `if`/`else` is detected
        // recursively. The walker descends into the then/else branch.
        let src = "\
            @get(\"/u/{id}\")\n\
            fn h(id: Int) -> Str {\n\
                if (id == 0) {\n\
                    return 404 {\"msg\": \"not found\"}\n\
                }\n\
                return \"ok\"\n\
            }\n\
        ";
        let schema = schema_for(src);
        let responses = &schema["paths"]["/u/{id}"]["get"]["responses"];
        assert!(responses.get("404").is_some());
    }

    #[test]
    fn responses_with_result_emit_200_and_500() {
        let r = build_responses(&Some(generic("Result", vec![named("User")])), &[]);
        let obj = r.as_object().unwrap();
        assert!(obj.contains_key("200"));
        assert!(obj.contains_key("500"));
        // 200 carries the inner.
        let ok_schema = obj["200"]["content"]["application/json"]["schema"].clone();
        assert_eq!(ok_schema, json!({ "$ref": "#/components/schemas/User" }));
        // 500 carries `{error: string}`.
        let err_schema = obj["500"]["content"]["application/json"]["schema"].clone();
        assert_eq!(err_schema["type"], json!("object"));
        assert_eq!(err_schema["required"], json!(["error"]));
    }

    // -------- type_def_to_schema --------

    #[test]
    fn type_def_emits_object_with_properties_and_required() {
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
        // Required includes both (no default and not nullable).
        let req = s["required"].as_array().unwrap();
        assert!(req.contains(&json!("id")));
        assert!(req.contains(&json!("name")));
    }

    #[test]
    fn type_def_excludes_nullables_and_default_from_required() {
        let fields = vec![
            Field {
                name: "id".into(),
                type_: named("Int"),
                default: None,
                decorators: vec![],
            },
            // Nullable: optional, does not appear in required.
            Field {
                name: "nickname".into(),
                type_: nullable(named("Str")),
                default: None,
                decorators: vec![],
            },
            // With default: optional, does not appear in required.
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

    /// Helper: parses + evaluates the src inside an active registry,
    /// and returns the schema. Useful for end-to-end tests of the
    /// generator that verify the complete wiring (TypeExpr →
    /// RouteSpec → schema).
    fn schema_for(src: &str) -> Value {
        let program = parse(tokenize(src).expect("lex OK")).expect("parse OK");
        let (res, registry) = crate::http::with_active_registry(|| {
            crate::evaluator::eval_with_base_sync(program.clone(), std::env::current_dir().unwrap())
        });
        res.expect("eval OK");
        // Q.2: replicates the real wiring of main.rs / http.rs — if
        // the program declares `@server(api_version=...)`, the
        // schema reflects it. Without the override, defaults to
        // "0.1.0".
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
    fn generator_emits_top_level_openapi_3_1_structure() {
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
    fn without_api_version_kwarg_default_remains_0_1_0() {
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
        // Direct test of the generator: with and without override.
        use crate::ast::Stmt;
        let program: Vec<Stmt> = vec![];
        let s1 = generate_openapi_with_version(&[], &program, Some("9.9.9"));
        assert_eq!(s1["info"]["version"], json!("9.9.9"));
        let s2 = generate_openapi_with_version(&[], &program, None);
        assert_eq!(s2["info"]["version"], json!("0.1.0"));
    }

    #[test]
    fn simple_get_route_appears_in_paths_with_operation_id() {
        let src = "@get(\"/health\")\nfn ping() => \"ok\"";
        let schema = schema_for(src);
        let get = &schema["paths"]["/health"]["get"];
        assert_eq!(get["operationId"], json!("ping"));
        assert!(get["responses"]["200"].is_object());
    }

    #[test]
    fn route_with_path_param_emits_parameter_in_path() {
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
    fn route_with_nullable_query_param_is_not_required() {
        let src = "@get(\"/search?limit={limit}\")\nfn search(limit: Int?) => limit";
        let schema = schema_for(src);
        let params = schema["paths"]["/search"]["get"]["parameters"]
            .as_array()
            .unwrap();
        assert_eq!(params[0]["name"], json!("limit"));
        assert_eq!(params[0]["in"], json!("query"));
        assert_eq!(params[0]["required"], json!(false));
        // The schema carries nullable: true.
        assert_eq!(params[0]["schema"]["nullable"], json!(true));
    }

    #[test]
    fn route_post_with_custom_type_body_emits_request_body_and_ref() {
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
        // And `UserInput` is in components.schemas.
        let user_input_schema = &schema["components"]["schemas"]["UserInput"];
        assert_eq!(user_input_schema["type"], json!("object"));
        assert!(user_input_schema["properties"]["name"].is_object());
    }

    #[test]
    fn route_with_required_header_appears_in_parameters() {
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
    fn route_with_nullable_header_is_not_required() {
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
    fn return_result_user_emits_200_user_and_500_error() {
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

    // ---- HC.2 mini-batch — Err({ status: ... }) status codes in schema ----

    #[test]
    fn err_with_status_field_literal_appears_in_schema_responses() {
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
        assert!(responses.contains_key("200"), "expected 200 (Ok)");
        assert!(responses.contains_key("500"), "expected 500 (Err fallback)");
        assert!(
            responses.contains_key("404"),
            "expected 404 (Err with literal status)"
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

    // ---- Phase 9.w.1.e — OpenAPI security scheme ----

    /// Base program reused by the schema auth tests: an
    /// `@auth_provider` + 3 handlers (public, `@authenticated`,
    /// `@admin`).
    const AUTH_SCHEMA_SRC: &str = "\
type User { id: Int, name: Str, role: Str }\n\
@auth_provider\n\
fn check(headers: Map<Str, Str>) -> Result<User> {\n\
    return Err(\"no auth\")\n\
}\n\
@get(\"/public\")\n\
fn public_route() -> Str => \"no auth\"\n\
@authenticated\n\
@get(\"/me\")\n\
fn me(user: User) -> Str => user.name\n\
@admin\n\
@get(\"/admin\")\n\
fn admin_route(user: User) -> Str => \"hello admin\"\n\
";

    #[test]
    fn auth_schema_emits_security_schemes_bearer_auth() {
        let schema = schema_for(AUTH_SCHEMA_SRC);
        let security_schemes = schema["components"]["securitySchemes"].as_object();
        assert!(
            security_schemes.is_some(),
            "components.securitySchemes missing — expected bearerAuth",
        );
        let bearer = &schema["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(bearer["type"], json!("http"));
        assert_eq!(bearer["scheme"], json!("bearer"));
        assert_eq!(bearer["bearerFormat"], json!("JWT"));
    }

    #[test]
    fn auth_schema_public_handler_has_no_security() {
        let schema = schema_for(AUTH_SCHEMA_SRC);
        let op = &schema["paths"]["/public"]["get"];
        assert!(
            op.get("security").is_none(),
            "public handler should NOT have `security`, was: {:?}",
            op,
        );
        // Doesn't emit 401/403 either (not a wrapper auth case).
        let resp = op["responses"].as_object().unwrap();
        assert!(!resp.contains_key("401"));
        assert!(!resp.contains_key("403"));
    }

    #[test]
    fn auth_schema_authenticated_handler_requiere_bearer() {
        let schema = schema_for(AUTH_SCHEMA_SRC);
        let op = &schema["paths"]["/me"]["get"];
        // security: [{ bearerAuth: [] }]
        let sec = op["security"].as_array().expect("security must be array");
        assert_eq!(sec.len(), 1);
        assert!(
            sec[0].get("bearerAuth").is_some(),
            "first requirement should be bearerAuth, was: {:?}",
            sec[0],
        );
        // responses includes 401 (auth) but NOT 403 (not admin).
        let resp = op["responses"].as_object().unwrap();
        assert!(resp.contains_key("401"), "@authenticated emits 401");
        assert!(
            !resp.contains_key("403"),
            "@authenticated does NOT emit 403"
        );
        // 200 from the happy path must still be there.
        assert!(resp.contains_key("200"));
    }

    #[test]
    fn auth_schema_admin_handler_emits_401_and_403() {
        let schema = schema_for(AUTH_SCHEMA_SRC);
        let op = &schema["paths"]["/admin"]["get"];
        let sec = op["security"].as_array().expect("security must be array");
        assert_eq!(sec.len(), 1);
        assert!(sec[0].get("bearerAuth").is_some());
        let resp = op["responses"].as_object().unwrap();
        assert!(resp.contains_key("401"), "@admin emits 401");
        assert!(resp.contains_key("403"), "@admin emits 403");
        // 401 and 403 are objects with shape `{"error": <string>}`.
        let r401_schema = &resp["401"]["content"]["application/json"]["schema"];
        assert_eq!(r401_schema["type"], json!("object"));
        assert!(r401_schema["properties"]["error"].is_object());
    }

    #[test]
    fn auth_schema_program_without_auth_does_not_emit_security_schemes() {
        // Without auth handlers, components.securitySchemes must be
        // omitted (don't emit an empty object — less noise in the
        // schema).
        let src = "\
@get(\"/x\")\n\
fn x() -> Str => \"ok\"\n\
";
        let schema = schema_for(src);
        assert!(
            schema["components"].get("securitySchemes").is_none(),
            "programs without auth should NOT emit securitySchemes",
        );
    }

    // ---- v0.19.0 Block 4 — Response built-in in OpenAPI schema ----
    //
    // Tests that handlers returning `Response { content_type: "X",
    // body: ... }` (or `body_bytes: ...` for binary) generate the
    // 200 response with the user-supplied media_type and schema
    // (`format: binary` when applicable) instead of the legacy
    // application/json + schema-from-T path.

    #[test]
    fn v019_block4_response_with_static_content_type_emits_custom_media_type() {
        // `fn rss() => Response { content_type: "application/rss+xml",
        //   body: "<rss/>" }`: schema 200 must list the custom media
        // type, NOT application/json.
        let src = "\
@get(\"/feed.rss\")
fn rss_feed() => Response {
    content_type: \"application/rss+xml; charset=utf-8\",
    body: \"<rss/>\",
}
";
        let schema = schema_for(src);
        let resp_200 = &schema["paths"]["/feed.rss"]["get"]["responses"]["200"];
        let content = &resp_200["content"];
        assert!(
            content.get("application/rss+xml; charset=utf-8").is_some(),
            "expected `application/rss+xml; charset=utf-8` key, was: {}",
            content
        );
        assert!(
            content.get("application/json").is_none(),
            "must NOT emit application/json for Response built-in"
        );
        // Schema should be a Str body (NOT binary, since body_bytes
        // wasn't supplied).
        let schema_obj = &content["application/rss+xml; charset=utf-8"]["schema"];
        assert_eq!(schema_obj["type"], json!("string"));
        assert!(
            schema_obj.get("format").is_none(),
            "non-binary schema must NOT have format key, was: {}",
            schema_obj
        );
    }

    #[test]
    fn v019_block4_response_with_body_bytes_emits_format_binary() {
        // `body_bytes: bytes(...)` set → schema marks `format: binary`.
        let src = "\
@get(\"/pdf\")
fn pdf() => Response {
    content_type: \"application/pdf\",
    body_bytes: bytes(\"%PDF-1.7 ...\"),
}
";
        let schema = schema_for(src);
        let resp_200 = &schema["paths"]["/pdf"]["get"]["responses"]["200"];
        let content = &resp_200["content"];
        let pdf_content = &content["application/pdf"];
        let schema_obj = &pdf_content["schema"];
        assert_eq!(schema_obj["type"], json!("string"));
        assert_eq!(schema_obj["format"], json!("binary"));
    }

    #[test]
    fn v019_block4_response_with_default_content_type_emits_application_json() {
        // `Response { body: "X" }` (no content_type supplied) defaults
        // to "application/json" (the built-in's canonical default,
        // matches `builtin_default_for`). This case is rare but should
        // be predictable.
        let src = "\
@get(\"/x\")
fn x() => Response { body: \"hi\" }
";
        let schema = schema_for(src);
        let content = &schema["paths"]["/x"]["get"]["responses"]["200"]["content"];
        assert!(content.get("application/json").is_some());
        let schema_obj = &content["application/json"]["schema"];
        // Note: type is `string` (the Response body field, a Str),
        // NOT a serialization of the Instance — that would be the
        // legacy `__to_fitz_json()` path. The Response built-in
        // shortcut wins.
        assert_eq!(schema_obj["type"], json!("string"));
    }

    #[test]
    fn v019_block4_result_response_keeps_500_for_err_arm() {
        // `fn h() -> Result<Response>`: 200 uses Response built-in
        // shortcut; 500 still uses the legacy JSON error_response
        // (parallel to the codegen Err arm).
        let src = "\
@get(\"/feed.rss\")
fn rss_feed() -> Result<Response> => Ok(Response {
    content_type: \"application/rss+xml\",
    body: \"<rss/>\",
})
";
        let schema = schema_for(src);
        let responses = &schema["paths"]["/feed.rss"]["get"]["responses"];
        assert!(responses["200"]["content"]["application/rss+xml"].is_object());
        // 500 must still be present.
        let resp_500 = &responses["500"];
        assert!(
            resp_500["content"]["application/json"].is_object(),
            "500 must keep application/json error body, was: {}",
            resp_500
        );
    }

    #[test]
    fn v019_block4_normal_handler_keeps_legacy_application_json() {
        // Handlers that do NOT return Response built-in must keep
        // the legacy application/json + schema-from-T path intact.
        let src = "\
@get(\"/users\")
fn list_users() -> List<Str> => [\"alice\", \"bob\"]
";
        let schema = schema_for(src);
        let content = &schema["paths"]["/users"]["get"]["responses"]["200"]["content"];
        assert!(content.get("application/json").is_some());
        let schema_obj = &content["application/json"]["schema"];
        assert_eq!(schema_obj["type"], json!("array"));
        assert_eq!(schema_obj["items"]["type"], json!("string"));
    }

    #[test]
    fn v019_block4_detect_helper_unit_tests() {
        // Direct unit tests on `detect_response_content_type_kind`
        // covering the heuristic's edge cases without going through
        // the schema_for pipeline.
        use crate::ast::{Expr, Span, Stmt};
        let str_body_resp = Stmt::Return(
            Expr::StructLit {
                type_name: "Response".into(),
                fields: vec![
                    (
                        "content_type".into(),
                        Expr::Str("text/html".into(), Span::ZERO),
                    ),
                    ("body".into(), Expr::Str("<h1/>".into(), Span::ZERO)),
                ],
                span: Span::ZERO,
            },
            Span::ZERO,
        );
        assert_eq!(
            detect_response_content_type_kind(&[str_body_resp]),
            Some(ResponseContentTypeKind::Static {
                media_type: "text/html".into(),
                is_binary: false,
            }),
        );
        // Dynamic content_type (ident).
        let dynamic_ct = Stmt::Return(
            Expr::StructLit {
                type_name: "Response".into(),
                fields: vec![(
                    "content_type".into(),
                    Expr::Ident("ct_var".into(), Span::ZERO),
                )],
                span: Span::ZERO,
            },
            Span::ZERO,
        );
        assert_eq!(
            detect_response_content_type_kind(&[dynamic_ct]),
            Some(ResponseContentTypeKind::Dynamic),
        );
        // Non-Response struct lit → None.
        let other = Stmt::Return(
            Expr::StructLit {
                type_name: "User".into(),
                fields: vec![],
                span: Span::ZERO,
            },
            Span::ZERO,
        );
        assert_eq!(detect_response_content_type_kind(&[other]), None);
        // Empty body → None.
        assert_eq!(detect_response_content_type_kind(&[]), None);
    }
}
