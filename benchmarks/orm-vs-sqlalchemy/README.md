# Benchmark — Fitz ORM nativo vs SQLAlchemy

**Comparación cabeza-a-cabeza** entre los dos boilerplates equivalentes:

| Impl | Boilerplate | Stack |
|---|---|---|
| Fitz ORM | [`api-postgres-fitz`](../../boilerplates/api-postgres-fitz/) | Driver Postgres v3.0 puro + ORM nativo de Fitz |
| Python | [`api-postgres-python`](../../boilerplates/api-postgres-python/) | Fitz + `from python import` + SQLAlchemy 2.x |

Ambos exponen los **mismos 3 endpoints** con misma firma:
- `GET /users` — lista
- `GET /users/{id}` — uno
- `POST /users` — crear

Misma DB Postgres 16, misma red Docker, mismo host. Solo cambia el
ORM/driver detrás.

## Métricas

- **Cold start** — segundos desde `docker compose up -d` hasta primer
  `200 OK` en `/users`.
- **Latencia** p50/p95/p99 (ms) por endpoint con
  [`oha`](https://github.com/hatoo/oha) (30s sostenido, c=10).
- **Throughput** (RPS) por endpoint.
- **Memory peak** (MB) via `docker stats` muestreado cada 500ms.
- **Image size** del container de la API.

## Prerequisitos

```bash
# oha (single binary, ~5 MB)
cargo install oha
#   o release pre-built: https://github.com/hatoo/oha/releases

# jq (parsing JSON)
#   Linux: apt install jq
#   macOS: brew install jq
#   Windows: scoop install jq  /  choco install jq

# docker + docker compose (estándar)
```

Verificar:

```bash
oha --version
jq --version
docker --version
docker compose version
```

## Correr

```bash
cd benchmarks/orm-vs-sqlalchemy
bash run.sh
```

Esto:
1. Para cada implementación (fitz, python):
   1. `docker compose up -d --build` del boilerplate.
   2. Espera primer 200 OK → mide cold start.
   3. Pre-seedea 200 users via POST.
   4. Inicia sampler de memory peak en background.
   5. Bencha `GET /users` con oha (30s, c=10) → JSON.
   6. Bencha `GET /users/1` con oha (30s, c=10) → JSON.
   7. Bencha `POST /users` con curl loop (500 sequential, emails únicos) → JSON.
   8. Detiene sampler, calcula peak.
   9. `docker compose down -v` (clean state).
2. Genera `results/<timestamp>/summary.md` con tabla comparativa.

**Tiempo total estimado**: ~10-15 min (build inicial Python ~3-5min,
Fitz ~30-60s con `ghcr.io/.../fitz:latest`).

### Config via env vars

```bash
BENCH_DURATION=60s BENCH_CONCURRENCY=20 SEED_USERS=500 bash run.sh
```

Defaults:
- `BENCH_DURATION=30s`
- `BENCH_CONCURRENCY=10`
- `SEED_USERS=200`
- `COLD_START_TIMEOUT=120` (s máximos esperando primer 200)

## Output

```
results/
  20260529-181530/
    fitz/
      build.log
      cold_start.sec
      mem.log
      mem_peak.mb
      image_sizes.txt
      get_users.json         # output oha (formato JSON oficial)
      get_user_id.json
      post_users.json        # formato custom (post_bench_custom)
    python/
      ...mismos archivos...
    summary.md               # tabla comparativa Markdown
```

`summary.md` se imprime en stdout al final del run y queda commiteable.

## Interpretación

- **Speedup**: lo leemos como _"Fitz es Nx más rápido"_:
  - Latencia: `speedup = python_lat / fitz_lat` (menor mejor → ratio > 1 favorece Fitz).
  - RPS: `speedup = fitz_rps / python_rps` (mayor mejor → ratio > 1 favorece Fitz).
  - Memory / cold start: análogo a latencia (menor mejor).

- **Variabilidad**: corridas locales tienen ±10% por CPU thermals,
  otros procesos, estado del cache PG. Para resultados publicables
  correr 3 veces y reportar mediana.

- **POST sequential ≠ POST concurrent**: medimos latencia honesta
  con emails únicos por request, pero NO throughput concurrente con
  bodies aleatorios. Para eso hace falta `k6` o `wrk+lua` (queda
  como extensión futura).

- **Memory peak puede subestimar**: muestreo cada 500ms; peaks
  transientes <500ms se pierden. Para precisión mayor usar
  `cgroup memory.peak` o muestreo más frecuente.

## Resultados publicables

Cuando tengas un run que querés publicar, copiá el `summary.md` al
final de este README bajo `## Última corrida (YYYY-MM-DD)` con
hardware anotado a mano:

```markdown
## Última corrida (2026-05-29)

- CPU: Intel i7-12700K / Ryzen 5800X / Apple M2 / ...
- RAM: 32 GB
- OS: Windows 11 / Linux 6.x / macOS Sonoma
- Docker: 25.0.x (Desktop / Engine)

<paste summary.md aquí>
```

## Notas técnicas

- **Por qué Fitz tiende a ganar**: el driver Postgres es código Rust
  puro compilado al binario nativo, sin libpq, sin libpython, sin
  GIL, sin marshalling Python ↔ Rust. Cada request HTTP usa solo
  tokio + axum + el driver — runtime overhead ~0. SQLAlchemy
  agrega capas: parsing SQL Python-side, connection pool con
  threading.Lock, conversión rows → ORM instances con `__init__`
  por row, GIL serializa todo eso.

- **Por qué Python no es ridículamente lento**: SQLAlchemy 2.x es
  muy optimizado, el GIL solo bloquea Python puro (no SQL execution
  ni I/O). Para queries DB-bound (el caso típico), el cuello de
  botella es Postgres, no Python. Esperar diferencias del orden
  ~1.2x-3x, no 10x.

- **Qué NO testeamos en MVP**:
  - Mixed workload realista (reads + writes intercalados).
  - Bulk inserts (1k+ rows en una transaction).
  - Queries con JOINs / preload eager loading (necesita api-orm-full).
  - Escritura concurrente con saturación del pool.

  Quedan como extensiones futuras (`run-extended.sh` cuando sea
  necesario).

---

## Última corrida publicable (2026-05-29) — v0.10.13

**Hardware**:
- CPU: Intel Core Ultra 7 155H (Meteor Lake, 16 cores)
- RAM: 64 GB
- OS: Windows 11 Pro
- Docker: 29.2.1 (Desktop con WSL2 backend)

**Versión Fitz**: `ghcr.io/thegreekman76/fitz:v0.10.13`
(incluye **B-1 fix** del driver Postgres: TCP_NODELAY + batch de los
5 mensajes del Extended Query Protocol — eliminó ~40ms de overhead
constante en queries con parámetros, ver `docs/deudas-post-5b.md`
sección "Deudas detectadas en el primer bench" → B-1).

### Headline

> **Fitz ORM es 5-10x más rápido y 5x más eficiente en memoria que
> Python+SQLAlchemy** en read workloads (sustained 30s, c=10).
> Empate técnico en write workload (POST es bottleneck del bench
> mismo, no del server).

### Cold start, image, memory

| Métrica | Fitz ORM | Python (SQLAlchemy) | Speedup Fitz |
|---|---:|---:|---:|
| Cold start (s) | **0.14** | 0.22 | 1.57x |
| Image size | 131 MB | 258 MB | **2x más liviano** |
| Memory peak (MB) | **9.2** | 51.0 | **5.54x más eficiente** |

### Latencia + throughput por endpoint

#### `GET /users` (lista de 50 rows, 30s sustained, c=10)

| Métrica | Fitz ORM | Python (SQLAlchemy) | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | **4.88** | 37.85 | **7.76x** |
| p95 latency (ms) | **7.68** | 68.01 | **8.86x** |
| p99 latency (ms) | **10.26** | 87.17 | **8.49x** |
| Throughput (RPS) | **1944** | 246 | **7.91x** |
| Total requests | 58,340 | 7,376 | — |
| Success rate | 100% | 100% | — |

#### `GET /users/{id}` (single read por PK, 30s sustained, c=10) ⭐

| Métrica | Fitz ORM | Python (SQLAlchemy) | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | **3.60** | 31.87 | **8.85x** |
| p95 latency (ms) | **5.85** | 56.17 | **9.60x** |
| p99 latency (ms) | **8.62** | 71.78 | **8.33x** |
| Throughput (RPS) | **2604** | 296 | **8.80x** |
| Total requests | 78,138 | 8,885 | — |
| Success rate | 100% | 100% | — |

> **Antes del B-1 fix** (v0.10.12): Fitz tenía p50=43.70ms aquí —
> ~30% MÁS LENTO que Python. El batching de Extended Query + Nagle
> off lo bajó a **3.60ms, 12x más rápido que antes** y 8.85x más
> rápido que Python.

#### `POST /users` (100 sequential, body único por request)

| Métrica | Fitz ORM | Python (SQLAlchemy) | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | 108.13 | 109.32 | ~empate |
| p95 latency (ms) | 188.74 | 184.67 | ~empate |
| p99 latency (ms) | 275.27 | 202.96 | 0.74x (Python wins) |
| Throughput (RPS) | 4.83 | 5.23 | 0.92x |

> **POST es bottleneck del cliente**, no del server. El script de
> bench hace `curl` sequential con email único por request — en
> Git Bash Windows cada subshell tarda ~1s overhead. Para medir
> POST throughput real necesitaríamos `k6` o `wrk+lua` con body
> randomization. Queda como extensión futura.

### Cómo se reproduce

```bash
cd benchmarks/orm-vs-sqlalchemy
bash run.sh
```

El script:
1. `docker compose up -d --build` de cada boilerplate (usa
   `ghcr.io/thegreekman76/fitz:latest{,−python}` pre-built).
2. Seed 50 users via POST.
3. Bench `GET /users` con oha 30s c=10 → JSON.
4. Bench `GET /users/1` con oha 30s c=10 → JSON.
5. Bench `POST /users` con curl loop 100 sequential.
6. Memory peak via `docker stats` muestreado cada 500ms.
7. `docker compose down -v` (clean state).
8. Genera `results/<timestamp>/summary.md` con tablas comparativas.

Tiempo total run: ~5-8 min (con imágenes ya cacheadas).

### Variabilidad esperada

±10% entre corridas. Para resultados más estables correr 3 veces y
reportar mediana. Los **headline numbers** (5.54x memory, 8x reads)
son consistentes entre corridas; solo cambian decimales.
