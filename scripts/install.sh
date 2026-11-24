#!/bin/sh
set -e

# ── Defaults ──
KUKU_HOME="${KUKU_HOME:-$HOME/.kuku}"
SOURCE_URL="https://github.com/Cloud370/kuku/releases/latest/download/latest.json"
FORCE=0
VERSION=""

# ── Parse args ──
while [ $# -gt 0 ]; do
  case "$1" in
    --source) SOURCE_URL="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --version) VERSION="$2"; shift 2 ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

# ── Detect platform ──
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "${OS}-${ARCH}" in
  linux-x86_64|linux-amd64)       PLATFORM="linux-x86_64" ;;
  linux-aarch64|linux-arm64)      PLATFORM="linux-aarch64" ;;
  darwin-aarch64|darwin-arm64)    PLATFORM="darwin-aarch64" ;;
  *)
    echo "Unsupported platform: ${OS}-${ARCH}"
    echo "Supported: linux-x86_64, linux-aarch64, darwin-aarch64"
    exit 1
    ;;
esac

case "$KUKU_HOME" in
  /*) ;;
  *) KUKU_HOME="$PWD/$KUKU_HOME" ;;
esac
BIN_DIR="$KUKU_HOME/bin"
CACHE_DIR="$KUKU_HOME/cache"
mkdir -p "$BIN_DIR" "$CACHE_DIR"

configure_path() {
  case ":$PATH:" in
    *":$BIN_DIR:"*) return ;;
  esac

  case "${SHELL:-}" in
    */zsh) PROFILE="$HOME/.zshrc" ;;
    */bash)
      if [ -f "$HOME/.bashrc" ]; then
        PROFILE="$HOME/.bashrc"
      elif [ -f "$HOME/.bash_profile" ]; then
        PROFILE="$HOME/.bash_profile"
      else
        PROFILE="$HOME/.profile"
      fi
      ;;
    */fish) PROFILE="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ;;
    *) PROFILE="$HOME/.profile" ;;
  esac

  case "${SHELL:-}" in
    */fish)
      QUOTED_BIN_DIR=$(printf '%s' "$BIN_DIR" | sed "s/['\\\\]/\\\\&/g")
      PATH_LINE="set -gx PATH '$QUOTED_BIN_DIR' \$PATH"
      ;;
    *)
      QUOTED_BIN_DIR=$(printf '%s' "$BIN_DIR" | sed "s/'/'\\\\''/g")
      PATH_LINE="export PATH='$QUOTED_BIN_DIR':\"\$PATH\""
      ;;
  esac

  if [ -f "$PROFILE" ] && grep -qxF -- "$PATH_LINE" "$PROFILE"; then
    printf 'PATH entry already exists in %s\n' "$PROFILE"
  elif (mkdir -p "${PROFILE%/*}" && printf '\n# kuku\n%s\n' "$PATH_LINE" >> "$PROFILE") 2>/dev/null; then
    printf 'Added kuku to PATH in %s\n' "$PROFILE"
  else
    printf 'Could not update PATH in %s\n' "$PROFILE" >&2
  fi
  printf '\nTo use kuku in this terminal, run:\n  %s\n' "$PATH_LINE"
}

download() {
  if [ -t 2 ]; then
    curl -fL --progress-bar --connect-timeout 15 --retry 2 --retry-max-time 30 "$@"
  else
    curl -fsSL --connect-timeout 15 --retry 2 --retry-max-time 30 "$@"
  fi
}

valid_checksum() {
  [ "${#1}" -eq 64 ] && case "$1" in
    *[!0-9a-fA-F]*) return 1 ;;
    *) return 0 ;;
  esac
}

# ── Fetch manifest ──
echo "Fetching latest release info..."
MANIFEST=$(curl -fsSL --connect-timeout 15 --retry 2 --retry-max-time 30 -- "$SOURCE_URL")
MANIFEST_VERSION=$(echo "$MANIFEST" | grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"/\1/')

if [ -n "$VERSION" ] && [ "$MANIFEST_VERSION" != "$VERSION" ]; then
  echo "Requested version $VERSION but manifest has $MANIFEST_VERSION"
  exit 1
fi

# ── Check current version ──
if [ -x "$BIN_DIR/kuku" ] && [ "$FORCE" -eq 0 ]; then
  CURRENT=$("$BIN_DIR/kuku" --version 2>/dev/null | awk '{print $2}') || CURRENT=""
  if [ "$CURRENT" = "$MANIFEST_VERSION" ]; then
    echo "kuku $CURRENT is already the latest version."
    configure_path
    exit 0
  fi
  echo "Updating kuku $CURRENT -> $MANIFEST_VERSION"
else
  echo "Installing kuku $MANIFEST_VERSION"
fi

# ── Select a package before downloading ──
if command -v jq >/dev/null 2>&1; then
  PLATFORM_INFO=$(printf '%s\n' "$MANIFEST" | jq -c --arg platform "$PLATFORM" '.platforms[$platform]')
  package_field() {
    printf '%s\n' "$PLATFORM_INFO" | jq -r --arg field "$1" '.[$field] // empty'
  }
else
  PLATFORM_INFO=$(printf '%s\n' "$MANIFEST" | sed -n '/^[[:space:]]*"'"$PLATFORM"'"[[:space:]]*:[[:space:]]*{[[:space:]]*$/,/^[[:space:]]*}/p')
  package_field() {
    printf '%s\n' "$PLATFORM_INFO" |
      sed -n 's/^[[:space:]]*"'"$1"'"[[:space:]]*:[[:space:]]*"\([^"]*\)"[,]\{0,1\}[[:space:]]*$/\1/p;
              s/^[[:space:]]*"'"$1"'"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\)[,]\{0,1\}[[:space:]]*$/\1/p'
  }
fi

STAGE=$(mktemp -d "$BIN_DIR/.install.XXXXXXXX")
trap 'rm -rf "$STAGE"' EXIT

ARCHIVE_NAME="kuku-${PLATFORM}.tar.gz"
URL=$(package_field url)
SHA256=$(package_field sha256)
SIZE=$(package_field size)

if [ -z "$URL" ] || ! valid_checksum "$SHA256"; then
  echo "Missing download URL or SHA-256 checksum for $PLATFORM" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  digest() { sha256sum < "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
  digest() { shasum -a 256 < "$1" | awk '{print $1}'; }
else
  echo "Cannot verify the download without sha256sum or shasum." >&2
  exit 1
fi

CACHE_FILE="$STAGE/$ARCHIVE_NAME"
CACHED="$CACHE_DIR/$ARCHIVE_NAME"
SHA256=$(printf '%s' "$SHA256" | tr 'A-F' 'a-f')
if [ -f "$CACHED" ]; then
  cp "$CACHED" "$CACHE_FILE"
  if [ "$(digest "$CACHE_FILE")" != "$SHA256" ]; then
    rm -f "$CACHE_FILE"
  fi
fi

CACHED_PACKAGE=0
if [ -f "$CACHE_FILE" ]; then
  echo "Using verified cached $ARCHIVE_NAME."
  CACHED_PACKAGE=1
else
  echo "Downloading $ARCHIVE_NAME${SIZE:+ ($SIZE bytes)} ..."
  download -o "$CACHE_FILE" -- "$URL"
  echo "Verifying checksum..."
  ACTUAL=$(digest "$CACHE_FILE")
  if [ "$ACTUAL" != "$SHA256" ]; then
    echo "Checksum mismatch: expected $SHA256, got $ACTUAL" >&2
    exit 1
  fi
fi

echo "Installing in $BIN_DIR..."
tar -xzOf "$CACHE_FILE" kuku > "$STAGE/kuku"
if [ ! -s "$STAGE/kuku" ]; then
  echo "Archive is missing an executable kuku" >&2
  exit 1
fi
chmod +x "$STAGE/kuku"
mv -f "$STAGE/kuku" "$BIN_DIR/kuku"
if [ "$CACHED_PACKAGE" -eq 0 ]; then
  mv -f "$CACHE_FILE" "$CACHED"
fi

echo ""
echo "kuku $MANIFEST_VERSION installed successfully."
configure_path
echo ""
echo "  kuku --help          Show usage"
echo "  kuku --version       Show version"
echo "  kuku                 Set up a model or open the terminal client"
echo ""
echo "Docs: https://kuku.run"
