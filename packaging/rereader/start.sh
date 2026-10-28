#!/bin/sh
set -eu
app_dir=/mnt/us/rereader
export REREADER_DATA_DIR="${REREADER_DATA_DIR:-/var/local/rereader}"
binary="$app_dir/bin/rereader"
[ -x "$binary" ] || { echo "Missing executable: $binary" >&2; exit 1; }
[ -c /dev/fb0 ] || { echo "Kindle framebuffer unavailable" >&2; exit 1; }
mkdir -p "$app_dir/logs"
# mkdir is atomic; never start a second owner of the display.
lock=/tmp/rereader.lock
mkdir "$lock" 2>/dev/null || { echo "Already running, or stale lock: $lock" >&2; exit 1; }
paused=""
child=""
cleanup() {
    code=$?
    trap - EXIT HUP INT TERM
    if [ -n "$child" ]; then
        kill -TERM "$child" 2>/dev/null || :
        wait "$child" 2>/dev/null || :
    fi
    for pid in $paused; do kill -CONT "$pid" 2>/dev/null || :; done
    rmdir "$lock" 2>/dev/null || :
    exit "$code"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
# Own the display while the app runs; cleanup resumes only processes we paused.
# Set KINDLE_MANAGE_FRAMEWORK=0 for device debugging without pausing the framework.
if [ "${KINDLE_MANAGE_FRAMEWORK:-1}" = 1 ]; then
    for name in awesome cvm KPPMainApp; do
        for pid in $(pidof "$name" 2>/dev/null || :); do
            # Leave processes which were already stopped untouched.
            if [ -r "/proc/$pid/status" ] &&
                ! grep -q '^State:[[:space:]]*[Tt][[:space:]]' "/proc/$pid/status"; then
                if kill -STOP "$pid" 2>/dev/null; then
                    paused="$paused $pid"
                else
                    echo "Unable to pause framework process: $name ($pid)" >&2
                fi
            fi
        done
    done
fi
log="$app_dir/logs/$(date +%Y%m%d-%H%M%S).log"
"$binary" --device "$@" >"$log" 2>&1 &
child=$!
set +e
wait "$child"
code=$?
set -e
child=""
exit "$code"
