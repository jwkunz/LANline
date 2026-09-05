# Running the server on Windows

**Status: audited, not tested.** Nobody has compiled or run `lanline-server`
on Windows yet. This is a source- and dependency-level review of what would
need to happen, done ahead of a v1.0.0 tag. Nothing in the Rust code hard-
blocks Windows; the friction is entirely in the native toolchain and the SDR
driver stack, and every build-time piece is something Linux already needs —
it's just harder to obtain on Windows.

## The one code change

`server/src/net.rs::hostname()` reads `/proc/sys/kernel/hostname`. On Windows
that read fails and the function returns `"unknown"` — no panic, no crash.
The hostname only appears in `GET /api/v1/server` and the UDP beacon payload,
so this is cosmetic, but it should be fixed for a real port:

- add the `hostname` crate (cross-platform, tiny), or
- fall back to `std::env::var("COMPUTERNAME")` / `gethostname` when the
  `/proc` read fails.

Nothing else in `server/src` is platform-specific: no `std::os::unix`, no
`libc`, no Unix sockets, no signal handling beyond `tokio::signal::ctrl_c()`
(which handles Ctrl+C and console-close on Windows), no hardcoded `/tmp` or
POSIX file modes.

## Build-time native dependencies

The MSVC toolchain (`x86_64-pc-windows-msvc`) is the target to aim for.
Install "Desktop development with C++" from the Visual Studio Build Tools —
that covers the C compiler `audiopus_sys` needs to build its vendored
libopus. Then, three more pieces:

| Need | Why | Get it |
|------|-----|--------|
| **libclang** (`LIBCLANG_PATH`) | `soapysdr-sys` runs bindgen against SoapySDR's headers | Install LLVM for Windows; point `LIBCLANG_PATH` at its `bin` |
| **NASM** on `PATH` | `ring` 0.17 (WebRTC's crypto — rustls + ring, no OpenSSL) assembles its x86-64 routines with NASM on the MSVC target | `winget install nasm` / choco / the NASM installer |
| **pkgconf** + SoapySDR `.pc` files | `soapysdr-sys` locates `SoapySDR.dll`/`.lib` and headers via pkg-config | Install `pkgconf`; set `PKG_CONFIG_PATH` to PothosSDR's `lib\pkgconfig` |

(`x86_64-pc-windows-gnu` instead of MSVC avoids the NASM requirement and the
Build Tools install, at the cost of a fussier SoapySDR link. MSVC is the
better-trodden path.)

`scripts/dev-env.sh` does not translate: its core trick is baking an ELF
`-Wl,-rpath` into the binary so it finds radioconda's `.so`s. Windows has no
rpath — DLLs resolve from `PATH` and the executable's directory. The Windows
equivalent is just "PothosSDR's `bin` is on `PATH`, and the three build
variables above are set." A short `scripts/dev-env.ps1` doing that is the
natural companion to a Windows port; it doesn't exist yet.

## SDR stack at runtime

- **PothosSDR** is the Windows analogue of radioconda: one installer that
  bundles `SoapySDR.dll`, the device modules (HackRF, RTL-SDR, Airspy,
  LimeSDR, PlutoSDR, ...), and `SoapySDRUtil.exe`. Its installer puts its
  `bin` on `PATH`. Set `SOAPY_SDR_PLUGIN_PATH` to its
  `lib\SoapySDR\modules0.8` if module autodiscovery misses.
- **HackRF USB driver.** Windows will not bind a generic libusb driver on
  its own. Run **Zadig** once and install the **WinUSB** driver for the
  HackRF interface. Until you do, `SoapySDRUtil --find` shows nothing and the
  server reports no devices. This is the step most likely to trip up a new
  Windows user.

## What works unchanged

- The REST API, the web client it serves, session handling, the DSP
  pipeline, Opus encode/decode, `--dump-wav`, and all `LANLINE_*` / CLI
  configuration — pure Rust, no platform assumptions.
- **WebRTC** (`webrtc` 0.13): rustls + ring, no OpenSSL, so no Perl/OpenSSL
  build tangle. Builds fine on Windows once NASM is present.
- **mDNS** (`mdns-sd`): pure Rust, works on Windows 10/11 and coexists with
  the OS's own mDNS responder.
- **`if-addrs`**: has a Windows backend (IP Helper API), so interface
  enumeration and the per-interface broadcast targets work.
- **Discovery beacon**: `set_broadcast` + directed/global broadcast sends
  work. Expect a **Windows Firewall prompt** on first run to allow the app
  to send/receive UDP — that's normal, allow it on Private networks.

## CI coverage

`.github/workflows/build.yml` builds the server on `ubuntu-latest`,
`macos-latest`, and `windows-latest` in two tiers:

- **`standalone`** — `--no-default-features`, no SoapySDR. One self-contained
  file per OS (the Windows zip bundles the redistributable VC runtime DLLs so
  it runs on a bare install). Required to pass on all three OSes; on a `v*`
  tag these are attached to a draft GitHub Release.
- **`soapy`** — default features, linking libSoapySDR. Linux/macOS install it
  from apt/brew and must pass. **Windows** pulls PothosSDR + NASM + pkgconf
  and runs with `continue-on-error: true` — the job goes green with a warning
  annotation whether or not the SDR link actually worked, so read its log to
  see where a Windows soapy build currently stands. A single-file *soapy*
  binary isn't possible anyway: SoapySDR loads its driver modules as separate
  shared libraries at runtime.

So "does the Windows soapy build work" now has a live answer in every CI run,
not just this document.

## Recommendation for v1.0.0

The code doesn't force the choice. Either:

1. **Linux-first release.** Tag v1.0.0, note "Windows: `--no-default-features`
   builds in CI on `windows-latest`; the SoapySDR link is unverified — see
   docs/windows-port.md." Least work, honest.
2. **Claim Windows support.** Land the `hostname()` fix, add
   `scripts/dev-env.ps1` + a `run-server.ps1`, get the `soapy` Windows CI leg
   green (drop its `continue-on-error`), and smoke-test the binary on a real
   Windows box with a HackRF. Then the claim is real.

Given the Windows soapy path is still unproven, option 1 is the accurate
story for a v1.0.0 cut today.
