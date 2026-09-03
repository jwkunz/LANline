# My_LANd — SDR command & control over the LAN

A Software Defined Radio command-and-control service. A Rust **server** owns a
SoapySDR radio (an attached HackRF One today; ADALM-Pluto / RTL-SDR "NESDR"
later) and exposes:

- a **UDP discovery beacon** on the LAN advertising three service ports,
- a **REST API** (the command & control port) to configure the radio,
- a **WebRTC Opus audio stream** of the demodulated RF (receive path),
- a reserved port for the future transmit path (modulated Opus in).

Clients: a **web app** (`client/web`) and, later, an **Android** wrapper
(`client/android`).

## Phase roadmap

| Phase | Scope |
|------:|-------|
| 1a | Workspace, REST API + docs, stubbed server, discovery beacon, web app shell (connection/mode/telemetry, no audio) |
| 1b | Opus encoder + WebRTC; **Debug Mode** 440 Hz (A4) tone playing end-to-end in the browser |
| 1c | Real NBFM receive; hear NOAA weather radio **KHB29 on 162.550 MHz**; retune over REST |
| 1d | Session reaper, error taxonomy, logging, tests, polish |
| 2  | Android WebView wrapper + native beacon listener; transmit / modulate path |

The first receive mode is **NBFM** for the NOAA Weather Radio (NWR)
service. More modes are added through the REST API over time.

## Layout

```
Cargo.toml            workspace root (only member: server/)
server/               Rust SDR C2 + streaming server
client/web/           Vite + TypeScript web client
client/android/       Android wrapper (phase 2)
docs/                 rest-api.md, architecture.md, beacon-protocol.md
scripts/dev-env.sh    points the build/runtime at radioconda's SoapySDR
```

## Building the server

The server links the SoapySDR that ships with
[radioconda](https://github.com/ryanvolz/radioconda) (`~/radioconda`), which
already carries the HackRF, PlutoSDR and RTL-SDR modules.

```sh
source scripts/dev-env.sh      # sets PKG_CONFIG_PATH, LD_LIBRARY_PATH,
                               # SOAPY_SDR_PLUGIN_PATH, LIBCLANG_PATH
cargo run -p sdr-c2-server
```

Sanity-check the radio independently with `SoapySDRUtil --find` from inside the
same shell. Non-root USB access to the HackRF may need a udev rule (added in
phase 1c).

## Running the web client

```sh
cd client/web
npm install
npm run dev
```

Then open the printed URL and enter the server host (shown in the server log,
or from the beacon).

## Docs

- [`docs/rest-api.md`](docs/rest-api.md) — full REST API
- [`docs/architecture.md`](docs/architecture.md) — pipeline & module map
- [`docs/beacon-protocol.md`](docs/beacon-protocol.md) — UDP discovery wire format
