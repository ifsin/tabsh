#!/bin/sh
set -e

REPO="ifsin/tabsh"
BIN="tabsh"
INSTALL_DIR="/usr/local/bin"

os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)

case "$arch" in
  x86_64)  arch="amd64" ;;
  aarch64|arm64) arch="arm64" ;;
  *) echo "Unsupported architecture: $arch" >&2; exit 1 ;;
esac

if [ "$os" != "linux" ]; then
  echo "This script is for Linux only. On macOS, use: brew install ifsin/tap/tabsh" >&2
  exit 1
fi

ASSET="${BIN}-${os}-${arch}"
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$LATEST" ]; then
  echo "Failed to fetch latest release" >&2
  exit 1
fi

URL="https://github.com/${REPO}/releases/download/${LATEST}/${ASSET}"

echo "Installing tabsh ${LATEST} (${os}/${arch})..."
curl -fsSL "$URL" -o "/tmp/${ASSET}"
chmod +x "/tmp/${ASSET}"

if [ -w "$INSTALL_DIR" ]; then
  mv "/tmp/${ASSET}" "${INSTALL_DIR}/${BIN}"
else
  sudo mv "/tmp/${ASSET}" "${INSTALL_DIR}/${BIN}"
fi

echo "Installed to ${INSTALL_DIR}/${BIN}"
