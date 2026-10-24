#!/usr/bin/bash
set -euo pipefail
set -x

container=""

function cleanup() {
    local status=$?
    if [[ -n "$container" ]]; then
        if (( status != 0 )); then
            podman inspect --format '{{json .State}}' "$container" || true
            podman logs "$container" || true
        fi
        local cleanup_status=0
        podman stop "$container" -i || cleanup_status=$?
        podman rm "$container" -f || cleanup_status=$?
        if (( status == 0 )); then
            status=$cleanup_status
        fi
    fi
    exit "$status"
}

trap cleanup EXIT
trap 'exit 143' TERM

container=$(podman run -d \
    -v "${PWD}/autobahn/config:/config" \
    -v "${PWD}/autobahn/reports:/reports" \
    -p 9001:9001 \
    --security-opt label=disable \
    --name autobahn \
    crossbario/autobahn-testsuite)

python3 scripts/wait_for_autobahn.py

target/release/examples/autobahn_client
