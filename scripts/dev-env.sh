#!/usr/bin/env bash
# Source this before building or running the server:
#
#     source scripts/dev-env.sh
#     cargo run -p sdr-c2-server
#
# It points the build and the resulting binary at the SoapySDR install that
# ships with radioconda, and at a libclang for soapysdr-sys' bindgen step.
#
# Override RADIOCONDA_PREFIX / LIBCLANG_PATH before sourcing if your layout
# differs.

RADIOCONDA_PREFIX="${RADIOCONDA_PREFIX:-$HOME/radioconda}"

if [ ! -d "$RADIOCONDA_PREFIX" ]; then
    echo "dev-env.sh: RADIOCONDA_PREFIX=$RADIOCONDA_PREFIX not found" >&2
    return 1 2>/dev/null || exit 1
fi

# Best-effort conda activate so SoapySDR's own relocation logic runs; the
# explicit exports below are what actually matter for a plain `cargo run`.
if [ -f "$RADIOCONDA_PREFIX/bin/activate" ]; then
    # shellcheck disable=SC1091
    source "$RADIOCONDA_PREFIX/bin/activate" 2>/dev/null || true
fi

# pkg-config: let soapysdr-sys discover SoapySDR 0.8.
export PKG_CONFIG_PATH="$RADIOCONDA_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"

# Bake an rpath to radioconda's libs into the binary instead of exporting
# LD_LIBRARY_PATH -- exporting it globally makes unrelated tools (curl, ...)
# pick up radioconda's libcurl/libssl and break. This does force a rebuild the
# first time it changes.
_RPATH_FLAG="-C link-arg=-Wl,-rpath,$RADIOCONDA_PREFIX/lib"
case "${RUSTFLAGS:-}" in
    *"$_RPATH_FLAG"*) : ;;
    "") export RUSTFLAGS="$_RPATH_FLAG" ;;
    *) export RUSTFLAGS="$RUSTFLAGS $_RPATH_FLAG" ;;
esac
unset _RPATH_FLAG

# Runtime: where SoapySDR looks for its device modules (hackrf, plutosdr, ...).
# Harmless to the rest of the shell; safe to also set this permanently.
export SOAPY_SDR_PLUGIN_PATH="$RADIOCONDA_PREFIX/lib/SoapySDR/modules0.8"

# bindgen (soapysdr-sys) needs a libclang. Prefer an explicit override, then a
# common system LLVM path, then whatever radioconda ships.
if [ -z "${LIBCLANG_PATH:-}" ]; then
    for _candidate in /usr/lib/llvm-21/lib /usr/lib/llvm-*/lib "$RADIOCONDA_PREFIX/lib"; do
        if ls "$_candidate"/libclang*.so* >/dev/null 2>&1; then
            export LIBCLANG_PATH="$_candidate"
            break
        fi
    done
    unset _candidate
fi

echo "dev-env.sh: RADIOCONDA_PREFIX=$RADIOCONDA_PREFIX"
echo "dev-env.sh: SOAPY_SDR_PLUGIN_PATH=$SOAPY_SDR_PLUGIN_PATH"
echo "dev-env.sh: LIBCLANG_PATH=${LIBCLANG_PATH:-<unset>}"
if command -v SoapySDRUtil >/dev/null 2>&1; then
    echo "dev-env.sh: $(SoapySDRUtil --info 2>/dev/null | grep -m1 'API Version' || echo 'SoapySDRUtil present')"
fi
