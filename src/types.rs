// types.rs — Fase 5.2
//
// Representación interna del sistema de tipos de Fitz. Mientras
// `ast::TypeExpr` es lo que el parser produce a partir del fuente,
// este módulo modela el tipo *resuelto* contra una tabla: cada
// nombre se busca, cada genérico valida aridad, cada nominal lleva
// identidad única dentro del programa.
//
// El flujo es:
//
//   AST (TypeExpr)  ──resolve_type_expr──►  Type  (resuelto)
//                          contra
//                       TypeEnv
//
// 5.2 valida las anotaciones top-level (campos de `type`, params y
// return de fns, anotaciones de let). El chequeo de cuerpos de
// funciones contra valores queda para 5.3.

use std::collections::HashMap;

use crate::ast::{Expr, Program, Span, Stmt, TypeExpr};
use crate::error::{ErrorKind, FitzError};

/// Identidad única para los tipos nominales (los declarados con
/// `type`). Internamente es un índice contra `TypeEnv.nominals`.
/// Dos `type User` en módulos distintos producen `TypeId`s distintos
/// — la identidad es nominal, no estructural.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);

/// Un tipo resuelto. Lo que el checker compara y muestra al usuario.
///
/// Diferencias con `TypeExpr`:
///  - `Nominal(TypeId)` lleva la identidad ya resuelta (no es solo
///    un string).
///  - Los genéricos built-in tienen variantes propias en lugar de
///    `Generic { name, args }` — facilita el pattern matching.
///  - Los primitivos son singletons (no llevan datos).
///
/// La igualdad estructural derivada sirve: dos `Type` que el checker
/// dice "compatible" deben dar `==`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Str,
    Bool,
    Null,
    /// `Range` solo aparece en `0..10` por ahora — no tiene parámetro.
    Range,

    /// `List<T>`.
    List(Box<Type>),
    /// `Map<K, V>`.
    Map(Box<Type>, Box<Type>),
    /// `Result<T>` o `Result<T, E>` (mini-tanda Re+). Cuando el usuario
    /// escribe `Result<T>` sin E explícito, el parser lo expande a
    /// `Result<T, Str>` por compatibilidad con todo el código que existía
    /// antes del refactor. Anotar `Result<T, MiError>` permite carry de
    /// tipos custom en el Err side.
    Result {
        ok: Box<Type>,
        err: Box<Type>,
    },

    /// `Future<T>` — el valor pendiente que produce una `async fn` al
    /// llamarse. Solo `.await` (adentro de otra `async fn`) lo desempaca
    /// a `T`. Aridad fija 1 (built-in genérico, paralelo a Result/List/
    /// Nullable). Introducido en Fase 6.2.
    Future(Box<Type>),

    /// Tipo declarado por el usuario (`type User { ... }`) o
    /// importado. La identidad va por `TypeId`.
    Nominal(TypeId),

    /// `T?` — el valor puede ser de tipo `T` o `Null`.
    Nullable(Box<Type>),

    /// Tipo de una función: `fn(p1, p2, ...) -> r`. Lo construye el
    /// checker al registrar `Stmt::FnDef` (5.3.2) y al sintetizar
    /// `Expr::FnExpr` (5.3.5). En 5.3.1 ya existe como variante para
    /// no refactorizar después.
    Function {
        params: Vec<Type>,
        ret: Box<Type>,
    },

    /// Tipo tupla `(T1, T2, ...)` (mini-tanda T). Heterogénea, tamaño
    /// fijo, posicional. Vec vacío → tupla unitaria `()`. Acceso por
    /// `t.0`, `t.1`, etc. La identidad estructural: dos `Tuple` con
    /// los mismos elementos en el mismo orden son iguales.
    Tuple(Vec<Type>),

    /// "Sin tipo determinado". Escape gradual: aparece donde el
    /// checker no puede o no quiere inferir un tipo concreto. Param
    /// sin anotación, `let` sin anotación con RHS no inferible,
    /// expresiones que el checker todavía no modela (calls antes de
    /// 5.3.2, métodos antes de 5.3.4, etc.). Cualquier comparación
    /// contra `Any` pasa: nada se rechaza por culpa de un `Any`.
    Any,

    /// Fase 8.4 — "Objeto Python opaco". Aparece en los bindings de
    /// `from python import X` y se propaga por field access
    /// (`mod.submod`, `obj.attr` → siguen siendo `PyAny`). Existe
    /// separado de `Any` para que el checker pueda distinguir "esto
    /// es Python opaco" de "esto es Any general" y refinar el tipo
    /// de las llamadas: `pyobj(args)` y `pyobj.method(args)` tipan
    /// como `Result<Any>` (el wrap automático de 8.3), forzando al
    /// usuario a manejar el error con `match` o `?` estáticamente.
    ///
    /// Compatibilidad: como `Any`, `PyAny` es bidireccionalmente
    /// compatible con cualquier otro tipo (gradual escape).
    /// Anotaciones explícitas (`let row: User = py_call(...)?`) son
    /// la vía recomendada para "salir" de PyAny y entrar a tipos
    /// Fitz concretos — el runtime hace la coerción real (deuda 8.4.3:
    /// dict → Instance vía field name match).
    PyAny,
}

impl Type {
    /// `true` si el tipo es `T?` a nivel top.
    pub fn is_nullable(&self) -> bool {
        matches!(self, Type::Nullable(_))
    }

    /// Devuelve `&Type` pelando una sola capa de `Nullable`. `Int? →
    /// Int`. `Int → Int`. No baja recursivamente.
    pub fn base(&self) -> &Type {
        match self {
            Type::Nullable(t) => t,
            other => other,
        }
    }

    /// Reproduce el tipo para mensajes al usuario. Necesita el env
    /// para resolver los nombres de los `Nominal`.
    pub fn display(&self, env: &TypeEnv) -> String {
        match self {
            Type::Int => "Int".into(),
            Type::Float => "Float".into(),
            Type::Str => "Str".into(),
            Type::Bool => "Bool".into(),
            Type::Null => "Null".into(),
            Type::Range => "Range".into(),
            Type::List(t) => format!("List<{}>", t.display(env)),
            Type::Map(k, v) => format!("Map<{}, {}>", k.display(env), v.display(env)),
            // Mini-tanda Re+ — Display omite el E cuando es Str
            // (default, compat con escritura `Result<T>`) o cuando es
            // Any. Para E concreto distinto (Int/Instance/etc.),
            // muestra la forma completa `Result<T, E>`.
            Type::Result { ok: t, err: e } => match e.as_ref() {
                Type::Str | Type::Any => format!("Result<{}>", t.display(env)),
                _ => format!("Result<{}, {}>", t.display(env), e.display(env)),
            },
            Type::Future(t) => format!("Future<{}>", t.display(env)),
            Type::Nominal(id) => env.info(*id).name.clone(),
            Type::Nullable(t) => format!("{}?", t.display(env)),
            Type::Function { params, ret } => {
                let ps: Vec<String> = params.iter().map(|p| p.display(env)).collect();
                format!("fn({}) -> {}", ps.join(", "), ret.display(env))
            }
            Type::Tuple(items) => {
                let parts: Vec<String> = items.iter().map(|t| t.display(env)).collect();
                if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                }
            }
            Type::Any => "Any".into(),
            Type::PyAny => "PyAny".into(),
        }
    }
}

/// Info de un tipo nominal declarado en el programa.
#[derive(Debug, Clone)]
pub struct NominalInfo {
    pub name: String,
    /// Campos resueltos. `None` mientras el tipo está siendo
    /// registrado en la primera vuelta (forward decl); se completa
    /// en la segunda vuelta una vez que todos los nominales son
    /// conocidos.
    pub fields: Option<Vec<ResolvedField>>,
    /// R.3 — métodos custom resueltos. Cada entry tiene el nombre del
    /// método, su firma `Function { params, ret }` resuelta a tipos
    /// Fitz y un flag `is_async` para que `infer_method_call` pueda
    /// envolver el ret en `Future<T>`. `Vec::new()` si el tipo no
    /// declara métodos.
    pub methods: Vec<NominalMethod>,
}

#[derive(Debug, Clone)]
pub struct NominalMethod {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
    pub is_async: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedField {
    pub name: String,
    pub type_: Type,
}

/// Entorno de tipos del programa. Lleva:
///  - Built-ins (primitivos y genéricos), implícitos vía
///    `resolve_named`.
///  - Tipos nominales declarados, accesibles por nombre.
///
/// Sin scopes anidados todavía: 5.2 trabaja a nivel del programa
/// completo. Cuando entren chequeos de bodies (5.3) se agregarán
/// scopes locales para `let`/params.
#[derive(Debug, Default)]
pub struct TypeEnv {
    nominals: Vec<NominalInfo>,
    by_name: HashMap<String, TypeId>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un tipo nominal por nombre, devolviendo su id.
    /// Si el nombre ya estaba → error "tipo redeclarado".
    pub fn declare_nominal(&mut self, name: String) -> Result<TypeId, FitzError> {
        if self.by_name.contains_key(&name) {
            return Err(FitzError::new(
                ErrorKind::TypeError,
                0,
                0,
                format!("tipo `{}` declarado más de una vez", name),
            ));
        }
        let id = TypeId(self.nominals.len());
        self.nominals.push(NominalInfo {
            name: name.clone(),
            fields: None,
            methods: Vec::new(),
        });
        self.by_name.insert(name, id);
        Ok(id)
    }

    /// Completa los fields de un nominal (segunda vuelta).
    pub fn set_fields(&mut self, id: TypeId, fields: Vec<ResolvedField>) {
        self.nominals[id.0].fields = Some(fields);
    }

    /// R.3 — Setea los métodos de un nominal (tercera vuelta).
    pub fn set_methods(&mut self, id: TypeId, methods: Vec<NominalMethod>) {
        self.nominals[id.0].methods = methods;
    }

    pub fn lookup(&self, name: &str) -> Option<TypeId> {
        self.by_name.get(name).copied()
    }

    pub fn info(&self, id: TypeId) -> &NominalInfo {
        &self.nominals[id.0]
    }

    /// Cantidad de nominales registrados. Útil para tests.
    #[allow(dead_code)]
    pub fn nominal_count(&self) -> usize {
        self.nominals.len()
    }
}

// ---------------------------------------------------------------------------
// Side-table de tipos sintetizados por nodo (Fase 9.0 — F16)
// ---------------------------------------------------------------------------

/// Clave hashable derivada de un `Span`. Existe porque `Span` tiene un
/// `PartialEq` custom que devuelve `true` siempre (necesario para que
/// los tests de AST comparen estructura sin re-derivar posiciones del
/// parser; ver el comentario sobre `impl PartialEq for Span` en
/// `src/ast.rs`). Con esa semántica, `Span` no sirve como clave de
/// `HashMap` — todas las entradas colisionarían. `SpanKey` envuelve
/// `(line, column)` con `Eq`/`Hash` reales para el side-table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanKey(pub usize, pub usize);

impl From<Span> for SpanKey {
    fn from(s: Span) -> Self {
        SpanKey(s.line, s.column)
    }
}

/// Side-table que persiste el `Type` sintetizado por `infer_expr` para
/// cada nodo `Expr` con `Span` conocido. Pre-requisito habilitante del
/// LSP (Fase 9): `textDocument/hover` consulta el tipo del nodo bajo
/// el cursor, y completion contextual (`u.` → fields de `User`)
/// necesita el tipo del receptor.
///
/// Política de poblamiento:
/// - El wrapper sobre `infer_expr` registra **todos** los `Expr` que
///   pasan por el checker — granularidad amplia, simple, sin "olvidé
///   tal caso".
/// - Nodos con `Span::ZERO` (sintéticos del parser, nodos de tests) se
///   omiten: no son user-visible y dos sintéticos colisionarían bajo
///   la misma clave `(0, 0)`.
/// - `Expr::Error` (F15) tipa como `Type::Any` y se persiste igual —
///   el LSP decide qué mostrar.
///
/// Sin index espacial (rango inicio-fin). Para hover, el LSP elige el
/// nodo cuyo span está más cerca del cursor; un futuro refinamiento
/// con rangos completos queda como deuda menor (requiere `end_span` en
/// `Expr`).
#[derive(Debug, Clone, Default)]
pub struct TypeInfo {
    inner: HashMap<SpanKey, Type>,
}

impl TypeInfo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Persiste el `Type` asociado al `Span` del nodo. Omite silenciosa
    /// para `Span::ZERO` (nodos sintéticos / tests): esos no aportan a
    /// hover y colisionarían entre sí.
    pub fn record(&mut self, span: Span, ty: Type) {
        if !span.is_known() {
            return;
        }
        self.inner.insert(SpanKey::from(span), ty);
    }

    /// Devuelve el `Type` previamente registrado para `span`, si existe.
    /// API pública para el LSP (Fase 9.x.2 — hover). `#[allow(dead_code)]`
    /// hasta que aterricen los consumidores, mismo patrón que
    /// `parse_with_recovery` en F15.
    #[allow(dead_code)]
    pub fn type_at(&self, span: Span) -> Option<&Type> {
        if !span.is_known() {
            return None;
        }
        self.inner.get(&SpanKey::from(span))
    }

    /// Cantidad de entries en el side-table. Útil para smoke tests y
    /// para que el LSP estime cobertura. `#[allow(dead_code)]` hasta
    /// que aterricen los consumidores externos.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` si no hay entries registradas.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Itera todas las entries del side-table. Útil para consumidores
    /// del LSP (Fase 9.x.2 — hover) que necesitan hacer un lookup
    /// heurístico sobre posiciones (encontrar el span más cercano a
    /// un cursor). Sin esto, `type_at` solo permite lookup exacto.
    pub fn iter(&self) -> impl Iterator<Item = (&SpanKey, &Type)> {
        self.inner.iter()
    }
}

/// Side-table que persiste el `Span` de la **declaración** de cada
/// `Ident` usado en el programa. Pre-requisito habilitante del LSP
/// (Fase 9.x.3 — go-to-definition): `textDocument/definition` busca
/// el ident bajo el cursor y devuelve la ubicación donde fue
/// declarado.
///
/// Política de poblamiento:
/// - Cada `Expr::Ident(name, use_span)` que el checker resuelve
///   exitosamente vía `lookup_binding` registra
///   `(use_span → def_span)` cuando la binding tiene span conocido.
/// - **Builtins** (`print`, `len`, `sleep`, `cors`) tienen
///   `def_span = Span::ZERO` y se omiten (no hay archivo donde
///   saltar).
/// - **Nodos con `use_span == Span::ZERO`** (sintéticos / tests)
///   se omiten igual que en `TypeInfo`.
///
/// Granularidad del `def_span` registrado: por limitaciones del AST
/// actual (sin spans propios en `AssignTarget::Ident`/`Param`/
/// `For.var`), usamos el span del `Stmt` contenedor como
/// aproximación. VSCode salta al stmt — el usuario ve la línea de
/// declaración. Precisión por nombre exacto queda como deuda S1.
#[derive(Debug, Clone, Default)]
pub struct DefinitionInfo {
    inner: HashMap<SpanKey, Span>,
}

impl DefinitionInfo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Persiste la relación `use_span → def_span`. Omite silenciosa
    /// cuando alguno de los dos es `Span::ZERO` (sintéticos / builtins).
    pub fn record(&mut self, use_span: Span, def_span: Span) {
        if !use_span.is_known() || !def_span.is_known() {
            return;
        }
        self.inner.insert(SpanKey::from(use_span), def_span);
    }

    /// Lookup exacto por span del uso. API pública para tests.
    #[allow(dead_code)]
    pub fn definition_at(&self, use_span: Span) -> Option<Span> {
        if !use_span.is_known() {
            return None;
        }
        self.inner.get(&SpanKey::from(use_span)).copied()
    }

    /// Cantidad de entries. `#[allow(dead_code)]` paralelo a
    /// `TypeInfo::len`.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` si no hay entries registradas.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Itera todas las entries. Útil para el LSP (Fase 9.x.3) que
    /// hace lookup heurístico sobre posiciones del cursor.
    pub fn iter(&self) -> impl Iterator<Item = (&SpanKey, &Span)> {
        self.inner.iter()
    }
}

// ---------------------------------------------------------------------------
// Resolución de TypeExpr → Type
// ---------------------------------------------------------------------------

/// Convierte un `TypeExpr` (sintáctico) en un `Type` (resuelto)
/// contra `env`. Devuelve el `Type` o un `FitzError` describiendo
/// qué falló. Los errores siempre son `ErrorKind::TypeError`.
pub fn resolve_type_expr(t: &TypeExpr, env: &TypeEnv) -> Result<Type, FitzError> {
    match t {
        TypeExpr::Named(name) => resolve_named(name, &[], env),
        TypeExpr::Generic { name, args } => resolve_named(name, args, env),
        TypeExpr::Nullable(inner) => {
            let inner = resolve_type_expr(inner, env)?;
            Ok(Type::Nullable(Box::new(inner)))
        }
        TypeExpr::Function { params, ret } => {
            let params: Vec<Type> = params
                .iter()
                .map(|p| resolve_type_expr(p, env))
                .collect::<Result<_, _>>()?;
            let ret = resolve_type_expr(ret, env)?;
            Ok(Type::Function {
                params,
                ret: Box::new(ret),
            })
        }
        // Tuples (mini-tanda T): resolución elemento por elemento.
        TypeExpr::Tuple(items) => {
            let resolved: Vec<Type> = items
                .iter()
                .map(|t| resolve_type_expr(t, env))
                .collect::<Result<_, _>>()?;
            Ok(Type::Tuple(resolved))
        }
    }
}

/// Resuelve un nombre + argumentos contra el env. La separación
/// entre `Named` y `Generic` desaparece acá: `List<Int>` y
/// `List` (sin argumentos) toman el mismo camino y la aridad
/// validada en el lugar correspondiente.
fn resolve_named(name: &str, args: &[TypeExpr], env: &TypeEnv) -> Result<Type, FitzError> {
    // Primitivos (aridad 0). Si el usuario los aplica como genéricos
    // → error de aridad explícito.
    let prim = match name {
        "Int" => Some(Type::Int),
        "Float" => Some(Type::Float),
        "Str" => Some(Type::Str),
        "Bool" => Some(Type::Bool),
        "Null" => Some(Type::Null),
        "Range" => Some(Type::Range),
        _ => None,
    };
    if let Some(t) = prim {
        if !args.is_empty() {
            return Err(arity_error(name, 0, args.len()));
        }
        return Ok(t);
    }

    // Genéricos built-in con aridad fija.
    match name {
        "List" => {
            expect_arity(name, 1, args)?;
            let inner = resolve_type_expr(&args[0], env)?;
            Ok(Type::List(Box::new(inner)))
        }
        "Map" => {
            expect_arity(name, 2, args)?;
            let k = resolve_type_expr(&args[0], env)?;
            let v = resolve_type_expr(&args[1], env)?;
            Ok(Type::Map(Box::new(k), Box::new(v)))
        }
        "Result" => {
            // Mini-tanda Re+ — aridad 1 o 2. `Result<T>` se expande a
            // `Result<T, Str>` (default por compatibilidad). `Result<T, E>`
            // con E explícito habilita carry de tipos custom en Err.
            if args.is_empty() || args.len() > 2 {
                return Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    format!(
                        "el tipo `Result` espera 1 o 2 argumentos, recibió {}",
                        args.len()
                    ),
                ));
            }
            let ok = resolve_type_expr(&args[0], env)?;
            let err = if args.len() == 2 {
                resolve_type_expr(&args[1], env)?
            } else {
                Type::Str
            };
            Ok(Type::Result { ok: Box::new(ok), err: Box::new(err) })
        }
        "Future" => {
            expect_arity(name, 1, args)?;
            let inner = resolve_type_expr(&args[0], env)?;
            Ok(Type::Future(Box::new(inner)))
        }
        _ => {
            // Nominal declarado por el usuario.
            match env.lookup(name) {
                Some(id) => {
                    if !args.is_empty() {
                        return Err(FitzError::new(
                            ErrorKind::TypeError,
                            0,
                            0,
                            format!(
                                "tipo `{}` no es genérico, no acepta argumentos de tipo",
                                name
                            ),
                        ));
                    }
                    Ok(Type::Nominal(id))
                }
                None => Err(FitzError::new(
                    ErrorKind::TypeError,
                    0,
                    0,
                    format!("tipo desconocido `{}`", name),
                )),
            }
        }
    }
}

fn expect_arity(name: &str, expected: usize, args: &[TypeExpr]) -> Result<(), FitzError> {
    if args.len() != expected {
        Err(arity_error(name, expected, args.len()))
    } else {
        Ok(())
    }
}

/// Pre-registra los tipos built-in que aporta el runtime HTTP de Fitz.
/// Hoy: `Request` (lo construye el dispatcher antes de cada handler/
/// middleware; expone `method`, `path`, `headers`) y `Response` (marker
/// opaco para anotar el retorno de middlewares — el valor real lo
/// produce `return <status> { ... }`).
///
/// Se llama desde `resolve_program` antes de la vuelta 1, así que un
/// `type Request { ... }` declarado por el usuario dispara el error de
/// redeclaración existente. El precio: dos nominales fijos en el env
/// aún en programas que no usan HTTP. Trade-off aceptable — los costos
/// de chequeo se mantienen O(1) y la superficie semántica del lenguaje
/// queda consistente.
fn register_http_builtin_types(env: &mut TypeEnv) {
    // `Request`: el id que queda asignado es estable porque corremos
    // antes que cualquier otra registración. Sus fields se completan
    // explícito (no derivados de un Stmt::TypeDef).
    let req_id = env
        .declare_nominal("Request".to_string())
        .expect("Request es el primer nominal — no puede colisionar");
    env.set_fields(
        req_id,
        vec![
            ResolvedField { name: "method".into(), type_: Type::Str },
            ResolvedField { name: "path".into(), type_: Type::Str },
            ResolvedField {
                name: "headers".into(),
                type_: Type::Map(Box::new(Type::Str), Box::new(Type::Str)),
            },
        ],
    );

    // `Response`: nominal opaco sin fields. El usuario no lo instancia
    // con struct lit (`Response { ... }` daría error: falta cualquier
    // field — pero como no tiene, struct lit con `{}` pasa; documentado).
    // El uso esperado es como marker en firmas: `fn auth(req) -> Response?`.
    let resp_id = env
        .declare_nominal("Response".to_string())
        .expect("Response es el segundo nominal — no puede colisionar");
    env.set_fields(resp_id, vec![]);
}

fn arity_error(name: &str, expected: usize, found: usize) -> FitzError {
    FitzError::new(
        ErrorKind::TypeError,
        0,
        0,
        format!(
            "el tipo `{}` espera {} argumento(s) de tipo, recibió {}",
            name, expected, found
        ),
    )
}

// ---------------------------------------------------------------------------
// Pasada de resolución sobre el programa
// ---------------------------------------------------------------------------

/// Resultado de chequear un programa: el `TypeEnv` con todos los
/// tipos declarados resueltos, y la lista (posiblemente vacía) de
/// errores acumulados. Devolvemos ambos siempre: el caller decide
/// si abortar (modo strict) o reportar como warnings (modo run).
pub fn resolve_program(program: &Program) -> (TypeEnv, Vec<FitzError>) {
    let mut env = TypeEnv::new();
    let mut errors = Vec::new();

    // Vuelta 0 (mini-fase MW.1): registrar tipos built-in del runtime HTTP.
    // `Request` lo construye el dispatcher antes de invocar middlewares
    // y handlers; el usuario lo lee adentro de sus middlewares con
    // `req.method`, `req.path`, `req.headers`. `Response` queda como
    // marker opaco para anotar `-> Response?` en middlewares; el usuario
    // no lo instancia (el valor lo produce `return <status> { ... }`).
    // Si el usuario declara `type Request`/`type Response`, la vuelta 1
    // emite el error de redeclaración existente.
    register_http_builtin_types(&mut env);

    // Vuelta 1: registrar los nombres de los `type` declarados localmente.
    // Forward refs entre nominales locales.
    for stmt in program {
        if let Stmt::TypeDef { name, .. } = stmt {
            if let Err(e) = env.declare_nominal(name.clone()) {
                errors.push(e);
            }
        }
    }

    // Vuelta 1b: registrar nombres traídos por `from ... import ...`
    // como nominales con fields desconocidos. Sin esto, un
    // `User { ... }` que viene de `from foo import User` queda sin
    // tipo declarado y el checker se queja. Si el nombre choca con
    // un type local, gana el local — el import se ignora en silencio
    // (decisión: 5.x mantiene comportamiento gradual; cuando 5.3.x
    // cargue módulos cross-archivo, podemos refinar el warning).
    //
    // `import foo` no agrega nombres en el TypeEnv — el módulo es un
    // value, no un type. Se registra como var en `check_stmt`.
    for stmt in program {
        if let Stmt::FromImport { names, .. } = stmt {
            for (n, alias) in names {
                // PreF8.4: con alias, el binding local en el TypeEnv
                // usa el alias. Sin alias, el nombre original.
                let binding = alias.clone().unwrap_or_else(|| n.clone());
                if env.lookup(&binding).is_none() {
                    // declare_nominal puede fallar solo si el nombre
                    // ya estaba; ya chequeamos así que es seguro.
                    let _ = env.declare_nominal(binding);
                }
            }
        }
    }

    // Vuelta 2: resolver los fields de cada `type`.
    for stmt in program {
        if let Stmt::TypeDef { name, fields, .. } = stmt {
            // Si la declaración falló (duplicado), no hay id que actualizar.
            let id = match env.lookup(name) {
                Some(id) => id,
                None => continue,
            };
            // Si el slot ya tiene fields, es la segunda vez que vemos
            // este nominal — un duplicado que ya reportamos. Saltar.
            if env.info(id).fields.is_some() {
                continue;
            }
            let mut resolved = Vec::new();
            for f in fields {
                match resolve_type_expr(&f.type_, &env) {
                    Ok(t) => {
                        if let Some(default) = &f.default {
                            if let Err(e) =
                                check_field_default(name, &f.name, &t, default, &env)
                            {
                                errors.push(e);
                            }
                        }
                        resolved.push(ResolvedField {
                            name: f.name.clone(),
                            type_: t,
                        });
                    }
                    Err(e) => errors.push(annotate(
                        e,
                        &format!("en el campo `{}` del tipo `{}`", f.name, name),
                    )),
                }
            }
            env.set_fields(id, resolved);
        }
    }

    // Vuelta 2.5 (R.3): resolver firmas de métodos custom. Después de
    // tener fields, los métodos pueden referenciar nominales en sus
    // params/return. Si un método ya tiene firma resuelta (segundo
    // import / forward ref), saltamos.
    for stmt in program {
        if let Stmt::TypeDef { name, methods, .. } = stmt {
            if methods.is_empty() {
                continue;
            }
            let id = match env.lookup(name) {
                Some(id) => id,
                None => continue,
            };
            if !env.info(id).methods.is_empty() {
                continue;
            }
            let mut resolved_methods: Vec<NominalMethod> = Vec::with_capacity(methods.len());
            for m in methods {
                let mut params = Vec::with_capacity(m.params.len());
                for p in &m.params {
                    let pty = match &p.type_ {
                        Some(t) => resolve_type_expr(t, &env).unwrap_or(Type::Any),
                        None => Type::Any,
                    };
                    params.push(pty);
                }
                let ret = match &m.return_type {
                    Some(r) => resolve_type_expr(r, &env).unwrap_or(Type::Any),
                    None => Type::Any,
                };
                resolved_methods.push(NominalMethod {
                    name: m.name.clone(),
                    params,
                    ret,
                    is_async: m.is_async,
                });
            }
            env.set_methods(id, resolved_methods);
        }
    }

    // Vuelta 3: anotaciones de FnDef / Assign / let internos.
    for stmt in program {
        resolve_stmt_annotations(stmt, &env, &mut errors);
    }

    (env, errors)
}

fn resolve_stmt_annotations(stmt: &Stmt, env: &TypeEnv, errors: &mut Vec<FitzError>) {
    match stmt {
        Stmt::Assign { type_: Some(t), .. } => {
            if let Err(e) = resolve_type_expr(t, env) {
                errors.push(e);
            }
        }
        Stmt::FnDef {
            name,
            params,
            return_type,
            body,
            ..
        } => {
            for p in params {
                if let Some(t) = &p.type_ {
                    if let Err(e) = resolve_type_expr(t, env) {
                        errors.push(annotate(
                            e,
                            &format!("en el parámetro `{}` de la función `{}`", p.name, name),
                        ));
                    }
                }
            }
            if let Some(t) = return_type {
                if let Err(e) = resolve_type_expr(t, env) {
                    errors.push(annotate(
                        e,
                        &format!("en el tipo de retorno de la función `{}`", name),
                    ));
                }
            }
            // Bajamos por el body para validar anotaciones de lets
            // internos. Las expresiones en sí (cuerpo del fn) se
            // validan en 5.3.
            for s in body {
                resolve_stmt_annotations(s, env, errors);
            }
        }
        Stmt::While { body, .. } | Stmt::Loop { body, .. } | Stmt::For { body, .. } => {
            for s in body {
                resolve_stmt_annotations(s, env, errors);
            }
        }
        _ => {}
    }
}

/// Chequea (caso simple) que un default literal coincida con el
/// tipo declarado del campo. Aplica solo a literales constantes:
/// otros defaults (expresiones, struct literals, llamadas) se
/// aceptan sin chequeo hasta 5.3, que valida expresiones contra
/// tipos esperados.
///
/// Reglas:
///   - `Null` aceptable si el declarado es `T?`.
///   - `Int` aceptable contra `Float` (coerción Int→Float, mismo
///     criterio que el evaluator usa en runtime).
///   - El resto: igualdad estructural sobre la base (pelando un
///     `Nullable` si lo hay).
fn check_field_default(
    type_name: &str,
    field_name: &str,
    declared: &Type,
    default: &Expr,
    env: &TypeEnv,
) -> Result<(), FitzError> {
    let lit_type = match default {
        Expr::Int(_, _) => Some(Type::Int),
        Expr::Float(_, _) => Some(Type::Float),
        Expr::Str(_, _) => Some(Type::Str),
        Expr::Bool(_, _) => Some(Type::Bool),
        Expr::Null(_) => Some(Type::Null),
        _ => None,
    };
    let lit_type = match lit_type {
        Some(t) => t,
        None => return Ok(()), // no literal, se valida en 5.3
    };
    // Null sobre tipo nullable: OK.
    if matches!(lit_type, Type::Null) && declared.is_nullable() {
        return Ok(());
    }
    // Coerción Int→Float.
    if matches!(lit_type, Type::Int) && matches!(declared.base(), Type::Float) {
        return Ok(());
    }
    // Igualdad estructural sobre la base.
    if &lit_type != declared.base() {
        return Err(FitzError::new(
            ErrorKind::TypeError,
            0,
            0,
            format!(
                "el campo `{}.{}` declarado como `{}` recibió un default `{}`",
                type_name,
                field_name,
                declared.display(env),
                lit_type.display(env),
            ),
        ));
    }
    Ok(())
}

/// Anexa contexto a un mensaje de error. El mensaje original queda
/// primero, el contexto entre paréntesis al final.
fn annotate(mut e: FitzError, context: &str) -> FitzError {
    e.message = format!("{} ({})", e.message, context);
    e
}

// ---------------------------------------------------------------------------
// Checker de expresiones (Fase 5.3.1)
//
// Mientras `resolve_program` chequea anotaciones, `check_program` corre
// además una pasada por las expresiones del programa. La idea:
//   1. Pre-registrar firmas de los `Stmt::FnDef` top-level y builtins
//      en un scope global de variables.
//   2. Recorrer cada Stmt, abriendo scopes por cada `FnDef`/loop/etc.
//   3. Para cada `Expr`, sintetizar su tipo (`infer_expr`).
//   4. Cuando hay un tipo *esperado* (anotación de `let`, default de
//      campo no-literal, etc.), validar compatibilidad.
//
// 5.3.1 cubre: literales, ident, BinOp aritmético/comparación/lógico,
// UnaryOp Neg, StrInterp, `if` expr, list/map literales, struct lit,
// field access sobre Nominal, Range. Resto devuelve `Any` y se cubre
// en 5.3.2+.
// ---------------------------------------------------------------------------

use crate::ast::{AssignTarget, BinOpKind, StrPart, UnaryOpKind};

/// Binding de una variable en un scope. Lleva el tipo y un flag
/// `annotated` que indica si la PRIMERA asignación de ese nombre
/// vino con anotación de tipo explícita (`x: Int = ...`). El flag
/// se usa para chequear reasignaciones: si la var fue anotada, las
/// reasignaciones posteriores sin anotación tienen que respetar
/// ese tipo. Si la var se infirió sin anotación, las
/// reasignaciones pueden cambiar el tipo (modelo gradual).
#[derive(Debug, Clone)]
struct VarBinding {
    ty: Type,
    annotated: bool,
    /// Span de la declaración (let stmt, fn def, type def, param, etc.).
    /// `Span::ZERO` para builtins — el LSP los filtra en go-to-definition
    /// porque no hay archivo donde saltar.
    def_span: Span,
}

/// Estado mutable durante la pasada de chequeo de expresiones.
struct CheckCtx<'a> {
    types: &'a TypeEnv,
    /// Stack de scopes para variables. El primero es el global
    /// (builtins + fns top-level + lets top-level). Cada `FnDef`
    /// body, cada loop body, abren un scope nuevo.
    scopes: Vec<std::collections::HashMap<String, VarBinding>>,
    /// Stack de tipos de retorno esperados, uno por cada función
    /// (FnDef o FnExpr) anidada que se está chequeando. Vacío en
    /// el scope top-level. `Stmt::Return` lo consulta para validar.
    return_stack: Vec<Type>,
    /// Stack paralelo a `return_stack`: cada frame recolecta los
    /// tipos sintetizados de los `Stmt::Return` adentro de esa
    /// función. `Expr::FnExpr` lo consume al salir para inferir su
    /// `ret`. Para `Stmt::FnDef` se acumula también pero se
    /// descarta (ya tenemos `return_type` declarado).
    inferred_returns: Vec<Vec<Type>>,
    /// Stack paralelo a `return_stack`: `true` cuando la fn actual
    /// es un handler HTTP (tiene decorator `@get`/`@post`/`@put`/
    /// `@delete`). `Stmt::ReturnStatus` (return con status code) lo
    /// consulta para validar que solo aparezca adentro de un handler.
    /// `FnExpr` no es handler nunca; pushea `false`.
    in_http_handler: Vec<bool>,
    /// Stack paralelo a `return_stack`: `true` cuando la fn actual es
    /// `async`. `Expr::Await` lo consulta para validar que solo aparezca
    /// adentro de una async fn. `FnExpr` no soporta async todavía
    /// (el parser no lo admite); siempre pushea `false`. Introducido
    /// en Fase 6.2.
    await_stack: Vec<bool>,
    /// Nombres de fns que aparecen como argumento de un `@middleware(...)`
    /// en algún FnDef del programa. Pre-scaneado en `check_program`. Lo
    /// usamos para tratar a esas fns como "contexto HTTP" a efectos de
    /// `Stmt::ReturnStatus` (un middleware puede hacer `return 401 { ... }`
    /// para short-circuitear el handler). Introducido en mini-fase MW.1.
    middleware_fn_names: std::collections::HashSet<String>,
    /// Mini-tanda L — stack paralelo a los `Expr::Loop` actualmente
    /// siendo chequeados. Cada frame recolecta los tipos de los
    /// valores de `break <v>` adentro. `Expr::Loop` consume el
    /// frame al salir para inferir el tipo de la expresión via
    /// `unify_returns`. Loops como statement (`Stmt::Loop`,
    /// `Stmt::While`, `Stmt::For`) NO empujan al stack — los
    /// `break <v>` adentro tipan el value pero NO se propagan.
    break_value_stack: Vec<Vec<Type>>,
    /// Profundidad de loops adentro de la función actual (R.2.4 — F3).
    /// `Stmt::Break`/`Continue` exige que este valor sea > 0; si es 0,
    /// el statement está huérfano (top-level o adentro de fn sin loop).
    /// `While`/`Loop`/`For` incrementan al entrar y decrementan al
    /// salir; `FnDef`/`FnExpr` guardan el valor previo, lo resetean a
    /// 0, y lo restauran al salir (un break adentro de una closure NO
    /// rompe el loop externo, igual que Rust).
    loop_depth: usize,
    errors: Vec<FitzError>,
    /// Side-table de tipos sintetizados por nodo `Expr` (Fase 9.0 — F16).
    /// Poblado por el wrapper `infer_expr` al salir de cada llamada; se
    /// expone vía `check_program` para que el LSP responda hover y
    /// completion contextual.
    type_info: TypeInfo,
    /// Side-table de definiciones por uso (Fase 9.x.3 — go-to-definition).
    /// Poblado cuando `infer_expr` resuelve un `Expr::Ident` vía
    /// `lookup_binding` y la binding tiene `def_span` conocido (no
    /// builtin). Mismo flujo de exposición que `type_info`.
    def_info: DefinitionInfo,
}

impl<'a> CheckCtx<'a> {
    fn new(types: &'a TypeEnv) -> Self {
        let mut ctx = Self {
            types,
            scopes: vec![std::collections::HashMap::new()],
            return_stack: Vec::new(),
            inferred_returns: Vec::new(),
            in_http_handler: Vec::new(),
            await_stack: Vec::new(),
            middleware_fn_names: std::collections::HashSet::new(),
            loop_depth: 0,
            break_value_stack: Vec::new(),
            errors: Vec::new(),
            type_info: TypeInfo::new(),
            def_info: DefinitionInfo::new(),
        };
        ctx.register_builtins();
        ctx
    }

    /// Builtins del lenguaje que existen siempre en el env del
    /// evaluator. Los de aridad fija reciben firma real (chequea
    /// aridad y eventualmente tipos); los variádicos se modelan
    /// como `Any` hasta tener una representación dedicada.
    fn register_builtins(&mut self) {
        // Todos los builtins usan `def_span: Span::ZERO` — no hay
        // archivo Fitz donde saltar para go-to-definition. El LSP
        // los filtra al responder `textDocument/definition`.

        // `print(args...)` — variádico. Modelado como Any: ningún
        // call sobre Any se chequea (gradual escape).
        self.scopes[0].insert(
            "print".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
            },
        );
        // `len(x) -> Int` — aridad 1 sobre List/Map/Str/Range. El
        // param es Any porque los receptores no comparten un solo
        // tipo (todavía no tenemos union types / "any iterable").
        // La aridad sí se valida; el tipo del receptor llega en 5.3.4.
        self.scopes[0].insert(
            "len".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Any],
                    ret: Box::new(Type::Int),
                },
                annotated: false,
                def_span: Span::ZERO,
            },
        );
        // `cors(config: Map?) -> CorsConfig` — built-in MW.2.
        // Hoy lo tipamos como `Any` (variádico de facto: 0 o 1 arg, y
        // el Map adentro tiene tipos heterogéneos por key). Una firma
        // más precisa requiere union types o un tipo dedicado para
        // CorsConfig en el `Type` enum — out of scope para MW.2.
        // El evaluator hace la validación completa en runtime.
        self.scopes[0].insert(
            "cors".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
            },
        );
        // `sleep(ms: Int) -> Future<Null>` — primer async primitive.
        // Introducido en Fase 6.3. La firma envuelve `Null` en
        // `Future<Null>` (paralelo a cualquier `async fn` del usuario):
        // el usuario obligatoriamente la await-ea adentro de otra
        // `async fn`, o guarda el Future suelto. El evaluator tiene
        // un stub que falla con "llega en 6.4" hasta que aterrice
        // el evaluator async.
        self.scopes[0].insert(
            "sleep".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Int],
                    ret: Box::new(Type::Future(Box::new(Type::Null))),
                },
                annotated: false,
                def_span: Span::ZERO,
            },
        );
        // Fase 9.z.2.a — assertion builtins. `assert` queda como `Any`
        // porque tiene aridad variable (1 o 2 args, msg opcional); el
        // runtime valida tipos y aridad. `assert_eq`/`assert_ne` tienen
        // aridad fija con args `Any` (estructural equality maneja
        // cualquier tipo). `assert_throws` exige `Function` aridad 0.
        self.scopes[0].insert(
            "assert".into(),
            VarBinding {
                ty: Type::Any,
                annotated: false,
                def_span: Span::ZERO,
            },
        );
        self.scopes[0].insert(
            "assert_eq".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Any, Type::Any],
                    ret: Box::new(Type::Null),
                },
                annotated: false,
                def_span: Span::ZERO,
            },
        );
        self.scopes[0].insert(
            "assert_ne".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Any, Type::Any],
                    ret: Box::new(Type::Null),
                },
                annotated: false,
                def_span: Span::ZERO,
            },
        );
        self.scopes[0].insert(
            "assert_throws".into(),
            VarBinding {
                ty: Type::Function {
                    params: vec![Type::Function {
                        params: vec![],
                        ret: Box::new(Type::Any),
                    }],
                    ret: Box::new(Type::Null),
                },
                annotated: false,
                def_span: Span::ZERO,
            },
        );
    }

    fn push_scope(&mut self) {
        self.scopes.push(std::collections::HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Declara una variable sin anotación de tipo (inferida o
    /// gradual). Permite que reasignaciones futuras cambien el
    /// tipo libremente. `def_span` es la posición de la declaración
    /// (Fase 9.x.3 — usado por go-to-definition); pasar `Span::ZERO`
    /// para builtins / declaraciones sintéticas.
    fn declare_var(&mut self, name: String, ty: Type, def_span: Span) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(
                name,
                VarBinding {
                    ty,
                    annotated: false,
                    def_span,
                },
            );
        }
    }

    /// Declara una variable con anotación explícita de tipo. Las
    /// reasignaciones posteriores sin anotación se van a chequear
    /// contra este tipo. `def_span` igual que `declare_var`.
    fn declare_var_annotated(&mut self, name: String, ty: Type, def_span: Span) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(
                name,
                VarBinding {
                    ty,
                    annotated: true,
                    def_span,
                },
            );
        }
    }

    fn lookup_binding(&self, name: &str) -> Option<&VarBinding> {
        for s in self.scopes.iter().rev() {
            if let Some(b) = s.get(name) {
                return Some(b);
            }
        }
        None
    }

    /// Reporta un error sin posición conocida. Tras S1.2 sub-paso 2,
    /// los sitios de error sobre `Expr` ya conocen su span y usan
    /// `error_at`. Este helper queda para reportes "globales" (sin
    /// nodo asociado) que puedan aparecer en el futuro.
    #[allow(dead_code)]
    fn error(&mut self, msg: impl Into<String>) {
        self.errors
            .push(FitzError::new(ErrorKind::TypeError, 0, 0, msg.into()));
    }

    /// Variante de `error` que cita la posición real del nodo (línea
    /// y columna del primer token del `Stmt`). Lo usan los sitios de
    /// reporte stmt-level — ver `check_stmt`. Cuando el span es
    /// `Span::ZERO` (nodos sintéticos del parser o tests),
    /// `FitzError::Display` omite el prefijo "en línea N:M" por la
    /// regla `is_known()` de Span — el comportamiento queda idéntico
    /// a `error` para esos casos.
    fn error_at(&mut self, span: Span, msg: impl Into<String>) {
        self.errors.push(FitzError::new(
            ErrorKind::TypeError,
            span.line,
            span.column,
            msg.into(),
        ));
    }
}

/// Convierte una `Option<TypeExpr>` en `Type` para anotaciones del
/// usuario. Si la anotación faltó → `Any`. Si la anotación está pero
/// no resuelve → `Any` y se asume que el error ya fue reportado por
/// `resolve_program`.
fn ann_to_type(ann: Option<&TypeExpr>, env: &TypeEnv) -> Type {
    match ann {
        None => Type::Any,
        Some(t) => resolve_type_expr(t, env).unwrap_or(Type::Any),
    }
}

/// Sintetiza el tipo de una expresión y lo persiste en el side-table
/// `ctx.type_info` antes de devolverlo. La lógica de síntesis vive en
/// `synthesize_expr`; este wrapper centraliza el `record` para que
/// **todos** los nodos `Expr` queden registrados al pasar por el
/// checker (incluyendo recursión: el wrapper se llama por nodo, así
/// que `BinOp { left, right }` y sus operandos quedan los tres). Nodos
/// con `Span::ZERO` (sintéticos / tests) se omiten — ver `TypeInfo::
/// record`. Pre-req habilitante del LSP (Fase 9 — F16).
fn infer_expr(ctx: &mut CheckCtx, e: &Expr) -> Type {
    let ty = synthesize_expr(ctx, e);
    ctx.type_info.record(e.span(), ty.clone());
    ty
}

/// Núcleo de síntesis. NO toca `type_info` directamente — el wrapper
/// `infer_expr` lo hace al salir. Esto centraliza la política de
/// poblamiento del side-table en un solo punto, evitando que cada
/// branch del match tenga que recordar el `record`.
///
/// Casos no cubiertos en 5.3.1 devuelven `Type::Any` silenciosamente
/// — no son errores, solo no chequeamos esa forma todavía. Las
/// sub-fases siguientes (5.3.2 calls, 5.3.3 Result, 5.3.4 métodos,
/// 5.3.5 FnExpr) los irán reemplazando.
fn synthesize_expr(ctx: &mut CheckCtx, e: &Expr) -> Type {
    match e {
        Expr::Int(_, _) => Type::Int,
        Expr::Float(_, _) => Type::Float,
        Expr::Str(_, _) => Type::Str,
        Expr::Bool(_, _) => Type::Bool,
        Expr::Null(_) => Type::Null,

        // Mini-tanda L — `loop { body }` como expresión. El tipo es
        // el `lub` de los valores de `break <v>` adentro. Sin
        // breaks con valor → `Null`. Recolectar los tipos de break
        // requiere walkear el body; usamos un side-channel
        // `break_value_stack` que `Stmt::Break(Some(e), _)` alimenta.
        Expr::Loop { body, .. } => {
            ctx.loop_depth += 1;
            ctx.break_value_stack.push(Vec::new());
            for s in body {
                check_stmt(ctx, s);
            }
            let values = ctx.break_value_stack.pop().unwrap_or_default();
            ctx.loop_depth -= 1;
            unify_returns(&values)
        }

        // Tuples (mini-tanda T) — tipamos cada slot y armamos
        // `Type::Tuple`.
        Expr::Tuple(items, _) => {
            let tys: Vec<Type> = items.iter().map(|x| infer_expr(ctx, x)).collect();
            Type::Tuple(tys)
        }
        Expr::TupleField { tuple, index, span } => {
            let ty = infer_expr(ctx, tuple);
            match ty.base() {
                Type::Tuple(items) => {
                    if let Some(t) = items.get(*index) {
                        t.clone()
                    } else {
                        ctx.error_at(*span, format!(
                            "tupla de {} elementos no tiene índice `{}`",
                            items.len(), index
                        ));
                        Type::Any
                    }
                }
                Type::Any | Type::PyAny => Type::Any,
                other => {
                    ctx.error_at(*span, format!(
                        "acceso `.{}` solo aplica a tuplas, recibí `{}`",
                        index, other.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }

        Expr::StrInterp(parts, _) => {
            // Las sub-expresiones se evalúan para errores aunque el
            // resultado siempre sea Str. Mini-tanda Fm: el spec se
            // valida en `validate_format_spec_for_type` — el filter de
            // tipos numéricos vs `f`, etc.
            for p in parts {
                if let StrPart::Expr(inner, spec) = p {
                    let ty = infer_expr(ctx, inner);
                    if let Some(s) = spec {
                        validate_format_spec_for_type(ctx, s, &ty, inner.span());
                    }
                }
            }
            Type::Str
        }

        Expr::Ident(name, span) => {
            // Resolvemos el binding y clonamos lo necesario para liberar
            // el préstamo inmutable de `ctx.scopes` antes de tocar
            // `ctx.def_info` (que requiere &mut self). Fase 9.x.3:
            // registramos el `def_span` para go-to-definition cuando
            // existe (no es builtin con Span::ZERO).
            let resolved = ctx
                .lookup_binding(name)
                .map(|b| (b.ty.clone(), b.def_span));
            if let Some((ty, def_span)) = resolved {
                ctx.def_info.record(*span, def_span);
                return ty;
            }
            // Si es un tipo nominal declarado, el usuario lo está
            // usando como valor (lo cual el evaluator soporta:
            // registra Value::Type en el env). No es error; lo
            // tratamos como Any.
            if ctx.types.lookup(name).is_some() {
                return Type::Any;
            }
            ctx.error_at(*span, format!("variable desconocida `{}`", name));
            Type::Any
        }

        Expr::UnaryOp { op, operand, span } => {
            let t = infer_expr(ctx, operand);
            match op {
                UnaryOpKind::Neg => match &t {
                    Type::Int | Type::Float | Type::Any => t,
                    other => {
                        ctx.error_at(*span, format!(
                            "el operador `-` (negación) espera Int o Float, recibió `{}`",
                            other.display(ctx.types)
                        ));
                        Type::Any
                    }
                },
                // R.1.1 — `not <expr>` exige `Bool` estricto. Sin
                // truthy/falsy en Fitz: pasar `Int`/`Str`/etc. es
                // error de tipo (consistente con `assert(cond)` que
                // también exige Bool estricto).
                UnaryOpKind::Not => match &t {
                    Type::Bool | Type::Any => Type::Bool,
                    other => {
                        ctx.error_at(*span, format!(
                            "el operador `not` espera Bool, recibió `{}`",
                            other.display(ctx.types)
                        ));
                        Type::Bool
                    }
                },
                // Mini-tanda Bits — `~x` solo Int.
                UnaryOpKind::BitNot => match &t {
                    Type::Int | Type::Any => Type::Int,
                    other => {
                        ctx.error_at(*span, format!(
                            "el operador `~` espera Int, recibió `{}`",
                            other.display(ctx.types)
                        ));
                        Type::Int
                    }
                },
            }
        }

        Expr::BinOp { op, left, right, span } => {
            let lt = infer_expr(ctx, left);
            let rt = infer_expr(ctx, right);
            infer_binop(ctx, op, &lt, &rt, *span)
        }

        Expr::If { condition, then, else_, .. } => {
            // Condición debe ser Bool (o Any).
            let cond_ty = infer_expr(ctx, condition);
            if !is_compatible(&cond_ty, &Type::Bool) {
                // Apuntamos al span de la condición misma — mejor
                // pista que el `if` mismo.
                ctx.error_at(condition.span(), format!(
                    "la condición de `if` debe ser Bool, recibió `{}`",
                    cond_ty.display(ctx.types)
                ));
            }
            // Cada rama es un bloque; el "tipo" de un if-stmt es el
            // de su última expresión-stmt. Para 5.3.1 nos alcanza con
            // walkear los bloques (con scope) y devolver Any.
            ctx.push_scope();
            check_block(ctx, then);
            ctx.pop_scope();
            if let Some(else_body) = else_ {
                ctx.push_scope();
                check_block(ctx, else_body);
                ctx.pop_scope();
            }
            Type::Any
        }

        Expr::List(items, _) => {
            // List<T> con T = tipo del primer elemento si los demás
            // son compatibles; si hay mezcla, T = Any.
            if items.is_empty() {
                return Type::List(Box::new(Type::Any));
            }
            let first = infer_expr(ctx, &items[0]);
            let mut all_same = true;
            for it in &items[1..] {
                let t = infer_expr(ctx, it);
                if !is_compatible(&t, &first) {
                    all_same = false;
                }
            }
            if all_same {
                Type::List(Box::new(first))
            } else {
                Type::List(Box::new(Type::Any))
            }
        }

        // Mini-tanda C — `[expr for var in iter [if filter]]`. Tipa el
        // iter como `List<T>` o `Range` (paralelo a Stmt::For), bindea
        // `var: T` en un scope dedicado, valida `filter: Bool` y tipa
        // `expr: U`. Devuelve `List<U>`. El scope se descarta al salir
        // — `var` no escapa al caller, consistente con la decisión
        // estilo Python del evaluator.
        Expr::ListComp { expr, var, iter, filter, span } => {
            let iter_ty = infer_expr(ctx, iter);
            // El tipo del var sale del iter.
            let var_ty = match iter_ty.base() {
                Type::List(t) => (**t).clone(),
                Type::Range => Type::Int,
                // Any pasa silencioso (gradual) — paralelo al for.
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(iter.span(), format!(
                        "list comprehension necesita un iterable (`List` o `Range`), recibió `{}`",
                        other.display(ctx.types)
                    ));
                    Type::Any
                }
            };
            ctx.push_scope();
            // El span del binding apunta al span de la comprehension
            // (sin span propio para `var` en el AST — deuda S1).
            ctx.declare_var(var.clone(), var_ty, *span);
            // Filter opcional: debe tipar `Bool` (Any pasa por gradual).
            if let Some(f) = filter {
                let f_ty = infer_expr(ctx, f);
                if !is_compatible(&f_ty, &Type::Bool) {
                    ctx.error_at(f.span(), format!(
                        "el filtro `if` de la list comprehension debe ser `Bool`, recibió `{}`",
                        f_ty.display(ctx.types)
                    ));
                }
            }
            let elem_ty = infer_expr(ctx, expr);
            ctx.pop_scope();
            Type::List(Box::new(elem_ty))
        }

        Expr::Map(pairs, _) => {
            if pairs.is_empty() {
                return Type::Map(Box::new(Type::Any), Box::new(Type::Any));
            }
            // Sintetizamos por el primer par. Mezcla de tipos cae a Any.
            let (fk, fv) = (infer_expr(ctx, &pairs[0].0), infer_expr(ctx, &pairs[0].1));
            let mut k_same = true;
            let mut v_same = true;
            for (k, v) in &pairs[1..] {
                let kt = infer_expr(ctx, k);
                let vt = infer_expr(ctx, v);
                if !is_compatible(&kt, &fk) {
                    k_same = false;
                }
                if !is_compatible(&vt, &fv) {
                    v_same = false;
                }
            }
            Type::Map(
                Box::new(if k_same { fk } else { Type::Any }),
                Box::new(if v_same { fv } else { Type::Any }),
            )
        }

        Expr::Range { start, end, .. } => {
            // Start y end deben ser Int (lo es en el evaluator). El
            // span del error apunta al extremo problemático para
            // distinguir cuál de los dos.
            for (label, e) in [("inicio", start.as_ref()), ("fin", end.as_ref())] {
                let t = infer_expr(ctx, e);
                if !is_compatible(&t, &Type::Int) {
                    ctx.error_at(e.span(), format!(
                        "{} del rango debe ser Int, recibió `{}`",
                        label,
                        t.display(ctx.types)
                    ));
                }
            }
            Type::Range
        }

        Expr::StructLit { type_name, fields, span } => {
            // Sintetiza Nominal si el nombre del tipo está declarado.
            // Validar campos contra el `type` declarado: faltantes,
            // extras, tipos incompatibles.
            let id = match ctx.types.lookup(type_name) {
                Some(id) => id,
                None => {
                    // resolve_program ya reporta tipos desconocidos
                    // como campos/anotaciones; un StructLit con
                    // nombre inexistente sí es propio del checker.
                    ctx.error_at(*span, format!(
                        "no existe el tipo `{}` para instanciar",
                        type_name
                    ));
                    // Igual evaluamos los valores para detectar errores
                    // adentro.
                    for (_, v) in fields {
                        let _ = infer_expr(ctx, v);
                    }
                    return Type::Any;
                }
            };
            // Comparamos contra los campos resueltos del nominal.
            let declared = ctx.types.info(id).fields.clone();
            // Inferir tipos provistos (siempre, para que warnings adentro
            // afloren).
            let mut provided_types: Vec<(String, Type, Span)> = Vec::new();
            for (n, v) in fields {
                let t = infer_expr(ctx, v);
                provided_types.push((n.clone(), t, v.span()));
            }
            if let Some(declared) = declared {
                // Extras
                let declared_names: std::collections::HashSet<&str> =
                    declared.iter().map(|f| f.name.as_str()).collect();
                for (n, _, fs) in &provided_types {
                    if !declared_names.contains(n.as_str()) {
                        ctx.error_at(*fs, format!(
                            "el tipo `{}` no tiene un campo llamado `{}`",
                            type_name, n
                        ));
                    }
                }
                // Faltantes y compatibilidad de los provistos.
                let provided_map: std::collections::HashMap<&str, (&Type, Span)> = provided_types
                    .iter()
                    .map(|(n, t, fs)| (n.as_str(), (t, *fs)))
                    .collect();
                for f in &declared {
                    match provided_map.get(f.name.as_str()) {
                        Some((actual, fs)) if !is_compatible(actual, &f.type_) => {
                            ctx.error_at(*fs, format!(
                                "el campo `{}.{}` espera `{}`, recibió `{}`",
                                type_name,
                                f.name,
                                f.type_.display(ctx.types),
                                actual.display(ctx.types)
                            ));
                        }
                        Some(_) => {}
                        None => {
                            // Faltante: válido si nullable o si el
                            // evaluator espera default (validado en
                            // resolve_program).
                            //
                            // En el caso nullable, no hay error. En el
                            // resto, podríamos alertar — pero el
                            // evaluator emite su propio error en
                            // runtime cuando falta un campo sin
                            // default. Para no duplicar mensajes,
                            // dejamos esto pasar en 5.3.1.
                        }
                    }
                }
            }
            Type::Nominal(id)
        }

        Expr::Field { object, field, .. } => {
            let obj_ty = infer_expr(ctx, object);
            match &obj_ty {
                Type::Nominal(id) => {
                    let info = ctx.types.info(*id);
                    if let Some(declared) = &info.fields {
                        if let Some(f) = declared.iter().find(|f| f.name == *field) {
                            return f.type_.clone();
                        }
                        // Campo desconocido. En 5.3.4 cuando entren
                        // métodos puede ser legítimo (el "field"
                        // sintáctico es un método). Por ahora silencio
                        // si está dentro de un Call (lo handlea
                        // infer_call), y warning si no — pero no
                        // sabemos el contexto acá. Devolvemos Any.
                        return Type::Any;
                    }
                    Type::Any
                }
                // 8.4: field access sobre `PyAny` da `PyAny`. Cubre
                // chains como `os.path` / `os.path.sep` / `engine.url`
                // — todos opacos hasta que el usuario anote
                // explícitamente. El chequeo runtime via getattr ya
                // tira AttributeError claro si el field no existe.
                Type::PyAny => Type::PyAny,
                // Cualquier otro receptor: 5.3.4 lo cubre con métodos
                // built-in. Por ahora Any.
                _ => Type::Any,
            }
        }

        Expr::Call { callee, args, span } => {
            // Camino de método: `obj.method(args)` ↔ callee
            // sintáctico es `Expr::Field`. Despachamos por
            // `(tipo del receptor, nombre del método)` contra la
            // tabla de built-ins (5.3.4) en lugar de pasar por la
            // ruta general — la ruta general no puede modelar
            // signatures paramétricas como `List<T>.map`.
            if let Expr::Field { object, field, .. } = callee.as_ref() {
                let obj_ty = infer_expr(ctx, object);
                let args_ty: Vec<Type> =
                    args.iter().map(|a| infer_expr(ctx, a)).collect();
                // 8.4: receptor PyAny — el método se invoca cruzando
                // a Python via dispatch_method (8.1.4). El runtime
                // envuelve TODO call Python en `Result<T>` (8.3); el
                // checker refleja eso: la llamada tipa como
                // `Result<Any>`, no `Any`. Esto activa la regla de
                // exhaustividad sobre Result (5.3.3) y la restricción
                // del operador `?` (5.3.2/5.3.3) — el usuario es
                // forzado a manejar la falla estáticamente, igual
                // que cualquier `Result<T>` nativo.
                if matches!(obj_ty, Type::PyAny) {
                    return Type::Result { ok: Box::new(Type::Any), err: Box::new(Type::Str) };
                }
                return match infer_method_call(ctx, &obj_ty, field, &args_ty, *span) {
                    Some(ret) => ret,
                    // Receptor que no entendemos (Nominal sin métodos
                    // custom, Module via import, Any): seguimos en
                    // modo gradual sin chequear nada de la llamada.
                    None => Type::Any,
                };
            }
            // Sintetizamos siempre callee y args para que afloren
            // errores adentro. Después validamos aridad y tipos según
            // lo que sea el callee.
            let callee_ty = infer_expr(ctx, callee);
            let args_ty: Vec<Type> = args.iter().map(|a| infer_expr(ctx, a)).collect();
            match callee_ty {
                // Gradual: callee de tipo desconocido no se chequea.
                Type::Any => Type::Any,
                // 8.4: callee es un PyObject opaco — el call cruza a
                // Python y vuelve envuelto en `Result<T>` (decisión
                // 8.3). Cubre `let f = math.sqrt; f(25.0)` (callee
                // resuelto por Ident después del field access).
                Type::PyAny => Type::Result { ok: Box::new(Type::Any), err: Box::new(Type::Str) },
                Type::Function { params, ret } => {
                    let label = describe_callee(callee);
                    if args.len() != params.len() {
                        ctx.error_at(*span, format!(
                            "{} espera {} argumento(s), recibió {}",
                            label,
                            params.len(),
                            args.len()
                        ));
                    } else {
                        for (i, (actual, expected)) in
                            args_ty.iter().zip(params.iter()).enumerate()
                        {
                            if !is_compatible(actual, expected) {
                                // Apuntamos al argumento concreto —
                                // mejor pista que el `(` del Call.
                                ctx.error_at(args[i].span(), format!(
                                    "{}: el argumento {} espera `{}`, recibió `{}`",
                                    label,
                                    i + 1,
                                    expected.display(ctx.types),
                                    actual.display(ctx.types)
                                ));
                            }
                        }
                    }
                    *ret
                }
                other => {
                    ctx.error_at(callee.span(), format!(
                        "`{}` no es una función",
                        other.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }
        Expr::FnExpr { params, body, span } => {
            // Walkeamos el body con un scope nuevo y los params
            // bindeados (con su tipo declarado o `Any` si la
            // anotación faltó). El tipo del FnExpr es `Function`;
            // 5.3.5 infiere el `ret` recolectando los tipos de los
            // `Stmt::Return` del body y unificándolos con `lub`.
            // Empujamos `Any` al return_stack porque sin anotación
            // no podemos validar contra qué — los returns se
            // recolectan, no se chequean.
            //
            // `await_stack` pushea `false`: el lenguaje no soporta
            // `async fn(...)` anónimas hoy. `.await` adentro de un
            // FnExpr (incluso si está anidado adentro de una `async
            // fn`) es error — el closure sync no puede await-ear.
            ctx.push_scope();
            ctx.return_stack.push(Type::Any);
            ctx.inferred_returns.push(Vec::new());
            ctx.in_http_handler.push(false);
            ctx.await_stack.push(false);
            // R.2.4 (F3): break/continue NO escapan FnExpr (closures).
            let saved_loop_depth = ctx.loop_depth;
            ctx.loop_depth = 0;
            let param_types: Vec<Type> = params
                .iter()
                .map(|p| ann_to_type(p.type_.as_ref(), ctx.types))
                .collect();
            for (p, t) in params.iter().zip(param_types.iter()) {
                // Sin span propio en `Param` (deuda S1), aproximamos
                // con el span del FnExpr contenedor: hover/go-to-def
                // sobre un uso del param salta al `fn(...)` que lo
                // declara.
                ctx.declare_var(p.name.clone(), t.clone(), *span);
            }
            check_block(ctx, body);
            ctx.loop_depth = saved_loop_depth;
            let returns = ctx.inferred_returns.pop().unwrap_or_default();
            ctx.return_stack.pop();
            ctx.in_http_handler.pop();
            ctx.await_stack.pop();
            ctx.pop_scope();
            let ret = unify_returns(&returns);
            Type::Function {
                params: param_types,
                ret: Box::new(ret),
            }
        }
        Expr::Slice { object, start, end, span, .. } => {
            let obj_ty = infer_expr(ctx, object);
            for (name, e) in [("start", start), ("end", end)] {
                if let Some(inner) = e {
                    let t = infer_expr(ctx, inner);
                    if !is_compatible(&t, &Type::Int) {
                        ctx.error_at(inner.span(), format!(
                            "el `{}` de un slice debe ser Int, recibió `{}`",
                            name,
                            t.display(ctx.types),
                        ));
                    }
                }
            }
            match obj_ty.base() {
                Type::List(t) => Type::List(Box::new((**t).clone())),
                Type::Str => Type::Str,
                Type::Any | Type::Nominal(_) => Type::Any,
                other => {
                    ctx.error_at(*span, format!(
                        "el tipo `{}` no soporta slicing con `[..]`",
                        other.display(ctx.types),
                    ));
                    Type::Any
                }
            }
        }

        Expr::Index { object, index, span } => {
            let obj_ty = infer_expr(ctx, object);
            let idx_ty = infer_expr(ctx, index);
            match obj_ty.base() {
                Type::List(t) => {
                    if !is_compatible(&idx_ty, &Type::Int) {
                        ctx.error_at(index.span(), format!(
                            "el índice de una `List` debe ser Int, recibió `{}`",
                            idx_ty.display(ctx.types)
                        ));
                    }
                    (**t).clone()
                }
                Type::Map(k, v) => {
                    if !is_compatible(&idx_ty, k) {
                        ctx.error_at(index.span(), format!(
                            "el índice de un `Map<{}, {}>` debe ser `{}`, recibió `{}`",
                            k.display(ctx.types),
                            v.display(ctx.types),
                            k.display(ctx.types),
                            idx_ty.display(ctx.types)
                        ));
                    }
                    (**v).clone()
                }
                Type::Str => {
                    // I.1 (mini-tanda I) — `s[i]` devuelve el i-ésimo
                    // char como `Str` de un char (Fitz no tiene Char).
                    // Indexación por CHAR, no por byte (consistente con
                    // `s.len()` que cuenta chars). Negativos soportados:
                    // `s[-1]` = último.
                    if !is_compatible(&idx_ty, &Type::Int) {
                        ctx.error_at(index.span(), format!(
                            "el índice de un `Str` debe ser Int, recibió `{}`",
                            idx_ty.display(ctx.types)
                        ));
                    }
                    Type::Str
                }
                // Gradual: Any y Nominal no chequean. Nominal con
                // operador `[]` es deuda (custom indexers no existen);
                // Any es el escape habitual.
                Type::Any | Type::Nominal(_) => Type::Any,
                other => {
                    ctx.error_at(*span, format!(
                        "el tipo `{}` no soporta indexing con `[]`",
                        other.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }
        Expr::Match { value, arms, span } => {
            let scrutinee = infer_expr(ctx, value);
            // Tipo del binding según el patrón. Para `Ok(x)` con
            // scrutinee `Result<T>`, x es T. Para `Err(e)` el error
            // está fijado en Str. Para Ident es el scrutinee
            // completo. Para literales/wildcard/range no hay bind.
            let mut first: Option<Type> = None;
            for arm in arms {
                ctx.push_scope();
                // Sin span propio en `MatchArm`/`Pattern` (deuda S1),
                // usamos el span del body como aproximación del
                // `def_span` del binding — el más cercano del arm en
                // el AST actual. go-to-def sobre el uso del binding
                // salta al body del arm.
                bind_pattern(ctx, &arm.pattern, &scrutinee, arm.body.span());
                // R.2.2 — el guard tipa adentro del scope del binding.
                // Debe sintetizar Bool; otro tipo es error.
                if let Some(guard_expr) = &arm.guard {
                    let guard_ty = infer_expr(ctx, guard_expr);
                    if !matches!(guard_ty, Type::Bool | Type::Any) {
                        ctx.error_at(guard_expr.span(), format!(
                            "el guard de un arm debe ser Bool, recibí {}",
                            guard_ty.display(ctx.types)
                        ));
                    }
                }
                let t = infer_expr(ctx, &arm.body);
                ctx.pop_scope();
                if first.is_none() {
                    first = Some(t);
                }
            }
            // Exhaustividad: solo la exigimos cuando el scrutinee es
            // `Result<T>` (puro, no nullable). Otros tipos no tienen
            // semántica de "variantes" para Fitz todavía.
            if matches!(scrutinee, Type::Result { .. }) {
                check_result_match_exhaustiveness(ctx, arms, *span);
            }
            first.unwrap_or(Type::Any)
        }
        Expr::Ok(inner, _) => {
            // Mini-tanda Re+ — sin contexto, E queda como `Any` (el
            // checker no sabe qué Err puede aparecer luego). El LUB
            // contra otros Results refinará E si se construyen Err en
            // el mismo flujo. La anotación destino (`-> Result<T, E>`)
            // gana sobre el inferido.
            let t = infer_expr(ctx, inner);
            Type::Result { ok: Box::new(t), err: Box::new(Type::Any) }
        }
        Expr::Err(inner, _) => {
            // Mini-tanda Re+ — el tipo del E ahora se infiere desde el
            // value. T queda Any sin contexto; el LUB/anotación destino
            // lo refinará.
            let e_ty = infer_expr(ctx, inner);
            Type::Result { ok: Box::new(Type::Any), err: Box::new(e_ty) }
        }
        Expr::Await(inner, span) => {
            // 6.2: semántica completa del checker.
            //
            // Regla 1 — contexto async. `.await` solo es legal adentro
            // de una `async fn` Fitz. Top-level y FnExpr (closures
            // sync) son inválidos. `await_stack.last()` nos dice si la
            // fn más cercana es async. Si no, error con mensaje
            // claro pero igual seguimos sintetizando un tipo para no
            // confundir al usuario con errores cascada.
            //
            // Regla 2 — operando `Future<T>`. Lo que `.await` desempaca
            // tiene que ser un `Future<T>` (o `Any` para escape
            // gradual). Cualquier otro tipo concreto es error.
            let operand_ty = infer_expr(ctx, inner);

            // Top-level (stack vacío) cuenta como contexto async válido
            // — el evaluator arranca el runtime tokio ahí y el codegen
            // emite `#[tokio::main] async fn main()` cuando el programa
            // usa async. Solo rechazamos cuando estamos adentro de una
            // fn sync explícita (`Some(false)`): FnDef no-async o
            // FnExpr (los closures no soportan async todavía).
            if matches!(ctx.await_stack.last(), Some(false)) {
                ctx.error_at(
                    *span,
                    "`.await` solo es válido adentro de `async fn` o a nivel top-level".to_string(),
                );
            }

            match &operand_ty {
                Type::Any => Type::Any,
                Type::Future(inner_ty) => (**inner_ty).clone(),
                // Fase 8.7.3: `.await` sobre `Result<PyAny>` o
                // `Result<Any>` (lo que el call Python sintetiza per
                // 8.4 → 8.3) NO está soportado directo en intérprete
                // (el evaluator rechaza con "se esperaba Future").
                // El patrón canónico es `<py_call>?.await`: el `?`
                // desempaca el Result a Future, y el .await opera
                // sobre el Future. Acá NO agregamos rama para
                // `Result<...>` — sigue siendo error de tipo si el
                // usuario omite el `?`. La rama para `PyAny` solo
                // cubre el caso del codegen 8.7.3 donde el inner del
                // await después de `?` es PyAny (`<call>?.await` con
                // el helper combinado).
                Type::PyAny => Type::Any,
                other => {
                    ctx.error_at(*span, format!(
                        "`.await` solo aplica a `Future<T>`, recibió `{}`",
                        other.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }

        Expr::Try(inner, span) => {
            let operand_ty = infer_expr(ctx, inner);
            match &operand_ty {
                // Gradual: operando de tipo desconocido no se chequea.
                // Cubre el caso típico de método built-in (callee
                // Field) que todavía devuelve Any hasta 5.3.4.
                Type::Any => Type::Any,
                Type::Result { ok: inner_ty, err: _ } => {
                    // Si estamos adentro de una función con
                    // return_type concreto, exigimos que sea Result —
                    // el `?` propaga un `Err(_)` vía `return`, así que
                    // la fn contenedora tiene que poder recibirlo.
                    // Fn sin return_type (Any) o top-level no chequea.
                    if let Some(expected) = ctx.return_stack.last().cloned() {
                        let is_ok = matches!(expected, Type::Any | Type::Result { .. });
                        if !is_ok {
                            ctx.error_at(*span, format!(
                                "el operador `?` solo puede usarse adentro de una función que retorne `Result<...>`; esta retorna `{}`",
                                expected.display(ctx.types)
                            ));
                        }
                    }
                    (**inner_ty).clone()
                }
                other => {
                    ctx.error_at(*span, format!(
                        "el operador `?` requiere un `Result`, recibió `{}`",
                        other.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }

        // Fase 9.0.1 (F15): `Expr::Error` solo lo produce
        // `parse_with_recovery`. El checker lo trata como `Type::Any`
        // y NO emite errores derivados — el error real ya está en la
        // lista de `recovered_errors` del parser. Silencioso es la
        // política correcta: si el LSP corre el checker sobre un AST
        // con Error nodes, no queremos cascada de errores derivados
        // sobre el mismo punto.
        Expr::Error(_) => Type::Any,
    }
}

/// Etiqueta amigable para el callee de un `Call`. Aparece en los
/// errores de aridad y de tipos de argumento. Cuando podemos
/// identificar el nombre (Ident o Field), lo usamos; si no, una
/// etiqueta genérica.
fn describe_callee(callee: &Expr) -> String {
    match callee {
        Expr::Ident(name, _) => format!("la función `{}`", name),
        Expr::Field { field, .. } => format!("el método `{}`", field),
        _ => "esta llamada".into(),
    }
}

/// "Least upper bound" pragmático para sintetizar el tipo de
/// retorno de una función cuyo body tiene varios `return` con
/// tipos diferentes. No es un lattice formal: prioriza preservar
/// información útil (Result<X> + Result<Any> = Result<X>) sobre
/// la pureza teórica.
///
/// Reglas:
///   - `a == b` → `a`.
///   - Cualquiera Any → el otro (Any cede al concreto).
///   - Int + Float → Float (coerción).
///   - Null + T → `T?` (rama opcional).
///   - T + T? → `T?`.
///   - Generics (List/Map/Result/Nullable) → recursión.
///   - Mix arbitrario → Any.
fn lub(a: &Type, b: &Type) -> Type {
    if a == b {
        return a.clone();
    }
    if matches!(a, Type::Any) {
        return b.clone();
    }
    if matches!(b, Type::Any) {
        return a.clone();
    }
    // Coerción Int↔Float.
    if (matches!(a, Type::Int) && matches!(b, Type::Float))
        || (matches!(a, Type::Float) && matches!(b, Type::Int))
    {
        return Type::Float;
    }
    // Null + T → T? (y simétrico).
    if matches!(a, Type::Null) {
        return Type::Nullable(Box::new(b.clone()));
    }
    if matches!(b, Type::Null) {
        return Type::Nullable(Box::new(a.clone()));
    }
    // T + T? → T? (y simétrico): si el inner del nullable es igual
    // al otro, ya es lo mejor que tenemos.
    if let Type::Nullable(inner) = a {
        if **inner == *b {
            return a.clone();
        }
    }
    if let Type::Nullable(inner) = b {
        if **inner == *a {
            return b.clone();
        }
    }
    // Generics recursivos.
    match (a, b) {
        (Type::List(ai), Type::List(bi)) => Type::List(Box::new(lub(ai, bi))),
        (Type::Map(ak, av), Type::Map(bk, bv)) => {
            Type::Map(Box::new(lub(ak, bk)), Box::new(lub(av, bv)))
        }
        // Mini-tanda Re+: lub recursivo en ambos lados (ok y err).
        (Type::Result { ok: a_ok, err: a_err }, Type::Result { ok: b_ok, err: b_err }) => {
            Type::Result {
                ok: Box::new(lub(a_ok, b_ok)),
                err: Box::new(lub(a_err, b_err)),
            }
        }
        (Type::Future(ai), Type::Future(bi)) => Type::Future(Box::new(lub(ai, bi))),
        (Type::Nullable(ai), Type::Nullable(bi)) => Type::Nullable(Box::new(lub(ai, bi))),
        _ => Type::Any,
    }
}

/// Unifica los tipos de los `return` recolectados durante el
/// walkeo del body de una función. Si la lista está vacía, la
/// función no retorna explícitamente y devolvemos `Null` (matchea
/// la semántica del evaluator: una fn que termina sin `return`
/// produce `Value::Null`).
fn unify_returns(types: &[Type]) -> Type {
    if types.is_empty() {
        return Type::Null;
    }
    let mut result = types[0].clone();
    for t in &types[1..] {
        result = lub(&result, t);
    }
    result
}

/// Despacho del checker para método built-in. Recibe el tipo del
/// receptor (`xs` en `xs.map(f)`), el nombre del método, y los
/// tipos ya inferidos de los argumentos. Devuelve `Some(ret)` con
/// el tipo del resultado, o `None` cuando el receptor no entra en
/// el dispatch built-in (Nominal sin métodos custom todavía,
/// Module via import — ambos modelados como `Any` o `Nominal`).
///
/// Para los casos `None`, el caller continúa en modo gradual
/// (devuelve `Any` sin chequear aridad/tipos). Para los casos
/// soportados, las violaciones se reportan vía `ctx.error(...)`
/// pero el dispatch siempre devuelve `Some(...)` con el ret
/// inferido (los errores no propagan, se acumulan).
///
/// Convención: `T` siempre proviene del receptor concreto en este
/// call site. `List<Int>.map(f)` y `List<Str>.map(f)` instancian
/// distinto.
fn infer_method_call(
    ctx: &mut CheckCtx,
    receiver_ty: &Type,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Option<Type> {
    // Pelamos un Nullable: `xs?.map(...)` cae cuando el `?` ya
    // desempacó, así que acá raramente vemos Nullable. Por las
    // dudas, lo dejamos transparente.
    let recv = receiver_ty.base();
    match recv {
        Type::List(t) => {
            let t = (**t).clone();
            Some(infer_list_method(ctx, &t, method, args_ty, span))
        }
        Type::Map(k, v) => {
            let k = (**k).clone();
            let v = (**v).clone();
            Some(infer_map_method(ctx, &k, &v, method, args_ty, span))
        }
        Type::Str => Some(infer_str_method(ctx, method, args_ty, span)),
        // R.3 — métodos custom sobre nominal. Buscamos en
        // `NominalInfo.methods`. Si existe: validamos aridad + tipos
        // de args, devolvemos el ret type (o `Future<T>` si async).
        // Si no existe: gradual (None), igual que Any.
        Type::Nominal(id) => {
            let info = ctx.types.info(*id);
            if let Some(nm) = info.methods.iter().find(|m| m.name == method).cloned() {
                // Aridad.
                if args_ty.len() != nm.params.len() {
                    ctx.error_at(span, format!(
                        "el método `{}.{}` espera {} argumento(s), recibió {}",
                        info.name, method, nm.params.len(), args_ty.len()
                    ));
                    let ret = if nm.is_async { Type::Future(Box::new(nm.ret)) } else { nm.ret };
                    return Some(ret);
                }
                // Tipos de args (compatible_with semánticamente).
                for (i, (got, expected)) in args_ty.iter().zip(nm.params.iter()).enumerate() {
                    if !is_compatible(got, expected) {
                        ctx.error_at(span, format!(
                            "el método `{}.{}` arg #{}: esperaba `{}`, recibió `{}`",
                            info.name,
                            method,
                            i,
                            expected.display(ctx.types),
                            got.display(ctx.types)
                        ));
                    }
                }
                let ret = if nm.is_async { Type::Future(Box::new(nm.ret)) } else { nm.ret };
                Some(ret)
            } else {
                // Método inexistente sobre nominal → gradual (Any).
                // El evaluator emitirá error en runtime; el codegen
                // también. Acá no levantamos para no duplicar.
                None
            }
        }
        // Mini-tanda Ir — métodos sobre Range. Range expone el subset
        // de iteradores que tiene sentido (enumerate/zip/chain) + `len`.
        // El evaluator materializa el Range a List<Int> y delega.
        Type::Range => Some(infer_range_method(ctx, method, args_ty, span)),
        // Gradual: no chequeamos sobre Any (no sabemos nada).
        Type::Any => None,
        other => {
            // Tipos sin métodos built-in: `42.foo()` y similares.
            // El evaluator también corta, acá nos adelantamos con
            // mensaje específico.
            ctx.error_at(span, format!(
                "el tipo `{}` no tiene el método `{}`",
                other.display(ctx.types),
                method
            ));
            Some(Type::Any)
        }
    }
}

/// Mini-tanda Ir — signatures de métodos built-in sobre `Range`. El
/// Range conceptualmente es un `List<Int>` lazy; los métodos coinciden
/// con los de `List<Int>` para enumerate/zip/chain, más `len`.
fn infer_range_method(
    ctx: &mut CheckCtx,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Type {
    match method {
        "enumerate" => {
            check_method_arity(ctx, "enumerate", args_ty, 0, span);
            Type::List(Box::new(Type::Tuple(vec![Type::Int, Type::Int])))
        }
        "zip" => {
            if !check_method_arity(ctx, "zip", args_ty, 1, span) {
                return Type::List(Box::new(Type::Tuple(vec![Type::Int, Type::Any])));
            }
            let u = match args_ty[0].base() {
                Type::List(inner) => (**inner).clone(),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(span, format!(
                        "`Range.zip()` espera `List<U>`, recibió `{}`",
                        other.display(ctx.types),
                    ));
                    Type::Any
                }
            };
            Type::List(Box::new(Type::Tuple(vec![Type::Int, u])))
        }
        "chain" => {
            if !check_method_arity(ctx, "chain", args_ty, 1, span) {
                return Type::List(Box::new(Type::Int));
            }
            match args_ty[0].base() {
                Type::List(inner) => {
                    if !is_compatible(inner, &Type::Int) {
                        ctx.error_at(span, format!(
                            "`Range.chain()` espera `List<Int>`, recibió `List<{}>`",
                            inner.display(ctx.types),
                        ));
                    }
                }
                Type::Any => {}
                other => {
                    ctx.error_at(span, format!(
                        "`Range.chain()` espera `List<Int>`, recibió `{}`",
                        other.display(ctx.types),
                    ));
                }
            }
            Type::List(Box::new(Type::Int))
        }
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        _ => {
            ctx.error_at(span, format!(
                "`Range` no tiene el método `{}` (hoy: enumerate/zip/chain/len)",
                method,
            ));
            Type::Any
        }
    }
}

/// Valida aridad de un método built-in. Devuelve `true` si la
/// aridad coincide (para que el caller pueda saltarse validaciones
/// extra sobre argumentos que no existen). Si falla, acumula error
/// y devuelve `false`.
fn check_method_arity(
    ctx: &mut CheckCtx,
    method: &str,
    args_ty: &[Type],
    expected: usize,
    span: Span,
) -> bool {
    if args_ty.len() != expected {
        ctx.error_at(span, format!(
            "el método `{}` espera {} argumento(s), recibió {}",
            method,
            expected,
            args_ty.len()
        ));
        false
    } else {
        true
    }
}

/// Valida un callback unario (`fn(T) -> U`). Devuelve el `U`
/// inferido del callback, o `Any` si el callback es Any o no
/// validable. Si `expected_ret` es `Some(B)`, además exige que U
/// sea compatible con B (caso típico: `.filter()` exige `Bool`).
fn check_unary_callback(
    ctx: &mut CheckCtx,
    cb: &Type,
    elem_ty: &Type,
    method: &str,
    expected_ret: Option<&Type>,
    span: Span,
) -> Type {
    match cb {
        Type::Any => Type::Any,
        Type::Function { params, ret } => {
            if params.len() != 1 {
                ctx.error_at(span, format!(
                    "la callback de `.{}()` debe tomar 1 argumento, recibió {}",
                    method,
                    params.len()
                ));
                return (**ret).clone();
            }
            // El param del callback tiene que poder recibir un T
            // (el tipo de los elementos). Si el callback declaró un
            // tipo concreto incompatible, error.
            if !is_compatible(elem_ty, &params[0]) {
                ctx.error_at(span, format!(
                    "la callback de `.{}()` recibe elementos `{}` pero su parámetro es `{}`",
                    method,
                    elem_ty.display(ctx.types),
                    params[0].display(ctx.types)
                ));
            }
            if let Some(expected) = expected_ret {
                if !is_compatible(ret, expected) {
                    ctx.error_at(span, format!(
                        "la callback de `.{}()` debe devolver `{}`, devuelve `{}`",
                        method,
                        expected.display(ctx.types),
                        ret.display(ctx.types)
                    ));
                }
            }
            (**ret).clone()
        }
        other => {
            ctx.error_at(span, format!(
                "la callback de `.{}()` debe ser una función, recibió `{}`",
                method,
                other.display(ctx.types)
            ));
            Type::Any
        }
    }
}

fn infer_list_method(
    ctx: &mut CheckCtx,
    t: &Type,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Type {
    match method {
        "push" => {
            check_method_arity(ctx, "push", args_ty, 1, span);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, t) {
                    ctx.error_at(span, format!(
                        "`push` sobre `List<{}>` recibió `{}`",
                        t.display(ctx.types),
                        arg.display(ctx.types)
                    ));
                }
            }
            Type::Null
        }
        "pop" => {
            check_method_arity(ctx, "pop", args_ty, 0, span);
            t.clone()
        }
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        "map" => {
            if !check_method_arity(ctx, "map", args_ty, 1, span) {
                return Type::List(Box::new(Type::Any));
            }
            let u = check_unary_callback(ctx, &args_ty[0], t, "map", None, span);
            Type::List(Box::new(u))
        }
        "filter" => {
            if !check_method_arity(ctx, "filter", args_ty, 1, span) {
                return Type::List(Box::new(t.clone()));
            }
            check_unary_callback(ctx, &args_ty[0], t, "filter", Some(&Type::Bool), span);
            Type::List(Box::new(t.clone()))
        }
        "find" => {
            if !check_method_arity(ctx, "find", args_ty, 1, span) {
                return Type::Result { ok: Box::new(t.clone()), err: Box::new(Type::Str) };
            }
            check_unary_callback(ctx, &args_ty[0], t, "find", Some(&Type::Bool), span);
            Type::Result { ok: Box::new(t.clone()), err: Box::new(Type::Str) }
        }
        // S.3 (mini-tanda S) — `sort`/`reverse` mutan in-place y
        // devuelven `Null`. `contains(v)` devuelve `Bool`. El
        // chequeo de "tipo comparable" para sort se hace en runtime
        // — el checker no rechaza `List<Any>.sort()` para preservar
        // el modelo gradual.
        "sort" | "reverse" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Null
        }
        "contains" => {
            if check_method_arity(ctx, "contains", args_ty, 1, span)
                && !is_compatible(&args_ty[0], t)
            {
                ctx.error_at(span, format!(
                    "`List<{}>.contains()` recibió `{}`",
                    t.display(ctx.types),
                    args_ty[0].display(ctx.types),
                ));
            }
            Type::Bool
        }
        // Mini-tanda It — `enumerate()` devuelve `List<(Int, T)>` con
        // pares (índice, elemento). Encaja natural con tuple
        // destructuring del for (Md): `for (i, x) in xs.enumerate()`.
        "enumerate" => {
            check_method_arity(ctx, "enumerate", args_ty, 0, span);
            Type::List(Box::new(Type::Tuple(vec![Type::Int, t.clone()])))
        }
        // Mini-tanda It — `zip(ys)` empareja dos listas, truncando al
        // más corto. `ys: List<U>` con U arbitrario; devuelve
        // `List<(T, U)>`.
        "zip" => {
            if !check_method_arity(ctx, "zip", args_ty, 1, span) {
                return Type::List(Box::new(Type::Tuple(vec![t.clone(), Type::Any])));
            }
            let u = match args_ty[0].base() {
                Type::List(inner) => (**inner).clone(),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(span, format!(
                        "`List<{}>.zip()` espera `List<U>`, recibió `{}`",
                        t.display(ctx.types),
                        other.display(ctx.types),
                    ));
                    Type::Any
                }
            };
            Type::List(Box::new(Type::Tuple(vec![t.clone(), u])))
        }
        // Mini-tanda It — `chain(ys)` concatena. `ys` debe ser
        // `List<T>` (mismo tipo). Devuelve `List<T>`.
        "chain" => {
            if !check_method_arity(ctx, "chain", args_ty, 1, span) {
                return Type::List(Box::new(t.clone()));
            }
            match args_ty[0].base() {
                Type::List(inner) => {
                    if !is_compatible(inner, t) {
                        ctx.error_at(span, format!(
                            "`List<{}>.chain()` espera `List<{}>`, recibió `List<{}>`",
                            t.display(ctx.types),
                            t.display(ctx.types),
                            inner.display(ctx.types),
                        ));
                    }
                }
                Type::Any => {}
                other => {
                    ctx.error_at(span, format!(
                        "`List<{}>.chain()` espera `List<{}>`, recibió `{}`",
                        t.display(ctx.types),
                        t.display(ctx.types),
                        other.display(ctx.types),
                    ));
                }
            }
            Type::List(Box::new(t.clone()))
        }
        // Mini-tanda Mb — `flatten()` requiere `List<List<U>>` y
        // devuelve `List<U>`. Si T no es List (o no Any), error claro.
        "flatten" => {
            if !check_method_arity(ctx, "flatten", args_ty, 0, span) {
                return Type::Any;
            }
            match t {
                Type::List(inner) => Type::List(inner.clone()),
                Type::Any => Type::List(Box::new(Type::Any)),
                other => {
                    ctx.error_at(span, format!(
                        "`.flatten()` requiere `List<List<U>>`, el receptor es `List<{}>`",
                        other.display(ctx.types),
                    ));
                    Type::Any
                }
            }
        }
        // Mini-tanda Mb — `sort_by(cmp)`. El callback es `fn(T, T) -> Int`.
        // Muta in-place, devuelve Null (paralelo a `sort`).
        "sort_by" => {
            if !check_method_arity(ctx, "sort_by", args_ty, 1, span) {
                return Type::Null;
            }
            let cb_ty = &args_ty[0];
            match cb_ty {
                Type::Function { params, ret } => {
                    if params.len() != 2 {
                        ctx.error_at(span, format!(
                            "`.sort_by(cmp)` espera `fn(T, T) -> Int` (2 params); el callback tiene {} params",
                            params.len(),
                        ));
                    } else {
                        for (i, p) in params.iter().enumerate() {
                            if !is_compatible(p, t) && !is_compatible(t, p) {
                                ctx.error_at(span, format!(
                                    "`.sort_by(cmp)`: param[{}] del callback es `{}`, esperaba `{}`",
                                    i,
                                    p.display(ctx.types),
                                    t.display(ctx.types),
                                ));
                            }
                        }
                        if !is_compatible(ret, &Type::Int) {
                            ctx.error_at(span, format!(
                                "`.sort_by(cmp)`: el callback debe retornar `Int`, retorna `{}`",
                                ret.display(ctx.types),
                            ));
                        }
                    }
                }
                Type::Any => {
                    // Gradual: callback sin tipo concreto, no chequeo.
                }
                other => {
                    ctx.error_at(span, format!(
                        "`.sort_by(cmp)` espera `fn(T, T) -> Int`, recibió `{}`",
                        other.display(ctx.types),
                    ));
                }
            }
            Type::Null
        }
        _ => {
            ctx.error_at(span, format!(
                "`List<{}>` no tiene el método `{}`",
                t.display(ctx.types),
                method
            ));
            Type::Any
        }
    }
}

fn infer_map_method(
    ctx: &mut CheckCtx,
    k: &Type,
    v: &Type,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Type {
    match method {
        "get" => {
            check_method_arity(ctx, "get", args_ty, 1, span);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, k) {
                    ctx.error_at(span, format!(
                        "`get` sobre `Map<{}, {}>` espera una clave `{}`, recibió `{}`",
                        k.display(ctx.types),
                        v.display(ctx.types),
                        k.display(ctx.types),
                        arg.display(ctx.types)
                    ));
                }
            }
            Type::Result { ok: Box::new(v.clone()), err: Box::new(Type::Str) }
        }
        "has" => {
            check_method_arity(ctx, "has", args_ty, 1, span);
            if let Some(arg) = args_ty.first() {
                if !is_compatible(arg, k) {
                    ctx.error_at(span, format!(
                        "`has` sobre `Map<{}, {}>` espera una clave `{}`, recibió `{}`",
                        k.display(ctx.types),
                        v.display(ctx.types),
                        k.display(ctx.types),
                        arg.display(ctx.types)
                    ));
                }
            }
            Type::Bool
        }
        "keys" => {
            check_method_arity(ctx, "keys", args_ty, 0, span);
            Type::List(Box::new(k.clone()))
        }
        "values" => {
            check_method_arity(ctx, "values", args_ty, 0, span);
            Type::List(Box::new(v.clone()))
        }
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        _ => {
            ctx.error_at(span, format!(
                "`Map<{}, {}>` no tiene el método `{}`",
                k.display(ctx.types),
                v.display(ctx.types),
                method
            ));
            Type::Any
        }
    }
}

fn infer_str_method(
    ctx: &mut CheckCtx,
    method: &str,
    args_ty: &[Type],
    span: Span,
) -> Type {
    match method {
        "len" => {
            check_method_arity(ctx, "len", args_ty, 0, span);
            Type::Int
        }
        "upper" | "lower" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Str
        }
        // S.1 (mini-tanda S) — `contains`/`starts_with`/`ends_with`
        // toman un `Str` y devuelven `Bool`. Mismo shape para los 3.
        "contains" | "starts_with" | "ends_with" => {
            if check_method_arity(ctx, method, args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Str)
            {
                ctx.error_at(span, format!(
                    "`Str.{}()` espera `Str`, recibió `{}`",
                    method,
                    args_ty[0].display(ctx.types),
                ));
            }
            Type::Bool
        }
        // S.2 — manipulación de strings:
        "split" => {
            if check_method_arity(ctx, "split", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Str)
            {
                ctx.error_at(span, format!(
                    "`Str.split()` espera `Str` como separador, recibió `{}`",
                    args_ty[0].display(ctx.types),
                ));
            }
            Type::List(Box::new(Type::Str))
        }
        "trim" => {
            check_method_arity(ctx, "trim", args_ty, 0, span);
            Type::Str
        }
        // Mini-tanda Mb — trim_start / trim_end (variantes parciales).
        "trim_start" | "trim_end" => {
            check_method_arity(ctx, method, args_ty, 0, span);
            Type::Str
        }
        "replace" => {
            if check_method_arity(ctx, "replace", args_ty, 2, span) {
                for (i, name) in ["old", "new"].iter().enumerate() {
                    if !is_compatible(&args_ty[i], &Type::Str) {
                        ctx.error_at(span, format!(
                            "`Str.replace({}, ...)` espera `Str`, recibió `{}`",
                            name,
                            args_ty[i].display(ctx.types),
                        ));
                    }
                }
            }
            Type::Str
        }
        "repeat" => {
            if check_method_arity(ctx, "repeat", args_ty, 1, span)
                && !is_compatible(&args_ty[0], &Type::Int)
            {
                ctx.error_at(span, format!(
                    "`Str.repeat()` espera `Int`, recibió `{}`",
                    args_ty[0].display(ctx.types),
                ));
            }
            Type::Str
        }
        _ => {
            ctx.error_at(span, format!("`Str` no tiene el método `{}`", method));
            Type::Any
        }
    }
}

/// Actualiza las flags de cobertura `Result` walkando el patrón.
/// Para `Pattern::Or` recursea en cada sub-pattern (cualquier
/// branch que cubra Ok cuenta para Ok, etc.). R.2.1 (mini-fase R).
fn update_result_coverage(
    pat: &crate::ast::Pattern,
    has_ok: &mut bool,
    has_err: &mut bool,
    has_catchall: &mut bool,
) {
    use crate::ast::Pattern;
    match pat {
        Pattern::OkBinding(_) | Pattern::OkWildcard => *has_ok = true,
        Pattern::ErrBinding(_) | Pattern::ErrWildcard => *has_err = true,
        Pattern::Wildcard | Pattern::Ident(_) => *has_catchall = true,
        Pattern::Or(subs) => {
            for sub in subs {
                update_result_coverage(sub, has_ok, has_err, has_catchall);
            }
        }
        // Tuples (mini-tanda T): un Tuple pattern NO cubre Ok/Err
        // ni es catch-all sobre Result — tipa solo contra tuples.
        // No suma a la cobertura.
        Pattern::Tuple(_) => {}
        _ => {}
    }
}

/// Mini-tanda Md — Bindea un Pattern del `for` contra el tipo del
/// elemento del iter, declarando las vars correspondientes en el
/// scope actual. Cubre Ident/Wildcard/Tuple recursivo. Otros patterns
/// (literales, Ok/Err, Range) emiten error "patrón no admitido en
/// for".
fn bind_for_pattern_in_checker(
    ctx: &mut CheckCtx,
    pat: &crate::ast::Pattern,
    elem_ty: &Type,
    fallback_span: Span,
) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name) => {
            ctx.declare_var(name.clone(), elem_ty.clone(), fallback_span);
        }
        Pattern::Wildcard => {
            // Sin binding — el elemento se descarta.
        }
        Pattern::Tuple(subs) => {
            // El elem tiene que ser una tupla del mismo largo.
            match elem_ty.base() {
                Type::Tuple(item_tys) if item_tys.len() == subs.len() => {
                    for (sub, t) in subs.iter().zip(item_tys.iter()) {
                        bind_for_pattern_in_checker(ctx, sub, t, fallback_span);
                    }
                }
                Type::Any => {
                    // Gradual — bindeamos cada ident del pattern como Any.
                    for sub in subs {
                        bind_for_pattern_in_checker(ctx, sub, &Type::Any, fallback_span);
                    }
                }
                other => {
                    ctx.error_at(fallback_span, format!(
                        "tuple pattern del `for` espera una tupla de {} elementos, recibió `{}`",
                        subs.len(),
                        other.display(ctx.types)
                    ));
                }
            }
        }
        other => {
            ctx.error_at(fallback_span, format!(
                "patrón `{:?}` no admitido como variable de `for` (usá Ident, `_`, o tupla)",
                other
            ));
        }
    }
}

/// Mini-tanda Fm — valida que un `FormatSpec` sea aplicable al tipo
/// del expr interpolado. Reglas (paralelas a Python):
///   - `f`/`F`/`e`/`E`/`g`/`G`/`%` exigen Float o Int (promoción
///     transparente).
///   - `d`/`b`/`o`/`x`/`X`/`c` exigen Int (sin promoción de Float).
///   - `s` acepta Str (o cualquier tipo via Display).
///   - Sin `kind`, cualquier tipo es válido (uses Display por default).
///   - Alineación, fill, width, sign, alternate y precision son válidos
///     para cualquier tipo (precision con Str es longitud máxima).
fn validate_format_spec_for_type(
    ctx: &mut CheckCtx,
    spec: &crate::ast::FormatSpec,
    ty: &Type,
    span: Span,
) {
    use crate::ast::FormatKind;
    let Some(kind) = spec.kind else { return };
    let is_num_int = matches!(ty.base(), Type::Int | Type::Any);
    let is_num_float = matches!(ty.base(), Type::Int | Type::Float | Type::Any);
    let ok = match kind {
        FormatKind::FixedLower
        | FormatKind::FixedUpper
        | FormatKind::ExponentLower
        | FormatKind::ExponentUpper
        | FormatKind::GeneralLower
        | FormatKind::GeneralUpper
        | FormatKind::Percent => is_num_float,
        FormatKind::Decimal
        | FormatKind::Binary
        | FormatKind::Octal
        | FormatKind::HexLower
        | FormatKind::HexUpper
        | FormatKind::Char => is_num_int,
        FormatKind::String => true,
    };
    if !ok {
        ctx.error_at(span, format!(
            "format spec `{}` no es compatible con tipo `{}` (esperaba {})",
            kind.to_char(),
            ty.display(ctx.types),
            match kind {
                FormatKind::FixedLower
                | FormatKind::FixedUpper
                | FormatKind::ExponentLower
                | FormatKind::ExponentUpper
                | FormatKind::GeneralLower
                | FormatKind::GeneralUpper
                | FormatKind::Percent => "Float o Int",
                FormatKind::Decimal
                | FormatKind::Binary
                | FormatKind::Octal
                | FormatKind::HexLower
                | FormatKind::HexUpper
                | FormatKind::Char => "Int",
                FormatKind::String => "cualquier tipo",
            },
        ));
    }
}

/// Chequea exhaustividad de un `match` sobre `Result<T>`. Los arms
/// deben cubrir tanto `Ok` como `Err`, o tener un catch-all
/// (wildcard `_` o ident binding). Patrones literales/de rango
/// sobre un Result no aportan a la exhaustividad — son
/// "imposibles" pero no los rechazamos acá (sería un check
/// separado).
fn check_result_match_exhaustiveness(
    ctx: &mut CheckCtx,
    arms: &[crate::ast::MatchArm],
    span: Span,
) {
    let mut has_ok = false;
    let mut has_err = false;
    let mut has_catchall = false;
    for arm in arms {
        // R.2.2: arms con guard NO cuentan para exhaustividad
        // (paralelo a Rust). El guard puede fallar en runtime y
        // dejar el match incompleto.
        if arm.guard.is_some() {
            continue;
        }
        update_result_coverage(&arm.pattern, &mut has_ok, &mut has_err, &mut has_catchall);
    }
    if has_catchall || (has_ok && has_err) {
        return;
    }
    let missing = match (has_ok, has_err) {
        (true, false) => "`Err`",
        (false, true) => "`Ok`",
        _ => "`Ok` y `Err`",
    };
    ctx.error_at(span, format!(
        "match sobre `Result` no es exhaustivo: falta el caso {}",
        missing
    ));
}

/// Bindea las variables introducidas por un patrón en el scope
/// actual. `scrutinee` es el tipo del valor que se está matcheando.
/// `arm_span` es el span de aproximación que el binding usa como
/// `def_span` (Fase 9.x.3) — sin span propio en `Pattern` (deuda
/// S1), el caller pasa el span del body del MatchArm.
fn bind_pattern(
    ctx: &mut CheckCtx,
    pat: &crate::ast::Pattern,
    scrutinee: &Type,
    arm_span: Span,
) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name) => {
            ctx.declare_var(name.clone(), scrutinee.clone(), arm_span);
        }
        Pattern::OkBinding(name) => {
            // `Ok(x)` desempaca `Result<T>` — x es T.
            let inner = match scrutinee {
                Type::Result { ok: t, err: _ } => (**t).clone(),
                _ => Type::Any,
            };
            ctx.declare_var(name.clone(), inner, arm_span);
        }
        Pattern::ErrBinding(name) => {
            // Mini-tanda Re+ — `Err(e)` desempaca `Result<T, E>` y `e`
            // queda con el tipo E inferido. Para Result legacy (sin E
            // explícito, default Str) o cualquier Any, fallback a la
            // semántica anterior (e: Str / e: Any).
            let inner = match scrutinee {
                Type::Result { ok: _, err: e } => (**e).clone(),
                _ => Type::Any,
            };
            ctx.declare_var(name.clone(), inner, arm_span);
        }
        Pattern::Wildcard
        | Pattern::OkWildcard
        | Pattern::ErrWildcard
        | Pattern::Int(_)
        | Pattern::Float(_)
        | Pattern::Str(_)
        | Pattern::Bool(_)
        | Pattern::Null
        | Pattern::Range { .. } => {
            // No introducen bindings.
        }
        Pattern::Or(_) => {
            // R.2.1: or-patterns no introducen bindings por
            // contrato del parser (rechaza Ident/OkBinding/
            // ErrBinding adentro). No hace falta walkear.
        }
        // Tuples (mini-tanda T): recursea en cada slot con el tipo
        // correspondiente. Si scrutinee no es `Tuple` o difiere en
        // longitud, los sub-patterns igual se chequean con Any
        // (gradual) — el evaluator hace el match real.
        Pattern::Tuple(subs) => {
            let slot_tys: Vec<Type> = match scrutinee {
                Type::Tuple(items) if items.len() == subs.len() => items.clone(),
                _ => (0..subs.len()).map(|_| Type::Any).collect(),
            };
            for (sub, ty) in subs.iter().zip(slot_tys.iter()) {
                bind_pattern(ctx, sub, ty, arm_span);
            }
        }
    }
}

/// Sintetiza el tipo de un BinOp dado los tipos de sus operandos.
/// Aplica coerción Int→Float donde corresponde.
fn infer_binop(
    ctx: &mut CheckCtx,
    op: &BinOpKind,
    lt: &Type,
    rt: &Type,
    span: Span,
) -> Type {
    // Si cualquiera de los operandos es Any, no podemos chequear
    // con confianza — devolvemos Any sin error.
    if matches!(lt, Type::Any) || matches!(rt, Type::Any) {
        return Type::Any;
    }
    match op {
        BinOpKind::Add => {
            // Numérico o Str+Str.
            match (lt, rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Int, Type::Float) | (Type::Float, Type::Int) | (Type::Float, Type::Float) => {
                    Type::Float
                }
                (Type::Str, Type::Str) => Type::Str,
                _ => {
                    ctx.error_at(span, format!(
                        "el operador `+` no acepta `{}` y `{}`",
                        lt.display(ctx.types),
                        rt.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }
        BinOpKind::Sub | BinOpKind::Mul | BinOpKind::Div => {
            let sym = match op {
                BinOpKind::Sub => "-",
                BinOpKind::Mul => "*",
                BinOpKind::Div => "/",
                _ => unreachable!(),
            };
            match (lt, rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Int, Type::Float) | (Type::Float, Type::Int) | (Type::Float, Type::Float) => {
                    Type::Float
                }
                _ => {
                    ctx.error_at(span, format!(
                        "el operador `{}` espera operandos numéricos, recibió `{}` y `{}`",
                        sym,
                        lt.display(ctx.types),
                        rt.display(ctx.types)
                    ));
                    Type::Any
                }
            }
        }
        // R.1.2 — operador `%` solo Int. Float % Float queda como
        // sub-paso futuro (la ambigüedad entre `fmod` y
        // `rem_euclid` sobre Float requiere decisión de diseño).
        BinOpKind::Mod => match (lt, rt) {
            (Type::Int, Type::Int) | (Type::Any, _) | (_, Type::Any) => Type::Int,
            _ => {
                ctx.error_at(span, format!(
                    "el operador `%` espera Int en ambos lados, recibió `{}` y `{}`",
                    lt.display(ctx.types),
                    rt.display(ctx.types)
                ));
                Type::Int
            }
        },
        BinOpKind::Lt | BinOpKind::LtEq | BinOpKind::Gt | BinOpKind::GtEq => {
            // Comparación: numéricos o ambos Str.
            let ok = matches!(
                (lt, rt),
                (Type::Int, Type::Int)
                    | (Type::Int, Type::Float)
                    | (Type::Float, Type::Int)
                    | (Type::Float, Type::Float)
                    | (Type::Str, Type::Str)
            );
            if !ok {
                ctx.error_at(span, format!(
                    "comparación entre `{}` y `{}` no soportada",
                    lt.display(ctx.types),
                    rt.display(ctx.types)
                ));
            }
            Type::Bool
        }
        BinOpKind::Eq | BinOpKind::NotEq => {
            // Igualdad: cualquier par. El evaluator hace coerción Int↔Float
            // adentro de listas/mapas/etc. No emitimos warning.
            Type::Bool
        }
        BinOpKind::And | BinOpKind::Or => {
            if !matches!(lt, Type::Bool) {
                ctx.error_at(span, format!(
                    "el operador lógico espera Bool, lado izquierdo es `{}`",
                    lt.display(ctx.types)
                ));
            }
            if !matches!(rt, Type::Bool) {
                ctx.error_at(span, format!(
                    "el operador lógico espera Bool, lado derecho es `{}`",
                    rt.display(ctx.types)
                ));
            }
            Type::Bool
        }
        // Mini-tanda Bits — todos los bitwise solo Int. Cualquier
        // otro tipo dispara error de tipo claro.
        BinOpKind::BitAnd | BinOpKind::BitOr | BinOpKind::BitXor
        | BinOpKind::Shl | BinOpKind::Shr => {
            let sym = match op {
                BinOpKind::BitAnd => "&",
                BinOpKind::BitOr => "|",
                BinOpKind::BitXor => "^",
                BinOpKind::Shl => "<<",
                BinOpKind::Shr => ">>",
                _ => unreachable!(),
            };
            if !matches!(lt, Type::Int | Type::Any) {
                ctx.error_at(span, format!(
                    "el operador bit-a-bit `{}` espera Int, lado izquierdo es `{}`",
                    sym,
                    lt.display(ctx.types)
                ));
            }
            if !matches!(rt, Type::Int | Type::Any) {
                ctx.error_at(span, format!(
                    "el operador bit-a-bit `{}` espera Int, lado derecho es `{}`",
                    sym,
                    rt.display(ctx.types)
                ));
            }
            Type::Int
        }
    }
}

/// Compatibilidad para asignación / paso de argumento: `actual` se
/// puede usar donde se espera `expected`?
///
/// Reglas:
///   - `Any` matchea con cualquier cosa (gradual, en ambas direcciones).
///   - `Null` matchea con `T?` para cualquier T.
///   - `T` matchea con `T?` si el inner es compatible.
///   - `Int` matchea con `Float` (coerción implícita en aritmética
///     y asignación).
///   - Generics built-in (`List`/`Map`/`Result`/`Nullable`) y
///     `Function` se comparan recursivamente — así `Result<Any>`
///     pasa por `Result<User>`, `List<Int>` por `List<Float>`, etc.
///   - Resto: igualdad estructural.
pub fn is_compatible(actual: &Type, expected: &Type) -> bool {
    if matches!(actual, Type::Any) || matches!(expected, Type::Any) {
        return true;
    }
    // Fase 8.4 — `PyAny` es gradual igual que `Any` pero conserva
    // identidad propia para que el checker pueda distinguir "esto
    // viene de Python" de "esto es Any general" (relevante en
    // `infer_call` para tipar calls Python como `Result<Any>`).
    if matches!(actual, Type::PyAny) || matches!(expected, Type::PyAny) {
        return true;
    }
    if matches!(actual, Type::Null) && expected.is_nullable() {
        return true;
    }
    // `T` compatible con `T?` (un valor no-null donde se acepta nullable).
    if let Type::Nullable(inner) = expected {
        if is_compatible(actual, inner) {
            return true;
        }
    }
    if matches!(actual, Type::Int) && matches!(expected, Type::Float) {
        return true;
    }
    match (actual, expected) {
        (Type::List(a), Type::List(b)) => is_compatible(a, b),
        (Type::Map(ka, va), Type::Map(kb, vb)) => {
            is_compatible(ka, kb) && is_compatible(va, vb)
        }
        // Mini-tanda Re+: ambos lados (ok y err) deben ser compatibles.
        (Type::Result { ok: a_ok, err: a_err }, Type::Result { ok: b_ok, err: b_err }) => {
            is_compatible(a_ok, b_ok) && is_compatible(a_err, b_err)
        }
        (Type::Future(a), Type::Future(b)) => is_compatible(a, b),
        (Type::Nullable(a), Type::Nullable(b)) => is_compatible(a, b),
        (
            Type::Function { params: pa, ret: ra },
            Type::Function { params: pb, ret: rb },
        ) => {
            pa.len() == pb.len()
                && pa.iter().zip(pb.iter()).all(|(a, b)| is_compatible(a, b))
                && is_compatible(ra, rb)
        }
        // Tuples (mini-tanda T): compatible si misma longitud y cada
        // slot es compatible. `(Int, Str)` ↔ `(Float, Str)` por la
        // promoción Int→Float en cada slot.
        (Type::Tuple(a), Type::Tuple(b)) => {
            a.len() == b.len()
                && a.iter().zip(b.iter()).all(|(x, y)| is_compatible(x, y))
        }
        _ => actual == expected,
    }
}

/// Walkea una lista de Stmt en orden, manteniendo el scope actual.
fn check_block(ctx: &mut CheckCtx, body: &[Stmt]) {
    for s in body {
        check_stmt(ctx, s);
    }
}

/// Walkea una sola Stmt: chequea sus expresiones, abre scopes,
/// declara variables.
fn check_stmt(ctx: &mut CheckCtx, stmt: &Stmt) {
    match stmt {
        // Mini-tanda T — destructuring. Inferimos el tipo del RHS y
        // bindeamos cada slot del pattern.
        Stmt::Destructure { pattern, value, span } => {
            let value_ty = infer_expr(ctx, value);
            // Si el value tipa como Tuple, validamos arity.
            if let Type::Tuple(items) = &value_ty {
                if let crate::ast::Pattern::Tuple(subs) = pattern {
                    if items.len() != subs.len() {
                        ctx.error_at(*span, format!(
                            "destructuring de tupla: el pattern tiene {} slots, el valor tiene {}",
                            subs.len(), items.len()
                        ));
                    }
                }
            }
            bind_pattern(ctx, pattern, &value_ty, *span);
        }
        Stmt::Assign { target, type_, value, span } => {
            let value_ty = infer_expr(ctx, value);
            if let AssignTarget::Ident(name) = target {
                match type_ {
                    Some(ann) => {
                        let declared = resolve_type_expr(ann, ctx.types).unwrap_or(Type::Any);
                        if !is_compatible(&value_ty, &declared) {
                            ctx.error_at(*span, format!(
                                "`{}` declarado como `{}` recibió un valor `{}`",
                                name,
                                declared.display(ctx.types),
                                value_ty.display(ctx.types)
                            ));
                        }
                        // Una anotación explícita "redeclara" el binding
                        // con el tipo declarado y marca annotated=true.
                        // `def_span = span` del Stmt::Assign: en caso de
                        // reasignación, go-to-def salta al ÚLTIMO
                        // binding stmt (semántica simplificada del MVP).
                        ctx.declare_var_annotated(name.clone(), declared, *span);
                    }
                    None => {
                        // Sin anotación nueva: si la variable ya existe
                        // con anotación previa, exigimos que el valor
                        // nuevo sea compatible con ese tipo. Si la
                        // variable se infirió sin anotación, el modelo
                        // gradual permite que el tipo cambie.
                        match ctx.lookup_binding(name) {
                            Some(existing) if existing.annotated => {
                                let existing_ty = existing.ty.clone();
                                if !is_compatible(&value_ty, &existing_ty) {
                                    ctx.error_at(*span, format!(
                                        "`{}` declarado como `{}` recibió un valor `{}`",
                                        name,
                                        existing_ty.display(ctx.types),
                                        value_ty.display(ctx.types)
                                    ));
                                }
                                // Conservamos el binding anotado — la
                                // reasignación no relaja el tipo.
                                ctx.declare_var_annotated(name.clone(), existing_ty, *span);
                            }
                            _ => {
                                ctx.declare_var(name.clone(), value_ty, *span);
                            }
                        }
                    }
                }
            }
            // AssignTarget::Field { object, field }: validar que el
            // receptor es un tipo nominal con ese campo y que el tipo
            // del valor es compatible con el declarado. Cubre el
            // agujero documentado en deudas-post-5b (F2): antes solo
            // se atajaba en runtime.
            else if let AssignTarget::Field { object, field } = target {
                let obj_ty = infer_expr(ctx, object);
                match &obj_ty {
                    Type::Any => {
                        // Gradual escape — no chequeamos (matchea
                        // `Expr::Field` en `infer_expr`).
                    }
                    Type::Nominal(id) => {
                        let info = ctx.types.info(*id);
                        let type_name = info.name.clone();
                        // Si los fields no están resueltos (declaración
                        // con error previo), no chequeamos para no
                        // doblar el error.
                        if let Some(declared_fields) = info.fields.clone() {
                            match declared_fields.iter().find(|f| &f.name == field) {
                                Some(f) => {
                                    if !is_compatible(&value_ty, &f.type_) {
                                        ctx.error_at(*span, format!(
                                            "el campo `{}.{}` espera `{}`, recibió `{}`",
                                            type_name,
                                            field,
                                            f.type_.display(ctx.types),
                                            value_ty.display(ctx.types)
                                        ));
                                    }
                                }
                                None => {
                                    ctx.error_at(*span, format!(
                                        "el tipo `{}` no tiene un campo llamado `{}`",
                                        type_name, field
                                    ));
                                }
                            }
                        }
                    }
                    other => {
                        ctx.error_at(*span, format!(
                            "asignación a campo `.{}` sobre `{}`: solo se permite \
                             sobre instancias de un tipo custom",
                            field,
                            other.display(ctx.types)
                        ));
                    }
                }
            }
            // R.1.3 — `objeto[indice] = value` (mini-fase R).
            // Validar receptor `List<T>` con index `Int`, RHS
            // compatible con T. O receptor `Map<K, V>` con index
            // compatible con K, RHS compatible con V.
            else if let AssignTarget::Index { object, index } = target {
                let obj_ty = infer_expr(ctx, object);
                let idx_ty = infer_expr(ctx, index);
                match &obj_ty {
                    Type::Any => { /* gradual */ }
                    Type::List(item_ty) => {
                        if !is_compatible(&idx_ty, &Type::Int) {
                            ctx.error_at(*span, format!(
                                "el índice de `List<{}>` debe ser `Int`, recibió `{}`",
                                item_ty.display(ctx.types),
                                idx_ty.display(ctx.types)
                            ));
                        }
                        if !is_compatible(&value_ty, item_ty) {
                            ctx.error_at(*span, format!(
                                "la lista contiene `{}`, no se puede asignar `{}`",
                                item_ty.display(ctx.types),
                                value_ty.display(ctx.types)
                            ));
                        }
                    }
                    Type::Map(k_ty, v_ty) => {
                        if !is_compatible(&idx_ty, k_ty) {
                            ctx.error_at(*span, format!(
                                "la clave del map es `{}`, recibió `{}`",
                                k_ty.display(ctx.types),
                                idx_ty.display(ctx.types)
                            ));
                        }
                        if !is_compatible(&value_ty, v_ty) {
                            ctx.error_at(*span, format!(
                                "el map contiene `{}`, no se puede asignar `{}`",
                                v_ty.display(ctx.types),
                                value_ty.display(ctx.types)
                            ));
                        }
                    }
                    other => {
                        ctx.error_at(*span, format!(
                            "asignación a índice `[...] = v` no soportada sobre `{}` \
                             (solo `List` y `Map`)",
                            other.display(ctx.types)
                        ));
                    }
                }
            }
        }

        Stmt::Return(e, span) => {
            // Inferimos siempre para que los errores adentro afloren.
            let ret_ty = infer_expr(ctx, e);
            // R.2.4 (F3): `return` huérfano (fuera de fn) → error
            // estático claro. El evaluator también lo emitía en
            // runtime, pero el checker lo caza antes.
            if ctx.return_stack.is_empty() {
                ctx.error_at(*span,
                    "`return` solo puede usarse adentro de una función".to_string(),
                );
            }
            // Si estamos adentro de una función con return_type
            // declarado (y resoluble), validamos. Fuera de fn o con
            // return_type ausente (Any), no chequeamos.
            if let Some(expected) = ctx.return_stack.last().cloned() {
                if !is_compatible(&ret_ty, &expected) {
                    ctx.error_at(*span, format!(
                        "`return` devuelve `{}` pero la función declara `{}`",
                        ret_ty.display(ctx.types),
                        expected.display(ctx.types)
                    ));
                }
            }
            // Alimentamos el frame inferido de la fn contenedora.
            // Para FnDef se descarta al pop; para FnExpr lo usa para
            // sintetizar `ret`.
            if let Some(frame) = ctx.inferred_returns.last_mut() {
                frame.push(ret_ty);
            }
        }

        Stmt::Expr(e, _) => {
            let _ = infer_expr(ctx, e);
        }

        Stmt::ReturnStatus { status, body, span } => {
            // Inferimos las exprs para que errores adentro afloren.
            let status_ty = infer_expr(ctx, status);
            let body_ty = body.as_ref().map(|b| infer_expr(ctx, b));
            // Regla: solo válido adentro de un handler HTTP. Fuera de
            // eso es error claro — sintaxis nueva del spec, restringida
            // a handlers para no abrir return polimórfico en cualquier fn.
            let in_handler = ctx.in_http_handler.last().copied().unwrap_or(false);
            if !in_handler {
                ctx.error_at(*span,
                    "`return <status> { ... }` solo se admite adentro de un handler HTTP (`@get`/`@post`/`@put`/`@delete`) o una fn aplicada como `@middleware(...)`".to_string()
                );
            }
            // Status debe ser Int (rango 100-599 se valida en runtime).
            if !is_compatible(&status_ty, &Type::Int) {
                ctx.error_at(*span, format!(
                    "el status code de `return` debe ser Int, recibió `{}`",
                    status_ty.display(ctx.types)
                ));
            }
            // El body puede ser cualquier valor serializable; no chequeamos
            // contra el `return_type` formal del handler (es polimórfico:
            // el spec permite que un handler con `-> User` también haga
            // `return 404 { ... }`). El cuerpo se serializa a JSON en
            // runtime con `value_to_json`.
            let _ = body_ty;
        }

        Stmt::FnDef {
            name: fn_name,
            params,
            return_type,
            body,
            decorators,
            is_async,
            span: fn_span,
        } => {
            // Abrimos scope nuevo para params y locales. Los params se
            // bindean con su tipo declarado (o Any). Empujamos el
            // return type esperado al stack para que los `return`
            // adentro lo vean. Sin anotación → `Any` (no chequea).
            // También pusheamos un frame en `inferred_returns` para
            // mantener consistencia con FnExpr (los frames van en
            // paralelo); el contenido se descarta acá porque FnDef
            // ya tiene `return_type` declarado.
            //
            // Async (6.2): la firma EXTERNA de una `async fn` envuelve
            // su return type en `Future<T>` (eso se construye en
            // `preregister_fn_signatures` para que las llamadas a la
            // fn tipen correctamente). Pero adentro del body, los
            // `return x` siguen produciendo `T` puro (no `Future<T>`)
            // — `async` es transparente desde adentro. Por eso al
            // pushear el `return_stack` usamos `T` (no envuelto).
            let ret = match return_type {
                Some(r) => resolve_type_expr(r, ctx.types).unwrap_or(Type::Any),
                None => Type::Any,
            };
            // "Contexto HTTP" para el chequeo de `Stmt::ReturnStatus`:
            // handlers HTTP (`@get`/`@post`/`@put`/`@delete`) y fns
            // referenciadas por `@middleware(name)` en otro FnDef.
            // El pre-scan llena `ctx.middleware_fn_names` antes del walk.
            let is_http_handler = decorators.iter().any(|d| {
                matches!(d.name.as_str(), "get" | "post" | "put" | "delete")
            }) || ctx.middleware_fn_names.contains(fn_name);
            ctx.push_scope();
            ctx.return_stack.push(ret);
            ctx.inferred_returns.push(Vec::new());
            ctx.in_http_handler.push(is_http_handler);
            ctx.await_stack.push(*is_async);
            // R.2.4 (F3): break/continue NO escapan funciones. Guardamos
            // el loop_depth previo, reseteamos a 0 para el body, y
            // restauramos al salir.
            let saved_loop_depth = ctx.loop_depth;
            ctx.loop_depth = 0;
            for p in params {
                let pty = ann_to_type(p.type_.as_ref(), ctx.types);
                // Sin span propio en `Param` (deuda S1), aproximamos
                // con el span del FnDef. go-to-def sobre el uso del
                // param salta a la línea de la fn.
                ctx.declare_var(p.name.clone(), pty, *fn_span);
            }
            check_block(ctx, body);
            ctx.loop_depth = saved_loop_depth;
            ctx.inferred_returns.pop();
            ctx.return_stack.pop();
            ctx.in_http_handler.pop();
            ctx.await_stack.pop();
            ctx.pop_scope();
        }

        Stmt::TypeDef { .. } => {
            // Ya validada por resolve_program.
        }

        Stmt::While { condition, body, span, .. } => {
            let cond_ty = infer_expr(ctx, condition);
            if !is_compatible(&cond_ty, &Type::Bool) {
                ctx.error_at(*span, format!(
                    "la condición de `while` debe ser Bool, recibió `{}`",
                    cond_ty.display(ctx.types)
                ));
            }
            ctx.push_scope();
            ctx.loop_depth += 1;
            check_block(ctx, body);
            ctx.loop_depth -= 1;
            ctx.pop_scope();
        }

        Stmt::Loop { body, .. } => {
            ctx.push_scope();
            ctx.loop_depth += 1;
            check_block(ctx, body);
            ctx.loop_depth -= 1;
            ctx.pop_scope();
        }

        Stmt::For { var, iter, body, span, .. } => {
            // Mini-tanda Md — `var` ahora es un Pattern. Tipo del elem
            // depende del iter: List<T> → T; Range → Int; Map<K, V> →
            // Tuple([K, V]) (cada iteración produce un par).
            let iter_ty = infer_expr(ctx, iter);
            let elem_ty = match &iter_ty {
                Type::List(t) => (**t).clone(),
                Type::Range => Type::Int,
                Type::Map(k, v) => Type::Tuple(vec![(**k).clone(), (**v).clone()]),
                Type::Any => Type::Any,
                other => {
                    ctx.error_at(*span, format!(
                        "el iterable de `for` debe ser List, Range o Map, recibió `{}`",
                        other.display(ctx.types)
                    ));
                    Type::Any
                }
            };
            ctx.push_scope();
            ctx.loop_depth += 1;
            bind_for_pattern_in_checker(ctx, var, &elem_ty, *span);
            check_block(ctx, body);
            ctx.loop_depth -= 1;
            ctx.pop_scope();
        }

        Stmt::Break(value, _label, span) => {
            // Mini-tanda L: chequear el valor si está y empujarlo
            // al `break_value_stack` para que el `Expr::Loop`
            // contenedor lo unifique como tipo de retorno.
            let v_ty = if let Some(e) = value {
                infer_expr(ctx, e)
            } else {
                Type::Null
            };
            if let Some(frame) = ctx.break_value_stack.last_mut() {
                frame.push(v_ty);
            }
            // R.2.4 (F3): `break` huérfano (fuera de loop) → error.
            if ctx.loop_depth == 0 {
                ctx.error_at(*span,
                    "`break` solo puede usarse adentro de un loop (`while`, `loop`, `for`)".to_string(),
                );
            }
        }
        Stmt::Continue(_label, span) => {
            // R.2.4 (F3): `continue` huérfano (fuera de loop) → error.
            if ctx.loop_depth == 0 {
                ctx.error_at(*span,
                    "`continue` solo puede usarse adentro de un loop (`while`, `loop`, `for`)".to_string(),
                );
            }
        }

        Stmt::Import { path, alias, span } => {
            // `import a.b.c` bindea `c` (o `alias` si está) como Module.
            // 8.4: si el path arranca con `python` (prefijo reservado de
            // interop), el binding tipa como `PyAny` para que el
            // checker pueda refinar el tipo de los calls a
            // `Result<Any>`. Resto sigue como `Any` (gradual estándar).
            let from_python = path.first().map(|s| s.as_str()) == Some("python");
            let binding = alias.clone().or_else(|| path.last().cloned());
            if let Some(name) = binding {
                let ty = if from_python { Type::PyAny } else { Type::Any };
                // go-to-def sobre el binding salta a la línea del
                // import (al stmt, no al módulo remoto — cross-module
                // def es deuda visible del MVP de 9.x.3).
                ctx.declare_var(name, ty, *span);
            }
        }

        Stmt::FromImport { path, names, span } => {
            // Cada nombre se trae al scope como var. Algunos pueden
            // ser tipos (los chequea StructLit vía TypeEnv, ya
            // registrados en resolve_program), otros funciones o
            // values — sin info del módulo importado, `Any` es lo
            // mejor que tenemos en 5.3.1. Con alias, el binding local
            // usa el alias en lugar del nombre original.
            //
            // 8.4: `from python import X` bindea `X` como `PyAny` para
            // que los call sites se refinen a `Result<Any>` en
            // `infer_call`. Submódulos `from python.X import Y` también
            // tipan como `PyAny` — todo lo que viene de Python es
            // opaco para el checker.
            let from_python = path.first().map(|s| s.as_str()) == Some("python");
            for (n, alias) in names {
                let binding = alias.clone().unwrap_or_else(|| n.clone());
                let ty = if from_python { Type::PyAny } else { Type::Any };
                // go-to-def sobre el binding salta a la línea del
                // `from foo import ...` — cross-module def remoto
                // queda como deuda visible del MVP.
                ctx.declare_var(binding, ty, *span);
            }
        }

        // Fase 9.0.1 (F15): paralelo a `Expr::Error`. `Stmt::Error`
        // se ignora silenciosamente — el error real ya está en
        // `recovered_errors` del parser. No queremos emitir errores
        // derivados desde el checker sobre el mismo punto.
        Stmt::Error(_) => {}
    }
}

/// Pre-registra las firmas de los `Stmt::FnDef` top-level como
/// `Type::Function` en el scope global. Esto destraba referencias
/// hacia adelante y mutuas entre funciones top-level.
fn preregister_fn_signatures(ctx: &mut CheckCtx, program: &Program) {
    for stmt in program {
        if let Stmt::FnDef {
            name,
            params,
            return_type,
            is_async,
            span,
            ..
        } = stmt
        {
            let param_types: Vec<Type> = params
                .iter()
                .map(|p| ann_to_type(p.type_.as_ref(), ctx.types))
                .collect();
            let ret = match return_type {
                Some(r) => resolve_type_expr(r, ctx.types).unwrap_or(Type::Any),
                None => Type::Any,
            };
            // Async (6.2): la firma EXTERNA envuelve el return type
            // en `Future<T>`. Llamar a `async_fn(args)` produce
            // `Future<T>`, que se desempaca con `.await`. Incluso
            // sin anotación (T = Any), envolvemos como `Future<Any>`
            // — el roadmap (cross-cutting #3) lo formaliza así:
            // toda async fn produce un Future al llamarse, sin
            // excepciones. `is_compatible` y `.await` ya tratan a
            // `Any` como gradual escape, así que `Future<Any>`
            // sigue dejando pasar todo.
            let outer_ret = if *is_async {
                Type::Future(Box::new(ret))
            } else {
                ret
            };
            // go-to-def sobre el uso de la fn salta al span del FnDef
            // (que apunta al `fn` keyword). Aproximación; precisión
            // por nombre requiere span propio del identificador.
            ctx.declare_var(
                name.clone(),
                Type::Function {
                    params: param_types,
                    ret: Box::new(outer_ret),
                },
                *span,
            );
        }
    }
}

/// Entrada pública del checker estático completo: corre resolución
/// de anotaciones (`resolve_program`) y luego chequeo de expresiones.
/// Devuelve el env, el side-table de tipos por nodo (`TypeInfo`, Fase
/// 9.0 — F16), el side-table de definiciones por uso (`DefinitionInfo`,
/// Fase 9.x.3) y la lista de errores acumulados.
///
/// Los side-tables se llenan durante el chequeo: cada nodo `Expr` con
/// `Span` conocido tipa en `TypeInfo`; cada `Expr::Ident` resuelto a
/// una binding con `def_span` conocido registra `(use_span → def_span)`
/// en `DefinitionInfo`. La CLI (`fitz run`/`build`/`check`) descarta
/// ambos; el LSP (Fase 9.x) los consume para hover y go-to-definition.
pub fn check_program(
    program: &Program,
) -> (TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>) {
    let (env, mut errors) = resolve_program(program);
    // Encapsulamos `ctx` en un bloque para que su préstamo sobre `env`
    // termine antes del return: queremos mover `env`, `ctx.type_info`
    // y `ctx.def_info` por separado al caller.
    let (type_info, def_info) = {
        let mut ctx = CheckCtx::new(&env);
        preregister_fn_signatures(&mut ctx, program);
        collect_middleware_fn_names(&mut ctx, program);
        check_block(&mut ctx, program);
        // R.3 — chequear bodies de los métodos custom de cada
        // `type`. Esto sucede DESPUÉS del check_block normal para
        // que los nominales declarados como `type X { ... }` ya
        // estén disponibles. Cada method body se chequea con:
        //  - scope hijo del global con los fields del tipo
        //    pre-declarados como locales (opción A).
        //  - params del método sobre el mismo scope (locales).
        //  - return_stack con el return_type declarado (o Any).
        check_custom_methods(&mut ctx, program);
        errors.append(&mut ctx.errors);
        (ctx.type_info, ctx.def_info)
    };
    (env, type_info, def_info, errors)
}

/// R.3 — chequea cada body de método custom adentro de los `type`
/// declarados en el programa. Vuelta separada de `check_block` para
/// que los fields del tipo (ya resueltos en `resolve_program`) estén
/// disponibles como locales en el scope del body.
fn check_custom_methods(ctx: &mut CheckCtx, program: &Program) {
    for stmt in program {
        let Stmt::TypeDef { name, methods, .. } = stmt else { continue };
        if methods.is_empty() {
            continue;
        }
        // Recuperar los fields resueltos del tipo (poblados por
        // `resolve_program`). Si el tipo no existe → silencioso
        // (ya hubo error en resolve_program).
        let Some(id) = ctx.types.lookup(name) else { continue };
        let resolved_fields = match &ctx.types.info(id).fields {
            Some(fs) => fs.clone(),
            None => continue,
        };
        for m in methods {
            // Return type del método.
            let ret_ty = match &m.return_type {
                Some(r) => resolve_type_expr(r, ctx.types).unwrap_or(Type::Any),
                None => Type::Any,
            };
            ctx.push_scope();
            ctx.return_stack.push(ret_ty);
            ctx.inferred_returns.push(Vec::new());
            ctx.in_http_handler.push(false);
            ctx.await_stack.push(m.is_async);
            let saved_loop_depth = ctx.loop_depth;
            ctx.loop_depth = 0;
            // Pre-declarar fields como locales (opción A).
            for f in &resolved_fields {
                ctx.declare_var(f.name.clone(), f.type_.clone(), m.span);
            }
            // Declarar params (sobreescriben fields homónimos en el
            // scope local — `declare_var` reemplaza el binding al
            // entrar a la misma var).
            for p in &m.params {
                let pty = ann_to_type(p.type_.as_ref(), ctx.types);
                ctx.declare_var(p.name.clone(), pty, m.span);
            }
            check_block(ctx, &m.body);
            ctx.loop_depth = saved_loop_depth;
            ctx.inferred_returns.pop();
            ctx.return_stack.pop();
            ctx.in_http_handler.pop();
            ctx.await_stack.pop();
            ctx.pop_scope();
        }
    }
}

/// Mini-fase MW.1: pre-scan del programa para recolectar nombres de fns
/// que aparecen como argumento de un `@middleware(name)` en cualquier
/// FnDef. Esos nombres se marcan en `ctx.middleware_fn_names` para que
/// el chequeo de `Stmt::ReturnStatus` los acepte como "contexto HTTP"
/// (un middleware puede hacer `return 401 { ... }`). Solo capturamos
/// referencias por `Expr::Ident` (la forma documentada); cualquier otra
/// forma (call, lambda, etc.) la captura el evaluator en runtime con su
/// propio error claro.
fn collect_middleware_fn_names(ctx: &mut CheckCtx, program: &Program) {
    for stmt in program {
        if let Stmt::FnDef { decorators, .. } = stmt {
            for deco in decorators {
                if deco.name != "middleware" {
                    continue;
                }
                for arg in &deco.args {
                    if let Expr::Ident(n, _) = arg {
                        ctx.middleware_fn_names.insert(n.clone());
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AssignTarget, Decorator, Field, Param};
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn env_with(types: &[&str]) -> TypeEnv {
        let mut env = TypeEnv::new();
        for n in types {
            env.declare_nominal((*n).into()).unwrap();
        }
        env
    }

    fn resolve_str(src: &str) -> (TypeEnv, Vec<FitzError>) {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        resolve_program(&program)
    }

    fn errors_of(src: &str) -> Vec<FitzError> {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (_env, _types, _defs, errors) = check_program(&program);
        errors
    }

    #[test]
    fn error_de_asignacion_con_tipo_incompatible_cita_linea_real() {
        // B.1: el error apunta al `let` del stmt (línea/col reales),
        // no al genérico `0:0` que se usaba antes.
        let errors = errors_of("\n\nlet x: Int = \"texto\"");
        assert_eq!(errors.len(), 1, "esperaba 1 error, fue {:?}", errors);
        let e = &errors[0];
        assert_eq!(e.line, 3, "esperaba línea 3, fue {}", e.line);
        assert_eq!(e.column, 1, "esperaba col 1, fue {}", e.column);
    }

    // ---- Fase 6.2: type checker para async/await ----

    #[test]
    fn future_se_resuelve_como_generico_built_in() {
        // `Future<T>` reusa `TypeExpr::Generic` (decisión de 6.1) y
        // 6.2 lo mapea a `Type::Future(Box<T>)`. Aridad fija 1.
        let env = TypeEnv::new();
        let te = TypeExpr::Generic {
            name: "Future".into(),
            args: vec![TypeExpr::Named("Int".into())],
        };
        let ty = resolve_type_expr(&te, &env).expect("Future<Int> debe resolver");
        assert_eq!(ty, Type::Future(Box::new(Type::Int)));
    }

    #[test]
    fn future_sin_argumento_es_error_de_aridad() {
        let env = TypeEnv::new();
        let te = TypeExpr::Generic { name: "Future".into(), args: vec![] };
        let err = resolve_type_expr(&te, &env).expect_err("aridad 0 debe fallar");
        assert!(matches!(err.kind, ErrorKind::TypeError));
    }

    #[test]
    fn future_con_dos_argumentos_es_error_de_aridad() {
        let env = TypeEnv::new();
        let te = TypeExpr::Generic {
            name: "Future".into(),
            args: vec![
                TypeExpr::Named("Int".into()),
                TypeExpr::Named("Str".into()),
            ],
        };
        let err = resolve_type_expr(&te, &env).expect_err("aridad 2 debe fallar");
        assert!(matches!(err.kind, ErrorKind::TypeError));
    }

    #[test]
    fn future_display_muestra_inner() {
        let env = TypeEnv::new();
        let ty = Type::Future(Box::new(Type::Int));
        assert_eq!(ty.display(&env), "Future<Int>");
    }

    #[test]
    fn await_top_level_es_valido() {
        // Fase 6.7: el top-level acepta `.await` — el evaluator arranca
        // el runtime tokio ahí y el codegen emite `#[tokio::main]
        // async fn main()` automáticamente. Solo las fns sync
        // explícitas (FnDef no-async o FnExpr) lo rechazan.
        let errors = errors_of(
            "async fn fetch() -> Int {\n\
                 return 0\n\
             }\n\
             let x = fetch().await",
        );
        assert!(
            errors.is_empty(),
            "esperaba sin errores (await top-level es válido), fue: {:?}",
            errors
        );
    }

    #[test]
    fn await_dentro_de_fn_sync_es_error() {
        // FnDef sin `async` cuenta como contexto sync → `.await`
        // adentro emite error claro.
        let errors = errors_of(
            "async fn fetch() -> Int {\n\
                 return 0\n\
             }\n\
             fn sync_caller() -> Int {\n\
                 return fetch().await\n\
             }",
        );
        assert!(!errors.is_empty(), "esperaba error en .await dentro de fn sync");
        let msg = &errors[0].message;
        assert!(
            msg.contains(".await") && msg.contains("async fn"),
            "esperaba mensaje sobre `.await` y `async fn`, fue: {}",
            msg
        );
    }

    #[test]
    fn await_sobre_no_future_es_error() {
        // Operando concreto distinto de `Future<T>` → error.
        let errors = errors_of(
            "async fn f() -> Int {\n\
                 let x: Int = 42\n\
                 return x.await\n\
             }",
        );
        assert!(!errors.is_empty(), "esperaba 1 error");
        let msg = &errors[0].message;
        assert!(
            msg.contains("Future") && msg.contains("Int"),
            "esperaba mensaje sobre Future y Int, fue: {}",
            msg
        );
    }

    #[test]
    fn await_sobre_future_dentro_de_async_fn_pasa() {
        // Caso happy: async fn que llama a otra async fn y await-ea
        // el resultado. La llamada a `inner()` tipa `Future<Int>`,
        // `.await` desempaca a `Int`, return Int matchea.
        let errors = errors_of(
            "async fn inner() -> Int {\n\
                 return 1\n\
             }\n\
             async fn outer() -> Int {\n\
                 return inner().await\n\
             }",
        );
        assert!(errors.is_empty(), "esperaba sin errores, fue: {:?}", errors);
    }

    #[test]
    fn async_fn_referenciada_como_ident_tipa_function_con_future() {
        // Una `async fn f() -> Int` referenciada como valor (sin
        // call) tipa `Function { ret: Future<Int> }`. La firma
        // EXTERNA del async fn envuelve en Future. Validamos via
        // un `let g: Future<Int> = f()` que el checker acepte.
        let errors = errors_of(
            "async fn f() -> Int {\n\
                 return 0\n\
             }\n\
             let g: Future<Int> = f()",
        );
        assert!(errors.is_empty(), "esperaba sin errores, fue: {:?}", errors);
    }

    #[test]
    fn return_dentro_de_async_fn_no_envuelve_en_future() {
        // El `async` es transparente desde adentro: un `return x: Int`
        // adentro de `async fn -> Int` tipa Int contra Int, no
        // Int contra Future<Int>.
        let errors = errors_of(
            "async fn f() -> Int {\n\
                 return 42\n\
             }",
        );
        assert!(errors.is_empty(), "esperaba sin errores, fue: {:?}", errors);
    }

    #[test]
    fn await_adentro_de_fnexpr_es_error_aunque_padre_sea_async() {
        // FnExpr (closure) siempre pushea `await_stack` con false —
        // el lenguaje no soporta `async fn(...)` anónimas. `.await`
        // adentro del closure es error aunque el contenedor sea
        // async fn.
        let errors = errors_of(
            "async fn fetch() -> Int {\n\
                 return 0\n\
             }\n\
             async fn outer() -> Int {\n\
                 let cb = fn() => fetch().await\n\
                 return cb()\n\
             }",
        );
        assert!(!errors.is_empty(), "esperaba error en el `.await` del closure");
        let msg = &errors[0].message;
        assert!(
            msg.contains("async fn"),
            "esperaba mensaje sobre async fn, fue: {}",
            msg
        );
    }

    #[test]
    fn await_sobre_any_es_gradual_y_no_chequea() {
        // Una fn sin anotación de return tipa `Function { ret: Any }`.
        // La llamada produce Any; `.await` sobre Any pasa por
        // escape gradual (resultado Any). Sin errores.
        let errors = errors_of(
            "fn untyped() => 0\n\
             async fn outer() -> Int {\n\
                 return untyped().await\n\
             }",
        );
        // El `.await` no debería disparar el error de "no es Future"
        // porque el operando es Any (gradual escape). Si hay errores
        // otros, los inspeccionamos — pero el mensaje específico de
        // "Future" no debe aparecer.
        let any_future_err = errors.iter().any(|e|
            e.message.contains("Future") && e.message.contains(".await")
        );
        assert!(!any_future_err,
            "el await sobre Any no debería disparar error de Future, fue: {:?}",
            errors);
    }

    // ---- Fase 6.3: built-in `sleep` ----

    #[test]
    fn sleep_tipa_su_call_como_future_null() {
        // `sleep(100)` tipa `Future<Null>`. Validamos vía una
        // anotación destino — si el RHS no fuera `Future<Null>`,
        // el checker emitiría error de incompatibilidad.
        let errors = errors_of("let r: Future<Null> = sleep(100)");
        assert!(errors.is_empty(), "esperaba sin errores, fue: {:?}", errors);
    }

    #[test]
    fn sleep_con_argumento_no_int_es_error() {
        let errors = errors_of("let r = sleep(\"x\")");
        assert!(!errors.is_empty(), "esperaba error de tipo");
        let msg = &errors[0].message;
        assert!(
            msg.contains("sleep") && msg.contains("Int") && msg.contains("Str"),
            "esperaba mensaje sobre sleep/Int/Str, fue: {}",
            msg
        );
    }

    #[test]
    fn sleep_con_aridad_incorrecta_es_error() {
        let errors = errors_of("let r = sleep(1, 2)");
        assert!(!errors.is_empty(), "esperaba error de aridad");
        let msg = &errors[0].message;
        assert!(
            msg.contains("sleep") && msg.contains("1") && msg.contains("2"),
            "esperaba mensaje sobre sleep/1/2, fue: {}",
            msg
        );
    }

    #[test]
    fn sleep_await_dentro_de_async_fn_tipa_null() {
        // Integración con 6.2: `sleep(50).await` adentro de `async fn`
        // tipa `Null`. La fn declara `-> Null` y el return matchea.
        let errors = errors_of(
            "async fn pausa() -> Null {\n\
                 return sleep(50).await\n\
             }",
        );
        assert!(errors.is_empty(), "esperaba sin errores, fue: {:?}", errors);
    }

    // ---- C-F2: field assignment chequeo ----

    #[test]
    fn field_assign_con_tipo_compatible_pasa_checker() {
        let errors = errors_of(
            "type U { name: Str }\n\
             let u = U { name: \"x\" }\n\
             u.name = \"y\"",
        );
        assert!(errors.is_empty(), "no debería haber errores, fue {:?}", errors);
    }

    #[test]
    fn field_assign_con_tipo_incompatible_es_error() {
        let errors = errors_of(
            "type U { name: Str }\n\
             let u = U { name: \"x\" }\n\
             u.name = 42",
        );
        assert_eq!(errors.len(), 1, "esperaba 1 error, fue {:?}", errors);
        let msg = &errors[0].message;
        assert!(
            msg.contains("`U.name`") && msg.contains("Str") && msg.contains("Int"),
            "esperaba mensaje sobre U.name/Str/Int, fue: {}",
            msg
        );
    }

    // ---- Status codes custom (return <int> { ... }) ----

    #[test]
    fn return_status_dentro_de_handler_http_pasa_checker() {
        // `return 401 { ... }` adentro de un handler con `@get` es
        // válido. El checker lo permite sin importar el return_type
        // formal del handler (decisión: polimorfismo solo en handlers
        // HTTP).
        let errors = errors_of(
            "@get(\"/x\") fn protected() -> Str {\n\
                 return 401 {\"msg\": \"no autorizado\"}\n\
             }",
        );
        assert!(errors.is_empty(), "esperaba sin errores, fue: {:?}", errors);
    }

    #[test]
    fn return_status_fuera_de_handler_es_error() {
        // `return 401 { ... }` adentro de una fn sin decorator HTTP
        // → error claro. Bloquea uso accidental fuera de handlers.
        let errors = errors_of(
            "fn helper() -> Str {\n\
                 return 401 {\"msg\": \"x\"}\n\
             }",
        );
        assert!(!errors.is_empty(), "esperaba 1 error");
        let msg = &errors[0].message;
        assert!(
            msg.contains("handler HTTP") && msg.contains("@get"),
            "esperaba mensaje sobre handler HTTP, fue: {}",
            msg
        );
    }

    #[test]
    fn return_status_top_level_es_error() {
        // `return 401 { ... }` a nivel top-level (sin fn contenedora)
        // tampoco es válido — el checker lo rechaza por la misma regla.
        let errors = errors_of("return 401 {\"x\": 1}");
        assert!(!errors.is_empty(), "esperaba error");
        let msg = &errors[0].message;
        assert!(msg.contains("handler HTTP"), "fue: {}", msg);
    }

    #[test]
    fn return_status_no_chequea_contra_return_type_formal() {
        // Spec: un handler `-> User` puede hacer `return user` (User) y
        // también `return 404 { ... }`. El checker NO valida el body
        // del ReturnStatus contra el return type — es polimórfico.
        let errors = errors_of(
            "type User { id: Int }\n\
             @get(\"/u\") fn get_u() -> User {\n\
                 return 404 {\"error\": \"no encontrado\"}\n\
             }",
        );
        assert!(errors.is_empty(), "esperaba sin errores, fue: {:?}", errors);
    }

    // ---- Mini-fase MW.1: middleware ----

    #[test]
    fn request_y_response_son_built_in_referenciables() {
        // Un middleware referencia `Request` y `Response` sin declararlos
        // — los registra `register_http_builtin_types`. Sin ese pre-registro,
        // el checker se quejaría con "tipo desconocido `Request`".
        let errors = errors_of(
            "fn auth(req: Request) -> Response? {\n\
                 return null\n\
             }",
        );
        assert!(errors.is_empty(), "esperaba sin errores, fue: {:?}", errors);
    }

    #[test]
    fn return_status_dentro_de_middleware_pasa_checker() {
        // Una fn aplicada como `@middleware(fn)` puede hacer
        // `return <int> { ... }` — el pre-scan de MW.1 la marca como
        // contexto HTTP y el checker no se queja.
        let errors = errors_of(
            "fn auth(req: Request) {\n\
                 return 401 {\"error\": \"no autorizado\"}\n\
             }\n\
             @middleware(auth)\n\
             @get(\"/admin\")\n\
             fn admin() -> Str => \"ok\"",
        );
        assert!(errors.is_empty(), "esperaba sin errores, fue: {:?}", errors);
    }

    #[test]
    fn return_status_en_fn_no_referenciada_como_middleware_es_error() {
        // Solo las fns que aparecen en `@middleware(name)` se marcan
        // como contexto HTTP. Una fn random con `return <int>` sigue
        // disparando el error existente.
        let errors = errors_of(
            "fn helper() {\n\
                 return 401 {\"x\": 1}\n\
             }",
        );
        assert!(!errors.is_empty(), "esperaba error");
        assert!(
            errors[0].message.contains("middleware") || errors[0].message.contains("handler HTTP"),
            "esperaba mensaje sobre handler/middleware, fue: {}",
            errors[0].message
        );
    }

    #[test]
    fn field_assign_a_campo_inexistente_es_error() {
        let errors = errors_of(
            "type U { name: Str }\n\
             let u = U { name: \"x\" }\n\
             u.email = \"y\"",
        );
        assert!(!errors.is_empty(), "esperaba error de campo inexistente");
        let msg = &errors[0].message;
        assert!(
            msg.contains("no tiene un campo llamado `email`"),
            "esperaba mensaje sobre campo inexistente, fue: {}",
            msg
        );
    }

    #[test]
    fn field_assign_sobre_no_nominal_es_error() {
        let errors = errors_of(
            "let x = 42\n\
             x.foo = 1",
        );
        assert!(!errors.is_empty(), "esperaba error: asignar a campo de Int");
        let msg = &errors[0].message;
        assert!(
            msg.contains("solo se permite") || msg.contains("Int"),
            "esperaba mensaje sobre tipo incompatible, fue: {}",
            msg
        );
    }

    #[test]
    fn field_assign_sobre_any_no_chequea() {
        // El binding `m` viene de `from foo import m` → tipo Any.
        // El checker debe permitir el assign sin chequear el field
        // (gradual escape).
        // Simulamos con una var sin anotación que parser/checker
        // tratan como Any en el contexto adecuado. Usamos
        // `from import` que registra como Any.
        let errors = errors_of(
            "from external import obj\n\
             obj.anything = 42",
        );
        // Aceptamos que falle la carga del módulo (no existe), pero
        // si llega al checker el assign sobre Any debería silenciar.
        // En la práctica el checker solo registra la var como Any
        // si el FromImport pasa.
        // Filtramos el error de import si lo hay y verificamos que
        // NO haya error específico sobre el field.
        let field_errors: Vec<_> = errors.iter()
            .filter(|e| e.message.contains("campo") || e.message.contains(".anything"))
            .collect();
        assert!(
            field_errors.is_empty(),
            "no debería haber error sobre el campo, fue: {:?}",
            field_errors
        );
    }

    #[test]
    fn field_assign_con_nullable_acepta_null() {
        // `email: Str?` admite null o Str. Asignar null debe pasar.
        let errors = errors_of(
            "type U { email: Str? }\n\
             let u = U { email: \"x\" }\n\
             u.email = null",
        );
        assert!(errors.is_empty(), "Null compatible con Str?, fue: {:?}", errors);
    }

    // ---- fin C-F2 ----

    #[test]
    fn error_de_while_no_bool_cita_linea_real() {
        let errors = errors_of("\nwhile (42) { let _ = 0 }");
        assert!(!errors.is_empty(), "esperaba error de tipo");
        let e = &errors[0];
        assert_eq!(e.line, 2, "esperaba línea 2, fue {}", e.line);
        assert!(
            e.message.contains("while"),
            "esperaba mensaje sobre while, fue: {}",
            e.message
        );
    }

    // ---- resolve_type_expr ----

    #[test]
    fn resolve_primitivos() {
        let env = TypeEnv::new();
        for (name, expected) in [
            ("Int", Type::Int),
            ("Float", Type::Float),
            ("Str", Type::Str),
            ("Bool", Type::Bool),
            ("Null", Type::Null),
            ("Range", Type::Range),
        ] {
            let r = resolve_type_expr(&TypeExpr::named(name), &env).unwrap();
            assert_eq!(r, expected);
        }
    }

    #[test]
    fn resolve_primitivo_con_args_es_error_de_aridad() {
        // `Int<Str>` no tiene sentido — Int es aridad 0.
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Int".into(),
            args: vec![TypeExpr::named("Str")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::TypeError));
        assert!(err.message.contains("espera 0 argumento(s)"));
    }

    #[test]
    fn resolve_list_de_int() {
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int")],
        };
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::List(Box::new(Type::Int)));
    }

    #[test]
    fn resolve_list_aridad_incorrecta() {
        let env = TypeEnv::new();
        // List sin args
        let t1 = TypeExpr::named("List");
        let err = resolve_type_expr(&t1, &env).unwrap_err();
        assert!(err.message.contains("`List`"));
        assert!(err.message.contains("1 argumento"));

        // List con dos args
        let t2 = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int"), TypeExpr::named("Str")],
        };
        let err = resolve_type_expr(&t2, &env).unwrap_err();
        assert!(err.message.contains("recibió 2"));
    }

    #[test]
    fn resolve_map_de_str_int() {
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::named("Str"), TypeExpr::named("Int")],
        };
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::Map(Box::new(Type::Str), Box::new(Type::Int)));
    }

    #[test]
    fn resolve_map_aridad_incorrecta() {
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::named("Str")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("`Map`"));
        assert!(err.message.contains("2 argumento"));
        assert!(err.message.contains("recibió 1"));
    }

    #[test]
    fn resolve_result_anidado() {
        // Result<List<Int>>
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "Result".into(),
            args: vec![TypeExpr::Generic {
                name: "List".into(),
                args: vec![TypeExpr::named("Int")],
            }],
        };
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(
            r,
            Type::Result { ok: Box::new(Type::List(Box::new(Type::Int))), err: Box::new(Type::Str) },
        );
    }

    #[test]
    fn resolve_nullable_sobre_primitivo() {
        let env = TypeEnv::new();
        let t = TypeExpr::Nullable(Box::new(TypeExpr::named("Str")));
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(r, Type::Nullable(Box::new(Type::Str)));
    }

    #[test]
    fn resolve_nullable_sobre_generico() {
        // List<Int>?
        let env = TypeEnv::new();
        let t = TypeExpr::Nullable(Box::new(TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Int")],
        }));
        let r = resolve_type_expr(&t, &env).unwrap();
        assert_eq!(
            r,
            Type::Nullable(Box::new(Type::List(Box::new(Type::Int)))),
        );
    }

    #[test]
    fn resolve_nominal_declarado() {
        let env = env_with(&["User"]);
        let t = TypeExpr::named("User");
        let r = resolve_type_expr(&t, &env).unwrap();
        let id = env.lookup("User").unwrap();
        assert_eq!(r, Type::Nominal(id));
    }

    #[test]
    fn resolve_nominal_no_definido_es_error() {
        let env = TypeEnv::new();
        let t = TypeExpr::named("Usuario");
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("desconocido"));
        assert!(err.message.contains("Usuario"));
    }

    #[test]
    fn resolve_nominal_con_args_es_error() {
        // El usuario escribe `User<Int>` pero User no es genérico.
        let env = env_with(&["User"]);
        let t = TypeExpr::Generic {
            name: "User".into(),
            args: vec![TypeExpr::named("Int")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("no es genérico"));
    }

    #[test]
    fn resolve_generic_con_arg_invalido_propaga_error() {
        // List<Usuario> — Usuario no existe.
        let env = TypeEnv::new();
        let t = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::named("Usuario")],
        };
        let err = resolve_type_expr(&t, &env).unwrap_err();
        assert!(err.message.contains("Usuario"));
    }

    // ---- TypeEnv ----

    #[test]
    fn type_env_lookup_devuelve_el_id() {
        let env = env_with(&["A", "B"]);
        let a = env.lookup("A").unwrap();
        let b = env.lookup("B").unwrap();
        assert_ne!(a, b);
        assert_eq!(env.info(a).name, "A");
        assert_eq!(env.info(b).name, "B");
    }

    #[test]
    fn type_env_declarar_dos_veces_es_error() {
        let mut env = TypeEnv::new();
        env.declare_nominal("Foo".into()).unwrap();
        let err = env.declare_nominal("Foo".into()).unwrap_err();
        assert!(err.message.contains("`Foo`"));
        assert!(err.message.contains("más de una vez"));
    }

    // ---- resolve_program ----

    #[test]
    fn programa_vacio_no_da_errores() {
        let (env, errors) = resolve_str("");
        assert!(errors.is_empty());
        // Mini-fase MW.1: `Request` y `Response` se pre-registran como
        // nominales built-in del runtime HTTP, incluso en programas
        // vacíos. El usuario los puede referenciar sin declararlos.
        assert_eq!(env.nominal_count(), 2);
        assert!(env.lookup("Request").is_some());
        assert!(env.lookup("Response").is_some());
    }

    #[test]
    fn type_con_primitivos_se_resuelve() {
        let (env, errors) = resolve_str("type User { id: Int, name: Str }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
        let id = env.lookup("User").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "id");
        assert_eq!(fields[0].type_, Type::Int);
        assert_eq!(fields[1].type_, Type::Str);
    }

    #[test]
    fn type_con_generico_y_nullable_se_resuelve() {
        let (env, errors) = resolve_str(
            "type Post { tags: List<Str>, author: Str? }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
        let id = env.lookup("Post").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        assert_eq!(fields[0].type_, Type::List(Box::new(Type::Str)));
        assert_eq!(fields[1].type_, Type::Nullable(Box::new(Type::Str)));
    }

    #[test]
    fn type_que_referencia_otro_type_local() {
        let (env, errors) = resolve_str(
            "type Address { city: Str }\n\
             type User { home: Address }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
        let user = env.lookup("User").unwrap();
        let addr = env.lookup("Address").unwrap();
        let user_fields = env.info(user).fields.as_ref().unwrap();
        assert_eq!(user_fields[0].type_, Type::Nominal(addr));
    }

    #[test]
    fn forward_refs_mutuas_se_resuelven() {
        // type A { b: B }; type B { a: A }
        let (env, errors) = resolve_str(
            "type A { b: B }\n\
             type B { a: A }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
        let a = env.lookup("A").unwrap();
        let b = env.lookup("B").unwrap();
        let a_fields = env.info(a).fields.as_ref().unwrap();
        let b_fields = env.info(b).fields.as_ref().unwrap();
        assert_eq!(a_fields[0].type_, Type::Nominal(b));
        assert_eq!(b_fields[0].type_, Type::Nominal(a));
    }

    #[test]
    fn type_con_field_de_tipo_inexistente_reporta_error() {
        let (_, errors) = resolve_str("type User { home: Address }");
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("Address"));
        assert!(msg.contains("desconocido"));
        assert!(msg.contains("campo `home`"));
        assert!(msg.contains("tipo `User`"));
    }

    #[test]
    fn type_redeclarado_es_error() {
        let (_, errors) = resolve_str("type Foo { x: Int }\ntype Foo { y: Str }");
        assert!(errors.iter().any(|e| e.message.contains("Foo")
            && e.message.contains("más de una vez")));
    }

    #[test]
    fn default_literal_compatible_pasa() {
        let (_, errors) = resolve_str(
            "type Cfg { port: Int = 3000, debug: Bool = false }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn default_literal_incompatible_reporta_error() {
        let (_, errors) = resolve_str("type Cfg { port: Int = \"3000\" }");
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("Cfg.port"));
        assert!(msg.contains("`Int`"));
        assert!(msg.contains("`Str`"));
    }

    #[test]
    fn default_null_sobre_campo_nullable_pasa() {
        let (_, errors) = resolve_str("type User { email: Str? = null }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn default_null_sobre_campo_no_nullable_falla() {
        let (_, errors) = resolve_str("type User { id: Int = null }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("User.id"));
    }

    #[test]
    fn default_int_sobre_float_se_acepta_por_coercion() {
        let (_, errors) = resolve_str("type Cfg { ratio: Float = 1 }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn default_no_literal_se_acepta_pending_para_5_3() {
        // Default es una expresión (no literal): suma. El checker la
        // deja pasar — 5.3 chequea expresiones contra tipos.
        let (_, errors) = resolve_str("type Cfg { port: Int = 3000 + 1 }");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    // ---- anotaciones de FnDef y Assign ----

    #[test]
    fn fndef_con_anotaciones_resueltas() {
        let (_, errors) = resolve_str(
            "fn add(a: Int, b: Int) -> Int { return a + b }",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn fndef_con_tipo_param_invalido_reporta_error() {
        let (_, errors) = resolve_str("fn f(x: Foo) { return x }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
        assert!(errors[0].message.contains("parámetro `x`"));
        assert!(errors[0].message.contains("función `f`"));
    }

    #[test]
    fn fndef_con_return_invalido_reporta_error() {
        let (_, errors) = resolve_str("fn f() -> Foo { return 0 }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
        assert!(errors[0].message.contains("retorno"));
        assert!(errors[0].message.contains("función `f`"));
    }

    #[test]
    fn fndef_con_generico_invalido_reporta_error() {
        // `List<Foo>` donde Foo no existe.
        let (_, errors) = resolve_str("fn f(xs: List<Foo>) { return xs }");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
    }

    #[test]
    fn assign_con_tipo_invalido_reporta_error() {
        let (_, errors) = resolve_str("let x: Foo = 0");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
    }

    #[test]
    fn assign_con_generico_valido_pasa() {
        let (_, errors) = resolve_str("let xs: List<Int> = []");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn anotaciones_dentro_del_body_de_fn_se_validan() {
        // El let `y: Foo` está adentro del fn — la pasada baja y lo encuentra.
        let (_, errors) = resolve_str(
            "fn f() {\n\
                let y: Foo = 0\n\
                return y\n\
             }",
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("Foo"));
    }

    #[test]
    fn multiples_errores_se_acumulan_y_no_cortan() {
        let (_, errors) = resolve_str(
            "type A { x: Foo }\n\
             let y: Bar = 0\n\
             fn f(z: Baz) { return z }",
        );
        // Esperamos 3: Foo, Bar, Baz.
        assert_eq!(errors.len(), 3);
        let combined: String = errors.iter().map(|e| e.message.clone()).collect();
        assert!(combined.contains("Foo"));
        assert!(combined.contains("Bar"));
        assert!(combined.contains("Baz"));
    }

    // ---- construcciones AST directas, sin parser ----

    #[test]
    fn resolve_program_construye_env_via_ast_directo() {
        // Sanity: armamos el AST a mano sin pasar por parser para
        // confirmar que resolve_program no depende de detalles del
        // parser.
        use crate::ast::TypeExpr as TE;
        let program: Program = vec![
            Stmt::TypeDef {
                name: "X".into(),
                fields: vec![Field {
                    name: "n".into(),
                    type_: TE::named("Int"),
                    default: None,
                }],
                methods: vec![],
             span: Span::ZERO },
            Stmt::FnDef {
                name: "noop".into(),
                params: vec![Param {
                    name: "p".into(),
                    type_: Some(TE::named("X")),
                }],
                return_type: None,
                body: vec![],
                is_async: false,
                decorators: Vec::<Decorator>::new(),
             span: Span::ZERO },
            Stmt::Assign {
                target: AssignTarget::Ident("v".into()),
                type_: Some(TE::Nullable(Box::new(TE::named("X")))),
                value: Expr::Null(Span::ZERO),
             span: Span::ZERO },
        ];
        let (env, errors) = resolve_program(&program);
        assert!(errors.is_empty(), "errores: {:?}", errors);
        let x = env.lookup("X").unwrap();
        assert_eq!(env.info(x).fields.as_ref().unwrap()[0].type_, Type::Int);
    }

    // -----------------------------------------------------------------------
    // Tests — checker de expresiones (Fase 5.3.1)
    //
    // Cubrimos la pasada nueva: synth de literales/ident/BinOp/UnaryOp/
    // StrInterp/If/List/Map/StructLit/Field/Range, asignaciones con
    // anotación, scope local (FnDef/FnExpr/Match arms), e imports.
    // -----------------------------------------------------------------------

    fn check_str(src: &str) -> (TypeEnv, Vec<FitzError>) {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (env, _types, _defs, errors) = check_program(&program);
        (env, errors)
    }

    fn assert_ok(src: &str) {
        let (_, errors) = check_str(src);
        assert!(
            errors.is_empty(),
            "esperado sin errores, hubo: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    fn assert_error_with(src: &str, contains: &[&str]) {
        let (_, errors) = check_str(src);
        assert!(!errors.is_empty(), "esperado al menos un error, no hubo");
        let combined: String = errors.iter().map(|e| e.message.clone()).collect();
        for needle in contains {
            assert!(
                combined.contains(needle),
                "mensaje esperado contener `{}`, fue: {}",
                needle,
                combined
            );
        }
    }

    // ---- ident / scope ----

    #[test]
    fn ident_desconocido_emite_warning() {
        assert_error_with("print(no_existe)", &["variable desconocida", "no_existe"]);
    }

    #[test]
    fn ident_conocido_no_emite_error() {
        assert_ok("let x = 1\nprint(x)");
    }

    #[test]
    fn ident_tipo_nominal_como_value_es_any() {
        // `type User { ... }; let u = User { id: 1, name: "x" }` —
        // el StructLit usa el tipo; usar User pelado tampoco rompe.
        // El evaluator registra el type como Value en el env.
        assert_ok("type User { id: Int }\nprint(User)");
    }

    #[test]
    fn builtin_print_y_len_se_consideran_definidos() {
        // print y len existen por defecto.
        assert_ok("print(\"hola\")\nlen([1, 2, 3])");
    }

    // ---- BinOp ----

    #[test]
    fn binop_int_mas_int_es_ok() {
        assert_ok("let x: Int = 1 + 2");
    }

    #[test]
    fn binop_int_mas_float_es_float() {
        // Float := Int + Float (coerción).
        assert_ok("let x: Float = 1 + 2.0");
    }

    #[test]
    fn binop_str_mas_str_es_str() {
        assert_ok("let s: Str = \"a\" + \"b\"");
    }

    #[test]
    fn binop_str_mas_int_es_error() {
        assert_error_with("let x = \"a\" + 1", &["`+`", "Str", "Int"]);
    }

    #[test]
    fn binop_mul_acepta_numericos() {
        assert_ok("let x: Float = 2 * 3.5");
    }

    #[test]
    fn binop_mul_rechaza_str() {
        assert_error_with(
            "let x = \"a\" * 2",
            &["`*`", "operandos numéricos", "Str"],
        );
    }

    #[test]
    fn binop_comparacion_str_str_es_bool() {
        assert_ok("let b: Bool = \"a\" < \"b\"");
    }

    #[test]
    fn binop_comparacion_str_int_es_error() {
        assert_error_with("let b = \"a\" < 1", &["comparación", "Str", "Int"]);
    }

    #[test]
    fn binop_and_con_bool_es_ok() {
        assert_ok("let b: Bool = true and false");
    }

    #[test]
    fn binop_and_con_int_es_error() {
        assert_error_with("let b = 1 and true", &["lógico", "Bool", "Int"]);
    }

    // ---- UnaryOp ----

    #[test]
    fn unary_neg_int_es_ok() {
        assert_ok("let x: Int = -5");
    }

    #[test]
    fn unary_neg_str_es_error() {
        assert_error_with("let x = -\"hola\"", &["negación", "Int", "Str"]);
    }

    // ---- R.1.1 — `not` (mini-fase R) ----

    #[test]
    fn unary_not_sobre_bool_literal_es_ok() {
        assert_ok("let x: Bool = not true");
    }

    #[test]
    fn unary_not_sobre_bool_ident_es_ok() {
        assert_ok("let active: Bool = false\nlet inactive: Bool = not active");
    }

    #[test]
    fn unary_not_sobre_int_es_type_error() {
        assert_error_with(
            "let x = not 5",
            &["not", "Bool", "Int"],
        );
    }

    #[test]
    fn unary_not_sobre_str_es_type_error() {
        assert_error_with(
            "let x = not \"hola\"",
            &["not", "Bool", "Str"],
        );
    }

    #[test]
    fn unary_not_en_condicion_de_if_es_ok() {
        // Bool en condición ✓.
        assert_ok("let active = false\nif (not active) { print(\"x\") }");
    }

    #[test]
    fn unary_not_anidado_tipa_bool() {
        // `not not x` con x: Bool → Bool.
        assert_ok("let x = true\nlet y: Bool = not not x");
    }

    // ---- R.1.2 — operador `%` (mini-fase R) ----

    #[test]
    fn op_modulo_int_int_es_ok() {
        assert_ok("let r: Int = 10 % 3");
    }

    #[test]
    fn op_modulo_con_var_int_es_ok() {
        assert_ok("let n: Int = 100\nlet r: Int = n % 7");
    }

    #[test]
    fn op_modulo_con_float_es_type_error() {
        assert_error_with(
            "let r = 10.0 % 3",
            &["%", "Int", "Float"],
        );
    }

    #[test]
    fn op_modulo_con_str_es_type_error() {
        assert_error_with(
            "let r = \"hola\" % 3",
            &["%", "Int", "Str"],
        );
    }

    #[test]
    fn op_modulo_devuelve_int_no_any() {
        // El tipo sintetizado tiene que ser Int concreto (no Any),
        // así que un binding Bool falla — Bool no admite Int.
        // (Float SÍ admite Int por promoción Int→Float, por eso no
        // testeamos eso.)
        assert_error_with(
            "let r: Bool = 7 % 3",
            &["Bool", "Int"],
        );
    }

    // ---- R.1.3 — asignación a índice (mini-fase R) ----

    #[test]
    fn assign_index_list_int_int_es_ok() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\nxs[0] = 99",
        );
    }

    #[test]
    fn assign_index_list_str_index_es_error() {
        // List<T> exige Int en el index.
        assert_error_with(
            "let xs: List<Int> = [1, 2]\nxs[\"a\"] = 99",
            &["List", "Int", "Str"],
        );
    }

    #[test]
    fn assign_index_list_valor_tipo_incorrecto_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\nxs[0] = \"hola\"",
            &["lista", "Int", "Str"],
        );
    }

    #[test]
    fn assign_index_map_correcto_es_ok() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\nm[\"b\"] = 2",
        );
    }

    #[test]
    fn assign_index_map_key_tipo_incorrecto_es_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\nm[42] = 2",
            &["clave", "Str", "Int"],
        );
    }

    #[test]
    fn assign_index_sobre_no_collection_es_error() {
        assert_error_with(
            "let x = 5\nx[0] = 1",
            &["List", "Map"],
        );
    }

    // ---- Range ----

    #[test]
    fn range_de_ints_es_ok() {
        assert_ok("let r = 0..10");
    }

    #[test]
    fn range_con_extremo_no_int_es_error() {
        assert_error_with(
            "let r = 0..\"diez\"",
            &["rango", "Int", "Str"],
        );
    }

    // ---- List / Map ----

    #[test]
    fn list_vacia_es_list_any() {
        let (_, errors) = check_str("let xs = []");
        assert!(errors.is_empty(), "{:?}", errors);
    }

    #[test]
    fn list_homogenea_int_es_list_int() {
        // No hay error; el tipo inferido es List<Int>.
        assert_ok("let xs: List<Int> = [1, 2, 3]");
    }

    #[test]
    fn list_anotada_con_tipo_incompatible_es_error() {
        // El RHS sintetiza List<Str>; la anotación es List<Int>.
        assert_error_with(
            "let xs: List<Int> = [\"a\", \"b\"]",
            &["xs", "List<Int>", "List<Str>"],
        );
    }

    #[test]
    fn map_vacio_es_map_any_any() {
        assert_ok("let m = {}");
    }

    // ---- StructLit ----

    #[test]
    fn struct_lit_con_tipo_conocido_y_campos_ok() {
        assert_ok(
            "type User { id: Int, name: Str }\n\
             let u = User { id: 1, name: \"x\" }",
        );
    }

    #[test]
    fn struct_lit_con_tipo_desconocido_es_error() {
        assert_error_with(
            "let u = Usuario { id: 1 }",
            &["Usuario", "no existe"],
        );
    }

    #[test]
    fn struct_lit_campo_de_tipo_incompatible_es_error() {
        assert_error_with(
            "type User { id: Int }\n\
             let u = User { id: \"no soy int\" }",
            &["User.id", "Int", "Str"],
        );
    }

    #[test]
    fn struct_lit_campo_extra_es_error() {
        assert_error_with(
            "type User { id: Int }\n\
             let u = User { id: 1, edad: 30 }",
            &["User", "edad"],
        );
    }

    // ---- Field access ----

    #[test]
    fn field_access_de_nominal_devuelve_tipo_del_campo() {
        // Si u.id es Int, asignarlo a un Int es OK.
        assert_ok(
            "type User { id: Int, name: Str }\n\
             let u = User { id: 1, name: \"x\" }\n\
             let i: Int = u.id",
        );
    }

    #[test]
    fn field_access_de_nominal_tipo_incompatible_es_error() {
        assert_error_with(
            "type User { id: Int, name: Str }\n\
             let u = User { id: 1, name: \"x\" }\n\
             let i: Int = u.name",
            &["Int", "Str"],
        );
    }

    // ---- Assign con anotación ----

    #[test]
    fn assign_int_a_int_es_ok() {
        assert_ok("let x: Int = 42");
    }

    #[test]
    fn assign_str_a_int_es_error() {
        assert_error_with(
            "let x: Int = \"hola\"",
            &["x", "Int", "Str"],
        );
    }

    #[test]
    fn assign_null_a_nullable_es_ok() {
        assert_ok("let x: Str? = null");
    }

    #[test]
    fn assign_int_a_float_es_ok_por_coercion() {
        assert_ok("let x: Float = 1");
    }

    #[test]
    fn assign_str_a_nullable_str_es_ok() {
        // T compatible con T?.
        assert_ok("let x: Str? = \"hola\"");
    }

    // ---- if / while / for ----

    #[test]
    fn if_con_cond_no_bool_es_error() {
        assert_error_with(
            "if 1 { print(\"x\") }",
            &["condición", "if", "Bool", "Int"],
        );
    }

    #[test]
    fn if_con_cond_bool_es_ok() {
        assert_ok("if true { print(\"sí\") } else { print(\"no\") }");
    }

    #[test]
    fn while_con_cond_no_bool_es_error() {
        assert_error_with("while 1 { break }", &["while", "Bool"]);
    }

    #[test]
    fn for_sobre_range_bindea_var_como_int() {
        // Adentro del for, i debe usarse como Int y la suma debe
        // tipear bien.
        assert_ok("for i in 0..10 { let n: Int = i + 1 }");
    }

    #[test]
    fn for_sobre_list_int_bindea_elemento_como_int() {
        assert_ok(
            "let xs = [1, 2, 3]\n\
             for x in xs { let n: Int = x }",
        );
    }

    #[test]
    fn for_sobre_no_iterable_es_error() {
        assert_error_with(
            "for x in 42 { print(x) }",
            &["for", "List", "Range", "Int"],
        );
    }

    // ---- FnDef / params bindeados ----

    #[test]
    fn fndef_param_se_bindea_en_body() {
        // El parámetro n es Int por su anotación.
        assert_ok("fn double(n: Int) -> Int { return n * 2 }");
    }

    #[test]
    fn fndef_param_sin_anotacion_es_any() {
        // Sin anotación, n es Any — no se queja de la suma.
        assert_ok("fn double(n) { return n * 2 }");
    }

    // ---- FnExpr / params bindeados ----

    #[test]
    fn fn_expr_bindea_su_param() {
        // Si no bindeara, `u` seria desconocido.
        assert_ok(
            "type User { id: Int }\n\
             let users = [User { id: 1 }]\n\
             let r = users.find(fn(u) => u.id == 1)",
        );
    }

    // ---- Match con bindings ----

    #[test]
    fn match_ident_pattern_bindea_var() {
        // El brazo `x => ...` bindea x como el tipo del scrutinee.
        assert_ok(
            "let v = 42\n\
             let s = match v {\n\
                 0 => \"cero\"\n\
                 x => \"otro\"\n\
             }",
        );
    }

    #[test]
    fn match_ok_pattern_bindea_inner_de_result() {
        // Ok(v) en match sobre Result<Int> → v es Int.
        // En 5.3.1 el scrutinee es Ok(Int) que tiene tipo Result<Int>,
        // y v se bindea con Int. Verificamos sumando v con un Int.
        assert_ok(
            "let r = Ok(5)\n\
             let s = match r {\n\
                 Ok(v)  => v + 1\n\
                 Err(e) => 0\n\
             }",
        );
    }

    #[test]
    fn match_err_pattern_bindea_inner_como_str() {
        // Err(e) bindea e como Str — concatenable con Str.
        assert_ok(
            "let r = Err(\"boom\")\n\
             let s = match r {\n\
                 Ok(v)  => \"OK\"\n\
                 Err(e) => \"E: \" + e\n\
             }",
        );
    }

    // ---- Imports ----

    #[test]
    fn from_import_bindea_nombres_en_scope() {
        // No podemos cargar un módulo real acá sin tocar disco. Lo
        // que validamos: el ident traído por `from` no se reporta
        // como desconocido.
        assert_ok(
            "from utils import slugify\n\
             let s = slugify",
        );
    }

    #[test]
    fn import_bindea_modulo_como_var() {
        // `import foo` deja `foo` accesible como variable.
        assert_ok(
            "import utils\n\
             let m = utils",
        );
    }

    #[test]
    fn struct_lit_de_tipo_importado_es_ok() {
        // `from foo import User; User { ... }` no falla porque
        // FromImport registra el nombre como nominal sin fields.
        // El checker no valida campos (no los conoce) y deja pasar.
        assert_ok(
            "from foo import User\n\
             let u = User { id: 1, name: \"x\" }",
        );
    }

    // ---- Múltiples errores acumulados ----

    #[test]
    fn checker_acumula_varios_errores_de_expresiones() {
        let (_, errors) = check_str(
            "let a: Int = \"x\"\n\
             let b = 1 + \"y\"\n\
             let c = no_var",
        );
        assert!(errors.len() >= 3, "esperaba 3+ errores, hubo {}: {:?}",
            errors.len(), errors.iter().map(|e| &e.message).collect::<Vec<_>>());
    }

    // ---- 5.3.2: llamadas y return ----

    #[test]
    fn call_aridad_correcta_y_tipos_ok() {
        assert_ok(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n: Int = add(1, 2)",
        );
    }

    #[test]
    fn call_aridad_de_menos_es_error() {
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n = add(1)",
            &["add", "2 argumento", "recibió 1"],
        );
    }

    #[test]
    fn call_aridad_de_mas_es_error() {
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n = add(1, 2, 3)",
            &["add", "2 argumento", "recibió 3"],
        );
    }

    #[test]
    fn call_tipo_de_arg_incompatible_es_error() {
        assert_error_with(
            "fn add(a: Int, b: Int) -> Int { return a + b }\n\
             let n = add(\"hola\", 2)",
            &["add", "argumento 1", "Int", "Str"],
        );
    }

    #[test]
    fn call_coercion_int_a_float_pasa() {
        assert_ok(
            "fn double(x: Float) -> Float { return x * 2.0 }\n\
             let n: Float = double(3)",
        );
    }

    #[test]
    fn call_null_a_param_nullable_pasa() {
        assert_ok(
            "fn greet(name: Str?) -> Str { return \"hola\" }\n\
             let g: Str = greet(null)",
        );
    }

    #[test]
    fn call_recursion_top_level_compila() {
        // El pre-registro de firmas debe ver a `fact` antes de chequear
        // su body para que la llamada recursiva no se queje.
        assert_ok(
            "fn fact(n: Int) -> Int {\n\
                 if (n <= 1) { return 1 }\n\
                 return n * fact(n - 1)\n\
             }",
        );
    }

    #[test]
    fn call_forward_reference_cross_fn_compila() {
        // `a` llama a `b` definida después. El pre-registro lo hace
        // visible.
        assert_ok(
            "fn a(n: Int) -> Int { return b(n) + 1 }\n\
             fn b(n: Int) -> Int { return n * 2 }",
        );
    }

    #[test]
    fn call_sobre_callee_no_funcion_es_error() {
        // `1(2)` no es una función llamable.
        assert_error_with(
            "let r = (1)(2)",
            &["no es una función", "Int"],
        );
    }

    #[test]
    fn call_fn_expr_inline_pasa() {
        // (fn(x) => x + 1)(2) — el callee se resuelve a Function.
        // Aridad y param Any → cualquier arg pasa.
        assert_ok("let r = (fn(x) => x + 1)(2)");
    }

    #[test]
    fn call_fn_expr_inline_aridad_falla() {
        // Aridad chequeada incluso en FnExpr inline.
        assert_error_with(
            "let r = (fn(x, y) => x + y)(1)",
            &["2 argumento", "recibió 1"],
        );
    }

    // ---- Builtins ----

    #[test]
    fn len_con_un_arg_pasa_y_devuelve_int() {
        assert_ok("let n: Int = len([1, 2, 3])");
    }

    #[test]
    fn len_sin_args_es_error_de_aridad() {
        assert_error_with(
            "let n = len()",
            &["len", "1 argumento", "recibió 0"],
        );
    }

    #[test]
    fn len_con_dos_args_es_error_de_aridad() {
        assert_error_with(
            "let n = len([1], [2])",
            &["len", "1 argumento", "recibió 2"],
        );
    }

    #[test]
    fn print_es_variadic_no_chequea_aridad() {
        // print sigue siendo Any → cualquier número de args pasa.
        assert_ok("print()\nprint(\"x\")\nprint(1, 2, 3, \"y\")");
    }

    // ---- Stmt::Return contra return_type ----

    #[test]
    fn return_tipo_compatible_pasa() {
        assert_ok(
            "fn double(n: Int) -> Int { return n * 2 }",
        );
    }

    #[test]
    fn return_tipo_incompatible_es_error() {
        assert_error_with(
            "fn double(n: Int) -> Int { return \"no soy int\" }",
            &["return", "Int", "Str"],
        );
    }

    #[test]
    fn return_sin_anotacion_no_chequea() {
        // Sin return_type → Any → no chequea.
        assert_ok("fn f() { return \"cualquier cosa\" }");
    }

    #[test]
    fn return_arrow_implicito_chequea_contra_return_type() {
        // `fn f() -> Int => "x"` se desugarea a `body: [Stmt::Return("x", Span::ZERO)]`.
        assert_error_with(
            "fn id(x: Int) -> Int => \"no soy int\"",
            &["return", "Int", "Str"],
        );
    }

    #[test]
    fn return_arrow_implicito_correcto_pasa() {
        assert_ok("fn double(n: Int) -> Int => n * 2");
    }

    #[test]
    fn return_ok_contra_result_pasa() {
        // Ok(user) tipea como Result<User>; debe matchear con
        // -> Result<User>.
        assert_ok(
            "type User { id: Int, name: Str }\n\
             fn make(id: Int) -> Result<User> {\n\
                 return Ok(User { id: id, name: \"x\" })\n\
             }",
        );
    }

    #[test]
    fn return_err_contra_result_pasa_por_is_compatible_recursivo() {
        // Err(_) tipea como Result<Any>. Sin recursividad de
        // is_compatible esto fallaría contra Result<User>.
        assert_ok(
            "type User { id: Int }\n\
             fn make() -> Result<User> {\n\
                 return Err(\"boom\")\n\
             }",
        );
    }

    #[test]
    fn return_huerfano_chequea() {
        // R.2.4 (F3): `return` fuera de fn ahora es error estático
        // del checker. Antes pasaba al evaluator y se reportaba en
        // runtime; ahora lo cazamos antes.
        let (_, errors) = check_str("return 1");
        assert!(errors.iter().any(|e| e.message.contains("return") && e.message.contains("función")));
    }

    // ---- is_compatible recursivo en generics ----

    #[test]
    fn is_compatible_list_recursivo() {
        // List<Int> vs List<Float> pasa por coerción Int→Float adentro.
        assert!(is_compatible(
            &Type::List(Box::new(Type::Int)),
            &Type::List(Box::new(Type::Float)),
        ));
        // List<Str> vs List<Int> no pasa.
        assert!(!is_compatible(
            &Type::List(Box::new(Type::Str)),
            &Type::List(Box::new(Type::Int)),
        ));
    }

    #[test]
    fn is_compatible_result_recursivo() {
        // Result<Any> matchea Result<User>.
        let env = env_with(&["User"]);
        let user = Type::Nominal(env.lookup("User").unwrap());
        assert!(is_compatible(
            &Type::Result { ok: Box::new(Type::Any), err: Box::new(Type::Str) },
            &Type::Result { ok: Box::new(user.clone()), err: Box::new(Type::Str) },
        ));
        // Result<Int> no matchea Result<Str>.
        assert!(!is_compatible(
            &Type::Result { ok: Box::new(Type::Int), err: Box::new(Type::Str) },
            &Type::Result { ok: Box::new(Type::Str), err: Box::new(Type::Str) },
        ));
    }

    #[test]
    fn is_compatible_map_recursivo() {
        // Map<Str, Int> matchea Map<Str, Float>.
        assert!(is_compatible(
            &Type::Map(Box::new(Type::Str), Box::new(Type::Int)),
            &Type::Map(Box::new(Type::Str), Box::new(Type::Float)),
        ));
        // Map<Int, X> no matchea Map<Str, X> (clave incompatible).
        assert!(!is_compatible(
            &Type::Map(Box::new(Type::Int), Box::new(Type::Int)),
            &Type::Map(Box::new(Type::Str), Box::new(Type::Int)),
        ));
    }

    #[test]
    fn is_compatible_function_estructural() {
        // fn(Int) -> Int matchea fn(Int) -> Int.
        let a = Type::Function {
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        };
        let b = Type::Function {
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        };
        assert!(is_compatible(&a, &b));
        // fn(Int) -> Int no matchea fn(Int, Int) -> Int (aridad distinta).
        let c = Type::Function {
            params: vec![Type::Int, Type::Int],
            ret: Box::new(Type::Int),
        };
        assert!(!is_compatible(&a, &c));
    }

    // ---- 5.3.3: `?` y match exhaustivo sobre Result ----

    #[test]
    fn try_sobre_result_adentro_de_fn_result_pasa() {
        // El operando es Result<Int>; la fn declara -> Result<Int>.
        // El `?` desempaca a Int.
        assert_ok(
            "fn f(r: Result<Int>) -> Result<Int> {\n\
                 let v: Int = r?\n\
                 return Ok(v + 1)\n\
             }",
        );
    }

    #[test]
    fn try_sobre_any_no_chequea() {
        // `users.find(...)` es método built-in: callee Field → Any.
        // `?` sobre Any pasa sin chequear (gradual, hasta 5.3.4).
        assert_ok(
            "type User { id: Int }\n\
             fn h(id: Int) {\n\
                 let users = [User { id: 1 }]\n\
                 let u = users.find(fn(u) => u.id == id)?\n\
                 return u\n\
             }",
        );
    }

    #[test]
    fn try_sobre_no_result_es_error() {
        // `?` sobre un Int no tiene sentido.
        assert_error_with(
            "fn f() -> Result<Int> { let x = 1?\n return Ok(x) }",
            &["?", "Result", "Int"],
        );
    }

    #[test]
    fn try_adentro_de_fn_no_result_es_error() {
        // La fn retorna Int (no Result) y adentro hay un `?`. El
        // operando es Result<Int> concreto, así que disparamos la
        // regla "fn debe retornar Result".
        assert_error_with(
            "fn f(r: Result<Int>) -> Int {\n\
                 let v = r?\n\
                 return v\n\
             }",
            &["?", "Result", "Int"],
        );
    }

    #[test]
    fn try_adentro_de_fn_sin_return_type_no_chequea() {
        // Sin anotación → return_stack es Any → no chequeamos la
        // regla de la fn contenedora. El operando sí tiene que ser
        // Result, así que el `?` desempaca a Int sin warnings.
        assert_ok(
            "fn f(r: Result<Int>) {\n\
                 let v: Int = r?\n\
                 return v\n\
             }",
        );
    }

    #[test]
    fn try_top_level_no_chequea_la_regla_de_fn_contenedora() {
        // `?` adentro del scope global — sin return_stack, no
        // disparamos la regla "fn debe retornar Result". El operando
        // sí se chequea: Result<Int> → desempaca a Int.
        assert_ok("let r: Result<Int> = Ok(1)\nlet v: Int = r?");
    }

    #[test]
    fn try_encadenado_con_field_access_funciona() {
        // r?.id sobre Result<User> → User → Int.
        assert_ok(
            "type User { id: Int, name: Str }\n\
             fn f(r: Result<User>) -> Result<Int> {\n\
                 let id: Int = r?.id\n\
                 return Ok(id)\n\
             }",
        );
    }

    // ---- match exhaustivo sobre Result ----

    #[test]
    fn match_result_con_ok_y_err_es_exhaustivo() {
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(v) => \"ok\"\n\
                 Err(e) => \"err\"\n\
             }",
        );
    }

    #[test]
    fn match_result_solo_ok_falta_err() {
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(v) => \"ok\"\n\
             }",
            &["match", "Result", "exhaustivo", "Err"],
        );
    }

    #[test]
    fn match_result_solo_err_falta_ok() {
        assert_error_with(
            "let r: Result<Int> = Err(\"x\")\n\
             let s = match r {\n\
                 Err(e) => \"err\"\n\
             }",
            &["match", "Result", "exhaustivo", "Ok"],
        );
    }

    #[test]
    fn match_result_con_wildcard_solo_es_exhaustivo() {
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 _ => \"cualquier\"\n\
             }",
        );
    }

    #[test]
    fn match_result_con_ok_mas_wildcard_es_exhaustivo() {
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(v) => \"ok\"\n\
                 _ => \"resto\"\n\
             }",
        );
    }

    #[test]
    fn match_result_con_ident_catchall_es_exhaustivo() {
        // Un ident binding (catch-all) cubre cualquier valor — el
        // evaluator lo trata como wildcard.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 x => \"siempre\"\n\
             }",
        );
    }

    #[test]
    fn match_sobre_int_no_exige_exhaustividad() {
        // Match sobre un tipo no-Result: el checker no exige
        // exhaustividad en 5.3.3.
        assert_ok(
            "let n = 1\n\
             let s = match n {\n\
                 0 => \"cero\"\n\
                 1 => \"uno\"\n\
             }",
        );
    }

    #[test]
    fn match_sobre_any_no_exige_exhaustividad() {
        // Match sobre un valor de tipo Any (gradual escape): no se
        // exige exhaustividad.
        assert_ok(
            "fn pick() { return Ok(1) }\n\
             let s = match pick() {\n\
                 Ok(v) => \"ok\"\n\
             }",
        );
    }

    // ---- 5.3.4: métodos built-in con templates paramétricos ----

    // List<T>: push

    #[test]
    fn list_push_con_tipo_compatible_pasa() {
        assert_ok(
            "let xs: List<Int> = [1, 2]\n\
             xs.push(3)",
        );
    }

    #[test]
    fn list_push_con_tipo_incompatible_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             xs.push(\"x\")",
            &["push", "List<Int>", "Str"],
        );
    }

    #[test]
    fn list_push_aridad_incorrecta_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             xs.push(1, 2)",
            &["push", "1 argumento", "recibió 2"],
        );
    }

    // List<T>: pop, len

    #[test]
    fn list_pop_devuelve_t() {
        // Si pop sobre List<Int> devuelve Int, asignarlo a Int es OK.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let last: Int = xs.pop()",
        );
    }

    #[test]
    fn list_len_devuelve_int() {
        assert_ok(
            "let xs = [1, 2, 3]\n\
             let n: Int = xs.len()",
        );
    }

    // List<T>: map

    #[test]
    fn list_map_devuelve_list_del_ret_del_callback() {
        // map sobre List<Int> con callback fn(Int) -> Str → List<Str>.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let strs: List<Str> = xs.map(fn(x: Int) -> Str { return \"x\" })",
        );
    }

    #[test]
    fn list_map_con_callback_param_incompatible_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.map(fn(x: Str) -> Str { return x })",
            &["map", "Int", "Str"],
        );
    }

    #[test]
    fn list_map_con_callback_sin_anotaciones_es_any() {
        // Callback sin anotaciones → params = [Any], ret = Any.
        // El map devuelve List<Any>; asignarlo a List<Int> pasa por
        // is_compatible recursivo + Any.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.map(fn(x) => x * 2)",
        );
    }

    // List<T>: filter

    #[test]
    fn list_filter_devuelve_list_t() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let evens: List<Int> = xs.filter(fn(x: Int) -> Bool { return true })",
        );
    }

    #[test]
    fn list_filter_callback_aridad_incorrecta_es_error() {
        // El FnExpr siempre tiene `ret = Any` hasta 5.3.5, así que
        // no podemos detectar "ret no es Bool" sobre un FnExpr inline.
        // Lo que sí captamos es aridad del callback: filter espera
        // fn(T) -> Bool con un solo param.
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.filter(fn(x, y) => true)",
            &["filter", "1 argumento", "recibió 2"],
        );
    }

    // List<T>: find

    #[test]
    fn list_find_devuelve_result_t() {
        // find sobre List<User> devuelve Result<User>.
        assert_ok(
            "type User { id: Int }\n\
             let xs: List<User> = [User { id: 1 }]\n\
             let r: Result<User> = xs.find(fn(u: User) -> Bool { return true })",
        );
    }

    #[test]
    fn list_find_con_try_destrabba_t() {
        // xs.find(...)? adentro de una fn -> Result<User> debería
        // desempacar a User.
        assert_ok(
            "type User { id: Int }\n\
             fn first(xs: List<User>) -> Result<User> {\n\
                 let u: User = xs.find(fn(u: User) -> Bool { return true })?\n\
                 return Ok(u)\n\
             }",
        );
    }

    // List<T>: método desconocido

    #[test]
    fn list_metodo_desconocido_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2]\n\
             xs.lenght()",
            &["List<Int>", "lenght"],
        );
    }

    // Map<K, V>: get, has

    #[test]
    fn map_get_devuelve_result_v() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let r: Result<Int> = m.get(\"a\")",
        );
    }

    #[test]
    fn map_get_con_clave_incompatible_es_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let r = m.get(42)",
            &["get", "Map<Str, Int>", "Int"],
        );
    }

    #[test]
    fn map_has_devuelve_bool() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let b: Bool = m.has(\"a\")",
        );
    }

    #[test]
    fn map_keys_y_values_devuelven_listas() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let ks: List<Str> = m.keys()\n\
             let vs: List<Int> = m.values()",
        );
    }

    #[test]
    fn map_len_devuelve_int() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let n: Int = m.len()",
        );
    }

    #[test]
    fn map_metodo_desconocido_es_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             m.foo()",
            &["Map<Str, Int>", "foo"],
        );
    }

    // Str

    #[test]
    fn str_upper_lower_devuelven_str() {
        assert_ok(
            "let s = \"hola\"\n\
             let u: Str = s.upper()\n\
             let l: Str = s.lower()",
        );
    }

    #[test]
    fn str_len_devuelve_int() {
        assert_ok(
            "let n: Int = \"hola\".len()",
        );
    }

    #[test]
    fn str_metodo_desconocido_es_error() {
        assert_error_with(
            "let s = \"hola\"\n\
             s.upcase()",
            &["Str", "upcase"],
        );
    }

    // ---- S.1: contains/starts_with/ends_with ----

    #[test]
    fn str_contains_devuelve_bool() {
        assert_ok(
            "let b: Bool = \"hola\".contains(\"ol\")",
        );
    }

    #[test]
    fn str_starts_with_ends_with_devuelven_bool() {
        assert_ok(
            "let a: Bool = \"hola\".starts_with(\"ho\")\n\
             let b: Bool = \"hola\".ends_with(\"la\")",
        );
    }

    #[test]
    fn str_contains_con_arg_no_str_es_error() {
        assert_error_with(
            "let b = \"hola\".contains(1)",
            &["contains", "Str"],
        );
    }

    // ---- S.2: split/trim/replace/repeat ----

    #[test]
    fn str_split_devuelve_list_str() {
        assert_ok(
            "let xs: List<Str> = \"a,b,c\".split(\",\")",
        );
    }

    #[test]
    fn str_trim_devuelve_str() {
        assert_ok(
            "let s: Str = \"  hola  \".trim()",
        );
    }

    #[test]
    fn str_replace_devuelve_str() {
        assert_ok(
            "let s: Str = \"hola\".replace(\"o\", \"O\")",
        );
    }

    #[test]
    fn str_replace_con_int_es_error() {
        assert_error_with(
            "let s = \"hola\".replace(\"o\", 42)",
            &["replace", "Str"],
        );
    }

    #[test]
    fn str_repeat_con_int_devuelve_str() {
        assert_ok(
            "let s: Str = \"ab\".repeat(3)",
        );
    }

    #[test]
    fn str_repeat_con_str_es_error() {
        assert_error_with(
            "let s = \"ab\".repeat(\"3\")",
            &["repeat", "Int"],
        );
    }

    // ---- S.3: List.sort/reverse/contains ----

    #[test]
    fn list_sort_y_reverse_devuelven_null() {
        assert_ok(
            "let xs: List<Int> = [3, 1, 2]\n\
             xs.sort()\n\
             xs.reverse()",
        );
    }

    #[test]
    fn list_contains_con_arg_compatible_devuelve_bool() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: Bool = xs.contains(2)",
        );
    }

    #[test]
    fn list_contains_con_arg_incompatible_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.contains(\"x\")",
            &["contains", "Int"],
        );
    }

    // ---- I.1: indexing con tipos ----

    #[test]
    fn str_index_devuelve_str() {
        // I.1: `s[i]` ahora tipa como Str (antes era error).
        assert_ok(
            "let s = \"hola\"\n\
             let c: Str = s[0]",
        );
    }

    #[test]
    fn str_index_con_arg_no_int_es_error() {
        assert_error_with(
            "let s = \"hola\"\n\
             let c = s[\"x\"]",
            &["Str", "Int"],
        );
    }

    // ---- I.2: slicing ----

    #[test]
    fn list_slice_devuelve_list_mismo_tipo() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3, 4, 5]\n\
             let ys: List<Int> = xs[1..3]\n\
             let zs: List<Int> = xs[..2]\n\
             let ws: List<Int> = xs[3..]\n\
             let qs: List<Int> = xs[..]",
        );
    }

    #[test]
    fn str_slice_devuelve_str() {
        assert_ok(
            "let s = \"hola\"\n\
             let a: Str = s[0..2]\n\
             let b: Str = s[..2]\n\
             let c: Str = s[2..]\n\
             let d: Str = s[..]",
        );
    }

    #[test]
    fn slice_con_inclusive() {
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let ys: List<Int> = xs[0..=2]",
        );
    }

    #[test]
    fn slice_bound_no_int_es_error() {
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let ys = xs[\"a\"..2]",
            &["slice", "Int"],
        );
    }

    #[test]
    fn slice_sobre_tipo_no_soportado_es_error() {
        assert_error_with(
            "let n: Int = 42\n\
             let r = n[0..1]",
            &["slicing"],
        );
    }

    // Encadenado

    #[test]
    fn metodo_encadenado_map_filter() {
        // map(...).filter(...) en una sola línea — el ret de map
        // (List<Any> por FnExpr.ret=Any hasta 5.3.5) alimenta al
        // filter. Encadenamiento multi-línea sigue siendo deuda
        // explícita del parser (3.4).
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.map(fn(x) => x * 2).filter(fn(y) => true)",
        );
    }

    // Receptores que no tienen métodos built-in

    #[test]
    fn metodo_sobre_int_es_error() {
        assert_error_with(
            "let n = 1\n\
             n.foo()",
            &["Int", "foo"],
        );
    }

    // Nominal: gradual, no chequea ni rechaza

    #[test]
    fn metodo_sobre_nominal_no_chequea() {
        // type sin métodos custom: user.greet() pasa sin warning
        // (el evaluator lo emite en runtime). Es la regla gradual
        // de 5.3.4 — los métodos custom sobre `type` no existen
        // todavía, no rompemos código que use ese patrón.
        assert_ok(
            "type User { id: Int }\n\
             let u = User { id: 1 }\n\
             u.greet()",
        );
    }

    // ---- 5.3.5: FnExpr.ret inferido + Expr::Index ----

    // FnExpr ret inferido — formas básicas

    #[test]
    fn fn_expr_arrow_devuelve_tipo_del_expr() {
        // `fn(x: Int) => x * 2` se desugarea a body=[Return(x*2)];
        // ret inferido = Int. Filter exige Bool, así que esto debe
        // disparar el chequeo de ret.
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.filter(fn(x: Int) => x * 2)",
            &["filter", "Bool", "Int"],
        );
    }

    #[test]
    fn fn_expr_arrow_bool_pasa_filter() {
        // Mismo escenario pero con ret Bool — filter acepta.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int> = xs.filter(fn(x: Int) => x > 0)",
        );
    }

    #[test]
    fn fn_expr_block_un_solo_return_infiere_ese_tipo() {
        // Forma bloque con un return — ret = tipo del return.
        assert_error_with(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r = xs.find(fn(x: Int) { return x * 2 })",
            &["find", "Bool", "Int"],
        );
    }

    #[test]
    fn fn_expr_sin_return_es_null() {
        // Una fn que no retorna explícitamente — ret = Null. Para
        // un map, los elementos quedan como List<Null>.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Null> = xs.map(fn(x: Int) { print(x) })",
        );
    }

    // FnExpr ret inferido — unificación (lub) sobre varios returns

    #[test]
    fn fn_expr_lub_int_float_es_float() {
        // Dos returns: Int y Float → Float (coerción).
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Float> = xs.map(fn(x: Int) {\n\
                 if (x > 0) { return 1.5 }\n\
                 return 0\n\
             })",
        );
    }

    #[test]
    fn fn_expr_lub_null_y_t_es_nullable() {
        // Una rama devuelve null, otra Int → ret = Int?.
        assert_ok(
            "let xs: List<Int> = [1, 2, 3]\n\
             let r: List<Int?> = xs.map(fn(x: Int) {\n\
                 if (x > 0) { return x }\n\
                 return null\n\
             })",
        );
    }

    #[test]
    fn fn_expr_lub_result_ok_y_err_es_result_concreto() {
        // Ok(User) + Err("...") → lub(Result<User>, Result<Any>)
        // = Result<User>. Detecta que el FnExpr puede usarse donde
        // se espera Result<User>.
        assert_ok(
            "type User { id: Int }\n\
             let xs: List<User> = [User { id: 1 }]\n\
             let r: List<Result<User>> = xs.map(fn(u: User) {\n\
                 if (u.id > 0) { return Ok(u) }\n\
                 return Err(\"boom\")\n\
             })",
        );
    }

    // Expr::Index

    #[test]
    fn index_list_devuelve_t() {
        assert_ok(
            "let xs: List<Int> = [10, 20, 30]\n\
             let n: Int = xs[0]",
        );
    }

    #[test]
    fn index_list_con_indice_no_int_es_error() {
        assert_error_with(
            "let xs: List<Int> = [10, 20]\n\
             let n = xs[\"x\"]",
            &["List", "Int", "Str"],
        );
    }

    #[test]
    fn index_map_devuelve_v() {
        assert_ok(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let n: Int = m[\"a\"]",
        );
    }

    #[test]
    fn index_map_con_clave_incompatible_es_error() {
        assert_error_with(
            "let m: Map<Str, Int> = {\"a\": 1}\n\
             let n = m[42]",
            &["Map<Str, Int>", "Str", "Int"],
        );
    }

    #[test]
    fn index_sobre_int_es_error() {
        assert_error_with(
            "let n = 1\n\
             let x = n[0]",
            &["Int", "indexing"],
        );
    }

    #[test]
    fn index_sobre_str_ahora_si_se_implementa() {
        // I.1 (mini-tanda I): `s[i]` devuelve `Str` (un char).
        assert_ok(
            "let s = \"hola\"\n\
             let c: Str = s[0]",
        );
    }

    #[test]
    fn index_sobre_any_no_chequea() {
        // Receptor Any (var traída por import) → gradual.
        assert_ok(
            "from foo import xs\n\
             let n = xs[0]",
        );
    }

    // lub directo

    #[test]
    fn lub_funciones_basicas() {
        assert_eq!(lub(&Type::Int, &Type::Int), Type::Int);
        assert_eq!(lub(&Type::Int, &Type::Any), Type::Int);
        assert_eq!(lub(&Type::Any, &Type::Str), Type::Str);
        assert_eq!(lub(&Type::Int, &Type::Float), Type::Float);
        assert_eq!(lub(&Type::Float, &Type::Int), Type::Float);
        // Null + Int → Int?.
        assert_eq!(
            lub(&Type::Null, &Type::Int),
            Type::Nullable(Box::new(Type::Int))
        );
        // Int + Str → Any (mix arbitrario).
        assert_eq!(lub(&Type::Int, &Type::Str), Type::Any);
    }

    #[test]
    fn lub_recursivo_en_result() {
        // lub(Result<User>, Result<Any>) → Result<User>.
        let env = env_with(&["User"]);
        let user = Type::Nominal(env.lookup("User").unwrap());
        let a = Type::Result { ok: Box::new(user.clone()), err: Box::new(Type::Str) };
        let b = Type::Result { ok: Box::new(Type::Any), err: Box::new(Type::Str) };
        assert_eq!(lub(&a, &b), a);
    }

    #[test]
    fn unify_returns_vacio_es_null() {
        // Sin returns explícitos → Null (matchea el evaluator).
        assert_eq!(unify_returns(&[]), Type::Null);
    }

    // ---- Deuda residual de 5a: reasignación contra tipo previo ----

    #[test]
    fn reasignacion_sin_anotacion_a_var_anotada_falla() {
        // `m: Int = 1; m = "x"` — la primera asignación marcó `m`
        // como anotada Int; la segunda sin anotación viola eso.
        assert_error_with(
            "let m: Int = 1\n\
             m = \"no soy int\"",
            &["m", "Int", "Str"],
        );
    }

    #[test]
    fn reasignacion_sin_anotacion_a_var_inferida_pasa() {
        // `n = 1; n = "x"` — la primera asignación NO tenía anotación,
        // así que el modelo gradual permite cambiar el tipo.
        assert_ok(
            "let n = 1\n\
             n = \"ahora soy texto\"",
        );
    }

    #[test]
    fn reasignacion_compatible_a_var_anotada_pasa() {
        // `m: Int = 1; m = 2` — la reasignación respeta el tipo.
        assert_ok(
            "let m: Int = 1\n\
             m = 2",
        );
    }

    #[test]
    fn reasignacion_int_a_float_anotado_pasa_por_coercion() {
        // `f: Float = 1.0; f = 2` — Int → Float por coerción.
        assert_ok(
            "let f: Float = 1.0\n\
             f = 2",
        );
    }

    #[test]
    fn re_anotacion_con_otro_tipo_pasa_como_redeclaracion() {
        // `m: Int = 1; m: Str = "x"` — el segundo `m: Str = ...` es
        // una redeclaración explícita; el modelo gradual la permite
        // (el evaluator hace lo mismo). El bug que cierra esta deuda
        // es la reasignación SIN anotación nueva.
        assert_ok(
            "let m: Int = 1\n\
             let m: Str = \"x\"",
        );
    }

    #[test]
    fn match_result_con_ok_wildcard_y_err_wildcard_es_exhaustivo() {
        // `Ok(_)` y `Err(_)` cubren las dos variantes — no falta nada.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(_) => \"ok\"\n\
                 Err(_) => \"err\"\n\
             }",
        );
    }

    #[test]
    fn match_result_con_solo_ok_wildcard_falta_err() {
        // OkWildcard cuenta como variante Ok, no como catch-all.
        // Si falta Err, error de exhaustividad.
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r {\n\
                 Ok(_) => \"ok\"\n\
             }",
            &["match", "Result", "exhaustivo", "Err"],
        );
    }

    // ---- R.2.1: or-patterns en exhaustividad ----

    #[test]
    fn or_pattern_ok_wildcard_y_err_wildcard_juntos_es_exhaustivo() {
        // `Ok(_) | Err(_)` en un solo arm cubre ambas variantes —
        // el `update_result_coverage` recursea en `Pattern::Or`.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(_) | Err(_) => \"siempre\" }",
        );
    }

    #[test]
    fn or_pattern_solo_ok_wildcards_combinados_falta_err() {
        // `Ok(_) | Ok(_) =>` solo cubre Ok, falta Err.
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(_) | Ok(_) => \"x\" }",
            &["match", "Result", "exhaustivo", "Err"],
        );
    }

    #[test]
    fn or_pattern_con_literales_int_no_dispara_exhaustividad() {
        // Scrutinee `Int`, no `Result`. `1 | 2 | 3` está OK con `_`.
        assert_ok(
            "let s = match 1 { 1 | 2 | 3 => \"chico\", _ => \"otro\" }",
        );
    }

    #[test]
    fn or_pattern_strings_homogeneo() {
        assert_ok(
            "let d = \"lun\"\n\
             let s = match d { \"lun\" | \"mar\" | \"mie\" => \"laboral\", _ => \"x\" }",
        );
    }

    #[test]
    fn or_pattern_con_wildcard_subcase_es_catchall() {
        // Si un sub-pattern del Or es `_`, el arm es catch-all
        // (cubre cualquier cosa). Aunque en la práctica el usuario
        // no escribiría `Ok(_) | _`, validamos que recursea correcto.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { 1 | _ => \"x\" }",
        );
    }

    // ---- R.2.2: guards en match ----

    #[test]
    fn guard_bool_es_valido() {
        assert_ok(
            "let s = match 5 { x if x > 0 => \"pos\", _ => \"neg\" }",
        );
    }

    #[test]
    fn guard_no_bool_es_error() {
        // `x if x` con x: Int → guard no es Bool.
        assert_error_with(
            "let s = match 5 { x if x => \"y\", _ => \"z\" }",
            &["guard", "Bool", "Int"],
        );
    }

    #[test]
    fn guard_referencia_binding_del_pattern() {
        // El binding del pattern (`v` de `Ok(v)`) debe ser visible
        // en el guard.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(v) if v > 0 => \"pos\", Ok(_) => \"neg\", Err(_) => \"err\" }",
        );
    }

    #[test]
    fn arm_con_guard_no_cuenta_para_exhaustividad_result() {
        // Solo `Ok(_) if true` cubre Ok con guard; no cuenta como Ok
        // y falta Err.
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(_) if true => \"x\" }",
            &["match", "Result", "exhaustivo"],
        );
    }

    #[test]
    fn arm_con_guard_no_cuenta_como_catchall() {
        // `_ if cond` no es catch-all real (cond puede ser false).
        assert_error_with(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { _ if true => \"x\" }",
            &["match", "Result", "exhaustivo"],
        );
    }

    #[test]
    fn arm_con_guard_seguido_de_catchall_es_exhaustivo() {
        // Con un catch-all sin guard al final, el match es exhaustivo.
        assert_ok(
            "let r: Result<Int> = Ok(1)\n\
             let s = match r { Ok(v) if v > 0 => \"pos\", _ => \"otro\" }",
        );
    }

    // ---- R.2.4 (F3): return/break/continue huérfanos ----

    #[test]
    fn return_huerfano_top_level_es_error() {
        assert_error_with(
            "return 42",
            &["return", "función"],
        );
    }

    #[test]
    fn return_adentro_de_fn_es_valido() {
        assert_ok(
            "fn f() -> Int { return 42 }\n\
             let x = f()",
        );
    }

    #[test]
    fn break_huerfano_top_level_es_error() {
        assert_error_with(
            "break",
            &["break", "loop"],
        );
    }

    #[test]
    fn continue_huerfano_top_level_es_error() {
        assert_error_with(
            "continue",
            &["continue", "loop"],
        );
    }

    #[test]
    fn break_adentro_de_for_es_valido() {
        assert_ok(
            "for i in 0..5 {\n\
                 if i == 3 { break }\n\
             }",
        );
    }

    #[test]
    fn continue_adentro_de_while_es_valido() {
        assert_ok(
            "let x = 0\n\
             while (x < 10) {\n\
                 x = x + 1\n\
                 if x == 5 { continue }\n\
             }",
        );
    }

    #[test]
    fn break_adentro_de_loop_es_valido() {
        assert_ok(
            "loop {\n\
                 break\n\
             }",
        );
    }

    #[test]
    fn break_anidado_dos_loops_es_valido() {
        assert_ok(
            "for i in 0..3 {\n\
                 for j in 0..3 {\n\
                     if j == 1 { break }\n\
                 }\n\
             }",
        );
    }

    #[test]
    fn break_adentro_de_fn_interna_no_escapa_loop_externo() {
        // El parser de Fitz NO permite fns nested (top-level only),
        // pero FnExpr (closures) sí. break adentro de un closure
        // que aparece adentro de un loop NO está adentro de un loop
        // para fines del checker.
        assert_error_with(
            "for i in 0..3 {\n\
                 let f = fn() => 0\n\
                 let g = fn() {\n\
                     break\n\
                 }\n\
             }",
            &["break", "loop"],
        );
    }

    #[test]
    fn return_huerfano_y_break_huerfano_ambos_reportados() {
        // Ambos errores deberían aparecer en el mismo programa.
        let (_, errors) = check_str("return 42\nbreak");
        let return_errs = errors.iter().filter(|e| e.message.contains("return")).count();
        let break_errs = errors.iter().filter(|e| e.message.contains("break")).count();
        assert!(return_errs >= 1, "esperaba al menos 1 error de return huérfano");
        assert!(break_errs >= 1, "esperaba al menos 1 error de break huérfano");
    }

    // ---- R.3: métodos custom sobre type ----

    #[test]
    fn metodo_lee_field_como_local_es_valido() {
        assert_ok(
            "type U {\n\
                 name: Str\n\
                 fn greet() -> Str { return \"hola {name}\" }\n\
             }",
        );
    }

    #[test]
    fn metodo_con_typo_en_field_es_error() {
        assert_error_with(
            "type U {\n\
                 name: Str\n\
                 fn greet() -> Str { return naem }\n\
             }",
            &["naem", "no"],
        );
    }

    #[test]
    fn metodo_con_return_type_mismatch_es_error() {
        assert_error_with(
            "type U {\n\
                 count: Int\n\
                 fn label() -> Int { return \"no soy int\" }\n\
             }",
            &["return", "Str", "Int"],
        );
    }

    #[test]
    fn metodo_con_param_no_bool_en_if_es_error() {
        assert_error_with(
            "type U {\n\
                 fn check(n: Int) -> Bool {\n\
                     if (n) { return true }\n\
                     return false\n\
                 }\n\
             }",
            &["if", "Bool", "Int"],
        );
    }

    #[test]
    fn metodo_param_shadowea_field_compila() {
        // Cuando un param tiene el mismo nombre que un field, el
        // param gana en el scope. El checker permite la combinación
        // sin error.
        assert_ok(
            "type U {\n\
                 name: Str\n\
                 fn rename(name: Str) -> Str { return name }\n\
             }",
        );
    }

    #[test]
    fn metodo_break_es_orfano_si_no_hay_loop_local() {
        // Un `break` dentro del body de un método sin loop local es
        // huérfano. (R.2.4 reset de loop_depth en cada fn body.)
        assert_error_with(
            "type U {\n\
                 fn f() {\n\
                     break\n\
                 }\n\
             }",
            &["break", "loop"],
        );
    }

    #[test]
    fn reasignacion_anotada_propaga_a_uso_posterior() {
        // Verifica que el binding sigue siendo `Int` después de un
        // intento de reasignación incompatible: el uso posterior
        // espera Int.
        let (_, errors) = check_str(
            "let m: Int = 1\n\
             m = \"no soy int\"\n\
             let n: Int = m + 1",
        );
        // Esperamos solo el error de la reasignación, no errores
        // adicionales del `m + 1` (porque m sigue siendo Int).
        let count_reassign = errors
            .iter()
            .filter(|e| e.message.contains("m") && e.message.contains("Str"))
            .count();
        assert!(count_reassign >= 1, "esperaba error de reasignación, hubo: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>());
        // El uso posterior `m + 1` tipa OK (m sigue siendo Int).
        let count_plus = errors
            .iter()
            .filter(|e| e.message.contains("operador") && e.message.contains("+"))
            .count();
        assert_eq!(count_plus, 0, "no esperaba error en `m + 1`, hubo: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>());
    }

    // ---- Tipo función `Fn(...) -> U` (higher-order, F12) ----

    #[test]
    fn type_expr_function_resuelve_a_type_function() {
        // type Box { f: Fn(Int) -> Int } — el field tiene tipo función.
        let (env, errors) = resolve_str("type Box { f: Fn(Int) -> Int }");
        assert!(errors.is_empty(), "errores: {:?}", errors);
        let id = env.lookup("Box").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        assert_eq!(
            fields[0].type_,
            Type::Function {
                params: vec![Type::Int],
                ret: Box::new(Type::Int),
            },
        );
    }

    #[test]
    fn type_expr_function_sin_params_resuelve() {
        let (env, errors) = resolve_str("type Lazy { f: Fn() -> Str }");
        assert!(errors.is_empty(), "errores: {:?}", errors);
        let id = env.lookup("Lazy").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        assert_eq!(
            fields[0].type_,
            Type::Function {
                params: vec![],
                ret: Box::new(Type::Str),
            },
        );
    }

    #[test]
    fn type_expr_function_higher_order_resuelve() {
        // Fn(Fn(Int) -> Int, Int) -> Int — param es a su vez función.
        let (env, errors) = resolve_str(
            "type Apply { f: Fn(Fn(Int) -> Int, Int) -> Int }",
        );
        assert!(errors.is_empty(), "errores: {:?}", errors);
        let id = env.lookup("Apply").unwrap();
        let fields = env.info(id).fields.as_ref().unwrap();
        let expected = Type::Function {
            params: vec![
                Type::Function {
                    params: vec![Type::Int],
                    ret: Box::new(Type::Int),
                },
                Type::Int,
            ],
            ret: Box::new(Type::Int),
        };
        assert_eq!(fields[0].type_, expected);
    }

    #[test]
    fn type_expr_function_con_tipo_inexistente_reporta_error() {
        let (_, errors) = resolve_str("type Box { f: Fn(NoExiste) -> Int }");
        assert!(!errors.is_empty(), "esperaba error, hubo: {:?}", errors);
        let combined: String = errors.iter().map(|e| e.message.clone()).collect();
        assert!(combined.contains("NoExiste"));
    }

    #[test]
    fn anotacion_function_en_param_de_fndef_pasa_checker() {
        // fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x) }
        // El checker debe tipar la llamada `f(x)` contra la firma.
        assert_ok(
            "fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x) }",
        );
    }

    #[test]
    fn anotacion_function_en_param_detecta_aridad_mala() {
        // apply pasa 2 args a un f que toma 1.
        assert_error_with(
            "fn apply(f: Fn(Int) -> Int, x: Int) -> Int { return f(x, x) }",
            &["espera 1", "argumento"],
        );
    }

    // -----------------------------------------------------------------------
    // Tests — Span en errores expr-level (S1.2 sub-paso 2)
    //
    // Antes de S1.2 los errores sobre expresiones heredaban la línea
    // del `Stmt` contenedor (correcta) pero con columna degradada
    // (la del primer token del stmt). Tras este sub-paso, cada error
    // de tipo sobre BinOp/Call/Field/Index/UnaryOp/Try/Match/Range/
    // StructLit/Ident apunta a la columna del nodo problemático.
    //
    // Estos tests fijan posiciones concretas para que cualquier
    // pérdida de span se note en la suite.
    // -----------------------------------------------------------------------

    /// Helper que devuelve el primer error reportado, o panica si no hay.
    fn first_error(src: &str) -> FitzError {
        let (_, mut errors) = check_str(src);
        assert!(!errors.is_empty(), "esperado al menos un error en: {}", src);
        errors.remove(0)
    }

    #[test]
    fn span_binop_apunta_a_columna_del_operador() {
        // `let x: Int = 1 + "a"` — el `+` está en columna 16. El error
        // ahora reporta la columna del operador, no la del `let`.
        let e = first_error("let x: Int = 1 + \"a\"");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 16);
        assert!(e.message.contains("operador `+`"), "msg: {}", e.message);
    }

    #[test]
    fn span_call_aridad_apunta_a_paren_del_call() {
        // `fn f(x: Int) -> Int => x` y `let _ = f(1, 2)` — el `(` del
        // call está en columna 41 (después de `fn f(x: Int) -> Int => x\n`,
        // contando que `let _ = f(` arranca en línea 2).
        let src = "fn f(x: Int) -> Int => x\nlet _ = f(1, 2)";
        let e = first_error(src);
        assert_eq!(e.line, 2);
        // `let _ = f` ocupa columnas 1-9, así que `(` está en 10.
        assert_eq!(e.column, 10);
        assert!(e.message.contains("espera 1"), "msg: {}", e.message);
    }

    #[test]
    fn span_call_arg_apunta_al_argumento_concreto() {
        // El error de "argumento N espera X recibió Y" apunta al
        // argumento, no al `(`. Permite distinguir cuál de varios args
        // tiene mal tipo.
        let src = "fn f(x: Int) -> Int => x\nlet _ = f(\"hola\")";
        let e = first_error(src);
        assert_eq!(e.line, 2);
        // `let _ = f(` ocupa 1-10, el `"hola"` arranca en 11.
        assert_eq!(e.column, 11);
        assert!(
            e.message.contains("argumento 1") && e.message.contains("Int"),
            "msg: {}", e.message,
        );
    }

    #[test]
    fn span_unary_apunta_al_menos() {
        // `let s = -"a"` — el `-` está en columna 9.
        let e = first_error("let s = -\"a\"");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 9);
        assert!(e.message.contains("negación"), "msg: {}", e.message);
    }

    #[test]
    fn span_index_apunta_al_indice_concreto() {
        // `let xs: List<Int> = [1, 2, 3]\nlet _ = xs["k"]` — el `"k"`
        // está en columna 12 de la línea 2.
        let src = "let xs: List<Int> = [1, 2, 3]\nlet _ = xs[\"k\"]";
        let e = first_error(src);
        assert_eq!(e.line, 2);
        // `let _ = xs[` ocupa 1-11, `"k"` arranca en 12.
        assert_eq!(e.column, 12);
        assert!(e.message.contains("Int"), "msg: {}", e.message);
    }

    #[test]
    fn span_field_struct_extra_apunta_al_valor_del_extra() {
        // `type U { id: Int }; let u = U { id: 1, x: 2 }` — el `2` del
        // field extra está en columna 44.
        let src = "type U { id: Int }\nlet u = U { id: 1, x: 2 }";
        let e = first_error(src);
        assert_eq!(e.line, 2);
        // `let u = U { id: 1, x: ` ocupa 1-22, `2` arranca en 23.
        assert_eq!(e.column, 23);
        assert!(
            e.message.contains("no tiene un campo") && e.message.contains("`x`"),
            "msg: {}", e.message,
        );
    }

    #[test]
    fn span_ident_desconocido_apunta_al_ident() {
        // `let _ = no_existe` — `no_existe` arranca en columna 9.
        let e = first_error("let _ = no_existe");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 9);
        assert!(e.message.contains("variable desconocida"));
    }

    #[test]
    fn span_try_apunta_al_signo_pregunta() {
        // `let _ = 42?` — el `?` está en columna 11.
        let e = first_error("let _ = 42?");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 11);
        assert!(e.message.contains("`?`"), "msg: {}", e.message);
    }

    #[test]
    fn span_range_apunta_al_extremo_problematico() {
        // `let _ = 1..\"a\"` — el `"a"` está en columna 12.
        let e = first_error("let _ = 1..\"a\"");
        assert_eq!(e.line, 1);
        assert_eq!(e.column, 12);
        assert!(e.message.contains("fin del rango"), "msg: {}", e.message);
    }

    // -----------------------------------------------------------------------
    // Fase 8.4.1 — Type::PyAny + bindings de `from python import` +
    // calls Python tipan como Result<Any> en el checker.
    // -----------------------------------------------------------------------
    //
    // Estos tests funcionan SIN la feature `python` activa porque el
    // checker solo mira el shape del AST: `path[0] == "python"` activa
    // la rama PyAny independiente de si el binario lincó libpython.
    // El runtime solo se invoca con la feature, pero el chequeo
    // estático corre siempre.

    #[test]
    fn checker_from_python_import_bindea_como_pyany_no_any() {
        // El checker acepta `from python import math` y bindea `math`
        // con tipo PyAny. Cualquier uso pasa por las reglas asimétricas
        // de PyAny (calls → Result<Any>, field access → PyAny).
        let (_, errors) = check_str("from python import math\nlet x = math\n");
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_call_python_tipa_como_result_any() {
        // 8.4.2 (entró junto con 8.4.1): la llamada `math.sqrt(16.0)`
        // tipa como `Result<Any>` — usar el resultado como `Float`
        // directo SIN desempaquetar dispara error de tipo.
        assert_error_with(
            "from python import math\nlet f: Float = math.sqrt(16.0)\n",
            &["Float", "Result"],
        );
    }

    #[test]
    fn checker_call_python_con_match_compila_limpio() {
        // El patrón canónico (match para desempaquetar) tipa OK.
        // Cubre la regla de exhaustividad sobre Result (5.3.3) — `Ok`
        // + `Err` exhaustivo es suficiente.
        let (_, errors) = check_str(
            "from python import math\n\
             let f = match math.sqrt(16.0) { Ok(v) => v, Err(_) => -1.0 }\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_call_python_match_no_exhaustivo_es_error() {
        // La regla de 5.3.3 ahora pega con calls Python: `match` que
        // omite `Err` (sin catch-all) dispara error de exhaustividad
        // porque el scrutinee tipa como Result<Any>.
        assert_error_with(
            "from python import math\n\
             let f = match math.sqrt(16.0) { Ok(v) => v }\n",
            &["exhaustivo"],
        );
    }

    #[test]
    fn checker_try_operator_sobre_call_python_compila_dentro_de_fn_result() {
        // El `?` adentro de una fn que retorna `Result<T>` desempaca
        // el `Result<Any>` Python al `Any` interno (que matchea
        // cualquier T por gradual). En éxito devuelve el valor; en
        // falla propaga el Err al caller.
        let (_, errors) = check_str(
            "from python import math\n\
             fn root(x: Float) -> Result<Float> { return Ok(math.sqrt(x)?) }\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_try_operator_sobre_call_python_fn_no_result_es_error() {
        // La regla de 5.3.3 sobre `?` también pega con calls Python:
        // dentro de una fn que retorna `Int` (no `Result<...>`), `?`
        // sobre `math.sqrt(...)` dispara error porque el contenedor
        // no puede recibir el `Err` propagado.
        // (`?` a nivel top no se chequea en el checker — se reporta
        // en runtime, decisión heredada de 5.3.3.)
        assert_error_with(
            "from python import math\n\
             fn bad(x: Float) -> Int { return 0 + math.sqrt(x)? }\n",
            &["operador", "?"],
        );
    }

    #[test]
    fn checker_field_access_sobre_pyany_devuelve_pyany() {
        // `os.path` es field access sobre PyAny — el tipo del binding
        // sigue siendo PyAny. El check pasa sin errores y un call
        // sobre el submódulo (`os.path.join(...)`) sigue tipando
        // como Result<Any>.
        let (_, errors) = check_str(
            "from python import os\n\
             let p = os.path\n\
             let r = match p.join(\"a\", \"b\") { Ok(s) => s, Err(_) => \"\" }\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_pyany_es_compatible_con_anotacion_concreta() {
        // El patrón canónico del roadmap: `let row: User = py_call()?`.
        // Estáticamente, el `?` desempaca Result<Any> a Any; la
        // anotación User pasa por gradual escape (PyAny/Any → User).
        // El runtime hace la coerción real en 8.4.3.
        let (_, errors) = check_str(
            "type User { id: Int, name: Str }\n\
             from python import json\n\
             fn parse(s: Str) -> Result<User> { return Ok(json.loads(s)?) }\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    #[test]
    fn checker_import_normal_no_es_pyany() {
        // `import utils` (sin prefijo `python`) sigue siendo Any,
        // no PyAny — la lógica de refinar calls a Result<Any> solo
        // aplica a `from python import`. Validación: una llamada a
        // un módulo normal sigue siendo Any, así que un binding
        // tipado a Float pasa por gradual sin error.
        let (_, errors) = check_str(
            "import utils\nlet f: Float = utils.something(1)\n",
        );
        assert!(errors.is_empty(), "errores inesperados: {:?}", errors);
    }

    // -----------------------------------------------------------------
    // Fase 9.0.2 — el checker es silencioso sobre nodos Error del AST
    // (los emite solo `parse_with_recovery`). Sin estas garantías, el
    // LSP corriendo `check_program` sobre un AST recuperado generaría
    // cascadas de errores derivados sobre el mismo punto que ya está
    // reportado en la lista del parser.
    // -----------------------------------------------------------------

    /// Helper específico de los tests de 9.0.2: corre el pipeline
    /// completo del LSP (`parse_with_recovery` → `check_program`) y
    /// devuelve los errores que reportaría el checker. Los errores del
    /// parser quedan separados — el caller los pide aparte si los
    /// necesita.
    fn check_recovering(src: &str) -> Vec<FitzError> {
        let tokens = tokenize(src).expect("lex OK");
        let (program, _parser_errors) = crate::parser::parse_with_recovery(tokens);
        let (_env, _types, _defs, errors) = check_program(&program);
        errors
    }

    #[test]
    fn checker_stmt_error_no_emite_errores_propios() {
        // El parser produce un `Stmt::Error` en el lugar del stmt
        // roto. El checker no debe agregar ningún error sobre ese
        // nodo (los errores reales viven en la lista del parser).
        let src = "let x = 1 +\nlet y: Int = 2";
        let errors = check_recovering(src);
        assert!(
            errors.is_empty(),
            "el checker no debe emitir errores sobre Stmt::Error ni sobre stmts válidos vecinos: {:?}",
            errors
        );
    }

    #[test]
    fn checker_stmt_error_silencioso_pero_errores_reales_se_reportan() {
        // El checker silencia el Stmt::Error pero sigue reportando
        // errores genuinos del código bueno. El `let z: Int = "no"`
        // tiene tipo incompatible — el checker lo debe captar aunque
        // haya un Stmt::Error antes.
        let src = "let x = 1 +\nlet z: Int = \"no\"";
        let errors = check_recovering(src);
        assert_eq!(
            errors.len(),
            1,
            "esperaba 1 error de tipo del stmt válido: {:?}",
            errors
        );
        // El error es del stmt válido en la línea 2, no del Error node.
        assert_eq!(errors[0].line, 2);
    }

    #[test]
    fn checker_stmt_error_en_fn_body_no_aborta_check() {
        // `fn foo() { ... }` con un stmt roto adentro: el checker
        // sigue chequeando el resto del programa (la fn `bar` y su
        // anotación de tipo incorrecta) sin abortar por el Error
        // node intermedio.
        let src = "fn foo() {\n  let a = 1 +\n}\nfn bar() -> Int { return \"no\" }\n";
        let errors = check_recovering(src);
        // El error de la anotación de retorno (`Int` vs `Str`) DEBE
        // reportarse. Otros errores derivados del Error node NO.
        // (Cantidad exacta puede variar según refinamientos futuros;
        // lo crítico es: al menos un error de tipo del stmt válido, y
        // ninguno que mencione el Error node directamente.)
        assert!(
            errors.iter().any(|e| e.line == 4),
            "esperaba al menos un error de tipo en la línea 4 (return mal tipado): {:?}",
            errors
        );
    }

    #[test]
    fn checker_pipeline_recovering_no_panic_sobre_buffer_muy_roto() {
        // Smoke: programa salpicado de errores no debe crashear el
        // checker. La validación real es que `check_program` retorne
        // (no panic) sobre el AST con varios Error nodes.
        let src = "let a = +\nlet b: Int = \"no\"\nlet c = *\nfn ok() -> Int { return 7 }\n";
        let errors = check_recovering(src);
        // Garantía: al menos el error genuino del `let b: Int = "no"`
        // se reporta (línea 2). El resto puede o no tener errores
        // derivados — el contrato es "no panic" + "errores genuinos
        // del código bueno".
        assert!(
            errors.iter().any(|e| e.line == 2),
            "esperaba error de tipo en la línea 2: {:?}",
            errors
        );
    }

    #[test]
    fn checker_expr_error_se_propaga_como_any_sin_emitir_error() {
        // `Expr::Error` directo en el AST debe sintetizar `Type::Any`
        // y no emitir ningún error desde el checker. Construimos el
        // nodo manualmente porque el parser en 9.0.1 solo produce
        // Stmt::Error (recovery sub-expression llega después).
        //
        // Caso: `let x: Int = <Expr::Error>` — anotación Int + valor
        // Any. La regla de gradual (`is_compatible(Any, _)` siempre
        // true) hace que no haya error de tipo.
        use crate::ast::{AssignTarget, Span, Stmt, Expr as AstExpr};
        let program = vec![Stmt::Assign {
            target: AssignTarget::Ident("x".into()),
            type_: Some(TypeExpr::Named("Int".into())),
            value: AstExpr::Error(Span::ZERO),
            span: Span::ZERO,
        }];
        let (_env, _types, _defs, errors) = check_program(&program);
        assert!(
            errors.is_empty(),
            "Expr::Error debe sintetizar Type::Any y no agregar errores: {:?}",
            errors
        );
    }

    // -----------------------------------------------------------------------
    // Fase 9.0 — F16: IR tipado persistido por nodo.
    //
    // Tests sobre el side-table `TypeInfo` que se devuelve desde
    // `check_program`. Cubrimos: literales, Ident, BinOp, Call, Field,
    // StructLit, Match — los nodos que el LSP va a consultar para hover
    // y completion contextual. También validamos las dos políticas de
    // poblamiento: Span::ZERO se omite, Expr::Error se persiste como Any.
    // -----------------------------------------------------------------------

    /// Helper: corre el pipeline completo lex → parse → check y devuelve
    /// el `TypeInfo`. Útil para los tests de F16 que quieren mirar
    /// directamente el side-table.
    fn types_of(src: &str) -> TypeInfo {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (_env, type_info, _defs, _errors) = check_program(&program);
        type_info
    }

    #[test]
    fn types_info_persiste_tipos_de_literales() {
        // Programa con un literal de cada primitivo. Cada uno debe
        // quedar en el side-table con el tipo correspondiente.
        let info = types_of("let a = 1\nlet b = 1.5\nlet c = \"hola\"\nlet d = true\nlet e = null\n");
        // El parser emite columnas 1-indexed; los RHS arrancan en la
        // columna del valor literal. No matcheamos columnas exactas
        // — buscamos por línea + tipo.
        let by_line: std::collections::HashMap<usize, Vec<Type>> = info
            .inner
            .iter()
            .map(|(k, v)| (k.0, v.clone()))
            .fold(std::collections::HashMap::new(), |mut acc, (line, ty)| {
                acc.entry(line).or_default().push(ty);
                acc
            });
        assert!(
            by_line[&1].iter().any(|t| matches!(t, Type::Int)),
            "línea 1 debe tener Int: {:?}", by_line.get(&1)
        );
        assert!(
            by_line[&2].iter().any(|t| matches!(t, Type::Float)),
            "línea 2 debe tener Float: {:?}", by_line.get(&2)
        );
        assert!(
            by_line[&3].iter().any(|t| matches!(t, Type::Str)),
            "línea 3 debe tener Str: {:?}", by_line.get(&3)
        );
        assert!(
            by_line[&4].iter().any(|t| matches!(t, Type::Bool)),
            "línea 4 debe tener Bool: {:?}", by_line.get(&4)
        );
        assert!(
            by_line[&5].iter().any(|t| matches!(t, Type::Null)),
            "línea 5 debe tener Null: {:?}", by_line.get(&5)
        );
    }

    #[test]
    fn types_info_persiste_ident_y_binop() {
        // `let x = 10` declara x: Int. `let y = x + 5` accede al
        // ident `x` (debe tipar Int) y produce un BinOp (debe tipar
        // Int también).
        let info = types_of("let x = 10\nlet y = x + 5\n");
        // Buscamos en la línea 2 un Int — el ident `x` y el BinOp
        // `x + 5` ambos deben aparecer.
        let int_count_line2 = info
            .inner
            .iter()
            .filter(|(k, t)| k.0 == 2 && matches!(t, Type::Int))
            .count();
        assert!(
            int_count_line2 >= 3,
            "línea 2 debe persistir ≥3 nodos Int (ident `x`, literal `5`, BinOp): {:?}",
            info.inner
        );
    }

    #[test]
    fn types_info_persiste_call_y_field() {
        // Programa con tipo custom + call de fn + field access. Cada
        // nodo `Expr` debe quedar persistido con su tipo sintetizado.
        let src = "\
type User { id: Int, name: Str }
fn greet(u: User) -> Str => u.name
let u = User { id: 1, name: \"Fitz\" }
let s = greet(u)
";
        let info = types_of(src);
        // El call `greet(u)` está en la línea 4 (última línea con
        // código) — debe tipar Str porque `greet` retorna Str.
        let any_str_call = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 4 && matches!(t, Type::Str));
        assert!(
            any_str_call,
            "línea 4 debe tener Str (resultado del call greet(u)): {:?}",
            info.inner
        );
        // El struct lit `User { ... }` está en línea 3 — debe tipar
        // Nominal(User).
        let any_nominal_struct = info
            .inner
            .iter()
            .any(|(k, t)| k.0 == 3 && matches!(t, Type::Nominal(_)));
        assert!(
            any_nominal_struct,
            "línea 3 debe tener Nominal(User): {:?}",
            info.inner
        );
    }

    #[test]
    fn types_info_persiste_match_arms() {
        // Match sobre Result<Int>: cada arm tipa su body, el match
        // entero hereda el tipo del primer arm. Verificamos que algún
        // nodo de las ramas haya quedado persistido.
        let src = "\
fn divide(a: Int, b: Int) -> Result<Int> {
  if (b == 0) { return Err(\"div0\") }
  return Ok(a / b)
}
let r = divide(10, 2)
let v = match r {
  Ok(x) => x
  Err(_) => 0
}
";
        let info = types_of(src);
        // El match en sí debe quedar registrado con Int (tipo del
        // primer arm `x` que es Int).
        let has_int_in_match = info
            .inner
            .iter()
            .any(|(k, t)| k.0 >= 6 && matches!(t, Type::Int));
        assert!(
            has_int_in_match,
            "el match debe haber persistido Int en alguno de sus arms o el resultado: {:?}",
            info.inner
        );
    }

    #[test]
    fn types_info_omite_span_zero() {
        // Construimos un programa con un nodo sintético (Span::ZERO)
        // y validamos que NO aparezca en el side-table. La política
        // documentada en `TypeInfo::record` es omitir Span::ZERO para
        // evitar colisiones entre sintéticos.
        use crate::ast::{AssignTarget, Span, Stmt, Expr as AstExpr};
        let program = vec![Stmt::Assign {
            target: AssignTarget::Ident("x".into()),
            type_: None,
            value: AstExpr::Int(42, Span::ZERO),
            span: Span::ZERO,
        }];
        let (_env, type_info, _defs, _errors) = check_program(&program);
        // El Int(42, Span::ZERO) NO debe quedar en el side-table —
        // su span no es known. Cualquier otra cosa (si el parser
        // emite algo) tampoco tiene span real porque el programa fue
        // construido a mano. Total esperado: 0.
        assert_eq!(
            type_info.len(),
            0,
            "Span::ZERO debe omitirse del side-table: {:?}",
            type_info.inner
        );
    }

    #[test]
    fn types_info_expr_error_se_persiste_como_any() {
        // Un `Stmt::Assign` con `Expr::Error` como valor debe persistir
        // el Error node como `Type::Any` en el side-table (siempre que
        // su span sea known). Política documentada en `TypeInfo` —
        // uniforme con el comportamiento del checker (synthesize_expr
        // devuelve `Type::Any` para Error nodes).
        use crate::ast::{AssignTarget, Span, Stmt, Expr as AstExpr};
        let span = Span::new(7, 11); // span arbitrario "known"
        let program = vec![Stmt::Assign {
            target: AssignTarget::Ident("x".into()),
            type_: None,
            value: AstExpr::Error(span),
            span,
        }];
        let (_env, type_info, _defs, _errors) = check_program(&program);
        assert_eq!(
            type_info.type_at(span),
            Some(&Type::Any),
            "Expr::Error con span known debe persistir como Any: {:?}",
            type_info.inner
        );
    }

    #[test]
    fn types_info_type_at_devuelve_none_para_span_desconocido() {
        // Lookup por un span que el checker nunca registró debe
        // devolver None. Caso típico: el LSP pide hover sobre una
        // posición vacía (entre tokens).
        let info = types_of("let x = 1\n");
        // Span en una línea que el programa no toca.
        assert!(
            info.type_at(Span::new(999, 999)).is_none(),
            "span ausente debe devolver None"
        );
        // Span::ZERO también devuelve None por política.
        assert!(
            info.type_at(Span::ZERO).is_none(),
            "Span::ZERO debe devolver None"
        );
    }

    #[test]
    fn types_info_smoke_programa_real() {
        // Smoke sobre un programa con variedad de constructos. No
        // matcheamos el N exacto (frágil contra cambios futuros del
        // parser/checker), solo un piso conservador: al menos un
        // puñado de nodos quedaron registrados.
        let src = "\
type Point { x: Int, y: Int }
fn sum(p: Point) -> Int => p.x + p.y
let p = Point { x: 3, y: 4 }
let total = sum(p)
print(total)
";
        let info = types_of(src);
        assert!(
            info.len() >= 10,
            "programa con varios nodos debe persistir ≥10 entries; got {}: {:?}",
            info.len(),
            info.inner
        );
    }

    // -----------------------------------------------------------------------
    // Fase 9.x.3 — DefinitionInfo: side-table de uso → declaración.
    //
    // Tests sobre la población del side-table desde el wrapper
    // `infer_expr` cuando ve un `Expr::Ident`. Cubrimos: var local, fn
    // top-level, no-registro para builtins (def_span Span::ZERO).
    // -----------------------------------------------------------------------

    /// Helper: corre el pipeline y devuelve el `DefinitionInfo`.
    fn defs_of(src: &str) -> DefinitionInfo {
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let (_env, _types, def_info, _errors) = check_program(&program);
        def_info
    }

    #[test]
    fn def_info_registra_uso_de_variable_local() {
        // `let x = 1` en línea 1, `let y = x` en línea 2. El uso de
        // `x` en línea 2 debe registrar (use_span, def_span) con
        // def_span apuntando al Stmt::Assign de la línea 1.
        let defs = defs_of("let x = 1\nlet y = x\n");
        assert!(
            !defs.is_empty(),
            "uso de variable local debe registrarse en DefinitionInfo"
        );
        // Al menos un entry tiene def_span en línea 1 (el let de x).
        let has_def_in_line_1 = defs.iter().any(|(_, def_span)| def_span.line == 1);
        assert!(
            has_def_in_line_1,
            "def_span del binding `x` debe apuntar a línea 1: {:?}",
            defs.iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn def_info_no_registra_builtins() {
        // `print` es builtin con def_span = Span::ZERO. Usar `print`
        // no debe agregar entries a DefinitionInfo (Span::ZERO se
        // omite por política — no hay archivo donde saltar).
        let defs = defs_of("print(42)\n");
        // Solo el ident `print` produciría una entry; el literal `42`
        // no es un Ident. Verificamos que NO hay registros (DefInfo
        // vacío) — el filtro de Span::ZERO descarta el builtin.
        assert!(
            defs.is_empty(),
            "uso de builtin no debe registrarse en DefinitionInfo: {:?}",
            defs.iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn def_info_registra_uso_de_fn_top_level() {
        // `fn dobla(n: Int) -> Int => n * 2` en línea 1.
        // `dobla(21)` en línea 2 — el uso del nombre `dobla` debe
        // registrar def_span en línea 1.
        let defs = defs_of("fn dobla(n: Int) -> Int => n * 2\nlet x = dobla(21)\n");
        assert!(!defs.is_empty(), "uso de fn top-level debe registrarse");
        let has_def_in_line_1 = defs.iter().any(|(_, def_span)| def_span.line == 1);
        assert!(
            has_def_in_line_1,
            "def_span del FnDef `dobla` debe estar en línea 1: {:?}",
            defs.iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn def_info_registra_uso_de_param_de_fn() {
        // El uso del param `n` adentro del body de `fn dobla` se
        // registra como Ident con def_span en la línea del FnDef
        // (sin span propio en `Param`, aproximamos al FnDef).
        let defs = defs_of("fn dobla(n: Int) -> Int => n * 2\n");
        // El cuerpo flecha contiene un uso del ident `n` en línea 1.
        // El def_span del param también es línea 1 (mismo Stmt).
        assert!(!defs.is_empty(), "uso del param debe registrarse");
        let entry = defs.iter().next().unwrap();
        let (use_span, def_span) = entry;
        assert_eq!(use_span.0, 1, "use en línea 1");
        assert_eq!(def_span.line, 1, "def_span del param es la fn (línea 1)");
    }

    #[test]
    fn def_info_definition_at_devuelve_none_para_span_desconocido() {
        let defs = defs_of("let x = 1\nlet y = x\n");
        assert!(
            defs.definition_at(Span::new(999, 999)).is_none(),
            "span ausente debe devolver None"
        );
        assert!(
            defs.definition_at(Span::ZERO).is_none(),
            "Span::ZERO debe devolver None"
        );
    }

    #[test]
    fn def_info_no_registra_uso_de_ident_no_definido() {
        // El ident `nope` no existe en scope — el checker emite
        // error, pero no debe registrar entries en DefinitionInfo
        // (no hay binding al cual apuntar).
        let defs = defs_of("let y = nope\n");
        assert!(
            defs.is_empty(),
            "ident no definido no debe registrarse: {:?}",
            defs.iter().collect::<Vec<_>>()
        );
    }

    // ---- Mini-tanda C — list comprehensions ----

    #[test]
    fn checker_list_comp_simple_tipa_como_list_del_expr() {
        // `[x * 2 for x in [1, 2, 3]]` debe tipar como `List<Int>`
        // (el expr es Int, el iter es List<Int>).
        let src = "let r: List<Int> = [x * 2 for x in [1, 2, 3]]\n";
        let errors = check_recovering(src);
        assert!(
            errors.is_empty(),
            "esperaba sin errores, dio {:?}",
            errors
        );
    }

    #[test]
    fn checker_list_comp_sobre_range_tipa_int_en_var() {
        // El var de la comprehension sobre Range debe tipar Int.
        // Si el expr usa `var * 2`, el resultado es List<Int>.
        let src = "let r: List<Int> = [n * 2 for n in 0..10]\n";
        let errors = check_recovering(src);
        assert!(
            errors.is_empty(),
            "esperaba sin errores, dio {:?}",
            errors
        );
    }

    #[test]
    fn checker_list_comp_filter_no_bool_es_error() {
        // El filter debe ser `Bool`. Si es Int → error de tipo.
        let src = "let r = [x for x in [1, 2, 3] if x]\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("filtro") || e.message.contains("Bool")),
            "esperaba error sobre el filtro: {:?}",
            errors
        );
    }

    #[test]
    fn checker_list_comp_iter_no_iterable_es_error() {
        // Iter Int → error de tipo (no es List ni Range).
        let src = "let r = [x for x in 42]\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("iterable") || e.message.contains("List o Range")),
            "esperaba error sobre el iter: {:?}",
            errors
        );
    }

    #[test]
    fn checker_list_comp_var_no_escapa_al_caller() {
        // El scope local del var significa que tras la comprehension,
        // `x` no está visible afuera. Usar `x` afuera debe emitir
        // "variable no definida".
        let src = "let r = [x for x in [1, 2, 3]]\nlet y = x\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("variable")
                && (e.message.contains("x") || e.message.contains("no definida"))),
            "esperaba error sobre `x` no definida: {:?}",
            errors
        );
    }

    // ---- Mini-tanda Fm — format spec compatibilidad ----

    #[test]
    fn checker_fm_spec_f_con_float_compila_limpio() {
        let src = "let x: Float = 3.14\nlet s = \"{x:.2f}\"\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_fm_spec_f_con_int_compila_limpio() {
        // Promoción Int → Float transparente.
        let src = "let n: Int = 42\nlet s = \"{n:.2f}\"\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_fm_spec_f_con_str_es_error() {
        let src = "let s: Str = \"hola\"\nlet r = \"{s:.2f}\"\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("`f`")
                && e.message.contains("Float o Int")),
            "esperaba error de compatibilidad: {:?}",
            errors
        );
    }

    #[test]
    fn checker_fm_spec_d_con_float_es_error() {
        let src = "let x: Float = 3.14\nlet r = \"{x:d}\"\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("`d`") && e.message.contains("Int")),
            "esperaba error de compatibilidad: {:?}",
            errors
        );
    }

    #[test]
    fn checker_fm_spec_string_es_compatible_con_cualquier_tipo() {
        // El kind `s` acepta cualquier tipo (vía Display).
        let src = "let n: Int = 42\nlet r = \"{n:s}\"\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    // ---- Mini-tanda Md — for con Pattern en `var` ----

    #[test]
    fn checker_for_tuple_pattern_sobre_map_bindea_k_y_v_con_tipos_correctos() {
        // `for (k, v) in m` con m: Map<Str, Int> debe bindear k: Str y v: Int.
        // Si los uso correctamente, sin errores.
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nfor (k, v) in m {\n    let len_k: Int = k.len()\n    let v2: Int = v + 1\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_for_wildcard_pattern_compila_sin_binding() {
        // `for _ in xs` no bindea nada, no debe haber errores aún si `_`
        // se usaría adentro del body (no existe).
        let src = "let xs: List<Int> = [1, 2, 3]\nfor _ in xs {\n    print(\"hola\")\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_for_tuple_pattern_sobre_list_es_error() {
        // `for (a, b) in xs` con xs: List<Int> no tiene sentido — error.
        let src = "let xs: List<Int> = [1, 2, 3]\nfor (a, b) in xs {\n    print(a)\n}\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("tupla")),
            "esperaba error sobre tuple pattern: {:?}",
            errors
        );
    }

    #[test]
    fn checker_for_pattern_int_literal_es_error() {
        // `for 42 in xs` — pattern literal no admitido como for var.
        let src = "for 42 in [1, 2] { print(\"x\") }\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("admitido") || e.message.contains("Ident")),
            "esperaba error sobre pattern no admitido: {:?}",
            errors
        );
    }

    // ---- Mini-tanda It — iteradores enumerate/zip/chain ----

    #[test]
    fn checker_list_enumerate_tipa_como_list_tuple_int_t() {
        // `xs.enumerate()` con xs: List<Int> debe tipar `List<(Int, Int)>`.
        let src = "let xs: List<Int> = [1, 2, 3]\nlet ys: List<(Int, Int)> = xs.enumerate()\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_list_zip_con_tipos_distintos_tipa_list_tuple_t_u() {
        // `xs.zip(ys)` con xs: List<Int>, ys: List<Str> debe tipar
        // `List<(Int, Str)>`.
        let src =
            "let xs: List<Int> = [1, 2]\nlet ys: List<Str> = [\"a\", \"b\"]\nlet pairs: List<(Int, Str)> = xs.zip(ys)\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_list_chain_con_tipos_iguales_compila() {
        // `xs.chain(ys)` con ambos List<Int> debe tipar `List<Int>`.
        let src =
            "let xs: List<Int> = [1, 2]\nlet ys: List<Int> = [3, 4]\nlet zs: List<Int> = xs.chain(ys)\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_list_chain_con_tipos_incompatibles_es_error() {
        // `xs.chain(ys)` con xs: List<Int>, ys: List<Str> → error.
        let src =
            "let xs: List<Int> = [1, 2]\nlet ys: List<Str> = [\"a\"]\nlet zs = xs.chain(ys)\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("chain")
                && e.message.contains("List<Int>")),
            "esperaba error sobre chain con tipos incompatibles: {:?}",
            errors
        );
    }

    // ---- Mini-tanda Bits — operadores bit-a-bit ----

    // ---- Mini-tanda Re+ — Type::Result { ok, err } tipado ----

    #[test]
    fn checker_re_plus_result_t_e_anotacion_explicita() {
        let src = "type ApiError { status: Int, msg: Str }\nfn fetch() -> Result<Int, ApiError> {\n    return Err(ApiError { status: 503, msg: \"down\" })\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_re_plus_match_err_bindea_e_con_tipo_inferido() {
        // El binding `e` del `Err(e)` ahora tipa con el E del Result.
        let src = "type ApiError { status: Int, msg: Str }\nfn fetch() -> Result<Int, ApiError> {\n    return Err(ApiError { status: 503, msg: \"x\" })\n}\nlet code: Int = match fetch() {\n    Ok(v) => v,\n    Err(e) => e.status\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_re_plus_result_legacy_sin_e_explicito_sigue_andando() {
        // `Result<T>` sin E debe seguir funcionando (default Str).
        let src = "fn div(a: Int, b: Int) -> Result<Int> {\n    if b == 0 { return Err(\"zero\") }\n    return Ok(a / b)\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_re_plus_result_aridad_invalida_es_error() {
        // `Result<T, E, X>` con 3 args es error.
        let src = "let r: Result<Int, Str, Bool> = Ok(1)\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("Result") && e.message.contains("1 o 2")),
            "esperaba error sobre aridad: {:?}",
            errors
        );
    }

    #[test]
    fn checker_re_plus_result_display_con_e_concreto() {
        use crate::types::Type;
        let env = TypeEnv::default();
        let r1 = Type::Result { ok: Box::new(Type::Int), err: Box::new(Type::Str) };
        let r2 = Type::Result { ok: Box::new(Type::Int), err: Box::new(Type::Int) };
        // E = Str (default) → omite E.
        assert_eq!(r1.display(&env), "Result<Int>");
        // E ≠ Str (Int) → forma completa.
        assert_eq!(r2.display(&env), "Result<Int, Int>");
    }

    #[test]
    fn checker_bits_sobre_int_es_ok() {
        let src = "let a: Int = 5 & 3\nlet b: Int = 5 | 3\nlet c: Int = 5 ^ 3\nlet d: Int = 1 << 4\nlet e: Int = ~0\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }

    #[test]
    fn checker_bits_sobre_float_es_error() {
        let src = "let r = 3.14 & 2\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("bit-a-bit")
                && e.message.contains("Float")),
            "esperaba error sobre bit-a-bit con Float: {:?}",
            errors
        );
    }

    #[test]
    fn checker_bits_sobre_bool_es_error() {
        let src = "let r = true & false\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("bit-a-bit")
                && e.message.contains("Bool")),
            "esperaba error sobre `&` con Bool: {:?}",
            errors
        );
    }

    #[test]
    fn checker_bitnot_sobre_float_es_error() {
        let src = "let r = ~3.14\n";
        let errors = check_recovering(src);
        assert!(
            errors.iter().any(|e| e.message.contains("`~`") && e.message.contains("Int")),
            "esperaba error sobre `~` con Float: {:?}",
            errors
        );
    }

    #[test]
    fn checker_list_enumerate_se_compone_con_for_destructuring_de_md() {
        // El caso canónico que motiva la mini-tanda: `for (i, x) in xs.enumerate()`.
        let src = "let xs: List<Str> = [\"a\", \"b\"]\nfor (i, x) in xs.enumerate() {\n    let idx: Int = i\n    let val: Str = x\n}\n";
        let errors = check_recovering(src);
        assert!(errors.is_empty(), "esperaba sin errores, dio {:?}", errors);
    }
}
