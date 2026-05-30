//! Fase 10.6 — Migraciones automáticas del ORM.
//!
//! Provee 4 capacidades:
//!
//! 1. **Introspección PG** ([`introspect_schema`]): consulta el
//!    schema actual de la DB (tablas/columnas/índices/FKs) via
//!    `information_schema` + `pg_catalog`. Devuelve un [`Schema`]
//!    que es la "fotografía" del estado real.
//! 2. **Schema desde @table types** ([`schema_from_program`]):
//!    walka el AST del programa Fitz + el `TypeEnv` con las
//!    `TableMetadata` resueltas por el checker → devuelve el
//!    [`Schema`] "esperado" (target).
//! 3. **Diff algorithm** ([`diff_schemas`]): compara `current` vs
//!    `target` y emite una lista de [`Change`] ordenadas (CREATE
//!    TABLE antes de INDEX, FK al final, etc.).
//! 4. **SQL emission** ([`changes_to_sql`]): cada [`Change`] sabe
//!    cómo generarse como statement DDL Postgres.
//!
//! El módulo NO se embebe en el output del codegen (`fitz build`)
//! — vive solo en el binario `fitz` CLI. Los binarios del usuario
//! no necesitan capacidad de introspección/migration; eso es del
//! lado del desarrollador.

use crate::db::{DbConnHandle, DbError, DbResult, PgValue};

// =================================================================
// Modelo del schema
// =================================================================

/// Snapshot del schema de una DB Postgres. Construido tanto via
/// introspección de la DB real como derivado del programa Fitz.
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
    /// v0.10.17 (10.6.b.2) — Si está, hint al diff de que la
    /// tabla SE LLAMABA `renamed_from` antes; emite `ALTER TABLE
    /// "old" RENAME TO "new"` en vez de `DROP + CREATE`. Solo
    /// poblado en el snapshot "target" (desde `schema_from_program`);
    /// `introspect_schema` lo deja en `None`.
    pub renamed_from: Option<String>,
    /// v0.10.21 (10.6.e.3) — Schema Postgres custom. `None` =
    /// `public` (default). Cuando `Some(s)`, el SQL emit usa
    /// `"s"."name"` qualified everywhere.
    pub schema: Option<String>,
}

impl Table {
    /// v0.10.21 — Identidad cross-schema. Dos tables son la
    /// misma sí y solo sí su `(schema, name)` matchea. `None`
    /// schema se trata como `"public"` para comparación canónica
    /// (matchea cualquier introspect que reporte explícitamente
    /// `public`).
    pub fn qualified_id(&self) -> (String, String) {
        let s = self.schema.as_deref().unwrap_or("public");
        (s.to_string(), self.name.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// Tipo SQL canonicalizado (`bigint`/`text`/`boolean`/etc.).
    pub sql_type: String,
    pub nullable: bool,
    /// `DEFAULT <expr>` declarado, sin la palabra `DEFAULT`. `None`
    /// si el column no tiene default.
    pub default: Option<String>,
    pub is_primary: bool,
    /// v0.10.17 (10.6.b.2) — Si está, hint al diff de que el
    /// column SE LLAMABA `renamed_from` antes; emite `ALTER
    /// TABLE ... RENAME COLUMN "old" TO "new"` en vez de
    /// `DROP + ADD`. Solo poblado en target.
    pub renamed_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub name: String,
    pub column: String,
    pub references_table: String,
    pub references_column: String,
    /// `CASCADE` / `SET NULL` / `RESTRICT` / `NO ACTION`. None si
    /// no se declaró (= NO ACTION default de Postgres).
    pub on_delete: Option<String>,
}

// =================================================================
// Introspección PG
// =================================================================

/// Consulta el schema actual de la DB conectada via `conn` y
/// devuelve un [`Schema`] con todas las user-tables (excluye
/// `pg_catalog`, `information_schema`, y la tabla interna
/// `_fitz_migrations` que tracking del migrate).
///
/// Política de exclusión:
/// - Schemas considerados: TODOS los user schemas (excluye
///   `pg_catalog`, `information_schema`, `pg_toast`, `pg_temp_*`).
/// - `table_type = 'BASE TABLE'` (skipea views).
/// - Excluye explícitamente `_fitz_migrations`.
///
/// v0.10.21 (10.6.e.3) — Multi-schema: introspect iterar TODAS
/// las user schemas, no solo `public`. Tables en schemas
/// custom aparecen con `Table.schema = Some("schema_name")`;
/// tables en `public` siguen con `schema = None` por
/// convención (compat con código pre-v0.10.21).
pub async fn introspect_schema(conn: &std::sync::Arc<DbConnHandle>) -> DbResult<Schema> {
    let qualified = list_user_tables_qualified(conn).await?;
    let mut tables = Vec::with_capacity(qualified.len());
    for (schema, name) in &qualified {
        let columns = introspect_columns(conn, schema, name).await?;
        let indexes = introspect_indexes(conn, schema, name).await?;
        let foreign_keys = introspect_foreign_keys(conn, schema, name).await?;
        let schema_for_struct = if schema == "public" {
            None
        } else {
            Some(schema.clone())
        };
        tables.push(Table {
            name: name.clone(),
            columns,
            indexes,
            foreign_keys,
            renamed_from: None,
            schema: schema_for_struct,
        });
    }
    // Orden determinístico: schema primero, después name. `public`
    // tables (schema=None → "public" para sort) primero por orden
    // alfabético del nombre canónico.
    tables.sort_by_key(|a| a.qualified_id());
    Ok(Schema { tables })
}

/// v0.10.21 — Lista user-tables con su schema. Devuelve `(schema,
/// name)` tuples ordenados. Excluye system schemas y
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

/// Lista las user-tables del schema `public`. Excluye system
/// tables + `_fitz_migrations`. Mantenido por compat — preferí
/// `list_user_tables_qualified` para multi-schema.
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

/// Lista columnas de una tabla en orden de declaración
/// (`ordinal_position`). Detecta `is_primary` via cruce con
/// `pg_catalog.pg_index` (la PK no aparece como tal en
/// `information_schema.columns`).
async fn introspect_columns(
    conn: &std::sync::Arc<DbConnHandle>,
    schema: &str,
    table: &str,
) -> DbResult<Vec<Column>> {
    // Cols base: nombre + tipo + nullable + default.
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
            is_primary: false, // se completa más abajo
            renamed_from: None,
        });
    }
    // PK: cruce con pg_index para marcar `is_primary`. La
    // `regclass` cast resuelve `schema.table` correctamente.
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
            // v0.10.16 — PG reporta `column_default = nextval(...)`
            // para `bigserial` PK; el target schema NUNCA lo emite
            // (es implícito del `PRIMARY KEY` con `bigserial`).
            // Limpiamos para evitar falso positivo en el diff.
            if let Some(d) = &c.default {
                if d.starts_with("nextval(") {
                    c.default = None;
                }
            }
        }
    }
    Ok(columns)
}

/// Lista indexes user-defined de una tabla. Excluye los auto-PK
/// (que tienen `indisprimary = true`) y los auto-UNIQUE
/// constraints (que ya están representados a nivel del column en
/// nuestra abstracción, no como Index separado).
async fn introspect_indexes(
    conn: &std::sync::Arc<DbConnHandle>,
    schema: &str,
    table: &str,
) -> DbResult<Vec<Index>> {
    let sql = "SELECT \
                   c.relname AS index_name, \
                   i.indisunique AS is_unique, \
                   array_to_string( \
                       ARRAY( \
                           SELECT a.attname \
                           FROM unnest(i.indkey) WITH ORDINALITY AS k(idx, ord) \
                           JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.idx \
                           ORDER BY k.ord \
                       ), ',') AS column_names \
               FROM pg_index i \
               JOIN pg_class c ON c.oid = i.indexrelid \
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
        indexes.push(Index {
            name,
            columns: cols,
            unique: is_unique,
        });
    }
    indexes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(indexes)
}

/// Lista FKs declarados sobre una tabla. Cada FK = un column local
/// que apunta a `(referenced_table, referenced_column)` con la
/// regla `ON DELETE` declarada.
async fn introspect_foreign_keys(
    conn: &std::sync::Arc<DbConnHandle>,
    schema: &str,
    table: &str,
) -> DbResult<Vec<ForeignKey>> {
    let sql = "SELECT \
                   tc.constraint_name AS name, \
                   kcu.column_name AS local_column, \
                   ccu.table_name AS ref_table, \
                   ccu.column_name AS ref_column, \
                   rc.delete_rule AS on_delete \
               FROM information_schema.table_constraints tc \
               JOIN information_schema.key_column_usage kcu \
                   ON tc.constraint_name = kcu.constraint_name \
                  AND tc.table_schema = kcu.table_schema \
               JOIN information_schema.constraint_column_usage ccu \
                   ON ccu.constraint_name = tc.constraint_name \
                  AND ccu.table_schema = tc.table_schema \
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
        let ref_table = extract_string(row, "ref_table")?;
        let ref_column = extract_string(row, "ref_column")?;
        let on_delete = extract_string_opt(row, "on_delete").and_then(|s| {
            // PG devuelve "NO ACTION" como string literal; lo
            // normalizamos a None para no diferir vs schemas que
            // NO declararon on_delete (mismo default).
            if s.eq_ignore_ascii_case("NO ACTION") {
                None
            } else {
                Some(s)
            }
        });
        fks.push(ForeignKey {
            name,
            column: local_column,
            references_table: ref_table,
            references_column: ref_column,
            on_delete,
        });
    }
    Ok(fks)
}

// =================================================================
// Schema desde @table types del programa Fitz
// =================================================================

/// Construye el [`Schema`] "esperado" walkeando el AST del programa
/// + cruzando con [`crate::types::TypeEnv`] para resolver
///   [`crate::types::TableMetadata`] de cada `@table` type.
///
/// Reglas de mapping:
/// - Cada `@table("name") type T { ... }` → 1 [`Table`] con
///   `name = TableMetadata.sql_name`.
/// - Fields sin decorator → 1 [`Column`] con tipo SQL derivado.
/// - Fields con `@belongs_to(...)` → 1 [`Column`] (FK real) + 1
///   [`ForeignKey`] con referenced_table/column + on_delete.
/// - Fields con `@has_one`/`@has_many` / `BelongsToCompanion` →
///   **skip** (virtuales, no van a la DB).
/// - Fields con `@index` → 1 [`Index`] (single-column, name
///   estándar `<table>_<col>_idx`).
/// - Fields con `@unique` → 1 [`Index`] con `unique=true` (PG
///   crea unique constraint backed by unique index).
/// - Field con `@primary` → marca [`Column.is_primary = true`].
///   El default `Int = 0` se traduce a `bigserial` (PG auto-increment).
///
/// Tipos Fitz → SQL:
/// - `Int` → `bigint` (o `bigserial` si `@primary`)
/// - `Float` → `double precision`
/// - `Str` → `text`
/// - `Bool` → `boolean`
/// - `List<T>` → `<T>[]` (array Postgres)
/// - `Map<Str, _>` → `jsonb`
/// - `Nullable<T>` → mismo SQL type del inner, `nullable = true`
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
    // v0.10.21 — Orden por (schema, name) canónico para que el
    // diff sea determinístico cross-schema (`public.users` ANTES
    // que `analytics.events` lex).
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
        // Skip virtuales (has_one/has_many/companion).
        if meta.is_virtual_field(&f.name) {
            continue;
        }
        let col_meta = meta.columns.get(&f.name);
        let is_primary = meta.primary_field.as_deref() == Some(f.name.as_str());

        // Tipo Fitz → resolver via TypeExpr → SQL.
        let (fitz_inner_ty, nullable) = unwrap_nullable_typeexpr(&f.type_);
        let sql_type = fitz_typeexpr_to_sql_type(&fitz_inner_ty, is_primary, col_meta)?;
        let sql_name = col_meta
            .and_then(|c| c.sql_name.clone())
            .unwrap_or_else(|| f.name.clone());

        // Default SQL: el ORM emite el CREATE TABLE sin DEFAULTS
        // explícitos (los defaults se aplican client-side al
        // construir la instancia). EXCEPCIONES:
        // - `@primary Int = 0` → `bigserial` ya implica default
        //   nextval; NO emitimos DEFAULT extra.
        // - `@db_default("<sql>")` (v0.10.16) — el user pasa la
        //   expresión SQL explícita y el diff la emite en
        //   `CREATE TABLE` / `ADD COLUMN` (e.g. `DEFAULT NOW()`).
        // - `@db_default` sin args sigue siendo marker-only (skip
        //   INSERT, sin default específico en la migration).
        let default = col_meta.and_then(|c| c.db_default_sql.clone());

        columns.push(Column {
            name: sql_name.clone(),
            sql_type,
            nullable,
            default,
            is_primary,
            renamed_from: col_meta.and_then(|c| c.renamed_from.clone()),
        });

        // Indexes per-field.
        if let Some(cm) = col_meta {
            if cm.unique {
                indexes.push(Index {
                    name: format!("{}_{}_key", table_name, sql_name),
                    columns: vec![sql_name.clone()],
                    unique: true,
                });
            }
            if cm.indexed {
                indexes.push(Index {
                    name: format!("{}_{}_idx", table_name, sql_name),
                    columns: vec![sql_name.clone()],
                    unique: false,
                });
            }
        }

        // FK desde @belongs_to.
        if let Some(rel) = meta.relations.get(&f.name) {
            if rel.kind == crate::types::RelationKind::BelongsTo {
                // Resolver target table name vía type_env.
                let target_table = type_env
                    .lookup(&rel.target_type)
                    .and_then(|tid| type_env.table_metadata(tid))
                    .map(|m| m.sql_name.clone())
                    .ok_or_else(|| {
                        format!(
                            "@belongs_to en `{}.{}` apunta a `{}` que no es @table",
                            type_name, f.name, rel.target_type
                        )
                    })?;
                // PK column del target (default "id"). En PG las
                // PK columns no tienen prefijo de tabla.
                let target_pk_field = type_env
                    .lookup(&rel.target_type)
                    .and_then(|tid| type_env.table_metadata(tid))
                    .and_then(|m| m.primary_field.clone())
                    .unwrap_or_else(|| "id".to_string());
                let target_pk_sql = type_env
                    .lookup(&rel.target_type)
                    .and_then(|tid| type_env.table_metadata(tid))
                    .and_then(|m| m.columns.get(&target_pk_field))
                    .and_then(|c| c.sql_name.clone())
                    .unwrap_or(target_pk_field);
                // Convención: nombre del constraint usa el
                // schema "<table>_<col>_fkey" que PG usa por
                // default cuando inline en CREATE TABLE.
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
                    on_delete,
                });
            }
        }
    }

    // Orden determinístico de indexes y FKs (para diffs estables).
    indexes.sort_by(|a, b| a.name.cmp(&b.name));
    foreign_keys.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Table {
        name: table_name,
        columns,
        indexes,
        foreign_keys,
        renamed_from: meta.renamed_from.clone(),
        schema: meta.schema.clone(),
    })
}

/// Si el TypeExpr es `T?` (Nullable), devuelve (T, true). Si no,
/// (T, false).
fn unwrap_nullable_typeexpr(t: &crate::ast::TypeExpr) -> (crate::ast::TypeExpr, bool) {
    match t {
        crate::ast::TypeExpr::Nullable(inner) => ((**inner).clone(), true),
        other => (other.clone(), false),
    }
}

/// Convierte un TypeExpr Fitz a su SQL type Postgres. Aplica
/// override de `col_meta.sql_type` si está declarado (escape hatch
/// para tipos no estándar como `uuid`/`numeric(10,2)`/etc.).
fn fitz_typeexpr_to_sql_type(
    t: &crate::ast::TypeExpr,
    is_primary: bool,
    col_meta: Option<&crate::types::ColumnMetadata>,
) -> Result<String, String> {
    // Override declarado tiene prioridad.
    if let Some(cm) = col_meta {
        if let Some(sql) = &cm.sql_type {
            return Ok(sql.clone());
        }
    }
    let head = t.head_name();
    match head {
        "Int" => {
            // @primary Int → bigserial (auto-increment Postgres).
            // Resto → bigint.
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
        "List" => {
            // List<T> → T[] Postgres array.
            let inner = match t {
                crate::ast::TypeExpr::Generic { name: _, args } if !args.is_empty() => {
                    args[0].clone()
                }
                _ => {
                    return Err(format!(
                        "List sin parámetro de tipo: `{}` (esperado `List<Int>`/`List<Str>`/etc.)",
                        t.display_name(),
                    ));
                }
            };
            let (inner_unwrapped, _inner_nullable) = unwrap_nullable_typeexpr(&inner);
            let inner_sql = fitz_typeexpr_to_sql_type(&inner_unwrapped, false, None)?;
            Ok(format!("{}[]", inner_sql))
        }
        "Map" => {
            // Map<Str, _> → jsonb. Otros key types no soportados.
            Ok("jsonb".to_string())
        }
        _ => Err(format!(
            "tipo Fitz `{}` no tiene mapping SQL automático \
             (usá @column(sql_type=\"...\") para forzar)",
            t.display_name()
        )),
    }
}

// =================================================================
// Diff algorithm: current vs target → Vec<Change>
// =================================================================

/// Una operación DDL para llevar el schema `current` al `target`.
/// El diff las emite en orden seguro para ejecución secuencial:
/// CREATE TABLE → ADD/DROP/ALTER COLUMN → CREATE/DROP INDEX →
/// DROP FK → ADD FK → DROP TABLE.
/// v0.10.21 (10.6.e.3) — Referencia a una tabla con schema
/// optativo. `schema = None` significa `public` (default Postgres).
/// El `quote_qualified` emite `"schema"."name"` o `"name"` según.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

impl TableRef {
    /// Constructor para tables en `public` (compat v0.10.0-v0.10.20).
    pub fn public(name: impl Into<String>) -> Self {
        Self {
            schema: None,
            name: name.into(),
        }
    }

    /// Constructor para tables en schema custom.
    pub fn qualified(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: Some(schema.into()),
            name: name.into(),
        }
    }

    /// Construye TableRef desde un `Table` (read schema field).
    pub fn from_table(t: &Table) -> Self {
        Self {
            schema: t.schema.clone(),
            name: t.name.clone(),
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
    /// v0.10.16 — Cambio del DEFAULT de un column existente.
    /// `new_default = Some(sql)` → `SET DEFAULT <sql>`; `None` →
    /// `DROP DEFAULT`. La normalización para el diff es
    /// case-insensitive sobre función calls SQL (`now()` matchea
    /// `NOW()` y `Now()`).
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
        /// Schema del index (= schema de la tabla a la que pertenece).
        /// `None` = public. Postgres `DROP INDEX` quotes con schema
        /// si non-public.
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
    /// v0.10.17 (10.6.b.2) — Rename de tabla preservando datos.
    /// Emitido cuando el target Table tiene `renamed_from = Some(old)`
    /// y existe en current con ese nombre. Va PRIMERO en el output
    /// (antes de cualquier ALTER de columns) para que las acciones
    /// siguientes operen sobre el nombre nuevo. El rename ocurre
    /// dentro del mismo schema (no se soporta cross-schema rename
    /// en MVP).
    RenameTable {
        schema: Option<String>,
        old_name: String,
        new_name: String,
    },
    /// v0.10.21 (10.6.e.3) — `CREATE SCHEMA IF NOT EXISTS "name"`.
    /// Emitido cuando el target referencia un schema custom que NO
    /// existe en current. Va PRIMERO en el output (antes de
    /// CREATE TABLE en ese schema), idempotente vía `IF NOT EXISTS`.
    CreateSchema {
        name: String,
    },
    /// v0.10.17 (10.6.b.2) — Rename de column preservando datos.
    /// Emitido cuando un target Column tiene `renamed_from = Some(old)`
    /// y existe en current.columns con ese nombre. Va inmediatamente
    /// después de RenameTable y antes de ADD/DROP COLUMN.
    RenameColumn {
        table: TableRef,
        old_name: String,
        new_name: String,
    },
}

/// Compara `current` (snapshot via [`introspect_schema`]) con
/// `target` (snapshot via [`schema_from_program`]) y emite la
/// lista ordenada de [`Change`] necesaria para sincronizar.
///
/// Garantías:
/// - Idempotente: `diff(target, target) == []` (sin cambios).
/// - Determinístico: el output es estable entre corridas
///   (categorías ordenadas + items dentro de cada categoría
///   sorted alfabéticamente).
/// - Seguro para ejecución secuencial: el orden permite aplicar
///   los Changes sin errores intermedios (CREATE TABLE antes
///   que ALTER, DROP FK antes que DROP TABLE/COLUMN).
///
/// Limitaciones MVP:
/// - **Renames detectados solo via `@renamed_from(...)`** (v0.10.17).
///   Sin el decorator, un rename de `name` → `full_name` se ve como
///   `DROP COLUMN name` + `ADD COLUMN full_name`, perdiendo los datos.
///   El user marca explícitamente el rename con `@renamed_from("old")`
///   sobre el field/type para que el diff emita `RENAME COLUMN`/
///   `RENAME TABLE` preservando los datos.
/// - **`AlterColumnType` directo sin USING** — si los datos NO
///   convierten al nuevo tipo, Postgres falla. El user debe
///   editar la migration para agregar `USING (col::new_type)` o
///   data migration script.
pub fn diff_schemas(current: &Schema, target: &Schema) -> Vec<Change> {
    let mut changes = Vec::new();

    // --- 0. RENAMES (v0.10.17, 10.6.b.2). Pre-procesa los hints
    // `renamed_from` del target: emite RenameTable + RenameColumn
    // PRIMERO, y construye un `current_renamed` con los nombres ya
    // actualizados para que el resto del diff (CREATE/DROP/ALTER)
    // compare contra el estado post-rename — sin esto, una table
    // renombrada se vería como DROP+CREATE perdiendo datos.
    let current = apply_renames_from_target(current, target, &mut changes);
    let current = &current;

    // v0.10.21 — Identidad de tabla = (schema, name). Las tables
    // del current se comparan por qualified_id contra las del
    // target, no por name plano.
    let current_ids: std::collections::HashSet<(String, String)> =
        current.tables.iter().map(|t| t.qualified_id()).collect();
    let target_ids: std::collections::HashSet<(String, String)> =
        target.tables.iter().map(|t| t.qualified_id()).collect();

    // --- 0.5. CREATE SCHEMA IF NOT EXISTS para schemas custom del
    // target que no existen en current. Va PRIMERO (antes de CREATE
    // TABLE en ese schema). Idempotente.
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

    // --- 1. CREATE TABLE (target tables no presentes en current).
    let mut create_tables: Vec<&Table> = target
        .tables
        .iter()
        .filter(|t| !current_ids.contains(&t.qualified_id()))
        .collect();
    create_tables.sort_by_key(|a| a.qualified_id());
    for t in &create_tables {
        changes.push(Change::CreateTable((*t).clone()));
    }

    // --- 2. ALTER de tablas en AMBOS (cross-schema match por
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

    // 2.1. Per-tabla: columns + indexes + FKs.
    for (current_t, target_t) in &tables_to_alter {
        diff_columns(current_t, target_t, &mut changes);
        diff_indexes(current_t, target_t, &mut changes);
    }

    // --- 3. DROP FK (antes de DROP TABLE / DROP COLUMN).
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

    // --- 4. ADD FK (después de tener todas las tables y cols).
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

    // --- 5. DROP TABLE (current tables no en target).
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

/// v0.10.17 (10.6.b.2) — Detecta los hints `renamed_from` del
/// `target` schema y:
/// 1. Emite los `Change::RenameTable` / `Change::RenameColumn`
///    al frente de `changes` (orden: tables primero, después
///    columns).
/// 2. Devuelve una versión renombrada de `current` para que el
///    resto del diff compare por nombres post-rename.
///
/// **Política**:
/// - Rename activo solo si: target tiene `renamed_from = Some(old)`
///   Y current.tables contiene una tabla con ese `old` name Y
///   current.tables NO contiene una tabla con el nombre target
///   (evita colisión accidental).
/// - Renames de column adentro de una tabla renombrada usan el
///   nombre target (post-rename) en el `RenameColumn.table`.
/// - Hints sin match en `current` (ej: usuario dejó el decorator
///   tras aplicar la migration) se ignoran silenciosamente — son
///   no-op, no error. El user puede limpiarlos cuando quiera.
fn apply_renames_from_target(
    current: &Schema,
    target: &Schema,
    changes: &mut Vec<Change>,
) -> Schema {
    // v0.10.21 — Rename ahora es schema-aware. La identidad de
    // tabla es `(schema, name)`. Renames cross-schema NO se
    // soportan en MVP — el `renamed_from` se interpreta dentro
    // del schema actual de la table target.
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

    // 2. Construir current renombrado (a nivel tabla).
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

    // 3. Detectar renames de column dentro de tablas que existen
    // en ambos schemas (post-rename, match por qualified_id).
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
            // Renombrar in-place en current_t para que el resto
            // del diff vea el nombre nuevo.
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

    // Drop columns no en target.
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

    // Add columns no en current.
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

    // Alter columns en AMBOS con diferencias.
    for tc in &target.columns {
        if let Some(cc) = current.columns.iter().find(|c| c.name == tc.name) {
            // Type change. Pero ignoramos `bigserial` vs `bigint`
            // como mismo type — PG reporta `bigint` para columns
            // tipo `bigserial` (la auto-increment está reflejada
            // en `column_default = nextval(...)`).
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

/// v0.10.16 — Normaliza una expresión SQL de default para
/// comparación en el diff. El objetivo es ser permisivo con
/// variaciones cosméticas que no cambian semántica:
///
/// - Lowercase de la expresión completa (PG normaliza `NOW()` →
///   `now()` en `column_default`).
/// - Strip de casts redundantes que PG agrega automáticamente
///   (`'public'::text` → `'public'`, `42::bigint` → `42`).
/// - Trim de whitespace.
///
/// NO intenta evaluar expresiones equivalentes (`now()` vs
/// `CURRENT_TIMESTAMP` son ambos válidos para `timestamptz` pero
/// se ven distintos — los tratamos como distintos).
fn normalize_default_for_diff(s: &str) -> String {
    let lower = s.trim().to_lowercase();
    // Strip casts `::tipo` cuando el tipo es alfanumérico simple.
    // Conservador: solo strip al final, no en medio (no rompemos
    // expresiones complejas como `(a::int) + (b::int)`).
    let no_cast = strip_trailing_pg_cast(&lower);
    no_cast.to_string()
}

fn strip_trailing_pg_cast(s: &str) -> &str {
    // Busca el último `::` y si lo que sigue es alfanumérico
    // simple (`text`, `bigint`, `timestamptz`, etc.), lo recorta.
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
    let current_idx_names: std::collections::HashSet<&str> =
        current.indexes.iter().map(|i| i.name.as_str()).collect();
    let target_idx_names: std::collections::HashSet<&str> =
        target.indexes.iter().map(|i| i.name.as_str()).collect();
    let table_ref = TableRef::from_table(target);

    // Drop indexes no en target.
    let mut to_drop: Vec<&Index> = current
        .indexes
        .iter()
        .filter(|i| !target_idx_names.contains(i.name.as_str()))
        .collect();
    to_drop.sort_by(|a, b| a.name.cmp(&b.name));
    for i in to_drop {
        changes.push(Change::DropIndex {
            schema: current.schema.clone(),
            index_name: i.name.clone(),
        });
    }

    // Create indexes no en current.
    let mut to_add: Vec<&Index> = target
        .indexes
        .iter()
        .filter(|i| !current_idx_names.contains(i.name.as_str()))
        .collect();
    to_add.sort_by(|a, b| a.name.cmp(&b.name));
    for i in to_add {
        changes.push(Change::CreateIndex {
            table: table_ref.clone(),
            index: i.clone(),
        });
    }
}

/// Para evitar falsos positivos del diff: `bigserial` y `bigint`
/// son equivalentes en la DB (la auto-increment es metadata
/// separada via default `nextval(...)`). Cuando el AST declara
/// `@primary Int = 0` el target sql_type es `bigserial`, pero la
/// DB lo reporta como `bigint`. Sin esta normalización, el diff
/// dispararía `AlterColumnType` espurio en cada corrida.
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

/// Convierte una lista de [`Change`] a SQL DDL Postgres. Cada
/// statement termina en `;\n\n` para legibilidad cuando se escriben
/// a archivos `.sql`. La salida es ejecutable directo via
/// `psql -f` o `db.exec(...)`.
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
            format!(
                "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
                quote_qualified(table),
                quote_ident(column),
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
            let cols: Vec<String> = index.columns.iter().map(|c| quote_ident(c)).collect();
            format!(
                "CREATE {}INDEX {} ON {} ({});",
                unique,
                quote_ident(&index.name),
                quote_qualified(table),
                cols.join(", "),
            )
        }
        Change::DropIndex { schema, index_name } => {
            // Postgres DROP INDEX requiere el schema si non-public
            // (sino busca en search_path y puede no encontrarlo).
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
            // v0.10.21 — references_table puede ser schema-qualified
            // si el target ORM declaró su `@table("schema.name")`.
            // Hoy lo guardamos como bare name + asumimos same-schema;
            // cross-schema FK queda como deuda menor.
            format!(
                "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({}){};",
                quote_qualified(table),
                quote_ident(&fk.name),
                quote_ident(&fk.column),
                quote_ident(&fk.references_table),
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
            // El nuevo name va SIN schema prefix (RENAME TO espera
            // solo el name; el schema se preserva).
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
    }
}

fn create_table_sql(t: &Table) -> String {
    let mut lines = Vec::with_capacity(t.columns.len() + 1);
    for c in &t.columns {
        let nullable = if c.is_primary {
            // PRIMARY KEY implica NOT NULL — no agregar redundante.
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
    // FKs NO inline acá — el diff las emite como ADD CONSTRAINT
    // separados para destrabar ciclos entre tablas nuevas.
    format!(
        "CREATE TABLE {} (\n{}\n);",
        quote_qualified(&TableRef::from_table(t)),
        lines.join(",\n"),
    )
}

/// Quote PG-style un identificador. Solo encerramos en `"` los
/// nombres que tienen chars no-ASCII-alphanumeric o que matchean
/// palabras reservadas. Para simplicidad MVP: SIEMPRE quoteamos —
/// trade-off: SQL más verboso, pero 100% seguro contra reserved
/// words y case sensitivity de PG.
fn quote_ident(name: &str) -> String {
    // Escapar `"` adentro del nombre (raro pero posible).
    let escaped = name.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

/// v0.10.21 (10.6.e.3) — Quote PG-style con schema qualifier
/// opcional. `schema = None` → `"name"`; `schema = Some(s)` →
/// `"s"."name"`. El SQL emit de Change usa siempre este helper
/// para que las tables en schemas custom funcionen.
fn quote_qualified(t: &TableRef) -> String {
    match &t.schema {
        Some(s) => format!("{}.{}", quote_ident(s), quote_ident(&t.name)),
        None => quote_ident(&t.name),
    }
}

// =================================================================
// Tracking + ejecutor (apply_pending_migrations)
// =================================================================

/// Nombre de la tabla interna donde Fitz trackea qué migrations
/// corrieron. Idempotente — re-correr `migrate` no aplica las ya
/// aplicadas.
const TRACKING_TABLE: &str = "_fitz_migrations";

/// Una migration encontrada en el directorio `migrations/`. La
/// `version` es el prefijo del filename (típicamente timestamp
/// `YYYYMMDDHHMMSS_descripcion.sql`). Postgres ordena por
/// `version` lexicográfico = orden cronológico real.
///
/// v0.10.17 (10.6.b.1) — el SQL se split en `up_sql` y `down_sql`
/// vía markers `-- UP` / `-- DOWN`. Backward-compat: archivos sin
/// marcadores → todo es UP, `down_sql = None` (no soporta rollback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationFile {
    pub version: String,
    pub filename: String,
    pub kind: MigrationKind,
}

/// v0.10.19 (10.6.d) — Tipo de migration según su backend.
///
/// - `Sql`: archivo `.sql` con SQL crudo splittable en `-- UP` /
///   `-- DOWN` (mantiene la semántica de v0.10.17).
/// - `Fitz` (v0.10.19): archivo `.fitz` que declara `async fn
///   migrate(db: DbConn) -> Result<Null>` y opcionalmente
///   `async fn rollback(db: DbConn) -> Result<Null>`. El runner
///   parsea + invoca la fn adentro de tx con `db` bindeado.
///   Habilita transforms con lógica que SQL crudo no expresa
///   (parseo JSON viejo → cols nuevas, back-fills condicionales,
///   etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationKind {
    Sql {
        /// SQL a ejecutar en `migrate` (forward). Siempre presente.
        up_sql: String,
        /// SQL a ejecutar en `rollback`. `None` si la migration
        /// no declaró sección `-- DOWN`.
        down_sql: Option<String>,
    },
    Fitz {
        /// Path absoluto al archivo `.fitz`. Lo guardamos por si
        /// el runner necesita base_dir para resolver imports
        /// relativos del module loader.
        path: std::path::PathBuf,
        /// Source completo del archivo. Cacheado en read para
        /// evitar I/O extra durante el dispatch.
        source: String,
    },
}

impl MigrationFile {
    /// `true` si la migration es un `.fitz` script con lógica
    /// (necesita el runner del lenguaje, no `db.exec` directo).
    pub fn is_fitz(&self) -> bool {
        matches!(self.kind, MigrationKind::Fitz { .. })
    }
}

/// Estado de una migration: aplicada en la DB, o pendiente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationStatus {
    Applied,
    Pending,
}

/// Crea la tabla de tracking si no existe. Idempotente.
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

/// Lista las versiones ya aplicadas, en orden cronológico.
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

/// Lee migrations files del directorio `dir`. Filtra `*.sql`,
/// extrae version del filename (prefix hasta `_` o el filename
/// entero sin extensión). Orden lexicográfico = cronológico si
/// usás timestamps `YYYYMMDDHHMMSS_*`.
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
        // v0.10.19 — aceptamos `.sql` Y `.fitz`. El runner despacha
        // por `MigrationKind` después.
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

/// v0.10.17 (10.6.b.1) — Split del contenido `.sql` en secciones
/// `-- UP` y `-- DOWN`. Reglas:
///
/// - Si NO hay marcador `-- UP` ni `-- DOWN` → todo el contenido
///   es UP, `down_sql = None`. Backward-compat con migrations
///   v0.10.16 sin secciones explícitas.
/// - Si hay `-- UP` (con o sin `-- DOWN`) → UP es el rango entre
///   `-- UP` y `-- DOWN` (o EOF si no hay DOWN).
/// - Si hay `-- DOWN` sin `-- UP` previo → todo lo previo es UP,
///   lo siguiente es DOWN. (Caso: user pone el marker DOWN
///   sin marker UP explícito.)
/// - Marcador case-insensitive en una línea propia (whitespace
///   permitido antes y después, sin chars adicionales adentro).
///   `-- up`, `-- Up`, `--  UP` matchean; `-- UP foo` no.
/// - Si `down_sql` queda como string vacío/whitespace → `None`
///   (sección DOWN declarada pero vacía equivale a no declararla).
fn split_up_down(raw: &str) -> (String, Option<String>) {
    let mut up_lines: Vec<&str> = Vec::new();
    let mut down_lines: Vec<&str> = Vec::new();
    // Modos: 0 = pre-marker (cuenta como UP), 1 = en UP, 2 = en DOWN.
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
    // Backward-compat sanity: si NO había marker UP ni DOWN,
    // el up_sql es el contenido entero (modo 0 todo el archivo).
    let _ = saw_up_marker;
    (up_sql, down_sql)
}

/// Aplica una migration adentro de una transaction. Si el SQL
/// falla, la tx se revierte y NO se trackea en `_fitz_migrations`.
/// Si OK, se inserta en tracking + COMMIT atomic.
///
/// **Garantía atomicidad**: la migration entera (todos sus
/// statements + el insert en tracking) corre en 1 BEGIN/COMMIT.
/// O todo persiste, o nada — sin estados intermedios.
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
            // Postgres `simple_query` permite múltiples statements
            // separados por `;` en una sola llamada (a diferencia
            // del Extended Query Protocol que es 1 stmt por
            // request). Usamos `query` que internamente despacha
            // a simple_query cuando args is_empty.
            tx.query(&sql, &[]).await?;
            // Track la version. INSERT en la misma tx — atomic.
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

/// v0.10.19 (10.6.d) — Helper para que el caller (main.rs) marque
/// una `.fitz` migration como aplicada DESPUÉS de que el runner
/// del lenguaje haya ejecutado `async fn migrate(db)` exitosamente.
/// La tx de la fn del usuario ya commiteó (vía `db.transaction`
/// adentro de la invocación), así que acá solo insertamos en
/// `_fitz_migrations` como acto separado.
///
/// **Nota de atomicidad**: `.fitz` migrations NO son atómicas
/// con respecto al tracking — si el `INSERT INTO _fitz_migrations`
/// falla después de que el script ya commiteó, queda en estado
/// "aplicada pero no trackeada". `migrate` la re-aplicaría en la
/// próxima corrida. Es responsabilidad del script ser idempotente
/// (paralelo a `CREATE TABLE IF NOT EXISTS` en `.sql`).
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

/// v0.10.19 (10.6.d) — Borra el tracking de una `.fitz` migration
/// revertida exitosamente. Paralelo a `track_fitz_migration_applied`
/// para el path rollback.
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

/// v0.10.17 (10.6.b.1) — Revierte una migration: ejecuta su
/// sección `-- DOWN` adentro de tx + borra el registro de
/// `_fitz_migrations`. Atomic: o todo persiste o nada.
///
/// **Errores**:
/// - Si `down_sql` es `None` (migration sin `-- DOWN`): retorna
///   `DbError::Protocol` con mensaje claro citando el filename.
///   El caller debe abortar el rollback entero — sin DOWN no hay
///   forma segura de revertir.
/// - Si el SQL del DOWN falla: la tx se revierte (no se borra el
///   registro de tracking) — el rollback parcial NO se persiste.
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

/// v0.10.17 (10.6.b.1) — Rollback de las últimas `n` migrations
/// aplicadas. Lee `_fitz_migrations` ordenado por `applied_at DESC`,
/// cruza con los archivos del dir, y aplica `revert_migration` en
/// ese orden (más reciente primero).
///
/// Retorna las versiones revertidas (vacío si no había nada
/// aplicado o `n=0`).
///
/// **Errores fatales** (abortan ANTES de tocar la DB):
/// - Alguna migration applied NO tiene file en el dir (archivo
///   fue borrado tras applicar): no podemos revertir sin el
///   `-- DOWN`. Error específico.
/// - Alguna migration target del rollback NO tiene `-- DOWN`:
///   error específico citando filename.
///
/// **Comportamiento incremental**: si la revert N falla
/// runtime, las anteriores YA persistieron (cada `revert_migration`
/// es atómico individual). Para rollback "todo o nada" sobre N
/// migrations habría que envolver en outer transaction — deuda
/// menor (raro tener N>1 en rollback típico).
pub async fn rollback_n(
    conn: &std::sync::Arc<DbConnHandle>,
    migrations: &[MigrationFile],
    n: usize,
) -> DbResult<Vec<String>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    ensure_tracking_table(conn).await?;
    // Versiones aplicadas, ordenadas por applied_at DESC (más
    // reciente primero). Lectura directa de _fitz_migrations.
    let applied_desc = applied_versions_desc(conn).await?;
    if applied_desc.is_empty() {
        return Ok(Vec::new());
    }
    let target_versions: Vec<&String> = applied_desc.iter().take(n).collect();
    // Buscar cada version en el dir de migrations.
    let by_version: std::collections::HashMap<&str, &MigrationFile> =
        migrations.iter().map(|m| (m.version.as_str(), m)).collect();
    // Pre-flight: validar que TODAS las versions target tienen
    // file + DOWN, ANTES de empezar a revertir. Falla fast.
    for v in &target_versions {
        let m = by_version.get(v.as_str()).ok_or_else(|| {
            DbError::Protocol(format!(
                "rollback: la version `{v}` está aplicada en la DB pero \
                 NO hay archivo en el dir de migrations. Restaurá el \
                 archivo o stampealá manualmente con SQL."
            ))
        })?;
        // v0.10.19 — `.fitz` migrations en el path SQL-rollback son
        // un error fast: el CLI las debe despachar al runner del
        // lenguaje antes de invocar `rollback_n`. Por defensa,
        // rechazamos acá.
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
        let m = by_version.get(v.as_str()).expect("pre-flight validó");
        revert_migration(conn, m).await?;
        reverted.push(v.clone());
    }
    Ok(reverted)
}

/// v0.10.20 (10.6.e.1) — Entrada del audit log de migraciones
/// aplicadas. Lo emite `fitz db history`. `applied_at` viene como
/// string ISO 8601 desde Postgres (el caller decide el display).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub version: String,
    pub applied_at: String,
    /// Filename del archivo en el dir, si existe. `None` si la
    /// version está aplicada pero el archivo fue removido (caso
    /// típico: `db stamp <legacy_version>` sin archivo, o squash
    /// que movió el viejo a `migrations/squashed/`).
    pub filename: Option<String>,
}

/// v0.10.20 (10.6.e.1) — Audit log de migraciones aplicadas.
/// Devuelve las entries ordenadas por `applied_at DESC` (más
/// reciente primero) — orden natural para "qué se aplicó última".
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

/// v0.10.18 (10.6.c.2) — Marca una version como aplicada en
/// `_fitz_migrations` SIN ejecutar el SQL del archivo. Útil
/// para adoptar Fitz en una DB legacy donde el schema ya está
/// aplicado manualmente — sin stamp, `migrate` intentaría
/// re-aplicar el `CREATE TABLE IF NOT EXISTS ...` que tal vez
/// es fine pero el seed data o ALTER ya estaba.
///
/// Devuelve:
/// - `Ok(true)` si insertó (la version no estaba registrada).
/// - `Ok(false)` si la version YA estaba aplicada (no-op).
///
/// **NO valida** que la version exista en el dir de migrations
/// — el caller (handler CLI `db_stamp_cmd`) decide la política
/// y muestra warning si no existe en el dir.
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
    // INSERT con ON CONFLICT DO NOTHING como defensa contra
    // race: dos `stamp` concurrentes sobre la misma version.
    let insert_sql = format!(
        "INSERT INTO {} (version) VALUES ($1) ON CONFLICT (version) DO NOTHING",
        quote_ident(TRACKING_TABLE),
    );
    conn.exec(&insert_sql, &[PgValue::Text(version.to_string())])
        .await?;
    Ok(true)
}

/// v0.10.18 (10.6.c.2) — Marca TODAS las migrations pendientes
/// del dir como aplicadas sin ejecutar SQL. Devuelve las
/// versiones nuevamente stamped (vacío si todas ya estaban).
/// Útil para adoptar Fitz en proyectos con schema legacy +
/// múltiples migrations ya aplicadas a mano.
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

/// Versiones aplicadas ordenadas por `applied_at DESC` (más
/// reciente primero). Usado por `rollback_n` para tomar las
/// últimas N revertibles.
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

/// Aplica TODAS las migrations pendientes en orden. Skipea las
/// ya aplicadas (idempotente).
///
/// Retorna las versiones que aplicó (vacío si nada pendiente).
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

/// Status report: cruza files en `dir` con applied_versions →
/// devuelve `(version, filename, status)` por migration. Útil
/// para `fitz db status`.
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
// Helpers de extracción de rows
// =================================================================

fn extract_string(row: &crate::db::Row, col: &str) -> DbResult<String> {
    match row.get(col) {
        Some(PgValue::Text(s)) => Ok(s.clone()),
        Some(PgValue::Null) => Err(DbError::Protocol(format!(
            "introspect: columna `{col}` NULL inesperado"
        ))),
        Some(other) => Err(DbError::Protocol(format!(
            "introspect: columna `{col}` esperaba Text, recibió {other:?}"
        ))),
        None => Err(DbError::Protocol(format!(
            "introspect: columna `{col}` no presente en row"
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

/// Canonicaliza el tipo SQL que reporta `information_schema` para
/// que matchee con lo que generamos del lado @table type. PG
/// reporta nombres como "double precision" en `data_type` pero
/// "float8" en `udt_name`; preferimos el nombre legible más
/// estándar (`text`, `bigint`, `boolean`, etc.).
fn canonicalize_sql_type(data_type: &str, udt_name: &str) -> String {
    // `data_type` suele ser el nombre estándar SQL ("bigint",
    // "text", "boolean", "timestamp with time zone", "jsonb",
    // "ARRAY"). Para ARRAY necesitamos el `udt_name` con prefijo
    // `_` (ej: `_text` para `text[]`) para reconstruir.
    if data_type == "ARRAY" {
        // udt_name viene como `_text`/`_int8`/etc. Convertimos al
        // sufijo `[]` que es más legible.
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
// Tests unitarios — solo lo que no requiere DB real
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
        // email: text → varchar (cambio de tipo)
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
        // PG devuelve `now()` lowercase; el user pasó `NOW()`.
        // El diff debe ser vacío (idempotente).
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
        // PG devuelve `'public'::text` para literales Str; el user
        // pasó `'public'`. El diff debe ser vacío.
        let current = Schema {
            tables: vec![Table {
                name: "settings".to_string(),
                columns: vec![col_with_default("scope", "text", Some("'public'::text"))],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
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
        // Simular "current" como si PG devolviera el default con
        // formato canonical lowercase.
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
    fn split_up_down_sin_marcadores_es_up_completo() {
        let (up, down) = split_up_down("CREATE TABLE x (id int);\n");
        assert_eq!(up.trim(), "CREATE TABLE x (id int);");
        assert!(down.is_none());
    }

    #[test]
    fn split_up_down_con_ambos_marcadores() {
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
    fn split_up_down_sin_up_marker_pero_con_down() {
        let raw = "CREATE TABLE x (id int);\n-- DOWN\nDROP TABLE x;\n";
        let (up, down) = split_up_down(raw);
        assert!(up.contains("CREATE TABLE x"));
        assert_eq!(down.as_deref().map(str::trim), Some("DROP TABLE x;"));
    }

    #[test]
    fn split_up_down_marker_con_chars_extra_no_es_marker() {
        // `-- UP foo` NO es marcador (chars adicionales)
        let raw = "-- UP foo\nA;\n";
        let (up, down) = split_up_down(raw);
        // Todo es UP (el "-- UP foo" es comment SQL inocuo)
        assert!(up.contains("-- UP foo"));
        assert!(up.contains("A;"));
        assert!(down.is_none());
    }

    #[test]
    fn read_migrations_dir_preserva_up_down() {
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
    fn read_migrations_dir_detecta_fitz_files() {
        // v0.10.19 (10.6.d) — `.fitz` y `.sql` se intercalan según
        // orden alfabético. La variante `kind` indica el backend.
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
        // Orden alfabético: 100000 < 120000 → sql primero, fitz segundo.
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
            }],
        };
        let changes = diff_schemas(&current, &target);
        // Debe emitir solo RenameTable; el resto del diff es no-op.
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
        // No debería emitir CREATE TABLE ni DROP TABLE.
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
        // Reemplazar `email` por `old_email` en current.
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
        // No debería emitir ADD/DROP COLUMN.
        for c in &changes {
            assert!(
                !matches!(c, Change::AddColumn { .. } | Change::DropColumn { .. }),
                "rename no debería emitir ADD/DROP COLUMN; got: {c:?}"
            );
        }
    }

    #[test]
    fn rename_table_sql_emite_alter_rename_to() {
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
    fn rename_column_sql_emite_alter_rename_column() {
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
    fn renamed_from_sin_old_en_current_es_noop_silencioso() {
        // User dejó el decorator @renamed_from("old") pero ya
        // aplicó la migration y la old_name ya no existe en current
        // (current ya está renombrada). El diff NO debe emitir
        // RenameTable spurio.
        let current = Schema {
            tables: vec![Table {
                name: "users".to_string(),
                columns: vec![col("id", "bigint", false, true)],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
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
            }],
        };
        let changes = diff_schemas(&current, &target);
        assert!(
            changes.is_empty(),
            "renamed_from sin match en current debe ser no-op, got: {changes:?}"
        );
    }

    #[test]
    fn rename_table_seguido_de_alter_column_orden_seguro() {
        // current: tabla "old_x" con col `name`.
        // target: tabla "x" renamed_from="old_x" con col `name`
        // marcado nullable=true (current era false). El diff debe
        // emitir RenameTable PRIMERO, después AlterColumnNullable
        // referenciando el nombre NUEVO ("x"). Si no se renombra
        // primero, el ALTER falla porque "x" no existe.
        let current = Schema {
            tables: vec![Table {
                name: "old_x".to_string(),
                columns: vec![col("name", "text", false, false)],
                indexes: vec![],
                foreign_keys: vec![],
                renamed_from: None,
                schema: None,
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
        // El AlterColumn debe referenciar el nombre NUEVO.
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
    fn schema_from_program_field_renamed_from_se_carga_a_column() {
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
    fn schema_from_program_table_renamed_from_se_carga_a_table() {
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
    // El handler `db_check_cmd` reusa `diff_schemas` (ya cubierto
    // por los tests de diff arriba) + decide exit code basado en
    // `changes.is_empty()`. Acá testeamos la decisión.

    #[test]
    fn check_es_verde_cuando_diff_es_vacio() {
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
    fn check_falla_cuando_hay_drift() {
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

    // Los stamps requieren conn real — los tests unitarios sin DB
    // solo pueden verificar que los símbolos están exportados.
    // La validación end-to-end vive en el CI con DB real (job
    // `db-postgres`) y en el smoke local del autor contra el
    // Postgres 15 instalado.

    #[test]
    fn stamp_version_y_stamp_all_pending_estan_exportadas() {
        // Smoke "el símbolo existe y es callable" — si alguien
        // renombra o cambia firma, este test rompe a compilar.
        let _f1: fn(_, _) -> _ = stamp_version;
        let _f2: fn(_, _) -> _ = stamp_all_pending;
    }

    // ============================================================
    // v0.10.20 (10.6.e.1) — history shape
    // ============================================================

    #[test]
    fn history_entry_shape() {
        // Smoke estructural: HistoryEntry expone los 3 fields que
        // el CLI usa para format. Si alguien renombra rompe acá.
        let e = HistoryEntry {
            version: "20260530100000".to_string(),
            applied_at: "2026-05-30 10:00:00+00".to_string(),
            filename: Some("init.sql".to_string()),
        };
        assert_eq!(e.version, "20260530100000");
        assert!(e.applied_at.contains("2026-05-30"));
        assert_eq!(e.filename.as_deref(), Some("init.sql"));
        // None filename para versions stamped sin archivo en dir.
        let e2 = HistoryEntry {
            version: "19990101000000".to_string(),
            applied_at: "ago".to_string(),
            filename: None,
        };
        assert!(e2.filename.is_none());
    }

    #[test]
    fn history_signature_compila() {
        // El símbolo `history` existe y devuelve el tipo esperado.
        let _f: fn(_, _) -> _ = history;
    }
}
