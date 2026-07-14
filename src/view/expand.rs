// view/expand.rs — Phase 11.2.a — lower a raw `.fitzv` `ViewFile`
// into an `ExpandedViewFile` where every raw source blob has been
// parsed as classic Fitz AST (`crate::ast::TypeExpr`, `Expr`, `Stmt`,
// `Param`).
//
// **Scope of this commit**: parsing only. The classic checker is NOT
// invoked from here — that lands in Phase 11.2.b. The goal is to
// prove the wiring from `.fitzv` raw blobs back through the classic
// lexer + parser is straightforward, and to give the follow-up
// checker step a well-typed AST to work against.
//
// **Isolation model** (Invariant 4 of `docs/stack.md`): the view
// parser (`src/view/parser.rs`) stays isolated from the classic
// pipeline. This module is the ONE bridge that reuses
// `crate::lexer::tokenize` + `crate::parser::parse_*_from_source`.
// A bug in the view parser cannot reach the classic pipeline, but
// this bridge deliberately consumes both — that is its role.
//
// **Position mapping**: raw blobs are tokenized as if they started at
// (1, 1) in their own coord system. Before parsing, we shift each
// token's `(line, column)` by the blob's base offset so that spans
// inside the produced AST — and error positions — sit inside the
// enclosing `.fitzv` file. The base offset today is approximate
// (the POC parser captures the field/handler/interpolation's Loc,
// not the exact start of the raw content); refining this needs the
// POC's parser to also track the blob start offset, which is debt
// documented in `docs/fase-11-plan.md` §7.

use super::ast::{
    Attr as RawAttr, Component as RawComponent, EventHandler as RawEventHandler, Loc,
    StateField as RawStateField, Style as RawStyle, Template as RawTemplate,
    TemplateNode as RawTemplateNode, ViewFile,
};
use crate::ast as fast; // Fitz AST — imported prefixed to avoid collisions with view AST names.
use crate::error::FitzError;
use crate::lexer::{tokenize, TokenWithPos};
use crate::parser::{
    parse_expression_from_source, parse_parameters_from_source, parse_statements_from_source,
    parse_type_expression_from_source,
};
use std::fmt;

// ---------------------------------------------------------------------------
// Expanded AST — mirrors the raw view AST 1:1, but with every raw
// source blob replaced by a parsed classic-Fitz AST node.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedViewFile {
    pub components: Vec<ExpandedComponent>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedComponent {
    pub name: String,
    pub loc: Loc,
    pub state: Vec<ExpandedStateField>,
    pub events: Vec<ExpandedEventHandler>,
    pub template: Option<ExpandedTemplate>,
    pub style: Option<RawStyle>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedStateField {
    pub name: String,
    pub type_expr: fast::TypeExpr,
    pub default: fast::Expr,
    pub loc: Loc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedEventHandler {
    pub name: String,
    pub params: Vec<fast::Param>,
    pub body: Vec<fast::Stmt>,
    pub loc: Loc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedTemplate {
    pub roots: Vec<ExpandedTemplateNode>,
    pub loc: Loc,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpandedTemplateNode {
    Text(String),
    Interpolation {
        expr: fast::Expr,
        loc: Loc,
    },
    Element {
        tag: String,
        attrs: Vec<ExpandedAttr>,
        children: Vec<ExpandedTemplateNode>,
        self_closing: bool,
        loc: Loc,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpandedAttr {
    Static {
        name: String,
        value: String,
        loc: Loc,
    },
    Interpolation {
        name: String,
        expr: fast::Expr,
        loc: Loc,
    },
    /// `@click="handler"` — the value must be a bare identifier that
    /// names one of the enclosing component's `event ...` handlers.
    /// Cross-checking that identity happens in Phase 11.2.b (checker).
    Event {
        event_name: String,
        handler_name: String,
        loc: Loc,
    },
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An expansion error carries a message plus a `Loc` inside the
/// `.fitzv` file. All classic-parser errors are shifted here — the
/// caller never sees a raw `FitzError` with blob-local coords.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandError {
    pub message: String,
    pub loc: Loc,
    /// Optional label naming the context in which the error arose
    /// (e.g. `"state field 'title' type"`, `"event handler 'save' body"`).
    /// Helps the user find the right blob in a busy component.
    pub context: String,
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "view expand error at {}:{} — {} ({})",
            self.loc.line, self.loc.column, self.message, self.context
        )
    }
}

impl std::error::Error for ExpandError {}

pub type ExpandResult<T> = Result<T, ExpandError>;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Lower a raw `.fitzv` view file into its expanded form. Every
/// state field, event handler, template interpolation, and attr
/// binding has its raw source parsed as classic Fitz AST.
///
/// Returns the first error encountered. Multi-error recovery is
/// deliberately deferred: this pass is a strict compile-time step,
/// mirroring how the classic `parse()` behaves. Recovery (for the
/// LSP) will land alongside 11.7 when the view LSP surface lands.
pub fn expand(file: &ViewFile) -> ExpandResult<ExpandedViewFile> {
    let components = file
        .components
        .iter()
        .map(expand_component)
        .collect::<ExpandResult<Vec<_>>>()?;
    Ok(ExpandedViewFile { components })
}

fn expand_component(c: &RawComponent) -> ExpandResult<ExpandedComponent> {
    let state = c
        .state
        .iter()
        .map(|f| expand_state_field(f, &c.name))
        .collect::<ExpandResult<Vec<_>>>()?;
    let events = c
        .events
        .iter()
        .map(|e| expand_event_handler(e, &c.name))
        .collect::<ExpandResult<Vec<_>>>()?;
    let template = c
        .template
        .as_ref()
        .map(|t| expand_template(t, &c.name))
        .transpose()?;
    Ok(ExpandedComponent {
        name: c.name.clone(),
        loc: c.loc,
        state,
        events,
        template,
        style: c.style.clone(),
    })
}

fn expand_state_field(f: &RawStateField, component_name: &str) -> ExpandResult<ExpandedStateField> {
    let ctx_type = format!(
        "component '{}': state field '{}' type",
        component_name, f.name
    );
    let ctx_default = format!(
        "component '{}': state field '{}' default",
        component_name, f.name
    );
    let type_expr = parse_type_at(&f.type_expr_raw, f.loc, ctx_type)?;
    let default = parse_expr_at(&f.default_expr_raw, f.loc, ctx_default)?;
    Ok(ExpandedStateField {
        name: f.name.clone(),
        type_expr,
        default,
        loc: f.loc,
    })
}

fn expand_event_handler(
    h: &RawEventHandler,
    component_name: &str,
) -> ExpandResult<ExpandedEventHandler> {
    let ctx_params = format!("component '{}': event '{}' params", component_name, h.name);
    let ctx_body = format!("component '{}': event '{}' body", component_name, h.name);
    let params = parse_params_at(&h.params_raw, h.loc, ctx_params)?;
    let body = parse_stmts_at(&h.body_raw, h.loc, ctx_body)?;
    Ok(ExpandedEventHandler {
        name: h.name.clone(),
        params,
        body,
        loc: h.loc,
    })
}

fn expand_template(t: &RawTemplate, component_name: &str) -> ExpandResult<ExpandedTemplate> {
    let roots = t
        .roots
        .iter()
        .map(|n| expand_template_node(n, component_name))
        .collect::<ExpandResult<Vec<_>>>()?;
    Ok(ExpandedTemplate { roots, loc: t.loc })
}

fn expand_template_node(
    node: &RawTemplateNode,
    component_name: &str,
) -> ExpandResult<ExpandedTemplateNode> {
    match node {
        RawTemplateNode::Text(s) => Ok(ExpandedTemplateNode::Text(s.clone())),
        RawTemplateNode::Interpolation { expr_raw, loc } => {
            let ctx = format!("component '{component_name}': template interpolation");
            let expr = parse_expr_at(expr_raw, *loc, ctx)?;
            Ok(ExpandedTemplateNode::Interpolation { expr, loc: *loc })
        }
        RawTemplateNode::Element {
            tag,
            attrs,
            children,
            self_closing,
            loc,
        } => {
            let attrs = attrs
                .iter()
                .map(|a| expand_attr(a, component_name, tag))
                .collect::<ExpandResult<Vec<_>>>()?;
            let children = children
                .iter()
                .map(|n| expand_template_node(n, component_name))
                .collect::<ExpandResult<Vec<_>>>()?;
            Ok(ExpandedTemplateNode::Element {
                tag: tag.clone(),
                attrs,
                children,
                self_closing: *self_closing,
                loc: *loc,
            })
        }
    }
}

fn expand_attr(attr: &RawAttr, component_name: &str, tag: &str) -> ExpandResult<ExpandedAttr> {
    match attr {
        RawAttr::Static { name, value, loc } => Ok(ExpandedAttr::Static {
            name: name.clone(),
            value: value.clone(),
            loc: *loc,
        }),
        RawAttr::Interpolation {
            name,
            expr_raw,
            loc,
        } => {
            let ctx = format!("component '{component_name}': <{tag}> attr '{name}' interpolation");
            let expr = parse_expr_at(expr_raw, *loc, ctx)?;
            Ok(ExpandedAttr::Interpolation {
                name: name.clone(),
                expr,
                loc: *loc,
            })
        }
        RawAttr::Event {
            event_name,
            handler_raw,
            loc,
        } => {
            // The handler value must be a bare identifier. We
            // deliberately do not re-parse it as a classic expression
            // — the semantics of `@click="start"` is a *name* of an
            // event handler declared in the same component, not an
            // expression to evaluate.
            let name = handler_raw.trim();
            if name.is_empty() {
                return Err(ExpandError {
                    message: format!(
                        "attribute `@{event_name}` needs a handler name (bare identifier)"
                    ),
                    loc: *loc,
                    context: format!("component '{component_name}': <{tag}> @{event_name}"),
                });
            }
            if !is_bare_ident(name) {
                return Err(ExpandError {
                    message: format!(
                        "attribute `@{event_name}` must reference a handler by name — expressions are not supported yet (Phase 11.2 defers `@click=\"expr\"` support to later)"
                    ),
                    loc: *loc,
                    context: format!("component '{component_name}': <{tag}> @{event_name}"),
                });
            }
            Ok(ExpandedAttr::Event {
                event_name: event_name.clone(),
                handler_name: name.to_string(),
                loc: *loc,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_type_at(source: &str, base: Loc, context: String) -> ExpandResult<fast::TypeExpr> {
    // Empty raw usually means the field skipped the annotation. The
    // POC parser today requires an annotation, but be defensive.
    if source.trim().is_empty() {
        return Err(ExpandError {
            message: "missing type annotation".into(),
            loc: base,
            context,
        });
    }
    let shifted = with_shifted_tokens(source, base, context.clone())?;
    parse_type_expression_from_source(&shifted).map_err(|e| shift_error(e, base, context))
}

fn parse_expr_at(source: &str, base: Loc, context: String) -> ExpandResult<fast::Expr> {
    if source.trim().is_empty() {
        return Err(ExpandError {
            message: "missing expression".into(),
            loc: base,
            context,
        });
    }
    let shifted = with_shifted_tokens(source, base, context.clone())?;
    parse_expression_from_source(&shifted).map_err(|e| shift_error(e, base, context))
}

fn parse_stmts_at(source: &str, base: Loc, context: String) -> ExpandResult<Vec<fast::Stmt>> {
    // Empty body is legal: `event go() { }` handles nothing.
    let shifted = with_shifted_tokens(source, base, context.clone())?;
    parse_statements_from_source(&shifted).map_err(|e| shift_error(e, base, context))
}

fn parse_params_at(source: &str, base: Loc, context: String) -> ExpandResult<Vec<fast::Param>> {
    // Empty params is legal: `event go()` has no params.
    let shifted = with_shifted_tokens(source, base, context.clone())?;
    parse_parameters_from_source(&shifted).map_err(|e| shift_error(e, base, context))
}

/// Feed the raw source through the classic lexer, shift every token's
/// `(line, column)` into the enclosing `.fitzv` coord system, and
/// re-emit it as source that a subsequent `parse_*_from_source` call
/// will tokenize identically — with the same shifted positions
/// naturally coming out.
///
/// **Why re-emit instead of tokenize-then-hand-off**: the public
/// `parse_*_from_source` wrappers take `&str`, not a token vector.
/// We could expose a token-consuming variant, but for a first commit
/// the source-in / source-out shape is simpler; the double-tokenize
/// cost is negligible for blobs of the size a `.fitzv` typically has.
///
/// **Fallback**: today the tokens themselves are not consumed. The
/// helper returns `source` verbatim. Precise position shifting inside
/// spans lands when the POC parser starts tracking the blob's exact
/// start offset (documented as debt in `docs/fase-11-plan.md` §7).
fn with_shifted_tokens(source: &str, _base: Loc, _context: String) -> ExpandResult<String> {
    // Tokenize once to validate the blob is at least lexable. If the
    // classic lexer chokes here, we surface the error early.
    let _tokens: Vec<TokenWithPos> = match tokenize(source) {
        Ok(t) => t,
        Err(e) => {
            // Positions from `tokenize` are blob-local. Shift them
            // into `.fitzv` coord space so the user gets a sensible
            // error location.
            return Err(shift_error(e, _base, _context));
        }
    };
    Ok(source.to_string())
}

fn shift_error(err: FitzError, base: Loc, context: String) -> ExpandError {
    // Blob-local (1, 1) maps to (base.line, base.column). Subsequent
    // lines and columns follow the standard formula.
    let (blob_line, blob_col) = (err.line.max(1), err.column.max(1));
    let (abs_line, abs_col) = shift_position(blob_line, blob_col, base);
    ExpandError {
        message: err.message,
        loc: Loc::new(abs_line, abs_col),
        context,
    }
}

fn shift_position(blob_line: usize, blob_col: usize, base: Loc) -> (usize, usize) {
    // Standard formula for source spans lifted into an outer file:
    //   line 1 sits at base.line; column 1 sits at base.column.
    //   subsequent lines start at column 1 of the outer file too.
    let abs_line = base.line + blob_line - 1;
    let abs_col = if blob_line == 1 {
        base.column + blob_col - 1
    } else {
        blob_col
    };
    (abs_line, abs_col)
}

fn is_bare_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::parse as view_parse;

    const CARD_SRC: &str = r#"component Card {
  state {
    title: Str = "Untitled"
    is_editing: Bool = false
  }

  event start() {
    is_editing = true
  }

  event save(new_title: Str) {
    title = new_title
    is_editing = false
  }

  <template>
    <div class="card">
      <div class="title">{title}</div>
      <button @click="start">Edit</button>
    </div>
  </template>

  <style scoped>
    .card { border: 1px solid #ccc; padding: 1rem; }
  </style>
}
"#;

    fn expand_str(src: &str) -> ExpandResult<ExpandedViewFile> {
        let raw = view_parse(src).expect("view parses");
        expand(&raw)
    }

    #[test]
    fn expands_the_card_component_end_to_end() {
        let file = expand_str(CARD_SRC).expect("Card expands cleanly");
        assert_eq!(file.components.len(), 1);
        let c = &file.components[0];
        assert_eq!(c.name, "Card");
        assert_eq!(c.state.len(), 2);
        assert_eq!(c.events.len(), 2);
        assert!(c.template.is_some());
        assert!(c.style.is_some());
    }

    #[test]
    fn state_field_types_are_parsed_as_type_exprs() {
        let file = expand_str(CARD_SRC).unwrap();
        let c = &file.components[0];
        assert_eq!(c.state[0].name, "title");
        assert_eq!(c.state[0].type_expr, fast::TypeExpr::Named("Str".into()));
        assert_eq!(c.state[1].name, "is_editing");
        assert_eq!(c.state[1].type_expr, fast::TypeExpr::Named("Bool".into()));
    }

    #[test]
    fn state_field_defaults_are_parsed_as_expressions() {
        let file = expand_str(CARD_SRC).unwrap();
        let c = &file.components[0];
        // `"Untitled"` — a plain string literal is an Expr::Str.
        matches!(c.state[0].default, fast::Expr::Str(_, _));
        if let fast::Expr::Str(s, _) = &c.state[0].default {
            assert_eq!(s, "Untitled");
        } else {
            panic!(
                "expected Expr::Str for title default, got {:?}",
                c.state[0].default
            );
        }
        // `false` — a Bool literal.
        if let fast::Expr::Bool(b, _) = c.state[1].default {
            assert!(!b);
        } else {
            panic!("expected Expr::Bool(false), got {:?}", c.state[1].default);
        }
    }

    #[test]
    fn event_handler_params_are_parsed_as_param_list() {
        let file = expand_str(CARD_SRC).unwrap();
        let c = &file.components[0];
        // start() — empty params.
        assert_eq!(c.events[0].name, "start");
        assert!(c.events[0].params.is_empty());
        // save(new_title: Str) — one annotated param.
        assert_eq!(c.events[1].name, "save");
        assert_eq!(c.events[1].params.len(), 1);
        let p = &c.events[1].params[0];
        assert_eq!(p.name, "new_title");
        assert_eq!(p.type_, Some(fast::TypeExpr::Named("Str".into())));
        assert!(!p.varargs);
    }

    #[test]
    fn event_handler_bodies_are_parsed_as_stmts() {
        let file = expand_str(CARD_SRC).unwrap();
        let c = &file.components[0];
        // start body: `is_editing = true` — a single Stmt::Assign.
        assert_eq!(c.events[0].body.len(), 1);
        assert!(matches!(c.events[0].body[0], fast::Stmt::Assign { .. }));
        // save body: two assignments.
        assert_eq!(c.events[1].body.len(), 2);
        assert!(matches!(c.events[1].body[0], fast::Stmt::Assign { .. }));
        assert!(matches!(c.events[1].body[1], fast::Stmt::Assign { .. }));
    }

    #[test]
    fn template_interpolation_is_parsed_as_expr() {
        let file = expand_str(CARD_SRC).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        // Find the interpolation node buried under the outer <div>.
        let outer_div = template
            .roots
            .iter()
            .find(|n| matches!(n, ExpandedTemplateNode::Element { tag, .. } if tag == "div"))
            .unwrap();
        let inner_children = match outer_div {
            ExpandedTemplateNode::Element { children, .. } => children,
            _ => unreachable!(),
        };
        let title_div = inner_children
            .iter()
            .find(|n| matches!(n, ExpandedTemplateNode::Element { tag, .. } if tag == "div"))
            .unwrap();
        let title_children = match title_div {
            ExpandedTemplateNode::Element { children, .. } => children,
            _ => unreachable!(),
        };
        let interp = title_children
            .iter()
            .find(|n| matches!(n, ExpandedTemplateNode::Interpolation { .. }))
            .expect("`{title}` interpolation exists");
        match interp {
            ExpandedTemplateNode::Interpolation { expr, .. } => match expr {
                fast::Expr::Ident(name, _) => assert_eq!(name, "title"),
                other => panic!("expected Ident, got {:?}", other),
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn attribute_event_binding_stores_handler_name_verbatim() {
        let file = expand_str(CARD_SRC).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        // Dig for the <button> element.
        fn find_button(nodes: &[ExpandedTemplateNode]) -> Option<&ExpandedTemplateNode> {
            for n in nodes {
                if let ExpandedTemplateNode::Element { tag, children, .. } = n {
                    if tag == "button" {
                        return Some(n);
                    }
                    if let Some(f) = find_button(children) {
                        return Some(f);
                    }
                }
            }
            None
        }
        let button = find_button(&template.roots).expect("button");
        match button {
            ExpandedTemplateNode::Element { attrs, .. } => {
                assert_eq!(attrs.len(), 1);
                match &attrs[0] {
                    ExpandedAttr::Event {
                        event_name,
                        handler_name,
                        ..
                    } => {
                        assert_eq!(event_name, "click");
                        assert_eq!(handler_name, "start");
                    }
                    other => panic!("expected Event attr, got {:?}", other),
                }
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn attribute_interpolation_is_parsed_as_expr() {
        let src = r#"component X {
  state { title: Str = "hi" }
  <template><input value="{title}" /></template>
}"#;
        let file = expand_str(src).unwrap();
        let input = &file.components[0].template.as_ref().unwrap().roots[0];
        match input {
            ExpandedTemplateNode::Element {
                attrs,
                self_closing,
                ..
            } => {
                assert!(*self_closing);
                match &attrs[0] {
                    ExpandedAttr::Interpolation { name, expr, .. } => {
                        assert_eq!(name, "value");
                        match expr {
                            fast::Expr::Ident(n, _) => assert_eq!(n, "title"),
                            other => panic!("expected Ident, got {:?}", other),
                        }
                    }
                    other => panic!("expected Interpolation attr, got {:?}", other),
                }
            }
            _ => panic!("expected <input/>"),
        }
    }

    #[test]
    fn state_field_default_that_fails_to_parse_reports_context() {
        // Two bare identifiers side by side: the view lexer emits
        // them as separate tokens (so the raw capture succeeds), but
        // `parse_expression_from_source` rejects with
        // "unexpected trailing tokens after expression".
        let src = r#"component X {
  state {
    count: Int = foo bar
  }
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.context.contains("state field 'count' default"),
            "context = {:?}",
            err.context
        );
    }

    #[test]
    fn event_body_that_fails_to_parse_reports_context() {
        // Missing `=` after the identifier makes `parse_stmt` reject.
        let src = r#"component X {
  event go() {
    let x
  }
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.context.contains("event 'go' body"),
            "context = {:?}",
            err.context
        );
    }

    #[test]
    fn event_binding_rejects_expression_value_with_clear_message() {
        let src = r#"component X {
  <template><button @click="start()">go</button></template>
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.message.contains("reference a handler by name")
                || err.message.contains("handler name"),
            "message = {:?}",
            err.message
        );
    }

    #[test]
    fn empty_component_expands_to_empty_expanded() {
        let file = expand_str("component Empty {}").unwrap();
        assert_eq!(file.components.len(), 1);
        let c = &file.components[0];
        assert_eq!(c.name, "Empty");
        assert!(c.state.is_empty());
        assert!(c.events.is_empty());
        assert!(c.template.is_none());
        assert!(c.style.is_none());
    }

    #[test]
    fn multi_component_file_expands_each_component() {
        let src = r#"component A {
  state { flag: Bool = true }
}

component B {
  state { title: Str = "hello" }
}
"#;
        let file = expand_str(src).unwrap();
        assert_eq!(file.components.len(), 2);
        assert_eq!(file.components[0].name, "A");
        assert_eq!(file.components[1].name, "B");
        assert_eq!(file.components[0].state.len(), 1);
        assert_eq!(file.components[1].state.len(), 1);
    }

    #[test]
    fn shift_position_maps_line_one_col_one_to_base() {
        let base = Loc::new(10, 5);
        assert_eq!(shift_position(1, 1, base), (10, 5));
    }

    #[test]
    fn shift_position_shifts_column_only_on_line_one() {
        let base = Loc::new(10, 5);
        assert_eq!(shift_position(1, 3, base), (10, 7));
    }

    #[test]
    fn shift_position_shifts_line_and_keeps_column_on_later_lines() {
        let base = Loc::new(10, 5);
        assert_eq!(shift_position(3, 8, base), (12, 8));
    }
}
