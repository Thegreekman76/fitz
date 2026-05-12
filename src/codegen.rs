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
    AssignTarget, BinOpKind, Expr, Param, Program, Stmt, StrPart, TypeExpr, UnaryOpKind,
};
use crate::error::{ErrorKind, FitzError};
use crate::types::{resolve_type_expr, Type, TypeEnv};

/// Genera código Rust válido a partir de un programa Fitz tipado.
/// El programa debe haber pasado por `check_program` antes (las
/// anotaciones de tipo deben estar resueltas y consistentes).
///
/// Errores acá son de **codegen**: features fuera de scope para
/// 5b.1 (tipos compuestos, FnExpr, fns sin return type que
/// retornan, etc.). No revalidamos lo que el checker ya hizo.
pub fn generate_rust(program: &Program, env: &TypeEnv) -> Result<String, FitzError> {
    let mut ctx = CodegenCtx::new(env);
    ctx.pre_register_fns(program)?;

    // Separamos fns top-level del resto: las fns van afuera de
    // `fn main()`. Los lets, prints, ifs y demás top-level entran
    // adentro del main en orden de aparición.
    let (top_fns, main_stmts): (Vec<&Stmt>, Vec<&Stmt>) = program
        .iter()
        .partition(|s| matches!(s, Stmt::FnDef { .. }));

    ctx.emit_prelude();
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
}

#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<Type>,
    ret: Type,
}

impl<'a> CodegenCtx<'a> {
    fn new(env: &'a TypeEnv) -> Self {
        Self {
            env,
            output: String::new(),
            indent: 0,
            scopes: vec![HashMap::new()],
            fn_sigs: HashMap::new(),
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
        self.emit("// Código generado por Fitz 5b.1 — no editar a mano.\n");
        self.emit("#![allow(unused_mut, unused_variables, unused_assignments)]\n\n");
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
            self.emit(&rust_type_for(pty)?);
        }
        self.emit(")");
        if !matches!(sig.ret, Type::Null) {
            self.emit(" -> ");
            self.emit(&rust_type_for(&sig.ret)?);
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
                "`type {}`: tipos custom no soportados en 5b.1 — llegan en 5b.2",
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
        let AssignTarget::Ident(name) = target else {
            return Err(self.err(
                "asignación a campo (`obj.field = ...`): requiere tipos custom — 5b.2+",
            ));
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
            self.emit(&rust_type_for(&declared_ty)?);
            self.emit(" = ");
            self.emit(&final_rhs);
            self.emit(";\n");
            self.declare_var(name.clone(), declared_ty);
        }
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
                // Para Str, generamos `.clone()` porque las funciones
                // consumen String. Es ineficiente pero correcto.
                let code = if matches!(ty, Type::Str) {
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
            Expr::Field { .. } => Err(self.err(
                "field access `.campo`: requiere tipos custom — 5b.2",
            )),
            Expr::StructLit { type_name, .. } => Err(self.err(format!(
                "struct literal `{} {{ ... }}`: requiere tipos custom — 5b.2",
                type_name
            ))),
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
        let mut pieces = Vec::with_capacity(args.len());
        for a in args {
            let (code, _ty) = self.gen_expr(a)?;
            pieces.push(code);
        }
        // `print(a, b, c)` → `println!("{} {} {}", a, b, c)`.
        let format_str: String = std::iter::repeat("{}")
            .take(args.len())
            .collect::<Vec<_>>()
            .join(" ");
        self.emit(&format!("println!(\"{}\", {})", format_str, pieces.join(", ")));
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
                    let (code, _) = self.gen_expr(e)?;
                    args.push(code);
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
        let Expr::Ident(name) = callee else {
            return Err(self.err(
                "llamadas con callee complejo (FnExpr inline, method calls): no soportadas en 5b.1",
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

    fn gen_if_expr(
        &mut self,
        condition: &Expr,
        then: &[Stmt],
        else_: Option<&[Stmt]>,
    ) -> Result<(String, Type), FitzError> {
        // 5b.1 soporta `if` como sentencia (que es el caso típico).
        // `if` como expresión con valor (devolviendo el último expr
        // de cada bloque) no se modela; el AST de `Expr::If` lo
        // permite pero el codegen lo trataría como statement con
        // valor `()`. Suficiente para 5b.1.
        let (cond_code, _) = self.gen_expr(condition)?;
        let mut s = String::new();
        write!(&mut s, "if {} {{ ", cond_code).unwrap();
        // Emitimos los stmts del bloque en string aparte para no
        // mezclar con el output principal. Simplificación: para 5b.1
        // los if-statements no producen valor (los usamos via gen_stmt).
        self.push_scope();
        let mut block_out = String::new();
        // Emit indirecto: redirigimos a un buffer local.
        let saved_output = std::mem::take(&mut self.output);
        for stmt in then {
            self.gen_stmt(stmt)?;
        }
        block_out.push_str(&self.output);
        self.output = saved_output;
        self.pop_scope();
        s.push_str(&block_out);
        s.push('}');
        if let Some(else_body) = else_ {
            self.push_scope();
            let saved_output = std::mem::take(&mut self.output);
            for stmt in else_body {
                self.gen_stmt(stmt)?;
            }
            let else_out = std::mem::take(&mut self.output);
            self.output = saved_output;
            self.pop_scope();
            write!(&mut s, " else {{ {} }}", else_out).unwrap();
        }
        Ok((s, Type::Null))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rust_type_for(t: &Type) -> Result<String, FitzError> {
    match t {
        Type::Int => Ok("i64".to_string()),
        Type::Float => Ok("f64".to_string()),
        Type::Str => Ok("String".to_string()),
        Type::Bool => Ok("bool".to_string()),
        Type::Null => Ok("()".to_string()),
        other => Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "codegen 5b.1 no soporta el tipo `{}` (solo primitivos: Int/Float/Str/Bool/Null)",
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

fn coerce(code: &str, from: &Type, to: &Type) -> String {
    match (from, to) {
        (Type::Int, Type::Float) => format!("({} as f64)", code),
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

    #[test]
    fn tipos_custom_no_soportados() {
        assert_err_contains(
            "type User { id: Int }\nlet u = User { id: 1 }",
            &["User", "5b.2"],
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
