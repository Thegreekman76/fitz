// codegen.rs — Fase 5b.1
//
// Transpila el AST de Fitz a código Rust. El binario final lo
// produce `rustc` invocado por el subcomando `fitz build` en
// `main.rs`. No introducimos IR intermedio en 5b.1; un visitor
// sobre el AST tipado por el checker alcanza para el subset
// soportado: literales, BinOp/UnaryOp/StrInterp, asignación,
// `if`/`while`/`loop`/`for in Range`, funciones top-level con
// tipos primitivos, `print`. Cuando entren los tipos compuestos
// (5b.2+) probablemente sumemos un IR pequeño para no acumular
// special cases en este visitor.
//
// Mapping AST de Fitz → Rust:
//
//   Int    → i64
//   Float  → f64
//   Str    → String
//   Bool   → bool
//   Null   → ()
//
// Convenciones:
//   * Variables Fitz se traducen a `let mut x = ...;` en Rust
//     (siempre mut) para simplificar la lógica de reasignación.
//   * Strings se concatenan con `format!("{}{}", a, b)`. Es
//     ineficiente pero evita los juegos de ownership de
//     `String + &str`. Optimizable después.
//   * Coerción Int→Float se inserta como `(x as f64)` en los
//     puntos donde se necesita (BinOp con tipos mixtos,
//     asignación a Float anotado, paso de Int a param Float).
//   * `print(a, b, c)` → `println!("{} {} {}", a, b, c)`. Sin
//     args, `println!()`.
//
// Limitaciones explícitas de 5b.1 (refinar en pasos siguientes):
//   * Solo tipos primitivos. Tipos custom, listas, mapas, Result,
//     módulos, HTTP — fuera de scope.
//   * Funciones anónimas (FnExpr) no se soportan.
//   * Funciones sin `return_type` declarado con cuerpo no vacío
//     que retornan algo → error de codegen. La inferencia desde
//     el body queda para 5b.2.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::ast::{
    AssignTarget, BinOpKind, Expr, Field, Param, Program, Stmt, StrPart, TypeExpr, UnaryOpKind,
};
use crate::error::{ErrorKind, FitzError};
use crate::types::{resolve_type_expr, ResolvedField, Type, TypeEnv, TypeId};

/// Genera código Rust válido a partir de un programa Fitz tipado.
/// El programa debe haber pasado por `check_program` antes (las
/// anotaciones de tipo deben estar resueltas y consistentes).
///
/// Errores acá son de **codegen**: features fuera de scope para
/// 5b.1 (tipos compuestos, FnExpr, fns sin return type que
/// retornan, etc.). No revalidamos lo que el checker ya hizo.
pub fn generate_rust(program: &Program, env: &TypeEnv) -> Result<String, FitzError> {
    let mut ctx = CodegenCtx::new(env);
    ctx.pre_register_types(program)?;
    ctx.pre_register_fns(program)?;

    // Tres categorías de stmts top-level:
    //   * `type Foo { ... }` → structs + alias + impl Display, afuera de `fn main()`.
    //   * `fn ...`            → fns top-level, afuera de `fn main()`.
    //   * el resto            → cuerpo de `fn main()`.
    let mut type_defs: Vec<&Stmt> = Vec::new();
    let mut top_fns: Vec<&Stmt> = Vec::new();
    let mut main_stmts: Vec<&Stmt> = Vec::new();
    for s in program {
        match s {
            Stmt::TypeDef { .. } => type_defs.push(s),
            Stmt::FnDef { .. } => top_fns.push(s),
            _ => main_stmts.push(s),
        }
    }

    ctx.emit_prelude();
    for stmt in &type_defs {
        ctx.gen_type_def(stmt)?;
    }
    for stmt in top_fns {
        ctx.gen_top_fn(stmt)?;
    }
    ctx.gen_main(&main_stmts)?;

    Ok(ctx.output)
}

// ---------------------------------------------------------------------------
// CodegenCtx
// ---------------------------------------------------------------------------

struct CodegenCtx<'a> {
    env: &'a TypeEnv,
    output: String,
    indent: usize,
    /// Stack de scopes de variables locales: nombre → tipo Fitz.
    /// El codegen usa esto para inferir tipos en expresiones y
    /// para decidir entre `let mut` (primera asignación) y `=`
    /// (reasignación).
    scopes: Vec<HashMap<String, Type>>,
    /// Firmas de las funciones top-level: nombre → (params, ret).
    /// Pre-registrado antes de emitir cuerpos, para que las
    /// llamadas resuelvan el ret type sin importar el orden.
    fn_sigs: HashMap<String, FnSig>,
    /// Firmas de los tipos custom declarados en el programa:
    /// nombre → (TypeId, lista de campos con tipo resuelto + default
    /// AST). Pre-registrado antes de emitir structs, para que las
    /// instancias y los field accesses puedan resolver tipos de
    /// campo sin volver a iterar el AST.
    type_sigs: HashMap<String, TypeSig>,
}

#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<Type>,
    ret: Type,
}

/// Info de un tipo custom durante el codegen. Combina los datos
/// resueltos del checker (tipos por campo) con los defaults del AST
/// (que el checker no conserva): los necesitamos para inline-ar los
/// defaults en cada struct literal que omita el campo.
#[derive(Debug, Clone)]
struct TypeSig {
    #[allow(dead_code)]
    id: TypeId,
    fields: Vec<TypeSigField>,
}

#[derive(Debug, Clone)]
struct TypeSigField {
    name: String,
    type_: Type,
    /// Default expr del campo, tomado del AST de `Stmt::TypeDef`.
    /// `None` si el campo no tenía default declarado.
    default: Option<Expr>,
}

impl<'a> CodegenCtx<'a> {
    fn new(env: &'a TypeEnv) -> Self {
        Self {
            env,
            output: String::new(),
            indent: 0,
            scopes: vec![HashMap::new()],
            fn_sigs: HashMap::new(),
            type_sigs: HashMap::new(),
        }
    }

    // --- emit helpers -----------------------------------------------------

    fn emit(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn emit_indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("    ");
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare_var(&mut self, name: String, ty: Type) {
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

    fn var_in_any_scope(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains_key(name))
    }

    // --- error helpers ----------------------------------------------------

    fn err(&self, msg: impl Into<String>) -> FitzError {
        FitzError::new(ErrorKind::TypeError, 0, 0, msg.into())
    }

    // --- prelude + main shell ---------------------------------------------

    fn emit_prelude(&mut self) {
        self.emit("// Código generado por Fitz 5b — no editar a mano.\n");
        self.emit("#![allow(unused_mut, unused_variables, unused_assignments, dead_code)]\n\n");
        // Rc<RefCell<>> es la representación de las instancias de
        // tipos custom — coincide con el modelo del intérprete (las
        // mutaciones se ven a través de cualquier alias).
        self.emit("use std::rc::Rc;\n");
        self.emit("use std::cell::RefCell;\n\n");
        // Helper de formato para Float: alinea con `Display` del
        // intérprete (`3.0` se imprime como `\"3.0\"`, no `\"3\"`).
        self.emit(
            "fn __fitz_fmt_float(v: f64) -> String {\n    \
             if v.is_finite() && v.fract() == 0.0 { format!(\"{:.1}\", v) } else { format!(\"{}\", v) }\n}\n\n",
        );
    }

    fn gen_main(&mut self, stmts: &[&Stmt]) -> Result<(), FitzError> {
        self.emit("fn main() {\n");
        self.indent += 1;
        self.push_scope();
        for stmt in stmts {
            self.gen_stmt(stmt)?;
        }
        self.pop_scope();
        self.indent -= 1;
        self.emit("}\n");
        Ok(())
    }

    // --- pre-registro de tipos custom -------------------------------------

    /// Recorre los `Stmt::TypeDef` del programa y arma `type_sigs` con
    /// el `TypeId`, los campos con tipo resuelto (vía `TypeEnv`) y la
    /// expresión default (vía AST). El checker ya validó nombres y
    /// recursividad de tipos, así que acá los `lookup`/`info` siempre
    /// resuelven.
    fn pre_register_types(&mut self, program: &Program) -> Result<(), FitzError> {
        for stmt in program {
            let Stmt::TypeDef { name, fields: ast_fields } = stmt else { continue };
            let id = self.env.lookup(name).ok_or_else(|| {
                self.err(format!("tipo `{}` no registrado en el TypeEnv (¿checker no corrió?)", name))
            })?;
            let resolved: Vec<ResolvedField> = match &self.env.info(id).fields {
                Some(fs) => fs.clone(),
                None => {
                    return Err(self.err(format!(
                        "tipo `{}`: campos no resueltos por el checker — no se puede codegen",
                        name
                    )));
                }
            };
            // Combinamos: el orden viene de los `ResolvedField` (que
            // el checker mantiene en orden de declaración). Para cada
            // uno, buscamos el AST por nombre para sacar el default.
            let mut combined = Vec::with_capacity(resolved.len());
            for r in resolved {
                let default = ast_fields
                    .iter()
                    .find(|f: &&Field| f.name == r.name)
                    .and_then(|f| f.default.clone());
                combined.push(TypeSigField {
                    name: r.name,
                    type_: r.type_,
                    default,
                });
            }
            self.type_sigs.insert(name.clone(), TypeSig { id, fields: combined });
        }
        Ok(())
    }

    // --- pre-registro de fns top-level ------------------------------------

    fn pre_register_fns(&mut self, program: &Program) -> Result<(), FitzError> {
        for stmt in program {
            if let Stmt::FnDef {
                name,
                params,
                return_type,
                ..
            } = stmt
            {
                let params: Vec<Type> = params
                    .iter()
                    .map(|p| self.resolve_param_type(name, &p.name, p.type_.as_ref()))
                    .collect::<Result<_, _>>()?;
                let ret = match return_type {
                    Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                        FitzError::new(
                            e.kind,
                            0,
                            0,
                            format!(
                                "fn `{}`: return type no resuelve: {}",
                                name, e.message
                            ),
                        )
                    })?,
                    None => Type::Null,
                };
                self.fn_sigs.insert(name.clone(), FnSig { params, ret });
            }
        }
        Ok(())
    }

    fn resolve_param_type(
        &self,
        fn_name: &str,
        param_name: &str,
        type_: Option<&TypeExpr>,
    ) -> Result<Type, FitzError> {
        match type_ {
            Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                FitzError::new(
                    e.kind,
                    0,
                    0,
                    format!(
                        "fn `{}`: parámetro `{}`: {}",
                        fn_name, param_name, e.message
                    ),
                )
            }),
            None => Err(self.err(format!(
                "fn `{}`: el parámetro `{}` necesita una anotación de tipo para el codegen (5b.1)",
                fn_name, param_name
            ))),
        }
    }

    // --- generación de tipos custom ---------------------------------------

    fn gen_type_def(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::TypeDef { name, .. } = stmt else {
            unreachable!("gen_type_def solo se llama sobre Stmt::TypeDef");
        };
        let sig = self
            .type_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("tipo `{}` no pre-registrado", name)))?;

        let data_name = format!("{}Data", name);

        // struct <Foo>Data { f1: T1, f2: T2, ... }
        //
        // `PartialEq` derivado compara campo a campo. Para campos
        // `Rc<RefCell<T>>` (instancias anidadas) `PartialEq` de
        // `Rc<T>` compara por **contenido** (no identidad), y
        // `RefCell<T>` compara borroweando — matchea exacto la
        // semántica estructural del intérprete.
        write!(
            &mut self.output,
            "#[derive(Clone, PartialEq)]\nstruct {} {{\n",
            data_name
        )
        .unwrap();
        for f in &sig.fields {
            write!(
                &mut self.output,
                "    {}: {},\n",
                f.name,
                rust_type_for(&f.type_, self.env)?
            )
            .unwrap();
        }
        self.emit("}\n\n");

        // type Foo = Rc<RefCell<FooData>>;
        write!(
            &mut self.output,
            "type {} = Rc<RefCell<{}>>;\n\n",
            name, data_name
        )
        .unwrap();

        // impl Display for FooData — reproduce el formato del
        // intérprete: `Foo { f1: v1, f2: v2 }`. Strings con comillas,
        // Floats con `.0` si fracción 0, instancias delegando a su
        // propio Display, Option como `null` cuando None.
        write!(
            &mut self.output,
            "impl std::fmt::Display for {} {{\n    fn fmt(&self, __f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n",
            data_name
        )
        .unwrap();
        if sig.fields.is_empty() {
            // `Foo {}` — sin espacios.
            write!(&mut self.output, "        write!(__f, \"{} {{{{}}}}\")\n    }}\n}}\n\n", name).unwrap();
        } else {
            write!(&mut self.output, "        write!(__f, \"{} {{{{\")?;\n", name).unwrap();
            for (i, f) in sig.fields.iter().enumerate() {
                if i > 0 {
                    self.emit("        write!(__f, \",\")?;\n");
                }
                write!(&mut self.output, "        write!(__f, \" {}: \")?;\n", f.name).unwrap();
                let field_expr = format!("self.{}", f.name);
                let stmt = inline_display_stmt(&field_expr, &f.type_);
                self.emit(&stmt);
            }
            self.emit("        write!(__f, \" }}\")\n");
            self.emit("    }\n}\n\n");
        }
        Ok(())
    }

    // --- generación de funciones top-level --------------------------------

    fn gen_top_fn(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        let Stmt::FnDef {
            name,
            params,
            return_type,
            body,
            decorators,
            ..
        } = stmt
        else {
            unreachable!("gen_top_fn solo se llama sobre Stmt::FnDef");
        };

        if !decorators.is_empty() {
            return Err(self.err(format!(
                "fn `{}`: decoradores (`@get`/`@post`/`@server`/etc.) no soportados en 5b.1 — HTTP llega en 5b.6",
                name
            )));
        }

        let sig = self
            .fn_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("fn `{}` no estaba pre-registrada", name)))?;

        // Header: fn <name>(p1: T1, p2: T2, ...) -> Ret {
        self.emit("fn ");
        self.emit(name);
        self.emit("(");
        for (i, (param, pty)) in params.iter().zip(sig.params.iter()).enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.emit("mut ");
            self.emit(&param.name);
            self.emit(": ");
            self.emit(&rust_type_for(pty, self.env)?);
        }
        self.emit(")");
        if !matches!(sig.ret, Type::Null) {
            self.emit(" -> ");
            self.emit(&rust_type_for(&sig.ret, self.env)?);
        }
        self.emit(" {\n");

        // Body
        self.indent += 1;
        self.push_scope();
        for (param, pty) in params.iter().zip(sig.params.iter()) {
            self.declare_var(param.name.clone(), pty.clone());
        }
        // Frame de "return esperado" para coerciones.
        for stmt in body {
            self.gen_stmt_in_fn(stmt, &sig.ret)?;
        }
        self.pop_scope();
        self.indent -= 1;

        self.emit("}\n\n");
        Ok(())
    }

    // --- generación de statements -----------------------------------------

    fn gen_stmt(&mut self, stmt: &Stmt) -> Result<(), FitzError> {
        // En el scope top-level (main), no hay return_type — usamos
        // Null como placeholder (los `return` ahí adentro son raros
        // pero válidos; el evaluator también los emite como huérfanos).
        self.gen_stmt_in_fn(stmt, &Type::Null)
    }

    fn gen_stmt_in_fn(&mut self, stmt: &Stmt, ret_expected: &Type) -> Result<(), FitzError> {
        match stmt {
            Stmt::Assign { target, type_, value } => self.gen_assign(target, type_.as_ref(), value),
            Stmt::Return(e) => self.gen_return(e, ret_expected),
            Stmt::Expr(e) => {
                self.emit_indent();
                self.gen_expr_for_stmt(e)?;
                self.emit(";\n");
                Ok(())
            }
            Stmt::While { condition, body } => self.gen_while(condition, body, ret_expected),
            Stmt::Loop { body } => self.gen_loop(body, ret_expected),
            Stmt::For { var, iter, body } => self.gen_for(var, iter, body, ret_expected),
            Stmt::Break => {
                self.emit_indent();
                self.emit("break;\n");
                Ok(())
            }
            Stmt::Continue => {
                self.emit_indent();
                self.emit("continue;\n");
                Ok(())
            }
            Stmt::FnDef { name, .. } => Err(self.err(format!(
                "fn anidada `{}`: no soportada en 5b.1 — declarala a nivel top",
                name
            ))),
            Stmt::TypeDef { name, .. } => Err(self.err(format!(
                "`type {}`: solo se admite a nivel top, no adentro de funciones u otros bloques",
                name
            ))),
            Stmt::Import { .. } | Stmt::FromImport { .. } => Err(self.err(
                "`import`: módulos no soportados en 5b.1 — llegan en 5b.5",
            )),
        }
    }

    fn gen_assign(
        &mut self,
        target: &AssignTarget,
        type_: Option<&TypeExpr>,
        value: &Expr,
    ) -> Result<(), FitzError> {
        let name = match target {
            AssignTarget::Ident(n) => n,
            AssignTarget::Field { object, field } => {
                return self.gen_field_assign(object, field, value);
            }
        };

        let (rhs_code, rhs_ty) = self.gen_expr(value)?;
        let declared_ty = match type_ {
            Some(t) => resolve_type_expr(t, self.env).map_err(|e| {
                self.err(format!("anotación de `{}` no resuelve: {}", name, e.message))
            })?,
            None => rhs_ty.clone(),
        };

        let final_rhs = coerce(&rhs_code, &rhs_ty, &declared_ty);
        self.emit_indent();
        // Si la var ya existe en algún scope visible (outer o
        // current), es reasignación: emitimos `name = ...`. Si no,
        // declaración: `let mut name: T = ...`. NOTA: una "primera
        // asignación" adentro de un bloque (while/loop/for body)
        // queda confinada a ese bloque en el Rust generado, mientras
        // que en Fitz persistiría afuera. Es una discrepancia
        // conocida del codegen 5b.1; refinarla pide pre-declarar
        // todas las vars del programa, que llega después.
        if self.var_in_any_scope(name) {
            // Reasignación.
            self.emit(name);
            self.emit(" = ");
            self.emit(&final_rhs);
            self.emit(";\n");
            // El scope ya tiene el tipo del binding original; lo
            // mantenemos.
        } else {
            // Primera vez en este scope — declaración.
            self.emit("let mut ");
            self.emit(name);
            self.emit(": ");
            self.emit(&rust_type_for(&declared_ty, self.env)?);
            self.emit(" = ");
            self.emit(&final_rhs);
            self.emit(";\n");
            self.declare_var(name.clone(), declared_ty);
        }
        Ok(())
    }

    fn gen_field_assign(
        &mut self,
        object: &Expr,
        field: &str,
        value: &Expr,
    ) -> Result<(), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        let Type::Nominal(id) = &obj_ty else {
            return Err(self.err(format!(
                "asignación a campo `.{}` sobre `{}`: solo se soporta sobre instancias",
                field,
                type_name(&obj_ty)
            )));
        };
        let info = self.env.info(*id);
        let declared = info.fields.clone().ok_or_else(|| {
            self.err(format!(
                "tipo `{}` con campos sin resolver — no se puede generar asignación",
                info.name
            ))
        })?;
        let Some(f) = declared.iter().find(|f| f.name == field) else {
            return Err(self.err(format!(
                "el tipo `{}` no tiene un campo llamado `{}`",
                info.name, field
            )));
        };
        let (rhs_code, rhs_ty) = self.gen_expr(value)?;
        let coerced = coerce(&rhs_code, &rhs_ty, &f.type_);
        self.emit_indent();
        write!(
            &mut self.output,
            "({}).borrow_mut().{} = {};\n",
            obj_code, field, coerced
        )
        .unwrap();
        Ok(())
    }

    fn gen_return(&mut self, e: &Expr, ret_expected: &Type) -> Result<(), FitzError> {
        let (code, ty) = self.gen_expr(e)?;
        let coerced = coerce(&code, &ty, ret_expected);
        self.emit_indent();
        self.emit("return ");
        self.emit(&coerced);
        self.emit(";\n");
        Ok(())
    }

    fn gen_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
        ret_expected: &Type,
    ) -> Result<(), FitzError> {
        let (cond_code, _) = self.gen_expr(condition)?;
        self.emit_indent();
        self.emit("while ");
        self.emit(&cond_code);
        self.emit(" {\n");
        self.indent += 1;
        self.push_scope();
        for s in body {
            self.gen_stmt_in_fn(s, ret_expected)?;
        }
        self.pop_scope();
        self.indent -= 1;
        self.emit_indent();
        self.emit("}\n");
        Ok(())
    }

    fn gen_loop(&mut self, body: &[Stmt], ret_expected: &Type) -> Result<(), FitzError> {
        self.emit_indent();
        self.emit("loop {\n");
        self.indent += 1;
        self.push_scope();
        for s in body {
            self.gen_stmt_in_fn(s, ret_expected)?;
        }
        self.pop_scope();
        self.indent -= 1;
        self.emit_indent();
        self.emit("}\n");
        Ok(())
    }

    fn gen_for(
        &mut self,
        var: &str,
        iter: &Expr,
        body: &[Stmt],
        ret_expected: &Type,
    ) -> Result<(), FitzError> {
        // Dos iterables soportados hoy:
        //   * `for v in start..end` — rango exclusivo (5b.1).
        //   * `for v in xs` con xs: List<T> — itera sobre snapshot.
        //     Snapshot (clone del Vec interno) para evitar re-entrancia
        //     al RefCell si el body muta la lista original. Mismo patrón
        //     que list_map en el intérprete.
        // Map como iterable directo NO se soporta (alineado con el
        // intérprete, que también lo rechaza).
        if let Expr::Range { start, end } = iter {
            let (start_code, _) = self.gen_expr(start)?;
            let (end_code, _) = self.gen_expr(end)?;
            self.emit_indent();
            write!(
                &mut self.output,
                "for mut {var} in ({start_code} as i64)..({end_code} as i64) {{\n"
            )
            .unwrap();
            self.indent += 1;
            self.push_scope();
            self.declare_var(var.to_string(), Type::Int);
            for s in body {
                self.gen_stmt_in_fn(s, ret_expected)?;
            }
            self.pop_scope();
            self.indent -= 1;
            self.emit_indent();
            self.emit("}\n");
            return Ok(());
        }
        // Caso general: el iter tiene que evaluar a List<T>.
        let (iter_code, iter_ty) = self.gen_expr(iter)?;
        let elem_ty = match &iter_ty {
            Type::List(inner) => (**inner).clone(),
            other => {
                return Err(self.err(format!(
                    "`for {} in <expr>`: el iterable es `{}`, solo se soportan Range y List<T>",
                    var,
                    display_type(other, self.env)
                )));
            }
        };
        if matches!(elem_ty, Type::Any) {
            return Err(self.err(format!(
                "`for {} in ...` sobre `List<Any>`: el subset compilado exige tipo homogéneo \
                 concreto",
                var
            )));
        }
        self.emit_indent();
        write!(
            &mut self.output,
            "for mut {var} in ({iter_code}).borrow().clone().into_iter() {{\n"
        )
        .unwrap();
        self.indent += 1;
        self.push_scope();
        self.declare_var(var.to_string(), elem_ty);
        for s in body {
            self.gen_stmt_in_fn(s, ret_expected)?;
        }
        self.pop_scope();
        self.indent -= 1;
        self.emit_indent();
        self.emit("}\n");
        Ok(())
    }

    // --- generación de expresiones ----------------------------------------

    /// Devuelve `(código Rust de la expresión, tipo Fitz)`.
    fn gen_expr(&mut self, e: &Expr) -> Result<(String, Type), FitzError> {
        match e {
            Expr::Int(n) => Ok((format!("{}i64", n), Type::Int)),
            Expr::Float(n) => {
                // `1.0` ya es f64 literal en Rust; sufijo opcional
                // pero claro. Para evitar `inf`/`-inf` corner cases
                // delegamos al Display de f64 que produce literal
                // válido.
                Ok((format!("{}f64", n), Type::Float))
            }
            Expr::Str(s) => Ok((format!("String::from({})", rust_str_literal(s)), Type::Str)),
            Expr::Bool(b) => Ok((b.to_string(), Type::Bool)),
            Expr::Null => Ok(("()".to_string(), Type::Null)),

            Expr::Ident(name) => {
                let ty = self
                    .lookup_var(name)
                    .cloned()
                    .ok_or_else(|| self.err(format!("variable desconocida en codegen: `{}`", name)))?;
                // Para tipos no-Copy (Str, Nominal, Option<...>),
                // generamos `.clone()` porque las expresiones consumen
                // por valor. Es ineficiente pero correcto. Para
                // Nominal el clone es del `Rc`, así que es barato y
                // preserva el aliasing — mutaciones siguen visibles.
                let code = if needs_clone(&ty) {
                    format!("{}.clone()", name)
                } else {
                    name.clone()
                };
                Ok((code, ty))
            }

            Expr::StrInterp(parts) => self.gen_str_interp(parts),

            Expr::BinOp { op, left, right } => self.gen_binop(op, left, right),
            Expr::UnaryOp { op, operand } => self.gen_unary(op, operand),

            Expr::Call { callee, args } => self.gen_call(callee, args),

            Expr::If { condition, then, else_ } => {
                self.gen_if_expr(condition, then, else_.as_deref())
            }

            Expr::Range { .. } => Err(self.err(
                "`Range` solo se acepta como iterable de `for`; otros usos no se generan",
            )),
            Expr::List(items) => self.gen_list_lit(items),
            Expr::Map(pairs) => self.gen_map_lit(pairs),
            Expr::Index { object, index } => self.gen_index(object, index),
            Expr::Field { object, field } => self.gen_field_access(object, field),
            Expr::StructLit { type_name, fields } => self.gen_struct_lit(type_name, fields),
            Expr::Ok(_) | Expr::Err(_) | Expr::Try(_) => Err(self.err(
                "Result / `Ok` / `Err` / `?`: no soportados en 5b.1 — llegan en 5b.4",
            )),
            Expr::Match { .. } => Err(self.err(
                "`match`: requiere Result/tipos custom — 5b.4",
            )),
            // FnExpr "suelto" — usado como valor, parámetro o retorno —
            // requiere closures escapados con tipo (Box<dyn Fn(...)>) y
            // captura por clone explícita. Higher-order completo queda
            // como sub-paso después de 5b.4 (Result). El único FnExpr
            // que SÍ se acepta hoy es como callback inline de
            // `.map`/`.filter` sobre List, y se intercepta en
            // `gen_method_call` antes de llegar acá.
            Expr::FnExpr { .. } => Err(self.err(
                "funciones anónimas `fn(...) => ...` solo se admiten hoy como callback inline de \
                 `.map(...)` o `.filter(...)` sobre listas. Usarlas como valor, parámetro o \
                 retorno (higher-order) llega en un sub-paso posterior de 5b.",
            )),
        }
    }

    /// Para statements `Stmt::Expr(e)`: si `e` es una llamada a
    /// `print(...)`, generamos `println!(...)` (que devuelve `()`).
    /// El resto cae al `gen_expr` normal.
    fn gen_expr_for_stmt(&mut self, e: &Expr) -> Result<(), FitzError> {
        if let Expr::Call { callee, args } = e {
            if let Expr::Ident(name) = callee.as_ref() {
                if name == "print" {
                    return self.gen_print(args);
                }
            }
        }
        let (code, _) = self.gen_expr(e)?;
        self.emit(&code);
        Ok(())
    }

    fn gen_print(&mut self, args: &[Expr]) -> Result<(), FitzError> {
        if args.is_empty() {
            self.emit("println!()");
            return Ok(());
        }
        // Para cada arg evaluamos el código y elegimos cómo formatearlo:
        //   * tipos "simples" (Int, Str, Bool) → `{}` con el arg directo,
        //     que es lo que el `println!` nativo ya hace bien;
        //   * tipos que necesitan formato custom (Float con `.0`, Null
        //     como `"null"`, instancias delegando a Display, Options
        //     desempaquetando a `null`) → expresión via `show_expr`
        //     que evalúa a `String`, todavía pasada con `{}`.
        let mut pieces: Vec<String> = Vec::with_capacity(args.len());
        for a in args {
            let (code, ty) = self.gen_expr(a)?;
            let piece = match &ty {
                Type::Int | Type::Bool | Type::Str => code,
                _ => show_expr(&code, &ty),
            };
            pieces.push(piece);
        }
        let format_str: String = std::iter::repeat("{}")
            .take(args.len())
            .collect::<Vec<_>>()
            .join(" ");
        self.emit(&format!(
            "println!(\"{}\", {})",
            format_str,
            pieces.join(", ")
        ));
        Ok(())
    }

    fn gen_str_interp(&mut self, parts: &[StrPart]) -> Result<(String, Type), FitzError> {
        let mut fmt = String::new();
        let mut args: Vec<String> = Vec::new();
        for p in parts {
            match p {
                StrPart::Lit(s) => {
                    // Escapamos `{` y `}` para el format string.
                    for c in s.chars() {
                        match c {
                            '{' => fmt.push_str("{{"),
                            '}' => fmt.push_str("}}"),
                            '\\' => fmt.push_str("\\\\"),
                            '"' => fmt.push_str("\\\""),
                            _ => fmt.push(c),
                        }
                    }
                }
                StrPart::Expr(e) => {
                    fmt.push_str("{}");
                    let (code, ty) = self.gen_expr(e)?;
                    // Para tipos formateables nativos (Int/Bool/Str),
                    // pasamos la expresión directo. Para el resto
                    // (Float con `.0`, Null como `null`, instancias
                    // por Display, Option desempacando) usamos
                    // `show_expr` que devuelve un `String`.
                    let piece = match &ty {
                        Type::Int | Type::Bool | Type::Str => code,
                        _ => show_expr(&code, &ty),
                    };
                    args.push(piece);
                }
            }
        }
        let call = if args.is_empty() {
            format!("String::from(\"{}\")", fmt)
        } else {
            format!("format!(\"{}\", {})", fmt, args.join(", "))
        };
        Ok((call, Type::Str))
    }

    fn gen_binop(
        &mut self,
        op: &BinOpKind,
        left: &Expr,
        right: &Expr,
    ) -> Result<(String, Type), FitzError> {
        let (lc, lt) = self.gen_expr(left)?;
        let (rc, rt) = self.gen_expr(right)?;
        match op {
            BinOpKind::Add => {
                // Str+Str → format!("{}{}", a, b).
                if matches!(lt, Type::Str) && matches!(rt, Type::Str) {
                    return Ok((format!("format!(\"{{}}{{}}\", {}, {})", lc, rc), Type::Str));
                }
                let (l, r, t) = numeric_coerce(&lc, &lt, &rc, &rt)
                    .ok_or_else(|| self.err(format!(
                        "operador `+` no aplicable a `{}` y `{}` en codegen",
                        type_name(&lt),
                        type_name(&rt)
                    )))?;
                Ok((format!("({} + {})", l, r), t))
            }
            BinOpKind::Sub | BinOpKind::Mul | BinOpKind::Div => {
                let sym = match op {
                    BinOpKind::Sub => "-",
                    BinOpKind::Mul => "*",
                    BinOpKind::Div => "/",
                    _ => unreachable!(),
                };
                let (l, r, t) = numeric_coerce(&lc, &lt, &rc, &rt)
                    .ok_or_else(|| self.err(format!(
                        "operador `{}` no aplicable a `{}` y `{}` en codegen",
                        sym, type_name(&lt), type_name(&rt)
                    )))?;
                Ok((format!("({} {} {})", l, sym, r), t))
            }
            BinOpKind::Lt | BinOpKind::LtEq | BinOpKind::Gt | BinOpKind::GtEq => {
                let sym = match op {
                    BinOpKind::Lt => "<",
                    BinOpKind::LtEq => "<=",
                    BinOpKind::Gt => ">",
                    BinOpKind::GtEq => ">=",
                    _ => unreachable!(),
                };
                // Para Str: usamos `as_str()` para comparar.
                if matches!(lt, Type::Str) && matches!(rt, Type::Str) {
                    return Ok((
                        format!("({}.as_str() {} {}.as_str())", lc, sym, rc),
                        Type::Bool,
                    ));
                }
                let (l, r, _t) = numeric_coerce(&lc, &lt, &rc, &rt)
                    .ok_or_else(|| self.err(format!(
                        "comparación entre `{}` y `{}` no aplicable",
                        type_name(&lt), type_name(&rt)
                    )))?;
                Ok((format!("({} {} {})", l, sym, r), Type::Bool))
            }
            BinOpKind::Eq | BinOpKind::NotEq => {
                let sym = match op {
                    BinOpKind::Eq => "==",
                    BinOpKind::NotEq => "!=",
                    _ => unreachable!(),
                };
                // Igualdad estructural entre instancias del mismo
                // tipo: borroweamos ambos lados y comparamos por
                // valor — `#[derive(PartialEq)]` sobre `FooData`
                // recursea campo a campo (incluyendo nominales
                // anidados como `Rc<RefCell<T>>`, que comparan por
                // contenido, no identidad).
                if let (Type::Nominal(id_l), Type::Nominal(id_r)) = (&lt, &rt) {
                    if id_l != id_r {
                        return Err(self.err(
                            "igualdad entre instancias de tipos distintos: el checker debería haberlo cazado",
                        ));
                    }
                    return Ok((
                        format!("(*({}).borrow() {} *({}).borrow())", lc, sym, rc),
                        Type::Bool,
                    ));
                }
                if matches!(lt, Type::Str) && matches!(rt, Type::Str) {
                    return Ok((format!("({} {} {})", lc, sym, rc), Type::Bool));
                }
                // Numéricos con posible coerción Int↔Float.
                if let Some((l, r, _)) = numeric_coerce(&lc, &lt, &rc, &rt) {
                    return Ok((format!("({} {} {})", l, sym, r), Type::Bool));
                }
                // Bools, Null directos.
                Ok((format!("({} {} {})", lc, sym, rc), Type::Bool))
            }
            BinOpKind::And => Ok((format!("({} && {})", lc, rc), Type::Bool)),
            BinOpKind::Or => Ok((format!("({} || {})", lc, rc), Type::Bool)),
        }
    }

    fn gen_unary(
        &mut self,
        op: &UnaryOpKind,
        operand: &Expr,
    ) -> Result<(String, Type), FitzError> {
        let (code, ty) = self.gen_expr(operand)?;
        match op {
            UnaryOpKind::Neg => Ok((format!("(-{})", code), ty)),
        }
    }

    fn gen_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        // Method call: el callee es `Expr::Field { object, field }`.
        // Despachamos por `(tipo del receptor, nombre del método)`
        // como hace el evaluator. Hoy solo cubrimos métodos built-in
        // sobre Str; List/Map y métodos custom sobre `type` quedan
        // como deuda (llegan en 5b.3 y post-3.2 respectivamente).
        if let Expr::Field { object, field } = callee {
            return self.gen_method_call(object, field, args);
        }
        let Expr::Ident(name) = callee else {
            return Err(self.err(
                "llamadas con callee complejo (FnExpr inline u otro Expr): no soportadas",
            ));
        };
        if name == "print" {
            return Err(self.err(
                "`print(...)` solo puede usarse como sentencia, no como expresión en 5b.1",
            ));
        }
        // Builtin global `len(x)`: despacha por tipo del argumento a la
        // misma implementación que el método (`.len()`). Cubre Str, List
        // y Map. Si el usuario tiene una fn `len` definida (raro pero
        // válido), su sig prevalece — chequeamos `fn_sigs` antes del
        // builtin.
        if name == "len" && !self.fn_sigs.contains_key(name) && args.len() == 1 {
            let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
            return match arg_ty {
                Type::Str => Ok((
                    format!("(({}).chars().count() as i64)", arg_code),
                    Type::Int,
                )),
                Type::List(_) | Type::Map(_, _) => Ok((
                    format!("(({}).borrow().len() as i64)", arg_code),
                    Type::Int,
                )),
                other => Err(self.err(format!(
                    "`len(...)`: no aplica a `{}` — solo Str, List<T> y Map<K, V>",
                    display_type(&other, self.env)
                ))),
            };
        }
        let sig = self
            .fn_sigs
            .get(name)
            .cloned()
            .ok_or_else(|| self.err(format!("función `{}` desconocida en codegen", name)))?;
        if args.len() != sig.params.len() {
            return Err(self.err(format!(
                "`{}` espera {} argumento(s), recibió {}",
                name,
                sig.params.len(),
                args.len()
            )));
        }
        let mut arg_codes = Vec::with_capacity(args.len());
        for (a, expected) in args.iter().zip(sig.params.iter()) {
            let (code, ty) = self.gen_expr(a)?;
            arg_codes.push(coerce(&code, &ty, expected));
        }
        Ok((format!("{}({})", name, arg_codes.join(", ")), sig.ret.clone()))
    }

    fn gen_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        match (&obj_ty, method) {
            // ---- Str ----
            (Type::Str, "len") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("(({}).chars().count() as i64)", obj_code), Type::Int))
            }
            (Type::Str, "upper") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).to_uppercase()", obj_code), Type::Str))
            }
            (Type::Str, "lower") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("({}).to_lowercase()", obj_code), Type::Str))
            }
            (Type::Str, other) => Err(self.err(format!(
                "Str no tiene el método `{}` en el subset compilado (hoy: len/upper/lower)",
                other
            ))),

            // ---- List ----
            (Type::List(t), "push") => self.gen_list_push(&obj_code, t, args),
            (Type::List(t), "pop") => self.gen_list_pop(&obj_code, t, args),
            (Type::List(_), "len") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("(({}).borrow().len() as i64)", obj_code), Type::Int))
            }
            (Type::List(t), "map") => self.gen_list_map(&obj_code, t, args),
            (Type::List(t), "filter") => self.gen_list_filter(&obj_code, t, args),
            (Type::List(_), "find") => Err(self.err(
                "`.find()` sobre List<T> devuelve `Result<T>`; el subset compilado no soporta \
                 `Result` todavía — llega en 5b.4. Mientras tanto, podés iterar con `for` y \
                 acumular tu propia condición, o usar `fitz run`.",
            )),
            (Type::List(_), other) => Err(self.err(format!(
                "List no tiene el método `{}` en el subset compilado (hoy: push/pop/len/map/filter)",
                other
            ))),

            // ---- Map ----
            (Type::Map(k, _), "has") => self.gen_map_has(&obj_code, k, args),
            (Type::Map(k, _), "keys") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "Rc::new(RefCell::new(({}).borrow().iter().map(|(__k, _)| __k.clone()).collect::<Vec<_>>()))",
                    obj_code
                );
                Ok((code, Type::List(Box::new((**k).clone()))))
            }
            (Type::Map(_, v), "values") => {
                check_method_arity(method, args, 0)?;
                let code = format!(
                    "Rc::new(RefCell::new(({}).borrow().iter().map(|(_, __v)| __v.clone()).collect::<Vec<_>>()))",
                    obj_code
                );
                Ok((code, Type::List(Box::new((**v).clone()))))
            }
            (Type::Map(_, _), "len") => {
                check_method_arity(method, args, 0)?;
                Ok((format!("(({}).borrow().len() as i64)", obj_code), Type::Int))
            }
            (Type::Map(_, _), "get") => Err(self.err(
                "`.get()` sobre Map<K, V> devuelve `Result<V>`; el subset compilado no soporta \
                 `Result` todavía — llega en 5b.4. Mientras tanto, podés usar `m.has(k)` + `m[k]`, \
                 o `fitz run`.",
            )),
            (Type::Map(_, _), other) => Err(self.err(format!(
                "Map no tiene el método `{}` en el subset compilado (hoy: has/keys/values/len)",
                other
            ))),

            // ---- Tipos custom ----
            (Type::Nominal(_), m) => Err(self.err(format!(
                "métodos custom sobre `type` (`.{}`): primero hay que cerrar la deuda de 3.2 en el parser",
                m
            ))),

            // ---- Otros ----
            (other, m) => Err(self.err(format!(
                "method call `.{}` sobre `{}`: no soportado en codegen",
                m,
                display_type(other, self.env)
            ))),
        }
    }

    // --- métodos List ----------------------------------------------------

    /// `xs.push(x)` → `({xs}).borrow_mut().push({coerce x → T})`. Devuelve
    /// `()` (Null en Fitz). El stmt-mode agrega el `;` final por encima.
    fn gen_list_push(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("push", args, 1)?;
        let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
        let coerced = coerce(&arg_code, &arg_ty, elem_ty);
        let code = format!("({}).borrow_mut().push({})", obj_code, coerced);
        Ok((code, Type::Null))
    }

    /// `xs.pop()` → `({xs}).borrow_mut().pop().expect(...)`. El intérprete
    /// tira error de runtime sobre lista vacía con ese mensaje; el binario
    /// generado paniquea — comportamiento esencial (abortar con mensaje)
    /// equivalente.
    fn gen_list_pop(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("pop", args, 0)?;
        let code = format!(
            "({}).borrow_mut().pop().expect(\"`.pop()` sobre lista vacía\")",
            obj_code
        );
        Ok((code, elem_ty.clone()))
    }

    /// `xs.map(callback)` → snapshot del Vec + map + collect, envuelto en
    /// `Rc::new(RefCell::new(...))`. El callback debe ser un FnExpr
    /// inline; no admitimos referencias a fns nombradas hoy (eso necesita
    /// higher-order, deuda explícita).
    fn gen_list_map(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("map", args, 1)?;
        let (callback_code, ret_ty) =
            self.gen_callback_inline(&args[0], elem_ty, None, "map")?;
        let code = format!(
            "{{ \
                let __items: Vec<_> = ({}).borrow().clone(); \
                Rc::new(RefCell::new(__items.into_iter().map({}).collect::<Vec<_>>())) \
            }}",
            obj_code, callback_code
        );
        Ok((code, Type::List(Box::new(ret_ty))))
    }

    /// `xs.filter(callback)` → snapshot + for-loop manual + push. Evitamos
    /// `.filter(...).collect()` porque el `filter` de Iterator pasa `&T`
    /// y el callback de Fitz toma T por valor. El loop manual clona el
    /// item para pasárselo al callback (para Nominal/List/Map es clone
    /// del Rc → barato) y mueve el original al output si el predicado
    /// retorna true.
    fn gen_list_filter(
        &mut self,
        obj_code: &str,
        elem_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("filter", args, 1)?;
        let (callback_code, _) =
            self.gen_callback_inline(&args[0], elem_ty, Some(&Type::Bool), "filter")?;
        let code = format!(
            "{{ \
                let __items: Vec<_> = ({}).borrow().clone(); \
                let __cb = {}; \
                let mut __out: Vec<_> = Vec::new(); \
                for __it in __items.into_iter() {{ \
                    if __cb(__it.clone()) {{ __out.push(__it); }} \
                }} \
                Rc::new(RefCell::new(__out)) \
            }}",
            obj_code, callback_code
        );
        Ok((code, Type::List(Box::new(elem_ty.clone()))))
    }

    // --- métodos Map -----------------------------------------------------

    /// `m.has(k)` → búsqueda lineal por igualdad → bool.
    fn gen_map_has(
        &mut self,
        obj_code: &str,
        key_ty: &Type,
        args: &[Expr],
    ) -> Result<(String, Type), FitzError> {
        check_method_arity("has", args, 1)?;
        let (arg_code, arg_ty) = self.gen_expr(&args[0])?;
        let coerced = coerce(&arg_code, &arg_ty, key_ty);
        let code = format!(
            "{{ let __k = {}; ({}).borrow().iter().any(|(__k2, _)| __k2 == &__k) }}",
            coerced, obj_code
        );
        Ok((code, Type::Bool))
    }

    // --- helpers para callback inline ------------------------------------

    /// Genera el código Rust de un closure inline a partir de un `FnExpr`.
    /// `param_ty` es el tipo que el receptor (List<T>) impone al param;
    /// el FnExpr puede traer anotación propia, pero la del receptor
    /// manda (el checker ya validó compatibilidad).
    ///
    /// `expected_ret_ty = Some(t)` fuerza el tipo de retorno (caso
    /// `filter` que exige `Bool`). `None` infiere desde el primer
    /// `return` del body, o el último `Stmt::Expr` no-print, o `Null`.
    /// La heurística cubre arrow form (`fn(x) => e`) y bodies simples
    /// con un solo return — los casos exóticos (returns adentro de cada
    /// rama de un `if`) caen a `Null` por simplicidad y requieren
    /// reescribir el callback en arrow form si el tipo importa.
    ///
    /// Devuelve `(código del closure, tipo de retorno inferido/forzado)`.
    fn gen_callback_inline(
        &mut self,
        arg: &Expr,
        param_ty: &Type,
        expected_ret_ty: Option<&Type>,
        method: &str,
    ) -> Result<(String, Type), FitzError> {
        let (params, body) = match arg {
            Expr::FnExpr { params, body } => (params, body),
            _ => {
                return Err(self.err(format!(
                    "`.{}(...)` exige un callback inline `fn(x) => ...` o `fn(x) {{ ... }}`. \
                     Pasar una fn nombrada como callback (higher-order) llega en un sub-paso \
                     posterior de 5b.",
                    method
                )));
            }
        };
        if params.len() != 1 {
            return Err(self.err(format!(
                "el callback de `.{}` toma 1 parámetro, recibió {}",
                method,
                params.len()
            )));
        }
        let param_name = params[0].name.clone();

        // Inferimos el ret type en dry-run sobre el primer Stmt::Return
        // del body, o el último Stmt::Expr no-print, o Null.
        let inferred_ret = self.infer_callback_ret_silently(body, &param_name, param_ty)?;
        let ret_ty = expected_ret_ty.cloned().unwrap_or_else(|| inferred_ret.clone());

        let param_ty_rs = rust_type_for(param_ty, self.env)?;
        let ret_ty_rs = rust_type_for(&ret_ty, self.env)?;

        // Emit el body en un buffer aparte, con el param ligado.
        self.push_scope();
        self.declare_var(param_name.clone(), param_ty.clone());
        let saved = std::mem::take(&mut self.output);
        let saved_indent = self.indent;
        self.indent = 0;
        let mut body_str = String::new();
        for s in body {
            self.gen_stmt_in_fn(s, &ret_ty)?;
            body_str.push_str(&std::mem::take(&mut self.output));
        }
        self.output = saved;
        self.indent = saved_indent;
        self.pop_scope();

        let code = format!(
            "|{}: {}| -> {} {{ {} }}",
            param_name, param_ty_rs, ret_ty_rs, body_str
        );
        Ok((code, ret_ty))
    }

    /// Dry-run para sintetizar el tipo de retorno de un callback. Pushea
    /// el scope del param, recorre el body buscando el primer
    /// `Stmt::Return(e)` (o el último `Stmt::Expr(e)` no-print), llama
    /// a `gen_expr` con `self.output` redirigido a un buffer descartable
    /// (no contamina la salida real).
    fn infer_callback_ret_silently(
        &mut self,
        body: &[Stmt],
        param_name: &str,
        param_ty: &Type,
    ) -> Result<Type, FitzError> {
        let target: Option<&Expr> = body
            .iter()
            .find_map(|s| if let Stmt::Return(e) = s { Some(e) } else { None })
            .or_else(|| {
                body.last().and_then(|s| match s {
                    Stmt::Expr(e) if !is_print_call(e) => Some(e),
                    _ => None,
                })
            });
        let Some(e) = target else { return Ok(Type::Null) };

        self.push_scope();
        self.declare_var(param_name.to_string(), param_ty.clone());
        let saved = std::mem::take(&mut self.output);
        let result = self.gen_expr(e);
        self.output = saved;
        self.pop_scope();
        result.map(|(_, t)| t)
    }

    // --- listas, mapas, indexing ------------------------------------------

    /// `[e1, e2, ...]` → `Rc::new(RefCell::new(vec![v1, v2, ...]))` con
    /// coerción de cada elemento al tipo común. Tipo común sintetizado
    /// como en el checker (5.3.1): primer elemento define el tipo, los
    /// demás deben unificar via `lub` (Int↔Float, T↔Null). Mezcla
    /// irrecuperable o lista vacía sin contexto → error claro.
    fn gen_list_lit(&mut self, items: &[Expr]) -> Result<(String, Type), FitzError> {
        if items.is_empty() {
            // Lista vacía: no podemos sintetizar T. Emitimos un código
            // genérico `Vec::new()` y devolvemos `List<Any>`. El
            // contexto (anotación destino, paso a fn tipada) coerciona
            // a un T concreto; si nadie lo restringe, el rustc generado
            // fallará con "type annotations needed", reflejando que el
            // usuario tiene que anotar.
            return Ok((
                "Rc::new(RefCell::new(Vec::new()))".to_string(),
                Type::List(Box::new(Type::Any)),
            ));
        }
        let mut item_codes_tys: Vec<(String, Type)> = Vec::with_capacity(items.len());
        for it in items {
            let (c, t) = self.gen_expr(it)?;
            item_codes_tys.push((c, t));
        }
        let mut common_ty = item_codes_tys[0].1.clone();
        for (_, t) in &item_codes_tys[1..] {
            common_ty = lub(&common_ty, t).map_err(|_| {
                self.err(format!(
                    "lista con elementos de tipos incompatibles (`{}` y `{}`): el subset compilado \
                     exige una lista homogénea (todos del mismo tipo, con coerciones Int→Float y \
                     T→T? permitidas)",
                    display_type(&common_ty, self.env),
                    display_type(t, self.env),
                ))
            })?;
        }
        if matches!(common_ty, Type::Any) {
            return Err(self.err(
                "lista con elementos cuyo tipo común es `Any`: el subset compilado exige tipo \
                 homogéneo concreto. Anotá el tipo o usá `fitz run` para interpretarlo sin restricción.",
            ));
        }
        let coerced: Vec<String> = item_codes_tys
            .iter()
            .map(|(c, t)| coerce(c, t, &common_ty))
            .collect();
        let code = format!(
            "Rc::new(RefCell::new(vec![{}]))",
            coerced.join(", ")
        );
        Ok((code, Type::List(Box::new(common_ty))))
    }

    /// `{k1: v1, k2: v2, ...}` → `Rc::new(RefCell::new(vec![(k1, v1), ...]))`.
    /// Orden de inserción preservado por Vec. K y V deben ser homogéneos
    /// (mismas reglas que List). Para `m["k"]` (Index) y `m.get(k)` la
    /// búsqueda es lineal O(n), pero matchea exactamente lo que hace
    /// el intérprete.
    fn gen_map_lit(&mut self, pairs: &[(Expr, Expr)]) -> Result<(String, Type), FitzError> {
        if pairs.is_empty() {
            return Ok((
                "Rc::new(RefCell::new(Vec::new()))".to_string(),
                Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
            ));
        }
        let mut entries: Vec<((String, Type), (String, Type))> = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let kt = self.gen_expr(k)?;
            let vt = self.gen_expr(v)?;
            entries.push((kt, vt));
        }
        let mut common_k = entries[0].0 .1.clone();
        let mut common_v = entries[0].1 .1.clone();
        for ((_, kt), (_, vt)) in &entries[1..] {
            common_k = lub(&common_k, kt).map_err(|_| {
                self.err(format!(
                    "mapa con claves de tipos incompatibles (`{}` y `{}`): el subset compilado \
                     exige claves homogéneas",
                    display_type(&common_k, self.env),
                    display_type(kt, self.env),
                ))
            })?;
            common_v = lub(&common_v, vt).map_err(|_| {
                self.err(format!(
                    "mapa con valores de tipos incompatibles (`{}` y `{}`): el subset compilado \
                     exige valores homogéneos",
                    display_type(&common_v, self.env),
                    display_type(vt, self.env),
                ))
            })?;
        }
        if matches!(common_k, Type::Any) || matches!(common_v, Type::Any) {
            return Err(self.err(
                "mapa con claves o valores cuyo tipo común es `Any`: el subset compilado exige \
                 tipos homogéneos concretos. Anotá el tipo o usá `fitz run` para interpretarlo \
                 sin restricción.",
            ));
        }
        let pieces: Vec<String> = entries
            .iter()
            .map(|((kc, kt), (vc, vt))| {
                format!(
                    "({}, {})",
                    coerce(kc, kt, &common_k),
                    coerce(vc, vt, &common_v)
                )
            })
            .collect();
        let code = format!(
            "Rc::new(RefCell::new(vec![{}]))",
            pieces.join(", ")
        );
        Ok((code, Type::Map(Box::new(common_k), Box::new(common_v))))
    }

    /// `obj[idx]` — dispatch por tipo del receptor.
    ///
    ///   - `List<T>[Int]`   → `({xs}.borrow()[idx as usize].clone())`.
    ///     Index out-of-bounds panicea en Rust (igual que el intérprete
    ///     que tira error de runtime).
    ///   - `Map<K, V>[K]`   → búsqueda lineal por igualdad. Si no hay,
    ///     panic con mensaje al estilo del intérprete.
    ///
    /// El clone del item es del Rc para Nominal/List/Map → barato y
    /// preserva el aliasing con la colección original (mutar via
    /// `xs[0].name = "x"` se ve en xs).
    fn gen_index(
        &mut self,
        object: &Expr,
        index: &Expr,
    ) -> Result<(String, Type), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        let (idx_code, idx_ty) = self.gen_expr(index)?;
        match &obj_ty {
            Type::List(inner) => {
                if !matches!(idx_ty, Type::Int) {
                    return Err(self.err(format!(
                        "indexing de lista con `{}`: el índice debe ser Int",
                        display_type(&idx_ty, self.env)
                    )));
                }
                let code = format!(
                    "({}).borrow()[({}) as usize].clone()",
                    obj_code, idx_code
                );
                Ok((code, (**inner).clone()))
            }
            Type::Map(k_ty, v_ty) => {
                let coerced_idx = coerce(&idx_code, &idx_ty, k_ty);
                // Búsqueda lineal por igualdad. `unwrap_or_else(panic)` con
                // mensaje al estilo del intérprete. Ligamos el Rc a una
                // var local antes de `.borrow()` para extender la vida
                // del temporal — `(m.clone()).borrow()` solo cuando la
                // expresión completa cabe en una stmt simple; acá usamos
                // un `let __m = ...` y necesitamos el holder.
                let code = format!(
                    "{{ \
                        let __map = {}; \
                        let __m = __map.borrow(); \
                        let __k = {}; \
                        __m.iter() \
                            .find(|(__k2, _)| __k2 == &__k) \
                            .map(|(_, __v)| __v.clone()) \
                            .unwrap_or_else(|| panic!(\"clave no encontrada en mapa: {{:?}}\", __k)) \
                    }}",
                    obj_code, coerced_idx
                );
                Ok((code, (**v_ty).clone()))
            }
            other => Err(self.err(format!(
                "indexing `[]` sobre `{}`: solo soportado en List<T> y Map<K, V>",
                display_type(other, self.env)
            ))),
        }
    }

    fn gen_struct_lit(
        &mut self,
        type_name: &str,
        provided: &[(String, Expr)],
    ) -> Result<(String, Type), FitzError> {
        let sig = self
            .type_sigs
            .get(type_name)
            .cloned()
            .ok_or_else(|| self.err(format!("tipo `{}` desconocido en codegen", type_name)))?;

        // Validamos campos extra. El checker debería haberlo cazado;
        // este chequeo es defensa en profundidad.
        for (provided_name, _) in provided {
            if !sig.fields.iter().any(|f| &f.name == provided_name) {
                return Err(self.err(format!(
                    "el tipo `{}` no tiene un campo llamado `{}`",
                    type_name, provided_name
                )));
            }
        }

        // Construimos los pares (campo, código Rust) en orden de
        // declaración del `type`. Esto importa para Display y para
        // futuras igualdades.
        let mut field_codes: Vec<String> = Vec::with_capacity(sig.fields.len());
        for f in &sig.fields {
            let supplied = provided.iter().find(|(n, _)| n == &f.name);
            let value_code = if let Some((_, expr)) = supplied {
                let (code, ty) = self.gen_expr(expr)?;
                coerce(&code, &ty, &f.type_)
            } else if let Some(default_expr) = &f.default {
                let (code, ty) = self.gen_expr(default_expr)?;
                coerce(&code, &ty, &f.type_)
            } else if matches!(f.type_, Type::Nullable(_)) {
                "None".to_string()
            } else {
                return Err(self.err(format!(
                    "falta el campo `{}` al instanciar `{}` (no tiene default y no es nullable)",
                    f.name, type_name
                )));
            };
            field_codes.push(format!("{}: {}", f.name, value_code));
        }

        let data_name = format!("{}Data", type_name);
        let code = format!(
            "Rc::new(RefCell::new({} {{ {} }}))",
            data_name,
            field_codes.join(", ")
        );
        let nominal_id = sig.id;
        Ok((code, Type::Nominal(nominal_id)))
    }

    fn gen_field_access(
        &mut self,
        object: &Expr,
        field: &str,
    ) -> Result<(String, Type), FitzError> {
        let (obj_code, obj_ty) = self.gen_expr(object)?;
        let Type::Nominal(id) = &obj_ty else {
            return Err(self.err(format!(
                "field access `.{}` sobre `{}`: solo se soporta sobre instancias de tipos custom",
                field,
                type_name(&obj_ty)
            )));
        };
        let info = self.env.info(*id);
        let declared = info.fields.clone().ok_or_else(|| {
            self.err(format!(
                "tipo `{}` con campos sin resolver — no se puede generar acceso",
                info.name
            ))
        })?;
        let Some(f) = declared.iter().find(|f| f.name == field) else {
            return Err(self.err(format!(
                "el tipo `{}` no tiene un campo llamado `{}`",
                info.name, field
            )));
        };
        // `code.borrow().field` es válido cuando el accesor consume
        // el valor en una expresión que se evalúa inmediatamente.
        // Como devolvemos una expresión Rust que puede entrar en
        // arbitrary contextos, agregamos `.clone()` cuando el tipo lo
        // requiere (Str, Nominal, Option de cualquier cosa). Para
        // tipos `Copy` (Int/Float/Bool/Null), el borrow basta.
        let access = if needs_clone(&f.type_) {
            format!("({}).borrow().{}.clone()", obj_code, field)
        } else {
            format!("({}).borrow().{}", obj_code, field)
        };
        Ok((access, f.type_.clone()))
    }

    fn gen_if_expr(
        &mut self,
        condition: &Expr,
        then: &[Stmt],
        else_: Option<&[Stmt]>,
    ) -> Result<(String, Type), FitzError> {
        let (cond_code, _) = self.gen_expr(condition)?;

        // Si ambas ramas (else incluido) terminan en un `Stmt::Expr`
        // que sea expresable como valor, el `if` es expresión con
        // valor. Si no, lo tratamos como statement con valor `()`
        // (`Type::Null`) y emitimos cada bloque entero como stmts.
        let (then_stmts, then_tail) = split_tail_expr(then);
        let (else_stmts_opt, else_tail) = match else_ {
            Some(body) => {
                let (s, t) = split_tail_expr(body);
                (Some(s), t)
            }
            None => (None, None),
        };

        let want_value = else_stmts_opt.is_some()
            && then_tail.is_some()
            && else_tail.is_some();

        if want_value {
            // Modo expresión: evaluamos los tails y unificamos.
            let (then_block, then_tail_code, then_tail_ty) = {
                self.push_scope();
                let stmts = self.gen_block_to_string(&then_stmts)?;
                let (c, t) = self.gen_expr(then_tail.unwrap())?;
                self.pop_scope();
                (stmts, c, t)
            };
            let (else_block, else_tail_code, else_tail_ty) = {
                self.push_scope();
                let stmts = self.gen_block_to_string(&else_stmts_opt.clone().unwrap())?;
                let (c, t) = self.gen_expr(else_tail.unwrap())?;
                self.pop_scope();
                (stmts, c, t)
            };
            let result_ty = lub(&then_tail_ty, &else_tail_ty).map_err(|_| {
                self.err(format!(
                    "ramas de `if` con tipos incompatibles: `{}` y `{}`",
                    type_name(&then_tail_ty),
                    type_name(&else_tail_ty)
                ))
            })?;
            let then_tail_coerced = coerce(&then_tail_code, &then_tail_ty, &result_ty);
            let else_tail_coerced = coerce(&else_tail_code, &else_tail_ty, &result_ty);
            let code = format!(
                "(if {} {{\n{}{}{}\n{}}} else {{\n{}{}{}\n{}}})",
                cond_code,
                then_block,
                self.indent_str(),
                then_tail_coerced,
                self.indent_str_outer(),
                else_block,
                self.indent_str(),
                else_tail_coerced,
                self.indent_str_outer(),
            );
            Ok((code, result_ty))
        } else {
            // Modo statement: re-emitimos los tails como stmts
            // (`gen_stmt` se encarga del `;` y la indentación).
            let then_block = {
                self.push_scope();
                let mut full = self.gen_block_to_string(&then_stmts)?;
                if let Some(e) = then_tail {
                    full.push_str(&self.gen_stmt_to_string(&Stmt::Expr(e.clone()))?);
                }
                self.pop_scope();
                full
            };
            let mut code = format!("if {} {{\n{}{}}}", cond_code, then_block, self.indent_str_outer());
            if let Some(else_stmts) = else_stmts_opt {
                let else_block = {
                    self.push_scope();
                    let mut full = self.gen_block_to_string(&else_stmts)?;
                    if let Some(e) = else_tail {
                        full.push_str(&self.gen_stmt_to_string(&Stmt::Expr(e.clone()))?);
                    }
                    self.pop_scope();
                    full
                };
                write!(
                    &mut code,
                    " else {{\n{}{}}}",
                    else_block,
                    self.indent_str_outer()
                )
                .unwrap();
            }
            Ok((code, Type::Null))
        }
    }

    /// Emite los `stmts` redirigiendo `self.output` a un buffer
    /// temporal y devuelve el resultado. Restaura el output original
    /// antes de devolver. La indentación actual se respeta (los
    /// `emit_indent` van con `self.indent + 1` porque entran en un
    /// `if`/`else` body).
    fn gen_block_to_string(&mut self, stmts: &[&Stmt]) -> Result<String, FitzError> {
        let saved = std::mem::take(&mut self.output);
        self.indent += 1;
        for s in stmts {
            self.gen_stmt(s)?;
        }
        self.indent -= 1;
        let out = std::mem::take(&mut self.output);
        self.output = saved;
        Ok(out)
    }

    fn gen_stmt_to_string(&mut self, stmt: &Stmt) -> Result<String, FitzError> {
        let saved = std::mem::take(&mut self.output);
        self.indent += 1;
        self.gen_stmt(stmt)?;
        self.indent -= 1;
        let out = std::mem::take(&mut self.output);
        self.output = saved;
        Ok(out)
    }

    fn indent_str(&self) -> String {
        "    ".repeat(self.indent + 1)
    }

    fn indent_str_outer(&self) -> String {
        "    ".repeat(self.indent)
    }
}

/// Si el último stmt del bloque es un `Stmt::Expr(e)` que se puede
/// usar como valor (no es un `print(...)`, que solo es stmt), lo
/// devolvemos separado del resto. Caso contrario, el tail va `None`
/// y el bloque queda completo.
fn split_tail_expr(body: &[Stmt]) -> (Vec<&Stmt>, Option<&Expr>) {
    if let Some(Stmt::Expr(e)) = body.last() {
        if !is_print_call(e) {
            let stmts: Vec<&Stmt> = body[..body.len() - 1].iter().collect();
            return (stmts, Some(e));
        }
    }
    (body.iter().collect(), None)
}

fn is_print_call(e: &Expr) -> bool {
    matches!(e, Expr::Call { callee, .. }
        if matches!(callee.as_ref(), Expr::Ident(n) if n == "print"))
}

/// "Least upper bound" pragmático sobre dos tipos resueltos. Mismo
/// criterio que `types.rs` para FnExpr (5.3.5) y para if-as-expression
/// (5b.2), acotado al subset compilable hoy. Usado además para unificar
/// elementos de listas/mapas literales (5b.3).
///
/// Reglas:
///   - `a == b`               → `a`
///   - `Int` ↔ `Float`        → `Float`
///   - `Null` ↔ `T`           → `T?` (T ≠ Null)
///   - `T?` ↔ `T`             → `T?`
///   - mismo `List<a>`/`List<b>` con `lub(a,b)` recursivo → `List<lub>`
///     (idem `Map`, `Nullable`)
///   - resto                  → `Err(())`
fn lub(a: &Type, b: &Type) -> Result<Type, ()> {
    if a == b {
        return Ok(a.clone());
    }
    match (a, b) {
        (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
        (Type::Null, other) | (other, Type::Null) if !matches!(other, Type::Null) => {
            Ok(Type::Nullable(Box::new(other.clone())))
        }
        (Type::Nullable(inner), other) | (other, Type::Nullable(inner))
            if **inner == *other =>
        {
            Ok(Type::Nullable(inner.clone()))
        }
        (Type::Nullable(a_in), Type::Nullable(b_in)) => {
            lub(a_in, b_in).map(|t| Type::Nullable(Box::new(t)))
        }
        (Type::List(a_in), Type::List(b_in)) => {
            lub(a_in, b_in).map(|t| Type::List(Box::new(t)))
        }
        (Type::Map(ak, av), Type::Map(bk, bv)) => {
            let k = lub(ak, bk)?;
            let v = lub(av, bv)?;
            Ok(Type::Map(Box::new(k), Box::new(v)))
        }
        _ => Err(()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rust_type_for(t: &Type, env: &TypeEnv) -> Result<String, FitzError> {
    match t {
        Type::Int => Ok("i64".to_string()),
        Type::Float => Ok("f64".to_string()),
        Type::Str => Ok("String".to_string()),
        Type::Bool => Ok("bool".to_string()),
        Type::Null => Ok("()".to_string()),
        Type::Nominal(id) => Ok(env.info(*id).name.clone()),
        Type::Nullable(inner) => Ok(format!("Option<{}>", rust_type_for(inner, env)?)),
        // List<T> y Map<K, V> se modelan con `Rc<RefCell<>>` para
        // preservar la semántica de referencia compartida del intérprete
        // (push/pop/asignación de elementos visibles vía cualquier alias).
        // T = Any (literal mixto sin contexto) → error explícito; el
        // subset compilable exige tipo homogéneo concreto.
        Type::List(inner) => {
            if matches!(**inner, Type::Any) {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    "listas con elementos de tipos mixtos (`List<Any>`): el subset compilado \
                     necesita tipo homogéneo concreto. Anotá el tipo o usá `fitz run` para \
                     interpretarlo sin restricción."
                        .to_string(),
                ));
            }
            Ok(format!("Rc<RefCell<Vec<{}>>>", rust_type_for(inner, env)?))
        }
        Type::Map(k, v) => {
            if matches!(**k, Type::Any) || matches!(**v, Type::Any) {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    "mapas con claves o valores de tipos mixtos (`Map<Any, ...>` o \
                     `Map<..., Any>`): el subset compilado necesita tipos homogéneos \
                     concretos. Anotá el tipo o usá `fitz run` para interpretarlo \
                     sin restricción."
                        .to_string(),
                ));
            }
            Ok(format!(
                "Rc<RefCell<Vec<({}, {})>>>",
                rust_type_for(k, env)?,
                rust_type_for(v, env)?
            ))
        }
        other => Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "codegen 5b no soporta el tipo `{}` (primitivos + tipos custom + nullables + List<T> + Map<K, V>)",
                type_name(other)
            ),
        )),
    }
}

fn type_name(t: &Type) -> &'static str {
    match t {
        Type::Int => "Int",
        Type::Float => "Float",
        Type::Str => "Str",
        Type::Bool => "Bool",
        Type::Null => "Null",
        Type::Range => "Range",
        Type::Any => "Any",
        Type::List(_) => "List<...>",
        Type::Map(_, _) => "Map<...>",
        Type::Result(_) => "Result<...>",
        Type::Nullable(_) => "T?",
        Type::Nominal(_) => "<nominal>",
        Type::Function { .. } => "fn(...)",
    }
}

/// Versión "linda" del tipo para mensajes de error, con T concreto
/// (recursa en generics, resuelve nominales). `List<User>` en vez de
/// `List<...>`. Usar `type_name` solo cuando el detalle no importa.
fn display_type(t: &Type, env: &TypeEnv) -> String {
    match t {
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::Str => "Str".into(),
        Type::Bool => "Bool".into(),
        Type::Null => "Null".into(),
        Type::Range => "Range".into(),
        Type::Any => "Any".into(),
        Type::List(inner) => format!("List<{}>", display_type(inner, env)),
        Type::Map(k, v) => format!("Map<{}, {}>", display_type(k, env), display_type(v, env)),
        Type::Result(inner) => format!("Result<{}>", display_type(inner, env)),
        Type::Nullable(inner) => format!("{}?", display_type(inner, env)),
        Type::Nominal(id) => env.info(*id).name.clone(),
        Type::Function { params, ret } => {
            let ps = params
                .iter()
                .map(|p| display_type(p, env))
                .collect::<Vec<_>>()
                .join(", ");
            format!("fn({}) -> {}", ps, display_type(ret, env))
        }
    }
}

/// `true` si el tipo subyacente NO es `Copy` en el Rust generado y por
/// ende necesita `.clone()` cuando se evalúa un `Ident`/`Field` que se
/// va a consumir en otro contexto.
///
/// Para List/Map el clone es del `Rc` envolvente — barato y, lo más
/// importante, **preserva el aliasing**: dos vars que se construyeron
/// a partir de la misma lista comparten contenido y mutaciones vía
/// `push`/asignación se ven en ambas. Mismo criterio que para Nominal.
fn needs_clone(t: &Type) -> bool {
    match t {
        Type::Int | Type::Float | Type::Bool | Type::Null => false,
        Type::Str | Type::Nominal(_) => true,
        // `Option<T>` no es Copy salvo casos extremos; clonamos siempre.
        Type::Nullable(_) => true,
        // `Rc<RefCell<Vec<...>>>` — clone del Rc, barato, alias preservado.
        Type::List(_) | Type::Map(_, _) => true,
        // Fallback conservador: clonamos.
        _ => true,
    }
}

/// Coerciona una expresión Rust (`code`) de tipo Fitz `from` al tipo
/// Fitz `to`. Devuelve la expresión Rust resultante. Si no aplica
/// ninguna coerción, devuelve `code` tal cual.
///
/// Coerciones soportadas:
///   - `Int → Float`           → `(x as f64)`
///   - `T   → T?`               → `Some(x)` (con eventual clone de T)
///   - `Null → T?`              → `None`
fn coerce(code: &str, from: &Type, to: &Type) -> String {
    match (from, to) {
        (Type::Int, Type::Float) => format!("({} as f64)", code),
        (Type::Null, Type::Nullable(_)) => "None".to_string(),
        (from, Type::Nullable(inner)) if !matches!(from, Type::Nullable(_)) => {
            let coerced = coerce(code, from, inner);
            format!("Some({})", coerced)
        }
        _ => code.to_string(),
    }
}

fn numeric_coerce(
    lc: &str,
    lt: &Type,
    rc: &str,
    rt: &Type,
) -> Option<(String, String, Type)> {
    match (lt, rt) {
        (Type::Int, Type::Int) => Some((lc.into(), rc.into(), Type::Int)),
        (Type::Float, Type::Float) => Some((lc.into(), rc.into(), Type::Float)),
        (Type::Int, Type::Float) => Some((format!("({} as f64)", lc), rc.into(), Type::Float)),
        (Type::Float, Type::Int) => Some((lc.into(), format!("({} as f64)", rc), Type::Float)),
        _ => None,
    }
}

/// Devuelve una **expresión Rust** que evalúa a `String` y representa
/// el valor `code` (de tipo Fitz `ty`) en formato `print` top-level:
/// strings sin comillas, null como `"null"`, floats con `.0` si tienen
/// fracción 0, instancias delegando a su Display, Option como `"null"`
/// cuando None.
fn show_expr(code: &str, ty: &Type) -> String {
    match ty {
        Type::Int | Type::Bool => format!("format!(\"{{}}\", {})", code),
        Type::Float => format!("__fitz_fmt_float({})", code),
        Type::Str => format!("({}).clone()", code),
        Type::Null => "String::from(\"null\")".to_string(),
        Type::Nominal(_) => format!("format!(\"{{}}\", &*({}).borrow())", code),
        Type::Nullable(inner) => {
            // Capturamos el valor por referencia para no consumirlo.
            // Para `Option<T>`, el match bindea `Some(__v)` y delega a
            // `show_expr` con código `__v` y tipo `*inner`. El `Option`
            // queda intacto.
            let inner_show = show_expr("__v", inner);
            format!(
                "(match &({}) {{ Some(__v) => {}, None => String::from(\"null\") }})",
                code, inner_show
            )
        }
        // List/Map en print top-level usan el formato "inline" (strings
        // con comillas adentro de los items, igual que `write_inline_value`
        // del intérprete). Construimos el string en runtime concatenando
        // sub-shows item por item. Ligamos primero el `Rc` a una `let`
        // antes de hacer `.borrow()` para extender la vida del temporal
        // — `(xs.clone()).borrow()` cae con la expresión.
        Type::List(inner) => {
            // Iteramos con `.cloned()` para que `__it` sea por valor
            // (no `&T`) — uniforma el código de `show_expr_inline` con
            // el de `show_expr` general (que asume valor). El clone es
            // barato para `Rc<RefCell<...>>` (Nominal/List/Map) y vivible
            // para `String` en contexto de print.
            let item_show = show_expr_inline("__it", inner);
            format!(
                "{{ \
                    let __list = {}; \
                    let __items = __list.borrow(); \
                    let mut __s = String::from(\"[\"); \
                    for (__i, __it) in __items.iter().cloned().enumerate() {{ \
                        if __i > 0 {{ __s.push_str(\", \"); }} \
                        __s.push_str(&({})); \
                    }} \
                    __s.push(']'); \
                    __s \
                }}",
                code, item_show
            )
        }
        Type::Map(kt, vt) => {
            let k_show = show_expr_inline("__k", kt);
            let v_show = show_expr_inline("__v", vt);
            format!(
                "{{ \
                    let __map = {}; \
                    let __pairs = __map.borrow(); \
                    let mut __s = String::from(\"{{\"); \
                    for (__i, (__k, __v)) in __pairs.iter().cloned().enumerate() {{ \
                        if __i > 0 {{ __s.push_str(\", \"); }} \
                        __s.push_str(&({})); \
                        __s.push_str(\": \"); \
                        __s.push_str(&({})); \
                    }} \
                    __s.push('}}'); \
                    __s \
                }}",
                code, k_show, v_show
            )
        }
        // Range, Any, Function, Result — fallback. Si el AST cuela algo
        // que llega acá, el error principal viene de otro lado.
        _ => format!("format!(\"{{:?}}\", {})", code),
    }
}

/// Versión "inline" de `show_expr` para items adentro de colecciones:
/// strings van **entre comillas** (igual a `write_inline_value` del
/// intérprete). Llama a `show_expr` para todo lo demás.
fn show_expr_inline(code: &str, ty: &Type) -> String {
    match ty {
        Type::Str => format!("format!(\"\\\"{{}}\\\"\", {})", code),
        _ => show_expr(code, ty),
    }
}

/// Devuelve **una o más sentencias Rust** que escriben `code` (de tipo
/// Fitz `ty`) en el `Formatter` `__f`, en formato "inline" (el que se
/// usa adentro de `Display for FooData`): strings ENTRE COMILLAS,
/// instancias por Display, Option como `"null"` cuando None. Igual a
/// `write_inline_value` del intérprete.
fn inline_display_stmt(code: &str, ty: &Type) -> String {
    match ty {
        Type::Int | Type::Bool => format!("        write!(__f, \"{{}}\", {})?;\n", code),
        Type::Float => format!("        write!(__f, \"{{}}\", __fitz_fmt_float({}))?;\n", code),
        // Para Str adentro de Instance, mostramos con comillas dobles
        // alrededor (igual que el `write_inline_value` del intérprete).
        Type::Str => format!("        write!(__f, \"\\\"{{}}\\\"\", {})?;\n", code),
        Type::Null => "        write!(__f, \"null\")?;\n".to_string(),
        Type::Nominal(_) => format!(
            "        {{ let __t = ({}).borrow(); write!(__f, \"{{}}\", &*__t)?; }}\n",
            code
        ),
        Type::Nullable(inner) => {
            // Borroweamos el `Option<T>` y matcheamos por referencia.
            // Para Nominal adentro de Some, el match bindea `__v` como
            // `&Rc<RefCell<T>>`, así que necesitamos `(*__v)` o pasar
            // un sub-código. Para tipos primitivos, `&T` también
            // funciona porque Display está implementado para &T.
            let inner_body = match inner.as_ref() {
                Type::Int | Type::Bool => "                write!(__f, \"{}\", __v)?;\n".to_string(),
                Type::Float => "                write!(__f, \"{}\", __fitz_fmt_float(*__v))?;\n".to_string(),
                Type::Str => "                write!(__f, \"\\\"{}\\\"\", __v)?;\n".to_string(),
                Type::Null => "                write!(__f, \"null\")?;\n".to_string(),
                Type::Nominal(_) => {
                    "                { let __t = (*__v).borrow(); write!(__f, \"{}\", &*__t)?; }\n"
                        .to_string()
                }
                _ => "                write!(__f, \"{:?}\", __v)?;\n".to_string(),
            };
            format!(
                "        match &({}) {{\n            Some(__v) => {{\n{}            }}\n            None => write!(__f, \"null\")?,\n        }}\n",
                code, inner_body
            )
        }
        _ => format!("        write!(__f, \"{{:?}}\", {})?;\n", code),
    }
}

fn check_method_arity(method: &str, args: &[Expr], expected: usize) -> Result<(), FitzError> {
    if args.len() != expected {
        return Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "el método `{}` toma {} argumento(s), recibió {}",
                method,
                expected,
                args.len()
            ),
        ));
    }
    Ok(())
}

fn rust_str_literal(s: &str) -> String {
    // Genera un literal Rust válido escapando comillas y barras.
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::types::check_program;

    fn gen(src: &str) -> Result<String, FitzError> {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (env, errors) = check_program(&program);
        if !errors.is_empty() {
            panic!("checker errors: {:?}", errors);
        }
        generate_rust(&program, &env)
    }

    fn assert_contains(src: &str, fragments: &[&str]) {
        let code = gen(src).unwrap_or_else(|e| panic!("codegen falló: {}", e));
        for f in fragments {
            assert!(
                code.contains(f),
                "esperaba `{}` en la salida, no estaba.\nSalida:\n{}",
                f,
                code
            );
        }
    }

    fn assert_err_contains(src: &str, needles: &[&str]) {
        let err = gen(src).expect_err("esperaba error de codegen");
        for n in needles {
            assert!(
                err.message.contains(n),
                "esperaba `{}` en el error, fue: {}",
                n,
                err.message
            );
        }
    }

    #[test]
    fn programa_vacio_genera_main_vacio() {
        let code = gen("").unwrap();
        assert!(code.contains("fn main()"));
    }

    #[test]
    fn let_int_anotado_genera_i64() {
        assert_contains(
            "let x: Int = 42",
            &["let mut x: i64 = 42i64;"],
        );
    }

    #[test]
    fn let_int_inferido_genera_i64() {
        assert_contains("let x = 42", &["let mut x: i64 = 42i64;"]);
    }

    #[test]
    fn let_float_anotado_genera_f64_con_coercion_int() {
        assert_contains(
            "let pi: Float = 3",
            &["let mut pi: f64 = (3i64 as f64);"],
        );
    }

    #[test]
    fn let_str_genera_string() {
        assert_contains(
            "let name = \"Fitz\"",
            &["let mut name: String = String::from(\"Fitz\");"],
        );
    }

    #[test]
    fn binop_int_int_es_int() {
        assert_contains(
            "let x = 1 + 2",
            &["(1i64 + 2i64)"],
        );
    }

    #[test]
    fn binop_int_float_coerciona_a_float() {
        assert_contains(
            "let x = 1 + 2.0",
            &["((1i64 as f64) + 2f64)"],
        );
    }

    #[test]
    fn str_interp_genera_format_macro() {
        // Para una var Int adentro de StrInterp, generamos `format!`
        // pasando la var directo (no necesita `.clone()`).
        assert_contains(
            "let n = 5\nlet s = \"x es {n}\"",
            &["format!(\"x es {}\", n)"],
        );
    }

    #[test]
    fn str_interp_con_var_str_clona() {
        // Para Str, generamos `.clone()` porque format! borrowea
        // pero seguimos pasando el `Ident` evaluado, que sí incluye
        // el clone.
        assert_contains(
            "let name = \"Fitz\"\nlet s = \"hola, {name}\"",
            &["format!(\"hola, {}\", name.clone())"],
        );
    }

    #[test]
    fn print_genera_println_macro() {
        assert_contains(
            "let x: Int = 1\nprint(x)",
            &["println!(\"{}\", x)"],
        );
    }

    #[test]
    fn print_multiples_args_genera_format_string_con_espacios() {
        assert_contains(
            "let a: Int = 1\nlet b: Int = 2\nprint(a, b)",
            &["println!(\"{} {}\", a, b)"],
        );
    }

    #[test]
    fn print_sin_args_genera_println_vacio() {
        assert_contains("print()", &["println!()"]);
    }

    #[test]
    fn fn_top_level_emite_signature_completa() {
        assert_contains(
            "fn double(n: Int) -> Int { return n * 2 }",
            &["fn double(mut n: i64) -> i64", "return (n * 2i64);"],
        );
    }

    #[test]
    fn fn_arrow_emite_return_implicito() {
        assert_contains(
            "fn double(n: Int) -> Int => n * 2",
            &["fn double(mut n: i64) -> i64", "return (n * 2i64);"],
        );
    }

    #[test]
    fn llamada_a_fn_top_level_resuelve_return_type() {
        let code = gen(
            "fn double(n: Int) -> Int => n * 2\n\
             let x = double(5)",
        )
        .unwrap();
        // x debe quedar como i64 (return de double).
        assert!(code.contains("let mut x: i64 = double(5i64);"), "got:\n{}", code);
    }

    #[test]
    fn if_else_genera_estructura_rust() {
        assert_contains(
            "let x = 1\nif (x > 0) { print(\"pos\") } else { print(\"neg\") }",
            &["if (x > 0i64) {", "} else {"],
        );
    }

    #[test]
    fn while_genera_estructura_rust() {
        assert_contains(
            "let n = 0\nwhile (n < 3) { n = n + 1 }",
            &["while (n < 3i64) {", "n = (n + 1i64);"],
        );
    }

    #[test]
    fn for_in_range_genera_rust() {
        assert_contains(
            "for i in 0..3 { print(i) }",
            &["for mut i in (0i64 as i64)..(3i64 as i64) {"],
        );
    }

    #[test]
    fn reasignacion_usa_igual_no_let() {
        let code = gen("let x = 1\nx = 2").unwrap();
        assert!(code.contains("let mut x: i64 = 1i64;"));
        // La segunda asignación no es `let`.
        assert!(code.contains("\n    x = 2i64;"), "got:\n{}", code);
    }

    #[test]
    fn neg_genera_unary_rust() {
        assert_contains("let x = -5", &["(-5i64)"]);
    }

    #[test]
    fn bool_y_logicos_generan_bool_rust() {
        assert_contains(
            "let b = true and false",
            &["let mut b: bool = (true && false);"],
        );
    }

    #[test]
    fn comparacion_str_usa_as_str() {
        assert_contains(
            "let a = \"hola\"\nlet b = a < \"mundo\"",
            &[".as_str() < "],
        );
    }

    // ---- features fuera de scope generan errores claros ----

    // ---- 5b.2: tipos custom (sí soportados, salvo igualdad) ----

    #[test]
    fn type_def_emite_struct_y_alias_rc_refcell() {
        assert_contains(
            "type User { id: Int, name: Str }",
            &[
                "struct UserData {",
                "    id: i64,",
                "    name: String,",
                "type User = Rc<RefCell<UserData>>;",
            ],
        );
    }

    #[test]
    fn type_def_emite_impl_display_canonico() {
        let code = gen("type User { id: Int, name: Str }").unwrap();
        assert!(
            code.contains("impl std::fmt::Display for UserData"),
            "falta impl Display, got:\n{}",
            code
        );
        // El Display escribe `User { id: <int>, name: "<str>" }` —
        // strings con comillas adentro de la instancia (igual al
        // intérprete).
        assert!(code.contains("\"User {{\""), "falta el header del Display");
        assert!(code.contains("\"\\\"{}\\\"\""), "falta el patrón con comillas para Str");
    }

    #[test]
    fn struct_lit_emite_rc_new_refcell_new() {
        assert_contains(
            "type User { id: Int, name: Str }\nlet u = User { id: 1, name: \"x\" }",
            &["Rc::new(RefCell::new(UserData { id: 1i64, name: String::from(\"x\") }))"],
        );
    }

    #[test]
    fn struct_lit_aplica_default_inline_si_falta_campo() {
        // `active: Bool = true` debe inyectarse cuando no se pasa.
        let code = gen(
            "type C { port: Int, active: Bool = true }\nlet c = C { port: 8080 }",
        )
        .unwrap();
        assert!(
            code.contains("active: true"),
            "esperaba que el default `true` esté inyectado, got:\n{}",
            code
        );
    }

    #[test]
    fn struct_lit_nullable_omitido_se_resuelve_como_none() {
        let code = gen(
            "type U { id: Int, email: Str? }\nlet u = U { id: 1 }",
        )
        .unwrap();
        assert!(
            code.contains("email: None"),
            "esperaba `email: None`, got:\n{}",
            code
        );
    }

    #[test]
    fn struct_lit_valor_str_a_campo_nullable_se_envuelve_en_some() {
        let code = gen(
            "type U { id: Int, email: Str? }\nlet u = U { id: 1, email: \"a@b\" }",
        )
        .unwrap();
        assert!(
            code.contains("email: Some(String::from(\"a@b\"))"),
            "esperaba `Some(String::from(...))`, got:\n{}",
            code
        );
    }

    #[test]
    fn struct_lit_null_literal_a_campo_nullable_es_none() {
        let code = gen(
            "type U { id: Int, email: Str? }\nlet u = U { id: 1, email: null }",
        )
        .unwrap();
        assert!(
            code.contains("email: None"),
            "esperaba `email: None`, got:\n{}",
            code
        );
    }

    #[test]
    fn field_access_int_emite_borrow_sin_clone() {
        let code = gen(
            "type U { id: Int }\nlet u = U { id: 1 }\nlet n = u.id",
        )
        .unwrap();
        assert!(
            code.contains(".borrow().id;") || code.contains(".borrow().id\n"),
            "esperaba acceso `borrow().id` sin clone, got:\n{}",
            code
        );
    }

    #[test]
    fn field_access_str_emite_borrow_clone() {
        let code = gen(
            "type U { name: Str }\nlet u = U { name: \"x\" }\nlet s = u.name",
        )
        .unwrap();
        assert!(
            code.contains(".borrow().name.clone()"),
            "esperaba `.borrow().name.clone()`, got:\n{}",
            code
        );
    }

    #[test]
    fn field_assign_emite_borrow_mut() {
        let code = gen(
            "type U { name: Str }\nlet u = U { name: \"x\" }\nu.name = \"y\"",
        )
        .unwrap();
        assert!(
            code.contains(".borrow_mut().name = String::from(\"y\");"),
            "esperaba `.borrow_mut().name = ...`, got:\n{}",
            code
        );
    }

    #[test]
    fn pasar_instance_a_fn_clona_el_rc() {
        // El Ident `u` de tipo Nominal se evalúa como `u.clone()` al
        // pasarlo a `f(u)`. Esto preserva el aliasing del intérprete.
        let code = gen(
            "type U { id: Int }\nfn f(x: U) -> Int => x.id\nlet u = U { id: 1 }\nlet n = f(u)",
        )
        .unwrap();
        assert!(
            code.contains("f(u.clone())"),
            "esperaba `f(u.clone())`, got:\n{}",
            code
        );
    }

    #[test]
    fn print_de_instance_usa_show_expr_con_display() {
        // `print(u)` para u: U → format!("{}", &*u.borrow()) dentro
        // del println!.
        let code = gen(
            "type U { id: Int }\nlet u = U { id: 1 }\nprint(u)",
        )
        .unwrap();
        assert!(
            code.contains("format!(\"{}\", &*"),
            "esperaba `format!(\"{{}}\", &*(...).borrow())`, got:\n{}",
            code
        );
        assert!(
            code.contains(".borrow())"),
            "esperaba `.borrow())` en el print, got:\n{}",
            code
        );
    }

    #[test]
    fn tipo_anidado_compila_con_nullable_de_nominal() {
        // `type Order { user: User? }` se traduce a un campo de tipo
        // `Option<User>` (= `Option<Rc<RefCell<UserData>>>`).
        let code = gen(
            "type User { name: Str }\ntype Order { user: User? }",
        )
        .unwrap();
        assert!(
            code.contains("user: Option<User>"),
            "esperaba `user: Option<User>` en OrderData, got:\n{}",
            code
        );
    }

    #[test]
    fn igualdad_estructural_entre_instancias_emite_borrow_eq() {
        let code = gen(
            "type U { id: Int }\nlet a = U { id: 1 }\nlet b = U { id: 1 }\nlet eq = a == b",
        )
        .unwrap();
        assert!(
            code.contains(").borrow() == *(") || code.contains(".borrow() == *"),
            "esperaba comparación con `*x.borrow() == *y.borrow()`, got:\n{}",
            code
        );
    }

    // ---- 5b.2+: if como expresión con valor ----

    #[test]
    fn if_como_expresion_emite_branches_sin_punto_y_coma() {
        let code = gen("let x = if (true) { 1 } else { 2 }").unwrap();
        // El bloque del if tiene su última expresión sin `;` para que
        // el `if` evalúe a un valor (`1` o `2`).
        assert!(
            code.contains("(if true {") || code.contains("(if (true)"),
            "esperaba un if-expression envuelto en paréntesis, got:\n{}",
            code
        );
        assert!(
            code.contains("1i64\n") && code.contains("2i64\n"),
            "esperaba `1i64` y `2i64` como tail sin `;`, got:\n{}",
            code
        );
        // x debe quedar como i64.
        assert!(
            code.contains("let mut x: i64 = "),
            "esperaba `let mut x: i64 = ...`, got:\n{}",
            code
        );
    }

    #[test]
    fn if_expresion_unifica_int_float_a_float() {
        let code = gen("let x = if (true) { 1 } else { 2.5 }").unwrap();
        assert!(
            code.contains("let mut x: f64 = "),
            "esperaba `x: f64`, got:\n{}",
            code
        );
        // La rama Int se coerciona explícitamente: `(1i64 as f64)`.
        assert!(
            code.contains("(1i64 as f64)"),
            "esperaba coerción Int→Float en la rama then, got:\n{}",
            code
        );
    }

    #[test]
    fn if_como_sentencia_mantiene_comportamiento_anterior() {
        // Sin asignar y con `print` adentro: el if sigue siendo
        // statement; print no se trata como tail expression
        // (no es una expresión con valor en Fitz).
        let code = gen("if (true) { print(\"a\") } else { print(\"b\") }").unwrap();
        // Cada print queda emitido con `;` final (terminator de stmt).
        assert!(
            code.contains("println!(\"{}\", String::from(\"a\"));"),
            "esperaba print como stmt con `;`, got:\n{}",
            code
        );
        assert!(
            code.contains("println!(\"{}\", String::from(\"b\"));"),
            "esperaba print como stmt con `;` en else, got:\n{}",
            code
        );
    }

    #[test]
    fn if_sin_else_no_se_trata_como_expresion() {
        // Sin else, no hay segunda rama → no es expresión con valor.
        // El último stmt del then se emite como statement común.
        let code = gen("if (true) { 1 }").unwrap();
        assert!(
            code.contains("1i64;"),
            "esperaba `1i64;` como stmt (no como tail), got:\n{}",
            code
        );
    }

    // ---- 5b.2+: métodos built-in sobre Str ----

    #[test]
    fn str_len_emite_chars_count_as_i64() {
        let code = gen("let s = \"hola\"\nlet n = s.len()").unwrap();
        assert!(
            code.contains(".chars().count() as i64"),
            "esperaba `.chars().count() as i64`, got:\n{}",
            code
        );
        assert!(
            code.contains("let mut n: i64 = "),
            "esperaba que n quede como i64, got:\n{}",
            code
        );
    }

    #[test]
    fn str_upper_emite_to_uppercase() {
        let code = gen("let s = \"hola\"\nlet u = s.upper()").unwrap();
        assert!(
            code.contains(".to_uppercase()"),
            "esperaba `.to_uppercase()`, got:\n{}",
            code
        );
    }

    #[test]
    fn str_lower_emite_to_lowercase() {
        let code = gen("let s = \"HOLA\"\nlet l = s.lower()").unwrap();
        assert!(
            code.contains(".to_lowercase()"),
            "esperaba `.to_lowercase()`, got:\n{}",
            code
        );
    }

    // (Métodos desconocidos sobre Str los ataja el checker antes de
    // llegar al codegen, así que no testeamos ese path desde acá.)

    #[test]
    fn type_def_emite_derive_partialeq() {
        let code = gen("type U { id: Int }").unwrap();
        assert!(
            code.contains("#[derive(Clone, PartialEq)]"),
            "esperaba derive(PartialEq) sobre el data struct, got:\n{}",
            code
        );
    }

    // ---- 5b.3: listas, mapas, indexing, métodos built-in ----

    #[test]
    fn list_literal_emite_rc_refcell_vec() {
        // `[1, 2, 3]` se modela como `Rc<RefCell<Vec<i64>>>`. Los items
        // se coercen al tipo común (acá Int → i64) y se construye con
        // el macro vec![].
        assert_contains(
            "let xs: List<Int> = [1, 2, 3]",
            &[
                "let mut xs: Rc<RefCell<Vec<i64>>>",
                "Rc::new(RefCell::new(vec![1i64, 2i64, 3i64]))",
            ],
        );
    }

    #[test]
    fn list_literal_homogeneo_int_float_promueve_a_float() {
        // Int+Float en la misma lista → `List<Float>` (mismo lub que
        // if-expression y FnExpr ret).
        let code = gen("let xs = [1, 2.5, 3]").unwrap();
        assert!(
            code.contains("Rc<RefCell<Vec<f64>>>"),
            "esperaba List<f64>, got:\n{}",
            code
        );
        assert!(
            code.contains("(1i64 as f64)") && code.contains("(3i64 as f64)"),
            "esperaba coerción Int→Float en los items, got:\n{}",
            code
        );
    }

    #[test]
    fn list_literal_vacia_es_list_any_a_resolver_por_contexto() {
        // `[]` sin contexto da `List<Any>`. Con anotación, el contexto
        // restringe a List<T> y el `Vec::new()` infiere desde el target.
        let code = gen("let xs: List<Int> = []").unwrap();
        assert!(
            code.contains("let mut xs: Rc<RefCell<Vec<i64>>>"),
            "esperaba `List<Int>` por anotación, got:\n{}",
            code
        );
        assert!(
            code.contains("Rc::new(RefCell::new(Vec::new()))"),
            "esperaba `Rc::new(RefCell::new(Vec::new()))` para lista vacía, got:\n{}",
            code
        );
    }

    #[test]
    fn list_literal_heterogeneo_es_error_homogeneo_requerido() {
        // Sin posibilidad de unificar (Int + Str), el codegen aborta
        // con mensaje claro mencionando la heterogeneidad.
        assert_err_contains(
            "let xs = [1, \"dos\"]",
            &["homogénea"],
        );
    }

    #[test]
    fn map_literal_emite_vec_pares() {
        assert_contains(
            "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}",
            &[
                "let mut m: Rc<RefCell<Vec<(String, i64)>>>",
                "(String::from(\"a\"), 1i64)",
                "(String::from(\"b\"), 2i64)",
            ],
        );
    }

    #[test]
    fn map_literal_vacio_resuelto_por_anotacion() {
        let code = gen("let m: Map<Str, Int> = {}").unwrap();
        assert!(
            code.contains("let mut m: Rc<RefCell<Vec<(String, i64)>>>"),
            "esperaba `Map<Str, Int>` por anotación, got:\n{}",
            code
        );
    }

    #[test]
    fn map_literal_valores_heterogeneos_es_error() {
        assert_err_contains(
            "let m = {\"a\": 1, \"b\": \"x\"}",
            &["homogéneos"],
        );
    }

    #[test]
    fn list_indexing_emite_borrow_clone() {
        // `xs[0]` → `(xs.clone()).borrow()[(0i64) as usize].clone()`.
        // El `.clone()` final es del Rc para Nominal/List/Map o copy
        // para primitivos — siempre seguro.
        let code = gen("let xs: List<Int> = [10, 20]\nlet x = xs[0]").unwrap();
        assert!(
            code.contains(".borrow()[(0i64) as usize].clone()"),
            "esperaba acceso por borrow + index + clone, got:\n{}",
            code
        );
        assert!(
            code.contains("let mut x: i64 ="),
            "esperaba que x quede tipado como i64, got:\n{}",
            code
        );
    }

    #[test]
    fn map_indexing_emite_busqueda_lineal_con_panic() {
        // `m["a"]` → bloque que linea la búsqueda y paniquea si falta.
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet n = m[\"a\"]",
        )
        .unwrap();
        assert!(
            code.contains(".find(|(__k2, _)| __k2 == &__k)"),
            "esperaba búsqueda lineal en map, got:\n{}",
            code
        );
        assert!(
            code.contains("clave no encontrada en mapa"),
            "esperaba mensaje de panic con texto del intérprete, got:\n{}",
            code
        );
    }

    #[test]
    fn for_sobre_list_genera_snapshot_iter() {
        // `for v in xs` → snapshot via `borrow().clone().into_iter()`
        // (evita re-entrancia si el body muta `xs`).
        let code = gen(
            "let xs: List<Int> = [1, 2, 3]\nfor v in xs { print(v) }",
        )
        .unwrap();
        assert!(
            code.contains(".borrow().clone().into_iter()"),
            "esperaba snapshot iter, got:\n{}",
            code
        );
    }

    #[test]
    fn for_sobre_list_de_any_es_error() {
        assert_err_contains(
            "let xs = []\nfor v in xs { print(v) }",
            &["List<Any>"],
        );
    }

    #[test]
    fn list_push_emite_borrow_mut_push() {
        assert_contains(
            "let xs: List<Int> = []\nxs.push(7)",
            &["(xs.clone()).borrow_mut().push(7i64);"],
        );
    }

    #[test]
    fn list_pop_emite_borrow_mut_pop_con_expect() {
        let code = gen("let xs: List<Int> = [1]\nlet x = xs.pop()").unwrap();
        assert!(
            code.contains(".borrow_mut().pop().expect(\"`.pop()` sobre lista vacía\")"),
            "esperaba `.pop().expect(...)`, got:\n{}",
            code
        );
    }

    #[test]
    fn list_len_metodo_emite_borrow_len_as_i64() {
        let code = gen("let xs: List<Int> = []\nlet n = xs.len()").unwrap();
        assert!(
            code.contains(".borrow().len() as i64"),
            "esperaba `.borrow().len() as i64`, got:\n{}",
            code
        );
    }

    #[test]
    fn len_builtin_global_sobre_list_resuelve_a_borrow_len() {
        // `len(xs)` despacha por tipo del argumento — mismo código que
        // `xs.len()` para List/Map; para Str sigue siendo chars().count.
        let code = gen("let xs: List<Int> = [1]\nlet n = len(xs)").unwrap();
        assert!(
            code.contains(".borrow().len() as i64"),
            "esperaba `.borrow().len() as i64` desde el builtin global, got:\n{}",
            code
        );
    }

    #[test]
    fn len_builtin_global_sobre_str_usa_chars_count() {
        let code = gen("let s = \"hola\"\nlet n = len(s)").unwrap();
        assert!(
            code.contains(".chars().count() as i64"),
            "esperaba `.chars().count() as i64`, got:\n{}",
            code
        );
    }

    #[test]
    fn list_map_con_fnexpr_inline_emite_closure() {
        let code = gen(
            "let xs: List<Int> = [1, 2, 3]\nlet ys = xs.map(fn(x) => x * 2)",
        )
        .unwrap();
        assert!(
            code.contains(".into_iter().map(|x: i64| -> i64"),
            "esperaba closure inline `|x: i64| -> i64`, got:\n{}",
            code
        );
        assert!(
            code.contains("Rc::new(RefCell::new"),
            "esperaba envoltorio Rc::new(RefCell::new(...)), got:\n{}",
            code
        );
        assert!(
            code.contains("let mut ys: Rc<RefCell<Vec<i64>>>"),
            "esperaba que `ys` quede tipado `List<Int>`, got:\n{}",
            code
        );
    }

    #[test]
    fn list_filter_con_fnexpr_inline_emite_for_manual() {
        // Filter usa un for manual (no .filter()) porque el callback
        // toma T por valor pero `Iterator::filter` quiere &T.
        let code = gen(
            "let xs: List<Int> = [1, 2, 3]\nlet ys = xs.filter(fn(x) => x > 1)",
        )
        .unwrap();
        assert!(
            code.contains("let __cb = |x: i64| -> bool"),
            "esperaba binding del callback como `|x: i64| -> bool`, got:\n{}",
            code
        );
        assert!(
            code.contains("if __cb(__it.clone())"),
            "esperaba aplicación del cb con clone, got:\n{}",
            code
        );
    }

    #[test]
    fn map_method_chaining_funciona() {
        // `xs.map(f).map(g)` debe poder componerse. El test es de
        // estructura: el tipo de salida del primer map alimenta al
        // siguiente sin friction.
        let code = gen(
            "let xs: List<Int> = [1, 2]\n\
             let ys = xs.map(fn(x) => x * 2).map(fn(x) => x + 1)",
        )
        .unwrap();
        assert!(
            code.matches(".into_iter().map(|x: i64| -> i64").count() >= 2,
            "esperaba dos map closures encadenados, got:\n{}",
            code
        );
    }

    #[test]
    fn map_has_emite_iter_any() {
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet b = m.has(\"a\")",
        )
        .unwrap();
        assert!(
            code.contains(".iter().any(|(__k2, _)| __k2 == &__k)"),
            "esperaba `.iter().any(...)`, got:\n{}",
            code
        );
    }

    #[test]
    fn map_keys_emite_lista_nueva_de_claves() {
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}\nlet ks = m.keys()",
        )
        .unwrap();
        assert!(
            code.contains(".iter().map(|(__k, _)| __k.clone()).collect::<Vec<_>>()"),
            "esperaba pipeline de keys, got:\n{}",
            code
        );
        assert!(
            code.contains("let mut ks: Rc<RefCell<Vec<String>>>"),
            "esperaba que keys retorne List<Str>, got:\n{}",
            code
        );
    }

    #[test]
    fn map_values_emite_lista_nueva_de_valores() {
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet vs = m.values()",
        )
        .unwrap();
        assert!(
            code.contains(".iter().map(|(_, __v)| __v.clone()).collect::<Vec<_>>()"),
            "esperaba pipeline de values, got:\n{}",
            code
        );
        assert!(
            code.contains("let mut vs: Rc<RefCell<Vec<i64>>>"),
            "esperaba que values retorne List<Int>, got:\n{}",
            code
        );
    }

    #[test]
    fn map_len_metodo_emite_borrow_len_as_i64() {
        let code = gen(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet n = m.len()",
        )
        .unwrap();
        assert!(
            code.contains(".borrow().len() as i64"),
            "esperaba `.borrow().len() as i64`, got:\n{}",
            code
        );
    }

    #[test]
    fn list_find_difere_a_5b4() {
        // find devuelve Result<T>; Result en codegen llega en 5b.4.
        assert_err_contains(
            "let xs: List<Int> = [1, 2]\nlet x = xs.find(fn(n) => n > 0)",
            &["5b.4"],
        );
    }

    #[test]
    fn map_get_difere_a_5b4() {
        assert_err_contains(
            "let m: Map<Str, Int> = {\"a\": 1}\nlet v = m.get(\"a\")",
            &["5b.4"],
        );
    }

    #[test]
    fn fnexpr_suelta_da_error_claro() {
        // FnExpr como valor (no como callback inline) no se soporta.
        assert_err_contains(
            "let f = fn(x: Int) => x * 2",
            &["higher-order", "callback inline"],
        );
    }

    #[test]
    fn print_de_lista_emite_iter_inline() {
        // El print/interp construye el string `[a, b, c]` en runtime
        // ligando primero el Rc a una var (vida del temporal).
        let code = gen("let xs: List<Int> = [1, 2]\nprint(xs)").unwrap();
        assert!(
            code.contains("let __list = "),
            "esperaba binding del Rc antes del borrow, got:\n{}",
            code
        );
        assert!(
            code.contains("String::from(\"[\")"),
            "esperaba header `[` para lista, got:\n{}",
            code
        );
    }

    #[test]
    fn print_de_mapa_emite_iter_inline_con_llaves() {
        let code = gen("let m: Map<Str, Int> = {\"a\": 1}\nprint(m)").unwrap();
        assert!(
            code.contains("let __map = "),
            "esperaba binding del Rc antes del borrow, got:\n{}",
            code
        );
        assert!(
            code.contains("String::from(\"{\")"),
            "esperaba header `{{` para mapa, got:\n{}",
            code
        );
    }

    #[test]
    fn match_no_soportado() {
        assert_err_contains(
            "let v = 1\nlet s = match v { 0 => \"cero\", _ => \"otro\" }",
            &["match", "5b.4"],
        );
    }

    #[test]
    fn imports_no_soportados() {
        assert_err_contains(
            "from foo import bar\nprint(bar)",
            &["import", "5b.5"],
        );
    }

    #[test]
    fn http_decoradores_no_soportados() {
        assert_err_contains(
            "@get(\"/\") fn index() => 0",
            &["decorador", "5b.6"],
        );
    }

    #[test]
    fn fn_sin_anotacion_de_param_es_error() {
        assert_err_contains(
            "fn double(n) -> Int { return n * 2 }",
            &["parámetro", "anotación"],
        );
    }
}
