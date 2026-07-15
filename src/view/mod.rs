// view/ — Phase 11 POC: Single-File Components (`.fitzv`).
//
// **Isolated module.** The classic `.fitz` parser does not touch
// this code, and vice versa. A bug here CANNOT break the classic
// pipeline (Invariant 4 of `docs/stack.md`).
//
// Current status: **parser POC**. Recognises the shape of a
// component and produces its own AST in `view::ast`. Does NOT
// evaluate, does NOT type-check, does NOT emit code. The full
// Phase 11 plan (why the extension is `.fitzv`, how the module
// connects with the checker + codegen, how it evolves toward
// SSR/WASM) lives in `docs/fase-11-plan.md`.
//
// `mod view` is declared in `src/lib.rs` as `pub mod view` plain
// (no feature gate) because:
//   - It adds zero new deps to `Cargo.toml`.
//   - The `fitz` binary does not dispatch to this module today —
//     only tests + external tooling can call `view::parse(...)`.
//   - A feature gate would add friction to the smoke without any
//     upside at this POC stage.
//
// Sub-modules:
//   - `ast`    — SFC AST types
//   - `lexer`  — dedicated tokenizer (`.fitzv` is its own dialect)
//   - `parser` — recursive parser + HTML sub-parser for `<template>`

pub mod ast;
pub mod check;
pub mod codegen_ssr;
pub mod codegen_wasm;
pub mod css_parser;
pub mod expand;
pub mod lexer;
pub mod parser;
pub mod wasm_build;

pub use check::{check, CheckError};
pub use codegen_ssr::{emit_component_ssr, emit_module_ssr, SsrEmitError, SsrEmitResult};
pub use codegen_wasm::{emit_component, emit_module, EmitError, EmitResult};
pub use css_parser::{apply_scope, CssParseError};
pub use expand::{
    expand, ExpandError, ExpandResult, ExpandedAttr, ExpandedComponent, ExpandedEventHandler,
    ExpandedStateField, ExpandedStyle, ExpandedTemplate, ExpandedTemplateNode, ExpandedViewFile,
};
pub use parser::{parse, ViewParseError, ViewParseResult};
pub use wasm_build::{
    compose_cargo_toml, compose_lib_rs, sanitise_wasm_pkg_name, write_wasm_crate_scaffold,
    ScaffoldError, ScaffoldResult,
};
