# M7.C1 — Setup + primer programa con interop Python

Ejemplo runnable del cap [M7.C1 del curso](../../../../docs/curso/m7-python-interop/c1-setup-imports.md).

## Setup

```bash
# 1. Crear venv (estándar Python, sin magia de Fitz).
$ python3 -m venv venv

# 2. Activar venv ANTES de correr fitz-python.
$ source venv/bin/activate                  # Linux/macOS
# PS> venv\Scripts\Activate.ps1             # Windows PowerShell

# 3. Compilar fitz con feature python (una sola vez).
$ cd /path/to/fitz && cargo build --release --features python
$ cp target/release/fitz ~/.local/bin/fitz-python   # o tu PATH
```

## Run

```bash
(venv) $ fitz-python run app.fitz
{"timestamp":"...","level":"INFO","msg":"server listo"}
```

## Probar

```bash
$ curl localhost:3000/circle/5.0
{"radius":5.0,"area":78.53981633974483,"circumference":31.41592653589793}

$ curl localhost:3000/timestamp
{"iso":"2026-06-03T21:45:12.487193","year":2026,"month":6,"day":3}

$ curl localhost:3000/echo
{"app": "demo-python-interop", "version": "v1.0"}
```

## Qué cubre

- `from python import` con módulos stdlib (math/json/datetime).
- Auto-coerción primitiva (math.pi → Float, math.sqrt → Float).
- Objetos opacos (datetime.datetime.now() → PyObject) con method calls
  (now.year → Int auto-coercionado).
- Handler HTTP nativo Fitz combinando los 3 módulos Python.
