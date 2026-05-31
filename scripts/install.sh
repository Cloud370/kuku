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
  darwin-aarch64|darwin-arm64)    PLATFORM="darwin-aarch64" ;;
  *)
    echo "Unsupported platform: ${OS}-${ARCH}"
    echo "Supported: linux-x86_64, darwin-aarch64"
    exit 1
    ;;
esac

BIN_DIR="${KUKU_HOME}/bin"
CACHE_DIR="${KUKU_HOME}/cache"
mkdir -p "$BIN_DIR" "$CACHE_DIR"

# ── Fetch manifest ──
echo "Fetching latest release info..."
MANIFEST=$(curl -fsSL "$SOURCE_URL")
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
    exit 0
  fi
  echo "Updating kuku $CURRENT -> $MANIFEST_VERSION"
else
  echo "Installing kuku $MANIFEST_VERSION"
fi

# ── Extract download URL and sha256 for platform ──
if command -v jq >/dev/null 2>&1; then
  URL=$(echo "$MANIFEST" | jq -r ".platforms.\"$PLATFORM\".url")
  SHA256=$(echo "$MANIFEST" | jq -r ".platforms.\"$PLATFORM\".sha256")
else
  URL=$(echo "$MANIFEST" | grep -A5 "\"$PLATFORM\"" | grep '"url"' | sed 's/.*"url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
  SHA256=$(echo "$MANIFEST" | grep -A5 "\"$PLATFORM\"" | grep '"sha256"' | sed 's/.*"sha256"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
fi

if [ -z "$URL" ]; then
  echo "No download URL found for platform $PLATFORM"
  exit 1
fi

# ── Download ──
ARCHIVE_NAME="kuku-${PLATFORM}.tar.gz"
CACHE_FILE="${CACHE_DIR}/${ARCHIVE_NAME}"
echo "Downloading $URL ..."
curl -fsSL "$URL" -o "$CACHE_FILE"

# ── Verify SHA256 ──
if [ -n "$SHA256" ]; then
  echo "Verifying checksum..."
  if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL=$(sha256sum "$CACHE_FILE" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    ACTUAL=$(shasum -a 256 "$CACHE_FILE" | awk '{print $1}')
  else
    echo "Warning: no sha256 tool found, skipping verification"
    ACTUAL="$SHA256"
  fi
  if [ "$ACTUAL" != "$SHA256" ]; then
    echo "Checksum mismatch: expected $SHA256, got $ACTUAL"
    rm -f "$CACHE_FILE"
    exit 1
  fi
fi

# ── Extract ──
echo "Extracting to $BIN_DIR..."
tar xzf "$CACHE_FILE" -C "$BIN_DIR"
chmod +x "$BIN_DIR/kuku"

# ── PATH check ──
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo ""
    echo "Add kuku to your PATH:"
    echo "  export PATH=\"$BIN_DIR:\$PATH\""
    echo ""
    echo "Add this to your shell profile (~/.bashrc, ~/.zshrc, etc.)"
    ;;
esac

echo "kuku $MANIFEST_VERSION installed successfully."
