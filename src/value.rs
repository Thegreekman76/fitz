// value.rs — Fase 2.4
//
// Representación de valores en runtime. Un programa Fitz se evalúa a un árbol
// de `Value`s. Esta es la moneda con la que opera el evaluador.
//
// Notas de diseño:
//  - Los floats e ints se promueven mutuamente en operaciones (1 + 1.0 == 2.0)
//    igual que en Python. Acá solo definimos los datos; la promoción la hace
//    el evaluador.
//  - `Display` muestra los valores como los vería el usuario al hacer `print`.
//    Strings van sin comillas, floats siempre con `.0` si no tienen decimales
//    (para distinguirlos visualmente de ints).
//  - `PartialEq` está implementado a mano porque la comparación entre Int y
//    Float requiere coerción (1 == 1.0 → true). Si lo derivamos, esa igualdad
//    daría false.
//  - `Value::Function` guarda un handle (`EnvRef`) al environment donde la
//    función fue definida. Esto crea una dependencia mutua value↔env, pero
//    Rust la acepta porque `Rc<RefCell<>>` es una indirección: el tamaño
//    de `Value` no depende del tamaño de `Environment`.

use crate::ast::{Field, Param, Stmt};
use crate::env::EnvRef;
use crate::error::FitzResult;

/// Un valor en runtime.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,

    /// Función nativa implementada en Rust (ej: `print`).
    /// La firma recibe los args ya evaluados y devuelve un valor o error.
    Builtin {
        name: &'static str,
        func: fn(&[Value]) -> FitzResult<Value>,
    },

    /// Función definida por el usuario. Guarda sus parámetros, su cuerpo,
    /// y un handle al env donde fue definida. Ese handle es el "closure":
    /// al llamar la función creamos un scope hijo de ese env, no del caller.
    /// Eso le da acceso a las variables del lugar donde se definió.
    Function {
        params: Vec<Param>,
        body: Vec<Stmt>,
        closure: EnvRef,
    },

    /// Tipo custom definido por el usuario (`type User { id: Int }`).
    /// Por ahora es un marcador inerte: existe en el env para que el nombre
    /// del tipo pueda resolverse, pero sin struct literals no se puede
    /// instanciar. Se vuelve útil en Fase 3 (instanciación, field access).
    Type {
        name: String,
        fields: Vec<Field>,
    },

    /// Lista en runtime. `Vec<Value>` ordenada y mutable internamente,
    /// pero por ahora los programas Fitz solo la construyen y consumen
    /// (mutación por method calls llega en el paso 4 de Fase 3).
    List(Vec<Value>),

    /// Mapa en runtime. `Vec<(K, V)>` en vez de `HashMap` por dos razones:
    ///  - preserva el orden de inserción (importa para `print` y para
    ///    iteración futura).
    ///  - acepta claves no-hash sin complicar `Value`. Acceso es O(n);
    ///    optimizable más adelante cuando importe.
    Map(Vec<(Value, Value)>),

    /// Rango exclusivo de Int. Iterable. Por ahora solo Int (Float
    /// no tiene una semántica discreta clara para iteración).
    Range { start: i64, end: i64 },
}

impl Value {
    /// Nombre del tipo, para mensajes de error.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Str(_) => "Str",
            Value::Bool(_) => "Bool",
            Value::Null => "Null",
            Value::Builtin { .. } => "Function",
            Value::Function { .. } => "Function",
            Value::Type { .. } => "Type",
            Value::List(_) => "List",
            Value::Map(_) => "Map",
            Value::Range { .. } => "Range",
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(x) => {
                // Si no tiene parte decimal, agregamos `.0` para que se vea
                // distinto a un Int. `3.0` → "3.0", `3.14` → "3.14".
                if x.fract() == 0.0 && x.is_finite() {
                    write!(f, "{:.1}", x)
                } else {
                    write!(f, "{}", x)
                }
            }
            Value::Str(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Null => write!(f, "null"),
            Value::Builtin { name, .. } => write!(f, "<builtin {}>", name),
            Value::Function { .. } => write!(f, "<function>"),
            Value::Type { name, .. } => write!(f, "<type {}>", name),
            Value::List(items) => {
                // Para strings, mostramos comillas adentro de la lista
                // (es la representación, no salida directa de `print`).
                // Ej: `[1, "hola", 2]`. Distinto del Display de `Str`
                // suelto, que va sin comillas porque ese caso es para
                // salida final.
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
            Value::Range { start, end } => write!(f, "{}..{}", start, end),
        }
    }
}

/// Representación "inline" de un Value cuando aparece adentro de otro
/// (lista, mapa). Los strings llevan comillas para distinguir
/// `[1, "2"]` de `[1, 2]` — es lectura, no impresión final. El resto
/// delega en `Display` normal.
fn write_inline_value(f: &mut std::fmt::Formatter, v: &Value) -> std::fmt::Result {
    match v {
        Value::Str(s) => write!(f, "\"{}\"", s),
        other => write!(f, "{}", other),
    }
}

/// Igualdad con coerción Int↔Float. El resto de combinaciones devuelven false.
/// Esto define la semántica de `==` en Fitz (la usa el evaluador en BinOp::Eq).
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
            // List y Map se comparan estructuralmente, elemento a elemento.
            // La igualdad recursiva delega en esta misma impl, así que Int↔Float
            // coerciona también adentro de listas y mapas.
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            (
                Value::Range { start: s1, end: e1 },
                Value::Range { start: s2, end: e2 },
            ) => s1 == s2 && e1 == e2,
            // Funciones no se comparan por valor — siempre desiguales.
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_int_sin_decimales() {
        assert_eq!(Value::Int(42).to_string(), "42");
        assert_eq!(Value::Int(-7).to_string(), "-7");
    }

    #[test]
    fn display_float_entero_lleva_punto_cero() {
        assert_eq!(Value::Float(3.0).to_string(), "3.0");
        assert_eq!(Value::Float(-0.0).to_string(), "-0.0");
    }

    #[test]
    fn display_float_con_decimales_se_muestra_normal() {
        assert_eq!(Value::Float(3.14).to_string(), "3.14");
    }

    #[test]
    fn display_str_sin_comillas() {
        // print("hola") debe mostrar `hola`, no `"hola"`.
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
    fn type_name_devuelve_el_nombre_del_tipo() {
        assert_eq!(Value::Int(0).type_name(), "Int");
        assert_eq!(Value::Float(0.0).type_name(), "Float");
        assert_eq!(Value::Str("".into()).type_name(), "Str");
        assert_eq!(Value::Bool(false).type_name(), "Bool");
        assert_eq!(Value::Null.type_name(), "Null");
    }

    #[test]
    fn igualdad_int_y_float_se_coerciona() {
        // En Fitz, `1 == 1.0` es true. Esto refleja la promoción Int↔Float.
        assert_eq!(Value::Int(1), Value::Float(1.0));
        assert_eq!(Value::Float(2.0), Value::Int(2));
    }

    #[test]
    fn igualdad_entre_tipos_distintos_es_false() {
        assert_ne!(Value::Int(1), Value::Str("1".into()));
        assert_ne!(Value::Bool(true), Value::Int(1));
        assert_ne!(Value::Null, Value::Bool(false));
    }

    #[test]
    fn igualdad_null_consigo_mismo() {
        assert_eq!(Value::Null, Value::Null);
    }

    #[test]
    fn igualdad_strings() {
        assert_eq!(Value::Str("hola".into()), Value::Str("hola".into()));
        assert_ne!(Value::Str("hola".into()), Value::Str("chau".into()));
    }

    // -----------------------------------------------------------------------
    // Tests — List, Map, Range (Fase 3, paso 1)
    // -----------------------------------------------------------------------

    #[test]
    fn display_list_vacia() {
        assert_eq!(Value::List(vec![]).to_string(), "[]");
    }

    #[test]
    fn display_list_con_ints() {
        let v = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        assert_eq!(v.to_string(), "[1, 2, 3]");
    }

    #[test]
    fn display_list_strings_van_con_comillas_dentro() {
        // Strings sueltos van sin comillas (print), pero adentro de
        // una lista llevan comillas para que se distinga `1` de `"1"`.
        let v = Value::List(vec![
            Value::Int(1),
            Value::Str("hola".into()),
            Value::Bool(true),
        ]);
        assert_eq!(v.to_string(), "[1, \"hola\", true]");
    }

    #[test]
    fn display_list_anidada() {
        let inner = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let outer = Value::List(vec![inner.clone(), inner]);
        assert_eq!(outer.to_string(), "[[1, 2], [1, 2]]");
    }

    #[test]
    fn display_map_vacio() {
        assert_eq!(Value::Map(vec![]).to_string(), "{}");
    }

    #[test]
    fn display_map_preserva_orden_y_comillas_en_strings() {
        let m = Value::Map(vec![
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
    fn display_range_negativo() {
        assert_eq!(Value::Range { start: -5, end: 5 }.to_string(), "-5..5");
    }

    #[test]
    fn type_name_de_list_map_range() {
        assert_eq!(Value::List(vec![]).type_name(), "List");
        assert_eq!(Value::Map(vec![]).type_name(), "Map");
        assert_eq!(Value::Range { start: 0, end: 1 }.type_name(), "Range");
    }

    #[test]
    fn igualdad_list_estructural() {
        let a = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let c = Value::List(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn igualdad_list_coerciona_int_float_adentro() {
        // [1, 2] == [1.0, 2.0] — la coerción Int↔Float vale adentro de listas.
        let a = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::List(vec![Value::Float(1.0), Value::Float(2.0)]);
        assert_eq!(a, b);
    }

    #[test]
    fn igualdad_map_estructural() {
        let a = Value::Map(vec![(Value::Str("k".into()), Value::Int(1))]);
        let b = Value::Map(vec![(Value::Str("k".into()), Value::Int(1))]);
        let c = Value::Map(vec![(Value::Str("k".into()), Value::Int(2))]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn igualdad_map_sensible_al_orden() {
        // Como usamos Vec<(K,V)>, orden importa para igualdad. Esto es
        // consistente con cómo lo imprimimos (preservando orden).
        let a = Value::Map(vec![
            (Value::Str("a".into()), Value::Int(1)),
            (Value::Str("b".into()), Value::Int(2)),
        ]);
        let b = Value::Map(vec![
            (Value::Str("b".into()), Value::Int(2)),
            (Value::Str("a".into()), Value::Int(1)),
        ]);
        assert_ne!(a, b);
    }

    #[test]
    fn igualdad_range() {
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
    fn igualdad_entre_tipos_distintos_es_false_para_nuevos() {
        // Sanity: list != map, list != range, etc.
        assert_ne!(Value::List(vec![]), Value::Map(vec![]));
        assert_ne!(
            Value::List(vec![Value::Int(0), Value::Int(1)]),
            Value::Range { start: 0, end: 2 },
        );
    }
}
