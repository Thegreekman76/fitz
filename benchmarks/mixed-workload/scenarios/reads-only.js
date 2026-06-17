// reads-only.js — scenario read-heavy: 50 VUs sostenidos 1min,
// mix de las 3 lecturas: list / single / posts.
//
// Útil para medir el "ceiling" de lectura del stack sin escrituras
// que comprometan el pool de conexiones. Es el escenario que el
// bench anterior (`orm-vs-sqlalchemy`) mide aislado por endpoint
// con oha — acá lo medimos con mix.

import { sleep } from 'k6';

import {
    SEED_USERS, randUserId,
    getListUsers, getSingleUser, getUserPosts,
    checkOK,
} from './common.js';

export const options = {
    scenarios: {
        reads: {
            executor: 'constant-vus',
            vus: parseInt(__ENV.BENCH_VUS_FOCUSED || '50', 10),
            duration: __ENV.BENCH_DURATION_FOCUSED || '60s',
        },
    },
    thresholds: {
        'http_req_duration': [
            { threshold: 'p(95)<300', abortOnFail: false },
        ],
        'http_req_failed': [
            { threshold: 'rate<0.01', abortOnFail: false },
        ],
    },
    summaryTrendStats: ['avg', 'min', 'med', 'p(90)', 'p(95)', 'p(99)', 'p(99.9)', 'max'],
};

export default function () {
    const dice = Math.random() * 100;

    if (dice < 50) {
        const res = getListUsers(20);
        checkOK(res, 'list_users');
    } else if (dice < 75) {
        const id = randUserId();
        const res = getUserPosts(id);
        checkOK(res, 'user_posts');
    } else {
        const id = randUserId();
        const res = getSingleUser(id);
        checkOK(res, 'single_user');
    }

    sleep(0.05);
}
