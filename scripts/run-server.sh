#!/usr/bin/env bash
#
# Build (if needed) and run the LANline server with the SoapySDR / libclang
# environment already set up.
#
#     scripts/run-server.sh                 # release build, HTTPS, auto device
#     scripts/run-server.sh --debug-tone    # any server flag passes straight through
#     scripts/run-server.sh --device driver=hackrf --enable-tx
#
# Flags handled by this script (everything else is forwarded to the server):
#
#     --debug        cargo build/run in the dev profile (default: --release)
#     --no-build     skip cargo build, just run the existing binary
#     --no-tls       serve plain HTTP (default is --tls: self-signed HTTPS, so
#                    the push-to-talk mic and location button work off-box)
#     --keep-stale   don't stop a previous instance holding the C2 port
#     -h | --help    show this and exit
#
# By default the script first stops any lanline-server / lanline-hypervisor
# still listening on the C2 port this run will bind (8730, or --c2-port /
# LANLINE_C2_PORT) so a restart doesn't fail with "address already in use".
# It never kills a non-LANline process on that port.
#
# Per-machine settings (device, ppm, --enable-tx, TLS cert paths, ...) go in
# scripts/run-server.env — copy scripts/run-server.env.sample to that path and
# edit. It is git-ignored. The server also reads every LANLINE_* variable on
# its own (see `lanline-server --help`), so exporting them in your shell works
# too.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

profile="release"
build=1
tls=1
kill_stale=1
server_args=()

while [ $# -gt 0 ]; do
    case "$1" in
        --debug)      profile="debug"; shift ;;
        --no-build)   build=0; shift ;;
        --no-tls)     tls=0; shift ;;
        --tls)        tls=1; shift ;;  # explicit; also the default
        --keep-stale) kill_stale=0; shift ;;
        -h|--help)    awk 'NR>1 && /^#/ {sub(/^# ?/,""); print; next} NR>1 {exit}' "$here/run-server.sh"; exit 0 ;;
        *)            server_args+=("$1"); shift ;;
    esac
done

# SoapySDR plugin path, libclang for the bindgen build, rpath to radioconda.
# shellcheck source=scripts/dev-env.sh
source "$here/dev-env.sh"

# Optional per-machine overrides (LANLINE_DEVICE, LANLINE_ENABLE_TX,
# LANLINE_TLS_CERT/KEY, LANLINE_FREQ_CORRECTION_PPM, ...). Git-ignored.
if [ -f "$here/run-server.env" ]; then
    echo "run-server.sh: loading $here/run-server.env"
    set -a
    # shellcheck disable=SC1091
    source "$here/run-server.env"
    set +a
fi

# HTTPS by default (--no-tls to opt out). Don't add the flag if the env file
# already set LANLINE_TLS.
if [ "$tls" -eq 1 ] && [ "${LANLINE_TLS:-}" != "true" ]; then
    server_args+=(--tls)
fi

# Free the C2 port a stale instance may still hold (so a restart doesn't die
# with "address already in use"). Resolve the port this run will bind: an
# explicit --c2-port on the command line wins, else LANLINE_C2_PORT, else the
# server's own default of 8730.
if [ "$kill_stale" -eq 1 ]; then
    c2_port="${LANLINE_C2_PORT:-8730}"
    for ((i = 0; i < ${#server_args[@]}; i++)); do
        case "${server_args[i]}" in
            --c2-port)   c2_port="${server_args[i+1]:-$c2_port}" ;;
            --c2-port=*) c2_port="${server_args[i]#*=}" ;;
        esac
    done

    listener_pids() {
        if command -v ss >/dev/null 2>&1; then
            ss -H -ltnp "sport = :$1" 2>/dev/null \
                | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u || true
        elif command -v lsof >/dev/null 2>&1; then
            lsof -tiTCP:"$1" -sTCP:LISTEN 2>/dev/null | sort -u || true
        fi
    }

    for pid in $(listener_pids "$c2_port"); do
        name="$(ps -p "$pid" -o comm= 2>/dev/null || true)"
        case "$name" in
            lanline-*)
                echo "run-server.sh: stopping stale $name (pid $pid) on C2 port $c2_port"
                kill "$pid" 2>/dev/null || true
                for _ in $(seq 1 50); do
                    kill -0 "$pid" 2>/dev/null || break
                    sleep 0.1
                done
                kill -9 "$pid" 2>/dev/null || true
                ;;
            "")
                ;;  # process vanished between the scan and now
            *)
                echo "run-server.sh: C2 port $c2_port is held by '$name' (pid $pid)," \
                     "not a LANline server — refusing to kill it. Free it or pass --c2-port." >&2
                exit 1
                ;;
        esac
    done
fi

if [ "$build" -eq 1 ]; then
    if [ "$profile" = "release" ]; then
        cargo build --release -p lanline-server
        bin="$repo/target/release/lanline-server"
    else
        cargo build -p lanline-server
        bin="$repo/target/debug/lanline-server"
    fi
else
    bin="$repo/target/$profile/lanline-server"
    [ -x "$bin" ] || { echo "run-server.sh: $bin not built — drop --no-build" >&2; exit 1; }
fi

echo "run-server.sh: exec $bin ${server_args[*]}"
exec "$bin" "${server_args[@]}"
