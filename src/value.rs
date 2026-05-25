// value.rs — Fase 2.4 (Shared migrado a Arc<Mutex<>> en F17.2)
//
// Representación de valores en runtime. Un programa Fitz se evalúa a un árbol
// de `Value`s. Esta es la moneda con la que opera el evaluador.
//
// Notas de diseño:
//  - Los floats e ints se promueven mutuamente en operaciones (1 + 1.0 == 2.0)
//    igual que en Python. Acá solo definimos los datos; la promoción la hace
//    el evaluador.
//  - `Display` muestra los valores como los vería el usuario al hacer `print`.
//    Strings van sin comillas, floats siempre con `.0` si no tienen decimales
//    (para distinguirlos visualmente de ints).
//  - `PartialEq` está implementado a mano porque la comparación entre Int y
//    Float requiere coerción (1 == 1.0 → true). Si lo derivamos, esa igualdad
//    daría false.
//  - `Value::Function` guarda un handle (`EnvRef`) al environment donde la
//    función fue definida. Esto crea una dependencia mutua value↔env, pero
//    Rust la acepta porque `Arc<Mutex<>>` es una indirección: el tamaño
//    de `Value` no depende del tamaño de `Environment`.
//  - **F17.2**: `Shared<T>` migrado de `Rc<RefCell<T>>` a
//    `Arc<parking_lot::Mutex<T>>`. El cambio es transparente para los call
//    sites que usaban `.borrow()`/`.borrow_mut()` → ambos pasan a `.lock()`
//    (parking_lot::Mutex no distingue lectura de escritura).
//  - **F17.3**: `Value` ya es `Send` post-F17.2 (los contenedores son
//    `Arc<Mutex<>>`) y el evaluator pasó a `#[async_recursion]` sin
//    `(?Send)`. `FitzFuture` carga `+ Send` ahora. Queda F17.4 para
//    switchear el runtime tokio a `rt-multi-thread` y F17.5 para
//    eliminar el bridge HTTP mpsc/oneshot.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::ast::{Field, Param, Stmt};
use crate::env::EnvRef;
use crate::error::FitzResult;

/// Future pendiente del evaluator. Se construye al llamar una `async fn`
/// Fitz sin `.await` (guardar el future suelto) o desde builtins async
/// como `sleep`. `.await` lo desempaca al `FitzResult<Value>` interno.
///
/// **`+ Send` post-F17.3**: el evaluator pasó a `#[async_recursion]` sin
/// `(?Send)`. Eso pide que cada future del eval sea `Send`, lo que se
/// propaga acá: los `Value::Future` que el lenguaje expone también
/// tienen que cargar un future `Send`. La condición se cumple porque
/// los contenedores compartidos (`Shared<T>` = `Arc<Mutex<T>>`,
/// `EnvRef`) ya son `Send` post-F17.2, y el resto de los capturados
/// del eval (`Vec<Stmt>`, `Param`, `Value` shallow) ya cumplían el
/// bound de antes. Habilita `tokio::spawn` y `rt-multi-thread`.
pub type FitzFuture = Pin<Box<dyn Future<Output = FitzResult<Value>> + Send>>;

/// Mini-tanda Mw-Wrap — wrapper opaco para el callback de
/// `Value::NativeFn`. El `Arc` permite clone barato (los `Value` se
/// clonan a lo largo del pipeline). Send + Sync para fluir a través
/// de tokio runtimes multi-thread (post-F17.4). El input es
/// `Vec<Value>` para uniformar con la convención del resto de las
/// llamadas (aridad 0 = vec vacío); para `next: Fn() -> Response`
/// siempre llega vacío. Wrapper struct (no type alias) para poder
/// implementar `Debug` (el `dyn Fn` no lo deriva).
#[derive(Clone)]
pub struct NativeAsyncFn(pub Arc<dyn Fn(Vec<Value>) -> FitzFuture + Send + Sync>);

impl std::fmt::Debug for NativeAsyncFn {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "NativeAsyncFn(<native>)")
    }
}

/// Wrapper sobre el future pendiente que aporta `Debug` manual. El
/// `dyn Future` no implementa `Debug` así que no podemos derivarlo
/// en `Value`. La celda envuelve `Option<...>` para que `.take()`
/// extraiga el future al hacer `.await` sin clonar (los futures se
/// consumen una sola vez).
///
/// **F17.2-3**: usa `Arc<Mutex<>>` igual que el resto de `Shared<T>`.
/// Como `FitzFuture` carga `+ Send` post-F17.3, `Mutex<Option<FitzFuture>>`
/// es `Send + Sync` y `FutureCell` es Send — un `Value::Future` puede
/// viajar entre tareas tokio cuando el runtime sea `rt-multi-thread`
/// (F17.4).
pub struct FutureCell(pub Arc<Mutex<Option<FitzFuture>>>);

/// Fase 9.w.2 — Handle opaco a una conexión WebSocket abierta. Lo
/// construye el runtime HTTP tras el upgrade HTTP→WS y se inyecta al
/// handler `@ws("/path")` como `Value::WsConn(Arc<WsConnHandle>)`.
///
/// Diseño:
///   - `rx`: read half del WebSocket (axum SplitStream). `recv()` lo
///     locka, awaitea el próximo frame, parsea contra T.
///   - `outbox_tx`: un mpsc channel del conn que un "writer task"
///     drena → empuja al sink del socket. `send(msg)` y
///     `broadcast(msg)` empujan al outbox sin contender por el sink.
///   - `broadcaster`: shared handle al registry per-endpoint. Permite
///     que `broadcast(msg)` itere los outboxes de TODOS los conns
///     vivos del endpoint (incluyendo el sender — convención
///     Socket.IO/Phoenix).
///   - `endpoint`: path del decorator `@ws("/x")`. Scope del broadcast.
///   - `conn_id`: id único del conn dentro del broadcaster, para
///     unregister al cerrar.
///   - `closed`: flag atomic que `close()` setea. Métodos chequean
///     antes de cualquier operación para fail-fast con `Err` claro.
///
/// El tipo concreto vive en `http.rs` para evitar leak de tipos
/// axum/tokio-tungstenite a `value.rs`. Acá lo declaramos como `dyn`
/// opaco con los métodos mínimos que el evaluator/codegen necesitan
/// dispatcheable.
///
/// Solo existe en runtime — Display imprime `<ws-conn>`, type_name
/// `WsConn`, JSON serialization rechaza (la conn no es marshalleable
/// a JSON; el `T` que ella transporta sí lo es individualmente).
pub struct WsConnHandle {
    /// Path del endpoint (e.g. `"/chat"`).
    pub endpoint: String,
    /// Id único del conn dentro del broadcaster. Único hasta restart
    /// del server. AtomicU64 garantiza no-colisión bajo concurrencia.
    pub conn_id: u64,
    /// Read half del WebSocket. `recv()` lo locka mientras espera el
    /// próximo frame; durante ese tiempo `send`/`broadcast` siguen
    /// libres (locks separados).
    ///
    /// Usamos `tokio::sync::Mutex` (no `parking_lot::Mutex`) porque
    /// `recv()` necesita sostener el lock a través de un `.await`
    /// — solo `tokio::sync::Mutex` garantiza `MutexGuard: Send` para
    /// uso en futures `Send`. El resto del codebase usa parking_lot
    /// para locks sync, pero acá el patrón es async-aware.
    pub rx: Arc<tokio::sync::Mutex<WsReadStream>>,
    /// Outbox del conn. `send(msg)` y los `broadcast(msg)` de OTROS
    /// conns escriben acá; un writer task drena → empuja al sink del
    /// socket. Unbounded para no bloquear el handler.
    pub outbox_tx: tokio::sync::mpsc::UnboundedSender<WsOutMessage>,
    /// Flag atomic — `true` cuando el conn se cerró (handler retornó,
    /// `close()` invocado, o el writer task detectó el sink cerrado).
    /// Los métodos del conn lo chequean al entrar para fail-fast.
    pub closed: Arc<std::sync::atomic::AtomicBool>,
    /// Handle al broadcaster compartido (per `HttpRegistry`). Permite
    /// que `broadcast(msg)` busque los outboxes del endpoint sin
    /// pasar por `HttpRegistry`.
    pub broadcaster: Arc<dyn WsBroadcasterTrait + Send + Sync>,
    /// Fase 9.w.2 — TypeExpr del T en `WsConn<T>`. `recv()` lo usa
    /// para coercer `Map` recibidos a `Instance` cuando T es nominal
    /// (paralelo a la coerción 8.4.3 sobre `Stmt::Assign`). `None`
    /// para conns construidos en tests sin contexto de tipo.
    ///
    /// 9.w.2-wsconn-bidir (v0.9.38): `recv_type` y `send_type` pueden
    /// diferir para canales asimétricos (`WsConn<In, Out>`). `recv()`
    /// usa `recv_type` para deserializar/coercer; `send()`/`broadcast()`
    /// usan `send_type` para detectar modo binary vs text JSON. Para
    /// `WsConn<T>` simétrico, los dos son `Some(T)` con el mismo
    /// TypeExpr.
    pub msg_type: Option<crate::ast::TypeExpr>,
    pub send_type: Option<crate::ast::TypeExpr>,
    /// Fase 9.w.2 — EnvRef del scope donde se declaró el handler.
    /// Necesario para resolver `msg_type` cuando `T` es nominal (el
    /// `Value::Type` del nominal vive en el env). `Arc<Mutex<>>`
    /// — clon barato.
    pub env: EnvRef,
}

impl std::fmt::Debug for WsConnHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("WsConnHandle")
            .field("endpoint", &self.endpoint)
            .field("conn_id", &self.conn_id)
            .field(
                "closed",
                &self.closed.load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish()
    }
}

/// Alias para el read half — typedef interno para no leakear el tipo
/// concreto `axum::extract::ws::WebSocket`'s SplitStream a value.rs.
/// El struct concreto vive en `http.rs` y se castea a `Box<dyn>` o
/// se almacena como tipo concreto vía generics en el handler.
///
/// Decisión MVP: usamos un trait object para abstraerlo. El read
/// half concreto se castea al impl `WsReadStreamImpl` definido en
/// http.rs. Esto evita que `value.rs` dependa de `axum` directo.
pub type WsReadStream = Box<dyn WsReadStreamTrait + Send + Unpin>;

/// Frame entrante leído por el read half. Distingue text (modo
/// JSON-marshalled, default para `WsConn<T>` con T ≠ Bytes) de
/// binary (modo raw, exclusivo de `WsConn<Bytes>` — 9.w.2-binary-frames).
///
/// El read stream NUNCA filtra entre text y binary: ambos se exponen
/// al evaluator/codegen, que discrimina según el T declarado en el
/// `WsConn<T>` del handler. Mismatch (T=Str pero llega Binary, o T=Bytes
/// pero llega Text) → `Err` claro desde el método del conn (`recv()`).
///
/// Ping/Pong/Close los maneja la stack axum/tungstenite por debajo;
/// nunca se exponen acá.
#[derive(Debug, Clone)]
pub enum IncomingFrame {
    /// Text frame UTF-8. `recv()` con T ≠ Bytes lo parsea como JSON y
    /// coerce al T declarado.
    Text(String),
    /// Binary frame raw. `recv()` con T = Bytes lo expone como
    /// `Value::Bytes(...)`.
    Binary(Vec<u8>),
}

/// Trait del read half — abstracción para no leakear axum tipos.
/// Define solo lo que necesita `recv()`: leer un frame (text o binary)
/// o detectar close.
pub trait WsReadStreamTrait {
    /// Lee el próximo frame. Devuelve:
    ///   - `Ok(Some(IncomingFrame::Text(s)))` — text frame.
    ///   - `Ok(Some(IncomingFrame::Binary(bs)))` — binary frame.
    ///   - `Ok(None)` — close frame; el conn cerró ordenadamente.
    ///   - `Err(msg)` — error de transporte.
    ///
    /// Ping/Pong: el impl los maneja internamente (axum auto-replies;
    /// los descartamos en el loop interno).
    fn next_frame<'a>(
        &'a mut self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<IncomingFrame>, String>> + Send + 'a>,
    >;
}

/// Mensaje "outbox" — texto, bytes o señal de cierre. El writer task
/// del conn lo consume y lo traduce al frame axum correspondiente.
#[derive(Debug, Clone)]
pub enum WsOutMessage {
    /// Frame text con `payload` (JSON serialization del T cuando T ≠ Bytes).
    Text(String),
    /// 9.w.2-binary-frames — frame binario raw. Construido por
    /// `WsConn<Bytes>.send(...)` / `.broadcast(...)`. El writer task lo
    /// traduce a `axum::extract::ws::Message::Binary(...)`.
    Binary(Vec<u8>),
    /// Pedido de cierre. El writer task lo procesa y termina.
    Close,
    /// Fase 9.w.2.e — heartbeat ping. El writer task lo traduce a
    /// `axum::extract::ws::Message::Ping(...)`. Si el sink.send() falla,
    /// el writer task termina y `closed` se setea (lo cual el heartbeat
    /// task también detecta en su próxima iteración).
    Ping,
}

/// Trait del broadcaster — abstracción que `WsConnHandle.broadcaster`
/// implementa. Para evitar que `value.rs` dependa de `http.rs` (que
/// es donde vive el broadcaster concreto), exponemos los métodos
/// `broadcast_text` y `broadcast_binary` (9.w.2-binary-frames separó
/// los dos para mantener tipo y evitar un enum extra en la API).
///
/// El runtime construye un broadcaster compartido por `HttpRegistry`
/// (`Arc<WsBroadcaster>`), lo registra en cada `WsConnHandle`, y
/// `broadcast(msg)` del lado del usuario delega al método que
/// corresponda según el T del `WsConn`.
pub trait WsBroadcasterTrait {
    /// Envía `payload` (text frame) al outbox de TODOS los conns
    /// vivos en `endpoint`, incluyendo el conn que invocó (convención
    /// Socket.IO/Phoenix). Conns con outbox cerrado se ignoran
    /// silenciosamente (cleanup lazy).
    fn broadcast_text(&self, endpoint: &str, payload: String);
    /// 9.w.2-binary-frames — variante binaria. Mismo modelo "broadcast
    /// a todos del endpoint incluyendo sender" que `broadcast_text`.
    fn broadcast_binary(&self, endpoint: &str, payload: Vec<u8>);
}

impl Clone for FutureCell {
    fn clone(&self) -> Self {
        FutureCell(Arc::clone(&self.0))
    }
}

impl std::fmt::Debug for FutureCell {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let occupied = self.0.lock().is_some();
        if occupied {
            write!(f, "FutureCell(pending)")
        } else {
            write!(f, "FutureCell(consumed)")
        }
    }
}

/// Alias para colecciones compartidas por referencia. Las listas, los
/// mapas y los campos de una instancia viven detrás de
/// `Arc<parking_lot::Mutex<>>`: `Arc` permite alias (la misma colección
/// visible desde múltiples variables/campos/argumentos) y `Mutex` permite
/// mutar a través del alias. Es la misma semántica que objetos en Python
/// y JS pero ya thread-safe — habilitará paralelismo real entre tareas
/// Fitz una vez que F17.3 cierre y quitemos `(?Send)` del async_recursion.
///
/// `Value::clone()` clona el `Arc` (barato), no el contenido — todas las
/// copias miran el mismo dato. Eso es lo que destraba `xs.push(...)`,
/// `user.name = "x"` y demás formas de mutación.
///
/// **F17.2**: migrado de `Rc<RefCell<T>>` a `Arc<parking_lot::Mutex<T>>`.
/// `.borrow()` y `.borrow_mut()` se mapean ambos a `.lock()` —
/// parking_lot no distingue lectura de escritura (si en algún hot path
/// las lecturas concurrentes ganan el costo extra, evaluamos `RwLock`).
pub type Shared<T> = Arc<Mutex<T>>;

/// Constructor del wrapper compartido. Usar siempre `shared(x)` en lugar
/// de `Arc::new(Mutex::new(x))` directo, para que el patrón quede uniforme.
pub fn shared<T>(value: T) -> Shared<T> {
    Arc::new(Mutex::new(value))
}

/// Handle opaco a un objeto Python (módulo, función, instancia, etc.) —
/// solo existe cuando el binario `fitz` se compila con la feature
/// `python` (Fase 8.1+). Envuelve `Py<PyAny>` de PyO3 en un `Arc` para
/// que `Value::clone()` quede O(1) sin tomar el GIL: el `Arc` cuenta
/// las copias del handle a nivel Rust, y solo cuando el último handle
/// se dropea PyO3 toma el GIL para decrementar el refcount Python.
///
/// La igualdad es por identidad del objeto Python (`Py::as_ptr()`),
/// igual que para `Value::Module` y `Value::Function`. Dos handles
/// distintos al mismo módulo importado son iguales.
///
/// Debug manual (Py<PyAny> no implementa Debug) — produce
/// `PyObjectHandle(<python object>)` sin tocar Python.
#[cfg(feature = "python")]
pub struct PyObjectHandle(pub Arc<pyo3::Py<pyo3::PyAny>>);

#[cfg(feature = "python")]
impl Clone for PyObjectHandle {
    fn clone(&self) -> Self {
        PyObjectHandle(Arc::clone(&self.0))
    }
}

#[cfg(feature = "python")]
impl std::fmt::Debug for PyObjectHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "PyObjectHandle(<python object>)")
    }
}

#[cfg(feature = "python")]
impl PyObjectHandle {
    /// Construye un handle a partir de un `Py<PyAny>` ya adquirido
    /// (por ejemplo, el retorno de `PyModule::import` adentro de un
    /// `Python::with_gil`). El caller mantiene la responsabilidad de
    /// haber tomado el GIL para obtener el `Py<PyAny>` original; este
    /// constructor solo envuelve.
    ///
    /// `dead_code` allow: la variante `Value::PyObject` y este
    /// constructor aún no se usan en 8.1.1 (solo placeholder); el
    /// loader Python en `evaluator::load_module` los consume en 8.1.2.
    #[allow(dead_code)]
    pub fn new(obj: pyo3::Py<pyo3::PyAny>) -> Self {
        PyObjectHandle(Arc::new(obj))
    }
}

/// Un valor en runtime.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,

    /// Mini-tanda Bytes — secuencia de bytes binarios. Construido vía
    /// literal `b"..."` con escapes hex (`\xHH`) o vía builtin
    /// `bytes_from_str(s)`. Inmutable de hecho (no se expone `push`
    /// para mantener el modelo simple). Clone es O(n).
    Bytes(Vec<u8>),

    /// Mini-tanda Mw-Wrap — función nativa async construida por el
    /// runtime y pasada como Value al usuario. Hoy se usa solo para
    /// el `next` callable de los wrap-style middlewares: el chain
    /// runner construye un `NativeFn` que captura el resto de la
    /// chain + el handler y lo pasa al middleware, que decide cuándo
    /// invocarlo (antes/después del handler, condicionalmente,
    /// midiendo tiempo, etc.). Send + Sync para que pueda fluir a
    /// través de tokio runtimes.
    NativeFn(NativeAsyncFn),

    /// Función nativa implementada en Rust (ej: `print`).
    /// La firma recibe los args ya evaluados y devuelve un valor o error.
    Builtin {
        name: &'static str,
        func: fn(&[Value]) -> FitzResult<Value>,
    },

    /// Función definida por el usuario. Guarda sus parámetros, su cuerpo,
    /// y un handle al env donde fue definida. Ese handle es el "closure":
    /// al llamar la función creamos un scope hijo de ese env, no del caller.
    /// Eso le da acceso a las variables del lugar donde se definió.
    ///
    /// `is_async` (Fase 6.4): replica el flag del `Stmt::FnDef` original.
    /// `FnExpr` siempre lo marca como `false` (no se soportan async fn
    /// anónimas hoy). El dispatcher de llamadas lo consulta: si una fn
    /// async se llama sin `.await`, devuelve un `Value::Future` que
    /// envuelve la evaluación del body; con `.await` desempaca al T.
    Function {
        params: Vec<Param>,
        body: Vec<Stmt>,
        closure: EnvRef,
        is_async: bool,
    },

    /// Tipo custom definido por el usuario (`type User { id: Int }`).
    /// Por ahora es un marcador inerte: existe en el env para que el nombre
    /// del tipo pueda resolverse, pero sin struct literals no se puede
    /// instanciar. Se vuelve útil en Fase 3 (instanciación, field access).
    ///
    /// PreF8.3: `resolved_defaults` queda vacío para tipos definidos en el
    /// archivo actual (sus defaults se evalúan lazy cada vez que se
    /// instancia, con el env del call site). Para tipos importados desde
    /// otro módulo, el loader pre-evalúa los `Field.default` en el env de
    /// origen y los materializa acá. El struct lit prefiere
    /// `resolved_defaults` antes de caer al `Field.default` como Expr,
    /// para que un default importado pueda referenciar consts u otros
    /// símbolos del módulo de origen sin que el importer los tenga que
    /// re-importar.
    Type {
        name: String,
        fields: Vec<Field>,
        resolved_defaults: Vec<(String, Value)>,
        /// R.3 (mini-fase R) — métodos custom declarados en el `type`.
        /// El dispatch sobre `Value::Instance` busca primero por
        /// nombre acá; si no existe, cae a los métodos built-in.
        methods: Vec<crate::ast::MethodDef>,
    },

    /// Tupla en runtime (mini-tanda T). Heterogénea, tamaño fijo
    /// conocido en compile-time. NO compartida por referencia
    /// (semántica de valor — clonar la tupla clona cada slot). El
    /// orden es el de declaración; el acceso es por índice
    /// (`Expr::TupleField`) o por destructuring (Pattern::Tuple).
    Tuple(Vec<Value>),

    /// Lista en runtime. Compartida por referencia (`Shared<T>` =
    /// `Arc<Mutex<>>` post-F17.2) para que `xs.push(...)`, pasar la lista
    /// a una función, o guardarla en un campo de instancia hablen del
    /// mismo dato. Construir con `Value::new_list(vec)`.
    List(Shared<Vec<Value>>),

    /// Mapa en runtime. `Vec<(K, V)>` en vez de `HashMap` por dos razones:
    ///  - preserva el orden de inserción (importa para `print` y para
    ///    iteración futura).
    ///  - acepta claves no-hash sin complicar `Value`. Acceso es O(n);
    ///    optimizable más adelante cuando importe.
    ///
    /// Compartido por referencia, mismo criterio que `List`.
    Map(Shared<Vec<(Value, Value)>>),

    /// Rango exclusivo de Int. Iterable. Por ahora solo Int (Float
    /// no tiene una semántica discreta clara para iteración).
    Range {
        start: i64,
        end: i64,
    },

    /// Instancia de un tipo custom: el resultado de evaluar un struct
    /// literal `User { id: 1, name: "x" }`. Guarda el nombre del tipo
    /// (para `Display` y mensajes de error) y los pares `(campo,
    /// valor)` en orden de declaración del `type`.
    ///
    /// El orden es estable: el evaluador lo arma siguiendo la lista
    /// de campos del `Value::Type`, no la del literal. Eso garantiza
    /// que dos instancias del mismo tipo se imprimen igual aunque el
    /// usuario haya tipeado los campos en otro orden.
    ///
    /// `fields` va compartido (`Shared<T>` = `Arc<Mutex<>>` post-F17.2)
    /// para destrabar `user.name = "x"`: la mutación se ve a través de
    /// cualquier alias a esta instancia. Construir con
    /// `Value::new_instance(...)`.
    Instance {
        type_name: String,
        fields: Shared<Vec<(String, Value)>>,
    },

    /// Sum type built-in `Result`: representa el desenlace de una
    /// operación que pudo fallar. Variante exitosa o de error, cada
    /// una con un valor cualquiera adentro.
    ///
    /// Se modela con variante propia (no como `Instance`) porque
    /// `Result` es sum type, no product type: tiene alternativas, no
    /// campos. La reglas de Display, igualdad y matching tendrían que
    /// ser especiales si lo reusáramos sobre `Instance`; mejor un tipo
    /// dedicado.
    Result(ResultVariant),

    /// Módulo cargado desde otro archivo. Resultado de un `import` que
    /// expone el módulo entero como namespace: `import utils` bindea
    /// un `Value::Module` bajo el nombre `utils`, y `utils.foo()`
    /// resuelve `foo` en el env del módulo.
    ///
    /// `name` es el último segmento del path original (`import sub.foo`
    /// → `name = "foo"`), útil para Display y mensajes de error.
    ///
    /// `env` es el environment donde se evaluó el body del módulo. El
    /// loader lo congela ahí: las top-level definitions (let, fn, type)
    /// del archivo viven en ese env y son visibles vía field access.
    /// La igualdad es por identidad del `Rc` (dos `Value::Module` son
    /// iguales si comparten el mismo env — sirve para detectar que dos
    /// imports del mismo archivo dieron el mismo módulo).
    Module {
        name: String,
        env: EnvRef,
    },

    /// Response HTTP con status code custom. Solo aparece como
    /// producto de un `return <Int> { ... }` adentro de un handler;
    /// el runtime HTTP (en `http.rs`) lo intercepta en
    /// `value_to_outcome` para emitir el `HandlerOutcome` con el
    /// status y body que pidió el usuario. Fuera de context HTTP
    /// es opaco — no se puede serializar a JSON ni se imprime, y
    /// el checker rechaza `Stmt::ReturnStatus` fuera de handlers.
    ///
    /// Sin variante `Pair`: el body queda como `Box<Value>` para
    /// reusar el camino existente de serialización. `body = None`
    /// se reserva para 204 No Content (hoy el parser exige body;
    /// el campo es opcional para preparar esa extensión).
    HttpResponse {
        status: u16,
        body: Option<Box<Value>>,
    },

    /// Configuración CORS opaca, producto del built-in `cors(...)`
    /// (mini-fase MW.2). Se usa como argumento de `@middleware(cors(...))`
    /// sobre un handler HTTP; el evaluador la detecta y la guarda en el
    /// slot `RouteSpec.cors` (no entra a la chain de middlewares user-fn).
    /// Fuera de ese context es opaca: no se puede imprimir ni
    /// serializar — usar `cors(...)` como expresión suelta no tiene
    /// sentido y el código que lo intenta recibe error claro.
    ///
    /// `Arc<CorsConfig>` (inmutable post-build): el config viaja al thread
    /// tokio para configurar el preflight handler de axum, así que tiene
    /// que ser `Send + Sync`. El payload (`String`s y `Vec<String>`) ya
    /// cumple eso. Post-F17 el resto de `Shared<T>` también es `Arc<Mutex>`
    /// — este caso sigue sin `Mutex` porque el config es read-only.
    CorsConfig(Arc<crate::http::CorsConfig>),

    /// Future pendiente introducido en Fase 6.4. Se construye cuando
    /// se llama una `async fn` Fitz sin `.await` (guardar el future
    /// suelto en una variable) o desde builtins async (`sleep`).
    /// `Expr::Await` lo desempaca; consumirlo dos veces es un panic
    /// del intérprete (futures se await-ean una sola vez).
    ///
    /// Envuelto en `FutureCell` (`Arc<Mutex<Option<...>>>`) por dos
    /// razones: (a) `Value: Clone` y los `Pin<Box<dyn Future>>` no
    /// son Clone — el `Arc` da clone barato y comparte la celda;
    /// (b) el `Option` permite extraer el future al hacer `.await`
    /// sin clonar (mover con `.take()`), preservando la regla
    /// "un future se await una sola vez".
    Future(FutureCell),

    /// Objeto Python opaco — solo existe con la feature `python`.
    /// Producido por el loader `from python import <mod>` (Fase 8.1.2)
    /// y por accesos a atributos / llamadas que devuelven objetos
    /// no-primitivos (8.1.3+). En 8.1 los primitivos (Int/Float/Str/
    /// Bool/None) se auto-coercionan a `Value` nativos en el cruce,
    /// así que `Value::PyObject` envuelve módulos, funciones, clases,
    /// instancias y demás callables/contenedores opacos. El
    /// marshaling de tipos compuestos (List/Map/Instance) llega en 8.2.
    ///
    /// `dead_code` allow: en 8.1.1 la variante existe como placeholder
    /// (Display/PartialEq/type_name preparados); el constructor real
    /// llega en 8.1.2 cuando `evaluator::load_module` rutea a Python.
    #[cfg(feature = "python")]
    #[allow(dead_code)]
    PyObject(PyObjectHandle),

    /// Fase 9.w.2 — Conexión WebSocket abierta. El runtime HTTP la
    /// construye tras el upgrade HTTP→WS y la inyecta como argumento
    /// del handler `@ws("/path")`. Opaco para el usuario: solo se
    /// accede vía los 4 métodos paramétricos del checker (`recv`/
    /// `send`/`broadcast`/`close`).
    ///
    /// Igualdad por identidad del `Arc` — dos referencias al mismo
    /// conn comparten state; conns distintos son siempre distintos.
    /// No serializable a JSON (ver `value_to_json` en `http.rs` —
    /// rechaza con mensaje claro). Display: `<ws-conn>`.
    WsConn(Arc<WsConnHandle>),

    /// Fase 10.1.b — Conexión Postgres abierta. Producida por el
    /// builtin `db.connect(url).await` y consumida vía los métodos
    /// `query/exec/close` despachados por `dispatch_method`. Opaco
    /// para el usuario: igual que WsConn, solo se accede vía métodos
    /// específicos del driver (`src/db.rs`).
    ///
    /// Igualdad por identidad del `Arc` — paralelo a WsConn. No
    /// serializable a JSON (es un handle a un recurso del sistema,
    /// no un valor). Display: `<db-conn user@host/db>` con el URL
    /// redacted (sin password).
    DbConn(Arc<crate::db::DbConnHandle>),
}

/// Variante de `Value::Result`. Usa `Box<Value>` para evitar enum
/// recursivo de tamaño infinito (mismo truco que `Box<Expr>` en el AST).
#[derive(Debug, Clone)]
pub enum ResultVariant {
    Ok(Box<Value>),
    Err(Box<Value>),
}

impl Value {
    /// Crea un `Value::List` a partir de un `Vec<Value>`. Envolvé siempre
    /// con este constructor para mantener el wrapping `Shared<T>` =
    /// `Arc<Mutex<>>` (post-F17.2) uniforme y no esparcir
    /// `Arc::new(Mutex::new(...))` por todos lados.
    pub fn new_list(items: Vec<Value>) -> Value {
        Value::List(shared(items))
    }

    /// Crea un `Value::Map` a partir de un `Vec<(Value, Value)>`.
    pub fn new_map(pairs: Vec<(Value, Value)>) -> Value {
        Value::Map(shared(pairs))
    }

    /// Crea un `Value::Instance` a partir del nombre del tipo y los
    /// pares `(campo, valor)`. El orden importa: el evaluador lo arma
    /// siguiendo la declaración del `type`.
    pub fn new_instance(type_name: String, fields: Vec<(String, Value)>) -> Value {
        Value::Instance {
            type_name,
            fields: shared(fields),
        }
    }

    /// Crea un `Value::Future` envolviendo un future Rust nativo.
    /// Usado por `builtin_sleep` y por el dispatcher de async fn Fitz
    /// al llamar sin `.await`. El future se ejecuta una sola vez:
    /// cuando `Expr::Await` lo desempaca, el `Option` queda en `None`
    /// y un segundo `.await` paniquea.
    pub fn new_future(fut: FitzFuture) -> Value {
        Value::Future(FutureCell(Arc::new(Mutex::new(Some(fut)))))
    }

    /// Nombre del tipo, para mensajes de error.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Str(_) => "Str",
            Value::Bool(_) => "Bool",
            Value::Null => "Null",
            Value::Bytes(_) => "Bytes",
            Value::NativeFn(_) => "Function",
            Value::Builtin { .. } => "Function",
            Value::Function { .. } => "Function",
            Value::Type { .. } => "Type",
            Value::List(_) => "List",
            Value::Map(_) => "Map",
            Value::Tuple(_) => "Tuple",
            Value::Range { .. } => "Range",
            Value::Instance { .. } => "Instance",
            Value::Result(_) => "Result",
            Value::HttpResponse { .. } => "HttpResponse",
            Value::Module { .. } => "Module",
            Value::CorsConfig(_) => "CorsConfig",
            Value::Future(_) => "Future",
            Value::WsConn(_) => "WsConn",
            Value::DbConn(_) => "DbConn",
            #[cfg(feature = "python")]
            Value::PyObject(_) => "PyObject",
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(x) => {
                // Si no tiene parte decimal, agregamos `.0` para que se vea
                // distinto a un Int. `3.0` → "3.0", `3.14` → "3.14".
                if x.fract() == 0.0 && x.is_finite() {
                    write!(f, "{:.1}", x)
                } else {
                    write!(f, "{}", x)
                }
            }
            Value::Str(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Null => write!(f, "null"),
            Value::Bytes(bs) => {
                // Mini-tanda Bytes — formato `b"..."` paralelo a
                // Rust. ASCII printable + escapes comunes (`\n`, `\r`,
                // `\t`, `\\`, `\"`) salen tal cual; el resto va como
                // `\xHH`. Mismo criterio que Rust's
                // `<[u8] as Debug>` para el contenido entre comillas.
                write!(f, "b\"")?;
                for &b in bs.iter() {
                    match b {
                        b'\\' => write!(f, "\\\\")?,
                        b'"' => write!(f, "\\\"")?,
                        b'\n' => write!(f, "\\n")?,
                        b'\r' => write!(f, "\\r")?,
                        b'\t' => write!(f, "\\t")?,
                        0x20..=0x7e => write!(f, "{}", b as char)?,
                        _ => write!(f, "\\x{:02x}", b)?,
                    }
                }
                write!(f, "\"")
            }
            Value::Builtin { name, .. } => write!(f, "<builtin {}>", name),
            Value::Function { .. } => write!(f, "<function>"),
            Value::Type { name, .. } => write!(f, "<type {}>", name),
            Value::List(items) => {
                // Para strings, mostramos comillas adentro de la lista
                // (es la representación, no salida directa de `print`).
                // Ej: `[1, "hola", 2]`. Distinto del Display de `Str`
                // suelto, que va sin comillas porque ese caso es para
                // salida final.
                let items = items.lock();
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write_inline_value(f, v)?;
                }
                write!(f, "]")
            }
            Value::Map(pairs) => {
                let pairs = pairs.lock();
                write!(f, "{{")?;
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write_inline_value(f, k)?;
                    write!(f, ": ")?;
                    write_inline_value(f, v)?;
                }
                write!(f, "}}")
            }
            Value::Tuple(items) => {
                // Tupla: `(1, "x", true)`. Strings con comillas adentro
                // (mismo criterio que List/Map/Instance). Single-element
                // tuple lleva trailing comma: `(42,)` para distinguir
                // de `(42)` (paréntesis de agrupación).
                write!(f, "(")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write_inline_value(f, v)?;
                }
                if items.len() == 1 {
                    write!(f, ",")?;
                }
                write!(f, ")")
            }
            Value::Range { start, end } => write!(f, "{}..{}", start, end),
            Value::Instance { type_name, fields } => {
                // Formato: `User { id: 1, name: "x" }`. Strings con
                // comillas adentro (mismo criterio que List/Map), para
                // distinguir `42` de `"42"` a simple vista.
                let fields = fields.lock();
                write!(f, "{} {{", type_name)?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, " {}: ", k)?;
                    write_inline_value(f, v)?;
                }
                if !fields.is_empty() {
                    write!(f, " ")?;
                }
                write!(f, "}}")
            }
            Value::Result(ResultVariant::Ok(inner)) => {
                write!(f, "Ok(")?;
                write_inline_value(f, inner)?;
                write!(f, ")")
            }
            Value::Result(ResultVariant::Err(inner)) => {
                write!(f, "Err(")?;
                write_inline_value(f, inner)?;
                write!(f, ")")
            }
            Value::Module { name, .. } => write!(f, "<module {}>", name),
            Value::HttpResponse { status, body } => match body {
                Some(b) => {
                    write!(f, "<response {} ", status)?;
                    write_inline_value(f, b)?;
                    write!(f, ">")
                }
                None => write!(f, "<response {}>", status),
            },
            Value::CorsConfig(_) => write!(f, "<cors-config>"),
            Value::Future(_) => write!(f, "<future>"),
            Value::WsConn(_) => write!(f, "<ws-conn>"),
            Value::DbConn(h) => write!(f, "<db-conn {}>", h.url_redacted),
            Value::NativeFn(_) => write!(f, "<native function>"),
            #[cfg(feature = "python")]
            Value::PyObject(_) => write!(f, "<python object>"),
        }
    }
}

/// Representación "inline" de un Value cuando aparece adentro de otro
/// (lista, mapa). Los strings llevan comillas para distinguir
/// `[1, "2"]` de `[1, 2]` — es lectura, no impresión final. El resto
/// delega en `Display` normal.
fn write_inline_value(f: &mut std::fmt::Formatter, v: &Value) -> std::fmt::Result {
    match v {
        Value::Str(s) => write!(f, "\"{}\"", s),
        other => write!(f, "{}", other),
    }
}

/// Igualdad con coerción Int↔Float. El resto de combinaciones devuelven false.
/// Esto define la semántica de `==` en Fitz (la usa el evaluador en BinOp::Eq).
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Null, Value::Null) => true,
            // Bytes: comparación byte a byte.
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            // List y Map se comparan estructuralmente, elemento a elemento.
            // La igualdad recursiva delega en esta misma impl, así que Int↔Float
            // coerciona también adentro de listas y mapas. Si los dos `Arc`
            // apuntan al mismo dato (alias del mismo origen), `Arc::ptr_eq`
            // es shortcut barato; si no, comparamos el contenido lockeando.
            (Value::List(a), Value::List(b)) => Arc::ptr_eq(a, b) || *a.lock() == *b.lock(),
            (Value::Map(a), Value::Map(b)) => Arc::ptr_eq(a, b) || *a.lock() == *b.lock(),
            // Tuples (mini-tanda T): comparación estructural por
            // longitud y elementos. La coerción Int↔Float vale
            // recursivamente vía esta misma impl.
            (Value::Tuple(a), Value::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
            }
            (Value::Range { start: s1, end: e1 }, Value::Range { start: s2, end: e2 }) => {
                s1 == s2 && e1 == e2
            }
            // Instancias se comparan estructuralmente: mismo tipo y mismo
            // contenido de campos (con el mismo orden, que está garantizado
            // por el evaluador porque sigue la declaración del `type`).
            // La coerción Int↔Float vale recursivamente vía esta misma impl.
            (
                Value::Instance {
                    type_name: t1,
                    fields: f1,
                },
                Value::Instance {
                    type_name: t2,
                    fields: f2,
                },
            ) => t1 == t2 && (Arc::ptr_eq(f1, f2) || *f1.lock() == *f2.lock()),
            // Result se compara variante por variante, recursivamente.
            // Misma coerción Int↔Float adentro vía esta misma impl.
            (Value::Result(a), Value::Result(b)) => match (a, b) {
                (ResultVariant::Ok(va), ResultVariant::Ok(vb)) => va == vb,
                (ResultVariant::Err(va), ResultVariant::Err(vb)) => va == vb,
                _ => false,
            },
            // Módulos se comparan por identidad del env (mismo Arc). El
            // loader cachea por path canonicalizado, así que dos
            // imports del mismo archivo dan dos `Value::Module` con el
            // mismo `Arc<Mutex<Environment>>`. Estructural no tiene
            // sentido — el env puede contener funciones y otros
            // valores no-comparables.
            (Value::Module { env: e1, .. }, Value::Module { env: e2, .. }) => Arc::ptr_eq(e1, e2),
            // PyObject se compara por identidad del objeto Python
            // (`Py::as_ptr()` da el `*mut PyObject` subyacente, que es
            // único por objeto vivo). Dos handles a `math` importado dos
            // veces son iguales — Python cachea los imports igual que
            // nuestro `Value::Module` cachea por path canonicalizado.
            // No hace falta tomar el GIL para leer el puntero.
            #[cfg(feature = "python")]
            (Value::PyObject(a), Value::PyObject(b)) => a.0.as_ptr() == b.0.as_ptr(),
            // WsConn — igualdad por identidad del Arc. Dos handles al
            // mismo conn comparten state; conns distintos jamás son
            // iguales estructuralmente (sockets distintos, broadcaster
            // entries distintas).
            (Value::WsConn(a), Value::WsConn(b)) => Arc::ptr_eq(a, b),
            // Igualdad por identidad del Arc — paralelo a WsConn.
            (Value::DbConn(a), Value::DbConn(b)) => Arc::ptr_eq(a, b),
            // Funciones no se comparan por valor — siempre desiguales.
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 en tests es un Float genérico, no PI.
mod tests {
    use super::*;

    #[test]
    fn display_int_sin_decimales() {
        assert_eq!(Value::Int(42).to_string(), "42");
        assert_eq!(Value::Int(-7).to_string(), "-7");
    }

    #[test]
    fn display_float_entero_lleva_punto_cero() {
        assert_eq!(Value::Float(3.0).to_string(), "3.0");
        assert_eq!(Value::Float(-0.0).to_string(), "-0.0");
    }

    #[test]
    fn display_float_con_decimales_se_muestra_normal() {
        assert_eq!(Value::Float(3.14).to_string(), "3.14");
    }

    #[test]
    fn display_str_sin_comillas() {
        // print("hola") debe mostrar `hola`, no `"hola"`.
        assert_eq!(Value::Str("hola".into()).to_string(), "hola");
    }

    #[test]
    fn display_bool_lowercase() {
        assert_eq!(Value::Bool(true).to_string(), "true");
        assert_eq!(Value::Bool(false).to_string(), "false");
    }

    #[test]
    fn display_null() {
        assert_eq!(Value::Null.to_string(), "null");
    }

    #[test]
    fn type_name_devuelve_el_nombre_del_tipo() {
        assert_eq!(Value::Int(0).type_name(), "Int");
        assert_eq!(Value::Float(0.0).type_name(), "Float");
        assert_eq!(Value::Str("".into()).type_name(), "Str");
        assert_eq!(Value::Bool(false).type_name(), "Bool");
        assert_eq!(Value::Null.type_name(), "Null");
        assert_eq!(Value::Bytes(vec![]).type_name(), "Bytes");
    }

    // ---- Mini-tanda Bytes ----

    #[test]
    fn bytes_display_ascii_printable() {
        assert_eq!(Value::Bytes(b"hola".to_vec()).to_string(), "b\"hola\"");
    }

    #[test]
    fn bytes_display_con_escapes_hex() {
        assert_eq!(
            Value::Bytes(vec![0x00, 0x01, 0xff]).to_string(),
            "b\"\\x00\\x01\\xff\""
        );
    }

    #[test]
    fn bytes_display_con_escapes_comunes() {
        assert_eq!(
            Value::Bytes(b"a\nb\tc\\d\"e".to_vec()).to_string(),
            "b\"a\\nb\\tc\\\\d\\\"e\""
        );
    }

    #[test]
    fn bytes_igualdad_byte_a_byte() {
        assert_eq!(Value::Bytes(vec![1, 2, 3]), Value::Bytes(vec![1, 2, 3]));
        assert_ne!(Value::Bytes(vec![1, 2, 3]), Value::Bytes(vec![1, 2, 4]));
        assert_ne!(Value::Bytes(vec![1, 2]), Value::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn bytes_distinto_de_str_aunque_mismo_contenido() {
        // Bytes("hola") y Str("hola") son tipos distintos — PartialEq
        // devuelve false (paralelo a Int vs Str).
        assert_ne!(Value::Bytes(b"hola".to_vec()), Value::Str("hola".into()));
    }

    #[test]
    fn igualdad_int_y_float_se_coerciona() {
        // En Fitz, `1 == 1.0` es true. Esto refleja la promoción Int↔Float.
        assert_eq!(Value::Int(1), Value::Float(1.0));
        assert_eq!(Value::Float(2.0), Value::Int(2));
    }

    #[test]
    fn igualdad_entre_tipos_distintos_es_false() {
        assert_ne!(Value::Int(1), Value::Str("1".into()));
        assert_ne!(Value::Bool(true), Value::Int(1));
        assert_ne!(Value::Null, Value::Bool(false));
    }

    #[test]
    fn igualdad_null_consigo_mismo() {
        assert_eq!(Value::Null, Value::Null);
    }

    #[test]
    fn igualdad_strings() {
        assert_eq!(Value::Str("hola".into()), Value::Str("hola".into()));
        assert_ne!(Value::Str("hola".into()), Value::Str("chau".into()));
    }

    // -----------------------------------------------------------------------
    // Tests — List, Map, Range (Fase 3, paso 1)
    // -----------------------------------------------------------------------

    #[test]
    fn display_list_vacia() {
        assert_eq!(Value::new_list(vec![]).to_string(), "[]");
    }

    #[test]
    fn display_list_con_ints() {
        let v = Value::new_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        assert_eq!(v.to_string(), "[1, 2, 3]");
    }

    #[test]
    fn display_list_strings_van_con_comillas_dentro() {
        // Strings sueltos van sin comillas (print), pero adentro de
        // una lista llevan comillas para que se distinga `1` de `"1"`.
        let v = Value::new_list(vec![
            Value::Int(1),
            Value::Str("hola".into()),
            Value::Bool(true),
        ]);
        assert_eq!(v.to_string(), "[1, \"hola\", true]");
    }

    #[test]
    fn display_list_anidada() {
        let inner = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let outer = Value::new_list(vec![inner.clone(), inner]);
        assert_eq!(outer.to_string(), "[[1, 2], [1, 2]]");
    }

    #[test]
    fn display_map_vacio() {
        assert_eq!(Value::new_map(vec![]).to_string(), "{}");
    }

    #[test]
    fn display_map_preserva_orden_y_comillas_en_strings() {
        let m = Value::new_map(vec![
            (Value::Str("a".into()), Value::Int(1)),
            (Value::Str("b".into()), Value::Int(2)),
        ]);
        assert_eq!(m.to_string(), "{\"a\": 1, \"b\": 2}");
    }

    #[test]
    fn display_range_simple() {
        assert_eq!(Value::Range { start: 0, end: 10 }.to_string(), "0..10");
    }

    #[test]
    fn display_range_negativo() {
        assert_eq!(Value::Range { start: -5, end: 5 }.to_string(), "-5..5");
    }

    #[test]
    fn type_name_de_list_map_range() {
        assert_eq!(Value::new_list(vec![]).type_name(), "List");
        assert_eq!(Value::new_map(vec![]).type_name(), "Map");
        assert_eq!(Value::Range { start: 0, end: 1 }.type_name(), "Range");
    }

    #[test]
    fn igualdad_list_estructural() {
        let a = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let c = Value::new_list(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn igualdad_list_coerciona_int_float_adentro() {
        // [1, 2] == [1.0, 2.0] — la coerción Int↔Float vale adentro de listas.
        let a = Value::new_list(vec![Value::Int(1), Value::Int(2)]);
        let b = Value::new_list(vec![Value::Float(1.0), Value::Float(2.0)]);
        assert_eq!(a, b);
    }

    #[test]
    fn igualdad_map_estructural() {
        let a = Value::new_map(vec![(Value::Str("k".into()), Value::Int(1))]);
        let b = Value::new_map(vec![(Value::Str("k".into()), Value::Int(1))]);
        let c = Value::new_map(vec![(Value::Str("k".into()), Value::Int(2))]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn igualdad_map_sensible_al_orden() {
        // Como usamos Vec<(K,V)>, orden importa para igualdad. Esto es
        // consistente con cómo lo imprimimos (preservando orden).
        let a = Value::new_map(vec![
            (Value::Str("a".into()), Value::Int(1)),
            (Value::Str("b".into()), Value::Int(2)),
        ]);
        let b = Value::new_map(vec![
            (Value::Str("b".into()), Value::Int(2)),
            (Value::Str("a".into()), Value::Int(1)),
        ]);
        assert_ne!(a, b);
    }

    #[test]
    fn igualdad_range() {
        assert_eq!(
            Value::Range { start: 0, end: 10 },
            Value::Range { start: 0, end: 10 },
        );
        assert_ne!(
            Value::Range { start: 0, end: 10 },
            Value::Range { start: 0, end: 11 },
        );
    }

    #[test]
    fn igualdad_entre_tipos_distintos_es_false_para_nuevos() {
        // Sanity: list != map, list != range, etc.
        assert_ne!(Value::new_list(vec![]), Value::new_map(vec![]));
        assert_ne!(
            Value::new_list(vec![Value::Int(0), Value::Int(1)]),
            Value::Range { start: 0, end: 2 },
        );
    }

    // -----------------------------------------------------------------------
    // Tests — Instance (Fase 3, paso 2: tipos custom instanciables)
    // -----------------------------------------------------------------------

    #[test]
    fn type_name_de_instance() {
        let i = Value::new_instance("User".into(), vec![]);
        assert_eq!(i.type_name(), "Instance");
    }

    #[test]
    fn display_instance_vacia_muestra_llaves_juntas() {
        let i = Value::new_instance("Empty".into(), vec![]);
        assert_eq!(i.to_string(), "Empty {}");
    }

    #[test]
    fn display_instance_con_campos() {
        let i = Value::new_instance(
            "User".into(),
            vec![
                ("id".into(), Value::Int(1)),
                ("name".into(), Value::Str("Fitz".into())),
            ],
        );
        // Strings llevan comillas adentro, igual que en List/Map.
        assert_eq!(i.to_string(), "User { id: 1, name: \"Fitz\" }");
    }

    #[test]
    fn igualdad_instance_estructural() {
        let a = Value::new_instance(
            "Point".into(),
            vec![("x".into(), Value::Int(1)), ("y".into(), Value::Int(2))],
        );
        let b = Value::new_instance(
            "Point".into(),
            vec![("x".into(), Value::Int(1)), ("y".into(), Value::Int(2))],
        );
        let c = Value::new_instance(
            "Point".into(),
            vec![("x".into(), Value::Int(1)), ("y".into(), Value::Int(3))],
        );
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn igualdad_instance_distinto_type_name_es_false() {
        // Misma forma de campos, distinto tipo → no son iguales.
        let a = Value::new_instance("User".into(), vec![("id".into(), Value::Int(1))]);
        let b = Value::new_instance("Admin".into(), vec![("id".into(), Value::Int(1))]);
        assert_ne!(a, b);
    }

    // -----------------------------------------------------------------------
    // Tests — Result (Fase 3, paso 3: Result + Ok/Err + `?`)
    // -----------------------------------------------------------------------

    fn ok(v: Value) -> Value {
        Value::Result(ResultVariant::Ok(Box::new(v)))
    }

    fn err(v: Value) -> Value {
        Value::Result(ResultVariant::Err(Box::new(v)))
    }

    #[test]
    fn type_name_de_result() {
        assert_eq!(ok(Value::Int(1)).type_name(), "Result");
        assert_eq!(err(Value::Str("boom".into())).type_name(), "Result");
    }

    #[test]
    fn display_ok_envuelve_inner() {
        // Mismo criterio que List/Map: strings adentro llevan comillas.
        assert_eq!(ok(Value::Int(42)).to_string(), "Ok(42)");
        assert_eq!(ok(Value::Str("hola".into())).to_string(), "Ok(\"hola\")");
    }

    #[test]
    fn display_err_envuelve_inner() {
        assert_eq!(err(Value::Str("boom".into())).to_string(), "Err(\"boom\")");
        assert_eq!(err(Value::Int(404)).to_string(), "Err(404)");
    }

    #[test]
    fn display_result_anidado() {
        // Ok(Err("x")) — improbable pero legal estructuralmente.
        let inner = err(Value::Str("x".into()));
        assert_eq!(ok(inner).to_string(), "Ok(Err(\"x\"))");
    }

    #[test]
    fn igualdad_ok_estructural() {
        assert_eq!(ok(Value::Int(1)), ok(Value::Int(1)));
        assert_ne!(ok(Value::Int(1)), ok(Value::Int(2)));
    }

    #[test]
    fn igualdad_err_estructural() {
        assert_eq!(err(Value::Str("x".into())), err(Value::Str("x".into())));
        assert_ne!(err(Value::Str("x".into())), err(Value::Str("y".into())));
    }

    #[test]
    fn igualdad_ok_vs_err_es_false() {
        assert_ne!(ok(Value::Int(1)), err(Value::Int(1)));
    }

    #[test]
    fn igualdad_result_coerciona_int_float_adentro() {
        // La coerción Int↔Float vale recursivamente dentro del inner.
        assert_eq!(ok(Value::Int(1)), ok(Value::Float(1.0)));
    }

    #[test]
    fn igualdad_result_con_otros_tipos_es_false() {
        assert_ne!(ok(Value::Int(1)), Value::Int(1));
        assert_ne!(err(Value::Str("x".into())), Value::Str("x".into()));
    }

    // -----------------------------------------------------------------------
    // Tests — Module (Fase 3, paso 5: módulos / import)
    // -----------------------------------------------------------------------

    use crate::env::Environment;

    #[test]
    fn type_name_de_module() {
        let env = Environment::new();
        let m = Value::Module {
            name: "utils".into(),
            env,
        };
        assert_eq!(m.type_name(), "Module");
    }

    #[test]
    fn display_module_muestra_nombre() {
        let env = Environment::new();
        let m = Value::Module {
            name: "utils".into(),
            env,
        };
        assert_eq!(m.to_string(), "<module utils>");
    }

    #[test]
    fn igualdad_module_es_por_identidad_del_env() {
        // Mismo env → iguales. Esto modela "el mismo archivo importado
        // dos veces es el mismo módulo".
        let env = Environment::new();
        let m1 = Value::Module {
            name: "utils".into(),
            env: env.clone(),
        };
        let m2 = Value::Module {
            name: "utils".into(),
            env: env.clone(),
        };
        assert_eq!(m1, m2);

        // Distinto env → desiguales aunque el nombre coincida.
        let other_env = Environment::new();
        let m3 = Value::Module {
            name: "utils".into(),
            env: other_env,
        };
        assert_ne!(m1, m3);
    }
}
