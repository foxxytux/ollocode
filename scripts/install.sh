#!/bin/sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
BIN_NAME="ollo-code"
TARGET_DIR="${HOME}/.local/bin"
TARGET_PATH="${TARGET_DIR}/${BIN_NAME}"

mkdir -p "${TARGET_DIR}"
cd "${ROOT_DIR}"
cargo build --release --locked
install -m 755 "target/release/${BIN_NAME}" "${TARGET_PATH}"
printf '%s\n' "Installed ${BIN_NAME} to ${TARGET_PATH}"
