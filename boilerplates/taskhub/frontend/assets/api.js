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
        const data = await resp.json().catch(() => ({}));
        if (!resp.ok) throw new Error(data.error || `HTTP ${resp.status}`);
        return data;
    },

    get(path)         { return this.req("GET", path); },
    post(path, body)  { return this.req("POST", path, body); },
    put(path, body)   { return this.req("PUT", path, body); },
    del(path)         { return this.req("DELETE", path); },
};
