#!/usr/bin/env bash
set -euo pipefail

source_dir=$1
image=$2
job=$3
case "$job" in verify|browser) ;; *) exit 2 ;; esac
test "$(id -u)" = 0
test -d "$source_dir/.git"
test ! -e /var/run/docker.sock
mkdir -p /ci/home /ci/config /ci/cache /ci/docker
touch /ci/empty
export HOME=/ci/home XDG_CONFIG_HOME=/ci/config XDG_CACHE_HOME=/ci/cache DOCKER_CONFIG=/ci/docker
export DOCKER_HOST=unix:///var/run/docker.sock
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH GITHUB_TOKEN GH_TOKEN

# Delegate controllers inside the private cgroup namespace, as in moby/moby's hack/dind.
if test -f /sys/fs/cgroup/cgroup.controllers; then
    mkdir -p /sys/fs/cgroup/init
    enabled=false
    for attempt in {1..5}; do
        xargs -rn1 < /sys/fs/cgroup/cgroup.procs > /sys/fs/cgroup/init/cgroup.procs || true
        if sed -e 's/ / +/g' -e 's/^/+/' < /sys/fs/cgroup/cgroup.controllers > /sys/fs/cgroup/cgroup.subtree_control; then
            enabled=true
            break
        fi
    done
    test "$enabled" = true
fi

dockerd --host=unix:///var/run/docker.sock --group=root --data-root=/var/lib/docker --storage-driver=overlay2 > /ci/dockerd.log 2>&1 &
daemon_pid=$!
finish() {
    rc=$?
    trap - EXIT INT TERM
    kill -TERM "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
    tail -n 100 /ci/dockerd.log
    exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
ready=false
for attempt in {1..60}; do
    if docker info >/dev/null 2>&1; then ready=true; break; fi
    kill -0 "$daemon_pid" || exit 1
    sleep 1
done
test "$ready" = true

# The only host bind is the snapshot. The runner image enters via stdin.
docker load
docker image inspect "$image" >/dev/null
cd "$source_dir"
sha=$(git -c safe.directory="$source_dir" rev-parse HEAD)
node - "$sha" > /ci/event.json <<'JS'
const sha = process.argv[2];
const repo = { full_name: 'AdamsGH/yoink-core', name: 'yoink-core', owner: { login: 'AdamsGH' }, default_branch: 'master' };
console.log(JSON.stringify({ action: 'synchronize', number: 1, repository: repo, pull_request: {
  number: 1, head: { sha, ref: 'ci-local', repo }, base: { sha, ref: 'master', repo },
} }));
JS
chown -R 10001:10001 "$source_dir"

options='--user=10001:10001 --group-add=0 --cap-add=SYS_ADMIN --shm-size=1g --cpus=4 --memory=7g --memory-swap=7g --pids-limit=4096'
# SYS_ADMIN permits Chrome's namespace sandbox within the disposable daemon.
# Fail before the workflow if the host kernel cannot provide that sandbox.
docker run --rm --network=bridge $options --entrypoint bash "$image" -euo pipefail -c '
    test "$(id -u)" = 10001
    test -u /opt/google/chrome/chrome-sandbox
    google-chrome --version
    timeout 30 google-chrome --headless --disable-gpu --allow-chrome-scheme-url --dump-dom chrome://sandbox > /tmp/sandbox.html
    cat /tmp/sandbox.html
    node -e '\''const fs = require("fs"); const text = fs.readFileSync("/tmp/sandbox.html", "utf8").replace(/<[^>]*>/g, " ").replace(/\s+/g, " "); if (!/(Namespace sandbox|PID namespaces)\s+Yes/.test(text) || !/Seccomp-BPF sandbox\s+Yes/.test(text)) process.exit(1);'\''
'

# --bind deliberately keeps act's default checkout skip. Remote checkout cannot
# fetch the temporary snapshot commit. All other workflow steps stay unchanged.
act pull_request --workflows .github/workflows/ci.yml --job "$job" \
    --eventpath /ci/event.json --actor AdamsGH \
    --platform "ubuntu-24.04=$image" --container-architecture linux/amd64 \
    --bind --pull=false --rebuild=false --rm --network bridge \
    --no-cache-server --action-cache-path /ci/cache/actions \
    --container-daemon-socket /var/run/docker.sock --container-options "$options" \
    --env-file /ci/empty --secret-file /ci/empty --var-file /ci/empty --input-file /ci/empty
