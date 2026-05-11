// env.rs — Fase 2.4
//
// Environment: el almacén de variables del intérprete. Es una cadena de
// scopes encadenados, cada uno con un mapa nombre→valor y un puntero a su
// scope padre.
//
//     global ←─ función outer ←─ función inner
//
// Lookups van hacia arriba en la cadena hasta encontrar la variable o
// llegar al global.
//
// Por qué `Rc<RefCell<Environment>>`:
//  - `Rc` (Reference Counted) permite compartir el mismo environment desde
//    múltiples lugares — clave para closures: la función "captura" un
//    handle al env donde fue definida, y ese mismo env puede seguir vivo
//    en el caller después.
//  - `RefCell` da mutabilidad interior — necesario porque Rust no nos deja
//    tener múltiples referencias `&mut` al mismo dato, pero nosotros sí
//    queremos mutar el env desde adentro de funciones que lo comparten.
//    `RefCell` mueve el chequeo de borrows del compile-time al runtime.
//  - El precio: cada acceso es `env.borrow()` o `env.borrow_mut()`. Y si
//    pedimos dos `borrow_mut` simultáneos del mismo `RefCell`, panic.
//
// Política de Assign (la decide el evaluador, no env):
//  - Si la variable existe en algún scope visible → reasignar ahí (`assign`).
//  - Si no existe → crear local (`define`).
//  Esto permite reasignar variables del scope padre sin sintaxis especial,
//  pero crear locales nuevos cuando hace falta. Si más adelante esto trae
//  confusión (ej: closures que mutan accidentalmente al padre) lo revisamos.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::Value;

/// Un alcance de variables con puntero opcional al scope padre.
#[derive(Debug)]
pub struct Environment {
    vars: HashMap<String, Value>,
    parent: Option<Rc<RefCell<Environment>>>,
}

/// Alias para abreviar las firmas: un handle compartido y mutable al env.
pub type EnvRef = Rc<RefCell<Environment>>;

impl Environment {
    /// Crea un environment raíz, sin padre. Típicamente uno solo por programa.
    pub fn new() -> EnvRef {
        Rc::new(RefCell::new(Environment {
            vars: HashMap::new(),
            parent: None,
        }))
    }

    /// Crea un scope hijo de `parent`. Usado al entrar a una función o bloque.
    pub fn new_child(parent: EnvRef) -> EnvRef {
        Rc::new(RefCell::new(Environment {
            vars: HashMap::new(),
            parent: Some(parent),
        }))
    }

    /// Define una variable en el scope actual. Si ya existía en este scope,
    /// la sobreescribe (shadowing local). No toca scopes padres.
    pub fn define(&mut self, name: impl Into<String>, value: Value) {
        self.vars.insert(name.into(), value);
    }

    /// Busca una variable subiendo por la cadena de scopes. Devuelve un clon
    /// del valor (los `Value` son baratos de clonar: enteros, floats, bools,
    /// y strings con clone "heavy" solo para Str — aceptable por ahora).
    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        self.parent.as_ref().and_then(|p| p.borrow().get(name))
    }

    /// `true` si la variable existe en este scope o en cualquier ancestro.
    pub fn has(&self, name: &str) -> bool {
        if self.vars.contains_key(name) {
            return true;
        }
        self.parent.as_ref().is_some_and(|p| p.borrow().has(name))
    }

    /// Reasigna una variable existente. Busca en la cadena de scopes y la
    /// reescribe en el scope donde fue definida. Devuelve `Err(())` si la
    /// variable no existe en ningún scope visible — el evaluador convierte
    /// ese error en `FitzError::UndefinedVariable`.
    pub fn assign(&mut self, name: &str, value: Value) -> Result<(), ()> {
        if self.vars.contains_key(name) {
            self.vars.insert(name.to_string(), value);
            return Ok(());
        }
        match &self.parent {
            Some(parent) => parent.borrow_mut().assign(name, value),
            None => Err(()),
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
    fn define_y_get_en_scope_simple() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(42));

        assert_eq!(env.borrow().get("x"), Some(Value::Int(42)));
    }

    #[test]
    fn get_devuelve_none_si_no_existe() {
        let env = Environment::new();
        assert_eq!(env.borrow().get("inexistente"), None);
    }

    #[test]
    fn child_ve_variables_del_padre() {
        let global = Environment::new();
        global.borrow_mut().define("x", Value::Int(1));

        let child = Environment::new_child(global.clone());
        assert_eq!(child.borrow().get("x"), Some(Value::Int(1)));
    }

    #[test]
    fn child_puede_sombrear_al_padre_con_define() {
        // define() siempre escribe local — sombrea al padre.
        let global = Environment::new();
        global.borrow_mut().define("x", Value::Int(1));

        let child = Environment::new_child(global.clone());
        child.borrow_mut().define("x", Value::Int(99));

        assert_eq!(child.borrow().get("x"), Some(Value::Int(99)));
        // El padre sigue intacto.
        assert_eq!(global.borrow().get("x"), Some(Value::Int(1)));
    }

    #[test]
    fn assign_desde_child_reasigna_en_el_padre() {
        // assign() busca en la cadena. Reescribe donde la variable existe.
        let global = Environment::new();
        global.borrow_mut().define("x", Value::Int(1));

        let child = Environment::new_child(global.clone());
        child.borrow_mut().assign("x", Value::Int(2)).unwrap();

        // El cambio se ve tanto en el child como en el padre.
        assert_eq!(child.borrow().get("x"), Some(Value::Int(2)));
        assert_eq!(global.borrow().get("x"), Some(Value::Int(2)));
    }

    #[test]
    fn assign_a_variable_no_definida_devuelve_err() {
        let env = Environment::new();
        assert!(env.borrow_mut().assign("x", Value::Int(1)).is_err());
    }

    #[test]
    fn has_busca_en_cadena_de_scopes() {
        let global = Environment::new();
        global.borrow_mut().define("global_var", Value::Int(1));

        let middle = Environment::new_child(global.clone());
        middle.borrow_mut().define("middle_var", Value::Int(2));

        let leaf = Environment::new_child(middle.clone());
        leaf.borrow_mut().define("leaf_var", Value::Int(3));

        let leaf_ref = leaf.borrow();
        assert!(leaf_ref.has("global_var"));
        assert!(leaf_ref.has("middle_var"));
        assert!(leaf_ref.has("leaf_var"));
        assert!(!leaf_ref.has("no_existe"));
    }

    #[test]
    fn assign_actualiza_solo_el_scope_correcto() {
        // global.x existe, middle.y existe. Desde leaf, reasigno ambas.
        let global = Environment::new();
        global.borrow_mut().define("x", Value::Int(10));

        let middle = Environment::new_child(global.clone());
        middle.borrow_mut().define("y", Value::Int(20));

        let leaf = Environment::new_child(middle.clone());

        leaf.borrow_mut().assign("x", Value::Int(100)).unwrap();
        leaf.borrow_mut().assign("y", Value::Int(200)).unwrap();

        assert_eq!(global.borrow().get("x"), Some(Value::Int(100)));
        assert_eq!(middle.borrow().get("y"), Some(Value::Int(200)));
        // No hay variables locales en leaf.
        assert_eq!(leaf.borrow().vars.len(), 0);
    }

    #[test]
    fn shadowing_local_no_afecta_lookups_del_padre_hacia_arriba() {
        // Si child sombrea x, el padre no se ve afectado al hacer get desde
        // un hermano del child.
        let global = Environment::new();
        global.borrow_mut().define("x", Value::Int(1));

        let child_a = Environment::new_child(global.clone());
        child_a.borrow_mut().define("x", Value::Int(99));

        let child_b = Environment::new_child(global.clone());
        // child_b no sombrea — ve el x del global.
        assert_eq!(child_b.borrow().get("x"), Some(Value::Int(1)));
    }
}
