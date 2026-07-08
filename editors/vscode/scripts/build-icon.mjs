// build-icon.mjs — Genera los PNGs del logo desde los SVG fuente.
//
// Los SVG viven en `assets/` (single source of truth, en la raíz
// del repo). Generamos PNGs en tres sitios:
// - `assets/logo.png`           (256×256) — README y propósitos generales.
// - `editors/vscode/icon.png`   (256×256) — .vsix de la extensión
//                                            (Marketplace + sidebar).
// - `assets/logo-social.png`    (1280×640) — Social preview de GitHub
//                                             (se sube manual a
//                                              Settings → Social preview).
//
// Usa @resvg/resvg-js — bindings JS del renderer SVG en Rust de
// resvg. Más liviano que sharp (sin compilación nativa pesada),
// más confiable que cairosvg en Windows.
//
// Uso: `npm run build:icon` desde `editors/vscode/`.
//       (o `node scripts/build-icon.mjs` directo)

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { Resvg } from "@resvg/resvg-js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.join(__dirname, "..", "..", "..");

/**
 * Renderiza un SVG a PNG con la dimensión deseada y escribe el
 * resultado en cada uno de los `targets`.
 */
function render(svgFile, width, targets, { loadSystemFonts = false } = {}) {
  const svg = fs.readFileSync(svgFile, "utf8");
  const resvg = new Resvg(svg, {
    fitTo: { mode: "width", value: width },
    background: "rgba(0, 0, 0, 0)", // transparente
    font: { loadSystemFonts },
  });
  const png = resvg.render().asPng();
  for (const target of targets) {
    fs.writeFileSync(target, png);
    console.log(`Generated ${target} (${png.length} bytes)`);
  }
}

// Logo cuadrado 256×256 → README + .vsix de la extensión.
render(
  path.join(repoRoot, "assets", "logo.svg"),
  256,
  [
    path.join(repoRoot, "assets", "logo.png"),
    path.join(repoRoot, "editors", "vscode", "icon.png"),
  ],
);

// Social preview 1280×640 → GitHub Settings → Social preview (subida
// manual). `loadSystemFonts: true` porque el SVG usa <text> con
// `Segoe UI, Arial, Helvetica, sans-serif` y necesitamos que resvg
// encuentre alguna del sistema para renderizarlo.
render(
  path.join(repoRoot, "assets", "logo-social.svg"),
  1280,
  [path.join(repoRoot, "assets", "logo-social.png")],
  { loadSystemFonts: true },
);

// Variante inglesa del social preview 1280×640 → Mastodon header /
// social posts en inglés (Twitter/X, LinkedIn, Bluesky). Mismo diseño
// del SVG hermano, texto traducido.
render(
  path.join(repoRoot, "assets", "logo-social-en.svg"),
  1280,
  [path.join(repoRoot, "assets", "logo-social-en.png")],
  { loadSystemFonts: true },
);
