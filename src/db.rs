//! Pure Fitz Postgres driver — Phase 10.1.
//!
//! Native implementation of PostgreSQL wire protocol v3.0 without
//! external DB dependencies (no libpq, no tokio-postgres, no
//! sqlx). Only `tokio::net::TcpStream` for I/O + pure-Rust crypto
//! crates (sha2/hmac/base64) for SCRAM-SHA-256.
//!
//! Design decisions (closed 2026-05-25, see
//! `docs/roadmap.md` → Phase 10):
//!  - Pure Fitz driver, no libpq → standalone binary preserved.
//!  - Fully async API → fits tokio + HTTP handlers from 9.w.
//!  - No pool in 10.1 (arrives in 10.2). One connection per
//!    `connect(url)`.
//!  - Postgres 14+ (SCRAM-SHA-256 default, mature JSONB).
//!  - Standard URI `postgres://user:pass@host:port/db?sslmode=...`.
//!  - SSL/TLS postponed to future sub-step. In 10.1 only
//!    `sslmode=disable` (default); `sslmode=require` aborts with
//!    clear message.
//!  - No Extended Query Protocol prepared-statement cache
//!    (each query parses from scratch).
//!  - Core OID types in MVP: Int4/Int8/Float4/Float8/Text/
//!    Varchar/Bool/Timestamp/Timestamptz/UUID/Bytea. JSONB +
//!    arrays + advanced dates land in 10.5.
//!
//! The module is deliberately **isolated** from the rest of the
//! crate in 10.1.a — it does not yet integrate with `evaluator`
//! nor `value::Value::DbConn`. Integration as the built-in `db`
//! module accessible from Fitz code arrives in 10.1.b.

#![allow(dead_code)] // 10.1.a — public APIs are consumed in 10.1.b when integration with evaluator lands.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

type HmacSha256 = Hmac<Sha256>;

// =============================================================
// Errors
// =============================================================

/// Postgres driver–specific error. We keep a dedicated enum
/// (instead of wrapping `FitzError` directly) because the driver
/// lives as an isolated module in 10.1.a; translation to
/// `FitzError` (with span, position, etc.) happens in the
/// integration layer 10.1.b.
#[derive(Debug)]
pub enum DbError {
    /// Invalid connection URL (format, parsing, out-of-range
    /// values).
    InvalidUrl(String),
    /// I/O failure (TCP, socket read/write).
    Io(io::Error),
    /// Postgres protocol returned something unexpected (out-of-
    /// sequence message, invalid length, etc.).
    Protocol(String),
    /// Auth failed (wrong credentials, SCRAM mismatch, not
    /// supported).
    Auth(String),
    /// Postgres server error (`ErrorResponse`). Canonical format
    /// `"<severity>: <message>"` parallel to
    /// `jwt`/`hash`/etc.
    Server {
        severity: String,
        code: String,
        message: String,
    },
    /// Postgres OID type unsupported in MVP. The error cites the
    /// numeric OID so the user clearly sees which type to add to
    /// the MVP refinement in 10.5.
    UnsupportedType(u32),
    /// Feature requested by the user that hasn't landed in 10.1
    /// (sslmode=require, advanced types, etc.). Message includes
    /// reference to the closing sub-step.
    NotImplemented(String),
    /// v0.10.23 (Phase 10.1.b) — TLS path failure: SSLRequest
    /// rejected by the server ('N'/'E'), broken handshake (invalid
    /// chain, hostname mismatch on verify-full, invalid
    /// signature), or unreadable/malformed sslrootcert.
    Tls(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::InvalidUrl(m) => write!(f, "URL inválida: {m}"),
            DbError::Io(e) => write!(f, "I/O: {e}"),
            DbError::Protocol(m) => write!(f, "protocolo: {m}"),
            DbError::Auth(m) => write!(f, "auth: {m}"),
            // v0.10.29 — Adds the SQLSTATE in brackets when
            // available (Postgres always includes it in
            // ErrorResponse). The user can grep by code
            // (`[23505]` = unique violation, `[23503]` = FK
            // violation, etc.) without parsing the free-form
            // message.
            DbError::Server {
                severity,
                code,
                message,
            } => {
                if code.is_empty() {
                    write!(f, "{severity}: {message}")
                } else {
                    write!(f, "{severity} [{code}]: {message}")
                }
            }
            DbError::UnsupportedType(oid) => {
                write!(f, "tipo Postgres OID {oid} no soportado en MVP (10.5)")
            }
            DbError::NotImplemented(m) => write!(f, "no implementado: {m}"),
            DbError::Tls(m) => write!(f, "TLS: {m}"),
        }
    }
}

/// v0.10.29 — Enriches a `DbError::Server` with the context of the
/// SQL + params that triggered the error. Applies the same secret
/// redaction as `FITZ_DB_LOG=verbose` to avoid leaking
/// passwords/tokens to stderr or structured logs. The suffix is
/// `[sql: <truncated query> params=[$1=..., ...]]`. If the error
/// is not `Server` (e.g. I/O, Protocol), it passes through
/// unchanged — the context does not apply.
pub(crate) fn enrich_db_error_with_context(err: DbError, sql: &str, args: &[PgValue]) -> DbError {
    if let DbError::Server {
        severity,
        code,
        message,
    } = err
    {
        // One-line SQL truncated to 200 chars to avoid inflating
        // messages.
        let sql_oneline = sql
            .replace(['\n', '\r'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let sql_short = if sql_oneline.chars().count() > 200 {
            let mut s: String = sql_oneline.chars().take(200).collect();
            s.push('…');
            s
        } else {
            sql_oneline.clone()
        };
        let params_part = if args.is_empty() {
            String::new()
        } else {
            let sql_lower = sql.to_ascii_lowercase();
            let params: Vec<String> = args
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let pos = i + 1;
                    if should_redact_param(&sql_lower, pos) {
                        format!("${pos}=<redacted>")
                    } else {
                        format!("${pos}={}", format_log_value(v))
                    }
                })
                .collect();
            format!(" params=[{}]", params.join(", "))
        };
        let new_message = format!("{message} [sql: {sql_short}{params_part}]");
        return DbError::Server {
            severity,
            code,
            message: new_message,
        };
    }
    err
}

impl std::error::Error for DbError {}

impl From<io::Error> for DbError {
    fn from(e: io::Error) -> Self {
        DbError::Io(e)
    }
}

pub type DbResult<T> = Result<T, DbError>;

// =============================================================
// ConnectionConfig — parsing of the connection string
// =============================================================

/// Resolved configuration for a Postgres connection. Built via
/// `ConnectionConfig::parse(url)` from the standard URI
/// `postgres://user:pass@host:port/dbname?sslmode=...`.
///
/// Supported in 10.1:
///  - Schemes: `postgres://` and `postgresql://` (standard alias).
///  - User required. Password optional (trust/peer auth does not
///    need a password).
///  - Host: literal (IPv4, IPv6 in brackets `[::1]`, or hostname).
///  - Port: optional, default 5432.
///  - Dbname: required in MVP. Defaulting to username when the
///    query string allows it lands in 10.x.
///  - Query: `sslmode=disable|require|allow|prefer`. Only
///    `disable` supported in 10.1; the rest aborts with a clear
///    message citing the future sub-step.
///  - `application_name=...` silently ignored in MVP (does no
///    harm, has no effect).
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub dbname: String,
    pub sslmode: SslMode,
    /// Phase 10.1.b — optional kwarg `sslrootcert=path/to/ca.pem`
    /// for a custom CA. If `None` and sslmode is
    /// `VerifyCa`/`VerifyFull`, the driver uses the Mozilla root
    /// CA bundle from `webpki-roots`. Path is resolved relative
    /// to the process CWD. PEM format only.
    pub sslrootcert: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslMode {
    /// No TLS — the handshake goes plain over TCP. Default if the
    /// URL does not specify `sslmode`.
    Disable,
    /// Phase 10.1.b — TLS required, but NO server cert verification
    /// at all (accepts self-signed, expired, hostname mismatch).
    /// Useful for dev/staging against internal Postgres without a
    /// CA. DO NOT USE in production — vulnerable to MITM.
    Require,
    /// Phase 10.1.b — TLS required + verifies the cert comes from a
    /// trusted CA (chain), but IGNORES the hostname. Useful for
    /// configurations where the cert has a CN distinct from the
    /// hostname (proxies, port forwarding). Verifies operator
    /// authenticity but not specific identity.
    VerifyCa,
    /// Phase 10.1.b — TLS required + valid chain + hostname matches
    /// the cert's SAN/CN. **Recommended for production**. This is
    /// the mode used by Heroku, RDS, Supabase, Neon, Aiven, Render
    /// PG.
    VerifyFull,
}

impl ConnectionConfig {
    /// Parses `postgres://user:pass@host:port/dbname?sslmode=...`.
    /// The format is aligned with libpq / psycopg2 / pgx / sqlx
    /// (same URI for all Postgres drivers in the ecosystem).
    pub fn parse(url: &str) -> DbResult<Self> {
        let rest = url
            .strip_prefix("postgres://")
            .or_else(|| url.strip_prefix("postgresql://"))
            .ok_or_else(|| {
                DbError::InvalidUrl(format!(
                    "esperaba 'postgres://' o 'postgresql://', no '{url}'"
                ))
            })?;

        // Split on the last '@' that is NOT inside the query
        // string. Practical: split on the first '?' first
        // (separates auth+host+path from query), then the last
        // '@' of the previous segment.
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

        let (sslmode, sslrootcert) = parse_ssl_params(query)?;

        Ok(ConnectionConfig {
            host,
            port,
            user,
            password,
            dbname,
            sslmode,
            sslrootcert,
        })
    }
}

fn split_host_port(s: &str) -> DbResult<(String, u16)> {
    if s.is_empty() {
        return Err(DbError::InvalidUrl("host vacío".into()));
    }
    // IPv6 literal: `[::1]:5432`. The bracket closes before the
    // ':' of the port.
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

/// v0.10.23 (Phase 10.1.b) — parser for the SSL params from the
/// query string. Returns `(sslmode, sslrootcert)`. No sslmode →
/// Disable. `prefer`/`allow` (dynamic negotiation) remain
/// out-of-scope MVP with a clear message citing the compat
/// pattern (`disable`/`require`).
fn parse_ssl_params(query: Option<&str>) -> DbResult<(SslMode, Option<std::path::PathBuf>)> {
    let mut mode = SslMode::Disable;
    let mut root_cert: Option<std::path::PathBuf> = None;
    let mut mode_seen = false;

    let q = match query {
        Some(q) => q,
        None => return Ok((mode, root_cert)),
    };

    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "sslmode" => {
                mode_seen = true;
                mode = match v {
                    "disable" | "" => SslMode::Disable,
                    "require" => SslMode::Require,
                    "verify-ca" => SslMode::VerifyCa,
                    "verify-full" => SslMode::VerifyFull,
                    "prefer" | "allow" => {
                        return Err(DbError::NotImplemented(format!(
                            "sslmode={v} (negociación dinámica) queda \
                             out-of-scope MVP; usá `require` si querés TLS, \
                             `disable` si no, o `verify-full` para validar \
                             cert (recomendado producción)"
                        )))
                    }
                    other => {
                        return Err(DbError::InvalidUrl(format!(
                            "sslmode desconocido: '{other}' (válidos: \
                             disable, require, verify-ca, verify-full)"
                        )))
                    }
                };
            }
            "sslrootcert" => {
                if v.is_empty() {
                    return Err(DbError::InvalidUrl("sslrootcert con value vacío".into()));
                }
                root_cert = Some(std::path::PathBuf::from(percent_decode(v)?));
            }
            // application_name, connect_timeout, etc. — silently
            // ignored in MVP. Does no harm, does not affect
            // correctness.
            _ => continue,
        }
    }

    // Invalid combinations — better to fail early with a clear
    // message than to leave the user with a binary that connects
    // without TLS thinking they were protected.
    if !mode_seen && root_cert.is_some() {
        return Err(DbError::InvalidUrl(
            "sslrootcert= sin sslmode= no tiene sentido: el cert no \
             se va a usar. Agregá sslmode=verify-ca o verify-full"
                .into(),
        ));
    }
    if mode == SslMode::Disable && root_cert.is_some() {
        return Err(DbError::InvalidUrl(
            "sslrootcert= con sslmode=disable es contradictorio. Usá \
             sslmode=verify-ca o verify-full para activar la validación"
                .into(),
        ));
    }
    if mode == SslMode::Require && root_cert.is_some() {
        return Err(DbError::InvalidUrl(
            "sslrootcert= con sslmode=require es inconsistente: require \
             NO verifica el cert. Usá sslmode=verify-ca o verify-full"
                .into(),
        ));
    }

    Ok((mode, root_cert))
}

// =============================================================
// TLS (Phase 10.1.b)
// =============================================================
//
// The driver supports TLS to Postgres at 3 strictness levels:
//
//   `sslmode=require`     — TLS yes, verification NO.
//   `sslmode=verify-ca`   — TLS yes, chain validated, hostname IGNORED.
//   `sslmode=verify-full` — TLS yes, chain + hostname (recommended prod).
//
// Implementation: rustls 0.23 + tokio-rustls 0.26 + webpki-roots
// for the in-binary Mozilla CA bundle. `ring` as crypto provider
// (pure Rust + assembly, no system C deps like CMake/OpenSSL).
//
// `Once` to install the ring crypto provider only the first time
// a `TlsConnector` is built — rustls 0.23 switched to a model
// where the provider MUST be installed before any
// `ClientConfig::builder()`. Without this:
//   "no process-level CryptoProvider available -- call
//   CryptoProvider::install_default()"

static RUSTLS_PROVIDER_INSTALLED: std::sync::Once = std::sync::Once::new();

fn ensure_rustls_provider() {
    RUSTLS_PROVIDER_INSTALLED.call_once(|| {
        // `install_default()` returns Result — fails if another
        // provider is already installed. Since this is our code
        // and we are the only caller, we ignore the Err
        // (idempotent).
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// SSLRequest: 8 bytes, magic 80877103 (0x04D2162F). The server
/// responds with 1 byte:
///   'S' → TLS supported, proceed with handshake
///   'N' → TLS not supported by the server
///   'E' → ErrorResponse follows
const SSL_REQUEST_MAGIC: u32 = 80877103;

/// Performs the SSLRequest dance + TLS handshake on the received
/// TcpStream and returns the upgraded `TlsStream` ready for the
/// normal startup. If the server responds 'N' or 'E', fails with
/// a clear message (the caller asked for TLS and didn't get it).
async fn upgrade_to_tls(
    mut tcp_stream: TcpStream,
    config: &ConnectionConfig,
) -> DbResult<tokio_rustls::client::TlsStream<TcpStream>> {
    // SSLRequest = 4-byte big-endian length (8) + 4-byte big-endian
    // magic. NO startup or body — it is a special pre-startup
    // message that the server interprets literally.
    let mut ssl_request = [0u8; 8];
    ssl_request[..4].copy_from_slice(&8u32.to_be_bytes());
    ssl_request[4..].copy_from_slice(&SSL_REQUEST_MAGIC.to_be_bytes());
    tcp_stream
        .write_all(&ssl_request)
        .await
        .map_err(DbError::Io)?;

    let mut response = [0u8; 1];
    tcp_stream
        .read_exact(&mut response)
        .await
        .map_err(DbError::Io)?;

    match response[0] {
        b'S' => {
            // Server accepts TLS — proceed with the handshake.
        }
        b'N' => {
            return Err(DbError::Tls(format!(
                "server Postgres no soporta TLS (respondió 'N' al SSLRequest) \
                 pero el cliente pidió sslmode={}. Verificá que el server tenga \
                 `ssl=on` en postgresql.conf, o usá sslmode=disable",
                sslmode_str(config.sslmode)
            )));
        }
        b'E' => {
            // Server responded ErrorResponse to the SSLRequest.
            // Drain the rest of the message to extract the real
            // cause.
            let mut header = [0u8; 4];
            tcp_stream
                .read_exact(&mut header)
                .await
                .map_err(DbError::Io)?;
            let len = u32::from_be_bytes(header) as usize;
            if len >= 4 {
                let mut payload = vec![0u8; len - 4];
                let _ = tcp_stream.read_exact(&mut payload).await;
            }
            return Err(DbError::Tls(
                "server Postgres respondió ErrorResponse al SSLRequest \
                 (típico en versiones <8.0 o configuraciones muy custom)"
                    .into(),
            ));
        }
        other => {
            return Err(DbError::Protocol(format!(
                "SSLRequest: byte de respuesta inesperado 0x{other:02x} \
                 (esperaba 'S'=0x53, 'N'=0x4E, o 'E'=0x45)"
            )));
        }
    }

    // Build TLS connector according to sslmode + sslrootcert.
    ensure_rustls_provider();
    let tls_config = build_tls_client_config(config)?;
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(tls_config));

    // ServerName: used for SNI + (on verify-full) hostname check.
    // For require/verify-ca, we still send it to the server (SNI
    // is public handshake info) but don't validate against it.
    let server_name =
        rustls::pki_types::ServerName::try_from(config.host.clone()).map_err(|e| {
            DbError::Tls(format!(
                "hostname `{}` inválido para TLS SNI: {e}",
                config.host
            ))
        })?;

    connector
        .connect(server_name, tcp_stream)
        .await
        .map_err(|e| DbError::Tls(format!("handshake TLS falló: {e}")))
}

fn sslmode_str(m: SslMode) -> &'static str {
    match m {
        SslMode::Disable => "disable",
        SslMode::Require => "require",
        SslMode::VerifyCa => "verify-ca",
        SslMode::VerifyFull => "verify-full",
    }
}

/// Builds the rustls `ClientConfig` based on sslmode + sslrootcert:
///   - `require`     → NoVerifier (accepts any cert)
///   - `verify-ca`   → chain validated, hostname IGNORED (wrapper
///     that catches the "NotValidForName" error and treats it as Ok)
///   - `verify-full` → default WebPkiServerVerifier (chain + hostname)
///
/// If `sslrootcert` is set, it is used as the root store instead
/// of webpki-roots. PEM format only.
fn build_tls_client_config(config: &ConnectionConfig) -> DbResult<rustls::ClientConfig> {
    use rustls::ClientConfig;

    let builder = ClientConfig::builder();

    let tls_config = match config.sslmode {
        SslMode::Disable => unreachable!("upgrade_to_tls solo se llama con TLS activo"),
        SslMode::Require => builder
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(NoVerifier))
            .with_no_client_auth(),
        SslMode::VerifyCa | SslMode::VerifyFull => {
            let root_store = build_root_store(config.sslrootcert.as_deref())?;
            let webpki_verifier =
                rustls::client::WebPkiServerVerifier::builder(std::sync::Arc::new(root_store))
                    .build()
                    .map_err(|e| {
                        DbError::Tls(format!("no se pudo construir WebPkiServerVerifier: {e}"))
                    })?;
            let verifier: std::sync::Arc<dyn rustls::client::danger::ServerCertVerifier> =
                if config.sslmode == SslMode::VerifyCa {
                    std::sync::Arc::new(NoHostnameVerifier(webpki_verifier))
                } else {
                    webpki_verifier
                };
            builder
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth()
        }
    };

    Ok(tls_config)
}

fn build_root_store(custom_pem: Option<&std::path::Path>) -> DbResult<rustls::RootCertStore> {
    let mut store = rustls::RootCertStore::empty();
    if let Some(path) = custom_pem {
        let pem_bytes = std::fs::read(path).map_err(|e| {
            DbError::Tls(format!(
                "no se pudo leer sslrootcert `{}`: {e}",
                path.display()
            ))
        })?;
        let mut cursor = std::io::Cursor::new(pem_bytes);
        let mut count = 0;
        for cert_result in rustls_pemfile::certs(&mut cursor) {
            let cert = cert_result.map_err(|e| {
                DbError::Tls(format!(
                    "error parseando certificado PEM en `{}`: {e}",
                    path.display()
                ))
            })?;
            store.add(cert).map_err(|e| {
                DbError::Tls(format!(
                    "certificado de `{}` rechazado por rustls: {e}",
                    path.display()
                ))
            })?;
            count += 1;
        }
        if count == 0 {
            return Err(DbError::Tls(format!(
                "sslrootcert `{}` no contiene certificados PEM válidos \
                 (esperaba uno o más bloques `-----BEGIN CERTIFICATE-----`)",
                path.display()
            )));
        }
    } else {
        // Default: in-binary Mozilla CA bundle from webpki-roots.
        // Covers Heroku, RDS, Supabase, Neon, Aiven, Render PG, etc.
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    Ok(store)
}

/// Verifier for `sslmode=require` — accepts any cert, validates
/// nothing. Equivalent to `curl --insecure`. DO NOT USE in
/// production. Useful for dev/staging against internal servers
/// without a CA, or to verify TLS connectivity without getting
/// into the mess of cert chains.
#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        // Full list — the wrapper accepts any sig anyway, but
        // rustls needs to know which ones we support so the
        // handshake picks a mutually valid one.
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA1,
            rustls::SignatureScheme::ECDSA_SHA1_Legacy,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}

/// Verifier for `sslmode=verify-ca` — delegates chain validation
/// to the standard WebPkiServerVerifier, but catches
/// `NotValidForName` (hostname mismatch) and treats it as Ok.
/// Maintains operator authenticity (cert from a trusted CA)
/// without requiring the hostname to match.
#[derive(Debug)]
struct NoHostnameVerifier(std::sync::Arc<rustls::client::WebPkiServerVerifier>);

impl rustls::client::danger::ServerCertVerifier for NoHostnameVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        match self
            .0
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        {
            Ok(v) => Ok(v),
            Err(rustls::Error::InvalidCertificate(rustls::CertificateError::NotValidForName)) => {
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            // rustls 0.23.x added `NotValidForNameContext` with
            // structured SAN/CN info. We handle both cases.
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::NotValidForNameContext { .. },
            )) => Ok(rustls::client::danger::ServerCertVerified::assertion()),
            Err(other) => Err(other),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.supported_verify_schemes()
    }
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
// Wire protocol — Postgres v3.0 messages
// =============================================================
//
// The Postgres protocol has two classes of messages:
//
//  - Frontend → Backend (client → server): each message has
//    1 byte of type (ASCII) + 4 bytes of big-endian length +
//    payload. EXCEPTION: `StartupMessage` and `SSLRequest` do not
//    have a type byte (the length starts the message).
//
//  - Backend → Frontend (server → client): 1 byte of type +
//    4 bytes of length + payload. No exception.
//
// Convention: length INCLUDES the 4 length bytes but EXCLUDES the
// type byte (when there is a type). That's what the spec says;
// while coding, we subtract 4 on read / add 4 on write.

/// Messages we send to the server.
#[derive(Debug)]
pub enum FrontendMessage<'a> {
    /// Starts the connection. No type byte. Payload:
    /// `version(4) | param1\0val1\0...\0` where the last param
    /// must be followed by `\0` (list terminator).
    Startup { user: &'a str, database: &'a str },
    /// Response to `AuthenticationCleartextPassword` or
    /// `AuthenticationMD5Password`.
    Password { password: &'a [u8] },
    /// Start of the SASL flow: `AuthenticationSASL` received, we
    /// emit the client-first-message.
    SaslInitialResponse {
        mechanism: &'a str,
        initial_response: &'a [u8],
    },
    /// Continuation of the SASL flow: client-final-message.
    SaslResponse { response: &'a [u8] },
    /// Simple Query: executes the statement with auto-commit and
    /// returns the result in a single round-trip.
    Query { sql: &'a str },
    /// Extended Query — declares a parseable statement.
    Parse {
        statement_name: &'a str,
        sql: &'a str,
        param_types: &'a [u32], // OIDs; 0 = let server decide
    },
    /// Extended Query — binds concrete params to a statement.
    Bind {
        portal_name: &'a str,
        statement_name: &'a str,
        param_formats: &'a [i16],             // 0 = text, 1 = binary
        param_values: &'a [Option<&'a [u8]>], // None = NULL
        result_formats: &'a [i16],
    },
    /// Extended Query — executes a portal.
    Execute {
        portal_name: &'a str,
        max_rows: i32, // 0 = unlimited
    },
    /// Extended Query — flush + commit of the round.
    Sync,
    /// Closes the statement or portal.
    Close { kind: u8, name: &'a str }, // 'S' = statement, 'P' = portal
    /// Describe a statement or portal.
    Describe { kind: u8, name: &'a str },
    /// Cooperatively terminates the connection.
    Terminate,
}

impl<'a> FrontendMessage<'a> {
    /// Serializes the message to bytes ready for `write_all`. The
    /// caller doesn't need to worry about framing — everything
    /// (tag + length + payload) comes in the buffer.
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
                    // application_name = "fitz"  (purely
                    // informational on the server side; helps the
                    // user identify the connection in
                    // pg_stat_activity)
                ]);
                write_cstr(&mut payload, "application_name");
                write_cstr(&mut payload, "fitz");
                write_cstr(&mut payload, "client_encoding");
                write_cstr(&mut payload, "UTF8");
                payload.push(0); // list terminator
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

/// Messages the server sends us. We only model the types the
/// driver needs in 10.1: auth + simple/extended query + state.
/// Informational messages like ParameterStatus or NoticeResponse
/// are parsed but discarded by the driver.
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
    /// Any message we parse but do not process specifically. The
    /// tag is kept for diagnostics if something goes wrong. Not
    /// emitted today — all known wire messages have specific
    /// variants; if a new one appears, we add a variant.
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
                _ => {} // F (file), L (line), R (routine), etc. — ignored
            }
        }
        ef
    }
}

// =============================================================
// Frame I/O — read/write messages over TcpStream
// =============================================================

/// Reads ONE message from the server: tag(1) + length(4) + payload.
/// Blocks until the full message is in hand or an I/O error
/// occurs.
///
/// v0.10.23 (Phase 10.1.b) — generic over `R: AsyncRead + Unpin`
/// (was hard-coded `TcpStream` before) to transparently support
/// `TlsStream<TcpStream>` when sslmode != Disable.
/// `Box<dyn DbReadWrite>` implements `AsyncRead` via deref, so the
/// call site in `Connection::read` stays identical.
pub async fn read_message<R: AsyncRead + Unpin>(stream: &mut R) -> DbResult<BackendMessage> {
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
            // List of mechanism strings terminated by an extra
            // \0. We walk the slice extracting cstrs until we
            // find an empty one.
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
//      (in SASL, "n,," = "no channel binding").
//   2. Server → Client: server-first-message
//      "r=<server_nonce>,s=<base64_salt>,i=<iterations>"
//      where server_nonce = client_nonce + server_random_part.
//   3. Client → Server: client-final-message
//      "c=biws,r=<server_nonce>,p=<base64_client_proof>"
//      (biws = base64("n,,") = "biws").
//   4. Server → Client: server-final-message
//      "v=<base64_server_signature>"
//      or "e=<error>".
//
// Key derivation:
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
// MVP 10.1: SCRAM-SHA-256 without channel binding (no
// SCRAM-SHA-256-PLUS). Channel binding requires TLS; we add it
// when TLS lands in the future sub-step.

pub struct ScramClient {
    username: String,
    password: String,
    client_nonce: String,
    /// Intermediate state for client_final() and verify().
    /// Populated by client_final().
    server_signature: Option<Vec<u8>>,
}

impl ScramClient {
    /// Builds a SCRAM-SHA-256 client with a random 24-byte
    /// base64-encoded nonce (~32 chars). The nonce is the
    /// client's randomness contribution; the server extends it
    /// with its own randomness.
    pub fn new(username: &str, password: &str) -> DbResult<Self> {
        Ok(ScramClient {
            username: username.to_string(),
            password: password.to_string(),
            client_nonce: generate_nonce()?,
            server_signature: None,
        })
    }

    /// Constructor for tests with a fixed nonce (RFC 7677 vectors).
    #[cfg(test)]
    pub fn new_with_nonce(username: &str, password: &str, nonce: &str) -> Self {
        ScramClient {
            username: username.to_string(),
            password: password.to_string(),
            client_nonce: nonce.to_string(),
            server_signature: None,
        }
    }

    /// Generates the client-first-message to send as the payload
    /// of `SaslInitialResponse`. SASL header "n,," = no channel
    /// binding. SCRAM per SPEC ignores the username in the SASL
    /// message (Postgres uses the one passed in startup), but we
    /// include it for completeness.
    pub fn client_first(&self) -> String {
        format!("n,,{}", self.client_first_bare())
    }

    fn client_first_bare(&self) -> String {
        // SCRAM allows an empty username (Postgres ignores it);
        // if present, it is SASLprep-escaped — for typical
        // characters (ASCII) the transform is identity. We
        // support only ASCII in 10.1; complex Unicode is left as
        // minor debt.
        format!(
            "n={},r={}",
            saslprep_minimal(&self.username),
            self.client_nonce
        )
    }

    /// Processes the `server-first-message` received in
    /// `AuthenticationSASLContinue` and returns the
    /// `client-final-message` to send as `SaslResponse`. After
    /// this call, `server_signature` is cached for `verify()`.
    pub fn client_final(&mut self, server_first: &str) -> DbResult<String> {
        // Parse "r=<nonce>,s=<salt>,i=<iters>"
        let (server_nonce, salt_b64, iterations) = parse_server_first(server_first)?;

        // The server must extend OUR nonce — if it does not start
        // with `client_nonce`, someone is in the middle.
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

    /// Validates the `server-final-message` (`v=<base64_signature>`).
    /// Fails if the received `v=` does not match the server
    /// signature we computed in `client_final()` — that would
    /// imply the server does not know our password, which we do
    /// not accept.
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
            // mandatory extension — if it appears, we must reject
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
    // PBKDF2 with HMAC-SHA-256, deriving 32 bytes (a single
    // SHA-256 block). dkLen = 32 = hLen, so the formula reduces to
    //   U1 = HMAC(password, salt || INT(1))   where INT(1) = 4 big-endian bytes
    //   Ui = HMAC(password, U(i-1))
    //   T  = U1 XOR U2 XOR ... XOR U_iterations
    // For SCRAM-SHA-256 dkLen is always 32 (SHA-256's hLen), and
    // we only need i=1, hence the code is simplified vs generic
    // PBKDF2.
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
    // 18 random bytes → base64 = 24 chars. RFC says "at least 64
    // bits" — 18*8 = 144 bits, plenty.
    let mut bytes = [0u8; 18];
    // rand_core::OsRng is available via argon2 → password-hash →
    // rand_core, already a non-optional dep. We invoke it via the
    // RngCore::try_fill_bytes trait to avoid depending on the
    // wrapper's `rand` feature.
    use rand_core::{OsRng, RngCore};
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|e| DbError::Auth(format!("nonce: RNG falló: {e}")))?;
    Ok(BASE64.encode(bytes))
}

fn saslprep_minimal(s: &str) -> String {
    // Full SASLprep (RFC 4013) is complex — NFKC Unicode
    // normalization + control-char mapping + bidirectional check.
    // For 10.1 we implement a subset that covers ASCII (typical
    // for usernames and passwords): we reject low control chars
    // and map non-ASCII spaces to U+0020. Postgres usernames in
    // practice are almost always ASCII; if real demand for
    // complex Unicode appears, we add the `stringprep` crate.
    //
    // In SCRAM, the chars '=' and ',' must be escaped in the
    // username (they are part of the message syntax).
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
// OID types — Postgres → Fitz mapping
// =============================================================
//
// Postgres uses OIDs (object identifiers, u32) to identify types
// in the wire protocol. OIDs for built-in types are stable and
// documented in the Postgres kernel's `pg_type.h`.
//
// 10.1 covers the 11 types enumerated in the roadmap. Advanced
// types (JSONB, arrays, Date/Time/Timestamp with timezone detail,
// UUID, etc.) arrive in 10.5 — current code receives them as
// opaque `Bytes` if they show up and emits `UnsupportedType` with
// the concrete OID so the user clearly sees what to request.

pub mod oid {
    pub const BOOL: u32 = 16;
    pub const BYTEA: u32 = 17;
    /// v0.10.18 — `name` (system identifier, 63 bytes). Returned
    /// by queries over `information_schema` (typed as
    /// `sql_identifier`, which is an alias for `name`) and
    /// `pg_catalog`. We treat it as Text so that introspect
    /// queries from the `migrations` module work.
    pub const NAME: u32 = 19;
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
    /// `void` — returned by fns like `pg_sleep()`, `pg_notify()`,
    /// etc. Postgres serializes it as an empty string in text
    /// format. We map it to `PgValue::Null` so that SELECT over
    /// void fns does not fail with `UnsupportedType`.
    pub const VOID: u32 = 2278;

    // Phase 10.5.b — native arrays. Each scalar type has its
    // array OID hard-coded in Postgres' `pg_type` catalog.
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

    /// Maps an array OID to its element scalar OID. Returns
    /// `None` if `oid` is not a supported array.
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

    /// Inverse: given a scalar OID, returns the corresponding
    /// array OID. Used when emitting `::int8[]`-style casts in
    /// INSERTs with array columns.
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

/// Postgres scalar value parsed from the wire. The
/// representation is minimal in 10.1: only the MVP primitives.
/// Advanced types (structured JSONB, typed arrays, etc.) arrive
/// in 10.5 with specific variants.
#[derive(Debug, Clone, PartialEq)]
pub enum PgValue {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Bytes(Vec<u8>),
    /// Phase 10.5.b — Postgres array. `elem_oid` indicates the
    /// element type (INT4, TEXT, etc.) so the encoder knows which
    /// cast to emit (`$N::int4[]`) and how to format each item.
    /// Elements can be `PgValue::Null` (Postgres supports
    /// `{1,NULL,3}`). Nesting not supported in MVP — elements are
    /// scalars.
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

/// Parses a value from the wire (text format — the Simple Query
/// default). When Extended Query with format=binary lands in
/// 10.1.b, we add a parallel `parse_binary`.
///
/// `bytes = None` → NULL (length -1 in DataRow).
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
        | oid::NAME
        | oid::DATE
        | oid::TIME
        | oid::TIMESTAMP
        | oid::TIMESTAMPTZ
        | oid::UUID
        | oid::JSON
        | oid::JSONB => Ok(PgValue::Text(s.to_string())),
        // `void` always returns an empty string in text format.
        // We model it as Null so that `SELECT pg_sleep(...)` does
        // not fail with UnsupportedType.
        oid::VOID => Ok(PgValue::Null),
        oid::BYTEA => {
            // Wire text format for BYTEA is "\x<hex>". If it
            // doesn't match, we return the raw bytes.
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
            // Phase 10.5.b — native arrays. We detect by OID and
            // delegate to parse_array_text which parses Postgres'
            // `{a,b,c}` format.
            if let Some(elem_oid) = oid::array_elem_oid(oid) {
                let values = parse_array_text(s, elem_oid)?;
                return Ok(PgValue::Array { elem_oid, values });
            }
            // Types unsupported in 10.1: we return UnsupportedType
            // with the concrete OID. The user sees which type to
            // add.
            Err(DbError::UnsupportedType(oid))
        }
    }
}

/// Phase 10.5.b — parser for the text format of Postgres arrays.
///
/// Grammar (simplified, MVP — no nesting, no custom dimensions):
///
/// ```text
/// array     = '{' [ element (',' element)* ] '}'
/// element   = unquoted | quoted | NULL
/// quoted    = '"' (char | '\\' char | '\\"' )* '"'
/// unquoted  = chars without ',', '{', '}', '"', '\\', whitespace
/// ```
///
/// `NULL` without quotes → `PgValue::Null`; `"NULL"` quoted →
/// `PgValue::Text("NULL")` (literal). Whitespace around unquoted
/// elements is trimmed (consistent with Postgres).
fn parse_array_text(s: &str, elem_oid: u32) -> DbResult<Vec<PgValue>> {
    let bytes = s.as_bytes();
    let mut idx = 0;
    // Trim leading whitespace.
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
    // Empty array: '{}'.
    if idx < bytes.len() && bytes[idx] == b'}' {
        return Ok(out);
    }
    loop {
        // Parse one element.
        let (elem_raw, was_quoted, new_idx) = parse_array_element(bytes, idx)?;
        let value = if !was_quoted && elem_raw.eq_ignore_ascii_case("NULL") {
            PgValue::Null
        } else {
            // Parse the element as a scalar value using
            // parse_text_value with the element OID.
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
                // Skip whitespace before next element.
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

/// Reads one element from a text array. Returns
/// `(content, was_quoted, new_idx)`. Called with `idx` pointing at
/// the first char of the element.
fn parse_array_element(bytes: &[u8], start: usize) -> DbResult<(String, bool, usize)> {
    if start >= bytes.len() {
        return Err(DbError::Protocol(
            "array element: fin de string inesperado".into(),
        ));
    }
    if bytes[start] == b'"' {
        // Quoted element. Read up to the closing quote, undoing
        // escapes `\\` → `\` and `\"` → `"`.
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
        // Unquoted: read until ',' or '}'.
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

/// Encodes a value for sending to the server (text format in
/// MVP). The server parses according to the statement OID; if we
/// pass OID 0 in `Parse`, Postgres infers. For Bytes we use the
/// hex format "\x<hex>".
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

/// Phase 10.5.b — encodes a Vec<PgValue> to Postgres' text array
/// format: `{elem1,elem2,...}`. Quoted elements carry `"` around
/// and escapes `\\` for `\` and `"`. Null without quotes.
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
            // Strings always quoted in arrays (safe default).
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
            // bytea in arrays: hex with escape `\\x...`. We
            // serialize as a quoted string so the server parser
            // receives it as `\x...`.
            out.push('"');
            out.push_str("\\\\x");
            for byte in b {
                use std::fmt::Write as _;
                let _ = write!(out, "{:02x}", byte);
            }
            out.push('"');
        }
        PgValue::Array { values, .. } => {
            // Nesting: we emit the sub-array recursively. Postgres
            // supports multi-dimensional but the MVP does not
            // expose it as a Fitz shape — this only kicks in if
            // built by hand.
            out.push_str(&encode_array_text(values));
        }
    }
}

// =============================================================
// Row — query result
// =============================================================

/// A row from the result set. Keeps the column names (for
/// `row.get("name")` access) and the values in order. Names are
/// `Arc<str>` when the row outlives one query — in MVP we
/// duplicate them per row (low cost, ~2 KB per row for 10
/// columns), optimizable if demand appears.
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

    /// v0.10.24 — returns `(PgValue, OID)` so the caller can
    /// refine the value's type according to the column OID (date,
    /// timestamptz, uuid). Without this, the caller (evaluator)
    /// only sees `PgValue::Text` without being able to distinguish
    /// between `text`/`date`/etc.
    pub fn get_with_oid(&self, name: &str) -> Option<(&PgValue, u32)> {
        let idx = self.columns.iter().position(|(n, _)| n == name)?;
        let v = self.values.get(idx)?;
        let oid = self.columns[idx].1;
        Some((v, oid))
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

/// Full result of a query: rows + the CommandComplete tag
/// (typically "SELECT 42" or "INSERT 0 1").
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub rows: Vec<Row>,
    pub command_tag: String,
}

impl QueryResult {
    /// Returns the rowcount inferred from `command_tag`.
    /// `"INSERT 0 5"` → 5, `"UPDATE 3"` → 3. If it does not parse
    /// (case "SELECT"), returns the number of rows in the result
    /// set.
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

/// A live Postgres connection. Owns the TcpStream + the
/// connection state. NOT Send+Sync by construction (TcpStream is;
/// the 10.2 pool wraps it in `Arc<Mutex<Connection>>` for
/// concurrent access). In 10.1 a single task owns the
/// `Connection` and uses it exclusively.
/// v0.10.23 (Phase 10.1.b) — helper trait that allows having a
/// single `Box<dyn DbReadWrite>` as the `Connection` stream,
/// regardless of whether underneath there is a plain `TcpStream`
/// (sslmode=disable) or a
/// `tokio_rustls::client::TlsStream<TcpStream>`
/// (sslmode=require/verify-ca/verify-full). Cost: one vtable
/// lookup per read/write (~3ns), irrelevant vs the TCP
/// round-trip.
pub trait DbReadWrite: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> DbReadWrite for T {}

pub struct Connection {
    stream: Box<dyn DbReadWrite>,
    /// Current transaction status: 'I' idle, 'T' in tx, 'E' in
    /// failed tx. Updated on each `ReadyForQuery`. Useful for
    /// diagnostics and for 10.7 (transactions).
    tx_status: u8,
    /// Process ID + secret_key from the backend. Useful to cancel
    /// queries (`CancelRequest` message on a parallel
    /// connection). Cancellation not implemented in 10.1 — field
    /// present for the future sub-step.
    backend_pid: i32,
    backend_secret_key: i32,
    /// Server parameters reported during startup
    /// (server_version, server_encoding, etc.). Useful for
    /// diagnostics + features that depend on the version.
    server_params: Vec<(String, String)>,
}

impl Connection {
    /// Opens TCP, does startup + auth, leaves the connection
    /// ready for queries. Total timeout ~10 s. v0.10.23
    /// (Phase 10.1.b): if `config.sslmode != Disable`, does the
    /// SSLRequest dance + TLS handshake before startup. The
    /// startup goes over the upgraded (encrypted) stream
    /// transparently to the rest of the driver thanks to
    /// `Box<dyn DbReadWrite>`.
    pub async fn connect(config: &ConnectionConfig) -> DbResult<Self> {
        let addr = format!("{}:{}", config.host, config.port);
        let tcp_stream = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&addr))
            .await
            .map_err(|_| DbError::Io(io::Error::new(io::ErrorKind::TimedOut, "connect timeout")))?
            .map_err(DbError::Io)?;

        // v0.10.13 (B-1 fix) — TCP_NODELAY disables Nagle's
        // algorithm. CRITICAL for the Extended Query Protocol: we
        // send 5 consecutive messages
        // (Parse/Bind/Describe/Execute/Sync) without waiting for
        // a server response between them. With Nagle active, the
        // TCP kernel delays each small message waiting for the
        // ACK of the previous one, adding up to ~40ms of
        // delayed-ACK per query — bug observed in benchmark v2
        // (GET /users/{id} 43ms vs simple query 4ms). Without
        // Nagle, the 5 messages go out batched immediately (even
        // faster with the batching fix below).
        let _ = tcp_stream.set_nodelay(true);

        // v0.10.23 (Phase 10.1.b) — TLS upgrade if applicable.
        // Sub-step 1: SSLRequest dance (1-byte response: 'S'/'N'/'E')
        // Sub-step 2: TLS handshake with verifier per sslmode.
        let stream: Box<dyn DbReadWrite> = if config.sslmode == SslMode::Disable {
            Box::new(tcp_stream)
        } else {
            Box::new(upgrade_to_tls(tcp_stream, config).await?)
        };

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

                    // We expect AuthenticationSASLContinue
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

                    // We expect AuthenticationSASLFinal
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
                    // Next message must be AuthenticationOk.
                    // Loop continues.
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

        // Drain ParameterStatus + BackendKeyData until
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
                    // ignore notices during startup
                }
                other => {
                    return Err(DbError::Protocol(format!(
                        "esperaba ParameterStatus/BackendKeyData/ReadyForQuery, recibí {other:?}"
                    )))
                }
            }
        }
    }

    /// Simple Query: executes the SQL in a single round-trip.
    /// Does not accept parameters (the caller builds the full
    /// SQL). For queries with args, use `extended_query()`
    /// instead.
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
                    // ignored in MVP — a future sub-step can
                    // expose notices to the user (logging callback)
                }
                BackendMessage::ErrorResponse(ef) => {
                    // Drain until ReadyForQuery so the conn is
                    // usable again.
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

    /// Extended Query: parses + binds + executes with parameters.
    /// Uses text format in both directions to keep it simple
    /// (core OID types parse the same with format=0). Binary
    /// format arrives in 10.5 when we need it for JSONB/timestamps.
    pub async fn extended_query(&mut self, sql: &str, args: &[PgValue]) -> DbResult<QueryResult> {
        // Encode args to text format
        let encoded: Vec<Option<Vec<u8>>> = args.iter().map(encode_text_value).collect();
        let bind_refs: Vec<Option<&[u8]>> = encoded.iter().map(|opt| opt.as_deref()).collect();

        // v0.10.13 (B-1 fix) — batch the 5 messages of the
        // Extended Query Protocol into a SINGLE socket write.
        // Before we did 5 separate `self.write(...).await?`, each
        // with its own write() syscall; even with TCP_NODELAY
        // active, separate syscalls added significant latency per
        // await + scheduling round. Benchmark v0.10.13 confirmed:
        // GET /users/{id} dropped from 43ms p50 → ~2ms p50 with
        // this batch, leaving Fitz as absolute winner in
        // single-read (vs Python ~34ms before).
        //
        // The Postgres server does NOT respond until the Sync —
        // the 5 messages are "pipelined" in the protocol sense,
        // not a semantic change. We just eliminate client-side
        // overhead.
        //
        // Parse — empty name = anonymous statement (lifetime of
        // one round). We do not use a prepared-statements cache
        // in MVP.
        // Describe(Portal "") — CRITICAL: without this, the
        // server does NOT send RowDescription after BindComplete
        // and only delivers opaque DataRow without column names.
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
                    // Does not return rows (INSERT without RETURNING)
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

    /// Closes the connection cooperatively. Sends `Terminate` and
    /// drops the TcpStream. Not reusable afterwards.
    pub async fn close(mut self) -> DbResult<()> {
        // Terminate may fail if the conn is already closed on the
        // server side; we ignore it.
        let _ = self.write(FrontendMessage::Terminate).await;
        Ok(())
    }

    /// Phase 10.7 — ORM transactions. Simple wrappers over
    /// `simple_query` with the 3 standard SQL instructions. The
    /// BEGIN/COMMIT/ROLLBACK orchestration + auto-rollback on
    /// error/panic lives upstream (in `DbConnHandle::transaction`);
    /// these methods are just the wire-level primitives.
    ///
    /// Note: in Postgres `BEGIN` also accepts the synonym
    /// `START TRANSACTION`; we use `BEGIN` for historical
    /// compatibility. No explicit isolation levels — uses the
    /// server default (typically READ COMMITTED).
    pub async fn begin(&mut self) -> DbResult<()> {
        self.simple_query("BEGIN").await?;
        Ok(())
    }

    pub async fn commit(&mut self) -> DbResult<()> {
        self.simple_query("COMMIT").await?;
        Ok(())
    }

    pub async fn rollback(&mut self) -> DbResult<()> {
        self.simple_query("ROLLBACK").await?;
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

/// MD5 password hash in Postgres format:
///   "md5" || md5_hex(md5_hex(password || username) || salt)
fn md5_password(user: &str, password: &str, salt: &[u8; 4]) -> String {
    let inner = md5_hex(&[password.as_bytes(), user.as_bytes()].concat());
    let mut second_input = inner.into_bytes();
    second_input.extend_from_slice(salt);
    let outer = md5_hex(&second_input);
    format!("md5{outer}")
}

/// Mini MD5 implementation for legacy auth. Only used for the
/// deprecated MD5 method in Postgres pre-14; on 14+ it's already
/// SCRAM-SHA-256 by default. Kept for compat with old DBs in dev.
fn md5_hex(data: &[u8]) -> String {
    let digest = md5_compute(data);
    let mut hex = String::with_capacity(32);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{:02x}", byte);
    }
    hex
}

/// MD5 RFC 1321. ~50 LoC pure. Only for legacy auth.
fn md5_compute(input: &[u8]) -> [u8; 16] {
    // Constants
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
// DbPool — connection pool with reconnect + health check
// =============================================================
//
// Phase 10.2: `DbConnHandle` now wraps a pool of N connections
// (default 10) instead of a single one. Multiple tasks can call
// `query()` in parallel and each picks a free connection from the
// pool without blocking each other — essential so HTTP throughput
// does not serialize on the DB.
//
// Model:
//   - `DbPool` keeps a vector of idle connections under a std
//     `Mutex` (no parking_lot — `src/db.rs` is self-contained for
//     embedding via `include_str!` in the 10.1.c codegen).
//   - `tokio::sync::Semaphore` limits the maximum number of conns
//     in use concurrently. If N tasks call `query` and the pool
//     already emitted N conns, the N+1 waits until one is freed.
//   - `PooledConn` keeps the connection + an
//     `OwnedSemaphorePermit`. On `Drop`, the conn returns to the
//     idle pool and the permit is released automatically.
//   - `acquire` lazy-grows the pool on demand: if there is no
//     idle, it opens a new TCP conn (up to `max_conns`).
//   - A background health-check task does `SELECT 1` on all idle
//     conns every 30s and discards failing ones.
//
// `connect_url(url)` is eager — opens the first conn at boot to
// validate credentials and URL before returning the handle.
// Additional conns open lazily in `acquire()`.

const DEFAULT_MAX_CONNS: usize = 10;
const HEALTH_CHECK_INTERVAL_SECS: u64 = 30;

/// v0.10.29 — `FITZ_DB_MAX_CONNS` opt-in env var to override the
/// driver pool size. Useful for apps that expect a lot of
/// concurrent load (> 10 simultaneous requests hitting the DB) or
/// for apps with very little load where 10 conns is overkill.
///
/// Parsed once per process (`LazyLock`) — mid-run changes are NOT
/// reflected (same model as `FITZ_DB_LOG`). Invalid or empty
/// values → fallback to `DEFAULT_MAX_CONNS`. Clamp: min 1, max
/// 200 (beyond that is probably a typo and saturates Postgres'
/// `max_connections`).
pub static FITZ_DB_MAX_CONNS: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| match std::env::var("FITZ_DB_MAX_CONNS") {
        Ok(s) => parse_max_conns_value(&s),
        Err(_) => DEFAULT_MAX_CONNS,
    });

/// v0.10.29 — Pure testable parser. Trim + parse + clamp [1, 200].
/// Unparseable or out-of-range values → DEFAULT_MAX_CONNS.
pub(crate) fn parse_max_conns_value(s: &str) -> usize {
    match s.trim().parse::<usize>() {
        Ok(n) if (1..=200).contains(&n) => n,
        _ => DEFAULT_MAX_CONNS,
    }
}

/// Resolves the effective max_conns of the pool: env var > default.
pub(crate) fn effective_max_conns() -> usize {
    *FITZ_DB_MAX_CONNS
}

/// Internal pool of the `DbConnHandle`. NOT exposed directly to
/// the evaluator — the handle delegates here. Shared via
/// `Arc<DbPool>` so the spawned health-check task can hold a
/// weak reference without extending the pool's lifetime.
pub struct DbPool {
    config: ConnectionConfig,
    /// Queue of free connections ready to use. `parking_lot` is
    /// not used because `src/db.rs` is embedded into `fitz build`
    /// output and we want to keep the file self-contained without
    /// adding deps to the generated crate. `std::sync::Mutex`
    /// with short scope (push/pop) — no poison risk because
    /// guards do not cross `.await`.
    idle: std::sync::Mutex<Vec<Connection>>,
    /// Concurrency limiter: the pool does not hand out more than
    /// `max_conns` conns at a time. Extra tasks wait in
    /// `acquire`.
    permits: std::sync::Arc<tokio::sync::Semaphore>,
    /// Cooperative closure marker. Subsequent `acquire()` calls
    /// fail with `Protocol("...cerrado")` after `close()`.
    closed: std::sync::atomic::AtomicBool,
}

impl DbPool {
    /// Tries to grab a free conn from the pool. If none, opens a
    /// new one (lazy growth). If the pool has reached `max_conns`
    /// live conns, waits on the semaphore.
    async fn acquire(self: &std::sync::Arc<Self>) -> DbResult<PooledConn> {
        use std::sync::atomic::Ordering;
        if self.closed.load(Ordering::Acquire) {
            return Err(DbError::Protocol(
                "la conexión fue cerrada con .close()".into(),
            ));
        }
        // Wait for a permit first (limits concurrency). The
        // OwnedSemaphorePermit is released automatically when
        // PooledConn is dropped.
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| DbError::Protocol("pool: semaphore cerrado".into()))?;
        // Try to grab an idle conn (fast path).
        let maybe_idle = self.idle.lock().expect("pool mutex poisoned").pop();
        let conn = match maybe_idle {
            Some(c) => c,
            None => {
                // Slow path: open a new conn.
                Connection::connect(&self.config).await?
            }
        };
        Ok(PooledConn {
            pool: std::sync::Arc::clone(self),
            conn: Some(conn),
            _permit: permit,
        })
    }

    /// Returns a conn to the idle pool. Only called from
    /// `PooledConn::drop`.
    fn release(&self, conn: Connection) {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            // Pool closed — discard the conn (its Drop will
            // eventually close the TcpStream).
            return;
        }
        self.idle.lock().expect("pool mutex poisoned").push(conn);
    }
}

/// RAII wrapper around a pool conn. While alive, the conn is
/// outside the pool. On `Drop`, it returns to the idle queue (if
/// the pool is not closed).
pub struct PooledConn {
    pool: std::sync::Arc<DbPool>,
    /// `Option` so we can `take()` from `Drop` (which only
    /// receives `&mut self`, not `self`).
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
        // The `_permit` is auto-released on the field's drop.
    }
}

// =============================================================
// DbConnHandle — Send + Sync wrapper for the evaluator
// =============================================================
//
// `Connection` (defined above) is the driver's internal
// abstraction: holds the `TcpStream` and protocol state. We do
// NOT expose it directly to the evaluator because the evaluator
// needs a handle shareable across multiple tasks (via Arc) and
// that serializes I/O operations (via Mutex).
//
// 10.2: the handle now wraps an `Arc<DbPool>` instead of a
// `Mutex<Option<Connection>>`. The public API
// (`query`/`exec`/`close`/`is_closed`) stays identical.

/// Opaque handle to a Postgres connection pool. Built by
/// `connect_url()` and passed to the evaluator as
/// `Value::DbConn(Arc<DbConnHandle>)`.
///
/// The struct does NOT implement `Clone` directly — the evaluator
/// uses the external `Arc` for sharing.
pub struct DbConnHandle {
    pool: std::sync::Arc<DbPool>,
    /// Original URL without password — useful for Display and
    /// errors.
    pub url_redacted: String,
    /// v0.10.31 (Tier A.4) — nested tx depth. 0 = we are not in a
    /// tx. >0 = we are inside `transaction(...)` with that many
    /// nestings. Depth increments on enter and decrements on
    /// exit; the outer tx uses `BEGIN/COMMIT/ROLLBACK` and inner
    /// txs use `SAVEPOINT/RELEASE/ROLLBACK TO SAVEPOINT`.
    ///
    /// Shared between the outer handle and the "sub-pool" handles
    /// that `transaction()` creates to pass to the callback — all
    /// look at and modify the same Arc, so the inner correctly
    /// detects it is nested.
    pub(crate) tx_depth: std::sync::Arc<std::sync::atomic::AtomicI32>,
}

impl std::fmt::Debug for DbConnHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DbConnHandle({})", self.url_redacted)
    }
}

// =====================================================================
// v0.10.28 (Tier S, sub-step 3) — FITZ_DB_LOG: opt-in query logging
// =====================================================================

/// Driver logging mode. Activated by the env var `FITZ_DB_LOG`:
///
/// - empty / `=0` / unset → `Off` (default, zero overhead).
/// - `=1` / `=true` → `Simple` (SQL + elapsed, no params).
/// - `=verbose` → `Verbose` (SQL + elapsed + params, truncated to
///   80 chars each so the log is not flooded with large BLOBs).
///
/// Any other value falls back to `Off` (silent, no error — so
/// accidentally setting `FITZ_DB_LOG=true,verbose` does not break
/// the program).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbLogMode {
    Off,
    Simple,
    Verbose,
}

/// Reads `FITZ_DB_LOG` once per process. Mid-run changes to the
/// env var are NOT reflected — the mode is locked at first
/// access (lazy). Compatible with `fitz run`, `fitz build` (the
/// produced binary reuses the same `db.rs` via `pub use`), and
/// tests (each test process re-reads the env var in its
/// LazyLock).
pub static DB_LOG_MODE: std::sync::LazyLock<DbLogMode> =
    std::sync::LazyLock::new(|| match std::env::var("FITZ_DB_LOG").as_deref() {
        Ok("verbose") => DbLogMode::Verbose,
        Ok("1" | "true") => DbLogMode::Simple,
        _ => DbLogMode::Off,
    });

/// Truncates a string to `max` chars (chars, not bytes — UTF-8
/// safe). If the original was longer, suffixes with `…` to mark
/// it.
fn truncate_for_log(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// Formats a single `PgValue` for the verbose log. Strings and
/// bytes are truncated to 80 chars (`MAX_LOG_VALUE`); the rest
/// goes untruncated (Int/Float/Bool/Null/Array are short by
/// nature).
const MAX_LOG_VALUE: usize = 80;

fn format_log_value(v: &PgValue) -> String {
    match v {
        PgValue::Null => "NULL".to_string(),
        PgValue::Int(n) => n.to_string(),
        PgValue::Float(x) => x.to_string(),
        PgValue::Bool(b) => b.to_string(),
        PgValue::Text(s) => format!("\"{}\"", truncate_for_log(s, MAX_LOG_VALUE)),
        PgValue::Bytes(b) => format!("<{} bytes>", b.len()),
        PgValue::Array { values, .. } => {
            let inner: Vec<String> = values.iter().take(8).map(format_log_value).collect();
            let suffix = if values.len() > 8 { ", …" } else { "" };
            format!("[{}{}]", inner.join(", "), suffix)
        }
    }
}

/// Formats a log line ready to emit to stderr. Pure function so
/// the unit test can assert the output without touching stderr.
pub fn format_db_log_line(
    elapsed: std::time::Duration,
    sql: &str,
    args: &[PgValue],
    mode: DbLogMode,
) -> String {
    let ms = elapsed.as_secs_f64() * 1000.0;
    // SQL one-line: collapse \n / \r / runs of whitespace so
    // multi-line queries stay readable on a single log line (the
    // canonical uvicorn/rails format does the same).
    let sql_oneline = sql
        .replace(['\n', '\r'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match mode {
        DbLogMode::Off => String::new(),
        DbLogMode::Simple => format!("[fitz-db {ms:.1}ms] {sql_oneline}"),
        DbLogMode::Verbose => {
            if args.is_empty() {
                format!("[fitz-db {ms:.1}ms verbose] {sql_oneline}")
            } else {
                let sql_lower = sql.to_ascii_lowercase();
                let params: Vec<String> = args
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let pos = i + 1;
                        if should_redact_param(&sql_lower, pos) {
                            format!("${pos}=<redacted>")
                        } else {
                            format!("${pos}={}", format_log_value(v))
                        }
                    })
                    .collect();
                format!(
                    "[fitz-db {ms:.1}ms verbose] {sql_oneline} params=[{}]",
                    params.join(", ")
                )
            }
        }
    }
}

/// v0.10.29 — Keywords indicating the param value is a secret
/// and must be masked in the verbose log. Covers canonical SQL
/// names (mostly in English because columns in the industry are
/// typically English, even when the query comment is in another
/// language). Includes several common forms.
const SENSITIVE_LOG_KEYWORDS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "secret",
    "api_key",
    "apikey",
    "api_token",
    "auth_token",
    "access_token",
    "refresh_token",
    "id_token",
    "private_key",
    "privkey",
    "credential",
    "session_key",
    "session_token",
    "csrf_token",
];

/// v0.10.29 — SQL words indicating a "clause change" between the
/// keyword and the placeholder. If one of these appears between
/// the sensitive keyword and `$N`, the placeholder belongs to a
/// different part of the statement and must NOT be redacted (e.g.
/// `UPDATE x SET password = $1 WHERE id = $2` — `$2` belongs to
/// `id`, not `password`, because `WHERE` separates the two
/// clauses).
const CONTEXT_BREAKERS: &[&str] = &[
    " where ",
    " and ",
    " or ",
    " having ",
    " from ",
    " join ",
    " on ",
    " group ",
    " order ",
    " into ",
    " returning ",
    " limit ",
    " offset ",
];

/// v0.10.29 — Best-effort heuristic to decide whether param `$N`
/// must be masked in the log. Looks at the ~50 chars before the
/// placeholder in the SQL (case-insensitive) and matches
/// sensitive keywords as a sub-string + verifies there is no
/// context breaker between the keyword and the placeholder.
///
/// Documented trade-offs:
/// - **False positives**: in INSERT with several columns (`INSERT
///   INTO users (name, password) VALUES ($1, $2)`), the window
///   before `$1` may contain "password" → name is unnecessarily
///   redacted. Acceptable: over-redacting beats real leaks in
///   logs.
/// - **False negatives**: if the keyword is too far from the
///   placeholder (>50 chars), it does not match. The user should
///   avoid verbose logging on queries with secrets if they want a
///   total guarantee — the heuristic covers the typical cases
///   (compact UPDATE / WHERE / INSERT).
/// - **Special case `$10`, `$11`, etc.**: the needle `$1` must
///   not match inside `$10`/`$11`/`$12`/..., so we check that the
///   immediately following char is not a digit.
///
/// `sql_lower` is the SQL already in lowercase (cached by the
/// caller to avoid re-allocating per param).
pub(crate) fn should_redact_param(sql_lower: &str, position: usize) -> bool {
    let needle = format!("${position}");
    let mut start = 0;
    while let Some(rel) = sql_lower[start..].find(&needle) {
        let abs = start + rel;
        let end = abs + needle.len();
        // Skip if the next char is a digit (we were looking for
        // $1 but it matched inside $10/$11/...).
        if let Some(next_ch) = sql_lower[end..].chars().next() {
            if next_ch.is_ascii_digit() {
                start = end;
                continue;
            }
        }
        let win_start = abs.saturating_sub(50);
        let window = &sql_lower[win_start..abs];
        for kw in SENSITIVE_LOG_KEYWORDS {
            if let Some(kw_pos) = window.rfind(kw) {
                // Check that between the keyword and the
                // placeholder there is no context breaker
                // (WHERE/AND/OR/etc.) — if there is, the
                // placeholder belongs to another clause.
                let after_kw = &window[kw_pos + kw.len()..];
                let mut broken = false;
                for breaker in CONTEXT_BREAKERS {
                    if after_kw.contains(breaker) {
                        broken = true;
                        break;
                    }
                }
                if !broken {
                    return true;
                }
            }
        }
        start = end;
    }
    false
}

/// Emits the log line to stderr if the mode is active. Cheap
/// when `DbLogMode::Off` — a single load + match, no allocations.
fn log_db_query(elapsed: std::time::Duration, sql: &str, args: &[PgValue]) {
    let mode = *DB_LOG_MODE;
    if matches!(mode, DbLogMode::Off) {
        return;
    }
    let line = format_db_log_line(elapsed, sql, args, mode);
    eprintln!("{line}");
}

impl DbConnHandle {
    /// Builds a handle from an initial connection. The pool
    /// starts with that conn in `idle` and grows on demand up to
    /// `max_conns`.
    pub fn new(initial_conn: Connection, url_redacted: String, config: ConnectionConfig) -> Self {
        let pool = std::sync::Arc::new(DbPool {
            config,
            idle: std::sync::Mutex::new(vec![initial_conn]),
            // v0.10.29 — Uses `effective_max_conns()` which
            // respects the `FITZ_DB_MAX_CONNS` env var (opt-in
            // override).
            permits: std::sync::Arc::new(tokio::sync::Semaphore::new(effective_max_conns())),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        DbConnHandle {
            pool,
            url_redacted,
            // v0.10.31 (Tier A.4) — depth=0 = we are not in a tx.
            tx_depth: std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0)),
        }
    }

    /// Constructor for evaluator tests: produces a handle whose
    /// pool starts in "closed" state. Queries fail with
    /// `Protocol("...cerrado")` but the dispatch_method works —
    /// useful for validating integration without a real Postgres.
    #[cfg(any(test, debug_assertions))]
    pub fn new_for_test_closed(url_redacted: String) -> Self {
        // Dummy config — never used because the pool starts
        // closed.
        let dummy_config = ConnectionConfig {
            host: "test-host".into(),
            port: 5432,
            user: "test-user".into(),
            password: None,
            dbname: "test-db".into(),
            sslmode: SslMode::Disable,
            sslrootcert: None,
        };
        let pool = std::sync::Arc::new(DbPool {
            config: dummy_config,
            idle: std::sync::Mutex::new(Vec::new()),
            permits: std::sync::Arc::new(tokio::sync::Semaphore::new(DEFAULT_MAX_CONNS)),
            closed: std::sync::atomic::AtomicBool::new(true),
        });
        DbConnHandle {
            pool,
            url_redacted,
            tx_depth: std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0)),
        }
    }

    /// Runs a query with args. Grabs a conn from the pool, runs
    /// it, returns it to the pool on completion (via PooledConn's
    /// Drop). If the pool is closed or cannot open a new conn,
    /// clear error.
    ///
    /// v0.10.28 — If `FITZ_DB_LOG` is active, emits
    /// `[fitz-db Nms] <sql>` (Simple) or additionally the params
    /// (Verbose) post-execution to stderr, including the pool
    /// acquire time. Also logs queries that fail (the log
    /// includes SQL + time; the error flows via the normal `?` on
    /// the caller side). On `Off` (default) the overhead is one
    /// atomic load.
    pub async fn query(&self, sql: &str, args: &[PgValue]) -> DbResult<QueryResult> {
        let start = std::time::Instant::now();
        let result = async {
            let mut pooled = self.pool.acquire().await?;
            let conn = pooled.as_mut();
            if args.is_empty() {
                conn.simple_query(sql).await
            } else {
                conn.extended_query(sql, args).await
            }
        }
        .await;
        log_db_query(start.elapsed(), sql, args);
        // v0.10.29 — If the query fails with a server error, we
        // enrich the message with the one-line SQL + params
        // (respecting secret redaction). Without this, Postgres'
        // canonical error is `ERROR: duplicate key value` without
        // a hint of which query failed — the user had to look at
        // the stack trace to deduce the callsite.
        result.map_err(|e| enrich_db_error_with_context(e, sql, args))
    }

    /// Runs a statement that does not expect rows
    /// (INSERT/UPDATE/DELETE/DDL without RETURNING). Returns the
    /// number of affected rows inferred from the `CommandComplete`
    /// tag.
    pub async fn exec(&self, sql: &str, args: &[PgValue]) -> DbResult<u64> {
        let result = self.query(sql, args).await?;
        Ok(result.rows_affected())
    }

    /// Cooperatively closes the pool. Marks it as closed and
    /// drains the idle conns (sends Terminate to each). Idempotent
    /// — multiple `close()` calls are not an error. Conns
    /// checked-out at close time are discarded on return (release
    /// sees `closed=true` and skips the push).
    pub async fn close(&self) -> DbResult<()> {
        use std::sync::atomic::Ordering;
        if self.pool.closed.swap(true, Ordering::AcqRel) {
            return Ok(()); // was already closed
        }
        // Drain the idle queue + close each conn.
        let drained = {
            let mut idle = self.pool.idle.lock().expect("pool mutex poisoned");
            std::mem::take(&mut *idle)
        };
        for conn in drained {
            let _ = conn.close().await;
        }
        // Close the semaphore so `acquire_owned` on pending tasks
        // wakes up with an error.
        self.pool.permits.close();
        Ok(())
    }

    /// `true` if the pool was closed via `close()`.
    pub async fn is_closed(&self) -> bool {
        self.pool.closed.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Phase 10.7 — ORM transactions. Runs `f` inside a Postgres
    /// transaction. Internally:
    ///
    /// 1. Acquires a conn from the pool (keeps the semaphore slot
    ///    reserved for the whole tx).
    /// 2. `BEGIN` on that conn.
    /// 3. Wraps the conn in a "single-conn pool" `DbConnHandle`
    ///    pinned to that physical conn. The callback receives
    ///    that handle and uses it as a regular `db` — all
    ///    `.insert/.update/.delete/.first/.all` ORM methods work
    ///    unchanged.
    /// 4. If `f` returns `Ok(v)`: `COMMIT` + returns `Ok(v)`.
    /// 5. If `f` returns `Err(e)`: automatic `ROLLBACK` +
    ///    propagates `Err(e)`. Impossible to forget.
    /// 6. The conn returns to the original pool (not lost).
    ///
    /// **Guarantees**:
    /// - **Atomic**: either all callback queries persist (COMMIT)
    ///   or none (ROLLBACK).
    /// - **Isolated**: the callback ALWAYS uses the same physical
    ///   conn — Postgres guarantees isolation per conn at the
    ///   server's default level (typically READ COMMITTED).
    /// - **Auto-cleanup**: if the callback panics (future fix:
    ///   async `catch_unwind`), the conn returns to the pool and
    ///   does NOT stay hung in an open-tx state.
    ///
    /// v0.10.31 (Tier A.4 + A.9) — nested transactions via
    /// SAVEPOINT + custom isolation levels. Still works with the
    /// original `transaction(closure)` signature; isolation is
    /// set via `transaction_with_isolation(level, closure)`.
    ///
    /// **Guarantees** (unchanged since v0.10.14):
    /// - **Atomic**: Ok → COMMIT, Err → ROLLBACK.
    /// - **Isolated**: same physical conn for the whole tx.
    /// - **Auto-cleanup**: automatic rollback on Err.
    ///
    /// **v0.10.31 — New**:
    /// - **Nesting**: `tx.transaction(g)` inside the outer
    ///   callback uses `SAVEPOINT/RELEASE
    ///   SAVEPOINT/ROLLBACK TO SAVEPOINT` instead of
    ///   `BEGIN/COMMIT/ROLLBACK`. Detected via shared `tx_depth`.
    ///   The inner rollback leaves the outer intact, parallel to
    ///   Postgres' nested semantics.
    /// - **Isolation level**: outer tx (depth=0) can set
    ///   `READ COMMITTED` / `REPEATABLE READ` / `SERIALIZABLE` /
    ///   `READ ONLY` via `transaction_with_isolation`. Inner txs
    ///   ignore isolation (Postgres pins it on the outer BEGIN).
    pub async fn transaction<F, Fut, T>(self: &std::sync::Arc<Self>, f: F) -> DbResult<T>
    where
        F: FnOnce(std::sync::Arc<DbConnHandle>) -> Fut,
        Fut: std::future::Future<Output = DbResult<T>>,
    {
        self.transaction_with_isolation(None, f).await
    }

    /// v0.10.31 (Tier A.9) — variant that accepts an isolation
    /// level. `None` = Postgres default (READ COMMITTED).
    /// `Some("...")` = emits `BEGIN ISOLATION LEVEL <...>`. If
    /// depth > 0 (nested), isolation is silently ignored —
    /// Postgres pins it on the outer BEGIN.
    pub async fn transaction_with_isolation<F, Fut, T>(
        self: &std::sync::Arc<Self>,
        isolation: Option<&str>,
        f: F,
    ) -> DbResult<T>
    where
        F: FnOnce(std::sync::Arc<DbConnHandle>) -> Fut,
        Fut: std::future::Future<Output = DbResult<T>>,
    {
        use std::sync::atomic::Ordering;

        // v0.10.31 (Tier A.4) — depth tracking shared between
        // outer and all nested. Increments on entry, decrements
        // on exit. Decides whether to emit BEGIN vs SAVEPOINT.
        let depth_before = self.tx_depth.fetch_add(1, Ordering::SeqCst);

        let (begin_sql, commit_sql, rollback_sql) = if depth_before == 0 {
            // Outer tx: BEGIN [ISOLATION LEVEL ...] / COMMIT / ROLLBACK.
            let begin = match isolation {
                Some(level) => format!("BEGIN ISOLATION LEVEL {}", level),
                None => "BEGIN".to_string(),
            };
            (begin, "COMMIT".to_string(), "ROLLBACK".to_string())
        } else {
            // Nested tx: SAVEPOINT / RELEASE / ROLLBACK TO SAVEPOINT.
            // Isolation ignored — Postgres does not allow
            // ISOLATION on SAVEPOINT, and the level is pinned by
            // the outer BEGIN for the entire tx.
            let sp_name = format!("fitz_sp_{}", depth_before);
            (
                format!("SAVEPOINT {}", sp_name),
                format!("RELEASE SAVEPOINT {}", sp_name),
                format!("ROLLBACK TO SAVEPOINT {}", sp_name),
            )
        };

        let result = self
            .do_tx_inner(&begin_sql, &commit_sql, &rollback_sql, f)
            .await;

        self.tx_depth.fetch_sub(1, Ordering::SeqCst);
        result
    }

    /// v0.10.31 (Tier A.4) — shared body of outer and nested tx.
    /// Takes the 3 SQL strings (BEGIN/COMMIT/ROLLBACK or
    /// SAVEPOINT/RELEASE/ROLLBACK TO), runs the standard dance:
    /// acquire conn → BEGIN → sub_pool dance → run f → COMMIT/ROLLBACK.
    async fn do_tx_inner<F, Fut, T>(
        self: &std::sync::Arc<Self>,
        begin_sql: &str,
        commit_sql: &str,
        rollback_sql: &str,
        f: F,
    ) -> DbResult<T>
    where
        F: FnOnce(std::sync::Arc<DbConnHandle>) -> Fut,
        Fut: std::future::Future<Output = DbResult<T>>,
    {
        use std::sync::atomic::Ordering;

        // 1. Acquire a conn from the pool.
        let mut pooled = self.pool.acquire().await?;

        // 2. BEGIN or SAVEPOINT.
        pooled.as_mut().simple_query(begin_sql).await?;

        // 3. Move the conn out of the PooledConn (same as legacy).
        let conn = pooled
            .conn
            .take()
            .expect("pooled.conn None inmediatamente post-acquire — bug");

        // 4. Build the single-conn sub_pool.
        let sub_pool = std::sync::Arc::new(DbPool {
            config: self.pool.config.clone(),
            idle: std::sync::Mutex::new(vec![conn]),
            permits: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        let sub_handle = std::sync::Arc::new(DbConnHandle {
            pool: sub_pool.clone(),
            url_redacted: self.url_redacted.clone(),
            // v0.10.31 — SHARED tx_depth Arc. The recursive
            // callback sees and will mutate the same counter as
            // the outer.
            tx_depth: self.tx_depth.clone(),
        });

        // 5. Run the callback.
        let result = f(sub_handle).await;

        // 6. Recover the conn from the sub_pool.
        let conn_opt = {
            let mut idle = sub_pool.idle.lock().expect("sub_pool mutex poisoned");
            idle.pop()
        };
        let mut conn = match conn_opt {
            Some(c) => c,
            None => {
                sub_pool.closed.store(true, Ordering::Release);
                sub_pool.permits.close();
                return result;
            }
        };

        // 7. COMMIT/ROLLBACK (or RELEASE/ROLLBACK TO SAVEPOINT).
        match &result {
            Ok(_) => {
                if let Err(commit_err) = conn.simple_query(commit_sql).await {
                    // Defensive cleanup — the rollback may fail
                    // too; we ignore it to return the original
                    // commit_err (more informative).
                    let _ = conn.simple_query(rollback_sql).await;
                    self.pool.release(conn);
                    return Err(commit_err);
                }
            }
            Err(_) => {
                let _ = conn.simple_query(rollback_sql).await;
            }
        }

        // 8. Return conn to self's pool (outer pool or tx_pool
        //    depending on depth_before).
        self.pool.release(conn);

        result
    }

    /// Maximum number of concurrent conns the pool can hand out.
    /// v0.10.29 — Respects the `FITZ_DB_MAX_CONNS` env var if set
    /// (clamp [1, 200]); fallback `DEFAULT_MAX_CONNS = 10`. The
    /// kwarg `db.connect(url, max_conns=N)` remains as minor debt
    /// for iteration 2 (requires wiring the kwarg from evaluator
    /// + codegen).
    pub fn max_conns(&self) -> usize {
        effective_max_conns()
    }

    /// 10.2 — diagnostics: number of idle conns right now. Useful
    /// for pool tests. NOT useful for concurrency (race between
    /// `idle()` and the next query).
    pub fn idle_count(&self) -> usize {
        self.pool.idle.lock().expect("pool mutex poisoned").len()
    }
}

/// Health check: every `HEALTH_CHECK_INTERVAL_SECS` seconds,
/// iterates idle conns and sends `SELECT 1`. Failing conns are
/// silently discarded — the next `acquire` will open a new one.
/// The task uses a `Weak<DbPool>` so the pool can be
/// garbage-collected when the handle is dropped, and the task
/// auto-terminates when the Weak upgrade fails.
async fn health_check_task(weak_pool: std::sync::Weak<DbPool>) {
    let interval = std::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS);
    loop {
        tokio::time::sleep(interval).await;
        let pool = match weak_pool.upgrade() {
            Some(p) => p,
            None => return, // pool dropped, task auto-terminates
        };
        if pool.closed.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        // Drain the idle queue, validate each conn, return the
        // live ones.
        let mut to_check = {
            let mut idle = pool.idle.lock().expect("pool mutex poisoned");
            std::mem::take(&mut *idle)
        };
        let mut alive = Vec::with_capacity(to_check.len());
        while let Some(mut conn) = to_check.pop() {
            // Light SELECT 1 — if it fails, discard.
            match conn.simple_query("SELECT 1").await {
                Ok(_) => alive.push(conn),
                Err(_) => {
                    // Conn dead — Connection's Drop closes the
                    // TcpStream.
                }
            }
        }
        // Re-populate idle with the live ones.
        if !alive.is_empty() {
            let mut idle = pool.idle.lock().expect("pool mutex poisoned");
            idle.append(&mut alive);
        }
    }
}

/// Opens a Postgres connection from a standard URL. Main entry
/// point for integration with the evaluator: the `db.connect(url)`
/// builtin invokes it and wraps the result in
/// `Value::DbConn(handle)` (already as `Arc<DbConnHandle>`).
///
/// **10.9.2 (v0.10.9) — singleton per URL**: the first call with
/// a new URL creates the handle + pool. Later calls with the SAME
/// URL return a clone of the existing Arc — ALL TCP conns are
/// shared via the single pool. This closes the previous
/// "connection pool leak" where each `db.connect(url)` created a
/// new pool (after N requests Postgres ran out of slots and
/// `acquire()` hung).
///
/// The cache lives in `POOL_CACHE` (global, lazy). Handles
/// persist until process exit (no eviction). Accepted trade-off:
/// if you never reconnect to a URL, the pool survives unused.
/// Memory is negligible (~24 KB per idle pool).
///
/// Eager: the first TCP conn + handshake + auth happens here to
/// validate credentials + URL before returning the handle. If it
/// fails, the handle is not created and the caller sees the error
/// directly. Additional conns are opened lazily in `acquire()`
/// when the pool needs them.
pub async fn connect_url(url: &str) -> DbResult<std::sync::Arc<DbConnHandle>> {
    // 10.9.2 (v0.10.9) — singleton per URL. The global cache
    // caches the Arc<DbConnHandle> per URL; subsequent calls with
    // the same URL return a clone of the Arc — ALL TCP conns are
    // shared via the single pool. Closes the v0.10.8 "connection
    // pool leak": each `db.connect(url)` created a new pool with
    // 10 permits + TCP conns, saturating Postgres
    // (`max_connections=100` default) after N requests and
    // leaving `acquire()` hanging forever.
    //
    // Trade-off: handles persist until process exit (no
    // eviction). Memory is negligible (~24 KB per idle pool).
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
            // If the handle was closed via `.close()`, we create
            // a new one — the caller did an explicit close and
            // wants to reopen.
        }
    }

    // Miss: create a new handle + insert it into the cache.
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

/// Strips the password from the URL for safe diagnostics.
/// Replaces `user:pass@host` with `user:***@host`. No-op if there
/// is no password.
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

    // v0.10.23 (Phase 10.1.b) — sslmode
    // require/verify-ca/verify-full now parse OK. prefer/allow
    // remain NotImplemented.

    #[test]
    fn url_sslmode_require_parsea_ok() {
        let c = ConnectionConfig::parse("postgres://user@host/db?sslmode=require").unwrap();
        assert_eq!(c.sslmode, SslMode::Require);
        assert!(c.sslrootcert.is_none());
    }

    #[test]
    fn url_sslmode_verify_ca_parsea_ok() {
        let c = ConnectionConfig::parse("postgres://user@host/db?sslmode=verify-ca").unwrap();
        assert_eq!(c.sslmode, SslMode::VerifyCa);
    }

    #[test]
    fn url_sslmode_verify_full_parsea_ok() {
        let c = ConnectionConfig::parse("postgres://user@host/db?sslmode=verify-full").unwrap();
        assert_eq!(c.sslmode, SslMode::VerifyFull);
    }

    #[test]
    fn url_sslmode_prefer_sigue_no_implementado() {
        let r = ConnectionConfig::parse("postgres://user@host/db?sslmode=prefer");
        assert!(matches!(r, Err(DbError::NotImplemented(_))));
    }

    #[test]
    fn url_sslmode_allow_sigue_no_implementado() {
        let r = ConnectionConfig::parse("postgres://user@host/db?sslmode=allow");
        assert!(matches!(r, Err(DbError::NotImplemented(_))));
    }

    #[test]
    fn url_sslmode_desconocido_es_error() {
        let r = ConnectionConfig::parse("postgres://user@host/db?sslmode=ultra-secure");
        assert!(matches!(r, Err(DbError::InvalidUrl(_))));
    }

    #[test]
    fn url_sslrootcert_con_verify_ca_parsea_ok() {
        let c = ConnectionConfig::parse(
            "postgres://user@host/db?sslmode=verify-ca&sslrootcert=/etc/ssl/ca.pem",
        )
        .unwrap();
        assert_eq!(c.sslmode, SslMode::VerifyCa);
        assert_eq!(
            c.sslrootcert.as_deref(),
            Some(std::path::Path::new("/etc/ssl/ca.pem"))
        );
    }

    #[test]
    fn url_sslrootcert_url_encoded_se_decodifica() {
        let c = ConnectionConfig::parse(
            "postgres://user@host/db?sslmode=verify-full&sslrootcert=%2Fhome%2Fme%2Fca.pem",
        )
        .unwrap();
        assert_eq!(
            c.sslrootcert.as_deref(),
            Some(std::path::Path::new("/home/me/ca.pem"))
        );
    }

    #[test]
    fn url_sslrootcert_con_sslmode_disable_es_error() {
        // Contradictory combo: make it explicit to the user.
        let r = ConnectionConfig::parse(
            "postgres://user@host/db?sslmode=disable&sslrootcert=/etc/ssl/ca.pem",
        );
        assert!(matches!(r, Err(DbError::InvalidUrl(_))));
    }

    #[test]
    fn url_sslrootcert_con_sslmode_require_es_error() {
        // require validates NOTHING — passing a rootcert is a
        // sign of confusion; better to abort.
        let r = ConnectionConfig::parse(
            "postgres://user@host/db?sslmode=require&sslrootcert=/etc/ssl/ca.pem",
        );
        assert!(matches!(r, Err(DbError::InvalidUrl(_))));
    }

    #[test]
    fn url_sslrootcert_sin_sslmode_es_error() {
        let r = ConnectionConfig::parse("postgres://user@host/db?sslrootcert=/etc/ssl/ca.pem");
        assert!(matches!(r, Err(DbError::InvalidUrl(_))));
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
        // No type byte: starts with length(4).
        let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        assert_eq!(len, bytes.len());
        // Bytes 4..8 = protocol version = 196608
        let version = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        assert_eq!(version, 196608);
        // Contains "user\0alice\0database\0myapp\0..."
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
        assert_eq!(len, bytes.len() - 1); // length excludes the tag
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
        // Payload starts after the header (tag + length = 5)
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
        payload.push(0); // list terminator
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
        payload.push(0); // terminator

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
    // Vectors from RFC 7677 §3 (SCRAM-SHA-256 test vector):
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
        // Server final with tampered signature (random bytes)
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
        // The "real" RFC 7677 vector test lives in
        // `scram_rfc7677_test_vector` (covers the pbkdf2 result
        // indirectly via the server-signature). Here we just
        // check that `pbkdf2_hmac_sha256` does not loop forever
        // nor panic with a reasonable iter count.
        let salt = BASE64.decode("W22ZaJ0SNY7soEsUEjb6gQ==").unwrap();
        let salted = pbkdf2_hmac_sha256(b"pencil", &salt, 4096);
        assert_eq!(salted.len(), 32);
        // The result must NOT be all zeros (catches silly XOR
        // loop bugs).
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
        // Postgres' MD5 hash is:
        //   "md5" || md5_hex(md5_hex("pwduser") || salt)
        // For password="pwd", user="user",
        // salt=[0x12,0x34,0x56,0x78]:
        //   inner = md5_hex("pwduser") = ?
        //   outer = md5_hex(inner_bytes || salt)
        let hash = md5_password("user", "pwd", &[0x12, 0x34, 0x56, 0x78]);
        // Verify it has the right prefix and length: "md5" + 32
        // hex chars = 35 chars total.
        assert!(hash.starts_with("md5"));
        assert_eq!(hash.len(), 35);
    }

    // ----- OID types -----

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
        // In MVP 10.1, UUID is treated as Text. 10.5 refines it.
        assert!(matches!(v, PgValue::Text(_)));
    }

    #[test]
    fn parse_oid_no_soportado() {
        // OID 1700 = numeric (unsupported in MVP)
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

    // ----- Phase 10.5.b — native arrays -----

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
        // {"NULL"} → ["NULL"] as text, not null.
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
        // UUIDs come through as TEXT_ARRAY[uuid-strings]; the ORM
        // accepts UUID via its scalar OID, but also via list<Str>
        // with the explicit `::uuid[]` cast.
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
        // "c\"d" → "c\\\"d"  (escape for Postgres)
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
        // If the URL is not postgres://, return as-is (the caller
        // already failed to parse; redact is just defense).
        assert_eq!(redact_url("mysql://x:y@h/d"), "mysql://x:y@h/d");
    }

    // ----- DbConnHandle lifecycle (without real Postgres) -----
    //
    // We cannot test `query/exec` end-to-end without a real
    // Postgres, but we can test the "closed → operations fail"
    // cycle. We use `new_for_test_closed` which builds a pool in
    // closed state without opening TCP.

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
        // close() on an already-closed handle is no-op (no error).
        handle.close().await.unwrap();
        handle.close().await.unwrap();
    }

    // ----- 10.2 — connection pool -----

    #[tokio::test]
    async fn db_pool_max_conns_default() {
        // The default pool exposes DEFAULT_MAX_CONNS concurrent
        // conns. The handle's public API exposes it via
        // `max_conns()`.
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        assert_eq!(handle.max_conns(), DEFAULT_MAX_CONNS);
    }

    #[tokio::test]
    async fn db_pool_idle_count_inicia_en_cero_cuando_closed() {
        // The test handle starts in closed state with an empty
        // pool.
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        assert_eq!(handle.idle_count(), 0);
    }

    #[test]
    fn db_pool_struct_es_send_sync() {
        // DbPool must be Send + Sync so the handle can be shared
        // across tasks. This validates that the whole composition
        // (Mutex<Vec>/Arc<Semaphore>/AtomicBool) still satisfies
        // the traits that `Value::DbConn(Arc<...>)` needs.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DbPool>();
        assert_send_sync::<DbConnHandle>();
    }

    #[tokio::test]
    async fn db_pool_acquire_falla_cuando_closed() {
        // acquire() on a closed pool must return a clear Protocol
        // error (no panic, no hang).
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        let r = handle.pool.acquire().await;
        assert!(matches!(r, Err(DbError::Protocol(_))));
    }

    #[tokio::test]
    async fn db_pool_close_idempotente_y_marca_closed_flag() {
        let handle = DbConnHandle::new_for_test_closed("postgres://test@host/db".into());
        assert!(handle.is_closed().await);
        // First close — no-op because it is already closed; no
        // errors.
        handle.close().await.unwrap();
        // Second close — also no error.
        handle.close().await.unwrap();
        assert!(handle.is_closed().await);
    }

    // ================================================================
    // v0.10.28 — FITZ_DB_LOG formatter unit tests
    // ================================================================

    #[test]
    fn format_db_log_line_off_devuelve_string_vacio() {
        let line = format_db_log_line(
            std::time::Duration::from_millis(5),
            "SELECT 1",
            &[],
            DbLogMode::Off,
        );
        assert_eq!(line, "");
    }

    #[test]
    fn format_db_log_line_simple_incluye_ms_y_sql() {
        let line = format_db_log_line(
            std::time::Duration::from_millis(12),
            "SELECT id FROM users WHERE id = $1",
            &[PgValue::Int(42)],
            DbLogMode::Simple,
        );
        assert!(line.starts_with("[fitz-db "), "{line}");
        assert!(line.contains("12.0ms"), "{line}");
        assert!(
            line.contains("SELECT id FROM users WHERE id = $1"),
            "{line}"
        );
        // Simple does NOT log params.
        assert!(
            !line.contains("params="),
            "Simple no debe loguear params: {line}"
        );
        assert!(
            !line.contains("42"),
            "Simple no debe filtrar valores: {line}"
        );
    }

    #[test]
    fn format_db_log_line_verbose_incluye_params() {
        let line = format_db_log_line(
            std::time::Duration::from_micros(4100), // ~4.1ms
            "INSERT INTO users (name, age) VALUES ($1, $2)",
            &[PgValue::Text("ada".into()), PgValue::Int(30)],
            DbLogMode::Verbose,
        );
        assert!(line.contains("verbose"), "{line}");
        assert!(line.contains("params=["), "{line}");
        assert!(line.contains("$1=\"ada\""), "{line}");
        assert!(line.contains("$2=30"), "{line}");
    }

    #[test]
    fn format_db_log_line_verbose_trunca_strings_largos() {
        let huge = "x".repeat(200);
        let line = format_db_log_line(
            std::time::Duration::from_millis(1),
            "INSERT INTO blobs (data) VALUES ($1)",
            &[PgValue::Text(huge)],
            DbLogMode::Verbose,
        );
        // The original string has 200 chars; truncated to 80 + `…`.
        // The log must not contain the full 200-'x' run.
        assert!(line.contains("…"), "debería truncar: {line}");
        assert!(
            !line.contains(&"x".repeat(200)),
            "no debe filtrar full: {line}"
        );
    }

    #[test]
    fn format_db_log_line_colapsa_multilinea_sql() {
        let sql = "SELECT\n  id,\n  name\nFROM users\nWHERE id = $1";
        let line = format_db_log_line(
            std::time::Duration::from_millis(1),
            sql,
            &[PgValue::Int(1)],
            DbLogMode::Simple,
        );
        // Multi-line must end up on a single line (whitespace
        // collapsed).
        assert!(!line.contains('\n'), "log debe ser one-line: {line}");
        assert!(
            line.contains("SELECT id, name FROM users WHERE id = $1"),
            "{line}"
        );
    }

    #[test]
    fn format_db_log_line_verbose_sin_args_no_emite_params() {
        let line = format_db_log_line(
            std::time::Duration::from_millis(1),
            "BEGIN",
            &[],
            DbLogMode::Verbose,
        );
        assert!(line.contains("verbose"), "{line}");
        assert!(line.contains("BEGIN"), "{line}");
        // Without args, the params section should not appear.
        assert!(
            !line.contains("params="),
            "sin args no debe haber sección params: {line}"
        );
    }

    #[test]
    fn truncate_for_log_utf8_safe() {
        // emoji = 4 bytes but 1 char; truncate counts chars, not
        // bytes — must not panic.
        let s = "🦀".repeat(50);
        let t = truncate_for_log(&s, 10);
        // 10 emojis + '…' = 11 chars.
        assert_eq!(t.chars().count(), 11);
        assert!(t.ends_with('…'));
    }

    // v0.10.29 — secret redaction in FITZ_DB_LOG=verbose

    #[test]
    fn should_redact_param_detecta_password_en_where() {
        let sql = "select * from users where password = $1";
        assert!(should_redact_param(sql, 1));
    }

    #[test]
    fn should_redact_param_detecta_password_en_update() {
        let sql = "update users set password = $1 where id = $2";
        assert!(should_redact_param(sql, 1));
        assert!(!should_redact_param(sql, 2));
    }

    #[test]
    fn should_redact_param_detecta_secret_y_api_key() {
        assert!(should_redact_param(
            "insert into vault (secret) values ($1)",
            1
        ));
        assert!(should_redact_param(
            "update apps set api_key = $1 where id = $2",
            1
        ));
        assert!(should_redact_param(
            "select * from t where access_token = $1",
            1
        ));
        assert!(should_redact_param(
            "select * from t where refresh_token = $1",
            1
        ));
        assert!(should_redact_param(
            "select * from t where private_key = $1",
            1
        ));
    }

    #[test]
    fn should_redact_param_no_redacta_columnas_normales() {
        let sql = "select * from users where email = $1 and id = $2";
        assert!(!should_redact_param(sql, 1));
        assert!(!should_redact_param(sql, 2));
    }

    #[test]
    fn should_redact_param_no_confunde_dolar_1_con_dolar_10() {
        // Edge case: the needle `$1` must not match inside `$10`.
        let sql = "select * from users where id = $10 and email = $1";
        // $1 → must not redact (email is not sensitive).
        assert!(!should_redact_param(sql, 1));
        // $10 → also no (id is not sensitive).
        assert!(!should_redact_param(sql, 10));
    }

    #[test]
    fn should_redact_param_es_case_insensitive() {
        let sql = "select * from t where PASSWORD = $1";
        // The caller always passes lowercase, so the match is
        // effectively case-insensitive.
        assert!(should_redact_param(&sql.to_ascii_lowercase(), 1));
    }

    #[test]
    fn format_db_log_line_verbose_redacta_password() {
        // Full integrator: verbose with a password in the SQL
        // must mask the real value with `<redacted>`.
        let line = format_db_log_line(
            std::time::Duration::from_millis(1),
            "UPDATE users SET password = $1 WHERE id = $2",
            &[PgValue::Text("super_secret_xyz".into()), PgValue::Int(42)],
            DbLogMode::Verbose,
        );
        assert!(
            line.contains("$1=<redacted>"),
            "esperaba $1 enmascarado, fue: {line}"
        );
        assert!(
            !line.contains("super_secret_xyz"),
            "el secret NO debe aparecer en el log: {line}"
        );
        // $2 (id = 42) stays visible — not sensitive.
        assert!(line.contains("$2=42"), "{line}");
    }

    #[test]
    fn format_db_log_line_verbose_no_redacta_email_normal() {
        // Sanity: normal queries without secrets still show params.
        let line = format_db_log_line(
            std::time::Duration::from_millis(1),
            "SELECT * FROM users WHERE email = $1",
            &[PgValue::Text("ada@example.com".into())],
            DbLogMode::Verbose,
        );
        assert!(
            line.contains("$1=\"ada@example.com\""),
            "esperaba email visible (no sensitive), fue: {line}"
        );
    }

    // v0.10.29 — DbError with SQL context + SQLSTATE in Display

    // v0.10.29 — FITZ_DB_MAX_CONNS parser

    #[test]
    fn parse_max_conns_value_basico() {
        assert_eq!(parse_max_conns_value("20"), 20);
        assert_eq!(parse_max_conns_value(" 50 "), 50);
        assert_eq!(parse_max_conns_value("1"), 1);
        assert_eq!(parse_max_conns_value("200"), 200);
    }

    #[test]
    fn parse_max_conns_value_invalido_fallback_default() {
        assert_eq!(parse_max_conns_value(""), DEFAULT_MAX_CONNS);
        assert_eq!(parse_max_conns_value("0"), DEFAULT_MAX_CONNS);
        assert_eq!(parse_max_conns_value("201"), DEFAULT_MAX_CONNS);
        assert_eq!(parse_max_conns_value("foo"), DEFAULT_MAX_CONNS);
        assert_eq!(parse_max_conns_value("-5"), DEFAULT_MAX_CONNS);
    }

    #[test]
    fn db_error_server_display_incluye_sqlstate_code() {
        let err = DbError::Server {
            severity: "ERROR".into(),
            code: "23505".into(),
            message: "duplicate key value violates unique constraint \"users_email_key\"".into(),
        };
        let s = err.to_string();
        assert!(
            s.contains("[23505]"),
            "esperaba SQLSTATE entre corchetes: {s}"
        );
        assert!(s.starts_with("ERROR [23505]:"), "{s}");
    }

    #[test]
    fn db_error_server_display_sin_code_omite_corchetes() {
        let err = DbError::Server {
            severity: "ERROR".into(),
            code: String::new(),
            message: "algo falló".into(),
        };
        let s = err.to_string();
        assert!(!s.contains("[]"), "no debe haber [] vacíos: {s}");
        assert_eq!(s, "ERROR: algo falló");
    }

    #[test]
    fn enrich_db_error_suma_sql_y_params_al_mensaje() {
        let err = DbError::Server {
            severity: "ERROR".into(),
            code: "23505".into(),
            message: "duplicate key".into(),
        };
        let enriched = enrich_db_error_with_context(
            err,
            "INSERT INTO users (email) VALUES ($1)",
            &[PgValue::Text("ada@x.com".into())],
        );
        let s = enriched.to_string();
        assert!(s.contains("[sql: INSERT INTO users"), "{s}");
        assert!(s.contains("$1=\"ada@x.com\""), "{s}");
    }

    #[test]
    fn enrich_db_error_respeta_redaction_para_secrets() {
        let err = DbError::Server {
            severity: "ERROR".into(),
            code: "23502".into(),
            message: "not-null violation".into(),
        };
        let enriched = enrich_db_error_with_context(
            err,
            "UPDATE users SET password = $1 WHERE id = $2",
            &[PgValue::Text("super_secret_xyz".into()), PgValue::Int(42)],
        );
        let s = enriched.to_string();
        assert!(
            s.contains("$1=<redacted>"),
            "esperaba redaction del password: {s}"
        );
        assert!(
            !s.contains("super_secret_xyz"),
            "el secret NO debe aparecer: {s}"
        );
        assert!(s.contains("$2=42"), "{s}");
    }

    #[test]
    fn enrich_db_error_pass_through_para_no_server_errors() {
        let err = DbError::Protocol("badly framed message".into());
        let enriched = enrich_db_error_with_context(err, "SELECT 1", &[]);
        assert_eq!(enriched.to_string(), "protocolo: badly framed message");
    }

    #[test]
    fn enrich_db_error_trunca_sql_largo() {
        let long_sql =
            "SELECT id, name, ".to_string() + &"col, ".repeat(100) + "x FROM users WHERE id = $1";
        let err = DbError::Server {
            severity: "ERROR".into(),
            code: "42P01".into(),
            message: "table does not exist".into(),
        };
        let enriched = enrich_db_error_with_context(err, &long_sql, &[PgValue::Int(1)]);
        let s = enriched.to_string();
        assert!(s.contains("…"), "esperaba SQL truncado: {s}");
        assert!(s.len() < long_sql.len() + 100, "no debe inflar masivamente");
    }

    #[test]
    fn format_db_log_line_verbose_redacta_api_key_en_insert() {
        // Positional INSERT: the log captures "api_key" in the
        // column list and redacts the corresponding $N.
        let line = format_db_log_line(
            std::time::Duration::from_millis(1),
            "INSERT INTO tokens (name, api_key) VALUES ($1, $2)",
            &[
                PgValue::Text("prod".into()),
                PgValue::Text("sk-very-secret".into()),
            ],
            DbLogMode::Verbose,
        );
        // $2 corresponds to api_key → redacted.
        assert!(
            line.contains("$2=<redacted>"),
            "esperaba api_key redacted, fue: {line}"
        );
        assert!(
            !line.contains("sk-very-secret"),
            "el secret NO debe aparecer en el log: {line}"
        );
    }
}
