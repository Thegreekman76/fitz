# M7.C4 — Deploy avanzado: Docker, healthz/readyz, K8s, 12-factor

**Pre-requisitos**: [M7.C3 — secrets management](c3-secrets-config.md).
Tu app maneja credenciales sin filtrarlas. Ahora la llevamos a
producción real.

**Objetivo**: usar `fitz docker init` para generar Dockerfile +
compose smart por defecto, entender `/healthz` y `/readyz` auto-mount
para K8s, manejar SIGTERM drain para zero-downtime deploys, y aplicar
los principios 12-factor.

**Por qué importa**: el último kilómetro entre "tengo un binario" y
"está corriendo en producción con monitoring y rolling deploys" es
el más friccionado en la mayoría de los stacks. Fitz lo automatiza
con detección AST: el sub-comando `fitz docker init` lee el shape de
tu programa y emite Dockerfile + compose adaptados.

**Cross-link**: [cap 35 de la guía](../../guide.md#35-deployment-ciudadano-primera-clase)
para la vista integradora completa.

---

## Por qué Fitz es distinto

| Feature                       | Python    | TypeScript | Go    | Spring | **Fitz** |
| ----------------------------- | --------- | ---------- | ----- | ------ | -------- |
| Dockerfile autogenerado       | ❌ (cookiecutter) | ❌ (Nx generator) | ❌ | ❌ (jib opcional) | ✅ **`fitz docker init`** |
| Detección AST del shape       | ❌        | ❌         | ❌    | ❌     | ✅ **@server/db/python/cron** |
| `/healthz` + `/readyz` automático | ❌ (Flask actuator) | ❌ (terminus opcional) | ❌ | ✅ (actuator) | ✅ **auto-mount sin código** |
| SIGTERM drain                 | ⚠ manual  | ⚠ manual   | ⚠ manual | ⚠ manual | ✅ **30s grace + readyz→503** |
| `docker build` wrapper        | ❌        | ❌         | ❌    | ❌     | ✅ **`fitz docker build [--tag X]`** |
| 12-factor compliance built-in | ⚠ parcial | ⚠ parcial  | ⚠ parcial | ⚠ parcial | ✅ **default por design** |

El diferencial: **NO instalás nada extra** para Dockerfile, no copiás
templates de Stack Overflow, no decidís entre distroless vs slim a
mano. Fitz lee tu programa, decide el runtime apropiado, y emite el
Dockerfile correcto.

---

## Paso 1 — Generar Dockerfile + compose con `fitz docker init`

Desde adentro de tu proyecto (con `fitz.toml`):

```bash
$ fitz docker init
▶ fitz docker init — proyecto `mi-app` en `/path/al/proyecto`
   detectado: @server(port = 3000)
   detectado: uso de DB (db.X(...)) → compose suma postgres:16-alpine
   detectado: @cron → compose suma restart: unless-stopped
✓ escrito: Dockerfile
✓ escrito: .dockerignore
✓ escrito: docker-compose.yml
```

Tres archivos generados:

**`Dockerfile`** — multi-stage:

```dockerfile
ARG FITZ_TAG=latest

FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder
WORKDIR /app
COPY fitz.toml ./
COPY src/ ./src/
RUN fitz build

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/mi-app /usr/local/bin/app
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/app"]
```

**`.dockerignore`** — excluye `target/`, `.git/`, `.env*`,
`__pycache__/`, etc.

**`docker-compose.yml`** — service `app` + service `db` (porque
detectó `db.X(...)`) + `restart: unless-stopped` (porque detectó
`@cron`):

```yaml
services:
  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: ${POSTGRES_USER:-fitz}
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-fitz}
      POSTGRES_DB: ${POSTGRES_DB:-fitz}
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U $$POSTGRES_USER"]
      interval: 5s
      retries: 5

  app:
    build: .
    container_name: mi-app
    environment:
      DATABASE_URL: "postgres://${POSTGRES_USER:-fitz}:${POSTGRES_PASSWORD:-fitz}@db:5432/${POSTGRES_DB:-fitz}"
    ports:
      - "3000:3000"
    restart: unless-stopped
    depends_on:
      db:
        condition: service_healthy
```

**Smart runtime selection** — si tu programa usa `from python import X`,
el Dockerfile cae automático a `python:3.12-slim-bookworm` (con
libpython + wget). Si no, queda `distroless/cc-debian12` (~22 MB).

---

## Paso 2 — Buildear y correr

```bash
$ fitz docker build --tag mi-app:v1.0
▶ fitz docker build — tag `mi-app:v1.0` en `/path/al/proyecto`
[+] Building 38.4s ...
✓ build OK — `mi-app:v1.0`

$ docker compose up
[+] Running 2/2
 ✔ Container mi-app-db   Healthy  3.0s
 ✔ Container mi-app      Started  3.5s

# En otra terminal:
$ curl localhost:3000/healthz
{"status":"ok"}
```

El `fitz docker build` es un thin wrapper sobre `docker build -t
<tag> .`. Útil porque toma el tag desde `package.name` por default y
te ahorra escribir el path.

---

## Paso 3 — Healthz + readyz auto-mount

Tu binario Fitz expone `/healthz` y `/readyz` **automáticamente**
desde el momento que tiene `@server(...)`. No escribiste código para
eso:

```bash
$ curl localhost:3000/healthz
{"status":"ok"}        ← liveness probe K8s

$ curl localhost:3000/readyz
{"status":"ready"}     ← readiness probe K8s
```

**`/healthz`** (liveness) — responde 200 mientras el proceso esté
vivo. K8s lo usa para decidir "¿reiniciar este pod?". Si el binario
está deadlocked, axum no responde, K8s reinicia.

**`/readyz`** (readiness) — responde 200 cuando el proceso está
listo para recibir tráfico, 503 durante el "draining" (SIGTERM ya
disparó pero el server sigue terminando requests en vuelo).

**Custom logic** — para chequear la DB o un cache antes de declarar
"ready", usá los decoradores dedicados:

```fitz
@healthz
fn db_alive() -> Bool {
    return db.is_closed() == false
}

@readyz
async fn cache_warm() -> Bool {
    let count = my_cache.size()
    return count > 0
}
```

Las custom override el default. Si la fn retorna `false`, el endpoint
devuelve 503.

---

## Paso 4 — SIGTERM drain para zero-downtime deploys

Cuando K8s va a matar un pod (rolling deploy, autoscaling), manda
`SIGTERM`. Tu app Fitz responde así **automático**:

1. **Flippea `/readyz` a 503** — K8s deja de rutear tráfico al pod
   (load balancer remueve este pod del pool).
2. **Sigue procesando requests en vuelo** — los que ya estaban
   ejecutando terminan limpiamente.
3. **Tras 30s (grace period default) o cuando termina el último
   request**, el proceso exitea con código 0.

Para K8s en deployment.yaml:

```yaml
spec:
  template:
    spec:
      terminationGracePeriodSeconds: 35  # 30s drain + 5s margen
      containers:
        - name: app
          image: mi-app:v1.0
          readinessProbe:
            httpGet:
              path: /readyz
              port: 3000
            periodSeconds: 5
          livenessProbe:
            httpGet:
              path: /healthz
              port: 3000
            periodSeconds: 30
```

Con `terminationGracePeriodSeconds=35` y `readinessProbe`
chequeando cada 5s, durante un rolling deploy:

- T+0s: K8s manda SIGTERM al pod viejo.
- T+0s: pod viejo flippea `/readyz` a 503.
- T+5s: K8s detecta `readyz=503`, remueve del load balancer.
- T+5-35s: pod sigue terminando los requests que llegaron antes.
- T+35s: pod exitea limpio.

**Zero requests fallidos durante el deploy**.

---

## Paso 5 — Aplicar a Kubernetes

Una vez que tu Dockerfile y tu deployment.yaml están listos:

```bash
$ docker push mi-app:v1.0  # subir a registry
$ kubectl apply -f deployment.yaml
$ kubectl apply -f service.yaml
$ kubectl apply -f ingress.yaml
$ kubectl rollout status deployment/mi-app
deployment "mi-app" successfully rolled out
```

Para rolling deploy de v1.1:

```bash
$ docker build --tag mi-app:v1.1 . && docker push mi-app:v1.1
$ kubectl set image deployment/mi-app app=mi-app:v1.1
$ kubectl rollout status deployment/mi-app
```

K8s crea pods con v1.1, espera que su `/readyz` retorne 200, y
recién después remueve los pods v1.0. Si v1.1 nunca pasa healthcheck,
K8s detiene el rollout — tu producción sigue con v1.0.

---

## Paso 6 — 12-factor compliance

Los 12 factores de The Twelve-Factor App (https://12factor.net) son
el estándar de facto para apps cloud-native. Fitz cubre los 12 por
default:

| #   | Factor                  | Cómo Fitz lo cumple                                  |
| --- | ----------------------- | ---------------------------------------------------- |
| I   | Codebase                | `fitz.toml` + git.                                   |
| II  | Dependencies            | `fitz.lock` reproducible.                            |
| III | Config                  | `config(key, default)` + `secret(key)`.              |
| IV  | Backing services        | `db.connect(url_from_env)`.                          |
| V   | Build/release/run       | `fitz build` separa explícitamente.                  |
| VI  | Processes               | Stateless por default; jobs con persist DB opcional. |
| VII | Port binding            | `@server(port, "0.0.0.0")`.                          |
| VIII| Concurrency             | Tokio multi-thread, scale horizontal.                |
| IX  | Disposability           | SIGTERM drain automático (30s grace).                |
| X   | Dev/prod parity         | Mismo binario, mismas env vars.                      |
| XI  | Logs                    | JSON estructurado a stderr.                          |
| XII | Admin processes         | `@command(...)` + `fitz run` ad-hoc.                 |

**El único factor que requiere atención manual** es V (no podés
compartir el `target/` entre dev y prod — buildeás separado). Los
demás están cubiertos por defaults del lenguaje.

---

## Paso 7 — Patrones canónicos de producción

**Pattern 1 — Multi-stage compose para entornos**:

```yaml
# compose.yml (base)
services:
  app:
    image: mi-app:${VERSION:-latest}
    env_file:
      - .env.${ENV:-dev}
    ports:
      - "3000:3000"
```

```bash
$ ENV=staging VERSION=v1.0-rc1 docker compose up
$ ENV=prod    VERSION=v1.0     docker compose up
```

**Pattern 2 — Sidecar para log shipping**:

En K8s podés mountar un sidecar que tail-ee los logs y los mande a
Loki/Splunk:

```yaml
spec:
  containers:
    - name: app
      image: mi-app:v1.0
    - name: log-shipper
      image: grafana/promtail:latest
      volumeMounts:
        - name: logs
          mountPath: /var/log
```

Tu app emite JSON a stderr; el sidecar lo recoge.

**Pattern 3 — CDN + edge cache para static**:

Si tu app tiene un endpoint `@get("/static/...")` (assets), poné un
CDN (Cloudflare, Bunny, CloudFront) adelante. Tu binario Fitz solo
sirve dinámico; el CDN cachea estático.

**Pattern 4 — Read replicas para escalar reads**:

Usá `config("DATABASE_URL_READ", "postgres://...replica...")` y
`config("DATABASE_URL_WRITE", "postgres://...primary...")` para
dirigir reads a una replica y writes a la primary:

```fitz
let read_db = db.connect(config("DATABASE_URL_READ", default_url))?
let write_db = db.connect(config("DATABASE_URL_WRITE", default_url))?
// reads usan read_db, writes usan write_db
```

---

## Validación final del módulo M7

Probá end-to-end el stack completo:

```bash
# 1. Buildeá producción.
$ fitz docker init
$ fitz docker build --tag mi-app:v1.0

# 2. Lanzá local con compose.
$ docker compose up -d
$ curl localhost:3000/healthz   # 200
$ curl localhost:3000/readyz    # 200

# 3. Simulá rolling deploy (down/up).
$ docker compose stop app
# `/readyz` ya no responde
$ docker compose start app
# Tras 1-2s, `/readyz` vuelve a responder

# 4. Verificá logs.
$ docker compose logs app
{"timestamp":"...","msg":"server listo"}
{"timestamp":"...","msg":"request","method":"GET","path":"/healthz"}

# 5. Verificá métricas (si activaste Prometheus).
$ curl localhost:3000/metrics | head -10
```

Si los 5 pasos funcionan, **tenés una app production-ready**.

---

## Cierre del curso

Llegaste al final. Repasemos qué construiste:

- **M1-M3**: setup, tipos, módulos. La base.
- **M4**: HTTP first-class con OpenAPI auto.
- **M5**: async + auth JWT/Argon2 + WebSockets + cron — el stack
  web entero.
- **M6**: Postgres + ORM nativo, capstone CRUD completo.
- **M7**: distribución binarios + observability + secrets + deploy
  con Dockerfile autogenerado.

**Diferenciales de Fitz que ahora conocés a fondo**:

1. **Binarios standalone** sin runtime de Fitz/Rust/Python en destino.
2. **Cross-compile gratis** vía rustc targets.
3. **Driver Postgres puro** sin libpq, sin tokio-postgres externa.
4. **ORM declarativo** con paridad bit-a-bit `fitz run` ↔ `fitz build`.
5. **Auth nativa** con JWT + Argon2 + RBAC (`@requires`).
6. **WebSockets tipados** con AsyncAPI auto.
7. **Jobs sin Celery** — cron + spawn nativos, persistencia opcional.
8. **Observability OTel built-in** — logs + spans + métricas + OTLP.
9. **Secrets tipados** con `Secret<T>` opaco + redacción automática.
10. **Dockerfile autogenerado** con detección AST del shape.

**¿Qué sigue?**:

- **Construí algo**: tomá uno de los [boilerplates](../../boilerplates/)
  y modificalo para tu caso.
- **Contribuí**: el repo está abierto, hay [issues etiquetados](https://github.com/Thegreekman76/fitz/issues)
  buenos para arrancar.
- **Compartí**: si publicaste algo con Fitz, contanos en el repo. Es
  early-stage; tu feedback moldea el lenguaje.

Gracias por hacer el curso entero. Te debo un café si nos cruzamos
en El Chaltén.

— Martin (autor de Fitz)
