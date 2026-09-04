# LANline

**LAN-native SDR command & control.** Point a browser (or the Android app) at a
radio on your network and listen.

A Rust **server** owns a SoapySDR radio (an attached HackRF One today;
ADALM-Pluto / RTL-SDR "NESDR" later) and exposes:

- a **UDP discovery beacon** on the LAN advertising three service ports,
- a **REST API** (the command & control port) to configure the radio,
- a **WebRTC Opus audio stream** of the demodulated RF (receive path),
- a reserved port for the future transmit path (modulated Opus in).

Clients: a **web app** (`client/web`) and, later, an **Android** wrapper
(`client/android`).

## Phase roadmap

| Phase | Scope | Status |
|------:|-------|:------:|
| 1a | Workspace, REST API + docs, discovery beacon, SoapySDR enumeration, web app shell | ✅ |
| 1b | Opus encoder + WebRTC; **Debug Mode** 440 Hz (A4) tone end-to-end in the browser | ✅ |
| 1c | Real NBFM receive of NOAA Weather Radio (162.4xx MHz); live retune + device-range validation over REST | ✅ |
| 1d | Live retune/gain without an audio gap, `noise_squelch` param, SNR metric, graceful shutdown, API contract tests | ✅ |
| 2a | Android WebView wrapper + native UDP beacon discovery (`window.LanlineNative`) | ✅ |
| 2b | Transmit / modulate path (the reserved `audio_in` port + `radio/tx*` endpoints) | |

The first receive mode is **NBFM** for the NOAA Weather Radio (NWR)
service. More modes are added through the REST API over time.

## Layout

```
Cargo.toml            workspace root (only member: server/)
server/               Rust LANline server (SDR + streaming)
client/web/           Vite + TypeScript web client
client/android/       Android WebView wrapper + native beacon listener
docs/                 rest-api.md, architecture.md, beacon-protocol.md
scripts/dev-env.sh    points the build/runtime at radioconda's SoapySDR
```

## Building the server

The server links the SoapySDR that ships with
[radioconda](https://github.com/ryanvolz/radioconda) (`~/radioconda`), which
already carries the HackRF, PlutoSDR and RTL-SDR modules.

```sh
source scripts/dev-env.sh      # sets PKG_CONFIG_PATH, RUSTFLAGS rpath,
                               # SOAPY_SDR_PLUGIN_PATH, LIBCLANG_PATH
cargo run -p lanline-server                    # discovers the HackRF, serves the API
cargo run -p lanline-server -- --debug-tone    # 440 Hz A4, no SDR needed
cargo run -p lanline-server -- --dump-wav /tmp/rx.wav   # also save pre-Opus audio
```

Sanity-check the radio independently with `SoapySDRUtil --find` from inside the
same shell.

## Running the web client

```sh
cd client/web
npm install
npm run dev
```

Then open the printed URL and enter the server host (shown in the server log,
or from the beacon).

## Building the Android client

```sh
cd client/web && npm run build          # bundle that the APK embeds
cd ../android
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
