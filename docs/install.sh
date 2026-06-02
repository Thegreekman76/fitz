#!/bin/sh
# fitz installer — Unix / macOS
#
# Uso:
#   curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh
#   curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --version v0.11.1
#   curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --prefix ~/.local
#   curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --uninstall
#
# Por defecto instala en ~/.fitz/bin/{fitz,fitz-lsp}. Sugiere agregar
# ese dir al PATH si no está; NO modifica .bashrc/.zshrc por sí solo.
#
# Plataformas soportadas:
#   - linux-x64    (Linux Intel/AMD 64-bit, GLIBC 2.35+)
#   - linux-arm64  (Raspberry Pi 4+, AWS Graviton, etc.)
#   - darwin-arm64 (macOS Apple Silicon M1/M2/M3/M4)
#
# macOS Intel y Windows ARM64 no se publican pre-compilados; en esos
# casos compilá desde fuente (ver Opción D del cap C1 del curso).

set -eu

REPO="Thegreekman76/fitz"
DEFAULT_PREFIX="${HOME}/.fitz"
PREFIX="${DEFAULT_PREFIX}"
VERSION=""
ACTION="install"

# Color helpers — solo si stdout es terminal interactiva.
if [ -t 1 ]; then
  YELLOW="$(printf '\033[33m')"
  RED="$(printf '\033[31m')"
  GREEN="$(printf '\033[32m')"
  BOLD="$(printf '\033[1m')"
  RESET="$(printf '\033[0m')"
else
  YELLOW=""; RED=""; GREEN=""; BOLD=""; RESET=""
fi

say()  { printf '%s\n' "$*"; }
info() { printf '%s==>%s %s\n' "${BOLD}" "${RESET}" "$*"; }
warn() { printf '%swarning:%s %s\n' "${YELLOW}" "${RESET}" "$*" >&2; }
err()  { printf '%serror:%s %s\n' "${RED}" "${RESET}" "$*" >&2; }
ok()   { printf '%s\xe2\x9c\x93%s %s\n' "${GREEN}" "${RESET}" "$*"; }

usage() {
  cat <<'EOF'
fitz installer

USO:
    install.sh [OPCIONES]

OPCIONES:
    --version <vX.Y.Z>   Instala una versión específica (default: última)
    --prefix <path>      Instala en <path>/bin (default: ~/.fitz)
    --uninstall          Elimina fitz del prefix
    --help, -h           Muestra esta ayuda

EJEMPLOS:
    # Instalar última versión
    curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh

    # Instalar versión específica
    curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --version v0.11.1

    # Instalar en un prefix custom
    curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --prefix ~/.local

    # Desinstalar
    curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --uninstall
EOF
}

# Parse args. Aceptamos `--flag value` y `--flag=value`.
while [ $# -gt 0 ]; do
  case "$1" in
    --version=*)   VERSION="${1#*=}"; shift ;;
    --version)     VERSION="${2:-}"; shift 2 ;;
    --prefix=*)    PREFIX="${1#*=}"; shift ;;
    --prefix)      PREFIX="${2:-}"; shift 2 ;;
    --uninstall)   ACTION="uninstall"; shift ;;
    --help|-h)     usage; exit 0 ;;
    *)             err "flag desconocido: $1"; say ""; usage; exit 1 ;;
  esac
done

if [ -z "$PREFIX" ]; then
  err "--prefix requiere un valor"; exit 1
fi
if [ "$ACTION" = "install" ] && [ -z "$VERSION" ]; then
  : # OK, resolveremos latest abajo
fi

# Variables que setean detect_target / resolve_version / download_and_extract.
target=""
ext=""
version_tag=""
version_plain=""
tmp_dir=""

detect_target() {
  os_uname="$(uname -s 2>/dev/null || echo unknown)"
  arch_uname="$(uname -m 2>/dev/null || echo unknown)"
  case "$os_uname" in
    Linux)  os_name="linux" ;;
    Darwin) os_name="darwin" ;;
    *)
      err "OS no soportado por el installer: $os_uname"
      err "Ver opciones manuales en https://github.com/${REPO}/releases"
      exit 1
      ;;
  esac
  case "$arch_uname" in
    x86_64|amd64) arch_name="x64" ;;
    aarch64|arm64) arch_name="arm64" ;;
    *)
      err "arquitectura no soportada: $arch_uname"
      exit 1
      ;;
  esac
  case "${os_name}-${arch_name}" in
    linux-x64)    target="linux-x64";    ext="tar.gz" ;;
    linux-arm64)  target="linux-arm64";  ext="tar.gz" ;;
    darwin-arm64) target="darwin-arm64"; ext="tar.gz" ;;
    darwin-x64)
      err "macOS Intel (x64) no se publica pre-compilado."
      err "Si tenés Mac M1/M2/M3/M4, este script debería detectarte como darwin-arm64."
      err "Si estás en Intel mac genuino, compilá desde fuente:"
      err "  git clone https://github.com/${REPO}.git && cd fitz && cargo build --release"
      exit 1
      ;;
    *)
      err "plataforma no soportada: ${os_name}-${arch_name}"
      exit 1
      ;;
  esac
}

resolve_version() {
  if [ -n "$VERSION" ]; then
    # Aceptamos --version 0.11.1 o --version v0.11.1.
    case "$VERSION" in
      v*) version_tag="$VERSION"; version_plain="${VERSION#v}" ;;
      *)  version_tag="v$VERSION"; version_plain="$VERSION" ;;
    esac
    return
  fi
  info "resolviendo última versión..."
  api_url="https://api.github.com/repos/${REPO}/releases/latest"
  raw=""
  if command -v curl >/dev/null 2>&1; then
    raw="$(curl -fsSL "$api_url" 2>/dev/null || true)"
  elif command -v wget >/dev/null 2>&1; then
    raw="$(wget -qO- "$api_url" 2>/dev/null || true)"
  else
    err "ni curl ni wget están instalados"
    exit 1
  fi
  if [ -z "$raw" ]; then
    err "no pude consultar $api_url"
    err "puede ser rate limit de la GitHub API (60 req/h sin auth). Reintentá en un rato o pasá --version <vX.Y.Z>."
    exit 1
  fi
  # Parseo defensivo del tag_name sin depender de jq.
  version_tag="$(printf '%s\n' "$raw" \
    | grep -E '"tag_name"' \
    | head -n 1 \
    | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  if [ -z "$version_tag" ]; then
    err "no pude parsear el tag de la última versión"
    err "pasá la versión manual con --version <vX.Y.Z>"
    exit 1
  fi
  version_plain="${version_tag#v}"
}

download_and_extract() {
  asset="fitz-${version_plain}-${target}.${ext}"
  url="https://github.com/${REPO}/releases/download/${version_tag}/${asset}"
  info "bajando ${asset}..."
  tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t fitz-install)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp_dir'" EXIT INT TERM
  if command -v curl >/dev/null 2>&1; then
    if ! curl -fSL --progress-bar "$url" -o "${tmp_dir}/${asset}"; then
      err "descarga fallida desde $url"
      err "verificá que el release ${version_tag} existe en https://github.com/${REPO}/releases"
      exit 1
    fi
  else
    if ! wget -O "${tmp_dir}/${asset}" "$url"; then
      err "descarga fallida desde $url"
      exit 1
    fi
  fi
  info "extrayendo..."
  ( cd "$tmp_dir" && tar -xzf "$asset" )
}

# Busca un binario en el dir top-level del tarball O en su único
# subdir (release.yml empaqueta como `fitz-<v>-<target>/{fitz,...}`,
# pero soportamos ambos shapes por defensa).
find_in_tmp() {
  name="$1"
  if [ -f "${tmp_dir}/${name}" ]; then
    printf '%s\n' "${tmp_dir}/${name}"
    return
  fi
  for candidate in "${tmp_dir}"/*/"${name}"; do
    if [ -f "$candidate" ]; then
      printf '%s\n' "$candidate"
      return
    fi
  done
}

install_files() {
  bin_dir="${PREFIX}/bin"
  mkdir -p "$bin_dir"
  src_fitz="$(find_in_tmp fitz || true)"
  src_lsp="$(find_in_tmp fitz-lsp || true)"
  if [ -z "$src_fitz" ]; then
    err "no se encontró el binario 'fitz' adentro del tarball"
    err "abrí un issue: https://github.com/${REPO}/issues"
    exit 1
  fi
  # `install -m` falla en algunas Darwin/Alpine si el target ya existe
  # como ejecutable corriendo. Usamos cp + chmod por portabilidad.
  cp -f "$src_fitz" "${bin_dir}/fitz"
  chmod 0755 "${bin_dir}/fitz"
  ok "instalado ${bin_dir}/fitz"
  if [ -n "$src_lsp" ]; then
    cp -f "$src_lsp" "${bin_dir}/fitz-lsp"
    chmod 0755 "${bin_dir}/fitz-lsp"
    ok "instalado ${bin_dir}/fitz-lsp"
  else
    warn "fitz-lsp no encontrado en el tarball (versión vieja?). El LSP de VSCode no va a funcionar hasta actualizar."
  fi
}

post_install_path_hint() {
  bin_dir="${PREFIX}/bin"
  case ":${PATH}:" in
    *":${bin_dir}:"*)
      return
      ;;
  esac
  say ""
  warn "${bin_dir} no está en tu PATH."
  say "Agregalo según tu shell (elegí una):"
  say ""
  say "  ${BOLD}bash${RESET}"
  say "    echo 'export PATH=\"${bin_dir}:\$PATH\"' >> ~/.bashrc && source ~/.bashrc"
  say ""
  say "  ${BOLD}zsh${RESET} (default en macOS)"
  say "    echo 'export PATH=\"${bin_dir}:\$PATH\"' >> ~/.zshrc && source ~/.zshrc"
  say ""
  say "  ${BOLD}fish${RESET}"
  say "    fish_add_path ${bin_dir}"
  say ""
  say "Después reabrí la terminal y ejecutá: ${BOLD}fitz --version${RESET}"
}

do_install() {
  detect_target
  resolve_version
  info "instalando fitz ${version_tag} (${target}) → ${PREFIX}/bin"
  download_and_extract
  install_files
  say ""
  ok "fitz ${version_tag} instalado en ${PREFIX}/bin"
  post_install_path_hint
  # Smoke si fitz es directamente ejecutable (no depende del PATH).
  if "${PREFIX}/bin/fitz" --version >/dev/null 2>&1; then
    say ""
    info "smoke:"
    "${PREFIX}/bin/fitz" --version
  fi
}

do_uninstall() {
  bin_dir="${PREFIX}/bin"
  removed=0
  for bin in fitz fitz-lsp; do
    if [ -f "${bin_dir}/${bin}" ]; then
      rm -f "${bin_dir}/${bin}"
      ok "borrado ${bin_dir}/${bin}"
      removed=$((removed + 1))
    fi
  done
  if [ "$removed" = 0 ]; then
    warn "no encontré binarios fitz en ${bin_dir}"
    say "Si usaste un --prefix custom al instalar, pasá el mismo acá:"
    say "  curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --uninstall --prefix <ruta>"
    exit 0
  fi
  # Limpieza opcional de cache local. NO la borramos sin avisar.
  cache_dir="${HOME}/.fitz/cache"
  if [ -d "$cache_dir" ]; then
    say ""
    warn "cache local en ${cache_dir} (deps de git, builds de cargo) NO se borró."
    say "Para limpiarla: ${BOLD}rm -rf ${cache_dir}${RESET}"
  fi
  say ""
  ok "fitz desinstalado."
  say "Si agregaste ${bin_dir} al PATH (~/.bashrc, ~/.zshrc, ...) ese cambio sigue ahí — quítalo a mano si querés."
}

case "$ACTION" in
  install)   do_install ;;
  uninstall) do_uninstall ;;
  *) err "acción interna desconocida: $ACTION"; exit 1 ;;
esac
