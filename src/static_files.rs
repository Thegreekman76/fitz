//! FITZ-02 (2026-08) — Shared pure logic for static file serving.
//!
//! `@server(static_dir=..., static_prefix=...)` mounts a route that serves
//! files from a directory (or, with `fitz build --embed-static`, from bytes
//! baked into the binary). Both the interpreter runtime (`src/http.rs`) and
//! the code generator (`src/codegen.rs`, via the `STATIC_PRELUDE` string it
//! emits) must produce **byte-for-byte identical** responses so that
//! `fitz run` and `fitz build` behave the same.
//!
//! To guarantee that, the algorithms live here as ordinary functions that
//! `http.rs` calls directly, and are **mirrored literally** as `__fitz_static_*`
//! helpers inside the codegen prelude. Any drift is caught by the run↔build
//! parity E2E test.

/// Maps a file path's extension (case-insensitive) to a MIME type. The table
/// is deliberately small and web-focused; unknown extensions fall back to
/// `application/octet-stream` (the safe default the browser treats as a
/// download). This same table is mirrored in the codegen prelude.
pub fn content_type_for(path: &str) -> &'static str {
    // Extract the extension after the last `.` in the last path component.
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = match name.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() => e.to_ascii_lowercase(),
        _ => return "application/octet-stream",
    };
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "csv" => "text/csv; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Content-based strong ETag: FNV-1a 64-bit over the bytes, formatted as a
/// quoted lowercase hex string (e.g. `"9d3f...\""`). Content-based (not
/// mtime-based) so the same bytes always produce the same ETag — this is
/// what makes `fitz run` and `fitz build` return identical ETags for the
/// same asset. Mirrored in the codegen prelude.
pub fn compute_etag(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("\"{:016x}\"", h)
}

/// Rejects a relative request path that could escape the static directory or
/// is otherwise unsafe. Returns `true` only for a clean relative path made of
/// non-empty components, none of which is `.` or `..`, none absolute, and
/// none containing a NUL byte. Belt-and-suspenders alongside the
/// canonicalize+containment check the disk handler also performs (and the
/// only guard the embed handler needs, since it keys a fixed asset map).
/// Mirrored in the codegen prelude.
pub fn is_safe_relative(rel: &str) -> bool {
    if rel.is_empty() || rel.starts_with('/') || rel.starts_with('\\') {
        return false;
    }
    for comp in rel.split(['/', '\\']) {
        if comp.is_empty() || comp == "." || comp == ".." || comp.contains('\0') {
            return false;
        }
    }
    true
}

/// Formats seconds-since-Unix-epoch as an HTTP-date (RFC 7231 IMF-fixdate),
/// e.g. `Wed, 21 Oct 2015 07:28:00 GMT`. Self-contained (no chrono) so the
/// codegen prelude can mirror it without pulling a date crate into the
/// generated `Cargo.toml`. Uses Howard Hinnant's days-from-civil algorithm.
/// Mirrored in the codegen prelude.
pub fn http_date(secs: u64) -> String {
    const WDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (hh, mi, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // 1970-01-01 was a Thursday (index 4 in WDAYS with Sun=0).
    let wd = (((days % 7) + 4) % 7 + 7) % 7;
    // civil-from-days (era-based), Hinnant.
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        WDAYS[wd as usize],
        d,
        MONTHS[(m - 1) as usize],
        y,
        hh,
        mi,
        ss
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_common_web_assets() {
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type_for("css/app.css"), "text/css; charset=utf-8");
        assert_eq!(content_type_for("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(
            content_type_for("manifest.webmanifest"),
            "application/manifest+json"
        );
        assert_eq!(content_type_for("logo.svg"), "image/svg+xml");
        assert_eq!(content_type_for("favicon.ico"), "image/x-icon");
        assert_eq!(content_type_for("bundle.wasm"), "application/wasm");
        assert_eq!(content_type_for("beep.mp3"), "audio/mpeg");
    }

    #[test]
    fn content_type_is_case_insensitive() {
        assert_eq!(content_type_for("PHOTO.JPG"), "image/jpeg");
        assert_eq!(content_type_for("Style.CSS"), "text/css; charset=utf-8");
    }

    #[test]
    fn content_type_unknown_is_octet_stream() {
        assert_eq!(content_type_for("data.bin"), "application/octet-stream");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
        assert_eq!(content_type_for("archive."), "application/octet-stream");
    }

    #[test]
    fn etag_is_deterministic_and_content_based() {
        let a = compute_etag(b"hello");
        let b = compute_etag(b"hello");
        let c = compute_etag(b"world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with('"') && a.ends_with('"'));
        // FNV-1a 64-bit of "hello" is a known constant.
        assert_eq!(a, "\"a430d84680aabd0b\"");
    }

    #[test]
    fn etag_of_empty_is_the_fnv_offset_basis() {
        assert_eq!(compute_etag(b""), "\"cbf29ce484222325\"");
    }

    #[test]
    fn safe_relative_accepts_clean_paths() {
        assert!(is_safe_relative("index.html"));
        assert!(is_safe_relative("css/app.css"));
        assert!(is_safe_relative("assets/img/logo.png"));
    }

    #[test]
    fn safe_relative_rejects_traversal_and_absolute() {
        assert!(!is_safe_relative("../etc/passwd"));
        assert!(!is_safe_relative("a/../../b"));
        assert!(!is_safe_relative("/etc/passwd"));
        assert!(!is_safe_relative("\\windows\\system32"));
        assert!(!is_safe_relative("a/./b"));
        assert!(!is_safe_relative(""));
        assert!(!is_safe_relative("a//b"));
        assert!(!is_safe_relative("a\0b"));
    }

    #[test]
    fn http_date_known_value() {
        // 1445412480 = Wed, 21 Oct 2015 07:28:00 GMT
        assert_eq!(http_date(1445412480), "Wed, 21 Oct 2015 07:28:00 GMT");
        // 0 = Thu, 01 Jan 1970 00:00:00 GMT (Unix epoch).
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    }
}
