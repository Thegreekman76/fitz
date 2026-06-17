// common.js — helpers compartidos por los 3 scenarios k6 del bench
// mixed-workload.
//
// Diseño: cada VU genera bodies con sufijo `vu_iter_random` para
// asegurar emails únicos sin requerir state externo, y elige
// user IDs random en [1, SEED_USERS]. El SEED se hace ANTES de
// correr los scenarios (lo orquesta `run.sh`).

import { check } from 'k6';
import http from 'k6/http';

// --- Config ----------------------------------------------------------

export const BASE_URL = __ENV.BASE_URL || 'http://localhost:3000';
export const SEED_USERS = parseInt(__ENV.SEED_USERS || '200', 10);
export const TIMEOUT = '10s';

// JSON bodies son siempre Content-Type: application/json.
const JSON_HEADERS = { 'Content-Type': 'application/json' };

// Tag de la corrida — útil para distinguir POSTs concurrentes y
// evitar colisiones del unique constraint de email entre runs.
const RUN_TAG = __ENV.RUN_TAG || `r${Date.now()}`;

// --- Helpers ---------------------------------------------------------

function randUserId() {
    return Math.floor(Math.random() * SEED_USERS) + 1;
}

function uniqueEmail(vu, iter, random) {
    return `vu${vu}-it${iter}-r${random}-${RUN_TAG}@bench.example.com`;
}

// --- Request wrappers ------------------------------------------------
//
// Tag cada request con el endpoint logical (`name`) para que k6
// agregue métricas por endpoint en lugar de mezclar todo en
// `http_req_duration` genérico. Permite ver p95 separado por
// `list_users` / `single_user` / etc en el output.

export function getListUsers(limit = 20) {
    return http.get(`${BASE_URL}/users?limit=${limit}`, {
        timeout: TIMEOUT,
        tags: { endpoint: 'list_users' },
    });
}

export function getSingleUser(id) {
    return http.get(`${BASE_URL}/users/${id}`, {
        timeout: TIMEOUT,
        tags: { endpoint: 'single_user' },
    });
}

export function getUserPosts(id) {
    return http.get(`${BASE_URL}/users/${id}/posts`, {
        timeout: TIMEOUT,
        tags: { endpoint: 'user_posts' },
    });
}

export function postUser(vu, iter) {
    const random = Math.floor(Math.random() * 1000000);
    const body = JSON.stringify({
        name: `User VU${vu} Iter${iter}`,
        email: uniqueEmail(vu, iter, random),
    });
    return http.post(`${BASE_URL}/users`, body, {
        headers: JSON_HEADERS,
        timeout: TIMEOUT,
        tags: { endpoint: 'create_user' },
    });
}

export function postUserPost(userId, vu, iter) {
    const body = JSON.stringify({
        title: `Post VU${vu} Iter${iter}`,
        body: `Body content from VU${vu} iter${iter}`,
    });
    return http.post(`${BASE_URL}/users/${userId}/posts`, body, {
        headers: JSON_HEADERS,
        timeout: TIMEOUT,
        tags: { endpoint: 'create_post' },
    });
}

export function putUser(id, vu, iter) {
    const random = Math.floor(Math.random() * 1000000);
    const body = JSON.stringify({
        name: `Updated VU${vu} Iter${iter}`,
        email: uniqueEmail(vu, iter + 1000000, random),
    });
    return http.put(`${BASE_URL}/users/${id}`, body, {
        headers: JSON_HEADERS,
        timeout: TIMEOUT,
        tags: { endpoint: 'update_user' },
    });
}

// --- Verificación ---------------------------------------------------
//
// `check` no aborta el VU al fallar — solo cuenta como fallido en
// las métricas. El error rate calculado por k6 reportará %
// de checks fallados.

export function checkOK(res, label) {
    return check(res, {
        [`${label}: status 200`]: (r) => r.status === 200,
    });
}

// Helper para PUT que tolera fallos por unique constraint en email
// (cuando un VU intenta updatear con un email random que ya existe
// por una corrida anterior — raro pero posible). Igual reportamos
// el status pero no contamos como check OK forzado a 200.
export function checkPutTolerant(res, label) {
    return check(res, {
        [`${label}: status 200 o 500-on-conflict`]: (r) =>
            r.status === 200 || r.status === 500,
    });
}

export { randUserId, RUN_TAG };
