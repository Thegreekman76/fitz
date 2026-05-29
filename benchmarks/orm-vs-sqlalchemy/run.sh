#!/usr/bin/env bash
# run.sh — benchmark Fitz ORM nativo vs SQLAlchemy (interop Python).
#
# Compara los dos boilerplates equivalentes:
#   - api-postgres-fitz       → ORM nativo Fitz (driver Postgres v3.0 puro)
#   - api-postgres-python     → SQLAlchemy via interop Python
#
# Ambos exponen los mismos 3 endpoints (`GET /users`, `GET /users/{id}`,
# `POST /users`) con misma firma de body. Comparación cabeza-a-cabeza.
#
# Métricas:
#   - Cold start: tiempo desde `docker compose up -d` hasta primer 200 OK
#   - Latencia p50/p95/p99 + RPS para cada endpoint (30s sostenido, c=10)
#   - Memory peak via `docker stats` muestreado durante el run
#   - Image size (`docker images`)
#
# Prerequisitos:
#   - docker + docker compose
#   - oha (https://github.com/hatoo/oha) — si no está, instala via cargo
#   - jq (parsing JSON)
#   - curl
#
# Uso:
#   cd benchmarks/orm-vs-sqlalchemy
#   bash run.sh
#
# Output: results/<timestamp>/ con JSON outputs de cada bench, raw memory
# samples, y un summary.md auto-generado.

set -euo pipefail

# --- Config ---------------------------------------------------------
BENCH_DURATION="${BENCH_DURATION:-30s}"   # duración por endpoint
BENCH_CONCURRENCY="${BENCH_CONCURRENCY:-10}"
COLD_START_TIMEOUT="${COLD_START_TIMEOUT:-120}"  # s máximos para ver primer 200
SEED_USERS="${SEED_USERS:-50}"  # pre-inserts para que /users tenga data
                                # (Git Bash en Windows: subshell overhead
                                # del for loop ~1s/iter, por eso default 50
                                # en vez de 200 — sigue siendo suficiente
                                # data para que GET /users sea representativo)
WAIT_BETWEEN_BENCHES=2  # s entre endpoints para que el server "respire"

# --- Setup paths ----------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
RESULTS_DIR="$SCRIPT_DIR/results/$TIMESTAMP"
mkdir -p "$RESULTS_DIR"

echo "════════════════════════════════════════════════════════════════"
echo " Fitz ORM vs SQLAlchemy — benchmark MVP"
echo "════════════════════════════════════════════════════════════════"
echo " Timestamp:   $TIMESTAMP"
echo " Results dir: $RESULTS_DIR"
echo " Duration:    $BENCH_DURATION per endpoint"
echo " Concurrency: $BENCH_CONCURRENCY"
echo " Seed users:  $SEED_USERS"
echo ""

# --- Tool checks ----------------------------------------------------
need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "ERROR: falta `$1`. Instalalo y volvé a correr."
        case "$1" in
            oha)    echo "  cargo install oha   (o release pre-built en https://github.com/hatoo/oha)" ;;
            jq)     echo "  https://jqlang.github.io/jq/" ;;
            docker) echo "  https://docs.docker.com/get-docker/" ;;
        esac
        exit 1
    fi
}
need docker
need oha
need jq
need curl

# --- Helpers --------------------------------------------------------
log() { echo "▸ $(date +%H:%M:%S)  $*"; }

# Wait until the URL returns HTTP 200. Echos cold start time (seconds, float).
wait_for_200() {
    local url="$1"
    local timeout="$2"
    local label="$3"
    local start=$(date +%s.%N)
    local deadline=$(( $(date +%s) + timeout ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        local code
        code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 2 "$url" || echo "000")
        if [ "$code" = "200" ]; then
            local end=$(date +%s.%N)
            awk -v s="$start" -v e="$end" 'BEGIN { printf "%.2f\n", e - s }'
            return 0
        fi
        sleep 0.5
    done
    echo "ERROR: $label no respondió 200 en ${timeout}s" >&2
    return 1
}

# Background memory sampler. Writes <out> con timestamps + RSS MB,
# y guarda el PID del background en <out>.pid.
#
# Fix Git Bash Windows: usar archivo PID en lugar de capturar via
# `$()` — la captura espera que TODO el subshell termine, incluyendo
# el `while true` background, → nunca retorna. Con `>/dev/null 2>&1`
# y archivo PID, el sampler queda completamente detached.
start_memory_sampler() {
    local container="$1"
    local out="$2"
    local pidfile="${out}.pid"
    (
        while true; do
            local mem
            mem=$(docker stats --no-stream --format "{{.MemUsage}}" "$container" 2>/dev/null || echo "")
            if [ -n "$mem" ]; then
                # `12.5MiB / 7.7GiB` → tomar primer número, convertir a MB
                local used="${mem%% *}"
                # Convert MiB/GiB/KiB → MB (approximación)
                local mb
                case "$used" in
                    *MiB)  mb=$(echo "$used" | sed 's/MiB//' | awk '{printf "%.1f", $1 * 1.048576}') ;;
                    *GiB)  mb=$(echo "$used" | sed 's/GiB//' | awk '{printf "%.1f", $1 * 1073.741824}') ;;
                    *KiB)  mb=$(echo "$used" | sed 's/KiB//' | awk '{printf "%.4f", $1 * 0.001024}') ;;
                    *)     mb="$used" ;;
                esac
                echo "$(date +%s.%N) $mb" >> "$out"
            fi
            sleep 0.5
        done
    ) >/dev/null 2>&1 &
    echo $! > "$pidfile"
}

stop_memory_sampler() {
    local pidfile="$1"
    if [ -f "$pidfile" ]; then
        local pid
        pid=$(cat "$pidfile")
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        rm -f "$pidfile"
    fi
}

# Calculate peak MB from sample log.
peak_mb() {
    local log="$1"
    if [ ! -s "$log" ]; then echo "?"; return; fi
    awk '{ if ($2+0 > max) max = $2+0 } END { printf "%.1f", max }' "$log"
}

# Seed N users via POST /users. Aborta temprano si el server tira
# 500 (típicamente bug de schema/timestamps — sin sentido seedear
# 50 más si todos van a fallar).
seed_users() {
    local url="$1"
    local n="$2"
    log "  seeding $n users..."
    local ok=0 fail=0
    for i in $(seq 1 "$n"); do
        local code
        code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 5 \
                    -X POST "$url/users" \
                    -H "Content-Type: application/json" \
                    -d "{\"name\":\"User $i\",\"email\":\"bench-$TIMESTAMP-$i-$RANDOM@example.com\"}" \
                    || echo "000")
        if [ "$code" = "200" ]; then
            ok=$((ok+1))
        else
            fail=$((fail+1))
            # Si los primeros 5 fallan TODOS, abort — algo está roto.
            if [ "$i" -le 5 ] && [ "$fail" -eq "$i" ]; then
                local err
                err=$(curl -s --max-time 5 -X POST "$url/users" \
                        -H "Content-Type: application/json" \
                        -d "{\"name\":\"probe\",\"email\":\"probe-err@x\"}")
                echo "ERROR: las primeras $fail requests del seed devolvieron $code." >&2
                echo "       Server response: $err" >&2
                return 1
            fi
        fi
        # Progress cada 10 iter.
        if [ $((i % 10)) -eq 0 ]; then
            log "    seeded $i/$n (ok=$ok fail=$fail)"
        fi
    done
    log "  seed done: ok=$ok fail=$fail"
}

# Run oha + capture JSON stats.
bench_endpoint() {
    local label="$1"
    local url="$2"
    local out_json="$3"
    log "  bench $label ($BENCH_DURATION, c=$BENCH_CONCURRENCY)..."
    # oha 1.14+ usa --output-format json (antes era -j). Capturamos
    # stderr en archivo separado para que `cat` del error muestre
    # solo el JSON output, no warnings (DeprecationWarning de oha
    # sobre puny code etc).
    oha -z "$BENCH_DURATION" -c "$BENCH_CONCURRENCY" --no-tui \
        --output-format json "$url" > "$out_json" 2> "$out_json.err" || {
        echo "ERROR: oha falló sobre $url" >&2
        echo "--- stderr ---" >&2
        cat "$out_json.err" >&2
        echo "--- stdout ---" >&2
        cat "$out_json" >&2
        return 1
    }
}

# Run oha POST with unique bodies. We pre-generate a JSON line file with
# unique emails, and oha takes a single body — so we use a custom curl
# loop sequential timing for POST instead. (~500 requests).
bench_post() {
    local label="$1"
    local url="$2"
    local out_json="$3"
    local n="${4:-500}"
    log "  bench $label (POST x $n)..."
    local lats_file
    lats_file=$(mktemp)
    local epoch_start=$(date +%s.%N)
    local ok=0 fail=0
    for i in $(seq 1 "$n"); do
        local email="post-bench-$TIMESTAMP-$i-$RANDOM@example.com"
        local body="{\"name\":\"POST Bench $i\",\"email\":\"$email\"}"
        local t0=$(date +%s.%N)
        local code
        code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 5 \
                    -X POST "$url/users" \
                    -H "Content-Type: application/json" \
                    -d "$body" || echo "000")
        local t1=$(date +%s.%N)
        if [ "$code" = "200" ]; then
            ok=$((ok+1))
            awk -v t0="$t0" -v t1="$t1" 'BEGIN { printf "%.6f\n", t1 - t0 }' >> "$lats_file"
        else
            fail=$((fail+1))
        fi
    done
    local epoch_end=$(date +%s.%N)
    # Compute p50/p95/p99 of lats from file.
    local sorted_lats
    sorted_lats=$(sort -n "$lats_file")
    local total_lats
    total_lats=$(echo "$sorted_lats" | wc -l | tr -d ' ')
    local p50_idx p95_idx p99_idx
    p50_idx=$(( total_lats / 2 ))
    p95_idx=$(( total_lats * 95 / 100 ))
    p99_idx=$(( total_lats * 99 / 100 ))
    [ "$p50_idx" -lt 1 ] && p50_idx=1
    [ "$p95_idx" -lt 1 ] && p95_idx=1
    [ "$p99_idx" -lt 1 ] && p99_idx=1
    local p50 p95 p99
    p50=$(echo "$sorted_lats" | sed -n "${p50_idx}p")
    p95=$(echo "$sorted_lats" | sed -n "${p95_idx}p")
    p99=$(echo "$sorted_lats" | sed -n "${p99_idx}p")
    local total_time
    total_time=$(awk -v s="$epoch_start" -v e="$epoch_end" 'BEGIN { printf "%.3f", e - s }')
    local rps
    rps=$(awk -v ok="$ok" -v t="$total_time" 'BEGIN { printf "%.2f", ok / t }')
    # Emit a tiny JSON con stats agregadas (formato paralelo al de oha).
    cat > "$out_json" <<EOF
{
  "kind": "post_bench_custom",
  "total_requests": $n,
  "ok": $ok,
  "fail": $fail,
  "total_time_sec": $total_time,
  "rps": $rps,
  "latency_sec": {
    "p50": $p50,
    "p95": $p95,
    "p99": $p99
  }
}
EOF
    rm -f "$lats_file"
}

# Main routine per implementation.
benchmark_impl() {
    local name="$1"           # "fitz" o "python"
    local boilerplate="$2"    # nombre del dir en boilerplates/
    local container="$3"      # nombre del container (fitz-api-... o similar)
    local out_dir="$RESULTS_DIR/$name"
    mkdir -p "$out_dir"
    echo ""
    echo "── $name (boilerplate: $boilerplate) ────────────────────────"

    cd "$REPO_ROOT/boilerplates/$boilerplate"

    log "docker compose down -v (clean state)..."
    docker compose down -v --remove-orphans >/dev/null 2>&1 || true

    log "docker compose up -d --build..."
    docker compose up -d --build >"$out_dir/build.log" 2>&1
    if [ $? -ne 0 ]; then
        echo "ERROR: docker compose up falló. Ver $out_dir/build.log" >&2
        tail -20 "$out_dir/build.log" >&2
        return 1
    fi

    # Cold start timing.
    local port
    port=$(docker compose port api 3000 2>/dev/null | sed 's/.*://' || echo "3000")
    [ -z "$port" ] && port=3000
    log "cold start — esperando primer 200 OK en http://localhost:$port/users..."
    local cold_secs
    cold_secs=$(wait_for_200 "http://localhost:$port/users" "$COLD_START_TIMEOUT" "$name") || return 1
    log "cold start: ${cold_secs}s"
    echo "$cold_secs" > "$out_dir/cold_start.sec"

    # Image size — match EXACTO al image que docker-compose genera
    # para el service "api" (formato `<dirname>-api:latest`). El grep
    # anterior con `^api-orm` pescaba imágenes cacheadas de OTROS
    # boilerplates (ej: `api-orm-full-fullstack-api`) cuando estaban
    # presentes en el host. Fix: anchor exacto al image del bench.
    docker images --format "{{.Repository}}:{{.Tag}} {{.Size}}" \
        | grep -E "^${boilerplate}-api:latest " \
        > "$out_dir/image_sizes.txt" 2>/dev/null || true

    # Seed.
    seed_users "http://localhost:$port" "$SEED_USERS"
    sleep "$WAIT_BETWEEN_BENCHES"

    # Memory sampler start (fire-and-forget al background, PID via archivo).
    start_memory_sampler "$container" "$out_dir/mem.log"

    # Bench GET /users (list).
    bench_endpoint "GET /users" "http://localhost:$port/users" \
        "$out_dir/get_users.json"
    sleep "$WAIT_BETWEEN_BENCHES"

    # Bench GET /users/{id} — random id en [1, SEED_USERS].
    # oha hace todas las requests al mismo URL, así que pegamos a /users/1
    # (consistente, fair entre impls; el behavior cache de PG aplica igual).
    bench_endpoint "GET /users/1" "http://localhost:$port/users/1" \
        "$out_dir/get_user_id.json"
    sleep "$WAIT_BETWEEN_BENCHES"

    # Bench POST /users (sequential curl loop con timing manual).
    # n=100 (Git Bash overhead ~1s/iter → 100s; suficiente para
    # p50/p95/p99 representativos sin que el bench tarde 10 min).
    bench_post "POST /users" "http://localhost:$port" \
        "$out_dir/post_users.json" 100
    sleep "$WAIT_BETWEEN_BENCHES"

    # Memory sampler stop + peak.
    stop_memory_sampler "$out_dir/mem.log.pid"
    peak_mb "$out_dir/mem.log" > "$out_dir/mem_peak.mb"
    log "memory peak: $(cat "$out_dir/mem_peak.mb") MB"

    log "docker compose down -v..."
    docker compose down -v --remove-orphans >/dev/null 2>&1 || true
}

# --- Run ------------------------------------------------------------
# Container names: vienen del `container_name:` del docker-compose
# de cada boilerplate. Sin el name correcto, `docker stats` falla
# y el memory sampler nunca escribe — `mem.log` queda inexistente
# y `peak_mb` reporta `?`.
benchmark_impl "fitz" "api-postgres-fitz" "fitz-api-postgres-orm"
benchmark_impl "python" "api-postgres-python" "fitz-api-postgres"

# --- Summary --------------------------------------------------------
echo ""
echo "════════════════════════════════════════════════════════════════"
echo " Generating summary.md..."
bash "$SCRIPT_DIR/summarize.sh" "$RESULTS_DIR" > "$RESULTS_DIR/summary.md"
echo " Done. Results en: $RESULTS_DIR"
echo "         Summary:  $RESULTS_DIR/summary.md"
echo "════════════════════════════════════════════════════════════════"
cat "$RESULTS_DIR/summary.md"
