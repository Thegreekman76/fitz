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
