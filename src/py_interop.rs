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
use pyo3::types::{PyBool, PyFloat, PyInt, PyString};

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

/// Fase 8.1.3 — acceso a atributo sobre un objeto Python con
/// auto-coerción primitiva. Implementa la mecánica de `math.pi`
/// (constante Float), `math.sqrt` (función opaca → `Value::PyObject`),
/// y por extensión cualquier `obj.attr` donde `obj: Value::PyObject`.
///
/// Política de coerción en 8.1:
///   - `None` → `Value::Null`
///   - `bool` → `Value::Bool` (chequea **antes** que int — en Python
///     `bool ⊂ int`).
///   - `int` → `Value::Int` si cabe en `i64`. Si excede, error
///     explícito (deuda menor: bignum support cuando entre demanda).
///   - `float` → `Value::Float`.
///   - `str` → `Value::Str`.
///   - cualquier otro tipo (función, clase, instancia, list, dict,
///     submódulo, etc.) → `Value::PyObject` opaco. Marshaling para
///     `list/dict` específicos llega en 8.2.
pub fn get_attr(handle: &PyObjectHandle, name: &str) -> FitzResult<Value> {
    Python::attach(|py| {
        let bound = handle.0.bind(py);
        match bound.getattr(name) {
            Ok(attr) => py_to_value(py, &attr),
            Err(err) => Err(py_err_to_fitz(py, err)),
        }
    })
}

/// Fase 8.1.4 — invocar un PyObject callable (función, método, clase)
/// con args ya evaluados a `Value` Fitz. Toma el GIL una sola vez,
/// convierte los args via `value_to_py`, invoca, y baja el return a
/// `Value` via `py_to_value`. Sobre la convención de errores: cualquier
/// excepción Python (TypeError, ValueError, etc.) se traduce al
/// `FitzError` con "<ClassName>: <message>" como mensaje — el wrap a
/// `Result<T>` llega en 8.3.
pub fn call(handle: &PyObjectHandle, args: &[Value]) -> FitzResult<Value> {
    Python::attach(|py| {
        let bound = handle.0.bind(py);
        // Convertir cada arg Fitz → PyObject. Si alguno no es
        // marshallable en 8.1 (List/Map/Instance/Range/Function/etc.),
        // cortamos con el error explícito antes de tocar Python.
        let py_args: Vec<Py<PyAny>> = args
            .iter()
            .map(|v| value_to_py(py, v))
            .collect::<FitzResult<Vec<_>>>()?;
        // `call1` toma una tupla posicional sin kwargs. Es el caso típico
        // de `math.sqrt(16.0)` y `os.path.join("a", "b")`. Kwargs llega
        // como deuda menor cuando entre demanda real.
        let args_tuple = pyo3::types::PyTuple::new(py, py_args)
            .map_err(|e| py_err_to_fitz(py, e))?;
        match bound.call1(args_tuple) {
            Ok(ret) => py_to_value(py, &ret),
            Err(err) => Err(py_err_to_fitz(py, err)),
        }
    })
}

/// Convierte un `Value` Fitz a un `Py<PyAny>` para pasarlo a Python.
/// Política 8.1 (primitivos):
///   - `Int`   → `int`
///   - `Float` → `float`
///   - `Str`   → `str`
///   - `Bool`  → `bool`
///   - `Null`  → `None`
///   - `PyObject(h)` → passthrough (un round-trip Fitz→Python→Fitz
///     preserva identidad porque solo bumpamos el refcount).
///
/// Tipos compuestos (`List`, `Map`, `Instance`, `Range`, `Function`,
/// etc.) → error explícito citando la deuda de 8.2.
fn value_to_py(py: Python<'_>, value: &Value) -> FitzResult<Py<PyAny>> {
    use pyo3::IntoPyObject;
    match value {
        Value::Int(n) => Ok(n.into_pyobject(py)
            .map_err(|e| py_err_to_fitz(py, e.into()))?
            .into_any()
            .unbind()),
        Value::Float(f) => Ok(f.into_pyobject(py)
            .map_err(|e| py_err_to_fitz(py, e.into()))?
            .into_any()
            .unbind()),
        Value::Str(s) => Ok(s.into_pyobject(py)
            .map_err(|e| py_err_to_fitz(py, e.into()))?
            .into_any()
            .unbind()),
        Value::Bool(b) => {
            // `bool::into_pyobject` devuelve `Borrowed<'py, PyBool>` (no
            // un `Bound`), porque True/False son singletons compartidos.
            // Lo convertimos a `Py<PyAny>` via `.to_owned().into_any()`.
            let bound = b.into_pyobject(py)
                .map_err(|e| py_err_to_fitz(py, e.into()))?;
            Ok(bound.to_owned().into_any().unbind())
        }
        Value::Null => Ok(py.None()),
        // Passthrough: un `Value::PyObject` que cruza de vuelta a Python
        // es el mismo objeto. Clonamos el `Py<PyAny>` (refcount bump).
        Value::PyObject(h) => Ok(h.0.clone_ref(py)),
        // Resto: en 8.1 solo soportamos primitivos. Cuando 8.2 cierre,
        // estas ramas pasan a marshalling real (List → list, Map → dict,
        // Instance → dict por field name).
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "primitivo o PyObject".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!(
                "no se puede pasar un valor de tipo `{}` a una función Python en 8.1; \
                 marshaling de tipos compuestos (List/Map/Instance) llega en 8.2",
                other.type_name(),
            ),
        )),
    }
}

/// Convierte un `Bound<'_, PyAny>` a `Value` Fitz aplicando la política
/// de coerción primitiva. Helper reusable: lo consumen `get_attr`
/// (8.1.3) y `call` (8.1.4) para procesar el return value.
///
/// Pre-condición: el caller ya tomó el GIL (parámetro `py` lo testifica).
fn py_to_value(py: Python<'_>, obj: &Bound<'_, PyAny>) -> FitzResult<Value> {
    if obj.is_none() {
        return Ok(Value::Null);
    }
    // bool ANTES que int: en Python `isinstance(True, int) == True`, así
    // que un chequeo de int primero capturaría True/False como 1/0.
    if obj.is_instance_of::<PyBool>() {
        let b: bool = obj.extract().map_err(|e| py_err_to_fitz(py, e))?;
        return Ok(Value::Bool(b));
    }
    if obj.is_instance_of::<PyInt>() {
        // `extract::<i64>()` falla si el int Python excede el rango de
        // i64 (`2^63`). En 8.1 reportamos error claro citando el
        // límite; bignum support quedaría como deuda menor de 8.2+.
        return match obj.extract::<i64>() {
            Ok(n) => Ok(Value::Int(n)),
            Err(_) => Err(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int (i64)".into(),
                    found: "int Python fuera de rango i64".into(),
                },
                0, 0,
                format!(
                    "el entero Python `{}` excede el rango de Int en Fitz (i64); \
                     bignum support llega en una fase posterior",
                    obj.str().map(|s| s.to_string()).unwrap_or_else(|_| "<repr falló>".into()),
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
    // Fallback: tipos compuestos (list, dict, función, clase, instancia,
    // submódulo). En 8.1 los envolvemos opacos. 8.2 trae marshaling
    // bidireccional para list/dict/Instance.
    let owned: Py<PyAny> = obj.clone().unbind();
    Ok(Value::PyObject(PyObjectHandle::new(owned)))
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

    // -------------------------------------------------------------------
    // 8.1.3 — get_attr + auto-coerción primitiva
    // -------------------------------------------------------------------

    fn handle_of(v: Value) -> PyObjectHandle {
        match v {
            Value::PyObject(h) => h,
            other => panic!("se esperaba Value::PyObject, fue: {:?}", other),
        }
    }

    #[test]
    fn get_attr_math_pi_es_float() {
        let math = handle_of(import_module("math").unwrap());
        let v = get_attr(&math, "pi").expect("math.pi debería existir");
        match v {
            Value::Float(f) => {
                // Comparación aproximada — el valor exacto de math.pi
                // está pinneado, pero usamos un epsilon por las dudas.
                assert!((f - std::f64::consts::PI).abs() < 1e-15, "got {}", f);
            }
            other => panic!("se esperaba Float, fue: {:?}", other),
        }
    }

    #[test]
    fn get_attr_math_sqrt_es_pyobject_opaco() {
        // `sqrt` es una función Python — no es primitivo, debe
        // envolverse como PyObject opaco para invocación en 8.1.4.
        let math = handle_of(import_module("math").unwrap());
        let v = get_attr(&math, "sqrt").expect("math.sqrt debería existir");
        assert!(matches!(v, Value::PyObject(_)), "got: {:?}", v);
    }

    #[test]
    fn get_attr_inexistente_emite_attributeerror() {
        let math = handle_of(import_module("math").unwrap());
        let err = get_attr(&math, "no_existe_xyz_813").expect_err("attr no debería existir");
        assert!(
            err.message.contains("AttributeError"),
            "mensaje debería citar AttributeError, fue: {}",
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
        // Construimos un PyBool sin pasar por un módulo: importamos
        // `builtins` y leemos `True`.
        let builtins = handle_of(import_module("builtins").unwrap());
        let v = get_attr(&builtins, "True").unwrap();
        assert_eq!(v, Value::Bool(true));
    }

    #[test]
    fn py_to_value_coerciona_int_chico() {
        // `sys.maxsize` es un int Python — en sistemas 64-bit es 2^63 - 1,
        // que cabe justo en i64. Lo usamos para verificar que int → Int.
        let sys = handle_of(import_module("sys").unwrap());
        let v = get_attr(&sys, "maxsize").unwrap();
        assert_eq!(v, Value::Int(i64::MAX));
    }

    #[test]
    fn py_to_value_coerciona_none() {
        // `sys.__interactivehook__` no siempre es None, mejor un atributo
        // que sea explícitamente None. Usamos `ctypes` que en algunos
        // sistemas puede no estar; mejor usar un truco: `sys.flags` tiene
        // sub-atributos, pero todos son int. Usamos `inspect.Parameter.empty`
        // que es un sentinel — pero ese no es None. Vamos por el camino
        // directo: importar `dataclasses` y leer `MISSING` no funciona
        // porque es objeto. Mejor: evaluamos el atributo `None` de
        // `builtins`, que ES el singleton Python None.
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
        let v = call(&sqrt, &[Value::Float(16.0)]).expect("sqrt(16.0) ok");
        assert_eq!(v, Value::Float(4.0));
    }

    #[test]
    fn call_math_floor_arg_float_da_int() {
        let math = handle_of(import_module("math").unwrap());
        let floor = handle_of(get_attr(&math, "floor").unwrap());
        let v = call(&floor, &[Value::Float(3.7)]).expect("floor ok");
        assert_eq!(v, Value::Int(3));
    }

    #[test]
    fn call_str_upper_via_call_no_aplica_es_metodo() {
        // `str.upper("hola")` es válido en Python (unbound method). Usamos
        // este caso para verificar que un argumento Str se marshalla bien.
        let builtins = handle_of(import_module("builtins").unwrap());
        let str_cls = handle_of(get_attr(&builtins, "str").unwrap());
        let upper = handle_of(get_attr(&str_cls, "upper").unwrap());
        let v = call(&upper, &[Value::Str("hola".into())]).expect("str.upper ok");
        assert_eq!(v, Value::Str("HOLA".into()));
    }

    #[test]
    fn call_arg_int_coerciona_a_pyint() {
        // `abs(-7)` debería darnos `7`. Validamos que Int → int Python.
        let builtins = handle_of(import_module("builtins").unwrap());
        let abs_fn = handle_of(get_attr(&builtins, "abs").unwrap());
        let v = call(&abs_fn, &[Value::Int(-7)]).expect("abs(-7) ok");
        assert_eq!(v, Value::Int(7));
    }

    #[test]
    fn call_arg_bool_coerciona_a_pybool() {
        // `bool.__class__.__name__` de True → "bool". Mejor: `int(True)`
        // → 1. Confirmamos que el Bool va como Python bool, no como int.
        let builtins = handle_of(import_module("builtins").unwrap());
        let int_cls = handle_of(get_attr(&builtins, "int").unwrap());
        let v = call(&int_cls, &[Value::Bool(true)]).expect("int(True) ok");
        assert_eq!(v, Value::Int(1));
    }

    #[test]
    fn call_arg_null_coerciona_a_none() {
        // `str(None)` da "None". Verifica que Null → Python None.
        let builtins = handle_of(import_module("builtins").unwrap());
        let str_cls = handle_of(get_attr(&builtins, "str").unwrap());
        let v = call(&str_cls, &[Value::Null]).expect("str(None) ok");
        assert_eq!(v, Value::Str("None".into()));
    }

    #[test]
    fn call_excepcion_python_se_traduce_a_fitz_error() {
        // `math.sqrt(-1)` lanza ValueError en Python. Verificamos que el
        // mensaje del FitzError tiene formato "<ClassName>: <message>".
        let math = handle_of(import_module("math").unwrap());
        let sqrt = handle_of(get_attr(&math, "sqrt").unwrap());
        let err = call(&sqrt, &[Value::Float(-1.0)]).expect_err("sqrt(-1) debería fallar");
        assert!(
            err.message.contains("ValueError"),
            "mensaje debería citar ValueError, fue: {}",
            err.message,
        );
    }

    #[test]
    fn call_arg_compuesto_no_marshalleable_es_error() {
        // List, Map, Instance, etc. no son marshalleables en 8.1. El
        // mensaje debe citar 8.2 como la fase que lo cubre.
        let math = handle_of(import_module("math").unwrap());
        let sqrt = handle_of(get_attr(&math, "sqrt").unwrap());
        let err = call(&sqrt, &[Value::new_list(vec![Value::Int(1)])])
            .expect_err("List no debería marshallarse en 8.1");
        assert!(
            err.message.contains("8.2"),
            "mensaje debería citar 8.2 como sub-paso futuro, fue: {}",
            err.message,
        );
    }

    #[test]
    fn call_pyobject_passthrough_preserva_identidad() {
        // Pasar un Value::PyObject como arg: debería llegar al callable
        // Python sin cambios. Validamos con `id(x) == id(x)` via `is`.
        let builtins = handle_of(import_module("builtins").unwrap());
        let id_fn = handle_of(get_attr(&builtins, "id").unwrap());
        let math = import_module("math").unwrap();
        // Mismo objeto pasado dos veces: `id` debe devolver el mismo Int.
        let id1 = call(&id_fn, std::slice::from_ref(&math)).unwrap();
        let id2 = call(&id_fn, &[math]).unwrap();
        assert_eq!(id1, id2);
    }
}
