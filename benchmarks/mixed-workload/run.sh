#!/usr/bin/env bash
# run.sh — benchmark mixed-workload (Fitz vs Python+SQLAlchemy vs Node+Prisma).
#
# Para cada stack:
#   1. docker compose up -d --build (apps/<stack>/)
#   2. Espera primer 200 OK → mide cold start
#   3. Seed: SEED_USERS + 5 posts promedio por user
#   4. Inicia sampler memory + CPU (cada 500ms)
#   5. Corre scenarios/mixed.js (3 min, ramp 10→50→100→50)
#   6. Corre scenarios/reads-only.js (1 min, 50 VUs)
#   7. Corre scenarios/writes-only.js (1 min, 50 VUs)
#   8. Detiene sampler, calcula peaks
#   9. docker compose down -v
# Después: genera summary.md comparativo.
#
# Prerequisitos: k6, jq, docker, docker compose, curl.

set -euo pipefail

# --- Config ---------------------------------------------------------
BENCH_DURATION_MIXED="${BENCH_DURATION_MIXED:-210s}"     # mixed = total stages
BENCH_DURATION_FOCUSED="${BENCH_DURATION_FOCUSED:-60s}"
BENCH_VUS_MAX="${BENCH_VUS_MAX:-100}"
BENCH_VUS_FOCUSED="${BENCH_VUS_FOCUSED:-50}"
SEED_USERS="${SEED_USERS:-200}"
SEED_POSTS_PER_USER="${SEED_POSTS_PER_USER:-5}"
COLD_START_TIMEOUT="${COLD_START_TIMEOUT:-180}"
WAIT_BETWEEN_BENCHES=3
WAIT_AFTER_SEED=5  # damos tiempo a que Postgres analice las stats

# --- Setup paths ----------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
RESULTS_DIR="$SCRIPT_DIR/results/$TIMESTAMP"
mkdir -p "$RESULTS_DIR"

echo "════════════════════════════════════════════════════════════════"
echo " Mixed-workload benchmark — Fitz vs Python+SQLAlchemy vs Node+Prisma"
echo "════════════════════════════════════════════════════════════════"
echo " Timestamp:        $TIMESTAMP"
echo " Results dir:      $RESULTS_DIR"
echo " Mixed VU peak:    $BENCH_VUS_MAX"
echo " Focused VUs:      $BENCH_VUS_FOCUSED"
echo " Focused duration: $BENCH_DURATION_FOCUSED"
echo " Seed users:       $SEED_USERS"
echo " Seed posts/user:  $SEED_POSTS_PER_USER (promedio)"
echo ""

# --- Tool checks ----------------------------------------------------
need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "ERROR: falta '$1'. Instalalo y volvé a correr." >&2
        case "$1" in
            k6)     echo "  https://k6.io/docs/get-started/installation/" >&2 ;;
            jq)     echo "  https://jqlang.github.io/jq/" >&2 ;;
            docker) echo "  https://docs.docker.com/get-docker/" >&2 ;;
        esac
        exit 1
    fi
}
need docker
need k6
need jq
need curl

# --- Helpers --------------------------------------------------------
log() { echo "▸ $(date +%H:%M:%S)  $*"; }

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

start_sampler() {
    local container="$1"
    local out="$2"
    local pidfile="${out}.pid"
    (
        while true; do
            local stats
            stats=$(docker stats --no-stream --format "{{.MemUsage}}|{{.CPUPerc}}" "$container" 2>/dev/null || echo "")
            if [ -n "$stats" ]; then
                local mem="${stats%%|*}"
                local cpu="${stats##*|}"
                local used="${mem%% *}"
                local mb
                case "$used" in
                    *MiB)  mb=$(echo "$used" | sed 's/MiB//' | awk '{printf "%.1f", $1 * 1.048576}') ;;
                    *GiB)  mb=$(echo "$used" | sed 's/GiB//' | awk '{printf "%.1f", $1 * 1073.741824}') ;;
                    *KiB)  mb=$(echo "$used" | sed 's/KiB//' | awk '{printf "%.4f", $1 * 0.001024}') ;;
                    *)     mb="$used" ;;
                esac
                local cpu_n="${cpu%\%}"
                echo "$(date +%s.%N) $mb $cpu_n" >> "$out"
            fi
            sleep 0.5
        done
    ) >/dev/null 2>&1 &
    echo $! > "$pidfile"
}

stop_sampler() {
    local pidfile="$1"
    if [ -f "$pidfile" ]; then
        local pid
        pid=$(cat "$pidfile")
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        rm -f "$pidfile"
    fi
}

peak_mb() {
    local log="$1"
    [ ! -s "$log" ] && { echo "?"; return; }
    awk '{ if ($2+0 > max) max = $2+0 } END { printf "%.1f", max }' "$log"
}

peak_cpu() {
    local log="$1"
    [ ! -s "$log" ] && { echo "?"; return; }
    awk '{ if ($3+0 > max) max = $3+0 } END { printf "%.1f", max }' "$log"
}

# Seed: SEED_USERS users + posts random por user. Inicio temprano si
# los primeros fallan (igual que el bench anterior).
seed_data() {
    local url="$1"
    local n_users="$2"
    local posts_per="$3"
    log "  seeding $n_users users + ~$posts_per posts/user..."
    local ok=0 fail=0
    local first_fail_logged=0
    for i in $(seq 1 "$n_users"); do
        local code
        code=$(curl -s -o /tmp/seed_user_resp.json -w "%{http_code}" --max-time 10 \
                    -X POST "$url/users" \
                    -H "Content-Type: application/json" \
                    -d "{\"name\":\"Seed $i\",\"email\":\"seed-$TIMESTAMP-$i@bench.example.com\"}" \
                    || echo "000")
        if [ "$code" = "200" ]; then
            ok=$((ok+1))
            # Extraer id del user creado para popular posts.
            local user_id
            user_id=$(jq -r '.id // empty' /tmp/seed_user_resp.json 2>/dev/null || echo "")
            if [ -n "$user_id" ] && [ "$user_id" != "null" ]; then
                # Distribución uniforme 0..2*posts_per (promedio = posts_per).
                local n_posts=$(( (RANDOM % (posts_per * 2 + 1)) ))
                for j in $(seq 1 "$n_posts"); do
                    curl -s -o /dev/null --max-time 5 \
                         -X POST "$url/users/$user_id/posts" \
                         -H "Content-Type: application/json" \
                         -d "{\"title\":\"Seed Post $i.$j\",\"body\":\"Seed body of post $j for user $i\"}" \
                         || true
                done
            fi
        else
            fail=$((fail+1))
            if [ "$first_fail_logged" -eq 0 ]; then
                local err
                err=$(cat /tmp/seed_user_resp.json 2>/dev/null || echo "")
                log "    primer fail ($code): $err"
                first_fail_logged=1
            fi
            if [ "$i" -le 5 ] && [ "$fail" -eq "$i" ]; then
                echo "ERROR: primeras $fail requests del seed fallaron." >&2
                return 1
            fi
        fi
        if [ $((i % 20)) -eq 0 ]; then
            log "    seeded $i/$n_users (ok=$ok fail=$fail)"
        fi
    done
    log "  seed done: ok=$ok fail=$fail"
    rm -f /tmp/seed_user_resp.json
}

run_k6() {
    local script="$1"
    local out_json="$2"
    local extra_env="${3:-}"
    log "  k6 run $script → $out_json..."
    # k6 con --summary-export para JSON oficial + --quiet para
    # silencio del progress bar (ruido en logs cuando no hay TTY).
    #
    # k6 EXIT CODES (relevant):
    #   0   — todos los checks y thresholds OK
    #   99  — al menos un threshold cruzó (`p(95)<500ms` etc); el
    #         scenario completó normal, el JSON output ES válido.
    #   1, 105+ — bugs reales (network, crash, script error). Acá sí
    #         abortamos el bench.
    #
    # Bajo carga peak (100 VUs mixed sobre Python con GIL), los
    # thresholds del scenario son EXPECTABLE crossing — eso muestra
    # saturation real del stack. Ignoramos 99 para no perder la data
    # de los stacks siguientes ni el summary final.
    set +e
    BASE_URL=http://localhost:3000 \
    SEED_USERS="$SEED_USERS" \
    BENCH_VUS_MAX="$BENCH_VUS_MAX" \
    BENCH_VUS_FOCUSED="$BENCH_VUS_FOCUSED" \
    BENCH_DURATION_FOCUSED="$BENCH_DURATION_FOCUSED" \
    RUN_TAG="$TIMESTAMP" \
    $extra_env \
    k6 run --quiet --summary-export="$out_json" "$SCRIPT_DIR/$script" \
       2> "${out_json}.stderr"
    local k6_exit=$?
    set -e

    if [ "$k6_exit" -eq 0 ]; then
        return 0
    elif [ "$k6_exit" -eq 99 ]; then
        log "  ⚠ thresholds violados (esperado bajo carga peak) — data válida en $out_json"
        return 0
    else
        echo "ERROR: k6 falló sobre $script (exit $k6_exit)" >&2
        echo "--- stderr ---" >&2
        tail -50 "${out_json}.stderr" >&2
        return 1
    fi
}

# Main routine per stack.
benchmark_impl() {
    local name="$1"           # fitz / python / node
    local app_dir="$2"        # apps/<name>
    local container="$3"      # nombre del container API
    local out_dir="$RESULTS_DIR/$name"
    mkdir -p "$out_dir"
    echo ""
    echo "──── $name (app: $app_dir) ──────────────────────────────────"

    cd "$REPO_ROOT/benchmarks/mixed-workload/$app_dir"

    log "docker compose down -v (clean state)..."
    docker compose down -v --remove-orphans >/dev/null 2>&1 || true

    log "docker compose up -d --build..."
    docker compose up -d --build >"$out_dir/build.log" 2>&1 || {
        echo "ERROR: docker compose up falló. Ver $out_dir/build.log" >&2
        tail -30 "$out_dir/build.log" >&2
        return 1
    }

    # Cold start.
    local port
    port=$(docker compose port api 3000 2>/dev/null | sed 's/.*://' || echo "3000")
    [ -z "$port" ] && port=3000
    log "cold start — esperando primer 200 OK en http://localhost:$port/users..."
    local cold_secs
    cold_secs=$(wait_for_200 "http://localhost:$port/users" "$COLD_START_TIMEOUT" "$name") || return 1
    log "cold start: ${cold_secs}s"
    echo "$cold_secs" > "$out_dir/cold_start.sec"

    # Image size del API.
    docker images --format "{{.Repository}}:{{.Tag}} {{.Size}}" \
        | grep -E "^${app_dir##*/}-api:latest " \
        > "$out_dir/image_sizes.txt" 2>/dev/null || true

    # Seed.
    seed_data "http://localhost:$port" "$SEED_USERS" "$SEED_POSTS_PER_USER"
    sleep "$WAIT_AFTER_SEED"

    # Sampler start (memory + CPU peak).
    start_sampler "$container" "$out_dir/stats.log"

    # Scenarios.
    run_k6 "scenarios/mixed.js"       "$out_dir/mixed.json"
    sleep "$WAIT_BETWEEN_BENCHES"
    run_k6 "scenarios/reads-only.js"  "$out_dir/reads_only.json"
    sleep "$WAIT_BETWEEN_BENCHES"
    run_k6 "scenarios/writes-only.js" "$out_dir/writes_only.json"
    sleep "$WAIT_BETWEEN_BENCHES"

    # Stop sampler + peaks.
    stop_sampler "$out_dir/stats.log.pid"
    peak_mb "$out_dir/stats.log" > "$out_dir/mem_peak.mb"
    peak_cpu "$out_dir/stats.log" > "$out_dir/cpu_peak.pct"
    log "memory peak: $(cat "$out_dir/mem_peak.mb") MB"
    log "CPU peak:    $(cat "$out_dir/cpu_peak.pct") %"

    log "docker compose down -v..."
    docker compose down -v --remove-orphans >/dev/null 2>&1 || true
}

# --- Run ------------------------------------------------------------
# (container_name viene del docker-compose de cada app — `bench-mw-api-<stack>`)
benchmark_impl "fitz"   "apps/fitz"   "bench-mw-api-fitz"
benchmark_impl "python" "apps/python" "bench-mw-api-python"
benchmark_impl "node"   "apps/node"   "bench-mw-api-node"

# --- Summary --------------------------------------------------------
echo ""
echo "════════════════════════════════════════════════════════════════"
echo " Generating summary.md..."
bash "$SCRIPT_DIR/summarize.sh" "$RESULTS_DIR" > "$RESULTS_DIR/summary.md"
echo " Done. Results en: $RESULTS_DIR"
echo "          Summary: $RESULTS_DIR/summary.md"
echo "════════════════════════════════════════════════════════════════"
cat "$RESULTS_DIR/summary.md"
