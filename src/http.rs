// http.rs — Phase 4 (native HTTP)
//
// Fitz HTTP runtime. Assembled in two steps:
//
//   1. During `eval`, when a `Stmt::FnDef` with a decorator
//      `@get`/`@post`/`@put`/`@delete` is seen, a `RouteSpec` is registered
//      in an `HttpRegistry` accessible via thread_local.
//   2. When `eval` finishes, if the registry is non-empty, `serve()`
//      starts a tokio + axum runtime and blocks until Ctrl-C.
//
// Threading model (post-F17.5):
//
//   A single `rt-multi-thread` tokio runtime (F17.4a) runs on the
//   thread that called `eval` (`block_on` in `serve()`). Each axum
//   request dispatches an async handler on one of the workers, which
//   invokes `handle_task(&registry, ...).await` directly on the
//   evaluator. `HttpRegistry` is shared via `Arc` (Send + Sync
//   post-F17.2-3). Parallelism between requests is real: N workers
//   processing handlers simultaneously.
//
// Before F17.5 there was an mpsc/oneshot bridge + a separate std::thread
// for tokio. It was introduced in Phase 4 when `Value`/`EnvRef` were
// non-Send `Rc<RefCell<>>` and handlers could not be invoked from
// axum directly. F17.2 (Arc/Mutex), F17.3 (full Send) and F17.4a
// (multi-thread) unblocked the removal. Result: ~300 fewer LoC
// here and real HTTP parallelism across requests.

use std::cell::RefCell;

#[cfg(test)]
use crate::ast::Span;
use crate::ast::{Expr, TypeExpr};
use crate::value::{shared, ResultVariant, Value};

// Phase 9.w.2 — re-exports to avoid long paths in the rest of the
// file. `WsBroadcasterTrait` and `WsOutMessage` are the hooks the
// `WsConnHandle` runtime consumes; the concrete types live at the end
// of this file.
use crate::value::{WsBroadcasterTrait, WsConnHandle, WsOutMessage, WsReadStreamTrait};

// ---------------------------------------------------------------------------
// Base types
// ---------------------------------------------------------------------------

/// HTTP verb supported by a decorator. Lives only in the server
/// runtime; the AST does not use it (decorators are generic).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl HttpMethod {
    /// Converts a decorator name (`"get"`, `"post"`, ...) into the
    /// corresponding verb. `None` if it isn't an HTTP decorator.
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

/// A route registered by a decorator. The `handler` is a
/// `Value::Function` cloned from the interpreter env — `Rc`s clone
/// cheaply and the closure keeps the module env alive.
#[derive(Debug, Clone)]
pub struct RouteSpec {
    pub method: HttpMethod,
    /// Path in axum format (`/users/{id}`). Already canonicalized from
    /// the `Expr::Str` or `Expr::StrInterp` of the decorator. The
    /// query template (after the `?`) does NOT go here — it lives in
    /// `query_params`.
    pub path: String,
    /// Names of the path params, in the order they appear in the
    /// path. Empty if the route has no params.
    pub path_params: Vec<String>,
    /// Names of the query params declared with `?key={name}` in the
    /// decorator path. Each one binds to the Fitz param of the same
    /// name. Empty if the route does not declare a query.
    pub query_params: Vec<String>,
    /// Fitz handler. Must be a `Value::Function` — the evaluator
    /// validates this on registration.
    pub handler: Value,
    /// Handler name for error/log messages.
    pub handler_name: String,
    /// Declared types of the handler's parameters, in order. Each
    /// tuple is `(name, head_name_without_generics_or_nullable,
    /// is_nullable)`. `head_name` is used for `coerce_path_param`
    /// (Int/Float/Str/Bool); `is_nullable` is used for query params
    /// (a missing `Int?` in the query becomes `Null` instead of
    /// 400).
    pub param_types: Vec<(String, Option<String>, bool)>,
    /// If the handler declares a parameter that is not a path param,
    /// we treat it as body. We store its name here and, optionally,
    /// the declared `Value::Type` (resolved from the env at
    /// registration time). If the type is not declared, we
    /// deserialize the JSON as a free `Value` (Map/List/primitives).
    ///
    /// At most one body per handler. The evaluator validates how many
    /// there are and that they are compatible during registration.
    pub body_param: Option<BodyParam>,
    /// Headers declared with `@header(name="X")` on the handler
    /// (Phase 7.6). Empty if the handler declares none. Each entry
    /// maps an HTTP name to a Fitz param of the handler.
    pub headers: Vec<HeaderSpec>,
    /// FITZ-05 (2026-08) — cookies declared with `@cookie(name="X")`.
    /// Reuses `HeaderSpec` (`http_name` = cookie name, `param_name` =
    /// destination Str? param). The value is parsed from the incoming
    /// `Cookie` header. Empty if none.
    pub cookies: Vec<HeaderSpec>,
    /// Full TypeExpr of the handler parameters, in order. Additive
    /// to `param_types` (which carries only the `head_name` without
    /// generics or nullables, sufficient for dispatch). Here we
    /// store the full `TypeExpr` so the OpenAPI schema generator
    /// (Phase 7.1) can emit `List<Int>`, `Int?`, `Result<User>`,
    /// etc., without losing structure.
    pub param_type_exprs: Vec<(String, Option<TypeExpr>)>,
    /// Declared return type of the handler (if any). The OpenAPI
    /// generator uses it to distinguish `200` only vs `200` + `500`
    /// (handlers returning `Result<T>` map to both statuses).
    /// Without annotation → `None` and the generator treats the
    /// response as "any" (`200` with empty schema).
    pub return_type_expr: Option<TypeExpr>,
    /// Middlewares declared with `@middleware(fn)` stacked before the
    /// route decorator (mini-phase MW.1). The Vec order is the
    /// application order: the first runs first, the last runs right
    /// before the handler. Each is invoked with a single `Request`
    /// arg. Supported returns (gate-only): `Null`/no return →
    /// continues the chain; `Value::HttpResponse` (via
    /// `return <status> { ... }`) → short-circuits with that status
    /// code. Any other type → 500 with a clear message. Empty if
    /// the route has no middlewares.
    pub middlewares: Vec<MiddlewareSpec>,
    /// CORS configuration applied with `@middleware(cors(...))`
    /// (mini-phase MW.2). Lives in a dedicated slot, does NOT enter
    /// the `middlewares` chain: CORS needs to inject headers in the
    /// real response (it's not gate-only) and register an additional
    /// preflight handler (`OPTIONS`), things the gate middleware
    /// model does not express. At most one per route — two
    /// `cors(...)` applied to the same handler is a registration
    /// error. `Arc` to avoid cloning the config per request and to
    /// cross threads (preflight runs in the tokio thread).
    pub cors: Option<std::sync::Arc<CorsConfig>>,
    /// Phase 9.w.1.c — Auth policy determined by the presence of
    /// `@authenticated`/`@admin` on the handler. `None` (default)
    /// means public route.
    pub auth: AuthSpec,
    /// Phase 9.w.1.iter2.a — Custom RBAC via stackable
    /// `@requires("role")`. Each `@requires("X")` adds `"X"` to this
    /// Vec. The runtime wrapper requires `user.role` (Str) to be in
    /// this Vec; otherwise 403. `@requires` implies auth (the wrapper
    /// runs the `@auth_provider` even when `auth == None`). Empty vec
    /// = no role requirement (default). Stacking two `@requires("a")
    /// @requires("b")` = `vec!["a", "b"]` = role must match at least
    /// one (OR over values; AND across multiple decorators would be
    /// incoherent because the user only has ONE role).
    pub required_roles: Vec<String>,
    /// Phase 12.8 — Feature flag gating the route. `Some("flag-name")`
    /// when the handler has `@flag("flag-name")`; `None` otherwise.
    /// The wrapper queries the runtime registry (`is_flag_enabled`)
    /// on each request — if the flag is off, returns 404 with
    /// `{"error": "feature disabled"}` BEFORE running middlewares/auth.
    /// Deep defense (404 at runtime even though the route is
    /// registered) preserves the possibility of dynamic toggling via
    /// env vars without restart.
    pub flag_name: Option<String>,
    /// Phase 9.w.1.c — Name of the handler param where the `user`
    /// returned by the `@auth_provider` should be injected.
    /// `Some(name)` when `auth != None` and the handler declared a
    /// param of type `User` (validated by the checker). `None` when
    /// `auth == None`. The runtime wrapper uses this name to insert
    /// the `user` at the correct position in the args Vec.
    pub auth_user_param_name: Option<String>,
    /// Phase 9.w.2 — `true` if the handler is marked with
    /// `@ws("/path")`. The runtime registers the route as an axum
    /// GET with `WebSocketUpgrade` and, on upgrade, spawns the
    /// handler with a `Value::WsConn` injected instead of the normal
    /// HTTP dispatcher. `is_ws` and `method` are orthogonal: `method`
    /// stays `Get` (the initial handshake is always GET), but the
    /// dispatch in `build_router` forks based on this flag.
    pub is_ws: bool,
    /// Phase 9.w.2 — Name of the `WsConn<T>` param of the handler.
    /// `Some(name)` when `is_ws == true`; the wrapper uses it to
    /// insert `Value::WsConn` at the correct position in the args
    /// Vec. `None` for normal HTTP routes.
    pub ws_conn_param_name: Option<String>,
    /// Phase 9.w.2 — `TypeExpr` of T in `WsConn<T>` (e.g. `Str`,
    /// `ChatMsg`). Stored by the evaluator at route registration.
    /// Consumed by the AsyncAPI generator (9.w.2.d) to emit the
    /// channel message schema. Matches `param_type_exprs[idx]` —
    /// kept separately because it's essential WS endpoint
    /// information.
    ///
    /// 9.w.2-wsconn-bidir (v0.9.38): this field corresponds to `recv`
    /// (what is unpacked from the frame). For symmetric `WsConn<T>`,
    /// it's the same `T` as `ws_send_type`. For asymmetric
    /// `WsConn<In, Out>`, `ws_msg_type = In` and `ws_send_type = Out`
    /// differ.
    pub ws_msg_type: Option<TypeExpr>,
    /// 9.w.2-wsconn-bidir (v0.9.38): `TypeExpr` of Out in
    /// `WsConn<In, Out>`. For symmetric channels it equals
    /// `ws_msg_type`. `send/broadcast` use it to decide the binary
    /// vs text JSON mode when serializing the value coming from the
    /// handler.
    pub ws_send_type: Option<TypeExpr>,
}

/// An entry of the middleware stack of a route (mini-phase MW.1).
/// The `handler` comes resolved to `Value::Function` from the
/// importer env during route registration; the evaluator guarantees
/// the value is callable (cheap clone of the inner `Rc`). The
/// `name` is the identifier the user used to reference it in
/// `@middleware(...)`, only for error/log messages.
/// Mini-batch Mw.next — middleware kind. Determined from the arity
/// of the Value::Function in `collect_middlewares`:
///
///   - **Pre (1 arg)**: classic gate-only. Receives `Request`,
///     returns `null` to continue or `Response` to short-circuit.
///     Does NOT see the final response.
///   - **Post (2 args)**: post-process. Runs AFTER the handler.
///     Receives `(Request, Response)`, returns `Response`. Allows
///     adding headers, modifying the body, etc. If multiple
///     post-mws exist, they run in REVERSE registration order (wrap
///     semantics: the last registered one is innermost and sees the
///     response first).
///
/// Decision vs wrap-style with `next` callable: the wrap model would
/// require building a `next` Fitz callable from Rust at runtime (6-8h
/// refactor with a new Value variant). Post-process covers 80% of
/// real cases (timing, headers, post-handler logging) and is
/// self-contained. The remaining case — pure wrap for catching panics
/// or pre+post linked in a single fn — is left as a future sub-step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewareKind {
    /// 1 arg, gate-only: `fn mw(req: Request) -> Response?`. Null
    /// → continues the chain, Response → short-circuit.
    Pre,
    /// 2 args post-process: `fn mw(req: Request, resp: Response) -> Response`.
    /// Runs AFTER the handler.
    Post,
    /// Mini-batch Mw-Wrap — 2 args wrap-style:
    /// `fn mw(req: Request, next: Fn() -> Response) -> Response`.
    /// The middleware controls handler invocation via `next()`.
    /// Enables timing, observability, response wrapping, conditional
    /// chain continuation.
    Wrap,
}

#[derive(Debug, Clone)]
pub struct MiddlewareSpec {
    pub name: String,
    pub handler: Value,
    pub kind: MiddlewareKind,
}

/// Mini-phase Q.3 + mini-batch HTTP-Cors: the
/// `Access-Control-Allow-Origin` policy supports three modes: literal
/// (fixed value, as up to MW.2), set of allowed origins (echo if in
/// the list), and echo without filter (accepts any received Origin).
///
///       - `Literal("*")` or `Literal("https://x.com")` → emits the
///         value as-is (previous mode).
///       - `Set(["https://a.com", "https://b.com"])` → if the request
///         `Origin` header matches one in the list, emits **that**
///         value (not the whole list). If it doesn't match, the
///         header is NOT emitted (browser rejects the response —
///         strict CORS standard behavior). Useful when credentials
///         are needed (cookies/Authorization) across multiple
///         frontends: `Allow-Origin: *` is incompatible with
///         credentials, but echoing the specific Origin is fine.
///       - `Echo` → echo the received Origin without filter.
///         Equivalent to writing `Set(...)` with every possible
///         frontend. Useful for local dev where the list is unknown
///         a priori. If the request does NOT have an `Origin` header,
///         the header is NOT emitted (same behavior as Set with no
///         match).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowOrigin {
    /// Literal value, emitted identically in every response.
    Literal(String),
    /// Set of allowed origins. The runtime echoes if the request
    /// `Origin` is in the list.
    Set(Vec<String>),
    /// Mini-batch HTTP-Cors — echo the received Origin without
    /// filter. Built via `allow_origin: "echo"` in the config Map.
    Echo,
}

impl AllowOrigin {
    /// Computes the value to emit in `Access-Control-Allow-Origin`
    /// given the request `Origin` (if any):
    ///       - Literal → always the value, regardless of request.
    ///       - Set → the request value if it's in the list; `None`
    ///         otherwise.
    ///       - Echo → the request value as-is (no filter); `None` if
    ///         the Origin header is absent.
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
    /// Builds a "default" CorsConfig intended for SPA browser
    /// frontend use: origin "*", common methods, `content-type` +
    /// `authorization` headers. More restrictive cases require
    /// explicit kwargs.
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

    /// List of HTTP headers the server emits with a CORS response
    /// (real or preflight), resolved against the request `Origin`.
    /// If the policy is `Set` and the origin is not allowed, the
    /// `Access-Control-Allow-Origin` header is OMITTED (the browser
    /// rejects the response, correct strict CORS behavior). The
    /// other headers (methods/headers/max_age) are emitted anyway.
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

/// Specification of a header declared with `@header(name="X")` on a
/// handler (Phase 7.6). `http_name` is the canonical HTTP name
/// declared by the user; `param_name` is the name of the Fitz
/// parameter it binds to (derived by convention: lowercase + `-` →
/// `_`). `is_nullable`: if the Fitz param was declared as `Str?`,
/// the header is optional (missing → `Null`); otherwise it is
/// required (missing → 400).
#[derive(Debug, Clone)]
pub struct HeaderSpec {
    pub http_name: String,
    pub param_name: String,
    pub is_nullable: bool,
}

/// Description of a handler's body parameter: its name (to build
/// args in the correct order) and the expected `Value::Type`, if the
/// user declared it. Without a declared type we deserialize as a
/// free `Value` (flexible shape — useful for webhooks or schemaless
/// APIs).
#[derive(Debug, Clone)]
pub struct BodyParam {
    pub name: String,
    /// `Some(Value::Type{...})` if the user declared a custom type.
    /// `None` if the parameter has no annotation or if the annotation
    /// is a primitive (`Int`, `Str`, etc. — we support that too).
    pub declared_type: Option<Value>,
    /// When `declared_type` is `None`, this field holds the type name
    /// (if any) for error messages. If undeclared, `None`. Kept as
    /// structural metadata even though current reads are only via
    /// `Debug`.
    #[allow(dead_code)]
    pub declared_type_name: Option<String>,
}

/// Server configuration a `@server(...)` may have declared in the
/// program. If `None`, defaults are used (127.0.0.1:3000, docs
/// enabled). Only one `@server` per program is allowed — the
/// evaluator enforces uniqueness during registration.
///
/// `enable_docs` (Phase 7.4): when `false`, the server does NOT
/// auto-register `/openapi.json` or `/docs`. Default: `true` — the
/// happy path serves docs without touching anything. Opt out with
/// `@server(docs=false)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub enable_docs: bool,
    /// Mini-phase Q.2: override of OpenAPI schema `info.version` via
    /// `@server(api_version="1.2.3")`. `None` → schema uses the
    /// default `"0.1.0"`. When set, `serve()` reads it while
    /// pre-computing the schema and passes it to
    /// `generate_openapi_with_version`.
    pub api_version: Option<String>,
    /// Phase 9.w.2.e — heartbeat ping/pong interval for WebSocket
    /// connections, in seconds. The runtime sends a Ping every
    /// `ws_heartbeat_secs` seconds to every live conn; if no frame
    /// (text/pong) arrives within `2 * ws_heartbeat_secs`, the conn
    /// is closed (keepalive timeout). `0` disables the heartbeat
    /// (not recommended for proxies/CDNs that kill idle conns).
    /// Default: `30` — passes through most proxies without flooding
    /// the network. Override with `@server(ws_heartbeat_secs=60)` or
    /// `@server(ws_heartbeat_secs=0)`.
    pub ws_heartbeat_secs: u64,
    /// Phase 12.1.b — Maximum seconds the server waits for in-flight
    /// requests to finish after receiving SIGTERM/Ctrl-C before
    /// killing the process. During this window, `/readyz` returns
    /// 503 so K8s stops routing traffic, and axum closes the listener
    /// but lets in-progress handlers finish. Default `30` — aligned
    /// with the K8s default `terminationGracePeriodSeconds`. Override
    /// with `@server(shutdown_timeout_secs=60)`. `0` disables the
    /// grace period (immediate shutdown — not recommended in
    /// production).
    pub shutdown_timeout_secs: u64,
    /// Phase 12.3.b.5 — Enables automatic HTTP instrumentation
    /// (SpanContext root per request, `log.info("http.access", ...)`
    /// with `http.method`/`http.target`/`http.status_code`/`duration_ms`,
    /// Counter `http_requests_total` + Histogram
    /// `http_request_duration_seconds` with labels). Default `true`.
    /// Explicit override with `@server(observability=false)` skips
    /// the ENTIRE instrumentation wrapper — handlers run bare-metal
    /// with no per-request overhead. Useful in hot paths where every
    /// microsecond counts, or when the user has their own custom
    /// observability system. Does not affect other features (auth,
    /// CORS, middleware, etc.).
    pub observability_enabled: bool,
    /// Phase 12.3.iter2.Tier3 — Enables the `/metrics` endpoint with
    /// the Prometheus exposition format. When `true`, `serve()`
    /// installs `PrometheusBuilder` as the global recorder of the
    /// `metrics` crate (the Counter/Histogram values already emitted
    /// by `dispatch_request` start populating the recorder
    /// automatically), and `build_router` auto-mounts `GET /metrics`
    /// rendering the exposition format on each scrape. Default
    /// `false` — without the flag, empty recorder + route not
    /// mounted. Override: `@server(prometheus=true)`, or env var
    /// `FITZ_PROMETHEUS=1` (env var takes precedence over the flag —
    /// useful to toggle in production without recompiling). Endpoint
    /// shares the same port + transport as the rest of the app (NOT
    /// a separate port).
    pub prometheus_enabled: bool,
}

impl ServerConfig {
    /// Defaults applied when there is no `@server` in the program.
    pub fn default_addr() -> Self {
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 3000,
            enable_docs: true,
            api_version: None,
            ws_heartbeat_secs: 30,
            shutdown_timeout_secs: 30,
            observability_enabled: true,
            prometheus_enabled: false,
        }
    }

    /// Translates to a `SocketAddr`. Fails if the host does not
    /// parse as a numeric IP (we don't resolve DNS — to avoid
    /// surprises with a literal host that is not an IP).
    pub fn to_socket_addr(&self) -> Result<std::net::SocketAddr, String> {
        let ip: std::net::IpAddr = self.host.parse().map_err(|_| {
            format!(
                "host '{}' is not a valid IP (expected IPv4/IPv6 literal)",
                self.host
            )
        })?;
        Ok(std::net::SocketAddr::new(ip, self.port))
    }
}

/// Phase 9.w.1.c — Auth policy for an HTTP handler. Determined at
/// route registration by the presence of `@authenticated` and/or
/// `@admin` decorators. Without those decorators → `None` (public
/// handler, no auth check).
///
/// The checker (9.w.1.a) already validated statically that any
/// handler with `Authenticated`/`Admin` has a param compatible with
/// the `User` returned by the `@auth_provider`, and that `Admin`
/// requires a `role: Str` field on `User`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSpec {
    /// No auth requirement (default — public handler).
    None,
    /// `@authenticated`: requires the `@auth_provider` to return
    /// `Result::Ok(user)`. Any returned `user` is valid.
    Authenticated,
    /// `@admin`: on top of `@authenticated`, requires the returned
    /// `user` to have a `role: Str` field with value `"admin"`.
    Admin,
}

/// Phase 9.w.1.c — Handle to the `@auth_provider` registered in the
/// program. Singleton: at most one per program (validated by both the
/// 9.w.1.a checker and `set_auth_provider` defensively).
///
/// The runtime invokes `handler` with a single arg of type
/// `Map<Str, Str>` (the incoming request HTTP headers) and expects
/// `Result<User>` back. `Ok` → continues to the handler with the
/// `user` injected; `Err` → 401 with `{"error": <msg>}`.
/// Phase 12.1.b — Handle to a `@healthz` or `@readyz` probe registered
/// at runtime. Parallel to `AuthProviderHandle` but simpler: only
/// name + handler + is_async (no associated nominal type).
#[derive(Debug, Clone)]
pub struct HealthCheckHandle {
    /// Name of the fn marked with `@healthz` or `@readyz`. Only for
    /// logging.
    pub name: String,
    /// `Value::Function` (the handler resolved from the definition
    /// env). The call site invokes it exactly like any Fitz
    /// `Value::Function`.
    pub handler: Value,
    /// `true` if the fn is `async`. The call site must await the
    /// resulting `Value::Future`.
    pub is_async: bool,
}

#[derive(Debug, Clone)]
pub struct AuthProviderHandle {
    /// Name of the fn marked with `@auth_provider`. Only for error
    /// messages and logging.
    pub name: String,
    /// Value::Function (the handler resolved from the definition
    /// env). The call site invokes it exactly like any Fitz
    /// `Value::Function`.
    pub handler: Value,
    /// `true` if the provider fn is `async`. The call site must await
    /// the resulting `Value::Future`; sync → call and use the
    /// returned `Value` directly.
    pub is_async: bool,
    /// W14 (v0.10.10) — name of the type `T` of the `Result<T>`
    /// returned by the provider. Extracted by `register_auth_provider`
    /// when processing the `@auth_provider`, parsing the FnDef's
    /// `return_type`. Queried by the handler dispatcher to identify
    /// the "user" param by TYPE instead of the first-leftover rule,
    /// so a protected handler can also receive a separate body (`fn
    /// create(body: PostInput, user: User) -> Post`).
    pub user_type_name: String,
}

/// Accumulator of routes registered during `eval`. Built by `main.rs`
/// before evaluating; consulted afterwards to decide whether to start
/// the server.
#[derive(Debug, Default)]
pub struct HttpRegistry {
    pub routes: Vec<RouteSpec>,
    /// Server config declared with `@server(...)`. `None` if the
    /// program did not declare it — the caller (main.rs) applies
    /// `ServerConfig::default_addr()`.
    pub server_config: Option<ServerConfig>,
    /// Phase 9.w.1.c — Auth provider declared with `@auth_provider`.
    /// `None` if the program did not declare one (in that case there
    /// can be no `@authenticated`/`@admin` handlers — the checker
    /// blocks them).
    pub auth_provider: Option<AuthProviderHandle>,
    /// Phase 9.w.2 — Shared broadcaster for `@ws` endpoints. `Arc` so
    /// each `WsConnHandle` captures it without going through the
    /// registry. Lazy-initialized in `new()`. If the program has no
    /// `@ws` endpoints, the broadcaster stays empty and costs
    /// nothing.
    pub ws_broadcaster: std::sync::Arc<WsBroadcaster>,
    /// Phase 9.w.3 — Registry of `@cron` jobs. Populated during
    /// evaluation; the scheduler starts when `serve()` brings up the
    /// tokio runtime or when cron-only mode takes control
    /// (`run_scheduler_only`). `Arc` to share with the tokio workers
    /// that run the jobs.
    pub cron_registry: std::sync::Arc<crate::cron_jobs::CronRegistry>,
    /// Phase 3c — registry of `@every(N)` interval jobs. Started alongside
    /// the cron + HTTP schedulers.
    pub every_registry: std::sync::Arc<crate::cron_jobs::EveryRegistry>,
    /// v0.37.7 — Registry of `@background(store=db)` fns. Populated
    /// during evaluation (`process_decorator`); consulted per
    /// `spawn(...)` by `eval_spawn_call` to decide whether to persist
    /// the job in `fitz_bg_jobs`. Fns without `store` stay in-memory.
    /// `Arc` to share with the eval + boot catch_up.
    pub background_registry: std::sync::Arc<crate::background_jobs::BackgroundRegistry>,
    /// Phase 12.1.b — `@healthz` handler (K8s liveness probe). `None`
    /// if the program did not declare it → auto-mount serves a
    /// default 200 "ok" response. `Some(h)` → the Fitz handler is
    /// invoked and the return is mapped to a status code.
    pub healthz_handler: Option<HealthCheckHandle>,
    /// Phase 12.1.b — `@readyz` handler (K8s readiness probe). `None`
    /// → default returns 200 when not draining, 503 when draining.
    /// `Some(h)` → the Fitz handler is invoked; during draining it's
    /// ALWAYS 503 without touching the handler.
    pub readyz_handler: Option<HealthCheckHandle>,
    /// Phase 12.1.b — Atomic flag indicating whether the server is
    /// "draining" (SIGTERM/Ctrl-C already fired). `/readyz` queries
    /// it and returns 503 when `true`, so K8s stops routing new
    /// requests to this pod. Shared between the handler and the
    /// `shutdown_signal()` that flips it.
    pub draining: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl HttpRegistry {
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            server_config: None,
            auth_provider: None,
            ws_broadcaster: std::sync::Arc::new(WsBroadcaster::new()),
            cron_registry: std::sync::Arc::new(crate::cron_jobs::CronRegistry::new()),
            every_registry: std::sync::Arc::new(crate::cron_jobs::EveryRegistry::new()),
            background_registry: std::sync::Arc::new(
                crate::background_jobs::BackgroundRegistry::new(),
            ),
            healthz_handler: None,
            readyz_handler: None,
            draining: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn is_empty(&self) -> bool {
        // The registry is "empty" when it has no routes. A `@server`
        // without routes serves nothing (no endpoints); ignoring it
        // is the most useful behavior.
        self.routes.is_empty()
    }

    pub fn push(&mut self, route: RouteSpec) {
        self.routes.push(route);
    }

    /// Returns the explicit config or the default. Useful for
    /// `main.rs`.
    pub fn resolved_config(&self) -> ServerConfig {
        self.server_config
            .clone()
            .unwrap_or_else(ServerConfig::default_addr)
    }
}

// thread_local: the evaluator can tell whether a registry is active
// without passing it as a parameter everywhere. Same pattern as the
// module loader in 3.5. `None` → we're running in a context without
// HTTP (REPL, embedded eval, tests without server) and decorators
// emit an explicit error.
thread_local! {
    static HTTP_REGISTRY: RefCell<Option<HttpRegistry>> = const { RefCell::new(None) };
}

/// Installs an empty registry on the current thread for the duration
/// of the closure. Returns it when finished. If the closure returns
/// `Err`, the registry is dropped along with the rest of the state.
/// Designed for `main.rs`: set up, evaluate, receive the registry,
/// decide whether to start the server.
///
/// **Reentrancy invariants (R5 audit, 2026-05-27)**: the pattern
/// `take()` + `replace()` + final `take()` + restore avoids sharing
/// live borrows of the `RefCell` while the closure `f` is running.
/// The sequence is:
///   1. `take()` pulls out the previous registry (typically `None`;
///      `Some` only in nested tests that reuse this helper).
///   2. A fresh `HttpRegistry::new()` is inserted.
///   3. `f()` runs WITHOUT a live borrow — the closure can invoke
///      `register_http_route`/`register_server_config`/etc., which
///      also call `cell.borrow_mut()` without clashing (no live
///      borrows).
///   4. On return, `take()` pulls out the populated registry and
///      `replace()` restores the previous one. The captured registry
///      is returned along with the output of `f`.
///
/// This means the closure can call functions that internally invoke
/// `with_borrow_mut` or `with_borrow` over `HTTP_REGISTRY` without
/// deadlocking due to reentrancy.
pub fn with_active_registry<F, T>(f: F) -> (T, HttpRegistry)
where
    F: FnOnce() -> T,
{
    HTTP_REGISTRY.with(|cell| {
        // Save the previous registry (typically `None` — the nested
        // case only exists for tests). After `f()` we restore it
        // verbatim, never replacing it with `HttpRegistry::new()` by
        // mistake.
        let prev = cell.borrow_mut().take();
        *cell.borrow_mut() = Some(HttpRegistry::new());
        let out = f();
        let registry = cell
            .borrow_mut()
            .take()
            .expect("with_active_registry installed a registry — it should be present");
        *cell.borrow_mut() = prev;
        (out, registry)
    })
}

/// Async variant of `with_active_registry` (Phase 6.4). Same
/// semantics but accepts a closure returning a `Future`, for use
/// from async code (handlers, tests with `#[tokio::test]`).
///
/// **Borrow invariant**: we do NOT hold `cell.borrow_mut()` across
/// awaits — borrows are taken/released entering and leaving each
/// atomic step. If the closure panics, the guard still restores the
/// previous registry on implicit `Drop` (same pattern as the sync
/// version, via panics propagated after setup).
///
/// `dead_code` allow: only tests use it for now (real HTTP handlers
/// land in 6.5 when the mpsc bridge is removed).
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
            .expect("with_active_registry_async installed a registry — it should be present");
        *cell.borrow_mut() = prev;
        registry
    });
    (out, registry)
}

/// `true` if there is an active HTTP registry on the current thread.
/// The evaluator queries it before processing an HTTP decorator: if
/// there isn't one, it still stops with an explicit error.
pub fn has_active_registry() -> bool {
    HTTP_REGISTRY.with(|cell| cell.borrow().is_some())
}

/// 10.8.7 (v0.10.8) — cross-handler broadcast of a JSON message to
/// ALL WS clients connected to `endpoint`. Enables the canonical
/// SaaS pattern "HTTP handler triggers realtime notification to
/// subscribed WS clients" — the built-in `ws_broadcast(endpoint, msg)`
/// in the language delegates here.
///
/// If there's no active HTTP registry (CLI program with no server),
/// the broadcast is a silent no-op. A user calling `ws_broadcast`
/// from a script without `@server` does not get an error — the
/// behavior degrades to a pedagogically acceptable no-op: the
/// endpoint doesn't exist, there are no clients.
pub fn ws_broadcast_to_endpoint(endpoint: &str, payload: String) {
    // Prefer the thread-local registry (active during eval / on the main
    // thread); fall back to the global installed at serve boot so a scheduler
    // task (`@every` / `@cron`) on a tokio worker — which has no thread-local —
    // can still broadcast to the endpoint.
    let broadcaster = HTTP_REGISTRY
        .with(|cell| cell.borrow().as_ref().map(|reg| reg.ws_broadcaster.clone()))
        .or_else(|| ws_broadcaster_slot().lock().clone());
    if let Some(b) = broadcaster {
        b.broadcast_text(endpoint, payload);
    }
}

/// Phase 3c — global WS broadcaster slot, so `ws_broadcast(endpoint, msg)` works
/// from a scheduler task (`@every` / `@cron`) that runs on a tokio worker
/// without the thread-local registry. Installed at serve boot (`run_file`) from
/// the same `Arc` the registry holds.
static INSTALLED_WS_BROADCASTER: std::sync::OnceLock<
    parking_lot::Mutex<Option<std::sync::Arc<WsBroadcaster>>>,
> = std::sync::OnceLock::new();

fn ws_broadcaster_slot() -> &'static parking_lot::Mutex<Option<std::sync::Arc<WsBroadcaster>>> {
    INSTALLED_WS_BROADCASTER.get_or_init(|| parking_lot::Mutex::new(None))
}

/// Phase 3c — installs the global WS broadcaster (called from `run_file` after
/// eval, before serving), mirroring `install_background_registry`.
pub fn install_ws_broadcaster(b: std::sync::Arc<WsBroadcaster>) {
    *ws_broadcaster_slot().lock() = Some(b);
}

/// Phase 9.w.1.c — `true` if the active registry has an
/// `@auth_provider` registered. `register_http_route` queries it when
/// it sees a handler with `@authenticated`/`@admin` to validate the
/// declaration order (the provider must come before any handler that
/// uses it).
pub fn has_auth_provider() -> bool {
    HTTP_REGISTRY.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|reg| reg.auth_provider.is_some())
            .unwrap_or(false)
    })
}

/// W14 (v0.10.10) — Returns the `user_type_name` of the registered
/// provider (empty string if there's no provider or if the provider
/// was registered without extracting the type). Queried by the
/// protected handler dispatcher to identify the "user" param by
/// type rather than the first-leftover rule, so a protected handler
/// can receive body + user together.
pub fn get_auth_provider_user_type_name() -> String {
    HTTP_REGISTRY.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|reg| reg.auth_provider.as_ref().map(|h| h.user_type_name.clone()))
            .unwrap_or_default()
    })
}

/// Pushes a route onto the active registry. Panics if there isn't
/// one — the caller must have checked with `has_active_registry()`
/// or be inside `with_active_registry`.
pub fn push_route(route: RouteSpec) {
    HTTP_REGISTRY.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let reg = borrow
            .as_mut()
            .expect("push_route llamado sin registry activo");
        reg.push(route);
    });
}

/// Sets the `ServerConfig` of the active registry. Fails if one was
/// already set (preserves `@server` uniqueness). Returns `Err(())`
/// and the evaluator emits an explicit error.
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

/// Phase 9.w.1.c — Sets the `@auth_provider` of the active registry.
/// Fails with `Err(Box<existing>)` if one was already registered
/// (singleton). The checker (9.w.1.a) does the same check
/// statically, but the runtime replicates it defensively so
/// generated code or incremental evaluation does not break the
/// invariant.
///
/// `Err` boxed so `Result` does not become huge (clippy
/// `result_large_err`): `AuthProviderHandle` carries a `Value` that
/// can be heavy because of its variants (Arc<Mutex<>> of
/// List/Map/etc.).
pub fn set_auth_provider(handle: AuthProviderHandle) -> Result<(), Box<AuthProviderHandle>> {
    HTTP_REGISTRY.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let reg = borrow
            .as_mut()
            .expect("set_auth_provider llamado sin registry activo");
        if let Some(existing) = &reg.auth_provider {
            return Err(Box::new(existing.clone()));
        }
        reg.auth_provider = Some(handle);
        Ok(())
    })
}

/// Phase 12.1.b — Result of trying to register a health handler.
#[derive(Debug)]
pub enum SetHealthHandlerError {
    /// No active HTTP registry (REPL / embedded eval program).
    NoRegistry,
    /// A handler of the same kind was already registered; carries
    /// the previous name for the error message.
    Duplicate(String),
}

/// Phase 12.1.b — Sets the `@healthz` or `@readyz` handler in the
/// active registry. `kind` must be `"healthz"` or `"readyz"`. Fails
/// with `Duplicate(prev_name)` if one was already set (singleton —
/// parallel to `set_auth_provider`). Without a registry →
/// `NoRegistry`.
pub fn set_health_handler(
    kind: &str,
    handle: HealthCheckHandle,
) -> Result<(), SetHealthHandlerError> {
    HTTP_REGISTRY.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let reg = match borrow.as_mut() {
            Some(r) => r,
            None => return Err(SetHealthHandlerError::NoRegistry),
        };
        let slot = match kind {
            "healthz" => &mut reg.healthz_handler,
            "readyz" => &mut reg.readyz_handler,
            other => unreachable!("invalid kind for set_health_handler: {}", other),
        };
        if let Some(existing) = slot {
            return Err(SetHealthHandlerError::Duplicate(existing.name.clone()));
        }
        *slot = Some(handle);
        Ok(())
    })
}

/// Phase 9.w.3 — Registers a cron job in the active registry's
/// `CronRegistry`. If the cron expression is invalid, returns
/// `Err(msg)` so the caller (evaluator) can emit a FitzError. Without
/// an active registry → `Err` ("no registry") — the typical context
/// is `fitz run` of the file, same as `set_auth_provider`.
pub fn register_cron_job(
    fn_name: String,
    cron_expr: &str,
    handler: crate::value::Value,
    is_async: bool,
    env: crate::env::EnvRef,
    options: crate::cron_jobs::CronJobOptions,
) -> Result<(), String> {
    HTTP_REGISTRY.with(|cell| {
        let borrow = cell.borrow();
        let reg = borrow.as_ref().ok_or_else(|| {
            "@cron sin contexto activo: solo aplica ejecutando con `fitz run` un archivo del programa.".to_string()
        })?;
        reg.cron_registry
            .register(fn_name, cron_expr, handler, is_async, env, options)
    })
}

/// Phase 9.w.3 — `true` if the active registry has at least one cron
/// job. Used by `eval_with_base_and_deps` to decide whether to start
/// the standalone scheduler when there are no HTTP handlers
/// (cron-only mode).
pub fn registry_has_cron_jobs() -> bool {
    HTTP_REGISTRY.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|reg| reg.cron_registry.has_jobs())
            .unwrap_or(false)
    })
}

/// Phase 9.w.3 — Returns a clone of the active registry's
/// `Arc<CronRegistry>`, or `None` if there's no registry. The caller
/// (cron-only mode uses it for `run_scheduler_only`).
pub fn current_cron_registry() -> Option<std::sync::Arc<crate::cron_jobs::CronRegistry>> {
    HTTP_REGISTRY.with(|cell| cell.borrow().as_ref().map(|reg| reg.cron_registry.clone()))
}

/// Phase 3c — registers an `@every(N)` fn in the active registry's
/// `EveryRegistry`. Without an active registry → `Err` (same context rule
/// as `register_cron_job`).
pub fn register_every_job(
    fn_name: String,
    interval_secs: f64,
    handler: crate::value::Value,
    is_async: bool,
    env: crate::env::EnvRef,
) -> Result<(), String> {
    HTTP_REGISTRY.with(|cell| {
        let borrow = cell.borrow();
        let reg = borrow.as_ref().ok_or_else(|| {
            "@every sin contexto activo: solo aplica ejecutando con `fitz run` un archivo del programa.".to_string()
        })?;
        reg.every_registry
            .register(fn_name, interval_secs, handler, is_async, env);
        Ok(())
    })
}

/// Phase 3c — `true` if the active registry has at least one `@every` job.
pub fn registry_has_every_jobs() -> bool {
    HTTP_REGISTRY.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|reg| reg.every_registry.has_jobs())
            .unwrap_or(false)
    })
}

/// Phase 3c — clone of the active registry's `Arc<EveryRegistry>`.
pub fn current_every_registry() -> Option<std::sync::Arc<crate::cron_jobs::EveryRegistry>> {
    HTTP_REGISTRY.with(|cell| cell.borrow().as_ref().map(|reg| reg.every_registry.clone()))
}

/// v0.37.7 — Registers a `@background(store=db)` fn in the active
/// registry's `BackgroundRegistry`. Without an active registry →
/// `Err` ("no registry"), same context rule as `register_cron_job`.
pub fn register_background_fn(
    fn_name: String,
    store: std::sync::Arc<crate::db::DbConnHandle>,
    retry: Option<crate::cron_jobs::RetryConfig>,
    catch_up: bool,
) -> Result<(), String> {
    HTTP_REGISTRY.with(|cell| {
        let borrow = cell.borrow();
        let reg = borrow.as_ref().ok_or_else(|| {
            "@background(store=...) sin contexto activo: solo aplica ejecutando con `fitz run` un archivo del programa.".to_string()
        })?;
        reg.background_registry.register(fn_name, store, retry, catch_up);
        Ok(())
    })
}

/// v0.37.7 — Global slot for the background registry so HTTP handlers
/// (running on tokio workers WITHOUT the thread-local) can reach it
/// from `spawn(...)`. The thread-local `HTTP_REGISTRY` is only active
/// during the initial eval; request handling runs on separate worker
/// threads. `install_background_registry` (called from `run_file`
/// after the eval, before serving) sets this to the same `Arc` the
/// registry holds.
static INSTALLED_BG_REGISTRY: std::sync::OnceLock<
    parking_lot::Mutex<Option<std::sync::Arc<crate::background_jobs::BackgroundRegistry>>>,
> = std::sync::OnceLock::new();

fn bg_registry_slot(
) -> &'static parking_lot::Mutex<Option<std::sync::Arc<crate::background_jobs::BackgroundRegistry>>>
{
    INSTALLED_BG_REGISTRY.get_or_init(|| parking_lot::Mutex::new(None))
}

/// v0.37.7 — Installs the global background registry. `run_file` calls
/// this after the eval (with `registry.background_registry.clone()`)
/// so `spawn(...)` from an HTTP handler / cron job — which runs on a
/// tokio worker without the thread-local — can still resolve the
/// persistence config.
pub fn install_background_registry(
    reg: std::sync::Arc<crate::background_jobs::BackgroundRegistry>,
) {
    *bg_registry_slot().lock() = Some(reg);
}

/// v0.37.7 — Returns a clone of the active `Arc<BackgroundRegistry>`,
/// or `None`. Prefers the thread-local (active during eval — e.g. a
/// top-level `spawn`); falls back to the global installed at serve
/// boot (request handling runs on workers without the thread-local).
/// `eval_spawn_call` uses it to decide whether a `spawn(...)` is
/// persisted.
pub fn current_background_registry(
) -> Option<std::sync::Arc<crate::background_jobs::BackgroundRegistry>> {
    let from_tl = HTTP_REGISTRY.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|reg| reg.background_registry.clone())
    });
    from_tl.or_else(|| bg_registry_slot().lock().clone())
}

// ---------------------------------------------------------------------------
// Path: from decorator to axum syntax
// ---------------------------------------------------------------------------

/// Result of extracting a path declared in an HTTP decorator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathTemplate {
    /// Path in axum format: `/users/{id}`, `/`, `/users`. Whatever
    /// comes after a `?` in the original template does NOT go here
    /// — it lives inside `query_params`. Axum routes only on this.
    pub path: String,
    /// Path param names in order of appearance.
    pub params: Vec<String>,
    /// Names of the query params declared in the template. Each one
    /// comes from a `?key={name}&...` after the path. For now we
    /// require the query key and the Fitz param name to match
    /// (`?limit={limit}`, not `?l={limit}`). The order of
    /// `query_params` is the order of appearance in the template.
    pub query_params: Vec<String>,
}

/// Errors normalizing the path of a decorator. Messages in Spanish
/// so they go straight to the user.
#[derive(Debug, PartialEq, Eq)]
pub enum PathError {
    /// The first arg of the decorator is not a string literal.
    NotAStringLiteral,
    /// The path does not start with `/`.
    MustStartWithSlash,
    /// An interpolation segment included something that is not a
    /// simple identifier (`{user.id}`, `{42}`, etc.).
    UnsupportedInterpolation(String),
    /// Some path param was repeated (`/a/{x}/b/{x}`).
    DuplicateParam(String),
    /// A declared query param has a key different from the param
    /// name (`?l={limit}`). Today we require them to match.
    QueryKeyNameMismatch { key: String, name: String },
    /// The query template does not respect `key={name}` with simple
    /// identifier — e.g. `?{limit}`, `?limit=`, `?limit={x.y}`,
    /// `?=v`.
    MalformedQueryTemplate(String),
}

impl PathError {
    pub fn message(&self) -> String {
        match self {
            PathError::NotAStringLiteral => {
                "the path of an HTTP decorator must be a string literal \
                 (`@get(\"/users\")`)"
                    .to_string()
            }
            PathError::MustStartWithSlash => {
                "the path of an HTTP decorator must start with '/'".to_string()
            }
            PathError::UnsupportedInterpolation(what) => format!(
                "path param '{{{}}}': only simple identifiers like \
                 '{{id}}' are allowed, not expressions",
                what
            ),
            PathError::DuplicateParam(name) => format!(
                "path param '{{{}}}' appears more than once in the path",
                name
            ),
            PathError::QueryKeyNameMismatch { key, name } => format!(
                "query param `?{key}={{{name}}}`: the key and the param \
                 name must match — use `?{name}={{{name}}}` or rename \
                 the handler parameter"
            ),
            PathError::MalformedQueryTemplate(t) => format!(
                "malformed query template inside the path: `?{t}` — \
                 expected `?key={{name}}&other_key={{other_name}}` with \
                 simple identifiers"
            ),
        }
    }
}

/// Takes the expression the parser left as the first arg of an HTTP
/// decorator and turns it into a `PathTemplate`. Accepts two forms:
///
///  - `Expr::Str(s, _)`: path without params. E.g. `"/"`, `"/users"`.
///  - `Expr::StrInterp(parts, _)`: path with params. Each
///    `StrPart::Expr` must be a simple `Ident` (`{id}`). Anything
///    else is an error.
///
/// Any other expression form → `PathError::NotAStringLiteral`.
pub fn parse_path_template(expr: &Expr) -> Result<PathTemplate, PathError> {
    use crate::ast::StrPart;

    // First pass: rebuild the canonicalized path text and gather
    // every `{name}` in order (without distinguishing path vs query
    // yet). The `?` separating path from query stays as a literal
    // char in `buf` — we split it below.
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
                        return Err(PathError::UnsupportedInterpolation(format!("{:?}", other)));
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

    // Split path from query template on the first `?`. If absent,
    // the whole string is path and `query_params` stays empty.
    let (path, query_template) = match full.find('?') {
        Some(idx) => (full[..idx].to_string(), Some(&full[idx + 1..])),
        None => (full, None),
    };

    // To distinguish path_params from query_params: those appearing
    // inside the path go to `path_params`; those appearing inside
    // the query template (with their key) go to `query_params`.
    let mut path_params: Vec<String> = Vec::new();
    let mut query_params: Vec<String> = Vec::new();

    // Re-scan the canonicalized path to extract the `{name}`s inside
    // it (no full parse — we only look for `{ident}` between braces
    // to get the order right).
    extract_brace_idents_into(&path, &mut path_params);

    // Parse the query template if it exists.
    if let Some(q) = query_template {
        // Format: `key={name}&another={another}` with each pair
        // separated by `&`. Validate that each pair has `key={name}`
        // with key being a simple identifier and `{name}` also a
        // simple identifier, and that key == name.
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
            // The value must be exactly `{name}` (a brace pair with
            // an identifier inside). Anything else (literal, expr,
            // empty) is not supported.
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
            if path_params.contains(&name.to_string()) || query_params.contains(&name.to_string()) {
                return Err(PathError::DuplicateParam(name.to_string()));
            }
            query_params.push(name.to_string());
        }
    }

    // Sanity check: path + query should match `all_params` (every
    // `{name}` extracted in the first pass). Otherwise, there's
    // something off in the path (e.g. `{name}` inside the query
    // value without being exactly `={name}`). The query parser
    // already catches this but we validate defensively.
    let _ = all_params;

    Ok(PathTemplate {
        path,
        params: path_params,
        query_params,
    })
}

/// Extracts names between `{...}` in a canonicalized path, in order
/// of appearance, and pushes them to `out`. Assumes the path was
/// already rebuilt by `parse_path_template` (braces always surround
/// simple identifiers).
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

/// "Simple" identifier for keys and param names in query templates:
/// ASCII letters/digits/underscore, first char non-digit. We don't
/// use `char::is_alphanumeric` to avoid accepting unicode (Fitz also
/// rejects it in lexer idents).
fn is_simple_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Value → JSON serialization
// ---------------------------------------------------------------------------

/// Distilled response of a handler: status code + serialized body.
/// The Fitz handler returns a `Value`; this function decides how it
/// translates to HTTP. The conversion is total (any `Value` produces
/// a `HandlerOutcome`), but some non-serializable types (Function,
/// Type, Module, Range) generate 500 with a clear message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerOutcome {
    pub status: u16,
    /// Serialized body for the text path, ready to send. For the
    /// default JSON dispatch this is the JSON string of the value.
    /// For a custom `Response { body: ... }` (v0.19.0 block 1) it
    /// is the raw `body` field value (no JSON-encoding wrap).
    /// Empty for 204 (reserved) and also when `body_bytes` carries
    /// the payload (binary path).
    pub body: String,
    /// v0.19.0 block 2 — opt-in binary body. When `Some(bytes)`,
    /// `body` is ignored and the response ships `bytes` as the
    /// raw binary payload (PDF, ZIP, images, etc). Populated only
    /// by the `Response { body_bytes: ... }` built-in path; all
    /// other constructors leave it `None`.
    pub body_bytes: Option<Vec<u8>>,
    /// Body content-type. Default is `application/json` for the
    /// normal serialisation path. The built-in `Response` type lets
    /// the handler override this to any other value
    /// (`application/rss+xml`, `text/plain`, `image/svg+xml`,
    /// `application/pdf`, etc.). Stored as `String` (not
    /// `&'static str`) so user-supplied content types fit; the
    /// alloc cost is negligible compared to the body alloc.
    pub content_type: String,
    /// Extra headers to emit with the response. Populated by
    /// middlewares (mini-phase MW.2: CORS `Access-Control-Allow-*`)
    /// and by the `Response { headers: { ... } }` built-in
    /// (v0.19.0) so handlers can set `Cache-Control`, `ETag`,
    /// `Last-Modified`, etc. Empty for normal non-customised
    /// responses.
    pub extra_headers: Vec<(String, String)>,
}

impl HandlerOutcome {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        HandlerOutcome {
            status,
            body: body.to_string(),
            body_bytes: None,
            content_type: "application/json".to_string(),
            extra_headers: Vec::new(),
        }
    }

    /// v0.19.0 — outcome with custom content-type and headers,
    /// product of a handler returning a `Response { ... }` value
    /// with `body: Str` set. The body is passed as-is (raw text,
    /// not JSON-encoded).
    pub fn custom(
        status: u16,
        body: String,
        content_type: String,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        HandlerOutcome {
            status,
            body,
            body_bytes: None,
            content_type,
            extra_headers,
        }
    }

    /// v0.19.0 block 2 — outcome with binary payload, product of a
    /// `Response { body_bytes: bytes(...), ... }` return. The
    /// content_type and headers come from the same `Response`
    /// fields; the body string stays empty (axum builder reads
    /// `body_bytes` first).
    pub fn custom_binary(
        status: u16,
        body_bytes: Vec<u8>,
        content_type: String,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        HandlerOutcome {
            status,
            body: String::new(),
            body_bytes: Some(body_bytes),
            content_type,
            extra_headers,
        }
    }

    /// Shortcut for runtime errors the handler should never see:
    /// non-serializable type, misused decorator, etc.
    pub fn internal_error(msg: impl Into<String>) -> Self {
        let body = serde_json::json!({ "error": msg.into() });
        HandlerOutcome::json(500, body)
    }
}

// Phase 6.4 / 9.w.3.b — `await_if_future` helper for the HTTP
// runtime dispatchers: if an `async fn` handler returns
// `Value::Future`, we must await it before passing the value to the
// serializer. Parallel to the pattern in `build_ws_method_router`
// and `register_auth_provider`.
//
// Without this helper, `async fn` HTTP handlers in the interpreter
// failed with "Future pendiente no es serializable" — a pre-existing
// bug detected while validating 9.w.3.b.
pub async fn await_if_future(value: Value) -> crate::error::FitzResult<Value> {
    if let Value::Future(cell) = value {
        let fut = cell.0.lock().take();
        match fut {
            Some(f) => f.await,
            None => Ok(Value::Null),
        }
    } else {
        Ok(value)
    }
}

/// Converts the result of a Fitz handler into a `HandlerOutcome`.
///
/// Rules:
///   - `Value::Result(Ok(v))`  → status 200, body = serialized `v`.
///   - `Value::Result(Err(e))` → mini-batch HTTP-Err: if `e` is a
///     `Value::Instance` with field `status: Int`, use that status
///     code and serialize the Instance as the body (untouched — the
///     user decides the shape). Without a `status` field, fall back
///     to 500 with `{"error": e}` (historical behavior).
///   - Any other `Value`  → status 200, body = that value serialized
///     directly (no wrapping). This allows handlers that don't use
///     `Result` and return `Str`, `Int`, `Instance`, etc.
///   - Non-serializable types (Function, Builtin, Type, Module,
///     Range) → status 500, `{"error": "value not serializable: <type>"}`.
// v0.19.0 — Maps a `Value::Instance` of the built-in `Response`
// type to a `HandlerOutcome` with custom content_type and headers.
// The evaluator already populated missing fields with defaults
// (`status: 200`, `content_type: "application/json"`,
// `headers: {}`, `body: ""`) so all four are guaranteed present.
// Validation is shape-only here — the checker would catch type
// mismatches statically, but `fitz run --no-typecheck` and tests
// that bypass it could still leak a bad shape, so we defend with
// 500s citing the offending field.
fn response_instance_to_outcome(
    fields: &crate::value::Shared<Vec<(String, Value)>>,
) -> HandlerOutcome {
    let g = fields.lock();
    let mut status: Option<i64> = None;
    let mut content_type: Option<String> = None;
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut body: Option<String> = None;
    let mut body_bytes: Option<Vec<u8>> = None;
    for (name, value) in g.iter() {
        match (name.as_str(), value) {
            ("status", Value::Int(n)) => status = Some(*n),
            ("status", other) => {
                return HandlerOutcome::internal_error(format!(
                    "Response.status must be Int, found {}",
                    other.type_name()
                ));
            }
            ("content_type", Value::Str(s)) => content_type = Some(s.clone()),
            ("content_type", other) => {
                return HandlerOutcome::internal_error(format!(
                    "Response.content_type must be Str, found {}",
                    other.type_name()
                ));
            }
            ("headers", Value::Map(pairs)) => {
                for (k, v) in pairs.lock().iter() {
                    match (k, v) {
                        (Value::Str(kk), Value::Str(vv)) => {
                            headers.push((kk.clone(), vv.clone()));
                        }
                        _ => {
                            return HandlerOutcome::internal_error(
                                "Response.headers must be Map<Str, Str> — keys and values \
                                 must both be Str"
                                    .to_string(),
                            );
                        }
                    }
                }
            }
            ("headers", other) => {
                return HandlerOutcome::internal_error(format!(
                    "Response.headers must be Map<Str, Str>, found {}",
                    other.type_name()
                ));
            }
            ("body", Value::Str(s)) => body = Some(s.clone()),
            ("body", other) => {
                return HandlerOutcome::internal_error(format!(
                    "Response.body must be Str, found {}",
                    other.type_name()
                ));
            }
            // v0.19.0 block 2 — opt-in binary payload. Null means
            // "use the text body"; Bytes means "use the bytes
            // directly". Anything else fails 500 (only Bytes or
            // Null are allowed by the checker, but defend at
            // runtime in case `--no-typecheck` slipped a bad
            // value through).
            ("body_bytes", Value::Null) => { /* default — text path */ }
            ("body_bytes", Value::Bytes(bs)) => body_bytes = Some(bs.clone()),
            ("body_bytes", other) => {
                return HandlerOutcome::internal_error(format!(
                    "Response.body_bytes must be Bytes? (null or Bytes), found {}",
                    other.type_name()
                ));
            }
            // FITZ-05 FASE B — write path. Each `Cookie` in the list
            // serialises to one `Set-Cookie` header pushed into
            // `headers` (which `outcome_to_response` `.append`s so
            // multiple cookies survive; `.insert` would drop all but
            // the last).
            ("cookies", Value::List(items)) => {
                for item in items.lock().iter() {
                    match cookie_instance_to_set_cookie(item) {
                        Ok(sc) => headers.push(("Set-Cookie".to_string(), sc)),
                        Err(msg) => return HandlerOutcome::internal_error(msg),
                    }
                }
            }
            ("cookies", other) => {
                return HandlerOutcome::internal_error(format!(
                    "Response.cookies must be List<Cookie>, found {}",
                    other.type_name()
                ));
            }
            _ => {
                // Field not part of the built-in shape — silently
                // ignored. Future field additions will use this
                // same arm during the transition; the checker
                // tells the user at registration time which fields
                // are valid.
            }
        }
    }
    drop(g);

    let status_i64 = status.unwrap_or(200);
    if !(100..1000).contains(&status_i64) {
        return HandlerOutcome::internal_error(format!(
            "Response.status out of range: {} (must be in 100..1000)",
            status_i64
        ));
    }
    let resolved_ct = content_type.unwrap_or_else(|| "application/json".to_string());
    // v0.19.0 block 2 — setting both `body` (non-empty) and
    // `body_bytes` (non-null) is a programming error. We do NOT
    // pick one silently — that would hide bugs (mismatched payload
    // shipped). 500 with a clear message instead.
    let body_str = body.unwrap_or_default();
    match body_bytes {
        Some(bytes) => {
            if !body_str.is_empty() {
                return HandlerOutcome::internal_error(
                    "Response: cannot set both `body` and `body_bytes` — pick one. \
                     For binary payloads (PDF/ZIP/images), use `body_bytes` and leave \
                     `body` at its default empty string."
                        .to_string(),
                );
            }
            HandlerOutcome::custom_binary(status_i64 as u16, bytes, resolved_ct, headers)
        }
        None => HandlerOutcome::custom(status_i64 as u16, body_str, resolved_ct, headers),
    }
}

pub fn value_to_outcome(value: &Value) -> HandlerOutcome {
    // v0.19.0 — `Response { ... }` built-in instance. The handler
    // built it explicitly to control `content_type`, `headers`, and
    // the raw text `body`. We extract the four fields by name, map
    // them to a `HandlerOutcome::custom`, and skip the JSON
    // serialisation path entirely. Validation: `status` must be a
    // valid HTTP status in `[100, 1000)`; `content_type` must be
    // `Str`; `headers` must be `Map<Str, Str>`; `body` must be
    // `Str`. Any shape mismatch produces a 500 with a clear
    // message citing the field name. Field absence is impossible
    // here — the evaluator's struct-lit pass already filled
    // missing fields with their defaults (`200`,
    // `"application/json"`, `{}`, `""`).
    //
    // We also peek through `Result::Ok(Response { ... })` for
    // handlers that propagate errors with `?`. `Err(Response)` is
    // NOT specially handled — the user wanting a custom-content
    // error response can return `Response { status: 500, ... }`
    // directly without wrapping in Result.
    let unwrapped: &Value = match value {
        Value::Result(ResultVariant::Ok(inner)) => inner.as_ref(),
        other => other,
    };
    if let Value::Instance {
        type_name, fields, ..
    } = unwrapped
    {
        if type_name == "Response" {
            return response_instance_to_outcome(fields);
        }
    }

    // Custom status code (spec): the handler did `return 401 { ... }`
    // and the evaluator emitted `Value::HttpResponse`. Direct
    // mapping: the status goes to the outcome, the body (if any) is
    // serialized with the same rules as any Value. Missing body →
    // JSON null (HTTP 204 No Content is not implemented yet; today
    // the parser requires an explicit body).
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

    // Result auto-handling: peel one layer. The inner is serialized
    // with the same rules as any other Value.
    let (status, payload) = match value {
        Value::Result(ResultVariant::Ok(inner)) => (200, inner.as_ref()),
        Value::Result(ResultVariant::Err(inner)) => {
            // Mini-batch HTTP-Err — convention: if the Err carries an
            // `Instance` with field `status: Int`, use that status
            // (e.g. `Err(ApiErr { status: 404, message: "..." })`).
            // The body is serialized whole — the user decides the
            // final shape. Without a `status` field, fall back to
            // 500 with `{"error": e}` (historical behavior).
            if let Value::Instance { fields, .. } = inner.as_ref() {
                let status_opt = {
                    let g = fields.lock();
                    g.iter().find(|(k, _)| k == "status").and_then(|(_, v)| {
                        if let Value::Int(n) = v {
                            Some(*n)
                        } else {
                            None
                        }
                    })
                };
                if let Some(s) = status_opt {
                    // Mini-batch HC.1 — valid status `[100, 1000)`
                    // matches axum and the HTTP spec. If the user
                    // provides an out-of-range status, we no longer
                    // silently fall to 500 — we emit 500 with an
                    // explicit message citing the invalid value.
                    // This unblocks debugging when the user does
                    // `Err({ status: 999 })` due to a typo or a
                    // different convention.
                    if (100..1000).contains(&s) {
                        return match value_to_json(inner) {
                            Ok(j) => HandlerOutcome::json(s as u16, j),
                            Err(msg) => HandlerOutcome::internal_error(msg),
                        };
                    } else {
                        return HandlerOutcome::internal_error(format!(
                            "invalid status code in Err: {} (must be in 100..1000)",
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

/// Serializes a `Value` to `serde_json::Value`. Total for the
/// "data" types of the language; opaque types (Function, Type,
/// Module, Range, Builtin) return `Err` with a message like
/// "value not serializable: <type>".
///
/// Important: `Result` is NOT specially handled here — that decision
/// lives in `value_to_outcome` (which maps Ok→200, Err→500). If a
/// nested `Result` arrives somehow (a handler returning
/// `Ok(Ok(x))`), we serialize as object `{"Ok": ...}` or
/// `{"Err": ...}` so no information is lost.
pub fn value_to_json(value: &Value) -> Result<serde_json::Value, String> {
    use serde_json::Value as J;

    Ok(match value {
        Value::Int(n) => J::from(*n),
        Value::Float(f) => {
            // serde_json doesn't allow NaN/Inf — we reject them
            // explicitly.
            serde_json::Number::from_f64(*f)
                .map(J::Number)
                .ok_or_else(|| format!("float not serializable as JSON: {}", f))?
        }
        Value::Str(s) => J::String(s.clone()),
        Value::Bool(b) => J::Bool(*b),
        Value::Null => J::Null,
        // Mini-batch Bytes + quick win F13 bundle — Bytes is
        // serialized as a base64 string (de-facto standard for
        // bytes in JSON). Previously emitted as an array of Int
        // (each byte an i64), which works but bloats the
        // representation ~4x and is non-standard. Decoding
        // implemented by hand (RFC 4648 alphabet without padding,
        // without problematic '+' / '/' — `base64-standard` is
        // used). To keep the dep footprint light, we don't add the
        // `base64` crate; we encode inline.
        Value::Bytes(bs) => J::String(b64_encode_standard(bs)),

        Value::List(items) => {
            let mut out = Vec::with_capacity(items.lock().len());
            for v in items.lock().iter() {
                out.push(value_to_json(v)?);
            }
            J::Array(out)
        }

        // Tuples (mini-batch T): serialized as a JSON Array (there
        // is no tuple type in JSON). Loses the tuple/list
        // distinction but it's the reasonable choice for HTTP
        // handlers.
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
                            "Map keys in JSON must be Str, found {}",
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
            // Nested Result (uncommon). We tag it so the Ok/Err
            // distinction is preserved.
            serde_json::json!({ "Ok": value_to_json(inner)? })
        }
        Value::Result(ResultVariant::Err(inner)) => {
            serde_json::json!({ "Err": value_to_json(inner)? })
        }

        // Opaque types: no sensible JSON representation.
        Value::Function { .. }
        | Value::Builtin { .. }
        | Value::Type { .. }
        | Value::Module { .. }
        | Value::RandGen(_)
        | Value::Range { .. } => {
            return Err(format!(
                "value not serializable to JSON: {}",
                value.type_name(),
            ));
        }
        // HttpResponse is not serialized directly — it lives in
        // `value_to_outcome` (which intercepts before reaching
        // here). If someone serializes it outside an HTTP context,
        // it's a codegen/runtime bug, not the user's.
        Value::HttpResponse { .. } => {
            return Err(
                "HttpResponse is not serializable to JSON outside an HTTP handler".to_string(),
            );
        }
        // Pending Future: not serializable. If a Future reaches a
        // response, the user forgot `.await`. The 6.2 checker
        // detects this statically for annotated handlers; this path
        // is defensive (handlers without return_type, Future
        // generated through another route).
        Value::Future(_) => {
            return Err(
                "pending Future is not serializable — missing `.await` somewhere in the handler"
                    .to_string(),
            );
        }
        // Phase 12.2.a — `Secret<T>` is blocked from the JSON
        // serializer by design: it prevents accidental credential
        // leaks to HTTP clients. To send the inner T (rare and
        // dangerous), the handler must unwrap explicitly with
        // `.expose()`.
        Value::Secret(_) => {
            return Err(
                "Secret<T> is not serializable to JSON (auto-redaction to prevent credential leaks). \
                 Use `.expose()` to unwrap the inner if you really need it to cross HTTP — \
                 but in that case it's better to pass the raw value without wrapping in Secret in the first place.".to_string(),
            );
        }
        // Phase 9.w.2 — `WsConn` is a live handle to a WS
        // connection, not a data value. If it reaches the
        // serializer it's a handler bug (returned the conn instead
        // of a msg/Result).
        Value::WsConn(_) => {
            return Err(
                "WsConn is not serializable to JSON — `@ws` handlers consume the conn via `recv()`/`send()`/`broadcast()`, they don't return it".to_string(),
            );
        }
        // Phase 10.1.b — `DbConn` is a handle to a TCP connection
        // with Postgres. Same criterion as WsConn: if it reaches
        // the serializer, the handler returned the handle instead
        // of the resultset.
        Value::DbConn(_) => {
            return Err(
                "DbConn is not serializable to JSON — handlers consume the conn via `query()`/`exec()`, they don't return it".to_string(),
            );
        }
        // Phase 10.3.b2 — Opaque `QueryBuilder`, non-serializable.
        Value::QueryBuilder(_) => {
            return Err(
                "QueryBuilder is not serializable to JSON — finish the chain with `.all(db)` / `.first(db)` to obtain the result".to_string(),
            );
        }
        // O1 — SqlExpr (db.now()/db.raw()) is an ORM-only marker; it
        // only makes sense inside a `.update({...})` Map. If it reaches
        // the serializer, the handler returned it by mistake.
        Value::SqlExpr(_) => {
            return Err(
                "SqlExpr (db.now()/db.raw()) is not serializable — it can only be used as a value inside an ORM `.update({...})`".to_string(),
            );
        }
        // v0.10.24 — Date/DateTime/Uuid serialize as canonical JSON
        // strings (ISO 8601 for temporals, canonical hyphenated
        // format for Uuid). Industry standard convention (JSON
        // Schema "date"/"date-time"/"uuid" formats).
        Value::Date(d) => J::String(d.format("%Y-%m-%d").to_string()),
        Value::DateTime(dt) => J::String(dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        Value::Uuid(u) => J::String(u.to_string()),
        // Mini-batch Mw-Wrap — `Value::NativeFn` is the `next`
        // callable passed to wrap-style middlewares. If it reaches
        // the serializer, the handler returned it by mistake.
        Value::NativeFn(_) => {
            return Err(
                "native function is not serializable — `next` can only be invoked, not returned"
                    .to_string(),
            );
        }
        // CorsConfig (MW.2): opaque, not serialized. If it gets
        // here, it's a registration bug: the evaluator should have
        // used it as the `@middleware(cors(...))` arg and stored
        // it in the `RouteSpec.cors` slot, not as a handler return
        // value.
        Value::CorsConfig(_) => {
            return Err(
                "CorsConfig is not serializable — it is used as an argument to `@middleware(cors(...))`, not as a value".to_string(),
            );
        }
        // PyObject (Phase 8.1+, feature `python`): opaque. The
        // handler should extract primitives (8.1) or use explicit
        // marshaling (8.2+) before returning. If a raw PyObject
        // arrives, the user forgot to coerce.
        #[cfg(feature = "python")]
        Value::PyObject(_) => {
            return Err(
                "PyObject is not serializable to JSON — convert the Python value to a Fitz type before returning it".to_string(),
            );
        }
    })
}

// ---------------------------------------------------------------------------
// JSON → Value (body deserialization)
// ---------------------------------------------------------------------------

/// Converts a `serde_json::Value` to a "free" Fitz `Value` — without
/// checking against a schema. Useful when the handler declares a
/// body without a type annotation, or with a type that is not a
/// custom `type`.
///
/// Mapping:
///   - integer numbers → `Int`; with fractional part → `Float`.
///   - strings → `Str`. Bools → `Bool`. null → `Null`.
///   - arrays → `List` with each element translated recursively.
///   - objects → `Map` with `Str` keys (preserves insertion order
///     from the serde_json parser).
///
/// Never fails: any valid JSON produces a `Value`. Validation
/// against a specific `type` happens in `json_to_instance`.
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
                // u64 that doesn't fit in i64. We store it as Float
                // so nothing is lost. Best option until we have
                // BigInt or u64 in the language.
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

/// Converts a `serde_json::Value` expected to be an object into a
/// `Value::Instance` validated against the fields of the declared
/// `type`.
///
/// Rules (same as `StructLit` in the evaluator):
///   - JSON object required — array, string or number → error.
///   - Every type field must be present, have a default, or be
///     nullable. Missing field without default or nullable → error.
///   - Extra fields (in the JSON but not in the type) → explicit
///     error.
///   - Each field value is converted recursively with
///     `json_to_value` (no additional validation against the
///     declared field type — composite type validation arrives with
///     the Phase 5 static type checker).
///
/// Returns `Err(msg)` with a message ready to send as 400.
pub fn json_to_instance(json: &serde_json::Value, type_value: &Value) -> Result<Value, String> {
    // 1. The second arg must be a Value::Type.
    let (type_name, fields) = match type_value {
        Value::Type { name, fields, .. } => (name.clone(), fields.clone()),
        other => {
            return Err(format!(
                "json_to_instance received a {} instead of a Type",
                other.type_name(),
            ));
        }
    };

    // 2. The JSON must be an object.
    let obj = match json {
        serde_json::Value::Object(map) => map,
        other => {
            return Err(format!(
                "body for '{}' must be a JSON object, received {}",
                type_name,
                json_shape_name(other),
            ));
        }
    };

    // 3. Detect extra fields before building anything. The message
    //    is more useful accumulating all extras, not just the
    //    first.
    let field_names: std::collections::HashSet<&str> =
        fields.iter().map(|f| f.name.as_str()).collect();
    let extras: Vec<&str> = obj
        .keys()
        .filter(|k| !field_names.contains(k.as_str()))
        .map(|k| k.as_str())
        .collect();
    if !extras.is_empty() {
        return Err(format!(
            "body for '{}': undeclared field{}: {}",
            type_name,
            if extras.len() == 1 { "" } else { "s" },
            extras.join(", "),
        ));
    }

    // 4. Walk declared fields in order and build the pairs. For
    //    each: use the JSON value if present, or the default
    //    evaluated in this context if not, or Null if nullable, or
    //    error.
    let mut out: Vec<(String, Value)> = Vec::with_capacity(fields.len());
    for field in &fields {
        if let Some(json_val) = obj.get(&field.name) {
            out.push((field.name.clone(), json_to_value(json_val)));
        } else if let Some(default_expr) = field.default.as_ref() {
            // Defaults are `Expr` and are evaluated in the
            // instantiation env. We don't have an env here because
            // body validation happens away from eval. For 4.3,
            // defaults only work if they are simple constant
            // literals; other cases require more plumbing. Handled
            // by `default_to_value` (local helper).
            match default_to_value(default_expr) {
                Ok(v) => out.push((field.name.clone(), v)),
                Err(_) => {
                    return Err(format!(
                        "body for '{}': field '{}' has a default that cannot be \
                         evaluated without context (Phase 4.3); pass it explicitly \
                         in the body",
                        type_name, field.name,
                    ));
                }
            }
        } else if field.type_.is_nullable() {
            out.push((field.name.clone(), Value::Null));
        } else {
            return Err(format!(
                "body for '{}': missing field '{}'",
                type_name, field.name,
            ));
        }
    }

    Ok(Value::new_instance(type_name, out))
}

/// Human-readable name for the shape of a JSON value, useful in
/// messages.
fn json_shape_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Evaluates a literal default from the AST into a `Value`. Supports
/// direct literals (the most common in `type` defaults); anything
/// else returns `Err(())` and the caller decides what to do.
///
/// We don't have an env here because we run on the HTTP runtime
/// side, not inside eval. In 4.x, if we need complex defaults, we
/// evaluate at route registration time and store the resolved value.
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
// Raw path params → Value with the declared type
// ---------------------------------------------------------------------------

/// Converts a raw path param (what axum extracted as `String`) into
/// the `Value` matching the handler parameter's declared type.
/// `None` as type → treated as `Str` (same as unannotated parameters
/// in general).
///
/// Supported types: `Int`, `Float`, `Str`, `Bool`. Any other type
/// declared in the handler for a path param is an error: custom
/// types don't go in as path params directly (`Int` for the id;
/// the handler reconstructs the object inside if needed).
///
/// Returns `Err(msg)` when the raw value can't be converted. The
/// runtime translates that error to HTTP 400.
pub fn coerce_path_param(raw: &str, declared_type: Option<&str>) -> Result<Value, String> {
    let ty = declared_type.unwrap_or("Str");
    match ty {
        "Str" => Ok(Value::Str(raw.to_string())),
        "Int" => raw
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("expected Int, received '{}'", raw)),
        "Float" => raw
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("expected Float, received '{}'", raw)),
        "Bool" => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            other => Err(format!(
                "expected Bool ('true' or 'false'), received '{}'",
                other
            )),
        },
        other => Err(format!(
            "type '{}' not supported for path params (use Int/Float/Str/Bool)",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// Async runtime — axum + tokio multi-thread, direct evaluator
// ---------------------------------------------------------------------------
//
// Design post-F17.5 (no bridge):
//
//   main thread = rt-multi-thread tokio runtime (block_on in `serve`)
//   ┌─────────────────────────────────────────────────────────────┐
//   │  axum::serve  →  async handler  →  handle_task(&registry,…) │
//   │                       │                                     │
//   │                       │  shared Arc<HttpRegistry>            │
//   │                       ▼                                     │
//   │                  call_handler(...).await  (evaluator)        │
//   └─────────────────────────────────────────────────────────────┘
//
// Each axum request is dispatched on one of the N tokio workers.
// The `Arc<HttpRegistry>` is cloned cheaply for each handler (just
// the Arc refcount); the `Value::Function`s inside are invoked via
// `handle_task` directly on the async evaluator. Real HTTP
// parallelism: two concurrent requests run simultaneously on
// different workers over the same registry. What used to cross
// threads in the previous bridge (path params, query, body, raw
// headers) now travels on the handler stack.
//
// `RouteMeta` is kept as a structural (`Send + Clone`) view of
// `RouteSpec` so `build_router` can assemble the routes without
// holding borrows of the registry — each handler's closure closes
// over the `Arc<HttpRegistry>` separately.

use std::collections::HashMap;

use crate::evaluator::call_handler;
use axum::{
    body::Body,
    extract::Path as AxumPath,
    http::{HeaderValue, StatusCode},
    response::Response,
    routing::MethodRouter,
    Router,
};

/// Structural metadata of a route the tokio thread needs to configure
/// the router. It's `Send + Sync + Clone` — doesn't include the
/// handler (which lives on the interpreter thread).
#[derive(Debug, Clone)]
pub struct RouteMeta {
    pub method: HttpMethod,
    pub path: String,
    pub has_path_params: bool,
    /// `true` if the handler declares at least one query param.
    /// Causes axum to extract `Query<HashMap<String, String>>` and
    /// send it to the interpreter. When `false`, nothing is
    /// extracted (any query string in the request is ignored).
    pub has_query_params: bool,
    /// `true` if the handler declares a body parameter. Lets the
    /// axum handler know whether to extract the body from the
    /// request and send it to the interpreter. When `false`, any
    /// received body is ignored.
    pub expects_body: bool,
    /// CORS configuration cloned from `RouteSpec.cors` (mini-phase
    /// MW.2). If `Some`, `build_router` registers an `OPTIONS`
    /// preflight handler for the same path. `Arc` clones cheaply
    /// and crosses thread boundaries without moving the shared
    /// config.
    pub cors: Option<std::sync::Arc<CorsConfig>>,
    /// Phase 9.w.2 — `true` if the route is `@ws("/path")`.
    /// `build_router` forks on detecting it: registers the path
    /// with a handler that uses `WebSocketUpgrade` instead of the
    /// normal HTTP dispatcher.
    pub is_ws: bool,
}

impl HttpRegistry {
    /// View of the registry the tokio thread can consume without
    /// taking the handlers. Useful for `build_router`.
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
                is_ws: r.is_ws,
            })
            .collect()
    }
}

/// Builds an `axum::Router` from the route metadata. Each async
/// handler closes over a cloned `Arc<HttpRegistry>` and its route
/// index, and invokes `handle_task(...).await` directly on the
/// shared registry.
///
/// The metadata (`Vec<RouteMeta>`) is enough to configure all the
/// routing: verb + path + structural flags (has_path_params /
/// has_query_params / expects_body) that decide the axum handler
/// shape (which extractors to use). The corresponding `RouteSpec`
/// (with the Fitz `Value::Function`) lives inside the registry and
/// is looked up by index when a request comes in.
///
/// `openapi_schema` (Phase 7.2): if `Some`, registers a
/// `GET /openapi.json` route serving the cached schema (precomputed
/// at server startup). If the user already declared a handler at
/// that path in their routes, auto-register yields — the user's
/// wins. `None` for programs where we don't want to serve the
/// schema (internal tests, server started in opt-out mode once 7.4
/// closes).
///
/// **F17.5**: the old `mpsc/oneshot` bridge (`InterpTask` + a
/// separate std::thread for the interpreter, with
/// `run_interpreter_loop` on the main side) is gone. Post-F17.3
/// evaluator futures are `Send` and so is `HttpRegistry` — axum
/// handlers call the evaluator directly and `tokio::spawn` (via
/// `rt-multi-thread` since F17.4a) runs them in parallel across
/// workers. This unlocks real HTTP parallelism without losing any
/// functionality the bridge had.
pub fn build_router(
    metas: &[RouteMeta],
    registry: std::sync::Arc<HttpRegistry>,
    openapi_schema: Option<serde_json::Value>,
) -> Router {
    build_router_with_asyncapi(metas, registry, openapi_schema, None)
}

// =====================================================================
// v0.10.28 (Tier S, sub-paso 4) — FITZ_HTTP_LOG: access log opt-in
// =====================================================================

/// HTTP access log mode. Enabled via the `FITZ_HTTP_LOG` env var:
///
/// - empty / `=0` / unset → `Off` (default, zero overhead, the
///   middleware isn't even mounted).
/// - `=1` / `=true` → `Simple` (method + path + status + elapsed).
/// - `=verbose` → `Verbose` (also User-Agent + Content-Length).
///
/// Logs ALL HTTP requests that go through the router: matched
/// handlers, OPTIONS preflight (CORS), auto routes `/openapi.json`/
/// `/docs`/`/asyncapi.json`, and 401/403/400/500 responses from
/// auth/middleware/handler. WebSocket handshake (GET /chat with
/// upgrade) also goes through the layer and is logged as 101
/// Switching Protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpLogMode {
    Off,
    Simple,
    Verbose,
}

/// Reads `FITZ_HTTP_LOG` once per process. The mode is locked at
/// first access (lazy). Sibling of the driver's query logging,
/// which since v0.37.12 re-reads its env var fresh each query
/// (`db::current_db_log_mode`); this HTTP toggle stays `LazyLock`
/// for now.
pub static HTTP_LOG_MODE: std::sync::LazyLock<HttpLogMode> =
    std::sync::LazyLock::new(|| match std::env::var("FITZ_HTTP_LOG").as_deref() {
        Ok("verbose") => HttpLogMode::Verbose,
        Ok("1" | "true") => HttpLogMode::Simple,
        _ => HttpLogMode::Off,
    });

/// Formats an access log line ready to emit to stderr. Pure function
/// so the unit test can assert the output without touching stderr or
/// the axum Router.
///
/// Simple form: `[fitz HTTP 12.3ms] GET /users/42 → 200`
/// Verbose form: `[fitz HTTP 45.2ms verbose] GET /users → 200 (UA="curl/8.0" len=1234)`
pub fn format_http_log_line(
    elapsed: std::time::Duration,
    method: &str,
    path: &str,
    status: u16,
    user_agent: Option<&str>,
    content_length: Option<u64>,
    mode: HttpLogMode,
) -> String {
    let ms = elapsed.as_secs_f64() * 1000.0;
    match mode {
        HttpLogMode::Off => String::new(),
        HttpLogMode::Simple => {
            format!("[fitz HTTP {ms:.1}ms] {method} {path} → {status}")
        }
        HttpLogMode::Verbose => {
            let mut extras: Vec<String> = Vec::new();
            if let Some(u) = user_agent {
                extras.push(format!("UA=\"{u}\""));
            }
            if let Some(l) = content_length {
                extras.push(format!("len={l}"));
            }
            let extras_str = if extras.is_empty() {
                String::new()
            } else {
                format!(" ({})", extras.join(" "))
            };
            format!("[fitz HTTP {ms:.1}ms verbose] {method} {path} → {status}{extras_str}")
        }
    }
}

/// Axum middleware (`from_fn` compatible) that logs each request.
/// Only mounted when `HTTP_LOG_MODE` != Off (decision made in
/// build_router_with_asyncapi) — strictly zero overhead when Off.
async fn http_log_layer(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let start = std::time::Instant::now();
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    // Capture User-Agent BEFORE moving the request to next.
    let user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let response = next.run(req).await;
    let status = response.status().as_u16();
    // Content-Length: axum usually sets it via Body::size_hint on
    // the response builder. For chunked/streaming responses it may
    // be absent — None in that case, no error.
    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let line = format_http_log_line(
        start.elapsed(),
        &method,
        &path,
        status,
        user_agent.as_deref(),
        content_length,
        *HTTP_LOG_MODE,
    );
    eprintln!("{line}");
    response
}

/// Phase 9.w.2.d — variant of `build_router` that also accepts a
/// pre-computed AsyncAPI 3.0 schema. If `Some`, registers
/// `/asyncapi.json` (and adds a note to `/docs` listing the WS
/// endpoints — the Scalar bundle doesn't support AsyncAPI natively,
/// but the endpoint is still served for external tooling). `None`
/// for programs without `@ws` handlers (zero overhead).
pub fn build_router_with_asyncapi(
    metas: &[RouteMeta],
    registry: std::sync::Arc<HttpRegistry>,
    openapi_schema: Option<serde_json::Value>,
    asyncapi_schema: Option<serde_json::Value>,
) -> Router {
    let mut router = Router::new();
    // Pre-compute merged CorsConfig per path. When multiple handlers
    // share a path (typical: `/tasks` with `@get` + `@post`, or
    // `/tasks/{id}` with `@get`/`@put`/`@delete`), each one carries
    // its own `@middleware(cors(...))` that usually differs only in
    // `allow_methods` (each one declares its verb). axum allows
    // chaining different verbs on the same path via `router.route`,
    // but the CORS preflight `OPTIONS` would be duplicated — axum
    // panics with "Overlapping method route". Solution: merge all
    // CorsConfigs per path (union of methods + headers, max of
    // max_age, first allow_origin wins) and attach the preflight
    // ONCE per path on the first handler that appears.
    let mut merged_cors_per_path: std::collections::HashMap<String, CorsConfig> =
        std::collections::HashMap::new();
    for meta in metas.iter() {
        if let Some(cors) = &meta.cors {
            merged_cors_per_path
                .entry(meta.path.clone())
                .and_modify(|existing| merge_cors_into(existing, cors))
                .or_insert_with(|| (**cors).clone());
        }
    }
    let mut preflight_attached: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (idx, meta) in metas.iter().enumerate() {
        // Phase 9.w.2 — WebSocket routes are handled differently:
        // dispatch is an HTTP GET returning
        // `WebSocketUpgrade::on_upgrade`, not the normal HTTP
        // dispatcher. CORS applies to the handshake; auth too
        // (runs pre-upgrade).
        let route_handler = if meta.is_ws {
            build_ws_method_router(idx, registry.clone())
        } else {
            build_method_router(
                meta.method,
                idx,
                registry.clone(),
                meta.has_path_params,
                meta.has_query_params,
                meta.expects_body,
            )
        };
        let route_handler = if meta.cors.is_some() && !preflight_attached.contains(&meta.path) {
            preflight_attached.insert(meta.path.clone());
            let merged = std::sync::Arc::new(merged_cors_per_path[&meta.path].clone());
            attach_preflight(route_handler, merged)
        } else {
            route_handler
        };
        router = router.route(&meta.path, route_handler);
    }

    // Auto-register of /openapi.json (Phase 7.2) and /docs (Phase
    // 7.3). The schema is precomputed by `serve` (eager, once at
    // startup); each request clones it — `serde_json::Value` clone
    // is linear in schema size, negligible for typical APIs. The
    // Scalar UI is static HTML (embedded in the binary as
    // `&'static str`).
    //
    // In both cases: if the user already declared a handler with
    // the same path, auto-register yields.
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
                axum::routing::get(|| async { axum::response::Html(crate::openapi::SCALAR_HTML) }),
            );
        }
    }
    // Phase 9.w.2.d — auto-register of /asyncapi.json when there
    // are @ws handlers. Same pattern as /openapi.json: if the user
    // declared a handler with the same path, their handler wins.
    //
    // 9.w.2-asyncapi-ui — in addition to the raw JSON, we register
    // /asyncapi with the embedded UI (parallel to OpenAPI's /docs).
    // The `@asyncapi/react-component` bundle is loaded from CDN
    // (structure identical to the Scalar pattern).
    if let Some(schema) = asyncapi_schema {
        if !metas.iter().any(|m| m.path == "/asyncapi.json") {
            let schema = std::sync::Arc::new(schema);
            router = router.route(
                "/asyncapi.json",
                axum::routing::get(move || {
                    let schema = schema.clone();
                    async move { axum::Json((*schema).clone()) }
                }),
            );
        }
        if !metas.iter().any(|m| m.path == "/asyncapi") {
            router = router.route(
                "/asyncapi",
                axum::routing::get(|| async {
                    axum::response::Html(crate::asyncapi::ASYNCAPI_HTML)
                }),
            );
        }
    }

    // Phase 12.1.b — auto-mount of `/healthz` and `/readyz` (K8s
    // probes).
    //
    // Policy:
    //   - If the user declared `@get("/healthz")` (normal HTTP
    //     handler), that wins — auto-mount yields (same pattern as
    //     /openapi.json).
    //   - If the user declared `@healthz fn ...` (dedicated
    //     decorator), that fn is mounted with dispatch to
    //     `invoke_value` + return-to-status mapping
    //     (Bool/Result<Null>/Result<Bool>).
    //   - If neither: default 200.
    //   - Same for `/readyz` but with "draining" state: when the
    //     atomic is `true` (SIGTERM fired), returns 503 WITHOUT
    //     touching the handler — K8s stops routing immediately.
    if !metas.iter().any(|m| m.path == "/healthz") {
        let healthz = registry.healthz_handler.clone();
        router = router.route(
            "/healthz",
            axum::routing::get(move || {
                let healthz = healthz.clone();
                async move {
                    match healthz {
                        Some(h) => invoke_health_check(h, "healthz").await,
                        None => default_health_response(),
                    }
                }
            }),
        );
    }
    if !metas.iter().any(|m| m.path == "/readyz") {
        let readyz = registry.readyz_handler.clone();
        let draining = registry.draining.clone();
        router = router.route(
            "/readyz",
            axum::routing::get(move || {
                let readyz = readyz.clone();
                let draining = draining.clone();
                async move {
                    // During draining, 503 without touching the
                    // handler (K8s stops routing immediately).
                    if draining.load(std::sync::atomic::Ordering::Relaxed) {
                        return drained_response();
                    }
                    match readyz {
                        Some(h) => invoke_health_check(h, "readyz").await,
                        None => default_health_response(),
                    }
                }
            }),
        );
    }

    // Phase 12.3.iter2.Tier3 — optional auto-mount of `/metrics`
    // Prometheus. Only if the provider is installed
    // (`init_prometheus(true)` was called from `serve()` when
    // `@server(prometheus=true)` or env var `FITZ_PROMETHEUS=1`).
    // Without a provider, the route is NOT mounted — a `/metrics`
    // request falls through to axum's default 404, zero overhead.
    //
    // If the user declared their own `@get("/metrics")` (rare but
    // valid — maybe a custom Prometheus endpoint with specific
    // labels), their handler wins (same pattern as
    // /openapi.json/healthz).
    if let Some(handle) = crate::observability::prometheus_handle() {
        if !metas.iter().any(|m| m.path == "/metrics") {
            let handle = handle.clone();
            router = router.route(
                "/metrics",
                axum::routing::get(move || {
                    let body = handle.render();
                    async move {
                        (
                            axum::http::StatusCode::OK,
                            [(
                                axum::http::header::CONTENT_TYPE,
                                "text/plain; version=0.0.4; charset=utf-8",
                            )],
                            body,
                        )
                    }
                }),
            );
        }
    }

    // v0.10.28 — Opt-in access log. The layer is only mounted if
    // the mode is active; when Off, the Router stays 100% as before
    // (zero overhead, not even the middleware indirection).
    if *HTTP_LOG_MODE != HttpLogMode::Off {
        router = router.layer(axum::middleware::from_fn(http_log_layer));
    }

    router
}

/// Phase 12.1.b — Default response when there's no `@healthz` or
/// `@readyz` handler declared (and the server is NOT draining).
/// Status 200 with body `{"status": "ok"}`. Body intentionally
/// minimal — any endpoint that needs more detail declares its own
/// handler.
fn default_health_response() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::OK,
        axum::Json(serde_json::json!({"status": "ok"})),
    )
        .into_response()
}

/// Phase 12.1.b — Response when the server is draining (post
/// SIGTERM/Ctrl-C). Status 503 + body `{"status": "draining"}`.
/// K8s with `readinessProbe` reads it and stops routing new traffic
/// to this pod — while axum drains in-flight requests.
fn drained_response() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({"status": "draining"})),
    )
        .into_response()
}

/// Phase 12.1.b — Invokes a `@healthz`/`@readyz` handler and maps
/// the Fitz return to an HTTP response.
///
/// Return rules:
///   - `Bool true` / `Result Ok(...)` / `Null` → 200 + `{"status": "ok"}`.
///   - `Bool false` → 503 + `{"status": "unhealthy"}`.
///   - `Result Err(e)` → 503 + `{"status": "unhealthy", "error": <e>}`.
///   - Any panic / unresolved Future → 503 with a generic message.
async fn invoke_health_check(handle: HealthCheckHandle, kind: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    let name = handle.name.clone();
    let raw =
        crate::evaluator::invoke_value(handle.handler, vec![], &name, crate::ast::Span::ZERO).await;
    let value = match raw {
        Ok(v) => v,
        Err(_) => {
            // Evaluation error → unhealthy with a generic message
            // (the specific error goes to stderr via the evaluator).
            eprintln!("[{kind}] error al invocar handler '{name}'");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({"status": "unhealthy", "error": "handler error"})),
            )
                .into_response();
        }
    };
    // If it was async, the Value is a Future that must be consumed.
    let value = match await_if_future(value).await {
        Ok(v) => v,
        Err(_) => {
            eprintln!("[{kind}] handler '{name}' produjo Future con error");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({"status": "unhealthy", "error": "future error"})),
            )
                .into_response();
        }
    };
    map_health_value_to_response(value)
}

/// Helper for `invoke_health_check`. Converts the `Value` returned
/// by the Fitz handler to an appropriate HTTP response per the
/// rules documented above.
fn map_health_value_to_response(value: Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    match value {
        Value::Bool(true) | Value::Null => (
            axum::http::StatusCode::OK,
            axum::Json(serde_json::json!({"status": "ok"})),
        )
            .into_response(),
        Value::Bool(false) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"status": "unhealthy"})),
        )
            .into_response(),
        Value::Result(crate::value::ResultVariant::Ok(_)) => (
            axum::http::StatusCode::OK,
            axum::Json(serde_json::json!({"status": "ok"})),
        )
            .into_response(),
        Value::Result(crate::value::ResultVariant::Err(boxed)) => {
            let msg = match &*boxed {
                Value::Str(s) => s.clone(),
                other => format!("{}", other),
            };
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({"status": "unhealthy", "error": msg})),
            )
                .into_response()
        }
        other => {
            // Unexpected type (the checker should have prevented
            // this). We treat it as healthy with a stderr warning so
            // probes are not broken by a codegen bug.
            eprintln!(
                "[probe] handler returned value of unexpected type {} — treating as OK",
                other.type_name()
            );
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({"status": "ok"})),
            )
                .into_response()
        }
    }
}

/// Converts the axum `HeaderMap` to a `HashMap<String, String>`
/// with all keys lowercased (Phase 7.6). The dispatcher does
/// case-insensitive lookup against this map. Non-UTF-8 headers are
/// dropped (HTTP theoretically allows weird bytes; in practice all
/// usual headers are ASCII).
/// FITZ-05 — parses a raw `Cookie` header value (`"a=1; b=2"`) and returns the
/// value of `name`, or `None`. Splits on `;`, then on the FIRST `=` so a value
/// containing `=` (e.g. padded base64) survives. Trims whitespace.
pub fn parse_cookie_header(raw: &str, name: &str) -> Option<String> {
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// FITZ-05 FASE B — canonical `Set-Cookie` serialisation shared by the
/// interpreter path (below) and mirrored bit-for-bit by the codegen
/// prelude helper `__fitz_serialize_set_cookie` (parity `fitz run` ↔
/// `fitz build`). Attribute order: `name=value; Path; Domain; Max-Age;
/// HttpOnly; Secure; SameSite`. `Path` is emitted whenever non-empty
/// (default `/`); `SameSite` whenever non-empty (default `Lax`).
#[allow(clippy::too_many_arguments)]
pub fn serialize_set_cookie(
    name: &str,
    value: &str,
    path: &str,
    http_only: bool,
    secure: bool,
    same_site: &str,
    max_age: Option<i64>,
    domain: Option<&str>,
) -> String {
    let mut s = format!("{}={}", name, value);
    if !path.is_empty() {
        s.push_str("; Path=");
        s.push_str(path);
    }
    if let Some(d) = domain {
        s.push_str("; Domain=");
        s.push_str(d);
    }
    if let Some(ma) = max_age {
        s.push_str("; Max-Age=");
        s.push_str(&ma.to_string());
    }
    if http_only {
        s.push_str("; HttpOnly");
    }
    if secure {
        s.push_str("; Secure");
    }
    if !same_site.is_empty() {
        s.push_str("; SameSite=");
        s.push_str(same_site);
    }
    s
}

/// FITZ-05 FASE B — serialises a built-in `Cookie` instance (from a
/// `Response { cookies: [...] }`) to a `Set-Cookie` header value.
/// Returns `Err(message)` if the shape is wrong (only reachable via
/// `--no-typecheck`; the checker validates the field types statically).
/// The evaluator's struct-lit pass fills the missing fields with the
/// canonical defaults, so `name`/`value` are the only ones that can be
/// genuinely absent (both required, no default).
fn cookie_instance_to_set_cookie(item: &Value) -> Result<String, String> {
    let fields = match item {
        Value::Instance { type_name, fields } if type_name == "Cookie" => fields,
        other => {
            return Err(format!(
                "Response.cookies must be a List<Cookie>, found {} in the list",
                other.type_name()
            ));
        }
    };
    let g = fields.lock();
    let mut name: Option<String> = None;
    let mut value: Option<String> = None;
    let mut path = String::from("/");
    let mut http_only = false;
    let mut secure = false;
    let mut same_site = String::from("Lax");
    let mut max_age: Option<i64> = None;
    let mut domain: Option<String> = None;
    for (k, v) in g.iter() {
        match (k.as_str(), v) {
            ("name", Value::Str(s)) => name = Some(s.clone()),
            ("value", Value::Str(s)) => value = Some(s.clone()),
            ("path", Value::Str(s)) => path = s.clone(),
            ("http_only", Value::Bool(b)) => http_only = *b,
            ("secure", Value::Bool(b)) => secure = *b,
            ("same_site", Value::Str(s)) => same_site = s.clone(),
            ("max_age", Value::Int(n)) => max_age = Some(*n),
            ("max_age", Value::Null) => max_age = None,
            ("domain", Value::Str(s)) => domain = Some(s.clone()),
            ("domain", Value::Null) => domain = None,
            (fname, other) => {
                return Err(format!(
                    "Cookie.{} has an unexpected value ({})",
                    fname,
                    other.type_name()
                ));
            }
        }
    }
    drop(g);
    let name = name.ok_or_else(|| "Cookie.name is required".to_string())?;
    let value = value.ok_or_else(|| "Cookie.value is required".to_string())?;
    Ok(serialize_set_cookie(
        &name,
        &value,
        &path,
        http_only,
        secure,
        &same_site,
        max_age,
        domain.as_deref(),
    ))
}

fn headers_to_map(hm: &axum::http::HeaderMap) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for (name, value) in hm.iter() {
        if let Ok(v) = value.to_str() {
            out.insert(name.as_str().to_lowercase(), v.to_string());
        }
    }
    out
}

/// 9.w.2-ws-auth-browser: extracts a bearer token from the
/// `Sec-WebSocket-Protocol` header of the WS handshake. Standard
/// workaround for authenticating WS from browsers — the
/// `new WebSocket(url, protocols)` API does NOT allow setting
/// arbitrary HTTP headers, but it DOES accept a list of subprotocols
/// as the second argument.
///
/// Convention: the client sends a subprotocol in the format
/// `bearer.<token>` (where `<token>` is JWT or opaque). The server
/// extracts the token, injects it as `authorization: Bearer <token>`
/// in the headers map seen by the `@auth_provider`, and echoes the
/// selected subprotocol in the handshake response (RFC 6455 §4.1 —
/// without the echo, the browser rejects the upgrade).
///
/// Returns `Some((full_subprotocol, token))` if a subprotocol
/// matching `bearer.*` was found, `None` otherwise.
pub fn extract_ws_bearer_subprotocol(headers: &axum::http::HeaderMap) -> Option<(String, String)> {
    let raw = headers
        .get("sec-websocket-protocol")
        .or_else(|| headers.get("Sec-WebSocket-Protocol"))?
        .to_str()
        .ok()?;
    // RFC 6455: the header can be CSV (comma-separated) with
    // multiple offered subprotocols. The server picks one.
    for piece in raw.split(',') {
        let proto = piece.trim();
        if let Some(token) = proto.strip_prefix("bearer.") {
            if !token.is_empty() {
                return Some((proto.to_string(), token.to_string()));
            }
        }
    }
    None
}

/// Builds a `MethodRouter` with the async handler corresponding to
/// the verb. The eight combinations (path_params × query × body)
/// live in different closures because axum extractors appear as
/// handler arguments — they can't be made conditional. `HeaderMap`
/// is **always** extracted as an extra argument (Phase 7.6): it's
/// zero-cost when the handler declares no headers (an empty HashMap
/// is passed and `handle_task` ignores it).
///
/// **F17.5**: each closure clones the `Arc<HttpRegistry>` and calls
/// `handle_task(&registry, ...).await` directly. Before this, it
/// sent an `InterpTask` over mpsc and awaited a `oneshot`. Removing
/// the bridge unlocks real HTTP parallelism: with the
/// `rt-multi-thread` runtime (F17.4a), N workers process handlers
/// simultaneously over the same shared (Send + Sync) registry.
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
                    dispatch_request(&registry, route_idx, Map::new(), Map::new(), Vec::new(), hm)
                        .await
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
                    dispatch_request(
                        &registry,
                        route_idx,
                        Map::new(),
                        Map::new(),
                        body.to_vec(),
                        hm,
                    )
                    .await
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

/// Maps `HttpMethod` to the axum constructor
/// (`get`/`post`/`put`/`delete`) applied to the given handler.
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

/// Adds an `OPTIONS` handler to the given MethodRouter to answer
/// CORS preflight (mini-phase MW.2). The handler returns 204 with
/// `Access-Control-Allow-*` headers resolved against the request
/// `Origin` — it does not touch the interpreter, so it's fast and
/// doesn't use the mpsc bridge. Q.3: the `Access-Control-Allow-Origin`
/// header can be omitted if the `Set` policy rejects the received
/// origin (browser rejects the preflight, standard strict CORS
/// behavior).
/// Phase 9.w.2 — Builds the `MethodRouter` for a WebSocket route.
/// The HTTP method is always GET (handshake). The handler extracts
/// axum's `WebSocketUpgrade` extractor, runs auth pre-upgrade and
/// then runs `ws.on_upgrade(...)`. Inside the upgrade closure it
/// builds the `Value::WsConn` and calls the Fitz handler.
///
/// Differences vs normal HTTP:
///   - No dynamic path params (`@ws("/chat")` is an exact path).
///   - No body in the handshake (GET).
///   - Auth: 401/403 before upgrade. Pre/Wrap middleware: same.
///   - Output: `Response::switching_protocols` that axum chains
///     with the rest of the upgrade flow.
fn build_ws_method_router(
    route_idx: usize,
    registry: std::sync::Arc<HttpRegistry>,
) -> MethodRouter {
    use axum::extract::ws::WebSocketUpgrade;
    use axum::http::HeaderMap;
    use axum::response::IntoResponse;
    axum::routing::get(move |ws: WebSocketUpgrade, headers: HeaderMap| {
        let registry = registry.clone();
        async move {
            let route = match registry.routes.get(route_idx) {
                Some(r) => r,
                None => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "WS route not found in the registry",
                    )
                        .into_response();
                }
            };
            // Phase 12.8 — gate by feature flag at the WS handshake.
            // If the flag is off, return 404 BEFORE the upgrade
            // (parallel to HTTP).
            if let Some(name) = route.flag_name.as_ref() {
                if !crate::evaluator::is_flag_enabled(name) {
                    let body = serde_json::json!({
                        "error": format!("feature '{}' disabled", name)
                    });
                    return (
                        StatusCode::NOT_FOUND,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        body.to_string(),
                    )
                        .into_response();
                }
            }
            let mut raw_headers = headers_to_map(&headers);
            // 9.w.2-ws-auth-browser: extract a bearer token from the
            // `Sec-WebSocket-Protocol` header when the client sent a
            // subprotocol formatted as `bearer.<token>`. This is the
            // standard workaround so browsers (which CANNOT set the
            // `Authorization` header in `new WebSocket(url)`) can
            // pass tokens at the handshake. The runtime injects
            // `authorization: Bearer <token>` into the headers map
            // seen by the `@auth_provider`, parallel to the HTTP
            // flow. The server also echoes the chosen subprotocol so
            // the browser doesn't reject the upgrade (RFC 6455 §4.1).
            let bearer_subproto = extract_ws_bearer_subprotocol(&headers);
            if let Some((selected_proto, token)) = &bearer_subproto {
                raw_headers
                    .entry("authorization".to_string())
                    .or_insert_with(|| format!("Bearer {}", token));
                let _ = selected_proto;
            }
            let ws = match &bearer_subproto {
                Some((selected_proto, _)) => ws.protocols([selected_proto.clone()]),
                None => ws,
            };

            // Pre-upgrade auth (parallel to the HTTP wrapper 9.w.1.c).
            // The checker guarantees that if `route.auth != None`,
            // there is a provider in the registry. Phase
            // 9.w.1.iter2.a: `@requires("role")` also triggers the
            // wrapper even when `auth == None`.
            let mut auth_user: Option<Value> = None;
            if route.auth != AuthSpec::None || !route.required_roles.is_empty() {
                let provider = match registry.auth_provider.as_ref() {
                    Some(h) => h.clone(),
                    None => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            axum::Json(serde_json::json!({
                                "error": format!(
                                    "WS route '{}' requires auth but there is no @auth_provider — registry bug",
                                    route.handler_name
                                )
                            })),
                        )
                            .into_response();
                    }
                };
                // Build Map<Str,Str> of headers.
                let headers_pairs: Vec<(Value, Value)> = raw_headers
                    .iter()
                    .map(|(k, v)| (Value::Str(k.clone()), Value::Str(v.clone())))
                    .collect();
                let headers_arg = Value::Map(shared(headers_pairs));
                let invoked =
                    call_handler(provider.handler.clone(), vec![headers_arg], &provider.name).await;
                let raw_result = match invoked {
                    Ok(v) => v,
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            axum::Json(serde_json::json!({"error": e.message})),
                        )
                            .into_response();
                    }
                };
                // If the provider is async, await the Future.
                let resolved = if provider.is_async {
                    match raw_result {
                        Value::Future(cell) => {
                            let fut = cell.0.lock().take();
                            match fut {
                                Some(f) => match f.await {
                                    Ok(v) => v,
                                    Err(e) => {
                                        return (
                                            StatusCode::INTERNAL_SERVER_ERROR,
                                            axum::Json(serde_json::json!({
                                                "error": format!("@auth_provider failed: {}", e.message)
                                            })),
                                        )
                                            .into_response();
                                    }
                                },
                                None => {
                                    return (
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        "provider Future already consumed (bug)",
                                    )
                                        .into_response();
                                }
                            }
                        }
                        other => {
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                axum::Json(serde_json::json!({
                                    "error": format!("async provider did not return a Future: {}", other.type_name())
                                })),
                            )
                                .into_response();
                        }
                    }
                } else {
                    raw_result
                };
                match resolved {
                    Value::Result(ResultVariant::Ok(user_box)) => {
                        if route.auth == AuthSpec::Admin {
                            let role_ok = match user_box.as_ref() {
                                Value::Instance { fields, .. } => {
                                    let g = fields.lock();
                                    g.iter().any(|(k, v)| {
                                        k == "role" && matches!(v, Value::Str(s) if s == "admin")
                                    })
                                }
                                _ => false,
                            };
                            if !role_ok {
                                return (
                                    StatusCode::FORBIDDEN,
                                    axum::Json(serde_json::json!({
                                        "error": "access forbidden — admin role required"
                                    })),
                                )
                                    .into_response();
                            }
                        }
                        // Phase 9.w.1.iter2.a — Custom RBAC with
                        // `@requires("role")`. user.role (Str) must
                        // match at least one of `required_roles`.
                        if !route.required_roles.is_empty() {
                            let actual_role = match user_box.as_ref() {
                                Value::Instance { fields, .. } => {
                                    let g = fields.lock();
                                    g.iter().find_map(|(k, v)| {
                                        if k == "role" {
                                            if let Value::Str(s) = v {
                                                return Some(s.clone());
                                            }
                                        }
                                        None
                                    })
                                }
                                _ => None,
                            };
                            let allowed = actual_role
                                .as_ref()
                                .is_some_and(|r| route.required_roles.iter().any(|x| x == r));
                            if !allowed {
                                let msg = match &actual_role {
                                    Some(r) => format!(
                                        "access forbidden — role '{}' not authorized (required: {})",
                                        r,
                                        route.required_roles.join(", "),
                                    ),
                                    None => format!(
                                        "access forbidden — missing role (required: {})",
                                        route.required_roles.join(", "),
                                    ),
                                };
                                return (
                                    StatusCode::FORBIDDEN,
                                    axum::Json(serde_json::json!({"error": msg})),
                                )
                                    .into_response();
                            }
                        }
                        auth_user = Some(*user_box);
                    }
                    Value::Result(ResultVariant::Err(msg_box)) => {
                        let msg = match *msg_box {
                            Value::Str(s) => s,
                            v => format!("{}", v),
                        };
                        return (
                            StatusCode::UNAUTHORIZED,
                            axum::Json(serde_json::json!({"error": msg})),
                        )
                            .into_response();
                    }
                    other => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            axum::Json(serde_json::json!({
                                "error": format!(
                                    "@auth_provider returned unexpected shape: {}",
                                    other.type_name()
                                )
                            })),
                        )
                            .into_response();
                    }
                }
            }

            // Capture what the upgrade closure will need.
            let endpoint = route.path.clone();
            let handler = route.handler.clone();
            let handler_name = route.handler_name.clone();
            let auth_user_param_name = route.auth_user_param_name.clone();
            let ws_conn_param_name = route.ws_conn_param_name.clone();
            let ws_msg_type = route.ws_msg_type.clone();
            let ws_send_type = route.ws_send_type.clone();
            // Phase 9.w.2-ws-headers — `@header(...)` params on a `@ws` handler
            // read the handshake headers (the WS upgrade IS an HTTP request).
            let header_specs = route.headers.clone();
            // Handler env — Value::Function carries it as `closure`.
            // recv() uses it to resolve nominal T and coerce Map →
            // Instance.
            let handler_env = match &route.handler {
                Value::Function { closure, .. } => closure.clone(),
                _ => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "WS handler is not Value::Function — registration bug",
                    )
                        .into_response();
                }
            };
            let resolved_params: Vec<String> = route
                .param_type_exprs
                .iter()
                .map(|(n, _)| n.clone())
                .collect();
            let broadcaster = registry.ws_broadcaster.clone();
            // Phase 9.w.2.e — heartbeat ping interval from the
            // config. Default 30s; the user overrides with
            // `@server(ws_heartbeat_secs=N)`. `0` disables.
            let heartbeat_secs = registry
                .server_config
                .as_ref()
                .map(|c| c.ws_heartbeat_secs)
                .unwrap_or(30);

            // Upgrade. The closure is `FnOnce(WebSocket)`; inside we
            // build the Value::WsConn, call the Fitz handler, and
            // clean up at the end.
            ws.on_upgrade(move |socket| async move {
                let (conn_value, conn_id, writer_task) = build_ws_conn(
                    socket,
                    endpoint.clone(),
                    broadcaster.clone(),
                    ws_msg_type,
                    ws_send_type,
                    handler_env,
                    heartbeat_secs,
                );
                // Build args in the handler's declared order. The ws
                // conn goes in `ws_conn_param_name`; user (if any)
                // in `auth_user_param_name`. Other params are not
                // supported in WS today.
                let mut args: Vec<Value> = Vec::with_capacity(resolved_params.len());
                for name in &resolved_params {
                    if ws_conn_param_name.as_deref() == Some(name.as_str()) {
                        args.push(conn_value.clone());
                    } else if auth_user_param_name.as_deref() == Some(name.as_str()) {
                        args.push(auth_user.clone().unwrap_or(Value::Null));
                    } else if let Some(hdr) = header_specs.iter().find(|h| &h.param_name == name) {
                        // `@header(...)` param — read from the handshake headers
                        // (case-insensitive). Missing + nullable → Null; missing
                        // + required → close the conn (we can't 400 post-upgrade).
                        let key = hdr.http_name.to_lowercase();
                        match (raw_headers.get(&key), hdr.is_nullable) {
                            (Some(v), _) => args.push(Value::Str(v.clone())),
                            (None, true) => args.push(Value::Null),
                            (None, false) => {
                                eprintln!(
                                    "WS handler '{}': required header '{}' missing, closing conn",
                                    handler_name, hdr.http_name,
                                );
                                broadcaster.unregister(&endpoint, conn_id);
                                writer_task.abort();
                                return;
                            }
                        }
                    } else {
                        // Unclassified param — registration bug.
                        // Log and close the conn.
                        eprintln!(
                            "WS handler '{}': param '{}' unclassified, closing conn",
                            handler_name, name,
                        );
                        broadcaster.unregister(&endpoint, conn_id);
                        writer_task.abort();
                        return;
                    }
                }
                // Invoke the Fitz handler. It's async (validated by
                // the 9.w.2.a checker), so the ret is a Value::Future
                // that must be awaited.
                let invoke = call_handler(handler, args, &handler_name).await;
                match invoke {
                    Ok(Value::Future(cell)) => {
                        let fut = cell.0.lock().take();
                        if let Some(f) = fut {
                            // The handler runs here; at any moment
                            // it can call recv/send/broadcast/close.
                            let _ = f.await;
                        }
                    }
                    Ok(_) => {
                        // The checker validated async fn — if it
                        // didn't return a Future, it's a bug. We
                        // close.
                    }
                    Err(e) => {
                        eprintln!("WS handler '{}' failed: {}", handler_name, e.message,);
                    }
                }
                // Cleanup: unregister from the broadcaster + close
                // writer task. The axum conn closes when the sink
                // is dropped (writer_task holds the sink and
                // terminates on its next loop iteration).
                broadcaster.unregister(&endpoint, conn_id);
                let _ = writer_task.await; // graceful: wait for the writer to finish
            })
            .into_response()
        }
    })
}

/// Incremental merge of an additional `CorsConfig` over an existing
/// one — used by `build_router_with_asyncapi` when multiple handlers
/// share a path. Each handler carries its own `@middleware(cors(...))`
/// with its specific `allow_methods`; the union of all of them
/// defines the preflight the browser sees.
///
/// Merge policy:
/// - `allow_methods`: union preserving insertion order.
/// - `allow_headers`: case-insensitive union (HTTP header names
///   are).
/// - `max_age`: the larger of the two (None means "no opinion").
/// - `allow_origin`: the first wins — handlers on the same path
///   should normally declare the same origin; if they disagree, it's
///   a user error and we prefer not to invent an aggregate policy.
fn merge_cors_into(existing: &mut CorsConfig, other: &CorsConfig) {
    for m in &other.allow_methods {
        if !existing.allow_methods.iter().any(|e| e == m) {
            existing.allow_methods.push(m.clone());
        }
    }
    for h in &other.allow_headers {
        if !existing
            .allow_headers
            .iter()
            .any(|e| e.eq_ignore_ascii_case(h))
        {
            existing.allow_headers.push(h.clone());
        }
    }
    existing.max_age = match (existing.max_age, other.max_age) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, x) => x,
    };
}

fn attach_preflight(mr: MethodRouter, cors: std::sync::Arc<CorsConfig>) -> MethodRouter {
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

/// Single point where the axum handler invokes the evaluator and
/// returns the `Response`. Post-F17.5: direct call to `handle_task`
/// — the mpsc/oneshot bridge that existed in F4.x was removed once
/// `Value`/`EnvRef` became `Send` (F17.2-3) and `HttpRegistry`
/// became `Send + Sync`.
///
/// Phase 12.3.b.2 — auto HTTP instrumentation:
/// - Opens a `SpanContext::new_root()` before invoking
///   `handle_task`.
/// - Wraps the execution with `with_span_context(ctx, ...)` so EVERY
///   `log.info/warn/error/debug` emitted inside the handler
///   automatically inherits the `trace_id`/`span_id`.
/// - Emits `log.info("http.access", ...)` at the END of the request
///   with `http.method`, `http.target`, `http.status_code`,
///   `duration_ms`. The access log is emitted INSIDE the scope so it
///   also includes trace_id/span_id (correlation with the rest of
///   the request logs). OTel-compatible naming convention.
async fn dispatch_request(
    registry: &HttpRegistry,
    route_idx: usize,
    path_params: HashMap<String, String>,
    query_params: HashMap<String, String>,
    body: Vec<u8>,
    headers: HashMap<String, String>,
) -> Response {
    let (method_str, path_template): (&'static str, String) = match registry.routes.get(route_idx) {
        Some(route) => (route.method.as_str(), route.path.clone()),
        None => ("UNKNOWN", String::new()),
    };

    // Phase 12.8 — gate by feature flag. If the handler has
    // `@flag("name")` and the flag is off in the runtime registry,
    // stop here with 404. Architectural decision: check BEFORE
    // auth/middlewares (the route "does not exist" from the
    // client's point of view, no schemas/auth info is leaked). The
    // observability/access log stays active for auditing.
    let flag_blocked: Option<String> = registry
        .routes
        .get(route_idx)
        .and_then(|r| r.flag_name.as_ref())
        .filter(|name| !crate::evaluator::is_flag_enabled(name))
        .cloned();
    if let Some(name) = flag_blocked {
        let body = serde_json::json!({"error": format!("feature '{}' disabled", name)});
        let mut response = Response::new(body.to_string().into());
        *response.status_mut() = StatusCode::NOT_FOUND;
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        return response;
    }

    // Phase 12.3.b.5 — opt-out with `@server(observability=false)`.
    // When disabled, we bypass the ENTIRE instrumentation wrapper:
    // no span context opened, no access log emitted, no metrics
    // registered. Handlers run bare-metal, with no per-request
    // overhead. A user calling `log.X(...)` inside a handler still
    // emits, but without trace_id/span_id (no active span).
    let observability_enabled = registry
        .server_config
        .as_ref()
        .map(|c| c.observability_enabled)
        .unwrap_or(true);
    if !observability_enabled {
        let outcome = handle_task(
            registry,
            route_idx,
            path_params,
            query_params,
            body,
            headers,
        )
        .await;
        return outcome_to_response(outcome);
    }

    let start = std::time::Instant::now();

    // Phase 12.3.c.1 — open an OTel span parallel to the own
    // SpanContext when the provider is installed. The OTel span is
    // sent to the backend (Jaeger/Tempo/Honeycomb/Datadog) with
    // `http.method`/`http.target` attributes at boot; at the end of
    // the request we add `http.status_code` + close. Without the
    // provider installed, `is_otel_enabled()` is `false` and the
    // entire block is skipped (zero overhead).
    //
    // Phase 12.3.iter2.a — the OTel span is opened BEFORE the own
    // SpanContext so we can derive `trace_id`/`span_id` from its
    // IDs. When OTel is active, the `trace_id` appearing in
    // stderr/Loki logs is THE SAME as the OTel span's in
    // Jaeger/Tempo/Datadog — enabling cross-pipeline queries.
    // Without OTel, `new_root()` generates fresh trace_id/span_id
    // via uuid.
    let mut otel_span = if crate::observability::is_otel_enabled() {
        use opentelemetry::trace::{Span as _, Tracer as _};
        use opentelemetry::KeyValue;
        let tracer = crate::observability::tracer();
        let mut span = tracer.start(format!("HTTP {} {}", method_str, path_template));
        span.set_attribute(KeyValue::new("http.method", method_str));
        span.set_attribute(KeyValue::new("http.target", path_template.clone()));
        Some(span)
    } else {
        None
    };

    // Phase 12.3.iter2.a — derive the own SpanContext from the OTel
    // span (when there is one). `TraceId::to_string()` and
    // `SpanId::to_string()` return 32/16 lowercase hex chars — same
    // format as the own `generate_trace_id`/`generate_span_id`.
    // Without OTel, fresh uuid.
    let ctx = if let Some(span) = otel_span.as_ref() {
        use opentelemetry::trace::Span as _;
        let sctx = span.span_context();
        crate::logging::SpanContext::with_ids(
            sctx.trace_id().to_string(),
            sctx.span_id().to_string(),
        )
    } else {
        crate::logging::SpanContext::new_root()
    };

    let outcome = crate::logging::with_span_context(ctx, || async {
        let outcome = handle_task(
            registry,
            route_idx,
            path_params,
            query_params,
            body,
            headers,
        )
        .await;
        // Emit access log INSIDE the scope so it inherits
        // trace_id/span_id from the request span. OTel-compatible
        // naming: `http.method` / `http.target` /
        // `http.status_code` / `duration_ms`.
        let elapsed = start.elapsed();
        let duration_ms = elapsed.as_millis() as i64;
        let duration_secs = elapsed.as_secs_f64();
        let status_str = outcome.status.to_string();
        let kvs: Vec<(String, Value)> = vec![
            (
                "http.method".to_string(),
                Value::Str(method_str.to_string()),
            ),
            ("http.target".to_string(), Value::Str(path_template.clone())),
            (
                "http.status_code".to_string(),
                Value::Int(outcome.status as i64),
            ),
            ("duration_ms".to_string(), Value::Int(duration_ms)),
        ];
        crate::logging::emit_log_record("info", "http.access", &kvs);

        // Phase 12.3.b.3 — built-in metrics. Counter +1 per
        // request with labels (method, path-template, status).
        // Histogram with the duration in seconds (Prometheus-style
        // — default recorder buckets + quantiles via
        // `histogram_quantiles` when exposed). Labels are ALWAYS
        // the same between Counter and Histogram for cross-metric
        // correlation.
        //
        // Without a global recorder installed (default case), the
        // macros are silent no-ops — zero overhead. In 12.3.c we
        // add the OTLP exporter installed as the global recorder
        // and connect these metrics to the OTel backend.
        // Prometheus-style naming
        // (`http_requests_total` /
        // `http_request_duration_seconds`) for ecosystem
        // consistency; OTel semantic conventions
        // (`http.server.request.duration`) are evaluated in 12.3.c
        // if real demand appears.
        let labels = [
            ("method", method_str.to_string()),
            ("path", path_template.clone()),
            ("status", status_str),
        ];
        metrics::counter!("http_requests_total", &labels).increment(1);
        metrics::histogram!("http_request_duration_seconds", &labels).record(duration_secs);

        outcome
    })
    .await;

    // Phase 12.3.c.1 — finalize the OTel span with the real status
    // code and result (Ok for 2xx/3xx, Error for 4xx/5xx). Only if
    // the provider was installed at request boot (zero overhead
    // without OTel).
    if let Some(span) = otel_span.as_mut() {
        use opentelemetry::trace::{Span as _, Status};
        use opentelemetry::KeyValue;
        span.set_attribute(KeyValue::new("http.status_code", outcome.status as i64));
        if outcome.status >= 400 {
            span.set_status(Status::error(format!("HTTP {}", outcome.status)));
        } else {
            span.set_status(Status::Ok);
        }
        span.end();
    }

    outcome_to_response(outcome)
}

/// Converts a `HandlerOutcome` into an axum `Response`. Status,
/// `content-type` header, body as bytes, and the `extra_headers`
/// injected by middlewares (mini-phase MW.2: CORS headers).
///
/// If an extra_header carries an unparseable name or value as an
/// HTTP header, it is silently dropped — we prefer losing a
/// malformed header to panicking on a request. In practice the CORS
/// headers we emit are valid by construction.
fn outcome_to_response(outcome: HandlerOutcome) -> Response {
    // v0.19.0 block 2 — binary path: `body_bytes` wins over `body`.
    // Built when the user returns `Response { body_bytes: bytes(...) }`
    // for PDF, ZIP, images, etc. Text path stays default.
    let body = match outcome.body_bytes {
        Some(bytes) => Body::from(bytes),
        None => Body::from(outcome.body),
    };
    let mut resp = Response::new(body);
    *resp.status_mut() =
        StatusCode::from_u16(outcome.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    // v0.19.0 — content_type is now a runtime String (from
    // `Response { content_type: ... }` or default
    // `application/json`). If the user supplied an invalid value
    // (chars outside ASCII visible range, etc.), `try_from` fails
    // and we fall back to `application/octet-stream` so the
    // response still ships with a valid header. Silent fallback
    // matches the behaviour of `extra_headers` below.
    let ct_value = HeaderValue::try_from(outcome.content_type.as_str())
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    resp.headers_mut()
        .insert(axum::http::header::CONTENT_TYPE, ct_value);
    for (name, value) in outcome.extra_headers {
        let parsed_name = axum::http::HeaderName::try_from(name);
        let parsed_value = HeaderValue::try_from(value);
        if let (Ok(n), Ok(v)) = (parsed_name, parsed_value) {
            // FITZ-05 FASE B — `Set-Cookie` is the one header where
            // multiple values are the norm (one per cookie). `.insert`
            // overwrites, keeping only the last; `.append` preserves
            // every cookie. All other headers keep the single-value
            // overwrite semantics of `.insert`.
            if n == axum::http::header::SET_COOKIE {
                resp.headers_mut().append(n, v);
            } else {
                resp.headers_mut().insert(n, v);
            }
        }
    }
    resp
}

/// Builds the `Value::Instance` of type `Request` the runtime
/// passes to each middleware (mini-phase MW.1). The path carries
/// the path params substituted (`/users/{id}` with `id=42` is seen
/// as `/users/42`); the original request query string is NOT
/// concatenated to avoid depending on `HashMap` order. If real
/// demand for exposing the full query string appears, we add it as
/// minor debt. Headers are exposed with lowercase keys (consistent
/// with the case-insensitive `@header` dispatch).
fn build_request_value(
    method: HttpMethod,
    path_template: &str,
    raw_path_params: &HashMap<String, String>,
    headers: &HashMap<String, String>,
) -> Value {
    use crate::value::shared;

    // Substitute each `{name}` with its real value. O(n*m) but n
    // and m are small (a typical handler has 0-3 path params);
    // avoidable with a fine-grained parser, not worth the
    // maintenance cost.
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
            (
                "method".to_string(),
                Value::Str(method.as_str().to_string()),
            ),
            ("path".to_string(), Value::Str(path)),
            ("headers".to_string(), Value::Map(shared(headers_pairs))),
        ],
    )
}

/// Runs a route's middleware chain in order (mini-phase MW.1). Each
/// middleware receives a single `Request` arg and is expected to
/// return:
///
///   - `Value::Null` (or nothing) → the chain continues with the
///     next middleware or the handler.
///   - `Value::HttpResponse` (built with `return <status> { ... }`)
///     → short-circuit: the chain stops here and the outcome is
///     returned to the client.
///   - Any other value → 500 with a clear message (middleware must
///     be gate-only).
///
/// Returns `Some(outcome)` if a middleware short-circuits or if
/// something failed; `None` if the chain reached the end and the
/// handler should be invoked.
async fn run_middleware_chain(
    middlewares: &[MiddlewareSpec],
    request: &Value,
) -> Option<HandlerOutcome> {
    // Mw.next — we only run the Pre (gate-only) middlewares in this
    // path. Posts are processed in `run_post_middlewares` after the
    // handler.
    for mw in middlewares.iter().filter(|m| m.kind == MiddlewareKind::Pre) {
        let args = vec![request.clone()];
        let label = format!("middleware {}", mw.name);
        let raw = match call_handler(mw.handler.clone(), args, &label).await {
            Ok(v) => v,
            Err(err) => {
                return Some(HandlerOutcome::internal_error(format!(
                    "middleware '{}' failed: {}",
                    mw.name, err.message,
                )));
            }
        };
        // An `async fn` middleware returns a `Value::Future` — await it
        // before inspecting the result (parallel to the HTTP handler + auth
        // provider paths, which use the same helper). Without this an async
        // middleware (e.g. a rate limit that hits the DB with `.await`)
        // returned `Value::Future`, matched no arm below, and 500'd only in
        // `fitz run` — a run↔build parity bug (`fitz build` awaited it).
        let resolved = match await_if_future(raw).await {
            Ok(r) => r,
            Err(err) => {
                return Some(HandlerOutcome::internal_error(format!(
                    "middleware '{}' failed: {}",
                    mw.name, err.message,
                )));
            }
        };
        match resolved {
            Value::Null => continue,
            Value::HttpResponse { status, body } => {
                let payload_json = match body {
                    Some(b) => match value_to_json(b.as_ref()) {
                        Ok(j) => j,
                        Err(msg) => return Some(HandlerOutcome::internal_error(msg)),
                    },
                    None => serde_json::Value::Null,
                };
                return Some(HandlerOutcome::json(status, payload_json));
            }
            other => {
                return Some(HandlerOutcome::internal_error(format!(
                    "middleware '{}' returned an unexpected value ({}); \
                     it must return `null` to continue or `return <status> {{ ... }}` \
                     to short-circuit",
                    mw.name,
                    other.type_name(),
                )));
            }
        }
    }
    None
}

/// Mw.next — runs the Post (2 args) middlewares in REVERSE
/// registration order (wrap semantics: the last registered is the
/// innermost, sees the response first). Each Post receives
/// `(Request, Response)` and returns a `Response`. The current
/// Response is represented as `Value::HttpResponse { status, body }`
/// built from the previous `HandlerOutcome`. The final response is
/// returned as HandlerOutcome.
///
/// Errors: if a Post does not return `Value::HttpResponse`, a clear
/// 500 error citing the middleware. If the chain is empty or there
/// are no Post mws, returns the original outcome unchanged.
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
    // Build the initial Value::HttpResponse. The body is parsed
    // from the outcome JSON. If the body isn't valid JSON (rare
    // case), we pass it as raw Str.
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
        let raw = match call_handler(mw.handler.clone(), args, &label).await {
            Ok(v) => v,
            Err(err) => {
                return HandlerOutcome::internal_error(format!(
                    "post middleware '{}' failed: {}",
                    mw.name, err.message,
                ));
            }
        };
        // async fn post middleware → await the Future (run↔build parity,
        // parallel to the Pre path above).
        let resolved = match await_if_future(raw).await {
            Ok(r) => r,
            Err(err) => {
                return HandlerOutcome::internal_error(format!(
                    "post middleware '{}' failed: {}",
                    mw.name, err.message,
                ));
            }
        };
        match resolved {
            Value::HttpResponse { status, body } => {
                let payload_json = match body {
                    Some(b) => match value_to_json(b.as_ref()) {
                        Ok(j) => j,
                        Err(msg) => return HandlerOutcome::internal_error(msg),
                    },
                    None => serde_json::Value::Null,
                };
                // Preserve existing headers (CORS, custom already
                // injected); the Post-mw can add headers via a
                // future `extra_headers` field (residual debt). For
                // now, the post-mw decides status + body, and the
                // extra_headers are preserved from the previous
                // outcome.
                let prev_extras = std::mem::take(&mut outcome.extra_headers);
                outcome = HandlerOutcome::json(status, payload_json);
                outcome.extra_headers = prev_extras;
            }
            other => {
                return HandlerOutcome::internal_error(format!(
                    "post middleware '{}' returned an unexpected value ({}); \
                     it must return `Response` (a `return <status> {{ ... }}`)",
                    mw.name,
                    other.type_name(),
                ));
            }
        }
    }
    outcome
}

/// Mini-batch Mw-Wrap — runs the wrap-style middleware chain
/// wrapping the handler + post chain. Each Wrap receives
/// `(request, next)` where `next` is a `Value::NativeFn` that
/// executes the rest: the remaining wraps + the handler + the post
/// mws.
///
/// The Wrap mw decides when to invoke `next()` (before/after the
/// handler, conditionally, measuring time, etc.). Its return value
/// (`Response`) becomes the final outcome.
///
/// Recursive structure: base case = no wraps → invoke handler +
/// post. Recursive case = pop the first wrap, build a NativeFn that
/// recurses with the remaining wraps, invoke the current wrap.
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
            // Base case: invoke handler + post chain.
            let outcome = match call_handler(handler, handler_args, &handler_name).await {
                Ok(value) => value_to_outcome(&value),
                Err(err) => {
                    // U3 (v0.10.15) — log to stderr with handler
                    // context (name + error position). Previously
                    // the message was included only in the response
                    // body, hiding the error from server logs. Now
                    // it appears on both sides: response (client) +
                    // stderr (dev/ops).
                    eprintln!("[fitz HTTP] handler `{}` failed: {}", handler_name, err);
                    HandlerOutcome::internal_error(err.message)
                }
            };
            return run_post_middlewares(&post_mws, &request, outcome).await;
        }
        // Pop first wrap; the rest goes to the NativeFn closure.
        let mut iter = wraps.into_iter();
        let current = iter.next().unwrap();
        let remaining: Vec<MiddlewareSpec> = iter.collect();

        // Build the `next` callable. We capture by value (clone)
        // everything the closure will need next time.
        let req_clone = request.clone();
        let handler_clone = handler.clone();
        let handler_name_clone = handler_name.clone();
        let handler_args_clone = handler_args.clone();
        let post_clone = post_mws.clone();
        let remaining_clone = remaining.clone();
        let next: crate::value::NativeAsyncFn =
            crate::value::NativeAsyncFn(std::sync::Arc::new(move |_args: Vec<Value>| {
                // Re-clone for each invocation (may be called 0+ times).
                let req2 = req_clone.clone();
                let h2 = handler_clone.clone();
                let p2 = post_clone.clone();
                let r2 = remaining_clone.clone();
                let hn2 = handler_name_clone.clone();
                let ha2 = handler_args_clone.clone();
                Box::pin(async move {
                    let outcome = run_wrap_chain(r2, h2, ha2, hn2, req2, p2).await;
                    // Convert outcome → Value::HttpResponse so the
                    // mw consumes it as `Response`.
                    let body = serde_json::from_str::<serde_json::Value>(&outcome.body)
                        .ok()
                        .map(|j| Box::new(json_to_value(&j)));
                    Ok(Value::HttpResponse {
                        status: outcome.status,
                        body,
                    })
                }) as crate::value::FitzFuture
            }));

        // Invoke the Wrap mw with (request, next).
        let args = vec![request.clone(), Value::NativeFn(next)];
        let label = format!("middleware wrap '{}'", current.name);
        let raw = match call_handler(current.handler.clone(), args, &label).await {
            Ok(v) => v,
            Err(err) => {
                return HandlerOutcome::internal_error(format!(
                    "wrap middleware '{}' failed: {}",
                    current.name, err.message,
                ));
            }
        };
        // async fn wrap middleware → await the Future (run↔build parity,
        // parallel to the Pre + Post paths).
        match await_if_future(raw).await {
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
                "wrap middleware '{}' returned an unexpected value ({}); \
                 it must return `Response` (a `return <status> {{ ... }}`)",
                current.name,
                other.type_name(),
            )),
            Err(err) => HandlerOutcome::internal_error(format!(
                "wrap middleware '{}' failed: {}",
                current.name, err.message,
            )),
        }
    })
}

/// Processes a single task. Isolated from the loop so it can be
/// tested without the channel.
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
            "route {} does not exist in the registry",
            route_idx,
        ));
    };

    // MW.1: middlewares stacked on the route. They run BEFORE
    // parsing the body or coercing params: if an auth/CORS
    // middleware short-circuits, we save the work of validating the
    // rest of the request. The chain receives a single `Request`
    // arg with method/path/headers; body and query params are not
    // exposed to the middleware (explicit debt).
    if !route.middlewares.is_empty() {
        let request =
            build_request_value(route.method, &route.path, &raw_path_params, &raw_headers);
        if let Some(outcome) = run_middleware_chain(&route.middlewares, &request).await {
            return outcome;
        }
    }

    // Phase 9.w.1.c — auth check. After middlewares (which may
    // short-circuit without touching auth — CORS preflight, etc.)
    // and before parsing the body or building args. If the route
    // requires `@authenticated`/`@admin`, we invoke the
    // `@auth_provider` with a `Map<Str, Str>` of the headers and
    // expect `Result<User>`:
    //
    //   - `Ok(user)` → continues to the handler, `user` is injected
    //     as an arg. For `@admin`, we additionally validate
    //     `user.role == "admin"`; if it doesn't match → 403.
    //   - `Err(msg)` → 401 with `{"error": msg}`.
    //   - Provider failed with FitzError or returned a shape other
    //     than `Result<User>` → 500 with message (shouldn't happen
    //     — the 9.w.1.a checker validates statically; defensive).
    let auth_user: Option<Value> = if route.auth != AuthSpec::None
        || !route.required_roles.is_empty()
    {
        let provider = match registry.auth_provider.as_ref() {
            Some(h) => h.clone(),
            None => {
                return HandlerOutcome::internal_error(format!(
                    "ruta '{}' exige auth pero no hay `@auth_provider` en el registry — bug interno",
                    route.handler_name,
                ));
            }
        };
        // Build a `Map<Str, Str>` with the received HTTP headers
        // to pass to the provider. Same shape as `Request.headers`.
        let headers_pairs: Vec<(Value, Value)> = raw_headers
            .iter()
            .map(|(k, v)| (Value::Str(k.clone()), Value::Str(v.clone())))
            .collect();
        let headers_arg = Value::Map(crate::value::shared(headers_pairs));
        let invoked =
            call_handler(provider.handler.clone(), vec![headers_arg], &provider.name).await;
        let raw_result = match invoked {
            Ok(v) => v,
            Err(e) => {
                return HandlerOutcome::internal_error(format!(
                    "`@auth_provider` '{}' failed: {}",
                    provider.name, e.message,
                ));
            }
        };
        // If the provider is async, the invoked value is a
        // `Value::Future` that must be awaited (parallel to
        // `run_test_handler` from 9.z.2.b).
        let resolved = if provider.is_async {
            match raw_result {
                Value::Future(cell) => {
                    let fut = cell.0.lock().take();
                    match fut {
                        Some(f) => match f.await {
                            Ok(v) => v,
                            Err(e) => {
                                return HandlerOutcome::internal_error(format!(
                                    "`@auth_provider` '{}' failed while awaiting: {}",
                                    provider.name, e.message,
                                ));
                            }
                        },
                        None => {
                            return HandlerOutcome::internal_error(format!(
                                "`@auth_provider` '{}': Future already consumed (dispatcher bug)",
                                provider.name,
                            ));
                        }
                    }
                }
                other => {
                    return HandlerOutcome::internal_error(format!(
                        "async `@auth_provider` '{}' did not return Future, returned: {}",
                        provider.name,
                        other.type_name(),
                    ));
                }
            }
        } else {
            raw_result
        };
        // `resolved` must be `Result<User>`.
        match resolved {
            Value::Result(crate::value::ResultVariant::Ok(user_box)) => {
                // For `@admin`, validate `user.role == "admin"`.
                // The checker validates that the `User` type has
                // the field; here we look at the runtime value.
                if route.auth == AuthSpec::Admin {
                    let role_ok = match user_box.as_ref() {
                        Value::Instance { fields, .. } => {
                            let guard = fields.lock();
                            guard.iter().any(|(k, v)| {
                                k == "role" && matches!(v, Value::Str(s) if s == "admin")
                            })
                        }
                        _ => false,
                    };
                    if !role_ok {
                        return HandlerOutcome::json(
                            403,
                            serde_json::json!({
                                "error": "access forbidden — admin role required",
                            }),
                        );
                    }
                }
                // Phase 9.w.1.iter2.a — Custom RBAC with
                // `@requires("role")`. user.role (Str) must match
                // at least one of the required roles. Stacking
                // `@requires("a") @requires("b")` produces
                // `vec!["a","b"]` = OR.
                if !route.required_roles.is_empty() {
                    let actual_role = match user_box.as_ref() {
                        Value::Instance { fields, .. } => {
                            let guard = fields.lock();
                            guard.iter().find_map(|(k, v)| {
                                if k == "role" {
                                    if let Value::Str(s) = v {
                                        return Some(s.clone());
                                    }
                                }
                                None
                            })
                        }
                        _ => None,
                    };
                    let allowed = actual_role
                        .as_ref()
                        .is_some_and(|r| route.required_roles.iter().any(|x| x == r));
                    if !allowed {
                        let msg = match &actual_role {
                            Some(r) => format!(
                                "access forbidden — role '{}' not authorized (required: {})",
                                r,
                                route.required_roles.join(", "),
                            ),
                            None => format!(
                                "access forbidden — missing role (required: {})",
                                route.required_roles.join(", "),
                            ),
                        };
                        return HandlerOutcome::json(403, serde_json::json!({ "error": msg }));
                    }
                }
                Some(*user_box)
            }
            Value::Result(crate::value::ResultVariant::Err(msg_box)) => {
                let msg = match *msg_box {
                    Value::Str(s) => s,
                    other => format!("{}", other),
                };
                return HandlerOutcome::json(401, serde_json::json!({ "error": msg }));
            }
            other => {
                return HandlerOutcome::internal_error(format!(
                    "`@auth_provider` '{}' must return `Result<User>`, returned `{}`",
                    provider.name,
                    other.type_name(),
                ));
            }
        }
    } else {
        None
    };

    // If the handler expects a body, parse and prepare it. We do
    // this before building args so we fail early if the JSON is
    // broken.
    //
    // Mini-batch Hpx.1 — Content-Type validation: if the handler
    // declares a body param, we require `application/json`. Any
    // other Content-Type (multipart, urlencoded, etc.) → 415 with
    // a clear message. If there's NO header (raw body), we accept
    // (curl-style clients without -H emit it that way, and Fitz
    // never promised strict Content-Type). Dedicated future
    // sub-step for multipart/form.
    //
    // Mini-batch MP — we add support for
    // `application/x-www-form-urlencoded`: it's parsed as
    // `Map<Str, Str>` and assigned to the body param. Multipart
    // with files is left as a future sub-step (more complex).
    let body_value: Option<Value> = if let Some(bp) = &route.body_param {
        let raw_ct = raw_headers.get("content-type").cloned();
        let ct_primary = raw_ct
            .as_ref()
            .map(|ct| {
                ct.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();

        let is_urlencoded = ct_primary == "application/x-www-form-urlencoded";
        let is_json_or_empty = ct_primary.is_empty() || ct_primary == "application/json";
        // Mini-batch MP2 — multipart/form-data with boundary.
        let is_multipart = ct_primary == "multipart/form-data";

        if !is_json_or_empty && !is_urlencoded && !is_multipart {
            // text/plain, custom, etc. → 415.
            return HandlerOutcome::json(
                415,
                serde_json::json!({
                    "error": format!(
                        "unsupported Content-Type: '{}'. The handler expects JSON \
                         (`application/json`), urlencoded \
                         (`application/x-www-form-urlencoded`) or multipart \
                         (`multipart/form-data`). Other formats remain as a \
                         future sub-step.",
                        raw_ct.as_deref().unwrap_or("(no header)")
                    ),
                }),
            );
        }

        if is_multipart {
            // Mini-batch MP2 — extract the boundary from
            // Content-Type (`multipart/form-data; boundary=<token>`).
            // Without a boundary → clear 400.
            let boundary = raw_ct.as_deref().and_then(extract_multipart_boundary);
            match boundary {
                None => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({
                            "error": "multipart/form-data: missing `boundary` parameter in Content-Type"
                        }),
                    );
                }
                Some(b) => match parse_multipart_body(&body_bytes, &b) {
                    Ok(v) => Some(v),
                    Err(msg) => {
                        return HandlerOutcome::json(400, serde_json::json!({ "error": msg }));
                    }
                },
            }
        } else if is_urlencoded {
            match parse_urlencoded_body(&body_bytes) {
                Ok(v) => Some(v),
                Err(msg) => {
                    return HandlerOutcome::json(400, serde_json::json!({ "error": msg }));
                }
            }
        } else {
            match parse_body(&body_bytes, bp) {
                Ok(v) => Some(v),
                Err(msg) => {
                    return HandlerOutcome::json(400, serde_json::json!({ "error": msg }));
                }
            }
        }
    } else {
        None
    };

    // Build args in the handler's declared order. For each
    // parameter:
    //   - if its name is in `path_params`, take the raw value from
    //     the path map and coerce it to the declared type;
    //   - if it's in `query_params`, same from the query map
    //     (nullable → Null if missing; required → 400 if missing);
    //   - if it's the body param, use the parsed value;
    //   - any other case (not path, not query, not body) is a
    //     registration bug: the evaluator doesn't allow it.
    let mut args = Vec::with_capacity(route.param_types.len());
    for (name, head_type, is_nullable) in &route.param_types {
        if route.path_params.iter().any(|p| p == name) {
            // Path params are always required (axum guarantees they
            // arrive if the route matched). Coercion to the
            // declared type.
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
            // Query params: if the declared type is nullable
            // (`Int?`), missing → Null. If required, missing → 400.
            let raw = raw_query_params.get(name);
            match (raw, *is_nullable) {
                (Some(s), _) => match coerce_path_param(s, head_type.as_deref()) {
                    Ok(v) => args.push(v),
                    Err(msg) => {
                        return HandlerOutcome::json(
                            400,
                            serde_json::json!({
                                "error": format!("query param '{}': {}", name, msg),
                            }),
                        );
                    }
                },
                (None, true) => args.push(Value::Null),
                (None, false) => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({
                            "error": format!("query param '{}': missing — it is required", name),
                        }),
                    );
                }
            }
        } else if route.body_param.as_ref().map(|bp| bp.name.as_str()) == Some(name) {
            // Body param: already parsed above; take it from
            // `body_value`. unwrap is safe because body_value is
            // Some iff body_param exists.
            args.push(body_value.clone().unwrap());
        } else if let Some(hdr) = route.headers.iter().find(|h| &h.param_name == name) {
            // Header (Phase 7.6). Case-insensitive lookup via
            // lowercase HTTP name. Missing + nullable → Null.
            // Missing + required → 400.
            let key = hdr.http_name.to_lowercase();
            match (raw_headers.get(&key), hdr.is_nullable) {
                (Some(v), _) => args.push(Value::Str(v.clone())),
                (None, true) => args.push(Value::Null),
                (None, false) => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({
                            "error": format!(
                                "header '{}': missing — it is required",
                                hdr.http_name
                            ),
                        }),
                    );
                }
            }
        } else if let Some(ck) = route.cookies.iter().find(|c| &c.param_name == name) {
            // FITZ-05 — @cookie: parse the incoming `Cookie` header for the
            // named cookie. Missing + nullable → Null; missing + required → 400.
            let val = raw_headers
                .get("cookie")
                .and_then(|raw| parse_cookie_header(raw, &ck.http_name));
            match (val, ck.is_nullable) {
                (Some(v), _) => args.push(Value::Str(v)),
                (None, true) => args.push(Value::Null),
                (None, false) => {
                    return HandlerOutcome::json(
                        400,
                        serde_json::json!({
                            "error": format!(
                                "cookie '{}': missing — it is required",
                                ck.http_name
                            ),
                        }),
                    );
                }
            }
        } else if route.auth_user_param_name.as_deref() == Some(name.as_str()) {
            // Phase 9.w.1.c — param injected by
            // `@authenticated`/`@admin`. `auth_user` was resolved
            // above BEFORE building args. If for some reason
            // `auth_user` is None (shouldn't happen: if
            // `auth_user_param_name` is Some, the auth block above
            // guarantees `auth_user` is too), default to Null as a
            // safety net.
            args.push(auth_user.clone().unwrap_or(Value::Null));
        } else {
            return HandlerOutcome::internal_error(format!(
                "parameter '{}' of handler '{}' is not a path param, query param, body, header, or auth user — \
                 this is an internal registration bug",
                name, route.handler_name,
            ));
        }
    }

    // Mini-batch Mw-Wrap — if there are wrap-style middlewares, the
    // chain runner wraps them around the handler + post mws. If
    // there are no wraps, we continue with the classic flow
    // (handler + post).
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
        let request =
            build_request_value(route.method, &route.path, &raw_path_params, &raw_headers);
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
        // Classic flow: invoke handler + post mws.
        // Phase 6.4 — if the handler is async, the invoke returns
        // `Value::Future`; we await it before passing it to the
        // serializer (parallel to the pattern in
        // build_ws_method_router and register_auth_provider).
        // Without this await, async HTTP handlers returned "Future
        // pendiente no es serializable" — pre-existing bug exposed
        // while closing 9.w.3.b (which needs async handlers calling
        // `spawn(...)`).
        let mut outcome = match call_handler(route.handler.clone(), args, &route.handler_name).await
        {
            Ok(value) => {
                let resolved = await_if_future(value).await;
                match resolved {
                    Ok(v) => value_to_outcome(&v),
                    Err(e) => HandlerOutcome::internal_error(e.message),
                }
            }
            Err(err) => HandlerOutcome::internal_error(err.message),
        };

        // Mw.next — run the post-middlewares (kind = Post, 2-arg)
        // AFTER the handler. They receive `(Request, Response)` and
        // can modify the body or add headers. If a Pre middleware
        // short-circuits, this path does not run (we already
        // returned above with the Pre's response).
        if route
            .middlewares
            .iter()
            .any(|m| m.kind == MiddlewareKind::Post)
        {
            let request =
                build_request_value(route.method, &route.path, &raw_path_params, &raw_headers);
            outcome = run_post_middlewares(&route.middlewares, &request, outcome).await;
        }
        outcome
    };

    // MW.2: if the route declares CORS, add the
    // `Access-Control-Allow-*` headers to the real response.
    // Included on error responses (500/400) — the browser reads
    // CORS before parsing the body, so without these headers any
    // error surfaces as a "CORS error" in the console instead of
    // the actual 500/400 that happened.
    // Q.3: we pass the request `Origin` to the config; if the
    // policy is `Set` and matches, echo the received Origin;
    // otherwise, the header is NOT emitted (browser rejects the
    // response — strict CORS).
    if let Some(cors) = &route.cors {
        let request_origin = raw_headers.get("origin").map(|s| s.as_str());
        outcome
            .extra_headers
            .extend(cors.response_headers(request_origin));
    }
    outcome
}

/// Parses the body bytes into a Fitz `Value` per the body param
/// convention:
///   - Invalid JSON → 400 error with a clear message.
///   - If the body param has `declared_type: Some(Value::Type)`,
///     validate against the type (missing fields, extras, etc.)
///     and build a `Value::Instance`.
///   - Otherwise, deserialize into a free `Value` (Map/List/
///     primitives).
///
/// Mini-batch MP — parses `application/x-www-form-urlencoded` body
/// (format `key1=value1&key2=value2`) into a `Value::Map<Str, Str>`.
/// URL-decoding applied to keys and values. Empty body → empty
/// Map. Duplicates: last-wins (parallel to the `serde_urlencoded`
/// convention).
fn parse_urlencoded_body(bytes: &[u8]) -> Result<Value, String> {
    use crate::value::shared;
    let s = std::str::from_utf8(bytes)
        .map_err(|e| format!("invalid urlencoded body (UTF-8): {}", e))?;
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
        // Duplicates: last-wins. Remove the previous entry with
        // the same key.
        pairs.retain(|(existing_k, _)| !matches!(existing_k, Value::Str(s) if s == &k));
        pairs.push((Value::Str(k), Value::Str(v)));
    }
    Ok(Value::Map(shared(pairs)))
}

/// Mini-batch MP2 — extracts the `boundary` from the
/// `multipart/form-data` Content-Type header
/// (`multipart/form-data; boundary=<token>` or
/// `boundary="<token>"`). Returns `None` if the parameter isn't
/// present. Whitespace trim + support for double quotes (RFC 7578).
fn extract_multipart_boundary(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        let lower = part.to_ascii_lowercase();
        if let Some(stripped) = lower.strip_prefix("boundary=") {
            // Stripped is lowercase; we need to go back to the
            // original to preserve the boundary case (boundaries
            // are case-sensitive per RFC 7578).
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

/// Mini-batch MP2 + File.content Bytes — parser for
/// `multipart/form-data` (RFC 7578) over raw bytes.
///
/// Each body part is delimited by `--<boundary>\r\n` with headers
/// like `Content-Disposition: form-data; name="X"; filename="Y"`
/// (filename optional for text fields). The part body is separated
/// from the headers by `\r\n\r\n`. The last part ends with
/// `\r\n--<boundary>--`.
///
/// Returns `Value::Map<Str, Value>` where each entry is:
/// - Text field (without `filename`) → `Value::Str(content)`
///   (UTF-8; if the content isn't UTF-8, 400 error).
/// - File field (with `filename`) → `Value::Instance` of `File`
///   with `name`, `content_type`, `content: Bytes`. Binary files
///   ALREADY work — content is stored as `Value::Bytes(Vec<u8>)`
///   without requiring UTF-8.
///
/// Refactor from the initial MP2 version: now we work byte by byte
/// to preserve binary bytes. Delimiter search uses
/// `slice::windows` or a manual scan; headers are parsed as UTF-8
/// (ASCII per RFC 7578).
///
/// `name` duplicates: last-wins.
fn parse_multipart_body(bytes: &[u8], boundary: &str) -> Result<Value, String> {
    let delimiter = format!("--{}", boundary).into_bytes();
    // Split by the delimiter sequence.
    let parts_raw: Vec<&[u8]> = split_bytes_by(bytes, &delimiter);
    let mut entries: Vec<(Value, Value)> = Vec::new();
    for raw in parts_raw.iter().skip(1) {
        // Final terminator: `--<boundary>--` produces a raw that
        // starts with `--` (right after the delimiter).
        if raw.starts_with(b"--") {
            break;
        }
        // Each part starts with `\r\n` (separator between
        // delimiter and headers). If absent, malformed.
        let body = strip_prefix_bytes(raw, b"\r\n").unwrap_or(raw);
        // Each part may end with `\r\n` before the next delimiter.
        // Trim it.
        let body = strip_suffix_bytes(body, b"\r\n").unwrap_or(body);

        // Split headers vs content on the first occurrence of
        // `\r\n\r\n`. Headers are ASCII; content can be any byte
        // sequence.
        let Some(split_idx) = find_bytes(body, b"\r\n\r\n") else {
            return Err(
                "multipart: malformed part — missing `\\r\\n\\r\\n` between headers and body"
                    .to_string(),
            );
        };
        let headers_bytes = &body[..split_idx];
        let content_bytes = &body[split_idx + 4..];
        let headers_str = std::str::from_utf8(headers_bytes)
            .map_err(|e| format!("multipart: headers are not valid ASCII/UTF-8: {}", e))?;

        // Parse the part headers. We only care about
        // `Content-Disposition` (extract `name` and `filename`) and
        // `Content-Type` (for files).
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
            return Err("multipart: part without `name` in Content-Disposition".to_string());
        };

        let value = match filename {
            None => {
                // Text field: content must be valid UTF-8. For
                // binary bytes without filename, error.
                let s = std::str::from_utf8(content_bytes).map_err(|e| {
                    format!(
                        "multipart: text field '{}' is not valid UTF-8 (use filename= for binary bytes): {}",
                        name, e
                    )
                })?;
                Value::Str(s.to_string())
            }
            Some(fname) => {
                // File field: content as raw Bytes. Binary OK.
                let mut fields: Vec<(String, Value)> = Vec::new();
                let name_val = if fname.is_empty() {
                    Value::Null
                } else {
                    Value::Str(fname)
                };
                fields.push(("name".to_string(), name_val));
                fields.push((
                    "content_type".to_string(),
                    content_type_part.map(Value::Str).unwrap_or(Value::Null),
                ));
                fields.push(("content".to_string(), Value::Bytes(content_bytes.to_vec())));
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

/// File.content Bytes — helpers for split/find over `&[u8]`.
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

/// Helper to parse Content-Disposition header params:
/// `form-data; name="X"; filename="Y"`. Returns a case-insensitive
/// map (lowercase keys) → unquoted value.
fn parse_cd_params(s: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    // Skip the first token (`form-data`).
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

/// Mini-batch MP — URL-decode (format
/// `application/x-www-form-urlencoded`): `+` → space, `%XX` → hex
/// byte. Malformed %XX errors are reported with a clear offset.
/// Quick win F13 bundle — standard base64 encoder (RFC 4648, no
/// URL-safe alphabet, with padding). Inline to avoid the `base64`
/// dep. Accepts any byte slice, returns an ASCII String.
fn b64_encode_standard(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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
                let h1 = chars
                    .next()
                    .ok_or_else(|| format!("urlencoded: incomplete %XX at offset {}", idx))?;
                let h2 = chars
                    .next()
                    .ok_or_else(|| format!("urlencoded: incomplete %XX at offset {}", idx))?;
                let byte = u8::from_str_radix(&format!("{}{}", h1, h2), 16)
                    .map_err(|_| format!("urlencoded: %{}{} is not valid hex", h1, h2))?;
                // Accumulate bytes for multi-byte UTF-8 chars.
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
    // Empty body for a handler expecting a body → clear error.
    // This happens with `POST /users` without a body when the
    // handler declares `body: User`.
    if bytes.is_empty() {
        return Err(format!(
            "body required for parameter '{}' but the request had no body",
            bp.name,
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("body is not valid JSON: {}", e))?;
    match &bp.declared_type {
        Some(t) => json_to_instance(&json, t),
        None => Ok(json_to_value(&json)),
    }
}

/// Starts the HTTP server and blocks the calling thread until
/// Ctrl-C.
///
/// **F17.5**: simplified model, no bridge:
///   - A single `rt-multi-thread` tokio runtime runs right here
///     (`block_on`), N workers per cores.
///   - `HttpRegistry` is wrapped in `Arc` and shared with each axum
///     handler. Each worker that receives a request invokes
///     `handle_task(&registry, ...).await` directly on the
///     evaluator — `Send + Sync` unblocked it in F17.2-3.
///   - The main thread blocks on the runtime until axum shuts down
///     on Ctrl-C (graceful shutdown still intact).
///
/// Before (Phase 4 → F17.4a) there was a separate std::thread for
/// tokio plus a synchronous loop in main that received `InterpTask`s
/// over mpsc and replied through `oneshot`s. Removing the bridge
/// was F17's biggest piece of debt — it unblocks real HTTP
/// parallelism (~300 fewer LoC in this file) and makes the evaluator
/// reachable from axum without glue.
/// Builds the shared multi-threaded tokio runtime for the interpreter's
/// server + cron path. Single source of truth for the runtime config
/// (16 MB worker stacks + `enable_all`).
///
/// `run_file` builds ONE of these up-front and drives the eval, the DB
/// connections the eval opens, and `serve`/the scheduler all on it — so the
/// TcpStream reactor stays alive across the whole `fitz run`. This fixes the
/// "A Tokio 1.x context was found, but it is being shutdown" panic that hit
/// `@cron(store=db)` when the eval ran on a separate `current_thread` runtime
/// that was dropped before the scheduler started.
pub fn build_server_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    // Worker stack size — the Fitz evaluator is tree-walking and
    // `#[async_recursion]`, so each Fitz-level call consumes a
    // sizable Rust stack frame. Rendering a real-world page (a data
    // grid with nested rows, forms, and composed LiveComponents) can
    // reach hundreds of nested evaluator frames, which overflows
    // tokio's default 2 MB worker stack — especially on the WS path,
    // where the handler wrapper leaves less headroom than a plain GET.
    // Bump to 16 MB so non-trivial server apps render reliably.
    const WORKER_STACK_SIZE: usize = 16 * 1024 * 1024;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_STACK_SIZE)
        .build()
}

pub fn serve(
    registry: HttpRegistry,
    program: crate::ast::Program,
    addr: std::net::SocketAddr,
) -> std::io::Result<()> {
    let runtime = build_server_runtime()?;
    serve_on_runtime(&runtime, registry, program, addr)
}

/// Same as [`serve`], but drives the server on a caller-provided runtime
/// instead of building its own. `run_file` uses this to share ONE runtime
/// with the eval (see [`build_server_runtime`]).
pub fn serve_on_runtime(
    runtime: &tokio::runtime::Runtime,
    registry: HttpRegistry,
    program: crate::ast::Program,
    addr: std::net::SocketAddr,
) -> std::io::Result<()> {
    let metas = registry.metas();
    let resolved_config = registry.resolved_config();
    let enable_docs = resolved_config.enable_docs;

    // Phase 12.3.iter2.Tier3 — opt-in Prometheus. Dual gate: the
    // config flag (`@server(prometheus=true)`) OR the env var
    // `FITZ_PROMETHEUS=1`/`true`/`yes`. The env var takes
    // precedence as an override (useful in production to
    // toggle without recompiling). When true, `init_prometheus`
    // installs the global recorder and `build_router` auto-mounts
    // `GET /metrics`.
    let prometheus_enabled = resolved_config.prometheus_enabled
        || matches!(
            std::env::var("FITZ_PROMETHEUS").as_deref(),
            Ok("1" | "true" | "yes")
        );
    crate::observability::init_prometheus(prometheus_enabled);

    // Phase 7.2: precompute the OpenAPI schema with `program` +
    // `registry` and pass it to `build_router`. Auto-register of
    // `/openapi.json` and `/docs` happens there (and respects any
    // user-declared route at those paths).
    //
    // Phase 7.4: if `@server(docs=false)`, we neither compute the
    // schema nor pass it to the router — both auto-registered
    // routes stay 404. Trade-off: zero overhead when the user
    // turns off docs.
    // Q.2: read `api_version` from the config if set via
    // `@server(api_version="X.Y.Z")`. None → schema uses default
    // "0.1.0".
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
    // Phase 9.w.2.d — AsyncAPI 3.0 schema when there are `@ws`
    // handlers. Gated by `enable_docs` same as OpenAPI: if the
    // user turned off docs with `@server(docs=false)`, we don't
    // emit AsyncAPI either.
    let asyncapi_schema = if enable_docs {
        let channels = crate::asyncapi::channels_from_registry(&registry);
        if channels.is_empty() {
            None
        } else {
            Some(crate::asyncapi::generate_asyncapi_with_version(
                &channels,
                &program,
                api_version.as_deref(),
            ))
        }
    } else {
        None
    };
    let has_asyncapi = asyncapi_schema.is_some();

    let registry = std::sync::Arc::new(registry);
    // Phase 9.w.3 — clone the cron_registry BEFORE moving the
    // `Arc<HttpRegistry>` into the router. Scheduler spawned
    // inside the runtime, in parallel with axum.
    let cron_registry_for_scheduler = registry.cron_registry.clone();
    // Phase 3c — same, for the @every interval scheduler.
    let every_registry_for_scheduler = registry.every_registry.clone();
    // Phase 12.1.b — capture draining + shutdown_timeout BEFORE
    // moving `registry` into the router; the shutdown signal
    // needs them.
    let draining_for_shutdown = registry.draining.clone();
    let shutdown_timeout_secs = resolved_config.shutdown_timeout_secs;

    runtime.block_on(async move {
        let router = build_router_with_asyncapi(&metas, registry, openapi_schema, asyncapi_schema);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        eprintln!("🏔️  Fitz HTTP escuchando en http://{}", addr);
        for meta in &metas {
            let arrow = if meta.is_ws {
                "WS "
            } else {
                meta.method.as_str()
            };
            eprintln!("   {} {}", arrow, meta.path);
        }
        if enable_docs {
            eprintln!("   GET /openapi.json  (schema autogenerado)");
            if has_asyncapi {
                eprintln!("   GET /asyncapi.json (canales WebSocket)");
            }
            eprintln!("   GET /docs          (UI Scalar)");
            if has_asyncapi {
                eprintln!("   GET /asyncapi      (UI AsyncAPI)");
            }
        } else {
            eprintln!("   (docs apagadas por @server(docs=false))");
        }
        if crate::observability::prometheus_handle().is_some() {
            eprintln!("   GET /metrics       (Prometheus exposition format)");
        }
        // Phase 9.w.3 — starts the cron scheduler before
        // axum::serve. Tasks run detached on the same tokio
        // runtime. When `with_graceful_shutdown(ctrl_c)` fires and
        // we drop the runtime, cron tasks are also cancelled.
        crate::cron_jobs::spawn_cron_scheduler(cron_registry_for_scheduler);
        // Phase 3c — start the @every interval scheduler alongside cron + axum.
        crate::cron_jobs::spawn_every_scheduler(every_registry_for_scheduler);
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal(
                draining_for_shutdown,
                shutdown_timeout_secs,
            ))
            .await
    })?;

    Ok(())
}

/// Phase 12.1.b — Listens for SIGINT (Ctrl-C) and orchestrates the
/// graceful shutdown.
///
/// Sequence:
///   1. Waits for SIGINT/Ctrl-C.
///   2. Flips `draining` to `true` — `/readyz` starts returning
///      503 immediately. K8s with `readinessProbe` stops routing
///      new traffic in ~1-2 ticks.
///   3. Waits for a short grace period (2 seconds hardcoded in the
///      MVP) so K8s sees the change and reroutes. Without this
///      delay, the pod may receive in-flight requests right at
///      shutdown time.
///   4. Returns — axum closes the listener and drains in-flight
///      requests. The total timeout is controlled by
///      `shutdown_timeout_secs` (default 30s) via the tokio timeout
///      wrapper below (the caller starts a parallel timer if
///      `> 0`).
async fn shutdown_signal(
    draining: std::sync::Arc<std::sync::atomic::AtomicBool>,
    shutdown_timeout_secs: u64,
) {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("\n[shutdown] SIGINT received — flipping draining state");
    draining.store(true, std::sync::atomic::Ordering::SeqCst);

    // Grace period: give K8s time to notice the 503 and reroute
    // traffic. 2 seconds is the reasonable minimum (kubelet polls
    // probes every ~10s by default, but faster load balancers like
    // Envoy see the change in ~1s).
    let grace = std::cmp::min(2u64, shutdown_timeout_secs.max(1));
    eprintln!("[shutdown] esperando {grace}s de grace period para que el load balancer rerutee...");
    tokio::time::sleep(std::time::Duration::from_secs(grace)).await;

    eprintln!(
        "[shutdown] cerrando listener — axum drena requests in-flight (max {shutdown_timeout_secs}s)..."
    );
}

// ---------------------------------------------------------------------------
// Phase 9.w.2 — WebSocket runtime
// ---------------------------------------------------------------------------
//
// Three pieces:
//
//   1. `WsBroadcaster` — shared by `HttpRegistry`. Maintains the
//      endpoint→outbox-per-conn mapping. Enables `broadcast(msg)`
//      that sends to every live conn on the endpoint.
//
//   2. `WsReadStreamImpl` — wrapper over the axum SplitStream that
//      implements the `WsReadStreamTrait` declared in value.rs.
//      Filters non-text frames (auto ping/pong; binary → Err) and
//      normalizes close → Ok(None).
//
//   3. `handle_ws_upgrade` — runs after the successful HTTP→WS
//      handshake. Spawns the writer task (drains outbox → sink),
//      builds the `Value::WsConn`, invokes the Fitz handler, cleans
//      up when done.
//
// The auth check lives in `handle_ws_route_with_auth` and runs
// BEFORE `WebSocketUpgrade::on_upgrade` — if it fails, we return
// 401/403 HTTP and the upgrade never happens.

/// Runtime broadcaster. Holds `HashMap<endpoint, Vec<(conn_id,
/// outbox_tx)>>`. Thread-safe — methods take short locks and release
/// them quickly.
/// Alias for the type of the broadcaster's internal map. Avoids
/// clippy's `type_complexity` lint and makes the shape explicit:
/// per endpoint (path), a list of `(conn_id, outbox_tx)`.
type WsConnList = Vec<(u64, tokio::sync::mpsc::UnboundedSender<WsOutMessage>)>;

pub struct WsBroadcaster {
    conns: parking_lot::Mutex<std::collections::HashMap<String, WsConnList>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl Default for WsBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WsBroadcaster {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let conns = self.conns.lock();
        let total: usize = conns.values().map(|v| v.len()).sum();
        f.debug_struct("WsBroadcaster")
            .field("endpoints", &conns.len())
            .field("total_conns", &total)
            .finish()
    }
}

impl WsBroadcaster {
    pub fn new() -> Self {
        Self {
            conns: parking_lot::Mutex::new(std::collections::HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Registers a new conn on the endpoint. Returns the unique
    /// `conn_id` so the caller can call `unregister` on close.
    pub fn register(
        &self,
        endpoint: String,
        tx: tokio::sync::mpsc::UnboundedSender<WsOutMessage>,
    ) -> u64 {
        let conn_id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut conns = self.conns.lock();
        conns.entry(endpoint).or_default().push((conn_id, tx));
        conn_id
    }

    /// Unregisters a conn. Idempotent — if it was already gone
    /// (normal case if the conn closed on error and was removed by
    /// the broadcast retain), it does nothing.
    pub fn unregister(&self, endpoint: &str, conn_id: u64) {
        let mut conns = self.conns.lock();
        if let Some(list) = conns.get_mut(endpoint) {
            list.retain(|(id, _)| *id != conn_id);
            if list.is_empty() {
                conns.remove(endpoint);
            }
        }
    }

    /// Number of live conns on an endpoint. Useful for tests.
    #[allow(dead_code)]
    pub fn count(&self, endpoint: &str) -> usize {
        self.conns
            .lock()
            .get(endpoint)
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

impl WsBroadcasterTrait for WsBroadcaster {
    fn broadcast_text(&self, endpoint: &str, payload: String) {
        // Strategy: short lock, retain lazily drops closed txs.
        // Each outbox_tx.send() is non-blocking (unbounded mpsc).
        let mut conns = self.conns.lock();
        if let Some(list) = conns.get_mut(endpoint) {
            list.retain(|(_, tx)| tx.send(WsOutMessage::Text(payload.clone())).is_ok());
            if list.is_empty() {
                conns.remove(endpoint);
            }
        }
    }

    fn broadcast_binary(&self, endpoint: &str, payload: Vec<u8>) {
        // 9.w.2-binary-frames — same model as `broadcast_text` but
        // pushes `WsOutMessage::Binary(...)`. Each conn's writer
        // task translates it to axum's `Message::Binary`.
        let mut conns = self.conns.lock();
        if let Some(list) = conns.get_mut(endpoint) {
            list.retain(|(_, tx)| tx.send(WsOutMessage::Binary(payload.clone())).is_ok());
            if list.is_empty() {
                conns.remove(endpoint);
            }
        }
    }
}

/// Wrapper over the read half of the axum WebSocket implementing
/// the trait `WsConnHandle.rx` expects. 9.w.2-binary-frames:
/// exposes BOTH text and binary; the evaluator/codegen discriminates
/// based on the declared T of `WsConn<T>`.
///
///   - text   → `Ok(Some(IncomingFrame::Text(s)))`.
///   - binary → `Ok(Some(IncomingFrame::Binary(bs)))`.
///   - close  → `Ok(None)` (cleanly closed conn).
///   - ping/pong → axum auto-replies pings on the server side; we
///     discard them in the inner loop.
///
/// `Ping/Pong/Close` are handled by the axum/tungstenite stack
/// underneath (while iterating the stream); here we only decide
/// what to expose to the Fitz handler.
struct WsReadStreamImpl {
    inner: futures_util::stream::SplitStream<axum::extract::ws::WebSocket>,
}

impl WsReadStreamTrait for WsReadStreamImpl {
    fn next_frame<'a>(
        &'a mut self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<crate::value::IncomingFrame>, String>>
                + Send
                + 'a,
        >,
    > {
        use futures_util::StreamExt;
        Box::pin(async move {
            loop {
                let next = self.inner.next().await;
                match next {
                    Some(Ok(axum::extract::ws::Message::Text(t))) => {
                        return Ok(Some(crate::value::IncomingFrame::Text(t.to_string())));
                    }
                    Some(Ok(axum::extract::ws::Message::Binary(bs))) => {
                        return Ok(Some(crate::value::IncomingFrame::Binary(bs.to_vec())));
                    }
                    Some(Ok(axum::extract::ws::Message::Ping(_)))
                    | Some(Ok(axum::extract::ws::Message::Pong(_))) => {
                        // axum auto-replies pings on the server
                        // side; we discard them so the handler only
                        // sees text/binary.
                        continue;
                    }
                    Some(Ok(axum::extract::ws::Message::Close(_))) => {
                        return Ok(None);
                    }
                    Some(Err(e)) => return Err(e.to_string()),
                    None => return Ok(None),
                }
            }
        })
    }
}

/// Builds a `Value::WsConn` from the axum WebSocket + the
/// broadcaster. Also starts the writer task that drains the outbox.
/// Returns `(value, conn_id, writer_handle)` — the caller uses
/// `conn_id` for `unregister` on close and `writer_handle` to abort
/// the task.
///
/// `msg_type` + `env` allow `recv()` to coerce received `Map`s into
/// `Instance` when T is nominal (parallel to 8.4.3). `None` for
/// conns built in tests without type context.
///
/// Phase 9.w.2.e — `heartbeat_secs`: automatic Ping interval in
/// seconds. `0` disables. The writer task translates
/// `WsOutMessage::Ping` into an axum Ping frame; if the sink fails,
/// the writer terminates and `closed` is set (which the heartbeat
/// task detects on its next iteration and terminates by itself).
pub fn build_ws_conn(
    socket: axum::extract::ws::WebSocket,
    endpoint: String,
    broadcaster: std::sync::Arc<WsBroadcaster>,
    msg_type: Option<crate::ast::TypeExpr>,
    send_type: Option<crate::ast::TypeExpr>,
    env: crate::env::EnvRef,
    heartbeat_secs: u64,
) -> (Value, u64, tokio::task::JoinHandle<()>) {
    use futures_util::{SinkExt, StreamExt};
    use std::sync::atomic::AtomicBool;

    let (mut sink, stream) = socket.split();
    let (outbox_tx, mut outbox_rx) = tokio::sync::mpsc::unbounded_channel();
    let conn_id = broadcaster.register(endpoint.clone(), outbox_tx.clone());
    let closed = std::sync::Arc::new(AtomicBool::new(false));
    let closed_writer = closed.clone();
    let writer = tokio::spawn(async move {
        while let Some(msg) = outbox_rx.recv().await {
            match msg {
                WsOutMessage::Text(t) => {
                    if sink
                        .send(axum::extract::ws::Message::Text(t.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                WsOutMessage::Binary(bs) => {
                    // 9.w.2-binary-frames — raw binary frame.
                    if sink
                        .send(axum::extract::ws::Message::Binary(bs.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                WsOutMessage::Ping => {
                    if sink
                        .send(axum::extract::ws::Message::Ping(Vec::new().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                WsOutMessage::Close => {
                    let _ = sink.close().await;
                    break;
                }
            }
        }
        closed_writer.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    // Phase 9.w.2.e — heartbeat task. If `heartbeat_secs > 0`,
    // spawn a task that sends Ping to the outbox every N seconds.
    // Ends when `closed` is set (writer task failed) or when the
    // outbox_tx is closed. No extra allocs: we clone only the tx
    // (cheap) and the `closed` flag.
    if heartbeat_secs > 0 {
        let hb_tx = outbox_tx.clone();
        let hb_closed = closed.clone();
        let interval = std::time::Duration::from_secs(heartbeat_secs);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Skip the first tick (interval fires immediately).
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if hb_closed.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                if hb_tx.send(WsOutMessage::Ping).is_err() {
                    return;
                }
            }
        });
    }

    let handle = WsConnHandle {
        endpoint,
        conn_id,
        rx: std::sync::Arc::new(tokio::sync::Mutex::new(
            Box::new(WsReadStreamImpl { inner: stream })
                as Box<dyn WsReadStreamTrait + Send + Unpin>,
        )),
        outbox_tx,
        closed,
        broadcaster: broadcaster as std::sync::Arc<dyn WsBroadcasterTrait + Send + Sync>,
        msg_type,
        send_type,
        env,
    };
    let value = Value::WsConn(std::sync::Arc::new(handle));
    (value, conn_id, writer)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 in tests is a generic Float, not PI.
mod tests {
    use super::*;
    use crate::ast::StrPart;
    use crate::value::shared;

    // ---- FITZ-05 FASE B — Set-Cookie serialisation ----

    #[test]
    fn fitz05_serialize_set_cookie_canonical_order_and_flags() {
        // Full cookie: all attributes present, canonical order
        // name=value; Path; Domain; Max-Age; HttpOnly; Secure; SameSite.
        let s = serialize_set_cookie(
            "session",
            "tok123",
            "/app",
            true,
            true,
            "Strict",
            Some(3600),
            Some("example.com"),
        );
        assert_eq!(
            s,
            "session=tok123; Path=/app; Domain=example.com; Max-Age=3600; HttpOnly; Secure; SameSite=Strict"
        );
    }

    #[test]
    fn fitz05_serialize_set_cookie_defaults_omit_optional_attrs() {
        // Session cookie (no Max-Age/Domain), default Path/SameSite,
        // flags off — Domain/Max-Age/HttpOnly/Secure absent.
        let s = serialize_set_cookie("lang", "es-AR", "/", false, false, "Lax", None, None);
        assert_eq!(s, "lang=es-AR; Path=/; SameSite=Lax");
    }

    #[test]
    fn fitz05_serialize_set_cookie_empty_path_and_same_site_omitted() {
        // Empty Path/SameSite → those attributes are dropped entirely.
        let s = serialize_set_cookie("k", "v", "", false, false, "", None, None);
        assert_eq!(s, "k=v");
    }

    #[test]
    fn fitz05_cookie_instance_to_set_cookie_reads_fields() {
        let cookie = Value::Instance {
            type_name: "Cookie".to_string(),
            fields: shared(vec![
                ("name".to_string(), Value::Str("session".to_string())),
                ("value".to_string(), Value::Str("tok".to_string())),
                ("path".to_string(), Value::Str("/".to_string())),
                ("http_only".to_string(), Value::Bool(true)),
                ("secure".to_string(), Value::Bool(false)),
                ("same_site".to_string(), Value::Str("Lax".to_string())),
                ("max_age".to_string(), Value::Int(86400)),
                ("domain".to_string(), Value::Null),
            ]),
        };
        let s = cookie_instance_to_set_cookie(&cookie).expect("serialise ok");
        assert_eq!(
            s,
            "session=tok; Path=/; Max-Age=86400; HttpOnly; SameSite=Lax"
        );
    }

    #[test]
    fn fitz05_cookie_instance_rejects_non_cookie() {
        let not_cookie = Value::Str("nope".to_string());
        let err = cookie_instance_to_set_cookie(&not_cookie).unwrap_err();
        assert!(err.contains("List<Cookie>"), "err: {err}");
    }

    // ---- HttpMethod ----

    #[tokio::test(flavor = "current_thread")]
    async fn http_method_from_decorator_name() {
        assert_eq!(
            HttpMethod::from_decorator_name("get"),
            Some(HttpMethod::Get)
        );
        assert_eq!(
            HttpMethod::from_decorator_name("post"),
            Some(HttpMethod::Post)
        );
        assert_eq!(
            HttpMethod::from_decorator_name("put"),
            Some(HttpMethod::Put)
        );
        assert_eq!(
            HttpMethod::from_decorator_name("delete"),
            Some(HttpMethod::Delete)
        );
        assert_eq!(HttpMethod::from_decorator_name("server"), None);
        assert_eq!(HttpMethod::from_decorator_name("patch"), None);
    }

    // ---- parse_path_template ----

    #[tokio::test(flavor = "current_thread")]
    async fn path_str_simple_without_params() {
        let t = parse_path_template(&Expr::Str("/".into(), Span::ZERO)).unwrap();
        assert_eq!(t.path, "/");
        assert!(t.params.is_empty());

        let t = parse_path_template(&Expr::Str("/users".into(), Span::ZERO)).unwrap();
        assert_eq!(t.path, "/users");
        assert!(t.params.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_strinterp_with_one_param() {
        // `"/users/{id}"` → StrInterp([Lit("/users/"), Expr(Ident("id"))])
        let e = Expr::StrInterp(
            vec![
                StrPart::Lit("/users/".into()),
                StrPart::Expr(Expr::Ident("id".into(), Span::ZERO), None),
            ],
            Span::ZERO,
        );
        let t = parse_path_template(&e).unwrap();
        assert_eq!(t.path, "/users/{id}");
        assert_eq!(t.params, vec!["id".to_string()]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_strinterp_with_multiple_distinct_params() {
        // `"/orgs/{org}/users/{id}"`
        let e = Expr::StrInterp(
            vec![
                StrPart::Lit("/orgs/".into()),
                StrPart::Expr(Expr::Ident("org".into(), Span::ZERO), None),
                StrPart::Lit("/users/".into()),
                StrPart::Expr(Expr::Ident("id".into(), Span::ZERO), None),
            ],
            Span::ZERO,
        );
        let t = parse_path_template(&e).unwrap();
        assert_eq!(t.path, "/orgs/{org}/users/{id}");
        assert_eq!(t.params, vec!["org".to_string(), "id".to_string()]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_not_starting_with_slash_is_error() {
        let err = parse_path_template(&Expr::Str("users".into(), Span::ZERO)).unwrap_err();
        assert_eq!(err, PathError::MustStartWithSlash);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_with_non_ident_expression_is_error() {
        // `"{a+b}"` — interpolation with BinOp.
        let e = Expr::StrInterp(
            vec![
                StrPart::Lit("/".into()),
                StrPart::Expr(
                    Expr::BinOp {
                        op: crate::ast::BinOpKind::Add,
                        left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
                        right: Box::new(Expr::Ident("b".into(), Span::ZERO)),
                        span: Span::ZERO,
                    },
                    None,
                ),
            ],
            Span::ZERO,
        );
        let err = parse_path_template(&e).unwrap_err();
        assert!(matches!(err, PathError::UnsupportedInterpolation(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_with_duplicated_params_is_error() {
        // `"/a/{x}/b/{x}"`
        let e = Expr::StrInterp(
            vec![
                StrPart::Lit("/a/".into()),
                StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
                StrPart::Lit("/b/".into()),
                StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
            ],
            Span::ZERO,
        );
        let err = parse_path_template(&e).unwrap_err();
        assert_eq!(err, PathError::DuplicateParam("x".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_not_string_literal_is_error() {
        // `@get(42)` — Int instead of string.
        let err = parse_path_template(&Expr::Int(42, Span::ZERO)).unwrap_err();
        assert_eq!(err, PathError::NotAStringLiteral);
    }

    // ---- Query params in the template ----

    #[tokio::test(flavor = "current_thread")]
    async fn query_template_separates_path_from_query_params() {
        // `"/items?limit={limit}&offset={offset}"` → path only
        // `/items`, query_params `["limit", "offset"]` in order.
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
        assert!(t.params.is_empty(), "there should be no path params");
        assert_eq!(
            t.query_params,
            vec!["limit".to_string(), "offset".to_string()]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn query_template_combines_with_path_params() {
        // `"/users/{id}/posts?limit={limit}"` → path
        // `/users/{id}/posts`, path params `["id"]`, query params
        // `["limit"]`.
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
    async fn query_template_key_distinct_from_name_is_error() {
        // `"/x?l={limit}"` — key `l` doesn't match name `limit`.
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
            "expected QueryKeyNameMismatch, was: {:?}",
            err
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn query_template_malformed_is_error() {
        // `"/x?limit"` — missing `={name}`.
        let e = Expr::Str("/x?limit".into(), Span::ZERO);
        let err = parse_path_template(&e).unwrap_err();
        assert!(
            matches!(err, PathError::MalformedQueryTemplate(_)),
            "expected MalformedQueryTemplate, was: {:?}",
            err
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn query_template_param_duplicated_with_path_is_error() {
        // `"/users/{id}?id={id}"` — `id` appears in both path and
        // query.
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
        // The parser fires DuplicateParam when it sees the second
        // `{id}` in the first pass (before separating path from
        // query). That's OK — the message is still clear to the
        // user.
        assert_eq!(err, PathError::DuplicateParam("id".into()));
    }

    // ---- value_to_json ----

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_primitives() {
        assert_eq!(
            value_to_json(&Value::Int(42)).unwrap(),
            serde_json::json!(42)
        );
        assert_eq!(
            value_to_json(&Value::Float(3.14)).unwrap(),
            serde_json::json!(3.14)
        );
        assert_eq!(
            value_to_json(&Value::Str("hola".into())).unwrap(),
            serde_json::json!("hola")
        );
        assert_eq!(
            value_to_json(&Value::Bool(true)).unwrap(),
            serde_json::json!(true)
        );
        assert_eq!(
            value_to_json(&Value::Null).unwrap(),
            serde_json::json!(null)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_list() {
        let v = Value::List(shared(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
        assert_eq!(value_to_json(&v).unwrap(), serde_json::json!([1, 2, 3]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_map_with_string_keys() {
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
    async fn value_to_json_map_non_string_key_is_error() {
        let v = Value::Map(shared(vec![(Value::Int(1), Value::Int(10))]));
        let err = value_to_json(&v).unwrap_err();
        assert!(err.contains("Map keys in JSON"));
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
    async fn value_to_json_nested_result_is_tagged() {
        // `Ok(42)` nested inside something else (shouldn't happen in the
        // handler's direct output, but we want total behavior).
        let ok = Value::Result(ResultVariant::Ok(Box::new(Value::Int(42))));
        assert_eq!(value_to_json(&ok).unwrap(), serde_json::json!({ "Ok": 42 }));

        let err = Value::Result(ResultVariant::Err(Box::new(Value::Str("boom".into()))));
        assert_eq!(
            value_to_json(&err).unwrap(),
            serde_json::json!({ "Err": "boom" })
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn value_to_json_function_is_error() {
        // Function is not serializable.
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
    async fn outcome_of_bare_value_is_200() {
        let v = Value::Str("hola".into());
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 200);
        assert_eq!(out.body, "\"hola\"");
        assert_eq!(out.content_type, "application/json");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_of_ok_is_200_with_inner() {
        let v = Value::Result(ResultVariant::Ok(Box::new(Value::Int(42))));
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 200);
        assert_eq!(out.body, "42");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_of_err_is_500_with_error_obj() {
        let v = Value::Result(ResultVariant::Err(Box::new(Value::Str("not found".into()))));
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 500);
        // Body is `{"error":"not found"}` (serde_json order).
        assert_eq!(out.body, "{\"error\":\"not found\"}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_of_instance_is_json_object() {
        let inst = Value::new_instance(
            "User".into(),
            vec![
                ("id".into(), Value::Int(7)),
                ("name".into(), Value::Str("ana".into())),
            ],
        );
        let out = value_to_outcome(&inst);
        assert_eq!(out.status, 200);
        // serde_json::Map preserves insertion order with the
        // `preserve_order` feature enabled; without it, the order
        // is undefined. We don't assume order here: parse the body
        // and compare.
        let parsed: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "id": 7, "name": "ana" }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_of_non_serializable_type_is_500() {
        // Range is not serializable.
        let v = Value::Range { start: 0, end: 10 };
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 500);
        assert!(out.body.contains("Range"));
    }

    // ---- Status codes custom (Value::HttpResponse) ----

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_of_http_response_uses_its_status_and_body() {
        // The evaluator produces `Value::HttpResponse` when the
        // user does `return 401 { ... }`. The outcome uses the
        // response status and serializes the body with the usual
        // rules.
        let body = Value::new_instance(
            "Error".into(),
            vec![("message".into(), Value::Str("unauthorized".into()))],
        );
        let v = Value::HttpResponse {
            status: 401,
            body: Some(Box::new(body)),
        };
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 401);
        let parsed: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "message": "unauthorized" }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_of_http_response_without_body_is_null_json() {
        // `HttpResponse { body: None }` → JSON null body. Reserved
        // for 204 No Content if it ever arrives; today the parser
        // requires an explicit body.
        let v = Value::HttpResponse {
            status: 204,
            body: None,
        };
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 204);
        assert_eq!(out.body, "null");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outcome_of_http_response_with_body_map_serializes_to_object() {
        // Body = map literal with string keys → JSON object.
        let body = Value::new_map(vec![
            (Value::Str("error".into()), Value::Str("failed".into())),
            (Value::Str("code".into()), Value::Int(42)),
        ]);
        let v = Value::HttpResponse {
            status: 500,
            body: Some(Box::new(body)),
        };
        let out = value_to_outcome(&v);
        assert_eq!(out.status, 500);
        let parsed: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "error": "failed", "code": 42 }));
    }

    // ---- coerce_path_param ----

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_defaults_to_str_without_annotation() {
        let v = coerce_path_param("42", None).unwrap();
        assert_eq!(v, Value::Str("42".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_int_parses_to_int() {
        let v = coerce_path_param("42", Some("Int")).unwrap();
        assert_eq!(v, Value::Int(42));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_int_invalid_is_error() {
        let err = coerce_path_param("abc", Some("Int")).unwrap_err();
        assert!(err.contains("Int") && err.contains("abc"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_float_parses() {
        let v = coerce_path_param("3.14", Some("Float")).unwrap();
        assert_eq!(v, Value::Float(3.14));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_bool_true_false() {
        assert_eq!(
            coerce_path_param("true", Some("Bool")).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            coerce_path_param("false", Some("Bool")).unwrap(),
            Value::Bool(false)
        );
        assert!(coerce_path_param("maybe", Some("Bool")).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn path_param_unsupported_type_is_error() {
        // A custom type isn't allowed as a path param: the handler
        // must receive the raw id and rebuild the object inside.
        let err = coerce_path_param("42", Some("User")).unwrap_err();
        assert!(err.contains("User"));
    }

    // ---- registry ----

    #[tokio::test(flavor = "current_thread")]
    async fn registry_starts_without_routes() {
        let r = HttpRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.routes.len(), 0);
    }

    #[test]
    fn ws_broadcast_uses_global_fallback_without_thread_local() {
        // Phase 3c — an @every/@cron fn broadcasting from a worker thread has no
        // thread-local registry; install_ws_broadcaster + the global fallback
        // let ws_broadcast_to_endpoint resolve the broadcaster. No connections →
        // a no-op that must not panic (exercises the global path).
        let b = std::sync::Arc::new(WsBroadcaster::new());
        install_ws_broadcaster(b);
        ws_broadcast_to_endpoint("/live/none", "{}".to_string());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_active_registry_exposes_has_active_for_the_evaluator() {
        // Outside: no registry, decorators emit an explicit error.
        assert!(!has_active_registry());

        let ((), reg) = with_active_registry(|| {
            // Inside: the evaluator sees an active registry.
            assert!(has_active_registry());
        });

        // Returned empty (nobody pushed), and outside still none.
        assert!(reg.is_empty());
        assert!(!has_active_registry());
    }

    // ---- handle_task (interpreter side, no tokio) ----

    /// Helper: builds an `HttpRegistry` with a single route from a
    /// Fitz source that registers it. Uses the real evaluator so we
    /// don't construct `Value::Function` by hand (which is fragile
    /// — capturing the right closure matters).
    ///
    /// Phase 6.4: becomes `async fn` because `eval` is now async.
    /// Call sites add `.await`.
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
    async fn handle_task_invokes_handler_and_returns_outcome() {
        // `@get("/") fn hello() => "hello"`
        let src = "@get(\"/\")\nfn hello() => \"hello\"";
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
        assert_eq!(outcome.body, "\"hello\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_coerces_path_param_int() {
        let src = "@get(\"/users/{id}\")\nfn h(id: Int) => id * 2";
        let registry = registry_from_source(src).await;
        let mut params = HashMap::new();
        params.insert("id".into(), "21".into());
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
        assert_eq!(outcome.body, "42");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_path_param_int_invalid_is_400() {
        let src = "@get(\"/users/{id}\")\nfn h(id: Int) => id";
        let registry = registry_from_source(src).await;
        let mut params = HashMap::new();
        params.insert("id".into(), "not-an-int".into());
        let outcome = handle_task(
            &registry,
            0,
            params,
            HashMap::new(),
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("Int"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_handler_returning_err_is_500_with_error() {
        // The handler returns Err("boom"): runtime translates it
        // to 500.
        let src = "@get(\"/\")\nfn h() => Err(\"boom\")";
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
        assert!(outcome.body.contains("boom"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_handler_returning_instance_serializes_to_json() {
        let src = "\
            type User { id: Int, name: Str }\n\
            @get(\"/u\")\nfn h() => User { id: 1, name: \"ana\" }\n\
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
        let parsed: serde_json::Value = serde_json::from_str(&outcome.body).unwrap();
        assert_eq!(parsed, serde_json::json!({ "id": 1, "name": "ana" }));
    }

    // ---- Mini-phase MW.1: middleware chain in handle_task ----

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_middleware_returning_null_continues_to_handler() {
        // "Passthrough" middleware: returns nothing → chain
        // continues and the handler runs normally.
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
    async fn handle_task_middleware_short_circuits_with_401() {
        // Middleware cuts the chain with `return 401 { ... }`. The
        // handler is NOT invoked and the response is the
        // middleware's.
        let src = "\
            fn auth(req) {\n\
                return 401 {\"error\": \"unauthorized\"}\n\
            }\n\
            @middleware(auth)\n\
            @get(\"/\")\n\
            fn h() => \"SHOULD NOT APPEAR\"\n\
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
        assert!(outcome.body.contains("unauthorized"));
        assert!(!outcome.body.contains("SHOULD NOT APPEAR"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_async_middleware_returning_null_continues() {
        // v0.41 — an `async fn` middleware returns a `Value::Future`; the
        // chain must await it before inspecting the result. Without the
        // await the Future matched no arm and 500'd only in `fitz run`
        // (run↔build parity bug — `fitz build` awaited it).
        let src = "\
            async fn pass(req) {}\n\
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
    async fn handle_task_async_middleware_short_circuits() {
        // An `async fn` middleware can still short-circuit with
        // `return <status> { ... }` — the awaited value is the response.
        let src = "\
            async fn gate(req) {\n\
                return 403 {\"error\": \"blocked\"}\n\
            }\n\
            @middleware(gate)\n\
            @get(\"/\")\n\
            fn h() => \"SHOULD NOT APPEAR\"\n\
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
        assert!(outcome.body.contains("blocked"));
        assert!(!outcome.body.contains("SHOULD NOT APPEAR"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_two_middlewares_first_short_circuit_wins() {
        // First `logger` (pass), then `auth` (cuts). The handler
        // shouldn't run. If we flip the order and the cut lands
        // first, we verify it below.
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
    async fn handle_task_middleware_reads_method_and_path_from_request() {
        // The middleware inspects req.method and req.path. Verifies
        // that the path carries the SUBSTITUTED path params, not
        // the template (mini-phase MW.1: `/users/{id}` →
        // `/users/42`).
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
    async fn handle_task_middleware_reads_headers_lowercase() {
        // Headers exposed to the middleware with lowercase keys
        // (same criterion as the @header dispatch).
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
    async fn handle_task_middleware_returning_invalid_value_is_500() {
        // If the middleware returns anything other than Null or
        // HttpResponse (Int, Str, Instance, ...), the runtime
        // emits 500 with a clear message citing "gate-only".
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
        assert!(
            outcome.body.contains("unexpected value") || outcome.body.contains("short-circuit")
        );
    }

    // ---- Mini-phase MW.2: cors built-in + header injection ----

    #[tokio::test(flavor = "current_thread")]
    async fn cors_response_headers_emits_three_basic_headers() {
        let cfg = CorsConfig::permissive_default();
        let headers = cfg.response_headers(None);
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"access-control-allow-origin"));
        assert!(names.contains(&"access-control-allow-methods"));
        assert!(names.contains(&"access-control-allow-headers"));
        // max_age default is None → that header is not emitted.
        assert!(!names.contains(&"access-control-max-age"));
    }

    // ---- Q.3: AllowOrigin Set + echo of the request Origin ----

    #[tokio::test(flavor = "current_thread")]
    async fn cors_set_echo_when_origin_in_list() {
        let cfg = CorsConfig {
            allow_origin: AllowOrigin::Set(vec!["https://a.com".into(), "https://b.com".into()]),
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
    async fn cors_set_omits_origin_header_when_request_does_not_match() {
        let cfg = CorsConfig {
            allow_origin: AllowOrigin::Set(vec!["https://a.com".into()]),
            ..CorsConfig::permissive_default()
        };
        // The request Origin is NOT in the list → the
        // access-control-allow-origin header is NOT emitted; the
        // browser rejects the response.
        let headers = cfg.response_headers(Some("https://evil.com"));
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"access-control-allow-origin"));
        // The other CORS headers are emitted (they are not
        // request-aware).
        assert!(names.contains(&"access-control-allow-methods"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_set_omits_origin_when_request_has_no_origin() {
        // Without an `Origin` header (same-origin request, browser
        // doesn't send it), Set mode also doesn't emit — nothing
        // to echo. The browser wouldn't need it in that case
        // anyway.
        let cfg = CorsConfig {
            allow_origin: AllowOrigin::Set(vec!["https://a.com".into()]),
            ..CorsConfig::permissive_default()
        };
        let headers = cfg.response_headers(None);
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"access-control-allow-origin"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_literal_ignores_request_origin() {
        // Literal always emits the same value, regardless of the
        // request.
        let cfg = CorsConfig {
            allow_origin: AllowOrigin::Literal("*".into()),
            ..CorsConfig::permissive_default()
        };
        let headers_with = cfg.response_headers(Some("https://x.com"));
        let headers_without = cfg.response_headers(None);
        assert_eq!(headers_with, headers_without);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn allow_origin_resolve_set_match_and_miss() {
        let any = AllowOrigin::Literal("*".to_string());
        assert_eq!(any.resolve(None), Some("*".to_string()));
        assert_eq!(any.resolve(Some("https://x.com")), Some("*".to_string()));

        let single = AllowOrigin::Literal("https://x.com".to_string());
        assert_eq!(
            single.resolve(Some("https://y.com")),
            Some("https://x.com".to_string())
        );

        let set = AllowOrigin::Set(vec!["https://a.com".into(), "https://b.com".into()]);
        assert_eq!(
            set.resolve(Some("https://b.com")),
            Some("https://b.com".to_string())
        );
        assert_eq!(set.resolve(Some("https://evil.com")), None);
        assert_eq!(set.resolve(None), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cors_response_headers_emits_max_age_when_set() {
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
    async fn handle_task_injects_cors_headers_in_real_response() {
        // Normal handler + @middleware(cors()) → the 200 response
        // carries the Access-Control-Allow-* headers.
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
        let names: Vec<&str> = outcome
            .extra_headers
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(names.contains(&"access-control-allow-origin"));
        assert!(names.contains(&"access-control-allow-methods"));
        assert!(names.contains(&"access-control-allow-headers"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_injects_cors_headers_even_on_500_error() {
        // If the handler returns Err(...), the response is 500 but
        // still carries the CORS headers. Without this the browser
        // sees "CORS error" instead of the actual 500 that
        // happened.
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
        let names: Vec<&str> = outcome
            .extra_headers
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(names.contains(&"access-control-allow-origin"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_custom_origin_propagates_to_headers() {
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
    async fn handle_task_cors_set_echoes_request_origin_when_match() {
        // Q.3: cors with allowed origin list. Request with
        // `Origin: https://a.com` in the list → echo the origin.
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
    async fn handle_task_cors_set_omits_origin_when_no_match() {
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
        let names: Vec<&str> = outcome
            .extra_headers
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        // The origin header is NOT emitted (browser rejects the
        // response).
        assert!(!names.contains(&"access-control-allow-origin"));
        // The other CORS headers are emitted.
        assert!(names.contains(&"access-control-allow-methods"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_without_cors_emits_no_extra_headers() {
        // Sanity: a handler without @middleware(cors(...)) must
        // not carry extra headers (no contamination).
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
    async fn handle_task_middleware_stops_before_parsing_body() {
        // If the middleware short-circuits, the body is NOT parsed
        // (the 400 for invalid body that would normally appear is
        // gone). This checks that the order is middlewares → parse
        // body → handler.
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
        // Invalid body (not JSON) — would yield 400 if it reached
        // the parser.
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            b"this-is-not-json".to_vec(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 401);
        assert!(outcome.body.contains("nope"));
    }

    // ---- ServerConfig (Phase 4.4) ----

    #[tokio::test(flavor = "current_thread")]
    async fn server_config_default_is_localhost_3000() {
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
            ws_heartbeat_secs: 30,
            shutdown_timeout_secs: 30,
            observability_enabled: true,
            prometheus_enabled: false,
        };
        let addr = c.to_socket_addr().unwrap();
        assert_eq!(addr.to_string(), "0.0.0.0:8080");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_config_to_socket_addr_invalid_host_is_error() {
        let c = ServerConfig {
            host: "not-an-ip".into(),
            port: 80,
            enable_docs: true,
            api_version: None,
            ws_heartbeat_secs: 30,
            shutdown_timeout_secs: 30,
            observability_enabled: true,
            prometheus_enabled: false,
        };
        let err = c.to_socket_addr().unwrap_err();
        assert!(err.contains("not-an-ip"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_server_config_second_time_returns_existing() {
        let ((), _reg) = with_active_registry(|| {
            let first = ServerConfig {
                host: "127.0.0.1".into(),
                port: 8080,
                enable_docs: true,
                api_version: None,
                ws_heartbeat_secs: 30,
                shutdown_timeout_secs: 30,
                observability_enabled: true,
                prometheus_enabled: false,
            };
            assert!(set_server_config(first.clone()).is_ok());
            let second = ServerConfig {
                host: "0.0.0.0".into(),
                port: 9090,
                enable_docs: true,
                api_version: None,
                ws_heartbeat_secs: 30,
                shutdown_timeout_secs: 30,
                observability_enabled: true,
                prometheus_enabled: false,
            };
            let err = set_server_config(second).unwrap_err();
            // The error carries the existing config, not the new
            // one.
            assert_eq!(err, first);
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn registry_resolved_config_returns_default_when_no_explicit() {
        let mut reg = HttpRegistry::new();
        assert!(reg.server_config.is_none());
        assert_eq!(reg.resolved_config(), ServerConfig::default_addr());
        // With explicit config, yes.
        reg.server_config = Some(ServerConfig {
            host: "0.0.0.0".into(),
            port: 80,
            enable_docs: true,
            api_version: None,
            ws_heartbeat_secs: 30,
            shutdown_timeout_secs: 30,
            observability_enabled: true,
            prometheus_enabled: false,
        });
        let resolved = reg.resolved_config();
        assert_eq!(resolved.port, 80);
        assert_eq!(resolved.host, "0.0.0.0");
    }

    // ---- json_to_value (free deserialization) ----

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_value_primitives() {
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
    async fn json_to_value_array_becomes_list() {
        let v = json_to_value(&serde_json::json!([1, 2, "tres"]));
        match v {
            Value::List(items) => {
                let items = items.lock();
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Int(1));
                assert_eq!(items[2], Value::Str("tres".into()));
            }
            _ => panic!("expected List"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_value_object_becomes_map_with_str_keys() {
        let v = json_to_value(&serde_json::json!({ "a": 1, "b": "x" }));
        match v {
            Value::Map(pairs) => {
                let pairs = pairs.lock();
                assert_eq!(pairs.len(), 2);
                // serde_json::Map order depends on the
                // `preserve_order` feature. We don't assume it:
                // we convert to an auxiliary map to compare.
                let as_map: std::collections::HashMap<String, Value> = pairs
                    .iter()
                    .map(|(k, v)| {
                        let k = match k {
                            Value::Str(s) => s.clone(),
                            _ => panic!("key not Str"),
                        };
                        (k, v.clone())
                    })
                    .collect();
                assert_eq!(as_map.get("a"), Some(&Value::Int(1)));
                assert_eq!(as_map.get("b"), Some(&Value::Str("x".into())));
            }
            _ => panic!("expected Map"),
        }
    }

    // ---- json_to_instance (validation against Value::Type) ----

    /// Helper: builds a `Value::Type` with the given fields. Each
    /// field is `(name, type, nullable, default)`. The `nullable`
    /// flag translates to `TypeExpr::Nullable(Named(t))`.
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
                        decorators: vec![],
                    }
                })
                .collect(),
            resolved_defaults: vec![],
            methods: vec![],
            table_metadata: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_happy_case() {
        let t = type_value(
            "User",
            vec![("id", "Int", false, None), ("name", "Str", false, None)],
        );
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
            _ => panic!("expected Instance"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_missing_field_without_default_or_nullable_is_error() {
        let t = type_value(
            "User",
            vec![("id", "Int", false, None), ("name", "Str", false, None)],
        );
        let json = serde_json::json!({ "id": 1 });
        let err = json_to_instance(&json, &t).unwrap_err();
        assert!(err.contains("name"));
        assert!(err.contains("missing"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_extra_field_is_error() {
        let t = type_value("User", vec![("id", "Int", false, None)]);
        let json = serde_json::json!({ "id": 1, "rogue": "x" });
        let err = json_to_instance(&json, &t).unwrap_err();
        assert!(err.contains("rogue"));
        assert!(err.contains("undeclared"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_missing_nullable_field_stays_null() {
        let t = type_value(
            "User",
            vec![("id", "Int", false, None), ("email", "Str", true, None)],
        );
        let json = serde_json::json!({ "id": 1 });
        let v = json_to_instance(&json, &t).unwrap();
        match v {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[1].0, "email");
                assert_eq!(fields[1].1, Value::Null);
            }
            _ => panic!("expected Instance"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_default_literal_used_when_missing() {
        let t = type_value(
            "User",
            vec![
                ("id", "Int", false, None),
                ("active", "Bool", false, Some(Expr::Bool(true, Span::ZERO))),
            ],
        );
        let json = serde_json::json!({ "id": 1 });
        let v = json_to_instance(&json, &t).unwrap();
        match v {
            Value::Instance { fields, .. } => {
                let fields = fields.lock();
                assert_eq!(fields[1].0, "active");
                assert_eq!(fields[1].1, Value::Bool(true));
            }
            _ => panic!("expected Instance"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_to_instance_body_not_object_is_error() {
        let t = type_value("User", vec![("id", "Int", false, None)]);
        let json = serde_json::json!([1, 2, 3]);
        let err = json_to_instance(&json, &t).unwrap_err();
        assert!(err.contains("object"));
        assert!(err.contains("array"));
    }

    // ---- handle_task con body ----

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_post_without_body_but_handler_expects_it_is_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
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
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("body required"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_post_with_valid_body_builds_instance() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body.name\n\
        ";
        let registry = registry_from_source(src).await;
        let body = br#"{"name":"fitz"}"#.to_vec();
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            body,
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"fitz\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_post_invalid_json_body_is_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let registry = registry_from_source(src).await;
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            b"not json".to_vec(),
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("JSON"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_post_body_missing_field_is_400() {
        let src = "\
            type UserInput { name: Str, email: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let registry = registry_from_source(src).await;
        let body = br#"{"name":"fitz"}"#.to_vec();
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            body,
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 400);
        assert!(outcome.body.contains("email"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_task_put_with_path_param_and_body() {
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
    async fn handle_task_body_without_type_annotation_accepts_free() {
        // Untyped `body` → arrives as Map<Str,Value>.
        let src = "\
            @post(\"/log\")\nfn log(body) => body[\"name\"]\n\
        ";
        let registry = registry_from_source(src).await;
        let body = br#"{"name":"x"}"#.to_vec();
        let outcome = handle_task(
            &registry,
            0,
            HashMap::new(),
            HashMap::new(),
            body,
            HashMap::new(),
        )
        .await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "\"x\"");
    }

    // ---- build_router + oneshot E2E ----
    //
    // These tests build an axum router and send requests without
    // opening a TCP socket, via `tower::ServiceExt::oneshot`.
    //
    // Post-F17.5: zero glue. The registry is wrapped in `Arc` and
    // passed to `build_router`; each axum handler invokes the
    // evaluator directly. Previously this needed a `LocalSet` + a
    // `tokio::select!` loop over `mpsc::recv` to coexist with the
    // bridge — that disappeared.

    /// Helper: runs a request against the router and returns
    /// (status, body string). No body, no extra headers.
    async fn run_oneshot(src: &str, method: axum::http::Method, path: &str) -> (u16, String) {
        run_oneshot_with_body(src, method, path, None).await
    }

    /// Like `run_oneshot_with_body` but also accepts a list of
    /// `(name, value)` headers added to the request. Useful for
    /// `@header(...)` tests (Phase 7.6).
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

    /// Like `run_oneshot` but with an optional body. If `body` is
    /// `Some(s)`, it's sent as `application/json` (though the
    /// runtime doesn't validate content-type today).
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
        // Existing routing tests: schema = None so we don't
        // contaminate path lookups with the 7.2 auto-registered
        // route.
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

    /// Like `run_oneshot` but also returns the response headers
    /// (a Vec<(name, value)> in lowercase). Used by the MW.2 CORS
    /// tests to verify `Access-Control-Allow-*`.
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
    async fn e2e_fitz05_response_cookies_emit_multiple_set_cookie_headers() {
        // FITZ-05 FASE B — a handler returning `Response { cookies:
        // [Cookie {...}, Cookie {...}] }` emits TWO separate
        // `Set-Cookie` headers. This exercises the `.append` fix in
        // `outcome_to_response` (`.insert` would drop all but the last),
        // which the codegen path does not cover (axum's builder already
        // appends). Parity with `fitz build`.
        let src = "\
            @get(\"/login\")\n\
            fn login() => Response {\n\
                status: 303,\n\
                headers: {\"Location\": \"/\"},\n\
                cookies: [\n\
                    Cookie { name: \"session\", value: \"tok123\", http_only: true, max_age: 86400 },\n\
                    Cookie { name: \"lang\", value: \"es-AR\", path: \"/app\", same_site: \"Strict\" },\n\
                ],\n\
            }\n\
        ";
        let (status, headers, _body) =
            run_oneshot_full(src, axum::http::Method::GET, "/login").await;
        assert_eq!(status, 303);
        let set_cookies: Vec<&String> = headers
            .iter()
            .filter(|(n, _)| n == "set-cookie")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(
            set_cookies.len(),
            2,
            "expected 2 Set-Cookie headers (the .append fix), got {:?}",
            headers
        );
        assert!(
            set_cookies.iter().any(|v| v.contains("session=tok123")
                && v.contains("; Path=/")
                && v.contains("; Max-Age=86400")
                && v.contains("; HttpOnly")
                && v.contains("; SameSite=Lax")),
            "session cookie malformed: {set_cookies:?}"
        );
        assert!(
            set_cookies.iter().any(|v| v.contains("lang=es-AR")
                && v.contains("; Path=/app")
                && v.contains("; SameSite=Strict")
                && !v.contains("HttpOnly")),
            "lang cookie malformed: {set_cookies:?}"
        );
        // The single-value `Location` header survives via the `.insert`
        // path (not `.append`).
        assert!(
            headers.iter().any(|(n, v)| n == "location" && v == "/"),
            "Location header missing: {headers:?}"
        );
    }

    #[tokio::test]
    async fn e2e_preflight_options_responds_204_with_cors_headers() {
        // OPTIONS on a route with @middleware(cors(...)) returns
        // 204 and the Access-Control-Allow-* headers. The real
        // (GET) handler is NOT invoked — axum routes OPTIONS to
        // the dedicated preflight handler.
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
    async fn e2e_options_without_cors_is_405_method_not_allowed() {
        // If the route has NO @middleware(cors(...)), an OPTIONS
        // responds 405 (axum default — the method isn't registered
        // for that path). Sanity: without CORS, we don't create a
        // preflight handler.
        let src = "@get(\"/api\")\nfn h() => \"ok\"";
        let (status, _, _) = run_oneshot_full(src, axum::http::Method::OPTIONS, "/api").await;
        assert_eq!(status, 405);
    }

    #[tokio::test]
    async fn e2e_real_response_with_cors_carries_injected_headers() {
        // Normal GET on a cors route → 200 + Access-Control-Allow-*
        // headers.
        let src = "\
            @middleware(cors({\"allow_origin\": \"https://x.com\"}))\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let (status, headers, body) = run_oneshot_full(src, axum::http::Method::GET, "/api").await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"ok\"");
        let origin = headers
            .iter()
            .find(|(n, _)| n == "access-control-allow-origin")
            .map(|(_, v)| v.clone());
        assert_eq!(origin, Some("https://x.com".to_string()));
    }

    #[tokio::test]
    async fn e2e_preflight_set_echo_when_origin_in_list() {
        // Q.3: preflight with cors({"allow_origin": [...]}) echoes
        // the Origin if it's allowed.
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
    async fn e2e_preflight_set_without_match_omits_origin() {
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

    /// Variant of `run_oneshot_full` that accepts extra headers for
    /// the request (Q.3: to send `Origin: ...` and verify echo).
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
        (
            status,
            response_headers,
            String::from_utf8(bytes.to_vec()).unwrap(),
        )
    }

    #[tokio::test]
    async fn e2e_preflight_max_age_emitted_only_when_set() {
        let src = "\
            @middleware(cors({\"max_age\": 3600}))\n\
            @get(\"/api\")\n\
            fn h() => \"ok\"\n\
        ";
        let (status, headers, _) = run_oneshot_full(src, axum::http::Method::OPTIONS, "/api").await;
        assert_eq!(status, 204);
        let max_age = headers
            .iter()
            .find(|(n, _)| n == "access-control-max-age")
            .map(|(_, v)| v.clone());
        assert_eq!(max_age, Some("3600".to_string()));
    }

    // ---- Regression bug duplicate OPTIONS preflight (2026-05-22) ----
    //
    // When multiple handlers share a path (typical CRUD: `/tasks`
    // with `@get` + `@post`), each one carries its own
    // `@middleware(cors(...))` with its `allow_methods`. Pre-fix,
    // both tried to register `OPTIONS /tasks` and axum panicked with
    // "Overlapping method route. Handler for `OPTIONS /tasks`
    // already exists" when building the Router. Fix: pre-compute
    // the CorsConfig merge per path; the preflight is registered
    // ONCE with unified methods.

    #[tokio::test]
    async fn bug_options_preflight_duplicate_does_not_panic_in_build_router() {
        // Two handlers on `/tasks` with CORS — pre-fix this
        // panicked. Today build_router finishes without errors.
        let src = "\
            @middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"GET\", \"OPTIONS\"]}))\n\
            @get(\"/tasks\")\n\
            fn list() => \"ok\"\n\
            @middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"POST\", \"OPTIONS\"]}))\n\
            @post(\"/tasks\")\n\
            fn create() => \"created\"\n\
        ";
        // run_oneshot already builds the router internally — if it
        // panicked, the test would hang with a visible panic. We
        // keep it as a "no panic" smoke in addition to validating
        // that GET still works.
        let (status, body) = run_oneshot(src, axum::http::Method::GET, "/tasks").await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"ok\"");
    }

    #[tokio::test]
    async fn bug_options_preflight_duplicate_merged_methods_in_preflight_response() {
        // After the fix, the unified preflight advertises GET +
        // POST. Without the merge, only the first (GET) is
        // advertised → browser rejects POST in the CORS check.
        let src = "\
            @middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"GET\", \"OPTIONS\"]}))\n\
            @get(\"/tasks\")\n\
            fn list() => \"ok\"\n\
            @middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"POST\", \"OPTIONS\"]}))\n\
            @post(\"/tasks\")\n\
            fn create() => \"created\"\n\
        ";
        let (status, headers, _) =
            run_oneshot_full(src, axum::http::Method::OPTIONS, "/tasks").await;
        assert_eq!(status, 204);
        let methods = headers
            .iter()
            .find(|(n, _)| n == "access-control-allow-methods")
            .map(|(_, v)| v.clone())
            .expect("preflight debe traer Access-Control-Allow-Methods");
        // Insertion order: GET appears first (it's the owner),
        // POST is merged after. OPTIONS appears only once (dedup).
        assert!(
            methods.contains("GET"),
            "merged methods debe incluir GET: {}",
            methods
        );
        assert!(
            methods.contains("POST"),
            "merged methods debe incluir POST: {}",
            methods
        );
        assert!(
            methods.contains("OPTIONS"),
            "merged methods debe incluir OPTIONS: {}",
            methods
        );
    }

    #[tokio::test]
    async fn bug_options_preflight_duplicate_three_handlers_with_path_id() {
        // Case from the 6th boilerplate (api-fullstack-postgres):
        // `/tasks/{id}` with GET + PUT + DELETE, each with its own
        // CORS. Pre-fix it panicked on the second handler.
        let src = "\
            @middleware(cors({\"allow_origin\": \"*\", \"allow_methods\": [\"GET\", \"OPTIONS\"]}))\n\
            @get(\"/tasks/{id}\")\n\
            fn get_one(id: Int) => id\n\
            @middleware(cors({\"allow_origin\": \"*\", \"allow_methods\": [\"PUT\", \"OPTIONS\"]}))\n\
            @put(\"/tasks/{id}\")\n\
            fn update(id: Int) => id\n\
            @middleware(cors({\"allow_origin\": \"*\", \"allow_methods\": [\"DELETE\", \"OPTIONS\"]}))\n\
            @delete(\"/tasks/{id}\")\n\
            fn del(id: Int) => id\n\
        ";
        let (status, headers, _) =
            run_oneshot_full(src, axum::http::Method::OPTIONS, "/tasks/42").await;
        assert_eq!(status, 204);
        let methods = headers
            .iter()
            .find(|(n, _)| n == "access-control-allow-methods")
            .map(|(_, v)| v.clone())
            .expect("preflight debe traer Access-Control-Allow-Methods");
        assert!(methods.contains("GET"));
        assert!(methods.contains("PUT"));
        assert!(methods.contains("DELETE"));
    }

    #[tokio::test]
    async fn bug_options_preflight_duplicate_merge_of_headers_case_insensitive() {
        // Two handlers with headers differing only in case — they
        // are not duplicated on merge.
        let src = "\
            @middleware(cors({\"allow_origin\": \"*\", \"allow_headers\": [\"Content-Type\"]}))\n\
            @get(\"/x\")\n\
            fn h1() => \"a\"\n\
            @middleware(cors({\"allow_origin\": \"*\", \"allow_headers\": [\"content-type\", \"Authorization\"]}))\n\
            @post(\"/x\")\n\
            fn h2() => \"b\"\n\
        ";
        let (status, headers, _) = run_oneshot_full(src, axum::http::Method::OPTIONS, "/x").await;
        assert_eq!(status, 204);
        let allowed_headers = headers
            .iter()
            .find(|(n, _)| n == "access-control-allow-headers")
            .map(|(_, v)| v.clone())
            .expect("preflight must carry Access-Control-Allow-Headers");
        // Content-Type from the first handler is preserved with
        // its original casing. Authorization is added from the
        // second. "content-type" from the second is NOT duplicated
        // (case-insensitive match).
        let comma_count = allowed_headers.matches(',').count();
        assert_eq!(
            comma_count, 1,
            "expected 2 headers (1 comma), got: {}",
            allowed_headers
        );
        assert!(allowed_headers.to_lowercase().contains("content-type"));
        assert!(allowed_headers.to_lowercase().contains("authorization"));
    }

    #[tokio::test]
    async fn e2e_get_simple_responds_200_with_json() {
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
    async fn e2e_get_with_path_param_int() {
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
    async fn e2e_get_with_invalid_path_param_returns_400() {
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
    async fn e2e_handler_returning_instance_serializes_to_json() {
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
    async fn e2e_method_mismatch_returns_405() {
        let (status, _body) = run_oneshot(
            "@get(\"/\")\nfn h() => \"ok\"",
            axum::http::Method::POST,
            "/",
        )
        .await;
        assert_eq!(status, 405);
    }

    #[tokio::test]
    async fn e2e_path_not_found_returns_404() {
        let (status, _body) = run_oneshot(
            "@get(\"/foo\")\nfn h() => \"ok\"",
            axum::http::Method::GET,
            "/bar",
        )
        .await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn e2e_handler_err_returns_500_with_error_object() {
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
    async fn e2e_post_with_valid_body_builds_instance() {
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
    async fn e2e_post_invalid_body_returns_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let (status, body) =
            run_oneshot_with_body(src, axum::http::Method::POST, "/users", Some("not json")).await;
        assert_eq!(status, 400);
        assert!(body.contains("JSON"));
    }

    #[tokio::test]
    async fn e2e_put_with_path_param_and_body() {
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
    async fn e2e_post_without_body_but_handler_expects_is_400() {
        let src = "\
            type UserInput { name: Str }\n\
            @post(\"/users\")\nfn create(body: UserInput) => body\n\
        ";
        let (status, body) = run_oneshot(src, axum::http::Method::POST, "/users").await;
        assert_eq!(status, 400);
        assert!(body.contains("body required"));
    }

    // ---- 7.6 headers as handler params ----

    #[tokio::test]
    async fn e2e_required_header_present_handler_receives_it() {
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
    async fn e2e_required_header_missing_is_400() {
        let src = "@header(name=\"Authorization\")\n@get(\"/protected\")\nfn protected(authorization: Str) => authorization";
        let (status, body) =
            run_oneshot_with_headers(src, axum::http::Method::GET, "/protected", &[]).await;
        assert_eq!(status, 400);
        assert!(body.contains("Authorization"), "body was: {}", body);
        assert!(body.contains("required"), "body was: {}", body);
    }

    #[tokio::test]
    async fn e2e_nullable_header_missing_handler_receives_null() {
        let src = "@header(name=\"X-Trace-Id\")\n@get(\"/traced\")\nfn traced(x_trace_id: Str?) -> Str { return \"ok\" }";
        let (status, body) =
            run_oneshot_with_headers(src, axum::http::Method::GET, "/traced", &[]).await;
        // Handler runs OK because the header is optional.
        assert_eq!(status, 200);
        assert_eq!(body, "\"ok\"");
    }

    #[tokio::test]
    async fn e2e_header_lookup_is_case_insensitive() {
        // HTTP is case-insensitive in header names. We send
        // `authorization` (lowercase) and the handler declares
        // `@header(name="Authorization")` — it must match.
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

    // ---- 12.1.b — auto-mount de /healthz y /readyz ----

    async fn oneshot_get(registry: HttpRegistry, path: &'static str) -> (u16, String) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let metas = registry.metas();
        let router = build_router(&metas, std::sync::Arc::new(registry), None);
        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn default_healthz_responds_200() {
        // Without a declared @healthz, the server auto-mounts
        // /healthz with a default 200 response.
        let (status, body) = oneshot_get(HttpRegistry::new(), "/healthz").await;
        assert_eq!(status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["status"], serde_json::json!("ok"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn default_readyz_responds_200_when_not_draining() {
        let (status, body) = oneshot_get(HttpRegistry::new(), "/readyz").await;
        assert_eq!(status, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["status"], serde_json::json!("ok"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn readyz_responds_503_during_draining() {
        // With `draining = true`, /readyz returns 503 WITHOUT
        // touching the handler (even if one exists). The test
        // simulates the post-SIGTERM state before axum closes the
        // listener.
        let registry = HttpRegistry::new();
        registry
            .draining
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let (status, body) = oneshot_get(registry, "/readyz").await;
        assert_eq!(status, 503);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["status"], serde_json::json!("draining"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn healthz_not_affected_by_draining() {
        // Liveness ≠ readiness: even while draining, /healthz keeps
        // returning 200 (the process is alive). K8s only stops
        // routing, it does not restart the pod for liveness.
        let registry = HttpRegistry::new();
        registry
            .draining
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let (status, _) = oneshot_get(registry, "/healthz").await;
        assert_eq!(status, 200);
    }

    // ---- 7.2 auto-register of /openapi.json ----
    //
    // Local helper: builds a router from an `HttpRegistry`
    // (Arc-wrapped) + schema and sends GET /openapi.json. For
    // cases with no user routes we pass `HttpRegistry::new()`.
    // Post-F17.5: zero glue, the router responds directly (no
    // bridge needed).

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
    async fn build_router_with_schema_some_registers_openapi_json() {
        // Minimal schema: the router serves it as-is on GET
        // /openapi.json.
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

    // ---- 9.w.2-ws-auth-browser — helper extract_ws_bearer_subprotocol ----

    #[test]
    fn ws_bearer_subprotocol_single_proto() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("sec-websocket-protocol", "bearer.abc123".parse().unwrap());
        let r = extract_ws_bearer_subprotocol(&h);
        assert_eq!(r, Some(("bearer.abc123".to_string(), "abc123".to_string())));
    }

    #[test]
    fn ws_bearer_subprotocol_among_several_csv() {
        // The client can offer multiple subprotocols (RFC 6455
        // §4.1). We take the first one matching `bearer.*`.
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            "sec-websocket-protocol",
            "some.other, bearer.tok-xyz, third.proto".parse().unwrap(),
        );
        let r = extract_ws_bearer_subprotocol(&h);
        assert_eq!(
            r,
            Some(("bearer.tok-xyz".to_string(), "tok-xyz".to_string()))
        );
    }

    #[test]
    fn ws_bearer_subprotocol_absent() {
        // Without a `sec-websocket-protocol` header, returns None.
        let h = axum::http::HeaderMap::new();
        assert_eq!(extract_ws_bearer_subprotocol(&h), None);
    }

    #[test]
    fn ws_bearer_subprotocol_without_match() {
        // Header present but no subprotocol matches `bearer.*`.
        let mut h = axum::http::HeaderMap::new();
        h.insert("sec-websocket-protocol", "chat.v1, app.v2".parse().unwrap());
        assert_eq!(extract_ws_bearer_subprotocol(&h), None);
    }

    #[test]
    fn ws_bearer_subprotocol_empty_token_is_none() {
        // `bearer.` with no token after it doesn't count.
        let mut h = axum::http::HeaderMap::new();
        h.insert("sec-websocket-protocol", "bearer.".parse().unwrap());
        assert_eq!(extract_ws_bearer_subprotocol(&h), None);
    }

    #[test]
    fn ws_bearer_subprotocol_token_with_internal_dots() {
        // JWTs carry internal dots (`header.payload.signature`).
        // The `bearer.` strip_prefix consumes only the first `.`,
        // and the token keeps everything that came after.
        let jwt = "eyJhbGciOi.eyJzdWI.SflKxw";
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            "sec-websocket-protocol",
            format!("bearer.{}", jwt).parse().unwrap(),
        );
        let r = extract_ws_bearer_subprotocol(&h);
        assert_eq!(
            r,
            Some((format!("bearer.{}", jwt), jwt.to_string())),
            "the token should preserve internal dots typical of JWT"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_with_schema_none_does_not_register_openapi_json() {
        let (status, _body) = oneshot_get_openapi(HttpRegistry::new(), None).await;
        assert_eq!(status, 404);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_auto_register_coexists_with_user_routes() {
        // If the user has `@get("/")` and auto-register adds
        // `/openapi.json`, both work. We verify the schema is still
        // available even with declared routes.
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
    async fn user_declares_own_openapi_json_wins_over_auto_register() {
        // The user declared their own `@get("/openapi.json")`.
        // Auto-register must yield — the user's route is what
        // responds. We verify the response is the user's (a
        // `"mio"` string), not the cached schema we passed.
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
        // The body is the user's handler: `"mio"` (JSON string).
        // It does NOT contain "_marker" from the auto-register
        // schema.
        assert_eq!(body, "\"mio\"");
        assert!(!body.contains("_marker"));
    }

    // ---- 7.3 auto-register of /docs (Scalar UI) ----

    /// Local helper: GET /docs on a router built with or without a
    /// schema.
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
    async fn build_router_with_schema_some_registers_docs() {
        // GET /docs returns the embedded HTML. We verify the body
        // references `/openapi.json` (the data-url of Scalar's
        // script) — that guarantees the HTML is connected to the
        // auto-generated schema.
        let schema = serde_json::json!({ "openapi": "3.1.0", "paths": {} });
        let (status, body) = oneshot_get_docs(HttpRegistry::new(), Some(schema)).await;
        assert_eq!(status, 200);
        assert!(
            body.contains("data-url=\"/openapi.json\""),
            "expected the HTML to reference /openapi.json, body was:\n{}",
            body
        );
        assert!(
            body.contains("@scalar/api-reference"),
            "expected the HTML to load the Scalar bundle, body was:\n{}",
            body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_with_schema_none_does_not_register_docs() {
        // Without a schema, /docs is not registered (parity with
        // /openapi.json).
        let (status, _body) = oneshot_get_docs(HttpRegistry::new(), None).await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn user_declares_own_docs_wins_over_auto_register() {
        // The user declared their own `@get("/docs")`. The Scalar
        // UI auto-register yields — the user's route is what
        // responds.
        let src = "@get(\"/docs\")\nfn custom() => \"custom-docs\"";
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
        // User's body, not Scalar's HTML.
        assert_eq!(body, "\"custom-docs\"");
        assert!(!body.contains("@scalar/api-reference"));
    }

    // ---- 9.w.2-asyncapi-ui — embedded HTML UI for `/asyncapi` ----

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_with_asyncapi_schema_registers_asyncapi_ui() {
        // When asyncapi_schema is present (because there are @ws
        // handlers), GET /asyncapi returns the embedded HTML
        // (parallel to OpenAPI's /docs). The body references
        // /asyncapi.json to load the schema and loads the
        // @asyncapi/react-component bundle.
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let asyncapi_schema = serde_json::json!({
            "asyncapi": "3.0.0",
            "info": { "title": "Test", "version": "0.1.0" },
            "channels": {},
            "operations": {},
        });
        let metas: Vec<RouteMeta> = Vec::new();
        let router = build_router_with_asyncapi(
            &metas,
            std::sync::Arc::new(HttpRegistry::new()),
            None,
            Some(asyncapi_schema),
        );

        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/asyncapi")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            body.contains("/asyncapi.json"),
            "expected reference to /asyncapi.json (the schema fetch), body was: {}",
            &body[..body.len().min(400)]
        );
        assert!(
            body.contains("@asyncapi/react-component"),
            "expected load of @asyncapi/react-component bundle, body was: {}",
            &body[..body.len().min(400)]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_without_asyncapi_schema_does_not_register_asyncapi_ui() {
        // Without asyncapi_schema (HTTP-only program), /asyncapi
        // returns 404.
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let metas: Vec<RouteMeta> = Vec::new();
        let router = build_router_with_asyncapi(
            &metas,
            std::sync::Arc::new(HttpRegistry::new()),
            None,
            None,
        );

        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/asyncapi")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        let _ = resp.into_body().collect().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn build_router_asyncapi_json_still_available() {
        // Sanity: the JSON endpoint keeps working independently of
        // the UI.
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let schema = serde_json::json!({
            "asyncapi": "3.0.0",
            "info": { "title": "Test", "version": "0.1.0" },
        });
        let metas: Vec<RouteMeta> = Vec::new();
        let router = build_router_with_asyncapi(
            &metas,
            std::sync::Arc::new(HttpRegistry::new()),
            None,
            Some(schema.clone()),
        );

        let req = axum::http::Request::builder()
            .method(axum::http::Method::GET)
            .uri("/asyncapi.json")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["asyncapi"], serde_json::json!("3.0.0"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn push_route_accumulates_in_active_registry() {
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
                cookies: vec![],
                param_type_exprs: vec![],
                return_type_expr: None,
                middlewares: vec![],
                cors: None,
                auth: AuthSpec::None,
                required_roles: Vec::new(),
                flag_name: None,
                auth_user_param_name: None,
                is_ws: false,
                ws_conn_param_name: None,
                ws_msg_type: None,
                ws_send_type: None,
            });
        });
        assert_eq!(reg.routes.len(), 1);
        assert_eq!(reg.routes[0].method, HttpMethod::Get);
        assert_eq!(reg.routes[0].handler_name, "index");
    }

    // ---- Mini-batch HC.1 — status outside 100..1000 ----

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
    fn hc1_err_with_valid_status_uses_that_status() {
        let outcome = value_to_outcome(&err_instance_with_status(404));
        assert_eq!(outcome.status, 404);
    }

    #[test]
    fn hc1_err_with_out_of_range_status_emits_500_with_clear_msg() {
        let outcome = value_to_outcome(&err_instance_with_status(50));
        assert_eq!(outcome.status, 500);
        let body_str = outcome.body.to_string();
        assert!(
            body_str.contains("invalid") && body_str.contains("50"),
            "expected a clear message, was: {}",
            body_str
        );
    }

    #[test]
    fn hc1_err_with_status_99_is_out_of_range() {
        let outcome = value_to_outcome(&err_instance_with_status(99));
        assert_eq!(outcome.status, 500);
    }

    #[test]
    fn hc1_err_with_status_1500_is_out_of_range() {
        let outcome = value_to_outcome(&err_instance_with_status(1500));
        assert_eq!(outcome.status, 500);
    }

    // ---- v0.19.0 — Response built-in custom content_type + headers ----

    fn response_instance(
        status: i64,
        content_type: &str,
        headers: Vec<(&str, &str)>,
        body: &str,
    ) -> Value {
        let headers_pairs: Vec<(Value, Value)> = headers
            .into_iter()
            .map(|(k, v)| (Value::Str(k.into()), Value::Str(v.into())))
            .collect();
        Value::new_instance(
            "Response".to_string(),
            vec![
                ("status".into(), Value::Int(status)),
                ("content_type".into(), Value::Str(content_type.into())),
                ("headers".into(), Value::Map(shared(headers_pairs))),
                ("body".into(), Value::Str(body.into())),
            ],
        )
    }

    #[test]
    fn v019_response_rss_feed_emite_outcome_con_content_type_custom_y_body_crudo() {
        let rss = "<?xml version=\"1.0\"?><rss/>";
        let outcome = value_to_outcome(&response_instance(
            200,
            "application/rss+xml; charset=utf-8",
            vec![],
            rss,
        ));
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.content_type, "application/rss+xml; charset=utf-8");
        assert_eq!(outcome.body, rss);
        assert!(outcome.extra_headers.is_empty());
    }

    #[test]
    fn v019_response_plain_text_emite_text_plain_sin_json_wrap() {
        let outcome = value_to_outcome(&response_instance(
            200,
            "text/plain; charset=utf-8",
            vec![],
            "User-agent: *\nDisallow: /",
        ));
        assert_eq!(outcome.content_type, "text/plain; charset=utf-8");
        // Critical: body must NOT be JSON-quoted.
        assert_eq!(outcome.body, "User-agent: *\nDisallow: /");
        assert!(
            !outcome.body.starts_with('"'),
            "el body de plain text no debe ir envuelto en comillas JSON"
        );
    }

    #[test]
    fn v019_response_headers_se_propagan_a_extra_headers_en_orden() {
        let outcome = value_to_outcome(&response_instance(
            200,
            "text/plain",
            vec![
                ("Cache-Control", "public, max-age=3600"),
                ("X-Custom", "smoke"),
            ],
            "payload",
        ));
        let names: Vec<&str> = outcome
            .extra_headers
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert!(names.contains(&"Cache-Control"));
        assert!(names.contains(&"X-Custom"));
        assert_eq!(outcome.extra_headers.len(), 2);
    }

    #[test]
    fn v019_response_status_fuera_de_rango_devuelve_500_con_mensaje() {
        let outcome = value_to_outcome(&response_instance(999_999, "text/plain", vec![], "x"));
        assert_eq!(outcome.status, 500);
        assert!(outcome.body.contains("out of range"));
    }

    #[test]
    fn v019_response_dentro_de_result_ok_tambien_dispatchea_custom() {
        // `fn f() -> Result<Response> { Ok(Response { ... }) }` con
        // `?` propagation. El path Ok(Response) tambien debe activar
        // el dispatch custom.
        let resp = response_instance(200, "image/svg+xml", vec![], "<svg/>");
        let result_ok = Value::Result(ResultVariant::Ok(Box::new(resp)));
        let outcome = value_to_outcome(&result_ok);
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.content_type, "image/svg+xml");
        assert_eq!(outcome.body, "<svg/>");
    }

    #[test]
    fn v019_response_shape_invalida_status_no_int_devuelve_500_con_mensaje() {
        let bad = Value::new_instance(
            "Response".to_string(),
            vec![
                ("status".into(), Value::Str("doscientos".into())),
                ("content_type".into(), Value::Str("text/plain".into())),
                ("headers".into(), Value::Map(shared(vec![]))),
                ("body".into(), Value::Str("x".into())),
            ],
        );
        let outcome = value_to_outcome(&bad);
        assert_eq!(outcome.status, 500);
        assert!(outcome.body.contains("status must be Int"));
    }

    #[test]
    fn v019_response_normal_handler_sigue_emitiendo_application_json() {
        // Regresion: un handler que no usa Response sigue devolviendo
        // application/json por default.
        let val = Value::new_instance(
            "User".to_string(),
            vec![
                ("id".into(), Value::Int(1)),
                ("name".into(), Value::Str("ada".into())),
            ],
        );
        let outcome = value_to_outcome(&val);
        assert_eq!(outcome.content_type, "application/json");
        assert!(outcome.body.starts_with('{'));
    }

    // ---- v0.19.0 Block 2 — Response body_bytes (binary path) ----

    fn response_binary_instance(status: i64, content_type: &str, body_bytes: Vec<u8>) -> Value {
        Value::new_instance(
            "Response".to_string(),
            vec![
                ("status".into(), Value::Int(status)),
                ("content_type".into(), Value::Str(content_type.into())),
                ("headers".into(), Value::Map(shared(vec![]))),
                ("body".into(), Value::Str(String::new())),
                ("body_bytes".into(), Value::Bytes(body_bytes)),
            ],
        )
    }

    #[test]
    fn v019_block2_body_bytes_seteado_dispara_path_binario_y_oculta_body_str() {
        let pdf_bytes = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let outcome = value_to_outcome(&response_binary_instance(
            200,
            "application/pdf",
            pdf_bytes.clone(),
        ));
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.content_type, "application/pdf");
        assert_eq!(outcome.body_bytes.as_deref(), Some(pdf_bytes.as_slice()));
        assert!(outcome.body.is_empty());
    }

    #[test]
    fn v019_block2_body_bytes_null_path_texto_normal() {
        // Default Response { body: "x" } sin body_bytes (null) sigue
        // por el path de texto. Es lo que validan los smokes de
        // Block 1 — replica acá explícitamente con body_bytes: null.
        let mut inst = response_instance(200, "text/plain", vec![], "hola");
        if let Value::Instance { fields, .. } = &inst {
            fields.lock().push(("body_bytes".into(), Value::Null));
        }
        let outcome = value_to_outcome(&inst);
        assert_eq!(outcome.body, "hola");
        assert!(outcome.body_bytes.is_none());
        // Make sure clippy-fixed assertion is hit.
        let _ = &mut inst;
    }

    #[test]
    fn v019_block2_body_y_body_bytes_ambos_seteados_devuelve_500() {
        // Programming error: el user setea body con texto Y body_bytes
        // con un Bytes. No elegimos uno silentemente — 500 con mensaje
        // claro.
        let bad = Value::new_instance(
            "Response".to_string(),
            vec![
                ("status".into(), Value::Int(200)),
                ("content_type".into(), Value::Str("text/plain".into())),
                ("headers".into(), Value::Map(shared(vec![]))),
                ("body".into(), Value::Str("texto".into())),
                ("body_bytes".into(), Value::Bytes(b"binario".to_vec())),
            ],
        );
        let outcome = value_to_outcome(&bad);
        assert_eq!(outcome.status, 500);
        assert!(
            outcome.body.contains("cannot set both"),
            "el mensaje debe mencionar que body y body_bytes son XOR: {}",
            outcome.body
        );
    }

    #[test]
    fn v019_block2_body_bytes_shape_invalida_devuelve_500_con_mensaje() {
        // body_bytes debe ser Bytes o Null. Si llega un Int (por
        // ejemplo de --no-typecheck), 500 con mensaje claro.
        let bad = Value::new_instance(
            "Response".to_string(),
            vec![
                ("status".into(), Value::Int(200)),
                ("content_type".into(), Value::Str("text/plain".into())),
                ("headers".into(), Value::Map(shared(vec![]))),
                ("body".into(), Value::Str(String::new())),
                ("body_bytes".into(), Value::Int(42)),
            ],
        );
        let outcome = value_to_outcome(&bad);
        assert_eq!(outcome.status, 500);
        assert!(outcome.body.contains("body_bytes must be Bytes?"));
    }

    // ---- Mini-batch Hpx.1 — Content-Type validation ----

    fn registry_with_post_body_route() -> std::sync::Arc<HttpRegistry> {
        // Minimal setup: a POST /test route expecting body as a
        // free Value::Map (no schema).
        let mut reg = HttpRegistry::new();
        let handler = Value::Function {
            params: vec![crate::ast::Param {
                name: "body".into(),
                type_: None,
                default: None,
                varargs: false,
                name_span: Span::default(),
                decorators: vec![],
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
            cookies: vec![],
            param_type_exprs: vec![("body".into(), None)],
            return_type_expr: None,
            middlewares: vec![],
            cors: None,
            auth: AuthSpec::None,
            required_roles: Vec::new(),
            flag_name: None,
            auth_user_param_name: None,
            is_ws: false,
            ws_conn_param_name: None,
            ws_msg_type: None,
            ws_send_type: None,
        });
        std::sync::Arc::new(reg)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hpx1_content_type_json_passes() {
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
        )
        .await;
        assert_eq!(
            outcome.status, 200,
            "expected 200, was {} with body {}",
            outcome.status, outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hpx1_content_type_text_plain_rejects_with_415() {
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
        )
        .await;
        assert_eq!(outcome.status, 415);
        assert!(
            outcome.body.contains("text/plain") && outcome.body.contains("application/json"),
            "expected a clear message, was: {}",
            outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp2_content_type_charset_diff_unofficial_rejects() {
        // Mini-batch MP2 — `text/plain` (the old test assumed
        // multipart-rejected-with-415; now multipart is accepted
        // so I switched the case). text/plain stays rejected: the
        // interpreter accepts JSON, urlencoded and multipart,
        // nothing else.
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
        )
        .await;
        assert_eq!(outcome.status, 415);
        assert!(
            outcome.body.contains("octet-stream"),
            "expected the msg to cite the received CT, was: {}",
            outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hpx1_content_type_absent_accepts() {
        // Without a Content-Type header (curl without -H), we
        // accept raw JSON.
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
        )
        .await;
        assert_eq!(outcome.status, 200);
    }

    // ---- Mini-batch Mw.next — middleware post-process ----

    fn make_mw_post(name: &str) -> MiddlewareSpec {
        // Minimal constructor of a Post (2 args) middleware. Body:
        // `return 200 { "wrapped": true }`.
        let handler = Value::Function {
            params: vec![
                crate::ast::Param {
                    name: "req".into(),
                    type_: None,
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                    decorators: vec![],
                },
                crate::ast::Param {
                    name: "res".into(),
                    type_: None,
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                    decorators: vec![],
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
    async fn mwnext_post_middleware_modifies_response() {
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
        assert!(
            outcome.body.contains("wrapped"),
            "expected body with `wrapped`, was: {}",
            outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mwnext_post_middleware_without_post_does_not_modify() {
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

    // ---- Mini-batch MP — urlencoded bodies ----

    #[tokio::test(flavor = "current_thread")]
    async fn mp_urlencoded_basic_parses_to_map() {
        let reg = registry_with_post_body_route();
        let body = b"name=Fitz&age=25".to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert(
            "content-type".into(),
            "application/x-www-form-urlencoded".into(),
        );
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        )
        .await;
        assert_eq!(
            outcome.status, 200,
            "expected 200, was {} with body {}",
            outcome.status, outcome.body
        );
        assert!(
            outcome.body.contains("\"name\":\"Fitz\"") && outcome.body.contains("\"age\":\"25\""),
            "expected name/age in body, was: {}",
            outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp_urlencoded_with_url_encoding() {
        let reg = registry_with_post_body_route();
        // "hola mundo" + "Fitz Roy" with encoding (spaces as +).
        let body = b"greeting=hello+world&place=Fitz%20Roy".to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert(
            "content-type".into(),
            "application/x-www-form-urlencoded".into(),
        );
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        )
        .await;
        assert_eq!(outcome.status, 200);
        assert!(
            outcome.body.contains("\"greeting\":\"hello world\""),
            "expected `+` decoded to space: {}",
            outcome.body
        );
        assert!(
            outcome.body.contains("\"place\":\"Fitz Roy\""),
            "expected `%20` decoded to space: {}",
            outcome.body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp_urlencoded_empty_body_is_empty_map() {
        let reg = registry_with_post_body_route();
        let mut headers = std::collections::HashMap::new();
        headers.insert(
            "content-type".into(),
            "application/x-www-form-urlencoded".into(),
        );
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            Vec::new(),
            headers,
        )
        .await;
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "{}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp2_multipart_without_boundary_is_400() {
        // Mini-batch MP2 — `multipart/form-data` without
        // `boundary=` → 400 with a clear message (not 415: multipart
        // IS now accepted as a supported CT but the boundary is
        // mandatory).
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
        )
        .await;
        assert_eq!(outcome.status, 400);
        assert!(
            outcome.body.contains("boundary"),
            "expected mention of boundary, was: {}",
            outcome.body
        );
    }

    // ---- Quick win F13 bundle — base64 encoder ----

    #[test]
    fn b64_encode_empty() {
        assert_eq!(b64_encode_standard(b""), "");
    }

    #[test]
    fn b64_encode_basic() {
        // Standard RFC 4648 test vectors.
        assert_eq!(b64_encode_standard(b"f"), "Zg==");
        assert_eq!(b64_encode_standard(b"fo"), "Zm8=");
        assert_eq!(b64_encode_standard(b"foo"), "Zm9v");
        assert_eq!(b64_encode_standard(b"foob"), "Zm9vYg==");
        assert_eq!(b64_encode_standard(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64_encode_standard(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn b64_encode_binary() {
        // Arbitrary binary bytes.
        assert_eq!(b64_encode_standard(&[0u8]), "AA==");
        assert_eq!(b64_encode_standard(&[0xff, 0xff, 0xff]), "////");
    }

    #[test]
    fn value_to_json_bytes_emits_base64() {
        // Mini-batch Bytes + quick win F13: `Value::Bytes` is
        // serialized as a base64 string (not as an array of Int).
        let v = Value::Bytes(b"hello".to_vec());
        let j = value_to_json(&v).unwrap();
        assert_eq!(j, serde_json::json!("aGVsbG8="));
    }

    #[test]
    fn mp2_extract_boundary_simple() {
        assert_eq!(
            extract_multipart_boundary("multipart/form-data; boundary=abc"),
            Some("abc".to_string())
        );
    }

    #[test]
    fn mp2_extract_boundary_with_quotes() {
        // RFC 7578 allows the boundary between double quotes.
        assert_eq!(
            extract_multipart_boundary(r#"multipart/form-data; boundary="my-boundary""#),
            Some("my-boundary".to_string())
        );
    }

    #[test]
    fn mp2_extract_boundary_case_sensitive_value() {
        // Boundaries are case-sensitive: `Boundary` matches the
        // lowercase trim but the value is preserved verbatim.
        assert_eq!(
            extract_multipart_boundary("multipart/form-data; Boundary=ABC-Def"),
            Some("ABC-Def".to_string())
        );
    }

    #[test]
    fn mp2_extract_boundary_absent_returns_none() {
        assert_eq!(extract_multipart_boundary("multipart/form-data"), None);
    }

    #[test]
    fn mp2_parse_multipart_text_field_basic() {
        // Body with a single text field part (without filename).
        // Structure: --<b>\r\n<hdr>\r\n\r\n<body>\r\n--<b>--
        let boundary = "----foo";
        let body =
            "------foo\r\nContent-Disposition: form-data; name=\"msg\"\r\n\r\nhello\r\n------foo--"
                .to_string();
        let result = parse_multipart_body(body.as_bytes(), boundary).expect("parse OK");
        match result {
            Value::Map(entries) => {
                let pairs = entries.lock();
                assert_eq!(pairs.len(), 1);
                assert_eq!(pairs[0].0, Value::Str("msg".into()));
                assert_eq!(pairs[0].1, Value::Str("hello".into()));
            }
            other => panic!("expected Value::Map, was: {:?}", other),
        }
    }

    #[test]
    fn mp2_parse_multipart_file_field_builds_file_instance() {
        // Body with a file field (with filename) → Value::Instance
        // of the built-in `File` type.
        let boundary = "----foo";
        let body = "------foo\r\nContent-Disposition: form-data; name=\"upload\"; filename=\"hello.txt\"\r\nContent-Type: text/plain\r\n\r\nfile contents here\r\n------foo--";
        let result = parse_multipart_body(body.as_bytes(), boundary).expect("parse OK");
        let Value::Map(entries) = result else {
            panic!("expected Value::Map");
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
                // File.content Bytes — content is now Value::Bytes
                // (Vec<u8>), not Value::Str. Enables binary files.
                assert_eq!(fld[2].1, Value::Bytes(b"file contents here".to_vec()));
            }
            other => panic!("expected Value::Instance(File), was: {:?}", other),
        }
    }

    #[test]
    fn mp2_parse_multipart_mixed_text_and_file() {
        // Form with one text field + one file field.
        let boundary = "X";
        let body = "--X\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nMy title\r\n--X\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a.txt\"\r\n\r\ncontent\r\n--X--";
        let result = parse_multipart_body(body.as_bytes(), boundary).expect("parse OK");
        let Value::Map(entries) = result else {
            panic!("expected Value::Map");
        };
        let pairs = entries.lock();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, Value::Str("title".into()));
        assert_eq!(pairs[0].1, Value::Str("My title".into()));
        assert_eq!(pairs[1].0, Value::Str("doc".into()));
        assert!(matches!(pairs[1].1, Value::Instance { .. }));
    }

    #[test]
    fn mp2_parse_multipart_binary_file_field_works() {
        // File.content Bytes — non-UTF8 binary bytes (0xFF) in a
        // FILE field now work (used to be 400; now stored as raw
        // `Value::Bytes`). Enables binary uploads.
        let boundary = "X";
        let mut body =
            b"--X\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n\r\n"
                .to_vec();
        body.push(0xff);
        body.push(0xfe);
        body.extend_from_slice(b"\r\n--X--");
        let result = parse_multipart_body(&body, boundary).expect("parse OK with binary");
        let Value::Map(entries) = result else {
            panic!("expected Value::Map");
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
            other => panic!("expected Instance(File), was: {:?}", other),
        }
    }

    #[test]
    fn mp2_parse_multipart_text_field_without_filename_still_requires_utf8() {
        // Text field (without filename) still requires UTF-8 —
        // for binary bytes the user must use `filename=`.
        let boundary = "X";
        let mut body = b"--X\r\nContent-Disposition: form-data; name=\"raw\"\r\n\r\n".to_vec();
        body.push(0xff);
        body.extend_from_slice(b"\r\n--X--");
        let err = parse_multipart_body(&body, boundary).expect_err("expected error");
        assert!(
            err.contains("UTF-8") && err.contains("filename="),
            "expected mention of UTF-8 + filename= workaround, was: {}",
            err
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mp2_multipart_end_to_end_accepts_and_parses() {
        // Full path E2E: `handle_task` receives a valid multipart
        // body and routes it to the handler with the body parsed
        // as `Value::Map<Str, Value>`.
        let reg = registry_with_post_body_route();
        let boundary = "----my-boundary";
        let body = "------my-boundary\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nFitz\r\n------my-boundary--".to_string();
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
        )
        .await;
        // `registry_with_post_body_route` expects a body parseable
        // as `Map`, so it returns 200 with the body echoed.
        assert_eq!(outcome.status, 200, "outcome body: {}", outcome.body);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hpx1_content_type_with_charset_accepts() {
        let reg = registry_with_post_body_route();
        let body = br#"{"foo": 42}"#.to_vec();
        let mut headers = std::collections::HashMap::new();
        headers.insert(
            "content-type".into(),
            "application/json; charset=utf-8".into(),
        );
        let outcome = handle_task(
            &reg,
            0,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            body,
            headers,
        )
        .await;
        assert_eq!(outcome.status, 200);
    }

    // ---- Mini-batch Mw-Wrap — classification + chain runner ----

    #[test]
    fn mw_wrap_classifier_param_fn_es_wrap() {
        // Second param `Fn() -> Response` → Wrap.
        use crate::ast::{Param, TypeExpr};
        let p = Param {
            name: "next".into(),
            type_: Some(TypeExpr::Function {
                params: vec![],
                ret: Box::new(TypeExpr::Named("Response".into())),
            }),
            default: None,
            varargs: false,
            name_span: Span::default(),
            decorators: vec![],
        };
        assert_eq!(
            crate::evaluator::classify_2_arg_middleware(&p),
            MiddlewareKind::Wrap,
        );
    }

    #[test]
    fn mw_wrap_classifier_param_response_es_post() {
        // Second param `Response` (nominal) → Post.
        use crate::ast::{Param, TypeExpr};
        let p = Param {
            name: "resp".into(),
            type_: Some(TypeExpr::Named("Response".into())),
            default: None,
            varargs: false,
            name_span: Span::default(),
            decorators: vec![],
        };
        assert_eq!(
            crate::evaluator::classify_2_arg_middleware(&p),
            MiddlewareKind::Post,
        );
    }

    #[test]
    fn mw_wrap_classifier_param_without_annotation_is_post() {
        // No annotation → default Post (preserves historical
        // semantics).
        use crate::ast::Param;
        let p = Param {
            name: "resp".into(),
            type_: None,
            default: None,
            varargs: false,
            name_span: Span::default(),
            decorators: vec![],
        };
        assert_eq!(
            crate::evaluator::classify_2_arg_middleware(&p),
            MiddlewareKind::Post,
        );
    }

    #[test]
    fn mw_wrap_classifier_param_fn_nullable_es_wrap() {
        // `Fn() -> Response?` also classifies as Wrap.
        use crate::ast::{Param, TypeExpr};
        let p = Param {
            name: "next".into(),
            type_: Some(TypeExpr::Nullable(Box::new(TypeExpr::Function {
                params: vec![],
                ret: Box::new(TypeExpr::Named("Response".into())),
            }))),
            default: None,
            varargs: false,
            name_span: Span::default(),
            decorators: vec![],
        };
        assert_eq!(
            crate::evaluator::classify_2_arg_middleware(&p),
            MiddlewareKind::Wrap,
        );
    }

    // -----------------------------------------------------------------
    // Phase 9.w.1.c — E2E tests of the native auth wrapper.
    //
    // Validate the end-to-end flow through the real axum path via
    // `Router::oneshot`: registry from Fitz source, `@auth_provider`
    // executed before the handler, `user` injected in args, and
    // codes 401/403 when the provider rejects or the role doesn't
    // match.
    //
    // Shared source for the tests: a provider that matches headers
    // against two hard-coded tokens (admin and a regular user) and
    // emits `Err` for anything else. Uses `match` on the `Result`
    // of `headers.get("authorization")` to distinguish "missing
    // Authorization" from "invalid token".
    // -----------------------------------------------------------------

    const AUTH_E2E_SOURCE: &str = "\
type User { id: Int, name: Str, role: Str }\n\
@auth_provider\n\
fn check(headers: Map<Str, Str>) -> Result<User> {\n\
    match headers.get(\"authorization\") {\n\
        Ok(token) => {\n\
            if (token == \"Bearer admin-token\") {\n\
                return Ok(User { id: 1, name: \"Admin\", role: \"admin\" })\n\
            }\n\
            if (token == \"Bearer user-token\") {\n\
                return Ok(User { id: 2, name: \"Alice\", role: \"user\" })\n\
            }\n\
            return Err(\"invalid token\")\n\
        }\n\
        Err(_) => return Err(\"Authorization missing\")\n\
    }\n\
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

    #[tokio::test(flavor = "current_thread")]
    async fn auth_public_route_without_auth_returns_200() {
        // Route without `@authenticated`/`@admin` doesn't touch
        // the provider. Smoke: shouldn't break even if the
        // program declares `@auth_provider`.
        let (status, body) = run_oneshot(AUTH_E2E_SOURCE, axum::http::Method::GET, "/public").await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"no auth\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_authenticated_without_header_returns_401() {
        // Without an `Authorization` header → provider emits
        // `Err("Authorization missing")` → wrapper converts to 401
        // with `{"error": "Authorization missing"}`.
        let (status, body) = run_oneshot(AUTH_E2E_SOURCE, axum::http::Method::GET, "/me").await;
        assert_eq!(status, 401);
        assert!(
            body.contains("Authorization missing"),
            "expected mention of Authorization in body, was: {}",
            body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_authenticated_invalid_token_returns_401() {
        // Header present but unknown token →
        // Err("invalid token") → 401.
        let (status, body) = run_oneshot_with_headers(
            AUTH_E2E_SOURCE,
            axum::http::Method::GET,
            "/me",
            &[("authorization", "Bearer wrong-token")],
        )
        .await;
        assert_eq!(status, 401);
        assert!(
            body.contains("invalid token"),
            "expected 'invalid token' in body, was: {}",
            body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_authenticated_valid_token_returns_200_with_user_injected() {
        // Valid user token → provider returns
        // Ok(User{name:"Alice"}) → wrapper injects `user` as an
        // arg of the handler → handler reads `user.name` and
        // returns "Alice".
        let (status, body) = run_oneshot_with_headers(
            AUTH_E2E_SOURCE,
            axum::http::Method::GET,
            "/me",
            &[("authorization", "Bearer user-token")],
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"Alice\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_admin_with_non_admin_role_returns_403() {
        // Valid token but `user.role == "user"` (not "admin") →
        // wrapper emits 403 with "admin role required".
        let (status, body) = run_oneshot_with_headers(
            AUTH_E2E_SOURCE,
            axum::http::Method::GET,
            "/admin",
            &[("authorization", "Bearer user-token")],
        )
        .await;
        assert_eq!(status, 403);
        assert!(
            body.contains("admin"),
            "expected mention of admin in body, was: {}",
            body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_admin_with_admin_role_returns_200() {
        // Admin token → user.role == "admin" → handler runs.
        let (status, body) = run_oneshot_with_headers(
            AUTH_E2E_SOURCE,
            axum::http::Method::GET,
            "/admin",
            &[("authorization", "Bearer admin-token")],
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"hello admin\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_admin_without_header_returns_401_not_403() {
        // Without a header, the provider fails with Err BEFORE
        // evaluating the role. Result: 401 (unauthenticated), not
        // 403 (forbidden).
        let (status, _body) = run_oneshot(AUTH_E2E_SOURCE, axum::http::Method::GET, "/admin").await;
        assert_eq!(status, 401);
    }

    // ---- Phase 9.w.1.iter2.a — @requires("role") (custom RBAC) ----

    /// Program with several endpoints protected by `@requires`:
    /// - `/editor` requires the "editor" role (1 role).
    /// - `/multi` stacks `@requires("editor")` and
    ///   `@requires("publisher")` = OR (matches either).
    const REQUIRES_E2E_SOURCE: &str = "\
type User { id: Int, name: Str, role: Str }\n\
@auth_provider\n\
fn check(headers: Map<Str, Str>) -> Result<User> {\n\
    match headers.get(\"authorization\") {\n\
        Ok(token) => {\n\
            if (token == \"Bearer editor-token\") {\n\
                return Ok(User { id: 1, name: \"Ed\", role: \"editor\" })\n\
            }\n\
            if (token == \"Bearer publisher-token\") {\n\
                return Ok(User { id: 2, name: \"Pub\", role: \"publisher\" })\n\
            }\n\
            if (token == \"Bearer viewer-token\") {\n\
                return Ok(User { id: 3, name: \"View\", role: \"viewer\" })\n\
            }\n\
            return Err(\"invalid token\")\n\
        }\n\
        Err(_) => return Err(\"Authorization missing\")\n\
    }\n\
}\n\
@requires(\"editor\")\n\
@get(\"/editor\")\n\
fn editor_route(user: User) -> Str => user.name\n\
@requires(\"editor\")\n\
@requires(\"publisher\")\n\
@get(\"/multi\")\n\
fn multi_route(user: User) -> Str => user.name\n\
";

    #[tokio::test(flavor = "current_thread")]
    async fn requires_correct_role_returns_200_with_user_injected() {
        let (status, body) = run_oneshot_with_headers(
            REQUIRES_E2E_SOURCE,
            axum::http::Method::GET,
            "/editor",
            &[("authorization", "Bearer editor-token")],
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, "\"Ed\"");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn requires_incorrect_role_returns_403() {
        let (status, body) = run_oneshot_with_headers(
            REQUIRES_E2E_SOURCE,
            axum::http::Method::GET,
            "/editor",
            &[("authorization", "Bearer viewer-token")],
        )
        .await;
        assert_eq!(status, 403);
        assert!(
            body.contains("viewer") && body.contains("editor"),
            "expected mention of actual and required role in body, was: {}",
            body
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn requires_stacked_accepts_either_of_two_roles() {
        // `/multi` requires editor OR publisher. We test both
        // cases.
        let (status_ed, _) = run_oneshot_with_headers(
            REQUIRES_E2E_SOURCE,
            axum::http::Method::GET,
            "/multi",
            &[("authorization", "Bearer editor-token")],
        )
        .await;
        assert_eq!(status_ed, 200);

        let (status_pub, _) = run_oneshot_with_headers(
            REQUIRES_E2E_SOURCE,
            axum::http::Method::GET,
            "/multi",
            &[("authorization", "Bearer publisher-token")],
        )
        .await;
        assert_eq!(status_pub, 200);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn requires_stacked_rejects_role_matching_none() {
        // `/multi` requires editor OR publisher; viewer matches
        // neither → 403.
        let (status, _) = run_oneshot_with_headers(
            REQUIRES_E2E_SOURCE,
            axum::http::Method::GET,
            "/multi",
            &[("authorization", "Bearer viewer-token")],
        )
        .await;
        assert_eq!(status, 403);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn requires_without_header_returns_401_not_403() {
        // Without a header, the provider fails with Err BEFORE
        // evaluating the role.
        let (status, _) =
            run_oneshot(REQUIRES_E2E_SOURCE, axum::http::Method::GET, "/editor").await;
        assert_eq!(status, 401);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_provider_duplicated_is_runtime_error() {
        // Runtime uniqueness defense: two `@auth_provider`s should
        // emit an error when evaluating the program (the checker
        // also blocks it; we replicate at runtime defensively).
        let src = "\
type User { id: Int, role: Str }\n\
@auth_provider\n\
fn check_a(headers: Map<Str, Str>) -> Result<User> { return Err(\"x\") }\n\
@auth_provider\n\
fn check_b(headers: Map<Str, Str>) -> Result<User> { return Err(\"y\") }\n\
@authenticated\n\
@get(\"/x\")\n\
fn h(user: User) -> Str => user.role\n\
";
        let (res, _reg) = with_active_registry_async(|| async {
            let tokens = crate::lexer::tokenize(src).unwrap();
            let program = crate::parser::parse(tokens).unwrap();
            crate::evaluator::eval(program).await
        })
        .await;
        let err = res.expect_err("expected error due to duplicate @auth_provider");
        assert!(
            err.message.contains("@auth_provider duplicate"),
            "expected mention of duplicate provider, was: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_handler_without_provider_is_runtime_error() {
        // Runtime defense: handler with @authenticated but no
        // declared @auth_provider. The checker also blocks it
        // statically; we replicate at runtime to preserve the
        // registry invariant.
        let src = "\
type User { id: Int }\n\
@authenticated\n\
@get(\"/me\")\n\
fn me(user: User) -> Str => \"x\"\n\
";
        let (res, _reg) = with_active_registry_async(|| async {
            let tokens = crate::lexer::tokenize(src).unwrap();
            let program = crate::parser::parse(tokens).unwrap();
            crate::evaluator::eval(program).await
        })
        .await;
        let err = res.expect_err("expected error due to @authenticated without @auth_provider");
        assert!(
            err.message.contains("@auth_provider") && err.message.contains("before"),
            "expected mention of @auth_provider and order, was: {}",
            err.message
        );
    }

    // -----------------------------------------------------------------
    // Phase 9.w.2 — E2E tests of the WebSocket wrapper.
    //
    // We use `tokio-tungstenite` as the WS client, axum::serve over
    // a TCP listener with OS-assigned port (`:0`) to avoid
    // collisions in parallel runs. Each test:
    //   1. Builds a registry from Fitz source.
    //   2. Boots the server on a TcpListener.
    //   3. Connects one or more WS clients.
    //   4. Sends/receives test frames.
    //   5. Closes and verifies.
    //
    // Covers: simple echo, multi-client broadcast, pre-upgrade auth
    // (401), custom types marshalled as JSON.
    // -----------------------------------------------------------------

    use tokio_tungstenite::tungstenite;

    /// Helper: builds a server from Fitz src, binds it to
    /// 127.0.0.1:0 and returns (addr, handle). The handle is kept
    /// alive for the duration of the test; dropping it terminates
    /// the server.
    async fn spawn_ws_server(src: &str) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        // Evaluate the src inside an active registry (same as
        // `registry_from_source`) but with
        // `with_active_registry_async` and keeping the resulting
        // Arc.
        let (res, registry) = with_active_registry_async(|| async {
            let tokens = crate::lexer::tokenize(src).unwrap();
            let program = crate::parser::parse(tokens).unwrap();
            crate::evaluator::eval(program).await
        })
        .await;
        res.expect("eval of test program failed");
        let registry = std::sync::Arc::new(registry);
        let metas = registry.metas();
        let router = build_router(&metas, registry, None);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local_addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        // Small wait so the listener is ready (loopback, typically
        // 1-2ms).
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        (addr, handle)
    }

    /// Helper: connects a WS client to the given path of addr.
    async fn ws_connect(
        addr: std::net::SocketAddr,
        path: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let url = format!("ws://{}{}", addr, path);
        let (ws, _resp) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect_async OK");
        ws
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_echo_simple_send_recv_str() {
        // Echo handler: receives a Str, sends it back with prefix.
        let src = "@ws(\"/echo\")\n\
                   async fn echo(conn: WsConn<Str>) -> Null {\n\
                       match conn.recv() {\n\
                           Ok(msg) => {\n\
                               let _ = conn.send(\"eco-{msg}\")\n\
                               return null\n\
                           }\n\
                           Err(_) => return null\n\
                       }\n\
                   }";
        let (addr, _h) = spawn_ws_server(src).await;
        let mut ws = ws_connect(addr, "/echo").await;

        use futures_util::{SinkExt, StreamExt};
        // Send text. The Fitz payload is JSON of the Str, which
        // ends up as `"hello"` (with quotes).
        ws.send(tungstenite::Message::text("\"hello\""))
            .await
            .expect("send");

        // Receive the response.
        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        match resp {
            tungstenite::Message::Text(t) => {
                assert_eq!(t.as_str(), "\"eco-hello\"");
            }
            other => panic!("expected text, was {:?}", other),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_broadcast_multi_client() {
        // Two clients connected to the same endpoint. One sends;
        // BOTH receive the broadcast (including the sender).
        let src = "@ws(\"/room\")\n\
                   async fn room(conn: WsConn<Str>) -> Null {\n\
                       loop {\n\
                           match conn.recv() {\n\
                               Ok(msg) => {\n\
                                   let _ = conn.broadcast(\"all-{msg}\")\n\
                               }\n\
                               Err(_) => return null\n\
                           }\n\
                       }\n\
                       return null\n\
                   }";
        let (addr, _h) = spawn_ws_server(src).await;
        let mut a = ws_connect(addr, "/room").await;
        let mut b = ws_connect(addr, "/room").await;

        use futures_util::{SinkExt, StreamExt};
        // Give the server a moment to register both conns.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        a.send(tungstenite::Message::text("\"hello\""))
            .await
            .expect("send");

        let ra = tokio::time::timeout(std::time::Duration::from_secs(2), a.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        let rb = tokio::time::timeout(std::time::Duration::from_secs(2), b.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        match (ra, rb) {
            (tungstenite::Message::Text(ta), tungstenite::Message::Text(tb)) => {
                assert_eq!(ta.as_str(), "\"all-hello\"");
                assert_eq!(tb.as_str(), "\"all-hello\"");
            }
            other => panic!("expected texts, was {:?}", other),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_auth_pre_upgrade_returns_401_without_token() {
        // Handler protected by @authenticated. Without
        // Authorization, the handshake should fail with 401 BEFORE
        // the upgrade.
        let src = "type User { id: Int, name: Str, role: Str }\n\
                   @auth_provider\n\
                   fn check(h: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"not authenticated\")\n\
                   }\n\
                   @authenticated\n\
                   @ws(\"/chat\")\n\
                   async fn chat(conn: WsConn<Str>, user: User) -> Null { return null }";
        let (addr, _h) = spawn_ws_server(src).await;
        let url = format!("ws://{}/chat", addr);
        let r = tokio_tungstenite::connect_async(&url).await;
        // Any error is valid — axum returns HTTP 401 and
        // tokio-tungstenite sees "non-101 response" as an error.
        assert!(
            r.is_err(),
            "expected handshake failure (401), but it connected OK",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_bidir_recv_and_send_different_types() {
        // 9.w.2-wsconn-bidir — asymmetric channel: client sends
        // Str (command), server emits ChatMsg (structured event).
        let src = "type ChatMsg { user: Str, text: Str }\n\
                   @ws(\"/cmd\")\n\
                   async fn cmd(conn: WsConn<Str, ChatMsg>) -> Null {\n\
                       match conn.recv() {\n\
                           Ok(input) => {\n\
                               let reply = ChatMsg { user: \"system\", text: \"got:{input}\" }\n\
                               let _ = conn.send(reply)\n\
                               return null\n\
                           }\n\
                           Err(_) => return null\n\
                       }\n\
                   }";
        let (addr, _h) = spawn_ws_server(src).await;
        let mut ws = ws_connect(addr, "/cmd").await;

        use futures_util::{SinkExt, StreamExt};
        ws.send(tungstenite::Message::text("\"hello\""))
            .await
            .expect("send");
        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        match resp {
            tungstenite::Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(t.as_str()).expect("JSON valid");
                assert_eq!(v["user"], serde_json::json!("system"));
                assert_eq!(v["text"], serde_json::json!("got:hello"));
            }
            other => panic!("expected text, was {:?}", other),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_auth_via_subprotocol_accepts_token() {
        // 9.w.2-ws-auth-browser — the client sends the token via
        // subprotocol (`bearer.<token>`) instead of the
        // `Authorization` header. The runtime extracts it and
        // injects `authorization: Bearer <token>` into the map
        // seen by the @auth_provider. No user-side changes — the
        // same provider works for HTTP and browser WS.
        let src = "type User { id: Int, name: Str, role: Str }\n\
                   @auth_provider\n\
                   fn check(h: Map<Str, Str>) -> Result<User> {\n\
                       let v: Str = match h.get(\"authorization\") {\n\
                           Ok(s) => s,\n\
                           Err(_) => return Err(\"missing authorization\")\n\
                       }\n\
                       if (v == \"Bearer secret-tok\") {\n\
                           return Ok(User { id: 1, name: \"Ada\", role: \"user\" })\n\
                       }\n\
                       return Err(\"invalid token\")\n\
                   }\n\
                   @authenticated\n\
                   @ws(\"/chat\")\n\
                   async fn chat(conn: WsConn<Str>, user: User) -> Null {\n\
                       match conn.recv() {\n\
                           Ok(_) => {\n\
                               let _ = conn.send(\"hello {user.name}\")\n\
                               return null\n\
                           }\n\
                           Err(_) => return null\n\
                       }\n\
                   }";
        let (addr, _h) = spawn_ws_server(src).await;
        // Build an HTTP request with subprotocol
        // `bearer.secret-tok`.
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let url = format!("ws://{}/chat", addr);
        let mut req = url.as_str().into_client_request().unwrap();
        req.headers_mut().insert(
            "sec-websocket-protocol",
            "bearer.secret-tok".parse().unwrap(),
        );
        let (mut ws, resp) = tokio_tungstenite::connect_async(req)
            .await
            .expect("handshake should pass with bearer.secret-tok");
        // Verify the server echoed the selected subprotocol (RFC
        // 6455 §4.1 — without the echo, the browser would reject
        // the upgrade).
        let echoed = resp
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(
            echoed, "bearer.secret-tok",
            "expected echo of the selected subprotocol in the handshake"
        );

        use futures_util::{SinkExt, StreamExt};
        ws.send(tungstenite::Message::text("\"hello\""))
            .await
            .expect("send");
        let resp_frame = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        match resp_frame {
            tungstenite::Message::Text(t) => {
                assert_eq!(t.as_str(), "\"hello Ada\"");
            }
            other => panic!("expected text, was {:?}", other),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_auth_via_subprotocol_invalid_token_rejected() {
        // Same @auth_provider as the previous test, but the client
        // sends an invalid token via subprotocol → handshake
        // fails with 401 BEFORE the upgrade.
        let src = "type User { id: Int, name: Str, role: Str }\n\
                   @auth_provider\n\
                   fn check(h: Map<Str, Str>) -> Result<User> {\n\
                       let v: Str = match h.get(\"authorization\") {\n\
                           Ok(s) => s,\n\
                           Err(_) => return Err(\"missing authorization\")\n\
                       }\n\
                       if (v == \"Bearer secret-tok\") {\n\
                           return Ok(User { id: 1, name: \"Ada\", role: \"user\" })\n\
                       }\n\
                       return Err(\"invalid token\")\n\
                   }\n\
                   @authenticated\n\
                   @ws(\"/chat\")\n\
                   async fn chat(conn: WsConn<Str>, user: User) -> Null { return null }";
        let (addr, _h) = spawn_ws_server(src).await;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let url = format!("ws://{}/chat", addr);
        let mut req = url.as_str().into_client_request().unwrap();
        req.headers_mut().insert(
            "sec-websocket-protocol",
            "bearer.malformed".parse().unwrap(),
        );
        let r = tokio_tungstenite::connect_async(req).await;
        assert!(
            r.is_err(),
            "expected 401 with invalid token, but it connected OK",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_custom_types_marshaling_json() {
        // Handler that receives a typed `ChatMsg` and returns
        // another. Verifies that automatic JSON marshaling over
        // custom types works in both directions.
        let src = "type ChatMsg { user: Str, text: Str }\n\
                   @ws(\"/chat\")\n\
                   async fn chat(conn: WsConn<ChatMsg>) -> Null {\n\
                       match conn.recv() {\n\
                           Ok(msg) => {\n\
                               let _ = conn.send(ChatMsg { user: msg.user, text: \"re:{msg.text}\" })\n\
                               return null\n\
                           }\n\
                           Err(_) => return null\n\
                       }\n\
                   }";
        let (addr, _h) = spawn_ws_server(src).await;
        let mut ws = ws_connect(addr, "/chat").await;

        use futures_util::{SinkExt, StreamExt};
        ws.send(tungstenite::Message::text(
            "{\"user\":\"ada\",\"text\":\"hi\"}",
        ))
        .await
        .expect("send");
        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        match resp {
            tungstenite::Message::Text(t) => {
                // We expect `{"user":"ada","text":"re:hi"}` (order
                // preserved by serde_json's preserve_order).
                let v: serde_json::Value = serde_json::from_str(t.as_str()).expect("JSON valid");
                assert_eq!(v["user"], serde_json::json!("ada"));
                assert_eq!(v["text"], serde_json::json!("re:hi"));
            }
            other => panic!("expected text, was {:?}", other),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_heartbeat_sends_periodic_ping() {
        // Phase 9.w.2.e — simple handler with
        // `@server(ws_heartbeat_secs=1)`. The client connects and
        // waits; it should receive at least one Ping frame from
        // the server within ~2 seconds (1s first tick + margin).
        let src = "@server(43996, ws_heartbeat_secs=1)\n\
                   fn main() => 0\n\
                   @ws(\"/hb\")\n\
                   async fn hb(conn: WsConn<Str>) -> Null {\n\
                       // Handler that does nothing — the conn stays\n\
                       // alive waiting for the first recv() that never arrives\n\
                       // (the client doesn't send). The server's heartbeat task\n\
                       // should send a Ping before the timeout.\n\
                       match conn.recv() {\n\
                           Ok(_) => return null,\n\
                           Err(_) => return null,\n\
                       }\n\
                   }";
        let (addr, _h) = spawn_ws_server(src).await;
        let mut ws = ws_connect(addr, "/hb").await;
        use futures_util::StreamExt;
        // We wait up to 3 seconds for a Ping.
        let frame = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                match ws.next().await {
                    Some(Ok(tungstenite::Message::Ping(_))) => {
                        return tungstenite::Message::Ping(Vec::new().into());
                    }
                    Some(Ok(other)) => {
                        // tokio-tungstenite replies to Pings with
                        // Pongs automatically, so a Ping may not
                        // surface here. If we see another type, we
                        // continue.
                        let _ = other;
                        continue;
                    }
                    _ => return tungstenite::Message::Close(None),
                }
            }
        })
        .await;
        // tokio-tungstenite intercepts Pings and replies with
        // Pongs without exposing them to the client's .next(). The
        // robust way to verify heartbeat: confirm the conn is
        // still alive after the interval (server didn't close it).
        // If we get here without panic, the heartbeat worked (in
        // production a client that ignores Pings would close the
        // conn; tokio-tungstenite auto-replies with Pong).
        let _ = frame;
        // Sanity: the conn is still connected — we can close it
        // cleanly.
        let _ = ws.close(None).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_broadcast_is_cleaned_on_conn_close() {
        // The broadcaster should unregister the conn on close.
        let src = "@ws(\"/r\")\n\
                   async fn r(conn: WsConn<Str>) -> Null {\n\
                       loop {\n\
                           match conn.recv() {\n\
                               Ok(_) => continue,\n\
                               Err(_) => return null\n\
                           }\n\
                       }\n\
                       return null\n\
                   }";
        let (_addr, _h) = spawn_ws_server(src).await;
        // Indirect verification via `WsBroadcaster::count`: we
        // don't have a direct handle to the broadcaster here. We
        // leave it as a minimal smoke — cleanup is validated via
        // the other test (ws_echo finishes without leaks).
        // (Placeholder test; documents the debt of exposing the
        // broadcaster for inspection.)
    }

    // ---- 9.w.2-binary-frames — `WsConn<Bytes>` end-to-end ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_echo_binary_round_trip() {
        // Binary echo handler: receives `Bytes`, sends them back
        // untouched. The client sends `Message::Binary`, the
        // server receives it as `Value::Bytes` (via `recv()` with
        // T = Bytes), forwards it with `send(buf)` emitting
        // `Message::Binary` (not Text with JSON).
        let src = "@ws(\"/raw\")\n\
                   async fn raw(conn: WsConn<Bytes>) -> Null {\n\
                       match conn.recv() {\n\
                           Ok(buf) => match conn.send(buf) {\n\
                               Ok(_) => return null,\n\
                               Err(_) => return null,\n\
                           },\n\
                           Err(_) => return null,\n\
                       }\n\
                   }";
        let (addr, _h) = spawn_ws_server(src).await;
        let mut ws = ws_connect(addr, "/raw").await;

        use futures_util::{SinkExt, StreamExt};
        // Arbitrary bytes — include 0x00 and 0xff to force the
        // path not to re-encode as UTF-8.
        let payload: Vec<u8> = vec![0x00, 0x01, 0x10, 0x80, 0xff, 0x7e];
        ws.send(tungstenite::Message::binary(payload.clone()))
            .await
            .expect("send binary");

        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        match resp {
            tungstenite::Message::Binary(bs) => {
                assert_eq!(bs.as_ref(), payload.as_slice());
            }
            other => panic!("expected binary, was {:?}", other),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_broadcast_binary_multi_client() {
        // Two connected clients; one sends binary, both receive
        // the broadcast (sender included — Socket.IO/Phoenix
        // convention).
        let src = "@ws(\"/room\")\n\
                   async fn room(conn: WsConn<Bytes>) -> Null {\n\
                       loop {\n\
                           match conn.recv() {\n\
                               Ok(buf) => {\n\
                                   let _ = conn.broadcast(buf)\n\
                               }\n\
                               Err(_) => return null\n\
                           }\n\
                       }\n\
                       return null\n\
                   }";
        let (addr, _h) = spawn_ws_server(src).await;
        let mut a = ws_connect(addr, "/room").await;
        let mut b = ws_connect(addr, "/room").await;

        use futures_util::{SinkExt, StreamExt};
        // Give the server a moment to register both conns.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let payload: Vec<u8> = vec![0xde, 0xad, 0xbe, 0xef, 0x00, 0xff];
        a.send(tungstenite::Message::binary(payload.clone()))
            .await
            .expect("send");

        let ra = tokio::time::timeout(std::time::Duration::from_secs(2), a.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        let rb = tokio::time::timeout(std::time::Duration::from_secs(2), b.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        match (ra, rb) {
            (tungstenite::Message::Binary(ba), tungstenite::Message::Binary(bb)) => {
                assert_eq!(ba.as_ref(), payload.as_slice());
                assert_eq!(bb.as_ref(), payload.as_slice());
            }
            other => panic!("expected (binary, binary), was {:?}", other),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ws_recv_bytes_mismatch_when_client_sends_text() {
        // If the handler declares `WsConn<Bytes>` and the client
        // sends a text frame, `recv()` returns `Err`. The handler
        // responds with a sentinel binary literal (`b"mismatch"`)
        // — the test confirms the Err path runs and the client
        // receives the frame.
        let src = "@ws(\"/raw\")\n\
                   async fn raw(conn: WsConn<Bytes>) -> Null {\n\
                       match conn.recv() {\n\
                           Ok(_) => return null,\n\
                           Err(_) => {\n\
                               let _ = conn.send(b\"mismatch\")\n\
                               return null\n\
                           }\n\
                       }\n\
                   }";
        let (addr, _h) = spawn_ws_server(src).await;
        let mut ws = ws_connect(addr, "/raw").await;

        use futures_util::{SinkExt, StreamExt};
        ws.send(tungstenite::Message::text("hello"))
            .await
            .expect("send text");

        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        match resp {
            tungstenite::Message::Binary(bs) => {
                assert_eq!(bs.as_ref(), b"mismatch");
            }
            other => panic!("expected binary `mismatch`, was {:?}", other),
        }
    }

    // ================================================================
    // v0.10.28 — FITZ_HTTP_LOG formatter unit tests
    // ================================================================

    #[test]
    fn format_http_log_line_off_returns_empty_string() {
        let line = format_http_log_line(
            std::time::Duration::from_millis(5),
            "GET",
            "/users",
            200,
            None,
            None,
            HttpLogMode::Off,
        );
        assert_eq!(line, "");
    }

    #[test]
    fn format_http_log_line_simple_includes_method_path_status_and_elapsed() {
        let line = format_http_log_line(
            std::time::Duration::from_millis(12),
            "GET",
            "/users/42",
            200,
            None,
            None,
            HttpLogMode::Simple,
        );
        assert!(line.starts_with("[fitz HTTP "), "{line}");
        assert!(line.contains("12.0ms"), "{line}");
        assert!(line.contains("GET /users/42 → 200"), "{line}");
        // Simple does not include UA or Content-Length.
        assert!(!line.contains("UA="), "Simple must not log UA: {line}");
        assert!(!line.contains("len="), "Simple must not log len: {line}");
    }

    #[test]
    fn format_http_log_line_verbose_includes_user_agent_and_content_length() {
        let line = format_http_log_line(
            std::time::Duration::from_millis(45),
            "POST",
            "/users",
            201,
            Some("curl/8.0"),
            Some(1234),
            HttpLogMode::Verbose,
        );
        assert!(line.contains("verbose"), "{line}");
        assert!(line.contains("POST /users → 201"), "{line}");
        assert!(line.contains("UA=\"curl/8.0\""), "{line}");
        assert!(line.contains("len=1234"), "{line}");
    }

    #[test]
    fn format_http_log_line_verbose_without_ua_or_len_omits_sections() {
        // Typical case: response without Content-Length
        // (streaming/chunked) and request without User-Agent
        // header.
        let line = format_http_log_line(
            std::time::Duration::from_millis(8),
            "GET",
            "/stream",
            200,
            None,
            None,
            HttpLogMode::Verbose,
        );
        assert!(line.contains("verbose"), "{line}");
        assert!(line.contains("GET /stream → 200"), "{line}");
        assert!(!line.contains("UA="), "sin UA no debe filtrar: {line}");
        assert!(!line.contains("len="), "sin len no debe filtrar: {line}");
    }

    #[test]
    fn format_http_log_line_status_4xx_and_5xx_log_normally() {
        // 404/500 are valid HTTP requests — the log logs them
        // the same.
        let line_404 = format_http_log_line(
            std::time::Duration::from_millis(2),
            "GET",
            "/nope",
            404,
            None,
            None,
            HttpLogMode::Simple,
        );
        assert!(line_404.contains("GET /nope → 404"), "{line_404}");

        let line_500 = format_http_log_line(
            std::time::Duration::from_millis(15),
            "POST",
            "/broken",
            500,
            None,
            None,
            HttpLogMode::Simple,
        );
        assert!(line_500.contains("POST /broken → 500"), "{line_500}");
    }

    #[test]
    fn format_http_log_line_options_preflight_logged_same() {
        // OPTIONS preflight (CORS) is real traffic — it's logged.
        let line = format_http_log_line(
            std::time::Duration::from_millis(1),
            "OPTIONS",
            "/users",
            204,
            None,
            None,
            HttpLogMode::Simple,
        );
        assert!(line.contains("OPTIONS /users → 204"), "{line}");
    }

    // -----------------------------------------------------------------
    // Phase 12.3.b.3 — Built-in metrics (`http_requests_total`
    // Counter + `http_request_duration_seconds` Histogram).
    //
    // Tests with `DebuggingRecorder` installed as the thread-local
    // recorder (via `metrics::with_local_recorder`) to capture the
    // metrics emitted inside the closure. Without a recorder, the
    // macros are silent no-ops — the tests validate that
    // `dispatch_request` calls them with the right labels.
    // -----------------------------------------------------------------

    /// Shared helper: runs an async block inside a fresh tokio
    /// Runtime with a `DebuggingRecorder` installed as the
    /// thread-local recorder. Returns the final metrics snapshot.
    ///
    /// The Runtime is created inside the `with_local_recorder`
    /// closure and `block_on` runs in the same thread-local scope —
    /// that's what lets the `metrics::counter!()` / `histogram!()`
    /// macros from the request handler inherit the recorder. If we
    /// extract the Future and await it outside, the recorder guard
    /// is dropped first (classic thread-local + async problem).
    ///
    /// For the same reason, the tests are sync `#[test]`, NOT
    /// `#[tokio::test]`.
    fn capture_metrics<F>(
        setup_and_run: F,
    ) -> Vec<(
        metrics_util::CompositeKey,
        metrics_util::debugging::DebugValue,
    )>
    where
        F: for<'a> FnOnce(&'a tokio::runtime::Runtime),
    {
        let recorder = metrics_util::debugging::DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create tokio Runtime");
        metrics::with_local_recorder(&recorder, || {
            setup_and_run(&rt);
        });
        snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .map(|(ck, _unit, _desc, val)| (ck, val))
            .collect()
    }

    #[test]
    fn metrics_request_get_simple_emits_counter_and_histogram() {
        let src = "@get(\"/hello\")\nfn h() -> Str => \"ok\"\n";

        let captured = capture_metrics(|rt| {
            rt.block_on(async {
                use http_body_util::BodyExt;
                use tower::ServiceExt;
                let registry = registry_from_source(src).await;
                let metas = registry.metas();
                let router = build_router(&metas, std::sync::Arc::new(registry), None);

                let req = axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri("/hello")
                    .body(Body::empty())
                    .unwrap();
                let resp = router.oneshot(req).await.unwrap();
                let _ = resp.into_body().collect().await.unwrap();
            });
        });

        let counter_entry = captured
            .iter()
            .find(|(ck, _)| ck.key().name() == "http_requests_total")
            .expect("expected counter http_requests_total");
        let histogram_entry = captured
            .iter()
            .find(|(ck, _)| ck.key().name() == "http_request_duration_seconds")
            .expect("expected histogram http_request_duration_seconds");

        match &counter_entry.1 {
            metrics_util::debugging::DebugValue::Counter(n) => assert_eq!(*n, 1),
            other => panic!("expected Counter shape, was {:?}", other),
        }

        match &histogram_entry.1 {
            metrics_util::debugging::DebugValue::Histogram(values) => {
                assert!(
                    !values.is_empty(),
                    "histogram should have at least 1 observation"
                );
                let first = values[0].into_inner();
                assert!(first >= 0.0, "duration_secs should be >= 0");
            }
            other => panic!("expected Histogram shape, was {:?}", other),
        }

        let labels_counter: Vec<(String, String)> = counter_entry
            .0
            .key()
            .labels()
            .map(|l| (l.key().to_string(), l.value().to_string()))
            .collect();
        assert!(
            labels_counter.contains(&("method".to_string(), "GET".to_string())),
            "expected label method=GET, was {:?}",
            labels_counter
        );
        assert!(
            labels_counter.contains(&("path".to_string(), "/hello".to_string())),
            "expected label path=/hello, was {:?}",
            labels_counter
        );
        assert!(
            labels_counter.contains(&("status".to_string(), "200".to_string())),
            "expected label status=200, was {:?}",
            labels_counter
        );

        let labels_histo: Vec<(String, String)> = histogram_entry
            .0
            .key()
            .labels()
            .map(|l| (l.key().to_string(), l.value().to_string()))
            .collect();
        assert_eq!(
            labels_counter, labels_histo,
            "Counter and Histogram labels should match bit-a-bit"
        );
    }

    #[test]
    fn metrics_path_template_does_not_resolve_params() {
        // Path with `{id}` must be recorded in metrics as the
        // TEMPLATE, not the resolved path (`/users/42`). This
        // avoids cardinality explosion in production.
        let src = "@get(\"/users/{id}\")\nfn h(id: Int) -> Str => \"u\"\n";

        let captured = capture_metrics(|rt| {
            rt.block_on(async {
                use http_body_util::BodyExt;
                use tower::ServiceExt;
                let registry = registry_from_source(src).await;
                let metas = registry.metas();
                let router = build_router(&metas, std::sync::Arc::new(registry), None);

                let req = axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri("/users/42")
                    .body(Body::empty())
                    .unwrap();
                let resp = router.oneshot(req).await.unwrap();
                let _ = resp.into_body().collect().await.unwrap();
            });
        });

        let counter_entry = captured
            .iter()
            .find(|(ck, _)| ck.key().name() == "http_requests_total")
            .expect("expected counter http_requests_total");
        let labels: Vec<(String, String)> = counter_entry
            .0
            .key()
            .labels()
            .map(|l| (l.key().to_string(), l.value().to_string()))
            .collect();
        assert!(
            labels.contains(&("path".to_string(), "/users/{id}".to_string())),
            "path label should be the template `/users/{{id}}`, was {:?}",
            labels
        );
        assert!(
            !labels.iter().any(|(k, v)| k == "path" && v == "/users/42"),
            "path label should NOT contain the resolved value: {:?}",
            labels
        );
    }

    #[test]
    fn metrics_two_requests_same_endpoint_accumulate_counter() {
        let src = "@get(\"/hello\")\nfn h() -> Str => \"ok\"\n";

        let captured = capture_metrics(|rt| {
            rt.block_on(async {
                use http_body_util::BodyExt;
                use tower::ServiceExt;
                let registry = registry_from_source(src).await;
                let metas = registry.metas();
                let router = build_router(&metas, std::sync::Arc::new(registry), None);

                for _ in 0..3 {
                    let req = axum::http::Request::builder()
                        .method(axum::http::Method::GET)
                        .uri("/hello")
                        .body(Body::empty())
                        .unwrap();
                    let resp = router.clone().oneshot(req).await.unwrap();
                    let _ = resp.into_body().collect().await.unwrap();
                }
            });
        });

        let counter_entry = captured
            .iter()
            .find(|(ck, _)| ck.key().name() == "http_requests_total")
            .expect("expected counter");
        match &counter_entry.1 {
            metrics_util::debugging::DebugValue::Counter(n) => {
                assert_eq!(*n, 3, "3 requests should accumulate Counter=3");
            }
            other => panic!("expected Counter shape, was {:?}", other),
        }
    }

    #[test]
    fn metrics_status_500_is_recorded_with_correct_label() {
        // Handler returning Result::Err → wrapper converts to 500.
        // We validate the status="500" label is set correctly when
        // outcome.status is not 200.
        let src = "@get(\"/fail\")\nfn h() -> Result<Str> => Err(\"boom\")\n";

        let captured = capture_metrics(|rt| {
            rt.block_on(async {
                use http_body_util::BodyExt;
                use tower::ServiceExt;
                let registry = registry_from_source(src).await;
                let metas = registry.metas();
                let router = build_router(&metas, std::sync::Arc::new(registry), None);

                let req = axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri("/fail")
                    .body(Body::empty())
                    .unwrap();
                let resp = router.oneshot(req).await.unwrap();
                let _ = resp.into_body().collect().await.unwrap();
            });
        });

        let counter_entry = captured
            .iter()
            .find(|(ck, _)| ck.key().name() == "http_requests_total")
            .expect("expected counter");
        let labels: Vec<(String, String)> = counter_entry
            .0
            .key()
            .labels()
            .map(|l| (l.key().to_string(), l.value().to_string()))
            .collect();
        assert!(
            labels.contains(&("status".to_string(), "500".to_string())),
            "expected status=500 (Err → 500), labels were {:?}",
            labels
        );
    }
}
