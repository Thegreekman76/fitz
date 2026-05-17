# 16b — Package manager (cap. 16b de la guía)

Ejemplo runnable del **package manager** de Fitz (Fase 9.y).
Dos proyectos:

- **`greetings/`** — librería con dos fns `hola` y `formal`.
  Su `fitz.toml` declara `[lib].entry = "src/lib.fitz"`.
- **`greeter/`** — binario que importa la lib via path dep.
  Su `fitz.toml` declara `[bin].main = "src/main.fitz"` y
  `[dependencies] greetings = { path = "../greetings" }`.

## Correrlo

Desde la raíz del repo Fitz:

```bash
cd examples/guide/16b-pkg-manager/greeter
fitz run
```

Output esperado:

```
Hola, Fitz!
Buenas tardes, Patagonia.
```

Detrás de escena `fitz run` sin args:

1. Busca `fitz.toml` en el cwd → encuentra el de `greeter`.
2. Resuelve `[dependencies] greetings = { path = "../greetings" }`
   → lee el manifest de `greetings`, registra
   `greetings -> <abs>/greetings/src/lib.fitz` en el dep_registry.
3. Genera/actualiza `fitz.lock` (la primera vez se crea con la
   versión resuelta y la próxima corrida lo verifica byte-a-byte).
4. Carga `src/main.fitz` (el `[bin].main`) y lo evalúa.
5. Cuando el código hace `from greetings import hola, formal`,
   el loader consulta el dep_registry y carga el `lib.fitz`
   correspondiente.

## Compilar a binario

```bash
fitz build
```

Emite `greeter/target/release/greeter.exe` (Windows) o
`greeter/target/release/greeter` (Linux/macOS) — el binario
incluye el código de la lib + el del bin, sin Fitz instalado en
la máquina destino. Output idéntico al de `fitz run`.

## CLI del PM (referencia)

| Comando | Para qué |
|---|---|
| `fitz new <nombre>` | Crea proyecto nuevo en carpeta `<nombre>/` |
| `fitz init` | Convierte el cwd actual en un proyecto |
| `fitz add <name> --path ../foo` | Suma dep path al `fitz.toml` |
| `fitz add <name> --git URL --tag v1` | Suma dep git con tag o `--rev` |
| `fitz remove <name>` | Quita dep + sincroniza lockfile |
| `fitz update [name]` | Invalida cache git + re-resuelve |

Ver [cap 16b de la guía](../../../docs/guide.md#16b-package-manager)
para el detalle.
