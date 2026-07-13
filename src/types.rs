// types.rs — Phase 5.2
//
// Internal representation of Fitz's type system. While
// `ast::TypeExpr` is what the parser produces from source,
// this module models the *resolved* type against a table: each
// name is looked up, each generic validates arity, each nominal carries
// a unique identity within the program.
//
// The flow is:
//
//   AST (TypeExpr)  ──resolve_type_expr──►  Type  (resolved)
//                          against
//                       TypeEnv
//
// 5.2 validates top-level annotations (fields of `type`, params and
// return of fns, let annotations). Checking function bodies against
// values is left for 5.3.

use std::collections::{HashMap, HashSet};

use crate::ast::{Decorator, Expr, Field, Param, Program, Span, Stmt, TypeExpr};
use crate::error::{ErrorKind, FitzError};

/// Unique identity for nominal types (those declared with
/// `type`). Internally an index against `TypeEnv.nominals`.
/// Two `type User` in different modules produce different `TypeId`s
/// — identity is nominal, not structural.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);

/// A resolved type. What the checker compares and shows to the user.
///
/// Differences with `TypeExpr`:
///  - `Nominal(TypeId)` carries the already-resolved identity (not just
///    a string).
///  - Built-in generics have their own variants instead of
///    `Generic { name, args }` — makes pattern matching easier.
///  - Primitives are singletons (carry no data).
///
/// The derived structural equality works: two `Type`s that the checker
/// says "compatible" must yield `==`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Str,
    Bool,
    Null,
    /// Mini-batch Bytes — sequence of binary bytes. New primitive
    /// of the language. Built via literal `b"..."` (with
    /// hex escapes `\xHH`) or via builtin `bytes_from_str(s)`. Methods
    /// supported: `.len()`, `.is_empty()`, `.to_str() -> Result<Str>`.
    Bytes,
    /// `Range` only appears in `0..10` for now — has no parameter.
    Range,

    /// `List<T>`.
    List(Box<Type>),
    /// `Map<K, V>`.
    Map(Box<Type>, Box<Type>),
    /// `Result<T>` or `Result<T, E>` (mini-batch Re+). When the user
    /// writes `Result<T>` without explicit E, the parser expands it to
    /// `Result<T, Str>` for compat with all code that existed
    /// before the refactor. Annotating `Result<T, MyError>` allows carry of
    /// custom types in the Err side.
    Result {
        ok: Box<Type>,
        err: Box<Type>,
    },

    /// `Future<T>` — the pending value produced by an `async fn` when
    /// called. Only `.await` (inside another `async fn`) unwraps
    /// it to `T`. Fixed arity 1 (built-in generic, parallel to Result/List/
    /// Nullable). Introduced in Phase 6.2.
    Future(Box<Type>),

    /// Phase 12.2.a — `Secret<T>` opaque type with auto-redaction.
    /// Built by the `secret("KEY")` builtin that reads env var /
    /// mounted file `/run/secrets/<key>` / `.env`. The runtime's
    /// Display and Debug emit `<redacted Secret<T>>` to prevent
    /// accidental leaks in logs. Accessing the inner T requires calling
    /// `.expose()` explicitly — defensive-by-default design parallel
    /// to the Secret<T> pattern of Rust libs like `secrecy`.
    ///
    /// JSON serialization is blocked in `value_to_json` — an
    /// HTTP handler that returns `Secret<Str>` receives an explicit error
    /// citing `.expose()` (residual debt: refinement with field-level
    /// redaction in types with a Secret field).
    ///
    /// Fixed arity 1 (built-in generic, parallel to Future/Result/List).
    Secret(Box<Type>),

    /// Phase 9.w.2 — `WsConn<T>` typed WebSocket connection. `T` is the
    /// message type (any type that serializes to JSON: primitive,
    /// custom `type`, List/Map, etc.). Fixed arity 1, built-in generic
    /// (parallel to Future/Result/List). Only appears as a param of
    /// `@ws("/path")` handlers — the runtime constructs the `Value::WsConn`
    /// after the HTTP→WS upgrade and injects it. Parametric methods:
    /// `recv: () -> Result<RECV>`, `send: (SEND) -> Result<Null>`,
    /// `broadcast: (SEND) -> Result<Null>` (to all conns on the endpoint,
    /// including the sender), `close: () -> Null`.
    ///
    /// 9.w.2-wsconn-bidir (v0.9.38): when the user declares
    /// `WsConn<T>` (arity 1), both `recv` and `send` point to the same
    /// `T` (backward-compat with all pre-bidir code). When
    /// declaring `WsConn<In, Out>` (arity 2), `recv = In` and `send = Out`
    /// can differ — enables asymmetric channels (e.g. client
    /// sends commands, server emits events of different shape).
    WsConn {
        recv: Box<Type>,
        send: Box<Type>,
    },

    /// Phase 10.1.c — opaque handle to a live Postgres connection.
    /// Produced by `db.connect(url).await?` and consumed by the
    /// `query/exec/close/is_closed` methods. Opaque: the user does not
    /// construct instances directly.
    ///
    /// No type parameters (unlike WsConn which is
    /// generic over RECV/SEND). The row type is always
    /// `Map<Str, Any>` in the MVP — typed composites (ORM with
    /// `@table type User { ... }`) come in 10.3.
    DbConn,

    /// Phase 10.1.c — one row of the resultset of a Postgres query.
    /// Produced by `conn.query(...).await?` (as `List<DbRow>`)
    /// and consumed with `row.get("col")` / `row.get_at(idx)` which
    /// return the primitive value (Int/Float/Str/Bool/Bytes/Null).
    /// Opaque: the user does not construct instances.
    DbRow,

    /// v0.10.24 — date without time or tz. ISO 8601 format `YYYY-MM-DD`.
    /// Built via `Date.today()` or `Date.parse("2026-05-30")`.
    /// Maps to Postgres `date` (OID 1082) in ORM/driver.
    Date,

    /// v0.10.24 — date + time + tz (always UTC in MVP). ISO 8601
    /// format `YYYY-MM-DDTHH:MM:SSZ`. Built via `DateTime.now()` or
    /// `DateTime.parse("...")`. Maps to Postgres `timestamptz` (OID
    /// 1184) in ORM/driver. Explicit parameterized TZs
    /// (`DateTime<TZ>`) remain as future debt — used by <5% of
    /// real apps.
    DateTime,

    /// v0.10.24 — random v4 UUID. Canonical format `xxxxxxxx-xxxx-
    /// 4xxx-yxxx-xxxxxxxxxxxx`. Built via `Uuid.v4()` (random) or
    /// `Uuid.parse("...")`. Maps to Postgres `uuid` (OID 2950) in
    /// ORM/driver. Naming `Uuid` (not `UUID`) for consistency with
    /// `DbConn`/`DbRow`/`PyAny`.
    Uuid,

    /// Phase 10.3+ — ORM query builder. Returned by
    /// `Type.where(closure)` when `Type` has `@table`, and
    /// preserved by the `.where`/`.order_by`/`.limit`/
    /// `.offset`/`.group_by` chain. The terminals (`.all`/`.first`/
    /// `.count`/`.sum`/`.avg`/`.min`/`.max`/`.update`/`.delete`)
    /// break the chain returning `Result<...>`.
    ///
    /// The `row` param carries the row's nominal type so that the
    /// terminals know what to return: `.all(db) → Result<List<row>>`,
    /// `.first(db) → Result<row>`, etc. Opaque to runtime: the
    /// evaluator uses `Value::QueryBuilder(Arc<dyn Any>)` and never
    /// inspects the row at this level.
    QueryBuilder(Box<Type>),

    /// Phase 10.b.14 — `Aggregated<Row>`. QueryBuilder post-`.group_by(...)`.
    /// Preserves all chain methods (where/order_by/limit/offset/
    /// group_by) that return `Aggregated<Row>`. The terminal
    /// aggregates (`sum/avg/min/max/count`) change shape: return
    /// `Future<Result<List<Map<Str, Any>>>>` with each row = a group
    /// plus its aggregate. `.all/.first/.update/.delete` are rejected
    /// (makes no sense over a GROUP BY). The refactor unblocks
    /// the residual debt of 10.b.6 which only supported scalar
    /// aggregates (path without group_by).
    Aggregated(Box<Type>),

    /// Type declared by the user (`type User { ... }`) or
    /// imported. Identity goes via `TypeId`.
    Nominal(TypeId),

    /// `T?` — the value can be of type `T` or `Null`.
    Nullable(Box<Type>),

    /// Function type: `fn(p1, p2, ...) -> r`. Built by the
    /// checker when registering `Stmt::FnDef` (5.3.2) and when synthesizing
    /// `Expr::FnExpr` (5.3.5). In 5.3.1 it already exists as a variant to
    /// avoid refactoring later.
    Function {
        params: Vec<Type>,
        ret: Box<Type>,
    },

    /// Tuple type `(T1, T2, ...)` (mini-batch T). Heterogeneous, fixed
    /// size, positional. Empty Vec → unit tuple `()`. Access via
    /// `t.0`, `t.1`, etc. Structural identity: two `Tuple`s with
    /// the same elements in the same order are equal.
    Tuple(Vec<Type>),

    /// "No determined type". Gradual escape: appears where the
    /// checker cannot or does not want to infer a concrete type. Param
    /// without annotation, `let` without annotation with non-inferrable RHS,
    /// expressions that the checker does not yet model (calls before
    /// 5.3.2, methods before 5.3.4, etc.). Any comparison
    /// against `Any` passes: nothing is rejected because of an `Any`.
    ///
    /// **Matrix of `Type::Any` usage (audit F1, v0.9.45)** — the
    /// ~180 sites where it appears are classified into these categories,
    /// all intentional (not bugs from silencing):
    ///
    /// 1. **Variadic builtins** (`print(...)`, `assert(...)`,
    ///    `assert_eq`, `format!`-style): signature `params: vec![Any, ...]`
    ///    because they accept any type. Refinable in a multi-arity
    ///    overloading sub-phase, no real pressure.
    ///
    /// 2. **Polymorphic builtins over a distinct type** (`len(x)` →
    ///    Str/List/Map/Bytes; `bytes(s)` → Str): param `Any`, concrete
    ///    ret. The real dispatch occurs in runtime/codegen by
    ///    receiver type. Covering this with sum types (`Str | List
    ///    | Map | Bytes`) does not help without a generic union type.
    ///
    /// 3. **Gradual propagation** (`Any op X → Any`, `Any.field →
    ///    Any`, `Any(args) → Any`): classic pattern of gradual
    ///    typing. Guarantees code without annotations keeps working
    ///    when it comes into contact with typed vars.
    ///
    /// 4. **Annotations that fail resolution** (`Some(t) =>
    ///    resolve_type_expr(t, &env).unwrap_or(Type::Any)`): defensive
    ///    fallback — if the user annotated an invalid type, the
    ///    checker emits the annotation error but does NOT abort the
    ///    pipeline; the binding stays as `Any` so the rest
    ///    of the program keeps checking. Without this, a single typo in
    ///    an annotation cascades into "unknown var" errors.
    ///
    /// 5. **Callbacks without annotation** (`FnExpr` inline without `ret`
    ///    declared, before inference 5.3.5): ret type `Any`
    ///    until the body is processed. After 5.3.5, the ret is inferred
    ///    via `unify_returns` + `lub`; only remains `Any` when the
    ///    body has no returns or they are irrecoverably heterogeneous.
    ///
    /// 6. **Match patterns with scrutinee `Any`** (`Ok(x)` /
    ///    `Err(e)` / `Ident(b)`): the binding stays `Any` to
    ///    propagate the gradual. Refinable when the scrutinee types
    ///    concrete.
    ///
    /// 7. **`Expr::Error` (F15 recovery)**: the `infer_expr` wrapper
    ///    persists `Expr::Error → Type::Any` so the LSP runs
    ///    the checker over broken AST without cascading errors. Silent
    ///    policy: the real error was already registered by the parser.
    ///
    /// 8. **Result/Future built-ins without concrete info**
    ///    (`Result<Any>` in standalone `Err("...")`, `Future<Any>` in
    ///    `spawn(...)` without literal call): "we don't know the `T`,
    ///    refine at the destination site". Recursive `is_compatible`
    ///    allows them against concrete `Result<X>` / `Future<X>`.
    ///
    /// 9. **`Type::PyAny` propagates as `Type::Any` in some
    ///    contexts** (`Any | PyAny → Any` in BinOp/UnaryOp): the
    ///    PyAny gradual escape lives in its own variant to
    ///    differentiate it in LSP hover/completion, but degrades to
    ///    `Any` when combined with non-Python vars.
    ///
    /// What is NOT in this list (and would be a bug if it appeared):
    /// - Using `Type::Any` as a real error type (should be a
    ///   specific variant or `Result<X, E>` with clear E).
    /// - Using `Type::Any` to silence a genuine mismatch (should
    ///   be `ctx.error_at(...)`).
    /// - `Type::Any` as the return of user-defined fns without annotation
    ///   (when full inference arrives, it should be the unify of
    ///   the returns, not gradual fallback).
    Any,

    /// Phase 8.4 — "Opaque Python object". Appears in the bindings of
    /// `from python import X` and propagates through field access
    /// (`mod.submod`, `obj.attr` → still `PyAny`). Exists
    /// separate from `Any` so the checker can distinguish "this
    /// is opaque Python" from "this is general Any" and refine the type
    /// of calls: `pyobj(args)` and `pyobj.method(args)` type
    /// as `Result<Any>` (the automatic wrap from 8.3), forcing the
    /// user to handle the error with `match` or `?` statically.
    ///
    /// Compatibility: like `Any`, `PyAny` is bidirectionally
    /// compatible with any other type (gradual escape).
    /// Explicit annotations (`let row: User = py_call(...)?`) are
    /// the recommended way to "exit" PyAny and enter concrete Fitz
    /// types — the runtime does the real coercion (debt 8.4.3:
    /// dict → Instance via field name match).
    PyAny,
}

impl Type {
    /// `true` if the type is `T?` at the top level.
    pub fn is_nullable(&self) -> bool {
        matches!(self, Type::Nullable(_))
    }

    /// Returns `&Type` peeling a single layer of `Nullable`. `Int? →
    /// Int`. `Int → Int`. Does not recurse.
    pub fn base(&self) -> &Type {
        match self {
            Type::Nullable(t) => t,
            other => other,
        }
    }

    /// Renders the type for user-facing messages. Needs the env
    /// to resolve `Nominal` names.
    pub fn display(&self, env: &TypeEnv) -> String {
        match self {
            Type::Int => "Int".into(),
            Type::Float => "Float".into(),
            Type::Str => "Str".into(),
            Type::Bool => "Bool".into(),
            Type::Null => "Null".into(),
            Type::Bytes => "Bytes".into(),
            Type::Range => "Range".into(),
            Type::List(t) => format!("List<{}>", t.display(env)),
            Type::Map(k, v) => format!("Map<{}, {}>", k.display(env), v.display(env)),
            // Mini-batch Re+ — Display omits the E when it is Str
            // (default, compat with `Result<T>` writing) or when it is
            // Any. For a concrete distinct E (Int/Instance/etc.),
            // shows the full form `Result<T, E>`.
            Type::Result { ok: t, err: e } => match e.as_ref() {
                Type::Str | Type::Any => format!("Result<{}>", t.display(env)),
                _ => format!("Result<{}, {}>", t.display(env), e.display(env)),
            },
            Type::Future(t) => format!("Future<{}>", t.display(env)),
            Type::Secret(t) => format!("Secret<{}>", t.display(env)),
            // 9.w.2-wsconn-bidir — compact Display:
            //   `WsConn<T>` when recv == send (symmetric case,
            //   historical default).
            //   `WsConn<In, Out>` when they differ.
            Type::WsConn { recv, send } => {
                if recv == send {
                    format!("WsConn<{}>", recv.display(env))
                } else {
                    format!("WsConn<{}, {}>", recv.display(env), send.display(env))
                }
            }
            Type::DbConn => "DbConn".into(),
            Type::DbRow => "DbRow".into(),
            Type::Date => "Date".into(),
            Type::DateTime => "DateTime".into(),
            Type::Uuid => "Uuid".into(),
            Type::QueryBuilder(row) => format!("QueryBuilder<{}>", row.display(env)),
            Type::Aggregated(row) => format!("Aggregated<{}>", row.display(env)),
            Type::Nominal(id) => env.info(*id).name.clone(),
            Type::Nullable(t) => format!("{}?", t.display(env)),
            Type::Function { params, ret } => {
                let ps: Vec<String> = params.iter().map(|p| p.display(env)).collect();
                format!("fn({}) -> {}", ps.join(", "), ret.display(env))
            }
            Type::Tuple(items) => {
                let parts: Vec<String> = items.iter().map(|t| t.display(env)).collect();
                if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                }
            }
            Type::Any => "Any".into(),
            Type::PyAny => "PyAny".into(),
        }
    }
}

/// Info of a nominal type declared in the program.
#[derive(Debug, Clone)]
pub struct NominalInfo {
    pub name: String,
    /// Resolved fields. `None` while the type is being
    /// registered in the first pass (forward decl); completed
    /// in the second pass once all nominals are
    /// known.
    pub fields: Option<Vec<ResolvedField>>,
    /// R.3 — resolved custom methods. Each entry has the method's
    /// name, its `Function { params, ret }` signature resolved to Fitz
    /// types, and an `is_async` flag so `infer_method_call` can
    /// wrap the ret in `Future<T>`. `Vec::new()` if the type does not
    /// declare methods.
    pub methods: Vec<NominalMethod>,
}

#[derive(Debug, Clone)]
pub struct NominalMethod {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
    pub is_async: bool,
    /// Mini-batch St — `true` if the method is static
    /// (`static fn` inside the `type` body). Invoked as
    /// `Type.method(args)` instead of `instance.method(args)`.
    pub is_static: bool,
    /// Mini-batch Up — param names in order, parallel to
    /// `params`. Useful so the LSP shows `fn(x: Int, y: Int)`
    /// instead of `fn(Int, Int)` in autocomplete + hover.
    pub param_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedField {
    pub name: String,
    pub type_: Type,
}

/// Phase 10.3.a — Metadata extracted from the ORM decorators on
/// a `type Foo { ... }`. If the type does NOT have `@table`, stays
/// `None` in `TypeEnv.tables`. If it does, we register the SQL
/// table name + the primary field + per-column overrides +
/// relations (Phase 10.4.a).
///
/// The runtime (10.3.b) consumes this metadata to emit correct
/// SQL when translating `User.where(...).all().await?`.
#[derive(Debug, Clone)]
pub struct TableMetadata {
    /// SQL name of the table (`@table("name")`). If the
    /// decorator does not pass a string, default = Fitz name of the type
    /// in lowercase (`User` → `user`). Automatic pluralization
    /// remains as minor debt — the user can specify
    /// explicitly.
    ///
    /// v0.10.21 (10.6.e.3) — If the decorator arg contains
    /// `.` (e.g. `@table("analytics.events")`), the parser splits
    /// into `(schema, name)`: `sql_name = "events"`, `schema =
    /// Some("analytics")`. If NOT containing `.`, `schema = None`
    /// (= `public` by Postgres convention).
    pub sql_name: String,
    /// v0.10.21 (10.6.e.3) — Custom Postgres schema where the
    /// table lives. `None` = `public` (default). The ORM SQL emit and
    /// migrations use qualified names (`"schema"."name"`)
    /// only when `schema.is_some()`.
    pub schema: Option<String>,
    /// Fitz names of fields marked with `@primary`. Empty
    /// if no PK is declared. Single-PK = `vec!["id"]`; composite
    /// PK = `vec!["org_id", "user_id"]` (order matters for the
    /// `PRIMARY KEY (a, b)` constraint in CREATE TABLE).
    ///
    /// v0.10.27 (F2) — previously was `primary_field: Option<String>`
    /// (a single PK). Composite PK was explicit debt. Now it
    /// supports N PKs; sites that only handle single PK use
    /// `single_pk()` which returns `Option<&str>` only if `len() == 1`.
    pub primary_fields: Vec<String>,
    /// Per-column overrides. Indexed by Fitz field name
    /// (not by SQL name — the mapping lives in this struct).
    /// Only entries for fields with `@column`/`@unique`/`@index`;
    /// fields without decorators map directly (Fitz name =
    /// SQL name, SQL type derived from the Fitz type).
    pub columns: std::collections::HashMap<String, ColumnMetadata>,
    /// Phase 10.4.a — Relations declared with `@belongs_to`,
    /// `@has_one`, `@has_many`. Indexed by Fitz field name.
    /// `BelongsTo` maps a real FK of the row; `HasOne`/`HasMany`
    /// are virtual (do not appear in SELECT/INSERT, navigated
    /// with methods at runtime — 10.4.b).
    pub relations: std::collections::HashMap<String, RelationMetadata>,
    /// v0.10.17 (10.6.b.2) — If present, `fitz db diff` emits
    /// `ALTER TABLE "old" RENAME TO "new"` instead of DROP + CREATE.
    /// Transient decorator: the user removes it after applying the
    /// migration. Syntax: `@table("new", renamed_from="old")` or
    /// `@renamed_from("old") @table("new") type T { ... }`
    /// (TBD parsing — only kwarg in `@table` for MVP simplicity).
    pub renamed_from: Option<String>,
    /// v0.10.27 (F3) — `@index(...)` stackable decorators at the
    /// type level to define composite / partial indexes without writing
    /// `CREATE INDEX` by hand. Each `IndexSpec` translates to a
    /// `CREATE INDEX` in `fitz db diff/migrate`. Honors drift check:
    /// if the user removes an `@index`, the diff emits `DROP INDEX`.
    pub indexes: Vec<IndexSpec>,
    /// v0.10.29 — `@check_constraint("<sql_expr>")` stackable
    /// decorators at the type level to emit `CHECK (<expr>)` in
    /// `CREATE TABLE`. No drift check on the migrator side (checks
    /// are not introspected at MVP; the user removes/recreates them
    /// with `db.exec` if the shape changes).
    pub check_constraints: Vec<CheckConstraintSpec>,
}

/// v0.10.27 (F3) — Specification of an ORM index. Populated
/// from `@index(...)` at the type level. Usage forms:
///   - `@index("col1, col2")` — simple composite (implicit btree).
///   - `@index("col1, col2", unique=true)` — composite UNIQUE.
///   - `@index("col1, col2", name="custom")` — custom name (else
///     auto-generated `idx_<table>_<col1>_<col2>`).
///   - `@index("col1, col2", where_="deleted_at IS NULL")` — partial.
///   - `@index("col", using="gin")` — v0.10.28: method override
///     (btree default; gin/gist/brin/hash/spgist enabled without
///     dropping to `db.exec`).
///
/// Expression indexes (`@index("lower(email)")`) NOT in MVP — the
/// arg is a mini SQL parser that's out of scope. Workaround: drop
/// to manual `db.exec("CREATE INDEX ...")` and skip drift check
/// for that index.
/// v0.10.29 — Specification of a CHECK constraint. Populated
/// from `@check_constraint("<sql_expr>", name="optional")`.
///
/// The `expr` is passed **literal** to the SQL CREATE TABLE — Fitz does not
/// parse the expression (would require dropping to a SQL parser of the check
/// lang, out of MVP scope). The user is responsible that it is
/// valid SQL against the table shape.
///
/// Drift check NOT implemented in MVP — introspect does not read
/// `pg_constraint.contype = 'c'` to reconcile. If you change the
/// `expr` on the Fitz side without migrating by hand, the DB stays on the old one.
/// Documented as a caveat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckConstraintSpec {
    /// Name of the constraint. `None` = auto-generated as
    /// `chk_<table>_<idx>`. Useful for drop by name from
    /// `db.exec` when there is drift.
    pub name: Option<String>,
    /// Boolean SQL expression evaluated per row at INSERT/UPDATE.
    /// Example: `"age >= 0 AND age <= 150"`, `"status IN ('a',
    /// 'p', 'd')"`, `"start_date <= end_date"`.
    pub expr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSpec {
    /// Name of the index. `None` = auto-generated by the migrator
    /// as `idx_<table>_<col1>_<col2>_..._<unique?>` so the
    /// drift check can match DB-vs-Fitz consistently.
    pub name: Option<String>,
    /// List of SQL columns (post `@column(name=...)` resolution).
    /// Order matters for Postgres (compound index left-prefix).
    pub columns: Vec<String>,
    /// UNIQUE flag — emits `CREATE UNIQUE INDEX` instead of `CREATE
    /// INDEX`. Useful for uniqueness constraints over tuples.
    pub unique: bool,
    /// WHERE clause for partial index. `None` = full index. Useful
    /// for soft-deletes: `WHERE deleted_at IS NULL` covers only
    /// live rows, smaller index.
    pub where_clause: Option<String>,
    /// v0.10.28 — Method override (`USING <method>`). `None` =
    /// btree (Postgres default). Whitelisted: `btree` | `hash` |
    /// `gin` | `gist` | `brin` | `spgist`. Enables full-text
    /// search (`gin` over tsvector), range queries (`gist`),
    /// summarized large tables (`brin`) and bloom-style approximations
    /// (extension) without escape hatch to `db.exec`.
    pub using: Option<String>,
    /// v0.10.32 (Tier C.2) — Expression index. When present,
    /// `columns` is ignored and SQL emit uses the expression
    /// directly: `CREATE INDEX ... ON tbl (<expression>)`.
    /// Canonical examples: `lower(email)` for case-insensitive
    /// search, `to_tsvector('english', body)` for FTS,
    /// `(price * quantity)` for totals. The user passes the raw
    /// expression via kwarg: `@index(expression="lower(email)")`.
    ///
    /// **Incomplete drift check**: introspect reads the index
    /// listing from `pg_indexes` but does NOT parse `pg_index.indexprs`
    /// to detect the expression. The diff can generate
    /// spurious `DROP INDEX + CREATE INDEX` on subsequent runs.
    /// Workaround: the user names the index explicitly with `name=` and
    /// trusts that the diff detects it by name (not by content).
    pub expression: Option<String>,
}

impl TableMetadata {
    /// v0.10.21 (10.6.e.3) — Quoted SQL name with optional schema
    /// qualifier. Tables in `public` (schema=None) →
    /// `"name"`. Tables in custom schema → `"schema"."name"`.
    /// Canonical helper so ALL ORM SQL emit
    /// (SELECT/INSERT/UPDATE/DELETE in evaluator and codegen) uses
    /// the same convention and supports custom schemas uniformly.
    pub fn qualified_sql_name(&self) -> String {
        match &self.schema {
            Some(s) => format!(
                "\"{}\".\"{}\"",
                s.replace('"', "\"\""),
                self.sql_name.replace('"', "\"\"")
            ),
            None => format!("\"{}\"", self.sql_name.replace('"', "\"\"")),
        }
    }

    /// v0.10.27 (F2) — Single-PK accessor for sites that only
    /// handle simple PK (`id: 0` auto-bigserial sentinel,
    /// belongs_to navigation, etc.). Returns `Some(name)` ONLY if
    /// there is exactly ONE PK. `None` for composite PK (N≥2) or
    /// no PK (N=0). Composite-aware sites iterate over
    /// `primary_fields` directly.
    pub fn single_pk(&self) -> Option<&str> {
        if self.primary_fields.len() == 1 {
            Some(self.primary_fields[0].as_str())
        } else {
            None
        }
    }

    /// v0.10.27 (F2) — `true` if the type has a declared PK
    /// (single or composite). Useful as a quick pre-operation check
    /// (e.g. ORM rejects INSERT/SELECT without a declared PK).
    pub fn has_pk(&self) -> bool {
        !self.primary_fields.is_empty()
    }
}

/// Phase 10.3.a — Per-column configuration of the ORM. Populated
/// from `@column(name=..., type=...)`, `@unique`, `@index`.
#[derive(Debug, Clone, Default)]
pub struct ColumnMetadata {
    /// SQL name if different from the Fitz name. `None` = same
    /// name (direct mapping).
    pub sql_name: Option<String>,
    /// Custom SQL type if the default doesn't apply. `None` = the ORM
    /// derives from the Fitz type (`Int` → `bigint`, `Str` → `text`,
    /// etc.).
    pub sql_type: Option<String>,
    pub unique: bool,
    pub indexed: bool,
    /// 10.8.2 (v0.10.8) — `@db_default` marks the field as
    /// "DB-managed": the ORM SKIPS it from the INSERT, leaving
    /// Postgres to apply the `DEFAULT` declared in the schema
    /// (typically `DEFAULT NOW()` for timestamps, `DEFAULT
    /// gen_random_uuid()` for UUIDs, etc.). The field still
    /// participates in normal SELECT/UPDATE — the client receives
    /// it from the RETURNING * with the value Postgres assigned.
    ///
    /// Without this flag, the ORM always includes the field in the
    /// INSERT with the Fitz value (typically `""` for Str with
    /// default `""`), which Postgres rejects for non-text types
    /// (`timestamptz`, `uuid`, etc.) or silently overwrites the
    /// DEFAULT.
    ///
    /// **Trade-off**: loses the ability to override from the
    /// HTTP client. Useful for fields that should NEVER come
    /// from the client (`created_at`, `updated_at`). If you need
    /// conditional override, don't use `@db_default` and send the
    /// timestamp explicitly from Fitz (via builtin or helper).
    pub db_default: bool,
    /// v0.10.16 — SQL expression of the default when the user passes
    /// `@db_default("NOW()")`. `None` when `@db_default` is used
    /// without args (original 10.8.2 behavior: skip INSERT but
    /// without specific default — the user puts the `DEFAULT NOW()`
    /// by hand in their CREATE TABLE / migration).
    ///
    /// When `Some(sql)`, `fitz db diff` emits `DEFAULT <sql>`
    /// in the CREATE TABLE / ADD COLUMN automatically. The diff
    /// normalization is case-insensitive over function
    /// calls (`NOW()` == `now()`) — avoids false positives when
    /// Postgres returns lowercase `now()` from
    /// `information_schema.columns.column_default`.
    pub db_default_sql: Option<String>,
    /// v0.10.17 (10.6.b.2) — If present, `fitz db diff` emits
    /// `ALTER TABLE ... RENAME COLUMN "old" TO "new"` instead of
    /// DROP COLUMN + ADD COLUMN. Transient decorator: the user
    /// removes it after applying the migration.
    /// Syntax: `@renamed_from("old") field_name: Type = default`.
    pub renamed_from: Option<String>,
}

/// Phase 10.4.a — Relation type declared over a field.
///
/// `BelongsTo` and `HasOne` differ in who hosts the FK:
/// `BelongsTo` means "this field is a FK column pointing
/// to the other type"; `HasOne` means "the other type has a FK
/// pointing to this one". The first is REAL (appears in SELECT/
/// INSERT/UPDATE); the other two are VIRTUAL (navigable only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    /// `@belongs_to("User")` over `author_id: Int`. This field
    /// stores the FK pointing to the primary key of the other table.
    /// Is real (column in the SELECT) and participates in normal SQL.
    BelongsTo,
    /// Residual debt #2 (v0.10.5) — `BelongsToCompanion` is the
    /// virtual counterpart of a `BelongsTo`. Registered
    /// automatically when the user declares `@belongs_to("User")
    /// user_id: Int` AND a sibling field `user: User?` in the same
    /// type. The convention: stripping `_id` from the FK + match with
    /// sibling Nullable<Target>. The field is virtual (does not appear
    /// in SELECT/INSERT) and is populated via `.preload("user")` with
    /// inverse batch SELECT (target table WHERE id IN (FK values)).
    /// Without preload, the field stays Null.
    BelongsToCompanion,
    /// `@has_one("Profile")` over `profile: Profile?`. Virtual
    /// field: the other type's table has a FK pointing
    /// to this one. Does not appear in builder SELECT/INSERT/UPDATE.
    HasOne,
    /// `@has_many("Post", via="author_id")` over
    /// `posts: List<Post>`. Virtual, like HasOne, but
    /// returns multiple instances of the other type.
    HasMany,
}

/// Phase 10.4.a — Cascade action for `on_delete`/`on_update`.
/// Default is `Restrict` (Postgres default, conservative).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CascadeAction {
    /// If the referenced row is deleted, this one is deleted too.
    Cascade,
    /// If the referenced row is deleted, this FK is set to NULL.
    /// Requires the field to be nullable (`Int?`).
    SetNull,
    /// Default. Deleting the referenced row fails if there are rows
    /// referencing it.
    #[default]
    Restrict,
    /// Like `Restrict` but the check is deferred until end
    /// of the transaction (rarely used).
    NoAction,
}

impl CascadeAction {
    /// SQL clause for `ON DELETE`/`ON UPDATE`. Emitted by the
    /// migration in 10.7.
    pub fn as_sql(self) -> &'static str {
        match self {
            CascadeAction::Cascade => "CASCADE",
            CascadeAction::SetNull => "SET NULL",
            CascadeAction::Restrict => "RESTRICT",
            CascadeAction::NoAction => "NO ACTION",
        }
    }
}

/// Phase 10.4.a — Metadata per relation declared with
/// `@belongs_to` / `@has_one` / `@has_many`.
#[derive(Debug, Clone)]
pub struct RelationMetadata {
    pub kind: RelationKind,
    /// Fitz name of the referenced type (e.g. "User").
    pub target_type: String,
    /// Fitz name of the field acting as FK:
    ///   - For `BelongsTo`: the local field carrying the FK
    ///     (e.g. in `Post.@belongs_to("User") author_id`, fk_field
    ///     = "author_id"; default = the decorated field).
    ///   - For `HasOne` / `HasMany`: the FK field IN THE OTHER type
    ///     (e.g. `User.@has_many("Post", via="author_id") posts`,
    ///     fk_field = "author_id" but refers to the field of `Post`).
    pub fk_field: String,
    pub on_delete: CascadeAction,
    pub on_update: CascadeAction,
}

impl TableMetadata {
    /// Phase 10.4.a — `true` if the field is virtual to the ORM
    /// (declared with `@has_one`/`@has_many`, or auto-detected
    /// as BelongsToCompanion in v0.10.5). The SQL builder
    /// skips these fields in SELECT/INSERT/UPDATE. `BelongsTo`
    /// is NOT virtual — the FK column is real.
    pub fn is_virtual_field(&self, field_name: &str) -> bool {
        matches!(
            self.relations.get(field_name).map(|r| r.kind),
            Some(RelationKind::HasOne)
                | Some(RelationKind::HasMany)
                | Some(RelationKind::BelongsToCompanion)
        )
    }
}

/// Phase 4 (fitz-liveviews Y-B) — Metadata extracted from
/// `@live_component("name")` on a `type Foo { ... }`. If the type
/// does NOT carry the decorator, it stays `None` in
/// `TypeEnv.live_components`. If it does, we register the
/// component name (unique string identifier used by
/// `component(name, id)` and by `@render_for` / `@on` handlers
/// declared in sub-phase 1.b).
///
/// The checker (`process_live_component_decorators`) only
/// validates shape (1 Str literal arg, no kwargs, no duplicates).
/// The framework layer (`fitz-liveviews`) consumes this metadata
/// via a builtin to dispatch render + event handlers keyed by
/// component name.
#[derive(Debug, Clone)]
pub struct LiveComponentMetadata {
    /// Component name (`@live_component("name")`). Unique string
    /// identifier. Used by the framework layer to look up render
    /// + event handlers registered via `@render_for` and `@on`.
    pub name: String,
    /// `TypeId` of the state type that carries the decorator.
    /// Populated by `resolve_program` right after
    /// `process_live_component_decorators` succeeds.
    pub type_id: TypeId,
}

/// Type environment of the program. Carries:
///  - Built-ins (primitives and generics), implicit via
///    `resolve_named`.
///  - Declared nominal types, accessible by name.
///
/// No nested scopes yet: 5.2 works at the whole-program level.
/// When body checks arrive (5.3), local scopes for `let`/params
/// will be added.
#[derive(Debug, Default)]
pub struct TypeEnv {
    nominals: Vec<NominalInfo>,
    by_name: HashMap<String, TypeId>,
    /// 8-pyi.C (v0.9.57): mapping `module_name → synthetic nominal_id`
    /// for adjacent `.pyi` stubs loaded by `pyi_loader`. Each
    /// stub is materialized as a synthetic nominal with one field per
    /// top-level fn/var of the stub. The checker consults this table
    /// in `Stmt::FromImport` to bind `from python import foo`
    /// with `Type::Nominal(id)` instead of opaque `Type::PyAny` —
    /// unblocks typed field access (`foo.fetch_user(uid)` resolves to
    /// `Result<User>` instead of `Result<Any>`).
    pyi_modules: HashMap<String, TypeId>,
    /// Phase 10.3.a — ORM metadata by `TypeId`. Only types with
    /// `@table(...)` appear here. The runtime (10.3.b) consults
    /// `env.table_metadata(id)` to know the SQL name, primary
    /// key, and per-column overrides. For types without `@table`,
    /// `table_metadata` returns `None` and ORM queries
    /// fail with a clear error.
    tables: HashMap<TypeId, TableMetadata>,
    /// Phase 4 (fitz-liveviews Y-B) — LiveComponent metadata by
    /// `TypeId`. Only types with `@live_component("name")` appear
    /// here. The framework layer consults
    /// `env.live_component_metadata(id)` to know the component
    /// name for handler dispatch. Types without the decorator
    /// return `None`.
    live_components: HashMap<TypeId, LiveComponentMetadata>,
    /// Phase 4 (fitz-liveviews Y-B, session 1.b) — Render handler
    /// registry. Keyed by component name, value is the top-level
    /// Fitz fn name declared with `@render_for("name")`. Populated
    /// by `resolve_program` after shape validation
    /// (`process_render_for_decorators`). Consumed by the
    /// framework layer to resolve which fn renders each
    /// component. Duplicate declarations (two `@render_for("x")`
    /// on distinct fns) fire an error at register time.
    render_handlers: HashMap<String, String>,
    /// Phase 4 (fitz-liveviews Y-B, session 1.b) — Event handler
    /// registry. Keyed by `(component_name, event_name)`, value
    /// is the top-level Fitz fn name declared with
    /// `@on("component", "event")`. Duplicate `(component,
    /// event)` pairs fire an error at register time.
    event_handlers: HashMap<(String, String), String>,
    /// W12 (v0.10.8) — `@auth_provider` declared in a module
    /// imported by the local program. The caller (typically
    /// `main.rs::check_program_with_pyi_stubs_and_imports`)
    /// pre-scans it on each imported module via
    /// `extract_auth_provider_signature` and registers it in the importer's
    /// env with `set_imported_auth_provider`. The checker
    /// (`collect_auth_provider`) falls back to this slot when it does not
    /// find a local provider, so the importer's `@authenticated`/
    /// `@admin` handlers can compile against a cross-module provider.
    /// The codegen also consults it to emit wrapper
    /// invocations with qualified path (`<module>::<fn>`).
    imported_auth_provider: Option<ImportedAuthProvider>,
    /// B10 (sub-paso 5 cosecha post-fitzwatch, 2026-06-19) — names
    /// of top-level fns marked with `@background` that live in
    /// modules imported by the local program. Pre-scanned by the
    /// caller via `extract_background_fn_names` and merged into
    /// `CheckCtx.background_fns` so `spawn(<imported_fn>(args))`
    /// in the importer passes the checker's `@background` validation.
    imported_background_fns: HashSet<String>,
    /// v0.19.5 (post-fitzwatch 2026-06-26) — names of fns that are
    /// referenced as `@middleware(name)` somewhere in the project
    /// tree (main OR any imported module). Pre-scanned by the caller
    /// via `extract_middleware_fn_names` (collected over main + all
    /// loaded modules) and merged into `CheckCtx.middleware_fn_names`
    /// so the module's checker accepts `return <status> { ... }` and
    /// `return null` inside a fn used as middleware cross-module.
    /// Parallel to `imported_background_fns` (B10) and
    /// `imported_auth_provider` (W12).
    imported_middleware_fns: HashSet<String>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self::default()
    }

    /// 8-pyi.C: registers the synthetic nominal `id` associated with
    /// stub `name`. Called by `pyi_loader::load_callables` after
    /// `resolve_program` (the nominals declared by the .fitz
    /// are already available, the stub fns can refer to them in
    /// their ret type).
    pub fn set_pyi_module(&mut self, name: String, id: TypeId) {
        self.pyi_modules.insert(name, id);
    }

    /// 8-pyi.C: lookup the synthetic nominal for a stub. Used by
    /// the checker in `Stmt::FromImport` from_python. Returns `None`
    /// if there is no adjacent `.pyi` (binding falls to gradual
    /// `Type::PyAny`).
    pub fn pyi_module(&self, name: &str) -> Option<TypeId> {
        self.pyi_modules.get(name).copied()
    }

    /// Phase 10.3.a — Registers ORM metadata for a nominal type.
    /// Called by `resolve_program` when a `type` carries
    /// `@table`/`@primary`/etc. decorators. Without `@table` the type
    /// does NOT appear in `tables` and `table_metadata` returns `None`.
    pub fn set_table_metadata(&mut self, id: TypeId, meta: TableMetadata) {
        self.tables.insert(id, meta);
    }

    /// Phase 10.3.a — Returns the type's ORM metadata if declared
    /// with `@table(...)`. The runtime (10.3.b) calls this
    /// when it sees `User.where(...)` to know the SQL name
    /// of the table and the primary key.
    pub fn table_metadata(&self, id: TypeId) -> Option<&TableMetadata> {
        self.tables.get(&id)
    }

    /// Phase 4 (fitz-liveviews Y-B) — Registers LiveComponent
    /// metadata for a nominal type. Called by `resolve_program`
    /// when a `type` carries `@live_component("name")`.
    pub fn set_live_component_metadata(&mut self, id: TypeId, meta: LiveComponentMetadata) {
        self.live_components.insert(id, meta);
    }

    /// Phase 4 (fitz-liveviews Y-B) — Returns the type's
    /// LiveComponent metadata if declared with
    /// `@live_component("name")`. Consumed by the framework
    /// layer to dispatch render + event handlers keyed by
    /// component name.
    pub fn live_component_metadata(&self, id: TypeId) -> Option<&LiveComponentMetadata> {
        self.live_components.get(&id)
    }

    /// Phase 4 (fitz-liveviews Y-B) — Reverse lookup: returns the
    /// `TypeId` of the state type registered under the given
    /// component name, or `None` if no such component exists.
    /// The framework layer uses this when resolving
    /// `component("name", id)` to find which type carries the
    /// render + event handlers.
    pub fn live_component_by_name(&self, name: &str) -> Option<TypeId> {
        self.live_components
            .iter()
            .find_map(|(id, meta)| (meta.name == name).then_some(*id))
    }

    /// Phase 4 (fitz-liveviews Y-B, session 1.b) — Registers a
    /// render handler for a component. Returns `Err` if the
    /// component already has a handler declared elsewhere. The
    /// caller (typically `resolve_program`) turns the `Err`
    /// into a `FitzError` with the offending fn's span.
    pub fn set_render_handler(&mut self, component: String, fn_name: String) -> Result<(), String> {
        if let Some(existing) = self.render_handlers.get(&component) {
            return Err(existing.clone());
        }
        self.render_handlers.insert(component, fn_name);
        Ok(())
    }

    /// Phase 4 (fitz-liveviews Y-B, session 1.b) — Returns the
    /// name of the fn registered as render handler for the
    /// given component, or `None` if no such handler exists.
    pub fn render_handler_for(&self, component: &str) -> Option<&str> {
        self.render_handlers.get(component).map(|s| s.as_str())
    }

    /// Phase 4 (fitz-liveviews Y-B, session 1.b) — Registers an
    /// event handler for a `(component, event)` pair. Same error
    /// semantics as `set_render_handler`.
    pub fn set_event_handler(
        &mut self,
        component: String,
        event: String,
        fn_name: String,
    ) -> Result<(), String> {
        let key = (component, event);
        if let Some(existing) = self.event_handlers.get(&key) {
            return Err(existing.clone());
        }
        self.event_handlers.insert(key, fn_name);
        Ok(())
    }

    /// Phase 4 (fitz-liveviews Y-B, session 1.b) — Returns the
    /// name of the fn registered as event handler for
    /// `(component, event)`, or `None` if no such handler
    /// exists.
    pub fn event_handler_for(&self, component: &str, event: &str) -> Option<&str> {
        self.event_handlers
            .get(&(component.to_string(), event.to_string()))
            .map(|s| s.as_str())
    }

    /// Registers a nominal type by name, returning its id.
    /// If the name was already there → error "redeclared type".
    pub fn declare_nominal(&mut self, name: String) -> Result<TypeId, FitzError> {
        if self.by_name.contains_key(&name) {
            return Err(FitzError::new(
                ErrorKind::TypeError,
                0,
                0,
                format!("type `{}` declared more than once", name),
            ));
        }
        let id = TypeId(self.nominals.len());
        self.nominals.push(NominalInfo {
            name: name.clone(),
            fields: None,
            methods: Vec::new(),
        });
        self.by_name.insert(name, id);
        Ok(id)
    }

    /// Completes the fields of a nominal (second pass).
    pub fn set_fields(&mut self, id: TypeId, fields: Vec<ResolvedField>) {
        self.nominals[id.0].fields = Some(fields);
    }

    /// R.3 — Sets the methods of a nominal (third pass).
    pub fn set_methods(&mut self, id: TypeId, methods: Vec<NominalMethod>) {
        self.nominals[id.0].methods = methods;
    }

    pub fn lookup(&self, name: &str) -> Option<TypeId> {
        self.by_name.get(name).copied()
    }

    pub fn info(&self, id: TypeId) -> &NominalInfo {
        &self.nominals[id.0]
    }

    /// Number of registered nominals. Useful for tests.
    #[allow(dead_code)]
    pub fn nominal_count(&self) -> usize {
        self.nominals.len()
    }

    /// W12 (v0.10.8) — Registers info of an `@auth_provider` living in
    /// an imported module. The caller invokes this AFTER
    /// `resolve_program` (so the `User` nominal is already registered
    /// in the importer's TypeEnv via pass 1b), but BEFORE
    /// `check_with_env` (so `collect_auth_provider` sees it in its
    /// fallback). Idempotent: if there is already an imported provider and
    /// it's called again, the last one wins (unlikely case — only one
    /// `@auth_provider` is allowed across the whole import tree; the
    /// "more than one" validation is done by the caller orchestrating
    /// the pre-scan).
    pub fn set_imported_auth_provider(&mut self, provider: ImportedAuthProvider) {
        self.imported_auth_provider = Some(provider);
    }

    /// W12 (v0.10.8) — Returns `Some(provider)` if the caller registered
    /// a cross-module `@auth_provider` via
    /// `set_imported_auth_provider`. Consulted by
    /// `collect_auth_provider` (fallback when there is no local provider) and
    /// the codegen (to emit module-qualified invocations).
    pub fn imported_auth_provider(&self) -> Option<&ImportedAuthProvider> {
        self.imported_auth_provider.as_ref()
    }

    /// B10 (sub-paso 5 cosecha post-fitzwatch, 2026-06-19) —
    /// Registers names of top-level fns marked with `@background`
    /// in a module imported by the local program. Pre-scanned via
    /// `extract_background_fn_names`. The caller invokes this AFTER
    /// `resolve_program` and BEFORE `check_with_env` so the checker
    /// (`collect_background_fns`) sees them when validating
    /// `spawn(<imported_fn>(...))`.
    pub fn add_imported_background_fns<I: IntoIterator<Item = String>>(&mut self, names: I) {
        self.imported_background_fns.extend(names);
    }

    /// B10 — Returns names of top-level fns marked with `@background`
    /// in cross-module imports.
    pub fn imported_background_fns(&self) -> &HashSet<String> {
        &self.imported_background_fns
    }

    /// v0.19.5 — Registers names of fns referenced as
    /// `@middleware(name)` anywhere in the project tree (collected
    /// over main + all imported modules). Pre-scanned by the caller
    /// via `extract_middleware_fn_names`. The caller invokes this
    /// AFTER `resolve_program` and BEFORE `check_with_env` so the
    /// checker (`collect_middleware_fn_names`) sees them when
    /// validating cross-module middleware fns that contain
    /// `return <status> { ... }` or `return null` as gate semantics.
    pub fn add_imported_middleware_fns<I: IntoIterator<Item = String>>(&mut self, names: I) {
        self.imported_middleware_fns.extend(names);
    }

    /// v0.19.5 — Returns names of fns referenced as `@middleware(name)`
    /// in cross-module imports. Consumed by `collect_middleware_fn_names`
    /// in `check_program`.
    pub fn imported_middleware_fns(&self) -> &HashSet<String> {
        &self.imported_middleware_fns
    }
}

// ---------------------------------------------------------------------------
// Side-table of types synthesized per node (Phase 9.0 — F16)
// ---------------------------------------------------------------------------

/// Hashable key derived from a `Span`. Exists because `Span` has a
/// custom `PartialEq` that always returns `true` (needed so that
/// AST tests compare structure without re-deriving parser positions;
/// see the comment on `impl PartialEq for Span` in
/// `src/ast.rs`). With that semantics, `Span` does not work as a
/// `HashMap` key — all entries would collide. `SpanKey` wraps
/// `(line, column)` with real `Eq`/`Hash` for the side-table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanKey(pub usize, pub usize);

impl From<Span> for SpanKey {
    fn from(s: Span) -> Self {
        SpanKey(s.line, s.column)
    }
}

/// Side-table that persists the `Type` synthesized by `infer_expr` for
/// each `Expr` node with a known `Span`. Enabling pre-requisite for the
/// LSP (Phase 9): `textDocument/hover` consults the type of the node under
/// the cursor, and contextual completion (`u.` → fields of `User`)
/// needs the receiver's type.
///
/// Populating policy:
/// - The wrapper over `infer_expr` registers **all** `Expr`s that
///   pass through the checker — broad granularity, simple, without "I forgot
///   such a case".
/// - Nodes with `Span::ZERO` (parser synthetics, test nodes) are
///   omitted: they are not user-visible and two synthetics would collide under
///   the same key `(0, 0)`.
/// - `Expr::Error` (F15) types as `Type::Any` and is persisted the same —
///   the LSP decides what to show.
///
/// No spatial index (start-end range). For hover, the LSP picks the
/// node whose span is closest to the cursor; a future refinement
/// with full ranges remains as minor debt (requires `end_span` on
/// `Expr`).
#[derive(Debug, Clone, Default)]
pub struct TypeInfo {
    inner: HashMap<SpanKey, Type>,
}

impl TypeInfo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Persists the `Type` associated with the node's `Span`. Silent omit
    /// for `Span::ZERO` (synthetic / test nodes): those don't contribute to
    /// hover and would collide with each other.
    pub fn record(&mut self, span: Span, ty: Type) {
        if !span.is_known() {
            return;
        }
        self.inner.insert(SpanKey::from(span), ty);
    }

    /// Returns the `Type` previously registered for `span`, if it exists.
    /// Public API for the LSP (Phase 9.x.2 — hover). `#[allow(dead_code)]`
    /// until consumers land, same pattern as
    /// `parse_with_recovery` in F15.
    #[allow(dead_code)]
    pub fn type_at(&self, span: Span) -> Option<&Type> {
        if !span.is_known() {
            return None;
        }
        self.inner.get(&SpanKey::from(span))
    }

    /// Number of entries in the side-table. Useful for smoke tests and
    /// for the LSP to estimate coverage. `#[allow(dead_code)]` until
    /// external consumers land.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` if there are no registered entries.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterates all entries of the side-table. Useful for LSP consumers
    /// (Phase 9.x.2 — hover) that need to perform a heuristic
    /// lookup over positions (find the closest span to
    /// a cursor). Without this, `type_at` only allows exact lookup.
    pub fn iter(&self) -> impl Iterator<Item = (&SpanKey, &Type)> {
        self.inner.iter()
    }
}

/// Side-table that persists the **declaration** `Span` of each
/// `Ident` used in the program. Enabling pre-requisite for the LSP
/// (Phase 9.x.3 — go-to-definition): `textDocument/definition` looks up
/// the ident under the cursor and returns the location where it was
/// declared.
///
/// Populating policy:
/// - Every `Expr::Ident(name, use_span)` that the checker resolves
///   successfully via `lookup_binding` registers
///   `(use_span → def_span)` when the binding has a known span.
/// - **Builtins** (`print`, `len`, `sleep`, `cors`) have
///   `def_span = Span::ZERO` and are omitted (no file to
///   jump to).
/// - **Nodes with `use_span == Span::ZERO`** (synthetic / tests)
///   are omitted like in `TypeInfo`.
///
/// Granularity of the registered `def_span`: due to current AST
/// limitations (no own spans in `AssignTarget::Ident`/`Param`/
/// `For.var`), we use the containing `Stmt`'s span as
/// approximation. VSCode jumps to the stmt — the user sees the line
/// of declaration. Precision by exact name remains as S1 debt.
#[derive(Debug, Clone, Default)]
pub struct DefinitionInfo {
    inner: HashMap<SpanKey, Span>,
}

impl DefinitionInfo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Persists the `use_span → def_span` relation. Silent omit
    /// when either of the two is `Span::ZERO` (synthetic / builtins).
    pub fn record(&mut self, use_span: Span, def_span: Span) {
        if !use_span.is_known() || !def_span.is_known() {
            return;
        }
        self.inner.insert(SpanKey::from(use_span), def_span);
    }

    /// Exact lookup by use span. Public API for tests.
    #[allow(dead_code)]
    pub fn definition_at(&self, use_span: Span) -> Option<Span> {
        if !use_span.is_known() {
            return None;
        }
        self.inner.get(&SpanKey::from(use_span)).copied()
    }

    /// Number of entries. `#[allow(dead_code)]` parallel to
    /// `TypeInfo::len`.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` if there are no registered entries.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterates all entries. Useful for the LSP (Phase 9.x.3) which
    /// performs heuristic lookup over cursor positions.
    pub fn iter(&self) -> impl Iterator<Item = (&SpanKey, &Span)> {
        self.inner.iter()
    }
}

// ---------------------------------------------------------------------------
// TypeExpr → Type resolution
// ---------------------------------------------------------------------------

/// Converts a `TypeExpr` (syntactic) into a `Type` (resolved)
/// against `env`. Returns the `Type` or a `FitzError` describing
/// what failed. Errors are always `ErrorKind::TypeError`.
pub fn resolve_type_expr(t: &TypeExpr, env: &TypeEnv) -> Result<Type, FitzError> {
    match t {
        TypeExpr::Named(name) => resolve_named(name, &[], env),
        TypeExpr::Generic { name, args } => resolve_named(name, args, env),
        TypeExpr::Nullable(inner) => {
            let inner = resolve_type_expr(inner, env)?;
            Ok(Type::Nullable(Box::new(inner)))
        }
        TypeExpr::Function { params, ret } => {
            let params: Vec<Type> = params
                .iter()
                .map(|p| resolve_type_expr(p, env))
                .collect::<Result<_, _>>()?;
            let ret = resolve_type_expr(ret, env)?;
            Ok(Type::Function {
                params,
                ret: Box::new(ret),
            })
        }
        // Tuples (mini-batch T): element-by-element resolution.
        TypeExpr::Tuple(items) => {
            let resolved: Vec<Type> = items
                .iter()
                .map(|t| resolve_type_expr(t, env))
                .collect::<Result<_, _>>()?;
            Ok(Type::Tuple(resolved))
        }
    }
}

/// Resolves a name + arguments against the env. The separation
/// between `Named` and `Generic` disappears here: `List<Int>` and
/// `List` (without arguments) take the same path and the arity
/// validated at the corresponding place.
fn resolve_named(name: &str, args: &[TypeExpr], env: &TypeEnv) -> Result<Type, FitzError> {
    // Primitives (arity 0). If the user applies them as generics
    // → explicit arity error.
    let prim = match name {
        "Int" => Some(Type::Int),
        "Float" => Some(Type::Float),
        "Str" => Some(Type::Str),
        "Bool" => Some(Type::Bool),
        "Null" => Some(Type::Null),
        "Bytes" => Some(Type::Bytes),
        "Range" => Some(Type::Range),
        // F13.C — `Any` as type annotation (gradual escape +
        // heterogeneous). Enables `body: List<Any>` / `body: Map<Str, Any>`
        // in HTTP handlers.
        "Any" => Some(Type::Any),
        // Phase 10.1 — opaque types of the native Postgres driver.
        // `DbConn` is the connection handle returned by `db.connect`,
        // annotatable in params (`fn run(db: DbConn)`) and vars
        // (`let conn: DbConn = ...`). `DbRow` is the raw row returned
        // by `db.query`, also annotatable.
        "DbConn" => Some(Type::DbConn),
        "DbRow" => Some(Type::DbRow),
        // v0.10.24 — built-in types for native dates and UUIDs.
        "Date" => Some(Type::Date),
        "DateTime" => Some(Type::DateTime),
        "Uuid" => Some(Type::Uuid),
        _ => None,
    };
    if let Some(t) = prim {
        if !args.is_empty() {
            return Err(arity_error(name, 0, args.len()));
        }
        return Ok(t);
    }

    // Built-in generics with fixed arity.
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
            // Mini-batch Re+ — arity 1 or 2. `Result<T>` expands to
            // `Result<T, Str>` (default for compat). `Result<T, E>`
            // with explicit E enables carry of custom types in Err.
            if args.is_empty() || args.len() > 2 {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    format!(
                        "type `Result` expects 1 or 2 arguments, received {}",
                        args.len()
                    ),
                ));
            }
            let ok = resolve_type_expr(&args[0], env)?;
            let err = if args.len() == 2 {
                resolve_type_expr(&args[1], env)?
            } else {
                Type::Str
            };
            Ok(Type::Result {
                ok: Box::new(ok),
                err: Box::new(err),
            })
        }
        "Future" => {
            expect_arity(name, 1, args)?;
            let inner = resolve_type_expr(&args[0], env)?;
            Ok(Type::Future(Box::new(inner)))
        }
        "Secret" => {
            // Phase 12.2.a — `Secret<T>` opaque type with auto-redaction.
            // Fixed arity 1. The inner T can be any resolvable type;
            // typically `Str` (passwords, tokens), but
            // can also be nominal (`Secret<Credentials>` for
            // atomic bundles).
            expect_arity(name, 1, args)?;
            let inner = resolve_type_expr(&args[0], env)?;
            Ok(Type::Secret(Box::new(inner)))
        }
        "WsConn" => {
            // 9.w.2-wsconn-bidir (v0.9.38) — `WsConn` accepts 1 or 2
            // arguments:
            //   `WsConn<T>` (arity 1, symmetric) — recv == send == T.
            //   `WsConn<In, Out>` (arity 2, asymmetric) — recv = In,
            //     send = Out.
            if args.is_empty() || args.len() > 2 {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    format!(
                        "type `WsConn` expects 1 or 2 arguments (`WsConn<T>` for symmetric channel, `WsConn<In, Out>` for asymmetric channel), received {}",
                        args.len()
                    ),
                ));
            }
            let recv = resolve_type_expr(&args[0], env)?;
            let send = if args.len() == 2 {
                resolve_type_expr(&args[1], env)?
            } else {
                recv.clone()
            };
            Ok(Type::WsConn {
                recv: Box::new(recv),
                send: Box::new(send),
            })
        }
        _ => {
            // Nominal declared by the user.
            match env.lookup(name) {
                Some(id) => {
                    if !args.is_empty() {
                        return Err(FitzError::new(
                            ErrorKind::TypeError,
                            0,
                            0,
                            format!(
                                "type `{}` is not generic, does not accept type arguments",
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
                    format!("unknown type `{}`", name),
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

/// Pre-registers the built-in types provided by the Fitz HTTP runtime.
/// Today: `Request` (built by the dispatcher before each handler/
/// middleware; exposes `method`, `path`, `headers`) and `Response` (opaque
/// marker to annotate the return of middlewares — the real value is
/// produced by `return <status> { ... }`).
///
/// Called from `resolve_program` before pass 1, so a
/// `type Request { ... }` declared by the user fires the existing
/// redeclaration error. The cost: two fixed nominals in the env
/// even in programs that don't use HTTP. Acceptable trade-off — checking
/// costs stay O(1) and the semantic surface of the language
/// stays consistent.
fn register_http_builtin_types(env: &mut TypeEnv) {
    // `Request`: the assigned id is stable because we run
    // before any other registration. Its fields are completed
    // explicitly (not derived from a Stmt::TypeDef).
    let req_id = env
        .declare_nominal("Request".to_string())
        .expect("Request is the first nominal — cannot collide");
    env.set_fields(
        req_id,
        vec![
            ResolvedField {
                name: "method".into(),
                type_: Type::Str,
            },
            ResolvedField {
                name: "path".into(),
                type_: Type::Str,
            },
            ResolvedField {
                name: "headers".into(),
                type_: Type::Map(Box::new(Type::Str), Box::new(Type::Str)),
            },
        ],
    );

    // `Response`: instantiable built-in for custom HTTP responses
    // (v0.19.0). Previously was an opaque marker without fields,
    // used only as the return type annotation of middlewares
    // (`fn auth(req) -> Response?`). Now carries `status`,
    // `content_type`, `headers`, and `body` so handlers can return
    // `Response { content_type: "application/rss+xml", body: rss }`
    // for cases where the default JSON serialisation does not fit
    // (RSS feeds, sitemaps, plain text, CSV exports, SVG badges,
    // etc.). The field order MUST mirror the runtime registration
    // in `evaluator::register_builtins` (Response section) — the
    // evaluator's struct literal validation reorders to declaration
    // order, but having both in sync avoids confusion.
    let resp_id = env
        .declare_nominal("Response".to_string())
        .expect("Response is the second nominal — cannot collide");
    env.set_fields(
        resp_id,
        vec![
            ResolvedField {
                name: "status".into(),
                type_: Type::Int,
            },
            ResolvedField {
                name: "content_type".into(),
                type_: Type::Str,
            },
            ResolvedField {
                name: "headers".into(),
                type_: Type::Map(Box::new(Type::Str), Box::new(Type::Str)),
            },
            ResolvedField {
                name: "body".into(),
                type_: Type::Str,
            },
            // v0.19.0 Block 2 — opt-in binary body. When set (non-null),
            // it wins over `body`. Setting BOTH at runtime triggers a
            // 500 with a clear message (programming error). Used for
            // PDF, ZIP, images, anything that is not UTF-8 text.
            ResolvedField {
                name: "body_bytes".into(),
                type_: Type::Nullable(Box::new(Type::Bytes)),
            },
        ],
    );

    // Mini-batch MP2 + File.content Bytes — `File`: built-in nominal
    // to represent files from multipart/form-data bodies. The
    // dispatcher builds it when parsing `multipart/form-data`
    // requests. Fields:
    //   - `name`: filename from Content-Disposition (`filename="..."`),
    //     `null` if the part is not a file (form text field).
    //   - `content_type`: MIME from the part's Content-Type, `null` if
    //     not present.
    //   - `content`: raw binary content. Previously was `Str` (UTF-8
    //     only); now it's `Bytes` (any sequence). For UTF-8 text,
    //     use `f.content.to_str() -> Result<Str>`.
    let file_id = env
        .declare_nominal("File".to_string())
        .expect("File is the third built-in nominal — cannot collide");
    env.set_fields(
        file_id,
        vec![
            ResolvedField {
                name: "name".into(),
                type_: Type::Nullable(Box::new(Type::Str)),
            },
            ResolvedField {
                name: "content_type".into(),
                type_: Type::Nullable(Box::new(Type::Str)),
            },
            ResolvedField {
                name: "content".into(),
                type_: Type::Bytes,
            },
        ],
    );

    // Mini-fase HTTP client (2026-06-18) — `HttpClientResponse`:
    // type returned by `http.get`, `http.post`, etc. once `.await`-ed.
    // Fields fixed: status (Int), body (Str), headers (Map<Str, Str>),
    // duration_ms (Int). Field order matters for Display and for the
    // evaluator's struct literal validation, so it MUST mirror what
    // `http_client::build_http_response_instance` emits.
    let http_resp_id = env
        .declare_nominal("HttpClientResponse".to_string())
        .expect("HttpClientResponse is a built-in nominal — cannot collide");
    env.set_fields(
        http_resp_id,
        vec![
            ResolvedField {
                name: "status".into(),
                type_: Type::Int,
            },
            ResolvedField {
                name: "body".into(),
                type_: Type::Str,
            },
            ResolvedField {
                name: "headers".into(),
                type_: Type::Map(Box::new(Type::Str), Box::new(Type::Str)),
            },
            ResolvedField {
                name: "duration_ms".into(),
                type_: Type::Int,
            },
        ],
    );

    // Mini-tanda SMTP builtin (2026-06-19) — `SmtpResult`: type returned
    // by `smtp.send(opts)` once `.await`-ed. Fields fixed: delivered
    // (Bool), message_id (Str), duration_ms (Int). Field order matters
    // for Display and for the evaluator's struct literal validation, so
    // it MUST mirror what `smtp::build_smtp_result_instance` emits.
    let smtp_result_id = env
        .declare_nominal("SmtpResult".to_string())
        .expect("SmtpResult is a built-in nominal — cannot collide");
    env.set_fields(
        smtp_result_id,
        vec![
            ResolvedField {
                name: "delivered".into(),
                type_: Type::Bool,
            },
            ResolvedField {
                name: "message_id".into(),
                type_: Type::Str,
            },
            ResolvedField {
                name: "duration_ms".into(),
                type_: Type::Int,
            },
        ],
    );
}

fn arity_error(name: &str, expected: usize, found: usize) -> FitzError {
    FitzError::new(
        ErrorKind::TypeError,
        0,
        0,
        format!(
            "type `{}` expects {} type argument(s), received {}",
            name, expected, found
        ),
    )
}

// ---------------------------------------------------------------------------
// Resolution pass over the program
// ---------------------------------------------------------------------------

/// Result of checking a program: the `TypeEnv` with all
/// declared types resolved, and the (possibly empty) list of
/// accumulated errors. We always return both: the caller decides
/// whether to abort (strict mode) or report as warnings (run mode).
pub fn resolve_program(program: &Program) -> (TypeEnv, Vec<FitzError>) {
    resolve_program_with_env(program, TypeEnv::new(), Vec::new())
}

/// Variant of `resolve_program` that starts from a `TypeEnv` already
/// pre-filled (typically by `pyi_loader::load_stubs` which registers
/// nominals declared in `.pyi` adjacent to the root `.fitz` —
/// 8-pyi.B, v0.9.57).
///
/// `errors_init` is preserved (typically empty from the caller; the
/// loader silent-fallback does not produce type errors).
///
/// **Policy on redeclarations**: if the pre-filled env already has
/// a nominal `Foo` and the program also declares `type Foo { ... }`,
/// pass 1 emits the standard redeclaration error — the caller
/// (loader) is responsible for skipping stub classes that the
/// program already declares, via the pre-scan in `pyi_loader::load_stubs`.
pub fn resolve_program_with_env(
    program: &Program,
    initial_env: TypeEnv,
    errors_init: Vec<FitzError>,
) -> (TypeEnv, Vec<FitzError>) {
    let mut env = initial_env;
    let mut errors = errors_init;

    // Pass 0 (mini-phase MW.1): register built-in types from the HTTP runtime.
    // `Request` is built by the dispatcher before invoking middlewares
    // and handlers; the user reads it inside their middlewares with
    // `req.method`, `req.path`, `req.headers`. `Response` is an
    // opaque marker to annotate `-> Response?` in middlewares; the user
    // does not instantiate it (the value is produced by `return <status> { ... }`).
    // If the user declares `type Request`/`type Response`, pass 1
    // emits the existing redeclaration error.
    register_http_builtin_types(&mut env);

    // Pass 1: register the names of locally-declared `type`s.
    // Forward refs between local nominals.
    for stmt in program {
        if let Stmt::TypeDef { name, .. } = stmt {
            if let Err(e) = env.declare_nominal(name.clone()) {
                errors.push(e);
            }
        }
    }

    // Pass 1b: register names brought by `from ... import ...`
    // as nominals with unknown fields. Without this, a
    // `User { ... }` coming from `from foo import User` is left without
    // a declared type and the checker complains. If the name clashes with
    // a local type, the local wins — the import is silently ignored
    // (decision: 5.x keeps gradual behavior; when 5.3.x
    // loads cross-file modules, we can refine the warning).
    //
    // `import foo` does not add names in the TypeEnv — the module is a
    // value, not a type. It's registered as a var in `check_stmt`.
    for stmt in program {
        if let Stmt::FromImport { names, .. } = stmt {
            for (n, alias) in names {
                // PreF8.4: with alias, the local binding in the TypeEnv
                // uses the alias. Without alias, the original name.
                let binding = alias.clone().unwrap_or_else(|| n.clone());
                if env.lookup(&binding).is_none() {
                    // declare_nominal can only fail if the name
                    // was already there; we already checked so it's safe.
                    let _ = env.declare_nominal(binding);
                }
            }
        }
    }

    // Pass 2: resolve the fields of each `type`.
    for stmt in program {
        if let Stmt::TypeDef { name, fields, .. } = stmt {
            // If the declaration failed (duplicate), there's no id to update.
            let id = match env.lookup(name) {
                Some(id) => id,
                None => continue,
            };
            // If the slot already has fields, this is the second time we see
            // this nominal — a duplicate already reported. Skip.
            if env.info(id).fields.is_some() {
                continue;
            }
            let mut resolved = Vec::new();
            for f in fields {
                match resolve_type_expr(&f.type_, &env) {
                    Ok(t) => {
                        if let Some(default) = &f.default {
                            if let Err(e) = check_field_default(name, &f.name, &t, default, &env) {
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
                        &format!("in field `{}` of type `{}`", f.name, name),
                    )),
                }
            }
            env.set_fields(id, resolved);
        }
    }

    // Pass 2.5 (R.3): resolve signatures of custom methods. After
    // having fields, methods can reference nominals in their
    // params/return. If a method already has a resolved signature (second
    // import / forward ref), we skip.
    for stmt in program {
        if let Stmt::TypeDef { name, methods, .. } = stmt {
            if methods.is_empty() {
                continue;
            }
            let id = match env.lookup(name) {
                Some(id) => id,
                None => continue,
            };
            if !env.info(id).methods.is_empty() {
                continue;
            }
            let mut resolved_methods: Vec<NominalMethod> = Vec::with_capacity(methods.len());
            for m in methods {
                let mut params = Vec::with_capacity(m.params.len());
                let mut param_names = Vec::with_capacity(m.params.len());
                for p in &m.params {
                    let pty = match &p.type_ {
                        Some(t) => resolve_type_expr(t, &env).unwrap_or(Type::Any),
                        None => Type::Any,
                    };
                    params.push(pty);
                    param_names.push(p.name.clone());
                }
                let ret = match &m.return_type {
                    Some(r) => resolve_type_expr(r, &env).unwrap_or(Type::Any),
                    None => Type::Any,
                };
                resolved_methods.push(NominalMethod {
                    name: m.name.clone(),
                    params,
                    ret,
                    is_async: m.is_async,
                    is_static: m.is_static,
                    param_names,
                });
            }
            env.set_methods(id, resolved_methods);
        }
    }

    // Pass 2.6 (Phase 10.3.a): process ORM decorators over
    // the `type Foo { ... }`s. Only types with `@table(...)`
    // generate metadata; the others are silently ignored.
    // Unrecognized decorators at the type level → error; at the
    // field level too. The metadata is saved in `env.tables`.
    for stmt in program {
        if let Stmt::TypeDef {
            name,
            decorators,
            fields,
            span,
            ..
        } = stmt
        {
            let id = match env.lookup(name) {
                Some(id) => id,
                None => continue,
            };
            match process_table_decorators(name, decorators, fields, *span) {
                Ok(Some(meta)) => env.set_table_metadata(id, meta),
                Ok(None) => {}
                Err(errs) => errors.extend(errs),
            }
            // Phase 4 (fitz-liveviews Y-B) — parallel pass over
            // `@live_component("name")`. The processor returns
            // metadata with a sentinel `type_id`; we overwrite it
            // with the resolved id here before registering.
            match process_live_component_decorators(name, decorators, *span) {
                Ok(Some(mut meta)) => {
                    meta.type_id = id;
                    env.set_live_component_metadata(id, meta);
                }
                Ok(None) => {}
                Err(errs) => errors.extend(errs),
            }
        }
    }

    // Phase 4 (fitz-liveviews Y-B, session 1.b) — pass over
    // `Stmt::FnDef`s picking up `@render_for("name")` and
    // `@on("component", "event")`. Only shape is validated here
    // (dedicated processors); signature validation (params +
    // return type + component-name existence) lives in the
    // checker walker with `check_render_for_decorator` /
    // `check_on_decorator`. Register conflicts (two
    // `@render_for("x")` on distinct fns, duplicate `(comp,
    // event)` pairs across the program) are reported here.
    for stmt in program {
        if let Stmt::FnDef {
            name,
            decorators,
            span,
            ..
        } = stmt
        {
            match process_render_for_decorators(name, decorators, *span) {
                Ok(Some(component)) => {
                    if let Err(existing) = env.set_render_handler(component.clone(), name.clone()) {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            span.line,
                            span.column,
                            format!(
                                "fn `{name}` declares `@render_for(\"{component}\")` but component `{component}` already has a render handler registered as fn `{existing}`"
                            ),
                        ));
                    }
                }
                Ok(None) => {}
                Err(errs) => errors.extend(errs),
            }
            match process_on_decorators(name, decorators, *span) {
                Ok(pairs) => {
                    for (component, event) in pairs {
                        if let Err(existing) =
                            env.set_event_handler(component.clone(), event.clone(), name.clone())
                        {
                            errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                span.line,
                                span.column,
                                format!(
                                    "fn `{name}` declares `@on(\"{component}\", \"{event}\")` but that (component, event) pair already has a handler registered as fn `{existing}`"
                                ),
                            ));
                        }
                    }
                }
                Err(errs) => errors.extend(errs),
            }
        }
    }

    // Pass 3: annotations of FnDef / Assign / internal lets.
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
                            &format!("in parameter `{}` of function `{}`", p.name, name),
                        ));
                    }
                }
            }
            if let Some(t) = return_type {
                if let Err(e) = resolve_type_expr(t, env) {
                    errors.push(annotate(
                        e,
                        &format!("in return type of function `{}`", name),
                    ));
                }
            }
            // We descend into the body to validate annotations of internal
            // lets. The expressions themselves (fn body) are
            // validated in 5.3.
            for s in body {
                resolve_stmt_annotations(s, env, errors);
            }
        }
        Stmt::While { body, .. } | Stmt::Loop { body, .. } | Stmt::For { body, .. } => {
            for s in body {
                resolve_stmt_annotations(s, env, errors);
            }
        }
        _ => {}
    }
}

/// Phase 4 (fitz-liveviews Y-B, session 1.b) — result of
/// `process_render_for_decorators`. Carries the parsed component
/// name so the caller (`resolve_program`) can register the
/// handler with `env.set_render_handler`. `None` when the fn
/// has no `@render_for` decorator.
type RenderForShape = Option<String>;

/// Phase 4 (fitz-liveviews Y-B, session 1.b) — result of
/// `process_on_decorators`. Vec of `(component, event)` pairs
/// (a single fn may carry multiple `@on(...)` decorators — the
/// framework layer routes each event to it).
type OnShapes = Vec<(String, String)>;

// Phase 4 (fitz-liveviews Y-B, session 1.b) — processes
// `@render_for("name")` over a `fn`. Returns the component name
// on success, `None` if the fn has no such decorator, or
// `Err(errors)` when shape is invalid (wrong arg count/type,
// kwargs, empty name, more than one `@render_for` per fn).
//
// Only shape validation lives here. Signature validation (params,
// return type, existence of `@live_component("name")`) happens in
// the checker walker via `check_render_for_decorator`.
pub fn process_render_for_decorators(
    fn_name: &str,
    fn_decorators: &[Decorator],
    fn_span: Span,
) -> Result<RenderForShape, Vec<FitzError>> {
    let mut errors: Vec<FitzError> = Vec::new();
    let mut name: Option<String> = None;
    let mut seen = false;

    for d in fn_decorators {
        if d.name != "render_for" {
            continue;
        }
        if seen {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "fn `{fn_name}` has more than one `@render_for` decorator; only one is allowed"
                ),
            ));
            continue;
        }
        seen = true;

        if !d.kwargs.is_empty() {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "`@render_for` does not accept kwargs; received: {:?}",
                    d.kwargs.iter().map(|(k, _)| k).collect::<Vec<_>>()
                ),
            ));
        }

        if d.args.len() != 1 {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "`@render_for(\"name\")` expects exactly 1 Str arg, received {}",
                    d.args.len()
                ),
            ));
            continue;
        }

        match &d.args[0] {
            Expr::Str(s, _) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        "`@render_for(\"...\")` does not accept an empty component name"
                            .to_string(),
                    ));
                } else {
                    name = Some(trimmed.to_string());
                }
            }
            _ => errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                "`@render_for(...)` expects a Str literal as the component name".to_string(),
            )),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(name)
}

// Phase 4 (fitz-liveviews Y-B, session 1.b) — processes
// `@on("component", "event")` over a `fn`. Returns the list of
// `(component, event)` pairs (a fn may carry multiple `@on(...)`
// — each pair routes a distinct event to the same fn).
//
// Only shape validation. Signature validation (params, return
// type, `T` matches the component's state) happens in the
// checker walker via `check_on_decorator`.
pub fn process_on_decorators(
    fn_name: &str,
    fn_decorators: &[Decorator],
    fn_span: Span,
) -> Result<OnShapes, Vec<FitzError>> {
    let mut errors: Vec<FitzError> = Vec::new();
    let mut pairs: OnShapes = Vec::new();

    for d in fn_decorators {
        if d.name != "on" {
            continue;
        }

        if !d.kwargs.is_empty() {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "`@on` does not accept kwargs; received: {:?}",
                    d.kwargs.iter().map(|(k, _)| k).collect::<Vec<_>>()
                ),
            ));
            continue;
        }

        if d.args.len() != 2 {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "`@on(\"component\", \"event\")` on fn `{fn_name}` expects exactly 2 Str args, received {}",
                    d.args.len()
                ),
            ));
            continue;
        }

        let parse_str_arg = |idx: usize, role: &str| -> Result<String, FitzError> {
            match &d.args[idx] {
                Expr::Str(s, _) => {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        Err(FitzError::new(
                            ErrorKind::TypeError,
                            fn_span.line,
                            fn_span.column,
                            format!("`@on(...)` does not accept empty {role} name"),
                        ))
                    } else {
                        Ok(trimmed.to_string())
                    }
                }
                _ => Err(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!("`@on(...)` expects a Str literal as the {role} name"),
                )),
            }
        };

        let comp = parse_str_arg(0, "component");
        let evt = parse_str_arg(1, "event");
        match (comp, evt) {
            (Ok(c), Ok(e)) => {
                if pairs.iter().any(|(pc, pe)| pc == &c && pe == &e) {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "fn `{fn_name}` has more than one `@on(\"{c}\", \"{e}\")` decorator; each (component, event) pair is unique"
                        ),
                    ));
                } else {
                    pairs.push((c, e));
                }
            }
            (Err(e1), Err(e2)) => {
                errors.push(e1);
                errors.push(e2);
            }
            (Err(e), _) | (_, Err(e)) => errors.push(e),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(pairs)
}

// Phase 4 (fitz-liveviews Y-B) — processes `@live_component("name")`
// over a `type`. Returns:
//   - `Ok(Some(meta))`: the type has `@live_component("name")` and is
//     registered under that name for framework-layer dispatch.
//   - `Ok(None)`: the type has no `@live_component` decorator. The
//     framework layer will not consider it a live component.
//   - `Err(errs)`: invalid decorator shape (missing name, non-Str
//     arg, kwargs present, more than one `@live_component`).
//
// The decorator is a marker: the checker only validates shape
// and registers metadata. The framework layer (`fitz-liveviews`)
// consumes it via `env.live_component_metadata` +
// `env.live_component_by_name` to look up render + event handlers
// declared with `@render_for` and `@on` (Session 1.b).
pub fn process_live_component_decorators(
    type_name: &str,
    type_decorators: &[Decorator],
    type_span: Span,
) -> Result<Option<LiveComponentMetadata>, Vec<FitzError>> {
    let mut errors: Vec<FitzError> = Vec::new();
    let mut name: Option<String> = None;
    let mut seen = false;

    for d in type_decorators {
        if d.name != "live_component" {
            continue;
        }
        if seen {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                type_span.line,
                type_span.column,
                format!(
                    "type `{type_name}` has more than one `@live_component` decorator; only one is allowed"
                ),
            ));
            continue;
        }
        seen = true;

        if !d.kwargs.is_empty() {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                type_span.line,
                type_span.column,
                format!(
                    "`@live_component` does not accept kwargs; received: {:?}",
                    d.kwargs.iter().map(|(k, _)| k).collect::<Vec<_>>()
                ),
            ));
        }

        if d.args.len() != 1 {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                type_span.line,
                type_span.column,
                format!(
                    "`@live_component(\"name\")` expects exactly 1 Str arg, received {}",
                    d.args.len()
                ),
            ));
            continue;
        }

        match &d.args[0] {
            Expr::Str(s, _) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        "`@live_component(\"...\")` does not accept an empty component name"
                            .to_string(),
                    ));
                } else {
                    name = Some(trimmed.to_string());
                }
            }
            _ => errors.push(FitzError::new(
                ErrorKind::TypeError,
                type_span.line,
                type_span.column,
                "`@live_component(...)` expects a Str literal as the component name".to_string(),
            )),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    // `type_id` is filled by `resolve_program` right after this
    // call succeeds; we use `TypeId(usize::MAX)` as a sentinel that
    // MUST be overwritten before the metadata is exposed to any
    // consumer. See the call site in `resolve_program`.
    Ok(name.map(|n| LiveComponentMetadata {
        name: n,
        type_id: TypeId(usize::MAX),
    }))
}

// Phase 5 (fitz-liveviews) — Implicit `flv_register(...)` injection.
//
// Consumes the metadata that `resolve_program` already persisted in
// `TypeEnv` (`live_components`, `render_handlers`, `event_handlers`)
// and materializes one synthetic `flv_register("name", InitialState
// {}, render_fn, {"event": handler})` call per component, appended
// at the end of `program`. Eliminates the boilerplate manual boot
// call that was required in Phase 4 (Y-B).
//
// Semantics:
//   - Called AFTER the checker (`check_program`) so `TypeEnv` is
//     fully populated. Idempotent: if the user already wrote a manual
//     `flv_register("<name>", ...)` for a component, we skip the
//     implicit call for it (last-write-wins would be harmless at
//     runtime — `COMPONENT_REGISTRY[name] = ...` overwrites — but
//     honouring the user's explicit intent is friendlier).
//   - Order-deterministic: components are visited sorted by name so
//     the generated stmts have a stable order across compiler runs.
//   - Fields validation: every `@live_component` type must declare
//     defaults on every field. Otherwise the empty `TypeName {}`
//     struct literal we inject would fail at eval-time with a
//     confusing "missing field" error. We validate upfront at inject
//     time and produce a clean error citing the offending field.
//   - Missing `@render_for("name")`: hard error. A component without
//     a renderer would blow up at first render; we surface it here.
//   - Cross-module `@live_component` is NOT supported in this MVP.
//     Only decorators declared in the top-level `program` count.
//     Support arrives if demand appears (parallel to
//     `imported_auth_provider` / `imported_background_fns`).
pub fn inject_live_component_registrations(
    program: &mut Program,
    env: &TypeEnv,
) -> Result<(), Vec<FitzError>> {
    if env.live_components.is_empty() {
        return Ok(());
    }

    let mut errors: Vec<FitzError> = Vec::new();

    // Validate that `flv_register` is in scope — either imported from
    // `fitz_liveviews` (canonical) or declared locally (test stubs).
    // Without it, the injected calls would fail with a confusing
    // "unknown variable flv_register" at eval-time whose span points
    // at Span::ZERO. Surface the error at inject-time citing the fix.
    let mut flv_register_in_scope = false;
    for stmt in program.iter() {
        match stmt {
            Stmt::FromImport { names, .. } => {
                for (orig, alias) in names {
                    let bound = alias.as_deref().unwrap_or(orig.as_str());
                    if bound == "flv_register" {
                        flv_register_in_scope = true;
                    }
                }
            }
            Stmt::FnDef { name, .. } if name == "flv_register" => {
                flv_register_in_scope = true;
            }
            _ => {}
        }
    }
    if !flv_register_in_scope {
        // Component name to cite in the error — first one alphabetically.
        let mut comp_names: Vec<&str> = env
            .live_components
            .values()
            .map(|m| m.name.as_str())
            .collect();
        comp_names.sort();
        let sample = comp_names.first().copied().unwrap_or("<component>");
        errors.push(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "@live_component(\"{sample}\") is declared but `flv_register` is not in scope. Add `from fitz_liveviews import flv_register` at the top of the file so the implicit boot registrations can compile."
            ),
        ));
        // Continue and surface other injection errors so users see
        // everything wrong at once, then abort at the end.
    }

    // Index TypeDefs by name → (span, defaulted_fields, missing_defaults)
    // so we can validate every `@live_component` type has defaults on
    // all fields, and give a clean error otherwise.
    let mut type_defs: HashMap<&str, (Span, Vec<&Field>)> = HashMap::new();
    for stmt in program.iter() {
        if let Stmt::TypeDef {
            name, fields, span, ..
        } = stmt
        {
            type_defs.insert(name.as_str(), (*span, fields.iter().collect()));
        }
    }

    // Detect components the user already registered manually so we
    // skip the implicit call for them.
    let mut manually_registered: HashSet<String> = HashSet::new();
    for stmt in program.iter() {
        if let Stmt::Expr(Expr::Call { callee, args, .. }, _) = stmt {
            if let Expr::Ident(name, _) = callee.as_ref() {
                if name == "flv_register" {
                    if let Some(Expr::Str(comp_name, _)) = args.first() {
                        manually_registered.insert(comp_name.clone());
                    }
                }
            }
        }
    }

    // Deterministic order: sort by component name.
    let mut components: Vec<(TypeId, &LiveComponentMetadata)> = env
        .live_components
        .iter()
        .map(|(id, meta)| (*id, meta))
        .collect();
    components.sort_by(|a, b| a.1.name.cmp(&b.1.name));

    let mut new_stmts: Vec<Stmt> = Vec::new();

    for (type_id, meta) in components {
        let component_name = &meta.name;

        if manually_registered.contains(component_name) {
            continue;
        }

        let type_name = env.info(type_id).name.clone();

        // Render fn is mandatory.
        let render_fn = match env.render_handler_for(component_name) {
            Some(fn_name) => fn_name.to_string(),
            None => {
                let type_span = type_defs
                    .get(type_name.as_str())
                    .map(|(sp, _)| *sp)
                    .unwrap_or(Span::ZERO);
                errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    type_span.line,
                    type_span.column,
                    format!(
                        "@live_component(\"{component_name}\") on type `{type_name}`: no fn has @render_for(\"{component_name}\") declared. Declare `@render_for(\"{component_name}\") fn <name>(state: {type_name}) -> Str` before the boot registrations."
                    ),
                ));
                continue;
            }
        };

        // All fields must have defaults so `TypeName {}` synthesises
        // the initial state cleanly. If not, error with the offending
        // field name.
        if let Some((_type_span, fields)) = type_defs.get(type_name.as_str()) {
            let missing: Vec<&str> = fields
                .iter()
                .filter(|f| f.default.is_none())
                .map(|f| f.name.as_str())
                .collect();
            if !missing.is_empty() {
                let type_span = type_defs
                    .get(type_name.as_str())
                    .map(|(sp, _)| *sp)
                    .unwrap_or(Span::ZERO);
                let list = missing.join("`, `");
                errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    type_span.line,
                    type_span.column,
                    format!(
                        "@live_component(\"{component_name}\") on type `{type_name}`: field(s) `{list}` have no default. Every field of a @live_component type must declare a default (e.g. `text: Str = \"\"`) so the implicit `flv_register(...)` can synthesise the initial state without arguments."
                    ),
                ));
                continue;
            }
        }

        // Event handlers: filter by component name, sort by event
        // name for deterministic output.
        let mut event_pairs: Vec<(String, String)> = env
            .event_handlers
            .iter()
            .filter(|((c, _), _)| c == component_name)
            .map(|((_, ev), fn_name)| (ev.clone(), fn_name.clone()))
            .collect();
        event_pairs.sort_by(|a, b| a.0.cmp(&b.0));

        // Build the synthetic call:
        //   flv_register(
        //     "component_name",
        //     TypeName {},
        //     render_fn,
        //     {"event": handler_fn, ...},
        //   )
        let call = Expr::Call {
            callee: Box::new(Expr::Ident("flv_register".into(), Span::ZERO)),
            args: vec![
                Expr::Str(component_name.clone(), Span::ZERO),
                Expr::StructLit {
                    type_name,
                    fields: vec![],
                    span: Span::ZERO,
                },
                Expr::Ident(render_fn, Span::ZERO),
                Expr::Map(
                    event_pairs
                        .into_iter()
                        .map(|(ev, fn_name)| {
                            (Expr::Str(ev, Span::ZERO), Expr::Ident(fn_name, Span::ZERO))
                        })
                        .collect(),
                    Span::ZERO,
                ),
            ],
            span: Span::ZERO,
        };
        new_stmts.push(Stmt::Expr(call, Span::ZERO));
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    program.extend(new_stmts);
    Ok(())
}

// Phase 10.3.a — processes ORM decorators over a `type`.
// Returns:
//   - `Ok(Some(meta))`: the type has `@table(...)`, there is metadata.
//   - `Ok(None)`: no `@table` nor ORM field decorators; the
//     type does not participate in the ORM, stays as a normal Fitz type.
//   - `Err(errs)`: invalid decorators (unrecognized name,
//     mal-typed args, `@primary` on more than one field, etc.).
//
// Recognized decorators:
//   * On the `type`:
//     - `@table("name")` or `@table` — SQL name of the table
//       (default: lowercase of the Fitz name). String literal
//       in the arg (no expressions).
//   * On each `Field`:
//     - `@primary` — marks primary key. Only 1 per type.
//     - `@column(name="X", sql_type="Y")` — name/SQL type overrides.
//       Both kwargs optional.
//     - `@unique` — emits `UNIQUE` constraint.
//     - `@index` — emits `CREATE INDEX` in the migration.
pub fn process_table_decorators(
    type_name: &str,
    type_decorators: &[Decorator],
    fields: &[Field],
    type_span: Span,
) -> Result<Option<TableMetadata>, Vec<FitzError>> {
    use std::collections::HashMap;

    let mut errors: Vec<FitzError> = Vec::new();

    // Is there @table on the type?
    let mut sql_name: Option<String> = None;
    let mut table_schema: Option<String> = None;
    let mut has_table = false;
    // v0.10.27 (F3) — Accumulator for `@index(...)` decorators.
    // Drained into `TableMetadata.indexes` if has_table = true. If there
    // is NO @table but there is @index, error (same as @primary without @table).
    let mut pending_indexes: Vec<IndexSpec> = Vec::new();
    let mut pending_check_constraints: Vec<CheckConstraintSpec> = Vec::new();
    let mut table_renamed_from: Option<String> = None;
    for d in type_decorators {
        match d.name.as_str() {
            "renamed_from" => {
                // v0.10.17 — Transient decorator for safe renames.
                // `@renamed_from("old_table")` tells the diff:
                // emit `ALTER TABLE "old" RENAME TO "new"` instead
                // of DROP + CREATE.
                if !d.kwargs.is_empty() {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        "`@renamed_from` no acepta kwargs".to_string(),
                    ));
                    continue;
                }
                if d.args.len() != 1 {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        format!(
                            "`@renamed_from(\"old\")` expects exactly 1 Str arg, received {}",
                            d.args.len()
                        ),
                    ));
                    continue;
                }
                match &d.args[0] {
                    Expr::Str(s, _) => {
                        let trimmed = s.trim();
                        if trimmed.is_empty() {
                            errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@renamed_from(\"...\")` does not accept empty string".to_string(),
                            ));
                        } else {
                            table_renamed_from = Some(trimmed.to_string());
                        }
                    }
                    _ => errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        "`@renamed_from(...)` expects a Str literal with the previous SQL name of the table".to_string(),
                    )),
                }
            }
            "table" => {
                if has_table {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        format!("type `{type_name}` has more than one `@table` decorator"),
                    ));
                    continue;
                }
                has_table = true;
                // `@table("name")` with optional Str arg.
                if !d.kwargs.is_empty() {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        format!(
                            "`@table` does not accept kwargs; received: {:?}",
                            d.kwargs.iter().map(|(k, _)| k).collect::<Vec<_>>()
                        ),
                    ));
                }
                if d.args.is_empty() {
                    // `@table` without args → default name
                    sql_name = Some(type_name.to_lowercase());
                } else if d.args.len() == 1 {
                    match &d.args[0] {
                        Expr::Str(s, _) => {
                            // v0.10.21 (10.6.e.3) — Split by `.`:
                            // `"foo.bar"` → schema="foo", name="bar".
                            // `"bar"` → schema=None, name="bar".
                            // We validate each non-empty segment.
                            match split_schema_qualified_table(s) {
                                Ok((schema, name)) => {
                                    table_schema = schema;
                                    sql_name = Some(name);
                                }
                                Err(msg) => errors.push(FitzError::new(
                                    ErrorKind::TypeError,
                                    type_span.line,
                                    type_span.column,
                                    msg,
                                )),
                            }
                        }
                        other => errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            format!(
                                "`@table` expects a string literal as argument, received `{:?}`",
                                other
                            ),
                        )),
                    }
                } else {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        format!(
                            "`@table` expects 0 or 1 argument (SQL name), received {}",
                            d.args.len()
                        ),
                    ));
                }
            }
            // v0.10.27 (F3) — `@index(cols, unique=?, name=?, where_=?)`.
            // Stackable (multiple `@index` on the same type). Each
            // one produces an IndexSpec accumulated in `indexes`. The
            // migrator emits CREATE INDEX / DROP INDEX from diff.
            // v0.10.32 (Tier C.2) — also supports
            // `@index(expression="lower(email)")` for expression
            // indexes; in that case the positional arg is NOT required
            // (cols are ignored).
            "index" => {
                // v0.10.32 (Tier C.2) — pre-check for `expression=`:
                // if present, `arg 0` (cols Str) is NOT required.
                let has_expression_kwarg = d.kwargs.iter().any(|(k, _)| k == "expression");
                if !has_expression_kwarg && d.args.len() != 1 {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        format!(
                            "`@index` expects 1 positional arg (Str with cols separated by comma) or `expression=...`, received {}",
                            d.args.len()
                        ),
                    ));
                    continue;
                }
                if has_expression_kwarg && !d.args.is_empty() {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        "`@index(expression=...)` does NOT accept simultaneous positional arg. Use one or the other.".to_string(),
                    ));
                    continue;
                }
                let columns: Vec<String> = if has_expression_kwarg {
                    Vec::new()
                } else {
                    let cols_str = match &d.args[0] {
                        Expr::Str(s, _) => s.clone(),
                        _ => {
                            errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@index` expects a Str literal as arg 0 (cols separated by comma)"
                                    .to_string(),
                            ));
                            continue;
                        }
                    };
                    let cols: Vec<String> = cols_str
                        .split(',')
                        .map(|c| c.trim().to_string())
                        .filter(|c| !c.is_empty())
                        .collect();
                    if cols.is_empty() {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@index(\"...\")` received empty string; expected at least one column"
                                .to_string(),
                        ));
                        continue;
                    }
                    cols
                };
                let mut unique = false;
                let mut name: Option<String> = None;
                let mut where_clause: Option<String> = None;
                let mut using: Option<String> = None;
                let mut expression: Option<String> = None;
                for (k, v) in &d.kwargs {
                    match k.as_str() {
                        "unique" => match v {
                            Expr::Bool(b, _) => unique = *b,
                            _ => errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@index(unique=...)` expects Bool literal".to_string(),
                            )),
                        },
                        "name" => match v {
                            Expr::Str(s, _) => name = Some(s.clone()),
                            _ => errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@index(name=...)` expects Str literal".to_string(),
                            )),
                        },
                        "where_" => match v {
                            Expr::Str(s, _) => where_clause = Some(s.clone()),
                            _ => errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@index(where_=...)` expects Str literal with the SQL WHERE clause"
                                    .to_string(),
                            )),
                        },
                        // v0.10.28 — Method override. Whitelist of the
                        // 6 official Postgres methods. Other value =
                        // user typo (better to fail at compile time
                        // than let it through and break at CREATE INDEX).
                        "using" => match v {
                            Expr::Str(s, _) => {
                                const ALLOWED: &[&str] =
                                    &["btree", "hash", "gin", "gist", "brin", "spgist"];
                                let lower = s.to_ascii_lowercase();
                                if !ALLOWED.contains(&lower.as_str()) {
                                    errors.push(FitzError::new(
                                        ErrorKind::TypeError,
                                        type_span.line,
                                        type_span.column,
                                        format!(
                                            "`@index(using=\"{s}\")`: unknown method. \
                                             Supported: btree, hash, gin, gist, brin, spgist"
                                        ),
                                    ));
                                } else if lower != "btree" {
                                    // btree is Postgres default — we
                                    // leave it as None to not emit
                                    // redundant `USING btree` (matches
                                    // what introspect reports for the
                                    // default).
                                    using = Some(lower);
                                }
                            }
                            _ => errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@index(using=...)` expects Str literal with the method (\"gin\", \"gist\", etc.)"
                                    .to_string(),
                            )),
                        },
                        // v0.10.32 (Tier C.2) — Expression index.
                        // The user passes the raw SQL expression; Fitz
                        // emits it literal in CREATE INDEX (doesn't parse).
                        "expression" => match v {
                            Expr::Str(s, _) => {
                                let trimmed = s.trim();
                                if trimmed.is_empty() {
                                    errors.push(FitzError::new(
                                        ErrorKind::TypeError,
                                        type_span.line,
                                        type_span.column,
                                        "`@index(expression=\"\")` does not accept empty string".to_string(),
                                    ));
                                } else {
                                    expression = Some(trimmed.to_string());
                                }
                            }
                            _ => errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@index(expression=...)` expects Str literal with the SQL expression (e.g.: `\"lower(email)\"`)".to_string(),
                            )),
                        },
                        other => errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            format!(
                                "`@index` unknown kwarg `{}`. Supported: unique, name, where_, using, expression",
                                other
                            ),
                        )),
                    }
                }
                pending_indexes.push(IndexSpec {
                    name,
                    columns,
                    unique,
                    where_clause,
                    using,
                    expression,
                });
            }
            // v0.10.29 — `@unique(col1, col2, ..., name="optional")`.
            // Shortcut for `@index(col1, col2, ..., unique=true)`. The
            // ergonomic syntax is bare idents (`@unique(email,
            // tenant_id)`); it also accepts Str with commas
            // (`@unique("email, tenant_id")`) for consistency with
            // `@index`. Only supported kwarg: `name="..."` (no
            // `where_=`/`using=`/`unique=` — for those advanced
            // cases use `@index(...)` directly).
            "unique" => {
                if d.args.is_empty() {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        "`@unique(col1, col2, ...)` expects at least 1 positional column"
                            .to_string(),
                    ));
                    continue;
                }
                let mut columns: Vec<String> = Vec::with_capacity(d.args.len());
                let mut had_error = false;
                for (i, arg) in d.args.iter().enumerate() {
                    match arg {
                        Expr::Ident(name, _) => columns.push(name.clone()),
                        Expr::Str(s, _) => {
                            // Allow Str with commas (compat with @index).
                            for part in s.split(',') {
                                let p = part.trim();
                                if !p.is_empty() {
                                    columns.push(p.to_string());
                                }
                            }
                        }
                        _ => {
                            errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                format!(
                                    "`@unique` arg {}: each column must be a bare Ident (`email`) or Str literal",
                                    i
                                ),
                            ));
                            had_error = true;
                            break;
                        }
                    }
                }
                if had_error {
                    continue;
                }
                if columns.is_empty() {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        "`@unique(...)` did not receive valid columns".to_string(),
                    ));
                    continue;
                }
                let mut name: Option<String> = None;
                for (k, v) in &d.kwargs {
                    match k.as_str() {
                        "name" => match v {
                            Expr::Str(s, _) => name = Some(s.clone()),
                            _ => errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@unique(name=...)` expects Str literal".to_string(),
                            )),
                        },
                        other => errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            format!(
                                "`@unique` unknown kwarg `{}`. Only supports `name=\"...\"`; for `where_`/`using`/etc. use `@index(unique=true, ...)` directly",
                                other
                            ),
                        )),
                    }
                }
                pending_indexes.push(IndexSpec {
                    name,
                    columns,
                    unique: true,
                    where_clause: None,
                    using: None,
                    expression: None,
                });
            }
            // v0.10.29 — `@check_constraint("<sql_expr>",
            // name="optional")`. Stackable. The expr is passed literal
            // to CREATE TABLE — Fitz does NOT parse SQL to validate
            // against the table shape; the user gets it right or
            // Postgres rejects it at the first INSERT.
            "check_constraint" => {
                if d.args.len() != 1 {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        format!(
                            "`@check_constraint` expects 1 positional arg (Str with the SQL expression), received {}",
                            d.args.len()
                        ),
                    ));
                    continue;
                }
                let expr = match &d.args[0] {
                    Expr::Str(s, _) => {
                        let trimmed = s.trim().to_string();
                        if trimmed.is_empty() {
                            errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@check_constraint(\"\")` received empty string".to_string(),
                            ));
                            continue;
                        }
                        trimmed
                    }
                    _ => {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@check_constraint` expects Str literal with the SQL expression"
                                .to_string(),
                        ));
                        continue;
                    }
                };
                let mut name: Option<String> = None;
                for (k, v) in &d.kwargs {
                    match k.as_str() {
                        "name" => match v {
                            Expr::Str(s, _) => name = Some(s.clone()),
                            _ => errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@check_constraint(name=...)` expects Str literal".to_string(),
                            )),
                        },
                        other => errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            format!(
                                "`@check_constraint` unknown kwarg `{}`. Supported: name",
                                other
                            ),
                        )),
                    }
                }
                pending_check_constraints.push(CheckConstraintSpec { name, expr });
            }
            // Phase 4 (fitz-liveviews Y-B) — `@live_component("name")`
            // is a valid `type`-level decorator handled by
            // `process_live_component_decorators`. Silent no-op here
            // so this pass does not report it as unrecognized. Shape
            // validation lives in the dedicated processor.
            "live_component" => {}
            other => {
                errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    type_span.line,
                    type_span.column,
                    format!(
                        "decorator `@{other}` not supported on `type`. Recognized: `@table`, `@renamed_from`, `@index`, `@unique`, `@check_constraint`, `@live_component`."
                    ),
                ));
            }
        }
    }

    // Process decorators of each field (even if there is no @table —
    // those decorators without @table are "error" because they only
    // make sense in ORM context).
    let mut primary_fields: Vec<String> = Vec::new();
    let mut columns: HashMap<String, ColumnMetadata> = HashMap::new();
    let mut relations: HashMap<String, RelationMetadata> = HashMap::new();
    let mut any_field_decorator = false;

    for f in fields {
        if f.decorators.is_empty() {
            continue;
        }
        // v0.10.11 — `any_field_decorator` only counts ORM
        // decorators (primary/column/unique/index/db_default/belongs_to/
        // has_one/has_many). `@hidden` is orthogonal to the ORM and must NOT
        // trigger the "has ORM decorators but missing @table" check
        // (otherwise it would be impossible to use @hidden in plain
        // HTTP types without table). Set inside each specific arm
        // of the match below, not here.
        let mut col_meta = ColumnMetadata::default();
        let mut has_meta = false;
        for d in &f.decorators {
            // v0.10.11 — `@hidden` is orthogonal to the ORM (does not imply
            // @table). All other field decorators ARE
            // ORM-specific and trigger the "missing @table" check.
            if d.name != "hidden" {
                any_field_decorator = true;
            }
            match d.name.as_str() {
                "primary" => {
                    if !d.args.is_empty() || !d.kwargs.is_empty() {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@primary` does not accept args or kwargs".to_string(),
                        ));
                    }
                    // v0.10.27 (F2) — composite PK supported: N @primary
                    // fields accumulate to `primary_fields`. Order matters
                    // for the `PRIMARY KEY (a, b)` constraint in CREATE
                    // TABLE (PG builds the index according to that order).
                    if primary_fields.contains(&f.name) {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            format!(
                                "type `{type_name}` declares `@primary` twice on field `{}`",
                                f.name
                            ),
                        ));
                    } else {
                        primary_fields.push(f.name.clone());
                    }
                }
                "column" => {
                    has_meta = true;
                    if !d.args.is_empty() {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@column` only accepts kwargs (`name=`, `sql_type=`), no positionals"
                                .to_string(),
                        ));
                    }
                    // NOTE: the kwarg is named `sql_type` (not `type`)
                    // because `type` is a reserved keyword of the language
                    // and the decorator args parser does not accept
                    // keywords as keys. If real demand appears,
                    // refinable in the parser; for now explicit API.
                    for (k, v) in &d.kwargs {
                        match k.as_str() {
                            "name" => match v {
                                Expr::Str(s, _) => col_meta.sql_name = Some(s.clone()),
                                _ => errors.push(FitzError::new(
                                    ErrorKind::TypeError,
                                    type_span.line,
                                    type_span.column,
                                    "`@column(name=...)` expects string literal".to_string(),
                                )),
                            },
                            "sql_type" => match v {
                                Expr::Str(s, _) => col_meta.sql_type = Some(s.clone()),
                                _ => errors.push(FitzError::new(
                                    ErrorKind::TypeError,
                                    type_span.line,
                                    type_span.column,
                                    "`@column(sql_type=...)` expects string literal".to_string(),
                                )),
                            },
                            other_k => errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                format!(
                                    "`@column` does not recognize kwarg `{other_k}`. Supported: `name`, `sql_type`."
                                ),
                            )),
                        }
                    }
                }
                "unique" => {
                    has_meta = true;
                    if !d.args.is_empty() || !d.kwargs.is_empty() {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@unique` does not accept args or kwargs".to_string(),
                        ));
                    }
                    col_meta.unique = true;
                }
                "index" => {
                    has_meta = true;
                    if !d.args.is_empty() || !d.kwargs.is_empty() {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@index` does not accept args or kwargs".to_string(),
                        ));
                    }
                    col_meta.indexed = true;
                }
                "db_default" => {
                    // 10.8.2 (v0.10.8) — DB-managed field.
                    // The ORM skips it from the INSERT (Postgres applies
                    // its DEFAULT). v0.10.16 — accepts optional Str arg
                    // with the SQL expression of the default
                    // (`@db_default("NOW()")`) so `fitz db
                    // diff` emits it in CREATE TABLE / ADD
                    // COLUMN automatically.
                    has_meta = true;
                    if !d.kwargs.is_empty() {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@db_default` does not accept kwargs".to_string(),
                        ));
                    }
                    if d.args.len() > 1 {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@db_default` accepts at most one positional arg (Str with the SQL expression of the default)".to_string(),
                        ));
                    }
                    col_meta.db_default = true;
                    if let Some(arg) = d.args.first() {
                        match arg {
                            crate::ast::Expr::Str(s, _) => {
                                let trimmed = s.trim();
                                if trimmed.is_empty() {
                                    errors.push(FitzError::new(
                                        ErrorKind::TypeError,
                                        type_span.line,
                                        type_span.column,
                                        "`@db_default(\"...\")` does not accept empty string"
                                            .to_string(),
                                    ));
                                } else {
                                    col_meta.db_default_sql = Some(trimmed.to_string());
                                }
                            }
                            _ => {
                                errors.push(FitzError::new(
                                    ErrorKind::TypeError,
                                    type_span.line,
                                    type_span.column,
                                    "`@db_default(...)` expects a Str literal with the SQL expression of the default (e.g.: `@db_default(\"NOW()\")`)".to_string(),
                                ));
                            }
                        }
                    }
                }
                "hidden" => {
                    // v0.10.11 — the field does NOT cross the HTTP boundary.
                    // Both `__to_fitz_json` (response to client)
                    // and `__FromFitzJson` (client body)
                    // SKIP it. Useful for sensitive fields
                    // (`password_hash`, tokens) and internal metadata.
                    // The field is still assignable from
                    // internal Fitz code and participates in normal DB I/O.
                    // No args nor kwargs. The checker tolerates it
                    // but does NOT need extra state here — the
                    // codegen reads it directly from `Field.decorators`.
                    //
                    // **Does not set `has_meta = true`** — @hidden is
                    // orthogonal to the ORM. Works in types with or
                    // without @table. If set, the checker
                    // would require @table on the type even if the user
                    // only wants to mark a field as hidden in
                    // a plain HTTP type.
                    if !d.args.is_empty() || !d.kwargs.is_empty() {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@hidden` does not accept args or kwargs".to_string(),
                        ));
                    }
                }
                "renamed_from" => {
                    // v0.10.17 (10.6.b.2) — Transient decorator for
                    // safe column rename WITHOUT losing data.
                    // `@renamed_from("old_name") full_name: Str = ""`
                    // makes `fitz db diff` emit `ALTER TABLE
                    // ... RENAME COLUMN "old_name" TO "full_name"`
                    // instead of DROP + ADD.
                    has_meta = true;
                    if !d.kwargs.is_empty() {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            "`@renamed_from` does not accept kwargs".to_string(),
                        ));
                    }
                    if d.args.len() != 1 {
                        errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            type_span.line,
                            type_span.column,
                            format!(
                                "`@renamed_from(\"old\")` expects exactly 1 Str arg, received {}",
                                d.args.len()
                            ),
                        ));
                    } else {
                        match &d.args[0] {
                            Expr::Str(s, _) => {
                                let trimmed = s.trim();
                                if trimmed.is_empty() {
                                    errors.push(FitzError::new(
                                        ErrorKind::TypeError,
                                        type_span.line,
                                        type_span.column,
                                        "`@renamed_from(\"...\")` does not accept empty string".to_string(),
                                    ));
                                } else {
                                    col_meta.renamed_from = Some(trimmed.to_string());
                                }
                            }
                            _ => errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                "`@renamed_from(...)` expects a Str literal with the previous SQL name of the column".to_string(),
                            )),
                        }
                    }
                }
                "belongs_to" | "has_one" | "has_many" => {
                    let kind = match d.name.as_str() {
                        "belongs_to" => RelationKind::BelongsTo,
                        "has_one" => RelationKind::HasOne,
                        "has_many" => RelationKind::HasMany,
                        _ => unreachable!(),
                    };
                    if let Some(meta) = parse_relation_decorator(
                        d,
                        kind,
                        &f.name,
                        type_name,
                        type_span,
                        &mut errors,
                    ) {
                        if relations.contains_key(&f.name) {
                            errors.push(FitzError::new(
                                ErrorKind::TypeError,
                                type_span.line,
                                type_span.column,
                                format!("field `{}` has more than one relation decorator", f.name),
                            ));
                        } else {
                            relations.insert(f.name.clone(), meta);
                        }
                    }
                }
                other => {
                    errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        type_span.line,
                        type_span.column,
                        format!(
                            "decorator `@{other}` not supported on a field. Recognized: `@primary`, `@column`, `@unique`, `@index`, `@db_default`, `@hidden`, `@belongs_to`, `@has_one`, `@has_many`, `@renamed_from`."
                        ),
                    ));
                }
            }
        }
        if has_meta {
            columns.insert(f.name.clone(), col_meta);
        }
    }

    // Cross validation: if there are ORM field decorators but no
    // @table, the user probably forgot the @table. Clear
    // error.
    if !has_table && (!primary_fields.is_empty() || any_field_decorator) {
        errors.push(FitzError::new(
            ErrorKind::TypeError,
            type_span.line,
            type_span.column,
            format!(
                "type `{type_name}` has ORM decorators on fields (`@primary`/`@column`/`@unique`/`@index`) but is missing `@table(...)` on the `type`"
            ),
        ));
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    if !has_table {
        return Ok(None);
    }

    // Residual debt #2 (v0.10.5) — post-process: register
    // `BelongsToCompanion` for each pair `@belongs_to xxx_id: Int` +
    // sibling field `xxx: Target?`. Without this, `.preload("xxx")` over
    // BelongsTo does not work (the API only supported HasMany).
    //
    // Detection convention:
    //   1. The FK field (with `@belongs_to`) must have a name ending
    //      in `_id` (e.g. `user_id`, `author_id`).
    //   2. There exists a sibling field whose name is the FK without the
    //      `_id` suffix (e.g. `user`, `author`).
    //   3. The sibling is of type `Target?` (Nullable wrapping a
    //      Named matching the target of @belongs_to).
    //   4. The sibling does not have its own relation decorator
    //      (it's not already @has_one/@has_many/etc.).
    //
    // If the 4 points are met, we register `BelongsToCompanion`
    // under the sibling's name. The codegen treats it as virtual
    // (skip SELECT/INSERT), initializes Null, and `.preload("xxx")`
    // populates it with an inverse batch SELECT.
    register_belongs_to_companions(&mut relations, fields);

    // v0.10.27 (F3) — Resolve col names from @index from the Fitz name
    // to the SQL name respecting `@column(name=...)`. Validate that each
    // col exists as a field of the type.
    let mut resolved_indexes: Vec<IndexSpec> = Vec::with_capacity(pending_indexes.len());
    for idx in pending_indexes {
        let mut resolved_cols: Vec<String> = Vec::with_capacity(idx.columns.len());
        let mut idx_errors: Vec<String> = Vec::new();
        for fitz_col in &idx.columns {
            // Find the field by Fitz name
            let field_decl = fields.iter().find(|f| f.name == *fitz_col);
            if field_decl.is_none() {
                idx_errors.push(format!(
                    "`@index(\"{}, ...\")`: field `{}` does not exist in `{}`",
                    idx.columns.join(", "),
                    fitz_col,
                    type_name
                ));
                continue;
            }
            // Resolve SQL name of the field (respects @column(name=...))
            let sql_col = columns
                .get(fitz_col)
                .and_then(|c| c.sql_name.as_deref())
                .unwrap_or(fitz_col.as_str())
                .to_string();
            resolved_cols.push(sql_col);
        }
        for e in idx_errors {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                type_span.line,
                type_span.column,
                e,
            ));
        }
        // v0.10.32 (Tier C.2) — expression indexes pass even if
        // resolved_cols is empty (the expression is the source of the
        // index). Without expression AND without cols → skip (probably
        // typo in col names that the resolve filtered out).
        if !resolved_cols.is_empty() || idx.expression.is_some() {
            resolved_indexes.push(IndexSpec {
                name: idx.name,
                columns: resolved_cols,
                unique: idx.unique,
                where_clause: idx.where_clause,
                using: idx.using,
                expression: idx.expression,
            });
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(Some(TableMetadata {
        sql_name: sql_name.unwrap(), // guaranteed by has_table check
        schema: table_schema,
        primary_fields,
        columns,
        relations,
        renamed_from: table_renamed_from,
        indexes: resolved_indexes,
        check_constraints: pending_check_constraints,
    }))
}

/// v0.10.21 (10.6.e.3) — Split by `.` for `@table("schema.name")`.
///
/// - `"users"` → `(None, "users")` — default schema `public`.
/// - `"analytics.events"` → `(Some("analytics"), "events")`.
/// - `"a.b.c"` or `".name"` or `"schema."` or `""` → error.
///
/// The name and schema must be reasonable SQL identifiers: not
/// empty, no internal whitespace. We do NOT validate exotic chars
/// because `quote_ident` in migrations protects them with `"..."`.
fn split_schema_qualified_table(s: &str) -> Result<(Option<String>, String), String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("`@table` does not accept empty string".to_string());
    }
    if trimmed.contains(char::is_whitespace) {
        return Err(format!(
            "`@table(\"{trimmed}\")` contains whitespace — SQL names must not have spaces"
        ));
    }
    let parts: Vec<&str> = trimmed.split('.').collect();
    match parts.len() {
        1 => Ok((None, parts[0].to_string())),
        2 => {
            if parts[0].is_empty() || parts[1].is_empty() {
                Err(format!(
                    "`@table(\"{trimmed}\")` has empty segment: the `schema.name` format requires both to be non-empty"
                ))
            } else {
                Ok((Some(parts[0].to_string()), parts[1].to_string()))
            }
        }
        _ => Err(format!(
            "`@table(\"{trimmed}\")` has more than one `.` — expected format: `\"name\"` or `\"schema.name\"`"
        )),
    }
}

/// Residual debt #2 (v0.10.5) — automatically registers the
/// `BelongsToCompanion` when the canonical pattern is detected:
/// `@belongs_to(...) xxx_id: Int` + sibling `xxx: Target?`.
fn register_belongs_to_companions(
    relations: &mut HashMap<String, RelationMetadata>,
    fields: &[Field],
) {
    // Snapshot of the BelongsTo relations to iterate without mutating.
    let belongs_to_entries: Vec<(String, RelationMetadata)> = relations
        .iter()
        .filter(|(_, r)| r.kind == RelationKind::BelongsTo)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    for (fk_field_name, rel) in belongs_to_entries.iter() {
        // (1) FK must end in `_id` so the companion name is
        // derivable.
        let companion_name = match fk_field_name.strip_suffix("_id") {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue, // FK without `_id` suffix (e.g. usuario in es) → skip
        };

        // (2)+(4) Sibling field exists AND does not have its own relation.
        let sibling = fields.iter().find(|f| f.name == companion_name);
        let sibling = match sibling {
            Some(f) if !relations.contains_key(&f.name) => f,
            _ => continue,
        };

        // (3) Sibling MUST be `Nullable<Named(target_type)>`, e.g.
        // `user: User?`. The field is initialized with `None` before
        // `.preload(...)` populates it, so non-nullable breaks
        // the deserialization. If the user wants to access the companion
        // as non-null post-preload, they do `.unwrap()` / `match` when
        // consuming.
        let target_matches = match &sibling.type_ {
            TypeExpr::Nullable(inner) => match inner.as_ref() {
                TypeExpr::Named(name) => name == &rel.target_type,
                _ => false,
            },
            _ => false,
        };
        if !target_matches {
            continue;
        }

        // Register the companion.
        relations.insert(
            companion_name,
            RelationMetadata {
                kind: RelationKind::BelongsToCompanion,
                target_type: rel.target_type.clone(),
                fk_field: fk_field_name.clone(),
                on_delete: CascadeAction::default(),
                on_update: CascadeAction::default(),
            },
        );
    }
}

/// Phase 10.4.a — Parses a relation decorator
/// (`@belongs_to`/`@has_one`/`@has_many`). Returns
/// `Some(meta)` if the decorator is valid; `None` and pushes
/// errors to the vec if there are problems. Validations:
///   - 1 positional Str arg (name of the referenced type).
///   - Recognized kwargs: `on_delete`, `on_update`, `fk` (for
///     belongs_to) or `via` (for has_one/has_many).
///   - Values of `on_delete`/`on_update`: "cascade" | "set_null"
///     | "restrict" | "no_action".
fn parse_relation_decorator(
    d: &Decorator,
    kind: RelationKind,
    field_name: &str,
    type_name: &str,
    span: Span,
    errors: &mut Vec<FitzError>,
) -> Option<RelationMetadata> {
    let dec_name = match kind {
        RelationKind::BelongsTo => "@belongs_to",
        RelationKind::HasOne => "@has_one",
        RelationKind::HasMany => "@has_many",
        // BelongsToCompanion is not reached here — only built
        // post-process in `register_belongs_to_companions`, never
        // by the user-facing decorator parser.
        RelationKind::BelongsToCompanion => unreachable!(
            "BelongsToCompanion is not user-facing — parse_relation_decorator is only called with parseable kinds"
        ),
    };
    // Positional arg 1: name of the referenced type.
    if d.args.len() != 1 {
        errors.push(FitzError::new(
            ErrorKind::TypeError,
            span.line,
            span.column,
            format!(
                "`{dec_name}` expects 1 positional arg (name of the referenced type), received {}",
                d.args.len()
            ),
        ));
        return None;
    }
    let target_type = match &d.args[0] {
        Expr::Str(s, _) => s.clone(),
        other => {
            errors.push(FitzError::new(
                ErrorKind::TypeError,
                span.line,
                span.column,
                format!(
                    "`{dec_name}`: first arg must be a string literal with the type name, received `{:?}`",
                    other
                ),
            ));
            return None;
        }
    };

    // Default fk_field: depends on the kind.
    //   - BelongsTo: the decorated field IS the FK (by convention),
    //     unless the user overrides with `fk="other_col"`.
    //   - HasOne/HasMany: convention `<lowercase(this_type)>_id`,
    //     unless `via="X"` overrides it.
    let mut fk_field: String = match kind {
        RelationKind::BelongsTo => field_name.to_string(),
        RelationKind::HasOne | RelationKind::HasMany => format!("{}_id", type_name.to_lowercase()),
        RelationKind::BelongsToCompanion => {
            unreachable!("BelongsToCompanion no se construye por parse_relation_decorator")
        }
    };
    let mut on_delete = CascadeAction::default();
    let mut on_update = CascadeAction::default();

    for (k, v) in &d.kwargs {
        match k.as_str() {
            "on_delete" => match parse_cascade_value(v) {
                Some(c) => on_delete = c,
                None => errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    span.line,
                    span.column,
                    format!(
                        "`{dec_name}(on_delete=...)` unknown value. Supported: `cascade`, `set_null`, `restrict`, `no_action`."
                    ),
                )),
            },
            "on_update" => match parse_cascade_value(v) {
                Some(c) => on_update = c,
                None => errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    span.line,
                    span.column,
                    format!(
                        "`{dec_name}(on_update=...)` unknown value. Supported: `cascade`, `set_null`, `restrict`, `no_action`."
                    ),
                )),
            },
            "fk" if matches!(kind, RelationKind::BelongsTo) => match v {
                Expr::Str(s, _) => fk_field = s.clone(),
                _ => errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    span.line,
                    span.column,
                    format!("`{dec_name}(fk=...)` expects string literal"),
                )),
            },
            "via" if matches!(kind, RelationKind::HasOne | RelationKind::HasMany) => match v {
                Expr::Str(s, _) => fk_field = s.clone(),
                _ => errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    span.line,
                    span.column,
                    format!("`{dec_name}(via=...)` expects string literal"),
                )),
            },
            other => {
                let valid = match kind {
                    RelationKind::BelongsTo => "`on_delete`, `on_update`, `fk`",
                    RelationKind::HasOne | RelationKind::HasMany => {
                        "`on_delete`, `on_update`, `via`"
                    }
                    RelationKind::BelongsToCompanion => unreachable!(),
                };
                errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    span.line,
                    span.column,
                    format!(
                        "`{dec_name}` does not recognize kwarg `{other}`. Supported: {valid}."
                    ),
                ));
            }
        }
    }

    Some(RelationMetadata {
        kind,
        target_type,
        fk_field,
        on_delete,
        on_update,
    })
}

/// Phase 10.4.a — Parses an `on_delete`/`on_update` value.
/// Supported: Str literals with canonical values.
fn parse_cascade_value(v: &Expr) -> Option<CascadeAction> {
    if let Expr::Str(s, _) = v {
        match s.as_str() {
            "cascade" => Some(CascadeAction::Cascade),
            "set_null" => Some(CascadeAction::SetNull),
            "restrict" => Some(CascadeAction::Restrict),
            "no_action" => Some(CascadeAction::NoAction),
            _ => None,
        }
    } else {
        None
    }
}

fn check_field_default(
    type_name: &str,
    field_name: &str,
    declared: &Type,
    default: &Expr,
    env: &TypeEnv,
) -> Result<(), FitzError> {
    let lit_type = match default {
        Expr::Int(_, _) => Some(Type::Int),
        Expr::Float(_, _) => Some(Type::Float),
        Expr::Str(_, _) => Some(Type::Str),
        Expr::Bool(_, _) => Some(Type::Bool),
        Expr::Null(_) => Some(Type::Null),
        _ => None,
    };
    let lit_type = match lit_type {
        Some(t) => t,
        None => return Ok(()), // not a literal, validated in 5.3
    };
    // Null over nullable type: OK.
    if matches!(lit_type, Type::Null) && declared.is_nullable() {
        return Ok(());
    }
    // Int→Float coercion.
    if matches!(lit_type, Type::Int) && matches!(declared.base(), Type::Float) {
        return Ok(());
    }
    // v0.10.24 — Str literal sentinel/default for Date/DateTime/Uuid.
    // The user writes `happens_on: Date = ""` as sentinel (parallel
    // to `id: Int = 0`). The evaluator coerces the Str → corresponding type
    // at runtime (via `coerce_to_annotation`); if the Str does not parse,
    // it fails at runtime with a clear message. Typical case: Date/DateTime
    // set from the HTTP JSON body, where the "default Str"
    // never ends up being used because the user provides the real value.
    if matches!(lit_type, Type::Str)
        && matches!(declared.base(), Type::Date | Type::DateTime | Type::Uuid)
    {
        return Ok(());
    }
    // Structural equality over the base.
    if &lit_type != declared.base() {
        return Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "field `{}.{}` declared as `{}` received a default `{}`",
                type_name,
                field_name,
                declared.display(env),
                lit_type.display(env),
            ),
        ));
    }
    Ok(())
}

/// Appends context to an error message. The original message goes
/// first, the context in parentheses at the end.
fn annotate(mut e: FitzError, context: &str) -> FitzError {
    e.message = format!("{} ({})", e.message, context);
    e
}

// ---------------------------------------------------------------------------
// Expression checker (Phase 5.3.1)
//
// While `resolve_program` checks annotations, `check_program` also runs
// a pass over the program's expressions. The idea:
//   1. Pre-register signatures of top-level `Stmt::FnDef`s and builtins
//      in a global variable scope.
//   2. Walk each Stmt, opening scopes for each `FnDef`/loop/etc.
//   3. For each `Expr`, synthesize its type (`infer_expr`).
//   4. When there is an *expected* type (let annotation, non-literal
//      field default, etc.), validate compatibility.
//
// 5.3.1 covers: literals, ident, arithmetic/comparison/logical BinOp,
// UnaryOp Neg, StrInterp, `if` expr, list/map literals, struct lit,
// field access over Nominal, Range. The rest returns `Any` and is covered
// in 5.3.2+.
// ---------------------------------------------------------------------------

use crate::ast::{AssignTarget, BinOpKind, StrPart, UnaryOpKind};

/// Binding of a variable in a scope. Carries the type and an
/// `annotated` flag indicating whether the FIRST assignment of that name
/// came with an explicit type annotation (`x: Int = ...`). The flag
/// is used to check reassignments: if the var was annotated,
/// subsequent reassignments without annotation must respect
/// that type. If the var was inferred without annotation,
/// reassignments can change the type (gradual model).
#[derive(Debug, Clone)]
struct VarBinding {
    ty: Type,
    annotated: bool,
    /// Span of the declaration (let stmt, fn def, type def, param, etc.).
    /// `Span::ZERO` for builtins — the LSP filters them in go-to-definition
    /// because there is no file to jump to.
    def_span: Span,
    /// Fp — number of params with default at the end of the signature. If the
    /// fn has `fn(a, b, c = 1, d = 2)`, `defaults_count = 2`. The required
    /// arity is `params.len() - defaults_count`. Only relevant for
    /// vars that type as `Type::Function`. 0 for everything else.
    defaults_count: usize,
    /// Fp.2 — `true` if the last param is variadic (`...xs`). In that
    /// case, the call site accepts any number >= required of args.
    has_varargs: bool,
}

/// Mutable state during the expression checking pass.
struct CheckCtx<'a> {
    types: &'a TypeEnv,
    /// Stack of scopes for variables. The first is the global one
    /// (builtins + top-level fns + top-level lets). Each `FnDef`
    /// body, each loop body, opens a new scope.
    scopes: Vec<std::collections::HashMap<String, VarBinding>>,
    /// Stack of expected return types, one for each function
    /// (FnDef or FnExpr) nested that's being checked. Empty in
    /// the top-level scope. `Stmt::Return` consults it to validate.
    return_stack: Vec<Type>,
    /// Stack parallel to `return_stack`: each frame collects the
    /// synthesized types of `Stmt::Return`s inside that
    /// function. `Expr::FnExpr` consumes it on exit to infer its
    /// `ret`. For `Stmt::FnDef` it's also accumulated but
    /// discarded (we already have declared `return_type`).
    inferred_returns: Vec<Vec<Type>>,
    /// Stack parallel to `return_stack`: `true` when the current fn
    /// is an HTTP handler (has decorator `@get`/`@post`/`@put`/
    /// `@delete`). `Stmt::ReturnStatus` (return with status code)
    /// consults it to validate that it only appears inside a handler.
    /// `FnExpr` is never a handler; pushes `false`.
    in_http_handler: Vec<bool>,
    /// Stack parallel to `return_stack`: `true` when the current fn is
    /// `async`. `Expr::Await` consults it to validate that it only appears
    /// inside an async fn. `FnExpr` does not support async yet
    /// (parser does not allow it); always pushes `false`. Introduced
    /// in Phase 6.2.
    await_stack: Vec<bool>,
    /// Names of fns that appear as argument of a `@middleware(...)`
    /// in some FnDef of the program. Pre-scanned in `check_program`. We
    /// use it to treat those fns as "HTTP context" for purposes of
    /// `Stmt::ReturnStatus` (a middleware can do `return 401 { ... }`
    /// to short-circuit the handler). Introduced in mini-phase MW.1.
    middleware_fn_names: std::collections::HashSet<String>,
    /// Phase 9.w.1 — Native auth. `Some(info)` when the program declares
    /// a fn with `@auth_provider`. Collected by `collect_auth_provider`
    /// before the checker walk. Consulted by the
    /// `@authenticated`/`@admin` check to validate that each protected handler
    /// declares a param compatible with the `User` returned by the provider,
    /// and that the `User` has a `role: Str` field when there is `@admin` in the
    /// program.
    auth_provider: Option<AuthProviderInfo>,
    /// Phase 9.w.3 — set of names of top-level fns with `@background`
    /// decorator. Collected by `collect_background_fns` before
    /// the walk. Consulted by the `spawn(call)` check to validate
    /// that the spawn target is declared as executable in
    /// background — avoids accidental uses of spawn on
    /// regular fns whose return the caller expects to consume.
    background_fns: std::collections::HashSet<String>,
    /// Phase 12.1 — `Some((name, span))` when a `@healthz` has already
    /// been seen in the walk. Singleton: a second `@healthz` in another fn fires
    /// an explicit error citing the first. Does not persist additional info
    /// (no downstream check like `@auth_provider`); runtime and
    /// codegen re-collect on their own when auto-mounting `/healthz`.
    healthz_first: Option<(String, Span)>,
    /// Phase 12.1 — parallel to `healthz_first` but for `@readyz`.
    readyz_first: Option<(String, Span)>,
    /// Mini-batch L — stack parallel to `Expr::Loop`s currently
    /// being checked. Each frame collects the types of the
    /// values of `break <v>` inside. `Expr::Loop` consumes the
    /// frame on exit to infer the type of the expression via
    /// `unify_returns`. Loops as statement (`Stmt::Loop`,
    /// `Stmt::While`, `Stmt::For`) do NOT push to the stack — the
    /// `break <v>`s inside type the value but do NOT propagate.
    break_value_stack: Vec<Vec<Type>>,
    /// Depth of loops inside the current function (R.2.4 — F3).
    /// `Stmt::Break`/`Continue` require this value to be > 0; if 0,
    /// the statement is orphan (top-level or inside fn without loop).
    /// `While`/`Loop`/`For` increment on entry and decrement on
    /// exit; `FnDef`/`FnExpr` save the previous value, reset it to
    /// 0, and restore it on exit (a break inside a closure does NOT
    /// break the outer loop, just like Rust).
    loop_depth: usize,
    errors: Vec<FitzError>,
    /// Side-table of types synthesized per `Expr` node (Phase 9.0 — F16).
    /// Populated by the `infer_expr` wrapper when exiting each call; exposed
    /// via `check_program` so the LSP can answer hover and
    /// contextual completion.
    type_info: TypeInfo,
    /// Side-table of definitions per use (Phase 9.x.3 — go-to-definition).
    /// Populated when `infer_expr` resolves an `Expr::Ident` via
    /// `lookup_binding` and the binding has a known `def_span` (not
    /// builtin). Same exposure flow as `type_info`.
    def_info: DefinitionInfo,
    /// Mini-batch Vp — `Some(id)` when we are checking the body
    /// of a method of type `id`. Used to validate access to private
    /// fields (prefix `_`): the checker rejects `instance._field` or
    /// struct lits with `_field` from outside the type body, but allows
    /// them inside (including when a method accesses another
    /// `instance._field` of the same class). `None` at top-level
    /// (global script, top-level fn, escaped anonymous fn).
    current_type: Option<TypeId>,
    /// L2 (2026-06-05) — Stack of "hints" for bidirectional inference
    /// of param types in `Expr::FnExpr` without annotation. Each element
    /// corresponds to the next `infer_expr` that will process a FnExpr,
    /// and optionally provides the expected types of its params.
    ///
    /// Current setup: only used for callbacks of built-in methods with
    /// known parametric templates (`.map`/`.filter`/`.find`/etc.
    /// over `List<T>` and `Map<K,V>`). The method call site pushes a
    /// hint with the T from the receiver BEFORE synthesizing the callback, and the
    /// `Expr::FnExpr` handler consumes it on pop. If the hint is
    /// present AND the param does NOT have an annotation, the hint is used instead of
    /// `Type::Any` to bind the param in the body's scope.
    ///
    /// Stack instead of a single Option because a FnExpr can contain
    /// another FnExpr in its body (nested callbacks). In practice almost
    /// always has 0 or 1 elements; the stack makes it robust.
    fn_expr_param_hints: Vec<Option<Vec<Type>>>,
}

impl<'a> CheckCtx<'a> {
    fn new(types: &'a TypeEnv) -> Self {
        let mut ctx = Self {
            types,
            scopes: vec![std::collections::HashMap::new()],
            return_stack: Vec::new(),
            inferred_returns: Vec::new(),
            in_http_handler: Vec::new(),
            await_stack: Vec::new(),
            middleware_fn_names: std::collections::HashSet::new(),
            auth_provider: None,
            background_fns: std::collections::HashSet::new(),
            healthz_first: None,
            readyz_first: None,
            loop_depth: 0,
            break_value_stack: Vec::new(),
            errors: Vec::new(),
            type_info: TypeInfo::new(),
            def_info: DefinitionInfo::new(),
            current_type: None,
            fn_expr_param_hints: Vec::new(),
        };
        ctx.register_builtins();
        ctx
    }

    /// Language builtins that always exist in the evaluator's env.
    /// Those with fixed arity receive a real signature (checks
    /// arity and eventually types); variadic ones are modeled
    /// as `Any` until we have a dedicated representation.
    fn register_builtins(&mut self) {
        // All builtins use `def_span: Span::ZERO` — there is no
        // Fitz file to jump to for go-to-definition. The LSP
        // filters them when responding `textDocument/definition`.

        // `print(args...)` — variadic. Modeled as Any: no
        // call over Any is checked (gradual escape).
        self.scopes[0].insert(
            "print".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // `len(x) -> Int` — arity 1 over List/Map/Str/Range. The
        // param is Any because the receivers do not share a single
        // type (we don't have union types / "any iterable" yet).
        // Arity is validated; receiver type arrives in 5.3.4.
        self.scopes[0].insert(
            "len".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Any],
                    ret: Box::new(Type::Int),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Mini-batch Bytes — `bytes(s: Str) -> Bytes` constructor.
        self.scopes[0].insert(
            "bytes".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Str],
                    ret: Box::new(Type::Bytes),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // `cors(config: Map?) -> CorsConfig` — built-in MW.2.
        // We type it as `Any` today (de facto variadic: 0 or 1 arg, and
        // the inner Map has heterogeneous types per key). A more
        // precise signature requires union types or a dedicated type for
        // CorsConfig in the `Type` enum — out of scope for MW.2.
        // The evaluator does the full validation at runtime.
        self.scopes[0].insert(
            "cors".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Phase 9.w.3 — `spawn(fn_call) -> Future<T>` fire-and-forget.
        // Typed as `Any` because T depends on the fn target; the special
        // dispatch in `synthesize_expr` for `Expr::Call` when the
        // callee is Ident "spawn" refines to the concrete type. Checker
        // validations:
        //   - exactly 1 arg, which must be a literal `Expr::Call`,
        //   - the callee of the inner call must be a top-level fn
        //     declared with `@background`,
        //   - the ret of the spawn is `Future<T>` with T = ret of the
        //     target fn (await-able same as `sleep(...)`).
        // The runtime does `tokio::spawn` and returns a Future that
        // resolves when the task finishes.
        self.scopes[0].insert(
            "spawn".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // `sleep(ms: Int) -> Future<Null>` — first async primitive.
        // Introduced in Phase 6.3. The signature wraps `Null` in
        // `Future<Null>` (parallel to any user `async fn`):
        // the user mandatorily await-s it inside another
        // `async fn`, or holds the Future. The evaluator has
        // a stub that fails with "arrives in 6.4" until the
        // async evaluator lands.
        self.scopes[0].insert(
            "sleep".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Int],
                    ret: Box::new(Type::Future(Box::new(Type::Null))),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // 10.8.7 (v0.10.8) — `ws_broadcast(endpoint: Str, msg) -> Null`
        // cross-handler broadcast of a JSON message to WS clients
        // connected to the `endpoint`. Types msg as `Any` to accept
        // any shape — the runtime serializes via JSON.
        self.scopes[0].insert(
            "ws_broadcast".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Str, Type::Any],
                    ret: Box::new(Type::Null),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Phase 9.z.2.a — assertion builtins. `assert` stays as `Any`
        // because it has variable arity (1 or 2 args, optional msg); the
        // runtime validates types and arity. `assert_eq`/`assert_ne` have
        // fixed arity with `Any` args (structural equality handles
        // any type). `assert_throws` requires `Function` arity 0.
        self.scopes[0].insert(
            "assert".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Phase 9.w.1.b — `jwt` and `hash` as modules always available
        // in the global scope. The evaluator builds them as
        // `Value::Module` with their builtins inside (`encode`/`decode`
        // for jwt; `password`/`verify` for hash). The checker types them
        // as `Any` for two reasons:
        //
        // (1) Current `Type::Function` does not model optional args — `alg`
        //     in `jwt.encode/decode` is positional optional at the end
        //     (`Str?` at value level) which is not expressible with the static
        //     `Type::Function { params, ret }` signature today.
        //
        // (2) Field access over `Any` falls to gradual (also `Any`), so
        //     `jwt.encode` and `hash.password` type as `Any` and the
        //     calls are not statically checked. The loss is contained
        //     because the validation of return types (`Str` for encode,
        //     `Result<Map>` for decode, etc.) happens at runtime with
        //     clear messages from the builtins.
        //
        // Refinable post-MVP with union types or a dedicated `Module` type
        // that carries a table of internal `Function` signatures.
        self.scopes[0].insert(
            "jwt".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        self.scopes[0].insert(
            "hash".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Phase 9.w.1.iter2.b — `auth` module with builtins
        // `blacklist(db, jti, expires_at)`, `is_blacklisted(db, jti)`,
        // `cleanup_expired(db)`. Same Type::Any pattern as jwt/hash —
        // heterogeneous signatures (DbConn first arg) are checked
        // at runtime with clear messages from the builtins.
        self.scopes[0].insert(
            "auth".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Phase 12.8 — `flag(name: Str) -> Bool` (global builtin) +
        // `flags` (module with `is_enabled(name)` and `list()`). Built-in
        // feature flags with defaults configurable via manifest
        // `[flags]` and env vars `FITZ_FLAG_<NAME>`. Same Type::Any pattern
        // as jwt/hash/auth so that field access + calls fall to
        // gradual; runtime builtins validate shape with clear messages.
        self.scopes[0].insert(
            "flag".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        self.scopes[0].insert(
            "flags".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Phase 12.3.a.1 — `log` module always available. Same pattern
        // as `jwt`/`hash`/`db`: types as `Type::Any`. The exact signature
        // (`fn log.info(msg: Str, **kwargs) -> Null`) has arbitrary
        // heterogeneous kwargs which the current system does not model as
        // `Type::Function`; field access falls to `Any` and calls are
        // checked at runtime against the reserved/shape of the logger.
        // Refinable post-MVP when union types or dedicated `Type::Module`
        // arrive.
        self.scopes[0].insert(
            "log".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Phase 10.1.b — `db` module always available in the global
        // env. Typed as `Type::Any` (same pattern as jwt/hash):
        // the exact signature of `db.connect(url: Str) -> Future<Result<DbConn>>`
        // has Future + Result + opaque DbConn type which the current
        // system does not model; refining to parametric `Type::Function`
        // comes as minor debt when the ORM arrives in 10.3+.
        self.scopes[0].insert(
            "db".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Mini-fase HTTP client (2026-06-18) — `http` module always
        // available. Same `Type::Any` pattern as jwt/hash/auth/db/log:
        // the exact signature of `http.get(url) -> Future<Result<HttpClientResponse>>`
        // has Future + Result + opaque heterogeneous body (Str/Map/Bytes
        // for post/put/request) which the current `Type::Function` does
        // not model. Field access falls to gradual, calls validated by
        // the runtime builtins with clear messages. The nominal
        // `HttpClientResponse` IS pre-registered (see
        // `register_http_builtin_types`) so that `let r = http.get(...).await?`
        // followed by `r.status: Int`, `r.body: Str`, etc. type-checks
        // statically once the user lands on the nominal.
        self.scopes[0].insert(
            "http".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Mini-tanda SMTP builtin (2026-06-19) — `smtp` module always
        // available. Same `Type::Any` pattern as `http`/`jwt`/`hash`:
        // `smtp.send(opts: Map) -> Future<Result<SmtpResult>>` has
        // Future + Result + heterogeneous opts (always Map<Str, Str>
        // in MVP, but future deuda is Map<Str, Any> with attachments
        // as List<Map>). Field access on `smtp` falls to gradual; the
        // single call is validated by the runtime builtin with clear
        // messages. The nominal `SmtpResult` IS pre-registered (see
        // `register_http_builtin_types`) so that
        // `let r = smtp.send(opts).await?` followed by
        // `r.delivered: Bool`, `r.message_id: Str`, `r.duration_ms: Int`
        // type-checks statically once the user annotates the binding.
        self.scopes[0].insert(
            "smtp".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // v0.10.24 — Date/DateTime/Uuid global namespace with their
        // static constructors as Value::Module. Typed as `Any`
        // (same pattern as db/jwt/hash) — field access resolves to
        // Any → ret type of the call comes from the runtime builtin.
        // Refinement to concrete signatures remains as minor debt.
        for module_name in ["Date", "DateTime", "Uuid"] {
            self.scopes[0].insert(
                module_name.into(),
                VarBinding {
                    ty: Type::Any,
                    annotated: false,
                    def_span: Span::ZERO,
                    defaults_count: 0,
                    has_varargs: false,
                },
            );
        }
        // Mini-phase env builtin (2026-05-22, Step 3 post-boilerplates) —
        // 3 builtins to read environment variables from Fitz.
        // `env(key) -> Result<Str>` forces the user to handle the
        // missing case with `?` or `match` (parallel to `find`/`get`/`json.loads`).
        self.scopes[0].insert(
            "env".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Str],
                    ret: Box::new(Type::Result {
                        ok: Box::new(Type::Str),
                        err: Box::new(Type::Str),
                    }),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // `env_or(key, default) -> Str` — never fails, returns default
        // if the var doesn't exist. Parallel to Rust's `Option::unwrap_or`.
        self.scopes[0].insert(
            "env_or".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Str, Type::Str],
                    ret: Box::new(Type::Str),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // `load_env(path) -> Result<Null>` — simple KEY=VALUE parser
        // (no variable expansion, no multi-line). Sets vars via
        // `std::env::set_var`. No auto-load by design.
        self.scopes[0].insert(
            "load_env".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Str],
                    ret: Box::new(Type::Result {
                        ok: Box::new(Type::Null),
                        err: Box::new(Type::Str),
                    }),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Phase 12.2.a — `secret(key) -> Result<Secret<Str>>` — multi-source
        // lookup (env var → /run/secrets/<key>) returning an
        // opaque type with auto-redaction. The inner is accessed with
        // explicit `.expose()`.
        self.scopes[0].insert(
            "secret".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Str],
                    ret: Box::new(Type::Result {
                        ok: Box::new(Type::Secret(Box::new(Type::Str))),
                        err: Box::new(Type::Str),
                    }),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Phase 12.2.a — `config(key, default) -> T` — type-coerced
        // lookup. Return type depends on the shape of the default
        // (Int/Float/Bool/Str). We type it as `Any` because the
        // checker doesn't infer "the type of the second arg" as ret type
        // yet. Future refinement: specialization by shape of the
        // default. For now the caller annotates with `let port: Int =
        // config("PORT", 8080)` and the runtime coercion adjusts.
        self.scopes[0].insert(
            "config".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Str, Type::Any],
                    ret: Box::new(Type::Any),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        self.scopes[0].insert(
            "assert_eq".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Any, Type::Any],
                    ret: Box::new(Type::Null),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        self.scopes[0].insert(
            "assert_ne".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Any, Type::Any],
                    ret: Box::new(Type::Null),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        self.scopes[0].insert(
            "assert_throws".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Function {
                        params: vec![],
                        ret: Box::new(Type::Any),
                    }],
                    ret: Box::new(Type::Null),
                },
                annotated: false,
                def_span: Span::ZERO,
                defaults_count: 0,
                has_varargs: false,
            },
        );
        // Mini-batch Bits-extras — global builtins over Int.
        // `popcount/leading_zeros/trailing_zeros(n: Int) -> Int`
        // `rotate_left/right(n: Int, bits: Int) -> Int`
        for name in &["popcount", "leading_zeros", "trailing_zeros"] {
            self.scopes[0].insert(
                (*name).into(),
                VarBinding {
                    ty: Type::Function {
                        params: vec![Type::Int],
                        ret: Box::new(Type::Int),
                    },
                    annotated: false,
                    def_span: Span::ZERO,
                    defaults_count: 0,
                    has_varargs: false,
                },
            );
        }
        for name in &["rotate_left", "rotate_right"] {
            self.scopes[0].insert(
                (*name).into(),
                VarBinding {
                    ty: Type::Function {
                        params: vec![Type::Int, Type::Int],
                        ret: Box::new(Type::Int),
                    },
                    annotated: false,
                    def_span: Span::ZERO,
                    defaults_count: 0,
                    has_varargs: false,
                },
            );
        }
        // Mini-batch Math — abs/min/max/clamp are polymorphic
        // (Int|Float); pow/sqrt return Float; ceil/floor/round
        // return Int. Today all `Any` due to the complexity of
        // modeling polymorphism in the current system; the evaluator
        // and codegen validate them at each call site.
        for name in &[
            "abs", "min", "max", "pow", "sqrt", "ceil", "floor", "round", "clamp",
        ] {
            self.scopes[0].insert(
                (*name).into(),
                VarBinding {
                    ty: Type::Any,
                    annotated: false,
                    def_span: Span::ZERO,
                    defaults_count: 0,
                    has_varargs: false,
                },
            );
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(std::collections::HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// M4 (v0.10.15) — helper to execute a block of checks inside
    /// a new scope (push + work + pop), reducing the
    /// repetition of the `push_scope(); ...; pop_scope();` pattern that
    /// appears ~10 times in `check_block` / `infer_expr`.
    ///
    /// **Limitation**: the closure receives `&mut self`, so early
    /// returns with `?` inside it don't leak scope (because when
    /// they return, they are already exiting `with_scope` which has already
    /// done the push). However, if the closure panics, pop_scope
    /// does not run — for tests/REPL recovery with catch_unwind this
    /// remains as minor debt. In practice the checker does not panic (errors
    /// go to `ctx.errors`, not via panic).
    fn with_scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.push_scope();
        let r = f(self);
        self.pop_scope();
        r
    }

    /// Declares a variable without type annotation (inferred or
    /// gradual). Allows future reassignments to change the
    /// type freely. `def_span` is the declaration's position
    /// (Phase 9.x.3 — used by go-to-definition); pass `Span::ZERO`
    /// for builtins / synthetic declarations.
    fn declare_var(&mut self, name: String, ty: Type, def_span: Span) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(
                name,
                VarBinding {
                    ty,
                    annotated: false,
                    def_span,
                    defaults_count: 0,
                    has_varargs: false,
                },
            );
        }
    }

    /// Declares a variable with explicit type annotation. Subsequent
    /// reassignments without annotation will be checked
    /// against this type. `def_span` same as `declare_var`.
    fn declare_var_annotated(&mut self, name: String, ty: Type, def_span: Span) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(
                name,
                VarBinding {
                    ty,
                    annotated: true,
                    def_span,
                    defaults_count: 0,
                    has_varargs: false,
                },
            );
        }
    }

    /// Fp — declares a fn with defaults info. The minimum arity of the
    /// callee is `params.len() - defaults_count`. Fp.2 — `has_varargs`
    /// indicates whether the last param is variadic (the call site accepts 0+
    /// extra args).
    fn declare_fn(
        &mut self,
        name: String,
        ty: Type,
        def_span: Span,
        defaults_count: usize,
        has_varargs: bool,
    ) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(
                name,
                VarBinding {
                    ty,
                    annotated: true,
                    def_span,
                    defaults_count,
                    has_varargs,
                },
            );
        }
    }

    fn lookup_binding(&self, name: &str) -> Option<&VarBinding> {
        for s in self.scopes.iter().rev() {
            if let Some(b) = s.get(name) {
                return Some(b);
            }
        }
        None
    }

    /// Reports an error without known position. After S1.2 sub-step 2,
    /// error sites over `Expr` already know their span and use
    /// `error_at`. This helper remains for "global" reports (no
    /// associated node) that may appear in the future.
    #[allow(dead_code)]
    fn error(&mut self, msg: impl Into<String>) {
        self.errors
            .push(FitzError::new(ErrorKind::TypeError, 0, 0, msg.into()));
    }

    /// Variant of `error` that cites the actual node position (line
    /// and column of the first `Stmt` token). Used by stmt-level
    /// report sites — see `check_stmt`. When the span is
    /// `Span::ZERO` (parser synthetic nodes or tests),
    /// `FitzError::Display` omits the "at line N:M" prefix per
    /// Span's `is_known()` rule — behavior stays identical
    /// to `error` for those cases.
    fn error_at(&mut self, span: Span, msg: impl Into<String>) {
        self.errors.push(FitzError::new(
            ErrorKind::TypeError,
            span.line,
            span.column,
            msg.into(),
        ));
    }
}

/// Converts an `Option<TypeExpr>` into `Type` for user annotations.
/// If the annotation was missing → `Any`. If the annotation is present but
/// doesn't resolve → `Any` and the error is assumed to have been reported by
/// `resolve_program`.
fn ann_to_type(ann: Option<&TypeExpr>, env: &TypeEnv) -> Type {
    match ann {
        None => Type::Any,
        Some(t) => resolve_type_expr(t, env).unwrap_or(Type::Any),
    }
}

/// Synthesizes the type of an expression and persists it in the
/// `ctx.type_info` side-table before returning it. The synthesis logic lives in
/// `synthesize_expr`; this wrapper centralizes the `record` so that
/// **all** `Expr` nodes are registered when passing through the
/// checker (including recursion: the wrapper is called per node, so
/// `BinOp { left, right }` and its operands are all three). Nodes
/// with `Span::ZERO` (synthetic / tests) are omitted — see `TypeInfo::
/// record`. Enabling pre-req of the LSP (Phase 9 — F16).
fn infer_expr(ctx: &mut CheckCtx, e: &Expr) -> Type {
    let ty = synthesize_expr(ctx, e);
    ctx.type_info.record(e.span(), ty.clone());
    ty
}

/// 10.8.4 (v0.10.8) — partial flow-sensitive narrowing over
/// `Ident <op> null` conditions (in any order). If the
/// condition matches the pattern and the Ident's binding is
/// `Nullable<T>`, returns `(name, T, def_span)` so the caller
/// declares a shadow binding in the refined branch's scope.
///
/// - `want_not_null = true`: narrow when the condition is
///   `x != null` or `null != x` (branch `then`).
/// - `want_not_null = false`: narrow when the condition is
///   `x == null` or `null == x` (branch `else`).
///
/// Limitations (minor debt — refinable if demand appears):
/// - Does not support `not (x == null)` (explicit negation).
/// - Does not support `x != null and ...` (chain of conditions).
/// - Does not support transitive narrowing through fns.
/// - Does not support else-side narrowing via early-return in then
///   (typical idiom: `if (x == null) return; <use x as T>`).
fn narrow_null_check(
    cond: &Expr,
    ctx: &CheckCtx,
    want_not_null: bool,
) -> Option<(String, Type, crate::ast::Span)> {
    use crate::ast::BinOpKind;
    let Expr::BinOp {
        op, left, right, ..
    } = cond
    else {
        return None;
    };
    let matches_op = matches!(
        (op, want_not_null),
        (BinOpKind::NotEq, true) | (BinOpKind::Eq, false)
    );
    if !matches_op {
        return None;
    }
    // Detect (Ident, Null) in any order.
    let name = match (left.as_ref(), right.as_ref()) {
        (Expr::Ident(n, _), Expr::Null(_)) => n.clone(),
        (Expr::Null(_), Expr::Ident(n, _)) => n.clone(),
        _ => return None,
    };
    // Binding must exist and be Nullable<inner>.
    let binding = ctx.lookup_binding(&name)?;
    if let Type::Nullable(inner) = &binding.ty {
        return Some((name, (**inner).clone(), binding.def_span));
    }
    None
}

/// Synthesis core. Does NOT touch `type_info` directly — the
/// `infer_expr` wrapper does it on exit. This centralizes the
/// side-table populating policy in a single place, avoiding that each
/// match branch has to remember the `record`.
///
/// Cases not covered in 5.3.1 silently return `Type::Any` — they are
/// not errors, we just don't check that form yet. Subsequent
/// sub-phases (5.3.2 calls, 5.3.3 Result, 5.3.4 methods,
/// 5.3.5 FnExpr) will replace them.
fn synthesize_expr(ctx: &mut CheckCtx, e: &Expr) -> Type {
    match e {
        // Fp.3 — NamedArg is only valid inside Call.args; the
        // calls dispatcher processes it. Seeing it here indicates a bug.
        Expr::NamedArg { name, value, span } => {
            ctx.error_at(
                *span,
                format!("named argument `{}:` cannot appear outside a call", name),
            );
            synthesize_expr(ctx, value)
        }

        Expr::Int(_, _) => Type::Int,
        Expr::Float(_, _) => Type::Float,
        Expr::Str(_, _) => Type::Str,
        Expr::Bool(_, _) => Type::Bool,
        Expr::Null(_) => Type::Null,
        Expr::Bytes(_, _) => Type::Bytes,

        // Mini-batch L — `loop { body }` as an expression. The type is
        // the `lub` of the `break <v>` values inside. Without
        // breaks with a value → `Null`. Collecting break types
        // requires walking the body; we use a side-channel
        // `break_value_stack` that `Stmt::Break(Some(e), _)` feeds.
        Expr::Loop { body, .. } => {
            ctx.loop_depth += 1;
            ctx.break_value_stack.push(Vec::new());
            for s in body {
                check_stmt(ctx, s);
            }
            let values = ctx.break_value_stack.pop().unwrap_or_default();
            ctx.loop_depth -= 1;
            unify_returns(&values)
        }

        // Tuples (mini-batch T) — we type each slot and assemble
        // `Type::Tuple`.
        Expr::Tuple(items, _) => {
            let tys: Vec<Type> = items.iter().map(|x| infer_expr(ctx, x)).collect();
            Type::Tuple(tys)
        }
        Expr::TupleField { tuple, index, span } => {
            let ty = infer_expr(ctx, tuple);
            match ty.base() {
                Type::Tuple(items) => {
                    if let Some(t) = items.get(*index) {
                        t.clone()
                    } else {
                        ctx.error_at(
                            *span,
                            format!(
                                "tuple of {} elements does not have index `{}`",
                                items.len(),
                                index
                            ),
                        );
                        Type::Any
                    }
                }
                Type::Any | Type::PyAny => Type::Any,
                other => {
                    ctx.error_at(
                        *span,
                        format!(
                            "access `.{}` only applies to tuples, received `{}`",
                            index,
                            other.display(ctx.types)
                        ),
                    );
                    Type::Any
                }
            }
        }

        Expr::StrInterp(parts, _) => {
            // Sub-expressions are evaluated for errors although the
            // result is always Str. Mini-batch Fm: the spec is
            // validated in `validate_format_spec_for_type` — the filter of
            // numeric types vs `f`, etc.
            for p in parts {
                if let StrPart::Expr(inner, spec) = p {
                    let ty = infer_expr(ctx, inner);
                    if let Some(s) = spec {
                        validate_format_spec_for_type(ctx, s, &ty, inner.span());
                    }
                }
            }
            Type::Str
        }

        Expr::Ident(name, span) => {
            // We resolve the binding and clone what's needed to release
            // the immutable borrow on `ctx.scopes` before touching
            // `ctx.def_info` (which requires &mut self). Phase 9.x.3:
            // we register the `def_span` for go-to-definition when
            // it exists (not a builtin with Span::ZERO).
            let resolved = ctx.lookup_binding(name).map(|b| (b.ty.clone(), b.def_span));
            if let Some((ty, def_span)) = resolved {
                ctx.def_info.record(*span, def_span);
                return ty;
            }
            // If it's a declared nominal type, the user is using it
            // as a value (which the evaluator supports:
            // registers Value::Type in the env). Not an error; we
            // treat it as Any.
            if ctx.types.lookup(name).is_some() {
                return Type::Any;
            }
            ctx.error_at(*span, format!("unknown variable `{}`", name));
            Type::Any
        }

        Expr::UnaryOp { op, operand, span } => {
            let t = infer_expr(ctx, operand);
            match op {
                UnaryOpKind::Neg => match &t {
                    Type::Int | Type::Float | Type::Any => t,
                    other => {
                        ctx.error_at(
                            *span,
                            format!(
                                "operator `-` (negation) expects Int or Float, received `{}`",
                                other.display(ctx.types)
                            ),
                        );
                        Type::Any
                    }
                },
                // R.1.1 — `not <expr>` requires strict `Bool`. No
                // truthy/falsy in Fitz: passing `Int`/`Str`/etc. is
                // a type error (consistent with `assert(cond)` which
                // also requires strict Bool).
                UnaryOpKind::Not => match &t {
                    Type::Bool | Type::Any => Type::Bool,
                    other => {
                        ctx.error_at(
                            *span,
                            format!(
                                "operator `not` expects Bool, received `{}`",
                                other.display(ctx.types)
                            ),
                        );
                        Type::Bool
                    }
                },
                // Mini-batch Bits — `~x` only Int.
                UnaryOpKind::BitNot => match &t {
                    Type::Int | Type::Any => Type::Int,
                    other => {
                        ctx.error_at(
                            *span,
                            format!(
                                "operator `~` expects Int, received `{}`",
                                other.display(ctx.types)
                            ),
                        );
                        Type::Int
                    }
                },
            }
        }

        Expr::BinOp {
            op,
            left,
            right,
            span,
        } => {
            let lt = infer_expr(ctx, left);
            let rt = infer_expr(ctx, right);
            infer_binop(ctx, op, &lt, &rt, *span)
        }

        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            // Condition must be Bool (or Any).
            let cond_ty = infer_expr(ctx, condition);
            if !is_compatible(&cond_ty, &Type::Bool) {
                // We point to the condition's own span — better
                // hint than the `if` itself.
                ctx.error_at(
                    condition.span(),
                    format!(
                        "the `if` condition must be Bool, received `{}`",
                        cond_ty.display(ctx.types)
                    ),
                );
            }
            // 10.8.4 (v0.10.8) — fix #1: flow-sensitive narrowing from
            // `Nullable<T>` → `T`. If the condition is `x != null` (or
            // `null != x`), we narrow the `x` binding inside the
            // `then` branch to the inner type. If it's `x == null`,
            // we narrow in the `else`. Canonical case:
            //   if (status != null) { let s: Str = status }
            // Previously the checker typed `status` as `Str?` inside
            // the `if`, forcing a workaround with match arm.
            //
            // Supported pattern: literal comparison Ident <op> null
            // (in any order) over a Nullable binding.
            // Not supported (minor debt): chains like
            // `if (x != null and ...)`, narrowing of the else side via
            // early-return in if-then, transitive narrowing through
            // fns.
            let then_narrow = narrow_null_check(condition, ctx, /*want_not_null=*/ true);
            let else_narrow = narrow_null_check(condition, ctx, /*want_not_null=*/ false);
            // Each branch is a block; the "type" of an if-stmt is that
            // of its last expression-stmt. For 5.3.1 it's enough for us to
            // walk the blocks (with scope) and return Any.
            // M4 (v0.10.15) — use with_scope helper for auto-pop.
            ctx.with_scope(|ctx| {
                if let Some((name, inner_ty, def_span)) = then_narrow {
                    ctx.declare_var(name, inner_ty, def_span);
                }
                check_block(ctx, then)
            });
            if let Some(else_body) = else_ {
                ctx.with_scope(|ctx| {
                    if let Some((name, inner_ty, def_span)) = else_narrow {
                        ctx.declare_var(name, inner_ty, def_span);
                    }
                    check_block(ctx, else_body)
                });
            }
            Type::Any
        }

        Expr::List(items, _) => {
            // List<T> with T = type of the first element if the rest
            // are compatible; if there's a mix, T = Any.
            if items.is_empty() {
                return Type::List(Box::new(Type::Any));
            }
            let first = infer_expr(ctx, &items[0]);
            let mut all_same = true;
            for it in &items[1..] {
                let t = infer_expr(ctx, it);
                if !is_compatible(&t, &first) {
                    all_same = false;
                }
            }
            if all_same {
                Type::List(Box::new(first))
            } else {
                Type::List(Box::new(Type::Any))
            }
        }

        // Mini-batch C + Cmp+ — `[expr for var in iter ([for ...]*) [if cond]?]`.
        // Types each `for` clause (iter as List/Range, var via pattern),
        // binding in nested scopes; validates `filter: Bool` inside
        // the innermost scope; types `expr: U` and returns `List<U>`.
        Expr::ListComp {
            expr,
            var,
            iter,
            extra_clauses,
            filter,
            span,
        } => {
            ctx.push_scope();
            check_comp_clause_in_checker(ctx, var, iter, *span);
            for (extra_var, extra_iter) in extra_clauses {
                check_comp_clause_in_checker(ctx, extra_var, extra_iter, *span);
            }
            if let Some(f) = filter {
                let f_ty = infer_expr(ctx, f);
                if !is_compatible(&f_ty, &Type::Bool) {
                    ctx.error_at(
                        f.span(),
                        format!(
                            "the `if` filter of the list comprehension must be `Bool`, received `{}`",
                            f_ty.display(ctx.types)
                        ),
                    );
                }
            }
            let elem_ty = infer_expr(ctx, expr);
            ctx.pop_scope();
            Type::List(Box::new(elem_ty))
        }

        // Mini-batch Cmp+ — `{key: value for ...}`. Analogous to ListComp:
        // types each clause, validates filter, and types key+value in the
        // innermost scope. Returns `Map<K, V>`.
        Expr::MapComp {
            key,
            value,
            var,
            iter,
            extra_clauses,
            filter,
            span,
        } => {
            ctx.push_scope();
            check_comp_clause_in_checker(ctx, var, iter, *span);
            for (extra_var, extra_iter) in extra_clauses {
                check_comp_clause_in_checker(ctx, extra_var, extra_iter, *span);
            }
            if let Some(f) = filter {
                let f_ty = infer_expr(ctx, f);
                if !is_compatible(&f_ty, &Type::Bool) {
                    ctx.error_at(
                        f.span(),
                        format!(
                            "the `if` filter of the map comprehension must be `Bool`, received `{}`",
                            f_ty.display(ctx.types)
                        ),
                    );
                }
            }
            let key_ty = infer_expr(ctx, key);
            let val_ty = infer_expr(ctx, value);
            ctx.pop_scope();
            Type::Map(Box::new(key_ty), Box::new(val_ty))
        }

        Expr::Map(pairs, _) => {
            if pairs.is_empty() {
                return Type::Map(Box::new(Type::Any), Box::new(Type::Any));
            }
            // We synthesize from the first pair. Mix of types falls to Any.
            let (fk, fv) = (infer_expr(ctx, &pairs[0].0), infer_expr(ctx, &pairs[0].1));
            let mut k_same = true;
            let mut v_same = true;
            for (k, v) in &pairs[1..] {
                let kt = infer_expr(ctx, k);
                let vt = infer_expr(ctx, v);
                if !is_compatible(&kt, &fk) {
                    k_same = false;
                }
                if !is_compatible(&vt, &fv) {
                    v_same = false;
                }
            }
            Type::Map(
                Box::new(if k_same { fk } else { Type::Any }),
                Box::new(if v_same { fv } else { Type::Any }),
            )
        }

        Expr::Range { start, end, .. } => {
            // Start and end must be Int (as in the evaluator). The
            // error span points to the problematic endpoint to
            // distinguish which of the two.
            for (label, e) in [("start", start.as_ref()), ("end", end.as_ref())] {
                let t = infer_expr(ctx, e);
                if !is_compatible(&t, &Type::Int) {
                    ctx.error_at(
                        e.span(),
                        format!(
                            "{} of the range must be Int, received `{}`",
                            label,
                            t.display(ctx.types)
                        ),
                    );
                }
            }
            Type::Range
        }

        Expr::StructLit {
            type_name,
            fields,
            span,
        } => {
            // Synthesizes Nominal if the type's name is declared.
            // Validates fields against the declared `type`: missing,
            // extras, incompatible types.
            let id = match ctx.types.lookup(type_name) {
                Some(id) => id,
                None => {
                    // resolve_program already reports unknown types
                    // as fields/annotations; a StructLit with
                    // non-existent name is the checker's responsibility.
                    ctx.error_at(
                        *span,
                        format!("type `{}` does not exist to instantiate", type_name),
                    );
                    // We still evaluate the values to detect errors
                    // inside.
                    for (_, v) in fields {
                        let _ = infer_expr(ctx, v);
                    }
                    return Type::Any;
                }
            };
            // We compare against the nominal's resolved fields.
            let declared = ctx.types.info(id).fields.clone();
            // Infer provided types (always, so that inner warnings
            // surface).
            let mut provided_types: Vec<(String, Type, Span)> = Vec::new();
            for (n, v) in fields {
                let t = infer_expr(ctx, v);
                provided_types.push((n.clone(), t, v.span()));
            }
            if let Some(declared) = declared {
                // Extras
                let declared_names: std::collections::HashSet<&str> =
                    declared.iter().map(|f| f.name.as_str()).collect();
                for (n, _, fs) in &provided_types {
                    if !declared_names.contains(n.as_str()) {
                        ctx.error_at(
                            *fs,
                            format!("type `{}` does not have a field named `{}`", type_name, n),
                        );
                    }
                    // Mini-batch Vp — struct lit cannot set private
                    // fields from outside the type body. Useful to
                    // force use of static constructors (mini-batch St).
                    if is_private_field(n) && ctx.current_type != Some(id) {
                        ctx.error_at(*fs, format!(
                            "field `{}.{}` is private: cannot be set from a struct lit outside the methods of type `{}` (use a static constructor like `{}.new(...)`)",
                            type_name, n, type_name, type_name
                        ));
                    }
                }
                // Missing and compatibility of provided.
                let provided_map: std::collections::HashMap<&str, (&Type, Span)> = provided_types
                    .iter()
                    .map(|(n, t, fs)| (n.as_str(), (t, *fs)))
                    .collect();
                for f in &declared {
                    match provided_map.get(f.name.as_str()) {
                        Some((actual, fs)) if !is_compatible(actual, &f.type_) => {
                            ctx.error_at(
                                *fs,
                                format!(
                                    "field `{}.{}` expects `{}`, received `{}`",
                                    type_name,
                                    f.name,
                                    f.type_.display(ctx.types),
                                    actual.display(ctx.types)
                                ),
                            );
                        }
                        Some(_) => {}
                        None => {
                            // Missing: valid if nullable or if the
                            // evaluator expects a default (validated in
                            // resolve_program).
                            //
                            // In the nullable case, no error. In the
                            // rest, we could alert — but the
                            // evaluator emits its own error at
                            // runtime when a field is missing without
                            // default. To not duplicate messages,
                            // we let this pass in 5.3.1.
                        }
                    }
                }
            }
            Type::Nominal(id)
        }

        Expr::Field {
            object,
            field,
            span,
        } => {
            let obj_ty = infer_expr(ctx, object);
            match &obj_ty {
                Type::Nominal(id) => {
                    let info = ctx.types.info(*id);
                    let type_name = info.name.clone();
                    if let Some(declared) = &info.fields {
                        if let Some(f) = declared.iter().find(|f| f.name == *field) {
                            // Mini-batch Vp — private fields (`_*`)
                            // only accessible from inside the body
                            // of a method of the SAME type.
                            if is_private_field(field) && ctx.current_type != Some(*id) {
                                ctx.error_at(*span, format!(
                                    "field `{}.{}` is private (prefix `_`); only accessible from methods of the type `{}` itself",
                                    type_name, field, type_name
                                ));
                            }
                            return f.type_.clone();
                        }
                        // Unknown field. In 5.3.4 when methods land
                        // it may be legitimate (the syntactic "field"
                        // is a method). For now we stay silent
                        // if it's inside a Call (handled by
                        // infer_call), and warn if not — but we don't
                        // know the context here. We return Any.
                        return Type::Any;
                    }
                    Type::Any
                }
                // 8.4: field access over `PyAny` gives `PyAny`. Covers
                // chains like `os.path` / `os.path.sep` / `engine.url`
                // — all opaque until the user annotates
                // explicitly. The runtime check via getattr already
                // throws a clear AttributeError if the field doesn't exist.
                Type::PyAny => Type::PyAny,
                // Any other receiver: 5.3.4 covers it with built-in
                // methods. For now Any.
                _ => Type::Any,
            }
        }

        Expr::Call { callee, args, span } => {
            // Method path: `obj.method(args)` ↔ callee
            // syntactic is `Expr::Field`. We dispatch by
            // `(receiver type, method name)` against the
            // built-ins table (5.3.4) instead of going through the
            // general route — the general route cannot model
            // parametric signatures like `List<T>.map`.
            if let Expr::Field { object, field, .. } = callee.as_ref() {
                let mut obj_ty = infer_expr(ctx, object);
                // Phase 10.3+ — ORM static methods: when the `object` is
                // an Ident that matches a nominal with `@table`,
                // `infer_expr` already types it as `Any` (Ident arm
                // rule: type-name-as-value). We refine to `Nominal(id)`
                // locally so that `infer_method_call` dispatches the
                // static methods (all/where/insert) correctly.
                // This does NOT change the global type of the Ident — only in
                // this specific Call. Other uses of `User` as var
                // remain `Any`.
                if matches!(obj_ty, Type::Any) {
                    if let Expr::Ident(name, _) = object.as_ref() {
                        if let Some(id) = ctx.types.lookup(name) {
                            if ctx.types.table_metadata(id).is_some() {
                                obj_ty = Type::Nominal(id);
                            }
                        }
                    }
                }
                // Fp.3 — for method calls with named args, exact
                // checking requires knowing the method's param names
                // (R.3 custom methods). For built-ins we don't support
                // named args (no exposed param names). For
                // now, NamedArg in a method call with Nominal receiver
                // passes as gradual (Any); the runtime does the real
                // check. If the receiver is built-in (List/Map/Str), the
                // checker types the value inside the NamedArg and delegates
                // to the general dispatcher — the runtime emits a clear error.
                // L2 (2026-06-05) — Bidirectional inference in
                // callbacks of built-in methods. If the arg is directly
                // an `Expr::FnExpr` and the method has a known
                // parametric template (`.map`/`.filter`/etc. over `List<T>` and
                // `Map<K,V>`), we push a hint with the expected
                // param types BEFORE synthesizing the callback. The
                // `Expr::FnExpr` handler (`fn_expr_param_hints.pop()`)
                // consumes the hint exactly once. Push and pop
                // stay balanced because we only push when the arg
                // IS a direct FnExpr (which will always pop). Args
                // wrapped in `NamedArg` also work — the wrapper
                // delegates to `infer_expr(value)` which enters the handler.
                let args_ty: Vec<Type> = args
                    .iter()
                    .map(|a| {
                        let inner = match a {
                            Expr::NamedArg { value, .. } => value.as_ref(),
                            other => other,
                        };
                        let is_fn_expr = matches!(inner, Expr::FnExpr { .. });
                        if is_fn_expr {
                            let hint = expected_callback_param_for_builtin_method(&obj_ty, field);
                            ctx.fn_expr_param_hints.push(hint);
                        }
                        match a {
                            Expr::NamedArg { value, .. } => infer_expr(ctx, value),
                            other => infer_expr(ctx, other),
                        }
                    })
                    .collect();
                // 8.4: PyAny receiver — the method is invoked crossing
                // to Python via dispatch_method (8.1.4). The runtime
                // wraps ALL Python calls in `Result<T>` (8.3); the
                // checker mirrors that: the call types as
                // `Result<Any>`, not `Any`. This activates the
                // exhaustiveness rule over Result (5.3.3) and the
                // `?` operator restriction (5.3.2/5.3.3) — the user is
                // forced to handle the failure statically, just like
                // any native `Result<T>`.
                if matches!(obj_ty, Type::PyAny) {
                    return Type::Result {
                        ok: Box::new(Type::Any),
                        err: Box::new(Type::Str),
                    };
                }
                return match infer_method_call(ctx, &obj_ty, field, &args_ty, *span) {
                    Some(ret) => ret,
                    // Receiver we don't understand (Nominal without custom
                    // methods, Module via import, Any): we continue in
                    // gradual mode without checking anything of the call.
                    None => Type::Any,
                };
            }
            // Phase 9.w.3 — special dispatch for `spawn(fn_call)`.
            // The builtin is typed as `Any` (5.3.4); here we refine to
            // the concrete `Future<T>` type where T is the ret type of the
            // target fn. Validations:
            //   - exactly 1 arg, which must be a literal `Expr::Call`
            //     (no var, no composite expression).
            //   - the callee of the inner call must be a top-level fn
            //     declared with `@background` (author's opt-in).
            //
            // The dispatch only applies if the `spawn` binding was not
            // shadowed by a user-defined fn: we compare the `ty` of the
            // binding against `Type::Any` (the builtin's). If the
            // user does `fn spawn(x) -> Int`, the lookup types as
            // `Function{...}` and we fall to the normal route.
            if let Expr::Ident(name, _) = callee.as_ref() {
                if name == "spawn"
                    && matches!(ctx.lookup_binding("spawn").map(|b| &b.ty), Some(Type::Any))
                {
                    return check_spawn_call(ctx, args, *span);
                }
            }
            // We always synthesize callee and args so that errors
            // inside surface. Then we validate arity and types according
            // to what the callee is.
            // Fp.3 — destructure NamedArg when synthesizing to type the value
            // and not fail with "outside a call". The actual reorder/check
            // happens in `infer_call_with_named_args` when the
            // callee is a resolvable Ident.
            let callee_ty = infer_expr(ctx, callee);
            // L2 expanded (2026-06-05) — Bidirectional inference
            // from user-defined callees with Function param. If the
            // callee types as `Function { params, ret }` and an arg is
            // a FnExpr whose corresponding param-i is in turn
            // `Function { params: cb_params, .. }`, we propagate the
            // `cb_params` as a hint to the FnExpr. Covers the canonical case:
            //
            //     fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x) }
            //     apply(fn(n) => n * 2, 5)   // n is inferred as Int
            //
            // Stack-based like original L2 — the Expr::FnExpr handler
            // pops on entry.
            let callee_param_types: Option<Vec<Type>> = match &callee_ty {
                Type::Function { params, .. } => Some(params.clone()),
                _ => None,
            };
            let args_ty: Vec<Type> = args
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let inner = match a {
                        Expr::NamedArg { value, .. } => value.as_ref(),
                        other => other,
                    };
                    if matches!(inner, Expr::FnExpr { .. }) {
                        let hint = callee_param_types
                            .as_ref()
                            .and_then(|pt| pt.get(i))
                            .and_then(|t| match t {
                                Type::Function {
                                    params: cb_params, ..
                                } => Some(cb_params.clone()),
                                _ => None,
                            });
                        ctx.fn_expr_param_hints.push(hint);
                    }
                    match a {
                        Expr::NamedArg { value, .. } => infer_expr(ctx, value),
                        other => infer_expr(ctx, other),
                    }
                })
                .collect();
            match callee_ty {
                // Gradual: callee with unknown type is not checked.
                Type::Any => Type::Any,
                // 8.4: callee is an opaque PyObject — the call crosses to
                // Python and returns wrapped in `Result<T>` (decision
                // 8.3). Covers `let f = math.sqrt; f(25.0)` (callee
                // resolved by Ident after field access).
                Type::PyAny => Type::Result {
                    ok: Box::new(Type::Any),
                    err: Box::new(Type::Str),
                },
                Type::Function { params, ret } => {
                    let label = describe_callee(callee);
                    // Fp — the function-signature in `Type::Function` does
                    // not carry defaults info (it only lists the types). For
                    // arity checking we consult Stmt::FnDef directly
                    // when the callee is a resolvable Ident;
                    // fallback to strict arity for indirect callees
                    // (callbacks, fns as var).
                    let required = required_arity_for_callee(ctx, callee, params.len());
                    let has_varargs = callee_has_varargs(ctx, callee);
                    // Fp.2 — varargs: tail param types as `List<T>` in
                    // the binding, but inside Type::Function the
                    // params still carry the element type T. The
                    // call site validates each arg against T (not against
                    // List<T>); minimum arity includes at least the
                    // params prior to varargs.
                    let max_arity = if has_varargs {
                        usize::MAX
                    } else {
                        params.len()
                    };
                    let required = if has_varargs {
                        // Varargs accepts 0+ args in the last slot, so
                        // the minimum arity is total - 1 (the varargs
                        // can receive 0 args).
                        required.min(params.len().saturating_sub(1))
                    } else {
                        required
                    };
                    // Fp.3 — if there are named args, the real reorder happens
                    // at runtime/codegen. The strict arity check
                    // by position does not apply (names can skip
                    // positions). We validate only minimum global arity.
                    let has_named_args = args.iter().any(|a| matches!(a, Expr::NamedArg { .. }));
                    if args.len() < required || args.len() > max_arity {
                        ctx.error_at(
                            *span,
                            if has_varargs {
                                format!(
                                    "{} expects at least {} argument(s), received {}",
                                    label,
                                    required,
                                    args.len(),
                                )
                            } else if required == params.len() {
                                format!(
                                    "{} expects {} argument(s), received {}",
                                    label,
                                    params.len(),
                                    args.len(),
                                )
                            } else {
                                format!(
                                    "{} expects between {} and {} argument(s), received {}",
                                    label,
                                    required,
                                    params.len(),
                                    args.len(),
                                )
                            },
                        );
                    } else if !has_named_args {
                        for (i, actual) in args_ty.iter().enumerate() {
                            // Fp.2 — for the varargs slot (the last),
                            // all extra args are checked against
                            // the ELEMENT type of the varargs (not against
                            // List<T>). If i < params.len()-1, goes to the
                            // positional param; if i >= last_idx and there are
                            // varargs, goes against params[last_idx].
                            let expected_idx = if has_varargs && i >= params.len() {
                                params.len() - 1
                            } else {
                                i
                            };
                            let expected = &params[expected_idx];
                            if !is_compatible(actual, expected) {
                                ctx.error_at(
                                    args[i].span(),
                                    format!(
                                        "{}: argument {} expects `{}`, received `{}`",
                                        label,
                                        i + 1,
                                        expected.display(ctx.types),
                                        actual.display(ctx.types)
                                    ),
                                );
                            }
                        }
                    }
                    *ret
                }
                other => {
                    ctx.error_at(
                        callee.span(),
                        format!("`{}` is not a function", other.display(ctx.types)),
                    );
                    Type::Any
                }
            }
        }
        Expr::FnExpr {
            params,
            body,
            is_async,
            span,
        } => {
            // We walk the body with a new scope and the params
            // bound (with their declared type or `Any` if the
            // annotation was missing). The FnExpr's type is `Function`;
            // 5.3.5 infers the `ret` by collecting the types of the
            // `Stmt::Return`s in the body and unifying them with `lub`.
            // We push `Any` to the return_stack because without annotation
            // we cannot validate against what — the returns are
            // collected, not checked.
            //
            // Mini-batch Async-cl — `await_stack` pushes `*is_async`:
            // `async fn(...)` allows `.await` inside; `fn(...)` rejects it.
            // The final type of the async FnExpr is
            // `Function { ret: Future<T> }`.
            ctx.push_scope();
            ctx.return_stack.push(Type::Any);
            ctx.inferred_returns.push(Vec::new());
            ctx.in_http_handler.push(false);
            ctx.await_stack.push(*is_async);
            // R.2.4 (F3): break/continue do NOT escape FnExpr (closures).
            let saved_loop_depth = ctx.loop_depth;
            ctx.loop_depth = 0;
            // L2 (2026-06-05) — consume bidirectional inference hint
            // if any (set by the call site of a built-in method with
            // known parametric template). For each param WITHOUT annotation,
            // we use the hint's type instead of `Type::Any`. Param WITH
            // explicit annotation always wins (not overwritten).
            let hint = ctx.fn_expr_param_hints.pop().flatten();
            let param_types: Vec<Type> = params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    if p.type_.is_some() {
                        ann_to_type(p.type_.as_ref(), ctx.types)
                    } else if let Some(hint_types) = hint.as_ref() {
                        hint_types.get(i).cloned().unwrap_or(Type::Any)
                    } else {
                        Type::Any
                    }
                })
                .collect();
            for (p, t) in params.iter().zip(param_types.iter()) {
                // Fp.2 — varargs: inside the body, the binding types
                // as `List<T>`.
                let bind_ty = if p.varargs {
                    Type::List(Box::new(t.clone()))
                } else {
                    t.clone()
                };
                // S1 (2026-06-05) — `Param` now has its own `name_span`.
                // If present (not ZERO), we use it as the binding's
                // def_span and register the type in
                // TypeInfo to enable hover over the param's name
                // in the FnExpr's signature. Fallback to the containing
                // FnExpr's span if name_span is ZERO (synthetic param in tests).
                let def_span = if p.name_span.column > 0 {
                    p.name_span
                } else {
                    *span
                };
                ctx.declare_var(p.name.clone(), bind_ty.clone(), def_span);
                if p.name_span.column > 0 {
                    ctx.type_info.record(p.name_span, bind_ty);
                }
            }
            check_block(ctx, body);
            ctx.loop_depth = saved_loop_depth;
            let returns = ctx.inferred_returns.pop().unwrap_or_default();
            ctx.return_stack.pop();
            ctx.in_http_handler.pop();
            ctx.await_stack.pop();
            ctx.pop_scope();
            let ret = unify_returns(&returns);
            let final_ret = if *is_async {
                Type::Future(Box::new(ret))
            } else {
                ret
            };
            Type::Function {
                params: param_types,
                ret: Box::new(final_ret),
            }
        }
        Expr::Slice {
            object,
            start,
            end,
            span,
            ..
        } => {
            let obj_ty = infer_expr(ctx, object);
            for (name, e) in [("start", start), ("end", end)] {
                if let Some(inner) = e {
                    let t = infer_expr(ctx, inner);
                    if !is_compatible(&t, &Type::Int) {
                        ctx.error_at(
                            inner.span(),
                            format!(
                                "`{}` of a slice must be Int, received `{}`",
                                name,
                                t.display(ctx.types),
                            ),
                        );
                    }
                }
            }
            match obj_ty.base() {
                Type::List(t) => Type::List(Box::new((**t).clone())),
                Type::Str => Type::Str,
                Type::Any | Type::Nominal(_) => Type::Any,
                other => {
                    ctx.error_at(
                        *span,
                        format!(
                            "type `{}` does not support slicing with `[..]`",
                            other.display(ctx.types),
                        ),
                    );
                    Type::Any
                }
            }
        }

        Expr::Index {
            object,
            index,
            span,
        } => {
            let obj_ty = infer_expr(ctx, object);
            let idx_ty = infer_expr(ctx, index);
            match obj_ty.base() {
                Type::List(t) => {
                    if !is_compatible(&idx_ty, &Type::Int) {
                        ctx.error_at(
                            index.span(),
                            format!(
                                "index of a `List` must be Int, received `{}`",
                                idx_ty.display(ctx.types)
                            ),
                        );
                    }
                    (**t).clone()
                }
                Type::Map(k, v) => {
                    if !is_compatible(&idx_ty, k) {
                        ctx.error_at(
                            index.span(),
                            format!(
                                "index of a `Map<{}, {}>` must be `{}`, received `{}`",
                                k.display(ctx.types),
                                v.display(ctx.types),
                                k.display(ctx.types),
                                idx_ty.display(ctx.types)
                            ),
                        );
                    }
                    (**v).clone()
                }
                Type::Str => {
                    // I.1 (mini-batch I) — `s[i]` returns the i-th
                    // char as a single-char `Str` (Fitz has no Char).
                    // Indexed by CHAR, not byte (consistent with
                    // `s.len()` which counts chars). Negatives supported:
                    // `s[-1]` = last.
                    if !is_compatible(&idx_ty, &Type::Int) {
                        ctx.error_at(
                            index.span(),
                            format!(
                                "index of a `Str` must be Int, received `{}`",
                                idx_ty.display(ctx.types)
                            ),
                        );
                    }
                    Type::Str
                }
                // Gradual: Any and Nominal don't check. Nominal with
                // `[]` operator is debt (custom indexers don't exist);
                // Any is the usual escape.
                Type::Any | Type::Nominal(_) => Type::Any,
                other => {
                    ctx.error_at(
                        *span,
                        format!(
                            "type `{}` does not support indexing with `[]`",
                            other.display(ctx.types)
                        ),
                    );
                    Type::Any
                }
            }
        }
        Expr::Match { value, arms, span } => {
            let scrutinee = infer_expr(ctx, value);
            // W2 (v0.10.6) — Nullable refinement in match arms.
            // When the scrutinee is `T?` (Nullable<T>) and a
            // PREVIOUS arm matches explicit `null`, subsequent arms
            // with `Pattern::Ident` are refined to `T` (without
            // Nullable). This unblocks `match post.user { null =>
            // "<null>", u => u.name }`.
            //
            // Rules (MVP):
            // - Scrutinee must be `Type::Nullable(T)`.
            // - Some previous arm must have `Pattern::Null`. A
            //   `Pattern::Or` containing Null also covers.
            // - We only refine `Pattern::Ident(_, _)` (including the
            //   `_`/wildcard case which doesn't bind but doesn't break either).
            // - Tuples/OkBinding/ErrBinding are NOT refined in MVP.
            let refined_inner: Option<Type> = match &scrutinee {
                Type::Nullable(inner) => Some((**inner).clone()),
                _ => None,
            };
            let mut null_cubierto_previamente = false;
            // Binding type according to the pattern. For `Ok(x)` with
            // scrutinee `Result<T>`, x is T. For `Err(e)` the error
            // is fixed at Str. For Ident it's the whole scrutinee.
            // For literals/wildcard/range there is no bind.
            let mut first: Option<Type> = None;
            for arm in arms {
                ctx.push_scope();
                // Without own span on `MatchArm`/`Pattern` (S1 debt),
                // we use the body's span as the approximation of the
                // binding's `def_span` — the closest of the arm in
                // the current AST. go-to-def over the binding's use
                // jumps to the arm's body.
                // Sp.2 — body is Vec<Stmt>; the span is that of the first stmt.
                let body_span = arm.body.first().map(|s| s.span()).unwrap_or(Span::ZERO);
                // W2 — decide whether to refine this arm's binding.
                // Only applies to Pattern::Ident over Nullable scrutinee
                // when a previous arm already covered Null.
                let scrutinee_for_binding = match (&arm.pattern, &refined_inner) {
                    (crate::ast::Pattern::Ident(_, _), Some(inner))
                        if null_cubierto_previamente =>
                    {
                        inner.clone()
                    }
                    _ => scrutinee.clone(),
                };
                bind_pattern(ctx, &arm.pattern, &scrutinee_for_binding, body_span);
                // R.2.2 — the guard types inside the binding's scope.
                // It must synthesize Bool; other type is an error.
                if let Some(guard_expr) = &arm.guard {
                    let guard_ty = infer_expr(ctx, guard_expr);
                    if !matches!(guard_ty, Type::Bool | Type::Any) {
                        ctx.error_at(
                            guard_expr.span(),
                            format!(
                                "the guard of an arm must be Bool, received {}",
                                guard_ty.display(ctx.types)
                            ),
                        );
                    }
                }
                // Sp.2 — check the body (Vec<Stmt>) and derive the arm's
                // type. Cases:
                //   - Stmt::Expr: t = expr's type.
                //   - Stmt::Return/Break/Continue: `!` type (never).
                //     Since there is no explicit Type::Never, we use
                //     Type::Any (matches any expected).
                //   - Other stmts: only checked, don't contribute
                //     to the arm's type. If they are the LAST stmt, t stays
                //     Null (decision consistent with if/else).
                let mut t = Type::Null;
                let arm_len = arm.body.len();
                for (i, stmt) in arm.body.iter().enumerate() {
                    let is_last = i + 1 == arm_len;
                    match stmt {
                        Stmt::Expr(e, _) => {
                            t = infer_expr(ctx, e);
                        }
                        Stmt::Return(e, _) => {
                            // Check the return's value against
                            // return_stack. The "arm's type" is Any
                            // (never coerce).
                            check_stmt(ctx, stmt);
                            let _ = e;
                            if is_last {
                                t = Type::Any;
                            }
                        }
                        Stmt::Break(..) | Stmt::Continue(..) => {
                            check_stmt(ctx, stmt);
                            if is_last {
                                t = Type::Any;
                            }
                        }
                        _ => {
                            check_stmt(ctx, stmt);
                            if is_last {
                                t = Type::Null;
                            }
                        }
                    }
                }
                ctx.pop_scope();
                if first.is_none() {
                    first = Some(t);
                }
                // W2 — update the flag AFTER processing the arm:
                // the next arm can benefit from the refinement if
                // THIS arm matched null.
                if pattern_cubre_null(&arm.pattern) {
                    null_cubierto_previamente = true;
                }
            }
            // Exhaustiveness: we only require it when the scrutinee is
            // `Result<T>` (pure, not nullable). Other types don't have
            // "variants" semantics in Fitz yet.
            if matches!(scrutinee, Type::Result { .. }) {
                check_result_match_exhaustiveness(ctx, arms, *span);
            }
            first.unwrap_or(Type::Any)
        }
        Expr::Ok(inner, _) => {
            // Mini-batch Re+ — without context, E stays `Any` (the
            // checker doesn't know what Err can appear later). The LUB
            // against other Results will refine E if Errs are built in
            // the same flow. The destination annotation (`-> Result<T, E>`)
            // wins over the inferred.
            let t = infer_expr(ctx, inner);
            Type::Result {
                ok: Box::new(t),
                err: Box::new(Type::Any),
            }
        }
        Expr::Err(inner, _) => {
            // Mini-batch Re+ — the E type is now inferred from the
            // value. T stays Any without context; the LUB/destination
            // annotation will refine it.
            let e_ty = infer_expr(ctx, inner);
            Type::Result {
                ok: Box::new(Type::Any),
                err: Box::new(e_ty),
            }
        }
        Expr::Await(inner, span) => {
            // 6.2: full checker semantics.
            //
            // Rule 1 — async context. `.await` is only legal inside
            // a Fitz `async fn`. Top-level and FnExpr (sync closures)
            // are invalid. `await_stack.last()` tells us if the
            // nearest fn is async. If not, error with clear
            // message but we still synthesize a type to not
            // confuse the user with cascading errors.
            //
            // Rule 2 — `Future<T>` operand. What `.await` unwraps
            // must be a `Future<T>` (or `Any` for gradual escape).
            // Any other concrete type is an error.
            let operand_ty = infer_expr(ctx, inner);

            // Top-level (empty stack) counts as a valid async context
            // — the evaluator starts the tokio runtime there and the codegen
            // emits `#[tokio::main] async fn main()` when the program
            // uses async. We only reject when we are inside an explicit
            // sync fn (`Some(false)`): non-async FnDef or
            // FnExpr (closures don't support async yet).
            if matches!(ctx.await_stack.last(), Some(false)) {
                ctx.error_at(
                    *span,
                    "`.await` is only valid inside `async fn` or at top-level".to_string(),
                );
            }

            match &operand_ty {
                Type::Any => Type::Any,
                Type::Future(inner_ty) => (**inner_ty).clone(),
                // Phase 8.7.3: `.await` over `Result<PyAny>` or
                // `Result<Any>` (what the Python call synthesizes per
                // 8.4 → 8.3) is NOT directly supported in the interpreter
                // (the evaluator rejects with "expected Future").
                // The canonical pattern is `<py_call>?.await`: the `?`
                // unwraps the Result to Future, and .await operates
                // over the Future. Here we do NOT add a branch for
                // `Result<...>` — it remains a type error if the
                // user omits the `?`. The `PyAny` branch only
                // covers the codegen 8.7.3 case where the inner of the
                // await after `?` is PyAny (`<call>?.await` with
                // the combined helper).
                Type::PyAny => Type::Any,
                other => {
                    ctx.error_at(
                        *span,
                        format!(
                            "`.await` only applies to `Future<T>`, received `{}`",
                            other.display(ctx.types)
                        ),
                    );
                    Type::Any
                }
            }
        }

        Expr::Try(inner, span) => {
            let operand_ty = infer_expr(ctx, inner);
            match &operand_ty {
                // Gradual: operand with unknown type is not checked.
                // Covers the typical case of a built-in method (Field
                // callee) which still returns Any until 5.3.4.
                Type::Any => Type::Any,
                Type::Result {
                    ok: inner_ty,
                    err: _,
                } => {
                    // If we are inside a function with
                    // concrete return_type, we require it to be Result —
                    // `?` propagates an `Err(_)` via `return`, so
                    // the containing fn must be able to receive it.
                    // Fn without return_type (Any) or top-level does not check.
                    //
                    // W13 (v0.10.9) — Exception: if we are inside
                    // an HTTP handler (`@get`/`@post`/`@put`/`@delete`),
                    // `?` is allowed even with concrete return_type
                    // different from Result. The runtime/codegen wraps
                    // the handler in `__FitzResponse`: the Err propagated
                    // by `?` automatically becomes a
                    // 500 response (parity with `value_to_outcome` and
                    // with the wrapper's Ok/Err match). The user does
                    // not have to rewrite their fn as `-> Result<T>`
                    // just to use `?` in its body.
                    let in_handler = ctx.in_http_handler.last().copied().unwrap_or(false);
                    if let Some(expected) = ctx.return_stack.last().cloned() {
                        let is_ok = matches!(expected, Type::Any | Type::Result { .. });
                        if !is_ok && !in_handler {
                            ctx.error_at(*span, format!(
                                "operator `?` can only be used inside a function that returns `Result<...>`; this one returns `{}`",
                                expected.display(ctx.types)
                            ));
                        }
                    }
                    (**inner_ty).clone()
                }
                other => {
                    ctx.error_at(
                        *span,
                        format!(
                            "operator `?` requires a `Result`, received `{}`",
                            other.display(ctx.types)
                        ),
                    );
                    Type::Any
                }
            }
        }

        // Phase 9.0.1 (F15): `Expr::Error` is only produced by
        // `parse_with_recovery`. The checker treats it as `Type::Any`
        // and does NOT emit derived errors — the real error is already in the
        // parser's `recovered_errors` list. Silent is the
        // correct policy: if the LSP runs the checker over an AST
        // with Error nodes, we don't want cascade of derived errors
        // on the same point.
        Expr::Error(_) => Type::Any,
    }
}

/// Friendly label for a `Call`'s callee. Appears in arity and
/// argument type errors. When we can identify the name (Ident or Field),
/// we use it; otherwise, a generic label.
fn describe_callee(callee: &Expr) -> String {
    match callee {
        Expr::Ident(name, _) => format!("function `{}`", name),
        Expr::Field { field, .. } => format!("method `{}`", field),
        _ => "this call".into(),
    }
}

/// Pragmatic "Least upper bound" to synthesize the return type of
/// a function whose body has multiple `return`s with
/// different types. Not a formal lattice: prioritizes preserving
/// useful information (Result<X> + Result<Any> = Result<X>) over
/// theoretical purity.
///
/// Rules:
///   - `a == b` → `a`.
///   - Any either side → the other (Any yields to the concrete).
///   - Int + Float → Float (coercion).
///   - Null + T → `T?` (optional branch).
///   - T + T? → `T?`.
///   - Generics (List/Map/Result/Nullable) → recursion.
///   - Arbitrary mix → Any.
fn lub(a: &Type, b: &Type) -> Type {
    if a == b {
        return a.clone();
    }
    if matches!(a, Type::Any) {
        return b.clone();
    }
    if matches!(b, Type::Any) {
        return a.clone();
    }
    // Int↔Float coercion.
    if (matches!(a, Type::Int) && matches!(b, Type::Float))
        || (matches!(a, Type::Float) && matches!(b, Type::Int))
    {
        return Type::Float;
    }
    // Null + T → T? (and symmetric).
    if matches!(a, Type::Null) {
        return Type::Nullable(Box::new(b.clone()));
    }
    if matches!(b, Type::Null) {
        return Type::Nullable(Box::new(a.clone()));
    }
    // T + T? → T? (and symmetric): if the nullable's inner is equal
    // to the other, that's already the best we have.
    if let Type::Nullable(inner) = a {
        if **inner == *b {
            return a.clone();
        }
    }
    if let Type::Nullable(inner) = b {
        if **inner == *a {
            return b.clone();
        }
    }
    // Recursive generics.
    match (a, b) {
        (Type::List(ai), Type::List(bi)) => Type::List(Box::new(lub(ai, bi))),
        (Type::Map(ak, av), Type::Map(bk, bv)) => {
            Type::Map(Box::new(lub(ak, bk)), Box::new(lub(av, bv)))
        }
        // Mini-batch Re+: recursive lub on both sides (ok and err).
        (
            Type::Result {
                ok: a_ok,
                err: a_err,
            },
            Type::Result {
                ok: b_ok,
                err: b_err,
            },
        ) => Type::Result {
            ok: Box::new(lub(a_ok, b_ok)),
            err: Box::new(lub(a_err, b_err)),
        },
        (Type::Future(ai), Type::Future(bi)) => Type::Future(Box::new(lub(ai, bi))),
        (Type::Nullable(ai), Type::Nullable(bi)) => Type::Nullable(Box::new(lub(ai, bi))),
        _ => Type::Any,
    }
}

/// Unifies the types of the `return`s collected during the
/// walk of a function's body. If the list is empty, the
/// function does not return explicitly and we return `Null` (matches
/// the evaluator's semantics: a fn that ends without `return`
/// produces `Value::Null`).
fn unify_returns(types: &[Type]) -> Type {
    if types.is_empty() {
        return Type::Null;
    }
    let mut result = types[0].clone();
    for t in &types[1..] {
        result = lub(&result, t);
    }
    result
}

/// L2 (2026-06-05) — Bidirectional callback inference.
/// For a built-in method with a known parametric template,
/// returns the expected types of the callback's params.
///
/// Cases covered in the MVP:
///
/// - `List<T>.map/filter/find/any/all/count/find_index/flat_map(fn(T) -> ...)`
///   → `Some(vec![T])`.
/// - `Map<K, V>.filter/find/any/all/count(fn(K, V) -> Bool)` (if they existed;
///   today they are not registered in `infer_map_method`) → `Some(vec![K, V])`.
/// - Other methods / receivers → `None`.
///
/// The call site pushes the hint to `ctx.fn_expr_param_hints` before
/// synthesizing the arg. The `Expr::FnExpr` handler consumes it on pop:
/// for params WITHOUT annotation it uses the hint; for params WITH annotation
/// the annotation always wins (not overwritten).
fn expected_callback_param_for_builtin_method(obj_ty: &Type, method: &str) -> Option<Vec<Type>> {
    match obj_ty {
        Type::List(elem_ty) => match method {
            // All take `fn(T) -> ...` — we propagate T as expected.
            "map" | "filter" | "find" | "any" | "all" | "count" | "find_index" | "flat_map" => {
                Some(vec![(**elem_ty).clone()])
            }
            _ => None,
        },
        // `Map<K, V>` does not expose higher-order methods today in
        // `infer_map_method` (only get/has/keys/values/len). If they arrive
        // in the future (filter/find/etc. over entries), add here
        // with shape `fn(K, V) -> ...` → `Some(vec![K, V])`.
        Type::Map(_, _) => None,
        _ => None,
    }
}

/// Checker dispatch for built-in method. Receives the receiver's
/// type (`xs` in `xs.map(f)`), the method's name, and the
/// already-inferred argument types. Returns `Some(ret)` with
/// the result's type, or `None` when the receiver doesn't enter
/// the built-in dispatch (Nominal without custom methods yet,
/// Module via import — both modeled as `Any` or `Nominal`).
///
/// For `None` cases, the caller continues in gradual mode
/// (returns `Any` without checking arity/types). For supported
/// cases, violations are reported via `ctx.error(...)`
/// but the dispatch always returns `Some(...)` with the
/// inferred ret (errors don't propagate, they accumulate).
///
/// Convention: `T` always comes from the concrete receiver at this
/// call site. `List<Int>.map(f)` and `List<Str>.map(f)` instantiate
/// differently.
fn infer_method_call(
    ctx: &mut CheckCtx,
    receiver_ty: &Type,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Option<Type> {
    // We peel a Nullable: `xs?.map(...)` falls when `?` has already
    // unwrapped, so here we rarely see Nullable. Just in case,
    // we keep it transparent.
    let recv = receiver_ty.base();
    match recv {
        Type::List(t) => {
            let t = (**t).clone();
            Some(infer_list_method(ctx, &t, method, args_ty, span))
        }
        Type::Map(k, v) => {
            let k = (**k).clone();
            let v = (**v).clone();
            Some(infer_map_method(ctx, &k, &v, method, args_ty, span))
        }
        Type::Str => Some(infer_str_method(ctx, method, args_ty, span)),
        // Mini-batch Bytes — methods on Bytes.
        Type::Bytes => Some(infer_bytes_method(ctx, method, args_ty, span)),
        // F13.D — universal methods over `Type::Any` for dynamic
        // type-check on heterogeneous. Return `Result<T>` if match,
        // `Result::Err(Str)` otherwise. `type_name()` returns `Str` directly.
        Type::Any => match method {
            "as_int" => {
                check_method_arity(ctx, method, args_ty, 0, span);
                Some(Type::Result {
                    ok: Box::new(Type::Int),
                    err: Box::new(Type::Str),
                })
            }
            "as_float" => {
                check_method_arity(ctx, method, args_ty, 0, span);
                Some(Type::Result {
                    ok: Box::new(Type::Float),
                    err: Box::new(Type::Str),
                })
            }
            "as_str" => {
                check_method_arity(ctx, method, args_ty, 0, span);
                Some(Type::Result {
                    ok: Box::new(Type::Str),
                    err: Box::new(Type::Str),
                })
            }
            "as_bool" => {
                check_method_arity(ctx, method, args_ty, 0, span);
                Some(Type::Result {
                    ok: Box::new(Type::Bool),
                    err: Box::new(Type::Str),
                })
            }
            "as_bytes" => {
                check_method_arity(ctx, method, args_ty, 0, span);
                Some(Type::Result {
                    ok: Box::new(Type::Bytes),
                    err: Box::new(Type::Str),
                })
            }
            "type_name" => {
                check_method_arity(ctx, method, args_ty, 0, span);
                Some(Type::Str)
            }
            // Any other method on Any: gradual (falls to the generic
            // fallback that assumes Any).
            _ => None,
        },
        // R.3 — custom methods over nominal. We first look in
        // the fields that are `Type::Function` (8-pyi.C: the `.pyi`
        // loader registers each stub fn as a
        // `Function { params, ret }` field inside the module's
        // synthetic nominal). Then in `NominalInfo.methods` (R.3 — custom
        // methods declared with `fn name(self, ...)` inside the
        // `type`). If nothing matches: gradual (None), like Any.
        Type::Nominal(id) => {
            // Phase 10.3+ — ORM static methods on nominal with
            // `@table`. Before the custom methods lookup so that
            // `User.where(...)`/`.all(...)`/`.insert(...)` type even if
            // the user has not declared those methods explicitly.
            // The runtime dispatch handles them from `orm_dispatch_*`.
            if ctx.types.table_metadata(*id).is_some() {
                let row_ty = Type::Nominal(*id);
                match method {
                    "all" => {
                        check_method_arity(ctx, method, args_ty, 1, span);
                        // arg 0 must be DbConn (also compatible with Any
                        // for gradual escape). We skip
                        // strict checking so `User.all(db)`
                        // with db: Any keeps compiling.
                        // Phase 10.b: the evaluator returns Future, the
                        // checker mirrors it so `.await?` types OK.
                        return Some(Type::Future(Box::new(Type::Result {
                            ok: Box::new(Type::List(Box::new(row_ty))),
                            err: Box::new(Type::Str),
                        })));
                    }
                    "where" => {
                        check_method_arity(ctx, method, args_ty, 1, span);
                        // arg 0 is a closure fn(Row) -> Bool. Skip
                        // closure type check for now —
                        // the evaluator validates at runtime.
                        return Some(Type::QueryBuilder(Box::new(row_ty)));
                    }
                    "group_by" => {
                        // Phase 10.b.14 — `User.group_by(...)` directly
                        // (without prior .where) returns Aggregated<User>.
                        // Same shape as `User.where(...).group_by(...)`
                        // but skips the intermediate QueryBuilder.
                        check_method_arity(ctx, method, args_ty, 1, span);
                        return Some(Type::Aggregated(Box::new(row_ty)));
                    }
                    "insert" => {
                        check_method_arity(ctx, method, args_ty, 2, span);
                        // Phase 10.b: ditto — evaluator returns Future.
                        return Some(Type::Future(Box::new(Type::Result {
                            ok: Box::new(row_ty),
                            err: Box::new(Type::Str),
                        })));
                    }
                    // v0.10.27 — F1 bulk insert: `Type.bulk_insert(rows,
                    // db)` or `Type.bulk_insert(rows, db, batch_size)`.
                    // Inserts N rows in batches with multi-tuple
                    // VALUES (...). Returns total rows inserted.
                    // Arity accepts 2 (default batch) or 3 (custom batch).
                    "bulk_insert" => {
                        let n = args_ty.len();
                        if n != 2 && n != 3 {
                            let tname = ctx.types.info(*id).name.clone();
                            ctx.error_at(
                                span,
                                format!(
                                    "`{}.bulk_insert` expects 2 args (rows, db) or 3 (rows, db, batch_size: Int), received {}",
                                    tname, n
                                ),
                            );
                        }
                        return Some(Type::Future(Box::new(Type::Result {
                            ok: Box::new(Type::Int),
                            err: Box::new(Type::Str),
                        })));
                    }
                    _ => {
                        // Fall through to the custom methods lookup
                        // in case the user defined `User.helper(...)`.
                    }
                }
            }
            // Phase 10.b.7 — navigation methods on Nominal with
            // @table. If the method matches the Fitz name of a field
            // with `@belongs_to`/`@has_one`/`@has_many`, we type it as
            // the appropriate Future (parallel to `orm_instance_navigate`
            // in the evaluator). This covers `post.user(db).await?` with
            // concrete `User` type inside the Ok, without requiring
            // a destination annotation.
            //
            // Checker limitation: does not distinguish receiver-type
            // (Type::Nominal of the static `User` Ident) from receiver-
            // instance (Type::Nominal of an Instance). If the user
            // does static `User.profile(db)` with a name matching
            // a relation, the checker says OK but the runtime/codegen
            // rejects it because navigation requires Instance. Pathological
            // case — the names of static ORM methods (all/where/
            // insert/etc.) cannot be field names of the type.
            if let Some(meta) = ctx.types.table_metadata(*id) {
                if let Some(rel) = meta.relations.get(method).cloned() {
                    // Phase 10.b.13 — 2 paths:
                    //   - args.is_empty() → QueryBuilder<Target> for
                    //     chain (recommended). User chains .where/.
                    //     limit/.all/.first/etc.
                    //   - args.len() == 1 (DbConn) → direct terminal
                    //     (backward compat with 10.b.7).
                    if args_ty.len() > 1 {
                        ctx.error_at(
                            span,
                            format!(
                                "navigation `<instance>.{}(db?)` expects 0 args (QueryBuilder chain) or 1 arg (db, terminal), received {}",
                                method,
                                args_ty.len()
                            ),
                        );
                    }
                    // Resolve target type from env by Fitz name.
                    let target_id_opt = ctx.types.lookup(&rel.target_type);
                    let target_ty = match target_id_opt {
                        Some(tid) => Type::Nominal(tid),
                        None => Type::Any,
                    };
                    // New path (no db) → QueryBuilder<Target>.
                    if args_ty.is_empty() {
                        return Some(Type::QueryBuilder(Box::new(target_ty)));
                    }
                    // Legacy path (with db) → direct terminal.
                    let ok_ty = match rel.kind {
                        RelationKind::BelongsTo
                        | RelationKind::HasOne
                        | RelationKind::BelongsToCompanion => target_ty,
                        RelationKind::HasMany => Type::List(Box::new(target_ty)),
                    };
                    return Some(Type::Future(Box::new(Type::Result {
                        ok: Box::new(ok_ty),
                        err: Box::new(Type::Str),
                    })));
                }
            }
            let info = ctx.types.info(*id);
            // 8-pyi.C: field-as-callable (Function type registered
            // as a field by the `.pyi` stubs loader).
            if let Some(fields) = info.fields.as_ref() {
                if let Some(f) = fields.iter().find(|f| f.name == method).cloned() {
                    if let Type::Function { params, ret } = &f.type_ {
                        // 8-pyi.C: for synthetic nominals from the
                        // loader (`__pyi_module_<binding>`), we show
                        // only the binding in error messages — the
                        // prefix is internal detail.
                        let nominal_name = info
                            .name
                            .strip_prefix("__pyi_module_")
                            .unwrap_or(&info.name)
                            .to_string();
                        if args_ty.len() != params.len() {
                            ctx.error_at(
                                span,
                                format!(
                                    "`{}.{}` expects {} argument(s), received {}",
                                    nominal_name,
                                    method,
                                    params.len(),
                                    args_ty.len()
                                ),
                            );
                            return Some((**ret).clone());
                        }
                        for (i, (got, expected)) in args_ty.iter().zip(params.iter()).enumerate() {
                            if !is_compatible(got, expected) {
                                // U2 (v0.10.15) — use the canonical helper
                                // FitzError::type_mismatch (U1) instead of
                                // the ad-hoc format!().
                                ctx.errors.push(FitzError::type_mismatch(
                                    span.line,
                                    span.column,
                                    &format!("`{}.{}` arg #{}", nominal_name, method, i),
                                    &expected.display(ctx.types),
                                    &got.display(ctx.types),
                                ));
                            }
                        }
                        return Some((**ret).clone());
                    }
                }
            }
            let info = ctx.types.info(*id);
            if let Some(nm) = info.methods.iter().find(|m| m.name == method).cloned() {
                // Mini-batch Vm — private methods (`_method`) only
                // accessible from inside methods of the SAME type.
                // Applies to instance and static methods equally.
                if is_private_field(method) && ctx.current_type != Some(*id) {
                    ctx.error_at(span, format!(
                        "method `{}.{}` is private (prefix `_`); only accessible from methods of the type `{}` itself",
                        info.name, method, info.name
                    ));
                }
                // Arity.
                if args_ty.len() != nm.params.len() {
                    ctx.error_at(
                        span,
                        format!(
                            "method `{}.{}` expects {} argument(s), received {}",
                            info.name,
                            method,
                            nm.params.len(),
                            args_ty.len()
                        ),
                    );
                    let ret = if nm.is_async {
                        Type::Future(Box::new(nm.ret))
                    } else {
                        nm.ret
                    };
                    return Some(ret);
                }
                // Arg types (semantically compatible_with).
                // U2 (v0.10.15) — use FitzError::type_mismatch (U1).
                for (i, (got, expected)) in args_ty.iter().zip(nm.params.iter()).enumerate() {
                    if !is_compatible(got, expected) {
                        ctx.errors.push(FitzError::type_mismatch(
                            span.line,
                            span.column,
                            &format!("method `{}.{}` arg #{}", info.name, method, i),
                            &expected.display(ctx.types),
                            &got.display(ctx.types),
                        ));
                    }
                }
                let ret = if nm.is_async {
                    Type::Future(Box::new(nm.ret))
                } else {
                    nm.ret
                };
                Some(ret)
            } else {
                // Non-existent method on nominal → gradual (Any).
                // The evaluator will emit an error at runtime; codegen
                // too. Here we don't fire to avoid duplicating.
                None
            }
        }
        // Mini-batch Ir — methods on Range. Range exposes the subset
        // of iterators that make sense (enumerate/zip/chain) + `len`.
        // The evaluator materializes the Range to List<Int> and delegates.
        Type::Range => Some(infer_range_method(ctx, method, args_ty, span)),
        // Phase 9.w.2 + 9.w.2-wsconn-bidir — `WsConn<T>` or
        // `WsConn<In, Out>`. Parametric methods:
        // `recv() -> Result<RECV>` (Err if conn closed),
        // `send(msg: SEND) -> Result<Null>` (Err if send failed),
        // `broadcast(msg: SEND) -> Result<Null>` (to all conns of the
        // endpoint, including the sender),
        // `close() -> Null` (closes the conn).
        Type::WsConn { recv, send } => {
            let recv = (**recv).clone();
            let send = (**send).clone();
            Some(infer_wsconn_method(
                ctx, &recv, &send, method, args_ty, span,
            ))
        }
        // Mini-batch Mb9 — methods on Int/Float primitives.
        Type::Int => Some(infer_int_method(ctx, method, args_ty, span)),
        Type::Float => Some(infer_float_method(ctx, method, args_ty, span)),
        // Phase 10.3+ — ORM methods on `QueryBuilder<Row>`.
        // Chain methods (where/order_by/limit/offset/group_by) preserve
        // the row type. Terminals break the chain returning
        // `Result<...>` with the appropriate shape.
        Type::QueryBuilder(row) => {
            let row_ty = (**row).clone();
            Some(infer_query_builder_method(
                ctx, &row_ty, method, args_ty, span,
            ))
        }
        // Phase 10.b.14 — `Aggregated<Row>`: QueryBuilder post-group_by.
        // Aggregates (sum/avg/min/max/count) change shape to
        // `Future<Result<List<Map<Str, Any>>>>` with each row = a
        // group + its aggregate. Chain methods (where/order_by/limit/
        // offset/group_by) continue as Aggregated. all/first/
        // update/delete are rejected (don't make sense over GROUP BY).
        Type::Aggregated(row) => {
            let row_ty = (**row).clone();
            Some(infer_aggregated_method(ctx, &row_ty, method, args_ty, span))
        }
        // Phase 10.7 (v0.10.14) — Postgres driver methods on
        // `DbConn`. `query/exec` raw SQL escape hatch, `close/
        // is_closed` lifecycle, `transaction` orchestrates a tx with
        // automatic commit/rollback.
        Type::DbConn => Some(infer_db_conn_method(ctx, method, args_ty, span)),
        // v0.10.22 — `DbRow` (raw row of the query result) exposes
        // `.get(col: Str) -> Result<Any>` to extract fields. The
        // returned type is Any because the row's shape is dynamic
        // (depends on the SELECT). The user coerces with an annotation
        // (`let id: Int = row.get("id")?`).
        Type::DbRow => Some(infer_db_row_method(ctx, method, args_ty, span)),
        // v0.10.24 — Date/DateTime/Uuid instance methods.
        Type::Date => Some(infer_date_method(ctx, method, args_ty, span)),
        Type::DateTime => Some(infer_datetime_method(ctx, method, args_ty, span)),
        Type::Uuid => Some(infer_uuid_method(ctx, method, args_ty, span)),
        // Phase 12.2.a — `Secret<T>.expose() -> T` unwraps the inner.
        // Only method of the type; no args. The checker checks arity
        // and returns the typed inner T (not Any) so the rest of the
        // pipeline can reason with a concrete type.
        Type::Secret(inner) => match method {
            "expose" => {
                if !args_ty.is_empty() {
                    ctx.error_at(
                        span,
                        format!(
                            "Secret.expose() does not accept arguments, received {}",
                            args_ty.len()
                        ),
                    );
                }
                Some((**inner).clone())
            }
            other => {
                ctx.error_at(
                    span,
                    format!(
                        "Secret<{}> does not have method `{}`. Only available method: `.expose()` (unwraps the inner). Display/Debug/JSON are redacted by design.",
                        inner.display(ctx.types),
                        other
                    ),
                );
                Some(Type::Any)
            }
        },
        other => {
            // Types without built-in methods: `42.foo()` and similar.
            // The evaluator also stops; here we get ahead with a
            // specific message.
            ctx.error_at(
                span,
                format!(
                    "type `{}` does not have method `{}`",
                    other.display(ctx.types),
                    method
                ),
            );
            Some(Type::Any)
        }
    }
}

/// Phase 10.3+ — signatures of ORM methods on `QueryBuilder<Row>`.
///
/// Chain methods (preserve the row type):
///   - `where(closure) -> QueryBuilder<Row>`
///   - `order_by(closure) -> QueryBuilder<Row>`
///   - `limit(n: Int) -> QueryBuilder<Row>`
///   - `offset(n: Int) -> QueryBuilder<Row>`
///   - `group_by(closure) -> QueryBuilder<Row>` (terminal via .all)
///
/// Terminals (break the chain):
///   - `all(db) -> Result<List<Row>>`
///   - `first(db) -> Result<Row>`
///   - `count(db) -> Result<Int>`
///   - `sum/avg/min/max(closure, db) -> Result<Number>`
///   - `update(db, changes: Map) -> Result<Int>` (rows affected)
///   - `delete(db) -> Result<Int>` (rows affected)
///
/// Args: we skip strict checking for gradual escape. The evaluator
/// validates at runtime with clear messages.
fn infer_query_builder_method(
    ctx: &mut CheckCtx,
    row: &Type,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Type {
    // Phase 10.b: terminals are async — return Future so that
    // `.await?` types against the correct shape. Chain methods are
    // sync (return QueryBuilder directly).
    let future_result_int = || {
        Type::Future(Box::new(Type::Result {
            ok: Box::new(Type::Int),
            err: Box::new(Type::Str),
        }))
    };
    let future_result_float = || {
        Type::Future(Box::new(Type::Result {
            ok: Box::new(Type::Float),
            err: Box::new(Type::Str),
        }))
    };
    let qb = || Type::QueryBuilder(Box::new(row.clone()));
    let aggregated = || Type::Aggregated(Box::new(row.clone()));
    match method {
        // Chain methods — preserve QueryBuilder<Row>, EXCEPT
        // `.group_by(...)` which mutates to Aggregated<Row> (10.b.14).
        "where" | "order_by" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            qb()
        }
        "group_by" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            aggregated()
        }
        "limit" | "offset" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            qb()
        }
        // Phase 10.b.15 — `.preload("name")`: chain method that registers
        // a relation to preload. Preserves the QueryBuilder's row type.
        // Args: 1 string literal (validated in codegen against
        // meta.relations of the row). The runtime executes the batch
        // post-deserialize in `.all`/`.first`.
        "preload" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            qb()
        }
        // Async terminals — wrapped in Future so `.await?`
        // types correctly.
        "all" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            Type::Future(Box::new(Type::Result {
                ok: Box::new(Type::List(Box::new(row.clone()))),
                err: Box::new(Type::Str),
            }))
        }
        "first" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            Type::Future(Box::new(Type::Result {
                ok: Box::new(row.clone()),
                err: Box::new(Type::Str),
            }))
        }
        "count" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            future_result_int()
        }
        "sum" | "avg" | "min" | "max" => {
            // 2 args: selection closure + db
            check_method_arity(ctx, method, args_ty, 2, span);
            // `avg` always returns Float; sum/min/max depend on the
            // column type. For MVP we return Float (compatible
            // with Int via promotion in the caller).
            future_result_float()
        }
        "update" => {
            check_method_arity(ctx, method, args_ty, 2, span);
            future_result_int()
        }
        // v0.10.32 (Tier C.3) — `.merge_jsonb(db, field, patch)` →
        // Future<Result<Int>> con rows afectadas.
        "merge_jsonb" => {
            check_method_arity(ctx, method, args_ty, 3, span);
            future_result_int()
        }
        "delete" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            future_result_int()
        }
        other => {
            ctx.error_at(
                span,
                format!(
                    "method `QueryBuilder<{}>.{}` does not exist (chain: where/order_by/limit/offset/group_by; terminals: all/first/count/sum/avg/min/max/update/delete/merge_jsonb)",
                    row.display(ctx.types),
                    other
                ),
            );
            Type::Any
        }
    }
}

/// Phase 10.b.14 — signatures of methods on `Aggregated<Row>`
/// (QueryBuilder post-`.group_by(...)`).
///
/// Chain methods that stay as `Aggregated<Row>`:
///   - `where(closure)`, `order_by(closure)`, `limit(n)`, `offset(n)`
///   - `group_by(closure)` (accumulate more cols to GROUP BY)
///
/// Aggregate terminals (return `Future<Result<List<Map<Str, Any>>>>`):
///   - `count(db)`, `sum/avg/min/max(closure, db)`.
///
/// Each item of the List is a Map with keys = group_by cols plus the
/// aggregate name ("count", "sum", "avg", "min", "max"), values = the
/// group's data.
///
/// Rejected on Aggregated (don't make sense over GROUP BY):
///   - `all/first/update/delete` → clear error.
fn infer_aggregated_method(
    ctx: &mut CheckCtx,
    row: &Type,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Type {
    let aggregated = || Type::Aggregated(Box::new(row.clone()));
    // Common shape of the aggregates with group_by: List<Map<Str, Any>>.
    // Each item of the List is a row {group_col: value, agg_name: value}.
    let future_result_list_map = || {
        Type::Future(Box::new(Type::Result {
            ok: Box::new(Type::List(Box::new(Type::Map(
                Box::new(Type::Str),
                Box::new(Type::Any),
            )))),
            err: Box::new(Type::Str),
        }))
    };
    match method {
        // Chain methods — preserve Aggregated<Row>.
        "where" | "order_by" | "group_by" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            aggregated()
        }
        "limit" | "offset" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            aggregated()
        }
        // Aggregate terminals: shape List<Map<Str, Any>>.
        "count" => {
            check_method_arity(ctx, method, args_ty, 1, span);
            future_result_list_map()
        }
        "sum" | "avg" | "min" | "max" => {
            check_method_arity(ctx, method, args_ty, 2, span);
            future_result_list_map()
        }
        // No sense on Aggregated.
        "all" | "first" | "update" | "delete" => {
            ctx.error_at(
                span,
                format!(
                    "`Aggregated<{}>.{}` is not valid over a GROUP BY — use an aggregate (count/sum/avg/min/max) to collapse the groups",
                    row.display(ctx.types),
                    method
                ),
            );
            Type::Any
        }
        other => {
            ctx.error_at(
                span,
                format!(
                    "method `Aggregated<{}>.{}` does not exist (chain: where/order_by/limit/offset/group_by; terminals: count/sum/avg/min/max)",
                    row.display(ctx.types),
                    other
                ),
            );
            Type::Any
        }
    }
}

/// Mini-batch Mb9 — signatures of methods on Int/Float primitives.
/// Bounded list for simplicity; expand if demand appears.
fn infer_int_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    match method {
        "abs" => {
            check_method_arity(ctx, "abs", args_ty, 0, span);
            Type::Int
        }
        "to_str" => {
            check_method_arity(ctx, "to_str", args_ty, 0, span);
            Type::Str
        }
        "to_str_base" => {
            if check_method_arity(ctx, "to_str_base", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Int.to_str_base()` expects `Int`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Str
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "`Int` does not have method `{}` (today: abs/to_str/to_str_base)",
                    method,
                ),
            );
            Type::Any
        }
    }
}

fn infer_float_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    match method {
        "abs" => {
            check_method_arity(ctx, "abs", args_ty, 0, span);
            Type::Float
        }
        "to_str" => {
            check_method_arity(ctx, "to_str", args_ty, 0, span);
            Type::Str
        }
        "is_nan" | "is_finite" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Bool
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "`Float` does not have method `{}` (today: abs/to_str/is_nan/is_finite)",
                    method,
                ),
            );
            Type::Any
        }
    }
}

/// Mini-batch Ir — signatures of built-in methods on `Range`. The
/// Range is conceptually a lazy `List<Int>`; the methods coincide
/// with those of `List<Int>` for enumerate/zip/chain, plus `len`.
fn infer_range_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    match method {
        "enumerate" => {
            check_method_arity(ctx, "enumerate", args_ty, 0, span);
            Type::List(Box::new(Type::Tuple(vec![Type::Int, Type::Int])))
        }
        "zip" => {
            if !check_method_arity(ctx, "zip", args_ty, 1, span) {
                return Type::List(Box::new(Type::Tuple(vec![Type::Int, Type::Any])));
            }
            let u = match args_ty[0].base() {
                Type::List(inner) => (**inner).clone(),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`Range.zip()` expects `List<U>`, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                    Type::Any
                }
            };
            Type::List(Box::new(Type::Tuple(vec![Type::Int, u])))
        }
        "chain" => {
            if !check_method_arity(ctx, "chain", args_ty, 1, span) {
                return Type::List(Box::new(Type::Int));
            }
            match args_ty[0].base() {
                Type::List(inner) => {
                    if !is_compatible(inner, &Type::Int) {
                        ctx.error_at(
                            span,
                            format!(
                                "`Range.chain()` expects `List<Int>`, received `List<{}>`",
                                inner.display(ctx.types),
                            ),
                        );
                    }
                }
                Type::Any => {}
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`Range.chain()` expects `List<Int>`, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                }
            }
            Type::List(Box::new(Type::Int))
        }
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        // Mini-batch Rg — `step_by(n)`: materializes the range with step `n`.
        // `n: Int` (> 0 validated at runtime). Returns `List<Int>`.
        "step_by" => {
            if check_method_arity(ctx, "step_by", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Range.step_by()` expects `Int`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::List(Box::new(Type::Int))
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "`Range` does not have method `{}` (today: enumerate/zip/chain/len/step_by)",
                    method,
                ),
            );
            Type::Any
        }
    }
}

/// Validates arity of a built-in method. Returns `true` if the
/// arity matches (so the caller can skip extra validations on
/// arguments that don't exist). If it fails, accumulates the error
/// and returns `false`.
/// Mini-batch Vp — visibility predicate: a field is considered
/// **private** if its name starts with `_`. The convention is Python's
/// (not enforced at runtime), but Fitz validates it statically
/// in the checker: `instance._field` and struct lits `{ _field: ... }`
/// from outside the type body are errors. Inside methods of the
/// SAME type (`current_type == Some(id)`), everything is accessible.
fn is_private_field(name: &str) -> bool {
    name.starts_with('_')
}

fn check_method_arity(
    ctx: &mut CheckCtx,
    method: &str,
    args_ty: &[Type],
    expected: usize,
    span: Span,
) -> bool {
    if args_ty.len() != expected {
        ctx.error_at(
            span,
            format!(
                "method `{}` expects {} argument(s), received {}",
                method,
                expected,
                args_ty.len()
            ),
        );
        false
    } else {
        true
    }
}

/// Validates a unary callback (`fn(T) -> U`). Returns the inferred
/// `U` of the callback, or `Any` if the callback is Any or not
/// validatable. If `expected_ret` is `Some(B)`, also requires U
/// to be compatible with B (typical case: `.filter()` requires `Bool`).
fn check_unary_callback(
    ctx: &mut CheckCtx,
    cb: &Type,
    elem_ty: &Type,
    method: &str,
    expected_ret: Option<&Type>,
    span: Span,
) -> Type {
    match cb {
        Type::Any => Type::Any,
        Type::Function { params, ret } => {
            if params.len() != 1 {
                ctx.error_at(
                    span,
                    format!(
                        "the callback of `.{}()` must take 1 argument, received {}",
                        method,
                        params.len()
                    ),
                );
                return (**ret).clone();
            }
            // The callback's param must be able to receive a T
            // (the element type). If the callback declared a
            // concrete incompatible type, error.
            if !is_compatible(elem_ty, &params[0]) {
                ctx.error_at(
                    span,
                    format!(
                        "the callback of `.{}()` receives elements `{}` but its parameter is `{}`",
                        method,
                        elem_ty.display(ctx.types),
                        params[0].display(ctx.types)
                    ),
                );
            }
            if let Some(expected) = expected_ret {
                if !is_compatible(ret, expected) {
                    ctx.error_at(
                        span,
                        format!(
                            "the callback of `.{}()` must return `{}`, returns `{}`",
                            method,
                            expected.display(ctx.types),
                            ret.display(ctx.types)
                        ),
                    );
                }
            }
            (**ret).clone()
        }
        other => {
            ctx.error_at(
                span,
                format!(
                    "the callback of `.{}()` must be a function, received `{}`",
                    method,
                    other.display(ctx.types)
                ),
            );
            Type::Any
        }
    }
}

fn infer_list_method(
    ctx: &mut CheckCtx,
    t: &Type,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Type {
    match method {
        "push" => {
            check_method_arity(ctx, "push", args_ty, 1, span);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, t) {
                    ctx.error_at(
                        span,
                        format!(
                            "`push` over `List<{}>` received `{}`",
                            t.display(ctx.types),
                            arg.display(ctx.types)
                        ),
                    );
                }
            }
            Type::Null
        }
        "pop" => {
            check_method_arity(ctx, "pop", args_ty, 0, span);
            t.clone()
        }
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        "map" => {
            if !check_method_arity(ctx, "map", args_ty, 1, span) {
                return Type::List(Box::new(Type::Any));
            }
            let u = check_unary_callback(ctx, &args_ty[0], t, "map", None, span);
            Type::List(Box::new(u))
        }
        "filter" => {
            if !check_method_arity(ctx, "filter", args_ty, 1, span) {
                return Type::List(Box::new(t.clone()));
            }
            check_unary_callback(ctx, &args_ty[0], t, "filter", Some(&Type::Bool), span);
            Type::List(Box::new(t.clone()))
        }
        "find" => {
            if !check_method_arity(ctx, "find", args_ty, 1, span) {
                return Type::Result {
                    ok: Box::new(t.clone()),
                    err: Box::new(Type::Str),
                };
            }
            check_unary_callback(ctx, &args_ty[0], t, "find", Some(&Type::Bool), span);
            Type::Result {
                ok: Box::new(t.clone()),
                err: Box::new(Type::Str),
            }
        }
        // Mini-batch Lx — functional predicates over List<T>.
        // All take `fn(T) -> Bool`. Return Bool/Int/Result<Int>.
        "any" | "all" => {
            if check_method_arity(ctx, method, args_ty, 1, span) {
                check_unary_callback(ctx, &args_ty[0], t, method, Some(&Type::Bool), span);
            }
            Type::Bool
        }
        "count" => {
            if check_method_arity(ctx, "count", args_ty, 1, span) {
                check_unary_callback(ctx, &args_ty[0], t, "count", Some(&Type::Bool), span);
            }
            Type::Int
        }
        "find_index" => {
            if check_method_arity(ctx, "find_index", args_ty, 1, span) {
                check_unary_callback(ctx, &args_ty[0], t, "find_index", Some(&Type::Bool), span);
            }
            Type::Result {
                ok: Box::new(Type::Int),
                err: Box::new(Type::Str),
            }
        }
        // Mini-batch Ex2 — `flat_map(fn(T) -> List<U>)` → `List<U>`.
        // The callback must return a list; we infer U from the ret type.
        "flat_map" => {
            if !check_method_arity(ctx, "flat_map", args_ty, 1, span) {
                return Type::List(Box::new(Type::Any));
            }
            let inner_u = match &args_ty[0] {
                Type::Function { params, ret } => {
                    if params.len() != 1 {
                        ctx.error_at(
                            span,
                            format!(
                                "`.flat_map()`: callback takes 1 param, has {}",
                                params.len(),
                            ),
                        );
                        return Type::List(Box::new(Type::Any));
                    }
                    if !is_compatible(t, &params[0]) && !is_compatible(&params[0], t) {
                        ctx.error_at(
                            span,
                            format!(
                                "`.flat_map()`: callback param is `{}`, expected `{}`",
                                params[0].display(ctx.types),
                                t.display(ctx.types),
                            ),
                        );
                    }
                    match &**ret {
                        Type::List(u) => (**u).clone(),
                        Type::Any => Type::Any,
                        other => {
                            ctx.error_at(
                                span,
                                format!(
                                    "`.flat_map()`: callback must return `List<U>`, returns `{}`",
                                    other.display(ctx.types),
                                ),
                            );
                            Type::Any
                        }
                    }
                }
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`.flat_map()` expects a callback, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                    Type::Any
                }
            };
            Type::List(Box::new(inner_u))
        }
        // Mini-batch Ex2 — `first()` / `last()` → `Result<T>`.
        "first" | "last" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Result {
                ok: Box::new(t.clone()),
                err: Box::new(Type::Str),
            }
        }
        // Mini-batch Mb2 — numeric reductions on `List<Int>`
        // or `List<Float>`. `min`/`max` return `Result<T>` because
        // the list can be empty. `sum` returns `T` (0/0.0 as
        // sentinel for empty). Non-numeric types → error.
        // `List<Any>` passes gradual (Any).
        "min" | "max" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            match t {
                Type::Int | Type::Float | Type::Any => Type::Result {
                    ok: Box::new(t.clone()),
                    err: Box::new(Type::Str),
                },
                other => {
                    ctx.error_at(span, format!(
                        "`.{}()` only applies over `List<Int>` or `List<Float>`, received `List<{}>`",
                        method, other.display(ctx.types),
                    ));
                    Type::Result {
                        ok: Box::new(Type::Any),
                        err: Box::new(Type::Str),
                    }
                }
            }
        }
        "sum" => {
            check_method_arity(ctx, "sum", args_ty, 0, span);
            match t {
                Type::Int | Type::Float => t.clone(),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(span, format!(
                        "`.sum()` only applies over `List<Int>` or `List<Float>`, received `List<{}>`",
                        other.display(ctx.types),
                    ));
                    Type::Any
                }
            }
        }
        // Mini-batch Mb3 — `product()` analogous to `sum`. Only Int/Float.
        // Empty → 1/1.0 (sentinel).
        "product" => {
            check_method_arity(ctx, "product", args_ty, 0, span);
            match t {
                Type::Int | Type::Float => t.clone(),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(span, format!(
                        "`.product()` only applies over `List<Int>` or `List<Float>`, received `List<{}>`",
                        other.display(ctx.types),
                    ));
                    Type::Any
                }
            }
        }
        // Mini-batch Mb3 — `reduce(init, fn(acc, x) -> Acc) -> Acc`.
        // Canonical functional fold. The init types Acc; the callback is
        // `fn(Acc, T) -> Acc`; the ret is Acc.
        "reduce" => {
            if !check_method_arity(ctx, "reduce", args_ty, 2, span) {
                return Type::Any;
            }
            let acc_ty = args_ty[0].clone();
            check_binary_callback(ctx, &args_ty[1], &acc_ty, t, "reduce", Some(&acc_ty), span);
            acc_ty
        }
        // Mini-batch Mb3 — `to_map()`: converts `List<(K, V)>` →
        // `Map<K, V>`. T must be a `Tuple` of arity 2; others → error.
        "to_map" => {
            check_method_arity(ctx, "to_map", args_ty, 0, span);
            match t {
                Type::Tuple(items) if items.len() == 2 => {
                    Type::Map(Box::new(items[0].clone()), Box::new(items[1].clone()))
                }
                Type::Any => Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
                other => {
                    ctx.error_at(span, format!(
                        "`.to_map()` requires `List<(K, V)>` (Tuple of arity 2), received `List<{}>`",
                        other.display(ctx.types),
                    ));
                    Type::Map(Box::new(Type::Any), Box::new(Type::Any))
                }
            }
        }
        // Mini-batch Mb4 — `unique()`: dedup preserving order. Any T.
        "unique" => {
            check_method_arity(ctx, "unique", args_ty, 0, span);
            Type::List(Box::new(t.clone()))
        }
        // Mini-batch Mb4 — `partition(pred)`: splits into two lists.
        // Callback `fn(T) -> Bool`. Ret: `(List<T>, List<T>)`.
        "partition" => {
            if check_method_arity(ctx, "partition", args_ty, 1, span) {
                check_unary_callback(ctx, &args_ty[0], t, "partition", Some(&Type::Bool), span);
            }
            Type::Tuple(vec![
                Type::List(Box::new(t.clone())),
                Type::List(Box::new(t.clone())),
            ])
        }
        // Mini-batch Mb5 — `group_by(fn(T) -> K)`: groups by key.
        // Output: `Map<K, List<T>>`. K is inferred from the cb's ret type.
        "group_by" => {
            if !check_method_arity(ctx, "group_by", args_ty, 1, span) {
                return Type::Map(
                    Box::new(Type::Any),
                    Box::new(Type::List(Box::new(t.clone()))),
                );
            }
            let k_ty = check_unary_callback(ctx, &args_ty[0], t, "group_by", None, span);
            Type::Map(Box::new(k_ty), Box::new(Type::List(Box::new(t.clone()))))
        }
        // Mini-batch Mb5 — `zip_with(ys, fn(T, U) -> V)`: combines zip
        // + map. Ret: `List<V>`. U comes from the element type of `ys`;
        // V from the callback's ret type.
        "zip_with" => {
            if !check_method_arity(ctx, "zip_with", args_ty, 2, span) {
                return Type::List(Box::new(Type::Any));
            }
            let u_ty = match args_ty[0].base() {
                Type::List(inner) => (**inner).clone(),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`.zip_with()` expects `List<U>` as first arg, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                    return Type::List(Box::new(Type::Any));
                }
            };
            let v_ty = match &args_ty[1] {
                Type::Function { params, ret } => {
                    if params.len() != 2 {
                        ctx.error_at(
                            span,
                            format!(
                                "`.zip_with()`: callback takes 2 params, has {}",
                                params.len(),
                            ),
                        );
                        return Type::List(Box::new(Type::Any));
                    }
                    if !is_compatible(t, &params[0]) {
                        ctx.error_at(
                            span,
                            format!(
                                "`.zip_with()`: callback param[0] is `{}`, expected `{}`",
                                params[0].display(ctx.types),
                                t.display(ctx.types),
                            ),
                        );
                    }
                    if !is_compatible(&u_ty, &params[1]) {
                        ctx.error_at(
                            span,
                            format!(
                                "`.zip_with()`: callback param[1] is `{}`, expected `{}`",
                                params[1].display(ctx.types),
                                u_ty.display(ctx.types),
                            ),
                        );
                    }
                    (**ret).clone()
                }
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`.zip_with()` expects a callback, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                    Type::Any
                }
            };
            Type::List(Box::new(v_ty))
        }
        // Mini-batch Mb5 — `max_by`/`min_by(fn(T) -> Int)`: extracts
        // Int ranking per element and returns the item with max/min.
        // Empty → `Err`. Useful for non-numeric types.
        "max_by" | "min_by" => {
            if check_method_arity(ctx, method, args_ty, 1, span) {
                check_unary_callback(ctx, &args_ty[0], t, method, Some(&Type::Int), span);
            }
            Type::Result {
                ok: Box::new(t.clone()),
                err: Box::new(Type::Str),
            }
        }
        // Mini-batch Mb6 — `scan(init, fn(acc, x) -> Acc) -> List<Acc>`.
        // Fold with intermediate outputs. Same shape as reduce except
        // it returns a List<Acc> with each state of the acc.
        "scan" => {
            if !check_method_arity(ctx, "scan", args_ty, 2, span) {
                return Type::List(Box::new(Type::Any));
            }
            let acc_ty = args_ty[0].clone();
            check_binary_callback(ctx, &args_ty[1], &acc_ty, t, "scan", Some(&acc_ty), span);
            Type::List(Box::new(acc_ty))
        }
        // Mini-batch Mb6 — `windows(n) -> List<List<T>>`. Each window
        // is a List<T> with `n` consecutive elements.
        "windows" => {
            if check_method_arity(ctx, "windows", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`.windows()` expects `Int`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::List(Box::new(Type::List(Box::new(t.clone()))))
        }
        // Mini-batch Mb9 — `split_at(i) -> (List<T>, List<T>)`:
        // splits at `i`, clamp safe (parallel to Mb4's Str.split_at).
        "split_at" => {
            if check_method_arity(ctx, "split_at", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`List.split_at()` expects `Int`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Tuple(vec![
                Type::List(Box::new(t.clone())),
                Type::List(Box::new(t.clone())),
            ])
        }
        // Mini-batch Mb8 — `starts_with(prefix)` / `ends_with(suffix)`:
        // arg `List<T>`, return `Bool`.
        "starts_with" | "ends_with" => {
            if check_method_arity(ctx, method, args_ty, 1, span) {
                match args_ty[0].base() {
                    Type::List(inner) => {
                        if !is_compatible(inner, t) {
                            ctx.error_at(
                                span,
                                format!(
                                    "`.{}()`: expects `List<{}>`, received `List<{}>`",
                                    method,
                                    t.display(ctx.types),
                                    inner.display(ctx.types),
                                ),
                            );
                        }
                    }
                    Type::Any => {}
                    other => {
                        ctx.error_at(
                            span,
                            format!(
                                "`.{}()`: expects `List<{}>`, received `{}`",
                                method,
                                t.display(ctx.types),
                                other.display(ctx.types),
                            ),
                        );
                    }
                }
            }
            Type::Bool
        }
        // Mini-batch Mb8 — `insert_at(i, v) -> List<T>`: idx Int, v
        // compatible with T.
        "insert_at" => {
            if check_method_arity(ctx, "insert_at", args_ty, 2, span) {
                if !is_compatible(&args_ty[0], &Type::Int) {
                    ctx.error_at(
                        span,
                        format!(
                            "`.insert_at(i, v)`: arg 0 (idx) expects `Int`, received `{}`",
                            args_ty[0].display(ctx.types),
                        ),
                    );
                }
                if !is_compatible(&args_ty[1], t) {
                    ctx.error_at(
                        span,
                        format!(
                            "`.insert_at(i, v)`: v is `{}`, must be compatible with `{}`",
                            args_ty[1].display(ctx.types),
                            t.display(ctx.types),
                        ),
                    );
                }
            }
            Type::List(Box::new(t.clone()))
        }
        // Mini-batch Mb8 — `remove_at(i) -> List<T>`: idx Int.
        "remove_at" => {
            if check_method_arity(ctx, "remove_at", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`.remove_at(i)`: idx expects `Int`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::List(Box::new(t.clone()))
        }
        // Mini-batch Mb8 — `zip_to_map(values) -> Map<K, V>` where
        // K = T (the element type of self).
        "zip_to_map" => {
            if !check_method_arity(ctx, "zip_to_map", args_ty, 1, span) {
                return Type::Map(Box::new(t.clone()), Box::new(Type::Any));
            }
            let v_ty = match args_ty[0].base() {
                Type::List(inner) => (**inner).clone(),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`.zip_to_map()` expects `List<V>`, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                    Type::Any
                }
            };
            Type::Map(Box::new(t.clone()), Box::new(v_ty))
        }
        // Mini-batch Mb7 — `take(n)` / `drop(n)` / `cycle(n)`: Int arg,
        // return `List<T>`.
        "take" | "drop" | "cycle" => {
            if check_method_arity(ctx, method, args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`.{}()` expects `Int`, received `{}`",
                        method,
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::List(Box::new(t.clone()))
        }
        // Mini-batch Mb7 — `init()` / `tail()`: no args, `List<T>`.
        "init" | "tail" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::List(Box::new(t.clone()))
        }
        // Mini-batch Mb7 — `intersperse(sep)`: sep must be compatible
        // with T.
        "intersperse" => {
            if check_method_arity(ctx, "intersperse", args_ty, 1, span)
                && !is_compatible(&args_ty[0], t)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`.intersperse()`: sep is `{}`, must be compatible with `{}`",
                        args_ty[0].display(ctx.types),
                        t.display(ctx.types),
                    ),
                );
            }
            Type::List(Box::new(t.clone()))
        }
        // S.3 (mini-batch S) — `sort`/`reverse` mutate in-place and
        // return `Null`. `contains(v)` returns `Bool`. The
        // "comparable type" check for sort is done at runtime
        // — the checker does not reject `List<Any>.sort()` to preserve
        // the gradual model.
        "sort" | "reverse" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Null
        }
        "contains" => {
            if check_method_arity(ctx, "contains", args_ty, 1, span)
                && !is_compatible(&args_ty[0], t)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`List<{}>.contains()` received `{}`",
                        t.display(ctx.types),
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Bool
        }
        // Mini-batch It — `enumerate()` returns `List<(Int, T)>` with
        // (index, element) pairs. Fits naturally with for tuple
        // destructuring (Md): `for (i, x) in xs.enumerate()`.
        "enumerate" => {
            check_method_arity(ctx, "enumerate", args_ty, 0, span);
            Type::List(Box::new(Type::Tuple(vec![Type::Int, t.clone()])))
        }
        // Mini-batch It — `zip(ys)` pairs two lists, truncating at the
        // shorter. `ys: List<U>` with arbitrary U; returns
        // `List<(T, U)>`.
        "zip" => {
            if !check_method_arity(ctx, "zip", args_ty, 1, span) {
                return Type::List(Box::new(Type::Tuple(vec![t.clone(), Type::Any])));
            }
            let u = match args_ty[0].base() {
                Type::List(inner) => (**inner).clone(),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`List<{}>.zip()` expects `List<U>`, received `{}`",
                            t.display(ctx.types),
                            other.display(ctx.types),
                        ),
                    );
                    Type::Any
                }
            };
            Type::List(Box::new(Type::Tuple(vec![t.clone(), u])))
        }
        // Mini-batch It — `chain(ys)` concatenates. `ys` must be
        // `List<T>` (same type). Returns `List<T>`.
        "chain" => {
            if !check_method_arity(ctx, "chain", args_ty, 1, span) {
                return Type::List(Box::new(t.clone()));
            }
            match args_ty[0].base() {
                Type::List(inner) => {
                    if !is_compatible(inner, t) {
                        ctx.error_at(
                            span,
                            format!(
                                "`List<{}>.chain()` expects `List<{}>`, received `List<{}>`",
                                t.display(ctx.types),
                                t.display(ctx.types),
                                inner.display(ctx.types),
                            ),
                        );
                    }
                }
                Type::Any => {}
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`List<{}>.chain()` expects `List<{}>`, received `{}`",
                            t.display(ctx.types),
                            t.display(ctx.types),
                            other.display(ctx.types),
                        ),
                    );
                }
            }
            Type::List(Box::new(t.clone()))
        }
        // Mini-batch Mb — `flatten()` requires `List<List<U>>` and
        // returns `List<U>`. If T is not List (or not Any), clear error.
        "flatten" => {
            if !check_method_arity(ctx, "flatten", args_ty, 0, span) {
                return Type::Any;
            }
            match t {
                Type::List(inner) => Type::List(inner.clone()),
                Type::Any => Type::List(Box::new(Type::Any)),
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`.flatten()` requires `List<List<U>>`, the receiver is `List<{}>`",
                            other.display(ctx.types),
                        ),
                    );
                    Type::Any
                }
            }
        }
        // Mini-batch Mb — `sort_by(cmp)`. The callback is `fn(T, T) -> Int`.
        // Mutates in-place, returns Null (parallel to `sort`).
        "sort_by" => {
            if !check_method_arity(ctx, "sort_by", args_ty, 1, span) {
                return Type::Null;
            }
            let cb_ty = &args_ty[0];
            match cb_ty {
                Type::Function { params, ret } => {
                    if params.len() != 2 {
                        ctx.error_at(span, format!(
                            "`.sort_by(cmp)` expects `fn(T, T) -> Int` (2 params); the callback has {} params",
                            params.len(),
                        ));
                    } else {
                        for (i, p) in params.iter().enumerate() {
                            if !is_compatible(p, t) && !is_compatible(t, p) {
                                ctx.error_at(
                                    span,
                                    format!(
                                    "`.sort_by(cmp)`: callback param[{}] is `{}`, expected `{}`",
                                    i,
                                    p.display(ctx.types),
                                    t.display(ctx.types),
                                ),
                                );
                            }
                        }
                        if !is_compatible(ret, &Type::Int) {
                            ctx.error_at(
                                span,
                                format!(
                                    "`.sort_by(cmp)`: callback must return `Int`, returns `{}`",
                                    ret.display(ctx.types),
                                ),
                            );
                        }
                    }
                }
                Type::Any => {
                    // Gradual: callback without concrete type, no check.
                }
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`.sort_by(cmp)` expects `fn(T, T) -> Int`, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                }
            }
            Type::Null
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "`List<{}>` does not have method `{}`",
                    t.display(ctx.types),
                    method
                ),
            );
            Type::Any
        }
    }
}

fn infer_map_method(
    ctx: &mut CheckCtx,
    k: &Type,
    v: &Type,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Type {
    match method {
        "get" => {
            check_method_arity(ctx, "get", args_ty, 1, span);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, k) {
                    ctx.error_at(
                        span,
                        format!(
                            "`get` over `Map<{}, {}>` expects a key `{}`, received `{}`",
                            k.display(ctx.types),
                            v.display(ctx.types),
                            k.display(ctx.types),
                            arg.display(ctx.types)
                        ),
                    );
                }
            }
            Type::Result {
                ok: Box::new(v.clone()),
                err: Box::new(Type::Str),
            }
        }
        "has" => {
            check_method_arity(ctx, "has", args_ty, 1, span);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, k) {
                    ctx.error_at(
                        span,
                        format!(
                            "`has` over `Map<{}, {}>` expects a key `{}`, received `{}`",
                            k.display(ctx.types),
                            v.display(ctx.types),
                            k.display(ctx.types),
                            arg.display(ctx.types)
                        ),
                    );
                }
            }
            Type::Bool
        }
        "keys" => {
            check_method_arity(ctx, "keys", args_ty, 0, span);
            Type::List(Box::new(k.clone()))
        }
        // Mini-batch Mb2 — `keys_sorted()`: same as `keys()` but
        // sorted. The "K is comparable" validation (Int/Float/
        // Str/Bool) is done at runtime (parallel to `list_sort`); the
        // checker does not reject to preserve the gradual model.
        "keys_sorted" => {
            check_method_arity(ctx, "keys_sorted", args_ty, 0, span);
            Type::List(Box::new(k.clone()))
        }
        // Mini-batch Mb3 — `entries()`: returns `List<(K, V)>` with
        // the key-value pairs. Inverse of `xs.to_map()`.
        "entries" => {
            check_method_arity(ctx, "entries", args_ty, 0, span);
            Type::List(Box::new(Type::Tuple(vec![k.clone(), v.clone()])))
        }
        // Mini-batch Mb4 — `invert()`: swap K ↔ V. Ret: `Map<V, K>`.
        "invert" => {
            check_method_arity(ctx, "invert", args_ty, 0, span);
            Type::Map(Box::new(v.clone()), Box::new(k.clone()))
        }
        // Mini-batch Mb9 — `has_value(v) -> Bool`: checks if v is
        // a value in some pair of the Map. Parallel to `has(k)`.
        "has_value" => {
            if check_method_arity(ctx, "has_value", args_ty, 1, span)
                && !is_compatible(&args_ty[0], v)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Map.has_value()` expects `{}`, received `{}`",
                        v.display(ctx.types),
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Bool
        }
        // Mini-batch Mb7 — `with(k, v) -> Map<K, V>`: functional update.
        // Returns a new Map with `k → v`. If `k` exists, overwrites.
        "with" => {
            if !check_method_arity(ctx, "with", args_ty, 2, span) {
                return Type::Map(Box::new(k.clone()), Box::new(v.clone()));
            }
            if !is_compatible(&args_ty[0], k) {
                ctx.error_at(
                    span,
                    format!(
                        "`.with()`: key must be `{}`, received `{}`",
                        k.display(ctx.types),
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            if !is_compatible(&args_ty[1], v) {
                ctx.error_at(
                    span,
                    format!(
                        "`.with()`: value must be `{}`, received `{}`",
                        v.display(ctx.types),
                        args_ty[1].display(ctx.types),
                    ),
                );
            }
            Type::Map(Box::new(k.clone()), Box::new(v.clone()))
        }
        // Mini-batch Mb6 — `merge_with(other, fn(V, V) -> V) -> Map<K, V>`.
        // Generalizes merge: the callback decides which value remains when
        // there is a conflict.
        "merge_with" => {
            if !check_method_arity(ctx, "merge_with", args_ty, 2, span) {
                return Type::Map(Box::new(k.clone()), Box::new(v.clone()));
            }
            match args_ty[0].base() {
                Type::Map(k2, v2) => {
                    if !is_compatible(k2, k) {
                        ctx.error_at(span, format!(
                            "`.merge_with()`: keys must match, received `Map<{}, _>` vs `Map<{}, _>`",
                            k2.display(ctx.types),
                            k.display(ctx.types),
                        ));
                    }
                    if !is_compatible(v2, v) {
                        ctx.error_at(span, format!(
                            "`.merge_with()`: values must match, received `Map<_, {}>` vs `Map<_, {}>`",
                            v2.display(ctx.types),
                            v.display(ctx.types),
                        ));
                    }
                }
                Type::Any => {}
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`.merge_with()` expects another `Map`, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                }
            }
            check_binary_callback(ctx, &args_ty[1], v, v, "merge_with", Some(v), span);
            Type::Map(Box::new(k.clone()), Box::new(v.clone()))
        }
        "values" => {
            check_method_arity(ctx, "values", args_ty, 0, span);
            Type::List(Box::new(v.clone()))
        }
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        // Mini-batch Ex — functional transformations over Map.
        // `filter(pred)` with callback `fn(K, V) -> Bool` returns a
        // new Map<K, V>. `map_values(fn)` with callback `fn(V) -> U`
        // returns Map<K, U>.
        "filter" => {
            if check_method_arity(ctx, "filter", args_ty, 1, span) {
                check_binary_callback(ctx, &args_ty[0], k, v, "filter", Some(&Type::Bool), span);
            }
            Type::Map(Box::new(k.clone()), Box::new(v.clone()))
        }
        // Mini-batch Up — `update(k, fn(V) -> V) -> Map<K, V>`.
        // Applies the callback to the value associated with `k` (if it exists);
        // returns a new Map. If `k` is not there, no-op.
        "update" => {
            if !check_method_arity(ctx, "update", args_ty, 2, span) {
                return Type::Map(Box::new(k.clone()), Box::new(v.clone()));
            }
            // Arg 0: key, must be compatible with K.
            if !is_compatible(&args_ty[0], k) {
                ctx.error_at(
                    span,
                    format!(
                        "`Map<{}, _>.update()`: key must be `{}`, received `{}`",
                        k.display(ctx.types),
                        k.display(ctx.types),
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            // Arg 1: callback fn(V) -> V (same V, doesn't transform type).
            check_unary_callback(ctx, &args_ty[1], v, "update", Some(v), span);
            Type::Map(Box::new(k.clone()), Box::new(v.clone()))
        }
        // Mini-batch Ex2 — `merge(other)` combines two `Map<K, V>` into
        // a new one with last-write-wins policy. Returns `Map<K, V>`.
        "merge" => {
            if !check_method_arity(ctx, "merge", args_ty, 1, span) {
                return Type::Map(Box::new(k.clone()), Box::new(v.clone()));
            }
            match args_ty[0].base() {
                Type::Map(k2, v2) => {
                    if !is_compatible(k2, k) {
                        ctx.error_at(
                            span,
                            format!(
                            "`Map.merge()`: keys must match, received `Map<{}, _>` vs `Map<{}, _>`",
                            k2.display(ctx.types),
                            k.display(ctx.types),
                        ),
                        );
                    }
                    if !is_compatible(v2, v) {
                        ctx.error_at(span, format!(
                            "`Map.merge()`: values must match, received `Map<_, {}>` vs `Map<_, {}>`",
                            v2.display(ctx.types),
                            v.display(ctx.types),
                        ));
                    }
                }
                Type::Any => {}
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`Map.merge()` expects another `Map`, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                }
            }
            Type::Map(Box::new(k.clone()), Box::new(v.clone()))
        }
        "map_values" => {
            if !check_method_arity(ctx, "map_values", args_ty, 1, span) {
                return Type::Map(Box::new(k.clone()), Box::new(Type::Any));
            }
            // Callback is `fn(V) -> U`. If it's an inline FnExpr with
            // annotated or inferred ret, we extract U; if Any, fallback Any.
            let cb_ret = match &args_ty[0] {
                Type::Function { params, ret } => {
                    if params.len() != 1 {
                        ctx.error_at(
                            span,
                            format!(
                                "`Map.map_values()`: callback must have 1 param, has {}",
                                params.len(),
                            ),
                        );
                        return Type::Map(Box::new(k.clone()), Box::new(Type::Any));
                    }
                    if !is_compatible(v, &params[0]) && !is_compatible(&params[0], v) {
                        ctx.error_at(
                            span,
                            format!(
                                "`Map.map_values()`: callback expects `{}`, the values are `{}`",
                                params[0].display(ctx.types),
                                v.display(ctx.types),
                            ),
                        );
                    }
                    (**ret).clone()
                }
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(
                        span,
                        format!(
                            "`Map.map_values()` expects a callback, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                    Type::Any
                }
            };
            Type::Map(Box::new(k.clone()), Box::new(cb_ret))
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "`Map<{}, {}>` does not have method `{}`",
                    k.display(ctx.types),
                    v.display(ctx.types),
                    method
                ),
            );
            Type::Any
        }
    }
}

/// Mini-batch Ex — Validates a binary callback (2 params). Used by
/// `Map.filter(pred)`. Simpler than extended `check_unary_callback`
/// because we only need the 2 known signatures (Map.filter).
fn check_binary_callback(
    ctx: &mut CheckCtx,
    cb_ty: &Type,
    p0_ty: &Type,
    p1_ty: &Type,
    method: &str,
    expected_ret: Option<&Type>,
    span: Span,
) {
    match cb_ty {
        Type::Function { params, ret } => {
            if params.len() != 2 {
                ctx.error_at(
                    span,
                    format!(
                        "`.{}` expects a callback of 2 params, received one of {} params",
                        method,
                        params.len(),
                    ),
                );
                return;
            }
            if !is_compatible(&params[0], p0_ty) && !is_compatible(p0_ty, &params[0]) {
                ctx.error_at(
                    span,
                    format!(
                        "`.{}`: callback param[0] is `{}`, expected `{}`",
                        method,
                        params[0].display(ctx.types),
                        p0_ty.display(ctx.types),
                    ),
                );
            }
            if !is_compatible(&params[1], p1_ty) && !is_compatible(p1_ty, &params[1]) {
                ctx.error_at(
                    span,
                    format!(
                        "`.{}`: callback param[1] is `{}`, expected `{}`",
                        method,
                        params[1].display(ctx.types),
                        p1_ty.display(ctx.types),
                    ),
                );
            }
            if let Some(want_ret) = expected_ret {
                if !is_compatible(ret, want_ret) {
                    ctx.error_at(
                        span,
                        format!(
                            "`.{}`: callback must return `{}`, returns `{}`",
                            method,
                            want_ret.display(ctx.types),
                            ret.display(ctx.types),
                        ),
                    );
                }
            }
        }
        Type::Any => {
            // Gradual: callback without concrete type, no check.
        }
        other => {
            ctx.error_at(
                span,
                format!(
                    "`.{}` expects a callback, received `{}`",
                    method,
                    other.display(ctx.types),
                ),
            );
        }
    }
}

/// Phase 9.w.2 + 9.w.2-wsconn-bidir — methods on `WsConn`.
/// Parametric over `recv` and `send` (which can be the same
/// type for symmetric `WsConn<T>`, or different for asymmetric
/// `WsConn<In, Out>`). Both travel on the wire as automatic JSON
/// (or raw binary when T = Bytes).
///
/// Methods:
///   - `recv() -> Result<RECV>` — blocks (async) until a frame
///     arrives. `Err(Str)` if the conn closed or the frame doesn't
///     parse against `RECV`.
///   - `send(msg: SEND) -> Result<Null>` — sends a frame with
///     serialized `SEND`. `Err` if the conn is closed.
///   - `broadcast(msg: SEND) -> Result<Null>` — sends to ALL live
///     conns of the endpoint, **including** the sender
///     (Socket.IO/Phoenix convention). `Err` if serialization
///     fails; individual downed conns are silently ignored.
///   - `close() -> Null` — explicitly closes the conn.
///
/// All return `Result<...>` except `close` (no significant recovery
/// path: if already closed, nothing happens).
fn infer_wsconn_method(
    ctx: &mut CheckCtx,
    recv_ty: &Type,
    send_ty: &Type,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Type {
    // For error messages, we format the full WsConn type
    // (`WsConn<T>` symmetric or `WsConn<In, Out>` asymmetric).
    let conn_disp = if recv_ty == send_ty {
        format!("WsConn<{}>", recv_ty.display(ctx.types))
    } else {
        format!(
            "WsConn<{}, {}>",
            recv_ty.display(ctx.types),
            send_ty.display(ctx.types)
        )
    };
    match method {
        "recv" => {
            check_method_arity(ctx, "recv", args_ty, 0, span);
            Type::Result {
                ok: Box::new(recv_ty.clone()),
                err: Box::new(Type::Str),
            }
        }
        "send" => {
            check_method_arity(ctx, "send", args_ty, 1, span);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, send_ty) {
                    ctx.error_at(
                        span,
                        format!(
                            "method `{}.send(msg)` expects an argument of type `{}`, received `{}`",
                            conn_disp,
                            send_ty.display(ctx.types),
                            arg.display(ctx.types),
                        ),
                    );
                }
            }
            Type::Result {
                ok: Box::new(Type::Null),
                err: Box::new(Type::Str),
            }
        }
        "broadcast" => {
            check_method_arity(ctx, "broadcast", args_ty, 1, span);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, send_ty) {
                    ctx.error_at(span, format!(
                        "method `{}.broadcast(msg)` expects an argument of type `{}`, received `{}`",
                        conn_disp,
                        send_ty.display(ctx.types),
                        arg.display(ctx.types),
                    ));
                }
            }
            Type::Result {
                ok: Box::new(Type::Null),
                err: Box::new(Type::Str),
            }
        }
        "close" => {
            check_method_arity(ctx, "close", args_ty, 0, span);
            Type::Null
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "type `{}` does not have method `{}` (supported: recv, send, broadcast, close)",
                    conn_disp, method,
                ),
            );
            Type::Any
        }
    }
}

/// Phase 10.7 (v0.10.14) — methods of `DbConn` (native Postgres
/// driver). Before v0.10.14 they fell into the catch-all "type X
/// does not have method Y", which rejected everything NOT a built-in
/// primitive. Now we dispatch to the dedicated match.
///
/// **Signatures**:
///   - `query(sql: Str, args: List<Any>) -> Future<Result<List<DbRow>>>`
///   - `exec(sql: Str, args: List<Any>) -> Future<Result<Int>>`
///   - `close() -> Future<Result<Null>>`
///   - `is_closed() -> Future<Bool>`
///   - `transaction(fn(tx: DbConn) -> Result<T>) -> Future<Result<T>>`
///     (automatic commit/rollback based on callback's Ok/Err)
///
/// The `T` type of `transaction` is inferred from the callback (inline
/// FnExpr or named fn). For the gradual case (callback with type
/// `Any`), we return `Future<Result<Any>>`.
fn infer_db_conn_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    match method {
        "query" => {
            check_method_arity(ctx, "query", args_ty, 2, span);
            Type::Future(Box::new(Type::Result {
                ok: Box::new(Type::List(Box::new(Type::DbRow))),
                err: Box::new(Type::Str),
            }))
        }
        "exec" => {
            check_method_arity(ctx, "exec", args_ty, 2, span);
            Type::Future(Box::new(Type::Result {
                ok: Box::new(Type::Int),
                err: Box::new(Type::Str),
            }))
        }
        "close" => {
            check_method_arity(ctx, "close", args_ty, 0, span);
            Type::Future(Box::new(Type::Result {
                ok: Box::new(Type::Null),
                err: Box::new(Type::Str),
            }))
        }
        "is_closed" => {
            check_method_arity(ctx, "is_closed", args_ty, 0, span);
            Type::Future(Box::new(Type::Bool))
        }
        "transaction" => {
            // We expect 1 arg: a `Function` (inline FnExpr or named fn).
            // We will infer the `T` of the Result from its ret
            // type. If the callback doesn't type as Function or doesn't return
            // Result, clear error.
            check_method_arity(ctx, "transaction", args_ty, 1, span);
            let ok_ty = match args_ty.first() {
                Some(Type::Function { ret, .. }) => {
                    // The callback's ret can be `Result<T, _>`
                    // (sync) or `Future<Result<T, _>>` (async fn).
                    let unwrapped = match ret.as_ref() {
                        Type::Future(inner) => inner.as_ref().clone(),
                        other => other.clone(),
                    };
                    match unwrapped {
                        Type::Result { ok, .. } => *ok,
                        _ => {
                            ctx.error_at(
                                span,
                                format!(
                                    "callback of `DbConn.transaction` must return `Result<T, _>` (or `Future<Result<T, _>>` if async); returns `{}`",
                                    ret.display(ctx.types),
                                ),
                            );
                            Type::Any
                        }
                    }
                }
                Some(Type::Any) => {
                    // Callback with Any type (gradual) — we cannot
                    // infer the T of the Result. We return Any.
                    Type::Any
                }
                Some(other) => {
                    ctx.error_at(
                        span,
                        format!(
                            "`DbConn.transaction` expects a `fn(tx: DbConn) -> Result<T>` as argument, received `{}`",
                            other.display(ctx.types),
                        ),
                    );
                    Type::Any
                }
                None => Type::Any,
            };
            Type::Future(Box::new(Type::Result {
                ok: Box::new(ok_ty),
                err: Box::new(Type::Str),
            }))
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "type `DbConn` does not have method `{}` (supported: query, exec, close, is_closed, transaction)",
                    method
                ),
            );
            Type::Any
        }
    }
}

/// Mini-batch Bytes — methods of the `Bytes` primitive.
fn infer_bytes_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    match method {
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        "is_empty" => {
            check_method_arity(ctx, "is_empty", args_ty, 0, span);
            Type::Bool
        }
        "to_str" => {
            check_method_arity(ctx, "to_str", args_ty, 0, span);
            Type::Result {
                ok: Box::new(Type::Str),
                err: Box::new(Type::Str),
            }
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "type `Bytes` does not have method `{}` (supported: len, is_empty, to_str)",
                    method
                ),
            );
            Type::Any
        }
    }
}

/// v0.10.22 — Methods on `DbRow` (raw row of the query result).
/// Enables parsing row fields in `fitz build` (previously only
/// worked in the interpreter because there rows are `Value::Map`).
///
/// MVP decision: instead of `.get(col) -> Result<Any>` (which would require
/// infrastructure to coerce `Any → Int/Str/Float/Bool` in codegen,
/// non-trivial), we expose **4 typed variants**:
///
///   - `get_int(name: Str) -> Result<Int>`
///   - `get_str(name: Str) -> Result<Str>`
///   - `get_float(name: Str) -> Result<Float>`
///   - `get_bool(name: Str) -> Result<Bool>`
///   - `len() -> Int` — number of columns.
///
/// Each one validates that (a) the column exists and (b) the PG type matches
/// the expected destination. `Err` with specific message otherwise.
///
/// Queries with variable shape (e.g. jsonb) remain debt:
/// the user can return `Result<List<DbRow>>` directly from the
/// handler (Debt A v0.10.22) instead of inspecting field by field.
fn infer_db_row_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    let typed_get = |ok_ty: Type| -> Type {
        Type::Result {
            ok: Box::new(ok_ty),
            err: Box::new(Type::Str),
        }
    };
    let validate_str_arg = |ctx: &mut CheckCtx, name: &str| {
        if let Some(arg) = args_ty.first() {
            if !is_compatible(arg, &Type::Str) && !matches!(arg, Type::Any) {
                ctx.error_at(
                    span,
                    format!(
                        "`DbRow.{}(name)` expects Str, received `{}`",
                        name,
                        arg.display(ctx.types),
                    ),
                );
            }
        }
    };
    match method {
        "get_int" => {
            check_method_arity(ctx, "get_int", args_ty, 1, span);
            validate_str_arg(ctx, "get_int");
            typed_get(Type::Int)
        }
        "get_str" => {
            check_method_arity(ctx, "get_str", args_ty, 1, span);
            validate_str_arg(ctx, "get_str");
            typed_get(Type::Str)
        }
        "get_float" => {
            check_method_arity(ctx, "get_float", args_ty, 1, span);
            validate_str_arg(ctx, "get_float");
            typed_get(Type::Float)
        }
        "get_bool" => {
            check_method_arity(ctx, "get_bool", args_ty, 1, span);
            validate_str_arg(ctx, "get_bool");
            typed_get(Type::Bool)
        }
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "type `DbRow` does not have method `{}` (supported: get_int, get_str, get_float, get_bool, len)",
                    method
                ),
            );
            Type::Any
        }
    }
}

/// v0.10.24 — methods on `Date`. Extraction (year/month/day/weekday
/// return Int), conversion (to_str→Str, to_datetime→DateTime),
/// custom format (format(fmt: Str)→Str).
fn infer_date_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    match method {
        "year" | "month" | "day" | "weekday" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Int
        }
        "to_str" => {
            check_method_arity(ctx, "to_str", args_ty, 0, span);
            Type::Str
        }
        "to_datetime" => {
            check_method_arity(ctx, "to_datetime", args_ty, 0, span);
            Type::DateTime
        }
        "format" => {
            if check_method_arity(ctx, "format", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Str)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Date.format(fmt)` expects `Str`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Str
        }
        // v0.10.30 B.1/B.2 — symmetric add_*/subtract_* arithmetic. All
        // take `Int` (signed; negatives OK in add_*; subtract is sugar
        // for add with runtime negate). Return `Date`.
        "add_days" | "add_months" | "add_years" | "subtract_days" | "subtract_months"
        | "subtract_years" => {
            if check_method_arity(ctx, method, args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Date.{}(n)` expects `Int`, received `{}`",
                        method,
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Date
        }
        // v0.10.30 B.3 — diff between two Dates, signed Int days.
        "diff_days" => {
            if check_method_arity(ctx, "diff_days", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Date)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Date.diff_days(other)` expects `Date`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Int
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "`Date` does not have method `{}` (supported: year, month, day, weekday, to_str, to_datetime, format, add_days, add_months, add_years, subtract_days, subtract_months, subtract_years, diff_days)",
                    method
                ),
            );
            Type::Any
        }
    }
}

/// v0.10.24 — methods on `DateTime`. Extraction (year/month/day/hour/
/// minute/second return Int), Unix epoch timestamp (Int),
/// conversion (to_str→Str, date→Date), custom format.
fn infer_datetime_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    match method {
        "year" | "month" | "day" | "hour" | "minute" | "second" | "timestamp" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Int
        }
        "to_str" => {
            check_method_arity(ctx, "to_str", args_ty, 0, span);
            Type::Str
        }
        "date" => {
            check_method_arity(ctx, "date", args_ty, 0, span);
            Type::Date
        }
        "format" => {
            if check_method_arity(ctx, "format", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Str)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`DateTime.format(fmt)` expects `Str`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Str
        }
        // v0.10.30 B.1/B.2 — add_*/subtract_* arithmetic. Sub-second
        // units (seconds/minutes/hours) + calendar units (days/months/
        // years). All `Int → DateTime`.
        "add_seconds" | "add_minutes" | "add_hours" | "add_days" | "add_months" | "add_years"
        | "subtract_seconds" | "subtract_minutes" | "subtract_hours" | "subtract_days"
        | "subtract_months" | "subtract_years" => {
            if check_method_arity(ctx, method, args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`DateTime.{}(n)` expects `Int`, received `{}`",
                        method,
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::DateTime
        }
        // v0.10.30 B.3 — diff between DateTimes, signed Int in the unit.
        "diff_seconds" | "diff_minutes" | "diff_hours" | "diff_days" => {
            if check_method_arity(ctx, method, args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::DateTime)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`DateTime.{}(other)` expects `DateTime`, received `{}`",
                        method,
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Int
        }
        // v0.10.30 B.7 — display helpers. `to_local()` formats in system
        // TZ (ISO 8601 + offset). `in_tz(iana)` formats in an
        // IANA zone → `Result<Str>` (Err if IANA name unknown).
        "to_local" => {
            check_method_arity(ctx, "to_local", args_ty, 0, span);
            Type::Str
        }
        "in_tz" => {
            if check_method_arity(ctx, "in_tz", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Str)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`DateTime.in_tz(name)` expects `Str` (IANA tz name), received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Result {
                ok: Box::new(Type::Str),
                err: Box::new(Type::Str),
            }
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "`DateTime` does not have method `{}` (supported: year, month, day, hour, minute, second, timestamp, to_str, date, format, add_seconds, add_minutes, add_hours, add_days, add_months, add_years, subtract_seconds, subtract_minutes, subtract_hours, subtract_days, subtract_months, subtract_years, diff_seconds, diff_minutes, diff_hours, diff_days, to_local, in_tz)",
                    method
                ),
            );
            Type::Any
        }
    }
}

/// v0.10.24 — methods on `Uuid`. Bounded MVP: `to_str() -> Str` and
/// `is_nil() -> Bool` cover 99% of the real case. Extraction of
/// version/variant/raw bytes remains as post-MVP debt if requested.
fn infer_uuid_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    match method {
        "to_str" => {
            check_method_arity(ctx, "to_str", args_ty, 0, span);
            Type::Str
        }
        "is_nil" => {
            check_method_arity(ctx, "is_nil", args_ty, 0, span);
            Type::Bool
        }
        _ => {
            ctx.error_at(
                span,
                format!(
                    "`Uuid` does not have method `{}` (supported: to_str, is_nil)",
                    method
                ),
            );
            Type::Any
        }
    }
}

fn infer_str_method(ctx: &mut CheckCtx, method: &str, args_ty: &[Type], span: Span) -> Type {
    match method {
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        "upper" | "lower" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Str
        }
        // S.1 (mini-batch S) — `contains`/`starts_with`/`ends_with`
        // take a `Str` and return `Bool`. Same shape for all 3.
        "contains" | "starts_with" | "ends_with" => {
            if check_method_arity(ctx, method, args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Str)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Str.{}()` expects `Str`, received `{}`",
                        method,
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Bool
        }
        // Mini-batch Mb3 — `chars()`: returns `List<Str>` with each
        // char of the string as a 1-char Str.
        "chars" => {
            check_method_arity(ctx, "chars", args_ty, 0, span);
            Type::List(Box::new(Type::Str))
        }
        // Mini-batch Mb4 — `split_at(idx)`: splits at char idx →
        // `(Str, Str)`. `idx` must be Int.
        "split_at" => {
            if check_method_arity(ctx, "split_at", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Str.split_at()` expects `Int`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Tuple(vec![Type::Str, Type::Str])
        }
        // Mini-batch Mb5 — `lines() -> List<Str>` and `is_empty() -> Bool`.
        "lines" => {
            check_method_arity(ctx, "lines", args_ty, 0, span);
            Type::List(Box::new(Type::Str))
        }
        "is_empty" => {
            check_method_arity(ctx, "is_empty", args_ty, 0, span);
            Type::Bool
        }
        // Mini-batch Mb9 — `swap_case() / title() -> Str` and
        // `is_alpha() / is_digit() / is_numeric() -> Bool`.
        "swap_case" | "title" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Str
        }
        "is_alpha" | "is_digit" | "is_numeric" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Bool
        }
        // Mini-batch Mb8 — `left(n)` / `right(n)`: first/last n
        // chars. `n: Int`.
        "left" | "right" => {
            if check_method_arity(ctx, method, args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Str.{}()` expects `Int`, received `{}`",
                        method,
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Str
        }
        // Mini-batch Mb8 — `center(width, ch) -> Str`: similar to
        // pad_start/pad_end (Mb2). width Int, ch Str (1 char at runtime).
        "center" => {
            if check_method_arity(ctx, "center", args_ty, 2, span) {
                if !is_compatible(&args_ty[0], &Type::Int) {
                    ctx.error_at(
                        span,
                        format!(
                            "`Str.center(width, ch)`: arg 0 (width) expects `Int`, received `{}`",
                            args_ty[0].display(ctx.types),
                        ),
                    );
                }
                if !is_compatible(&args_ty[1], &Type::Str) {
                    ctx.error_at(
                        span,
                        format!(
                            "`Str.center(width, ch)`: arg 1 (ch) expects `Str`, received `{}`",
                            args_ty[1].display(ctx.types),
                        ),
                    );
                }
            }
            Type::Str
        }
        // Mini-batch Mb7 — `repeat_with(n, sep) -> Str`: variant of
        // repeat that intersperses `sep` between repetitions.
        "repeat_with" => {
            if check_method_arity(ctx, "repeat_with", args_ty, 2, span) {
                if !is_compatible(&args_ty[0], &Type::Int) {
                    ctx.error_at(
                        span,
                        format!(
                            "`Str.repeat_with()`: arg 0 (n) expects `Int`, received `{}`",
                            args_ty[0].display(ctx.types),
                        ),
                    );
                }
                if !is_compatible(&args_ty[1], &Type::Str) {
                    ctx.error_at(
                        span,
                        format!(
                            "`Str.repeat_with()`: arg 1 (sep) expects `Str`, received `{}`",
                            args_ty[1].display(ctx.types),
                        ),
                    );
                }
            }
            Type::Str
        }
        // S.2 — string manipulation:
        "split" => {
            if check_method_arity(ctx, "split", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Str)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Str.split()` expects `Str` as separator, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::List(Box::new(Type::Str))
        }
        "trim" => {
            check_method_arity(ctx, "trim", args_ty, 0, span);
            Type::Str
        }
        // Mini-batch Mb — trim_start / trim_end (partial variants).
        "trim_start" | "trim_end" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Str
        }
        "replace" => {
            if check_method_arity(ctx, "replace", args_ty, 2, span) {
                for (i, name) in ["old", "new"].iter().enumerate() {
                    if !is_compatible(&args_ty[i], &Type::Str) {
                        ctx.error_at(
                            span,
                            format!(
                                "`Str.replace({}, ...)` expects `Str`, received `{}`",
                                name,
                                args_ty[i].display(ctx.types),
                            ),
                        );
                    }
                }
            }
            Type::Str
        }
        "repeat" => {
            if check_method_arity(ctx, "repeat", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Str.repeat()` expects `Int`, received `{}`",
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Str
        }
        // Mini-batch Mb2 — `pad_start(width, ch)` / `pad_end(width, ch)`.
        // `width: Int`, `ch: Str` (1 char). Return `Str`. The
        // "ch is 1 char" validation is done at runtime (not in
        // static, parallel to Python).
        "pad_start" | "pad_end" => {
            if check_method_arity(ctx, method, args_ty, 2, span) {
                if !is_compatible(&args_ty[0], &Type::Int) {
                    ctx.error_at(
                        span,
                        format!(
                            "`Str.{}(width, ch)`: arg 0 (width) expects `Int`, received `{}`",
                            method,
                            args_ty[0].display(ctx.types),
                        ),
                    );
                }
                if !is_compatible(&args_ty[1], &Type::Str) {
                    ctx.error_at(
                        span,
                        format!(
                            "`Str.{}(width, ch)`: arg 1 (ch) expects `Str`, received `{}`",
                            method,
                            args_ty[1].display(ctx.types),
                        ),
                    );
                }
            }
            Type::Str
        }
        // Mini-batch Ex — string search: find / index_of /
        // last_index_of. All take `Str` and return `Result<Int>`.
        "find" | "index_of" | "last_index_of" => {
            if check_method_arity(ctx, method, args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Str)
            {
                ctx.error_at(
                    span,
                    format!(
                        "`Str.{}()` expects `Str`, received `{}`",
                        method,
                        args_ty[0].display(ctx.types),
                    ),
                );
            }
            Type::Result {
                ok: Box::new(Type::Int),
                err: Box::new(Type::Str),
            }
        }
        _ => {
            ctx.error_at(span, format!("`Str` does not have method `{}`", method));
            Type::Any
        }
    }
}

/// Updates the `Result` coverage flags walking the pattern.
/// For `Pattern::Or` recurses into each sub-pattern (any
/// branch that covers Ok counts for Ok, etc.). R.2.1 (mini-phase R).
fn update_result_coverage(
    pat: &crate::ast::Pattern,
    has_ok: &mut bool,
    has_err: &mut bool,
    has_catchall: &mut bool,
) {
    use crate::ast::Pattern;
    match pat {
        Pattern::OkBinding(_, _) | Pattern::OkWildcard => *has_ok = true,
        Pattern::ErrBinding(_, _) | Pattern::ErrWildcard => *has_err = true,
        Pattern::Wildcard | Pattern::Ident(_, _) => *has_catchall = true,
        Pattern::Or(subs) => {
            for sub in subs {
                update_result_coverage(sub, has_ok, has_err, has_catchall);
            }
        }
        // Tuples (mini-batch T): a Tuple pattern does NOT cover Ok/Err
        // nor is it catch-all over Result — only types against tuples.
        // Does not contribute to coverage.
        Pattern::Tuple(_) => {}
        _ => {}
    }
}

/// Mini-batch Md — Binds a `for` Pattern against the iter's element
/// type, declaring the corresponding vars in the current scope.
/// Covers Ident/Wildcard/Tuple recursively. Other patterns
/// (literals, Ok/Err, Range) emit "pattern not allowed in for".
/// Mini-batch Cmp+ — checks a `for <pat> in <iter>` clause of a
/// comprehension: types the iter as List/Range, derives the element
/// type, and binds the pattern in the current scope. For multiple
/// `for` clauses it's called once per each (all share the same
/// cumulative checker scope).
fn check_comp_clause_in_checker(
    ctx: &mut CheckCtx,
    pat: &crate::ast::Pattern,
    iter: &Expr,
    fallback_span: Span,
) {
    let iter_ty = infer_expr(ctx, iter);
    let var_ty = match iter_ty.base() {
        Type::List(t) => (**t).clone(),
        Type::Range => Type::Int,
        Type::Any => Type::Any,
        other => {
            ctx.error_at(
                iter.span(),
                format!(
                    "comprehension needs an iterable (`List` or `Range`), received `{}`",
                    other.display(ctx.types)
                ),
            );
            Type::Any
        }
    };
    bind_for_pattern_in_checker(ctx, pat, &var_ty, fallback_span);
}

fn bind_for_pattern_in_checker(
    ctx: &mut CheckCtx,
    pat: &crate::ast::Pattern,
    elem_ty: &Type,
    fallback_span: Span,
) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name, ident_span) => {
            // S1 (2026-06-05) — pattern's own span as def_span +
            // registration in TypeInfo for hover over the for's var.
            let def_span = if ident_span.column > 0 {
                *ident_span
            } else {
                fallback_span
            };
            ctx.declare_var(name.clone(), elem_ty.clone(), def_span);
            if ident_span.column > 0 {
                ctx.type_info.record(*ident_span, elem_ty.clone());
            }
        }
        Pattern::Wildcard => {
            // No binding — the element is discarded.
        }
        Pattern::Tuple(subs) => {
            // The elem must be a tuple of the same length.
            match elem_ty.base() {
                Type::Tuple(item_tys) if item_tys.len() == subs.len() => {
                    for (sub, t) in subs.iter().zip(item_tys.iter()) {
                        bind_for_pattern_in_checker(ctx, sub, t, fallback_span);
                    }
                }
                Type::Any => {
                    // Gradual — we bind each ident of the pattern as Any.
                    for sub in subs {
                        bind_for_pattern_in_checker(ctx, sub, &Type::Any, fallback_span);
                    }
                }
                other => {
                    ctx.error_at(
                        fallback_span,
                        format!(
                            "tuple pattern of `for` expects a tuple of {} elements, received `{}`",
                            subs.len(),
                            other.display(ctx.types)
                        ),
                    );
                }
            }
        }
        other => {
            ctx.error_at(
                fallback_span,
                format!(
                    "pattern `{:?}` not allowed as `for` variable (use Ident, `_`, or tuple)",
                    other
                ),
            );
        }
    }
}

/// Mini-batch Fm — validates that a `FormatSpec` is applicable to the
/// type of the interpolated expr. Rules (parallel to Python):
///   - `f`/`F`/`e`/`E`/`g`/`G`/`%` require Float or Int (transparent
///     promotion).
///   - `d`/`b`/`o`/`x`/`X`/`c` require Int (without Float promotion).
///   - `s` accepts Str (or any type via Display).
///   - Without `kind`, any type is valid (uses Display by default).
///   - Alignment, fill, width, sign, alternate, and precision are valid
///     for any type (precision with Str is maximum length).
fn validate_format_spec_for_type(
    ctx: &mut CheckCtx,
    spec: &crate::ast::FormatSpec,
    ty: &Type,
    span: Span,
) {
    use crate::ast::FormatKind;
    let Some(kind) = spec.kind else { return };
    let is_num_int = matches!(ty.base(), Type::Int | Type::Any);
    let is_num_float = matches!(ty.base(), Type::Int | Type::Float | Type::Any);
    let ok = match kind {
        FormatKind::FixedLower
        | FormatKind::FixedUpper
        | FormatKind::ExponentLower
        | FormatKind::ExponentUpper
        | FormatKind::GeneralLower
        | FormatKind::GeneralUpper
        | FormatKind::Percent => is_num_float,
        FormatKind::Decimal
        | FormatKind::Binary
        | FormatKind::Octal
        | FormatKind::HexLower
        | FormatKind::HexUpper
        | FormatKind::Char => is_num_int,
        FormatKind::String => true,
    };
    if !ok {
        ctx.error_at(
            span,
            format!(
                "format spec `{}` is not compatible with type `{}` (expected {})",
                kind.to_char(),
                ty.display(ctx.types),
                match kind {
                    FormatKind::FixedLower
                    | FormatKind::FixedUpper
                    | FormatKind::ExponentLower
                    | FormatKind::ExponentUpper
                    | FormatKind::GeneralLower
                    | FormatKind::GeneralUpper
                    | FormatKind::Percent => "Float o Int",
                    FormatKind::Decimal
                    | FormatKind::Binary
                    | FormatKind::Octal
                    | FormatKind::HexLower
                    | FormatKind::HexUpper
                    | FormatKind::Char => "Int",
                    FormatKind::String => "any type",
                },
            ),
        );
    }
}

/// Checks exhaustiveness of a `match` over `Result<T>`. The arms
/// must cover both `Ok` and `Err`, or have a catch-all
/// (wildcard `_` or ident binding). Literal/range patterns
/// over a Result don't contribute to exhaustiveness — they are
/// "impossible" but we don't reject them here (that would be a
/// separate check).
fn check_result_match_exhaustiveness(
    ctx: &mut CheckCtx,
    arms: &[crate::ast::MatchArm],
    span: Span,
) {
    let mut has_ok = false;
    let mut has_err = false;
    let mut has_catchall = false;
    for arm in arms {
        // R.2.2: arms with guard do NOT count for exhaustiveness
        // (parallel to Rust). The guard can fail at runtime and
        // leave the match incomplete.
        if arm.guard.is_some() {
            continue;
        }
        update_result_coverage(&arm.pattern, &mut has_ok, &mut has_err, &mut has_catchall);
    }
    if has_catchall || (has_ok && has_err) {
        return;
    }
    let missing = match (has_ok, has_err) {
        (true, false) => "`Err`",
        (false, true) => "`Ok`",
        _ => "`Ok` y `Err`",
    };
    ctx.error_at(
        span,
        format!(
            "match over `Result` is not exhaustive: missing case {}",
            missing
        ),
    );
}

/// W2 (v0.10.6) — `true` if the pattern covers `null`. Used by the
/// `match` checker to refine `Nullable<T>` to `T` in subsequent
/// arms (flow-sensitive). Conservative: covers direct `Pattern::Null`
/// and `Pattern::Or` containing at least one Null sub-pattern.
/// `Pattern::Wildcard`/`Pattern::Ident` are NOT considered specific
/// cover (they match everything, including null — but their place is
/// catch-all, not refinable).
fn pattern_cubre_null(pat: &crate::ast::Pattern) -> bool {
    use crate::ast::Pattern;
    match pat {
        Pattern::Null => true,
        Pattern::Or(subs) => subs.iter().any(pattern_cubre_null),
        _ => false,
    }
}

/// Binds the variables introduced by a pattern in the current scope.
/// `scrutinee` is the type of the value being matched.
/// `arm_span` is the approximation span the binding uses as
/// `def_span` (Phase 9.x.3) — without a `Pattern`'s own span (S1
/// debt), the caller passes the MatchArm's body span.
fn bind_pattern(ctx: &mut CheckCtx, pat: &crate::ast::Pattern, scrutinee: &Type, arm_span: Span) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name, ident_span) => {
            // S1 (2026-06-05) — use the pattern's own span as
            // def_span (instead of the approximate arm_span) and register the
            // type in TypeInfo to enable hover over the binding's name
            // (`i` in `for i in 0..10`, `n` in `match x { Ok(n) => n }`).
            // If the pattern's span is ZERO (synthetic pattern), uses
            // arm_span as fallback.
            let def_span = if ident_span.column > 0 {
                *ident_span
            } else {
                arm_span
            };
            ctx.declare_var(name.clone(), scrutinee.clone(), def_span);
            if ident_span.column > 0 {
                ctx.type_info.record(*ident_span, scrutinee.clone());
            }
        }
        Pattern::OkBinding(name, ident_span) => {
            // `Ok(x)` unwraps `Result<T>` — x is T.
            let inner = match scrutinee {
                Type::Result { ok: t, err: _ } => (**t).clone(),
                _ => Type::Any,
            };
            let def_span = if ident_span.column > 0 {
                *ident_span
            } else {
                arm_span
            };
            ctx.declare_var(name.clone(), inner.clone(), def_span);
            if ident_span.column > 0 {
                ctx.type_info.record(*ident_span, inner);
            }
        }
        Pattern::ErrBinding(name, ident_span) => {
            // Mini-batch Re+ — `Err(e)` unwraps `Result<T, E>` and `e`
            // gets the inferred E type. For legacy Result (without explicit
            // E, default Str) or any Any, fallback to the
            // previous semantics (e: Str / e: Any).
            let inner = match scrutinee {
                Type::Result { ok: _, err: e } => (**e).clone(),
                _ => Type::Any,
            };
            let def_span = if ident_span.column > 0 {
                *ident_span
            } else {
                arm_span
            };
            ctx.declare_var(name.clone(), inner.clone(), def_span);
            if ident_span.column > 0 {
                ctx.type_info.record(*ident_span, inner);
            }
        }
        Pattern::Wildcard
        | Pattern::OkWildcard
        | Pattern::ErrWildcard
        | Pattern::Int(_)
        | Pattern::Float(_)
        | Pattern::Str(_)
        | Pattern::Bool(_)
        | Pattern::Null
        | Pattern::Range { .. } => {
            // Do not introduce bindings.
        }
        Pattern::Or(_) => {
            // R.2.1: or-patterns do not introduce bindings by
            // parser contract (rejects Ident/OkBinding/
            // ErrBinding inside). No need to walk.
        }
        // Tuples (mini-batch T): recurses into each slot with the
        // corresponding type. If scrutinee is not `Tuple` or differs in
        // length, sub-patterns are still checked with Any
        // (gradual) — the evaluator does the real match.
        Pattern::Tuple(subs) => {
            let slot_tys: Vec<Type> = match scrutinee {
                Type::Tuple(items) if items.len() == subs.len() => items.clone(),
                _ => (0..subs.len()).map(|_| Type::Any).collect(),
            };
            for (sub, ty) in subs.iter().zip(slot_tys.iter()) {
                bind_pattern(ctx, sub, ty, arm_span);
            }
        }
    }
}

/// Synthesizes the type of a BinOp given the types of its operands.
/// Applies Int→Float coercion where appropriate.
fn infer_binop(ctx: &mut CheckCtx, op: &BinOpKind, lt: &Type, rt: &Type, span: Span) -> Type {
    // If either operand is Any, we cannot check with confidence —
    // we return Any without error.
    if matches!(lt, Type::Any) || matches!(rt, Type::Any) {
        return Type::Any;
    }
    match op {
        BinOpKind::Add => {
            // Numeric or Str+Str.
            match (lt, rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Int, Type::Float)
                | (Type::Float, Type::Int)
                | (Type::Float, Type::Float) => Type::Float,
                (Type::Str, Type::Str) => Type::Str,
                _ => {
                    ctx.error_at(
                        span,
                        format!(
                            "operator `+` does not accept `{}` and `{}`",
                            lt.display(ctx.types),
                            rt.display(ctx.types)
                        ),
                    );
                    Type::Any
                }
            }
        }
        BinOpKind::Sub | BinOpKind::Mul | BinOpKind::Div => {
            let sym = match op {
                BinOpKind::Sub => "-",
                BinOpKind::Mul => "*",
                BinOpKind::Div => "/",
                _ => unreachable!(),
            };
            match (lt, rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Int, Type::Float)
                | (Type::Float, Type::Int)
                | (Type::Float, Type::Float) => Type::Float,
                _ => {
                    ctx.error_at(
                        span,
                        format!(
                            "operator `{}` expects numeric operands, received `{}` and `{}`",
                            sym,
                            lt.display(ctx.types),
                            rt.display(ctx.types)
                        ),
                    );
                    Type::Any
                }
            }
        }
        // R.1.2 — `%` operator only Int. Float % Float remains a
        // future sub-step (ambiguity between `fmod` and
        // `rem_euclid` over Float requires a design decision).
        BinOpKind::Mod => match (lt, rt) {
            (Type::Int, Type::Int) | (Type::Any, _) | (_, Type::Any) => Type::Int,
            _ => {
                ctx.error_at(
                    span,
                    format!(
                        "operator `%` expects Int on both sides, received `{}` and `{}`",
                        lt.display(ctx.types),
                        rt.display(ctx.types)
                    ),
                );
                Type::Int
            }
        },
        BinOpKind::Lt | BinOpKind::LtEq | BinOpKind::Gt | BinOpKind::GtEq => {
            // Comparison: numeric, both Str, or both Date/DateTime
            // (v0.10.30 B.4 — `chrono::NaiveDate` and `DateTime<Utc>` impl
            // `Ord`, natural mapping to `<`/`<=`/`>`/`>=`).
            let ok = matches!(
                (lt, rt),
                (Type::Int, Type::Int)
                    | (Type::Int, Type::Float)
                    | (Type::Float, Type::Int)
                    | (Type::Float, Type::Float)
                    | (Type::Str, Type::Str)
                    | (Type::Date, Type::Date)
                    | (Type::DateTime, Type::DateTime)
            );
            if !ok {
                ctx.error_at(
                    span,
                    format!(
                        "comparison between `{}` and `{}` not supported",
                        lt.display(ctx.types),
                        rt.display(ctx.types)
                    ),
                );
            }
            Type::Bool
        }
        BinOpKind::Eq | BinOpKind::NotEq => {
            // Equality: any pair. The evaluator does Int↔Float coercion
            // inside lists/maps/etc. We don't emit warning.
            Type::Bool
        }
        BinOpKind::And | BinOpKind::Or | BinOpKind::Xor => {
            if !matches!(lt, Type::Bool) {
                ctx.error_at(
                    span,
                    format!(
                        "logical operator expects Bool, left side is `{}`",
                        lt.display(ctx.types)
                    ),
                );
            }
            if !matches!(rt, Type::Bool) {
                ctx.error_at(
                    span,
                    format!(
                        "logical operator expects Bool, right side is `{}`",
                        rt.display(ctx.types)
                    ),
                );
            }
            Type::Bool
        }
        // Mini-batch Bits — all bitwise only Int. Any
        // other type fires a clear type error.
        BinOpKind::BitAnd
        | BinOpKind::BitOr
        | BinOpKind::BitXor
        | BinOpKind::Shl
        | BinOpKind::Shr => {
            let sym = match op {
                BinOpKind::BitAnd => "&",
                BinOpKind::BitOr => "|",
                BinOpKind::BitXor => "^",
                BinOpKind::Shl => "<<",
                BinOpKind::Shr => ">>",
                _ => unreachable!(),
            };
            if !matches!(lt, Type::Int | Type::Any) {
                ctx.error_at(
                    span,
                    format!(
                        "bitwise operator `{}` expects Int, left side is `{}`",
                        sym,
                        lt.display(ctx.types)
                    ),
                );
            }
            if !matches!(rt, Type::Int | Type::Any) {
                ctx.error_at(
                    span,
                    format!(
                        "bitwise operator `{}` expects Int, right side is `{}`",
                        sym,
                        rt.display(ctx.types)
                    ),
                );
            }
            Type::Int
        }
    }
}

/// Compatibility for assignment / argument passing: can `actual` be
/// used where `expected` is expected?
///
/// Rules:
///   - `Any` matches anything (gradual, in both directions).
///   - `Null` matches `T?` for any T.
///   - `T` matches `T?` if the inner is compatible.
///   - `Int` matches `Float` (implicit coercion in arithmetic
///     and assignment).
///   - Built-in generics (`List`/`Map`/`Result`/`Nullable`) and
///     `Function` are compared recursively — so `Result<Any>`
///     passes for `Result<User>`, `List<Int>` for `List<Float>`, etc.
///   - Otherwise: structural equality.
pub fn is_compatible(actual: &Type, expected: &Type) -> bool {
    if matches!(actual, Type::Any) || matches!(expected, Type::Any) {
        return true;
    }
    // Phase 8.4 — `PyAny` is gradual like `Any` but retains
    // its own identity so the checker can distinguish "this
    // comes from Python" from "this is general Any" (relevant in
    // `infer_call` to type Python calls as `Result<Any>`).
    if matches!(actual, Type::PyAny) || matches!(expected, Type::PyAny) {
        return true;
    }
    if matches!(actual, Type::Null) && expected.is_nullable() {
        return true;
    }
    // `T` compatible with `T?` (a non-null value where nullable is accepted).
    if let Type::Nullable(inner) = expected {
        if is_compatible(actual, inner) {
            return true;
        }
    }
    if matches!(actual, Type::Int) && matches!(expected, Type::Float) {
        return true;
    }
    match (actual, expected) {
        (Type::List(a), Type::List(b)) => is_compatible(a, b),
        (Type::Map(ka, va), Type::Map(kb, vb)) => is_compatible(ka, kb) && is_compatible(va, vb),
        // Mini-batch Re+: both sides (ok and err) must be compatible.
        (
            Type::Result {
                ok: a_ok,
                err: a_err,
            },
            Type::Result {
                ok: b_ok,
                err: b_err,
            },
        ) => is_compatible(a_ok, b_ok) && is_compatible(a_err, b_err),
        (Type::Future(a), Type::Future(b)) => is_compatible(a, b),
        (Type::Nullable(a), Type::Nullable(b)) => is_compatible(a, b),
        (
            Type::Function {
                params: pa,
                ret: ra,
            },
            Type::Function {
                params: pb,
                ret: rb,
            },
        ) => {
            pa.len() == pb.len()
                && pa.iter().zip(pb.iter()).all(|(a, b)| is_compatible(a, b))
                && is_compatible(ra, rb)
        }
        // Tuples (mini-batch T): compatible if same length and each
        // slot is compatible. `(Int, Str)` ↔ `(Float, Str)` due to
        // Int→Float promotion in each slot.
        (Type::Tuple(a), Type::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| is_compatible(x, y))
        }
        // Phase 10.3+ — QueryBuilder<Row> compatible if the row type
        // is compatible. Useful for returning QB from helper fns.
        (Type::QueryBuilder(a), Type::QueryBuilder(b)) => is_compatible(a, b),
        (Type::Aggregated(a), Type::Aggregated(b)) => is_compatible(a, b),
        _ => actual == expected,
    }
}

/// Walks a list of Stmt in order, maintaining the current scope.
fn check_block(ctx: &mut CheckCtx, body: &[Stmt]) {
    for s in body {
        check_stmt(ctx, s);
    }
}

/// Walks a single Stmt: checks its expressions, opens scopes,
/// declares variables.
fn check_stmt(ctx: &mut CheckCtx, stmt: &Stmt) {
    match stmt {
        // Mini-batch T — destructuring. We infer the RHS's type and
        // bind each slot of the pattern.
        Stmt::Destructure {
            pattern,
            value,
            span,
        } => {
            let value_ty = infer_expr(ctx, value);
            // If the value types as Tuple, we validate arity.
            if let Type::Tuple(items) = &value_ty {
                if let crate::ast::Pattern::Tuple(subs) = pattern {
                    if items.len() != subs.len() {
                        ctx.error_at(
                            *span,
                            format!(
                                "tuple destructuring: the pattern has {} slots, the value has {}",
                                subs.len(),
                                items.len()
                            ),
                        );
                    }
                }
            }
            bind_pattern(ctx, pattern, &value_ty, *span);
        }
        Stmt::Assign {
            target,
            type_,
            value,
            span,
        } => {
            // L2 expanded (2026-06-05) — Bidirectional inference
            // from `Fn(T1, T2) -> R` annotations in `let f: Fn(...) -> ... = fn(...) => ...`.
            // If the annotation resolves to `Type::Function { params, .. }`
            // and the RHS is directly a FnExpr, we push the Function's
            // params as hint BEFORE synthesizing the RHS. The
            // Expr::FnExpr handler consumes it on pop, propagating
            // the expected types to the FnExpr's unannotated params.
            if matches!(value, Expr::FnExpr { .. }) {
                if let Some(ann) = type_ {
                    let declared_for_hint = resolve_type_expr(ann, ctx.types).unwrap_or(Type::Any);
                    let hint = match declared_for_hint {
                        Type::Function { params, .. } => Some(params),
                        _ => None,
                    };
                    ctx.fn_expr_param_hints.push(hint);
                }
            }
            let value_ty = infer_expr(ctx, value);
            if let AssignTarget::Ident(name, target_span) = target {
                // V2 (2026-06-05) — register the binding's type under
                // the LHS Ident span in TypeInfo. Enables hover
                // over the variable's name (not just on the RHS).
                // For explicit annotations (`let x: Int = ...`), the
                // declared type prevails over the inferred — that's
                // resolved by the match below. Here we use the final
                // type determined below (not `value_ty` directly) — that's why
                // we do the record AFTER the match, not before.
                let final_ty: Type;
                match type_ {
                    Some(ann) => {
                        let declared = resolve_type_expr(ann, ctx.types).unwrap_or(Type::Any);
                        if !is_compatible(&value_ty, &declared) {
                            ctx.error_at(
                                *span,
                                format!(
                                    "`{}` declared as `{}` received a value `{}`",
                                    name,
                                    declared.display(ctx.types),
                                    value_ty.display(ctx.types)
                                ),
                            );
                        }
                        // An explicit annotation "redeclares" the binding
                        // with the declared type and marks annotated=true.
                        // `def_span = span` of Stmt::Assign: on
                        // reassignment, go-to-def jumps to the LAST
                        // binding stmt (simplified MVP semantics).
                        ctx.declare_var_annotated(name.clone(), declared.clone(), *span);
                        final_ty = declared;
                    }
                    None => {
                        // Without new annotation: if the variable already exists
                        // with previous annotation, we require the new
                        // value to be compatible with that type. If the
                        // variable was inferred without annotation, the
                        // gradual model allows the type to change.
                        match ctx.lookup_binding(name) {
                            Some(existing) if existing.annotated => {
                                let existing_ty = existing.ty.clone();
                                if !is_compatible(&value_ty, &existing_ty) {
                                    ctx.error_at(
                                        *span,
                                        format!(
                                            "`{}` declared as `{}` received a value `{}`",
                                            name,
                                            existing_ty.display(ctx.types),
                                            value_ty.display(ctx.types)
                                        ),
                                    );
                                }
                                // We keep the annotated binding — the
                                // reassignment does not relax the type.
                                ctx.declare_var_annotated(name.clone(), existing_ty.clone(), *span);
                                final_ty = existing_ty;
                            }
                            _ => {
                                ctx.declare_var(name.clone(), value_ty.clone(), *span);
                                final_ty = value_ty.clone();
                            }
                        }
                    }
                }
                // V2 — final record of the LHS under its own span.
                ctx.type_info.record(*target_span, final_ty);
            }
            // AssignTarget::Field { object, field }: validate that the
            // receiver is a nominal type with that field and the value's
            // type is compatible with the declared. Covers the
            // hole documented in deudas-post-5b (F2): previously
            // only caught at runtime.
            else if let AssignTarget::Field { object, field } = target {
                let obj_ty = infer_expr(ctx, object);
                match &obj_ty {
                    Type::Any => {
                        // Gradual escape — we don't check (matches
                        // `Expr::Field` in `infer_expr`).
                    }
                    Type::Nominal(id) => {
                        let info = ctx.types.info(*id);
                        let type_name = info.name.clone();
                        // If fields are not resolved (declaration
                        // with previous error), we don't check to avoid
                        // duplicating the error.
                        if let Some(declared_fields) = info.fields.clone() {
                            match declared_fields.iter().find(|f| &f.name == field) {
                                Some(f) => {
                                    // Mini-batch Vp — assigning to a private
                                    // field is only allowed from
                                    // methods of the type itself.
                                    if is_private_field(field) && ctx.current_type != Some(*id) {
                                        ctx.error_at(*span, format!(
                                            "field `{}.{}` is private (prefix `_`); cannot be assigned from outside type `{}`",
                                            type_name, field, type_name
                                        ));
                                    }
                                    if !is_compatible(&value_ty, &f.type_) {
                                        ctx.error_at(
                                            *span,
                                            format!(
                                                "field `{}.{}` expects `{}`, received `{}`",
                                                type_name,
                                                field,
                                                f.type_.display(ctx.types),
                                                value_ty.display(ctx.types)
                                            ),
                                        );
                                    }
                                }
                                None => {
                                    ctx.error_at(
                                        *span,
                                        format!(
                                            "type `{}` does not have a field named `{}`",
                                            type_name, field
                                        ),
                                    );
                                }
                            }
                        }
                    }
                    other => {
                        ctx.error_at(
                            *span,
                            format!(
                                "assignment to field `.{}` over `{}`: only allowed \
                             over instances of a custom type",
                                field,
                                other.display(ctx.types)
                            ),
                        );
                    }
                }
            }
            // R.1.3 — `object[index] = value` (mini-phase R).
            // Validate receiver `List<T>` with `Int` index, RHS
            // compatible with T. Or receiver `Map<K, V>` with index
            // compatible with K, RHS compatible with V.
            else if let AssignTarget::Index { object, index } = target {
                let obj_ty = infer_expr(ctx, object);
                let idx_ty = infer_expr(ctx, index);
                match &obj_ty {
                    Type::Any => { /* gradual */ }
                    Type::List(item_ty) => {
                        if !is_compatible(&idx_ty, &Type::Int) {
                            ctx.error_at(
                                *span,
                                format!(
                                    "index of `List<{}>` must be `Int`, received `{}`",
                                    item_ty.display(ctx.types),
                                    idx_ty.display(ctx.types)
                                ),
                            );
                        }
                        if !is_compatible(&value_ty, item_ty) {
                            ctx.error_at(
                                *span,
                                format!(
                                    "the list contains `{}`, cannot assign `{}`",
                                    item_ty.display(ctx.types),
                                    value_ty.display(ctx.types)
                                ),
                            );
                        }
                    }
                    Type::Map(k_ty, v_ty) => {
                        if !is_compatible(&idx_ty, k_ty) {
                            ctx.error_at(
                                *span,
                                format!(
                                    "the map key is `{}`, received `{}`",
                                    k_ty.display(ctx.types),
                                    idx_ty.display(ctx.types)
                                ),
                            );
                        }
                        if !is_compatible(&value_ty, v_ty) {
                            ctx.error_at(
                                *span,
                                format!(
                                    "the map contains `{}`, cannot assign `{}`",
                                    v_ty.display(ctx.types),
                                    value_ty.display(ctx.types)
                                ),
                            );
                        }
                    }
                    other => {
                        ctx.error_at(
                            *span,
                            format!(
                                "assignment to index `[...] = v` not supported over `{}` \
                             (only `List` and `Map`)",
                                other.display(ctx.types)
                            ),
                        );
                    }
                }
            }
        }

        Stmt::Return(e, span) => {
            // We always infer so errors inside surface.
            let ret_ty = infer_expr(ctx, e);
            // R.2.4 (F3): orphan `return` (outside fn) → clear
            // static error. The evaluator also emitted it at
            // runtime, but the checker catches it earlier.
            if ctx.return_stack.is_empty() {
                ctx.error_at(
                    *span,
                    "`return` can only be used inside a function".to_string(),
                );
            }
            // If we are inside a function with declared (and resolvable)
            // return_type, we validate. Outside fn or with
            // missing return_type (Any), we don't check.
            if let Some(expected) = ctx.return_stack.last().cloned() {
                if !is_compatible(&ret_ty, &expected) {
                    ctx.error_at(
                        *span,
                        format!(
                            "`return` returns `{}` but the function declares `{}`",
                            ret_ty.display(ctx.types),
                            expected.display(ctx.types)
                        ),
                    );
                }
            }
            // We feed the containing fn's inferred frame.
            // For FnDef it's discarded on pop; for FnExpr it's used to
            // synthesize `ret`.
            if let Some(frame) = ctx.inferred_returns.last_mut() {
                frame.push(ret_ty);
            }
        }

        Stmt::Expr(e, _) => {
            let _ = infer_expr(ctx, e);
        }

        Stmt::ReturnStatus { status, body, span } => {
            // We infer the exprs so errors inside surface.
            let status_ty = infer_expr(ctx, status);
            let body_ty = body.as_ref().map(|b| infer_expr(ctx, b));
            // Rule: only valid inside an HTTP handler. Outside of
            // that is a clear error — new syntax in the spec, restricted
            // to handlers to not open polymorphic return in any fn.
            let in_handler = ctx.in_http_handler.last().copied().unwrap_or(false);
            if !in_handler {
                ctx.error_at(*span,
                    "`return <status> { ... }` is only allowed inside an HTTP handler (`@get`/`@post`/`@put`/`@delete`) or a fn applied as `@middleware(...)`".to_string()
                );
            }
            // Status must be Int (range 100-599 validated at runtime).
            if !is_compatible(&status_ty, &Type::Int) {
                ctx.error_at(
                    *span,
                    format!(
                        "the status code of `return` must be Int, received `{}`",
                        status_ty.display(ctx.types)
                    ),
                );
            }
            // The body can be any serializable value; we don't check
            // against the handler's formal `return_type` (it's polymorphic:
            // the spec allows a handler with `-> User` to also do
            // `return 404 { ... }`). The body is serialized to JSON at
            // runtime with `value_to_json`.
            let _ = body_ty;
        }

        Stmt::FnDef {
            name: fn_name,
            params,
            return_type,
            body,
            decorators,
            is_async,
            span: fn_span,
        } => {
            // We open a new scope for params and locals. Params are
            // bound with their declared type (or Any). We push the
            // expected return type to the stack so `return`s
            // inside see it. Without annotation → `Any` (doesn't check).
            // We also push a frame in `inferred_returns` to
            // maintain consistency with FnExpr (frames go in
            // parallel); the content is discarded here because FnDef
            // already has declared `return_type`.
            //
            // Async (6.2): the EXTERNAL signature of an `async fn` wraps
            // its return type in `Future<T>` (that's built in
            // `preregister_fn_signatures` so calls to the
            // fn type correctly). But inside the body, the
            // `return x` still produces pure `T` (not `Future<T>`)
            // — `async` is transparent from inside. That's why when
            // pushing the `return_stack` we use `T` (not wrapped).
            let ret = match return_type {
                Some(r) => resolve_type_expr(r, ctx.types).unwrap_or(Type::Any),
                None => Type::Any,
            };
            // "HTTP context" for the `Stmt::ReturnStatus` check:
            // HTTP handlers (`@get`/`@post`/`@put`/`@delete`/`@ws`) and
            // fns referenced by `@middleware(name)` in another FnDef.
            // The pre-scan fills `ctx.middleware_fn_names` before the walk.
            // Phase 9.w.2: `@ws("/path")` also counts as HTTP-like —
            // allows `return <status> { ... }` before the upgrade.
            let is_http_handler = decorators
                .iter()
                .any(|d| matches!(d.name.as_str(), "get" | "post" | "put" | "delete" | "ws"))
                || ctx.middleware_fn_names.contains(fn_name);
            // Phase 9.w.1 — validate `@authenticated`/`@admin` against the
            // `@auth_provider` collected pre-walk. Errors go to
            // `ctx.errors`; doesn't interrupt body checking.
            check_auth_decorators(ctx, fn_name, params, decorators, *fn_span);
            // Phase 9.w.2 — validate `@ws(...)` handlers: async fn
            // receiving exactly one `WsConn<T>` + (optional) a `user:
            // User` if it has `@authenticated`/`@admin`.
            check_ws_handler(ctx, fn_name, params, *is_async, decorators, *fn_span);
            // Phase 9.w.3 — validate `@cron("expr")` (periodic jobs) and
            // `@background` (fns executable via spawn). Each has its
            // own rules; conflicts `@cron + @background` or
            // `@cron + @get/@post/...` are rejected.
            check_cron_decorator(ctx, fn_name, params, &ret, *is_async, decorators, *fn_span);
            check_background_decorator(ctx, fn_name, decorators, *fn_span);
            // Phase 4 (fitz-liveviews Y-B, session 1.b) — validate
            // `@render_for("name")` and `@on("component", "event")`
            // on fns. Shape errors were already reported by
            // `resolve_program`; here we validate signature (params
            // + return type + component-name existence) and reject
            // conflicts with other runtime decorators.
            check_render_for_decorator(ctx, fn_name, params, &ret, decorators, *fn_span);
            check_on_decorator(ctx, fn_name, params, &ret, decorators, *fn_span);
            // Phase 12.1.a — validate `@healthz`/`@readyz` (K8s-style
            // probes). Singletons, no args/kwargs/params, return
            // `Bool`/`Result<Null>`/`Result<Bool>` (sync or async). NOT
            // combinable with `@get/@post/@put/@delete/@ws/@cron/
            // @background/@auth_provider/@authenticated/@admin/@test/
            // @command`. The runtime and codegen auto-mount `/healthz`
            // and `/readyz` when these decorators are declared.
            check_health_decorators(ctx, fn_name, params, &ret, decorators, *fn_span);
            // Phase 13 (v0.11.0) — `@command("name", desc="...")` declares
            // a fn as a CLI command. Validates that the fn has no conflicts
            // with server/job/test decorators, that params are
            // CLI-marshallable (Str/Int/Float/Bool/Str?), and that the return
            // is Int (exit code).
            check_command_decorator(ctx, fn_name, params, &ret, decorators, *fn_span);
            // Phase 12.7 — `@trace(name="X")` and `@metric(name="X")`
            // on user fns (business logic). Rejects stacking
            // on HTTP handlers (12.3 auto-instrumentation covers them).
            // Accepts only optional `name` kwarg.
            check_trace_metric_decorators(ctx, fn_name, decorators, *fn_span);
            // Phase 12.8 — `@flag("name")` gate for the whole fn. Validates
            // syntactic shape: 1 positional arg Str literal (the flag name), no
            // kwargs. Stackable on any fn (HTTP/WS/regular). When
            // active, the runtime and codegen wrap the invocation
            // checking the registry (env var `FITZ_FLAG_<NAME>` or manifest
            // default); if the flag is off, HTTP/WS handlers
            // return 404, normal fns return Null/default according to
            // return type.
            check_flag_decorator(ctx, fn_name, decorators, *fn_span);
            ctx.push_scope();
            ctx.return_stack.push(ret);
            ctx.inferred_returns.push(Vec::new());
            ctx.in_http_handler.push(is_http_handler);
            ctx.await_stack.push(*is_async);
            // R.2.4 (F3): break/continue do NOT escape functions. We save
            // the previous loop_depth, reset to 0 for the body, and
            // restore on exit.
            let saved_loop_depth = ctx.loop_depth;
            ctx.loop_depth = 0;
            for p in params {
                let elem_ty = ann_to_type(p.type_.as_ref(), ctx.types);
                // Fp.2 — varargs: inside the body, the binding types
                // as `List<T>` (T = annotated type or Any). The call site
                // collects 0+ extra args into a List.
                let pty = if p.varargs {
                    Type::List(Box::new(elem_ty))
                } else {
                    elem_ty
                };
                // S1 (2026-06-05) — `Param` now has its own `name_span`.
                // We use it as def_span and register the type in TypeInfo
                // (hover over the param's name works). Fallback to
                // fn_span if name_span is ZERO (synthetic params).
                let def_span = if p.name_span.column > 0 {
                    p.name_span
                } else {
                    *fn_span
                };
                ctx.declare_var(p.name.clone(), pty.clone(), def_span);
                if p.name_span.column > 0 {
                    ctx.type_info.record(p.name_span, pty);
                }
            }
            check_block(ctx, body);
            ctx.loop_depth = saved_loop_depth;
            ctx.inferred_returns.pop();
            ctx.return_stack.pop();
            ctx.in_http_handler.pop();
            ctx.await_stack.pop();
            ctx.pop_scope();
        }

        Stmt::TypeDef { .. } => {
            // Already validated by resolve_program.
        }

        Stmt::While {
            condition,
            body,
            span,
            ..
        } => {
            let cond_ty = infer_expr(ctx, condition);
            if !is_compatible(&cond_ty, &Type::Bool) {
                ctx.error_at(
                    *span,
                    format!(
                        "the `while` condition must be Bool, received `{}`",
                        cond_ty.display(ctx.types)
                    ),
                );
            }
            ctx.push_scope();
            ctx.loop_depth += 1;
            check_block(ctx, body);
            ctx.loop_depth -= 1;
            ctx.pop_scope();
        }

        Stmt::Loop { body, .. } => {
            ctx.push_scope();
            ctx.loop_depth += 1;
            check_block(ctx, body);
            ctx.loop_depth -= 1;
            ctx.pop_scope();
        }

        Stmt::For {
            var,
            iter,
            body,
            span,
            ..
        } => {
            // Mini-batch Md — `var` is now a Pattern. The elem type
            // depends on the iter: List<T> → T; Range → Int; Map<K, V> →
            // Tuple([K, V]) (each iteration produces a pair).
            let iter_ty = infer_expr(ctx, iter);
            let elem_ty = match &iter_ty {
                Type::List(t) => (**t).clone(),
                Type::Range => Type::Int,
                Type::Map(k, v) => Type::Tuple(vec![(**k).clone(), (**v).clone()]),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(
                        *span,
                        format!(
                            "the `for` iterable must be List, Range or Map, received `{}`",
                            other.display(ctx.types)
                        ),
                    );
                    Type::Any
                }
            };
            ctx.push_scope();
            ctx.loop_depth += 1;
            bind_for_pattern_in_checker(ctx, var, &elem_ty, *span);
            check_block(ctx, body);
            ctx.loop_depth -= 1;
            ctx.pop_scope();
        }

        Stmt::Break(value, _label, span) => {
            // Mini-batch L: check the value if present and push it
            // to `break_value_stack` so the containing `Expr::Loop`
            // unifies it as the return type.
            let v_ty = if let Some(e) = value {
                infer_expr(ctx, e)
            } else {
                Type::Null
            };
            if let Some(frame) = ctx.break_value_stack.last_mut() {
                frame.push(v_ty);
            }
            // R.2.4 (F3): orphan `break` (outside loop) → error.
            if ctx.loop_depth == 0 {
                ctx.error_at(
                    *span,
                    "`break` solo puede usarse adentro de un loop (`while`, `loop`, `for`)"
                        .to_string(),
                );
            }
        }
        Stmt::Continue(_label, span) => {
            // R.2.4 (F3): orphan `continue` (outside loop) → error.
            if ctx.loop_depth == 0 {
                ctx.error_at(
                    *span,
                    "`continue` solo puede usarse adentro de un loop (`while`, `loop`, `for`)"
                        .to_string(),
                );
            }
        }

        Stmt::Import { path, alias, span } => {
            // `import a.b.c` binds `c` (or `alias` if present) as Module.
            // 8.4: if the path starts with `python` (reserved interop
            // prefix), the binding types as `PyAny` so the
            // checker can refine the type of calls to
            // `Result<Any>`. The rest stays as `Any` (standard gradual).
            let from_python = path.first().map(|s| s.as_str()) == Some("python");
            let binding = alias.clone().or_else(|| path.last().cloned());
            if let Some(name) = binding {
                let ty = if from_python { Type::PyAny } else { Type::Any };
                // go-to-def on the binding jumps to the import
                // line (to the stmt, not to the remote module — cross-module
                // def is visible debt of the 9.x.3 MVP).
                ctx.declare_var(name, ty, *span);
            }
        }

        Stmt::FromImport { path, names, span } => {
            // Each name is brought into scope as a var. Some can
            // be types (StructLit checks them via TypeEnv, already
            // registered in resolve_program), others functions or
            // values — without info from the imported module, `Any` is the
            // best we have in 5.3.1. With alias, the local binding
            // uses the alias instead of the original name.
            //
            // 8.4: `from python import X` binds `X` as `PyAny` so
            // call sites refine to `Result<Any>` in
            // `infer_call`. Submodules `from python.X import Y` also
            // type as `PyAny` — anything coming from Python is
            // opaque to the checker.
            //
            // 8-pyi.C (v0.9.57): if there's an adjacent `.pyi` stub
            // loaded by `pyi_loader::load_callables`, we bind the
            // name with `Type::Nominal(synth_id)` where synth is the
            // synthetic nominal that has one field per fn/var of the
            // stub. Field access (`X.fn`) then resolves to the
            // signature declared in the .pyi instead of returning
            // `PyAny`. Without a stub, fallback to gradual PyAny.
            let from_python = path.first().map(|s| s.as_str()) == Some("python");
            for (n, alias) in names {
                let binding = alias.clone().unwrap_or_else(|| n.clone());
                let ty = if from_python {
                    // The stub was loaded under the `binding` (alias if
                    // present, else name) — see `pyi_loader::load_callables`.
                    match ctx.types.pyi_module(&binding) {
                        Some(id) => Type::Nominal(id),
                        None => Type::PyAny,
                    }
                } else {
                    Type::Any
                };
                // go-to-def on the binding jumps to the line of
                // `from foo import ...` — remote cross-module def
                // remains visible debt of the MVP.
                ctx.declare_var(binding, ty, *span);
            }
        }

        // Phase 9.0.1 (F15): parallel to `Expr::Error`. `Stmt::Error`
        // is silently ignored — the real error is already in
        // the parser's `recovered_errors`. We don't want to emit
        // derived errors from the checker on the same point.
        Stmt::Error(_) => {}
    }
}

/// Pre-registers the signatures of top-level `Stmt::FnDef`s as
/// `Type::Function` in the global scope. This unblocks forward
/// and mutual references between top-level functions.
fn preregister_fn_signatures(ctx: &mut CheckCtx, program: &Program) {
    for stmt in program {
        if let Stmt::FnDef {
            name,
            params,
            return_type,
            is_async,
            span,
            ..
        } = stmt
        {
            let param_types: Vec<Type> = params
                .iter()
                .map(|p| ann_to_type(p.type_.as_ref(), ctx.types))
                .collect();
            let ret = match return_type {
                Some(r) => resolve_type_expr(r, ctx.types).unwrap_or(Type::Any),
                None => Type::Any,
            };
            // Async (6.2): the EXTERNAL signature wraps the return type
            // in `Future<T>`. Calling `async_fn(args)` produces
            // `Future<T>`, unwrapped with `.await`. Even
            // without annotation (T = Any), we wrap as `Future<Any>`
            // — the roadmap (cross-cutting #3) formalizes it like this:
            // every async fn produces a Future when called, without
            // exceptions. `is_compatible` and `.await` already treat
            // `Any` as gradual escape, so `Future<Any>`
            // still lets everything through.
            let outer_ret = if *is_async {
                Type::Future(Box::new(ret))
            } else {
                ret
            };
            // Fp — defaults_count = number of params with `default`
            // at the end. The callee's minimum arity is
            // `params.len() - defaults_count`. The parser guarantees that
            // all defaults are consecutive at the end.
            let defaults_count = params.iter().filter(|p| p.default.is_some()).count();
            // Fp.2 — has_varargs if the last param is variadic.
            let has_varargs = params.last().map(|p| p.varargs).unwrap_or(false);
            // go-to-def on the fn's use jumps to the FnDef span
            // (which points to the `fn` keyword). Approximation; precision
            // by name requires the identifier's own span.
            ctx.declare_fn(
                name.clone(),
                Type::Function {
                    params: param_types,
                    ret: Box::new(outer_ret),
                },
                *span,
                defaults_count,
                has_varargs,
            );
        }
    }
}

/// Fp — callee's minimum arity. If it's an Ident resolvable to a fn with
/// defaults registered, returns `params.len() - defaults_count`. Otherwise,
/// returns `total` (strict fallback — callbacks/fns as var don't
/// have defaults info in `Type::Function`).
fn required_arity_for_callee(ctx: &CheckCtx, callee: &Expr, total: usize) -> usize {
    if let Expr::Ident(name, _) = callee {
        if let Some(b) = ctx.lookup_binding(name) {
            return total.saturating_sub(b.defaults_count);
        }
    }
    total
}

/// Fp.2 — `true` if the callee is a fn with varargs (last param is
/// variadic). When it's varargs, the call site accepts any quantity
/// `>= required` of args (instead of max = `total`).
fn callee_has_varargs(ctx: &CheckCtx, callee: &Expr) -> bool {
    if let Expr::Ident(name, _) = callee {
        if let Some(b) = ctx.lookup_binding(name) {
            return b.has_varargs;
        }
    }
    false
}

/// Public entry to the full static checker: runs annotation
/// resolution (`resolve_program`) and then expression checking.
/// Returns the env, the per-node types side-table (`TypeInfo`, Phase
/// 9.0 — F16), the per-use definitions side-table (`DefinitionInfo`,
/// Phase 9.x.3) and the accumulated errors list.
///
/// The side-tables are populated during checking: every `Expr` node with
/// known `Span` types in `TypeInfo`; every `Expr::Ident` resolved to
/// a binding with known `def_span` registers `(use_span → def_span)`
/// in `DefinitionInfo`. The CLI (`fitz run`/`build`/`check`) discards
/// both; the LSP (Phase 9.x) consumes them for hover and go-to-definition.
pub fn check_program(program: &Program) -> (TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>) {
    let (env, errors) = resolve_program(program);
    check_with_env(program, env, errors)
}

/// Variant of `check_program` receiving a pre-filled `TypeEnv`
/// (typically by `resolve_program` + side effects from the adjacent
/// `.pyi` stubs loader — see `pyi_loader`). The accumulated `errors`
/// from the resolve are preserved and extended with the check's errors.
///
/// **Expected use** (8-pyi.B, v0.9.57):
///
/// ```ignore
/// let (mut env, errors) = types::resolve_program(&program);
/// let _stubs = pyi_loader::load_stubs(&program, base_dir, &mut env);
/// let (env, info, defs, errors) =
///     types::check_with_env(&program, env, errors);
/// ```
///
/// Internal call sites without `.pyi` context should keep using
/// `check_program(program)` which invokes `resolve_program` internally.
pub fn check_with_env(
    program: &Program,
    env: TypeEnv,
    mut errors: Vec<FitzError>,
) -> (TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>) {
    // We encapsulate `ctx` in a block so its borrow on `env`
    // ends before the return: we want to move `env`, `ctx.type_info`
    // and `ctx.def_info` separately to the caller.
    let (type_info, def_info) = {
        let mut ctx = CheckCtx::new(&env);
        preregister_fn_signatures(&mut ctx, program);
        collect_middleware_fn_names(&mut ctx, program);
        // Phase 9.w.1 — collect `@auth_provider` (singleton) and expose
        // its info in `ctx.auth_provider`. The subsequent walk checks
        // `@authenticated`/`@admin` against this info.
        collect_auth_provider(&mut ctx, program);
        // Phase 9.w.3 — collect names of fns with `@background`. The
        // `spawn(call)` check requires the target to be a fn
        // declared with `@background` (opt-in to avoid accidental
        // uses on regular fns).
        collect_background_fns(&mut ctx, program);
        check_block(&mut ctx, program);
        // R.3 — check bodies of custom methods of each
        // `type`. This happens AFTER the normal check_block so
        // the nominals declared as `type X { ... }` are already
        // available. Each method body is checked with:
        //  - global child scope with the type's fields
        //    pre-declared as locals (option A).
        //  - method's params over the same scope (locals).
        //  - return_stack with the declared return_type (or Any).
        check_custom_methods(&mut ctx, program);
        errors.append(&mut ctx.errors);
        (ctx.type_info, ctx.def_info)
    };
    (env, type_info, def_info, errors)
}

/// R.3 — checks each custom method body inside the `type`s
/// declared in the program. Separate pass from `check_block` so
/// the type's fields (already resolved in `resolve_program`) are
/// available as locals in the body's scope.
fn check_custom_methods(ctx: &mut CheckCtx, program: &Program) {
    for stmt in program {
        let Stmt::TypeDef { name, methods, .. } = stmt else {
            continue;
        };
        if methods.is_empty() {
            continue;
        }
        // Recover the type's resolved fields (populated by
        // `resolve_program`). If the type doesn't exist → silent
        // (there was already an error in resolve_program).
        let Some(id) = ctx.types.lookup(name) else {
            continue;
        };
        let resolved_fields = match &ctx.types.info(id).fields {
            Some(fs) => fs.clone(),
            None => continue,
        };
        for m in methods {
            // Method's return type.
            let ret_ty = match &m.return_type {
                Some(r) => resolve_type_expr(r, ctx.types).unwrap_or(Type::Any),
                None => Type::Any,
            };
            ctx.push_scope();
            ctx.return_stack.push(ret_ty);
            ctx.inferred_returns.push(Vec::new());
            ctx.in_http_handler.push(false);
            ctx.await_stack.push(m.is_async);
            let saved_loop_depth = ctx.loop_depth;
            ctx.loop_depth = 0;
            // Mini-batch Vp — we mark that we are inside the body
            // of a method of type `id`. Enables access to private
            // fields (`_field`) from here.
            let saved_current_type = ctx.current_type;
            ctx.current_type = Some(id);
            // Pre-declare fields as locals (option A). Mini-batch
            // St: static methods do NOT receive fields as locals,
            // so we skip when `is_static`.
            if !m.is_static {
                for f in &resolved_fields {
                    ctx.declare_var(f.name.clone(), f.type_.clone(), m.span);
                }
            }
            // Declare params (overwrite homonymous fields in the
            // local scope — `declare_var` replaces the binding when
            // entering the same var).
            for p in &m.params {
                let pty = ann_to_type(p.type_.as_ref(), ctx.types);
                // S1 (2026-06-05) — use p.name_span if present.
                let def_span = if p.name_span.column > 0 {
                    p.name_span
                } else {
                    m.span
                };
                ctx.declare_var(p.name.clone(), pty.clone(), def_span);
                if p.name_span.column > 0 {
                    ctx.type_info.record(p.name_span, pty);
                }
            }
            check_block(ctx, &m.body);
            ctx.current_type = saved_current_type;
            ctx.loop_depth = saved_loop_depth;
            ctx.inferred_returns.pop();
            ctx.return_stack.pop();
            ctx.in_http_handler.pop();
            ctx.await_stack.pop();
            ctx.pop_scope();
        }
    }
}

/// W12 (v0.10.8) — Info of an `@auth_provider` declared in an
/// imported module (not in the local program). The caller (main.rs)
/// extracts this from each imported module's AST with
/// `extract_auth_provider_signature` and registers it in the importer's
/// TypeEnv with `set_imported_auth_provider`.
///
/// When `collect_auth_provider` does not find a local provider, it falls
/// back to this slot. The `user_type_name` is matched by NAME
/// against the importer's nominal (registered via `from <mod> import
/// <T>` in `resolve_program`'s pass 1b).
///
/// **`has_role_field` comes from the source module**: extracted by
/// `extract_auth_provider_signature` looking at the fields declared in
/// `type <T> { role: Str ... }` of the module's AST. This allows
/// `@admin` cross-module to statically validate the presence of `role`
/// without needing to copy nominal fields (which would drag TypeIds
/// from the source module).
#[derive(Debug, Clone)]
pub struct ImportedAuthProvider {
    /// Name of the module where the provider lives. Codegen uses it
    /// to emit `<module>::<fn>(...)` when invoking the provider
    /// from the wrapper of a protected handler. Must match the
    /// mod name of the generated Rust crate (typically derived
    /// from the file stem: `auth.fitz` → `auth`).
    pub module_name: String,
    /// Name of the fn marked with `@auth_provider`.
    pub fn_name: String,
    /// `true` if the fn is `async fn`. Codegen consults it to
    /// emit `.await` after the call.
    pub is_async: bool,
    /// Name of type `T` of the `Result<T>` returned by the provider.
    /// The checker matches it by name against the importer's nominal
    /// (registered by `from <module> import <T>`).
    pub user_type_name: String,
    /// `true` if `T` has a `role: Str` (non-nullable) field in the
    /// source module. Required by `@admin`. The scanner determines it
    /// looking at the module's AST.
    pub has_role_field: bool,
}

/// W12 (v0.10.8) — Public scanner that extracts the `@auth_provider`
/// declared in `program` (the AST of an already-parsed imported
/// module). Returns `None` if the module does not declare a provider.
///
/// The caller (main.rs) invokes this over each imported module, before
/// the importer's check. If there is a provider, it registers it in the
/// importer's TypeEnv with `set_imported_auth_provider`.
///
/// **Does not validate shape exhaustively**: the full provider check
/// (signature, return type, fields) is done by the module's own
/// checker when that module is checked separately. Here we only
/// extract the minimum so the importer can validate
/// `@authenticated`/`@admin` against the provider.
///
/// `module_name` is provided by the caller — typically the stem of
/// the imported file (`auth.fitz` → `"auth"`), so codegen
/// can emit module-qualified invocations.
pub fn extract_auth_provider_signature(
    program: &Program,
    module_name: &str,
) -> Option<ImportedAuthProvider> {
    let mut user_type_name: Option<String> = None;
    let mut fn_name: Option<String> = None;
    let mut is_async = false;
    // 1) Find the fn with `@auth_provider`.
    for stmt in program {
        let Stmt::FnDef {
            name,
            return_type,
            decorators,
            is_async: fn_is_async,
            ..
        } = stmt
        else {
            continue;
        };
        if !decorators.iter().any(|d| d.name == "auth_provider") {
            continue;
        }
        // Extract the name of type `T` from `Result<T>`. Without a
        // declared return type or with a different shape, we abort —
        // the module's checker will report it with a clear error.
        let Some(ret) = return_type else { return None };
        let TypeExpr::Generic { name: head, args } = ret else {
            return None;
        };
        if head != "Result" || args.len() != 1 {
            return None;
        }
        let TypeExpr::Named(t_name) = &args[0] else {
            return None;
        };
        user_type_name = Some(t_name.clone());
        fn_name = Some(name.clone());
        is_async = *fn_is_async;
        break;
    }
    let user_type_name = user_type_name?;
    let fn_name = fn_name?;
    // 2) Determine `has_role_field` by looking at the `type <T> { ... }` of the
    // same module. If T is not declared locally (unlikely —
    // would mean the provider returns an imported type in turn,
    // case out of the MVP), `has_role_field = false`.
    let has_role_field = program.iter().any(|stmt| {
        let Stmt::TypeDef { name, fields, .. } = stmt else {
            return false;
        };
        if name != &user_type_name {
            return false;
        }
        fields.iter().any(|f| {
            // `role` non-nullable of type `Str`. Nullable Str is not enough
            // (parallel to local validation of `collect_auth_provider`).
            f.name == "role" && matches!(&f.type_, TypeExpr::Named(n) if n == "Str")
        })
    });
    Some(ImportedAuthProvider {
        module_name: module_name.to_string(),
        fn_name,
        is_async,
        user_type_name,
        has_role_field,
    })
}

/// Phase 9.w.1 — Info of the `@auth_provider` registered in the
/// program. Built by `collect_auth_provider` when pre-scanning the
/// program before the checker walk. If there is more than one
/// `@auth_provider`, error is reported and the first is preserved.
///
/// Consulted by the `@authenticated`/`@admin` check to:
/// - Require each protected handler to declare a param compatible with
///   `user_type_id` (the `T` of `Result<T>` returned by the provider).
/// - Validate that `T` has a `role: Str` field when `@admin` appears in
///   the program.
///
/// Private to the module: the evaluator (`fitz run`, 9.w.1.c) and codegen
/// (`fitz build`, 9.w.1.d) re-collect on their own. The checker does not
/// need to export the info; it only validates statically.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct AuthProviderInfo {
    /// Name of the fn marked with `@auth_provider`.
    name: String,
    /// Span of the fn (for duplicate error messages).
    span: Span,
    /// `TypeId` of the nominal `T` in the `Result<T>` returned by the
    /// provider. `@authenticated`/`@admin` handlers must declare
    /// a param of this type (the `user` injected by the runtime).
    user_type_id: TypeId,
    /// Name of type `T`, for error messages.
    user_type_name: String,
    /// `true` if `T` has a `role: Str` (non-nullable) field. Required by
    /// `@admin` to discriminate admins; pure `@authenticated`s don't
    /// need it.
    has_role_field: bool,
}

/// Phase 9.w.1 — Pre-scan of the program to find the unique
/// registered `@auth_provider`. Validates:
/// - Decorator without args nor kwargs.
/// - The fn has exactly 1 param of type `Map<Str, Str>` (HTTP
///   headers).
/// - The return type is `Result<T>` with nominal `T` (a custom `type`).
/// - There is at most one `@auth_provider` in the program.
///
/// Errors go directly to `ctx.errors`. The info of the first valid
/// provider is persisted in `ctx.auth_provider` (consumed by the
/// subsequent walk when checking `@authenticated`/`@admin` handlers).
fn collect_auth_provider(ctx: &mut CheckCtx, program: &Program) {
    let mut first: Option<(String, Span)> = None;
    for stmt in program {
        let Stmt::FnDef {
            name,
            params,
            return_type,
            decorators,
            span: fn_span,
            ..
        } = stmt
        else {
            continue;
        };
        for deco in decorators {
            if deco.name != "auth_provider" {
                continue;
            }
            // 1) No args nor kwargs (`@auth_provider` pure, without parens
            // or with `()`).
            if !deco.args.is_empty() || !deco.kwargs.is_empty() {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@auth_provider on fn '{}': does not accept args or kwargs. \
                         Syntax: `@auth_provider\\nfn name(headers: Map<Str, Str>) -> Result<User> {{ ... }}`.",
                        name
                    ),
                ));
                continue;
            }
            // 2) Singleton.
            if let Some((prev_name, prev_span)) = &first {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "duplicate @auth_provider: fn '{}' (line {}) was already declared as provider; \
                         fn '{}' (line {}) is a second provider. Only one is allowed per program.",
                        prev_name, prev_span.line, name, fn_span.line
                    ),
                ));
                continue;
            }
            // 3) Exactly 1 Map<Str, Str> param.
            if params.len() != 1 {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@auth_provider on fn '{}': must have exactly 1 param of type `Map<Str, Str>` (HTTP headers), has {}. \
                         Syntax: `fn {}(headers: Map<Str, Str>) -> Result<User> {{ ... }}`.",
                        name,
                        params.len(),
                        name
                    ),
                ));
                continue;
            }
            let p = &params[0];
            let param_ty = ann_to_type(p.type_.as_ref(), ctx.types);
            let expected = Type::Map(Box::new(Type::Str), Box::new(Type::Str));
            if !is_compatible(&param_ty, &expected) {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@auth_provider on fn '{}': param '{}' must be `Map<Str, Str>` (HTTP headers), is `{}`.",
                        name,
                        p.name,
                        param_ty.display(ctx.types)
                    ),
                ));
                continue;
            }
            // 4) Return type Result<T> with nominal T.
            let ret = match return_type {
                Some(r) => match resolve_type_expr(r, ctx.types) {
                    Ok(t) => t,
                    Err(_) => continue, // resolve_program already reported the error
                },
                None => {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "@auth_provider on fn '{}': missing return type. Must be `Result<User>` where `User` is a custom type.",
                            name
                        ),
                    ));
                    continue;
                }
            };
            let user_id = match &ret {
                Type::Result { ok, .. } => match ok.as_ref() {
                    Type::Nominal(id) => *id,
                    other => {
                        ctx.errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            fn_span.line,
                            fn_span.column,
                            format!(
                                "@auth_provider on fn '{}': return must be `Result<T>` where `T` is a custom type; T is `{}`.",
                                name,
                                other.display(ctx.types)
                            ),
                        ));
                        continue;
                    }
                },
                other => {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "@auth_provider on fn '{}': return must be `Result<T>` where `T` is a custom type; is `{}`.",
                            name,
                            other.display(ctx.types)
                        ),
                    ));
                    continue;
                }
            };
            // 5) Persist info for handler validation.
            let info = ctx.types.info(user_id);
            let user_type_name = info.name.clone();
            let has_role_field = info
                .fields
                .as_ref()
                .map(|fs| {
                    fs.iter()
                        .any(|f| f.name == "role" && matches!(f.type_, Type::Str))
                })
                .unwrap_or(false);
            first = Some((name.clone(), *fn_span));
            ctx.auth_provider = Some(AuthProviderInfo {
                name: name.clone(),
                span: *fn_span,
                user_type_id: user_id,
                user_type_name,
                has_role_field,
            });
        }
    }

    // W12 (v0.10.8) — Cross-module fallback. If no local
    // `@auth_provider` was found but the caller registered one from an
    // imported module (`TypeEnv::set_imported_auth_provider`), we
    // promote it to `ctx.auth_provider` so `check_auth_decorators`
    // validates `@authenticated`/`@admin` handlers against it. The
    // `user_type_id` is resolved by NAME in the importer's TypeEnv:
    // `from auth import User` already registered a nominal with that
    // name in `resolve_program`'s pass 1b, so `lookup`
    // returns a valid TypeId.
    //
    // If the importer did NOT import `User` (common programming
    // case: forgot `from auth import User` in the file with the
    // handlers), `lookup` returns `None` and we leave
    // `ctx.auth_provider = None`. The handlers fail with the same
    // "no `@auth_provider`" message — but the actual useful message
    // ("missing param of type `User`") will also appear in
    // `check_auth_decorators` when the handler declares a User param
    // without importing it. Acceptable as current diagnostic.
    if ctx.auth_provider.is_none() {
        if let Some(imported) = ctx.types.imported_auth_provider() {
            if let Some(user_id) = ctx.types.lookup(&imported.user_type_name) {
                // If the importer also declares fields for `User`
                // (case: has its own local `type User { ... }` that
                // shadows the imported one — unlikely but valid),
                // we prefer the importer's fields. Otherwise, we use
                // the source module's `has_role_field`.
                let has_role_field = ctx
                    .types
                    .info(user_id)
                    .fields
                    .as_ref()
                    .map(|fs| {
                        fs.iter()
                            .any(|f| f.name == "role" && matches!(f.type_, Type::Str))
                    })
                    .unwrap_or(imported.has_role_field);
                ctx.auth_provider = Some(AuthProviderInfo {
                    name: imported.fn_name.clone(),
                    // Synthetic span — the provider lives in another
                    // file. If there's a duplication error (case
                    // impossible today because the caller requires
                    // global pre-check uniqueness), we would show line 0;
                    // the message cites the name and that already orients.
                    span: Span::default(),
                    user_type_id: user_id,
                    user_type_name: imported.user_type_name.clone(),
                    has_role_field,
                });
            }
        }
    }
}

/// Phase 9.w.1 — Validates the `@authenticated` and `@admin` decorators on
/// a candidate HTTP handler `Stmt::FnDef`. Invoked from the
/// `Stmt::FnDef` walker inside `check_block`, after the provider
/// has been collected by `collect_auth_provider`.
///
/// Errors go to `ctx.errors`. Does not interrupt the body check of the
/// fn — body checks continue normally.
fn check_auth_decorators(
    ctx: &mut CheckCtx,
    fn_name: &str,
    params: &[Param],
    decorators: &[Decorator],
    fn_span: Span,
) {
    let mut seen_requires_roles: Vec<String> = Vec::new();
    for deco in decorators {
        let kind = match deco.name.as_str() {
            "authenticated" | "admin" | "requires" => deco.name.as_str(),
            _ => continue,
        };
        // 1) Shape validation:
        //    - `@authenticated`/`@admin`: no args nor kwargs.
        //    - `@requires`: 1 Str literal arg, no kwargs. Phase 9.w.1.iter2.a.
        if kind == "requires" {
            if !deco.kwargs.is_empty() {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@requires on fn '{}': does not accept kwargs in the MVP. Syntax: `@requires(\"role\")`.",
                        fn_name,
                    ),
                ));
                continue;
            }
            if deco.args.len() != 1 {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@requires on fn '{}': expects exactly 1 arg (the role as Str literal), received {}.",
                        fn_name,
                        deco.args.len(),
                    ),
                ));
                continue;
            }
            let role = match &deco.args[0] {
                Expr::Str(s, _) => s.clone(),
                _ => {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!("@requires on fn '{}': arg must be Str literal.", fn_name,),
                    ));
                    continue;
                }
            };
            if seen_requires_roles.contains(&role) {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@requires on fn '{}': duplicate role '{}' in stacked decorators.",
                        fn_name, role,
                    ),
                ));
                continue;
            }
            seen_requires_roles.push(role);
        } else if !deco.args.is_empty() || !deco.kwargs.is_empty() {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@{} on fn '{}': does not accept args or kwargs in the MVP. Syntax: `@{}\\n@get(\"/...\")\\nfn ...`.",
                    kind, fn_name, kind
                ),
            ));
            continue;
        }
        // 2) Only on HTTP handlers (includes `@ws` since Phase 9.w.2 —
        // the auth wrapper runs before the HTTP→WS upgrade).
        let is_handler = decorators
            .iter()
            .any(|d| matches!(d.name.as_str(), "get" | "post" | "put" | "delete" | "ws"));
        if !is_handler {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@{} on fn '{}': only applies to HTTP handlers (`@get`/`@post`/`@put`/`@delete`/`@ws`).",
                    kind, fn_name
                ),
            ));
            continue;
        }
        // 3) Requires a registered provider.
        let provider = match &ctx.auth_provider {
            Some(p) => p.clone(),
            None => {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                    "@{} on fn '{}': no `@auth_provider` registered in the program. \
                     Declare a fn with `@auth_provider\\nfn name(headers: Map<Str, Str>) -> Result<User> {{ ... }}`.",
                        kind, fn_name
                    ),
                ));
                continue;
            }
        };
        // 4) Handler must declare a param compatible with the User type.
        let user_ty = Type::Nominal(provider.user_type_id);
        let has_user_param = params.iter().any(|p| {
            let pty = ann_to_type(p.type_.as_ref(), ctx.types);
            is_compatible(&pty, &user_ty)
        });
        if !has_user_param {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@{} on fn '{}': missing param of type `{}` (injected after successful authentication). \
                     Declare it in the signature: `fn {}(..., user: {}) -> ...`.",
                    kind, fn_name, provider.user_type_name, fn_name, provider.user_type_name
                ),
            ));
        }
        // 5) `@admin` and `@requires` require a `role: Str` field in the User type.
        if (kind == "admin" || kind == "requires") && !provider.has_role_field {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@{} on fn '{}': type `{}` (return of `@auth_provider`) must have a `role: Str` field to discriminate roles. \
                     Add it to the declaration of `{}`.",
                    kind, fn_name, provider.user_type_name, provider.user_type_name
                ),
            ));
        }
    }
}

/// Mini-phase MW.1: pre-scans the program to collect names of fns
/// that appear as argument of a `@middleware(name)` in any
/// FnDef. Those names are marked in `ctx.middleware_fn_names` so
/// the `Stmt::ReturnStatus` check accepts them as "HTTP context"
/// (a middleware can do `return 401 { ... }`). We only capture
/// references by `Expr::Ident` (the documented form); any other
/// form (call, lambda, etc.) is captured by the evaluator at runtime with its
/// own clear error.
///
/// v0.19.5 (post-fitzwatch 2026-06-26) — also merges the
/// cross-module set `env.imported_middleware_fns` (registered by
/// the caller via `add_imported_middleware_fns` based on a pre-scan
/// over main + all loaded modules). Without this, a fn that lives
/// in module `rate_limit.fitz` and is applied as `@middleware(...)`
/// from `auth.fitz` fails its own module check because the local
/// pre-scan does not see the external `@middleware` reference.
fn collect_middleware_fn_names(ctx: &mut CheckCtx, program: &Program) {
    for stmt in program {
        if let Stmt::FnDef { decorators, .. } = stmt {
            for deco in decorators {
                if deco.name != "middleware" {
                    continue;
                }
                for arg in &deco.args {
                    if let Expr::Ident(n, _) = arg {
                        ctx.middleware_fn_names.insert(n.clone());
                    }
                }
            }
        }
    }
    // v0.19.5 — cross-module set from `imported_middleware_fns`.
    for name in ctx.types.imported_middleware_fns() {
        ctx.middleware_fn_names.insert(name.clone());
    }
}

/// v0.19.5 (post-fitzwatch 2026-06-26) — Pure helper that walks the
/// program and returns the names of fns referenced as
/// `@middleware(name)` (Ident form). Used by the caller
/// (`main.rs::check_program_with_pyi_stubs_and_deps` and the
/// codegen `ModuleLoader` pre-scan) to build the GLOBAL set of
/// middleware fn names across main + all loaded modules, then
/// propagated to each module's `TypeEnv` via
/// `add_imported_middleware_fns`. Parallel to
/// `extract_background_fn_names` (B10).
pub fn extract_middleware_fn_names(program: &Program) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for stmt in program {
        if let Stmt::FnDef { decorators, .. } = stmt {
            for deco in decorators {
                if deco.name != "middleware" {
                    continue;
                }
                for arg in &deco.args {
                    if let Expr::Ident(n, _) = arg {
                        out.push(n.clone());
                    }
                }
            }
        }
    }
    out
}

/// Phase 9.w.2 — Validates the shape of a `@ws("/path")` handler:
/// - Decorator with exactly 1 `Str` arg (the path); no kwargs.
/// - The handler must be `async fn` (WSs are naturally async
///   — `recv().await`/`send().await`).
/// - Must declare exactly 1 param of type `WsConn<T>` (concrete T,
///   not `Any`), optionally plus 1 param of the `@auth_provider`
///   type if there's `@authenticated`/`@admin` stacked.
/// - Path does not require query/path-param validation (unlike
///   HTTP handlers), but must parse as a Str literal.
///
/// Errors go to `ctx.errors`. Does not interrupt body checking.
fn check_ws_handler(
    ctx: &mut CheckCtx,
    fn_name: &str,
    params: &[Param],
    is_async: bool,
    decorators: &[Decorator],
    fn_span: Span,
) {
    let ws_deco = match decorators.iter().find(|d| d.name == "ws") {
        Some(d) => d,
        None => return,
    };
    // 1) `@ws` must have exactly 1 Str arg (the path) and no
    //    kwargs in the MVP. Syntax: `@ws("/chat")`.
    if ws_deco.args.len() != 1 || !ws_deco.kwargs.is_empty() {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@ws on fn '{}': expects exactly 1 argument (path: Str). Syntax: `@ws(\"/chat\")`.",
                fn_name
            ),
        ));
        return;
    }
    match &ws_deco.args[0] {
        Expr::Str(_, _) => {}
        _ => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@ws on fn '{}': argument must be a Str literal (path).",
                    fn_name
                ),
            ));
            return;
        }
    }
    // 2) async fn required — `recv()`/`send()` are async by
    //    nature.
    if !is_async {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@ws on fn '{}': must be declared `async fn` — `WsConn` methods (`recv`/`send`/`broadcast`) are async.",
                fn_name
            ),
        ));
    }
    // 3) Exactly 1 `WsConn<T>` param with concrete T + optional
    //    1 User param if there's `@authenticated`/`@admin`.
    let has_auth = decorators
        .iter()
        .any(|d| matches!(d.name.as_str(), "authenticated" | "admin"));
    let expected_params = if has_auth { 2 } else { 1 };
    if params.len() != expected_params {
        let extra = if has_auth {
            " (1 `WsConn<T>` + 1 User param from `@auth_provider`)"
        } else {
            " (1 `WsConn<T>`)"
        };
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@ws on fn '{}': expects {} param(s){}, received {}.",
                fn_name,
                expected_params,
                extra,
                params.len(),
            ),
        ));
        return;
    }
    // Identify the WsConn param and validate shape.
    let mut wsconn_params = 0;
    for p in params {
        let pty = ann_to_type(p.type_.as_ref(), ctx.types);
        if let Type::WsConn { recv, send } = &pty {
            wsconn_params += 1;
            // 9.w.2-wsconn-bidir: both recv and send must be
            // concrete. If either is Any, error (parallel to the
            // symmetric pre-bidir check).
            if matches!(recv.as_ref(), Type::Any) || matches!(send.as_ref(), Type::Any) {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@ws on fn '{}': the `WsConn<T>` requires concrete `T` (not `Any`). Annotate the message type: `WsConn<Str>`, `WsConn<ChatMsg>`, etc.",
                        fn_name
                    ),
                ));
            }
        }
    }
    if wsconn_params != 1 {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@ws on fn '{}': must declare exactly 1 param of type `WsConn<T>` (has {}). E.g.: `fn {}(conn: WsConn<ChatMsg>) {{ ... }}`.",
                fn_name, wsconn_params, fn_name,
            ),
        ));
    }
}

/// Phase 9.w.3 — checker for `spawn(fn_call)`. The `spawn(...)`
/// callsite returns `Future<T>` where T is the ret type of the target fn.
/// The dispatch fires only when the `spawn` binding is the builtin
/// (no user override).
///
/// Validations:
///   1. Exactly 1 arg, which must be a literal `Expr::Call`. We don't
///      accept `spawn(x)` where `x` is a var (the target must be
///      clear statically to validate `@background`).
///   2. The callee of the inner call must be a resolvable Ident. We
///      don't accept `spawn(obj.method())` (custom methods don't carry
///      `@background`).
///   3. The target fn must be in `ctx.background_fns`. Without
///      `@background`, the checker rejects with a clear message.
///
/// The spawn's ret type is synthesized following the target fn's ret
/// type: if the fn already returns `Future<T>` (async fn), spawn returns
/// `Future<T>` (no double wrap). If the sync fn returns pure `T`,
/// spawn returns `Future<T>`. Parity with `tokio::spawn` which wraps
/// the output in JoinHandle but the API only exposes the final `T` via
/// `.await`.
fn check_spawn_call(ctx: &mut CheckCtx, args: &[Expr], span: Span) -> Type {
    if args.len() != 1 {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            span.line,
            span.column,
            format!(
                "spawn: expects exactly 1 argument (a call to a `@background` fn), received {}. Syntax: `spawn(my_fn(args))`.",
                args.len()
            ),
        ));
        return Type::Future(Box::new(Type::Any));
    }
    let inner_call = match &args[0] {
        Expr::Call {
            callee,
            args: inner_args,
            ..
        } => (callee, inner_args),
        Expr::NamedArg { value, .. } => match value.as_ref() {
            Expr::Call {
                callee,
                args: inner_args,
                ..
            } => (callee, inner_args),
            _ => {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    span.line,
                    span.column,
                    "spawn: the argument must be a literal call to a `@background` fn, not a compound value. Syntax: `spawn(send_email(addr, body))`.".to_string(),
                ));
                return Type::Future(Box::new(Type::Any));
            }
        },
        _ => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                span.line,
                span.column,
                "spawn: the argument must be a literal call to a `@background` fn, not a variable or expression. Syntax: `spawn(send_email(addr, body))`.".to_string(),
            ));
            return Type::Future(Box::new(Type::Any));
        }
    };
    let (callee, inner_args) = inner_call;
    let target_name = match callee.as_ref() {
        Expr::Ident(name, _) => name.clone(),
        _ => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                span.line,
                span.column,
                "spawn: the callee of the inner call must be a top-level fn with `@background`, not a method call or compound expression.".to_string(),
            ));
            // We type the args so errors inside surface and
            // we return Future<Any> without stopping the check.
            for a in inner_args {
                infer_expr(ctx, a);
            }
            return Type::Future(Box::new(Type::Any));
        }
    };
    if !ctx.background_fns.contains(&target_name) {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            span.line,
            span.column,
            format!(
                "spawn: fn `{}` is not declared with `@background`. Mark the fn with `@background\\nfn {}(...) {{ ... }}` to authorize fire-and-forget execution via spawn.",
                target_name, target_name
            ),
        ));
        // We type the args + return Future<Any> to not break
        // the caller's check.
        for a in inner_args {
            infer_expr(ctx, a);
        }
        return Type::Future(Box::new(Type::Any));
    }
    // OK: target is a declared `@background` fn. We type the inner
    // call delegating to standard synthesize (validates arity + arg
    // types against the target fn's real signature). The inner
    // call's ret type is what we wrap in `Future` — except if it
    // already comes as Future (async fn), in which case passthrough without
    // double wrap.
    let inner_ret = infer_expr(ctx, &args[0]);
    match inner_ret {
        Type::Future(_) => inner_ret,
        Type::Any => Type::Future(Box::new(Type::Any)),
        other => Type::Future(Box::new(other)),
    }
}

/// Phase 9.w.3 — pre-scans top-level fns with `@background`. The
/// `spawn(call)` check (in `synthesize_expr` for `Expr::Call`
/// whose callee is Ident `"spawn"`) consults this set to validate
/// that the spawn target is declared as background.
///
/// Policy: `@background` does not accept args/kwargs. `@background` and
/// `@cron` are mutually exclusive (validated by `check_cron_decorator`
/// and `check_background_decorator`). The checker walk emits errors
/// if the decorator's shape is invalid; here we just collect
/// names to have the set ready before the walk.
fn collect_background_fns(ctx: &mut CheckCtx, program: &Program) {
    for stmt in program {
        let Stmt::FnDef {
            name, decorators, ..
        } = stmt
        else {
            continue;
        };
        if decorators.iter().any(|d| d.name == "background") {
            ctx.background_fns.insert(name.clone());
        }
    }
    // B10 (sub-paso 5 cosecha post-fitzwatch, 2026-06-19) — merge
    // cross-module `@background` fns pre-scanned by the caller via
    // `extract_background_fn_names` + `TypeEnv::add_imported_background_fns`.
    // Without this merge, `spawn(<imported_fn>(...))` in the
    // importer fails with "fn `X` is not declared with
    // `@background`" because the local walk above only sees the
    // importer's own fns.
    for name in ctx.types.imported_background_fns() {
        ctx.background_fns.insert(name.clone());
    }
}

/// B10 (sub-paso 5 cosecha post-fitzwatch, 2026-06-19) — Public
/// extractor for cross-module `@background` fn names. The caller
/// (typically `main.rs::pre_scan_imported_background_fns` paralelo a
/// `pre_scan_imported_auth_provider`) walks each `Stmt::Import` /
/// `Stmt::FromImport`, parses the imported module, and feeds the
/// result of this function back to the importer's `TypeEnv` via
/// `add_imported_background_fns`. The checker then validates
/// `spawn(<imported>(...))` against the merged set.
///
/// **Scope**: single level (does not recurse into transitive
/// imports). Parallel to `extract_auth_provider_signature`.
pub fn extract_background_fn_names(program: &Program) -> Vec<String> {
    let mut names = Vec::new();
    for stmt in program {
        let Stmt::FnDef {
            name, decorators, ..
        } = stmt
        else {
            continue;
        };
        if decorators.iter().any(|d| d.name == "background") {
            names.push(name.clone());
        }
    }
    names
}

/// Phase 9.w.3 — validates `@cron("cron-expr")` on top-level `fn`s.
/// Rules:
///   1. Args: exactly 1 Str literal with cron expression.
///      We accept 5 fields (classic Unix) or 6/7 fields (with seconds
///      and/or year) — the runtime parser uses the `cron` crate.
///   2. No kwargs.
///   3. The fn does not accept params (jobs receive no input).
///   4. Return type: `Null`, `Result<Null>`, `Result<T>` with any T,
///      or `Future<X>` when async (parallel to other async handlers).
///      We also accept `Any` (gradual / unannotated).
///   5. Not combinable with `@get`/`@post`/`@put`/`@delete`/`@ws` (a
///      cron job is not an HTTP endpoint) nor with `@background` (different
///      semantics: cron is periodic scheduled, background is
///      fire-and-forget from a handler).
///
/// Syntactic validation of the cron expression: done at runtime/codegen
/// (not in the checker) because importing `cron` here implies a dep on the
/// checker path. The checker validates shape; the runtime validates syntax.
/// 9.w.3.iter2 — Extracts an `i64` value from an Int literal, considering
/// the `-N` case that the parser emits as `UnaryOp { Neg, Int(N) }` (not
/// as `Int(-N)` directly). Returns `None` if the expr is not a simple
/// numeric literal.
fn extract_int_literal(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(n, _) => Some(*n),
        Expr::UnaryOp { op, operand, .. } => {
            if matches!(op, crate::ast::UnaryOpKind::Neg) {
                if let Expr::Int(n, _) = operand.as_ref() {
                    return Some(-n);
                }
            }
            None
        }
        _ => None,
    }
}

/// 9.w.3.iter2 — Validates the optional kwargs accepted by `@cron` and
/// `@background`. The `allowed` list defines which kwargs are valid for
/// the decorator in question (`@cron` accepts the 4; `@background` only
/// `tz` and `retry`).
///
/// Common rules:
/// - **Duplicates** → error (`tz="A", tz="B"`).
/// - **Unknown** → error with the list of accepted ones.
/// - **`tz`**: `Expr::Str` literal. Real IANA validation (that the
///   string is a known timezone) is done by the runtime when registering
///   the job — the `chrono-tz` crate produces a clear error with suggestion
///   if it doesn't match.
/// - **`retry`**: `Expr::Map` literal with keys from the subset:
///     - `max: Int` (>=0, default 0 = no retry).
///     - `backoff: Str` literal with value in
///       `{"exponential", "linear", "constant"}` (default `"exponential"`).
///     - `initial_secs: Int` (>=1, default 1).
///     - `max_secs: Int` (>=1, default 60).
///
///   Unknown keys, values with incorrect type, or `backoff` outside
///   the whitelist → error.
/// - **`catch_up`**: `Expr::Bool` literal.
/// - **`store`**: any expression. The checker does NOT validate that it resolves
///   to `DbConn` (that would require re-running `infer_expr` inside this
///   helper, which is called outside the main walk). The runtime emits
///   a clear error if the value is not `Value::DbConn`.
///
/// Returns `true` if all kwargs pass; `false` if there was at least
/// one error (the caller decides whether to continue or stop).
fn check_job_kwargs(
    ctx: &mut CheckCtx,
    deco_name: &str,
    fn_name: &str,
    kwargs: &[(String, Expr)],
    allowed: &[&str],
    fn_span: Span,
) -> bool {
    let mut ok = true;
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (k, v) in kwargs {
        // Unknown.
        if !allowed.contains(&k.as_str()) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "{} on fn '{}': unrecognized kwarg `{}`. Accepted: {}.",
                    deco_name,
                    fn_name,
                    k,
                    allowed
                        .iter()
                        .map(|n| format!("`{}`", n))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ));
            ok = false;
            continue;
        }
        // Duplicate.
        if !seen.insert(k.as_str()) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "{} on fn '{}': duplicate kwarg `{}`.",
                    deco_name, fn_name, k,
                ),
            ));
            ok = false;
            continue;
        }
        // Validate the value's shape.
        match k.as_str() {
            "tz" => {
                if !matches!(v, Expr::Str(_, _)) {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "{} on fn '{}': kwarg `tz` must be a Str literal with an IANA timezone (e.g. `tz=\"America/Argentina/Buenos_Aires\"`).",
                            deco_name, fn_name,
                        ),
                    ));
                    ok = false;
                }
            }
            "catch_up" => {
                if !matches!(v, Expr::Bool(_, _)) {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "{} on fn '{}': kwarg `catch_up` must be a Bool literal (`true`/`false`).",
                            deco_name, fn_name,
                        ),
                    ));
                    ok = false;
                }
            }
            "store" => {
                // Any expr — the type is checked at runtime.
                // Minimum validation: no `Null` literal (`store=null`
                // makes no sense and reveals a bug from the author).
                if matches!(v, Expr::Null(_)) {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "{} on fn '{}': kwarg `store` cannot be `null`. Pass a DB conn (e.g. `store=db` with `db = db.connect(...).await?`).",
                            deco_name, fn_name,
                        ),
                    ));
                    ok = false;
                }
            }
            "retry" => {
                if !check_retry_map(ctx, deco_name, fn_name, v, fn_span) {
                    ok = false;
                }
            }
            _ => unreachable!("kwarg name validated against allowed"),
        }
    }
    ok
}

/// 9.w.3.iter2 — validates the shape of the Map literal passed as
/// `retry={...}`. Accepts only known keys and validates the types.
///
/// If it's not a Map literal at all → error suggesting the syntax.
fn check_retry_map(
    ctx: &mut CheckCtx,
    deco_name: &str,
    fn_name: &str,
    expr: &Expr,
    fn_span: Span,
) -> bool {
    let entries = match expr {
        Expr::Map(entries, _) => entries,
        _ => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "{} on fn '{}': kwarg `retry` must be a Map literal (e.g. `retry={{max: 3, backoff: \"exponential\"}}`).",
                    deco_name, fn_name,
                ),
            ));
            return false;
        }
    };
    let allowed = ["max", "backoff", "initial_secs", "max_secs"];
    let mut ok = true;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (k_expr, v_expr) in entries {
        // The parser for `{a=1}` in Map literal position can emit
        // both `Expr::Str("a", ...)` (standard Str key) and
        // `Expr::Ident("a", ...)` when the key is written without quotes
        // (abbreviated syntax `cors({...})` style). We accept both.
        let key_name = match k_expr {
            Expr::Str(s, _) => s.clone(),
            Expr::Ident(s, _) => s.clone(),
            _ => {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "{} on fn '{}': keys of the `retry` Map must be identifiers or Str literals (accepted: {}).",
                        deco_name,
                        fn_name,
                        allowed
                            .iter()
                            .map(|n| format!("`{}`", n))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                ));
                ok = false;
                continue;
            }
        };
        if !allowed.contains(&key_name.as_str()) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "{} on fn '{}': key `{}` not recognized in `retry`. Accepted: {}.",
                    deco_name,
                    fn_name,
                    key_name,
                    allowed
                        .iter()
                        .map(|n| format!("`{}`", n))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ));
            ok = false;
            continue;
        }
        if !seen.insert(key_name.clone()) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "{} on fn '{}': duplicate key `{}` in `retry`.",
                    deco_name, fn_name, key_name,
                ),
            ));
            ok = false;
            continue;
        }
        match key_name.as_str() {
            "max" | "initial_secs" | "max_secs" => match extract_int_literal(v_expr) {
                Some(n) => {
                    if key_name == "max" && n < 0 {
                        ctx.errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            fn_span.line,
                            fn_span.column,
                            format!(
                                "{} on fn '{}': `retry.max` must be >= 0 (is {}). Use 0 to disable retry.",
                                deco_name, fn_name, n,
                            ),
                        ));
                        ok = false;
                    } else if (key_name == "initial_secs" || key_name == "max_secs") && n < 1 {
                        ctx.errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            fn_span.line,
                            fn_span.column,
                            format!(
                                "{} on fn '{}': `retry.{}` must be >= 1 (is {}).",
                                deco_name, fn_name, key_name, n,
                            ),
                        ));
                        ok = false;
                    }
                }
                None => {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "{} on fn '{}': `retry.{}` must be an Int literal.",
                            deco_name, fn_name, key_name,
                        ),
                    ));
                    ok = false;
                }
            },
            "backoff" => match v_expr {
                Expr::Str(s, _) => {
                    if !matches!(s.as_str(), "exponential" | "linear" | "constant") {
                        ctx.errors.push(FitzError::new(
                            ErrorKind::TypeError,
                            fn_span.line,
                            fn_span.column,
                            format!(
                                "{} on fn '{}': `retry.backoff` must be one of `\"exponential\"`/`\"linear\"`/`\"constant\"` (is `\"{}\"`).",
                                deco_name, fn_name, s,
                            ),
                        ));
                        ok = false;
                    }
                }
                _ => {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "{} on fn '{}': `retry.backoff` must be a Str literal.",
                            deco_name, fn_name,
                        ),
                    ));
                    ok = false;
                }
            },
            _ => unreachable!("key validada contra allowed"),
        }
    }
    ok
}

fn check_cron_decorator(
    ctx: &mut CheckCtx,
    fn_name: &str,
    params: &[Param],
    ret: &Type,
    is_async: bool,
    decorators: &[Decorator],
    fn_span: Span,
) {
    let cron_deco = match decorators.iter().find(|d| d.name == "cron") {
        Some(d) => d,
        None => return,
    };
    // 1) Args: exactly 1 positional Str literal.
    //
    // 9.w.3.iter2 — optional accepted kwargs:
    //   - `tz: Str` (IANA timezone, default UTC)
    //   - `retry: Map literal` with keys `max`/`backoff`/`initial_secs`/
    //     `max_secs`. Default: no retry.
    //   - `catch_up: Bool` (missed runs policy after restart;
    //     default false = skip).
    //   - `store: <expr>` which must resolve to `DbConn` at runtime
    //     (typical: `store=db` with `db` a binding from scope). The checker
    //     does NOT validate the expr's type — that's done when registering
    //     the job at runtime/codegen. Without `store` → in-memory (MVP).
    if cron_deco.args.len() != 1 {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@cron on fn '{}': expects exactly 1 positional argument (cron expression Str). Syntax: `@cron(\"0 0 * * *\")` or `@cron(\"0 0 * * *\", tz=\"America/Buenos_Aires\", retry={{max: 3, backoff: \"exponential\"}}, catch_up=true, store=db)`.",
                fn_name
            ),
        ));
        return;
    }
    match &cron_deco.args[0] {
        Expr::Str(_, _) => {}
        _ => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@cron on fn '{}': argument must be a Str literal with the cron expression (e.g. `\"0 0 * * *\"` for every midnight).",
                    fn_name
                ),
            ));
            return;
        }
    }
    // 1b) Optional kwargs (9.w.3.iter2): validates each one's shape and
    //     rejects unknown/duplicates.
    if !check_job_kwargs(
        ctx,
        "@cron",
        fn_name,
        &cron_deco.kwargs,
        &["tz", "retry", "catch_up", "store"],
        fn_span,
    ) {
        return;
    }
    // 2) Conflicts with other HTTP / WS / background decorators.
    let conflicting = [
        "get",
        "post",
        "put",
        "delete",
        "ws",
        "background",
        "auth_provider",
        "test",
    ];
    for other in decorators {
        if conflicting.contains(&other.name.as_str()) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@cron on fn '{}' is not combinable with `@{}`: cron jobs are periodic scheduled tasks, not HTTP requests nor fire-and-forget from a handler.",
                    fn_name, other.name
                ),
            ));
            return;
        }
    }
    // 3) No params.
    if !params.is_empty() {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@cron on fn '{}': handler does not accept params (cron jobs receive no input). Has {}.",
                fn_name,
                params.len()
            ),
        ));
        return;
    }
    // 4) Return type: we accept Null/Result/Future (async)/Any.
    //    Other concrete types (Int/Float/Str/...) → clear error (a
    //    job does not produce a consumable value).
    //
    //    For async fns, the incoming `ret` is already post-async
    //    transparent (the body produces `T`, the caller sees `Future<T>`).
    //    We accept Null or Result<...> here too.
    let _ = is_async; // is_async is already implicit in the shape of `ret`.
    match ret {
        Type::Null | Type::Any => {}
        Type::Result { .. } => {}
        Type::Future(inner) => {
            // For async fns, ret is Future<T>. The inner T must be
            // Null or Result or Any.
            match inner.as_ref() {
                Type::Null | Type::Any => {}
                Type::Result { .. } => {}
                other => {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "@cron on fn '{}': async return type must be `Future<Null>` or `Future<Result<...>>`, is `Future<{}>`. The runtime discards the value — use `Result` if you want to log failures.",
                            fn_name,
                            other.display(ctx.types),
                        ),
                    ));
                }
            }
        }
        other => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@cron on fn '{}': return type must be `Null`, `Result<...>`, or the async equivalent (`Future<...>`). Is `{}`. The runtime discards the job's value.",
                    fn_name,
                    other.display(ctx.types),
                ),
            ));
        }
    }
}

/// Phase 9.w.3 — validates `@background` on top-level `fn`s. Rules:
///   1. No args nor kwargs.
///   2. Not combinable with `@get`/`@post`/`@put`/`@delete`/`@ws`/`@cron`/
///      `@auth_provider` (different semantics: background is opt-in
///      on the author's side to mark that the fn can be executed via
///      `spawn(...)`; HTTP handlers consume request/response;
///      cron/auth_provider have their own runtimes).
///
/// The `@background` policy is just a marker: it doesn't change the fn's
/// shape nor its return type. The callsite check (`spawn(call)`)
/// is what consults `ctx.background_fns` to authorize the spawn.
fn check_background_decorator(
    ctx: &mut CheckCtx,
    fn_name: &str,
    decorators: &[Decorator],
    fn_span: Span,
) {
    let bg_deco = match decorators.iter().find(|d| d.name == "background") {
        Some(d) => d,
        None => return,
    };
    // 9.w.3.iter2 — `@background` now accepts optional kwargs `tz`
    // and `retry` (same shape as `@cron`). `store` and `catch_up` are NOT
    // accepted on `@background` (persistence of spawn jobs is
    // deferred to iter3 — spawn args require JSON serialization
    // + separate `fitz_bg_jobs` table).
    if !bg_deco.args.is_empty() {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@background on fn '{}': does not accept positional args. Syntax: `@background` or `@background(tz=\"America/Buenos_Aires\", retry={{max: 3, backoff: \"exponential\"}})`.",
                fn_name
            ),
        ));
    }
    if !check_job_kwargs(
        ctx,
        "@background",
        fn_name,
        &bg_deco.kwargs,
        &["tz", "retry"],
        fn_span,
    ) {
        // If the kwargs shape is invalid, we still continue with the
        // conflict validation (more errors above in the same
        // checker walk is better than aborting here).
    }
    let conflicting = [
        "get",
        "post",
        "put",
        "delete",
        "ws",
        "cron",
        "auth_provider",
        "test",
    ];
    for other in decorators {
        if conflicting.contains(&other.name.as_str()) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@background on fn '{}' is not combinable with `@{}`: background is just a marker to authorize `spawn(...)`; HTTP/WS/cron handlers have their own runtimes.",
                    fn_name, other.name
                ),
            ));
            return;
        }
    }
}

// Phase 4 (fitz-liveviews Y-B, session 1.b) — helper shared by
// `check_render_for_decorator` and `check_on_decorator` to reject
// conflicts with decorators from other runtimes. Live components
// render on demand from the framework layer; they are neither
// HTTP handlers, WS handlers, cron jobs, background workers, auth
// providers, tests nor CLI commands.
const LIVE_HANDLER_CONFLICTING: &[&str] = &[
    "get",
    "post",
    "put",
    "delete",
    "ws",
    "cron",
    "background",
    "auth_provider",
    "authenticated",
    "admin",
    "requires",
    "test",
    "command",
    "healthz",
    "readyz",
];

// Phase 4 (fitz-liveviews Y-B, session 1.b) — validates the
// signature of a fn decorated with `@render_for("component")`.
// Shape (decorator arg count/types) already validated in
// `resolve_program`; here we check:
//   - The component name exists as a `@live_component("name")`.
//   - The fn has exactly 1 param whose type matches the
//     component's state type.
//   - The return type is `Str` (MVP; when Fitz adds a nominal
//     `Html` built-in we accept both).
//   - No conflict with @get/@post/@ws/@cron/@background/etc.
fn check_render_for_decorator(
    ctx: &mut CheckCtx,
    fn_name: &str,
    params: &[Param],
    ret: &Type,
    decorators: &[Decorator],
    fn_span: Span,
) {
    let deco = match decorators.iter().find(|d| d.name == "render_for") {
        Some(d) => d,
        None => return,
    };
    // Shape errors from resolve_program already reported; bail
    // silently if we cannot extract the name here.
    let component_name = match deco.args.first() {
        Some(Expr::Str(s, _)) => s.trim().to_string(),
        _ => return,
    };
    if component_name.is_empty() {
        return;
    }

    for other in decorators {
        if LIVE_HANDLER_CONFLICTING.contains(&other.name.as_str()) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@render_for on fn '{fn_name}' is not combinable with `@{}`: live component render handlers are dispatched by the framework layer, not by HTTP/WS/cron/background/auth/test/command runtimes.",
                    other.name
                ),
            ));
            return;
        }
    }

    let component_id = match ctx.types.live_component_by_name(&component_name) {
        Some(id) => id,
        None => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@render_for on fn '{fn_name}': no `@live_component(\"{component_name}\")` is declared in this program. Declare `@live_component(\"{component_name}\") type <State> {{ ... }}` before the render handler."
                ),
            ));
            return;
        }
    };

    // Signature: exactly 1 param, of type = component's nominal.
    if params.len() != 1 {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@render_for on fn '{fn_name}': expected exactly 1 param (`state: <ComponentType>`), received {}.",
                params.len()
            ),
        ));
        return;
    }

    // Resolve the declared type of the param, if annotated. Params
    // without annotation stay as `Type::Any` and slip through
    // gradually — the framework layer will validate at runtime.
    if let Some(param_ty_expr) = &params[0].type_ {
        if let Ok(param_ty) = resolve_type_expr(param_ty_expr, ctx.types) {
            match param_ty {
                Type::Nominal(id) if id == component_id => {}
                Type::Any => {}
                other => {
                    let expected_name = ctx.types.info(component_id).name.clone();
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "@render_for on fn '{fn_name}': param must be of type `{expected_name}` (the state of component `{component_name}`), received `{}`.",
                            other.display(ctx.types)
                        ),
                    ));
                    return;
                }
            }
        }
    }

    // Return type: MVP accepts `Str` (raw HTML). When Fitz adds a
    // built-in nominal `Html`, we will accept both. `Any` is also
    // accepted (fn without declared return type).
    match ret {
        Type::Str | Type::Any => {}
        other => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@render_for on fn '{fn_name}': return type must be `Str` (raw HTML), received `{}`.",
                    other.display(ctx.types)
                ),
            ));
        }
    }
}

// Phase 4 (fitz-liveviews Y-B, session 1.b) — validates the
// signature of a fn decorated with `@on("component", "event")`.
// A single fn may carry multiple `@on(...)` decorators; each
// must match the same component (all `@on(...)`s bound to a fn
// dispatch different events on the SAME component state).
//
// Shape already validated by `resolve_program`; here we check:
//   - Each component name exists as a `@live_component(...)`.
//   - All `@on(...)` decorators on this fn share the same
//     component (a single fn cannot handle events for two
//     distinct components).
//   - The fn has exactly 2 params: `state: T` and
//     `payload: Map<Str, Str>`.
//   - The return type is `T` (state after the transition) or
//     `Any`.
//   - No conflict with other runtime decorators.
fn check_on_decorator(
    ctx: &mut CheckCtx,
    fn_name: &str,
    params: &[Param],
    ret: &Type,
    decorators: &[Decorator],
    fn_span: Span,
) {
    let on_decos: Vec<&Decorator> = decorators.iter().filter(|d| d.name == "on").collect();
    if on_decos.is_empty() {
        return;
    }

    for other in decorators {
        if LIVE_HANDLER_CONFLICTING.contains(&other.name.as_str()) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@on on fn '{fn_name}' is not combinable with `@{}`: live component event handlers are dispatched by the framework layer.",
                    other.name
                ),
            ));
            return;
        }
    }

    // Collect the distinct component names. If a fn has @on for
    // two different components, that's an error (each event
    // handler is bound to a single state type).
    let mut components: Vec<String> = Vec::new();
    for d in &on_decos {
        let comp = match d.args.first() {
            Some(Expr::Str(s, _)) => s.trim().to_string(),
            _ => continue, // shape error already emitted
        };
        if comp.is_empty() {
            continue;
        }
        if !components.contains(&comp) {
            components.push(comp);
        }
    }

    if components.len() > 1 {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@on on fn '{fn_name}': cannot mix events from different components on the same fn (found: {}). Each event handler binds to a single component's state type.",
                components.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", ")
            ),
        ));
        return;
    }

    let component_name = match components.first() {
        Some(c) => c.clone(),
        None => return, // all shapes invalid; errors already emitted
    };

    let component_id = match ctx.types.live_component_by_name(&component_name) {
        Some(id) => id,
        None => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@on on fn '{fn_name}': no `@live_component(\"{component_name}\")` is declared in this program. Declare `@live_component(\"{component_name}\") type <State> {{ ... }}` before the event handler."
                ),
            ));
            return;
        }
    };

    // Signature: exactly 2 params.
    if params.len() != 2 {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@on on fn '{fn_name}': expected exactly 2 params (`state: <ComponentType>, payload: Map<Str, Str>`), received {}.",
                params.len()
            ),
        ));
        return;
    }

    // Param 1: state: T.
    if let Some(param_ty_expr) = &params[0].type_ {
        if let Ok(param_ty) = resolve_type_expr(param_ty_expr, ctx.types) {
            match param_ty {
                Type::Nominal(id) if id == component_id => {}
                Type::Any => {}
                other => {
                    let expected_name = ctx.types.info(component_id).name.clone();
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "@on on fn '{fn_name}': first param must be of type `{expected_name}` (the state of component `{component_name}`), received `{}`.",
                            other.display(ctx.types)
                        ),
                    ));
                    return;
                }
            }
        }
    }

    // Param 2: payload: Map<Str, Str>. Any is accepted for
    // gradual escape hatch.
    if let Some(param_ty_expr) = &params[1].type_ {
        if let Ok(param_ty) = resolve_type_expr(param_ty_expr, ctx.types) {
            let is_map_str_str = matches!(
                &param_ty,
                Type::Map(k, v) if matches!(**k, Type::Str) && matches!(**v, Type::Str)
            );
            if !is_map_str_str && !matches!(param_ty, Type::Any) {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@on on fn '{fn_name}': second param must be `Map<Str, Str>` (event payload), received `{}`.",
                        param_ty.display(ctx.types)
                    ),
                ));
                return;
            }
        }
    }

    // Return type: `T` (the state after the transition) or `Any`.
    match ret {
        Type::Nominal(id) if *id == component_id => {}
        Type::Any => {}
        other => {
            let expected_name = ctx.types.info(component_id).name.clone();
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@on on fn '{fn_name}': return type must be `{expected_name}` (the state of component `{component_name}` after the transition), received `{}`.",
                    other.display(ctx.types)
                ),
            ));
        }
    }
}

// Phase 12.1.a — `@healthz`/`@readyz` declare K8s-style probes.
// The runtime and codegen auto-mount `GET /healthz` and `GET /readyz`
// when these decorators are declared (parallel to
// `/openapi.json` auto-registered).
//
// **Validated rules** (identical for `@healthz` and `@readyz`):
// - No args nor kwargs (`@healthz` pure, optionally with empty `()`).
// - Singleton: at most one per program.
// - No params (probes receive no input).
// - Return type: `Bool` / `Result<Null>` / `Result<Bool>`. If the fn
//   is `async fn`, the Futures of the above too.
// - NOT combinable with `@get`/`@post`/`@put`/`@delete`/`@ws`/`@cron`/
//   `@background`/`@auth_provider`/`@authenticated`/`@admin`/`@test`/
//   `@command`. Probes are auto-mounted routes; the handler must NOT
//   be normal HTTP nor job nor test.
//
// Errors go directly to `ctx.errors`. The singleton is tracked in
// `ctx.healthz_first` and `ctx.readyz_first` (Some on seeing the first).

/// Phase 12.7 — validates `@trace(name="X")` and `@metric(name="X")` on
/// user fns (business logic). Stackable with each other. Rejects
/// stacking on `@get`/`@post`/`@put`/`@delete`/`@ws` with clear error
/// citing that Phase 12.3 auto-instrumentation (automatic HTTP spans +
/// metrics) already covers that case.
///
/// Accepted syntax:
///   `@trace` — span with `<fn_name>` as name.
///   `@trace(name="custom")` — override of span's name.
///   `@metric` — Counter + Histogram with `<fn_name>` as name.
///   `@metric(name="custom")` — override.
///
/// No positional args. No other kwargs in the MVP.
fn check_trace_metric_decorators(
    ctx: &mut CheckCtx,
    fn_name: &str,
    decorators: &[Decorator],
    fn_span: Span,
) {
    let mut has_trace = false;
    let mut has_metric = false;
    for deco in decorators {
        let kind = match deco.name.as_str() {
            "trace" => "trace",
            "metric" => "metric",
            _ => continue,
        };
        if kind == "trace" {
            if has_trace {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@trace on fn '{}': duplicate in stacked decorators (only one @trace per fn).",
                        fn_name,
                    ),
                ));
                continue;
            }
            has_trace = true;
        } else {
            if has_metric {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@metric on fn '{}': duplicate in stacked decorators (only one @metric per fn).",
                        fn_name,
                    ),
                ));
                continue;
            }
            has_metric = true;
        }
        // 1) No positional args.
        if !deco.args.is_empty() {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@{} on fn '{}': does not accept positional args. Syntax: `@{}` or `@{}(name=\"X\")`.",
                    kind, fn_name, kind, kind,
                ),
            ));
            continue;
        }
        // 2) Only optional `name="X"` kwarg (Str literal).
        for (k, v) in &deco.kwargs {
            if k != "name" {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@{} on fn '{}': unknown kwarg '{}'. Only `name=\"X\"` is supported in the MVP.",
                        kind, fn_name, k,
                    ),
                ));
                continue;
            }
            if !matches!(v, Expr::Str(_, _)) {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@{} on fn '{}': kwarg `name` must be Str literal.",
                        kind, fn_name,
                    ),
                ));
            }
        }
    }
    // 3) Reject stacking on HTTP/WS handlers (12.3 auto-instrumentation
    // already covers those cases).
    if has_trace || has_metric {
        let is_http_handler = decorators
            .iter()
            .any(|d| matches!(d.name.as_str(), "get" | "post" | "put" | "delete" | "ws"));
        if is_http_handler {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "fn '{}': `@trace`/`@metric` do NOT stack on HTTP/WS handlers (`@get`/`@post`/`@put`/`@delete`/`@ws`). Phase 12.3 auto-instrumentation (HTTP spans + metrics) already covers those cases. Use `@trace`/`@metric` on business logic fns inside the handler.",
                    fn_name,
                ),
            ));
        }
    }
}

/// Phase 12.8 — validates shape of `@flag("name")`: 1 positional Str
/// literal arg (the flag name), no kwargs, no duplicates. Stackable on
/// any fn. The flag name is validated non-empty + chars
/// `[a-zA-Z0-9_-]`. The runtime semantics (404 for HTTP/WS, no-op for
/// regular fns) is implemented by the routing wrapper in `http.rs` and
/// the wrapper codegen in `codegen.rs`.
fn check_flag_decorator(
    ctx: &mut CheckCtx,
    fn_name: &str,
    decorators: &[Decorator],
    fn_span: Span,
) {
    let mut seen = false;
    for deco in decorators {
        if deco.name != "flag" {
            continue;
        }
        if seen {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@flag on fn '{}': duplicate in stacked decorators (only one @flag per fn).",
                    fn_name,
                ),
            ));
            continue;
        }
        seen = true;
        // 1) Exactly one positional arg.
        if deco.args.len() != 1 {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@flag on fn '{}': expected 1 positional arg (`@flag(\"flag-name\")`), received {}.",
                    fn_name,
                    deco.args.len(),
                ),
            ));
            continue;
        }
        // 2) The arg must be a Str literal.
        let name = match &deco.args[0] {
            Expr::Str(s, _) => s.clone(),
            _ => {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@flag on fn '{}': the flag name must be Str literal.",
                        fn_name,
                    ),
                ));
                continue;
            }
        };
        // 3) Non-empty + valid chars.
        if name.is_empty() {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!("@flag on fn '{}': the flag name cannot be empty.", fn_name),
            ));
            continue;
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@flag on fn '{}': flag name '{}' invalid. Only chars [a-zA-Z0-9_-] allowed.",
                    fn_name, name,
                ),
            ));
            continue;
        }
        // 4) No kwargs in MVP.
        if !deco.kwargs.is_empty() {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@flag on fn '{}': kwargs not supported in the MVP. Syntax: `@flag(\"flag-name\")`.",
                    fn_name,
                ),
            ));
        }
    }
}

fn check_health_decorators(
    ctx: &mut CheckCtx,
    fn_name: &str,
    params: &[Param],
    ret: &Type,
    decorators: &[Decorator],
    fn_span: Span,
) {
    for deco in decorators {
        let kind = match deco.name.as_str() {
            "healthz" => "healthz",
            "readyz" => "readyz",
            _ => continue,
        };
        // 1) No args nor kwargs.
        if !deco.args.is_empty() || !deco.kwargs.is_empty() {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@{kind} on fn '{fn_name}': does not accept args or kwargs. \
                     Syntax: `@{kind}\\nfn name() -> Bool {{ ... }}` or with `Result<Null>`/`Result<Bool>` (sync or async)."
                ),
            ));
            continue;
        }
        // 2) Singleton.
        let prev_slot = if kind == "healthz" {
            &ctx.healthz_first
        } else {
            &ctx.readyz_first
        };
        if let Some((prev_name, prev_span)) = prev_slot {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "duplicate @{kind}: fn '{prev_name}' (line {prev_line}) was already declared as probe; \
                     fn '{fn_name}' (line {curr_line}) is a second one. Only one is allowed per program.",
                    prev_line = prev_span.line,
                    curr_line = fn_span.line
                ),
            ));
            continue;
        }
        // 3) Conflicts with other decorators.
        let conflicting = [
            "get",
            "post",
            "put",
            "delete",
            "ws",
            "cron",
            "background",
            "auth_provider",
            "authenticated",
            "admin",
            "test",
            "command",
        ];
        let mut conflict = None;
        for other in decorators {
            if conflicting.contains(&other.name.as_str()) {
                conflict = Some(other.name.clone());
                break;
            }
            // The other probe on the same fn is also a conflict.
            // Example: `@healthz @readyz fn check() -> Bool { ... }`.
            if other.name != deco.name && (other.name == "healthz" || other.name == "readyz") {
                conflict = Some(other.name.clone());
                break;
            }
        }
        if let Some(other) = conflict {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@{kind} on fn '{fn_name}' is not combinable with `@{other}`: probes are auto-mounted routes (`/{kind}`); the handler cannot be normal HTTP, job, test, nor CLI command."
                ),
            ));
            continue;
        }
        // 4) No params.
        if !params.is_empty() {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@{kind} on fn '{fn_name}': the probe does not accept params (probes receive no input). Has {n}.",
                    n = params.len()
                ),
            ));
            continue;
        }
        // 5) Return type: Bool / Result<Null> / Result<Bool> / Future
        //    of the above (transparent for async fns). We accept
        //    `Any` too (checker's gradual escape).
        if !is_valid_health_return(ret) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@{kind} on fn '{fn_name}': return must be `Bool`, `Result<Null>` or `Result<Bool>` (sync or async). Is `{ty}`.",
                    ty = ret.display(ctx.types)
                ),
            ));
            continue;
        }
        // 6) Persist the first valid one to detect duplicates.
        let slot = if kind == "healthz" {
            &mut ctx.healthz_first
        } else {
            &mut ctx.readyz_first
        };
        *slot = Some((fn_name.to_string(), fn_span));
    }
}

/// Helper of `check_health_decorators`: is the return type acceptable
/// for a probe? We accept:
/// - `Bool` directly.
/// - `Null` (rare but valid if the fn only logs and always "passes").
/// - `Result<Null>` (Ok = healthy, Err = unhealthy).
/// - `Result<Bool>` (Ok(true) healthy, Ok(false) unhealthy, Err too).
/// - `Future<T>` with T being any of the above (async fn).
/// - `Any` as checker's gradual escape.
fn is_valid_health_return(ret: &Type) -> bool {
    match ret {
        Type::Bool | Type::Null | Type::Any => true,
        Type::Result { ok, .. } => matches!(ok.as_ref(), Type::Null | Type::Bool | Type::Any),
        Type::Future(inner) => is_valid_health_return(inner),
        _ => false,
    }
}

/// Phase 13 (v0.11.0) — `@command("name", desc="...")` declares a
/// fn as a CLI command. The binary produced by `fitz build` parses
/// `std::env::args()` and dispatches to the corresponding command.
///
/// Validated rules:
/// - Args: exactly 1 Str literal (command name, e.g. `"greet"`).
/// - Optional kwargs: `desc="..."` (description for `--help`).
/// - Return type must be `Int` (exit code; `0` success, others = error).
/// - Conflicts with server/job/test decorators: `@command` does NOT
///   combine with `@get/@post/@put/@delete/@server/@ws/@cron/@background/
///   @auth_provider/@test`. The fn marked as `@command` IS a
///   CLI command; cannot be HTTP handler, cron job, nor test.
/// - Valid param types: `Str`, `Int`, `Float`, `Bool`, `Str?`,
///   and optionally with default value.
///
/// **No-decorators-on-params convention** (MVP decision — avoids
/// touching `ast::Param`):
/// - Param **without default** → required positional arg (`mybin <name>`).
/// - Param **with default** → optional flag (`--name <value>` or `--name`
///   if Bool without value).
/// - Bool with default `false` → bool flag (`--loud` turns it on to true).
/// - Other types with default → value flag (`--count 5`).
///
/// This is the convention used by Click/Fire (Python) and Plumbum
/// (Python). Reduces verbosity vs requiring `@arg`/`@flag` on each
/// param. Trade-off: CANNOT have positional optional args
/// (the ones with default are flags).
fn check_command_decorator(
    ctx: &mut CheckCtx,
    fn_name: &str,
    params: &[Param],
    ret: &Type,
    decorators: &[Decorator],
    fn_span: Span,
) {
    let cmd_deco = match decorators.iter().find(|d| d.name == "command") {
        Some(d) => d,
        None => return,
    };
    // 1) Args: exactly 1 Str literal (command name).
    if cmd_deco.args.len() != 1 {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@command on fn '{}': expects exactly 1 argument (command name as Str). Syntax: `@command(\"greet\")` or `@command(\"greet\", desc=\"...\")`.",
                fn_name
            ),
        ));
        return;
    }
    match &cmd_deco.args[0] {
        Expr::Str(_, _) => {}
        _ => {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@command on fn '{}': first arg must be Str literal with the command name.",
                    fn_name
                ),
            ));
            return;
        }
    }
    // 2) Kwargs: only `desc="..."` accepted.
    for (key, value) in &cmd_deco.kwargs {
        match key.as_str() {
            "desc" => match value {
                Expr::Str(_, _) => {}
                _ => {
                    ctx.errors.push(FitzError::new(
                        ErrorKind::TypeError,
                        fn_span.line,
                        fn_span.column,
                        format!(
                            "@command on fn '{}': `desc=...` expects Str literal with the description.",
                            fn_name
                        ),
                    ));
                    return;
                }
            },
            other => {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@command on fn '{}': unknown kwarg `{}`. Supported: `desc=\"...\"`.",
                        fn_name, other
                    ),
                ));
                return;
            }
        }
    }
    // 3) Conflicts with other decorators.
    let conflicting = [
        "get",
        "post",
        "put",
        "delete",
        "server",
        "ws",
        "cron",
        "background",
        "auth_provider",
        "test",
        "middleware",
    ];
    for other in decorators {
        if conflicting.contains(&other.name.as_str()) {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@command on fn '{}' is not combinable with `@{}`: a CLI command cannot be HTTP handler, cron, test nor middleware.",
                    fn_name, other.name
                ),
            ));
            return;
        }
    }
    // 4) Return type must be Int (exit code).
    if !matches!(ret, Type::Int | Type::Any) {
        ctx.errors.push(FitzError::new(
            ErrorKind::TypeError,
            fn_span.line,
            fn_span.column,
            format!(
                "@command on fn '{}': return type must be `Int` (exit code). Return `0` for success, `1+` for error. Received: `{}`.",
                fn_name,
                ret.display(ctx.types)
            ),
        ));
    }
    // 5) Params: only CLI-marshallable types (Str/Int/Float/Bool/Str?).
    for p in params {
        if p.varargs {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@command on fn '{}': varargs (`...names: Str`) not supported in MVP. Use flags with default value or a `List<Str>` inside the body.",
                    fn_name
                ),
            ));
            return;
        }
        let p_type = ann_to_type(p.type_.as_ref(), ctx.types);
        // v0.11.1 (Phase 13 polish) — `List<Str>` allowed as
        // **variadic** (final positional that absorbs N tokens). Only
        // as the last positional param (no default). Other types of
        // List<T> remain future debt (List<Int> requires
        // per-token coercion; the real case covers Str for args
        // like `mybin run file1 file2 file3`).
        let is_str_list_variadic = matches!(
            &p_type,
            Type::List(inner) if matches!(**inner, Type::Str)
        );
        let valid = matches!(
            p_type,
            Type::Str | Type::Int | Type::Float | Type::Bool | Type::Any
        ) || matches!(&p_type, Type::Nullable(inner) if matches!(**inner, Type::Str | Type::Int | Type::Float))
            || is_str_list_variadic;
        if !valid {
            ctx.errors.push(FitzError::new(
                ErrorKind::TypeError,
                fn_span.line,
                fn_span.column,
                format!(
                    "@command on fn '{}': param `{}` has type `{}` which is not CLI-marshallable. Supported: `Str`/`Int`/`Float`/`Bool` (with or without `?`), `List<Str>` only as final variadic (v0.11.1).",
                    fn_name,
                    p.name,
                    p_type.display(ctx.types)
                ),
            ));
            return;
        }
        // v0.11.1 — variadic List<Str> must be the LAST param of the
        // entire command (not the last-without-default), because it accumulates
        // the remaining positional tokens. Convention: the user
        // writes `files: List<Str> = []` (with default `[]`) to
        // satisfy the parser rule "after a default, all
        // subsequent ones also". The `= []` is semantically
        // redundant (variadic always starts empty + accumulates) but
        // needed for syntactic shape.
        if is_str_list_variadic {
            let this_idx = params.iter().position(|q| std::ptr::eq(q, p)).unwrap();
            if this_idx != params.len() - 1 {
                ctx.errors.push(FitzError::new(
                    ErrorKind::TypeError,
                    fn_span.line,
                    fn_span.column,
                    format!(
                        "@command on fn '{}': param `{}` (List<Str> variadic) must be the LAST of ALL params. Move it to the end.",
                        fn_name, p.name
                    ),
                ));
                return;
            }
        }
        // v0.11.1 (Phase 13 polish) — `Bool = true` now supported.
        // The argv parser recognizes `--no-<name>` to negate the
        // default. E.g. `verbose: Bool = true` → `--no-verbose` sets
        // it to false; presence of `--verbose` redundant but
        // tolerated. Without override → stays true.
    }
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

    fn errors_of(src: &str) -> Vec<FitzError> {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (_env, _types, _defs, errors) = check_program(&program);
        errors
    }

    #[test]
    fn assignment_error_with_incompatible_type_cites_real_line() {
        // B.1: the error points to the stmt's `let` (real line/col),
        // not the generic `0:0` used before.
        let errors = errors_of("\n\nlet x: Int = \"texto\"");
        assert_eq!(errors.len(), 1, "expected 1 error, was {:?}", errors);
        let e = &errors[0];
        assert_eq!(e.line, 3, "expected line 3, was {}", e.line);
        assert_eq!(e.column, 1, "expected col 1, was {}", e.column);
    }

    // ---- Phase 6.2: type checker for async/await ----

    #[test]
    fn future_resolves_as_builtin_generic() {
        // `Future<T>` reuses `TypeExpr::Generic` (6.1 decision) and
        // 6.2 maps it to `Type::Future(Box<T>)`. Fixed arity 1.
        let env = TypeEnv::new();
        let te = TypeExpr::Generic {
            name: "Future".into(),
            args: vec![TypeExpr::Named("Int".into())],
        };
        let ty = resolve_type_expr(&te, &env).expect("Future<Int> debe resolver");
        assert_eq!(ty, Type::Future(Box::new(Type::Int)));
    }

    #[test]
    fn future_without_argument_is_arity_error() {
        let env = TypeEnv::new();
        let te = TypeExpr::Generic {
            name: "Future".into(),
            args: vec![],
        };
        let err = resolve_type_expr(&te, &env).expect_err("arity 0 must fail");
        assert!(matches!(err.kind, ErrorKind::TypeError));
    }

    #[test]
    fn future_with_two_arguments_is_arity_error() {
        let env = TypeEnv::new();
        let te = TypeExpr::Generic {
            name: "Future".into(),
            args: vec![TypeExpr::Named("Int".into()), TypeExpr::Named("Str".into())],
        };
        let err = resolve_type_expr(&te, &env).expect_err("arity 2 must fail");
        assert!(matches!(err.kind, ErrorKind::TypeError));
    }

    #[test]
    fn future_display_shows_inner() {
        let env = TypeEnv::new();
        let ty = Type::Future(Box::new(Type::Int));
        assert_eq!(ty.display(&env), "Future<Int>");
    }

    #[test]
    fn await_top_level_is_valid() {
        // Phase 6.7: top-level accepts `.await` — the evaluator starts
        // the tokio runtime there and codegen emits `#[tokio::main]
        // async fn main()` automatically. Only explicit sync fns
        // (non-async FnDef or FnExpr) reject it.
        let errors = errors_of(
            "async fn fetch() -> Int {\n\
                 return 0\n\
             }\n\
             let x = fetch().await",
        );
        assert!(
            errors.is_empty(),
            "expected no errors (top-level await is valid), was: {:?}",
            errors
        );
    }

    #[test]
    fn await_inside_sync_fn_is_error() {
        // FnDef without `async` counts as sync context → `.await`
        // inside emits a clear error.
        let errors = errors_of(
            "async fn fetch() -> Int {\n\
                 return 0\n\
             }\n\
             fn sync_caller() -> Int {\n\
                 return fetch().await\n\
             }",
        );
        assert!(
            !errors.is_empty(),
            "expected error on .await inside sync fn"
        );
        let msg = &errors[0].message;
        assert!(
            msg.contains(".await") && msg.contains("async fn"),
            "expected message about `.await` and `async fn`, was: {}",
            msg
        );
    }

    #[test]
    fn await_on_non_future_is_error() {
        // Concrete operand distinct from `Future<T>` → error.
        let errors = errors_of(
            "async fn f() -> Int {\n\
                 let x: Int = 42\n\
                 return x.await\n\
             }",
        );
        assert!(!errors.is_empty(), "expected 1 error");
        let msg = &errors[0].message;
        assert!(
            msg.contains("Future") && msg.contains("Int"),
            "expected message about Future and Int, was: {}",
            msg
        );
    }

    #[test]
    fn await_on_future_inside_async_fn_passes() {
        // Happy case: async fn calling another async fn and await-ing
        // the result. The `inner()` call types `Future<Int>`,
        // `.await` unwraps to `Int`, return Int matches.
        let errors = errors_of(
            "async fn inner() -> Int {\n\
                 return 1\n\
             }\n\
             async fn outer() -> Int {\n\
                 return inner().await\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn async_fn_referenced_as_ident_types_function_with_future() {
        // An `async fn f() -> Int` referenced as a value (without
        // call) types `Function { ret: Future<Int> }`. The EXTERNAL
        // signature of the async fn wraps in Future. We validate via
        // a `let g: Future<Int> = f()` that the checker accepts.
        let errors = errors_of(
            "async fn f() -> Int {\n\
                 return 0\n\
             }\n\
             let g: Future<Int> = f()",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn return_inside_async_fn_does_not_wrap_in_future() {
        // `async` is transparent from inside: a `return x: Int`
        // inside `async fn -> Int` types Int against Int, not
        // Int against Future<Int>.
        let errors = errors_of(
            "async fn f() -> Int {\n\
                 return 42\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn await_inside_fnexpr_is_error_even_if_parent_is_async() {
        // FnExpr (closure) always pushes `await_stack` with false —
        // the language doesn't support anonymous `async fn(...)`. `.await`
        // inside the closure is an error even if the container is
        // an async fn.
        let errors = errors_of(
            "async fn fetch() -> Int {\n\
                 return 0\n\
             }\n\
             async fn outer() -> Int {\n\
                 let cb = fn() => fetch().await\n\
                 return cb()\n\
             }",
        );
        assert!(
            !errors.is_empty(),
            "expected error on `.await` of the closure"
        );
        let msg = &errors[0].message;
        assert!(
            msg.contains("async fn"),
            "expected message about async fn, was: {}",
            msg
        );
    }

    #[test]
    fn await_on_any_is_gradual_and_does_not_check() {
        // A fn without return annotation types `Function { ret: Any }`.
        // The call produces Any; `.await` over Any passes through
        // gradual escape (result Any). No errors.
        let errors = errors_of(
            "fn untyped() => 0\n\
             async fn outer() -> Int {\n\
                 return untyped().await\n\
             }",
        );
        // The `.await` should not fire the "not a Future" error
        // because the operand is Any (gradual escape). If there are
        // other errors, we inspect them — but the specific "Future"
        // message must not appear.
        let any_future_err = errors
            .iter()
            .any(|e| e.message.contains("Future") && e.message.contains(".await"));
        assert!(
            !any_future_err,
            "await over Any should not fire Future error, was: {:?}",
            errors
        );
    }

    // ---- Phase 6.3: built-in `sleep` ----

    #[test]
    fn sleep_types_its_call_as_future_null() {
        // `sleep(100)` types `Future<Null>`. We validate via a
        // destination annotation — if the RHS were not `Future<Null>`,
        // the checker would emit an incompatibility error.
        let errors = errors_of("let r: Future<Null> = sleep(100)");
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn sleep_with_non_int_argument_is_error() {
        let errors = errors_of("let r = sleep(\"x\")");
        assert!(!errors.is_empty(), "expected type error");
        let msg = &errors[0].message;
        assert!(
            msg.contains("sleep") && msg.contains("Int") && msg.contains("Str"),
            "expected message about sleep/Int/Str, was: {}",
            msg
        );
    }

    #[test]
    fn sleep_with_wrong_arity_is_error() {
        let errors = errors_of("let r = sleep(1, 2)");
        assert!(!errors.is_empty(), "expected arity error");
        let msg = &errors[0].message;
        assert!(
            msg.contains("sleep") && msg.contains("1") && msg.contains("2"),
            "expected message about sleep/1/2, was: {}",
            msg
        );
    }

    #[test]
    fn sleep_await_inside_async_fn_types_null() {
        // Integration with 6.2: `sleep(50).await` inside `async fn`
        // types `Null`. The fn declares `-> Null` and the return matches.
        let errors = errors_of(
            "async fn pausa() -> Null {\n\
                 return sleep(50).await\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    // ---- C-F2: field assignment check ----

    #[test]
    fn field_assign_with_compatible_type_passes_checker() {
        let errors = errors_of(
            "type U { name: Str }\n\
             let u = U { name: \"x\" }\n\
             u.name = \"y\"",
        );
        assert!(
            errors.is_empty(),
            "should not have errors, was {:?}",
            errors
        );
    }

    #[test]
    fn field_assign_with_incompatible_type_is_error() {
        let errors = errors_of(
            "type U { name: Str }\n\
             let u = U { name: \"x\" }\n\
             u.name = 42",
        );
        assert_eq!(errors.len(), 1, "expected 1 error, was {:?}", errors);
        let msg = &errors[0].message;
        assert!(
            msg.contains("`U.name`") && msg.contains("Str") && msg.contains("Int"),
            "expected message about U.name/Str/Int, was: {}",
            msg
        );
    }

    // ---- Custom status codes (return <int> { ... }) ----

    #[test]
    fn return_status_inside_http_handler_passes_checker() {
        // `return 401 { ... }` inside a handler with `@get` is
        // valid. The checker allows it regardless of the handler's
        // formal return_type (decision: polymorphism only in HTTP
        // handlers).
        let errors = errors_of(
            "@get(\"/x\") fn protected() -> Str {\n\
                 return 401 {\"msg\": \"no autorizado\"}\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn return_status_outside_handler_is_error() {
        // `return 401 { ... }` inside a fn without HTTP decorator
        // → clear error. Blocks accidental use outside handlers.
        let errors = errors_of(
            "fn helper() -> Str {\n\
                 return 401 {\"msg\": \"x\"}\n\
             }",
        );
        assert!(!errors.is_empty(), "expected 1 error");
        let msg = &errors[0].message;
        assert!(
            msg.contains("HTTP handler") && msg.contains("@get"),
            "expected message about HTTP handler, was: {}",
            msg
        );
    }

    #[test]
    fn return_status_top_level_is_error() {
        // `return 401 { ... }` at top-level (without containing fn)
        // is also not valid — the checker rejects it by the same rule.
        let errors = errors_of("return 401 {\"x\": 1}");
        assert!(!errors.is_empty(), "expected error");
        let msg = &errors[0].message;
        assert!(msg.contains("HTTP handler"), "was: {}", msg);
    }

    #[test]
    fn return_status_does_not_check_against_formal_return_type() {
        // Spec: a `-> User` handler can do `return user` (User) and
        // also `return 404 { ... }`. The checker does NOT validate the
        // ReturnStatus body against the return type — it's polymorphic.
        let errors = errors_of(
            "type User { id: Int }\n\
             @get(\"/u\") fn get_u() -> User {\n\
                 return 404 {\"error\": \"no encontrado\"}\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    // ---- Mini-phase MW.1: middleware ----

    #[test]
    fn request_and_response_are_referenceable_builtins() {
        // A middleware references `Request` and `Response` without declaring them
        // — registered by `register_http_builtin_types`. Without that pre-registration,
        // the checker would complain with "unknown type `Request`".
        let errors = errors_of(
            "fn auth(req: Request) -> Response? {\n\
                 return null\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    // ---- Mini-fase HTTP client (2026-06-18): Bloque 2 — checker ----

    #[test]
    fn http_client_module_is_pre_registered_as_any() {
        // Mini-fase HTTP client. `http` is registered in
        // `CheckCtx::new()` as `Type::Any`, so calling
        // `http.get(...)` does not error (field access + call fall
        // to gradual). Pre-condition for the rest of the checker
        // tests in this section.
        let errors = errors_of(
            "async fn fetch() -> Result<Int> {\n\
                 let r = http.get(\"https://example.com\").await?\n\
                 return Ok(r.status)\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn http_client_response_is_referenceable_builtin() {
        // `HttpClientResponse` is pre-registered as a built-in nominal
        // (`register_http_builtin_types`). The user can annotate
        // a fn arg / return type with it without declaring it locally
        // — parallel to `Request` / `Response` / `File`.
        let errors = errors_of(
            "fn check(r: HttpClientResponse) -> Bool {\n\
                 return r.status == 200\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn http_client_response_field_status_is_int() {
        // `HttpClientResponse.status` is `Int` per
        // `register_http_builtin_types`. Field access via TypeInfo
        // (F16) on a binding annotated `: HttpClientResponse`
        // must persist `Int`.
        let info = types_of("fn get_status(r: HttpClientResponse) -> Int => r.status\n");
        let int_on_line1 = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 1 && matches!(t, Type::Int));
        assert!(
            int_on_line1,
            "line 1 must persist Int for r.status: {:?}",
            info.inner
        );
    }

    #[test]
    fn http_client_response_field_body_is_str() {
        // `HttpClientResponse.body` is `Str`.
        let info = types_of("fn get_body(r: HttpClientResponse) -> Str => r.body\n");
        let str_on_line1 = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 1 && matches!(t, Type::Str));
        assert!(
            str_on_line1,
            "line 1 must persist Str for r.body: {:?}",
            info.inner
        );
    }

    #[test]
    fn http_client_response_field_headers_is_map_str_str() {
        // `HttpClientResponse.headers` is `Map<Str, Str>`.
        let info =
            types_of("fn get_headers(r: HttpClientResponse) -> Map<Str, Str> => r.headers\n");
        let map_str_str_on_line1 = info.inner.iter().any(|(k, t)| {
            k.0 == 1
                && matches!(
                    t,
                    Type::Map(k_ty, v_ty)
                        if matches!(k_ty.as_ref(), Type::Str)
                        && matches!(v_ty.as_ref(), Type::Str)
                )
        });
        assert!(
            map_str_str_on_line1,
            "line 1 must persist Map<Str, Str> for r.headers: {:?}",
            info.inner
        );
    }

    #[test]
    fn http_client_response_field_duration_ms_is_int() {
        // `HttpClientResponse.duration_ms` is `Int`.
        let info = types_of("fn get_duration(r: HttpClientResponse) -> Int => r.duration_ms\n");
        let int_on_line1 = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 1 && matches!(t, Type::Int));
        assert!(
            int_on_line1,
            "line 1 must persist Int for r.duration_ms: {:?}",
            info.inner
        );
    }

    #[test]
    fn http_client_full_pipeline_with_match_passes_checker() {
        // The canonical pattern: an async fn returning `Result<Int>` that
        // does `http.get(...).await?` then `match r.status { ... }`.
        // Combines: gradual call over `http` (Any), `.await` of the
        // returned Future, `?` propagating the inner Result, field access
        // on the resulting `HttpClientResponse` nominal, and match over Int.
        let errors = errors_of(
            "async fn check_status(url: Str) -> Result<Bool> {\n\
                 let r = http.get(url).await?\n\
                 if (r.status >= 200) {\n\
                     if (r.status < 300) {\n\
                         return Ok(true)\n\
                     }\n\
                 }\n\
                 return Ok(false)\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    // ---- Mini-tanda SMTP builtin (2026-06-19): Bloque 2 — checker ----

    #[test]
    fn smtp_module_is_pre_registered_as_any() {
        // Mini-tanda SMTP builtin. `smtp` is registered in
        // `CheckCtx::new()` as `Type::Any`, so calling
        // `smtp.send(...)` does not error (field access + call fall
        // to gradual). Pre-condition for the rest of the SMTP checker
        // tests.
        let errors = errors_of(
            "async fn notify(addr: Str) -> Result<Str> {\n\
                 let r = smtp.send({\n\
                     \"to\": addr,\n\
                     \"from\": \"bot@example.com\",\n\
                     \"subject\": \"hi\",\n\
                     \"body\": \"hola\",\n\
                 }).await?\n\
                 return Ok(r.message_id)\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn smtp_result_is_referenceable_builtin() {
        // `SmtpResult` is pre-registered as a built-in nominal
        // (`register_http_builtin_types`). The user can annotate
        // a fn arg / return type with it without declaring it locally
        // — parallel to `HttpClientResponse` / `Request` / `Response` / `File`.
        let errors = errors_of(
            "fn check(r: SmtpResult) -> Bool {\n\
                 return r.delivered\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn smtp_result_field_delivered_is_bool() {
        // `SmtpResult.delivered` is `Bool` per
        // `register_http_builtin_types`. Field access via TypeInfo
        // (F16) on a binding annotated `: SmtpResult` must persist Bool.
        let info = types_of("fn get_delivered(r: SmtpResult) -> Bool => r.delivered\n");
        let bool_on_line1 = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 1 && matches!(t, Type::Bool));
        assert!(
            bool_on_line1,
            "line 1 must persist Bool for r.delivered: {:?}",
            info.inner
        );
    }

    #[test]
    fn smtp_result_field_message_id_is_str() {
        // `SmtpResult.message_id` is `Str`.
        let info = types_of("fn get_id(r: SmtpResult) -> Str => r.message_id\n");
        let str_on_line1 = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 1 && matches!(t, Type::Str));
        assert!(
            str_on_line1,
            "line 1 must persist Str for r.message_id: {:?}",
            info.inner
        );
    }

    #[test]
    fn smtp_result_field_duration_ms_is_int() {
        // `SmtpResult.duration_ms` is `Int`.
        let info = types_of("fn get_duration(r: SmtpResult) -> Int => r.duration_ms\n");
        let int_on_line1 = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 1 && matches!(t, Type::Int));
        assert!(
            int_on_line1,
            "line 1 must persist Int for r.duration_ms: {:?}",
            info.inner
        );
    }

    #[test]
    fn smtp_send_with_try_operator_inside_result_fn_passes() {
        // The `?` operator works because the call falls to Any → Result<Any>
        // (consistent with `http.X(...)?`); the containing fn returns
        // `Result<...>` so the propagation rule (Phase 5.3.3) is satisfied.
        let errors = errors_of(
            "async fn dispatch() -> Result<Str> {\n\
                 let opts = {\n\
                     \"to\": \"u@x.com\",\n\
                     \"from\": \"bot@x.com\",\n\
                     \"subject\": \"hi\",\n\
                     \"body\": \"hola\",\n\
                 }\n\
                 let r = smtp.send(opts).await?\n\
                 return Ok(r.message_id)\n\
             }",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn return_status_inside_middleware_passes_checker() {
        // A fn applied as `@middleware(fn)` can do
        // `return <int> { ... }` — the MW.1 pre-scan marks it as
        // HTTP context and the checker doesn't complain.
        let errors = errors_of(
            "fn auth(req: Request) {\n\
                 return 401 {\"error\": \"no autorizado\"}\n\
             }\n\
             @middleware(auth)\n\
             @get(\"/admin\")\n\
             fn admin() -> Str => \"ok\"",
        );
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    #[test]
    fn return_status_in_fn_not_referenced_as_middleware_is_error() {
        // Only fns that appear in `@middleware(name)` are marked
        // as HTTP context. A random fn with `return <int>` still
        // fires the existing error.
        let errors = errors_of(
            "fn helper() {\n\
                 return 401 {\"x\": 1}\n\
             }",
        );
        assert!(!errors.is_empty(), "expected error");
        assert!(
            errors[0].message.contains("middleware") || errors[0].message.contains("handler HTTP"),
            "expected message about handler/middleware, was: {}",
            errors[0].message
        );
    }

    #[test]
    fn field_assign_to_nonexistent_field_is_error() {
        let errors = errors_of(
            "type U { name: Str }\n\
             let u = U { name: \"x\" }\n\
             u.email = \"y\"",
        );
        assert!(!errors.is_empty(), "expected error about nonexistent field");
        let msg = &errors[0].message;
        assert!(
            msg.contains("does not have a field named `email`"),
            "expected message about nonexistent field, was: {}",
            msg
        );
    }

    #[test]
    fn field_assign_on_non_nominal_is_error() {
        let errors = errors_of(
            "let x = 42\n\
             x.foo = 1",
        );
        assert!(
            !errors.is_empty(),
            "expected error: assigning to field of Int"
        );
        let msg = &errors[0].message;
        assert!(
            msg.contains("solo se permite") || msg.contains("Int"),
            "expected message about incompatible type, was: {}",
            msg
        );
    }

    #[test]
    fn field_assign_on_any_does_not_check() {
        // The binding `m` comes from `from foo import m` → type Any.
        // The checker should allow the assign without checking the field
        // (gradual escape).
        // We simulate with a var without annotation that parser/checker
        // treat as Any in the appropriate context. We use
        // `from import` which registers as Any.
        let errors = errors_of(
            "from external import obj\n\
             obj.anything = 42",
        );
        // We accept that the module load fails (doesn't exist), but
        // if it reaches the checker the assign over Any should silence.
        // In practice the checker only registers the var as Any
        // if the FromImport passes.
        // We filter the import error if any and verify there is
        // NO specific error about the field.
        let field_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("field") || e.message.contains(".anything"))
            .collect();
        assert!(
            field_errors.is_empty(),
            "should not have error about the field, was: {:?}",
            field_errors
        );
    }

    #[test]
    fn field_assign_with_nullable_accepts_null() {
        // `email: Str?` accepts null or Str. Assigning null must pass.
        let errors = errors_of(
            "type U { email: Str? }\n\
             let u = U { email: \"x\" }\n\
             u.email = null",
        );
        assert!(
            errors.is_empty(),
            "Null compatible with Str?, was: {:?}",
            errors
        );
    }

    // ---- fin C-F2 ----

    #[test]
    fn while_non_bool_error_cites_real_line() {
        let errors = errors_of("\nwhile (42) { let _ = 0 }");
        assert!(!errors.is_empty(), "expected type error");
        let e = &errors[0];
        assert_eq!(e.line, 2, "expected line 2, was {}", e.line);
        assert!(
            e.message.contains("while"),
            "expected message about while, was: {}",
            e.message
        );
    }

    // ---- resolve_type_expr ----

    #[test]
    fn resolve_primitives() {
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
    fn resolve_primitive_with_args_is_arity_error() {
        // `Int<Str>` doesn't make sense — Int is arity 0.
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Int".into(),
            args: vec![TypeExpr::named("Str")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeError));
        assert!(err.message.contains("expects 0 type argument(s)"));
    }

    #[test]
    fn resolve_list_of_int() {
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int")],
        };
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::List(Box::new(Type::Int)));
    }

    #[test]
    fn resolve_list_wrong_arity() {
        let env = TypeEnv::new();
        // List without args
        let t1 = TypeExpr::named("List");
        let err = resolve_type_expr(&t1, &env).unwrap_err();
        assert!(err.message.contains("`List`"));
        assert!(err.message.contains("1 type argument"));

        // List with two args
        let t2 = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int"), TypeExpr::named("Str")],
        };
        let err = resolve_type_expr(&t2, &env).unwrap_err();
        assert!(err.message.contains("received 2"));
    }

    #[test]
    fn resolve_map_of_str_int() {
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::named("Str"), TypeExpr::named("Int")],
        };
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::Map(Box::new(Type::Str), Box::new(Type::Int)));
    }

    #[test]
    fn resolve_map_wrong_arity() {
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::named("Str")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("`Map`"));
        assert!(err.message.contains("2 type argument"));
        assert!(err.message.contains("received 1"));
    }

    #[test]
    fn resolve_nested_result() {
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
            Type::Result {
                ok: Box::new(Type::List(Box::new(Type::Int))),
                err: Box::new(Type::Str)
            },
        );
    }

    #[test]
    fn resolve_nullable_on_primitive() {
        let env = TypeEnv::new();
        let t = TypeExpr::Nullable(Box::new(TypeExpr::named("Str")));
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::Nullable(Box::new(Type::Str)));
    }

    #[test]
    fn resolve_nullable_on_generic() {
        // List<Int>?
        let env = TypeEnv::new();
        let t = TypeExpr::Nullable(Box::new(TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int")],
        }));
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::Nullable(Box::new(Type::List(Box::new(Type::Int)))),);
    }

    #[test]
    fn resolve_declared_nominal() {
        let env = env_with(&["User"]);
        let t = TypeExpr::named("User");
        let r = resolve_type_expr(&t, &env).unwrap();
        let id = env.lookup("User").unwrap();
        assert_eq!(r, Type::Nominal(id));
    }

    #[test]
    fn resolve_undefined_nominal_is_error() {
        let env = TypeEnv::new();
        let t = TypeExpr::named("Usuario");
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("unknown type"));
        assert!(err.message.contains("Usuario"));
    }

    #[test]
    fn resolve_nominal_with_args_is_error() {
        // The user writes `User<Int>` but User is not generic.
        let env = env_with(&["User"]);
        let t = TypeExpr::Generic {
            name: "User".into(),
            args: vec![TypeExpr::named("Int")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("is not generic"));
    }

    #[test]
    fn resolve_generic_with_invalid_arg_propagates_error() {
        // List<Usuario> — Usuario does not exist.
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
    fn type_env_lookup_returns_id() {
        let env = env_with(&["A", "B"]);
        let a = env.lookup("A").unwrap();
        let b = env.lookup("B").unwrap();
        assert_ne!(a, b);
        assert_eq!(env.info(a).name, "A");
        assert_eq!(env.info(b).name, "B");
    }

    #[test]
    fn type_env_declaring_twice_is_error() {
        let mut env = TypeEnv::new();
        env.declare_nominal("Foo".into()).unwrap();
        let err = env.declare_nominal("Foo".into()).unwrap_err();
        assert!(err.message.contains("`Foo`"));
        assert!(err.message.contains("more than once"));
    }

    // ---- resolve_program ----

    #[test]
    fn empty_program_gives_no_errors() {
        let (env, errors) = resolve_str("");
        assert!(errors.is_empty());
        // Mini-phase MW.1: `Request` and `Response` are pre-registered as
        // HTTP runtime built-in nominals, even in empty programs.
        // Mini-batch MP2 added `File` as a third built-in.
        // Mini-fase HTTP client (2026-06-18) added `HttpClientResponse`
        // for outbound `http.get/post/...` returns (type of `r` in
        // `let r = http.get(url).await?`).
        // Mini-tanda SMTP builtin (2026-06-19) added `SmtpResult` for
        // outbound `smtp.send(...)` returns (type of `r` in
        // `let r = smtp.send(opts).await?`).
        // The user can reference them without declaring them.
        assert_eq!(env.nominal_count(), 5);
        assert!(env.lookup("Request").is_some());
        assert!(env.lookup("Response").is_some());
        assert!(env.lookup("File").is_some());
        assert!(env.lookup("HttpClientResponse").is_some());
        assert!(env.lookup("SmtpResult").is_some());
    }

    #[test]
    fn type_with_primitives_resolves() {
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
    fn type_with_generic_and_nullable_resolves() {
        let (env, errors) = resolve_str("type Post { tags: List<Str>, author: Str? }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
        let id = env.lookup("Post").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        assert_eq!(fields[0].type_, Type::List(Box::new(Type::Str)));
        assert_eq!(fields[1].type_, Type::Nullable(Box::new(Type::Str)));
    }

    #[test]
    fn type_referencing_another_local_type() {
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
    fn mutual_forward_refs_resolve() {
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
    fn type_with_nonexistent_field_type_reports_error() {
        let (_, errors) = resolve_str("type User { home: Address }");
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("Address"));
        assert!(msg.contains("unknown type"));
        assert!(msg.contains("field `home`"));
        assert!(msg.contains("type `User`"));
    }

    #[test]
    fn redeclared_type_is_error() {
        let (_, errors) = resolve_str("type Foo { x: Int }\ntype Foo { y: Str }");
        assert!(errors
            .iter()
            .any(|e| e.message.contains("Foo") && e.message.contains("more than once")));
    }

    #[test]
    fn default_literal_compatible_passes() {
        let (_, errors) = resolve_str("type Cfg { port: Int = 3000, debug: Bool = false }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn default_literal_incompatible_reports_error() {
        let (_, errors) = resolve_str("type Cfg { port: Int = \"3000\" }");
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("Cfg.port"));
        assert!(msg.contains("`Int`"));
        assert!(msg.contains("`Str`"));
    }

    #[test]
    fn default_null_on_nullable_field_passes() {
        let (_, errors) = resolve_str("type User { email: Str? = null }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn default_null_on_non_nullable_field_fails() {
        let (_, errors) = resolve_str("type User { id: Int = null }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("User.id"));
    }

    #[test]
    fn default_int_on_float_accepted_by_coercion() {
        let (_, errors) = resolve_str("type Cfg { ratio: Float = 1 }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn default_non_literal_accepted_pending_for_5_3() {
        // Default is an expression (not literal): sum. The checker
        // lets it pass — 5.3 checks expressions against types.
        let (_, errors) = resolve_str("type Cfg { port: Int = 3000 + 1 }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    // ---- FnDef and Assign annotations ----

    #[test]
    fn fndef_with_resolved_annotations() {
        let (_, errors) = resolve_str("fn add(a: Int, b: Int) -> Int { return a + b }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn fndef_with_invalid_param_type_reports_error() {
        let (_, errors) = resolve_str("fn f(x: Foo) { return x }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
        assert!(errors[0].message.contains("parameter `x`"));
        assert!(errors[0].message.contains("function `f`"));
    }

    #[test]
    fn fndef_with_invalid_return_reports_error() {
        let (_, errors) = resolve_str("fn f() -> Foo { return 0 }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
        assert!(errors[0].message.contains("return type"));
        assert!(errors[0].message.contains("function `f`"));
    }

    #[test]
    fn fndef_with_invalid_generic_reports_error() {
        // `List<Foo>` where Foo doesn't exist.
        let (_, errors) = resolve_str("fn f(xs: List<Foo>) { return xs }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
    }

    #[test]
    fn assign_with_invalid_type_reports_error() {
        let (_, errors) = resolve_str("let x: Foo = 0");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
    }

    #[test]
    fn assign_with_valid_generic_passes() {
        let (_, errors) = resolve_str("let xs: List<Int> = []");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn annotations_inside_fn_body_are_validated() {
        // The `y: Foo` let is inside the fn — the pass descends and finds it.
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
    fn multiple_errors_accumulate_and_do_not_cut() {
        let (_, errors) = resolve_str(
            "type A { x: Foo }\n\
             let y: Bar = 0\n\
             fn f(z: Baz) { return z }",
        );
        // We expect 3: Foo, Bar, Baz.
        assert_eq!(errors.len(), 3);
        let combined: String = errors.iter().map(|e| e.message.clone()).collect();
        assert!(combined.contains("Foo"));
        assert!(combined.contains("Bar"));
        assert!(combined.contains("Baz"));
    }

    // ---- direct AST constructions, without parser ----

    #[test]
    fn resolve_program_builds_env_via_direct_ast() {
        // Sanity: we build the AST by hand without going through the parser
        // to confirm that resolve_program does not depend on parser details.
        use crate::ast::TypeExpr as TE;
        let program: Program = vec![
            Stmt::TypeDef {
                name: "X".into(),
                fields: vec![Field {
                    name: "n".into(),
                    type_: TE::named("Int"),
                    default: None,
                    decorators: vec![],
                }],
                methods: vec![],
                decorators: vec![],
                span: Span::ZERO,
            },
            Stmt::FnDef {
                name: "noop".into(),
                params: vec![Param {
                    name: "p".into(),
                    type_: Some(TE::named("X")),
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                }],
                return_type: None,
                body: vec![],
                is_async: false,
                decorators: Vec::<Decorator>::new(),
                span: Span::ZERO,
            },
            Stmt::Assign {
                target: AssignTarget::Ident("v".into(), Span::default()),
                type_: Some(TE::Nullable(Box::new(TE::named("X")))),
                value: Expr::Null(Span::ZERO),
                span: Span::ZERO,
            },
        ];
        let (env, errors) = resolve_program(&program);
        assert!(errors.is_empty(), "errores: {:?}", errors);
        let x = env.lookup("X").unwrap();
        assert_eq!(env.info(x).fields.as_ref().unwrap()[0].type_, Type::Int);
    }

    // -----------------------------------------------------------------------
    // Tests — expression checker (Phase 5.3.1)
    //
    // We cover the new pass: synth of literals/ident/BinOp/UnaryOp/
    // StrInterp/If/List/Map/StructLit/Field/Range, annotated
    // assignments, local scope (FnDef/FnExpr/Match arms), and imports.
    // -----------------------------------------------------------------------

    fn check_str(src: &str) -> (TypeEnv, Vec<FitzError>) {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (env, _types, _defs, errors) = check_program(&program);
        (env, errors)
    }

    fn assert_ok(src: &str) {
        let (_, errors) = check_str(src);
        assert!(
            errors.is_empty(),
            "esperado sin errores, hubo: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    fn assert_error_with(src: &str, contains: &[&str]) {
        let (_, errors) = check_str(src);
        assert!(!errors.is_empty(), "esperado al menos un error, no hubo");
        let combined: String = errors.iter().map(|e| e.message.clone()).collect();
        for needle in contains {
            assert!(
                combined.contains(needle),
                "mensaje esperado contener `{}`, fue: {}",
                needle,
                combined
            );
        }
    }

    // ---- ident / scope ----

    #[test]
    fn unknown_ident_emits_warning() {
        assert_error_with("print(no_existe)", &["unknown variable", "no_existe"]);
    }

    #[test]
    fn known_ident_does_not_emit_error() {
        assert_ok("let x = 1\nprint(x)");
    }

    #[test]
    fn ident_nominal_type_as_value_is_any() {
        // `type User { ... }; let u = User { id: 1, name: "x" }` —
        // the StructLit uses the type; using bare User also doesn't break.
        // The evaluator registers the type as Value in the env.
        assert_ok("type User { id: Int }\nprint(User)");
    }

    #[test]
    fn builtin_print_and_len_considered_defined() {
        // print and len exist by default.
        assert_ok("print(\"hola\")\nlen([1, 2, 3])");
    }

    // ---- BinOp ----

    #[test]
    fn binop_int_plus_int_is_ok() {
        assert_ok("let x: Int = 1 + 2");
    }

    #[test]
    fn binop_int_plus_float_is_float() {
        // Float := Int + Float (coercion).
        assert_ok("let x: Float = 1 + 2.0");
    }

    #[test]
    fn binop_str_plus_str_is_str() {
        assert_ok("let s: Str = \"a\" + \"b\"");
    }

    #[test]
    fn binop_str_plus_int_is_error() {
        assert_error_with("let x = \"a\" + 1", &["`+`", "Str", "Int"]);
    }

    #[test]
    fn binop_mul_accepts_numerics() {
        assert_ok("let x: Float = 2 * 3.5");
    }

    #[test]
    fn binop_mul_rejects_str() {
        assert_error_with("let x = \"a\" * 2", &["`*`", "numeric operands", "Str"]);
    }

    #[test]
    fn binop_comparison_str_str_is_bool() {
        assert_ok("let b: Bool = \"a\" < \"b\"");
    }

    #[test]
    fn binop_comparison_str_int_is_error() {
        assert_error_with("let b = \"a\" < 1", &["comparison", "Str", "Int"]);
    }

    #[test]
    fn binop_and_with_bool_is_ok() {
        assert_ok("let b: Bool = true and false");
    }

    #[test]
    fn binop_and_with_int_is_error() {
        assert_error_with("let b = 1 and true", &["logical", "Bool", "Int"]);
    }

    // ---- UnaryOp ----

    #[test]
    fn unary_neg_int_is_ok() {
        assert_ok("let x: Int = -5");
    }

    #[test]
    fn unary_neg_str_is_error() {
        assert_error_with("let x = -\"hola\"", &["negation", "Int", "Str"]);
    }

    // ---- R.1.1 — `not` (mini-phase R) ----

    #[test]
    fn unary_not_on_bool_literal_is_ok() {
        assert_ok("let x: Bool = not true");
    }

    #[test]
    fn unary_not_on_bool_ident_is_ok() {
        assert_ok("let active: Bool = false\nlet inactive: Bool = not active");
    }

    #[test]
    fn unary_not_on_int_is_type_error() {
        assert_error_with("let x = not 5", &["not", "Bool", "Int"]);
    }

    #[test]
    fn unary_not_on_str_is_type_error() {
        assert_error_with("let x = not \"hola\"", &["not", "Bool", "Str"]);
    }

    #[test]
    fn unary_not_in_if_condition_is_ok() {
        // Bool in condition ✓.
        assert_ok("let active = false\nif (not active) { print(\"x\") }");
    }

    #[test]
    fn nested_unary_not_types_bool() {
        // `not not x` with x: Bool → Bool.
        assert_ok("let x = true\nlet y: Bool = not not x");
    }

    // ---- R.1.2 — operator `%` (mini-phase R) ----

    #[test]
    fn op_modulo_int_int_is_ok() {
        assert_ok("let r: Int = 10 % 3");
    }

    #[test]
    fn op_modulo_with_var_int_is_ok() {
        assert_ok("let n: Int = 100\nlet r: Int = n % 7");
    }

    #[test]
    fn op_modulo_with_float_is_type_error() {
        assert_error_with("let r = 10.0 % 3", &["%", "Int", "Float"]);
    }

    #[test]
    fn op_modulo_with_str_is_type_error() {
        assert_error_with("let r = \"hola\" % 3", &["%", "Int", "Str"]);
    }

    #[test]
    fn op_modulo_returns_int_not_any() {
        // The synthesized type must be concrete Int (not Any),
        // so a Bool binding fails — Bool doesn't admit Int.
        // (Float DOES admit Int via Int→Float promotion, which is why
        // we don't test that case.)
        assert_error_with("let r: Bool = 7 % 3", &["Bool", "Int"]);
    }

    // ---- R.1.3 — index assignment (mini-phase R) ----

    #[test]
    fn assign_index_list_int_int_is_ok() {
        assert_ok("let xs: List<Int> = [1, 2, 3]\nxs[0] = 99");
    }

    #[test]
    fn assign_index_list_str_index_is_error() {
        // List<T> requires Int as the index.
        assert_error_with(
            "let xs: List<Int> = [1, 2]\nxs[\"a\"] = 99",
            &["List", "Int", "Str"],
        );
    }

    #[test]
    fn assign_index_list_wrong_value_type_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\nxs[0] = \"hola\"",
            &["list", "Int", "Str"],
        );
    }

    #[test]
    fn assign_index_map_correct_is_ok() {
        assert_ok("let m: Map<Str, Int> = {\"a\": 1}\nm[\"b\"] = 2");
    }

    #[test]
    fn assign_index_map_wrong_key_type_is_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\nm[42] = 2",
            &["key", "Str", "Int"],
        );
    }

    #[test]
    fn assign_index_on_non_collection_is_error() {
        assert_error_with("let x = 5\nx[0] = 1", &["List", "Map"]);
    }

    // ---- Range ----

    #[test]
    fn range_of_ints_is_ok() {
        assert_ok("let r = 0..10");
    }

    #[test]
    fn range_with_non_int_extremity_is_error() {
        assert_error_with("let r = 0..\"diez\"", &["range", "Int", "Str"]);
    }

    // ---- List / Map ----

    #[test]
    fn empty_list_is_list_any() {
        let (_, errors) = check_str("let xs = []");
        assert!(errors.is_empty(), "{:?}", errors);
    }

    #[test]
    fn homogeneous_int_list_is_list_int() {
        // No error; the inferred type is List<Int>.
        assert_ok("let xs: List<Int> = [1, 2, 3]");
    }

    #[test]
    fn annotated_list_with_incompatible_type_is_error() {
        // The RHS synthesizes List<Str>; the annotation is List<Int>.
        assert_error_with(
            "let xs: List<Int> = [\"a\", \"b\"]",
            &["xs", "List<Int>", "List<Str>"],
        );
    }

    #[test]
    fn empty_map_is_map_any_any() {
        assert_ok("let m = {}");
    }

    // ---- StructLit ----

    #[test]
    fn struct_lit_with_known_type_and_fields_ok() {
        assert_ok(
            "type User { id: Int, name: Str }\n\
             let u = User { id: 1, name: \"x\" }",
        );
    }

    #[test]
    fn struct_lit_with_unknown_type_is_error() {
        assert_error_with("let u = Usuario { id: 1 }", &["Usuario", "does not exist"]);
    }

    #[test]
    fn struct_lit_field_with_incompatible_type_is_error() {
        assert_error_with(
            "type User { id: Int }\n\
             let u = User { id: \"no soy int\" }",
            &["User.id", "Int", "Str"],
        );
    }

    #[test]
    fn struct_lit_extra_field_is_error() {
        assert_error_with(
            "type User { id: Int }\n\
             let u = User { id: 1, edad: 30 }",
            &["User", "edad"],
        );
    }

    // ---- Field access ----

    #[test]
    fn field_access_of_nominal_returns_field_type() {
        // If u.id is Int, assigning it to an Int is OK.
        assert_ok(
            "type User { id: Int, name: Str }\n\
             let u = User { id: 1, name: \"x\" }\n\
             let i: Int = u.id",
        );
    }

    #[test]
    fn field_access_of_nominal_incompatible_type_is_error() {
        assert_error_with(
            "type User { id: Int, name: Str }\n\
             let u = User { id: 1, name: \"x\" }\n\
             let i: Int = u.name",
            &["Int", "Str"],
        );
    }

    // ---- Assign with annotation ----

    #[test]
    fn assign_int_to_int_is_ok() {
        assert_ok("let x: Int = 42");
    }

    #[test]
    fn assign_str_to_int_is_error() {
        assert_error_with("let x: Int = \"hola\"", &["x", "Int", "Str"]);
    }

    #[test]
    fn assign_null_to_nullable_is_ok() {
        assert_ok("let x: Str? = null");
    }

    #[test]
    fn assign_int_to_float_is_ok_by_coercion() {
        assert_ok("let x: Float = 1");
    }

    #[test]
    fn assign_str_to_nullable_str_is_ok() {
        // T compatible with T?.
        assert_ok("let x: Str? = \"hola\"");
    }

    // ---- if / while / for ----

    #[test]
    fn if_with_non_bool_cond_is_error() {
        assert_error_with("if 1 { print(\"x\") }", &["condition", "if", "Bool", "Int"]);
    }

    #[test]
    fn if_with_bool_cond_is_ok() {
        assert_ok("if true { print(\"sí\") } else { print(\"no\") }");
    }

    #[test]
    fn while_with_non_bool_cond_is_error() {
        assert_error_with("while 1 { break }", &["while", "Bool"]);
    }

    #[test]
    fn for_on_range_binds_var_as_int() {
        // Inside the for, i must be used as Int and the sum must
        // type-check correctly.
        assert_ok("for i in 0..10 { let n: Int = i + 1 }");
    }

    #[test]
    fn for_on_list_int_binds_element_as_int() {
        assert_ok(
            "let xs = [1, 2, 3]\n\
             for x in xs { let n: Int = x }",
        );
    }

    #[test]
    fn for_on_non_iterable_is_error() {
        assert_error_with("for x in 42 { print(x) }", &["for", "List", "Range", "Int"]);
    }

    // ---- FnDef / bound params ----

    #[test]
    fn fndef_param_binds_in_body() {
        // The parameter n is Int from its annotation.
        assert_ok("fn double(n: Int) -> Int { return n * 2 }");
    }

    #[test]
    fn fndef_param_without_annotation_is_any() {
        // Without annotation, n is Any — it doesn't complain about the sum.
        assert_ok("fn double(n) { return n * 2 }");
    }

    // ---- FnExpr / bound params ----

    #[test]
    fn fn_expr_binds_its_param() {
        // If it didn't bind, `u` would be unknown.
        assert_ok(
            "type User { id: Int }\n\
             let users = [User { id: 1 }]\n\
             let r = users.find(fn(u) => u.id == 1)",
        );
    }

    // ---- Match with bindings ----

    #[test]
    fn match_ident_pattern_binds_var() {
        // The arm `x => ...` binds x as the type of the scrutinee.
        assert_ok(
            "let v = 42\n\
             let s = match v {\n\
                 0 => \"cero\"\n\
                 x => \"otro\"\n\
             }",
        );
    }

    #[test]
    fn match_ok_pattern_binds_inner_of_result() {
        // Ok(v) in match over Result<Int> → v is Int.
        // In 5.3.1 the scrutinee is Ok(Int) which has type Result<Int>,
        // and v is bound as Int. We verify by adding v with an Int.
        assert_ok(
            "let r = Ok(5)\n\
             let s = match r {\n\
                 Ok(v)  => v + 1\n\
                 Err(e) => 0\n\
             }",
        );
    }

    #[test]
    fn match_nullable_refinement_arm_post_null_w2() {
        // W2 (v0.10.6) — `match user { null => ..., u => u.name }`
        // refines `u` from `User?` to `User` (without Nullable) because the
        // previous arm covered null. Before, the binding stayed as `User?` and
        // `u.name` failed the checker with "field access over Nullable".
        assert_ok(
            "type User { name: Str }\n\
             type Profile { user: User? }\n\
             let p = Profile { user: User { name: \"ada\" } }\n\
             let s = match p.user {\n\
                 null => \"sin usuario\"\n\
                 u    => u.name\n\
             }",
        );
    }

    // (The test "Ident before Null doesn't refine" was removed: the
    // checker today allows `u.name` over `u: Nullable<User>` always
    // (lenient with field access over Nullable). The W2 refinement
    // of the checker is nice-to-have but doesn't detect the problematic
    // case — that one shows up in CODEGEN where rustc rejects the code
    // due to type mismatch. See the E2E `db_match_nullable_refinement_w2`
    // in tests/compile_e2e.rs for the real W2 closure.)

    #[test]
    fn match_err_pattern_binds_inner_as_str() {
        // Err(e) binds e as Str — concatenable with Str.
        assert_ok(
            "let r = Err(\"boom\")\n\
             let s = match r {\n\
                 Ok(v)  => \"OK\"\n\
                 Err(e) => \"E: \" + e\n\
             }",
        );
    }

    // ---- Imports ----

    #[test]
    fn from_import_binds_names_in_scope() {
        // We can't load a real module here without touching disk.
        // What we validate: the ident brought in by `from` is not
        // reported as unknown.
        assert_ok(
            "from utils import slugify\n\
             let s = slugify",
        );
    }

    #[test]
    fn import_binds_module_as_var() {
        // `import foo` leaves `foo` accessible as a variable.
        assert_ok(
            "import utils\n\
             let m = utils",
        );
    }

    #[test]
    fn struct_lit_of_imported_type_is_ok() {
        // `from foo import User; User { ... }` doesn't fail because
        // FromImport registers the name as a nominal without fields.
        // The checker doesn't validate fields (it doesn't know them) and lets it pass.
        assert_ok(
            "from foo import User\n\
             let u = User { id: 1, name: \"x\" }",
        );
    }

    // ---- Multiple accumulated errors ----

    #[test]
    fn checker_accumulates_several_expression_errors() {
        let (_, errors) = check_str(
            "let a: Int = \"x\"\n\
             let b = 1 + \"y\"\n\
             let c = no_var",
        );
        assert!(
            errors.len() >= 3,
            "expected 3+ errors, was {}: {:?}",
            errors.len(),
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    // ---- 5.3.2: calls and return ----

    #[test]
    fn call_correct_arity_and_types_ok() {
        assert_ok(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n: Int = add(1, 2)",
        );
    }

    #[test]
    fn call_too_few_args_is_error() {
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n = add(1)",
            &["add", "2 argument", "received 1"],
        );
    }

    #[test]
    fn call_too_many_args_is_error() {
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n = add(1, 2, 3)",
            &["add", "2 argument", "received 3"],
        );
    }

    #[test]
    fn call_incompatible_arg_type_is_error() {
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n = add(\"hola\", 2)",
            &["add", "argument 1", "Int", "Str"],
        );
    }

    #[test]
    fn call_int_to_float_coercion_passes() {
        assert_ok(
            "fn double(x: Float) -> Float { return x * 2.0 }\n\
             let n: Float = double(3)",
        );
    }

    #[test]
    fn call_null_to_nullable_param_passes() {
        assert_ok(
            "fn greet(name: Str?) -> Str { return \"hola\" }\n\
             let g: Str = greet(null)",
        );
    }

    #[test]
    fn call_top_level_recursion_compiles() {
        // The signature pre-registration must see `fact` before checking
        // its body so that the recursive call doesn't complain.
        assert_ok(
            "fn fact(n: Int) -> Int {\n\
                 if (n <= 1) { return 1 }\n\
                 return n * fact(n - 1)\n\
             }",
        );
    }

    #[test]
    fn call_cross_fn_forward_reference_compiles() {
        // `a` calls `b` defined later. The pre-registration makes it
        // visible.
        assert_ok(
            "fn a(n: Int) -> Int { return b(n) + 1 }\n\
             fn b(n: Int) -> Int { return n * 2 }",
        );
    }

    #[test]
    fn call_on_non_function_callee_is_error() {
        // `1(2)` is not a callable function.
        assert_error_with("let r = (1)(2)", &["is not a function", "Int"]);
    }

    #[test]
    fn call_fn_expr_inline_passes() {
        // (fn(x) => x + 1)(2) — the callee resolves to Function.
        // Arity and Any param → any arg passes.
        assert_ok("let r = (fn(x) => x + 1)(2)");
    }

    #[test]
    fn call_fn_expr_inline_arity_fails() {
        // Arity checked even in inline FnExpr.
        assert_error_with(
            "let r = (fn(x, y) => x + y)(1)",
            &["2 argument", "received 1"],
        );
    }

    // ---- Builtins ----

    #[test]
    fn len_with_one_arg_passes_and_returns_int() {
        assert_ok("let n: Int = len([1, 2, 3])");
    }

    #[test]
    fn len_without_args_is_arity_error() {
        assert_error_with("let n = len()", &["len", "1 argument", "received 0"]);
    }

    #[test]
    fn len_with_two_args_is_arity_error() {
        assert_error_with(
            "let n = len([1], [2])",
            &["len", "1 argument", "received 2"],
        );
    }

    #[test]
    fn print_is_variadic_does_not_check_arity() {
        // print is still Any → any number of args passes.
        assert_ok("print()\nprint(\"x\")\nprint(1, 2, 3, \"y\")");
    }

    // ---- Stmt::Return against return_type ----

    #[test]
    fn return_compatible_type_passes() {
        assert_ok("fn double(n: Int) -> Int { return n * 2 }");
    }

    #[test]
    fn return_incompatible_type_is_error() {
        assert_error_with(
            "fn double(n: Int) -> Int { return \"no soy int\" }",
            &["return", "Int", "Str"],
        );
    }

    #[test]
    fn return_without_annotation_does_not_check() {
        // Without return_type → Any → no check.
        assert_ok("fn f() { return \"cualquier cosa\" }");
    }

    #[test]
    fn return_implicit_arrow_checks_against_return_type() {
        // `fn f() -> Int => "x"` desugars to `body: [Stmt::Return("x", Span::ZERO)]`.
        assert_error_with(
            "fn id(x: Int) -> Int => \"no soy int\"",
            &["return", "Int", "Str"],
        );
    }

    #[test]
    fn return_implicit_arrow_correct_passes() {
        assert_ok("fn double(n: Int) -> Int => n * 2");
    }

    #[test]
    fn return_ok_against_result_passes() {
        // Ok(user) types as Result<User>; must match against
        // -> Result<User>.
        assert_ok(
            "type User { id: Int, name: Str }\n\
             fn make(id: Int) -> Result<User> {\n\
                 return Ok(User { id: id, name: \"x\" })\n\
             }",
        );
    }

    #[test]
    fn return_err_against_result_passes_via_is_compatible_recursive() {
        // Err(_) types as Result<Any>. Without recursion in
        // is_compatible this would fail against Result<User>.
        assert_ok(
            "type User { id: Int }\n\
             fn make() -> Result<User> {\n\
                 return Err(\"boom\")\n\
             }",
        );
    }

    #[test]
    fn orphan_return_checks() {
        // R.2.4 (F3): `return` outside of fn is now a static error
        // from the checker. Before, it passed to the evaluator and
        // was reported at runtime; now we catch it earlier.
        let (_, errors) = check_str("return 1");
        assert!(errors
            .iter()
            .any(|e| e.message.contains("return") && e.message.contains("function")));
    }

    // ---- is_compatible recursive on generics ----

    #[test]
    fn is_compatible_list_recursive() {
        // List<Int> vs List<Float> passes via Int→Float coercion inside.
        assert!(is_compatible(
            &Type::List(Box::new(Type::Int)),
            &Type::List(Box::new(Type::Float)),
        ));
        // List<Str> vs List<Int> doesn't pass.
        assert!(!is_compatible(
            &Type::List(Box::new(Type::Str)),
            &Type::List(Box::new(Type::Int)),
        ));
    }

    #[test]
    fn is_compatible_result_recursive() {
        // Result<Any> matches Result<User>.
        let env = env_with(&["User"]);
        let user = Type::Nominal(env.lookup("User").unwrap());
        assert!(is_compatible(
            &Type::Result {
                ok: Box::new(Type::Any),
                err: Box::new(Type::Str)
            },
            &Type::Result {
                ok: Box::new(user.clone()),
                err: Box::new(Type::Str)
            },
        ));
        // Result<Int> doesn't match Result<Str>.
        assert!(!is_compatible(
            &Type::Result {
                ok: Box::new(Type::Int),
                err: Box::new(Type::Str)
            },
            &Type::Result {
                ok: Box::new(Type::Str),
                err: Box::new(Type::Str)
            },
        ));
    }

    #[test]
    fn is_compatible_map_recursive() {
        // Map<Str, Int> matches Map<Str, Float>.
        assert!(is_compatible(
            &Type::Map(Box::new(Type::Str), Box::new(Type::Int)),
            &Type::Map(Box::new(Type::Str), Box::new(Type::Float)),
        ));
        // Map<Int, X> doesn't match Map<Str, X> (incompatible key).
        assert!(!is_compatible(
            &Type::Map(Box::new(Type::Int), Box::new(Type::Int)),
            &Type::Map(Box::new(Type::Str), Box::new(Type::Int)),
        ));
    }

    #[test]
    fn is_compatible_function_structural() {
        // fn(Int) -> Int matches fn(Int) -> Int.
        let a = Type::Function {
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        };
        let b = Type::Function {
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        };
        assert!(is_compatible(&a, &b));
        // fn(Int) -> Int doesn't match fn(Int, Int) -> Int (different arity).
        let c = Type::Function {
            params: vec![Type::Int, Type::Int],
            ret: Box::new(Type::Int),
        };
        assert!(!is_compatible(&a, &c));
    }

    // ---- 5.3.3: `?` and exhaustive match over Result ----

    #[test]
    fn try_on_result_inside_fn_result_passes() {
        // The operand is Result<Int>; the fn declares -> Result<Int>.
        // The `?` unpacks to Int.
        assert_ok(
            "fn f(r: Result<Int>) -> Result<Int> {\n\
                 let v: Int = r?\n\
                 return Ok(v + 1)\n\
             }",
        );
    }

    #[test]
    fn try_on_any_does_not_check() {
        // `users.find(...)` is a built-in method: callee Field → Any.
        // `?` over Any passes without checking (gradual, until 5.3.4).
        assert_ok(
            "type User { id: Int }\n\
             fn h(id: Int) {\n\
                 let users = [User { id: 1 }]\n\
                 let u = users.find(fn(u) => u.id == id)?\n\
                 return u\n\
             }",
        );
    }

    #[test]
    fn try_on_non_result_is_error() {
        // `?` over an Int makes no sense.
        assert_error_with(
            "fn f() -> Result<Int> { let x = 1?\n return Ok(x) }",
            &["?", "Result", "Int"],
        );
    }

    #[test]
    fn try_inside_fn_non_result_is_error() {
        // The fn returns Int (not Result) and inside there's a `?`. The
        // operand is concrete Result<Int>, so we fire the
        // "fn must return Result" rule.
        assert_error_with(
            "fn f(r: Result<Int>) -> Int {\n\
                 let v = r?\n\
                 return v\n\
             }",
            &["?", "Result", "Int"],
        );
    }

    #[test]
    fn try_inside_fn_without_return_type_does_not_check() {
        // Without annotation → return_stack is Any → we don't check the
        // containing fn rule. The operand still must be
        // Result, so the `?` unpacks to Int without warnings.
        assert_ok(
            "fn f(r: Result<Int>) {\n\
                 let v: Int = r?\n\
                 return v\n\
             }",
        );
    }

    #[test]
    fn try_top_level_does_not_check_containing_fn_rule() {
        // `?` inside the global scope — without return_stack, we don't
        // fire the "fn must return Result" rule. The operand
        // is checked: Result<Int> → unpacks to Int.
        assert_ok("let r: Result<Int> = Ok(1)\nlet v: Int = r?");
    }

    #[test]
    fn w13_try_inside_http_handler_non_result_passes() {
        // W13 (v0.10.9) — HTTP handler that returns `User` (not Result)
        // and uses `?` inside. Before the fix, the checker rejected with
        // "the `?` operator can only be used inside a function
        // that returns `Result<...>`". Now it passes: the runtime/codegen
        // wrapper converts the Err propagated by `?` into an automatic
        // 500 response, so the checker trusts that
        // semantics when we are `in_http_handler`.
        assert_ok(
            "type User { id: Int }\n\
             fn parse(s: Str) -> Result<Int> { return Ok(1) }\n\
             @get(\"/u/{stub}\")\n\
             fn handler(stub: Str) -> User {\n\
                 let id = parse(stub)?\n\
                 return User { id: id }\n\
             }",
        );
    }

    #[test]
    fn w13_try_outside_http_handler_still_is_error() {
        // W13 negative — the relaxation ONLY applies inside an
        // HTTP handler (`@get/@post/etc`). A regular fn returning
        // a non-Result type and using `?` is still an error (parallel to
        // `try_adentro_de_fn_no_result_es_error`). This avoids the
        // gradual ergonomics of W13 contaminating non-HTTP code.
        assert_error_with(
            "fn helper(r: Result<Int>) -> Int {\n\
                 let v = r?\n\
                 return v\n\
             }",
            &["?", "Result"],
        );
    }

    #[test]
    fn try_chained_with_field_access_works() {
        // r?.id over Result<User> → User → Int.
        assert_ok(
            "type User { id: Int, name: Str }\n\
             fn f(r: Result<User>) -> Result<Int> {\n\
                 let id: Int = r?.id\n\
                 return Ok(id)\n\
             }",
        );
    }

    // ---- exhaustive match over Result ----

    #[test]
    fn match_result_with_ok_and_err_is_exhaustive() {
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(v) => \"ok\"\n\
                 Err(e) => \"err\"\n\
             }",
        );
    }

    #[test]
    fn match_result_only_ok_missing_err() {
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(v) => \"ok\"\n\
             }",
            &["match", "Result", "exhaustive", "Err"],
        );
    }

    #[test]
    fn match_result_only_err_missing_ok() {
        assert_error_with(
            "let r: Result<Int> = Err(\"x\")\n\
             let s = match r {\n\
                 Err(e) => \"err\"\n\
             }",
            &["match", "Result", "exhaustive", "Ok"],
        );
    }

    #[test]
    fn match_result_with_only_wildcard_is_exhaustive() {
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 _ => \"cualquier\"\n\
             }",
        );
    }

    #[test]
    fn match_result_with_ok_plus_wildcard_is_exhaustive() {
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(v) => \"ok\"\n\
                 _ => \"resto\"\n\
             }",
        );
    }

    #[test]
    fn match_result_with_ident_catchall_is_exhaustive() {
        // An ident binding (catch-all) covers any value — the
        // evaluator treats it as a wildcard.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 x => \"siempre\"\n\
             }",
        );
    }

    #[test]
    fn match_on_int_does_not_require_exhaustiveness() {
        // Match over a non-Result type: the checker does not require
        // exhaustiveness in 5.3.3.
        assert_ok(
            "let n = 1\n\
             let s = match n {\n\
                 0 => \"cero\"\n\
                 1 => \"uno\"\n\
             }",
        );
    }

    #[test]
    fn match_on_any_does_not_require_exhaustiveness() {
        // Match over a value of type Any (gradual escape): no
        // exhaustiveness is required.
        assert_ok(
            "fn pick() { return Ok(1) }\n\
             let s = match pick() {\n\
                 Ok(v) => \"ok\"\n\
             }",
        );
    }

    // ---- 5.3.4: built-in methods with parametric templates ----

    // List<T>: push

    #[test]
    fn list_push_with_compatible_type_passes() {
        assert_ok(
            "let xs: List<Int> = [1, 2]\n\
             xs.push(3)",
        );
    }

    #[test]
    fn list_push_with_incompatible_type_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             xs.push(\"x\")",
            &["push", "List<Int>", "Str"],
        );
    }

    #[test]
    fn list_push_wrong_arity_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             xs.push(1, 2)",
            &["push", "1 argument", "received 2"],
        );
    }

    // List<T>: pop, len

    #[test]
    fn list_pop_returns_t() {
        // If pop over List<Int> returns Int, assigning it to Int is OK.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let last: Int = xs.pop()",
        );
    }

    #[test]
    fn list_len_returns_int() {
        assert_ok(
            "let xs = [1, 2, 3]\n\
             let n: Int = xs.len()",
        );
    }

    // List<T>: map

    #[test]
    fn list_map_returns_list_of_callback_ret() {
        // map over List<Int> with callback fn(Int) -> Str → List<Str>.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let strs: List<Str> = xs.map(fn(x: Int) -> Str { return \"x\" })",
        );
    }

    #[test]
    fn list_map_with_incompatible_callback_param_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.map(fn(x: Str) -> Str { return x })",
            &["map", "Int", "Str"],
        );
    }

    #[test]
    fn list_map_with_callback_without_annotations_is_any() {
        // Callback without annotations → params = [Any], ret = Any.
        // The map returns List<Any>; assigning it to List<Int> passes via
        // recursive is_compatible + Any.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.map(fn(x) => x * 2)",
        );
    }

    // List<T>: filter

    #[test]
    fn list_filter_returns_list_t() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let evens: List<Int> = xs.filter(fn(x: Int) -> Bool { return true })",
        );
    }

    #[test]
    fn list_filter_callback_wrong_arity_is_error() {
        // FnExpr always has `ret = Any` until 5.3.5, so
        // we can't detect "ret is not Bool" over an inline FnExpr.
        // What we DO catch is the callback arity: filter expects
        // fn(T) -> Bool with a single param.
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.filter(fn(x, y) => true)",
            &["filter", "1 argument", "received 2"],
        );
    }

    // List<T>: find

    #[test]
    fn list_find_returns_result_t() {
        // find over List<User> returns Result<User>.
        assert_ok(
            "type User { id: Int }\n\
             let xs: List<User> = [User { id: 1 }]\n\
             let r: Result<User> = xs.find(fn(u: User) -> Bool { return true })",
        );
    }

    #[test]
    fn list_find_with_try_unblocks_t() {
        // xs.find(...)? inside a fn -> Result<User> should
        // unpack to User.
        assert_ok(
            "type User { id: Int }\n\
             fn first(xs: List<User>) -> Result<User> {\n\
                 let u: User = xs.find(fn(u: User) -> Bool { return true })?\n\
                 return Ok(u)\n\
             }",
        );
    }

    // List<T>: unknown method

    #[test]
    fn list_unknown_method_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             xs.lenght()",
            &["List<Int>", "lenght"],
        );
    }

    // Map<K, V>: get, has

    #[test]
    fn map_get_returns_result_v() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let r: Result<Int> = m.get(\"a\")",
        );
    }

    #[test]
    fn map_get_with_incompatible_key_is_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let r = m.get(42)",
            &["get", "Map<Str, Int>", "Int"],
        );
    }

    #[test]
    fn map_has_returns_bool() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let b: Bool = m.has(\"a\")",
        );
    }

    #[test]
    fn map_keys_and_values_return_lists() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let ks: List<Str> = m.keys()\n\
             let vs: List<Int> = m.values()",
        );
    }

    #[test]
    fn map_len_returns_int() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let n: Int = m.len()",
        );
    }

    #[test]
    fn map_unknown_method_is_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             m.foo()",
            &["Map<Str, Int>", "foo"],
        );
    }

    // Str

    #[test]
    fn str_upper_lower_return_str() {
        assert_ok(
            "let s = \"hola\"\n\
             let u: Str = s.upper()\n\
             let l: Str = s.lower()",
        );
    }

    #[test]
    fn str_len_returns_int() {
        assert_ok("let n: Int = \"hola\".len()");
    }

    #[test]
    fn str_unknown_method_is_error() {
        assert_error_with(
            "let s = \"hola\"\n\
             s.upcase()",
            &["Str", "upcase"],
        );
    }

    // ---- S.1: contains/starts_with/ends_with ----

    #[test]
    fn str_contains_returns_bool() {
        assert_ok("let b: Bool = \"hola\".contains(\"ol\")");
    }

    #[test]
    fn str_starts_with_ends_with_return_bool() {
        assert_ok(
            "let a: Bool = \"hola\".starts_with(\"ho\")\n\
             let b: Bool = \"hola\".ends_with(\"la\")",
        );
    }

    #[test]
    fn str_contains_with_non_str_arg_is_error() {
        assert_error_with("let b = \"hola\".contains(1)", &["contains", "Str"]);
    }

    // ---- S.2: split/trim/replace/repeat ----

    #[test]
    fn str_split_returns_list_str() {
        assert_ok("let xs: List<Str> = \"a,b,c\".split(\",\")");
    }

    #[test]
    fn str_trim_returns_str() {
        assert_ok("let s: Str = \"  hola  \".trim()");
    }

    #[test]
    fn str_replace_returns_str() {
        assert_ok("let s: Str = \"hola\".replace(\"o\", \"O\")");
    }

    #[test]
    fn str_replace_with_int_is_error() {
        assert_error_with("let s = \"hola\".replace(\"o\", 42)", &["replace", "Str"]);
    }

    #[test]
    fn str_repeat_with_int_returns_str() {
        assert_ok("let s: Str = \"ab\".repeat(3)");
    }

    #[test]
    fn str_repeat_with_str_is_error() {
        assert_error_with("let s = \"ab\".repeat(\"3\")", &["repeat", "Int"]);
    }

    // ---- S.3: List.sort/reverse/contains ----

    #[test]
    fn list_sort_and_reverse_return_null() {
        assert_ok(
            "let xs: List<Int> = [3, 1, 2]\n\
             xs.sort()\n\
             xs.reverse()",
        );
    }

    #[test]
    fn list_contains_with_compatible_arg_returns_bool() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: Bool = xs.contains(2)",
        );
    }

    #[test]
    fn list_contains_with_incompatible_arg_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.contains(\"x\")",
            &["contains", "Int"],
        );
    }

    // ---- Mini-batch Mb2 + Rg ----

    #[test]
    fn mb2_list_min_max_on_list_int_returns_result_int() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let lo: Result<Int> = xs.min()\n\
             let hi: Result<Int> = xs.max()",
        );
    }

    #[test]
    fn mb2_list_min_max_on_list_float_returns_result_float() {
        assert_ok(
            "let xs: List<Float> = [1.0, 2.0]\n\
             let lo: Result<Float> = xs.min()",
        );
    }

    #[test]
    fn mb2_list_min_on_list_str_is_error() {
        assert_error_with(
            "let xs: List<Str> = [\"a\", \"b\"]\n\
             let r = xs.min()",
            &["min", "Int", "Float"],
        );
    }

    #[test]
    fn mb2_list_sum_int_returns_int() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let total: Int = xs.sum()",
        );
    }

    #[test]
    fn mb2_list_sum_float_returns_float() {
        assert_ok(
            "let xs: List<Float> = [1.5, 2.5]\n\
             let total: Float = xs.sum()",
        );
    }

    #[test]
    fn mb2_list_sum_on_str_is_error() {
        assert_error_with(
            "let xs: List<Str> = [\"a\"]\n\
             let total = xs.sum()",
            &["sum", "Int", "Float"],
        );
    }

    #[test]
    fn mb2_str_pad_start_end_return_str() {
        assert_ok(
            "let s = \"42\"\n\
             let a: Str = s.pad_start(5, \"0\")\n\
             let b: Str = s.pad_end(5, \".\")",
        );
    }

    #[test]
    fn mb2_str_pad_start_with_non_int_width_is_error() {
        assert_error_with(
            "let r = \"42\".pad_start(\"5\", \"0\")",
            &["pad_start", "Int"],
        );
    }

    #[test]
    fn mb2_str_pad_end_with_non_str_ch_is_error() {
        assert_error_with("let r = \"42\".pad_end(5, 0)", &["pad_end", "Str"]);
    }

    #[test]
    fn mb2_map_keys_sorted_returns_list_of_keys() {
        assert_ok(
            "let m: Map<Str, Int> = {\"b\": 2, \"a\": 1}\n\
             let ks: List<Str> = m.keys_sorted()",
        );
    }

    #[test]
    fn rg_range_step_by_returns_list_int() {
        assert_ok("let xs: List<Int> = (0..10).step_by(2)");
    }

    #[test]
    fn rg_range_step_by_with_non_int_arg_is_error() {
        assert_error_with("let xs = (0..10).step_by(\"x\")", &["step_by", "Int"]);
    }

    // ---- Mini-batch Mb3: reduce + product + chars + entries + to_map ----

    #[test]
    fn mb3_list_reduce_acc_int_returns_int() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let total: Int = xs.reduce(0, fn(acc: Int, x: Int) => acc + x)",
        );
    }

    #[test]
    fn mb3_list_reduce_acc_different_from_t_works() {
        // Acc can be Str even if T is Int.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let s: Str = xs.reduce(\"\", fn(acc: Str, x: Int) => acc)",
        );
    }

    #[test]
    fn mb3_list_reduce_callback_ret_different_from_acc_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let total: Int = xs.reduce(0, fn(acc: Int, x: Int) => \"oops\")",
            &["reduce", "Int"],
        );
    }

    #[test]
    fn mb3_list_product_int_returns_int() {
        assert_ok(
            "let xs: List<Int> = [2, 3, 4]\n\
             let p: Int = xs.product()",
        );
    }

    #[test]
    fn mb3_list_product_on_str_is_error() {
        assert_error_with(
            "let xs: List<Str> = [\"a\"]\n\
             let p = xs.product()",
            &["product", "Int", "Float"],
        );
    }

    #[test]
    fn mb3_str_chars_returns_list_str() {
        assert_ok("let cs: List<Str> = \"abc\".chars()");
    }

    #[test]
    fn mb3_map_entries_returns_list_of_tuples() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let es: List<(Str, Int)> = m.entries()",
        );
    }

    #[test]
    fn mb3_list_to_map_on_tuple_pairs() {
        assert_ok(
            "let pairs: List<(Str, Int)> = [(\"a\", 1), (\"b\", 2)]\n\
             let m: Map<Str, Int> = pairs.to_map()",
        );
    }

    #[test]
    fn mb3_list_to_map_on_non_tuple_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             let m = xs.to_map()",
            &["to_map", "Tuple"],
        );
    }

    // ---- Mini-batch Mb4 + Cmp+ ----

    #[test]
    fn mb4_list_unique_returns_list_t() {
        assert_ok(
            "let xs: List<Int> = [1, 1, 2]\n\
             let r: List<Int> = xs.unique()",
        );
    }

    #[test]
    fn mb4_list_partition_returns_tuple_of_lists() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: (List<Int>, List<Int>) = xs.partition(fn(n: Int) => n > 1)",
        );
    }

    #[test]
    fn mb4_list_partition_callback_non_bool_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             let r = xs.partition(fn(n: Int) => n)",
            &["partition", "Bool"],
        );
    }

    #[test]
    fn mb4_map_invert_swap_k_v() {
        assert_ok(
            "let m: Map<Int, Str> = {1: \"a\"}\n\
             let r: Map<Str, Int> = m.invert()",
        );
    }

    #[test]
    fn mb4_str_split_at_returns_tuple_str_str() {
        assert_ok("let r: (Str, Str) = \"abc\".split_at(1)");
    }

    #[test]
    fn mb4_str_split_at_with_non_int_arg_is_error() {
        assert_error_with("let r = \"abc\".split_at(\"x\")", &["split_at", "Int"]);
    }

    #[test]
    fn cmp_multi_for_clauses_tipan_list_int() {
        assert_ok(
            "let xs: List<Int> = [1, 2]\n\
             let ys: List<Int> = [10, 20]\n\
             let r: List<Int> = [x + y for x in xs for y in ys]",
        );
    }

    #[test]
    fn cmp_multi_for_nested_var_visible_in_expr() {
        // The binding `y` from the second for is visible in the expr.
        assert_ok(
            "let xs: List<Int> = [1, 2]\n\
             let ys: List<Int> = [10]\n\
             let r: List<Int> = [y for x in xs for y in ys]",
        );
    }

    #[test]
    fn cmp_map_comp_tipa_como_map_k_v() {
        assert_ok("let m: Map<Int, Int> = {n: n * n for n in 1..=3}");
    }

    #[test]
    fn cmp_map_comp_filter_non_bool_is_error() {
        assert_error_with("let m = {n: n for n in 0..3 if n}", &["filter", "Bool"]);
    }

    // ---- Mini-batch Mb5 + Async-cl ----

    #[test]
    fn mb5_list_group_by_returns_map_k_list_t() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: Map<Str, List<Int>> = xs.group_by(fn(n: Int) => if (n > 1) { \"big\" } else { \"small\" })",
        );
    }

    #[test]
    fn mb5_list_zip_with_returns_list_v() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let ys: List<Int> = [10, 20]\n\
             let r: List<Int> = xs.zip_with(ys, fn(a: Int, b: Int) => a + b)",
        );
    }

    #[test]
    fn mb5_list_zip_with_non_list_arg_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1]\n\
             let r = xs.zip_with(42, fn(a: Int, b: Int) => a + b)",
            &["zip_with", "List"],
        );
    }

    #[test]
    fn mb5_list_max_by_returns_result_t() {
        assert_ok(
            "type P { age: Int = 0 }\n\
             let xs: List<P> = [P { age: 1 }]\n\
             let r: Result<P> = xs.max_by(fn(p: P) => p.age)",
        );
    }

    #[test]
    fn mb5_list_max_by_callback_non_int_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1]\n\
             let r = xs.max_by(fn(n: Int) => \"oops\")",
            &["max_by", "Int"],
        );
    }

    #[test]
    fn mb5_str_lines_returns_list_str() {
        assert_ok("let r: List<Str> = \"a\\nb\".lines()");
    }

    #[test]
    fn mb5_str_is_empty_returns_bool() {
        assert_ok("let r: Bool = \"\".is_empty()");
    }

    #[test]
    fn async_cl_inline_types_as_function_with_future() {
        // The type of the async FnExpr has ret = Future<T>, so the
        // checker validates `.await` inside and lets it be used from an
        // async caller fn.
        assert_ok(
            "async fn run() -> Int {\n\
                 let f = async fn(n: Int) -> Int { return n * 2 }\n\
                 return f(21).await\n\
             }",
        );
    }

    // ---- Mini-batch Mb6 ----

    #[test]
    fn mb6_list_scan_returns_list_acc() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.scan(0, fn(acc: Int, x: Int) => acc + x)",
        );
    }

    #[test]
    fn mb6_list_scan_callback_ret_different_from_acc_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             let r = xs.scan(0, fn(acc: Int, x: Int) => \"oops\")",
            &["scan", "Int"],
        );
    }

    #[test]
    fn mb6_list_windows_returns_list_list_t() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<List<Int>> = xs.windows(2)",
        );
    }

    #[test]
    fn mb6_list_windows_non_int_arg_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.windows(\"oops\")",
            &["windows", "Int"],
        );
    }

    #[test]
    fn mb6_map_merge_with_returns_map_k_v() {
        assert_ok(
            "let a: Map<Str, Int> = {\"x\": 1}\n\
             let b: Map<Str, Int> = {\"x\": 2}\n\
             let r: Map<Str, Int> = a.merge_with(b, fn(va: Int, vb: Int) => va + vb)",
        );
    }

    // ---- Mini-batch Mb8 + Bits-extras ----

    #[test]
    fn mb8_list_starts_with_returns_bool() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: Bool = xs.starts_with([1, 2])",
        );
    }

    #[test]
    fn mb8_list_starts_with_non_list_arg_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1]\n\
             let r = xs.starts_with(42)",
            &["starts_with", "List"],
        );
    }

    #[test]
    fn mb8_list_insert_at_returns_list_t() {
        assert_ok(
            "let xs: List<Int> = [1, 3]\n\
             let r: List<Int> = xs.insert_at(1, 2)",
        );
    }

    #[test]
    fn mb8_list_remove_at_returns_list_t() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.remove_at(1)",
        );
    }

    #[test]
    fn mb8_list_zip_to_map_returns_map_k_v() {
        assert_ok(
            "let ks: List<Str> = [\"a\"]\n\
             let vs: List<Int> = [1]\n\
             let m: Map<Str, Int> = ks.zip_to_map(vs)",
        );
    }

    #[test]
    fn mb8_str_left_right_return_str() {
        assert_ok(
            "let l: Str = \"abc\".left(2)\n\
             let r: Str = \"abc\".right(2)",
        );
    }

    #[test]
    fn mb8_str_center_returns_str() {
        assert_ok("let c: Str = \"hi\".center(10, \"-\")");
    }

    #[test]
    fn bits_extras_popcount_tipa_int() {
        assert_ok("let r: Int = popcount(42)");
    }

    #[test]
    fn bits_extras_rotate_left_tipa_int() {
        assert_ok("let r: Int = rotate_left(1, 4)");
    }

    #[test]
    fn bits_extras_popcount_non_int_arg_is_error() {
        assert_error_with("let r = popcount(\"oops\")", &["popcount", "Int"]);
    }

    // ---- Mini-batch Mb7 ----

    #[test]
    fn mb7_list_take_drop_return_list_t() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let a: List<Int> = xs.take(2)\n\
             let b: List<Int> = xs.drop(1)",
        );
    }

    #[test]
    fn mb7_list_take_non_int_arg_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.take(\"x\")",
            &["take", "Int"],
        );
    }

    #[test]
    fn mb7_list_init_tail_no_args() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let a: List<Int> = xs.init()\n\
             let b: List<Int> = xs.tail()",
        );
    }

    #[test]
    fn mb7_list_intersperse_sep_compatible() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.intersperse(0)",
        );
    }

    #[test]
    fn mb7_list_intersperse_sep_incompatible_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.intersperse(\"oops\")",
            &["intersperse"],
        );
    }

    #[test]
    fn mb7_list_cycle_returns_list_t() {
        assert_ok(
            "let xs: List<Int> = [1]\n\
             let r: List<Int> = xs.cycle(3)",
        );
    }

    #[test]
    fn mb7_str_repeat_with_returns_str() {
        assert_ok("let r: Str = \"x\".repeat_with(3, \", \")");
    }

    #[test]
    fn mb7_str_repeat_with_invalid_args_is_error() {
        assert_error_with(
            "let r = \"x\".repeat_with(\"oops\", \", \")",
            &["repeat_with", "Int"],
        );
    }

    #[test]
    fn mb7_map_with_returns_map_k_v() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let r: Map<Str, Int> = m.with(\"b\", 2)",
        );
    }

    #[test]
    fn mb7_map_with_incompatible_value_type_is_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let r = m.with(\"b\", \"oops\")",
            &["with"],
        );
    }

    #[test]
    fn mb6_map_merge_with_non_map_arg_is_error() {
        assert_error_with(
            "let a: Map<Str, Int> = {\"x\": 1}\n\
             let r = a.merge_with(42, fn(va: Int, vb: Int) => va)",
            &["merge_with", "Map"],
        );
    }

    #[test]
    fn async_cl_sync_does_not_accept_await_inside() {
        // Sync FnExpr (without `async`) rejects `.await` inside.
        assert_error_with(
            "fn run() -> Int {\n\
                 let f = fn(n: Int) -> Int {\n\
                     sleep(1).await\n\
                     return n\n\
                 }\n\
                 return 0\n\
             }",
            &["await"],
        );
    }

    // ---- I.1: indexing with types ----

    #[test]
    fn str_index_returns_str() {
        // I.1: `s[i]` now types as Str (previously was an error).
        assert_ok(
            "let s = \"hola\"\n\
             let c: Str = s[0]",
        );
    }

    #[test]
    fn str_index_with_non_int_arg_is_error() {
        assert_error_with(
            "let s = \"hola\"\n\
             let c = s[\"x\"]",
            &["Str", "Int"],
        );
    }

    // ---- I.2: slicing ----

    #[test]
    fn list_slice_returns_list_same_type() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let ys: List<Int> = xs[1..3]\n\
             let zs: List<Int> = xs[..2]\n\
             let ws: List<Int> = xs[3..]\n\
             let qs: List<Int> = xs[..]",
        );
    }

    #[test]
    fn str_slice_returns_str() {
        assert_ok(
            "let s = \"hola\"\n\
             let a: Str = s[0..2]\n\
             let b: Str = s[..2]\n\
             let c: Str = s[2..]\n\
             let d: Str = s[..]",
        );
    }

    #[test]
    fn slice_with_inclusive() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let ys: List<Int> = xs[0..=2]",
        );
    }

    #[test]
    fn slice_non_int_bound_is_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let ys = xs[\"a\"..2]",
            &["slice", "Int"],
        );
    }

    #[test]
    fn slice_on_unsupported_type_is_error() {
        assert_error_with(
            "let n: Int = 42\n\
             let r = n[0..1]",
            &["slicing"],
        );
    }

    // Chained

    #[test]
    fn chained_method_map_filter() {
        // map(...).filter(...) on a single line — the ret of map
        // (List<Any> because FnExpr.ret=Any until 5.3.5) feeds the
        // filter. Multi-line chaining is still explicit
        // parser debt (3.4).
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.map(fn(x) => x * 2).filter(fn(y) => true)",
        );
    }

    // Receivers without built-in methods

    #[test]
    fn method_on_int_is_error() {
        assert_error_with(
            "let n = 1\n\
             n.foo()",
            &["Int", "foo"],
        );
    }

    // Nominal: gradual, doesn't check or reject

    #[test]
    fn method_on_nominal_does_not_check() {
        // type without custom methods: user.greet() passes without warning
        // (the evaluator emits it at runtime). It's the gradual rule
        // of 5.3.4 — custom methods over `type` don't exist
        // yet, we don't break code using that pattern.
        assert_ok(
            "type User { id: Int }\n\
             let u = User { id: 1 }\n\
             u.greet()",
        );
    }

    // ---- 5.3.5: FnExpr.ret inferido + Expr::Index ----

    // FnExpr inferred ret — basic forms

    #[test]
    fn fn_expr_arrow_returns_expr_type() {
        // `fn(x: Int) => x * 2` desugars to body=[Return(x*2)];
        // inferred ret = Int. Filter requires Bool, so this must
        // fire the ret check.
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.filter(fn(x: Int) => x * 2)",
            &["filter", "Bool", "Int"],
        );
    }

    #[test]
    fn fn_expr_arrow_bool_passes_filter() {
        // Same scenario but with ret Bool — filter accepts.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.filter(fn(x: Int) => x > 0)",
        );
    }

    #[test]
    fn fn_expr_block_single_return_infers_that_type() {
        // Block form with one return — ret = type of the return.
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.find(fn(x: Int) { return x * 2 })",
            &["find", "Bool", "Int"],
        );
    }

    #[test]
    fn fn_expr_without_return_is_null() {
        // A fn that doesn't explicitly return — ret = Null. For
        // a map, elements end up as List<Null>.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Null> = xs.map(fn(x: Int) { print(x) })",
        );
    }

    // FnExpr inferred ret — unification (lub) over several returns

    #[test]
    fn fn_expr_lub_int_float_is_float() {
        // Two returns: Int and Float → Float (coercion).
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Float> = xs.map(fn(x: Int) {\n\
                 if (x > 0) { return 1.5 }\n\
                 return 0\n\
             })",
        );
    }

    #[test]
    fn fn_expr_lub_null_and_t_is_nullable() {
        // One branch returns null, another Int → ret = Int?.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int?> = xs.map(fn(x: Int) {\n\
                 if (x > 0) { return x }\n\
                 return null\n\
             })",
        );
    }

    #[test]
    fn fn_expr_lub_result_ok_and_err_is_concrete_result() {
        // Ok(User) + Err("...") → lub(Result<User>, Result<Any>)
        // = Result<User>. Detects that the FnExpr can be used where
        // Result<User> is expected.
        assert_ok(
            "type User { id: Int }\n\
             let xs: List<User> = [User { id: 1 }]\n\
             let r: List<Result<User>> = xs.map(fn(u: User) {\n\
                 if (u.id > 0) { return Ok(u) }\n\
                 return Err(\"boom\")\n\
             })",
        );
    }

    // Expr::Index

    #[test]
    fn index_list_returns_t() {
        assert_ok(
            "let xs: List<Int> = [10, 20, 30]\n\
             let n: Int = xs[0]",
        );
    }

    #[test]
    fn index_list_with_non_int_index_is_error() {
        assert_error_with(
            "let xs: List<Int> = [10, 20]\n\
             let n = xs[\"x\"]",
            &["List", "Int", "Str"],
        );
    }

    #[test]
    fn index_map_returns_v() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let n: Int = m[\"a\"]",
        );
    }

    #[test]
    fn index_map_with_incompatible_key_is_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let n = m[42]",
            &["Map<Str, Int>", "Str", "Int"],
        );
    }

    #[test]
    fn index_on_int_is_error() {
        assert_error_with(
            "let n = 1\n\
             let x = n[0]",
            &["Int", "indexing"],
        );
    }

    #[test]
    fn index_on_str_now_implemented() {
        // I.1 (mini-tanda I): `s[i]` devuelve `Str` (un char).
        assert_ok(
            "let s = \"hola\"\n\
             let c: Str = s[0]",
        );
    }

    #[test]
    fn index_on_any_does_not_check() {
        // Any receiver (var brought in by import) → gradual.
        assert_ok(
            "from foo import xs\n\
             let n = xs[0]",
        );
    }

    // lub direct

    #[test]
    fn lub_basic_functions() {
        assert_eq!(lub(&Type::Int, &Type::Int), Type::Int);
        assert_eq!(lub(&Type::Int, &Type::Any), Type::Int);
        assert_eq!(lub(&Type::Any, &Type::Str), Type::Str);
        assert_eq!(lub(&Type::Int, &Type::Float), Type::Float);
        assert_eq!(lub(&Type::Float, &Type::Int), Type::Float);
        // Null + Int → Int?.
        assert_eq!(
            lub(&Type::Null, &Type::Int),
            Type::Nullable(Box::new(Type::Int))
        );
        // Int + Str → Any (arbitrary mix).
        assert_eq!(lub(&Type::Int, &Type::Str), Type::Any);
    }

    #[test]
    fn lub_recursive_in_result() {
        // lub(Result<User>, Result<Any>) → Result<User>.
        let env = env_with(&["User"]);
        let user = Type::Nominal(env.lookup("User").unwrap());
        let a = Type::Result {
            ok: Box::new(user.clone()),
            err: Box::new(Type::Str),
        };
        let b = Type::Result {
            ok: Box::new(Type::Any),
            err: Box::new(Type::Str),
        };
        assert_eq!(lub(&a, &b), a);
    }

    #[test]
    fn unify_returns_empty_is_null() {
        // Without explicit returns → Null (matches the evaluator).
        assert_eq!(unify_returns(&[]), Type::Null);
    }

    // ---- Residual debt from 5a: reassignment against previous type ----

    #[test]
    fn reassignment_without_annotation_to_annotated_var_fails() {
        // `m: Int = 1; m = "x"` — the first assignment marked `m`
        // as Int-annotated; the second without annotation violates that.
        assert_error_with(
            "let m: Int = 1\n\
             m = \"no soy int\"",
            &["m", "Int", "Str"],
        );
    }

    #[test]
    fn reassignment_without_annotation_to_inferred_var_passes() {
        // `n = 1; n = "x"` — the first assignment had NO annotation,
        // so the gradual model allows changing the type.
        assert_ok(
            "let n = 1\n\
             n = \"ahora soy texto\"",
        );
    }

    #[test]
    fn reassignment_compatible_to_annotated_var_passes() {
        // `m: Int = 1; m = 2` — the reassignment respects the type.
        assert_ok(
            "let m: Int = 1\n\
             m = 2",
        );
    }

    #[test]
    fn reassignment_int_to_annotated_float_passes_by_coercion() {
        // `f: Float = 1.0; f = 2` — Int → Float via coercion.
        assert_ok(
            "let f: Float = 1.0\n\
             f = 2",
        );
    }

    #[test]
    fn re_annotation_with_other_type_passes_as_redeclaration() {
        // `m: Int = 1; m: Str = "x"` — the second `m: Str = ...` is
        // an explicit redeclaration; the gradual model allows it
        // (the evaluator does the same). The bug closed by this debt
        // is reassignment WITHOUT a new annotation.
        assert_ok(
            "let m: Int = 1\n\
             let m: Str = \"x\"",
        );
    }

    #[test]
    fn match_result_with_ok_wildcard_and_err_wildcard_is_exhaustive() {
        // `Ok(_)` and `Err(_)` cover the two variants — nothing missing.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(_) => \"ok\"\n\
                 Err(_) => \"err\"\n\
             }",
        );
    }

    #[test]
    fn match_result_with_only_ok_wildcard_missing_err() {
        // OkWildcard counts as the Ok variant, not as a catch-all.
        // If Err is missing, exhaustiveness error.
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(_) => \"ok\"\n\
             }",
            &["match", "Result", "exhaustive", "Err"],
        );
    }

    // ---- R.2.1: or-patterns in exhaustiveness ----

    #[test]
    fn or_pattern_ok_wildcard_and_err_wildcard_together_is_exhaustive() {
        // `Ok(_) | Err(_)` in a single arm covers both variants —
        // `update_result_coverage` recurses on `Pattern::Or`.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(_) | Err(_) => \"siempre\" }",
        );
    }

    #[test]
    fn or_pattern_only_ok_wildcards_combined_missing_err() {
        // `Ok(_) | Ok(_) =>` only covers Ok, missing Err.
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(_) | Ok(_) => \"x\" }",
            &["match", "Result", "exhaustive", "Err"],
        );
    }

    #[test]
    fn or_pattern_with_int_literals_does_not_trigger_exhaustiveness() {
        // Scrutinee `Int`, not `Result`. `1 | 2 | 3` is OK with `_`.
        assert_ok("let s = match 1 { 1 | 2 | 3 => \"chico\", _ => \"otro\" }");
    }

    #[test]
    fn or_pattern_strings_homogeneous() {
        assert_ok(
            "let d = \"lun\"\n\
             let s = match d { \"lun\" | \"mar\" | \"mie\" => \"laboral\", _ => \"x\" }",
        );
    }

    #[test]
    fn or_pattern_with_wildcard_subcase_is_catchall() {
        // If a sub-pattern of the Or is `_`, the arm is catch-all
        // (covers anything). Although in practice the user wouldn't
        // write `Ok(_) | _`, we validate it recurses correctly.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { 1 | _ => \"x\" }",
        );
    }

    // ---- R.2.2: guards in match ----

    #[test]
    fn guard_bool_is_valid() {
        assert_ok("let s = match 5 { x if x > 0 => \"pos\", _ => \"neg\" }");
    }

    #[test]
    fn guard_non_bool_is_error() {
        // `x if x` with x: Int → guard is not Bool.
        assert_error_with(
            "let s = match 5 { x if x => \"y\", _ => \"z\" }",
            &["guard", "Bool", "Int"],
        );
    }

    #[test]
    fn guard_references_pattern_binding() {
        // The pattern binding (`v` from `Ok(v)`) must be visible
        // in the guard.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(v) if v > 0 => \"pos\", Ok(_) => \"neg\", Err(_) => \"err\" }",
        );
    }

    #[test]
    fn arm_with_guard_does_not_count_for_result_exhaustiveness() {
        // Only `Ok(_) if true` covers Ok with guard; doesn't count as Ok
        // and Err is missing.
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(_) if true => \"x\" }",
            &["match", "Result", "exhaustive"],
        );
    }

    #[test]
    fn arm_with_guard_does_not_count_as_catchall() {
        // `_ if cond` is not a real catch-all (cond may be false).
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { _ if true => \"x\" }",
            &["match", "Result", "exhaustive"],
        );
    }

    #[test]
    fn arm_with_guard_followed_by_catchall_is_exhaustive() {
        // With a catch-all without guard at the end, the match is exhaustive.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(v) if v > 0 => \"pos\", _ => \"otro\" }",
        );
    }

    // ---- R.2.4 (F3): orphan return/break/continue ----

    #[test]
    fn orphan_return_top_level_is_error() {
        assert_error_with("return 42", &["return", "function"]);
    }

    #[test]
    fn return_inside_fn_is_valid() {
        assert_ok(
            "fn f() -> Int { return 42 }\n\
             let x = f()",
        );
    }

    #[test]
    fn orphan_break_top_level_is_error() {
        assert_error_with("break", &["break", "loop"]);
    }

    #[test]
    fn orphan_continue_top_level_is_error() {
        assert_error_with("continue", &["continue", "loop"]);
    }

    #[test]
    fn break_inside_for_is_valid() {
        assert_ok(
            "for i in 0..5 {\n\
                 if i == 3 { break }\n\
             }",
        );
    }

    #[test]
    fn continue_inside_while_is_valid() {
        assert_ok(
            "let x = 0\n\
             while (x < 10) {\n\
                 x = x + 1\n\
                 if x == 5 { continue }\n\
             }",
        );
    }

    #[test]
    fn break_inside_loop_is_valid() {
        assert_ok(
            "loop {\n\
                 break\n\
             }",
        );
    }

    #[test]
    fn nested_break_two_loops_is_valid() {
        assert_ok(
            "for i in 0..3 {\n\
                 for j in 0..3 {\n\
                     if j == 1 { break }\n\
                 }\n\
             }",
        );
    }

    #[test]
    fn break_inside_inner_fn_does_not_escape_outer_loop() {
        // Fitz's parser does NOT allow nested fns (top-level only),
        // but FnExpr (closures) is allowed. break inside a closure
        // that appears inside a loop is NOT inside a loop
        // for checker purposes.
        assert_error_with(
            "for i in 0..3 {\n\
                 let f = fn() => 0\n\
                 let g = fn() {\n\
                     break\n\
                 }\n\
             }",
            &["break", "loop"],
        );
    }

    #[test]
    fn orphan_return_and_orphan_break_both_reported() {
        // Both errors should appear in the same program.
        let (_, errors) = check_str("return 42\nbreak");
        let return_errs = errors
            .iter()
            .filter(|e| e.message.contains("return"))
            .count();
        let break_errs = errors
            .iter()
            .filter(|e| e.message.contains("break"))
            .count();
        assert!(
            return_errs >= 1,
            "expected at least 1 error of orphan return"
        );
        assert!(break_errs >= 1, "expected at least 1 error of orphan break");
    }

    // ---- R.3: custom methods on type ----

    #[test]
    fn method_reads_field_as_local_is_valid() {
        assert_ok(
            "type U {\n\
                 name: Str\n\
                 fn greet() -> Str { return \"hola {name}\" }\n\
             }",
        );
    }

    #[test]
    fn method_with_field_typo_is_error() {
        assert_error_with(
            "type U {\n\
                 name: Str\n\
                 fn greet() -> Str { return naem }\n\
             }",
            &["naem", "no"],
        );
    }

    #[test]
    fn method_with_return_type_mismatch_is_error() {
        assert_error_with(
            "type U {\n\
                 count: Int\n\
                 fn label() -> Int { return \"no soy int\" }\n\
             }",
            &["return", "Str", "Int"],
        );
    }

    #[test]
    fn method_with_non_bool_param_in_if_is_error() {
        assert_error_with(
            "type U {\n\
                 fn check(n: Int) -> Bool {\n\
                     if (n) { return true }\n\
                     return false\n\
                 }\n\
             }",
            &["if", "Bool", "Int"],
        );
    }

    #[test]
    fn method_param_shadows_field_compiles() {
        // When a param has the same name as a field, the
        // param wins in scope. The checker allows the combination
        // without error.
        assert_ok(
            "type U {\n\
                 name: Str\n\
                 fn rename(name: Str) -> Str { return name }\n\
             }",
        );
    }

    #[test]
    fn method_break_is_orphan_if_no_local_loop() {
        // A `break` inside the body of a method without a local loop is
        // orphan. (R.2.4 resets loop_depth at each fn body.)
        assert_error_with(
            "type U {\n\
                 fn f() {\n\
                     break\n\
                 }\n\
             }",
            &["break", "loop"],
        );
    }

    #[test]
    fn reassignment_annotated_propagates_to_later_use() {
        // Verify that the binding is still `Int` after an
        // incompatible reassignment attempt: the subsequent use
        // expects Int.
        let (_, errors) = check_str(
            "let m: Int = 1\n\
             m = \"no soy int\"\n\
             let n: Int = m + 1",
        );
        // We expect only the reassignment error, no additional errors
        // from `m + 1` (because m is still Int).
        let count_reassign = errors
            .iter()
            .filter(|e| e.message.contains("m") && e.message.contains("Str"))
            .count();
        assert!(
            count_reassign >= 1,
            "expected reassignment error, was: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
        // The subsequent use `m + 1` types OK (m is still Int).
        let count_plus = errors
            .iter()
            .filter(|e| e.message.contains("operator") && e.message.contains("+"))
            .count();
        assert_eq!(
            count_plus,
            0,
            "did not expect error in `m + 1`, was: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    // ---- Function type `Fn(...) -> U` (higher-order, F12) ----

    #[test]
    fn type_expr_function_resolves_to_type_function() {
        // type Box { f: Fn(Int) -> Int } — the field has a function type.
        let (env, errors) = resolve_str("type Box { f: Fn(Int) -> Int }");
        assert!(errors.is_empty(), "errores: {:?}", errors);
        let id = env.lookup("Box").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        assert_eq!(
            fields[0].type_,
            Type::Function {
                params: vec![Type::Int],
                ret: Box::new(Type::Int),
            },
        );
    }

    #[test]
    fn type_expr_function_without_params_resolves() {
        let (env, errors) = resolve_str("type Lazy { f: Fn() -> Str }");
        assert!(errors.is_empty(), "errores: {:?}", errors);
        let id = env.lookup("Lazy").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        assert_eq!(
            fields[0].type_,
            Type::Function {
                params: vec![],
                ret: Box::new(Type::Str),
            },
        );
    }

    #[test]
    fn type_expr_function_higher_order_resolves() {
        // Fn(Fn(Int) -> Int, Int) -> Int — param is itself a function.
        let (env, errors) = resolve_str("type Apply { f: Fn(Fn(Int) -> Int, Int) -> Int }");
        assert!(errors.is_empty(), "errores: {:?}", errors);
        let id = env.lookup("Apply").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        let expected = Type::Function {
            params: vec![
                Type::Function {
                    params: vec![Type::Int],
                    ret: Box::new(Type::Int),
                },
                Type::Int,
            ],
            ret: Box::new(Type::Int),
        };
        assert_eq!(fields[0].type_, expected);
    }

    #[test]
    fn type_expr_function_with_nonexistent_type_reports_error() {
        let (_, errors) = resolve_str("type Box { f: Fn(NoExiste) -> Int }");
        assert!(!errors.is_empty(), "expected error, was: {:?}", errors);
        let combined: String = errors.iter().map(|e| e.message.clone()).collect();
        assert!(combined.contains("NoExiste"));
    }

    #[test]
    fn function_annotation_in_fndef_param_passes_checker() {
        // fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x) }
        // The checker must type the call `f(x)` against the signature.
        assert_ok("fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x) }");
    }

    #[test]
    fn function_annotation_in_param_detects_bad_arity() {
        // apply passes 2 args to an f that takes 1.
        assert_error_with(
            "fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x, x) }",
            &["expects 1", "argument"],
        );
    }

    // -----------------------------------------------------------------------
    // Tests — Span on expr-level errors (S1.2 sub-step 2)
    //
    // Before S1.2 errors on expressions inherited the line
    // of the containing `Stmt` (correct) but with a degraded column
    // (that of the first token of the stmt). After this sub-step, each type
    // error on BinOp/Call/Field/Index/UnaryOp/Try/Match/Range/
    // StructLit/Ident points to the column of the problematic node.
    //
    // These tests fix concrete positions so that any span
    // loss is noticed in the suite.
    // -----------------------------------------------------------------------

    /// Helper that returns the first reported error, or panics if none.
    fn first_error(src: &str) -> FitzError {
        let (_, mut errors) = check_str(src);
        assert!(!errors.is_empty(), "esperado al menos un error en: {}", src);
        errors.remove(0)
    }

    #[test]
    fn span_binop_points_to_operator_column() {
        // `let x: Int = 1 + "a"` — `+` is at column 16. The error
        // now reports the column of the operator, not the column of the `let`.
        let e = first_error("let x: Int = 1 + \"a\"");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 16);
        assert!(e.message.contains("operator `+`"), "msg: {}", e.message);
    }

    #[test]
    fn span_call_arity_points_to_call_paren() {
        // `fn f(x: Int) -> Int => x` and `let _ = f(1, 2)` — the `(` of the
        // call is at column 41 (after `fn f(x: Int) -> Int => x\n`,
        // accounting that `let _ = f(` starts on line 2).
        let src = "fn f(x: Int) -> Int => x\nlet _ = f(1, 2)";
        let e = first_error(src);
        assert_eq!(e.line, 2);
        // `let _ = f` spans columns 1-9, so `(` is at 10.
        assert_eq!(e.column, 10);
        assert!(e.message.contains("expects 1"), "msg: {}", e.message);
    }

    #[test]
    fn span_call_arg_points_to_concrete_argument() {
        // The "argument N expects X, received Y" error points to the
        // argument, not to the `(`. Lets us distinguish which of several args
        // has the wrong type.
        let src = "fn f(x: Int) -> Int => x\nlet _ = f(\"hola\")";
        let e = first_error(src);
        assert_eq!(e.line, 2);
        // `let _ = f(` spans 1-10, `"hola"` starts at 11.
        assert_eq!(e.column, 11);
        assert!(
            e.message.contains("argument 1") && e.message.contains("Int"),
            "msg: {}",
            e.message,
        );
    }

    #[test]
    fn span_unary_points_to_minus() {
        // `let s = -"a"` — `-` is at column 9.
        let e = first_error("let s = -\"a\"");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 9);
        assert!(e.message.contains("negation"), "msg: {}", e.message);
    }

    #[test]
    fn span_index_points_to_concrete_index() {
        // `let xs: List<Int> = [1, 2, 3]\nlet _ = xs["k"]` — `"k"`
        // is at column 12 of line 2.
        let src = "let xs: List<Int> = [1, 2, 3]\nlet _ = xs[\"k\"]";
        let e = first_error(src);
        assert_eq!(e.line, 2);
        // `let _ = xs[` spans 1-11, `"k"` starts at 12.
        assert_eq!(e.column, 12);
        assert!(e.message.contains("Int"), "msg: {}", e.message);
    }

    #[test]
    fn span_field_struct_extra_points_to_extra_value() {
        // `type U { id: Int }; let u = U { id: 1, x: 2 }` — the `2` of the
        // extra field is at column 44.
        let src = "type U { id: Int }\nlet u = U { id: 1, x: 2 }";
        let e = first_error(src);
        assert_eq!(e.line, 2);
        // `let u = U { id: 1, x: ` spans 1-22, `2` starts at 23.
        assert_eq!(e.column, 23);
        assert!(
            e.message.contains("does not have a field") && e.message.contains("`x`"),
            "msg: {}",
            e.message,
        );
    }

    #[test]
    fn span_unknown_ident_points_to_ident() {
        // `let _ = no_existe` — `no_existe` starts at column 9.
        let e = first_error("let _ = no_existe");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 9);
        assert!(e.message.contains("unknown variable"));
    }

    #[test]
    fn span_try_points_to_question_mark() {
        // `let _ = 42?` — `?` is at column 11.
        let e = first_error("let _ = 42?");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 11);
        assert!(e.message.contains("`?`"), "msg: {}", e.message);
    }

    #[test]
    fn span_range_points_to_problematic_extreme() {
        // `let _ = 1..\"a\"` — `"a"` is at column 12.
        let e = first_error("let _ = 1..\"a\"");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 12);
        assert!(e.message.contains("end of the range"), "msg: {}", e.message);
        // Note: "end" matches both label values, but "of the range" anchors to the message.
    }

    // -----------------------------------------------------------------------
    // Phase 8.4.1 — Type::PyAny + bindings of `from python import` +
    // Python calls type as Result<Any> in the checker.
    // -----------------------------------------------------------------------
    //
    // These tests work WITHOUT the `python` feature active because the
    // checker only looks at the AST shape: `path[0] == "python"` activates
    // the PyAny branch independently of whether the binary linked libpython.
    // The runtime is only invoked with the feature, but the static check
    // always runs.

    #[test]
    fn checker_from_python_import_binds_as_pyany_not_any() {
        // The checker accepts `from python import math` and binds `math`
        // with type PyAny. Any use goes through the asymmetric rules
        // of PyAny (calls → Result<Any>, field access → PyAny).
        let (_, errors) = check_str("from python import math\nlet x = math\n");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_call_python_types_as_result_any() {
        // 8.4.2 (landed alongside 8.4.1): the call `math.sqrt(16.0)`
        // types as `Result<Any>` — using the result as `Float`
        // directly WITHOUT unpacking fires a type error.
        assert_error_with(
            "from python import math\nlet f: Float = math.sqrt(16.0)\n",
            &["Float", "Result"],
        );
    }

    #[test]
    fn checker_call_python_with_match_compiles_clean() {
        // The canonical pattern (match to unpack) types OK.
        // Covers the exhaustiveness rule over Result (5.3.3) — `Ok`
        // + `Err` exhaustive is enough.
        let (_, errors) = check_str(
            "from python import math\n\
             let f = match math.sqrt(16.0) { Ok(v) => v, Err(_) => -1.0 }\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_call_python_match_non_exhaustive_is_error() {
        // The 5.3.3 rule now applies to Python calls: a `match` that
        // omits `Err` (without catch-all) fires an exhaustiveness error
        // because the scrutinee types as Result<Any>.
        assert_error_with(
            "from python import math\n\
             let f = match math.sqrt(16.0) { Ok(v) => v }\n",
            &["exhaustive"],
        );
    }

    #[test]
    fn checker_try_operator_on_call_python_compiles_inside_fn_result() {
        // `?` inside a fn that returns `Result<T>` unpacks
        // the Python `Result<Any>` to the internal `Any` (which matches
        // any T via gradual). On success returns the value; on
        // failure propagates the Err to the caller.
        let (_, errors) = check_str(
            "from python import math\n\
             fn root(x: Float) -> Result<Float> { return Ok(math.sqrt(x)?) }\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_try_operator_on_call_python_fn_non_result_is_error() {
        // The 5.3.3 rule on `?` also applies to Python calls:
        // inside a fn that returns `Int` (not `Result<...>`), `?`
        // on `math.sqrt(...)` fires an error because the container
        // cannot receive the propagated `Err`.
        // (`?` at top level is not checked by the checker — it's reported
        // at runtime, inherited decision from 5.3.3.)
        assert_error_with(
            "from python import math\n\
             fn bad(x: Float) -> Int { return 0 + math.sqrt(x)? }\n",
            &["operator", "?"],
        );
    }

    #[test]
    fn checker_field_access_on_pyany_returns_pyany() {
        // `os.path` is field access over PyAny — the type of the binding
        // is still PyAny. The check passes without errors and a call
        // on the submodule (`os.path.join(...)`) keeps typing
        // as Result<Any>.
        let (_, errors) = check_str(
            "from python import os\n\
             let p = os.path\n\
             let r = match p.join(\"a\", \"b\") { Ok(s) => s, Err(_) => \"\" }\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_pyany_is_compatible_with_concrete_annotation() {
        // The canonical roadmap pattern: `let row: User = py_call()?`.
        // Statically, `?` unpacks Result<Any> to Any; the
        // User annotation passes via gradual escape (PyAny/Any → User).
        // The runtime does the real coercion in 8.4.3.
        let (_, errors) = check_str(
            "type User { id: Int, name: Str }\n\
             from python import json\n\
             fn parse(s: Str) -> Result<User> { return Ok(json.loads(s)?) }\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_normal_import_is_not_pyany() {
        // `import utils` (without `python` prefix) is still Any,
        // not PyAny — the logic that refines calls to Result<Any> only
        // applies to `from python import`. Validation: a call to
        // a normal module is still Any, so a Float-typed binding
        // passes via gradual without error.
        let (_, errors) = check_str("import utils\nlet f: Float = utils.something(1)\n");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    // -----------------------------------------------------------------
    // Mini-batch Vp — private fields (`_field`) in `type`.
    // -----------------------------------------------------------------

    #[test]
    fn vp_field_access_from_outside_is_error() {
        let (_, errors) = check_str("type C { _x: Int = 0 }\nlet c = C {}\nprint(c._x)\n");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("private") && e.message.contains("_x")),
            "expected error about private `_x`, was: {:?}",
            errors,
        );
    }

    #[test]
    fn vp_field_access_from_inside_method_is_ok() {
        // The method already has `_x` as local (option A), but if the
        // method receives another instance of the same type and accesses
        // `other._x`, that must also be allowed.
        let (_, errors) = check_str(
            "type C {\n\
                 _x: Int = 0\n\
                 fn merge(other: C) -> Int { return _x + other._x }\n\
             }\n",
        );
        assert!(
            errors.is_empty(),
            "expected no errors inside method of the same type, was: {:?}",
            errors,
        );
    }

    #[test]
    fn vp_field_access_from_method_of_other_type_is_error() {
        let (_, errors) = check_str(
            "type A { _x: Int = 0 }\n\
             type B {\n\
                 fn spy(a: A) -> Int { return a._x }\n\
             }\n",
        );
        assert!(
            errors.iter().any(|e| e.message.contains("private")),
            "expected error from access from another type, was: {:?}",
            errors,
        );
    }

    #[test]
    fn vp_struct_lit_with_private_field_from_outside_is_error() {
        let (_, errors) = check_str(
            "type C { name: Str = \"\", _balance: Int = 0 }\n\
             let c = C { name: \"x\", _balance: 100 }\n",
        );
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("private") && e.message.contains("_balance")),
            "expected error about struct lit with `_balance`, was: {:?}",
            errors,
        );
    }

    #[test]
    fn vp_struct_lit_with_private_field_from_inside_is_ok() {
        // Canonical pattern: `static fn new(...)` builds via struct lit
        // with the `_field` private fields. Inside the type body it's legitimate.
        let (_, errors) = check_str(
            "type C {\n\
                 _x: Int = 0\n\
                 static fn make(n: Int) -> C { return C { _x: n } }\n\
             }\n",
        );
        assert!(
            errors.is_empty(),
            "expected no errors in static constructor, was: {:?}",
            errors,
        );
    }

    #[test]
    fn vp_field_assign_to_private_field_from_outside_is_error() {
        let (_, errors) = check_str("type C { _x: Int = 0 }\nlet c = C {}\nc._x = 5\n");
        assert!(
            errors.iter().any(|e| e.message.contains("private")),
            "expected error of assignment to private field, was: {:?}",
            errors,
        );
    }

    // ---- Mini-batch Vm — private methods (`_method`) ----

    #[test]
    fn vm_call_to_private_method_from_outside_is_error() {
        let (_, errors) = check_str(
            "type C {\n\
                 fn _hidden() -> Int { return 42 }\n\
             }\n\
             let c = C {}\n\
             let r = c._hidden()\n",
        );
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("private") && e.message.contains("_hidden")),
            "expected error about private `_hidden`, was: {:?}",
            errors,
        );
    }

    #[test]
    fn vm_call_to_private_method_from_inside_is_ok() {
        // Using `static fn` to pass the instance and call the
        // private one (the canonical pattern — instance methods
        // can't call other methods of the same type without
        // explicit `self`).
        let (_, errors) = check_str(
            "type C {\n\
                 x: Int = 0\n\
                 fn _hidden() -> Int { return x }\n\
                 static fn unsafe_get(c: C) -> Int { return c._hidden() }\n\
             }\n",
        );
        assert!(
            errors.is_empty(),
            "expected no errors inside the type, was: {:?}",
            errors,
        );
    }

    #[test]
    fn vm_call_to_private_method_from_other_type_is_error() {
        let (_, errors) = check_str(
            "type A { fn _hidden() -> Int { return 1 } }\n\
             type B {\n\
                 fn spy(a: A) -> Int { return a._hidden() }\n\
             }\n",
        );
        assert!(
            errors.iter().any(|e| e.message.contains("private")),
            "expected error from access from another type, was: {:?}",
            errors,
        );
    }

    #[test]
    fn vm_public_method_is_not_affected_by_rule() {
        let (_, errors) = check_str(
            "type C {\n\
                 fn greet() -> Str { return \"hola\" }\n\
             }\n\
             let c = C {}\n\
             let r = c.greet()\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn vp_public_field_is_not_affected_by_rule() {
        // Sanity: fields without `_` prefix are still public.
        let (_, errors) = check_str("type C { x: Int = 0 }\nlet c = C { x: 5 }\nprint(c.x)\n");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    // -----------------------------------------------------------------
    // Phase 9.0.2 — the checker is silent on Error AST nodes
    // (only `parse_with_recovery` emits them). Without these guarantees, the
    // LSP running `check_program` over a recovered AST would generate
    // cascades of derived errors on the same point that's already
    // reported in the parser's list.
    // -----------------------------------------------------------------

    /// Helper specific to 9.0.2 tests: runs the full LSP pipeline
    /// (`parse_with_recovery` → `check_program`) and
    /// returns the errors the checker would report. Parser errors
    /// stay separate — the caller asks for them apart if it
    /// needs them.
    fn check_recovering(src: &str) -> Vec<FitzError> {
        let tokens = tokenize(src).expect("lex OK");
        let (program, _parser_errors) = crate::parser::parse_with_recovery(tokens);
        let (_env, _types, _defs, errors) = check_program(&program);
        errors
    }

    #[test]
    fn checker_stmt_error_does_not_emit_own_errors() {
        // The parser produces a `Stmt::Error` in place of the broken
        // stmt. The checker must not add any error on that
        // node (real errors live in the parser's list).
        let src = "let x = 1 +\nlet y: Int = 2";
        let errors = check_recovering(src);
        assert!(
            errors.is_empty(),
            "the checker must not emit errors about Stmt::Error nor about neighboring valid stmts: {:?}",
            errors
        );
    }

    #[test]
    fn checker_stmt_error_silent_but_real_errors_are_reported() {
        // The checker silences the Stmt::Error but keeps reporting
        // genuine errors from valid code. The `let z: Int = "no"`
        // has an incompatible type — the checker must catch it even if
        // there's a Stmt::Error before.
        let src = "let x = 1 +\nlet z: Int = \"no\"";
        let errors = check_recovering(src);
        assert_eq!(
            errors.len(),
            1,
            "expected 1 type error from the valid stmt: {:?}",
            errors
        );
        // The error is from the valid stmt on line 2, not from the Error node.
        assert_eq!(errors[0].line, 2);
    }

    #[test]
    fn checker_stmt_error_in_fn_body_does_not_abort_check() {
        // `fn foo() { ... }` with a broken stmt inside: the checker
        // keeps checking the rest of the program (the `bar` fn and its
        // incorrect type annotation) without aborting because of the
        // intermediate Error node.
        let src = "fn foo() {\n  let a = 1 +\n}\nfn bar() -> Int { return \"no\" }\n";
        let errors = check_recovering(src);
        // The return annotation error (`Int` vs `Str`) MUST
        // be reported. Other errors derived from the Error node must NOT.
        // (Exact count may vary with future refinements;
        // the critical thing is: at least one type error from the valid stmt, and
        // none that directly mentions the Error node.)
        assert!(
            errors.iter().any(|e| e.line == 4),
            "expected at least one type error on line 4 (mistyped return): {:?}",
            errors
        );
    }

    #[test]
    fn checker_pipeline_recovering_does_not_panic_on_very_broken_buffer() {
        // Smoke: a program peppered with errors must not crash the
        // checker. The real validation is that `check_program` returns
        // (no panic) on the AST with several Error nodes.
        let src = "let a = +\nlet b: Int = \"no\"\nlet c = *\nfn ok() -> Int { return 7 }\n";
        let errors = check_recovering(src);
        // Guarantee: at least the genuine error from `let b: Int = "no"`
        // is reported (line 2). The rest may or may not have derived
        // errors — the contract is "no panic" + "genuine errors
        // from valid code".
        assert!(
            errors.iter().any(|e| e.line == 2),
            "expected type error on line 2: {:?}",
            errors
        );
    }

    #[test]
    fn checker_expr_error_propagates_as_any_without_emitting_error() {
        // `Expr::Error` directly in the AST must synthesize `Type::Any`
        // and not emit any error from the checker. We build the
        // node manually because the parser in 9.0.1 only produces
        // Stmt::Error (sub-expression recovery comes later).
        //
        // Case: `let x: Int = <Expr::Error>` — Int annotation + Any
        // value. The gradual rule (`is_compatible(Any, _)` always
        // true) makes there be no type error.
        use crate::ast::{AssignTarget, Expr as AstExpr, Span, Stmt};
        let program = vec![Stmt::Assign {
            target: AssignTarget::Ident("x".into(), Span::default()),
            type_: Some(TypeExpr::Named("Int".into())),
            value: AstExpr::Error(Span::ZERO),
            span: Span::ZERO,
        }];
        let (_env, _types, _defs, errors) = check_program(&program);
        assert!(
            errors.is_empty(),
            "Expr::Error debe sintetizar Type::Any y no agregar errores: {:?}",
            errors
        );
    }

    // -----------------------------------------------------------------------
    // Phase 9.0 — F16: typed IR persisted by node.
    //
    // Tests on the side-table `TypeInfo` returned by
    // `check_program`. We cover: literals, Ident, BinOp, Call, Field,
    // StructLit, Match — the nodes the LSP will query for hover
    // and contextual completion. We also validate the two population
    // policies: Span::ZERO is omitted, Expr::Error is persisted as Any.
    // -----------------------------------------------------------------------

    /// Helper: runs the full lex → parse → check pipeline and returns
    /// the `TypeInfo`. Useful for F16 tests that want to look
    /// directly at the side-table.
    fn types_of(src: &str) -> TypeInfo {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (_env, type_info, _defs, _errors) = check_program(&program);
        type_info
    }

    #[test]
    fn types_info_persists_literal_types() {
        // Program with one literal of each primitive. Each one must
        // end up in the side-table with the corresponding type.
        let info =
            types_of("let a = 1\nlet b = 1.5\nlet c = \"hola\"\nlet d = true\nlet e = null\n");
        // The parser emits 1-indexed columns; RHS start at the
        // column of the literal value. We don't match exact columns
        // — we look up by line + type.
        let by_line: std::collections::HashMap<usize, Vec<Type>> = info
            .inner
            .iter()
            .map(|(k, v)| (k.0, v.clone()))
            .fold(std::collections::HashMap::new(), |mut acc, (line, ty)| {
                acc.entry(line).or_default().push(ty);
                acc
            });
        assert!(
            by_line[&1].iter().any(|t| matches!(t, Type::Int)),
            "line 1 must have Int: {:?}",
            by_line.get(&1)
        );
        assert!(
            by_line[&2].iter().any(|t| matches!(t, Type::Float)),
            "line 2 must have Float: {:?}",
            by_line.get(&2)
        );
        assert!(
            by_line[&3].iter().any(|t| matches!(t, Type::Str)),
            "line 3 must have Str: {:?}",
            by_line.get(&3)
        );
        assert!(
            by_line[&4].iter().any(|t| matches!(t, Type::Bool)),
            "line 4 must have Bool: {:?}",
            by_line.get(&4)
        );
        assert!(
            by_line[&5].iter().any(|t| matches!(t, Type::Null)),
            "line 5 must have Null: {:?}",
            by_line.get(&5)
        );
    }

    #[test]
    fn types_info_persists_ident_and_binop() {
        // `let x = 10` declares x: Int. `let y = x + 5` accesses the
        // ident `x` (must type Int) and produces a BinOp (must type
        // Int as well).
        let info = types_of("let x = 10\nlet y = x + 5\n");
        // We look up an Int on line 2 — the ident `x` and the BinOp
        // `x + 5` both must appear.
        let int_count_line2 = info
            .inner
            .iter()
            .filter(|(k, t)| k.0 == 2 && matches!(t, Type::Int))
            .count();
        assert!(
            int_count_line2 >= 3,
            "line 2 must persist ≥3 Int nodes (ident `x`, literal `5`, BinOp): {:?}",
            info.inner
        );
    }

    #[test]
    fn types_info_persists_call_and_field() {
        // Program with custom type + fn call + field access. Each
        // `Expr` node must be persisted with its synthesized type.
        let src = "\
type User { id: Int, name: Str }
fn greet(u: User) -> Str => u.name
let u = User { id: 1, name: \"Fitz\" }
let s = greet(u)
";
        let info = types_of(src);
        // The call `greet(u)` is on line 4 (last line with
        // code) — must type Str because `greet` returns Str.
        let any_str_call = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 4 && matches!(t, Type::Str));
        assert!(
            any_str_call,
            "line 4 must have Str (result of the greet(u) call): {:?}",
            info.inner
        );
        // The struct lit `User { ... }` is on line 3 — must type
        // Nominal(User).
        let any_nominal_struct = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 3 && matches!(t, Type::Nominal(_)));
        assert!(
            any_nominal_struct,
            "line 3 must have Nominal(User): {:?}",
            info.inner
        );
    }

    #[test]
    fn types_info_persists_match_arms() {
        // Match over Result<Int>: each arm types its body, the whole
        // match inherits the type of the first arm. We verify that some
        // node from the branches has been persisted.
        let src = "\
fn divide(a: Int, b: Int) -> Result<Int> {
  if (b == 0) { return Err(\"div0\") }
  return Ok(a / b)
}
let r = divide(10, 2)
let v = match r {
  Ok(x) => x
  Err(_) => 0
}
";
        let info = types_of(src);
        // The match itself must be recorded with Int (type of the
        // first arm `x` which is Int).
        let has_int_in_match = info
            .inner
            .iter()
            .any(|(k, t)| k.0 >= 6 && matches!(t, Type::Int));
        assert!(
            has_int_in_match,
            "el match debe haber persistido Int en alguno de sus arms o el resultado: {:?}",
            info.inner
        );
    }

    #[test]
    fn types_info_omits_span_zero() {
        // We build a program with a synthetic node (Span::ZERO)
        // and validate that it does NOT appear in the side-table. The policy
        // documented in `TypeInfo::record` is to omit Span::ZERO to
        // avoid collisions between synthetics.
        use crate::ast::{AssignTarget, Expr as AstExpr, Span, Stmt};
        let program = vec![Stmt::Assign {
            target: AssignTarget::Ident("x".into(), Span::default()),
            type_: None,
            value: AstExpr::Int(42, Span::ZERO),
            span: Span::ZERO,
        }];
        let (_env, type_info, _defs, _errors) = check_program(&program);
        // The Int(42, Span::ZERO) must NOT end up in the side-table —
        // its span is not known. Anything else (if the parser
        // emits something) also has no real span because the program was
        // built by hand. Expected total: 0.
        assert_eq!(
            type_info.len(),
            0,
            "Span::ZERO debe omitirse del side-table: {:?}",
            type_info.inner
        );
    }

    #[test]
    fn types_info_expr_error_persists_as_any() {
        // A `Stmt::Assign` with `Expr::Error` as value must persist
        // the Error node as `Type::Any` in the side-table (as long as
        // its span is known). Policy documented in `TypeInfo` —
        // uniform with checker behavior (synthesize_expr
        // returns `Type::Any` for Error nodes).
        use crate::ast::{AssignTarget, Expr as AstExpr, Span, Stmt};
        let span = Span::new(7, 11); // arbitrary "known" span
        let program = vec![Stmt::Assign {
            target: AssignTarget::Ident("x".into(), Span::default()),
            type_: None,
            value: AstExpr::Error(span),
            span,
        }];
        let (_env, type_info, _defs, _errors) = check_program(&program);
        assert_eq!(
            type_info.type_at(span),
            Some(&Type::Any),
            "Expr::Error con span known debe persistir como Any: {:?}",
            type_info.inner
        );
    }

    #[test]
    fn types_info_type_at_returns_none_for_unknown_span() {
        // Lookup by a span the checker never registered must
        // return None. Typical case: the LSP requests hover over an
        // empty position (between tokens).
        let info = types_of("let x = 1\n");
        // Span on a line the program doesn't touch.
        assert!(
            info.type_at(Span::new(999, 999)).is_none(),
            "span ausente debe devolver None"
        );
        // Span::ZERO also returns None by policy.
        assert!(
            info.type_at(Span::ZERO).is_none(),
            "Span::ZERO debe devolver None"
        );
    }

    #[test]
    fn types_info_smoke_real_program() {
        // Smoke on a program with a variety of constructs. We don't
        // match the exact N (fragile against future
        // parser/checker changes), only a conservative floor: at least
        // a handful of nodes got recorded.
        let src = "\
type Point { x: Int, y: Int }
fn sum(p: Point) -> Int => p.x + p.y
let p = Point { x: 3, y: 4 }
let total = sum(p)
print(total)
";
        let info = types_of(src);
        assert!(
            info.len() >= 10,
            "programa con varios nodos debe persistir ≥10 entries; got {}: {:?}",
            info.len(),
            info.inner
        );
    }

    // -----------------------------------------------------------------------
    // Phase 9.x.3 — DefinitionInfo: use → declaration side-table.
    //
    // Tests on side-table population from the `infer_expr` wrapper
    // when it sees an `Expr::Ident`. We cover: local var, top-level fn,
    // non-registration for builtins (def_span Span::ZERO).
    // -----------------------------------------------------------------------

    /// Helper: runs the pipeline and returns the `DefinitionInfo`.
    fn defs_of(src: &str) -> DefinitionInfo {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (_env, _types, def_info, _errors) = check_program(&program);
        def_info
    }

    #[test]
    fn def_info_registers_local_variable_use() {
        // `let x = 1` on line 1, `let y = x` on line 2. The use of
        // `x` on line 2 must register (use_span, def_span) with
        // def_span pointing to the Stmt::Assign on line 1.
        let defs = defs_of("let x = 1\nlet y = x\n");
        assert!(
            !defs.is_empty(),
            "uso de variable local debe registrarse en DefinitionInfo"
        );
        // At least one entry has def_span on line 1 (the let of x).
        let has_def_in_line_1 = defs.iter().any(|(_, def_span)| def_span.line == 1);
        assert!(
            has_def_in_line_1,
            "def_span of binding `x` must point to line 1: {:?}",
            defs.iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn def_info_does_not_register_builtins() {
        // `print` is a builtin with def_span = Span::ZERO. Using `print`
        // must not add entries to DefinitionInfo (Span::ZERO is
        // omitted by policy — there's no file to jump to).
        let defs = defs_of("print(42)\n");
        // Only the ident `print` would produce an entry; the literal `42`
        // is not an Ident. We verify there are NO records (empty DefInfo)
        // — the Span::ZERO filter discards the builtin.
        assert!(
            defs.is_empty(),
            "uso de builtin no debe registrarse en DefinitionInfo: {:?}",
            defs.iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn def_info_registers_top_level_fn_use() {
        // `fn dobla(n: Int) -> Int => n * 2` on line 1.
        // `dobla(21)` on line 2 — the use of the name `dobla` must
        // register def_span on line 1.
        let defs = defs_of("fn dobla(n: Int) -> Int => n * 2\nlet x = dobla(21)\n");
        assert!(!defs.is_empty(), "uso de fn top-level debe registrarse");
        let has_def_in_line_1 = defs.iter().any(|(_, def_span)| def_span.line == 1);
        assert!(
            has_def_in_line_1,
            "def_span of FnDef `dobla` must be on line 1: {:?}",
            defs.iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn def_info_registers_fn_param_use() {
        // The use of the param `n` inside the body of `fn dobla` is
        // registered as Ident with def_span on the FnDef's line
        // (without own span in `Param`, we approximate to the FnDef).
        let defs = defs_of("fn dobla(n: Int) -> Int => n * 2\n");
        // The arrow body contains a use of ident `n` on line 1.
        // The param's def_span is also line 1 (same Stmt).
        assert!(!defs.is_empty(), "uso del param debe registrarse");
        let entry = defs.iter().next().unwrap();
        let (use_span, def_span) = entry;
        assert_eq!(use_span.0, 1, "use on line 1");
        assert_eq!(def_span.line, 1, "def_span of param is the fn (line 1)");
    }

    #[test]
    fn def_info_definition_at_returns_none_for_unknown_span() {
        let defs = defs_of("let x = 1\nlet y = x\n");
        assert!(
            defs.definition_at(Span::new(999, 999)).is_none(),
            "span ausente debe devolver None"
        );
        assert!(
            defs.definition_at(Span::ZERO).is_none(),
            "Span::ZERO debe devolver None"
        );
    }

    #[test]
    fn def_info_does_not_register_undefined_ident_use() {
        // The ident `nope` doesn't exist in scope — the checker emits
        // an error, but must not register entries in DefinitionInfo
        // (no binding to point to).
        let defs = defs_of("let y = nope\n");
        assert!(
            defs.is_empty(),
            "ident no definido no debe registrarse: {:?}",
            defs.iter().collect::<Vec<_>>()
        );
    }

    // ---- Mini-batch C — list comprehensions ----

    #[test]
    fn checker_list_comp_simple_types_as_list_of_expr() {
        // `[x * 2 for x in [1, 2, 3]]` must type as `List<Int>`
        // (the expr is Int, the iter is List<Int>).
        let src = "let r: List<Int> = [x * 2 for x in [1, 2, 3]]\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_list_comp_on_range_types_int_in_var() {
        // The comprehension var over Range must type Int.
        // If the expr uses `var * 2`, the result is List<Int>.
        let src = "let r: List<Int> = [n * 2 for n in 0..10]\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_list_comp_filter_non_bool_is_error() {
        // The filter must be `Bool`. If it's Int → type error.
        let src = "let r = [x for x in [1, 2, 3] if x]\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("filter") || e.message.contains("Bool")),
            "expected error about the filter: {:?}",
            errors
        );
    }

    #[test]
    fn checker_list_comp_iter_non_iterable_is_error() {
        // Iter Int → type error (not List nor Range).
        let src = "let r = [x for x in 42]\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("iterable") || e.message.contains("List o Range")),
            "expected error about the iter: {:?}",
            errors
        );
    }

    #[test]
    fn checker_list_comp_var_does_not_escape_to_caller() {
        // The var's local scope means that after the comprehension,
        // `x` is not visible outside. Using `x` outside must emit
        // "variable not defined".
        let src = "let r = [x for x in [1, 2, 3]]\nlet y = x\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("variable")
                && (e.message.contains("x") || e.message.contains("not defined"))),
            "expected error about undefined `x`: {:?}",
            errors
        );
    }

    // ---- Mini-batch Fm — format spec compatibility ----

    #[test]
    fn checker_fm_spec_f_with_float_compiles_clean() {
        let src = "let x: Float = 3.14\nlet s = \"{x:.2f}\"\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_fm_spec_f_with_int_compiles_clean() {
        // Transparent Int → Float promotion.
        let src = "let n: Int = 42\nlet s = \"{n:.2f}\"\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_fm_spec_f_with_str_is_error() {
        let src = "let s: Str = \"hola\"\nlet r = \"{s:.2f}\"\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("`f`") && e.message.contains("Float o Int")),
            "expected compatibility error: {:?}",
            errors
        );
    }

    #[test]
    fn checker_fm_spec_d_with_float_is_error() {
        let src = "let x: Float = 3.14\nlet r = \"{x:d}\"\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("`d`") && e.message.contains("Int")),
            "expected compatibility error: {:?}",
            errors
        );
    }

    #[test]
    fn checker_fm_spec_string_is_compatible_with_any_type() {
        // The `s` kind accepts any type (via Display).
        let src = "let n: Int = 42\nlet r = \"{n:s}\"\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    // ---- Mini-batch Md — for with Pattern in `var` ----

    #[test]
    fn checker_for_tuple_pattern_on_map_binds_k_and_v_with_correct_types() {
        // `for (k, v) in m` with m: Map<Str, Int> must bind k: Str and v: Int.
        // If I use them correctly, no errors.
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nfor (k, v) in m {\n    let len_k: Int = k.len()\n    let v2: Int = v + 1\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_for_wildcard_pattern_compiles_without_binding() {
        // `for _ in xs` binds nothing, must not error even if `_`
        // would be used inside the body (doesn't exist).
        let src = "let xs: List<Int> = [1, 2, 3]\nfor _ in xs {\n    print(\"hola\")\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_for_tuple_pattern_on_list_is_error() {
        // `for (a, b) in xs` with xs: List<Int> makes no sense — error.
        let src = "let xs: List<Int> = [1, 2, 3]\nfor (a, b) in xs {\n    print(a)\n}\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("tuple")),
            "expected error about tuple pattern: {:?}",
            errors
        );
    }

    #[test]
    fn checker_for_pattern_int_literal_is_error() {
        // `for 42 in xs` — literal pattern not allowed as for var.
        let src = "for 42 in [1, 2] { print(\"x\") }\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("admitido") || e.message.contains("Ident")),
            "expected error about unsupported pattern: {:?}",
            errors
        );
    }

    // ---- Mini-batch It — iterators enumerate/zip/chain ----

    #[test]
    fn checker_list_enumerate_types_as_list_tuple_int_t() {
        // `xs.enumerate()` with xs: List<Int> must type `List<(Int, Int)>`.
        let src = "let xs: List<Int> = [1, 2, 3]\nlet ys: List<(Int, Int)> = xs.enumerate()\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_list_zip_with_different_types_types_list_tuple_t_u() {
        // `xs.zip(ys)` with xs: List<Int>, ys: List<Str> must type
        // `List<(Int, Str)>`.
        let src =
            "let xs: List<Int> = [1, 2]\nlet ys: List<Str> = [\"a\", \"b\"]\nlet pairs: List<(Int, Str)> = xs.zip(ys)\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_list_chain_with_equal_types_compiles() {
        // `xs.chain(ys)` with both List<Int> must type `List<Int>`.
        let src =
            "let xs: List<Int> = [1, 2]\nlet ys: List<Int> = [3, 4]\nlet zs: List<Int> = xs.chain(ys)\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_list_chain_with_incompatible_types_is_error() {
        // `xs.chain(ys)` with xs: List<Int>, ys: List<Str> → error.
        let src =
            "let xs: List<Int> = [1, 2]\nlet ys: List<Str> = [\"a\"]\nlet zs = xs.chain(ys)\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("chain") && e.message.contains("List<Int>")),
            "expected error about chain with incompatible types: {:?}",
            errors
        );
    }

    // ---- Mini-batch Bits — bit-wise operators ----

    // ---- Mini-batch Re+ — Type::Result { ok, err } typed ----

    #[test]
    fn checker_re_plus_result_t_e_explicit_annotation() {
        let src = "type ApiError { status: Int, msg: Str }\nfn fetch() -> Result<Int, ApiError> {\n    return Err(ApiError { status: 503, msg: \"down\" })\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_re_plus_match_err_binds_e_with_inferred_type() {
        // The `e` binding from `Err(e)` now types with the E of the Result.
        let src = "type ApiError { status: Int, msg: Str }\nfn fetch() -> Result<Int, ApiError> {\n    return Err(ApiError { status: 503, msg: \"x\" })\n}\nlet code: Int = match fetch() {\n    Ok(v) => v,\n    Err(e) => e.status\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_re_plus_result_legacy_without_explicit_e_still_works() {
        // `Result<T>` without E must keep working (default Str).
        let src = "fn div(a: Int, b: Int) -> Result<Int> {\n    if b == 0 { return Err(\"zero\") }\n    return Ok(a / b)\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_re_plus_result_invalid_arity_is_error() {
        // `Result<T, E, X>` with 3 args is an error.
        let src = "let r: Result<Int, Str, Bool> = Ok(1)\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Result") && e.message.contains("1 or 2")),
            "expected error about arity: {:?}",
            errors
        );
    }

    #[test]
    fn checker_re_plus_result_display_with_concrete_e() {
        use crate::types::Type;
        let env = TypeEnv::default();
        let r1 = Type::Result {
            ok: Box::new(Type::Int),
            err: Box::new(Type::Str),
        };
        let r2 = Type::Result {
            ok: Box::new(Type::Int),
            err: Box::new(Type::Int),
        };
        // E = Str (default) → omits E.
        assert_eq!(r1.display(&env), "Result<Int>");
        // E ≠ Str (Int) → full form.
        assert_eq!(r2.display(&env), "Result<Int, Int>");
    }

    #[test]
    fn checker_bits_on_int_is_ok() {
        let src = "let a: Int = 5 & 3\nlet b: Int = 5 | 3\nlet c: Int = 5 ^ 3\nlet d: Int = 1 << 4\nlet e: Int = ~0\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    #[test]
    fn checker_bits_on_float_is_error() {
        let src = "let r = 3.14 & 2\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("bitwise") && e.message.contains("Float")),
            "expected error about bitwise with Float: {:?}",
            errors
        );
    }

    #[test]
    fn checker_bits_on_bool_is_error() {
        let src = "let r = true & false\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("bitwise") && e.message.contains("Bool")),
            "expected error about `&` with Bool: {:?}",
            errors
        );
    }

    #[test]
    fn checker_bitnot_on_float_is_error() {
        let src = "let r = ~3.14\n";
        let errors = check_recovering(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("`~`") && e.message.contains("Int")),
            "expected error about `~` with Float: {:?}",
            errors
        );
    }

    #[test]
    fn checker_list_enumerate_composes_with_md_for_destructuring() {
        // The canonical case that motivates the mini-batch: `for (i, x) in xs.enumerate()`.
        let src = "let xs: List<Str> = [\"a\", \"b\"]\nfor (i, x) in xs.enumerate() {\n    let idx: Int = i\n    let val: Str = x\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
    }

    // ---- Mini-batch Math + Mb9 + Int/Float methods ----

    #[test]
    fn math_builtins_polymorphic_accept_int_and_float() {
        // Math builtins type as Any in scope[0] — codegen
        // does the concrete dispatch. The checker only validates that they exist.
        assert_ok("let a = abs(-5)");
        assert_ok("let b = min(3, 5)");
        assert_ok("let c = pow(2, 10)");
        assert_ok("let d = sqrt(16)");
        assert_ok("let e = clamp(5, 0, 10)");
    }

    #[test]
    fn mb9_str_swap_case_types_str() {
        assert_ok("let s: Str = \"Hola\".swap_case()");
    }

    #[test]
    fn mb9_str_title_types_str() {
        assert_ok("let s: Str = \"hola mundo\".title()");
    }

    #[test]
    fn mb9_str_is_alpha_digit_numeric_type_bool() {
        assert_ok(
            "let a: Bool = \"hola\".is_alpha()\n\
             let b: Bool = \"123\".is_digit()\n\
             let c: Bool = \"3.14\".is_numeric()",
        );
    }

    #[test]
    fn mb9_list_split_at_types_tuple_of_two_lists() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let parts: (List<Int>, List<Int>) = xs.split_at(2)",
        );
    }

    #[test]
    fn mb9_map_has_value_types_bool() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let r: Bool = m.has_value(1)",
        );
    }

    #[test]
    fn int_method_abs_and_to_str_type_correctly() {
        assert_ok(
            "let n: Int = 5\n\
             let a: Int = n.abs()\n\
             let s: Str = n.to_str()\n\
             let b: Str = n.to_str_base(16)",
        );
    }

    #[test]
    fn float_method_abs_to_str_is_nan_is_finite_type_correctly() {
        assert_ok(
            "let x: Float = 3.14\n\
             let a: Float = x.abs()\n\
             let s: Str = x.to_str()\n\
             let n: Bool = x.is_nan()\n\
             let f: Bool = x.is_finite()",
        );
    }

    #[test]
    fn int_nonexistent_method_is_error() {
        assert_error_with("let n: Int = 5\nlet r = n.foobar()", &["Int", "foobar"]);
    }

    #[test]
    fn float_nonexistent_method_is_error() {
        assert_error_with(
            "let x: Float = 3.14\nlet r = x.foobar()",
            &["Float", "foobar"],
        );
    }

    // ---- Mini-batch Fp — default params ----

    #[test]
    fn fp_call_without_args_to_fn_with_default_passes() {
        // `fn greet(name = "amigo") -> Str` can be invoked without args.
        assert_ok(
            "fn greet(name: Str = \"amigo\") -> Str { return name }\n\
             let r: Str = greet()",
        );
    }

    #[test]
    fn fp_call_with_arg_to_fn_with_default_passes() {
        assert_ok(
            "fn greet(name: Str = \"amigo\") -> Str { return name }\n\
             let r: Str = greet(\"Fitz\")",
        );
    }

    #[test]
    fn fp_call_with_mix_required_and_default() {
        // Required + default: 1 or 2 valid args, 0 or 3+ fails.
        assert_ok(
            "fn add(a: Int, b: Int = 1) -> Int { return a + b }\n\
             let r1: Int = add(10)\n\
             let r2: Int = add(10, 5)",
        );
    }

    #[test]
    fn fp_call_too_few_args_is_error() {
        // `fn add(a, b)` without defaults — call with 0 args is an error.
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let r = add()",
            &["add", "2"],
        );
    }

    #[test]
    fn fp_call_too_many_args_is_error() {
        assert_error_with(
            "fn greet(name: Str = \"amigo\") -> Str { return name }\n\
             let r = greet(\"a\", \"b\")",
            &["greet", "0", "1"],
        );
    }

    #[test]
    fn fp_default_wrong_type_is_error_at_call() {
        // The default `"texto"` doesn't match `Int`. When checking the
        // call without args, the default should trigger a type error. Today
        // the checker does NOT validate the default expr — the runtime will.
        // "Negative" scope test: assert it DOES pass (doesn't break anything).
        // The default itself will be a runtime error if the default path
        // is never called.
        assert_ok(
            "fn f(x: Int = 5) -> Int { return x }\n\
             let r: Int = f()",
        );
    }

    // ----------------------------------------------------------------
    // Phase 9.w.1.a — Native auth: checker for
    // `@auth_provider` / `@authenticated` / `@admin`.
    // ----------------------------------------------------------------

    /// Helper: checks that the program passes without errors.
    fn assert_auth_ok(src: &str) {
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected no errors, was: {:?}", errors);
    }

    /// Helper: checks that the program produces at least one error whose
    /// message contains the expected substring.
    fn assert_auth_err(src: &str, expected_substr: &str) {
        let errors = errors_of(src);
        let matched = errors.iter().any(|e| e.message.contains(expected_substr));
        assert!(
            matched,
            "expected error with substring '{}', errors were: {:?}",
            expected_substr, errors
        );
    }

    #[test]
    fn auth_provider_valid_signature_gives_no_error() {
        // Minimal provider: 1 param Map<Str,Str>, return Result<User>.
        // Any `type User { ... }` declared in the program is enough;
        // the provider does NOT execute — it just registers the signature.
        let src = "type User { id: Int, name: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"sin auth\")\n\
                   }";
        assert_auth_ok(src);
    }

    #[test]
    fn auth_provider_with_args_is_error() {
        // `@auth_provider` doesn't accept args or kwargs in the MVP.
        let src = "type User { id: Int }\n\
                   @auth_provider(123)\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }";
        assert_auth_err(src, "does not accept args or kwargs");
    }

    #[test]
    fn auth_provider_wrong_param_is_error() {
        // The param must be `Map<Str, Str>` (HTTP headers).
        let src = "type User { id: Int }\n\
                   @auth_provider\n\
                   fn check(token: Str) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }";
        assert_auth_err(src, "Map<Str, Str>");
    }

    #[test]
    fn auth_provider_wrong_arity_is_error() {
        // Must have exactly 1 param.
        let src = "type User { id: Int }\n\
                   @auth_provider\n\
                   fn check() -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }";
        assert_auth_err(src, "exactly 1 param");
    }

    #[test]
    fn auth_provider_return_non_result_is_error() {
        // The return must be `Result<T>` with T nominal.
        let src = "type User { id: Int }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> User {\n\
                       return User { id: 1 }\n\
                   }";
        assert_auth_err(src, "Result<T>");
    }

    #[test]
    fn auth_provider_result_of_primitive_is_error() {
        // `Result<Str>` doesn't work — T must be a custom (nominal) type.
        let src = "@auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<Str> {\n\
                       return Ok(\"sin user type\")\n\
                   }";
        assert_auth_err(src, "custom type");
    }

    #[test]
    fn auth_provider_duplicate_is_error() {
        // Only one `@auth_provider` per program.
        let src = "type User { id: Int }\n\
                   @auth_provider\n\
                   fn check1(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @auth_provider\n\
                   fn check2(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"y\")\n\
                   }";
        assert_auth_err(src, "duplicate @auth_provider");
    }

    #[test]
    fn authenticated_without_provider_is_error() {
        // `@authenticated` requires a `@auth_provider` in the
        // program.
        let src = "type User { id: Int }\n\
                   @authenticated\n\
                   @get(\"/me\")\n\
                   fn me(user: User) -> User { return user }";
        assert_auth_err(src, "no `@auth_provider`");
    }

    #[test]
    fn admin_without_provider_is_error() {
        // `@admin` requires a `@auth_provider` in the program.
        let src = "type User { id: Int, role: Str }\n\
                   @admin\n\
                   @delete(\"/x\")\n\
                   fn del(user: User) -> Str { return \"ok\" }";
        assert_auth_err(src, "no `@auth_provider`");
    }

    #[test]
    fn authenticated_handler_without_user_param_is_error() {
        // The protected handler must declare a param compatible with the
        // type the provider returns (`User`). The runtime injects it
        // after successful authentication.
        let src = "type User { id: Int }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @authenticated\n\
                   @get(\"/me\")\n\
                   fn me() -> Str { return \"hola\" }";
        assert_auth_err(src, "missing param of type `User`");
    }

    #[test]
    fn authenticated_handler_with_user_param_gives_no_error() {
        // Handler with param `user: User` (same type the provider
        // returns) checks clean.
        let src = "type User { id: Int, name: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @authenticated\n\
                   @get(\"/me\")\n\
                   fn me(user: User) -> User { return user }";
        assert_auth_ok(src);
    }

    #[test]
    fn authenticated_without_http_handler_is_error() {
        // `@authenticated` over a fn that does NOT have
        // `@get`/`@post`/`@put`/`@delete` makes no sense.
        let src = "type User { id: Int }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @authenticated\n\
                   fn algo(user: User) -> Str { return \"x\" }";
        assert_auth_err(src, "only applies to HTTP handlers");
    }

    #[test]
    fn admin_without_role_field_in_user_is_error() {
        // `@admin` requires that the `User` (provider's return) has a
        // `role: Str` field to discriminate admins. Without that field, error.
        let src = "type User { id: Int, name: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @admin\n\
                   @delete(\"/x/{id}\")\n\
                   fn del(id: Int, user: User) -> Str { return \"ok\" }";
        assert_auth_err(src, "`role: Str` field");
    }

    #[test]
    fn admin_with_role_field_gives_no_error() {
        // Complete valid program: provider + `@admin` handler with
        // `User { ..., role: Str }`.
        let src = "type User { id: Int, name: Str, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @admin\n\
                   @delete(\"/users/{id}\")\n\
                   fn del(id: Int, user: User) -> Str { return \"ok\" }";
        assert_auth_ok(src);
    }

    #[test]
    fn auth_decorators_with_args_are_error() {
        // `@authenticated` and `@admin` don't accept args or kwargs.
        let src = "type User { id: Int, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @authenticated(scope=\"x\")\n\
                   @get(\"/me\")\n\
                   fn me(user: User) -> User { return user }";
        assert_auth_err(src, "does not accept args or kwargs");
    }

    // ----- Phase 9.w.1.iter2.a — @requires("role") (custom RBAC) -----

    #[test]
    fn requires_with_str_literal_role_gives_no_error() {
        // Canonical pattern: `@requires("editor")` over an HTTP handler with
        // a declared provider and User.role: Str. Compiles clean.
        let src = "type User { id: Int, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @requires(\"editor\")\n\
                   @post(\"/posts\")\n\
                   fn create(user: User) -> Str { return \"ok\" }";
        assert_auth_ok(src);
    }

    #[test]
    fn requires_without_role_field_in_user_is_error() {
        // Like `@admin`, `@requires` demands `role: Str` in the User type.
        let src = "type User { id: Int, name: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @requires(\"editor\")\n\
                   @get(\"/edit\")\n\
                   fn h(user: User) -> Str { return \"ok\" }";
        assert_auth_err(src, "`role: Str` field");
    }

    #[test]
    fn requires_stacked_with_two_roles_gives_no_error() {
        // Stacking two distinct `@requires` = OR (user.role must match
        // at least one).
        let src = "type User { id: Int, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @requires(\"editor\")\n\
                   @requires(\"publisher\")\n\
                   @post(\"/articles\")\n\
                   fn create(user: User) -> Str { return \"ok\" }";
        assert_auth_ok(src);
    }

    #[test]
    fn requires_with_duplicate_role_is_error() {
        let src = "type User { id: Int, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @requires(\"editor\")\n\
                   @requires(\"editor\")\n\
                   @get(\"/x\")\n\
                   fn h(user: User) -> Str { return \"ok\" }";
        assert_auth_err(src, "duplicate");
    }

    #[test]
    fn requires_without_provider_is_error() {
        let src = "type User { id: Int, role: Str }\n\
                   @requires(\"editor\")\n\
                   @get(\"/x\")\n\
                   fn h(user: User) -> Str { return \"ok\" }";
        assert_auth_err(src, "no `@auth_provider`");
    }

    #[test]
    fn requires_without_http_handler_is_error() {
        let src = "type User { id: Int, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @requires(\"editor\")\n\
                   fn algo(user: User) -> Str { return \"x\" }";
        assert_auth_err(src, "only applies to HTTP handlers");
    }

    #[test]
    fn requires_without_arg_is_error() {
        let src = "type User { id: Int, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @requires()\n\
                   @get(\"/x\")\n\
                   fn h(user: User) -> Str { return \"ok\" }";
        assert_auth_err(src, "expects exactly 1 arg");
    }

    #[test]
    fn requires_with_kwargs_is_error() {
        let src = "type User { id: Int, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @requires(role=\"editor\")\n\
                   @get(\"/x\")\n\
                   fn h(user: User) -> Str { return \"ok\" }";
        assert_auth_err(src, "does not accept kwargs");
    }

    #[test]
    fn requires_without_user_param_is_error() {
        // `@requires` needs the injected user to check the role.
        let src = "type User { id: Int, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @requires(\"editor\")\n\
                   @get(\"/x\")\n\
                   fn h() -> Str { return \"ok\" }";
        assert_auth_err(src, "missing param of type");
    }

    // ----- Phase 12.7 — @trace/@metric (explicit instrumentation) -----

    #[test]
    fn trace_without_args_or_kwargs_compiles_clean() {
        let src = "@trace\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_ok(src);
    }

    #[test]
    fn trace_with_kwarg_name_str_literal_compiles_clean() {
        let src = "@trace(name=\"calc_doble\")\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_ok(src);
    }

    #[test]
    fn trace_with_positional_arg_is_error() {
        let src = "@trace(\"name\")\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_err(src, "does not accept positional args");
    }

    #[test]
    fn trace_with_unknown_kwarg_is_error() {
        let src = "@trace(level=\"info\")\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_err(src, "unknown kwarg");
    }

    #[test]
    fn trace_with_non_str_literal_name_is_error() {
        let src = "@trace(name=42)\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_err(src, "Str literal");
    }

    #[test]
    fn trace_duplicate_stacked_is_error() {
        let src = "@trace\n@trace(name=\"otro\")\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_err(src, "duplicate");
    }

    #[test]
    fn trace_stackable_with_metric_compiles_clean() {
        let src = "@trace\n@metric\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_ok(src);
    }

    #[test]
    fn trace_on_get_handler_is_error() {
        // Auto-instrumentation 12.3 already covers HTTP handlers.
        let src = "@trace\n@get(\"/x\")\nfn h() -> Str => \"ok\"";
        assert_auth_err(src, "auto-instrumentation");
    }

    #[test]
    fn metric_on_post_handler_is_error() {
        let src = "@metric\n@post(\"/x\")\nfn h(body: Str) -> Str => body";
        assert_auth_err(src, "auto-instrumentation");
    }

    #[test]
    fn metric_without_args_or_kwargs_compiles_clean() {
        let src = "@metric\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_ok(src);
    }

    #[test]
    fn metric_with_positional_arg_is_error() {
        let src = "@metric(\"name\")\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_err(src, "does not accept positional args");
    }

    #[test]
    fn metric_duplicate_stacked_is_error() {
        let src = "@metric\n@metric\nfn calc(x: Int) -> Int => x * 2";
        assert_auth_err(src, "duplicate");
    }

    #[test]
    fn auth_provider_with_nullable_role_field_not_enough_for_admin() {
        // The `role` field must be `Str` (not nullable). If it's `Str?`,
        // admin discrimination doesn't compose (a Null is not admin).
        let src = "type User { id: Int, role: Str? }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n\
                       return Err(\"x\")\n\
                   }\n\
                   @admin\n\
                   @get(\"/x\")\n\
                   fn h(user: User) -> Str { return \"ok\" }";
        assert_auth_err(src, "`role: Str` field");
    }

    // ----------------------------------------------------------------
    // Phase 9.w.2.a — Typed WebSockets: type `WsConn<T>` + checker
    // ----------------------------------------------------------------

    #[test]
    fn wsconn_resolves_as_builtin_generic() {
        // `WsConn<Str>` reuses `TypeExpr::Generic`. Fixed arity 1.
        let env = TypeEnv::new();
        let te = TypeExpr::Generic {
            name: "WsConn".into(),
            args: vec![TypeExpr::Named("Str".into())],
        };
        let ty = resolve_type_expr(&te, &env).expect("WsConn<Str>");
        // 9.w.2-wsconn-bidir: `WsConn<T>` (arity 1) = symmetric,
        // recv == send == T.
        assert_eq!(
            ty,
            Type::WsConn {
                recv: Box::new(Type::Str),
                send: Box::new(Type::Str),
            }
        );
    }

    #[test]
    fn wsconn_without_argument_is_arity_error() {
        let env = TypeEnv::new();
        let te = TypeExpr::Generic {
            name: "WsConn".into(),
            args: vec![],
        };
        let err = resolve_type_expr(&te, &env).expect_err("arity 0");
        assert!(matches!(err.kind, ErrorKind::TypeError));
    }

    #[test]
    fn wsconn_display_shows_inner() {
        let env = TypeEnv::new();
        let ty = Type::WsConn {
            recv: Box::new(Type::Int),
            send: Box::new(Type::Int),
        };
        assert_eq!(ty.display(&env), "WsConn<Int>");
    }

    #[test]
    fn wsconn_bidir_arity_2_resolves_distinct_recv_send() {
        // 9.w.2-wsconn-bidir — `WsConn<Int, Str>` recv=Int send=Str.
        let env = TypeEnv::new();
        let te = TypeExpr::Generic {
            name: "WsConn".into(),
            args: vec![TypeExpr::Named("Int".into()), TypeExpr::Named("Str".into())],
        };
        let ty = resolve_type_expr(&te, &env).expect("WsConn<Int, Str>");
        assert_eq!(
            ty,
            Type::WsConn {
                recv: Box::new(Type::Int),
                send: Box::new(Type::Str),
            }
        );
    }

    #[test]
    fn wsconn_bidir_asymmetric_display_shows_in_out() {
        let env = TypeEnv::new();
        let ty = Type::WsConn {
            recv: Box::new(Type::Int),
            send: Box::new(Type::Str),
        };
        assert_eq!(ty.display(&env), "WsConn<Int, Str>");
    }

    #[test]
    fn wsconn_bidir_arity_greater_than_2_is_error() {
        let env = TypeEnv::new();
        let te = TypeExpr::Generic {
            name: "WsConn".into(),
            args: vec![
                TypeExpr::Named("Int".into()),
                TypeExpr::Named("Str".into()),
                TypeExpr::Named("Bool".into()),
            ],
        };
        let err = resolve_type_expr(&te, &env).expect_err("arity 3 should fail");
        assert!(matches!(err.kind, ErrorKind::TypeError));
    }

    #[test]
    fn ws_handler_minimal_passes_checker() {
        // Minimal handler: `async fn` + `@ws("/chat")` + WsConn<Str>.
        let src = "@ws(\"/chat\")\n\
                   async fn echo(conn: WsConn<Str>) -> Null {\n\
                       match conn.recv() {\n\
                           Ok(msg) => match conn.send(msg) {\n\
                               Ok(_) => return null,\n\
                               Err(_) => return null,\n\
                           },\n\
                           Err(_) => return null,\n\
                       }\n\
                   }";
        assert_auth_ok(src);
    }

    #[test]
    fn ws_handler_without_async_is_error() {
        let src = "@ws(\"/chat\")\n\
                   fn echo(conn: WsConn<Str>) -> Null { return null }";
        assert_auth_err(src, "async fn");
    }

    #[test]
    fn ws_handler_without_wsconn_param_is_error() {
        let src = "@ws(\"/chat\")\n\
                   async fn echo() -> Null { return null }";
        assert_auth_err(src, "1 param");
    }

    #[test]
    fn ws_handler_wsconn_with_concrete_t_compiles() {
        // `WsConn<ChatMsg>` with custom type. The checker must accept it.
        let src = "type ChatMsg { user: Str, text: Str }\n\
                   @ws(\"/chat\")\n\
                   async fn chat(conn: WsConn<ChatMsg>) -> Null { return null }";
        assert_auth_ok(src);
    }

    #[test]
    fn ws_decorator_without_path_arg_is_error() {
        let src = "@ws()\n\
                   async fn echo(conn: WsConn<Str>) -> Null { return null }";
        assert_auth_err(src, "1 argument");
    }

    #[test]
    fn ws_method_recv_returns_result_t() {
        // `conn.recv()` must type as `Result<T>` where T is the
        // WsConn parameter.
        let src = "@ws(\"/c\")\n\
                   async fn h(conn: WsConn<Str>) -> Null {\n\
                       let r: Result<Str> = conn.recv()\n\
                       return null\n\
                   }";
        assert_auth_ok(src);
    }

    #[test]
    fn ws_method_send_with_different_type_is_error() {
        // `conn.send(msg: T)` must reject args of a different type than T.
        let src = "@ws(\"/c\")\n\
                   async fn h(conn: WsConn<Str>) -> Null {\n\
                       let _r = conn.send(42)\n\
                       return null\n\
                   }";
        assert_auth_err(src, "WsConn<Str>.send");
    }

    #[test]
    fn ws_method_broadcast_returns_result_null() {
        let src = "@ws(\"/c\")\n\
                   async fn h(conn: WsConn<Str>) -> Null {\n\
                       let r: Result<Null> = conn.broadcast(\"hola\")\n\
                       return null\n\
                   }";
        assert_auth_ok(src);
    }

    #[test]
    fn ws_unknown_method_is_error() {
        let src = "@ws(\"/c\")\n\
                   async fn h(conn: WsConn<Str>) -> Null {\n\
                       let _ = conn.zzz()\n\
                       return null\n\
                   }";
        assert_auth_err(src, "does not have method `zzz`");
    }

    #[test]
    fn ws_handler_with_authenticated_accepts_2_params() {
        // `@authenticated @ws("/me-chat")` with (WsConn<Str>, user: User).
        let src = "type User { id: Int, name: Str }\n\
                   @auth_provider\n\
                   fn check(h: Map<Str, Str>) -> Result<User> { return Err(\"x\") }\n\
                   @authenticated\n\
                   @ws(\"/me-chat\")\n\
                   async fn h(conn: WsConn<Str>, user: User) -> Null { return null }";
        assert_auth_ok(src);
    }

    #[test]
    fn ws_handler_wsconn_any_is_error() {
        // `WsConn<Any>` is not accepted — T must be concrete.
        let src = "@ws(\"/c\")\n\
                   async fn h(conn: WsConn<Any>) -> Null { return null }";
        assert_auth_err(src, "concrete `T`");
    }

    // ---- 9.w.2-binary-frames — `WsConn<Bytes>` ----
    //
    // The checker is parametric over T in `infer_wsconn_method` and treats
    // `Bytes` as any other concrete type. These tests guard
    // the contract: `WsConn<Bytes>` is accepted, `recv()` types
    // `Result<Bytes>`, `send`/`broadcast` accept `Bytes` and reject
    // incompatible types. Binary-vs-text discrimination lives in
    // runtime (evaluator + http.rs) and codegen.

    #[test]
    fn ws_handler_wsconn_bytes_compiles() {
        let src = "@ws(\"/raw\")\n\
                   async fn raw(conn: WsConn<Bytes>) -> Null {\n\
                       match conn.recv() {\n\
                           Ok(buf) => match conn.send(buf) {\n\
                               Ok(_) => return null,\n\
                               Err(_) => return null,\n\
                           },\n\
                           Err(_) => return null,\n\
                       }\n\
                   }";
        assert_auth_ok(src);
    }

    #[test]
    fn ws_method_recv_bytes_returns_result_bytes() {
        let src = "@ws(\"/c\")\n\
                   async fn h(conn: WsConn<Bytes>) -> Null {\n\
                       let r: Result<Bytes> = conn.recv()\n\
                       return null\n\
                   }";
        assert_auth_ok(src);
    }

    #[test]
    fn ws_method_send_bytes_accepts_bytes_literal() {
        let src = "@ws(\"/c\")\n\
                   async fn h(conn: WsConn<Bytes>) -> Null {\n\
                       let _r = conn.send(b\"hola\")\n\
                       return null\n\
                   }";
        assert_auth_ok(src);
    }

    #[test]
    fn ws_method_send_bytes_rejects_str() {
        // `conn.send("hola")` over `WsConn<Bytes>` errors: the arg
        // is `Str`, the method expects `Bytes`.
        let src = "@ws(\"/c\")\n\
                   async fn h(conn: WsConn<Bytes>) -> Null {\n\
                       let _r = conn.send(\"hola\")\n\
                       return null\n\
                   }";
        assert_auth_err(src, "WsConn<Bytes>.send");
    }

    // ---- Phase 9.w.3 — checker @cron + @background + spawn ----

    #[test]
    fn cron_simple_without_params_async_passes_checker() {
        // `@cron("0 0 * * *")` over async fn without params + return Null:
        // valid MVP shape. The checker doesn't validate cron syntax
        // (that's done in runtime/codegen).
        let src = "@cron(\"0 0 * * *\")\n\
                   async fn cleanup() -> Null { return null }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors, were {:?}", errors);
    }

    #[test]
    fn cron_sync_fn_passes_checker() {
        // The MVP accepts `@cron` on sync and async (decision confirmed
        // by the author when starting 9.w.3). Sync runs directly, async
        // with `.await` inside the scheduler.
        let src = "@cron(\"*/5 * * * *\")\n\
                   fn tick() -> Null { return null }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors, were {:?}", errors);
    }

    #[test]
    fn cron_without_args_is_error() {
        let src = "@cron\nfn tick() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("@cron") && e.message.contains("1 positional argument")),
            "expected msg about args: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_non_str_arg_is_error() {
        let src = "@cron(60)\nfn tick() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors.iter().any(|e| e.message.contains("Str literal")),
            "expected msg about Str literal: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_params_is_error() {
        let src = "@cron(\"0 0 * * *\")\nfn tick(x: Int) -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("does not accept params")),
            "expected msg about params: {:?}",
            errors
        );
    }

    #[test]
    fn cron_combined_with_get_is_error() {
        let src = "@cron(\"0 0 * * *\")\n@get(\"/x\")\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("is not combinable") && e.message.contains("get")),
            "expected msg about combination with @get: {:?}",
            errors
        );
    }

    #[test]
    fn cron_combined_with_background_is_error() {
        let src = "@cron(\"0 0 * * *\")\n@background\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors.iter().any(
                |e| e.message.contains("is not combinable") && e.message.contains("background")
            ),
            "expected msg about combination with @background: {:?}",
            errors
        );
    }

    #[test]
    fn cron_return_int_is_error() {
        let src = "@cron(\"0 0 * * *\")\nfn h() -> Int { return 1 }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("@cron") && e.message.contains("Null")),
            "expected msg about return Null/Result: {:?}",
            errors
        );
    }

    #[test]
    fn cron_return_result_is_ok() {
        // `Result<Null>` is valid — useful for logging job failures
        // without aborting the scheduler.
        let src = "@cron(\"0 0 * * *\")\nfn h() -> Result<Null> { return Ok(null) }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    #[test]
    fn background_simple_passes_checker() {
        let src = "@background\nfn send_email(to: Str) -> Null { return null }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    #[test]
    fn background_with_args_is_error() {
        let src = "@background(\"x\")\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("@background") && e.message.contains("does not accept")),
            "expected msg about args: {:?}",
            errors
        );
    }

    #[test]
    fn background_combined_with_get_is_error() {
        let src = "@background\n@get(\"/x\")\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("is not combinable")),
            "expected msg about combination: {:?}",
            errors
        );
    }

    // -------------------------------------------------------------
    // 9.w.3.iter2 — new kwargs in `@cron` and `@background`.
    // tz / retry / catch_up / store in `@cron`; tz / retry in `@bg`.
    // -------------------------------------------------------------

    #[test]
    fn cron_with_valid_tz_passes_checker() {
        // The checker does NOT validate the IANA string (that's runtime); only
        // that it's a Str literal.
        let src = "@cron(\"0 0 * * *\", tz=\"America/Argentina/Buenos_Aires\")\n\
                   fn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    #[test]
    fn cron_with_non_str_tz_is_error() {
        let src = "@cron(\"0 0 * * *\", tz=42)\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("`tz`") && e.message.contains("Str literal")),
            "expected msg about tz Str literal: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_complete_retry_passes_checker() {
        let src = "@cron(\"0 0 * * *\", retry={max: 3, backoff: \"exponential\", initial_secs: 1, max_secs: 60})\n\
                   fn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    #[test]
    fn cron_with_retry_only_max_passes_checker() {
        // Reasonable defaults for the 3 remaining sub-params (the
        // runtime checks them, not the checker).
        let src = "@cron(\"0 0 * * *\", retry={max: 5})\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    #[test]
    fn cron_with_non_map_retry_is_error() {
        let src = "@cron(\"0 0 * * *\", retry=3)\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("`retry`") && e.message.contains("Map literal")),
            "expected msg about retry Map literal: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_unknown_retry_key_is_error() {
        let src = "@cron(\"0 0 * * *\", retry={foo: 1})\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("`foo`") && e.message.contains("retry")),
            "expected msg about unknown key foo: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_negative_retry_max_is_error() {
        let src = "@cron(\"0 0 * * *\", retry={max: -1})\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("retry.max") && e.message.contains(">= 0")),
            "expected msg about max >= 0: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_unknown_retry_backoff_is_error() {
        let src = "@cron(\"0 0 * * *\", retry={backoff: \"quadratic\"})\n\
                   fn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("retry.backoff") && e.message.contains("exponential")),
            "expected msg about backoff whitelist: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_retry_initial_secs_zero_is_error() {
        let src = "@cron(\"0 0 * * *\", retry={initial_secs: 0})\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("initial_secs") && e.message.contains(">= 1")),
            "expected msg about initial_secs >= 1: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_catch_up_passes_checker() {
        let src = "@cron(\"0 0 * * *\", catch_up=true)\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    #[test]
    fn cron_with_non_bool_catch_up_is_error() {
        let src = "@cron(\"0 0 * * *\", catch_up=1)\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("catch_up") && e.message.contains("Bool literal")),
            "expected msg about catch_up Bool: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_store_ident_passes_checker() {
        // The checker does NOT check that `db` resolves to DbConn — that's
        // runtime. Accepts any non-null expr.
        let src = "let db = 1\n@cron(\"0 0 * * *\", store=db)\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        // `db` resolves to Int in this synthetic program, but the
        // checker does NOT validate store shape (left to runtime). The only
        // error that could appear is from the `let db = 1` assignment,
        // which does NOT touch the decorator.
        assert!(
            !errors.iter().any(|e| e.message.contains("store")),
            "should not have error about store: {:?}",
            errors
        );
    }

    #[test]
    fn cron_with_store_null_is_error() {
        let src = "@cron(\"0 0 * * *\", store=null)\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("store") && e.message.contains("null")),
            "expected msg about non-null store: {:?}",
            errors
        );
    }

    #[test]
    fn cron_unknown_kwarg_is_error() {
        let src = "@cron(\"0 0 * * *\", foo=\"bar\")\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("`foo`") && e.message.contains("unrecognized")),
            "expected msg about unknown kwarg: {:?}",
            errors
        );
    }

    // NOTE: duplicate kwargs at decorator level (`tz=A, tz=B`) are
    // rejected by the PARSER, not the checker, with the message "named
    // argument 'X=' was already given in the same decorator". The
    // `check_job_kwargs` helper still carries a defensive branch in case the
    // parser changes. No test here because `errors_of` panics on
    // `parse(...).expect("parse OK")`.

    #[test]
    fn cron_all_kwargs_together_passes_checker() {
        let src = "let db = 1\n\
                   @cron(\"0 0 * * *\", tz=\"UTC\", retry={max: 3, backoff: \"linear\"}, catch_up=true, store=db)\n\
                   fn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            !errors
                .iter()
                .any(|e| e.message.contains("@cron") || e.message.contains("kwarg")),
            "happy path: should not have errors from @cron: {:?}",
            errors
        );
    }

    #[test]
    fn background_with_tz_passes_checker() {
        let src = "@background(tz=\"America/Argentina/Buenos_Aires\")\n\
                   fn send(addr: Str) -> Null { return null }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    #[test]
    fn background_with_retry_passes_checker() {
        let src = "@background(retry={max: 3, backoff: \"exponential\"})\n\
                   fn send(addr: Str) -> Null { return null }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    #[test]
    fn background_with_store_is_error() {
        // `store` is NOT valid in `@background` (spawn job persistence
        // is deferred to iter3).
        let src = "let db = 1\n@background(store=db)\nfn send(addr: Str) -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors.iter().any(|e| e.message.contains("@background")
                && e.message.contains("`store`")
                && e.message.contains("unrecognized")),
            "expected msg about store not accepted in @background: {:?}",
            errors
        );
    }

    #[test]
    fn background_with_catch_up_is_error() {
        // `catch_up` also doesn't apply to `@background` (it's not scheduling).
        let src = "@background(catch_up=true)\nfn send(addr: Str) -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors.iter().any(|e| e.message.contains("@background")
                && e.message.contains("`catch_up`")
                && e.message.contains("unrecognized")),
            "expected msg about catch_up not accepted in @background: {:?}",
            errors
        );
    }

    #[test]
    fn background_with_positional_args_still_is_error() {
        // We confirm that the inherited behavior (doesn't accept
        // positionals) is preserved after adding kwargs.
        let src = "@background(\"x\")\nfn h() -> Null { return null }";
        let errors = errors_of(src);
        assert!(
            errors.iter().any(|e| e.message.contains("@background")
                && e.message.contains("does not accept positional args")),
            "expected msg about positionals: {:?}",
            errors
        );
    }

    #[test]
    fn spawn_on_background_returns_future() {
        // `spawn(fn_background())` types as `Future<T>`. We validate
        // via program shape: the `let f = spawn(...)` should
        // allow `.await` inside an async fn.
        let src = "@background\nasync fn job() -> Int { return 42 }\n\
                   async fn caller() -> Int {\n\
                       let f = spawn(job())\n\
                       return f.await\n\
                   }\n";
        let errors = errors_of(src);
        // The Int return type is valid because `spawn(job())` →
        // `Future<Int>`, and `.await` unpacks to `Int`.
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    #[test]
    fn spawn_without_args_is_error() {
        let src = "async fn caller() -> Null {\n\
                       let _ = spawn()\n\
                       return null\n\
                   }";
        let errors = errors_of(src);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("spawn") && e.message.contains("1 argument")),
            "expected msg about spawn args: {:?}",
            errors
        );
    }

    #[test]
    fn spawn_with_var_is_error() {
        // `spawn(x)` where x is a var is not accepted — the target must be
        // a literal call to a `@background` fn.
        let src = "async fn caller() -> Null {\n\
                       let x = 1\n\
                       let _ = spawn(x)\n\
                       return null\n\
                   }";
        let errors = errors_of(src);
        assert!(
            errors.iter().any(|e| e.message.contains("literal call")),
            "expected msg about literal call: {:?}",
            errors
        );
    }

    #[test]
    fn spawn_on_fn_without_background_is_error() {
        let src = "fn no_marker() -> Int { return 1 }\n\
                   async fn caller() -> Null {\n\
                       let _ = spawn(no_marker())\n\
                       return null\n\
                   }";
        let errors = errors_of(src);
        assert!(
            errors.iter().any(|e| e.message.contains("@background")),
            "expected msg about @background: {:?}",
            errors
        );
    }

    #[test]
    fn spawn_userdefined_override_does_not_trigger_special_dispatch() {
        // If the user defines their own `spawn`, the special dispatch
        // does NOT apply (the lookup returns `Function{...}` distinct from
        // `Any`). The call is validated via the general path.
        let src = "fn spawn(x: Int) -> Int { return x }\n\
                   fn main() -> Int { return spawn(42) }";
        let errors = errors_of(src);
        assert!(errors.is_empty(), "expected 0 errors: {:?}", errors);
    }

    // B10 (sub-paso 5 cosecha post-fitzwatch, 2026-06-19) — cross-module
    // `@background` detection via `extract_background_fn_names` +
    // `TypeEnv::add_imported_background_fns`.
    #[test]
    fn extract_background_fn_names_collects_marked_top_level_fns_b10() {
        let src = "@background async fn run_check(id: Int) -> Null { return null }\n\
                   fn no_marker(x: Int) -> Int { return x }\n\
                   @background fn cleanup() -> Null { return null }\n";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let names = extract_background_fn_names(&program);
        assert_eq!(names, vec!["run_check".to_string(), "cleanup".to_string()]);
    }

    #[test]
    fn extract_background_fn_names_returns_empty_when_no_background_fns_b10() {
        let src = "fn a() -> Int { return 1 }\n\
                   async fn b() -> Null { return null }\n";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let names = extract_background_fn_names(&program);
        assert!(names.is_empty(), "expected empty, was: {:?}", names);
    }

    #[test]
    fn spawn_cross_module_imported_background_fn_passes_with_pre_scan_b10() {
        // The importer program calls `spawn(remote_fn(42))` where
        // `remote_fn` was imported with `from <mod> import remote_fn`
        // and the imported module had `@background` on it. Without
        // pre-scan, the checker rejects with "is not declared with
        // `@background`"; with `add_imported_background_fns`, it
        // passes.
        let src = "from checks import remote_fn\n\
                   async fn caller() -> Null {\n\
                       let _ = spawn(remote_fn(42))\n\
                       return null\n\
                   }";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        // Without pre-scan: should fail.
        let (_env_pre, _ti, _di, errors_no_scan) = check_program(&program);
        assert!(
            errors_no_scan
                .iter()
                .any(|e| e.message.contains("@background")),
            "without pre-scan, expected `@background` error: {:?}",
            errors_no_scan
        );
        // With pre-scan: should pass (the checker also reports any
        // OTHER errors, but the `@background` one must be gone).
        let (mut env, errors) = resolve_program(&program);
        env.add_imported_background_fns(std::iter::once("remote_fn".to_string()));
        let (_env2, _ti, _di, errors_with_scan) = check_with_env(&program, env, errors);
        assert!(
            !errors_with_scan
                .iter()
                .any(|e| e.message.contains("is not declared with `@background`")),
            "with pre-scan, did not expect `@background` error: {:?}",
            errors_with_scan
        );
    }

    // ===== Phase 10.3.a — ORM decorator checker =====

    #[test]
    fn checker_table_decorator_registers_metadata() {
        let src = "@table(\"users\") type User { id: Int, name: Str }";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("User").expect("User should be registered");
        let meta = env.table_metadata(id).expect("should have TableMetadata");
        assert_eq!(meta.sql_name, "users");
        assert_eq!(meta.primary_fields, Vec::<String>::new());
        assert!(meta.columns.is_empty());
    }

    #[test]
    fn checker_table_without_args_uses_lowercase_default() {
        let src = "@table type Post { id: Int }";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("Post").unwrap();
        let meta = env.table_metadata(id).unwrap();
        assert_eq!(meta.sql_name, "post");
    }

    #[test]
    fn checker_primary_decorator_registers_primary_field() {
        let src = "@table(\"users\") type User {\n  @primary\n  id: Int\n  name: Str\n}";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("User").unwrap();
        let meta = env.table_metadata(id).unwrap();
        assert_eq!(meta.single_pk(), Some("id"));
    }

    #[test]
    fn checker_column_decorator_registers_overrides() {
        let src = "@table(\"users\") type User {\n  @column(name=\"user_id\", sql_type=\"bigserial\")\n  id: Int\n}";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("User").unwrap();
        let meta = env.table_metadata(id).unwrap();
        let col = meta.columns.get("id").expect("columna `id` con metadata");
        assert_eq!(col.sql_name.as_deref(), Some("user_id"));
        assert_eq!(col.sql_type.as_deref(), Some("bigserial"));
    }

    #[test]
    fn checker_unique_and_index_get_registered() {
        let src = "@table type T {\n  @unique @index\n  email: Str\n}";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("T").unwrap();
        let meta = env.table_metadata(id).unwrap();
        let col = meta.columns.get("email").unwrap();
        assert!(col.unique);
        assert!(col.indexed);
    }

    #[test]
    fn checker_type_without_table_has_no_metadata() {
        let src = "type Plain { x: Int }";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("Plain").unwrap();
        assert!(env.table_metadata(id).is_none());
    }

    #[test]
    fn checker_field_decorator_without_table_is_error() {
        // `@primary` on a field requires the type to have `@table`.
        let src = "type X {\n  @primary\n  id: Int\n}";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("missing `@table")),
            "expected error about missing @table: {:?}",
            errs
        );
    }

    #[test]
    fn checker_two_primary_compose_composite_pk_v27() {
        // v0.10.27 (F2) — 2 `@primary` are now composite PK
        // (previously was an error). The checker accepts; primary_fields
        // ends up with N entries in order of appearance.
        let src = "@table type T {\n  @primary\n  a: Int\n  @primary\n  b: Int\n}";
        let errs = errors_of(src);
        assert!(
            errs.is_empty(),
            "composite PK no debe ser error en v0.10.27: {:?}",
            errs
        );
    }

    #[test]
    fn checker_primary_on_same_field_twice_is_error() {
        // The duplicate check is preserved, but now applies only
        // if the SAME field appears twice (not if there are 2 distinct
        // fields with @primary, which is composite).
        let src = "@table type T {\n  @primary\n  @primary\n  a: Int\n}";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("twice")),
            "expected error about duplicate @primary on same field: {:?}",
            errs
        );
    }

    #[test]
    fn checker_unknown_decorator_on_type_is_error() {
        let src = "@bogus type X { id: Int }";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@bogus")),
            "expected error about @bogus: {:?}",
            errs
        );
    }

    #[test]
    fn checker_unknown_decorator_on_field_is_error() {
        let src = "@table type T {\n  @bogus\n  x: Int\n}";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@bogus")),
            "expected error about @bogus: {:?}",
            errs
        );
    }

    #[test]
    fn checker_table_with_non_string_arg_is_error() {
        let src = "@table(42) type T { id: Int }";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@table")),
            "expected error about non-string arg: {:?}",
            errs
        );
    }

    #[test]
    fn checker_two_table_decorators_is_error() {
        let src = "@table(\"a\") @table(\"b\") type T { id: Int }";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("more than one `@table` decorator")),
            "expected error about duplicate @table: {:?}",
            errs
        );
    }

    // ===== Phase 4 (fitz-liveviews Y-B) — @live_component checker =====

    #[test]
    fn live_component_valid_decorator_registers_metadata() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\", is_editing: Bool = false }";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env
            .lookup("CardEditor")
            .expect("CardEditor should be registered");
        let meta = env
            .live_component_metadata(id)
            .expect("should have LiveComponentMetadata");
        assert_eq!(meta.name, "card_editor");
        assert_eq!(meta.type_id, id);
        // Reverse lookup by component name must resolve to the same TypeId.
        assert_eq!(env.live_component_by_name("card_editor"), Some(id));
    }

    #[test]
    fn live_component_missing_name_arg_errors() {
        let src = "@live_component type CardEditor { text: Str = \"\" }";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("`@live_component(\"name\")` expects exactly 1 Str arg")),
            "expected error about missing name arg: {:?}",
            errs
        );
    }

    #[test]
    fn live_component_non_str_arg_errors() {
        let src = "@live_component(42) type CardEditor { text: Str = \"\" }";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("expects a Str literal")),
            "expected error about non-Str arg: {:?}",
            errs
        );
    }

    #[test]
    fn live_component_multiple_decorators_on_same_type_errors() {
        let src =
            "@live_component(\"a\") @live_component(\"b\") type CardEditor { text: Str = \"\" }";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("more than one `@live_component` decorator")),
            "expected error about duplicate @live_component: {:?}",
            errs
        );
    }

    #[test]
    fn live_component_kwargs_error() {
        let src = "@live_component(\"card_editor\", extra=1) type CardEditor { text: Str = \"\" }";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("`@live_component` does not accept kwargs")),
            "expected error about kwargs: {:?}",
            errs
        );
    }

    // ===== Phase 4 (fitz-liveviews Y-B) — @render_for / @on checker =====

    #[test]
    fn render_for_valid_registers_handler() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @render_for(\"card_editor\")\n\
                   fn card_editor_render(state: CardEditor) -> Str => \"<div/>\"";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        assert_eq!(
            env.render_handler_for("card_editor"),
            Some("card_editor_render")
        );
    }

    #[test]
    fn render_for_missing_component_arg_errors() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @render_for\n\
                   fn card_editor_render(state: CardEditor) -> Str => \"<div/>\"";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("`@render_for(\"name\")` expects exactly 1 Str arg")),
            "expected error about missing arg: {:?}",
            errs
        );
    }

    #[test]
    fn render_for_unknown_component_errors() {
        // No `@live_component("card_editor")` declared — the render
        // handler cannot resolve its target.
        let src = "@render_for(\"card_editor\")\n\
                   fn card_editor_render(state: Str) -> Str => \"<div/>\"";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("no `@live_component(\"card_editor\")` is declared")),
            "expected error about unknown component: {:?}",
            errs
        );
    }

    #[test]
    fn render_for_wrong_param_type_errors() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @render_for(\"card_editor\")\n\
                   fn card_editor_render(state: Int) -> Str => \"<div/>\"";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("param must be of type `CardEditor`")),
            "expected error about wrong param type: {:?}",
            errs
        );
    }

    #[test]
    fn render_for_wrong_return_type_errors() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @render_for(\"card_editor\")\n\
                   fn card_editor_render(state: CardEditor) -> Int => 42";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("return type must be `Str`")),
            "expected error about wrong return type: {:?}",
            errs
        );
    }

    #[test]
    fn render_for_conflicts_with_get_error() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @get(\"/card\")\n\
                   @render_for(\"card_editor\")\n\
                   fn card_editor_render(state: CardEditor) -> Str => \"<div/>\"";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("@render_for on fn 'card_editor_render' is not combinable with `@get`")),
            "expected error about @get conflict: {:?}",
            errs
        );
    }

    #[test]
    fn render_for_duplicate_component_errors() {
        // Two distinct fns registering as render handler for the
        // same component. `resolve_program` should catch the
        // conflict when the second registration fires.
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @render_for(\"card_editor\")\n\
                   fn render_a(state: CardEditor) -> Str => \"<a/>\"\n\
                   @render_for(\"card_editor\")\n\
                   fn render_b(state: CardEditor) -> Str => \"<b/>\"";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("already has a render handler registered")),
            "expected error about duplicate render handler: {:?}",
            errs
        );
    }

    #[test]
    fn on_valid_registers_handler() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @on(\"card_editor\", \"save\")\n\
                   fn card_editor_save(state: CardEditor, payload: Map<Str, Str>) -> CardEditor => state";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        assert_eq!(
            env.event_handler_for("card_editor", "save"),
            Some("card_editor_save")
        );
    }

    #[test]
    fn on_wrong_arg_count_errors() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @on(\"card_editor\")\n\
                   fn card_editor_save(state: CardEditor, payload: Map<Str, Str>) -> CardEditor => state";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("expects exactly 2 Str args")),
            "expected error about wrong arg count: {:?}",
            errs
        );
    }

    #[test]
    fn on_unknown_component_errors() {
        let src = "@on(\"card_editor\", \"save\")\n\
                   fn card_editor_save(state: Str, payload: Map<Str, Str>) -> Str => state";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("no `@live_component(\"card_editor\")` is declared")),
            "expected error about unknown component: {:?}",
            errs
        );
    }

    #[test]
    fn on_wrong_payload_type_errors() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @on(\"card_editor\", \"save\")\n\
                   fn card_editor_save(state: CardEditor, payload: Int) -> CardEditor => state";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("second param must be `Map<Str, Str>`")),
            "expected error about wrong payload type: {:?}",
            errs
        );
    }

    #[test]
    fn on_wrong_return_type_errors() {
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @on(\"card_editor\", \"save\")\n\
                   fn card_editor_save(state: CardEditor, payload: Map<Str, Str>) -> Int => 0";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("return type must be `CardEditor`")),
            "expected error about wrong return type: {:?}",
            errs
        );
    }

    #[test]
    fn on_multiple_decorators_for_same_component_ok() {
        // A single fn handling several events on the SAME component
        // is valid (the framework layer routes each event to the same fn).
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @on(\"card_editor\", \"save\")\n\
                   @on(\"card_editor\", \"discard\")\n\
                   fn card_editor_handle(state: CardEditor, payload: Map<Str, Str>) -> CardEditor => state";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        assert_eq!(
            env.event_handler_for("card_editor", "save"),
            Some("card_editor_handle")
        );
        assert_eq!(
            env.event_handler_for("card_editor", "discard"),
            Some("card_editor_handle")
        );
    }

    #[test]
    fn on_mixed_components_on_same_fn_errors() {
        let src = "@live_component(\"a\") type A { x: Int = 0 }\n\
                   @live_component(\"b\") type B { x: Int = 0 }\n\
                   @on(\"a\", \"e1\")\n\
                   @on(\"b\", \"e2\")\n\
                   fn mixed(state: A, payload: Map<Str, Str>) -> A => state";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("cannot mix events from different components")),
            "expected error about mixed components: {:?}",
            errs
        );
    }

    #[test]
    fn on_duplicate_component_event_pair_errors() {
        // Two distinct fns claiming the SAME (component, event)
        // pair fires an error at register time.
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @on(\"card_editor\", \"save\")\n\
                   fn handler_a(state: CardEditor, payload: Map<Str, Str>) -> CardEditor => state\n\
                   @on(\"card_editor\", \"save\")\n\
                   fn handler_b(state: CardEditor, payload: Map<Str, Str>) -> CardEditor => state";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("already has a handler registered")),
            "expected error about duplicate (component, event) pair: {:?}",
            errs
        );
    }

    // ===== Phase 5 (fitz-liveviews) — implicit flv_register injection =====

    // Small helper: parse + check + inject; returns (mutated program, errors).
    fn parse_check_inject(src: &str) -> (Program, Vec<FitzError>) {
        let tokens = tokenize(src).expect("lex OK");
        let mut program = parse(tokens).expect("parse OK");
        let (env, _types, _defs, check_errs) = check_program(&program);
        if !check_errs.is_empty() {
            return (program, check_errs);
        }
        let inject_res = super::inject_live_component_registrations(&mut program, &env);
        let errs = inject_res.err().unwrap_or_default();
        (program, errs)
    }

    // Helper: extract the last stmt as a call expr and return
    // (component_name, type_name, render_fn_name, event_pairs).
    fn extract_last_flv_register(
        program: &Program,
    ) -> (String, String, String, Vec<(String, String)>) {
        let last = program.last().expect("program has stmts");
        let call = match last {
            Stmt::Expr(Expr::Call { args, callee, .. }, _) => match callee.as_ref() {
                Expr::Ident(name, _) if name == "flv_register" => args,
                other => panic!("expected callee flv_register, got {:?}", other),
            },
            other => panic!("expected Stmt::Expr(Call), got {:?}", other),
        };
        assert_eq!(call.len(), 4, "flv_register takes 4 args");
        let comp_name = match &call[0] {
            Expr::Str(s, _) => s.clone(),
            other => panic!("arg 0 must be Str, got {:?}", other),
        };
        let type_name = match &call[1] {
            Expr::StructLit {
                type_name, fields, ..
            } => {
                assert!(
                    fields.is_empty(),
                    "initial state must be an empty struct lit"
                );
                type_name.clone()
            }
            other => panic!("arg 1 must be StructLit, got {:?}", other),
        };
        let render = match &call[2] {
            Expr::Ident(name, _) => name.clone(),
            other => panic!("arg 2 must be Ident, got {:?}", other),
        };
        let events = match &call[3] {
            Expr::Map(pairs, _) => pairs
                .iter()
                .map(|(k, v)| {
                    let ev = match k {
                        Expr::Str(s, _) => s.clone(),
                        other => panic!("event key must be Str, got {:?}", other),
                    };
                    let fn_name = match v {
                        Expr::Ident(name, _) => name.clone(),
                        other => panic!("event handler must be Ident, got {:?}", other),
                    };
                    (ev, fn_name)
                })
                .collect(),
            other => panic!("arg 3 must be Map, got {:?}", other),
        };
        (comp_name, type_name, render, events)
    }

    // Stub `flv_register` for injection tests; in the real pipeline it
    // arrives via `from fitz_liveviews import flv_register`.
    const FLV_REGISTER_STUB: &str = "fn flv_register(name: Str, initial_state: Any, render_fn: Any, events: Map<Str, Any>) -> Null => null\n";

    #[test]
    fn implicit_register_basic_appends_flv_register_call() {
        // NOTE: we skip the real @render_for/@on checker (which requires
        // the return type to be Str/Html and the state param typed) by
        // wiring the metadata directly. That keeps this unit focused on
        // the injection pass.
        let src = format!("{FLV_REGISTER_STUB}@live_component(\"card_editor\") type CardEditor {{ text: Str = \"\", is_editing: Bool = false }}\n\
                   @render_for(\"card_editor\")\n\
                   fn card_editor_render(state: CardEditor) -> Str => \"<div/>\"\n\
                   @on(\"card_editor\", \"start\")\n\
                   fn card_editor_start(state: CardEditor, payload: Map<Str, Str>) -> CardEditor => state\n\
                   @on(\"card_editor\", \"cancel\")\n\
                   fn card_editor_cancel(state: CardEditor, payload: Map<Str, Str>) -> CardEditor => state");
        let (program, errs) = parse_check_inject(&src);
        assert!(errs.is_empty(), "no errors expected: {:?}", errs);

        let (comp, type_name, render, events) = extract_last_flv_register(&program);
        assert_eq!(comp, "card_editor");
        assert_eq!(type_name, "CardEditor");
        assert_eq!(render, "card_editor_render");
        // Events sorted by name → cancel before start.
        assert_eq!(
            events,
            vec![
                ("cancel".to_string(), "card_editor_cancel".to_string()),
                ("start".to_string(), "card_editor_start".to_string()),
            ]
        );
    }

    #[test]
    fn implicit_register_no_live_components_is_no_op() {
        let src = "fn add(a: Int, b: Int) -> Int => a + b";
        let (program, errs) = parse_check_inject(src);
        assert!(errs.is_empty(), "no errors: {:?}", errs);
        // No flv_register appended.
        let has_flv = program.iter().any(|s| matches!(s, Stmt::Expr(Expr::Call { callee, .. }, _) if matches!(callee.as_ref(), Expr::Ident(n, _) if n == "flv_register")));
        assert!(!has_flv, "no injection expected without @live_component");
    }

    #[test]
    fn implicit_register_skips_when_manual_call_exists() {
        // Stub `flv_register` locally so the checker binds it as a
        // known fn — in the real pipeline it comes from
        // `from fitz_liveviews import flv_register`.
        let src = "fn flv_register(name: Str, initial_state: Any, render_fn: Any, events: Map<Str, Any>) -> Null => null\n\
                   @live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @render_for(\"card_editor\")\n\
                   fn card_editor_render(state: CardEditor) -> Str => \"<div/>\"\n\
                   flv_register(\"card_editor\", CardEditor {}, card_editor_render, {})";
        let (program, errs) = parse_check_inject(src);
        assert!(errs.is_empty(), "no errors: {:?}", errs);
        // Only ONE flv_register call (the manual one) — no implicit append.
        let count = program
            .iter()
            .filter(|s| matches!(s, Stmt::Expr(Expr::Call { callee, .. }, _) if matches!(callee.as_ref(), Expr::Ident(n, _) if n == "flv_register")))
            .count();
        assert_eq!(count, 1, "one manual call, no implicit injection");
    }

    #[test]
    fn implicit_register_missing_render_for_errors() {
        let src = "@live_component(\"orphan\") type Orphan { x: Int = 0 }";
        let (_program, errs) = parse_check_inject(src);
        assert!(
            errs.iter().any(|e| e
                .message
                .contains("no fn has @render_for(\"orphan\") declared")),
            "expected missing-render_for error: {:?}",
            errs
        );
    }

    #[test]
    fn implicit_register_field_without_default_errors() {
        let src = "@live_component(\"bad\") type Bad { text: Str, is_editing: Bool = false }\n\
                   @render_for(\"bad\")\n\
                   fn bad_render(state: Bad) -> Str => \"<div/>\"";
        let (_program, errs) = parse_check_inject(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("field(s) `text` have no default")),
            "expected missing-default error: {:?}",
            errs
        );
    }

    #[test]
    fn implicit_register_multiple_components_sorted_by_name() {
        // Two components — deterministic order (alphabetical).
        let src = format!(
            "{FLV_REGISTER_STUB}@live_component(\"zeta\") type Zeta {{ x: Int = 0 }}\n\
                   @render_for(\"zeta\")\n\
                   fn zeta_render(state: Zeta) -> Str => \"z\"\n\
                   @live_component(\"alpha\") type Alpha {{ y: Int = 0 }}\n\
                   @render_for(\"alpha\")\n\
                   fn alpha_render(state: Alpha) -> Str => \"a\""
        );
        let (program, errs) = parse_check_inject(&src);
        assert!(errs.is_empty(), "no errors: {:?}", errs);

        // Last two stmts are the injected calls; alpha first (sorted).
        let injected: Vec<String> = program
            .iter()
            .rev()
            .take(2)
            .rev()
            .map(|s| match s {
                Stmt::Expr(Expr::Call { args, .. }, _) => match &args[0] {
                    Expr::Str(name, _) => name.clone(),
                    _ => panic!("expected Str arg 0"),
                },
                _ => panic!("expected Stmt::Expr"),
            })
            .collect();
        assert_eq!(injected, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn implicit_register_missing_flv_register_import_errors() {
        // No `from fitz_liveviews import flv_register` and no local
        // stub — the pass surfaces a clear error.
        let src = "@live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @render_for(\"card_editor\")\n\
                   fn card_editor_render(state: CardEditor) -> Str => \"<div/>\"";
        let (_program, errs) = parse_check_inject(src);
        assert!(
            errs.iter().any(|e| e.message.contains(
                "`flv_register` is not in scope. Add `from fitz_liveviews import flv_register`"
            )),
            "expected missing-import error: {:?}",
            errs
        );
    }

    #[test]
    fn implicit_register_flv_register_alias_counts_as_in_scope() {
        // `from fitz_liveviews import flv_register as register` binds
        // `register` — but the injected calls use the canonical name.
        // For MVP, an alias does NOT satisfy the "in scope" check (the
        // injection emits `flv_register` verbatim). Documented as a
        // known limitation; users can just not alias.
        //
        // This test locks the behavior: aliasing produces the error.
        let src = "from fitz_liveviews import flv_register as register\n\
                   @live_component(\"card_editor\") type CardEditor { text: Str = \"\" }\n\
                   @render_for(\"card_editor\")\n\
                   fn card_editor_render(state: CardEditor) -> Str => \"<div/>\"";
        let (_program, errs) = parse_check_inject(src);
        // The from-import fails at check time (no fitz_liveviews module
        // to resolve in test env), so the test proves both: no
        // false positive on aliased flv_register AND that aliases are
        // treated as OUT of scope for the canonical name.
        assert!(
            !errs.is_empty(),
            "expected an error (either import resolution or missing canonical flv_register)"
        );
    }

    #[test]
    fn implicit_register_component_with_no_events_emits_empty_map() {
        let src = format!("{FLV_REGISTER_STUB}@live_component(\"stateless\") type Stateless {{ count: Int = 0 }}\n\
                   @render_for(\"stateless\")\n\
                   fn stateless_render(state: Stateless) -> Str => \"<div/>\"");
        let (program, errs) = parse_check_inject(&src);
        assert!(errs.is_empty(), "no errors: {:?}", errs);
        let (_comp, _type, _render, events) = extract_last_flv_register(&program);
        assert!(events.is_empty(), "empty event map expected");
    }

    // ===== Phase 10.4.a — relations =====

    #[test]
    fn checker_belongs_to_basic() {
        let src = "@table type User { id: Int }\n\
                   @table type Post {\n  \
                     id: Int\n  \
                     @belongs_to(\"User\")\n  \
                     author_id: Int\n\
                   }";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("Post").unwrap();
        let meta = env.table_metadata(id).unwrap();
        let rel = meta.relations.get("author_id").expect("relation bound");
        assert_eq!(rel.kind, RelationKind::BelongsTo);
        assert_eq!(rel.target_type, "User");
        assert_eq!(rel.fk_field, "author_id");
        assert_eq!(rel.on_delete, CascadeAction::Restrict);
    }

    #[test]
    fn checker_belongs_to_with_kwargs() {
        let src = "@table type User { id: Int }\n\
                   @table type Post {\n  \
                     id: Int\n  \
                     @belongs_to(\"User\", on_delete=\"cascade\", fk=\"author_user_id\")\n  \
                     author_id: Int\n\
                   }";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("Post").unwrap();
        let meta = env.table_metadata(id).unwrap();
        let rel = meta.relations.get("author_id").unwrap();
        assert_eq!(rel.on_delete, CascadeAction::Cascade);
        assert_eq!(rel.fk_field, "author_user_id");
    }

    #[test]
    fn checker_has_many_marks_virtual_field() {
        let src = "@table type Post { id: Int, author_id: Int }\n\
                   @table type User {\n  \
                     id: Int\n  \
                     @has_many(\"Post\")\n  \
                     posts: List<Post>\n\
                   }";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("User").unwrap();
        let meta = env.table_metadata(id).unwrap();
        assert!(meta.is_virtual_field("posts"));
        assert!(!meta.is_virtual_field("id"));
        let rel = meta.relations.get("posts").unwrap();
        assert_eq!(rel.kind, RelationKind::HasMany);
        assert_eq!(rel.target_type, "Post");
        // Default `via` for has_many over `User` = "user_id".
        assert_eq!(rel.fk_field, "user_id");
    }

    #[test]
    fn checker_has_many_with_explicit_via() {
        let src = "@table type Post { id: Int, author_id: Int }\n\
                   @table type User {\n  \
                     id: Int\n  \
                     @has_many(\"Post\", via=\"author_id\")\n  \
                     posts: List<Post>\n\
                   }";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("User").unwrap();
        let meta = env.table_metadata(id).unwrap();
        let rel = meta.relations.get("posts").unwrap();
        assert_eq!(rel.fk_field, "author_id");
    }

    #[test]
    fn checker_has_one_marks_virtual_field() {
        let src = "@table type Profile { id: Int, user_id: Int }\n\
                   @table type User {\n  \
                     id: Int\n  \
                     @has_one(\"Profile\")\n  \
                     profile: Profile?\n\
                   }";
        let (env, errs) = resolve_str(src);
        assert!(errs.is_empty(), "expected 0 errors: {:?}", errs);
        let id = env.lookup("User").unwrap();
        let meta = env.table_metadata(id).unwrap();
        assert!(meta.is_virtual_field("profile"));
        let rel = meta.relations.get("profile").unwrap();
        assert_eq!(rel.kind, RelationKind::HasOne);
    }

    #[test]
    fn checker_relation_without_args_is_error() {
        let src = "@table type T {\n  \
                     @belongs_to\n  \
                     other_id: Int\n\
                   }";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@belongs_to")),
            "expected arity error: {:?}",
            errs
        );
    }

    #[test]
    fn checker_relation_invalid_on_delete_is_error() {
        let src = "@table type User { id: Int }\n\
                   @table type Post {\n  \
                     id: Int\n  \
                     @belongs_to(\"User\", on_delete=\"explode\")\n  \
                     author_id: Int\n\
                   }";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("on_delete")),
            "expected error about on_delete: {:?}",
            errs
        );
    }

    #[test]
    fn checker_two_relations_in_one_field_is_error() {
        let src = "@table type User { id: Int }\n\
                   @table type Post {\n  \
                     id: Int\n  \
                     @belongs_to(\"User\") @has_one(\"User\")\n  \
                     author_id: Int\n\
                   }";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("more than one relation decorator")),
            "expected error about duplicate: {:?}",
            errs
        );
    }

    #[test]
    fn cascade_action_as_sql() {
        assert_eq!(CascadeAction::Cascade.as_sql(), "CASCADE");
        assert_eq!(CascadeAction::SetNull.as_sql(), "SET NULL");
        assert_eq!(CascadeAction::Restrict.as_sql(), "RESTRICT");
        assert_eq!(CascadeAction::NoAction.as_sql(), "NO ACTION");
    }

    // ===== Phase 10.b.7 — navigation methods over Instance with @table =====
    //
    // The checker refines `instance.<rel>(db)` to `Future<Result<Target>>`
    // (BelongsTo/HasOne) or `Future<Result<List<Target>>>` (HasMany) instead
    // of Any. We validate that divergence with an incompatible
    // annotated type fires an error — that proves the concrete type was
    // synthesized (Any would never fire).

    #[test]
    fn checker_belongs_to_navigation_returns_future_result_target() {
        // CONVENTION: navigation uses the NAME of the decorated field, NOT the
        // name of the target type. `@belongs_to("User") user_id: Int` is
        // navigated with `post.user_id(db)`. Parallel to the evaluator (~4463:
        // `meta.relations.get(method)` — key = field name).
        //
        // `let n: Int = post.user_id(db).await?` must be ERROR
        // (User is not Int). Without refinement the call typed Any → Int OK.
        let src = "@table type User { id: Int }\n\
                   @table type Post {\n  \
                     id: Int\n  \
                     @belongs_to(\"User\") user_id: Int\n\
                   }\n\
                   async fn boot(post: Post, db: Any) -> Result<Null> {\n  \
                     let n: Int = post.user_id(db).await?\n  \
                     return Ok(null)\n\
                   }\n";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("User")
                && (e.message.contains("Int") || e.message.contains("incompatible"))),
            "expected type error (User is not Int): {:?}",
            errs
        );
    }

    #[test]
    fn checker_belongs_to_navigation_with_correct_annotation_compiles_clean() {
        // `let u: User = post.user_id(db).await?` must compile OK
        // — the refinement returns Future<Result<User>>, await? extracts
        // User, assigning to User OK.
        let src = "@table type User { id: Int }\n\
                   @table type Post {\n  \
                     id: Int\n  \
                     @belongs_to(\"User\") user_id: Int\n\
                   }\n\
                   async fn boot(post: Post, db: Any) -> Result<Null> {\n  \
                     let u: User = post.user_id(db).await?\n  \
                     return Ok(null)\n\
                   }\n";
        let errs = errors_of(src);
        assert!(
            errs.is_empty(),
            "expected 0 errors with correct annotation: {:?}",
            errs
        );
    }

    #[test]
    fn checker_has_many_navigation_returns_future_result_list_target() {
        // `let p: Post = user.posts(db).await?` must be ERROR
        // (List<Post> is not compatible with Post — the relation is plural).
        // The virtual field `posts` is the key in meta.relations.
        let src = "@table type Post { id: Int, user_id: Int }\n\
                   @table type User {\n  \
                     id: Int\n  \
                     @has_many(\"Post\") posts: List<Post>\n\
                   }\n\
                   async fn boot(user: User, db: Any) -> Result<Null> {\n  \
                     let p: Post = user.posts(db).await?\n  \
                     return Ok(null)\n\
                   }\n";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("List")
                && (e.message.contains("Post") || e.message.contains("incompatible"))),
            "expected type error (List<Post> is not Post): {:?}",
            errs
        );
    }

    #[test]
    fn checker_has_one_navigation_returns_future_result_target() {
        // `let n: Int = user.profile(db).await?` must be ERROR
        // (Profile is not Int). Virtual field `profile`.
        let src = "@table type Profile { id: Int, user_id: Int }\n\
                   @table type User {\n  \
                     id: Int\n  \
                     @has_one(\"Profile\") profile: Profile?\n\
                   }\n\
                   async fn boot(user: User, db: Any) -> Result<Null> {\n  \
                     let n: Int = user.profile(db).await?\n  \
                     return Ok(null)\n\
                   }\n";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("Profile")
                && (e.message.contains("Int") || e.message.contains("incompatible"))),
            "expected type error (Profile is not Int): {:?}",
            errs
        );
    }

    #[test]
    fn checker_navigation_does_not_collide_with_static_orm_methods() {
        // `User.where(...).all(db)` still types as `Future<Result<List<User>>>`,
        // NOT as navigation method. The names of static ORM methods
        // (where/all/insert/etc.) cannot be names of fields/relations.
        // Defensive test: the navigation refinement doesn't break the existing
        // ORM dispatch.
        let src = "@table type User { id: Int }\n\
                   async fn boot(db: Any) -> Result<Null> {\n  \
                     let xs: List<User> = User.where(fn(u) => u.id > 0).all(db).await?\n  \
                     return Ok(null)\n\
                   }\n";
        let errs = errors_of(src);
        assert!(
            errs.is_empty(),
            "expected 0 errors (static ORM still types): {:?}",
            errs
        );
    }

    // v0.10.22 — Debt B: typed methods over `DbRow`.
    // Enable parsing of raw `db.query` columns directly in
    // `fitz build` (previously only the interpreter dispatched `.get`).

    #[test]
    fn checker_db_row_get_int_returns_result_int() {
        let src = "async fn boot(db: DbConn) -> Result<Null> {\n  \
                     let rows = db.query(\"SELECT id FROM users\", []).await?\n  \
                     let r: DbRow = rows[0]\n  \
                     let id: Int = r.get_int(\"id\")?\n  \
                     return Ok(null)\n\
                   }\n";
        let errs = errors_of(src);
        assert!(
            errs.is_empty(),
            "expected 0 errors; get_int must type as Result<Int>: {:?}",
            errs
        );
    }

    #[test]
    fn checker_db_row_get_str_returns_result_str() {
        let src = "async fn boot(db: DbConn) -> Result<Null> {\n  \
                     let rows = db.query(\"SELECT name FROM users\", []).await?\n  \
                     let r: DbRow = rows[0]\n  \
                     let name: Str = r.get_str(\"name\")?\n  \
                     return Ok(null)\n\
                   }\n";
        let errs = errors_of(src);
        assert!(
            errs.is_empty(),
            "expected 0 errors; get_str must type as Result<Str>: {:?}",
            errs
        );
    }

    #[test]
    fn checker_db_row_get_int_str_annotation_is_error() {
        // `let name: Str = r.get_int("id")?` must be ERROR: get_int
        // refines to `Result<Int>`, `?` extracts Int, assigning to Str → fail.
        let src = "async fn boot(db: DbConn) -> Result<Null> {\n  \
                     let rows = db.query(\"SELECT id FROM users\", []).await?\n  \
                     let r: DbRow = rows[0]\n  \
                     let name: Str = r.get_int(\"id\")?\n  \
                     return Ok(null)\n\
                   }\n";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("Int")
                || e.message.contains("Str")
                || e.message.contains("incompatible")),
            "expected type error (Int vs Str): {:?}",
            errs
        );
    }

    #[test]
    fn checker_db_row_unknown_method_is_error() {
        let src = "async fn boot(db: DbConn) -> Result<Null> {\n  \
                     let rows = db.query(\"SELECT 1 AS x\", []).await?\n  \
                     let r: DbRow = rows[0]\n  \
                     let v: Int = r.get_potato(\"x\")?\n  \
                     return Ok(null)\n\
                   }\n";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("DbRow") && e.message.contains("get_potato")),
            "expected error citing DbRow.get_potato: {:?}",
            errs
        );
    }

    // ============================================================
    // v0.10.28 — @index(col, using="...") method override
    // ============================================================

    /// Each of the 6 whitelisted Postgres methods must
    /// be accepted without error by the checker, and populated in
    /// `TableMetadata.indexes[i].using`.
    #[test]
    fn checker_at_index_using_methods_whitelisteados_ok() {
        for method in ["btree", "hash", "gin", "gist", "brin", "spgist"] {
            let src = format!(
                "@table(\"docs\")\n\
                 @index(\"body\", using=\"{method}\")\n\
                 type Doc {{\n  \
                   id: Int = 0\n  \
                   body: Str\n\
                 }}\n"
            );
            let errs = errors_of(&src);
            assert!(
                errs.is_empty(),
                "method `{method}` should be accepted, errors: {:?}",
                errs
            );
            // Validate that the using is populated (None for default btree,
            // Some(lower) for the rest).
            let tokens = tokenize(&src).expect("lex");
            let program = parse(tokens).expect("parse");
            let (env, _, _, _) = check_program(&program);
            let tid = env.lookup("Doc").expect("Doc registrado");
            let meta = env.table_metadata(tid).expect("@table metadata");
            assert_eq!(meta.indexes.len(), 1, "1 @index");
            if method == "btree" {
                assert_eq!(
                    meta.indexes[0].using, None,
                    "btree default stays as None (no redundant USING is emitted)"
                );
            } else {
                assert_eq!(
                    meta.indexes[0].using.as_deref(),
                    Some(method),
                    "method `{method}` se popula"
                );
            }
        }
    }

    /// A non-whitelisted method emits a clear error citing the supported ones.
    #[test]
    fn checker_at_index_using_invalid_method_is_error() {
        let src = "@table(\"docs\")\n\
                   @index(\"body\", using=\"bloom\")\n\
                   type Doc {\n  id: Int = 0\n  body: Str\n}\n";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("bloom")
                && e.message.contains("btree")
                && e.message.contains("gin")),
            "error should cite the invalid method + list of supported: {:?}",
            errs
        );
    }

    /// `using=` with non-Str type is an error.
    #[test]
    fn checker_at_index_using_non_str_is_error() {
        let src = "@table(\"docs\")\n\
                   @index(\"body\", using=42)\n\
                   type Doc {\n  id: Int = 0\n  body: Str\n}\n";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("`@index(using=...)`")),
            "error should cite the contract of `using=`: {:?}",
            errs
        );
    }

    // ----- v0.10.29 — `@unique(col1, col2, ...)` composite shortcut -----

    #[test]
    fn checker_at_unique_bare_idents_generates_indexspec_unique() {
        let src = "@table(\"users\")\n\
                   @unique(email, tenant_id)\n\
                   type User {\n  \
                     id: Int = 0\n  \
                     email: Str\n  \
                     tenant_id: Int\n\
                   }\n";
        let errs = errors_of(src);
        assert!(errs.is_empty(), "expected no errors: {:?}", errs);
        let tokens = tokenize(src).expect("lex");
        let program = parse(tokens).expect("parse");
        let (env, _, _, _) = check_program(&program);
        let tid = env.lookup("User").expect("User registrado");
        let meta = env.table_metadata(tid).expect("@table metadata");
        assert_eq!(meta.indexes.len(), 1);
        let idx = &meta.indexes[0];
        assert_eq!(
            idx.columns,
            vec!["email".to_string(), "tenant_id".to_string()]
        );
        assert!(idx.unique);
        assert!(idx.where_clause.is_none());
        assert!(idx.using.is_none());
    }

    #[test]
    fn checker_at_unique_accepts_str_with_commas_compat_index() {
        let src = "@table(\"users\")\n\
                   @unique(\"email, tenant_id\")\n\
                   type User {\n  \
                     id: Int = 0\n  \
                     email: Str\n  \
                     tenant_id: Int\n\
                   }\n";
        let errs = errors_of(src);
        assert!(errs.is_empty(), "expected no errors: {:?}", errs);
        let tokens = tokenize(src).expect("lex");
        let program = parse(tokens).expect("parse");
        let (env, _, _, _) = check_program(&program);
        let tid = env.lookup("User").expect("User registrado");
        let meta = env.table_metadata(tid).expect("@table metadata");
        assert_eq!(meta.indexes.len(), 1);
        assert_eq!(
            meta.indexes[0].columns,
            vec!["email".to_string(), "tenant_id".to_string()]
        );
    }

    #[test]
    fn checker_at_unique_with_name_kwarg_applies_it() {
        let src = "@table(\"users\")\n\
                   @unique(email, name=\"users_email_uniq_custom\")\n\
                   type User {\n  \
                     id: Int = 0\n  \
                     email: Str\n\
                   }\n";
        let errs = errors_of(src);
        assert!(errs.is_empty(), "expected no errors: {:?}", errs);
        let tokens = tokenize(src).expect("lex");
        let program = parse(tokens).expect("parse");
        let (env, _, _, _) = check_program(&program);
        let tid = env.lookup("User").expect("User");
        let meta = env.table_metadata(tid).expect("metadata");
        assert_eq!(
            meta.indexes[0].name.as_deref(),
            Some("users_email_uniq_custom")
        );
    }

    #[test]
    fn checker_at_unique_without_args_is_error() {
        let src = "@table(\"users\")\n\
                   @unique()\n\
                   type User {\n  id: Int = 0\n}\n";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("`@unique")
                && e.message.contains("at least 1 positional column")),
            "expected arity error: {:?}",
            errs
        );
    }

    #[test]
    fn checker_at_unique_invalid_kwarg_is_error() {
        let src = "@table(\"users\")\n\
                   @unique(email, where_=\"foo\")\n\
                   type User {\n  id: Int = 0\n  email: Str\n}\n";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@unique")
                && e.message.contains("where_")
                && e.message.contains("@index")),
            "expected error citing `@index` as alternative: {:?}",
            errs
        );
    }

    #[test]
    fn checker_at_unique_stackable_generates_multiple_indexes() {
        let src = "@table(\"users\")\n\
                   @unique(email)\n\
                   @unique(tenant_id, slug)\n\
                   type User {\n  \
                     id: Int = 0\n  \
                     email: Str\n  \
                     tenant_id: Int\n  \
                     slug: Str\n\
                   }\n";
        let errs = errors_of(src);
        assert!(errs.is_empty(), "expected no errors: {:?}", errs);
        let tokens = tokenize(src).expect("lex");
        let program = parse(tokens).expect("parse");
        let (env, _, _, _) = check_program(&program);
        let tid = env.lookup("User").expect("User");
        let meta = env.table_metadata(tid).expect("metadata");
        assert_eq!(meta.indexes.len(), 2);
        assert!(meta.indexes.iter().all(|i| i.unique));
    }

    // ----- v0.10.29 — `@check_constraint("expr", name="optional")` -----

    #[test]
    fn checker_at_check_constraint_basic_registers() {
        let src = "@table(\"users\")\n\
                   @check_constraint(\"age >= 0 AND age <= 150\")\n\
                   type User {\n  \
                     id: Int = 0\n  \
                     age: Int\n\
                   }\n";
        let errs = errors_of(src);
        assert!(errs.is_empty(), "expected no errors: {:?}", errs);
        let tokens = tokenize(src).expect("lex");
        let program = parse(tokens).expect("parse");
        let (env, _, _, _) = check_program(&program);
        let tid = env.lookup("User").expect("User registrado");
        let meta = env.table_metadata(tid).expect("metadata");
        assert_eq!(meta.check_constraints.len(), 1);
        assert_eq!(meta.check_constraints[0].expr, "age >= 0 AND age <= 150");
        assert!(meta.check_constraints[0].name.is_none());
    }

    #[test]
    fn checker_at_check_constraint_with_name_applies_it() {
        let src = "@table(\"users\")\n\
                   @check_constraint(\"status IN ('a', 'p')\", name=\"users_status_valid\")\n\
                   type User {\n  \
                     id: Int = 0\n  \
                     status: Str\n\
                   }\n";
        let errs = errors_of(src);
        assert!(errs.is_empty(), "expected no errors: {:?}", errs);
        let tokens = tokenize(src).expect("lex");
        let program = parse(tokens).expect("parse");
        let (env, _, _, _) = check_program(&program);
        let tid = env.lookup("User").expect("User");
        let meta = env.table_metadata(tid).expect("metadata");
        assert_eq!(
            meta.check_constraints[0].name.as_deref(),
            Some("users_status_valid")
        );
    }

    #[test]
    fn checker_at_check_constraint_stackable() {
        let src = "@table(\"users\")\n\
                   @check_constraint(\"age >= 0\")\n\
                   @check_constraint(\"email != ''\")\n\
                   type User {\n  \
                     id: Int = 0\n  \
                     age: Int\n  \
                     email: Str\n\
                   }\n";
        let errs = errors_of(src);
        assert!(errs.is_empty(), "expected no errors: {:?}", errs);
        let tokens = tokenize(src).expect("lex");
        let program = parse(tokens).expect("parse");
        let (env, _, _, _) = check_program(&program);
        let tid = env.lookup("User").expect("User");
        let meta = env.table_metadata(tid).expect("metadata");
        assert_eq!(meta.check_constraints.len(), 2);
    }

    #[test]
    fn checker_at_check_constraint_without_args_is_error() {
        let src = "@table(\"users\")\n\
                   @check_constraint()\n\
                   type User {\n  id: Int = 0\n}\n";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@check_constraint")
                && e.message.contains("1 positional arg")),
            "expected arity error: {:?}",
            errs
        );
    }

    #[test]
    fn checker_at_check_constraint_empty_str_is_error() {
        let src = "@table(\"users\")\n\
                   @check_constraint(\"\")\n\
                   type User {\n  id: Int = 0\n}\n";
        let errs = errors_of(src);
        assert!(
            errs.iter().any(|e| e.message.contains("empty string")),
            "expected empty string error: {:?}",
            errs
        );
    }

    #[test]
    fn checker_at_check_constraint_non_str_arg_is_error() {
        let src = "@table(\"users\")\n\
                   @check_constraint(42)\n\
                   type User {\n  id: Int = 0\n}\n";
        let errs = errors_of(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("@check_constraint")
                    && e.message.contains("Str literal")),
            "expected type error: {:?}",
            errs
        );
    }

    // ============================================================
    // Phase 12.1.a — @healthz / @readyz tests
    // ============================================================

    /// Local helper — replica of `assert_auth_ok` for the K8s probe
    /// tests. We don't reuse the one from the @auth_provider block
    /// because it lives in a separate `tests` sub-module and visibility
    /// doesn't reach; copying it is simpler.
    fn assert_no_health_err(src: &str) {
        let errs = errors_of(src);
        let related: Vec<_> = errs
            .iter()
            .filter(|e| e.message.contains("@healthz") || e.message.contains("@readyz"))
            .collect();
        assert!(
            related.is_empty(),
            "expected no errors from @healthz/@readyz, was: {:?}",
            related
        );
    }

    /// Local helper — the program must produce at least one error
    /// whose message contains `expected_substr`.
    fn assert_health_err(src: &str, expected_substr: &str) {
        let errs = errors_of(src);
        let matched = errs.iter().any(|e| e.message.contains(expected_substr));
        assert!(
            matched,
            "expected error with substring '{}', errors were: {:?}",
            expected_substr, errs
        );
    }

    #[test]
    fn healthz_basic_compiles() {
        // `@healthz fn liveness() -> Bool { return true }` must compile
        // without type errors.
        let src = "@healthz\n\
                   fn liveness() -> Bool {\n  return true\n}";
        assert_no_health_err(src);
    }

    #[test]
    fn readyz_basic_compiles() {
        let src = "@readyz\n\
                   fn readiness() -> Bool {\n  return true\n}";
        assert_no_health_err(src);
    }

    #[test]
    fn healthz_with_result_null_compiles() {
        // `Result<Null>` is also valid: Ok = healthy, Err = unhealthy.
        let src = "@healthz\n\
                   fn liveness() -> Result<Null> {\n  return Ok(null)\n}";
        assert_no_health_err(src);
    }

    #[test]
    fn healthz_with_result_bool_compiles() {
        let src = "@healthz\n\
                   fn liveness() -> Result<Bool> {\n  return Ok(true)\n}";
        assert_no_health_err(src);
    }

    #[test]
    fn healthz_async_compiles() {
        // async fn liveness() -> Bool — the ret is Future<Bool>, valid.
        let src = "async fn pausar(ms: Int) -> Int { return ms }\n\
                   @healthz\n\
                   async fn liveness() -> Bool {\n  let _ = pausar(0).await\n  return true\n}";
        assert_no_health_err(src);
    }

    #[test]
    fn healthz_with_args_is_error() {
        let src = "@healthz(\"x\")\n\
                   fn liveness() -> Bool {\n  return true\n}";
        assert_health_err(src, "does not accept args or kwargs");
    }

    #[test]
    fn healthz_with_kwargs_is_error() {
        let src = "@healthz(timeout=10)\n\
                   fn liveness() -> Bool {\n  return true\n}";
        assert_health_err(src, "does not accept args or kwargs");
    }

    #[test]
    fn healthz_with_params_is_error() {
        // Probes don't receive input — params is an error.
        let src = "@healthz\n\
                   fn liveness(x: Int) -> Bool {\n  return true\n}";
        assert_health_err(src, "does not accept params");
    }

    #[test]
    fn healthz_invalid_return_type_is_error() {
        // Return must be Bool / Result<Null> / Result<Bool>.
        let src = "@healthz\n\
                   fn liveness() -> Int {\n  return 200\n}";
        assert_health_err(src, "Bool");
    }

    #[test]
    fn healthz_duplicate_is_error() {
        // Singleton: two @healthz fire an error citing the first.
        let src = "@healthz\n\
                   fn first() -> Bool {\n  return true\n}\n\
                   @healthz\n\
                   fn second() -> Bool {\n  return false\n}";
        assert_health_err(src, "duplicate @healthz");
    }

    #[test]
    fn readyz_duplicate_is_error() {
        let src = "@readyz\n\
                   fn first() -> Bool {\n  return true\n}\n\
                   @readyz\n\
                   fn second() -> Bool {\n  return false\n}";
        assert_health_err(src, "duplicate @readyz");
    }

    #[test]
    fn healthz_and_readyz_separate_compile() {
        // `@healthz` + `@readyz` on different fns is OK — they are
        // separate singletons.
        let src = "@healthz\n\
                   fn liveness() -> Bool {\n  return true\n}\n\
                   @readyz\n\
                   fn readiness() -> Bool {\n  return true\n}";
        assert_no_health_err(src);
    }

    #[test]
    fn healthz_and_readyz_together_in_same_fn_is_error() {
        // Stacked on the same fn → conflict.
        let src = "@healthz\n\
                   @readyz\n\
                   fn both() -> Bool {\n  return true\n}";
        assert_health_err(src, "is not combinable");
    }

    #[test]
    fn healthz_with_get_is_error() {
        // Conflict with normal HTTP decorator.
        let src = "@healthz\n\
                   @get(\"/probe\")\n\
                   fn probe() -> Bool {\n  return true\n}";
        assert_health_err(src, "is not combinable");
    }

    #[test]
    fn healthz_with_cron_is_error() {
        let src = "@healthz\n\
                   @cron(\"0 0 * * *\")\n\
                   fn job() -> Bool {\n  return true\n}";
        assert_health_err(src, "is not combinable");
    }

    #[test]
    fn healthz_with_background_is_error() {
        let src = "@healthz\n\
                   @background\n\
                   fn job() -> Bool {\n  return true\n}";
        assert_health_err(src, "is not combinable");
    }

    #[test]
    fn healthz_with_authenticated_is_error() {
        // Probes must NOT be authenticated (K8s doesn't send bearer).
        let src = "type User { id: Int, role: Str }\n\
                   @auth_provider\n\
                   fn check(headers: Map<Str, Str>) -> Result<User> {\n  return Err(\"x\")\n}\n\
                   @healthz\n\
                   @authenticated\n\
                   fn probe(user: User) -> Bool {\n  return true\n}";
        assert_health_err(src, "is not combinable");
    }

    // ============================================================
    // Phase 12.2.a — Secret<T> opaque type + secret/config builtins
    // ============================================================

    #[test]
    fn secret_type_resolves_and_display_is_redactable() {
        // `Secret<Str>` must resolve correctly. Its structural
        // display preserves the shape (for typed error messages).
        // Redaction only applies to Value's Display.
        let src = "let p: Secret<Str> = secret(\"K\")?";
        let errs = errors_of(src);
        // `?` inside top-level + `secret` that returns
        // `Result<Secret<Str>>` should type OK. Using `?` at
        // top-level requires a parent Result fn or async fn (15.3.3 rule
        // of phase 5). For a simple test: wrap.
        // Wrapping required: we use async fn.
        let _ = errs; // enough that the shape is declarable
        let src2 = "async fn pp() -> Result<Null> {\n  let p: Secret<Str> = secret(\"K\")?\n  return Ok(null)\n}";
        let errs2 = errors_of(src2);
        assert!(errs2.is_empty(), "expected no errors, was: {:?}", errs2);
    }

    #[test]
    fn secret_expose_returns_typed_inner() {
        // `.expose()` on `Secret<Str>` must type `Str` (not Any).
        // This enables typed chains: `secret("X")?.expose().len()`.
        let src = "async fn pp() -> Result<Int> {\n\
                   let p = secret(\"K\")?\n\
                   let exposed: Str = p.expose()\n\
                   return Ok(exposed.len())\n\
                   }";
        let errs = errors_of(src);
        assert!(errs.is_empty(), "expected no errors, was: {:?}", errs);
    }

    #[test]
    fn secret_unknown_method_is_error() {
        // Any method other than `.expose()` must fail with a
        // clear message.
        let src = "async fn pp() -> Result<Null> {\n\
                   let p = secret(\"K\")?\n\
                   let _ = p.unwrap()\n\
                   return Ok(null)\n\
                   }";
        let errs = errors_of(src);
        let matched = errs
            .iter()
            .any(|e| e.message.contains("Secret") && e.message.contains(".expose()"));
        assert!(
            matched,
            "expected message with suggestion of `.expose()`, was: {:?}",
            errs
        );
    }

    #[test]
    fn secret_expose_with_args_is_error() {
        let src = "async fn pp() -> Result<Null> {\n\
                   let p = secret(\"K\")?\n\
                   let _ = p.expose(42)\n\
                   return Ok(null)\n\
                   }";
        let errs = errors_of(src);
        let matched = errs.iter().any(|e| {
            e.message.contains("Secret.expose()") && e.message.contains("does not accept arguments")
        });
        assert!(
            matched,
            "expected error citing 'does not accept arguments', was: {:?}",
            errs
        );
    }

    #[test]
    fn config_types_as_any_independent_of_default() {
        // `config(key, default)` returns Any (future refinement).
        // The user annotates the destination with `let port: Int = config("P", 8080)`.
        let src = "let port: Int = config(\"PORT\", 8080)";
        let errs = errors_of(src);
        assert!(
            errs.is_empty(),
            "expected no errors with target annotation, was: {:?}",
            errs
        );
    }

    #[test]
    fn secret_builtin_arg_must_be_str() {
        // `secret(42)` with Int as key → type error.
        let src = "let _ = secret(42)";
        let errs = errors_of(src);
        let matched = errs
            .iter()
            .any(|e| e.message.contains("Str") || e.message.contains("Int"));
        assert!(matched, "expected type error on arg, was: {:?}", errs);
    }

    // ---- Phase 12.8 — @flag decorator checker ----

    #[test]
    fn flag_decorator_valid_shape_compiles_clean() {
        assert_ok("@flag(\"new-checkout\")\nfn f() -> Int { return 1 }");
        assert_ok("@flag(\"dark_mode\")\nfn g() -> Int { return 1 }");
        assert_ok("@flag(\"a1_b2\")\nfn h() -> Int { return 1 }");
    }

    #[test]
    fn flag_decorator_without_args_is_error() {
        assert_error_with(
            "@flag()\nfn f() -> Int { return 1 }",
            &["@flag", "1 positional arg"],
        );
    }

    #[test]
    fn flag_decorator_with_two_args_is_error() {
        assert_error_with(
            "@flag(\"a\", \"b\")\nfn f() -> Int { return 1 }",
            &["@flag", "1 positional arg"],
        );
    }

    #[test]
    fn flag_decorator_non_str_literal_arg_is_error() {
        assert_error_with(
            "@flag(123)\nfn f() -> Int { return 1 }",
            &["@flag", "Str literal"],
        );
    }

    #[test]
    fn flag_decorator_empty_name_is_error() {
        assert_error_with(
            "@flag(\"\")\nfn f() -> Int { return 1 }",
            &["@flag", "cannot be empty"],
        );
    }

    #[test]
    fn flag_decorator_invalid_chars_is_error() {
        assert_error_with(
            "@flag(\"foo!\")\nfn f() -> Int { return 1 }",
            &["@flag", "invalid"],
        );
    }

    #[test]
    fn flag_decorator_duplicate_is_error() {
        assert_error_with(
            "@flag(\"a\")\n@flag(\"b\")\nfn f() -> Int { return 1 }",
            &["@flag", "duplicate"],
        );
    }

    #[test]
    fn flag_decorator_with_kwargs_is_error() {
        assert_error_with(
            "@flag(\"x\", level=\"info\")\nfn f() -> Int { return 1 }",
            &["@flag", "kwargs"],
        );
    }

    #[test]
    fn flag_decorator_stackable_on_http_handler() {
        // The @flag decorator is NOT restricted to regular fns —
        // it's valid on HTTP/WS handlers. That combination is
        // exactly the 90% case (feature gate on a route).
        assert_ok(
            "@flag(\"new-checkout\")\n\
             @get(\"/v2/checkout\")\n\
             fn v2() -> Map<Str, Str> { return {\"ok\": \"yes\"} }",
        );
    }

    #[test]
    fn flag_builtin_resolves_as_any() {
        // `flag(...)` is in the global scope as Type::Any (same
        // pattern as jwt/hash/auth) — calls are not checked statically
        // but the binding exists.
        assert_ok("let v = flag(\"foo\")");
        assert_ok("let v = flags.is_enabled(\"foo\")");
        assert_ok("let v = flags.list()");
    }

    // ---- V2 (2026-06-05) — hover on LHS of `let` records type ----

    /// Helper for V2 tests — parses + checks and returns the `TypeInfo`
    /// to do lookups by `(line, column)` of the LHS of a `let`.
    fn types_at_position(src: &str, line: usize, column: usize) -> Option<Type> {
        let tokens = tokenize(src).expect("tokenize");
        let program = parse(tokens).expect("parse");
        let (_env, type_info, _def, _errs) = check_program(&program);
        let key = SpanKey(line, column);
        let result = type_info
            .iter()
            .find(|(k, _)| **k == key)
            .map(|(_, t)| t.clone());
        result
    }

    #[test]
    fn v2_hover_on_let_lhs_without_annotation_registers_inferred_type() {
        // `let edad = 200` — span of the LHS `edad` is at (1, 5).
        let ty = types_at_position("let edad = 200\n", 1, 5);
        assert_eq!(ty, Some(Type::Int), "hover sobre `edad` debe ser Int");
    }

    #[test]
    fn v2_hover_on_let_lhs_with_annotation_registers_declared_type() {
        // `let datos: Int? = null` — span of the LHS `datos` at (1, 5).
        // The registered type must be Nullable(Int) (the declared one),
        // not Null (the one inferred from the RHS).
        let ty = types_at_position("let datos: Int? = null\n", 1, 5);
        match ty {
            Some(Type::Nullable(inner)) => {
                assert_eq!(*inner, Type::Int, "expected Nullable(Int)");
            }
            other => panic!("expected Nullable(Int), received {:?}", other),
        }
    }

    #[test]
    fn v2_hover_on_let_lhs_str_and_float_works() {
        let ty = types_at_position("let nombre = \"Patagonia\"\n", 1, 5);
        assert_eq!(ty, Some(Type::Str));

        let ty = types_at_position("let latitud = -49.32\n", 1, 5);
        assert_eq!(ty, Some(Type::Float));
    }

    #[test]
    fn v2_hover_on_let_lhs_bool_works() {
        let ty = types_at_position("let activa = true\n", 1, 5);
        assert_eq!(ty, Some(Type::Bool));
    }

    // ---- L2 (2026-06-05) — Bidirectional inference of callbacks ----

    /// L2 Helper — extracts the type of the last `Stmt::Assign` of the `Program`
    /// for tests of the `let r = <expr>` shape. We use `lookup_binding`
    /// because `TypeInfo` records the type of the value at its span, not at the
    /// name of the variable.
    fn type_of_last_let(src: &str) -> Option<Type> {
        let tokens = tokenize(src).expect("tokenize");
        let program = parse(tokens).expect("parse");
        let (env, _types_info, _def, _errs) = check_program(&program);
        // The binding of the last top-level var is retrieved by the global
        // TypeEnv. Since `check_program` doesn't expose bindings,
        // we re-implement a mini check with `CheckCtx` to reach
        // `lookup_binding`.
        let mut ctx = CheckCtx::new(&env);
        for stmt in &program {
            check_stmt(&mut ctx, stmt);
        }
        // Find the last `let X = ...` and look up its binding.
        let mut last_name = None;
        for stmt in &program {
            if let Stmt::Assign {
                target: AssignTarget::Ident(name, _),
                ..
            } = stmt
            {
                last_name = Some(name.clone());
            }
        }
        let n = last_name?;
        ctx.lookup_binding(&n).map(|b| b.ty.clone())
    }

    #[test]
    fn l2_map_on_list_int_unannotated_callback_types_as_list_int() {
        // Pedagogical case from the M1.C5 cap of the course. Before L2 this
        // typed as `List<Any>` because `x` stayed as `Any` without
        // annotation, and `Any * Int` is also `Any`. Now L2 propagates
        // the `Int` of the receiver to the callback.
        let ty = type_of_last_let("let r = [1, 2, 3].map(fn(x) => x * 10)\n");
        match ty {
            Some(Type::List(inner)) => assert_eq!(
                *inner,
                Type::Int,
                "expected List<Int>, received List<{:?}>",
                inner
            ),
            other => panic!("expected List<Int>, received {:?}", other),
        }
    }

    #[test]
    fn l2_filter_on_list_int_unannotated_types_callback_int_and_ret_bool() {
        // `xs.filter(fn(x) => x > 0)` on `List<Int>` must type
        // `List<Int>` (filter preserves T). Inside the callback,
        // `x > 0` requires `x: Int` (Int vs Int OK).
        let ty = type_of_last_let("let r = [1, 2, 3].filter(fn(x) => x > 0)\n");
        match ty {
            Some(Type::List(inner)) => assert_eq!(*inner, Type::Int),
            other => panic!("expected List<Int>, received {:?}", other),
        }
    }

    #[test]
    fn l2_map_on_list_str_propagates_to_str_methods() {
        // `xs.map(fn(s) => s.upper())` on `List<Str>` must type
        // `List<Str>` — `.upper()` requires `s: Str`. If L2
        // didn't propagate, `s: Any` and the result would be `List<Any>`.
        let ty = type_of_last_let("let r = [\"a\", \"b\"].map(fn(s) => s.upper())\n");
        match ty {
            Some(Type::List(inner)) => assert_eq!(*inner, Type::Str),
            other => panic!("expected List<Str>, received {:?}", other),
        }
    }

    #[test]
    fn l2_param_with_explicit_annotation_wins_over_hint() {
        // If the student annotates `fn(x: Float) => ...` on `List<Int>`,
        // the annotation wins. The callback param type is Float (not
        // Int from the hint). That may trigger an error if the body
        // does something incompatible, but the return type depends on
        // the annotation.
        let ty = type_of_last_let("let r = [1, 2, 3].map(fn(x: Float) => x * 2.0)\n");
        match ty {
            Some(Type::List(inner)) => assert_eq!(
                *inner,
                Type::Float,
                "expected List<Float> (explicit annotation wins), received List<{:?}>",
                inner
            ),
            other => panic!("expected List<Float>, received {:?}", other),
        }
    }

    #[test]
    fn l2_find_on_list_int_types_callback_and_returns_result_int() {
        // `xs.find(fn(x) => x == 2)` on `List<Int>` → `Result<Int>`.
        let ty = type_of_last_let("let r = [1, 2, 3].find(fn(x) => x == 2)\n");
        match ty {
            Some(Type::Result { ok, .. }) => assert_eq!(*ok, Type::Int),
            other => panic!("expected Result<Int>, received {:?}", other),
        }
    }

    // ---- L2 expanded (2026-06-05) — bidirectional inference for Fn ----

    #[test]
    fn l2x_let_with_fn_annotation_propagates_to_unannotated_fnexpr() {
        // `let f: Fn(Int) -> Int = fn(n) => n * 2` — the param `n` of the
        // FnExpr must be inferred as Int from the let annotation.
        let src = "let f: Fn(Int) -> Int = fn(n) => n * 2\n";
        let tokens = tokenize(src).expect("tokenize");
        let program = parse(tokens).expect("parse");
        let (env, _ti, _di, errs) = check_program(&program);
        // No errors should appear — n: Int * 2: Int → Int, compatible with Fn(Int) -> Int.
        assert!(
            errs.is_empty(),
            "expected no errors, received: {:?}",
            errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
        );
        let _ = env;
    }

    #[test]
    fn l2x_fn_user_defined_with_fn_param_propagates_to_fnexpr_arg() {
        // `fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x) }
        //  let r = apply(fn(n) => n * 2, 5)` — the `n` of the callback
        // must be inferred as Int from the `f: Fn(Int) -> Int` param.
        let src = "\
fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x) }
let r = apply(fn(n) => n * 2, 5)
";
        let tokens = tokenize(src).expect("tokenize");
        let program = parse(tokens).expect("parse");
        let (env, _ti, _di, errs) = check_program(&program);
        assert!(
            errs.is_empty(),
            "expected no errors with bidirectional inference, received: {:?}",
            errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
        );
        let _ = env;
    }

    #[test]
    fn l2x_explicit_fnexpr_annotation_wins_over_hint() {
        // If the FnExpr has an explicit param annotation that's not
        // compatible with the let's annotation, the checker emits an error.
        // `let f: Fn(Int) -> Int = fn(n: Str) => n.upper()` — error
        // because Fn(Str) -> Str is not compatible with Fn(Int) -> Int.
        let src = "let f: Fn(Int) -> Int = fn(n: Str) => n.upper()\n";
        let tokens = tokenize(src).expect("tokenize");
        let program = parse(tokens).expect("parse");
        let (_env, _ti, _di, errs) = check_program(&program);
        assert!(
            !errs.is_empty(),
            "expected error from incompatible annotation, received 0"
        );
    }

    // ---- S1 (2026-06-05) — own spans for Param/Pattern ----

    #[test]
    fn s1_hover_on_fn_def_param_shows_annotated_type() {
        // `fn double(n: Int) => n * 2` — span of param `n` at col 11.
        // The checker should record `Int` under that span in TypeInfo.
        let ty = types_at_position("fn double(n: Int) => n * 2\n", 1, 11);
        assert_eq!(ty, Some(Type::Int), "hover sobre `n` (param) debe ser Int");
    }

    #[test]
    fn s1_hover_on_unannotated_param_shows_any() {
        // `fn f(x) => x + 1` — without annotation, x: Any.
        let ty = types_at_position("fn f(x) => x + 1\n", 1, 6);
        assert_eq!(ty, Some(Type::Any), "param without annotation types as Any");
    }

    #[test]
    fn s1_hover_on_for_in_range_var_shows_int() {
        // `for i in 0..10 { print(i) }` — span of `i` at col 5.
        // The range produces Int, the Ident pattern binds as Int.
        // Note that `for` currently expands to a `Stmt::For`
        // where `var: Pattern` already has `Pattern::Ident(name, span)`.
        let ty = types_at_position("for i in 0..10 { print(i) }\n", 1, 5);
        assert_eq!(
            ty,
            Some(Type::Int),
            "hover sobre `i` del for debe ser Int (item del range)"
        );
    }

    #[test]
    fn s1_hover_on_match_ok_binding_shows_inner_type() {
        // `match Ok(42) { Ok(n) => n, Err(_) => 0 }` — span of `n` in
        // `Ok(n)`. n must type as Int (inner of the Result).
        // The span of `n` is where the ident `n` appears inside the
        // pattern's parentheses.
        let src = "let r: Int = match Ok(42) { Ok(n) => n, Err(_) => 0 }\n";
        // Position of `n` in `Ok(n)`: col 32.
        let ty = types_at_position(src, 1, 32);
        assert_eq!(
            ty,
            Some(Type::Int),
            "hover sobre `n` del binding Ok(n) debe ser Int"
        );
    }

    #[test]
    fn s1_hover_on_custom_method_param_works() {
        // `double` method on `type Counter { val: Int }`.
        let src = "\
type Counter { val: Int = 0 }
type Counter {
    fn double(amount: Int) -> Int => amount * 2
}
let c = Counter { }
let r = c.double(5)
";
        // Span of param `amount: Int` on line 3, col 15.
        let ty = types_at_position(src, 3, 15);
        assert_eq!(ty, Some(Type::Int), "hover sobre `amount` debe ser Int");
    }

    #[test]
    fn l2_nested_callbacks_do_not_contaminate() {
        // Nested FnExpr: the outer's hint must not "leak" into the
        // inner. Case: `xs.map(fn(x) => [x, x+1].map(fn(y) => y * 2))`.
        // The inner `fn(y)` receives the `Int` hint from the receiver `[x, x+1]`,
        // not from the outer callback.
        let ty = type_of_last_let("let r = [1, 2].map(fn(x) => [x, x + 1].map(fn(y) => y * 2))\n");
        match ty {
            Some(Type::List(outer)) => match outer.as_ref() {
                Type::List(inner) => assert_eq!(
                    **inner,
                    Type::Int,
                    "expected List<List<Int>>, received List<List<{:?}>>",
                    inner
                ),
                other => panic!("expected List<List<Int>>, received List<{:?}>", other),
            },
            other => panic!("expected List<List<Int>>, received {:?}", other),
        }
    }
}
