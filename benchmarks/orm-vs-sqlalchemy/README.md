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

## Última corrida publicable (2026-08-11) — v0.37.12 (mediana de 3 corridas)

**Hardware**:
- CPU: Intel Core Ultra 7 155H (Meteor Lake, 16 cores)
- RAM: 64 GB
- OS: Windows 11 Pro
- Docker: 29.2.1 (Desktop con WSL2 backend)

**Versión Fitz**: `ghcr.io/thegreekman76/fitz:v0.37.12` — mediana de
3 corridas. El driver Postgres es byte-idéntico al de v0.37.8 en el
hot path (incluye el **B-1 fix** desde v0.10.13: TCP_NODELAY + batch de
los 5 mensajes del Extended Query Protocol, ver `docs/deudas-post-5b.md`
→ B-1). v0.37.12 revalidó los números **sin regresión**: el fix de
`FITZ_DB_*` mid-run reload agrega una lectura de env var por query en
`log_db_query`, despreciable en workload network-bound.

### Headline

> **Fitz ORM es ~8x más rápido y 5.7x más eficiente en memoria que
> Python+SQLAlchemy** en read workloads (sustained 30s, c=10).
> Empate técnico en write workload (POST es bottleneck del bench
> mismo, no del server).

### Cold start, image, memory

| Métrica | Fitz ORM | Python (SQLAlchemy) | Ratio |
|---|---:|---:|---:|
| Cold start (s) | 0.34 | **0.31** | 0.91x (~empate) |
| Image size | **134 MB** | 272 MB | **2x más liviano** |
| Memory peak (MB) | **9.2** | 52.4 | **5.7x más eficiente** |

### Latencia + throughput por endpoint

#### `GET /users` (lista de 50 rows, 30s sustained, c=10)

| Métrica | Fitz ORM | Python (SQLAlchemy) | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | **3.57** | 31.24 | **8.75x** |
| p95 latency (ms) | **5.76** | 56.75 | **9.85x** |
| p99 latency (ms) | **8.22** | 72.39 | **8.81x** |
| Throughput (RPS) | **2618** | 297 | **8.81x** |
| Total requests | 78,580 | 8,915 | — |
| Success rate | 100% | 100% | — |

#### `GET /users/{id}` (single read por PK, 30s sustained, c=10) ⭐

| Métrica | Fitz ORM | Python (SQLAlchemy) | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | **2.74** | 21.52 | **7.85x** |
| p95 latency (ms) | **4.51** | 44.07 | **9.77x** |
| p99 latency (ms) | **6.44** | 61.50 | **9.55x** |
| Throughput (RPS) | **3377** | 411 | **8.22x** |
| Total requests | 101,341 | 12,334 | — |
| Success rate | 100% | 100% | — |

> **El B-1 fix** (v0.10.13): batching del Extended Query Protocol +
> Nagle off bajó este single-read por PK de p50=43.70ms (v0.10.12,
> ~30% MÁS LENTO que Python) a ~2.7ms — ~8x más rápido que Python,
> estable desde entonces (v0.37.12: p50 2.74ms mediana).

#### `POST /users` (100 sequential, body único por request)

| Métrica | Fitz ORM | Python (SQLAlchemy) | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | 174.69 | 190.23 | 1.09x |
| p95 latency (ms) | 243.97 | 271.75 | 1.11x |
| p99 latency (ms) | 304.35 | 372.30 | 1.22x |
| Throughput (RPS) | 3.28 | 2.98 | 1.10x |

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
5. Bench `POST /users` con curl loop 500 sequential.
6. Memory peak via `docker stats` muestreado cada 500ms.
7. `docker compose down -v` (clean state).
8. Genera `results/<timestamp>/summary.md` con tablas comparativas.

Tiempo total run: ~5-8 min (con imágenes ya cacheadas).

### Variabilidad esperada

±10% entre corridas. Para resultados más estables correr 3 veces y
reportar mediana. Los **headline numbers** (5.7x memory, ~8x reads)
son consistentes entre corridas; solo cambian decimales.
