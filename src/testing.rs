// testing.rs — Phase 9.z.2 (built-in testing)
//
// Fitz tests runtime. Mechanics analogous to `HttpRegistry`:
//
//   1. During `eval`, when a `Stmt::FnDef` is seen with a `@test`
//      decorator, the evaluator pushes a `TestSpec` onto the
//      `TestRegistry` active via thread_local.
//   2. The `fitz test` sub-command (9.z.2.b) installs the registry via
//      `with_active_test_registry`, evaluates the file (which discovers
//      the tests + loads the modules), and on exit iterates the registry
//      invoking each handler. `fitz run` does NOT install a registry — the
//      `@test` decorators turn into silent no-ops (parallel to
//      Rust's `#[cfg(test)]`).
//
// There is no parallelism between tests: the runner runs them serially in the
// order of discovery. Parallelization is minor debt if pressure
// appears (inline tests + tests in modules tend to be fast; the real
// cost is in setup/teardown, which does not exist in the MVP).
//
// Sub-steps:
//   - 9.z.2.a (this file + branch in `process_decorator` +
//     assertion builtins): registry + decorator + builtins.
//   - 9.z.2.b: CLI `fitz test` + discovery (lib/bin + `tests/*.fitz`)
//     + runner with cargo-style output + filtering.
//   - 9.z.2.c: chapter in the guide + examples + formal close.

use std::cell::RefCell;

use crate::ast::Span;
use crate::value::Value;

// ---------------------------------------------------------------------------
// Base types
// ---------------------------------------------------------------------------

/// A fn marked with `@test`. The `handler` is a `Value::Function`
/// cloned from the interpreter's env — the `closure: EnvRef` keeps
/// alive the env of the module where it was declared, same as `RouteSpec`.
///
/// `is_async` is preserved from the FnDef so the runner can
/// choose between invoking sync or awaiting the resulting
/// `Value::Future`. `span` points to `@test` (not the fn) — useful for
/// "test X declared at line:Y failed because ..." reports.
///
/// `source_file` is `Some(path)` when the test was discovered
/// inside `with_test_source(path, ...)` (manifest-mode case
/// of the runner — tags each loaded file so the output can
/// prefix the test name with `<file>::<test>`). In
/// single-file mode or when the evaluator is invoked from a unit
/// test, `None`.
#[derive(Debug, Clone)]
pub struct TestSpec {
    pub name: String,
    pub handler: Value,
    pub is_async: bool,
    pub span: Span,
    pub source_file: Option<String>,
}

/// Collection of tests discovered during evaluation. Preserves
/// declaration order (insertion order) — the runner runs them in
/// that order, which typically matches the user's reading order.
#[derive(Debug, Clone, Default)]
pub struct TestRegistry {
    tests: Vec<TestSpec>,
}

impl TestRegistry {
    pub fn new() -> Self {
        Self { tests: Vec::new() }
    }

    pub fn push(&mut self, spec: TestSpec) {
        self.tests.push(spec);
    }

    /// Immutable view of registered tests, in discovery order.
    /// The runner iterates them with this.
    pub fn tests(&self) -> &[TestSpec] {
        &self.tests
    }

    pub fn len(&self) -> usize {
        self.tests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tests.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Thread-local registry — same pattern as `http::HTTP_REGISTRY`.
// ---------------------------------------------------------------------------

thread_local! {
    static TEST_REGISTRY: RefCell<Option<TestRegistry>> = const { RefCell::new(None) };

    /// Current source file from which tests are being discovered. The
    /// runner (`fitz test`) sets this via `with_test_source` before
    /// evaluating each project file; `register_test` reads it
    /// to label the `TestSpec`. `None` means "do not label"
    /// (single-file mode or call from unit tests).
    static CURRENT_TEST_SOURCE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Installs an empty registry for the current thread for the
/// duration of the closure. Returns it when finished. Designed for the
/// `fitz test` sub-command (9.z.2.b): install registry, evaluate
/// the project files (lib/bin + tests/), receive the
/// populated registry, and run the runner.
///
/// If `f()` returns `Err` or panics, the previous registry is
/// still restored (`with_*` semantics from `http.rs`).
#[allow(dead_code)] // 9.z.2.a registers; 9.z.2.b installs from the CLI
pub fn with_active_test_registry<F, T>(f: F) -> (T, TestRegistry)
where
    F: FnOnce() -> T,
{
    TEST_REGISTRY.with(|cell| {
        let prev = cell.borrow_mut().take();
        *cell.borrow_mut() = Some(TestRegistry::new());
        let out = f();
        let registry = cell
            .borrow_mut()
            .take()
            .expect("with_active_test_registry instaló un registry — debería estar presente");
        *cell.borrow_mut() = prev;
        (out, registry)
    })
}

/// Async variant of `with_active_test_registry`. Same semantics
/// but accepts a closure that returns a `Future` — needed
/// because the evaluator is async since Phase 6.4.
#[allow(dead_code)] // 9.z.2.b will consume this API
pub async fn with_active_test_registry_async<F, Fut, T>(f: F) -> (T, TestRegistry)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let prev = TEST_REGISTRY.with(|cell| {
        let prev = cell.borrow_mut().take();
        *cell.borrow_mut() = Some(TestRegistry::new());
        prev
    });
    let out = f().await;
    let registry = TEST_REGISTRY.with(|cell| {
        let registry = cell
            .borrow_mut()
            .take()
            .expect("with_active_test_registry_async instaló un registry — debería estar presente");
        *cell.borrow_mut() = prev;
        registry
    });
    (out, registry)
}

/// `true` if there is an active test registry on the current thread.
/// The evaluator checks this before processing a `@test`: if there is none,
/// the decorator is a silent no-op (parallel to Rust's `#[cfg(test)]`:
/// `fitz run` ignores `@test`s without installing a registry).
pub fn has_active_test_registry() -> bool {
    TEST_REGISTRY.with(|cell| cell.borrow().is_some())
}

/// Pushes a test onto the active registry. Call it only after
/// checking `has_active_test_registry()` — if there is no registry,
/// panic (evaluator bug).
pub fn push_test(spec: TestSpec) {
    TEST_REGISTRY.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let reg = borrow
            .as_mut()
            .expect("push_test llamado sin registry activo");
        reg.push(spec);
    });
}

/// Returns the current `source_file` set by
/// `with_test_source`, or `None` if we are not inside a source
/// tracking scope. The evaluator's `@test` branch reads it when
/// building a `TestSpec`.
pub fn current_test_source() -> Option<String> {
    CURRENT_TEST_SOURCE.with(|cell| cell.borrow().clone())
}

/// Installs the current `source_file` for the thread for the
/// duration of the closure. The runner (`fitz test`) uses this before
/// evaluating each project file: any `TestSpec`s pushed inside
/// get labeled with `path`. Nests cleanly (restores the previous
/// one on exit), but real nesting is not expected in normal use.
#[allow(dead_code)] // 9.z.2.b consumes this API from the CLI
pub fn with_test_source<F, T>(path: String, f: F) -> T
where
    F: FnOnce() -> T,
{
    let prev = CURRENT_TEST_SOURCE.with(|cell| {
        let prev = cell.borrow_mut().take();
        *cell.borrow_mut() = Some(path);
        prev
    });
    let out = f();
    CURRENT_TEST_SOURCE.with(|cell| *cell.borrow_mut() = prev);
    out
}

/// Async variant of `with_test_source`. Same semantics + tolerates
/// awaits inside the closure (the evaluator is async since 6.4).
#[allow(dead_code)] // 9.z.2.b consumes this API from the CLI
pub async fn with_test_source_async<F, Fut, T>(path: String, f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let prev = CURRENT_TEST_SOURCE.with(|cell| {
        let prev = cell.borrow_mut().take();
        *cell.borrow_mut() = Some(path);
        prev
    });
    let out = f().await;
    CURRENT_TEST_SOURCE.with(|cell| *cell.borrow_mut() = prev);
    out
}

// ---------------------------------------------------------------------------
// Registry tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    fn dummy_handler() -> Value {
        Value::Function {
            params: vec![],
            body: vec![],
            closure: crate::env::Environment::new(),
            is_async: false,
        }
    }

    #[test]
    fn registry_empty_default() {
        let reg = TestRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.tests().is_empty());
    }

    #[test]
    fn registry_push_preserves_order() {
        let mut reg = TestRegistry::new();
        reg.push(TestSpec {
            name: "primero".into(),
            handler: dummy_handler(),
            is_async: false,
            span: Span::ZERO,
            source_file: None,
        });
        reg.push(TestSpec {
            name: "segundo".into(),
            handler: dummy_handler(),
            is_async: true,
            span: Span::ZERO,
            source_file: None,
        });

        assert_eq!(reg.len(), 2);
        assert_eq!(reg.tests()[0].name, "primero");
        assert_eq!(reg.tests()[1].name, "segundo");
        assert!(!reg.tests()[0].is_async);
        assert!(reg.tests()[1].is_async);
    }

    #[test]
    fn has_active_registry_is_false_by_default() {
        // Outside `with_active_test_registry`, the thread_local is None.
        // We cannot guarantee the state inside other tests of the
        // same binary (they run on different threads), but absent
        // explicit setup in this test, it should be false.
        assert!(!has_active_test_registry());
    }

    #[test]
    fn with_active_test_registry_installs_and_returns() {
        let prev = has_active_test_registry();
        assert!(!prev, "el registry no debería estar instalado al arrancar");

        let (out, reg) = with_active_test_registry(|| {
            assert!(has_active_test_registry());
            push_test(TestSpec {
                name: "smoke".into(),
                handler: dummy_handler(),
                is_async: false,
                span: Span::ZERO,
                source_file: None,
            });
            42
        });

        assert_eq!(out, 42);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.tests()[0].name, "smoke");
        assert!(!has_active_test_registry(), "el registry se restauró");
    }

    #[test]
    fn with_active_test_registry_nested_are_isolated() {
        // Defensive case: if two `with_active` nest, the inner one
        // does not contaminate the outer one. Mirror of the
        // `http::with_active_registry` behavior.
        let (_, outer) = with_active_test_registry(|| {
            push_test(TestSpec {
                name: "outer".into(),
                handler: dummy_handler(),
                is_async: false,
                span: Span::ZERO,
                source_file: None,
            });
            let (_, inner) = with_active_test_registry(|| {
                push_test(TestSpec {
                    name: "inner".into(),
                    handler: dummy_handler(),
                    is_async: false,
                    span: Span::ZERO,
                    source_file: None,
                });
            });
            assert_eq!(inner.len(), 1);
            assert_eq!(inner.tests()[0].name, "inner");
        });
        assert_eq!(outer.len(), 1);
        assert_eq!(outer.tests()[0].name, "outer");
    }

    #[test]
    fn with_test_source_labels_the_specs() {
        // 9.z.2.b: the runner uses `with_test_source` to label
        // tests with the file they come from. Here we validate the
        // flow: inside the scope, `current_test_source` returns the
        // path; on exit it goes back to None.
        assert!(current_test_source().is_none());

        let result = with_test_source("tests/math.fitz".to_string(), || {
            assert_eq!(current_test_source(), Some("tests/math.fitz".to_string()),);
            "ok"
        });
        assert_eq!(result, "ok");
        assert!(current_test_source().is_none(), "se restauró al salir");
    }

    #[test]
    fn with_test_source_nested_are_restored() {
        with_test_source("outer.fitz".to_string(), || {
            assert_eq!(current_test_source(), Some("outer.fitz".to_string()));
            with_test_source("inner.fitz".to_string(), || {
                assert_eq!(current_test_source(), Some("inner.fitz".to_string()));
            });
            // The outer one is restored after the inner.
            assert_eq!(current_test_source(), Some("outer.fitz".to_string()));
        });
    }

    #[tokio::test]
    async fn with_active_test_registry_async_works() {
        // Tokio test to validate the async variant.
        let (out, reg) = with_active_test_registry_async(|| async {
            push_test(TestSpec {
                name: "async_smoke".into(),
                handler: dummy_handler(),
                is_async: true,
                span: Span::ZERO,
                source_file: None,
            });
            "done"
        })
        .await;

        assert_eq!(out, "done");
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.tests()[0].name, "async_smoke");
        assert!(reg.tests()[0].is_async);
    }
}
