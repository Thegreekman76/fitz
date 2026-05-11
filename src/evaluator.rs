// evaluator.rs — Fase 2.4
//
// Recorre el AST y produce efectos (imprimir, mutar variables) y valores.
//
// Estructura interna:
//
//  ┌──────────────┐   programa
//  │ eval(...)    │ ──────────► env global + register_builtins
//  └──────┬───────┘
//         │ por cada Stmt
//         ▼
//  ┌──────────────┐         ┌──────────────┐
//  │ eval_stmt    │ ◀──────►│ eval_expr    │
//  └──────────────┘         └──────────────┘
//
// Control de flujo y errores comparten un mismo canal: `EvalSignal`. Esto
// nos permite usar `?` para propagar tanto errores reales como un `return`
// que tiene que escalar hasta el caller de la función. El truco lo tomé de
// Crafting Interpreters; en Rust funciona naturalmente con `Result`.

use crate::ast::{BinOpKind, Expr, Pattern, Program, Stmt, StrPart, UnaryOpKind};
use crate::env::{EnvRef, Environment};
use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::value::Value;

// ---------------------------------------------------------------------------
// EvalSignal — el canal único de "salida no normal" de eval_stmt/eval_expr.
// ---------------------------------------------------------------------------

/// Una interrupción del flujo normal de evaluación. Cubre dos cosas en una:
///  - errores reales del programa (`Error`)
///  - control de flujo no local (`Return`, `Break`, `Continue`)
///
/// Cuando una función llama a otra, el caller espera convertir
/// `Err(Return(v))` en `Ok(v)`. Cuando un loop captura un `break`, convierte
/// `Err(Break)` en una salida normal. Cualquier otra cosa se propaga.
#[derive(Debug)]
pub enum EvalSignal {
    Error(FitzError),
    Return(Value),
    Break,
    Continue,
}

/// `From<FitzError>` permite hacer `return Err(error.into())` o usar `?`
/// directamente cuando una función auxiliar devuelve `FitzResult`.
impl From<FitzError> for EvalSignal {
    fn from(e: FitzError) -> Self {
        EvalSignal::Error(e)
    }
}

pub type EvalResult<T> = Result<T, EvalSignal>;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Ejecuta un programa. Construye el env global, registra builtins, e itera
/// las sentencias del programa.
///
/// Signals "huérfanos" (`return`/`break`/`continue` fuera de su contexto)
/// se convierten acá en errores del usuario.
pub fn eval(program: Program) -> FitzResult<()> {
    let env = Environment::new();
    register_builtins(&env);

    for stmt in &program {
        if let Err(signal) = eval_stmt(stmt, env.clone()) {
            return Err(signal_to_error(signal));
        }
    }
    Ok(())
}

/// Convierte un signal sin contexto en un `FitzError` legible.
fn signal_to_error(signal: EvalSignal) -> FitzError {
    match signal {
        EvalSignal::Error(e) => e,
        EvalSignal::Return(_) => FitzError::new(
            ErrorKind::ReturnOutsideFunction,
            0, 0,
            "`return` solo puede usarse adentro de una función",
        ),
        EvalSignal::Break => FitzError::new(
            ErrorKind::BreakOutsideLoop,
            0, 0,
            "`break` solo puede usarse adentro de un loop",
        ),
        EvalSignal::Continue => FitzError::new(
            ErrorKind::ContinueOutsideLoop,
            0, 0,
            "`continue` solo puede usarse adentro de un loop",
        ),
    }
}

// ---------------------------------------------------------------------------
// eval_stmt — evalúa una sentencia. Devuelve un valor para que `if` y otros
// constructos-bloque puedan usarse como expresión: el valor de un bloque es
// el valor del último stmt evaluado (o `Null` si fue sentencia-puro).
// ---------------------------------------------------------------------------

fn eval_stmt(stmt: &Stmt, env: EnvRef) -> EvalResult<Value> {
    match stmt {
        Stmt::Expr(expr) => eval_expr(expr, env),

        // `name = value` o `name: Tipo = value`. La anotación de tipo se
        // ignora en runtime — tipado gradual, los checks de tipos los hará
        // un type-checker estático más adelante.
        //
        // Política: si la variable ya existe en algún scope visible, reasignar
        // ahí. Si no, crear local. Ver comentario de env.rs.
        Stmt::Assign { name, type_: _, value } => {
            let v = eval_expr(value, env.clone())?;
            // Borrows separados: `has` toma borrow inmutable, lo soltamos
            // antes de pedir un borrow mutable. RefCell paniquea en runtime
            // si los anidamos.
            let already_defined = env.borrow().has(name);
            if already_defined {
                env.borrow_mut()
                    .assign(name, v)
                    .expect("la variable existe — acabamos de chequear con has()");
            } else {
                env.borrow_mut().define(name.clone(), v);
            }
            Ok(Value::Null)
        }

        // `return expr` — evalúa el valor y lo emite como signal. El handler
        // de Call lo intercepta y lo convierte en valor de retorno. Si nadie
        // lo intercepta, llega al top level y se reporta como error.
        Stmt::Return(expr) => {
            let v = eval_expr(expr, env)?;
            Err(EvalSignal::Return(v))
        }

        // `fn name(params) -> ret { body }`. Construye un `Value::Function`
        // capturando el env actual como closure y lo registra con `define`.
        //
        // El orden importa para recursión: como `closure` y el env donde se
        // hace `define` son el MISMO Rc, el body de la función "ve" su
        // propia definición — puede llamarse a sí misma sin hacer nada extra.
        //
        // `return_type` y `is_async` se ignoran en runtime (deuda explícita
        // para type-checker estático en Fase 5 y async en Fase 4).
        Stmt::FnDef { name, params, return_type: _, body, is_async: _ } => {
            let func = Value::Function {
                params: params.clone(),
                body: body.clone(),
                closure: env.clone(),
            };
            env.borrow_mut().define(name.clone(), func);
            Ok(Value::Null)
        }

        // `type Name { campo1: T1, ... }`. Por ahora solo registramos el
        // tipo en el env como un valor inerte. La instanciación (`User { id: 1 }`)
        // y el field access requieren extensiones del AST (Fase 3).
        Stmt::TypeDef { name, fields } => {
            let t = Value::Type {
                name: name.clone(),
                fields: fields.clone(),
            };
            env.borrow_mut().define(name.clone(), t);
            Ok(Value::Null)
        }
        Stmt::HttpEndpoint { .. } => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            0, 0,
            "Los endpoints HTTP requieren la Fase 4 (HTTP nativo)",
        ))),
        Stmt::Break => Err(EvalSignal::Break),
        Stmt::Continue => Err(EvalSignal::Continue),

        // `for var in iter { body }` — evalúa `iter` una sola vez al
        // entrar, después itera. `var` se redefine en el env actual en
        // cada iteración (no creamos scope nuevo, consistente con la
        // política de bloques de Fitz: las variables del cuerpo persisten).
        //
        // Iterables soportados:
        //  - List: itera los elementos en orden.
        //  - Range: itera los Int de start a end-1.
        //  - Map: aún no (necesita el tipo `Pair`/`entry`; paso 4 de Fase 3).
        //  - Otros: type error explícito.
        Stmt::For { var, iter, body } => {
            let iter_v = eval_expr(iter, env.clone())?;
            let items_iter: Box<dyn Iterator<Item = Value>> = match iter_v {
                Value::List(items) => Box::new(items.into_iter()),
                Value::Range { start, end } => Box::new((start..end).map(Value::Int)),
                Value::Map(_) => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0, 0,
                    "`for` sobre Map aún no soportado — esperá al tipo Pair (paso 4 de Fase 3)",
                ))),
                other => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "List o Range".into(),
                        found: other.type_name().into(),
                    },
                    0, 0,
                    format!(
                        "no se puede iterar sobre un valor de tipo `{}`",
                        other.type_name()
                    ),
                ))),
            };
            for item in items_iter {
                env.borrow_mut().define(var.clone(), item);
                match run_loop_body(body, env.clone()) {
                    LoopControl::Continue => continue,
                    LoopControl::Break => break,
                    LoopControl::Propagate(signal) => return Err(signal),
                }
            }
            Ok(Value::Null)
        }

        // `while cond { body }`. La cond se evalúa antes de cada iteración.
        // Tiene que ser Bool; otros tipos → type error.
        //
        // Captura `Break` y `Continue` como signals — `Break` termina el
        // loop, `Continue` salta a la siguiente iteración. Errors y
        // `Return` se propagan al caller (un return dentro de un while
        // dentro de una función rompe ambos hasta la función).
        Stmt::While { condition, body } => {
            loop {
                let cond_v = eval_expr(condition, env.clone())?;
                let cond_bool = match cond_v {
                    Value::Bool(b) => b,
                    other => return Err(EvalSignal::Error(FitzError::new(
                        ErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            found: other.type_name().into(),
                        },
                        0, 0,
                        format!(
                            "la condición de `while` debe ser Bool, no `{}`",
                            other.type_name()
                        ),
                    ))),
                };
                if !cond_bool {
                    break;
                }
                match run_loop_body(body, env.clone()) {
                    LoopControl::Continue => continue,
                    LoopControl::Break => break,
                    LoopControl::Propagate(signal) => return Err(signal),
                }
            }
            Ok(Value::Null)
        }

        // `loop { body }` — itera para siempre. Solo `break` o `return`
        // pueden sacarte.
        Stmt::Loop { body } => {
            loop {
                match run_loop_body(body, env.clone()) {
                    LoopControl::Continue => continue,
                    LoopControl::Break => break,
                    LoopControl::Propagate(signal) => return Err(signal),
                }
            }
            Ok(Value::Null)
        }
    }
}

/// Resultado de correr el cuerpo de un loop una vez. Convierte signals de
/// control de flujo en una decisión local (seguir / salir / propagar).
enum LoopControl {
    Continue,
    Break,
    Propagate(EvalSignal),
}

/// Ejecuta los stmts del body en orden. Si alguno emite `Break` o `Continue`,
/// los traduce a control local. Cualquier otro signal (Error, Return) sube
/// como `Propagate` para que el loop lo devuelva al caller.
fn run_loop_body(body: &[Stmt], env: EnvRef) -> LoopControl {
    for stmt in body {
        match eval_stmt(stmt, env.clone()) {
            Ok(_) => {}
            Err(EvalSignal::Break) => return LoopControl::Break,
            Err(EvalSignal::Continue) => return LoopControl::Continue,
            Err(other) => return LoopControl::Propagate(other),
        }
    }
    LoopControl::Continue
}

// ---------------------------------------------------------------------------
// eval_expr — evalúa una expresión a un Value.
// ---------------------------------------------------------------------------

fn eval_expr(expr: &Expr, env: EnvRef) -> EvalResult<Value> {
    match expr {
        // Literales — el valor está embebido en el AST.
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(x) => Ok(Value::Float(*x)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Null => Ok(Value::Null),

        // Identificador — lookup encadenado en la cadena de scopes.
        Expr::Ident(name) => env.borrow().get(name).ok_or_else(|| {
            EvalSignal::Error(FitzError::new(
                ErrorKind::UndefinedVariable(name.clone()),
                0, 0,
                format!("variable `{}` no definida", name),
            ))
        }),

        // And/Or hacen short-circuit: no evaluamos `right` salvo que haga
        // falta. El resto de BinOps evalúan ambos lados antes de combinar.
        Expr::BinOp { op, left, right } if matches!(op, BinOpKind::And | BinOpKind::Or) => {
            eval_logical(op, left, right, env)
        }
        Expr::BinOp { op, left, right } => {
            let lv = eval_expr(left, env.clone())?;
            let rv = eval_expr(right, env)?;
            eval_binop(op, lv, rv)
        }

        Expr::UnaryOp { op, operand } => {
            let v = eval_expr(operand, env)?;
            eval_unary(op, v)
        }

        // String con interpolación: cada `StrPart::Expr` se evalúa y se
        // convierte a string vía `Display`. Los `Lit` van tal cual.
        Expr::StrInterp(parts) => {
            let mut result = String::new();
            for part in parts {
                match part {
                    StrPart::Lit(s) => result.push_str(s),
                    StrPart::Expr(e) => {
                        let v = eval_expr(e, env.clone())?;
                        result.push_str(&v.to_string());
                    }
                }
            }
            Ok(Value::Str(result))
        }

        // Llamada a función. Por ahora solo builtins; las user-defined
        // (`Value::Function`) llegan en el próximo paso.
        Expr::Call { name, args } => eval_call(name, args, env),

        Expr::Field { .. } => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::InvalidSyntax,
            0, 0,
            "Field access requiere tipos custom instanciados (Fase 3)",
        ))),

        // `[e1, e2, ...]` — evaluamos los elementos en orden.
        Expr::List(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(eval_expr(item, env.clone())?);
            }
            Ok(Value::List(values))
        }

        // `{k1: v1, ...}` — evaluamos cada par en orden (clave, valor).
        // El orden de inserción se preserva en el Vec resultante.
        Expr::Map(pairs) => {
            let mut entries = Vec::with_capacity(pairs.len());
            for (k_expr, v_expr) in pairs {
                let k = eval_expr(k_expr, env.clone())?;
                let v = eval_expr(v_expr, env.clone())?;
                entries.push((k, v));
            }
            Ok(Value::Map(entries))
        }

        // `start..end` — ambos extremos tienen que ser Int (no hay rangos
        // de Float). El rango se materializa como `Value::Range`; la
        // iteración real (cuando se usa en `for`) ocurre en Stmt::For.
        Expr::Range { start, end } => {
            let s_v = eval_expr(start, env.clone())?;
            let e_v = eval_expr(end, env)?;
            let s = expect_int_for_range(&s_v, "inicio")?;
            let e = expect_int_for_range(&e_v, "fin")?;
            Ok(Value::Range { start: s, end: e })
        }

        // `obj[idx]` — indexing. Dispatch por tipo del objeto.
        Expr::Index { object, index } => {
            let obj = eval_expr(object, env.clone())?;
            let idx = eval_expr(index, env)?;
            eval_index(&obj, &idx)
        }

        // `if cond { then } else { else_ }`. Funciona como expresión: su
        // valor es el del último stmt del bloque ejecutado. Sin else y cond
        // falsa → Null.
        //
        // Los bloques NO crean scope nuevo — variables declaradas adentro
        // persisten en el scope contenedor (estilo Python). Deuda explícita
        // si después esto trae sorpresas.
        Expr::If { condition, then, else_ } => {
            let cond_v = eval_expr(condition, env.clone())?;
            let cond_bool = match cond_v {
                Value::Bool(b) => b,
                other => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Bool".into(),
                        found: other.type_name().into(),
                    },
                    0, 0,
                    format!(
                        "la condición de `if` debe ser Bool, no `{}`",
                        other.type_name()
                    ),
                ))),
            };

            if cond_bool {
                eval_block(then, env)
            } else if let Some(else_block) = else_ {
                eval_block(else_block, env)
            } else {
                Ok(Value::Null)
            }
        }

        // `match value { pat1 => body1, pat2 => body2, ... }`. Recorre los
        // arms en orden y devuelve el body del primero que matchee.
        //
        // Patrones soportados:
        //  - `Ident(name)`: siempre matchea, bindea el valor a `name` para
        //    el body. Igual semántica que `n =>` en Rust.
        //  - `Wildcard`: siempre matchea, sin binding.
        //  - `Ok(x)` / `Err(e)`: requieren el tipo Result en runtime, que
        //    no existe aún. Error explícito hasta Fase 3.
        //
        // Cada arm crea un scope hijo para que el binding no contamine el
        // scope contenedor.
        Expr::Match { value, arms } => {
            let v = eval_expr(value, env.clone())?;

            for arm in arms {
                // Patrones literales — igualdad ESTRUCTURAL (sin coerción
                // Int↔Float, a diferencia del operador `==`). Si no matchea,
                // probamos el siguiente arm.
                let matched = match (&arm.pattern, &v) {
                    (Pattern::Int(p), Value::Int(vv)) => p == vv,
                    (Pattern::Float(p), Value::Float(vv)) => p == vv,
                    (Pattern::Str(p), Value::Str(vv)) => p == vv,
                    (Pattern::Bool(p), Value::Bool(vv)) => p == vv,
                    (Pattern::Null, Value::Null) => true,
                    (Pattern::Wildcard, _) => true,
                    // Ident matchea todo, pero con efecto secundario.
                    (Pattern::Ident(_), _) => true,
                    // Patrón de rango: solo aplica a Int. start <= v < end.
                    (Pattern::Range { start, end }, Value::Int(vv)) => start <= vv && vv < end,
                    // Ok/Err sin tipo Result — error explícito.
                    (Pattern::OkBinding(_) | Pattern::ErrBinding(_), _) => {
                        return Err(EvalSignal::Error(FitzError::new(
                            ErrorKind::InvalidSyntax,
                            0, 0,
                            "patrones `Ok(...)` / `Err(...)` requieren el tipo Result (Fase 3)",
                        )));
                    }
                    _ => false,
                };

                if !matched {
                    continue;
                }

                // Matcheó. Si es Ident, creamos scope con el binding.
                if let Pattern::Ident(name) = &arm.pattern {
                    let arm_env = Environment::new_child(env.clone());
                    arm_env.borrow_mut().define(name.clone(), v.clone());
                    return eval_expr(&arm.body, arm_env);
                }
                return eval_expr(&arm.body, env.clone());
            }

            // Ningún arm matcheó. Con Ident/Wildcard presentes es imposible;
            // ocurre solo si el match no tiene arms o todos son Ok/Err y el
            // valor no es un Result (caso futuro).
            Err(EvalSignal::Error(FitzError::new(
                ErrorKind::InvalidSyntax,
                0, 0,
                "el `match` no matcheó ningún brazo",
            )))
        }
    }
}

/// Evalúa una secuencia de sentencias en el env dado (sin crear scope
/// nuevo) y devuelve el valor de la última. Bloque vacío → Null.
///
/// Los signals (Return/Break/Continue/Error) se propagan: si un stmt los
/// emite, el resto del bloque no se ejecuta.
fn eval_block(stmts: &[Stmt], env: EnvRef) -> EvalResult<Value> {
    let mut last = Value::Null;
    for stmt in stmts {
        last = eval_stmt(stmt, env.clone())?;
    }
    Ok(last)
}

/// Resolver de llamadas. Hace lookup del nombre en el env, evalúa los args
/// en orden, y despacha según el tipo del valor encontrado.
///
/// En este paso solo soportamos `Value::Builtin`. La rama de `Value::Function`
/// (user-defined) viene en el próximo paso junto con `FnDef`.
fn eval_call(name: &str, args: &[Expr], env: EnvRef) -> EvalResult<Value> {
    let callee = env.borrow().get(name).ok_or_else(|| {
        EvalSignal::Error(FitzError::new(
            ErrorKind::UndefinedFunction(name.to_string()),
            0, 0,
            format!("función `{}` no definida", name),
        ))
    })?;

    // Evaluamos args de izquierda a derecha. Si alguno falla, el `?` corta.
    let mut arg_values = Vec::with_capacity(args.len());
    for arg in args {
        arg_values.push(eval_expr(arg, env.clone())?);
    }

    match callee {
        Value::Builtin { func, .. } => func(&arg_values).map_err(EvalSignal::Error),

        Value::Function { params, body, closure } => {
            // Validación de aridad. Defaults / args opcionales pueden venir
            // más adelante; por ahora cantidades estrictamente iguales.
            if arg_values.len() != params.len() {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::WrongArgCount {
                        expected: params.len(),
                        found: arg_values.len(),
                    },
                    0, 0,
                    format!(
                        "`{}` espera {} argumento(s), recibió {}",
                        name,
                        params.len(),
                        arg_values.len(),
                    ),
                )));
            }

            // Nuevo scope hijo del CLOSURE, no del caller. Esto es lo que
            // hace que las funciones vean las variables del lugar donde se
            // definieron, no del lugar donde se llaman. Lexical scoping.
            let call_env = Environment::new_child(closure);
            for (param, value) in params.iter().zip(arg_values) {
                call_env.borrow_mut().define(param.name.clone(), value);
                // param.type_ se ignora — tipado gradual sin checks runtime.
            }

            // Ejecutamos el body sentencia por sentencia. Si alguna emite
            // `EvalSignal::Return(v)`, la capturamos acá y la convertimos
            // en el valor de retorno. Cualquier otro signal (Error, Break,
            // Continue) se propaga al caller.
            for stmt in &body {
                match eval_stmt(stmt, call_env.clone()) {
                    Ok(_) => {}
                    Err(EvalSignal::Return(v)) => return Ok(v),
                    Err(other) => return Err(other),
                }
            }

            // Sin `return` explícito, la función devuelve Null. Más adelante
            // podemos cambiar esto a "el valor del último stmt" si queremos
            // estilo Rust, pero por ahora lo dejamos explícito.
            Ok(Value::Null)
        }

        other => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "función".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!("`{}` no es invocable (es {})", name, other.type_name()),
        ))),
    }
}

// ---------------------------------------------------------------------------
// Operaciones binarias
// ---------------------------------------------------------------------------
//
// Tabla de promoción para aritmética (Add, Sub, Mul, Div):
//
//   Int    + Int    → Int
//   Int    + Float  → Float
//   Float  + Int    → Float
//   Float  + Float  → Float
//   Str    + Str    → Str   (solo Add, concatenación)
//   resto           → TypeMismatch
//
// Para Div: si el divisor es 0 (Int) o 0.0 (Float), se emite DivisionByZero
// en vez de dejar pasar IEEE 754 infinitos/NaN.
//
// Comparaciones (Lt, LtEq, Gt, GtEq): numéricas con promoción Int↔Float, o
// strings alfabéticamente. El resto → TypeMismatch.
//
// Igualdad (Eq, NotEq): delega en `PartialEq` de `Value`, que ya hace
// coerción Int↔Float. Tipos incompatibles dan `false` sin error.

fn eval_binop(op: &BinOpKind, l: Value, r: Value) -> EvalResult<Value> {
    use BinOpKind::*;
    match op {
        Add => eval_add(l, r),
        Sub => arith(l, r, "-", |a, b| a - b, |a, b| a - b),
        Mul => arith(l, r, "*", |a, b| a * b, |a, b| a * b),
        Div => eval_div(l, r),
        Eq => Ok(Value::Bool(l == r)),
        NotEq => Ok(Value::Bool(l != r)),
        Lt | LtEq | Gt | GtEq => compare(op, l, r),
        And | Or => unreachable!("And/Or se manejan en eval_logical antes de llegar acá"),
    }
}

/// Add tiene un caso especial: `Str + Str` concatena. El resto delega en
/// `arith` con el mismo patrón de promoción Int↔Float.
fn eval_add(l: Value, r: Value) -> EvalResult<Value> {
    if let (Value::Str(a), Value::Str(b)) = (&l, &r) {
        return Ok(Value::Str(format!("{}{}", a, b)));
    }
    arith(l, r, "+", |a, b| a + b, |a, b| a + b)
}

/// Div chequea 0 antes de delegar — error explícito en vez de Infinity/NaN.
fn eval_div(l: Value, r: Value) -> EvalResult<Value> {
    match &r {
        Value::Int(0) => return div_by_zero(),
        Value::Float(b) if *b == 0.0 => return div_by_zero(),
        _ => {}
    }
    arith(l, r, "/", |a, b| a / b, |a, b| a / b)
}

/// Helper genérico para Add/Sub/Mul/Div: aplica `int_op` si ambos son Int,
/// `float_op` si alguno es Float (promoviendo el Int a f64). Resto → error.
///
/// `Fn(i64, i64) -> i64` es una _trait bound_ que acepta cualquier closure
/// que no consume su entorno. Los closures `|a, b| a + b` que pasamos no
/// capturan nada, así que cumplen. Esto evita repetir el match cuatro veces.
fn arith<I, F>(l: Value, r: Value, op_name: &str, int_op: I, float_op: F) -> EvalResult<Value>
where
    I: Fn(i64, i64) -> i64,
    F: Fn(f64, f64) -> f64,
{
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_op(a, b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(float_op(a as f64, b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(float_op(a, b as f64))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(a, b))),
        (l, r) => type_error(op_name, &l, &r),
    }
}

fn compare(op: &BinOpKind, l: Value, r: Value) -> EvalResult<Value> {
    use BinOpKind::*;

    // Numérico (con promoción Int→f64). NaN propaga como false en cualquiera
    // de los cuatro operadores, lo cual es la semántica de IEEE 754.
    if let (Some(a), Some(b)) = (as_f64(&l), as_f64(&r)) {
        return Ok(Value::Bool(match op {
            Lt => a < b,
            LtEq => a <= b,
            Gt => a > b,
            GtEq => a >= b,
            _ => unreachable!(),
        }));
    }

    // Strings alfabéticamente (orden lexicográfico estándar de Rust).
    if let (Value::Str(a), Value::Str(b)) = (&l, &r) {
        return Ok(Value::Bool(match op {
            Lt => a < b,
            LtEq => a <= b,
            Gt => a > b,
            GtEq => a >= b,
            _ => unreachable!(),
        }));
    }

    type_error(op_name(op), &l, &r)
}

/// Convierte un Value numérico a f64. Devuelve None si no es numérico —
/// usado en `compare` para discriminar el camino numérico del de strings.
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(x) => Some(*x),
        _ => None,
    }
}

/// And/Or con short-circuit y type-check de Bool. Vive aparte de `eval_binop`
/// porque necesita acceso a las expresiones SIN evaluar (para no evaluar el
/// lado derecho cuando el izquierdo ya determina el resultado).
fn eval_logical(op: &BinOpKind, left: &Expr, right: &Expr, env: EnvRef) -> EvalResult<Value> {
    let lv = eval_expr(left, env.clone())?;
    let lb = expect_bool(&lv, op_name(op), "izquierdo")?;

    // Short-circuit: `false and ...` → false, `true or ...` → true.
    match op {
        BinOpKind::And if !lb => return Ok(Value::Bool(false)),
        BinOpKind::Or if lb => return Ok(Value::Bool(true)),
        _ => {}
    }

    let rv = eval_expr(right, env)?;
    let rb = expect_bool(&rv, op_name(op), "derecho")?;
    Ok(Value::Bool(rb))
}

/// Helper para chequear que un Value sea Bool. Devuelve el bool o un
/// TypeMismatch contextualizado al operador y lado.
fn expect_bool(v: &Value, op: &str, side: &str) -> EvalResult<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Bool".into(),
                found: v.type_name().into(),
            },
            0, 0,
            format!("operando {} de `{}` debe ser Bool, no `{}`", side, op, v.type_name()),
        ))),
    }
}

/// Símbolo legible de un BinOpKind, para mensajes de error.
fn op_name(op: &BinOpKind) -> &'static str {
    use BinOpKind::*;
    match op {
        Add => "+", Sub => "-", Mul => "*", Div => "/",
        Eq => "==", NotEq => "!=",
        Lt => "<", LtEq => "<=", Gt => ">", GtEq => ">=",
        And => "and", Or => "or",
    }
}

fn type_error<T>(op: &str, l: &Value, r: &Value) -> EvalResult<T> {
    Err(EvalSignal::Error(FitzError::new(
        ErrorKind::TypeMismatch {
            expected: "operandos compatibles".into(),
            found: format!("{} {} {}", l.type_name(), op, r.type_name()),
        },
        0, 0,
        format!(
            "operación `{}` no soportada entre `{}` y `{}`",
            op, l.type_name(), r.type_name()
        ),
    )))
}

fn div_by_zero<T>() -> EvalResult<T> {
    Err(EvalSignal::Error(FitzError::new(
        ErrorKind::DivisionByZero,
        0, 0,
        "división por cero",
    )))
}

// ---------------------------------------------------------------------------
// Listas, mapas, rangos: helpers de runtime
// ---------------------------------------------------------------------------

/// Extrae el Int de un Value, o emite un TypeMismatch claro indicando si
/// fue el "inicio" o el "fin" del rango. Float NO coerciona — los rangos
/// son discretos.
fn expect_int_for_range(v: &Value, side: &str) -> EvalResult<i64> {
    match v {
        Value::Int(n) => Ok(*n),
        other => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!(
                "el {} de un rango debe ser Int, no `{}`",
                side, other.type_name()
            ),
        ))),
    }
}

/// `obj[idx]`. Dispatch por tipo del receptor:
///  - List + Int: bounds-check, devuelve el elemento.
///  - Map + cualquier valor: búsqueda lineal por igualdad (la misma
///    igualdad que usa `==`, así que claves Int↔Float matchean).
///  - Range: no indexable por ahora (semántica no obvia: ¿`(0..10)[3]` = 3?
///    Probablemente sí, pero lo dejamos para más adelante).
///  - Str: no indexable hasta que decidamos si la unidad es char o byte.
///  - Otros: type error.
fn eval_index(obj: &Value, idx: &Value) -> EvalResult<Value> {
    match obj {
        Value::List(items) => {
            let i = match idx {
                Value::Int(n) => *n,
                other => return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::TypeMismatch {
                        expected: "Int".into(),
                        found: other.type_name().into(),
                    },
                    0, 0,
                    format!(
                        "el índice de una lista debe ser Int, no `{}`",
                        other.type_name()
                    ),
                ))),
            };
            // Sin índices negativos por ahora (sin Python-style xs[-1]).
            // Si después lo agregamos, vivirá acá.
            if i < 0 {
                return Err(EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0, 0,
                    format!("índice negativo en lista: {}", i),
                )));
            }
            let i_usize = i as usize;
            items.get(i_usize).cloned().ok_or_else(|| {
                EvalSignal::Error(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    0, 0,
                    format!(
                        "índice fuera de rango: {} en lista de tamaño {}",
                        i,
                        items.len()
                    ),
                ))
            })
        }
        Value::Map(pairs) => {
            // Búsqueda lineal por igualdad. Esto va a ser O(n) hasta que
            // promovamos Map a una estructura indexada de verdad.
            for (k, v) in pairs {
                if k == idx {
                    return Ok(v.clone());
                }
            }
            Err(EvalSignal::Error(FitzError::new(
                ErrorKind::InvalidSyntax,
                0, 0,
                format!("clave no encontrada en mapa: {}", idx),
            )))
        }
        other => Err(EvalSignal::Error(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "List o Map".into(),
                found: other.type_name().into(),
            },
            0, 0,
            format!(
                "el tipo `{}` no soporta indexing con `[]`",
                other.type_name()
            ),
        ))),
    }
}

// ---------------------------------------------------------------------------
// Operación unaria
// ---------------------------------------------------------------------------
//
// Por ahora solo `Neg`: negación numérica (`-x`). Cuando el lexer emita `!`
// como operador lógico, sumaremos `Not` acá.

fn eval_unary(op: &UnaryOpKind, v: Value) -> EvalResult<Value> {
    match op {
        UnaryOpKind::Neg => match v {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(x) => Ok(Value::Float(-x)),
            other => Err(EvalSignal::Error(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "Int o Float".into(),
                    found: other.type_name().into(),
                },
                0, 0,
                format!("no se puede negar un valor de tipo `{}`", other.type_name()),
            ))),
        },
    }
}

// ---------------------------------------------------------------------------
// Builtins — funciones nativas implementadas en Rust, expuestas como
// identificadores en el env global.
// ---------------------------------------------------------------------------

/// Registra todas las funciones builtin en el environment. Llamar una sola
/// vez al inicio del programa.
fn register_builtins(env: &EnvRef) {
    env.borrow_mut().define(
        "print",
        Value::Builtin {
            name: "print",
            func: builtin_print,
        },
    );
    env.borrow_mut().define(
        "len",
        Value::Builtin {
            name: "len",
            func: builtin_len,
        },
    );
}

/// `print(arg1, arg2, ...)` — imprime los args convertidos a string,
/// separados por espacio, seguido de newline. Como Python.
fn builtin_print(args: &[Value]) -> FitzResult<Value> {
    let parts: Vec<String> = args.iter().map(|v| v.to_string()).collect();
    println!("{}", parts.join(" "));
    Ok(Value::Null)
}

/// `len(x)` — longitud de listas, mapas, strings y rangos.
///  - List: cantidad de elementos.
///  - Map: cantidad de pares.
///  - Str: cantidad de chars (no bytes — UTF-8 aware).
///  - Range: `end - start`, clampeado a 0 si el rango va al revés.
///  - Otros: type error.
fn builtin_len(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(FitzError::new(
            ErrorKind::WrongArgCount {
                expected: 1,
                found: args.len(),
            },
            0, 0,
            format!("`len` espera 1 argumento, recibió {}", args.len()),
        ));
    }
    let n: i64 = match &args[0] {
        Value::List(items) => items.len() as i64,
        Value::Map(pairs) => pairs.len() as i64,
        Value::Str(s) => s.chars().count() as i64,
        Value::Range { start, end } => (end - start).max(0),
        other => {
            return Err(FitzError::new(
                ErrorKind::TypeMismatch {
                    expected: "List, Map, Str o Range".into(),
                    found: other.type_name().into(),
                },
                0, 0,
                format!(
                    "`len` no aplica a un valor de tipo `{}`",
                    other.type_name()
                ),
            ));
        }
    };
    Ok(Value::Int(n))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers ----

    /// Evalúa una expresión aislada en un env vacío. Para tests cortos.
    fn eval_expr_test(expr: Expr) -> EvalResult<Value> {
        let env = Environment::new();
        eval_expr(&expr, env)
    }

    // ---- entry point ----

    #[test]
    fn programa_vacio_no_falla() {
        assert!(eval(vec![]).is_ok());
    }

    // ---- literales ----

    #[test]
    fn evalua_int_literal() {
        assert_eq!(eval_expr_test(Expr::Int(42)).unwrap(), Value::Int(42));
    }

    #[test]
    fn evalua_float_literal() {
        assert_eq!(eval_expr_test(Expr::Float(3.14)).unwrap(), Value::Float(3.14));
    }

    #[test]
    fn evalua_string_literal() {
        assert_eq!(
            eval_expr_test(Expr::Str("hola".into())).unwrap(),
            Value::Str("hola".into())
        );
    }

    #[test]
    fn evalua_bool_literal() {
        assert_eq!(eval_expr_test(Expr::Bool(true)).unwrap(), Value::Bool(true));
        assert_eq!(eval_expr_test(Expr::Bool(false)).unwrap(), Value::Bool(false));
    }

    #[test]
    fn evalua_null_literal() {
        assert_eq!(eval_expr_test(Expr::Null).unwrap(), Value::Null);
    }

    // ---- Ident ----

    #[test]
    fn ident_resuelve_variable_del_env() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(99));

        let result = eval_expr(&Expr::Ident("x".into()), env).unwrap();
        assert_eq!(result, Value::Int(99));
    }

    #[test]
    fn ident_no_definido_devuelve_error() {
        let env = Environment::new();
        let result = eval_expr(&Expr::Ident("nope".into()), env);

        match result {
            Err(EvalSignal::Error(e)) => {
                assert!(matches!(e.kind, ErrorKind::UndefinedVariable(ref n) if n == "nope"));
            }
            _ => panic!("se esperaba Error::UndefinedVariable"),
        }
    }

    #[test]
    fn ident_busca_en_scope_padre() {
        let global = Environment::new();
        global.borrow_mut().define("x", Value::Str("from_global".into()));

        let child = Environment::new_child(global);
        let result = eval_expr(&Expr::Ident("x".into()), child).unwrap();
        assert_eq!(result, Value::Str("from_global".into()));
    }

    // ---- Stmt::Expr (paso intermedio para verificar el wiring stmt→expr) ----

    #[test]
    fn stmt_expr_evalua_la_expresion_interna() {
        let env = Environment::new();
        let stmt = Stmt::Expr(Expr::Int(7));
        let result = eval_stmt(&stmt, env).unwrap();
        assert_eq!(result, Value::Int(7));
    }

    // ---- builtins ----

    #[test]
    fn builtin_print_devuelve_null() {
        let result = builtin_print(&[Value::Str("test".into())]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn register_builtins_define_print_en_env() {
        let env = Environment::new();
        register_builtins(&env);

        let print = env.borrow().get("print");
        assert!(print.is_some());
        match print.unwrap() {
            Value::Builtin { name, .. } => assert_eq!(name, "print"),
            _ => panic!("se esperaba Value::Builtin"),
        }
    }

    // ---- signals ----

    #[test]
    fn fitzerror_se_convierte_a_evalsignal_error() {
        let err = FitzError::new(ErrorKind::DivisionByZero, 1, 1, "test");
        let signal: EvalSignal = err.into();
        assert!(matches!(signal, EvalSignal::Error(_)));
    }

    #[test]
    fn break_fuera_de_loop_es_error() {
        let result = eval(vec![Stmt::Break]);
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::BreakOutsideLoop
        ));
    }

    #[test]
    fn continue_fuera_de_loop_es_error() {
        let result = eval(vec![Stmt::Continue]);
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::ContinueOutsideLoop
        ));
    }

    // ---- BinOp: aritmética ----

    /// Helper: construye `BinOp { op, left: l, right: r }` con boxes.
    fn binop(op: BinOpKind, l: Expr, r: Expr) -> Expr {
        Expr::BinOp { op, left: Box::new(l), right: Box::new(r) }
    }

    #[test]
    fn add_int_int_da_int() {
        let e = binop(BinOpKind::Add, Expr::Int(2), Expr::Int(3));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(5));
    }

    #[test]
    fn add_int_float_promueve_a_float() {
        let e = binop(BinOpKind::Add, Expr::Int(2), Expr::Float(0.5));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Float(2.5));
    }

    #[test]
    fn add_float_int_promueve_a_float() {
        let e = binop(BinOpKind::Add, Expr::Float(1.5), Expr::Int(2));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Float(3.5));
    }

    #[test]
    fn add_strings_concatena() {
        let e = binop(
            BinOpKind::Add,
            Expr::Str("hola ".into()),
            Expr::Str("mundo".into()),
        );
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("hola mundo".into()));
    }

    #[test]
    fn add_tipos_incompatibles_es_type_error() {
        let e = binop(BinOpKind::Add, Expr::Str("x".into()), Expr::Int(1));
        match eval_expr_test(e) {
            Err(EvalSignal::Error(err)) => {
                assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
            }
            _ => panic!("se esperaba TypeMismatch"),
        }
    }

    #[test]
    fn sub_mul_funcionan() {
        let sub = binop(BinOpKind::Sub, Expr::Int(10), Expr::Int(3));
        assert_eq!(eval_expr_test(sub).unwrap(), Value::Int(7));

        let mul = binop(BinOpKind::Mul, Expr::Int(4), Expr::Int(5));
        assert_eq!(eval_expr_test(mul).unwrap(), Value::Int(20));
    }

    #[test]
    fn div_int_int_trunca() {
        // 10 / 3 = 3 (truncado), no 3.33
        let e = binop(BinOpKind::Div, Expr::Int(10), Expr::Int(3));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(3));
    }

    #[test]
    fn div_int_float_da_float() {
        let e = binop(BinOpKind::Div, Expr::Int(10), Expr::Float(4.0));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Float(2.5));
    }

    #[test]
    fn div_por_cero_int_es_error() {
        let e = binop(BinOpKind::Div, Expr::Int(1), Expr::Int(0));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::DivisionByZero, .. })
        ));
    }

    #[test]
    fn div_por_cero_float_es_error() {
        let e = binop(BinOpKind::Div, Expr::Float(1.0), Expr::Float(0.0));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::DivisionByZero, .. })
        ));
    }

    // ---- BinOp: comparación e igualdad ----

    #[test]
    fn eq_con_coercion_int_float() {
        // 1 == 1.0 → true
        let e = binop(BinOpKind::Eq, Expr::Int(1), Expr::Float(1.0));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn eq_tipos_distintos_da_false_sin_error() {
        // 1 == "1" → false (no error)
        let e = binop(BinOpKind::Eq, Expr::Int(1), Expr::Str("1".into()));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(false));
    }

    #[test]
    fn noteq_funciona() {
        let e = binop(BinOpKind::NotEq, Expr::Int(1), Expr::Int(2));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn lt_gt_lteq_gteq_numericos() {
        assert_eq!(
            eval_expr_test(binop(BinOpKind::Lt, Expr::Int(2), Expr::Int(3))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval_expr_test(binop(BinOpKind::Gt, Expr::Int(2), Expr::Int(3))).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            eval_expr_test(binop(BinOpKind::LtEq, Expr::Int(3), Expr::Int(3))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval_expr_test(binop(BinOpKind::GtEq, Expr::Int(2), Expr::Int(3))).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn comparacion_con_promocion_int_float() {
        // 2 < 2.5 → true
        let e = binop(BinOpKind::Lt, Expr::Int(2), Expr::Float(2.5));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn comparacion_de_strings_es_alfabetica() {
        let e = binop(
            BinOpKind::Lt,
            Expr::Str("abc".into()),
            Expr::Str("abd".into()),
        );
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn comparacion_entre_bool_es_type_error() {
        // Bool no se compara con <. Sí con ==.
        let e = binop(BinOpKind::Lt, Expr::Bool(true), Expr::Bool(false));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- BinOp: lógicos con short-circuit ----

    #[test]
    fn and_true_true_da_true() {
        let e = binop(BinOpKind::And, Expr::Bool(true), Expr::Bool(true));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn and_false_corta_y_no_evalua_derecho() {
        // El lado derecho es un Ident no definido. Si se evaluara, daría error.
        // Como `false and ...` corta, devuelve false sin error.
        let e = binop(
            BinOpKind::And,
            Expr::Bool(false),
            Expr::Ident("no_existe".into()),
        );
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(false));
    }

    #[test]
    fn or_true_corta_y_no_evalua_derecho() {
        let e = binop(
            BinOpKind::Or,
            Expr::Bool(true),
            Expr::Ident("no_existe".into()),
        );
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn or_false_true_da_true() {
        let e = binop(BinOpKind::Or, Expr::Bool(false), Expr::Bool(true));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn and_con_no_bool_izquierda_es_type_error() {
        let e = binop(BinOpKind::And, Expr::Int(1), Expr::Bool(true));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn and_con_no_bool_derecha_es_type_error() {
        // Para que el lado derecho se evalúe, el izquierdo debe ser true.
        let e = binop(BinOpKind::And, Expr::Bool(true), Expr::Int(1));
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- BinOp anidados ----

    #[test]
    fn expresion_anidada_2_mas_3_por_4_da_14() {
        // 2 + (3 * 4) — Stmt::Expr para verificar wiring completo.
        let inner = binop(BinOpKind::Mul, Expr::Int(3), Expr::Int(4));
        let outer = binop(BinOpKind::Add, Expr::Int(2), inner);
        assert_eq!(eval_expr_test(outer).unwrap(), Value::Int(14));
    }

    // ---- UnaryOp ----

    #[test]
    fn neg_int() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Int(5)),
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(-5));
    }

    #[test]
    fn neg_float() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Float(3.14)),
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Float(-3.14));
    }

    #[test]
    fn doble_negacion_devuelve_el_original() {
        // -(-7) = 7
        let inner = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Int(7)),
        };
        let outer = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(inner),
        };
        assert_eq!(eval_expr_test(outer).unwrap(), Value::Int(7));
    }

    #[test]
    fn neg_de_bool_es_type_error() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Bool(true)),
        };
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn neg_de_string_es_type_error() {
        let e = Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            operand: Box::new(Expr::Str("hola".into())),
        };
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- Stmt::Assign ----

    #[test]
    fn assign_define_variable_nueva_en_scope_local() {
        let env = Environment::new();
        let stmt = Stmt::Assign {
            name: "x".into(),
            type_: None,
            value: Expr::Int(42),
        };
        eval_stmt(&stmt, env.clone()).unwrap();

        assert_eq!(env.borrow().get("x"), Some(Value::Int(42)));
    }

    #[test]
    fn assign_reasigna_variable_existente_en_el_mismo_scope() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(1));

        let stmt = Stmt::Assign {
            name: "x".into(),
            type_: None,
            value: Expr::Int(99),
        };
        eval_stmt(&stmt, env.clone()).unwrap();

        assert_eq!(env.borrow().get("x"), Some(Value::Int(99)));
    }

    #[test]
    fn assign_desde_child_reasigna_en_el_padre_si_existe() {
        let global = Environment::new();
        global.borrow_mut().define("x", Value::Int(1));

        let child = Environment::new_child(global.clone());
        let stmt = Stmt::Assign {
            name: "x".into(),
            type_: None,
            value: Expr::Int(42),
        };
        eval_stmt(&stmt, child).unwrap();

        // El cambio se ve en el global.
        assert_eq!(global.borrow().get("x"), Some(Value::Int(42)));
    }

    #[test]
    fn assign_crea_local_si_la_variable_no_existe_en_la_cadena() {
        let global = Environment::new();
        let child = Environment::new_child(global.clone());

        let stmt = Stmt::Assign {
            name: "nueva".into(),
            type_: None,
            value: Expr::Int(7),
        };
        eval_stmt(&stmt, child.clone()).unwrap();

        // Solo existe en child, no se propagó al padre.
        assert_eq!(child.borrow().get("nueva"), Some(Value::Int(7)));
        assert_eq!(global.borrow().get("nueva"), None);
    }

    #[test]
    fn assign_ignora_la_anotacion_de_tipo() {
        // type_: Some("Int") con value String — no falla (tipado gradual,
        // sin checks en runtime todavía).
        let env = Environment::new();
        let stmt = Stmt::Assign {
            name: "x".into(),
            type_: Some("Int".into()),
            value: Expr::Str("soy un string".into()),
        };
        assert!(eval_stmt(&stmt, env.clone()).is_ok());
        assert_eq!(env.borrow().get("x"), Some(Value::Str("soy un string".into())));
    }

    // ---- Expr::Call (builtins) ----

    #[test]
    fn call_a_print_devuelve_null() {
        // print(...) escribe a stdout y devuelve Null. Verificamos el Value
        // de retorno; la salida real la chequeamos manualmente con hello.fitz.
        let env = Environment::new();
        register_builtins(&env);

        let call = Expr::Call {
            name: "print".into(),
            args: vec![Expr::Str("test".into())],
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Null);
    }

    #[test]
    fn call_a_funcion_no_definida_es_error() {
        let env = Environment::new();
        let call = Expr::Call {
            name: "noexiste".into(),
            args: vec![],
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::UndefinedFunction(_), .. })
        ));
    }

    #[test]
    fn call_a_no_funcion_es_type_error() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(5));

        let call = Expr::Call {
            name: "x".into(),
            args: vec![],
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn call_evalua_args_antes_de_invocar() {
        // El arg `1 + 2` debe llegar al builtin como Int(3), no como BinOp.
        // Como print no nos deja inspeccionar, usamos un assert indirecto:
        // si el eval de args fallara, daría error. Si llega bien, Null.
        let env = Environment::new();
        register_builtins(&env);

        let call = Expr::Call {
            name: "print".into(),
            args: vec![Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1)),
                right: Box::new(Expr::Int(2)),
            }],
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Null);
    }

    // ---- Expr::StrInterp ----

    #[test]
    fn str_interp_solo_con_literales_concatena() {
        let e = Expr::StrInterp(vec![
            StrPart::Lit("hola ".into()),
            StrPart::Lit("mundo".into()),
        ]);
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("hola mundo".into()));
    }

    #[test]
    fn str_interp_interpola_ident() {
        let env = Environment::new();
        env.borrow_mut().define("name", Value::Str("Fitz".into()));

        let e = Expr::StrInterp(vec![
            StrPart::Lit("Hola, ".into()),
            StrPart::Expr(Expr::Ident("name".into())),
            StrPart::Lit("!".into()),
        ]);
        assert_eq!(
            eval_expr(&e, env).unwrap(),
            Value::Str("Hola, Fitz!".into())
        );
    }

    #[test]
    fn str_interp_convierte_int_a_string() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(42));

        let e = Expr::StrInterp(vec![
            StrPart::Lit("x es ".into()),
            StrPart::Expr(Expr::Ident("x".into())),
        ]);
        assert_eq!(eval_expr(&e, env).unwrap(), Value::Str("x es 42".into()));
    }

    #[test]
    fn str_interp_evalua_expresiones_internas() {
        // "{1 + 2}" → "3"
        let e = Expr::StrInterp(vec![
            StrPart::Expr(Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1)),
                right: Box::new(Expr::Int(2)),
            }),
        ]);
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("3".into()));
    }

    // ---- Integración mini: hello.fitz a mano ----

    // ---- FnDef + Return + Call (user-defined) ----

    /// Helper: arma `fn name(params) { body }` como Stmt.
    fn fn_def(name: &str, params: Vec<&str>, body: Vec<Stmt>) -> Stmt {
        Stmt::FnDef {
            name: name.into(),
            params: params.into_iter().map(|p| crate::ast::Param {
                name: p.into(),
                type_: None,
            }).collect(),
            return_type: None,
            body,
            is_async: false,
        }
    }

    #[test]
    fn fn_sin_return_devuelve_null() {
        // fn f() { } ; f()
        let env = Environment::new();
        eval_stmt(&fn_def("f", vec![], vec![]), env.clone()).unwrap();

        let call = Expr::Call { name: "f".into(), args: vec![] };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Null);
    }

    #[test]
    fn fn_return_constante() {
        // fn f() { return 42 } ; f()
        let env = Environment::new();
        eval_stmt(
            &fn_def("f", vec![], vec![Stmt::Return(Expr::Int(42))]),
            env.clone(),
        ).unwrap();

        let call = Expr::Call { name: "f".into(), args: vec![] };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(42));
    }

    #[test]
    fn fn_con_un_param_arrow_style() {
        // fn double(n) => n * 2 → body es vec![Return(n * 2)]
        // double(7) → 14
        let env = Environment::new();
        let body = vec![Stmt::Return(Expr::BinOp {
            op: BinOpKind::Mul,
            left: Box::new(Expr::Ident("n".into())),
            right: Box::new(Expr::Int(2)),
        })];
        eval_stmt(&fn_def("double", vec!["n"], body), env.clone()).unwrap();

        let call = Expr::Call {
            name: "double".into(),
            args: vec![Expr::Int(7)],
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(14));
    }

    #[test]
    fn fn_con_dos_params_suma() {
        // fn add(a, b) => a + b ; add(3, 4) → 7
        let env = Environment::new();
        let body = vec![Stmt::Return(Expr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(Expr::Ident("a".into())),
            right: Box::new(Expr::Ident("b".into())),
        })];
        eval_stmt(&fn_def("add", vec!["a", "b"], body), env.clone()).unwrap();

        let call = Expr::Call {
            name: "add".into(),
            args: vec![Expr::Int(3), Expr::Int(4)],
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(7));
    }

    #[test]
    fn fn_ve_variables_del_scope_donde_se_definio() {
        // Closure básico: la función accede a `x` del scope global.
        //
        //   x = 10
        //   fn get_x() => x
        //   get_x()  → 10
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(10));

        let body = vec![Stmt::Return(Expr::Ident("x".into()))];
        eval_stmt(&fn_def("get_x", vec![], body), env.clone()).unwrap();

        let call = Expr::Call { name: "get_x".into(), args: vec![] };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(10));
    }

    #[test]
    fn fn_param_sombrea_variable_externa() {
        // x = 100; fn f(x) => x ; f(7) → 7 (no 100)
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(100));

        let body = vec![Stmt::Return(Expr::Ident("x".into()))];
        eval_stmt(&fn_def("f", vec!["x"], body), env.clone()).unwrap();

        let call = Expr::Call {
            name: "f".into(),
            args: vec![Expr::Int(7)],
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(7));
    }

    #[test]
    fn fn_con_pocos_args_es_error() {
        // fn f(a, b) ... ; f(1)
        let env = Environment::new();
        eval_stmt(
            &fn_def("f", vec!["a", "b"], vec![Stmt::Return(Expr::Int(0))]),
            env.clone(),
        ).unwrap();

        let call = Expr::Call {
            name: "f".into(),
            args: vec![Expr::Int(1)],
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError {
                kind: ErrorKind::WrongArgCount { expected: 2, found: 1 }, ..
            })
        ));
    }

    #[test]
    fn fn_con_muchos_args_es_error() {
        let env = Environment::new();
        eval_stmt(
            &fn_def("f", vec![], vec![Stmt::Return(Expr::Int(0))]),
            env.clone(),
        ).unwrap();

        let call = Expr::Call {
            name: "f".into(),
            args: vec![Expr::Int(1), Expr::Int(2)],
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError {
                kind: ErrorKind::WrongArgCount { expected: 0, found: 2 }, ..
            })
        ));
    }

    #[test]
    fn return_fuera_de_fn_es_error() {
        // En el top level, `return 5` no tiene caller que lo intercepte.
        let result = eval(vec![Stmt::Return(Expr::Int(5))]);
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::ReturnOutsideFunction
        ));
    }

    #[test]
    fn fn_con_body_de_varias_sentencias() {
        // fn f(n) {
        //     x = n * 2
        //     return x + 1
        // }
        // f(5) → 11
        let env = Environment::new();
        let body = vec![
            Stmt::Assign {
                name: "x".into(),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("n".into())),
                    right: Box::new(Expr::Int(2)),
                },
            },
            Stmt::Return(Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Ident("x".into())),
                right: Box::new(Expr::Int(1)),
            }),
        ];
        eval_stmt(&fn_def("f", vec!["n"], body), env.clone()).unwrap();

        let call = Expr::Call {
            name: "f".into(),
            args: vec![Expr::Int(5)],
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(11));
    }

    #[test]
    fn return_corta_la_ejecucion_del_body() {
        // fn f() {
        //     return 1
        //     return 2   ← nunca se ejecuta
        // }
        let env = Environment::new();
        let body = vec![
            Stmt::Return(Expr::Int(1)),
            Stmt::Return(Expr::Int(2)),
        ];
        eval_stmt(&fn_def("f", vec![], body), env.clone()).unwrap();

        let call = Expr::Call { name: "f".into(), args: vec![] };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(1));
    }

    // ---- Expr::If ----

    /// Helper: arma `if cond { then } else? { else_ }`.
    fn if_expr(cond: Expr, then: Vec<Stmt>, else_: Option<Vec<Stmt>>) -> Expr {
        Expr::If { condition: Box::new(cond), then, else_ }
    }

    #[test]
    fn if_true_sin_else_devuelve_valor_del_then() {
        // if true { 7 } → 7
        let e = if_expr(Expr::Bool(true), vec![Stmt::Expr(Expr::Int(7))], None);
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(7));
    }

    #[test]
    fn if_false_sin_else_devuelve_null() {
        let e = if_expr(Expr::Bool(false), vec![Stmt::Expr(Expr::Int(7))], None);
        assert_eq!(eval_expr_test(e).unwrap(), Value::Null);
    }

    #[test]
    fn if_else_toma_la_rama_correcta() {
        // if true { 1 } else { 2 } → 1
        let then = vec![Stmt::Expr(Expr::Int(1))];
        let else_ = vec![Stmt::Expr(Expr::Int(2))];
        let e = if_expr(Expr::Bool(true), then.clone(), Some(else_.clone()));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(1));

        let e = if_expr(Expr::Bool(false), then, Some(else_));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(2));
    }

    #[test]
    fn if_condicion_no_bool_es_type_error() {
        // if 1 { ... } → error (no truthy coercion).
        let e = if_expr(Expr::Int(1), vec![], None);
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn if_evalua_solo_la_rama_correspondiente() {
        // El then es un Ident no definido. Si se evaluara, daría error.
        // Como cond es false, no se toca → resultado del else.
        let then = vec![Stmt::Expr(Expr::Ident("no_existe".into()))];
        let else_ = vec![Stmt::Expr(Expr::Int(99))];
        let e = if_expr(Expr::Bool(false), then, Some(else_));
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(99));
    }

    #[test]
    fn variables_definidas_dentro_del_if_persisten_afuera() {
        // x = 1
        // if x == 1 { y = 99 }
        // print(y)  → "99"
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(1));

        let if_stmt = Stmt::Expr(if_expr(
            Expr::BinOp {
                op: BinOpKind::Eq,
                left: Box::new(Expr::Ident("x".into())),
                right: Box::new(Expr::Int(1)),
            },
            vec![Stmt::Assign {
                name: "y".into(),
                type_: None,
                value: Expr::Int(99),
            }],
            None,
        ));
        eval_stmt(&if_stmt, env.clone()).unwrap();

        assert_eq!(env.borrow().get("y"), Some(Value::Int(99)));
    }

    #[test]
    fn else_if_anidado_funciona() {
        // if false { 1 } else if true { 2 } else { 3 } → 2
        //
        // El parser modela `else if` como `else_: vec![Stmt::Expr(Expr::If)]`.
        let inner = if_expr(
            Expr::Bool(true),
            vec![Stmt::Expr(Expr::Int(2))],
            Some(vec![Stmt::Expr(Expr::Int(3))]),
        );
        let outer = if_expr(
            Expr::Bool(false),
            vec![Stmt::Expr(Expr::Int(1))],
            Some(vec![Stmt::Expr(inner)]),
        );
        assert_eq!(eval_expr_test(outer).unwrap(), Value::Int(2));
    }

    #[test]
    fn if_como_expresion_en_assign() {
        // let r = if true { 42 } else { 0 }
        let env = Environment::new();
        let stmt = Stmt::Assign {
            name: "r".into(),
            type_: None,
            value: if_expr(
                Expr::Bool(true),
                vec![Stmt::Expr(Expr::Int(42))],
                Some(vec![Stmt::Expr(Expr::Int(0))]),
            ),
        };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Int(42)));
    }

    #[test]
    fn factorial_recursivo_funciona() {
        // El test que ata todo: closures + recursión + if + comparación
        // + BinOp + Return.
        //
        //   fn factorial(n) {
        //       if n == 0 { return 1 }
        //       return n * factorial(n - 1)
        //   }
        //   factorial(5) → 120
        let env = Environment::new();

        let body = vec![
            Stmt::Expr(if_expr(
                Expr::BinOp {
                    op: BinOpKind::Eq,
                    left: Box::new(Expr::Ident("n".into())),
                    right: Box::new(Expr::Int(0)),
                },
                vec![Stmt::Return(Expr::Int(1))],
                None,
            )),
            Stmt::Return(Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::Ident("n".into())),
                right: Box::new(Expr::Call {
                    name: "factorial".into(),
                    args: vec![Expr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(Expr::Ident("n".into())),
                        right: Box::new(Expr::Int(1)),
                    }],
                }),
            }),
        ];

        eval_stmt(&fn_def("factorial", vec!["n"], body), env.clone()).unwrap();

        let call = Expr::Call {
            name: "factorial".into(),
            args: vec![Expr::Int(5)],
        };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(120));
    }

    // ---- Expr::Match ----

    use crate::ast::MatchArm;

    fn match_arm(pattern: Pattern, body: Expr) -> MatchArm {
        MatchArm { pattern, body }
    }

    #[test]
    fn match_wildcard_siempre_matchea() {
        // match 42 { _ => 99 } → 99
        let e = Expr::Match {
            value: Box::new(Expr::Int(42)),
            arms: vec![match_arm(Pattern::Wildcard, Expr::Int(99))],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(99));
    }

    #[test]
    fn match_ident_bindea_el_valor() {
        // match 42 { n => n + 1 } → 43
        let e = Expr::Match {
            value: Box::new(Expr::Int(42)),
            arms: vec![match_arm(
                Pattern::Ident("n".into()),
                Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Ident("n".into())),
                    right: Box::new(Expr::Int(1)),
                },
            )],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(43));
    }

    #[test]
    fn match_toma_el_primer_arm_que_matchea() {
        // match "hola" {
        //     x => "primer arm: ${x}",
        //     _ => "segundo arm (no se toca)",
        // }
        let e = Expr::Match {
            value: Box::new(Expr::Str("hola".into())),
            arms: vec![
                match_arm(
                    Pattern::Ident("x".into()),
                    Expr::StrInterp(vec![
                        StrPart::Lit("primer arm: ".into()),
                        StrPart::Expr(Expr::Ident("x".into())),
                    ]),
                ),
                match_arm(Pattern::Wildcard, Expr::Str("segundo arm".into())),
            ],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("primer arm: hola".into()));
    }

    #[test]
    fn match_binding_vive_solo_en_el_arm() {
        // El binding `n` no debe escapar al scope contenedor.
        let env = Environment::new();
        let e = Expr::Match {
            value: Box::new(Expr::Int(7)),
            arms: vec![match_arm(Pattern::Ident("n".into()), Expr::Ident("n".into()))],
        };
        eval_expr(&e, env.clone()).unwrap();

        // `n` no quedó definida en el scope de afuera.
        assert_eq!(env.borrow().get("n"), None);
    }

    #[test]
    fn match_ok_binding_es_error_explicito() {
        // match x { Ok(v) => v } → error "Result requiere Fase 3"
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(5));

        let e = Expr::Match {
            value: Box::new(Expr::Ident("x".into())),
            arms: vec![match_arm(
                Pattern::OkBinding("v".into()),
                Expr::Ident("v".into()),
            )],
        };
        match eval_expr(&e, env) {
            Err(EvalSignal::Error(err)) => {
                assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
                assert!(err.message.contains("Result"));
            }
            _ => panic!("se esperaba error de patrón Ok no soportado"),
        }
    }

    #[test]
    fn match_err_binding_es_error_explicito() {
        let env = Environment::new();
        env.borrow_mut().define("x", Value::Int(5));

        let e = Expr::Match {
            value: Box::new(Expr::Ident("x".into())),
            arms: vec![match_arm(
                Pattern::ErrBinding("e".into()),
                Expr::Ident("e".into()),
            )],
        };
        assert!(matches!(
            eval_expr(&e, env).unwrap_err(),
            EvalSignal::Error(_)
        ));
    }

    #[test]
    fn match_literal_int_matchea() {
        // match 2 { 1 => "uno", 2 => "dos", _ => "otro" } → "dos"
        let e = Expr::Match {
            value: Box::new(Expr::Int(2)),
            arms: vec![
                match_arm(Pattern::Int(1), Expr::Str("uno".into())),
                match_arm(Pattern::Int(2), Expr::Str("dos".into())),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into())),
            ],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("dos".into()));
    }

    #[test]
    fn match_literal_int_no_coerciona_a_float() {
        // match 1.0 { 1 => "int", _ => "no-int" } → "no-int"
        // (En match, igualdad es estructural — sin la coerción del `==`).
        let e = Expr::Match {
            value: Box::new(Expr::Float(1.0)),
            arms: vec![
                match_arm(Pattern::Int(1), Expr::Str("int".into())),
                match_arm(Pattern::Wildcard, Expr::Str("no-int".into())),
            ],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("no-int".into()));
    }

    #[test]
    fn match_literal_str_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Str("hola".into())),
            arms: vec![
                match_arm(Pattern::Str("chau".into()), Expr::Int(1)),
                match_arm(Pattern::Str("hola".into()), Expr::Int(2)),
                match_arm(Pattern::Wildcard, Expr::Int(0)),
            ],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Int(2));
    }

    #[test]
    fn match_literal_bool_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Bool(true)),
            arms: vec![
                match_arm(Pattern::Bool(false), Expr::Str("falso".into())),
                match_arm(Pattern::Bool(true), Expr::Str("verdadero".into())),
            ],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("verdadero".into()));
    }

    #[test]
    fn match_literal_null_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Null),
            arms: vec![
                match_arm(Pattern::Null, Expr::Str("es null".into())),
                match_arm(Pattern::Wildcard, Expr::Str("no null".into())),
            ],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("es null".into()));
    }

    #[test]
    fn match_int_negativo_matchea() {
        let e = Expr::Match {
            value: Box::new(Expr::Int(-5)),
            arms: vec![
                match_arm(Pattern::Int(-5), Expr::Str("menos cinco".into())),
                match_arm(Pattern::Wildcard, Expr::Str("otro".into())),
            ],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("menos cinco".into()));
    }

    #[test]
    fn match_literales_caen_a_ident_si_ninguno_matchea() {
        // match 42 { 1 => "uno", n => "default ${n}" }
        let e = Expr::Match {
            value: Box::new(Expr::Int(42)),
            arms: vec![
                match_arm(Pattern::Int(1), Expr::Str("uno".into())),
                match_arm(
                    Pattern::Ident("n".into()),
                    Expr::StrInterp(vec![
                        StrPart::Lit("default ".into()),
                        StrPart::Expr(Expr::Ident("n".into())),
                    ]),
                ),
            ],
        };
        assert_eq!(eval_expr_test(e).unwrap(), Value::Str("default 42".into()));
    }

    #[test]
    fn match_sin_arms_es_error() {
        let e = Expr::Match {
            value: Box::new(Expr::Int(1)),
            arms: vec![],
        };
        assert!(matches!(
            eval_expr_test(e).unwrap_err(),
            EvalSignal::Error(_)
        ));
    }

    // ---- while / loop ----

    #[test]
    fn while_itera_hasta_que_cond_es_falsa() {
        // i = 0
        // total = 0
        // while i < 5 { total = total + i; i = i + 1 }
        // total → 0+1+2+3+4 = 10
        let env = Environment::new();
        env.borrow_mut().define("i", Value::Int(0));
        env.borrow_mut().define("total", Value::Int(0));

        let stmt = Stmt::While {
            condition: Expr::BinOp {
                op: BinOpKind::Lt,
                left: Box::new(Expr::Ident("i".into())),
                right: Box::new(Expr::Int(5)),
            },
            body: vec![
                Stmt::Assign {
                    name: "total".into(),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("total".into())),
                        right: Box::new(Expr::Ident("i".into())),
                    },
                },
                Stmt::Assign {
                    name: "i".into(),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("i".into())),
                        right: Box::new(Expr::Int(1)),
                    },
                },
            ],
        };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(10)));
    }

    #[test]
    fn while_con_cond_inicialmente_falsa_no_itera() {
        let env = Environment::new();
        env.borrow_mut().define("counter", Value::Int(0));

        let stmt = Stmt::While {
            condition: Expr::Bool(false),
            body: vec![Stmt::Assign {
                name: "counter".into(),
                type_: None,
                value: Expr::Int(99),
            }],
        };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("counter"), Some(Value::Int(0)));
    }

    #[test]
    fn while_break_termina_loop() {
        let env = Environment::new();
        env.borrow_mut().define("i", Value::Int(0));

        // while true { i = i + 1; if i == 3 { break } }
        let stmt = Stmt::While {
            condition: Expr::Bool(true),
            body: vec![
                Stmt::Assign {
                    name: "i".into(),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("i".into())),
                        right: Box::new(Expr::Int(1)),
                    },
                },
                Stmt::Expr(Expr::If {
                    condition: Box::new(Expr::BinOp {
                        op: BinOpKind::Eq,
                        left: Box::new(Expr::Ident("i".into())),
                        right: Box::new(Expr::Int(3)),
                    }),
                    then: vec![Stmt::Break],
                    else_: None,
                }),
            ],
        };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("i"), Some(Value::Int(3)));
    }

    #[test]
    fn while_continue_salta_a_la_siguiente_iteracion() {
        let env = Environment::new();
        env.borrow_mut().define("i", Value::Int(0));
        env.borrow_mut().define("total", Value::Int(0));

        // while i < 5 {
        //   i = i + 1
        //   if i == 3 { continue }
        //   total = total + i
        // }
        // total → 1+2+4+5 = 12 (saltó el 3)
        let stmt = Stmt::While {
            condition: Expr::BinOp {
                op: BinOpKind::Lt,
                left: Box::new(Expr::Ident("i".into())),
                right: Box::new(Expr::Int(5)),
            },
            body: vec![
                Stmt::Assign {
                    name: "i".into(),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("i".into())),
                        right: Box::new(Expr::Int(1)),
                    },
                },
                Stmt::Expr(Expr::If {
                    condition: Box::new(Expr::BinOp {
                        op: BinOpKind::Eq,
                        left: Box::new(Expr::Ident("i".into())),
                        right: Box::new(Expr::Int(3)),
                    }),
                    then: vec![Stmt::Continue],
                    else_: None,
                }),
                Stmt::Assign {
                    name: "total".into(),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("total".into())),
                        right: Box::new(Expr::Ident("i".into())),
                    },
                },
            ],
        };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(12)));
    }

    #[test]
    fn while_cond_no_bool_es_type_error() {
        let env = Environment::new();
        let stmt = Stmt::While {
            condition: Expr::Int(1),
            body: vec![],
        };
        assert!(matches!(
            eval_stmt(&stmt, env).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    #[test]
    fn loop_infinito_se_corta_con_break() {
        let env = Environment::new();
        env.borrow_mut().define("count", Value::Int(0));

        // loop {
        //   count = count + 1
        //   if count == 5 { break }
        // }
        let stmt = Stmt::Loop {
            body: vec![
                Stmt::Assign {
                    name: "count".into(),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("count".into())),
                        right: Box::new(Expr::Int(1)),
                    },
                },
                Stmt::Expr(Expr::If {
                    condition: Box::new(Expr::BinOp {
                        op: BinOpKind::Eq,
                        left: Box::new(Expr::Ident("count".into())),
                        right: Box::new(Expr::Int(5)),
                    }),
                    then: vec![Stmt::Break],
                    else_: None,
                }),
            ],
        };
        eval_stmt(&stmt, env.clone()).unwrap();
        assert_eq!(env.borrow().get("count"), Some(Value::Int(5)));
    }

    #[test]
    fn return_dentro_de_while_dentro_de_fn_propaga() {
        // fn f() {
        //   while true { return 42 }
        // }
        // f() → 42
        let env = Environment::new();
        let body = vec![Stmt::While {
            condition: Expr::Bool(true),
            body: vec![Stmt::Return(Expr::Int(42))],
        }];
        eval_stmt(&fn_def("f", vec![], body), env.clone()).unwrap();

        let call = Expr::Call { name: "f".into(), args: vec![] };
        assert_eq!(eval_expr(&call, env).unwrap(), Value::Int(42));
    }

    // ---- Stmt::TypeDef ----

    use crate::ast::Field;

    fn make_field(name: &str, type_: &str, nullable: bool) -> Field {
        Field {
            name: name.into(),
            type_: type_.into(),
            nullable,
            default: None,
        }
    }

    #[test]
    fn type_def_registra_el_tipo_en_el_env() {
        // type User { id: Int, name: Str }
        let env = Environment::new();
        let stmt = Stmt::TypeDef {
            name: "User".into(),
            fields: vec![
                make_field("id", "Int", false),
                make_field("name", "Str", false),
            ],
        };
        eval_stmt(&stmt, env.clone()).unwrap();

        let v = env.borrow().get("User").expect("User no quedó en el env");
        match v {
            Value::Type { name, fields } => {
                assert_eq!(name, "User");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "id");
                assert_eq!(fields[1].name, "name");
            }
            other => panic!("se esperaba Value::Type, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn type_value_type_name_es_type() {
        let t = Value::Type {
            name: "Foo".into(),
            fields: vec![],
        };
        assert_eq!(t.type_name(), "Type");
    }

    #[test]
    fn type_se_puede_referenciar_como_ident_sin_error() {
        // Después de definir un type, `User` como Expr::Ident lo encuentra.
        let env = Environment::new();
        eval_stmt(
            &Stmt::TypeDef {
                name: "User".into(),
                fields: vec![make_field("id", "Int", false)],
            },
            env.clone(),
        ).unwrap();

        let result = eval_expr(&Expr::Ident("User".into()), env).unwrap();
        assert!(matches!(result, Value::Type { .. }));
    }

    #[test]
    fn llamar_un_type_como_funcion_es_type_error() {
        // User(1) sin struct literals → TypeMismatch porque Type no es callable.
        // Esto es deuda explícita: la instanciación viene en Fase 3.
        let env = Environment::new();
        eval_stmt(
            &Stmt::TypeDef {
                name: "User".into(),
                fields: vec![make_field("id", "Int", false)],
            },
            env.clone(),
        ).unwrap();

        let call = Expr::Call {
            name: "User".into(),
            args: vec![Expr::Int(1)],
        };
        assert!(matches!(
            eval_expr(&call, env).unwrap_err(),
            EvalSignal::Error(FitzError { kind: ErrorKind::TypeMismatch { .. }, .. })
        ));
    }

    // ---- Criterio de Fase 2: el programa completo ----

    #[test]
    fn criterio_fase_2_corre_end_to_end() {
        // El programa del roadmap:
        //   name = "Fitz"
        //   x = 10 + 5
        //   print("Hola {name}, x es {x}")
        //   fn double(n) => n * 2
        //   print(double(x))
        //
        // Output esperado (vía stdout, no chequeado acá):
        //   Hola Fitz, x es 15
        //   30
        let program = vec![
            Stmt::Assign {
                name: "name".into(),
                type_: None,
                value: Expr::Str("Fitz".into()),
            },
            Stmt::Assign {
                name: "x".into(),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(10)),
                    right: Box::new(Expr::Int(5)),
                },
            },
            Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::StrInterp(vec![
                    StrPart::Lit("Hola ".into()),
                    StrPart::Expr(Expr::Ident("name".into())),
                    StrPart::Lit(", x es ".into()),
                    StrPart::Expr(Expr::Ident("x".into())),
                ])],
            }),
            fn_def(
                "double",
                vec!["n"],
                vec![Stmt::Return(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("n".into())),
                    right: Box::new(Expr::Int(2)),
                })],
            ),
            Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::Call {
                    name: "double".into(),
                    args: vec![Expr::Ident("x".into())],
                }],
            }),
        ];
        assert!(eval(program).is_ok());
    }

    /// Test de integración: el pipeline completo (lexer → parser → eval)
    /// sobre el programa exacto del criterio de Fase 2 escrito como source.
    /// Si esto pasa, las tres fases hablan bien entre sí.
    #[test]
    fn integracion_criterio_fase_2_lexer_parser_evaluator() {
        let source = r#"
name = "Fitz"
x = 10 + 5
print("Hola {name}, x es {x}")

fn double(n) => n * 2
print(double(x))
"#;
        let tokens = crate::lexer::tokenize(source).expect("lexer falla");
        let program = crate::parser::parse(tokens).expect("parser falla");
        eval(program).expect("evaluator falla");
    }

    #[test]
    fn integracion_factorial_recursivo_end_to_end() {
        // Test de pipeline con recursión + if + return + cierre.
        // Verifica que el evaluator atrapa Return correctamente vía signal.
        let source = r#"
fn factorial(n) {
    if n == 0 {
        return 1
    }
    return n * factorial(n - 1)
}
print(factorial(5))
"#;
        let tokens = crate::lexer::tokenize(source).expect("lexer falla");
        let program = crate::parser::parse(tokens).expect("parser falla");
        eval(program).expect("evaluator falla");
    }

    #[test]
    fn hello_fitz_corre_sin_error() {
        // Réplica del AST equivalente a:
        //   name = "Patagonia"
        //   print("Hola, {name}!")
        //
        // Verifica que el camino Assign → StrInterp → Call (builtin) funciona
        // end-to-end. La salida real se ve con `cargo run -- run examples/hello.fitz`.
        let program = vec![
            Stmt::Assign {
                name: "name".into(),
                type_: None,
                value: Expr::Str("Patagonia".into()),
            },
            Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::StrInterp(vec![
                    StrPart::Lit("Hola, ".into()),
                    StrPart::Expr(Expr::Ident("name".into())),
                    StrPart::Lit("!".into()),
                ])],
            }),
        ];
        assert!(eval(program).is_ok());
    }

    // -----------------------------------------------------------------------
    // Tests — listas, mapas, rangos, indexing, for (Fase 3, paso 1)
    // -----------------------------------------------------------------------

    /// Helper: parsea y evalúa programa entero. Devuelve el env final.
    fn parse_and_eval(src: &str) -> FitzResult<()> {
        let tokens = crate::lexer::tokenize(src).expect("la fuente debe tokenizar");
        let program = crate::parser::parse(tokens).expect("la fuente debe parsear");
        eval(program)
    }

    /// Como `parse_and_eval`, pero conserva el env para inspeccionarlo.
    /// Útil cuando querés assertear valores específicos al final.
    fn parse_eval_into_env(src: &str) -> (EnvRef, FitzResult<()>) {
        let tokens = crate::lexer::tokenize(src).expect("la fuente debe tokenizar");
        let program = crate::parser::parse(tokens).expect("la fuente debe parsear");
        let env = Environment::new();
        register_builtins(&env);
        for stmt in &program {
            if let Err(signal) = eval_stmt(stmt, env.clone()) {
                return (env, Err(signal_to_error(signal)));
            }
        }
        (env, Ok(()))
    }

    // ---- List literal ----

    #[test]
    fn evalua_list_vacia() {
        let v = eval_expr_test(Expr::List(vec![])).unwrap();
        assert_eq!(v, Value::List(vec![]));
    }

    #[test]
    fn evalua_list_con_literales() {
        let v = eval_expr_test(Expr::List(vec![
            Expr::Int(1),
            Expr::Int(2),
            Expr::Int(3),
        ])).unwrap();
        assert_eq!(v, Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
    }

    #[test]
    fn evalua_list_con_expresiones() {
        // [1 + 1, 2 * 2]
        let v = eval_expr_test(Expr::List(vec![
            Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1)),
                right: Box::new(Expr::Int(1)),
            },
            Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::Int(2)),
                right: Box::new(Expr::Int(2)),
            },
        ])).unwrap();
        assert_eq!(v, Value::List(vec![Value::Int(2), Value::Int(4)]));
    }

    // ---- Map literal ----

    #[test]
    fn evalua_map_vacio() {
        let v = eval_expr_test(Expr::Map(vec![])).unwrap();
        assert_eq!(v, Value::Map(vec![]));
    }

    #[test]
    fn evalua_map_con_pares() {
        let v = eval_expr_test(Expr::Map(vec![
            (Expr::Str("a".into()), Expr::Int(1)),
            (Expr::Str("b".into()), Expr::Int(2)),
        ])).unwrap();
        assert_eq!(
            v,
            Value::Map(vec![
                (Value::Str("a".into()), Value::Int(1)),
                (Value::Str("b".into()), Value::Int(2)),
            ]),
        );
    }

    // ---- Range literal ----

    #[test]
    fn evalua_range_simple() {
        let v = eval_expr_test(Expr::Range {
            start: Box::new(Expr::Int(0)),
            end: Box::new(Expr::Int(10)),
        }).unwrap();
        assert_eq!(v, Value::Range { start: 0, end: 10 });
    }

    #[test]
    fn evalua_range_con_float_es_error() {
        // 0..1.5 — float no es Int.
        let res = eval_expr_test(Expr::Range {
            start: Box::new(Expr::Int(0)),
            end: Box::new(Expr::Float(1.5)),
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(matches!(e.kind, ErrorKind::TypeMismatch { .. })),
            _ => panic!("se esperaba Error"),
        }
    }

    // ---- Indexing ----

    #[test]
    fn index_list_con_int_valido() {
        // [10, 20, 30][1] → 20
        let v = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(10), Expr::Int(20), Expr::Int(30)])),
            index: Box::new(Expr::Int(1)),
        }).unwrap();
        assert_eq!(v, Value::Int(20));
    }

    #[test]
    fn index_list_fuera_de_rango_es_error() {
        // [1, 2][5]
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1), Expr::Int(2)])),
            index: Box::new(Expr::Int(5)),
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => {
                assert!(e.message.contains("fuera de rango"));
            }
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_list_negativo_es_error() {
        // [1, 2][-1] — sin Python-style por ahora
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1), Expr::Int(2)])),
            index: Box::new(Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(Expr::Int(1)),
            }),
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(e.message.contains("negativo")),
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_list_con_string_es_type_error() {
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::List(vec![Expr::Int(1)])),
            index: Box::new(Expr::Str("a".into())),
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(matches!(e.kind, ErrorKind::TypeMismatch { .. })),
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_map_clave_existente() {
        // {"a": 1, "b": 2}["b"] → 2
        let v = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Map(vec![
                (Expr::Str("a".into()), Expr::Int(1)),
                (Expr::Str("b".into()), Expr::Int(2)),
            ])),
            index: Box::new(Expr::Str("b".into())),
        }).unwrap();
        assert_eq!(v, Value::Int(2));
    }

    #[test]
    fn index_map_clave_inexistente_es_error() {
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Map(vec![
                (Expr::Str("a".into()), Expr::Int(1)),
            ])),
            index: Box::new(Expr::Str("z".into())),
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(e.message.contains("clave no encontrada")),
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_sobre_int_es_type_error() {
        // 42[0] — Int no se indexa
        let res = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Int(42)),
            index: Box::new(Expr::Int(0)),
        });
        let err = res.unwrap_err();
        match err {
            EvalSignal::Error(e) => assert!(matches!(e.kind, ErrorKind::TypeMismatch { .. })),
            _ => panic!("se esperaba Error"),
        }
    }

    #[test]
    fn index_encadenado_funciona() {
        // [[1, 2], [3, 4]][0][1] → 2
        let v = eval_expr_test(Expr::Index {
            object: Box::new(Expr::Index {
                object: Box::new(Expr::List(vec![
                    Expr::List(vec![Expr::Int(1), Expr::Int(2)]),
                    Expr::List(vec![Expr::Int(3), Expr::Int(4)]),
                ])),
                index: Box::new(Expr::Int(0)),
            }),
            index: Box::new(Expr::Int(1)),
        }).unwrap();
        assert_eq!(v, Value::Int(2));
    }

    // ---- for ----

    #[test]
    fn for_sobre_lista_itera_los_elementos() {
        // total = 1 + 2 + 3 + 4 = 10
        let src = r#"
total = 0
for x in [1, 2, 3, 4] {
    total = total + x
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(10)));
    }

    #[test]
    fn for_sobre_range_itera_inclusivo_exclusivo() {
        // 0..3 → 0 + 1 + 2 = 3 (la cota superior es exclusiva)
        let src = r#"
total = 0
for i in 0..3 {
    total = total + i
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(3)));
    }

    #[test]
    fn for_sobre_lista_vacia_no_itera() {
        let src = r#"
ran = false
for x in [] {
    ran = true
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("ran"), Some(Value::Bool(false)));
    }

    #[test]
    fn for_con_break_corta_iteracion() {
        // Corta cuando i == 3 → last queda en 2.
        let src = r#"
last = 0
for i in 0..10 {
    if i == 3 {
        break
    }
    last = i
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("last"), Some(Value::Int(2)));
    }

    #[test]
    fn for_con_continue_salta_iteracion() {
        // 0..5, saltea i == 2 → 0 + 1 + 3 + 4 = 8.
        let src = r#"
total = 0
for i in 0..5 {
    if i == 2 {
        continue
    }
    total = total + i
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(8)));
    }

    #[test]
    fn for_sobre_map_es_error_explicito() {
        let src = r#"
for x in {"a": 1} {
    print(x)
}
"#;
        let res = parse_and_eval(src);
        let err = res.unwrap_err();
        assert!(err.message.contains("Map"));
    }

    #[test]
    fn for_sobre_int_es_type_error() {
        let src = r#"
for x in 42 {
    print(x)
}
"#;
        let res = parse_and_eval(src);
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[test]
    fn for_loop_var_persiste_despues_del_loop() {
        // Consistente con la política de bloques de Fitz: las variables
        // del body (incluida la variable de iteración) persisten en el
        // scope contenedor. Tras 0..3, i = 2 e last = 2.
        let src = r#"
for i in 0..3 {
    last = i
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("i"), Some(Value::Int(2)));
        assert_eq!(env.borrow().get("last"), Some(Value::Int(2)));
    }

    #[test]
    fn for_anidado_funciona() {
        // 3 * 3 = 9 iteraciones totales.
        let src = r#"
total = 0
for i in 0..3 {
    for j in 0..3 {
        total = total + 1
    }
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("total"), Some(Value::Int(9)));
    }

    // ---- Pattern::Range ----

    #[test]
    fn pattern_range_matchea_valor_dentro() {
        let src = r#"
let n = 5
let r = match n {
    0..10 => "in"
    _     => "out"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("in".into())));
    }

    #[test]
    fn pattern_range_no_matchea_valor_fuera() {
        let src = r#"
let n = 15
let r = match n {
    0..10 => "in"
    _     => "out"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("out".into())));
    }

    #[test]
    fn pattern_range_es_exclusivo_en_el_fin() {
        // n = 10 con patrón 0..10 NO matchea (exclusivo). El segundo arm sí.
        let src = r#"
let n = 10
let r = match n {
    0..10 => "menor"
    10..20 => "diez_o_mas"
    _ => "otro"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("diez_o_mas".into())));
    }

    #[test]
    fn pattern_range_con_negativos() {
        let src = r#"
let n = -3
let r = match n {
    -10..0 => "negativo"
    0..10 => "chico"
    _ => "otro"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("negativo".into())));
    }

    #[test]
    fn pattern_range_no_matchea_no_int() {
        // 3.14 contra patrón 0..10 → no matchea, cae a wildcard.
        let src = r#"
let n = 3.14
let r = match n {
    0..10 => "int_chico"
    _ => "no_int"
}
"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("r"), Some(Value::Str("no_int".into())));
    }

    // ---- builtin len ----

    #[test]
    fn len_de_lista_devuelve_cantidad_de_elementos() {
        let src = "n = len([1, 2, 3, 4, 5])";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(5)));
    }

    #[test]
    fn len_de_lista_vacia_es_cero() {
        let src = "n = len([])";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(0)));
    }

    #[test]
    fn len_de_mapa_devuelve_cantidad_de_pares() {
        let src = r#"n = len({"a": 1, "b": 2, "c": 3})"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(3)));
    }

    #[test]
    fn len_de_string_cuenta_chars_no_bytes() {
        // "ñandú" tiene 5 chars y más de 5 bytes en UTF-8.
        let src = r#"n = len("ñandú")"#;
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(5)));
    }

    #[test]
    fn len_de_range_devuelve_cantidad_de_elementos() {
        let src = "n = len(0..10)";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(10)));
    }

    #[test]
    fn len_de_range_al_reves_es_cero() {
        // 10..0 — el evaluador trata rangos invertidos como vacíos.
        let src = "n = len(10..0)";
        let (env, res) = parse_eval_into_env(src);
        res.unwrap();
        assert_eq!(env.borrow().get("n"), Some(Value::Int(0)));
    }

    #[test]
    fn len_de_int_es_type_error() {
        let src = "n = len(42)";
        let res = parse_and_eval(src);
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeMismatch { .. }));
    }

    #[test]
    fn len_con_cantidad_de_args_incorrecta_es_error() {
        let src = "n = len([1], [2])";
        let res = parse_and_eval(src);
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::WrongArgCount { .. }));
    }
}
