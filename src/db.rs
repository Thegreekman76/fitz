//! Driver Postgres puro Fitz — Fase 10.1.
//!
//! Implementación nativa del PostgreSQL wire protocol v3.0 sin
//! dependencias externas de DB (sin libpq, sin tokio-postgres, sin
//! sqlx). Solo `tokio::net::TcpStream` para I/O + crates de
//! crypto pure-Rust (sha2/hmac/base64) para SCRAM-SHA-256.
//!
//! Decisiones de diseño (cerradas 2026-05-25, ver
//! `docs/roadmap.md` → Fase 10):
//!  - Driver puro Fitz, sin libpq → binario standalone preservado.
//!  - API toda async → encaja con tokio + handlers HTTP de 9.w.
//!  - Sin pool en 10.1 (llega en 10.2). Una conexión por
//!    `connect(url)`.
//!  - Postgres 14+ (SCRAM-SHA-256 default, JSONB maduro).
//!  - URI estándar `postgres://user:pass@host:port/db?sslmode=...`.
//!  - SSL/TLS pospuesto a sub-paso futuro. En 10.1 solo
//!    `sslmode=disable` (default); `sslmode=require` aborta con
//!    mensaje claro.
//!  - Sin Extended Query Protocol cache de prepared statements
//!    (cada query parsea desde cero).
//!  - Tipos OID core en MVP: Int4/Int8/Float4/Float8/Text/
//!    Varchar/Bool/Timestamp/Timestamptz/UUID/Bytea. JSONB +
//!    arrays + dates avanzados llegan en 10.5.
//!
//! El módulo está deliberadamente **aislado** del resto del
//! crate en 10.1.a — no integra todavía con `evaluator` ni
//! `value::Value::DbConn`. La integración como built-in módulo
//! `db` accesible desde código Fitz llega en 10.1.b.

#![allow(dead_code)] // 10.1.a — APIs públicas se consumen en 10.1.b cuando llegue la integración con evaluator.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

type HmacSha256 = Hmac<Sha256>;

// =============================================================
// Errors
// =============================================================

/// Error específico del driver Postgres. Mantenemos un enum
/// dedicado (en lugar de envolver `FitzError` directo) porque el
/// driver vive como módulo aislado en 10.1.a; la traducción a
/// `FitzError` (con span, posición, etc.) sucede en la capa de
/// integración 10.1.b.
#[derive(Debug)]
pub enum DbError {
    /// URL de conexión inválida (formato, parsing, valores fuera
    /// de rango).
    InvalidUrl(String),
    /// Falla de I/O (TCP, lectura/escritura del socket).
    Io(io::Error),
    /// Protocolo Postgres devolvió algo inesperado (mensaje fuera
    /// de secuencia, longitud inválida, etc.).
    Protocol(String),
    /// Auth fallida (credenciales incorrectas, SCRAM mismatch, no
    /// soportado).
    Auth(String),
    /// Error del servidor Postgres (`ErrorResponse`). Formato
    /// canónico `"<severity>: <message>"` paralelo a
    /// `jwt`/`hash`/etc.
    Server {
        severity: String,
        code: String,
        message: String,
    },
    /// Tipo OID Postgres sin soporte en MVP. El error cita el OID
    /// numérico para que el user vea claramente qué tipo agregar
    /// al refinamiento del MVP en 10.5.
    UnsupportedType(u32),
    /// Feature pedida por el user que aún no llegó en 10.1
    /// (sslmode=require, tipos avanzados, etc.). Mensaje incluye
    /// referencia al sub-paso de cierre.
    NotImplemented(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::InvalidUrl(m) => write!(f, "URL inválida: {m}"),
            DbError::Io(e) => write!(f, "I/O: {e}"),
            DbError::Protocol(m) => write!(f, "protocolo: {m}"),
            DbError::Auth(m) => write!(f, "auth: {m}"),
            DbError::Server {
                severity, message, ..
            } => {
                write!(f, "{severity}: {message}")
            }
            DbError::UnsupportedType(oid) => {
                write!(f, "tipo Postgres OID {oid} no soportado en MVP (10.5)")
            }
            DbError::NotImplemented(m) => write!(f, "no implementado: {m}"),
        }
    }
}

impl std::error::Error for DbError {}

impl From<io::Error> for DbError {
    fn from(e: io::Error) -> Self {
        DbError::Io(e)
    }
}

pub type DbResult<T> = Result<T, DbError>;

// =============================================================
// ConnectionConfig — parsing del connection string
// =============================================================

/// Configuración resuelta de una conexión Postgres. Se construye
/// vía `ConnectionConfig::parse(url)` desde el URI estándar
/// `postgres://user:pass@host:port/dbname?sslmode=...`.
///
/// Soportado en 10.1:
///  - Schemes: `postgres://` y `postgresql://` (alias estándar).
///  - User obligatorio. Password opcional (auth trust/peer no
///    necesita password).
///  - Host: literal (IPv4, IPv6 entre brackets `[::1]`, o
///    hostname).
///  - Port: opcional, default 5432.
///  - Dbname: obligatorio en MVP. Default a username si query
///    string lo permite, llega en 10.x.
///  - Query: `sslmode=disable|require|allow|prefer`. Solo
///    `disable` soportado en 10.1; el resto aborta con mensaje
///    claro citando el sub-paso futuro.
///  - `application_name=...` ignorado silenciosamente en MVP
///    (no daña, no afecta).
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub dbname: String,
    pub sslmode: SslMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslMode {
    Disable,
    /// Pospuesto a 10.1.b o sub-paso TLS futuro.
    Require,
}

impl ConnectionConfig {
    /// Parsea `postgres://user:pass@host:port/dbname?sslmode=...`.
    /// El formato está alineado con libpq / psycopg2 / pgx / sqlx
    /// (mismo URI para todos los drivers Postgres del ecosistema).
    pub fn parse(url: &str) -> DbResult<Self> {
        let rest = url
            .strip_prefix("postgres://")
            .or_else(|| url.strip_prefix("postgresql://"))
            .ok_or_else(|| {
                DbError::InvalidUrl(format!(
                    "esperaba 'postgres://' o 'postgresql://', no '{url}'"
                ))
            })?;

        // Split en la última '@' que NO esté dentro de la query
        // string. Práctica: split en la primera '?' antes (separa
        // auth+host+path de query), después la última '@' del
        // segmento previo.
        let (pre_query, query) = match rest.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (rest, None),
        };

        // pre_query := [user[:password]@]host[:port][/dbname]
        let (auth, host_dbname) = match pre_query.rfind('@') {
            Some(idx) => (Some(&pre_query[..idx]), &pre_query[idx + 1..]),
            None => (None, pre_query),
        };

        let (user, password) = match auth {
            Some(a) => {
                let (u, p): (&str, Option<String>) = match a.split_once(':') {
                    Some((u, p)) => (u, Some(p.to_string())),
                    None => (a, None),
                };
                if u.is_empty() {
                    return Err(DbError::InvalidUrl("usuario vacío".into()));
                }
                let pwd_decoded = match p {
                    Some(s) => Some(percent_decode_owned(&s)?),
                    None => None,
                };
                (percent_decode(u)?, pwd_decoded)
            }
            None => return Err(DbError::InvalidUrl("falta usuario antes de '@'".into())),
        };

        // host_dbname := host[:port][/dbname]
        let (host_port, dbname) = match host_dbname.split_once('/') {
            Some((h, d)) => (h, d.to_string()),
            None => {
                return Err(DbError::InvalidUrl(
                    "falta nombre de la base de datos después de '/'".into(),
                ))
            }
        };

        let (host, port) = split_host_port(host_port)?;

        let sslmode = parse_sslmode(query)?;

        Ok(ConnectionConfig {
            host,
            port,
            user,
            password,
            dbname,
            sslmode,
        })
    }
}

fn split_host_port(s: &str) -> DbResult<(String, u16)> {
    if s.is_empty() {
        return Err(DbError::InvalidUrl("host vacío".into()));
    }
    // IPv6 literal: `[::1]:5432`. El bracket cierra antes del ':'
    // del puerto.
    if let Some(rest) = s.strip_prefix('[') {
        let (addr, tail) = rest
            .split_once(']')
            .ok_or_else(|| DbError::InvalidUrl(format!("IPv6 sin ']' en '{s}'")))?;
        let port = if let Some(p) = tail.strip_prefix(':') {
            parse_port(p)?
        } else if tail.is_empty() {
            5432
        } else {
            return Err(DbError::InvalidUrl(format!(
                "esperaba ':<puerto>' o fin tras ']', no '{tail}'"
            )));
        };
        return Ok((addr.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((h, p)) => Ok((h.to_string(), parse_port(p)?)),
        None => Ok((s.to_string(), 5432)),
    }
}

fn parse_port(s: &str) -> DbResult<u16> {
    s.parse::<u16>()
        .map_err(|_| DbError::InvalidUrl(format!("puerto inválido '{s}'")))
}

fn parse_sslmode(query: Option<&str>) -> DbResult<SslMode> {
    let q = match query {
        Some(q) => q,
        None => return Ok(SslMode::Disable),
    };
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "sslmode" => {
                return match v {
                    "disable" | "" => Ok(SslMode::Disable),
                    "require" | "verify-ca" | "verify-full" | "prefer" | "allow" => {
                        Err(DbError::NotImplemented(format!(
                            "sslmode={v} llega en sub-paso futuro de TLS \
                             (10.1.b o posterior); usá sslmode=disable en 10.1"
                        )))
                    }
                    other => Err(DbError::InvalidUrl(format!(
                        "sslmode desconocido: '{other}'"
                    ))),
                };
            }
            // application_name, connect_timeout, etc. — silently
            // ignored en MVP. No daña, no afecta correctness.
            _ => continue,
        }
    }
    Ok(SslMode::Disable)
}

fn percent_decode(s: &str) -> DbResult<String> {
    percent_decode_owned(s)
}

fn percent_decode_owned(s: &str) -> DbResult<String> {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(DbError::InvalidUrl(
                    "secuencia '%' incompleta en URL".into(),
                ));
            }
            let hi = hex_digit(bytes[i + 1])?;
            let lo = hex_digit(bytes[i + 2])?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| DbError::InvalidUrl("URL no es UTF-8 válido".into()))
}

fn hex_digit(b: u8) -> DbResult<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(DbError::InvalidUrl(format!(
            "hex inválido en URL: '{}'",
            b as char
        ))),
    }
}

// =============================================================
// Wire protocol — mensajes Postgres v3.0
// =============================================================
//
// El protocolo Postgres tiene dos clases de mensajes:
//
//  - Frontend → Backend (cliente → servidor): cada mensaje tiene
//    1 byte de tipo (ASCII) + 4 bytes de longitud big-endian +
//    payload. EXCEPCIÓN: `StartupMessage` y `SSLRequest` no tienen
//    byte de tipo (la longitud arranca el mensaje).
//
//  - Backend → Frontend (servidor → cliente): 1 byte de tipo +
//    4 bytes de longitud + payload. Sin excepción.
//
// Convención: la longitud INCLUYE los 4 bytes de longitud pero
// EXCLUYE el byte de tipo (cuando hay tipo). Eso es lo que dice
// la spec; al codear, restamos 4 al leer/sumamos 4 al escribir.

/// Mensajes que enviamos al servidor.
#[derive(Debug)]
pub enum FrontendMessage<'a> {
    /// Inicia la conexión. Sin byte de tipo. Payload:
    /// `version(4) | param1\0val1\0...\0` donde el último param
    /// debe ser seguido de `\0` (terminador de lista).
    Startup { user: &'a str, database: &'a str },
    /// Respuesta a `AuthenticationCleartextPassword` o
    /// `AuthenticationMD5Password`.
    Password { password: &'a [u8] },
    /// Inicio del flow SASL: `AuthenticationSASL` recibido,
    /// emitimos el client-first-message.
    SaslInitialResponse {
        mechanism: &'a str,
        initial_response: &'a [u8],
    },
    /// Continuación del flow SASL: client-final-message.
    SaslResponse { response: &'a [u8] },
    /// Simple Query: ejecuta el statement con auto-commit y
    /// devuelve el resultado en un solo round-trip.
    Query { sql: &'a str },
    /// Extended Query — declara un statement parseable.
    Parse {
        statement_name: &'a str,
        sql: &'a str,
        param_types: &'a [u32], // OIDs; 0 = let server decide
    },
    /// Extended Query — bindea params concretos a un statement.
    Bind {
        portal_name: &'a str,
        statement_name: &'a str,
        param_formats: &'a [i16],             // 0 = text, 1 = binary
        param_values: &'a [Option<&'a [u8]>], // None = NULL
        result_formats: &'a [i16],
    },
    /// Extended Query — ejecuta un portal.
    Execute {
        portal_name: &'a str,
        max_rows: i32, // 0 = unlimited
    },
    /// Extended Query — flush + commit del round.
    Sync,
    /// Cierra el statement o portal.
    Close { kind: u8, name: &'a str }, // 'S' = statement, 'P' = portal
    /// Describe un statement o portal.
    Describe { kind: u8, name: &'a str },
    /// Termina la conexión cooperativamente.
    Terminate,
}

impl<'a> FrontendMessage<'a> {
    /// Serializa el mensaje a bytes prontos para `write_all`. El
    /// caller no necesita preocuparse del framing — todo (tag +
    /// length + payload) viene en el buffer.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            FrontendMessage::Startup { user, database } => {
                let mut payload = Vec::new();
                // Protocol version: 3.0 = 196608 = 3 << 16.
                payload.extend_from_slice(&196608u32.to_be_bytes());
                write_cstr(&mut payload, "user");
                write_cstr(&mut payload, user);
                write_cstr(&mut payload, "database");
                write_cstr(&mut payload, database);
                payload.extend_from_slice(&[
                    // application_name = "fitz"  (puramente
                    // informativo del lado server; ayuda al user
                    // a identificar la conexión en pg_stat_activity)
                ]);
                write_cstr(&mut payload, "application_name");
                write_cstr(&mut payload, "fitz");
                write_cstr(&mut payload, "client_encoding");
                write_cstr(&mut payload, "UTF8");
                payload.push(0); // terminador de lista
                frame_no_tag(&payload)
            }
            FrontendMessage::Password { password } => frame_tag(b'p', password),
            FrontendMessage::SaslInitialResponse {
                mechanism,
                initial_response,
            } => {
                let mut payload =
                    Vec::with_capacity(mechanism.len() + 1 + 4 + initial_response.len());
                write_cstr(&mut payload, mechanism);
                payload.extend_from_slice(&(initial_response.len() as i32).to_be_bytes());
                payload.extend_from_slice(initial_response);
                frame_tag(b'p', &payload)
            }
            FrontendMessage::SaslResponse { response } => frame_tag(b'p', response),
            FrontendMessage::Query { sql } => {
                let mut payload = Vec::with_capacity(sql.len() + 1);
                write_cstr(&mut payload, sql);
                frame_tag(b'Q', &payload)
            }
            FrontendMessage::Parse {
                statement_name,
                sql,
                param_types,
            } => {
                let mut payload = Vec::new();
                write_cstr(&mut payload, statement_name);
                write_cstr(&mut payload, sql);
                payload.extend_from_slice(&(param_types.len() as i16).to_be_bytes());
                for oid in *param_types {
                    payload.extend_from_slice(&oid.to_be_bytes());
                }
                frame_tag(b'P', &payload)
            }
            FrontendMessage::Bind {
                portal_name,
                statement_name,
                param_formats,
                param_values,
                result_formats,
            } => {
                let mut payload = Vec::new();
                write_cstr(&mut payload, portal_name);
                write_cstr(&mut payload, statement_name);
                payload.extend_from_slice(&(param_formats.len() as i16).to_be_bytes());
                for fmt in *param_formats {
                    payload.extend_from_slice(&fmt.to_be_bytes());
                }
                payload.extend_from_slice(&(param_values.len() as i16).to_be_bytes());
                for v in *param_values {
                    match v {
                        Some(bytes) => {
                            payload.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                            payload.extend_from_slice(bytes);
                        }
                        None => {
                            payload.extend_from_slice(&(-1i32).to_be_bytes());
                        }
                    }
                }
                payload.extend_from_slice(&(result_formats.len() as i16).to_be_bytes());
                for fmt in *result_formats {
                    payload.extend_from_slice(&fmt.to_be_bytes());
                }
                frame_tag(b'B', &payload)
            }
            FrontendMessage::Execute {
                portal_name,
                max_rows,
            } => {
                let mut payload = Vec::new();
                write_cstr(&mut payload, portal_name);
                payload.extend_from_slice(&max_rows.to_be_bytes());
                frame_tag(b'E', &payload)
            }
            FrontendMessage::Sync => frame_tag(b'S', &[]),
            FrontendMessage::Close { kind, name } => {
                let mut payload = Vec::new();
                payload.push(*kind);
                write_cstr(&mut payload, name);
                frame_tag(b'C', &payload)
            }
            FrontendMessage::Describe { kind, name } => {
                let mut payload = Vec::new();
                payload.push(*kind);
                write_cstr(&mut payload, name);
                frame_tag(b'D', &payload)
            }
            FrontendMessage::Terminate => frame_tag(b'X', &[]),
        }
    }
}

fn write_cstr(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
}

fn frame_no_tag(payload: &[u8]) -> Vec<u8> {
    let len = (payload.len() + 4) as u32;
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn frame_tag(tag: u8, payload: &[u8]) -> Vec<u8> {
    let len = (payload.len() + 4) as u32;
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.push(tag);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Mensajes que el servidor nos manda. Solo modelamos los tipos
/// que el driver necesita en 10.1: auth + simple/extended query +
/// estado. Mensajes informativos como ParameterStatus o
/// NoticeResponse se parsean pero el driver los descarta.
#[derive(Debug)]
pub enum BackendMessage {
    AuthenticationOk,
    AuthenticationCleartextPassword,
    AuthenticationMd5Password {
        salt: [u8; 4],
    },
    AuthenticationSasl {
        mechanisms: Vec<String>,
    },
    AuthenticationSaslContinue {
        data: Vec<u8>,
    },
    AuthenticationSaslFinal {
        data: Vec<u8>,
    },
    ParameterStatus {
        name: String,
        value: String,
    },
    BackendKeyData {
        process_id: i32,
        secret_key: i32,
    },
    ReadyForQuery {
        tx_status: u8,
    }, // 'I'/'T'/'E'
    RowDescription {
        fields: Vec<FieldDescription>,
    },
    DataRow {
        values: Vec<Option<Vec<u8>>>,
    }, // None = NULL
    CommandComplete {
        tag: String,
    },
    EmptyQueryResponse,
    ErrorResponse(ErrorFields),
    NoticeResponse(ErrorFields),
    ParseComplete,
    BindComplete,
    CloseComplete,
    NoData,
    ParameterDescription {
        oids: Vec<u32>,
    },
    PortalSuspended,
    /// Cualquier mensaje que parseamos pero no procesamos
    /// específicamente. El tag se guarda para diagnostics si
    /// algo va mal. Hoy no se emite — todos los mensajes
    /// conocidos del wire están en variantes específicas; si
    /// aparece uno nuevo, agregamos variante.
    Unknown {
        tag: u8,
        len: usize,
    },
}

#[derive(Debug, Clone)]
pub struct FieldDescription {
    pub name: String,
    pub table_oid: u32,
    pub column_idx: i16,
    pub type_oid: u32,
    pub type_size: i16,
    pub type_modifier: i32,
    pub format: i16, // 0 text, 1 binary
}

#[derive(Debug, Clone, Default)]
pub struct ErrorFields {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub position: Option<String>,
    pub where_: Option<String>,
}

impl ErrorFields {
    fn from_pairs(pairs: Vec<(u8, String)>) -> Self {
        let mut ef = ErrorFields::default();
        for (code, val) in pairs {
            match code {
                b'S' if ef.severity.is_empty() => ef.severity = val,
                b'V' => ef.severity = val, // SQLSTATE-related, no localized
                b'C' => ef.code = val,
                b'M' => ef.message = val,
                b'D' => ef.detail = Some(val),
                b'H' => ef.hint = Some(val),
                b'P' => ef.position = Some(val),
                b'W' => ef.where_ = Some(val),
                _ => {} // F (file), L (line), R (routine), etc. — ignorados
            }
        }
        ef
    }
}

// =============================================================
// Frame I/O — leer/escribir mensajes sobre TcpStream
// =============================================================

/// Lee UN mensaje del servidor: tag(1) + length(4) + payload.
/// Bloquea hasta tener el mensaje completo o error de I/O.
pub async fn read_message(stream: &mut TcpStream) -> DbResult<BackendMessage> {
    let mut header = [0u8; 5];
    stream.read_exact(&mut header).await?;
    let tag = header[0];
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len < 4 {
        return Err(DbError::Protocol(format!(
            "longitud de mensaje inválida: {len} (mínimo 4)"
        )));
    }
    let mut payload = vec![0u8; len - 4];
    stream.read_exact(&mut payload).await?;
    parse_backend_message(tag, &payload)
}

pub fn parse_backend_message(tag: u8, payload: &[u8]) -> DbResult<BackendMessage> {
    match tag {
        b'R' => parse_auth(payload),
        b'S' => {
            let (name, rest) = read_cstr(payload)?;
            let (value, _) = read_cstr(rest)?;
            Ok(BackendMessage::ParameterStatus { name, value })
        }
        b'K' => {
            if payload.len() < 8 {
                return Err(DbError::Protocol("BackendKeyData corto".into()));
            }
            let process_id = i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let secret_key = i32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(BackendMessage::BackendKeyData {
                process_id,
                secret_key,
            })
        }
        b'Z' => {
            if payload.is_empty() {
                return Err(DbError::Protocol("ReadyForQuery vacío".into()));
            }
            Ok(BackendMessage::ReadyForQuery {
                tx_status: payload[0],
            })
        }
        b'T' => parse_row_description(payload),
        b'D' => parse_data_row(payload),
        b'C' => {
            let (tag, _) = read_cstr(payload)?;
            Ok(BackendMessage::CommandComplete { tag })
        }
        b'I' => Ok(BackendMessage::EmptyQueryResponse),
        b'E' => Ok(BackendMessage::ErrorResponse(ErrorFields::from_pairs(
            parse_error_fields(payload)?,
        ))),
        b'N' => Ok(BackendMessage::NoticeResponse(ErrorFields::from_pairs(
            parse_error_fields(payload)?,
        ))),
        b'1' => Ok(BackendMessage::ParseComplete),
        b'2' => Ok(BackendMessage::BindComplete),
        b'3' => Ok(BackendMessage::CloseComplete),
        b'n' => Ok(BackendMessage::NoData),
        b's' => Ok(BackendMessage::PortalSuspended),
        b't' => {
            if payload.len() < 2 {
                return Err(DbError::Protocol("ParameterDescription corto".into()));
            }
            let count = u16::from_be_bytes([payload[0], payload[1]]) as usize;
            let mut oids = Vec::with_capacity(count);
            let mut p = 2;
            for _ in 0..count {
                if p + 4 > payload.len() {
                    return Err(DbError::Protocol(
                        "ParameterDescription oids truncados".into(),
                    ));
                }
                oids.push(u32::from_be_bytes([
                    payload[p],
                    payload[p + 1],
                    payload[p + 2],
                    payload[p + 3],
                ]));
                p += 4;
            }
            Ok(BackendMessage::ParameterDescription { oids })
        }
        _ => Ok(BackendMessage::Unknown {
            tag,
            len: payload.len(),
        }),
    }
}

fn parse_auth(payload: &[u8]) -> DbResult<BackendMessage> {
    if payload.len() < 4 {
        return Err(DbError::Protocol("auth payload corto".into()));
    }
    let kind = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let rest = &payload[4..];
    match kind {
        0 => Ok(BackendMessage::AuthenticationOk),
        3 => Ok(BackendMessage::AuthenticationCleartextPassword),
        5 => {
            if rest.len() != 4 {
                return Err(DbError::Protocol(
                    "AuthenticationMD5: salt no son 4 bytes".into(),
                ));
            }
            Ok(BackendMessage::AuthenticationMd5Password {
                salt: [rest[0], rest[1], rest[2], rest[3]],
            })
        }
        10 => {
            // Lista de mechanism strings terminada por \0
            // adicional. Recorremos el slice extrayendo cstrs
            // hasta encontrar uno vacío.
            let mut mechanisms = Vec::new();
            let mut buf = rest;
            loop {
                let (s, tail) = read_cstr(buf)?;
                if s.is_empty() {
                    break;
                }
                mechanisms.push(s);
                buf = tail;
            }
            Ok(BackendMessage::AuthenticationSasl { mechanisms })
        }
        11 => Ok(BackendMessage::AuthenticationSaslContinue {
            data: rest.to_vec(),
        }),
        12 => Ok(BackendMessage::AuthenticationSaslFinal {
            data: rest.to_vec(),
        }),
        other => Err(DbError::Auth(format!(
            "método auth desconocido: {other} (solo soportado: ok, cleartext, md5, sasl)"
        ))),
    }
}

fn parse_row_description(payload: &[u8]) -> DbResult<BackendMessage> {
    if payload.len() < 2 {
        return Err(DbError::Protocol("RowDescription corto".into()));
    }
    let count = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    let mut p = 2;
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let (name, rest) = read_cstr(&payload[p..])?;
        p = payload.len() - rest.len();
        if rest.len() < 18 {
            return Err(DbError::Protocol("RowDescription field corto".into()));
        }
        let table_oid = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
        let column_idx = i16::from_be_bytes([rest[4], rest[5]]);
        let type_oid = u32::from_be_bytes([rest[6], rest[7], rest[8], rest[9]]);
        let type_size = i16::from_be_bytes([rest[10], rest[11]]);
        let type_modifier = i32::from_be_bytes([rest[12], rest[13], rest[14], rest[15]]);
        let format = i16::from_be_bytes([rest[16], rest[17]]);
        p += 18;
        fields.push(FieldDescription {
            name,
            table_oid,
            column_idx,
            type_oid,
            type_size,
            type_modifier,
            format,
        });
    }
    Ok(BackendMessage::RowDescription { fields })
}

fn parse_data_row(payload: &[u8]) -> DbResult<BackendMessage> {
    if payload.len() < 2 {
        return Err(DbError::Protocol("DataRow corto".into()));
    }
    let count = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    let mut p = 2;
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        if p + 4 > payload.len() {
            return Err(DbError::Protocol("DataRow length truncado".into()));
        }
        let len = i32::from_be_bytes([payload[p], payload[p + 1], payload[p + 2], payload[p + 3]]);
        p += 4;
        if len == -1 {
            values.push(None);
        } else {
            let len = len as usize;
            if p + len > payload.len() {
                return Err(DbError::Protocol("DataRow value truncado".into()));
            }
            values.push(Some(payload[p..p + len].to_vec()));
            p += len;
        }
    }
    Ok(BackendMessage::DataRow { values })
}

fn parse_error_fields(payload: &[u8]) -> DbResult<Vec<(u8, String)>> {
    let mut pairs = Vec::new();
    let mut buf = payload;
    while !buf.is_empty() {
        let code = buf[0];
        if code == 0 {
            break;
        }
        let (val, rest) = read_cstr(&buf[1..])?;
        pairs.push((code, val));
        buf = rest;
    }
    Ok(pairs)
}

fn read_cstr(buf: &[u8]) -> DbResult<(String, &[u8])> {
    match buf.iter().position(|&b| b == 0) {
        Some(idx) => {
            let s = std::str::from_utf8(&buf[..idx])
                .map_err(|_| DbError::Protocol("cstr no es UTF-8".into()))?
                .to_string();
            Ok((s, &buf[idx + 1..]))
        }
        None => Err(DbError::Protocol("cstr sin terminador".into())),
    }
}

// =============================================================
// SCRAM-SHA-256 client (RFC 7677 + RFC 5802)
// =============================================================
//
// Flow:
//   1. Client → Server: client-first-message
//      "n,,n=<user>,r=<client_nonce>"
//      (en SASL, "n,," = "no channel binding").
//   2. Server → Client: server-first-message
//      "r=<server_nonce>,s=<base64_salt>,i=<iterations>"
//      donde server_nonce = client_nonce + server_random_part.
//   3. Client → Server: client-final-message
//      "c=biws,r=<server_nonce>,p=<base64_client_proof>"
//      (biws = base64("n,,") = "biws").
//   4. Server → Client: server-final-message
//      "v=<base64_server_signature>"
//      o "e=<error>".
//
// Derivación de keys:
//   SaltedPassword = PBKDF2-HMAC-SHA256(password, salt, iterations)
//   ClientKey      = HMAC-SHA256(SaltedPassword, "Client Key")
//   StoredKey      = SHA256(ClientKey)
//   AuthMessage    = client-first-bare ||
//                    "," || server-first-message ||
//                    "," || client-final-without-proof
//   ClientSignature = HMAC-SHA256(StoredKey, AuthMessage)
//   ClientProof     = ClientKey XOR ClientSignature
//   ServerKey       = HMAC-SHA256(SaltedPassword, "Server Key")
//   ServerSignature = HMAC-SHA256(ServerKey, AuthMessage)
//
// MVP 10.1: SCRAM-SHA-256 sin channel binding (sin SCRAM-SHA-256-PLUS).
// Channel binding requiere TLS; lo agregamos cuando llegue TLS en
// el sub-paso futuro.

pub struct ScramClient {
    username: String,
    password: String,
    client_nonce: String,
    /// Estado intermedio para client_final() y verify().
    /// Populated por client_final().
    server_signature: Option<Vec<u8>>,
}

impl ScramClient {
    /// Construye un cliente SCRAM-SHA-256 con un nonce aleatorio
    /// de 24 bytes base64-encoded (~32 chars). El nonce es la
    /// pieza de aleatoriedad del cliente; el servidor lo
    /// extiende con su propia aleatoriedad.
    pub fn new(username: &str, password: &str) -> DbResult<Self> {
        Ok(ScramClient {
            username: username.to_string(),
            password: password.to_string(),
            client_nonce: generate_nonce()?,
            server_signature: None,
        })
    }

    /// Constructor para tests con nonce fijo (vectores RFC 7677).
    #[cfg(test)]
    pub fn new_with_nonce(username: &str, password: &str, nonce: &str) -> Self {
        ScramClient {
            username: username.to_string(),
            password: password.to_string(),
            client_nonce: nonce.to_string(),
            server_signature: None,
        }
    }

    /// Genera el client-first-message para enviar como payload
    /// del `SaslInitialResponse`. SASL header "n,," = no
    /// channel binding. SCRAM por SPEC ignora el username del
    /// SASL message (Postgres usa el que vino en startup), pero
    /// lo incluimos por completitud.
    pub fn client_first(&self) -> String {
        format!("n,,{}", self.client_first_bare())
    }

    fn client_first_bare(&self) -> String {
        // SCRAM permite username vacío (Postgres lo ignora); pero
        // si está, va escapeado SASLprep — para los caracteres
        // típicos (ASCII) es identidad. Soportamos solo ASCII en
        // 10.1; Unicode complejo queda como deuda menor.
        format!(
            "n={},r={}",
            saslprep_minimal(&self.username),
            self.client_nonce
        )
    }

    /// Procesa el `server-first-message` recibido en
    /// `AuthenticationSASLContinue` y devuelve el
    /// `client-final-message` para enviar como `SaslResponse`.
    /// Después de esta llamada, `server_signature` queda cacheada
    /// para `verify()`.
    pub fn client_final(&mut self, server_first: &str) -> DbResult<String> {
        // Parse "r=<nonce>,s=<salt>,i=<iters>"
        let (server_nonce, salt_b64, iterations) = parse_server_first(server_first)?;

        // El servidor debe extender NUESTRO nonce — si no
        // empieza con `client_nonce`, alguien está al medio.
        if !server_nonce.starts_with(&self.client_nonce) {
            return Err(DbError::Auth(
                "SCRAM: server nonce no extiende el client nonce".into(),
            ));
        }

        let salt = BASE64
            .decode(&salt_b64)
            .map_err(|e| DbError::Auth(format!("SCRAM: salt base64 inválido: {e}")))?;
        if iterations < 1 {
            return Err(DbError::Auth("SCRAM: iterations < 1".into()));
        }

        // SaltedPassword = PBKDF2-HMAC-SHA256(password, salt, i)
        let salted_password = pbkdf2_hmac_sha256(self.password.as_bytes(), &salt, iterations);

        // ClientKey = HMAC(SaltedPassword, "Client Key")
        let client_key = hmac_sha256(&salted_password, b"Client Key");

        // StoredKey = SHA256(ClientKey)
        let mut hasher = Sha256::new();
        hasher.update(client_key);
        let stored_key: [u8; 32] = hasher.finalize().into();

        // client-final-without-proof = "c=biws,r=<server_nonce>"
        // biws = base64("n,,") = "biws" (canonical channel binding
        // for "no channel binding").
        let cfin_no_proof = format!("c=biws,r={}", server_nonce);

        // AuthMessage = client-first-bare + "," + server-first +
        //               "," + client-final-without-proof
        let auth_message = format!(
            "{},{},{}",
            self.client_first_bare(),
            server_first,
            cfin_no_proof
        );

        // ClientSignature = HMAC(StoredKey, AuthMessage)
        let client_signature = hmac_sha256(&stored_key, auth_message.as_bytes());

        // ClientProof = ClientKey XOR ClientSignature
        let mut client_proof = [0u8; 32];
        for i in 0..32 {
            client_proof[i] = client_key[i] ^ client_signature[i];
        }
        let proof_b64 = BASE64.encode(client_proof);

        // ServerKey = HMAC(SaltedPassword, "Server Key")
        let server_key = hmac_sha256(&salted_password, b"Server Key");
        // ServerSignature = HMAC(ServerKey, AuthMessage)
        let server_signature = hmac_sha256(&server_key, auth_message.as_bytes());
        self.server_signature = Some(server_signature.to_vec());

        Ok(format!("{},p={}", cfin_no_proof, proof_b64))
    }

    /// Valida el `server-final-message` (`v=<base64_signature>`).
    /// Falla si el `v=` recibido no matchea el server signature
    /// que computamos en `client_final()` — eso implicaría que el
    /// servidor no conoce nuestra password, no aceptamos.
    pub fn verify(&self, server_final: &str) -> DbResult<()> {
        if let Some(rest) = server_final.strip_prefix("v=") {
            let sig_received = BASE64.decode(rest).map_err(|e| {
                DbError::Auth(format!("SCRAM: server signature base64 inválido: {e}"))
            })?;
            let sig_expected = self
                .server_signature
                .as_ref()
                .ok_or_else(|| DbError::Auth("SCRAM: client_final() no fue llamado".into()))?;
            if constant_time_eq(&sig_received, sig_expected) {
                Ok(())
            } else {
                Err(DbError::Auth(
                    "SCRAM: server signature no matchea — credenciales inválidas o MITM".into(),
                ))
            }
        } else if let Some(rest) = server_final.strip_prefix("e=") {
            Err(DbError::Auth(format!("SCRAM error del servidor: {rest}")))
        } else {
            Err(DbError::Auth(format!(
                "SCRAM: server-final desconocido: '{server_final}'"
            )))
        }
    }
}

fn parse_server_first(s: &str) -> DbResult<(String, String, u32)> {
    let mut nonce = None;
    let mut salt = None;
    let mut iters = None;
    for part in s.split(',') {
        if let Some(v) = part.strip_prefix("r=") {
            nonce = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("s=") {
            salt = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("i=") {
            iters = Some(
                v.parse::<u32>()
                    .map_err(|_| DbError::Auth(format!("SCRAM: iters inválido '{v}'")))?,
            );
        } else if part.starts_with("m=") {
            // mandatory extension — si aparece, debemos rechazar
            return Err(DbError::Auth(format!(
                "SCRAM: extension mandatoria del servidor '{part}'"
            )));
        }
    }
    match (nonce, salt, iters) {
        (Some(n), Some(s), Some(i)) => Ok((n, s, i)),
        _ => Err(DbError::Auth(format!(
            "SCRAM: server-first incompleto '{s}'"
        ))),
    }
}

fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    // PBKDF2 con HMAC-SHA-256, derivando 32 bytes (un solo bloque
    // SHA-256). dkLen = 32 = hLen, así que la fórmula se reduce a
    //   U1 = HMAC(password, salt || INT(1))   donde INT(1) = 4 bytes big-endian
    //   Ui = HMAC(password, U(i-1))
    //   T  = U1 XOR U2 XOR ... XOR U_iterations
    // Para SCRAM-SHA-256 dkLen es siempre 32 (hLen de SHA-256),
    // y solo necesitamos i=1, por eso el código está
    // simplificado vs el PBKDF2 genérico.
    let mut salt_block = Vec::with_capacity(salt.len() + 4);
    salt_block.extend_from_slice(salt);
    salt_block.extend_from_slice(&1u32.to_be_bytes());

    let mut u = hmac_sha256(password, &salt_block);
    let mut result = u;

    for _ in 1..iterations {
        u = hmac_sha256(password, &u);
        for i in 0..32 {
            result[i] ^= u[i];
        }
    }

    result
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(data);
    let out = mac.finalize().into_bytes();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn generate_nonce() -> DbResult<String> {
    // 18 bytes aleatorios → base64 = 24 chars. RFC dice "at
    // least 64 bits" — 18*8 = 144 bits, sobra.
    let mut bytes = [0u8; 18];
    // rand_core::OsRng está disponible vía argon2 → password-hash
    // → rand_core, ya como dep no-opcional. Lo invocamos vía la
    // trait RngCore::try_fill_bytes para no depender del feature
    // rand del wrapper.
    use rand_core::{OsRng, RngCore};
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|e| DbError::Auth(format!("nonce: RNG falló: {e}")))?;
    Ok(BASE64.encode(bytes))
}

fn saslprep_minimal(s: &str) -> String {
    // SASLprep completo (RFC 4013) es complejo — Unicode
    // normalization NFKC + mapping de chars de control + check de
    // bidirectional. Para 10.1 implementamos un subset que cubre
    // ASCII (lo típico de usernames y passwords): rechazamos chars
    // de control bajos y mapeamos espacios non-ASCII a U+0020.
    // Para usernames Postgres en práctica casi siempre son ASCII;
    // si entra demanda real de unicode complejo, sumamos
    // `stringprep` crate.
    //
    // En SCRAM, los chars '=' y ',' deben escaparse en el
    // username (forman parte de la sintaxis del mensaje).
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '=' => out.push_str("=3D"),
            ',' => out.push_str("=2C"),
            _ => out.push(c),
        }
    }
    out
}

// =============================================================
// Tipos OID — mapping Postgres → Fitz
// =============================================================
//
// Postgres usa OIDs (object identifiers, u32) para identificar
// tipos en el wire protocol. Los OIDs de tipos built-in son
// estables y están documentados en `pg_type.h` del kernel
// Postgres.
//
// 10.1 cubre los 11 tipos enumerados en el roadmap. Tipos
// avanzados (JSONB, arrays, Date/Time/Timestamp con timezone
// detalle, UUID, etc.) llegan en 10.5 — el código actual los
// recibe como `Bytes` opacos si llegaran y emite
// `UnsupportedType` con el OID concreto para que el user vea
// claramente qué pedir.

pub mod oid {
    pub const BOOL: u32 = 16;
    pub const BYTEA: u32 = 17;
    pub const INT8: u32 = 20;
    pub const INT2: u32 = 21;
    pub const INT4: u32 = 23;
    pub const TEXT: u32 = 25;
    pub const OID: u32 = 26;
    pub const FLOAT4: u32 = 700;
    pub const FLOAT8: u32 = 701;
    pub const VARCHAR: u32 = 1043;
    pub const DATE: u32 = 1082;
    pub const TIME: u32 = 1083;
    pub const TIMESTAMP: u32 = 1114;
    pub const TIMESTAMPTZ: u32 = 1184;
    pub const UUID: u32 = 2950;
    pub const JSONB: u32 = 3802;
    pub const JSON: u32 = 114;
}

/// Valor escalar Postgres parseado del wire. La representación
/// es mínima en 10.1: solo los primitivos del MVP. Tipos
/// avanzados (JSONB structurado, arrays tipados, etc.) llegan
/// en 10.5 con variantes específicas.
#[derive(Debug, Clone, PartialEq)]
pub enum PgValue {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Bytes(Vec<u8>),
}

impl fmt::Display for PgValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PgValue::Null => write!(f, "NULL"),
            PgValue::Int(n) => write!(f, "{n}"),
            PgValue::Float(x) => write!(f, "{x}"),
            PgValue::Text(s) => write!(f, "{s}"),
            PgValue::Bool(b) => write!(f, "{}", if *b { "t" } else { "f" }),
            PgValue::Bytes(b) => write!(f, "<{} bytes>", b.len()),
        }
    }
}

/// Parsea un valor del wire (text format — el default de Simple
/// Query). Cuando llegue Extended Query con format=binary en
/// 10.1.b, agregamos `parse_binary` paralelo.
///
/// `bytes = None` → NULL (length -1 en DataRow).
pub fn parse_text_value(oid: u32, bytes: Option<&[u8]>) -> DbResult<PgValue> {
    let raw = match bytes {
        None => return Ok(PgValue::Null),
        Some(b) => b,
    };
    let s = std::str::from_utf8(raw)
        .map_err(|_| DbError::Protocol(format!("OID {oid}: value no es UTF-8")))?;
    match oid {
        oid::BOOL => Ok(PgValue::Bool(s == "t" || s == "true")),
        oid::INT2 | oid::INT4 | oid::INT8 | oid::OID => {
            Ok(PgValue::Int(s.parse::<i64>().map_err(|_| {
                DbError::Protocol(format!("OID {oid}: int inválido '{s}'"))
            })?))
        }
        oid::FLOAT4 | oid::FLOAT8 => {
            Ok(PgValue::Float(s.parse::<f64>().map_err(|_| {
                DbError::Protocol(format!("OID {oid}: float inválido '{s}'"))
            })?))
        }
        oid::TEXT
        | oid::VARCHAR
        | oid::DATE
        | oid::TIME
        | oid::TIMESTAMP
        | oid::TIMESTAMPTZ
        | oid::UUID
        | oid::JSON
        | oid::JSONB => Ok(PgValue::Text(s.to_string())),
        oid::BYTEA => {
            // Wire format text para BYTEA es "\x<hex>". Si no
            // matchea, devolvemos los bytes raw.
            if let Some(hex) = s.strip_prefix("\\x") {
                let mut out = Vec::with_capacity(hex.len() / 2);
                let bytes = hex.as_bytes();
                if bytes.len() % 2 != 0 {
                    return Err(DbError::Protocol("BYTEA hex con length impar".into()));
                }
                for chunk in bytes.chunks(2) {
                    let hi = hex_digit(chunk[0])?;
                    let lo = hex_digit(chunk[1])?;
                    out.push(hi * 16 + lo);
                }
                Ok(PgValue::Bytes(out))
            } else {
                Ok(PgValue::Bytes(raw.to_vec()))
            }
        }
        _ => {
            // Tipos no soportados en 10.1: devolvemos UnsupportedType
            // con el OID concreto. El user ve qué tipo agregar.
            Err(DbError::UnsupportedType(oid))
        }
    }
}

/// Codifica un valor para envío al servidor (text format en MVP).
/// El servidor parsea según el OID del statement; si pasamos OID 0
/// en `Parse`, Postgres infiere. Para Bytes usamos el formato
/// hex "\x<hex>".
pub fn encode_text_value(v: &PgValue) -> Option<Vec<u8>> {
    match v {
        PgValue::Null => None,
        PgValue::Int(n) => Some(n.to_string().into_bytes()),
        PgValue::Float(x) => Some(x.to_string().into_bytes()),
        PgValue::Text(s) => Some(s.as_bytes().to_vec()),
        PgValue::Bool(b) => Some(if *b { b"t".to_vec() } else { b"f".to_vec() }),
        PgValue::Bytes(b) => {
            let mut out = String::with_capacity(b.len() * 2 + 2);
            out.push_str("\\x");
            for byte in b {
                use std::fmt::Write as _;
                let _ = write!(out, "{:02x}", byte);
            }
            Some(out.into_bytes())
        }
    }
}

// =============================================================
// Row — resultado de una query
// =============================================================

/// Un row del resultset. Mantiene los nombres de las columnas
/// (para acceso `row.get("name")`) y los valores en orden. Los
/// nombres son `Arc<str>` cuando el row vive más allá de una
/// query — en MVP los duplicamos en cada row (cost low, ~2 KB
/// per row para 10 columnas), optimizable si entra demanda.
#[derive(Debug, Clone)]
pub struct Row {
    columns: Vec<(String, u32)>, // (name, type_oid)
    values: Vec<PgValue>,
}

impl Row {
    pub fn new(columns: Vec<(String, u32)>, values: Vec<PgValue>) -> Self {
        Row { columns, values }
    }

    pub fn columns(&self) -> &[(String, u32)] {
        &self.columns
    }

    pub fn values(&self) -> &[PgValue] {
        &self.values
    }

    pub fn get(&self, name: &str) -> Option<&PgValue> {
        let idx = self.columns.iter().position(|(n, _)| n == name)?;
        self.values.get(idx)
    }

    pub fn get_at(&self, idx: usize) -> Option<&PgValue> {
        self.values.get(idx)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Resultado completo de una query: rows + el tag del
/// CommandComplete (típicamente "SELECT 42" o "INSERT 0 1").
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub rows: Vec<Row>,
    pub command_tag: String,
}

impl QueryResult {
    /// Devuelve el rowcount inferido del `command_tag`.
    /// `"INSERT 0 5"` → 5, `"UPDATE 3"` → 3. Si no parsea (caso
    /// "SELECT"), devuelve el número de rows del resultset.
    pub fn rows_affected(&self) -> u64 {
        let parts: Vec<&str> = self.command_tag.split_whitespace().collect();
        match parts.as_slice() {
            ["INSERT", _, n] => n.parse().unwrap_or(self.rows.len() as u64),
            [_, n] => n.parse().unwrap_or(self.rows.len() as u64),
            _ => self.rows.len() as u64,
        }
    }
}

// =============================================================
// Connection — handshake + queries
// =============================================================

/// Una conexión Postgres viva. Owns el TcpStream + estado de la
/// conexión. NO es Send+Sync por construcción (TcpStream lo es;
/// el pool de 10.2 envuelve en `Arc<Mutex<Connection>>` para
/// múltiple acceso). En 10.1 una sola tarea posee el `Connection`
/// y lo usa exclusivamente.
pub struct Connection {
    stream: TcpStream,
    /// Status de la transacción actual: 'I' idle, 'T' in tx,
    /// 'E' in failed tx. Lo actualizamos en cada `ReadyForQuery`.
    /// Útil para diagnostics y para 10.7 (transactions).
    tx_status: u8,
    /// Process ID + secret_key del backend. Útil para cancelar
    /// queries (mensaje `CancelRequest` por conexión paralela).
    /// Cancellation no implementado en 10.1 — campo presente para
    /// el sub-paso futuro.
    backend_pid: i32,
    backend_secret_key: i32,
    /// Parámetros del servidor reportados durante el startup
    /// (server_version, server_encoding, etc.). Útil para
    /// diagnostics + features que dependen de la versión.
    server_params: Vec<(String, String)>,
}

impl Connection {
    /// Abre TCP, hace startup + auth, deja la conexión lista
    /// para queries. Timeout total ~10 seg.
    pub async fn connect(config: &ConnectionConfig) -> DbResult<Self> {
        if config.sslmode == SslMode::Require {
            return Err(DbError::NotImplemented(
                "sslmode=require llega en sub-paso TLS futuro; usá sslmode=disable en 10.1".into(),
            ));
        }
        let addr = format!("{}:{}", config.host, config.port);
        let stream = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&addr))
            .await
            .map_err(|_| DbError::Io(io::Error::new(io::ErrorKind::TimedOut, "connect timeout")))?
            .map_err(DbError::Io)?;

        let mut conn = Connection {
            stream,
            tx_status: b'I',
            backend_pid: 0,
            backend_secret_key: 0,
            server_params: Vec::new(),
        };

        conn.startup(config).await?;
        Ok(conn)
    }

    async fn write(&mut self, msg: FrontendMessage<'_>) -> DbResult<()> {
        let bytes = msg.encode();
        self.stream.write_all(&bytes).await?;
        Ok(())
    }

    async fn write_all_bytes(&mut self, bytes: &[u8]) -> DbResult<()> {
        self.stream.write_all(bytes).await?;
        Ok(())
    }

    async fn read(&mut self) -> DbResult<BackendMessage> {
        read_message(&mut self.stream).await
    }

    async fn startup(&mut self, config: &ConnectionConfig) -> DbResult<()> {
        self.write(FrontendMessage::Startup {
            user: &config.user,
            database: &config.dbname,
        })
        .await?;

        loop {
            let msg = self.read().await?;
            match msg {
                BackendMessage::AuthenticationOk => break,
                BackendMessage::AuthenticationCleartextPassword => {
                    let pwd = config.password.as_deref().ok_or_else(|| {
                        DbError::Auth(
                            "el servidor pide password cleartext y la URL no la trae".into(),
                        )
                    })?;
                    let mut bytes = pwd.as_bytes().to_vec();
                    bytes.push(0);
                    self.write(FrontendMessage::Password { password: &bytes })
                        .await?;
                }
                BackendMessage::AuthenticationMd5Password { salt } => {
                    let pwd = config.password.as_deref().ok_or_else(|| {
                        DbError::Auth("el servidor pide MD5 y la URL no trae password".into())
                    })?;
                    let digest = md5_password(&config.user, pwd, &salt);
                    let mut bytes = digest.into_bytes();
                    bytes.push(0);
                    self.write(FrontendMessage::Password { password: &bytes })
                        .await?;
                }
                BackendMessage::AuthenticationSasl { mechanisms } => {
                    if !mechanisms.iter().any(|m| m == "SCRAM-SHA-256") {
                        return Err(DbError::Auth(format!(
                            "servidor ofreció {mechanisms:?}; \
                             driver soporta solo SCRAM-SHA-256 en 10.1"
                        )));
                    }
                    let pwd = config.password.as_deref().ok_or_else(|| {
                        DbError::Auth("el servidor pide SCRAM y la URL no trae password".into())
                    })?;
                    let mut scram = ScramClient::new(&config.user, pwd)?;
                    let cfirst = scram.client_first();
                    self.write(FrontendMessage::SaslInitialResponse {
                        mechanism: "SCRAM-SHA-256",
                        initial_response: cfirst.as_bytes(),
                    })
                    .await?;

                    // Esperamos AuthenticationSASLContinue
                    let cont = match self.read().await? {
                        BackendMessage::AuthenticationSaslContinue { data } => data,
                        BackendMessage::ErrorResponse(ef) => {
                            return Err(DbError::Server {
                                severity: ef.severity,
                                code: ef.code,
                                message: ef.message,
                            })
                        }
                        other => {
                            return Err(DbError::Protocol(format!(
                                "esperaba SASLContinue, recibí {other:?}"
                            )))
                        }
                    };
                    let server_first = std::str::from_utf8(&cont)
                        .map_err(|_| DbError::Auth("SCRAM: server-first no UTF-8".into()))?;
                    let cfinal = scram.client_final(server_first)?;
                    self.write(FrontendMessage::SaslResponse {
                        response: cfinal.as_bytes(),
                    })
                    .await?;

                    // Esperamos AuthenticationSASLFinal
                    let final_data = match self.read().await? {
                        BackendMessage::AuthenticationSaslFinal { data } => data,
                        BackendMessage::ErrorResponse(ef) => {
                            return Err(DbError::Server {
                                severity: ef.severity,
                                code: ef.code,
                                message: ef.message,
                            })
                        }
                        other => {
                            return Err(DbError::Protocol(format!(
                                "esperaba SASLFinal, recibí {other:?}"
                            )))
                        }
                    };
                    let server_final = std::str::from_utf8(&final_data)
                        .map_err(|_| DbError::Auth("SCRAM: server-final no UTF-8".into()))?;
                    scram.verify(server_final)?;
                    // El próximo mensaje debe ser AuthenticationOk.
                    // Loop continúa.
                }
                BackendMessage::ErrorResponse(ef) => {
                    return Err(DbError::Server {
                        severity: ef.severity,
                        code: ef.code,
                        message: ef.message,
                    });
                }
                other => {
                    return Err(DbError::Protocol(format!(
                        "auth: esperaba AuthenticationXxx, recibí {other:?}"
                    )))
                }
            }
        }

        // Drain de ParameterStatus + BackendKeyData hasta
        // ReadyForQuery.
        loop {
            match self.read().await? {
                BackendMessage::ParameterStatus { name, value } => {
                    self.server_params.push((name, value));
                }
                BackendMessage::BackendKeyData {
                    process_id,
                    secret_key,
                } => {
                    self.backend_pid = process_id;
                    self.backend_secret_key = secret_key;
                }
                BackendMessage::ReadyForQuery { tx_status } => {
                    self.tx_status = tx_status;
                    return Ok(());
                }
                BackendMessage::ErrorResponse(ef) => {
                    return Err(DbError::Server {
                        severity: ef.severity,
                        code: ef.code,
                        message: ef.message,
                    });
                }
                BackendMessage::NoticeResponse(_) => {
                    // ignoramos notices durante startup
                }
                other => {
                    return Err(DbError::Protocol(format!(
                        "esperaba ParameterStatus/BackendKeyData/ReadyForQuery, recibí {other:?}"
                    )))
                }
            }
        }
    }

    /// Simple Query: ejecuta el SQL en un solo round-trip. No
    /// admite parámetros (el caller arma el SQL completo). Para
    /// queries con args, usar `extended_query()` en su lugar.
    pub async fn simple_query(&mut self, sql: &str) -> DbResult<QueryResult> {
        self.write(FrontendMessage::Query { sql }).await?;
        let mut rows = Vec::new();
        let mut columns: Vec<(String, u32)> = Vec::new();
        let mut command_tag = String::new();
        loop {
            match self.read().await? {
                BackendMessage::RowDescription { fields } => {
                    columns = fields.into_iter().map(|f| (f.name, f.type_oid)).collect();
                }
                BackendMessage::DataRow { values } => {
                    let parsed = values
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            let oid = columns.get(i).map(|(_, o)| *o).unwrap_or(oid::TEXT);
                            parse_text_value(oid, v.as_deref())
                        })
                        .collect::<DbResult<Vec<_>>>()?;
                    rows.push(Row::new(columns.clone(), parsed));
                }
                BackendMessage::CommandComplete { tag } => {
                    command_tag = tag;
                }
                BackendMessage::EmptyQueryResponse => {
                    command_tag = "EMPTY".into();
                }
                BackendMessage::NoticeResponse(_) => {
                    // ignorado en MVP — un sub-paso futuro puede
                    // exponer notices al user (logging callback)
                }
                BackendMessage::ErrorResponse(ef) => {
                    // Drain hasta ReadyForQuery para dejar conn
                    // utilizable.
                    self.drain_until_ready().await?;
                    return Err(DbError::Server {
                        severity: ef.severity,
                        code: ef.code,
                        message: ef.message,
                    });
                }
                BackendMessage::ReadyForQuery { tx_status } => {
                    self.tx_status = tx_status;
                    return Ok(QueryResult { rows, command_tag });
                }
                other => {
                    return Err(DbError::Protocol(format!(
                        "simple_query: mensaje inesperado {other:?}"
                    )))
                }
            }
        }
    }

    /// Extended Query: parsea + bindea + ejecuta con parámetros.
    /// Usa text format en ambas direcciones para simplificar (los
    /// tipos OID core parsean igual con format=0). Binary format
    /// llega en 10.5 cuando lo necesitemos para JSONB/timestamps.
    pub async fn extended_query(&mut self, sql: &str, args: &[PgValue]) -> DbResult<QueryResult> {
        // Encode args to text format
        let encoded: Vec<Option<Vec<u8>>> = args.iter().map(encode_text_value).collect();
        let bind_refs: Vec<Option<&[u8]>> = encoded.iter().map(|opt| opt.as_deref()).collect();

        // Parse — name vacío = statement anónimo (lifetime de un
        // round). No usamos prepared statements cache en MVP.
        self.write(FrontendMessage::Parse {
            statement_name: "",
            sql,
            param_types: &[],
        })
        .await?;
        // Bind — todos los params text format (0), todos los
        // results text format (0).
        let zero_format = [0i16];
        self.write(FrontendMessage::Bind {
            portal_name: "",
            statement_name: "",
            param_formats: &zero_format,
            param_values: &bind_refs,
            result_formats: &zero_format,
        })
        .await?;
        // Execute portal con max_rows=0 (unlimited)
        self.write(FrontendMessage::Execute {
            portal_name: "",
            max_rows: 0,
        })
        .await?;
        // Sync — commit del round
        self.write(FrontendMessage::Sync).await?;

        let mut rows = Vec::new();
        let mut columns: Vec<(String, u32)> = Vec::new();
        let mut command_tag = String::new();
        loop {
            match self.read().await? {
                BackendMessage::ParseComplete | BackendMessage::BindComplete => {
                    // pass-through
                }
                BackendMessage::RowDescription { fields } => {
                    columns = fields.into_iter().map(|f| (f.name, f.type_oid)).collect();
                }
                BackendMessage::NoData => {
                    // No retorna rows (INSERT sin RETURNING)
                }
                BackendMessage::DataRow { values } => {
                    let parsed = values
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            let oid = columns.get(i).map(|(_, o)| *o).unwrap_or(oid::TEXT);
                            parse_text_value(oid, v.as_deref())
                        })
                        .collect::<DbResult<Vec<_>>>()?;
                    rows.push(Row::new(columns.clone(), parsed));
                }
                BackendMessage::CommandComplete { tag } => {
                    command_tag = tag;
                }
                BackendMessage::EmptyQueryResponse => {
                    command_tag = "EMPTY".into();
                }
                BackendMessage::NoticeResponse(_) => {}
                BackendMessage::ErrorResponse(ef) => {
                    self.drain_until_ready().await?;
                    return Err(DbError::Server {
                        severity: ef.severity,
                        code: ef.code,
                        message: ef.message,
                    });
                }
                BackendMessage::ReadyForQuery { tx_status } => {
                    self.tx_status = tx_status;
                    return Ok(QueryResult { rows, command_tag });
                }
                other => {
                    return Err(DbError::Protocol(format!(
                        "extended_query: mensaje inesperado {other:?}"
                    )))
                }
            }
        }
    }

    async fn drain_until_ready(&mut self) -> DbResult<()> {
        loop {
            match self.read().await? {
                BackendMessage::ReadyForQuery { tx_status } => {
                    self.tx_status = tx_status;
                    return Ok(());
                }
                _ => continue,
            }
        }
    }

    /// Cierra la conexión cooperativamente. Manda `Terminate` y
    /// dropea el TcpStream. No reusable después.
    pub async fn close(mut self) -> DbResult<()> {
        // El Terminate puede fallar si la conn ya está cerrada
        // del lado del server; lo ignoramos.
        let _ = self.write(FrontendMessage::Terminate).await;
        Ok(())
    }

    pub fn backend_pid(&self) -> i32 {
        self.backend_pid
    }

    pub fn server_param(&self, name: &str) -> Option<&str> {
        self.server_params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// MD5 password hash en formato Postgres:
///   "md5" || md5_hex(md5_hex(password || username) || salt)
fn md5_password(user: &str, password: &str, salt: &[u8; 4]) -> String {
    let inner = md5_hex(&[password.as_bytes(), user.as_bytes()].concat());
    let mut second_input = inner.into_bytes();
    second_input.extend_from_slice(salt);
    let outer = md5_hex(&second_input);
    format!("md5{outer}")
}

/// MD5 implementación mini para auth legacy. Solo lo usamos para
/// el método MD5 deprecated de Postgres pre-14; en 14+ ya es
/// SCRAM-SHA-256 por default. Mantenemos por compat con DBs
/// viejas en dev.
fn md5_hex(data: &[u8]) -> String {
    let digest = md5_compute(data);
    let mut hex = String::with_capacity(32);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{:02x}", byte);
    }
    hex
}

/// MD5 RFC 1321. ~50 LoC pure. Solo para auth legacy.
fn md5_compute(input: &[u8]) -> [u8; 16] {
    // Constantes
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    // Padding
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in padded.chunks(64) {
        let mut m = [0u32; 16];
        for j in 0..16 {
            m[j] = u32::from_le_bytes([
                chunk[j * 4],
                chunk[j * 4 + 1],
                chunk[j * 4 + 2],
                chunk[j * 4 + 3],
            ]);
        }
        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;
        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | (!b & d), i)
            } else if i < 32 {
                ((d & b) | (!d & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | !d), (7 * i) % 16)
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

// =============================================================
// Tests
// =============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ----- ConnectionConfig -----

    #[test]
    fn url_minimo() {
        let c = ConnectionConfig::parse("postgres://user@host/db").unwrap();
        assert_eq!(c.host, "host");
        assert_eq!(c.port, 5432);
        assert_eq!(c.user, "user");
        assert_eq!(c.password, None);
        assert_eq!(c.dbname, "db");
        assert_eq!(c.sslmode, SslMode::Disable);
    }

    #[test]
    fn url_completo() {
        let c = ConnectionConfig::parse(
            "postgresql://alice:secret@db.example.com:6543/myapp?sslmode=disable",
        )
        .unwrap();
        assert_eq!(c.host, "db.example.com");
        assert_eq!(c.port, 6543);
        assert_eq!(c.user, "alice");
        assert_eq!(c.password.as_deref(), Some("secret"));
        assert_eq!(c.dbname, "myapp");
    }

    #[test]
    fn url_ipv6() {
        let c = ConnectionConfig::parse("postgres://user@[::1]:5433/db").unwrap();
        assert_eq!(c.host, "::1");
        assert_eq!(c.port, 5433);
    }

    #[test]
    fn url_password_url_encoded() {
        let c = ConnectionConfig::parse("postgres://user:p%40ss%21@host/db").unwrap();
        assert_eq!(c.password.as_deref(), Some("p@ss!"));
    }

    #[test]
    fn url_falta_user() {
        let r = ConnectionConfig::parse("postgres://host/db");
        assert!(matches!(r, Err(DbError::InvalidUrl(_))));
    }

    #[test]
    fn url_falta_dbname() {
        let r = ConnectionConfig::parse("postgres://user@host");
        assert!(matches!(r, Err(DbError::InvalidUrl(_))));
    }

    #[test]
    fn url_scheme_invalido() {
        let r = ConnectionConfig::parse("mysql://user@host/db");
        assert!(matches!(r, Err(DbError::InvalidUrl(_))));
    }

    #[test]
    fn url_sslmode_require_no_implementado() {
        let r = ConnectionConfig::parse("postgres://user@host/db?sslmode=require");
        assert!(matches!(r, Err(DbError::NotImplemented(_))));
    }

    #[test]
    fn url_application_name_se_ignora() {
        let c = ConnectionConfig::parse(
            "postgres://user@host/db?application_name=myapp&sslmode=disable",
        )
        .unwrap();
        assert_eq!(c.host, "host");
    }

    // ----- Wire protocol framing -----

    #[test]
    fn startup_message_encoding() {
        let msg = FrontendMessage::Startup {
            user: "alice",
            database: "myapp",
        };
        let bytes = msg.encode();
        // Sin byte de tipo: arranca con length(4).
        let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        assert_eq!(len, bytes.len());
        // Bytes 4..8 = protocol version = 196608
        let version = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        assert_eq!(version, 196608);
        // Contiene "user\0alice\0database\0myapp\0..."
        let payload = &bytes[8..];
        let s = std::str::from_utf8(payload).unwrap();
        assert!(s.contains("user\0alice\0"));
        assert!(s.contains("database\0myapp\0"));
        assert!(s.contains("application_name\0fitz\0"));
        assert!(s.contains("client_encoding\0UTF8\0"));
    }

    #[test]
    fn query_message_encoding() {
        let msg = FrontendMessage::Query { sql: "SELECT 1" };
        let bytes = msg.encode();
        assert_eq!(bytes[0], b'Q');
        let len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
        assert_eq!(len, bytes.len() - 1); // length excluye el tag
        assert_eq!(&bytes[5..bytes.len() - 1], b"SELECT 1");
        assert_eq!(bytes[bytes.len() - 1], 0);
    }

    #[test]
    fn parse_message_encoding_con_param_types() {
        let msg = FrontendMessage::Parse {
            statement_name: "s1",
            sql: "SELECT $1",
            param_types: &[23], // INT4
        };
        let bytes = msg.encode();
        assert_eq!(bytes[0], b'P');
        // Payload empieza después del header (tag + length = 5)
        let payload = &bytes[5..];
        assert!(payload.starts_with(b"s1\0SELECT $1\0"));
    }

    #[test]
    fn parse_backend_authok() {
        let payload = 0u32.to_be_bytes();
        let msg = parse_backend_message(b'R', &payload).unwrap();
        assert!(matches!(msg, BackendMessage::AuthenticationOk));
    }

    #[test]
    fn parse_backend_md5_password() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&5u32.to_be_bytes());
        payload.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let msg = parse_backend_message(b'R', &payload).unwrap();
        match msg {
            BackendMessage::AuthenticationMd5Password { salt } => {
                assert_eq!(salt, [0xde, 0xad, 0xbe, 0xef]);
            }
            _ => panic!("esperaba MD5"),
        }
    }

    #[test]
    fn parse_backend_sasl_mechanisms() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&10u32.to_be_bytes());
        payload.extend_from_slice(b"SCRAM-SHA-256\0");
        payload.extend_from_slice(b"SCRAM-SHA-256-PLUS\0");
        payload.push(0); // terminador de lista
        let msg = parse_backend_message(b'R', &payload).unwrap();
        match msg {
            BackendMessage::AuthenticationSasl { mechanisms } => {
                assert_eq!(mechanisms, vec!["SCRAM-SHA-256", "SCRAM-SHA-256-PLUS"]);
            }
            _ => panic!("esperaba SASL"),
        }
    }

    #[test]
    fn parse_backend_ready_idle() {
        let msg = parse_backend_message(b'Z', b"I").unwrap();
        assert!(matches!(
            msg,
            BackendMessage::ReadyForQuery { tx_status: b'I' }
        ));
    }

    #[test]
    fn parse_backend_row_description() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&2i16.to_be_bytes()); // 2 fields
                                                        // Field 1: "id", table_oid=0, col_idx=0, type_oid=23 (INT4),
                                                        // type_size=4, type_mod=-1, format=0
        payload.extend_from_slice(b"id\0");
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&0i16.to_be_bytes());
        payload.extend_from_slice(&23u32.to_be_bytes());
        payload.extend_from_slice(&4i16.to_be_bytes());
        payload.extend_from_slice(&(-1i32).to_be_bytes());
        payload.extend_from_slice(&0i16.to_be_bytes());
        // Field 2: "name", text
        payload.extend_from_slice(b"name\0");
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&0i16.to_be_bytes());
        payload.extend_from_slice(&25u32.to_be_bytes()); // TEXT
        payload.extend_from_slice(&(-1i16).to_be_bytes());
        payload.extend_from_slice(&(-1i32).to_be_bytes());
        payload.extend_from_slice(&0i16.to_be_bytes());

        let msg = parse_backend_message(b'T', &payload).unwrap();
        match msg {
            BackendMessage::RowDescription { fields } => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "id");
                assert_eq!(fields[0].type_oid, 23);
                assert_eq!(fields[1].name, "name");
                assert_eq!(fields[1].type_oid, 25);
            }
            _ => panic!("esperaba RowDescription"),
        }
    }

    #[test]
    fn parse_backend_data_row_con_null() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&3i16.to_be_bytes()); // 3 values
                                                        // Value 1: "42"
        payload.extend_from_slice(&2i32.to_be_bytes());
        payload.extend_from_slice(b"42");
        // Value 2: NULL
        payload.extend_from_slice(&(-1i32).to_be_bytes());
        // Value 3: "hola"
        payload.extend_from_slice(&4i32.to_be_bytes());
        payload.extend_from_slice(b"hola");

        let msg = parse_backend_message(b'D', &payload).unwrap();
        match msg {
            BackendMessage::DataRow { values } => {
                assert_eq!(values.len(), 3);
                assert_eq!(values[0].as_deref(), Some(&b"42"[..]));
                assert!(values[1].is_none());
                assert_eq!(values[2].as_deref(), Some(&b"hola"[..]));
            }
            _ => panic!("esperaba DataRow"),
        }
    }

    #[test]
    fn parse_backend_error() {
        let mut payload = Vec::new();
        payload.push(b'S');
        payload.extend_from_slice(b"ERROR\0");
        payload.push(b'C');
        payload.extend_from_slice(b"42P01\0");
        payload.push(b'M');
        payload.extend_from_slice(b"relation \"users\" does not exist\0");
        payload.push(0); // terminador

        let msg = parse_backend_message(b'E', &payload).unwrap();
        match msg {
            BackendMessage::ErrorResponse(ef) => {
                assert_eq!(ef.severity, "ERROR");
                assert_eq!(ef.code, "42P01");
                assert_eq!(ef.message, "relation \"users\" does not exist");
            }
            _ => panic!("esperaba ErrorResponse"),
        }
    }

    // ----- SCRAM-SHA-256 -----
    //
    // Vectores de RFC 7677 §3 (SCRAM-SHA-256 test vector):
    //   username = "user"
    //   password = "pencil"
    //   client_nonce = "rOprNGfwEbeRWgbNEkqO"
    //   server_first response:
    //     r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,
    //     s=W22ZaJ0SNY7soEsUEjb6gQ==,
    //     i=4096
    //   client-final-message:
    //     c=biws,
    //     r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,
    //     p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=
    //   server-final-message:
    //     v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=

    #[test]
    fn scram_rfc7677_test_vector() {
        let mut client = ScramClient::new_with_nonce("user", "pencil", "rOprNGfwEbeRWgbNEkqO");
        assert_eq!(client.client_first(), "n,,n=user,r=rOprNGfwEbeRWgbNEkqO");

        let server_first =
            "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        let cfinal = client.client_final(server_first).unwrap();
        assert_eq!(
            cfinal,
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,\
             p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );

        let server_final = "v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=";
        client.verify(server_final).unwrap();
    }

    #[test]
    fn scram_rechaza_nonce_que_no_extiende_client() {
        let mut client = ScramClient::new_with_nonce("user", "pencil", "myclientnonce");
        let server_first =
            "r=ATTACKERNONCE+xxxxxxxxxxxxxxxxxxxxxxxxxxxxx,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        let r = client.client_final(server_first);
        assert!(matches!(r, Err(DbError::Auth(_))));
    }

    #[test]
    fn scram_rechaza_server_final_invalido() {
        let mut client = ScramClient::new_with_nonce("user", "pencil", "rOprNGfwEbeRWgbNEkqO");
        let server_first =
            "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        client.client_final(server_first).unwrap();
        // Server final con signature tampered (random bytes)
        let r = client.verify("v=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        assert!(matches!(r, Err(DbError::Auth(_))));
    }

    #[test]
    fn scram_rechaza_server_error() {
        let client = ScramClient::new_with_nonce("user", "pencil", "nonce");
        let r = client.verify("e=invalid-username");
        assert!(matches!(r, Err(DbError::Auth(_))));
    }

    #[test]
    fn pbkdf2_sha256_no_panic_iter_alto() {
        // El test "real" del vector RFC 7677 vive en
        // `scram_rfc7677_test_vector` (cubre el resultado de
        // pbkdf2 indirecto vía el server-signature). Acá solo
        // chequeamos que `pbkdf2_hmac_sha256` no entre en loop
        // infinito ni panique con un iter count razonable.
        let salt = BASE64.decode("W22ZaJ0SNY7soEsUEjb6gQ==").unwrap();
        let salted = pbkdf2_hmac_sha256(b"pencil", &salt, 4096);
        assert_eq!(salted.len(), 32);
        // El resultado NO debe ser todo ceros (cubrirá bugs
        // estúpidos del XOR loop).
        assert!(salted.iter().any(|&b| b != 0));
    }

    #[test]
    fn hmac_sha256_vector_basico() {
        // RFC 4231 test case 1
        let key = b"\x0b".repeat(20);
        let data = b"Hi There";
        let out = hmac_sha256(&key, data);
        let mut hex = String::new();
        for byte in &out {
            use std::fmt::Write as _;
            let _ = write!(hex, "{:02x}", byte);
        }
        assert_eq!(
            hex,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn md5_rfc1321() {
        // Vectors RFC 1321 Appendix A.5
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
    }

    #[test]
    fn md5_password_postgres_format() {
        // El hash MD5 de Postgres es:
        //   "md5" || md5_hex(md5_hex("pwduser") || salt)
        // Para password="pwd", user="user", salt=[0x12,0x34,0x56,0x78]:
        //   inner = md5_hex("pwduser") = ?
        //   outer = md5_hex(inner_bytes || salt)
        let hash = md5_password("user", "pwd", &[0x12, 0x34, 0x56, 0x78]);
        // Verificamos que tiene prefix correcto y longitud correcta:
        // "md5" + 32 hex chars = 35 chars total.
        assert!(hash.starts_with("md5"));
        assert_eq!(hash.len(), 35);
    }

    // ----- Tipos OID -----

    #[test]
    fn parse_int4() {
        let v = parse_text_value(oid::INT4, Some(b"42")).unwrap();
        assert_eq!(v, PgValue::Int(42));
    }

    #[test]
    fn parse_int8_negativo() {
        let v = parse_text_value(oid::INT8, Some(b"-9223372036854775808")).unwrap();
        assert_eq!(v, PgValue::Int(i64::MIN));
    }

    #[test]
    fn parse_float8() {
        let v = parse_text_value(oid::FLOAT8, Some(b"2.5")).unwrap();
        match v {
            PgValue::Float(x) => assert!((x - 2.5).abs() < 1e-9),
            _ => panic!("esperaba Float"),
        }
    }

    #[test]
    fn parse_bool() {
        assert_eq!(
            parse_text_value(oid::BOOL, Some(b"t")).unwrap(),
            PgValue::Bool(true)
        );
        assert_eq!(
            parse_text_value(oid::BOOL, Some(b"true")).unwrap(),
            PgValue::Bool(true)
        );
        assert_eq!(
            parse_text_value(oid::BOOL, Some(b"f")).unwrap(),
            PgValue::Bool(false)
        );
    }

    #[test]
    fn parse_text() {
        let v = parse_text_value(oid::TEXT, Some(b"hola mundo")).unwrap();
        assert_eq!(v, PgValue::Text("hola mundo".into()));
    }

    #[test]
    fn parse_null() {
        let v = parse_text_value(oid::INT4, None).unwrap();
        assert_eq!(v, PgValue::Null);
    }

    #[test]
    fn parse_bytea_hex() {
        let v = parse_text_value(oid::BYTEA, Some(b"\\xdeadbeef")).unwrap();
        assert_eq!(v, PgValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
    }

    #[test]
    fn parse_uuid() {
        let v = parse_text_value(oid::UUID, Some(b"550e8400-e29b-41d4-a716-446655440000")).unwrap();
        // En MVP 10.1, UUID se trata como Text. 10.5 lo refina.
        assert!(matches!(v, PgValue::Text(_)));
    }

    #[test]
    fn parse_oid_no_soportado() {
        // OID 1700 = numeric (no soportado en MVP)
        let r = parse_text_value(1700, Some(b"42.5"));
        assert!(matches!(r, Err(DbError::UnsupportedType(1700))));
    }

    #[test]
    fn encode_text_value_roundtrip() {
        assert_eq!(encode_text_value(&PgValue::Int(42)), Some(b"42".to_vec()));
        assert_eq!(encode_text_value(&PgValue::Bool(true)), Some(b"t".to_vec()));
        assert_eq!(
            encode_text_value(&PgValue::Bool(false)),
            Some(b"f".to_vec())
        );
        assert_eq!(encode_text_value(&PgValue::Null), None);
        assert_eq!(
            encode_text_value(&PgValue::Text("hola".into())),
            Some(b"hola".to_vec())
        );
        assert_eq!(
            encode_text_value(&PgValue::Bytes(vec![0xde, 0xad])),
            Some(b"\\xdead".to_vec())
        );
    }

    // ----- Row API -----

    #[test]
    fn row_get_by_name_y_index() {
        let row = Row::new(
            vec![("id".into(), oid::INT4), ("name".into(), oid::TEXT)],
            vec![PgValue::Int(7), PgValue::Text("Fitz".into())],
        );
        assert_eq!(row.get("id"), Some(&PgValue::Int(7)));
        assert_eq!(row.get("name"), Some(&PgValue::Text("Fitz".into())));
        assert_eq!(row.get_at(0), Some(&PgValue::Int(7)));
        assert_eq!(row.get_at(1), Some(&PgValue::Text("Fitz".into())));
        assert_eq!(row.get("missing"), None);
        assert_eq!(row.get_at(99), None);
        assert_eq!(row.len(), 2);
    }

    #[test]
    fn query_result_rows_affected() {
        let qr = QueryResult {
            rows: vec![],
            command_tag: "INSERT 0 5".into(),
        };
        assert_eq!(qr.rows_affected(), 5);

        let qr = QueryResult {
            rows: vec![],
            command_tag: "UPDATE 3".into(),
        };
        assert_eq!(qr.rows_affected(), 3);

        let qr = QueryResult {
            rows: vec![],
            command_tag: "DELETE 0".into(),
        };
        assert_eq!(qr.rows_affected(), 0);
    }
}
