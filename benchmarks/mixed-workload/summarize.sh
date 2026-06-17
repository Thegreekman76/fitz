#!/usr/bin/env bash
# summarize.sh — genera summary.md a partir de los results/<timestamp>/
# del bench mixed-workload. Lee los JSON de k6 + cold_start + mem/cpu
# peaks y arma tablas comparativas Fitz vs Python vs Node.
#
# Uso: bash summarize.sh <results_dir>

set -euo pipefail

# Fuerza locale neutral C para `printf "%.2f"`. Sin esto, en sistemas
# con locale es-* / pt-* / fr-* / etc., bash printf usa coma decimal
# (`3,00` en vez de `3.00`). awk siempre usa punto, así que mezclábamos
# formatos en el mismo summary. Hay que tener `.` para que el output
# sea consumible por jq/herramientas downstream y para que los lectores
# internacionales (publicaciones, blogs) reciban el formato esperado.
export LC_NUMERIC=C

if [ "$#" -lt 1 ]; then
    echo "Uso: $0 <results_dir>" >&2
    exit 1
fi

RESULTS_DIR="$1"
TS=$(basename "$RESULTS_DIR")

# Helpers para extraer values de los JSON de k6. k6 puede exportar
# claves con diferentes shapes según versión (5.x vs 6.x). Estos
# helpers son tolerantes — devuelven `?` si la clave no existe.

# Extract HTTP req duration percentile from k6 summary export.
#
# Estructura real verificada con k6 v1.x (output --summary-export):
#   metrics.http_req_duration = { min, med, p(90), p(95), p(99), p(99.9), max, avg }
#   metrics.http_req_failed   = { passes, fails, value }
#   metrics.http_reqs         = { count, rate }
#
# Nota: k6 NO emite `p(50)` por default — el médiano se llama `med`.
# Para los demás percentiles los pedimos en `summaryTrendStats` de
# cada scenario. Si el scenario no listó un percentile, queda null.
k6_dur() {
    local file="$1"
    local pct="$2"
    if [ ! -s "$file" ]; then echo "?"; return; fi
    local jq_key
    if [ "$pct" = "50" ]; then
        # k6 usa `med` para el 50, no `p(50)`.
        jq_key='.metrics.http_req_duration.med'
    else
        jq_key=".metrics.http_req_duration[\"p($pct)\"]"
    fi
    local val
    val=$(jq -r "$jq_key // empty" "$file" 2>/dev/null || echo "")
    if [ -z "$val" ] || [ "$val" = "null" ]; then
        echo "?"
    else
        printf "%.2f" "$val"
    fi
}

k6_rps() {
    local file="$1"
    if [ ! -s "$file" ]; then echo "?"; return; fi
    local val
    val=$(jq -r '.metrics.http_reqs.rate // empty' "$file" 2>/dev/null || echo "")
    if [ -z "$val" ] || [ "$val" = "null" ]; then
        echo "?"
    else
        printf "%.1f" "$val"
    fi
}

k6_errors() {
    local file="$1"
    if [ ! -s "$file" ]; then echo "?"; return; fi
    # k6 v1.x: el rate del metric `http_req_failed` se serializa como
    # `value` (no `rate`). Es un float [0..1] que multiplicamos por
    # 100 para mostrar como porcentaje.
    local val
    val=$(jq -r '.metrics.http_req_failed.value // empty' "$file" 2>/dev/null || echo "")
    if [ -z "$val" ] || [ "$val" = "null" ]; then
        echo "?"
    else
        awk -v v="$val" 'BEGIN { printf "%.2f", v * 100 }'
    fi
}

k6_count() {
    local file="$1"
    if [ ! -s "$file" ]; then echo "?"; return; fi
    local val
    val=$(jq -r '.metrics.http_reqs.count // empty' "$file" 2>/dev/null || echo "")
    if [ -z "$val" ] || [ "$val" = "null" ]; then
        echo "?"
    else
        printf "%.0f" "$val"
    fi
}

# --- Hardware auto-detect -------------------------------------------
#
# Portado del summarize.sh del bench `orm-vs-sqlalchemy` (v0.10.23).
# Soporta los 3 OS típicos (Windows via Git Bash/MSYS, Linux, macOS)
# con fallback claro si alguna herramienta no está disponible.
# Falla silenciosa por defecto — el summary se genera con "?" en
# los campos que no se pudieron detectar.
#
# **Windows**: usamos `powershell.exe + Get-CimInstance` en vez de
# `wmic` porque WMIC fue deprecado en 2021 y removido de Win 11.

detect_cpu() {
    case "$(uname -s)" in
        Linux*)
            if command -v lscpu >/dev/null 2>&1; then
                lscpu 2>/dev/null | awk -F: '/^Model name/ { sub(/^[[:space:]]+/, "", $2); print $2; exit }'
            else
                cat /proc/cpuinfo 2>/dev/null | awk -F: '/^model name/ { sub(/^[[:space:]]+/, "", $2); print $2; exit }' || echo "?"
            fi
            ;;
        Darwin*)
            sysctl -n machdep.cpu.brand_string 2>/dev/null || echo "?"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            powershell.exe -NoProfile -Command \
                "(Get-CimInstance Win32_Processor).Name" 2>/dev/null \
                | tr -d '\r' \
                | head -n1 \
                || echo "?"
            ;;
        *) echo "?" ;;
    esac
}

detect_ram_gb() {
    case "$(uname -s)" in
        Linux*)
            awk '/^MemTotal/ { printf "%.0f GB", $2/1024/1024 }' /proc/meminfo 2>/dev/null || echo "?"
            ;;
        Darwin*)
            local b
            b=$(sysctl -n hw.memsize 2>/dev/null || echo "0")
            awk -v b="$b" 'BEGIN { if (b > 0) printf "%.0f GB", b/1024/1024/1024; else print "?" }'
            ;;
        MINGW*|MSYS*|CYGWIN*)
            local gb
            gb=$(powershell.exe -NoProfile -Command \
                "[math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)" \
                2>/dev/null | tr -d '\r' | head -n1)
            if [ -n "$gb" ]; then echo "$gb GB"; else echo "?"; fi
            ;;
        *) echo "?" ;;
    esac
}

detect_os() {
    case "$(uname -s)" in
        Linux*)
            if [ -f /etc/os-release ]; then
                . /etc/os-release
                echo "${PRETTY_NAME:-Linux $(uname -r)}"
            else
                echo "Linux $(uname -r)"
            fi
            ;;
        Darwin*)
            local ver
            ver=$(sw_vers -productVersion 2>/dev/null || echo "?")
            echo "macOS $ver"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            powershell.exe -NoProfile -Command \
                "(Get-CimInstance Win32_OperatingSystem).Caption" 2>/dev/null \
                | tr -d '\r' \
                | sed 's/^Microsoft //' \
                | head -n1 \
                || echo "Windows"
            ;;
        *) uname -sr ;;
    esac
}

detect_docker() {
    docker --version 2>/dev/null | sed 's/^Docker version //;s/, build .*$//' || echo "?"
}

HW_CPU=$(detect_cpu)
HW_RAM=$(detect_ram_gb)
HW_OS=$(detect_os)
HW_DOCKER=$(detect_docker)
# Defaults para los casos en que detection falló silenciosa
# (awk no encontró match, PowerShell sin output, etc.).
[ -z "$HW_CPU" ] && HW_CPU="?"
[ -z "$HW_RAM" ] && HW_RAM="?"
[ -z "$HW_OS" ] && HW_OS="?"
[ -z "$HW_DOCKER" ] && HW_DOCKER="?"

read_val() {
    local file="$1"
    if [ -s "$file" ]; then cat "$file"; else echo "?"; fi
}

image_size() {
    local file="$1"
    if [ -s "$file" ]; then
        awk '{print $2 $3}' "$file" | head -1
    else
        echo "?"
    fi
}

# --- Header ---------------------------------------------------------
cat <<EOF
# Benchmark Mixed-workload — corrida $TS

Tres stacks side-by-side: **Fitz ORM nativo** vs **Python+SQLAlchemy**
vs **Node+Prisma**. Mismo dominio (users + posts), mismos endpoints,
misma DB Postgres 16. Carga: mixed 60% reads + 40% writes con VUs
rampeando 10→50→100→50 sobre 3 min.

## Cold start, image, memory, CPU

| Métrica | Fitz | Python | Node |
|---|---:|---:|---:|
| Cold start (s) | $(read_val "$RESULTS_DIR/fitz/cold_start.sec") | $(read_val "$RESULTS_DIR/python/cold_start.sec") | $(read_val "$RESULTS_DIR/node/cold_start.sec") |
| Image size | $(image_size "$RESULTS_DIR/fitz/image_sizes.txt") | $(image_size "$RESULTS_DIR/python/image_sizes.txt") | $(image_size "$RESULTS_DIR/node/image_sizes.txt") |
| Memory peak (MB) | $(read_val "$RESULTS_DIR/fitz/mem_peak.mb") | $(read_val "$RESULTS_DIR/python/mem_peak.mb") | $(read_val "$RESULTS_DIR/node/mem_peak.mb") |
| CPU peak (%) | $(read_val "$RESULTS_DIR/fitz/cpu_peak.pct") | $(read_val "$RESULTS_DIR/python/cpu_peak.pct") | $(read_val "$RESULTS_DIR/node/cpu_peak.pct") |

## Mixed workload (3 min, ramp 10→50→100→50, 60% reads + 40% writes)

| Métrica | Fitz | Python | Node |
|---|---:|---:|---:|
| Total requests | $(k6_count "$RESULTS_DIR/fitz/mixed.json") | $(k6_count "$RESULTS_DIR/python/mixed.json") | $(k6_count "$RESULTS_DIR/node/mixed.json") |
| Throughput (RPS) | $(k6_rps "$RESULTS_DIR/fitz/mixed.json") | $(k6_rps "$RESULTS_DIR/python/mixed.json") | $(k6_rps "$RESULTS_DIR/node/mixed.json") |
| p50 latency (ms) | $(k6_dur "$RESULTS_DIR/fitz/mixed.json" 50) | $(k6_dur "$RESULTS_DIR/python/mixed.json" 50) | $(k6_dur "$RESULTS_DIR/node/mixed.json" 50) |
| p95 latency (ms) | $(k6_dur "$RESULTS_DIR/fitz/mixed.json" 95) | $(k6_dur "$RESULTS_DIR/python/mixed.json" 95) | $(k6_dur "$RESULTS_DIR/node/mixed.json" 95) |
| p99 latency (ms) | $(k6_dur "$RESULTS_DIR/fitz/mixed.json" 99) | $(k6_dur "$RESULTS_DIR/python/mixed.json" 99) | $(k6_dur "$RESULTS_DIR/node/mixed.json" 99) |
| p99.9 latency (ms) | $(k6_dur "$RESULTS_DIR/fitz/mixed.json" 99.9) | $(k6_dur "$RESULTS_DIR/python/mixed.json" 99.9) | $(k6_dur "$RESULTS_DIR/node/mixed.json" 99.9) |
| Error rate (%) | $(k6_errors "$RESULTS_DIR/fitz/mixed.json") | $(k6_errors "$RESULTS_DIR/python/mixed.json") | $(k6_errors "$RESULTS_DIR/node/mixed.json") |

## Reads-only (1 min, 50 VUs)

| Métrica | Fitz | Python | Node |
|---|---:|---:|---:|
| Throughput (RPS) | $(k6_rps "$RESULTS_DIR/fitz/reads_only.json") | $(k6_rps "$RESULTS_DIR/python/reads_only.json") | $(k6_rps "$RESULTS_DIR/node/reads_only.json") |
| p50 (ms) | $(k6_dur "$RESULTS_DIR/fitz/reads_only.json" 50) | $(k6_dur "$RESULTS_DIR/python/reads_only.json" 50) | $(k6_dur "$RESULTS_DIR/node/reads_only.json" 50) |
| p95 (ms) | $(k6_dur "$RESULTS_DIR/fitz/reads_only.json" 95) | $(k6_dur "$RESULTS_DIR/python/reads_only.json" 95) | $(k6_dur "$RESULTS_DIR/node/reads_only.json" 95) |
| p99 (ms) | $(k6_dur "$RESULTS_DIR/fitz/reads_only.json" 99) | $(k6_dur "$RESULTS_DIR/python/reads_only.json" 99) | $(k6_dur "$RESULTS_DIR/node/reads_only.json" 99) |
| Error rate (%) | $(k6_errors "$RESULTS_DIR/fitz/reads_only.json") | $(k6_errors "$RESULTS_DIR/python/reads_only.json") | $(k6_errors "$RESULTS_DIR/node/reads_only.json") |

## Writes-only (1 min, 50 VUs)

> Este scenario llena el gap del bench anterior — write concurrency
> real con saturación del pool de cada ORM.

| Métrica | Fitz | Python | Node |
|---|---:|---:|---:|
| Throughput (RPS) | $(k6_rps "$RESULTS_DIR/fitz/writes_only.json") | $(k6_rps "$RESULTS_DIR/python/writes_only.json") | $(k6_rps "$RESULTS_DIR/node/writes_only.json") |
| p50 (ms) | $(k6_dur "$RESULTS_DIR/fitz/writes_only.json" 50) | $(k6_dur "$RESULTS_DIR/python/writes_only.json" 50) | $(k6_dur "$RESULTS_DIR/node/writes_only.json" 50) |
| p95 (ms) | $(k6_dur "$RESULTS_DIR/fitz/writes_only.json" 95) | $(k6_dur "$RESULTS_DIR/python/writes_only.json" 95) | $(k6_dur "$RESULTS_DIR/node/writes_only.json" 95) |
| p99 (ms) | $(k6_dur "$RESULTS_DIR/fitz/writes_only.json" 99) | $(k6_dur "$RESULTS_DIR/python/writes_only.json" 99) | $(k6_dur "$RESULTS_DIR/node/writes_only.json" 99) |
| Error rate (%) | $(k6_errors "$RESULTS_DIR/fitz/writes_only.json") | $(k6_errors "$RESULTS_DIR/python/writes_only.json") | $(k6_errors "$RESULTS_DIR/node/writes_only.json") |

---

## Cómo leer estos números

- **RPS más alto = mejor** (más users servidos por segundo).
- **Latencia más baja = mejor** (los users esperan menos).
- **Error rate más bajo = mejor** (sostiene más carga sin romperse).
- **p99 / p99.9** son la verdad del bench — el p50 puede ser bueno
  con el p99 catastrófico (long-tail latencies) que pegan al user.

Para resultados publicables: correr 3 veces y reportar mediana de
las 3 corridas para cada métrica.

## Contexto de la corrida

- **Hardware del runner** (auto-detectado por \`summarize.sh\`;
  verificá antes de publicar — PowerShell/lscpu/sysctl pueden
  devolver strings raros):
  - CPU: $HW_CPU
  - RAM: $HW_RAM
  - OS: $HW_OS
  - Docker: $HW_DOCKER
- **Variabilidad esperada**: ±10-15% entre corridas locales por
  CPU thermals, otros procesos, cache state de PG.
EOF
