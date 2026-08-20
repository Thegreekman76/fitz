// rand.rs — FITZ-01 (2026-08): the `rand` builtin module.
//
// Two families, deliberately separated (the design decision at the core of the
// feature — most languages conflate "secure" and "reproducible"):
//
//   * GLOBAL `rand.*` — non-reproducible. Backed by a process-global SplitMix64
//     seeded ONCE from OS entropy (`rand_core::OsRng`). `rand.bytes(n)` goes
//     straight to `OsRng` (CSPRNG, for tokens).
//
//   * SEEDED `rand.seeded(N)` -> RandGen — reproducible. A `RandGen` value holds
//     its own SplitMix64 state. The SAME algorithm runs in the interpreter and
//     in the codegen-emitted Rust, so `rand.seeded(N)` produces byte-identical
//     sequences under `fitz run` and `fitz build`. This is what lets a program
//     store `seed + index` and reconstruct a run from two integers.
//
// SplitMix64 is fixed and specified here (NOT delegated to the `rand` crate,
// whose `StdRng` algorithm is not stable across versions — that would break the
// reproducibility contract and any replay stored on disk). The codegen prelude
// in `codegen.rs` emits the identical `__fitz_splitmix64` + derivations.

use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::value::{shared, ResultVariant, Shared, Value};
use parking_lot::Mutex;
use std::sync::LazyLock;

// --- Core algorithm (must stay byte-identical to the codegen prelude) --------

/// SplitMix64 — advances `state` and returns the next 64-bit output.
#[inline]
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Inclusive integer in `[min, max]`. `Err` if `min > max`.
fn next_int(state: &mut u64, min: i64, max: i64) -> Result<i64, String> {
    if min > max {
        return Err(format!("rand.int: min ({}) must be <= max ({})", min, max));
    }
    let lo = min as i128;
    let hi = max as i128;
    let span = (hi - lo + 1) as u128;
    let v = (splitmix64(state) as u128) % span;
    Ok((lo + v as i128) as i64)
}

/// Float in `[0, 1)` with 53 bits of mantissa.
fn next_float(state: &mut u64) -> f64 {
    (splitmix64(state) >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
}

/// Uniform bool.
fn next_bool(state: &mut u64) -> bool {
    splitmix64(state) & 1 == 1
}

/// Fisher-Yates shuffle of `items` in place, using `state`.
fn shuffle_in_place(state: &mut u64, items: &mut [Value]) {
    let n = items.len();
    if n < 2 {
        return;
    }
    for i in (1..n).rev() {
        // j in [0, i] — reuse next_int (i fits in i64 for any real list).
        let j = next_int(state, 0, i as i64).unwrap() as usize;
        items.swap(i, j);
    }
}

// --- Process-global RNG (non-reproducible) -----------------------------------

static GLOBAL_RNG: LazyLock<Mutex<u64>> = LazyLock::new(|| {
    use rand_core::RngCore;
    // Seed once from OS entropy. A zero seed would make SplitMix64 start at a
    // fixed point, so re-roll on the (astronomically unlikely) zero.
    let mut seed = rand_core::OsRng.next_u64();
    if seed == 0 {
        seed = 0x9E37_79B9_7F4A_7C15;
    }
    Mutex::new(seed)
});

// --- Helpers -----------------------------------------------------------------

fn arg_err(msg: impl Into<String>) -> FitzError {
    FitzError::new(ErrorKind::InvalidSyntax, 0, 0, msg.into())
}

fn expect_int(v: &Value, ctx: &str) -> FitzResult<i64> {
    match v {
        Value::Int(n) => Ok(*n),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "Int".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!("{}: expects Int, received `{}`", ctx, other.type_name()),
        )),
    }
}

fn expect_list(v: &Value, ctx: &str) -> FitzResult<Vec<Value>> {
    match v {
        Value::List(items) => Ok(items.lock().clone()),
        other => Err(FitzError::new(
            ErrorKind::TypeMismatch {
                expected: "List".into(),
                found: other.type_name().into(),
            },
            0,
            0,
            format!("{}: expects List, received `{}`", ctx, other.type_name()),
        )),
    }
}

fn ok(v: Value) -> Value {
    Value::Result(ResultVariant::Ok(Box::new(v)))
}

fn err_str(msg: impl Into<String>) -> Value {
    Value::Result(ResultVariant::Err(Box::new(Value::Str(msg.into()))))
}

// --- Global builtins (`rand.<fn>`) -------------------------------------------

/// `rand.int(min, max) -> Int` — inclusive both ends.
pub fn builtin_rand_int(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(arg_err("`rand.int(min, max)` expects 2 arguments"));
    }
    let min = expect_int(&args[0], "rand.int")?;
    let max = expect_int(&args[1], "rand.int")?;
    let mut g = GLOBAL_RNG.lock();
    next_int(&mut g, min, max).map(Value::Int).map_err(arg_err)
}

/// `rand.float() -> Float` — in `[0, 1)`.
pub fn builtin_rand_float(args: &[Value]) -> FitzResult<Value> {
    if !args.is_empty() {
        return Err(arg_err("`rand.float()` expects 0 arguments"));
    }
    let mut g = GLOBAL_RNG.lock();
    Ok(Value::Float(next_float(&mut g)))
}

/// `rand.bool() -> Bool`.
pub fn builtin_rand_bool(args: &[Value]) -> FitzResult<Value> {
    if !args.is_empty() {
        return Err(arg_err("`rand.bool()` expects 0 arguments"));
    }
    let mut g = GLOBAL_RNG.lock();
    Ok(Value::Bool(next_bool(&mut g)))
}

/// `rand.choice(xs) -> Result<T>` — `Err` if empty.
pub fn builtin_rand_choice(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`rand.choice(xs)` expects 1 argument"));
    }
    let items = expect_list(&args[0], "rand.choice")?;
    if items.is_empty() {
        return Ok(err_str("rand.choice: empty list"));
    }
    let mut g = GLOBAL_RNG.lock();
    let idx = next_int(&mut g, 0, items.len() as i64 - 1).unwrap() as usize;
    Ok(ok(items[idx].clone()))
}

/// `rand.shuffle(xs) -> List<T>` — a new shuffled copy (Fisher-Yates).
pub fn builtin_rand_shuffle(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`rand.shuffle(xs)` expects 1 argument"));
    }
    let mut items = expect_list(&args[0], "rand.shuffle")?;
    let mut g = GLOBAL_RNG.lock();
    shuffle_in_place(&mut g, &mut items);
    Ok(Value::List(shared(items)))
}

/// `rand.sample(xs, n) -> Result<List<T>>` — `n` distinct items; `Err` if `n > len`.
pub fn builtin_rand_sample(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 2 {
        return Err(arg_err("`rand.sample(xs, n)` expects 2 arguments"));
    }
    let mut items = expect_list(&args[0], "rand.sample")?;
    let n = expect_int(&args[1], "rand.sample")?;
    if n < 0 {
        return Ok(err_str("rand.sample: n must be >= 0"));
    }
    let n = n as usize;
    if n > items.len() {
        return Ok(err_str(format!(
            "rand.sample: n ({}) is larger than the list ({})",
            n,
            items.len()
        )));
    }
    let mut g = GLOBAL_RNG.lock();
    // Partial Fisher-Yates: shuffle the first `n` positions and take them.
    let len = items.len();
    for i in 0..n {
        let j = next_int(&mut g, i as i64, len as i64 - 1).unwrap() as usize;
        items.swap(i, j);
    }
    items.truncate(n);
    Ok(ok(Value::List(shared(items))))
}

/// `rand.bytes(n) -> Bytes` — CSPRNG (OS entropy), for tokens.
pub fn builtin_rand_bytes(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`rand.bytes(n)` expects 1 argument"));
    }
    let n = expect_int(&args[0], "rand.bytes")?;
    if n < 0 {
        return Err(arg_err("`rand.bytes(n)`: n must be >= 0"));
    }
    use rand_core::RngCore;
    let mut buf = vec![0u8; n as usize];
    rand_core::OsRng.fill_bytes(&mut buf);
    Ok(Value::Bytes(buf))
}

/// `rand.seeded(N) -> RandGen` — a reproducible generator seeded with `N`.
pub fn builtin_rand_seeded(args: &[Value]) -> FitzResult<Value> {
    if args.len() != 1 {
        return Err(arg_err("`rand.seeded(seed)` expects 1 argument (Int)"));
    }
    let seed = expect_int(&args[0], "rand.seeded")?;
    Ok(Value::RandGen(shared(seed as u64)))
}

// --- Seeded generator methods (`<RandGen>.<method>`) -------------------------

/// Dispatches a method call on a `RandGen` value. Sync — mutates the generator's
/// SplitMix64 state under its own lock. Same surface as `rand.*` minus `bytes`.
pub fn rand_gen_method(state: &Shared<u64>, method: &str, args: &[Value]) -> FitzResult<Value> {
    match method {
        "int" => {
            if args.len() != 2 {
                return Err(arg_err("`<RandGen>.int(min, max)` expects 2 arguments"));
            }
            let min = expect_int(&args[0], "RandGen.int")?;
            let max = expect_int(&args[1], "RandGen.int")?;
            let mut s = state.lock();
            next_int(&mut s, min, max).map(Value::Int).map_err(arg_err)
        }
        "float" => {
            if !args.is_empty() {
                return Err(arg_err("`<RandGen>.float()` expects 0 arguments"));
            }
            let mut s = state.lock();
            Ok(Value::Float(next_float(&mut s)))
        }
        "bool" => {
            if !args.is_empty() {
                return Err(arg_err("`<RandGen>.bool()` expects 0 arguments"));
            }
            let mut s = state.lock();
            Ok(Value::Bool(next_bool(&mut s)))
        }
        "choice" => {
            if args.len() != 1 {
                return Err(arg_err("`<RandGen>.choice(xs)` expects 1 argument"));
            }
            let items = expect_list(&args[0], "RandGen.choice")?;
            if items.is_empty() {
                return Ok(err_str("rand.choice: empty list"));
            }
            let mut s = state.lock();
            let idx = next_int(&mut s, 0, items.len() as i64 - 1).unwrap() as usize;
            Ok(ok(items[idx].clone()))
        }
        "shuffle" => {
            if args.len() != 1 {
                return Err(arg_err("`<RandGen>.shuffle(xs)` expects 1 argument"));
            }
            let mut items = expect_list(&args[0], "RandGen.shuffle")?;
            let mut s = state.lock();
            shuffle_in_place(&mut s, &mut items);
            Ok(Value::List(shared(items)))
        }
        "sample" => {
            if args.len() != 2 {
                return Err(arg_err("`<RandGen>.sample(xs, n)` expects 2 arguments"));
            }
            let mut items = expect_list(&args[0], "RandGen.sample")?;
            let n = expect_int(&args[1], "RandGen.sample")?;
            if n < 0 {
                return Ok(err_str("rand.sample: n must be >= 0"));
            }
            let n = n as usize;
            if n > items.len() {
                return Ok(err_str(format!(
                    "rand.sample: n ({}) is larger than the list ({})",
                    n,
                    items.len()
                )));
            }
            let mut s = state.lock();
            let len = items.len();
            for i in 0..n {
                let j = next_int(&mut s, i as i64, len as i64 - 1).unwrap() as usize;
                items.swap(i, j);
            }
            items.truncate(n);
            Ok(ok(Value::List(shared(items))))
        }
        other => Err(arg_err(format!(
            "`RandGen` has no method `{}` (supported: int, float, bool, choice, shuffle, sample)",
            other
        ))),
    }
}
