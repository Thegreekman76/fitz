# `cli-tool` — boilerplate CLI puro a binario nativo

Generador de report de ventas que demuestra el subset CLI del
lenguaje Fitz compilado a **binario nativo standalone** y
empaquetado en una imagen Docker de **~22 MB** (`distroless/cc`).

Sin runtime Fitz en el destino. Sin Python. Sin Node. Sin libc del
host. El binario `fitz-report-generator` corre, imprime el report,
y sale.

```text
📊 Sales Report
===============

Total ventas: 7
Revenue total: $959.75

Por región:
  AR: 3 ventas, $362.00
  BR: 2 ventas, $167.75
  US: 2 ventas, $430.00

Top venta única:
  gadget ($320.00) en US
```

## Qué demuestra

- **Tipos custom** con campos tipados (`type Sale { product: Str,
  amount: Float, region: Str }`).
- **Listas + métodos higher-order** (`.filter()`, `.map()`,
  `.reduce()`, `.unique()`).
- **Interpolación de strings con format specs** (`"${x:.2f}"`).
- **Funciones top-level** con tipos explícitos.
- **`fitz build`** comprime ~50 líneas de Fitz a un binario nativo
  Linux x86_64 standalone.

## Estructura del directorio

```
cli-tool/
├── README.md          ← este archivo
├── fitz.toml          ← manifest del package manager Fitz
├── src/
│   └── main.fitz      ← código fuente (~50 LoC)
├── Dockerfile         ← multi-stage build: fitz builder + distroless runtime
├── .dockerignore
└── .gitignore
```

## Prerequisitos

**Solo Docker** (versión 24+ recomendada por BuildKit). Nada más.

```bash
docker --version
# Docker version 24.x o superior
```

> **Nota**: NO necesitás Fitz instalado localmente. El Dockerfile
> usa la imagen oficial `ghcr.io/thegreekman76/fitz:latest` como
> builder, que ya trae `fitz` + toolchain Rust.

## Paso a paso

### 1. Construir la imagen

```bash
cd boilerplates/cli-tool
docker build -t fitz-cli-tool .
```

El primer build tarda ~2-3 min (descarga la imagen base + buildea).
Builds subsiguientes son cacheados, ~10 segundos si solo cambias
`src/main.fitz`.

### 2. Ejecutar

```bash
docker run --rm fitz-cli-tool
```

El `--rm` borra el container al terminar (el binario corre y sale,
no queda nada). Output esperado: el report de ventas mostrado
arriba.

### 3. (Opcional) Inspeccionar el binario

Si querés ver el tamaño / metadata del binario nativo:

```bash
docker run --rm --entrypoint=ls fitz-cli-tool -lh /usr/local/bin/app
# -rwxr-xr-x 1 root root 5.2M Jan  1  1970 /usr/local/bin/app
```

~5 MB para todo el binario. Sin features extra es aún más chico.

## Cómo extender

### Reemplazar los datos hardcoded

El boilerplate usa un array `SALES` hardcoded porque `args()` y
`stdin()` no están implementados todavía en Fitz (vienen en Fase 9
extras). Para tu proyecto real:

- Editá `let SALES = [...]` en `src/main.fitz` con tu data.
- Rebuild: `docker build -t fitz-cli-tool .`.

### Agregar más reports

Sumá nuevas fns top-level en `src/main.fitz`:

```fitz
fn average_per_region(sales: List<Sale>, region: Str) -> Float {
    let subset = by_region(sales, region)
    if (subset.len() == 0) { return 0.0 }
    return total(subset) / (subset.len() as Float)
}
```

Llamalas desde el bloque `print(...)` al final.

### Cambiar a `FROM scratch` (binario ultra-mínimo)

`distroless/cc-debian12` trae glibc + libgcc, lo que permite linkear
con C ABI estándar. Si querés llegar a **`FROM scratch`** (~5 MB en
total), compilá con musl + static linking:

1. En el `Dockerfile`, agregá target musl al builder:
   ```dockerfile
   RUN rustup target add x86_64-unknown-linux-musl
   RUN fitz build --target x86_64-unknown-linux-musl  # cuando Fitz soporte --target
   ```
2. Cambiá la base del stage runtime a `FROM scratch`.

**Importante**: Fitz no soporta `--target` todavía. Esta es deuda
futura del lenguaje. Mientras tanto, `distroless/cc` es la
alternativa más chica viable.

## Variables de entorno

Este boilerplate no usa ninguna. El binario es 100% deterministic
sobre datos hardcoded.

## Troubleshooting

### `docker pull ghcr.io/thegreekman76/fitz:latest` falla con `401 Unauthorized` o `denied`

La imagen base es pública, pero si tu Docker daemon tiene credenciales
inválidas a GHCR puede confundirse. Solución:

```bash
docker logout ghcr.io
docker pull ghcr.io/thegreekman76/fitz:latest
```

Si seguís sin poder pullear, abrí un issue en
[github.com/Thegreekman76/fitz/issues](https://github.com/Thegreekman76/fitz/issues)
con el output completo del `docker pull`.

### El build tarda más de 5 minutos

Casi siempre es porque la cache de Docker se invalida innecesariamente.
Asegurate de NO modificar `fitz.toml` ni `src/main.fitz` entre builds
si querés cached layers. Si la cache se invalida, el `RUN fitz build`
tiene que recompilar todo (~1-2 min).

### El binario no corre — `exec format error`

Tu Docker está intentando correr un binario x86_64 en un host ARM
(M-series Macs). Solución: agregar `--platform linux/amd64` al
`docker run`:

```bash
docker run --rm --platform linux/amd64 fitz-cli-tool
```

Para builds nativos en M-series, necesitamos que `ghcr.io/thegreekman76/fitz`
publique una imagen ARM64 — deuda futura del `release.yml` (hoy
publica solo Linux x64 base).

## Siguientes pasos

- Mirá [`boilerplates/api-simple/`](../api-simple/) para un servicio
  HTTP nativo.
- Si tu uso es batch periódico (cleanup nocturno, generar reportes
  cada N minutos), considerá `@cron` + `@background` — ver
  [cap 30 de la guía](../../docs/guide.md#30-jobs-sin-celery).

## Roadmap del boilerplate

Mejoras planificadas cuando aparezca presión real:

- **`args()` builtin** del lenguaje → el report acepta filtros por
  CLI args (`--region AR`, `--top 3`).
- **`read_file()` builtin** → leer ventas desde CSV / JSON.
- **`exit(code)` builtin** → exit codes distintos según
  succeed/error.
- **Cross-compile a musl** → binario `FROM scratch` ~3 MB total.
- **Multi-arch image** (linux/amd64 + linux/arm64) → soporte nativo
  para Mac M-series.
