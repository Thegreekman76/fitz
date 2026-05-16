# `fitz fmt` — Referencia de estilo

Esta es la referencia formal del estilo canónico que aplica
`fitz fmt`. Fitz adopta la filosofía **cero config**, estilo gofmt
— no hay opciones configurables. La uniformidad cross-codebase es
más valiosa que la preferencia individual.

Si discrepás con una decisión, abrí un issue con tu argumento; las
reglas pueden ajustarse, pero NO se configuran por proyecto.

## Reglas principales

### Indentación

- **4 espacios** por nivel. No tabs.
- Bloques siempre con `{` al final de la línea de apertura.

### Strings

- **Comillas dobles** siempre: `"hola"`, nunca `'hola'`.
- Strings con interpolación preservan `"{expr}"` literal.

### Blank lines

- **Máximo 1 blank line consecutiva**. Múltiples del usuario se
  colapsan a 1.
- **Blank obligatoria entre fn/type top-level** consecutivos.
- Dentro de bloques: blank lines 100% user-driven (no se insertan
  automáticamente).
- **Suprimida si hay leading comment**: cuando un comment
  precede a un fn/type, no se inserta blank entre el comment y
  el stmt — el comment se atá al stmt siguiente.

### Comments

- **Estilo `//` line comments**: normalizados con espacio
  post-`//`. `//foo` → `// foo`.
- **Trailing comments**: 2 espacios de separación entre el código
  y el `//`. `let x = 1  // explicación`.
- **Block comments `/* */`**: preservados raw (sin normalización
  interna).
- **Posición preservada**: comments entre stmts mantienen su
  ubicación original.

### Trailing commas

- **Solo en multi-línea**: match arms y type fields obtienen
  trailing comma cuando se emiten multi-línea.
- En single-line (listas, maps, args de fn inline): sin trailing
  comma.

### Operadores binarios

- Espacios alrededor: `a + b`, `x == y`, nunca `a+b`.
- Operadores lógicos como keywords: `and` / `or` / `not` (NO `&&` /
  `||` / `!`).

### Paréntesis

- **Obligatorios en condición** de `if`/`while`: `if (cond) { ... }`.
- `for` y `loop` sin paréntesis: `for x in 0..10 { ... }`,
  `loop { ... }`.

### Definiciones de tipo (`type`)

- **Siempre multi-línea** con un field por línea (regardless de
  cómo lo haya escrito el user). Defaults con `= value` al final
  del field.

### Funciones (`fn`)

- Forma de bloque siempre: `fn f() { return ... }`. La forma
  flecha (`fn f() => expr`) la convierte el parser a bloque, y
  el formatter NO la restaura (deuda menor del AST).
- **FnExpr inline** (`fn(x) => expr` en posición de expresión)
  preserva la flecha si el body es un único `Return`.

### Imports

- Una sola línea cada uno. `import foo` o `from foo import a, b`.
- Aliases preservados: `import foo as f`, `from foo import bar as b`.

## Lo que el formatter NO hace (deuda residual)

- **Auto-wrap de líneas largas**: no rompe líneas que excedan los
  100 chars. El user es responsable. Soft limit, no enforced.
- **Multi-líneas user-formateadas se colapsan**: listas, maps y
  method chains que el user formateó multi-línea para
  legibilidad se inlinean si entran. No hay heurística que
  detecte intención del user. Deuda futura.
- **Comments adentro de expresiones** (`f(x, // foo\n y)`): no
  soportados. Si aparecen, pueden quedar mal posicionados.
- **Comments entre último stmt de un bloque y el `}`** (`fn f() {
  x = 1\n // trailing\n}`): pueden terminar fuera del bloque al
  re-formatear. Caso raro.
- **Format on save desde el LSP** (`textDocument/formatting`): no
  conectado todavía. `fmt::format_source` es library-able, así
  que el wiring desde el LSP es trivial — pendiente cuando
  aparezca demanda.

## Uso del CLI

### Formatear archivos explícitos

```bash
fitz fmt src/main.fitz src/utils.fitz
```

Modifica los archivos in-place. Reporta `✓ formateado <path>` por
cada uno que cambió. Si el archivo ya estaba canónico, silencioso.

### Formatear todo el proyecto

```bash
fitz fmt
```

Requiere `fitz.toml` (manifest mode). Hace walk recursivo de
`src/` excluyendo `target/` y directorios ocultos.

### Modo CI / check

```bash
fitz fmt --check
fitz fmt --check src/main.fitz
```

**Read-only**: no modifica nada. Exit code 0 si todos los archivos
están en forma canónica, 1 si alguno difiere. Útil en pre-commit
hooks o pipelines CI.

### Errores

- Si un archivo tiene errores de sintaxis, el formatter aborta
  para ese archivo con el error del parser. Los otros siguen.
- Si no hay `fitz.toml` en project mode, error claro pidiendo
  archivos explícitos o `fitz new`.

## Idempotencia

`fitz fmt` es **idempotente**: aplicarlo dos veces produce el
mismo resultado que aplicarlo una vez. Si `fitz fmt --check`
después de `fitz fmt` reporta diffs, es un bug. Reportalo.

## Historia

- **9.z.1.a** (2026-05-16) — pretty-printer básico sobre el AST.
  CLI con warning loud porque modo write borraba comments y
  blank lines.
- **9.z.1.b** (2026-05-16) — comment + blank line preservation
  vía lexer `Trivia` side-stream. Warning loud removido,
  formatter production-ready.
