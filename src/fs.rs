// fs.rs — FITZ-03 (2026-08): the `fs` builtin module (filesystem access).
//
// General-purpose filesystem for a backend language that compiles to a native
// binary. All operations run at RUNTIME (not compile time): `fs.read(path)` at
// boot reads from the binary's working dir. Everything returns `Result` (except
// `exists`), consistent with the rest of the language. Paths are relative to the
// process working directory.
//
// MVP: no sandbox (expected for a backend lang). A Deno-style permission layer
// (`--allow-read=locales/`) is a future opt-in. Streaming of large files
// (`open()` with seek) is a follow-up; the 8 builtins here cover ~95% of cases.

use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::value::{shared, ResultVariant, Value};

fn arg_err(msg: impl Into<String>) -> FitzError {
    FitzError::new(ErrorKind::InvalidSyntax, 0, 0, msg.into())
}

fn expect_str(v: &Value, ctx: &str) -> FitzResult<String> {
    match v {
        Value::Str(s) => Ok(s.clone()),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Str".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!(
                "{}: expects a Str path, received `{}`",
                ctx,
                other.type_name()
            ),
        )),
    }
}

fn ok(v: Value) -> Value {
    Value::Result(ResultVariant::Ok(Box::new(v)))
}

fn err_str(msg: impl Into<String>) -> Value {
    Value::Result(ResultVariant::Err(Box::new(Value::Str(msg.into()))))
}

fn ok_null() -> Value {
    ok(Value::Null)
}

/// `content` for `write`/`append` accepts `Str` (UTF-8 bytes) or `Bytes`.
fn content_bytes(v: &Value, ctx: &str) -> FitzResult<Vec<u8>> {
    match v {
        Value::Str(s) => Ok(s.as_bytes().to_vec()),
        Value::Bytes(b) => Ok(b.clone()),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Str | Bytes".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!(
                "{}: content must be Str or Bytes, received `{}`",
                ctx,
                other.type_name()
            ),
        )),
    }
}

/// `fs.read(path) -> Result<Str>`.
pub fn builtin_fs_read(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`fs.read(path)` expects 1 argument"));
    }
    let path = expect_str(&args[0], "fs.read")?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(ok(Value::Str(s))),
        Err(e) => Ok(err_str(format!("fs.read: `{}`: {}", path, e))),
    }
}

/// `fs.read_bytes(path) -> Result<Bytes>`.
pub fn builtin_fs_read_bytes(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`fs.read_bytes(path)` expects 1 argument"));
    }
    let path = expect_str(&args[0], "fs.read_bytes")?;
    match std::fs::read(&path) {
        Ok(b) => Ok(ok(Value::Bytes(b))),
        Err(e) => Ok(err_str(format!("fs.read_bytes: `{}`: {}", path, e))),
    }
}

/// `fs.write(path, content) -> Result<Null>` — content: Str | Bytes.
pub fn builtin_fs_write(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(arg_err("`fs.write(path, content)` expects 2 arguments"));
    }
    let path = expect_str(&args[0], "fs.write")?;
    let bytes = content_bytes(&args[1], "fs.write")?;
    match std::fs::write(&path, &bytes) {
        Ok(()) => Ok(ok_null()),
        Err(e) => Ok(err_str(format!("fs.write: `{}`: {}", path, e))),
    }
}

/// `fs.append(path, content) -> Result<Null>` — content: Str | Bytes.
pub fn builtin_fs_append(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(arg_err("`fs.append(path, content)` expects 2 arguments"));
    }
    let path = expect_str(&args[0], "fs.append")?;
    let bytes = content_bytes(&args[1], "fs.append")?;
    use std::io::Write;
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(&bytes));
    match result {
        Ok(()) => Ok(ok_null()),
        Err(e) => Ok(err_str(format!("fs.append: `{}`: {}", path, e))),
    }
}

/// `fs.exists(path) -> Bool`.
pub fn builtin_fs_exists(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`fs.exists(path)` expects 1 argument"));
    }
    let path = expect_str(&args[0], "fs.exists")?;
    Ok(Value::Bool(std::path::Path::new(&path).exists()))
}

/// `fs.list(path) -> Result<List<Str>>` — names of the entries in a directory.
pub fn builtin_fs_list(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`fs.list(path)` expects 1 argument"));
    }
    let path = expect_str(&args[0], "fs.list")?;
    match std::fs::read_dir(&path) {
        Ok(entries) => {
            let mut names: Vec<Value> = Vec::new();
            for e in entries {
                match e {
                    Ok(entry) => {
                        names.push(Value::Str(entry.file_name().to_string_lossy().into_owned()))
                    }
                    Err(e) => return Ok(err_str(format!("fs.list: `{}`: {}", path, e))),
                }
            }
            Ok(ok(Value::List(shared(names))))
        }
        Err(e) => Ok(err_str(format!("fs.list: `{}`: {}", path, e))),
    }
}

/// `fs.remove(path) -> Result<Null>` — a file or an (empty) directory.
pub fn builtin_fs_remove(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`fs.remove(path)` expects 1 argument"));
    }
    let path = expect_str(&args[0], "fs.remove")?;
    let p = std::path::Path::new(&path);
    let result = if p.is_dir() {
        std::fs::remove_dir(p)
    } else {
        std::fs::remove_file(p)
    };
    match result {
        Ok(()) => Ok(ok_null()),
        Err(e) => Ok(err_str(format!("fs.remove: `{}`: {}", path, e))),
    }
}

/// `fs.mkdir_all(path) -> Result<Null>` — creates the directory and all parents.
pub fn builtin_fs_mkdir_all(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`fs.mkdir_all(path)` expects 1 argument"));
    }
    let path = expect_str(&args[0], "fs.mkdir_all")?;
    match std::fs::create_dir_all(&path) {
        Ok(()) => Ok(ok_null()),
        Err(e) => Ok(err_str(format!("fs.mkdir_all: `{}`: {}", path, e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_err_result(v: &Value) -> bool {
        matches!(v, Value::Result(ResultVariant::Err(_)))
    }
    fn is_ok_result(v: &Value) -> bool {
        matches!(v, Value::Result(ResultVariant::Ok(_)))
    }

    #[test]
    fn read_nonexistent_returns_err_result_not_panic() {
        let v = builtin_fs_read(&[Value::Str("does/not/exist_fitz03.txt".into())]).unwrap();
        assert!(
            is_err_result(&v),
            "read of a missing file must be Err, not abort"
        );
    }

    #[test]
    fn read_wrong_arity_is_error() {
        assert!(builtin_fs_read(&[]).is_err());
        assert!(builtin_fs_read(&[Value::Str("a".into()), Value::Str("b".into())]).is_err());
    }

    #[test]
    fn read_non_str_path_is_type_error() {
        assert!(builtin_fs_read(&[Value::Int(3)]).is_err());
    }

    #[test]
    fn exists_returns_bool_never_result() {
        let v = builtin_fs_exists(&[Value::Str("still/missing_fitz03".into())]).unwrap();
        assert_eq!(v, Value::Bool(false));
    }

    #[test]
    fn write_roundtrip_read_ok() {
        let dir = std::env::temp_dir().join("fitz_fs_unit_fitz03");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("u.txt");
        let path = file.to_string_lossy().into_owned();
        let w = builtin_fs_write(&[Value::Str(path.clone()), Value::Str("xyz".into())]).unwrap();
        assert!(is_ok_result(&w));
        let r = builtin_fs_read(&[Value::Str(path.clone())]).unwrap();
        match r {
            Value::Result(ResultVariant::Ok(inner)) => assert_eq!(*inner, Value::Str("xyz".into())),
            other => panic!("expected Ok(Str), got {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_bytes_content_ok() {
        let dir = std::env::temp_dir().join("fitz_fs_unit_bytes_fitz03");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("b.bin").to_string_lossy().into_owned();
        let w = builtin_fs_write(&[Value::Str(path), Value::Bytes(vec![1, 2, 3])]).unwrap();
        assert!(is_ok_result(&w));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
