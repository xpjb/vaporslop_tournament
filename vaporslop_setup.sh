#!/usr/bin/env bash

# One-time bootstrap: vaporslop-tournament.service under /root/vaporslop-tournament (listen PORT=3089).
TARGET_HOST="${DEPLOY_HOST:-45.77.218.179}"
TARGET_USER="root"
SERVICE_NAME="vaporslop-tournament.service"
REMOTE_DIR="/root/vaporslop-tournament"
BINARY_PATH="$REMOTE_DIR/vaporslop_tournament"

echo "Deploying $SERVICE_NAME to $TARGET_USER@$TARGET_HOST..."

ssh "$TARGET_USER@$TARGET_HOST" "bash -s" <<EOF
	set -e
	echo "Creating $REMOTE_DIR directory..."
	mkdir -p "$REMOTE_DIR"

	echo "Creating service file at /etc/systemd/system/$SERVICE_NAME..."
	cat > /etc/systemd/system/$SERVICE_NAME <<SERVICE_DEF
[Unit]
Description=Vaporslop tournament game server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Environment=PORT=3089
ExecStart=$BINARY_PATH
Restart=always
User=root
WorkingDirectory=$REMOTE_DIR

[Install]
WantedBy=multi-user.target
SERVICE_DEF

	echo "Reloading systemd daemon..."
	systemctl daemon-reload

	echo "Enabling $SERVICE_NAME..."
	systemctl enable $SERVICE_NAME

	echo "Starting $SERVICE_NAME..."
	systemctl start $SERVICE_NAME

	echo "Current Status:"
	systemctl status $SERVICE_NAME --no-pager
EOF

echo "Bootstrap complete."
