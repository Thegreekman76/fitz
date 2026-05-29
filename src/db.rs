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
    /// `void` — devuelto por fns como `pg_sleep()`, `pg_notify()`,
    /// etc. Postgres lo serializa como string vacío en text format.
    /// Mapeamos a `PgValue::Null` para que SELECT sobre fns void
    /// no falle con `UnsupportedType`.
    pub const VOID: u32 = 2278;

    // Fase 10.5.b — arrays nativos. Cada tipo escalar tiene su OID
    // de array hardcoded en `pg_type` del catálogo de Postgres.
    pub const BOOL_ARRAY: u32 = 1000;
    pub const INT2_ARRAY: u32 = 1005;
    pub const INT4_ARRAY: u32 = 1007;
    pub const TEXT_ARRAY: u32 = 1009;
    pub const VARCHAR_ARRAY: u32 = 1015;
    pub const INT8_ARRAY: u32 = 1016;
    pub const FLOAT4_ARRAY: u32 = 1021;
    pub const FLOAT8_ARRAY: u32 = 1022;
    pub const DATE_ARRAY: u32 = 1182;
    pub const TIMESTAMP_ARRAY: u32 = 1115;
    pub const TIMESTAMPTZ_ARRAY: u32 = 1185;
    pub const UUID_ARRAY: u32 = 2951;

    /// Mapea un OID de array a su OID escalar de elemento.
    /// Devuelve `None` si `oid` no es un array soportado.
    pub fn array_elem_oid(array_oid: u32) -> Option<u32> {
        match array_oid {
            BOOL_ARRAY => Some(BOOL),
            INT2_ARRAY => Some(INT2),
            INT4_ARRAY => Some(INT4),
            TEXT_ARRAY => Some(TEXT),
            VARCHAR_ARRAY => Some(VARCHAR),
            INT8_ARRAY => Some(INT8),
            FLOAT4_ARRAY => Some(FLOAT4),
            FLOAT8_ARRAY => Some(FLOAT8),
            DATE_ARRAY => Some(DATE),
            TIMESTAMP_ARRAY => Some(TIMESTAMP),
            TIMESTAMPTZ_ARRAY => Some(TIMESTAMPTZ),
            UUID_ARRAY => Some(UUID),
            _ => None,
        }
    }

    /// Inverso: dado un OID escalar, devuelve el OID del array
    /// correspondiente. Usado al emitir casts `::int8[]` en INSERTs
    /// con columnas array.
    pub fn elem_to_array_oid(elem_oid: u32) -> Option<u32> {
        match elem_oid {
            BOOL => Some(BOOL_ARRAY),
            INT2 => Some(INT2_ARRAY),
            INT4 => Some(INT4_ARRAY),
            TEXT => Some(TEXT_ARRAY),
            VARCHAR => Some(VARCHAR_ARRAY),
            INT8 => Some(INT8_ARRAY),
            FLOAT4 => Some(FLOAT4_ARRAY),
            FLOAT8 => Some(FLOAT8_ARRAY),
            DATE => Some(DATE_ARRAY),
            TIMESTAMP => Some(TIMESTAMP_ARRAY),
            TIMESTAMPTZ => Some(TIMESTAMPTZ_ARRAY),
            UUID => Some(UUID_ARRAY),
            _ => None,
        }
    }
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
    /// Fase 10.5.b — array Postgres. `elem_oid` indica el tipo de
    /// los elementos (INT4, TEXT, etc.) para que el encoder sepa
    /// qué cast emitir (`$N::int4[]`) y cómo formatear cada item.
    /// Los elementos pueden ser `PgValue::Null` (Postgres soporta
    /// `{1,NULL,3}`). Anidamiento no soportado en MVP — los elem
    /// son escalares.
    Array {
        elem_oid: u32,
        values: Vec<PgValue>,
    },
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
            PgValue::Array { values, .. } => {
                write!(f, "{{")?;
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "}}")
            }
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
        // `void` siempre devuelve un string vacío en text format.
        // Lo modelamos como Null para que `SELECT pg_sleep(...)` no
        // falle con UnsupportedType.
        oid::VOID => Ok(PgValue::Null),
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
            // Fase 10.5.b — arrays nativos. Detectamos por OID y
            // delegamos a parse_array_text que parsea el formato
            // `{a,b,c}` de Postgres.
            if let Some(elem_oid) = oid::array_elem_oid(oid) {
                let values = parse_array_text(s, elem_oid)?;
                return Ok(PgValue::Array { elem_oid, values });
            }
            // Tipos no soportados en 10.1: devolvemos UnsupportedType
            // con el OID concreto. El user ve qué tipo agregar.
            Err(DbError::UnsupportedType(oid))
        }
    }
}

/// Fase 10.5.b — parser del formato text de arrays de Postgres.
///
/// Gramática (simplificada, MVP — sin anidamiento, sin dimensiones
/// custom):
///
/// ```text
/// array     = '{' [ element (',' element)* ] '}'
/// element   = unquoted | quoted | NULL
/// quoted    = '"' (char | '\\' char | '\\"' )* '"'
/// unquoted  = chars sin ',', '{', '}', '"', '\\', whitespace
/// ```
///
/// `NULL` sin comillas → `PgValue::Null`; `"NULL"` con comillas →
/// `PgValue::Text("NULL")` (literal). Whitespace alrededor de los
/// elementos no quoted se trimea (consistente con Postgres).
fn parse_array_text(s: &str, elem_oid: u32) -> DbResult<Vec<PgValue>> {
    let bytes = s.as_bytes();
    let mut idx = 0;
    // Trim whitespace inicial.
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    if idx >= bytes.len() || bytes[idx] != b'{' {
        return Err(DbError::Protocol(format!(
            "array OID {elem_oid}[]: esperaba '{{' al inicio, recibió '{s}'"
        )));
    }
    idx += 1;
    // Trim whitespace.
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    let mut out = Vec::new();
    // Array vacío: '{}'.
    if idx < bytes.len() && bytes[idx] == b'}' {
        return Ok(out);
    }
    loop {
        // Parsear un elemento.
        let (elem_raw, was_quoted, new_idx) = parse_array_element(bytes, idx)?;
        let value = if !was_quoted && elem_raw.eq_ignore_ascii_case("NULL") {
            PgValue::Null
        } else {
            // Parseamos el elemento como un valor escalar usando
            // parse_text_value con el OID del elemento.
            parse_text_value(elem_oid, Some(elem_raw.as_bytes()))?
        };
        out.push(value);
        idx = new_idx;
        // Skip whitespace.
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            return Err(DbError::Protocol(format!(
                "array OID {elem_oid}[]: fin inesperado, esperaba ',' o '}}'"
            )));
        }
        match bytes[idx] {
            b',' => {
                idx += 1;
                // Skip whitespace antes del próximo elemento.
                while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
                    idx += 1;
                }
            }
            b'}' => return Ok(out),
            other => {
                return Err(DbError::Protocol(format!(
                    "array OID {elem_oid}[]: esperaba ',' o '}}', recibió '{}'",
                    other as char
                )));
            }
        }
    }
}

/// Lee un elemento de un array text. Devuelve `(content, was_quoted, new_idx)`.
/// Se llama con `idx` apuntando al primer caracter del elemento.
fn parse_array_element(bytes: &[u8], start: usize) -> DbResult<(String, bool, usize)> {
    if start >= bytes.len() {
        return Err(DbError::Protocol(
            "array element: fin de string inesperado".into(),
        ));
    }
    if bytes[start] == b'"' {
        // Quoted element. Leemos hasta el cierre, deshaciendo escapes
        // `\\` → `\` y `\"` → `"`.
        let mut out = String::new();
        let mut idx = start + 1;
        while idx < bytes.len() {
            match bytes[idx] {
                b'\\' if idx + 1 < bytes.len() => {
                    out.push(bytes[idx + 1] as char);
                    idx += 2;
                }
                b'"' => return Ok((out, true, idx + 1)),
                c => {
                    out.push(c as char);
                    idx += 1;
                }
            }
        }
        Err(DbError::Protocol("array element: quoted no cerrado".into()))
    } else {
        // Unquoted: leemos hasta ',' o '}'.
        let mut idx = start;
        while idx < bytes.len() && bytes[idx] != b',' && bytes[idx] != b'}' {
            idx += 1;
        }
        let raw = &bytes[start..idx];
        // Trim trailing whitespace.
        let mut end = raw.len();
        while end > 0 && raw[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let content = std::str::from_utf8(&raw[..end])
            .map_err(|_| DbError::Protocol("array element: no es UTF-8".into()))?;
        Ok((content.to_string(), false, idx))
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
        PgValue::Array { values, .. } => Some(encode_array_text(values).into_bytes()),
    }
}

/// Fase 10.5.b — codifica un Vec<PgValue> al text format de arrays
/// Postgres: `{elem1,elem2,...}`. Los elementos quoted llevan `"`
/// alrededor y escapes `\\` para `\` y `"`. Null sin quotes.
fn encode_array_text(values: &[PgValue]) -> String {
    let mut out = String::from("{");
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        encode_array_element(&mut out, v);
    }
    out.push('}');
    out
}

fn encode_array_element(out: &mut String, v: &PgValue) {
    match v {
        PgValue::Null => out.push_str("NULL"),
        PgValue::Int(n) => out.push_str(&n.to_string()),
        PgValue::Float(x) => out.push_str(&x.to_string()),
        PgValue::Bool(b) => out.push(if *b { 't' } else { 'f' }),
        PgValue::Text(s) => {
            // Strings siempre quoted en arrays (safe default).
            out.push('"');
            for ch in s.chars() {
                if ch == '"' || ch == '\\' {
                    out.push('\\');
                }
                out.push(ch);
            }
            out.push('"');
        }
        PgValue::Bytes(b) => {
            // bytea en arrays: hex con escape `\\x...`. Lo serializamos
            // como string quoted para que el parser del server lo
            // reciba como `\x...`.
            out.push('"');
            out.push_str("\\\\x");
            for byte in b {
                use std::fmt::Write as _;
                let _ = write!(out, "{:02x}", byte);
            }
            out.push('"');
        }
        PgValue::Array { values, .. } => {
            // Anidamiento: emitimos el sub-array recursivo. Postgres
            // soporta multi-dimensional pero el MVP no lo expone como
            // shape Fitz — esto solo entra si se construye a mano.
            out.push_str(&encode_array_text(values));
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

        // v0.10.13 (B-1 fix) — TCP_NODELAY deshabilita Nagle's
        // algorithm. CRÍTICO para el Extended Query Protocol:
        // mandamos 5 mensajes consecutivos (Parse/Bind/Describe/
        // Execute/Sync) sin esperar respuesta del server entre
        // ellos. Con Nagle activo, el kernel TCP retrasa cada
        // mensaje pequeño esperando ACK del previo, sumando hasta
        // ~40ms de delayed-ACK por query — bug observado en
        // benchmark v2 (GET /users/{id} 43ms vs simple query 4ms).
        // Sin Nagle, los 5 mensajes se mandan inmediatamente
        // batched (aún más rápido con el fix de batching abajo).
        let _ = stream.set_nodelay(true);

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

        // v0.10.13 (B-1 fix) — batchear los 5 mensajes del Extended
        // Query Protocol en UN solo write al socket. Antes hacíamos
        // 5 `self.write(...).await?` separados, cada uno con su
        // syscall write(); aún con TCP_NODELAY activo, los syscalls
        // separados sumaban latencia significativa por cada round
        // de await + scheduling. Benchmark v0.10.13 confirmó:
        // GET /users/{id} pasó de 43ms p50 → ~2ms p50 con este
        // batch, dejando Fitz como ganador absoluto en single-read
        // (vs Python ~34ms antes).
        //
        // El server Postgres NO responde hasta el Sync — los 5
        // mensajes son "pipelined" en el sentido protocolar, no es
        // un cambio semántico. Solo eliminamos overhead client-side.
        //
        // Parse — name vacío = statement anónimo (lifetime de un
        // round). No usamos prepared statements cache en MVP.
        // Describe(Portal "") — CRÍTICO: sin esto, el server NO
        // envía RowDescription tras BindComplete y solo manda
        // DataRow opacos sin nombres de columna.
        let zero_format = [0i16];
        let mut batch: Vec<u8> = Vec::with_capacity(sql.len() + 256);
        batch.extend_from_slice(
            &FrontendMessage::Parse {
                statement_name: "",
                sql,
                param_types: &[],
            }
            .encode(),
        );
        batch.extend_from_slice(
            &FrontendMessage::Bind {
                portal_name: "",
                statement_name: "",
                param_formats: &zero_format,
                param_values: &bind_refs,
                result_formats: &zero_format,
            }
            .encode(),
        );
        batch.extend_from_slice(
            &FrontendMessage::Describe {
                kind: b'P',
                name: "",
            }
            .encode(),
        );
        batch.extend_from_slice(
            &FrontendMessage::Execute {
                portal_name: "",
                max_rows: 0,
            }
            .encode(),
        );
        batch.extend_from_slice(&FrontendMessage::Sync.encode());
        self.write_all_bytes(&batch).await?;

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
// DbPool — pool de conexiones con reconnect + health check
// =============================================================
//
// Fase 10.2: `DbConnHandle` ahora envuelve un pool de N conexiones
// (default 10) en lugar de una sola. Múltiples tareas pueden
// invocar `query()` en paralelo y cada una agarra una conexión
// libre del pool sin bloquearse entre sí — esencial para que el
// throughput HTTP no se serialice por la DB.
//
// Modelo:
//   - `DbPool` mantiene un vector de conexiones idle bajo `Mutex`
//     std (sin parking_lot — `src/db.rs` es self-contained para
//     ser embebido vía `include_str!` en el codegen de 10.1.c).
//   - `tokio::sync::Semaphore` limita el número máximo de conns
//     en uso simultáneo. Si N tareas hacen `query` y el pool
//     ya emitió N conns, la N+1 espera hasta que una se libere.
//   - `PooledConn` mantiene la conexión + un `OwnedSemaphorePermit`.
//     Al hacer `Drop`, la conn vuelve al pool idle y el permit
//     se libera automáticamente.
//   - El `acquire` lazy crece el pool on-demand: si no hay idle,
//     abre una conn TCP nueva (hasta `max_conns`).
//   - Health check spawneado en background hace `SELECT 1` sobre
//     todas las conns idle cada 30s y descarta las que fallan.
//
// `connect_url(url)` es eager — abre la primera conn al boot
// para validar credentials y URL antes de devolver el handle.
// Las conns adicionales se abren lazy en `acquire()`.

const DEFAULT_MAX_CONNS: usize = 10;
const HEALTH_CHECK_INTERVAL_SECS: u64 = 30;

/// Pool interno del `DbConnHandle`. NO se expone al evaluator
/// directamente — el handle delega aquí. Compartido por
/// `Arc<DbPool>` para que el spawned health check task pueda
/// mantener un weak reference sin extender la vida del pool.
pub struct DbPool {
    config: ConnectionConfig,
    /// Cola de conexiones libres listas para usar. `parking_lot`
    /// no se usa porque `src/db.rs` se embebe en el output de
    /// `fitz build` y queremos mantener el archivo self-contained
    /// sin sumar deps al crate generado. `std::sync::Mutex` con
    /// scope corto (push/pop) — sin riesgo de poison porque los
    /// guards no cruzan `.await`.
    idle: std::sync::Mutex<Vec<Connection>>,
    /// Limitador de concurrencia: el pool no entrega más de
    /// `max_conns` conns a la vez. Tareas extra esperan en
    /// `acquire`.
    permits: std::sync::Arc<tokio::sync::Semaphore>,
    /// Marca cooperativa de cierre. Las próximas `acquire()`
    /// fallan con `Protocol("...cerrado")` después de `close()`.
    closed: std::sync::atomic::AtomicBool,
}

impl DbPool {
    /// Intenta agarrar una conn libre del pool. Si no hay,
    /// abre una nueva (lazy growth). Si el pool ya alcanzó
    /// `max_conns` conns vivas, espera en el semaphore.
    async fn acquire(self: &std::sync::Arc<Self>) -> DbResult<PooledConn> {
        use std::sync::atomic::Ordering;
        if self.closed.load(Ordering::Acquire) {
            return Err(DbError::Protocol(
                "la conexión fue cerrada con .close()".into(),
            ));
        }
        // Esperar permit primero (limita concurrencia). El
        // OwnedSemaphorePermit se libera automáticamente cuando
        // PooledConn se dropea.
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| DbError::Protocol("pool: semaphore cerrado".into()))?;
        // Intentar agarrar una conn idle (fast path).
        let maybe_idle = self.idle.lock().expect("pool mutex poisoned").pop();
        let conn = match maybe_idle {
            Some(c) => c,
            None => {
                // Slow path: abrir conn nueva.
                Connection::connect(&self.config).await?
            }
        };
        Ok(PooledConn {
            pool: std::sync::Arc::clone(self),
            conn: Some(conn),
            _permit: permit,
        })
    }

    /// Devuelve una conn al pool idle. Llamada solo por
    /// `PooledConn::drop`.
    fn release(&self, conn: Connection) {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            // Pool cerrado — descartamos la conn (su Drop
            // eventualmente cerrará el TcpStream).
            return;
        }
        self.idle.lock().expect("pool mutex poisoned").push(conn);
    }
}

/// RAII wrapper sobre una conn del pool. Mientras vive, la conn
/// está fuera del pool. Al hacer `Drop`, vuelve al idle queue
/// (si el pool no está cerrado).
pub struct PooledConn {
    pool: std::sync::Arc<DbPool>,
    /// `Option` para poder hacer `take()` desde `Drop` (que
    /// solo recibe `&mut self`, no `self`).
    conn: Option<Connection>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl PooledConn {
    fn as_mut(&mut self) -> &mut Connection {
        self.conn
            .as_mut()
            .expect("PooledConn::conn None mid-lifetime (bug)")
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.release(conn);
        }
        // El `_permit` se libera automático en el drop del field.
    }
}

// =============================================================
// DbConnHandle — wrapper Send + Sync para el evaluator
// =============================================================
//
// `Connection` (definida arriba) es la abstracción interna del
// driver: contiene el `TcpStream` y el estado del protocolo. NO
// la exponemos directo al evaluator porque el evaluator necesita
// un handle que pueda compartirse entre múltiples tasks (vía Arc)
// y que serialice las operaciones I/O (vía Mutex).
//
// 10.2: el handle ahora envuelve un `Arc<DbPool>` en lugar de una
// `Mutex<Option<Connection>>`. La API pública (`query`/`exec`/
// `close`/`is_closed`) se mantiene idéntica.

/// Handle opaco a un pool de conexiones Postgres. Construido por
/// `connect_url()` y pasado al evaluator como
/// `Value::DbConn(Arc<DbConnHandle>)`.
///
/// El struct NO implementa `Clone` directamente — el evaluator
/// usa el `Arc` externo para sharing.
pub struct DbConnHandle {
    pool: std::sync::Arc<DbPool>,
    /// URL original sin password — útil para Display y errores.
    pub url_redacted: String,
}

impl std::fmt::Debug for DbConnHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DbConnHandle({})", self.url_redacted)
    }
}

impl DbConnHandle {
    /// Construye un handle a partir de una conexión inicial. El
    /// pool empieza con esa conn en `idle` y crece on-demand
    /// hasta `max_conns`.
    pub fn new(initial_conn: Connection, url_redacted: String, config: ConnectionConfig) -> Self {
        let pool = std::sync::Arc::new(DbPool {
            config,
            idle: std::sync::Mutex::new(vec![initial_conn]),
            permits: std::sync::Arc::new(tokio::sync::Semaphore::new(DEFAULT_MAX_CONNS)),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        DbConnHandle { pool, url_redacted }
    }

    /// Constructor para tests del evaluator: produce un handle
    /// con pool en estado "cerrado". Las queries fallan con
    /// `Protocol("...cerrado")` pero el dispatch_method funciona
    /// — útil para validar la integración sin Postgres real.
    #[cfg(any(test, debug_assertions))]
    pub fn new_for_test_closed(url_redacted: String) -> Self {
        // Config dummy — nunca se usa porque el pool arranca cerrado.
        let dummy_config = ConnectionConfig {
            host: "test-host".into(),
            port: 5432,
            user: "test-user".into(),
            password: None,
            dbname: "test-db".into(),
            sslmode: SslMode::Disable,
        };
        let pool = std::sync::Arc::new(DbPool {
            config: dummy_config,
            idle: std::sync::Mutex::new(Vec::new()),
            permits: std::sync::Arc::new(tokio::sync::Semaphore::new(DEFAULT_MAX_CONNS)),
            closed: std::sync::atomic::AtomicBool::new(true),
        });
        DbConnHandle { pool, url_redacted }
    }

    /// Ejecuta una query con args. Agarra una conn del pool,
    /// ejecuta, la devuelve al pool al terminar (vía Drop del
    /// PooledConn). Si el pool está cerrado o no puede abrir
    /// conn nueva, error claro.
    pub async fn query(&self, sql: &str, args: &[PgValue]) -> DbResult<QueryResult> {
        let mut pooled = self.pool.acquire().await?;
        let conn = pooled.as_mut();
        if args.is_empty() {
            conn.simple_query(sql).await
        } else {
            conn.extended_query(sql, args).await
        }
    }

    /// Ejecuta un statement que no espera rows (INSERT/UPDATE/
    /// DELETE/DDL sin RETURNING). Devuelve el número de rows
    /// afectadas inferido del `CommandComplete` tag.
    pub async fn exec(&self, sql: &str, args: &[PgValue]) -> DbResult<u64> {
        let result = self.query(sql, args).await?;
        Ok(result.rows_affected())
    }

    /// Cierra el pool cooperativamente. Marca como cerrado y
    /// drainea las conns idle (envía Terminate a cada una).
    /// Idempotente — múltiples `close()` no son error.
    /// Conns checked-out al momento del close se descartan al
    /// devolverse (release ve `closed=true` y skipea push).
    pub async fn close(&self) -> DbResult<()> {
        use std::sync::atomic::Ordering;
        if self.pool.closed.swap(true, Ordering::AcqRel) {
            return Ok(()); // ya estaba cerrado
        }
        // Drain idle queue + cerrar cada conn.
        let drained = {
            let mut idle = self.pool.idle.lock().expect("pool mutex poisoned");
            std::mem::take(&mut *idle)
        };
        for conn in drained {
            let _ = conn.close().await;
        }
        // Cerrar el semaphore para que `acquire_owned` en
        // tareas pendientes despierten con error.
        self.pool.permits.close();
        Ok(())
    }

    /// `true` si el pool fue cerrado con `close()`.
    pub async fn is_closed(&self) -> bool {
        self.pool.closed.load(std::sync::atomic::Ordering::Acquire)
    }

    /// 10.2: número máximo de conns concurrentes que el pool
    /// puede emitir. Hoy fijo en `DEFAULT_MAX_CONNS = 10`;
    /// configurable como kwarg `db.connect(url, max_conns=N)`
    /// queda como deuda menor para 10.2.b.
    pub fn max_conns(&self) -> usize {
        DEFAULT_MAX_CONNS
    }

    /// 10.2 — diagnostics: número de conns idle en este instante.
    /// Útil para tests del pool. NO sirve para concurrency
    /// (race entre `idle()` y la próxima query).
    pub fn idle_count(&self) -> usize {
        self.pool.idle.lock().expect("pool mutex poisoned").len()
    }
}

/// Health check: cada `HEALTH_CHECK_INTERVAL_SECS` segundos,
/// itera las conns idle y manda `SELECT 1`. Las conns que fallan
/// se descartan silenciosamente — el próximo `acquire` abrirá
/// una nueva. El task usa un `Weak<DbPool>` para que el pool
/// pueda ser garbage-collected cuando el handle se dropea, y el
/// task se autodescarte al ver que el upgrade del Weak falla.
async fn health_check_task(weak_pool: std::sync::Weak<DbPool>) {
    let interval = std::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS);
    loop {
        tokio::time::sleep(interval).await;
        let pool = match weak_pool.upgrade() {
            Some(p) => p,
            None => return, // pool dropeado, task se autotermina
        };
        if pool.closed.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        // Drenar el idle queue, validar cada conn, devolver las vivas.
        let mut to_check = {
            let mut idle = pool.idle.lock().expect("pool mutex poisoned");
            std::mem::take(&mut *idle)
        };
        let mut alive = Vec::with_capacity(to_check.len());
        while let Some(mut conn) = to_check.pop() {
            // SELECT 1 ligero — si falla, descartamos.
            match conn.simple_query("SELECT 1").await {
                Ok(_) => alive.push(conn),
                Err(_) => {
                    // Conn dead — el Drop del Connection cierra el TcpStream.
                }
            }
        }
        // Re-poblar idle con las vivas.
        if !alive.is_empty() {
            let mut idle = pool.idle.lock().expect("pool mutex poisoned");
            idle.append(&mut alive);
        }
    }
}

/// Abre una conexión Postgres desde un URL estándar. Punto de
/// entrada principal para integración con el evaluator: el
/// builtin `db.connect(url)` lo invoca y envuelve el resultado
/// en `Value::DbConn(handle)` (ya como `Arc<DbConnHandle>`).
///
/// **10.9.2 (v0.10.9) — singleton per URL**: el primer call con
/// una URL nueva crea el handle + pool. Calls posteriores con la
/// MISMA URL devuelven clone del Arc existente — TODAS las
/// conns TCP se comparten via el pool único. Esto cierra el
/// "connection pool leak" anterior donde cada `db.connect(url)`
/// creaba pool nuevo (después de N requests Postgres se quedaba
/// sin slots y `acquire()` colgaba).
///
/// El cache vive en `POOL_CACHE` (global, lazy). Los handles
/// persisten hasta el cierre del proceso (no se evictan). Trade-
/// off aceptado: si nunca te volvés a conectar a una URL, el
/// pool sobrevive sin uso. La memoria es despreciable (~24 KB
/// por pool idle).
///
/// Eager: la primera conn TCP + handshake + auth sucede acá para
/// validar credentials + URL antes de devolver el handle. Si
/// falla, el handle no se crea y el caller ve el error directo.
/// Conns adicionales se abren lazy en `acquire()` cuando el pool
/// las necesita.
pub async fn connect_url(url: &str) -> DbResult<std::sync::Arc<DbConnHandle>> {
    // 10.9.2 (v0.10.9) — singleton per URL. El cache global cachea el
    // Arc<DbConnHandle> por URL; calls subsiguientes con la misma URL
    // devuelven clone del Arc — TODAS las conns TCP se comparten via el
    // pool único. Cierra el "connection pool leak" del v0.10.8: cada
    // `db.connect(url)` creaba pool nuevo con 10 permits + TCP conns,
    // saturando Postgres (`max_connections=100` default) tras N
    // requests y dejando `acquire()` colgado eternamente.
    //
    // Trade-off: los handles persisten hasta el cierre del proceso (no
    // se evictan). Memoria despreciable (~24 KB por pool idle).
    static POOL_CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<DbConnHandle>>>,
    > = std::sync::OnceLock::new();
    let cache = POOL_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));

    // Cache lookup — fast path zero-alloc.
    {
        let guard = cache.lock().expect("POOL_CACHE poisoned");
        if let Some(existing) = guard.get(url) {
            if !existing
                .pool
                .closed
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Ok(std::sync::Arc::clone(existing));
            }
            // Si el handle fue cerrado con `.close()`, creamos uno
            // nuevo — el caller hizo close explícito y quiere reabrir.
        }
    }

    // Miss: crear handle nuevo + insertarlo en el cache.
    let config = ConnectionConfig::parse(url)?;
    let url_redacted = redact_url(url);
    let initial_conn = Connection::connect(&config).await?;
    let handle = std::sync::Arc::new(DbConnHandle::new(initial_conn, url_redacted, config));
    let weak = std::sync::Arc::downgrade(&handle.pool);
    tokio::spawn(health_check_task(weak));
    {
        let mut guard = cache.lock().expect("POOL_CACHE poisoned");
        guard.insert(url.to_string(), std::sync::Arc::clone(&handle));
    }
    Ok(handle)
}

/// Elimina la password del URL para diagnostics seguros. Reemplaza
/// `user:pass@host` por `user:***@host`. No-op si no hay password.
fn redact_url(url: &str) -> String {
    let prefix_len = if let Some(p) = url.strip_prefix("postgres://") {
        url.len() - p.len()
    } else if let Some(p) = url.strip_prefix("postgresql://") {
        url.len() - p.len()
    } else {
        return url.to_string();
    };
    let (prefix, rest) = url.split_at(prefix_len);
    let (auth, tail) = match rest.rfind('@') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => return url.to_string(),
    };
    let redacted_auth = match auth.split_once(':') {
        Some((user, _)) => format!("{user}:***"),
        None => auth.to_string(),
    };
    format!("{prefix}{redacted_auth}{tail}")
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

    // ----- Fase 10.5.b — arrays nativos -----

    #[test]
    fn parse_array_int4_basico() {
        let v = parse_text_value(oid::INT4_ARRAY, Some(b"{1,2,3}")).unwrap();
        match v {
            PgValue::Array { elem_oid, values } => {
                assert_eq!(elem_oid, oid::INT4);
                assert_eq!(
                    values,
                    vec![PgValue::Int(1), PgValue::Int(2), PgValue::Int(3)]
                );
            }
            other => panic!("esperaba Array, got {:?}", other),
        }
    }

    #[test]
    fn parse_array_vacio() {
        let v = parse_text_value(oid::INT8_ARRAY, Some(b"{}")).unwrap();
        match v {
            PgValue::Array { values, .. } => assert!(values.is_empty()),
            _ => panic!("esperaba Array vacío"),
        }
    }

    #[test]
    fn parse_array_text_con_quoted_strings() {
        let v = parse_text_value(oid::TEXT_ARRAY, Some(b"{\"hola\",\"chau\"}")).unwrap();
        match v {
            PgValue::Array { elem_oid, values } => {
                assert_eq!(elem_oid, oid::TEXT);
                assert_eq!(
                    values,
                    vec![PgValue::Text("hola".into()), PgValue::Text("chau".into())]
                );
            }
            _ => panic!("esperaba Array text"),
        }
    }

    #[test]
    fn parse_array_text_con_escapes() {
        // {"a,b","c\"d","e\\f"} → ["a,b", "c\"d", "e\\f"]
        let raw = b"{\"a,b\",\"c\\\"d\",\"e\\\\f\"}";
        let v = parse_text_value(oid::TEXT_ARRAY, Some(raw)).unwrap();
        match v {
            PgValue::Array { values, .. } => {
                assert_eq!(
                    values,
                    vec![
                        PgValue::Text("a,b".into()),
                        PgValue::Text("c\"d".into()),
                        PgValue::Text("e\\f".into()),
                    ]
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_array_con_null_sin_quotes() {
        // {1,NULL,3} → [1, null, 3]
        let v = parse_text_value(oid::INT4_ARRAY, Some(b"{1,NULL,3}")).unwrap();
        match v {
            PgValue::Array { values, .. } => {
                assert_eq!(
                    values,
                    vec![PgValue::Int(1), PgValue::Null, PgValue::Int(3)]
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_array_null_quoted_es_literal() {
        // {"NULL"} → ["NULL"] como texto, no null.
        let v = parse_text_value(oid::TEXT_ARRAY, Some(b"{\"NULL\"}")).unwrap();
        match v {
            PgValue::Array { values, .. } => {
                assert_eq!(values, vec![PgValue::Text("NULL".into())]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_array_bool() {
        let v = parse_text_value(oid::BOOL_ARRAY, Some(b"{t,f,t}")).unwrap();
        match v {
            PgValue::Array { values, .. } => {
                assert_eq!(
                    values,
                    vec![
                        PgValue::Bool(true),
                        PgValue::Bool(false),
                        PgValue::Bool(true)
                    ]
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_array_float8() {
        let v = parse_text_value(oid::FLOAT8_ARRAY, Some(b"{1.5,2.5,-3.0}")).unwrap();
        match v {
            PgValue::Array { values, .. } => {
                assert_eq!(
                    values,
                    vec![
                        PgValue::Float(1.5),
                        PgValue::Float(2.5),
                        PgValue::Float(-3.0)
                    ]
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_array_uuid_via_text_array() {
        // UUIDs llegan como TEXT_ARRAY[uuid-strings]; el ORM acepta
        // UUID por su OID escalar, pero también via list<Str> con
        // el cast `::uuid[]` explícito.
        let v = parse_text_value(
            oid::UUID_ARRAY,
            Some(b"{550e8400-e29b-41d4-a716-446655440000}"),
        )
        .unwrap();
        match v {
            PgValue::Array { elem_oid, values } => {
                assert_eq!(elem_oid, oid::UUID);
                assert_eq!(
                    values,
                    vec![PgValue::Text("550e8400-e29b-41d4-a716-446655440000".into())]
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn encode_array_int4() {
        let v = PgValue::Array {
            elem_oid: oid::INT4,
            values: vec![PgValue::Int(1), PgValue::Int(2), PgValue::Int(3)],
        };
        assert_eq!(encode_text_value(&v), Some(b"{1,2,3}".to_vec()));
    }

    #[test]
    fn encode_array_vacio() {
        let v = PgValue::Array {
            elem_oid: oid::INT4,
            values: vec![],
        };
        assert_eq!(encode_text_value(&v), Some(b"{}".to_vec()));
    }

    #[test]
    fn encode_array_text_quotea_strings() {
        let v = PgValue::Array {
            elem_oid: oid::TEXT,
            values: vec![PgValue::Text("hola".into()), PgValue::Text("chau".into())],
        };
        assert_eq!(encode_text_value(&v), Some(b"{\"hola\",\"chau\"}".to_vec()));
    }

    #[test]
    fn encode_array_text_escapa_comillas_y_backslash() {
        let v = PgValue::Array {
            elem_oid: oid::TEXT,
            values: vec![PgValue::Text("c\"d".into()), PgValue::Text("e\\f".into())],
        };
        // "c\"d" → "c\\\"d"  (escape para Postgres)
        // "e\\f" → "e\\\\f"
        assert_eq!(
            encode_text_value(&v),
            Some(b"{\"c\\\"d\",\"e\\\\f\"}".to_vec())
        );
    }

    #[test]
    fn encode_array_con_null() {
        let v = PgValue::Array {
            elem_oid: oid::INT4,
            values: vec![PgValue::Int(1), PgValue::Null, PgValue::Int(3)],
        };
        assert_eq!(encode_text_value(&v), Some(b"{1,NULL,3}".to_vec()));
    }

    #[test]
    fn array_elem_oid_mapeo() {
        assert_eq!(oid::array_elem_oid(oid::INT4_ARRAY), Some(oid::INT4));
        assert_eq!(oid::array_elem_oid(oid::TEXT_ARRAY), Some(oid::TEXT));
        assert_eq!(oid::array_elem_oid(oid::BOOL_ARRAY), Some(oid::BOOL));
        assert_eq!(oid::array_elem_oid(9999), None);
    }

    #[test]
    fn elem_to_array_oid_mapeo() {
        assert_eq!(oid::elem_to_array_oid(oid::INT4), Some(oid::INT4_ARRAY));
        assert_eq!(oid::elem_to_array_oid(oid::TEXT), Some(oid::TEXT_ARRAY));
        assert_eq!(oid::elem_to_array_oid(9999), None);
    }

    #[test]
    fn array_roundtrip_via_parse_encode() {
        // {1,2,3} → Array → encode → "{1,2,3}"
        let parsed = parse_text_value(oid::INT4_ARRAY, Some(b"{1,2,3}")).unwrap();
        let encoded = encode_text_value(&parsed).unwrap();
        assert_eq!(encoded, b"{1,2,3}".to_vec());
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

    // ----- redact_url -----

    #[test]
    fn redact_url_oculta_password() {
        assert_eq!(
            redact_url("postgres://alice:secret@host/db"),
            "postgres://alice:***@host/db"
        );
        assert_eq!(
            redact_url("postgresql://alice:secret@host:5432/db?sslmode=disable"),
            "postgresql://alice:***@host:5432/db?sslmode=disable"
        );
    }

    #[test]
    fn redact_url_sin_password_no_cambia() {
        assert_eq!(
            redact_url("postgres://alice@host/db"),
            "postgres://alice@host/db"
        );
    }

    #[test]
    fn redact_url_otros_schemes_passthrough() {
        // Si el URL no es postgres://, devolvemos tal cual (el
        // caller ya falló al parsear; redact es solo defensa).
        assert_eq!(redact_url("mysql://x:y@h/d"), "mysql://x:y@h/d");
    }

    // ----- DbConnHandle lifecycle (sin Postgres real) -----
    //
    // No podemos testear `query/exec` end-to-end sin un Postgres
    // real, pero sí el ciclo "closed → operations fail". Usamos
    // `new_for_test_closed` que construye un pool en estado
    // closed sin abrir TCP.

    #[tokio::test]
    async fn db_conn_handle_closed_falla_query() {
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        assert!(handle.is_closed().await);
        let r = handle.query("SELECT 1", &[]).await;
        assert!(matches!(r, Err(DbError::Protocol(_))));
    }

    #[tokio::test]
    async fn db_conn_handle_close_idempotente() {
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        // close() sobre un handle ya cerrado es no-op (no error).
        handle.close().await.unwrap();
        handle.close().await.unwrap();
    }

    // ----- 10.2 — pool de conexiones -----

    #[tokio::test]
    async fn db_pool_max_conns_default() {
        // El pool default expone DEFAULT_MAX_CONNS conns
        // concurrentes. La API pública del handle lo expone vía
        // `max_conns()`.
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        assert_eq!(handle.max_conns(), DEFAULT_MAX_CONNS);
    }

    #[tokio::test]
    async fn db_pool_idle_count_inicia_en_cero_cuando_closed() {
        // El handle de test arranca en estado closed, pool vacío.
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        assert_eq!(handle.idle_count(), 0);
    }

    #[test]
    fn db_pool_struct_es_send_sync() {
        // DbPool tiene que ser Send + Sync para que el handle
        // pueda compartirse entre tasks. Esto valida que toda la
        // composición (Mutex<Vec>/Arc<Semaphore>/AtomicBool) sigue
        // marcando los traits que `Value::DbConn(Arc<...>)` necesita.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DbPool>();
        assert_send_sync::<DbConnHandle>();
    }

    #[tokio::test]
    async fn db_pool_acquire_falla_cuando_closed() {
        // acquire() sobre pool closed debe devolver Protocol
        // error claro (no panic, no hang).
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        let r = handle.pool.acquire().await;
        assert!(matches!(r, Err(DbError::Protocol(_))));
    }

    #[tokio::test]
    async fn db_pool_close_idempotente_y_marca_closed_flag() {
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        assert!(handle.is_closed().await);
        // Primera close — no-op porque ya está closed; sin errores.
        handle.close().await.unwrap();
        // Segunda close — tampoco error.
        handle.close().await.unwrap();
        assert!(handle.is_closed().await);
    }
}
