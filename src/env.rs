// env.rs — Phase 2.4 (EnvRef migrated to Arc<Mutex<>> in F17.2)
//
// Environment: the interpreter's variable store. A chain of linked
// scopes, each with a name→value map and a pointer to its parent.
//
//     global ←─ outer function ←─ inner function
//
// Lookups walk up the chain until they find the variable or reach the
// global scope.
//
// Why `Arc<parking_lot::Mutex<Environment>>` (F17.2):
//  - `Arc` lets the same environment be shared from multiple places and
//    across threads — key for closures (the function "captures" a handle
//    to the env where it was defined) and to unblock `Send` on
//    `Value::Function` (axum HTTP handlers require `Send + 'static`).
//  - `parking_lot::Mutex` provides thread-safe interior mutability.
//    Chosen over `std::sync::Mutex` because it is faster without
//    contention (~10 ns vs ~30 ns) and does not poison on panic. It
//    does not distinguish reads from writes (a single `.lock()` for
//    both cases), same model as the RefCell it replaces.
//  - The price: every access is `env.lock()` and the guard is released
//    when the scope ends. Double `.lock()` on the same thread →
//    deadlock (parking_lot::Mutex is NOT reentrant). Policy: take the
//    lock for the smallest possible scope and release it before any
//    recursive call or `.await`.
//  - Before F17.2: `Rc<RefCell<>>` — single-threaded, runtime-checked.
//    Double borrow_mut used to panic at runtime; now it deadlocks
//    (equivalent risk, different symptom). The re-entrancy audit was
//    done in F17.2 over `eval_call` and `Environment::assign/get/has`.
//
// Assign policy (decided by the evaluator, not by env):
//  - If the variable exists in some visible scope → reassign there (`assign`).
//  - If it does not exist → create local (`define`).
//  This allows reassigning variables from the parent scope without
//  special syntax, while still creating new locals when needed. If this
//  ever causes confusion (e.g. closures accidentally mutating the
//  parent) we will revisit.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::value::Value;

/// A variable scope with an optional pointer to the parent scope.
#[derive(Debug)]
pub struct Environment {
    vars: HashMap<String, Value>,
    parent: Option<Arc<Mutex<Environment>>>,
}

/// Shorthand alias for the signatures: a shared, mutable handle to the env.
pub type EnvRef = Arc<Mutex<Environment>>;

impl Environment {
    /// Creates a root environment, with no parent. Typically one per program.
    pub fn new() -> EnvRef {
        Arc::new(Mutex::new(Environment {
            vars: HashMap::new(),
            parent: None,
        }))
    }

    /// Creates a child scope of `parent`. Used when entering a function or block.
    pub fn new_child(parent: EnvRef) -> EnvRef {
        Arc::new(Mutex::new(Environment {
            vars: HashMap::new(),
            parent: Some(parent),
        }))
    }

    /// Defines a variable in the current scope. If it already existed in
    /// this scope, it is overwritten (local shadowing). Parent scopes are
    /// not touched.
    pub fn define(&mut self, name: impl Into<String>, value: Value) {
        self.vars.insert(name.into(), value);
    }

    /// Looks up a variable by walking up the chain of scopes. Returns a
    /// clone of the value (`Value`s are cheap to clone: ints, floats,
    /// bools, and strings with a "heavy" clone only for Str — acceptable
    /// for now).
    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        self.parent.as_ref().and_then(|p| p.lock().get(name))
    }

    /// `true` if the variable exists in this scope or in any ancestor.
    pub fn has(&self, name: &str) -> bool {
        if self.vars.contains_key(name) {
            return true;
        }
        self.parent.as_ref().is_some_and(|p| p.lock().has(name))
    }

    /// Lists the names defined in the current scope (without recursing
    /// into the parent). Used by the REPL: it shows the bindings the
    /// user created in their session, separated from the builtins
    /// (`print`, `len`, etc.) that live in the same root scope. The
    /// caller does the filtering. Returns the names sorted
    /// alphabetically for predictable output.
    pub fn local_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.vars.keys().cloned().collect();
        names.sort();
        names
    }

    /// Reassigns an existing variable. Walks the chain of scopes and
    /// rewrites it in the scope where it was defined. Returns `Err(())`
    /// if the variable does not exist in any visible scope — the
    /// evaluator turns that error into `FitzError::UndefinedVariable`.
    //
    // `Result<(), ()>` is an intentional sentinel (we carry no extra
    // info in the `Err`, the caller enriches it with `FitzError`). The
    // `clippy::result_unit_err` lint appears from clippy 1.95 onwards,
    // exposed by the lib+bin refactor of Phase 9.x.1.b. Refactoring to
    // a newtype error is left as minor debt — no immediate value.
    #[allow(clippy::result_unit_err)]
    pub fn assign(&mut self, name: &str, value: Value) -> Result<(), ()> {
        if self.vars.contains_key(name) {
            self.vars.insert(name.to_string(), value);
            return Ok(());
        }
        match &self.parent {
            Some(parent) => parent.lock().assign(name, value),
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
    fn define_and_get_in_simple_scope() {
        let env = Environment::new();
        env.lock().define("x", Value::Int(42));

        assert_eq!(env.lock().get("x"), Some(Value::Int(42)));
    }

    #[test]
    fn get_returns_none_if_not_exists() {
        let env = Environment::new();
        assert_eq!(env.lock().get("inexistente"), None);
    }

    #[test]
    fn child_sees_parent_variables() {
        let global = Environment::new();
        global.lock().define("x", Value::Int(1));

        let child = Environment::new_child(global.clone());
        assert_eq!(child.lock().get("x"), Some(Value::Int(1)));
    }

    #[test]
    fn child_can_shadow_parent_with_define() {
        // define() always writes local — it shadows the parent.
        let global = Environment::new();
        global.lock().define("x", Value::Int(1));

        let child = Environment::new_child(global.clone());
        child.lock().define("x", Value::Int(99));

        assert_eq!(child.lock().get("x"), Some(Value::Int(99)));
        // The parent stays untouched.
        assert_eq!(global.lock().get("x"), Some(Value::Int(1)));
    }

    #[test]
    fn assign_from_child_reassigns_in_parent() {
        // assign() walks the chain. It rewrites where the variable exists.
        let global = Environment::new();
        global.lock().define("x", Value::Int(1));

        let child = Environment::new_child(global.clone());
        child.lock().assign("x", Value::Int(2)).unwrap();

        // The change is visible both in the child and in the parent.
        assert_eq!(child.lock().get("x"), Some(Value::Int(2)));
        assert_eq!(global.lock().get("x"), Some(Value::Int(2)));
    }

    #[test]
    fn assign_to_undefined_variable_returns_err() {
        let env = Environment::new();
        assert!(env.lock().assign("x", Value::Int(1)).is_err());
    }

    #[test]
    fn has_searches_through_scope_chain() {
        let global = Environment::new();
        global.lock().define("global_var", Value::Int(1));

        let middle = Environment::new_child(global.clone());
        middle.lock().define("middle_var", Value::Int(2));

        let leaf = Environment::new_child(middle.clone());
        leaf.lock().define("leaf_var", Value::Int(3));

        let leaf_ref = leaf.lock();
        assert!(leaf_ref.has("global_var"));
        assert!(leaf_ref.has("middle_var"));
        assert!(leaf_ref.has("leaf_var"));
        assert!(!leaf_ref.has("no_existe"));
    }

    #[test]
    fn assign_updates_only_the_correct_scope() {
        // global.x exists, middle.y exists. From leaf, reassign both.
        let global = Environment::new();
        global.lock().define("x", Value::Int(10));

        let middle = Environment::new_child(global.clone());
        middle.lock().define("y", Value::Int(20));

        let leaf = Environment::new_child(middle.clone());

        leaf.lock().assign("x", Value::Int(100)).unwrap();
        leaf.lock().assign("y", Value::Int(200)).unwrap();

        assert_eq!(global.lock().get("x"), Some(Value::Int(100)));
        assert_eq!(middle.lock().get("y"), Some(Value::Int(200)));
        // No local variables in leaf.
        assert_eq!(leaf.lock().vars.len(), 0);
    }

    #[test]
    fn local_shadowing_does_not_affect_parent_lookups_upward() {
        // If a child shadows x, the parent is not affected when a
        // sibling of that child runs get.
        let global = Environment::new();
        global.lock().define("x", Value::Int(1));

        let child_a = Environment::new_child(global.clone());
        child_a.lock().define("x", Value::Int(99));

        let child_b = Environment::new_child(global.clone());
        // child_b does not shadow — it sees x from the global scope.
        assert_eq!(child_b.lock().get("x"), Some(Value::Int(1)));
    }
}
