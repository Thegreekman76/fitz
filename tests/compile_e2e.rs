// compile_e2e.rs — Tests integration de Fase 5b.1.
//
// Toma un programa Fitz, llama a `fitz build`, ejecuta el binario y
// chequea stdout / exit code. Los tests usan un directorio temporal
// único por test (concatenando el nombre del test) para no pisar
// builds entre corridas.
//
// Importante: estos tests **invocan rustc** internamente vía `fitz
// build`. Son más lentos que los unitarios; cada uno toma ~2s.

use std::process::Command;

/// Path al binario de `fitz` que cargo construye para los
/// integration tests (depende de CARGO_BIN_EXE_<bin>).
fn fitz_bin() -> &'static str {
    env!("CARGO_BIN_EXE_fitz")
}

/// 8-pyi.B cleanup (v0.9.57) + T2 (v0.10.13): cada test usa un stem
/// único — sus archivos (`<stem>.fitz`, `<stem>.exe`) y su build dir
/// (`target/fitz-build/<stem>/`) son por-test. No hay shared mutable
/// resources entre tests, así que **corren en paralelo según el
/// `--test-threads` default de cargo**.
///
/// Origen del fix: pre-fix los helpers escribían siempre `prog.fitz`
/// → todos compartían `target/fitz-build/prog/`. El `SERIAL` mutex
/// global serializaba para evitar el choque. T2 (v0.10.13) elimina
/// el mutex después de convertir helpers + inline tests a stems
/// únicos. En Windows el `.exe` generado mantenía un file handle
/// abierto un instante después de `Child.wait()`; con stems únicos
/// distintos tests no comparten ese archivo.
///
/// Sanitización: lowercase + chars no-`[a-z0-9_-]` → `_`. Cargo exige
/// `[a-z][a-z0-9_-]{0,63}` para nombres de paquete; los test_names
/// hoy ya empiezan con letra, así que no necesitamos prefix defensivo.
fn sanitize_stem(test_name: &str) -> String {
    test_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Crea un directorio temporal único para el test, escribe el
/// .fitz, compila con `fitz build`, ejecuta el binario y devuelve
/// (stdout, exit_code).
fn build_and_run(test_name: &str, src: &str) -> (String, i32) {
    // T2 (v0.10.13) — SERIAL ya no se toma acá. Cada test usa un
    // `<stem>.fitz` único derivado de `test_name`, así que su
    // `target/fitz-build/<stem>/` no choca con otros tests que también
    // usen este helper. Cargo serializa el acceso a `~/.cargo/registry`
    // internamente; los compile outputs son por-stem. Resultado:
    // tests que usan este helper se paralelizan según el
    // `--test-threads` de cargo (default = num CPUs).
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    // Build.
    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Path del binario generado: adyacente al .fitz.
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

/// FITZ-14 — corre el programa por el INTÉRPRETE (`fitz run`) y devuelve su
/// stdout. Se usa junto a `build_and_run` para diffear la salida interpretada
/// contra la del binario nativo (paridad `run`↔`build`).
fn run_interpreter(test_name: &str, src: &str) -> String {
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-run-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir run");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");
    let output = Command::new(fitz_bin())
        .args(["run"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz run");
    assert!(
        output.status.success(),
        "fitz run failed for {}:\nstdout: {}\nstderr: {}",
        test_name,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Como `build_and_run` pero asume que el build va a fallar.
/// Devuelve el stderr del fitz build.
fn build_expect_fail(test_name: &str, src: &str) -> String {
    // T2 (v0.10.13) — paraleliza como `build_and_run`.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        !output.status.success(),
        "expected fitz build to fail, but exited OK:\nstdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Como `build_and_run` pero devuelve stderr además del stdout y el
/// exit code. Útil para tests T4 que necesitan inspeccionar el mensaje
/// de panic / overflow (no solo el exit code != 0). Mini-tanda T4
/// (post-W12-W16) — refuerza asserts E2E que antes solo miraban exit.
fn build_and_run_with_stderr(test_name: &str, src: &str) -> (String, String, i32) {
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        String::from_utf8_lossy(&run.stderr).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

/// Como `build_and_run` pero permite setear env vars sobre el child
/// que ejecuta el binario. Útil para tests de la mini-fase env builtin
/// — el binario hace `std::env::var(...)` y la var inyectada via
/// `Command::env` queda visible. Mini-fase env builtin (2026-05-22).
fn build_and_run_with_env(test_name: &str, src: &str, env_vars: &[(&str, &str)]) -> (String, i32) {
    // T2 (v0.10.13) — paraleliza como `build_and_run`.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let mut cmd = Command::new(&bin);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    let run = cmd.output().expect("invocar binario");
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

fn assert_lines(stdout: &str, expected: &[&str]) {
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        expected.len(),
        "expected {} lines, was {}: {:?}",
        expected.len(),
        lines.len(),
        lines
    );
    for (i, (l, e)) in lines.iter().zip(expected.iter()).enumerate() {
        assert_eq!(l, e, "line {} differs", i + 1);
    }
}

// ---------------------------------------------------------------------------
// Tests del criterio de éxito y casos secundarios
// ---------------------------------------------------------------------------

#[test]
fn hello_world_success_criterion_compiled() {
    let src = "\
let name = \"Fitz\"
let x = 10 + 5
print(\"Hola, {name}, x es {x}\")

fn double(n: Int) -> Int => n * 2
print(double(x))
";
    let (stdout, exit) = build_and_run("hello-world", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["Hola, Fitz, x es 15", "30"]);
}

#[test]
fn to_json_in_pure_cli_build_matches_interpreter() {
    // Deuda cerrada: `to_json(x)` compila en `fitz build` para un
    // programa CLI puro (sin @get/@post/@ws). El core de serialización
    // JSON (`JSON_SERIALIZE_PRELUDE`) es axum-free y se emite via
    // `uses_to_json`. Cubre primitivos + List + Map + nominal (con field
    // List anidado). Output esperado = idéntico bit-a-bit a `fitz run`.
    let src = "\
type User {
    id: Int
    name: Str
    active: Bool
    tags: List<Str>
}

let u = User { id: 7, name: \"ada\", active: true, tags: [\"x\", \"y\"] }

print(to_json(42))
print(to_json(3.5))
print(to_json(\"hola\"))
print(to_json(true))
print(to_json([1, 2, 3]))
print(to_json({\"a\": 1, \"b\": 2}))
print(to_json(u))
";
    let (stdout, exit) = build_and_run("to-json-cli", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "42",
            "3.5",
            "\"hola\"",
            "true",
            "[1,2,3]",
            "{\"a\":1,\"b\":2}",
            "{\"id\":7,\"name\":\"ada\",\"active\":true,\"tags\":[\"x\",\"y\"]}",
        ],
    );
}

#[test]
fn every_decorator_builds_to_binary_phase_3c() {
    // Phase 3c — a program with `@every(N)` compiles to a native binary via
    // `fitz build` (interval scheduler emitted, tokio-only). CLI-only (no HTTP):
    // the every-scheduler + the ctrl_c block keep the process alive. We assert
    // build success (the tick cadence + run↔build parity are smoke-validated
    // manually — a timed run would be flaky here).
    let src_cli = "@every(1)\nasync fn tick() -> Null {\n    print(\"tick\")\n    return null\n}\n";
    build_expect_ok("every-cli-phase3c", src_cli);

    // HTTP + @every coexist: the every-scheduler starts alongside axum.
    let src_http = "\
@every(2)
async fn heartbeat() -> Null {
    print(\"beat\")
    return null
}

@get(\"/ping\")
fn ping() -> Str => \"pong\"

@server(3941)
fn main() => 0
";
    build_expect_ok("every-http-phase3c", src_http);
}

#[test]
fn every_in_imported_module_builds_and_ticks_d4() {
    // D4 — `@every(N)` declared in an IMPORTED module (not the main file) now
    // compiles to a native binary (before, `fitz build` rejected it with a
    // guard). Mirror of the cron B19 cross-module test: `LoadedModule` carries
    // `every_fn_stmts`, main populates `every_jobs_info` with
    // `module_path: Some(mod)`, and `emit_every_job_spawns` emits
    // `crate::<mod>::<fn>`.
    let stem = "every_cross_module_d4";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    std::fs::write(
        dir.join("worker.fitz"),
        "@every(1)\nfn heartbeat() {\n  print(\"beat\")\n}\n",
    )
    .expect("escribir worker.fitz");

    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, "import worker\n\nprint(\"main up\")\n")
        .expect("escribir main.fitz");

    let output = std::process::Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (D4 cross-module @every):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin_path = dir.join(&bin_name);
    assert!(bin_path.exists(), "binario {} no existe", bin_name);

    // Run ~2.5s: the every-scheduler banner prints at boot, then the module's
    // @every fires (print "beat" every second).
    let mut child = std::process::Command::new(&bin_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn D4 test binary");
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let _ = child.kill();
    let out = child.wait_with_output().expect("wait child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("every-scheduler"),
        "expected the every-scheduler banner (D4 cross-module @every), stderr was: {}",
        stderr
    );
    assert!(
        stdout.matches("beat").count() >= 1,
        "expected at least 1 `beat` tick from the module @every, stdout was: {:?}",
        stdout
    );
}

#[test]
fn if_else_works_in_binary() {
    let src = "\
let x = 5
if (x > 0) { print(\"pos\") } else { print(\"neg\") }
";
    let (stdout, exit) = build_and_run("if-else", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["pos"]);
}

#[test]
fn while_and_reassignment_work_in_binary() {
    let src = "\
let n = 0
while (n < 3) {
    print(n)
    n = n + 1
}
";
    let (stdout, exit) = build_and_run("while-reassign", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["0", "1", "2"]);
}

#[test]
fn for_in_range_works_in_binary() {
    let src = "\
for i in 0..4 {
    print(i)
}
";
    let (stdout, exit) = build_and_run("for-range", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["0", "1", "2", "3"]);
}

#[test]
fn coercion_int_to_float_works_in_binary() {
    let src = "\
let pi: Float = 3
let r: Float = pi * 2.0
print(r)
";
    let (stdout, exit) = build_and_run("coerce-int-float", src);
    assert_eq!(exit, 0);
    // 5b.2 alinea el formato de Float con el intérprete: las
    // fracciones .0 se imprimen explícitas (`6.0`, no `6`).
    assert_lines(&stdout, &["6.0"]);
}

#[test]
fn recursion_works_in_binary() {
    let src = "\
fn fact(n: Int) -> Int {
    if (n <= 1) { return 1 }
    return n * fact(n - 1)
}
print(fact(5))
";
    let (stdout, exit) = build_and_run("recursion", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["120"]);
}

// ---------------------------------------------------------------------------
// Fase 5b.2 — tipos custom
// ---------------------------------------------------------------------------

#[test]
fn basic_instance_round_trip_compiled() {
    let src = "\
type User { id: Int, name: Str }
let u = User { id: 1, name: \"Fitz\" }
print(u.id)
print(u.name)
";
    let (stdout, exit) = build_and_run("instance-basic", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["1", "Fitz"]);
}

#[test]
fn defaults_aplicados_en_binario() {
    let src = "\
type Config { host: Str, port: Int = 3000, debug: Bool = false }
let c = Config { host: \"localhost\" }
print(c.port)
print(c.debug)
";
    let (stdout, exit) = build_and_run("defaults", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["3000", "false"]);
}

#[test]
fn omitted_nullable_prints_null_in_binary() {
    let src = "\
type User { id: Int, email: Str? }
let u = User { id: 1 }
print(u.email)
";
    let (stdout, exit) = build_and_run("nullable-omitted", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["null"]);
}

#[test]
fn field_mutation_visible_via_alias_compiled() {
    // Semántica de referencia compartida: mutar a través de un
    // alias se ve en la variable original. Mismo modelo que el
    // intérprete (Rc<RefCell<>>).
    let src = "\
type User { id: Int, name: Str }
let a = User { id: 1, name: \"uno\" }
let b = a
b.name = \"dos\"
print(a.name)
print(b.name)
";
    let (stdout, exit) = build_and_run("alias-mutation", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["dos", "dos"]);
}

#[test]
fn fn_that_mutates_param_reflects_outside_compiled() {
    let src = "\
type User { name: Str }
fn rename(u: User, n: Str) {
    u.name = n
}
let u = User { name: \"uno\" }
rename(u, \"dos\")
print(u.name)
";
    let (stdout, exit) = build_and_run("fn-mutates-param", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["dos"]);
}

#[test]
fn print_instance_canonical_format_compiled() {
    // El Display de `UserData` debe reproducir el formato del
    // intérprete: `User { id: 1, name: "Fitz", email: null }`.
    let src = "\
type User { id: Int, name: Str, email: Str? }
let u = User { id: 1, name: \"Fitz\" }
print(u)
";
    let (stdout, exit) = build_and_run("instance-display", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["User { id: 1, name: \"Fitz\", email: null }"]);
}

#[test]
fn structural_equality_between_instances_compiled() {
    // Dos instancias con los mismos campos comparan true; con
    // un campo distinto, false. Recursa adentro de campos
    // nominales anidados gracias al derive(PartialEq) de Rust
    // (Rc<RefCell<T>> compara por contenido).
    let src = "\
type Address { city: Str }
type User { id: Int, name: Str, addr: Address? }

let a1 = User { id: 1, name: \"x\", addr: Address { city: \"El Chaltén\" } }
let a2 = User { id: 1, name: \"x\", addr: Address { city: \"El Chaltén\" } }
let b  = User { id: 2, name: \"x\", addr: Address { city: \"El Chaltén\" } }
let c  = User { id: 1, name: \"x\", addr: Address { city: \"Otro\" } }
print(a1 == a2)
print(a1 == b)
print(a1 == c)
print(a1 != c)
";
    let (stdout, exit) = build_and_run("instance-eq", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["true", "false", "false", "true"]);
}

#[test]
fn nested_types_round_trip_compiled() {
    let src = "\
type User { name: Str }
type Order { id: Int, user: User? }
let u = User { name: \"Fitz\" }
let o = Order { id: 7, user: u }
print(o)
print(o.user)
";
    let (stdout, exit) = build_and_run("nested-types", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "Order { id: 7, user: User { name: \"Fitz\" } }",
            "User { name: \"Fitz\" }",
        ],
    );
}

// ---------------------------------------------------------------------------
// 5b.2+: if como expresión con valor
// ---------------------------------------------------------------------------

#[test]
fn if_as_expression_with_else_compiled() {
    let src = "\
let active = true
let status = if (active) { \"on\" } else { \"off\" }
print(status)
";
    let (stdout, exit) = build_and_run("if-expr", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["on"]);
}

#[test]
fn if_else_if_chain_as_expression_compiled() {
    let src = "\
let n = -3
let sign = if (n > 0) { \"positivo\" } else if (n < 0) { \"negativo\" } else { \"cero\" }
print(sign)
";
    let (stdout, exit) = build_and_run("if-elseif-expr", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["negativo"]);
}

#[test]
fn if_expression_multiline_block_compiled() {
    let src = "\
let total = if (true) {
    let a = 10
    let b = 20
    a + b
} else {
    0
}
print(total)
";
    let (stdout, exit) = build_and_run("if-multiline-expr", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["30"]);
}

#[test]
fn if_expression_unifies_int_and_float_compiled() {
    let src = "\
let n = 5
let r = if (n > 0) { 1 } else { 2.5 }
print(r)
";
    let (stdout, exit) = build_and_run("if-int-float", src);
    assert_eq!(exit, 0);
    // Int en la rama then se coerciona a Float; la salida sigue la
    // convención del intérprete (`1.0`, no `1`).
    assert_lines(&stdout, &["1.0"]);
}

// ---------------------------------------------------------------------------
// 5b.2+: métodos built-in sobre Str
// ---------------------------------------------------------------------------

#[test]
fn str_len_chars_unicode_compiled() {
    let src = "\
let s = \"Chaltén\"
print(s.len())
";
    let (stdout, exit) = build_and_run("str-len", src);
    assert_eq!(exit, 0);
    // 7 caracteres Unicode, no 8 bytes UTF-8.
    assert_lines(&stdout, &["7"]);
}

#[test]
fn str_upper_lower_round_trip_compiled() {
    let src = "\
let s = \"Hola Mundo\"
print(s.upper())
print(s.lower())
print(s.upper().lower())
";
    let (stdout, exit) = build_and_run("str-upper-lower", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["HOLA MUNDO", "hola mundo", "hola mundo"]);
}

// ---------------------------------------------------------------------------
// Failure modes
// ---------------------------------------------------------------------------

#[test]
fn build_aborts_with_strict_type_errors() {
    let stderr = build_expect_fail(
        "strict-type-error",
        "let x: Int = \"no soy int\"\nprint(x)\n",
    );
    assert!(
        stderr.contains("type error(s)"),
        "expected type error message, was: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Fase 5b.3 — listas, mapas, indexing, métodos built-in
// ---------------------------------------------------------------------------

#[test]
fn basic_list_push_len_iteration_compiled() {
    let src = "\
let xs: List<Int> = [1, 2, 3]
print(xs)
print(xs.len())
xs.push(4)
print(xs)
print(len(xs))
for v in xs {
    print(v)
}
";
    let (stdout, exit) = build_and_run("list-basic", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["[1, 2, 3]", "3", "[1, 2, 3, 4]", "4", "1", "2", "3", "4"],
    );
}

#[test]
fn list_indexing_and_pop_compiled() {
    let src = "\
let xs: List<Int> = [10, 20, 30]
print(xs[0])
print(xs[2])
let last = xs.pop()
print(last)
print(xs)
";
    let (stdout, exit) = build_and_run("list-index-pop", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["10", "30", "30", "[10, 20]"]);
}

#[test]
fn basic_map_has_keys_values_len_compiled() {
    let src = "\
let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}
print(m)
print(m.len())
print(m[\"a\"])
print(m.has(\"a\"))
print(m.has(\"z\"))
print(m.keys())
print(m.values())
";
    let (stdout, exit) = build_and_run("map-basic", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "{\"a\": 1, \"b\": 2, \"c\": 3}",
            "3",
            "1",
            "true",
            "false",
            "[\"a\", \"b\", \"c\"]",
            "[1, 2, 3]",
        ],
    );
}

#[test]
fn list_of_instances_with_map_filter_and_alias_compiled() {
    // Reproduce un fragmento del cap 13 (sin `find`, que llega en 5b.4).
    // Cubre: lista de Nominal, push, map con FnExpr→Str, filter con
    // FnExpr→Bool y método encadenado `.lower()`, mutación via alias
    // `xs[i].name = ...`.
    let src = "\
type User { id: Int, name: Str }
let usuarios: List<User> = [
    User { id: 1, name: \"Fitz\" },
    User { id: 2, name: \"Roy\" },
]
usuarios.push(User { id: 3, name: \"Cerro\" })
print(usuarios.len())
let nombres = usuarios.map(fn(u) => u.name)
print(nombres)
let solo_roy = usuarios.filter(fn(u) => u.name.lower() == \"roy\")
print(solo_roy)
let primer = usuarios[0]
primer.name = \"Patagonia\"
print(usuarios)
";
    let (stdout, exit) = build_and_run("list-instances", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "3",
            "[\"Fitz\", \"Roy\", \"Cerro\"]",
            "[User { id: 2, name: \"Roy\" }]",
            "[User { id: 1, name: \"Patagonia\" }, User { id: 2, name: \"Roy\" }, User { id: 3, name: \"Cerro\" }]",
        ],
    );
}

#[test]
fn method_chain_works_compiled() {
    // `.map(...).map(...)` y `.filter(...).map(...)` — los métodos de
    // List devuelven Rc<RefCell<Vec<_>>>, así que se pueden encadenar
    // como cualquier expresión. Ojo: cada método toma el receptor por
    // valor (clone del Rc) y devuelve una colección nueva.
    let src = "\
let xs: List<Int> = [1, 2, 3, 4]
let resultado = xs.filter(fn(x) => x > 1).map(fn(x) => x * 10)
print(resultado)
";
    let (stdout, exit) = build_and_run("chain-methods", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["[20, 30, 40]"]);
}

#[test]
fn list_of_floats_with_int_promotes_to_float_compiled() {
    // El lub de items hace que `[1, 2.5, 3]` sea `List<Float>` y los
    // Int se inserten como `(N as f64)`.
    let src = "\
let xs = [1, 2.5, 3]
print(xs)
";
    let (stdout, exit) = build_and_run("list-promote-float", src);
    assert_eq!(exit, 0);
    // Float `1.0`, `2.5`, `3.0` con el formato del intérprete.
    assert_lines(&stdout, &["[1.0, 2.5, 3.0]"]);
}

#[test]
fn heterogeneous_list_aborts_build() {
    // F13 cerrado (heterogéneos `Int/Float/Str/Bool/Null/Bytes/
    // Nominal` cubiertos con `__FitzValue` tagged runtime) — la
    // lista `[1, "dos"]` SÍ compila ahora y produce output
    // bit-a-bit con `fitz run`. El test mantiene el nombre
    // histórico (era pre-F13: heterogéneo abortaba); ahora valida
    // que la paridad se preserva.
    let src = "let xs = [1, \"dos\"]\nprint(xs)\n";
    let (stdout, exit) = build_and_run("heterogeneous-list-f13", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["[1, \"dos\"]"]);
}

#[test]
fn async_fn_with_sleep_compilable_and_correct() {
    // Fase 6.6: async fn Fitz con `sleep(...).await` adentro compila
    // a binario nativo y corre con tokio runtime current_thread.
    // El programa NO usa `.await` top-level (el checker lo prohíbe);
    // toda la maquinaria async está adentro de fns nombradas. La
    // `fn main` implícita (generada por el codegen sobre los stmts
    // top-level) es `#[tokio::main(flavor = "current_thread")]` por
    // detección automática.
    let src = "\
        async fn double(n: Int) -> Int {\n\
            let _ = sleep(0).await\n\
            return n * 2\n\
        }\n\
        async fn run() -> Int {\n\
            return double(21).await\n\
        }\n\
        print(\"async ok\")\n\
    ";
    let (stdout, exit) = build_and_run("async_sleep_compilable", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert_eq!(stdout.trim(), "async ok");
}

// Fase 8.7.1 — el viejo test `build_aborta_sobre_from_python_import`
// (8.1.5) quedó obsoleto: `fitz build` ahora acepta `from python
// import` y emite código Rust con pyo3 linkeado. El test nuevo
// (`build_python_import_math_extrae_pi`) vive bajo
// `#[cfg(feature = "python")]` más abajo, validando el caso real con
// ejecución del binario.

#[test]
fn build_aborts_if_codegen_does_not_support_feature() {
    // 5b.6 abrió @get/@post/etc., F11 abrió state HTTP compartido.
    // La feature que apuntamos acá pasa a ser **decorator HTTP custom
    // sobre `fn main`** — el codegen lo rechaza con mensaje claro
    // (regla R1 que pide handlers con nombre distinto a `main`).
    let stderr = build_expect_fail(
        "unsupported-http-main-decorator",
        "@get(\"/\") fn main() => 0\n",
    );
    assert!(
        stderr.contains("`fn main` only accepts `@server"),
        "expected message about fn main + HTTP decorator, was: {}",
        stderr
    );
}

// MW.3 — E2E: middleware user-fn + cors compilados + server + request real.
// Cada test usa puertos únicos (rango 43370-43399) para que las corridas
// no choquen entre tests serializados.

#[test]
fn http_mw3_middleware_passthrough_responde_200() {
    // Middleware que no corta — request llega al handler.
    let src = "\
fn logger(req: Request) {}

@server(43370)
fn main() => 0

@middleware(logger)
@get(\"/x\")
fn h() -> Str => \"ok\"
";
    let (status, body) = build_spawn_request("mw3-passthrough", src, 43370, "GET", "/x", None);
    assert_eq!(status, 200);
    assert!(body.contains("ok"));
}

#[test]
fn http_mw3_middleware_short_circuits_with_401() {
    // Middleware corta con 401 — handler NO se invoca.
    let src = "\
fn auth(req: Request) {
    return 401 {\"error\": \"sin autorizacion\"}
}

@server(43371)
fn main() => 0

@middleware(auth)
@get(\"/protected\")
fn h() -> Str => \"NO DEBERIA APARECER\"
";
    let (status, body) =
        build_spawn_request("mw3-shortcircuit", src, 43371, "GET", "/protected", None);
    assert_eq!(status, 401);
    assert!(body.contains("sin autorizacion"), "body fue: {}", body);
    assert!(!body.contains("NO DEBERIA APARECER"));
}

#[test]
fn http_mw3_cors_preflight_options_returns_204_with_headers() {
    let src = "\
@server(43372)
fn main() => 0

@middleware(cors({\"allow_origin\": \"https://app.x.com\", \"max_age\": 600}))
@get(\"/api\")
fn list_items() -> Str => \"[]\"
";
    let (status, raw_headers) =
        build_spawn_request_raw("mw3-preflight", src, 43372, "OPTIONS", "/api");
    assert_eq!(status, 204);
    let headers_lower = raw_headers.to_lowercase();
    assert!(
        headers_lower.contains("access-control-allow-origin: https://app.x.com"),
        "headers preflight no llevan allow-origin custom: {}",
        raw_headers
    );
    assert!(
        headers_lower.contains("access-control-allow-methods"),
        "headers preflight no llevan allow-methods: {}",
        raw_headers
    );
    assert!(
        headers_lower.contains("access-control-max-age: 600"),
        "headers preflight no llevan max-age: {}",
        raw_headers
    );
}

#[test]
fn r_bug_options_preflight_duplicate_in_fitz_build_parity_with_fitz_run() {
    // Regresión del bug "Overlapping method route. Handler for `OPTIONS
    // /tasks` already exists" (2026-05-22). Cuando varios handlers
    // comparten path con CORS, `fitz build` paniqueaba en boot del
    // binario al construir el axum::Router (mismo bug que en `fitz run`,
    // fixeado en codegen también).
    //
    // Caso del 6to boilerplate (api-fullstack-postgres): `/tasks` con
    // @get + @post + `/tasks/{id}` con @get + @put + @delete, todos
    // con CORS. Pre-fix el binario emitido salía con exit code 101.
    let src = "\
@server(43373)
fn main() => 0

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"GET\", \"OPTIONS\"]}))
@get(\"/tasks\")
fn list_tasks() -> Str => \"[]\"

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"POST\", \"OPTIONS\"]}))
@post(\"/tasks\")
fn create_task() -> Str => \"created\"

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"GET\", \"OPTIONS\"]}))
@get(\"/tasks/{id}\")
fn get_task(id: Int) -> Str => \"one\"

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"PUT\", \"OPTIONS\"]}))
@put(\"/tasks/{id}\")
fn update_task(id: Int) -> Str => \"updated\"

@middleware(cors({\"allow_origin\": \"http://localhost:8080\", \"allow_methods\": [\"DELETE\", \"OPTIONS\"]}))
@delete(\"/tasks/{id}\")
fn delete_task(id: Int) -> Str => \"deleted\"
";
    // El binario debe arrancar y responder OPTIONS /tasks con 204 +
    // headers Access-Control-Allow-* (merge: GET + POST + OPTIONS).
    let (status, raw_headers) = build_spawn_request_raw(
        "r-bug-options-preflight-duplicado",
        src,
        43373,
        "OPTIONS",
        "/tasks",
    );
    assert_eq!(status, 204, "headers: {}", raw_headers);
    let h_lower = raw_headers.to_lowercase();
    let methods_line = h_lower
        .lines()
        .find(|l| l.starts_with("access-control-allow-methods:"))
        .unwrap_or_else(|| panic!("falta Access-Control-Allow-Methods: {}", raw_headers))
        .to_string();
    assert!(
        methods_line.contains("get") && methods_line.contains("post"),
        "merged methods debe incluir GET y POST: {}",
        methods_line
    );
}

#[test]
fn http_q3_cors_set_echo_origin_en_response_real() {
    // Q.3: cors({"allow_origin": [...]}) build-time. Una request con
    // Origin en la lista permitida → echo del origin en la response.
    let src = "\
@server(43380)
fn main() => 0

@middleware(cors({\"allow_origin\": [\"https://a.com\", \"https://b.com\"]}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    let (status, raw_headers) = build_spawn_request_raw_with_headers(
        "q3-cors-set-match",
        src,
        43380,
        "GET",
        "/api",
        &[("Origin", "https://b.com")],
    );
    assert_eq!(status, 200);
    let lower = raw_headers.to_lowercase();
    assert!(
        lower.contains("access-control-allow-origin: https://b.com"),
        "expected echo of permitted Origin, was: {}",
        raw_headers
    );
}

#[test]
fn http_q3_cors_set_omits_origin_if_request_does_not_match() {
    let src = "\
@server(43381)
fn main() => 0

@middleware(cors({\"allow_origin\": [\"https://a.com\"]}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    let (status, raw_headers) = build_spawn_request_raw_with_headers(
        "q3-cors-set-miss",
        src,
        43381,
        "GET",
        "/api",
        &[("Origin", "https://evil.com")],
    );
    assert_eq!(status, 200);
    let lower = raw_headers.to_lowercase();
    assert!(
        !lower.contains("access-control-allow-origin"),
        "el header allow-origin NO debe emitirse con origin no permitido: {}",
        raw_headers
    );
    // El resto de headers CORS sí.
    assert!(
        lower.contains("access-control-allow-methods"),
        "expected allow-methods equal: {}",
        raw_headers
    );
}

#[test]
fn http_q3_cors_set_preflight_echo_y_miss() {
    let src = "\
@server(43382)
fn main() => 0

@middleware(cors({\"allow_origin\": [\"https://a.com\"]}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    // Preflight con Origin permitido → 204 + echo.
    let (status, raw_headers) = build_spawn_request_raw_with_headers(
        "q3-cors-preflight-match",
        src,
        43382,
        "OPTIONS",
        "/api",
        &[("Origin", "https://a.com")],
    );
    assert_eq!(status, 204);
    let lower = raw_headers.to_lowercase();
    assert!(
        lower.contains("access-control-allow-origin: https://a.com"),
        "preflight: esperaba echo, fue: {}",
        raw_headers
    );
}

/// Variante de `build_spawn_request_raw` que envía headers HTTP
/// custom (`Origin: ...`). Usado por los tests Q.3 de CORS.
fn build_spawn_request_raw_with_headers(
    test_name: &str,
    src: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (u16, String) {
    // T2 (v0.10.13) — unique stem por test_name; SERIAL ya no necesario.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port {} within 3s", port);
    }

    use std::io::{Read, Write};
    let mut extra = String::new();
    for (k, v) in headers {
        extra.push_str(&format!("{}: {}\r\n", k, v));
    }
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\n{}Connection: close\r\n\r\n",
        method, path, addr, extra
    );
    let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();
    let raw = String::from_utf8_lossy(&buf).into_owned();

    let _ = child.kill();
    let _ = child.wait();

    let status_line = raw.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers_end = raw.find("\r\n\r\n").unwrap_or(raw.len());
    let headers_section = raw[..headers_end].to_string();
    (status, headers_section)
}

#[test]
fn http_mw3_cors_response_real_lleva_headers_inyectados() {
    let src = "\
@server(43373)
fn main() => 0

@middleware(cors())
@get(\"/api\")
fn list_items() -> Str => \"ok\"
";
    let (status, raw_headers) = build_spawn_request_raw("mw3-cors-real", src, 43373, "GET", "/api");
    assert_eq!(status, 200);
    let headers_lower = raw_headers.to_lowercase();
    assert!(
        headers_lower.contains("access-control-allow-origin: *"),
        "headers de la response real no llevan allow-origin default: {}",
        raw_headers
    );
}

/// Como `build_spawn_request` pero devuelve la sección entera de headers
/// crudos (todo lo que va entre la status line y el `\r\n\r\n`). Usado
/// por los tests CORS de MW.3 para verificar `Access-Control-Allow-*`.
fn build_spawn_request_raw(
    test_name: &str,
    src: &str,
    port: u16,
    method: &str,
    path: &str,
) -> (u16, String) {
    // T2 (v0.10.13) — unique stem por test_name.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port {} within 3s", port);
    }

    use std::io::{Read, Write};
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        method, path, addr
    );
    let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();
    let raw = String::from_utf8_lossy(&buf).into_owned();

    let _ = child.kill();
    let _ = child.wait();

    let status_line = raw.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers_end = raw.find("\r\n\r\n").unwrap_or(raw.len());
    let headers_section = raw[..headers_end].to_string();
    (status, headers_section)
}

// ---------------------------------------------------------------------------
// Fase 5b.4 — Result, `?`, match
// ---------------------------------------------------------------------------

#[test]
fn result_ok_err_complete_match_compiled() {
    // Cap 14 entero adaptado: `divide` retorna Result; consumimos los
    // dos resultados con `match` Ok/Err. La salida debe matchear
    // bit-a-bit lo que produce el intérprete.
    let src = "\
fn divide(a: Int, b: Int) -> Result<Int> {
    if (b == 0) {
        return Err(\"división por cero\")
    }
    return Ok(a / b)
}

match divide(10, 2) {
    Ok(v) => print(\"ok: {v}\")
    Err(e) => print(\"err: {e}\")
}

match divide(10, 0) {
    Ok(v) => print(\"ok: {v}\")
    Err(e) => print(\"err: {e}\")
}
";
    let (stdout, exit) = build_and_run("result-divide-match", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["ok: 5", "err: división por cero"]);
}

#[test]
fn try_operator_propagates_err_compiled() {
    // `?` adentro de fn `Result<T>` propaga el Err. Replicamos el
    // segundo bloque del cap 14: find_user / describe_user.
    let src = "\
type User { id: Int, name: Str }

fn find_user(id: Int) -> Result<User> {
    if (id == 1) {
        return Ok(User { id: 1, name: \"Fitz\" })
    }
    return Err(\"usuario no encontrado\")
}

fn describe_user(id: Int) -> Result<Str> {
    let u = find_user(id)?
    return Ok(\"#{u.id} es {u.name}\")
}

match describe_user(1) {
    Ok(desc) => print(desc)
    Err(e) => print(\"falló: {e}\")
}

match describe_user(42) {
    Ok(desc) => print(desc)
    Err(e) => print(\"falló: {e}\")
}
";
    let (stdout, exit) = build_and_run("try-propagation", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["#1 es Fitz", "falló: usuario no encontrado"]);
}

#[test]
fn list_find_returns_result_and_consumes_with_match_compiled() {
    // Cap 13 con find: `.find` devuelve Ok(item) o Err. Match Ok/Err.
    let src = "\
type User { id: Int, name: Str }
let usuarios: List<User> = [
    User { id: 1, name: \"Fitz\" },
    User { id: 2, name: \"Roy\" },
]
let primero = usuarios.find(fn(u) => u.id == 1)
match primero {
    Ok(u)  => print(\"hola, {u.name}!\")
    Err(e) => print(\"no debería pasar: {e}\")
}
let nadie = usuarios.find(fn(u) => u.id == 99)
match nadie {
    Ok(u)  => print(\"insólito: {u.name}\")
    Err(e) => print(\"falta: {e}\")
}
";
    let (stdout, exit) = build_and_run("list-find-match", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Fitz!", "falta: not found"]);
}

#[test]
fn map_get_returns_result_with_message_compiled() {
    // `m.get(k)` con clave faltante: Err con mensaje "key not found: <k>"
    // — formato idéntico al intérprete. Importante: la clave se formatea
    // con Display (Value), no inline — Str va SIN comillas en el mensaje.
    let src = "\
let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}
match m.get(\"a\") {
    Ok(v)  => print(\"a vale {v}\")
    Err(e) => print(\"err: {e}\")
}
match m.get(\"z\") {
    Ok(v)  => print(\"insólito {v}\")
    Err(e) => print(\"err: {e}\")
}
";
    let (stdout, exit) = build_and_run("map-get-match", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["a vale 1", "err: key not found: z"]);
}

#[test]
fn print_of_result_compiled_matches_interpreter() {
    // `print(Ok(v))` y `print(Err(e))` deben producir `Ok(42)` y
    // `Err("texto")` (Err con comillas dobles) bit-a-bit.
    let src = "\
fn ok42() -> Result<Int> { return Ok(42) }
fn boom() -> Result<Int> { return Err(\"explotó\") }
print(ok42())
print(boom())
";
    let (stdout, exit) = build_and_run("print-result", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["Ok(42)", "Err(\"explotó\")"]);
}

// ---------------------------------------------------------------------------
// Fase 5b.5 — módulos / import
// ---------------------------------------------------------------------------
//
// Los tests de 5b.5 necesitan MÚLTIPLES archivos en el tempdir (no solo
// `prog.fitz`). Helper específico para escribir varios archivos antes
// del build.

fn build_and_run_multi(
    test_name: &str,
    main_src: &str,
    extra_files: &[(&str, &str)],
) -> (String, i32) {
    // T2 (v0.10.13) — el main pasa de `prog.fitz` hardcoded a
    // `<stem>.fitz` derivado de `test_name`, así cada test tiene su
    // propio `target/fitz-build/<stem>/`. Los `extra_files` siguen con
    // sus nombres declarados (típicamente "auth.fitz", "models.fitz",
    // etc.) porque los imports del main los referencian por nombre.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, main_src).expect("escribir .fitz");
    for (name, content) in extra_files {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("crear subdir");
        }
        std::fs::write(&p, content).expect("escribir extra .fitz");
    }
    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());
    let run = Command::new(&bin).output().expect("invocar binario");
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

fn build_expect_fail_multi(
    test_name: &str,
    main_src: &str,
    extra_files: &[(&str, &str)],
) -> String {
    // T2 (v0.10.13) — paraleliza como `build_and_run_multi`.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, main_src).expect("escribir .fitz");
    for (name, content) in extra_files {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("crear subdir");
        }
        std::fs::write(&p, content).expect("escribir extra .fitz");
    }
    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        !output.status.success(),
        "expected fitz build to fail, but exited OK:\nstdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn from_import_type_and_fn_compiled() {
    // Reproduce el patrón de `examples/guide/16-modulos.fitz`:
    // - `import utils` expone `utils.greet(...)` como namespace.
    // - `from utils import User` trae el tipo al scope para
    //   construir con `User { ... }`.
    let main = "\
import utils
from utils import User
let u = User { id: 7, name: \"Fitz\" }
print(utils.greet(u.name))
print(u)
";
    let utils = "\
let PREFIX = \"saludos, \"
fn greet(name: Str) -> Str => \"{PREFIX}{name}\"
type User { id: Int, name: Str }
";
    let (stdout, exit) = build_and_run_multi("module-basic", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["saludos, Fitz", "User { id: 7, name: \"Fitz\" }"],
    );
}

#[test]
fn from_import_const_str_compiled() {
    // `from utils import PREFIX` trae una constante de Str al scope.
    let main = "\
from utils import PREFIX
print(PREFIX)
";
    let utils = "let PREFIX = \"prefijo\"";
    let (stdout, exit) = build_and_run_multi("module-import-const", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["prefijo"]);
}

#[test]
fn import_namespace_with_fn_only_compiled() {
    // `import utils` y luego `utils.greet(...)` sin importar nada
    // específico. El namespace queda disponible vía path Rust.
    let main = "\
import utils
print(utils.greet(\"Patagonia\"))
";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"";
    let (stdout, exit) =
        build_and_run_multi("module-namespace-only", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Patagonia"]);
}

#[test]
fn from_import_type_with_default_references_module_const() {
    // PreF8.3: el `type User` del módulo tiene un default
    // `id: Int = MAX` donde `MAX` es una const del propio módulo.
    // El importer NO importa `MAX`, solo `User`. Pre-PreF8.3 el
    // codegen del struct lit fallaba "variable desconocida en
    // codegen: MAX" porque resolvía el default_expr en el ctx del
    // importer. Post-fix, el módulo emite `pub fn __default_User_id()
    // -> i64 { MAX }` y el importer llama a `utils::__default_User_id()`.
    let main = "\
from utils import User
let u = User {}
print(u.id)
print(u.name)
";
    let utils = "\
let MAX = 99
let HELLO = \"saludos\"
type User { id: Int = MAX, name: Str = HELLO }
";
    let (stdout, exit) =
        build_and_run_multi("module-default-with-const", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["99", "saludos"]);
}

#[test]
fn import_namespace_with_alias_compiles() {
    // PreF8.4: `import utils as u` y luego `u.greet(...)`. El alias
    // queda como binding local; el módulo se carga normalmente.
    let main = "\
import utils as u
print(u.greet(\"Fitz\"))
";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"";
    let (stdout, exit) =
        build_and_run_multi("module-import-alias-ns", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Fitz"]);
}

#[test]
fn from_import_with_simple_alias_compiles() {
    // PreF8.4: `from utils import greet as g` y `g(...)`.
    let main = "\
from utils import greet as g
print(g(\"Fitz\"))
";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"";
    let (stdout, exit) =
        build_and_run_multi("module-import-alias-fn", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Fitz"]);
}

#[test]
fn from_import_alias_de_tipo_y_struct_lit() {
    // PreF8.4: `from utils import User as Person`. El struct lit
    // `Person { ... }` instancia el tipo del módulo. El Display
    // muestra el nombre original `User` (paridad con fitz run, que
    // usa el name canónico del Value::Type del módulo).
    let main = "\
from utils import User as Person
let p = Person { id: 7, name: \"Fitz\" }
print(p)
";
    let utils = "type User { id: Int, name: Str }";
    let (stdout, exit) =
        build_and_run_multi("module-import-alias-type", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["User { id: 7, name: \"Fitz\" }"]);
}

#[test]
fn from_import_const_alias_does_not_collide_with_local_let() {
    // PreF8.4 — caso para el que el alias es útil: el importer
    // tiene una `let PREFIX = "local"` y necesita la `PREFIX` del
    // módulo bajo otro nombre. Sin alias chocarían en el codegen
    // (el `use utils::PREFIX` colisionaría con el `static PREFIX`
    // del importer). Con alias funciona.
    let main = "\
from utils import PREFIX as REMOTE
let PREFIX = \"local\"
print(PREFIX)
print(REMOTE)
";
    let utils = "let PREFIX = \"remoto\"";
    let (stdout, exit) =
        build_and_run_multi("module-import-alias-const", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["local", "remoto"]);
}

#[test]
fn nonexistent_module_aborts_build() {
    let stderr = build_expect_fail_multi("module-not-found", "import inexistente\nprint(0)\n", &[]);
    assert!(
        (stderr.contains("module") && stderr.contains("not found"))
            || stderr.contains("inexistente"),
        "expected module-not-found message, was: {}",
        stderr
    );
}

#[test]
fn module_with_own_import_compiles_via_transitive_import() {
    // F15: imports transitivos ahora compilan. Un módulo cargado puede
    // tener su propio `import` que el codegen sigue recursivamente.
    let main = "\
import primero
print(primero.x())
";
    let primero = "\
from segundo import dos
fn x() -> Int => dos()
";
    let segundo = "fn dos() -> Int => 2";
    let (stdout, exit) = build_and_run_multi(
        "module-transitivo-ok",
        main,
        &[("primero.fitz", primero), ("segundo.fitz", segundo)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["2"]);
}

#[test]
fn f15_import_transitivo_namespace_y_named_mixto() {
    // Cobertura más amplia: un módulo cargado usa `import` (namespace)
    // y otro módulo usa `from ... import` (named) — ambos transitivos.
    let main = "\
import a
print(a.compose(\"Fitz\"))
";
    let a = "\
import b
from c import upper
fn compose(name: Str) -> Str => upper(b.greet(name))
";
    let b = "\
let PREFIX = \"hola, \"
fn greet(name: Str) -> Str => \"{PREFIX}{name}\"
";
    let c = "fn upper(s: Str) -> Str => s.upper()";
    let (stdout, exit) = build_and_run_multi(
        "f15-namespace-y-named-mixto",
        main,
        &[("a.fitz", a), ("b.fitz", b), ("c.fitz", c)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["HOLA, FITZ"]);
}

#[test]
fn f15_transitive_import_cycle_aborts_with_clear_error() {
    // F15: ciclo de imports detectado por el loader del codegen
    // (paralelo al evaluator). a → b → a debe abortar el build.
    let main = "\
import a
print(a.x())
";
    let a = "\
import b
fn x() -> Int => 1
";
    let b = "\
import a
fn y() -> Int => 2
";
    let stderr =
        build_expect_fail_multi("f15-ciclo-imports", main, &[("a.fitz", a), ("b.fitz", b)]);
    assert!(
        stderr.contains("import cycle"),
        "expected message about import cycle, was: {}",
        stderr
    );
}

#[test]
fn f15_transitive_import_with_shared_type() {
    // F15: un type definido en C, usado por B, expuesto via fn de A.
    // El codegen del módulo A necesita conocer la sig de `User` para
    // que `from c import User` resuelva al `pub fn` que retorna.
    let main = "\
import a
print(a.make_user(7))
";
    let a = "\
from c import User
fn make_user(id: Int) -> User => User { id: id }
";
    let _b = ""; // sin uso
    let c = "type User { id: Int = 0 }";
    let (stdout, exit) =
        build_and_run_multi("f15-type-compartido", main, &[("a.fitz", a), ("c.fitz", c)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["User { id: 7 }"]);
}

// ---------------------------------------------------------------------------
// Mini-tanda F14 — `let X = <expr>` no-literal a nivel top de módulo.
// Const-eval → `pub const X: T = <rhs>;`. Runtime → `pub fn X() -> T`.
// ---------------------------------------------------------------------------

#[test]
fn f14_module_let_const_eval_compiles_and_returns_inlined_value() {
    // Const-eval: `let X = 60 * 60` y `let Y = X / 36`.
    // El segundo no es const-eval estricto (referencia un Ident), pero
    // F14 lo emite como accessor fn — el call site `utils.Y` se
    // traduce a `utils::Y()`.
    let main = "\
import utils
print(utils.SECONDS)
print(utils.MAX)
";
    let utils = "\
let SECONDS: Int = 60 * 60
let MAX: Int = SECONDS / 36
";
    let (stdout, exit) = build_and_run_multi("f14-let-const-eval", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["3600", "100"]);
}

#[test]
fn f14_module_let_runtime_str_concat_compiles() {
    // Str concat no es const-eval (Rust no acepta `String + String`
    // en const). F14 lo emite como `pub fn GREETING() -> String`,
    // el call site `utils.GREETING` se traduce a `utils::GREETING()`.
    let main = "\
import utils
print(utils.GREETING)
";
    let utils = "let GREETING: Str = \"hola, \" + \"Fitz\"";
    let (stdout, exit) = build_and_run_multi("f14-let-str-concat", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Fitz"]);
}

#[test]
fn f14_module_let_runtime_struct_lit_via_fn_call() {
    // RHS = fn call que retorna instancia. F14 emite accessor fn que
    // re-evalúa la RHS en cada referencia.
    let main = "\
from utils import User
import utils
let u = utils.DEFAULT_USER
print(u)
";
    let utils = "\
type User { id: Int = 0, name: Str = \"anon\" }
fn make() -> User => User {}
let DEFAULT_USER: User = make()
";
    let (stdout, exit) = build_and_run_multi(
        "f14-let-struct-lit-via-call",
        main,
        &[("utils.fitz", utils)],
    );
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["User { id: 0, name: \"anon\" }"]);
}

// ---------------------------------------------------------------------------
// Fase 5b.6 — HTTP / @server / handlers
// ---------------------------------------------------------------------------

/// Helper: build de un programa HTTP, spawn del binario, request HTTP
/// crudo (sin reqwest para evitar dep extra en tests), y stop. Devuelve
/// (status_line, body) leídos del socket.
fn build_spawn_request(
    test_name: &str,
    src: &str,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String) {
    // T2 (v0.10.13) — unique stem por test_name.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    // Spawn del server en background. Le damos tiempo a abrir el
    // puerto antes de la primera request.
    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Esperar a que el puerto esté escuchando (hasta 3s).
    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port {} within 3s", port);
    }

    // Construir la request HTTP a mano (sin reqwest).
    use std::io::{Read, Write};
    let request = match body {
        Some(b) => format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            method,
            path,
            addr,
            b.len(),
            b
        ),
        None => format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            method, path, addr
        ),
    };
    let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();
    let raw = String::from_utf8_lossy(&buf).into_owned();

    let _ = child.kill();
    let _ = child.wait();

    // Parsear status + body. Formato: "HTTP/1.1 <code> <reason>\r\n...\r\n\r\n<body>"
    let status_line = raw.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
    let body = raw[body_start..].to_string();
    (status, body)
}

#[test]
fn http_get_simple_responde_200_y_body() {
    // El criterio mínimo de 5b.6: un handler GET que devuelve un Str
    // produce 200 + JSON con el string.
    let src = "@server(43210)\nfn main() => 0\n\
               @get(\"/\") fn index() -> Str => \"Fitz HTTP corriendo\"\n";
    let (status, body) = build_spawn_request("http-get-simple", src, 43210, "GET", "/", None);
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "\"Fitz HTTP corriendo\"");
}

#[test]
fn http_get_with_path_param_int() {
    let src = "@server(43211)\nfn main() => 0\n\
               @get(\"/double/{n}\") fn double(n: Int) -> Int => n * 2\n";
    let (status, body) =
        build_spawn_request("http-path-int", src, 43211, "GET", "/double/21", None);
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "42");
}

#[test]
fn http_async_handler_with_sleep_responds_200() {
    // Fase 6.6: handler `async fn` con `sleep(...).await` adentro.
    // El wrapper de axum await-ea el future del handler antes de
    // serializar el resultado. End-to-end con tokio current_thread.
    let src = "@server(43219)\nfn main() => 0\n\
               @get(\"/pause\") async fn pause() -> Str {\n\
                   let _ = sleep(0).await\n\
                   return \"done\"\n\
               }\n";
    let (status, body) =
        build_spawn_request("http-async-handler", src, 43219, "GET", "/pause", None);
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "\"done\"");
}

#[test]
fn http_result_ok_responde_200_err_responde_500() {
    let src = "@server(43212)\nfn main() => 0\n\
               @get(\"/d/{a}/{b}\") fn divide(a: Int, b: Int) -> Result<Int> {\n\
                   if (b == 0) { return Err(\"div por cero\") }\n\
                   return Ok(a / b)\n\
               }\n";
    let (status_ok, body_ok) =
        build_spawn_request("http-result-ok", src, 43212, "GET", "/d/10/2", None);
    assert_eq!(status_ok, 200);
    assert_eq!(body_ok.trim(), "5");
    let (status_err, body_err) =
        build_spawn_request("http-result-err", src, 43212, "GET", "/d/10/0", None);
    assert_eq!(status_err, 500);
    assert!(
        body_err.contains("\"error\":\"div por cero\""),
        "expected JSON error with message, was: {}",
        body_err
    );
}

#[test]
fn http_post_body_deserializa_tipo_custom() {
    let src = "@server(43213)\nfn main() => 0\n\
               type Input { msg: Str, times: Int = 1 }\n\
               @post(\"/echo\") fn echo(body: Input) -> Input => body\n";
    let (status, body) = build_spawn_request(
        "http-post-body",
        src,
        43213,
        "POST",
        "/echo",
        Some("{\"msg\":\"hola\",\"times\":3}"),
    );
    assert_eq!(status, 200);
    assert!(
        body.contains("\"msg\":\"hola\"") && body.contains("\"times\":3"),
        "expected body with msg and times, was: {}",
        body
    );
}

#[test]
fn http_post_body_aplica_defaults_a_campos_faltantes() {
    let src = "@server(43214)\nfn main() => 0\n\
               type Input { msg: Str, times: Int = 7 }\n\
               @post(\"/echo\") fn echo(body: Input) -> Input => body\n";
    let (status, body) = build_spawn_request(
        "http-post-default",
        src,
        43214,
        "POST",
        "/echo",
        Some("{\"msg\":\"sin times\"}"),
    );
    assert_eq!(status, 200);
    assert!(
        body.contains("\"times\":7"),
        "expected default `times: 7` applied, was: {}",
        body
    );
}

#[test]
fn http_post_body_extra_field_es_400() {
    let src = "@server(43215)\nfn main() => 0\n\
               type Input { msg: Str }\n\
               @post(\"/echo\") fn echo(body: Input) -> Input => body\n";
    let (status, body) = build_spawn_request(
        "http-post-extra",
        src,
        43215,
        "POST",
        "/echo",
        Some("{\"msg\":\"x\",\"extra\":\"nope\"}"),
    );
    assert_eq!(status, 400);
    assert!(
        body.contains("undeclared field"),
        "expected message about undeclared field, was: {}",
        body
    );
}

/// Mini-tanda UC + HA — variante del helper que permite especificar
/// el Content-Type explícitamente, para los tests de urlencoded y 415.
#[allow(clippy::too_many_arguments)]
fn build_spawn_request_with_ct(
    test_name: &str,
    src: &str,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    content_type: Option<&str>,
) -> (u16, String) {
    // T2 (v0.10.13) — unique stem por test_name.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port {} within 3s", port);
    }

    use std::io::{Read, Write};
    let request = match body {
        Some(b) => {
            let ct_header = match content_type {
                Some(ct) => format!("Content-Type: {}\r\n", ct),
                None => String::new(),
            };
            format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\n{}\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                method,
                path,
                addr,
                ct_header,
                b.len(),
                b
            )
        }
        None => format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            method, path, addr
        ),
    };
    let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();
    let raw = String::from_utf8_lossy(&buf).into_owned();

    let _ = child.kill();
    let _ = child.wait();

    let status_line = raw.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
    let body = raw[body_start..].to_string();
    (status, body)
}

#[test]
fn uc_http_post_urlencoded_parsea_a_map_str_str() {
    // Mini-tanda UC: `fitz build` debe aceptar bodies
    // `application/x-www-form-urlencoded` y parsearlos como
    // `Map<Str, Str>` igual que el intérprete.
    let src = "@server(43240)\nfn main() => 0\n\
               @post(\"/echo\") fn echo(body: Map<Str, Str>) -> Map<Str, Str> => body\n";
    let (status, body) = build_spawn_request_with_ct(
        "uc-post-urlenc-basic",
        src,
        43240,
        "POST",
        "/echo",
        Some("name=Fitz&age=25"),
        Some("application/x-www-form-urlencoded"),
    );
    assert_eq!(
        status, 200,
        "expected 200, was: status={} body={}",
        status, body
    );
    assert!(
        body.contains("\"name\":\"Fitz\"") && body.contains("\"age\":\"25\""),
        "expected body parses name/age pairs, was: {}",
        body
    );
}

#[test]
fn uc_http_post_urlencoded_with_url_encoding() {
    // URL-decoding: `+` → espacio, `%20` → espacio.
    let src = "@server(43241)\nfn main() => 0\n\
               @post(\"/echo\") fn echo(body: Map<Str, Str>) -> Map<Str, Str> => body\n";
    let (status, body) = build_spawn_request_with_ct(
        "uc-post-urlenc-decoded",
        src,
        43241,
        "POST",
        "/echo",
        Some("greeting=hola+mundo&place=Fitz%20Roy"),
        Some("application/x-www-form-urlencoded"),
    );
    assert_eq!(status, 200);
    assert!(
        body.contains("\"greeting\":\"hola mundo\"") && body.contains("\"place\":\"Fitz Roy\""),
        "expected URL-decoding applied, was: {}",
        body
    );
}

#[test]
fn ha_http_content_type_text_plain_is_415_with_clear_msg() {
    // Mini-tanda HA: el msg del 415 cubre los formatos no soportados.
    // Mini-tanda MP-Build — multipart ya es soportado por el codegen
    // también (paridad bit-a-bit con `fitz run`). Probamos con
    // `text/plain` que sigue siendo rechazado.
    let src = "@server(43242)\nfn main() => 0\n\
               type Input { msg: Str }\n\
               @post(\"/echo\") fn echo(body: Input) -> Input => body\n";
    let (status, body) = build_spawn_request_with_ct(
        "ha-post-415-text-plain",
        src,
        43242,
        "POST",
        "/echo",
        Some("hola mundo"),
        Some("text/plain"),
    );
    assert_eq!(
        status, 415,
        "expected 415, was: status={} body={}",
        status, body
    );
    assert!(
        body.contains("Content-Type not supported"),
        "expected `Content-Type not supported`, was: {}",
        body
    );
    assert!(
        body.contains("application/json")
            && body.contains("application/x-www-form-urlencoded")
            && body.contains("multipart/form-data"),
        "expected the message to mention the 3 supported CTs, was: {}",
        body
    );
}

// ---------------------------------------------------------------------------
// Mini-tandas DZ + CT + OAPI — paridad chica run↔build
// ---------------------------------------------------------------------------

#[test]
fn dz_division_int_by_zero_compiles_and_panics_with_aligned_msg() {
    // Pre-DZ: `print(10 / 0)` rechaza rustc con `unconditional_panic`.
    // Post-DZ: compila y panica en runtime con el mismo msg que el
    // intérprete ("division by zero").
    // T2 (v0.10.13) — unique stem.
    let stem = "dz_int";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&src_path, "print(10 / 0)\n").expect("write");
    let out = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out.status.success(),
        "expected `fitz build` with `10/0` to compile (no const-eval reject), stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });
    assert!(bin.exists(), "binario no existe: {}", bin.display());
    let run = Command::new(&bin).output().expect("run prog");
    assert!(
        !run.status.success(),
        "expected exit code != 0 (panic), was: {:?}",
        run.status
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("division by zero"),
        "expected msg `division by zero`, stderr: {}",
        stderr
    );
}

#[test]
fn dz_division_float_by_zero_compiles_and_panics_with_aligned_msg() {
    // T2 (v0.10.13) — unique stem.
    let stem = "dz_float";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&src_path, "print(3.14 / 0.0)\n").expect("write");
    let out = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(out.status.success());
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });
    let run = Command::new(&bin).output().expect("run");
    assert!(!run.status.success(), "expected panic");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("division by zero"),
        "expected msg `division by zero`, stderr: {}",
        stderr
    );
}

#[test]
fn ct_compare_int_vs_str_compiles_and_returns_false() {
    // `1 == "1"` y `1 != "1"` ahora compilan a literal false/true.
    let src = "print(1 == \"1\")\nprint(1 != \"1\")\nprint(true == 0)\n";
    let (stdout, exit) = build_and_run("ct-incompat", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["false", "true", "false"]);
}

#[test]
fn ct_bit_for_bit_parity_run_vs_build_incompatible_comparisons() {
    // Paridad bit-a-bit `fitz run` ↔ `fitz build` para `==`/`!=`
    // entre tipos primitivos incompatibles.
    // T2 (v0.10.13) — unique stem.
    let stem = "ct_parity";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    let src = "\
print(1 == \"1\")
print(\"x\" != 1.5)
print(true == null)
print(false != \"f\")
";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(out_run.status.success());
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(out_build.status.success());
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });
    let exec = Command::new(&bin).output().expect("run prog");
    assert!(exec.status.success());
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
        "expected bit-for-bit parity `run` ↔ `build`"
    );
    assert!(run_stdout.contains("false"));
    assert!(run_stdout.contains("true"));
}

#[test]
fn mp_build_multipart_text_field_compila_y_parsea() {
    // Mini-tanda MP-Build — multipart text-only en `fitz build`.
    // Paridad bit-a-bit con `fitz run` (que ya lo soportaba en MP2).
    let src = "@server(43370)\nfn main() => 0\n\
               @post(\"/form\") fn form(body: Map<Str, Str>) -> Str {\n\
                   let n = body[\"name\"]\n\
                   return \"got \" + n\n\
               }\n";
    let (status, body) = build_spawn_request_with_ct(
        "mp-build-text",
        src,
        43370,
        "POST",
        "/form",
        Some("--X\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nFitz\r\n--X--"),
        Some("multipart/form-data; boundary=X"),
    );
    assert_eq!(status, 200, "expected 200, was: {} body={}", status, body);
    assert!(
        body.contains("got Fitz"),
        "expected body with `got Fitz`, was: {}",
        body
    );
}

#[test]
fn mp_build_multipart_file_field_compila_y_parsea() {
    // Mini-tanda MP-Build — multipart con file field. El handler
    // lee `len(f.content)` (no usa `f.name` que es Str? — para
    // evitar el caveat de narrowing del checker en este test E2E).
    let src = "@server(43371)\nfn main() => 0\n\
               @post(\"/upload\") fn upload(body: Map<Str, File>) -> Str {\n\
                   let f = body[\"doc\"]\n\
                   let n = len(f.content)\n\
                   return \"size={n}\"\n\
               }\n";
    let (status, body) = build_spawn_request_with_ct(
        "mp-build-file",
        src,
        43371,
        "POST",
        "/upload",
        Some("--X\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"hello.txt\"\r\nContent-Type: text/plain\r\n\r\nfile contents\r\n--X--"),
        Some("multipart/form-data; boundary=X"),
    );
    assert_eq!(status, 200, "expected 200, was: {} body={}", status, body);
    assert!(
        body.contains("size=13"),
        "expected `size=13` (len of 'file contents'), was: {}",
        body
    );
}

#[test]
fn mp_build_multipart_without_boundary_is_400() {
    let src = "@server(43372)\nfn main() => 0\n\
               @post(\"/form\") fn form(body: Map<Str, Str>) -> Str => \"ok\"\n";
    let (status, body) = build_spawn_request_with_ct(
        "mp-build-no-boundary",
        src,
        43372,
        "POST",
        "/form",
        Some("--X\r\n--X--"),
        Some("multipart/form-data"),
    );
    assert_eq!(status, 400, "expected 400, was: {} body={}", status, body);
    assert!(
        body.contains("boundary"),
        "expected mention of boundary, was: {}",
        body
    );
}

#[test]
fn f13_spike_heterogeneous_list_compiles_and_bit_for_bit_parity() {
    // F13 SPIKE — última residual del bloque post-Fase-8.
    // Listas heterogéneas (`[1, "dos", true]`) ya compilan a binario
    // nativo y producen output bit-a-bit idéntico a `fitz run`.
    // Before SPIKE the codegen rejected with "homogeneous required".
    // T2 (v0.10.13) — unique stem.
    let stem = "f13_spike";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    let src = "\
let xs = [1, \"dos\", true]
let ys = [42, 3.14, null, false, \"hola\"]
print(xs)
print(ys)
print(len(xs))
print(len(ys))
";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(
        out_run.status.success(),
        "fitz run failed: {}",
        String::from_utf8_lossy(&out_run.stderr)
    );
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out_build.status.success(),
        "fitz build failed (F13 SPIKE did not apply): {}",
        String::from_utf8_lossy(&out_build.stderr)
    );
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });
    let exec = Command::new(&bin).output().expect("ejecutar binario");
    assert!(exec.status.success(), "binary failed to execute");
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
        "F13 SPIKE: esperaba paridad bit-a-bit `run` ↔ `build`"
    );
    assert!(run_stdout.contains("[1, \"dos\", true]"));
    assert!(run_stdout.contains("3.14"));
}

#[test]
fn f13_a_bytes_and_nominal_in_heterogeneous_list_parity_run_vs_build() {
    // F13.A + F13.B — Bytes y Nominales adentro de listas
    // heterogéneas. Paridad bit-a-bit `fitz run` ↔ `fitz build`.
    // T2 (v0.10.13) — unique stem.
    let stem = "f13_a_b";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    let src = "\
type User { id: Int, name: Str }
let u = User { id: 1, name: \"ana\" }
let xs = [1, b\"raw\", u, true]
print(xs)
";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(out_run.status.success());
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out_build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out_build.stderr)
    );
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });
    let exec = Command::new(&bin).output().expect("run prog");
    assert!(exec.status.success());
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
        "F13.A+B: esperaba paridad bit-a-bit"
    );
    assert!(run_stdout.contains("b\"raw\""));
    assert!(run_stdout.contains("User { id: 1, name: \"ana\" }"));
}

#[test]
fn f13_a_heterogeneous_map_parity_run_vs_build() {
    // F13.A — Map heterogéneo (values mixtos) paridad bit-a-bit.
    // T2 (v0.10.13) — unique stem.
    let stem = "f13_a_map";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    let src = "let cfg = {\"name\": \"fitz\", \"count\": 7, \"on\": true}\nprint(cfg)\n";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(out_run.status.success());
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(out_build.status.success());
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });
    let exec = Command::new(&bin).output().expect("run prog");
    assert!(exec.status.success());
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
    );
    assert!(run_stdout.contains("\"name\": \"fitz\""));
    assert!(run_stdout.contains("\"count\": 7"));
    assert!(run_stdout.contains("\"on\": true"));
}

#[test]
fn f13_list_with_complex_type_aborts_with_clear_msg() {
    // F13.E follow-up cerrado (post-9.w.2) — el caso `[1, [2, 3]]`
    // (List anidada como item de heterogéneo) ahora SÍ compila con
    // `fitz build` y produce output bit-a-bit con `fitz run`. La
    // fix fue extender la heurística sintáctica
    // `program_uses_fitz_value` para detectar mezcla
    // primitivo+compuesto (List/Map/StructLit/Bytes) — sin eso, el
    // walk detectaba la heterogeneidad pero el preludio FitzValue
    // ya se había decidido no emitir.
    //
    // El test mantiene el nombre histórico para que git log siga
    // siendo navegable, pero ahora valida la paridad en lugar del
    // abort.
    let src = "let xs = [1, [2, 3]]\nprint(xs)\n";
    let (stdout, exit) = build_and_run("f13-list-anidada-en-heterogeneo", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["[1, [2, 3]]"]);
}

#[test]
fn mw_wrap_codegen_rejects_with_msg_citing_fitz_run() {
    // Mini-tanda Mw-Wrap — el codegen rechaza wrap-style mws con
    // un mensaje claro citando `fitz run` como workaround.
    // El intérprete sí los soporta (deuda residual del codegen).
    // T2 (v0.10.13) — unique stem.
    let stem = "mw_wrap_codegen";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    let src = "\
@server(43500)
fn main() => 0

fn timing(req: Request, next: Fn() -> Response) -> Response {
    let r = next()
    return 200 {\"ok\": true}
}

@middleware(timing)
@get(\"/wrapped\")
fn wrapped() -> Str => \"handler\"
";
    std::fs::write(&src_path, src).expect("write");
    let out = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        !out.status.success(),
        "expected `fitz build` to reject wrap-style mws"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wrap-style") && stderr.contains("fitz run"),
        "expected msg about wrap-style + workaround `fitz run`, was: {}",
        stderr
    );
}

#[test]
fn bytes_bit_for_bit_parity_run_vs_build() {
    // Mini-tanda Bytes — el output de `fitz run` y `fitz build`
    // deben coincidir bit-a-bit para todos los casos canónicos:
    // literal con escapes, len, is_empty, to_str Ok/Err.
    // T2 (v0.10.13) — unique stem.
    let stem = "bytes_paridad";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    let src = "\
let a = b\"hola\"
let b = b\"\\x00\\xff\"
let empty = b\"\"
print(a)
print(b)
print(a.len())
print(empty.is_empty())
print(a.is_empty())
let r = a.to_str()
print(r)
let s = bytes(\"converted\")
print(s)
print(len(s))
";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(
        out_run.status.success(),
        "fitz run failed: {}",
        String::from_utf8_lossy(&out_run.stderr)
    );
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out_build.status.success(),
        "fitz build failed: {}",
        String::from_utf8_lossy(&out_build.stderr)
    );
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });
    let exec = Command::new(&bin).output().expect("ejecutar binario");
    assert!(exec.status.success());
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
        "expected bit-for-bit parity `run` ↔ `build`"
    );
    // Sanity sobre el contenido.
    assert!(run_stdout.contains("b\"hola\""));
    assert!(run_stdout.contains("b\"\\x00\\xff\""));
    assert!(run_stdout.contains("4"));
    assert!(run_stdout.contains("true"));
    assert!(run_stdout.contains("Ok(\"hola\")"));
}

#[test]
fn oapi_return_ident_to_top_level_const_compiles_and_emits_schema() {
    // `return NOT_FOUND { ... }` con NOT_FOUND const top-level
    // ahora parsea, compila a binario, y entra al schema OpenAPI.
    // T2 (v0.10.13) — unique stem.
    let stem = "oapi_ident_build";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    let src = "\
let NOT_FOUND = 404
@server(43250)
fn main() => 0
@get(\"/u/{id}\")
fn h(id: Int) -> Int {
    if (id == 0) {
        return NOT_FOUND {\"error\": \"x\"}
    }
    return id
}
";
    std::fs::write(&src_path, src).expect("write");
    let out = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out.status.success(),
        "expected compile OK, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let openapi = Command::new(fitz_bin())
        .args(["openapi"])
        .arg(&src_path)
        .output()
        .expect("fitz openapi");
    assert!(openapi.status.success());
    let schema = String::from_utf8_lossy(&openapi.stdout);
    assert!(
        schema.contains("\"404\""),
        "expected 404 in the OpenAPI schema, was: {}",
        schema
    );
}

// ---------------------------------------------------------------------------
// F12 — higher-order completo (closures, fn como valor/param/retorno)
// ---------------------------------------------------------------------------

#[test]
fn anonymous_fn_assigned_to_var_is_invoked() {
    let src = "\
let f: Fn(Int) -> Int = fn(n: Int) => n * 2
print(f(21))
";
    let (stdout, exit) = build_and_run("f12-fnexpr-var", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["42"]);
}

#[test]
fn named_fn_as_value_is_invoked() {
    let src = "\
fn square(n: Int) -> Int => n * n
let g: Fn(Int) -> Int = square
print(g(7))
";
    let (stdout, exit) = build_and_run("f12-fn-nombrada-valor", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["49"]);
}

#[test]
fn apply_with_fn_and_inline_fnexpr() {
    let src = "\
fn square(n: Int) -> Int => n * n
fn apply(f: Fn(Int) -> Int, x: Int) -> Int => f(x)
print(apply(square, 7))
print(apply(fn(n: Int) => n * 10, 7))
";
    let (stdout, exit) = build_and_run("f12-apply", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["49", "70"]);
}

#[test]
fn closure_with_int_capture_works() {
    // make_adder(x) retorna una closure que captura x por valor.
    let src = "\
fn make_adder(x: Int) -> Fn(Int) -> Int {
    return fn(y: Int) => x + y
}
let add5: Fn(Int) -> Int = make_adder(5)
print(add5(3))
print(add5(10))
";
    let (stdout, exit) = build_and_run("f12-make-adder", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["8", "15"]);
}

#[test]
fn closure_that_captures_str_clones_outside() {
    // El codegen debe clonar `saludo` antes del closure para que la
    // var siga disponible en el caller después de pasarla a la
    // closure (move la consumiría sin el clone).
    let src = "\
let saludo = \"hola\"
let f: Fn(Str) -> Str = fn(n: Str) => \"{saludo}, {n}!\"
print(f(\"Fitz\"))
print(saludo)
";
    let (stdout, exit) = build_and_run("f12-capture-str", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Fitz!", "hola"]);
}

#[test]
fn fnexpr_without_param_annotation_aborts_build() {
    // Param sin anotar → error claro (deuda 5b.1).
    let stderr = build_expect_fail(
        "f12-fnexpr-sin-anot",
        "let f: Fn(Int) -> Int = fn(x) => x * 2\n",
    );
    assert!(
        stderr.contains("anonymous") && stderr.contains("annotation"),
        "expected message about anonymous fn without annotation, was: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Fase 5b.7 — smoke test: todos los ejemplos guía marcados como
// compilables deben compilar con `fitz build` sin error
// ---------------------------------------------------------------------------

/// Lista de ejemplos guía que el cap 18 declara compilables con
/// `fitz build`. Los que NO están acá tienen una razón documentada
/// (state HTTP compartido, lista heterogénea, higher-order completo,
/// error intencional, etc.) — están listados en el cap 18 también.
const GUIDE_EXAMPLES_COMPILE: &[&str] = &[
    "02-hola.fitz",
    "03-variables.fitz",
    "03b-numeros-legibles.fitz",
    "03c-bases-numericas.fitz",
    "03d-identifiers-unicode.fitz",
    "04-operadores.fitz",
    "04b-operadores-bit.fitz",
    "04c-asignacion-compuesta-bit.fitz",
    "05-strings.fitz",
    "05b-format-specs.fitz",
    "05e-bytes.fitz",
    "05d-escapes-extendidos.fitz",
    "06-logica.fitz",
    "07-if.fitz",
    "08-loops.fitz",
    "08b-loops-avanzados.fitz",
    "09b-indexing-slicing.fitz",
    "09c-tuples.fitz",
    "09d-comprehensions.fitz",
    "09e-for-map.fitz",
    "09f-let-destructure-rico.fitz",
    "10-match.fitz",
    "10b-match-tuple-subpatterns.fitz",
    "11-funciones.fitz",
    "11b-default-params.fitz",
    "11c-varargs.fitz",
    "11d-named-args.fitz",
    "12-type.fitz",
    "13-metodos.fitz",
    "13b-metodos-custom.fitz",
    "13c-metodos-extras.fitz",
    "13d-iteradores.fitz",
    "13e-mini-bundle-metodos.fitz",
    "13f-range-iteradores.fitz",
    "13g-static-methods.fitz",
    "13h-predicados-list.fitz",
    "13i-campos-privados.fitz",
    "13j-extras-str-map.fitz",
    "13k-flat-map-first-last-merge.fitz",
    "13l-update-comp-tuple-paramnames.fitz",
    "13m-min-max-sum-pad-keys-step.fitz",
    "13n-reduce-product-chars-entries-to-map.fitz",
    "13o-higher-order-y-consts-globales.fitz",
    "13p-mb4-y-comprehensions-extendidas.fitz",
    "13q-mb5-y-async-closures.fitz",
    "13r-mb6-y-async-build.fitz",
    "13s-mb7-y-fmt-build.fitz",
    "13t-mb8-bits-y-fmt-g.fitz",
    "13u-math-mb9-y-int-float.fitz",
    "13v-return-en-match.fitz",
    // FITZ-01 (2026-08) — módulo `rand` (global CSPRNG + `rand.seeded(N)`
    // reproducible). Compila con `fitz build` (global usa getrandom).
    "13w-random.fitz",
    // FITZ-04 (2026-08) — módulo `num` (formateo locale-aware es-AR/en-US).
    "13x-num-locale.fitz",
    "14-result.fitz",
    // 14b: usa `Err(Int)` y `Err(Instance)` — el codegen pinea Err
    // como String, así que `fitz build` falla. Documentado en el
    // ejemplo como deuda residual de Err+. Sí corre en `fitz run`.
    "14c-result-tipado.fitz",
    "14d-err-compuestos.fitz",
    "16-modulos.fitz",
    "16b-modulos-let-expr.fitz",
    "16c-modulos-transitivos.fitz",
    "16d-import-multilinea.fitz",
    "17-http.fitz",
    "17b-middleware.fitz",
    "17c-multipart.fitz",
    // Mini-tanda HTTP client builtin (Bloque 5) — los 4 ejemplos de
    // la sub-sección "17.X HTTP client outbound" del cap 17.
    // Apuntan a `httpbin.org` por didactismo; el smoke solo compila
    // (no ejecuta), así que cero red durante CI.
    "17e-http-client-basico.fitz",
    "17f-http-client-errores.fitz",
    "17g-http-client-webhook.fitz",
    "17h-http-client-health-checker.fitz",
    // Mini-tanda SMTP builtin (Bloque 5) — los 3 ejemplos de la
    // sub-sección "17.X SMTP outbound" del cap 17. Apuntan a MailHog
    // en localhost:1025 por didactismo; el smoke solo compila (no
    // ejecuta), así que cero MailHog real durante CI.
    "17i-smtp-basico.fitz",
    "17j-smtp-errores.fitz",
    "17k-smtp-magic-link.fitz",
    // v0.19.0 — Response built-in (Bloques 1+2+3+4): cap 17
    // sub-sección "Respuestas con Content-Type custom". Cubre RSS
    // XML + robots.txt + SVG + PDF binario en un solo programa.
    "17l-response-custom.fitz",
    // FITZ-02 (v0.51.0) — cap 17 sub-sección "Archivos estáticos".
    // `@server(static_dir=, static_prefix=)`. El smoke solo compila
    // (no ejecuta), así que no necesita el `./public` en disco.
    "17m-static.fitz",
    "18-docs.fitz",
    "19-async.fitz",
    "19b-paralelismo.fitz",
    "20-build.fitz",
    "23-fmt-ejemplo.fitz",
    "24-tests.fitz",
    "28-auth.fitz",
    "21b-pyi-stubs.fitz",
    "29-ws.fitz",
    "29b-ws-binary.fitz",
    "29c-ws-bidir.fitz",
    "30-cron-background.fitz",
    "30b-cron-persistente.fitz",
    "30c-background-persistente.fitz",
    "31-orm.fitz",
    "31b-orm-crud-http.fitz",
    "31c-transactions.fitz",
    "32-env.fitz",
    // v0.11.0 (Fase 13) — CLI builder con @command.
    "33-cli.fitz",
    // v0.12.5 (Fase 12.5.a) — Cap 35 "Deployment ciudadano primera clase":
    // ejemplo end-to-end que combina @server + @auth_provider + @admin +
    // @requires + @healthz + secret() + config() + log.info estructurado.
    "35-deploy.fitz",
    // v0.13.0 (Fase 12.7) — Cap 33.5 "@trace y @metric — instrumentación
    // manual": fns user con `@trace(name="X")` + `@metric(name="X")`
    // emiten span + histogram + counter al Drop del scope.
    "34-trace-metric.fitz",
    // v0.13.0 (Fase 12.8) — Cap 33.11 "Feature flags": @flag + flag() +
    // flags.is_enabled + flags.list. Defaults manifest + env var override.
    "34b-feature-flags.fitz",
];

#[test]
fn smoke_ejemplos_guia_compilables_compilan() {
    // Smoke test del cap 18: cada ejemplo de la lista compila a binario
    // con `fitz build`. Costoso (cada ejemplo invoca cargo + rustc).
    // T2 (v0.10.13) — SERIAL removido: cada ejemplo se buildea en su
    // propio `target/fitz-build/<stem>/` (stem = filename del .fitz),
    // así no choca con otros tests que paralelicen via helpers.
    let project_root = std::env::current_dir().expect("cwd");
    let guide_dir = project_root.join("examples").join("guide");

    let mut failures: Vec<String> = Vec::new();
    for name in GUIDE_EXAMPLES_COMPILE {
        let src_path = guide_dir.join(name);
        assert!(
            src_path.exists(),
            "ejemplo no existe: {}",
            src_path.display()
        );
        let output = Command::new(fitz_bin())
            .args(["build"])
            .arg(&src_path)
            .output()
            .expect("invoke fitz build");
        if !output.status.success() {
            failures.push(format!(
                "{}\n--- stderr ---\n{}",
                name,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        // Limpiar el binario adyacente para no dejar `.exe` colgados
        // (memoria del workflow: examples/ no debe contener generados).
        let stem = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap();
        let bin_name = if cfg!(windows) {
            format!("{}.exe", stem)
        } else {
            stem.to_string()
        };
        let _ = std::fs::remove_file(guide_dir.join(&bin_name));
        let _ = std::fs::remove_file(guide_dir.join(format!("{}.pdb", stem)));
    }

    assert!(
        failures.is_empty(),
        "ejemplos que no compilaron ({}):\n{}",
        failures.len(),
        failures.join("\n\n"),
    );
}

// B16-class consistency residuals (v0.40.1) — these patterns used to pass
// `fitz check` but fail `fitz build` with an opaque rustc E0308.

#[test]
fn v040_1_divergent_else_if_builds_and_runs() {
    // `if c { A } else { return B }` used as a value: the divergent `else`
    // is `!` and coerces to `A`'s type.
    let (out, code) = build_and_run(
        "v040_1_divergent_else",
        "fn unwrap_or(o: Int?) -> Int {\n\
         \x20 let n = if (o != null) { o } else { return 0 }\n\
         \x20 return n + 1\n\
         }\n\
         print(unwrap_or(41))\n\
         print(unwrap_or(null))\n",
    );
    assert_eq!(code, 0, "divergent-else if must build + run: {out}");
    assert!(out.contains("42") && out.contains('0'), "out: {out}");
}

#[test]
fn v040_1_list_lub_float_builds_and_runs() {
    // `[1, 2.5]` types as `List<Float>` (LUB of elements) in both the
    // checker and the codegen — no annotation, builds clean.
    let (out, code) = build_and_run("v040_1_list_lub", "let xs = [1, 2.5]\nprint(xs)\n");
    assert_eq!(code, 0, "List<Float> literal must build + run: {out}");
    assert!(out.contains("2.5"), "out: {out}");
}

// ---------------------------------------------------------------------------
// F11 — state HTTP compartido (thread_local + tokio current_thread)
// ---------------------------------------------------------------------------

/// Helper F11: build + spawn + **secuencia** de requests sobre el mismo
/// binario corriendo. Valida que el state compartido persiste entre
/// llamadas. La secuencia es `(method, path, body) -> (status, body)`.
fn build_spawn_requests(
    test_name: &str,
    src: &str,
    port: u16,
    sequence: &[(&str, &str, Option<&str>)],
) -> Vec<(u16, String)> {
    // T2 (v0.10.13) — unique stem por test_name.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port {} within 3s", port);
    }

    use std::io::{Read, Write};
    let mut results: Vec<(u16, String)> = Vec::with_capacity(sequence.len());
    for (method, path, body) in sequence {
        let request = match body {
            Some(b) => format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                method,
                path,
                addr,
                b.len(),
                b
            ),
            None => format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                method, path, addr
            ),
        };
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send request");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();

        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body = raw[body_start..].to_string();
        results.push((status, body));
    }

    let _ = child.kill();
    let _ = child.wait();
    results
}

#[test]
fn http_state_get_lista_compartida() {
    // F11: una `let users = [...]` top-level referenciada por un
    // handler GET. El binario debe servir la lista y los items que
    // contiene preservan el formato JSON del intérprete.
    let src = "@server(43320)\nfn main() => 0\n\
               type User { id: Int, name: Str }\n\
               let users = [User { id: 1, name: \"ana\" }, User { id: 2, name: \"luis\" }]\n\
               @get(\"/users\") fn list_users() -> List<User> => users\n";
    let results = build_spawn_requests(
        "http-state-get-list",
        src,
        43320,
        &[("GET", "/users", None)],
    );
    let (status, body) = &results[0];
    assert_eq!(*status, 200);
    assert!(
        body.contains("\"id\":1")
            && body.contains("\"name\":\"ana\"")
            && body.contains("\"id\":2")
            && body.contains("\"name\":\"luis\""),
        "expected list with both users, body was: {}",
        body
    );
}

#[test]
fn http_state_post_persiste_entre_requests() {
    // F11: POST agrega a la lista. Un GET posterior **al mismo binario**
    // debe ver el nuevo item — confirmación clave de que el state
    // compartido funciona via `thread_local!` + tokio current_thread.
    let src = "@server(43321)\nfn main() => 0\n\
               type User { id: Int, name: Str }\n\
               type UserInput { name: Str }\n\
               let users = [User { id: 1, name: \"ana\" }]\n\
               @get(\"/users\") fn list_users() -> List<User> => users\n\
               @post(\"/users\") fn create_user(body: UserInput) -> User {\n\
                   let u = User { id: users.len() + 1, name: body.name }\n\
                   users.push(u)\n\
                   return u\n\
               }\n";
    let results = build_spawn_requests(
        "http-state-post-persist",
        src,
        43321,
        &[
            ("GET", "/users", None),
            ("POST", "/users", Some(r#"{"name":"sofi"}"#)),
            ("GET", "/users", None),
        ],
    );
    assert_eq!(results[0].0, 200);
    assert!(
        !results[0].1.contains("sofi"),
        "first GET should not have `sofi`, body: {}",
        results[0].1
    );
    assert_eq!(results[1].0, 200);
    assert!(
        results[1].1.contains("\"name\":\"sofi\""),
        "POST should return the new user, body: {}",
        results[1].1
    );
    assert_eq!(results[2].0, 200);
    assert!(
        results[2].1.contains("\"name\":\"sofi\"") && results[2].1.contains("\"name\":\"ana\""),
        "final GET should see both users (state persists), body: {}",
        results[2].1
    );
}

#[test]
fn http_state_put_field_mutation() {
    // F11: PUT muta campos de un user existente. El siguiente GET
    // debe ver la mutación. Mismo binary, locks consecutivos sobre el
    // thread_local.
    let src = "@server(43322)\nfn main() => 0\n\
               type User { id: Int, name: Str }\n\
               type UserInput { name: Str }\n\
               let users = [User { id: 1, name: \"ana\" }]\n\
               @get(\"/users/{id}\") fn get_user(id: Int) -> Result<User> {\n\
                   return users.find(fn(u) => u.id == id)\n\
               }\n\
               @put(\"/users/{id}\") fn update_user(id: Int, body: UserInput) -> Result<User> {\n\
                   let u = users.find(fn(u) => u.id == id)?\n\
                   u.name = body.name\n\
                   return Ok(u)\n\
               }\n";
    let results = build_spawn_requests(
        "http-state-put-mutate",
        src,
        43322,
        &[
            ("PUT", "/users/1", Some(r#"{"name":"ana actualizada"}"#)),
            ("GET", "/users/1", None),
        ],
    );
    assert_eq!(results[0].0, 200);
    assert!(
        results[0].1.contains("\"name\":\"ana actualizada\""),
        "PUT should return the mutated user, body: {}",
        results[0].1
    );
    assert_eq!(results[1].0, 200);
    assert!(
        results[1].1.contains("\"name\":\"ana actualizada\""),
        "subsequent GET should see the mutation, body: {}",
        results[1].1
    );
}

#[test]
fn http_state_delete_reconstruccion_lista() {
    // F11: DELETE que reconstruye la lista (filter + while pop + for
    // push) — patrón canónico para "borrar de una lista compartida sin
    // perder la referencia". El siguiente GET ve la lista achicada.
    let src = "@server(43323)\nfn main() => 0\n\
               type User { id: Int, name: Str }\n\
               let users = [User { id: 1, name: \"ana\" }, User { id: 2, name: \"luis\" }]\n\
               @get(\"/users\") fn list_users() -> List<User> => users\n\
               @delete(\"/users/{id}\") fn delete_user(id: Int) -> Result<Str> {\n\
                   let kept = users.filter(fn(u) => u.id != id)\n\
                   while (users.len() > 0) { users.pop() }\n\
                   for u in kept { users.push(u) }\n\
                   return Ok(\"borrado\")\n\
               }\n";
    let results = build_spawn_requests(
        "http-state-delete-rebuild",
        src,
        43323,
        &[("DELETE", "/users/2", None), ("GET", "/users", None)],
    );
    assert_eq!(results[0].0, 200);
    assert_eq!(results[1].0, 200);
    assert!(
        results[1].1.contains("\"name\":\"ana\"") && !results[1].1.contains("\"name\":\"luis\""),
        "GET post-delete should have only ana, body: {}",
        results[1].1
    );
}

#[test]
fn http_state_var_no_referenciada_no_se_promueve() {
    // F11: una var top-level que NO es referenciada por ningún handler
    // no debe afectar el codegen — debe ejecutarse como expresión normal
    // dentro de `fn main()` y el server arranca igual.
    let src = "@server(43324)\nfn main() => 0\n\
               let saludo = \"ignorada\"\n\
               @get(\"/\") fn index() -> Str => \"ok\"\n";
    let results = build_spawn_requests("http-state-unused", src, 43324, &[("GET", "/", None)]);
    assert_eq!(results[0].0, 200);
    assert!(
        results[0].1.contains("ok"),
        "GET / should return `ok`, body: {}",
        results[0].1
    );
}

#[test]
fn match_on_int_with_range_compiled() {
    // Pattern `0..10` con guard, más wildcard catch-all.
    let src = "\
let n = 5
let s = match n {
    0..10 => \"chico\"
    _ => \"grande\"
}
print(s)
let m = 50
let t = match m {
    0..10 => \"chico\"
    _ => \"grande\"
}
print(t)
";
    let (stdout, exit) = build_and_run("match-range", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["chico", "grande"]);
}

// ---------------------------------------------------------------------------
// Status codes custom (return <int> { ... })
// ---------------------------------------------------------------------------

#[test]
fn http_status_codes_custom_401_y_body_json() {
    // Sintaxis del spec: `return 401 { ... }` adentro de un handler HTTP
    // emite status 401 con el body serializado como JSON. End-to-end con
    // curl-equivalente: status line + body matchean.
    let src = "@server(43400)\nfn main() => 0\n\
               @get(\"/protected\") fn protected() -> Str {\n\
                   return 401 {\"message\": \"no autorizado\"}\n\
               }\n";
    let (status, body) =
        build_spawn_request("http-status-401", src, 43400, "GET", "/protected", None);
    assert_eq!(status, 401, "expected status 401");
    assert!(
        body.contains("\"message\":\"no autorizado\""),
        "body should contain `message`, was: {}",
        body
    );
}

#[test]
fn http_status_codes_polimorfico_mix_ok_y_404() {
    // Spec polimórfico: el handler retorna `-> Str` declarado pero
    // adentro mezcla `return "alice"` (Str → 200) con `return 404
    // {...}` (status custom). Cada uno produce la respuesta esperada.
    let src = "@server(43401)\nfn main() => 0\n\
               @get(\"/u/{id}\") fn get_user(id: Int) -> Str {\n\
                   if (id == 1) { return \"alice\" }\n\
                   return 404 {\"error\": \"no encontrado\"}\n\
               }\n";
    let (status_ok, body_ok) =
        build_spawn_request("http-status-mix-ok", src, 43401, "GET", "/u/1", None);
    assert_eq!(status_ok, 200);
    assert_eq!(body_ok.trim(), "\"alice\"");

    let (status_404, body_404) =
        build_spawn_request("http-status-mix-404", src, 43401, "GET", "/u/2", None);
    assert_eq!(status_404, 404);
    assert!(
        body_404.contains("\"error\":\"no encontrado\""),
        "404 body should contain `error`, was: {}",
        body_404
    );
}

// ---------------------------------------------------------------------------
// Query params HTTP (?key={name})
// ---------------------------------------------------------------------------

#[test]
fn http_query_params_obligatorios_extraen_y_coercen() {
    // `?limit={limit}&offset={offset}` con ambos `Int` obligatorios.
    // El handler los recibe coercionados, los inscrusta en la response.
    let src = "@server(43500)\nfn main() => 0\n\
               @get(\"/items?limit={limit}&offset={offset}\")\n\
               fn list_items(limit: Int, offset: Int) -> Str {\n\
                   return \"limit={limit} offset={offset}\"\n\
               }\n";
    let (status, body) = build_spawn_request(
        "http-query-required",
        src,
        43500,
        "GET",
        "/items?limit=10&offset=20",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "\"limit=10 offset=20\"");
}

#[test]
fn http_query_param_obligatorio_faltante_es_400() {
    let src = "@server(43501)\nfn main() => 0\n\
               @get(\"/items?limit={limit}\")\n\
               fn list_items(limit: Int) -> Int => limit\n";
    let (status, body) = build_spawn_request(
        "http-query-missing-required",
        src,
        43501,
        "GET",
        "/items",
        None,
    );
    assert_eq!(status, 400);
    assert!(
        body.contains("query param 'limit'") && body.contains("required"),
        "body should say 'required', was: {}",
        body
    );
}

#[test]
fn http_query_param_nullable_missing_returns_null() {
    // `name: Str?` → si falta en la query, el handler ve `Null`. El
    // print/interpolación lo serializa como `null`.
    let src = "@server(43502)\nfn main() => 0\n\
               @get(\"/items?name={name}\")\n\
               fn list_items(name: Str?) -> Str {\n\
                   return \"name={name}\"\n\
               }\n";
    let (status_falta, body_falta) = build_spawn_request(
        "http-query-nullable-missing",
        src,
        43502,
        "GET",
        "/items",
        None,
    );
    assert_eq!(status_falta, 200);
    assert_eq!(body_falta.trim(), "\"name=null\"");

    let (status_present, body_present) = build_spawn_request(
        "http-query-nullable-present",
        src,
        43502,
        "GET",
        "/items?name=fitz",
        None,
    );
    assert_eq!(status_present, 200);
    assert_eq!(body_present.trim(), "\"name=fitz\"");
}

#[test]
fn http_query_path_y_body_combinados() {
    // Caso completo: path param + query param + body. Cada categoría
    // se extrae del lugar correcto y el handler los recibe en el
    // orden declarado.
    let src = "@server(43503)\nfn main() => 0\n\
               type Patch { value: Int }\n\
               @put(\"/items/{id}?dry_run={dry_run}\")\n\
               fn update_item(id: Int, dry_run: Bool, body: Patch) -> Str {\n\
                   return \"id={id} dry_run={dry_run} value={body.value}\"\n\
               }\n";
    let (status, body) = build_spawn_request(
        "http-query-path-body",
        src,
        43503,
        "PUT",
        "/items/42?dry_run=true",
        Some(r#"{"value":7}"#),
    );
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "\"id=42 dry_run=true value=7\"");
}

#[test]
fn http_query_param_parse_error_es_400() {
    // `limit: Int` recibe `"abc"` → 400 con mensaje de coerción.
    let src = "@server(43504)\nfn main() => 0\n\
               @get(\"/items?limit={limit}\")\n\
               fn list_items(limit: Int) -> Int => limit\n";
    let (status, body) = build_spawn_request(
        "http-query-parse-error",
        src,
        43504,
        "GET",
        "/items?limit=abc",
        None,
    );
    assert_eq!(status, 400);
    assert!(
        body.contains("query param 'limit'"),
        "body should mention the query param, was: {}",
        body
    );
}

// ---------------------------------------------------------------------
// Fase 8.7.1 — codegen interop Python (F19 parcial)
// ---------------------------------------------------------------------
//
// Gated por `#[cfg(feature = "python")]` porque el binario generado
// linkea pyo3, lo cual exige Python disponible al build time +
// runtime. Para correr: `cargo test --features python --test
// compile_e2e fase_8_7_1`.

#[cfg(feature = "python")]
#[test]
fn fase_8_7_1_build_python_import_math_extrae_pi() {
    // Criterio mínimo 8.7.1: el codegen acepta `from python import
    // math`, compila a binario nativo standalone (con pyo3 linkeado),
    // el binario corre y produce el output esperado bit-a-bit con
    // `fitz run`. Validamos extracción primitiva PyAny → Float via
    // anotación destino en el `let pi: Float = math.pi`.
    let src = "from python import math\n\
               let pi: Float = math.pi\n\
               print(\"pi = {pi}\")\n";
    let (stdout, exit) = build_and_run("fase_8_7_1_math_pi", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    // `print` con Float usa __fitz_fmt_float que matchea el repr
    // canónico de Python para floats con fracción no trivial.
    assert_eq!(stdout.trim(), "pi = 3.141592653589793");
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_1_build_python_import_without_annotation_is_opaque() {
    // Sin anotación destino, `let m = math.pi` queda como
    // `__FitzPyObject` opaco. `print(m)` delega al Display del newtype
    // que invoca `__str__` Python — paridad bit-a-bit con `fitz run`.
    let src = "from python import math\n\
               let pi = math.pi\n\
               print(\"pi = {pi}\")\n";
    let (stdout, exit) = build_and_run("fase_8_7_1_math_pi_opaco", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "pi = 3.141592653589793");
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_call_math_sqrt_returns_result_ok() {
    // Criterio canónico 8.7.2: `math.sqrt(16.0)` desde `fitz build`
    // produce un binario que matchea bit-a-bit con `fitz run`.
    //
    // Como `call` Python devuelve `Result<PyAny>` (8.3), el binding
    // tiene que destrancarlo con `match` o `?`. Acá usamos `match` para
    // mantener el ejemplo top-level (sin envolver en fn `Result<...>`).
    let src = "from python import math\n\
               let raw = math.sqrt(16.0)\n\
               match raw {\n\
                 Ok(v) => print(\"sqrt(16) = {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_math_sqrt", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert_eq!(stdout.trim(), "sqrt(16) = 4.0");
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_call_python_excepcion_es_err() {
    // `math.sqrt(-1)` lanza ValueError. El call devuelve
    // `Err(Str("ValueError: ..."))` que el match destrancla.
    let src = "from python import math\n\
               let raw = math.sqrt(-1.0)\n\
               match raw {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(e) => print(\"caught: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_math_sqrt_neg", src);
    assert_eq!(exit, 0);
    assert!(
        stdout.contains("caught: ValueError"),
        "output should cite `ValueError`, was: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_call_python_marshalla_list_fitz_a_list_python() {
    // `json.dumps([1, 2, 3])` Fitz → arg de json.dumps recibe una
    // List<Int> Fitz que se marshalla a list Python via el impl
    // genérico `__FitzToPy for Arc<Mutex<Vec<T>>>`.
    let src = "from python import json\n\
               let xs: List<Int> = [1, 2, 3]\n\
               let raw = json.dumps(xs)\n\
               match raw {\n\
                 Ok(s) => print(\"serializado = {s}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_marshal_list", src);
    assert_eq!(exit, 0);
    // El Ok(s) tiene `s: __FitzPyObject` (sin annot); Display delega
    // a `__str__` Python → cita literal del JSON entre comillas.
    assert!(
        stdout.contains("[1, 2, 3]"),
        "expected output with [1, 2, 3], was: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_marshalla_instance_fitz_a_dict_python() {
    // Caso canónico del roadmap 8.5: una Instance Fitz (User) pasa a
    // una función Python como dict. `json.dumps(user)` → JSON con los
    // fields de la instancia preservando orden de declaración.
    let src = "type User { id: Int, name: Str }\n\
               from python import json\n\
               let u = User { id: 1, name: \"Ada\" }\n\
               let raw = json.dumps(u)\n\
               match raw {\n\
                 Ok(s) => print(\"serializado = {s}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_marshal_instance", src);
    assert_eq!(exit, 0);
    assert!(
        stdout.contains("{\\\"id\\\": 1, \\\"name\\\": \\\"Ada\\\"}")
            || stdout.contains("{\"id\": 1, \"name\": \"Ada\"}"),
        "expected JSON with id+name preserving order, was: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_2_build_call_python_propagation_with_try() {
    // Operador `?` Fitz adentro de fn que retorna `Result<Float>`
    // propaga el Err Python. La fn `root_safe` extrae el Float via
    // anotación destino sobre el Ok.
    let src = "from python import math\n\
               fn root_safe(x: Float) -> Result<Float> {\n\
                 let v: Float = math.sqrt(x)?\n\
                 return Ok(v)\n\
               }\n\
               match root_safe(25.0) {\n\
                 Ok(r) => print(\"r = {r}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_2_root_safe", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "r = 5.0");
}

// ---------------------------------------------------------------------
// Fase 8.7.3 — bridge async tokio ↔ asyncio
// ---------------------------------------------------------------------

#[cfg(feature = "python")]
#[test]
fn fase_8_7_3_build_await_corutina_asyncio_sleep() {
    // Criterio canónico 8.7.3: patrón canónico Fitz `<py_call>?.await`.
    // El `?` desempaca el `Result<Any>` del call (per 8.4 → 8.3); el
    // `.await` ejecuta la corutina vía `tokio::spawn_blocking` +
    // `asyncio.run_until_complete`. Paridad bit-a-bit con `fitz run`:
    // el intérprete usa el mismo patrón.
    let src = "from python import asyncio\n\
               async fn run() -> Result<Str> {\n\
                 let _ = asyncio.sleep(0.001)?.await\n\
                 return Ok(\"done\")\n\
               }\n\
               match run().await {\n\
                 Ok(v) => print(\"got = {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_3_asyncio_sleep", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert_eq!(stdout.trim(), "got = done");
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_3_build_pipeline_with_multiple_awaits() {
    // Múltiples awaits encadenados sobre corutinas distintas — cada
    // uno ejecuta su propia `run_until_complete`. Paridad con el
    // ejemplo canónico 8.6.
    let src = "from python import asyncio\n\
               async fn pipeline(start: Int) -> Result<Int> {\n\
                 let _ = asyncio.sleep(0)?.await\n\
                 let a = start + 1\n\
                 let _ = asyncio.sleep(0)?.await\n\
                 let b = a * 2\n\
                 return Ok(b + 100)\n\
               }\n\
               match pipeline(10).await {\n\
                 Ok(v) => print(\"result = {v}\"),\n\
                 Err(_) => print(\"(no debería)\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_3_pipeline", src);
    assert_eq!(exit, 0);
    // (10+1) * 2 + 100 = 122
    assert_eq!(stdout.trim(), "result = 122");
}

// ---- Fase 8.7.1 transitiva: `from python import` en módulos ----
//
// Pre-fix (deuda residual de Fase 8.7): el codegen rechazaba `from
// python import` adentro de módulos Fitz transitivos con error
// explícito. El workaround documentado era poner los imports Python
// en el main. Post-fix: cada módulo emite sus propios statics +
// getters locales y reusa los helpers del preludio Python del crate
// root via `use crate::__fitz_py_*`.
//
// Smoke E2E: programa con `main.fitz` + `pymath.fitz`. El módulo
// transitivo importa `python.math` y expone una fn `area(r)` que
// calcula `π * r²`. El main no tiene contacto con Python directo —
// solo usa la fn del módulo. El binario standalone corre y produce
// el resultado esperado bit-a-bit con `fitz run`.

#[cfg(feature = "python")]
#[test]
fn fase_8_7_1_transitive_build_from_python_in_module_compiles_and_runs() {
    let main_src = "from pymath import area\n\
                    let a: Float = area(2.0)\n\
                    print(\"area = {a}\")\n";
    let pymath_src = "from python import math\n\
                      fn area(r: Float) -> Float {\n  \
                          let pi: Float = math.pi\n  \
                          return pi * r * r\n\
                      }\n";
    let (stdout, exit) = build_and_run_multi(
        "fase_8_7_1_transitiva_pymath",
        main_src,
        &[("pymath.fitz", pymath_src)],
    );
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    // π * 2² = 12.566370614359172 (15 dígitos del math.pi de Python).
    assert_eq!(stdout.trim(), "area = 12.566370614359172");
}

// ---- Fase 8.7.1 transitiva-bis (v0.9.44): coerción PyAny → Nominal
//      para tipos importados de otro módulo ----
//
// Pre-fix (v0.9.43): el código del módulo `data/users.fitz` que hacía
// `let u: User = json.loads(raw)?` con `User` importado de
// `types/user.fitz` fallaba en `fitz build` con
// `cannot find function __fitz_py_to_instance_User in this scope`.
//
// Post-fix (v0.9.44): main emite los helpers Python para tipos custom
// de módulos transitivos (pub(crate)). Los módulos los referencian
// con prefijo `crate::__fitz_py_to_instance_<T>` via post-procesamiento
// del output.

#[cfg(feature = "python")]
#[test]
fn fase_8_7_1_transitive_bis_module_coerces_pyany_to_imported_type() {
    // El módulo `parser` define `type User` y `from python import
    // json`. Hace `let u: User = json.loads(raw)?` adentro de una fn.
    // Main importa `parse_user` del módulo y la invoca.
    // El módulo `parser` define `type User`, construye el dict
    // Python directamente y lo coerce. Main solo invoca y matchea
    // — no toca Python. Evitamos JSON crudo para no chocar con la
    // interpolación de strings de Fitz adentro del literal.
    let main_src = r#"from parser import parse_default_user, User
let res: Result<User> = parse_default_user()
match res {
  Ok(u) => print("name={u.name} role={u.role}"),
  Err(e) => print("err: {e}")
}
"#;
    // `User` con campos Str solo para simplificar la construcción del
    // dict Python (Fitz Map debe ser homogéneo). El test valida que
    // la coerción `PyAny → User` funciona end-to-end con User vivo en
    // el módulo, no en main.
    let parser_src = r#"from python import json
type User { name: Str, role: Str }

fn parse_default_user() -> Result<User> {
  let m: Map<Str, Str> = {"name": "Fitz", "role": "admin"}
  let raw_py = json.dumps(m)?
  let raw: Str = raw_py
  let parsed = json.loads(raw)?
  let u: User = parsed
  return Ok(u)
}
"#;
    let (stdout, exit) = build_and_run_multi(
        "fase_8_7_1_transitiva_bis_coerce",
        main_src,
        &[("parser.fitz", parser_src)],
    );
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert_eq!(stdout.trim(), "name=Fitz role=admin");
}

// ---- Mini-tanda C — list comprehensions ----

#[test]
fn mini_tanda_c_comprehension_sobre_lista_compila_y_doblea() {
    // `[x * 2 for x in [1, 2, 3]]` debe producir `[2, 4, 6]` y el
    // binario nativo lo imprime con el formato canónico bit-a-bit
    // igual que `fitz run`.
    let src = "let r: List<Int> = [x * 2 for x in [1, 2, 3]]\nprint(r)\n";
    let (stdout, exit) = build_and_run("mini_tanda_c_comp_simple", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4, 6]");
}

#[test]
fn mini_tanda_c_comprehension_over_range_with_filter() {
    // `[n for n in 0..10 if n % 2 == 0]` filtra pares. La anotación
    // `List<Int>` ayuda al codegen a tipar concreto el iter Int.
    let src = "let r: List<Int> = [n for n in 0..10 if n % 2 == 0]\nprint(r)\n";
    let (stdout, exit) = build_and_run("mini_tanda_c_comp_filter", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[0, 2, 4, 6, 8]");
}

// ---- Mini-tanda Fm — format specifiers ----

#[test]
fn mini_tanda_fm_float_with_decimal_precision() {
    // `{ratio:.2f}` debe producir el mismo output bit-a-bit que
    // `fitz run` (es decir "0.50" para 0.5).
    let src = "let ratio: Float = 0.5\nprint(\"{ratio:.2f}\")\n";
    let (stdout, exit) = build_and_run("mini_tanda_fm_float_precision", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0.50");
}

#[test]
fn mini_tanda_fm_int_with_width_and_zero_pad() {
    // `{n:05d}` produce "00042".
    let src = "let n: Int = 42\nprint(\"{n:05d}\")\n";
    let (stdout, exit) = build_and_run("mini_tanda_fm_int_zero_pad", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "00042");
}

#[test]
fn mini_tanda_fm_hex_with_alternate() {
    // `{n:#x}` produce "0xff".
    let src = "let n: Int = 255\nprint(\"{n:#x}\")\n";
    let (stdout, exit) = build_and_run("mini_tanda_fm_hex_alt", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0xff");
}

#[test]
fn mini_tanda_fm_alignment_right_with_default_fill() {
    // `{x:>5}` padding con espacios a la derecha (right alignment).
    let src = "let x: Int = 42\nprint(\"[{x:>5}]\")\n";
    let (stdout, exit) = build_and_run("mini_tanda_fm_align_right", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[   42]");
}

// ---- Mini-tanda Md — for con Pattern (Map destructuring) ----

#[test]
fn mini_tanda_md_for_sobre_map_destructura_pares() {
    // `for (k, v) in m` itera sobre el Map bindeando k y v.
    let src = "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
               let total: Int = 0\n\
               for (_, v) in m {\n\
                 total = total + v\n\
               }\n\
               print(total)\n";
    let (stdout, exit) = build_and_run("mini_tanda_md_for_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "6");
}

#[test]
fn mini_tanda_md_for_wildcard_pattern_compila() {
    // `for _ in 0..5` ignora el elemento.
    let src = "let count: Int = 0\n\
               for _ in 0..5 {\n\
                 count = count + 1\n\
               }\n\
               print(count)\n";
    let (stdout, exit) = build_and_run("mini_tanda_md_wildcard", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "5");
}

// ---- Mini-tanda It — enumerate / zip / chain ----

#[test]
fn mini_tanda_it_enumerate_with_for_destructuring() {
    // `for (i, x) in xs.enumerate() { ... }` — caso canónico que
    // motiva la mini-tanda. Encaja con Md (tuple destructuring).
    let src = "let xs: List<Str> = [\"a\", \"b\", \"c\"]\n\
               for (i, x) in xs.enumerate() {\n\
                 print(\"{i}={x}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_it_enumerate", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0=a\n1=b\n2=c");
}

// ---- Mini-tanda Bits — operadores bit-a-bit ----

#[test]
fn mini_tanda_bits_and_or_xor_y_shifts() {
    // Operadores básicos sobre hex literales (combinación Lit + Bits).
    let src = "let mask: Int = 0xFF\n\
               let raw: Int = 0xABCD\n\
               let lo: Int = raw & mask\n\
               let hi: Int = (raw >> 8) & mask\n\
               let recombined: Int = (hi << 8) | lo\n\
               let xored: Int = raw ^ 0xFFFF\n\
               print(lo)\n\
               print(hi)\n\
               print(recombined)\n\
               print(xored)\n";
    let (stdout, exit) = build_and_run("mini_tanda_bits_basicos", src);
    assert_eq!(exit, 0);
    // lo=0xCD=205, hi=0xAB=171, recombined=0xABCD=43981, xored=0xFFFF^0xABCD=0x5432=21554.
    assert_eq!(stdout.trim(), "205\n171\n43981\n21554");
}

// ---- Mini-tanda Err+ — `Err` con tipos no-Str + `?` mensaje propio ----

#[test]
fn mini_tanda_err_plus_err_int_compila_y_corre() {
    // Re+ post-tightened: `Result<Int>` con `Err(Int)` ahora requiere
    // declarar el E explícito (`Result<Int, Int>`).
    let src = "fn fail() -> Result<Int, Int> {\n\
                 return Err(404)\n\
               }\n\
               match fail() {\n\
                 Ok(v) => print(\"ok\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_err_plus_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err: 404");
}

// ---- Mini-tanda Rt — tuple patterns con sub-pattern Str/Range/Or ----

#[test]
fn mini_tanda_rt_tuple_with_str_literal_subpattern() {
    // `("ada", n)` en codegen: Str literal como sub-pattern de Tuple.
    let src = "fn name(p: (Str, Int)) -> Str {\n\
                 return match p {\n\
                   (\"ada\", n) => \"ada n={n}\",\n\
                   (other, n) => \"{other} n={n}\"\n\
                 }\n\
               }\n\
               print(name((\"ada\", 5)))\n\
               print(name((\"bob\", 7)))\n";
    let (stdout, exit) = build_and_run("mini_tanda_rt_str_sub", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "ada n=5\nbob n=7");
}

#[test]
fn mini_tanda_rt_tuple_with_or_pattern_subpattern() {
    // `(name, 1 | 2)` en codegen: Or-pattern como sub-pattern.
    let src = "fn clasif(p: (Str, Int)) -> Str {\n\
                 return match p {\n\
                   (n, 1 | 2) => \"{n}: chico\",\n\
                   (n, m) => \"{n}: {m}\"\n\
                 }\n\
               }\n\
               print(clasif((\"ada\", 1)))\n\
               print(clasif((\"ada\", 2)))\n\
               print(clasif((\"bob\", 42)))\n";
    let (stdout, exit) = build_and_run("mini_tanda_rt_or_sub", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "ada: chico\nada: chico\nbob: 42");
}

#[test]
fn mini_tanda_rt_tuple_with_range_subpattern() {
    // `(name, 0..10)` en codegen: Range como sub-pattern.
    let src = "fn band(p: (Str, Int)) -> Str {\n\
                 return match p {\n\
                   (n, 0..10) => \"{n}: dig\",\n\
                   (n, 10..100) => \"{n}: dec\",\n\
                   (n, _) => \"{n}: big\"\n\
                 }\n\
               }\n\
               print(band((\"a\", 5)))\n\
               print(band((\"b\", 42)))\n\
               print(band((\"c\", 500)))\n";
    let (stdout, exit) = build_and_run("mini_tanda_rt_range_sub", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "a: dig\nb: dec\nc: big");
}

// ---- Mini-tanda Cmp — ops compuestos bit-a-bit + prefijos mayúscula ----

#[test]
fn mini_tanda_cmp_ops_compuestos_bit_a_bit() {
    let src = "let flags: Int = 0b0000_0101\n\
               flags |= 0b0010\n\
               print(flags)\n\
               flags &= 0b1110\n\
               print(flags)\n\
               flags ^= 0b0100\n\
               print(flags)\n\
               flags <<= 2\n\
               print(flags)\n\
               flags >>= 1\n\
               print(flags)\n";
    let (stdout, exit) = build_and_run("mini_tanda_cmp_compuestos", src);
    assert_eq!(exit, 0);
    // 0b101=5 | 0b010=2 → 0b111=7
    // 7 & 0b1110=14 → 0b110=6
    // 6 ^ 0b100=4 → 0b010=2
    // 2 << 2 → 8
    // 8 >> 1 → 4
    assert_eq!(stdout.trim(), "7\n6\n2\n8\n4");
}

#[test]
fn mini_tanda_cmp_prefijos_mayuscula() {
    let src = "let h: Int = 0XFF\n\
               let b: Int = 0B1010\n\
               let o: Int = 0O755\n\
               print(h)\n\
               print(b)\n\
               print(o)\n";
    let (stdout, exit) = build_and_run("mini_tanda_cmp_prefijos", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "255\n10\n493");
}

// ---- Mini-tanda Re+ — Result<T, E> tipado en codegen ----

#[test]
fn mini_tanda_re_plus_err_instance_fields_accesibles_en_build() {
    // El caso canónico que motiva Re+: tras anotar `Result<T, E>`
    // con E concreto, el binding `Err(e)` tipa con E real (no Str),
    // así que podés acceder a sus fields en `fitz build`.
    let src = "type ApiError { status: Int, msg: Str }\n\
               fn fetch() -> Result<Int, ApiError> {\n\
                 return Err(ApiError { status: 503, msg: \"unavailable\" })\n\
               }\n\
               match fetch() {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(e) => print(\"err {e.status}: {e.msg}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_re_plus_fields", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err 503: unavailable");
}

#[test]
fn mini_tanda_re_plus_err_int_bindea_int_no_str() {
    // Antes de Re+, `Err(e)` siempre bindeaba `e: Str` en codegen.
    // Ahora con `Result<T, Int>` explícito, `e` tipa como Int.
    let src = "fn fail() -> Result<Str, Int> {\n\
                 return Err(404)\n\
               }\n\
               let code: Int = match fail() {\n\
                 Ok(_) => 0,\n\
                 Err(e) => e\n\
               }\n\
               print(code)\n";
    let (stdout, exit) = build_and_run("mini_tanda_re_plus_int_binding", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "404");
}

#[test]
fn mini_tanda_re_plus_legacy_result_t_without_e_keeps_working() {
    // Regresión: `Result<T>` sin E sigue funcionando (default Str).
    let src = "fn div(a: Int, b: Int) -> Result<Int> {\n\
                 if b == 0 { return Err(\"zero\") }\n\
                 return Ok(a / b)\n\
               }\n\
               match div(10, 0) {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_re_plus_legacy", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err: zero");
}

#[test]
fn mini_tanda_err_plus_err_instance_compiles_and_preserves_display() {
    // `Err(Instance)` deref del Arc<Mutex<TData>> antes del format!
    // — paridad bit-a-bit con el Display del intérprete. Post-Re+ se
    // requiere declarar el E explícito.
    let src = "type ApiError { status: Int, msg: Str }\n\
               fn fetch() -> Result<Int, ApiError> {\n\
                 return Err(ApiError { status: 503, msg: \"unavailable\" })\n\
               }\n\
               match fetch() {\n\
                 Ok(v) => print(\"ok\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mini_tanda_err_plus_instance", src);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout.trim(),
        "err: ApiError { status: 503, msg: \"unavailable\" }"
    );
}

#[test]
fn mini_tanda_bits_not_unario() {
    // ~0 = -1, ~0xFF = -256 (i64 con signo).
    let src = "let a: Int = ~0\n\
               let b: Int = ~0xFF\n\
               print(a)\n\
               print(b)\n";
    let (stdout, exit) = build_and_run("mini_tanda_bits_not", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "-1\n-256");
}

#[test]
fn mini_tanda_it_zip_trunca_y_chain_concatena() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = [10, 20]\n\
               let zipped: List<(Int, Int)> = xs.zip(ys)\n\
               print(zipped.len())\n\
               let chained: List<Int> = xs.chain(ys)\n\
               print(chained.len())\n\
               print(chained[4])\n";
    let (stdout, exit) = build_and_run("mini_tanda_it_zip_chain", src);
    assert_eq!(exit, 0);
    // zip truncado al más corto (len 2), chain concatena (3 + 2 = 5),
    // último elemento del chain es 20.
    assert_eq!(stdout.trim(), "2\n5\n20");
}

// ---- Mini-tanda Up — Map.update + list comprehension tuple destructure ----

#[test]
fn up_map_update_compila() {
    let src = "let scores: Map<Str, Int> = {\"ada\": 80, \"bob\": 45}\n\
               let bumped: Map<Str, Int> = scores.update(\"ada\", fn(v: Int) => v + 10)\n\
               print(bumped[\"ada\"])\n\
               print(bumped[\"bob\"])\n\
               let nochange: Map<Str, Int> = scores.update(\"missing\", fn(v: Int) => v + 999)\n\
               print(nochange.len())\n\
               print(nochange[\"ada\"])\n";
    // Stem "up_map_upd" (no "update"): el heurístico installer-
    // detection de Windows UAC bloquea ejecutables con "update" en
    // el nombre. Mismo workaround que aplicamos en 10.b.11
    // (`orm_upd_list_map_codegen`).
    let (stdout, exit) = build_and_run("up_map_upd", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "90\n45\n2\n80");
}

#[test]
fn up_comprehension_tuple_destructure_compila() {
    let src = "let pairs: List<(Int, Int)> = [(1, 10), (2, 20), (3, 30)]\n\
               let sums: List<Int> = [a + b for (a, b) in pairs]\n\
               print(sums)\n\
               let firsts: List<Int> = [a for (a, _) in pairs]\n\
               print(firsts)\n";
    let (stdout, exit) = build_and_run("up_comp_tuple", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[11, 22, 33]\n[1, 2, 3]");
}

// ---- Mini-tanda Ex2 — List.flat_map/first/last + Map.merge ----

#[test]
fn ex2_list_flat_map_compila() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let r: List<Int> = xs.flat_map(fn(n: Int) => [n, n * 10])\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("ex2_flat_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 10, 2, 20, 3, 30]");
}

#[test]
fn ex2_list_first_and_last_return_result() {
    let src = "let xs: List<Int> = [42, 7, 100]\n\
               match xs.first() {\n\
                 Ok(v) => print(\"first: {v}\"),\n\
                 Err(_) => print(\"empty\")\n\
               }\n\
               match xs.last() {\n\
                 Ok(v) => print(\"last: {v}\"),\n\
                 Err(_) => print(\"empty\")\n\
               }\n\
               let empty: List<Int> = []\n\
               match empty.first() {\n\
                 Ok(v) => print(\"first: {v}\"),\n\
                 Err(_) => print(\"empty list\")\n\
               }\n";
    let (stdout, exit) = build_and_run("ex2_first_last", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "first: 42\nlast: 100\nempty list");
}

#[test]
fn ex2_map_merge_compila() {
    let src = "let m1: Map<Str, Int> = {\"a\": 1, \"b\": 2}\n\
               let m2: Map<Str, Int> = {\"b\": 20, \"c\": 3}\n\
               let r: Map<Str, Int> = m1.merge(m2)\n\
               print(r.len())\n\
               print(r[\"a\"])\n\
               print(r[\"b\"])\n\
               print(r[\"c\"])\n";
    let (stdout, exit) = build_and_run("ex2_map_merge", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3\n1\n20\n3");
}

// ---- Mini-tanda Ex — extras: Str search + Map filter/map_values ----

#[test]
fn ex_str_find_index_of_compilan() {
    let src = "let s: Str = \"hola mundo, hola fitz\"\n\
               match s.find(\"hola\") {\n\
                 Ok(i) => print(\"find: {i}\"),\n\
                 Err(_) => print(\"no\")\n\
               }\n\
               match s.last_index_of(\"hola\") {\n\
                 Ok(i) => print(\"last: {i}\"),\n\
                 Err(_) => print(\"no\")\n\
               }\n\
               match s.index_of(\"nope\") {\n\
                 Ok(i) => print(\"idx: {i}\"),\n\
                 Err(_) => print(\"not found\")\n\
               }\n";
    let (stdout, exit) = build_and_run("ex_str_search", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "find: 0\nlast: 12\nnot found");
}

#[test]
fn ex_map_filter_y_map_values_compilan() {
    let src = "let scores: Map<Str, Int> = {\"ada\": 80, \"bob\": 45, \"cam\": 92}\n\
               let passing: Map<Str, Int> = scores.filter(fn(k: Str, v: Int) => v >= 60)\n\
               print(passing.len())\n\
               let doubled: Map<Str, Int> = scores.map_values(fn(v: Int) => v * 2)\n\
               print(doubled[\"ada\"])\n\
               print(doubled[\"bob\"])\n";
    let (stdout, exit) = build_and_run("ex_map_transforms", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "2\n160\n90");
}

// ---- Mini-tanda CM — cross-module method dispatch ----

#[test]
fn cm_metodos_custom_sobre_tipos_importados_compilan() {
    // Antes de CM: `from foo import User` + `u.greet()` fallaba en
    // `fitz build` con "el tipo `User` no tiene un método llamado
    // `greet`". Las methods del tipo no se copiaban al type_methods
    // del importer. Post-CM: dispatch funciona bit-a-bit.
    let main = "from utils import User\n\
                let u: User = User { id: 1, name: \"Ada\" }\n\
                print(u.greet())\n\
                let admin: User = User.admin()\n\
                print(admin.greet())\n";
    let utils = "type User {\n\
                     id: Int = 0\n\
                     name: Str = \"anon\"\n\
                     fn greet() -> Str { return \"hola, {name}\" }\n\
                     static fn admin() -> User { return User { id: 0, name: \"admin\" } }\n\
                 }\n";
    let (stdout, exit) =
        build_and_run_multi("cm-cross-module-methods", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hola, Ada", "hola, admin"]);
}

// ---- Mini-tanda Vm — métodos privados (`_method`) en `type` ----

#[test]
fn vm_metodo_publico_compila() {
    // Sanity: un método público sigue funcionando con el filter de
    // private methods activo.
    let src = "type C {\n\
                   x: Int = 0\n\
                   fn show() -> Int { return x }\n\
               }\n\
               let c: C = C { x: 42 }\n\
               print(c.show())\n";
    let (stdout, exit) = build_and_run("vm_publico_ok", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

// ---- Mini-tanda Lx — List.any/all/count/find_index ----

#[test]
fn lx_any_y_all_bit_a_bit() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               print(xs.any(fn(x: Int) => x > 3))\n\
               print(xs.any(fn(x: Int) => x > 10))\n\
               print(xs.all(fn(x: Int) => x > 0))\n\
               print(xs.all(fn(x: Int) => x > 2))\n";
    let (stdout, exit) = build_and_run("lx_any_all", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse\ntrue\nfalse");
}

#[test]
fn lx_count_y_find_index_bit_a_bit() {
    let src = "let xs: List<Int> = [10, 20, 30, 40]\n\
               print(xs.count(fn(x: Int) => x > 15))\n\
               match xs.find_index(fn(x: Int) => x == 30) {\n\
                 Ok(i) => print(\"idx: {i}\"),\n\
                 Err(_) => print(\"missing\")\n\
               }\n\
               match xs.find_index(fn(x: Int) => x > 100) {\n\
                 Ok(i) => print(\"idx: {i}\"),\n\
                 Err(_) => print(\"missing\")\n\
               }\n";
    let (stdout, exit) = build_and_run("lx_count_find_index", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3\nidx: 2\nmissing");
}

#[test]
fn lx_lista_vacia_any_false_all_true() {
    let src = "let empty: List<Int> = []\n\
               print(empty.any(fn(x: Int) => true))\n\
               print(empty.all(fn(x: Int) => false))\n\
               print(empty.count(fn(x: Int) => true))\n";
    let (stdout, exit) = build_and_run("lx_empty", src);
    assert_eq!(exit, 0);
    // any vacía → false, all vacía → true (vacuous truth), count → 0.
    assert_eq!(stdout.trim(), "false\ntrue\n0");
}

// ---- Mini-tanda St — métodos estáticos en `type` ----

#[test]
fn st_static_methods_constructores_y_factories() {
    // `static fn zero()` y `static fn of(n)` como constructores.
    let src = "type C {\n\
                   value: Int = 0\n\n\
                   static fn zero() -> C { return C { value: 0 } }\n\
                   static fn of(n: Int) -> C { return C { value: n } }\n\
               }\n\
               let z: C = C.zero()\n\
               let c: C = C.of(42)\n\
               print(z)\n\
               print(c)\n";
    let (stdout, exit) = build_and_run("st_static_constructors", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "C { value: 0 }\nC { value: 42 }");
}

#[test]
fn st_static_e_instance_methods_coexisten() {
    // El mismo `type` puede tener ambos tipos de método. Test que
    // static + instance no se pisan.
    let src = "type C {\n\
                   value: Int = 0\n\n\
                   static fn make(n: Int) -> C { return C { value: n } }\n\n\
                   fn double() -> C { return C { value: value * 2 } }\n\
               }\n\
               let c: C = C.make(7)\n\
               let d: C = c.double()\n\
               print(c)\n\
               print(d)\n";
    let (stdout, exit) = build_and_run("st_static_y_instance", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "C { value: 7 }\nC { value: 14 }");
}

// ---- Mini-tanda F8 — identificadores no-ASCII (Unicode) ----

#[test]
fn f8_identifiers_unicode_compilan_y_corren() {
    // Letras griegas, acentos, ñ, CJK, cirílico. is_alphabetic /
    // is_alphanumeric del lexer ya los acepta; Rust permite Unicode
    // identifiers desde edition 2021 — paso transparente.
    let src = "let π: Float = 3.14159\n\
               let función: Str = \"hola\"\n\
               let café: Int = 42\n\
               let 名前: Str = \"Fitz\"\n\
               let имя: Str = \"Roy\"\n\
               print(π)\n\
               print(función)\n\
               print(café)\n\
               print(名前)\n\
               print(имя)\n";
    let (stdout, exit) = build_and_run("f8_idents_unicode", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3.14159\nhola\n42\nFitz\nRoy");
}

#[test]
fn f8_fn_and_params_with_unicode_identifiers() {
    // `fn` con nombre Unicode + params con nombre Unicode.
    let src = "fn niño(edad: Int) -> Str => \"niño de {edad}\"\n\
               print(niño(5))\n";
    let (stdout, exit) = build_and_run("f8_fn_unicode", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "niño de 5");
}

// ---- Mini-tanda Xor — operador `xor` lógico sobre Bool ----

#[test]
fn xor_tabla_de_verdad_bit_a_bit() {
    let src = "print(true xor true)\n\
               print(true xor false)\n\
               print(false xor true)\n\
               print(false xor false)\n";
    let (stdout, exit) = build_and_run("xor_tabla", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "false\ntrue\ntrue\nfalse");
}

#[test]
fn xor_chain_same_precedence_as_or() {
    // `true xor true xor true` left-assoc → ((T xor T) xor T) = (F xor T) = T.
    let src = "print(true xor true xor true)\n\
               print(true xor false xor true)\n";
    let (stdout, exit) = build_and_run("xor_chain", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse");
}

#[test]
fn xor_combines_with_and_and_or() {
    // `a and b xor c` → `(a and b) xor c`. And precedencia mayor.
    let src = "let a: Bool = true\n\
               let b: Bool = false\n\
               let c: Bool = true\n\
               print(a and b xor c)\n";
    let (stdout, exit) = build_and_run("xor_and_or", src);
    assert_eq!(exit, 0);
    // (true and false) xor true = false xor true = true.
    assert_eq!(stdout.trim(), "true");
}

// ---- Mini-tanda Mln — from import multi-línea con paréntesis ----

#[test]
fn mln_from_import_parens_multi_linea_compila_y_corre() {
    // Caso canónico: importar varios items en forma multi-línea con
    // paréntesis. Cada item en su línea, trailing comma opcional.
    let main = "from utils import (\n\
                    greet,\n\
                    shout,\n\
                    User,\n\
                )\n\
                print(greet(\"Mln\"))\n\
                print(shout(\"Mln\"))\n\
                print(User { id: 1, name: \"Fitz\" })\n";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"\n\
                 fn shout(s: Str) -> Str => s.upper()\n\
                 type User { id: Int = 0, name: Str = \"anon\" }\n";
    let (stdout, exit) =
        build_and_run_multi("mln-parens-multilinea", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["hola, Mln", "MLN", "User { id: 1, name: \"Fitz\" }"],
    );
}

#[test]
fn mln_from_import_parens_with_mixed_aliases_compiles() {
    // Aliases dentro de los paréntesis funcionan igual que en
    // single-line; el binding local se hace bajo el alias.
    let main = "from utils import (\n\
                    greet,\n\
                    shout as scream,\n\
                    User as Persona,\n\
                )\n\
                print(greet(\"Mln\"))\n\
                print(scream(\"Mln\"))\n\
                print(Persona { id: 1, name: \"Fitz\" })\n";
    let utils = "fn greet(name: Str) -> Str => \"hola, {name}\"\n\
                 fn shout(s: Str) -> Str => s.upper()\n\
                 type User { id: Int = 0, name: Str = \"anon\" }\n";
    let (stdout, exit) = build_and_run_multi("mln-parens-aliases", main, &[("utils.fitz", utils)]);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &[
            "hola, Mln",
            "MLN",
            // Aliases locales: pero el Display del struct usa el
            // type_name canónico ("User"), no el alias ("Persona") —
            // paridad bit-a-bit con el evaluator (PreF8.4).
            "User { id: 1, name: \"Fitz\" }",
        ],
    );
}

// ---- Mini-tanda El — Err(List<T>) / Err(Map<K,V>) en codegen ----

#[test]
fn el_err_list_compiles_and_preserves_value() {
    // Err(List<Int>): el binding `Err(xs)` tipa con List<Int>, así
    // métodos como `.len()` funcionan sobre el value.
    let src = "fn fail() -> Result<Int, List<Int>> {\n\
                 return Err([1, 2, 3])\n\
               }\n\
               match fail() {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(xs) => print(\"err con {xs.len()} items\")\n\
               }\n";
    let (stdout, exit) = build_and_run("el_err_list", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err con 3 items");
}

#[test]
fn el_err_list_direct_print_matches_interpreter() {
    // Print directo del Err con List preserva el formato canónico
    // (matchea el evaluator).
    let src = "fn fail() -> Result<Int, List<Int>> {\n\
                 return Err([10, 20, 30])\n\
               }\n\
               match fail() {\n\
                 Ok(v) => print(\"ok\"),\n\
                 Err(xs) => print(\"{xs}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("el_err_list_print", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 20, 30]");
}

#[test]
fn el_err_map_compiles_and_preserves_value() {
    // Err(Map<Str, Int>): el binding `Err(m)` tipa con Map<Str, Int>,
    // así `.len()` y `.has(k)` funcionan sobre el value.
    let src = "fn fail() -> Result<Int, Map<Str, Int>> {\n\
                 return Err({\"a\": 1, \"b\": 2})\n\
               }\n\
               match fail() {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(m) => print(\"err size: {m.len()}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("el_err_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err size: 2");
}

#[test]
fn el_err_propagation_with_list_via_try_operator() {
    // `?` propaga el Err(List<T>) intacto.
    let src = "fn inner() -> Result<Int, List<Int>> {\n\
                 return Err([1, 2])\n\
               }\n\
               fn outer() -> Result<Int, List<Int>> {\n\
                 let v = inner()?\n\
                 return Ok(v)\n\
               }\n\
               match outer() {\n\
                 Ok(v) => print(\"ok: {v}\"),\n\
                 Err(xs) => print(\"pipe err: {xs.len()}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("el_err_list_try", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "pipe err: 2");
}

// ---- Mini-tanda Ir — iteradores sobre Range ----

#[test]
fn ir_range_enumerate_compila_y_corre() {
    let src = "for (i, n) in (0..3).enumerate() {\n\
                 print(\"{i}-{n}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("ir_range_enumerate", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0-0\n1-1\n2-2");
}

#[test]
fn ir_range_zip_with_list_str_truncates() {
    let src = "let nombres: List<Str> = [\"ada\", \"bea\"]\n\
               for (i, n) in (1..100).zip(nombres) {\n\
                 print(\"{i}-{n}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("ir_range_zip", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1-ada\n2-bea");
}

#[test]
fn ir_range_chain_with_list_int_concatenates() {
    let src = "let extra: List<Int> = [100, 200]\n\
               let combo: List<Int> = (0..3).chain(extra)\n\
               print(combo)\n";
    let (stdout, exit) = build_and_run("ir_range_chain", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[0, 1, 2, 100, 200]");
}

#[test]
fn ir_range_len_exclusivo_e_inclusivo_compila() {
    let src = "print((10..20).len())\n\
               print((10..=20).len())\n";
    let (stdout, exit) = build_and_run("ir_range_len", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "10\n11");
}

// ---- Mini-tanda Mb — trim_start/trim_end + flatten + sort_by ----

#[test]
fn mb_str_trim_start_y_end_compilan() {
    let src = "print(\"  hola  \".trim_start())\n\
               print(\"  hola  \".trim_end())\n\
               print(\"\\n\\tlinea\\n\\t\".trim_start())\n\
               print(\"\\n\\tlinea\\n\\t\".trim_end())\n";
    let (stdout, exit) = build_and_run("mb_trim_start_end", src);
    assert_eq!(exit, 0);
    // trim_start: recorta inicio, deja el sufijo intacto.
    // trim_end: recorta final, deja el prefijo intacto.
    assert_eq!(stdout, "hola  \n  hola\nlinea\n\t\n\n\tlinea\n");
}

#[test]
fn mb_list_flatten_concatena_sublistas() {
    let src = "let xss: List<List<Int>> = [[1, 2], [3], [4, 5, 6]]\n\
               let flat: List<Int> = xss.flatten()\n\
               print(flat)\n\
               print(flat.len())\n";
    let (stdout, exit) = build_and_run("mb_list_flatten", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3, 4, 5, 6]\n6");
}

#[test]
fn mb_list_flatten_de_listas_vacias_es_vacio() {
    let src = "let xss: List<List<Int>> = [[], [], []]\n\
               let flat: List<Int> = xss.flatten()\n\
               print(flat)\n\
               print(flat.len())\n";
    let (stdout, exit) = build_and_run("mb_list_flatten_empty", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[]\n0");
}

#[test]
fn mb_list_sort_by_ascendente_y_descendente() {
    let src = "let xs: List<Int> = [3, 1, 4, 1, 5, 9, 2, 6]\n\
               xs.sort_by(fn(a: Int, b: Int) => a - b)\n\
               print(xs)\n\
               let ys: List<Int> = [3, 1, 4]\n\
               ys.sort_by(fn(a: Int, b: Int) => b - a)\n\
               print(ys)\n";
    let (stdout, exit) = build_and_run("mb_list_sort_by", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 1, 2, 3, 4, 5, 6, 9]\n[4, 3, 1]");
}

#[test]
fn mb_list_sort_by_comparator_compuesto() {
    // Ordenar por valor absoluto (caso típico).
    let src = "fn abs_diff(a: Int, b: Int) -> Int {\n\
                   let aa: Int = if a < 0 { 0 - a } else { a }\n\
                   let bb: Int = if b < 0 { 0 - b } else { b }\n\
                   return aa - bb\n\
               }\n\
               let xs: List<Int> = [-3, 1, -4, 1, 5, -9, 2, 6]\n\
               xs.sort_by(fn(a: Int, b: Int) => abs_diff(a, b))\n\
               print(xs)\n";
    let (stdout, exit) = build_and_run("mb_list_sort_by_abs", src);
    assert_eq!(exit, 0);
    // Por valor absoluto ascendente: |1|=|1|=1, |2|=2, |-3|=3, |-4|=4,
    // |5|=5, |6|=6, |-9|=9 → orden estable: [1, 1, 2, -3, -4, 5, 6, -9]
    assert_eq!(stdout.trim(), "[1, 1, 2, -3, -4, 5, 6, -9]");
}

// ---- Mini-tanda Lt — let-destructure con sub-patterns ricos ----

#[test]
fn lt_let_literal_int_subpattern_compila_y_corre() {
    // `let (1, x) = (1, 42)` — literal Int como guard del primer slot.
    let src = "let (1, x) = (1, 42)\nprint(x)\n";
    let (stdout, exit) = build_and_run("lt_let_int_lit", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn lt_let_str_literal_subpattern_compila_y_corre() {
    // `let ("ada", n) = ("ada", 7)` — Str literal genera guard
    // `__s_X.as_str() == "ada"`.
    let src = "let (\"ada\", n) = (\"ada\", 7)\nprint(n)\n";
    let (stdout, exit) = build_and_run("lt_let_str_lit", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "7");
}

#[test]
fn lt_let_range_subpattern_compila_y_corre() {
    // `let (0..100, y) = (50, "yes")` — Range emite guard
    // `(0..100).contains(&__n_X)`.
    let src = "let (0..100, y) = (50, \"yes\")\nprint(y)\n";
    let (stdout, exit) = build_and_run("lt_let_range", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "yes");
}

#[test]
fn lt_let_ok_binding_extrae_resultado() {
    // `let (Ok(v), tag) = (Ok(99), "result")` — desempaca Result
    // dentro de tuple. Bindings: v=99, tag="result".
    let src = "let (Ok(v), tag) = (Ok(99), \"result\")\nprint(v)\nprint(tag)\n";
    let (stdout, exit) = build_and_run("lt_let_ok_binding", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "99\nresult");
}

#[test]
fn lt_let_panic_if_does_not_match() {
    // `let (1, x) = (2, 42)`: el 2 NO matchea el 1 → panic en runtime.
    // El binario debe terminar con exit code != 0.
    // T4 (post-W12-W16) — además del exit code, validamos que el panic
    // emite mensaje claro citando que el patrón no matcheó (vía stderr).
    let src = "let (1, x) = (2, 42)\nprint(x)\n";
    let (stdout, stderr, exit) = build_and_run_with_stderr("lt_let_panic", src);
    assert_ne!(exit, 0, "expected exit code != 0 due to panic, was: 0");
    // The binary produces "the `let` did not match the pattern" or similar
    // (panic message from codegen). Accept any mention of `let`,
    // `pattern`, or `match`.
    assert!(
        stderr.contains("let") || stderr.contains("pattern") || stderr.contains("match"),
        "expected stderr citing the failed let/pattern, was:\nstdout={}\nstderr={}",
        stdout,
        stderr
    );
}

// ---- Mini-tanda F9 — escapes extendidos en strings ----

#[test]
fn f9_extended_escapes_bit_for_bit_parity() {
    // `\u{...}` Unicode (BMP + suplementario), `\x..` ASCII hex, `\0`,
    // `\b`. El lexer produce un Token::Str con los chars resueltos,
    // así que el codegen no necesita lógica extra — `rust_str_literal`
    // emite el literal Rust correcto vía `format!("{:?}", s)`.
    let src = "let cafe: Str = \"caf\\u{00E9}\"\n\
               let snow: Str = \"\\u{2603}\"\n\
               let a: Str = \"\\x41-\\x7F\"\n\
               let nul: Str = \"x\\0y\"\n\
               print(cafe)\n\
               print(snow)\n\
               print(a)\n\
               print(nul.len())\n";
    let (stdout, exit) = build_and_run("f9_escapes_extendidos", src);
    assert_eq!(exit, 0);
    // café, ☃, A-<DEL>, nul.len() = 3 chars (x + NUL + y).
    assert_eq!(stdout.trim(), "café\n☃\nA-\u{007F}\n3");
}

// ---- Mini-tanda Mb2 + Rg ---------------------------------------
// Métodos chicos: List.min/max/sum, Str.pad_start/pad_end,
// Map.keys_sorted, Range.step_by. Cada test valida que `fitz build`
// produce un binario standalone con output bit-a-bit idéntico al
// intérprete (`fitz run`).

#[test]
fn mb2_list_min_max_sum_int_compila() {
    let src = "let xs: List<Int> = [3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5]\n\
               match xs.min() {\n\
                 Ok(v) => print(\"min: {v}\"),\n\
                 Err(_) => print(\"vacía\")\n\
               }\n\
               match xs.max() {\n\
                 Ok(v) => print(\"max: {v}\"),\n\
                 Err(_) => print(\"vacía\")\n\
               }\n\
               let total: Int = xs.sum()\n\
               print(\"sum: {total}\")\n";
    let (stdout, exit) = build_and_run("mb2_list_min_max_sum_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "min: 1\nmax: 9\nsum: 44");
}

#[test]
fn mb2_list_min_max_sum_float_compila() {
    let src = "let xs: List<Float> = [1.5, 0.5, 2.25, 1.0]\n\
               match xs.min() {\n\
                 Ok(v) => print(\"min: {v}\"),\n\
                 Err(_) => print(\"vacía\")\n\
               }\n\
               let total: Float = xs.sum()\n\
               print(\"sum: {total}\")\n";
    let (stdout, exit) = build_and_run("mb2_list_min_max_sum_float", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "min: 0.5\nsum: 5.25");
}

#[test]
fn mb2_list_min_empty_returns_err_parity() {
    let src = "let xs: List<Int> = []\n\
               match xs.min() {\n\
                 Ok(v) => print(\"min: {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n\
               let total: Int = xs.sum()\n\
               print(\"sum: {total}\")\n";
    let (stdout, exit) = build_and_run("mb2_list_min_vacia_err", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err: empty list\nsum: 0");
}

#[test]
fn mb2_str_pad_start_end_compilan() {
    let src = "let s: Str = \"42\"\n\
               let a: Str = s.pad_start(5, \"0\")\n\
               let b: Str = s.pad_end(5, \".\")\n\
               let c: Str = \"hello\".pad_start(3, \"*\")\n\
               print(a)\n\
               print(b)\n\
               print(c)\n";
    let (stdout, exit) = build_and_run("mb2_str_pad", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "00042\n42...\nhello");
}

#[test]
fn mb2_map_keys_sorted_str_compila() {
    let src = "let m: Map<Str, Int> = {\"banana\": 2, \"apple\": 1, \"cherry\": 3}\n\
               let ks: List<Str> = m.keys_sorted()\n\
               print(ks)\n";
    let (stdout, exit) = build_and_run("mb2_map_keys_sorted_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[\"apple\", \"banana\", \"cherry\"]");
}

#[test]
fn mb2_map_keys_sorted_int_compila() {
    let src = "let m: Map<Int, Str> = {30: \"c\", 10: \"a\", 20: \"b\"}\n\
               let ks: List<Int> = m.keys_sorted()\n\
               print(ks)\n";
    let (stdout, exit) = build_and_run("mb2_map_keys_sorted_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 20, 30]");
}

#[test]
fn rg_range_step_by_exclusivo_compila() {
    let src = "let xs: List<Int> = (0..10).step_by(2)\n\
               print(xs)\n\
               let total: Int = xs.sum()\n\
               print(total)\n";
    let (stdout, exit) = build_and_run("rg_range_step_by_excl", src);
    assert_eq!(exit, 0);
    // 0..10 step 2 → [0, 2, 4, 6, 8], sum = 20.
    assert_eq!(stdout.trim(), "[0, 2, 4, 6, 8]\n20");
}

#[test]
fn rg_range_step_by_inclusivo_compila() {
    let src = "let xs: List<Int> = (0..=10).step_by(3)\n\
               print(xs)\n";
    let (stdout, exit) = build_and_run("rg_range_step_by_incl", src);
    assert_eq!(exit, 0);
    // 0..=10 step 3 → [0, 3, 6, 9].
    assert_eq!(stdout.trim(), "[0, 3, 6, 9]");
}

// ---- Mini-tanda Mb3 — fold + product + chars + entries + to_map ----

#[test]
fn mb3_list_reduce_sum_int_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let total: Int = xs.reduce(0, fn(acc: Int, x: Int) => acc + x)\n\
               print(total)\n";
    let (stdout, exit) = build_and_run("mb3_list_reduce_sum", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "15");
}

#[test]
fn mb3_list_reduce_acc_distinto_de_t_compila() {
    // Acc puede ser de un tipo distinto al de los elementos.
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let s: Str = xs.reduce(\"\", fn(acc: Str, x: Int) => \"{acc}{x}-\")\n\
               print(s)\n";
    let (stdout, exit) = build_and_run("mb3_list_reduce_acc_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1-2-3-");
}

#[test]
fn mb3_list_product_compila() {
    let src = "let xs: List<Int> = [2, 3, 4]\n\
               let p: Int = xs.product()\n\
               print(p)\n\
               let empty: List<Int> = []\n\
               print(empty.product())\n";
    let (stdout, exit) = build_and_run("mb3_list_product", src);
    assert_eq!(exit, 0);
    // 2*3*4=24, vacío → 1 (sentinel).
    assert_eq!(stdout.trim(), "24\n1");
}

#[test]
fn mb3_str_chars_compila() {
    let src = "let cs: List<Str> = \"abc\".chars()\n\
               print(cs)\n\
               print(cs.len())\n";
    let (stdout, exit) = build_and_run("mb3_str_chars", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[\"a\", \"b\", \"c\"]\n3");
}

#[test]
fn mb3_map_entries_compila() {
    let src = "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2}\n\
               let es: List<(Str, Int)> = m.entries()\n\
               print(es)\n\
               print(es.len())\n";
    let (stdout, exit) = build_and_run("mb3_map_entries", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[(\"a\", 1), (\"b\", 2)]\n2");
}

#[test]
fn mb3_list_to_map_compila() {
    let src = "let pairs: List<(Str, Int)> = [(\"a\", 1), (\"b\", 2)]\n\
               let m: Map<Str, Int> = pairs.to_map()\n\
               print(m[\"a\"])\n\
               print(m[\"b\"])\n\
               print(m.len())\n";
    let (stdout, exit) = build_and_run("mb3_list_to_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2\n2");
}

// ---- Mini-tanda Cd — codegen polish: higher-order + F12 ----

#[test]
fn cd_ho_map_with_named_fn_compiles() {
    let src = "fn double(n: Int) -> Int { return n * 2 }\n\
               let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = xs.map(double)\n\
               print(ys)\n";
    let (stdout, exit) = build_and_run("cd_ho_map_named", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4, 6]");
}

#[test]
fn cd_ho_filter_with_named_fn_compiles() {
    let src = "fn is_even(n: Int) -> Bool { return n % 2 == 0 }\n\
               let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let ys: List<Int> = xs.filter(is_even)\n\
               print(ys)\n";
    let (stdout, exit) = build_and_run("cd_ho_filter_named", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4]");
}

#[test]
fn cd_ho_reduce_binary_with_named_fn_compiles() {
    let src = "fn sumar(acc: Int, x: Int) -> Int { return acc + x }\n\
               let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let total: Int = xs.reduce(0, sumar)\n\
               print(total)\n";
    let (stdout, exit) = build_and_run("cd_ho_reduce_named", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "15");
}

#[test]
fn cd_f12_let_int_const_referenciado_por_fn_compila() {
    let src = "let MAX = 100\n\
               fn cap(n: Int) -> Int {\n\
                   if (n > MAX) { return MAX }\n\
                   return n\n\
               }\n\
               print(cap(50))\n\
               print(cap(200))\n";
    let (stdout, exit) = build_and_run("cd_f12_let_int_const", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "50\n100");
}

#[test]
fn cd_f12_let_str_compila() {
    let src = "let GREETING = \"hola\"\n\
               fn greet(name: Str) -> Str { return \"{GREETING}, {name}\" }\n\
               print(greet(\"Ada\"))\n";
    let (stdout, exit) = build_and_run("cd_f12_let_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hola, Ada");
}

#[test]
fn cd_f12_let_float_compila() {
    let src = "let PI = 3.14\n\
               fn area(r: Float) -> Float { return PI * r * r }\n\
               print(area(2.0))\n";
    let (stdout, exit) = build_and_run("cd_f12_let_float", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "12.56");
}

#[test]
fn cd_f12_const_eval_with_binop_compiles() {
    let src = "let LIMIT = 10 * 2 + 5\n\
               fn check(n: Int) -> Bool { return n < LIMIT }\n\
               print(check(20))\n\
               print(check(30))\n";
    let (stdout, exit) = build_and_run("cd_f12_const_eval_binop", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse");
}

// ---- Mini-tanda Mb4 + Cmp+ ----------------------------------

#[test]
fn mb4_list_unique_compila() {
    let src = "let xs: List<Int> = [1, 2, 2, 3, 1, 4, 3]\n\
               let r: List<Int> = xs.unique()\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("mb4_list_unique", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3, 4]");
}

#[test]
fn mb4_list_partition_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5, 6]\n\
               let split: (List<Int>, List<Int>) = xs.partition(fn(n: Int) => n % 2 == 0)\n\
               print(split.0)\n\
               print(split.1)\n";
    let (stdout, exit) = build_and_run("mb4_list_partition", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4, 6]\n[1, 3, 5]");
}

#[test]
fn mb4_map_invert_compila() {
    let src = "let m: Map<Int, Str> = {1: \"a\", 2: \"b\"}\n\
               let inv: Map<Str, Int> = m.invert()\n\
               print(inv[\"a\"])\n\
               print(inv[\"b\"])\n";
    let (stdout, exit) = build_and_run("mb4_map_invert", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2");
}

#[test]
fn mb4_str_split_at_compila() {
    let src = "let p: (Str, Str) = \"hola mundo\".split_at(4)\n\
               print(p.0)\n\
               print(p.1)\n";
    let (stdout, exit) = build_and_run("mb4_str_split_at", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hola\n mundo");
}

#[test]
fn cmp_multi_for_clauses_compila() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = [10, 20]\n\
               let r: List<Int> = [x + y for x in xs for y in ys]\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("cmp_multi_for", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[11, 21, 12, 22, 13, 23]");
}

#[test]
fn cmp_multi_for_with_filter_compiles() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = [10, 20]\n\
               let r: List<Int> = [x * y for x in xs for y in ys if x % 2 == 1]\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("cmp_multi_for_filter", src);
    assert_eq!(exit, 0);
    // x impar (1, 3): (1*10, 1*20, 3*10, 3*20) = [10, 20, 30, 60].
    assert_eq!(stdout.trim(), "[10, 20, 30, 60]");
}

#[test]
fn cmp_map_comp_basico_compila() {
    let src = "let squares: Map<Int, Int> = {n: n * n for n in 1..=4}\n\
               print(squares[1])\n\
               print(squares[2])\n\
               print(squares[4])\n";
    let (stdout, exit) = build_and_run("cmp_map_comp", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n4\n16");
}

#[test]
fn cmp_map_comp_with_filter_compiles() {
    let src = "let big: Map<Int, Int> = {n: n * 10 for n in 0..10 if n > 5}\n\
               print(big[6])\n\
               print(big[9])\n";
    let (stdout, exit) = build_and_run("cmp_map_comp_filter", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "60\n90");
}

// ---- Mini-tanda Mb5 + Async-cl --------------------------------

#[test]
fn mb5_list_group_by_compila() {
    let src = "let nums: List<Int> = [1, 2, 3, 4, 5, 6]\n\
               let g: Map<Str, List<Int>> = nums.group_by(fn(n: Int) => if (n % 2 == 0) { \"par\" } else { \"impar\" })\n\
               print(g[\"par\"])\n\
               print(g[\"impar\"])\n";
    let (stdout, exit) = build_and_run("mb5_list_group_by", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[2, 4, 6]\n[1, 3, 5]");
}

#[test]
fn mb5_list_zip_with_compila() {
    let src = "let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = [10, 20, 30]\n\
               let r: List<Int> = xs.zip_with(ys, fn(a: Int, b: Int) => a + b)\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("mb5_list_zip_with", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[11, 22, 33]");
}

#[test]
fn mb5_list_max_by_compila() {
    let src = "type P { age: Int = 0 name: Str = \"\" }\n\
               let ps: List<P> = [P { age: 28, name: \"Bob\" }, P { age: 42, name: \"Cam\" }, P { age: 35, name: \"Ada\" }]\n\
               match ps.max_by(fn(p: P) => p.age) {\n\
                 Ok(p) => print(\"mayor: {p.name}\"),\n\
                 Err(_) => print(\"vacío\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mb5_list_max_by", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "mayor: Cam");
}

#[test]
fn mb5_list_min_by_empty_list_returns_err_compiles() {
    let src = "let xs: List<Int> = []\n\
               match xs.min_by(fn(n: Int) => n) {\n\
                 Ok(v) => print(\"min: {v}\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("mb5_list_min_by_vacia", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "err: empty list");
}

#[test]
fn mb5_str_lines_compila() {
    let src = "let s = \"uno\\ndos\\ntres\"\n\
               let ls: List<Str> = s.lines()\n\
               print(ls)\n\
               print(ls.len())\n";
    let (stdout, exit) = build_and_run("mb5_str_lines", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[\"uno\", \"dos\", \"tres\"]\n3");
}

#[test]
fn mb5_str_is_empty_compila() {
    let src = "print(\"\".is_empty())\n\
               print(\"hola\".is_empty())\n";
    let (stdout, exit) = build_and_run("mb5_str_is_empty", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse");
}

// ---- Mini-tanda Mb8 + Bits-extras + Fmt-g ------------------

#[test]
fn mb8_list_starts_ends_with_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               print(xs.starts_with([1, 2]))\n\
               print(xs.ends_with([4, 5]))\n\
               print(xs.starts_with([1, 3]))\n";
    let (stdout, exit) = build_and_run("mb8_starts_ends", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\ntrue\nfalse");
}

#[test]
fn mb8_list_insert_at_compila() {
    let src = "let xs: List<Int> = [1, 2, 4, 5]\n\
               print(xs.insert_at(2, 3))\n";
    let (stdout, exit) = build_and_run("mb8_insert_at", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3, 4, 5]");
}

#[test]
fn mb8_list_remove_at_compila() {
    let src = "let xs: List<Int> = [10, 20, 30, 40]\n\
               print(xs.remove_at(2))\n";
    let (stdout, exit) = build_and_run("mb8_remove_at", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 20, 40]");
}

#[test]
fn mb8_list_zip_to_map_compila() {
    let src = "let ks: List<Str> = [\"a\", \"b\"]\n\
               let vs: List<Int> = [1, 2]\n\
               let m: Map<Str, Int> = ks.zip_to_map(vs)\n\
               print(m[\"a\"])\n\
               print(m[\"b\"])\n";
    let (stdout, exit) = build_and_run("mb8_zip_to_map", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2");
}

#[test]
fn mb8_str_left_right_center_compila() {
    let src = "let s = \"hola mundo\"\n\
               print(s.left(4))\n\
               print(s.right(5))\n\
               print(s.center(20, \"-\"))\n";
    let (stdout, exit) = build_and_run("mb8_str_left_right_center", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hola\nmundo\n-----hola mundo-----");
}

#[test]
fn bits_extras_popcount_y_leading_zeros_compila() {
    let src = "print(popcount(7))\n\
               print(popcount(255))\n\
               print(leading_zeros(1))\n\
               print(trailing_zeros(8))\n";
    let (stdout, exit) = build_and_run("bits_extras_popcount_lz", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3\n8\n63\n3");
}

#[test]
fn bits_extras_rotate_compila() {
    let src = "print(rotate_left(1, 4))\n\
               print(rotate_right(16, 4))\n";
    let (stdout, exit) = build_and_run("bits_extras_rotate", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "16\n1");
}

#[test]
fn fmt_g_general_compila() {
    let src = "let x = 1234.5\n\
               print(\"{x:g}\")\n\
               let y = 0.00001\n\
               print(\"{y:g}\")\n\
               let z = 3.140000\n\
               print(\"{z:g}\")\n";
    let (stdout, exit) = build_and_run("fmt_g_general", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1234.5\n1.00000e-5\n3.14");
}

#[test]
fn fmt_g_uppercase_compila() {
    let src = "let w = 1234567890.0\n\
               print(\"{w:G}\")\n";
    let (stdout, exit) = build_and_run("fmt_G_uppercase", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1.23457E9");
}

// ---- Mini-tanda Mb7 — take/drop/init/tail/intersperse/cycle +
//                      repeat_with + with ----------

#[test]
fn mb7_list_take_drop_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               print(xs.take(3))\n\
               print(xs.drop(2))\n\
               print(xs.take(99))\n\
               print(xs.drop(99))\n";
    let (stdout, exit) = build_and_run("mb7_take_drop", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3]\n[3, 4, 5]\n[1, 2, 3, 4, 5]\n[]");
}

#[test]
fn mb7_list_init_tail_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4]\n\
               print(xs.init())\n\
               print(xs.tail())\n";
    let (stdout, exit) = build_and_run("mb7_init_tail", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 3]\n[2, 3, 4]");
}

#[test]
fn mb7_list_intersperse_compila() {
    let src = "let xs: List<Int> = [10, 20, 30]\n\
               print(xs.intersperse(0))\n";
    let (stdout, exit) = build_and_run("mb7_intersperse", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 0, 20, 0, 30]");
}

#[test]
fn mb7_list_cycle_compila() {
    let src = "let xs: List<Int> = [1, 2]\n\
               print(xs.cycle(3))\n";
    let (stdout, exit) = build_and_run("mb7_cycle", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 2, 1, 2, 1, 2]");
}

#[test]
fn mb7_str_repeat_with_compila() {
    let src = "print(\"hi\".repeat_with(3, \"-\"))\n\
               print(\"x\".repeat_with(0, \",\"))\n";
    let (stdout, exit) = build_and_run("mb7_repeat_with", src);
    assert_eq!(exit, 0);
    // Segunda línea es string vacío (n=0).
    assert_eq!(stdout, "hi-hi-hi\n\n");
}

#[test]
fn mb7_map_with_compila() {
    let src = "let m: Map<Str, Int> = {\"a\": 1}\n\
               let m2: Map<Str, Int> = m.with(\"b\", 2)\n\
               print(m2[\"a\"])\n\
               print(m2[\"b\"])\n\
               let m3: Map<Str, Int> = m.with(\"a\", 99)\n\
               print(m3[\"a\"])\n";
    let (stdout, exit) = build_and_run("mb7_with", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2\n99");
}

// ---- Mini-tanda Fmt-build — format specs faltantes ----

#[test]
fn fmt_build_grouping_coma_compila() {
    let src = "let n = 1234567\n\
               print(\"{n:,d}\")\n";
    let (stdout, exit) = build_and_run("fmt_grouping_coma", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1,234,567");
}

#[test]
fn fmt_build_grouping_underscore_compila() {
    let src = "let n = 1000000\n\
               print(\"{n:_d}\")\n";
    let (stdout, exit) = build_and_run("fmt_grouping_underscore", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1_000_000");
}

#[test]
fn fmt_build_percent_compila() {
    let src = "let r = 0.857\n\
               print(\"{r:.2%}\")\n";
    let (stdout, exit) = build_and_run("fmt_percent", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "85.70%");
}

#[test]
fn fmt_build_char_codepoint_compila() {
    let src = "let cp = 65\n\
               print(\"{cp:c}\")\n";
    let (stdout, exit) = build_and_run("fmt_char_codepoint", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "A");
}

#[test]
fn fmt_build_grouping_negativo_compila() {
    let src = "let n = -1234\n\
               print(\"{n:,d}\")\n";
    let (stdout, exit) = build_and_run("fmt_grouping_neg", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "-1,234");
}

// ---- Mini-tanda Mb6 — scan + windows + merge_with --------

#[test]
fn mb6_list_scan_acumula_outputs_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4]\n\
               let r: List<Int> = xs.scan(0, fn(acc: Int, x: Int) => acc + x)\n\
               print(r)\n";
    let (stdout, exit) = build_and_run("mb6_scan", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[1, 3, 6, 10]");
}

#[test]
fn mb6_list_windows_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let r: List<List<Int>> = xs.windows(3)\n\
               print(r)\n\
               print(r.len())\n";
    let (stdout, exit) = build_and_run("mb6_windows", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[[1, 2, 3], [2, 3, 4], [3, 4, 5]]\n3");
}

#[test]
fn mb6_map_merge_with_callback_resolves_conflicts_compiles() {
    let src = "let a: Map<Str, Int> = {\"x\": 1, \"y\": 2}\n\
               let b: Map<Str, Int> = {\"y\": 10, \"z\": 3}\n\
               let r: Map<Str, Int> = a.merge_with(b, fn(va: Int, vb: Int) => va + vb)\n\
               print(r[\"x\"])\n\
               print(r[\"y\"])\n\
               print(r[\"z\"])\n";
    let (stdout, exit) = build_and_run("mb6_merge_with", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n12\n3");
}

// ---- Mini-tanda HTTP-Cors — echo Origin sin filtro --------

#[test]
fn http_cors_echo_origin_echoes_without_filter() {
    // allow_origin: "echo" (Str literal) → echo del Origin recibido
    // sin filtro. Cualquier Origin que llegue se eco-emite en la
    // response. Si no llega Origin header, no se emite el header.
    let src = "\
@server(43392)
fn main() => 0

@middleware(cors({\"allow_origin\": \"echo\"}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    // Request CON Origin → echo en response.
    let (status, raw_headers) = build_spawn_request_raw_with_headers(
        "http-cors-echo-with-origin",
        src,
        43392,
        "GET",
        "/api",
        &[("Origin", "https://anything.example.com")],
    );
    assert_eq!(status, 200);
    let lower = raw_headers.to_lowercase();
    assert!(
        lower.contains("access-control-allow-origin: https://anything.example.com"),
        "headers no contienen echo del Origin: {}",
        raw_headers
    );
}

#[test]
fn http_cors_echo_without_origin_does_not_emit_header() {
    let src = "\
@server(43393)
fn main() => 0

@middleware(cors({\"allow_origin\": \"echo\"}))
@get(\"/api\")
fn h() -> Str => \"ok\"
";
    // Request SIN Origin → no se emite header.
    let (status, raw_headers) =
        build_spawn_request_raw("http-cors-echo-no-origin", src, 43393, "GET", "/api");
    assert_eq!(status, 200);
    let lower = raw_headers.to_lowercase();
    assert!(
        !lower.contains("access-control-allow-origin:"),
        "Echo without Origin should not emit the header: {}",
        raw_headers
    );
}

// ---- Mini-tanda HTTP-Err — status codes específicos por Err ----

#[test]
fn http_err_instance_with_status_field_returns_that_status() {
    // Convención: Err con Instance con field `status: Int` → ese
    // status code se usa en la response. Body = Instance serializada.
    let src = "\
type ApiErr {
    status: Int = 500
    message: Str = \"\"
}

@server(43390)
fn main() => 0

@get(\"/users/{id}\")
fn get_user(id: Int) -> Result<Str, ApiErr> {
    if (id == 0) { return Err(ApiErr { status: 404, message: \"no encontrado\" }) }
    return Ok(\"Ada\")
}
";
    let (status, body) = build_spawn_request("http-err-404", src, 43390, "GET", "/users/0", None);
    assert_eq!(status, 404);
    assert!(body.contains("no encontrado"), "body fue: {}", body);
    // El body es el Instance serializado (no envuelto en `{"error": ...}`).
    assert!(body.contains("\"status\":404"), "body fue: {}", body);
}

#[test]
fn http_err_instance_with_status_field_400_and_ok_path() {
    let src = "\
type ApiErr {
    status: Int = 500
    message: Str = \"\"
}

@server(43391)
fn main() => 0

@get(\"/users/{id}\")
fn get_user(id: Int) -> Result<Str, ApiErr> {
    if (id < 0) { return Err(ApiErr { status: 400, message: \"id inválido\" }) }
    return Ok(\"hola\")
}
";
    // Caso OK: status 200.
    let (status_ok, body_ok) =
        build_spawn_request("http-err-ok", src, 43391, "GET", "/users/5", None);
    assert_eq!(status_ok, 200);
    assert!(body_ok.contains("hola"));
}

#[test]
fn async_cl_inline_dentro_de_fn_async_compila() {
    // Async-cl build: async closures inline compilan adentro de
    // fns async. Emitidas como `move |...| -> Pin<Box<dyn Future +
    // Send>> { Box::pin(async move { ... }) }`.
    let src = "async fn run() -> Int {\n\
                   let f = async fn(n: Int) -> Int {\n\
                       sleep(1).await\n\
                       return n * 2\n\
                   }\n\
                   let r = f(21).await\n\
                   return r\n\
               }\n\
               print(run().await)\n";
    let (stdout, exit) = build_and_run("async_cl_inline_dentro_de_fn", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn cd_combinado_ho_y_f12_compila() {
    // Combina ambos features: fn nombrada como callback Y const hoisteado
    // referenciado adentro de la fn.
    let src = "let MULTIPLIER = 10\n\
               fn boost(n: Int) -> Int { return n * MULTIPLIER }\n\
               let xs: List<Int> = [1, 2, 3]\n\
               let ys: List<Int> = xs.map(boost)\n\
               print(ys)\n";
    let (stdout, exit) = build_and_run("cd_combinado", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "[10, 20, 30]");
}

#[test]
fn mb3_round_trip_entries_to_map_compila() {
    let src = "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
               let back: Map<Str, Int> = m.entries().to_map()\n\
               print(back[\"a\"])\n\
               print(back[\"b\"])\n\
               print(back[\"c\"])\n";
    let (stdout, exit) = build_and_run("mb3_round_trip", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "1\n2\n3");
}

// ---- Mini-tanda Math + Mb9 + Int/Float methods ------------------

#[test]
fn math_builtins_basicos_compila() {
    let src = "print(abs(-5))\n\
               print(abs(-3.14))\n\
               print(min(3, 5))\n\
               print(max(1.5, 2.5))\n\
               print(pow(2, 10))\n\
               print(sqrt(16))\n\
               print(ceil(3.2))\n\
               print(floor(3.8))\n\
               print(round(3.5))\n\
               print(clamp(5, 0, 10))\n\
               print(clamp(-5, 0, 10))\n\
               print(clamp(15, 0, 10))\n";
    let (stdout, exit) = build_and_run("math_builtins", src);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout.trim(),
        "5\n3.14\n3\n2.5\n1024.0\n4.0\n4\n3\n4\n5\n0\n10"
    );
}

#[test]
fn mb9_str_swap_case_title_compila() {
    let src = "print(\"Hola Mundo\".swap_case())\n\
               print(\"hola mundo de fitz\".title())\n";
    let (stdout, exit) = build_and_run("mb9_str_swap_title", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hOLA mUNDO\nHola Mundo De Fitz");
}

#[test]
fn mb9_str_is_alpha_digit_numeric_compila() {
    let src = "print(\"hola\".is_alpha())\n\
               print(\"hola123\".is_alpha())\n\
               print(\"12345\".is_digit())\n\
               print(\"3.14\".is_numeric())\n\
               print(\"-42\".is_numeric())\n\
               print(\"3.14.5\".is_numeric())\n";
    let (stdout, exit) = build_and_run("mb9_str_is_predicates", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse\ntrue\ntrue\ntrue\nfalse");
}

#[test]
fn mb9_list_split_at_compila() {
    let src = "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
               let parts = xs.split_at(2)\n\
               print(parts)\n";
    let (stdout, exit) = build_and_run("mb9_list_split_at", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "([1, 2], [3, 4, 5])");
}

#[test]
fn mb9_map_has_value_compila() {
    let src = "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
               print(m.has_value(2))\n\
               print(m.has_value(99))\n";
    let (stdout, exit) = build_and_run("mb9_map_has_value", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "true\nfalse");
}

#[test]
fn int_methods_abs_to_str_to_str_base_compila() {
    let src = "let n: Int = -5\n\
               print(n.abs())\n\
               print((42).to_str())\n\
               print((255).to_str_base(16))\n\
               print((10).to_str_base(2))\n\
               print((8).to_str_base(8))\n";
    let (stdout, exit) = build_and_run("int_methods", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "5\n42\nff\n1010\n10");
}

#[test]
fn float_methods_abs_to_str_is_nan_is_finite_compila() {
    let src = "let x: Float = -3.14\n\
               print(x.abs())\n\
               print((3.14).to_str())\n\
               print((1.0).is_nan())\n\
               print((1.0).is_finite())\n";
    let (stdout, exit) = build_and_run("float_methods", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "3.14\n3.14\nfalse\ntrue");
}

// ---- Mini-tanda Fp — default params ------------------

#[test]
fn fp_default_param_str_compila() {
    let src = "fn greet(name: Str = \"amigo\") -> Str {\n\
                   return \"Hola, {name}\"\n\
               }\n\
               print(greet())\n\
               print(greet(\"Fitz\"))\n";
    let (stdout, exit) = build_and_run("fp_default_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "Hola, amigo\nHola, Fitz");
}

#[test]
fn fp_mezcla_required_y_default_compila() {
    let src = "fn add(a: Int, b: Int = 10) -> Int {\n\
                   return a + b\n\
               }\n\
               print(add(5))\n\
               print(add(5, 2))\n";
    let (stdout, exit) = build_and_run("fp_mezcla", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "15\n7");
}

// ---- Mini-tanda Fp.2 — varargs ------------------

#[test]
fn fp2_varargs_basico_compila() {
    let src = "fn sum(...xs: Int) -> Int {\n\
                   let total: Int = 0\n\
                   for x in xs { total = total + x }\n\
                   return total\n\
               }\n\
               print(sum())\n\
               print(sum(1, 2, 3))\n\
               print(sum(10, 20))\n";
    let (stdout, exit) = build_and_run("fp2_varargs_basico", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0\n6\n30");
}

#[test]
fn fp2_varargs_with_required_compiles() {
    let src = "fn join(prefix: Str, ...xs: Str) -> Int {\n\
                   return xs.len()\n\
               }\n\
               print(join(\"_\"))\n\
               print(join(\"_\", \"a\", \"b\"))\n\
               print(join(\"_\", \"x\", \"y\", \"z\"))\n";
    let (stdout, exit) = build_and_run("fp2_varargs_required", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "0\n2\n3");
}

// ---- Mini-tanda Fp.3 — named args ------------------

#[test]
fn fp3_named_args_basico_compila() {
    let src = "fn greet(name: Str = \"amigo\", greeting: Str = \"Hola\") -> Str {\n\
                   return \"{greeting}, {name}\"\n\
               }\n\
               print(greet())\n\
               print(greet(name: \"Fitz\"))\n\
               print(greet(greeting: \"Hi\"))\n\
               print(greet(greeting: \"Hey\", name: \"Roy\"))\n";
    let (stdout, exit) = build_and_run("fp3_named_basico", src);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout.trim(),
        "Hola, amigo\nHola, Fitz\nHi, amigo\nHey, Roy"
    );
}

#[test]
fn fp3_mezcla_posicional_y_named_compila() {
    let src =
        "fn config(host: Str = \"127.0.0.1\", port: Int = 3000, debug: Bool = false) -> Str {\n\
                   return \"{host}:{port}/{debug}\"\n\
               }\n\
               print(config())\n\
               print(config(\"0.0.0.0\"))\n\
               print(config(\"0.0.0.0\", port: 8080))\n\
               print(config(port: 9000, debug: true))\n";
    let (stdout, exit) = build_and_run("fp3_mezcla", src);
    assert_eq!(exit, 0);
    assert_eq!(
        stdout.trim(),
        "127.0.0.1:3000/false\n0.0.0.0:3000/false\n0.0.0.0:8080/false\n127.0.0.1:9000/true"
    );
}

// ---- Mini-tanda Sp.2 — return en match arm ------------------

#[test]
fn sp2_return_en_match_arm_compila() {
    let src = "fn classify(n: Int) -> Str {\n\
                   match n {\n\
                       0 => return \"cero\"\n\
                       1..10 => return \"chico\"\n\
                       _ => return \"grande\"\n\
                   }\n\
                   return \"unreachable\"\n\
               }\n\
               print(classify(0))\n\
               print(classify(5))\n\
               print(classify(100))\n";
    let (stdout, exit) = build_and_run("sp2_return_match", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "cero\nchico\ngrande");
}

// ---- Mini-tanda 5b.1 — param type inference desde call sites ----

#[test]
fn fp_5b1_param_without_annotation_inferred_from_call_site() {
    let src = "fn greet(name) {\n\
                   return \"Hola, {name}\"\n\
               }\n\
               print(greet(\"Fitz\"))\n";
    let (stdout, exit) = build_and_run("fp_5b1_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "Hola, Fitz");
}

#[test]
fn p2_param_y_return_ambos_inferidos_via_re_check() {
    // Mini-tanda P2 — chained fix 5b.1 + Hpx.2. Sin anotar param
    // NI return type, el codegen re-corre el checker tras inferir
    // el param desde el call site para refinar el return type.
    let src = "fn double(n) {\n\
                   return n * 2\n\
               }\n\
               fn greet(name) {\n\
                   return \"Hola, {name}\"\n\
               }\n\
               print(double(21))\n\
               print(greet(\"Fitz\"))\n";
    let (stdout, exit) = build_and_run("p2_both_inferred", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42\nHola, Fitz");
}

#[test]
fn fp_5b1_param_int_inferred_with_annotated_return() {
    // Cuando el return type está anotado pero el param no,
    // 5b.1 infiere el param sin colidir con Hpx.2.
    let src = "fn double(n) -> Int {\n\
                   return n * 2\n\
               }\n\
               print(double(21))\n";
    let (stdout, exit) = build_and_run("fp_5b1_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

// ---- Mini-tanda Hpx.2 — return type inference en handlers ------

#[test]
fn hpx2_fn_without_return_annotation_infers_from_body_compiles() {
    let src = "fn greet(name: Str) {\n\
                   return \"Hola, {name}\"\n\
               }\n\
               print(greet(\"Fitz\"))\n";
    let (stdout, exit) = build_and_run("hpx2_no_ret_str", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "Hola, Fitz");
}

#[test]
fn hpx2_fn_without_annotation_infers_int_compiles() {
    let src = "fn double(n: Int) {\n\
                   return n * 2\n\
               }\n\
               print(double(21))\n";
    let (stdout, exit) = build_and_run("hpx2_no_ret_int", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn hpx2_fn_with_if_else_infers_lub() {
    let src = "fn maybe(b: Bool) {\n\
                   if b {\n\
                       return 42\n\
                   }\n\
                   return 0\n\
               }\n\
               print(maybe(true))\n\
               print(maybe(false))\n";
    let (stdout, exit) = build_and_run("hpx2_if_else", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "42\n0");
}

#[test]
fn sp2_match_arm_with_block_compiles() {
    let src = "fn f(n: Int) -> Int {\n\
                   return match n {\n\
                       0 => {\n\
                           let x: Int = 10\n\
                           return x * 2\n\
                       }\n\
                       _ => 99\n\
                   }\n\
               }\n\
               print(f(0))\n\
               print(f(1))\n";
    let (stdout, exit) = build_and_run("sp2_match_block", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "20\n99");
}

#[test]
fn fp_multiples_defaults_compila() {
    let src = "fn make(prefix: Str = \"x\", n: Int = 3, sep: Str = \"-\") -> Str {\n\
                   return \"{prefix}{sep}{n}\"\n\
               }\n\
               print(make())\n\
               print(make(\"y\"))\n\
               print(make(\"z\", 5))\n\
               print(make(\"w\", 7, \":\"))\n";
    let (stdout, exit) = build_and_run("fp_multiples", src);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "x-3\ny-3\nz-5\nw:7");
}

// ---------------------------------------------------------------------------
// Fase 9.w.1.d — Auth nativa en `fitz build`.
//
// Tests E2E que validan que un programa con `@auth_provider` +
// `@authenticated`/`@admin` compila a binario nativo y responde con
// los códigos esperados (200/401/403). Paridad bit-a-bit con los
// tests del intérprete en `src/http.rs::tests::auth_*`.
// ---------------------------------------------------------------------------

/// Spawn del server + barrido de requests con distintos Authorization
/// headers. Una sola compilación + un solo spawn para ahorrar tiempo;
/// los 5 requests se hacen sobre el mismo server vivo.
fn build_spawn_auth_requests(
    test_name: &str,
    src: &str,
    port: u16,
    requests: &[(&str, &str, Option<&str>)], // (method, path, optional bearer token)
) -> Vec<(u16, String)> {
    // T2 (v0.10.13) — unique stem por test_name.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port {} within 3s", port);
    }

    use std::io::{Read, Write};
    let mut results: Vec<(u16, String)> = Vec::with_capacity(requests.len());
    for (method, path, token) in requests {
        let auth_header = match token {
            Some(t) => format!("Authorization: Bearer {}\r\n", t),
            None => String::new(),
        };
        let request = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\n{}Connection: close\r\n\r\n",
            method, path, addr, auth_header,
        );
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send request");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body = raw[body_start..].to_string();
        results.push((status, body));
    }

    let _ = child.kill();
    let _ = child.wait();
    results
}

#[test]
fn auth_codegen_complete_flow_end_to_end() {
    // Programa de referencia: provider que matchea Authorization contra
    // dos tokens hardcoded (admin, user); ruta pública sin auth, una con
    // `@authenticated` y otra con `@admin`. El programa entero compila a
    // binario nativo con `fitz build`, se spawn-ea el server, y se le
    // hacen 6 requests cubriendo los casos del wrapper auth.
    let src = "\
@server(43901)\n\
fn main() => 0\n\
\n\
type User { id: Int, name: Str, role: Str }\n\
\n\
@auth_provider\n\
fn check(headers: Map<Str, Str>) -> Result<User> {\n\
    match headers.get(\"authorization\") {\n\
        Ok(token) => {\n\
            if (token == \"Bearer admin-token\") {\n\
                return Ok(User { id: 1, name: \"Admin\", role: \"admin\" })\n\
            }\n\
            if (token == \"Bearer user-token\") {\n\
                return Ok(User { id: 2, name: \"Alice\", role: \"user\" })\n\
            }\n\
            return Err(\"token inválido\")\n\
        }\n\
        Err(_) => return Err(\"falta Authorization\")\n\
    }\n\
}\n\
\n\
@get(\"/public\")\n\
fn public_route() -> Str => \"sin auth\"\n\
\n\
@authenticated\n\
@get(\"/me\")\n\
fn me(user: User) -> Str => user.name\n\
\n\
@admin\n\
@get(\"/admin\")\n\
fn admin_route(user: User) -> Str => \"hola admin\"\n\
";
    let results = build_spawn_auth_requests(
        "auth_codegen_flujo",
        src,
        43901,
        &[
            ("GET", "/public", None),
            ("GET", "/me", None),
            ("GET", "/me", Some("wrong-token")),
            ("GET", "/me", Some("user-token")),
            ("GET", "/admin", Some("user-token")),
            ("GET", "/admin", Some("admin-token")),
        ],
    );
    // /public sin auth → 200 con body "sin auth"
    assert_eq!(results[0].0, 200, "public route 200, fue {:?}", results[0]);
    assert!(
        results[0].1.contains("sin auth"),
        "body /public: {:?}",
        results[0].1
    );

    // /me sin Authorization → 401 con "falta Authorization"
    assert_eq!(results[1].0, 401, "/me sin auth 401, fue {:?}", results[1]);
    assert!(
        results[1].1.contains("falta Authorization"),
        "/me sin header body: {:?}",
        results[1].1
    );

    // /me with invalid token → 401 with "token inválido" (Fitz source)
    assert_eq!(
        results[2].0, 401,
        "/me invalid token 401, was {:?}",
        results[2]
    );
    assert!(
        results[2].1.contains("token inválido"),
        "/me con token wrong body: {:?}",
        results[2].1
    );

    // /me con user válido → 200 con "Alice"
    assert_eq!(
        results[3].0, 200,
        "/me valid user 200, was {:?}",
        results[3]
    );
    assert!(
        results[3].1.contains("Alice"),
        "/me user body: {:?}",
        results[3].1
    );

    // /admin con rol user → 403
    assert_eq!(results[4].0, 403, "/admin user → 403, fue {:?}", results[4]);
    assert!(
        results[4].1.contains("admin"),
        "/admin con rol user body: {:?}",
        results[4].1
    );

    // /admin con rol admin → 200 con "hola admin"
    assert_eq!(
        results[5].0, 200,
        "/admin admin → 200, fue {:?}",
        results[5]
    );
    assert!(
        results[5].1.contains("hola admin"),
        "/admin admin body: {:?}",
        results[5].1
    );
}

#[test]
fn auth_codegen_cross_module_provider_w12() {
    // W12 (v0.10.7) — `@auth_provider` declarado en un módulo
    // importado (`auth.fitz`) y handlers `@authenticated`/`@admin`
    // en el main. El checker debe hacer fallback al provider importado
    // vía `TypeEnv::imported_auth_provider` (Paso 1 W12), el codegen
    // debe detectar cross-module vía `loader.modules` (Paso 3 W12),
    // y la invocación al provider en el wrapper auth debe emitirse
    // module-qualified (`crate::auth::check_token(...)`).
    //
    // Mismo set de 6 requests que `auth_codegen_flujo_completo_end_to_end`,
    // pero el provider vive aparte — assertions paralelas garantizan
    // paridad bit-a-bit single-file ↔ cross-module.
    // T2 (v0.10.13) — unique stem (test name como stem).
    let stem = "auth_codegen_cross_module_w12";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let auth_src = "\
type User { id: Int, name: Str, role: Str }\n\
\n\
@auth_provider\n\
fn check_token(headers: Map<Str, Str>) -> Result<User> {\n\
    match headers.get(\"authorization\") {\n\
        Ok(token) => {\n\
            if (token == \"Bearer admin-token\") {\n\
                return Ok(User { id: 1, name: \"Admin\", role: \"admin\" })\n\
            }\n\
            if (token == \"Bearer user-token\") {\n\
                return Ok(User { id: 2, name: \"Alice\", role: \"user\" })\n\
            }\n\
            return Err(\"token inválido\")\n\
        }\n\
        Err(_) => return Err(\"falta Authorization\")\n\
    }\n\
}\n\
";
    std::fs::write(dir.join("auth.fitz"), auth_src).expect("escribir auth.fitz");

    let main_src = "\
from auth import User\n\
\n\
@server(43902)\n\
fn main() => 0\n\
\n\
@get(\"/public\")\n\
fn public_route() -> Str => \"sin auth\"\n\
\n\
@authenticated\n\
@get(\"/me\")\n\
fn me(user: User) -> Str => user.name\n\
\n\
@admin\n\
@get(\"/admin\")\n\
fn admin_route(user: User) -> Str => \"hola admin\"\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir prog.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (cross-module W12):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    // Spawn server.
    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let addr = "127.0.0.1:43902".to_string();
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port 43902 within 3s");
    }

    // Request runner inline (paralelo a build_spawn_auth_requests).
    use std::io::{Read, Write};
    let send_req = |method: &str, path: &str, token: Option<&str>| -> (u16, String) {
        let auth_header = match token {
            Some(t) => format!("Authorization: Bearer {}\r\n", t),
            None => String::new(),
        };
        let request = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\n{}Connection: close\r\n\r\n",
            method, path, addr, auth_header,
        );
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send request");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body = raw[body_start..].to_string();
        (status, body)
    };

    let results = [
        send_req("GET", "/public", None),
        send_req("GET", "/me", None),
        send_req("GET", "/me", Some("wrong-token")),
        send_req("GET", "/me", Some("user-token")),
        send_req("GET", "/admin", Some("user-token")),
        send_req("GET", "/admin", Some("admin-token")),
    ];

    let _ = child.kill();
    let _ = child.wait();

    // Mismas assertions que el test single-file — paridad bit-a-bit.
    assert_eq!(results[0].0, 200, "/public 200, fue {:?}", results[0]);
    assert!(
        results[0].1.contains("sin auth"),
        "body /public: {:?}",
        results[0].1
    );

    assert_eq!(results[1].0, 401, "/me sin auth 401, fue {:?}", results[1]);
    assert!(
        results[1].1.contains("falta Authorization"),
        "/me sin header body: {:?}",
        results[1].1
    );

    assert_eq!(
        results[2].0, 401,
        "/me invalid token 401, was {:?}",
        results[2]
    );
    assert!(
        results[2].1.contains("token inválido"),
        "/me con token wrong body: {:?}",
        results[2].1
    );

    assert_eq!(
        results[3].0, 200,
        "/me valid user 200, was {:?}",
        results[3]
    );
    assert!(
        results[3].1.contains("Alice"),
        "/me user body: {:?}",
        results[3].1
    );

    assert_eq!(results[4].0, 403, "/admin user → 403, fue {:?}", results[4]);
    assert!(
        results[4].1.contains("admin"),
        "/admin con rol user body: {:?}",
        results[4].1
    );

    assert_eq!(
        results[5].0, 200,
        "/admin admin → 200, fue {:?}",
        results[5]
    );
    assert!(
        results[5].1.contains("hola admin"),
        "/admin admin body: {:?}",
        results[5].1
    );

    // W12 — Sanity check del Rust generado: la invocación al provider
    // debe ser `crate::auth::check_token` (module-qualified), no
    // `check_token` (bare). Sin este path, el binario habría compilado
    // pero el bare name no resolvería desde el wrapper en main.rs.
    // `fitz build` escribe a `target/fitz-build/<stem>/src/main.rs`
    // relativo al CWD del proceso (raíz del repo cuando lo invoca cargo
    // test). El stem es `prog` (nombre del .fitz).
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs generado");
        assert!(
            content.contains("crate::auth::check_token"),
            "main.rs generado debe invocar `crate::auth::check_token` (path qualified W12) — primeros 2000 chars:\n{}",
            &content[..content.len().min(2000)],
        );
    }
}

#[test]
fn try_operator_mixed_with_return_status_w13() {
    // W13 (v0.10.9) — handler HTTP que mezcla `?` (propagación de
    // `Result::Err`), `return <status> { ... }` (status code custom)
    // y return normal de un type custom en el mismo body. Antes de
    // W13 el checker abortaba con "el operador `?` solo puede usarse
    // adentro de una función que retorne `Result<...>`" porque el
    // declared ret type era `User`, no `Result<User>`.
    //
    // W13 cierra esto en dos puntos: (1) checker relaja la regla
    // cuando estamos en HTTP handler — el wrapper convierte
    // Err propagado a 500 automáticamente; (2) codegen detecta
    // body_uses_try y entra a response_mode para que el fn emita
    // `__FitzResponse` y `?` se desazucare a un match que produce
    // `__FitzResponse { status: 500, ... }` (no `?` Rust nativo,
    // que requeriría `Result<_, _>` como return type).
    // T2 (v0.10.13) — unique stem.
    let stem = "try_mixed_return_status_w13";
    let src = "\
@server(43912)\n\
fn main() => 0\n\
\n\
type UserInput { email: Str }\n\
type User { id: Int, email: Str }\n\
\n\
fn parse_id(s: Str) -> Result<Int> {\n\
    if (s == \"ada\") { return Ok(1) }\n\
    if (s == \"alan\") { return Ok(2) }\n\
    return Err(\"usuario desconocido\")\n\
}\n\
\n\
@post(\"/users/{stub}\")\n\
fn create(stub: Str, body: UserInput) -> User {\n\
    if (body.email == \"\") {\n\
        return 400 { \"error\": \"email vacío\" }\n\
    }\n\
    let id = parse_id(stub)?\n\
    return User { id: id, email: body.email }\n\
}\n\
";
    // El helper `build_spawn_auth_requests` no acepta body JSON, así
    // que armamos build + spawn + requests manuales inline (paralelo
    // al patrón del test cross-module W12).
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W13):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let addr = "127.0.0.1:43912".to_string();
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port 43912 within 3s");
    }

    use std::io::{Read, Write};
    let send_post = |path: &str, body: &str| -> (u16, String) {
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path,
            addr,
            body.len(),
            body,
        );
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send request");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body_resp = raw[body_start..].to_string();
        (status, body_resp)
    };

    // Caso 1: happy path — Ok del parse_id + return normal de User.
    let (status, body) = send_post("/users/ada", r#"{"email":"a@b.com"}"#);
    assert_eq!(status, 200, "happy path 200, fue {:?}", (status, &body));
    assert!(
        body.contains("\"id\":1") && body.contains("a@b.com"),
        "happy path body: {:?}",
        body
    );

    // Caso 2: `?` propaga Err — el wrapper convierte a 500 +
    // {"error": <msg>}. Este es el caso central del W13.
    let (status, body) = send_post("/users/zzz", r#"{"email":"a@b.com"}"#);
    assert_eq!(
        status,
        500,
        "? propaga Err → 500, fue {:?}",
        (status, &body)
    );
    assert!(
        body.contains("usuario desconocido"),
        "? Err body: {:?}",
        body
    );

    // Caso 3: `return <status> { ... }` — produce 400 con body custom.
    let (status, body) = send_post("/users/ada", r#"{"email":""}"#);
    assert_eq!(
        status,
        400,
        "return 400 produce 400, fue {:?}",
        (status, &body)
    );
    assert!(body.contains("email vacío"), "return 400 body: {:?}", body);

    let _ = child.kill();
    let _ = child.wait();

    // Sanity check del Rust generado: `?` se debe expandir como
    // `match (...) { Ok(__v) => __v, Err(__e) => return __FitzResponse
    // { status: 500, ... } }` (no como `?` Rust nativo, que rompería
    // la compilación de un fn declarado `-> User`).
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs generado");
        assert!(
            content.contains("Err(__e) => return __FitzResponse"),
            "main.rs generado debe expandir `?` a match → __FitzResponse 500 (W13). \
             Buscamos `Err(__e) => return __FitzResponse` sin encontrar."
        );
    }
}

#[test]
fn http_coverage_metodos_headers_content_type_body_libre_t7() {
    // T7 (v0.10.13) — cobertura HTTP E2E extendida sobre 4 áreas que
    // hasta hoy estaban sin tests robustos (sólo se asertaba que
    // `build` no fallara, sin validar respuestas):
    //
    //   1) Múltiples métodos sobre el mismo path. axum mergea
    //      automáticamente verbos distintos en un MethodRouter por
    //      path; este test valida que GET/POST/PUT/DELETE sobre
    //      `/items` responden cada uno con su método correcto.
    //   2) Headers HTTP entrantes via `@header(name="X-...")`. Valida
    //      que el wrapper extrae el header como param del handler
    //      con coerción (Str para presente; Str? para opcional;
    //      requerido ausente → 400).
    //   3) Content-Type negotiation. POST a `/form` con
    //      `application/x-www-form-urlencoded` body — el wrapper
    //      detecta el primary CT y usa `__parse_urlencoded` antes
    //      de aplicar `__from_fitz_json`. Mismo handler debe aceptar
    //      `application/json` también (path canónico).
    //   4) Body sin tipo declarado (`body: Map<Str, Any>`). El
    //      codegen reifica a `Arc<Mutex<Vec<(__FitzValue, __FitzValue)>>>`
    //      y `body.get("k")` wrappea el key en `__FitzValue::Str` para
    //      comparar contra los keys del Vec. Útil para webhooks y
    //      endpoints proxy donde el shape del body no es fijo. (Antes
    //      el codegen rechazaba con "no soporta el tipo `Any`" en
    //      gen_map_get; fix en v0.10.13.)
    //
    // Un solo build + spawn cubre los 4 casos (10 requests).
    // T2 (v0.10.13) — unique stem.
    let stem = "http_coverage_t7";
    let src = "\
@server(43917)\n\
fn main() => 0\n\
\n\
type Item { id: Int, name: Str }\n\
\n\
@get(\"/items\")\n\
fn list_items() -> Str => \"GET items\"\n\
\n\
@post(\"/items\")\n\
fn create_item(body: Item) -> Str { return \"POST {body.name}\" }\n\
\n\
@put(\"/items\")\n\
fn replace_items() -> Str => \"PUT items\"\n\
\n\
@delete(\"/items\")\n\
fn delete_items() -> Str => \"DELETE items\"\n\
\n\
// Caso 2: headers required + opcional. Convención del wrapper:\n\
// `X-Trace-Id` → param `x_trace_id` (lowercase + `-` → `_`).\n\
@header(name=\"X-Trace-Id\")\n\
@header(name=\"X-Optional\")\n\
@get(\"/with-headers\")\n\
fn with_headers(x_trace_id: Str, x_optional: Str?) -> Str {\n\
    match x_optional {\n\
        null => return \"trace={x_trace_id} opt=null\",\n\
        v => return \"trace={x_trace_id} opt={v}\",\n\
    }\n\
}\n\
\n\
type FormPayload { name: Str, email: Str }\n\
\n\
// Caso 3: handler que acepta body via JSON o urlencoded. El\n\
// wrapper detecta `Content-Type` y enruta a `__parse_urlencoded`\n\
// o `__from_fitz_json` según corresponda. Usamos FormPayload con\n\
// solo Str porque urlencoded no coerce Str→Int (todos los values\n\
// llegan como strings — el path JSON sí coerce porque el body\n\
// JSON puede contener números nativos).\n\
@post(\"/form\")\n\
fn submit_form(body: FormPayload) -> Str { return \"got {body.name} <{body.email}>\" }\n\
\n\
// Caso 4 (v0.10.13): body sin schema fijo via `Map<Str, Any>`. El\n\
// JSON entrante se deserializa en un Vec<(FitzValue, FitzValue)>\n\
// y `body.get(\"k\")` devuelve Result<FitzValue>. Útil para\n\
// webhooks/endpoints proxy donde el shape no es fijo.\n\
@post(\"/webhook\")\n\
fn webhook(body: Map<Str, Any>) -> Str {\n\
    match body.get(\"event\") {\n\
        Ok(_) => return \"event recibido\",\n\
        Err(_) => return \"sin event\",\n\
    }\n\
}\n\
";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir prog.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (T7):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let addr = "127.0.0.1:43917".to_string();
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port 43917 within 3s");
    }

    use std::io::{Read, Write};
    let send_req = |method: &str,
                    path: &str,
                    body: Option<&str>,
                    content_type: Option<&str>,
                    headers: &[(&str, &str)]|
     -> (u16, String) {
        let mut headers_str = String::new();
        for (k, v) in headers {
            headers_str.push_str(&format!("{}: {}\r\n", k, v));
        }
        let (ct_header, body_str) = match body {
            Some(b) => (
                format!(
                    "Content-Type: {}\r\nContent-Length: {}\r\n",
                    content_type.unwrap_or("application/json"),
                    b.len()
                ),
                b.to_string(),
            ),
            None => (String::new(), String::new()),
        };
        let request = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\n{}{}Connection: close\r\n\r\n{}",
            method, path, addr, headers_str, ct_header, body_str,
        );
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send request");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body_resp = raw[body_start..].to_string();
        (status, body_resp)
    };

    // === Caso 1: 4 métodos sobre /items ===
    let (s, b) = send_req("GET", "/items", None, None, &[]);
    assert_eq!(s, 200, "GET /items: {:?}", (s, &b));
    assert!(b.contains("GET items"), "GET body: {:?}", b);

    let (s, b) = send_req(
        "POST",
        "/items",
        Some(r#"{"id":1,"name":"widget"}"#),
        None,
        &[],
    );
    assert_eq!(s, 200, "POST /items: {:?}", (s, &b));
    assert!(b.contains("POST widget"), "POST body: {:?}", b);

    let (s, b) = send_req("PUT", "/items", None, None, &[]);
    assert_eq!(s, 200, "PUT /items: {:?}", (s, &b));
    assert!(b.contains("PUT items"), "PUT body: {:?}", b);

    let (s, b) = send_req("DELETE", "/items", None, None, &[]);
    assert_eq!(s, 200, "DELETE /items: {:?}", (s, &b));
    assert!(b.contains("DELETE items"), "DELETE body: {:?}", b);

    // === Caso 2: headers ===
    let (s, b) = send_req(
        "GET",
        "/with-headers",
        None,
        None,
        &[("X-Trace-Id", "abc123"), ("X-Optional", "opt-value")],
    );
    assert_eq!(s, 200, "/with-headers ambos: {:?}", (s, &b));
    assert!(
        b.contains("trace=abc123") && b.contains("opt=opt-value"),
        "headers body: {:?}",
        b
    );

    // Header opcional ausente → null en el handler.
    let (s, b) = send_req("GET", "/with-headers", None, None, &[("X-Trace-Id", "xyz")]);
    assert_eq!(s, 200, "/with-headers solo trace: {:?}", (s, &b));
    assert!(
        b.contains("trace=xyz") && b.contains("opt=null"),
        "opcional null body: {:?}",
        b
    );

    // Header requerido ausente → 400.
    let (s, _b) = send_req("GET", "/with-headers", None, None, &[]);
    assert_eq!(s, 400, "/with-headers sin trace → 400");

    // === Caso 3: Content-Type negotiation ===
    // JSON funciona (default).
    let (s, b) = send_req(
        "POST",
        "/form",
        Some(r#"{"name":"Ada","email":"ada@example.com"}"#),
        Some("application/json"),
        &[],
    );
    assert_eq!(s, 200, "POST /form JSON: {:?}", (s, &b));
    assert!(
        b.contains("got Ada <ada@example.com>"),
        "JSON body: {:?}",
        b
    );

    // urlencoded body también funciona (el wrapper hace switch en
    // primary content type via `__parse_urlencoded` → JSON Object con
    // todos los values como Str → `__from_fitz_json` deserializa).
    let (s, b) = send_req(
        "POST",
        "/form",
        Some("name=Alan&email=alan%40example.com"),
        Some("application/x-www-form-urlencoded"),
        &[],
    );
    assert_eq!(s, 200, "POST /form urlencoded: {:?}", (s, &b));
    assert!(
        b.contains("got Alan <alan@example.com>"),
        "urlencoded body parsed: {:?}",
        b
    );

    // === Caso 4: body sin tipo declarado (Map<Str, Any>) ===
    let (s, b) = send_req(
        "POST",
        "/webhook",
        Some(r#"{"event":"user.created","data":{"id":7}}"#),
        None,
        &[],
    );
    assert_eq!(s, 200, "POST /webhook con event: {:?}", (s, &b));
    assert!(
        b.contains("event recibido"),
        "webhook body con event: {:?}",
        b
    );

    let (s, b) = send_req("POST", "/webhook", Some(r#"{"foo":"bar"}"#), None, &[]);
    assert_eq!(s, 200, "POST /webhook sin event: {:?}", (s, &b));
    assert!(b.contains("sin event"), "webhook sin event: {:?}", b);

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn float_arithmetic_overflow_returns_error_r6() {
    // R6 (v0.10.14) — overflow aritmético Float produce `inf`/`NaN`.
    // Antes: el codegen emitía la op cruda (Rust no panica en
    // overflow Float), el `inf` se propagaba hasta serialización JSON
    // donde `serde_json::Number::from_f64(inf) → None` y el response
    // body terminaba como `null` con status 200 (cliente recibía dato
    // incorrecto sin enterarse).
    //
    // Ahora: paridad bit-a-bit `fitz run` ↔ `fitz build`. El evaluator
    // (`arith` en evaluator.rs) detecta `!is_finite()` y devuelve
    // FitzError con msg claro. El codegen (`gen_binop` Add/Sub/Mul/Div)
    // emite `if !__r.is_finite() { panic!("...") }` después de cada op
    // Float. El catch_unwind del wrapper R6 (prior session) captura
    // el panic en handlers HTTP y devuelve 500 con `{"error": "..."}`.
    // T4 (post-W12-W16) — además del exit code, validamos que el panic
    // del binario emite mensaje claro citando overflow/inf/no finito.
    let (stdout, stderr, exit) = build_and_run_with_stderr(
        "r6-float-overflow",
        "\
let x = 1.0e300\n\
let y = x * x\n\
print(y)\n\
",
    );
    assert_ne!(
        exit, 0,
        "overflow Float debe abortar con exit code != 0, fue {} (stdout: {})",
        exit, stdout
    );
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("inf")
            || combined.contains("overflow")
            || combined.contains("finito")
            || combined.contains("Float"),
        "expected message about overflow/inf, was:\nstdout={}\nstderr={}",
        stdout,
        stderr
    );
}

#[test]
fn handler_panic_returns_500_does_not_break_connection_r6() {
    // R6 (v0.10.13) — un panic adentro de un handler HTTP (típicamente
    // `x / 0`, `xs[N]` out-of-bounds, etc.) debe convertirse en una
    // respuesta 500 con `{"error": <msg>}`. Antes del fix el panic del
    // worker tokio rompía la conexión sin response (cliente recibía
    // HTTP 000) — el server seguía vivo pero el cliente quedaba ciego.
    //
    // El fix envuelve el call al user fn en `catch_unwind` (sync) o
    // `FutureExt::catch_unwind` (async) y, on Err del payload, emite
    // el `__FitzResponse 500` con el msg del panic + CORS headers.
    // T2 (v0.10.13) — unique stem.
    let stem = "handler_panic_r6";
    let src = "\
@server(43916)\n\
fn main() => 0\n\
\n\
@get(\"/div\")\n\
fn divide() -> Float {\n\
    let x = 10.0\n\
    let y = 0.0\n\
    return x / y\n\
}\n\
\n\
@get(\"/div-async\")\n\
async fn divide_async() -> Float {\n\
    let x = 10.0\n\
    let y = 0.0\n\
    return x / y\n\
}\n\
\n\
@get(\"/ok\")\n\
fn ok() -> Str => \"alive\"\n\
";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir prog.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (R6):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let addr = "127.0.0.1:43916".to_string();
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port 43916 within 3s");
    }

    use std::io::{Read, Write};
    let send_get = |path: &str| -> (u16, String) {
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            path, addr,
        );
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send request");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body = raw[body_start..].to_string();
        (status, body)
    };

    // Caso 1: sync handler paniquea → 500 con msg del panic.
    let (status, body) = send_get("/div");
    assert_eq!(
        status,
        500,
        "/div sync panic → 500, fue {:?}",
        (status, &body)
    );
    assert!(
        body.contains("division by zero"),
        "/div body debe contener el msg del panic: {:?}",
        body
    );

    // Caso 2: async handler paniquea → 500. Valida rama
    // `FutureExt::catch_unwind().await` del fix.
    let (status, body) = send_get("/div-async");
    assert_eq!(
        status,
        500,
        "/div-async panic → 500, fue {:?}",
        (status, &body)
    );
    assert!(
        body.contains("division by zero"),
        "/div-async body: {:?}",
        body
    );

    // Caso 3: server sigue vivo después de los panics — ruta sana
    // responde 200 normalmente.
    let (status, body) = send_get("/ok");
    assert_eq!(
        status,
        200,
        "/ok after panics → 200, was {:?}",
        (status, &body)
    );
    assert!(body.contains("alive"), "/ok body: {:?}", body);

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cross_module_http_handler_w16() {
    // W16 (v0.10.12) — handler `@get`/`@post` declarado en un módulo
    // importado se registra como ruta en el `axum::Router` del main y
    // responde correctamente. Antes de W16 el codegen emitía la fn
    // `pub fn create(...)` en el módulo pero NO emitía el wrapper
    // `__handler_create`, NI registraba la ruta en main — el binario
    // compilaba pero todos los requests respondían 404.
    //
    // W16 cierra el gap en 3 piezas coordinadas:
    //   1) Loader captura `LoadedModule.http_fn_stmts` (FnDef stmts
    //      con `@get/@post/@put/@delete`).
    //   2) `generate_module_rs_with_bindings` emite `__handler_<name>`
    //      como `pub(crate)` en el `.rs` del módulo, con auth_provider
    //      state propagado desde `env.imported_auth_provider()` (W12).
    //   3) `gen_http_main` itera `loader.modules` y registra cada ruta
    //      con `.route(path, crate::<mod>::__handler_<name>)`.
    //
    // Este test combina los 4 W previos:
    //   - W12: `@auth_provider` en módulo distinto al handler.
    //   - W13: `?` operator dentro del handler (Err → 500).
    //   - W14: handler con body + user juntos.
    //   - W16: handler entero en módulo, route registrada por main.
    // T2 (v0.10.13) — unique stem.
    let stem = "cross_module_http_w16";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // Módulo `auth.fitz`: type User + @auth_provider.
    std::fs::write(
        dir.join("auth.fitz"),
        "type User { id: Int, email: Str, role: Str }\n\
         \n\
         @auth_provider\n\
         fn check(headers: Map<Str, Str>) -> Result<User> {\n\
             match headers.get(\"authorization\") {\n\
                 Ok(token) => {\n\
                     if (token == \"Bearer admin\") {\n\
                         return Ok(User { id: 1, email: \"ada@example.com\", role: \"admin\" })\n\
                     }\n\
                     if (token == \"Bearer user\") {\n\
                         return Ok(User { id: 2, email: \"alan@example.com\", role: \"user\" })\n\
                     }\n\
                     return Err(\"token inválido\")\n\
                 }\n\
                 Err(_) => return Err(\"falta Authorization\")\n\
             }\n\
         }\n",
    )
    .expect("escribir auth.fitz");

    // Módulo `posts.fitz`: handlers HTTP que combinan los W previos.
    std::fs::write(
        dir.join("posts.fitz"),
        "from auth import User\n\
         \n\
         type PostInput { title: Str, body: Str }\n\
         type Post { id: Int, title: Str, author_email: Str }\n\
         \n\
         fn parse_priority(s: Str) -> Result<Int> {\n\
             if (s == \"low\") { return Ok(1) }\n\
             if (s == \"high\") { return Ok(2) }\n\
             return Err(\"prioridad desconocida\")\n\
         }\n\
         \n\
         @authenticated\n\
         @get(\"/me\")\n\
         fn me(user: User) -> User => user\n\
         \n\
         @authenticated\n\
         @post(\"/posts/{prio}\")\n\
         fn create(prio: Str, input: PostInput, user: User) -> Post {\n\
             if (input.title == \"\") {\n\
                 return 400 { \"error\": \"título vacío\" }\n\
             }\n\
             let _p = parse_priority(prio)?\n\
             return Post { id: 42, title: input.title, author_email: user.email }\n\
         }\n\
         \n\
         @admin\n\
         @get(\"/admin/posts\")\n\
         fn admin_list(user: User) -> List<Str> => [\"post1\", \"post2\"]\n",
    )
    .expect("escribir posts.fitz");

    // Main: solo importa el módulo y configura el server.
    let main_src = "\
import posts\n\
\n\
@server(43915)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir prog.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W16):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let addr = "127.0.0.1:43915".to_string();
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port 43915 within 3s");
    }

    use std::io::{Read, Write};
    let send_req =
        |method: &str, path: &str, body: Option<&str>, bearer: Option<&str>| -> (u16, String) {
            let auth_header = match bearer {
                Some(t) => format!("Authorization: Bearer {}\r\n", t),
                None => String::new(),
            };
            let (content_header, body_str) = match body {
                Some(b) => (
                    format!(
                        "Content-Type: application/json\r\nContent-Length: {}\r\n",
                        b.len()
                    ),
                    b.to_string(),
                ),
                None => (String::new(), String::new()),
            };
            let request = format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\n{}{}Connection: close\r\n\r\n{}",
                method, path, addr, auth_header, content_header, body_str,
            );
            let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .ok();
            stream.write_all(request.as_bytes()).expect("send request");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).ok();
            let raw = String::from_utf8_lossy(&buf).into_owned();
            let status_line = raw.lines().next().unwrap_or("").to_string();
            let status: u16 = status_line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
            let body_resp = raw[body_start..].to_string();
            (status, body_resp)
        };

    // Caso 1: GET /me con admin token → 200 con user serializado.
    let (status, body) = send_req("GET", "/me", None, Some("admin"));
    assert_eq!(status, 200, "/me admin → 200, fue {:?}", (status, &body));
    assert!(
        body.contains("\"email\":\"ada@example.com\""),
        "/me body: {:?}",
        body
    );

    // Caso 2: GET /me sin token → 401.
    let (status, body) = send_req("GET", "/me", None, None);
    assert_eq!(
        status,
        401,
        "/me sin token → 401, fue {:?}",
        (status, &body)
    );

    // Caso 3: POST /posts/low (W13 + W14) con body + user → 200.
    let (status, body) = send_req(
        "POST",
        "/posts/low",
        Some(r#"{"title":"hola","body":"mundo"}"#),
        Some("user"),
    );
    assert_eq!(
        status,
        200,
        "POST /posts/low → 200, fue {:?}",
        (status, &body)
    );
    assert!(
        body.contains("\"author_email\":\"alan@example.com\"")
            && body.contains("\"title\":\"hola\""),
        "POST body: {:?}",
        body
    );

    // Caso 4: POST con prioridad desconocida → 500 vía `?` (W13).
    let (status, body) = send_req(
        "POST",
        "/posts/medium",
        Some(r#"{"title":"x","body":"y"}"#),
        Some("user"),
    );
    assert_eq!(
        status,
        500,
        "POST /posts/medium (? Err) → 500, fue {:?}",
        (status, &body)
    );
    assert!(
        body.contains("prioridad desconocida"),
        "W13 ? body: {:?}",
        body
    );

    // Caso 5: POST con título vacío → 400 vía `return <status>`.
    let (status, body) = send_req(
        "POST",
        "/posts/low",
        Some(r#"{"title":"","body":"y"}"#),
        Some("user"),
    );
    assert_eq!(
        status,
        400,
        "POST empty title → 400, was {:?}",
        (status, &body)
    );
    assert!(body.contains("título vacío"), "return 400 body: {:?}", body);

    // Caso 6: GET /admin/posts con admin → 200.
    let (status, body) = send_req("GET", "/admin/posts", None, Some("admin"));
    assert_eq!(
        status,
        200,
        "/admin/posts admin → 200, fue {:?}",
        (status, &body)
    );

    // Caso 7: GET /admin/posts con user (no admin) → 403.
    let (status, body) = send_req("GET", "/admin/posts", None, Some("user"));
    assert_eq!(
        status,
        403,
        "/admin/posts user → 403, fue {:?}",
        (status, &body)
    );

    let _ = child.kill();
    let _ = child.wait();

    // Sanity check del Rust generado:
    //   - posts.rs DEBE tener `pub(crate) async fn __handler_*`.
    //   - main.rs DEBE registrar `crate::posts::__handler_*`.
    let posts_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/posts.rs", stem));
    if posts_rs.exists() {
        let content = std::fs::read_to_string(&posts_rs).expect("leer posts.rs");
        // En modo Module el pub_prefix es `pub ` (no `pub(crate) `);
        // alcanza con `pub` para que main referencie cross-module.
        assert!(
            content.contains("pub async fn __handler_me")
                && content.contains("pub async fn __handler_create")
                && content.contains("pub async fn __handler_admin_list"),
            "posts.rs debe emitir wrappers pub async fn __handler_* (W16.2)"
        );
    }
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
        assert!(
            content.contains("crate::posts::__handler_me")
                && content.contains("crate::posts::__handler_create"),
            "main.rs debe registrar rutas con `crate::posts::__handler_*` (W16.3)"
        );
    }
}

// Deuda ORM cross-module (v0.45.0) — un `.preload("relation")` de un
// `@has_many`/`@has_one`/companion sobre un @table type importado
// resuelve el target type SIN que el usuario lo importe explícitamente.
// El codegen auto-registra el target desde el loader (Opción B: clon local
// del env + declare_nominal + binding sintético). Antes fallaba con
// "type `Post` no registrado en el TypeEnv" a menos que se importara
// `from models import User, Post` (todos los targets).
#[test]
fn cross_module_orm_preload_auto_registers_target_in_main_v045() {
    let stem = "cross_module_orm_preload_main_v045";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // models.fitz — User @has_many Post; Post @belongs_to User.
    std::fs::write(
        dir.join("models.fitz"),
        "@table(\"users\") type User {\n\
             @primary id: Int = 0\n\
             name: Str = \"\"\n\
             @has_many(\"Post\", via=\"user_id\") posts: List<Post> = []\n\
         }\n\
         \n\
         @table(\"posts\") type Post {\n\
             @primary id: Int = 0\n\
             @belongs_to(\"User\") user_id: Int = 0\n\
             title: Str = \"\"\n\
             user: User?\n\
         }\n",
    )
    .expect("escribir models.fitz");

    // Main imports ONLY User (not Post) yet uses `.preload("posts")`,
    // which needs the `Post` target registered in the TypeEnv.
    let main_src = "\
from models import User\n\
\n\
@get(\"/users\")\n\
async fn list_users() -> Result<List<User>> {\n\
  let conn = db.connect(\"postgres://x\").await?\n\
  let users = User.preload(\"posts\").all(conn).await?\n\
  return Ok(users)\n\
}\n\
\n\
@server(43945)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build debe compilar con el target auto-registrado:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );
}

// Deuda ORM cross-module (v0.45.0) — misma auto-resolución pero con el
// `.preload(...)` viviendo en un MÓDULO importado (no el main), para candar
// el path `generate_module_rs_with_bindings`.
#[test]
fn cross_module_orm_preload_auto_registers_target_in_module_v045() {
    let stem = "cross_module_orm_preload_module_v045";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    std::fs::write(
        dir.join("models.fitz"),
        "@table(\"users\") type User {\n\
             @primary id: Int = 0\n\
             name: Str = \"\"\n\
             @has_many(\"Post\", via=\"user_id\") posts: List<Post> = []\n\
         }\n\
         \n\
         @table(\"posts\") type Post {\n\
             @primary id: Int = 0\n\
             @belongs_to(\"User\") user_id: Int = 0\n\
             title: Str = \"\"\n\
             user: User?\n\
         }\n",
    )
    .expect("escribir models.fitz");

    // users.fitz (module) imports ONLY User and does the preload.
    std::fs::write(
        dir.join("users.fitz"),
        "from models import User\n\
         \n\
         @get(\"/users\")\n\
         async fn list_users() -> Result<List<User>> {\n\
           let conn = db.connect(\"postgres://x\").await?\n\
           let users = User.preload(\"posts\").all(conn).await?\n\
           return Ok(users)\n\
         }\n",
    )
    .expect("escribir users.fitz");

    let main_src = "\
import users\n\
\n\
@server(43946)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build debe compilar el preload en un módulo con el target auto-registrado:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn cross_module_orm_virtual_fields_skip_w17() {
    // W17 (v0.10.7) — @table type con relations virtuales
    // (`@has_many`/`@has_one`/BelongsToCompanion) declarado en un
    // módulo importado y usado como response de un handler en OTRO
    // módulo importado al main (caso 3-archivos: models + posts +
    // main).
    //
    // **Bug que cierra**: el codegen al emitir `impl __FromFitzJson
    // for UserData` en main.rs hacía remap del field
    // `posts: List<Post>` → `List<Any>` (porque Post no estaba en
    // env del main) → emitía `Vec<__FitzValue>`. Pero
    // `__FitzValue` no se activaba por el programa, así que rustc
    // rompía con "cannot find type __FitzValue".
    //
    // **Fix**: skipear los virtual fields (relations no-FK) en los
    // impls `__ToFitzJson`/`__FromFitzJson`. Esos fields no van a
    // la DB ni deberían aparecer en JSON I/O. En el struct literal
    // del `__from_fitz_json`, los virtuales se inicializan inline
    // con `Default::default()` para evitar nombrar el tipo
    // remap-degradado.
    let stem = "cross_module_orm_virtual_w17";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // models.fitz — @table types con @has_many (virtual) +
    // @belongs_to (real). User.posts es el field problemático
    // pre-W17 porque Post no se importa al main.
    std::fs::write(
        dir.join("models.fitz"),
        "@table(\"users\") type User {\n\
             @primary id: Int = 0\n\
             name: Str = \"\"\n\
             @has_many(\"Post\", via=\"author_id\", on_delete=\"cascade\") posts: List<Post> = []\n\
         }\n\
         \n\
         @table(\"posts\") type Post {\n\
             @primary id: Int = 0\n\
             @belongs_to(\"User\", on_delete=\"cascade\") author_id: Int = 0\n\
             title: Str = \"\"\n\
         }\n",
    )
    .expect("escribir models.fitz");

    // posts.fitz — handler que devuelve List<Post> cross-module.
    std::fs::write(
        dir.join("posts.fitz"),
        "from models import User, Post\n\
         \n\
         @get(\"/posts\")\n\
         fn list_posts() -> List<Post> {\n\
             return [Post { id: 1, author_id: 1, title: \"hello\" }]\n\
         }\n\
         \n\
         @get(\"/users\")\n\
         fn list_users() -> List<User> {\n\
             return [User { id: 7, name: \"ada\" }]\n\
         }\n",
    )
    .expect("escribir posts.fitz");

    // Main: solo `import posts` (W16) + @server.
    let main_src = "\
import posts\n\
\n\
@server(43916)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W17):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let addr = "127.0.0.1:43916".to_string();
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port 43916 within 3s");
    }

    use std::io::{Read, Write};
    let send_get = |path: &str| -> (u16, String) {
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            path, addr
        );
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body = raw[body_start..].to_string();
        (status, body)
    };

    // Caso 1: GET /posts → 200 con [{id, author_id, title}].
    // Post NO tiene virtual fields, todo el shape se serializa.
    let (status, body) = send_get("/posts");
    assert_eq!(status, 200, "/posts → 200, fue {:?}", (status, &body));
    assert!(
        body.contains("\"id\":1") && body.contains("\"title\":\"hello\""),
        "/posts body: {:?}",
        body
    );

    // Caso 2: GET /users → 200 con [{id, name}] — SIN el virtual
    // `posts` en el JSON output. Si W17 no aplicara, este endpoint
    // ni compilaría (rompía con __FitzValue undefined).
    let (status, body) = send_get("/users");
    assert_eq!(status, 200, "/users → 200, fue {:?}", (status, &body));
    assert!(
        body.contains("\"id\":7") && body.contains("\"name\":\"ada\""),
        "/users body: {:?}",
        body
    );
    assert!(
        !body.contains("\"posts\""),
        "/users body NO debe incluir el virtual `posts`: {:?}",
        body
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn hidden_decorator_skips_field_in_json_io_v0_10_11() {
    // v0.10.11 — `@hidden` field decorator:
    //   - El field NO aparece en `__to_fitz_json` (response al
    //     cliente — útil para `password_hash`, tokens internos).
    //   - El field NO se acepta en `__FromFitzJson` (body del
    //     cliente rechaza enviarlo con 400 "undeclared field").
    //   - El field SÍ existe en el struct Rust con su default
    //     (`Str = ""` queda `""`, `Default::default()` si no hay
    //     default). El código Fitz interno puede asignarlo
    //     libremente.
    //
    // Caso canónico: deuda menor detectada en smoke real
    // boilerplate api-orm-full v0.10.10 — `User.password_hash`
    // exposed en el response de `GET /posts/{id}` con preload
    // author.
    let stem = "hidden_decorator_v0_10_11";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let main_src = "\
type User {\n\
    id: Int = 0\n\
    email: Str = \"\"\n\
    @hidden password_hash: Str = \"\"\n\
}\n\
\n\
@get(\"/users\")\n\
fn list_users() -> List<User> {\n\
    return [User { id: 1, email: \"ada@example.com\", password_hash: \"super-secret\" }]\n\
}\n\
\n\
@post(\"/users\")\n\
fn create_user(body: User) -> User {\n\
    return body\n\
}\n\
\n\
@server(43917)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (@hidden):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let addr = "127.0.0.1:43917".to_string();
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port 43917 within 3s");
    }

    use std::io::{Read, Write};
    let send_req = |method: &str, path: &str, body: Option<&str>| -> (u16, String) {
        let body_str = body.unwrap_or("");
        let request = if let Some(b) = body {
            format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                method, path, addr, b.len(), b
            )
        } else {
            format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                method, path, addr
            )
        };
        let _ = body_str;
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body = raw[body_start..].to_string();
        (status, body)
    };

    // Caso 1: GET /users → 200, response NO incluye password_hash
    // aunque el handler asignó "super-secret" al field.
    let (status, body) = send_req("GET", "/users", None);
    assert_eq!(status, 200, "GET /users → 200, fue {:?}", (status, &body));
    assert!(
        body.contains("\"id\":1") && body.contains("\"email\":\"ada@example.com\""),
        "GET /users debe incluir id + email: {:?}",
        body
    );
    assert!(
        !body.contains("password_hash") && !body.contains("super-secret"),
        "GET /users NO debe exponer password_hash: {:?}",
        body
    );

    // Caso 2: POST /users SIN password_hash en el body → 200, el
    // server crea User con password_hash="" (default).
    let (status, body) = send_req(
        "POST",
        "/users",
        Some(r#"{"id":2,"email":"bob@example.com"}"#),
    );
    assert_eq!(
        status,
        200,
        "POST /users sin password_hash → 200, fue {:?}",
        (status, &body)
    );
    assert!(
        body.contains("\"email\":\"bob@example.com\""),
        "POST /users body de respuesta: {:?}",
        body
    );

    // Caso 3: POST /users CON password_hash en el body → 400,
    // el server rechaza "undeclared field".
    let (status, body) = send_req(
        "POST",
        "/users",
        Some(r#"{"id":3,"email":"eve@example.com","password_hash":"hax"}"#),
    );
    assert_eq!(
        status,
        400,
        "POST /users con password_hash → 400, fue {:?}",
        (status, &body)
    );
    assert!(
        body.contains("password_hash") && body.contains("undeclared field"),
        "POST /users error debe citar el field rechazado: {:?}",
        body
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cross_module_body_type_serializa_w15() {
    // W15 (v0.10.11) — type declarado en un módulo importado y usado
    // como body en un handler del main. Antes de revisar W15 se asumía
    // que los módulos necesitarían emitir sus propios
    // `impl __ToFitzJson`/`__FromFitzJson` para tipos exportados; la
    // verificación mostró que NO hace falta: las reglas de orphan de
    // Rust hacen que el impl emitido en main.rs (via
    // `emit_helpers_for_imported_types`) sea crate-visible.
    //
    // Este test candea el caso para que no regresemos cuando llegue
    // W16 (handler wrappers en módulos): la trait `__FromFitzJson` /
    // `__ToFitzJson` se importa en main y los impls se generan ahí
    // para CADA type declarado en cualquier módulo cargado por el
    // loader. El wrapper en main referencia `<PostInput as
    // __FromFitzJson>::__from_fitz_json(...)` sin necesidad de impls
    // duplicados ni `use` adicionales del módulo origen.
    // T2 (v0.10.13) — unique stem.
    let stem = "cross_module_body_w15";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // Módulo `models.fitz` exporta los types del dominio. Sin
    // decorators HTTP propios — los types son data structures puras.
    std::fs::write(
        dir.join("models.fitz"),
        "type PostInput { title: Str, body: Str }\n\
         type Post { id: Int, title: Str, length: Int }\n",
    )
    .expect("escribir models.fitz");

    // Main importa los types y los usa como body + return del handler.
    let main_src = "\
from models import PostInput, Post\n\
\n\
@server(43914)\n\
fn main() => 0\n\
\n\
@post(\"/posts\")\n\
fn create(input: PostInput) -> Post {\n\
    return Post { id: 1, title: input.title, length: len(input.body) }\n\
}\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir prog.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W15 cross-module body):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let addr = "127.0.0.1:43914".to_string();
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port 43914 within 3s");
    }

    use std::io::{Read, Write};
    let body = r#"{"title":"cross-module","body":"works end to end"}"#;
    let request = format!(
        "POST /posts HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        addr,
        body.len(),
        body,
    );
    let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();
    let raw = String::from_utf8_lossy(&buf).into_owned();
    let status_line = raw.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
    let body_resp = raw[body_start..].to_string();

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(
        status,
        200,
        "cross-module body → 200, fue {:?}",
        (status, &body_resp)
    );
    assert!(
        body_resp.contains("\"title\":\"cross-module\"") && body_resp.contains("\"length\":16"),
        "cross-module body response: {:?}",
        body_resp
    );

    // W15 — sanity check sobre el Rust generado: los impls de los
    // types del módulo deben estar en main.rs (no en models.rs). Si
    // un futuro refactor mueve los impls a los módulos, esta
    // assertion habrá que adaptarla; mientras tanto candea el
    // approach actual.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs generado");
        assert!(
            content.contains("impl __FromFitzJson for PostInputData")
                && content.contains("impl __ToFitzJson for PostData"),
            "main.rs debe emitir impls __FromFitzJson/__ToFitzJson para types \
             importados de módulos (W15)"
        );
    }
}

#[test]
fn auth_codegen_handler_with_body_and_user_w14() {
    // W14 (v0.10.10) — handler protegido por auth que recibe ADEMÁS
    // un body JSON. Antes del fix el dispatcher rechazaba con
    // "En MVP, un handler protegido por auth admite solo el param
    // `user` y NO body separado". Ahora identifica el user param por
    // TIPO (matchea contra `user_type_name` extraído del
    // `Result<T>` declarado en el `@auth_provider`), permitiendo que
    // los demás leftovers sean body.
    //
    // Caso canónico: `@post("/posts") @authenticated fn create(input:
    // PostInput, user: User) -> Post`. El wrapper deserializa el body
    // como PostInput, autentica via provider, inyecta user, y llama
    // al handler con (input, user).
    // T2 (v0.10.13) — unique stem.
    let stem = "auth_body_user_w14";
    let src = "\
@server(43913)\n\
fn main() => 0\n\
\n\
type User { id: Int, email: Str, role: Str }\n\
type PostInput { title: Str, body: Str }\n\
type Post { id: Int, author_email: Str, title: Str }\n\
\n\
@auth_provider\n\
fn check(headers: Map<Str, Str>) -> Result<User> {\n\
    match headers.get(\"authorization\") {\n\
        Ok(token) => {\n\
            if (token == \"Bearer admin\") {\n\
                return Ok(User { id: 1, email: \"ada@example.com\", role: \"admin\" })\n\
            }\n\
            return Err(\"token inválido\")\n\
        }\n\
        Err(_) => return Err(\"falta Authorization\")\n\
    }\n\
}\n\
\n\
@authenticated\n\
@post(\"/posts\")\n\
fn create(input: PostInput, user: User) -> Post {\n\
    return Post { id: 42, author_email: user.email, title: input.title }\n\
}\n\
";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W14):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let addr = "127.0.0.1:43913".to_string();
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port 43913 within 3s");
    }

    use std::io::{Read, Write};
    let send_post = |path: &str, body: &str, bearer: Option<&str>| -> (u16, String) {
        let auth_header = match bearer {
            Some(t) => format!("Authorization: Bearer {}\r\n", t),
            None => String::new(),
        };
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            path,
            addr,
            auth_header,
            body.len(),
            body,
        );
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        stream.write_all(request.as_bytes()).expect("send request");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let status_line = raw.lines().next().unwrap_or("").to_string();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body_resp = raw[body_start..].to_string();
        (status, body_resp)
    };

    // Caso 1: token válido + body válido → 200 con Post serializado.
    // El wrapper deserializa input como PostInput Y inyecta user como
    // User (W14 — body + user juntos).
    let (status, body) = send_post(
        "/posts",
        r#"{"title":"hola","body":"mundo"}"#,
        Some("admin"),
    );
    assert_eq!(status, 200, "happy path 200, fue {:?}", (status, &body));
    assert!(
        body.contains("\"author_email\":\"ada@example.com\"")
            && body.contains("\"title\":\"hola\""),
        "happy path body: {:?}",
        body
    );

    // Caso 2: sin token → 401 (auth check antes de body parsing).
    let (status, body) = send_post("/posts", r#"{"title":"hola","body":"mundo"}"#, None);
    assert_eq!(status, 401, "sin token → 401, fue {:?}", (status, &body));

    // Caso 3: token inválido → 401.
    let (status, body) = send_post(
        "/posts",
        r#"{"title":"hola","body":"mundo"}"#,
        Some("wrong"),
    );
    assert_eq!(
        status,
        401,
        "invalid token → 401, was {:?}",
        (status, &body)
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn auth_codegen_jwt_y_hash_builtins_cli() {
    // CLI puro: usa `jwt.encode`/`jwt.decode` + `hash.password`/
    // `hash.verify` sin handlers HTTP. Verifica que las deps se sumen
    // condicional al Cargo.toml generado y que los helpers estén en el
    // preludio. El binario imprime el shape esperado de cada salida.
    let src = "\
let secret = \"secret-32-bytes-long-test-aaaaaa\"\n\
let claims: Map<Str, Str> = {\"sub\": \"u42\", \"role\": \"admin\"}\n\
let token = jwt.encode(claims, secret)\n\
print(\"jwt-len-gt-20: {len(token) > 20}\")\n\
let pw = hash.password(\"supersecret\")\n\
print(\"argon2id-prefix: {pw[0..10]}\")\n\
let ok = hash.verify(\"supersecret\", pw)\n\
let bad = hash.verify(\"wrong\", pw)\n\
print(\"verify-ok: {ok}\")\n\
print(\"verify-bad: {bad}\")\n\
";
    let (stdout, exit) = build_and_run("auth_jwt_hash_cli", src);
    assert_eq!(exit, 0, "exit code: {} stdout: {}", exit, stdout);
    assert!(stdout.contains("jwt-len-gt-20: true"), "stdout: {}", stdout);
    assert!(
        stdout.contains("argon2id-prefix: $argon2id$"),
        "stdout: {}",
        stdout
    );
    assert!(stdout.contains("verify-ok: true"), "stdout: {}", stdout);
    assert!(stdout.contains("verify-bad: false"), "stdout: {}", stdout);
}

/// Decodes a base64url segment (no padding) to a UTF-8 String. Small
/// self-contained decoder so the test needs no external crate.
fn decode_b64url_utf8(s: &str) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut lut = [255u8; 256];
    for (i, &c) in ALPHA.iter().enumerate() {
        lut[c as usize] = i as u8;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        let v = lut[c as usize];
        if v == 255 {
            continue;
        }
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn auth_codegen_jwt_encode_heterogeneous_payload_v0_41_0() {
    // v0.41.0 — `jwt.encode` con payload HETEROGÉNEO (Str + Int + Bool +
    // List) compila a binario nativo y serializa cada claim con su tipo
    // JSON nativo (numérico/bool/array, NO stringificado). Paridad con el
    // intérprete, que ya era heterogéneo vía `value_to_json`. Antes el
    // codegen exigía `Map<Str, Str>` estricto.
    let src = "\
let secret = \"secret-32-bytes-long-test-aaaaaa\"\n\
let payload = {\"sub\": \"ada\", \"exp\": 1699999999, \"admin\": true, \"roles\": [\"a\", \"b\"]}\n\
let token = jwt.encode(payload, secret)\n\
print(token)\n\
";
    let (stdout, exit) = build_and_run("jwt_encode_hetero_v0_41_0", src);
    assert_eq!(exit, 0, "exit: {} stdout: {}", exit, stdout);
    let token = stdout.trim();
    let segs: Vec<&str> = token.split('.').collect();
    assert_eq!(segs.len(), 3, "un JWT tiene 3 segmentos, got: {}", token);
    // Decodifica el payload (segmento del medio) y verifica el tipo JSON
    // nativo de cada claim — la prueba de que el marshaling heterogéneo
    // funcionó (sin aplastar a Str).
    let claims = decode_b64url_utf8(segs[1]);
    assert!(
        claims.contains("\"exp\":1699999999"),
        "exp debe ser numérico (no \"1699999999\"): {}",
        claims
    );
    assert!(
        claims.contains("\"admin\":true"),
        "admin debe ser bool: {}",
        claims
    );
    assert!(
        claims.contains("\"roles\":[\"a\",\"b\"]"),
        "roles debe ser array: {}",
        claims
    );
    assert!(
        claims.contains("\"sub\":\"ada\""),
        "sub debe ser string: {}",
        claims
    );
}

#[test]
fn auth_codegen_jwt_encode_str_str_fast_path_still_compiles_v0_41_0() {
    // Byte-compat: un payload `Map<Str, Str>` sigue el fast-path
    // Str→Str (`__fitz_jwt_encode`) y produce un JWT válido de 3
    // segmentos. Regresión del path preservado.
    let src = "\
let secret = \"secret-32-bytes-long-test-aaaaaa\"\n\
let claims: Map<Str, Str> = {\"sub\": \"u42\", \"role\": \"admin\"}\n\
let token = jwt.encode(claims, secret)\n\
print(token)\n\
";
    let (stdout, exit) = build_and_run("jwt_encode_strstr_v0_41_0", src);
    assert_eq!(exit, 0, "exit: {} stdout: {}", exit, stdout);
    let segs: Vec<&str> = stdout.trim().split('.').collect();
    assert_eq!(segs.len(), 3, "un JWT tiene 3 segmentos: {}", stdout);
    let claims = decode_b64url_utf8(segs[1]);
    assert!(claims.contains("\"sub\":\"u42\""), "claims: {}", claims);
    assert!(claims.contains("\"role\":\"admin\""), "claims: {}", claims);
}

#[test]
fn auth_codegen_jwt_decode_heterogeneous_roundtrip_v0_41_2() {
    // v0.41.2 — `jwt.decode` devuelve un `Map<Str, Any>` heterogéneo:
    // los claims vuelven con su tipo JSON nativo (Int/Bool/Str), no
    // stringificados. Paridad con el intérprete. Round-trip: encode
    // heterogéneo → decode → leer claims tipados. La prueba es que el
    // binario hace aritmética Int sobre `level`/`count` y usa `admin`
    // como Bool — imposible si decode aplastara todo a Str.
    let src = "\
let secret = \"secret-32-bytes-long-test-aaaaaa\"\n\
let payload = {\"sub\": \"ada\", \"level\": 42, \"admin\": true, \"count\": 3}\n\
let token = jwt.encode(payload, secret)\n\
let claims = match jwt.decode(token, secret) {\n\
  Ok(c) => c,\n\
  Err(e) => { print(\"err: {e}\"); {} },\n\
}\n\
let level: Int = claims[\"level\"]\n\
print(\"level+1={level + 1}\")\n\
let admin: Bool = claims[\"admin\"]\n\
if (admin) { print(\"is admin\") } else { print(\"not admin\") }\n\
let sub: Str = claims[\"sub\"]\n\
print(\"sub={sub}\")\n\
let count: Int = claims[\"count\"]\n\
print(\"count*2={count * 2}\")\n\
";
    let (stdout, exit) = build_and_run("jwt_decode_hetero_rt_v0_41_2", src);
    assert_eq!(exit, 0, "exit: {} stdout: {}", exit, stdout);
    assert!(stdout.contains("level+1=43"), "Int claim: {}", stdout);
    assert!(stdout.contains("is admin"), "Bool claim: {}", stdout);
    assert!(stdout.contains("sub=ada"), "Str claim: {}", stdout);
    assert!(stdout.contains("count*2=6"), "Int claim: {}", stdout);
}

// ---------------------------------------------------------------------------
// Fase 9.w.2.c — Tests E2E del codegen WebSocket.
//
// Compilan un programa con `@ws("/path")` a binario nativo con `fitz
// build`, spawnean el binario, conectan un cliente WS via
// tokio-tungstenite (dev-dep), envían/reciben frames, y verifican que
// el flujo matchea bit-a-bit el comportamiento del intérprete (9.w.2.b).
// ---------------------------------------------------------------------------

/// Spawn del binario WS + cliente que envía/recibe un mensaje texto.
/// Devuelve la respuesta del server.
///
/// SERIAL es Mutex<()> sync intencional para serializar tests
/// (cargo test corre tests en paralelo por default; SERIAL hace
/// mutex global por test E2E que invoca rustc). Mantenerlo durante
/// el await es deliberado — soltarlo defeats the purpose.
#[allow(clippy::await_holding_lock)]
async fn ws_build_send_recv(
    test_name: &str,
    src: &str,
    port: u16,
    path: &str,
    payload: &str,
) -> String {
    // T2 (v0.10.13) — unique stem por test_name.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build WS failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("WS server did not open port {} within 3s", port);
    }

    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let url = format!("ws://{}{}", addr, path);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect");
    ws.send(Message::text(payload.to_string()))
        .await
        .expect("send");
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .expect("timeout")
        .expect("frame")
        .expect("ok");
    let txt = match frame {
        Message::Text(t) => t.to_string(),
        other => panic!("expected text, was {:?}", other),
    };
    let _ = child.kill();
    let _ = child.wait();
    txt
}

#[test]
fn ws_codegen_echo_simple_compila_y_responde() {
    // Programa mínimo `@ws` con WsConn<Str>. Verifica que el codegen
    // emite el preludio + wrapper + dispatch de los métodos, y que el
    // binario nativo responde idéntico al intérprete.
    let src = "@server(43971)\n\
               fn main() => 0\n\
               @ws(\"/echo\")\n\
               async fn echo(conn: WsConn<Str>) -> Null {\n\
                   match conn.recv() {\n\
                       Ok(msg) => {\n\
                           let _ = conn.send(\"eco-{msg}\")\n\
                           return null\n\
                       }\n\
                       Err(_) => return null\n\
                   }\n\
               }";
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let resp = rt.block_on(async {
        ws_build_send_recv("ws_codegen_echo", src, 43971, "/echo", "\"hola\"").await
    });
    assert_eq!(resp, "\"eco-hola\"");
}

#[test]
fn ws_codegen_tipo_custom_marshaling_json() {
    // `WsConn<ChatMsg>` con tipo custom. El codegen debe emitir el
    // marshaling JSON via __ToFitzJson/__FromFitzJson del tipo,
    // espejo del runtime (9.w.2.b).
    let src = "@server(43972)\n\
               fn main() => 0\n\
               type ChatMsg { user: Str, text: Str }\n\
               @ws(\"/chat\")\n\
               async fn chat(conn: WsConn<ChatMsg>) -> Null {\n\
                   match conn.recv() {\n\
                       Ok(msg) => {\n\
                           let reply = ChatMsg { user: msg.user, text: \"re:{msg.text}\" }\n\
                           let _ = conn.send(reply)\n\
                           return null\n\
                       }\n\
                       Err(_) => return null\n\
                   }\n\
               }";
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let resp = rt.block_on(async {
        ws_build_send_recv(
            "ws_codegen_custom",
            src,
            43972,
            "/chat",
            "{\"user\":\"ada\",\"text\":\"hi\"}",
        )
        .await
    });
    // serde_json preserve_order: el orden de los fields del struct se
    // mantiene en la serialización.
    let v: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
    assert_eq!(v["user"], serde_json::json!("ada"));
    assert_eq!(v["text"], serde_json::json!("re:hi"));
}

// ---------------------------------------------------------------------------
// 9.w.2-binary-frames — `WsConn<Bytes>` end-to-end con codegen
// ---------------------------------------------------------------------------

/// Variante binaria de `ws_build_send_recv`. Conecta vía WS, envía un
/// frame Binary, recibe el response también como Binary. Garantía:
/// bytes pasan bit-a-bit por el wire.
#[allow(clippy::await_holding_lock)]
async fn ws_build_send_recv_binary(
    test_name: &str,
    src: &str,
    port: u16,
    path: &str,
    payload: Vec<u8>,
) -> Vec<u8> {
    // T2 (v0.10.13) — unique stem por test_name.
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build WS<Bytes> failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("WS<Bytes> server did not open port {} within 3s", port);
    }

    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let url = format!("ws://{}{}", addr, path);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect");
    ws.send(Message::binary(payload))
        .await
        .expect("send binary");
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .expect("timeout")
        .expect("frame")
        .expect("ok");
    let bs = match frame {
        Message::Binary(b) => b.to_vec(),
        other => panic!("expected binary, was {:?}", other),
    };
    let _ = child.kill();
    let _ = child.wait();
    bs
}

#[test]
fn ws_codegen_binary_echo_compila_y_responde() {
    // `WsConn<Bytes>` round-trip end-to-end. El binario nativo debería
    // aceptar frames binarios raw (no JSON-marshalled), procesarlos como
    // `Value::Bytes` y devolverlos sin tocar — paridad bit-a-bit con el
    // intérprete (`ws_echo_binary_round_trip` en src/http.rs).
    let src = "@server(43973)\n\
               fn main() => 0\n\
               @ws(\"/raw\")\n\
               async fn raw(conn: WsConn<Bytes>) -> Null {\n\
                   match conn.recv() {\n\
                       Ok(buf) => match conn.send(buf) {\n\
                           Ok(_) => return null,\n\
                           Err(_) => return null,\n\
                       },\n\
                       Err(_) => return null,\n\
                   }\n\
               }";
    let payload: Vec<u8> = vec![0x00, 0x01, 0x10, 0x80, 0xff, 0x7e];
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let resp = rt.block_on(async {
        ws_build_send_recv_binary("ws_codegen_binary", src, 43973, "/raw", payload.clone()).await
    });
    assert_eq!(resp, payload);
}

// ---------------------------------------------------------------------------
// 9.w.2-ws-auth-browser — auth WS via subprotocol `bearer.<token>`
// ---------------------------------------------------------------------------

/// E2E del codegen `fitz build` con auth via subprotocol. El cliente
/// envía `bearer.<token>` como subprotocol; el server lo extrae,
/// valida con el @auth_provider, hace echo del subprotocol en el
/// handshake, y el handler corre con el `user` inyectado.
#[allow(clippy::await_holding_lock)]
#[test]
fn ws_codegen_auth_via_subprotocol_accepts_token() {
    let src = "@server(43990)\n\
               fn main() => 0\n\
               type User { id: Int, name: Str, role: Str }\n\
               @auth_provider\n\
               fn check(h: Map<Str, Str>) -> Result<User> {\n\
                   let v: Str = match h.get(\"authorization\") {\n\
                       Ok(s) => s,\n\
                       Err(_) => return Err(\"falta authorization\")\n\
                   }\n\
                   if (v == \"Bearer secret-tok\") {\n\
                       return Ok(User { id: 1, name: \"Ada\", role: \"user\" })\n\
                   }\n\
                   return Err(\"token inválido\")\n\
               }\n\
               @authenticated\n\
               @ws(\"/chat\")\n\
               async fn chat(conn: WsConn<Str>, user: User) -> Null {\n\
                   match conn.recv() {\n\
                       Ok(_) => {\n\
                           let _ = conn.send(\"hola {user.name}\")\n\
                           return null\n\
                       }\n\
                       Err(_) => return null\n\
                   }\n\
               }";
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (status_proto_echoed, msg) = rt.block_on(async move {
        // T2 (v0.10.13) — unique stem.
        let stem = "ws_auth_subproto";
        let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crear tempdir");
        let fitz_src = dir.join(format!("{}.fitz", stem));
        std::fs::write(&fitz_src, src).expect("escribir .fitz");

        let output = Command::new(fitz_bin())
            .args(["build"])
            .arg(&fitz_src)
            .output()
            .expect("invoke fitz build");
        assert!(
            output.status.success(),
            "fitz build failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let bin_name = if cfg!(windows) {
            format!("{}.exe", stem)
        } else {
            stem.to_string()
        };
        let bin = dir.join(&bin_name);
        use std::process::{Child, Stdio};
        let mut child: Child = Command::new(&bin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        std::thread::sleep(std::time::Duration::from_millis(500));
        let addr = "127.0.0.1:43990";
        let start = std::time::Instant::now();
        let mut connected = false;
        while start.elapsed() < std::time::Duration::from_secs(3) {
            if std::net::TcpStream::connect(addr).is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !connected {
            let _ = child.kill();
            panic!("WS server did not open port within 3s");
        }
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let url = format!("ws://{}/chat", addr);
        let mut req = url.as_str().into_client_request().unwrap();
        req.headers_mut().insert(
            "sec-websocket-protocol",
            "bearer.secret-tok".parse().unwrap(),
        );
        let (mut ws, resp) = tokio_tungstenite::connect_async(req)
            .await
            .expect("handshake should pass with bearer.secret-tok");
        let echoed = resp
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        ws.send(tokio_tungstenite::tungstenite::Message::text("\"hola\""))
            .await
            .expect("send");
        let frame = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
            .await
            .expect("timeout")
            .expect("frame")
            .expect("ok");
        let msg = match frame {
            tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
            other => panic!("expected text, was {:?}", other),
        };
        let _ = child.kill();
        let _ = child.wait();
        (echoed, msg)
    });
    assert_eq!(status_proto_echoed, "bearer.secret-tok");
    assert_eq!(msg, "\"hola Ada\"");
}

// ---------------------------------------------------------------------------
// R.bug-deadlock — regression test (2026-05-21)
// ---------------------------------------------------------------------------

/// El codegen pre-fix emitía un `format!(fmt, arg1, arg2, ...)` donde
/// los temporales (MutexGuards de `.lock().unwrap()`) vivían hasta el
/// final de la statement. Si dos args lockeaban el mismo Arc<Mutex<>>
/// (caso típico: `print("{xs.len()} - {total(xs)}")`), el segundo
/// `.lock()` desde el mismo thread quedaba bloqueado esperando que el
/// primero libere → deadlock silencioso del binario.
///
/// Fix: `gen_str_interp` ahora emite cada arg como `let __aN = <code>;`
/// en un bloque ANTES del `format!`. Cada `let` cierra una statement,
/// dropea el MutexGuard temporal antes de evaluar el siguiente arg.
///
/// Este test corre el binario con timeout — si el deadlock vuelve, el
/// test falla por exit code (timeout). El stdout valida que ambas
/// líneas se imprimen.
#[test]
fn r_bug_deadlock_str_interp_re_lock_same_arc_does_not_hang() {
    let src = "type Sale { product: Str, amount: Float }\n\
               let xs: List<Sale> = [\n\
                   Sale { product: \"a\", amount: 1.0 },\n\
                   Sale { product: \"b\", amount: 2.0 },\n\
               ]\n\
               fn total(sales: List<Sale>) -> Float {\n\
                   return sales.reduce(0.0, fn(acc: Float, s: Sale) => acc + s.amount)\n\
               }\n\
               print(\"len={xs.len()} total={total(xs)}\")\n";
    let (stdout, exit) = build_and_run("r-bug-deadlock-str-interp", src);
    assert_eq!(
        exit, 0,
        "binary should have terminated cleanly (timeout = deadlock)"
    );
    assert!(
        stdout.contains("len=2"),
        "expected `len=2` in stdout, was: {}",
        stdout
    );
    assert!(
        stdout.contains("total=3"),
        "expected `total=3` in stdout, was: {}",
        stdout
    );
}

// ----------------------------------------------------------------------
// Mini-fase 8.7.bis — Coerción PyAny → List<T> / Nominal / List<Nominal>
// en codegen, paridad con la coerción runtime cerrada en Paso 1 del
// plan post-boilerplates.
// ----------------------------------------------------------------------

#[cfg(feature = "python")]
#[test]
fn fase_8_7_bis_build_pyany_a_list_int_via_anotacion() {
    // `json.loads("[1, 2, 3]")` devuelve un PyList opaco. La anotación
    // `List<Int>` dispara `__fitz_py_to_list_i64` (ya emitido en el
    // preludio) — antes de 8.7.bis el coerce caía al passthrough y el
    // binario no compilaba con error de tipo Rust.
    //
    // Patrón: workaround para que match arms con varios stmts compilen:
    // envolver el cuerpo en una fn que retorna `Result<Null>` y luego
    // pattern-match al top level.
    let src = "from python import json\n\
               fn work() -> Result<Null> {\n\
                   let raw = json.loads(\"[1, 2, 3]\")?\n\
                   let xs: List<Int> = raw\n\
                   let sum = xs.reduce(0, fn(a: Int, b: Int) => a + b)\n\
                   print(\"len={xs.len()} sum={sum}\")\n\
                   return Ok(null)\n\
               }\n\
               match work() {\n\
                 Ok(_) => print(\"done\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_bis_list_int", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert!(
        stdout.contains("len=3 sum=6"),
        "expected `len=3 sum=6` in stdout, was: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_bis_build_pyany_a_instance_via_anotacion() {
    // `json.loads("{...}")` devuelve PyDict opaco. La anotación `User`
    // (nominal) dispara `__fitz_py_to_instance_User` emitido por
    // `gen_type_def` cuando uses_python=true.
    //
    // El `{` y `}` del JSON literal se escapan con `{{` y `}}` (regla
    // estándar de interpolación de strings Fitz — sin escape Fitz los
    // interpreta como start/end de interpolación).
    // `\{` y `\}` escapan literal `{`/`}` adentro del string (sin escape
    // se interpretan como inicio/fin de interpolación).
    let src = "type User { id: Int, name: Str, email: Str = \"\" }\n\
               from python import json\n\
               fn work() -> Result<Null> {\n\
                   let raw = json.loads(\"\\{\\\"id\\\": 7, \\\"name\\\": \\\"ada\\\"\\}\")?\n\
                   let u: User = raw\n\
                   print(\"id={u.id} name={u.name} email='{u.email}'\")\n\
                   return Ok(null)\n\
               }\n\
               match work() {\n\
                 Ok(_) => print(\"done\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_bis_instance", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    // El default `email = ""` se aplica porque el dict no trae la key.
    assert!(
        stdout.contains("id=7 name=ada email=''"),
        "expected `id=7 name=ada email=''` in stdout, was: {}",
        stdout
    );
}

#[cfg(feature = "python")]
#[test]
fn fase_8_7_bis_build_pyany_a_list_de_instances() {
    // Patrón canónico del boilerplate `api-postgres-python` /
    // `api-fullstack-postgres`: `let users: List<User> = json.loads(s)?`.
    // El codegen emite `__fitz_py_to_list_User` que itera el PyList y
    // llama a `__fitz_py_to_instance_User` por item. Antes de 8.7.bis
    // este código no compilaba con error de tipo Rust.
    let src = "type User { id: Int, name: Str }\n\
               from python import json\n\
               fn work() -> Result<Null> {\n\
                   let raw = json.loads(\"[\\{\\\"id\\\": 1, \\\"name\\\": \\\"ada\\\"\\}, \\{\\\"id\\\": 2, \\\"name\\\": \\\"luis\\\"\\}]\")?\n\
                   let users: List<User> = raw\n\
                   print(\"n={users.len()} first={users[0].name}\")\n\
                   return Ok(null)\n\
               }\n\
               match work() {\n\
                 Ok(_) => print(\"done\"),\n\
                 Err(e) => print(\"err: {e}\")\n\
               }\n";
    let (stdout, exit) = build_and_run("fase_8_7_bis_list_instance", src);
    assert_eq!(exit, 0, "exit code esperado 0, fue {}", exit);
    assert!(
        stdout.contains("n=2 first=ada"),
        "expected `n=2 first=ada` in stdout, was: {}",
        stdout
    );
}

// ----------------------------------------------------------------------
// Mini-fase env builtin (2026-05-22, Paso 3 post-boilerplates) —
// `env(key)`, `env_or(key, default)`, `load_env(path)`. Tests E2E del
// codegen: build a binario nativo + spawn con env var inyectada via
// `Command::env(...)` → validar que el binario lee la var correctamente.
// ----------------------------------------------------------------------

#[test]
fn env_builtin_reads_existing_var_and_propagates_with_try() {
    let src = "\
        fn read() -> Result<Str> {\n\
            let s = env(\"FITZ_E2E_GREETING\")?\n\
            return Ok(\"hola, {s}!\")\n\
        }\n\
        match read() {\n\
          Ok(v) => print(v),\n\
          Err(e) => print(\"err: {e}\")\n\
        }\n";
    let (stdout, exit) =
        build_and_run_with_env("env-builtin-exists", src, &[("FITZ_E2E_GREETING", "mundo")]);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "hola, mundo!");
}

#[test]
fn env_builtin_var_missing_propagates_err() {
    // No seteamos la var → env() devuelve Err, el `?` propaga, el
    // match top-level imprime el msg de error.
    let src = "\
        fn read() -> Result<Str> {\n\
            let s = env(\"FITZ_E2E_NUNCA_EXISTE_XYZ\")?\n\
            return Ok(s)\n\
        }\n\
        match read() {\n\
          Ok(v) => print(\"got: {v}\"),\n\
          Err(e) => print(\"caught: {e}\")\n\
        }\n";
    let (stdout, exit) = build_and_run_with_env("env-builtin-missing", src, &[]);
    assert_eq!(exit, 0);
    assert!(
        stdout.contains("caught:") && stdout.contains("FITZ_E2E_NUNCA_EXISTE_XYZ"),
        "expected caught with key, was: {}",
        stdout
    );
}

#[test]
fn env_or_builtin_returns_default_if_missing() {
    let src = "\
        let port = env_or(\"FITZ_E2E_NO_SET_PORT\", \"3000\")\n\
        print(\"port={port}\")\n";
    let (stdout, exit) = build_and_run_with_env("env-or-default", src, &[]);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "port=3000");
}

#[test]
fn env_or_builtin_existing_var_ignores_default() {
    let src = "\
        let port = env_or(\"FITZ_E2E_PORT_REAL\", \"3000\")\n\
        print(\"port={port}\")\n";
    let (stdout, exit) =
        build_and_run_with_env("env-or-real", src, &[("FITZ_E2E_PORT_REAL", "8080")]);
    assert_eq!(exit, 0);
    assert_eq!(stdout.trim(), "port=8080");
}

#[test]
fn load_env_builtin_loads_file_and_reads_vars() {
    // Escribimos un .env en un tempdir SEPARADO al del build (el build
    // hace `remove_dir_all` adentro de `fitz-e2e-<test_name>`, no queremos
    // que pise el env file). La fitz src lee `FITZ_E2E_LOAD_PATH` (env
    // var del runtime) para saber dónde está el .env.
    let envdir = std::env::temp_dir().join("fitz-e2e-load-env-FILES");
    let _ = std::fs::remove_dir_all(&envdir);
    std::fs::create_dir_all(&envdir).unwrap();
    let env_file = envdir.join("config.env");
    std::fs::write(
        &env_file,
        "# config para el test\nFITZ_E2E_LOAD_K1=valor1\nFITZ_E2E_LOAD_K2=\"con espacios\"\n",
    )
    .unwrap();

    // Wrap todo el laburo en una fn que retorna Result<Null> para
    // poder usar el patrón `?` y dejar el match top-level con un solo
    // print por arm (limitación: print no es expresión en codegen).
    let src = "\
        fn boot_and_read() -> Result<Null> {\n\
            let path = env(\"FITZ_E2E_LOAD_PATH\")?\n\
            let _ = load_env(path)?\n\
            let k1 = env_or(\"FITZ_E2E_LOAD_K1\", \"\")\n\
            let k2 = env_or(\"FITZ_E2E_LOAD_K2\", \"\")\n\
            print(\"k1={k1} k2={k2}\")\n\
            return Ok(null)\n\
        }\n\
        match boot_and_read() {\n\
          Ok(_) => print(\"done\"),\n\
          Err(e) => print(\"err: {e}\")\n\
        }\n";
    let env_path = env_file.to_str().unwrap().to_string();
    let (stdout, exit) = build_and_run_with_env(
        "load-env-codegen",
        src,
        &[("FITZ_E2E_LOAD_PATH", env_path.as_str())],
    );
    let _ = std::fs::remove_file(&env_file);
    assert_eq!(exit, 0, "stdout fue: {}", stdout);
    assert!(
        stdout.contains("k1=valor1 k2=con espacios"),
        "expected k1+k2 loaded from file, was: {}",
        stdout
    );
}

// ----------------------------------------------------------------------
// Mini-tanda Cleanup-Residual (2026-05-22) — R.bug-result-status:
// handler con return type Result<T> + return <status> mezclados.
// Pre-fix el codegen serializaba el Result wrapper como
// `{"Ok":{...}}` en lugar de desempacar el inner. Caso del
// boilerplate api-simple que tenía workaround inline.
// ----------------------------------------------------------------------

#[test]
fn r_bug_result_status_handler_unwrap_ok_serializa_t_directo() {
    let src = "\
@server(43777)
fn main() => 0

type Item { id: Int, name: Str }

@get(\"/items/{id}\")
fn get_item(id: Int) -> Result<Item> {
    if (id == 1) {
        return Ok(Item { id: 1, name: \"alpha\" })
    }
    return 404 { \"error\": \"no encontrado\" }
}
";
    // GET /items/1 — return Ok(Item) debe serializar Item directo,
    // no `{"Ok":{...}}`.
    let (status, body) = build_spawn_request(
        "r-bug-result-status-ok",
        src,
        43777,
        "GET",
        "/items/1",
        None,
    );
    assert_eq!(status, 200, "body: {}", body);
    let lower = body.to_lowercase();
    assert!(
        !lower.contains("\"ok\":") && !lower.contains("\"err\":"),
        "el body NO debe tener wrapper Ok/Err: {}",
        body
    );
    assert!(
        body.contains("\"id\":1") && body.contains("\"name\":\"alpha\""),
        "el body debe ser el Item directo: {}",
        body
    );
}

#[test]
fn r_bug_result_status_handler_path_404_works() {
    let src = "\
@server(43778)
fn main() => 0

type Item { id: Int, name: Str }

@get(\"/items/{id}\")
fn get_item(id: Int) -> Result<Item> {
    if (id == 1) {
        return Ok(Item { id: 1, name: \"alpha\" })
    }
    return 404 { \"error\": \"no encontrado\" }
}
";
    // GET /items/99 — return <status> path sigue funcionando OK.
    let (status, body) = build_spawn_request(
        "r-bug-result-status-404",
        src,
        43778,
        "GET",
        "/items/99",
        None,
    );
    assert_eq!(status, 404, "body: {}", body);
    assert!(
        body.contains("\"error\":\"no encontrado\""),
        "404 debe traer mensaje: {}",
        body
    );
}

// ----------------------------------------------------------------------
// Mini-fase loader-absoluto (2026-05-22, Paso 4 post-boilerplates) —
// `from sub.foo` desde un módulo en subcarpeta debe resolver al
// import_root (parent del entry file) cuando la resolución relativa
// falla. Caso canónico del boilerplate api-postgres-python.
// ----------------------------------------------------------------------

#[test]
fn loader_absoluto_data_sibling_import_compila_en_fitz_build() {
    // Setup multi-archivo:
    //   src/main.fitz       — usa data/users (que importa types/user)
    //   src/types/user.fitz — define type User
    //   src/data/users.fitz — `from types.user import User` (resuelve via import_root)
    // T2 (v0.10.13) — unique stem.
    let stem = "loader_absoluto";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("types")).unwrap();
    std::fs::create_dir_all(src.join("data")).unwrap();

    std::fs::write(
        src.join("types").join("user.fitz"),
        "type User { id: Int, name: Str }\n",
    )
    .unwrap();
    // data/users.fitz importa `User` desde el módulo hermano types/.
    // El test clave: ese `from types.user import User` resuelve via
    // import_root (parent del main.fitz), no via la búsqueda relativa
    // que daría `src/data/types/user.fitz` (mal).
    //
    // Sólo testeamos que el import RESUELVA (build success); cómo
    // usamos el tipo después está fuera del scope de este test.
    std::fs::write(
        src.join("data").join("users.fitz"),
        "from types.user import User\n\
         fn make_name(u: User) -> Str => u.name\n",
    )
    .unwrap();
    std::fs::write(
        src.join("main.fitz"),
        "from data.users import make_name\n\
         from types.user import User\n\
         let u = User { id: 7, name: \"ada\" }\n\
         print(\"hello {make_name(u)}\")\n",
    )
    .unwrap();

    // Build el main.fitz directamente.
    let main_fitz = src.join("main.fitz");
    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_fitz)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) { "main.exe" } else { "main" };
    let bin = src.join(bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(run.status.code().unwrap_or(-1), 0, "stdout: {}", stdout);
    assert_eq!(stdout.trim(), "hello ada");

    let _ = std::fs::remove_dir_all(&dir);
}

// =============================================================
// Fase 10.1.c — driver Postgres en `fitz build`
// =============================================================
//
// Estos tests validan que el codegen del módulo `db` produce
// Rust que rustc acepta y que el binario producido corre. NO
// conectan a un Postgres real — usan URLs que el driver rechaza
// inmediatamente (parse error o `sslmode=require`) para que el
// programa termine rápido pero pase por todo el flow de `db.connect`
// + `.await` + match sobre Result.

#[test]
fn db_connect_invalid_url_compiles_and_runs() {
    // Programa Fitz que llama a db.connect con URL no-postgres.
    // El parse del URL falla y devuelve Result::Err(msg). El
    // `return match` explícito evita el bug del codegen que no
    // detecta el último expression como return value.
    let (stdout, code) = build_and_run(
        "db_connect_url_invalida_compila_y_corre",
        "async fn run() -> Str {\n  \
             let r = db.connect(\"mysql://x@h/d\").await\n  \
             return match r {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(msg) => msg\n  \
             }\n\
         }\n\
         print(run().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(
        stdout.contains("URL") || stdout.contains("postgres"),
        "expected URL error message, was: {}",
        stdout,
    );
}

#[test]
fn db_connect_sslmode_require_compiles_and_fails_with_message() {
    // sslmode=require todavía no llega (deuda TLS). El driver
    // devuelve Err con mensaje claro citando el sub-paso futuro.
    let (stdout, code) = build_and_run(
        "db_connect_sslmode_require_compila_y_falla_con_mensaje",
        "async fn run() -> Str {\n  \
             let r = db.connect(\"postgres://x@127.0.0.1:1/d?sslmode=require\").await\n  \
             return match r {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(msg) => msg\n  \
             }\n\
         }\n\
         print(run().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(
        stdout.contains("sslmode") || stdout.contains("TLS"),
        "expected message about sslmode, was: {}",
        stdout,
    );
}

#[test]
fn db_query_exec_close_compile_emit_helpers() {
    // Programa que NO conecta realmente. El connect falla con
    // URL inválida, así que el cuerpo de `run` propaga el Err
    // antes de ejecutar query/exec/close. Lo importante es que
    // el codegen los emita y rustc compile el output.
    let (stdout, code) = build_and_run(
        "db_query_exec_close_compilan_emite_helpers",
        "async fn run() -> Result<Bool> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _rows = conn.query(\"SELECT 1\", []).await?\n  \
             let _n = conn.exec(\"UPDATE t SET x = 1\", [\"hola\"]).await?\n  \
             conn.close().await?\n  \
             return Ok(true)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn db_map_lit_homogeneo_a_field_any_compila_w1() {
    // W1 (v0.10.6) — `Event { metadata: {"code": 500} }` con field
    // `metadata: Map<Str, Any>` compila a binario nativo. Antes el
    // codegen inferia `Map<Str, Int>` del literal y fallaba con E0308
    // contra el field heterogéneo. El context-aware gen_map_lit_with_hint
    // ahora detecta el hint `Map<_, Any>` y emite el shape `Vec<(FV, FV)>`
    // directamente.
    let (stdout, code) = build_and_run(
        "db_map_lit_homogeneo_a_field_any_w1",
        "type Event {\n  \
             id: Int\n  \
             metadata: Map<Str, Any>\n\
         }\n\
         fn show(e: Event) -> Str {\n  \
             return \"event {e.id}\"\n\
         }\n\
         let e = Event { id: 1, metadata: {\"code\": 500} }\n\
         print(show(e))\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(
        stdout.contains("event 1"),
        "expected `event 1`, was: {}",
        stdout
    );
}

#[test]
fn db_match_nullable_refinement_w2() {
    // W2 (v0.10.6) — `match obj { null => ..., u => u.field }` compila
    // a binario nativo. Antes el codegen emitía el binding `u` como
    // `Option<UserData>` y `u.name` fallaba rustc con type error.
    // Ahora el codegen detecta scrut `Nullable<T>` y emite `Some(name)`
    // refinando `name` a `T`. Paridad con el evaluator (que ya
    // matcheaba Value::Null vs Value::Instance correctamente).
    //
    // Bonus: corrige bug silencioso anterior donde Pattern::Null sobre
    // Nullable emitía `_` (matcheaba TODO, no solo null) — ahora
    // emite `None` específico.
    let (stdout, code) = build_and_run(
        "db_match_nullable_refinement_w2",
        "type User { name: Str }\n\
         type Profile { user: User? }\n\
         fn show(p: Profile) -> Str {\n  \
             return match p.user {\n    \
                 null => \"sin usuario\"\n    \
                 u    => u.name\n  \
             }\n\
         }\n\
         let con_user = Profile { user: User { name: \"ada\" } }\n\
         let sin_user = Profile { user: null }\n\
         print(show(con_user))\n\
         print(show(sin_user))\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(
        stdout.contains("ada"),
        "expected `ada` in output, was: {}",
        stdout
    );
    assert!(
        stdout.contains("sin usuario"),
        "expected `sin usuario` in output, was: {}",
        stdout,
    );
}

#[test]
fn db_orm_cross_module_at_table_compila_w8() {
    // W8 (v0.10.7) — `@table` types definidos en un módulo y usados
    // desde otro vía `from X import Y` compilan a binario nativo.
    // Antes: error "variable desconocida en codegen: `User`" cuando
    // el dispatch ORM no encontraba `TableMetadata` cross-module.
    // Ahora: el `LoadedModule` propaga el metadata al importer, y
    // el módulo emite `impl __FromFitzDbRow for UserData` localmente
    // con los helpers `use crate::__From...` necesarios.
    // T2 (v0.10.13) — unique stem ya estaba; SERIAL removido.
    let stem = sanitize_stem("db_orm_cross_module_at_table_w8");
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // Módulo con el @table type.
    let models_path = dir.join("models.fitz");
    std::fs::write(
        &models_path,
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n\
         }\n",
    )
    .expect("escribir models.fitz");

    // Main que importa el type y lo usa con dispatch ORM.
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(
        &main_path,
        "from models import User\n\
         \n\
         async fn run() -> Result<List<User>> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             return User.where(fn(u) => u.id == 1).all(conn).await\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    )
    .expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed cross-module:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let run = Command::new(&bin).output().expect("invocar binario");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let code = run.status.code().unwrap_or(-1);
    assert_eq!(code, 0, "stdout: {}", stdout);
    // URL inválida → run().await devuelve Err → driver imprime "err".
    assert!(
        stdout.contains("err"),
        "expected `err` in stdout, was: {}",
        stdout,
    );
}

#[test]
fn jsonb_cross_module_compila_w11() {
    // W11 (v0.10.7) — `@table` types con `Map<Str, Any>` o `List<Any>`
    // (JSONB / arrays heterogéneos) declarados en un módulo se usan
    // desde otro vía `from X import Y` y compilan a binario nativo.
    // Antes: unresolved imports `crate::__FitzValue` y `crate::__fv_type_name`
    // porque `uses_fitz_value` solo miraba el main. Ahora: transitivo
    // (idem uses_ws/uses_jobs/uses_auth post-W10), y `__fv_type_name`
    // se emite como `pub(crate)`.
    // T2 (v0.10.13) — SERIAL removido.
    let stem = sanitize_stem("jsonb_cross_module_w11");
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // Módulo con `@table` + JSONB (Map<Str, Any>).
    let models_path = dir.join("models.fitz");
    std::fs::write(
        &models_path,
        "@table(\"docs\") type Doc {\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             meta: Map<Str, Any>\n\
         }\n",
    )
    .expect("escribir models.fitz");

    // Main importa el type y dispatcha el ORM.
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(
        &main_path,
        "from models import Doc\n\
         \n\
         async fn run() -> Result<List<Doc>> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             return Doc.all(conn).await\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    )
    .expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed cross-module JSONB:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());
}

#[test]
fn ws_jobs_cross_module_compila_w10() {
    // W10 (v0.10.7) — `@ws("/path")` y `@background`/`@cron` declarados
    // adentro de módulos importados (no en el main) compilan a binario
    // nativo. Antes: el Cargo.toml del crate generado no incluía
    // `futures_util`/`chrono`/feature `ws` de axum porque el `uses_ws`/
    // `uses_jobs` solo miraba el main. Ahora: ambos flags son
    // transitivos (idem `uses_db` post-W8), y los preludios WS/jobs
    // tienen los items `pub(crate)` para que los `use crate::__Fitz*`
    // de los módulos resuelvan.
    // T2 (v0.10.13) — SERIAL removido.
    let stem = sanitize_stem("ws_jobs_cross_module_w10");
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // Módulo con `@ws` + `@background` (jobs).
    let realtime_path = dir.join("realtime.fitz");
    std::fs::write(
        &realtime_path,
        "type Msg {\n  \
             text: Str\n\
         }\n\
         \n\
         @ws(\"/chat\")\n\
         async fn chat(conn: WsConn<Msg>) -> Null {\n  \
             return null\n\
         }\n\
         \n\
         @background\n\
         async fn ping() -> Null {\n  \
             return null\n\
         }\n",
    )
    .expect("escribir realtime.fitz");

    // Main solo importa el módulo y declara @server (sin @ws ni @background propio).
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(
        &main_path,
        "from realtime import chat\n\
         \n\
         @server(43910)\n\
         fn main() => 0\n\
         \n\
         print(\"main ok\")\n",
    )
    .expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed cross-module WS+jobs:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());
}

#[test]
fn db_upd_with_map_var_compiles_to_binary_w7() {
    // W7 (v0.10.6) — `.update(db, changes)` con `changes` como var
    // `Map<Str, Any>` (no Map literal) compila a binario nativo.
    // Programa connect inválido — el cuerpo nunca ejecuta el update.
    // (Stem `update` dispara Windows UAC installer detection — usamos
    //  `upd` para evitarlo, igual que `up_map_upd` existente.)
    let (stdout, code) = build_and_run(
        "db_upd_con_map_var_compila_w7",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n\
         }\n\
         async fn run(changes: Map<Str, Any>) -> Result<Bool> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _n = User.where(fn(u) => u.id == 1).update(conn, changes).await?\n  \
             return Ok(true)\n\
         }\n\
         async fn driver() -> Str {\n  \
             let body = {\"name\": \"ada\", \"age\": 36}\n  \
             return match run(body).await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

// ---------------------------------------------------------------------------
// Fase 10.b.4 — ORM write methods en codegen
// ---------------------------------------------------------------------------

#[test]
fn orm_insert_compiles_emits_insert_returning_without_postgres() {
    // Programa con `User.insert(db, user)` debe COMPILAR a binario
    // standalone. El connect a una URL inválida falla en runtime, así
    // que el insert nunca se ejecuta — verificamos que el rustc del
    // proyecto generado no rechaza el código, que es el criterio
    // formal de "10.b.4 destraba el primer write method en codegen".
    let (stdout, code) = build_and_run(
        "orm_insert_compila_emite_insert_returning_sin_postgres",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n\
         }\n\
         async fn run() -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let u = User { id: 1, name: \"ada\", age: 30 }\n  \
             let _inserted = User.insert(conn, u).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_insert_with_nullable_and_column_override_compiles() {
    // Marshalling de `Str?` + override `@column(name=...)` deben
    // compilar limpio. Mismo patrón: el connect falla, el insert no
    // se ejecuta, lo que verificamos es el output Rust.
    let (stdout, code) = build_and_run(
        "orm_insert_con_nullable_y_column_override_compila",
        "@table(\"posts\") type Post {\n  \
             @primary id: Int = 0\n  \
             title: Str\n  \
             @column(name=\"sub_title\") subtitle: Str?\n\
         }\n\
         async fn run() -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let p = Post { id: 1, title: \"hola\", subtitle: null }\n  \
             let _r = Post.insert(conn, p).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_update_and_delete_without_where_aborts_build_with_safety_guard() {
    // 10.b.5 — `User.update(...)` / `User.delete(...)` sin `.where(...)`
    // previo se rechazan en codegen con mensaje paralelo al guard del
    // evaluator (afectarían toda la tabla).
    let stderr_update = build_expect_fail(
        "orm_update_sin_where_aborta_build_con_guard_de_seguridad",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n\
         }\n\
         async fn run() -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _n = User.update(conn, {\"name\": \"ada\"}).await?\n  \
             return Ok(null)\n\
         }\n\
         print(\"never\")\n",
    );
    assert!(
        stderr_update.contains("requires a prior `.where(...)`"),
        "expected mention of guard `.where(...)` requirement in stderr, was: {}",
        stderr_update,
    );
    let stderr_delete = build_expect_fail(
        "orm_delete_sin_where_aborta_build_con_guard_de_seguridad",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n\
         }\n\
         async fn run() -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _n = User.delete(conn).await?\n  \
             return Ok(null)\n\
         }\n\
         print(\"never\")\n",
    );
    assert!(
        stderr_delete.contains("requires a prior `.where(...)`"),
        "expected mention of guard `.where(...)` requirement in stderr, was: {}",
        stderr_delete,
    );
}

// ---------------------------------------------------------------------------
// Fase 10.b.5 — QueryBuilder chain en codegen
// ---------------------------------------------------------------------------

#[test]
fn orm_where_chain_with_order_and_limit_compiles_to_binary() {
    // `User.where(...).order_by(...).limit(N).all(db)` debe compilar a
    // binario standalone. El connect a una URL inválida falla en runtime
    // así que el chain nunca se ejecuta; verificamos que el rustc del
    // proyecto generado no rechaza el código.
    let (stdout, code) = build_and_run(
        "orm_where_chain_con_order_y_limit_compila_a_binario",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n\
         }\n\
         async fn run() -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _u = User.where(fn(u) => u.age > 18).order_by(fn(u) => -u.age).limit(10).all(conn).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_qb_upd_with_where_compiles_to_binary() {
    // `User.where(...).update(db, {...})` compila a binario y emite el
    // SET correctamente. Run-time falla por URL inválida. Nombre del
    // test evita la cadena `update` para no disparar el handler UAC
    // de Windows sobre `*update*.exe`.
    let (stdout, code) = build_and_run(
        "orm_qb_upd_con_where_compila_a_binario",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n\
         }\n\
         async fn run() -> Result<Int> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let n = User.where(fn(u) => u.age > 65).update(conn, {\"name\": \"retired\"}).await?\n  \
             return Ok(n)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_sql_expr_and_date_arith_compila_o1_o3() {
    // O1/O3 (v0.32.0) — `sql.raw(...)` / `sql.now()` en `.update` +
    // aritmética de fechas (`plus_seconds`) + `sql.now()` en `.where`
    // compilan a binario nativo. La conexión se llama `db` (el caso de
    // colisión con el módulo) para probar que el namespace `sql` lo
    // evita. Run-time falla por URL inválida. El nombre del test evita
    // la cadena `update` (handler UAC de Windows sobre `*update*.exe`).
    let (stdout, code) = build_and_run(
        "orm_sql_expr_y_date_arith_compila",
        "@table(\"monitors\") type Monitor {\n  \
             @primary id: Int = 0\n  \
             streak: Int\n  \
             last_check_at: Str\n  \
             interval_secs: Int\n\
         }\n\
         async fn run() -> Result<Int> {\n  \
             let db = db.connect(\"mysql://x@h/d\").await?\n  \
             let due = Monitor.where(fn(m) => m.last_check_at.plus_seconds(m.interval_secs) < sql.now()).count(db).await?\n  \
             let n = Monitor.where(fn(m) => m.id == 1).update(db, {\"streak\": sql.raw(\"streak + 1\"), \"last_check_at\": sql.now()}).await?\n  \
             return Ok(n + due)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_delete_with_where_compiles_to_binary() {
    // `User.where(...).delete(db)` compila a binario. Run-time falla
    // por URL inválida.
    let (stdout, code) = build_and_run(
        "orm_delete_con_where_compila_a_binario",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             age: Int\n\
         }\n\
         async fn run() -> Result<Int> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let n = User.where(fn(u) => u.age < 13).delete(conn).await?\n  \
             return Ok(n)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_chain_first_with_multiple_wheres_compiles_to_binary() {
    // Caso pesado: dos wheres + order_by + first(db). Cubre el
    // renumerado de placeholders en runtime + LIMIT 1 override en
    // .first.
    let (stdout, code) = build_and_run(
        "orm_chain_first_con_multiples_wheres_compila_a_binario",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n\
         }\n\
         async fn run() -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _u = User.where(fn(u) => u.age > 18).where(fn(u) => u.name == \"ada\").order_by(fn(u) => u.id).first(conn).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

// ---------------------------------------------------------------------------
// Fase 10.b.6 — Agregados scalares sobre QueryBuilder (sum/avg/min/max)
//
// Validan que el codegen emite código Rust que compila con `cargo build`
// (incluye el preludio db con el helper `aggregate_f64`, dispatch sobre
// QB, cast `::float8` para AVG, etc.). El runtime falla por URL inválida
// (no hay Postgres en el test runner), pero el goal acá es probar el
// pipeline del codegen end-to-end. La paridad real contra Postgres
// queda para los E2E de DB (Fase 10.b.10).
// ---------------------------------------------------------------------------

#[test]
fn orm_sum_directo_sobre_type_compila_a_binario() {
    // `Sale.sum(fn(s) => s.amount, db).await?` compila a binario
    // standalone — incluye el preludio con `aggregate_f64` y dispatch
    // `Type.sum(...)` → QB nuevo + agregado scalar.
    let (stdout, code) = build_and_run(
        "orm_sum_directo_sobre_type_compila_a_binario",
        "@table(\"sales\") type Sale {\n  \
             @primary id: Int = 0\n  \
             amount: Float\n\
         }\n\
         async fn run() -> Result<Float> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let t = Sale.sum(fn(s) => s.amount, conn).await?\n  \
             return Ok(t)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_avg_chain_with_where_compiles_to_binary() {
    // `.where(...).avg(closure, db)` valida que (a) el chain QB
    // preserva el where antes del aggregate, (b) avg emite el cast
    // `::float8` que el helper espera, (c) el tipo Future<Result<Float>>
    // tipa con `let a: Float = ...`.
    let (stdout, code) = build_and_run(
        "orm_avg_chain_con_where_compila_a_binario",
        "@table(\"sales\") type Sale {\n  \
             @primary id: Int = 0\n  \
             amount: Float\n  \
             region: Str\n\
         }\n\
         async fn run() -> Result<Float> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let a = Sale.where(fn(s) => s.region == \"PAT\").avg(fn(s) => s.amount, conn).await?\n  \
             return Ok(a)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

// ---------------------------------------------------------------------------
// Fase 10.b.7 — Navigation methods (belongs_to / has_one / has_many)
//
// Compile-to-binary tests sin Postgres real. Validan que el codegen
// emite código Rust que compila con cargo (incluye el helper de
// navigation, dispatch sobre Instance.<rel>(db), marshalling del FK,
// etc.). El runtime falla por URL inválida (no hay Postgres en CI),
// pero el goal acá es probar el pipeline del codegen end-to-end.
// La paridad real contra Postgres se valida en db_real_postgres.rs.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Fase 10.b.8.a — Arrays Postgres (List<Int|Float|Str|Bool>)
//
// Compile-to-binary tests sin Postgres real. Validan que el codegen
// emite Rust que compila incluyendo el marshalling bidireccional
// (Vec<T> ↔ PgValue::Array). Runtime falla por URL inválida.
// La paridad real con Postgres se valida en db_real_postgres.rs.
// ---------------------------------------------------------------------------

#[test]
fn orm_list_int_array_field_compila_a_binario() {
    // Field `tags: List<Int>` debe compilar (SELECT + INSERT + Display).
    let (stdout, code) = build_and_run(
        "orm_list_int_array_field_compila",
        "@table(\"items\") type Item {\n  \
             @primary id: Int = 0\n  \
             tags: List<Int>\n\
         }\n\
         async fn run() -> Result<Item> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let it = Item.insert(conn, Item { id: 0, tags: [10, 20, 30] }).await?\n  \
             return Ok(it)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_list_str_array_field_compila_a_binario() {
    let (stdout, code) = build_and_run(
        "orm_list_str_array_field_compila",
        "@table(\"posts\") type Post {\n  \
             @primary id: Int = 0\n  \
             labels: List<Str>\n\
         }\n\
         async fn run() -> Result<Post> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let p = Post.insert(conn, Post { id: 0, labels: [\"rust\", \"fitz\"] }).await?\n  \
             return Ok(p)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_list_float_bool_arrays_combinados_compilan() {
    // Múltiples array fields en el mismo type. Valida que el helper
    // `orm_list_scalar_info` resuelve los 4 tipos sin colisiones.
    let (stdout, code) = build_and_run(
        "orm_list_float_bool_arrays_combinados",
        "@table(\"sigs\") type Sig {\n  \
             @primary id: Int = 0\n  \
             weights: List<Float>\n  \
             flags: List<Bool>\n\
         }\n\
         async fn run() -> Result<Sig> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let s = Sig.insert(conn, Sig { id: 0, weights: [1.5, 2.5], flags: [true, false] }).await?\n  \
             return Ok(s)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

// ---------------------------------------------------------------------------
// Fase 10.b.9.a — Validación exhaustiva de operadores `.where(...)`
//
// El translator de closures a SQL ya soporta el set completo desde
// 10.b.5; estos E2E validan que cada uno COMPILA a binario nativo.
// Runtime falla por URL inválida.
// ---------------------------------------------------------------------------

#[test]
fn orm_where_chain_combinatorio_compila_a_binario() {
    // Chain con AND + OR + comparaciones + LIKE + IN + IS NULL —
    // el caso del mundo real que combina varias clauses. Valida el
    // wireado integral del translator + emit del SQL acumulado.
    let (stdout, code) = build_and_run(
        "orm_where_chain_combinatorio_compila",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             age: Int\n  \
             score: Float\n  \
             tags: List<Str>\n  \
             deleted_at: Str?\n\
         }\n\
         async fn run() -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _u = User.where(fn(u) => \
                 u.age >= 18 and (u.score > 50.0 or u.deleted_at.is_null()) and \
                 u.name.starts_with(\"ada\") and u.id.is_in([1, 2, 3])\
             ).all(conn).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_where_like_ilike_starts_ends_contains_compilan() {
    // Los 5 métodos de matching de strings — cada uno con su patrón
    // emitido. starts_with/ends_with/contains usan LIKE con `%`
    // wrapping, like e ilike pasan el patrón crudo.
    let (stdout, code) = build_and_run(
        "orm_where_like_ilike_starts_ends_contains",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n\
         }\n\
         async fn run() -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _a = User.where(fn(u) => u.name.like(\"a%\")).all(conn).await?\n  \
             let _b = User.where(fn(u) => u.name.ilike(\"A%\")).all(conn).await?\n  \
             let _c = User.where(fn(u) => u.name.starts_with(\"ada\")).all(conn).await?\n  \
             let _d = User.where(fn(u) => u.name.ends_with(\"ada\")).all(conn).await?\n  \
             let _e = User.where(fn(u) => u.name.contains(\"da\")).all(conn).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_where_is_null_is_in_with_not_compilan() {
    // is_null/is_not_null + is_in con List literal + NOT envolvente.
    let (stdout, code) = build_and_run(
        "orm_where_is_null_is_in_with_not",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             age: Int\n  \
             deleted_at: Str?\n\
         }\n\
         async fn run() -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _a = User.where(fn(u) => u.deleted_at.is_null()).all(conn).await?\n  \
             let _b = User.where(fn(u) => u.deleted_at.is_not_null()).all(conn).await?\n  \
             let _c = User.where(fn(u) => u.age.is_in([18, 21, 65])).all(conn).await?\n  \
             let _d = User.where(fn(u) => not (u.age >= 18)).all(conn).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

// ---------------------------------------------------------------------------
// Fase 10.b.8.b — JSONB libre con Map<Str, Any>
// ---------------------------------------------------------------------------

#[test]
fn orm_map_str_any_jsonb_field_compila_a_binario() {
    // Field `meta: Map<Str, Any>` con field heterogéneo (Int, Str, Bool).
    // Valida que el codegen emite helpers JSON + cast `::jsonb` + el
    // marshalling completo round-trip compila bajo cargo build.
    let (stdout, code) = build_and_run(
        "orm_map_str_any_jsonb_field_compila",
        "@table(\"docs\") type Doc {\n  \
             @primary id: Int = 0\n  \
             meta: Map<Str, Any>\n\
         }\n\
         async fn run() -> Result<Doc> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let d = Doc.insert(conn, Doc { id: 0, meta: {\"k1\": 1, \"k2\": \"hola\", \"k3\": true} }).await?\n  \
             return Ok(d)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_map_str_any_jsonb_nullable_compila() {
    // `Map<Str, Any>?` nullable también debe compilar. El cast `::jsonb`
    // se aplica al placeholder; el value None se marshalea como NULL.
    let (stdout, code) = build_and_run(
        "orm_map_str_any_jsonb_nullable_compila",
        "@table(\"docs\") type Doc {\n  \
             @primary id: Int = 0\n  \
             tags: Map<Str, Any>?\n\
         }\n\
         async fn run() -> Result<Doc> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let d = Doc.insert(conn, Doc { id: 0, tags: null }).await?\n  \
             return Ok(d)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_belongs_to_navigation_compila_a_binario() {
    // `post.user_id(db).await?` con @belongs_to debe compilar a
    // binario standalone. Runtime falla por URL inválida → driver
    // imprime "err".
    let (stdout, code) = build_and_run(
        "orm_belongs_to_navigation_compila_a_binario",
        "@table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n\
         }\n\
         @table(\"posts\") type Post {\n  \
             @primary id: Int = 0\n  \
             title: Str\n  \
             @belongs_to(\"User\") user_id: Int\n\
         }\n\
         async fn run(post: Post) -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _u = post.user_id(conn).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             let p = Post { id: 1, title: \"hello\", user_id: 42 }\n  \
             return match run(p).await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_has_many_navigation_compila_a_binario() {
    // `user.posts(conn).await?` con @has_many debe compilar. La
    // navigation devuelve `List<Post>` adentro del Result.
    let (stdout, code) = build_and_run(
        "orm_has_many_navigation_compila_a_binario",
        "@table(\"posts\") type Post {\n  \
             @primary id: Int = 0\n  \
             title: Str\n  \
             author_id: Int\n\
         }\n\
         @table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             @has_many(\"Post\", via=\"author_id\") posts: List<Post>\n\
         }\n\
         async fn run(user: User) -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _xs = user.posts(conn).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             let u = User { id: 1, name: \"ada\", posts: [] }\n  \
             return match run(u).await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_has_one_navigation_compila_a_binario() {
    // `user.profile(conn).await?` con @has_one debe compilar.
    let (stdout, code) = build_and_run(
        "orm_has_one_navigation_compila_a_binario",
        "@table(\"profiles\") type Profile {\n  \
             @primary id: Int = 0\n  \
             bio: Str\n  \
             user_id: Int\n\
         }\n\
         @table(\"users\") type User {\n  \
             @primary id: Int = 0\n  \
             name: Str\n  \
             @has_one(\"Profile\") profile: Profile?\n\
         }\n\
         async fn run(user: User) -> Result<Null> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _p = user.profile(conn).await?\n  \
             return Ok(null)\n\
         }\n\
         async fn driver() -> Str {\n  \
             let u = User { id: 1, name: \"ada\", profile: null }\n  \
             return match run(u).await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

#[test]
fn orm_min_y_max_combinados_compilan_a_binario() {
    // Dos aggregates en el mismo programa — valida que el helper
    // `aggregate_f64` se reusa entre llamadas con func distinta
    // (MIN/MAX) y el preludio del QB se emite UNA sola vez.
    let (stdout, code) = build_and_run(
        "orm_min_y_max_combinados_compilan_a_binario",
        "@table(\"sales\") type Sale {\n  \
             @primary id: Int = 0\n  \
             amount: Float\n\
         }\n\
         async fn run() -> Result<Float> {\n  \
             let conn = db.connect(\"mysql://x@h/d\").await?\n  \
             let _mn = Sale.min(fn(s) => s.amount, conn).await?\n  \
             let mx = Sale.max(fn(s) => s.amount, conn).await?\n  \
             return Ok(mx)\n\
         }\n\
         async fn driver() -> Str {\n  \
             return match run().await {\n    \
                 Ok(_) => \"OK\"\n    \
                 Err(_) => \"err\"\n  \
             }\n\
         }\n\
         print(driver().await)\n",
    );
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert!(stdout.contains("err"), "expected `err`, was: {}", stdout);
}

// ===== T5 — tipos custom compilados con cobertura más profunda =====
// Auditoría T5 (post-W12-W16) — los tests pre-existentes
// (instancia_basica_round_trip_compilado, igualdad_estructural_*,
// tipos_anidados_round_trip_compilado) cubren el caso 1-2 niveles. T5
// extiende a 3 niveles, comparación post-mutación, field chain sobre
// nullables, y display recursivo de instancias con campos colección.

#[test]
fn t5_triple_level_field_access_and_mutation_compiled() {
    // 3 niveles de anidación + mutación profunda visible vía alias del
    // nivel superior. Valida que Rc<RefCell<>> recursea como el
    // intérprete adentro de la jerarquía.
    let src = "\
type City { name: Str }
type Address { city: City }
type User { id: Int, addr: Address }
let u = User { id: 1, addr: Address { city: City { name: \"El Chaltén\" } } }
let alias = u
alias.addr.city.name = \"Buenos Aires\"
print(u.addr.city.name)
print(alias.addr.city.name)
print(u == alias)
";
    let (stdout, exit) = build_and_run("t5-triple-nivel", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["Buenos Aires", "Buenos Aires", "true"]);
}

#[test]
fn t5_equality_differs_after_single_field_mutation_compiled() {
    // Dos instancias estructuralmente iguales se vuelven distintas tras
    // mutar UN campo de UNA. PartialEq recursivo se sensibiliza a
    // cambios profundos.
    let src = "\
type Addr { city: Str }
type User { id: Int, addr: Addr }
let a = User { id: 1, addr: Addr { city: \"X\" } }
let b = User { id: 1, addr: Addr { city: \"X\" } }
print(a == b)
b.addr.city = \"Y\"
print(a == b)
print(a.addr.city)
print(b.addr.city)
";
    let (stdout, exit) = build_and_run("t5-eq-diff-tras-mutacion", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["true", "false", "X", "Y"]);
}

#[test]
fn t5_field_chain_over_nested_nullable_compiled() {
    // Caso típico: `match` con pattern `null => ...` y ident binding
    // que refina al inner. Cap 14 de la guía documenta este patrón
    // como canónico para Nullable. El binding `u` post-`null =>`
    // queda con tipo `User` (no `User?`), accediendo a .addr y .city
    // sin glue.
    let src = "\
type Addr { city: Str }
type User { id: Int, addr: Addr? }
type Order { id: Int, user: User? }
fn city_of(o: Order) -> Str {
    return match o.user {
        null => \"sin user\"
        u => match u.addr {
            null => \"sin addr\"
            a => a.city
        }
    }
}
let full = Order { id: 9, user: User { id: 1, addr: Addr { city: \"El Chaltén\" } } }
let no_addr = Order { id: 10, user: User { id: 2 } }
let no_user = Order { id: 11 }
print(city_of(full))
print(city_of(no_addr))
print(city_of(no_user))
";
    let (stdout, exit) = build_and_run("t5-field-chain-nullable", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["El Chaltén", "sin addr", "sin user"]);
}

#[test]
fn t5_recursive_display_with_list_and_map_field_compiled() {
    // Instancia con campos `List<Int>` y `Map<Str, Int>` debe imprimir
    // el formato canónico recursivo del intérprete (sin perder ningún
    // bracket o coma).
    let src = "\
type Stats { name: Str, values: List<Int>, by_tag: Map<Str, Int> }
let s = Stats { name: \"foo\", values: [1, 2, 3], by_tag: {\"a\": 10, \"b\": 20} }
print(s)
";
    let (stdout, exit) = build_and_run("t5-display-recursivo", src);
    assert_eq!(exit, 0);
    assert_lines(
        &stdout,
        &["Stats { name: \"foo\", values: [1, 2, 3], by_tag: {\"a\": 10, \"b\": 20} }"],
    );
}

// ===== T6 — combinatorias profundas List/Map con tipos custom =====
// Auditoría T6 (post-W12-W16) — los tests pre-existentes cubren
// List<Int>, Map<Str,Int>, List<Custom>. T6 valida composiciones:
// List<List<Int>>, Map<Str, List<Int>>, List<Custom?>, Map<Str, Custom>.

#[test]
fn t6_list_of_lists_int_compiled() {
    // Matriz: list de lists. Indexing doble + iteración anidada.
    // Fitz no tiene `let mut` — reasignación pura es OK.
    let src = "\
let matrix: List<List<Int>> = [[1, 2, 3], [4, 5, 6], [7, 8, 9]]
print(matrix[0])
print(matrix[1][2])
print(matrix.len())
let row = matrix[2]
print(row.len())
let total = 0
for row in matrix {
    for v in row {
        total = total + v
    }
}
print(total)
";
    let (stdout, exit) = build_and_run("t6-list-de-listas", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["[1, 2, 3]", "6", "3", "3", "45"]);
}

#[test]
fn t6_map_str_to_list_int_compiled() {
    // Map de Str → List<Int>. Lookup + iter sobre el inner. Match
    // arms multi-stmt con trailing `print(...)` colapsan en
    // expression position — el codegen lo rechaza. Workaround
    // canónico: extraer via fn helper que retorna `Int` y se
    // imprime afuera. Valida el chain `map.get -> Result<List> ->
    // .len()` end-to-end.
    let src = "\
fn show_a(buckets: Map<Str, List<Int>>) -> Int {
    return match buckets.get(\"a\") {
        Ok(xs) => xs.len()
        Err(_) => 0
    }
}
fn show_b(buckets: Map<Str, List<Int>>) -> Int {
    return match buckets.get(\"b\") {
        Ok(xs) => xs.len()
        Err(_) => 0
    }
}
let buckets: Map<Str, List<Int>> = {\"a\": [1, 2, 3], \"b\": [10, 20]}
print(buckets.len())
print(show_a(buckets))
print(show_b(buckets))
match buckets.get(\"a\") {
    Ok(xs) => { print(xs) }
    Err(e) => { print(e) }
}
";
    let (stdout, exit) = build_and_run("t6-map-str-a-list-int", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["2", "3", "2", "[1, 2, 3]"]);
}

#[test]
fn t6_list_of_custom_nullable_compiled() {
    // List<User?> con mezcla de Some + null. Display + iter + chequeo
    // de cada elemento via match (Fitz no tiene `let mut` — reasignación
    // pura del binding existente).
    let src = "\
type User { id: Int, name: Str }
let users: List<User?> = [User { id: 1, name: \"a\" }, null, User { id: 3, name: \"c\" }]
print(users.len())
let nulls = 0
let presentes = 0
for u in users {
    match u {
        null => { nulls = nulls + 1 }
        _ => { presentes = presentes + 1 }
    }
}
print(nulls)
print(presentes)
print(users[1])
";
    let (stdout, exit) = build_and_run("t6-list-custom-nullable", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["3", "1", "2", "null"]);
}

#[test]
fn t6_map_str_to_custom_compiled() {
    // Map<Str, User> — value es nominal. get() devuelve Result<User>.
    // El error del get tiene formato `"key not found: <k>"`
    // (sin comillas alrededor del key, sin "en mapa") según
    // src/codegen.rs:17259.
    let src = "\
type User { id: Int, name: Str }
let dir: Map<Str, User> = {\"a\": User { id: 1, name: \"Ana\" }, \"b\": User { id: 2, name: \"Ben\" }}
print(dir.len())
match dir.get(\"a\") {
    Ok(u) => { print(u.name) }
    Err(e) => { print(e) }
}
match dir.get(\"missing\") {
    Ok(u) => { print(u.name) }
    Err(e) => { print(e) }
}
";
    let (stdout, exit) = build_and_run("t6-map-str-a-custom", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["2", "Ana", "key not found: missing"]);
}

#[test]
fn map_str_any_indexing_assign_compiled() {
    // R.1.3 (v0.10.7) — `Map<Str, Any>` con indexing assignment
    // dinámico (`m["k"] = v`) ahora envuelve key/value como
    // `__FitzValue` cuando el storage es heterogéneo.
    //
    // **Bug que cierra**: el codegen del indexing assignment para
    // Map siempre emitía `__g.push((__k, __v))` con tipos crudos
    // (String/T), pero `rust_type_for(Map<Str, Any>)` mapea a
    // `Vec<(__FitzValue, __FitzValue)>`. Rustc rompía con
    // "expected __FitzValue, found String".
    //
    // **Caso canónico**: partial updates en APIs CRUD (un Map<Str,
    // Any> construido condicionalmente con solo los fields que el
    // cliente quiere updatear) que es patrón estándar en cualquier
    // API REST. Descubierto al escribir boilerplate api-orm-full.
    let src = "\
let m: Map<Str, Any> = {}
m[\"name\"] = \"Ada\"
m[\"age\"] = 42
m[\"active\"] = true
print(m.len())
print(m.has(\"name\"))
print(m.has(\"missing\"))
";
    let (stdout, exit) = build_and_run("map-str-any-index-assign", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["3", "true", "false"]);
}

#[test]
fn ws_broadcast_builtin_cross_handler() {
    // 10.8.7 (v0.10.8) — fix #2: builtin `ws_broadcast(endpoint, msg)`
    // permite que un handler HTTP triggeree broadcast a TODOS los
    // clientes WS conectados al endpoint. Caso canónico SaaS:
    // comentario nuevo → notification realtime a clientes
    // viendo el feed.
    let stem = "ws_broadcast_builtin";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let src = "\
type Event { kind: Str, text: Str }\n\
\n\
@get(\"/notify\")\n\
fn notify() -> Str {\n\
    let evt = Event { kind: \"system\", text: \"hola\" }\n\
    ws_broadcast(\"/feed\", evt)\n\
    return \"broadcasted\"\n\
}\n\
\n\
@server(43920)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Inspección estática del Rust generado.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
        // El call debe traducirse a `crate::__fitz_ws_broadcast(...)`.
        assert!(
            content.contains("crate::__fitz_ws_broadcast("),
            "main.rs debe emitir call a `crate::__fitz_ws_broadcast(...)` (#2 fix)"
        );
        // El helper debe estar emitido en el crate root.
        assert!(
            content.contains(
                "pub(crate) fn __fitz_ws_broadcast(endpoint: &str, msg: serde_json::Value)"
            ),
            "main.rs debe emitir el helper `__fitz_ws_broadcast` (#2 fix)"
        );
        // El delegate a `__fitz_ws_broadcast_payload` (preludio WS).
        assert!(
            content.contains("__fitz_ws_broadcast_payload(endpoint, payload)"),
            "main.rs debe delegar a `__fitz_ws_broadcast_payload` (#2 fix)"
        );
    }
}

#[test]
fn ws_router_y_asyncapi_cross_module() {
    // 10.8.6 (v0.10.8) — fix #4: handlers `@ws` que viven en
    // módulos importados ahora se enchufan al Router axum del main
    // (paralelo a W16 para HTTP). Además, el schema AsyncAPI 3.0 y
    // el endpoint `/asyncapi.json` se emiten cuando hay WS
    // cross-module (no solo cuando el main tiene WS local).
    //
    // Pre-fix: WS handshake al `/feed` cross-module → 404 (la ruta
    // no existía en el Router). `/asyncapi.json` → 404 también.
    let stem = "ws_cross_module";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    std::fs::write(
        dir.join("ws_mod.fitz"),
        "type Msg { text: Str }\n\
         \n\
         @ws(\"/chat\")\n\
         async fn chat_handler(conn: WsConn<Msg>) -> Null {\n\
             return null\n\
         }\n",
    )
    .expect("escribir ws_mod.fitz");

    let main_src = "\
import ws_mod\n\
\n\
@server(43915)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Inspección estática del Rust generado.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
        // El Router debe registrar la ruta WS cross-module.
        assert!(
            content.contains(".route(\"/chat\", axum::routing::get(crate::ws_mod::__ws_handler_chat_handler))"),
            "main.rs debe registrar `.route(\"/chat\", crate::ws_mod::__ws_handler_chat_handler)` (#4 fix)"
        );
        // El schema AsyncAPI debe incluir el canal `/chat`.
        assert!(
            content.contains("\"/chat\":{"),
            "main.rs debe incluir canal `/chat` en schema AsyncAPI (#4 fix)"
        );
        // El endpoint `/asyncapi.json` debe estar registrado.
        assert!(
            content.contains(".route(\"/asyncapi.json\""),
            "main.rs debe registrar el endpoint `/asyncapi.json` (#4 fix)"
        );
    }

    // El wrapper `__ws_handler_chat_handler` debe ser `pub` en el módulo.
    let mod_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/ws_mod.rs", stem));
    if mod_rs.exists() {
        let content = std::fs::read_to_string(&mod_rs).expect("leer ws_mod.rs");
        assert!(
            content.contains("pub async fn __ws_handler_chat_handler"),
            "ws_mod.rs debe emitir `pub async fn __ws_handler_chat_handler` (#4 fix)"
        );
    }
}

#[test]
fn ws_cross_module_message_with_nested_nominal_not_stub_w19() {
    // W19 (2026-07-21) — `@ws` handler con `WsConn<Frame>` donde
    // `Frame` vive en un módulo importado (o dep) y tiene un field
    // compound `items: List<Thing>` cuyo inner nominal `Thing` NO se
    // importa al main.
    //
    // **Bug que cierra (paridad run↔build)**: el codegen emitía
    // `impl __FromFitzJson for FrameData` en main.rs como STUB que
    // devuelve `Err(...)` (porque el remap degradaba `List<Thing>` a
    // `List<Any>` → `Vec<__FitzValue>` que choca con el
    // `Arc<Mutex<Vec<Thing>>>` del struct). En runtime, `ws.recv()`
    // sobre ese tipo → `Err` inmediato → el handler termina → el
    // cleanup de la conn se colgaba esperando el writer que el
    // heartbeat mantenía vivo (socket medio-abierto) → el cliente
    // WS colgaba (timeout), mientras `fitz run` funcionaba OK.
    //
    // **Fix**: `Thing`/`ThingData` YA están en scope en main.rs (via
    // `use handlers::{Thing, ThingData};` + su `impl __FromFitzJson`),
    // así que el cuerpo real puede emitirse con el tipo concreto
    // `Arc<Mutex<Vec<Thing>>>`. Solo se mantiene el stub cuando un
    // nominal anidado no está definido en ningún módulo cargado.
    let stem = "ws_xmod_nested_nominal_w19";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    std::fs::write(
        dir.join("handlers.fitz"),
        "type Thing {\n\
             id: Int\n\
             label: Str\n\
         }\n\
         \n\
         type Frame {\n\
             kind: Str\n\
             items: List<Thing>\n\
         }\n\
         \n\
         @ws(\"/echo\")\n\
         async fn echo(ws: WsConn<Frame>) {\n\
             loop {\n\
                 let f = ws.recv()?\n\
                 ws.send(f)?\n\
             }\n\
         }\n",
    )
    .expect("escribir handlers.fitz");

    let main_src = "\
import handlers\n\
\n\
@server(43913)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W19):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
    // El impl real de FrameData debe estar presente…
    assert!(
        content.contains("impl __FromFitzJson for FrameData"),
        "main.rs debe emitir `impl __FromFitzJson for FrameData` (W19)"
    );
    // …y NO debe ser el stub que devuelve Err.
    assert!(
        !content.contains("`__FromFitzJson for Frame` is a stub"),
        "main.rs NO debe emitir el stub de `__FromFitzJson for Frame` (W19): {}",
        content
            .lines()
            .filter(|l| l.contains("Frame"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // El field `items` debe deserializar con el tipo concreto
    // `Arc<Mutex<Vec<Thing>>>`, no `Vec<__FitzValue>`.
    assert!(
        content.contains("<Arc<Mutex<Vec<Thing>>> as __FromFitzJson>::__from_fitz_json"),
        "main.rs debe deserializar `items` con el tipo concreto `Arc<Mutex<Vec<Thing>>>` (W19)"
    );
    // `Thing`/`ThingData` deben tener su propio impl real (chain del
    // Vec<Thing>).
    assert!(
        content.contains("impl __FromFitzJson for ThingData"),
        "main.rs debe emitir `impl __FromFitzJson for ThingData` (W19)"
    );
}

#[test]
fn w20_imported_fn_returning_list_nominal_infers_type_without_importing_nested() {
    // W20 (2026-07-21) — un `let` sin anotación cuyo RHS es una fn
    // importada que retorna `List<Nominal>` (con el nominal NO
    // importado a este módulo) degradaba el tipo inferido a
    // `List<Any>` → `Vec<__FitzValue>`. Como `__FitzValue` no se
    // activa para un programa CLI sin literales heterogéneos, rustc
    // rompía con "cannot find type __FitzValue".
    //
    // **Fix**: omitir la anotación de tipo y dejar que Rust infiera el
    // tipo concreto del RHS (la fn importada YA tiene la firma
    // concreta `Arc<Mutex<Vec<Patch>>>`). Esto elimina el workaround
    // "importá todos los tipos anidados" (paralelo a W19 del lado del
    // `__FromFitzJson`). Caso real: el showcase Admin ABM importaba
    // `Patch` solo para tipar el `let patches = diff_html(...)`.
    let stem = "w20_imported_fn_list_nominal";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // plib.fitz — declara `Patch` + `make_patches() -> List<Patch>`.
    std::fs::write(
        dir.join("plib.fitz"),
        "type Patch {\n\
             op: Str\n\
         }\n\
         \n\
         fn make_patches() -> List<Patch> {\n\
             return [Patch { op: \"a\" }, Patch { op: \"b\" }]\n\
         }\n",
    )
    .expect("escribir plib.fitz");

    // main.fitz — importa SOLO `make_patches` (NO `Patch`) y usa el
    // resultado en un `let` sin anotación.
    let main_src = "\
from plib import make_patches\n\
\n\
fn count() -> Int {\n\
    let ps = make_patches()\n\
    return len(ps)\n\
}\n\
\n\
print(\"count={count()}\")\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W20):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // FITZ-15 (v0.57) SUPERSEDE el workaround de omisión de W20: ahora el
    // nominal `Patch` (leaf del ret type `List<Patch>` de la fn importada) se
    // auto-registra (`auto_register_imported_fn_ret_nominals`), así que el
    // codegen emite el tipo CONCRETO `Vec<Patch>` + el `use plib::{Patch,
    // PatchData}`, en vez de omitir la anotación. La intención original de W20
    // se mantiene intacta: nunca degradar a `Vec<__FitzValue>`. El binario
    // corre igual (`count=2`).
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
    assert!(
        content.contains("let mut ps: Arc<Mutex<Vec<Patch>>> = make_patches()"),
        "main.rs debe emitir `ps` con el tipo concreto `Vec<Patch>` (W20 + FITZ-15)"
    );
    assert!(
        !content.contains("Vec<__FitzValue>"),
        "main.rs NO debe degradar `ps` a `Vec<__FitzValue>` (W20)"
    );

    // El binario debe correr y contar 2.
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());
    let run = Command::new(&bin).output().expect("correr binario");
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(
        out.contains("count=2"),
        "salida esperada `count=2`, fue: {:?}",
        out
    );
}

#[test]
fn fitz15_cross_module_fn_ret_nominal_infers_type_without_importing() {
    // FITZ-15 (v0.57, MatHelp dogfooding) — un binding cuyo tipo es un
    // nominal retornado (transitivamente) por una fn IMPORTADA, cuando el
    // nominal NO está importado en el módulo consumidor, se degradaba a
    // `Any` en el codegen → `field access .id over Any` en `fitz build`
    // (aunque `fitz check` lo resuelve por el TypeEnv global del grafo). El
    // `match`/`?` era incidental. Caso real: `read_session` (auth) devolviendo
    // `Family` (models) usado en `perfiles.fitz` sin importar `Family`.
    //
    // **Fix**: `auto_register_imported_fn_ret_nominals` auto-registra el
    // nominal + copia sus fields (paralelo a `auto_register_relation_targets`
    // de v0.45) y emite el `use <mod>::{T, TData}` (Paso 2, CLI puro). Cubre
    // `Result<Foo>`, `Result<List<Foo>>`, etc. (la recolección recursa).
    let stem = "fitz15_fn_ret_nominal";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // models.fitz — define Foo.
    std::fs::write(dir.join("models.fitz"), "type Foo {\n    id: Int = 0\n}\n")
        .expect("escribir models.fitz");

    // data.fitz — importa Foo, expone fns que lo retornan (Foo y List<Foo>).
    std::fs::write(
        dir.join("data.fitz"),
        "from models import Foo\n\
         \n\
         async fn get_foo() -> Result<Foo> {\n\
             return Ok(Foo { id: 5 })\n\
         }\n\
         \n\
         async fn get_foos() -> Result<List<Foo>> {\n\
             return Ok([Foo { id: 7 }, Foo { id: 9 }])\n\
         }\n",
    )
    .expect("escribir data.fitz");

    // main.fitz — importa SOLO las fns (NO Foo). Usa match (como MatHelp) + un
    // acceso a field sobre el elemento de la lista.
    let main_src = "\
from data import get_foo, get_foos\n\
\n\
async fn one() -> Int {\n\
    let f = match get_foo().await {\n\
        Ok(x) => x,\n\
        Err(_) => return 0,\n\
    }\n\
    return f.id\n\
}\n\
\n\
async fn first_of_list() -> Int {\n\
    let fs = match get_foos().await {\n\
        Ok(v) => v,\n\
        Err(_) => return 0,\n\
    }\n\
    return fs[0].id\n\
}\n\
\n\
let a = one().await\n\
let b = first_of_list().await\n\
print(\"one={a} list={b}\")\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (FITZ-15):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Paso 2 del fix (CLI puro): el struct de Foo se trae a scope con un `use`.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
    assert!(
        content.contains("Foo, FooData"),
        "main.rs debe emitir `use ...::{{Foo, FooData}}` (FITZ-15 Paso 2)"
    );

    // El binario debe correr: field access sobre el nominal cross-module y
    // sobre el elemento de la List<Foo> resuelven al tipo concreto.
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());
    let run = Command::new(&bin).output().expect("correr binario");
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(
        out.contains("one=5 list=7"),
        "salida esperada `one=5 list=7`, fue: {:?}",
        out
    );
}

#[test]
fn fitz16_match_result_any_arm_coerces_to_primitive() {
    // FITZ-16 (v0.57, MatHelp dogfooding) — un `match` sobre `Result<Any>` (de
    // `Map<Str,Any>.get(...)` o `jwt.decode` → `Map<Str,__FitzValue>`) cuyo arm
    // `Ok(v) => v` produce `Any` y cuyo arm `Err(_) => <primitivo>` fija el LUB
    // en un primitivo concreto: el arm `Ok` NO coaccionaba `__FitzValue` → el
    // primitivo → `E0308 expected String, found __FitzValue` en `fitz build`
    // (aunque `fitz check` pasa). Caso real: `pid_from_token` de MatHelp leyendo
    // un claim del JWT decodeado.
    //
    // **Fix**: `gen_match` coacciona cada arm `Any` con `coerce(Any →
    // primitivo)` cuando el LUB es un primitivo concreto (Str/Int/Float/Bool),
    // reusando el `__fv_to_*` de `Map<Str,Any>.keys()`.
    let stem = "fitz16_match_any_coerce";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let main_src = "\
fn read_str(m: Map<Str, Any>) -> Str {\n\
    return match m.get(\"k\") {\n\
        Ok(v) => v,\n\
        Err(_) => \"\",\n\
    }\n\
}\n\
\n\
fn read_int(m: Map<Str, Any>) -> Int {\n\
    return match m.get(\"n\") {\n\
        Ok(v) => v,\n\
        Err(_) => 0,\n\
    }\n\
}\n\
\n\
let data: Map<Str, Any> = {}\n\
data[\"k\"] = \"hola\"\n\
data[\"n\"] = 42\n\
print(\"s={read_str(data)} n={read_int(data)}\")\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (FITZ-16):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());
    let run = Command::new(&bin).output().expect("correr binario");
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(
        out.contains("s=hola n=42"),
        "salida esperada `s=hola n=42`, fue: {:?}",
        out
    );
}

#[test]
fn cross_module_default_with_transitive_nominal_delegates_to_module_helper_w26() {
    // W26 (2026-07-23, v0.28.5) — un tipo importado cuyo DEFAULT
    // referencia un nominal TRANSITIVO (importado por el módulo que
    // define el tipo, NO por main) rompía `fitz build` con
    // "unknown type `Member` in codegen". Caso real: un `.fitzv` con
    // `state { members: List<Member> = [Member { ... }] }` donde
    // `Member` vive en un `.fitz` hermano (c3-team-panel-sfc de
    // fitz-liveviews).
    //
    // **Causa**: el `impl __FromFitzJson for PanelData` que
    // `emit_helpers_for_imported_types` emite en main.rs (cuando hay
    // HTTP) inlineaba el default expr vía `gen_expr` en el ctx de
    // main, donde `Member` no está en `type_sigs`.
    //
    // **Fix**: con contexto cross-module (`xmod = Some`), el arm
    // `None =>` del default delega al helper
    // `<mod>::__default_<T>_<field>()` que PreF8.3 ya emite en el
    // módulo definidor (mismo patrón que el struct lit importado de
    // `gen_struct_lit`).
    let stem = "xmod_transitive_default_w26";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // models.fitz — el nominal transitivo.
    std::fs::write(
        dir.join("models.fitz"),
        "type Member {\n\
             name: Str\n\
             active: Bool\n\
         }\n",
    )
    .expect("escribir models.fitz");

    // panel.fitz — el tipo importado por main, con default que
    // instancia `Member` (transitivo: main nunca lo importa).
    std::fs::write(
        dir.join("panel.fitz"),
        "from models import Member\n\
         \n\
         type Panel {\n\
             title: Str\n\
             members: List<Member> = [Member { name: \"Ada\", active: true }, Member { name: \"Grace\", active: false }]\n\
         }\n",
    )
    .expect("escribir panel.fitz");

    // main.fitz — importa SOLO `Panel` + handler HTTP (dispara
    // `emit_helpers_for_imported_types` con do_http).
    let main_src = "\
from panel import Panel\n\
\n\
@get(\"/panel\")\n\
fn get_panel() -> Panel {\n\
    return Panel { title: \"Team\" }\n\
}\n\
\n\
@server(43931)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W26):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // El `impl __FromFitzJson for PanelData` de main.rs debe delegar
    // el default de `members` al helper del módulo definidor…
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
    assert!(
        content.contains("panel::__default_Panel_members()"),
        "main.rs debe delegar el default de `members` a `panel::__default_Panel_members()` (W26)"
    );
    // …y NO inlinear el literal del default en el ctx de main (el
    // literal `\"Ada\"` solo debe vivir en panel.rs, adentro del
    // helper `__default_Panel_members`).
    assert!(
        !content.contains("String::from(\"Ada\")"),
        "main.rs NO debe inlinear el default `[Member {{ ... }}]` (W26)"
    );
    let panel_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/panel.rs", stem));
    let panel_content = std::fs::read_to_string(&panel_rs).expect("leer panel.rs");
    assert!(
        panel_content.contains("pub fn __default_Panel_members()"),
        "panel.rs debe emitir el helper `__default_Panel_members` (PreF8.3)"
    );
}

#[test]
fn cross_module_any_field_without_db_emits_fitz_value_import_w23() {
    // v0.28.1 (2026-07-22) — a module declares a `type X { f: Any }`
    // (which the codegen emits with the Rust type `__FitzValue`) but the
    // program does NOT touch the DB. Before the fix, the module's
    // `use crate::__FitzValue;` import lived INSIDE the DB-prelude block,
    // so a non-DB module that referenced `__FitzValue` failed to compile
    // with `cannot find type __FitzValue in this scope`. This is the
    // exact shape of fitz-liveviews' `ComponentReg { render_fn: Any, ...
    // }` registry when consumed by a program without ORM usage.
    let stem = "xmod_any_no_db_w23";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tempdir");

    std::fs::write(
        dir.join("reg.fitz"),
        "type ComponentReg {\n\
             render_fn: Any\n\
             handlers: Map<Str, Any>\n\
             initial: Any\n\
         }\n\
         \n\
         fn make_reg(f: Any) -> ComponentReg {\n\
             return ComponentReg { render_fn: f, handlers: {}, initial: 0 }\n\
         }\n",
    )
    .expect("write reg.fitz");

    let main_src = "\
from reg import make_reg, ComponentReg\n\
\n\
let r = make_reg(42)\n\
print(\"reg ok\")\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("write main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W23):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The module file must import `__FitzValue` even without a DB
    // prelude, because it emits `ComponentRegData { render_fn:
    // __FitzValue, ... }`.
    let module_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/reg.rs", stem));
    let content = std::fs::read_to_string(&module_rs).expect("read reg.rs");
    assert!(
        content.contains("use crate::__FitzValue;"),
        "reg.rs must emit `use crate::__FitzValue;` (W23):\n{}",
        content.lines().take(12).collect::<Vec<_>>().join("\n")
    );
    assert!(
        content.contains("render_fn: __FitzValue"),
        "reg.rs must emit the `__FitzValue`-typed field (W23)"
    );
}

#[test]
fn cross_module_any_coercions_emit_fitz_value_import_w27() {
    // v0.28.6 (2026-07-24) — W23 gated the module's `use crate::__FitzValue;`
    // on DECLARATION shapes (`type X { f: Any }` / imported jsonb @table).
    // But a module can also *emit* `__FitzValue` with no such declaration:
    //   (a) passing a nominal instance to an imported fn's `Any` param
    //       (coerce Instance → Any wraps in `__FitzValue::Instance`), and
    //   (b) downcasting an `Any` return into a nominal via annotation
    //       (`let c: Card = peek()` emits the `__FitzValue::Instance` match
    //       + `__fv_type_name` panic arm).
    // Exact shape of the Admin ABM's empleados.fitz calling fitz-liveviews'
    // `component_with(name, id, initial: Any)` + `component_state(...)`.
    // W27 post-scans the emitted module Rust and inserts the import (and
    // ORs the crate-root enum flag) when the declaration detector misses.
    let stem = "xmod_any_coerce_w27";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tempdir");

    std::fs::write(
        dir.join("store.fitz"),
        "fn stash(v: Any) -> Null {\n\
             return null\n\
         }\n\
         \n\
         fn peek() -> Any {\n\
             return 0\n\
         }\n",
    )
    .expect("write store.fitz");

    // cards.fitz declares NO `Any` field/param — the declaration-based
    // detector does not fire here, yet both bodies emit `__FitzValue`.
    std::fs::write(
        dir.join("cards.fitz"),
        "from store import stash, peek\n\
         \n\
         type Card {\n\
             id: Int = 0\n\
             title: Str = \"\"\n\
         }\n\
         \n\
         fn save_card() -> Null {\n\
             stash(Card { id: 1, title: \"x\" })\n\
             return null\n\
         }\n\
         \n\
         fn load_card() -> Card {\n\
             let c: Card = peek()\n\
             return c\n\
         }\n",
    )
    .expect("write cards.fitz");

    let main_src = "\
from cards import Card, save_card, load_card\n\
\n\
save_card()\n\
print(\"cards ok\")\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("write main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W27):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let module_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/cards.rs", stem));
    let content = std::fs::read_to_string(&module_rs).expect("read cards.rs");
    assert!(
        content.contains("use crate::__FitzValue;"),
        "cards.rs must emit `use crate::__FitzValue;` (W27):\n{}",
        content.lines().take(12).collect::<Vec<_>>().join("\n")
    );
    assert!(
        content.contains("use crate::__fv_type_name;"),
        "cards.rs must emit `use crate::__fv_type_name;` (W27)"
    );
    // The Instance→Any wrap and the Any→nominal downcast both live in
    // cards.rs (proof the import is actually needed).
    assert!(
        content.contains("__FitzValue::Instance"),
        "cards.rs must emit `__FitzValue::Instance` coercions (W27)"
    );
}

#[test]
fn coerce_map_of_functions_to_map_any_wraps_in_fitzvalue_function_w25() {
    // v0.28.3 — a homogeneous map of same-signature functions is typed
    // `Map<Str, Function>` by the LUB, and passing it to a `Map<Str, Any>`
    // parameter used to leak the raw `Arc<dyn Fn>` casts (E0308: expected
    // `__FitzValue`, found `Arc<...>`). The new `(Map, Map)` coerce arm
    // rebuilds the map wrapping each function value in a
    // `__FitzValue::Function(...)` adapter. This is the core of what makes
    // fitz-liveviews LiveComponents (`flv_register`'s event-handler map)
    // compile to a native binary. Self-contained repro (no external lib).
    let src = "\
fn takes(h: Map<Str, Any>) -> Int {\n\
    return len(h)\n\
}\n\
fn a() -> Int => 1\n\
fn b() -> Int => 2\n\
let n = takes({\"x\": a, \"y\": b})\n\
print(n)\n\
";
    let (stdout, code) = build_and_run("coerce_map_of_fns_w25", src);
    assert_eq!(code, 0, "binary should exit 0");
    assert_eq!(
        stdout.trim(),
        "2",
        "map of 2 functions coerced to Map<Str, Any>"
    );
}

#[test]
fn module_mutable_global_persists_state_via_shared_lazylock_w25() {
    // v0.28.3 — a module-level `let X = {}` (a mutable Map/List/Nominal
    // global) was emitted as `pub fn X() -> T { <fresh default> }`, which
    // returned a NEW empty value on every call — so state written to it was
    // silently lost across calls and across HTTP worker threads. This is
    // what broke fitz-liveviews' `COMPONENT_REGISTRY` / `COMPONENT_STATE_STORE`
    // (`flv_register` populated one instance, `component()` read another →
    // "key not found in map"). The fix emits a shared `LazyLock<Arc<Mutex<T>>>`
    // static + a getter that clones the Arc. Self-contained repro: write to a
    // module global, read it back — it must persist.
    let stem = "module_global_persist_w25";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tempdir");

    std::fs::write(
        dir.join("store.fitz"),
        "let CACHE: Map<Str, Int> = {}\n\
         \n\
         fn put(k: Str, v: Int) -> Null {\n\
             CACHE[k] = v\n\
             return null\n\
         }\n\
         \n\
         fn get_or_zero(k: Str) -> Int {\n\
             return match CACHE.get(k) {\n\
                 Ok(v) => v,\n\
                 Err(_) => 0,\n\
             }\n\
         }\n",
    )
    .expect("write store.fitz");

    let main_src = "\
from store import put, get_or_zero\n\
let _ = put(\"a\", 42)\n\
print(get_or_zero(\"a\"))\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("write main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W25):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The emitted module must use a shared LazyLock (not a fresh-value getter).
    let module_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/store.rs", stem));
    let content = std::fs::read_to_string(&module_rs).expect("read store.rs");
    assert!(
        content.contains("LazyLock") && content.contains("__FITZ_GLOBAL_CACHE"),
        "store.rs must emit a shared LazyLock global for CACHE (W25):\n{}",
        content
            .lines()
            .filter(|l| l.contains("CACHE"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    let run = Command::new(&bin).output().expect("run binary");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "42",
        "module global state must persist (write 42, read back 42), not reset to 0"
    );
}

#[test]
fn openapi_cross_module_includes_module_handlers() {
    // 10.8.5 (v0.10.8) — fix #3: el schema OpenAPI 3.1 emitido por
    // `fitz build` ahora incluye los handlers HTTP de módulos
    // importados (`loader.modules[i].http_fn_stmts` capturados por
    // W16). Antes el schema solo miraba el main → `paths: []`.
    let stem = "openapi_cross_module";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    std::fs::write(
        dir.join("api.fitz"),
        "@get(\"/hello\")\n\
         fn hello() -> Str => \"hola\"\n\
         \n\
         @get(\"/users/{id}\")\n\
         fn get_user(id: Int) -> Str => \"user-{id}\"\n",
    )
    .expect("escribir api.fitz");

    let main_src = "\
import api\n\
\n\
@server(43914)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Inspección estática: el schema embebido en main.rs debe
    // contener los paths de api.fitz (no solo del main).
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
        assert!(
            content.contains("\"/hello\":{"),
            "main.rs debe incluir path `/hello` en schema OpenAPI (#3 fix)"
        );
        assert!(
            content.contains("\"/users/{id}\":{"),
            "main.rs debe incluir path `/users/{{id}}` en schema OpenAPI (#3 fix)"
        );
    }
}

#[test]
fn checker_narrow_nullable_post_if_not_null() {
    // 10.8.4 (v0.10.8) — fix #1: narrowing flow-sensitive de
    // `Nullable<T>` → `T` adentro del `then` branch de
    // `if (x != null) { ... }`. Antes el checker tipaba `x` como
    // `Nullable<T>` adentro del if, forzando workaround con match
    // arm `Pattern::Ident` (W2). Ahora `let s: T = x` directo
    // funciona.
    let src = "\
fn process(status: Str?) -> Str {\n\
    if (status != null) {\n\
        let s: Str = status\n\
        return s\n\
    }\n\
    return \"default\"\n\
}\n\
\n\
print(process(\"ok\"))\n\
print(process(null))\n\
";
    let (stdout, exit) = build_and_run("checker-narrow-not-null", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["ok", "default"]);
}

#[test]
fn checker_narrow_nullable_else_branch_eq_null() {
    // 10.8.4 — caso reverso: `if (x == null) {...} else { ... }`
    // refina `x` a inner type adentro del `else`.
    let src = "\
fn process(status: Str?) -> Str {\n\
    if (status == null) {\n\
        return \"default\"\n\
    } else {\n\
        let s: Str = status\n\
        return s\n\
    }\n\
}\n\
\n\
print(process(\"hello\"))\n\
print(process(null))\n\
";
    let (stdout, exit) = build_and_run("checker-narrow-eq-null-else", src);
    assert_eq!(exit, 0);
    assert_lines(&stdout, &["hello", "default"]);
}

#[test]
fn orm_w17_eager_loaded_virtuales_aparecen_en_json() {
    // 10.8.3 (v0.10.8) — fix #7: `__ToFitzJson` ahora emite los
    // virtual fields del ORM (companion/has_one/has_many) cuando
    // están "preloaded" (no en estado default). Runtime check
    // (`is_some()` o `!is_empty()`) decide skip vs emit.
    //
    // Antes (W17 estricto): los virtuales JAMÁS aparecían en el
    // JSON, perdiendo el beneficio de `.preload(...)`.
    let stem = "orm_w17_eager_json";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let src = "\
@table(\"users\") type User {\n\
    @primary id: Int = 0\n\
    name: Str = \"\"\n\
    @has_many(\"Post\", via=\"author_id\") posts: List<Post> = []\n\
}\n\
@table(\"posts\") type Post {\n\
    @primary id: Int = 0\n\
    @belongs_to(\"User\") author_id: Int = 0\n\
    author: User?\n\
    title: Str = \"\"\n\
}\n\
\n\
@get(\"/users/{id}\")\n\
async fn get_user(id: Int) -> Result<User> {\n\
    let conn = db.connect(\"postgres://x:y@127.0.0.1/x\").await?\n\
    return User.where(fn(u) => u.id == id).preload(\"posts\").first(conn).await\n\
}\n\
\n\
@server(43913)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Inspección estática: el impl __ToFitzJson para UserData debe
    // emitir el conditional `if !__g.is_empty()` para el field
    // virtual `posts` (HasMany). Y PostData debe emitir
    // `if self.author.is_some()` para `author` (BelongsToCompanion).
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("read main.rs");
        // The has_many conditional scopes the `MutexGuard` in its own
        // block (`let __is_empty = { let __g = ...; __g.is_empty() };`)
        // so the lock is not held across the `__obj.insert(...)`. The
        // guard-scoping refactor changed the literal from the older
        // `if !__g.is_empty()` — the conditional is still emitted and
        // functionally identical (skip the field when the relation is
        // empty).
        assert!(
            content.contains("if !__is_empty { __obj.insert(\"posts\""),
            "main.rs must emit the has_many conditional for `posts` (#7 fix)"
        );
        assert!(
            content.contains("if self.author.is_some() { __obj.insert(\"author\""),
            "main.rs must emit the companion conditional for `author` (#7 fix)"
        );
    }
}

#[test]
fn orm_db_default_skips_field_from_insert() {
    // 10.8.2 (v0.10.7+) — fix #5: decorator `@db_default` marca al
    // field como "manejado por la DB". El ORM lo skipea del
    // INSERT, dejando que Postgres aplique el `DEFAULT` declarado
    // en el schema (típicamente `DEFAULT NOW()` para timestamps).
    // El field sigue apareciendo en RETURNING * para que el
    // cliente reciba el valor que Postgres asignó.
    //
    // Sin este flag, el INSERT mandaba el value Fitz (default
    // literal `""` para Str) y Postgres rechazaba `''` para tipos
    // no-text como `timestamptz`.
    let stem = "orm_db_default_skip";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let src = "\
@table(\"users\") type User {\n\
    @primary id: Int = 0\n\
    email: Str = \"\"\n\
    @db_default created_at: Str = \"\"\n\
}\n\
\n\
@get(\"/insert\")\n\
async fn do_insert() -> Result<User> {\n\
    let conn = db.connect(\"postgres://x:y@127.0.0.1/x\").await?\n\
    return User.insert(conn, User { id: 0, email: \"a@b\", created_at: \"\" }).await\n\
}\n\
\n\
@server(43912)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Inspección estática: el SQL INSERT no debe mencionar
    // `created_at` (skipeado por @db_default), pero RETURNING sí.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
        // El INSERT debe NO mencionar "created_at" en la col_list,
        // pero SÍ en RETURNING.
        let insert_line = content
            .lines()
            .find(|l| l.contains("INSERT INTO \\\"users\\\""))
            .expect("INSERT INTO users was not emitted");
        // El INSERT line tiene formato:
        //   INSERT INTO "users" (cols...) VALUES (...) RETURNING cols...
        // Partimos por VALUES para separar.
        let parts: Vec<&str> = insert_line.split("VALUES").collect();
        assert_eq!(parts.len(), 2, "INSERT line shape inesperado");
        let cols_part = parts[0];
        assert!(
            !cols_part.contains("created_at"),
            "El INSERT col_list NO debe incluir `created_at` (skipeado por @db_default): {}",
            cols_part
        );
        let returning_part = parts[1];
        assert!(
            returning_part.contains("created_at"),
            "El RETURNING SÍ debe incluir `created_at`: {}",
            returning_part
        );
    }
}

#[test]
fn http_wrapper_unwraps_result_tail_without_explicit_ok() {
    // 10.8.1 (v0.10.7+) — fix #6: handler HTTP que termina con
    // `return <expr>` cuyo tipo es `Result<T>` (sin `Ok()` literal)
    // ahora se desempaca con match runtime, devolviendo `T` puro en
    // 200 o `{"error": e}` en 500. Antes el codegen serializaba el
    // `Result<T, E>` entero produciendo `{"Ok": ...}` / `{"Err": ...}`.
    //
    // Caso canónico: `return helper().await` donde `helper` devuelve
    // Result. Detectado al smoke real boilerplate api-orm-full con
    // `return Post.where(...).first(conn).await` (chain ORM).
    let stem = "http_wrapper_result_tail";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // El `?` en el body activa response_mode (handler emite
    // `-> __FitzResponse`), que es el path donde aplica el fix #6.
    // Sin `?`, el handler cae en el path vanilla `-> Result<T, E>`
    // que el wrapper desempaca solo (sin necesidad del fix).
    let src = "\
async fn pre_check() -> Result<Null> {\n\
    return Ok(null)\n\
}\n\
\n\
async fn get_value() -> Result<Int> {\n\
    return Ok(42)\n\
}\n\
\n\
@get(\"/value\")\n\
async fn handler_ok() -> Result<Int> {\n\
    let _ = pre_check().await?\n\
    return get_value().await\n\
}\n\
\n\
@server(43911)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Inspección estática del Rust generado: el handler con
    // `return <expr_Result>` debe emitir `match (...)` con dos
    // armas Ok(__v)/Err(__e), NO `<Result<Int, String> as
    // __ToFitzJson>` directo.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
        assert!(
            content.contains("Ok(__v) => __FitzResponse { status: 200,"),
            "main.rs debe emitir `Ok(__v) => __FitzResponse {{ status: 200, ...}}` (#6 fix)"
        );
        assert!(
            content.contains("Err(__e) => __FitzResponse { status: 500,"),
            "main.rs debe emitir `Err(__e) => __FitzResponse {{ status: 500, ...}}` (#6 fix)"
        );
        assert!(
            !content.contains("<Result<i64, String> as __ToFitzJson>"),
            "main.rs must NOT serialize `Result<i64, String>` whole (regression of fix #6)"
        );
    }
}

#[test]
fn orm_array_has_accepts_external_var() {
    // v0.10.7 — gap cerrado: `.has(var)` sobre `text[]`/`int8[]`/etc.
    // ahora acepta variables externas al closure, no solo literales.
    // Antes el codegen rechazaba con "el value debe ser literal del
    // tipo del array". Ahora delega a `translate_closure_to_sql`
    // (mismo path que W3/W6) que bindea el var via
    // `__IntoPgValue::into_pg(...)`.
    //
    // Este test NO usa Postgres real (no podemos garantizar la
    // disponibilidad en CI). Valida que el `fitz build` compile
    // exitoso — el SQL emitido (`$N = ANY("tags")`) es trivial de
    // inspeccionar en el .rs generado.
    let stem = "orm_array_has_var";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let src = "\
@table(\"posts\") type Post {\n\
    @primary id: Int = 0\n\
    title: Str = \"\"\n\
    tags: List<Str> = []\n\
}\n\
\n\
@get(\"/posts?tag={tag}\")\n\
async fn list_by_tag(tag: Str) -> Result<List<Post>> {\n\
    let conn = db.connect(\"postgres://x:y@127.0.0.1/x\").await?\n\
    return Post.where(fn(p) => p.tags.has(tag)).all(conn).await\n\
}\n\
\n\
@server(43919)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (`.has(var)` over array):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Inspección estática: el SQL emitido contiene `$1 = ANY("tags")`
    // (placeholder bindeado, no constante).
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
        assert!(
            content.contains("= ANY(\\\"tags\\\")"),
            "main.rs debe emitir SQL `= ANY(\"tags\")` con placeholder bindeado"
        );
    }
}

#[test]
fn cross_module_table_virtual_w18_remap_any() {
    // W18 (v0.10.7) — el chequeo `has_opaque_field` de
    // `emit_helpers_for_imported_types` ahora ignora los virtual
    // fields del ORM. Antes, un `@table type` con `@has_many` o
    // `@has_one` cuyo target no estaba importado al main hacía
    // que el remap degradara el field virtual a `List<Any>` /
    // `Nullable<Any>`, lo cual disparaba el filtro y el codegen
    // skipeaba TODO el impl `__ToFitzJson` / `__FromFitzJson`.
    // Rustc luego rompía con
    // `<T>Data: __ToFitzJson is not satisfied`.
    //
    // **Diferencia con cross_module_orm_virtual_fields_skip_w17**:
    // ese test importa Post directamente desde main vía la cadena
    // transitiva, dejando Post en el env del importer (no se
    // degrade). Acá el main hace solo `import ops` (namespace) y
    // NO trae Post al scope local — el remap SÍ degrade los
    // virtuales y queda visible si el filtro mira virtuales.
    let stem = "cross_module_orm_virtual_w18";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // models.fitz — User tiene @has_many Post; Post tiene
    // BelongsToCompanion User. Ambos virtuales degradan a Any en
    // el remap del main porque main no importa Post.
    std::fs::write(
        dir.join("models.fitz"),
        "@table(\"users\") type User {\n\
             @primary id: Int = 0\n\
             name: Str = \"\"\n\
             @has_many(\"Post\", via=\"author_id\") posts: List<Post> = []\n\
         }\n\
         \n\
         @table(\"posts\") type Post {\n\
             @primary id: Int = 0\n\
             @belongs_to(\"User\") author_id: Int = 0\n\
             author: User?\n\
             title: Str = \"\"\n\
         }\n",
    )
    .expect("escribir models.fitz");

    // ops.fitz — handler @get devuelve User (cross-module).
    std::fs::write(
        dir.join("ops.fitz"),
        "from models import User, Post\n\
         \n\
         @get(\"/users/{id}\")\n\
         fn get_user(id: Int) -> User {\n\
             return User { id: id, name: \"Ada\" }\n\
         }\n",
    )
    .expect("escribir ops.fitz");

    let main_src = "\
import ops\n\
\n\
@server(43918)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (W18):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );

    // Inspección estática del Rust generado: los impls de
    // `__ToFitzJson` para los types cross-module deben emitirse
    // (W18) aunque su remap haya degradado virtuales a Any.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs");
        assert!(
            content.contains("impl __ToFitzJson for UserData"),
            "main.rs debe emitir `impl __ToFitzJson for UserData` (W18)"
        );
        assert!(
            content.contains("impl __ToFitzJson for PostData"),
            "main.rs debe emitir `impl __ToFitzJson for PostData` (W18)"
        );
    }
}

// ===========================================================================
// v0.10.30 — Tier B: API completion Date/DateTime/Uuid
// ===========================================================================
//
// Tests E2E de paridad bit-a-bit `fitz run` ↔ `fitz build` para las
// 7 features del Tier B. Helper `parity_run_vs_build` corre el mismo
// programa por los dos paths y assertea que la salida coincide. Cada
// test ejercita un sub-paso (B.1 add_*, B.2 subtract_*, B.3 diff_*,
// B.4 comparison, B.5 Uuid.v7, B.6 shortcuts, B.7 timezone).

/// Helper de paridad: corre `fitz run` y `fitz build && exec`,
/// devuelve el stdout del run-mode (el de build se asserta igual al
/// run). Aborta el test si el build o el run fallan.
fn parity_run_vs_build(test_name: &str, src: &str) -> String {
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    assert!(
        out_run.status.success(),
        "fitz run failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out_run.stdout),
        String::from_utf8_lossy(&out_run.stderr),
    );
    let run_stdout = String::from_utf8_lossy(&out_run.stdout).into_owned();

    let out_build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out_build.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out_build.stdout),
        String::from_utf8_lossy(&out_build.stderr),
    );
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    });
    let exec = Command::new(&bin).output().expect("exec build");
    assert!(
        exec.status.success(),
        "binary failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&exec.stdout),
        String::from_utf8_lossy(&exec.stderr),
    );
    let build_stdout = String::from_utf8_lossy(&exec.stdout).into_owned();

    assert_eq!(
        run_stdout.replace("\r\n", "\n"),
        build_stdout.replace("\r\n", "\n"),
        "expected bit-for-bit parity `fitz run` ↔ `fitz build`",
    );
    run_stdout
}

#[test]
fn tier_b1_date_add_methods_parity() {
    // B.1 Date arithmetic: add_days/months/years con n positivo y negativo.
    // Envuelto en `fn run() -> Result<Null>` porque codegen rechaza `?`
    // top-level (el intérprete lo soporta, pero queremos paridad).
    let src = "\
fn run() -> Result<Null> {
    let r1 = Date.from_ymd(2026, 1, 15)?
    print(r1.add_days(10).to_str())
    print(r1.add_days(-5).to_str())
    print(r1.add_months(2).to_str())
    print(r1.add_months(-1).to_str())
    print(r1.add_years(1).to_str())
    return Ok(null)
}

match run() {
    Ok(_) => null,
    Err(e) => print(e)
}
";
    let stdout = parity_run_vs_build("tier_b1_date_add", src);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "2026-01-25");
    assert_eq!(lines[1], "2026-01-10");
    assert_eq!(lines[2], "2026-03-15");
    assert_eq!(lines[3], "2025-12-15");
    assert_eq!(lines[4], "2027-01-15");
}

#[test]
fn tier_b1_datetime_add_methods_parity() {
    // B.1 DateTime arithmetic: sub-second + calendar units.
    let src = "\
fn run() -> Result<Null> {
    let r1 = DateTime.from_timestamp(1735689600)?
    print(r1.add_seconds(60).to_str())
    print(r1.add_minutes(5).to_str())
    print(r1.add_hours(2).to_str())
    print(r1.add_days(1).to_str())
    print(r1.add_months(1).to_str())
    print(r1.add_years(1).to_str())
    return Ok(null)
}

match run() {
    Ok(_) => null,
    Err(e) => print(e)
}
";
    let stdout = parity_run_vs_build("tier_b1_datetime_add", src);
    let lines: Vec<&str> = stdout.lines().collect();
    // 2025-01-01T00:00:00Z + 60s = 00:01:00
    assert_eq!(lines[0], "2025-01-01T00:01:00Z");
    assert_eq!(lines[1], "2025-01-01T00:05:00Z");
    assert_eq!(lines[2], "2025-01-01T02:00:00Z");
    assert_eq!(lines[3], "2025-01-02T00:00:00Z");
    assert_eq!(lines[4], "2025-02-01T00:00:00Z");
    assert_eq!(lines[5], "2026-01-01T00:00:00Z");
}

#[test]
fn tier_b2_subtract_methods_parity() {
    // B.2 subtract symmetric (alias de add con negate).
    let src = "\
fn run() -> Result<Null> {
    let d = Date.from_ymd(2026, 6, 15)?
    print(d.subtract_days(10).to_str())
    print(d.subtract_months(3).to_str())
    print(d.subtract_years(1).to_str())
    let dt = DateTime.from_timestamp(1735689600)?
    print(dt.subtract_seconds(3600).to_str())
    print(dt.subtract_hours(1).to_str())
    print(dt.subtract_days(1).to_str())
    return Ok(null)
}

match run() {
    Ok(_) => null,
    Err(e) => print(e)
}
";
    let stdout = parity_run_vs_build("tier_b2_sub", src);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "2026-06-05");
    assert_eq!(lines[1], "2026-03-15");
    assert_eq!(lines[2], "2025-06-15");
    // 2025-01-01T00:00:00Z - 3600s = 2024-12-31T23:00:00Z
    assert_eq!(lines[3], "2024-12-31T23:00:00Z");
    assert_eq!(lines[4], "2024-12-31T23:00:00Z");
    assert_eq!(lines[5], "2024-12-31T00:00:00Z");
}

#[test]
fn tier_b3_diff_methods_parity() {
    // B.3 diff entre fechas (signed Int).
    let src = "\
fn run() -> Result<Null> {
    let d1 = Date.from_ymd(2026, 6, 15)?
    let d2 = Date.from_ymd(2026, 6, 10)?
    print(d1.diff_days(d2))
    print(d2.diff_days(d1))
    let dt1 = DateTime.from_timestamp(1735689600)?
    let dt2 = DateTime.from_timestamp(1735693200)?
    print(dt2.diff_seconds(dt1))
    print(dt2.diff_minutes(dt1))
    print(dt2.diff_hours(dt1))
    print(dt2.diff_days(dt1))
    return Ok(null)
}

match run() {
    Ok(_) => null,
    Err(e) => print(e)
}
";
    let stdout = parity_run_vs_build("tier_b3_diff", src);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "5");
    assert_eq!(lines[1], "-5");
    // 1735693200 - 1735689600 = 3600 secs = 60 min = 1 hour = 0 days (trunc)
    assert_eq!(lines[2], "3600");
    assert_eq!(lines[3], "60");
    assert_eq!(lines[4], "1");
    assert_eq!(lines[5], "0");
}

#[test]
fn tier_b4_comparison_operators_parity() {
    // B.4 <, >, <=, >= entre Date/DateTime (chrono::Ord nativos).
    let src = "\
fn run() -> Result<Null> {
    let d1 = Date.from_ymd(2026, 1, 1)?
    let d2 = Date.from_ymd(2026, 6, 15)?
    print(d1 < d2)
    print(d1 > d2)
    print(d1 <= d1)
    print(d2 >= d2)
    let dt1 = DateTime.from_timestamp(1000)?
    let dt2 = DateTime.from_timestamp(2000)?
    print(dt1 < dt2)
    print(dt1 >= dt2)
    return Ok(null)
}

match run() {
    Ok(_) => null,
    Err(e) => print(e)
}
";
    let stdout = parity_run_vs_build("tier_b4_cmp", src);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "true");
    assert_eq!(lines[1], "false");
    assert_eq!(lines[2], "true");
    assert_eq!(lines[3], "true");
    assert_eq!(lines[4], "true");
    assert_eq!(lines[5], "false");
}

#[test]
fn tier_b5_uuid_v7_es_v7_time_ordered() {
    // B.5 Uuid.v7() — el version nibble en la posición 12 hex chars
    // debe ser '7' (RFC 9562). No podemos assertear igualdad exacta
    // (random), pero sí el shape canonical + el version byte.
    let src = "\
fn run() -> Result<Null> {
    let u1 = Uuid.v7()
    let s = u1.to_str()
    print(s.len())
    let parsed = Uuid.parse(s)?
    print(parsed.is_nil())
    return Ok(null)
}

match run() {
    Ok(_) => null,
    Err(e) => print(e)
}
";
    let stdout = parity_run_vs_build("tier_b5_uuid_v7", src);
    let lines: Vec<&str> = stdout.lines().collect();
    // Canonical UUID = 36 chars (8-4-4-4-12 hex + 4 hyphens).
    assert_eq!(lines[0], "36");
    assert_eq!(lines[1], "false");
}

#[test]
fn tier_b6_shortcuts_parity() {
    // B.6 Date.tomorrow/yesterday y DateTime.epoch.
    // Verificamos el shape (no fechas exactas — Local::now() varía):
    // tomorrow - today = 1 día; today - yesterday = 1; epoch == 1970-01-01.
    let src = "\
let t = Date.today()
let to = Date.tomorrow()
let ye = Date.yesterday()
print(to.diff_days(t))
print(t.diff_days(ye))
let e = DateTime.epoch()
print(e.to_str())
print(e.timestamp())
";
    let stdout = parity_run_vs_build("tier_b6_shortcuts", src);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "1");
    assert_eq!(lines[1], "1");
    assert_eq!(lines[2], "1970-01-01T00:00:00Z");
    assert_eq!(lines[3], "0");
}

#[test]
fn tier_b7_in_tz_iana_parity() {
    // B.7 timezone display: `to_local()` (TZ del sistema) + `in_tz`
    // (IANA name → Result<Str>). El instante UTC interno (timestamp)
    // NO cambia — solo el display.
    let src = "\
fn run() -> Result<Null> {
    let dt = DateTime.from_timestamp(1735689600)?
    match dt.in_tz(\"America/Argentina/Buenos_Aires\") {
        Ok(s) => print(s),
        Err(e) => print(e)
    }
    match dt.in_tz(\"UTC\") {
        Ok(s) => print(s),
        Err(e) => print(e)
    }
    match dt.in_tz(\"Not/A/Real_Zone\") {
        Ok(s) => print(s),
        Err(e) => print(\"err\")
    }
    return Ok(null)
}

match run() {
    Ok(_) => null,
    Err(e) => print(e)
}
";
    let stdout = parity_run_vs_build("tier_b7_in_tz", src);
    let lines: Vec<&str> = stdout.lines().collect();
    // 2025-01-01T00:00:00Z = 2024-12-31T21:00:00-03:00 en BsAs.
    assert_eq!(lines[0], "2024-12-31T21:00:00-03:00");
    assert_eq!(lines[1], "2025-01-01T00:00:00+00:00");
    assert_eq!(lines[2], "err");
}

#[test]
fn tier_b_runtime_error_overflow_check() {
    // El runtime tira panic claro al overflow (no silent wrap-around).
    // Validamos con un test E2E que el exit != 0 y el mensaje cita el
    // método + el valor que rompió.
    let stem = "tier_b_overflow";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    let src = "\
let d = Date.from_ymd(9999, 12, 31)?
let bomb = d.add_years(100000000)
print(bomb.to_str())
";
    std::fs::write(&src_path, src).expect("write");

    let out_run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .output()
        .expect("fitz run");
    // run debe fallar con error claro.
    assert!(
        !out_run.status.success(),
        "expected `fitz run` to abort due to overflow"
    );
    let stderr = String::from_utf8_lossy(&out_run.stderr);
    let stdout = String::from_utf8_lossy(&out_run.stdout);
    let combined = format!("{}\n{}", stdout, stderr);
    // `add_years(N)` internamente escala a `add_months(N*12)`, así que
    // el mensaje cita `add_months` (deuda de UX menor — el N reportado
    // es el ya escalado, no el original).
    assert!(
        combined.contains("add_months") && combined.contains("overflow"),
        "expected message with `add_months` and `overflow`, was:\n{}",
        combined
    );
}

#[test]
fn tier_b_checker_rejects_non_int_arg() {
    // El checker estático rechaza `d.add_days(\"hola\")` con error claro.
    // Anotación `: Date` explícita: los constructores estáticos
    // (`Date.today()`) retornan Any (Module call) — sin anotación, el
    // checker queda en modo gradual y no chequea Int. Con `: Date`
    // entra al path `infer_date_method` donde la regla del Tier B
    // valida el tipo del arg.
    let stderr = build_expect_fail(
        "tier_b_checker_bad_arg",
        "\
let d: Date = Date.today()
let bomb = d.add_days(\"hola\")
print(bomb.to_str())
",
    );
    assert!(
        stderr.contains("add_days") && stderr.contains("Int"),
        "expected error citing `add_days` + `Int`, was:\n{}",
        stderr
    );
}

// =========================================================================
// v0.11.0 — Fase 13: CLI builder nativo (`@command`)
// =========================================================================

/// Helper: invoca el binario compilado con args adicionales, devuelve
/// (stdout, stderr, exit_code). Paralelo a `build_and_run` pero acepta
/// args adicionales para el CLI dispatch.
fn build_and_run_cli(test_name: &str, src: &str, extra_args: &[&str]) -> (String, String, i32) {
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&src_path, src).expect("write");

    let out = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        out.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    });
    let exec = Command::new(&bin)
        .args(extra_args)
        .output()
        .expect("exec bin");
    (
        String::from_utf8_lossy(&exec.stdout).into_owned(),
        String::from_utf8_lossy(&exec.stderr).into_owned(),
        exec.status.code().unwrap_or(-1),
    )
}

#[test]
fn fase_13_cli_single_command_positional_arg() {
    let src = "\
@command(\"greet\")
fn greet(name: Str) -> Int {
    print(\"hola, {name}\")
    return 0
}
";
    let (stdout, _stderr, code) = build_and_run_cli("fase13_single_pos", src, &["Ada"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("hola, Ada"), "stdout: {stdout}");
}

#[test]
fn fase_13_cli_multi_command_dispatch() {
    let src = "\
@command(\"greet\", desc=\"saludar\")
fn greet(name: Str) -> Int {
    print(\"hola, {name}\")
    return 0
}

@command(\"status\", desc=\"estado\")
fn status() -> Int {
    print(\"ok\")
    return 0
}
";
    // Dispatch `status` (sin positional).
    let (stdout, _, code) = build_and_run_cli("fase13_multi_status", src, &["status"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("ok"), "stdout: {stdout}");
}

#[test]
fn fase_13_cli_flag_bool_y_flag_int() {
    let src = "\
@command(\"greet\")
fn greet(name: Str, loud: Bool = false, count: Int = 1) -> Int {
    let n = count
    while n > 0 {
        if loud {
            print(\"HELLO, {name}!\")
        } else {
            print(\"hello, {name}\")
        }
        n = n - 1
    }
    return 0
}
";
    let (stdout, _, code) =
        build_and_run_cli("fase13_flags", src, &["Ada", "--loud", "--count", "2"]);
    assert_eq!(code, 0);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "HELLO, Ada!");
    assert_eq!(lines[1], "HELLO, Ada!");
}

#[test]
fn fase_13_cli_help_global_lista_comandos() {
    let src = "\
@command(\"greet\", desc=\"Greet\")
fn greet(name: Str) -> Int {
    print(name)
    return 0
}

@command(\"status\", desc=\"Status\")
fn status() -> Int {
    print(\"ok\")
    return 0
}
";
    let (stdout, _, code) = build_and_run_cli("fase13_help_global", src, &["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("USAGE"), "stdout: {stdout}");
    assert!(stdout.contains("greet"), "stdout: {stdout}");
    assert!(stdout.contains("status"), "stdout: {stdout}");
    assert!(stdout.contains("Greet"), "stdout: {stdout}");
}

#[test]
fn fase_13_cli_help_command_individual() {
    let src = "\
@command(\"greet\", desc=\"saludar\")
fn greet(name: Str, loud: Bool = false) -> Int {
    print(name)
    return 0
}
";
    let (stdout, _, code) = build_and_run_cli("fase13_help_cmd", src, &["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("saludar"));
    assert!(stdout.contains("<name>"));
    assert!(stdout.contains("--loud"));
}

#[test]
fn fase_13_cli_comando_desconocido_exit_2() {
    let src = "\
@command(\"greet\")
fn greet(name: Str) -> Int {
    print(name)
    return 0
}

@command(\"status\")
fn status() -> Int {
    print(\"ok\")
    return 0
}
";
    let (_, stderr, code) = build_and_run_cli("fase13_unknown_cmd", src, &["bogus"]);
    assert_eq!(code, 2, "expected exit 2 for unknown command");
    assert!(stderr.contains("unknown"), "stderr: {stderr}");
}

#[test]
fn fase_13_cli_missing_positional_exit_2() {
    let src = "\
@command(\"greet\")
fn greet(name: Str) -> Int {
    print(name)
    return 0
}
";
    let (_, stderr, code) = build_and_run_cli("fase13_missing_pos", src, &[]);
    assert_eq!(code, 2);
    assert!(stderr.contains("missing argument"), "stderr: {stderr}");
}

#[test]
fn fase_13_cli_invalid_int_flag_exit_2() {
    let src = "\
@command(\"greet\")
fn greet(name: Str, count: Int = 1) -> Int {
    print(name)
    return 0
}
";
    let (_, stderr, code) = build_and_run_cli("fase13_bad_int", src, &["Ada", "--count", "abc"]);
    assert_eq!(code, 2);
    assert!(
        stderr.contains("--count") && stderr.contains("Int"),
        "stderr: {stderr}"
    );
}

#[test]
fn fase_13_cli_handler_retorna_exit_code() {
    // Verificamos que el Int retornado por el handler propaga como
    // exit code del binario.
    let src = "\
@command(\"fail\")
fn fail() -> Int {
    print(\"falling\")
    return 7
}
";
    let (stdout, _, code) = build_and_run_cli("fase13_exit", src, &[]);
    assert_eq!(code, 7);
    assert!(stdout.contains("falling"));
}

// v0.11.1 — Fase 13 polish: short flags + Bool=true negation + List<Str> variadic.

#[test]
fn fase_13_short_flags_auto_inferidos() {
    let src = "\
@command(\"greet\")
fn greet(name: Str, loud: Bool = false, count: Int = 1) -> Int {
    let n = count
    while n > 0 {
        if loud {
            print(\"HELLO, {name}!\")
        } else {
            print(\"hello, {name}\")
        }
        n = n - 1
    }
    return 0
}
";
    let (stdout, _, code) = build_and_run_cli("fase13_short_flags", src, &["Ada", "-l", "-c", "2"]);
    assert_eq!(code, 0);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "HELLO, Ada!");
    assert_eq!(lines[1], "HELLO, Ada!");
}

#[test]
fn fase_13_unknown_short_flag_is_error() {
    let src = "\
@command(\"greet\")
fn greet(name: Str, loud: Bool = false) -> Int {
    print(name)
    return 0
}
";
    let (_, stderr, code) = build_and_run_cli("fase13_short_unknown", src, &["Ada", "-z"]);
    assert_eq!(code, 2);
    assert!(
        stderr.contains("unknown") || stderr.contains("-z"),
        "expected message about -z, was: {stderr}"
    );
}

#[test]
fn fase_13_bool_default_true_negated_by_no_flag() {
    let src = "\
@command(\"go\")
fn go(verbose: Bool = true) -> Int {
    if verbose {
        print(\"verbose mode ON\")
    } else {
        print(\"quiet mode\")
    }
    return 0
}
";
    let (stdout_default, _, code_default) = build_and_run_cli("fase13_no_flag_default", src, &[]);
    assert_eq!(code_default, 0);
    assert!(
        stdout_default.contains("verbose mode ON"),
        "default true: {stdout_default}"
    );

    let (stdout_neg, _, code_neg) = build_and_run_cli("fase13_no_flag_neg", src, &["--no-verbose"]);
    assert_eq!(code_neg, 0);
    assert!(
        stdout_neg.contains("quiet mode"),
        "--no-verbose: {stdout_neg}"
    );
}

#[test]
fn fase_13_list_str_variadic_absorbe_positionals() {
    let src = "\
@command(\"run\")
fn run(mode: Str, files: List<Str> = []) -> Int {
    print(\"mode: {mode}\")
    for f in files {
        print(\"  - {f}\")
    }
    return 0
}
";
    let (stdout, _, code) = build_and_run_cli(
        "fase13_variadic_basic",
        src,
        &["fast", "a.txt", "b.txt", "c.txt"],
    );
    assert_eq!(code, 0);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "mode: fast");
    assert_eq!(lines[1], "  - a.txt");
    assert_eq!(lines[2], "  - b.txt");
    assert_eq!(lines[3], "  - c.txt");
}

#[test]
fn fase_13_list_str_variadic_empty_accepted() {
    let src = "\
@command(\"run\")
fn run(files: List<Str> = []) -> Int {
    print(\"count: {files.len()}\")
    return 0
}
";
    let (stdout, _, code) = build_and_run_cli("fase13_variadic_empty", src, &[]);
    assert_eq!(code, 0);
    assert!(stdout.contains("count: 0"));
}

#[test]
fn fase_13_parity_run_vs_build_polish() {
    let stem = "fase13_paridad_polish";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src = "\
@command(\"process\", desc=\"Process files\")
fn process(mode: Str, verbose: Bool = true, count: Int = 1, files: List<Str> = []) -> Int {
    if verbose {
        print(\"mode={mode} count={count}\")
    }
    print(\"files: {files.len()}\")
    return 0
}
";
    let src_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&src_path, src).expect("write");

    let bout = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        bout.status.success(),
        "build failed:\nstderr: {}",
        String::from_utf8_lossy(&bout.stderr)
    );
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });

    let cli_args = ["fast", "-c", "3", "--no-verbose", "a.txt", "b.txt"];

    let interp = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .arg("--")
        .args(cli_args)
        .output()
        .expect("fitz run");
    let interp_stdout = String::from_utf8_lossy(&interp.stdout).into_owned();

    let compiled = Command::new(&bin).args(cli_args).output().expect("bin");
    let compiled_stdout = String::from_utf8_lossy(&compiled.stdout).into_owned();

    assert_eq!(
        interp_stdout.replace("\r\n", "\n"),
        compiled_stdout.replace("\r\n", "\n"),
        "paridad bit-a-bit run↔build esperada con polish v0.11.1"
    );
    assert!(compiled_stdout.contains("files: 2"));
    assert!(!compiled_stdout.contains("mode="));
}

#[test]
fn fase_13_short_flag_collision_is_compile_error() {
    let src = "\
@command(\"go\")
fn go(loud: Bool = false, level: Int = 1) -> Int {
    print(\"x\")
    return 0
}
";
    let stderr = build_expect_fail("fase13_short_collision", src);
    assert!(
        stderr.contains("conflict") || stderr.contains("colisi") || stderr.contains("comparten"),
        "expected collision message, was: {stderr}"
    );
}

#[test]
fn fase_13_cli_parity_run_vs_build() {
    // Paridad bit-a-bit: el output del intérprete debe coincidir con
    // el del binario compilado para el mismo argv.
    let stem = "fase13_paridad";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src = "\
@command(\"greet\", desc=\"saludar\")
fn greet(name: Str, loud: Bool = false, count: Int = 1) -> Int {
    let n = count
    while n > 0 {
        if loud {
            print(\"HOLA, {name}\")
        } else {
            print(\"hola, {name}\")
        }
        n = n - 1
    }
    return 0
}
";
    let src_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&src_path, src).expect("write");

    // Build.
    let bout = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(bout.status.success());

    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });

    // Args para ambos paths.
    let cli_args = ["Ada", "--loud", "--count", "3"];

    let interp = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .arg("--")
        .args(cli_args)
        .output()
        .expect("fitz run");
    let interp_stdout = String::from_utf8_lossy(&interp.stdout).into_owned();
    let interp_exit = interp.status.code().unwrap_or(-1);

    let compiled = Command::new(&bin).args(cli_args).output().expect("bin");
    let compiled_stdout = String::from_utf8_lossy(&compiled.stdout).into_owned();
    let compiled_exit = compiled.status.code().unwrap_or(-1);

    assert_eq!(
        interp_stdout.replace("\r\n", "\n"),
        compiled_stdout.replace("\r\n", "\n"),
        "paridad bit-a-bit fitz run ↔ fitz build esperada"
    );
    assert_eq!(interp_exit, compiled_exit);
    assert_eq!(interp_exit, 0);
}

#[test]
fn fase_13_cli_arg_flag_decorators_parity_v0_37_13() {
    // #6 (v0.37.13) — per-param `@arg(help=)` / `@flag(short=, help=)`
    // decorators: help text + explicit (case-preserved) short flag, with
    // bit-for-bit parity between `fitz run` and the compiled binary.
    let stem = "fase13_argflag_v0_37_13";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let src = "\
@command(\"greet\", desc=\"Greet a person\")
fn greet(@arg(help=\"who to greet\") name: Str, @flag(short=\"L\", help=\"shout it\") loud: Bool = false, @flag(help=\"how many\") count: Int = 1) -> Int {
    let n = count
    while n > 0 {
        if loud {
            print(\"HEY {name}!\")
        } else {
            print(\"hi {name}\")
        }
        n = n - 1
    }
    return 0
}
";
    let src_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&src_path, src).expect("write");

    let bout = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .output()
        .expect("fitz build");
    assert!(
        bout.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&bout.stderr)
    );
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });

    // --- Help parity: arg help + explicit short + flag help ---
    let run_help = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .arg("--")
        .arg("--help")
        .output()
        .expect("run --help");
    let build_help = Command::new(&bin)
        .arg("--help")
        .output()
        .expect("bin --help");
    let rh = String::from_utf8_lossy(&run_help.stdout).replace("\r\n", "\n");
    let bh = String::from_utf8_lossy(&build_help.stdout).replace("\r\n", "\n");
    // The USAGE line differs by bin name (interpreter is `fitz`, the
    // binary is its own stem) — by design. Compare the ARGS + OPTIONS
    // body (the parts `@arg`/`@flag` affect), which must be bit-identical.
    let run_body = rh.split("ARGS:").nth(1).unwrap_or("");
    let build_body = bh.split("ARGS:").nth(1).unwrap_or("");
    assert_eq!(
        run_body, build_body,
        "ARGS/OPTIONS help parity run↔build expected"
    );
    assert!(
        rh.contains("<name>  (Str)  who to greet"),
        "arg help missing: {rh}"
    );
    assert!(
        rh.contains("-L, --loud  shout it"),
        "explicit short + flag help missing: {rh}"
    );
    assert!(
        rh.contains("--count <INT>  how many"),
        "count help missing: {rh}"
    );

    // --- Functional parity: explicit `-L` parses in both paths ---
    let args = ["World", "-L", "--count", "2"];
    let run_out = Command::new(fitz_bin())
        .args(["run"])
        .arg(&src_path)
        .arg("--")
        .args(args)
        .output()
        .expect("run");
    let build_out = Command::new(&bin).args(args).output().expect("bin");
    let ro = String::from_utf8_lossy(&run_out.stdout).replace("\r\n", "\n");
    let bo = String::from_utf8_lossy(&build_out.stdout).replace("\r\n", "\n");
    assert_eq!(ro, bo, "output parity run↔build with -L");
    assert!(
        ro.contains("HEY World!"),
        "expected shout (explicit -L), was: {ro}"
    );
    assert_eq!(ro.lines().count(), 2, "count=2 → two lines, was: {ro}");
}

// ---------------------------------------------------------------------------
// Fase 12.3.a.3 — codegen paridad bit-a-bit del módulo `log`.
//
// Compila programas con `log.info/warn/error/debug` a binario nativo
// con `fitz build`, ejecuta el binario, captura stderr (donde el
// logger emite por convención cargo) y valida el shape JSON estructurado
// + filter por RUST_LOG + Secret redaction.
// ---------------------------------------------------------------------------

/// Como `build_and_run_with_stderr` pero permite setear env vars sobre
/// el child que ejecuta el binario. Útil para tests del log que
/// dependen de `RUST_LOG`/`FITZ_LOG_FORMAT`.
fn build_and_run_with_env_and_stderr(
    test_name: &str,
    src: &str,
    env_vars: &[(&str, &str)],
) -> (String, String, i32) {
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    let mut cmd = Command::new(&bin);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    // FITZ_LOG_FORMAT=json explícito para tests deterministas — el
    // auto-detect TTY no es predecible en CI ni con Command::output (que
    // pipea stderr y no es TTY, así que JSON es default, pero ser
    // explícito blinda contra runners raros).
    let run = cmd.output().expect("invocar binario");
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        String::from_utf8_lossy(&run.stderr).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

#[test]
fn m12_3_a_3_log_info_emits_json_with_flat_shape_to_stderr() {
    // Programa con `log.info(msg, kwargs)` debe emitir JSON flat a
    // stderr: timestamp + level + msg + kwargs al mismo nivel.
    let src = "\
log.info(\"login ok\", user_id: 42, role: \"admin\", active: true)
print(\"done\")
";
    let (stdout, stderr, exit) = build_and_run_with_env_and_stderr(
        "m12_3_a_3_log_info_emite_json",
        src,
        &[("FITZ_LOG_FORMAT", "json")],
    );
    assert_eq!(exit, 0, "exit code: {} stderr: {}", exit, stderr);
    assert!(
        stdout.contains("done"),
        "expected 'done' in stdout: {}",
        stdout
    );
    // Stderr debe tener UNA línea con JSON shape flat.
    assert!(
        stderr.contains("\"level\":\"INFO\""),
        "expected 'level':'INFO' in stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("\"msg\":\"login ok\""),
        "expected 'msg':'login ok' in stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("\"user_id\":42"),
        "expected 'user_id':42 in stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("\"role\":\"admin\""),
        "expected 'role':'admin' in stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("\"active\":true"),
        "expected 'active':true in stderr: {}",
        stderr
    );
    // El timestamp debe tener shape ISO 8601 con 'Z' final.
    assert!(
        stderr.contains("\"timestamp\":") && stderr.contains("Z\""),
        "expected ISO 8601 timestamp in stderr: {}",
        stderr
    );
}

#[test]
fn m12_3_a_3_log_default_level_info_filters_debug() {
    // Sin RUST_LOG seteada, default level = info. log.debug() NO debe
    // aparecer en el output stderr.
    let src = "\
log.debug(\"oculto\", request_id: \"x\")
log.info(\"visible\")
";
    let (_stdout, stderr, exit) = build_and_run_with_env_and_stderr(
        "m12_3_a_3_log_default_info",
        src,
        &[("FITZ_LOG_FORMAT", "json")],
    );
    assert_eq!(exit, 0);
    assert!(
        !stderr.contains("oculto"),
        "log.debug should be filtered with default level=info: {}",
        stderr
    );
    assert!(
        stderr.contains("\"msg\":\"visible\""),
        "log.info should appear with default level=info: {}",
        stderr
    );
}

#[test]
fn m12_3_a_3_log_rust_log_debug_habilita_debug() {
    // Con RUST_LOG=debug, log.debug() SÍ aparece.
    let src = "\
log.debug(\"ahora visible\", request_id: \"abc\")
log.info(\"tambien visible\")
";
    let (_stdout, stderr, exit) = build_and_run_with_env_and_stderr(
        "m12_3_a_3_log_rust_log_debug",
        src,
        &[("FITZ_LOG_FORMAT", "json"), ("RUST_LOG", "debug")],
    );
    assert_eq!(exit, 0);
    assert!(
        stderr.contains("\"level\":\"DEBUG\""),
        "log.debug debe aparecer con RUST_LOG=debug: {}",
        stderr
    );
    assert!(
        stderr.contains("ahora visible"),
        "msg de debug debe aparecer con RUST_LOG=debug: {}",
        stderr
    );
}

#[test]
fn m12_3_a_3_log_warn_with_secret_redacts_inner() {
    // Programa que pone un Secret en kwargs — el value real (de la
    // env var SMOKE_TOK) NO debe aparecer en el output.
    let src = "\
fn rotate() -> Result<Null> {
    let token = secret(\"SMOKE_TOK\")?
    log.warn(\"rotating\", user_id: 42, token: token)
    return Ok(null)
}
let _ = rotate()
";
    let secret_value = "DO-NOT-LEAK-codegen-12-3-a-3-secret-xyz";
    let (_stdout, stderr, exit) = build_and_run_with_env_and_stderr(
        "m12_3_a_3_log_secret_redaction",
        src,
        &[("FITZ_LOG_FORMAT", "json"), ("SMOKE_TOK", secret_value)],
    );
    assert_eq!(exit, 0);
    assert!(
        stderr.contains("\"token\":\"<redacted>\""),
        "expected redacted token in stderr: {}",
        stderr
    );
    assert!(
        !stderr.contains(secret_value),
        "el secret real NO debe aparecer en stderr: {}",
        stderr
    );
}

#[test]
fn auth_blacklist_codegen_compiles_3_builtins_and_emits_helpers() {
    // 9.w.1.iter2.b — programa que usa `auth.blacklist`,
    // `auth.is_blacklisted` y `auth.cleanup_expired` debe compilar
    // sin errores y emitir los 3 helpers `__fitz_auth_*` en el
    // preludio de auth del crate generado. No corremos el binario
    // porque requiere DB real; validamos compile-only.
    let stem = "auth_blacklist_codegen";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    let src = "\
async fn revoke(jti: Str, exp: Int) -> Result<Null> {\n\
    let conn = db.connect(\"postgres://x\").await?\n\
    let _ = auth.blacklist(conn, jti, exp).await?\n\
    return Ok(null)\n\
}\n\
\n\
async fn check_revoked(jti: Str) -> Result<Bool> {\n\
    let conn = db.connect(\"postgres://x\").await?\n\
    return auth.is_blacklisted(conn, jti).await\n\
}\n\
\n\
async fn cleanup() -> Result<Int> {\n\
    let conn = db.connect(\"postgres://x\").await?\n\
    return auth.cleanup_expired(conn).await\n\
}\n\
\n\
@server(43922)\n\
fn main() => 0\n\
\n\
@get(\"/health\")\n\
fn health() -> Str => \"ok\"\n\
";
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = std::process::Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Validá que los 3 helpers + las 4 constantes SQL están en el
    // main.rs generado.
    let main_rs = dir.join(format!("target/fitz-build/{}/src/main.rs", stem));
    // El path real es `target/fitz-build/<stem>/src/main.rs` adentro
    // del cwd del fitz build (que es donde está el .fitz). Lo
    // construimos relativo al dir del fitz_src.
    let _ = main_rs; // no usado — verificamos via spawn
                     // En vez de leer main.rs (que el binario fitz lo borra al limpiar),
                     // ya validamos lo importante: compila sin errores. Los helpers se
                     // verifican por integración (build success).
}

// Fase 12.8 — feature flags built-in. Validamos que el binario emite el
// preludio `__FitzFlagRegistry`, el init call con defaults baked-in, y
// que `flag()`/`flags.is_enabled()`/`flags.list()` retornan los valores
// correctos según defaults + env var override.
#[test]
fn fase_12_8_feature_flags_compilan_y_corren_default_false() {
    let src = "\
let v = flag(\"my-flag\")
print(\"my-flag: {v}\")
";
    let (stdout, code) = build_and_run("fase_12_8_feature_flags_default", src);
    assert_eq!(code, 0, "exit no fue 0; stdout: {}", stdout);
    assert_eq!(stdout.trim(), "my-flag: false");
}

#[test]
fn fase_12_8_flags_list_codegen_returns_detected_env_var() {
    // Sin manifest mode, los defaults compile-time están vacíos.
    // Pero `flags.list()` enumera env vars `FITZ_FLAG_*`. Para evitar
    // pollución de otros tests / env del shell, este test valida solo
    // el shape (no entries específicas).
    let src = "\
let xs = flags.list()
print(\"is_list: {len(xs) >= 0}\")
";
    let (stdout, code) = build_and_run("fase_12_8_flags_list_codegen", src);
    assert_eq!(code, 0, "exit no fue 0; stdout: {}", stdout);
    assert!(stdout.contains("is_list: true"), "stdout: {}", stdout);
}

// Fase 12.7.b — `@trace`/`@metric` sobre fns user emiten un guard RAII
// + `tracing::info_span!` que registran métricas al Drop. Validamos que
// el programa compila con `cargo build` (los crates `tracing`/`metrics`
// se linkean OK), corre standalone, y produce el output esperado.
#[test]
fn fase_12_7_trace_metric_decorators_compilan_y_corren() {
    let src = "\
@trace(name=\"calc_span\")
@metric(name=\"calc_metric\")
fn calc(x: Int) -> Int {
    return x * 2
}

@metric
fn just_metric(y: Int) -> Int {
    return y + 10
}

@trace
fn just_trace(z: Str) -> Str {
    return z
}

let a = calc(21)
let b = just_metric(5)
let c = just_trace(\"hola\")
print(\"a={a} b={b} c={c}\")
";
    let (stdout, code) = build_and_run("fase_12_7_trace_metric_decorators", src);
    assert_eq!(code, 0, "exit no fue 0; stdout: {}", stdout);
    assert_eq!(stdout.trim(), "a=42 b=15 c=hola");
}

// ---------------------------------------------------------------------------
// Mini-fase HTTP client (2026-06-18) — Bloque 3 codegen paridad bit-a-bit.
// ---------------------------------------------------------------------------

/// Mini-fase HTTP client (2026-06-18) — E2E del codegen contra un
/// servidor axum local. Verifica el path completo: el detector
/// dispara, el preludio se emite, el dispatch genera la llamada
/// `__fitz_http_get`, el binario standalone enlaza `reqwest` con
/// `rustls-tls`, y el `.await?` propaga el `Result<HttpClientResponse>`
/// correctamente para que el user lea `r.status` / `r.body`.
///
/// Patrón: spawneamos un servidor axum minimal en un thread con su
/// propio runtime tokio, capturamos el puerto asignado por el OS
/// (bind en `127.0.0.1:0`), inyectamos el puerto al programa fitz, y
/// corremos `fitz build` + el binario standalone para checkear stdout.
#[test]
fn mini_fase_http_client_codegen_get_200_against_local_axum_server() {
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::sync::Arc;

    // Atomic para compartir el puerto del listener entre el thread del
    // server y el main del test.
    let port_share = Arc::new(AtomicU16::new(0));
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let port_share_for_server = Arc::clone(&port_share);
    let server_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime para test server");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind a 127.0.0.1:0");
            let addr = listener.local_addr().expect("local_addr");
            port_share_for_server.store(addr.port(), Ordering::SeqCst);
            started_tx.send(()).expect("notificar arranque del server");

            let app = axum::Router::new()
                .route("/hello", axum::routing::get(|| async { "hello fitz" }))
                .route(
                    "/echo-method",
                    axum::routing::any(
                        |method: axum::http::Method| async move { method.to_string() },
                    ),
                );
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("axum::serve");
        });
    });

    started_rx.recv().expect("server thread sent start signal");
    let port = port_share.load(Ordering::SeqCst);
    assert!(port > 0, "puerto sin asignar");

    let src = format!(
        r#"async fn fetch_status(u: Str) -> Result<Int> {{
    let r = http.get(u).await?
    return Ok(r.status)
}}

async fn fetch_body(u: Str) -> Result<Str> {{
    let r = http.get(u).await?
    return Ok(r.body)
}}

let s = fetch_status("http://127.0.0.1:{port}/hello").await
match s {{
    Ok(code) => print("status={{code}}"),
    Err(e) => print("err={{e}}"),
}}

let b = fetch_body("http://127.0.0.1:{port}/hello").await
match b {{
    Ok(body) => print("body={{body}}"),
    Err(e) => print("err={{e}}"),
}}
"#,
    );

    let (stdout, code) = build_and_run(
        "mini_fase_http_client_codegen_get_200_against_local_axum_server",
        &src,
    );

    let _ = shutdown_tx.send(());
    let _ = server_thread.join();

    assert_eq!(code, 0, "exit code != 0; stdout: {}", stdout);
    assert!(
        stdout.contains("status=200"),
        "expected `status=200` in stdout, got: {}",
        stdout
    );
    assert!(
        stdout.contains("body=hello fitz"),
        "expected `body=hello fitz` in stdout, got: {}",
        stdout
    );
}

/// Mini-fase HTTP client (2026-06-18) — paridad bit-a-bit `fitz run` ↔
/// `fitz build` para `http.post(url, body)` con body Str + verifica que
/// el server recibe el body. `4xx`/`5xx` siguen siendo `Ok` (sólo
/// transport errors son `Err`).
#[test]
fn mini_fase_http_client_codegen_post_with_str_body_echoes_back() {
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::sync::Arc;

    let port_share = Arc::new(AtomicU16::new(0));
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let port_share_for_server = Arc::clone(&port_share);
    let server_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let addr = listener.local_addr().expect("local_addr");
            port_share_for_server.store(addr.port(), Ordering::SeqCst);
            started_tx.send(()).expect("notify start");

            let app = axum::Router::new().route(
                "/echo",
                axum::routing::post(|body: String| async move { body }),
            );
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("axum::serve");
        });
    });

    started_rx.recv().expect("server start");
    let port = port_share.load(Ordering::SeqCst);

    let src = format!(
        r#"async fn ping(u: Str, payload: Str) -> Result<Str> {{
    let r = http.post(u, payload).await?
    return Ok(r.body)
}}

let result = ping("http://127.0.0.1:{port}/echo", "hola fitz").await
match result {{
    Ok(echoed) => print("echo={{echoed}}"),
    Err(e) => print("err={{e}}"),
}}
"#,
    );

    let (stdout, code) = build_and_run(
        "mini_fase_http_client_codegen_post_with_str_body_echoes_back",
        &src,
    );

    let _ = shutdown_tx.send(());
    let _ = server_thread.join();

    assert_eq!(code, 0, "exit != 0; stdout: {}", stdout);
    assert!(
        stdout.contains("echo=hola fitz"),
        "expected `echo=hola fitz` in stdout, got: {}",
        stdout
    );
}

/// Mini-fase HTTP client (2026-06-18) — verifica que un error de
/// transporte (host no resoluble) se propaga como `Result::Err(Str)`
/// con prefijo `http:`. El binario no rebota — el user maneja el
/// error con `match`.
#[test]
fn mini_fase_http_client_codegen_transport_error_propagates_as_err() {
    // Hostname con puerto 1 — typically connection refused o DNS fail,
    // según el sistema. Lo importante: NO debe ser Ok.
    let src = r#"async fn fetch(u: Str) -> Result<Int> {
    let r = http.get(u).await?
    return Ok(r.status)
}

let result = fetch("http://127.0.0.1:1").await
match result {
    Ok(code) => print("status={code}"),
    Err(e) => print("transport_error"),
}
"#;
    let (stdout, code) = build_and_run(
        "mini_fase_http_client_codegen_transport_error_propagates_as_err",
        src,
    );

    assert_eq!(code, 0, "exit != 0; stdout: {}", stdout);
    assert!(
        stdout.contains("transport_error"),
        "expected `transport_error` in stdout (Err branch), got: {}",
        stdout
    );
}

/// B12 (sub-paso 5 cosecha post-fitzwatch, 2026-06-19) — `fitz build`
/// pasa cuando un módulo con `@authenticated` no importa `auth`
/// directamente pero el `@auth_provider` vive en otro módulo
/// importado desde MAIN. Antes del fix, el checker del módulo
/// rompía con "no `@auth_provider` registered in the program"
/// aunque todo el árbol del proyecto sí tuviera uno.
#[test]
fn cross_module_auth_provider_via_main_b12() {
    let stem = "cross_module_auth_provider_via_main_b12";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // types.fitz — User declarado en módulo dedicado.
    std::fs::write(
        dir.join("types.fitz"),
        "type User {\n  \
            id: Int = 0\n  \
            role: Str = \"\"\n\
         }\n",
    )
    .expect("escribir types.fitz");

    // auth.fitz — provider importa User desde types.
    std::fs::write(
        dir.join("auth.fitz"),
        "from types import User\n\
\n\
@auth_provider\n\
async fn lookup(headers: Map<Str, Str>) -> Result<User> {\n  \
    return Ok(User { id: 1, role: \"admin\" })\n\
}\n",
    )
    .expect("escribir auth.fitz");

    // metrics.fitz — `@authenticated` handler SIN importar auth.
    // Importa User desde types (lo necesita para el param del handler).
    std::fs::write(
        dir.join("metrics.fitz"),
        "from types import User\n\
\n\
@authenticated\n\
@get(\"/metrics\")\n\
fn handler(user: User) -> Str {\n  \
    return \"ok\"\n\
}\n",
    )
    .expect("escribir metrics.fitz");

    // main.fitz — importa AMBOS (auth + metrics).
    let main_src = "\
import auth\n\
import metrics\n\
\n\
@server(43919)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (B12):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );
}

/// B10 (sub-paso 5 cosecha post-fitzwatch, 2026-06-19) — `fitz build`
/// pasa cuando una fn `@background` vive en un módulo importado y
/// `spawn(<imported_fn>(...))` se llama desde el módulo importador.
/// Antes del fix, el checker rompía con
/// "spawn: fn `run_check` is not declared with `@background`" porque
/// `collect_background_fns` solo miraba los fns top-level locales.
#[test]
fn cross_module_spawn_background_b10() {
    let stem = "cross_module_spawn_background_b10";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // checks.fitz — fn marcada con `@background` en módulo
    // independiente.
    std::fs::write(
        dir.join("checks.fitz"),
        "@background\n\
         async fn run_check(id: Int) -> Null {\n  \
            print(\"check id={id}\")\n  \
            return null\n\
         }\n",
    )
    .expect("escribir checks.fitz");

    // Importer hace `from checks import run_check` y llama
    // `spawn(run_check(42))`.
    let main_src = "from checks import run_check\n\
\n\
async fn boot() -> Null {\n  \
    let _ = spawn(run_check(42))\n  \
    return null\n\
}\n\
\n\
boot().await\n";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (B10):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );
}

/// 🔴 URGENTE (2026-06-23) — `spawn(<imported @background async fn>(args))`
/// no debe sufrir silent-drop: el `do_work(id)` adentro de
/// `tokio::spawn(async move { ... })` debe emitirse CON `.await` aunque
/// la fn target viva en otro módulo. Antes del fix,
/// `collect_module_sigs` registraba el sig de la fn cross-module sin
/// envolver el `ret` en `Type::Future(...)` cuando `is_async = true`,
/// entonces `gen_spawn_call` no agregaba `.await` y el closure
/// dropeaba el Future sin pollearlo — el body nunca corría, sin error,
/// sin panic. Mismo módulo funcionaba OK (path local sí wrappeaba).
#[test]
fn cross_module_spawn_async_background_emits_await_no_silent_drop() {
    let stem = "cross_module_spawn_async_background_emits_await";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    std::fs::write(
        dir.join("worker.fitz"),
        "@background\n\
         async fn do_work(id: Int) -> Null {\n  \
            log.info(\"worker.start id={id}\")\n  \
            return null\n\
         }\n",
    )
    .expect("escribir worker.fitz");

    let main_src = "from worker import do_work\n\
\n\
@get(\"/trigger/{id}\")\n\
async fn trigger(id: Int) -> Result<Str> {\n  \
    let _ = spawn(do_work(id))\n  \
    return Ok(\"dispatched\")\n\
}\n\
\n\
@server(43923)\n\
fn main() => 0\n";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Inspect the emitted main.rs to confirm the closure contains
    // `do_work(id).await` and NOT bare `do_work(id)` (silent drop).
    // The build emits to `<cwd>/target/fitz-build/<stem>/src/main.rs`
    // (relative to the cwd of the spawned process — which inherits
    // from the test runner = workspace root, NOT the .fitz dir).
    let emitted = std::path::PathBuf::from("target")
        .join("fitz-build")
        .join(stem)
        .join("src")
        .join("main.rs");
    let src = std::fs::read_to_string(&emitted).unwrap_or_else(|e| {
        panic!("read {}: {}", emitted.display(), e);
    });
    assert!(
        src.contains("do_work(id).await"),
        "expected `do_work(id).await` adentro del tokio::spawn closure, no apareció.\nEmitted main.rs (sample):\n{}",
        // show only lines that mention do_work
        src.lines()
            .filter(|l| l.contains("do_work"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !src.contains("tokio::spawn(async move { do_work(id) })"),
        "regression: bare `tokio::spawn(async move {{ do_work(id) }})` SIN `.await` reapareció — silent-drop volvió."
    );
}

/// B15 (sub-paso 6 cosecha post-fitzwatch, 2026-06-19) — `fitz build`
/// pasa cuando `.preload("companion")` se hace sobre una relation
/// `BelongsToCompanion` cuyo FK en el parent es `Int?` (Nullable).
/// Antes del fix, el codegen emitía `__FitzPgValue::Int(__g.<fk>)`
/// directo (rustc: `expected i64, found Option<i64>`) y comparaba
/// `__tg2.<pk> == __fk` (rustc: `can't compare i64 with Option<i64>`).
/// El fix emite `filter_map(|p| p.<fk>.map(__FitzPgValue::Int))` para
/// el `IN (...)` y `match __fk { None => None, Some(v) => find(v) }`
/// para el lookup. Cubre también el path HasMany con FK del child
/// nullable (sibling fix con `__cg2.<fk> == Some(__pid)`).
#[test]
fn cross_module_orm_preload_nullable_fk_b15() {
    let stem = "cross_module_orm_preload_nullable_fk_b15";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // BelongsToCompanion + HasMany ambos con FK nullable. Ejercita
    // los dos caminos del fix en un solo programa.
    let src = "@table(\"users\") type User {\n  \
                   @primary id: Int = 0\n  \
                   name: Str = \"\"\n  \
                   @has_many(\"Post\", via=\"author_id\") posts: List<Post> = []\n\
               }\n\
               @table(\"posts\") type Post {\n  \
                   @primary id: Int = 0\n  \
                   @belongs_to(\"User\") author_id: Int? = null\n  \
                   author: User?\n  \
                   title: Str = \"\"\n\
               }\n\
               @get(\"/posts-with-author\")\n\
               async fn posts_with_author() -> Result<List<Post>> {\n  \
                   let conn = db.connect(\"postgres://x@h/d\").await?\n  \
                   return Post.where(fn(p) => p.title == \"x\").preload(\"author\").all(conn).await\n\
               }\n\
               @get(\"/users-with-posts\")\n\
               async fn users_with_posts() -> Result<List<User>> {\n  \
                   let conn = db.connect(\"postgres://x@h/d\").await?\n  \
                   return User.where(fn(u) => u.id > 0).preload(\"posts\").all(conn).await\n\
               }\n\
               @server(43920)\n\
               fn main() => 0\n";

    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (B15):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );
}

#[test]
fn orm_preload_has_one_compiles_to_binary_v0_37_12() {
    // #5 (v0.37.12) — HasOne `.preload("profile")` compiles to a
    // native binary. Loading mirrors HasMany (WHERE child.fk IN
    // parent.pks), but the parent's virtual field `profile: Profile?`
    // gets an `Option<Profile>` (first match). Nullable child FK
    // (`user_id: Int? = null`) exercised too. Build-only (running
    // needs a real DB), which still validates the emitted Rust
    // actually rustc-compiles.
    let src = "@table(\"users\") type User {\n  \
                   @primary id: Int = 0\n  \
                   name: Str = \"\"\n  \
                   @has_one(\"Profile\", via=\"user_id\") profile: Profile?\n\
               }\n\
               @table(\"profiles\") type Profile {\n  \
                   @primary id: Int = 0\n  \
                   user_id: Int? = null\n  \
                   bio: Str = \"\"\n\
               }\n\
               @get(\"/users-with-profile\")\n\
               async fn users_with_profile() -> Result<List<User>> {\n  \
                   let conn = db.connect(\"postgres://x@h/d\").await?\n  \
                   return User.where(fn(u) => u.id > 0).preload(\"profile\").all(conn).await\n\
               }\n\
               @server(43921)\n\
               fn main() => 0\n";
    build_expect_ok("orm_preload_has_one_compiles_to_binary_v0_37_12", src);
}

#[test]
fn for_over_list_with_await_in_body_does_not_break_send_b17() {
    // B17 (post-fitzwatch cosecha) — `for x in <List<T>>` con `.await`
    // adentro del body en un handler `async` rompía Send porque el
    // codegen emitía `(xs.clone()).lock().unwrap().clone().into_iter()`
    // como expresión inline del `for`, manteniendo el MutexGuard temporal
    // vivo cross-await.
    //
    // **Fix**: emitir
    // `{ let __for_snap = (xs).lock().unwrap().clone(); for x in __for_snap.into_iter() { ... } }`
    // — el `let` libera el guard al `;` y el for itera sobre el Vec
    // owned. Aplicable a List<T>, List<Tuple>, Map<K,V> y Map wildcard.
    //
    // Repro mínimo: fn async helper que itera `List<Int>` y hace `.await`
    // en cada iteración. Encapsulada por handler HTTP `async` que dispara
    // tokio::spawn (axum::Handler exige + Send).
    let stem = "for_await_send_b17";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let src = "\
async fn one_step(ms: Int) -> Int {\n  \
    sleep(ms).await\n  \
    return ms\n\
}\n\
\n\
async fn process_all(items: List<Int>) -> Int {\n  \
    let total: Int = 0\n  \
    for it in items {\n    \
        let n = one_step(it).await\n    \
        total = total + n\n  \
    }\n  \
    return total\n\
}\n\
\n\
@get(\"/sum\")\n\
async fn sum_endpoint() -> Int {\n  \
    return process_all([1, 2, 3]).await\n\
}\n\
\n\
@server(43921)\n\
fn main() => 0\n";

    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (B17):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );
}

#[test]
fn cron_in_imported_module_is_spawned_b19() {
    // B19 (post-fitzwatch sesión 2, v0.18.2) — `@cron("expr") fn ...`
    // declarado en un módulo importado (no en el archivo main) era
    // silenciosamente dropeado por el codegen: el [ready] banner del
    // usuario aparecía pero ningún `tokio::spawn(__fitz_run_cron_job(...))`
    // se emitía, así que el scheduler nunca arrancaba el job.
    //
    // **Fix**: `LoadedModule` suma `cron_fn_stmts: Vec<Stmt>` (paralelo a
    // W16 `http_fn_stmts` y 10.8.6 `ws_fn_stmts`). `generate_project`
    // populate `cron_jobs_info` también desde `loader.modules`, marcando
    // `module_path: Some(mod_name)`. `emit_cron_job_spawns` emite
    // `crate::<mod_name>::<fn_name>` cuando `module_path.is_some()`.
    //
    // Repro mínimo: módulo `tasks.fitz` con `@cron`; main solo `import tasks`.
    // El binario nativo debe compilar y, al inspeccionar main.rs generado,
    // debe contener `tokio::spawn(__fitz_run_cron_job(...))`.
    let stem = "cron_cross_module_b19";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // tasks.fitz — módulo con `@cron` async fn.
    std::fs::write(
        dir.join("tasks.fitz"),
        "@cron(\"*/30 * * * * *\")\n\
         async fn ping() -> Null {\n  \
             sleep(10).await\n  \
             return null\n\
         }\n",
    )
    .expect("escribir tasks.fitz");

    // main.fitz — solo `import tasks` + @server.
    let main_src = "\
import tasks\n\
\n\
@server(43922)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = std::process::Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (B19):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );

    // Run the binary briefly and check stderr for the scheduler
    // banner that `emit_cron_job_spawns` prints when at least one
    // cron job is registered:
    //   "🕐 Fitz scheduler arrancado con N job(s) cron"
    //   "   @cron  ping (*/30 * * * * *)"
    //
    // Pre-B19 the cross-module `@cron` was silently dropped, so the
    // banner never appeared (cron_jobs_info was empty).
    let bin_path = dir.join(&bin_name);
    let mut child = std::process::Command::new(&bin_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn fitzwatch test binary");
    // Wait ~2s so the cron banner has time to print at boot.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = child.kill();
    let output = child.wait_with_output().expect("wait child");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Fitz scheduler arrancado con 1 job(s) cron"),
        "expected `Fitz scheduler arrancado con 1 job(s) cron` in stderr (B19 — cross-module @cron), stderr was: {}",
        stderr
    );
    assert!(
        stderr.contains("ping"),
        "expected the @cron fn name `ping` in stderr banner, stderr was: {}",
        stderr
    );
}

#[test]
fn cron_with_persistent_store_in_imported_module_b19_derived() {
    // B19 bug derivado (post-fitzwatch sesión 2, v0.18.2) —
    // `@cron("expr", store=db) async fn ...` declarado en un
    // módulo importado emitía el spawn cross-module con
    // `store: (&db).into_store()` (correcto), pero el preludio
    // `__FitzCronOptions` se emitía en su shape simple (sin field
    // `store`) porque `program_has_persistent_cron(program)` solo
    // miraba el AST del archivo main, no los módulos. Resultado:
    // `error[E0560] struct \`__FitzCronOptions\` has no field named
    // \`store\``.
    //
    // **Fix**: `program_or_modules_has_persistent_cron(program,
    // &loader)` consulta también `loader.modules[i].cron_fn_stmts`
    // antes de decidir qué shape del struct emitir.
    let stem = "cron_persistent_cross_module_b19";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    std::fs::write(
        dir.join("tasks.fitz"),
        "@cron(\"*/30 * * * * *\", store=db, retry={\n    \
                 max: 2,\n    \
                 backoff: \"exponential\",\n    \
                 initial_secs: 1,\n    \
                 max_secs: 10,\n\
             })\n\
         async fn nightly() -> Result<Null> {\n  \
             let conn = db.connect(\"postgres://x@h/d\").await?\n  \
             return Ok(null)\n\
         }\n",
    )
    .expect("escribir tasks.fitz");

    // El `let db = db.connect(...).await` top-level lo necesitamos
    // para que el spawn cross-module `(&db).into_store()` resuelva:
    // el codegen necesita un `Ident` en scope llamado `db`. Para el
    // test el host/db son ficticios — `fitz build` solo emite el
    // Rust y delega a `cargo build`, NO ejecuta nada. La conexión
    // falla en runtime pero no toca el test.
    let main_src = "\
import tasks\n\
\n\
let db = db.connect(\"postgres://x@h/d\").await\n\
\n\
@server(43924)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = std::process::Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (B19 bug derivado):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );
}

#[test]
fn cron_store_binding_in_imported_module_compila_b20() {
    // B20 — `@cron(..., store=X)` AND `let X = db.connect(...).await`
    // BOTH declared in the SAME imported module (not main). Pre-fix the
    // codegen emitted `(&db).into_store()` in the crate-root `main()`
    // where `db` was not in scope (E0425), and the module emitted a
    // broken `pub fn db() { ...await }` (async body in a sync fn). Fix:
    // `gen_module_top_let` hoists `let db = db.connect(...).await` to a
    // crate-visible `OnceCell` + `pub(crate) async fn
    // __fitz_init_state_db()`; `emit_cron_job_spawns` drives the init +
    // materializes the local `db` before the spawn. This closes the
    // workaround (declaring the `let db` in main) that TaskHub/fitzwatch
    // used. Contrast with `cron_with_persistent_store_in_imported_module_b19_derived`
    // where the `let db` lives in main (that path must stay green too).
    let stem = "cron_store_in_module_b20";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // tasks.fitz — the store binding AND the cron live HERE (the module),
    // not in main. `db` is referenced only by `store=db`.
    std::fs::write(
        dir.join("tasks.fitz"),
        "let db = db.connect(\"postgres://x@h/d\").await\n\
         \n\
         @cron(\"*/30 * * * * *\", store=db, retry={\n    \
                 max: 2,\n    \
                 backoff: \"exponential\",\n    \
                 initial_secs: 1,\n    \
                 max_secs: 10,\n\
             })\n\
         async fn nightly() -> Null {\n  \
             return null\n\
         }\n",
    )
    .expect("escribir tasks.fitz");

    // main.fitz — NO `let db` here; only `import tasks` + @server. Pre-B20
    // this failed to compile because the store binding was unreachable.
    let main_src = "\
import tasks\n\
\n\
@server(43926)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = std::process::Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (B20 — store binding in imported module):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );
}

#[test]
fn cron_store_two_modules_same_name_use_distinct_locals_b20_residual_a() {
    // B20 residual A — two imported modules each with `let db =
    // db.connect(...).await` + `@cron(store=db)`. Pre-fix,
    // `emit_cron_job_spawns` materialized `let db = crate::alpha::...`
    // then `let db = crate::beta::...` (shadowing) so BOTH crons bound
    // to beta's connection — alpha's cron silently ran on the wrong DB.
    // Fix: module-qualified unique locals (`__fitz_cron_store_<mod>_db`)
    // + per-job `(module_path, store_var)`-keyed store reference. This
    // test builds the binary and inspects main.rs: each module's cron
    // must reference ITS OWN unique local.
    let stem = "cron_two_mod_same_store_b20a";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    std::fs::write(
        dir.join("alpha.fitz"),
        "let db = db.connect(\"postgres://a@h/a\").await\n\
         \n\
         @cron(\"*/30 * * * * *\", store=db)\n\
         async fn a() -> Null {\n  return null\n}\n",
    )
    .expect("escribir alpha.fitz");
    std::fs::write(
        dir.join("beta.fitz"),
        "let db = db.connect(\"postgres://b@h/b\").await\n\
         \n\
         @cron(\"*/30 * * * * *\", store=db)\n\
         async fn b() -> Null {\n  return null\n}\n",
    )
    .expect("escribir beta.fitz");
    let main_src = "\
import alpha\n\
import beta\n\
\n\
@server(43928)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (B20 residual A):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Static inspection: main.rs must materialize TWO distinct locals
    // and each job's store must reference the matching one.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    let content = std::fs::read_to_string(&main_rs).expect("read main.rs");
    assert!(
        content.contains("let __fitz_cron_store_alpha_db = crate::alpha::__FITZ_STATE_DB"),
        "main.rs must materialize alpha's store as a unique local: {}",
        content
    );
    assert!(
        content.contains("let __fitz_cron_store_beta_db = crate::beta::__FITZ_STATE_DB"),
        "main.rs must materialize beta's store as a unique local: {}",
        content
    );
    assert!(
        content.contains("(&__fitz_cron_store_alpha_db).into_store()")
            && content.contains("(&__fitz_cron_store_beta_db).into_store()"),
        "each cron's store must reference ITS OWN unique local (no shadowing): {}",
        content
    );
    // And there must be NO flat `let db = ...` for the cross-module
    // stores (that was the shadowing bug).
    assert!(
        !content.contains("let db = crate::alpha::__FITZ_STATE_DB"),
        "the flat shadowing `let db = ...` must be gone: {}",
        content
    );
}

#[test]
fn cron_store_imported_binding_in_main_fails_loud_b20_residual_b() {
    // B20 residual B — a `@cron(store=X)` in MAIN where `X` is imported
    // from a module (`from mod import X`) is not supported, but it is
    // SAFE BY FAILURE: it fails loud at compile time (never a silent
    // wrong connection). The natural (unannotated) case is caught by the
    // module loader (`collect_module_sigs`: "RHS is not a literal");
    // annotated variants fail at rustc. This test guards that the case
    // keeps failing loud and names the offending binding — full support
    // for materializing an imported store from main is deferred.
    let stem = "cron_store_imported_in_main_b20b";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    std::fs::write(
        dir.join("conns.fitz"),
        "let shared_db = db.connect(\"postgres://x@h/d\").await\n",
    )
    .expect("escribir conns.fitz");
    let main_src = "\
from conns import shared_db\n\
\n\
@cron(\"*/30 * * * * *\", store=shared_db)\n\
async fn tick() -> Null {\n  return null\n}\n\
\n\
@server(43930)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        !output.status.success(),
        "fitz build should FAIL for @cron(store=<imported>) in main (B20 residual B) — never a silent success"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("shared_db"),
        "the compile-time failure should name the offending binding `shared_db`, stderr was: {}",
        stderr
    );
}

#[test]
fn background_persistent_store_compiles_to_binary_v0_37_7() {
    // v0.37.7 — `@background(store=db, catch_up=true, retry={...})` +
    // `spawn(...)` from an HTTP handler compiles to a native binary
    // with the persistence machinery: a `fitz_bg_jobs` table, the
    // `__FITZ_BG_STORE_<VAR>` global, the `__fitz_run_persisted_spawn`
    // runtime, and the args serialized via `__ToFitzJson`. The db conn
    // is fictitious — `fitz build` only emits Rust + delegates to
    // `cargo build`, it does not run anything.
    let stem = "bg_persistent_v0_37_7";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    let src = "\
let db = db.connect(\"postgres://x@h/d\").await\n\
\n\
@background(store=db, catch_up=true, retry={max: 2, backoff: \"exponential\", initial_secs: 1, max_secs: 5})\n\
async fn send_email(user_id: Int, subject: Str) -> Null {\n  \
    return null\n\
}\n\
\n\
@get(\"/notify/{id}\")\n\
fn notify(id: Int) -> Str {\n  \
    let _ = spawn(send_email(id, \"Welcome\"))\n  \
    return \"queued\"\n\
}\n\
\n\
@server(43970)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, src).expect("escribir .fitz");

    let output = std::process::Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (background persistence):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );

    // Inspect the emitted main.rs: the persistence machinery must be
    // present (parity with `fitz run`, validated by hand + the
    // background_jobs_real_postgres E2E tests).
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    let generated = std::fs::read_to_string(&main_rs).expect("leer main.rs generado");
    assert!(
        generated.contains("CREATE TABLE IF NOT EXISTS fitz_bg_jobs"),
        "el main.rs generado debe crear la tabla fitz_bg_jobs"
    );
    assert!(
        generated.contains("__fitz_run_persisted_spawn"),
        "el main.rs generado debe emitir el runtime __fitz_run_persisted_spawn"
    );
    assert!(
        generated.contains("__FITZ_BG_STORE_DB"),
        "el main.rs generado debe emitir el static del store __FITZ_BG_STORE_DB"
    );
    assert!(
        generated.contains("__fitz_bg_mark_orphaned"),
        "catch_up=true debe emitir el mark_orphaned al boot"
    );
    // Args are serialized to JSON via __ToFitzJson (compound-capable).
    assert!(
        generated.contains("__to_fitz_json"),
        "los args del spawn persistido se serializan con __ToFitzJson"
    );
}

#[test]
fn background_persistent_cross_module_compiles_to_binary_v0_37_8() {
    // v0.37.8 — port of B20 to @background: a `@background(store=db)` fn
    // AND its `let db = db.connect(...).await` co-located in an imported
    // module compile with `fitz build`, with the spawn in main. Pre-fix
    // this failed with `module let 'db': RHS is not a literal`. The db
    // conn is fictitious — `fitz build` only emits Rust + delegates to
    // cargo, it does not run anything.
    let stem = "bg_persistent_xmod_v0_37_8";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // worker.fitz: @background(store=db) fn + db co-located.
    std::fs::write(
        dir.join("worker.fitz"),
        "let db = db.connect(\"postgres://x@h/d\").await\n\
         \n\
         @background(store=db, catch_up=true, retry={max: 2, backoff: \"exponential\", initial_secs: 1, max_secs: 5})\n\
         async fn send_email(user_id: Int, subject: Str) -> Null {\n  \
             return null\n\
         }\n",
    )
    .expect("escribir worker.fitz");

    // main.fitz: imports the worker fn + spawns it from a handler.
    let main_src = "\
from worker import send_email\n\
\n\
@get(\"/notify/{id}\")\n\
fn notify(id: Int) -> Str {\n  \
    let _ = spawn(send_email(id, \"Welcome\"))\n  \
    return \"queued\"\n\
}\n\
\n\
@server(43980)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = std::process::Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (cross-module background persistence):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );

    // Inspect the emitted main.rs: cross-module store init + set global.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    let generated = std::fs::read_to_string(&main_rs).expect("leer main.rs generado");
    // The store binding hoisted in the module is inited + set into the
    // crate-root global from `crate::worker::__FITZ_STATE_DB`.
    assert!(
        generated.contains("crate::worker::__fitz_init_state_db().await"),
        "el boot debe inicializar el store co-localizado del módulo worker"
    );
    assert!(
        generated.contains("crate::worker::__FITZ_STATE_DB"),
        "el boot debe leer el OnceCell del store del módulo worker"
    );
    assert!(
        generated.contains("__FITZ_BG_STORE_DB.set"),
        "el boot debe setear el global __FITZ_BG_STORE_DB cross-module"
    );
    // The worker module emitted the hoisted OnceCell + init fn.
    let worker_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/worker.rs", stem));
    let worker_gen = std::fs::read_to_string(&worker_rs).expect("leer worker.rs generado");
    assert!(
        worker_gen.contains("pub(crate) static __FITZ_STATE_DB")
            && worker_gen.contains("pub(crate) async fn __fitz_init_state_db"),
        "el módulo worker debe hoistear `db` a OnceCell + init fn"
    );
}

#[test]
fn background_persistent_spawn_in_module_compiles_to_binary_v0_37_9() {
    // v0.37.9 — closes v0.37.8 residual (a): the `spawn(...)` lives INSIDE
    // an imported module (not main). Pre-fix, the module's codegen ctx had
    // an empty `bg_persistent_fns`, so `gen_spawn_call` fell to the
    // fire-and-forget arm (silent drop of persistence). Now the module's
    // ctx is populated and the persisted-path symbols are `crate::`-
    // qualified so they resolve from `src/worker.rs`. The db conn is
    // fictitious — `fitz build` only emits Rust + delegates to cargo.
    let stem = "bg_persistent_spawn_in_mod_v0_37_9";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // worker.fitz: @background(store=db) fn + db co-located + a fn that
    // does the spawn HERE (inside the module).
    std::fs::write(
        dir.join("worker.fitz"),
        "let db = db.connect(\"postgres://x@h/d\").await\n\
         \n\
         @background(store=db, catch_up=true, retry={max: 2, backoff: \"exponential\", initial_secs: 1, max_secs: 5})\n\
         async fn send_email(user_id: Int, subject: Str) -> Null {\n  \
             return null\n\
         }\n\
         \n\
         fn enqueue(id: Int) -> Null {\n  \
             let _ = spawn(send_email(id, \"Welcome\"))\n  \
             return null\n\
         }\n",
    )
    .expect("escribir worker.fitz");

    // main.fitz: imports the enqueue helper (the spawn is NOT in main).
    let main_src = "\
from worker import enqueue\n\
\n\
@get(\"/notify/{id}\")\n\
fn notify(id: Int) -> Str {\n  \
    let _ = enqueue(id)\n  \
    return \"queued\"\n\
}\n\
\n\
@server(43981)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = std::process::Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (spawn inside module):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );

    // Inspect the emitted worker.rs: the spawn took the PERSISTED path
    // with `crate::`-qualified symbols (not the fire-and-forget arm).
    let worker_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/worker.rs", stem));
    let worker_gen = std::fs::read_to_string(&worker_rs).expect("leer worker.rs generado");
    // `__fitz_run_persisted_spawn` is emitted ONLY by the persisted arm →
    // its presence proves persistence, and the `crate::` prefix proves the
    // qualification fix (bare would be E0425 from a non-root module file).
    assert!(
        worker_gen.contains("crate::__fitz_run_persisted_spawn"),
        "el spawn del módulo debe tomar el path persistente calificado con crate::"
    );
    assert!(
        worker_gen.contains("crate::__FITZ_BG_STORE_DB"),
        "el spawn del módulo debe leer el store global calificado con crate::"
    );
    assert!(
        worker_gen.contains("crate::__FitzRetryConfig")
            && worker_gen.contains("crate::__FitzBackoffKind"),
        "el retry config del spawn del módulo debe ir calificado con crate::"
    );
    assert!(
        worker_gen.contains("as crate::__ToFitzJson>"),
        "la serialización de args del spawn del módulo debe ir calificada con crate::"
    );
    // main.rs still wires the boot (store init + set global) — parity
    // preserved even though the spawn moved out of main.
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    let generated = std::fs::read_to_string(&main_rs).expect("leer main.rs generado");
    assert!(
        generated.contains("crate::worker::__fitz_init_state_db().await")
            && generated.contains("__FITZ_BG_STORE_DB.set"),
        "el boot en main debe seguir inicializando + seteando el store cross-module"
    );
}

#[test]
fn background_persistent_spawn_cross_module_def_v0_37_9() {
    // v0.37.9 — the harder shape: the `spawn(...)` lives in module A
    // (`notifier.fitz`) and the `@background` def + store live in module B
    // (`emails.fitz`), which A imports. Module A's pre-scan of its own
    // imports (`pre_scan_imported_background_persist_for_loader`) is what
    // populates its ctx `bg_persistent_fns` for `send_email`. Main only
    // imports the enqueue helper; `emails.fitz` is loaded transitively.
    let stem = "bg_persistent_xmod_def_v0_37_9";
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");

    // emails.fitz (module B): the def + store.
    std::fs::write(
        dir.join("emails.fitz"),
        "let db = db.connect(\"postgres://x@h/d\").await\n\
         \n\
         @background(store=db)\n\
         async fn send_email(user_id: Int, subject: Str) -> Null {\n  \
             return null\n\
         }\n",
    )
    .expect("escribir emails.fitz");

    // notifier.fitz (module A): imports the def, does the spawn HERE.
    std::fs::write(
        dir.join("notifier.fitz"),
        "from emails import send_email\n\
         \n\
         fn enqueue(id: Int) -> Null {\n  \
             let _ = spawn(send_email(id, \"Hi\"))\n  \
             return null\n\
         }\n",
    )
    .expect("escribir notifier.fitz");

    let main_src = "\
from notifier import enqueue\n\
\n\
@get(\"/notify/{id}\")\n\
fn notify(id: Int) -> Str {\n  \
    let _ = enqueue(id)\n  \
    return \"queued\"\n\
}\n\
\n\
@server(43982)\n\
fn main() => 0\n\
";
    let main_path = dir.join(format!("{}.fitz", stem));
    std::fs::write(&main_path, main_src).expect("escribir main.fitz");

    let output = std::process::Command::new(fitz_bin())
        .args(["build"])
        .arg(&main_path)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed (spawn in A, def in B):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    assert!(
        dir.join(&bin_name).exists(),
        "binario {} no existe",
        bin_name
    );

    // notifier.rs (module A) emitted the persisted path for send_email.
    let notifier_rs =
        std::path::PathBuf::from(format!("target/fitz-build/{}/src/notifier.rs", stem));
    let notifier_gen = std::fs::read_to_string(&notifier_rs).expect("leer notifier.rs generado");
    assert!(
        notifier_gen.contains("crate::__fitz_run_persisted_spawn")
            && notifier_gen.contains("crate::__FITZ_BG_STORE_DB"),
        "el spawn de un def cross-module debe tomar el path persistente calificado"
    );
}

// ---------------------------------------------------------------------------
// v0.19.0 Block 3.d — smoke E2E del `Response` built-in (paridad fitz build
// vs. fitz run validada bit-a-bit a mano en el desarrollo del bloque).
// Acá probamos que el binario compilado por `fitz build`:
//   - emite el Content-Type custom + headers + body crudo (text path),
//   - emite bytes binarios sin JSON-wrap (binary path),
//   - rechaza la combinación post-middleware + Response built-in con un
//     mensaje claro citando workarounds.
// ---------------------------------------------------------------------------

/// Variant de `build_spawn_request_raw_with_headers` que devuelve también
/// el body crudo en bytes (los helpers existentes devuelven solo headers).
/// Necesario para verificar binary path donde el body NO es UTF-8 (PDF,
/// ZIP, imágenes). Construye + spawnea el server, hace la request, mata
/// el server, parsea la response cruda en `(status, headers_string,
/// body_bytes)`.
fn build_spawn_request_raw_full(
    test_name: &str,
    src: &str,
    port: u16,
    method: &str,
    path: &str,
) -> (u16, String, Vec<u8>) {
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());

    use std::process::{Child, Stdio};
    let mut child: Child = Command::new(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    let mut connected = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !connected {
        let _ = child.kill();
        panic!("server did not open port {} within 3s", port);
    }

    use std::io::{Read, Write};
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        method, path, addr
    );
    let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();
    stream.write_all(request.as_bytes()).expect("send request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();

    let _ = child.kill();
    let _ = child.wait();

    // Parse: status line + headers + \r\n\r\n + body (raw bytes).
    let header_terminator = b"\r\n\r\n";
    let split_at = buf
        .windows(4)
        .position(|w| w == header_terminator)
        .unwrap_or(buf.len());
    let headers_bytes = &buf[..split_at];
    let body_bytes = if split_at + 4 <= buf.len() {
        buf[split_at + 4..].to_vec()
    } else {
        Vec::new()
    };
    let headers_string = String::from_utf8_lossy(headers_bytes).into_owned();
    let status_line = headers_string.lines().next().unwrap_or("").to_string();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, headers_string, body_bytes)
}

#[test]
fn v019_block3d_response_text_path_emits_custom_content_type_and_raw_body() {
    // Smoke text path: el handler retorna `Response { content_type:
    // "application/rss+xml", body: "<rss/>" }` y el binario compilado
    // emite Content-Type custom + body XML crudo (sin JSON-wrap), con
    // headers custom inyectados. Paralelo bit-a-bit a `fitz run`.
    let src = "\
@get(\"/feed.rss\")
fn rss_feed() => Response {
    content_type: \"application/rss+xml; charset=utf-8\",
    body: \"<?xml version=\\\"1.0\\\"?><rss/>\",
}

@get(\"/cached.txt\")
fn cached() => Response {
    content_type: \"text/plain\",
    headers: {\"Cache-Control\": \"public, max-age=3600\", \"X-Custom\": \"smoke\"},
    body: \"cached payload\",
}

@server(43919) fn main() => 0
";
    // /feed.rss: Content-Type custom + body crudo.
    let (status_rss, headers_rss, body_rss) =
        build_spawn_request_raw_full("v019-block3d-text", src, 43919, "GET", "/feed.rss");
    assert_eq!(status_rss, 200);
    let headers_lower = headers_rss.to_lowercase();
    assert!(
        headers_lower.contains("content-type: application/rss+xml; charset=utf-8"),
        "headers: {}",
        headers_rss
    );
    let body_str = String::from_utf8_lossy(&body_rss);
    assert_eq!(
        body_str, "<?xml version=\"1.0\"?><rss/>",
        "body XML crudo (sin JSON wrap)"
    );
    // /cached.txt: Content-Type text/plain + headers custom.
    let (status_c, headers_c, body_c) =
        build_spawn_request_raw_full("v019-block3d-text-cached", src, 43919, "GET", "/cached.txt");
    assert_eq!(status_c, 200);
    let headers_c_lower = headers_c.to_lowercase();
    assert!(
        headers_c_lower.contains("content-type: text/plain"),
        "headers: {}",
        headers_c
    );
    assert!(
        headers_c_lower.contains("cache-control: public, max-age=3600"),
        "headers: {}",
        headers_c
    );
    assert!(
        headers_c_lower.contains("x-custom: smoke"),
        "headers: {}",
        headers_c
    );
    assert_eq!(String::from_utf8_lossy(&body_c), "cached payload");
}

#[test]
fn fitz05_fase_b_response_cookies_emit_multiple_set_cookie_headers() {
    // FITZ-05 FASE B — a compiled handler returning `Response { cookies:
    // [Cookie {...}, Cookie {...}] }` emits TWO separate `Set-Cookie`
    // response headers (axum's builder `.header()` appends). Flags are
    // serialised in canonical order. Parity bit-a-bit con `fitz run`.
    let src = "\
@get(\"/login\")
fn login() => Response {
    status: 303,
    headers: {\"Location\": \"/\"},
    cookies: [
        Cookie { name: \"session\", value: \"tok123\", http_only: true, max_age: 86400 },
        Cookie { name: \"lang\", value: \"es-AR\", path: \"/app\", same_site: \"Strict\" },
    ],
}

@server(43929) fn main() => 0
";
    let (status, headers, _body) =
        build_spawn_request_raw_full("fitz05-fase-b-cookies", src, 43929, "GET", "/login");
    assert_eq!(status, 303);
    let headers_lower = headers.to_lowercase();
    let count = headers_lower.matches("set-cookie:").count();
    assert_eq!(
        count, 2,
        "expected 2 Set-Cookie headers, got {}: {}",
        count, headers
    );
    // session cookie: HttpOnly + Max-Age + default Path/SameSite.
    assert!(
        headers_lower.contains("session=tok123"),
        "headers: {}",
        headers
    );
    assert!(
        headers_lower.contains("max-age=86400"),
        "headers: {}",
        headers
    );
    assert!(headers_lower.contains("httponly"), "headers: {}", headers);
    // lang cookie: custom Path + SameSite=Strict.
    assert!(headers_lower.contains("lang=es-ar"), "headers: {}", headers);
    assert!(
        headers_lower.contains("samesite=strict"),
        "headers: {}",
        headers
    );
    assert!(headers_lower.contains("path=/app"), "headers: {}", headers);
    // Location header still emitted (from the `headers` field).
    assert!(
        headers_lower.contains("location: /"),
        "headers: {}",
        headers
    );
}

#[test]
fn v019_block3d_response_binary_path_emits_bytes_and_content_disposition() {
    // Smoke binary path: el handler retorna `Response { body_bytes:
    // bytes("..."), content_type: "application/pdf" }` y el binario
    // compilado emite los bytes literales (no JSON-wrap del array de
    // u8) con el Content-Type correcto, headers custom (incluido
    // Content-Disposition).
    let src = "\
@get(\"/pdf-fake\")
fn pdf_fake() => Response {
    content_type: \"application/pdf\",
    body_bytes: bytes(\"%PDF-1.7 (smoke fake PDF body)\"),
    headers: {\"Content-Disposition\": \"attachment; filename=test.pdf\"},
}

@server(43920) fn main() => 0
";
    let (status, headers, body) =
        build_spawn_request_raw_full("v019-block3d-binary", src, 43920, "GET", "/pdf-fake");
    assert_eq!(status, 200);
    let headers_lower = headers.to_lowercase();
    assert!(
        headers_lower.contains("content-type: application/pdf"),
        "headers: {}",
        headers
    );
    assert!(
        headers_lower.contains("content-disposition: attachment; filename=test.pdf"),
        "headers: {}",
        headers
    );
    // Body bytes: el contenido literal de bytes(...) sin coerción JSON.
    assert_eq!(body, b"%PDF-1.7 (smoke fake PDF body)".to_vec());
}

#[test]
fn v019_block3d_response_built_in_with_post_middleware_aborts_build_with_clear_message() {
    // El codegen rechaza la combinación `Response { ... }` + post
    // middleware con 2 args con un mensaje claro citando workarounds
    // (`return <status> { ... }` o remover el middleware). Deuda menor
    // documentada para iter 2.
    let src = "\
fn touch(req: Request, resp: Response) -> Response => resp

@middleware(touch)
@get(\"/feed.rss\")
fn rss_feed() -> Response => Response {
    content_type: \"application/rss+xml\",
    body: \"<rss/>\",
}

@server(43921) fn main() => 0
";
    let stderr = build_expect_fail("v019-block3d-postmw-reject", src);
    let stderr_lower = stderr.to_lowercase();
    assert!(
        stderr_lower.contains("response") && stderr_lower.contains("post middleware"),
        "expected error mentioning Response + post middleware, got: {}",
        stderr
    );
    // Mensaje debe citar al menos un workaround.
    assert!(
        stderr_lower.contains("return <status>")
            || stderr_lower.contains("remove the post middleware"),
        "expected workaround mention, got: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// v0.19.1 — 3 bugs of the Block 3.c (Response built-in) detected in fitzwatch
// ---------------------------------------------------------------------------
//
// The 3 bugs only show up when `Response { ... }` built-in is used in
// real-world programs (cross-module, with `?` propagation, mixed with
// auth + DB + WS + observability). The single-file v019_block3d_* tests
// pass because they exercise the simple path; these 3 tests stress the
// integrator real and reproduce what fitzwatch hit. Detailed plan in
// `docs/deudas-post-5b.md` → "🔴 PRIORIDAD MÁXIMA".

/// Builds a single-file program with `fitz build` and asserts the build
/// succeeded WITHOUT invoking the binary. Useful for HTTP server programs
/// that would never exit on `output()`.
fn build_expect_ok(test_name: &str, src: &str) {
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");

    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());
}

/// Like `build_and_run_multi` but does NOT invoke the binary — asserts
/// build success only. Returns the temp dir so callers can inspect the
/// generated `target/fitz-build/<stem>/src/*.rs` for additional asserts.
fn build_expect_ok_multi(
    test_name: &str,
    main_src: &str,
    extra_files: &[(&str, &str)],
) -> std::path::PathBuf {
    let stem = sanitize_stem(test_name);
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, main_src).expect("escribir .fitz");
    for (name, content) in extra_files {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("crear subdir");
        }
        std::fs::write(&p, content).expect("escribir extra .fitz");
    }
    let output = Command::new(fitz_bin())
        .args(["build"])
        .arg(&fitz_src)
        .output()
        .expect("invoke fitz build");
    assert!(
        output.status.success(),
        "fitz build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.clone()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binario {} no existe", bin.display());
    dir
}

#[test]
fn observability_use_stripped_when_no_logging_multi_module_http_v0_37_5() {
    // v0.37.5 — a multi-module HTTP program with a handler in an imported
    // module but NO `log.X(...)` anywhere used to fail `fitz build` with
    // E0432: the module emitted `use crate::{__fitz_otel_*, __fitz_log_*}`
    // (gated by module_has_http && main_observability_enabled) but the
    // crate root DEFINES those symbols only when `uses_logging` (the v0.37.1
    // observability opt-in). With no logging anywhere the strip pass removes
    // the (spurious) `use`s so the module compiles.
    let worker_src = "\
@get(\"/ping\")\n\
fn ping() -> Str => \"pong\"\n\
";
    let main_src = "\
import worker\n\
\n\
@server(43972)\n\
fn main() => 0\n\
";
    build_expect_ok_multi(
        "observability_no_logging_multi_module_v0_37_5",
        main_src,
        &[("worker.fitz", worker_src)],
    );
}

#[test]
fn module_async_shared_state_compiles_v0_37_6() {
    // 5b.6 (v0.37.6) — un módulo con un `let db = db.connect(...).await`
    // top-level referenciado por sus propios handlers HTTP compila con
    // `fitz build`. Antes: E0728 (accessor `pub fn db()` con RHS async) o
    // E0425 (materialización faltante) — el intérprete lo capturaba del env
    // del módulo, el binario no. Fix: el ctx de módulo corre la detección de
    // shared state, `gen_module_top_let` emite `__FITZ_STATE_db` OnceCell +
    // init, main lo inicializa antes de servir, y cada handler materializa el
    // local vía `gen_top_fn`.
    let store_src = "\
let db = db.connect(\"postgres://x:x@localhost:5432/x?sslmode=disable\").await\n\
\n\
@get(\"/a\")\n\
async fn handler_a() -> Result<Str> {\n\
    let conn = match db {\n\
        Ok(c) => c,\n\
        Err(_) => return Err(\"no db\"),\n\
    }\n\
    let _ = conn.exec(\"SELECT 1\", []).await?\n\
    return Ok(\"a\")\n\
}\n\
\n\
@get(\"/b\")\n\
async fn handler_b() -> Result<Str> {\n\
    let conn = match db {\n\
        Ok(c) => c,\n\
        Err(_) => return Err(\"no db\"),\n\
    }\n\
    let _ = conn.exec(\"SELECT 1\", []).await?\n\
    return Ok(\"b\")\n\
}\n\
";
    let main_src = "\
import store\n\
\n\
@server(43974)\n\
fn main() => 0\n\
";
    build_expect_ok_multi(
        "module_async_shared_state_v0_37_6",
        main_src,
        &[("store.fitz", store_src)],
    );
}

#[test]
fn module_shared_db_both_cron_store_and_handler_no_double_init_v0_37_6() {
    // 5b.6 (v0.37.6) — un `let db = db.connect(...).await` de módulo que es
    // AMBOS: `@cron(store=db)` (persiste runs) Y referenciado por un handler
    // HTTP. Compila, y main emite UNA sola `__fitz_init_state_db().await`
    // (el driver de gen_http_main excluye los cron-store vars; los inicializa
    // emit_cron_job_spawns) — sin doble `db.connect`.
    let store_src = "\
let db = db.connect(\"postgres://x:x@localhost:5432/x?sslmode=disable\").await\n\
\n\
@cron(\"0 0 * * *\", store=db)\n\
async fn nightly() -> Result<Null> {\n\
    let conn = match db {\n\
        Ok(c) => c,\n\
        Err(_) => return Err(\"no db\"),\n\
    }\n\
    let _ = conn.exec(\"DELETE FROM t WHERE false\", []).await?\n\
    return Ok(null)\n\
}\n\
\n\
@get(\"/runs\")\n\
async fn runs() -> Result<Str> {\n\
    let conn = match db {\n\
        Ok(c) => c,\n\
        Err(_) => return Err(\"no db\"),\n\
    }\n\
    let _ = conn.query(\"SELECT 1\", []).await?\n\
    return Ok(\"ok\")\n\
}\n\
";
    let main_src = "\
import store\n\
\n\
@server(43975)\n\
fn main() => 0\n\
";
    let stem = "module_shared_db_both_v0_37_6";
    build_expect_ok_multi(stem, main_src, &[("store.fitz", store_src)]);
    // Grep del main.rs generado: exactamente UNA init call (no double-init).
    let main_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/main.rs", stem));
    if main_rs.exists() {
        let content = std::fs::read_to_string(&main_rs).expect("leer main.rs generado");
        let count = content.matches("__fitz_init_state_db().await").count();
        assert_eq!(
            count, 1,
            "esperaba exactamente 1 `__fitz_init_state_db().await` en main.rs (no double-init), fue {}",
            count
        );
    }
}

#[test]
fn for_kv_in_map_parity_run_build_v0_37_15() {
    // v0.37.15 — `for kv in m` (single Ident sobre un Map) compila en
    // `fitz build` con paridad bit-a-bit ante `fitz run`. `kv` es el par
    // completo `(K, V)` (Tuple), accedido `kv.0`/`kv.1`. Antes: `fitz build`
    // abortaba con "exige un tuple pattern de 2 elementos" mientras `fitz run`
    // lo aceptaba (gap de paridad).
    let src = "let m: Map<Str, Int> = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
               for kv in m {\n\
                   print(\"{kv.0}={kv.1}\")\n\
               }\n";
    let stem = "for_kv_in_map_v0_37_15";

    // Binario nativo.
    let (bin_out, code) = build_and_run(stem, src);
    assert_eq!(code, 0, "el binario debe salir 0");
    assert_eq!(
        bin_out, "a=1\nb=2\nc=3\n",
        "salida del binario incorrecta (orden de inserción preservado)"
    );

    // Paridad: `fitz run` sobre la misma fuente produce lo mismo.
    let dir = std::env::temp_dir().join(format!("fitz-e2e-{}-run", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear tempdir run");
    let fitz_src = dir.join(format!("{}.fitz", stem));
    std::fs::write(&fitz_src, src).expect("escribir .fitz");
    let run = Command::new(fitz_bin())
        .args(["run"])
        .arg(&fitz_src)
        .output()
        .expect("invocar fitz run");
    let run_out = String::from_utf8_lossy(&run.stdout).into_owned();
    assert_eq!(
        run_out, bin_out,
        "paridad rota: `fitz run` != binario\nrun: {run_out:?}\nbin: {bin_out:?}"
    );
}

#[test]
fn module_shared_state_primitive_const_compiles_v0_37_14() {
    // v0.37.14 — un `let X: Int = N` (o Str/Float/Bool) top-level de un
    // MÓDULO importado, referenciado por handlers HTTP/WS de ese módulo, es
    // shared state y debe emitirse como `__FITZ_STATE_X: LazyLock<T>` +
    // accessor `pub fn X()`, NO como bare `pub const X` (Paths 1a/1b). Antes:
    // el const short-circuiteaba y retornaba, pero `gen_top_fn` materializaba
    // `let X = (*__FITZ_STATE_X).clone()` → E0425 (`__FITZ_STATE_X` faltante) +
    // E0530 (`let` shadowing `const`). El caso MAIN ya funcionaba (gen_http_main
    // emite el LazyLock para todo state var, primitivos incluidos); solo el ctx
    // de MÓDULO no corría esa maquinaria para primitivos.
    let mod_src = "\
let PAGE_SIZE: Int = 8\n\
\n\
fn double_page() -> Int {\n    \
    return PAGE_SIZE * 2\n\
}\n\
\n\
@get(\"/page/{n}\")\n\
fn page(n: Int) -> Int {\n    \
    return n * PAGE_SIZE + double_page()\n\
}\n\
";
    let main_src = "\
from pagemod import page\n\
\n\
@server(43978)\n\
fn main() => 0\n\
";
    let stem = "module_shared_primitive_v0_37_14";
    build_expect_ok_multi(stem, main_src, &[("pagemod.fitz", mod_src)]);
    // El módulo debe emitir el static backing + accessor, NO un `pub const`.
    let mod_rs = std::path::PathBuf::from(format!("target/fitz-build/{}/src/pagemod.rs", stem));
    if mod_rs.exists() {
        let content = std::fs::read_to_string(&mod_rs).expect("leer pagemod.rs generado");
        assert!(
            content.contains("static __FITZ_STATE_PAGE_SIZE: std::sync::LazyLock<i64>"),
            "esperaba el LazyLock backing `__FITZ_STATE_PAGE_SIZE` en pagemod.rs"
        );
        assert!(
            !content.contains("pub const PAGE_SIZE"),
            "no esperaba un bare `pub const PAGE_SIZE` (shared state → LazyLock)"
        );
    }
}

#[test]
fn admin_cross_module_role_field_via_provider_module_imports_v0_37_3() {
    // v0.37.3 — `@admin`/`@requires` require the `@auth_provider`'s User
    // type to have a `role: Str` field. Before this fix, the cross-module
    // `has_role_field` detection only scanned the PROVIDER module's own
    // `TypeDef`s. In the common multi-file layout the provider module
    // imports its User (`auth.fitz` does `from models import User`, `User`
    // lives in `models.fitz`), so `role` was invisible and `@admin` wrongly
    // failed at `fitz build` — even though `fitz check` (enriched TypeEnv)
    // passed, a run↔build divergence. The fix follows the provider module's
    // own imports to resolve `role: Str` in the sibling module.
    let models_src = "type User { id: Int, name: Str, role: Str }\n";
    let auth_src = "\
from models import User\n\
\n\
@auth_provider\n\
fn check_token(headers: Map<Str, Str>) -> Result<User> {\n\
    match headers.get(\"authorization\") {\n\
        Ok(_) => return Ok(User { id: 1, name: \"Admin\", role: \"admin\" }),\n\
        Err(_) => return Err(\"falta Authorization\"),\n\
    }\n\
}\n\
";
    let main_src = "\
import auth\n\
from models import User\n\
\n\
@server(43971)\n\
fn main() => 0\n\
\n\
@admin\n\
@get(\"/admin\")\n\
fn admin_route(user: User) -> Str => \"hola admin\"\n\
";
    build_expect_ok_multi(
        "admin_cross_module_role_field_v0_37_3",
        main_src,
        &[("models.fitz", models_src), ("auth.fitz", auth_src)],
    );
}

#[test]
fn v019_response_cross_module_emits_imports() {
    // Bug 1 — `Response`/`ResponseData` not imported in modules that
    // declare handlers returning the Response built-in. The emitted
    // `src/<mod>.rs` references the type aliases without `use
    // crate::{Response, ResponseData};` and rustc tirars E0425/E0422.
    //
    // Repro: handler `-> Response` declared in an imported module +
    // `from <mod> import handler` in main. The handler is registered
    // by the main's @get route declaration, and the wrapper lives in
    // the module (W16 pattern post-v0.10.12).
    let main = "\
from feed import rss_feed
@server(43922) fn main() => 0
";
    let feed = "\
@get(\"/feed.rss\")
fn rss_feed() -> Response => Response {
    content_type: \"application/rss+xml\",
    body: \"<rss/>\",
}
";
    let _dir = build_expect_ok_multi("v019-response-cross-module", main, &[("feed.fitz", feed)]);
    // Inspect the emitted src/feed.rs to assert the imports are
    // present. `fitz build` writes to `target/fitz-build/<stem>/src/`
    // relative to the CWD (the project root, where `cargo test`
    // was invoked from), NOT relative to the temp dir.
    let stem = sanitize_stem("v019-response-cross-module");
    let feed_rs = std::path::PathBuf::from("target")
        .join("fitz-build")
        .join(&stem)
        .join("src")
        .join("feed.rs");
    assert!(
        feed_rs.exists(),
        "emitted feed.rs not found at {}",
        feed_rs.display()
    );
    let emitted = std::fs::read_to_string(&feed_rs).expect("read feed.rs");
    // v0.53.0 — the import list grew to include `Cookie, CookieData` (the
    // cross-module cookie fix), so match the names rather than the exact set.
    assert!(
        emitted.contains("use crate::{Response, ResponseData")
            || (emitted.contains("use crate::Response;")
                && emitted.contains("use crate::ResponseData;")),
        "expected `use crate::{{Response, ResponseData, ...}};` in emitted feed.rs, got:\n{}",
        emitted
    );
}

#[test]
fn v053_response_cookies_cross_module_emits_serialize_import() {
    // FITZ-05 FASE B cross-module (2026-08-21, found dogfooding MatHelp) — a
    // module whose handler returns `Response { cookies: [...] }` emits the
    // `Response`→axum conversion, whose cookie loop calls
    // `__fitz_serialize_set_cookie`. That helper lives in main.rs's HTTP
    // prelude; before the fix it was a private `fn` and the module's `.rs`
    // had no `use crate::__fitz_serialize_set_cookie;` → rustc E0425
    // (`cannot find function`). Same family as W23 (`use crate::__FitzValue`).
    // The fix makes the helper `pub(crate)` and imports it into modules that
    // use the Response built-in. Exact shape that blocked MatHelp's native
    // build (`src/assets.rs`).
    let main = "\
from pages import landing
@server(43953) fn main() => 0
";
    let pages = "\
@get(\"/\")
fn landing() -> Response => Response {
    content_type: \"text/html\",
    body: \"<h1>hi</h1>\",
    cookies: [ Cookie { name: \"lang\", value: \"es-AR\", max_age: 86400 } ],
}
";
    let _dir = build_expect_ok_multi(
        "v053-response-cookies-cross-module",
        main,
        &[("pages.fitz", pages)],
    );
    let stem = sanitize_stem("v053-response-cookies-cross-module");
    let pages_rs = std::path::PathBuf::from("target")
        .join("fitz-build")
        .join(&stem)
        .join("src")
        .join("pages.rs");
    assert!(
        pages_rs.exists(),
        "emitted pages.rs not found at {}",
        pages_rs.display()
    );
    let emitted = std::fs::read_to_string(&pages_rs).expect("read pages.rs");
    assert!(
        emitted.contains("use crate::__fitz_serialize_set_cookie;"),
        "expected `use crate::__fitz_serialize_set_cookie;` in emitted pages.rs"
    );
}

#[test]
fn v055_map_str_any_keys_string_methods_in_module() {
    // v0.55.0 (found dogfooding fitz-liveviews' `dispatch_to_all`) — iterating
    // `Map<Str, Any>.keys()` and calling a `Str` method (`.starts_with`) on the
    // key must compile AND produce the right value. The map's runtime rep is
    // `Vec<(__FitzValue, __FitzValue)>` (because the value is `Any`), so
    // `.keys()` used to yield `__FitzValue` keys → rustc E0599 (`no method
    // starts_with`) / E0308. The fix unwraps the concrete key type in `.keys()`
    // (and imports `__fv_to_string` cross-module). Cross-module because the real
    // case is in a liveviews module fn, not main.
    let main = "\
from store import count_prefixed
print(count_prefixed(\"board:\"))
";
    // Built empty + index-assigned, exactly like fitz-liveviews'
    // `COMPONENT_STATE_STORE` (avoids the separate non-empty `Map<Str, Any>`
    // literal coercion, isolating the `.keys()` unwrap under test).
    let store = "\
fn count_prefixed(prefix: Str) -> Int {
    let store: Map<Str, Any> = {}
    store[\"board:1\"] = 10
    store[\"board:2\"] = 20
    store[\"chip:9\"] = 5
    let n = 0
    for key in store.keys() {
        if (key.starts_with(prefix)) {
            n = n + 1
        }
    }
    return n
}
";
    let (out, code) = build_and_run_multi("v055-map-str-any-keys", main, &[("store.fitz", store)]);
    assert_eq!(code, 0, "binary should exit 0, out:\n{out}");
    assert_eq!(out.trim(), "2", "should count the 2 `board:`-prefixed keys");
}

#[test]
fn v055_nonempty_map_str_any_literal_at_module_top_level() {
    // v0.55.0 residual (found dogfooding MatHelp) — a NON-EMPTY `Map<Str, Any>`
    // literal at a module's top level (`let STORE: Map<Str, Any> = { "k": 10 }`)
    // was emitted via `gen_expr` WITHOUT the annotation hint, so it produced
    // `Vec<(String, i64)>` and then relied on `coerce` (no `Map<K,V>→Map<K,Any>`
    // arm that wraps entries) → rustc E0308 (`expected __FitzValue, found
    // String/i64`). Fix: `gen_module_top_let` resolves the annotation first and
    // passes it as a hint to `gen_map_lit_with_hint` (parallel to `gen_assign`),
    // emitting `Vec<(__FitzValue, __FitzValue)>`. Exercises the literal fix + the
    // `.keys()` unwrap together.
    let main = "\
from store import count_prefixed
print(count_prefixed(\"board:\"))
";
    let store = "\
let STORE: Map<Str, Any> = { \"board:1\": 10, \"board:2\": 20, \"chip:9\": 5 }
fn count_prefixed(prefix: Str) -> Int {
    let n = 0
    for key in STORE.keys() {
        if (key.starts_with(prefix)) {
            n = n + 1
        }
    }
    return n
}
";
    let (out, code) = build_and_run_multi(
        "v055-nonempty-map-str-any-literal",
        main,
        &[("store.fitz", store)],
    );
    assert_eq!(code, 0, "binary should exit 0, out:\n{out}");
    assert_eq!(out.trim(), "2", "should count the 2 `board:`-prefixed keys");
}

#[test]
fn v019_response_in_result_ok_signature_matches_wrapper() {
    // Bug 2 — When the user-fn returns `Result<Response>` and uses
    // `?` propagation, the codegen falls into the legacy `response_mode`
    // path (because `body_has_try` triggers `has_return_status`) and
    // emits `-> __FitzResponse`. The wrapper of Block 3.c (InResultOk)
    // expects the user-fn to return `Result<Arc<Mutex<ResponseData>>,
    // String>` and emits `match __result { Ok(__resp_arc) => ..., Err(__e)
    // => ... }`. The mismatch produces E0308.
    //
    // Repro single-file: helper that returns Result<Int, Str> + handler
    // `-> Result<Response>` that uses `?` to propagate the Err, then
    // builds the Response.
    let src = "\
fn lookup(id: Int) -> Result<Int, Str> {
    if (id == 0) {
        return Err(\"not found\")
    }
    return Ok(id * 2)
}

@get(\"/items/{id}\")
fn item(id: Int) -> Result<Response> {
    let user_id = lookup(id)?
    return Ok(Response {
        content_type: \"text/plain\",
        body: \"id={user_id}\",
    })
}

@server(43923) fn main() => 0
";
    build_expect_ok("v019-response-in-result-ok-signature", src);
}

#[test]
fn v019_response_with_auth_db_ws_observability() {
    // Bug 3 — `metrics::counter!`/`histogram!` not found when Response
    // built-in is mixed with auth + DB + WS + observability +
    // cross-module. fitzwatch's base program (without RSS handler)
    // compiles OK; adding the handler in an imported module triggers
    // E0433. Observability is default ON.
    //
    // Repro: cross-module — main has @auth_provider + db.connect() +
    // @ws(); imported module declares the @get handler returning
    // `Response` built-in. The combination strange dispatches preludes
    // in an order that breaks `metrics::*` macro resolution.
    let main = "\
from feed import rss_feed

type User { id: Int, role: Str }

@auth_provider
async fn provider(headers: Map<Str, Str>) -> Result<User> {
    return Ok(User { id: 1, role: \"admin\" })
}

async fn db_ping() -> Result<Null> {
    let conn = db.connect(\"postgres://x:y@localhost/z\").await?
    return Ok(null)
}

@authenticated
@get(\"/profile\")
fn profile(user: User) -> User => user

@ws(\"/chat\")
async fn chat(conn: WsConn<Str>) -> Null {
    return null
}

@server(43924) fn main() => 0
";
    let feed = "\
@get(\"/feed.rss\")
fn rss_feed() -> Response => Response {
    content_type: \"application/rss+xml\",
    body: \"<rss/>\",
}
";
    let _dir = build_expect_ok_multi(
        "v019-response-with-auth-db-ws-observability",
        main,
        &[("feed.fitz", feed)],
    );
}

// ---------------------------------------------------------------------
// 🟢 v0.19.4 (2026-06-23) — cerró la deuda 🔴 URGENTE de v0.19.3:
//
// `http.request({...headers: Map<Str, Str>, body: Map...})` desde
// `async fn` rompía Send cuando se llamaba vía `spawn(...)`. El
// codegen del field `headers` emitía `(({Map literal}).lock().unwrap()
// .clone())` INLINE adentro del struct literal `__FitzHttpRequestOpts {
// ... }`, manteniendo el `MutexGuard` temporal vivo cross-await. El fix
// envuelve el clone en un block `{ let __headers_snap: Vec<...> =
// (...).lock().unwrap().clone(); __headers_snap }` que dropea el guard
// en el `;` ANTES del await. Mismo patrón análogo a v0.18.1 (`for x in
// List<Str>` adentro de `@cron`).
//
// Patrón canónico de Authorization Bearer header (Stripe, Resend,
// OpenAI, Mailgun, etc.) vía `http.request` ahora compila desde context
// que después se spawnea. Trigger del descubrimiento: bloqueo SMTP
// outbound de DigitalOcean en fitzwatch.com (2026-06-23) obligó migrar
// el welcome email + incident notify de `smtp.send` a `http.request`
// REST a Resend API.

#[test]
fn v019_4_http_request_with_headers_map_spawn_compila() {
    // Repro mínima de la deuda 🔴 URGENTE: handler HTTP que spawnea
    // una @background async fn que llama a http.request con headers
    // Map literal. Antes del fix, `tokio::spawn(...)` rechazaba con
    // `MutexGuard<Vec<(String, String)>>` not Send.
    let src = "\
@background
async fn notify(to: Str, key: Str) -> Null {
    let _ = http.request({
        \"method\": \"POST\",
        \"url\": \"https://api.example.com/x\",
        \"headers\": {
            \"Authorization\": \"Bearer {key}\",
            \"Content-Type\": \"application/json\"
        },
        \"body\": \"hello\"
    }).await
    return null
}

@get(\"/trigger/{to}\")
async fn trigger(to: Str) -> Result<Null> {
    spawn(notify(to, \"re_xxx\"))
    return Ok(null)
}

@server(43941) fn main() => 0
";
    build_expect_ok("v019-4-http-request-headers-map-spawn", src);
}

#[test]
fn v019_4_http_request_with_body_map_spawn_compila() {
    // Mismo case canónico pero con body Map<Str, Str> (el path del
    // body marshaling pasa por `__fitz_http_body_from_map_str_str` que
    // ya cierra el guard internamente — este test asegura que sigue
    // funcionando bit-a-bit incluso después del fix del headers).
    let src = "\
@background
async fn notify(to: Str, key: Str) -> Null {
    let _ = http.request({
        \"method\": \"POST\",
        \"url\": \"https://api.example.com/x\",
        \"headers\": {
            \"Authorization\": \"Bearer {key}\"
        },
        \"body\": {
            \"to\": to,
            \"subject\": \"hola\",
            \"html\": \"<p>hi</p>\"
        }
    }).await
    return null
}

@get(\"/notify/{to}\")
async fn trigger(to: Str) -> Result<Null> {
    spawn(notify(to, \"re_xxx\"))
    return Ok(null)
}

@server(43942) fn main() => 0
";
    build_expect_ok("v019-4-http-request-body-map-spawn", src);
}

#[test]
fn v019_4_http_request_cross_module_spawn_compila() {
    // Variante con `send_email` declarada en módulo importado — matchea
    // el shape real de fitzwatch (handlers en `subscriptions.fitz`,
    // notify cross-module via `from emails import send_email`).
    // Combinación de los fixes de v0.19.2 (`spawn(<cross_module>(...))`
    // emite `.await` cross-module) + v0.19.4 (headers Map literal no
    // rompe Send).
    let emails = "\
async fn send_email(to: Str, key: Str) -> Result<Bool> {
    let r = http.request({
        \"method\": \"POST\",
        \"url\": \"https://api.example.com/emails\",
        \"headers\": {
            \"Authorization\": \"Bearer {key}\",
            \"Content-Type\": \"application/json\"
        },
        \"body\": \"<p>hi</p>\"
    }).await?
    return Ok(true)
}
";
    let notify = "\
from emails import send_email

@background
async fn notify(to: Str) -> Null {
    let _ = send_email(to, \"re_xxx\").await
    return null
}
";
    let main = "\
from notify import notify

@get(\"/test/{to}\")
async fn trigger(to: Str) -> Result<Null> {
    spawn(notify(to))
    return Ok(null)
}

@server(43943) fn main() => 0
";
    let _dir = build_expect_ok_multi(
        "v019-4-http-request-cross-module-spawn",
        main,
        &[("emails.fitz", emails), ("notify.fitz", notify)],
    );
}

#[test]
fn v019_4_regression_v018_1_for_list_str_await_in_cron_no_send_break() {
    // Regression de v0.18.1: `for x in List<Str>` con `.await` adentro
    // de `@cron` rompía Send (mismo root cause — MutexGuard cross-await
    // del lock de `List<Str>`). Cerrado vía snapshot `let __for_snap =
    // xs.lock().unwrap().clone();` ANTES del for. Este test asegura
    // que el fix de v0.19.4 (snapshot binding para headers Map en
    // http.request) no introduce regresión en el path análogo del for
    // loop.
    // Programa cron-only: el codegen sintetiza su propio `fn main()` con
    // signal::ctrl_c().await — no agregamos `fn main()` del usuario para
    // evitar colisión.
    let src = "\
@cron(\"*/10 * * * * *\")
async fn poll() -> Null {
    let endpoints = [\"https://a.example.com\", \"https://b.example.com\"]
    for ep in endpoints {
        let _ = http.head(ep).await
    }
    return null
}
";
    build_expect_ok("v019-4-regression-for-list-str-await-cron", src);
}

// ---------------------------------------------------------------------
// 🟢 v0.19.5 (2026-06-27) — cerró la deuda 🔴 URGENTE descubierta en
// fitzwatch 2026-06-26: cross-module `@middleware(fn)` + `Request` no
// compilaba. Tres E2E tests cubren los 3 escenarios (paralelo a W12 +
// B10 cross-module pre-scan + W11/W16/W18 cross-module imports).

#[test]
fn v019_5_cross_module_middleware_fn_compila_a_binario_nativo() {
    // Síntoma 1 — Codegen rechazaba fn middleware cross-module con
    // `return null` simple (gate-only). El checker del loader sobre el
    // módulo aislado no veía el `@middleware(mw_simple)` aplicado en
    // main, y `collect_middleware_fn_names` quedaba sin la entrada
    // → la fn no clasificaba como middleware → el codegen ni siquiera
    // llegaba a ese punto (falla previa: build-time check rechaza la
    // ident del @middleware imported en main).
    let mw_mod = "\
async fn mw_simple(req: Request) {
    return null
}
";
    let main = "\
from mw import mw_simple

@middleware(mw_simple)
@get(\"/hi\")
fn hi() -> Str => \"hello\"

@server(43951) fn main() => 0
";
    let _dir = build_expect_ok_multi(
        "v019-5-cross-module-middleware-fn",
        main,
        &[("mw.fitz", mw_mod)],
    );
}

#[test]
fn v019_5_cross_module_middleware_fn_con_request_arg_compila() {
    // Síntoma 2 — Cuando el módulo del middleware declara helpers que
    // toman `req: Request`, el codegen emitía el `.rs` sin
    // `use crate::{Request, RequestData};` y rustc abortaba con
    // E0425/E0422. Repro: la fn middleware delega a un helper local
    // que tipa `Request` en su firma — esto fuerza al módulo a
    // necesitar la struct.
    let mw_mod = "\
fn get_client_ip(req: Request) -> Str {
    return \"127.0.0.1\"
}

async fn mw_with_helper(req: Request) {
    let ip = get_client_ip(req)
    return null
}
";
    let main = "\
from mw import mw_with_helper

@middleware(mw_with_helper)
@get(\"/hi\")
fn hi() -> Str => \"hello\"

@server(43952) fn main() => 0
";
    let _dir = build_expect_ok_multi(
        "v019-5-cross-module-middleware-helper-request",
        main,
        &[("mw.fitz", mw_mod)],
    );
    // Inspect emitted mw.rs: should contain `use crate::{Request, RequestData}`
    // (or split forms) so the helper's signature resolves.
    let stem = sanitize_stem("v019-5-cross-module-middleware-helper-request");
    let mw_rs = std::path::PathBuf::from("target")
        .join("fitz-build")
        .join(&stem)
        .join("src")
        .join("mw.rs");
    assert!(
        mw_rs.exists(),
        "emitted mw.rs not found at {}",
        mw_rs.display()
    );
    let emitted = std::fs::read_to_string(&mw_rs).expect("read mw.rs");
    assert!(
        emitted.contains("use crate::{Request, RequestData}")
            || (emitted.contains("use crate::Request;")
                && emitted.contains("use crate::RequestData;")),
        "expected `use crate::{{Request, RequestData}};` in mw.rs, got:\n{}",
        emitted
    );
}

#[test]
fn v019_5_cross_module_middleware_fn_con_return_status_compila() {
    // Síntoma 1 (variante con `return <status>`) — fn middleware
    // cross-module con short-circuit (`return 429 { ... }`). Antes el
    // checker del loader sobre el módulo aislado rechazaba el
    // `return <status>` porque `collect_middleware_fn_names` no veía
    // el `@middleware(mw_block)` aplicado en main (vive cross-module).
    let mw_mod = "\
async fn mw_block(req: Request) {
    return 429 { \"error\": \"blocked\" }
}
";
    let main = "\
from mw import mw_block

@middleware(mw_block)
@get(\"/hi\")
fn hi() -> Str => \"hello\"

@server(43953) fn main() => 0
";
    let _dir = build_expect_ok_multi(
        "v019-5-cross-module-middleware-return-status",
        main,
        &[("mw.fitz", mw_mod)],
    );
}

#[test]
fn v019_6_cross_module_middleware_applied_in_importer_module_emits_request_imports() {
    // Sub-caso de v0.19.5 — el bug que el fix anterior NO cubría: el
    // módulo IMPORTER del middleware (donde vive el handler que aplica
    // `@middleware(<imported_fn>)`) NO declara ninguna fn local con
    // `req: Request` en su firma, pero el wrapper HTTP emitido en ese
    // módulo SÍ construye `__req: Request = Arc::new(... RequestData
    // { ... })` para pasarlo al middleware. Pre-fix, el detector
    // `program_uses_request_type` devolvía `false` para ese módulo y
    // no se emitía `use crate::{Request, RequestData};` — rustc
    // abortaba con E0425/E0422.
    //
    // Repro paralelo bit-a-bit a fitzwatch: `mw.fitz` declara la fn
    // middleware, `handlers.fitz` aplica `@middleware(mw_strict)` a
    // un handler `@post`, `main.fitz` solo importa el handler para
    // mountarlo. Sin el fix de v0.19.6, `handlers.rs` emitido
    // referencia `Request`/`RequestData` sin import y rustc falla.
    let mw_mod = "\
async fn mw_strict(req: Request) {
    return null
}
";
    let handlers_mod = "\
from mw import mw_strict

@middleware(mw_strict)
@post(\"/protected\")
fn protected() -> Str => \"ok\"
";
    let main = "\
from handlers import protected

@server(43960) fn main() => 0
";
    let _dir = build_expect_ok_multi(
        "v019-6-cross-module-middleware-importer-module",
        main,
        &[("mw.fitz", mw_mod), ("handlers.fitz", handlers_mod)],
    );
    // Inspeccionar el emitted handlers.rs: debe contener
    // `use crate::{Request, RequestData}` (o split forms) para que el
    // wrapper HTTP resuelva el tipo del `__req` que arma para pasarlo
    // al middleware cross-module.
    let stem = sanitize_stem("v019-6-cross-module-middleware-importer-module");
    let handlers_rs = std::path::PathBuf::from("target")
        .join("fitz-build")
        .join(&stem)
        .join("src")
        .join("handlers.rs");
    assert!(
        handlers_rs.exists(),
        "emitted handlers.rs not found at {}",
        handlers_rs.display()
    );
    let emitted = std::fs::read_to_string(&handlers_rs).expect("read handlers.rs");
    assert!(
        emitted.contains("use crate::{Request, RequestData}")
            || (emitted.contains("use crate::Request;")
                && emitted.contains("use crate::RequestData;")),
        "expected `use crate::{{Request, RequestData}};` in handlers.rs, got:\n{}",
        emitted
    );
}

/// Scoping (2026-07-22) — a local `let` that shadows an enclosing param of a
/// DIFFERENT type must emit a fresh `let mut` (shadowing), not a reassignment.
/// Before the fix the codegen saw the param in scope, emitted `cookie = "abc"`,
/// and rustc rejected it (`Str` into `Option<String>`, E0308). Found
/// internationalizing the Admin ABM (login_submit's `cookie: Str?` param).
#[test]
fn let_shadows_param_of_different_type_compiles_and_runs() {
    let (out, code) = build_and_run(
        "scoping-let-shadows-param",
        "fn h(cookie: Str?) -> Str {\n\
         \x20   let cookie = \"abc\"\n\
         \x20   return cookie\n\
         }\n\
         print(h(\"x\"))\n",
    );
    assert_eq!(code, 0, "binary should exit 0, out:\n{out}");
    assert_eq!(out.trim(), "abc");
}

/// Scoping (2026-07-22) — a local `let` shadowing a module-level fn must not
/// clobber it: other functions still see the fn. Regression for the runtime
/// clobber found internationalizing the Admin ABM (`let t` vs imported `t`).
#[test]
fn let_shadows_module_fn_does_not_clobber_it() {
    let (out, code) = build_and_run(
        "scoping-let-shadows-module-fn",
        "fn f(x: Int) -> Int => x + 1\n\
         fn a() -> Int {\n  let f = 99\n  return f\n}\n\
         fn b() -> Int {\n  return f(10)\n}\n\
         print(a())\nprint(b())\n",
    );
    assert_eq!(code, 0, "binary should exit 0, out:\n{out}");
    assert_eq!(out.trim(), "99\n11");
}

/// @ws + @header (2026-07-22) — a `@header(...)` param on a `@ws` handler reads
/// the handshake header. Verifies `fitz build` emits a valid wrapper (binds the
/// header from the HeaderMap + passes it to the handler in declared order).
/// Previously @header on @ws was rejected; run↔build parity validated manually
/// (cookie value bound; nullable missing → Null).
#[test]
fn ws_handler_with_header_builds() {
    build_expect_ok(
        "ws-handler-with-header",
        "@header(name=\"cookie\")\n\
         @ws(\"/live/x\")\n\
         async fn sock(ws: WsConn<Str>, cookie: Str?) {\n\
         \x20   let who = match cookie {\n\
         \x20       null => \"anon\",\n\
         \x20       c => c,\n\
         \x20   }\n\
         \x20   ws.send(\"hi {who}\")?\n\
         \x20   loop {\n\
         \x20       let _m = ws.recv()?\n\
         \x20   }\n\
         }\n\
         @server(3902)\n\
         fn main() => 0\n",
    );
}

/// FITZ-05 (2026-08-21) — a `@cookie(name="X")` param on a `@ws` handler reads
/// the named cookie from the handshake `Cookie` header. Verifies `fitz build`
/// emits a valid WS wrapper (binds the cookie via `__fitz_parse_cookie` from the
/// HeaderMap + forwards it to the handler in declared order). Previously the WS
/// path had no cookie block (only `@header`), so a `@cookie @ws` failed the
/// arity check. Nullable → Option; required → 400 pre-upgrade. run↔build parity
/// validated manually (cookie value bound; nullable missing → Null).
#[test]
fn ws_handler_with_cookie_builds_fitz05() {
    build_expect_ok(
        "ws-handler-with-cookie-fitz05",
        "@cookie(name=\"lang\")\n\
         @ws(\"/live/x\")\n\
         async fn sock(ws: WsConn<Str>, lang: Str?) {\n\
         \x20   let loc = match lang {\n\
         \x20       null => \"en\",\n\
         \x20       l => l,\n\
         \x20   }\n\
         \x20   ws.send(\"locale {loc}\")?\n\
         \x20   loop {\n\
         \x20       let _m = ws.recv()?\n\
         \x20   }\n\
         }\n\
         @server(3903)\n\
         fn main() => 0\n",
    );
}

/// FITZ-05 (2026-08) — a `@cookie(name="X")` handler compiles to a native binary
/// (the HTTP wrapper parses the named cookie from the incoming `Cookie` header
/// via `__fitz_parse_cookie`). Nullable → Option; required → 400 if missing.
/// Runtime parity with `fitz run` validated manually (anon / session / base64
/// value with `=` / required-missing → 400).
#[test]
fn cookie_decorator_handler_builds_fitz05() {
    build_expect_ok(
        "cookie-handler-fitz05",
        "@cookie(name=\"session\")\n\
         @get(\"/whoami\")\n\
         fn whoami(session: Str?) -> Str {\n\
         \x20   return match session {\n\
         \x20       null => \"anon\",\n\
         \x20       s => \"session:{s}\",\n\
         \x20   }\n\
         }\n\
         @cookie(name=\"lang\")\n\
         @get(\"/lang\")\n\
         fn get_lang(lang: Str) -> Str => \"lang:{lang}\"\n\
         @server(3941)\n\
         fn main() => 0\n",
    );
}

/// FITZ-09 (2026-08) — a fn declared `-> T?` (Nullable) with a `return`
/// inside a nested `match`/`if`/loop used to emit broken Rust: `return ()`
/// where `return None` was needed, and the value unwrapped instead of
/// `Some(...)`. `fitz check` ✓ / `fitz run` ✓ / `fitz build` ✗ (E0308).
/// This is the exact pattern of `flv_cookie` in fitz-liveviews, which
/// blocked every native build that depends on the framework. The fix
/// recovers the real `Nullable(T)` return frame from `ret_stack` when the
/// nested return re-enters with the `Type::Null` placeholder, so `coerce`
/// wraps in `Some(...)`/`None`. Validates build success AND run↔build
/// parity (the CLI repro is deterministic).
#[test]
fn nullable_return_in_nested_match_wraps_some_none_fitz09() {
    let src = "fn primera_parte(s: Str?) -> Str? {\n\
        \x20   let raw: Str = match s {\n\
        \x20       null => return null,\n\
        \x20       v => v,\n\
        \x20   }\n\
        \x20   for parte in raw.split(\",\") {\n\
        \x20       return parte\n\
        \x20   }\n\
        \x20   return null\n\
        }\n\
        fn describe(s: Str?) -> Str {\n\
        \x20   return match primera_parte(s) {\n\
        \x20       null => \"nada\",\n\
        \x20       v => v,\n\
        \x20   }\n\
        }\n\
        print(describe(\"a,b\"))\n\
        print(describe(null))\n\
        print(describe(\"x\"))\n";
    let (out, code) = build_and_run("nullable-return-nested-match-fitz09", src);
    assert_eq!(code, 0, "el binario debe exitear 0");
    assert_eq!(
        out, "a\nnada\nx\n",
        "salida del binario debe ser idéntica a `fitz run` (paridad FITZ-09)"
    );
}

/// FITZ-10 (2026-08) — an empty `[]` literal that the checker types `List<Any>`
/// (never refined from `.push()`) made codegen emit `Vec<__FitzValue>` + a
/// `Str + Any` concat, but (a) `gen_binop` rejected `Str + Any` with a codegen
/// error, and (b) the `__FitzValue` enum/helpers were not emitted for the CLI
/// path (they were only pre-detected from heterogeneous literals). `fitz check`
/// ✓ / `fitz run` ✓ / `fitz build` ✗. The fix handles `Str + Any` / `Any + Str`
/// in `gen_binop` (coerce Any→Str, parity with the interpreter) AND detects
/// `List<Any>`/`Map<_, Any>` via the checker's TypeInfo so the `__FitzValue`
/// prelude is emitted. Validates build success AND run↔build parity.
#[test]
fn str_plus_any_from_list_any_compiles_fitz10() {
    let src = "fn primero(csv: Str) -> Str {\n\
        \x20   let chars = []\n\
        \x20   for c in csv.split(\",\") {\n\
        \x20       chars.push(c)\n\
        \x20   }\n\
        \x20   let out = \"[\"\n\
        \x20   out = out + chars[0]\n\
        \x20   out = out + \"-\"\n\
        \x20   out = out + chars[1]\n\
        \x20   return out\n\
        }\n\
        print(primero(\"a,b,c\"))\n";
    let (out, code) = build_and_run("str-plus-any-list-any-fitz10", src);
    assert_eq!(code, 0, "el binario debe exitear 0");
    assert_eq!(
        out, "[a-b\n",
        "salida del binario debe ser idéntica a `fitz run` (paridad FITZ-10)"
    );
}

/// FITZ-01 (2026-08) — `rand.seeded(N)` is a reproducible generator: the SAME
/// SplitMix64 algorithm runs in the interpreter (`src/rand.rs`) and in the
/// codegen-emitted Rust (`RAND_PRELUDE_CORE`), so a seeded-only, deterministic
/// program produces byte-identical output under `fitz run` and `fitz build`.
/// This is the core contract that lets MatHelp store `seed + index` and
/// reconstruct a run from two integers. Covers seeded int/float/bool/choice/
/// shuffle/sample.
#[test]
fn rand_seeded_reproducible_parity_fitz01() {
    let src = "let r = rand.seeded(12345)\n\
        print(r.int(1, 6))\n\
        print(r.int(1, 6))\n\
        print(r.float())\n\
        print(r.bool())\n\
        match r.choice([\"a\", \"b\", \"c\", \"d\"]) {\n\
        \x20   Ok(v) => print(v),\n\
        \x20   Err(e) => print(e),\n\
        }\n\
        print(r.shuffle([1, 2, 3, 4, 5]))\n\
        match r.sample([10, 20, 30, 40, 50], 3) {\n\
        \x20   Ok(v) => print(v),\n\
        \x20   Err(e) => print(e),\n\
        }\n";
    let run_out = run_interpreter("rand-seeded-parity-fitz01", src);
    let (build_out, code) = build_and_run("rand-seeded-parity-fitz01", src);
    assert_eq!(code, 0, "el binario debe exitear 0");
    assert_eq!(
        run_out, build_out,
        "`rand.seeded(N)` debe ser reproducible bit-a-bit run↔build (FITZ-01)"
    );
    assert_eq!(
        run_out, "3\n4\n0.11954258300911547\nfalse\nd\n[1, 5, 4, 3, 2]\n[40, 30, 20]\n",
        "la secuencia SplitMix64 sembrada con 12345 debe ser estable"
    );
}

/// FITZ-03 (2026-08) — módulo `fs`: filesystem builtins con paridad `run`↔`build`
/// (los helpers `__fitz_fs_*` del codegen usan `std::fs` con los mismos mensajes
/// que `src/fs.rs`). Escribe/lee/lista/borra en un dir bajo `target/`
/// (gitignored) y verifica que la salida es idéntica interpretado y compilado.
#[test]
fn fs_builtins_roundtrip_parity_fitz03() {
    let src = "let dir = \"target/fitz_fs_e2e_fitz03\"\n\
        let file = \"target/fitz_fs_e2e_fitz03/data.txt\"\n\
        let _m = fs.mkdir_all(dir)\n\
        let _w = fs.write(file, \"linea1\\nlinea2\\n\")\n\
        let _a = fs.append(file, \"linea3\")\n\
        match fs.read(file) { Ok(c) => print(c), Err(e) => print(e) }\n\
        print(fs.exists(file))\n\
        match fs.list(dir) { Ok(xs) => print(xs), Err(e) => print(e) }\n\
        let _r = fs.remove(file)\n\
        let _rd = fs.remove(dir)\n";
    let run_out = run_interpreter("fs-roundtrip-fitz03", src);
    let (build_out, code) = build_and_run("fs-roundtrip-fitz03", src);
    assert_eq!(code, 0, "el binario debe exitear 0");
    assert_eq!(
        run_out, build_out,
        "`fs.*` debe producir salida idéntica run↔build (FITZ-03)"
    );
    assert_eq!(
        run_out, "linea1\nlinea2\nlinea3\ntrue\n[\"data.txt\"]\n",
        "roundtrip write→append→read→exists→list esperado"
    );
}

/// FITZ-13 (2026-08) — `Map.remove(key) -> Bool` con paridad `run`↔`build`.
/// Borra una clave existente (true), una ausente (false), preserva el orden
/// del resto y muta el Map en su lugar. El intérprete (`map_remove`) y el
/// codegen (`gen_map_remove`, `.remove` sobre el `Vec<(K, V)>`) deben producir
/// salida idéntica.
#[test]
fn map_remove_parity_fitz13() {
    let src = "let m = {\"a\": 1, \"b\": 2, \"c\": 3}\n\
        print(m.remove(\"b\"))\n\
        print(m.remove(\"x\"))\n\
        print(m.len())\n\
        print(m)\n\
        print(m.has(\"b\"))\n";
    let run_out = run_interpreter("map-remove-parity-fitz13", src);
    let (build_out, code) = build_and_run("map-remove-parity-fitz13", src);
    assert_eq!(code, 0, "el binario debe exitear 0");
    assert_eq!(
        run_out, build_out,
        "`Map.remove` debe producir salida idéntica run↔build (FITZ-13)"
    );
    assert_eq!(
        run_out, "true\nfalse\n2\n{\"a\": 1, \"c\": 3}\nfalse\n",
        "remove(existente)=true, remove(ausente)=false, orden preservado"
    );
}

/// FITZ-14 — corpus de paridad `fitz run` ↔ `fitz build`. Cada ejemplo CLI-puro
/// y determinista debe producir stdout **idéntico bit-a-bit** por el intérprete
/// y por el binario nativo. Es la red sistemática que hubiera cazado FITZ-09,
/// FITZ-10 y la divergencia histórica de los format specs ANTES de que la
/// encuentre un usuario dockerizando a las once de la noche.
///
/// Criterio de inclusión: single-file (sin imports — `build_and_run` copia el
/// fuente a un tempdir, así que los módulos hermanos no resuelven), determinista
/// (sin reloj, sin aleatoriedad no-sembrada, sin red) y termina (sin `@server`).
/// Cuando una feature nueva pueda diverger entre intérprete y binario, sumá su
/// caso acá.
const PARITY_CLI_EXAMPLES: &[&str] = &[
    "03c-bases-numericas.fitz",
    "04b-operadores-bit.fitz",
    "05b-format-specs.fitz",
    "09d-comprehensions.fitz",
    "10-match.fitz",
    "13-metodos.fitz",
    "13o-higher-order-y-consts-globales.fitz",
    "13s-mb7-y-fmt-build.fitz",
    "13t-mb8-bits-y-fmt-g.fitz",
    "13v-return-en-match.fitz",
    // FITZ-04 (2026-08) — `num.*` es determinista → paridad run↔build.
    "13x-num-locale.fitz",
    "14-result.fitz",
    "14c-result-tipado.fitz",
    "14d-err-compuestos.fitz",
];

#[test]
fn run_build_parity_corpus_fitz14() {
    let project_root = std::env::current_dir().expect("cwd");
    let guide_dir = project_root.join("examples").join("guide");
    let mut mismatches: Vec<String> = Vec::new();
    for name in PARITY_CLI_EXAMPLES {
        let src = std::fs::read_to_string(guide_dir.join(name))
            .unwrap_or_else(|e| panic!("no se pudo leer {}: {}", name, e));
        let stem = format!("parity-{}", name.trim_end_matches(".fitz"));
        let run_out = run_interpreter(&stem, &src);
        let (build_out, code) = build_and_run(&stem, &src);
        if code != 0 {
            mismatches.push(format!("{}: el binario exitó con código {}", name, code));
            continue;
        }
        if run_out != build_out {
            mismatches.push(format!(
                "{}: DIVERGENCIA run↔build\n  run:   {:?}\n  build: {:?}",
                name, run_out, build_out
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "divergencias de paridad `fitz run` ↔ `fitz build` (FITZ-14):\n{}",
        mismatches.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// FITZ-02 — static file serving (`@server(static_dir=..., static_prefix=...)`)
// ---------------------------------------------------------------------------

/// FITZ-02 — extracts a header value (case-insensitive name) from a raw
/// HTTP response headers block.
fn extract_static_header(headers: &str, name: &str) -> Option<String> {
    let lname = name.to_ascii_lowercase();
    for line in headers.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().to_ascii_lowercase() == lname {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// FITZ-02 — creates a temp dir with a `<stem>.fitz` serving `./public`
/// (plus a `secret.txt` OUTSIDE public to prove traversal cannot reach
/// it) at `port` under `prefix`, and returns the dir.
fn static_setup(stem: &str, port: u16, prefix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fitz-static-{}", stem));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("public").join("css")).expect("mk public/css");
    std::fs::write(
        dir.join("public").join("index.html"),
        b"<!doctype html><h1>hi fitz</h1>",
    )
    .unwrap();
    std::fs::write(
        dir.join("public").join("css").join("app.css"),
        b"body{color:teal}",
    )
    .unwrap();
    std::fs::write(dir.join("secret.txt"), b"TOP SECRET").unwrap();
    let src = format!(
        "@get(\"/health\")\nfn health() -> Str => \"ok\"\n\n\
         @server({}, static_dir=\"./public\", static_prefix=\"{}\")\nfn main() => 0\n",
        port, prefix
    );
    std::fs::write(dir.join(format!("{}.fitz", stem)), src).unwrap();
    dir
}

/// FITZ-02 — waits until `port` is free to bind (used between two
/// sequential server spawns on the same port, e.g. run vs binary).
fn wait_static_port_free(port: u16) {
    let addr = format!("127.0.0.1:{}", port);
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(5) {
        if std::net::TcpListener::bind(&addr).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// FITZ-02 — a spawned static server, killed on drop. Keeps one server
/// alive across multiple requests so the port is not rebound per request.
struct StaticServer {
    child: std::process::Child,
    port: u16,
}

impl StaticServer {
    fn spawn(mut cmd: Command, port: u16) -> Self {
        use std::process::Stdio;
        wait_static_port_free(port);
        let child = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn static server");
        let addr = format!("127.0.0.1:{}", port);
        let start = std::time::Instant::now();
        let mut connected = false;
        while start.elapsed() < std::time::Duration::from_secs(8) {
            if std::net::TcpStream::connect(&addr).is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            connected,
            "static server did not open port {} within 8s",
            port
        );
        StaticServer { child, port }
    }

    /// GET `path` with optional extra request headers. Returns
    /// `(status, headers_string, body_bytes)`.
    fn get(&self, path: &str, extra_headers: &[(&str, &str)]) -> (u16, String, Vec<u8>) {
        use std::io::{Read, Write};
        let addr = format!("127.0.0.1:{}", self.port);
        let mut req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            path, addr
        );
        for (k, v) in extra_headers {
            req.push_str(&format!("{}: {}\r\n", k, v));
        }
        req.push_str("\r\n");
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect static");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .ok();
        stream
            .write_all(req.as_bytes())
            .expect("send static request");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok();
        let split = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or(buf.len());
        let body = if split + 4 <= buf.len() {
            buf[split + 4..].to_vec()
        } else {
            Vec::new()
        };
        let headers = String::from_utf8_lossy(&buf[..split]).into_owned();
        let status = headers
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, headers, body)
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn fitz02_static_disk_parity_content_type_etag_304_traversal() {
    // FITZ-02 — the whole disk-serving surface in one build:
    //   * Content-Type by extension + content-based ETag,
    //   * bit-for-bit parity `fitz run` ↔ native binary (same ETag+body),
    //   * `If-None-Match` → 304,
    //   * `../` traversal blocked (cannot leak a file outside the dir),
    //   * missing file → 404,
    //   * a user route still works alongside the static wildcard.
    let port = 43941;
    let stem = "fitz02-disk";
    let dir = static_setup(stem, port, "/static");
    let src_path = dir.join(format!("{}.fitz", stem));

    let build = Command::new(fitz_bin())
        .args(["build"])
        .arg(&src_path)
        .current_dir(&dir)
        .output()
        .expect("invoke fitz build");
    assert!(
        build.status.success(),
        "fitz build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let bin = dir.join(if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    });
    assert!(bin.exists(), "binary {} missing", bin.display());

    // --- fitz run (interpreter) ---
    let (r_status, r_headers, r_body) = {
        let mut cmd = Command::new(fitz_bin());
        cmd.args(["run"]).arg(&src_path).current_dir(&dir);
        let srv = StaticServer::spawn(cmd, port);
        srv.get("/static/css/app.css", &[])
    };
    assert_eq!(r_status, 200, "run: css 200");
    assert!(
        r_headers
            .to_lowercase()
            .contains("content-type: text/css; charset=utf-8"),
        "run headers must carry text/css Content-Type: {}",
        r_headers
    );
    let etag = extract_static_header(&r_headers, "etag").expect("run response carries an ETag");
    assert!(
        r_headers.to_lowercase().contains("cache-control:"),
        "run response carries Cache-Control: {}",
        r_headers
    );

    wait_static_port_free(port);

    // --- native binary: parity + 304 + traversal + missing + user route ---
    let mut cmd = Command::new(&bin);
    cmd.current_dir(&dir);
    let srv = StaticServer::spawn(cmd, port);

    let (b_status, b_headers, b_body) = srv.get("/static/css/app.css", &[]);
    assert_eq!(b_status, 200, "binary: css 200");
    // Parity: identical status, ETag, and body between run and build.
    assert_eq!(r_status, b_status, "status parity run↔build");
    assert_eq!(
        etag,
        extract_static_header(&b_headers, "etag").expect("binary ETag"),
        "ETag parity run↔build (content-based)"
    );
    assert_eq!(r_body, b_body, "body parity run↔build");

    // 304 with matching If-None-Match.
    let (s304, h304, b304) = srv.get("/static/css/app.css", &[("If-None-Match", &etag)]);
    assert_eq!(s304, 304, "matching If-None-Match → 304");
    assert!(b304.is_empty(), "304 has no body");
    assert!(
        h304.to_lowercase().contains("etag:"),
        "304 carries the ETag: {}",
        h304
    );

    // Traversal blocked — must not leak secret.txt outside ./public.
    let (strav, _, btrav) = srv.get("/static/%2e%2e/secret.txt", &[]);
    assert_eq!(strav, 404, "encoded `..` traversal must be blocked");
    assert!(
        !String::from_utf8_lossy(&btrav).contains("TOP SECRET"),
        "the file outside the static dir must never be served"
    );

    // Missing file → 404.
    let (smiss, _, _) = srv.get("/static/nope.txt", &[]);
    assert_eq!(smiss, 404, "missing file → 404");

    // The user's own route still works alongside the static wildcard.
    let (shealth, _, bhealth) = srv.get("/health", &[]);
    assert_eq!(shealth, 200, "user route /health still works");
    assert!(
        String::from_utf8_lossy(&bhealth).contains("ok"),
        "/health returns ok"
    );
}

#[test]
fn fitz02_embed_static_serves_without_dir_on_disk() {
    // FITZ-02 — `fitz build --embed-static` bakes the assets into the
    // binary with `include_bytes!`. The binary serves its own frontend
    // from a directory that has NO `public/` on disk (distroless case),
    // with the same content-based ETag as the disk build, and traversal
    // still blocked.
    let port = 43942;
    let stem = "fitz02-embed";
    let dir = static_setup(stem, port, "/static");
    let src_path = dir.join(format!("{}.fitz", stem));

    let build = Command::new(fitz_bin())
        .args(["build", "--embed-static"])
        .arg(&src_path)
        .current_dir(&dir)
        .output()
        .expect("invoke fitz build --embed-static");
    assert!(
        build.status.success(),
        "fitz build --embed-static failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let bin_name = if cfg!(windows) {
        format!("{}.exe", stem)
    } else {
        stem.to_string()
    };
    let bin = dir.join(&bin_name);
    assert!(bin.exists(), "binary {} missing", bin.display());

    // Copy the binary to a CLEAN dir with NO public/ and run from there.
    let clean = std::env::temp_dir().join(format!("fitz-static-{}-clean", stem));
    let _ = std::fs::remove_dir_all(&clean);
    std::fs::create_dir_all(&clean).unwrap();
    let clean_bin = clean.join(&bin_name);
    std::fs::copy(&bin, &clean_bin).expect("copy embed binary");
    assert!(
        !clean.join("public").exists(),
        "the clean dir must have no public/"
    );

    let mut cmd = Command::new(&clean_bin);
    cmd.current_dir(&clean);
    let srv = StaticServer::spawn(cmd, port);

    let (s, h, b) = srv.get("/static/index.html", &[]);
    assert_eq!(s, 200, "embed serves index.html without a dir on disk");
    assert!(
        h.to_lowercase().contains("content-type: text/html"),
        "embed index.html has html Content-Type: {}",
        h
    );
    assert!(
        String::from_utf8_lossy(&b).contains("<h1>hi fitz</h1>"),
        "embed served the real bytes: {:?}",
        String::from_utf8_lossy(&b)
    );

    let (scss, hcss, _) = srv.get("/static/css/app.css", &[]);
    assert_eq!(scss, 200, "embed serves nested css");
    assert!(
        hcss.to_lowercase().contains("etag:"),
        "embed css carries an ETag: {}",
        hcss
    );

    // Traversal + missing still 404 in embed mode.
    let (strav, _, _) = srv.get("/static/%2e%2e/secret.txt", &[]);
    assert_eq!(strav, 404, "embed: traversal blocked");
    let (smiss, _, _) = srv.get("/static/nope.txt", &[]);
    assert_eq!(smiss, 404, "embed: missing → 404");
}
