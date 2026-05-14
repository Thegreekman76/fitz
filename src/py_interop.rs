// py_interop.rs — Fase 8.1.2: interop con CPython via PyO3
//
// Punto único de entrada al runtime Python embebido desde el evaluator.
// Todo el resto del compilador habla `Value` Fitz y `FitzError`; este
// módulo se encarga de cruzar la frontera: tomar el GIL, llamar APIs
// de PyO3, traducir excepciones Python a `FitzError`, envolver el
// `Py<PyAny>` resultante en `Value::PyObject`.
//
// Existe solo cuando se compila con `--features python`. El binario
// `fitz` default (sin la feature) ni siquiera linkea libpython.
//
// Política de GIL: un `Python::with_gil` por cada operación pública de
// este módulo. Eso quiere decir: el GIL se toma y suelta en cada
// `import_module`. Para casos típicos (un `from python import math`
// por programa) el costo es despreciable. Cuando llegue Fase 8.6
// (async + tokio + asyncio bridge), revisamos.
//
// Política de errores: cualquier `PyErr` se traduce a `FitzError` con
// mensaje "<ClassName>: <message>". La conversión a `Result<T>`
// automática llega en Fase 8.3 — en 8.1 el error aborta el programa
// igual que un panic del intérprete.

use pyo3::prelude::*;

use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::value::{PyObjectHandle, Value};

/// Importa un módulo Python dado su path "punteado" (`"math"`,
/// `"sqlalchemy.orm"`, etc.) y lo devuelve envuelto en `Value::PyObject`.
///
/// Internamente delega en `Bound::<PyModule>::import(py, dotted)` —
/// equivalente al `import <dotted>` de un script Python con el `sys.path`
/// estándar del intérprete embebido. Política de venvs (8.1): el
/// usuario activa su venv antes de `fitz run`; Python lo detecta vía
/// `VIRTUAL_ENV` al boot. Sin venv, los packages se buscan en el
/// site-packages global del intérprete base contra el que se linkeó.
///
/// Errores: si el módulo no existe (`ModuleNotFoundError`), si el path
/// es inválido, o si Python explota inicializando, devolvemos
/// `FitzError` con la línea/columna del caller (que se inyecta arriba)
/// y mensaje "<ClassName>: <message>".
pub fn import_module(dotted: &str) -> FitzResult<Value> {
    // `Python::attach` reemplazó a `Python::with_gil` en pyo3 0.23+;
    // la API es idéntica en uso (closure recibe `Python<'_>`). Toma
    // el GIL si la feature `auto-initialize` no lo hizo todavía;
    // sobre runs subsiguientes es un fetch + lock.
    Python::attach(|py| match py.import(dotted) {
        Ok(module) => {
            // `module: Bound<'py, PyModule>`. Lo convertimos a
            // `Py<PyAny>` 'static para guardarlo en `Value::PyObject`
            // sin atarnos al lifetime del GIL token. `.into_any()` baja
            // el tipo de `PyModule` a `PyAny` (subtipo); `.unbind()`
            // libera el lifetime del `Bound` y devuelve `Py<PyAny>`.
            let py_any: Py<PyAny> = module.into_any().unbind();
            Ok(Value::PyObject(PyObjectHandle::new(py_any)))
        }
        Err(err) => Err(py_err_to_fitz(py, err)),
    })
}

/// Convierte un `PyErr` a un `FitzError` con mensaje
/// "<ClassName>: <message>". El formato matchea la convención que la
/// Fase 8.3 va a estabilizar cuando los wraps a `Result<T>` lleguen —
/// el `Err(...)` que va a recibir el usuario será el mismo string que
/// hoy aparece en el `FitzError`.
///
/// Si la introspección de la excepción falla (caso raro: error del
/// propio PyO3 al consultar el type), devolvemos un mensaje genérico
/// sin colgar el programa.
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
    fn importa_math_devuelve_pyobject() {
        let v = import_module("math").expect("math debería importar");
        assert!(matches!(v, Value::PyObject(_)));
    }

    #[test]
    fn importa_json_devuelve_pyobject() {
        let v = import_module("json").expect("json debería importar");
        assert!(matches!(v, Value::PyObject(_)));
    }

    #[test]
    fn importa_submodulo_devuelve_pyobject() {
        // `os.path` existe siempre en cualquier instalación de Python.
        let v = import_module("os.path").expect("os.path debería importar");
        assert!(matches!(v, Value::PyObject(_)));
    }

    #[test]
    fn modulo_inexistente_da_error_claro() {
        let err = import_module("este_modulo_no_existe_xyz_8_1_2")
            .expect_err("el módulo no debería existir");
        // Esperamos algo como "ModuleNotFoundError: No module named 'este_modulo_no_existe_xyz_8_1_2'"
        assert!(
            err.message.contains("ModuleNotFoundError"),
            "mensaje debería citar ModuleNotFoundError, fue: {}",
            err.message
        );
        assert!(
            err.message.contains("este_modulo_no_existe_xyz_8_1_2"),
            "mensaje debería citar el nombre del módulo buscado, fue: {}",
            err.message
        );
    }

    #[test]
    fn dos_imports_del_mismo_modulo_son_iguales() {
        // Python cachea los imports (sys.modules), así que dos
        // `import math` devuelven el mismo objeto. Nuestro `PartialEq`
        // sobre `Value::PyObject` (Py::as_ptr) debería reflejar eso.
        let a = import_module("math").unwrap();
        let b = import_module("math").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn imports_de_modulos_distintos_no_son_iguales() {
        let a = import_module("math").unwrap();
        let b = import_module("json").unwrap();
        assert_ne!(a, b);
    }
}
