#!/usr/bin/env bash
# Deploy vaporslop_tournament to REMOTE_DIR: systemd vaporslop-tournament.service (port 3089 via PORT env).
set -euo pipefail

cd "$(dirname "$0")"

HOST="${DEPLOY_HOST:-root@45.77.218.179}"
REMOTE_DIR="/root/vaporslop-tournament"

# Rclone remote is only a LABEL for server+credentials host/user (not "one app per remote").
# One SFTP remote to this box covers every project: paths after ':' differ (/root/tradscape vs /root/vaporslop-tournament).
# Easiest: keep using your existing remote (default tradscape matches older deploy scripts).
# Optional: duplicate that remote in `rclone config` and name it vaporslop-tournament for clarity — same host/user only.
RCLONE_REMOTE="${RCLONE_REMOTE:-tradscape}"
RCLONE_DEST="${RCLONE_REMOTE}:${REMOTE_DIR}"

ROOT="$(pwd)"

cargo zigbuild --target x86_64-unknown-linux-gnu --release

SERVICE="vaporslop-tournament.service"
BIN_NAME="vaporslop_tournament"

ssh "$HOST" "systemctl stop $SERVICE || true"

rclone copyto \
	"target/x86_64-unknown-linux-gnu/release/$BIN_NAME" \
	"${RCLONE_DEST}/$BIN_NAME"

rclone sync "${ROOT}/static/" "${RCLONE_DEST}/static/"

if [[ -d "${ROOT}/assets" ]]; then
	rclone sync "${ROOT}/assets/" "${RCLONE_DEST}/assets/"
fi

ssh "$HOST" "chmod +x $REMOTE_DIR/$BIN_NAME && systemctl start $SERVICE"
