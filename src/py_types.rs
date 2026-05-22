// py_types.rs — Fase 8.5: auto-mapeo de modelos SQLAlchemy a `type`
// de Fitz.
//
// El sub-comando `fitz py-types <archivo.py> [--out <archivo.fitz>]`
// invoca `generate_from_file(path)` que:
//
//   1. Toma el GIL via `Python::attach`.
//   2. Importa el archivo Python como módulo dinámico via
//      `importlib.util.spec_from_file_location` + `module_from_spec`.
//   3. Itera el `__dict__` del módulo buscando clases (top-level) con
//      atributo `__table__.columns`. Duck typing: compatible con
//      SQLAlchemy real (`DeclarativeBase` subclasses) y con mocks que
//      cumplan el mismo contract (`column.name`, `column.type`,
//      `column.nullable`, `column.default`).
//   4. Por cada clase, emite un bloque `type ClassName { f1: T1, ... }`
//      siguiendo el mapping del roadmap.
//
// Decisiones de diseño (alineadas con roadmap 8.5):
//
//   - **In-process via PyO3** (no subprocess). Reusa el GIL + dep
//     PyO3 ya disponible con `--features python`. Sin feature el
//     sub-comando aborta antes de llegar acá.
//   - **Duck typing** (`__table__.columns`) en vez de `isinstance`
//     contra `DeclarativeBase`. Permite tests con mocks sin requerir
//     SQLAlchemy real instalado; funciona igual con SQLAlchemy real.
//   - **Solo top-level del archivo** importado. Modelos heredados
//     de bases externas (`from app.models_base import Base`) se
//     detectan por el shape, no por la herencia.
//   - **Mapping conservador**: tipos primitivos directos, DateTime
//     → Str (ISO 8601), desconocidos → `Any` con comentario `// ?`
//     para que el usuario los marque a mano.
//   - **Defaults solo literales** (Int/Float/Str/Bool/None). Defaults
//     callable (`default=datetime.utcnow`) se ignoran silenciosamente
//     — emitir `= func()` no aporta y puede confundir.

use std::path::Path;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString};

/// Punto de entrada. Importa `source` con PyO3 + introspecciona +
/// genera el texto Fitz. Devuelve el texto generado o un error
/// legible para el caller (que decide stdout vs archivo).
pub fn generate_from_file(source: &Path) -> Result<String, String> {
    let abs = source
        .canonicalize()
        .map_err(|e| format!("no se pudo resolver el path `{}`: {}", source.display(), e))?;
    let abs_str = abs.to_string_lossy().to_string();
    let module_name = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module")
        .to_string();

    Python::attach(|py| {
        let module = import_file_as_module(py, &abs_str, &module_name)
            .map_err(|e| pyerr_to_string(py, e))?;
        let models = collect_models(py, &module).map_err(|e| pyerr_to_string(py, e))?;
        let mut out = String::new();
        out.push_str("// Generado por `fitz py-types` — no editar a mano.\n");
        out.push_str(&format!("// Fuente: {}\n\n", source.display(),));
        for model in &models {
            emit_type(model, &mut out);
            out.push('\n');
        }
        if models.is_empty() {
            return Err(format!(
                "el archivo `{}` no expone ninguna clase con `__table__.columns` \
                 (esperado: modelos SQLAlchemy o mocks con ese shape)",
                source.display()
            ));
        }
        Ok(out)
    })
}

/// Importa un archivo Python como módulo dinámico usando
/// `importlib.util`. El nombre del módulo es solo cosmético — lo
/// importante es la ruta absoluta. La importación ejecuta el código
/// top-level (definiciones de clases, etc.), así que si el archivo
/// tiene side-effects pesados (conexiones DB, etc.) van a correr.
fn import_file_as_module<'py>(
    py: Python<'py>,
    abs_path: &str,
    module_name: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let importlib_util = py.import("importlib.util")?;
    let spec = importlib_util.call_method1("spec_from_file_location", (module_name, abs_path))?;
    if spec.is_none() {
        return Err(pyo3::exceptions::PyImportError::new_err(format!(
            "no se pudo armar el spec de import para `{}`",
            abs_path
        )));
    }
    let module = importlib_util.call_method1("module_from_spec", (&spec,))?;
    let loader = spec.getattr("loader")?;
    loader.call_method1("exec_module", (&module,))?;
    Ok(module)
}

/// Recolecta los modelos del módulo importado. Un "modelo" es
/// cualquier clase top-level con atributo `__table__.columns`.
fn collect_models<'py>(py: Python<'py>, module: &Bound<'py, PyAny>) -> PyResult<Vec<Model>> {
    let module_name: String = module.getattr("__name__")?.extract()?;
    let dict = module.getattr("__dict__")?;
    let dict = dict.cast::<PyDict>()?;
    let mut models: Vec<Model> = Vec::new();
    for (name_obj, value_obj) in dict.iter() {
        let name: String = match name_obj.cast::<PyString>() {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };
        // Filtrar dunders.
        if name.starts_with('_') {
            continue;
        }
        // Tiene que ser una clase.
        let builtins = py.import("builtins")?;
        let isinstance: bool = builtins
            .call_method1("isinstance", (&value_obj, builtins.getattr("type")?))?
            .extract()?;
        if !isinstance {
            continue;
        }
        // Solo clases definidas EN este módulo (filtra los re-exports
        // de SQLAlchemy mismo: `Base`, `Column`, `Integer`, etc.).
        let cls_module: String = match value_obj.getattr("__module__") {
            Ok(m) => m.extract().unwrap_or_default(),
            Err(_) => continue,
        };
        if cls_module != module_name {
            continue;
        }
        // Duck typing: ¿tiene __table__.columns?
        let table = match value_obj.getattr("__table__") {
            Ok(t) => t,
            Err(_) => continue,
        };
        let columns = match table.getattr("columns") {
            Ok(c) => c,
            Err(_) => continue,
        };
        let fields = collect_fields(py, &columns)?;
        models.push(Model { name, fields });
    }
    Ok(models)
}

/// Recolecta los campos de la colección `columns` de una tabla. La
/// colección debe ser iterable y dar objetos con
/// `(name, type, nullable, default)`.
fn collect_fields<'py>(py: Python<'py>, columns: &Bound<'py, PyAny>) -> PyResult<Vec<Field>> {
    let iter = columns.try_iter()?;
    let mut fields: Vec<Field> = Vec::new();
    for col in iter {
        let col = col?;
        let name: String = col.getattr("name")?.extract()?;
        let py_type_obj = col.getattr("type")?;
        let fitz_type = python_type_to_fitz_type(py, &py_type_obj)?;
        let nullable: bool = col
            .getattr("nullable")
            .and_then(|v| v.extract::<bool>())
            .unwrap_or(false);
        let default = extract_default(py, &col)?;
        fields.push(Field {
            name,
            fitz_type,
            nullable,
            default,
        });
    }
    Ok(fields)
}

/// Mapea un tipo SQLAlchemy a un nombre de tipo Fitz. La inspección
/// es por nombre de clase del objeto `Column.type` para no requerir
/// importar sqlalchemy directamente. Tipos desconocidos producen
/// `Any` (con `// ?` en el output como pista al usuario).
fn python_type_to_fitz_type<'py>(
    _py: Python<'py>,
    py_type: &Bound<'py, PyAny>,
) -> PyResult<FitzType> {
    let cls = py_type.get_type();
    let cls_name: String = cls.name()?.to_string();
    // El mapping va por el nombre canónico de la clase SQLAlchemy.
    // Soporta tanto `Column(Integer)` (donde `type` es una instancia
    // de `Integer`) como type-as-class (raro pero posible).
    let mapped = match cls_name.as_str() {
        "Integer" | "BigInteger" | "SmallInteger" | "INTEGER" | "BIGINT" | "SMALLINT" => {
            FitzType::Int
        }
        "Float" | "Numeric" | "Double" | "REAL" | "FLOAT" | "NUMERIC" => FitzType::Float,
        "String" | "Text" | "Unicode" | "VARCHAR" | "TEXT" | "CHAR" | "CLOB" => FitzType::Str,
        "Boolean" | "BOOLEAN" => FitzType::Bool,
        "DateTime" | "Date" | "Time" | "TIMESTAMP" | "DATE" | "TIME" => FitzType::Str,
        other => FitzType::Unknown(other.to_string()),
    };
    Ok(mapped)
}

/// Extrae el valor del default si es un literal simple (Int/Float/
/// Str/Bool/None). Defaults callable (`default=datetime.utcnow`) se
/// ignoran silenciosamente. SQLAlchemy envuelve los defaults en
/// `ColumnDefault(arg=<valor>)` — accedemos a `.arg` y filtramos
/// callables con `inspect.isfunction`/`callable`.
fn extract_default<'py>(
    py: Python<'py>,
    column: &Bound<'py, PyAny>,
) -> PyResult<Option<FitzLiteral>> {
    let default = match column.getattr("default") {
        Ok(d) if !d.is_none() => d,
        _ => return Ok(None),
    };
    // SQLAlchemy: `default` puede ser un `ColumnDefault` con `.arg`,
    // o un literal directo. Probamos ambos.
    let value = match default.getattr("arg") {
        Ok(arg) => arg,
        Err(_) => default,
    };
    // Si es callable, lo ignoramos.
    let builtins = py.import("builtins")?;
    let is_callable: bool = builtins.call_method1("callable", (&value,))?.extract()?;
    if is_callable {
        return Ok(None);
    }
    if value.is_none() {
        return Ok(Some(FitzLiteral::Null));
    }
    // Tipo Python → literal Fitz. Cuidado: bool antes que int.
    if let Ok(b) = value.extract::<bool>() {
        let bool_type = builtins.getattr("bool")?;
        let is_bool: bool = builtins
            .call_method1("isinstance", (&value, &bool_type))?
            .extract()?;
        if is_bool {
            return Ok(Some(FitzLiteral::Bool(b)));
        }
    }
    if let Ok(n) = value.extract::<i64>() {
        return Ok(Some(FitzLiteral::Int(n)));
    }
    if let Ok(f) = value.extract::<f64>() {
        return Ok(Some(FitzLiteral::Float(f)));
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(Some(FitzLiteral::Str(s)));
    }
    // Tipo no representable como literal Fitz — lo ignoramos.
    Ok(None)
}

/// Emite un bloque `type Name { ... }` con los fields.
fn emit_type(model: &Model, out: &mut String) {
    out.push_str(&format!("type {} {{\n", model.name));
    let n = model.fields.len();
    for (i, f) in model.fields.iter().enumerate() {
        let ty_str = match &f.fitz_type {
            FitzType::Int => "Int".to_string(),
            FitzType::Float => "Float".to_string(),
            FitzType::Str => "Str".to_string(),
            FitzType::Bool => "Bool".to_string(),
            FitzType::Unknown(_) => "Any".to_string(),
        };
        let ty_str = if f.nullable {
            format!("{}?", ty_str)
        } else {
            ty_str
        };
        let default_str = match &f.default {
            Some(lit) => format!(" = {}", emit_literal(lit)),
            None => String::new(),
        };
        // Comentario para tipos desconocidos: pista para el usuario.
        let comment = match &f.fitz_type {
            FitzType::Unknown(name) => format!("  // ? tipo SQLAlchemy `{}` mapeado a Any", name),
            _ => String::new(),
        };
        let sep = if i + 1 < n { "," } else { "" };
        out.push_str(&format!(
            "    {}: {}{}{}{}\n",
            f.name, ty_str, default_str, sep, comment,
        ));
    }
    out.push_str("}\n");
}

fn emit_literal(lit: &FitzLiteral) -> String {
    match lit {
        FitzLiteral::Int(n) => n.to_string(),
        FitzLiteral::Float(f) => {
            if f.fract() == 0.0 && f.is_finite() {
                format!("{:.1}", f)
            } else {
                f.to_string()
            }
        }
        FitzLiteral::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        FitzLiteral::Bool(b) => b.to_string(),
        FitzLiteral::Null => "null".to_string(),
    }
}

fn pyerr_to_string(py: Python<'_>, err: PyErr) -> String {
    let class = err
        .get_type(py)
        .qualname()
        .ok()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "PyError".to_string());
    let msg = err.value(py).to_string();
    if msg.is_empty() {
        class
    } else {
        format!("{}: {}", class, msg)
    }
}

/// Suprime warnings de unused — usado solo en el path de
/// introspección que itera el `__dict__` del módulo.
#[allow(dead_code)]
fn _suppress_unused(_pl: &PyList) {}

// ---------------------------------------------------------------------------
// Datos internos
// ---------------------------------------------------------------------------

struct Model {
    name: String,
    fields: Vec<Field>,
}

struct Field {
    name: String,
    fitz_type: FitzType,
    nullable: bool,
    default: Option<FitzLiteral>,
}

enum FitzType {
    Int,
    Float,
    Str,
    Bool,
    Unknown(String),
}

enum FitzLiteral {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Helper: escribe `code` Python a un archivo temporal y corre
    /// `generate_from_file` contra él. Devuelve el output emitido o
    /// el mensaje de error.
    fn run(code: &str) -> Result<String, String> {
        let mut f = NamedTempFile::with_suffix(".py").expect("tempfile");
        f.write_all(code.as_bytes()).expect("write");
        f.flush().expect("flush");
        generate_from_file(f.path())
    }

    /// Mock de un modelo SQLAlchemy. Cumple el contract `__table__.columns`
    /// que la introspección espera, sin requerir SQLAlchemy instalado.
    const MOCK_BOILERPLATE: &str = "\
class Column:
    def __init__(self, type_, nullable=False, default=None):
        self.type = type_
        self.nullable = nullable
        self.default = default

class _Columns:
    def __init__(self, items):
        self._items = items
        for c in items:
            # Hack para que el iter dé el nombre: el caller setea
            # `name` antes de meter la Column en _Columns.
            pass
    def __iter__(self):
        return iter(self._items)

class _Table:
    def __init__(self, columns):
        self.columns = _Columns(columns)

def _named(name, col):
    col.name = name
    return col

class Integer: pass
class BigInteger: pass
class String: pass
class Float: pass
class Boolean: pass
class DateTime: pass
";

    #[test]
    fn modelo_simple_emite_type_con_fields_primitivos() {
        let code = format!(
            "{}\nclass User:\n    __table__ = _Table([\n        _named('id', Column(Integer())),\n        _named('email', Column(String())),\n    ])\n",
            MOCK_BOILERPLATE
        );
        let out = run(&code).expect("generate ok");
        assert!(out.contains("type User {"), "out:\n{}", out);
        assert!(out.contains("id: Int"), "out:\n{}", out);
        assert!(out.contains("email: Str"), "out:\n{}", out);
    }

    #[test]
    fn mapping_de_tipos_primitivos() {
        let code = format!(
            "{}\nclass T:\n    __table__ = _Table([\n        _named('a', Column(Integer())),\n        _named('b', Column(BigInteger())),\n        _named('c', Column(Float())),\n        _named('d', Column(String())),\n        _named('e', Column(Boolean())),\n        _named('f', Column(DateTime())),\n    ])\n",
            MOCK_BOILERPLATE
        );
        let out = run(&code).expect("generate ok");
        assert!(out.contains("a: Int"));
        assert!(out.contains("b: Int"));
        assert!(out.contains("c: Float"));
        assert!(out.contains("d: Str"));
        assert!(out.contains("e: Bool"));
        assert!(out.contains("f: Str")); // DateTime → Str ISO 8601
    }

    #[test]
    fn nullable_anota_con_sufijo_pregunta() {
        let code = format!(
            "{}\nclass T:\n    __table__ = _Table([\n        _named('a', Column(Integer(), nullable=True)),\n        _named('b', Column(String(), nullable=False)),\n    ])\n",
            MOCK_BOILERPLATE
        );
        let out = run(&code).expect("generate ok");
        assert!(out.contains("a: Int?"), "out:\n{}", out);
        assert!(out.contains("b: Str"), "out:\n{}", out);
        assert!(!out.contains("b: Str?"), "b no debería ser nullable");
    }

    #[test]
    fn default_literal_se_emite_inline() {
        let code = format!(
            "{}\nclass T:\n    __table__ = _Table([\n        _named('a', Column(Integer(), default=42)),\n        _named('b', Column(String(), default='hola')),\n        _named('c', Column(Boolean(), default=True)),\n    ])\n",
            MOCK_BOILERPLATE
        );
        let out = run(&code).expect("generate ok");
        assert!(out.contains("a: Int = 42"), "out:\n{}", out);
        assert!(out.contains("b: Str = \"hola\""), "out:\n{}", out);
        assert!(out.contains("c: Bool = true"), "out:\n{}", out);
    }

    #[test]
    fn default_callable_se_ignora() {
        // `default=callable` es común en SQLAlchemy (`default=datetime.utcnow`);
        // emitir `= func()` no aporta, lo ignoramos.
        let code = format!(
            "{}\nimport datetime\nclass T:\n    __table__ = _Table([\n        _named('created_at', Column(DateTime(), default=datetime.datetime.utcnow)),\n    ])\n",
            MOCK_BOILERPLATE
        );
        let out = run(&code).expect("generate ok");
        assert!(out.contains("created_at: Str"), "out:\n{}", out);
        assert!(
            !out.contains("created_at: Str ="),
            "default callable debería ignorarse"
        );
    }

    #[test]
    fn tipo_desconocido_cae_a_any_con_comentario() {
        let code = format!(
            "{}\nclass JSON: pass\nclass T:\n    __table__ = _Table([\n        _named('payload', Column(JSON())),\n    ])\n",
            MOCK_BOILERPLATE
        );
        let out = run(&code).expect("generate ok");
        assert!(out.contains("payload: Any"), "out:\n{}", out);
        assert!(
            out.contains("// ?"),
            "esperaba comentario citando tipo SQLA, out:\n{}",
            out
        );
        assert!(
            out.contains("JSON"),
            "comentario debería citar el nombre original"
        );
    }

    #[test]
    fn varios_modelos_emiten_varios_types() {
        let code = format!(
            "{}\nclass User:\n    __table__ = _Table([_named('id', Column(Integer()))])\nclass Order:\n    __table__ = _Table([_named('id', Column(Integer())), _named('total', Column(Float()))])\n",
            MOCK_BOILERPLATE
        );
        let out = run(&code).expect("generate ok");
        assert!(out.contains("type User {"));
        assert!(out.contains("type Order {"));
        assert!(out.contains("total: Float"));
    }

    #[test]
    fn archivo_sin_modelos_es_error_claro() {
        let code = "x = 1\n";
        let err = run(code).expect_err("no debería haber modelos");
        assert!(
            err.contains("ninguna clase") || err.contains("__table__"),
            "msg: {}",
            err,
        );
    }

    #[test]
    fn clases_sin_table_attribute_se_ignoran() {
        // `Helper` no tiene `__table__` — debe filtrarse.
        let code = format!(
            "{}\nclass Helper:\n    def hello(self): pass\nclass User:\n    __table__ = _Table([_named('id', Column(Integer()))])\n",
            MOCK_BOILERPLATE
        );
        let out = run(&code).expect("generate ok");
        assert!(out.contains("type User {"));
        assert!(!out.contains("type Helper {"), "Helper no debería emitirse");
    }

    #[test]
    fn header_cita_archivo_fuente() {
        let code = format!(
            "{}\nclass User:\n    __table__ = _Table([_named('id', Column(Integer()))])\n",
            MOCK_BOILERPLATE
        );
        let out = run(&code).expect("generate ok");
        assert!(out.starts_with("// Generado por `fitz py-types`"));
        assert!(out.contains("// Fuente:"));
    }
}
