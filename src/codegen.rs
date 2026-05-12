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
        // 5b.1: solo `for v in start..end` (Range). El iter tiene
        // que ser un Expr::Range; otros iterables llegan en 5b.3.
        let Expr::Range { start, end } = iter else {
            return Err(self.err(
                "`for` sobre listas y otros iterables: no soportado en 5b.1 — llega en 5b.3",
            ));
        };
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
                "`Range` solo se acepta como iterable de `for` en 5b.1; otros usos no se generan",
            )),
            Expr::List(_) => Err(self.err(
                "listas literales `[...]`: no soportadas en 5b.1 — llegan en 5b.3",
            )),
            Expr::Map(_) => Err(self.err(
                "mapas literales `{...}`: no soportados en 5b.1 — llegan en 5b.3",
            )),
            Expr::Index { .. } => Err(self.err(
                "indexing `[]`: requiere listas/mapas — 5b.3",
            )),
            Expr::Field { object, field } => self.gen_field_access(object, field),
            Expr::StructLit { type_name, fields } => self.gen_struct_lit(type_name, fields),
            Expr::Ok(_) | Expr::Err(_) | Expr::Try(_) => Err(self.err(
                "Result / `Ok` / `Err` / `?`: no soportados en 5b.1 — llegan en 5b.4",
            )),
            Expr::Match { .. } => Err(self.err(
                "`match`: requiere Result/tipos custom — 5b.4",
            )),
            Expr::FnExpr { .. } => Err(self.err(
                "funciones anónimas `fn(...) => ...`: no soportadas en 5b.1",
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
                "llamadas con callee complejo (FnExpr inline): no soportadas en 5b",
            ));
        };
        if name == "print" {
            return Err(self.err(
                "`print(...)` solo puede usarse como sentencia, no como expresión en 5b.1",
            ));
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
            (Type::List(_) | Type::Map(_, _), m) => Err(self.err(format!(
                "métodos sobre List/Map (`.{}`): llegan en 5b.3",
                m
            ))),
            (Type::Nominal(_), m) => Err(self.err(format!(
                "métodos custom sobre `type` (`.{}`): primero hay que cerrar la deuda de 3.2 en el parser",
                m
            ))),
            (other, m) => Err(self.err(format!(
                "method call `.{}` sobre `{}`: no soportado en codegen",
                m,
                type_name(other)
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
            let result_ty = lub_for_if(&then_tail_ty, &else_tail_ty).map_err(|_| {
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

/// "Least upper bound" pragmático sobre dos tipos resueltos para
/// unificar las ramas de un `if`. Mismo criterio que `types.rs`
/// para FnExpr (5.3.5) pero acotado al subset compilable hoy.
fn lub_for_if(a: &Type, b: &Type) -> Result<Type, ()> {
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
        other => Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "codegen 5b no soporta el tipo `{}` (primitivos + tipos custom + nullables)",
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

/// `true` si el tipo subyacente NO es `Copy` en el Rust generado y por
/// ende necesita `.clone()` cuando se evalúa un `Ident`/`Field` que se
/// va a consumir en otro contexto.
fn needs_clone(t: &Type) -> bool {
    match t {
        Type::Int | Type::Float | Type::Bool | Type::Null => false,
        Type::Str | Type::Nominal(_) => true,
        // `Option<T>` no es Copy salvo casos extremos; clonamos siempre.
        Type::Nullable(_) => true,
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
        // Cualquier otro tipo (List, Map, Result, Range, Any, Function)
        // hoy no llega acá en codegen — quien lo intente recibe el
        // error general de tipo no soportado. Damos un fallback debug
        // solo para no romper si el AST cuela algo.
        _ => format!("format!(\"{{:?}}\", {})", code),
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

    #[test]
    fn listas_no_soportadas() {
        assert_err_contains("let xs = [1, 2, 3]", &["listas", "5b.3"]);
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
