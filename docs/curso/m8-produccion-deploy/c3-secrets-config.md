# M8.C3 — Secrets management: `secret()`, `config()` y `Secret<T>`

**Pre-requisitos**: [M8.C2 — observability](c2-observability-otel.md).
Sabés emitir logs estructurados; ahora vas a asegurarte de que **no
filtren credenciales**.

**Objetivo**: usar `secret(key) -> Secret<Str>` y `config(key, default)
-> Str` para leer env vars con dos semánticas distintas (sensitive vs
público), aprovechar el tipo opaco `Secret<T>` para que el checker
rechace fugas, y entender patterns canónicos (.env files, K8s
secrets, sidecars).

**Por qué importa**: en Python o Node, `os.getenv("DB_PASSWORD")`
devuelve un `Str` plano que podés tipear en cualquier `print()` o
`logger.info()` por accidente. **Las fugas de secrets en logs son la
fuente #1 de incidentes de seguridad menores**. Fitz lo previene a
nivel del checker: si pasás un `Secret<T>` a un kwarg de `log.*`,
se redacta automático.

**Cross-link**: [cap 32 de la guía](../../guide.md#32-variables-de-entorno)
con detalle de `env()`/`env_or()` builtins.

---

## Por qué Fitz es distinto

Comparalo con el patrón típico:

```python
# Python — fácil de filtrar
import os

DB_PASS = os.getenv("DB_PASSWORD")
logger.info(f"connecting with pass={DB_PASS}")  # ❌ filtrado al log
```

```typescript
// TypeScript — mismo problema
const dbPass = process.env.DB_PASSWORD;
console.log(`connecting with pass=${dbPass}`);  // ❌ filtrado
```

```fitz
# Fitz — el checker bloquea el filtro
let db_pass = secret("DB_PASSWORD")
log.info("connecting", password: db_pass)
// → JSON output: {"...":"password":"***"}  ← automático
```

Comparativo:

| Feature                       | Python    | TypeScript | Go    | Spring | **Fitz** |
| ----------------------------- | --------- | ---------- | ----- | ------ | -------- |
| Leer env var                  | `os.getenv` | `process.env` | `os.Getenv` | `@Value` | `config("X", default)` |
| Default value built-in        | ⚠ truthy  | ⚠ default arg | ⚠ check  | ✅ | ✅ |
| Tipo opaco para secrets       | ❌        | ❌         | ❌    | ⚠ con lib | ✅ **`Secret<T>` built-in** |
| Redacción automática en logs  | ❌        | ❌         | ❌    | ⚠ con lib | ✅ **checker + runtime** |
| Validación estática           | ❌ runtime | ❌ runtime | ❌    | ✅ | ✅ **checker rechaza el filtro** |
| .env loader built-in          | `python-dotenv` | `dotenv` | `godotenv` | ❌ | ✅ **`load_env(path)`** |

El diferencial real: **el tipo `Secret<T>` es opaco** — no podés
imprimir, loggear, ni serializar a JSON un `Secret<T>` directamente.
Para extraer el valor necesitás llamar `.expose()` explícitamente.
Es **opt-in difícil de hacer por error**.

---

## Paso 1 — Distinguir `config()` de `secret()`

Las dos leen env vars, pero con semánticas distintas:

- **`config(key, default)`** devuelve `Str` directo. Para
  configuración no-sensitive: hostnames, ports, log levels,
  feature flags. Si la env var no existe, usa el `default`.
- **`secret(key)`** devuelve `Secret<Str>` opaco. Para credenciales:
  passwords, API keys, tokens. Si la env var no existe → error
  (los secrets NO tienen default silencioso — la app debe abortar
  si no están configurados).

```fitz
let db_host = config("DB_HOST", "localhost")   // Str — público
let db_port = config("DB_PORT", "5432")        // Str — público
let db_pass = secret("DB_PASSWORD")             // Secret<Str> — opaco
```

**Regla práctica**: si te incomoda ver el valor en un log, usá
`secret()`. Si está bien que el valor aparezca en logs ("conectando a
db.prod.example.com"), usá `config()`.

---

## Paso 2 — Trabajar con `Secret<T>`

`Secret<T>` es un tipo opaco. **No podés hacer nada con él** salvo:

1. **Pasarlo a un kwarg de log** — se redacta automático.
2. **Llamar `.expose() -> T`** explícitamente para extraer el valor.
3. **Pasarlo a un context que requiere `Secret<T>`** (caso futuro:
   `db.connect(url_with_secret)`).

```fitz
let pass = secret("DB_PASS")

log.info("login attempt", password: pass)   // → "password":"***"

let raw = pass.expose()                      // Str — escapas al world
let url = "postgres://user:{raw}@db:5432/app"
let conn = db.connect(url)?
```

**Por qué `.expose()`**: si querés que un valor sensitive salga del
tipo opaco (ej: para construir una URL de DB), tenés que escribir
`.expose()` explícitamente. Es visible en code review. Si alguien
hace una PR con `pass.expose()` adentro de un `log.info(...)`, el
reviewer lo nota inmediato.

**Sin `.expose()`**, el checker rechaza:

```fitz
let pass = secret("DB_PASS")
print(pass)               // ❌ Error: `print(Secret<Str>)` no se puede formatear directo
print(pass.expose())      // OK — pero feo a propósito
```

---

## Paso 3 — Redacción automática en estructuras

`Secret<T>` se redacta recursivamente cuando cae adentro de:

- **Kwargs directos** del `log.*`.
- **`List<Secret<T>>`** (lista de secrets).
- **`Map<Str, Secret<T>>`** (mapa con secrets como values).
- **`Instance` con field `Secret<T>`** (typed object con un field
  sensitive).

```fitz
let creds = {
    "db_user": "admin",
    "db_pass": secret("DB_PASS").expose()  // ❌ ya expusiste, no se redacta
}
log.info("config", creds: creds)
// → "creds":{"db_user":"admin","db_pass":"actualvalue"}  ← FILTRADO

let creds_safe = {
    "db_user": "admin",
    "db_pass": secret("DB_PASS")  // Sin .expose() — mantiene tipo opaco
}
log.info("config", creds: creds_safe)
// → "creds":{"db_user":"admin","db_pass":"***"}  ← redactado
```

**Lección**: `.expose()` "extrae" el valor del Secret. Una vez
extraído, el sistema de tipos no puede protegerlo más. Llamá
`.expose()` lo más tarde posible, idealmente sólo cuando vas a usar
el valor (construir URL, mandar a la DB).

---

## Paso 4 — Cargar `.env` para dev local

En desarrollo local, querés tener un `.env` con tus credenciales
sin tener que `export` cada vez. Built-in `load_env(path) ->
Result<Null>`:

```fitz
@server(3000)
fn main() {
    // Cargar .env si existe (en prod no debería; usá env vars del orchestrator).
    let _ = load_env(".env")  // Result<Null> — ignoramos el Err
    let db_pass = secret("DB_PASS")
    // ...
}
```

Crea un `.env` adyacente al binario:

```bash
# .env (NUNCA commitear)
DB_HOST=localhost
DB_PORT=5432
DB_USER=fitz
DB_PASS=supersecret-local-dev
```

Y un `.env.example` que SÍ committeás (sin valores sensitive):

```bash
# .env.example (committeado, muestra qué vars necesita la app)
DB_HOST=
DB_PORT=
DB_USER=
DB_PASS=
```

`load_env()` parsea el formato estándar (`KEY=VALUE` per line,
comentarios con `#`, comillas opcionales). Sobreescribe env vars
existentes con los valores del file.

> **Importante**: en producción NO usás `load_env(".env")`. Las env
> vars deben venir del orchestrator (K8s Secret, fly secrets,
> Docker compose env, etc.). El `.env` es solo para dev local.

---

## Paso 5 — K8s secrets

En Kubernetes el patrón canónico es declarar un `Secret` separado y
montarlo como env vars:

```yaml
# k8s-secret.yaml
apiVersion: v1
kind: Secret
metadata:
  name: mi-app-secrets
type: Opaque
stringData:
  DB_PASS: "supersecret-prod"
  JWT_SECRET: "anothersupersecret"
```

```yaml
# k8s-deployment.yaml
spec:
  containers:
    - name: app
      image: mi-app:v1
      envFrom:
        - secretRef:
            name: mi-app-secrets
      env:
        - name: DB_HOST
          value: "db.prod.example.com"  # config, no secret
        - name: DB_PORT
          value: "5432"
```

Aplicalo:

```bash
$ kubectl apply -f k8s-secret.yaml
$ kubectl apply -f k8s-deployment.yaml
```

Tu app Fitz lee `secret("DB_PASS")` y `config("DB_HOST", ...)` de
forma transparente. No sabe que vienen de un K8s Secret, solo ve env
vars.

**Cuidado**: los K8s Secrets están en etcd codificados en base64 por
default (NO encriptados). Para encriptación real, usá:

- **Sealed Secrets** (Bitnami) — encripta a nivel de operator.
- **External Secrets Operator** — sincroniza desde HashiCorp Vault /
  AWS Secrets Manager / etc.
- **CSI Driver Secrets Store** — monta secrets como volúmenes con
  rotación.

---

## Paso 6 — fly.io / Railway / Heroku patterns

Para deploys "click and deploy" (sin K8s), los proveedores tienen
sus propios secret managers:

```bash
# fly.io
$ fly secrets set DB_PASS=supersecret
$ fly secrets set JWT_SECRET=anothersecret
$ fly deploy

# Railway
$ railway variables set DB_PASS=supersecret
$ railway up

# Heroku
$ heroku config:set DB_PASS=supersecret
$ git push heroku main
```

Todos terminan inyectando env vars al container. Tu app Fitz lo
consume con `secret()` igual que en K8s o local.

---

## Validación del cap

Probá end-to-end:

```bash
# 1. Local con .env
$ cat > .env <<EOF
DB_HOST=localhost
DB_PASS=test-pass
APP_NAME=demo-local
EOF

$ ./mi-app
{"timestamp":"...","msg":"server listo","app":"demo-local"}

# 2. Verificá redacción en logs
$ curl localhost:3000/admin -H "Authorization: Bearer demo-admin"
{"timestamp":"...","msg":"stats request","admin":"Ada","password":"***"}
                                                            ^^^
                                  redactado automático aunque pasaste secret() en kwargs

# 3. Probá K8s (si tenés cluster)
$ kubectl create secret generic mi-app-secrets \
    --from-literal=DB_PASS=k8s-pass \
    --from-literal=JWT_SECRET=k8s-jwt
$ kubectl apply -f deployment.yaml
$ kubectl logs deployment/mi-app
{"...","app":"demo-prod","host":"db.svc.cluster.local"}

# 4. Verificá que el binario NO contiene el secret (search en strings)
$ strings ./mi-app | grep -i "supersecret"  # debería NO matchear
```

Si los 4 pasos pasan, dominaste secrets management.

---

## Próximo: M8.C4 — Deploy avanzado con Docker

Tu app ahora maneja secrets sin filtrarlos. La última pieza:
**llevarla a producción con `fitz docker init`**, healthz/readyz para
K8s, SIGTERM drain, y patterns 12-factor.

→ [M8.C4 — Deploy avanzado: Docker, healthz, K8s](c4-deploy-docker-k8s.md)
