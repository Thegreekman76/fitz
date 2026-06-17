// mixed.js — scenario principal: workload realista 60% reads + 40% writes
// con VUs rampeando 10→50→100→50 sobre 3 minutos.
//
// Mix por VU (de un total de 100 unidades de tráfico):
//   30 → GET /users?limit=20
//   15 → GET /users/{id}/posts          (JOIN/preload realista)
//   15 → GET /users/{id}
//   20 → POST /users
//   15 → POST /users/{id}/posts
//    5 → PUT /users/{id}
//
// → 60 reads + 40 writes total = mix típico de un servicio web
//    con read-replicas, según patrones de produccion mid-traffic.
//
// Ramp-up shape:
//   0:00 → 0:30   ramp 0 → 10  (calentamiento)
//   0:30 → 1:00   sostener 10  (baseline)
//   1:00 → 1:30   ramp 10 → 50  (carga media)
//   1:30 → 2:00   sostener 50
//   2:00 → 2:30   ramp 50 → 100 (peak)
//   2:30 → 3:00   sostener 100
//   3:00 → 3:30   ramp 100 → 50 (descenso)

import { sleep } from 'k6';

import {
    BASE_URL, SEED_USERS, randUserId,
    getListUsers, getSingleUser, getUserPosts,
    postUser, postUserPost, putUser,
    checkOK, checkPutTolerant,
} from './common.js';

export const options = {
    scenarios: {
        mixed: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '30s', target: 10 },
                { duration: '30s', target: 10 },
                { duration: '30s', target: 50 },
                { duration: '30s', target: 50 },
                { duration: '30s', target: parseInt(__ENV.BENCH_VUS_MAX || '100', 10) },
                { duration: '30s', target: parseInt(__ENV.BENCH_VUS_MAX || '100', 10) },
                { duration: '30s', target: 50 },
            ],
            gracefulRampDown: '10s',
        },
    },
    // Saturation thresholds — el run sigue corriendo pero estos
    // umbrales aparecen en el summary final. Útiles para detectar
    // cuándo el stack ya no aguanta.
    thresholds: {
        // Latencia agregada sobre todos los endpoints.
        'http_req_duration': [
            { threshold: 'p(50)<100', abortOnFail: false },
            { threshold: 'p(95)<500', abortOnFail: false },
            { threshold: 'p(99)<1000', abortOnFail: false },
        ],
        // Error rate global (sin contar 5xx por unique-constraint que
        // checkPutTolerant deja pasar — esos quedan como 500 contables).
        'http_req_failed': [
            { threshold: 'rate<0.05', abortOnFail: false },
        ],
    },
    // Cero noisy logs en stdout del k6 — el orchestrator necesita
    // el JSON limpio.
    summaryTrendStats: ['avg', 'min', 'med', 'p(90)', 'p(95)', 'p(99)', 'p(99.9)', 'max'],
};

// VU loop: cada iteración elige un endpoint según el mix definido.
// Tirar dado [1..100] y mapear a buckets cumulativos.
export default function () {
    const dice = Math.random() * 100;
    const vu = __VU;
    const iter = __ITER;

    if (dice < 30) {
        // GET /users?limit=20
        const res = getListUsers(20);
        checkOK(res, 'list_users');
    } else if (dice < 45) {
        // GET /users/{id}/posts
        const id = randUserId();
        const res = getUserPosts(id);
        checkOK(res, 'user_posts');
    } else if (dice < 60) {
        // GET /users/{id}
        const id = randUserId();
        const res = getSingleUser(id);
        checkOK(res, 'single_user');
    } else if (dice < 80) {
        // POST /users
        const res = postUser(vu, iter);
        checkOK(res, 'create_user');
    } else if (dice < 95) {
        // POST /users/{id}/posts
        const id = randUserId();
        const res = postUserPost(id, vu, iter);
        checkOK(res, 'create_post');
    } else {
        // PUT /users/{id}
        const id = randUserId();
        const res = putUser(id, vu, iter);
        checkPutTolerant(res, 'update_user');
    }

    // Sleep mínimo — simula "think time" entre requests del user.
    // Sin él los VUs hacen requests back-to-back a velocidad máxima,
    // que NO es realista para tráfico de usuarios reales.
    sleep(0.1);
}
