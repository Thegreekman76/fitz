# M7.C1 — Distribución avanzada: binarios standalone y cross-compile

**Pre-requisitos**: [M6.C6 — capstone CRUD completo](../m6-postgres-orm/c6-capstone-crud-completo.md).
Tenés una app real cerrada (auth + ORM + WS + cron + Docker). Ahora vamos
a sacarla del laptop y meterla en otras plataformas, con binarios que
funcionen **sin instalar Fitz ni Rust ni Python en el destino**.

**Objetivo**: producir binarios standalone Linux/macOS/Windows desde
tu máquina sin instalar toolchains extras, opcionalmente bundlear
CPython y paquetes pip adentro del binario, y entender qué se incluye
en el binario emitido por `fitz build`.

**Por qué importa**: el diferencial de Fitz es que el binario es
**totalmente standalone**. No hay runtime de Fitz, no hay libpython
linkeada externa (salvo si usás interop Python), no hay `node_modules`
ni `__pycache__`. Copiás el binario al server, lo corrés, listo.

**Cross-link**: [cap 20 de la guía](../../guide.md#20-fitz-build--compilar-a-binario-nativo) y
[cap 21.11 (`--bundle-python`)](../../guide.md#2111-fitz-build---bundle-python--binario-standalone).

---

## Por qué Fitz es distinto

| Feature                       | Python    | TypeScript | Go    | Rust  | **Fitz** |
| ----------------------------- | --------- | ---------- | ----- | ----- | -------- |
| Binario standalone            | ❌ (req. Python) | ❌ (req. Node) | ✅ | ✅ | ✅ |
| Cross-compile sin toolchain   | ❌        | ❌         | ✅    | ✅ con `cross` | ✅ **gratis via rustc targets** |
| Sin runtime en destino        | ❌        | ❌         | ✅    | ✅    | ✅ |
| Embebido CPython opcional     | N/A       | N/A        | ❌    | ⚠ PyO3 manual | ✅ **`--bundle-python`** |
| Embebido pip packages opcional| N/A       | N/A        | ❌    | ❌    | ✅ **`--bundle-pip`** |
| Tamaño típico (CLI simple)    | ~50 MB (con Python) | ~80 MB (pkg) | ~5 MB | ~3 MB | ~3-5 MB |
| Tamaño con HTTP + axum        | N/A       | N/A        | ~12 MB | ~8 MB | ~8-10 MB |

Lo que más diferencia a Fitz: **un mismo binario `fitz` cross-compila
a 4 plataformas** (Windows x64, Linux x64, Linux ARM64, macOS Apple
Silicon) **sin instalar nada extra** porque usa los targets que
`rustc` ya conoce.

---

## Paso 1 — Buildear para tu plataforma actual

Desde adentro de tu proyecto (con `fitz.toml`):

```bash
$ fitz build
   Compiling app v0.1.0 (target\fitz-build\app)
    Finished `release` profile [optimized] target(s) in 38.42s
✓ binario: target/release/app
```

El binario sale en `target/release/<package.name>`. Inspeccioná el
tamaño:

```bash
$ ls -lh target/release/app
-rwxr-xr-x 1 user user 8.4M Jun  3 18:42 target/release/app
```

8.4 MB para una app con HTTP nativo + auth + JWT + Argon2. Comparalo
con un proyecto FastAPI equivalente (Python 3.12 + dependencies =
~50 MB sin contar libpython).

**Probá que funciona standalone**:

```bash
$ ./target/release/app
{"timestamp":"...","level":"INFO","msg":"server listo"}

# Desde otro shell:
$ curl localhost:3000
{"app":"demo","version":"v1.0"}
```

Funciona sin que tengas Fitz, Rust, ni nada en el PATH. Es un binario
puro.

---

## Paso 2 — Cross-compile a otras plataformas

`fitz build` usa rustc por debajo. rustc viene con muchos targets
pre-cargados. Listalos:

```bash
$ rustc --print target-list | head -10
aarch64-apple-darwin
aarch64-unknown-linux-gnu
x86_64-apple-darwin
x86_64-pc-windows-msvc
x86_64-unknown-linux-gnu
...
```

Para cross-compilear, agregás el target al `fitz build` y rustc se
ocupa. **Importante**: para targets que NO son tu plataforma actual,
necesitás `rustup` para descargar el toolchain primero.

```bash
$ rustup target add aarch64-unknown-linux-gnu
info: downloading component 'rust-std' for 'aarch64-unknown-linux-gnu'
info: installing component 'rust-std' for 'aarch64-unknown-linux-gnu'

$ fitz build --target aarch64-unknown-linux-gnu
✓ binario: target/aarch64-unknown-linux-gnu/release/app
```

> **Nota**: para algunos targets exóticos (musl, embedded) podés
> necesitar un linker cross-platform (`lld`, `gcc-aarch64-linux-gnu`).
> El error de `fitz build` te lo dice claramente.

**Patrón de release multi-platform** desde una sola máquina:

```bash
for target in \
    x86_64-pc-windows-msvc \
    x86_64-unknown-linux-gnu \
    aarch64-unknown-linux-gnu \
    aarch64-apple-darwin
do
    rustup target add $target
    fitz build --target $target
done
```

Esto produce 4 binarios en `target/<triple>/release/<name>` listos
para distribución.

---

## Paso 3 — Bundlear CPython para apps con interop Python

Si tu app usa `from python import X`, el binario necesita
`libpython3.X.so` en el destino. Para evitar eso (deployment más
simple, distroless container), `fitz build --bundle-python` empaqueta
CPython adentro del binario:

```bash
$ fitz build --bundle-python
   Descargando CPython 3.14 standalone... (~30 MB)
   Empacando launcher + python-build-standalone tarball
✓ binario: target/release/app
```

El binario salta de ~10 MB a ~40 MB (CPython base + stdlib). Al
ejecutarlo, el launcher extrae CPython a un dir temporal del usuario
(`%LOCALAPPDATA%\fitz-bundles\` en Windows / `~/.cache/fitz-bundles/`
en Linux) y arranca tu app contra ese intérprete embebido. Es
**transparente**: tu app no nota la diferencia.

**Cuándo usar `--bundle-python`**:

- Cuando vas a deployar a un container distroless (sin Python).
- Cuando querés un binario único que el usuario corre sin instalar
  nada.
- Cuando el sistema destino tiene una versión de Python distinta a la
  que necesita tu programa (matching de ABI).

**Cuándo NO usar `--bundle-python`**:

- Si el destino ya tiene Python instalado con tus paquetes (~30 MB
  ahorrados).
- Si tu app NO usa interop Python (es redundante).

---

## Paso 4 — Bundlear paquetes pip

`--bundle-python` empaqueta CPython base + stdlib. **No** empaqueta
los paquetes que instalaste con pip. Para eso está `--bundle-pip`:

```bash
$ fitz build --bundle-python --bundle-pip sqlalchemy --bundle-pip psycopg2-binary
   Empacando CPython 3.14 + stdlib
   Instalando paquetes pip en /tmp/fitz-pip-XYZ
   Empacando 3 paquetes pip (sqlalchemy + psycopg2 + sqlalchemy-utils)
✓ binario: target/release/app  (95 MB)
```

O lee desde un `requirements.txt` estándar:

```bash
$ fitz build --bundle-python --bundle-pip-requirements requirements.txt
```

El binario resultante incluye CPython + stdlib + tus paquetes pip
+ tu app Fitz. Deploy: copiar UN solo archivo al server. Sin
`pip install`, sin venv, sin matching de versiones.

> **Trade-off**: cada paquete pip suma ~5-50 MB al binario. Una app
> con sqlalchemy + pandas puede llegar a 200-300 MB. Es el precio
> del "binario único". Si te importa el tamaño y aceptás el
> requerimiento de "Python instalado en destino", usá runtime
> images Python en lugar de bundlear.

---

## Paso 5 — Inspeccionar el binario

Para ver qué está adentro del binario producido por `fitz build`:

```bash
$ file target/release/app
target/release/app: ELF 64-bit LSB pie executable, x86-64, version 1 (SYSV), dynamically linked

$ ldd target/release/app
        linux-vdso.so.1 (0x00007ffd...)
        libgcc_s.so.1 => /lib/x86_64-linux-gnu/libgcc_s.so.1
        libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6
        /lib64/ld-linux-x86-64.so.2

$ strip target/release/app  # opcional: remueve símbolos de debug
$ ls -lh target/release/app
-rwxr-xr-x 1 user user 6.1M Jun  3 18:42 target/release/app  # 25% más chico
```

El binario depende **sólo** de `libc` y `libgcc_s` (presentes en
TODO Linux moderno). No depende de Fitz, Rust, ni nada custom.

Para Windows:

```powershell
PS> dumpbin /dependents target\release\app.exe
File Type: EXECUTABLE IMAGE
  Image has the following dependencies:
    KERNEL32.dll
    ADVAPI32.dll
    WS2_32.dll  # Solo si la app usa HTTP
    USERENV.dll
```

Solo DLLs estándar de Windows. Cero deps externas.

---

## Paso 6 — Optimizar el binario para producción

Por default `fitz build` usa `--release` (optimizaciones rustc full).
Si querés más reducción de tamaño:

1. **Strip symbols** (mostrado arriba): `strip <binary>` quita info de
   debug. ~20-30% de reducción.

2. **LTO (Link Time Optimization)** y **opt-level=z**: editá el
   `Cargo.toml` que Fitz emite en `target/fitz-build/<name>/` y
   agregá:

   ```toml
   [profile.release]
   lto = true
   codegen-units = 1
   opt-level = "z"
   strip = true
   ```

   Re-buildeá y vas a ver entre 10-25% menos de tamaño, a costa de
   tiempos de compilación más largos.

3. **UPX compression** (cuidado): `upx --best <binary>` comprime el
   ejecutable. Reducción del 50-70% pero al ejecutar el binario,
   tiene que descomprimirse en RAM. NO uses en containers — algunas
   herramientas antivirus marcan UPX como sospechoso. Útil para
   distribuir CLI tools por download directo.

---

## Validación del cap

Probá end-to-end:

```bash
# 1. Buildeá tu plataforma actual.
$ fitz build
$ ./target/release/<name>  # debería arrancar el server.

# 2. Si querés multi-platform, agregá un target nuevo.
$ rustup target add aarch64-unknown-linux-gnu
$ fitz build --target aarch64-unknown-linux-gnu
$ ls target/aarch64-unknown-linux-gnu/release/

# 3. Si usás interop Python, bundleá CPython.
$ fitz build --bundle-python
$ ls -lh target/release/<name>  # debería ser ~30 MB más grande.

# 4. Inspeccioná el binario para confirmar que es standalone.
$ ldd target/release/<name>  # Linux
$ otool -L target/release/<name>  # macOS
$ dumpbin /dependents target\release\<name>.exe  # Windows
```

Si las 4 funcionan, dominaste distribución avanzada.

---

## Próximo: M7.C2 — Observability en producción

Tu binario ya está listo para deployment. Ahora viene la pregunta:
**cuando algo falla en producción, ¿cómo te enterás?** El próximo cap
cubre logs estructurados, spans HTTP, métricas y el bridge OTel para
integrar con Jaeger/Datadog/Honeycomb.

→ [M7.C2 — Observability con OpenTelemetry](c2-observability-otel.md)
