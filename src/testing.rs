// testing.rs — Fase 9.z.2 (testing built-in)
//
// Runtime de tests de Fitz. Mecánica análoga al `HttpRegistry`:
//
//   1. Durante `eval`, cuando se ve un `Stmt::FnDef` con un decorator
//      `@test`, el evaluator empuja una `TestSpec` al `TestRegistry`
//      activo via thread_local.
//   2. El sub-comando `fitz test` (9.z.2.b) instala el registry vía
//      `with_active_test_registry`, evalúa el archivo (lo que descubre
//      los tests + carga los módulos), y al salir itera el registry
//      invocando cada handler. `fitz run` NO instala registry — los
//      decorators `@test` se vuelven no-op silencioso (paralelo a
//      `#[cfg(test)]` de Rust).
//
// No hay paralelismo entre tests: el runner los corre serie en el
// orden de descubrimiento. La paralelización es deuda menor si aparece
// presión (los tests inline + en módulos suelen ser rápidos; el costo
// real está en setup/teardown que no existe en el MVP).
//
// Sub-pasos:
//   - 9.z.2.a (este archivo + branch en `process_decorator` +
//     assertion builtins): registry + decorator + builtins.
//   - 9.z.2.b: CLI `fitz test` + discovery (lib/bin + `tests/*.fitz`)
//     + runner con output estilo cargo + filtrado.
//   - 9.z.2.c: cap en la guía + ejemplos + cierre formal.

use std::cell::RefCell;

use crate::ast::Span;
use crate::value::Value;

// ---------------------------------------------------------------------------
// Tipos base
// ---------------------------------------------------------------------------

/// Una fn marcada con `@test`. El `handler` es un `Value::Function`
/// clonado del env del intérprete — el `closure: EnvRef` mantiene
/// viva la env del módulo donde se declaró, igual que `RouteSpec`.
///
/// `is_async` se preserva del FnDef para que el runner pueda
/// elegir entre invocar sync o await-ear el `Value::Future`
/// resultante. `span` apunta al `@test` (no a la fn) — útil para
/// reportes "test X declarado en línea:Y falló por ...".
#[derive(Debug, Clone)]
pub struct TestSpec {
    pub name: String,
    pub handler: Value,
    pub is_async: bool,
    pub span: Span,
}

/// Colección de tests descubiertos durante la evaluación. Preserva
/// el orden de declaración (insertion order) — el runner los corre
/// en ese orden, que típicamente coincide con el orden de lectura
/// del usuario.
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

    /// Vista inmutable de los tests registrados, en orden de
    /// descubrimiento. El runner los itera con esto.
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
// Thread-local registry — mismo patrón que `http::HTTP_REGISTRY`.
// ---------------------------------------------------------------------------

thread_local! {
    static TEST_REGISTRY: RefCell<Option<TestRegistry>> = const { RefCell::new(None) };
}

/// Instala un registry vacío para el thread actual durante la
/// duración del closure. Al terminar lo devuelve. Pensado para el
/// sub-comando `fitz test` (9.z.2.b): instalar registry, evaluar
/// los archivos del proyecto (lib/bin + tests/), recibir el
/// registry poblado y correr el runner.
///
/// Si `f()` retorna `Err` o paniquea, el registry previo se
/// restaura igual (semántica del `with_*` de `http.rs`).
#[allow(dead_code)] // 9.z.2.a registra; 9.z.2.b instala desde el CLI
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

/// Variante async de `with_active_test_registry`. Misma semántica
/// pero acepta una closure que devuelve un `Future` — necesaria
/// porque el evaluator es async desde Fase 6.4.
#[allow(dead_code)] // 9.z.2.b consumirá esta API
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
            .expect(
                "with_active_test_registry_async instaló un registry — debería estar presente",
            );
        *cell.borrow_mut() = prev;
        registry
    });
    (out, registry)
}

/// `true` si hay un registry de tests activo en el thread actual.
/// El evaluator lo consulta antes de procesar un `@test`: si no hay,
/// el decorator es no-op silencioso (paralelo a `#[cfg(test)]` Rust:
/// `fitz run` ignora los `@test` sin instalar registry).
pub fn has_active_test_registry() -> bool {
    TEST_REGISTRY.with(|cell| cell.borrow().is_some())
}

/// Empuja un test al registry activo. Llamarlo solo después de
/// chequear `has_active_test_registry()` — si no hay registry,
/// pánico (bug del evaluator).
pub fn push_test(spec: TestSpec) {
    TEST_REGISTRY.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let reg = borrow
            .as_mut()
            .expect("push_test llamado sin registry activo");
        reg.push(spec);
    });
}

// ---------------------------------------------------------------------------
// Tests del registry
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
        });
        reg.push(TestSpec {
            name: "segundo".into(),
            handler: dummy_handler(),
            is_async: true,
            span: Span::ZERO,
        });

        assert_eq!(reg.len(), 2);
        assert_eq!(reg.tests()[0].name, "primero");
        assert_eq!(reg.tests()[1].name, "segundo");
        assert!(!reg.tests()[0].is_async);
        assert!(reg.tests()[1].is_async);
    }

    #[test]
    fn has_active_registry_es_false_por_default() {
        // Fuera de `with_active_test_registry`, el thread_local es None.
        // No podemos asegurar el estado adentro de otros tests del
        // mismo binary (corren en threads distintos), pero a falta de
        // setup explícito en este test, debería ser false.
        assert!(!has_active_test_registry());
    }

    #[test]
    fn with_active_test_registry_instala_y_devuelve() {
        let prev = has_active_test_registry();
        assert!(!prev, "el registry no debería estar instalado al arrancar");

        let (out, reg) = with_active_test_registry(|| {
            assert!(has_active_test_registry());
            push_test(TestSpec {
                name: "smoke".into(),
                handler: dummy_handler(),
                is_async: false,
                span: Span::ZERO,
            });
            42
        });

        assert_eq!(out, 42);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.tests()[0].name, "smoke");
        assert!(!has_active_test_registry(), "el registry se restauró");
    }

    #[test]
    fn with_active_test_registry_anidados_se_aislan() {
        // Caso defensivo: si dos `with_active` se anidan, el del inner
        // no contamina al outer. Mirror del comportamiento de
        // `http::with_active_registry`.
        let (_, outer) = with_active_test_registry(|| {
            push_test(TestSpec {
                name: "outer".into(),
                handler: dummy_handler(),
                is_async: false,
                span: Span::ZERO,
            });
            let (_, inner) = with_active_test_registry(|| {
                push_test(TestSpec {
                    name: "inner".into(),
                    handler: dummy_handler(),
                    is_async: false,
                    span: Span::ZERO,
                });
            });
            assert_eq!(inner.len(), 1);
            assert_eq!(inner.tests()[0].name, "inner");
        });
        assert_eq!(outer.len(), 1);
        assert_eq!(outer.tests()[0].name, "outer");
    }

    #[tokio::test]
    async fn with_active_test_registry_async_funciona() {
        // Tokio test para validar la variante async.
        let (out, reg) = with_active_test_registry_async(|| async {
            push_test(TestSpec {
                name: "async_smoke".into(),
                handler: dummy_handler(),
                is_async: true,
                span: Span::ZERO,
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
