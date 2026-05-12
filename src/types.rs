// types.rs — Fase 5.2
//
// Representación interna del sistema de tipos de Fitz. Mientras
// `ast::TypeExpr` es lo que el parser produce a partir del fuente,
// este módulo modela el tipo *resuelto* contra una tabla: cada
// nombre se busca, cada genérico valida aridad, cada nominal lleva
// identidad única dentro del programa.
//
// El flujo es:
//
//   AST (TypeExpr)  ──resolve_type_expr──►  Type  (resuelto)
//                          contra
//                       TypeEnv
//
// 5.2 valida las anotaciones top-level (campos de `type`, params y
// return de fns, anotaciones de let). El chequeo de cuerpos de
// funciones contra valores queda para 5.3.

use std::collections::HashMap;

use crate::ast::{Expr, Program, Stmt, TypeExpr};
use crate::error::{ErrorKind, FitzError};

/// Identidad única para los tipos nominales (los declarados con
/// `type`). Internamente es un índice contra `TypeEnv.nominals`.
/// Dos `type User` en módulos distintos producen `TypeId`s distintos
/// — la identidad es nominal, no estructural.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);

/// Un tipo resuelto. Lo que el checker compara y muestra al usuario.
///
/// Diferencias con `TypeExpr`:
///  - `Nominal(TypeId)` lleva la identidad ya resuelta (no es solo
///    un string).
///  - Los genéricos built-in tienen variantes propias en lugar de
///    `Generic { name, args }` — facilita el pattern matching.
///  - Los primitivos son singletons (no llevan datos).
///
/// La igualdad estructural derivada sirve: dos `Type` que el checker
/// dice "compatible" deben dar `==`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Str,
    Bool,
    Null,
    /// `Range` solo aparece en `0..10` por ahora — no tiene parámetro.
    Range,

    /// `List<T>`.
    List(Box<Type>),
    /// `Map<K, V>`.
    Map(Box<Type>, Box<Type>),
    /// `Result<T>`. La E está fijada como `Str` por convención hasta
    /// que el lenguaje soporte genéricos de usuario (post-Fase 5).
    Result(Box<Type>),

    /// Tipo declarado por el usuario (`type User { ... }`) o
    /// importado. La identidad va por `TypeId`.
    Nominal(TypeId),

    /// `T?` — el valor puede ser de tipo `T` o `Null`.
    Nullable(Box<Type>),

    /// Tipo de una función: `fn(p1, p2, ...) -> r`. Lo construye el
    /// checker al registrar `Stmt::FnDef` (5.3.2) y al sintetizar
    /// `Expr::FnExpr` (5.3.5). En 5.3.1 ya existe como variante para
    /// no refactorizar después.
    Function {
        params: Vec<Type>,
        ret: Box<Type>,
    },

    /// "Sin tipo determinado". Escape gradual: aparece donde el
    /// checker no puede o no quiere inferir un tipo concreto. Param
    /// sin anotación, `let` sin anotación con RHS no inferible,
    /// expresiones que el checker todavía no modela (calls antes de
    /// 5.3.2, métodos antes de 5.3.4, etc.). Cualquier comparación
    /// contra `Any` pasa: nada se rechaza por culpa de un `Any`.
    Any,
}

impl Type {
    /// `true` si el tipo es `T?` a nivel top.
    pub fn is_nullable(&self) -> bool {
        matches!(self, Type::Nullable(_))
    }

    /// Devuelve `&Type` pelando una sola capa de `Nullable`. `Int? →
    /// Int`. `Int → Int`. No baja recursivamente.
    pub fn base(&self) -> &Type {
        match self {
            Type::Nullable(t) => t,
            other => other,
        }
    }

    /// Reproduce el tipo para mensajes al usuario. Necesita el env
    /// para resolver los nombres de los `Nominal`.
    pub fn display(&self, env: &TypeEnv) -> String {
        match self {
            Type::Int => "Int".into(),
            Type::Float => "Float".into(),
            Type::Str => "Str".into(),
            Type::Bool => "Bool".into(),
            Type::Null => "Null".into(),
            Type::Range => "Range".into(),
            Type::List(t) => format!("List<{}>", t.display(env)),
            Type::Map(k, v) => format!("Map<{}, {}>", k.display(env), v.display(env)),
            Type::Result(t) => format!("Result<{}>", t.display(env)),
            Type::Nominal(id) => env.info(*id).name.clone(),
            Type::Nullable(t) => format!("{}?", t.display(env)),
            Type::Function { params, ret } => {
                let ps: Vec<String> = params.iter().map(|p| p.display(env)).collect();
                format!("fn({}) -> {}", ps.join(", "), ret.display(env))
            }
            Type::Any => "Any".into(),
        }
    }
}

/// Info de un tipo nominal declarado en el programa.
#[derive(Debug, Clone)]
pub struct NominalInfo {
    pub name: String,
    /// Campos resueltos. `None` mientras el tipo está siendo
    /// registrado en la primera vuelta (forward decl); se completa
    /// en la segunda vuelta una vez que todos los nominales son
    /// conocidos.
    pub fields: Option<Vec<ResolvedField>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedField {
    pub name: String,
    pub type_: Type,
}

/// Entorno de tipos del programa. Lleva:
///  - Built-ins (primitivos y genéricos), implícitos vía
///    `resolve_named`.
///  - Tipos nominales declarados, accesibles por nombre.
///
/// Sin scopes anidados todavía: 5.2 trabaja a nivel del programa
/// completo. Cuando entren chequeos de bodies (5.3) se agregarán
/// scopes locales para `let`/params.
#[derive(Debug, Default)]
pub struct TypeEnv {
    nominals: Vec<NominalInfo>,
    by_name: HashMap<String, TypeId>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un tipo nominal por nombre, devolviendo su id.
    /// Si el nombre ya estaba → error "tipo redeclarado".
    pub fn declare_nominal(&mut self, name: String) -> Result<TypeId, FitzError> {
        if self.by_name.contains_key(&name) {
            return Err(FitzError::new(
                ErrorKind::TypeError,
                0,
                0,
                format!("tipo `{}` declarado más de una vez", name),
            ));
        }
        let id = TypeId(self.nominals.len());
        self.nominals.push(NominalInfo {
            name: name.clone(),
            fields: None,
        });
        self.by_name.insert(name, id);
        Ok(id)
    }

    /// Completa los fields de un nominal (segunda vuelta).
    pub fn set_fields(&mut self, id: TypeId, fields: Vec<ResolvedField>) {
        self.nominals[id.0].fields = Some(fields);
    }

    pub fn lookup(&self, name: &str) -> Option<TypeId> {
        self.by_name.get(name).copied()
    }

    pub fn info(&self, id: TypeId) -> &NominalInfo {
        &self.nominals[id.0]
    }

    /// Cantidad de nominales registrados. Útil para tests.
    pub fn nominal_count(&self) -> usize {
        self.nominals.len()
    }
}

// ---------------------------------------------------------------------------
// Resolución de TypeExpr → Type
// ---------------------------------------------------------------------------

/// Convierte un `TypeExpr` (sintáctico) en un `Type` (resuelto)
/// contra `env`. Devuelve el `Type` o un `FitzError` describiendo
/// qué falló. Los errores siempre son `ErrorKind::TypeError`.
pub fn resolve_type_expr(t: &TypeExpr, env: &TypeEnv) -> Result<Type, FitzError> {
    match t {
        TypeExpr::Named(name) => resolve_named(name, &[], env),
        TypeExpr::Generic { name, args } => resolve_named(name, args, env),
        TypeExpr::Nullable(inner) => {
            let inner = resolve_type_expr(inner, env)?;
            Ok(Type::Nullable(Box::new(inner)))
        }
    }
}

/// Resuelve un nombre + argumentos contra el env. La separación
/// entre `Named` y `Generic` desaparece acá: `List<Int>` y
/// `List` (sin argumentos) toman el mismo camino y la aridad
/// validada en el lugar correspondiente.
fn resolve_named(name: &str, args: &[TypeExpr], env: &TypeEnv) -> Result<Type, FitzError> {
    // Primitivos (aridad 0). Si el usuario los aplica como genéricos
    // → error de aridad explícito.
    let prim = match name {
        "Int" => Some(Type::Int),
        "Float" => Some(Type::Float),
        "Str" => Some(Type::Str),
        "Bool" => Some(Type::Bool),
        "Null" => Some(Type::Null),
        "Range" => Some(Type::Range),
        _ => None,
    };
    if let Some(t) = prim {
        if !args.is_empty() {
            return Err(arity_error(name, 0, args.len()));
        }
        return Ok(t);
    }

    // Genéricos built-in con aridad fija.
    match name {
        "List" => {
            expect_arity(name, 1, args)?;
            let inner = resolve_type_expr(&args[0], env)?;
            Ok(Type::List(Box::new(inner)))
        }
        "Map" => {
            expect_arity(name, 2, args)?;
            let k = resolve_type_expr(&args[0], env)?;
            let v = resolve_type_expr(&args[1], env)?;
            Ok(Type::Map(Box::new(k), Box::new(v)))
        }
        "Result" => {
            expect_arity(name, 1, args)?;
            let inner = resolve_type_expr(&args[0], env)?;
            Ok(Type::Result(Box::new(inner)))
        }
        _ => {
            // Nominal declarado por el usuario.
            match env.lookup(name) {
                Some(id) => {
                    if !args.is_empty() {
                        return Err(FitzError::new(
                            ErrorKind::TypeError,
                            0,
                            0,
                            format!(
                                "tipo `{}` no es genérico, no acepta argumentos de tipo",
                                name
                            ),
                        ));
                    }
                    Ok(Type::Nominal(id))
                }
                None => Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    format!("tipo desconocido `{}`", name),
                )),
            }
        }
    }
}

fn expect_arity(name: &str, expected: usize, args: &[TypeExpr]) -> Result<(), FitzError> {
    if args.len() != expected {
        Err(arity_error(name, expected, args.len()))
    } else {
        Ok(())
    }
}

fn arity_error(name: &str, expected: usize, found: usize) -> FitzError {
    FitzError::new(
        ErrorKind::TypeError,
        0,
        0,
        format!(
            "el tipo `{}` espera {} argumento(s) de tipo, recibió {}",
            name, expected, found
        ),
    )
}

// ---------------------------------------------------------------------------
// Pasada de resolución sobre el programa
// ---------------------------------------------------------------------------

/// Resultado de chequear un programa: el `TypeEnv` con todos los
/// tipos declarados resueltos, y la lista (posiblemente vacía) de
/// errores acumulados. Devolvemos ambos siempre: el caller decide
/// si abortar (modo strict) o reportar como warnings (modo run).
pub fn resolve_program(program: &Program) -> (TypeEnv, Vec<FitzError>) {
    let mut env = TypeEnv::new();
    let mut errors = Vec::new();

    // Vuelta 1: registrar los nombres de los `type` declarados localmente.
    // Forward refs entre nominales locales.
    for stmt in program {
        if let Stmt::TypeDef { name, .. } = stmt {
            if let Err(e) = env.declare_nominal(name.clone()) {
                errors.push(e);
            }
        }
    }

    // Vuelta 1b: registrar nombres traídos por `from ... import ...`
    // como nominales con fields desconocidos. Sin esto, un
    // `User { ... }` que viene de `from foo import User` queda sin
    // tipo declarado y el checker se queja. Si el nombre choca con
    // un type local, gana el local — el import se ignora en silencio
    // (decisión: 5.x mantiene comportamiento gradual; cuando 5.3.x
    // cargue módulos cross-archivo, podemos refinar el warning).
    //
    // `import foo` no agrega nombres en el TypeEnv — el módulo es un
    // value, no un type. Se registra como var en `check_stmt`.
    for stmt in program {
        if let Stmt::FromImport { names, .. } = stmt {
            for n in names {
                if env.lookup(n).is_none() {
                    // declare_nominal puede fallar solo si el nombre
                    // ya estaba; ya chequeamos así que es seguro.
                    let _ = env.declare_nominal(n.clone());
                }
            }
        }
    }

    // Vuelta 2: resolver los fields de cada `type`.
    for stmt in program {
        if let Stmt::TypeDef { name, fields } = stmt {
            // Si la declaración falló (duplicado), no hay id que actualizar.
            let id = match env.lookup(name) {
                Some(id) => id,
                None => continue,
            };
            // Si el slot ya tiene fields, es la segunda vez que vemos
            // este nominal — un duplicado que ya reportamos. Saltar.
            if env.info(id).fields.is_some() {
                continue;
            }
            let mut resolved = Vec::new();
            for f in fields {
                match resolve_type_expr(&f.type_, &env) {
                    Ok(t) => {
                        if let Some(default) = &f.default {
                            if let Err(e) =
                                check_field_default(name, &f.name, &t, default, &env)
                            {
                                errors.push(e);
                            }
                        }
                        resolved.push(ResolvedField {
                            name: f.name.clone(),
                            type_: t,
                        });
                    }
                    Err(e) => errors.push(annotate(
                        e,
                        &format!("en el campo `{}` del tipo `{}`", f.name, name),
                    )),
                }
            }
            env.set_fields(id, resolved);
        }
    }

    // Vuelta 3: anotaciones de FnDef / Assign / let internos.
    for stmt in program {
        resolve_stmt_annotations(stmt, &env, &mut errors);
    }

    (env, errors)
}

fn resolve_stmt_annotations(stmt: &Stmt, env: &TypeEnv, errors: &mut Vec<FitzError>) {
    match stmt {
        Stmt::Assign { type_: Some(t), .. } => {
            if let Err(e) = resolve_type_expr(t, env) {
                errors.push(e);
            }
        }
        Stmt::FnDef {
            name,
            params,
            return_type,
            body,
            ..
        } => {
            for p in params {
                if let Some(t) = &p.type_ {
                    if let Err(e) = resolve_type_expr(t, env) {
                        errors.push(annotate(
                            e,
                            &format!("en el parámetro `{}` de la función `{}`", p.name, name),
                        ));
                    }
                }
            }
            if let Some(t) = return_type {
                if let Err(e) = resolve_type_expr(t, env) {
                    errors.push(annotate(
                        e,
                        &format!("en el tipo de retorno de la función `{}`", name),
                    ));
                }
            }
            // Bajamos por el body para validar anotaciones de lets
            // internos. Las expresiones en sí (cuerpo del fn) se
            // validan en 5.3.
            for s in body {
                resolve_stmt_annotations(s, env, errors);
            }
        }
        Stmt::While { body, .. } | Stmt::Loop { body } | Stmt::For { body, .. } => {
            for s in body {
                resolve_stmt_annotations(s, env, errors);
            }
        }
        _ => {}
    }
}

/// Chequea (caso simple) que un default literal coincida con el
/// tipo declarado del campo. Aplica solo a literales constantes:
/// otros defaults (expresiones, struct literals, llamadas) se
/// aceptan sin chequeo hasta 5.3, que valida expresiones contra
/// tipos esperados.
///
/// Reglas:
///   - `Null` aceptable si el declarado es `T?`.
///   - `Int` aceptable contra `Float` (coerción Int→Float, mismo
///     criterio que el evaluator usa en runtime).
///   - El resto: igualdad estructural sobre la base (pelando un
///     `Nullable` si lo hay).
fn check_field_default(
    type_name: &str,
    field_name: &str,
    declared: &Type,
    default: &Expr,
    env: &TypeEnv,
) -> Result<(), FitzError> {
    let lit_type = match default {
        Expr::Int(_) => Some(Type::Int),
        Expr::Float(_) => Some(Type::Float),
        Expr::Str(_) => Some(Type::Str),
        Expr::Bool(_) => Some(Type::Bool),
        Expr::Null => Some(Type::Null),
        _ => None,
    };
    let lit_type = match lit_type {
        Some(t) => t,
        None => return Ok(()), // no literal, se valida en 5.3
    };
    // Null sobre tipo nullable: OK.
    if matches!(lit_type, Type::Null) && declared.is_nullable() {
        return Ok(());
    }
    // Coerción Int→Float.
    if matches!(lit_type, Type::Int) && matches!(declared.base(), Type::Float) {
        return Ok(());
    }
    // Igualdad estructural sobre la base.
    if &lit_type != declared.base() {
        return Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "el campo `{}.{}` declarado como `{}` recibió un default `{}`",
                type_name,
                field_name,
                declared.display(env),
                lit_type.display(env),
            ),
        ));
    }
    Ok(())
}

/// Anexa contexto a un mensaje de error. El mensaje original queda
/// primero, el contexto entre paréntesis al final.
fn annotate(mut e: FitzError, context: &str) -> FitzError {
    e.message = format!("{} ({})", e.message, context);
    e
}

// ---------------------------------------------------------------------------
// Checker de expresiones (Fase 5.3.1)
//
// Mientras `resolve_program` chequea anotaciones, `check_program` corre
// además una pasada por las expresiones del programa. La idea:
//   1. Pre-registrar firmas de los `Stmt::FnDef` top-level y builtins
//      en un scope global de variables.
//   2. Recorrer cada Stmt, abriendo scopes por cada `FnDef`/loop/etc.
//   3. Para cada `Expr`, sintetizar su tipo (`infer_expr`).
//   4. Cuando hay un tipo *esperado* (anotación de `let`, default de
//      campo no-literal, etc.), validar compatibilidad.
//
// 5.3.1 cubre: literales, ident, BinOp aritmético/comparación/lógico,
// UnaryOp Neg, StrInterp, `if` expr, list/map literales, struct lit,
// field access sobre Nominal, Range. Resto devuelve `Any` y se cubre
// en 5.3.2+.
// ---------------------------------------------------------------------------

use crate::ast::{AssignTarget, BinOpKind, StrPart, UnaryOpKind};

/// Estado mutable durante la pasada de chequeo de expresiones.
struct CheckCtx<'a> {
    types: &'a TypeEnv,
    /// Stack de scopes para variables. El primero es el global
    /// (builtins + fns top-level + lets top-level). Cada `FnDef`
    /// body, cada loop body, abren un scope nuevo.
    scopes: Vec<std::collections::HashMap<String, Type>>,
    /// Stack de tipos de retorno esperados, uno por cada función
    /// (FnDef o FnExpr) anidada que se está chequeando. Vacío en
    /// el scope top-level. `Stmt::Return` lo consulta para validar.
    return_stack: Vec<Type>,
    errors: Vec<FitzError>,
}

impl<'a> CheckCtx<'a> {
    fn new(types: &'a TypeEnv) -> Self {
        let mut ctx = Self {
            types,
            scopes: vec![std::collections::HashMap::new()],
            return_stack: Vec::new(),
            errors: Vec::new(),
        };
        ctx.register_builtins();
        ctx
    }

    /// Builtins del lenguaje que existen siempre en el env del
    /// evaluator. Los de aridad fija reciben firma real (chequea
    /// aridad y eventualmente tipos); los variádicos se modelan
    /// como `Any` hasta tener una representación dedicada.
    fn register_builtins(&mut self) {
        // `print(args...)` — variádico. Modelado como Any: ningún
        // call sobre Any se chequea (gradual escape).
        self.scopes[0].insert("print".into(), Type::Any);
        // `len(x) -> Int` — aridad 1 sobre List/Map/Str/Range. El
        // param es Any porque los receptores no comparten un solo
        // tipo (todavía no tenemos union types / "any iterable").
        // La aridad sí se valida; el tipo del receptor llega en 5.3.4.
        self.scopes[0].insert(
            "len".into(),
            Type::Function {
                params: vec![Type::Any],
                ret: Box::new(Type::Int),
            },
        );
    }

    fn push_scope(&mut self) {
        self.scopes.push(std::collections::HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare_var(&mut self, name: String, ty: Type) {
        // Se permite shadowing — el nombre se redeclara en el scope
        // actual sin advertir. El evaluator se comporta igual.
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, ty);
        }
    }

    fn lookup_var(&self, name: &str) -> Option<&Type> {
        for s in self.scopes.iter().rev() {
            if let Some(t) = s.get(name) {
                return Some(t);
            }
        }
        None
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.errors
            .push(FitzError::new(ErrorKind::TypeError, 0, 0, msg.into()));
    }
}

/// Convierte una `Option<TypeExpr>` en `Type` para anotaciones del
/// usuario. Si la anotación faltó → `Any`. Si la anotación está pero
/// no resuelve → `Any` y se asume que el error ya fue reportado por
/// `resolve_program`.
fn ann_to_type(ann: Option<&TypeExpr>, env: &TypeEnv) -> Type {
    match ann {
        None => Type::Any,
        Some(t) => resolve_type_expr(t, env).unwrap_or(Type::Any),
    }
}

/// Sintetiza el tipo de una expresión.
///
/// Casos no cubiertos en 5.3.1 devuelven `Type::Any` silenciosamente
/// — no son errores, solo no chequeamos esa forma todavía. Las
/// sub-fases siguientes (5.3.2 calls, 5.3.3 Result, 5.3.4 métodos,
/// 5.3.5 FnExpr) los irán reemplazando.
fn infer_expr(ctx: &mut CheckCtx, e: &Expr) -> Type {
    match e {
        Expr::Int(_) => Type::Int,
        Expr::Float(_) => Type::Float,
        Expr::Str(_) => Type::Str,
        Expr::Bool(_) => Type::Bool,
        Expr::Null => Type::Null,

        Expr::StrInterp(parts) => {
            // Las sub-expresiones se evalúan para errores aunque el
            // resultado siempre sea Str.
            for p in parts {
                if let StrPart::Expr(inner) = p {
                    let _ = infer_expr(ctx, inner);
                }
            }
            Type::Str
        }

        Expr::Ident(name) => {
            if let Some(t) = ctx.lookup_var(name) {
                return t.clone();
            }
            // Si es un tipo nominal declarado, el usuario lo está
            // usando como valor (lo cual el evaluator soporta:
            // registra Value::Type en el env). No es error; lo
            // tratamos como Any.
            if ctx.types.lookup(name).is_some() {
                return Type::Any;
            }
            ctx.error(format!("variable desconocida `{}`", name));
            Type::Any
        }

        Expr::UnaryOp { op, operand } => {
            let t = infer_expr(ctx, operand);
            match op {
                UnaryOpKind::Neg => match &t {
                    Type::Int | Type::Float | Type::Any => t,
                    other => {
                        ctx.error(format!(
                            "el operador `-` (negación) espera Int o Float, recibió `{}`",
                            other.display(ctx.types)
                        ));
                        Type::Any
                    }
                },
            }
        }

        Expr::BinOp { op, left, right } => {
            let lt = infer_expr(ctx, left);
            let rt = infer_expr(ctx, right);
            infer_binop(ctx, op, &lt, &rt)
        }

        Expr::If { condition, then, else_ } => {
            // Condición debe ser Bool (o Any).
            let cond_ty = infer_expr(ctx, condition);
            if !is_compatible(&cond_ty, &Type::Bool) {
                ctx.error(format!(
                    "la condición de `if` debe ser Bool, recibió `{}`",
                    cond_ty.display(ctx.types)
                ));
            }
            // Cada rama es un bloque; el "tipo" de un if-stmt es el
            // de su última expresión-stmt. Para 5.3.1 nos alcanza con
            // walkear los bloques (con scope) y devolver Any.
            ctx.push_scope();
            check_block(ctx, then);
            ctx.pop_scope();
            if let Some(else_body) = else_ {
                ctx.push_scope();
                check_block(ctx, else_body);
                ctx.pop_scope();
            }
            Type::Any
        }

        Expr::List(items) => {
            // List<T> con T = tipo del primer elemento si los demás
            // son compatibles; si hay mezcla, T = Any.
            if items.is_empty() {
                return Type::List(Box::new(Type::Any));
            }
            let first = infer_expr(ctx, &items[0]);
            let mut all_same = true;
            for it in &items[1..] {
                let t = infer_expr(ctx, it);
                if !is_compatible(&t, &first) {
                    all_same = false;
                }
            }
            if all_same {
                Type::List(Box::new(first))
            } else {
                Type::List(Box::new(Type::Any))
            }
        }

        Expr::Map(pairs) => {
            if pairs.is_empty() {
                return Type::Map(Box::new(Type::Any), Box::new(Type::Any));
            }
            // Sintetizamos por el primer par. Mezcla de tipos cae a Any.
            let (fk, fv) = (infer_expr(ctx, &pairs[0].0), infer_expr(ctx, &pairs[0].1));
            let mut k_same = true;
            let mut v_same = true;
            for (k, v) in &pairs[1..] {
                let kt = infer_expr(ctx, k);
                let vt = infer_expr(ctx, v);
                if !is_compatible(&kt, &fk) {
                    k_same = false;
                }
                if !is_compatible(&vt, &fv) {
                    v_same = false;
                }
            }
            Type::Map(
                Box::new(if k_same { fk } else { Type::Any }),
                Box::new(if v_same { fv } else { Type::Any }),
            )
        }

        Expr::Range { start, end } => {
            // Start y end deben ser Int (lo es en el evaluator).
            for (label, e) in [("inicio", start.as_ref()), ("fin", end.as_ref())] {
                let t = infer_expr(ctx, e);
                if !is_compatible(&t, &Type::Int) {
                    ctx.error(format!(
                        "{} del rango debe ser Int, recibió `{}`",
                        label,
                        t.display(ctx.types)
                    ));
                }
            }
            Type::Range
        }

        Expr::StructLit { type_name, fields } => {
            // Sintetiza Nominal si el nombre del tipo está declarado.
            // Validar campos contra el `type` declarado: faltantes,
            // extras, tipos incompatibles.
            let id = match ctx.types.lookup(type_name) {
                Some(id) => id,
                None => {
                    // resolve_program ya reporta tipos desconocidos
                    // como campos/anotaciones; un StructLit con
                    // nombre inexistente sí es propio del checker.
                    ctx.error(format!(
                        "no existe el tipo `{}` para instanciar",
                        type_name
                    ));
                    // Igual evaluamos los valores para detectar errores
                    // adentro.
                    for (_, v) in fields {
                        let _ = infer_expr(ctx, v);
                    }
                    return Type::Any;
                }
            };
            // Comparamos contra los campos resueltos del nominal.
            let declared = ctx.types.info(id).fields.clone();
            // Inferir tipos provistos (siempre, para que warnings adentro
            // afloren).
            let mut provided_types: Vec<(String, Type)> = Vec::new();
            for (n, v) in fields {
                let t = infer_expr(ctx, v);
                provided_types.push((n.clone(), t));
            }
            if let Some(declared) = declared {
                // Extras
                let declared_names: std::collections::HashSet<&str> =
                    declared.iter().map(|f| f.name.as_str()).collect();
                for (n, _) in &provided_types {
                    if !declared_names.contains(n.as_str()) {
                        ctx.error(format!(
                            "el tipo `{}` no tiene un campo llamado `{}`",
                            type_name, n
                        ));
                    }
                }
                // Faltantes y compatibilidad de los provistos.
                let provided_map: std::collections::HashMap<&str, &Type> = provided_types
                    .iter()
                    .map(|(n, t)| (n.as_str(), t))
                    .collect();
                for f in &declared {
                    match provided_map.get(f.name.as_str()) {
                        Some(actual) => {
                            if !is_compatible(actual, &f.type_) {
                                ctx.error(format!(
                                    "el campo `{}.{}` espera `{}`, recibió `{}`",
                                    type_name,
                                    f.name,
                                    f.type_.display(ctx.types),
                                    actual.display(ctx.types)
                                ));
                            }
                        }
                        None => {
                            // Faltante: válido si nullable o si el
                            // evaluator espera default (validado en
                            // resolve_program).
                            //
                            // En el caso nullable, no hay error. En el
                            // resto, podríamos alertar — pero el
                            // evaluator emite su propio error en
                            // runtime cuando falta un campo sin
                            // default. Para no duplicar mensajes,
                            // dejamos esto pasar en 5.3.1.
                        }
                    }
                }
            }
            Type::Nominal(id)
        }

        Expr::Field { object, field } => {
            let obj_ty = infer_expr(ctx, object);
            match &obj_ty {
                Type::Nominal(id) => {
                    let info = ctx.types.info(*id);
                    if let Some(declared) = &info.fields {
                        if let Some(f) = declared.iter().find(|f| f.name == *field) {
                            return f.type_.clone();
                        }
                        // Campo desconocido. En 5.3.4 cuando entren
                        // métodos puede ser legítimo (el "field"
                        // sintáctico es un método). Por ahora silencio
                        // si está dentro de un Call (lo handlea
                        // infer_call), y warning si no — pero no
                        // sabemos el contexto acá. Devolvemos Any.
                        return Type::Any;
                    }
                    Type::Any
                }
                // Cualquier otro receptor: 5.3.4 lo cubre con métodos
                // built-in. Por ahora Any.
                _ => Type::Any,
            }
        }

        Expr::Call { callee, args } => {
            // Camino de método: `obj.method(args)` ↔ callee
            // sintáctico es `Expr::Field`. Despachamos por
            // `(tipo del receptor, nombre del método)` contra la
            // tabla de built-ins (5.3.4) en lugar de pasar por la
            // ruta general — la ruta general no puede modelar
            // signatures paramétricas como `List<T>.map`.
            if let Expr::Field { object, field } = callee.as_ref() {
                let obj_ty = infer_expr(ctx, object);
                let args_ty: Vec<Type> =
                    args.iter().map(|a| infer_expr(ctx, a)).collect();
                return match infer_method_call(ctx, &obj_ty, field, &args_ty) {
                    Some(ret) => ret,
                    // Receptor que no entendemos (Nominal sin métodos
                    // custom, Module via import, Any): seguimos en
                    // modo gradual sin chequear nada de la llamada.
                    None => Type::Any,
                };
            }
            // Sintetizamos siempre callee y args para que afloren
            // errores adentro. Después validamos aridad y tipos según
            // lo que sea el callee.
            let callee_ty = infer_expr(ctx, callee);
            let args_ty: Vec<Type> = args.iter().map(|a| infer_expr(ctx, a)).collect();
            match callee_ty {
                // Gradual: callee de tipo desconocido no se chequea.
                Type::Any => Type::Any,
                Type::Function { params, ret } => {
                    let label = describe_callee(callee);
                    if args.len() != params.len() {
                        ctx.error(format!(
                            "{} espera {} argumento(s), recibió {}",
                            label,
                            params.len(),
                            args.len()
                        ));
                    } else {
                        for (i, (actual, expected)) in
                            args_ty.iter().zip(params.iter()).enumerate()
                        {
                            if !is_compatible(actual, expected) {
                                ctx.error(format!(
                                    "{}: el argumento {} espera `{}`, recibió `{}`",
                                    label,
                                    i + 1,
                                    expected.display(ctx.types),
                                    actual.display(ctx.types)
                                ));
                            }
                        }
                    }
                    *ret
                }
                other => {
                    ctx.error(format!(
                        "`{}` no es una función",
                        other.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }
        Expr::FnExpr { params, body } => {
            // Walkeamos el body con un scope nuevo y los params
            // bindeados (con su tipo declarado o `Any` si la anotación
            // faltó). El tipo del FnExpr es `Function`; 5.3.5 refina
            // el `ret` inferido del body. Como no tenemos return_type
            // declarado, empujamos `Any` al return_stack — los `return`
            // adentro del FnExpr no se chequean en 5.3.2.
            ctx.push_scope();
            ctx.return_stack.push(Type::Any);
            let param_types: Vec<Type> = params
                .iter()
                .map(|p| ann_to_type(p.type_.as_ref(), ctx.types))
                .collect();
            for (p, t) in params.iter().zip(param_types.iter()) {
                ctx.declare_var(p.name.clone(), t.clone());
            }
            check_block(ctx, body);
            ctx.return_stack.pop();
            ctx.pop_scope();
            Type::Function {
                params: param_types,
                ret: Box::new(Type::Any),
            }
        }
        Expr::Index { object, index } => {
            let _ = infer_expr(ctx, object);
            let _ = infer_expr(ctx, index);
            Type::Any // 5.3.4
        }
        Expr::Match { value, arms } => {
            let scrutinee = infer_expr(ctx, value);
            // Tipo del binding según el patrón. Para `Ok(x)` con
            // scrutinee `Result<T>`, x es T. Para `Err(e)` el error
            // está fijado en Str. Para Ident es el scrutinee
            // completo. Para literales/wildcard/range no hay bind.
            let mut first: Option<Type> = None;
            for arm in arms {
                ctx.push_scope();
                bind_pattern(ctx, &arm.pattern, &scrutinee);
                let t = infer_expr(ctx, &arm.body);
                ctx.pop_scope();
                if first.is_none() {
                    first = Some(t);
                }
            }
            // Exhaustividad: solo la exigimos cuando el scrutinee es
            // `Result<T>` (puro, no nullable). Otros tipos no tienen
            // semántica de "variantes" para Fitz todavía.
            if matches!(scrutinee, Type::Result(_)) {
                check_result_match_exhaustiveness(ctx, arms);
            }
            first.unwrap_or(Type::Any)
        }
        Expr::Ok(inner) => {
            let t = infer_expr(ctx, inner);
            Type::Result(Box::new(t))
        }
        Expr::Err(inner) => {
            let _ = infer_expr(ctx, inner);
            // E está fijado en Str pero el T es desconocido sin contexto.
            Type::Result(Box::new(Type::Any))
        }
        Expr::Try(inner) => {
            let operand_ty = infer_expr(ctx, inner);
            match &operand_ty {
                // Gradual: operando de tipo desconocido no se chequea.
                // Cubre el caso típico de método built-in (callee
                // Field) que todavía devuelve Any hasta 5.3.4.
                Type::Any => Type::Any,
                Type::Result(inner_ty) => {
                    // Si estamos adentro de una función con
                    // return_type concreto, exigimos que sea Result —
                    // el `?` propaga un `Err(_)` vía `return`, así que
                    // la fn contenedora tiene que poder recibirlo.
                    // Fn sin return_type (Any) o top-level no chequea.
                    if let Some(expected) = ctx.return_stack.last().cloned() {
                        let is_ok = matches!(expected, Type::Any | Type::Result(_));
                        if !is_ok {
                            ctx.error(format!(
                                "el operador `?` solo puede usarse adentro de una función que retorne `Result<...>`; esta retorna `{}`",
                                expected.display(ctx.types)
                            ));
                        }
                    }
                    (**inner_ty).clone()
                }
                other => {
                    ctx.error(format!(
                        "el operador `?` requiere un `Result`, recibió `{}`",
                        other.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }
    }
}

/// Etiqueta amigable para el callee de un `Call`. Aparece en los
/// errores de aridad y de tipos de argumento. Cuando podemos
/// identificar el nombre (Ident o Field), lo usamos; si no, una
/// etiqueta genérica.
fn describe_callee(callee: &Expr) -> String {
    match callee {
        Expr::Ident(name) => format!("la función `{}`", name),
        Expr::Field { field, .. } => format!("el método `{}`", field),
        _ => "esta llamada".into(),
    }
}

/// Despacho del checker para método built-in. Recibe el tipo del
/// receptor (`xs` en `xs.map(f)`), el nombre del método, y los
/// tipos ya inferidos de los argumentos. Devuelve `Some(ret)` con
/// el tipo del resultado, o `None` cuando el receptor no entra en
/// el dispatch built-in (Nominal sin métodos custom todavía,
/// Module via import — ambos modelados como `Any` o `Nominal`).
///
/// Para los casos `None`, el caller continúa en modo gradual
/// (devuelve `Any` sin chequear aridad/tipos). Para los casos
/// soportados, las violaciones se reportan vía `ctx.error(...)`
/// pero el dispatch siempre devuelve `Some(...)` con el ret
/// inferido (los errores no propagan, se acumulan).
///
/// Convención: `T` siempre proviene del receptor concreto en este
/// call site. `List<Int>.map(f)` y `List<Str>.map(f)` instancian
/// distinto.
fn infer_method_call(
    ctx: &mut CheckCtx,
    receiver_ty: &Type,
    method: &str,
    args_ty: &[Type],
) -> Option<Type> {
    // Pelamos un Nullable: `xs?.map(...)` cae cuando el `?` ya
    // desempacó, así que acá raramente vemos Nullable. Por las
    // dudas, lo dejamos transparente.
    let recv = receiver_ty.base();
    match recv {
        Type::List(t) => {
            let t = (**t).clone();
            Some(infer_list_method(ctx, &t, method, args_ty))
        }
        Type::Map(k, v) => {
            let k = (**k).clone();
            let v = (**v).clone();
            Some(infer_map_method(ctx, &k, &v, method, args_ty))
        }
        Type::Str => Some(infer_str_method(ctx, method, args_ty)),
        // Gradual: no aplicamos chequeo sobre Any (no sabemos
        // nada) ni sobre Nominal (los métodos custom sobre `type`
        // no existen todavía — deuda de 3.2). Quien llame retorna
        // `Any`.
        Type::Any | Type::Nominal(_) => None,
        other => {
            // Tipos sin métodos built-in: `42.foo()` y similares.
            // El evaluator también corta, acá nos adelantamos con
            // mensaje específico.
            ctx.error(format!(
                "el tipo `{}` no tiene el método `{}`",
                other.display(ctx.types),
                method
            ));
            Some(Type::Any)
        }
    }
}

/// Valida aridad de un método built-in. Devuelve `true` si la
/// aridad coincide (para que el caller pueda saltarse validaciones
/// extra sobre argumentos que no existen). Si falla, acumula error
/// y devuelve `false`.
fn check_method_arity(
    ctx: &mut CheckCtx,
    method: &str,
    args_ty: &[Type],
    expected: usize,
) -> bool {
    if args_ty.len() != expected {
        ctx.error(format!(
            "el método `{}` espera {} argumento(s), recibió {}",
            method,
            expected,
            args_ty.len()
        ));
        false
    } else {
        true
    }
}

/// Valida un callback unario (`fn(T) -> U`). Devuelve el `U`
/// inferido del callback, o `Any` si el callback es Any o no
/// validable. Si `expected_ret` es `Some(B)`, además exige que U
/// sea compatible con B (caso típico: `.filter()` exige `Bool`).
fn check_unary_callback(
    ctx: &mut CheckCtx,
    cb: &Type,
    elem_ty: &Type,
    method: &str,
    expected_ret: Option<&Type>,
) -> Type {
    match cb {
        Type::Any => Type::Any,
        Type::Function { params, ret } => {
            if params.len() != 1 {
                ctx.error(format!(
                    "la callback de `.{}()` debe tomar 1 argumento, recibió {}",
                    method,
                    params.len()
                ));
                return (**ret).clone();
            }
            // El param del callback tiene que poder recibir un T
            // (el tipo de los elementos). Si el callback declaró un
            // tipo concreto incompatible, error.
            if !is_compatible(elem_ty, &params[0]) {
                ctx.error(format!(
                    "la callback de `.{}()` recibe elementos `{}` pero su parámetro es `{}`",
                    method,
                    elem_ty.display(ctx.types),
                    params[0].display(ctx.types)
                ));
            }
            if let Some(expected) = expected_ret {
                if !is_compatible(ret, expected) {
                    ctx.error(format!(
                        "la callback de `.{}()` debe devolver `{}`, devuelve `{}`",
                        method,
                        expected.display(ctx.types),
                        ret.display(ctx.types)
                    ));
                }
            }
            (**ret).clone()
        }
        other => {
            ctx.error(format!(
                "la callback de `.{}()` debe ser una función, recibió `{}`",
                method,
                other.display(ctx.types)
            ));
            Type::Any
        }
    }
}

fn infer_list_method(
    ctx: &mut CheckCtx,
    t: &Type,
    method: &str,
    args_ty: &[Type],
) -> Type {
    match method {
        "push" => {
            check_method_arity(ctx, "push", args_ty, 1);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, t) {
                    ctx.error(format!(
                        "`push` sobre `List<{}>` recibió `{}`",
                        t.display(ctx.types),
                        arg.display(ctx.types)
                    ));
                }
            }
            Type::Null
        }
        "pop" => {
            check_method_arity(ctx, "pop", args_ty, 0);
            t.clone()
        }
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0);
            Type::Int
        }
        "map" => {
            if !check_method_arity(ctx, "map", args_ty, 1) {
                return Type::List(Box::new(Type::Any));
            }
            let u = check_unary_callback(ctx, &args_ty[0], t, "map", None);
            Type::List(Box::new(u))
        }
        "filter" => {
            if !check_method_arity(ctx, "filter", args_ty, 1) {
                return Type::List(Box::new(t.clone()));
            }
            check_unary_callback(ctx, &args_ty[0], t, "filter", Some(&Type::Bool));
            Type::List(Box::new(t.clone()))
        }
        "find" => {
            if !check_method_arity(ctx, "find", args_ty, 1) {
                return Type::Result(Box::new(t.clone()));
            }
            check_unary_callback(ctx, &args_ty[0], t, "find", Some(&Type::Bool));
            Type::Result(Box::new(t.clone()))
        }
        _ => {
            ctx.error(format!(
                "`List<{}>` no tiene el método `{}`",
                t.display(ctx.types),
                method
            ));
            Type::Any
        }
    }
}

fn infer_map_method(
    ctx: &mut CheckCtx,
    k: &Type,
    v: &Type,
    method: &str,
    args_ty: &[Type],
) -> Type {
    match method {
        "get" => {
            check_method_arity(ctx, "get", args_ty, 1);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, k) {
                    ctx.error(format!(
                        "`get` sobre `Map<{}, {}>` espera una clave `{}`, recibió `{}`",
                        k.display(ctx.types),
                        v.display(ctx.types),
                        k.display(ctx.types),
                        arg.display(ctx.types)
                    ));
                }
            }
            Type::Result(Box::new(v.clone()))
        }
        "has" => {
            check_method_arity(ctx, "has", args_ty, 1);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, k) {
                    ctx.error(format!(
                        "`has` sobre `Map<{}, {}>` espera una clave `{}`, recibió `{}`",
                        k.display(ctx.types),
                        v.display(ctx.types),
                        k.display(ctx.types),
                        arg.display(ctx.types)
                    ));
                }
            }
            Type::Bool
        }
        "keys" => {
            check_method_arity(ctx, "keys", args_ty, 0);
            Type::List(Box::new(k.clone()))
        }
        "values" => {
            check_method_arity(ctx, "values", args_ty, 0);
            Type::List(Box::new(v.clone()))
        }
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0);
            Type::Int
        }
        _ => {
            ctx.error(format!(
                "`Map<{}, {}>` no tiene el método `{}`",
                k.display(ctx.types),
                v.display(ctx.types),
                method
            ));
            Type::Any
        }
    }
}

fn infer_str_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type]) -> Type {
    match method {
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0);
            Type::Int
        }
        "upper" | "lower" => {
            check_method_arity(ctx, method, args_ty, 0);
            Type::Str
        }
        _ => {
            ctx.error(format!("`Str` no tiene el método `{}`", method));
            Type::Any
        }
    }
}

/// Chequea exhaustividad de un `match` sobre `Result<T>`. Los arms
/// deben cubrir tanto `Ok` como `Err`, o tener un catch-all
/// (wildcard `_` o ident binding). Patrones literales/de rango
/// sobre un Result no aportan a la exhaustividad — son
/// "imposibles" pero no los rechazamos acá (sería un check
/// separado).
fn check_result_match_exhaustiveness(ctx: &mut CheckCtx, arms: &[crate::ast::MatchArm]) {
    use crate::ast::Pattern;
    let mut has_ok = false;
    let mut has_err = false;
    let mut has_catchall = false;
    for arm in arms {
        match &arm.pattern {
            Pattern::OkBinding(_) => has_ok = true,
            Pattern::ErrBinding(_) => has_err = true,
            Pattern::Wildcard | Pattern::Ident(_) => has_catchall = true,
            _ => {}
        }
    }
    if has_catchall || (has_ok && has_err) {
        return;
    }
    let missing = match (has_ok, has_err) {
        (true, false) => "`Err`",
        (false, true) => "`Ok`",
        _ => "`Ok` y `Err`",
    };
    ctx.error(format!(
        "match sobre `Result` no es exhaustivo: falta el caso {}",
        missing
    ));
}

/// Bindea las variables introducidas por un patrón en el scope
/// actual. `scrutinee` es el tipo del valor que se está matcheando.
fn bind_pattern(ctx: &mut CheckCtx, pat: &crate::ast::Pattern, scrutinee: &Type) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name) => {
            ctx.declare_var(name.clone(), scrutinee.clone());
        }
        Pattern::OkBinding(name) => {
            // `Ok(x)` desempaca `Result<T>` — x es T.
            let inner = match scrutinee {
                Type::Result(t) => (**t).clone(),
                _ => Type::Any,
            };
            ctx.declare_var(name.clone(), inner);
        }
        Pattern::ErrBinding(name) => {
            // `Err(e)` — por convención la E está fijada en Str.
            ctx.declare_var(name.clone(), Type::Str);
        }
        Pattern::Wildcard
        | Pattern::Int(_)
        | Pattern::Float(_)
        | Pattern::Str(_)
        | Pattern::Bool(_)
        | Pattern::Null
        | Pattern::Range { .. } => {
            // No introducen bindings.
        }
    }
}

/// Sintetiza el tipo de un BinOp dado los tipos de sus operandos.
/// Aplica coerción Int→Float donde corresponde.
fn infer_binop(ctx: &mut CheckCtx, op: &BinOpKind, lt: &Type, rt: &Type) -> Type {
    // Si cualquiera de los operandos es Any, no podemos chequear
    // con confianza — devolvemos Any sin error.
    if matches!(lt, Type::Any) || matches!(rt, Type::Any) {
        return Type::Any;
    }
    match op {
        BinOpKind::Add => {
            // Numérico o Str+Str.
            match (lt, rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Int, Type::Float) | (Type::Float, Type::Int) | (Type::Float, Type::Float) => {
                    Type::Float
                }
                (Type::Str, Type::Str) => Type::Str,
                _ => {
                    ctx.error(format!(
                        "el operador `+` no acepta `{}` y `{}`",
                        lt.display(ctx.types),
                        rt.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }
        BinOpKind::Sub | BinOpKind::Mul | BinOpKind::Div => {
            let sym = match op {
                BinOpKind::Sub => "-",
                BinOpKind::Mul => "*",
                BinOpKind::Div => "/",
                _ => unreachable!(),
            };
            match (lt, rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Int, Type::Float) | (Type::Float, Type::Int) | (Type::Float, Type::Float) => {
                    Type::Float
                }
                _ => {
                    ctx.error(format!(
                        "el operador `{}` espera operandos numéricos, recibió `{}` y `{}`",
                        sym,
                        lt.display(ctx.types),
                        rt.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }
        BinOpKind::Lt | BinOpKind::LtEq | BinOpKind::Gt | BinOpKind::GtEq => {
            // Comparación: numéricos o ambos Str.
            let ok = matches!(
                (lt, rt),
                (Type::Int, Type::Int)
                    | (Type::Int, Type::Float)
                    | (Type::Float, Type::Int)
                    | (Type::Float, Type::Float)
                    | (Type::Str, Type::Str)
            );
            if !ok {
                ctx.error(format!(
                    "comparación entre `{}` y `{}` no soportada",
                    lt.display(ctx.types),
                    rt.display(ctx.types)
                ));
            }
            Type::Bool
        }
        BinOpKind::Eq | BinOpKind::NotEq => {
            // Igualdad: cualquier par. El evaluator hace coerción Int↔Float
            // adentro de listas/mapas/etc. No emitimos warning.
            Type::Bool
        }
        BinOpKind::And | BinOpKind::Or => {
            if !matches!(lt, Type::Bool) {
                ctx.error(format!(
                    "el operador lógico espera Bool, lado izquierdo es `{}`",
                    lt.display(ctx.types)
                ));
            }
            if !matches!(rt, Type::Bool) {
                ctx.error(format!(
                    "el operador lógico espera Bool, lado derecho es `{}`",
                    rt.display(ctx.types)
                ));
            }
            Type::Bool
        }
    }
}

/// Compatibilidad para asignación / paso de argumento: `actual` se
/// puede usar donde se espera `expected`?
///
/// Reglas:
///   - `Any` matchea con cualquier cosa (gradual, en ambas direcciones).
///   - `Null` matchea con `T?` para cualquier T.
///   - `T` matchea con `T?` si el inner es compatible.
///   - `Int` matchea con `Float` (coerción implícita en aritmética
///     y asignación).
///   - Generics built-in (`List`/`Map`/`Result`/`Nullable`) y
///     `Function` se comparan recursivamente — así `Result<Any>`
///     pasa por `Result<User>`, `List<Int>` por `List<Float>`, etc.
///   - Resto: igualdad estructural.
pub fn is_compatible(actual: &Type, expected: &Type) -> bool {
    if matches!(actual, Type::Any) || matches!(expected, Type::Any) {
        return true;
    }
    if matches!(actual, Type::Null) && expected.is_nullable() {
        return true;
    }
    // `T` compatible con `T?` (un valor no-null donde se acepta nullable).
    if let Type::Nullable(inner) = expected {
        if is_compatible(actual, inner) {
            return true;
        }
    }
    if matches!(actual, Type::Int) && matches!(expected, Type::Float) {
        return true;
    }
    match (actual, expected) {
        (Type::List(a), Type::List(b)) => is_compatible(a, b),
        (Type::Map(ka, va), Type::Map(kb, vb)) => {
            is_compatible(ka, kb) && is_compatible(va, vb)
        }
        (Type::Result(a), Type::Result(b)) => is_compatible(a, b),
        (Type::Nullable(a), Type::Nullable(b)) => is_compatible(a, b),
        (
            Type::Function { params: pa, ret: ra },
            Type::Function { params: pb, ret: rb },
        ) => {
            pa.len() == pb.len()
                && pa.iter().zip(pb.iter()).all(|(a, b)| is_compatible(a, b))
                && is_compatible(ra, rb)
        }
        _ => actual == expected,
    }
}

/// Walkea una lista de Stmt en orden, manteniendo el scope actual.
fn check_block(ctx: &mut CheckCtx, body: &[Stmt]) {
    for s in body {
        check_stmt(ctx, s);
    }
}

/// Walkea una sola Stmt: chequea sus expresiones, abre scopes,
/// declara variables.
fn check_stmt(ctx: &mut CheckCtx, stmt: &Stmt) {
    match stmt {
        Stmt::Assign { target, type_, value } => {
            let value_ty = infer_expr(ctx, value);
            if let AssignTarget::Ident(name) = target {
                let bound_ty = match type_ {
                    Some(ann) => {
                        let declared = resolve_type_expr(ann, ctx.types).unwrap_or(Type::Any);
                        if !is_compatible(&value_ty, &declared) {
                            ctx.error(format!(
                                "`{}` declarado como `{}` recibió un valor `{}`",
                                name,
                                declared.display(ctx.types),
                                value_ty.display(ctx.types)
                            ));
                        }
                        // El binding usa el tipo declarado, no el inferido.
                        declared
                    }
                    None => value_ty,
                };
                ctx.declare_var(name.clone(), bound_ty);
            }
            // AssignTarget::Field: no introduce variable, no chequeamos
            // tipo del campo todavía (requeriría check del object como
            // Nominal con field y compararlo — bajo el radar de 5.3.1).
        }

        Stmt::Return(e) => {
            // Inferimos siempre para que los errores adentro afloren.
            let ret_ty = infer_expr(ctx, e);
            // Si estamos adentro de una función con return_type
            // declarado (y resoluble), validamos. Fuera de fn o con
            // return_type ausente (Any), no chequeamos — el evaluator
            // ya emite error en runtime si `return` está huérfano.
            if let Some(expected) = ctx.return_stack.last().cloned() {
                if !is_compatible(&ret_ty, &expected) {
                    ctx.error(format!(
                        "`return` devuelve `{}` pero la función declara `{}`",
                        ret_ty.display(ctx.types),
                        expected.display(ctx.types)
                    ));
                }
            }
        }

        Stmt::Expr(e) => {
            let _ = infer_expr(ctx, e);
        }

        Stmt::FnDef {
            params,
            return_type,
            body,
            ..
        } => {
            // Abrimos scope nuevo para params y locales. Los params se
            // bindean con su tipo declarado (o Any). Empujamos el
            // return type esperado al stack para que los `return`
            // adentro lo vean. Sin anotación → `Any` (no chequea).
            let ret = match return_type {
                Some(r) => resolve_type_expr(r, ctx.types).unwrap_or(Type::Any),
                None => Type::Any,
            };
            ctx.push_scope();
            ctx.return_stack.push(ret);
            for p in params {
                let pty = ann_to_type(p.type_.as_ref(), ctx.types);
                ctx.declare_var(p.name.clone(), pty);
            }
            check_block(ctx, body);
            ctx.return_stack.pop();
            ctx.pop_scope();
        }

        Stmt::TypeDef { .. } => {
            // Ya validada por resolve_program.
        }

        Stmt::While { condition, body } => {
            let cond_ty = infer_expr(ctx, condition);
            if !is_compatible(&cond_ty, &Type::Bool) {
                ctx.error(format!(
                    "la condición de `while` debe ser Bool, recibió `{}`",
                    cond_ty.display(ctx.types)
                ));
            }
            ctx.push_scope();
            check_block(ctx, body);
            ctx.pop_scope();
        }

        Stmt::Loop { body } => {
            ctx.push_scope();
            check_block(ctx, body);
            ctx.pop_scope();
        }

        Stmt::For { var, iter, body } => {
            let iter_ty = infer_expr(ctx, iter);
            let elem_ty = match &iter_ty {
                Type::List(t) => (**t).clone(),
                Type::Range => Type::Int,
                Type::Any => Type::Any,
                other => {
                    ctx.error(format!(
                        "el iterable de `for` debe ser List o Range, recibió `{}`",
                        other.display(ctx.types)
                    ));
                    Type::Any
                }
            };
            ctx.push_scope();
            ctx.declare_var(var.clone(), elem_ty);
            check_block(ctx, body);
            ctx.pop_scope();
        }

        Stmt::Break | Stmt::Continue => {}

        Stmt::Import { path } => {
            // `import a.b.c` bindea `c` como Module (Any en el checker).
            if let Some(last) = path.last() {
                ctx.declare_var(last.clone(), Type::Any);
            }
        }

        Stmt::FromImport { names, .. } => {
            // Cada nombre se trae al scope como var. Algunos pueden
            // ser tipos (los chequea StructLit vía TypeEnv, ya
            // registrados en resolve_program), otros funciones o
            // values — sin info del módulo importado, `Any` es lo
            // mejor que tenemos en 5.3.1.
            for n in names {
                ctx.declare_var(n.clone(), Type::Any);
            }
        }
    }
}

/// Pre-registra las firmas de los `Stmt::FnDef` top-level como
/// `Type::Function` en el scope global. Esto destraba referencias
/// hacia adelante y mutuas entre funciones top-level.
fn preregister_fn_signatures(ctx: &mut CheckCtx, program: &Program) {
    for stmt in program {
        if let Stmt::FnDef {
            name,
            params,
            return_type,
            ..
        } = stmt
        {
            let param_types: Vec<Type> = params
                .iter()
                .map(|p| ann_to_type(p.type_.as_ref(), ctx.types))
                .collect();
            let ret = match return_type {
                Some(r) => resolve_type_expr(r, ctx.types).unwrap_or(Type::Any),
                None => Type::Any,
            };
            ctx.declare_var(
                name.clone(),
                Type::Function {
                    params: param_types,
                    ret: Box::new(ret),
                },
            );
        }
    }
}

/// Entrada pública del checker estático completo: corre resolución
/// de anotaciones (`resolve_program`) y luego chequeo de expresiones.
/// Devuelve el env + lista de errores acumulados (mezcla de los dos).
pub fn check_program(program: &Program) -> (TypeEnv, Vec<FitzError>) {
    let (env, mut errors) = resolve_program(program);
    let mut ctx = CheckCtx::new(&env);
    preregister_fn_signatures(&mut ctx, program);
    check_block(&mut ctx, program);
    errors.append(&mut ctx.errors);
    (env, errors)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AssignTarget, Decorator, Field, Param};
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn env_with(types: &[&str]) -> TypeEnv {
        let mut env = TypeEnv::new();
        for n in types {
            env.declare_nominal((*n).into()).unwrap();
        }
        env
    }

    fn resolve_str(src: &str) -> (TypeEnv, Vec<FitzError>) {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        resolve_program(&program)
    }

    // ---- resolve_type_expr ----

    #[test]
    fn resolve_primitivos() {
        let env = TypeEnv::new();
        for (name, expected) in [
            ("Int", Type::Int),
            ("Float", Type::Float),
            ("Str", Type::Str),
            ("Bool", Type::Bool),
            ("Null", Type::Null),
            ("Range", Type::Range),
        ] {
            let r = resolve_type_expr(&TypeExpr::named(name), &env).unwrap();
            assert_eq!(r, expected);
        }
    }

    #[test]
    fn resolve_primitivo_con_args_es_error_de_aridad() {
        // `Int<Str>` no tiene sentido — Int es aridad 0.
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Int".into(),
            args: vec![TypeExpr::named("Str")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeError));
        assert!(err.message.contains("espera 0 argumento(s)"));
    }

    #[test]
    fn resolve_list_de_int() {
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int")],
        };
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::List(Box::new(Type::Int)));
    }

    #[test]
    fn resolve_list_aridad_incorrecta() {
        let env = TypeEnv::new();
        // List sin args
        let t1 = TypeExpr::named("List");
        let err = resolve_type_expr(&t1, &env).unwrap_err();
        assert!(err.message.contains("`List`"));
        assert!(err.message.contains("1 argumento"));

        // List con dos args
        let t2 = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int"), TypeExpr::named("Str")],
        };
        let err = resolve_type_expr(&t2, &env).unwrap_err();
        assert!(err.message.contains("recibió 2"));
    }

    #[test]
    fn resolve_map_de_str_int() {
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::named("Str"), TypeExpr::named("Int")],
        };
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::Map(Box::new(Type::Str), Box::new(Type::Int)));
    }

    #[test]
    fn resolve_map_aridad_incorrecta() {
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::named("Str")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("`Map`"));
        assert!(err.message.contains("2 argumento"));
        assert!(err.message.contains("recibió 1"));
    }

    #[test]
    fn resolve_result_anidado() {
        // Result<List<Int>>
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Result".into(),
            args: vec![TypeExpr::Generic {
                name: "List".into(),
                args: vec![TypeExpr::named("Int")],
            }],
        };
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(
            r,
            Type::Result(Box::new(Type::List(Box::new(Type::Int)))),
        );
    }

    #[test]
    fn resolve_nullable_sobre_primitivo() {
        let env = TypeEnv::new();
        let t = TypeExpr::Nullable(Box::new(TypeExpr::named("Str")));
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::Nullable(Box::new(Type::Str)));
    }

    #[test]
    fn resolve_nullable_sobre_generico() {
        // List<Int>?
        let env = TypeEnv::new();
        let t = TypeExpr::Nullable(Box::new(TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int")],
        }));
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(
            r,
            Type::Nullable(Box::new(Type::List(Box::new(Type::Int)))),
        );
    }

    #[test]
    fn resolve_nominal_declarado() {
        let env = env_with(&["User"]);
        let t = TypeExpr::named("User");
        let r = resolve_type_expr(&t, &env).unwrap();
        let id = env.lookup("User").unwrap();
        assert_eq!(r, Type::Nominal(id));
    }

    #[test]
    fn resolve_nominal_no_definido_es_error() {
        let env = TypeEnv::new();
        let t = TypeExpr::named("Usuario");
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("desconocido"));
        assert!(err.message.contains("Usuario"));
    }

    #[test]
    fn resolve_nominal_con_args_es_error() {
        // El usuario escribe `User<Int>` pero User no es genérico.
        let env = env_with(&["User"]);
        let t = TypeExpr::Generic {
            name: "User".into(),
            args: vec![TypeExpr::named("Int")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("no es genérico"));
    }

    #[test]
    fn resolve_generic_con_arg_invalido_propaga_error() {
        // List<Usuario> — Usuario no existe.
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Usuario")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("Usuario"));
    }

    // ---- TypeEnv ----

    #[test]
    fn type_env_lookup_devuelve_el_id() {
        let env = env_with(&["A", "B"]);
        let a = env.lookup("A").unwrap();
        let b = env.lookup("B").unwrap();
        assert_ne!(a, b);
        assert_eq!(env.info(a).name, "A");
        assert_eq!(env.info(b).name, "B");
    }

    #[test]
    fn type_env_declarar_dos_veces_es_error() {
        let mut env = TypeEnv::new();
        env.declare_nominal("Foo".into()).unwrap();
        let err = env.declare_nominal("Foo".into()).unwrap_err();
        assert!(err.message.contains("`Foo`"));
        assert!(err.message.contains("más de una vez"));
    }

    // ---- resolve_program ----

    #[test]
    fn programa_vacio_no_da_errores() {
        let (env, errors) = resolve_str("");
        assert!(errors.is_empty());
        assert_eq!(env.nominal_count(), 0);
    }

    #[test]
    fn type_con_primitivos_se_resuelve() {
        let (env, errors) = resolve_str("type User { id: Int, name: Str }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
        let id = env.lookup("User").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "id");
        assert_eq!(fields[0].type_, Type::Int);
        assert_eq!(fields[1].type_, Type::Str);
    }

    #[test]
    fn type_con_generico_y_nullable_se_resuelve() {
        let (env, errors) = resolve_str(
            "type Post { tags: List<Str>, author: Str? }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
        let id = env.lookup("Post").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        assert_eq!(fields[0].type_, Type::List(Box::new(Type::Str)));
        assert_eq!(fields[1].type_, Type::Nullable(Box::new(Type::Str)));
    }

    #[test]
    fn type_que_referencia_otro_type_local() {
        let (env, errors) = resolve_str(
            "type Address { city: Str }\n\
             type User { home: Address }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
        let user = env.lookup("User").unwrap();
        let addr = env.lookup("Address").unwrap();
        let user_fields = env.info(user).fields.as_ref().unwrap();
        assert_eq!(user_fields[0].type_, Type::Nominal(addr));
    }

    #[test]
    fn forward_refs_mutuas_se_resuelven() {
        // type A { b: B }; type B { a: A }
        let (env, errors) = resolve_str(
            "type A { b: B }\n\
             type B { a: A }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
        let a = env.lookup("A").unwrap();
        let b = env.lookup("B").unwrap();
        let a_fields = env.info(a).fields.as_ref().unwrap();
        let b_fields = env.info(b).fields.as_ref().unwrap();
        assert_eq!(a_fields[0].type_, Type::Nominal(b));
        assert_eq!(b_fields[0].type_, Type::Nominal(a));
    }

    #[test]
    fn type_con_field_de_tipo_inexistente_reporta_error() {
        let (_, errors) = resolve_str("type User { home: Address }");
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("Address"));
        assert!(msg.contains("desconocido"));
        assert!(msg.contains("campo `home`"));
        assert!(msg.contains("tipo `User`"));
    }

    #[test]
    fn type_redeclarado_es_error() {
        let (_, errors) = resolve_str("type Foo { x: Int }\ntype Foo { y: Str }");
        assert!(errors.iter().any(|e| e.message.contains("Foo")
            && e.message.contains("más de una vez")));
    }

    #[test]
    fn default_literal_compatible_pasa() {
        let (_, errors) = resolve_str(
            "type Cfg { port: Int = 3000, debug: Bool = false }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn default_literal_incompatible_reporta_error() {
        let (_, errors) = resolve_str("type Cfg { port: Int = \"3000\" }");
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("Cfg.port"));
        assert!(msg.contains("`Int`"));
        assert!(msg.contains("`Str`"));
    }

    #[test]
    fn default_null_sobre_campo_nullable_pasa() {
        let (_, errors) = resolve_str("type User { email: Str? = null }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn default_null_sobre_campo_no_nullable_falla() {
        let (_, errors) = resolve_str("type User { id: Int = null }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("User.id"));
    }

    #[test]
    fn default_int_sobre_float_se_acepta_por_coercion() {
        let (_, errors) = resolve_str("type Cfg { ratio: Float = 1 }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn default_no_literal_se_acepta_pending_para_5_3() {
        // Default es una expresión (no literal): suma. El checker la
        // deja pasar — 5.3 chequea expresiones contra tipos.
        let (_, errors) = resolve_str("type Cfg { port: Int = 3000 + 1 }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    // ---- anotaciones de FnDef y Assign ----

    #[test]
    fn fndef_con_anotaciones_resueltas() {
        let (_, errors) = resolve_str(
            "fn add(a: Int, b: Int) -> Int { return a + b }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn fndef_con_tipo_param_invalido_reporta_error() {
        let (_, errors) = resolve_str("fn f(x: Foo) { return x }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
        assert!(errors[0].message.contains("parámetro `x`"));
        assert!(errors[0].message.contains("función `f`"));
    }

    #[test]
    fn fndef_con_return_invalido_reporta_error() {
        let (_, errors) = resolve_str("fn f() -> Foo { return 0 }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
        assert!(errors[0].message.contains("retorno"));
        assert!(errors[0].message.contains("función `f`"));
    }

    #[test]
    fn fndef_con_generico_invalido_reporta_error() {
        // `List<Foo>` donde Foo no existe.
        let (_, errors) = resolve_str("fn f(xs: List<Foo>) { return xs }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
    }

    #[test]
    fn assign_con_tipo_invalido_reporta_error() {
        let (_, errors) = resolve_str("let x: Foo = 0");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
    }

    #[test]
    fn assign_con_generico_valido_pasa() {
        let (_, errors) = resolve_str("let xs: List<Int> = []");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn anotaciones_dentro_del_body_de_fn_se_validan() {
        // El let `y: Foo` está adentro del fn — la pasada baja y lo encuentra.
        let (_, errors) = resolve_str(
            "fn f() {\n\
                let y: Foo = 0\n\
                return y\n\
             }",
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
    }

    #[test]
    fn multiples_errores_se_acumulan_y_no_cortan() {
        let (_, errors) = resolve_str(
            "type A { x: Foo }\n\
             let y: Bar = 0\n\
             fn f(z: Baz) { return z }",
        );
        // Esperamos 3: Foo, Bar, Baz.
        assert_eq!(errors.len(), 3);
        let combined: String = errors.iter().map(|e| e.message.clone()).collect();
        assert!(combined.contains("Foo"));
        assert!(combined.contains("Bar"));
        assert!(combined.contains("Baz"));
    }

    // ---- construcciones AST directas, sin parser ----

    #[test]
    fn resolve_program_construye_env_via_ast_directo() {
        // Sanity: armamos el AST a mano sin pasar por parser para
        // confirmar que resolve_program no depende de detalles del
        // parser.
        use crate::ast::TypeExpr as TE;
        let program: Program = vec![
            Stmt::TypeDef {
                name: "X".into(),
                fields: vec![Field {
                    name: "n".into(),
                    type_: TE::named("Int"),
                    default: None,
                }],
            },
            Stmt::FnDef {
                name: "noop".into(),
                params: vec![Param {
                    name: "p".into(),
                    type_: Some(TE::named("X")),
                }],
                return_type: None,
                body: vec![],
                is_async: false,
                decorators: Vec::<Decorator>::new(),
            },
            Stmt::Assign {
                target: AssignTarget::Ident("v".into()),
                type_: Some(TE::Nullable(Box::new(TE::named("X")))),
                value: Expr::Null,
            },
        ];
        let (env, errors) = resolve_program(&program);
        assert!(errors.is_empty(), "errores: {:?}", errors);
        let x = env.lookup("X").unwrap();
        assert_eq!(env.info(x).fields.as_ref().unwrap()[0].type_, Type::Int);
    }

    // -----------------------------------------------------------------------
    // Tests — checker de expresiones (Fase 5.3.1)
    //
    // Cubrimos la pasada nueva: synth de literales/ident/BinOp/UnaryOp/
    // StrInterp/If/List/Map/StructLit/Field/Range, asignaciones con
    // anotación, scope local (FnDef/FnExpr/Match arms), e imports.
    // -----------------------------------------------------------------------

    fn check_str(src: &str) -> (TypeEnv, Vec<FitzError>) {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        check_program(&program)
    }

    fn assert_ok(src: &str) {
        let (_, errors) = check_str(src);
        assert!(
            errors.is_empty(),
            "esperado sin errores, hubo: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    fn assert_error_with(src: &str, contains: &[&str]) {
        let (_, errors) = check_str(src);
        assert!(!errors.is_empty(), "esperado al menos un error, no hubo");
        let combined: String = errors.iter().map(|e| e.message.clone()).collect();
        for needle in contains {
            assert!(
                combined.contains(needle),
                "mensaje esperado contener `{}`, fue: {}",
                needle,
                combined
            );
        }
    }

    // ---- ident / scope ----

    #[test]
    fn ident_desconocido_emite_warning() {
        assert_error_with("print(no_existe)", &["variable desconocida", "no_existe"]);
    }

    #[test]
    fn ident_conocido_no_emite_error() {
        assert_ok("let x = 1\nprint(x)");
    }

    #[test]
    fn ident_tipo_nominal_como_value_es_any() {
        // `type User { ... }; let u = User { id: 1, name: "x" }` —
        // el StructLit usa el tipo; usar User pelado tampoco rompe.
        // El evaluator registra el type como Value en el env.
        assert_ok("type User { id: Int }\nprint(User)");
    }

    #[test]
    fn builtin_print_y_len_se_consideran_definidos() {
        // print y len existen por defecto.
        assert_ok("print(\"hola\")\nlen([1, 2, 3])");
    }

    // ---- BinOp ----

    #[test]
    fn binop_int_mas_int_es_ok() {
        assert_ok("let x: Int = 1 + 2");
    }

    #[test]
    fn binop_int_mas_float_es_float() {
        // Float := Int + Float (coerción).
        assert_ok("let x: Float = 1 + 2.0");
    }

    #[test]
    fn binop_str_mas_str_es_str() {
        assert_ok("let s: Str = \"a\" + \"b\"");
    }

    #[test]
    fn binop_str_mas_int_es_error() {
        assert_error_with("let x = \"a\" + 1", &["`+`", "Str", "Int"]);
    }

    #[test]
    fn binop_mul_acepta_numericos() {
        assert_ok("let x: Float = 2 * 3.5");
    }

    #[test]
    fn binop_mul_rechaza_str() {
        assert_error_with(
            "let x = \"a\" * 2",
            &["`*`", "operandos numéricos", "Str"],
        );
    }

    #[test]
    fn binop_comparacion_str_str_es_bool() {
        assert_ok("let b: Bool = \"a\" < \"b\"");
    }

    #[test]
    fn binop_comparacion_str_int_es_error() {
        assert_error_with("let b = \"a\" < 1", &["comparación", "Str", "Int"]);
    }

    #[test]
    fn binop_and_con_bool_es_ok() {
        assert_ok("let b: Bool = true and false");
    }

    #[test]
    fn binop_and_con_int_es_error() {
        assert_error_with("let b = 1 and true", &["lógico", "Bool", "Int"]);
    }

    // ---- UnaryOp ----

    #[test]
    fn unary_neg_int_es_ok() {
        assert_ok("let x: Int = -5");
    }

    #[test]
    fn unary_neg_str_es_error() {
        assert_error_with("let x = -\"hola\"", &["negación", "Int", "Str"]);
    }

    // ---- Range ----

    #[test]
    fn range_de_ints_es_ok() {
        assert_ok("let r = 0..10");
    }

    #[test]
    fn range_con_extremo_no_int_es_error() {
        assert_error_with(
            "let r = 0..\"diez\"",
            &["rango", "Int", "Str"],
        );
    }

    // ---- List / Map ----

    #[test]
    fn list_vacia_es_list_any() {
        let (_, errors) = check_str("let xs = []");
        assert!(errors.is_empty(), "{:?}", errors);
    }

    #[test]
    fn list_homogenea_int_es_list_int() {
        // No hay error; el tipo inferido es List<Int>.
        assert_ok("let xs: List<Int> = [1, 2, 3]");
    }

    #[test]
    fn list_anotada_con_tipo_incompatible_es_error() {
        // El RHS sintetiza List<Str>; la anotación es List<Int>.
        assert_error_with(
            "let xs: List<Int> = [\"a\", \"b\"]",
            &["xs", "List<Int>", "List<Str>"],
        );
    }

    #[test]
    fn map_vacio_es_map_any_any() {
        assert_ok("let m = {}");
    }

    // ---- StructLit ----

    #[test]
    fn struct_lit_con_tipo_conocido_y_campos_ok() {
        assert_ok(
            "type User { id: Int, name: Str }\n\
             let u = User { id: 1, name: \"x\" }",
        );
    }

    #[test]
    fn struct_lit_con_tipo_desconocido_es_error() {
        assert_error_with(
            "let u = Usuario { id: 1 }",
            &["Usuario", "no existe"],
        );
    }

    #[test]
    fn struct_lit_campo_de_tipo_incompatible_es_error() {
        assert_error_with(
            "type User { id: Int }\n\
             let u = User { id: \"no soy int\" }",
            &["User.id", "Int", "Str"],
        );
    }

    #[test]
    fn struct_lit_campo_extra_es_error() {
        assert_error_with(
            "type User { id: Int }\n\
             let u = User { id: 1, edad: 30 }",
            &["User", "edad"],
        );
    }

    // ---- Field access ----

    #[test]
    fn field_access_de_nominal_devuelve_tipo_del_campo() {
        // Si u.id es Int, asignarlo a un Int es OK.
        assert_ok(
            "type User { id: Int, name: Str }\n\
             let u = User { id: 1, name: \"x\" }\n\
             let i: Int = u.id",
        );
    }

    #[test]
    fn field_access_de_nominal_tipo_incompatible_es_error() {
        assert_error_with(
            "type User { id: Int, name: Str }\n\
             let u = User { id: 1, name: \"x\" }\n\
             let i: Int = u.name",
            &["Int", "Str"],
        );
    }

    // ---- Assign con anotación ----

    #[test]
    fn assign_int_a_int_es_ok() {
        assert_ok("let x: Int = 42");
    }

    #[test]
    fn assign_str_a_int_es_error() {
        assert_error_with(
            "let x: Int = \"hola\"",
            &["x", "Int", "Str"],
        );
    }

    #[test]
    fn assign_null_a_nullable_es_ok() {
        assert_ok("let x: Str? = null");
    }

    #[test]
    fn assign_int_a_float_es_ok_por_coercion() {
        assert_ok("let x: Float = 1");
    }

    #[test]
    fn assign_str_a_nullable_str_es_ok() {
        // T compatible con T?.
        assert_ok("let x: Str? = \"hola\"");
    }

    // ---- if / while / for ----

    #[test]
    fn if_con_cond_no_bool_es_error() {
        assert_error_with(
            "if 1 { print(\"x\") }",
            &["condición", "if", "Bool", "Int"],
        );
    }

    #[test]
    fn if_con_cond_bool_es_ok() {
        assert_ok("if true { print(\"sí\") } else { print(\"no\") }");
    }

    #[test]
    fn while_con_cond_no_bool_es_error() {
        assert_error_with("while 1 { break }", &["while", "Bool"]);
    }

    #[test]
    fn for_sobre_range_bindea_var_como_int() {
        // Adentro del for, i debe usarse como Int y la suma debe
        // tipear bien.
        assert_ok("for i in 0..10 { let n: Int = i + 1 }");
    }

    #[test]
    fn for_sobre_list_int_bindea_elemento_como_int() {
        assert_ok(
            "let xs = [1, 2, 3]\n\
             for x in xs { let n: Int = x }",
        );
    }

    #[test]
    fn for_sobre_no_iterable_es_error() {
        assert_error_with(
            "for x in 42 { print(x) }",
            &["for", "List", "Range", "Int"],
        );
    }

    // ---- FnDef / params bindeados ----

    #[test]
    fn fndef_param_se_bindea_en_body() {
        // El parámetro n es Int por su anotación.
        assert_ok("fn double(n: Int) -> Int { return n * 2 }");
    }

    #[test]
    fn fndef_param_sin_anotacion_es_any() {
        // Sin anotación, n es Any — no se queja de la suma.
        assert_ok("fn double(n) { return n * 2 }");
    }

    // ---- FnExpr / params bindeados ----

    #[test]
    fn fn_expr_bindea_su_param() {
        // Si no bindeara, `u` seria desconocido.
        assert_ok(
            "type User { id: Int }\n\
             let users = [User { id: 1 }]\n\
             let r = users.find(fn(u) => u.id == 1)",
        );
    }

    // ---- Match con bindings ----

    #[test]
    fn match_ident_pattern_bindea_var() {
        // El brazo `x => ...` bindea x como el tipo del scrutinee.
        assert_ok(
            "let v = 42\n\
             let s = match v {\n\
                 0 => \"cero\"\n\
                 x => \"otro\"\n\
             }",
        );
    }

    #[test]
    fn match_ok_pattern_bindea_inner_de_result() {
        // Ok(v) en match sobre Result<Int> → v es Int.
        // En 5.3.1 el scrutinee es Ok(Int) que tiene tipo Result<Int>,
        // y v se bindea con Int. Verificamos sumando v con un Int.
        assert_ok(
            "let r = Ok(5)\n\
             let s = match r {\n\
                 Ok(v)  => v + 1\n\
                 Err(e) => 0\n\
             }",
        );
    }

    #[test]
    fn match_err_pattern_bindea_inner_como_str() {
        // Err(e) bindea e como Str — concatenable con Str.
        assert_ok(
            "let r = Err(\"boom\")\n\
             let s = match r {\n\
                 Ok(v)  => \"OK\"\n\
                 Err(e) => \"E: \" + e\n\
             }",
        );
    }

    // ---- Imports ----

    #[test]
    fn from_import_bindea_nombres_en_scope() {
        // No podemos cargar un módulo real acá sin tocar disco. Lo
        // que validamos: el ident traído por `from` no se reporta
        // como desconocido.
        assert_ok(
            "from utils import slugify\n\
             let s = slugify",
        );
    }

    #[test]
    fn import_bindea_modulo_como_var() {
        // `import foo` deja `foo` accesible como variable.
        assert_ok(
            "import utils\n\
             let m = utils",
        );
    }

    #[test]
    fn struct_lit_de_tipo_importado_es_ok() {
        // `from foo import User; User { ... }` no falla porque
        // FromImport registra el nombre como nominal sin fields.
        // El checker no valida campos (no los conoce) y deja pasar.
        assert_ok(
            "from foo import User\n\
             let u = User { id: 1, name: \"x\" }",
        );
    }

    // ---- Múltiples errores acumulados ----

    #[test]
    fn checker_acumula_varios_errores_de_expresiones() {
        let (_, errors) = check_str(
            "let a: Int = \"x\"\n\
             let b = 1 + \"y\"\n\
             let c = no_var",
        );
        assert!(errors.len() >= 3, "esperaba 3+ errores, hubo {}: {:?}",
            errors.len(), errors.iter().map(|e| &e.message).collect::<Vec<_>>());
    }

    // ---- 5.3.2: llamadas y return ----

    #[test]
    fn call_aridad_correcta_y_tipos_ok() {
        assert_ok(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n: Int = add(1, 2)",
        );
    }

    #[test]
    fn call_aridad_de_menos_es_error() {
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n = add(1)",
            &["add", "2 argumento", "recibió 1"],
        );
    }

    #[test]
    fn call_aridad_de_mas_es_error() {
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n = add(1, 2, 3)",
            &["add", "2 argumento", "recibió 3"],
        );
    }

    #[test]
    fn call_tipo_de_arg_incompatible_es_error() {
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n = add(\"hola\", 2)",
            &["add", "argumento 1", "Int", "Str"],
        );
    }

    #[test]
    fn call_coercion_int_a_float_pasa() {
        assert_ok(
            "fn double(x: Float) -> Float { return x * 2.0 }\n\
             let n: Float = double(3)",
        );
    }

    #[test]
    fn call_null_a_param_nullable_pasa() {
        assert_ok(
            "fn greet(name: Str?) -> Str { return \"hola\" }\n\
             let g: Str = greet(null)",
        );
    }

    #[test]
    fn call_recursion_top_level_compila() {
        // El pre-registro de firmas debe ver a `fact` antes de chequear
        // su body para que la llamada recursiva no se queje.
        assert_ok(
            "fn fact(n: Int) -> Int {\n\
                 if (n <= 1) { return 1 }\n\
                 return n * fact(n - 1)\n\
             }",
        );
    }

    #[test]
    fn call_forward_reference_cross_fn_compila() {
        // `a` llama a `b` definida después. El pre-registro lo hace
        // visible.
        assert_ok(
            "fn a(n: Int) -> Int { return b(n) + 1 }\n\
             fn b(n: Int) -> Int { return n * 2 }",
        );
    }

    #[test]
    fn call_sobre_callee_no_funcion_es_error() {
        // `1(2)` no es una función llamable.
        assert_error_with(
            "let r = (1)(2)",
            &["no es una función", "Int"],
        );
    }

    #[test]
    fn call_fn_expr_inline_pasa() {
        // (fn(x) => x + 1)(2) — el callee se resuelve a Function.
        // Aridad y param Any → cualquier arg pasa.
        assert_ok("let r = (fn(x) => x + 1)(2)");
    }

    #[test]
    fn call_fn_expr_inline_aridad_falla() {
        // Aridad chequeada incluso en FnExpr inline.
        assert_error_with(
            "let r = (fn(x, y) => x + y)(1)",
            &["2 argumento", "recibió 1"],
        );
    }

    // ---- Builtins ----

    #[test]
    fn len_con_un_arg_pasa_y_devuelve_int() {
        assert_ok("let n: Int = len([1, 2, 3])");
    }

    #[test]
    fn len_sin_args_es_error_de_aridad() {
        assert_error_with(
            "let n = len()",
            &["len", "1 argumento", "recibió 0"],
        );
    }

    #[test]
    fn len_con_dos_args_es_error_de_aridad() {
        assert_error_with(
            "let n = len([1], [2])",
            &["len", "1 argumento", "recibió 2"],
        );
    }

    #[test]
    fn print_es_variadic_no_chequea_aridad() {
        // print sigue siendo Any → cualquier número de args pasa.
        assert_ok("print()\nprint(\"x\")\nprint(1, 2, 3, \"y\")");
    }

    // ---- Stmt::Return contra return_type ----

    #[test]
    fn return_tipo_compatible_pasa() {
        assert_ok(
            "fn double(n: Int) -> Int { return n * 2 }",
        );
    }

    #[test]
    fn return_tipo_incompatible_es_error() {
        assert_error_with(
            "fn double(n: Int) -> Int { return \"no soy int\" }",
            &["return", "Int", "Str"],
        );
    }

    #[test]
    fn return_sin_anotacion_no_chequea() {
        // Sin return_type → Any → no chequea.
        assert_ok("fn f() { return \"cualquier cosa\" }");
    }

    #[test]
    fn return_arrow_implicito_chequea_contra_return_type() {
        // `fn f() -> Int => "x"` se desugarea a `body: [Stmt::Return("x")]`.
        assert_error_with(
            "fn id(x: Int) -> Int => \"no soy int\"",
            &["return", "Int", "Str"],
        );
    }

    #[test]
    fn return_arrow_implicito_correcto_pasa() {
        assert_ok("fn double(n: Int) -> Int => n * 2");
    }

    #[test]
    fn return_ok_contra_result_pasa() {
        // Ok(user) tipea como Result<User>; debe matchear con
        // -> Result<User>.
        assert_ok(
            "type User { id: Int, name: Str }\n\
             fn make(id: Int) -> Result<User> {\n\
                 return Ok(User { id: id, name: \"x\" })\n\
             }",
        );
    }

    #[test]
    fn return_err_contra_result_pasa_por_is_compatible_recursivo() {
        // Err(_) tipea como Result<Any>. Sin recursividad de
        // is_compatible esto fallaría contra Result<User>.
        assert_ok(
            "type User { id: Int }\n\
             fn make() -> Result<User> {\n\
                 return Err(\"boom\")\n\
             }",
        );
    }

    #[test]
    fn return_huerfano_no_chequea() {
        // `return` fuera de una función — el checker no se queja;
        // el evaluator emite error en runtime.
        let (_, errors) = check_str("return 1");
        assert!(errors.is_empty(), "{:?}", errors);
    }

    // ---- is_compatible recursivo en generics ----

    #[test]
    fn is_compatible_list_recursivo() {
        // List<Int> vs List<Float> pasa por coerción Int→Float adentro.
        assert!(is_compatible(
            &Type::List(Box::new(Type::Int)),
            &Type::List(Box::new(Type::Float)),
        ));
        // List<Str> vs List<Int> no pasa.
        assert!(!is_compatible(
            &Type::List(Box::new(Type::Str)),
            &Type::List(Box::new(Type::Int)),
        ));
    }

    #[test]
    fn is_compatible_result_recursivo() {
        // Result<Any> matchea Result<User>.
        let env = env_with(&["User"]);
        let user = Type::Nominal(env.lookup("User").unwrap());
        assert!(is_compatible(
            &Type::Result(Box::new(Type::Any)),
            &Type::Result(Box::new(user.clone())),
        ));
        // Result<Int> no matchea Result<Str>.
        assert!(!is_compatible(
            &Type::Result(Box::new(Type::Int)),
            &Type::Result(Box::new(Type::Str)),
        ));
    }

    #[test]
    fn is_compatible_map_recursivo() {
        // Map<Str, Int> matchea Map<Str, Float>.
        assert!(is_compatible(
            &Type::Map(Box::new(Type::Str), Box::new(Type::Int)),
            &Type::Map(Box::new(Type::Str), Box::new(Type::Float)),
        ));
        // Map<Int, X> no matchea Map<Str, X> (clave incompatible).
        assert!(!is_compatible(
            &Type::Map(Box::new(Type::Int), Box::new(Type::Int)),
            &Type::Map(Box::new(Type::Str), Box::new(Type::Int)),
        ));
    }

    #[test]
    fn is_compatible_function_estructural() {
        // fn(Int) -> Int matchea fn(Int) -> Int.
        let a = Type::Function {
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        };
        let b = Type::Function {
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        };
        assert!(is_compatible(&a, &b));
        // fn(Int) -> Int no matchea fn(Int, Int) -> Int (aridad distinta).
        let c = Type::Function {
            params: vec![Type::Int, Type::Int],
            ret: Box::new(Type::Int),
        };
        assert!(!is_compatible(&a, &c));
    }

    // ---- 5.3.3: `?` y match exhaustivo sobre Result ----

    #[test]
    fn try_sobre_result_adentro_de_fn_result_pasa() {
        // El operando es Result<Int>; la fn declara -> Result<Int>.
        // El `?` desempaca a Int.
        assert_ok(
            "fn f(r: Result<Int>) -> Result<Int> {\n\
                 let v: Int = r?\n\
                 return Ok(v + 1)\n\
             }",
        );
    }

    #[test]
    fn try_sobre_any_no_chequea() {
        // `users.find(...)` es método built-in: callee Field → Any.
        // `?` sobre Any pasa sin chequear (gradual, hasta 5.3.4).
        assert_ok(
            "type User { id: Int }\n\
             fn h(id: Int) {\n\
                 let users = [User { id: 1 }]\n\
                 let u = users.find(fn(u) => u.id == id)?\n\
                 return u\n\
             }",
        );
    }

    #[test]
    fn try_sobre_no_result_es_error() {
        // `?` sobre un Int no tiene sentido.
        assert_error_with(
            "fn f() -> Result<Int> { let x = 1?\n return Ok(x) }",
            &["?", "Result", "Int"],
        );
    }

    #[test]
    fn try_adentro_de_fn_no_result_es_error() {
        // La fn retorna Int (no Result) y adentro hay un `?`. El
        // operando es Result<Int> concreto, así que disparamos la
        // regla "fn debe retornar Result".
        assert_error_with(
            "fn f(r: Result<Int>) -> Int {\n\
                 let v = r?\n\
                 return v\n\
             }",
            &["?", "Result", "Int"],
        );
    }

    #[test]
    fn try_adentro_de_fn_sin_return_type_no_chequea() {
        // Sin anotación → return_stack es Any → no chequeamos la
        // regla de la fn contenedora. El operando sí tiene que ser
        // Result, así que el `?` desempaca a Int sin warnings.
        assert_ok(
            "fn f(r: Result<Int>) {\n\
                 let v: Int = r?\n\
                 return v\n\
             }",
        );
    }

    #[test]
    fn try_top_level_no_chequea_la_regla_de_fn_contenedora() {
        // `?` adentro del scope global — sin return_stack, no
        // disparamos la regla "fn debe retornar Result". El operando
        // sí se chequea: Result<Int> → desempaca a Int.
        assert_ok("let r: Result<Int> = Ok(1)\nlet v: Int = r?");
    }

    #[test]
    fn try_encadenado_con_field_access_funciona() {
        // r?.id sobre Result<User> → User → Int.
        assert_ok(
            "type User { id: Int, name: Str }\n\
             fn f(r: Result<User>) -> Result<Int> {\n\
                 let id: Int = r?.id\n\
                 return Ok(id)\n\
             }",
        );
    }

    // ---- match exhaustivo sobre Result ----

    #[test]
    fn match_result_con_ok_y_err_es_exhaustivo() {
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(v) => \"ok\"\n\
                 Err(e) => \"err\"\n\
             }",
        );
    }

    #[test]
    fn match_result_solo_ok_falta_err() {
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(v) => \"ok\"\n\
             }",
            &["match", "Result", "exhaustivo", "Err"],
        );
    }

    #[test]
    fn match_result_solo_err_falta_ok() {
        assert_error_with(
            "let r: Result<Int> = Err(\"x\")\n\
             let s = match r {\n\
                 Err(e) => \"err\"\n\
             }",
            &["match", "Result", "exhaustivo", "Ok"],
        );
    }

    #[test]
    fn match_result_con_wildcard_solo_es_exhaustivo() {
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 _ => \"cualquier\"\n\
             }",
        );
    }

    #[test]
    fn match_result_con_ok_mas_wildcard_es_exhaustivo() {
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(v) => \"ok\"\n\
                 _ => \"resto\"\n\
             }",
        );
    }

    #[test]
    fn match_result_con_ident_catchall_es_exhaustivo() {
        // Un ident binding (catch-all) cubre cualquier valor — el
        // evaluator lo trata como wildcard.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 x => \"siempre\"\n\
             }",
        );
    }

    #[test]
    fn match_sobre_int_no_exige_exhaustividad() {
        // Match sobre un tipo no-Result: el checker no exige
        // exhaustividad en 5.3.3.
        assert_ok(
            "let n = 1\n\
             let s = match n {\n\
                 0 => \"cero\"\n\
                 1 => \"uno\"\n\
             }",
        );
    }

    #[test]
    fn match_sobre_any_no_exige_exhaustividad() {
        // Match sobre un valor de tipo Any (gradual escape): no se
        // exige exhaustividad.
        assert_ok(
            "fn pick() { return Ok(1) }\n\
             let s = match pick() {\n\
                 Ok(v) => \"ok\"\n\
             }",
        );
    }

    // ---- 5.3.4: métodos built-in con templates paramétricos ----

    // List<T>: push

    #[test]
    fn list_push_con_tipo_compatible_pasa() {
        assert_ok(
            "let xs: List<Int> = [1, 2]\n\
             xs.push(3)",
        );
    }

    #[test]
    fn list_push_con_tipo_incompatible_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             xs.push(\"x\")",
            &["push", "List<Int>", "Str"],
        );
    }

    #[test]
    fn list_push_aridad_incorrecta_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             xs.push(1, 2)",
            &["push", "1 argumento", "recibió 2"],
        );
    }

    // List<T>: pop, len

    #[test]
    fn list_pop_devuelve_t() {
        // Si pop sobre List<Int> devuelve Int, asignarlo a Int es OK.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let last: Int = xs.pop()",
        );
    }

    #[test]
    fn list_len_devuelve_int() {
        assert_ok(
            "let xs = [1, 2, 3]\n\
             let n: Int = xs.len()",
        );
    }

    // List<T>: map

    #[test]
    fn list_map_devuelve_list_del_ret_del_callback() {
        // map sobre List<Int> con callback fn(Int) -> Str → List<Str>.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let strs: List<Str> = xs.map(fn(x: Int) -> Str { return \"x\" })",
        );
    }

    #[test]
    fn list_map_con_callback_param_incompatible_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.map(fn(x: Str) -> Str { return x })",
            &["map", "Int", "Str"],
        );
    }

    #[test]
    fn list_map_con_callback_sin_anotaciones_es_any() {
        // Callback sin anotaciones → params = [Any], ret = Any.
        // El map devuelve List<Any>; asignarlo a List<Int> pasa por
        // is_compatible recursivo + Any.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.map(fn(x) => x * 2)",
        );
    }

    // List<T>: filter

    #[test]
    fn list_filter_devuelve_list_t() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let evens: List<Int> = xs.filter(fn(x: Int) -> Bool { return true })",
        );
    }

    #[test]
    fn list_filter_callback_aridad_incorrecta_es_error() {
        // El FnExpr siempre tiene `ret = Any` hasta 5.3.5, así que
        // no podemos detectar "ret no es Bool" sobre un FnExpr inline.
        // Lo que sí captamos es aridad del callback: filter espera
        // fn(T) -> Bool con un solo param.
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.filter(fn(x, y) => true)",
            &["filter", "1 argumento", "recibió 2"],
        );
    }

    // List<T>: find

    #[test]
    fn list_find_devuelve_result_t() {
        // find sobre List<User> devuelve Result<User>.
        assert_ok(
            "type User { id: Int }\n\
             let xs: List<User> = [User { id: 1 }]\n\
             let r: Result<User> = xs.find(fn(u: User) -> Bool { return true })",
        );
    }

    #[test]
    fn list_find_con_try_destrabba_t() {
        // xs.find(...)? adentro de una fn -> Result<User> debería
        // desempacar a User.
        assert_ok(
            "type User { id: Int }\n\
             fn first(xs: List<User>) -> Result<User> {\n\
                 let u: User = xs.find(fn(u: User) -> Bool { return true })?\n\
                 return Ok(u)\n\
             }",
        );
    }

    // List<T>: método desconocido

    #[test]
    fn list_metodo_desconocido_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             xs.lenght()",
            &["List<Int>", "lenght"],
        );
    }

    // Map<K, V>: get, has

    #[test]
    fn map_get_devuelve_result_v() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let r: Result<Int> = m.get(\"a\")",
        );
    }

    #[test]
    fn map_get_con_clave_incompatible_es_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let r = m.get(42)",
            &["get", "Map<Str, Int>", "Int"],
        );
    }

    #[test]
    fn map_has_devuelve_bool() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let b: Bool = m.has(\"a\")",
        );
    }

    #[test]
    fn map_keys_y_values_devuelven_listas() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let ks: List<Str> = m.keys()\n\
             let vs: List<Int> = m.values()",
        );
    }

    #[test]
    fn map_len_devuelve_int() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let n: Int = m.len()",
        );
    }

    #[test]
    fn map_metodo_desconocido_es_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             m.foo()",
            &["Map<Str, Int>", "foo"],
        );
    }

    // Str

    #[test]
    fn str_upper_lower_devuelven_str() {
        assert_ok(
            "let s = \"hola\"\n\
             let u: Str = s.upper()\n\
             let l: Str = s.lower()",
        );
    }

    #[test]
    fn str_len_devuelve_int() {
        assert_ok(
            "let n: Int = \"hola\".len()",
        );
    }

    #[test]
    fn str_metodo_desconocido_es_error() {
        assert_error_with(
            "let s = \"hola\"\n\
             s.upcase()",
            &["Str", "upcase"],
        );
    }

    // Encadenado

    #[test]
    fn metodo_encadenado_map_filter() {
        // map(...).filter(...) en una sola línea — el ret de map
        // (List<Any> por FnExpr.ret=Any hasta 5.3.5) alimenta al
        // filter. Encadenamiento multi-línea sigue siendo deuda
        // explícita del parser (3.4).
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.map(fn(x) => x * 2).filter(fn(y) => true)",
        );
    }

    // Receptores que no tienen métodos built-in

    #[test]
    fn metodo_sobre_int_es_error() {
        assert_error_with(
            "let n = 1\n\
             n.foo()",
            &["Int", "foo"],
        );
    }

    // Nominal: gradual, no chequea ni rechaza

    #[test]
    fn metodo_sobre_nominal_no_chequea() {
        // type sin métodos custom: user.greet() pasa sin warning
        // (el evaluator lo emite en runtime). Es la regla gradual
        // de 5.3.4 — los métodos custom sobre `type` no existen
        // todavía, no rompemos código que use ese patrón.
        assert_ok(
            "type User { id: Int }\n\
             let u = User { id: 1 }\n\
             u.greet()",
        );
    }
}
