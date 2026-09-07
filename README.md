# LANline

[![build](https://github.com/jwkunz/LANline/actions/workflows/build.yml/badge.svg)](https://github.com/jwkunz/LANline/actions/workflows/build.yml)

**LAN-native SDR command & control.** Point a browser (or the Android app) at a
radio on your network and listen.

A Rust **server** owns a SoapySDR radio (an attached HackRF One today;
ADALM-Pluto / RTL-SDR "NESDR" later) and exposes:

- a **REST API** (the command & control port) to configure the radio,
- the **web client itself**, served from that same port — open
  `http://<host>:8730/` (or `http://lanline.local:8730/`) and it connects with
  nothing to type,
- a **WebRTC Opus audio stream** of the demodulated RF (receive path) —
  NOAA Weather Radio, FM broadcast, **AM** (mediumwave/shortwave),
  **FRS** (462/467 MHz walkie-talkie channels), and **Amateur FM**
  (VHF/UHF NBFM, band-plan-organized channel picker for 6 m – 23 cm),
- **push-to-talk transmit** — live mic audio up the same WebRTC connection,
  FM-modulated; **FRS** (personal-use only — a HackRF isn't Part 95
  type-accepted) and **amateur** (simplex or through a repeater's
  input+offset with an encoded CTCSS uplink tone; Part 97, licensed control
  operator). Off by default (`--enable-tx`); see
  [docs/architecture.md](docs/architecture.md#frs-and-the-transmit-question),
- **traffic trackers** — **ADS-B** aircraft (1090 MHz), **AIS** vessels
  (162 MHz), and **APRS** stations (144.390 MHz, 1200-baud packet): decoded
  tracks over REST + a raw TCP feed (Beast / AIVDM / TNC2), plotted on a
  self-contained radar scope in the web client,
- **NOAA APT** (137 MHz weather satellite): a real-time image downlink decoded
  into a two-channel grayscale raster over REST and rendered to canvas in the
  web client,
- **Receiver Analysis**: a server-computed FFT panadapter + waterfall,
  rendered as an interactive (zoom / pan / click-to-tune) plot in the web
  client, with IQ `.wav` recording,
- **recording**: a ⏺ button in the audio card captures the demodulated audio
  to a 48 kHz mono `.wav` on the server (any audio mode), with a download
  link — separate from Analysis mode's IQ recorder,
- **discovery** so clients find it unaided: an **mDNS** responder
  (`lanline.local` + `_lanline._tcp`) for browsers, and a **UDP beacon** for the
  Android/CLI clients that can't use mDNS.

Clients: a **web app** (`web/`, also served by the server) and an **Android**
wrapper (`client/android`).

## Phase roadmap

| Phase | Scope | Status |
|------:|-------|:------:|
| 1a | Workspace, REST API + docs, discovery beacon, SoapySDR enumeration, web app shell | ✅ |
| 1b | Opus encoder + WebRTC; **Debug Mode** 440 Hz (A4) tone end-to-end in the browser | ✅ |
| 1c | Real NBFM receive of NOAA Weather Radio (162.4xx MHz); live retune + device-range validation over REST | ✅ |
| 1d | Live retune/gain without an audio gap, `noise_squelch` param, SNR metric, graceful shutdown, API contract tests | ✅ |
| 2a | Android WebView wrapper + native UDP beacon discovery (`window.LanlineNative`) | ✅ |
| 2b | Mode-selection wizard, wideband FM broadcast, nearest-station finders, seek | ✅ |
| 2c | Server serves the web client + fixed C2 port + mDNS (`lanline.local`) — zero-config browser | ✅ |
| 2d | ADS-B tracker mode (1090 MHz Mode S decode), radar scope, Beast feed | ✅ |
| 2e | AIS tracker mode (162 MHz GMSK/ITU-R M.1371 decode), AIVDM feed, shared scope | ✅ |
| 2f | AM receive mode (mediumwave/shortwave), station-picker wizard | ✅ |
| 2g | NOAA APT receive mode (137 MHz weather satellite image), canvas rendering | ✅ |
| 2h | FRS receive mode (462/467 MHz, 22-channel plan), channel-picker wizard | ✅ |
| 2i | Push-to-talk transmit (FRS only; FM modulator, half-duplex device switching, live mic-audio uplink over WebRTC, PTT protocol) — off by default (`--enable-tx`), personal-use only per FCC Part 95 | ✅ |
| 2j | Multi-radio device picker (HackRF / ADALM-Pluto), TLS with a self-signed cert, Receiver Analysis waterfall | ✅ |
| 2k | Amateur NBFM — band-plan-organized channel picker (6 m – 23 cm), simplex/calling, voluntary ARRL band-plan hints, CTCSS tone squelch + always-on tone identifier, regional repeater directory + manual entry | ✅ |
| 2l | Amateur transmit — hold-to-talk on simplex or through a repeater (input offset + encoded CTCSS uplink tone), `--enable-tx`-gated, Part 97, transmit audit log | ✅ |
| 2m | Server-side demod-audio recording — `POST /radio/record`, 48 kHz mono WAV, per-request start/stop + download, any audio mode | ✅ |
| 2n | Server-side channel scan — `POST /radio/scan`, sweeps a channel list / band range in the pipeline thread, parks on squelch-open with a hang time; Scan button in the web client for FRS / NWR / ham / broadcast | ✅ |
| 2o | Repeater directory coverage — hearham region matcher now reads spelled-out state names (83 → ~1460 FL), `HAM_REPEATER_CENTER` distance filter, paste-shorthand in the add form | ✅ |
| 2p | DCS (Digital Coded Squelch) — decode (134.4 bps Golay(23,12), transition-locked bit clock) + encode in the TX modulator, `dcs_code`/`dcs_invert`/`dcs_squelch` params, wizard picker | ✅ |
| 2q | APRS receive mode — 1200-baud AFSK/AX.25 decode (position / MIC-E / status / message), per-callsign station tracker on the shared radar scope, `GET /aprs/stations` + `/aprs/packets`, TNC2 TCP feed | ✅ |
| 2r | Concurrent radios — `lanline-hypervisor` runs one server per SDR (server split into lib + bin), collision-free port blocks + `serial=`-resolved devices + backoff supervision, aggregated discovery (`LANLINE-FLEET-BEACON`, `GET /api/v1/fleet`, per-child mDNS), native Android radio chooser | ✅ |
| 2s | APRS transmit — `POST /aprs/tx` (message / position / raw), half-duplex burst in `run_aprs` reusing the demod's modulator, `--enable-tx`-gated, local TNC echo + audit log; web `aprs` panel messaging card → two browser tabs = an APRS chat room between the fleet's two radios | ✅ |
| 2t | Voice ↔ text for the FM/AM modes (optional, feature `voice-text`) — `POST /radio/tx/say` speaks typed text on the air (espeak-ng / Piper), `GET /radio/transcript` transcribes received overs (pure-Rust Whisper via candle); web "Voice text" card in `frs`/`ham` | ✅ |
| 2u | Repeater directory is now nationwide (US, ~9k) and the web client sorts it by your location — same as the FM station list; the `FL`-only default is gone | ✅ |
| 2v | Flight lookup — tapping an ADS-B contact fetches tail number / type / owner / route from adsbdb.com via a server-side `GET /api/v1/adsb/flight/{icao}` proxy (TTL cache, cross-client de-dupe); the one internet-facing feature, on by default, `--flight-lookup false` to disable | ✅ |
| 2w | Vessel lookup — the same for AIS: tapping a ship fetches flag / tonnage / year built / dimensions / photo from vesselfinder.com via `GET /api/v1/ais/vessel/{mmsi}`; shares the `--flight-lookup` toggle | ✅ |

The first receive mode is **NBFM** for the NOAA Weather Radio (NWR) service;
**wideband FM**, **AM**, an **ADS-B** aircraft tracker, an **AIS** vessel
tracker, and **NOAA APT** image reception followed. More modes are added
through the REST API over time.

## Layout

```
Cargo.toml            workspace root (members: server/, hypervisor/)
server/               Rust LANline server (SDR + streaming; embeds & serves web/); also a lib
hypervisor/           lanline-hypervisor: run one server per radio (see docs/hypervisor.md)
web/                  Vite + TypeScript web client (built bundle is embedded in the server + the APK)
client/android/       Android WebView wrapper + native beacon listener + fleet radio chooser
docs/                 rest-api.md, architecture.md, beacon-protocol.md, hypervisor.md, windows-port.md
scripts/dev-env.sh    points the build/runtime at radioconda's SoapySDR
scripts/run-server.sh build (if needed) + run the server with that env already set up
scripts/run-fleet.sh  same, for lanline-hypervisor
```

## Download and run (no SDR)

The [Releases](https://github.com/jwkunz/LANline/releases) page has a
**standalone server** for each platform. The web client is compiled into the
binary, so it's genuinely one file — run it and open a browser.

| Download | Platform | Contains |
|----------|----------|----------|
| `lanline-server-linux-x86_64.tar.gz` | Linux x86-64, glibc ≥ 2.35 (Ubuntu 22.04 / Debian 12 / Fedora 36 or newer) | the binary |
| `lanline-server-macos-arm64.tar.gz` | macOS on Apple Silicon (M1+) | the binary |
| `lanline-server-windows-x86_64.zip` | Windows 10/11 x64 | `lanline-server.exe` + its VC runtime DLLs |

**Dependencies: none.** opus is statically linked, the Windows zip carries
`vcruntime140*.dll` / `msvcp140.dll`, and macOS/Linux need only libraries the
OS already ships. No radioconda, no SoapySDR, no runtime install.

**These builds do not talk to real SDR hardware** — they run the full server
(REST API, web UI, WebRTC audio, discovery) with the SDR layer stubbed. Use
`--debug-tone` for an end-to-end audio check. For a live HackRF / RTL-SDR, see
[Building the server](#building-the-server) below.

### Linux

```sh
tar xzf lanline-server-linux-x86_64.tar.gz
./lanline-server --debug-tone          # or plain ./lanline-server
```

### macOS

```sh
tar xzf lanline-server-macos-arm64.tar.gz
xattr -d com.apple.quarantine lanline-server   # it's unsigned; clears Gatekeeper
./lanline-server --debug-tone
```

### Windows

Extract the zip (keep the `.exe` and the DLLs together) and run
`lanline-server.exe` — a terminal, or double-click. SmartScreen will flag it
as unrecognised (unsigned): **More info → Run anyway**. On first launch
Windows Firewall will ask — **Allow** on private networks so LAN clients can
reach it.

### Then

The server prints the exact URLs it's listening on (`http://…`, or `https://…`
with `--tls`). Open one in a browser:

- **same machine:** `localhost:8730`
- **another device on the LAN:** `<server-ip>:8730` (shown at startup), or
  `lanline.local:8730` where mDNS resolves
- the **Android app** finds it over the LAN on its own

The mic and the location button need `--tls` (or `localhost`) — see below.

Stop it with Ctrl-C. Common flags (full list: `lanline-server --help`):

| Flag | |
|------|--|
| `--debug-tone` | serve a 440 Hz test tone — proves the audio path with no SDR |
| `--tls` | serve **HTTPS** (see below) — needed for the mic / location button off-box |
| `--c2-port 9000` | change the web/REST port from 8730 |
| `--no-mdns` / `--no-beacon` | disable a discovery mechanism |
| `--bind 127.0.0.1` | listen on loopback only (default is all interfaces) |

### HTTPS (`--tls`)

Browsers only expose **`getUserMedia`** (the push-to-talk mic) and the
**"📍 My location"** button on a *secure context*: `https://`, or
`http://localhost`. Reached from another device over a plain-`http://` LAN
address, those features silently do nothing. `--tls` fixes that.

```sh
lanline-server --tls                       # self-signed, auto-generated
lanline-server --tls --tls-cert cert.pem --tls-key key.pem   # your own cert
```

With no cert given, the server generates a self-signed one on first run —
SANs cover `localhost`, `lanline.local`, and every LAN IPv4 it sees — and
caches it (`~/.local/state/lanline/tls/` on Linux, `Library/Application
Support/lanline/tls/` on macOS, `%LOCALAPPDATA%\lanline\tls\` on Windows;
override with `--tls-dir`). It logs the cert's SHA-256 fingerprint at startup.

Because it's self-signed, each browser shows a one-time **"Your connection is
not private"** warning — click **Advanced → Proceed**. After that the origin
*is* a secure context and the mic / location work; the cached cert keeps that
exception valid across restarts. The Android app trusts a self-signed cert
from a LAN address automatically.

For **zero warnings**, issue a locally-trusted cert with
[`mkcert`](https://github.com/FiloSottile/mkcert) and pass it via
`--tls-cert` / `--tls-key`:

```sh
mkcert -install                                   # once per device
mkcert lanline.local localhost 192.168.1.234      # -> *.pem in the cwd
lanline-server --tls --tls-cert ./lanline.local+2.pem --tls-key ./lanline.local+2-key.pem
```

`--tls` makes the C2 port HTTPS-only; plain-HTTP clients can't connect. The
discovery beacon and mDNS advertise the `https://` URL automatically.

## Building the server

The server links the SoapySDR that ships with
[radioconda](https://github.com/ryanvolz/radioconda) (`~/radioconda`), which
already carries the HackRF, PlutoSDR and RTL-SDR modules.

```sh
npm --prefix web install && npm --prefix web run build   # bundle the server embeds & serves

scripts/run-server.sh                     # sources dev-env.sh, cargo build --release, run (HTTPS)
scripts/run-server.sh --debug-tone        # 440 Hz A4, no SDR needed
scripts/run-server.sh --no-tls            # plain HTTP instead of self-signed HTTPS
scripts/run-server.sh --no-build          # skip the rebuild, just launch
```

`run-server.sh` defaults to `--tls` (self-signed HTTPS, so the mic and
location button work off-box) and forwards every unrecognised flag to the
server; per-machine settings (device filter, ppm correction, `--enable-tx`,
TLS cert paths) go in a git-ignored `scripts/run-server.env` — copy
`scripts/run-server.env.sample` to start.

To do it by hand instead:

```sh
source scripts/dev-env.sh      # sets PKG_CONFIG_PATH, RUSTFLAGS rpath,
                               # SOAPY_SDR_PLUGIN_PATH, LIBCLANG_PATH
cargo run --release -p lanline-server
```

The server builds without the web bundle (it serves a placeholder page and logs
a warning); rebuild after `npm run build` to embed the real client.

Use `--release` for real listening — the debug build's DSP is ~5× slower and
wideband FM at 4 Msps can starve the WebRTC threads (audio drops).

Sanity-check the radio independently with `SoapySDRUtil --find` from inside the
same shell.

### Voice ↔ text (optional)

Type a message and have the server speak it on the air, and transcribe received
transmissions back to text — for the `nbfm` / `wbfm` / `am` / `frs` / `ham`
modes. Off by default; build it in with `--features voice-text`:

```sh
sudo apt-get install -y espeak-ng            # TTS engine (or: brew install espeak-ng)
cargo build --release --features voice-text -p lanline-server
# STT needs a local Whisper model in the HF layout, e.g.
huggingface-cli download openai/whisper-base.en --local-dir ~/models/whisper-base.en
./target/release/lanline-server --device driver=hackrf --enable-tx \
    --tts --stt --stt-model ~/models/whisper-base.en
```

`POST /api/v1/radio/tx/say {"text": "…"}` speaks it; `GET /api/v1/radio/transcript`
returns the recognized overs. The web client shows a **Voice text** card in the
`frs` / `ham` panels. `--tts-voice <model.onnx>` swaps espeak-ng's robotic
voice for a Piper neural one (needs the `piper` binary). See
[`docs/rest-api.md`](docs/rest-api.md) and the "Voice ↔ text" section of
[`docs/architecture.md`](docs/architecture.md).

### Running multiple radios

`lanline-server` is one process, one SDR. To run several at once, the
`lanline-hypervisor` binary launches one server per radio, keeps their ports
and state dirs from colliding, resolves each to a distinct physical SDR, and
aggregates LAN discovery.

```sh
cp scripts/fleet.toml.sample fleet.toml     # edit the [[radio]] blocks
scripts/run-fleet.sh --config fleet.toml    # sources dev-env.sh, builds the workspace, runs
```

Children land on `8730`, `8731`, … ; the fleet index is `http://<host>:8720/`
(`GET /api/v1/fleet` for JSON). Any LANline client can point at a child port
directly, and the **Android app** shows a native "Select radio" chooser when
it hears the fleet on the LAN, with a "Switch radio" action to change later.
Full reference: [`docs/hypervisor.md`](docs/hypervisor.md).

**Windows:** the server is Linux-first today. The Rust code is portable bar one
hostname helper, but the SoapySDR/HackRF stack and native build deps need
manual setup — see [`docs/windows-port.md`](docs/windows-port.md).

## Using the web client

The server serves it. With `lanline-server` running, open **`http://lanline.local:8730/`**
(or `http://<server-ip>:8730/`) in any browser on the LAN — it connects to that
origin automatically, no host to type. The Android app finds the server over the
LAN on its own.

**⚙ Radio options** (in the "Now playing" card) opens a panel over the raw
SoapySDR knobs for the selected device — PPM correction, per-element gain
(LNA / VGA / AMP on a HackRF, a single PGA on a Pluto), analog bandwidth,
sample rate, antenna, DC-offset mode, and free-form `writeSetting`
key/values. Ranges come from the device itself; frequency and gain apply
live, the rest briefly restart the receiver.

If more than one SDR is attached, a **Radio** picker appears in the *Server*
card — any SoapySDR-supported device works (HackRF and ADALM-Pluto verified;
RTL-SDR, LimeSDR, etc. should too). Switching stops the receiver and resets
the device-specific tuner fields; the mode strip greys out modes whose home
frequency is outside the new device's tuning range (e.g. AM on a Pluto).

To iterate on the web client itself:

```sh
cd web
npm install
npm run dev        # http://localhost:5173 — enter the server host manually
```

The dev server still talks to a running `lanline-server` over its REST API; a
typed/saved host is only needed in this mode (and inside the Android wrapper).

### AM Radio

Pick **AM Radio** in the mode strip — a station-picker wizard (nearest AM
broadcast stations from the FCC database, or a manual 520–1710 kHz dial with
seek) just like FM Broadcast. A quick heads-up: the HackRF's front end has no
preselection filtering below ~30 MHz, so mediumwave sensitivity trails a
dedicated AM/shortwave receiver — nearby, higher-power stations come in fine
at max gain; weak/distant ones may not. See
[architecture.md](docs/architecture.md#am-and-hackrf-mflf-sensitivity) for
what was actually verified on the bench.

### Traffic trackers (ADS-B / AIS / APRS)

Pick **ADS-B** (aircraft, 1090 MHz), **AIS** (vessels, 162 MHz) or **APRS**
(stations, 144.390 MHz) in the mode strip. Set your location (for range/bearing
and a centred scope) and start the receiver — decoded contacts appear on the
shared radar scope (range rings, heading vectors, position trails; click a
target for detail) and in the list below it. APRS decodes 1200-baud Bell 202
AFSK / AX.25 UI frames: uncompressed + compressed positions, **MIC-E** (the
common tracker format), status and message packets.

Tapping an aircraft in the ADS-B list also looks it up on **adsbdb.com** —
tail number, type, operator, and the origin → destination route — and shows
it inline; tapping an AIS vessel does the same against **vesselfinder.com**
(flag, tonnage, year built, dimensions, photo). This is the only part of
LANline that reaches the internet; it is on by default and
`--flight-lookup false` (or `LANLINE_FLIGHT_LOOKUP=0`) turns it off for
airgapped use.

The server also exports the tracks over REST and as a raw TCP feed:

| Mode | REST | TCP feed |
|------|------|----------|
| ADS-B | `GET /api/v1/adsb/{aircraft,messages}` | **Beast** binary on `30005` (`--beast-port 0` to disable) → `readsb` / `tar1090` / VRS |
| AIS | `GET /api/v1/ais/{vessels,messages}` | **AIVDM** NMEA on `10110` (`--ais-nmea-port 0` to disable) → OpenCPN / AIS-catcher |
| APRS | `GET /api/v1/aprs/{stations,packets}` | **TNC2** text on `10152` (`--aprs-port 0` to disable) → Xastir / YAAC / an APRS-IS gateway |

### NOAA APT (weather satellite)

Pick **NOAA APT** in the mode strip — quick-picks for NOAA-15/18/19's 137 MHz
downlinks (each showing its predicted next pass and max elevation for your
saved location), or a manual frequency, plus a canvas that fills in with the
decoded image as scan lines arrive (`GET /api/v1/apt/{status,image}`, the
latter a small binary raster, no image crate involved). Unlike every other
mode here, this one has no ambient signal to fall back on: it's a real-time
feed from one satellite, receivable only during an actual overhead pass (a
few minutes, several times a day). Outside of a pass, expect a low
`sync_quality` and a noisy/empty image; that's the decoder correctly
declining to claim a lock, not a bug.

Pass predictions are computed **locally in the browser** via SGP4
(`satellite.js`) against a bundled set of orbital elements
(`GET /apt-tle.json`) — no live tracking API call, works offline. That
snapshot goes stale faster than the station directories (accurate for
roughly 1–2 weeks); regenerate it with `node scripts/fetch-apt-tle.mjs`.

### FRS (walkie-talkie channels)

Pick **FRS** in the mode strip — a fixed picker for all 22 FRS channels
(462/467 MHz), each labeled with its FCC power limit and whether it's
FRS-exclusive (channels 8–14) or shared with GMRS (1–7, 15–22). There's no
station directory here — anyone could be transmitting on any channel from
anywhere — so **Seek** in "Now playing" is the closest thing to browsing: it
steps channel-by-channel looking for squelch-open traffic. **Scan** (next to
Seek, also on NWR / ham / broadcast) hands the same idea to the server: the
pipeline sweeps the whole channel list and *parks* on live traffic, with a
hang time before it resumes — a real scanner, no client round-trips.

A **push-to-talk** button appears below the channel picker when the server
is running with `--enable-tx` and a transmit-capable device is selected —
hold it to key up and talk (live mic audio, not a test tone), release to
stop; a hard 10s cap auto-releases regardless. This is off by default and
deliberately so: FRS is a Part 95 certified-equipment service, and a
general-purpose SDR like a HackRF isn't type-accepted for FRS transmission,
independent of power level or intent. `--enable-tx` exists for personal,
non-distributed use at the operator's own informed discretion, not as a
feature this project is presenting as generally compliant — see
[architecture.md](docs/architecture.md#frs-and-the-transmit-question) for
the full design (half-duplex hand-off, the WebRTC mic uplink, and a couple
of real bugs found and fixed getting it working end-to-end against an
actual handheld).

If FRS reception sounds distorted or silent despite squelch opening, your
SDR's crystal is likely just off-frequency enough at UHF to matter — set
`tuner.freq_correction_ppm` (or `--freq-correction-ppm` at startup); see
[architecture.md](docs/architecture.md#hackrf-frequency-calibration) for how
to measure it.

### Amateur FM (VHF/UHF NBFM)

Pick **Amateur FM** in the mode strip for narrowband FM on the amateur
VHF/UHF bands. The picker is organized by **band** — 6 m, 2 m, 1.25 m,
70 cm, 33 cm, 23 cm — and each band shows:

- its **simplex & calling** frequencies (2 m 146.52, 70 cm 446.00, …) as
  quick-tune rows,
- a **manual dial** bounded to the band edges (5 kHz steps),
- the **voluntary ARRL band plan** as a reference list, with a "you are
  here" marker that tracks the dial and a caution when you land in a
  CW/SSB/weak-signal segment where FM isn't used.

Every band here is open to **all U.S. license classes** (Technician and up)
for FM voice; the panel notes the conventional repeater offset for the band.

A **sub-audible squelch** picker sits under the manual dial: choose the
**CTCSS** tone ("PL") *or* the **DCS** code a repeater requires and the
channel only unmutes while that signalling is present (a "monitor" checkbox
detects and reports without muting; a "DCS inv" checkbox for the inverted
codes). CTCSS is a single-tone presence check; DCS decodes the 134.4 bps
Golay-coded stream. The panel also shows the **CTCSS tone actually on the
air** (an always-on 50-tone Goertzel scanner) with a one-tap "use it" — so
an unknown repeater's tone identifies itself.

A **Simplex / Repeaters** toggle switches the channel list to a **nationwide**
(US) repeater directory (~9k FM repeaters from
[hearham.com](https://hearham.com)) — the client sorts it by your location
just like the FM station list, showing the nearest ~60 for the band. Each row
shows the output frequency, offset (−0.6 / +5 MHz …), uplink/output tones,
town and distance. Tapping one tunes the repeater's output and, if it
transmits a tone, sets that as your receive tone squelch; the offset and
uplink tone are remembered for transmit.

`node scripts/fetch-repeaters.mjs` rebuilds the bundle
(`HAM_REPEATER_CENTER="lat,lon"` or `HAM_REPEATER_REGIONS="SC,NC,GA"` make a
smaller one). **+ Add repeater** stores your own in the browser — its paste
box takes shorthand like `146.94 - 100.0` or `442.100 +5 PL 131.8 W4ABC`.

**Transmit** (`--enable-tx`, off by default): the same hold-to-talk button
FRS uses appears in *Now playing*. On simplex it keys the tuned frequency;
with a repeater selected it keys the **input** (output ± offset) and encodes
the **uplink signalling** — the repeater's CTCSS tone, or a DCS code you set
in the wizard. Every key is written to a **transmit audit log**
(`GET /api/v1/radio/tx/log`, and a collapsible list under the PTT button) —
time, frequency, offset, tone/code, duration. Part 97 permits homebrew gear
under an amateur licence — you are the control operator (KZ4AZ). The same UHF
`freq_correction_ppm` note as FRS applies.

### Receiver Analysis (waterfall)

Pick **Analysis** in the mode strip for an interactive FFT panadapter +
waterfall over the current `tuner.sample_rate_hz` span. Wheel to zoom the
frequency axis, drag to pan, click to re-tune the centre; adjustable dynamic
range, colour map, FFT size, window and frame rate. No audio in this mode —
switch to nbfm/wbfm/am to listen.

**⏺ Record IQ** writes a 2-channel int16 `.wav` (interleaved I/Q at the
device rate — the format GQRX / SDR# / SDRuno read) to the server's
`--iq-dir` (default: its working directory), then offers a download link.
~8 MB/s at 2 Msps; 60-second cap by default.

## Building the Android client

```sh
cd web && npm run build                 # bundle that the APK embeds
cd ../client/android
cp local.properties.sample local.properties   # set sdk.dir
./gradlew assembleDebug                        # app/build/outputs/apk/debug/
```

or open `client/android/` in Android Studio and Run. See
[`client/android/README.md`](client/android/README.md).

## Docs

- [`docs/rest-api.md`](docs/rest-api.md) — full REST API
- [`docs/architecture.md`](docs/architecture.md) — pipeline & module map
- [`docs/beacon-protocol.md`](docs/beacon-protocol.md) — UDP discovery wire format
- [`docs/windows-port.md`](docs/windows-port.md) — what a Windows build of the server would take (Linux-first today)

## License

MIT — see [LICENSE](LICENSE).
