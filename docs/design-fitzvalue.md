# Design doc — F13: `FitzValue` tagged runtime para heterogéneos en `fitz build`

**Estado**: SPIKE (2026-05-20). Prototipo mínimo implementado para
`List<Any>`; design completo pendiente como follow-up.

**Contexto**: F13 es la última deuda residual del bloque post-Fase-8.
El intérprete (`fitz run`) acepta listas/mapas heterogéneos
(`[1, "dos", true]`) naturalmente porque `Value` ya es un tagged
union. El codegen (`fitz build`) los rechaza con
`"listas con elementos de tipos mixtos: necesita tipo homogéneo
concreto"` porque Rust no tiene un tipo "Value" genérico en runtime.

## El problema

```fitz
let xs = [1, "dos", true]       // Tipo Fitz: List<Any>
print(xs)                        // En fitz run: [1, "dos", true]
                                 // En fitz build: error codegen
```

El checker sintetiza `Type::List(Type::Any)` para el literal mixto.
El codegen, al traducir `Type::List(Any)` → Rust, falla porque no
hay un equivalente directo: `Vec<???>` no compila.

## La solución propuesta

Introducir un tipo `__FitzValue` (enum tagged) emitido en el preludio
del codegen, paralelo al `Value` enum del intérprete. Las listas/
mapas heterogéneos se mapean a `Vec<__FitzValue>` / `Vec<(__FitzValue,
__FitzValue)>` en lugar de los tipos homogéneos actuales.

### Shape del enum

```rust
enum __FitzValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Bytes(Vec<u8>),
    List(Arc<Mutex<Vec<__FitzValue>>>),
    Map(Arc<Mutex<Vec<(__FitzValue, __FitzValue)>>>),
    // Tipos custom (Nominal): representación opaca via JSON.
    // Trade-off: pierde field access tipado, pero evita explotar
    // el enum con variantes por cada `type` del programa.
    Nominal(serde_json::Value),
}
```

### Activación

**Auto-detectada por el checker, NO opt-in**:

- Si `Type::List(inner)` tiene `inner == Type::Any`, el codegen
  emite el binding como `__FitzList = Arc<Mutex<Vec<__FitzValue>>>`
  (alias `__FitzList`).
- Si `Type::Map(k, v)` tiene `k == Any` o `v == Any`, idem con
  `__FitzMap = Arc<Mutex<Vec<(__FitzValue, __FitzValue)>>>`.
- Listas/maps con T concreto siguen emitiendo el tipo homogéneo
  actual (sin overhead de FitzValue para el caso 90%).
- `Type::Any` en un binding suelto (`let x: Any = ...`) podría
  también requerir `__FitzValue`, pero el SPIKE deja esto pendiente.

Justificación: el código del usuario que usa listas homogéneas
(la mayoría) sigue compilando idéntico, sin overhead nuevo. Solo
cuando se necesita el tagged union, se activa.

### Coerción

`__FitzValue::from_int(n: i64) -> __FitzValue`,
`__FitzValue::from_str(s: String) -> __FitzValue`, etc. Métodos
constructores por primitivo. El codegen los invoca al emitir cada
elemento del literal heterogéneo.

`__FitzValue::as_int() -> Option<i64>`, etc. Métodos extractores.
Pero NO se exponen al usuario Fitz — los heterogéneos se consumen
via Display/print, comparación, iteración con type checks dinámicos.

### Operaciones soportadas

- **Display**: paralelo a `fmt::Display for Value` del intérprete.
  Strings con comillas adentro de colecciones, Float con `.0` si
  fract=0, etc.
- **PartialEq**: byte a byte, coerción Int↔Float, recursivo en
  List/Map/Nominal. Paralelo a la impl del intérprete.
- **`__ToFitzJson`**: dispatch por variant. Cada variant emite el
  JSON correspondiente (Int → Number, Str → String, etc.). Nominal
  contiene ya un `serde_json::Value` → directo.
- **`__FromFitzJson`**: inspecciona el shape del JSON y elige
  variant (Number → Int o Float según fract, String → Str, etc.).

### Helpers en el preludio

Emitidos solo cuando `program_uses_fitz_value()` devuelve `true`
(gating idéntico a `uses_fmt_helpers`, `uses_python`).

```rust
#[derive(Clone, Debug)]
enum __FitzValue { ... }

impl __FitzValue {
    fn from_int(n: i64) -> Self { Self::Int(n) }
    fn from_float(x: f64) -> Self { Self::Float(x) }
    fn from_str(s: String) -> Self { Self::Str(s) }
    fn from_bool(b: bool) -> Self { Self::Bool(b) }
    fn null() -> Self { Self::Null }
    // ... etc
}

impl std::fmt::Display for __FitzValue {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{}", n),
            Self::Float(x) => __fitz_fmt_float(*x, f),
            Self::Str(s) => write!(f, "{}", s),
            // ... etc
        }
    }
}

impl PartialEq for __FitzValue { ... }

impl __ToFitzJson for __FitzValue { ... }
impl __FromFitzJson for __FitzValue { ... }
```

## Tradeoffs y limitaciones aceptados

1. **Nominales via JSON intermedio**: heterogéneos pueden contener
   instancias de `type User { ... }` pero pierde field access
   tipado adentro de la lista. El usuario tiene que convertir a
   un Map o serializar/deserializar para acceso.
   - Alternativa: `FitzValue::Instance(Box<dyn ToFitzJson>)` — más
     overhead y complejidad.
   - Decisión SPIKE: aceptar la limitación. Cap 9 de la guía
     documenta.

2. **`==`/`!=` entre FitzValue y tipo concreto**: si `xs: List<__FitzValue>`
   y se compara `xs[0] == 42`, el codegen necesita coercer `42` al
   variant FitzValue. Posible solución: emitir `__FitzValue::from_int(42)`
   automáticamente cuando uno de los lados es FitzValue.

3. **Method dispatch sobre FitzValue**: `xs[0].upper()` no funciona
   estáticamente — el tipo no se conoce. Workaround MVP: el usuario
   debe convertir antes (`xs[0].as_str().unwrap().upper()` o similar).
   - Decisión SPIKE: documentar como deuda residual, no implementar.

4. **Performance**: cada elemento del heterogéneo es un enum tag +
   payload (16 bytes mínimo en x64). Para colecciones grandes
   homogéneas, sigue siendo más eficiente usar el tipo concreto.
   El auto-detect mitiga esto: el usuario solo paga el overhead
   cuando lo necesita.

5. **Hash para Map keys**: `__FitzValue` necesita impl `Hash` para
   keys de map. Decisión: solo primitivos hashables (Int, Float,
   Str, Bool); Nominal/List/Map como keys → error de codegen claro.

## Prototipo mínimo (SPIKE) + extensión F13.A + F13.B

Alcance acumulado al cierre de las mini-tandas SPIKE + F13.A + F13.B:

- [x] Design doc (este archivo).
- [x] Detección en codegen: `Type::List(Type::Any)` y `Type::Map(Any, _)` /
  `Map(_, Any)` triggerean emisión del helper `__FitzValue`.
- [x] Helper `__FitzValue` con 7 variantes: Int/Float/Str/Bool/Null/
  Bytes/Nominal.
- [x] Display + PartialEq + constructores por variant.
- [x] Literal heterogéneo `[1, "dos", true, b"raw", User { ... }]`
  compila y produce output bit-a-bit idéntico a `fitz run`.
- [x] Map heterogéneo `{"name": "fitz", "count": 7, "on": true}`
  compila y produce paridad bit-a-bit (ambos lados — keys y values
  — se wrapean como FitzValue cuando al menos uno es Any).
- [x] Nominales adentro de heterogéneos: `__FitzValue::Nominal(String)`
  captura el Display del Data como String. Trade-off: pierde field
  access tipado en heterogéneos pero evita dependencia en
  `serde_json` (que solo se emite con HTTP) y mantiene el bundle
  manejable.
- [x] Bytes en heterogéneos: `__FitzValue::Bytes(Vec<u8>)`.
- [x] Quick win: `Value::Bytes` en JSON ahora se serializa como
  base64 string (encoder inline, sin dep externa).
- [x] 4 tests compile_e2e validando paridad bit-a-bit + reject
  de tipos no soportados.

**NO incluido** todavía (queda como follow-up dedicado):

- HTTP body con heterogéneos (`body: List<Any>` deserializado
  desde JSON entrante).
- Method dispatch sobre FitzValue (downcasting ergonómico:
  `xs[0].as_int() -> Option<Int>`, type checks dinámicos).
- Listas/Mapas anidados con mix interno (`[1, [2, 3]]` — el
  segundo item es List que el FitzValue actual no soporta).
- Functions y Tuples adentro de heterogéneos.
- Round-trip nominal vía JSON (hoy es lossy: captura Display,
  no preserva structure tipada).

## Follow-up estimado (post-F13.A + F13.B)

Para cerrar F13 completo: ~4-6h adicionales. Sub-pasos restantes:

1. ~~**F13.A — Map heterogéneo**~~ ✓ CERRADO 2026-05-20 (mini-tanda
   F13.A+B+base64).
2. ~~**F13.B — Nominales en FitzValue**~~ ✓ CERRADO 2026-05-20.
   Variante Nominal(String) que captura Display. Trade-off
   documentado (lossy).
3. **F13.C — HTTP body con heterogéneos** (~2h): `body: List<Any>`
   funciona end-to-end (JSON in → Vec<FitzValue>).
4. **F13.D — Method dispatch básico** (~2h): `xs[0].to_str()`,
   `xs[0].as_int()`, `xs[0].type_name()`. Sin pattern matching
   sobre variants (que requeriría syntax nueva del lenguaje).
5. **F13.E — Listas/Mapas anidados con mix interno** (~2h):
   `[1, [2, 3]]` requiere `FitzValue::List(Vec<FitzValue>)` y
   `FitzValue::Map(Vec<(FitzValue, FitzValue)>)`. Recursion en
   el enum.

## Por qué este spike

- Reduce el riesgo del follow-up grande: las decisiones de diseño
  más complejas quedan documentadas y validadas con un caso real
  mínimo.
- Habilita el caso visible (`print([1, "dos"])` compila) sin
  bloquear ni complicar el camino al follow-up completo.
- Si F13 nunca se cierra completo, el spike igualmente entrega
  valor: el caso de literal mixto print-only es el más común
  y el menos exigente.

## Referencias

- Intérprete: `src/value.rs` (`enum Value`).
- Codegen actual: `src/codegen.rs::rust_type_for` rechaza
  `Type::List(Type::Any)`.
- Deudas: `docs/deudas-post-5b.md` (F13), `docs/deudas_lenguaje.md`
  (entradas residual recurrente).
