#!/bin/sh
set -eu

REPO="monorkin/okuri"
BINARY="okuri"
ICON_SIZES="48 64 128 256"

# Detect OS
OS="$(uname -s)"
case "$OS" in
  Linux*) OS="linux" ;;
  *)
    echo "Error: Okuri is a GTK application for Linux, and this is $OS" >&2
    exit 1
    ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH="amd64" ;;
  *)
    echo "Error: no prebuilt binary for $ARCH" >&2
    echo "Build it yourself with 'cargo build --release -p okuri', or install from the AUR." >&2
    exit 1
    ;;
esac

# GTK and libadwaita are linked dynamically, so a machine without them downloads a binary that
# cannot start. Say so now rather than after the install, when the failure is a window that
# never opens.
if ! ldconfig -p 2>/dev/null | grep -q libadwaita-1; then
  echo "Error: GTK 4 and libadwaita are missing, and Okuri links against them" >&2
  echo "On Arch:   sudo pacman -S gtk4 libadwaita" >&2
  echo "On Debian: sudo apt install libgtk-4-1 libadwaita-1-0" >&2
  exit 1
fi

# Overridable so this can go somewhere other than the system, and so it can be tried out
# against a scratch directory without touching anything.
INSTALL_DIR="${INSTALL_DIR:-/usr/bin}"
SHARE_DIR="${SHARE_DIR:-/usr/share}"

echo "Detected: ${OS}/${ARCH}"
echo "Install directory: ${INSTALL_DIR}"

# Fetch latest release tag
echo "Fetching latest release..."
TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"//;s/".*//')"

if [ -z "$TAG" ]; then
  echo "Error: could not determine latest release" >&2
  exit 1
fi

echo "Latest release: ${TAG}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

# Anything that is not the binary comes from the tag rather than the release, so the icon and
# the desktop entry always match the version being installed.
RAW="https://github.com/${REPO}/raw/${TAG}"
ASSET="https://github.com/${REPO}/releases/download/${TAG}/${BINARY}-${OS}-${ARCH}"

echo "Downloading ${ASSET}..."
curl -fSL -o "${TMPDIR}/${BINARY}" "$ASSET"
chmod +x "${TMPDIR}/${BINARY}"

curl -fsSL -o "${TMPDIR}/${BINARY}.desktop" "${RAW}/packaging/${BINARY}.desktop"
for size in $ICON_SIZES; do
  curl -fsSL -o "${TMPDIR}/${BINARY}-${size}.png" "${RAW}/assets/icons/${BINARY}-${size}.png"
done

# One decision about sudo, made once, rather than a prompt per file.
if [ -w "$INSTALL_DIR" ]; then
  AS_ROOT=""
else
  echo "Need elevated permissions to install to ${INSTALL_DIR}"
  AS_ROOT="sudo"
fi

$AS_ROOT install -Dm755 "${TMPDIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
$AS_ROOT install -Dm644 "${TMPDIR}/${BINARY}.desktop" \
  "${SHARE_DIR}/applications/${BINARY}.desktop"

for size in $ICON_SIZES; do
  $AS_ROOT install -Dm644 "${TMPDIR}/${BINARY}-${size}.png" \
    "${SHARE_DIR}/icons/hicolor/${size}x${size}/apps/${BINARY}.png"
done

# Without these the entry is on disk but the launcher has not noticed it yet.
if command -v update-desktop-database >/dev/null 2>&1; then
  $AS_ROOT update-desktop-database -q "${SHARE_DIR}/applications" || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  $AS_ROOT gtk-update-icon-cache -qtf "${SHARE_DIR}/icons/hicolor" || true
fi

echo "Installed ${BINARY} ${TAG} to ${INSTALL_DIR}/${BINARY}"
echo "Okuri should now be in your app launcher."
