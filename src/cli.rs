//! Fase 13 (v0.11.0) — CLI builder nativo.
//!
//! `@command("name", desc="...")` sobre una fn declara un comando CLI.
//! El binario producido por `fitz build` parsea `std::env::args()` y
//! dispatcha al comando matcheante. El intérprete `fitz run` también
//! puede correr CLI programs — útil para development.
//!
//! Convención de params (sin decorators en `Param` — decisión MVP):
//! - Param **sin default** → positional arg requerido (`mybin <name>`).
//! - Param **con default** → flag opcional (`--name <value>` o `--name`
//!   si es Bool sin valor).
//! - Bool con default `false` → flag bool (`--loud`).
//! - Otros tipos con default → flag value (`--count 5`).
//!
//! Help autogenerado: `mybin --help` lista comandos, `mybin <cmd> --help`
//! muestra usage del comando con args + flags + descs.

use crate::ast::Param;
use crate::value::Value;

/// Spec de un comando CLI registrado vía `@command`.
#[derive(Clone)]
pub struct CliCommand {
    /// Nombre del comando (arg 0 de `@command("name")`). Es lo que
    /// el user tipea: `mybin <name>`.
    pub name: String,
    /// Descripción opcional para `--help`. None si no se pasó `desc=`.
    pub desc: Option<String>,
    /// Nombre Fitz de la fn (para mensajes de error). Puede diferir
    /// del nombre del comando.
    pub fn_name: String,
    /// Params del handler. Determinan positional args + flags según
    /// la convención (sin default = positional, con default = flag).
    pub params: Vec<Param>,
    /// Handler como `Value::Function` para invocar al dispatch.
    pub handler: Value,
}

impl std::fmt::Debug for CliCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliCommand")
            .field("name", &self.name)
            .field("desc", &self.desc)
            .field("fn_name", &self.fn_name)
            .field("params", &self.params.len())
            .finish()
    }
}

/// Registry global de comandos CLI. Poblada por el evaluator durante
/// la fase de procesamiento de decorators. El main del binario (o el
/// `fitz run`) consulta el registry post-eval para detectar modo CLI
/// y dispatchear desde `argv`.
#[derive(Debug, Default)]
pub struct CliRegistry {
    commands: parking_lot::Mutex<Vec<CliCommand>>,
}

impl CliRegistry {
    pub fn new() -> Self {
        CliRegistry::default()
    }

    /// Registra un comando. Rechaza nombres duplicados (dos `@command(
    /// "name")` sobre fns distintas — el user típicamente NO quiere eso).
    pub fn register(&self, cmd: CliCommand) -> Result<(), String> {
        let mut cmds = self.commands.lock();
        if cmds.iter().any(|c| c.name == cmd.name) {
            return Err(format!(
                "@command(\"{}\"): nombre del comando duplicado. Cada `@command` debe tener un nombre único.",
                cmd.name
            ));
        }
        cmds.push(cmd);
        Ok(())
    }

    /// Devuelve la cantidad de comandos registrados. Usado por el
    /// main para decidir modo: `count == 0` → no es CLI; `count == 1`
    /// → modo single-command (sin subcomando); `count >= 2` → modo
    /// multi-comando (subcomandos).
    pub fn count(&self) -> usize {
        self.commands.lock().len()
    }

    /// Toma snapshot de los comandos para inspección sin mutex hold.
    pub fn snapshot(&self) -> Vec<CliCommand> {
        self.commands.lock().clone()
    }

    /// Busca un comando por nombre. None si no existe.
    pub fn find(&self, name: &str) -> Option<CliCommand> {
        self.commands
            .lock()
            .iter()
            .find(|c| c.name == name)
            .cloned()
    }
}

/// Clasifica un param como positional arg o flag según la convención:
/// - Sin default → positional arg.
/// - Con default → flag.
pub fn param_is_flag(p: &Param) -> bool {
    p.default.is_some()
}

/// v0.11.1 (Fase 13 polish) — auto-infiere short flags por primera
/// letra del nombre del param. Devuelve `HashMap<char, String>` que
/// mapea `'l' → "loud"`, `'c' → "count"`, etc. Si dos flags empiezan
/// con la misma letra (colisión), devuelve `Err` con mensaje claro:
/// la `@command` debe renombrar uno o aceptar que solo el primero
/// reciba short flag.
///
/// Decisión de diseño: short flags **auto** en vez de `@flag(short="l")`
/// kwargs. Razón: la mayoría de las CLI standards (POSIX, GNU)
/// usan primera letra; el override manual queda como deuda futura
/// (kwargs en @flag requeriría AST change en Param). Si una colisión
/// aparece, el user renombra o desactiva via kwarg futuro.
pub fn compute_short_flags(
    params: &[Param],
) -> Result<std::collections::HashMap<char, String>, String> {
    // v0.11.1 — variadic List<Str> al final NO recibe short flag (no es flag).
    let variadic_idx = params
        .last()
        .filter(|p| {
            matches!(
                p.type_.as_ref().map(|t| t.display_name()),
                Some(n) if n.starts_with("List<")
            )
        })
        .map(|_| params.len() - 1);
    let mut out: std::collections::HashMap<char, String> = std::collections::HashMap::new();
    for (i, p) in params.iter().enumerate() {
        if Some(i) == variadic_idx {
            continue;
        }
        if !param_is_flag(p) {
            continue;
        }
        let first = match p.name.chars().next() {
            Some(c) => c.to_ascii_lowercase(),
            None => continue,
        };
        if !first.is_ascii_alphanumeric() {
            // Flags con nombre que empieza con `_` u otro símbolo:
            // skipear short flag (poco común en CLIs públicas).
            continue;
        }
        if let Some(existing) = out.get(&first) {
            return Err(format!(
                "@command short flag conflict: `--{}` y `--{}` comparten primera letra `-{}`. \
                 Renombrá uno de los dos o desactivá short flags para uno (deuda futura: kwarg \
                 `@flag(short=null)`).",
                existing, p.name, first
            ));
        }
        out.insert(first, p.name.clone());
    }
    Ok(out)
}

/// Construye el usage line "USAGE: bin <command> <positionals> [OPTIONS]"
/// para un comando.
pub fn usage_line(bin_name: &str, cmd: &CliCommand, multi_command: bool) -> String {
    let mut s = String::from("USAGE:\n    ");
    s.push_str(bin_name);
    if multi_command {
        s.push(' ');
        s.push_str(&cmd.name);
    }
    let mut has_flags = false;
    for p in &cmd.params {
        if param_is_flag(p) {
            has_flags = true;
            continue;
        }
        s.push_str(&format!(" <{}>", p.name));
    }
    if has_flags {
        s.push_str(" [OPTIONS]");
    }
    s
}

/// Renderea la sección ARGS (positional args required) del help de un
/// comando. Vacío si el comando no tiene positional args.
pub fn render_args_section(cmd: &CliCommand) -> String {
    let positional: Vec<&Param> = cmd.params.iter().filter(|p| !param_is_flag(p)).collect();
    if positional.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\nARGS:\n");
    for p in positional {
        let ty = p
            .type_
            .as_ref()
            .map(|t| t.display_name())
            .unwrap_or_else(|| "Any".into());
        s.push_str(&format!("    <{}>  ({})\n", p.name, ty));
    }
    s.trim_end().to_string()
}

/// Renderea la sección OPTIONS (flags) del help de un comando. Incluye
/// `-h, --help` siempre. Cada flag muestra short form (`-x`, v0.11.1)
/// si lo tiene auto-asignado + default si está.
pub fn render_options_section(cmd: &CliCommand) -> String {
    let mut s = String::from("\n\nOPTIONS:\n");
    // v0.11.1 — pre-compute short map para anotar cada flag con su `-x`.
    let short_map = compute_short_flags(&cmd.params).unwrap_or_default();
    // Invertir: name → short char (para lookup directo en el loop).
    let mut by_name: std::collections::HashMap<&str, char> = std::collections::HashMap::new();
    for (c, n) in &short_map {
        by_name.insert(n.as_str(), *c);
    }
    for p in &cmd.params {
        if !param_is_flag(p) {
            continue;
        }
        let ty = p
            .type_
            .as_ref()
            .map(|t| t.display_name())
            .unwrap_or_else(|| "Any".into());
        // v0.11.1 — short prefix (`-x, ` o vacío).
        let short_prefix = match by_name.get(p.name.as_str()) {
            Some(c) => format!("-{}, ", c),
            None => String::new(),
        };
        // v0.11.1 — Bool flag con default = true muestra `--no-<name>`
        // como forma de negar (paralelo a Cargo `--no-default-features`).
        // Bool con default = false muestra `--<name>` para activar.
        let flag_form = if ty == "Bool" {
            let default_true = matches!(&p.default, Some(crate::ast::Expr::Bool(true, _)));
            if default_true {
                format!("{}--no-{}", short_prefix, p.name)
            } else {
                format!("{}--{}", short_prefix, p.name)
            }
        } else {
            format!("{}--{} <{}>", short_prefix, p.name, ty.to_uppercase())
        };
        s.push_str(&format!("    {}\n", flag_form));
    }
    s.push_str("    -h, --help");
    s
}

/// Help completo de un comando individual.
pub fn render_command_help(bin_name: &str, cmd: &CliCommand, multi_command: bool) -> String {
    let mut s = String::new();
    if let Some(desc) = &cmd.desc {
        s.push_str(desc);
        s.push_str("\n\n");
    }
    s.push_str(&usage_line(bin_name, cmd, multi_command));
    s.push_str(&render_args_section(cmd));
    s.push_str(&render_options_section(cmd));
    s
}

/// Help global del binario (modo multi-comando). Lista todos los
/// comandos con su descripción corta.
pub fn render_global_help(bin_name: &str, cmds: &[CliCommand]) -> String {
    let mut s = format!(
        "USAGE:\n    {} <command> [ARGS] [OPTIONS]\n\nCOMMANDS:\n",
        bin_name
    );
    let mut max_width = 0;
    for c in cmds {
        if c.name.len() > max_width {
            max_width = c.name.len();
        }
    }
    for c in cmds {
        let desc = c.desc.as_deref().unwrap_or("");
        s.push_str(&format!(
            "    {:width$}    {}\n",
            c.name,
            desc,
            width = max_width
        ));
    }
    s.push_str(&format!(
        "\nRun `{} <command> --help` for more info on a specific command.",
        bin_name
    ));
    s
}

/// Parsea `argv` (sin el `bin_name` inicial) según la convención CLI
/// de Fitz, dispatcha al `Value::Function` correspondiente. Devuelve
/// el exit code (Int del return) o un error de parsing.
///
/// Comportamiento:
/// - `bin_name --help` o `bin_name -h` → imprime help global + exit 0
///   (solo en modo multi-comando).
/// - `bin_name <cmd> --help` → imprime help del comando + exit 0.
/// - `bin_name <cmd> <args> [--flags]` → invoca el handler.
/// - Errors: comando desconocido, arg faltante, tipo inválido → exit 2
///   con mensaje al stderr.
///
/// La función NO ejecuta el handler — devuelve la spec resuelta para
/// que el caller (que tiene contexto del evaluator) lo invoque. Esto
/// separa parsing de execution para facilitar testing.
pub struct ParsedInvocation {
    pub cmd: CliCommand,
    pub args: Vec<Value>,
}

/// Resultado del parsing de argv. Tres casos:
/// - Help global pedido (`--help` en modo multi).
/// - Help de un comando pedido.
/// - Invocación lista para ejecutar.
/// - Error con mensaje + exit code sugerido.
pub enum ParseResult {
    GlobalHelp,
    CommandHelp(CliCommand),
    Invoke(ParsedInvocation),
    Error(String, i32),
}

/// Parser principal del CLI. Recibe `argv` post-binary-name + las
/// commands registradas. Aplica la convención (positional sin default,
/// flags con default).
pub fn parse_argv(argv: &[String], registry: &CliRegistry) -> ParseResult {
    let cmds = registry.snapshot();
    let multi = cmds.len() >= 2;

    // Caso single-comando: argv == args del único comando, sin subcomando.
    let (cmd, remaining) = if multi {
        // Modo multi: argv[0] es el nombre del subcomando.
        if argv.is_empty() {
            return ParseResult::GlobalHelp;
        }
        if argv[0] == "--help" || argv[0] == "-h" {
            return ParseResult::GlobalHelp;
        }
        let sub_name = &argv[0];
        let matched = match cmds.iter().find(|c| &c.name == sub_name) {
            Some(c) => c.clone(),
            None => {
                return ParseResult::Error(
                    format!(
                        "comando desconocido: `{}`. Run `--help` para ver comandos disponibles.",
                        sub_name
                    ),
                    2,
                );
            }
        };
        (matched, &argv[1..])
    } else {
        // Modo single: solo un comando, el subcomando NO se especifica.
        // argv son directamente los args del comando.
        if cmds.is_empty() {
            return ParseResult::Error("no hay comandos `@command` registrados".into(), 1);
        }
        (cmds[0].clone(), argv)
    };

    // Help del comando.
    for a in remaining {
        if a == "--help" || a == "-h" {
            return ParseResult::CommandHelp(cmd);
        }
    }

    // Parsear positional + flags.
    // v0.11.1 — variadic List<Str> es el último param del cmd (con default
    // `= []` por convención sintáctica). Se excluye de positional_params Y
    // de flag_params para tratamiento especial (acumula tokens restantes).
    let variadic_idx: Option<usize> = cmd
        .params
        .last()
        .filter(|p| {
            matches!(
                p.type_.as_ref().map(|t| t.display_name()),
                Some(n) if n.starts_with("List<")
            )
        })
        .map(|_| cmd.params.len() - 1);
    let positional_params: Vec<&Param> = cmd
        .params
        .iter()
        .enumerate()
        .filter(|(i, p)| Some(*i) != variadic_idx && !param_is_flag(p))
        .map(|(_, p)| p)
        .collect();
    let flag_params: Vec<&Param> = cmd
        .params
        .iter()
        .enumerate()
        .filter(|(i, p)| Some(*i) != variadic_idx && param_is_flag(p))
        .map(|(_, p)| p)
        .collect();
    let mut positional_values: Vec<String> = Vec::new();
    let mut flag_values: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();

    // v0.11.1 — pre-compute short flag map (`-l` → `loud`). Errores
    // de colisión son responsabilidad del codegen/checker en runtime
    // de codegen; acá fallback silencioso (`unwrap_or_default` = vacío).
    let short_map = compute_short_flags(&cmd.params).unwrap_or_default();

    let mut i = 0;
    while i < remaining.len() {
        let tok = &remaining[i];
        if let Some(rest) = tok.strip_prefix("--") {
            // v0.11.1 — `--no-<name>` para negar Bool flag (default
            // true → false). Si el rest empieza con `no-` Y el resto
            // matchea un flag Bool del comando, lo seteamos a false.
            // Si no matchea (ej: `--noisy` es flag legítima), seguimos
            // con el path normal.
            let negation_target = rest.strip_prefix("no-").and_then(|n| {
                flag_params
                    .iter()
                    .find(|p| p.name == n && param_is_bool(p))
                    .map(|_| n.to_string())
            });
            if let Some(target) = negation_target {
                // Aplicar negación: usamos sentinel `Some("false")`
                // que el coerce maneja consistente con `--name=false`.
                flag_values.insert(target, Some("false".to_string()));
                i += 1;
                continue;
            }
            // Flag long form: `--name` o `--name=value`.
            if let Some(eq_pos) = rest.find('=') {
                let key = &rest[..eq_pos];
                let val = &rest[eq_pos + 1..];
                flag_values.insert(key.to_string(), Some(val.to_string()));
                i += 1;
            } else {
                let key = rest;
                // ¿Es Bool flag? Buscar param matcheante.
                let param = flag_params.iter().find(|p| p.name == key);
                let is_bool_flag = param.map(|p| param_is_bool(p)).unwrap_or(false);
                if is_bool_flag {
                    flag_values.insert(key.to_string(), None);
                    i += 1;
                } else {
                    // Espera valor en el siguiente token.
                    if i + 1 >= remaining.len() {
                        return ParseResult::Error(format!("flag `--{}` espera un valor", key), 2);
                    }
                    flag_values.insert(key.to_string(), Some(remaining[i + 1].clone()));
                    i += 2;
                }
            }
        } else if let Some(short_part) = tok.strip_prefix('-').filter(|s| !s.is_empty()) {
            // v0.11.1 — short flag `-x` (single dash + single char).
            // Resuelve via `short_map` a su long name y reutiliza el mismo
            // path de flag long. MVP: solo `-x` single, sin combo `-xyz`
            // ni `-x=v`. El user usa la forma larga para esos shapes.
            let mut chars_iter = short_part.chars();
            let first = chars_iter
                .next()
                .expect("short_part no vacío validado arriba");
            if chars_iter.next().is_some() {
                return ParseResult::Error(
                    format!(
                        "flag corta combinada o con `=` no soportada en MVP: `{}`. Usá la forma larga `--<name>`.",
                        tok
                    ),
                    2,
                );
            }
            let long_name = match short_map.get(&first.to_ascii_lowercase()) {
                Some(n) => n.clone(),
                None => {
                    return ParseResult::Error(
                        format!(
                            "flag corta desconocida `-{}`. Disponibles: {}",
                            first,
                            short_map
                                .iter()
                                .map(|(c, n)| format!("-{} (--{})", c, n))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        2,
                    );
                }
            };
            let param = flag_params.iter().find(|p| p.name == long_name);
            let is_bool_flag = param.map(|p| param_is_bool(p)).unwrap_or(false);
            if is_bool_flag {
                flag_values.insert(long_name, None);
                i += 1;
            } else {
                if i + 1 >= remaining.len() {
                    return ParseResult::Error(
                        format!("flag `-{}` (--{}) espera un valor", first, long_name),
                        2,
                    );
                }
                flag_values.insert(long_name, Some(remaining[i + 1].clone()));
                i += 2;
            }
        } else {
            positional_values.push(tok.clone());
            i += 1;
        }
    }

    // v0.11.1 — variadic_idx ya capturado arriba. Si está, los positionals
    // requeridos son los que vienen ANTES del variadic.
    let has_variadic = variadic_idx.is_some();
    let required_positional_count = positional_params.len();
    if positional_values.len() < required_positional_count {
        let missing_param = &positional_params[positional_values.len()];
        return ParseResult::Error(
            format!(
                "falta el argumento posicional `<{}>` (esperaba {} positional args, recibió {})",
                missing_param.name,
                required_positional_count,
                positional_values.len()
            ),
            2,
        );
    }
    if !has_variadic && positional_values.len() > positional_params.len() {
        return ParseResult::Error(
            format!(
                "demasiados argumentos posicionales: esperaba {}, recibió {} (extras: {:?})",
                positional_params.len(),
                positional_values.len(),
                &positional_values[positional_params.len()..]
            ),
            2,
        );
    }

    // Coerce values to Value. Order: positional first (in declaration
    // order), then flags (in declaration order), to match the handler
    // signature.
    let mut invoke_args: Vec<Value> = Vec::with_capacity(cmd.params.len());
    let mut pos_iter = positional_values.into_iter();
    for (idx, p) in cmd.params.iter().enumerate() {
        // v0.11.1 — variadic param: consume todos los tokens restantes
        // del positional stream en una List.
        if Some(idx) == variadic_idx {
            let rest_strs: Vec<Value> = pos_iter.by_ref().map(Value::Str).collect();
            let list_shared = std::sync::Arc::new(parking_lot::Mutex::new(rest_strs));
            invoke_args.push(Value::List(list_shared));
            continue;
        }
        if param_is_flag(p) {
            // Flag — buscar en flag_values, sino usar default.
            let provided = flag_values.remove(&p.name);
            let val = match provided {
                Some(raw) => match coerce_arg_value(p, raw.as_deref()) {
                    Ok(v) => v,
                    Err(e) => return ParseResult::Error(e, 2),
                },
                None => {
                    // No provided → usar default. El default se eval-uará
                    // por el evaluator al invocar — mandamos sentinel
                    // `Value::Null` y el invoker resuelve.
                    Value::Null
                }
            };
            invoke_args.push(val);
        } else {
            let raw = pos_iter.next().expect("positional count validated above");
            let val = match coerce_arg_value(p, Some(&raw)) {
                Ok(v) => v,
                Err(e) => return ParseResult::Error(e, 2),
            };
            invoke_args.push(val);
        }
    }

    // Flags sobrantes → error claro.
    if let Some((unknown, _)) = flag_values.iter().next() {
        return ParseResult::Error(
            format!(
                "flag desconocida `--{}`. Run `--help` para ver flags válidas.",
                unknown
            ),
            2,
        );
    }

    ParseResult::Invoke(ParsedInvocation {
        cmd,
        args: invoke_args,
    })
}

/// Detecta si un param es Bool puro (no Bool?).
fn param_is_bool(p: &Param) -> bool {
    matches!(
        p.type_.as_ref().map(|t| t.head_name()),
        Some(n) if n == "Bool"
    )
}

/// Coerciona un valor string del CLI al tipo del param. None = flag
/// sin valor (Bool present → true).
fn coerce_arg_value(p: &Param, raw: Option<&str>) -> Result<Value, String> {
    let head: &str = p.type_.as_ref().map(|t| t.head_name()).unwrap_or("Str");
    match raw {
        None => {
            // Solo válido para Bool (flag sin valor).
            if head == "Bool" {
                Ok(Value::Bool(true))
            } else {
                Err(format!(
                    "flag `--{}` requiere un valor (tipo `{}`)",
                    p.name, head
                ))
            }
        }
        Some(s) => match head {
            "Str" => Ok(Value::Str(s.to_string())),
            "Int" => s
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| format!("`--{}` espera Int, recibió `{}`", p.name, s)),
            "Float" => s
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| format!("`--{}` espera Float, recibió `{}`", p.name, s)),
            "Bool" => match s {
                "true" | "1" | "yes" => Ok(Value::Bool(true)),
                "false" | "0" | "no" => Ok(Value::Bool(false)),
                _ => Err(format!(
                    "`--{}` espera Bool (true/false/1/0/yes/no), recibió `{}`",
                    p.name, s
                )),
            },
            other => Err(format!(
                "tipo `{}` no marshallable desde CLI (deuda menor — bajá a Str y parseá en el body)",
                other
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Param, Span, TypeExpr};

    fn param(name: &str, ty: &str, has_default: bool) -> Param {
        Param {
            name: name.into(),
            type_: Some(TypeExpr::Named(ty.into())),
            default: if has_default {
                // El valor del default no importa para los tests de
                // parsing — solo importa que esté presente para la
                // convención positional vs flag.
                Some(crate::ast::Expr::Int(0, crate::ast::Span::ZERO))
            } else {
                None
            },
            varargs: false,
            name_span: Span::default(),
        }
    }

    fn mkcmd(name: &str, params: Vec<Param>) -> CliCommand {
        CliCommand {
            name: name.into(),
            desc: None,
            fn_name: name.into(),
            params,
            handler: Value::Null,
        }
    }

    #[test]
    fn registry_register_y_count() {
        let reg = CliRegistry::new();
        assert_eq!(reg.count(), 0);
        reg.register(mkcmd("greet", vec![])).unwrap();
        assert_eq!(reg.count(), 1);
    }

    #[test]
    fn registry_duplicate_name_rechazado() {
        let reg = CliRegistry::new();
        reg.register(mkcmd("greet", vec![])).unwrap();
        let dup = reg.register(mkcmd("greet", vec![]));
        assert!(dup.is_err());
        assert!(dup.unwrap_err().contains("duplicado"));
    }

    #[test]
    fn param_is_flag_segun_default() {
        let p_no_default = param("name", "Str", false);
        let p_with_default = param("loud", "Bool", true);
        assert!(!param_is_flag(&p_no_default));
        assert!(param_is_flag(&p_with_default));
    }

    #[test]
    fn parse_argv_single_command_positional() {
        let reg = CliRegistry::new();
        reg.register(mkcmd("greet", vec![param("name", "Str", false)]))
            .unwrap();
        let result = parse_argv(&["Ada".to_string()], &reg);
        match result {
            ParseResult::Invoke(inv) => {
                assert_eq!(inv.cmd.name, "greet");
                assert_eq!(inv.args.len(), 1);
                matches!(inv.args[0], Value::Str(_));
            }
            _ => panic!("esperaba Invoke"),
        }
    }

    #[test]
    fn parse_argv_single_command_help() {
        let reg = CliRegistry::new();
        reg.register(mkcmd("greet", vec![])).unwrap();
        let result = parse_argv(&["--help".to_string()], &reg);
        assert!(matches!(result, ParseResult::CommandHelp(_)));
    }

    #[test]
    fn parse_argv_multi_command_global_help() {
        let reg = CliRegistry::new();
        reg.register(mkcmd("greet", vec![])).unwrap();
        reg.register(mkcmd("status", vec![])).unwrap();
        let result = parse_argv(&[], &reg);
        assert!(matches!(result, ParseResult::GlobalHelp));
    }

    #[test]
    fn parse_argv_multi_command_dispatcha_subcomando() {
        let reg = CliRegistry::new();
        reg.register(mkcmd("greet", vec![param("name", "Str", false)]))
            .unwrap();
        reg.register(mkcmd("status", vec![])).unwrap();
        let result = parse_argv(&["status".to_string()], &reg);
        match result {
            ParseResult::Invoke(inv) => assert_eq!(inv.cmd.name, "status"),
            _ => panic!("esperaba Invoke"),
        }
    }

    #[test]
    fn parse_argv_unknown_command() {
        let reg = CliRegistry::new();
        reg.register(mkcmd("greet", vec![])).unwrap();
        reg.register(mkcmd("status", vec![])).unwrap();
        let result = parse_argv(&["foo".to_string()], &reg);
        match result {
            ParseResult::Error(msg, code) => {
                assert!(msg.contains("desconocido"));
                assert_eq!(code, 2);
            }
            _ => panic!("esperaba Error"),
        }
    }

    #[test]
    fn parse_argv_missing_positional_es_error() {
        let reg = CliRegistry::new();
        reg.register(mkcmd("greet", vec![param("name", "Str", false)]))
            .unwrap();
        let result = parse_argv(&[], &reg);
        match result {
            ParseResult::Error(msg, _) => assert!(msg.contains("falta")),
            _ => panic!("esperaba Error"),
        }
    }

    #[test]
    fn parse_argv_flag_bool_present() {
        let reg = CliRegistry::new();
        reg.register(mkcmd(
            "greet",
            vec![param("name", "Str", false), param("loud", "Bool", true)],
        ))
        .unwrap();
        let result = parse_argv(&["Ada".to_string(), "--loud".to_string()], &reg);
        match result {
            ParseResult::Invoke(inv) => {
                assert_eq!(inv.args.len(), 2);
                assert!(matches!(inv.args[1], Value::Bool(true)));
            }
            _ => panic!("esperaba Invoke"),
        }
    }

    #[test]
    fn parse_argv_flag_int_con_valor_siguiente() {
        let reg = CliRegistry::new();
        reg.register(mkcmd(
            "greet",
            vec![param("name", "Str", false), param("count", "Int", true)],
        ))
        .unwrap();
        let result = parse_argv(
            &["Ada".to_string(), "--count".to_string(), "5".to_string()],
            &reg,
        );
        match result {
            ParseResult::Invoke(inv) => {
                assert_eq!(inv.args.len(), 2);
                assert!(matches!(inv.args[1], Value::Int(5)));
            }
            _ => panic!("esperaba Invoke"),
        }
    }

    #[test]
    fn parse_argv_flag_eq_value() {
        let reg = CliRegistry::new();
        reg.register(mkcmd(
            "greet",
            vec![param("name", "Str", false), param("count", "Int", true)],
        ))
        .unwrap();
        let result = parse_argv(&["Ada".to_string(), "--count=7".to_string()], &reg);
        match result {
            ParseResult::Invoke(inv) => {
                assert!(matches!(inv.args[1], Value::Int(7)));
            }
            _ => panic!("esperaba Invoke"),
        }
    }

    #[test]
    fn parse_argv_unknown_flag_es_error() {
        let reg = CliRegistry::new();
        reg.register(mkcmd("greet", vec![param("name", "Str", false)]))
            .unwrap();
        let result = parse_argv(
            &["Ada".to_string(), "--bogus".to_string(), "x".to_string()],
            &reg,
        );
        // El parser primero asocia `--bogus` con valor "x", después
        // al final ve la flag desconocida.
        match result {
            ParseResult::Error(msg, _) => assert!(msg.contains("desconocida")),
            _ => panic!("esperaba Error"),
        }
    }

    #[test]
    fn render_global_help_lista_comandos() {
        let cmds = vec![
            CliCommand {
                name: "greet".into(),
                desc: Some("Greet a person".into()),
                fn_name: "greet".into(),
                params: vec![],
                handler: Value::Null,
            },
            CliCommand {
                name: "status".into(),
                desc: None,
                fn_name: "status".into(),
                params: vec![],
                handler: Value::Null,
            },
        ];
        let help = render_global_help("mybin", &cmds);
        assert!(help.contains("greet"));
        assert!(help.contains("status"));
        assert!(help.contains("Greet a person"));
        assert!(help.contains("USAGE"));
    }

    #[test]
    fn render_command_help_incluye_usage_y_options() {
        let cmd = CliCommand {
            name: "greet".into(),
            desc: Some("Greet a person".into()),
            fn_name: "greet".into(),
            params: vec![param("name", "Str", false), param("loud", "Bool", true)],
            handler: Value::Null,
        };
        let help = render_command_help("mybin", &cmd, true);
        assert!(help.contains("Greet a person"));
        assert!(help.contains("USAGE"));
        assert!(help.contains("mybin greet"));
        assert!(help.contains("<name>"));
        assert!(help.contains("--loud"));
        assert!(help.contains("--help"));
    }
}
