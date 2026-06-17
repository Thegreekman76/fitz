# Benchmark — Mixed workload (Fitz vs Python+SQLAlchemy vs Node+Prisma)

**Comparación cabeza-a-cabeza** de un workload realista
(60% reads + 40% writes intercalados, VUs rampeando 10→50→100→50
durante 3 minutos) entre tres stacks equivalentes corriendo el
mismo dominio CRUD (`users` + `posts`) contra Postgres 16.

| Impl | App | Stack |
|---|---|---|
| **Fitz** | [`apps/fitz/`](apps/fitz/) | Driver Postgres v3.0 puro + ORM nativo (cap 31 de la guía) |
| **Python** | [`apps/python/`](apps/python/) | Fitz + `from python import` + SQLAlchemy 2.x + psycopg2 |
| **Node** | [`apps/node/`](apps/node/) | Node 20 + Express 5 + Prisma 5 |

Las tres apps exponen **los mismos 6 endpoints** con misma firma de
body y misma respuesta JSON:

| Método | Path | Qué hace |
|---|---|---|
| `GET`  | `/users?limit=N` | Lista paginada (N rows, ordenada por id) |
| `GET`  | `/users/{id}` | Single read por PK |
| `GET`  | `/users/{id}/posts` | Posts de un user (JOIN/preload) |
| `POST` | `/users` | Crear user |
| `POST` | `/users/{id}/posts` | Crear post asociado a user |
| `PUT`  | `/users/{id}` | Update name/email del user |

Misma DB Postgres 16-alpine, misma red Docker, mismo host. Solo
cambia el stack de la API.

> **⚠️ Atención — este bench usa `k6`, no `oha`.**
> El bench anterior (`benchmarks/orm-vs-sqlalchemy/`) usa
> [`oha`](https://github.com/hatoo/oha), perfecto para "bombardear
> un endpoint a velocidad fija c=10". Este bench necesita una
> herramienta distinta porque mide un workload **scripteado**
> (mix 60/40, ramping de VUs, percentiles p99.9) que `oha` no
> soporta. Ambos son single binary y se instalan global una sola
> vez — no es overhead por bench. Si ya tenés `oha` para el bench
> anterior, **igual necesitás instalar `k6`** para este. Ver
> [Prerequisitos](#prerequisitos) abajo.

## Por qué este benchmark (vs el bench actual `orm-vs-sqlalchemy`)

El benchmark anterior (`benchmarks/orm-vs-sqlalchemy/`) mide
**latencia/throughput aislado por endpoint** con `oha` corriendo
sobre un único endpoint a la vez con concurrencia fija (c=10). Eso
captura bien el _ceiling_ de cada operación, pero no representa el
patrón real de un servicio web en producción: **muchos usuarios
haciendo cosas distintas al mismo tiempo**.

Este bench llena esos gaps:

| Dimensión | `orm-vs-sqlalchemy` | `mixed-workload` (este) |
|---|---|---|
| **Workload shape** | Single-endpoint aislado | Mix realista 60/40 reads/writes |
| **Concurrencia** | Fija c=10 | VUs rampeando (10→50→100→50) |
| **Writes concurrentes** | No (curl loop secuencial) | Sí (cada VU su goroutine k6) |
| **JOINs** | No (`users` solo) | Sí (`/users/{id}/posts` con FK) |
| **Saturation behavior** | No mide | Sí (ramp-up detecta knee point) |
| **Error rate bajo carga** | No mide | Sí (k6 reporta % de 5xx + timeouts) |
| **p99.9** | No (oha no lo expone fácil) | Sí (k6 sí) |
| **Stacks** | 2 (Fitz, Python) | 3 (+ Node) |

Cubre la deuda explícita "POST throughput con concurrencia real
queda como extensión futura" del bench anterior.

## Métricas

Por cada stack + scenario:

- **Throughput agregado** (RPS total del mix)
- **Latencia p50/p95/p99/p99.9** por endpoint y agregada
- **Error rate** (% de respuestas no-2xx + timeouts)
- **Saturation point** — VU count cuando p95 cruza umbral
  (default: 200ms) o error rate >1%
- **Memory peak** (MB) del container del API via `docker stats`
- **CPU peak** (% del container API)
- **Cold start** (s) — desde `up -d` al primer 200 OK

## Prerequisitos

Cuatro herramientas, **todas single binary system-wide** (se instalan
una sola vez):

| Tool | Por qué | Bench anterior lo usa? |
|---|---|---|
| **k6** | Scripted scenarios — mix de endpoints, VU ramping, p99.9 | ❌ (usa `oha`) |
| **jq** | Parsear JSON de los outputs k6 + docker stats | ✅ |
| **docker** | Levanta DB + cada app del bench | ✅ |
| **curl** | Cold-start probe + seed de data | ✅ |

> Si venís de `benchmarks/orm-vs-sqlalchemy/` ya tenés todo menos
> **k6** — instalalo y listo.

### Instalar k6

`k6` es desarrollado por Grafana Labs. Single binary, ~5 MB.

**Windows**:

```powershell
# Opción 1 — winget (viene con Windows 11, recomendado):
winget install k6 --source winget

# Opción 2 — Chocolatey:
choco install k6

# Opción 3 — Scoop:
scoop install k6

# Opción 4 — manual: bajar k6.exe del release de GitHub
#   https://github.com/grafana/k6/releases/latest
#   Extraer en una carpeta y agregar al PATH del sistema.
```

**macOS**:

```bash
brew install k6
```

**Linux**:

```bash
# Debian/Ubuntu (repo oficial Grafana):
sudo gpg -k
sudo gpg --no-default-keyring --keyring /usr/share/keyrings/k6-archive-keyring.gpg \
         --keyserver hkp://keyserver.ubuntu.com:80 \
         --recv-keys C5AD17C747E3415A3642D57D77C6C491D6AC1D69
echo "deb [signed-by=/usr/share/keyrings/k6-archive-keyring.gpg] https://dl.k6.io/deb stable main" \
     | sudo tee /etc/apt/sources.list.d/k6.list
sudo apt update && sudo apt install k6

# Otras distros: bajar binario de https://github.com/grafana/k6/releases/latest
```

Documentación oficial completa con más opciones:
<https://k6.io/docs/get-started/installation/>.

### Instalar jq, docker, curl

```bash
# jq
#   Linux:   sudo apt install jq
#   macOS:   brew install jq
#   Windows: winget install jqlang.jq  (o scoop install jq / choco install jq)

# docker + docker compose
#   Estándar — https://docs.docker.com/get-docker/
#   (Docker Desktop trae compose embebido en Windows/macOS)

# curl
#   Linux/macOS: viene de fábrica
#   Windows 10+: viene de fábrica (curl.exe en %SystemRoot%/System32)
```

### Verificar instalación

```bash
k6 version          # k6 v0.50+ o superior
jq --version        # jq-1.6+ o superior
docker --version    # cualquier 20.10+
docker compose version
curl --version
```

Si alguno falla con "command not found" / "no se reconoce", revisá
que el binario esté en el PATH del shell donde corrés `run.sh`.

## Correr

```bash
cd benchmarks/mixed-workload
bash run.sh
```

El script:

1. Para cada stack (`fitz`, `python`, `node`):
   1. `docker compose up -d --build` desde `apps/<stack>/`.
   2. Espera primer 200 OK en `GET /users?limit=1` → mide cold start.
   3. Pre-seed: 200 users + 1000 posts (cada user con 0-15 posts random).
   4. Inicia sampler background de memory + CPU peak (cada 500ms).
   5. Corre `scenarios/mixed.js` (3 min, ramp 10→50→100→50).
   6. Corre `scenarios/reads-only.js` (1 min, 50 VUs sostenidos).
   7. Corre `scenarios/writes-only.js` (1 min, 50 VUs sostenidos).
   8. Detiene sampler, calcula peaks.
   9. `docker compose down -v` (clean state).
2. Genera `results/<timestamp>/summary.md` con tablas comparativas.

**Tiempo total estimado**: ~25-35 min con imágenes cacheadas
(3 stacks × ~8 min de bench cada uno + setup/teardown).

### Config via env vars

```bash
BENCH_DURATION_MIXED=180s   # mixed scenario
BENCH_DURATION_FOCUSED=60s  # reads-only y writes-only
BENCH_VUS_MAX=100           # peak VUs del ramp-up
SEED_USERS=200
SEED_POSTS_PER_USER=5       # promedio (0-15 random)
COLD_START_TIMEOUT=120
bash run.sh
```

## Output

```
results/
  20260617-HHMMSS/
    fitz/
      build.log
      cold_start.sec
      mem.log
      mem_peak.mb
      cpu.log
      cpu_peak.pct
      image_sizes.txt
      mixed.json         # k6 output JSON
      reads_only.json
      writes_only.json
    python/
      ...mismos archivos...
    node/
      ...mismos archivos...
    summary.md           # tabla comparativa Markdown
```

`summary.md` se imprime en stdout al final y queda commiteable.

## Interpretación

- **Speedup**: igual convención que el bench anterior — "Fitz es Nx
  más rápido / más eficiente":
  - Latencia / memory / cold start: `speedup = stack_X / fitz`
    (menor mejor → ratio >1 favorece Fitz).
  - RPS: `speedup = fitz / stack_X` (mayor mejor → ratio >1
    favorece Fitz).

- **Saturation point**: el VU count donde el sistema empieza a
  degradar. Útil para entender "cuánta carga aguanta esta app antes
  de que los users vean lag perceptible". Definido como el primer
  VU del ramp-up donde se cumple **al menos uno** de:
  - p95 latencia > 200ms
  - error rate > 1%

- **POST throughput sustained ≠ p50 individual**: el bench anterior
  reportaba ~110ms p50 en POST single. Acá medimos con 50 VUs
  concurrentes — saturación de connection pool, lock contention en
  Postgres, GIL vs work-stealing tokio — todo entra al ratio.

- **Variabilidad**: corridas locales tienen ±10-15% por CPU
  thermals, otros procesos, estado del cache PG. Para resultados
  publicables correr 3 veces y reportar mediana.

## Limitaciones explícitas

- **Single-host**: cliente k6 + API + DB en la misma máquina.
  Para resultados de red real (cliente remoto via internet) hace
  falta hardware separado — fuera del scope del bench reproducible.
- **Sin connection pooling externo**: cada app usa el pool de su
  driver/ORM. No medimos pgbouncer/pgpool.
- **Workload mix fijo**: 60/40 reads/writes. Apps read-heavy o
  write-heavy reales pueden tener perfiles distintos.
- **Sin queries pesadas**: no probamos full-text search, agregaciones
  GROUP BY masivas, window functions, etc. Eso requiere un dominio
  más rico — extensión futura.

## Resultados publicables

Cuando tengas un run que querés publicar, copiá el `summary.md` al
final de este README bajo `## Última corrida (YYYY-MM-DD)` con
hardware anotado:

```markdown
## Última corrida (2026-MM-DD)

- CPU: ...
- RAM: ...
- OS: ...
- Docker: ...

<paste summary.md aquí>
```

---

## Última corrida publicable (2026-06-17)

**Hardware** (auto-detectado por `summarize.sh`):

- CPU: Intel Core Ultra 7 155H (Meteor Lake, 16 cores)
- RAM: 64 GB
- OS: Windows 11 Pro
- Docker: 29.2.1 (Desktop con WSL2 backend)

**Versión Fitz**: binario actual del repo (post-v0.16.x) con
`ghcr.io/thegreekman76/fitz:latest{,-python}`.

### Headline

> **Fitz domina las 3 dimensiones del bench: throughput, latencia
> y eficiencia.** Bajo carga peak mixed workload (100 VUs ramping
> 10→100→50, 60/40 reads/writes sostenidos 3 min), Fitz **mantiene
> p95 de 11 ms** mientras Python+SQLAlchemy **satura a 503 ms p95**
> y Node+Prisma queda en el medio con **69 ms p95** + 11.7x más
> memoria que Fitz.

### Cold start, image, memory, CPU

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Cold start (s) | **0.15** | 0.81 | 2.22 | **5.4x** | **14.8x** |
| Image size | **131 MB** | 268 MB | 437 MB | **2.0x** | **3.3x** |
| Memory peak (MB) | **14.0** | 61.1 | 163.4 | **4.4x** | **11.7x** |
| CPU peak (%) | 131.0 | 171.1 | 215.3 | 1.3x | 1.6x |

### Mixed workload (3 min, ramp 10→50→100→50, 60/40 reads/writes)

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Total reqs | **97,303** | 34,466 | 82,486 | 2.82x | 1.18x |
| Throughput (RPS) | **463.1** | 164.0 | 392.6 | **2.82x** | 1.18x |
| p50 latency (ms) | **4.58** | 165.74 | 14.67 | **36.2x** | **3.20x** |
| p95 latency (ms) | **11.07** | 502.75 | 69.32 | **45.4x** | **6.26x** |
| p99 latency (ms) | **18.90** | 638.16 | 92.11 | **33.8x** | **4.87x** |
| p99.9 latency (ms) | **45.22** | 839.33 | 172.78 | **18.6x** | **3.82x** |
| Error rate (%) | 0.00 | 0.00 | 0.00 | empate | empate |

> Python+SQLAlchemy **cruzó dos thresholds** (`p(50)<100ms` y
> `p(95)<500ms`) — esto NO es bug, es la métrica del bench: el
> stack saturó bajo el peak. El error rate sigue en 0% (sin
> timeouts) pero la cola crece y los users esperan medio segundo
> por requests triviales.

### Reads-only (1 min, 50 VUs sostenidos)

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Throughput (RPS) | **900.4** | 261.9 | 628.9 | **3.44x** | 1.43x |
| p50 (ms) | **4.20** | 132.56 | 26.35 | **31.6x** | **6.27x** |
| p95 (ms) | **8.64** | 235.50 | 57.74 | **27.3x** | **6.68x** |
| p99 (ms) | **15.16** | 299.79 | 82.47 | **19.8x** | **5.44x** |
| Error rate (%) | 0.00 | 0.00 | 0.00 | empate | empate |

### Writes-only (1 min, 50 VUs sostenidos) ⭐

> Este scenario **llena el gap del bench anterior** — el
> `orm-vs-sqlalchemy` mide POST sequential con curl loop ("POST mide
> el cliente, no el server"). Acá Fitz mantiene **5x mayor RPS y
> 31x mejor p95** que Python+SQLAlchemy bajo escritura concurrente
> real.

| Métrica | Fitz | Python | Node | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Throughput (RPS) | **846.9** | 169.6 | 577.4 | **4.99x** | 1.47x |
| p50 (ms) | **7.86** | 234.80 | 33.14 | **29.9x** | **4.22x** |
| p95 (ms) | **12.54** | 392.73 | 69.19 | **31.3x** | **5.52x** |
| p99 (ms) | **20.89** | 480.05 | 94.43 | **23.0x** | **4.52x** |
| Error rate (%) | 0.00 | 0.00 | 0.00 | empate | empate |

### Cómo se reproduce

```bash
cd benchmarks/mixed-workload
bash run.sh
```

El script:

1. `docker compose up -d --build` de cada app (`apps/fitz/`,
   `apps/python/`, `apps/node/`).
2. Seed 200 users + ~5 posts/user (1000 posts promedio).
3. Sampler background de memoria + CPU (cada 500ms).
4. Corre `scenarios/mixed.js` (3 min, ramp 10→50→100→50).
5. Corre `scenarios/reads-only.js` (1 min, 50 VUs sostenidos).
6. Corre `scenarios/writes-only.js` (1 min, 50 VUs sostenidos).
7. `docker compose down -v` + siguiente stack.
8. Genera `results/<timestamp>/summary.md` con tablas + hardware
   auto-detectado.

**Tiempo total**: ~25-35 min con imágenes cacheadas. **Prerequisitos**:
`k6`, `jq`, `docker`, `curl` (ver sección
[Prerequisitos](#prerequisitos) arriba).

### Por qué Fitz tiende a ganar bajo carga mixed

Mismas razones que el bench anterior (driver Postgres puro Rust,
SQL constante codegen-time, Extended Query Protocol batched) **más**:

- **Async nativo + tokio multi-thread**: cada handler HTTP es una
  task tokio sobre work-stealing scheduler — el peak de 100 VUs
  concurrentes paraleliza sobre los 16 cores sin GIL ni event loop
  bloqueado.
- **Cero marshaling JSON intermedio**: `__ToFitzJson` impl emitido
  para cada `type` en codegen-time va directo a bytes — no hay
  Pydantic + dict + serialize round-trip.
- **Connection pool nativo** (`parking_lot::Mutex` + Arc) sin
  GIL serializando checkouts.

### Por qué Python satura tan fuerte

A 100 VUs concurrentes:

- **GIL** serializa el parsing de queries + construcción de ORM
  objects en la práctica (aunque el SQL execution salga del lock).
- **SQLAlchemy 2.x sync** + psycopg2 hace round-trip Python → C →
  socket → C → Python por cada query. Sumado al GIL: la cola del
  pool crece.
- **Stdlib HTTP de Python no es competitivo** para tráfico
  sostenido. uvicorn/gunicorn + workers ayudarían, pero el bench
  mide el setup default (single proceso Fitz `fitz run --features
  python` + interop).

### Por qué Node queda en el medio

Express + Prisma es razonable en performance, pero:

- **Prisma genera SQL en runtime** (no en codegen-time como Fitz)
  — cada query parsea + serializa params + valida types.
- **V8 garbage collector** mete pausas de 1-5ms bajo carga peak —
  visible en el p99.9 de 172ms vs 45ms de Fitz.
- **Memory footprint** (163 MB) es lo más visible — Node carga V8 +
  Prisma client + Express runtime + connection pool. Fitz mantiene
  14 MB con el mismo workload.

### Variabilidad esperada

±10-15% entre corridas locales por CPU thermals, otros procesos,
estado del cache de PG. Los **headline numbers** (Fitz >30x p95
sobre Python, >6x p95 sobre Node) son consistentes entre corridas;
solo cambian decimales.

Para resultados más estables: correr 3 veces y reportar mediana.
