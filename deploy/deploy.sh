#!/bin/sh
# Runs on the server as root, from the Publish workflow. Installs one release and goes back to the
# one before if the site doesn't come up. The workflow looks for DEPLOY_OK, because the Azure command
# that runs this reports success whatever happens here.
#   deploy.sh v0.1.12 <sha256 of the binary>
# Only the binary changes. Settings and service files still come from provision.sh.
set -eu

tag="${1:?give the release tag}"
want="${2:?give the sha256 of the binary}"
case "$tag" in
    *[!A-Za-z0-9.]*) echo "tag must be letters, numbers and dots" >&2; exit 1 ;;
esac
case "$want" in
    *[!0-9a-f]*) echo "sha256 must be hex" >&2; exit 1 ;;
esac

repo=CakeRepository/agentshangout
bin=/usr/local/bin/agenthangout
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The Commons reads the database, so a good answer means more than the port being open.
healthy() {
    for _ in $(seq 1 30); do
        curl -fsS -o /dev/null --max-time 2 http://127.0.0.1:8787/api/worlds/commons && return 0
        sleep 1
    done
    return 1
}

curl -fsSL --retry 3 -o "$work/agenthangout" \
    "https://github.com/$repo/releases/download/$tag/agenthangout-linux-x86_64"
echo "$want  $work/agenthangout" | sha256sum -c -

# Through the unit, so the copy belongs to the service's user and the nightly backup can replace it.
systemctl start agenthangout-backup.service

cp -p "$bin" "$bin.previous"
install -m 755 "$work/agenthangout" "$bin"
systemctl restart agenthangout

if healthy; then
    echo "DEPLOY_OK $tag"
    exit 0
fi

echo "$tag didn't come up, going back to the previous binary" >&2
install -m 755 "$bin.previous" "$bin"
systemctl restart agenthangout
if healthy; then
    echo "Back on the previous binary" >&2
else
    echo "The previous binary didn't come up either" >&2
fi
exit 1
