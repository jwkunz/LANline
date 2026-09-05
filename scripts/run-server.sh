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
#     -h | --help    show this and exit
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
server_args=()

while [ $# -gt 0 ]; do
    case "$1" in
        --debug)    profile="debug"; shift ;;
        --no-build) build=0; shift ;;
        --no-tls)   tls=0; shift ;;
        --tls)      tls=1; shift ;;  # explicit; also the default
        -h|--help)  awk 'NR>1 && /^#/ {sub(/^# ?/,""); print; next} NR>1 {exit}' "$here/run-server.sh"; exit 0 ;;
        *)          server_args+=("$1"); shift ;;
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
