// index.js — app Node 20 + Express 5 + Prisma 5 para el benchmark
// mixed-workload.
//
// Six endpoints idénticos a apps/fitz/ y apps/python/:
//
//   GET  /users?limit=N
//   GET  /users/:id
//   GET  /users/:id/posts
//   POST /users
//   POST /users/:id/posts
//   PUT  /users/:id
//
// Express 5 maneja errores en async handlers nativamente, sin
// `express-async-errors`. Prisma 5 con `pool_size` configurado
// via DATABASE_URL connection params (?connection_limit=20).

import express from 'express';
import { PrismaClient } from '@prisma/client';

// Pool size lo controlamos via query param de la connection URL
// (`connection_limit=20`). El docker-compose lo setea.
const prisma = new PrismaClient();
const app = express();

app.use(express.json());

// --- Schema init -----------------------------------------------------
//
// Prisma no tiene `metadata.create_all()` runtime equivalente a
// SQLAlchemy. Las tablas las crea `prisma db push` desde el schema
// — lo corremos en boot antes de `app.listen()` para que el
// container quede self-contained sin pasos manuales (paralelo a
// los apps Fitz y Python que crean schema en el primer boot).

async function ensureSchema() {
    // El comando `prisma db push --skip-generate` aplica el schema
    // (idempotente — solo crea lo que falta). Lo invocamos via
    // child_process una sola vez al boot.
    const { execSync } = await import('node:child_process');
    try {
        execSync('npx prisma db push --skip-generate --accept-data-loss', {
            stdio: 'inherit',
            env: process.env,
        });
        console.log('[boot] DB conectada y schema inicializado');
    } catch (e) {
        console.error('[boot] ERROR aplicando schema —', e.message);
        // No abortamos — paralelo a los apps Fitz y Python que loguean
        // y siguen, el primer request va a fallar con detalle si la
        // DB no está accesible.
    }
}

// --- Handlers --------------------------------------------------------

// GET /users?limit=N — lista paginada (default 20).
app.get('/users', async (req, res, next) => {
    try {
        const limit = parseInt(req.query.limit, 10) || 20;
        const users = await prisma.user.findMany({
            orderBy: { id: 'asc' },
            take: limit,
        });
        res.json(users.map(formatUser));
    } catch (e) { next(e); }
});

// GET /users/:id — single read.
app.get('/users/:id', async (req, res, next) => {
    try {
        const id = parseInt(req.params.id, 10);
        const user = await prisma.user.findUniqueOrThrow({ where: { id } });
        res.json(formatUser(user));
    } catch (e) {
        if (e.code === 'P2025') {
            // Prisma "no record found" error.
            res.status(500).json({ error: `NotFound: User ${req.params.id}` });
            return;
        }
        next(e);
    }
});

// GET /users/:id/posts — posts de un user.
app.get('/users/:id/posts', async (req, res, next) => {
    try {
        const userId = parseInt(req.params.id, 10);
        const posts = await prisma.post.findMany({
            where: { userId },
            orderBy: { id: 'asc' },
        });
        res.json(posts.map(formatPost));
    } catch (e) { next(e); }
});

// POST /users — crear user.
app.post('/users', async (req, res, next) => {
    try {
        const { name, email } = req.body;
        const user = await prisma.user.create({ data: { name, email } });
        res.json(formatUser(user));
    } catch (e) { next(e); }
});

// POST /users/:id/posts — crear post asociado a user.
app.post('/users/:id/posts', async (req, res, next) => {
    try {
        const userId = parseInt(req.params.id, 10);
        const { title, body } = req.body;
        const post = await prisma.post.create({
            data: { userId, title, body: body ?? '' },
        });
        res.json(formatPost(post));
    } catch (e) { next(e); }
});

// PUT /users/:id — update name/email.
app.put('/users/:id', async (req, res, next) => {
    try {
        const id = parseInt(req.params.id, 10);
        const { name, email } = req.body;
        const user = await prisma.user.update({
            where: { id },
            data: { name, email },
        });
        res.json(formatUser(user));
    } catch (e) { next(e); }
});

// --- Error handling --------------------------------------------------
//
// Express 5: errores async se propagan a este handler automático.
// Convención del bench: error genérico → 500 con `{"error": msg}`
// (paralelo a Fitz/Python).
app.use((err, req, res, _next) => {
    console.error('[error]', err.message);
    res.status(500).json({ error: err.message || 'Internal server error' });
});

// --- Helpers ---------------------------------------------------------
//
// Prisma devuelve `createdAt` como Date objeto. Convertimos a ISO
// para que el JSON wire matchee bit-a-bit el de Fitz/Python (Str ISO).

function formatUser(u) {
    return {
        id: u.id,
        name: u.name,
        email: u.email,
        created_at: u.createdAt ? u.createdAt.toISOString() : '',
    };
}

function formatPost(p) {
    return {
        id: p.id,
        user_id: p.userId,
        title: p.title,
        body: p.body,
        created_at: p.createdAt ? p.createdAt.toISOString() : '',
    };
}

// --- Boot ------------------------------------------------------------

const PORT = 3000;

ensureSchema().then(() => {
    app.listen(PORT, '0.0.0.0', () => {
        console.log(`[ready] Server arrancando en :${PORT}`);
        console.log('[ready] Endpoints (mixed-workload bench):');
        console.log('[ready]   GET  /users?limit=N           — lista paginada');
        console.log('[ready]   GET  /users/<id>              — single read');
        console.log('[ready]   GET  /users/<id>/posts        — posts del user');
        console.log('[ready]   POST /users                   — crear user');
        console.log('[ready]   POST /users/<id>/posts        — crear post');
        console.log('[ready]   PUT  /users/<id>              — update');
    });
});

// Cleanup on signal.
function shutdown() {
    console.log('[shutdown] cerrando conexiones...');
    prisma.$disconnect().finally(() => process.exit(0));
}
process.on('SIGTERM', shutdown);
process.on('SIGINT', shutdown);
