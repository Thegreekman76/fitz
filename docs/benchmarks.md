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

### Headline numbers (v0.37.8, 2026-08-10 — mediana de 3 corridas)

!!! success "Fitz ORM: 5.9x menos memoria, 7-8x más throughput y ~8x menor latencia en reads"
    Read workloads sustained (30s, c=10) — el caso típico de un
    servicio HTTP que sirve API REST. Fitz ORM sostiene ~8x el
    throughput de SQLAlchemy con ~8x menor latencia y usa **5.9x menos
    memoria** (9.0 MB vs 53.4 MB). Empate técnico en write (POST es
    bottleneck del bench mismo, no del server).

**Hardware del run**: Intel Core Ultra 7 155H (Meteor Lake, 16 cores),
64 GB RAM, Windows 11 Pro, Docker 29.2.1 (Desktop con WSL2 backend).
**Versión**: `ghcr.io/thegreekman76/fitz:v0.37.8` — mediana de 3
corridas (±10% de variabilidad por corrida).

#### Cold start, image, memory

| Métrica | Fitz ORM | Python+SQLAlchemy | Ratio |
|---|---:|---:|---:|
| Cold start (s) | 0.29 | **0.24** | 0.83x (~empate) |
| Image size | **134 MB** | 272 MB | **2.0x más liviano** |
| Memory peak (MB) | **9.0** | 53.4 | **5.9x más eficiente** |

!!! note "El cold start ya no es ventaja de Fitz vs Python"
    En v0.10.13 Fitz arrancaba en 0.14s (vs 0.22s de Python). Desde
    v0.12.3 el binario HTTP linkea las deps de observability
    (OpenTelemetry + tracing + metrics), lo que subió el cold start de
    Fitz a ~0.29-0.33s — ahora arranca a la par de Python. Sigue siendo
    **8x más rápido que Node** (2.59s, ver bench de abajo) y el costo
    es opt-out con `@server(observability=false)`.

#### `GET /users` — lista de 50 rows, sustained 30s c=10

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | **4.94** | 39.89 | **8.07x** |
| p95 latency (ms) | **12.19** | 74.30 | **6.10x** |
| p99 latency (ms) | **19.75** | 93.03 | **4.71x** |
| Throughput (RPS) | **1693** | 232 | **7.30x** |
| Total requests | 50,793 | 6,972 | — |
| Success rate | 100% | 100% | — |

#### `GET /users/{id}` — single read por PK, sustained 30s c=10 ⭐

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | **3.76** | 32.02 | **8.52x** |
| p95 latency (ms) | **8.27** | 63.37 | **7.66x** |
| p99 latency (ms) | **15.26** | 84.70 | **5.55x** |
| Throughput (RPS) | **2244** | 282 | **7.96x** |
| Total requests | 67,340 | 8,451 | — |
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

    Resultado: GET /users/{id} pasó de **43.70ms → ~3.7ms p50**
    (~12x más rápido), de "Fitz pierde" a "Fitz gana ~8x" — se
    mantiene estable en v0.37.8 (mediana 3.76ms p50).

#### `POST /users` — 500 sequential con email único por request

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| p50 latency (ms) | 173.84 | 175.56 | ~empate |
| p95 latency (ms) | 293.64 | 265.06 | 0.90x |
| p99 latency (ms) | 439.93 | 301.52 | 0.68x (Python wins) |
| Throughput (RPS) | 3.08 | 3.31 | 0.95x |

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

**Las diferencias que vemos (7-8x en reads)** se explican por:

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

### Headline numbers (v0.37.8, 2026-08-10 — mediana de 3 corridas)

!!! success "Fitz: p95 de 16 ms bajo carga peak, 4-11x menos memoria y menos CPU que los otros dos"
    Bajo mixed workload sostenido 3 min con peak 100 VUs concurrentes,
    Fitz mantiene latencia bajo 90 ms hasta el p99.9 con **14.6 MB de
    memoria peak** (4.2x menos que Python, **11.3x menos que Node**) y
    **menos CPU** que ambos. Python+SQLAlchemy satura: p95 de 629 ms
    (cola de más de medio segundo por request CRUD trivial). Node+Prisma
    lidera en throughput entre los competidores pero paga 11x la memoria
    de Fitz y una latencia tail (p99) 4x peor.

**Hardware del run**: Intel Core Ultra 7 155H, 64 GB RAM, Windows 11
Pro, Docker 29.2.1 (Desktop con WSL2 backend). **Versión**:
`ghcr.io/thegreekman76/fitz:v0.37.8` — mediana de 3 corridas.

#### Cold start, image, memory, CPU

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Cold start (s) | 0.33 | **0.30** | 2.59 | 0.91x | **7.85x** |
| Image size | **134 MB** | 272 MB | 437 MB | **2.0x** | **3.3x** |
| Memory peak (MB) | **14.6** | 60.6 | 165.0 | **4.2x** | **11.3x** |
| CPU peak (%) | **108.5** | 178.9 | 215.3 | **1.6x** | **2.0x** |

!!! note "Dónde Fitz sigue aplastando: memoria, CPU y latencia tail"
    El throughput de Fitz lidera vs Python (3x) y va cabeza a cabeza
    con Node, pero la ventaja estructural está en la **eficiencia de
    recursos**: 4-11x menos memoria y ~2x menos CPU peak sostenido — un
    binario nativo con un driver Postgres en codegen-time no paga el
    runtime de V8/Prisma ni el del intérprete Python/SQLAlchemy. Y en
    **latencia tail** (p95/p99/p99.9), que es lo que el user siente
    bajo carga, el gap sigue enorme: p95 de 16 ms vs 629 ms (Python) y
    108 ms (Node).

#### Mixed workload (3 min, ramp 10→50→100→50, 60/40 reads/writes)

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Total reqs | **95,450** | 30,032 | 73,847 | 3.18x | 1.29x |
| Throughput (RPS) | **454.3** | 142.8 | 351.5 | **3.18x** | 1.29x |
| p50 latency (ms) | **5.61** | 200.69 | 23.57 | **35.8x** | **4.20x** |
| p95 latency (ms) | **16.36** | 629.36 | 107.74 | **38.5x** | **6.59x** |
| p99 latency (ms) | **37.12** | 807.77 | 149.67 | **21.8x** | **4.03x** |
| p99.9 latency (ms) | **87.41** | 1010.67 | 299.24 | **11.6x** | **3.42x** |
| Error rate (%) | 0.00 | 0.00 | 0.00 | empate | empate |

!!! note "Python cruzó dos thresholds bajo el peak"
    Los scenarios k6 declaran `p(50)<100ms` y `p(95)<500ms` como
    thresholds. Python+SQLAlchemy las violó (201ms p50, 629ms p95) —
    esto **no es bug del bench**, es exactamente la métrica que
    valida la dirección: el stack saturó bajo el peak. El error
    rate sigue en 0% (sin timeouts) pero la cola crece.

#### Reads-only (1 min, 50 VUs sostenidos)

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Throughput (RPS) | **861.1** | 203.9 | 574.3 | **4.22x** | 1.50x |
| p50 (ms) | **5.56** | 187.27 | 34.43 | **33.7x** | **6.19x** |
| p95 (ms) | **15.40** | 322.79 | 68.66 | **21.0x** | **4.46x** |
| p99 (ms) | **26.77** | 398.63 | 93.01 | **14.9x** | **3.47x** |
| Error rate (%) | 0.00 | 0.00 | 0.00 | empate | empate |

#### Writes-only (1 min, 50 VUs sostenidos) ⭐

!!! tip "Este scenario llena el gap del bench anterior"
    `orm-vs-sqlalchemy` reportaba "POST mide el cliente, no el
    server" — el test era curl-loop secuencial. Acá vemos write
    concurrency real con saturación del pool de cada ORM: **Fitz
    mantiene 6x mayor RPS y 25x mejor p95** que Python+SQLAlchemy.

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Throughput (RPS) | **811.1** | 136.1 | 541.6 | **5.96x** | 1.50x |
| p50 (ms) | **8.88** | 310.62 | 38.66 | **35.0x** | **4.35x** |
| p95 (ms) | **19.59** | 491.59 | 78.87 | **25.1x** | **4.03x** |
| p99 (ms) | **40.09** | 573.75 | 107.49 | **14.3x** | **2.68x** |
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
- **V8 garbage collector** mete pausas bajo carga peak — visible en
  el p99.9 de 299 ms vs 87 ms de Fitz.
- **Memory footprint** (165 MB) es lo más visible — Node carga V8 +
  Prisma client + Express runtime + connection pool. Fitz mantiene
  14.6 MB con el mismo workload (11.3x menos).

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

- **v0.37.8 (2026-08-10)** — refresh de ambos benchmarks (mediana de
  3 corridas). Fitz mantiene su liderazgo casi idéntico a v0.10.13:
  reads 7-8x más rápido que SQLAlchemy, memoria 5.9x (orm) / 4-11x
  (mixed), CPU y latencia tail dominantes. Único cambio real: el cold
  start subió (0.15→0.33s) por las deps de observability que el binario
  HTTP linkea desde v0.12.3 — ahora arranca a la par de Python, sigue
  8x mejor que Node.
- **v0.10.13 / 2026-06-17** — primeras corridas publicables (orm +
  mixed).

Cuando aparezcan nuevas corridas publicables (por hardware nuevo,
versión nueva del lenguaje, o escenarios extendidos), las anotamos
en la sección "Última corrida publicable" del README del bench
correspondiente y refrescamos esta página:

- [`benchmarks/orm-vs-sqlalchemy/README.md`](https://github.com/Thegreekman76/fitz/blob/main/benchmarks/orm-vs-sqlalchemy/README.md)
- [`benchmarks/mixed-workload/README.md`](https://github.com/Thegreekman76/fitz/blob/main/benchmarks/mixed-workload/README.md)
