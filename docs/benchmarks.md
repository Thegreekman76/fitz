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

### Qué no testeamos en el MVP

Quedan como extensiones futuras (cuando aparezca demanda):

- Mixed workload realista (reads + writes intercalados).
- Bulk inserts (1k+ rows en una transaction).
- Queries con JOINs / preload eager loading (necesita
  `api-orm-full` como base).
- Escritura concurrente con saturación del pool.

---

## Histórico

Cuando aparezcan nuevas corridas publicables (por hardware nuevo,
versión nueva del lenguaje, o escenarios extendidos), las anotamos
en [`benchmarks/orm-vs-sqlalchemy/README.md`](https://github.com/Thegreekman76/fitz/blob/main/benchmarks/orm-vs-sqlalchemy/README.md)
sección "Última corrida publicable" y refrescamos esta página.
