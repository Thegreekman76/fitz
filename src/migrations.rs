//! Phase 10.6 — Automatic ORM migrations.
//!
//! Provides 4 capabilities:
//!
//! 1. **PG introspection** ([`introspect_schema`]): queries the
//!    current schema of the DB (tables/columns/indexes/FKs) via
//!    `information_schema` + `pg_catalog`. Returns a [`Schema`]
//!    that is the "snapshot" of the real state.
//! 2. **Schema from @table types** ([`schema_from_program`]):
//!    walks the AST of the Fitz program + the `TypeEnv` with the
//!    `TableMetadata` resolved by the checker → returns the
//!    "expected" [`Schema`] (target).
//! 3. **Diff algorithm** ([`diff_schemas`]): compares `current` vs
//!    `target` and emits an ordered list of [`Change`] (CREATE
//!    TABLE before INDEX, FK at the end, etc.).
//! 4. **SQL emission** ([`changes_to_sql`]): each [`Change`] knows
//!    how to generate itself as a Postgres DDL statement.
//!
//! The module is NOT embedded in the codegen output (`fitz build`)
//! — it lives only in the `fitz` CLI binary. User binaries do not
//! need introspection/migration capabilities; that is on the
//! developer side.

use crate::db::{DbConnHandle, DbError, DbResult, PgValue};

// =================================================================
// Schema model
// =================================================================

/// Snapshot of a Postgres DB schema. Built both via introspection
/// of the real DB and derived from the Fitz program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schema {
    pub tables: Vec<Table>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
    pub foreign_keys: Vec<ForeignKey>,
    /// v0.10.17 (10.6.b.2) — When present, hints to the diff that
    /// the table USED TO BE NAMED `renamed_from` before; emits
    /// `ALTER TABLE "old" RENAME TO "new"` instead of
    /// `DROP + CREATE`. Only populated in the "target" snapshot
    /// (from `schema_from_program`); `introspect_schema` leaves
    /// it as `None`.
    pub renamed_from: Option<String>,
    /// v0.10.21 (10.6.e.3) — Custom Postgres schema. `None` =
    /// `public` (default). When `Some(s)`, the SQL emit uses
    /// `"s"."name"` qualified everywhere.
    pub schema: Option<String>,
    /// v0.10.27 (F2) — composite PK. If present and `len() >= 2`,
    /// the CREATE TABLE emits `PRIMARY KEY (a, b)` as a table-level
    /// constraint (individual columns do NOT emit `PRIMARY KEY`
    /// inline). If present and `len() == 1`, redundant with
    /// `Column.is_primary` — we prefer the inline in that case. If
    /// empty or None, there is no composite. The introspect
    /// populates from `pg_constraint` with contype='p'.
    pub composite_pk: Vec<String>,
    /// v0.10.29 — `CHECK (<expr>)` constraints declared with
    /// `@check_constraint("expr", name="optional")` at the type
    /// level. Empty = no checks. Only populated in the migrator
    /// "target" snapshot (from `schema_from_program`);
    /// `introspect_schema` does NOT read them from `pg_constraint`
    /// (MVP — minor debt). Consequence: the diff does NOT detect
    /// drift of checks. If the user changes an `expr` Fitz-side
    /// without recreating the table, the DB stays with the old
    /// one. Workaround: manual drop + create.
    pub check_constraints: Vec<CheckConstraint>,
}

/// v0.10.29 — CHECK constraint declared with `@check_constraint(...)`.
/// The `expr` is the boolean SQL expression that Postgres validates
/// on INSERT/UPDATE. The `name` is auto-generated if not specified
/// (`<table>_<idx>_check`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckConstraint {
    pub name: String,
    pub expr: String,
}

impl Table {
    /// v0.10.21 — Cross-schema identity. Two tables are the same
    /// if and only if their `(schema, name)` matches. `None`
    /// schema is treated as `"public"` for canonical comparison
    /// (matches any introspect that reports `public` explicitly).
    pub fn qualified_id(&self) -> (String, String) {
        let s = self.schema.as_deref().unwrap_or("public");
        (s.to_string(), self.name.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// Canonicalized SQL type (`bigint`/`text`/`boolean`/etc.).
    pub sql_type: String,
    pub nullable: bool,
    /// `DEFAULT <expr>` declared, without the `DEFAULT` keyword.
    /// `None` if the column has no default.
    pub default: Option<String>,
    pub is_primary: bool,
    /// v0.10.17 (10.6.b.2) — When present, hints to the diff that
    /// the column USED TO BE NAMED `renamed_from` before; emits
    /// `ALTER TABLE ... RENAME COLUMN "old" TO "new"` instead of
    /// `DROP + ADD`. Only populated in target.
    pub renamed_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    /// v0.10.27 (F3) — SQL WHERE clause for partial indexes
    /// (`CREATE INDEX ... WHERE deleted_at IS NULL`). `None` = full
    /// index. When present, the diff compares the exact string vs
    /// what Postgres reports (whitespace/canonical form may differ
    /// — best-effort match, fallback to regenerate the index).
    pub where_clause: Option<String>,
    /// v0.10.28 — Access method (`USING <method>`). `None` = btree
    /// (Postgres default, redundant `USING` is not emitted).
    /// `Some("gin"|"gist"|"brin"|"hash"|"spgist")` for overrides.
    /// Introspect reads `pg_am.amname`; the target schema loads it
    /// from `IndexSpec.using`. Name-based diff (same as
    /// where_clause) — changing ONLY the method does not trigger
    /// DROP/CREATE; rename the index to force regen (minor debt —
    /// follows the v0.10.27 pattern with where_clause).
    pub using: Option<String>,
    /// v0.10.32 (Tier C.2) — Expression index. When present,
    /// `columns` is ignored and the CREATE INDEX uses the raw
    /// expression (e.g.: `lower(email)`, `to_tsvector('english', body)`).
    /// Name-based diff with the same limitation as
    /// where_clause/using: introspect does NOT parse
    /// `pg_index.indexprs` to detect the expression, so a change
    /// of the expression with the same name is NOT detected as
    /// drift (refactor the name to force regen).
    pub expression: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub name: String,
    pub column: String,
    pub references_table: String,
    pub references_column: String,
    /// v0.10.29 — Custom target schema. `None` = same schema as the
    /// current table (parallel to the Postgres `public` convention).
    /// When `Some(s)`, the SQL emit uses `REFERENCES "s"."table"(col)`
    /// qualified, enabling transparent cross-schema FKs (the user
    /// keeps declaring `@belongs_to("User")` with a Fitz name —
    /// Fitz resolves the target schema from its `@table("schema.name")`).
    /// Introspect leaves it as `None` for MVP simplicity —
    /// cross-schema drift remains a minor debt.
    pub references_schema: Option<String>,
    /// `CASCADE` / `SET NULL` / `RESTRICT` / `NO ACTION`. None if
    /// not declared (= NO ACTION Postgres default).
    pub on_delete: Option<String>,
}

// =================================================================
// PG introspection
// =================================================================

/// Queries the current schema of the DB connected via `conn` and
/// returns a [`Schema`] with all user-tables (excludes
/// `pg_catalog`, `information_schema`, and the internal table
/// `_fitz_migrations` that tracks migrate).
///
/// Exclusion policy:
/// - Considered schemas: ALL user schemas (excludes `pg_catalog`,
///   `information_schema`, `pg_toast`, `pg_temp_*`).
/// - `table_type = 'BASE TABLE'` (skips views).
/// - Explicitly excludes `_fitz_migrations`.
///
/// v0.10.21 (10.6.e.3) — Multi-schema: introspect iterates ALL
/// user schemas, not just `public`. Tables in custom schemas
/// appear with `Table.schema = Some("schema_name")`; tables in
/// `public` keep `schema = None` by convention (compat with
/// pre-v0.10.21 code).
pub async fn introspect_schema(conn: &std::sync::Arc<DbConnHandle>) -> DbResult<Schema> {
    let qualified = list_user_tables_qualified(conn).await?;
    let mut tables = Vec::with_capacity(qualified.len());
    for (schema, name) in &qualified {
        let columns = introspect_columns(conn, schema, name).await?;
        let indexes = introspect_indexes(conn, schema, name).await?;
        let foreign_keys = introspect_foreign_keys(conn, schema, name).await?;
        // v0.10.31 (Tier A.7) — table-level CHECK constraints
        // (pg_constraint.contype='c'). Enables drift check: if the
        // target declares `@check_constraint("...")` different from
        // the DB, the diff emits DROP+ADD. Before v0.10.31, current
        // was always empty and the diff did not detect changes — the
        // old version of the CHECK stayed in the DB until manual drop.
        let check_constraints = introspect_check_constraints(conn, schema, name).await?;
        let schema_for_struct = if schema == "public" {
            None
        } else {
            Some(schema.clone())
        };
        // v0.10.27 (F2) — composite PK: if there are >=2 columns
        // with is_primary=true post-introspect, we accumulate them
        // here. Single PK stays in Column.is_primary inline
        // (composite_pk empty).
        let pk_cols_introspected: Vec<String> = columns
            .iter()
            .filter(|c| c.is_primary)
            .map(|c| c.name.clone())
            .collect();
        let composite_pk = if pk_cols_introspected.len() >= 2 {
            // Clear is_primary inline (the source of truth for
            // composite is the table-level list) — otherwise diff vs
            // target (which has composite_pk + is_primary=false on
            // cols) emits false changes.
            let mut cols_normalized = columns.clone();
            for c in cols_normalized.iter_mut() {
                if c.is_primary {
                    c.is_primary = false;
                }
            }
            tables.push(Table {
                name: name.clone(),
                columns: cols_normalized,
                indexes,
                foreign_keys,
                renamed_from: None,
                schema: schema_for_struct,
                composite_pk: pk_cols_introspected,
                check_constraints: check_constraints.clone(),
            });
            continue;
        } else {
            Vec::new()
        };
        tables.push(Table {
            name: name.clone(),
            columns,
            indexes,
            foreign_keys,
            renamed_from: None,
            schema: schema_for_struct,
            composite_pk,
            check_constraints,
        });
    }
    // Deterministic order: schema first, then name. `public`
    // tables (schema=None → "public" for sort) first by alphabetical
    // order of the canonical name.
    tables.sort_by_key(|a| a.qualified_id());
    Ok(Schema { tables })
}

/// v0.10.21 — Lists user-tables with their schema. Returns ordered
/// `(schema, name)` tuples. Excludes system schemas and
/// `_fitz_migrations`.
async fn list_user_tables_qualified(
    conn: &std::sync::Arc<DbConnHandle>,
) -> DbResult<Vec<(String, String)>> {
    let sql = "SELECT table_schema, table_name FROM information_schema.tables \
               WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
                 AND table_schema NOT LIKE 'pg_toast%' \
                 AND table_schema NOT LIKE 'pg_temp_%' \
                 AND table_type = 'BASE TABLE' \
                 AND table_name <> '_fitz_migrations' \
               ORDER BY table_schema, table_name";
    let qr = conn.query(sql, &[]).await?;
    let mut out = Vec::with_capacity(qr.rows.len());
    for row in &qr.rows {
        let schema = extract_string(row, "table_schema")?;
        let name = extract_string(row, "table_name")?;
        out.push((schema, name));
    }
    Ok(out)
}

/// Lists the user-tables of the `public` schema. Excludes system
/// tables + `_fitz_migrations`. Kept for compat — prefer
/// `list_user_tables_qualified` for multi-schema.
#[allow(dead_code)]
async fn list_user_tables(conn: &std::sync::Arc<DbConnHandle>) -> DbResult<Vec<String>> {
    let sql = "SELECT table_name FROM information_schema.tables \
               WHERE table_schema = 'public' \
                 AND table_type = 'BASE TABLE' \
                 AND table_name <> '_fitz_migrations' \
               ORDER BY table_name";
    let qr = conn.query(sql, &[]).await?;
    qr.rows
        .iter()
        .map(|row| extract_string(row, "table_name"))
        .collect()
}

/// Lists columns of a table in declaration order
/// (`ordinal_position`). Detects `is_primary` by joining with
/// `pg_catalog.pg_index` (the PK does not appear as such in
/// `information_schema.columns`).
async fn introspect_columns(
    conn: &std::sync::Arc<DbConnHandle>,
    schema: &str,
    table: &str,
) -> DbResult<Vec<Column>> {
    // Base cols: name + type + nullable + default.
    let sql_cols = "SELECT column_name, data_type, udt_name, is_nullable, column_default \
                    FROM information_schema.columns \
                    WHERE table_schema = $1 AND table_name = $2 \
                    ORDER BY ordinal_position";
    let qr = conn
        .query(
            sql_cols,
            &[
                PgValue::Text(schema.to_string()),
                PgValue::Text(table.to_string()),
            ],
        )
        .await?;
    let mut columns: Vec<Column> = Vec::with_capacity(qr.rows.len());
    for row in &qr.rows {
        let name = extract_string(row, "column_name")?;
        let data_type = extract_string(row, "data_type")?;
        let udt_name = extract_string(row, "udt_name").unwrap_or_default();
        let is_nullable = extract_string(row, "is_nullable")?;
        let default = extract_string_opt(row, "column_default");
        columns.push(Column {
            name,
            sql_type: canonicalize_sql_type(&data_type, &udt_name),
            nullable: is_nullable.eq_ignore_ascii_case("YES"),
            default,
            is_primary: false, // filled in below
            renamed_from: None,
        });
    }
    // PK: join with pg_index to flag `is_primary`. The `regclass`
    // cast resolves `schema.table` correctly.
    let sql_pk = "SELECT a.attname AS column_name \
                  FROM pg_index i \
                  JOIN pg_attribute a ON a.attrelid = i.indrelid \
                                     AND a.attnum = ANY(i.indkey) \
                  WHERE i.indrelid = ($1::regclass) AND i.indisprimary";
    let pk_qr = conn
        .query(sql_pk, &[PgValue::Text(format!("{schema}.{table}"))])
        .await?;
    let pk_cols: std::collections::HashSet<String> = pk_qr
        .rows
        .iter()
        .filter_map(|r| extract_string(r, "column_name").ok())
        .collect();
    for c in columns.iter_mut() {
        if pk_cols.contains(&c.name) {
            c.is_primary = true;
            // v0.10.16 — PG reports `column_default = nextval(...)`
            // for `bigserial` PK; the target schema NEVER emits it
            // (it's implicit from `PRIMARY KEY` with `bigserial`).
            // We clear it to avoid a false positive in the diff.
            if let Some(d) = &c.default {
                if d.starts_with("nextval(") {
                    c.default = None;
                }
            }
        }
    }
    Ok(columns)
}

/// Lists user-defined indexes of a table. Excludes the auto-PK
/// (which have `indisprimary = true`) and the auto-UNIQUE
/// constraints (already represented at column level in our
/// abstraction, not as a separate Index).
async fn introspect_indexes(
    conn: &std::sync::Arc<DbConnHandle>,
    schema: &str,
    table: &str,
) -> DbResult<Vec<Index>> {
    // v0.10.27 (F3) — `pg_get_expr(i.indpred, i.indrelid)` returns
    // the WHERE clause of partial indexes (`NULL` for full).
    // v0.10.28 — `am.amname` returns the index access method
    // (`btree`/`gin`/`gist`/`hash`/`brin`/`spgist`). Mapping to `using`:
    // `"btree"` → `None` (default, redundant `USING btree` is not
    // re-emitted); rest → `Some(lowercase)`.
    let sql = "SELECT \
                   c.relname AS index_name, \
                   i.indisunique AS is_unique, \
                   am.amname AS access_method, \
                   array_to_string( \
                       ARRAY( \
                           SELECT a.attname \
                           FROM unnest(i.indkey) WITH ORDINALITY AS k(idx, ord) \
                           JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.idx \
                           ORDER BY k.ord \
                       ), ',') AS column_names, \
                   pg_get_expr(i.indpred, i.indrelid) AS where_clause \
               FROM pg_index i \
               JOIN pg_class c ON c.oid = i.indexrelid \
               JOIN pg_am am ON am.oid = c.relam \
               WHERE i.indrelid = ($1::regclass) \
                 AND i.indisprimary = false";
    let qr = conn
        .query(sql, &[PgValue::Text(format!("{schema}.{table}"))])
        .await?;
    let mut indexes = Vec::with_capacity(qr.rows.len());
    for row in &qr.rows {
        let name = extract_string(row, "index_name")?;
        let cols_csv = extract_string(row, "column_names")?;
        let cols: Vec<String> = cols_csv.split(',').map(|s| s.trim().to_string()).collect();
        let is_unique = extract_bool(row, "is_unique").unwrap_or(false);
        // pg_get_expr returns NULL (= PgValue::Null) if not partial.
        let where_clause = match row.get("where_clause") {
            Some(PgValue::Text(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        let using = match extract_string(row, "access_method").ok() {
            Some(m) if !m.eq_ignore_ascii_case("btree") => Some(m.to_ascii_lowercase()),
            _ => None,
        };
        indexes.push(Index {
            name,
            columns: cols,
            unique: is_unique,
            where_clause,
            using,
            // v0.10.32 (Tier C.2) — introspect does NOT parse
            // `pg_index.indexprs` for expression indexes. The user
            // who declares `@index(expression="...")` must name it
            // explicitly with `name=` so that the diff matches it.
            expression: None,
        });
    }
    indexes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(indexes)
}

/// Lists FKs declared on a table. Each FK = one local column
/// that points to `(referenced_table, referenced_column)` with
/// the declared `ON DELETE` rule.
async fn introspect_foreign_keys(
    conn: &std::sync::Arc<DbConnHandle>,
    schema: &str,
    table: &str,
) -> DbResult<Vec<ForeignKey>> {
    // v0.10.31 (Tier A.8) — cross-schema FK drift. We also pull
    // `ccu.table_schema AS ref_schema` to detect FKs that point to
    // tables in other schemas (`@belongs_to("schema.User")`). If
    // the target schema == local schema (same `tc.table_schema`),
    // we leave it as `None` to match the `schema_from_program`
    // convention (same-schema → None). If they differ →
    // `Some(ref_schema)` so that the diff compares cross-schema
    // without false changes.
    let sql = "SELECT \
                   tc.constraint_name AS name, \
                   kcu.column_name AS local_column, \
                   ccu.table_schema AS ref_schema, \
                   ccu.table_name AS ref_table, \
                   ccu.column_name AS ref_column, \
                   rc.delete_rule AS on_delete \
               FROM information_schema.table_constraints tc \
               JOIN information_schema.key_column_usage kcu \
                   ON tc.constraint_name = kcu.constraint_name \
                  AND tc.table_schema = kcu.table_schema \
               JOIN information_schema.constraint_column_usage ccu \
                   ON ccu.constraint_name = tc.constraint_name \
                  AND ccu.constraint_schema = tc.constraint_schema \
               JOIN information_schema.referential_constraints rc \
                   ON rc.constraint_name = tc.constraint_name \
                  AND rc.constraint_schema = tc.constraint_schema \
               WHERE tc.constraint_type = 'FOREIGN KEY' \
                 AND tc.table_schema = $1 \
                 AND tc.table_name = $2 \
               ORDER BY tc.constraint_name, kcu.ordinal_position";
    let qr = conn
        .query(
            sql,
            &[
                PgValue::Text(schema.to_string()),
                PgValue::Text(table.to_string()),
            ],
        )
        .await?;
    let mut fks = Vec::with_capacity(qr.rows.len());
    for row in &qr.rows {
        let name = extract_string(row, "name")?;
        let local_column = extract_string(row, "local_column")?;
        let ref_schema_raw = extract_string(row, "ref_schema")?;
        let ref_table = extract_string(row, "ref_table")?;
        let ref_column = extract_string(row, "ref_column")?;
        let on_delete = extract_string_opt(row, "on_delete").and_then(|s| {
            // PG returns "NO ACTION" as a literal string; we
            // normalize it to None to avoid differing vs schemas
            // that did NOT declare on_delete (same default).
            if s.eq_ignore_ascii_case("NO ACTION") {
                None
            } else {
                Some(s)
            }
        });
        // v0.10.31 (Tier A.8) — references_schema only when it
        // differs from the local schema (parallel to the
        // same-schema=None convention of `schema_from_program`).
        // This preserves that the diff does not emit false changes
        // for legacy same-schema FKs.
        let references_schema = if ref_schema_raw == schema {
            None
        } else {
            Some(ref_schema_raw)
        };
        fks.push(ForeignKey {
            name,
            column: local_column,
            references_table: ref_table,
            references_column: ref_column,
            references_schema,
            on_delete,
        });
    }
    Ok(fks)
}

/// v0.10.31 (Tier A.7) — Lists table-level CHECK constraints
/// declared. Reads `pg_constraint` with `contype = 'c'` (the only
/// table-level checks; column-level checks like `CHECK (n > 0)`
/// inline in `ADD COLUMN` are also `contype='c'` in PG). The
/// `pg_get_constraintdef` returns the canonicalized SQL
/// expression (`CHECK (price >= 0)`), which we trim to
/// `(price >= 0)` to match the shape the user passes in
/// `@check_constraint("price >= 0")`.
///
/// Excludes inherited constraints (with `conislocal = false`) and
/// those auto-generated by NOT NULL (they do not appear as
/// contype='c' in PG 14+, but some legacy ones do — we filter
/// them by `convalidated`).
async fn introspect_check_constraints(
    conn: &std::sync::Arc<DbConnHandle>,
    schema: &str,
    table: &str,
) -> DbResult<Vec<CheckConstraint>> {
    let sql = "SELECT \
                   con.conname AS name, \
                   pg_get_constraintdef(con.oid) AS def \
               FROM pg_catalog.pg_constraint con \
               JOIN pg_catalog.pg_class rel ON rel.oid = con.conrelid \
               JOIN pg_catalog.pg_namespace ns ON ns.oid = rel.relnamespace \
               WHERE con.contype = 'c' \
                 AND con.conislocal = true \
                 AND ns.nspname = $1 \
                 AND rel.relname = $2 \
               ORDER BY con.conname";
    let qr = conn
        .query(
            sql,
            &[
                PgValue::Text(schema.to_string()),
                PgValue::Text(table.to_string()),
            ],
        )
        .await?;
    let mut checks = Vec::with_capacity(qr.rows.len());
    for row in &qr.rows {
        let name = extract_string(row, "name")?;
        let def = extract_string(row, "def")?;
        // `pg_get_constraintdef` returns `CHECK ((expr))` with
        // double-paren by PG convention. We normalize it to `expr`
        // (without `CHECK` or wrapping parens) to match the shape
        // that `schema_from_program` loads from the decorator.
        let expr = parse_check_def(&def);
        checks.push(CheckConstraint { name, expr });
    }
    Ok(checks)
}

/// v0.10.31 (Tier A.7) — extracts the `expr` from the output of
/// `pg_get_constraintdef`. PG emits `CHECK (<expr>)` or
/// `CHECK ((<expr>))` (sometimes double paren if the expr is
/// composite). We trim `CHECK ` at the start and all balanced
/// outer paren pairs.
///
/// The result is not necessarily identical to the user's original
/// string (PG canonicalizes whitespace, case, etc.) — the diff
/// uses trim+exact comparison, so a purely cosmetic change may
/// trigger a spurious DROP+ADD. Minor debt — refinable with a
/// SQL normalizer parser if pressure appears.
fn parse_check_def(def: &str) -> String {
    let trimmed = def.trim();
    let after_check = trimmed.strip_prefix("CHECK ").unwrap_or(trimmed).trim();
    // Iterate: if the outer parens are balanced (i.e., the first
    // `(` closes exactly at the last `)`), we peel one level.
    // `(a) AND (b)` is NOT peeled (the first `)` closes at an
    // internal position). PG sometimes emits 2 levels for complex
    // exprs.
    let mut current = after_check.to_string();
    loop {
        let s = current.trim().to_string();
        if !(s.starts_with('(') && s.ends_with(')')) {
            return s;
        }
        let bytes = s.as_bytes();
        let mut depth: i32 = 0;
        let mut closes_at_end = true;
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 && i < bytes.len() - 1 {
                        // The outer parens do not wrap everything
                        // (case `(a) AND (b)`).
                        closes_at_end = false;
                        break;
                    }
                }
                _ => {}
            }
        }
        if !closes_at_end || depth != 0 {
            return s;
        }
        current = s[1..s.len() - 1].trim().to_string();
    }
}

// =================================================================
// Schema from @table types of the Fitz program
// =================================================================

/// Builds the "expected" [`Schema`] by walking the AST of the
/// program + joining with [`crate::types::TypeEnv`] to resolve the
/// [`crate::types::TableMetadata`] of each `@table` type.
///
/// Mapping rules:
/// - Each `@table("name") type T { ... }` → 1 [`Table`] with
///   `name = TableMetadata.sql_name`.
/// - Fields without decorator → 1 [`Column`] with derived SQL type.
/// - Fields with `@belongs_to(...)` → 1 [`Column`] (real FK) + 1
///   [`ForeignKey`] with referenced_table/column + on_delete.
/// - Fields with `@has_one`/`@has_many` / `BelongsToCompanion` →
///   **skip** (virtual, do not go to the DB).
/// - Fields with `@index` → 1 [`Index`] (single-column, standard
///   name `<table>_<col>_idx`).
/// - Fields with `@unique` → 1 [`Index`] with `unique=true` (PG
///   creates unique constraint backed by unique index).
/// - Field with `@primary` → flags [`Column.is_primary = true`].
///   The default `Int = 0` is translated to `bigserial` (PG
///   auto-increment).
///
/// Fitz types → SQL:
/// - `Int` → `bigint` (or `bigserial` if `@primary`)
/// - `Float` → `double precision`
/// - `Str` → `text`
/// - `Bool` → `boolean`
/// - `List<T>` → `<T>[]` (Postgres array)
/// - `Map<Str, _>` → `jsonb`
/// - `Nullable<T>` → same SQL type as inner, `nullable = true`
pub fn schema_from_program(
    program: &crate::ast::Program,
    type_env: &crate::types::TypeEnv,
) -> Result<Schema, String> {
    let mut tables = Vec::new();
    for stmt in program {
        let crate::ast::Stmt::TypeDef {
            name,
            fields: ast_fields,
            ..
        } = stmt
        else {
            continue;
        };
        let Some(type_id) = type_env.lookup(name) else {
            continue;
        };
        let Some(meta) = type_env.table_metadata(type_id) else {
            // No `@table` — skip.
            continue;
        };
        let table = build_table_from_type(name, ast_fields, meta, type_env)?;
        tables.push(table);
    }
    // v0.10.21 — Order by canonical (schema, name) so that the
    // diff is deterministic cross-schema (`public.users` BEFORE
    // `analytics.events` lex).
    tables.sort_by_key(|a| a.qualified_id());
    Ok(Schema { tables })
}

fn build_table_from_type(
    type_name: &str,
    ast_fields: &[crate::ast::Field],
    meta: &crate::types::TableMetadata,
    type_env: &crate::types::TypeEnv,
) -> Result<Table, String> {
    let table_name = meta.sql_name.clone();
    let mut columns = Vec::new();
    let mut indexes = Vec::new();
    let mut foreign_keys = Vec::new();

    for f in ast_fields {
        // Skip virtuals (has_one/has_many/companion).
        if meta.is_virtual_field(&f.name) {
            continue;
        }
        let col_meta = meta.columns.get(&f.name);
        // v0.10.27 (F2) — composite PK: the field is PK if it is in
        // `primary_fields` (not only if it is the ONLY one). For
        // single PK it keeps working identically.
        let is_primary = meta.primary_fields.iter().any(|p| p == &f.name);
        // v0.10.27 (F2) — the CREATE TABLE inline `<col> <type> PRIMARY KEY`
        // is only emitted if SINGLE PK (1 field). For composite PK,
        // the constraint goes at the end `PRIMARY KEY (a, b)` and
        // the individual cols only emit `NOT NULL` (inline would
        // break).
        let is_inline_pk = is_primary && meta.primary_fields.len() == 1;

        // Fitz type → resolve via TypeExpr → SQL.
        let (fitz_inner_ty, nullable) = unwrap_nullable_typeexpr(&f.type_);
        // v0.10.27 (F2) — `bigserial` (auto-increment) is only
        // emitted for SINGLE PK Int. Composite PK: each col is a
        // normal `bigint` (the user provides explicit values for
        // the PK tuple).
        let sql_type = fitz_typeexpr_to_sql_type(&fitz_inner_ty, is_inline_pk, col_meta)?;
        let sql_name = col_meta
            .and_then(|c| c.sql_name.clone())
            .unwrap_or_else(|| f.name.clone());

        // SQL default: the ORM emits the CREATE TABLE without
        // explicit DEFAULTS (defaults are applied client-side when
        // constructing the instance). EXCEPTIONS:
        // - `@primary Int = 0` → `bigserial` already implies
        //   default nextval; we do NOT emit an extra DEFAULT.
        // - `@db_default("<sql>")` (v0.10.16) — the user passes the
        //   explicit SQL expression and the diff emits it in
        //   `CREATE TABLE` / `ADD COLUMN` (e.g. `DEFAULT NOW()`).
        // - `@db_default` without args stays marker-only (skip
        //   INSERT, without a specific default in the migration).
        let default = col_meta.and_then(|c| c.db_default_sql.clone());

        columns.push(Column {
            name: sql_name.clone(),
            sql_type,
            nullable,
            default,
            // v0.10.27 (F2) — only single PK emits `PRIMARY KEY`
            // inline; composite PK emits the table-level constraint
            // below.
            is_primary: is_inline_pk,
            renamed_from: col_meta.and_then(|c| c.renamed_from.clone()),
        });

        // Indexes per-field.
        if let Some(cm) = col_meta {
            if cm.unique {
                indexes.push(Index {
                    name: format!("{}_{}_key", table_name, sql_name),
                    columns: vec![sql_name.clone()],
                    unique: true,
                    where_clause: None,
                    using: None,
                    expression: None,
                });
            }
            if cm.indexed {
                indexes.push(Index {
                    name: format!("{}_{}_idx", table_name, sql_name),
                    columns: vec![sql_name.clone()],
                    unique: false,
                    where_clause: None,
                    using: None,
                    expression: None,
                });
            }
        }

        // FK from @belongs_to.
        if let Some(rel) = meta.relations.get(&f.name) {
            if rel.kind == crate::types::RelationKind::BelongsTo {
                // Resolve target table name via type_env.
                let target_meta = type_env
                    .lookup(&rel.target_type)
                    .and_then(|tid| type_env.table_metadata(tid))
                    .ok_or_else(|| {
                        format!(
                            "@belongs_to on `{}.{}` points to `{}` which is not @table",
                            type_name, f.name, rel.target_type
                        )
                    })?;
                let target_table = target_meta.sql_name.clone();
                // v0.10.29 — Cross-schema FK. If the target type
                // declares `@table("schema.name")` different from
                // the current type's schema, the FK emit uses
                // `REFERENCES "schema"."name"(col)` qualified.
                // Same-schema → references_schema = None (matches
                // implicit `public` Postgres convention).
                let target_schema = target_meta.schema.clone();
                let same_schema = target_schema.as_deref().unwrap_or("public")
                    == meta.schema.as_deref().unwrap_or("public");
                let references_schema = if same_schema { None } else { target_schema };
                // PK column of the target. In PG, PK columns do
                // not have a table prefix.
                //
                // v0.10.31 (Tier A.6) — clear pre-DDL error if the
                // target has composite PK. Postgres rejects FK
                // single-column referencing composite PK (it is not
                // UNIQUE on its own). Before v0.10.31 there was a
                // silent fallback to `"id"`, which typically did
                // not exist and gave a cryptic Postgres error in
                // `fitz db migrate`. Now the error aborts
                // `schema_from_program` with a specific message
                // citing the composite PK fields and suggesting
                // workarounds.
                let target_pk_field = match target_meta.single_pk() {
                    Some(pk) => pk.to_string(),
                    None => {
                        let pk_fields: Vec<&str> = target_meta
                            .primary_fields
                            .iter()
                            .map(|s| s.as_str())
                            .collect();
                        return Err(format!(
                            "@belongs_to en `{}.{}` apunta a `{}` que tiene composite PK \
                             ({}). FK single-column no puede referenciar composite PK — \
                             Postgres exige que la columna target sea UNIQUE por sí sola. \
                             Workarounds: (a) declarar un UNIQUE constraint en una sola \
                             columna del target y referenciarla via `@belongs_to(refs=\"<col>\")` \
                             (deuda futura — sub-paso refs=); (b) usar `@table` sin composite \
                             PK con auto-increment id como surrogate key.",
                            type_name,
                            f.name,
                            rel.target_type,
                            pk_fields.join(", ")
                        ));
                    }
                };
                let target_pk_sql = target_meta
                    .columns
                    .get(&target_pk_field)
                    .and_then(|c| c.sql_name.clone())
                    .unwrap_or_else(|| target_pk_field.clone());
                // Convention: constraint name uses the schema
                // "<table>_<col>_fkey" that PG uses by default
                // when inline in CREATE TABLE.
                let constraint_name = format!("{}_{}_fkey", table_name, sql_name);
                let on_delete = match rel.on_delete {
                    crate::types::CascadeAction::Cascade => Some("CASCADE".to_string()),
                    crate::types::CascadeAction::SetNull => Some("SET NULL".to_string()),
                    crate::types::CascadeAction::Restrict => Some("RESTRICT".to_string()),
                    crate::types::CascadeAction::NoAction => None,
                };
                foreign_keys.push(ForeignKey {
                    name: constraint_name,
                    column: sql_name.clone(),
                    references_table: target_table,
                    references_column: target_pk_sql,
                    references_schema,
                    on_delete,
                });
            }
        }
    }

    // v0.10.27 (F3) — Indexes from `@index(...)` decorators at the
    // type level (composite + partial + unique). We auto-generate
    // the name if the user did not pass one:
    // `idx_<table>_<col1>_<col2>...` with `_uniq` suffix if unique.
    // The name must match what Postgres reports via pg_indexes so
    // the drift check does not trigger CREATE/DROP on every run.
    for idx in &meta.indexes {
        // v0.10.32 (Tier C.2) — auto-naming for expression indexes:
        // `idx_<table>_expr_<N>` with N based on position. The user
        // is strongly recommended to name explicitly with `name=`
        // for drift detection.
        let auto_name = || {
            let mut s = String::from("idx_");
            s.push_str(&table_name);
            if idx.expression.is_some() {
                s.push_str("_expr");
            } else {
                for c in &idx.columns {
                    s.push('_');
                    s.push_str(c);
                }
            }
            if idx.unique {
                s.push_str("_uniq");
            }
            s
        };
        let index_name = idx.name.clone().unwrap_or_else(auto_name);
        indexes.push(Index {
            name: index_name,
            columns: idx.columns.clone(),
            unique: idx.unique,
            where_clause: idx.where_clause.clone(),
            using: idx.using.clone(),
            expression: idx.expression.clone(),
        });
    }

    // Deterministic order of indexes and FKs (for stable diffs).
    indexes.sort_by(|a, b| a.name.cmp(&b.name));
    foreign_keys.sort_by(|a, b| a.name.cmp(&b.name));

    // v0.10.27 (F2) — composite PK: resolve SQL names of the PK
    // fields (respects @column(name=...)). We only populate if
    // N>=2; single PK stays as `Column.is_primary` inline.
    let composite_pk: Vec<String> = if meta.primary_fields.len() >= 2 {
        meta.primary_fields
            .iter()
            .map(|pk_fitz| {
                meta.columns
                    .get(pk_fitz)
                    .and_then(|c| c.sql_name.clone())
                    .unwrap_or_else(|| pk_fitz.clone())
            })
            .collect()
    } else {
        Vec::new()
    };

    // v0.10.29 — CHECK constraints declared with
    // `@check_constraint("expr", name="optional")` at the type
    // level. Auto-naming: `chk_<table>_<idx>` when the user does
    // not specify `name=`. Deterministic by order of appearance in
    // the AST.
    let check_constraints: Vec<CheckConstraint> = meta
        .check_constraints
        .iter()
        .enumerate()
        .map(|(i, spec)| CheckConstraint {
            name: spec
                .name
                .clone()
                .unwrap_or_else(|| format!("chk_{}_{}", table_name, i)),
            expr: spec.expr.clone(),
        })
        .collect();

    Ok(Table {
        name: table_name,
        columns,
        indexes,
        foreign_keys,
        renamed_from: meta.renamed_from.clone(),
        schema: meta.schema.clone(),
        composite_pk,
        check_constraints,
    })
}

/// If the TypeExpr is `T?` (Nullable), returns (T, true).
/// Otherwise, (T, false).
fn unwrap_nullable_typeexpr(t: &crate::ast::TypeExpr) -> (crate::ast::TypeExpr, bool) {
    match t {
        crate::ast::TypeExpr::Nullable(inner) => ((**inner).clone(), true),
        other => (other.clone(), false),
    }
}

/// Converts a Fitz TypeExpr to its Postgres SQL type. Applies
/// override of `col_meta.sql_type` if declared (escape hatch for
/// non-standard types like `uuid`/`numeric(10,2)`/etc.).
fn fitz_typeexpr_to_sql_type(
    t: &crate::ast::TypeExpr,
    is_primary: bool,
    col_meta: Option<&crate::types::ColumnMetadata>,
) -> Result<String, String> {
    // Declared override has priority.
    if let Some(cm) = col_meta {
        if let Some(sql) = &cm.sql_type {
            return Ok(sql.clone());
        }
    }
    let head = t.head_name();
    match head {
        "Int" => {
            // @primary Int → bigserial (Postgres auto-increment).
            // Rest → bigint.
            if is_primary {
                Ok("bigserial".to_string())
            } else {
                Ok("bigint".to_string())
            }
        }
        "Float" => Ok("double precision".to_string()),
        "Str" => Ok("text".to_string()),
        "Bool" => Ok("boolean".to_string()),
        "Bytes" => Ok("bytea".to_string()),
        // v0.10.24 — native temporal and UUID types. Canonical
        // Postgres mappings: Date → date (4 bytes), DateTime →
        // timestamptz (8 bytes UTC with offset), Uuid → uuid (16
        // bytes). The ORM marshaling (driver wire + JSON) handles
        // the text ↔ binary conversion.
        "Date" => Ok("date".to_string()),
        "DateTime" => Ok("timestamptz".to_string()),
        "Uuid" => Ok("uuid".to_string()),
        "List" => {
            // List<T> → T[] Postgres array.
            let inner = match t {
                crate::ast::TypeExpr::Generic { name: _, args } if !args.is_empty() => {
                    args[0].clone()
                }
                _ => {
                    return Err(format!(
                        "List without type parameter: `{}` (expected `List<Int>`/`List<Str>`/etc.)",
                        t.display_name(),
                    ));
                }
            };
            let (inner_unwrapped, _inner_nullable) = unwrap_nullable_typeexpr(&inner);
            let inner_sql = fitz_typeexpr_to_sql_type(&inner_unwrapped, false, None)?;
            Ok(format!("{}[]", inner_sql))
        }
        "Map" => {
            // Map<Str, _> → jsonb. Other key types not supported.
            Ok("jsonb".to_string())
        }
        _ => Err(format!(
            "Fitz type `{}` has no automatic SQL mapping \
             (use @column(sql_type=\"...\") to force one)",
            t.display_name()
        )),
    }
}

// =================================================================
// Diff algorithm: current vs target → Vec<Change>
// =================================================================

/// A DDL operation to bring schema `current` to `target`. The
/// diff emits them in safe order for sequential execution:
/// CREATE TABLE → ADD/DROP/ALTER COLUMN → CREATE/DROP INDEX →
/// DROP FK → ADD FK → DROP TABLE.
/// v0.10.21 (10.6.e.3) — Reference to a table with optional
/// schema. `schema = None` means `public` (Postgres default). The
/// `quote_qualified` emits `"schema"."name"` or `"name"`
/// accordingly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

impl TableRef {
    /// Constructor for tables in `public` (compat v0.10.0-v0.10.20).
    pub fn public(name: impl Into<String>) -> Self {
        Self {
            schema: None,
            name: name.into(),
        }
    }

    /// Constructor for tables in a custom schema.
    pub fn qualified(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: Some(schema.into()),
            name: name.into(),
        }
    }

    /// Builds TableRef from a `Table` (read schema field).
    pub fn from_table(t: &Table) -> Self {
        Self {
            schema: t.schema.clone(),
            name: t.name.clone(),
        }
    }
}

/// v0.10.31 (Tier A.1) — impact classification of a `Change` for
/// the `fitz db diff --check-destructive` flag. The policy is
/// opinionated but conservative:
///
/// - **Safe**: does not touch existing data and cannot fail by
///   shape. E.g.: `CREATE TABLE`, `ADD COLUMN nullable`, `DROP
///   FOREIGN KEY`.
/// - **Risky**: may fail at runtime (NOT NULL over existing rows,
///   cast with precision loss) but does NOT destroy data if it
///   succeeds. The emitted SQL is still valid — the user reviews.
/// - **Destructive**: guaranteed data loss if applied. E.g.:
///   `DROP TABLE`, `DROP COLUMN`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Safe,
    Risky,
    Destructive,
}

impl Severity {
    /// Short label for comments in the SQL output of the diff with
    /// `--check-destructive`. `[SAFE]`/`[RISKY]`/`[DESTRUCTIVE]`.
    pub fn label(&self) -> &'static str {
        match self {
            Severity::Safe => "SAFE",
            Severity::Risky => "RISKY",
            Severity::Destructive => "DESTRUCTIVE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    CreateTable(Table),
    DropTable(TableRef),
    AddColumn {
        table: TableRef,
        column: Column,
    },
    DropColumn {
        table: TableRef,
        column: String,
    },
    AlterColumnType {
        table: TableRef,
        column: String,
        new_type: String,
    },
    AlterColumnNullable {
        table: TableRef,
        column: String,
        nullable: bool,
    },
    /// v0.10.16 — Change to the DEFAULT of an existing column.
    /// `new_default = Some(sql)` → `SET DEFAULT <sql>`; `None` →
    /// `DROP DEFAULT`. Normalization for the diff is
    /// case-insensitive over SQL function calls (`now()` matches
    /// `NOW()` and `Now()`).
    AlterColumnDefault {
        table: TableRef,
        column: String,
        new_default: Option<String>,
    },
    CreateIndex {
        table: TableRef,
        index: Index,
    },
    DropIndex {
        /// Index schema (= schema of the table it belongs to).
        /// `None` = public. Postgres `DROP INDEX` quotes with
        /// schema if non-public.
        schema: Option<String>,
        index_name: String,
    },
    AddForeignKey {
        table: TableRef,
        fk: ForeignKey,
    },
    DropForeignKey {
        table: TableRef,
        fk_name: String,
    },
    /// v0.10.17 (10.6.b.2) — Rename of table preserving data.
    /// Emitted when the target Table has `renamed_from = Some(old)`
    /// and exists in current with that name. Goes FIRST in the
    /// output (before any column ALTER) so that following actions
    /// operate on the new name. The rename occurs within the same
    /// schema (cross-schema rename not supported in MVP).
    RenameTable {
        schema: Option<String>,
        old_name: String,
        new_name: String,
    },
    /// v0.10.21 (10.6.e.3) — `CREATE SCHEMA IF NOT EXISTS "name"`.
    /// Emitted when the target references a custom schema that does
    /// NOT exist in current. Goes FIRST in the output (before
    /// CREATE TABLE in that schema), idempotent via `IF NOT EXISTS`.
    CreateSchema {
        name: String,
    },
    /// v0.10.17 (10.6.b.2) — Rename of column preserving data.
    /// Emitted when a target Column has `renamed_from = Some(old)`
    /// and exists in current.columns with that name. Goes
    /// immediately after RenameTable and before ADD/DROP COLUMN.
    RenameColumn {
        table: TableRef,
        old_name: String,
        new_name: String,
    },
    /// v0.10.31 (Tier A.5) — `ALTER TABLE ... ADD CONSTRAINT <name>
    /// CHECK (<expr>)`. Emitted when the target type declares a
    /// `@check_constraint(...)` that does not exist in the current
    /// schema. Requires the table to already exist (not used in
    /// CREATE TABLE — CREATE CHECKs go inline in `create_table_sql`).
    AddCheckConstraint {
        table: TableRef,
        name: String,
        expr: String,
    },
    /// v0.10.31 (Tier A.5) — `ALTER TABLE ... DROP CONSTRAINT <name>`.
    /// Emitted when the current schema has a CHECK that is no
    /// longer in the target. **NOTE**: current introspect (v0.10.31)
    /// does not populate `current.check_constraints` until A.7
    /// closes (pg_constraint.contype='c'). Until then, DROP CHECK
    /// is not triggered (current always empty), but the variant +
    /// emit are ready for when A.7 closes.
    DropCheckConstraint {
        table: TableRef,
        name: String,
    },
}

impl Change {
    /// v0.10.31 (Tier A.1) — classifies the `Change` by impact
    /// level.
    ///
    /// See `Severity` for the policy. Summary:
    /// - DropTable / DropColumn → **Destructive** (data loss).
    /// - AddColumn NOT NULL without default, AlterColumnType,
    ///   AlterColumnNullable false, AlterColumnDefault, DropIndex
    ///   → **Risky** (may fail at runtime or impact performance).
    /// - Everything else → **Safe**.
    pub fn severity(&self) -> Severity {
        match self {
            // Guaranteed data loss.
            Change::DropTable(_) => Severity::Destructive,
            Change::DropColumn { .. } => Severity::Destructive,

            // May fail or impact performance, but does not destroy data.
            Change::AddColumn { column, .. } => {
                if !column.nullable && column.default.is_none() {
                    // NOT NULL without default → fails if the table has rows.
                    Severity::Risky
                } else {
                    Severity::Safe
                }
            }
            Change::AlterColumnType { .. } => Severity::Risky,
            Change::AlterColumnNullable { nullable, .. } => {
                if !nullable {
                    // SET NOT NULL → fails if there are rows with NULL.
                    Severity::Risky
                } else {
                    // DROP NOT NULL → always safe.
                    Severity::Safe
                }
            }
            Change::AlterColumnDefault { .. } => Severity::Risky,
            Change::DropIndex { .. } => Severity::Risky,
            // v0.10.31 (Tier A.5) — ADD CHECK may fail if existing
            // rows violate the predicate. DROP CHECK is safe (only
            // removes the rule, does not touch data).
            Change::AddCheckConstraint { .. } => Severity::Risky,

            // Safe: do not touch data, do not fail by shape.
            Change::CreateSchema { .. } => Severity::Safe,
            Change::CreateTable(_) => Severity::Safe,
            Change::CreateIndex { .. } => Severity::Safe,
            Change::AddForeignKey { .. } => Severity::Safe,
            Change::DropForeignKey { .. } => Severity::Safe,
            Change::RenameTable { .. } => Severity::Safe,
            Change::RenameColumn { .. } => Severity::Safe,
            Change::DropCheckConstraint { .. } => Severity::Safe,
        }
    }
}

/// v0.10.31 (Tier A.1) — emits the SQL of the diff with severity
/// comments for each change. Enriched version of `changes_to_sql`
/// for the `--check-destructive` mode.
///
/// Each change is emitted as:
///
/// ```sql
/// -- [SAFE] short_label
/// SQL;
/// ```
///
/// The `short_label` is informative (type + table/collaborable).
pub fn changes_to_sql_with_severity(changes: &[Change]) -> String {
    let mut out = String::new();
    for c in changes {
        let sev = c.severity();
        out.push_str(&format!("-- [{}] {}\n", sev.label(), change_short_label(c)));
        out.push_str(&change_to_sql(c));
        out.push('\n');
    }
    out
}

/// Helper for `changes_to_sql_with_severity` — short
/// human-readable label of a change ("AddColumn email to users").
fn change_short_label(c: &Change) -> String {
    match c {
        Change::CreateTable(t) => format!("CreateTable {}", t.name),
        Change::DropTable(tr) => format!("DropTable {}", tr.name),
        Change::AddColumn { table, column } => {
            format!("AddColumn {} to {}", column.name, table.name)
        }
        Change::DropColumn { table, column } => {
            format!("DropColumn {} from {}", column, table.name)
        }
        Change::AlterColumnType {
            table,
            column,
            new_type,
        } => format!("AlterColumnType {}.{} -> {}", table.name, column, new_type),
        Change::AlterColumnNullable {
            table,
            column,
            nullable,
        } => format!(
            "AlterColumnNullable {}.{} -> {}",
            table.name,
            column,
            if *nullable { "NULL" } else { "NOT NULL" }
        ),
        Change::AlterColumnDefault { table, column, .. } => {
            format!("AlterColumnDefault {}.{}", table.name, column)
        }
        Change::CreateIndex { table, index } => {
            format!("CreateIndex {} on {}", index.name, table.name)
        }
        Change::DropIndex { index_name, .. } => format!("DropIndex {}", index_name),
        Change::AddForeignKey { table, fk } => {
            format!("AddForeignKey {} on {}", fk.name, table.name)
        }
        Change::DropForeignKey { table, fk_name } => {
            format!("DropForeignKey {} on {}", fk_name, table.name)
        }
        Change::RenameTable {
            old_name, new_name, ..
        } => format!("RenameTable {} -> {}", old_name, new_name),
        Change::RenameColumn {
            table,
            old_name,
            new_name,
        } => format!("RenameColumn {}.{} -> {}", table.name, old_name, new_name),
        Change::CreateSchema { name } => format!("CreateSchema {}", name),
        Change::AddCheckConstraint { table, name, .. } => {
            format!("AddCheckConstraint {} on {}", name, table.name)
        }
        Change::DropCheckConstraint { table, name } => {
            format!("DropCheckConstraint {} on {}", name, table.name)
        }
    }
}

/// v0.10.31 (Tier A.1) — counts changes by severity. Useful for
/// the summary of the `--check-destructive` mode.
pub fn count_by_severity(changes: &[Change]) -> (usize, usize, usize) {
    let mut safe = 0;
    let mut risky = 0;
    let mut destructive = 0;
    for c in changes {
        match c.severity() {
            Severity::Safe => safe += 1,
            Severity::Risky => risky += 1,
            Severity::Destructive => destructive += 1,
        }
    }
    (safe, risky, destructive)
}

/// Compares `current` (snapshot via [`introspect_schema`]) with
/// `target` (snapshot via [`schema_from_program`]) and emits the
/// ordered list of [`Change`] needed to synchronize.
///
/// Guarantees:
/// - Idempotent: `diff(target, target) == []` (no changes).
/// - Deterministic: the output is stable between runs
///   (categories ordered + items within each category sorted
///   alphabetically).
/// - Safe for sequential execution: the order allows applying
///   the Changes without intermediate errors (CREATE TABLE
///   before ALTER, DROP FK before DROP TABLE/COLUMN).
///
/// MVP limitations:
/// - **Renames detected only via `@renamed_from(...)`** (v0.10.17).
///   Without the decorator, a rename from `name` → `full_name` is
///   seen as `DROP COLUMN name` + `ADD COLUMN full_name`, losing
///   the data. The user explicitly marks the rename with
///   `@renamed_from("old")` on the field/type so that the diff
///   emits `RENAME COLUMN`/`RENAME TABLE` preserving data.
/// - **Direct `AlterColumnType` without USING** — if the data
///   does NOT convert to the new type, Postgres fails. The user
///   must edit the migration to add `USING (col::new_type)` or a
///   data migration script.
pub fn diff_schemas(current: &Schema, target: &Schema) -> Vec<Change> {
    let mut changes = Vec::new();

    // --- 0. RENAMES (v0.10.17, 10.6.b.2). Pre-processes the
    // `renamed_from` hints of the target: emits RenameTable +
    // RenameColumn FIRST, and builds a `current_renamed` with the
    // names already updated so that the rest of the diff
    // (CREATE/DROP/ALTER) compares against the post-rename state —
    // without this, a renamed table would look like DROP+CREATE
    // losing data.
    let current = apply_renames_from_target(current, target, &mut changes);
    let current = &current;

    // v0.10.21 — Table identity = (schema, name). Tables of the
    // current are compared by qualified_id against those of the
    // target, not by flat name.
    let current_ids: std::collections::HashSet<(String, String)> =
        current.tables.iter().map(|t| t.qualified_id()).collect();
    let target_ids: std::collections::HashSet<(String, String)> =
        target.tables.iter().map(|t| t.qualified_id()).collect();

    // --- 0.5. CREATE SCHEMA IF NOT EXISTS for custom schemas of
    // the target that do not exist in current. Goes FIRST (before
    // CREATE TABLE in that schema). Idempotent.
    let current_schemas: std::collections::HashSet<String> = current
        .tables
        .iter()
        .filter_map(|t| t.schema.clone())
        .collect();
    let mut new_schemas: Vec<String> = target
        .tables
        .iter()
        .filter_map(|t| t.schema.clone())
        .filter(|s| !current_schemas.contains(s))
        .collect();
    new_schemas.sort();
    new_schemas.dedup();
    for s in &new_schemas {
        changes.push(Change::CreateSchema { name: s.clone() });
    }

    // --- 1. CREATE TABLE (target tables not present in current).
    let mut create_tables: Vec<&Table> = target
        .tables
        .iter()
        .filter(|t| !current_ids.contains(&t.qualified_id()))
        .collect();
    create_tables.sort_by_key(|a| a.qualified_id());
    for t in &create_tables {
        changes.push(Change::CreateTable((*t).clone()));
        // v0.10.27 (F3) — indexes defined by `@index(...)` for new
        // tables also go as separate CREATE INDEX (not inline in the
        // CREATE TABLE to stay symmetric with the diff path of
        // existing tables). Auto-PK and auto-UNIQUE of individual
        // columns do NOT go here — they are part of the CREATE
        // TABLE inline. We only emit the "extra" indexes from the
        // @index decorator or from @column.indexed/unique.
        let mut idx_to_create: Vec<&Index> = t.indexes.iter().collect();
        idx_to_create.sort_by(|a, b| a.name.cmp(&b.name));
        let table_ref = TableRef::from_table(t);
        for i in idx_to_create {
            changes.push(Change::CreateIndex {
                table: table_ref.clone(),
                index: i.clone(),
            });
        }
    }

    // --- 2. ALTER of tables in BOTH (cross-schema match by
    // qualified_id).
    let mut tables_to_alter: Vec<(&Table, &Table)> = current
        .tables
        .iter()
        .filter_map(|c| {
            target
                .tables
                .iter()
                .find(|t| t.qualified_id() == c.qualified_id())
                .map(|t| (c, t))
        })
        .collect();
    tables_to_alter.sort_by_key(|a| a.0.qualified_id());

    // 2.1. Per-table: columns + indexes + checks.
    for (current_t, target_t) in &tables_to_alter {
        diff_columns(current_t, target_t, &mut changes);
        diff_indexes(current_t, target_t, &mut changes);
        // v0.10.31 (Tier A.5) — ADD/DROP CHECK constraints on
        // existing table. The `diff_check_constraints` function
        // only triggers DropCheckConstraint when A.7 is closed
        // (current.check_constraints populated from introspect).
        diff_check_constraints(current_t, target_t, &mut changes);
    }

    // --- 3. DROP FK (before DROP TABLE / DROP COLUMN).
    for (current_t, target_t) in &tables_to_alter {
        let target_fk_names: std::collections::HashSet<&str> = target_t
            .foreign_keys
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        let mut to_drop: Vec<&ForeignKey> = current_t
            .foreign_keys
            .iter()
            .filter(|f| !target_fk_names.contains(f.name.as_str()))
            .collect();
        to_drop.sort_by(|a, b| a.name.cmp(&b.name));
        for fk in to_drop {
            changes.push(Change::DropForeignKey {
                table: TableRef::from_table(current_t),
                fk_name: fk.name.clone(),
            });
        }
    }

    // --- 4. ADD FK (after having all tables and cols).
    for t in &create_tables {
        let mut fks: Vec<&ForeignKey> = t.foreign_keys.iter().collect();
        fks.sort_by(|a, b| a.name.cmp(&b.name));
        for fk in fks {
            changes.push(Change::AddForeignKey {
                table: TableRef::from_table(t),
                fk: fk.clone(),
            });
        }
    }
    for (current_t, target_t) in &tables_to_alter {
        let current_fk_names: std::collections::HashSet<&str> = current_t
            .foreign_keys
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        let mut to_add: Vec<&ForeignKey> = target_t
            .foreign_keys
            .iter()
            .filter(|f| !current_fk_names.contains(f.name.as_str()))
            .collect();
        to_add.sort_by(|a, b| a.name.cmp(&b.name));
        for fk in to_add {
            changes.push(Change::AddForeignKey {
                table: TableRef::from_table(target_t),
                fk: fk.clone(),
            });
        }
    }

    // --- 5. DROP TABLE (current tables not in target).
    let mut drop_tables: Vec<&Table> = current
        .tables
        .iter()
        .filter(|t| !target_ids.contains(&t.qualified_id()))
        .collect();
    drop_tables.sort_by_key(|a| a.qualified_id());
    for t in drop_tables {
        changes.push(Change::DropTable(TableRef::from_table(t)));
    }

    changes
}

/// v0.10.17 (10.6.b.2) — Detects the `renamed_from` hints of the
/// `target` schema and:
/// 1. Emits the `Change::RenameTable` / `Change::RenameColumn`
///    at the front of `changes` (order: tables first, then
///    columns).
/// 2. Returns a renamed version of `current` so that the rest of
///    the diff compares by post-rename names.
///
/// **Policy**:
/// - Rename active only if: target has `renamed_from = Some(old)`
///   AND current.tables contains a table with that `old` name AND
///   current.tables does NOT contain a table with the target name
///   (avoids accidental collision).
/// - Column renames inside a renamed table use the target
///   (post-rename) name in `RenameColumn.table`.
/// - Hints without match in `current` (e.g.: user left the
///   decorator after applying the migration) are silently
///   ignored — they are no-op, not an error. The user can clean
///   them up whenever.
fn apply_renames_from_target(
    current: &Schema,
    target: &Schema,
    changes: &mut Vec<Change>,
) -> Schema {
    // v0.10.21 — Rename is now schema-aware. Table identity is
    // `(schema, name)`. Cross-schema renames are NOT supported in
    // MVP — the `renamed_from` is interpreted within the current
    // schema of the target table.
    let mut renamed_tables: std::collections::HashMap<(Option<String>, String), String> =
        std::collections::HashMap::new();
    let current_qual: std::collections::HashSet<(Option<String>, String)> = current
        .tables
        .iter()
        .map(|t| (t.schema.clone(), t.name.clone()))
        .collect();
    let mut table_renames: Vec<(Option<String>, String, String)> = Vec::new();
    for t in &target.tables {
        if let Some(old) = &t.renamed_from {
            let old_key = (t.schema.clone(), old.clone());
            let new_key = (t.schema.clone(), t.name.clone());
            if old != &t.name && current_qual.contains(&old_key) && !current_qual.contains(&new_key)
            {
                table_renames.push((t.schema.clone(), old.clone(), t.name.clone()));
            }
        }
    }
    table_renames.sort_by_key(|a| (a.0.clone(), a.1.clone()));
    for (schema, old, new) in &table_renames {
        changes.push(Change::RenameTable {
            schema: schema.clone(),
            old_name: old.clone(),
            new_name: new.clone(),
        });
        renamed_tables.insert((schema.clone(), old.clone()), new.clone());
    }

    // 2. Build renamed current (at table level).
    let mut new_tables: Vec<Table> = current
        .tables
        .iter()
        .map(|t| {
            let mut t = t.clone();
            let key = (t.schema.clone(), t.name.clone());
            if let Some(new_name) = renamed_tables.get(&key) {
                t.name = new_name.clone();
            }
            t
        })
        .collect();

    // 3. Detect column renames within tables that exist in both
    // schemas (post-rename, match by qualified_id).
    for target_t in &target.tables {
        let Some(current_t) = new_tables
            .iter_mut()
            .find(|t| t.qualified_id() == target_t.qualified_id())
        else {
            continue;
        };
        let current_col_names: std::collections::HashSet<String> =
            current_t.columns.iter().map(|c| c.name.clone()).collect();
        let mut col_renames: Vec<(String, String)> = Vec::new();
        for c in &target_t.columns {
            if let Some(old) = &c.renamed_from {
                if old != &c.name
                    && current_col_names.contains(old.as_str())
                    && !current_col_names.contains(c.name.as_str())
                {
                    col_renames.push((old.clone(), c.name.clone()));
                }
            }
        }
        col_renames.sort_by(|a, b| a.0.cmp(&b.0));
        for (old, new) in &col_renames {
            changes.push(Change::RenameColumn {
                table: TableRef::from_table(target_t),
                old_name: old.clone(),
                new_name: new.clone(),
            });
            // Rename in-place in current_t so that the rest of the
            // diff sees the new name.
            if let Some(col) = current_t.columns.iter_mut().find(|c| &c.name == old) {
                col.name = new.clone();
            }
        }
    }

    Schema { tables: new_tables }
}

fn diff_columns(current: &Table, target: &Table, changes: &mut Vec<Change>) {
    let current_col_names: std::collections::HashSet<&str> =
        current.columns.iter().map(|c| c.name.as_str()).collect();
    let target_col_names: std::collections::HashSet<&str> =
        target.columns.iter().map(|c| c.name.as_str()).collect();
    let table_ref = TableRef::from_table(target);

    // Drop columns not in target.
    let mut to_drop: Vec<&Column> = current
        .columns
        .iter()
        .filter(|c| !target_col_names.contains(c.name.as_str()))
        .collect();
    to_drop.sort_by(|a, b| a.name.cmp(&b.name));
    for c in to_drop {
        changes.push(Change::DropColumn {
            table: table_ref.clone(),
            column: c.name.clone(),
        });
    }

    // Add columns not in current.
    let mut to_add: Vec<&Column> = target
        .columns
        .iter()
        .filter(|c| !current_col_names.contains(c.name.as_str()))
        .collect();
    to_add.sort_by(|a, b| a.name.cmp(&b.name));
    for c in to_add {
        changes.push(Change::AddColumn {
            table: table_ref.clone(),
            column: c.clone(),
        });
    }

    // Alter columns in BOTH with differences.
    for tc in &target.columns {
        if let Some(cc) = current.columns.iter().find(|c| c.name == tc.name) {
            // Type change. But we ignore `bigserial` vs `bigint`
            // as the same type — PG reports `bigint` for columns
            // of `bigserial` type (the auto-increment is reflected
            // in `column_default = nextval(...)`).
            let current_type = normalize_sql_type_for_diff(&cc.sql_type);
            let target_type = normalize_sql_type_for_diff(&tc.sql_type);
            if current_type != target_type {
                changes.push(Change::AlterColumnType {
                    table: table_ref.clone(),
                    column: tc.name.clone(),
                    new_type: tc.sql_type.clone(),
                });
            }
            if cc.nullable != tc.nullable {
                changes.push(Change::AlterColumnNullable {
                    table: table_ref.clone(),
                    column: tc.name.clone(),
                    nullable: tc.nullable,
                });
            }
            let current_norm = cc.default.as_deref().map(normalize_default_for_diff);
            let target_norm = tc.default.as_deref().map(normalize_default_for_diff);
            if current_norm != target_norm {
                changes.push(Change::AlterColumnDefault {
                    table: table_ref.clone(),
                    column: tc.name.clone(),
                    new_default: tc.default.clone(),
                });
            }
        }
    }
}

/// v0.10.16 — Normalizes a SQL default expression for comparison
/// in the diff. The goal is to be permissive with cosmetic
/// variations that do not change semantics:
///
/// - Lowercase of the full expression (PG normalizes `NOW()` →
///   `now()` in `column_default`).
/// - Strip of redundant casts that PG adds automatically
///   (`'public'::text` → `'public'`, `42::bigint` → `42`).
/// - Trim whitespace.
///
/// Does NOT try to evaluate equivalent expressions (`now()` vs
/// `CURRENT_TIMESTAMP` are both valid for `timestamptz` but look
/// different — we treat them as different).
fn normalize_default_for_diff(s: &str) -> String {
    let lower = s.trim().to_lowercase();
    // Strip `::type` casts when the type is a simple alphanumeric.
    // Conservative: only strip at the end, not in the middle (we
    // do not break complex expressions like `(a::int) + (b::int)`).
    let no_cast = strip_trailing_pg_cast(&lower);
    no_cast.to_string()
}

fn strip_trailing_pg_cast(s: &str) -> &str {
    // Look for the last `::` and if what follows is a simple
    // alphanumeric (`text`, `bigint`, `timestamptz`, etc.), trim
    // it.
    if let Some(idx) = s.rfind("::") {
        let tail = &s[idx + 2..];
        if !tail.is_empty()
            && tail
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ' ')
        {
            return s[..idx].trim_end();
        }
    }
    s
}

fn diff_indexes(current: &Table, target: &Table, changes: &mut Vec<Change>) {
    let current_by_name: std::collections::HashMap<&str, &Index> = current
        .indexes
        .iter()
        .map(|i| (i.name.as_str(), i))
        .collect();
    let target_by_name: std::collections::HashMap<&str, &Index> = target
        .indexes
        .iter()
        .map(|i| (i.name.as_str(), i))
        .collect();
    let table_ref = TableRef::from_table(target);

    // Drop indexes not in target.
    let mut to_drop: Vec<&Index> = current
        .indexes
        .iter()
        .filter(|i| !target_by_name.contains_key(i.name.as_str()))
        .collect();
    to_drop.sort_by(|a, b| a.name.cmp(&b.name));
    for i in to_drop {
        changes.push(Change::DropIndex {
            schema: current.schema.clone(),
            index_name: i.name.clone(),
        });
    }

    // Create indexes not in current.
    let mut to_add: Vec<&Index> = target
        .indexes
        .iter()
        .filter(|i| !current_by_name.contains_key(i.name.as_str()))
        .collect();
    to_add.sort_by(|a, b| a.name.cmp(&b.name));
    for i in to_add {
        changes.push(Change::CreateIndex {
            table: table_ref.clone(),
            index: i.clone(),
        });
    }

    // v0.10.29 — Closes the v0.10.27/v0.10.28 debt: for indexes
    // with the SAME name in current and target, compare the shape
    // (columns, unique, where_clause, using). If they differ →
    // DROP + CREATE to regenerate with the new shape. Before, the
    // diff was purely name-based: the user had to change the
    // `name=` to force regeneration when only `using=` or
    // `where_=` changed, which is ergonomically broken.
    //
    // Notes:
    // - `where_clause` is compared via `where_clauses_match`
    //   (defensive reading: Postgres canonicalizes it with extra
    //   parens and whitespace). The comparator matches
    //   case-insensitive + whitespace-collapsed. Tie → preserve
    //   (no spurious regen).
    // - `using` is compared case-insensitive; `None`/`Some("btree")`
    //   are considered equivalent (btree is the Postgres default).
    let mut changed: Vec<(&Index, &Index)> = Vec::new();
    for (name, target_idx) in target_by_name.iter() {
        if let Some(current_idx) = current_by_name.get(name) {
            if !indexes_equivalent_for_diff(current_idx, target_idx) {
                changed.push((*current_idx, *target_idx));
            }
        }
    }
    changed.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    for (current_idx, target_idx) in changed {
        changes.push(Change::DropIndex {
            schema: current.schema.clone(),
            index_name: current_idx.name.clone(),
        });
        changes.push(Change::CreateIndex {
            table: table_ref.clone(),
            index: target_idx.clone(),
        });
    }
}

/// v0.10.29 — Comparator for index diff (same name, compare
/// shape). Best-effort match — Postgres canonicalizes the
/// `where_clause` with extra parens and whitespace on
/// re-introspect, so we compare case-insensitive +
/// whitespace-collapsed to avoid triggering spurious regens.
fn indexes_equivalent_for_diff(a: &Index, b: &Index) -> bool {
    if a.columns != b.columns || a.unique != b.unique {
        return false;
    }
    let a_method = a.using.as_deref().unwrap_or("btree").to_ascii_lowercase();
    let b_method = b.using.as_deref().unwrap_or("btree").to_ascii_lowercase();
    if a_method != b_method {
        return false;
    }
    let a_where = a.where_clause.as_deref().map(canonicalize_where_clause);
    let b_where = b.where_clause.as_deref().map(canonicalize_where_clause);
    a_where == b_where
}

/// Best-effort canonicalization to compare WHERE clauses between
/// the string declared by the user and the one Postgres reports
/// after re-introspect. Normalizes whitespace + case. Does NOT
/// parse SQL — only avoids false positives of the diff from
/// trivial reformatting.
fn canonicalize_where_clause(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// To avoid false positives of the diff: `bigserial` and `bigint`
/// are equivalent in the DB (the auto-increment is separate
/// metadata via default `nextval(...)`). When the AST declares
/// `@primary Int = 0` the target sql_type is `bigserial`, but the
/// DB reports it as `bigint`. Without this normalization, the diff
/// would trigger a spurious `AlterColumnType` on every run.
fn normalize_sql_type_for_diff(sql: &str) -> String {
    match sql.to_ascii_lowercase().as_str() {
        "bigserial" => "bigint".to_string(),
        "serial" => "integer".to_string(),
        "smallserial" => "smallint".to_string(),
        other => other.to_string(),
    }
}

// =================================================================
// SQL emission: Change → DDL Postgres
// =================================================================

/// Converts a list of [`Change`] to Postgres DDL SQL. Each
/// statement ends in `;\n\n` for readability when written to
/// `.sql` files. The output is executable directly via
/// `psql -f` or `db.exec(...)`.
pub fn changes_to_sql(changes: &[Change]) -> String {
    let mut out = String::new();
    for c in changes {
        let stmt = change_to_sql(c);
        out.push_str(&stmt);
        if !stmt.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn change_to_sql(change: &Change) -> String {
    match change {
        Change::CreateTable(t) => create_table_sql(t),
        Change::DropTable(tr) => format!("DROP TABLE {};", quote_qualified(tr)),
        Change::AddColumn { table, column } => {
            let default = column
                .default
                .as_deref()
                .map(|d| format!(" DEFAULT {}", d))
                .unwrap_or_default();
            format!(
                "ALTER TABLE {} ADD COLUMN {} {}{}{};",
                quote_qualified(table),
                quote_ident(&column.name),
                column.sql_type,
                default,
                if column.nullable { "" } else { " NOT NULL" },
            )
        }
        Change::DropColumn { table, column } => {
            format!(
                "ALTER TABLE {} DROP COLUMN {};",
                quote_qualified(table),
                quote_ident(column),
            )
        }
        Change::AlterColumnType {
            table,
            column,
            new_type,
        } => {
            // v0.10.31 (Tier A.2) — always emit
            // `USING <col>::<new_type>`. Postgres accepts the
            // explicit cast even for auto-castable (`int → bigint`),
            // and it is required for non-auto casts (`text → int`,
            // `varchar → int`, casts with different precision). The
            // `col::type(n)` syntax works for parameterized types
            // (`varchar(50)`/`numeric(10,2)`) and for arrays
            // (`int8[]`).
            //
            // For casts Postgres does not support directly (e.g.
            // bytea → text in a specific encoding, `timestamptz` ↔
            // `date` with custom format), the user edits the
            // emitted SQL to use the right function (`USING
            // encode(col, 'utf8')` / `USING col::date AT TIME
            // ZONE 'UTC'`/etc.).
            let col = quote_ident(column);
            format!(
                "ALTER TABLE {} ALTER COLUMN {} TYPE {} USING {}::{};",
                quote_qualified(table),
                col,
                new_type,
                col,
                new_type,
            )
        }
        Change::AlterColumnDefault {
            table,
            column,
            new_default,
        } => match new_default {
            Some(expr) => format!(
                "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {};",
                quote_qualified(table),
                quote_ident(column),
                expr,
            ),
            None => format!(
                "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT;",
                quote_qualified(table),
                quote_ident(column),
            ),
        },
        Change::AlterColumnNullable {
            table,
            column,
            nullable,
        } => {
            if *nullable {
                format!(
                    "ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL;",
                    quote_qualified(table),
                    quote_ident(column),
                )
            } else {
                format!(
                    "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL;",
                    quote_qualified(table),
                    quote_ident(column),
                )
            }
        }
        Change::CreateIndex { table, index } => {
            let unique = if index.unique { "UNIQUE " } else { "" };
            // v0.10.32 (Tier C.2) — expression index. If
            // `expression` is set, the index is created on the
            // literal expression instead of columns
            // (`CREATE INDEX ON tbl (lower(email))`). The user
            // passes the raw expression — Fitz does not parse it.
            let target_expr = match &index.expression {
                Some(expr) => format!("({})", expr),
                None => {
                    let cols: Vec<String> = index.columns.iter().map(|c| quote_ident(c)).collect();
                    format!("({})", cols.join(", "))
                }
            };
            // v0.10.28 — method override (USING gin/gist/brin/etc.).
            // `None` = btree Postgres default, USING is not emitted.
            let using_clause = match &index.using {
                Some(m) => format!(" USING {}", m),
                None => String::new(),
            };
            // v0.10.27 (F3) — partial index: append `WHERE <clause>`.
            // The user passes the RAW SQL clause (without `WHERE`);
            // Postgres validates the predicate on CREATE INDEX. If
            // the clause references non-existent cols, the CREATE
            // fails with a clear message.
            let where_suffix = match &index.where_clause {
                Some(w) => format!(" WHERE {}", w),
                None => String::new(),
            };
            format!(
                "CREATE {}INDEX {} ON {}{} {}{};",
                unique,
                quote_ident(&index.name),
                quote_qualified(table),
                using_clause,
                target_expr,
                where_suffix,
            )
        }
        Change::DropIndex { schema, index_name } => {
            // Postgres DROP INDEX requires the schema if non-public
            // (otherwise it searches in search_path and may not
            // find it).
            let qualified = match schema {
                Some(s) => format!("{}.{}", quote_ident(s), quote_ident(index_name)),
                None => quote_ident(index_name),
            };
            format!("DROP INDEX {};", qualified)
        }
        Change::AddForeignKey { table, fk } => {
            let on_delete = fk
                .on_delete
                .as_deref()
                .map(|action| format!(" ON DELETE {}", action))
                .unwrap_or_default();
            // v0.10.29 — Cross-schema FK. If `references_schema`
            // is set, emit `REFERENCES "schema"."table"(col)` with
            // schema qualifier. None = same-schema (parallel to
            // the implicit `public` Postgres convention).
            let target_ref = TableRef {
                schema: fk.references_schema.clone(),
                name: fk.references_table.clone(),
            };
            format!(
                "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({}){};",
                quote_qualified(table),
                quote_ident(&fk.name),
                quote_ident(&fk.column),
                quote_qualified(&target_ref),
                quote_ident(&fk.references_column),
                on_delete,
            )
        }
        Change::DropForeignKey { table, fk_name } => {
            format!(
                "ALTER TABLE {} DROP CONSTRAINT {};",
                quote_qualified(table),
                quote_ident(fk_name),
            )
        }
        Change::RenameTable {
            schema,
            old_name,
            new_name,
        } => {
            let old = TableRef {
                schema: schema.clone(),
                name: old_name.clone(),
            };
            // The new name goes WITHOUT schema prefix (RENAME TO
            // expects only the name; the schema is preserved).
            format!(
                "ALTER TABLE {} RENAME TO {};",
                quote_qualified(&old),
                quote_ident(new_name),
            )
        }
        Change::RenameColumn {
            table,
            old_name,
            new_name,
        } => {
            format!(
                "ALTER TABLE {} RENAME COLUMN {} TO {};",
                quote_qualified(table),
                quote_ident(old_name),
                quote_ident(new_name),
            )
        }
        Change::CreateSchema { name } => {
            format!("CREATE SCHEMA IF NOT EXISTS {};", quote_ident(name))
        }
        // v0.10.31 (Tier A.5) — ADD/DROP CHECK constraint. The
        // name is quoted as identifier, the expr goes RAW (the
        // user writes valid SQL inside `@check_constraint("...")`).
        Change::AddCheckConstraint { table, name, expr } => {
            format!(
                "ALTER TABLE {} ADD CONSTRAINT {} CHECK ({});",
                quote_qualified(table),
                quote_ident(name),
                expr,
            )
        }
        Change::DropCheckConstraint { table, name } => {
            format!(
                "ALTER TABLE {} DROP CONSTRAINT {};",
                quote_qualified(table),
                quote_ident(name),
            )
        }
    }
}

/// v0.10.31 (Tier A.5) — diff of CHECK constraints between current
/// and target. Emits `Change::AddCheckConstraint` for those of the
/// target not in current, and `Change::DropCheckConstraint` for
/// those of current not in target. Identity is by `name` (matching
/// pg_constraint.conname). If the `expr` changes with the same
/// name, DROP + ADD is emitted (Postgres does not support ALTER of
/// the expression, requires explicit drop + add).
///
/// **NOTE pre-A.7**: until A.7 closes, `current.check_constraints`
/// is always empty (introspect does not read
/// `pg_constraint.contype='c'`). That means in this version
/// `DropCheckConstraint` is NOT triggered and a change of `expr`
/// with the same name looks like only ADD (the old rule stays in
/// the DB). Documented workaround: the user does a manual DROP
/// before re-running. A.7 closes this.
fn diff_check_constraints(current: &Table, target: &Table, changes: &mut Vec<Change>) {
    let current_names: std::collections::HashSet<&str> = current
        .check_constraints
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let target_names: std::collections::HashSet<&str> = target
        .check_constraints
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let table_ref = TableRef::from_table(target);

    // Drops first (same order as diff_columns / diff_indexes).
    let mut to_drop: Vec<&CheckConstraint> = current
        .check_constraints
        .iter()
        .filter(|c| !target_names.contains(c.name.as_str()))
        .collect();
    to_drop.sort_by(|a, b| a.name.cmp(&b.name));
    for c in to_drop {
        changes.push(Change::DropCheckConstraint {
            table: table_ref.clone(),
            name: c.name.clone(),
        });
    }

    // Adds.
    let mut to_add: Vec<&CheckConstraint> = target
        .check_constraints
        .iter()
        .filter(|c| !current_names.contains(c.name.as_str()))
        .collect();
    to_add.sort_by(|a, b| a.name.cmp(&b.name));
    for c in to_add {
        changes.push(Change::AddCheckConstraint {
            table: table_ref.clone(),
            name: c.name.clone(),
            expr: c.expr.clone(),
        });
    }

    // Mismo name, expr distinto → DROP + ADD. Hasta que A.7 popule
    // current.check_constraints, esto solo aplica para users que
    // mantienen current_schemas custom (uso interno de la lib).
    for tc in &target.check_constraints {
        if let Some(cc) = current.check_constraints.iter().find(|c| c.name == tc.name) {
            if cc.expr.trim() != tc.expr.trim() {
                changes.push(Change::DropCheckConstraint {
                    table: table_ref.clone(),
                    name: tc.name.clone(),
                });
                changes.push(Change::AddCheckConstraint {
                    table: table_ref.clone(),
                    name: tc.name.clone(),
                    expr: tc.expr.clone(),
                });
            }
        }
    }
}

/// v0.10.32 (Tier D.2) — public wrapper for use from the LSP
/// (`fitz::lsp::try_table_create_sql`). Returns the same SQL that
/// `change_to_sql(Change::CreateTable(t))` emits. Kept as alias
/// to keep `create_table_sql` private to the module (we do not
/// want to expose the entire internal API of the migrator).
pub fn create_table_sql_for(t: &Table) -> String {
    create_table_sql(t)
}

fn create_table_sql(t: &Table) -> String {
    let mut lines = Vec::with_capacity(t.columns.len() + 1);
    for c in &t.columns {
        let nullable = if c.is_primary {
            // PRIMARY KEY implies NOT NULL — do not add redundant.
            " PRIMARY KEY".to_string()
        } else if c.nullable {
            String::new()
        } else {
            " NOT NULL".to_string()
        };
        let default = c
            .default
            .as_deref()
            .map(|d| format!(" DEFAULT {}", d))
            .unwrap_or_default();
        lines.push(format!(
            "    {} {}{}{}",
            quote_ident(&c.name),
            c.sql_type,
            nullable,
            default,
        ));
    }
    // v0.10.27 (F2) — table-level composite PK constraint when
    // `composite_pk.len() >= 2`. Single PK stays as `is_primary`
    // inline on the column (more readable and matches introspect).
    if t.composite_pk.len() >= 2 {
        let cols: Vec<String> = t.composite_pk.iter().map(|c| quote_ident(c)).collect();
        lines.push(format!("    PRIMARY KEY ({})", cols.join(", ")));
    }
    // v0.10.29 — table-level CHECK constraints (`@check_constraint`).
    // Each one emits `CONSTRAINT "name" CHECK (<expr>)` inside the
    // CREATE TABLE so Postgres validates on INSERT/UPDATE.
    for chk in &t.check_constraints {
        lines.push(format!(
            "    CONSTRAINT {} CHECK ({})",
            quote_ident(&chk.name),
            chk.expr
        ));
    }
    // FKs NOT inline here — the diff emits them as separate ADD
    // CONSTRAINT to unblock cycles between new tables.
    format!(
        "CREATE TABLE {} (\n{}\n);",
        quote_qualified(&TableRef::from_table(t)),
        lines.join(",\n"),
    )
}

/// PG-style quote of an identifier. We only wrap in `"` names
/// that have non-ASCII-alphanumeric chars or that match reserved
/// words. For MVP simplicity: we ALWAYS quote — trade-off: more
/// verbose SQL, but 100% safe against reserved words and PG case
/// sensitivity.
fn quote_ident(name: &str) -> String {
    // Escape `"` inside the name (rare but possible).
    let escaped = name.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

/// v0.10.21 (10.6.e.3) — PG-style quote with optional schema
/// qualifier. `schema = None` → `"name"`; `schema = Some(s)` →
/// `"s"."name"`. The Change SQL emit always uses this helper so
/// that tables in custom schemas work.
fn quote_qualified(t: &TableRef) -> String {
    match &t.schema {
        Some(s) => format!("{}.{}", quote_ident(s), quote_ident(&t.name)),
        None => quote_ident(&t.name),
    }
}

// =================================================================
// Tracking + executor (apply_pending_migrations)
// =================================================================

/// Name of the internal table where Fitz tracks which migrations
/// ran. Idempotent — re-running `migrate` does not apply ones
/// already applied.
const TRACKING_TABLE: &str = "_fitz_migrations";

/// A migration found in the `migrations/` directory. The `version`
/// is the filename prefix (typically timestamp
/// `YYYYMMDDHHMMSS_description.sql`). Postgres orders by
/// lexicographic `version` = real chronological order.
///
/// v0.10.17 (10.6.b.1) — the SQL is split into `up_sql` and
/// `down_sql` via `-- UP` / `-- DOWN` markers. Backward-compat:
/// files without markers → everything is UP, `down_sql = None`
/// (does not support rollback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationFile {
    pub version: String,
    pub filename: String,
    pub kind: MigrationKind,
}

/// v0.10.19 (10.6.d) — Migration type according to its backend.
///
/// - `Sql`: `.sql` file with raw SQL splittable into `-- UP` /
///   `-- DOWN` (keeps v0.10.17 semantics).
/// - `Fitz` (v0.10.19): `.fitz` file that declares `async fn
///   migrate(db: DbConn) -> Result<Null>` and optionally
///   `async fn rollback(db: DbConn) -> Result<Null>`. The runner
///   parses + invokes the fn inside tx with `db` bound. Enables
///   transforms with logic that raw SQL cannot express (parsing
///   old JSON → new cols, conditional back-fills, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationKind {
    Sql {
        /// SQL to execute in `migrate` (forward). Always present.
        up_sql: String,
        /// SQL to execute in `rollback`. `None` if the migration
        /// did not declare a `-- DOWN` section.
        down_sql: Option<String>,
    },
    Fitz {
        /// Absolute path to the `.fitz` file. We keep it in case
        /// the runner needs base_dir to resolve relative imports
        /// from the module loader.
        path: std::path::PathBuf,
        /// Full source of the file. Cached on read to avoid extra
        /// I/O during dispatch.
        source: String,
    },
}

impl MigrationFile {
    /// `true` if the migration is a `.fitz` script with logic
    /// (needs the language runner, not direct `db.exec`).
    pub fn is_fitz(&self) -> bool {
        matches!(self.kind, MigrationKind::Fitz { .. })
    }
}

/// State of a migration: applied in the DB, or pending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationStatus {
    Applied,
    Pending,
}

/// Creates the tracking table if it does not exist. Idempotent.
pub async fn ensure_tracking_table(conn: &std::sync::Arc<DbConnHandle>) -> DbResult<()> {
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {} ( \
            version VARCHAR(255) PRIMARY KEY, \
            applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW() \
         )",
        quote_ident(TRACKING_TABLE)
    );
    conn.exec(&sql, &[]).await?;
    Ok(())
}

/// Lists the already-applied versions, in chronological order.
pub async fn applied_versions(conn: &std::sync::Arc<DbConnHandle>) -> DbResult<Vec<String>> {
    ensure_tracking_table(conn).await?;
    let sql = format!(
        "SELECT version FROM {} ORDER BY version",
        quote_ident(TRACKING_TABLE),
    );
    let qr = conn.query(&sql, &[]).await?;
    qr.rows
        .iter()
        .map(|row| extract_string(row, "version"))
        .collect()
}

/// Reads migration files from the `dir` directory. Filters
/// `*.sql`, extracts version from the filename (prefix up to `_`
/// or the full filename without extension). Lexicographic order
/// = chronological if you use `YYYYMMDDHHMMSS_*` timestamps.
pub fn read_migrations_dir(dir: &std::path::Path) -> Result<Vec<MigrationFile>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("leyendo `{}`: {e}", dir.display()))?;
    let mut migrations = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read_dir entry: {e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(filename_os) = path.file_name() else {
            continue;
        };
        let filename = filename_os.to_string_lossy().into_owned();
        // v0.10.19 — we accept `.sql` AND `.fitz`. The runner
        // dispatches by `MigrationKind` afterwards.
        let ext = if filename.ends_with(".sql") {
            ".sql"
        } else if filename.ends_with(".fitz") {
            ".fitz"
        } else {
            continue;
        };
        let version = filename
            .strip_suffix(ext)
            .unwrap_or(&filename)
            .split_once('_')
            .map(|(prefix, _)| prefix.to_string())
            .unwrap_or_else(|| filename.strip_suffix(ext).unwrap_or(&filename).to_string());
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("leyendo `{}`: {e}", path.display()))?;
        let kind = match ext {
            ".sql" => {
                let (up_sql, down_sql) = split_up_down(&raw);
                MigrationKind::Sql { up_sql, down_sql }
            }
            ".fitz" => MigrationKind::Fitz {
                path: path.clone(),
                source: raw,
            },
            _ => unreachable!(),
        };
        migrations.push(MigrationFile {
            version,
            filename,
            kind,
        });
    }
    migrations.sort_by(|a, b| a.version.cmp(&b.version));
    Ok(migrations)
}

/// v0.10.17 (10.6.b.1) — Split of `.sql` content into `-- UP` and
/// `-- DOWN` sections. Rules:
///
/// - If there is NO `-- UP` or `-- DOWN` marker → all content is
///   UP, `down_sql = None`. Backward-compat with v0.10.16
///   migrations without explicit sections.
/// - If there is `-- UP` (with or without `-- DOWN`) → UP is the
///   range between `-- UP` and `-- DOWN` (or EOF if no DOWN).
/// - If there is `-- DOWN` without a previous `-- UP` → everything
///   before is UP, what follows is DOWN. (Case: user places the
///   DOWN marker without explicit UP marker.)
/// - Case-insensitive marker on its own line (whitespace allowed
///   before and after, no additional chars inside). `-- up`,
///   `-- Up`, `--  UP` match; `-- UP foo` does not.
/// - If `down_sql` ends up as empty/whitespace string → `None`
///   (declared DOWN section but empty is equivalent to not
///   declaring it).
fn split_up_down(raw: &str) -> (String, Option<String>) {
    let mut up_lines: Vec<&str> = Vec::new();
    let mut down_lines: Vec<&str> = Vec::new();
    // Modes: 0 = pre-marker (counts as UP), 1 = in UP, 2 = in DOWN.
    let mut mode: u8 = 0;
    let mut saw_up_marker = false;
    let mut saw_down_marker = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        let is_up_marker = lower == "-- up" || lower == "--up";
        let is_down_marker = lower == "-- down" || lower == "--down";
        if is_up_marker {
            mode = 1;
            saw_up_marker = true;
            continue;
        }
        if is_down_marker {
            mode = 2;
            saw_down_marker = true;
            continue;
        }
        match mode {
            0 | 1 => up_lines.push(line),
            2 => down_lines.push(line),
            _ => unreachable!(),
        }
    }
    let up_sql = up_lines.join("\n");
    let down_sql = if saw_down_marker {
        let joined = down_lines.join("\n");
        if joined.trim().is_empty() {
            None
        } else {
            Some(joined)
        }
    } else {
        None
    };
    // Backward-compat sanity: if there was NO UP or DOWN marker,
    // up_sql is the entire content (mode 0 = whole file).
    let _ = saw_up_marker;
    (up_sql, down_sql)
}

/// Applies a migration inside a transaction. If the SQL fails,
/// the tx is reverted and it is NOT tracked in `_fitz_migrations`.
/// If OK, it is inserted into tracking + COMMIT atomically.
///
/// **Atomicity guarantee**: the entire migration (all its
/// statements + the insert into tracking) runs in 1 BEGIN/COMMIT.
/// Either everything persists or nothing — no intermediate states.
pub async fn apply_migration(
    conn: &std::sync::Arc<DbConnHandle>,
    migration: &MigrationFile,
) -> DbResult<()> {
    let MigrationKind::Sql { up_sql, .. } = &migration.kind else {
        return Err(DbError::Protocol(format!(
            "apply_migration: `{}` es una `.fitz` migration y NO se aplica via SQL crudo. \
             El caller (CLI) debe despachar a un runner del lenguaje. \
             Si llegaste acá desde código tuyo, usá la dispatch por `MigrationKind` antes \
             de llamar a `apply_migration`.",
            migration.filename
        )));
    };
    ensure_tracking_table(conn).await?;
    let version = migration.version.clone();
    let sql = up_sql.clone();
    let tracking_table = TRACKING_TABLE.to_string();
    conn.transaction(move |tx| {
        let version = version.clone();
        let sql = sql.clone();
        let tracking_table = tracking_table.clone();
        async move {
            // Postgres `simple_query` allows multiple statements
            // separated by `;` in a single call (unlike the
            // Extended Query Protocol which is 1 stmt per request).
            // We use `query` which internally dispatches to
            // simple_query when args is_empty.
            tx.query(&sql, &[]).await?;
            // Track the version. INSERT in the same tx — atomic.
            let insert_sql = format!(
                "INSERT INTO {} (version) VALUES ($1)",
                quote_ident(&tracking_table),
            );
            tx.exec(&insert_sql, &[PgValue::Text(version)]).await?;
            Ok(())
        }
    })
    .await
}

/// v0.10.19 (10.6.d) — Helper for the caller (main.rs) to mark a
/// `.fitz` migration as applied AFTER the language runner has
/// successfully executed `async fn migrate(db)`. The user fn's tx
/// already committed (via `db.transaction` inside the invocation),
/// so here we only insert into `_fitz_migrations` as a separate
/// act.
///
/// **Atomicity note**: `.fitz` migrations are NOT atomic with
/// respect to tracking — if the `INSERT INTO _fitz_migrations`
/// fails after the script already committed, it stays in
/// "applied but not tracked" state. `migrate` would re-apply it
/// on the next run. It is the script's responsibility to be
/// idempotent (parallel to `CREATE TABLE IF NOT EXISTS` in
/// `.sql`).
pub async fn track_fitz_migration_applied(
    conn: &std::sync::Arc<DbConnHandle>,
    version: &str,
) -> DbResult<()> {
    ensure_tracking_table(conn).await?;
    let insert_sql = format!(
        "INSERT INTO {} (version) VALUES ($1) ON CONFLICT (version) DO NOTHING",
        quote_ident(TRACKING_TABLE),
    );
    conn.exec(&insert_sql, &[PgValue::Text(version.to_string())])
        .await?;
    Ok(())
}

/// v0.10.19 (10.6.d) — Deletes the tracking of a successfully
/// reverted `.fitz` migration. Parallel to
/// `track_fitz_migration_applied` for the rollback path.
pub async fn untrack_fitz_migration(
    conn: &std::sync::Arc<DbConnHandle>,
    version: &str,
) -> DbResult<()> {
    ensure_tracking_table(conn).await?;
    let delete_sql = format!(
        "DELETE FROM {} WHERE version = $1",
        quote_ident(TRACKING_TABLE),
    );
    conn.exec(&delete_sql, &[PgValue::Text(version.to_string())])
        .await?;
    Ok(())
}

/// v0.10.17 (10.6.b.1) — Reverts a migration: executes its
/// `-- DOWN` section inside tx + deletes the `_fitz_migrations`
/// record. Atomic: either everything persists or nothing.
///
/// **Errors**:
/// - If `down_sql` is `None` (migration without `-- DOWN`):
///   returns `DbError::Protocol` with a clear message citing the
///   filename. The caller must abort the entire rollback — without
///   DOWN there is no safe way to revert.
/// - If the DOWN SQL fails: the tx is reverted (the tracking
///   record is not deleted) — the partial rollback is NOT
///   persisted.
pub async fn revert_migration(
    conn: &std::sync::Arc<DbConnHandle>,
    migration: &MigrationFile,
) -> DbResult<()> {
    let MigrationKind::Sql { down_sql, .. } = &migration.kind else {
        return Err(DbError::Protocol(format!(
            "revert_migration: `{}` es una `.fitz` migration. \
             El rollback de `.fitz` requiere despachar al runner del \
             lenguaje (responsabilidad del CLI), no a SQL crudo.",
            migration.filename
        )));
    };
    let down_sql = down_sql.as_ref().ok_or_else(|| {
        DbError::Protocol(format!(
            "migration `{}` no tiene sección `-- DOWN` — no se puede revertir. \
             Agregá la sección `-- DOWN` con el SQL inverso (`DROP COLUMN`, \
             etc.) y volvé a correr `fitz db rollback`.",
            migration.filename
        ))
    })?;
    ensure_tracking_table(conn).await?;
    let version = migration.version.clone();
    let sql = down_sql.clone();
    let tracking_table = TRACKING_TABLE.to_string();
    conn.transaction(move |tx| {
        let version = version.clone();
        let sql = sql.clone();
        let tracking_table = tracking_table.clone();
        async move {
            tx.query(&sql, &[]).await?;
            let delete_sql = format!(
                "DELETE FROM {} WHERE version = $1",
                quote_ident(&tracking_table),
            );
            tx.exec(&delete_sql, &[PgValue::Text(version)]).await?;
            Ok(())
        }
    })
    .await
}

/// v0.10.17 (10.6.b.1) — Rollback of the last `n` applied
/// migrations. Reads `_fitz_migrations` ordered by
/// `applied_at DESC`, joins with the dir's files, and applies
/// `revert_migration` in that order (most recent first).
///
/// Returns the reverted versions (empty if nothing was applied
/// or `n=0`).
///
/// **Fatal errors** (abort BEFORE touching the DB):
/// - Some applied migration has NO file in the dir (file was
///   deleted after applying): we cannot revert without the
///   `-- DOWN`. Specific error.
/// - Some rollback target migration has NO `-- DOWN`: specific
///   error citing filename.
///
/// **Incremental behavior**: if the Nth revert fails at runtime,
/// the previous ones ALREADY persisted (each `revert_migration`
/// is individually atomic). For "all or nothing" rollback over N
/// migrations one would have to wrap in an outer transaction —
/// minor debt (rare to have N>1 in a typical rollback).
pub async fn rollback_n(
    conn: &std::sync::Arc<DbConnHandle>,
    migrations: &[MigrationFile],
    n: usize,
) -> DbResult<Vec<String>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    ensure_tracking_table(conn).await?;
    // Applied versions, ordered by applied_at DESC (most recent
    // first). Direct read from _fitz_migrations.
    let applied_desc = applied_versions_desc(conn).await?;
    if applied_desc.is_empty() {
        return Ok(Vec::new());
    }
    let target_versions: Vec<&String> = applied_desc.iter().take(n).collect();
    // Look up each version in the migrations dir.
    let by_version: std::collections::HashMap<&str, &MigrationFile> =
        migrations.iter().map(|m| (m.version.as_str(), m)).collect();
    // Pre-flight: validate that ALL target versions have file +
    // DOWN, BEFORE starting to revert. Fail fast.
    for v in &target_versions {
        let m = by_version.get(v.as_str()).ok_or_else(|| {
            DbError::Protocol(format!(
                "rollback: la version `{v}` está aplicada en la DB pero \
                 NO hay archivo en el dir de migrations. Restaurá el \
                 archivo o stampealá manualmente con SQL."
            ))
        })?;
        // v0.10.19 — `.fitz` migrations on the SQL-rollback path
        // are a fast error: the CLI must dispatch them to the
        // language runner before invoking `rollback_n`. As a
        // defense, we reject here.
        match &m.kind {
            MigrationKind::Fitz { .. } => {
                return Err(DbError::Protocol(format!(
                    "rollback: migration `{}` es `.fitz` — el rollback \
                     requiere despachar al runner del lenguaje, no SQL \
                     crudo. Si llegaste acá vía `rollback_n` directo, \
                     usá el dispatch del CLI o llamá al runner manual.",
                    m.filename
                )));
            }
            MigrationKind::Sql { down_sql, .. } => {
                if down_sql.is_none() {
                    return Err(DbError::Protocol(format!(
                        "rollback: migration `{}` no tiene sección `-- DOWN` — \
                         no se puede revertir. Editá el archivo agregando `-- DOWN` \
                         con el SQL inverso y reintentá.",
                        m.filename
                    )));
                }
            }
        }
    }
    let mut reverted = Vec::with_capacity(target_versions.len());
    for v in target_versions {
        let m = by_version.get(v.as_str()).expect("pre-flight validated");
        revert_migration(conn, m).await?;
        reverted.push(v.clone());
    }
    Ok(reverted)
}

/// v0.10.20 (10.6.e.1) — Entry of the audit log of applied
/// migrations. Emitted by `fitz db history`. `applied_at` comes
/// as an ISO 8601 string from Postgres (the caller decides
/// display).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub version: String,
    pub applied_at: String,
    /// Filename of the file in the dir, if it exists. `None` if
    /// the version is applied but the file was removed (typical
    /// case: `db stamp <legacy_version>` without file, or squash
    /// that moved the old one to `migrations/squashed/`).
    pub filename: Option<String>,
}

/// v0.10.20 (10.6.e.1) — Audit log of applied migrations.
/// Returns the entries ordered by `applied_at DESC` (most recent
/// first) — natural order for "what was applied last".
pub async fn history(
    conn: &std::sync::Arc<DbConnHandle>,
    dir: &std::path::Path,
) -> DbResult<Vec<HistoryEntry>> {
    ensure_tracking_table(conn).await?;
    let sql = format!(
        "SELECT version, applied_at FROM {} ORDER BY applied_at DESC, version DESC",
        quote_ident(TRACKING_TABLE),
    );
    let qr = conn.query(&sql, &[]).await?;
    let files = read_migrations_dir(dir).unwrap_or_default();
    let by_version: std::collections::HashMap<String, String> = files
        .iter()
        .map(|m| (m.version.clone(), m.filename.clone()))
        .collect();
    let mut entries = Vec::with_capacity(qr.rows.len());
    for row in &qr.rows {
        let version = extract_string(row, "version")?;
        let applied_at = match row.get("applied_at") {
            Some(PgValue::Text(s)) => s.clone(),
            Some(other) => format!("{other:?}"),
            None => String::new(),
        };
        let filename = by_version.get(&version).cloned();
        entries.push(HistoryEntry {
            version,
            applied_at,
            filename,
        });
    }
    Ok(entries)
}

/// v0.10.18 (10.6.c.2) — Marks a version as applied in
/// `_fitz_migrations` WITHOUT executing the file's SQL. Useful
/// to adopt Fitz on a legacy DB where the schema is already
/// applied manually — without stamp, `migrate` would try to
/// re-apply the `CREATE TABLE IF NOT EXISTS ...` which might be
/// fine but the seed data or ALTER may already be there.
///
/// Returns:
/// - `Ok(true)` if it inserted (the version was not registered).
/// - `Ok(false)` if the version was ALREADY applied (no-op).
///
/// **Does NOT validate** that the version exists in the
/// migrations dir — the caller (CLI handler `db_stamp_cmd`)
/// decides the policy and shows warning if not in the dir.
pub async fn stamp_version(conn: &std::sync::Arc<DbConnHandle>, version: &str) -> DbResult<bool> {
    ensure_tracking_table(conn).await?;
    let select_sql = format!(
        "SELECT version FROM {} WHERE version = $1",
        quote_ident(TRACKING_TABLE),
    );
    let qr = conn
        .query(&select_sql, &[PgValue::Text(version.to_string())])
        .await?;
    if !qr.rows.is_empty() {
        return Ok(false);
    }
    // INSERT with ON CONFLICT DO NOTHING as a defense against
    // race: two concurrent `stamp`s on the same version.
    let insert_sql = format!(
        "INSERT INTO {} (version) VALUES ($1) ON CONFLICT (version) DO NOTHING",
        quote_ident(TRACKING_TABLE),
    );
    conn.exec(&insert_sql, &[PgValue::Text(version.to_string())])
        .await?;
    Ok(true)
}

/// v0.10.18 (10.6.c.2) — Marks ALL pending migrations of the dir
/// as applied without executing SQL. Returns the newly stamped
/// versions (empty if all were already stamped). Useful to adopt
/// Fitz in projects with legacy schema + multiple migrations
/// already applied by hand.
pub async fn stamp_all_pending(
    conn: &std::sync::Arc<DbConnHandle>,
    migrations: &[MigrationFile],
) -> DbResult<Vec<String>> {
    let applied = applied_versions(conn).await?;
    let applied_set: std::collections::HashSet<&str> = applied.iter().map(|s| s.as_str()).collect();
    let mut stamped = Vec::new();
    for m in migrations {
        if applied_set.contains(m.version.as_str()) {
            continue;
        }
        if stamp_version(conn, &m.version).await? {
            stamped.push(m.version.clone());
        }
    }
    Ok(stamped)
}

/// Applied versions ordered by `applied_at DESC` (most recent
/// first). Used by `rollback_n` to take the last N revertible
/// ones.
async fn applied_versions_desc(conn: &std::sync::Arc<DbConnHandle>) -> DbResult<Vec<String>> {
    ensure_tracking_table(conn).await?;
    let sql = format!(
        "SELECT version FROM {} ORDER BY applied_at DESC, version DESC",
        quote_ident(TRACKING_TABLE),
    );
    let qr = conn.query(&sql, &[]).await?;
    qr.rows
        .iter()
        .map(|row| extract_string(row, "version"))
        .collect()
}

/// Applies ALL pending migrations in order. Skips already-applied
/// ones (idempotent).
///
/// Returns the applied versions (empty if nothing pending).
pub async fn apply_pending_migrations(
    conn: &std::sync::Arc<DbConnHandle>,
    migrations: &[MigrationFile],
) -> DbResult<Vec<String>> {
    let applied = applied_versions(conn).await?;
    let applied_set: std::collections::HashSet<&str> = applied.iter().map(|s| s.as_str()).collect();
    let mut new_versions = Vec::new();
    for m in migrations {
        if applied_set.contains(m.version.as_str()) {
            continue;
        }
        apply_migration(conn, m).await?;
        new_versions.push(m.version.clone());
    }
    Ok(new_versions)
}

/// Status report: joins files in `dir` with applied_versions →
/// returns `(version, filename, status)` per migration. Useful
/// for `fitz db status`.
pub async fn status(
    conn: &std::sync::Arc<DbConnHandle>,
    dir: &std::path::Path,
) -> DbResult<Vec<(String, String, MigrationStatus)>> {
    let migrations = read_migrations_dir(dir).map_err(DbError::Protocol)?;
    let applied = applied_versions(conn).await?;
    let applied_set: std::collections::HashSet<&str> = applied.iter().map(|s| s.as_str()).collect();
    let mut out = Vec::with_capacity(migrations.len());
    for m in &migrations {
        let status = if applied_set.contains(m.version.as_str()) {
            MigrationStatus::Applied
        } else {
            MigrationStatus::Pending
        };
        out.push((m.version.clone(), m.filename.clone(), status));
    }
    Ok(out)
}

// =================================================================
// Row extraction helpers
// =================================================================

fn extract_string(row: &crate::db::Row, col: &str) -> DbResult<String> {
    match row.get(col) {
        Some(PgValue::Text(s)) => Ok(s.clone()),
        Some(PgValue::Null) => Err(DbError::Protocol(format!(
            "introspect: column `{col}` unexpected NULL"
        ))),
        Some(other) => Err(DbError::Protocol(format!(
            "introspect: column `{col}` expected Text, received {other:?}"
        ))),
        None => Err(DbError::Protocol(format!(
            "introspect: column `{col}` not present in row"
        ))),
    }
}

fn extract_string_opt(row: &crate::db::Row, col: &str) -> Option<String> {
    match row.get(col) {
        Some(PgValue::Text(s)) => Some(s.clone()),
        _ => None,
    }
}

fn extract_bool(row: &crate::db::Row, col: &str) -> Option<bool> {
    match row.get(col) {
        Some(PgValue::Bool(b)) => Some(*b),
        Some(PgValue::Text(s)) => match s.to_ascii_lowercase().as_str() {
            "t" | "true" | "yes" => Some(true),
            "f" | "false" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Canonicalizes the SQL type that `information_schema` reports
/// so that it matches what we generate from the @table type
/// side. PG reports names like "double precision" in `data_type`
/// but "float8" in `udt_name`; we prefer the more readable
/// standard name (`text`, `bigint`, `boolean`, etc.).
fn canonicalize_sql_type(data_type: &str, udt_name: &str) -> String {
    // `data_type` is usually the SQL standard name ("bigint",
    // "text", "boolean", "timestamp with time zone", "jsonb",
    // "ARRAY"). For ARRAY we need the `udt_name` with `_` prefix
    // (e.g.: `_text` for `text[]`) to reconstruct.
    if data_type == "ARRAY" {
        // udt_name comes as `_text`/`_int8`/etc. We convert to
        // the more readable `[]` suffix.
        if let Some(elem) = udt_name.strip_prefix('_') {
            let elem_canon = match elem {
                "int8" => "bigint",
                "int4" => "integer",
                "int2" => "smallint",
                "float8" => "double precision",
                "float4" => "real",
                "bool" => "boolean",
                other => other,
            };
            return format!("{elem_canon}[]");
        }
    }
    data_type.to_string()
}

// =================================================================
// v0.10.28 — `fitz db inspect`: formatters of introspected schema
// =================================================================

/// Filters schema tables according to `schema_filter` (default
/// `"public"` if `None`) and optional `table_filter`. Returns a
/// new vec with the matching tables; order preserved from input
/// (already ordered by `(schema, name)` from `introspect_schema`).
fn filter_inspect_tables<'a>(
    schema: &'a Schema,
    schema_filter: Option<&str>,
    table_filter: Option<&str>,
) -> Vec<&'a Table> {
    let want_schema = schema_filter.unwrap_or("public");
    schema
        .tables
        .iter()
        .filter(|t| {
            let s = t.schema.as_deref().unwrap_or("public");
            s == want_schema
        })
        .filter(|t| match table_filter {
            Some(name) => t.name == name,
            None => true,
        })
        .collect()
}

/// Readable plain-text view of the introspected schema. Designed
/// for humans in terminal — tabular alignment of columns,
/// PK/FK/indexes annotated, schema-qualified if the filter asks
/// for something other than `public`.
pub fn format_inspection_text(
    schema: &Schema,
    schema_filter: Option<&str>,
    table_filter: Option<&str>,
) -> String {
    let want_schema = schema_filter.unwrap_or("public");
    let filtered = filter_inspect_tables(schema, schema_filter, table_filter);

    let mut out = String::new();
    out.push_str(&format!("Schema: {want_schema}\n"));

    if filtered.is_empty() {
        match table_filter {
            Some(name) => {
                out.push_str(&format!(
                    "\n  (table `{name}` not found in schema `{want_schema}`)\n"
                ));
            }
            None => {
                out.push_str(&format!(
                    "\n  (no user-defined tables in schema `{want_schema}`)\n"
                ));
            }
        }
        return out;
    }

    for t in &filtered {
        out.push('\n');
        out.push_str(&format!(
            "Table: {} ({} col{})\n",
            t.name,
            t.columns.len(),
            if t.columns.len() == 1 { "" } else { "s" }
        ));

        // Dynamic width to align name + type per column.
        let max_name = t.columns.iter().map(|c| c.name.len()).max().unwrap_or(0);
        let max_type = t
            .columns
            .iter()
            .map(|c| c.sql_type.len())
            .max()
            .unwrap_or(0);
        let single_pk_col: Option<&str> = if t.composite_pk.is_empty() {
            t.columns
                .iter()
                .find(|c| c.is_primary)
                .map(|c| c.name.as_str())
        } else {
            None
        };

        for c in &t.columns {
            let nullable_tag = if c.nullable { "NULL    " } else { "NOT NULL" };
            let mut tags: Vec<String> = Vec::new();
            if let Some(pk) = single_pk_col {
                if pk == c.name {
                    tags.push("PK".to_string());
                }
            }
            if let Some(d) = &c.default {
                tags.push(format!("default {d}"));
            }
            let suffix = if tags.is_empty() {
                String::new()
            } else {
                format!("  {}", tags.join("  "))
            };
            out.push_str(&format!(
                "  {name:<name_w$}  {ty:<ty_w$}  {nul}{suffix}\n",
                name = c.name,
                name_w = max_name,
                ty = c.sql_type,
                ty_w = max_type,
                nul = nullable_tag,
                suffix = suffix,
            ));
        }

        if !t.composite_pk.is_empty() {
            out.push_str(&format!("  Primary key: ({})\n", t.composite_pk.join(", ")));
        }

        if !t.indexes.is_empty() {
            out.push_str("  Indexes:\n");
            for idx in &t.indexes {
                let kind = if idx.unique { "UNIQUE " } else { "" };
                let using_label = idx
                    .using
                    .as_ref()
                    .map(|u| format!("  USING {u}"))
                    .unwrap_or_default();
                let where_suffix = idx
                    .where_clause
                    .as_ref()
                    .map(|w| format!("  WHERE {w}"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "    {name}  {kind}({cols}){using_label}{where_suffix}\n",
                    name = idx.name,
                    kind = kind,
                    cols = idx.columns.join(", "),
                    using_label = using_label,
                    where_suffix = where_suffix,
                ));
            }
        }

        if !t.foreign_keys.is_empty() {
            out.push_str("  Foreign keys:\n");
            for fk in &t.foreign_keys {
                let on_delete = fk
                    .on_delete
                    .as_ref()
                    .map(|d| format!(" ON DELETE {d}"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "    {name}: {col} -> {rt}({rc}){on_delete}\n",
                    name = fk.name,
                    col = fk.column,
                    rt = fk.references_table,
                    rc = fk.references_column,
                    on_delete = on_delete,
                ));
            }
        }
    }

    out
}

/// JSON machine-readable del schema introspectado. Shape lockeada
/// para parsers externos:
///
/// ```json
/// {
///   "schema": "public",
///   "tables": [
///     {
///       "name": "users",
///       "schema": "public",
///       "columns": [
///         { "name": "id", "sql_type": "bigint", "nullable": false,
///           "default": null, "is_primary": true }
///       ],
///       "primary_key": ["id"],
///       "indexes": [
///         { "name": "idx_users_email", "columns": ["email"],
///           "unique": true, "where_clause": null }
///       ],
///       "foreign_keys": [
///         { "name": "fk_posts_author", "column": "author_id",
///           "references_table": "users", "references_column": "id",
///           "on_delete": "CASCADE" }
///       ]
///     }
///   ]
/// }
/// ```
///
/// `serde_json` with `preserve_order` feature guarantees stable
/// field order. Tables and columns come in the order returned by
/// introspect (canonical sort by `(schema, name)` and declaration
/// order respectively).
pub fn format_inspection_json(
    schema: &Schema,
    schema_filter: Option<&str>,
    table_filter: Option<&str>,
) -> Result<String, String> {
    let want_schema = schema_filter.unwrap_or("public");
    let filtered = filter_inspect_tables(schema, schema_filter, table_filter);

    let tables: Vec<serde_json::Value> = filtered
        .iter()
        .map(|t| {
            let primary_key: Vec<&str> = if !t.composite_pk.is_empty() {
                t.composite_pk.iter().map(|s| s.as_str()).collect()
            } else {
                t.columns
                    .iter()
                    .filter(|c| c.is_primary)
                    .map(|c| c.name.as_str())
                    .collect()
            };
            let columns: Vec<serde_json::Value> = t
                .columns
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "name": c.name,
                        "sql_type": c.sql_type,
                        "nullable": c.nullable,
                        "default": c.default,
                        "is_primary": c.is_primary,
                    })
                })
                .collect();
            let indexes: Vec<serde_json::Value> = t
                .indexes
                .iter()
                .map(|i| {
                    serde_json::json!({
                        "name": i.name,
                        "columns": i.columns,
                        "unique": i.unique,
                        "where_clause": i.where_clause,
                        "using": i.using,
                    })
                })
                .collect();
            let foreign_keys: Vec<serde_json::Value> = t
                .foreign_keys
                .iter()
                .map(|fk| {
                    serde_json::json!({
                        "name": fk.name,
                        "column": fk.column,
                        "references_table": fk.references_table,
                        "references_column": fk.references_column,
                        "on_delete": fk.on_delete,
                    })
                })
                .collect();
            serde_json::json!({
                "name": t.name,
                "schema": t.schema.as_deref().unwrap_or("public"),
                "columns": columns,
                "primary_key": primary_key,
                "indexes": indexes,
                "foreign_keys": foreign_keys,
            })
        })
        .collect();

    let report = serde_json::json!({
        "schema": want_schema,
        "tables": tables,
    });

    serde_json::to_string_pretty(&report).map_err(|e| format!("serde_json: {e}"))
}

/// v0.10.29 — Lists the user-defined schemas detected by the
/// introspect, in deterministic alphabetic order, with their
/// tables grouped. Each table without explicit schema is treated
/// as `public`. Useful to audit multi-schema DBs without having
/// to run `fitz db inspect --schema X` for each one.
fn collect_schemas_from_tables(schema: &Schema) -> Vec<&str> {
    let mut set: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for t in &schema.tables {
        set.insert(t.schema.as_deref().unwrap_or("public"));
    }
    set.into_iter().collect()
}

/// v0.10.29 — Variant of [`format_inspection_text`] that iterates
/// ALL detected schemas. Output: each schema with its header +
/// sub-view. If `table_filter` is set, filters the name across
///   ALL schemas — the same name may appear in several.
pub fn format_inspection_text_all_schemas(schema: &Schema, table_filter: Option<&str>) -> String {
    let mut out = String::new();
    let schemas = collect_schemas_from_tables(schema);
    if schemas.is_empty() {
        out.push_str("(no user-defined tables detected)\n");
        return out;
    }
    out.push_str(&format!(
        "Detected schemas: {}\n",
        schemas.to_vec().join(", ")
    ));
    for s in &schemas {
        out.push_str("\n=== ");
        let sub = format_inspection_text(schema, Some(s), table_filter);
        out.push_str(sub.trim_start_matches("Schema: ").trim_end());
        out.push_str("\n=== end\n");
    }
    out
}

/// v0.10.29 — Variant of [`format_inspection_json`] that emits a
/// JSON report with user-defined schemas grouped. Shape:
/// `{"schemas": [{"schema": "public", "tables": [...]}, ...]}`.
/// Deterministic (alphabetic sort of schema names).
pub fn format_inspection_json_all_schemas(
    schema: &Schema,
    table_filter: Option<&str>,
) -> Result<String, String> {
    let schemas = collect_schemas_from_tables(schema);
    let mut reports = Vec::with_capacity(schemas.len());
    for s in &schemas {
        let single = format_inspection_json(schema, Some(s), table_filter)?;
        let parsed: serde_json::Value =
            serde_json::from_str(&single).map_err(|e| format!("serde_json reparse: {e}"))?;
        reports.push(parsed);
    }
    let report = serde_json::json!({
        "schemas": reports,
    });
    serde_json::to_string_pretty(&report).map_err(|e| format!("serde_json: {e}"))
}

// =================================================================
// Unit tests — only what does not require a real DB
// =================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_sql_type_basic() {
        assert_eq!(canonicalize_sql_type("text", "text"), "text");
        assert_eq!(canonicalize_sql_type("bigint", "int8"), "bigint");
        assert_eq!(canonicalize_sql_type("boolean", "bool"), "boolean");
        assert_eq!(
            canonicalize_sql_type("timestamp with time zone", "timestamptz"),
            "timestamp with time zone"
        );
        assert_eq!(canonicalize_sql_type("jsonb", "jsonb"), "jsonb");
    }

    #[test]
    fn canonicalize_sql_type_arrays() {
        assert_eq!(canonicalize_sql_type("ARRAY", "_text"), "text[]");
        assert_eq!(canonicalize_sql_type("ARRAY", "_int8"), "bigint[]");
        assert_eq!(canonicalize_sql_type("ARRAY", "_int4"), "integer[]");
        assert_eq!(
            canonicalize_sql_type("ARRAY", "_float8"),
            "double precision[]"
        );
        assert_eq!(canonicalize_sql_type("ARRAY", "_bool"), "boolean[]");
    }

    #[test]
    fn schema_default_empty() {
        let s = Schema::default();
        assert!(s.tables.is_empty());
    }

    // ============================================================
    // diff_schemas — fixtures
    // ============================================================

    fn col(name: &str, sql_type: &str, nullable: bool, is_primary: bool) -> Column {
        Column {
            name: name.to_string(),
            sql_type: sql_type.to_string(),
            nullable,
            default: None,
            is_primary,
            renamed_from: None,
        }
    }

    fn table_users() -> Table {
        Table {
            name: "users".to_string(),
            columns: vec![
                col("id", "bigint", false, true),
                col("email", "text", false, false),
            ],
            indexes: vec![],
            foreign_keys: vec![],
            renamed_from: None,
            schema: None,
            composite_pk: vec![],
            check_constraints: vec![],
        }
    }

    #[test]
    fn diff_empty_target_against_empty_current_is_empty() {
        let s = Schema::default();
        assert_eq!(diff_schemas(&s, &s), vec![]);
    }

    #[test]
    fn diff_target_equal_to_current_is_empty() {
        let s = Schema {
            tables: vec![table_users()],
        };
        assert_eq!(diff_schemas(&s, &s), vec![]);
    }

    #[test]
    fn diff_new_table_emits_create_table() {
        let current = Schema::default();
        let target = Schema {
            tables: vec![table_users()],
        };
        let changes = diff_schemas(&current, &target);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::CreateTable(t) => assert_eq!(t.name, "users"),
            _ => panic!("se esperaba CreateTable, got: {:?}", changes[0]),
        }
    }

    #[test]
    fn diff_dropped_table_emits_drop_table() {
        let current = Schema {
            tables: vec![table_users()],
        };
        let target = Schema::default();
        let changes = diff_schemas(&current, &target);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::DropTable(tr) => assert_eq!(tr.name, "users"),
            _ => panic!("se esperaba DropTable, got: {:?}", changes[0]),
        }
    }

    #[test]
    fn diff_add_column_emits_add_column() {
        let current = Schema {
            tables: vec![table_users()],
        };
        let mut target_users = table_users();
        target_users.columns.push(col("name", "text", false, false));
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        let added: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::AddColumn { table, column } => {
                    Some((table.name.as_str(), column.name.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(added, vec![("users", "name")]);
    }

    #[test]
    fn diff_drop_column_emits_drop_column() {
        let current = Schema {
            tables: vec![table_users()],
        };
        let mut target_users = table_users();
        target_users.columns.retain(|c| c.name != "email");
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        let dropped: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::DropColumn { table, column } => {
                    Some((table.name.as_str(), column.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(dropped, vec![("users", "email")]);
    }

    #[test]
    fn diff_alter_column_type_emits_alter_column_type() {
        let current = Schema {
            tables: vec![table_users()],
        };
        let mut target_users = table_users();
        // email: text → varchar (type change)
        target_users
            .columns
            .iter_mut()
            .find(|c| c.name == "email")
            .unwrap()
            .sql_type = "varchar".to_string();
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        let altered: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::AlterColumnType {
                    table,
                    column,
                    new_type,
                } => Some((table.name.as_str(), column.as_str(), new_type.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(altered, vec![("users", "email", "varchar")]);
    }

    #[test]
    fn diff_alter_column_nullable_emits_alter_column_nullable() {
        let current = Schema {
            tables: vec![table_users()],
        };
        let mut target_users = table_users();
        // email: NOT NULL → NULL
        target_users
            .columns
            .iter_mut()
            .find(|c| c.name == "email")
            .unwrap()
            .nullable = true;
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        let altered: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::AlterColumnNullable {
                    table,
                    column,
                    nullable,
                } => Some((table.name.as_str(), column.as_str(), *nullable)),
                _ => None,
            })
            .collect();
        assert_eq!(altered, vec![("users", "email", true)]);
    }

    #[test]
    fn diff_create_index_emits_create_index() {
        let current = Schema {
            tables: vec![table_users()],
        };
        let mut target_users = table_users();
        target_users.indexes.push(Index {
            name: "users_email_key".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
            where_clause: None,
            using: None,
            expression: None,
        });
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        let created: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::CreateIndex { table, index } => {
                    Some((table.name.as_str(), index.name.as_str(), index.unique))
                }
                _ => None,
            })
            .collect();
        assert_eq!(created, vec![("users", "users_email_key", true)]);
    }

    #[test]
    fn diff_drop_index_emits_drop_index() {
        let mut current_users = table_users();
        current_users.indexes.push(Index {
            name: "users_email_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            where_clause: None,
            using: None,
            expression: None,
        });
        let current = Schema {
            tables: vec![current_users],
        };
        let target = Schema {
            tables: vec![table_users()],
        };
        let changes = diff_schemas(&current, &target);
        let dropped: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::DropIndex {
                    schema: _,
                    index_name,
                } => Some(index_name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(dropped, vec!["users_email_idx"]);
    }

    // v0.10.29 — Full index diff: detect changes in
    // `using` / `where_clause` / `unique` / `columns` when names
    // match. Before, the diff was purely name-based and the user
    // had to change `name=` to force regeneration.

    #[test]
    fn diff_indexes_change_of_using_triggers_drop_create() {
        // index name + cols equal, only `using` changes btree→gin
        // → must regenerate (DROP + CREATE).
        let mut current_users = table_users();
        current_users.indexes.push(Index {
            name: "users_meta_idx".to_string(),
            columns: vec!["meta".to_string()],
            unique: false,
            where_clause: None,
            using: None, // btree default
            expression: None,
        });
        let mut target_users = table_users();
        target_users.indexes.push(Index {
            name: "users_meta_idx".to_string(),
            columns: vec!["meta".to_string()],
            unique: false,
            where_clause: None,
            using: Some("gin".to_string()),
            expression: None,
        });
        let current = Schema {
            tables: vec![current_users],
        };
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        let dropped: Vec<&str> = changes
            .iter()
            .filter_map(|c| match c {
                Change::DropIndex { index_name, .. } => Some(index_name.as_str()),
                _ => None,
            })
            .collect();
        let created: Vec<(&str, Option<&str>)> = changes
            .iter()
            .filter_map(|c| match c {
                Change::CreateIndex { index, .. } => {
                    Some((index.name.as_str(), index.using.as_deref()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(dropped, vec!["users_meta_idx"]);
        assert_eq!(created, vec![("users_meta_idx", Some("gin"))]);
    }

    #[test]
    fn diff_indexes_change_of_where_clause_triggers_drop_create() {
        // Partial index: WHERE change → regenerate.
        let mut current_users = table_users();
        current_users.indexes.push(Index {
            name: "users_active_email_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
            where_clause: Some("deleted_at IS NULL".to_string()),
            using: None,
            expression: None,
        });
        let mut target_users = table_users();
        target_users.indexes.push(Index {
            name: "users_active_email_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
            where_clause: Some("disabled = false".to_string()),
            using: None,
            expression: None,
        });
        let current = Schema {
            tables: vec![current_users],
        };
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        let dropped: Vec<&str> = changes
            .iter()
            .filter_map(|c| match c {
                Change::DropIndex { index_name, .. } => Some(index_name.as_str()),
                _ => None,
            })
            .collect();
        let created_where: Vec<Option<&str>> = changes
            .iter()
            .filter_map(|c| match c {
                Change::CreateIndex { index, .. } => Some(index.where_clause.as_deref()),
                _ => None,
            })
            .collect();
        assert_eq!(dropped, vec!["users_active_email_idx"]);
        assert_eq!(created_where, vec![Some("disabled = false")]);
    }

    #[test]
    fn diff_indexes_canonicalizes_where_clause_to_avoid_spurious_regens() {
        // Same WHERE semantics but different formatting
        // (whitespace + case + trivial parens): must NOT trigger
        // regeneration. This closes the typical case where
        // Postgres re-introspects the clause with parens or case
        // different from the declared one.
        let mut current_users = table_users();
        current_users.indexes.push(Index {
            name: "users_active_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            where_clause: Some("(deleted_at IS NULL)".to_string()),
            using: None,
            expression: None,
        });
        let mut target_users = table_users();
        target_users.indexes.push(Index {
            name: "users_active_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            where_clause: Some("deleted_at IS NULL".to_string()),
            using: None,
            expression: None,
        });
        let current = Schema {
            tables: vec![current_users],
        };
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        // The comparator does NOT normalize parens (only
        // whitespace + case). We expect regen — documented as an
        // honest MVP limitation. Ideal case would be WHERE parsing
        // with formal canonicalization, minor debt. The
        // "whitespace-only" case IS canonicalized:
        let mut current_b = table_users();
        current_b.indexes.push(Index {
            name: "users_b_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            where_clause: Some("deleted_at  IS  NULL".to_string()),
            using: None,
            expression: None,
        });
        let mut target_b = table_users();
        target_b.indexes.push(Index {
            name: "users_b_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            where_clause: Some("DELETED_AT IS NULL".to_string()),
            using: None,
            expression: None,
        });
        let changes_b = diff_schemas(
            &Schema {
                tables: vec![current_b],
            },
            &Schema {
                tables: vec![target_b],
            },
        );
        let regen_count = changes_b
            .iter()
            .filter(|c| matches!(c, Change::CreateIndex { .. } | Change::DropIndex { .. }))
            .count();
        assert_eq!(
            regen_count, 0,
            "esperaba 0 cambios (whitespace+case canonicalizado), fueron: {:?}",
            changes_b
        );
        // The other case (extra parens) does trigger — documented.
        let regen_count_a = changes
            .iter()
            .filter(|c| matches!(c, Change::CreateIndex { .. } | Change::DropIndex { .. }))
            .count();
        assert!(regen_count_a >= 2);
    }

    #[test]
    fn diff_indexes_change_of_unique_triggers_drop_create() {
        // unique change → regenerate.
        let mut current_users = table_users();
        current_users.indexes.push(Index {
            name: "users_email_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            where_clause: None,
            using: None,
            expression: None,
        });
        let mut target_users = table_users();
        target_users.indexes.push(Index {
            name: "users_email_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
            where_clause: None,
            using: None,
            expression: None,
        });
        let changes = diff_schemas(
            &Schema {
                tables: vec![current_users],
            },
            &Schema {
                tables: vec![target_users],
            },
        );
        let drops = changes
            .iter()
            .filter(|c| matches!(c, Change::DropIndex { .. }))
            .count();
        let creates = changes
            .iter()
            .filter_map(|c| match c {
                Change::CreateIndex { index, .. } => Some(index.unique),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(drops, 1);
        assert_eq!(creates, vec![true]);
    }

    #[test]
    fn diff_indexes_btree_vs_none_are_equivalent() {
        // `using: None` and `using: Some("btree")` must be treated
        // as equivalent (btree is the Postgres default). Must not
        // trigger regen.
        let mut current_users = table_users();
        current_users.indexes.push(Index {
            name: "users_email_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            where_clause: None,
            using: None,
            expression: None,
        });
        let mut target_users = table_users();
        target_users.indexes.push(Index {
            name: "users_email_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            where_clause: None,
            using: Some("btree".to_string()),
            expression: None,
        });
        let changes = diff_schemas(
            &Schema {
                tables: vec![current_users],
            },
            &Schema {
                tables: vec![target_users],
            },
        );
        let regen = changes
            .iter()
            .filter(|c| matches!(c, Change::CreateIndex { .. } | Change::DropIndex { .. }))
            .count();
        assert_eq!(
            regen, 0,
            "esperaba 0 cambios (btree == None), fueron: {:?}",
            changes
        );
    }

    #[test]
    fn diff_add_foreign_key_emits_add_foreign_key() {
        let current = Schema {
            tables: vec![table_users()],
        };
        let mut target_users = table_users();
        target_users
            .columns
            .push(col("org_id", "bigint", false, false));
        target_users.foreign_keys.push(ForeignKey {
            name: "users_org_id_fkey".to_string(),
            column: "org_id".to_string(),
            references_table: "orgs".to_string(),
            references_column: "id".to_string(),
            on_delete: Some("CASCADE".to_string()),
            references_schema: None,
        });
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        let added_fks: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::AddForeignKey { table, fk } => Some((
                    table.name.as_str(),
                    fk.column.as_str(),
                    fk.references_table.as_str(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(added_fks, vec![("users", "org_id", "orgs")]);
    }

    #[test]
    fn diff_is_deterministic_across_runs() {
        let current = Schema::default();
        let mut target_users = table_users();
        target_users.columns.push(col("a", "text", true, false));
        target_users.columns.push(col("b", "text", true, false));
        let target = Schema {
            tables: vec![
                table_users(),
                Table {
                    name: "posts".to_string(),
                    columns: vec![col("id", "bigint", false, true)],
                    indexes: vec![],
                    foreign_keys: vec![],
                    renamed_from: None,
                    schema: None,
                    composite_pk: vec![],
                    check_constraints: vec![],
                },
            ],
        };
        let r1 = diff_schemas(&current, &target);
        let r2 = diff_schemas(&current, &target);
        assert_eq!(r1, r2);
    }

    #[test]
    fn diff_create_tables_ordered_before_drop_tables() {
        let current = Schema {
            tables: vec![Table {
                name: "old_table".to_string(),
                columns: vec![col("id", "bigint", false, true)],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let target = Schema {
            tables: vec![table_users()],
        };
        let changes = diff_schemas(&current, &target);
        let create_idx = changes
            .iter()
            .position(|c| matches!(c, Change::CreateTable(_)));
        let drop_idx = changes
            .iter()
            .position(|c| matches!(c, Change::DropTable(_)));
        assert!(create_idx.is_some());
        assert!(drop_idx.is_some());
        assert!(
            create_idx.unwrap() < drop_idx.unwrap(),
            "CREATE TABLE debe ir antes que DROP TABLE para evitar referencias dangling"
        );
    }

    // ============================================================
    // changes_to_sql — emission
    // ============================================================

    #[test]
    fn changes_to_sql_empty_is_empty_string() {
        let sql = changes_to_sql(&[]);
        assert!(sql.trim().is_empty(), "esperaba string vacío, got: {sql:?}");
    }

    #[test]
    fn changes_to_sql_create_table_emits_ddl() {
        let changes = vec![Change::CreateTable(table_users())];
        let sql = changes_to_sql(&changes);
        assert!(sql.contains("CREATE TABLE"), "esperaba CREATE TABLE: {sql}");
        assert!(sql.contains("\"users\""), "esperaba tabla quoted: {sql}");
        assert!(sql.contains("\"id\""), "esperaba col id quoted: {sql}");
        assert!(sql.contains("PRIMARY KEY"), "esperaba PK declarada: {sql}");
    }

    #[test]
    fn changes_to_sql_drop_table_emits_ddl() {
        let changes = vec![Change::DropTable(TableRef::public("legacy".to_string()))];
        let sql = changes_to_sql(&changes);
        assert!(sql.contains("DROP TABLE"), "esperaba DROP TABLE: {sql}");
        assert!(sql.contains("\"legacy\""), "esperaba nombre quoted: {sql}");
    }

    #[test]
    fn changes_to_sql_add_column_emits_alter_add() {
        let changes = vec![Change::AddColumn {
            table: TableRef::public("users"),
            column: col("name", "text", true, false),
        }];
        let sql = changes_to_sql(&changes);
        assert!(sql.contains("ALTER TABLE"), "esperaba ALTER TABLE: {sql}");
        assert!(sql.contains("ADD COLUMN"), "esperaba ADD COLUMN: {sql}");
        assert!(sql.contains("\"name\""), "esperaba col quoted: {sql}");
    }

    #[test]
    fn changes_to_sql_drop_column_emits_alter_drop() {
        let changes = vec![Change::DropColumn {
            table: TableRef::public("users"),
            column: "old".to_string(),
        }];
        let sql = changes_to_sql(&changes);
        assert!(sql.contains("ALTER TABLE"), "esperaba ALTER TABLE: {sql}");
        assert!(sql.contains("DROP COLUMN"), "esperaba DROP COLUMN: {sql}");
        assert!(sql.contains("\"old\""), "esperaba col quoted: {sql}");
    }

    #[test]
    fn changes_to_sql_alter_column_type_emits_alter_type() {
        let changes = vec![Change::AlterColumnType {
            table: TableRef::public("users"),
            column: "email".to_string(),
            new_type: "varchar".to_string(),
        }];
        let sql = changes_to_sql(&changes);
        assert!(sql.contains("ALTER COLUMN"), "esperaba ALTER COLUMN: {sql}");
        assert!(sql.contains("TYPE"), "esperaba TYPE: {sql}");
        assert!(sql.contains("varchar"), "esperaba new_type: {sql}");
    }

    // ============================================================
    // schema_from_program — round-trip Fitz AST → Schema
    // ============================================================

    fn parse_and_check(src: &str) -> (crate::ast::Program, crate::types::TypeEnv) {
        let tokens = crate::lexer::tokenize(src).expect("lex");
        let program = crate::parser::parse(tokens).expect("parse");
        let (env, _ti, _di, errs) = crate::types::check_program(&program);
        assert!(errs.is_empty(), "checker errores: {errs:?}");
        (program, env)
    }

    #[test]
    fn schema_from_program_basic_orm_type() {
        let src = r#"
@table("users") type User {
    @primary id: Int = 0
    email: Str = ""
    age: Int? = null
}
"#;
        let (prog, env) = parse_and_check(src);
        let schema = schema_from_program(&prog, &env).expect("schema");
        assert_eq!(schema.tables.len(), 1);
        let t = &schema.tables[0];
        assert_eq!(t.name, "users");
        let col_names: Vec<_> = t.columns.iter().map(|c| c.name.as_str()).collect();
        assert!(col_names.contains(&"id"));
        assert!(col_names.contains(&"email"));
        assert!(col_names.contains(&"age"));
        let age = t.columns.iter().find(|c| c.name == "age").unwrap();
        assert!(age.nullable, "age: Int? debe ser nullable");
        let id = t.columns.iter().find(|c| c.name == "id").unwrap();
        assert!(id.is_primary, "id debe ser primary");
    }

    #[test]
    fn schema_from_program_skips_types_without_table_decorator() {
        let src = r#"
type Plain {
    x: Int = 0
}
@table("orders") type Order {
    @primary id: Int = 0
}
"#;
        let (prog, env) = parse_and_check(src);
        let schema = schema_from_program(&prog, &env).expect("schema");
        assert_eq!(schema.tables.len(), 1);
        assert_eq!(schema.tables[0].name, "orders");
    }

    #[test]
    fn schema_from_program_round_trip_diff_against_self_is_empty() {
        let src = r#"
@table("users") type User {
    @primary id: Int = 0
    email: Str = ""
}
"#;
        let (prog, env) = parse_and_check(src);
        let schema = schema_from_program(&prog, &env).expect("schema");
        assert_eq!(diff_schemas(&schema, &schema), vec![]);
    }

    #[test]
    fn schema_from_program_two_versions_yields_add_column() {
        let src_v1 = r#"
@table("users") type User {
    @primary id: Int = 0
    email: Str = ""
}
"#;
        let src_v2 = r#"
@table("users") type User {
    @primary id: Int = 0
    email: Str = ""
    name: Str = ""
}
"#;
        let (p1, e1) = parse_and_check(src_v1);
        let (p2, e2) = parse_and_check(src_v2);
        let v1 = schema_from_program(&p1, &e1).expect("v1");
        let v2 = schema_from_program(&p2, &e2).expect("v2");
        let changes = diff_schemas(&v1, &v2);
        let added: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::AddColumn { table, column } => {
                    Some((table.name.as_str(), column.name.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(added, vec![("users", "name")]);
    }

    // ============================================================
    // v0.10.16 — @db_default("expr") + AlterColumnDefault
    // ============================================================

    fn col_with_default(name: &str, sql_type: &str, default: Option<&str>) -> Column {
        Column {
            name: name.to_string(),
            sql_type: sql_type.to_string(),
            nullable: true,
            default: default.map(|s| s.to_string()),
            is_primary: false,
            renamed_from: None,
        }
    }

    #[test]
    fn create_table_with_default_emits_default_clause() {
        let t = Table {
            name: "events".to_string(),
            columns: vec![
                col("id", "bigint", false, true),
                col_with_default("created_at", "timestamp with time zone", Some("NOW()")),
            ],
            indexes: vec![],
            foreign_keys: vec![],
            renamed_from: None,
            schema: None,
            composite_pk: vec![],
            check_constraints: vec![],
        };
        let sql = changes_to_sql(&[Change::CreateTable(t)]);
        assert!(
            sql.contains("DEFAULT NOW()"),
            "esperaba DEFAULT NOW(): {sql}"
        );
    }

    #[test]
    fn add_column_with_default_emits_default_clause() {
        let change = Change::AddColumn {
            table: TableRef::public("events"),
            column: col_with_default("created_at", "timestamp with time zone", Some("NOW()")),
        };
        let sql = changes_to_sql(&[change]);
        assert!(sql.contains("ADD COLUMN"), "esperaba ADD COLUMN: {sql}");
        assert!(
            sql.contains("DEFAULT NOW()"),
            "esperaba DEFAULT NOW(): {sql}"
        );
    }

    #[test]
    fn alter_column_default_set_emits_set_default() {
        let change = Change::AlterColumnDefault {
            table: TableRef::public("events"),
            column: "created_at".to_string(),
            new_default: Some("NOW()".to_string()),
        };
        let sql = changes_to_sql(&[change]);
        assert!(sql.contains("SET DEFAULT NOW()"), "got: {sql}");
    }

    #[test]
    fn alter_column_default_drop_emits_drop_default() {
        let change = Change::AlterColumnDefault {
            table: TableRef::public("events"),
            column: "created_at".to_string(),
            new_default: None,
        };
        let sql = changes_to_sql(&[change]);
        assert!(sql.contains("DROP DEFAULT"), "got: {sql}");
    }

    #[test]
    fn diff_adds_alter_column_default_when_default_added() {
        let current = Schema {
            tables: vec![Table {
                name: "events".to_string(),
                columns: vec![
                    col("id", "bigint", false, true),
                    col_with_default("created_at", "timestamp with time zone", None),
                ],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let target = Schema {
            tables: vec![Table {
                name: "events".to_string(),
                columns: vec![
                    col("id", "bigint", false, true),
                    col_with_default("created_at", "timestamp with time zone", Some("NOW()")),
                ],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let changes = diff_schemas(&current, &target);
        let set_defaults: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::AlterColumnDefault {
                    new_default: Some(d),
                    column,
                    ..
                } => Some((column.as_str(), d.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(set_defaults, vec![("created_at", "NOW()")]);
    }

    #[test]
    fn diff_adds_alter_column_default_when_default_removed() {
        let current = Schema {
            tables: vec![Table {
                name: "events".to_string(),
                columns: vec![
                    col("id", "bigint", false, true),
                    col_with_default("created_at", "timestamp with time zone", Some("now()")),
                ],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let target = Schema {
            tables: vec![Table {
                name: "events".to_string(),
                columns: vec![
                    col("id", "bigint", false, true),
                    col_with_default("created_at", "timestamp with time zone", None),
                ],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let changes = diff_schemas(&current, &target);
        let drops = changes
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Change::AlterColumnDefault {
                        new_default: None,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(drops, 1);
    }

    #[test]
    fn diff_default_idempotente_case_insensitive() {
        // PG returns `now()` lowercase; the user passed `NOW()`.
        // The diff must be empty (idempotent).
        let current = Schema {
            tables: vec![Table {
                name: "events".to_string(),
                columns: vec![col_with_default(
                    "created_at",
                    "timestamp with time zone",
                    Some("now()"),
                )],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let target = Schema {
            tables: vec![Table {
                name: "events".to_string(),
                columns: vec![col_with_default(
                    "created_at",
                    "timestamp with time zone",
                    Some("NOW()"),
                )],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let changes = diff_schemas(&current, &target);
        assert!(
            changes.is_empty(),
            "esperaba diff vacío (idempotente), got: {changes:?}"
        );
    }

    #[test]
    fn diff_default_idempotente_strip_pg_cast() {
        // PG returns `'public'::text` for Str literals; the user
        // passed `'public'`. The diff must be empty.
        let current = Schema {
            tables: vec![Table {
                name: "settings".to_string(),
                columns: vec![col_with_default("scope", "text", Some("'public'::text"))],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let target = Schema {
            tables: vec![Table {
                name: "settings".to_string(),
                columns: vec![col_with_default("scope", "text", Some("'public'"))],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let changes = diff_schemas(&current, &target);
        assert!(
            changes.is_empty(),
            "esperaba diff vacío (cast strippeado), got: {changes:?}"
        );
    }

    #[test]
    fn normalize_default_for_diff_lowercases_and_trims() {
        assert_eq!(normalize_default_for_diff("  NOW()  "), "now()");
        assert_eq!(normalize_default_for_diff("NOW()"), "now()");
        assert_eq!(normalize_default_for_diff("Now()"), "now()");
    }

    #[test]
    fn normalize_default_for_diff_strips_trailing_cast() {
        assert_eq!(normalize_default_for_diff("'public'::text"), "'public'");
        assert_eq!(normalize_default_for_diff("42::bigint"), "42");
        assert_eq!(normalize_default_for_diff("0::double precision"), "0");
    }

    #[test]
    fn schema_from_program_db_default_with_sql_arg() {
        let src = r#"
@table("events") type Event {
    @primary id: Int = 0
    @db_default("NOW()") created_at: Str = ""
}
"#;
        let (prog, env) = parse_and_check(src);
        let schema = schema_from_program(&prog, &env).expect("schema");
        let t = &schema.tables[0];
        let created = t.columns.iter().find(|c| c.name == "created_at").unwrap();
        assert_eq!(created.default.as_deref(), Some("NOW()"));
    }

    #[test]
    fn schema_from_program_db_default_without_arg_leaves_default_none() {
        let src = r#"
@table("events") type Event {
    @primary id: Int = 0
    @db_default created_at: Str = ""
}
"#;
        let (prog, env) = parse_and_check(src);
        let schema = schema_from_program(&prog, &env).expect("schema");
        let t = &schema.tables[0];
        let created = t.columns.iter().find(|c| c.name == "created_at").unwrap();
        assert!(
            created.default.is_none(),
            "esperaba default = None (marker-only), got: {:?}",
            created.default
        );
    }

    #[test]
    fn db_default_round_trip_no_diff() {
        let src = r#"
@table("events") type Event {
    @primary id: Int = 0
    @db_default("NOW()") created_at: Str = ""
}
"#;
        let (prog, env) = parse_and_check(src);
        let schema = schema_from_program(&prog, &env).expect("schema");
        // Simulate "current" as if PG returned the default with
        // canonical lowercase format.
        let mut current = schema.clone();
        if let Some(t) = current.tables.first_mut() {
            for c in &mut t.columns {
                if c.name == "created_at" {
                    c.default = Some("now()".to_string());
                }
            }
        }
        let changes = diff_schemas(&current, &schema);
        assert!(
            changes.is_empty(),
            "esperaba diff vacío post-round-trip, got: {changes:?}"
        );
    }

    // ============================================================
    // v0.10.17 (10.6.b.1) — UP / DOWN parsing
    // ============================================================

    #[test]
    fn split_up_down_without_markers_is_up_complete() {
        let (up, down) = split_up_down("CREATE TABLE x (id int);\n");
        assert_eq!(up.trim(), "CREATE TABLE x (id int);");
        assert!(down.is_none());
    }

    #[test]
    fn split_up_down_with_both_markers() {
        let raw = "-- UP\nCREATE TABLE x (id int);\n-- DOWN\nDROP TABLE x;\n";
        let (up, down) = split_up_down(raw);
        assert!(up.contains("CREATE TABLE x"));
        assert!(!up.contains("DROP TABLE"));
        let down = down.expect("esperaba DOWN");
        assert!(down.contains("DROP TABLE x"));
        assert!(!down.contains("CREATE TABLE"));
    }

    #[test]
    fn split_up_down_marcador_case_insensitive() {
        let raw = "-- up\nA;\n-- Down\nB;\n";
        let (up, down) = split_up_down(raw);
        assert!(up.contains("A;"));
        assert_eq!(down.as_deref().map(str::trim), Some("B;"));
    }

    #[test]
    fn split_up_down_seccion_down_vacia_es_none() {
        let raw = "-- UP\nA;\n-- DOWN\n   \n  \n";
        let (up, down) = split_up_down(raw);
        assert!(up.contains("A;"));
        assert!(
            down.is_none(),
            "DOWN whitespace-only debería normalizarse a None"
        );
    }

    #[test]
    fn split_up_down_without_up_marker_but_with_down() {
        let raw = "CREATE TABLE x (id int);\n-- DOWN\nDROP TABLE x;\n";
        let (up, down) = split_up_down(raw);
        assert!(up.contains("CREATE TABLE x"));
        assert_eq!(down.as_deref().map(str::trim), Some("DROP TABLE x;"));
    }

    #[test]
    fn split_up_down_marker_with_extra_chars_is_not_marker() {
        // `-- UP foo` is NOT a marker (extra chars)
        let raw = "-- UP foo\nA;\n";
        let (up, down) = split_up_down(raw);
        // Everything is UP (the "-- UP foo" is an innocuous SQL comment)
        assert!(up.contains("-- UP foo"));
        assert!(up.contains("A;"));
        assert!(down.is_none());
    }

    #[test]
    fn read_migrations_dir_preserves_up_down() {
        let tmp = std::env::temp_dir().join(format!("fitz_test_updown_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("20260530120000_test.sql");
        std::fs::write(
            &file,
            "-- UP\nCREATE TABLE foo (id int);\n-- DOWN\nDROP TABLE foo;\n",
        )
        .unwrap();
        let migrations = read_migrations_dir(&tmp).unwrap();
        assert_eq!(migrations.len(), 1);
        let m = &migrations[0];
        match &m.kind {
            MigrationKind::Sql { up_sql, down_sql } => {
                assert!(up_sql.contains("CREATE TABLE foo"));
                assert!(down_sql.as_deref().unwrap().contains("DROP TABLE foo"));
            }
            other => panic!("esperaba MigrationKind::Sql, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_migrations_dir_detects_fitz_files() {
        // v0.10.19 (10.6.d) — `.fitz` and `.sql` interleave by
        // alphabetic order. The `kind` variant indicates the backend.
        let tmp = std::env::temp_dir().join(format!("fitz_test_fitzmig_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let sql_file = tmp.join("20260530100000_create_users.sql");
        let fitz_file = tmp.join("20260530120000_backfill_emails.fitz");
        std::fs::write(&sql_file, "CREATE TABLE x (id int);\n").unwrap();
        std::fs::write(
            &fitz_file,
            "async fn migrate(db: DbConn) -> Result<Null> { return Ok(null) }\n",
        )
        .unwrap();
        let migrations = read_migrations_dir(&tmp).unwrap();
        assert_eq!(migrations.len(), 2);
        // Alphabetic order: 100000 < 120000 → sql first, fitz second.
        assert_eq!(migrations[0].version, "20260530100000");
        assert!(matches!(migrations[0].kind, MigrationKind::Sql { .. }));
        assert!(!migrations[0].is_fitz());
        assert_eq!(migrations[1].version, "20260530120000");
        assert!(matches!(migrations[1].kind, MigrationKind::Fitz { .. }));
        assert!(migrations[1].is_fitz());
        match &migrations[1].kind {
            MigrationKind::Fitz { source, .. } => {
                assert!(source.contains("async fn migrate"));
            }
            _ => unreachable!(),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ============================================================
    // v0.10.17 (10.6.b.2) — Renames via @renamed_from
    // ============================================================

    #[test]
    fn diff_emits_rename_table_when_renamed_from_set() {
        let current = Schema {
            tables: vec![Table {
                name: "old_users".to_string(),
                columns: vec![col("id", "bigint", false, true)],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let target = Schema {
            tables: vec![Table {
                name: "users".to_string(),
                columns: vec![col("id", "bigint", false, true)],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: Some("old_users".to_string()),
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let changes = diff_schemas(&current, &target);
        // Must emit only RenameTable; the rest of the diff is no-op.
        let renames: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::RenameTable {
                    schema: None,
                    old_name,
                    new_name,
                } => Some((old_name.as_str(), new_name.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(renames, vec![("old_users", "users")]);
        // Must not emit CREATE TABLE or DROP TABLE.
        for c in &changes {
            assert!(
                !matches!(c, Change::CreateTable(_) | Change::DropTable(_)),
                "rename no debería emitir CREATE/DROP TABLE; got: {c:?}"
            );
        }
    }

    #[test]
    fn diff_emits_rename_column_when_renamed_from_set() {
        let mut current_users = table_users();
        // Replace `email` with `old_email` in current.
        current_users.columns = vec![
            col("id", "bigint", false, true),
            col("old_email", "text", false, false),
        ];
        let mut target_users = table_users();
        target_users.columns = vec![
            col("id", "bigint", false, true),
            Column {
                name: "email".to_string(),
                sql_type: "text".to_string(),
                nullable: false,
                default: None,
                is_primary: false,
                renamed_from: Some("old_email".to_string()),
            },
        ];
        let current = Schema {
            tables: vec![current_users],
        };
        let target = Schema {
            tables: vec![target_users],
        };
        let changes = diff_schemas(&current, &target);
        let renames: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::RenameColumn {
                    table,
                    old_name,
                    new_name,
                } => Some((table.name.as_str(), old_name.as_str(), new_name.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(renames, vec![("users", "old_email", "email")]);
        // Must not emit ADD/DROP COLUMN.
        for c in &changes {
            assert!(
                !matches!(c, Change::AddColumn { .. } | Change::DropColumn { .. }),
                "rename no debería emitir ADD/DROP COLUMN; got: {c:?}"
            );
        }
    }

    #[test]
    fn rename_table_sql_emits_alter_rename_to() {
        let change = Change::RenameTable {
            schema: None,
            old_name: "old_users".to_string(),
            new_name: "users".to_string(),
        };
        let sql = changes_to_sql(&[change]);
        assert!(
            sql.contains("ALTER TABLE \"old_users\" RENAME TO \"users\""),
            "got: {sql}"
        );
    }

    #[test]
    fn rename_column_sql_emits_alter_rename_column() {
        let change = Change::RenameColumn {
            table: TableRef::public("users"),
            old_name: "name".to_string(),
            new_name: "full_name".to_string(),
        };
        let sql = changes_to_sql(&[change]);
        assert!(
            sql.contains("ALTER TABLE \"users\" RENAME COLUMN \"name\" TO \"full_name\""),
            "got: {sql}"
        );
    }

    #[test]
    fn renamed_from_without_old_in_current_is_silent_noop() {
        // User left the @renamed_from("old") decorator but already
        // applied the migration and old_name no longer exists in
        // current (current is already renamed). The diff must NOT
        // emit a spurious RenameTable.
        let current = Schema {
            tables: vec![Table {
                name: "users".to_string(),
                columns: vec![col("id", "bigint", false, true)],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let target = Schema {
            tables: vec![Table {
                name: "users".to_string(),
                columns: vec![col("id", "bigint", false, true)],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: Some("old_users".to_string()),
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let changes = diff_schemas(&current, &target);
        assert!(
            changes.is_empty(),
            "renamed_from sin match en current debe ser no-op, got: {changes:?}"
        );
    }

    #[test]
    fn rename_table_followed_by_alter_column_safe_order() {
        // current: table "old_x" with col `name`.
        // target: table "x" renamed_from="old_x" with col `name`
        // flagged nullable=true (current was false). The diff must
        // emit RenameTable FIRST, then AlterColumnNullable
        // referencing the NEW name ("x"). If we do not rename
        // first, the ALTER fails because "x" does not exist.
        let current = Schema {
            tables: vec![Table {
                name: "old_x".to_string(),
                columns: vec![col("name", "text", false, false)],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let target = Schema {
            tables: vec![Table {
                name: "x".to_string(),
                columns: vec![col("name", "text", true, false)],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: Some("old_x".to_string()),
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let changes = diff_schemas(&current, &target);
        let rename_idx = changes
            .iter()
            .position(|c| matches!(c, Change::RenameTable { .. }));
        let alter_idx = changes
            .iter()
            .position(|c| matches!(c, Change::AlterColumnNullable { .. }));
        assert!(rename_idx.is_some(), "esperaba RenameTable: {changes:?}");
        assert!(
            alter_idx.is_some(),
            "esperaba AlterColumnNullable: {changes:?}"
        );
        assert!(
            rename_idx.unwrap() < alter_idx.unwrap(),
            "RenameTable debe ir antes que AlterColumnNullable"
        );
        // The AlterColumn must reference the NEW name.
        for c in &changes {
            if let Change::AlterColumnNullable { table, .. } = c {
                assert_eq!(
                    table.name, "x",
                    "AlterColumnNullable debe referenciar el nombre POST-rename"
                );
            }
        }
    }

    #[test]
    fn schema_from_program_field_renamed_from_loads_to_column() {
        let src = r#"
@table("users") type User {
    @primary id: Int = 0
    @renamed_from("name") full_name: Str = ""
}
"#;
        let (prog, env) = parse_and_check(src);
        let schema = schema_from_program(&prog, &env).expect("schema");
        let t = &schema.tables[0];
        let col = t.columns.iter().find(|c| c.name == "full_name").unwrap();
        assert_eq!(col.renamed_from.as_deref(), Some("name"));
    }

    #[test]
    fn schema_from_program_table_renamed_from_loads_to_table() {
        let src = r#"
@table("users") @renamed_from("legacy_users") type User {
    @primary id: Int = 0
}
"#;
        let (prog, env) = parse_and_check(src);
        let schema = schema_from_program(&prog, &env).expect("schema");
        let t = &schema.tables[0];
        assert_eq!(t.renamed_from.as_deref(), Some("legacy_users"));
    }

    // ============================================================
    // v0.10.18 (10.6.c) — check + stamp
    // ============================================================
    //
    // The `db_check_cmd` handler reuses `diff_schemas` (already
    // covered by the diff tests above) + decides the exit code
    // based on `changes.is_empty()`. Here we test the decision.

    #[test]
    fn check_is_green_when_diff_is_empty() {
        let s = Schema {
            tables: vec![table_users()],
        };
        let changes = diff_schemas(&s, &s);
        assert!(
            changes.is_empty(),
            "diff de schemas iguales debe ser vacío (check exit 0)"
        );
    }

    #[test]
    fn check_fails_when_there_is_drift() {
        let current = Schema::default();
        let target = Schema {
            tables: vec![table_users()],
        };
        let changes = diff_schemas(&current, &target);
        assert!(
            !changes.is_empty(),
            "diff entre vacío y populated NO debe ser vacío (check exit 1)"
        );
    }

    // The stamps require a real conn — unit tests without a DB
    // can only verify that the symbols are exported. End-to-end
    // validation lives in CI with a real DB (job `db-postgres`)
    // and in the author's local smoke against the installed
    // Postgres 15.

    #[test]
    fn stamp_version_y_stamp_all_pending_estan_exportadas() {
        // "Symbol exists and is callable" smoke — if someone
        // renames or changes signature, this test fails to compile.
        let _f1: fn(_, _) -> _ = stamp_version;
        let _f2: fn(_, _) -> _ = stamp_all_pending;
    }

    // ============================================================
    // v0.10.20 (10.6.e.1) — history shape
    // ============================================================

    #[test]
    fn history_entry_shape() {
        // Structural smoke: HistoryEntry exposes the 3 fields the
        // CLI uses for format. If someone renames, it breaks here.
        let e = HistoryEntry {
            version: "20260530100000".to_string(),
            applied_at: "2026-05-30 10:00:00+00".to_string(),
            filename: Some("init.sql".to_string()),
        };
        assert_eq!(e.version, "20260530100000");
        assert!(e.applied_at.contains("2026-05-30"));
        assert_eq!(e.filename.as_deref(), Some("init.sql"));
        // None filename for stamped versions without a file in dir.
        let e2 = HistoryEntry {
            version: "19990101000000".to_string(),
            applied_at: "ago".to_string(),
            filename: None,
        };
        assert!(e2.filename.is_none());
    }

    #[test]
    fn history_signature_compila() {
        // The `history` symbol exists and returns the expected type.
        let _f: fn(_, _) -> _ = history;
    }

    // ============================================================
    // v0.10.28 — `fitz db inspect` formatters
    // ============================================================

    /// Mock schema with 2 tables: `users` (PK + UNIQUE partial
    /// index) and `posts` (FK ON DELETE CASCADE). Covers most
    /// cases the formatter has to show.
    fn inspect_schema_fixture() -> Schema {
        Schema {
            tables: vec![
                Table {
                    name: "users".to_string(),
                    columns: vec![
                        col("id", "bigint", false, true),
                        col("email", "text", false, false),
                        col("deleted_at", "timestamp with time zone", true, false),
                    ],
                    indexes: vec![Index {
                        name: "idx_users_email_active".to_string(),
                        columns: vec!["email".to_string()],
                        unique: true,
                        where_clause: Some("(deleted_at IS NULL)".to_string()),
                        using: None,
                        expression: None,
                    }],
                    foreign_keys: vec![],
                    renamed_from: None,
                    schema: None,
                    composite_pk: vec![],
                    check_constraints: vec![],
                },
                Table {
                    name: "posts".to_string(),
                    columns: vec![
                        col("id", "bigint", false, true),
                        col("author_id", "bigint", false, false),
                        col("title", "text", false, false),
                    ],
                    indexes: vec![Index {
                        name: "idx_posts_author".to_string(),
                        columns: vec!["author_id".to_string()],
                        unique: false,
                        where_clause: None,
                        using: None,
                        expression: None,
                    }],
                    foreign_keys: vec![ForeignKey {
                        name: "fk_posts_author".to_string(),
                        column: "author_id".to_string(),
                        references_table: "users".to_string(),
                        references_column: "id".to_string(),
                        on_delete: Some("CASCADE".to_string()),
                        references_schema: None,
                    }],
                    renamed_from: None,
                    schema: None,
                    composite_pk: vec![],
                    check_constraints: vec![],
                },
            ],
        }
    }

    #[test]
    fn format_inspection_text_header_y_tables_completas() {
        let s = inspect_schema_fixture();
        let text = format_inspection_text(&s, None, None);
        // Default schema header.
        assert!(text.starts_with("Schema: public\n"), "text: {text}");
        // Both tables appear with their cols count.
        assert!(
            text.contains("Table: users (3 cols)\n"),
            "missing users header: {text}"
        );
        assert!(
            text.contains("Table: posts (3 cols)\n"),
            "missing posts header: {text}"
        );
        // Cols with type + nullability + PK tag where applicable.
        assert!(text.contains("id"), "missing id col: {text}");
        assert!(text.contains("bigint"), "missing bigint: {text}");
        assert!(text.contains("NOT NULL"), "missing NOT NULL: {text}");
        assert!(text.contains("PK"), "missing PK tag: {text}");
        // Nullable col is annotated as NULL.
        assert!(text.contains("deleted_at"), "missing deleted_at: {text}");
        assert!(text.contains("NULL "), "missing NULL tag: {text}");
        // Unique index with WHERE clause.
        assert!(
            text.contains("idx_users_email_active"),
            "missing index name: {text}"
        );
        assert!(
            text.contains("UNIQUE (email)"),
            "missing UNIQUE label: {text}"
        );
        assert!(
            text.contains("WHERE (deleted_at IS NULL)"),
            "missing WHERE: {text}"
        );
        // Non-unique index comes out without UNIQUE label.
        assert!(
            text.contains("idx_posts_author"),
            "missing non-unique index: {text}"
        );
        // FK with ON DELETE.
        assert!(
            text.contains("fk_posts_author: author_id -> users(id) ON DELETE CASCADE"),
            "missing FK line: {text}"
        );
    }

    #[test]
    fn format_inspection_text_filter_by_table() {
        let s = inspect_schema_fixture();
        let text = format_inspection_text(&s, None, Some("users"));
        assert!(
            text.contains("Table: users"),
            "users debería aparecer: {text}"
        );
        assert!(
            !text.contains("Table: posts"),
            "posts NO debería aparecer cuando filtramos a users: {text}"
        );
    }

    #[test]
    fn format_inspection_text_schema_vacio_mensaje_claro() {
        let s = Schema::default();
        let text = format_inspection_text(&s, None, None);
        assert!(text.starts_with("Schema: public\n"));
        assert!(
            text.contains("no user-defined tables"),
            "mensaje 'sin tablas': {text}"
        );
    }

    #[test]
    fn format_inspection_text_filter_table_inexistente_mensaje_claro() {
        let s = inspect_schema_fixture();
        let text = format_inspection_text(&s, None, Some("nonexistent"));
        assert!(
            text.contains("table `nonexistent` not found"),
            "mensaje table inexistente: {text}"
        );
    }

    #[test]
    fn format_inspection_json_shape_estable() {
        let s = inspect_schema_fixture();
        let json_str = format_inspection_json(&s, None, None).expect("json should serialize");
        // Parse back to validate the shape (do not compare string
        // bit-by-bit because the pretty-printer puts spaces that
        // may change between serde_json versions).
        let v: serde_json::Value = serde_json::from_str(&json_str).expect("valid JSON");
        // Top-level shape.
        assert_eq!(v["schema"], "public");
        let tables = v["tables"].as_array().expect("tables is array");
        assert_eq!(tables.len(), 2);
        // First table (users) — fixture order preserved.
        let users = &tables[0];
        assert_eq!(users["name"], "users");
        assert_eq!(users["schema"], "public");
        assert_eq!(users["primary_key"], serde_json::json!(["id"]));
        // Cols inside users — order preserved, shape stable.
        let cols = users["columns"].as_array().unwrap();
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0]["name"], "id");
        assert_eq!(cols[0]["sql_type"], "bigint");
        assert_eq!(cols[0]["nullable"], false);
        assert_eq!(cols[0]["is_primary"], true);
        assert!(cols[0]["default"].is_null());
        // Index with where_clause appears.
        let idx = &users["indexes"][0];
        assert_eq!(idx["name"], "idx_users_email_active");
        assert_eq!(idx["unique"], true);
        assert_eq!(idx["where_clause"], "(deleted_at IS NULL)");
        // posts FK serializes with on_delete.
        let posts = &tables[1];
        let fk = &posts["foreign_keys"][0];
        assert_eq!(fk["name"], "fk_posts_author");
        assert_eq!(fk["references_table"], "users");
        assert_eq!(fk["on_delete"], "CASCADE");
    }

    #[test]
    fn format_inspection_json_filter_by_table_returns_only_that() {
        let s = inspect_schema_fixture();
        let json_str = format_inspection_json(&s, None, Some("posts")).expect("json");
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let tables = v["tables"].as_array().unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0]["name"], "posts");
    }

    #[test]
    fn format_inspection_text_composite_pk_se_lista_separado() {
        // Composite PK does not mark is_primary inline on columns
        // — the formatter must read `composite_pk` and emit a
        // separate line.
        let schema = Schema {
            tables: vec![Table {
                name: "memberships".to_string(),
                columns: vec![
                    col("user_id", "bigint", false, false),
                    col("group_id", "bigint", false, false),
                    col("role", "text", false, false),
                ],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec!["user_id".to_string(), "group_id".to_string()],
                check_constraints: vec![],
            }],
        };
        let text = format_inspection_text(&schema, None, None);
        assert!(
            text.contains("Primary key: (user_id, group_id)"),
            "composite PK debería listarse: {text}"
        );
        // Must not mark PK inline on cols (because is_primary=false).
        assert!(
            !text.contains("user_id  bigint    NOT NULL  PK"),
            "composite PK no debería tagear PK inline: {text}"
        );
    }

    // ============================================================
    // v0.10.28 — @index(using="...") SQL emission + introspect
    // ============================================================

    #[test]
    fn create_index_with_using_emits_using_method() {
        let table_ref = TableRef {
            schema: None,
            name: "docs".to_string(),
        };
        let change = Change::CreateIndex {
            table: table_ref,
            index: Index {
                name: "idx_docs_body_gin".to_string(),
                columns: vec!["body".to_string()],
                unique: false,
                where_clause: None,
                using: Some("gin".to_string()),
                expression: None,
            },
        };
        let sql = changes_to_sql(&[change]);
        assert!(
            sql.contains("USING gin"),
            "SQL debe contener `USING gin`: {sql}"
        );
        // Expected form: `CREATE INDEX "idx_..." ON "docs" USING gin ("body");`
        assert!(
            sql.contains("ON \"docs\" USING gin (\"body\")"),
            "SQL completo: {sql}"
        );
    }

    #[test]
    fn create_index_without_using_does_not_emit_clause() {
        let table_ref = TableRef {
            schema: None,
            name: "users".to_string(),
        };
        let change = Change::CreateIndex {
            table: table_ref,
            index: Index {
                name: "idx_users_email".to_string(),
                columns: vec!["email".to_string()],
                unique: false,
                where_clause: None,
                using: None,
                expression: None,
            },
        };
        let sql = changes_to_sql(&[change]);
        assert!(
            !sql.contains("USING"),
            "btree default no debería emitir USING: {sql}"
        );
    }

    #[test]
    fn create_index_combina_unique_using_y_where() {
        let table_ref = TableRef {
            schema: None,
            name: "users".to_string(),
        };
        let change = Change::CreateIndex {
            table: table_ref,
            index: Index {
                name: "idx_users_email_active".to_string(),
                columns: vec!["email".to_string()],
                unique: true,
                where_clause: Some("deleted_at IS NULL".to_string()),
                using: Some("btree".to_string()),
                expression: None,
            },
        };
        let sql = changes_to_sql(&[change]);
        // Order: CREATE UNIQUE INDEX ... ON tbl USING ... (cols) WHERE ...
        assert!(sql.starts_with("CREATE UNIQUE INDEX"), "{sql}");
        assert!(sql.contains("USING btree"), "{sql}");
        assert!(sql.contains("WHERE deleted_at IS NULL"), "{sql}");
        // Trailing `;`.
        assert!(sql.trim_end().ends_with(';'), "{sql}");
    }

    #[test]
    fn format_inspection_text_shows_using_when_not_btree() {
        let schema = Schema {
            tables: vec![Table {
                name: "docs".to_string(),
                columns: vec![
                    col("id", "bigint", false, true),
                    col("body", "text", false, false),
                ],
                indexes: vec![Index {
                    name: "idx_docs_body_gin".to_string(),
                    columns: vec!["body".to_string()],
                    unique: false,
                    where_clause: None,
                    using: Some("gin".to_string()),
                    expression: None,
                }],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let text = format_inspection_text(&schema, None, None);
        assert!(text.contains("USING gin"), "inspect text: {text}");
    }

    #[test]
    fn format_inspection_json_includes_using_field() {
        let schema = Schema {
            tables: vec![Table {
                name: "docs".to_string(),
                columns: vec![
                    col("id", "bigint", false, true),
                    col("body", "text", false, false),
                ],
                indexes: vec![Index {
                    name: "idx_docs_body_gin".to_string(),
                    columns: vec!["body".to_string()],
                    unique: false,
                    where_clause: None,
                    using: Some("gin".to_string()),
                    expression: None,
                }],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
                composite_pk: vec![],
                check_constraints: vec![],
            }],
        };
        let json_str = format_inspection_json(&schema, None, None).expect("json");
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let idx = &v["tables"][0]["indexes"][0];
        assert_eq!(idx["using"], "gin");
    }

    #[test]
    fn format_inspection_filter_by_custom_schema() {
        // Tables in non-public schemas must appear when we pass
        // --schema <name>, and NOT when we filter to public.
        let schema = Schema {
            tables: vec![
                Table {
                    name: "users".to_string(),
                    columns: vec![col("id", "bigint", false, true)],
                    indexes: vec![],
                    foreign_keys: vec![],
                    renamed_from: None,
                    schema: None,
                    composite_pk: vec![],
                    check_constraints: vec![],
                },
                Table {
                    name: "tenants_data".to_string(),
                    columns: vec![col("id", "bigint", false, true)],
                    indexes: vec![],
                    foreign_keys: vec![],
                    renamed_from: None,
                    schema: Some("tenant_a".to_string()),
                    composite_pk: vec![],
                    check_constraints: vec![],
                },
            ],
        };
        // Default public: only users.
        let text = format_inspection_text(&schema, None, None);
        assert!(text.contains("Table: users"), "users aparece: {text}");
        assert!(
            !text.contains("Table: tenants_data"),
            "tenants_data NO aparece bajo public: {text}"
        );
        // Explicit filter tenant_a: only tenants_data.
        let text2 = format_inspection_text(&schema, Some("tenant_a"), None);
        assert!(
            text2.contains("Schema: tenant_a"),
            "header tenant_a: {text2}"
        );
        assert!(
            text2.contains("Table: tenants_data"),
            "tenants_data aparece bajo tenant_a: {text2}"
        );
        assert!(
            !text2.contains("Table: users"),
            "users NO aparece bajo tenant_a: {text2}"
        );
    }

    // v0.10.29 — Cross-schema FK emit

    #[test]
    fn add_foreign_key_emit_uses_references_schema_qualified() {
        let fk = ForeignKey {
            name: "tenants_user_id_fkey".to_string(),
            column: "user_id".to_string(),
            references_table: "users".to_string(),
            references_column: "id".to_string(),
            references_schema: Some("public".to_string()),
            on_delete: Some("CASCADE".to_string()),
        };
        let change = Change::AddForeignKey {
            table: TableRef {
                schema: Some("tenants".to_string()),
                name: "memberships".to_string(),
            },
            fk,
        };
        let sql = change_to_sql(&change);
        assert!(
            sql.contains("REFERENCES \"public\".\"users\" (\"id\")"),
            "esperaba REFERENCES qualified, fue: {sql}"
        );
        assert!(
            sql.contains("ALTER TABLE \"tenants\".\"memberships\""),
            "current table qualified: {sql}"
        );
    }

    #[test]
    fn add_foreign_key_emit_without_references_schema_is_backward_compat() {
        let fk = ForeignKey {
            name: "posts_author_id_fkey".to_string(),
            column: "author_id".to_string(),
            references_table: "users".to_string(),
            references_column: "id".to_string(),
            references_schema: None,
            on_delete: None,
        };
        let change = Change::AddForeignKey {
            table: TableRef {
                schema: None,
                name: "posts".to_string(),
            },
            fk,
        };
        let sql = change_to_sql(&change);
        // Without schema qualifier — compat with previous tests.
        assert!(
            sql.contains("REFERENCES \"users\" (\"id\")"),
            "esperaba REFERENCES sin qualifier: {sql}"
        );
        assert!(!sql.contains("\"public\""), "no debe sumar public: {sql}");
    }

    // v0.10.29 — @check_constraint emit a CREATE TABLE

    #[test]
    fn create_table_sql_includes_check_constraints() {
        let t = Table {
            name: "users".to_string(),
            columns: vec![
                col("id", "bigint", false, true),
                col("age", "integer", false, false),
            ],
            indexes: vec![],
            foreign_keys: vec![],
            renamed_from: None,
            schema: None,
            composite_pk: vec![],
            check_constraints: vec![
                CheckConstraint {
                    name: "chk_users_age_positive".to_string(),
                    expr: "age >= 0 AND age <= 150".to_string(),
                },
                CheckConstraint {
                    name: "chk_users_status_valid".to_string(),
                    expr: "status IN ('a', 'p')".to_string(),
                },
            ],
        };
        let sql = create_table_sql(&t);
        assert!(
            sql.contains("CONSTRAINT \"chk_users_age_positive\" CHECK (age >= 0 AND age <= 150)"),
            "esperaba CHECK 1, fue: {sql}"
        );
        assert!(
            sql.contains("CONSTRAINT \"chk_users_status_valid\" CHECK (status IN ('a', 'p'))"),
            "esperaba CHECK 2, fue: {sql}"
        );
    }

    // v0.10.29 — `fitz db inspect --all-schemas`

    fn multi_schema_fixture() -> Schema {
        Schema {
            tables: vec![
                Table {
                    name: "users".to_string(),
                    columns: vec![col("id", "bigint", false, true)],
                    indexes: vec![],
                    foreign_keys: vec![],
                    renamed_from: None,
                    schema: None,
                    composite_pk: vec![],
                    check_constraints: vec![],
                },
                Table {
                    name: "tenants".to_string(),
                    columns: vec![col("id", "bigint", false, true)],
                    indexes: vec![],
                    foreign_keys: vec![],
                    renamed_from: None,
                    schema: Some("tenant_a".to_string()),
                    composite_pk: vec![],
                    check_constraints: vec![],
                },
                Table {
                    name: "audit_log".to_string(),
                    columns: vec![col("id", "bigint", false, true)],
                    indexes: vec![],
                    foreign_keys: vec![],
                    renamed_from: None,
                    schema: Some("ops".to_string()),
                    composite_pk: vec![],
                    check_constraints: vec![],
                },
            ],
        }
    }

    #[test]
    fn format_inspection_text_all_schemas_shows_all_schemas() {
        let s = multi_schema_fixture();
        let text = format_inspection_text_all_schemas(&s, None);
        // Header with detected schemas (alphabetic order).
        assert!(
            text.starts_with("Detected schemas: ops, public, tenant_a\n"),
            "header: {text}"
        );
        // Each schema appears as a section.
        assert!(text.contains("public"), "missing public section: {text}");
        assert!(text.contains("ops"), "missing ops section: {text}");
        assert!(
            text.contains("tenant_a"),
            "missing tenant_a section: {text}"
        );
        // Tables of each schema appear.
        assert!(text.contains("Table: users"), "missing users: {text}");
        assert!(text.contains("Table: tenants"), "missing tenants: {text}");
        assert!(
            text.contains("Table: audit_log"),
            "missing audit_log: {text}"
        );
    }

    #[test]
    fn format_inspection_json_all_schemas_emits_schemas_array() {
        let s = multi_schema_fixture();
        let json_str = format_inspection_json_all_schemas(&s, None).expect("json OK");
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let schemas = v["schemas"].as_array().expect("schemas array");
        // 3 schemas: ops, public, tenant_a (alphabetic order).
        assert_eq!(schemas.len(), 3);
        assert_eq!(schemas[0]["schema"], "ops");
        assert_eq!(schemas[1]["schema"], "public");
        assert_eq!(schemas[2]["schema"], "tenant_a");
        // Each one with its table.
        assert_eq!(schemas[0]["tables"][0]["name"], "audit_log");
        assert_eq!(schemas[1]["tables"][0]["name"], "users");
        assert_eq!(schemas[2]["tables"][0]["name"], "tenants");
    }

    #[test]
    fn format_inspection_text_all_schemas_with_table_filter_applies_global() {
        // `--table users` with `--all-schemas` must show only the
        // tables with that name in ANY schema.
        let s = multi_schema_fixture();
        let text = format_inspection_text_all_schemas(&s, Some("users"));
        assert!(text.contains("Table: users"), "users debe aparecer: {text}");
        assert!(
            !text.contains("Table: tenants"),
            "tenants no debe aparecer: {text}"
        );
        assert!(
            !text.contains("Table: audit_log"),
            "audit_log no debe aparecer: {text}"
        );
    }

    #[test]
    fn format_inspection_text_all_schemas_without_tables_emits_empty_message() {
        let s = Schema { tables: vec![] };
        let text = format_inspection_text_all_schemas(&s, None);
        assert!(
            text.contains("no user-defined tables detected"),
            "esperaba mensaje vacío, fue: {text}"
        );
    }
}
