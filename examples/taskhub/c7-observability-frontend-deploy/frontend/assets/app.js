// app.js — router + rendering.

const APP = document.getElementById("app");
const NAV = document.getElementById("nav");

function route() {
    const hash = location.hash || "#login";
    if (hash === "#login") return renderLogin();
    if (hash === "#projects") return renderProjects();
    const m = hash.match(/^#projects\/(\d+)$/);
    if (m) return renderBoard(parseInt(m[1], 10));
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
            WS.disconnect();
            location.hash = "#login";
        };
        NAV.appendChild(b);
    }
}

function renderLogin() {
    APP.innerHTML = `
        <form id="login-form" class="centered">
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
            list.innerHTML = '<p class="empty">Sin projects todavía. Creá el primero arriba.</p>';
        } else {
            projects.forEach((p) => {
                const a = document.createElement("a");
                a.href = `#projects/${p.id}`;
                a.innerHTML = `<strong>${escapeHTML(p.name)}</strong><small>${escapeHTML(p.description || "")}</small>`;
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

let currentBoardId = null;

async function renderBoard(id) {
    if (!API.token) return (location.hash = "#login");
    currentBoardId = id;
    try {
        const project = await API.get(`/projects/${id}`);
        APP.innerHTML = `
            <h2>${escapeHTML(project.name)}</h2>
            <form id="new-task">
                <label>Nueva task</label>
                <input id="task-title" placeholder="Título" required>
                <button type="submit">Agregar</button>
            </form>
            <div class="board">
                <div class="column" data-status="todo">
                    <h3>To do</h3>
                    <div class="tasks" data-col="todo"></div>
                </div>
                <div class="column" data-status="doing">
                    <h3>Doing</h3>
                    <div class="tasks" data-col="doing"></div>
                </div>
                <div class="column" data-status="done">
                    <h3>Done</h3>
                    <div class="tasks" data-col="done"></div>
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

        wireDragAndDrop(id);
    } catch (err) {
        APP.innerHTML = `<p class="error">${err.message}</p>`;
    }
    renderNav();
}

function renderTasks(tasks) {
    document.querySelectorAll(".tasks").forEach((c) => (c.innerHTML = ""));
    tasks.forEach((t) => {
        const div = document.createElement("div");
        div.className = "task";
        div.dataset.id = t.id;
        div.draggable = true;
        div.innerHTML = `
            <strong>${escapeHTML(t.title)}</strong>
            <span class="priority">P${t.priority}</span>
        `;
        div.ondragstart = (e) => {
            e.dataTransfer.setData("text/plain", t.id.toString());
        };
        const col = document.querySelector(`.tasks[data-col="${t.status}"]`);
        if (col) col.appendChild(div);
    });
}

function wireDragAndDrop(projectId) {
    document.querySelectorAll(".column").forEach((col) => {
        col.ondragover = (e) => {
            e.preventDefault();
            col.classList.add("drag-over");
        };
        col.ondragleave = () => col.classList.remove("drag-over");
        col.ondrop = async (e) => {
            e.preventDefault();
            col.classList.remove("drag-over");
            const taskId = parseInt(e.dataTransfer.getData("text/plain"), 10);
            const status = col.dataset.status;
            try {
                await API.put(`/tasks/${taskId}`, { status });
                // Broadcast WS para que otros clientes refresquen.
                WS.send({
                    kind: "updated",
                    task_id: taskId,
                    project_id: projectId,
                    status: status,
                    user_email: "",
                });
                const project = await API.get(`/projects/${projectId}`);
                renderTasks(project.tasks);
            } catch (err) {
                console.error("update fallido", err);
            }
        };
    });
}

function escapeHTML(s) {
    if (!s) return "";
    return s
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;")
        .replace(/'/g, "&#39;");
}

// Listener WS para live updates del board activo.
WS.on((msg) => {
    if (!currentBoardId) return;
    if (msg.project_id !== currentBoardId) return;
    if (msg.kind === "connected") return;
    // Refrescar el board cuando llega un evento del project actual.
    renderBoard(currentBoardId);
});

window.addEventListener("hashchange", route);
window.addEventListener("load", () => {
    if (API.token) WS.connect();
    route();
});
