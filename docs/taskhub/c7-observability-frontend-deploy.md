# C7 — Observability completa + frontend + deploy production

**Pre-requisitos**: [C6 — Interop Python con LLM](c6-interop-python-llm.md)
cerrado. Tenés el stack único de Fitz completo funcionando:
HTTP + auth + RBAC + ORM + WS + cron + background + interop
Python. Solo falta hacerlo **production-ready** con frontend
visible + observability live + deploy bundleado.

**Objetivo**: cerrar el proyecto TaskHub con **5 piezas finales**:

1. **`@server(prometheus=true)`** para exponer `/metrics`
   scrapeable, con dashboard en Prometheus UI.
2. **`@healthz`** + **`@readyz`** con check real contra la DB +
   SIGTERM drain automático para K8s zero-downtime.
3. **Frontend vanilla JS funcional** (sin frameworks): login →
   lista de projects → board kanban con tasks + WebSocket live
   updates. Reemplaza el placeholder del C1.
4. **`fitz build --bundle-python --bundle-pip openai`** para
   image final ~50 MB con CPython + openai embebidos en el
   binario, volviendo a base distroless. Path B fallback
   documentado si el bundling no anda.
5. **Publicación como boilerplate descargable**
   `boilerplates/taskhub/` al lado de los 9 boilerplates
   existentes — para que cualquiera clone + `docker compose up -d --build` y tenga TaskHub corriendo en 30s.

**Por qué importa**: este es **el cierre del proyecto**. C1-C6
construyeron la app pieza a pieza; C7 la deja **deployable a
producción** con monitoring real + frontend visible + image
optimizada. Y la **publica como producto independiente** para
que alguien que no quiera hacer los 7 caps simplemente clone el
boilerplate.

**Cross-link**: [Cap 33 de la guía — Observability](../guide.md#33-observability)

- [Cap 35 de la guía — Deployment ciudadano primera clase](../guide.md#35-deployment-ciudadano-primera-clase)
- [Curso M8.C2-C5](../curso/m8-produccion-deploy/c1-distribucion-binarios.md).

---

## Mapa del cap

```mermaid
flowchart LR
    A[Activar prometheus=true] --> B[/metrics scrape]
    C[@healthz custom + @readyz] --> D[SIGTERM drain auto]
    E[Frontend vanilla JS] --> F[login → board → WS]
    G[Dockerfile final con bundling] --> H[image 50 MB distroless]
    I[Boilerplate taskhub/] --> J[clonar + docker compose up]
    F --> K[stack completo visible end-to-end]
    H --> K
    B --> L[Prometheus UI :9090]
    D --> M[K8s rolling deploy zero-downtime]
```

---

## Por qué Fitz es distinto en deployment

| Feature                     | Python+FastAPI+uvicorn                         | Node+Express+pm2                       | Spring Boot                        | **Fitz TaskHub (C7)**                                                             |
| --------------------------- | ---------------------------------------------- | -------------------------------------- | ---------------------------------- | --------------------------------------------------------------------------------- |
| Image Docker final          | ~200-400 MB                                    | ~150-300 MB                            | ~300-500 MB                        | **~50 MB** (con bundling) o ~250 MB (sin)                                         |
| Cold start                  | 2-5s                                           | 1-2s                                   | 10-30s                             | **50-100ms**                                                                      |
| Memory idle                 | 120-200 MB                                     | 80-120 MB                              | 250-400 MB                         | **20-40 MB**                                                                      |
| Prometheus `/metrics`       | extras:`prometheus-fastapi-instrumentator`     | extras:`prom-client`                   | extras:`micrometer`                | **`@server(prometheus=true)` built-in**                                           |
| OTel tracing                | extras:`opentelemetry-instrumentation-fastapi` | extras:`@opentelemetry/sdk-trace-node` | extras:`opentelemetry-spring-boot` | **env var `OTEL_EXPORTER_OTLP_ENDPOINT` y listo**                                 |
| `/healthz` + `/readyz` auto | manual con `@app.get("/healthz")`              | manual                                 | extras:`spring-boot-actuator`      | **`@healthz fn` + `@readyz fn` decorators**                                       |
| SIGTERM drain auto          | manual `signal.signal(SIGTERM, ...)`           | pm2 graceful reload                    | Spring's GracefulShutdown bean     | **automático**: SIGTERM → readyz devuelve 503 → drain                             |
| Frontend bundling           | webpack / vite separado                        | webpack / vite separado                | webpack o thymeleaf                | **vanilla JS servido por nginx, sin build** (mismo binario)                       |
| Single-binary deploy        | ❌ (Python interp + libs)                      | ❌ (node + node_modules)               | ✅ jar (50-200 MB)                 | ✅**~50 MB con bundling**                                                         |
| 12-factor compliance        | manual cada uno                                | manual cada uno                        | parcial                            | **built-in todo** ([cap 35.6](../guide.md#35-deployment-ciudadano-primera-clase)) |

**Diferencial estructural**: el binario de TaskHub al cierre del
C7 es **portable, autónomo, observable, drainable**. **Sin SDK
externo para auth, sin Celery, sin Redis, sin ORM externo, sin
WS lib, sin Pydantic equivalente** — todo en el lenguaje. El
image Docker final pesa menos que **una sola dependencia**
típica de los stacks vecinos.

---

## Paso 1 — Activar `@server(prometheus=true)`

Editás `src/main.fitz` y cambiás:

```fitz
@server(port=8080, host="0.0.0.0", ws_heartbeat_secs=30, prometheus=true)
fn main() => 0
```

(`host="0.0.0.0"` viene desde C1 — necesario para Docker. La
sintaxis kwarg `port=`/`host=` está disponible desde v0.15.13.)

**El kwarg `prometheus=true` activa**:

- Endpoint `/metrics` automático en el binario (formato Prometheus
  exposition text).
- Counter `http_requests_total{method, path, status}` poblado
  automáticamente por cada request HTTP.
- Histogram `http_request_duration_seconds{method, path, status}`
  con buckets default para latency.
- Sin librerías extras — todo built-in.

Rebuild + verificación:

```bash
docker compose up -d --build app

curl http://localhost:8000/metrics | head -20
# → # HELP http_requests_total Total number of HTTP requests
#   # TYPE http_requests_total counter
#   http_requests_total{method="GET",path="/healthz",status="200"} 12
#   ...
```

**Prometheus UI** (que ya configuramos en C1) ahora debe ver el
target `taskhub` como **UP** (era DOWN en C1-C6 esperado):

```bash
open http://localhost:9090
# → Status → Targets:
#   - prometheus: UP
#   - taskhub: UP    ← ahora funciona
```

Probá una query en la UI:

```promql
rate(http_requests_total[1m])
# → muestra el throughput por endpoint en req/s
```

---

## Paso 2 — Validar OTel tracing en Jaeger

Las env vars ya están seteadas desde C1
(`OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317`,
`OTEL_SERVICE_NAME=taskhub`). El binario emite **spans
automáticos por cada request HTTP** + **span por cada handler
`@cron`/`@background`** sin código extra.

Generá tráfico:

```bash
ADMIN_TOKEN=$(curl -sX POST http://localhost:8000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}' \
  | jq -r .token)

# 10 requests para popular Jaeger.
for i in {1..10}; do
  curl -s http://localhost:8000/api/projects \
    -H "Authorization: Bearer $ADMIN_TOKEN" > /dev/null
done
```

Abrí Jaeger UI:

```bash
open http://localhost:16686
# → Services: aparece `taskhub`
# → Operations: GET /healthz, GET /projects, POST /auth/login, ...
# → Find Traces: click → ves spans con duraciones, status codes, etc.
```

**Spans incluyen automáticamente**:

- `http.method`, `http.target` (template del route), `http.status_code`.
- `duration_ms`.
- `user.id` y `user.role` cuando el handler está autenticado
  (extraído del `@auth_provider`).
- `db.query` spans anidados cuando el handler hace queries con
  el ORM.

**Cero código extra** para esto. La env var `OTEL_EXPORTER_OTLP_ENDPOINT`
es todo lo que necesitás. Si la quitás, todo el OTel deja de
funcionar — **zero overhead cuando no hay endpoint OTel**.

---

## Paso 3 — `@healthz` + `@readyz` con check real

Sumás al `src/main.fitz`:

```fitz
// `c.is_closed()` retorna `Future<Bool>` (paridad intérprete + codegen),
// por eso las fns son `async` y hacen `.await` explícito antes del `not`.
@healthz
async fn check_db_alive() -> Bool {
    // Devuelve true si la DB está accesible.
    // Si db.connect() falló al boot, db_result es Err — el
    // health check refleja eso devolviendo false.
    let conn: DbConn = match db_result {
        Ok(c)  => c,
        Err(_) => return false,
    }
    return not conn.is_closed().await
}

@readyz
async fn check_ready_for_traffic() -> Bool {
    // Mismo check para el MVP. En producción podrías refinar
    // chequeando que el cache está warm, que las migraciones
    // corrieron, etc.
    let conn: DbConn = match db_result {
        Ok(c)  => c,
        Err(_) => return false,
    }
    return not conn.is_closed().await
}
```

**Detalles**:

- **`@healthz fn ... -> Bool`** — el decorator auto-mount-ea el
  endpoint `/healthz`. Si la fn devuelve `true`, response `200 OK`; si `false`, `503 Service Unavailable`.
- **Override del `@get("/healthz")` simple del C1-C6** — el
  decorator `@healthz` tiene prioridad sobre cualquier handler
  manual con el mismo path. Podés borrar el handler manual del
  C1.
- **SIGTERM drain automático**: cuando el container recibe
  SIGTERM (típico durante `docker compose down` o K8s rolling
  deploy), Fitz pone `/readyz` en `503` automáticamente. K8s ve
  el 503, deja de rutear tráfico a este pod, y el pod termina
  los requests existentes antes de morir. **Zero-downtime
  rolling deploy sin código manual**.

Verificación:

```bash
# Healthz cuando la app está OK.
curl -i http://localhost:8000/healthz
# → HTTP/1.1 200 OK

# Readyz mismo.
curl -i http://localhost:8000/readyz
# → HTTP/1.1 200 OK

# Si la DB se cae, ambos pasan a 503.
docker compose stop db
sleep 5
curl -i http://localhost:8000/healthz
# → HTTP/1.1 503 Service Unavailable

docker compose start db
# (esperar healthcheck)
```

---

## Paso 4 — Frontend vanilla JS funcional

Reemplazás el placeholder `frontend/index.html` del C1 con una
SPA real vanilla. Structure:

```text
frontend/
├── index.html          # login screen + ROUTER
├── assets/
│   ├── style.css       # estilos minimal
│   ├── api.js          # fetch helpers (Bearer auth)
│   ├── ws.js           # WebSocket client
│   ├── app.js          # orchestrator + rendering
│   └── logo.svg        # opcional
```

Por simplicidad pedagógica, todo en una sola `index.html` con
routing client-side por hash.

### `frontend/index.html`

```html
<!DOCTYPE html>
<html lang="es">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>TaskHub</title>
    <link rel="stylesheet" href="/assets/style.css" />
  </head>
  <body>
    <header>
      <h1>TaskHub</h1>
      <nav id="nav"></nav>
    </header>

    <main id="app"></main>

    <script src="/assets/api.js"></script>
    <script src="/assets/ws.js"></script>
    <script src="/assets/app.js"></script>
  </body>
</html>
```

### `frontend/assets/style.css`

```css
* {
  box-sizing: border-box;
  margin: 0;
  padding: 0;
}

body {
  font-family: -apple-system, system-ui, sans-serif;
  color: #2c3e50;
  background: #f4f6f8;
  line-height: 1.5;
}

header {
  background: #ce412b;
  color: white;
  padding: 1rem 2rem;
  display: flex;
  justify-content: space-between;
  align-items: center;
}

header h1 {
  font-size: 1.5rem;
}

nav a,
nav button {
  background: rgba(255, 255, 255, 0.2);
  color: white;
  padding: 0.5rem 1rem;
  border: none;
  border-radius: 4px;
  text-decoration: none;
  cursor: pointer;
  margin-left: 0.5rem;
}

main {
  max-width: 1200px;
  margin: 2rem auto;
  padding: 0 1rem;
}

form {
  background: white;
  padding: 2rem;
  border-radius: 8px;
  max-width: 400px;
  margin: 0 auto;
}

form h2 {
  margin-bottom: 1rem;
}

form label {
  display: block;
  margin-top: 1rem;
  font-weight: 600;
}

form input,
form textarea {
  width: 100%;
  padding: 0.5rem;
  border: 1px solid #ddd;
  border-radius: 4px;
  font-size: 1rem;
}

form button {
  margin-top: 1rem;
  padding: 0.75rem 2rem;
  background: #ce412b;
  color: white;
  border: none;
  border-radius: 4px;
  font-size: 1rem;
  cursor: pointer;
}

.board {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 1rem;
  margin-top: 2rem;
}

.column {
  background: white;
  padding: 1rem;
  border-radius: 8px;
}

.column h3 {
  text-transform: uppercase;
  font-size: 0.85rem;
  color: #888;
  margin-bottom: 1rem;
}

.task {
  background: #f4f6f8;
  padding: 0.75rem;
  border-radius: 4px;
  margin-bottom: 0.5rem;
  cursor: grab;
}

.task strong {
  display: block;
  margin-bottom: 0.25rem;
}

.task .priority {
  display: inline-block;
  padding: 2px 6px;
  background: #ce412b;
  color: white;
  border-radius: 3px;
  font-size: 0.75rem;
}

.projects-list a {
  display: block;
  background: white;
  padding: 1rem;
  margin-bottom: 0.5rem;
  border-radius: 8px;
  text-decoration: none;
  color: #2c3e50;
}

.error {
  color: #c0392b;
  margin-top: 1rem;
}
.success {
  color: #27ae60;
  margin-top: 1rem;
}
```

### `frontend/assets/api.js`

```javascript
// api.js — wrappers de fetch con Bearer auth.

const API = {
  token: localStorage.getItem("taskhub_token") || null,

  setToken(t) {
    this.token = t;
    if (t) localStorage.setItem("taskhub_token", t);
    else localStorage.removeItem("taskhub_token");
  },

  async req(method, path, body) {
    const opts = {
      method,
      headers: { "Content-Type": "application/json" },
    };
    if (this.token) opts.headers["Authorization"] = `Bearer ${this.token}`;
    if (body) opts.body = JSON.stringify(body);

    const resp = await fetch(`/api${path}`, opts);
    if (resp.status === 401) {
      this.setToken(null);
      location.hash = "#login";
      throw new Error("no auth");
    }
    const data = await resp.json();
    if (!resp.ok) throw new Error(data.error || "request fallido");
    return data;
  },

  get(path) {
    return this.req("GET", path);
  },
  post(path, body) {
    return this.req("POST", path, body);
  },
  put(path, body) {
    return this.req("PUT", path, body);
  },
  del(path) {
    return this.req("DELETE", path);
  },
};
```

### `frontend/assets/ws.js`

```javascript
// ws.js — cliente WebSocket simple.

const WS = {
  socket: null,
  listeners: [],

  connect() {
    if (this.socket) return;
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    this.socket = new WebSocket(
      `${proto}//${location.host}/ws/events`,
      `bearer.${API.token}`,
    );
    this.socket.onmessage = (ev) => {
      const msg = JSON.parse(ev.data);
      this.listeners.forEach((cb) => cb(msg));
    };
    this.socket.onclose = () => {
      this.socket = null;
      // Reconnect después de 2s si todavía estamos logueados.
      if (API.token) setTimeout(() => this.connect(), 2000);
    };
  },

  on(cb) {
    this.listeners.push(cb);
  },

  send(msg) {
    if (this.socket && this.socket.readyState === WebSocket.OPEN) {
      this.socket.send(JSON.stringify(msg));
    }
  },
};
```

### `frontend/assets/app.js`

```javascript
// app.js — router + rendering.

const APP = document.getElementById("app");
const NAV = document.getElementById("nav");

function route() {
  const hash = location.hash || "#login";
  if (hash === "#login") return renderLogin();
  if (hash === "#projects") return renderProjects();
  const m = hash.match(/^#projects\/(\d+)$/);
  if (m) return renderBoard(parseInt(m[1]));
  return renderLogin();
}

function renderNav() {
  NAV.innerHTML = "";
  if (API.token) {
    const a = document.createElement("a");
    a.href = "#projects";
    a.textContent = "Projects";
    NAV.appendChild(a);

    const b = document.createElement("button");
    b.textContent = "Logout";
    b.onclick = () => {
      API.setToken(null);
      location.hash = "#login";
    };
    NAV.appendChild(b);
  }
}

function renderLogin() {
  APP.innerHTML = `
        <form id="login-form">
            <h2>Login</h2>
            <label>Email</label>
            <input type="email" id="email" required>
            <label>Password</label>
            <input type="password" id="password" required>
            <button type="submit">Entrar</button>
            <p class="error" id="err"></p>
        </form>
    `;
  document.getElementById("login-form").onsubmit = async (e) => {
    e.preventDefault();
    const email = document.getElementById("email").value;
    const password = document.getElementById("password").value;
    try {
      const { token } = await API.post("/auth/login", { email, password });
      API.setToken(token);
      WS.connect();
      location.hash = "#projects";
    } catch (err) {
      document.getElementById("err").textContent = err.message;
    }
  };
  renderNav();
}

async function renderProjects() {
  if (!API.token) return (location.hash = "#login");
  try {
    const projects = await API.get("/projects");
    APP.innerHTML = `
            <h2>Mis projects</h2>
            <form id="new-project">
                <label>Nuevo project</label>
                <input id="proj-name" placeholder="Nombre" required>
                <button type="submit">Crear</button>
            </form>
            <div class="projects-list" id="list"></div>
        `;
    const list = document.getElementById("list");
    if (projects.length === 0) {
      list.innerHTML = "<p>Sin projects todavía.</p>";
    } else {
      projects.forEach((p) => {
        const a = document.createElement("a");
        a.href = `#projects/${p.id}`;
        a.innerHTML = `<strong>${p.name}</strong><br><small>${p.description || ""}</small>`;
        list.appendChild(a);
      });
    }
    document.getElementById("new-project").onsubmit = async (e) => {
      e.preventDefault();
      const name = document.getElementById("proj-name").value;
      await API.post("/projects", { name });
      renderProjects();
    };
  } catch (err) {
    APP.innerHTML = `<p class="error">${err.message}</p>`;
  }
  renderNav();
}

async function renderBoard(id) {
  if (!API.token) return (location.hash = "#login");
  try {
    const project = await API.get(`/projects/${id}`);
    APP.innerHTML = `
            <h2>${project.name}</h2>
            <form id="new-task">
                <label>Nueva task</label>
                <input id="task-title" placeholder="Título" required>
                <button type="submit">Agregar</button>
            </form>
            <div class="board">
                <div class="column" data-status="todo">
                    <h3>To do</h3>
                    <div id="col-todo"></div>
                </div>
                <div class="column" data-status="doing">
                    <h3>Doing</h3>
                    <div id="col-doing"></div>
                </div>
                <div class="column" data-status="done">
                    <h3>Done</h3>
                    <div id="col-done"></div>
                </div>
            </div>
        `;
    renderTasks(project.tasks);

    document.getElementById("new-task").onsubmit = async (e) => {
      e.preventDefault();
      const title = document.getElementById("task-title").value;
      await API.post(`/projects/${id}/tasks`, { title });
      renderBoard(id);
    };

    // Listener WS para updates en vivo.
    WS.on((msg) => {
      if (msg.project_id !== id) return;
      renderBoard(id);
    });
  } catch (err) {
    APP.innerHTML = `<p class="error">${err.message}</p>`;
  }
  renderNav();
}

function renderTasks(tasks) {
  document.getElementById("col-todo").innerHTML = "";
  document.getElementById("col-doing").innerHTML = "";
  document.getElementById("col-done").innerHTML = "";
  tasks.forEach((t) => {
    const div = document.createElement("div");
    div.className = "task";
    div.dataset.id = t.id;
    div.draggable = true;
    div.innerHTML = `
            <strong>${t.title}</strong>
            <span class="priority">P${t.priority}</span>
        `;
    div.ondragstart = (e) => e.dataTransfer.setData("task_id", t.id);
    document.getElementById(`col-${t.status}`).appendChild(div);
  });
  document.querySelectorAll(".column").forEach((col) => {
    col.ondragover = (e) => e.preventDefault();
    col.ondrop = async (e) => {
      e.preventDefault();
      const taskId = parseInt(e.dataTransfer.getData("task_id"));
      const status = col.dataset.status;
      const project_id = parseInt(location.hash.match(/(\d+)/)[1]);
      await API.put(`/tasks/${taskId}`, { status });
      // Broadcast WS para que otros clientes refresquen.
      WS.send({
        kind: "updated",
        task_id: taskId,
        project_id,
        status,
        user_email: "",
      });
      const project = await API.get(`/projects/${project_id}`);
      renderTasks(project.tasks);
    };
  });
}

window.addEventListener("hashchange", route);
window.addEventListener("load", () => {
  if (API.token) WS.connect();
  route();
});
```

**Features del frontend**:

- **Login** con JWT guardado en localStorage.
- **Lista de projects** del user (scope por rol auto-aplicado del
  backend).
- **Board kanban** con 3 columnas (todo/doing/done) y drag&drop
  HTML5 nativo.
- **WebSocket reconexión auto** cada 2s si se cae.
- **Live updates**: cuando otro user mueve una task, todos ven el
  cambio.
- **Patrón del C4**: cliente que hace HTTP PUT también emite frame
  WS para que el server lo broadcastee.

**~250 LoC JS total**. Sin frameworks, sin build step.

---

## Paso 5 — `nginx.conf` validación para SPA

El `nginx.conf` del C1 ya tiene `try_files $uri $uri/ /index.html`
para soportar SPA routing. **Sin cambios**. Verificá:

```nginx
location / {
    root /usr/share/nginx/html;
    index index.html;
    try_files $uri $uri/ /index.html;
}
```

El `try_files` hace que `/projects/5` (un hash route del cliente,
pero si el user lo carga directo, no existe el archivo
`/projects/5.html`) caiga al `index.html` que ejecuta el router
client-side.

---

## Paso 6 — `Dockerfile` final multi-stage (real, lo que el boilerplate publica)

El Dockerfile del boilerplate publicado usa Python 3.12 en el runtime
porque el interop con `priority.py` necesita libpython al runtime. La
versión "bundling" (Path A — distroless de ~50 MB con CPython + pip
empaquetados al binario) queda como aspirational hasta que el
toolchain Docker se publique con cargo + python-build-standalone
out of the box (deuda CI documentada — ver `docs/deudas-post-5b.md`).

```dockerfile
# Stage 1A — extraer binario fitz con feature python desde la imagen
# oficial. La imagen `:latest-python` viene con fitz `--features
# python` compilado y libpython3.12 dev headers.
FROM ghcr.io/thegreekman76/fitz:latest-python AS fitz_source

# Stage 1B — builder con Python 3.12 + Rust toolchain.
# `:latest-python` NO trae cargo (es runtime image), por eso necesitamos
# armar el builder a mano con python:3.12-trixie (GLIBC 2.41 — match
# del binario fitz que se linkea contra esa lib) + rustup.
FROM python:3.12-trixie AS builder

ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain 1.95.0 --profile minimal

RUN apt-get update && \
    apt-get install -y --no-install-recommends pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Binario fitz extraído del :latest-python (tiene feature python).
COPY --from=fitz_source /usr/local/bin/fitz /usr/local/bin/fitz

WORKDIR /build
COPY fitz.toml .
COPY src/ ./src/
COPY migrations/ ./migrations/
COPY python/ ./python/

# `fitz build` invoca cargo internamente para producir el binario nativo.
RUN fitz build

# Stage 2 — runtime con Python 3.12-slim-trixie (necesario por interop
# Python al runtime + GLIBC ≥ 2.39 que el binario producido requiere).
FROM python:3.12-slim-trixie AS runtime

WORKDIR /app

# postgresql-client: pg_isready en entrypoint.sh para esperar a db.
# curl: healthcheck HTTP contra /healthz (python:3.12-slim NO trae curl).
RUN apt-get update && \
    apt-get install -y --no-install-recommends postgresql-client curl && \
    rm -rf /var/lib/apt/lists/*

# Python deps del interop (priority.py usa openai opcional).
COPY python/requirements.txt /app/python/requirements.txt
RUN pip install --no-cache-dir -r /app/python/requirements.txt

# Binario fitz también en runtime para correr `fitz db migrate` al
# boot via entrypoint.sh.
COPY --from=fitz_source /usr/local/bin/fitz /usr/local/bin/fitz

# El binario taskhub producido + assets.
COPY --from=builder /build/target/release/taskhub /app/taskhub
COPY migrations/ /app/migrations/
COPY fitz.toml /app/fitz.toml
COPY python/priority.py /app/python/priority.py

# Entrypoint: corre migrations + arranca el binario.
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

ENV PYTHONPATH=/app/python
EXPOSE 8080

ENTRYPOINT ["/app/entrypoint.sh"]
```

**`entrypoint.sh`** (al lado del Dockerfile):

```bash
#!/bin/sh
# Espera a postgres + corre migrations + exec del binario.
set -eu

DB_HOST=${DB_HOST:-db}
DB_USER=${DB_USER:-taskhub}
DB_NAME=${DB_NAME:-taskhub}

echo "[entrypoint] esperando a postgres en ${DB_HOST}:5432..."
for i in $(seq 1 30); do
    if pg_isready -h "$DB_HOST" -p 5432 -U "$DB_USER" -d "$DB_NAME" -q; then
        echo "[entrypoint] postgres ready en intento $i"
        break
    fi
    [ "$i" -eq 30 ] && { echo "[entrypoint] postgres no respondió"; exit 1; }
    sleep 1
done

echo "[entrypoint] aplicando migrations con 'fitz db migrate'..."
cd /app
fitz db migrate || { echo "[entrypoint] migrate falló"; exit 2; }
echo "[entrypoint] migrations OK"

exec /app/taskhub
```

**Healthcheck del compose** (actualizar de C1):

```yaml
app:
  # ...
  healthcheck:
    test: ["CMD-SHELL", "curl -fsS http://localhost:8080/healthz || exit 1"]
    interval: 10s
    timeout: 5s
    retries: 3
    start_period: 30s  # acomoda el tiempo de pg_isready + fitz db migrate
```

**Image final**: ~280 MB.

- Python 3.12-slim-trixie base: ~150 MB
- pip install openai + deps: ~100 MB
- Binario taskhub: ~10 MB
- libs sistema (postgresql-client + curl): ~20 MB

**Compará con stacks típicos**:

- Python+FastAPI+SQLAlchemy+Celery+Redis: ~600 MB compose total.
- Node+Express+TypeORM+bull+Redis: ~500 MB compose total.
- Spring Boot + Hibernate + Quartz: ~400 MB compose total.

TaskHub al ~280 MB es ~2x más liviano que los stacks típicos a pesar
de incluir CPython 3.12 + openai al runtime.

### Path A aspirational — bundling completo (futuro)

`fitz build --bundle-python --bundle-pip openai` produce un binario
de ~28 MB con CPython 3.14 + openai pip empaquetados adentro. El
runtime entonces puede ser **distroless** (~22 MB base + 28 MB
binario = ~50 MB total).

**Por qué no lo usamos hoy en el boilerplate publicado**: el comando
`fitz build --bundle-python` requiere `python-build-standalone` (de
Astral) instalado + cargo en el builder. La imagen
`ghcr.io/thegreekman76/fitz:latest-python` actual NO trae ninguno
de los dos (es runtime, no builder). Cuando el workflow de release
publique una imagen `:latest-python-builder` con cargo + el toolchain
de bundling, el Dockerfile se simplifica al Path A de ~50 MB
distroless. Deuda CI documentada.

---

## Paso 7 — Tests visuales end-to-end

```bash
docker compose up -d --build

# Bootstrap admin si no lo hiciste antes.
curl -X POST http://localhost:8000/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@taskhub.local","password":"adminpass123"}'

psql "$DATABASE_URL" -c "UPDATE users SET role='admin' WHERE id=1;"

# Abrí el frontend.
open http://localhost:8000
```

**Flujo manual del cliente**:

1. **Login screen** carga.
2. Ingresás `admin@taskhub.local` + `adminpass123` → entrás.
3. **Lista de projects** (vacía la primera vez).
4. Creás "Sprint Q3" → aparece en la lista.
5. Clickeás → entrás al board.
6. Agregás 3 tasks: "Diseñar UI", "URGENT: server down", "Refactor models".
7. **Drag & drop** "Diseñar UI" de To do → Doing.
8. **(Opcional)** abrí otra ventana del browser con el mismo
   user → verás los cambios en vivo via WebSocket.

**Validá los endpoints técnicos** en paralelo:

```bash
# Prometheus muestra el target taskhub como UP.
open http://localhost:9090
# → Status → Targets

# Jaeger muestra spans de los requests recientes.
open http://localhost:16686
# → Service: taskhub → Find Traces

# Healthz y Readyz.
curl http://localhost:8000/healthz   # 200
curl http://localhost:8000/readyz    # 200

# /metrics expone counters + histograms.
curl http://localhost:8000/metrics | grep http_requests
```

---

## Paso 8 — Publicación como `boilerplates/taskhub/`

Cerrás el proyecto **publicándolo como boilerplate descargable**.
Esto cumple la decisión del autor (desde el index): que
cualquiera pueda probar TaskHub sin pasar por los 7 caps.

```bash
# Desde la raíz del repo de Fitz:
cp -r examples/taskhub/c7-observability-frontend-deploy \
      boilerplates/taskhub

# Editar boilerplates/taskhub/README.md con foco en
# "clonalo y arrancá" (no en el cap pedagógico).
```

El README del boilerplate explica:

- **Qué es TaskHub** (1 párrafo, no 7 caps).
- **Cómo levantarlo**: `docker compose up -d --build` + bootstrap
  del admin.
- **Cómo extenderlo**: links a los 7 caps de
  `Construyendo TaskHub` para entender cada pieza.
- **Stack único de Fitz** integrado: tabla de features +
  comparativa final con stacks vecinos.

**El boilerplate convive con los 9 boilerplates existentes**
(`api-simple`, `api-orm-full-fullstack`, etc.). Es el **10mo**
y **el más completo** — el showcase del stack entero.

---

## Validación del cap (y del proyecto entero)

- [ ] `@server(prometheus=true)` activo + Prometheus target UP.
- [ ] Jaeger UI muestra spans con `user.id`/`user.role` cuando
      hay auth.
- [ ] `@healthz` + `@readyz` con check real de DB.
- [ ] SIGTERM en el container → `/readyz` pasa a 503.
- [ ] Frontend vanilla JS funciona end-to-end:
      login → projects list → board con drag&drop → live updates
      WS.
- [ ] `docker compose up -d --build` arranca todo en <60s.
- [ ] Path A (bundling): image final ~50 MB. **O** Path B
      (fallback): ~250 MB documentado.
- [ ] `boilerplates/taskhub/` publicado con README dedicado.

---

## Troubleshooting

### `@server(prometheus=true)` no activa `/metrics`

Verificá que el binario fue rebuildeado (`docker compose up -d --build app`). El kwarg `prometheus=true` es **compile-time** —
cambios en `@server(...)` requieren rebuild.

### Prometheus muestra `taskhub` target DOWN

- ¿La env var `prometheus.yml` apunta a `app:8080` (DNS interno
  del compose)? Verificá `prometheus/prometheus.yml`.
- ¿El binario está escuchando? `docker compose logs app`.

### Jaeger vacío sin spans

- ¿La env var `OTEL_EXPORTER_OTLP_ENDPOINT` está seteada en el
  container `app`? `docker compose exec app env | grep OTEL`.
- ¿Generaste tráfico? Sin requests, no hay spans.

### Frontend muestra 401 al login

- ¿`/api/auth/login` resuelve via nginx? Verificá
  `nginx.conf` con `location /api/`.
- ¿El cliente está mandando `Authorization: Bearer <token>` en
  los requests post-login? Inspect en DevTools → Network.

### WebSocket no conecta — `1006 Abnormal closure`

- Verificá el subprotocol: `new WebSocket(url, "bearer." + token)`
  (con el prefijo `bearer.` literal).
- ¿nginx tiene el upgrade headers? Verificá `nginx.conf` con
  `proxy_http_version 1.1`, `Upgrade $http_upgrade`,
  `Connection $connection_upgrade`.

### `fitz build --bundle-python` falla con `image not found`

La variante `ghcr.io/thegreekman76/fitz:latest-python` con feature
python puede no estar pre-built. Fallback: **Path B** del
Dockerfile del C6 (`python:3.12-slim-bookworm` runtime).

### `docker compose up` cuelga en `Building`

`fitz build` con bundling tarda 3-10 min la primera vez (compila
CPython + pip install openai). Builds posteriores son cache-friendly.
Si querés feedback de progreso: `docker compose up --build` sin
`-d`.

### Drag & drop no actualiza la task

DevTools → Network: verificá que `PUT /api/tasks/<id>` se envía
con `{"status":"doing"}`. Si responde 500, mirá `docker compose logs app`.

---

## Lo que cubriste (y cierre del proyecto)

**Felicidades** — terminaste **el cap final de Construyendo
TaskHub** y por tanto el proyecto entero.

### Stack único de Fitz integrado en TaskHub (C1 → C7)

| Pieza                               | Cap | Diferenciador                                                          |
| ----------------------------------- | --- | ---------------------------------------------------------------------- |
| Docker-first 5 services             | C1  | Compose con app + db + prometheus + jaeger + nginx desde día 1         |
| `fitz db diff/migrate` versionado   | C2  | Workflow real de migrations con `_fitz_migrations` audit               |
| Auth + RBAC 3 roles apilables       | C3  | `@auth_provider` + `@requires` apilable validado por checker           |
| ORM + relations + WS                | C4  | `@has_many` + `.preload(...)` + `WsConn<T>` tipado                     |
| Cron + background sin Celery/Redis  | C5  | `@cron(store=db_result)` + `@background` + `spawn` + persistencia auto |
| Interop Python con LLM              | C6  | `from python import` + `match Result<Int>` con fallback                |
| Observability + frontend + bundling | C7  | `@server(prometheus=true)` + `@healthz` + frontend vanilla + ~50 MB    |

### Comparativa final vs stack típico

| Métrica                | Python+FastAPI+Celery+Redis                                 | Node+Express+bull+Redis               | Spring Boot          | **Fitz TaskHub final**                         |
| ---------------------- | ----------------------------------------------------------- | ------------------------------------- | -------------------- | ---------------------------------------------- |
| Services en compose    | 6-7 (app + worker + beat + redis + db + nginx + flower opt) | 5 (app + worker + redis + db + nginx) | 3 (app + db + nginx) | **5 (app + db + prometheus + jaeger + nginx)** |
| Image total compose    | ~800 MB                                                     | ~700 MB                               | ~600 MB              | **~330 MB** (con bundling)                     |
| Boot full stack        | 60-90s                                                      | 30-60s                                | 60-120s              | **20-30s**                                     |
| Memory idle full       | 600-900 MB                                                  | 400-600 MB                            | 800-1200 MB          | **80-150 MB**                                  |
| Deps en el binario app | 20-40 pip packages                                          | 100+ npm packages                     | 30-80 jar deps       | **0** (todo built-in)                          |
| OpenAPI auto           | ✅ FastAPI                                                  | extras: swagger-ui                    | extras: springdoc    | ✅ built-in                                    |
| AsyncAPI auto WS       | ❌                                                          | ❌                                    | ❌                   | ✅ built-in                                    |
| Migrations             | Alembic                                                     | typeorm-cli                           | Flyway               | **`fitz db` built-in**                         |
| Auth + RBAC built-in   | extras: passlib + jose                                      | extras: jsonwebtoken + bcrypt         | spring-security      | **built-in**                                   |
| Cron sin broker        | ❌                                                          | ❌                                    | `@Scheduled`         | **`@cron` built-in**                           |
| Interop Python         | nativo                                                      | ❌                                    | ❌                   | **`from python import` built-in**              |

### Lo que ganaste

- **1 binario** que reemplaza el stack completo de SaaS típico.
- **Observability completa** sin SDKs extras.
- **Frontend visible** funcional end-to-end.
- **Image ~50 MB** vs ~600 MB del stack vecino.
- **Cero curva de aprendizaje** de N libs externas — todo es
  el lenguaje.

---

## Próximo paso: probá el boilerplate

**[`boilerplates/taskhub/`](https://github.com/Thegreekman76/fitz/tree/main/boilerplates/taskhub)**
ya está publicado. Cualquiera puede:

```bash
git clone https://github.com/Thegreekman76/fitz
cd fitz/boilerplates/taskhub
cp .env.example .env
# editá DB_PASSWORD + JWT_SECRET
docker compose up -d --build
# bootstrap admin
# → TaskHub corriendo en http://localhost:8000
```

Sin pasar por los 7 caps. **Para extender** — los caps están ahí
para entender qué hace cada pieza y cómo modificarla.

---

## Construir tu propia app sobre TaskHub

Ideas para fork del boilerplate:

- **Cambiar el dominio**: en lugar de tasks/projects → events,
  appointments, expenses, lo que sea.
- **Sumar más roles**: enterprise tier con scopes finos por
  resource.
- **Integrar más LLMs**: Anthropic Claude, local LLMs via
  Ollama.
- **Sumar email real**: en C5 mockeamos con `print`; integrá
  SendGrid / Postmark.
- **Sumar billing**: Stripe integration via interop Python.

**El stack está cerrado**. Las extensiones son tuyas.

---

## Gracias por hacer Construyendo TaskHub

Si llegaste hasta acá, **dominás el stack único de Fitz**. Tenés
una app real production-ready + un boilerplate descargable +
entendés cada pieza del puzzle.

**Compartí lo que construyas** — issues, PRs, feedback son
bienvenidos en
[github.com/Thegreekman76/fitz](https://github.com/Thegreekman76/fitz).

🏔️ Te debo una cerveza si nos cruzamos en El Chaltén.

— Martin TheGreekMan (autor de Fitz)
