# fitz installer -- Windows (PowerShell 5.1+)
#
# Uso con pipe (recomendado):
#   irm https://thegreekman76.github.io/fitz/install.ps1 | iex
#
#   # Pasar opciones via env vars antes del iex:
#   $env:FITZ_VERSION = "v0.11.1"; irm https://thegreekman76.github.io/fitz/install.ps1 | iex
#   $env:FITZ_PREFIX  = "C:\Tools\fitz"; irm https://thegreekman76.github.io/fitz/install.ps1 | iex
#   $env:FITZ_ACTION  = "uninstall"; irm https://thegreekman76.github.io/fitz/install.ps1 | iex
#
# Uso bajando el script primero (acepta flags nativos):
#   irm https://thegreekman76.github.io/fitz/install.ps1 -OutFile install.ps1
#   .\install.ps1                       # instala ultima version
#   .\install.ps1 -Version v0.11.1      # instala version especifica
#   .\install.ps1 -Prefix C:\Tools\fitz # prefix custom
#   .\install.ps1 -Uninstall            # desinstala
#
# Plataformas: solo win32-x64. Windows ARM64 no se publica pre-compilado.
# Por defecto instala en %USERPROFILE%\.fitz\bin\{fitz.exe, fitz-lsp.exe}
# y agrega ese dir al PATH del User (persistente, sin admin).
#
# Nota: los mensajes de output van en ASCII puro (sin acentos) para
# que rendericen bien en consolas con codepage legacy (cp850/cp1252)
# default en Windows 10/11 con region es-AR. El flujo funciona en
# Windows Terminal / VSCode terminal (UTF-8) igual.

[CmdletBinding()]
param(
  [string] $Version = $env:FITZ_VERSION,
  [string] $Prefix  = $env:FITZ_PREFIX,
  [switch] $Uninstall
)

$ErrorActionPreference = 'Stop'
$ProgressPreference    = 'SilentlyContinue'  # acelera Invoke-WebRequest

$Repo = 'Thegreekman76/fitz'

if ([string]::IsNullOrWhiteSpace($Prefix)) {
  $Prefix = Join-Path $env:USERPROFILE '.fitz'
}

# Action: -Uninstall switch o env var FITZ_ACTION=uninstall.
$Action = if ($Uninstall.IsPresent -or $env:FITZ_ACTION -eq 'uninstall') { 'uninstall' } else { 'install' }

# Colores (solo si la consola los soporta).
$useColor = $Host.UI.RawUI -ne $null -and (-not $env:NO_COLOR)
function Write-Info($msg) {
  if ($useColor) { Write-Host "==> $msg" -ForegroundColor Cyan }
  else { Write-Host "==> $msg" }
}
function Write-Ok($msg) {
  if ($useColor) { Write-Host "[OK] $msg" -ForegroundColor Green }
  else { Write-Host "[OK] $msg" }
}
function Write-Warn2($msg) {
  if ($useColor) { Write-Host "warning: $msg" -ForegroundColor Yellow }
  else { Write-Host "warning: $msg" }
}
function Write-Err($msg) {
  if ($useColor) { Write-Host "error: $msg" -ForegroundColor Red }
  else { Write-Host "error: $msg" }
}

function Get-Target {
  $arch = $env:PROCESSOR_ARCHITECTURE
  # Bajo SysWOW64 (PowerShell 32-bit en Windows 64-bit),
  # PROCESSOR_ARCHITEW6432 trae la arch real.
  if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
  switch ($arch) {
    'AMD64' { return 'win32-x64' }
    'x86'   {
      Write-Err "Windows 32-bit no se publica pre-compilado."
      Write-Err "Compila desde fuente: https://github.com/$Repo#instalacion"
      exit 1
    }
    'ARM64' {
      Write-Err "Windows ARM64 no se publica pre-compilado actualmente."
      Write-Err "Compila desde fuente: https://github.com/$Repo#instalacion"
      exit 1
    }
    default {
      Write-Err "arquitectura no soportada: $arch"
      exit 1
    }
  }
}

function Resolve-Version {
  param([string] $Requested)
  if (-not [string]::IsNullOrWhiteSpace($Requested)) {
    # Aceptamos "0.11.1" o "v0.11.1".
    if ($Requested -match '^v') {
      return @{ Tag = $Requested; Plain = $Requested.Substring(1) }
    } else {
      return @{ Tag = "v$Requested"; Plain = $Requested }
    }
  }
  Write-Info 'resolviendo ultima version...'
  $apiUrl = "https://api.github.com/repos/$Repo/releases/latest"
  try {
    $resp = Invoke-RestMethod -Uri $apiUrl -UseBasicParsing -Headers @{
      'User-Agent' = 'fitz-installer'
      'Accept'     = 'application/vnd.github+json'
    }
  } catch {
    Write-Err "no pude consultar $apiUrl"
    Write-Err "puede ser rate limit de GitHub API (60 req/h sin auth). Reintenta en un rato o pasa -Version vX.Y.Z."
    Write-Err "detalle: $($_.Exception.Message)"
    exit 1
  }
  $tag = $resp.tag_name
  if ([string]::IsNullOrWhiteSpace($tag)) {
    Write-Err 'la respuesta no incluye tag_name'
    exit 1
  }
  return @{ Tag = $tag; Plain = $tag.TrimStart('v') }
}

function Download-And-Extract {
  param(
    [string] $Target,
    [string] $VersionTag,
    [string] $VersionPlain
  )
  $asset = "fitz-$VersionPlain-$Target.zip"
  $url   = "https://github.com/$Repo/releases/download/$VersionTag/$asset"
  $tmp   = Join-Path $env:TEMP "fitz-install-$([guid]::NewGuid().Guid)"
  New-Item -ItemType Directory -Path $tmp -Force | Out-Null
  $zipPath = Join-Path $tmp $asset
  Write-Info "bajando $asset..."
  try {
    Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing -Headers @{
      'User-Agent' = 'fitz-installer'
    }
  } catch {
    Write-Err "descarga fallida desde $url"
    Write-Err "verifica que el release $VersionTag existe en https://github.com/$Repo/releases"
    Write-Err "detalle: $($_.Exception.Message)"
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    exit 1
  }
  Write-Info 'extrayendo...'
  Expand-Archive -Path $zipPath -DestinationPath $tmp -Force
  return $tmp
}

# Busca un archivo por nombre en el dir top-level del extracto
# o en su unico subdir (release.yml empaqueta como `fitz-<v>-<target>/...`).
function Find-In-Extract {
  param([string] $TmpDir, [string] $Name)
  $top = Join-Path $TmpDir $Name
  if (Test-Path $top -PathType Leaf) { return $top }
  $found = Get-ChildItem -Path $TmpDir -Filter $Name -Recurse -File -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($found) { return $found.FullName }
  return $null
}

function Install-Files {
  param([string] $TmpDir, [string] $Prefix)
  $binDir = Join-Path $Prefix 'bin'
  New-Item -ItemType Directory -Path $binDir -Force | Out-Null
  $srcFitz = Find-In-Extract -TmpDir $TmpDir -Name 'fitz.exe'
  $srcLsp  = Find-In-Extract -TmpDir $TmpDir -Name 'fitz-lsp.exe'
  if (-not $srcFitz) {
    Write-Err "no se encontro fitz.exe adentro del zip"
    Write-Err "abri un issue: https://github.com/$Repo/issues"
    exit 1
  }
  Copy-Item -Path $srcFitz -Destination (Join-Path $binDir 'fitz.exe') -Force
  Write-Ok "instalado $(Join-Path $binDir 'fitz.exe')"
  if ($srcLsp) {
    Copy-Item -Path $srcLsp -Destination (Join-Path $binDir 'fitz-lsp.exe') -Force
    Write-Ok "instalado $(Join-Path $binDir 'fitz-lsp.exe')"
  } else {
    Write-Warn2 'fitz-lsp.exe no encontrado en el zip (version vieja?). El LSP de VSCode no va a funcionar hasta actualizar.'
  }
}

# PATH del USER (persistente, sin admin). Lectura/escritura con
# Environment.SetEnvironmentVariable; agregamos solo si no estaba.
function Add-To-User-Path {
  param([string] $Dir)
  $current = [Environment]::GetEnvironmentVariable('Path', 'User')
  if ([string]::IsNullOrEmpty($current)) { $current = '' }
  $parts = $current -split ';' | Where-Object { $_ -ne '' }
  $already = $parts | Where-Object { $_.TrimEnd('\').ToLowerInvariant() -eq $Dir.TrimEnd('\').ToLowerInvariant() }
  if ($already) {
    return $false  # ya estaba
  }
  $newPath = if ($current.EndsWith(';') -or $current -eq '') { "$current$Dir" } else { "$current;$Dir" }
  [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
  return $true
}

function Remove-From-User-Path {
  param([string] $Dir)
  $current = [Environment]::GetEnvironmentVariable('Path', 'User')
  if ([string]::IsNullOrEmpty($current)) { return $false }
  $parts = $current -split ';' | Where-Object {
    $_ -ne '' -and $_.TrimEnd('\').ToLowerInvariant() -ne $Dir.TrimEnd('\').ToLowerInvariant()
  }
  $newPath = ($parts -join ';')
  if ($newPath -eq $current) { return $false }
  [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
  return $true
}

function Invoke-Install {
  $target = Get-Target
  $resolved = Resolve-Version -Requested $Version
  $tag = $resolved.Tag
  $plain = $resolved.Plain
  Write-Info "instalando fitz $tag ($target) -> $Prefix\bin"
  $tmp = Download-And-Extract -Target $target -VersionTag $tag -VersionPlain $plain
  try {
    Install-Files -TmpDir $tmp -Prefix $Prefix
  } finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  }
  Write-Host ''
  Write-Ok "fitz $tag instalado en $Prefix\bin"

  $binDir = Join-Path $Prefix 'bin'
  $added = Add-To-User-Path -Dir $binDir
  if ($added) {
    Write-Host ''
    Write-Info "agregue $binDir al PATH del User (persistente)."
    Write-Warn2 'IMPORTANTE: cerra y reabri la terminal para que el PATH actualice. Procesos existentes no ven el cambio hasta reiniciar.'
  } else {
    Write-Host ''
    Write-Info "$binDir ya estaba en el PATH del User."
  }

  # Smoke directo contra el .exe (no depende del PATH refrescado).
  $fitzExe = Join-Path $binDir 'fitz.exe'
  if (Test-Path $fitzExe) {
    Write-Host ''
    Write-Info 'smoke:'
    & $fitzExe --version
  }
}

function Invoke-Uninstall {
  $binDir = Join-Path $Prefix 'bin'
  $removed = 0
  foreach ($bin in @('fitz.exe', 'fitz-lsp.exe')) {
    $full = Join-Path $binDir $bin
    if (Test-Path $full) {
      Remove-Item -Force $full
      Write-Ok "borrado $full"
      $removed++
    }
  }
  if ($removed -eq 0) {
    Write-Warn2 "no encontre binarios fitz en $binDir"
    Write-Host "Si usaste un -Prefix custom al instalar, pasa el mismo aca:"
    Write-Host '  $env:FITZ_PREFIX = "<ruta>"; $env:FITZ_ACTION = "uninstall"; irm https://thegreekman76.github.io/fitz/install.ps1 | iex'
    return
  }
  $pathRemoved = Remove-From-User-Path -Dir $binDir
  if ($pathRemoved) {
    Write-Ok "$binDir removido del PATH del User."
    Write-Warn2 'Cerra y reabri la terminal para que el PATH actualice.'
  }
  $cacheDir = Join-Path $env:USERPROFILE '.fitz\cache'
  if (Test-Path $cacheDir) {
    Write-Host ''
    Write-Warn2 "cache local en $cacheDir (deps de git, builds de cargo) NO se borro."
    Write-Host "Para limpiarla: Remove-Item -Recurse -Force '$cacheDir'"
  }
  Write-Host ''
  Write-Ok 'fitz desinstalado.'
}

switch ($Action) {
  'install'   { Invoke-Install }
  'uninstall' { Invoke-Uninstall }
  default {
    Write-Err "accion desconocida: $Action"
    exit 1
  }
}
