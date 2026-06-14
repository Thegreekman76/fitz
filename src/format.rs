//! Mini-tanda Fm — aplicación de `FormatSpec` a un `Value`.
//!
//! Implementa la semántica completa del format spec estilo Python:
//! width, alignment, fill, sign, alternate form, grouping (`,`/`_`),
//! precision, y type chars (`b`/`c`/`d`/`e`/`E`/`f`/`F`/`g`/`G`/`o`/
//! `s`/`x`/`X`/`%`).
//!
//! El entry point es `format_value_with_spec(value, spec)` que devuelve
//! un `Result<String, String>` con el mensaje de error legible si el
//! tipo no es compatible con el spec.

use crate::ast::{FormatAlign, FormatKind, FormatSign, FormatSpec};
use crate::value::Value;

/// Aplica el spec al value y devuelve el string formateado. Falla con
/// mensaje claro si tipo y spec son incompatibles.
pub fn format_value_with_spec(v: &Value, spec: &FormatSpec) -> Result<String, String> {
    // 1. Body — la representación cruda del valor según el `kind` del
    // spec. Sin width/padding/grouping todavía.
    let (body, is_numeric) = format_body(v, spec)?;

    // 2. Apply grouping antes del sign (sobre la parte numérica).
    // `,` y `_` solo aplican a Int y Float (integer part).
    let body = if let Some(g) = spec.grouping {
        if is_numeric {
            apply_grouping(&body, g)
        } else {
            body
        }
    } else {
        body
    };

    // 3. Sign — agregado al frente si numérico y empieza con dígito.
    let body = apply_sign(&body, spec, is_numeric);

    // 4. Padding (width + align + fill).
    let body = apply_padding(&body, spec, is_numeric);

    Ok(body)
}

/// Convierte el value a string según `spec.kind`. Devuelve también un
/// flag `is_numeric` para que las pasadas siguientes sepan si aplicar
/// grouping, signed padding, etc.
fn format_body(v: &Value, spec: &FormatSpec) -> Result<(String, bool), String> {
    let Some(kind) = spec.kind else {
        // Sin type char → Display por default. No es estrictamente
        // "numeric" para padding (no aplica zero-pad), pero los Int/
        // Float crudos sí lo son para grouping.
        let s = v.to_string();
        let is_num = matches!(v, Value::Int(_) | Value::Float(_));
        return Ok((s, is_num));
    };
    match kind {
        FormatKind::String => Ok((v.to_string(), false)),
        FormatKind::FixedLower | FormatKind::FixedUpper => {
            let f = coerce_float(v).ok_or_else(|| numeric_err("Float o Int", v, "f"))?;
            let prec = spec.precision.unwrap_or(6);
            // Rust no tiene `FixedUpper` directo (todos `f` son
            // lowercase); para 'F' uppercase nada cambia con dígitos
            // pero capitaliza `inf`/`nan` si aparecieran.
            let s = format!("{:.*}", prec, f);
            let s = if matches!(kind, FormatKind::FixedUpper) {
                s.to_uppercase()
            } else {
                s
            };
            Ok((s, true))
        }
        FormatKind::ExponentLower | FormatKind::ExponentUpper => {
            let f = coerce_float(v).ok_or_else(|| numeric_err("Float o Int", v, "e/E"))?;
            let prec = spec.precision.unwrap_or(6);
            let raw = format!("{:.*e}", prec, f);
            let s = if matches!(kind, FormatKind::ExponentUpper) {
                raw.to_uppercase()
            } else {
                raw
            };
            Ok((s, true))
        }
        FormatKind::GeneralLower | FormatKind::GeneralUpper => {
            // `g`/`G`: precisión = número total de dígitos significativos.
            // Decisión Python: si el exponente es < -4 o >= precision,
            // usa notación científica; sino fija.
            let f = coerce_float(v).ok_or_else(|| numeric_err("Float o Int", v, "g/G"))?;
            let prec = spec.precision.unwrap_or(6).max(1);
            let s = general_format(f, prec, matches!(kind, FormatKind::GeneralUpper));
            Ok((s, true))
        }
        FormatKind::Percent => {
            let f = coerce_float(v).ok_or_else(|| numeric_err("Float o Int", v, "%"))?;
            let prec = spec.precision.unwrap_or(6);
            let s = format!("{:.*}%", prec, f * 100.0);
            Ok((s, true))
        }
        FormatKind::Decimal => {
            let n = coerce_int(v).ok_or_else(|| numeric_err("Int", v, "d"))?;
            Ok((format!("{}", n), true))
        }
        FormatKind::Binary => {
            let n = coerce_int(v).ok_or_else(|| numeric_err("Int", v, "b"))?;
            let body = format_int_with_alternate(n, 2, spec.alternate, "0b", false);
            Ok((body, true))
        }
        FormatKind::Octal => {
            let n = coerce_int(v).ok_or_else(|| numeric_err("Int", v, "o"))?;
            let body = format_int_with_alternate(n, 8, spec.alternate, "0o", false);
            Ok((body, true))
        }
        FormatKind::HexLower => {
            let n = coerce_int(v).ok_or_else(|| numeric_err("Int", v, "x"))?;
            let body = format_int_with_alternate(n, 16, spec.alternate, "0x", false);
            Ok((body, true))
        }
        FormatKind::HexUpper => {
            let n = coerce_int(v).ok_or_else(|| numeric_err("Int", v, "X"))?;
            let body = format_int_with_alternate(n, 16, spec.alternate, "0X", true);
            Ok((body, true))
        }
        FormatKind::Char => {
            let n = coerce_int(v).ok_or_else(|| numeric_err("Int", v, "c"))?;
            let c = u32::try_from(n)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| format!(
                    "`c` requiere un Int en rango de Unicode codepoint válido (0..=0x10FFFF, sin surrogates), recibió `{}`",
                    n
                ))?;
            Ok((c.to_string(), false))
        }
    }
}

fn coerce_int(v: &Value) -> Option<i64> {
    match v {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

fn coerce_float(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(x) => Some(*x),
        _ => None,
    }
}

fn numeric_err(expected: &str, v: &Value, kind: &str) -> String {
    format!(
        "format spec `{}` esperaba {}, recibió `{}`",
        kind,
        expected,
        v.type_name()
    )
}

/// Formato Int en una base dada. Devuelve el string sin signo (el signo
/// lo agrega `apply_sign` después). `alternate` agrega el prefijo
/// (`0b`/`0o`/`0x`/`0X`). `upper` aplica uppercase al hex.
fn format_int_with_alternate(
    n: i64,
    base: u32,
    alternate: bool,
    prefix: &str,
    upper: bool,
) -> String {
    let (sign_prefix, abs_str) = if n < 0 {
        let s = match base {
            2 => format!("{:b}", n.unsigned_abs()),
            8 => format!("{:o}", n.unsigned_abs()),
            16 => format!("{:x}", n.unsigned_abs()),
            _ => format!("{}", n.unsigned_abs()),
        };
        ("-".to_string(), s)
    } else {
        let s = match base {
            2 => format!("{:b}", n),
            8 => format!("{:o}", n),
            16 => format!("{:x}", n),
            _ => format!("{}", n),
        };
        (String::new(), s)
    };
    let abs_str = if upper {
        abs_str.to_uppercase()
    } else {
        abs_str
    };
    if alternate {
        format!("{}{}{}", sign_prefix, prefix, abs_str)
    } else {
        format!("{}{}", sign_prefix, abs_str)
    }
}

/// Implementa la semántica `g`/`G` de Python. Precision = dígitos
/// significativos. Si el exponente decimal cae fuera de
/// `[-4, precision)`, usa notación científica; sino formato fijo.
fn general_format(f: f64, precision: usize, upper: bool) -> String {
    if f == 0.0 {
        return "0".into();
    }
    let abs = f.abs();
    let exp = abs.log10().floor() as i32;
    let use_exp = exp < -4 || exp >= precision as i32;
    let result = if use_exp {
        // Notación científica: la precisión es total - 1 dígito antes
        // del punto.
        let prec_after = precision.saturating_sub(1);
        let s = format!("{:.*e}", prec_after, f);
        s
    } else {
        // Fijo: ajusta precisión a "dígitos totales".
        let after = (precision as i32 - 1 - exp).max(0) as usize;
        let s = format!("{:.*}", after, f);
        // Strip trailing zeros (Python lo hace).
        strip_trailing_zeros(s)
    };
    if upper {
        result.to_uppercase()
    } else {
        result
    }
}

fn strip_trailing_zeros(s: String) -> String {
    if !s.contains('.') {
        return s;
    }
    let trimmed = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed
    }
}

/// Inserta separadores cada 3 dígitos en la parte entera del número.
/// `sep` es `,` o `_`. Maneja signos y parte decimal.
fn apply_grouping(s: &str, sep: char) -> String {
    let (sign, rest) = if let Some(stripped) = s.strip_prefix('-') {
        ("-", stripped)
    } else {
        ("", s)
    };
    // Separar parte entera de parte fraccional.
    let (int_part, frac_part) = match rest.find('.') {
        Some(i) => (&rest[..i], Some(&rest[i..])),
        None => (rest, None),
    };
    // No agrupar si la parte entera tiene prefix `0x`/`0b`/`0o` (alternate).
    if int_part.starts_with("0x")
        || int_part.starts_with("0X")
        || int_part.starts_with("0b")
        || int_part.starts_with("0o")
    {
        return s.to_string();
    }
    let chars: Vec<char> = int_part.chars().collect();
    let mut grouped = String::new();
    for (i, c) in chars.iter().rev().enumerate() {
        if i > 0 && i % 3 == 0 && c.is_ascii_digit() {
            grouped.push(sep);
        }
        grouped.push(*c);
    }
    let grouped: String = grouped.chars().rev().collect();
    match frac_part {
        Some(f) => format!("{}{}{}", sign, grouped, f),
        None => format!("{}{}", sign, grouped),
    }
}

/// Aplica el signo explícito si está y el body no tiene signo natural.
fn apply_sign(body: &str, spec: &FormatSpec, is_numeric: bool) -> String {
    let Some(sign) = spec.sign else {
        return body.to_string();
    };
    if !is_numeric {
        return body.to_string();
    }
    // Si ya empieza con `-`, sign Minus es no-op y Plus/Space igual
    // dejan el signo natural intacto.
    if body.starts_with('-') {
        return body.to_string();
    }
    match sign {
        FormatSign::Plus => format!("+{}", body),
        FormatSign::Space => format!(" {}", body),
        FormatSign::Minus => body.to_string(),
    }
}

/// Aplica padding según `width`, `align`, `fill`, `zero_pad`. Sin width
/// no hace nada.
fn apply_padding(body: &str, spec: &FormatSpec, is_numeric: bool) -> String {
    let Some(w) = spec.width else {
        return body.to_string();
    };
    let cur_len = body.chars().count();
    if cur_len >= w {
        return body.to_string();
    }
    let pad = w - cur_len;
    // zero_pad implica fill='0', align='=' (después del signo) si no
    // hay align explícito. Si hay align explícito, zero_pad solo
    // cambia el fill.
    let (fill, align) = if spec.zero_pad && is_numeric {
        (
            spec.fill.unwrap_or('0'),
            spec.align.unwrap_or(FormatAlign::Pad),
        )
    } else {
        (
            spec.fill.unwrap_or(' '),
            spec.align.unwrap_or(
                // Default align: right para numéricos, left para strings.
                if is_numeric {
                    FormatAlign::Right
                } else {
                    FormatAlign::Left
                },
            ),
        )
    };
    let fill_str: String = std::iter::repeat_n(fill, pad).collect();
    match align {
        FormatAlign::Left => format!("{}{}", body, fill_str),
        FormatAlign::Right => format!("{}{}", fill_str, body),
        FormatAlign::Center => {
            let left = pad / 2;
            let right = pad - left;
            let l: String = std::iter::repeat_n(fill, left).collect();
            let r: String = std::iter::repeat_n(fill, right).collect();
            format!("{}{}{}", l, body, r)
        }
        FormatAlign::Pad => {
            // Insertar fill entre el signo (si lo hay) y el body.
            // Solo si el body empieza con signo o prefix (0x/etc.) Sino
            // equivale a Right.
            if let Some(rest) = body.strip_prefix('-') {
                format!("-{}{}", fill_str, rest)
            } else if let Some(rest) = body.strip_prefix('+') {
                format!("+{}{}", fill_str, rest)
            } else if let Some(rest) = body.strip_prefix(' ') {
                format!(" {}{}", fill_str, rest)
            } else if body.starts_with("0x")
                || body.starts_with("0X")
                || body.starts_with("0b")
                || body.starts_with("0o")
            {
                let (prefix, rest) = body.split_at(2);
                format!("{}{}{}", prefix, fill_str, rest)
            } else {
                format!("{}{}", fill_str, body)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::FormatSpec;

    /// Mini-helper: arma un `FormatSpec` con `precision` y `kind`
    /// FixedLower (caso `.Nf`).
    fn fixed(p: usize) -> FormatSpec {
        FormatSpec {
            precision: Some(p),
            kind: Some(FormatKind::FixedLower),
            ..FormatSpec::default()
        }
    }

    #[test]
    #[allow(clippy::approx_constant)] // 1.234 no es PI, es un Float genérico.
    fn fixed_precision_float_rounds_to_n_decimals() {
        let s = format_value_with_spec(&Value::Float(1.2345), &fixed(2)).unwrap();
        assert_eq!(s, "1.23");
    }

    #[test]
    fn fixed_precision_int_promotes_to_float() {
        let s = format_value_with_spec(&Value::Int(42), &fixed(2)).unwrap();
        assert_eq!(s, "42.00");
    }

    #[test]
    fn width_with_zero_pad_for_int() {
        let spec = FormatSpec {
            zero_pad: true,
            width: Some(5),
            kind: Some(FormatKind::Decimal),
            ..FormatSpec::default()
        };
        let s = format_value_with_spec(&Value::Int(42), &spec).unwrap();
        assert_eq!(s, "00042");
    }

    #[test]
    fn align_right_with_default_space_fill() {
        let spec = FormatSpec {
            align: Some(FormatAlign::Right),
            width: Some(5),
            kind: Some(FormatKind::Decimal),
            ..FormatSpec::default()
        };
        let s = format_value_with_spec(&Value::Int(42), &spec).unwrap();
        assert_eq!(s, "   42");
    }

    #[test]
    fn align_left_with_custom_fill() {
        let spec = FormatSpec {
            fill: Some('*'),
            align: Some(FormatAlign::Left),
            width: Some(5),
            kind: None,
            ..FormatSpec::default()
        };
        let s = format_value_with_spec(&Value::Str("ab".into()), &spec).unwrap();
        assert_eq!(s, "ab***");
    }

    #[test]
    fn align_center_with_balanced_padding() {
        let spec = FormatSpec {
            align: Some(FormatAlign::Center),
            width: Some(7),
            kind: None,
            ..FormatSpec::default()
        };
        let s = format_value_with_spec(&Value::Str("ab".into()), &spec).unwrap();
        assert_eq!(s, "  ab   ");
    }

    #[test]
    fn sign_plus_shows_sign_on_positives() {
        let spec = FormatSpec {
            sign: Some(FormatSign::Plus),
            kind: Some(FormatKind::Decimal),
            ..FormatSpec::default()
        };
        let s = format_value_with_spec(&Value::Int(42), &spec).unwrap();
        assert_eq!(s, "+42");
    }

    #[test]
    fn hex_with_alternate_emits_0x_prefix() {
        let spec = FormatSpec {
            alternate: true,
            kind: Some(FormatKind::HexLower),
            ..FormatSpec::default()
        };
        let s = format_value_with_spec(&Value::Int(255), &spec).unwrap();
        assert_eq!(s, "0xff");
    }

    #[test]
    fn binary_simple() {
        let spec = FormatSpec {
            kind: Some(FormatKind::Binary),
            ..FormatSpec::default()
        };
        let s = format_value_with_spec(&Value::Int(10), &spec).unwrap();
        assert_eq!(s, "1010");
    }

    #[test]
    fn grouping_comma_int() {
        let spec = FormatSpec {
            grouping: Some(','),
            kind: Some(FormatKind::Decimal),
            ..FormatSpec::default()
        };
        let s = format_value_with_spec(&Value::Int(1_000_000), &spec).unwrap();
        assert_eq!(s, "1,000,000");
    }

    #[test]
    fn percent_multiplies_by_100() {
        let spec = FormatSpec {
            precision: Some(1),
            kind: Some(FormatKind::Percent),
            ..FormatSpec::default()
        };
        let s = format_value_with_spec(&Value::Float(0.42), &spec).unwrap();
        assert_eq!(s, "42.0%");
    }

    #[test]
    fn fixed_with_str_is_incompatible_type_error() {
        let spec = fixed(2);
        let err = format_value_with_spec(&Value::Str("abc".into()), &spec).unwrap_err();
        assert!(err.contains("`f`"), "error: {err}");
        assert!(err.contains("Float o Int"));
    }
}
