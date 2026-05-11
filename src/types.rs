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

    // Vuelta 1: registrar los nombres de los `type`. Forward refs.
    for stmt in program {
        if let Stmt::TypeDef { name, .. } = stmt {
            if let Err(e) = env.declare_nominal(name.clone()) {
                errors.push(e);
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
}
