// num.rs — FITZ-04 (2026-08): the `num` module — locale-aware number formatting.
//
// The format specs (`{n:,}`, `{r:.1%}`, ...) already compile to native binary,
// but they are NOT locale-aware: the decimal point is always `.` and the
// thousands separator is the char you pick. For a math game aimed at Argentine
// kids, `1.234,5` and `$ 1.250` must render correctly — showing `1,234.5` is
// pedagogically wrong. `num.*` fills that gap.
//
// MVP: positional args (`num.format(x, "es-AR")`) — kwargs (`locale:`) are a
// future ergonomic refinement (they need the heavier `dispatch_builtin_kwargs`
// path + a codegen parallel). Locales `es-AR` and `en-US` are explicit;
// anything else falls back to `en-US`. The core formatting is byte-identical in
// the interpreter and in the codegen prelude (paridad run↔build).

use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::value::Value;

/// Locale formatting rules. Kept tiny on purpose (no ICU).
pub struct LocaleFmt {
    pub decimal: char,
    pub thousands: char,
    /// `true` = symbol before the amount (`$ 1.250`), `false` = after.
    pub symbol_before: bool,
    /// `true` = a space between symbol and amount (`$ 1.250` vs `$1,250`).
    pub symbol_space: bool,
    /// `true` = a space before `%` (`42,0 %` vs `42.0%`).
    pub percent_space: bool,
}

pub fn locale_fmt(locale: &str) -> LocaleFmt {
    match locale {
        "es-AR" | "es-ar" => LocaleFmt {
            decimal: ',',
            thousands: '.',
            symbol_before: true,
            symbol_space: true,
            percent_space: true,
        },
        // en-US and any unknown locale.
        _ => LocaleFmt {
            decimal: '.',
            thousands: ',',
            symbol_before: true,
            symbol_space: false,
            percent_space: false,
        },
    }
}

/// Currency symbol for a code (minimal table). Unknown → the code itself.
pub fn currency_symbol(code: &str) -> String {
    match code {
        "ARS" | "USD" | "MXN" | "CAD" | "AUD" | "CLP" | "COP" => "$".to_string(),
        "EUR" => "€".to_string(),
        "GBP" => "£".to_string(),
        "JPY" => "¥".to_string(),
        "BRL" => "R$".to_string(),
        other => other.to_string(),
    }
}

/// Groups the integer digit string in 3s with `sep`. Handles a leading `-`.
/// `"1234567"` → `"1.234.567"` (with `sep = '.'`).
pub fn group_int(int_digits: &str, sep: char) -> String {
    let (neg, digits) = match int_digits.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, int_digits),
    };
    let bytes = digits.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(n + n / 3 + 1);
    if neg {
        out.push('-');
    }
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(*b as char);
    }
    out
}

/// Formats a number string (`"1234.5"`, `"-42"`) into the locale: groups the
/// integer part, swaps the decimal separator. Shared by all three builtins.
pub fn apply_locale(number_str: &str, fmt: &LocaleFmt) -> String {
    match number_str.split_once('.') {
        Some((int_part, frac_part)) => {
            format!(
                "{}{}{}",
                group_int(int_part, fmt.thousands),
                fmt.decimal,
                frac_part
            )
        }
        None => group_int(number_str, fmt.thousands),
    }
}

/// `num.format(x, locale)` core: render `x` with its natural digits, localised.
pub fn format_value(x: f64, is_int: bool, locale: &str) -> String {
    let fmt = locale_fmt(locale);
    let s = if is_int {
        format!("{}", x as i64)
    } else {
        format!("{}", x)
    };
    apply_locale(&s, &fmt)
}

/// `num.percent(x, locale, digits)` core: `x*100` with `digits` decimals + `%`.
pub fn format_percent(x: f64, locale: &str, digits: usize) -> String {
    let fmt = locale_fmt(locale);
    let s = format!("{:.*}", digits, x * 100.0);
    let body = apply_locale(&s, &fmt);
    if fmt.percent_space {
        format!("{} %", body)
    } else {
        format!("{}%", body)
    }
}

/// `num.currency(x, locale, code)` core: symbol + amount with 2 decimals.
pub fn format_currency(x: f64, locale: &str, code: &str) -> String {
    let fmt = locale_fmt(locale);
    let sym = currency_symbol(code);
    let s = format!("{:.2}", x);
    let body = apply_locale(&s, &fmt);
    match (fmt.symbol_before, fmt.symbol_space) {
        (true, true) => format!("{} {}", sym, body),
        (true, false) => format!("{}{}", sym, body),
        (false, true) => format!("{} {}", body, sym),
        (false, false) => format!("{}{}", body, sym),
    }
}

// --- helpers -----------------------------------------------------------------

fn arg_err(msg: impl Into<String>) -> FitzError {
    FitzError::new(ErrorKind::InvalidSyntax, 0, 0, msg.into())
}

fn expect_number(v: &Value, ctx: &str) -> FitzResult<(f64, bool)> {
    match v {
        Value::Int(n) => Ok((*n as f64, true)),
        Value::Float(f) => Ok((*f, false)),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int | Float".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!(
                "{}: expects Int or Float, received `{}`",
                ctx,
                other.type_name()
            ),
        )),
    }
}

fn expect_str(v: &Value, ctx: &str) -> FitzResult<String> {
    match v {
        Value::Str(s) => Ok(s.clone()),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Str".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!("{}: expects Str, received `{}`", ctx, other.type_name()),
        )),
    }
}

// --- builtins ----------------------------------------------------------------

/// `num.format(x, locale) -> Str`.
pub fn builtin_num_format(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(arg_err("`num.format(x, locale)` expects 2 arguments"));
    }
    let (x, is_int) = expect_number(&args[0], "num.format")?;
    let locale = expect_str(&args[1], "num.format")?;
    Ok(Value::Str(format_value(x, is_int, &locale)))
}

/// `num.percent(x, locale, digits) -> Str`.
pub fn builtin_num_percent(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 3 {
        return Err(arg_err(
            "`num.percent(x, locale, digits)` expects 3 arguments",
        ));
    }
    let (x, _) = expect_number(&args[0], "num.percent")?;
    let locale = expect_str(&args[1], "num.percent")?;
    let digits = match &args[2] {
        Value::Int(n) if *n >= 0 => *n as usize,
        _ => return Err(arg_err("`num.percent`: digits must be a non-negative Int")),
    };
    Ok(Value::Str(format_percent(x, &locale, digits)))
}

/// `num.currency(x, locale, code) -> Str`.
pub fn builtin_num_currency(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 3 {
        return Err(arg_err(
            "`num.currency(x, locale, code)` expects 3 arguments",
        ));
    }
    let (x, _) = expect_number(&args[0], "num.currency")?;
    let locale = expect_str(&args[1], "num.currency")?;
    let code = expect_str(&args[2], "num.currency")?;
    Ok(Value::Str(format_currency(x, &locale, &code)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn es_ar_format_groups_and_swaps_decimal() {
        assert_eq!(format_value(1234.5, false, "es-AR"), "1.234,5");
        assert_eq!(format_value(1234567.0, false, "es-AR"), "1.234.567");
        assert_eq!(format_value(1234567.0, true, "es-AR"), "1.234.567");
        assert_eq!(format_value(-1234.5, false, "es-AR"), "-1.234,5");
    }

    #[test]
    fn en_us_format() {
        assert_eq!(format_value(1234.5, false, "en-US"), "1,234.5");
        assert_eq!(format_value(1234567.0, true, "en-US"), "1,234,567");
    }

    #[test]
    fn percent_es_ar_has_space_and_comma() {
        assert_eq!(format_percent(0.42, "es-AR", 1), "42,0 %");
        assert_eq!(format_percent(0.42, "en-US", 1), "42.0%");
    }

    #[test]
    fn currency_es_ar_symbol_before_with_space() {
        assert_eq!(format_currency(1250.0, "es-AR", "ARS"), "$ 1.250,00");
        assert_eq!(format_currency(1250.0, "en-US", "USD"), "$1,250.00");
        assert_eq!(format_currency(1234.5, "es-AR", "EUR"), "€ 1.234,50");
    }

    #[test]
    fn unknown_locale_falls_back_to_en_us() {
        assert_eq!(format_value(1234.5, false, "xx-YY"), "1,234.5");
    }
}
