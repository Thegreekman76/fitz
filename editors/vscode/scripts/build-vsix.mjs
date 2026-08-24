// build-vsix.mjs — Orquesta el build de la extensión VSCode para una
// plataforma específica, produciendo un `.vsix` con el binario
// `fitz-lsp` bundleado en `server/`.
//
// Pasos:
//   1. `cargo build --release --features lsp --bin fitz-lsp [--target <triple>]`.
//   2. Copiar el binario producido a `editors/vscode/server/fitz-lsp[.exe]`.
//   3. `npm run compile` (TypeScript → JavaScript).
//   4. `npx @vscode/vsce package --target <vsce-target>` — genera
//      `fitz-language-X.Y.Z-<vsce-target>.vsix`.
//
// Args:
//   --target <vsce-target>   Vsce target (`win32-x64`, `darwin-arm64`,
//                             `linux-x64`, etc). Default: plataforma actual
//                             detectada via `process.platform`+`process.arch`.
//   --rust-target <triple>   Override del Rust triple si el default del
//                             vsce-target no aplica. Casos: usar GNU toolchain
//                             en Windows (`x86_64-pc-windows-gnu` en lugar
//                             del MSVC default).
//
// Uso típico (build local de tu plataforma):
//   npm run build:vsix
//
// Build cross-platform (requiere `rustup target add <triple>` previo):
//   node scripts/build-vsix.mjs --target linux-x64
//
// Output: `editors/vscode/fitz-language-X.Y.Z-<target>.vsix`.

import { execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const extDir = path.join(__dirname, "..");
const repoRoot = path.join(extDir, "..", "..");

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

const args = process.argv.slice(2);
function argValue(name) {
  const idx = args.findIndex((a) => a === name);
  return idx >= 0 ? args[idx + 1] : undefined;
}

// ---------------------------------------------------------------------------
// Mapping vsce-target → Rust triple por defecto + nombre del binario
// ---------------------------------------------------------------------------

const PLATFORM_DEFAULTS = {
  "win32-x64":   { rustTarget: "x86_64-pc-windows-msvc",     exe: "fitz-lsp.exe" },
  "win32-arm64": { rustTarget: "aarch64-pc-windows-msvc",    exe: "fitz-lsp.exe" },
  "linux-x64":   { rustTarget: "x86_64-unknown-linux-gnu",   exe: "fitz-lsp" },
  "linux-arm64": { rustTarget: "aarch64-unknown-linux-gnu",  exe: "fitz-lsp" },
  "darwin-x64":  { rustTarget: "x86_64-apple-darwin",        exe: "fitz-lsp" },
  "darwin-arm64":{ rustTarget: "aarch64-apple-darwin",       exe: "fitz-lsp" },
};

function detectCurrentTarget() {
  const platform = process.platform; // 'win32' | 'darwin' | 'linux'
  const arch = process.arch;         // 'x64' | 'arm64' | ...
  return `${platform}-${arch}`;      // matchea con `win32-x64`, `darwin-arm64`, ...
}

const vsceTarget = argValue("--target") ?? detectCurrentTarget();
if (!PLATFORM_DEFAULTS[vsceTarget]) {
  console.error(
    `Unknown target: ${vsceTarget}. Supported: ${Object.keys(PLATFORM_DEFAULTS).join(", ")}`,
  );
  process.exit(1);
}

const rustTarget =
  argValue("--rust-target") ?? PLATFORM_DEFAULTS[vsceTarget].rustTarget;
const exeName = PLATFORM_DEFAULTS[vsceTarget].exe;

// Si target == plataforma actual → cargo build sin --target (más
// rápido, no requiere toolchain instalada del target). Sino, asume
// que el user corrió `rustup target add <triple>` previamente.
const useExplicitTarget = vsceTarget !== detectCurrentTarget();

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

console.log(`▶ vsce target  : ${vsceTarget}`);
console.log(`  Rust target  : ${rustTarget}${useExplicitTarget ? "" : " (native)"}`);
console.log(`  Binary       : ${exeName}`);
console.log("");

// 1. Cargo build.
const cargoArgs = [
  "build",
  "--release",
  "--features",
  "lsp",
  "--bin",
  "fitz-lsp",
];
if (useExplicitTarget) {
  cargoArgs.push("--target", rustTarget);
}
console.log(`▶ cargo ${cargoArgs.join(" ")}`);
execSync(`cargo ${cargoArgs.join(" ")}`, { stdio: "inherit", cwd: repoRoot });

// 2. Copiar binario al `server/`.
const builtBinaryPath = useExplicitTarget
  ? path.join(repoRoot, "target", rustTarget, "release", exeName)
  : path.join(repoRoot, "target", "release", exeName);
const serverDir = path.join(extDir, "server");
fs.mkdirSync(serverDir, { recursive: true });
const dstBinaryPath = path.join(serverDir, exeName);
fs.copyFileSync(builtBinaryPath, dstBinaryPath);
console.log(`▶ Copied ${path.relative(repoRoot, builtBinaryPath)} → ${path.relative(repoRoot, dstBinaryPath)}`);

// 3. TypeScript compile.
console.log("▶ npm run compile");
execSync("npm run compile", { stdio: "inherit", cwd: extDir });

// 4. vsce package con target. Preferí el `vsce` instalado en node_modules
//    (rápido, sin red); `npx --yes` fuerza una resolución por red que se
//    cuelga. Fallback a npx sólo si no está instalado localmente.
const localVsce = path.join(
  extDir, "node_modules", ".bin",
  process.platform === "win32" ? "vsce.cmd" : "vsce",
);
const vsceCmd = fs.existsSync(localVsce)
  ? `"${localVsce}"`
  : "npx --yes @vscode/vsce";
console.log(`▶ ${vsceCmd} package --target ${vsceTarget}`);
execSync(`${vsceCmd} package --target ${vsceTarget}`, {
  stdio: "inherit",
  cwd: extDir,
});

console.log("");
console.log(`✓ Done. .vsix en editors/vscode/ con sufijo -${vsceTarget}.`);
