# LANline

**LAN-native SDR command & control.** Point a browser (or the Android app) at a
radio on your network and listen.

A Rust **server** owns a SoapySDR radio (an attached HackRF One today;
ADALM-Pluto / RTL-SDR "NESDR" later) and exposes:

- a **REST API** (the command & control port) to configure the radio,
- the **web client itself**, served from that same port — open
  `http://<host>:8730/` (or `http://lanline.local:8730/`) and it connects with
  nothing to type,
- a **WebRTC Opus audio stream** of the demodulated RF (receive path),
- an **ADS-B tracker** (1090 MHz): decoded aircraft over REST + a Beast TCP feed,
  plotted on a self-contained radar scope in the web client,
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
| 2e | Transmit / modulate path (the reserved `audio_in` port + `radio/tx*` endpoints) | |

The first receive mode is **NBFM** for the NOAA Weather Radio (NWR)
service; **wideband FM** and an **ADS-B** aircraft tracker followed. More modes
are added through the REST API over time.

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

### ADS-B

Pick **ADS-B** in the mode strip. Set your location (for range/bearing and a
centred scope) and start the receiver — decoded aircraft appear on the radar
scope (range rings in NM, heading vectors, position trails; click a target for
detail) and in the list below it. The server also exposes the tracks at
`GET /api/v1/adsb/aircraft` and streams a **Beast** feed on TCP `30005`
(`--beast-port 0` to disable) for `readsb` / `tar1090` / Virtual Radar Server.

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
