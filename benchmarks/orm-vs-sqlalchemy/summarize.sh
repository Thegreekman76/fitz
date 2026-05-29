#!/usr/bin/env bash
# summarize.sh — parsea los JSON outputs de oha y produce un summary.md
# comparativo Fitz ORM vs SQLAlchemy.
#
# Uso: bash summarize.sh <results_dir>
# Output: stdout (markdown).
#
# Espera el layout:
#   <results_dir>/
#     fitz/{get_users,get_user_id,post_users}.json
#     fitz/cold_start.sec
#     fitz/mem_peak.mb
#     fitz/image_sizes.txt
#     python/{...mismos archivos...}

set -euo pipefail

RESULTS_DIR="${1:?usage: $0 <results_dir>}"

if [ ! -d "$RESULTS_DIR/fitz" ] || [ ! -d "$RESULTS_DIR/python" ]; then
    echo "ERROR: $RESULTS_DIR debe contener fitz/ y python/" >&2
    exit 1
fi

# Helpers para extraer valores de los JSON de oha 1.14+.
# oha 1.14 schema (relevante):
#   summary.successRate, summary.total (= duración total en SEG)
#   rps.mean (= RPS promedio; antes era summary.requestsPerSec)
#   latencyPercentiles.p50, p95, p99 (en SEG)
#   statusCodeDistribution: {"200": N, ...} (sum para total requests)
oha_p50() { jq -r '.latencyPercentiles.p50' "$1"; }
oha_p95() { jq -r '.latencyPercentiles.p95' "$1"; }
oha_p99() { jq -r '.latencyPercentiles.p99' "$1"; }
# RPS sustained = total_requests / total_time (segundos).
# Antes usaba `rps.mean` pero en oha 1.14 ese es el RPS INSTANTÁNEO
# del peak, NO el sustained — daba números 50-80x el real (ej:
# 17400 RPS reportado para 6521 reqs en 30s = 217 RPS real).
oha_rps() {
    jq -r '
        ([.statusCodeDistribution | to_entries[].value] | add) as $total |
        if .summary.total > 0 then ($total / .summary.total) else 0 end
    ' "$1"
}
oha_total() { jq -r '[.statusCodeDistribution | to_entries[].value] | add' "$1"; }
oha_succ_rate() { jq -r '.summary.successRate' "$1"; }

# POST bench is custom JSON (kind=post_bench_custom).
post_p50() { jq -r '.latency_sec.p50' "$1"; }
post_p95() { jq -r '.latency_sec.p95' "$1"; }
post_p99() { jq -r '.latency_sec.p99' "$1"; }
post_rps() { jq -r '.rps' "$1"; }
post_total() { jq -r '.total_requests' "$1"; }
post_succ() { jq -r '(.ok / .total_requests)' "$1"; }

# Format seconds → ms with 2 decimals.
ms() { awk -v v="$1" 'BEGIN { printf "%.2f", v * 1000 }'; }
n2() { awk -v v="$1" 'BEGIN { printf "%.2f", v + 0 }'; }
n1() { awk -v v="$1" 'BEGIN { printf "%.1f", v + 0 }'; }
pct() { awk -v v="$1" 'BEGIN { printf "%.1f%%", v * 100 }'; }

read_val() {
    local f="$1"
    [ -f "$f" ] && cat "$f" || echo "?"
}

# Cold start.
COLD_FITZ=$(read_val "$RESULTS_DIR/fitz/cold_start.sec")
COLD_PY=$(read_val "$RESULTS_DIR/python/cold_start.sec")

# Memory peak.
MEM_FITZ=$(read_val "$RESULTS_DIR/fitz/mem_peak.mb")
MEM_PY=$(read_val "$RESULTS_DIR/python/mem_peak.mb")

# Image size — primer línea de image_sizes.txt, segundo campo.
IMG_FITZ=$(awk 'NR==1 { for (i=2; i<=NF; i++) printf "%s ", $i; print "" }' "$RESULTS_DIR/fitz/image_sizes.txt" 2>/dev/null | xargs || echo "?")
IMG_PY=$(awk 'NR==1 { for (i=2; i<=NF; i++) printf "%s ", $i; print "" }' "$RESULTS_DIR/python/image_sizes.txt" 2>/dev/null | xargs || echo "?")

cat <<EOF
# Benchmark Fitz ORM nativo vs SQLAlchemy (interop Python)

**Timestamp:** $(basename "$RESULTS_DIR")
**Setup:**
- Boilerplate Fitz: \`api-postgres-fitz\` (driver Postgres v3.0 puro, ORM nativo)
- Boilerplate Python: \`api-postgres-python\` (Fitz + \`from python import\` + SQLAlchemy 2.x)
- Postgres 16-alpine como DB (mismo container compose en ambos casos)
- 3 endpoints idénticos: \`GET /users\`, \`GET /users/{id}\`, \`POST /users\`
- 30s sostenidos a concurrency=10 por endpoint (GET); 500 POST sequential

## Cold start, image, memory

| Métrica | Fitz ORM | Python (SQLAlchemy) | Ratio (Fitz/Python) |
|---|---:|---:|---:|
| Cold start (s) | $COLD_FITZ | $COLD_PY | $(awk -v f="$COLD_FITZ" -v p="$COLD_PY" 'BEGIN { if (p+0 > 0) printf "%.2fx", f/p; else print "?" }') |
| Image size | $IMG_FITZ | $IMG_PY | — |
| Memory peak (MB) | $MEM_FITZ | $MEM_PY | $(awk -v f="$MEM_FITZ" -v p="$MEM_PY" 'BEGIN { if (p+0 > 0) printf "%.2fx", f/p; else print "?" }') |

## Latencia por endpoint (ms)

EOF

for ep in get_users get_user_id; do
    f_p50=$(oha_p50 "$RESULTS_DIR/fitz/$ep.json")
    f_p95=$(oha_p95 "$RESULTS_DIR/fitz/$ep.json")
    f_p99=$(oha_p99 "$RESULTS_DIR/fitz/$ep.json")
    f_rps=$(oha_rps "$RESULTS_DIR/fitz/$ep.json")
    f_total=$(oha_total "$RESULTS_DIR/fitz/$ep.json")
    f_succ=$(oha_succ_rate "$RESULTS_DIR/fitz/$ep.json")
    p_p50=$(oha_p50 "$RESULTS_DIR/python/$ep.json")
    p_p95=$(oha_p95 "$RESULTS_DIR/python/$ep.json")
    p_p99=$(oha_p99 "$RESULTS_DIR/python/$ep.json")
    p_rps=$(oha_rps "$RESULTS_DIR/python/$ep.json")
    p_total=$(oha_total "$RESULTS_DIR/python/$ep.json")
    p_succ=$(oha_succ_rate "$RESULTS_DIR/python/$ep.json")
    label=$(echo "$ep" | sed 's/_/ /g; s/get/GET /; s/user id/users\/{id}/; s/users$/users/')
    cat <<EOF
### \`${label}\` (30s, c=10)

| Métrica | Fitz ORM | Python (SQLAlchemy) | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | $(ms "$f_p50") | $(ms "$p_p50") | $(awk -v f="$f_p50" -v p="$p_p50" 'BEGIN { if (f+0 > 0) printf "%.2fx", p/f; else print "?" }') |
| p95 latency (ms) | $(ms "$f_p95") | $(ms "$p_p95") | $(awk -v f="$f_p95" -v p="$p_p95" 'BEGIN { if (f+0 > 0) printf "%.2fx", p/f; else print "?" }') |
| p99 latency (ms) | $(ms "$f_p99") | $(ms "$p_p99") | $(awk -v f="$f_p99" -v p="$p_p99" 'BEGIN { if (f+0 > 0) printf "%.2fx", p/f; else print "?" }') |
| Throughput (RPS) | $(n2 "$f_rps") | $(n2 "$p_rps") | $(awk -v f="$f_rps" -v p="$p_rps" 'BEGIN { if (p+0 > 0) printf "%.2fx", f/p; else print "?" }') |
| Total requests | $(n2 "$f_total") | $(n2 "$p_total") | — |
| Success rate | $(pct "$f_succ") | $(pct "$p_succ") | — |

EOF
done

# POST bench (custom format).
f_p50=$(post_p50 "$RESULTS_DIR/fitz/post_users.json")
f_p95=$(post_p95 "$RESULTS_DIR/fitz/post_users.json")
f_p99=$(post_p99 "$RESULTS_DIR/fitz/post_users.json")
f_rps=$(post_rps "$RESULTS_DIR/fitz/post_users.json")
f_total=$(post_total "$RESULTS_DIR/fitz/post_users.json")
f_succ=$(post_succ "$RESULTS_DIR/fitz/post_users.json")
p_p50=$(post_p50 "$RESULTS_DIR/python/post_users.json")
p_p95=$(post_p95 "$RESULTS_DIR/python/post_users.json")
p_p99=$(post_p99 "$RESULTS_DIR/python/post_users.json")
p_rps=$(post_rps "$RESULTS_DIR/python/post_users.json")
p_total=$(post_total "$RESULTS_DIR/python/post_users.json")
p_succ=$(post_succ "$RESULTS_DIR/python/post_users.json")

cat <<EOF
### \`POST /users\` (500 sequential)

| Métrica | Fitz ORM | Python (SQLAlchemy) | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | $(ms "$f_p50") | $(ms "$p_p50") | $(awk -v f="$f_p50" -v p="$p_p50" 'BEGIN { if (f+0 > 0) printf "%.2fx", p/f; else print "?" }') |
| p95 latency (ms) | $(ms "$f_p95") | $(ms "$p_p95") | $(awk -v f="$f_p95" -v p="$p_p95" 'BEGIN { if (f+0 > 0) printf "%.2fx", p/f; else print "?" }') |
| p99 latency (ms) | $(ms "$f_p99") | $(ms "$p_p99") | $(awk -v f="$f_p99" -v p="$p_p99" 'BEGIN { if (f+0 > 0) printf "%.2fx", p/f; else print "?" }') |
| Throughput (RPS) | $(n2 "$f_rps") | $(n2 "$p_rps") | $(awk -v f="$f_rps" -v p="$p_rps" 'BEGIN { if (p+0 > 0) printf "%.2fx", f/p; else print "?" }') |
| Total requests | $(n2 "$f_total") | $(n2 "$p_total") | — |
| Success rate | $(pct "$f_succ") | $(pct "$p_succ") | — |

## Notas

- **"Speedup"** se lee como _"Fitz es Nx más rápido"_ para latencia
  (menor mejor) y RPS (mayor mejor). Para memory/cold start, lo mismo
  con Fitz/Python (menor mejor).
- **Variabilidad esperada**: ±10% entre corridas en máquinas locales
  (CPU thermals, otros procesos, cache state de PG). Para medidas
  publicables correr 3 veces y reportar mediana.
- **POST sequential** mide latencia "honesta" con bodies únicos por
  request. No mide throughput concurrente — para eso necesitaría tool
  con body randomization (k6, wrk+lua).
- **Memory peak** muestreado cada 500ms via \`docker stats\` durante
  el bench. Puede subestimar peaks transientes <500ms.
- Hardware del run NO está auto-detectado — anotar a mano abajo:
  - CPU: TODO
  - RAM: TODO
  - OS: TODO
  - Docker: TODO

EOF
