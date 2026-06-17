// writes-only.js — scenario write-heavy: 50 VUs sostenidos 1min,
// mix de POST user / POST post / PUT user.
//
// Mide el "ceiling" de escritura — el caso que el bench anterior
// NO cubre (`orm-vs-sqlalchemy` hace POST sequential con curl loop).
// Acá tenemos write concurrency real con saturación del pool de
// conexiones de cada ORM.

import { sleep } from 'k6';

import {
    SEED_USERS, randUserId,
    postUser, postUserPost, putUser,
    checkOK, checkPutTolerant,
} from './common.js';

export const options = {
    scenarios: {
        writes: {
            executor: 'constant-vus',
            vus: parseInt(__ENV.BENCH_VUS_FOCUSED || '50', 10),
            duration: __ENV.BENCH_DURATION_FOCUSED || '60s',
        },
    },
    thresholds: {
        'http_req_duration': [
            { threshold: 'p(95)<500', abortOnFail: false },
        ],
        'http_req_failed': [
            { threshold: 'rate<0.05', abortOnFail: false },
        ],
    },
    summaryTrendStats: ['avg', 'min', 'med', 'p(90)', 'p(95)', 'p(99)', 'p(99.9)', 'max'],
};

export default function () {
    const dice = Math.random() * 100;
    const vu = __VU;
    const iter = __ITER;

    if (dice < 50) {
        // POST /users — write dominante.
        const res = postUser(vu, iter);
        checkOK(res, 'create_user');
    } else if (dice < 85) {
        // POST /users/{id}/posts — write con FK lookup.
        const id = randUserId();
        const res = postUserPost(id, vu, iter);
        checkOK(res, 'create_post');
    } else {
        // PUT /users/{id} — update.
        const id = randUserId();
        const res = putUser(id, vu, iter);
        checkPutTolerant(res, 'update_user');
    }

    sleep(0.05);
}
