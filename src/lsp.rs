// lsp.rs — Lógica del Language Server Protocol (Fase 9.x.1.b).
//
// Vive en la lib (`src/lib.rs` la expone como `pub mod lsp` detrás de
// la feature `lsp`) en lugar de adentro del bin para que sea
// unit-testeable: cargo no soporta bien `#[cfg(test)]` en `src/bin/*.rs`.
// El bin `src/bin/fitz-lsp.rs` consume esto vía `use fitz::lsp::...`.

use crate::error::FitzError;
use crate::lexer::tokenize;
use crate::parser::parse_with_recovery;
use crate::types::check_program;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

/// Pipeline LSP-style sobre `source`: tokeniza, parsea con recovery,
/// chequea tipos, y devuelve la lista combinada de errores. El nombre
/// "lsp-style" lo distingue de `fitz check`/`fitz run` que usan
/// `parser::parse` strict y abortan al primer error de parser.
///
/// Los errores del lexer abortan la pipeline (no hay AST sobre el cual
/// chequear). Parser y checker siempre devuelven sus errores en
/// paralelo a lo que pudieron recuperar.
pub fn check_source(source: &str) -> Vec<FitzError> {
    let tokens = match tokenize(source) {
        Ok(t) => t,
        Err(e) => return vec![e],
    };
    let (program, mut errors) = parse_with_recovery(tokens);
    let (_env, _types, mut type_errors) = check_program(&program);
    errors.append(&mut type_errors);
    errors
}

/// Convierte una lista de `FitzError` en `Diagnostic`s LSP. Pure
/// function — no toca el server ni I/O. El test suite cubre las
/// reglas de mapeo (1-based Fitz → 0-based LSP, sin posición → range
/// degenerado, hint → concatenado al message).
pub fn fitz_errors_to_diagnostics(errors: &[FitzError]) -> Vec<Diagnostic> {
    errors.iter().map(error_to_diagnostic).collect()
}

fn error_to_diagnostic(err: &FitzError) -> Diagnostic {
    // Fitz usa convención 1-based para line/column (el lexer empieza
    // en `line: 1, column: 1`). LSP usa 0-based en `Position`. Cuando
    // line == 0 && column == 0 es el sentinel "sin posición" (ver
    // `FitzError::Display` en `error.rs`); lo mapeamos a un range
    // degenerado al inicio del documento.
    let range = if err.line == 0 && err.column == 0 {
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        }
    } else {
        // saturating_sub por defensa contra `line == 0 && column != 0`
        // (no debería ocurrir, pero evita underflow). El range es de
        // 1 carácter — refinable a span completo cuando S1.Pattern/
        // TypeExpr sume `end_span` a los nodos del AST.
        let line = (err.line.saturating_sub(1)) as u32;
        let col = (err.column.saturating_sub(1)) as u32;
        Range {
            start: Position::new(line, col),
            end: Position::new(line, col + 1),
        }
    };

    // Concatenamos el hint al message porque LSP no tiene un campo
    // dedicado para sugerencias. VSCode renderea `\n` en el tooltip;
    // el formato matchea cómo `FitzError::Display` lo emite en CLI.
    let mut message = err.message.clone();
    if let Some(hint) = &err.hint {
        message.push_str("\n  Sugerencia: ");
        message.push_str(hint);
    }

    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("fitz".into()),
        message,
        ..Diagnostic::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn err_at(line: usize, column: usize, msg: &str) -> FitzError {
        FitzError::new(ErrorKind::TypeError, line, column, msg)
    }

    #[test]
    fn error_con_posicion_mapea_a_range_0_based_de_1_caracter() {
        let errs = vec![err_at(3, 5, "tipo incompatible")];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.range.start, Position::new(2, 4)); // 1-based → 0-based
        assert_eq!(d.range.end, Position::new(2, 5));
        assert_eq!(d.message, "tipo incompatible");
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(d.source.as_deref(), Some("fitz"));
    }

    #[test]
    fn error_sin_posicion_mapea_a_range_degenerado_al_inicio() {
        let errs = vec![err_at(0, 0, "sin línea ni columna")];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert_eq!(diags[0].range.start, Position::new(0, 0));
        assert_eq!(diags[0].range.end, Position::new(0, 0));
    }

    #[test]
    fn error_con_hint_concatena_sugerencia_al_message() {
        let err = err_at(1, 1, "variable no definida")
            .with_hint("¿quisiste decir `name`?");
        let diags = fitz_errors_to_diagnostics(&[err]);
        assert!(
            diags[0].message.contains("variable no definida"),
            "message base: {}",
            diags[0].message,
        );
        assert!(
            diags[0].message.contains("Sugerencia: ¿quisiste decir `name`?"),
            "message con hint: {}",
            diags[0].message,
        );
    }

    #[test]
    fn error_sin_hint_no_agrega_la_palabra_sugerencia() {
        let errs = vec![err_at(1, 1, "tipo incompatible")];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert!(!diags[0].message.contains("Sugerencia"));
    }

    #[test]
    fn lista_vacia_devuelve_vec_vacio() {
        let diags = fitz_errors_to_diagnostics(&[]);
        assert!(diags.is_empty());
    }

    #[test]
    fn multiples_errores_preservan_orden() {
        let errs = vec![
            err_at(1, 1, "primero"),
            err_at(5, 3, "segundo"),
            err_at(0, 0, "tercero sin posición"),
        ];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert_eq!(diags.len(), 3);
        assert_eq!(diags[0].message, "primero");
        assert_eq!(diags[1].message, "segundo");
        assert_eq!(diags[2].message, "tercero sin posición");
    }

    // Tests sobre `check_source` — pipeline LSP-style entero.

    #[test]
    fn check_source_programa_valido_no_emite_errores() {
        let src = "let x = 1\nlet y = 2\nprint(x + y)";
        let errs = check_source(src);
        assert!(errs.is_empty(), "errores inesperados: {errs:?}");
    }

    #[test]
    fn check_source_error_de_tipo_sale_del_checker() {
        let src = "let x: Int = \"texto\"";
        let errs = check_source(src);
        assert!(!errs.is_empty(), "checker debería rechazar Int = Str");
        assert!(
            errs.iter().any(|e| matches!(e.kind, ErrorKind::TypeError)),
            "esperaba al menos un TypeError: {errs:?}",
        );
    }

    #[test]
    fn check_source_recovery_no_aborta_ante_stmts_rotos() {
        // El parser con recovery debería darnos AST parcial + errores;
        // el checker chequea lo que recuperó. Sin recovery, la pipeline
        // abortaría en el primer error. El smoke acá es que `check_source`
        // devuelve algo (no panic) sobre una entrada con sintaxis rota.
        let src = "let x = ???\nlet y = 1\nlet z: Int = \"mal\"";
        let errs = check_source(src);
        assert!(
            !errs.is_empty(),
            "debería haber al menos un error de parser",
        );
    }
}
