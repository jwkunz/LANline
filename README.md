# LANline

**LAN-native SDR command & control.** Point a browser (or the Android app) at a
radio on your network and listen.

A Rust **server** owns a SoapySDR radio (an attached HackRF One today;
ADALM-Pluto / RTL-SDR "NESDR" later) and exposes:

- a **REST API** (the command & control port) to configure the radio,
- the **web client itself**, served from that same port — open
  `http://<host>:8730/` (or `http://lanline.local:8730/`) and it connects with
  nothing to type,
- a **WebRTC Opus audio stream** of the demodulated RF (receive path) —
  NOAA Weather Radio, FM broadcast, and **AM** (mediumwave/shortwave),
- **traffic trackers** — **ADS-B** aircraft (1090 MHz) and **AIS** vessels
  (162 MHz): decoded tracks over REST + a raw TCP feed (Beast / AIVDM), plotted
  on a self-contained radar scope in the web client,
- a reserved port for the future transmit path (modulated Opus in),
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
| 2g | Transmit / modulate path (the reserved `audio_in` port + `radio/tx*` endpoints) | |

The first receive mode is **NBFM** for the NOAA Weather Radio (NWR) service;
**wideband FM**, **AM**, an **ADS-B** aircraft tracker and an **AIS** vessel
tracker followed. More modes are added through the REST API over time.

## Layout

```
Cargo.toml            workspace root (only member: server/)
server/               Rust LANline server (SDR + streaming; embeds & serves web/)
web/                  Vite + TypeScript web client (built bundle is embedded in the server + the APK)
client/android/       Android WebView wrapper + native beacon listener
docs/                 rest-api.md, architecture.md, beacon-protocol.md
scripts/dev-env.sh    points the build/runtime at radioconda's SoapySDR
```

## Building the server

The server links the SoapySDR that ships with
[radioconda](https://github.com/ryanvolz/radioconda) (`~/radioconda`), which
already carries the HackRF, PlutoSDR and RTL-SDR modules.

```sh
npm --prefix web install && npm --prefix web run build   # bundle the server embeds & serves

source scripts/dev-env.sh      # sets PKG_CONFIG_PATH, RUSTFLAGS rpath,
                               # SOAPY_SDR_PLUGIN_PATH, LIBCLANG_PATH
cargo run --release -p lanline-server                  # discovers the HackRF, serves API + web client
cargo run --release -p lanline-server -- --debug-tone  # 440 Hz A4, no SDR needed
cargo run --release -p lanline-server -- --dump-wav /tmp/rx.wav   # also save pre-Opus audio
```

The server builds without the web bundle (it serves a placeholder page and logs
a warning); rebuild after `npm run build` to embed the real client.

Use `--release` for real listening — the debug build's DSP is ~5× slower and
wideband FM at 4 Msps can starve the WebRTC threads (audio drops).

Sanity-check the radio independently with `SoapySDRUtil --find` from inside the
same shell.

## Using the web client

The server serves it. With `lanline-server` running, open **`http://lanline.local:8730/`**
(or `http://<server-ip>:8730/`) in any browser on the LAN — it connects to that
origin automatically, no host to type. The Android app finds the server over the
LAN on its own.

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

### Traffic trackers (ADS-B / AIS)

Pick **ADS-B** (aircraft, 1090 MHz) or **AIS** (vessels, 162 MHz) in the mode
strip. Set your location (for range/bearing and a centred scope) and start the
receiver — decoded contacts appear on the shared radar scope (range rings in NM,
heading vectors, position trails; click a target for detail) and in the list
below it.

The server also exports the tracks over REST and as a raw TCP feed:

| Mode | REST | TCP feed |
|------|------|----------|
| ADS-B | `GET /api/v1/adsb/{aircraft,messages}` | **Beast** binary on `30005` (`--beast-port 0` to disable) → `readsb` / `tar1090` / VRS |
| AIS | `GET /api/v1/ais/{vessels,messages}` | **AIVDM** NMEA on `10110` (`--ais-nmea-port 0` to disable) → OpenCPN / AIS-catcher |

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

## License

MIT — see [LICENSE](LICENSE).
