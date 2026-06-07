// ws.js — cliente WebSocket simple.

const WS = {
    socket: null,
    listeners: [],

    connect() {
        if (this.socket) return;
        if (!API.token) return;
        const proto = location.protocol === "https:" ? "wss:" : "ws:";
        this.socket = new WebSocket(
            `${proto}//${location.host}/ws/events`,
            `bearer.${API.token}`
        );
        this.socket.onmessage = (ev) => {
            try {
                const msg = JSON.parse(ev.data);
                this.listeners.forEach((cb) => cb(msg));
            } catch (e) {
                console.warn("WS frame inválido", e);
            }
        };
        this.socket.onclose = () => {
            this.socket = null;
            // Reconnect después de 2s si todavía estamos logueados.
            if (API.token) setTimeout(() => this.connect(), 2000);
        };
    },

    on(cb) { this.listeners.push(cb); },

    send(msg) {
        if (this.socket && this.socket.readyState === WebSocket.OPEN) {
            this.socket.send(JSON.stringify(msg));
        }
    },

    disconnect() {
        if (this.socket) {
            this.socket.close();
            this.socket = null;
        }
    },
};
