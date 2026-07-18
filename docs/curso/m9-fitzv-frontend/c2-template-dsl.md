# C2 — Template DSL: interpolación, directivas, composición

**Pre-requisitos**: [C1 — Tu primer `.fitzv`](c1-primer-fitzv.md).
Ya tenés `mi-counter` corriendo, sabés qué es un `component`,
`state`, `event`, `<template>` y `<style scoped>`.

**Objetivo**: dominar las **5 features del template** del
`.fitzv` que este cap tocó por arriba: `{expr}` (interpolación
de expresiones), attribute interpolation, `{#if}` / `{#for}`
(directivas de control), `data-flv-*` attrs (wire de eventos),
y `<Child prop="v" />` (composición de components).

**Por qué importa**: el template es donde vivís la mayoría
del tiempo cuando escribís UI. Cuanto más natural sea la
sintaxis + más ricas las expresiones, menos escapes al plain
HTML/JS necesitás.

---

## Feature 1 — Interpolación de expresiones

Adentro del `<template>...</template>` (Y en attribute values
con la forma `attr="{expr}"`), Fitz reconoce **cualquier
expresión Fitz válida** dentro de `{...}`. El SSR emitter la
lowering a classic Fitz con las reglas de scoping:

1. Closure params (de `{#for x in xs}`) shadow todo.
2. State field name → rewrite a `state.<name>`.
3. Imported name → emit verbatim.
4. Otherwise → error del checker.

Ejemplos progresivos:

```fitzv
component App {
  state {
    count: Int = 0
    title: Str = "Hola"
    user: User = User { id: 1, name: "Ada" }
    tags: List<Str> = ["fitz", "sfc"]
  }

  <template>
    <!-- Bare state field ref -->
    <p>{count}</p>

    <!-- Field access -->
    <p>Hola, {user.name}!</p>

    <!-- Method call -->
    <p>{title.upper()}</p>

    <!-- Aritmética inline -->
    <p>Next: {count + 1}</p>

    <!-- Method chain -->
    <p>Tags: {tags.len()}</p>
  </template>
}
```

**Attribute interpolation** — con la forma `attr="{expr}"`, TODO
el value del attribute es una expresión:

```fitzv
<template>
  <!-- data-flv-value-* pasa el value al event handler payload -->
  <button data-flv-click="delete_item"
          data-flv-value-item_id="{item.id}">
    Delete
  </button>

  <!-- Interpolación en class attr -->
  <div class="badge-{status}">
    {status}
  </div>

  <!-- Nullable state field como class flag -->
  <input value="{title}" required />
</template>
```

📚 **Capítulo dedicado**: [cap 36 — Frontend nativo](../../guide.md#36-frontend-nativo-con-fitzv-sfc)
(sección "Interpolación de expresiones").

---

## Feature 2 — Directivas de control

Dos directivas hoy: `{#if}` / `{#else}` / `{/if}` (conditional
block) y `{#for x in xs}` / `{/for}` (iteration).

### `{#if}` / `{#else}` / `{/if}`

```fitzv
component App {
  state {
    is_logged_in: Bool = false
    user_name: Str = ""
  }

  <template>
    <div id="app">
      {#if is_logged_in}
        <p>Hola, {user_name}!</p>
        <button data-flv-click="logout">Logout</button>
      {#else}
        <p>No estás logueado.</p>
        <button data-flv-click="login">Login</button>
      {/if}
    </div>
  </template>
}
```

La `<cond>` puede ser **cualquier expresión que sintetice a
Bool**: state field bool (`is_logged_in`), comparison
(`count > 0`), method call (`items.len() > 0`), etc. El
`{#else}` es opcional (podés hacer solo `{#if} ... {/if}`).

### `{#for x in xs}` / `{/for}`

```fitzv
component App {
  state {
    tags: List<Str> = ["fitz", "sfc", "wasm"]
    items: List<Item> = []
  }

  <template>
    <div id="app">
      <!-- Iteración simple sobre List<Str> -->
      <ul>
        {#for tag in tags}
          <li class="tag">{tag}</li>
        {/for}
      </ul>

      <!-- Iteración sobre List<Item> con field access -->
      <ul>
        {#for item in items}
          <li class="item">
            <span>{item.name}</span>
            <span class="price">${item.price}</span>
          </li>
        {/for}
      </ul>
    </div>
  </template>
}
```

**Scoping**: `tag` y `item` son **closure params locales** del
body del `#for`. Shadow-ean any state field con el mismo
nombre (regla 1). Adentro del body, expresiones como
`{item.name}` acceden al closure param, no al state.

**Directivas anidadas** funcionan naturalmente:

```fitzv
<template>
  {#for category in categories}
    <section>
      <h2>{category.name}</h2>
      <ul>
        {#for item in category.items}
          {#if item.available}
            <li>{item.name} — ${item.price}</li>
          {/if}
        {/for}
      </ul>
    </section>
  {/for}
</template>
```

---

## Feature 3 — Wire de eventos con `data-flv-*`

`fitz-liveviews` reserva el namespace `data-flv-*` para wire
eventos del DOM a los event handlers del componente:

| Attribute | Cuando dispara | Payload en el event body |
|---|---|---|
| `data-flv-click="handler"` | Click del elemento | `payload` map vacío |
| `data-flv-submit="handler"` | Submit del `<form>` | `payload` con name/value de cada `<input>` |
| `data-flv-value-<key>="{expr}"` | Se **inyecta al payload** del próximo click/submit del ancestor | `payload["<key>"]` = el value |
| `data-flv-clear` | Post-submit del form: limpia el input | (sin payload) |

Ejemplo canónico — form submit + delete button:

```fitzv
component TodoList {
  state {
    items: List<Str> = []
    next_id: Int = 1
  }

  event add() {
    if (payload.has("text")) {
      items.push(payload["text"])
      next_id = next_id + 1
    }
  }

  event delete_item() {
    if (payload.has("index")) {
      let idx_str = payload["index"]
      // ... convertir a Int + remove ...
    }
  }

  <template>
    <div id="todo-app">
      <form data-flv-submit="add">
        <input name="text" required data-flv-clear />
        <button type="submit">Add</button>
      </form>

      <ul>
        {#for item in items}
          <li>
            {item}
            <button data-flv-click="delete_item"
                    data-flv-value-index="{item.index}">
              ×
            </button>
          </li>
        {/for}
      </ul>
    </div>
  </template>
}
```

**Cómo funciona el `data-flv-value-<key>`**: cuando el
usuario clickea el `<button>`, el runtime walka HACIA ARRIBA
del elemento buscando ancestros con `data-flv-value-*` attrs,
y agrega TODOS al payload del event. Esto te permite
"scoping" del payload a un elemento contenedor (una `<li>`
en el ejemplo — el `data-flv-value-index` está en el
`<button>` del ejemplo, pero podría estar en un ancestor).

---

## Feature 4 — Composición de components

Cuando declarás dos components en el MISMO `.fitzv`, uno puede
componer al otro con syntax XML-like `<Child />`:

```fitzv
component App {
  state { title: Str = "Mi Tablero" }

  <template>
    <div>
      <h1>{title}</h1>
      <Column name="To Do" />
      <Column name="In Progress" />
      <Column name="Done" />
    </div>
  </template>
}

component Column {
  state { name: Str = "" }

  <template>
    <section class="column">
      <h2>{name}</h2>
      <ul></ul>
    </section>
  </template>
}
```

**Props** — cada attribute matchea con un state field del
child:

| Attribute shape | Field type acepted |
|---|---|
| `name="Foo"` | `Str` primitive |
| `count="42"` | `Int` primitive |
| `active="true"` | `Bool` primitive |
| `tags="a,b,c"` | `List<Str>` (comma-separated, K-3) |
| `meta="k=v,x=y"` | `Map<Str, Str>` (k=v pairs, S.2) |
| `name="{state_ref}"` | Cualquier tipo compatible (interpolación, K-3 remainder) |

**Interpolación** — la forma poderosa. Podés pasar cualquier
expresión que el emitter pueda inline-ar con el scoping del
parent:

```fitzv
component Board {
  state { title: Str = "Mi tablero"  card_count: Int = 42 }
  <template>
    <!-- Interpolación de state field -->
    <Header title="{title}" />

    <!-- Interpolación con computación inline -->
    <Stats total="{card_count + 10}" />
  </template>
}
```

**Cross-file composition** — components declarados en OTRO
`.fitzv` (importados con `from Card import Card`) NO se pueden
componer con syntax `<Child />` hoy. Workaround: usar el
runtime API `component("Card", "instance-id")` del
`fitz-liveviews`. Ver cap C3 para el pattern completo.

---

## Feature 5 — Style scoped vs global

Ya viste `<style scoped>` en C1. Complemento: `<style global>`
NO aplica el rewriting — el CSS va tal cual al DOM. Útil para:

- Reset del body / html (`body { margin: 0 }`).
- Tipografía global (`html { font-family: system-ui }`).
- Utility classes que querés compartir (aunque para eso vale
  la pena tener un componente reset dedicado).

```fitzv
component GlobalStyles {
  state {}

  <template>
    <div>
      <!-- Este componente no renderiza nada visible; solo
           existe para inyectar CSS global. Instancialo una vez
           en el layout raíz. -->
    </div>
  </template>

  <style global>
    body {
      margin: 0;
      font-family: system-ui, sans-serif;
    }
    * { box-sizing: border-box; }
  </style>
}
```

**Cap tiene un solo `<style>` block** (scoped O global, no
ambos). Si necesitás CSS scoped Y CSS global desde el mismo
componente, extractá el global a un componente dedicado.

---

## Validación del capítulo

- **Ejercicio 1**: agregá un TodoList con `{#for}` en tu
  proyecto de C1. Deberías ver una lista que crece al submit.
- **Ejercicio 2**: agregá un `<Header title="{title}" />`
  componente al App. Editá el value en el state y refreshá —
  el header cambia sin recargar.

## Troubleshooting

| Síntoma | Causa probable | Fix |
|---|---|---|
| `identifier 'X' is not a state field nor an imported name` | Ident bajo `{...}` no matchea ningún scope válido | Verificá que sea state field, closure param del `{#for}`, o imported con `from` |
| Error del expander sobre `{#for}` malformado | Falta cerrar con `{/for}` (o closing wrong: `{end}`) | Cerrás con `{/for}` — el `/` es requerido |
| Element interpolation `name="{expr}"` no funciona pero `name="hi"` sí | Estás en versión pre-K-3 remainder (< v0.21.1) | Actualizá a fitz ≥ v0.21.1 |
| Component composition `<Child prop="{expr}" />` da error en WASM | WASM path no soporta interpolated props todavía (Phase 11.7+) | Usá static values, o usá SSR target |

## Qué sigue

- **[C3 — Full-page SFC: Board.fitzv migration del kanban](c3-full-page-sfc.md)** —
  el pattern de architecture "types en `.fitz` + helpers puros
  en `.fitz` + SFC en `.fitzv` + HTTP+WS thin wire-up en
  `main.fitz`" aplicado al kanban board completo (Session A +
  B validation criterion).
