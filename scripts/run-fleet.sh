#!/usr/bin/env bash
#
# Build (if needed) and run the LANline hypervisor — one lanline-server per
# radio — with the SoapySDR / libclang environment already set up.
#
#     scripts/run-fleet.sh --config scripts/fleet.toml
#     scripts/run-fleet.sh --radio driver=hackrf --radio driver=plutosdr --tls
#
# Flags handled by this script (everything else is forwarded to the hypervisor):
#
#     --debug        cargo build/run in the dev profile (default: --release)
#     --no-build     skip cargo build, just run the existing binary
#     -h | --help    show this and exit
#
# The hypervisor locates lanline-server as a sibling of its own binary, so a
# plain `cargo build` of the workspace is enough. See docs/hypervisor.md.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

profile="release"
build=1
args=()

while [ $# -gt 0 ]; do
    case "$1" in
        --debug)    profile="debug"; shift ;;
        --no-build) build=0; shift ;;
        -h|--help)  awk 'NR>1 && /^#/ {sub(/^# ?/,""); print; next} NR>1 {exit}' "$here/run-fleet.sh"; exit 0 ;;
        *)          args+=("$1"); shift ;;
    esac
done

# shellcheck source=scripts/dev-env.sh
source "$here/dev-env.sh"

if [ "$build" -eq 1 ]; then
    if [ "$profile" = "release" ]; then
        cargo build --release --workspace
        bin="$repo/target/release/lanline-hypervisor"
    else
        cargo build --workspace
        bin="$repo/target/debug/lanline-hypervisor"
    fi
else
    bin="$repo/target/$profile/lanline-hypervisor"
    [ -x "$bin" ] || { echo "run-fleet.sh: $bin not built — drop --no-build" >&2; exit 1; }
fi

echo "run-fleet.sh: exec $bin ${args[*]}"
exec "$bin" "${args[@]}"
