# `@rpc` — server functions (Phase 11.11)

A `.fitzv` client that calls server functions directly, as if they were
local async calls. No hand-written `fetch`, no JSON glue, no route
strings — the compiler generates both halves from one `@rpc async fn`.

```
api.fitz      # the @rpc server functions (+ the User type)
server.fitz   # the server binary — imports api, mounts POST /__rpc/*
App.fitzv     # the client SPA — imports api, calls greet(...).await?
```

## How it works

`api.fitz` declares:

```fitz
@rpc
async fn greet(name: Str) -> Result<Str> {
  return Ok("Hello, {name}!")
}
```

- **Server half** (`fitz build --bin server`): the `@rpc` fn is mounted
  as `POST /__rpc/greet`. The request body is a JSON object
  (`{"name": "..."}`); each param is deserialized from its field, the
  fn runs, and its `Result<T>` becomes `200` + JSON (Ok) or `500` +
  `{"error": ...}` (Err).
- **Client half** (`fitz build --bin web --target wasm-client`): the
  imported `@rpc` fn is emitted as an async `fetch` stub. In an event
  handler, `greet(who).await?` serializes the args, POSTs to
  `/__rpc/greet` (same origin → the session cookie rides along),
  deserializes the reply, and — because the handler now `.await`s — the
  compiler wraps its body in `spawn_local` so state updates + a
  re-render fire when the reply arrives.

## Loading state — mid-flight render (Phase 11.10)

The handlers set `loading = true` **before** the `.await` and
`loading = false` after. The async worker flushes a render right before
it suspends, so `{#if loading}Loading…{/if}` paints while the request is
in flight, then clears when the reply resolves. A handler with no state
write before its `.await` (the plain fetch-then-assign pattern) emits
byte-identically — no extra render is inserted.

## Run it

```bash
# 1. build + start the server (mounts /__rpc/greet + /__rpc/get_user on :3838)
fitz build --bin server
./target/release/rpc-demo         # (or the produced binary)

# 2. build the client SPA to WASM
fitz build --bin web              # produces target/wasm/web/{web.js,web_bg.wasm}
```

Serve the SPA (`index.html` + `target/wasm/web/*`) from the **same
origin** as the server so the relative `fetch("/__rpc/...")` reaches it
(in production, have the server serve the static bundle, or put both
behind one reverse proxy). Open the page and click the buttons: the
greeting and the user name are fetched from the server and rendered.

## MVP notes

- A nominal type used across the wire (here `User`) must be imported
  into the `.fitzv` (`from api import ..., User`) so the client gets its
  struct.
- `@rpc` fns must be `async` and return `Result<T>` (checker-enforced).
- Auth stacked on the generated endpoint (`@authenticated` / `@admin`)
  is a post-MVP refinement — for now the same-origin session cookie is
  what travels with the request.
- The re-render fires once, when the reply arrives (a mid-request
  "loading…" flash is a later fine-grained-reactivity slice).
