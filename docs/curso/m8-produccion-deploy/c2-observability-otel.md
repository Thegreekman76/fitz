# M8.C2 — Observability en producción: logs, spans, métricas, OTel

**Pre-requisitos**: [M8.C1 — distribución avanzada](c1-distribucion-binarios.md).
Tu binario está listo. Ahora vas a saber qué hace en producción.

**Objetivo**: emitir logs estructurados que un parser entiende, abrir
spans HTTP automáticos con `trace_id` propagado a logs, exponer
métricas Prometheus/OTel, y conectar todo a un backend OTel real
(Jaeger/Tempo/Honeycomb/Datadog).

**Por qué importa**: en producción, **no podés debuggear con prints**.
Necesitás logs que un agregador (Loki, ELK, CloudWatch) parsee, spans
que muestran flujos cross-service, y métricas que un dashboard
(Grafana, Datadog) gradúa. En la mayoría de los lenguajes esto es
**6-12 paquetes externos** + setup manual. En Fitz es **built-in**.

**Cross-link**: [cap 33 de la guía](../../guide.md#33-observability-logs-spans-metricas-otel)
con detalle exhaustivo de cada feature.

---

## Por qué Fitz es distinto

Comparalo:

```python
# Python — setup OTel típico
pip install \
    opentelemetry-api \
    opentelemetry-sdk \
    opentelemetry-exporter-otlp \
    opentelemetry-instrumentation-fastapi \
    opentelemetry-instrumentation-logging \
    prometheus-client

from opentelemetry import trace, metrics
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.trace.export import BatchSpanProcessor
# ... 50 LoC de setup, instrumentación manual ...
```

```fitz
# Fitz — setup OTel built-in
log.info("hola, mundo")           // ya emite JSON estructurado
// los handlers HTTP automático tienen spans + métricas
// con `OTEL_EXPORTER_OTLP_ENDPOINT` env var, todo va al backend
```

Cero `pip install`, cero setup manual. **El logger, el tracer, las
métricas y el OTLP exporter son parte del binario `fitz`**.

---

## Paso 1 — Logs estructurados con `log.*`

Fitz tiene 4 niveles de log built-in: `debug`, `info`, `warn`, `error`.
Aceptan un mensaje + kwargs heterogéneos:

```fitz
log.info("user logged in", user_id: 42, role: "admin")
log.warn("DB query slow", duration_ms: 1500, query: "SELECT ...")
log.error("payment failed", order_id: "ABC123", reason: "card declined")
```

Output JSON estructurado a stderr (default):

```json
{"timestamp":"2026-06-03T18:42:15Z","level":"INFO","msg":"user logged in","user_id":42,"role":"admin"}
{"timestamp":"2026-06-03T18:42:16Z","level":"WARN","msg":"DB query slow","duration_ms":1500,"query":"SELECT ..."}
```

**Pretty mode** automático cuando stderr es TTY (dev local). Override
con `FITZ_LOG_FORMAT=pretty` o `=json`:

```bash
$ FITZ_LOG_FORMAT=pretty ./app
2026-06-03T18:42:15Z INFO  user logged in user_id=42 role=admin
                          (ANSI colors cuando TTY)
```

**Filtrado** con `RUST_LOG`:

```bash
$ RUST_LOG=warn ./app    # solo warn y error
$ RUST_LOG=debug ./app   # todo, incluyendo debug
$ RUST_LOG=app=info,fitz=warn ./app  # nivel por crate (avanzado)
```

**Redacción de secrets** automática:

```fitz
let pass = secret("DB_PASSWORD")
log.info("connecting", password: pass)
// → {"...","password":"***"}  ← redactado automático
```

Si un `Secret<T>` cae adentro de un kwarg (directo o anidado en
List/Map), se redacta a `"***"`. Esto evita que devs distraídos
filtren credenciales en logs.

---

## Paso 2 — Spans HTTP automáticos

Cada request HTTP abre un **span root** con `trace_id` (32 hex) y
`span_id` (16 hex), OTel-compatibles. Todos los `log.*` adentro del
handler heredan el contexto automático:

> **`fitz build` — opt-in vía `log.X` (v0.37.1+)**: en el binario
> nativo, el access-log/spans/métricas + las deps OpenTelemetry se
> activan cuando el programa usa **al menos un** `log.X(...)`. Como
> este servicio ya loguea (el ejemplo de abajo llama `log.info`),
> tenés todo el stack. Un HTTP que no llame `log.X` compila a un
> binario más liviano sin las crates OTel. `fitz run` (dev) muestra
> el access-log siempre. Detalle en el cap 33 de la guía.

```fitz
@get("/users/{id}")
async fn get_user(id: Int) -> User {
    log.info("fetching user", id: id)  // automático: trace_id + span_id
    let user = db.users.where(fn(u) => u.id == id).first(db).await?
    log.info("user found", name: user.name)  // mismo trace_id
    return user
}
```

Output:

```json
{"timestamp":"...","level":"INFO","msg":"fetching user","id":42,"trace_id":"a1b2c3...","span_id":"4d5e6f..."}
{"timestamp":"...","level":"INFO","msg":"user found","name":"Ada","trace_id":"a1b2c3...","span_id":"4d5e6f..."}
```

Los dos logs tienen el **mismo `trace_id`** porque corren adentro del
mismo request. En un agregador (Loki, ELK), buscás `trace_id=a1b2c3...`
y ves todo el flujo del request: cuál endpoint, qué pasó, qué falló.

**Atributos del span automáticos**:

- `http.method`: GET/POST/etc
- `http.target`: el path template (`"/users/{id}"`, NO el path
  expandido — preserva cardinality del backend)
- `http.status_code`: 200/404/500/etc
- `duration_ms`: tiempo del handler

---

## Paso 3 — Métricas built-in

Fitz emite dos métricas automático sobre cada request:

- **Counter `http_requests_total{method, path, status}`** — cuántos
  requests por endpoint y código.
- **Histogram `http_request_duration_seconds{method, path, status}`** —
  distribución de tiempos por endpoint.

Para exponerlas a Prometheus, activá el endpoint `/metrics`:

```fitz
@server(3000, prometheus=true)
fn main() {}
```

O via env var:

```bash
$ FITZ_PROMETHEUS=1 ./app
```

Ahora:

```bash
$ curl localhost:3000/metrics
# HELP http_requests_total counter
# TYPE http_requests_total counter
http_requests_total{method="GET",path="/users/{id}",status="200"} 142
http_requests_total{method="GET",path="/users/{id}",status="404"} 3
# HELP http_request_duration_seconds histogram
# TYPE http_request_duration_seconds histogram
http_request_duration_seconds_bucket{...,le="0.005"} 89
http_request_duration_seconds_bucket{...,le="0.01"} 134
...
```

Configurá Prometheus para que haga scrape:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'mi-app'
    static_configs:
      - targets: ['mi-app:3000']
    metrics_path: '/metrics'
```

Y en Grafana, query `rate(http_requests_total[1m])` para ver
requests-por-segundo, `histogram_quantile(0.99, http_request_duration_seconds_bucket)`
para p99 latency.

---

## Paso 4 — Bridge OpenTelemetry

Para mandar todo (spans + logs) a un backend OTel real (Jaeger,
Tempo, Honeycomb, Datadog), seteá estas env vars:

```bash
$ export OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4318
$ export OTEL_SERVICE_NAME=mi-app-prod
$ export OTEL_TRACES_SAMPLER_ARG=0.1   # 10% sampling
$ ./app
```

**Sin la env var**, el OTLP exporter NO se instala (zero overhead
default). **Con la env var**, el binario:

1. Abre conexión HTTP/proto a `<endpoint>/v1/traces` y `<endpoint>/v1/logs`.
2. Por cada span HTTP, lo manda al backend con todos los atributos.
3. Por cada `log.*`, lo manda al backend en paralelo a stderr (no
   reemplaza el stderr).
4. El `trace_id` que aparece en stderr matchea EXACTAMENTE el del
   backend OTel → en Jaeger podés buscar el trace y ver los logs
   correlacionados.

**Sampling**: `OTEL_TRACES_SAMPLER_ARG=0.1` significa "exportá el 10%
de los traces" (sampleo head-based). Default `1.0` = todo. Clamp
`[0.0, 1.0]`.

> **Recomendación de production**: 100% sampling es caro (más volumen
> de span exports, más bandwidth al backend). Empezá con 10-30% para
> servicios high-throughput. Por debajo del 5% perdés visibilidad
> de errores raros.

---

## Paso 5 — Setup local con Jaeger

Para experimentar local, lanzá Jaeger en docker:

```bash
$ docker run -d --name jaeger \
    -p 4318:4318 \
    -p 16686:16686 \
    jaegertracing/all-in-one:latest
```

Abrí tu app contra el collector:

```bash
$ export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
$ export OTEL_SERVICE_NAME=mi-app-dev
$ ./app

# desde otra terminal:
$ curl localhost:3000/users/42
```

Abrí http://localhost:16686 en el browser. Vas a ver:

- **Service**: `mi-app-dev`.
- **Operation**: `HTTP GET /users/{id}`.
- **Trace**: el span con atributos + duration.
- **Logs adentro del span**: los `log.info(...)` del handler con
  contenido exacto.

Ese trace_id coincide con el del stderr de tu app. Habilita el
"buscar el request específico en logs y en el tracer al mismo tiempo".

---

## Paso 6 — Patrones de producción

**Pattern 1 — Service name por entorno**:

```bash
# dev
OTEL_SERVICE_NAME=mi-app-dev

# staging
OTEL_SERVICE_NAME=mi-app-staging

# prod
OTEL_SERVICE_NAME=mi-app-prod
```

En el backend OTel, queries `service.name="mi-app-prod"` filtran solo
los traces relevantes.

**Pattern 2 — Sampling dinámico por endpoint**:

Hoy Fitz solo soporta sampling head-based global (`OTEL_TRACES_SAMPLER_ARG`).
Para tail-based o por endpoint, usá un OTel collector con
`tail_sampling_processor` que decide en base al span completo (ej:
samplear 100% si hay error, 10% si OK).

**Pattern 3 — Logs sin OTel**:

Si no querés bridge OTel pero sí logs estructurados a un agregador
(Loki, ELK), el JSON de stderr es directamente parseable. En K8s,
el log driver del nodo recoge stderr y lo manda al agregador
configurado:

```yaml
# k8s pod
containers:
  - name: app
    image: mi-app:v1
    env:
      - name: FITZ_LOG_FORMAT
        value: "json"
      - name: RUST_LOG
        value: "info"
```

**Pattern 4 — Debug en prod sin redeploy**:

Para subir el nivel de log temporalmente sin redeploy, usá un sidecar
que rota la env var via signal. O construí una API admin que cambia
el filter dinámicamente (deuda visible — `log.set_filter(...)` no
existe todavía).

---

## Validación del cap

Probá end-to-end con OTel local:

```bash
# 1. Lanzá Jaeger.
$ docker run -d -p 4318:4318 -p 16686:16686 jaegertracing/all-in-one

# 2. Activá las env vars + Prometheus.
$ export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
$ export OTEL_SERVICE_NAME=mi-app-dev
$ FITZ_PROMETHEUS=1 ./app

# 3. Generá tráfico.
$ for i in {1..100}; do curl localhost:3000/users/$i > /dev/null; done

# 4. Verificá:
$ curl localhost:3000/metrics | grep http_requests_total
$ open http://localhost:16686  # Jaeger UI

# 5. Buscá un request específico por trace_id en los logs:
$ ./app 2>&1 | grep "trace_id\":\"a1b2c3"
```

Si los 5 pasos funcionan, sabés operar Fitz en producción.

---

## Próximo: M8.C3 — Secrets management

Tu app ahora emite logs, spans y métricas. La pregunta siguiente es:
**¿cómo configurás credenciales sin filtrarlas?** El próximo cap
cubre `secret()` y `config()` built-ins, el tipo opaco `Secret<T>`,
y patterns canónicos (.env, K8s secrets, Vault).

→ [M8.C3 — Secrets management](c3-secrets-config.md)
