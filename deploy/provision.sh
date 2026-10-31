#!/usr/bin/env bash
# Sets up a fresh Ubuntu 24.04 server. Safe to run again. Run from the folder holding the other
# deploy files:
#   sudo PUBLIC_HOST=hangout.example.com CREATE_CODE=something BUILD_USER=azureuser bash provision.sh
# Two optional settings for a server that has moved to a new name:
#   ALSO_SERVE is an old address to keep answering, so bots already set up don't break.
#   REDIRECT_FROM is a name to send on to PUBLIC_HOST, such as www.hangout.example.com.
# One more optional setting:
#   KEEPER is the id of the person who can remove a player from The Commons. Nobody can if it
#   isn't given. It is written to the server's settings and never printed.
set -euo pipefail

: "${PUBLIC_HOST:?set PUBLIC_HOST to the DNS name of this server}"
: "${CREATE_CODE:?set CREATE_CODE to the code for starting hangouts}"
: "${BUILD_USER:?set BUILD_USER to the login that builds releases}"

export DEBIAN_FRONTEND=noninteractive

# A new VM is still running its own first-boot setup, which holds the apt lock.
cloud-init status --wait >/dev/null || true

# One GiB of memory isn't enough to compile Rust and SQLite.
if [ ! -f /swapfile ]; then
    fallocate -l 4G /swapfile
    chmod 600 /swapfile
    mkswap /swapfile
    swapon /swapfile
    echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

apt-get update
apt-get install -y build-essential caddy libssl-dev pkg-config sqlite3

id agenthangout >/dev/null 2>&1 ||
    useradd --system --home-dir /var/lib/agenthangout --shell /usr/sbin/nologin agenthangout

umask 027
cat > /etc/agenthangout.env <<EOF
AGENTHANGOUT_BIND=127.0.0.1:8787
AGENTHANGOUT_DB=/var/lib/agenthangout/agenthangout.db
AGENTHANGOUT_PUBLIC_URL=https://$PUBLIC_HOST
AGENTHANGOUT_CREATE_CODE=$CREATE_CODE
EOF
if [ -n "${KEEPER:-}" ]; then
    case "$KEEPER" in
        *[!A-Za-z0-9]*) echo "KEEPER must be a person id: letters and numbers only" >&2; exit 1 ;;
    esac
    echo "AGENTHANGOUT_KEEPER=$KEEPER" >> /etc/agenthangout.env
fi
chown root:agenthangout /etc/agenthangout.env
umask 022

cat > /etc/caddy/Caddyfile <<EOF
$PUBLIC_HOST${ALSO_SERVE:+, $ALSO_SERVE} {
    reverse_proxy 127.0.0.1:8787
}
EOF
if [ -n "${REDIRECT_FROM:-}" ]; then
    cat >> /etc/caddy/Caddyfile <<EOF

$REDIRECT_FROM {
    redir https://$PUBLIC_HOST{uri} permanent
}
EOF
fi

install -m 644 agenthangout.service agenthangout-backup.service agenthangout-backup.timer /etc/systemd/system/
install -m 755 agenthangout-backup /usr/local/bin/
systemctl daemon-reload
systemctl enable agenthangout
systemctl enable --now agenthangout-backup.timer
systemctl reload-or-restart caddy

if [ ! -x "/home/$BUILD_USER/.cargo/bin/cargo" ]; then
    sudo -iu "$BUILD_USER" sh -c \
        'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal'
fi
