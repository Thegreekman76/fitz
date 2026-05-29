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
/// - `schemaname = 'public'` (default Postgres; configurable
///   futura si entra demanda).
/// - `table_type = 'BASE TABLE'` (skipea views).
/// - Excluye explícitamente `_fitz_migrations`.
pub async fn introspect_schema(conn: &std::sync::Arc<DbConnHandle>) -> DbResult<Schema> {
    let table_names = list_user_tables(conn).await?;
    let mut tables = Vec::with_capacity(table_names.len());
    for name in &table_names {
        let columns = introspect_columns(conn, name).await?;
        let indexes = introspect_indexes(conn, name).await?;
        let foreign_keys = introspect_foreign_keys(conn, name).await?;
        tables.push(Table {
            name: name.clone(),
            columns,
            indexes,
            foreign_keys,
        });
    }
    // Orden alfabético para snapshot determinístico (facilita
    // tests + diffs reproducibles).
    tables.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Schema { tables })
}

/// Lista las user-tables del schema `public`. Excluye system
/// tables + `_fitz_migrations`.
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
    table: &str,
) -> DbResult<Vec<Column>> {
    // Cols base: nombre + tipo + nullable + default.
    let sql_cols = "SELECT column_name, data_type, udt_name, is_nullable, column_default \
                    FROM information_schema.columns \
                    WHERE table_schema = 'public' AND table_name = $1 \
                    ORDER BY ordinal_position";
    let qr = conn
        .query(sql_cols, &[PgValue::Text(table.to_string())])
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
        });
    }
    // PK: cruce con pg_index para marcar `is_primary`.
    let sql_pk = "SELECT a.attname AS column_name \
                  FROM pg_index i \
                  JOIN pg_attribute a ON a.attrelid = i.indrelid \
                                     AND a.attnum = ANY(i.indkey) \
                  WHERE i.indrelid = ($1::regclass) AND i.indisprimary";
    let pk_qr = conn
        .query(sql_pk, &[PgValue::Text(format!("public.{}", table))])
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
        .query(sql, &[PgValue::Text(format!("public.{}", table))])
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
                 AND tc.table_schema = 'public' \
                 AND tc.table_name = $1 \
               ORDER BY tc.constraint_name, kcu.ordinal_position";
    let qr = conn.query(sql, &[PgValue::Text(table.to_string())]).await?;
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
    tables.sort_by(|a, b| a.name.cmp(&b.name));
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    CreateTable(Table),
    DropTable(String),
    AddColumn {
        table: String,
        column: Column,
    },
    DropColumn {
        table: String,
        column: String,
    },
    AlterColumnType {
        table: String,
        column: String,
        new_type: String,
    },
    AlterColumnNullable {
        table: String,
        column: String,
        nullable: bool,
    },
    /// v0.10.16 — Cambio del DEFAULT de un column existente.
    /// `new_default = Some(sql)` → `SET DEFAULT <sql>`; `None` →
    /// `DROP DEFAULT`. La normalización para el diff es
    /// case-insensitive sobre función calls SQL (`now()` matchea
    /// `NOW()` y `Now()`).
    AlterColumnDefault {
        table: String,
        column: String,
        new_default: Option<String>,
    },
    CreateIndex {
        table: String,
        index: Index,
    },
    DropIndex {
        table: String,
        index_name: String,
    },
    AddForeignKey {
        table: String,
        fk: ForeignKey,
    },
    DropForeignKey {
        table: String,
        fk_name: String,
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
/// - **No detecta renames** (column ni table). Un rename de
///   `name` → `full_name` se ve como `DROP COLUMN name` + `ADD
///   COLUMN full_name`, perdiendo los datos. El user debe editar
///   el SQL manualmente o usar `fitz db new` + migration custom.
/// - **`AlterColumnType` directo sin USING** — si los datos NO
///   convierten al nuevo tipo, Postgres falla. El user debe
///   editar la migration para agregar `USING (col::new_type)` o
///   data migration script.
pub fn diff_schemas(current: &Schema, target: &Schema) -> Vec<Change> {
    let mut changes = Vec::new();

    let current_table_names: std::collections::HashSet<&str> =
        current.tables.iter().map(|t| t.name.as_str()).collect();
    let target_table_names: std::collections::HashSet<&str> =
        target.tables.iter().map(|t| t.name.as_str()).collect();

    // --- 1. CREATE TABLE (target tables no presentes en current).
    let mut create_tables: Vec<&Table> = target
        .tables
        .iter()
        .filter(|t| !current_table_names.contains(t.name.as_str()))
        .collect();
    create_tables.sort_by(|a, b| a.name.cmp(&b.name));
    for t in &create_tables {
        changes.push(Change::CreateTable((*t).clone()));
    }

    // --- 2. ALTER de tablas en AMBOS schemas.
    let mut tables_to_alter: Vec<(&Table, &Table)> = current
        .tables
        .iter()
        .filter_map(|c| {
            target
                .tables
                .iter()
                .find(|t| t.name == c.name)
                .map(|t| (c, t))
        })
        .collect();
    tables_to_alter.sort_by(|a, b| a.0.name.cmp(&b.0.name));

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
                table: current_t.name.clone(),
                fk_name: fk.name.clone(),
            });
        }
    }

    // --- 4. ADD FK (después de tener todas las tables y cols).
    // Incluye FKs de CreateTable también: si la tabla es nueva,
    // emit FKs como ADD separado (en lugar de inline en el
    // CREATE TABLE) — eso destraba ciclos entre tablas nuevas.
    for t in &create_tables {
        let mut fks: Vec<&ForeignKey> = t.foreign_keys.iter().collect();
        fks.sort_by(|a, b| a.name.cmp(&b.name));
        for fk in fks {
            changes.push(Change::AddForeignKey {
                table: t.name.clone(),
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
                table: target_t.name.clone(),
                fk: fk.clone(),
            });
        }
    }

    // --- 5. DROP TABLE (current tables no en target).
    let mut drop_tables: Vec<&Table> = current
        .tables
        .iter()
        .filter(|t| !target_table_names.contains(t.name.as_str()))
        .collect();
    drop_tables.sort_by(|a, b| a.name.cmp(&b.name));
    for t in drop_tables {
        changes.push(Change::DropTable(t.name.clone()));
    }

    changes
}

fn diff_columns(current: &Table, target: &Table, changes: &mut Vec<Change>) {
    let current_col_names: std::collections::HashSet<&str> =
        current.columns.iter().map(|c| c.name.as_str()).collect();
    let target_col_names: std::collections::HashSet<&str> =
        target.columns.iter().map(|c| c.name.as_str()).collect();

    // Drop columns no en target.
    let mut to_drop: Vec<&Column> = current
        .columns
        .iter()
        .filter(|c| !target_col_names.contains(c.name.as_str()))
        .collect();
    to_drop.sort_by(|a, b| a.name.cmp(&b.name));
    for c in to_drop {
        changes.push(Change::DropColumn {
            table: current.name.clone(),
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
            table: target.name.clone(),
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
                    table: target.name.clone(),
                    column: tc.name.clone(),
                    new_type: tc.sql_type.clone(),
                });
            }
            if cc.nullable != tc.nullable {
                changes.push(Change::AlterColumnNullable {
                    table: target.name.clone(),
                    column: tc.name.clone(),
                    nullable: tc.nullable,
                });
            }
            // v0.10.16 — Default diff con normalización tolerante.
            // Postgres devuelve `now()` lowercase con casts (e.g.
            // `'foo'::text`); el user típicamente pasa `NOW()` o
            // `'foo'`. Comparamos versiones normalizadas para evitar
            // falsos positivos. Si el user remueve `@db_default(...)`,
            // emitimos `DROP DEFAULT`; si agrega, `SET DEFAULT`.
            let current_norm = cc.default.as_deref().map(normalize_default_for_diff);
            let target_norm = tc.default.as_deref().map(normalize_default_for_diff);
            if current_norm != target_norm {
                changes.push(Change::AlterColumnDefault {
                    table: target.name.clone(),
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

    // Drop indexes no en target.
    let mut to_drop: Vec<&Index> = current
        .indexes
        .iter()
        .filter(|i| !target_idx_names.contains(i.name.as_str()))
        .collect();
    to_drop.sort_by(|a, b| a.name.cmp(&b.name));
    for i in to_drop {
        changes.push(Change::DropIndex {
            table: current.name.clone(),
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
            table: target.name.clone(),
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
        Change::DropTable(name) => format!("DROP TABLE {};", quote_ident(name)),
        Change::AddColumn { table, column } => {
            let default = column
                .default
                .as_deref()
                .map(|d| format!(" DEFAULT {}", d))
                .unwrap_or_default();
            format!(
                "ALTER TABLE {} ADD COLUMN {} {}{}{};",
                quote_ident(table),
                quote_ident(&column.name),
                column.sql_type,
                default,
                if column.nullable { "" } else { " NOT NULL" },
            )
        }
        Change::DropColumn { table, column } => {
            format!(
                "ALTER TABLE {} DROP COLUMN {};",
                quote_ident(table),
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
                quote_ident(table),
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
                quote_ident(table),
                quote_ident(column),
                expr,
            ),
            None => format!(
                "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT;",
                quote_ident(table),
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
                    quote_ident(table),
                    quote_ident(column),
                )
            } else {
                format!(
                    "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL;",
                    quote_ident(table),
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
                quote_ident(table),
                cols.join(", "),
            )
        }
        Change::DropIndex {
            table: _,
            index_name,
        } => {
            // Postgres DROP INDEX no requiere prefijo de tabla.
            format!("DROP INDEX {};", quote_ident(index_name))
        }
        Change::AddForeignKey { table, fk } => {
            let on_delete = fk
                .on_delete
                .as_deref()
                .map(|action| format!(" ON DELETE {}", action))
                .unwrap_or_default();
            format!(
                "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({}){};",
                quote_ident(table),
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
                quote_ident(table),
                quote_ident(fk_name),
            )
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
        quote_ident(&t.name),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationFile {
    pub version: String,
    pub filename: String,
    pub sql: String,
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
        if !filename.ends_with(".sql") {
            continue;
        }
        let version = filename
            .strip_suffix(".sql")
            .unwrap_or(&filename)
            .split_once('_')
            .map(|(prefix, _)| prefix.to_string())
            .unwrap_or_else(|| {
                filename
                    .strip_suffix(".sql")
                    .unwrap_or(&filename)
                    .to_string()
            });
        let sql = std::fs::read_to_string(&path)
            .map_err(|e| format!("leyendo `{}`: {e}", path.display()))?;
        migrations.push(MigrationFile {
            version,
            filename,
            sql,
        });
    }
    migrations.sort_by(|a, b| a.version.cmp(&b.version));
    Ok(migrations)
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
    ensure_tracking_table(conn).await?;
    let version = migration.version.clone();
    let sql = migration.sql.clone();
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
            Change::DropTable(name) => assert_eq!(name, "users"),
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
                Change::AddColumn { table, column } => Some((table.as_str(), column.name.as_str())),
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
                Change::DropColumn { table, column } => Some((table.as_str(), column.as_str())),
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
                } => Some((table.as_str(), column.as_str(), new_type.as_str())),
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
                } => Some((table.as_str(), column.as_str(), *nullable)),
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
                    Some((table.as_str(), index.name.as_str(), index.unique))
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
                Change::DropIndex { table, index_name } => {
                    Some((table.as_str(), index_name.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(dropped, vec![("users", "users_email_idx")]);
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
                    table.as_str(),
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
        let changes = vec![Change::DropTable("legacy".to_string())];
        let sql = changes_to_sql(&changes);
        assert!(sql.contains("DROP TABLE"), "esperaba DROP TABLE: {sql}");
        assert!(sql.contains("\"legacy\""), "esperaba nombre quoted: {sql}");
    }

    #[test]
    fn changes_to_sql_add_column_emits_alter_add() {
        let changes = vec![Change::AddColumn {
            table: "users".to_string(),
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
            table: "users".to_string(),
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
            table: "users".to_string(),
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
                Change::AddColumn { table, column } => Some((table.as_str(), column.name.as_str())),
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
            table: "events".to_string(),
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
            table: "events".to_string(),
            column: "created_at".to_string(),
            new_default: Some("NOW()".to_string()),
        };
        let sql = changes_to_sql(&[change]);
        assert!(sql.contains("SET DEFAULT NOW()"), "got: {sql}");
    }

    #[test]
    fn alter_column_default_drop_emits_drop_default() {
        let change = Change::AlterColumnDefault {
            table: "events".to_string(),
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
            }],
        };
        let target = Schema {
            tables: vec![Table {
                name: "settings".to_string(),
                columns: vec![col_with_default("scope", "text", Some("'public'"))],
                indexes: vec![],
                foreign_keys: vec![],
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
}
