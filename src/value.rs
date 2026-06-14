// value.rs — Phase 2.4 (Shared migrated to Arc<Mutex<>> in F17.2)
//
// Runtime value representation. A Fitz program evaluates down to a tree
// of `Value`s. This is the currency the evaluator operates on.
//
// Design notes:
//  - Floats and ints promote to each other in operations (1 + 1.0 == 2.0)
//    just like in Python. Here we only define the data; promotion lives
//    in the evaluator.
//  - `Display` shows values as the user would see them with `print`.
//    Strings come out without quotes; floats always carry `.0` when
//    they have no decimals (to tell them apart visually from ints).
//  - `PartialEq` is implemented by hand because the Int↔Float
//    comparison needs coercion (1 == 1.0 → true). Deriving it would
//    make that equality return false.
//  - `Value::Function` keeps a handle (`EnvRef`) to the environment
//    where the function was defined. This creates a mutual
//    value↔env dependency, but Rust accepts it because
//    `Arc<Mutex<>>` is an indirection: the size of `Value` does
//    not depend on the size of `Environment`.
//  - **F17.2**: `Shared<T>` migrated from `Rc<RefCell<T>>` to
//    `Arc<parking_lot::Mutex<T>>`. The change is transparent for the
//    call sites that used `.borrow()`/`.borrow_mut()` — both now map
//    to `.lock()` (parking_lot::Mutex does not distinguish reads
//    from writes).
//  - **F17.3**: `Value` is already `Send` post-F17.2 (the containers
//    are `Arc<Mutex<>>`) and the evaluator moved to `#[async_recursion]`
//    without `(?Send)`. `FitzFuture` now carries `+ Send`. F17.4 is
//    left for switching the tokio runtime to `rt-multi-thread`, and
//    F17.5 for removing the HTTP mpsc/oneshot bridge.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::ast::{Field, Param, Stmt};
use crate::env::EnvRef;
use crate::error::FitzResult;

/// Pending evaluator future. It is built when calling a Fitz `async fn`
/// without `.await` (storing the bare future) or from async builtins
/// like `sleep`. `.await` unpacks it into the inner `FitzResult<Value>`.
///
/// **`+ Send` post-F17.3**: the evaluator moved to `#[async_recursion]`
/// without `(?Send)`. That requires every eval future to be `Send`,
/// and the requirement propagates here: the `Value::Future`s the
/// language exposes must also carry a `Send` future. The bound holds
/// because the shared containers (`Shared<T>` = `Arc<Mutex<T>>`,
/// `EnvRef`) are already `Send` post-F17.2, and the rest of the
/// captures from the eval (`Vec<Stmt>`, `Param`, shallow `Value`)
/// already met the bound. This unlocks `tokio::spawn` and
/// `rt-multi-thread`.
pub type FitzFuture = Pin<Box<dyn Future<Output = FitzResult<Value>> + Send>>;

/// Mini-batch Mw-Wrap — opaque wrapper for the `Value::NativeFn`
/// callback. The `Arc` allows cheap cloning (the `Value`s get cloned
/// along the pipeline). Send + Sync to flow across multi-thread tokio
/// runtimes (post-F17.4). The input is `Vec<Value>` for uniformity
/// with the rest of the call convention (arity 0 = empty vec); for
/// `next: Fn() -> Response` it is always empty. Wrapper struct (not a
/// type alias) so we can implement `Debug` (the `dyn Fn` does not
/// derive it).
#[derive(Clone)]
pub struct NativeAsyncFn(pub Arc<dyn Fn(Vec<Value>) -> FitzFuture + Send + Sync>);

impl std::fmt::Debug for NativeAsyncFn {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "NativeAsyncFn(<native>)")
    }
}

/// Wrapper around the pending future that supplies a manual `Debug`.
/// The `dyn Future` does not implement `Debug`, so we cannot derive
/// it on `Value`. The cell wraps `Option<...>` so `.take()` extracts
/// the future when `.await`ing without cloning (futures are consumed
/// once).
///
/// **F17.2-3**: uses `Arc<Mutex<>>` like the rest of `Shared<T>`.
/// Since `FitzFuture` carries `+ Send` post-F17.3, `Mutex<Option<FitzFuture>>`
/// is `Send + Sync` and `FutureCell` is Send — a `Value::Future` can
/// travel across tokio tasks once the runtime becomes
/// `rt-multi-thread` (F17.4).
pub struct FutureCell(pub Arc<Mutex<Option<FitzFuture>>>);

/// Phase 9.w.2 — Opaque handle to an open WebSocket connection. The
/// HTTP runtime builds it after the HTTP→WS upgrade and injects it
/// into the `@ws("/path")` handler as `Value::WsConn(Arc<WsConnHandle>)`.
///
/// Design:
///   - `rx`: read half of the WebSocket (axum SplitStream). `recv()`
///     locks it, awaits the next frame, parses against T.
///   - `outbox_tx`: a per-conn mpsc channel that a "writer task"
///     drains → pushes to the socket sink. `send(msg)` and
///     `broadcast(msg)` push to the outbox without contending on
///     the sink.
///   - `broadcaster`: shared handle to the per-endpoint registry. It
///     lets `broadcast(msg)` iterate the outboxes of ALL live conns
///     on the endpoint (including the sender — Socket.IO/Phoenix
///     convention).
///   - `endpoint`: the `@ws("/x")` decorator path. Broadcast scope.
///   - `conn_id`: unique id of the conn inside the broadcaster, used
///     for unregister on close.
///   - `closed`: atomic flag set by `close()`. Methods check it
///     before any operation for fail-fast with a clear `Err`.
///
/// The concrete type lives in `http.rs` to avoid leaking axum /
/// tokio-tungstenite types into `value.rs`. Here we declare it as an
/// opaque `dyn` with the minimum methods the evaluator/codegen need
/// to dispatch.
///
/// Only exists at runtime — Display prints `<ws-conn>`, type_name
/// `WsConn`, JSON serialisation rejects it (the conn is not
/// marshallable to JSON; the `T` it carries individually is).
pub struct WsConnHandle {
    /// Endpoint path (e.g. `"/chat"`).
    pub endpoint: String,
    /// Unique id of the conn inside the broadcaster. Unique until the
    /// server restarts. AtomicU64 guarantees non-collision under
    /// concurrency.
    pub conn_id: u64,
    /// Read half of the WebSocket. `recv()` locks it while it waits
    /// for the next frame; during that time `send`/`broadcast` stay
    /// free (separate locks).
    ///
    /// We use `tokio::sync::Mutex` (not `parking_lot::Mutex`) because
    /// `recv()` needs to hold the lock across an `.await` — only
    /// `tokio::sync::Mutex` guarantees `MutexGuard: Send` for use in
    /// `Send` futures. The rest of the codebase uses parking_lot for
    /// sync locks, but here the pattern is async-aware.
    pub rx: Arc<tokio::sync::Mutex<WsReadStream>>,
    /// Conn outbox. `send(msg)` from this conn and `broadcast(msg)`
    /// from OTHER conns write here; a writer task drains → pushes to
    /// the socket sink. Unbounded so it does not block the handler.
    pub outbox_tx: tokio::sync::mpsc::UnboundedSender<WsOutMessage>,
    /// Atomic flag — `true` once the conn closed (handler returned,
    /// `close()` invoked, or the writer task detected the sink
    /// closed). The conn methods check it on entry for fail-fast.
    pub closed: Arc<std::sync::atomic::AtomicBool>,
    /// Handle to the shared broadcaster (per `HttpRegistry`). Lets
    /// `broadcast(msg)` look up the endpoint's outboxes without going
    /// through `HttpRegistry`.
    pub broadcaster: Arc<dyn WsBroadcasterTrait + Send + Sync>,
    /// Phase 9.w.2 — TypeExpr of T in `WsConn<T>`. `recv()` uses it
    /// to coerce incoming `Map`s into `Instance` when T is nominal
    /// (parallel to the 8.4.3 coercion in `Stmt::Assign`). `None`
    /// for conns built in tests with no type context.
    ///
    /// 9.w.2-wsconn-bidir (v0.9.38): `recv_type` and `send_type` can
    /// differ for asymmetric channels (`WsConn<In, Out>`). `recv()`
    /// uses `recv_type` to deserialise/coerce; `send()`/`broadcast()`
    /// use `send_type` to choose between binary mode and JSON text.
    /// For a symmetric `WsConn<T>`, both are `Some(T)` with the same
    /// TypeExpr.
    pub msg_type: Option<crate::ast::TypeExpr>,
    pub send_type: Option<crate::ast::TypeExpr>,
    /// Phase 9.w.2 — EnvRef of the scope where the handler was
    /// declared. Needed to resolve `msg_type` when `T` is nominal
    /// (the nominal's `Value::Type` lives in the env). `Arc<Mutex<>>`
    /// — cheap clone.
    pub env: EnvRef,
}

impl std::fmt::Debug for WsConnHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("WsConnHandle")
            .field("endpoint", &self.endpoint)
            .field("conn_id", &self.conn_id)
            .field(
                "closed",
                &self.closed.load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish()
    }
}

/// Alias for the read half — internal typedef so we do not leak the
/// concrete `axum::extract::ws::WebSocket`'s SplitStream into
/// value.rs. The concrete struct lives in `http.rs` and is cast to
/// `Box<dyn>` or stored as a concrete type via generics in the handler.
///
/// MVP decision: we use a trait object to abstract it. The concrete
/// read half is cast to the `WsReadStreamImpl` impl defined in
/// http.rs. This stops `value.rs` from depending on `axum` directly.
pub type WsReadStream = Box<dyn WsReadStreamTrait + Send + Unpin>;

/// Incoming frame read by the read half. Distinguishes text (JSON-
/// marshalled mode, the default for `WsConn<T>` with T ≠ Bytes) from
/// binary (raw mode, exclusive to `WsConn<Bytes>` —
/// 9.w.2-binary-frames).
///
/// The read stream NEVER filters between text and binary: both are
/// exposed to the evaluator/codegen, which discriminates based on
/// the T declared in the handler's `WsConn<T>`. A mismatch (T=Str
/// but Binary arrives, or T=Bytes but Text arrives) → clear `Err`
/// from the conn method (`recv()`).
///
/// Ping/Pong/Close are handled by the axum/tungstenite stack
/// underneath; they are never exposed here.
#[derive(Debug, Clone)]
pub enum IncomingFrame {
    /// UTF-8 text frame. `recv()` with T ≠ Bytes parses it as JSON
    /// and coerces it to the declared T.
    Text(String),
    /// Raw binary frame. `recv()` with T = Bytes exposes it as
    /// `Value::Bytes(...)`.
    Binary(Vec<u8>),
}

/// Trait for the read half — abstraction so we do not leak axum
/// types. Only defines what `recv()` needs: read a frame (text or
/// binary) or detect close.
pub trait WsReadStreamTrait {
    /// Reads the next frame. Returns:
    ///   - `Ok(Some(IncomingFrame::Text(s)))` — text frame.
    ///   - `Ok(Some(IncomingFrame::Binary(bs)))` — binary frame.
    ///   - `Ok(None)` — close frame; the conn closed cleanly.
    ///   - `Err(msg)` — transport error.
    ///
    /// Ping/Pong: the impl handles them internally (axum auto-
    /// replies; we drop them in the internal loop).
    fn next_frame<'a>(
        &'a mut self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<IncomingFrame>, String>> + Send + 'a>,
    >;
}

/// "Outbox" message — text, bytes, or close signal. The conn's
/// writer task consumes it and translates it into the matching axum
/// frame.
#[derive(Debug, Clone)]
pub enum WsOutMessage {
    /// Text frame with `payload` (JSON serialisation of T when T ≠ Bytes).
    Text(String),
    /// 9.w.2-binary-frames — raw binary frame. Built by
    /// `WsConn<Bytes>.send(...)` / `.broadcast(...)`. The writer task
    /// translates it to `axum::extract::ws::Message::Binary(...)`.
    Binary(Vec<u8>),
    /// Close request. The writer task processes it and exits.
    Close,
    /// Phase 9.w.2.e — heartbeat ping. The writer task translates it
    /// to `axum::extract::ws::Message::Ping(...)`. If sink.send()
    /// fails, the writer task exits and `closed` is set (which the
    /// heartbeat task also detects on its next iteration).
    Ping,
}

/// Trait for the broadcaster — abstraction that
/// `WsConnHandle.broadcaster` implements. To stop `value.rs` from
/// depending on `http.rs` (where the concrete broadcaster lives),
/// we expose `broadcast_text` and `broadcast_binary`
/// (9.w.2-binary-frames split the two to keep types and avoid an
/// extra enum in the API).
///
/// The runtime builds a shared broadcaster per `HttpRegistry`
/// (`Arc<WsBroadcaster>`), registers it on every `WsConnHandle`, and
/// the user-side `broadcast(msg)` delegates to the matching method
/// based on the `WsConn`'s T.
pub trait WsBroadcasterTrait {
    /// Sends `payload` (text frame) to the outbox of EVERY live conn
    /// on `endpoint`, including the conn that invoked
    /// (Socket.IO/Phoenix convention). Conns with a closed outbox
    /// are silently ignored (lazy cleanup).
    fn broadcast_text(&self, endpoint: &str, payload: String);
    /// 9.w.2-binary-frames — binary variant. Same "broadcast to
    /// everyone on the endpoint including the sender" model as
    /// `broadcast_text`.
    fn broadcast_binary(&self, endpoint: &str, payload: Vec<u8>);
}

impl Clone for FutureCell {
    fn clone(&self) -> Self {
        FutureCell(Arc::clone(&self.0))
    }
}

impl std::fmt::Debug for FutureCell {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let occupied = self.0.lock().is_some();
        if occupied {
            write!(f, "FutureCell(pending)")
        } else {
            write!(f, "FutureCell(consumed)")
        }
    }
}

/// Alias for collections shared by reference. Lists, maps and the
/// fields of an instance live behind `Arc<parking_lot::Mutex<>>`:
/// `Arc` allows aliasing (the same collection visible from multiple
/// variables/fields/arguments) and `Mutex` allows mutation through
/// the alias. Same semantics as objects in Python and JS but already
/// thread-safe — it will enable real parallelism between Fitz tasks
/// once F17.3 closes and we drop `(?Send)` from async_recursion.
///
/// `Value::clone()` clones the `Arc` (cheap), not the contents —
/// every copy sees the same data. That is what enables `xs.push(...)`,
/// `user.name = "x"` and every other form of mutation.
///
/// **F17.2**: migrated from `Rc<RefCell<T>>` to
/// `Arc<parking_lot::Mutex<T>>`. `.borrow()` and `.borrow_mut()`
/// both map to `.lock()` — parking_lot does not distinguish reads
/// from writes (if a hot path with concurrent reads eats the extra
/// cost, we evaluate `RwLock`).
pub type Shared<T> = Arc<Mutex<T>>;

/// Constructor for the shared wrapper. Always use `shared(x)` instead
/// of `Arc::new(Mutex::new(x))` directly so the pattern stays
/// uniform.
pub fn shared<T>(value: T) -> Shared<T> {
    Arc::new(Mutex::new(value))
}

/// Opaque handle to a Python object (module, function, instance,
/// etc.) — only exists when the `fitz` binary is built with the
/// `python` feature (Phase 8.1+). Wraps PyO3's `Py<PyAny>` in an
/// `Arc` so `Value::clone()` is O(1) without taking the GIL: the
/// `Arc` counts the Rust-side handle copies, and only when the last
/// handle drops does PyO3 take the GIL to decrement the Python
/// refcount.
///
/// Equality is by Python-object identity (`Py::as_ptr()`), same as
/// `Value::Module` and `Value::Function`. Two distinct handles to
/// the same imported module compare equal.
///
/// Manual Debug (Py<PyAny> does not implement Debug) — produces
/// `PyObjectHandle(<python object>)` without touching Python.
#[cfg(feature = "python")]
pub struct PyObjectHandle(pub Arc<pyo3::Py<pyo3::PyAny>>);

#[cfg(feature = "python")]
impl Clone for PyObjectHandle {
    fn clone(&self) -> Self {
        PyObjectHandle(Arc::clone(&self.0))
    }
}

#[cfg(feature = "python")]
impl std::fmt::Debug for PyObjectHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "PyObjectHandle(<python object>)")
    }
}

#[cfg(feature = "python")]
impl PyObjectHandle {
    /// Builds a handle from an already-acquired `Py<PyAny>` (e.g.
    /// the return of `PyModule::import` inside a `Python::with_gil`).
    /// The caller keeps the responsibility of having taken the GIL
    /// to obtain the original `Py<PyAny>`; this constructor only
    /// wraps.
    ///
    /// `dead_code` allow: the `Value::PyObject` variant and this
    /// constructor are not used in 8.1.1 yet (placeholder only); the
    /// Python loader in `evaluator::load_module` consumes them in
    /// 8.1.2.
    #[allow(dead_code)]
    pub fn new(obj: pyo3::Py<pyo3::PyAny>) -> Self {
        PyObjectHandle(Arc::new(obj))
    }
}

/// Phase 12.2.a — Wrapper for the inner of `Value::Secret` with a
/// custom `Debug` that redacts. It exists as a dedicated type
/// (instead of `Box<Value>` directly) so the `derive(Debug)` on
/// `Value` propagates the redaction automatically — without this,
/// the inner gets printed raw in any `format!("{:?}", value)` and
/// leaks the secret to logs/panics. `PartialEq` is derived,
/// delegating to the inner.
#[derive(Clone, PartialEq)]
pub struct SecretInner(pub Box<Value>);

impl std::fmt::Debug for SecretInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Do NOT show the inner — that is the rule. The `Debug` of
        // the Value that wraps a Secret prints `Secret(<redacted>)`.
        write!(f, "<redacted>")
    }
}

/// A runtime value.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,

    /// Mini-batch Bytes — binary byte sequence. Built via the
    /// `b"..."` literal with hex escapes (`\xHH`) or via the builtin
    /// `bytes_from_str(s)`. Immutable in practice (no `push` is
    /// exposed, to keep the model simple). Clone is O(n).
    Bytes(Vec<u8>),

    /// Mini-batch Mw-Wrap — async native function built by the
    /// runtime and passed as a Value to the user. Today it is used
    /// only for the `next` callable of wrap-style middlewares: the
    /// chain runner builds a `NativeFn` capturing the rest of the
    /// chain + the handler and hands it to the middleware, which
    /// decides when to invoke it (before/after the handler,
    /// conditionally, measuring time, etc.). Send + Sync so it can
    /// flow across tokio runtimes.
    NativeFn(NativeAsyncFn),

    /// Native function implemented in Rust (e.g. `print`).
    /// The signature receives already-evaluated args and returns a
    /// value or error.
    Builtin {
        name: &'static str,
        func: fn(&[Value]) -> FitzResult<Value>,
    },

    /// User-defined function. Stores its parameters, its body, and a
    /// handle to the env where it was defined. That handle is the
    /// "closure": when calling the function we create a child scope
    /// of that env, not of the caller. That gives access to the
    /// variables of the place where it was defined.
    ///
    /// `is_async` (Phase 6.4): mirrors the flag of the original
    /// `Stmt::FnDef`. `FnExpr` always marks it as `false` (anonymous
    /// async fns are not supported today). The call dispatcher
    /// checks it: if an async fn is called without `.await`, it
    /// returns a `Value::Future` wrapping the body evaluation; with
    /// `.await` it unpacks to T.
    Function {
        params: Vec<Param>,
        body: Vec<Stmt>,
        closure: EnvRef,
        is_async: bool,
    },

    /// User-defined custom type (`type User { id: Int }`).
    /// For now it is an inert marker: it lives in the env so the
    /// type name can resolve, but without struct literals it cannot
    /// be instantiated. It becomes useful in Phase 3 (instantiation,
    /// field access).
    ///
    /// PreF8.3: `resolved_defaults` stays empty for types defined
    /// in the current file (their defaults are evaluated lazily on
    /// every instantiation, with the call-site env). For types
    /// imported from another module, the loader pre-evaluates the
    /// `Field.default`s in the origin env and materialises them
    /// here. The struct literal prefers `resolved_defaults` before
    /// falling back to `Field.default` as Expr, so an imported
    /// default can reference consts or other symbols from the
    /// origin module without the importer having to re-import them.
    Type {
        name: String,
        fields: Vec<Field>,
        resolved_defaults: Vec<(String, Value)>,
        /// R.3 (mini-phase R) — custom methods declared inside the
        /// `type`. Dispatch on `Value::Instance` looks them up by
        /// name here first; if not found, it falls back to the
        /// built-in methods.
        methods: Vec<crate::ast::MethodDef>,
        /// Phase 10.3.b — cached ORM metadata: if the type has
        /// `@table(...)`, contains the SQL name + primary field +
        /// column overrides. `None` for normal Fitz types without
        /// `@table`. The evaluator populates it on `Stmt::TypeDef`
        /// by calling the checker's `process_table_decorators`.
        ///
        /// Caching here (instead of re-querying `TypeEnv`) gives the
        /// runtime access to the metadata without going through the
        /// checker — important because the evaluator only has
        /// `EnvRef`, not `TypeEnv`.
        ///
        /// `Box` to keep the enum size small (TableMetadata weighs
        /// ~100 bytes; without Box it bloats every Value and trips
        /// `clippy::result_large_err` on hundreds of signatures
        /// returning `Result<_, EvalSignal>`).
        table_metadata: Option<Box<crate::types::TableMetadata>>,
    },

    /// Runtime tuple (mini-batch T). Heterogeneous, fixed size known
    /// at compile time. NOT shared by reference (value semantics —
    /// cloning the tuple clones every slot). The order matches the
    /// declaration; access is by index (`Expr::TupleField`) or by
    /// destructuring (Pattern::Tuple).
    Tuple(Vec<Value>),

    /// Runtime list. Shared by reference (`Shared<T>` =
    /// `Arc<Mutex<>>` post-F17.2) so `xs.push(...)`, passing the
    /// list to a function, or storing it in an instance field all
    /// talk about the same data. Build with `Value::new_list(vec)`.
    List(Shared<Vec<Value>>),

    /// Runtime map. `Vec<(K, V)>` instead of `HashMap` for two
    /// reasons:
    ///  - preserves insertion order (matters for `print` and for
    ///    future iteration).
    ///  - accepts non-hashable keys without complicating `Value`.
    ///    Access is O(n); optimisable later when it matters.
    ///
    /// Shared by reference, same criterion as `List`.
    Map(Shared<Vec<(Value, Value)>>),

    /// Exclusive Int range. Iterable. Int-only for now (Float has no
    /// clear discrete semantics for iteration).
    Range {
        start: i64,
        end: i64,
    },

    /// Instance of a custom type: the result of evaluating a struct
    /// literal `User { id: 1, name: "x" }`. Stores the type name
    /// (for `Display` and error messages) and the `(field, value)`
    /// pairs in the order the `type` declared them.
    ///
    /// Order is stable: the evaluator builds it following the
    /// `Value::Type` field list, not the literal's. That guarantees
    /// two instances of the same type print the same even if the
    /// user typed the fields in a different order.
    ///
    /// `fields` is shared (`Shared<T>` = `Arc<Mutex<>>` post-F17.2)
    /// so `user.name = "x"` works: the mutation is visible through
    /// any alias to this instance. Build with
    /// `Value::new_instance(...)`.
    Instance {
        type_name: String,
        fields: Shared<Vec<(String, Value)>>,
    },

    /// Built-in sum type `Result`: represents the outcome of an
    /// operation that may have failed. Success or error variant,
    /// each carrying any value inside.
    ///
    /// Modelled with its own variant (not as `Instance`) because
    /// `Result` is a sum type, not a product type: it has
    /// alternatives, not fields. The Display, equality and matching
    /// rules would have to be special if we reused it on
    /// `Instance`; a dedicated type is better.
    Result(ResultVariant),

    /// Module loaded from another file. Result of an `import` that
    /// exposes the whole module as a namespace: `import utils`
    /// binds a `Value::Module` under the name `utils`, and
    /// `utils.foo()` resolves `foo` inside the module env.
    ///
    /// `name` is the last segment of the original path (`import
    /// sub.foo` → `name = "foo"`), useful for Display and error
    /// messages.
    ///
    /// `env` is the environment where the module body was
    /// evaluated. The loader freezes it there: the file's top-level
    /// definitions (let, fn, type) live in that env and are visible
    /// via field access. Equality is by `Rc` identity (two
    /// `Value::Module`s are equal if they share the same env — used
    /// to detect that two imports of the same file yielded the same
    /// module).
    Module {
        name: String,
        env: EnvRef,
    },

    /// HTTP response with a custom status code. Only appears as the
    /// product of a `return <Int> { ... }` inside a handler; the
    /// HTTP runtime (in `http.rs`) intercepts it in
    /// `value_to_outcome` to emit a `HandlerOutcome` with the
    /// status and body the user asked for. Outside an HTTP context
    /// it is opaque — it cannot be JSON-serialised or printed, and
    /// the checker rejects `Stmt::ReturnStatus` outside handlers.
    ///
    /// No `Pair` variant: the body stays as `Box<Value>` to reuse
    /// the existing serialisation path. `body = None` is reserved
    /// for 204 No Content (today the parser requires a body; the
    /// field is optional to prepare that extension).
    HttpResponse {
        status: u16,
        body: Option<Box<Value>>,
    },

    /// Opaque CORS configuration, product of the `cors(...)` builtin
    /// (mini-phase MW.2). Used as an argument of
    /// `@middleware(cors(...))` on an HTTP handler; the evaluator
    /// detects it and stores it in the `RouteSpec.cors` slot (it
    /// does not enter the user-fn middleware chain). Outside that
    /// context it is opaque: it cannot be printed or serialised —
    /// using `cors(...)` as a bare expression makes no sense, and
    /// code that tries gets a clear error.
    ///
    /// `Arc<CorsConfig>` (immutable post-build): the config travels
    /// to the tokio thread to configure axum's preflight handler,
    /// so it has to be `Send + Sync`. The payload (`String`s and
    /// `Vec<String>`) already satisfies that. Post-F17 the rest of
    /// `Shared<T>` is also `Arc<Mutex>` — this case stays without
    /// `Mutex` because the config is read-only.
    CorsConfig(Arc<crate::http::CorsConfig>),

    /// Pending future introduced in Phase 6.4. It is built when a
    /// Fitz `async fn` is called without `.await` (storing the bare
    /// future in a variable) or from async builtins (`sleep`).
    /// `Expr::Await` unpacks it; consuming it twice is an
    /// interpreter panic (futures are awaited once).
    ///
    /// Wrapped in `FutureCell` (`Arc<Mutex<Option<...>>>`) for two
    /// reasons: (a) `Value: Clone` and `Pin<Box<dyn Future>>` is
    /// not Clone — the `Arc` gives cheap cloning and shares the
    /// cell; (b) the `Option` lets us extract the future on
    /// `.await` without cloning (move with `.take()`), preserving
    /// the "a future is awaited once" rule.
    Future(FutureCell),

    /// Phase 12.2.a — `Secret<T>` opaque value with auto-redaction.
    /// Produced by the `secret("KEY")` builtin which reads env var
    /// / mounted file `/run/secrets/<KEY>` / `.env` (precedence).
    /// The inner T is stored wrapped in `SecretInner` whose `Debug`
    /// emits `<redacted>` — the `derive(Debug)` on `Value`
    /// propagates that redaction to the whole enum's `Debug`. The
    /// Value's `Display` (manual impl below) emits
    /// `<redacted Secret<T>>`.
    ///
    /// `.expose() -> T` explicitly unpacks the inner (dispatched in
    /// `evaluator::dispatch_method`). It is the only way to get the
    /// raw value — `print(secret)` and `Debug` are blocked. JSON
    /// serialisation also rejects it with an explicit error in
    /// `crate::http::value_to_json` (12.2.b will refine codegen
    /// emit to reach bit-for-bit parity).
    ///
    /// `PartialEq` (manual impl below) compares the inners: two
    /// `Secret<Str>`s are equal if their contents are — useful for
    /// validating passwords against hashes in MVP.
    Secret(SecretInner),

    /// Opaque Python object — only exists with the `python` feature.
    /// Produced by the `from python import <mod>` loader
    /// (Phase 8.1.2) and by attribute accesses / calls that return
    /// non-primitive objects (8.1.3+). In 8.1 primitives
    /// (Int/Float/Str/Bool/None) auto-coerce to native `Value`s on
    /// crossing, so `Value::PyObject` wraps modules, functions,
    /// classes, instances and other opaque callables/containers.
    /// Marshalling of composite types (List/Map/Instance) lands in
    /// 8.2.
    ///
    /// `dead_code` allow: in 8.1.1 the variant exists as a
    /// placeholder (Display/PartialEq/type_name ready); the real
    /// constructor lands in 8.1.2 when `evaluator::load_module`
    /// routes to Python.
    #[cfg(feature = "python")]
    #[allow(dead_code)]
    PyObject(PyObjectHandle),

    /// Phase 9.w.2 — Open WebSocket connection. The HTTP runtime
    /// builds it after the HTTP→WS upgrade and injects it as the
    /// argument of the `@ws("/path")` handler. Opaque to the user:
    /// it is only accessed via the 4 parametric methods of the
    /// checker (`recv`/`send`/`broadcast`/`close`).
    ///
    /// Equality by `Arc` identity — two references to the same
    /// conn share state; distinct conns are always different. Not
    /// JSON-serialisable (see `value_to_json` in `http.rs` — it
    /// rejects with a clear message). Display: `<ws-conn>`.
    WsConn(Arc<WsConnHandle>),

    /// Phase 10.1.b — Open Postgres connection. Produced by the
    /// `db.connect(url).await` builtin and consumed via the
    /// `query/exec/close` methods dispatched by `dispatch_method`.
    /// Opaque to the user: just like WsConn, it is only accessed
    /// via driver-specific methods (`src/db.rs`).
    ///
    /// Equality by `Arc` identity — parallel to WsConn. Not
    /// JSON-serialisable (it is a handle to a system resource,
    /// not a value). Display: `<db-conn user@host/db>` with the
    /// URL redacted (no password).
    DbConn(Arc<crate::db::DbConnHandle>),

    /// Phase 10.3.b2 — ORM query builder accumulating state.
    /// Produced by `User.where(closure)` and consumed by
    /// `.all(db)`/`.first(db)`/`.count(db)`. Immutable: every chain
    /// call returns a NEW QueryBuilder with the accumulated state
    /// (functional semantics).
    ///
    /// The concrete struct (`QueryBuilderState`) lives in
    /// `evaluator.rs`. We wrap it in `Arc<dyn Any + Send + Sync>`
    /// to avoid the `evaluator → value → evaluator` import cycle
    /// (`evaluator` already depends on `value`; we cannot
    /// reference `evaluator::QueryBuilderState` from here). The
    /// downcast lives in `dispatch_method` when the value arrives
    /// as the receiver.
    ///
    /// Equality by `Arc` identity. Opaque to the user; no Display
    /// or JSON serialisation.
    QueryBuilder(Arc<dyn std::any::Any + Send + Sync>),

    /// v0.10.24 — date with no time or tz. Wrapper around
    /// `chrono::NaiveDate`. Built via `Date.today()` /
    /// `Date.parse("2026-05-30")`. Display: ISO 8601
    /// `YYYY-MM-DD`. Structural equality (chrono impls PartialEq).
    Date(chrono::NaiveDate),

    /// v0.10.24 — date + time + tz (always UTC in MVP). Wrapper
    /// around `chrono::DateTime<chrono::Utc>`. Display: ISO 8601
    /// `YYYY-MM-DDTHH:MM:SSZ`. Structural equality.
    DateTime(chrono::DateTime<chrono::Utc>),

    /// v0.10.24 — UUID. Wrapper around `uuid::Uuid`. Built via
    /// `Uuid.v4()` (random) or `Uuid.parse("...")`. Display:
    /// canonical format `xxxxxxxx-xxxx-Mxxx-Nxxx-xxxxxxxxxxxx`
    /// (lowercase, with dashes). Structural equality.
    Uuid(uuid::Uuid),
}

/// `Value::Result` variant. Uses `Box<Value>` to avoid an
/// infinitely-sized recursive enum (same trick as `Box<Expr>` in
/// the AST).
#[derive(Debug, Clone)]
pub enum ResultVariant {
    Ok(Box<Value>),
    Err(Box<Value>),
}

impl Value {
    /// Builds a `Value::List` from a `Vec<Value>`. Always wrap with
    /// this constructor to keep the `Shared<T>` =
    /// `Arc<Mutex<>>` (post-F17.2) wrapping uniform and avoid
    /// scattering `Arc::new(Mutex::new(...))` everywhere.
    pub fn new_list(items: Vec<Value>) -> Value {
        Value::List(shared(items))
    }

    /// Builds a `Value::Map` from a `Vec<(Value, Value)>`.
    pub fn new_map(pairs: Vec<(Value, Value)>) -> Value {
        Value::Map(shared(pairs))
    }

    /// Builds a `Value::Instance` from the type name and the
    /// `(field, value)` pairs. The order matters: the evaluator
    /// builds it following the `type` declaration.
    pub fn new_instance(type_name: String, fields: Vec<(String, Value)>) -> Value {
        Value::Instance {
            type_name,
            fields: shared(fields),
        }
    }

    /// Builds a `Value::Future` wrapping a native Rust future. Used
    /// by `builtin_sleep` and by the Fitz async fn dispatcher when
    /// called without `.await`. The future runs once: when
    /// `Expr::Await` unpacks it, the `Option` becomes `None` and a
    /// second `.await` panics.
    pub fn new_future(fut: FitzFuture) -> Value {
        Value::Future(FutureCell(Arc::new(Mutex::new(Some(fut)))))
    }

    /// Type name, for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Str(_) => "Str",
            Value::Bool(_) => "Bool",
            Value::Null => "Null",
            Value::Bytes(_) => "Bytes",
            Value::NativeFn(_) => "Function",
            Value::Builtin { .. } => "Function",
            Value::Function { .. } => "Function",
            Value::Type { .. } => "Type",
            Value::List(_) => "List",
            Value::Map(_) => "Map",
            Value::Tuple(_) => "Tuple",
            Value::Range { .. } => "Range",
            Value::Instance { .. } => "Instance",
            Value::Result(_) => "Result",
            Value::HttpResponse { .. } => "HttpResponse",
            Value::Module { .. } => "Module",
            Value::CorsConfig(_) => "CorsConfig",
            Value::Future(_) => "Future",
            Value::Secret(_) => "Secret",
            Value::WsConn(_) => "WsConn",
            Value::DbConn(_) => "DbConn",
            Value::QueryBuilder(_) => "QueryBuilder",
            Value::Date(_) => "Date",
            Value::DateTime(_) => "DateTime",
            Value::Uuid(_) => "Uuid",
            #[cfg(feature = "python")]
            Value::PyObject(_) => "PyObject",
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(x) => {
                // If it has no decimal part we add `.0` so it looks
                // different from an Int. `3.0` → "3.0", `3.14` →
                // "3.14".
                if x.fract() == 0.0 && x.is_finite() {
                    write!(f, "{:.1}", x)
                } else {
                    write!(f, "{}", x)
                }
            }
            Value::Str(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Null => write!(f, "null"),
            Value::Bytes(bs) => {
                // Mini-batch Bytes — `b"..."` format parallel to
                // Rust. Printable ASCII + common escapes (`\n`,
                // `\r`, `\t`, `\\`, `\"`) come through as-is; the
                // rest go as `\xHH`. Same criterion as Rust's
                // `<[u8] as Debug>` for the contents between
                // quotes.
                write!(f, "b\"")?;
                for &b in bs.iter() {
                    match b {
                        b'\\' => write!(f, "\\\\")?,
                        b'"' => write!(f, "\\\"")?,
                        b'\n' => write!(f, "\\n")?,
                        b'\r' => write!(f, "\\r")?,
                        b'\t' => write!(f, "\\t")?,
                        0x20..=0x7e => write!(f, "{}", b as char)?,
                        _ => write!(f, "\\x{:02x}", b)?,
                    }
                }
                write!(f, "\"")
            }
            Value::Builtin { name, .. } => write!(f, "<builtin {}>", name),
            Value::Function { .. } => write!(f, "<function>"),
            Value::Type { name, .. } => write!(f, "<type {}>", name),
            Value::List(items) => {
                // For strings we show quotes inside the list (it is
                // the representation, not the direct `print`
                // output). E.g. `[1, "hola", 2]`. Different from
                // the bare-Str Display, which has no quotes
                // because that case is final output.
                let items = items.lock();
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write_inline_value(f, v)?;
                }
                write!(f, "]")
            }
            Value::Map(pairs) => {
                let pairs = pairs.lock();
                write!(f, "{{")?;
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write_inline_value(f, k)?;
                    write!(f, ": ")?;
                    write_inline_value(f, v)?;
                }
                write!(f, "}}")
            }
            Value::Tuple(items) => {
                // Tuple: `(1, "x", true)`. Strings with quotes
                // inside (same criterion as List/Map/Instance). A
                // single-element tuple carries a trailing comma:
                // `(42,)` to distinguish it from `(42)` (grouping
                // parens).
                write!(f, "(")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write_inline_value(f, v)?;
                }
                if items.len() == 1 {
                    write!(f, ",")?;
                }
                write!(f, ")")
            }
            Value::Range { start, end } => write!(f, "{}..{}", start, end),
            Value::Instance { type_name, fields } => {
                // Format: `User { id: 1, name: "x" }`. Strings with
                // quotes inside (same criterion as List/Map), so
                // `42` and `"42"` can be told apart at a glance.
                let fields = fields.lock();
                write!(f, "{} {{", type_name)?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, " {}: ", k)?;
                    write_inline_value(f, v)?;
                }
                if !fields.is_empty() {
                    write!(f, " ")?;
                }
                write!(f, "}}")
            }
            Value::Result(ResultVariant::Ok(inner)) => {
                write!(f, "Ok(")?;
                write_inline_value(f, inner)?;
                write!(f, ")")
            }
            Value::Result(ResultVariant::Err(inner)) => {
                write!(f, "Err(")?;
                write_inline_value(f, inner)?;
                write!(f, ")")
            }
            Value::Module { name, .. } => write!(f, "<module {}>", name),
            Value::HttpResponse { status, body } => match body {
                Some(b) => {
                    write!(f, "<response {} ", status)?;
                    write_inline_value(f, b)?;
                    write!(f, ">")
                }
                None => write!(f, "<response {}>", status),
            },
            Value::CorsConfig(_) => write!(f, "<cors-config>"),
            Value::Future(_) => write!(f, "<future>"),
            // Phase 12.2.a — Display redacts the inner to prevent
            // accidental leaks in logs/print. The only way to
            // access the raw value is via explicit `.expose()`.
            Value::Secret(_) => write!(f, "<redacted Secret>"),
            Value::WsConn(_) => write!(f, "<ws-conn>"),
            Value::DbConn(h) => write!(f, "<db-conn {}>", h.url_redacted),
            Value::QueryBuilder(_) => write!(f, "<query-builder>"),
            // v0.10.24 — canonical ISO 8601 / UUID Display. No
            // wrapper like `<date 2026-05-30>` because these values
            // are user-facing (they get printed, interpolated, sent
            // to JSON without wrap). Same format as their
            // `to_str()` instance method.
            Value::Date(d) => write!(f, "{}", d.format("%Y-%m-%d")),
            Value::DateTime(dt) => write!(f, "{}", dt.format("%Y-%m-%dT%H:%M:%SZ")),
            Value::Uuid(u) => write!(f, "{}", u),
            Value::NativeFn(_) => write!(f, "<native function>"),
            #[cfg(feature = "python")]
            Value::PyObject(_) => write!(f, "<python object>"),
        }
    }
}

/// "Inline" representation of a Value when it appears inside another
/// (list, map). Strings get quotes so we can tell `[1, "2"]` apart
/// from `[1, 2]` — it is for reading, not for final printing. The
/// rest defers to the normal `Display`.
fn write_inline_value(f: &mut std::fmt::Formatter, v: &Value) -> std::fmt::Result {
    match v {
        Value::Str(s) => write!(f, "\"{}\"", s),
        other => write!(f, "{}", other),
    }
}

/// Equality with Int↔Float coercion. Every other combination returns
/// false. This defines the semantics of `==` in Fitz (used by the
/// evaluator in BinOp::Eq).
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Null, Value::Null) => true,
            // Bytes: byte-by-byte comparison.
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            // List and Map are compared structurally, element by
            // element. The recursive equality delegates back to
            // this impl, so the Int↔Float coercion also works
            // inside lists and maps. If the two `Arc`s point at
            // the same data (alias from the same origin),
            // `Arc::ptr_eq` is a cheap shortcut; otherwise we
            // compare the contents under the lock.
            (Value::List(a), Value::List(b)) => Arc::ptr_eq(a, b) || *a.lock() == *b.lock(),
            (Value::Map(a), Value::Map(b)) => Arc::ptr_eq(a, b) || *a.lock() == *b.lock(),
            // Tuples (mini-batch T): structural comparison by length
            // and elements. The Int↔Float coercion applies
            // recursively via this same impl.
            (Value::Tuple(a), Value::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
            }
            (Value::Range { start: s1, end: e1 }, Value::Range { start: s2, end: e2 }) => {
                s1 == s2 && e1 == e2
            }
            // Instances are compared structurally: same type and
            // same field contents (with the same order, which is
            // guaranteed by the evaluator because it follows the
            // `type` declaration). The Int↔Float coercion applies
            // recursively via this same impl.
            (
                Value::Instance {
                    type_name: t1,
                    fields: f1,
                },
                Value::Instance {
                    type_name: t2,
                    fields: f2,
                },
            ) => t1 == t2 && (Arc::ptr_eq(f1, f2) || *f1.lock() == *f2.lock()),
            // Result is compared variant by variant, recursively.
            // Same Int↔Float coercion inside, via this impl.
            (Value::Result(a), Value::Result(b)) => match (a, b) {
                (ResultVariant::Ok(va), ResultVariant::Ok(vb)) => va == vb,
                (ResultVariant::Err(va), ResultVariant::Err(vb)) => va == vb,
                _ => false,
            },
            // Modules are compared by env identity (same Arc). The
            // loader caches by canonical path, so two imports of
            // the same file produce two `Value::Module`s with the
            // same `Arc<Mutex<Environment>>`. Structural equality
            // makes no sense — the env can hold functions and
            // other non-comparable values.
            (Value::Module { env: e1, .. }, Value::Module { env: e2, .. }) => Arc::ptr_eq(e1, e2),
            // PyObject is compared by Python-object identity
            // (`Py::as_ptr()` gives the underlying `*mut PyObject`,
            // which is unique per live object). Two handles to a
            // twice-imported `math` are equal — Python caches
            // imports the same way our `Value::Module` caches by
            // canonical path. We do not need to take the GIL to
            // read the pointer.
            #[cfg(feature = "python")]
            (Value::PyObject(a), Value::PyObject(b)) => a.0.as_ptr() == b.0.as_ptr(),
            // WsConn — equality by `Arc` identity. Two handles to
            // the same conn share state; distinct conns are never
            // structurally equal (distinct sockets, distinct
            // broadcaster entries).
            (Value::WsConn(a), Value::WsConn(b)) => Arc::ptr_eq(a, b),
            // Phase 12.2.a — Secret: equality delegated to the
            // inner. Lets us validate credentials
            // (`stored == provided`) without exposing the contents.
            // The comparison is structural; a constant-time
            // refinement is left as minor debt (relevant for
            // timing attacks on hash validation).
            (Value::Secret(a), Value::Secret(b)) => a == b,
            // Equality by `Arc` identity — parallel to WsConn.
            (Value::DbConn(a), Value::DbConn(b)) => Arc::ptr_eq(a, b),
            // Same criterion for QueryBuilder.
            (Value::QueryBuilder(a), Value::QueryBuilder(b)) => Arc::ptr_eq(a, b),
            // Functions are not compared by value — always unequal.
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 in tests is a generic Float, not PI.
mod tests {
    use super::*;

    #[test]
    fn display_int_without_decimals() {
        assert_eq!(Value::Int(42).to_string(), "42");
        assert_eq!(Value::Int(-7).to_string(), "-7");
    }

    #[test]
    fn display_float_integer_carries_dot_zero() {
        assert_eq!(Value::Float(3.0).to_string(), "3.0");
        assert_eq!(Value::Float(-0.0).to_string(), "-0.0");
    }

    #[test]
    fn display_float_with_decimals_shows_normally() {
        assert_eq!(Value::Float(3.14).to_string(), "3.14");
    }

    #[test]
    fn display_str_without_quotes() {
        // print("hola") should show `hola`, not `"hola"`.
        assert_eq!(Value::Str("hola".into()).to_string(), "hola");
    }

    #[test]
    fn display_bool_lowercase() {
        assert_eq!(Value::Bool(true).to_string(), "true");
        assert_eq!(Value::Bool(false).to_string(), "false");
    }

    #[test]
    fn display_null() {
        assert_eq!(Value::Null.to_string(), "null");
    }

    #[test]
    fn type_name_returns_the_type_name() {
        assert_eq!(Value::Int(0).type_name(), "Int");
        assert_eq!(Value::Float(0.0).type_name(), "Float");
        assert_eq!(Value::Str("".into()).type_name(), "Str");
        assert_eq!(Value::Bool(false).type_name(), "Bool");
        assert_eq!(Value::Null.type_name(), "Null");
        assert_eq!(Value::Bytes(vec![]).type_name(), "Bytes");
    }

    // ---- Mini-batch Bytes ----

    #[test]
    fn bytes_display_ascii_printable() {
        assert_eq!(Value::Bytes(b"hola".to_vec()).to_string(), "b\"hola\"");
    }

    #[test]
    fn bytes_display_with_hex_escapes() {
        assert_eq!(
            Value::Bytes(vec![0x00, 0x01, 0xff]).to_string(),
            "b\"\\x00\\x01\\xff\""
        );
    }

    #[test]
    fn bytes_display_with_common_escapes() {
        assert_eq!(
            Value::Bytes(b"a\nb\tc\\d\"e".to_vec()).to_string(),
            "b\"a\\nb\\tc\\\\d\\\"e\""
        );
    }

    #[test]
    fn bytes_equality_byte_by_byte() {
        assert_eq!(Value::Bytes(vec![1, 2, 3]), Value::Bytes(vec![1, 2, 3]));
        assert_ne!(Value::Bytes(vec![1, 2, 3]), Value::Bytes(vec![1, 2, 4]));
        assert_ne!(Value::Bytes(vec![1, 2]), Value::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn bytes_distinct_from_str_even_with_same_content() {
        // Bytes("hola") and Str("hola") are distinct types —
        // PartialEq returns false (parallel to Int vs Str).
        assert_ne!(Value::Bytes(b"hola".to_vec()), Value::Str("hola".into()));
    }

    #[test]
    fn equality_int_and_float_coerces() {
        // In Fitz, `1 == 1.0` is true. Reflects the Int↔Float
        // promotion.
        assert_eq!(Value::Int(1), Value::Float(1.0));
        assert_eq!(Value::Float(2.0), Value::Int(2));
    }

    #[test]
    fn equality_between_different_types_is_false() {
        assert_ne!(Value::Int(1), Value::Str("1".into()));
        assert_ne!(Value::Bool(true), Value::Int(1));
        assert_ne!(Value::Null, Value::Bool(false));
    }

    #[test]
    fn equality_null_with_itself() {
        assert_eq!(Value::Null, Value::Null);
    }

    #[test]
    fn equality_strings() {
        assert_eq!(Value::Str("hola".into()), Value::Str("hola".into()));
        assert_ne!(Value::Str("hola".into()), Value::Str("chau".into()));
    }

    // -----------------------------------------------------------------------
    // Tests — List, Map, Range (Phase 3, step 1)
    // -----------------------------------------------------------------------

    #[test]
    fn display_list_empty() {
        assert_eq!(Value::new_list(vec![]).to_string(), "[]");
    }

    #[test]
    fn display_list_with_ints() {
        let v = Value::new_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        assert_eq!(v.to_string(), "[1, 2, 3]");
    }

    #[test]
    fn display_list_strings_carry_quotes_inside() {
        // Bare strings carry no quotes (print), but inside a list
        // they carry quotes so `1` and `"1"` can be told apart.
        let v = Value::new_list(vec![
            Value::Int(1),
            Value::Str("hola".into()),
            Value::Bool(true),
        ]);
        assert_eq!(v.to_string(), "[1, \"hola\", true]");
    }

    #[test]
    fn display_list_nested() {
        let inner = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let outer = Value::new_list(vec![inner.clone(), inner]);
        assert_eq!(outer.to_string(), "[[1, 2], [1, 2]]");
    }

    #[test]
    fn display_map_empty() {
        assert_eq!(Value::new_map(vec![]).to_string(), "{}");
    }

    #[test]
    fn display_map_preserves_order_and_quotes_in_strings() {
        let m = Value::new_map(vec![
            (Value::Str("a".into()), Value::Int(1)),
            (Value::Str("b".into()), Value::Int(2)),
        ]);
        assert_eq!(m.to_string(), "{\"a\": 1, \"b\": 2}");
    }

    #[test]
    fn display_range_simple() {
        assert_eq!(Value::Range { start: 0, end: 10 }.to_string(), "0..10");
    }

    #[test]
    fn display_range_negative() {
        assert_eq!(Value::Range { start: -5, end: 5 }.to_string(), "-5..5");
    }

    #[test]
    fn type_name_of_list_map_range() {
        assert_eq!(Value::new_list(vec![]).type_name(), "List");
        assert_eq!(Value::new_map(vec![]).type_name(), "Map");
        assert_eq!(Value::Range { start: 0, end: 1 }.type_name(), "Range");
    }

    #[test]
    fn equality_list_structural() {
        let a = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let c = Value::new_list(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn equality_list_coerces_int_float_inside() {
        // [1, 2] == [1.0, 2.0] — the Int↔Float coercion applies
        // inside lists.
        let a = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::new_list(vec![Value::Float(1.0), Value::Float(2.0)]);
        assert_eq!(a, b);
    }

    #[test]
    fn equality_map_structural() {
        let a = Value::new_map(vec![(Value::Str("k".into()), Value::Int(1))]);
        let b = Value::new_map(vec![(Value::Str("k".into()), Value::Int(1))]);
        let c = Value::new_map(vec![(Value::Str("k".into()), Value::Int(2))]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn equality_map_order_sensitive() {
        // Since we use Vec<(K,V)>, order matters for equality. This
        // is consistent with how we print them (preserving order).
        let a = Value::new_map(vec![
            (Value::Str("a".into()), Value::Int(1)),
            (Value::Str("b".into()), Value::Int(2)),
        ]);
        let b = Value::new_map(vec![
            (Value::Str("b".into()), Value::Int(2)),
            (Value::Str("a".into()), Value::Int(1)),
        ]);
        assert_ne!(a, b);
    }

    #[test]
    fn equality_range() {
        assert_eq!(
            Value::Range { start: 0, end: 10 },
            Value::Range { start: 0, end: 10 },
        );
        assert_ne!(
            Value::Range { start: 0, end: 10 },
            Value::Range { start: 0, end: 11 },
        );
    }

    #[test]
    fn equality_between_different_types_is_false_for_new_ones() {
        // Sanity: list != map, list != range, etc.
        assert_ne!(Value::new_list(vec![]), Value::new_map(vec![]));
        assert_ne!(
            Value::new_list(vec![Value::Int(0), Value::Int(1)]),
            Value::Range { start: 0, end: 2 },
        );
    }

    // -----------------------------------------------------------------------
    // Tests — Instance (Phase 3, step 2: instantiable custom types)
    // -----------------------------------------------------------------------

    #[test]
    fn type_name_of_instance() {
        let i = Value::new_instance("User".into(), vec![]);
        assert_eq!(i.type_name(), "Instance");
    }

    #[test]
    fn display_instance_empty_shows_braces_together() {
        let i = Value::new_instance("Empty".into(), vec![]);
        assert_eq!(i.to_string(), "Empty {}");
    }

    #[test]
    fn display_instance_with_fields() {
        let i = Value::new_instance(
            "User".into(),
            vec![
                ("id".into(), Value::Int(1)),
                ("name".into(), Value::Str("Fitz".into())),
            ],
        );
        // Strings carry quotes inside, same as List/Map.
        assert_eq!(i.to_string(), "User { id: 1, name: \"Fitz\" }");
    }

    #[test]
    fn equality_instance_structural() {
        let a = Value::new_instance(
            "Point".into(),
            vec![("x".into(), Value::Int(1)), ("y".into(), Value::Int(2))],
        );
        let b = Value::new_instance(
            "Point".into(),
            vec![("x".into(), Value::Int(1)), ("y".into(), Value::Int(2))],
        );
        let c = Value::new_instance(
            "Point".into(),
            vec![("x".into(), Value::Int(1)), ("y".into(), Value::Int(3))],
        );
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn equality_instance_different_type_name_is_false() {
        // Same field shape, different type → not equal.
        let a = Value::new_instance("User".into(), vec![("id".into(), Value::Int(1))]);
        let b = Value::new_instance("Admin".into(), vec![("id".into(), Value::Int(1))]);
        assert_ne!(a, b);
    }

    // -----------------------------------------------------------------------
    // Tests — Result (Phase 3, step 3: Result + Ok/Err + `?`)
    // -----------------------------------------------------------------------

    fn ok(v: Value) -> Value {
        Value::Result(ResultVariant::Ok(Box::new(v)))
    }

    fn err(v: Value) -> Value {
        Value::Result(ResultVariant::Err(Box::new(v)))
    }

    #[test]
    fn type_name_of_result() {
        assert_eq!(ok(Value::Int(1)).type_name(), "Result");
        assert_eq!(err(Value::Str("boom".into())).type_name(), "Result");
    }

    #[test]
    fn display_ok_wraps_inner() {
        // Same criterion as List/Map: strings inside carry quotes.
        assert_eq!(ok(Value::Int(42)).to_string(), "Ok(42)");
        assert_eq!(ok(Value::Str("hola".into())).to_string(), "Ok(\"hola\")");
    }

    #[test]
    fn display_err_wraps_inner() {
        assert_eq!(err(Value::Str("boom".into())).to_string(), "Err(\"boom\")");
        assert_eq!(err(Value::Int(404)).to_string(), "Err(404)");
    }

    #[test]
    fn display_result_nested() {
        // Ok(Err("x")) — unlikely but structurally legal.
        let inner = err(Value::Str("x".into()));
        assert_eq!(ok(inner).to_string(), "Ok(Err(\"x\"))");
    }

    #[test]
    fn equality_ok_structural() {
        assert_eq!(ok(Value::Int(1)), ok(Value::Int(1)));
        assert_ne!(ok(Value::Int(1)), ok(Value::Int(2)));
    }

    #[test]
    fn equality_err_structural() {
        assert_eq!(err(Value::Str("x".into())), err(Value::Str("x".into())));
        assert_ne!(err(Value::Str("x".into())), err(Value::Str("y".into())));
    }

    #[test]
    fn equality_ok_vs_err_is_false() {
        assert_ne!(ok(Value::Int(1)), err(Value::Int(1)));
    }

    #[test]
    fn equality_result_coerces_int_float_inside() {
        // The Int↔Float coercion applies recursively inside the inner.
        assert_eq!(ok(Value::Int(1)), ok(Value::Float(1.0)));
    }

    #[test]
    fn equality_result_with_other_types_is_false() {
        assert_ne!(ok(Value::Int(1)), Value::Int(1));
        assert_ne!(err(Value::Str("x".into())), Value::Str("x".into()));
    }

    // -----------------------------------------------------------------------
    // Tests — Module (Phase 3, step 5: modules / import)
    // -----------------------------------------------------------------------

    use crate::env::Environment;

    #[test]
    fn type_name_of_module() {
        let env = Environment::new();
        let m = Value::Module {
            name: "utils".into(),
            env,
        };
        assert_eq!(m.type_name(), "Module");
    }

    #[test]
    fn display_module_shows_name() {
        let env = Environment::new();
        let m = Value::Module {
            name: "utils".into(),
            env,
        };
        assert_eq!(m.to_string(), "<module utils>");
    }

    #[test]
    fn equality_module_is_by_env_identity() {
        // Same env → equal. Models "the same file imported twice
        // is the same module".
        let env = Environment::new();
        let m1 = Value::Module {
            name: "utils".into(),
            env: env.clone(),
        };
        let m2 = Value::Module {
            name: "utils".into(),
            env: env.clone(),
        };
        assert_eq!(m1, m2);

        // Different env → unequal even if the name matches.
        let other_env = Environment::new();
        let m3 = Value::Module {
            name: "utils".into(),
            env: other_env,
        };
        assert_ne!(m1, m3);
    }
}
