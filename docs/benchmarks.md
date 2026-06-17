# Benchmarks

Página dedicada a las comparaciones de performance del lenguaje
contra alternativas equivalentes. Los benchmarks son
**reproducibles**, viven en el repo bajo
[`benchmarks/`](https://github.com/Thegreekman76/fitz/tree/main/benchmarks),
y se corren contra **boilerplates funcionalmente equivalentes** —
mismo dominio, mismos endpoints, misma DB.

!!! tip "Filosofía"
    No publicamos números que no podamos reproducir. Cada bench
    tiene un `run.sh` ejecutable + las versiones exactas del software
    + el hardware del run. El lector puede correrlo en su máquina y
    verificar (espera ±10% de variabilidad por CPU thermals y cache
    state).

---

## Fitz ORM nativo vs SQLAlchemy

**Comparación cabeza-a-cabeza** entre los dos boilerplates equivalentes:

| Implementación | Boilerplate | Stack |
|---|---|---|
| **Fitz ORM nativo** | [`api-postgres-fitz`](https://github.com/Thegreekman76/fitz/tree/main/boilerplates/api-postgres-fitz) | Driver Postgres v3.0 puro escrito en Rust + ORM declarativo nativo del lenguaje |
| **Python+SQLAlchemy** | [`api-postgres-python`](https://github.com/Thegreekman76/fitz/tree/main/boilerplates/api-postgres-python) | Fitz + `from python import` + SQLAlchemy 2.x + psycopg2 |

Ambos exponen los mismos 3 endpoints (`GET /users`, `GET /users/{id}`,
`POST /users`) con misma firma de body. Misma DB Postgres 16-alpine,
misma red Docker, mismo host.

### Headline numbers (v0.10.13, 2026-05-29)

!!! success "Fitz ORM es 5-10x más rápido y 5.5x más eficiente en memoria"
    Read workloads sustained (30s, c=10) — el caso típico de un
    servicio HTTP que sirve API REST. Empate técnico en write workload
    (POST es bottleneck del bench mismo, no del server).

**Hardware del run**: Intel Core Ultra 7 155H (Meteor Lake, 16 cores),
64 GB RAM, Windows 11 Pro, Docker 29.2.1 (Desktop con WSL2 backend).
**Versión**: `ghcr.io/thegreekman76/fitz:v0.10.13`.

#### Cold start, image, memory

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup Fitz |
|---|---:|---:|---:|
| Cold start (s) | **0.14** | 0.22 | 1.57x |
| Image size | 131 MB | 258 MB | **2x más liviano** |
| Memory peak (MB) | **9.2** | 51.0 | **5.54x más eficiente** |

#### `GET /users` — lista de 50 rows, sustained 30s c=10

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | **4.88** | 37.85 | **7.76x** |
| p95 latency (ms) | **7.68** | 68.01 | **8.86x** |
| p99 latency (ms) | **10.26** | 87.17 | **8.49x** |
| Throughput (RPS) | **1944** | 246 | **7.91x** |
| Total requests | 58,340 | 7,376 | — |
| Success rate | 100% | 100% | — |

#### `GET /users/{id}` — single read por PK, sustained 30s c=10 ⭐

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | **3.60** | 31.87 | **8.85x** |
| p95 latency (ms) | **5.85** | 56.17 | **9.60x** |
| p99 latency (ms) | **8.62** | 71.78 | **8.33x** |
| Throughput (RPS) | **2604** | 296 | **8.80x** |
| Total requests | 78,138 | 8,885 | — |
| Success rate | 100% | 100% | — |

!!! note "Historia del fix B-1 (v0.10.13)"
    En el bench v0.10.12, `GET /users/{id}` tenía p50=43.70ms — un
    ~30% MÁS LENTO que Python. La investigación dedicada
    ([deuda B-1 en `deudas-post-5b.md`](deudas-post-5b.md)) reveló
    que el driver Postgres mandaba los 5 mensajes del Extended Query
    Protocol (Parse/Bind/Describe/Execute/Sync) con `self.write(...).await`
    separados → Nagle's algorithm sumaba ~40ms de delayed-ACK por
    query parametrizada.

    **Fix doble en `src/db.rs`**:

    1. `set_nodelay(true)` al construir el `TcpStream` (deshabilita
       Nagle entre el cliente y el server).
    2. Batch los 5 mensajes en un solo `write_all_bytes(...)`.

    Resultado: GET /users/{id} pasó de **43.70ms → 3.60ms p50**
    (**12x más rápido**), de "Fitz pierde" a "Fitz gana 8.85x".

#### `POST /users` — 100 sequential con email único por request

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | 108.13 | 109.32 | ~empate |
| p95 latency (ms) | 188.74 | 184.67 | ~empate |
| p99 latency (ms) | 275.27 | 202.96 | 0.74x (Python wins) |
| Throughput (RPS) | 4.83 | 5.23 | 0.92x |

!!! warning "POST mide el cliente, no el server"
    El script de bench hace `curl` sequential con email único por
    request — en Git Bash Windows cada subshell tarda ~1s de
    overhead. Para medir POST throughput **honesto** necesitaríamos
    `k6` o `wrk+lua` con body randomization. Queda como extensión
    futura del bench.

    Lo que SÍ se ve: la latencia per-request es ~empate, lo que
    indica que el cuello de botella es Postgres (write durable), no
    el ORM/driver de cada lado.

### Cómo reproducir

```bash
cd benchmarks/orm-vs-sqlalchemy
bash run.sh
```

El script:

1. `docker compose up -d --build` de cada boilerplate (usa
   `ghcr.io/thegreekman76/fitz:latest` y `:latest-python` pre-built).
2. Seed 50 users via POST.
3. Bench `GET /users` con [`oha`](https://github.com/hatoo/oha) 30s c=10 → JSON.
4. Bench `GET /users/1` con `oha` 30s c=10 → JSON.
5. Bench `POST /users` con curl loop 100 sequential.
6. Memory peak via `docker stats` muestreado cada 500ms.
7. `docker compose down -v` (clean state).
8. Genera `results/<timestamp>/summary.md` con tablas comparativas.

**Prerequisitos**: `oha` (`cargo install oha`), `jq`, Docker. **Tiempo
total**: ~5-8 min con cache Docker caliente.

Detalle completo en
[`benchmarks/orm-vs-sqlalchemy/README.md`](https://github.com/Thegreekman76/fitz/blob/main/benchmarks/orm-vs-sqlalchemy/README.md).

### Por qué Fitz tiende a ganar

- **Driver Postgres puro** en Rust, compilado al binario nativo. Sin
  libpq (la lib C oficial de Postgres), sin libpython, sin GIL, sin
  marshalling Python ↔ Rust por cada row. Cada request HTTP usa solo
  tokio + axum + el driver — runtime overhead ~0.
- **SQL constante en codegen-time**. Cada `.where(closure)` se walka
  del AST DURANTE EL CODEGEN, fragmento SQL hard-coded en el binario
  emitido. No hay parsing SQL en runtime ni construcción de
  prepared statements via objetos. Comparable a Diesel/sqlx, mejor
  que SQLAlchemy/ActiveRecord.
- **Extended Query Protocol batched** (v0.10.13+). Los 5 mensajes
  del protocol van en un solo `write()` al socket, sin Nagle delays
  ni round-trips intermedios.

### Por qué Python no es ridículamente lento

SQLAlchemy 2.x es muy optimizado, el GIL solo bloquea Python puro
(no SQL execution ni I/O TCP). Para queries DB-bound (el caso típico
de un servicio CRUD), el cuello de botella suele ser Postgres mismo,
no el ORM/driver. Por eso esperar diferencias del orden ~1.2x-3x es
razonable.

**Las diferencias que vemos (5-10x)** se explican por:

- **Concurrencia bajo carga**. A c=10 sustained, Python+GIL serializa
  el parsing/construcción de respuestas; Fitz+tokio paraleliza sobre
  cores. Por eso el throughput es 7-8x, no solo el p50.
- **Memory footprint**. Python+SQLAlchemy carga libpython + ORM +
  models + connection pool con threading.Lock. Fitz es un solo
  binario Rust con tokio + axum + el driver. Diferencia ~5-6x.

### Qué no testeamos en este bench

El bench `orm-vs-sqlalchemy` mide latencia/throughput aislado por
endpoint con concurrencia fija — buena foto del **ceiling** de cada
operación, pero no del patrón real de un servicio en producción.
Los gaps los cubre el [bench mixed workload](#mixed-workload-fitz-vs-pythonsqlalchemy-vs-nodeprisma)
abajo. Lo que sigue afuera del MVP:

- Bulk inserts (1k+ rows en una transaction).
- Queries con JOINs profundos / preload eager loading sobre el
  `api-orm-full` base.

---

## Mixed workload (Fitz vs Python+SQLAlchemy vs Node+Prisma)

**Tres stacks side-by-side**, mismo dominio (`users` + `posts` con
FK), mismos 6 endpoints, mismo Postgres 16 — solo cambia el stack
de la API:

| Implementación | App | Stack |
|---|---|---|
| **Fitz ORM nativo** | [`apps/fitz/`](https://github.com/Thegreekman76/fitz/tree/main/benchmarks/mixed-workload/apps/fitz) | Driver Postgres puro + ORM nativo (cap 31 guía) |
| **Python+SQLAlchemy** | [`apps/python/`](https://github.com/Thegreekman76/fitz/tree/main/benchmarks/mixed-workload/apps/python) | Fitz + `from python import` + SQLAlchemy 2.x + psycopg2 |
| **Node+Prisma** | [`apps/node/`](https://github.com/Thegreekman76/fitz/tree/main/benchmarks/mixed-workload/apps/node) | Node 20 + Express 5 + Prisma 5 |

Workload: **60% reads + 40% writes intercalados** con VUs rampeando
**10 → 50 → 100 → 50 sobre 3 minutos** vía
[`k6`](https://k6.io/) (no `oha` como en el bench anterior — la
diferencia es scripting de scenarios). Endpoints ejercitados:

- `GET /users?limit=N` (30% del mix) — lista paginada
- `GET /users/{id}/posts` (15%) — JOIN realista
- `GET /users/{id}` (15%) — single read
- `POST /users` (20%) — write
- `POST /users/{id}/posts` (15%) — write con FK
- `PUT /users/{id}` (5%) — update

### Por qué este bench (vs el de arriba)

| Eje | `orm-vs-sqlalchemy` | `mixed-workload` |
|---|---|---|
| Workload | Single-endpoint aislado | Mix realista 60/40 intercalado |
| Concurrencia | Fija c=10 | VUs rampeando 10→100 |
| Writes concurrentes | No (curl loop) | Sí (cada VU su goroutine k6) |
| JOINs | No (`users` solo) | Sí (`/users/{id}/posts`) |
| Saturation point | No mide | Sí (ramp-up detecta knee) |
| p99.9 | No expuesto | Sí |
| Stacks | 2 (Fitz, Python) | 3 (+ Node) |

Cubre la deuda explícita del bench anterior: "POST throughput con
concurrencia real queda como extensión futura".

### Headline numbers (2026-06-17)

!!! success "Fitz mantiene p95 de 11 ms bajo carga peak; Python satura a 503 ms p95"
    Bajo mixed workload sostenido 3 min con peak 100 VUs
    concurrentes, Fitz mantiene latencia bajo 50 ms hasta el p99.9.
    Python+SQLAlchemy satura: cruza el threshold de
    p95<500ms (506ms reales) — sin errores, sin timeouts, pero
    cola de medio segundo por requests CRUD triviales. Node+Prisma
    queda en el medio con **11.7x más memoria** que Fitz.

**Hardware del run**: Intel Core Ultra 7 155H, 64 GB RAM, Windows 11
Pro, Docker 29.2.1 (Desktop con WSL2 backend).

#### Cold start, image, memory, CPU

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Cold start (s) | **0.15** | 0.81 | 2.22 | **5.4x** | **14.8x** |
| Image size | **131 MB** | 268 MB | 437 MB | **2.0x** | **3.3x** |
| Memory peak (MB) | **14.0** | 61.1 | 163.4 | **4.4x** | **11.7x** |
| CPU peak (%) | 131.0 | 171.1 | 215.3 | 1.3x | 1.6x |

#### Mixed workload (3 min, ramp 10→50→100→50, 60/40 reads/writes)

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Total reqs | **97,303** | 34,466 | 82,486 | 2.82x | 1.18x |
| Throughput (RPS) | **463.1** | 164.0 | 392.6 | **2.82x** | 1.18x |
| p50 latency (ms) | **4.58** | 165.74 | 14.67 | **36.2x** | **3.20x** |
| p95 latency (ms) | **11.07** | 502.75 | 69.32 | **45.4x** | **6.26x** |
| p99 latency (ms) | **18.90** | 638.16 | 92.11 | **33.8x** | **4.87x** |
| p99.9 latency (ms) | **45.22** | 839.33 | 172.78 | **18.6x** | **3.82x** |
| Error rate (%) | 0.00 | 0.00 | 0.00 | empate | empate |

!!! note "Python cruzó dos thresholds bajo el peak"
    Los scenarios k6 declaran `p(50)<100ms` y `p(95)<500ms` como
    thresholds. Python+SQLAlchemy las violó (165ms p50, 506ms p95) —
    esto **no es bug del bench**, es exactamente la métrica que
    valida la dirección: el stack saturó bajo el peak. El error
    rate sigue en 0% (sin timeouts) pero la cola crece.

#### Reads-only (1 min, 50 VUs sostenidos)

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Throughput (RPS) | **900.4** | 261.9 | 628.9 | **3.44x** | 1.43x |
| p50 (ms) | **4.20** | 132.56 | 26.35 | **31.6x** | **6.27x** |
| p95 (ms) | **8.64** | 235.50 | 57.74 | **27.3x** | **6.68x** |
| p99 (ms) | **15.16** | 299.79 | 82.47 | **19.8x** | **5.44x** |
| Error rate (%) | 0.00 | 0.00 | 0.00 | empate | empate |

#### Writes-only (1 min, 50 VUs sostenidos) ⭐

!!! tip "Este scenario llena el gap del bench anterior"
    `orm-vs-sqlalchemy` reportaba "POST mide el cliente, no el
    server" — el test era curl-loop secuencial. Acá vemos write
    concurrency real con saturación del pool de cada ORM: **Fitz
    mantiene 5x mayor RPS y 31x mejor p95** que Python+SQLAlchemy.

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Throughput (RPS) | **846.9** | 169.6 | 577.4 | **4.99x** | 1.47x |
| p50 (ms) | **7.86** | 234.80 | 33.14 | **29.9x** | **4.22x** |
| p95 (ms) | **12.54** | 392.73 | 69.19 | **31.3x** | **5.52x** |
| p99 (ms) | **20.89** | 480.05 | 94.43 | **23.0x** | **4.52x** |
| Error rate (%) | 0.00 | 0.00 | 0.00 | empate | empate |

### Cómo reproducir

```bash
cd benchmarks/mixed-workload
bash run.sh
```

El script orquesta los 3 stacks secuencialmente:

1. `docker compose up -d --build` de cada app.
2. Seed: 200 users + ~5 posts/user (1000 posts promedio).
3. Sampler background memory + CPU (cada 500ms).
4. Corre los 3 scenarios k6 (mixed + reads-only + writes-only).
5. `docker compose down -v` + siguiente stack.
6. Genera `summary.md` con tablas + hardware auto-detectado.

**Prerequisitos**: `k6`, `jq`, `docker`, `curl`. **Tiempo total**:
~25-35 min con imágenes Docker cacheadas. Detalle reproducible en
[`benchmarks/mixed-workload/README.md`](https://github.com/Thegreekman76/fitz/blob/main/benchmarks/mixed-workload/README.md).

### Por qué Fitz tiende a ganar bajo carga mixed

Mismas razones que el bench anterior (driver Postgres puro Rust,
SQL constante codegen-time, Extended Query Protocol batched) **más**:

- **Async nativo + tokio multi-thread**: cada handler HTTP es una
  task tokio sobre work-stealing scheduler — el peak de 100 VUs
  concurrentes paraleliza sobre los 16 cores sin GIL ni event loop
  bloqueado.
- **Cero marshaling JSON intermedio**: `__ToFitzJson` impl emitido
  para cada `type` en codegen-time va directo a bytes — sin
  Pydantic + dict + round-trip.
- **Connection pool nativo** (`parking_lot::Mutex` + Arc) sin GIL
  serializando checkouts.

### Por qué Python satura tan fuerte

A 100 VUs concurrentes el stack default (Fitz `--features python` +
SQLAlchemy sync + psycopg2) muestra los límites del setup:

- **GIL** serializa el parsing de queries + construcción de ORM
  objects en la práctica (aunque el SQL execution salga del lock).
- **SQLAlchemy 2.x sync** + psycopg2 hace round-trip Python → C →
  socket → C → Python por cada query. Con GIL bloqueante: la cola
  del pool crece.
- **Sin uvicorn/gunicorn + workers** (el bench mide el setup
  default `fitz run --features python`, que es single proceso).
  Multi-worker mitigaría parcialmente, no elimina, el efecto.

### Por qué Node queda en el medio

Express + Prisma es razonable en performance, pero:

- **Prisma genera SQL en runtime** (no en codegen-time como Fitz) —
  cada query parsea + serializa params + valida types.
- **V8 garbage collector** mete pausas de 1-5 ms bajo carga peak —
  visible en el p99.9 de 172 ms vs 45 ms de Fitz.
- **Memory footprint** (163 MB) es lo más visible — Node carga V8 +
  Prisma client + Express runtime + connection pool. Fitz mantiene
  14 MB con el mismo workload.

### Limitaciones del bench

- **Single-host**: cliente k6 + API + DB en la misma máquina.
  Para latencias de red real (cliente remoto via internet) hace
  falta hardware separado — fuera del scope del bench reproducible.
- **Sin connection pooling externo**: cada app usa el pool de su
  driver/ORM. No medimos pgbouncer/pgpool.
- **Workload mix fijo** 60/40 reads/writes. Apps read-heavy o
  write-heavy reales pueden tener perfiles distintos.
- **Sin queries pesadas**: no probamos full-text search,
  agregaciones GROUP BY masivas, window functions, etc.

---

## Histórico

Cuando aparezcan nuevas corridas publicables (por hardware nuevo,
versión nueva del lenguaje, o escenarios extendidos), las anotamos
en la sección "Última corrida publicable" del README del bench
correspondiente y refrescamos esta página:

- [`benchmarks/orm-vs-sqlalchemy/README.md`](https://github.com/Thegreekman76/fitz/blob/main/benchmarks/orm-vs-sqlalchemy/README.md)
- [`benchmarks/mixed-workload/README.md`](https://github.com/Thegreekman76/fitz/blob/main/benchmarks/mixed-workload/README.md)
