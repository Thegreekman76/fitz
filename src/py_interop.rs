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
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};

use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::value::{FitzFuture, PyObjectHandle, ResultVariant, Value};

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

/// Fase 8.3 — invocar un PyObject callable (función, método, clase)
/// con args ya evaluados a `Value` Fitz. **Toda llamada Python desde
/// Fitz se envuelve automáticamente en `Result<T>`**: éxito produce
/// `Value::Result(Ok(v))` con el valor coercionado adentro; cualquier
/// falla del path Python (excepción Python, marshaling de args
/// imposible, etc.) produce `Value::Result(Err(Str("<ClassName>:
/// <message>")))` sin abortar el programa.
///
/// Esta convención preserva el modelo de errores de Fitz (sin
/// excepciones): el usuario es forzado a manejar la falla con
/// `match` o el operador `?`, igual que con `find`/`get`/`json.loads`
/// nativos. Excepciones Python ya no se cuelan como panics opacos.
///
/// El path de la firma `FitzResult<Value>` se mantiene solo para
/// errores catastróficos del propio runtime de Fitz (que no han
/// aparecido en la práctica); en el flujo normal devolvemos
/// `Ok(Value::Result(...))` siempre.
///
/// **Errores cubiertos por el `Result::Err`**:
///   - Excepción Python lanzada por el callable (ValueError,
///     TypeError, etc.) — incluyendo KeyboardInterrupt/SystemExit
///     según roadmap (no hay forma de matar el runtime Fitz desde
///     una excepción Python).
///   - Marshaling de args fallido (tipo Fitz no representable en
///     Python — Range/Function/Type/Module/etc. con breadcrumb
///     informativo via `path`).
///   - Marshaling del return fallido (raro: int Python > i64).
///   - Construcción de la tupla de args (defensive — debería ser
///     infalible en práctica).
pub fn call(handle: &PyObjectHandle, args: &[Value]) -> FitzResult<Value> {
    Python::attach(|py| {
        let bound = handle.0.bind(py);
        // Convertir cada arg Fitz → PyObject. Errores de marshaling
        // (Range/Function/etc., o keys no hashables) se envuelven en
        // `Result::Err` con el mensaje del FitzError. Esto unifica
        // todo el path: el usuario ve UN solo punto de error
        // (`?` o `match`) independiente de qué falló.
        let py_args_result: FitzResult<Vec<Py<PyAny>>> = args
            .iter()
            .enumerate()
            .map(|(i, v)| value_to_py(py, v, &format!("arg{}", i)))
            .collect();
        let py_args = match py_args_result {
            Ok(v) => v,
            Err(e) => return Ok(err_value_from_message(e.message)),
        };
        // `call1` toma una tupla posicional sin kwargs. Es el caso típico
        // de `math.sqrt(16.0)` y `os.path.join("a", "b")`. Kwargs llega
        // como deuda menor cuando entre demanda real.
        let args_tuple = match pyo3::types::PyTuple::new(py, py_args) {
            Ok(t) => t,
            Err(e) => return Ok(err_value_from_message(py_err_to_fitz(py, e).message)),
        };
        match bound.call1(args_tuple) {
            Ok(ret) => {
                // Fase 8.6: si el return es una corutina Python
                // (caso típico cuando se llama una `async def`),
                // convertimos a `Value::Future` en lugar de `PyObject`
                // opaco. Esto destraba `py_async_fn().await` desde
                // Fitz sin glue manual — el `.await` postfix existente
                // (Fase 6) desempaca el `Value::Future` y devuelve el
                // valor coercionado.
                if is_coroutine(py, &ret) {
                    return match py_coro_to_fitz_future(&ret) {
                        Ok(fut) => Ok(Value::Result(ResultVariant::Ok(Box::new(
                            Value::new_future(fut),
                        )))),
                        Err(e) => Ok(err_value_from_message(
                            py_err_to_fitz(py, e).message,
                        )),
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

/// Fase 8.6 — chequea si un objeto Python es awaitable (una corutina
/// `async def`, un Task, o cualquier objeto con `__await__`). Usa
/// `inspect.isawaitable`, que es la forma canónica en Python stdlib.
///
/// Tomamos el GIL implícito (el caller ya lo tiene). Devolvemos
/// `false` si la introspección falla — es defensivo: mejor tratar el
/// objeto como no-awaitable que producir un wrap incorrecto.
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

/// Fase 8.6 — convierte una corutina Python en un `FitzFuture` que
/// el `Value::Future` Fitz puede envolver. El usuario escribe
/// `py_async_fn().await` y el bridge es invisible.
///
/// **Implementación (8.6.1, "baseline blocking")**: usamos
/// `tokio::task::spawn_blocking` + `asyncio.run_until_complete` adentro
/// del worker thread. Esto:
///
///   - **Es Send-safe**: el `Py<PyAny>` viaja entre threads (PyO3 lo
///     marca como Send), el `Bound<'py, PyAny>` derivado solo existe
///     adentro del `Python::attach` del worker thread. El FitzFuture
///     externo solo tiene el `JoinHandle` (Send).
///   - **No deadlockea con el runtime tokio existente**: el worker es
///     del blocking pool de tokio, no del scheduler async.
///   - **Serializa por GIL**: 100 awaits concurrentes a corutinas
///     Python distintas se serializan en el GIL — comportamiento
///     esperado y documentado en el roadmap. Para APIs DB-bound
///     (caso típico SQLAlchemy/asyncpg), la DB es el bottleneck, no
///     el GIL.
///
/// **Trade-off conocido**: cada `.await` ocupa un thread del blocking
/// pool durante toda la duración de la corutina. Para corutinas largas
/// (segundos) en alta concurrencia, esto es subóptimo. Una versión
/// future-based real (compartiendo un event loop asyncio persistente)
/// queda como deuda menor — `pyo3-async-runtimes::tokio::into_future`
/// es la API correcta, pero requiere que pyo3-async-runtimes controle
/// el runtime tokio, lo cual choca con el setup ya establecido de
/// Fitz (tokio current_thread CLI / rt-multi-thread HTTP).
///
/// **Política de GIL**: el GIL se mantiene durante todo el
/// `run_until_complete`. PyO3 no lo suelta automáticamente entre
/// pasos de asyncio porque toda la coordinación es de un solo thread.
fn py_coro_to_fitz_future(coro: &Bound<'_, PyAny>) -> PyResult<FitzFuture> {
    // `Py<PyAny>` es Send — lo capturamos para mover al worker.
    let coro_owned: Py<PyAny> = coro.clone().unbind();
    let fitz_future: FitzFuture = Box::pin(async move {
        // Mover el trabajo bloqueante al pool de threads tokio.
        let join_result = tokio::task::spawn_blocking(move || {
            Python::attach(|py| -> FitzResult<Value> {
                let bound = coro_owned.bind(py);
                let asyncio = py.import("asyncio").map_err(|e| {
                    FitzError::new(
                        ErrorKind::UndefinedVariable("PyError".to_string()),
                        0, 0,
                        py_err_to_fitz(py, e).message,
                    )
                })?;
                // Crear un loop nuevo por call. Costo: ~ms; aceptable
                // para 8.6.1. Optimización: pool persistente como
                // deuda menor.
                let event_loop = asyncio.call_method0("new_event_loop").map_err(|e| {
                    FitzError::new(
                        ErrorKind::UndefinedVariable("PyError".to_string()),
                        0, 0,
                        py_err_to_fitz(py, e).message,
                    )
                })?;
                let result = event_loop.call_method1("run_until_complete", (bound,));
                let _ = event_loop.call_method0("close"); // best-effort
                match result {
                    Ok(value) => py_to_value(py, &value),
                    Err(e) => Err(FitzError::new(
                        ErrorKind::UndefinedVariable("PyError".to_string()),
                        0, 0,
                        py_err_to_fitz(py, e).message,
                    )),
                }
            })
        })
        .await;
        match join_result {
            Ok(inner) => inner,
            Err(join_err) => Err(FitzError::new(
                ErrorKind::UndefinedVariable("RuntimeError".to_string()),
                0, 0,
                format!("error del blocking pool al ejecutar corutina Python: {}", join_err),
            )),
        }
    });
    Ok(fitz_future)
}

/// Helper para construir el `Value::Result(Err(Str(msg)))` que envuelve
/// los errores de una llamada Python. El `msg` ya viene con formato
/// `"<ClassName>: <message>"` desde `py_err_to_fitz` (excepciones
/// Python) o desde el `FitzError.message` de los marshaling fallos.
fn err_value_from_message(msg: String) -> Value {
    Value::Result(ResultVariant::Err(Box::new(Value::Str(msg))))
}

/// Convierte un `Value` Fitz a un `Py<PyAny>` para pasarlo a Python.
/// Política 8.2 (primitivos + compuestos):
///
///   - `Int`   → `int`
///   - `Float` → `float`
///   - `Str`   → `str`
///   - `Bool`  → `bool`
///   - `Null`  → `None`
///   - `PyObject(h)` → passthrough con `clone_ref` (refcount bump);
///     un round-trip Fitz→Python→Fitz preserva identidad.
///   - `List<T>` → `list` (copia eager elemento por elemento; cada
///     elemento se marshalla recursivo).
///   - `Map<K, V>` → `dict` (copia eager; las keys deben ser
///     primitivos hashables Python — Int/Float/Str/Bool/Null —
///     porque `dict` requiere `__hash__`).
///   - `Instance { type_name, fields }` → `dict` con field names
///     como keys (traducción nominal). El tipo Fitz se "olvida" del
///     lado Python — recoverlo en el round-trip requiere anotación
///     destino (deuda 8.4).
///
/// Tipos no marshalleables (`Range`, `Function`, `Future`, etc.) →
/// error con `path` que apunta al sitio exacto adentro de la
/// estructura (ej. `arg0.users[2].email`).
///
/// Política "copia eager" (decisión cross-cutting #4 del roadmap):
/// no compartimos estado entre los dos GCs. Una `List<T>` Fitz que
/// va a Python se convierte en una `list` Python independiente; si
/// la `list` Python se muta, la `List<T>` Fitz original no se entera.
fn value_to_py(py: Python<'_>, value: &Value, path: &str) -> FitzResult<Py<PyAny>> {
    use pyo3::IntoPyObject;
    use pyo3::types::{PyDict, PyList};
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

        // Fase 8.2 — compuestos.
        Value::List(items) => {
            // Clonamos el Vec adentro del lock para no mantener el
            // MutexGuard vivo durante la recursión: cada elemento
            // toma su propio lock potencialmente (Lists anidadas).
            let snapshot: Vec<Value> = items.lock().clone();
            let mut py_items: Vec<Py<PyAny>> = Vec::with_capacity(snapshot.len());
            for (i, v) in snapshot.iter().enumerate() {
                let elem_path = format!("{}[{}]", path, i);
                py_items.push(value_to_py(py, v, &elem_path)?);
            }
            let list = PyList::new(py, py_items)
                .map_err(|e| py_err_to_fitz(py, e))?;
            Ok(list.into_any().unbind())
        }
        Value::Map(pairs) => {
            let snapshot: Vec<(Value, Value)> = pairs.lock().clone();
            let dict = PyDict::new(py);
            for (k, v) in snapshot.iter() {
                // Las keys deben ser primitivos hashables Python.
                // Tipos compuestos (List/Map/Instance) no son hashables
                // y romperían `dict.__setitem__`. Detectamos antes de
                // tocar Python para dar mensaje específico.
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
            // Instance se traduce a dict con field names como keys.
            // Si el path arranca vacío (caso top-level, raro porque
            // `call` siempre setea `arg<i>`), usamos `type_name`
            // como prefijo para que el error sea legible.
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

        // Resto (Range, Function/Builtin, Type, Module, HttpResponse,
        // CorsConfig, Future, Result): no son marshalleables. El
        // `Result` se traduce mejor del lado handler HTTP (Ok→200,
        // Err→500); cruzar a Python como objeto no tiene semántica útil.
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "primitivo, compuesto (List/Map/Instance) o PyObject".into(),
                found: other.type_name().into(),
            },
            0, 0,
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

/// Valida que una `key` de `Map` Fitz sea hashable en Python (Int/
/// Float/Str/Bool/Null) y la marshalla. Compuestos como key →
/// error claro, porque Python `dict` exige `__hash__` y List/Map/
/// Instance no lo tienen (igual que `list`/`dict` en Python no son
/// hashables).
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
            0, 0,
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

/// Formatea una `key` de `Map` para usarla como segmento en el
/// breadcrumb del path. `Str("a")` → `"\"a\""`, `Int(42)` → `42`,
/// resto → su `Display`. Solo cosmético — no afecta el marshalling.
fn fmt_map_key(k: &Value) -> String {
    match k {
        Value::Str(s) => format!("\"{}\"", s),
        other => format!("{}", other),
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

    // Fase 8.2.2 — compuestos.
    //
    // `list` Python → `Value::List` (copia eager; cada elemento
    // recursivo via `py_to_value`). El resultado es semánticamente
    // `List<Any>` desde el lado Fitz porque Python no nos da tipo
    // estático; las anotaciones del lado Fitz para refinar a
    // `List<T>` concreto llegan en 8.4.
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

    // `dict` Python → `Value::Map`. CPython 3.7+ garantiza orden de
    // inserción para `dict`; preservarlo nos da paridad bit-a-bit con
    // `serde_json::preserve_order` que ya usa el resto del proyecto.
    // Cada par (key, value) se recursa via `py_to_value` — keys
    // típicamente son primitivos pero permitimos cualquier hashable
    // (Python valida; si la clave es un PyObject opaco, queda como
    // tal). No se auto-coerciona dict → Instance: eso requiere
    // anotación destino del lado Fitz (deuda 8.4).
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

    // Fallback: tipos restantes (función, clase, instancia, submódulo,
    // tuple, set, bytes, etc.) los envolvemos como `Value::PyObject`
    // opaco para que el usuario los pase a otra función Python o haga
    // field access. Tuples/sets/bytes podrían marshallar a List/Map
    // en una fase futura si entra demanda real.
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

    /// Helper post-8.3: el `call` ahora envuelve siempre en `Result`.
    /// Para tests del happy path, desempaquetamos `Ok(inner)` y devolvemos
    /// el `Value` adentro. Si llega `Err(...)`, el test falla con el
    /// mensaje (útil para debugging cuando algo cambia inesperadamente).
    fn ok_inner(v: Value) -> Value {
        match v {
            Value::Result(ResultVariant::Ok(inner)) => *inner,
            Value::Result(ResultVariant::Err(msg)) => {
                panic!("esperaba Ok(...), llegó Err({:?})", msg)
            }
            other => panic!("esperaba Value::Result, fue {:?}", other),
        }
    }

    /// Helper post-8.3: extrae el mensaje de `Err(Str(...))` que produce
    /// un call Python fallido. Si el `Value` no es `Result::Err(Str)`,
    /// el test falla — útil para chequear el formato `"<Class>: <msg>"`
    /// sin asumir el shape.
    fn err_message(v: Value) -> String {
        match v {
            Value::Result(ResultVariant::Err(inner)) => match *inner {
                Value::Str(s) => s,
                other => panic!("Err debería envolver Str, fue {:?}", other),
            },
            Value::Result(ResultVariant::Ok(inner)) => {
                panic!("esperaba Err(...), llegó Ok({:?})", inner)
            }
            other => panic!("esperaba Value::Result, fue {:?}", other),
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
    fn call_str_upper_via_call_no_aplica_es_metodo() {
        // `str.upper("hola")` es válido en Python (unbound method). Usamos
        // este caso para verificar que un argumento Str se marshalla bien.
        let builtins = handle_of(import_module("builtins").unwrap());
        let str_cls = handle_of(get_attr(&builtins, "str").unwrap());
        let upper = handle_of(get_attr(&str_cls, "upper").unwrap());
        let v = ok_inner(call(&upper, &[Value::Str("hola".into())]).unwrap());
        assert_eq!(v, Value::Str("HOLA".into()));
    }

    #[test]
    fn call_arg_int_coerciona_a_pyint() {
        // `abs(-7)` debería darnos `7`. Validamos que Int → int Python.
        let builtins = handle_of(import_module("builtins").unwrap());
        let abs_fn = handle_of(get_attr(&builtins, "abs").unwrap());
        let v = ok_inner(call(&abs_fn, &[Value::Int(-7)]).unwrap());
        assert_eq!(v, Value::Int(7));
    }

    #[test]
    fn call_arg_bool_coerciona_a_pybool() {
        // `bool.__class__.__name__` de True → "bool". Mejor: `int(True)`
        // → 1. Confirmamos que el Bool va como Python bool, no como int.
        let builtins = handle_of(import_module("builtins").unwrap());
        let int_cls = handle_of(get_attr(&builtins, "int").unwrap());
        let v = ok_inner(call(&int_cls, &[Value::Bool(true)]).unwrap());
        assert_eq!(v, Value::Int(1));
    }

    #[test]
    fn call_arg_null_coerciona_a_none() {
        // `str(None)` da "None". Verifica que Null → Python None.
        let builtins = handle_of(import_module("builtins").unwrap());
        let str_cls = handle_of(get_attr(&builtins, "str").unwrap());
        let v = ok_inner(call(&str_cls, &[Value::Null]).unwrap());
        assert_eq!(v, Value::Str("None".into()));
    }

    #[test]
    fn call_excepcion_python_envuelve_en_result_err() {
        // 8.3: `math.sqrt(-1)` lanza ValueError en Python. El call no
        // aborta — devuelve `Value::Result(Err(Str("ValueError: ...")))`.
        // El usuario tiene que manejarlo con `match` o `?`.
        let math = handle_of(import_module("math").unwrap());
        let sqrt = handle_of(get_attr(&math, "sqrt").unwrap());
        let v = call(&sqrt, &[Value::Float(-1.0)]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.contains("ValueError"),
            "mensaje debería citar ValueError, fue: {}",
            msg,
        );
    }

    #[test]
    fn call_arg_no_marshalleable_envuelve_en_result_err() {
        // 8.3: Range no es marshalleable. En vez de abortar con
        // FitzError, el error se envuelve en `Result::Err(Str(...))`
        // — uniformidad: TODO error del path call se ve como `Err` para
        // el usuario, sea excepción Python o marshaling fail.
        let math = handle_of(import_module("math").unwrap());
        let sqrt = handle_of(get_attr(&math, "sqrt").unwrap());
        let v = call(&sqrt, &[Value::Range { start: 0, end: 10 }]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.contains("Range") && msg.contains("arg0"),
            "mensaje debería citar Range + arg0, fue: {}",
            msg,
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
        let id1 = ok_inner(call(&id_fn, std::slice::from_ref(&math)).unwrap());
        let id2 = ok_inner(call(&id_fn, &[math]).unwrap());
        assert_eq!(id1, id2);
    }

    // -------------------------------------------------------------------
    // 8.2.1 — value_to_py para List/Map/Instance (Fitz → Python)
    // -------------------------------------------------------------------

    #[test]
    fn list_de_ints_se_marshalla_a_list_python() {
        // `json.dumps([1, 2, 3])` → "[1, 2, 3]". El round-trip vía
        // json valida que la list Python que produjimos tiene los
        // elementos correctos en orden.
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
        // Python permite mezcla en una list: `[1, "dos", true]`.
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let list = Value::new_list(vec![
            Value::Int(1),
            Value::Str("dos".into()),
            Value::Bool(true),
        ]);
        let v = ok_inner(call(&dumps, &[list]).unwrap());
        // JSON serializa true como `true`. Confirma que Bool Fitz
        // cruza como bool Python (no como int).
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
    fn map_de_str_a_int_se_marshalla_a_dict() {
        // `json.dumps({"a": 1, "b": 2})` → '{"a": 1, "b": 2}'.
        // PyDict preserva el orden de inserción (Python 3.7+), igual
        // que `serde_json::preserve_order` que ya usa el resto del
        // proyecto.
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
    fn map_con_keys_no_hashables_es_error_con_path() {
        // 8.3: List como key → `Result::Err(Str)` con mensaje que cita
        // el path "arg0" y la restricción "hashable".
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
    fn instance_se_marshalla_a_dict_por_field_name() {
        // Una `Instance` con type_name="User" y fields ordenados
        // {id: 1, name: "x"} → `{"id": 1, "name": "x"}` después de
        // `json.dumps`. Verifica que el orden de campos se preserva
        // (PyDict en CPython 3.7+).
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
    fn list_de_instances_se_marshalla() {
        // Caso pre-canónico del roadmap: `List<User>` pasado a una
        // función Python. La lista se marshalla a list[dict].
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
                  {\"id\": 2, \"email\": \"b@x.com\"}]".into()
            ),
        );
    }

    #[test]
    fn null_dentro_de_list_se_marshalla_a_none() {
        let json = handle_of(import_module("json").unwrap());
        let dumps = handle_of(get_attr(&json, "dumps").unwrap());
        let list = Value::new_list(vec![Value::Int(1), Value::Null, Value::Int(3)]);
        let v = ok_inner(call(&dumps, &[list]).unwrap());
        assert_eq!(v, Value::Str("[1, null, 3]".into()));
    }

    #[test]
    fn elemento_no_marshalleable_en_list_es_error_con_path() {
        // 8.3: Range adentro de list → `Result::Err(Str)` con path
        // "arg0[1]" en el mensaje.
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
    fn json_loads_de_array_devuelve_list() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(call(&loads, &[Value::Str("[1, 2, 3]".into())]).unwrap());
        assert_eq!(
            v,
            Value::new_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        );
    }

    #[test]
    fn json_loads_de_array_vacio_devuelve_list_vacia() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(call(&loads, &[Value::Str("[]".into())]).unwrap());
        assert_eq!(v, Value::new_list(vec![]));
    }

    #[test]
    fn json_loads_de_objeto_devuelve_map_con_orden_de_insercion() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(
            call(&loads, &[Value::Str("{\"a\": 1, \"b\": 2}".into())]).unwrap()
        );
        // Python 3.7+ garantiza orden de inserción para dict;
        // verificamos que llega en el orden serializado del JSON.
        assert_eq!(
            v,
            Value::new_map(vec![
                (Value::Str("a".into()), Value::Int(1)),
                (Value::Str("b".into()), Value::Int(2)),
            ]),
        );
    }

    #[test]
    fn json_loads_de_array_heterogeneo() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(
            call(&loads, &[Value::Str("[1, \"dos\", true, null]".into())]).unwrap()
        );
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
    fn json_loads_de_array_anidado() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(
            call(&loads, &[Value::Str("[[1, 2], [3, 4]]".into())]).unwrap()
        );
        assert_eq!(
            v,
            Value::new_list(vec![
                Value::new_list(vec![Value::Int(1), Value::Int(2)]),
                Value::new_list(vec![Value::Int(3), Value::Int(4)]),
            ]),
        );
    }

    #[test]
    fn json_loads_de_dict_de_dict() {
        let json = handle_of(import_module("json").unwrap());
        let loads = handle_of(get_attr(&json, "loads").unwrap());
        let v = ok_inner(call(&loads, &[Value::Str(
            "{\"user\": {\"id\": 1, \"name\": \"x\"}}".into()
        )]).unwrap());
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
        // dumps + loads → la lista debería volver igual.
        // 8.3: cada call envuelve en Ok, así que tenemos que
        // desempaquetar el `s` antes de pasarlo al siguiente call.
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
    fn pylist_directo_de_python_se_coerce_a_list() {
        // `list("abc")` en Python da `['a', 'b', 'c']`. Cruzamos un
        // PyList vivo, no un JSON deserializado, para validar que el
        // dispatch sobre PyList funciona aunque venga de cualquier API.
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
    fn pydict_directo_de_python_se_coerce_a_map() {
        // `dict(zip(["a", "b"], [1, 2]))` en Python da `{"a": 1, "b": 2}`.
        // Validamos un dict construido en runtime, no un JSON loads.
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
    fn campo_no_marshalleable_en_instance_es_error_con_path() {
        // 8.3: Range como valor de un field → `Result::Err(Str)` con
        // path "arg0.User.<field>" o similar.
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
                && msg.contains("range")  // el field se llama "range"
                && msg.contains("arg0"),
            "msg: {}",
            msg,
        );
    }

    // -------------------------------------------------------------------
    // 8.3 — Wrap automático en Result<T> + formato de error
    // -------------------------------------------------------------------

    #[test]
    fn call_exitoso_devuelve_value_result_ok() {
        // Validación explícita del shape: el `Value` que devuelve `call`
        // siempre es `Value::Result(Ok(...))` para éxito (no
        // `Value::Float` directo). Confirma el invariante del 8.3.
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
    fn call_jsonloads_malformado_es_err_con_jsondecodeerror() {
        // Criterio textual del roadmap 8.3:
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
            "mensaje del Err debería citar JSONDecodeError, fue: {}",
            msg,
        );
    }

    #[test]
    fn call_typeerror_python_se_envuelve_no_aborta() {
        // `int("no es un número")` lanza ValueError. El call no
        // aborta — el Err contiene el mensaje legible.
        let builtins = handle_of(import_module("builtins").unwrap());
        let int_cls = handle_of(get_attr(&builtins, "int").unwrap());
        let v = call(&int_cls, &[Value::Str("no es un número".into())]).unwrap();
        let msg = err_message(v);
        assert!(
            msg.starts_with("ValueError:"),
            "mensaje debería empezar con `ValueError:`, fue: {}",
            msg,
        );
    }

    #[test]
    fn call_formato_err_es_classname_dos_puntos_message() {
        // El formato canónico `<ClassName>: <message>` queda estable
        // bit-a-bit entre 8.1 y 8.3 (solo cambia el envoltorio:
        // FitzError → Value::Result(Err(Str))). Tests futuros que
        // dependan del formato exacto se apoyan en esto.
        let builtins = handle_of(import_module("builtins").unwrap());
        let int_cls = handle_of(get_attr(&builtins, "int").unwrap());
        let v = call(&int_cls, &[Value::Str("zz".into())]).unwrap();
        let msg = err_message(v);
        // Forma esperada: "ValueError: invalid literal for int() with base 10: 'zz'"
        let parts: Vec<&str> = msg.splitn(2, ": ").collect();
        assert_eq!(parts.len(), 2, "esperaba `<ClassName>: <message>`, fue: {}", msg);
        assert_eq!(parts[0], "ValueError");
        assert!(!parts[1].is_empty(), "message body vacío");
    }
}
