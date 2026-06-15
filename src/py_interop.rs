// py_interop.rs — Phase 8.1.2: CPython interop via PyO3
//
// Single entry point to the embedded Python runtime from the evaluator.
// The rest of the compiler speaks Fitz `Value` and `FitzError`; this
// module crosses the boundary: take the GIL, call PyO3 APIs,
// translate Python exceptions to `FitzError`, wrap the resulting
// `Py<PyAny>` in `Value::PyObject`.
//
// Only exists when compiled with `--features python`. The default
// `fitz` binary (without the feature) does not even link libpython.
//
// GIL policy: one `Python::with_gil` per public operation of
// this module. That means: the GIL is acquired and released on each
// `import_module`. For typical cases (one `from python import math`
// per program) the cost is negligible. When Phase 8.6 lands
// (async + tokio + asyncio bridge), we revisit.
//
// Error policy: any `PyErr` is translated to `FitzError` with
// the message "<ClassName>: <message>". Automatic conversion to `Result<T>`
// lands in Phase 8.3 — in 8.1 the error aborts the program
// just like an interpreter panic.

use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};
use std::sync::{Once, OnceLock};

use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::value::{FitzFuture, PyObjectHandle, ResultVariant, Value};

/// Cleanup-Residual+ mini-batch (2026-05-22) — initializes the
/// Python interpreter exactly once (idempotent). Called from
/// `import_module` before the first `Python::attach`. Replaces PyO3's
/// `auto-initialize` feature, which was incompatible with
/// `abi3-py310` (auto-initialize linked against the builder-specific
/// libpython, losing abi3 portability).
///
/// The `Once` guarantees that `prepare_freethreaded_python()` runs
/// exactly once per process, regardless of how many imports the
/// Fitz program makes.
fn ensure_python_initialized() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        Python::initialize();
    });
}

/// Imports a Python module given its "dotted" path (`"math"`,
/// `"sqlalchemy.orm"`, etc.) and returns it wrapped in `Value::PyObject`.
///
/// Internally delegates to `Bound::<PyModule>::import(py, dotted)` —
/// equivalent to a script's `import <dotted>` with the standard
/// `sys.path` of the embedded interpreter. Venv policy (8.1): the
/// user activates their venv before `fitz run`; Python detects it via
/// `VIRTUAL_ENV` at boot. Without a venv, packages are looked up in the
/// global site-packages of the base interpreter that was linked against.
///
/// Errors: if the module does not exist (`ModuleNotFoundError`), if the path
/// is invalid, or if Python blows up while initializing, we return a
/// `FitzError` with the caller's line/column (injected above)
/// and message "<ClassName>: <message>".
pub fn import_module(dotted: &str) -> FitzResult<Value> {
    // Cleanup-Residual+ mini-batch (2026-05-22) — without the
    // `auto-initialize` feature, the first `Python::attach` requires
    // the Python interpreter to be initialized. `prepare_freethreaded_python()`
    // is idempotent, we can call it on every import (common case).
    // Together with `abi3-py310`, this produces a truly portable
    // binary: a single binary runs against Python 3.10/3.11/3.12/
    // 3.13/3.14 without reconfiguration.
    ensure_python_initialized();
    // `Python::attach` replaced `Python::with_gil` in pyo3 0.23+;
    // the API is identical in use (closure receives `Python<'_>`). On
    // subsequent runs it is a fetch + lock.
    Python::attach(|py| match py.import(dotted) {
        Ok(module) => {
            // `module: Bound<'py, PyModule>`. We convert it to
            // `Py<PyAny>` 'static to store it in `Value::PyObject`
            // without tying ourselves to the GIL token lifetime. `.into_any()`
            // downcasts `PyModule` to `PyAny` (subtype); `.unbind()`
            // releases the `Bound` lifetime and returns `Py<PyAny>`.
            let py_any: Py<PyAny> = module.into_any().unbind();
            Ok(Value::PyObject(PyObjectHandle::new(py_any)))
        }
        Err(err) => Err(py_err_to_fitz(py, err)),
    })
}

/// Phase 8.1.3 — attribute access on a Python object with
/// primitive auto-coercion. Implements the mechanics of `math.pi`
/// (Float constant), `math.sqrt` (opaque function → `Value::PyObject`),
/// and by extension any `obj.attr` where `obj: Value::PyObject`.
///
/// 8.1 coercion policy:
///   - `None` → `Value::Null`
///   - `bool` → `Value::Bool` (checked **before** int — in Python
///     `bool ⊂ int`).
///   - `int` → `Value::Int` if it fits in `i64`. If it overflows, explicit
///     error (minor debt: bignum support when demand appears).
///   - `float` → `Value::Float`.
///   - `str` → `Value::Str`.
///   - any other type (function, class, instance, list, dict,
///     submodule, etc.) → opaque `Value::PyObject`. Marshaling for
///     specific `list/dict` lands in 8.2.
pub fn get_attr(handle: &PyObjectHandle, name: &str) -> FitzResult<Value> {
    Python::attach(|py| {
        let bound = handle.0.bind(py);
        match bound.getattr(name) {
            Ok(attr) => py_to_value(py, &attr),
            Err(err) => Err(py_err_to_fitz(py, err)),
        }
    })
}

/// Phase 8.3 — invokes a callable PyObject (function, method, class)
/// with args already evaluated to Fitz `Value`. **Every Python call from
/// Fitz is automatically wrapped in `Result<T>`**: success produces
/// `Value::Result(Ok(v))` with the coerced value inside; any
/// failure from the Python path (Python exception, marshaling of args
/// impossible, etc.) produces `Value::Result(Err(Str("<ClassName>:
/// <message>")))` without aborting the program.
///
/// This convention preserves Fitz's error model (no
/// exceptions): the user is forced to handle the failure with
/// `match` or the `?` operator, just like native `find`/`get`/`json.loads`.
/// Python exceptions no longer leak as opaque panics.
///
/// The `FitzResult<Value>` signature path is kept only for
/// catastrophic errors of Fitz's own runtime (which have not
/// appeared in practice); in the normal flow we always return
/// `Ok(Value::Result(...))`.
///
/// **Errors covered by `Result::Err`**:
///   - Python exception raised by the callable (ValueError,
///     TypeError, etc.) — including KeyboardInterrupt/SystemExit
///     per roadmap (there is no way to kill the Fitz runtime from
///     a Python exception).
///   - Failed args marshaling (Fitz type not representable in
///     Python — Range/Function/Type/Module/etc. with informative
///     breadcrumb via `path`).
///   - Failed return marshaling (rare: Python int > i64).
///   - Args tuple construction (defensive — should be
///     infallible in practice).
pub fn call(handle: &PyObjectHandle, args: &[Value]) -> FitzResult<Value> {
    Python::attach(|py| {
        let bound = handle.0.bind(py);
        // Convert each Fitz arg → PyObject. Marshaling errors
        // (Range/Function/etc., or unhashable keys) are wrapped in
        // `Result::Err` with the FitzError's message. This unifies
        // the whole path: the user sees ONE single point of error
        // (`?` or `match`) regardless of what failed.
        let py_args_result: FitzResult<Vec<Py<PyAny>>> = args
            .iter()
            .enumerate()
            .map(|(i, v)| value_to_py(py, v, &format!("arg{}", i)))
            .collect();
        let py_args = match py_args_result {
            Ok(v) => v,
            Err(e) => return Ok(err_value_from_message(e.message)),
        };
        // `call1` takes a positional tuple without kwargs. This is the typical
        // case of `math.sqrt(16.0)` and `os.path.join("a", "b")`. Kwargs is
        // minor debt for when real demand appears.
        let args_tuple = match pyo3::types::PyTuple::new(py, py_args) {
            Ok(t) => t,
            Err(e) => return Ok(err_value_from_message(py_err_to_fitz(py, e).message)),
        };
        match bound.call1(args_tuple) {
            Ok(ret) => {
                // Phase 8.6: if the return is a Python coroutine
                // (typical case when calling an `async def`),
                // we convert it to `Value::Future` instead of opaque
                // `PyObject`. This unblocks `py_async_fn().await` from
                // Fitz without manual glue — the existing postfix
                // `.await` (Phase 6) unwraps the `Value::Future` and
                // returns the coerced value.
                if is_coroutine(py, &ret) {
                    return match py_coro_to_fitz_future(&ret) {
                        Ok(fut) => Ok(Value::Result(ResultVariant::Ok(Box::new(
                            Value::new_future(fut),
                        )))),
                        Err(e) => Ok(err_value_from_message(py_err_to_fitz(py, e).message)),
                    };
                }
                match py_to_value(py, &ret) {
                    Ok(v) => Ok(Value::Result(ResultVariant::Ok(Box::new(v)))),
                    Err(e) => Ok(err_value_from_message(e.message)),
                }
            }
            Err(err) => Ok(err_value_from_message(py_err_to_fitz(py, err).message)),
        }
    })
}

/// Phase 8.6 — checks whether a Python object is awaitable (a coroutine
/// `async def`, a Task, or any object with `__await__`). Uses
/// `inspect.isawaitable`, which is the canonical form in Python stdlib.
///
/// We hold the GIL implicitly (the caller already has it). We return
/// `false` if introspection fails — defensive: better treat the
/// object as non-awaitable than produce an incorrect wrap.
fn is_coroutine<'py>(py: Python<'py>, obj: &Bound<'py, PyAny>) -> bool {
    let inspect = match py.import("inspect") {
        Ok(m) => m,
        Err(_) => return false,
    };
    inspect
        .call_method1("isawaitable", (obj,))
        .and_then(|v| v.extract::<bool>())
        .unwrap_or(false)
}

// ============================================================================
// Phase 8.6-bis — asyncio bridge with persistent event loop.
//
// Design: a single dedicated Python thread keeps an event loop alive
// and processes requests serially from a Rust mpsc channel. Each
// `.await` from Fitz builds a request with the coroutine's `Py<PyAny>`
// + a `tokio::sync::oneshot::Sender`, sends it to the Python
// thread, and `.await`s the receiver.
//
// **Benefits over the "baseline blocking" approach of 8.6.1**:
//   - **Zero per-call overhead**: the loop is created ONCE. There is no
//     `new_event_loop()` + `close()` for each `.await`.
//   - **Does not consume tokio's blocking pool**: only one dedicated
//     Python thread. Hundreds of pending Fitz awaits queue up but do not
//     saturate threads.
//   - **asyncio state reuse**: DB pools, HTTP clients and other
//     loop-cached primitives survive between calls.
//
// **MVP limitation**: requests are serialized in the loop thread
// (one at a time with `run_until_complete`). Same as happened
// with 8.6.1 because of the GIL; the approach can be iterated to
// real concurrency if demand appears (sub-loops via gather,
// multi-process, etc.).
//
// **Why not `run_coroutine_threadsafe`**: the
// "loop.run_forever on a thread + threadsafe schedule from other
// threads" approach clashes with GIL coordination in PyO3 0.28 when
// `pyo3-asyncio` is not used (it requires control of the tokio
// runtime, incompatible with Fitz's setup). The loop thread needs the
// GIL to react to the newly scheduled task, but the thread that
// schedules it holds the GIL during the call to
// `run_coroutine_threadsafe`. Design discarded after a real attempt.
// ============================================================================

/// Request to the loop thread: coroutine to execute + sender to
/// return the result to the caller.
struct AsyncioRequest {
    coro: Py<PyAny>,
    response: tokio::sync::oneshot::Sender<FitzResult<Value>>,
}

// Py<PyAny> is Send-safe in PyO3 (access is always via GIL). The
// whole struct is Send by composition. We mark it explicitly
// because the mpsc channel requires it.
unsafe impl Send for AsyncioRequest {}

struct AsyncioBridge {
    tx: std::sync::mpsc::Sender<AsyncioRequest>,
}

static ASYNCIO_BRIDGE: OnceLock<AsyncioBridge> = OnceLock::new();

/// Ensures the loop thread is running and returns the
/// sender to schedule work. Idempotent: the thread is
/// initialized only the first time.
fn ensure_asyncio_bridge() -> &'static AsyncioBridge {
    ASYNCIO_BRIDGE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<AsyncioRequest>();
        std::thread::Builder::new()
            .name("fitz-asyncio".into())
            .spawn(move || asyncio_worker_loop(rx))
            .expect("no se pudo crear el thread fitz-asyncio");
        AsyncioBridge { tx }
    })
}

/// Main loop of the bridge thread. Initializes the event loop ONCE
/// (under `Python::attach`), then processes requests from the channel:
/// the `recv()` happens OUTSIDE attach so we do not hold the GIL
/// during the wait (other threads building the next coroutine
/// need the GIL to do Fitz → Python marshaling).
fn asyncio_worker_loop(rx: std::sync::mpsc::Receiver<AsyncioRequest>) {
    // Initialize the loop. We keep it as `Py<PyAny>` (Send-safe,
    // unbound from `Python<'_>`) to reuse it on each iteration.
    let event_loop_init: Option<Py<PyAny>> = Python::attach(|py| {
        let asyncio = py.import("asyncio").ok()?;
        let loop_obj = asyncio.call_method0("new_event_loop").ok()?;
        asyncio.call_method1("set_event_loop", (&loop_obj,)).ok()?;
        Some(loop_obj.clone().unbind())
    });
    let event_loop = match event_loop_init {
        Some(o) => o,
        None => return,
    };

    // Process requests. `rx.recv()` is OUTSIDE `Python::attach`
    // → does not hold the GIL during the wait. When a request
    // arrives, we re-attach to run it.
    while let Ok(req) = rx.recv() {
        let AsyncioRequest { coro, response } = req;
        let result: FitzResult<Value> = Python::attach(|py| {
            let bound_loop = event_loop.bind(py);
            let bound_coro = coro.bind(py);
            match bound_loop.call_method1("run_until_complete", (bound_coro,)) {
                Ok(value) => py_to_value(py, &value),
                Err(e) => Err(FitzError::new(
                    ErrorKind::UndefinedVariable("PyError".to_string()),
                    0,
                    0,
                    py_err_to_fitz(py, e).message,
                )),
            }
        });
        // The receiver may be dropped if the FitzFuture was
        // canceled before completing; we ignore the send_err.
        let _ = response.send(result);
    }
    // Exit the while → close the loop. Best-effort.
    Python::attach(|py| {
        let bound = event_loop.bind(py);
        let _ = bound.call_method0("close");
    });
}

/// Phase 8.6 — converts a Python coroutine to a `FitzFuture` that
/// Fitz `Value::Future` can wrap. The user writes
/// `py_async_fn().await` and the bridge is invisible.
///
/// **8.6-bis implementation ("persistent event loop"; see module doc
/// above)**: enqueues the coroutine on the dedicated Python thread
/// via mpsc and `.await`s a `tokio::sync::oneshot::
/// Receiver`. The FitzFuture is truly asynchronous — it does not occupy
/// a tokio blocking thread, only a slot in the asyncio
/// thread's queue.
///
/// **Thread lifetime**: the asyncio thread stays alive until
/// the process exits. Since it is not a daemon, the tokio runtime waits
/// for all threads to terminate — but the asyncio thread blocks on
/// `rx.recv()` indefinitely. Pragmatic solution: the caller
/// drops the `Sender` when `main` exits, `rx.recv()`
/// returns `Err`, and the thread leaves the while. **In practice,
/// we rely on `process::exit` to clean up everything** — the global
/// channel's `Drop` does not run. Not ideal, but the same was
/// already true with 8.6.1 (blocking workers were not cleaned up either).
fn py_coro_to_fitz_future(coro: &Bound<'_, PyAny>) -> PyResult<FitzFuture> {
    let bridge = ensure_asyncio_bridge();
    let coro_owned: Py<PyAny> = coro.clone().unbind();
    let (tx, rx) = tokio::sync::oneshot::channel::<FitzResult<Value>>();
    let request = AsyncioRequest {
        coro: coro_owned,
        response: tx,
    };
    if bridge.tx.send(request).is_err() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(
            "Fitz asyncio thread terminated unexpectedly",
        ));
    }
    let fitz_future: FitzFuture = Box::pin(async move {
        match rx.await {
            Ok(result) => result,
            Err(_) => Err(FitzError::new(
                ErrorKind::UndefinedVariable("RuntimeError".to_string()),
                0,
                0,
                "asyncio thread did not respond to the coroutine (loop closed?)".to_string(),
            )),
        }
    });
    Ok(fitz_future)
}

/// Helper to build the `Value::Result(Err(Str(msg)))` that wraps
/// errors from a Python call. The `msg` already comes with format
/// `"<ClassName>: <message>"` from `py_err_to_fitz` (Python
/// exceptions) or from the `FitzError.message` of marshaling failures.
fn err_value_from_message(msg: String) -> Value {
    Value::Result(ResultVariant::Err(Box::new(Value::Str(msg))))
}

/// Converts a Fitz `Value` to a `Py<PyAny>` to pass to Python.
/// 8.2 policy (primitives + compounds):
///
///   - `Int`   → `int`
///   - `Float` → `float`
///   - `Str`   → `str`
///   - `Bool`  → `bool`
///   - `Null`  → `None`
///   - `PyObject(h)` → passthrough with `clone_ref` (refcount bump);
///     a Fitz→Python→Fitz round-trip preserves identity.
///   - `List<T>` → `list` (eager element-by-element copy; each
///     element is marshaled recursively).
///   - `Map<K, V>` → `dict` (eager copy; keys must be
///     Python-hashable primitives — Int/Float/Str/Bool/Null —
///     because `dict` requires `__hash__`).
///   - `Instance { type_name, fields }` → `dict` with field names
///     as keys (nominal translation). The Fitz type is "forgotten" on
///     the Python side — recovering it on the round-trip requires a
///     destination annotation (8.4 debt).
///
/// Non-marshaleable types (`Range`, `Function`, `Future`, etc.) →
/// error with `path` pointing to the exact site inside the
/// structure (e.g. `arg0.users[2].email`).
///
/// "Eager copy" policy (cross-cutting decision #4 of the roadmap):
/// we do not share state between the two GCs. A Fitz `List<T>` that
/// goes to Python becomes an independent Python `list`; if
/// the Python `list` is mutated, the original Fitz `List<T>` is unaware.
fn value_to_py(py: Python<'_>, value: &Value, path: &str) -> FitzResult<Py<PyAny>> {
    use pyo3::types::{PyDict, PyList};
    use pyo3::IntoPyObject;
    match value {
        Value::Int(n) => Ok(n
            .into_pyobject(py)
            .map_err(|e| py_err_to_fitz(py, e.into()))?
            .into_any()
            .unbind()),
        Value::Float(f) => Ok(f
            .into_pyobject(py)
            .map_err(|e| py_err_to_fitz(py, e.into()))?
            .into_any()
            .unbind()),
        Value::Str(s) => Ok(s
            .into_pyobject(py)
            .map_err(|e| py_err_to_fitz(py, e.into()))?
            .into_any()
            .unbind()),
        Value::Bool(b) => {
            // `bool::into_pyobject` returns `Borrowed<'py, PyBool>` (not
            // a `Bound`), because True/False are shared singletons.
            // We convert it to `Py<PyAny>` via `.to_owned().into_any()`.
            let bound = b
                .into_pyobject(py)
                .map_err(|e| py_err_to_fitz(py, e.into()))?;
            Ok(bound.to_owned().into_any().unbind())
        }
        Value::Null => Ok(py.None()),
        // Passthrough: a `Value::PyObject` that crosses back to Python
        // is the same object. We clone the `Py<PyAny>` (refcount bump).
        Value::PyObject(h) => Ok(h.0.clone_ref(py)),

        // Phase 8.2 — compounds.
        Value::List(items) => {
            // We clone the Vec inside the lock so we do not hold the
            // MutexGuard alive during recursion: each element
            // potentially takes its own lock (nested Lists).
            let snapshot: Vec<Value> = items.lock().clone();
            let mut py_items: Vec<Py<PyAny>> = Vec::with_capacity(snapshot.len());
            for (i, v) in snapshot.iter().enumerate() {
                let elem_path = format!("{}[{}]", path, i);
                py_items.push(value_to_py(py, v, &elem_path)?);
            }
            let list = PyList::new(py, py_items).map_err(|e| py_err_to_fitz(py, e))?;
            Ok(list.into_any().unbind())
        }
        Value::Map(pairs) => {
            let snapshot: Vec<(Value, Value)> = pairs.lock().clone();
            let dict = PyDict::new(py);
            for (k, v) in snapshot.iter() {
                // Keys must be Python-hashable primitives.
                // Compound types (List/Map/Instance) are not hashable
                // and would break `dict.__setitem__`. We detect before
                // touching Python to give a specific message.
                let py_k = marshal_map_key(py, k, path)?;
                let v_path = format!("{}[{}]", path, fmt_map_key(k));
                let py_v = value_to_py(py, v, &v_path)?;
                dict.set_item(py_k, py_v)
                    .map_err(|e| py_err_to_fitz(py, e))?;
            }
            Ok(dict.into_any().unbind())
        }
        Value::Instance { type_name, fields } => {
            let snapshot: Vec<(String, Value)> = fields.lock().clone();
            let dict = PyDict::new(py);
            // Instance translates to a dict with field names as keys.
            // If `path` starts empty (top-level case, rare because
            // `call` always sets `arg<i>`), we use `type_name`
            // as prefix so the error is readable.
            let prefix: String = if path.is_empty() {
                type_name.clone()
            } else {
                path.to_string()
            };
            for (field_name, v) in snapshot.iter() {
                let field_path = format!("{}.{}", prefix, field_name);
                let py_v = value_to_py(py, v, &field_path)?;
                dict.set_item(field_name.as_str(), py_v)
                    .map_err(|e| py_err_to_fitz(py, e))?;
            }
            Ok(dict.into_any().unbind())
        }

        // Rest (Range, Function/Builtin, Type, Module, HttpResponse,
        // CorsConfig, Future, Result): not marshaleable. The
        // `Result` translates better on the HTTP handler side (Ok→200,
        // Err→500); crossing to Python as an object has no useful semantics.
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "primitivo, compuesto (List/Map/Instance) o PyObject".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!(
                "no se puede pasar un valor de tipo `{}` a Python (en `{}`); \
                 tipos no marshalleables: Range, Function, Type, Module, \
                 HttpResponse, CorsConfig, Future, Result",
                other.type_name(),
                path,
            ),
        )),
    }
}

/// Validates that a Fitz `Map` `key` is Python-hashable (Int/
/// Float/Str/Bool/Null) and marshals it. Compound types as a key →
/// clear error, because Python `dict` requires `__hash__` and List/Map/
/// Instance do not have it (just like Python `list`/`dict` are not
/// hashable).
fn marshal_map_key(py: Python<'_>, k: &Value, path: &str) -> FitzResult<Py<PyAny>> {
    match k {
        Value::Int(_) | Value::Float(_) | Value::Str(_) | Value::Bool(_) | Value::Null => {
            value_to_py(py, k, &format!("{}.<key>", path))
        }
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int/Float/Str/Bool/Null (hashable en Python)".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!(
                "key de Map no es hashable en Python (en `{}`): \
                 las keys de un dict Python deben ser primitivos \
                 (Int/Float/Str/Bool/Null), pero llegó un `{}`",
                path,
                other.type_name(),
            ),
        )),
    }
}

/// Formats a `Map` `key` to use it as a segment of the
/// path breadcrumb. `Str("a")` → `"\"a\""`, `Int(42)` → `42`,
/// rest → its `Display`. Cosmetic only — does not affect marshaling.
fn fmt_map_key(k: &Value) -> String {
    match k {
        Value::Str(s) => format!("\"{}\"", s),
        other => format!("{}", other),
    }
}

/// Converts a `Bound<'_, PyAny>` to Fitz `Value` applying the primitive
/// coercion policy. Reusable helper: consumed by `get_attr`
/// (8.1.3) and `call` (8.1.4) to process the return value.
///
/// Pre-condition: the caller already holds the GIL (the `py` parameter testifies).
fn py_to_value(py: Python<'_>, obj: &Bound<'_, PyAny>) -> FitzResult<Value> {
    if obj.is_none() {
        return Ok(Value::Null);
    }
    // bool BEFORE int: in Python `isinstance(True, int) == True`, so
    // an int check first would capture True/False as 1/0.
    if obj.is_instance_of::<PyBool>() {
        let b: bool = obj.extract().map_err(|e| py_err_to_fitz(py, e))?;
        return Ok(Value::Bool(b));
    }
    if obj.is_instance_of::<PyInt>() {
        // `extract::<i64>()` fails if the Python int exceeds the
        // i64 (`2^63`) range. In 8.1 we report a clear error citing the
        // limit; bignum support would be minor debt for 8.2+.
        return match obj.extract::<i64>() {
            Ok(n) => Ok(Value::Int(n)),
            Err(_) => Err(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int (i64)".into(),
                    found: "int Python fuera de rango i64".into(),
                },
                0,
                0,
                format!(
                    "el entero Python `{}` excede el rango de Int en Fitz (i64); \
                     bignum support llega en una fase posterior",
                    obj.str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|_| "<repr failed>".into()),
                ),
            )),
        };
    }
    if obj.is_instance_of::<PyFloat>() {
        let f: f64 = obj.extract().map_err(|e| py_err_to_fitz(py, e))?;
        return Ok(Value::Float(f));
    }
    if obj.is_instance_of::<PyString>() {
        let s: String = obj.extract().map_err(|e| py_err_to_fitz(py, e))?;
        return Ok(Value::Str(s));
    }

    // Phase 8.2.2 — compounds.
    //
    // Python `list` → `Value::List` (eager copy; each element
    // recursive via `py_to_value`). The result is semantically
    // `List<Any>` from the Fitz side because Python does not give us
    // a static type; Fitz-side annotations to refine to
    // a concrete `List<T>` land in 8.4.
    if obj.is_instance_of::<PyList>() {
        let list = obj
            .cast::<PyList>()
            .map_err(|e| py_err_to_fitz(py, e.into()))?;
        let mut items: Vec<Value> = Vec::with_capacity(list.len());
        for item in list.iter() {
            items.push(py_to_value(py, &item)?);
        }
        return Ok(Value::new_list(items));
    }

    // Python `dict` → `Value::Map`. CPython 3.7+ guarantees insertion
    // order for `dict`; preserving it gives us bit-exact parity with
    // `serde_json::preserve_order` that the rest of the project already uses.
    // Each (key, value) pair recurses via `py_to_value` — keys
    // are typically primitives but we allow any hashable
    // (Python validates; if the key is an opaque PyObject, it stays
    // that way). dict → Instance is not auto-coerced: that requires
    // a destination annotation on the Fitz side (8.4 debt).
    if obj.is_instance_of::<PyDict>() {
        let dict = obj
            .cast::<PyDict>()
            .map_err(|e| py_err_to_fitz(py, e.into()))?;
        let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(dict.len());
        for (k, v) in dict.iter() {
            let k_val = py_to_value(py, &k)?;
            let v_val = py_to_value(py, &v)?;
            pairs.push((k_val, v_val));
        }
        return Ok(Value::new_map(pairs));
    }

    // Fallback: remaining types (function, class, instance, submodule,
    // tuple, set, bytes, etc.) we wrap as opaque `Value::PyObject`
    // so the user can pass them to another Python function or do
    // field access. Tuples/sets/bytes could marshal to List/Map
    // in a future phase if real demand appears.
    let owned: Py<PyAny> = obj.clone().unbind();
    Ok(Value::PyObject(PyObjectHandle::new(owned)))
}

/// Converts a `PyErr` to a `FitzError` with message
/// "<ClassName>: <message>". The format matches the convention that
/// Phase 8.3 will stabilize when the `Result<T>` wraps land —
/// the `Err(...)` the user will receive will be the same string that
/// today appears in `FitzError`.
///
/// If introspection of the exception fails (rare case: PyO3's own
/// error querying the type), we return a generic message
/// without crashing the program.
fn py_err_to_fitz(py: Python<'_>, err: PyErr) -> FitzError {
    let class = err
        .get_type(py)
        .qualname()
        .ok()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "PyError".to_string());
    let value = err.value(py).to_string();
    let message = if value.is_empty() {
        class.clone()
    } else {
        format!("{}: {}", class, value)
    };
    FitzError::new(ErrorKind::UndefinedVariable(class), 0, 0, message)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importing_math_returns_pyobject() {
        let v = import_module("math").expect("math should import");
        assert!(matches!(v, Value::PyObject(_)));
    }

    #[test]
    fn importing_json_returns_pyobject() {
        let v = import_module("json").expect("json should import");
        assert!(matches!(v, Value::PyObject(_)));
    }

    #[test]
    fn importing_submodule_returns_pyobject() {
        // `os.path` always exists in any Python installation.
        let v = import_module("os.path").expect("os.path should import");
        assert!(matches!(v, Value::PyObject(_)));
    }

    #[test]
    fn modulo_inexistente_da_error_claro() {
        let err =
            import_module("este_modulo_no_existe_xyz_8_1_2").expect_err("module should not exist");
        // Esperamos algo como "ModuleNotFoundError: No module named 'este_modulo_no_existe_xyz_8_1_2'"
        assert!(
            err.message.contains("ModuleNotFoundError"),
            "message should cite ModuleNotFoundError, was: {}",
            err.message
        );
        assert!(
            err.message.contains("este_modulo_no_existe_xyz_8_1_2"),
            "message should cite the searched module name, was: {}",
            err.message
        );
    }

    #[test]
    fn two_imports_of_the_same_module_are_equal() {
        // Python caches imports (sys.modules), so two
        // `import math` return the same object. Our `PartialEq`
        // on `Value::PyObject` (Py::as_ptr) should reflect that.
        let a = import_module("math").unwrap();
        let b = import_module("math").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn imports_of_distinct_modules_are_not_equal() {
        let a = import_module("math").unwrap();
        let b = import_module("json").unwrap();
        assert_ne!(a, b);
    }

    // -------------------------------------------------------------------
    // 8.1.3 — get_attr + primitive auto-coercion
    // -------------------------------------------------------------------

    fn handle_of(v: Value) -> PyObjectHandle {
        match v {
            Value::PyObject(h) => h,
            other => panic!("se esperaba Value::PyObject, fue: {:?}", other),
        }
    }

    /// Post-8.3 helper: `call` now always wraps in `Result`.
    /// For happy-path tests, we unwrap `Ok(inner)` and return
    /// the `Value` inside. If `Err(...)` arrives, the test fails with the
    /// message (useful for debugging when something changes unexpectedly).
    fn ok_inner(v: Value) -> Value {
        match v {
            Value::Result(ResultVariant::Ok(inner)) => *inner,
            Value::Result(ResultVariant::Err(msg)) => {
                panic!("expected Ok(...), got Err({:?})", msg)
            }
            other => panic!("esperaba Value::Result, fue {:?}", other),
        }
    }

    /// Post-8.3 helper: extracts the message from `Err(Str(...))` that a
    /// failed Python call produces. If `Value` is not `Result::Err(Str)`,
    /// the test fails — useful to check the `"<Class>: <msg>"` format
    /// without assuming the shape.
    fn err_message(v: Value) -> String {
        match v {
            Value::Result(ResultVariant::Err(inner)) => match *inner {
                Value::Str(s) => s,
                other => panic!("Err should wrap Str, was {:?}", other),
            },
            Value::Result(ResultVariant::Ok(inner)) => {
                panic!("expected Err(...), got Ok({:?})", inner)
            }
            other => panic!("esperaba Value::Result, fue {:?}", other),
        }
    }

    #[test]
    fn get_attr_math_pi_es_float() {
        let math = handle_of(import_module("math").unwrap());
        let v = get_attr(&math, "pi").expect("math.pi should exist");
        match v {
            Value::Float(f) => {
                // Approximate comparison — math.pi's exact value
                // is pinned, but we use an epsilon just in case.
                assert!((f - std::f64::consts::PI).abs() < 1e-15, "got {}", f);
            }
            other => panic!("se esperaba Float, fue: {:?}", other),
        }
    }

    #[test]
    fn get_attr_math_sqrt_es_pyobject_opaco() {
        // `sqrt` is a Python function — not a primitive, must
        // wrap as an opaque PyObject for invocation in 8.1.4.
        let math = handle_of(import_module("math").unwrap());
        let v = get_attr(&math, "sqrt").expect("math.sqrt should exist");
        assert!(matches!(v, Value::PyObject(_)), "got: {:?}", v);
    }

    #[test]
    fn get_attr_missing_emits_attributeerror() {
        let math = handle_of(import_module("math").unwrap());
        let err = get_attr(&math, "no_existe_xyz_813").expect_err("attr should not exist");
        assert!(
            err.message.contains("AttributeError"),
            "message should cite AttributeError, was: {}",
            err.message,
        );
    }

    #[test]
    fn get_attr_str_es_str() {
        // `math.__name__` es la string "math".
        let math = handle_of(import_module("math").unwrap());
        let v = get_attr(&math, "__name__").expect("__name__ debe existir");
        assert_eq!(v, Value::Str("math".to_string()));
    }

    #[test]
    fn py_to_value_coerciona_bool_true() {
        // We build a PyBool without going through a module: import
        // `builtins` and read `True`.
        let builtins = handle_of(import_module("builtins").unwrap());
        let v = get_attr(&builtins, "True").unwrap();
        assert_eq!(v, Value::Bool(true));
    }

    #[test]
    fn py_to_value_coerciona_int_chico() {
        // `sys.maxsize` is a Python int — on 64-bit systems it is 2^63 - 1,
        // which fits exactly in i64. We use it to verify int → Int.
        let sys = handle_of(import_module("sys").unwrap());
        let v = get_attr(&sys, "maxsize").unwrap();
        assert_eq!(v, Value::Int(i64::MAX));
    }

    #[test]
    fn py_to_value_coerciona_none() {
        // `sys.__interactivehook__` is not always None; better an attribute
        // that is explicitly None. We use `ctypes`, which on some
        // systems may not exist; better to use a trick: `sys.flags` has
        // sub-attributes, but all are int. We use `inspect.Parameter.empty`
        // which is a sentinel — but that is not None. Take the direct
        // route: importing `dataclasses` and reading `MISSING` does not work
        // because it is an object. Better: we evaluate the `None` attribute of
        // `builtins`, which IS the Python None singleton.
        let builtins = handle_of(import_module("builtins").unwrap());
        let v = get_attr(&builtins, "None").unwrap();
        assert_eq!(v, Value::Null);
    }

    // -------------------------------------------------------------------
    // 8.1.4 — call + value_to_py
    // -------------------------------------------------------------------

    #[test]
    fn call_math_sqrt_16_da_float_4() {
        let math = handle_of(import_module("math").unwrap());
        let sqrt = handle_of(get_attr(&math, "sqrt").unwrap());
        let v = ok_inner(call(&sqrt, &[Value::Float(16.0)]).unwrap());
        assert_eq!(v, Value::Float(4.0));
    }

    #[test]
    fn call_math_floor_arg_float_da_int() {
        let math = handle_of(import_module("math").unwrap());
        let floor = handle_of(get_attr(&math, "floor").unwrap());
        let v = ok_inner(call(&floor, &[Value::Float(3.7)]).unwrap());
        assert_eq!(v, Value::Int(3));
    }

    #[test]
    fn call_str_upper_via_call_does_not_apply_is_method() {
        // `str.upper("hola")` is valid in Python (unbound method). We use
        // this case to verify that a Str argument marshals correctly.
        let builtins = handle_of(import_module("builtins").unwrap());
        let str_cls = handle_of(get_attr(&builtins, "str").unwrap());
        let upper = handle_of(get_attr(&str_cls, "upper").unwrap());
        let v = ok_inner(call(&upper, &[Value::Str("hola".into())]).unwrap());
        assert_eq!(v, Value::Str("HOLA".into()));
    }

    #[test]
    fn call_arg_int_coerciona_a_pyint() {
        // `abs(-7)` should give us `7`. We validate that Int → Python int.
        let builtins = handle_of(import_module("builtins").unwrap());
        let abs_fn = handle_of(get_attr(&builtins, "abs").unwrap());
        let v = ok_inner(call(&abs_fn, &[Value::Int(-7)]).unwrap());
        assert_eq!(v, Value::Int(7));
    }

    #[test]
    fn call_arg_bool_coerciona_a_pybool() {
        // `bool.__class__.__name__` of True → "bool". Better: `int(True)`
        // → 1. We confirm that Bool goes as Python bool, not as int.
        let builtins = handle_of(import_module("builtins").unwrap());
        let int_cls = handle_of(get_attr(&builtins, "int").unwrap());
        let v = ok_inner(call(&int_cls, &[Value::Bool(true)]).unwrap());
        assert_eq!(v, Value::Int(1));
    }

    #[test]
    fn call_arg_null_coerciona_a_none() {
        // `str(None)` gives "None". Verifies that Null → Python None.
        let builtins = handle_of(import_module("builtins").unwrap());
        let str_cls = handle_of(get_attr(&builtins, "str").unwrap());
        let v = ok_inner(call(&str_cls, &[Value::Null]).unwrap());
        assert_eq!(v, Value::Str("None".into()));
    }

    #[test]
    fn call_python_exception_wraps_in_result_err() {
        // 8.3: `math.sqrt(-1)` raises ValueError in Python. The call does
        // not abort — it returns `Value::Result(Err(Str("ValueError: ...")))`.
        // The user has to handle it with `match` or `?`.
        let math = handle_of(import_module("math").unwrap());
        let sqrt = handle_of(get_attr(&math, "sqrt").unwrap());
        let v = call(&sqrt, &[Value::Float(-1.0)]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.contains("ValueError"),
            "message should cite ValueError, was: {}",
            msg,
        );
    }

    #[test]
    fn call_non_marshallable_arg_wraps_in_result_err() {
        // 8.3: Range is not marshaleable. Instead of aborting with
        // FitzError, the error is wrapped in `Result::Err(Str(...))`
        // — uniformity: EVERY error from the call path looks like `Err` to
        // the user, whether Python exception or marshaling failure.
        let math = handle_of(import_module("math").unwrap());
        let sqrt = handle_of(get_attr(&math, "sqrt").unwrap());
        let v = call(&sqrt, &[Value::Range { start: 0, end: 10 }]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.contains("Range") && msg.contains("arg0"),
            "message should cite Range + arg0, was: {}",
            msg,
        );
    }

    #[test]
    fn call_pyobject_passthrough_preserves_identity() {
        // Pass a Value::PyObject as arg: should reach the Python callable
        // unchanged. We validate with `id(x) == id(x)` via `is`.
        let builtins = handle_of(import_module("builtins").unwrap());
        let id_fn = handle_of(get_attr(&builtins, "id").unwrap());
        let math = import_module("math").unwrap();
        // Same object passed twice: `id` must return the same Int.
        let id1 = ok_inner(call(&id_fn, std::slice::from_ref(&math)).unwrap());
        let id2 = ok_inner(call(&id_fn, &[math]).unwrap());
        assert_eq!(id1, id2);
    }

    // -------------------------------------------------------------------
    // 8.2.1 — value_to_py para List/Map/Instance (Fitz → Python)
    // -------------------------------------------------------------------

    #[test]
    fn list_of_ints_marshalls_to_python_list() {
        // `json.dumps([1, 2, 3])` → "[1, 2, 3]". The round-trip via
        // json validates that the Python list we produced has the
        // correct elements in order.
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let list = Value::new_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let v = ok_inner(call(&dumps, &[list]).unwrap());
        assert_eq!(v, Value::Str("[1, 2, 3]".into()));
    }

    #[test]
    fn list_vacia_se_marshalla_a_list_vacia() {
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let list = Value::new_list(vec![]);
        let v = ok_inner(call(&dumps, &[list]).unwrap());
        assert_eq!(v, Value::Str("[]".into()));
    }

    #[test]
    fn list_heterogenea_se_marshalla_ok() {
        // Python allows mixing in a list: `[1, "dos", true]`.
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let list = Value::new_list(vec![
            Value::Int(1),
            Value::Str("dos".into()),
            Value::Bool(true),
        ]);
        let v = ok_inner(call(&dumps, &[list]).unwrap());
        // JSON serializes true as `true`. Confirms that Fitz Bool
        // crosses as Python bool (not as int).
        assert_eq!(v, Value::Str("[1, \"dos\", true]".into()));
    }

    #[test]
    fn list_anidada_se_marshalla_recursivo() {
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let inner = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let outer = Value::new_list(vec![inner, Value::new_list(vec![Value::Int(3)])]);
        let v = ok_inner(call(&dumps, &[outer]).unwrap());
        assert_eq!(v, Value::Str("[[1, 2], [3]]".into()));
    }

    #[test]
    fn map_from_str_to_int_marshalls_to_dict() {
        // `json.dumps({"a": 1, "b": 2})` → '{"a": 1, "b": 2}'.
        // PyDict preserves insertion order (Python 3.7+), same
        // as `serde_json::preserve_order` that the rest of the
        // project already uses.
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let map = Value::new_map(vec![
            (Value::Str("a".into()), Value::Int(1)),
            (Value::Str("b".into()), Value::Int(2)),
        ]);
        let v = ok_inner(call(&dumps, &[map]).unwrap());
        assert_eq!(v, Value::Str("{\"a\": 1, \"b\": 2}".into()));
    }

    #[test]
    fn map_with_non_hashable_keys_is_error_with_path() {
        // 8.3: List as key → `Result::Err(Str)` with message citing
        // the path "arg0" and the "hashable" restriction.
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let bad_key = Value::new_list(vec![Value::Int(1)]);
        let map = Value::new_map(vec![(bad_key, Value::Int(42))]);
        let v = call(&dumps, &[map]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.contains("hashable") && msg.contains("List") && msg.contains("arg0"),
            "msg: {}",
            msg,
        );
    }

    #[test]
    fn instance_marshalls_to_dict_by_field_name() {
        // An `Instance` with type_name="User" and ordered fields
        // {id: 1, name: "x"} → `{"id": 1, "name": "x"}` after
        // `json.dumps`. Verifies that field order is preserved
        // (PyDict in CPython 3.7+).
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let user = Value::new_instance(
            "User".to_string(),
            vec![
                ("id".to_string(), Value::Int(1)),
                ("name".to_string(), Value::Str("x".into())),
            ],
        );
        let v = ok_inner(call(&dumps, &[user]).unwrap());
        assert_eq!(v, Value::Str("{\"id\": 1, \"name\": \"x\"}".into()));
    }

    #[test]
    fn list_of_instances_marshalls() {
        // Pre-canonical case from the roadmap: `List<User>` passed to a
        // Python function. The list marshals to list[dict].
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let users = Value::new_list(vec![
            Value::new_instance(
                "User".to_string(),
                vec![
                    ("id".to_string(), Value::Int(1)),
                    ("email".to_string(), Value::Str("a@x.com".into())),
                ],
            ),
            Value::new_instance(
                "User".to_string(),
                vec![
                    ("id".to_string(), Value::Int(2)),
                    ("email".to_string(), Value::Str("b@x.com".into())),
                ],
            ),
        ]);
        let v = ok_inner(call(&dumps, &[users]).unwrap());
        assert_eq!(
            v,
            Value::Str(
                "[{\"id\": 1, \"email\": \"a@x.com\"}, \
                  {\"id\": 2, \"email\": \"b@x.com\"}]"
                    .into()
            ),
        );
    }

    #[test]
    fn null_inside_list_marshalls_to_none() {
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let list = Value::new_list(vec![Value::Int(1), Value::Null, Value::Int(3)]);
        let v = ok_inner(call(&dumps, &[list]).unwrap());
        assert_eq!(v, Value::Str("[1, null, 3]".into()));
    }

    #[test]
    fn non_marshallable_element_in_list_is_error_with_path() {
        // 8.3: Range inside list → `Result::Err(Str)` with path
        // "arg0[1]" in the message.
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let list = Value::new_list(vec![
            Value::Int(1),
            Value::Range { start: 0, end: 10 },
            Value::Int(3),
        ]);
        let v = call(&dumps, &[list]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.contains("arg0[1]") && msg.contains("Range"),
            "msg: {}",
            msg,
        );
    }

    // -------------------------------------------------------------------
    // 8.2.2 — py_to_value para list/dict (Python → Fitz)
    // -------------------------------------------------------------------

    #[test]
    fn json_loads_of_array_returns_list() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(call(&loads, &[Value::Str("[1, 2, 3]".into())]).unwrap());
        assert_eq!(
            v,
            Value::new_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        );
    }

    #[test]
    fn json_loads_of_empty_array_returns_empty_list() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(call(&loads, &[Value::Str("[]".into())]).unwrap());
        assert_eq!(v, Value::new_list(vec![]));
    }

    #[test]
    fn json_loads_of_object_returns_map_with_insertion_order() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(call(&loads, &[Value::Str("{\"a\": 1, \"b\": 2}".into())]).unwrap());
        // Python 3.7+ guarantees insertion order for dict;
        // we verify it arrives in the order serialized in JSON.
        assert_eq!(
            v,
            Value::new_map(vec![
                (Value::Str("a".into()), Value::Int(1)),
                (Value::Str("b".into()), Value::Int(2)),
            ]),
        );
    }

    #[test]
    fn json_loads_of_heterogeneous_array() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(call(&loads, &[Value::Str("[1, \"dos\", true, null]".into())]).unwrap());
        assert_eq!(
            v,
            Value::new_list(vec![
                Value::Int(1),
                Value::Str("dos".into()),
                Value::Bool(true),
                Value::Null,
            ]),
        );
    }

    #[test]
    fn json_loads_of_nested_array() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(call(&loads, &[Value::Str("[[1, 2], [3, 4]]".into())]).unwrap());
        assert_eq!(
            v,
            Value::new_list(vec![
                Value::new_list(vec![Value::Int(1), Value::Int(2)]),
                Value::new_list(vec![Value::Int(3), Value::Int(4)]),
            ]),
        );
    }

    #[test]
    fn json_loads_of_dict_of_dict() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(
            call(
                &loads,
                &[Value::Str(
                    "{\"user\": {\"id\": 1, \"name\": \"x\"}}".into(),
                )],
            )
            .unwrap(),
        );
        assert_eq!(
            v,
            Value::new_map(vec![(
                Value::Str("user".into()),
                Value::new_map(vec![
                    (Value::Str("id".into()), Value::Int(1)),
                    (Value::Str("name".into()), Value::Str("x".into())),
                ]),
            )]),
        );
    }

    #[test]
    fn round_trip_list_via_json() {
        // dumps + loads → the list should come back the same.
        // 8.3: each call wraps in Ok, so we have to
        // unwrap `s` before passing it to the next call.
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let original = Value::new_list(vec![
            Value::Int(1),
            Value::Str("dos".into()),
            Value::Bool(false),
            Value::Null,
        ]);
        let s = ok_inner(call(&dumps, std::slice::from_ref(&original)).unwrap());
        let back = ok_inner(call(&loads, std::slice::from_ref(&s)).unwrap());
        assert_eq!(back, original);
    }

    #[test]
    fn pylist_direct_from_python_coerces_to_list() {
        // `list("abc")` in Python gives `['a', 'b', 'c']`. We cross a
        // live PyList, not a deserialized JSON, to validate that
        // dispatch over PyList works regardless of which API it comes from.
        let builtins = handle_of(import_module("builtins").unwrap());
        let list_cls = handle_of(get_attr(&builtins, "list").unwrap());
        let v = ok_inner(call(&list_cls, &[Value::Str("abc".into())]).unwrap());
        assert_eq!(
            v,
            Value::new_list(vec![
                Value::Str("a".into()),
                Value::Str("b".into()),
                Value::Str("c".into()),
            ]),
        );
    }

    #[test]
    fn pydict_direct_from_python_coerces_to_map() {
        // `dict(zip(["a", "b"], [1, 2]))` in Python gives `{"a": 1, "b": 2}`.
        // We validate a dict built at runtime, not a JSON loads.
        let builtins = handle_of(import_module("builtins").unwrap());
        let dict_cls = handle_of(get_attr(&builtins, "dict").unwrap());
        let zip_fn = handle_of(get_attr(&builtins, "zip").unwrap());
        let keys = Value::new_list(vec![Value::Str("a".into()), Value::Str("b".into())]);
        let vals = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let zipped = ok_inner(call(&zip_fn, &[keys, vals]).unwrap());
        let v = ok_inner(call(&dict_cls, &[zipped]).unwrap());
        assert_eq!(
            v,
            Value::new_map(vec![
                (Value::Str("a".into()), Value::Int(1)),
                (Value::Str("b".into()), Value::Int(2)),
            ]),
        );
    }

    #[test]
    fn non_marshallable_field_in_instance_is_error_with_path() {
        // 8.3: Range as a field value → `Result::Err(Str)` with
        // path "arg0.User.<field>" or similar.
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let user = Value::new_instance(
            "User".to_string(),
            vec![
                ("id".to_string(), Value::Int(1)),
                ("range".to_string(), Value::Range { start: 0, end: 5 }),
            ],
        );
        let v = call(&dumps, &[user]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.contains("Range")
                && msg.contains("range")  // the field is called "range"
                && msg.contains("arg0"),
            "msg: {}",
            msg,
        );
    }

    // -------------------------------------------------------------------
    // 8.3 — Automatic wrap in Result<T> + error format
    // -------------------------------------------------------------------

    #[test]
    fn call_success_returns_value_result_ok() {
        // Explicit shape validation: the `Value` returned by `call`
        // is always `Value::Result(Ok(...))` for success (not
        // `Value::Float` directly). Confirms the 8.3 invariant.
        let math = handle_of(import_module("math").unwrap());
        let sqrt = handle_of(get_attr(&math, "sqrt").unwrap());
        let v = call(&sqrt, &[Value::Float(16.0)]).unwrap();
        assert!(
            matches!(v, Value::Result(ResultVariant::Ok(_))),
            "esperaba Value::Result(Ok(...)), fue {:?}",
            v,
        );
    }

    #[test]
    fn call_jsonloads_malformed_is_err_with_jsondecodeerror() {
        // Textual criterion of the 8.3 roadmap:
        //   match parse("{ malformado") {
        //     Ok(m)  => print("ok: {m}"),
        //     Err(e) => print("error: {e}")
        //   }
        // → "error: JSONDecodeError: Expecting ..."
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = call(&loads, &[Value::Str("{ malformado".into())]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.contains("JSONDecodeError"),
            "Err message should cite JSONDecodeError, was: {}",
            msg,
        );
    }

    #[test]
    fn call_python_typeerror_wraps_does_not_abort() {
        // `int("no es un número")` raises ValueError. The call does not
        // abort — the Err contains the readable message.
        let builtins = handle_of(import_module("builtins").unwrap());
        let int_cls = handle_of(get_attr(&builtins, "int").unwrap());
        let v = call(&int_cls, &[Value::Str("no es un número".into())]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.starts_with("ValueError:"),
            "message should start with `ValueError:`, was: {}",
            msg,
        );
    }

    #[test]
    fn call_formato_err_es_classname_dos_puntos_message() {
        // The canonical `<ClassName>: <message>` format stays bit-exact
        // between 8.1 and 8.3 (only the wrapper changes:
        // FitzError → Value::Result(Err(Str))). Future tests that
        // depend on the exact format lean on this.
        let builtins = handle_of(import_module("builtins").unwrap());
        let int_cls = handle_of(get_attr(&builtins, "int").unwrap());
        let v = call(&int_cls, &[Value::Str("zz".into())]).unwrap();
        let msg = err_message(v);
        // Expected form: "ValueError: invalid literal for int() with base 10: 'zz'"
        let parts: Vec<&str> = msg.splitn(2, ": ").collect();
        assert_eq!(
            parts.len(),
            2,
            "esperaba `<ClassName>: <message>`, fue: {}",
            msg
        );
        assert_eq!(parts[0], "ValueError");
        assert!(!parts[1].is_empty(), "empty message body");
    }
}
