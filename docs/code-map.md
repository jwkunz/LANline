# Code map

A one-page orientation. For the detailed pipeline / DSP write-up see
[`architecture.md`](architecture.md); for the wire protocols see
[`rest-api.md`](rest-api.md) and [`beacon-protocol.md`](beacon-protocol.md).

## Repository layout

```
Cargo.toml              workspace root — members: server/, hypervisor/
LICENSE                 MIT
README.md               install / run / GUI guide + phase roadmap
CHANGELOG.md            per-phase history

server/                 the LANline server — one SDR, one DSP pipeline
  src/
    main.rs             binary entry point: parse config, build AppState, spawn tasks, serve
    lib.rs              re-exports every module so hypervisor/ can reuse them
    config.rs           clap Config — every CLI flag / LANLINE_* env var
    state.rs            AppState: shared handles given to every axum handler
    model.rs            serde types shared by the API and the pipeline
    catalog.rs          the mode catalog + base capability list
    api/                axum router + one file per endpoint group
      mod.rs            router assembly, CORS, the test app builders
      radio.rs          GET/PATCH /radio, start/stop, status, record, scan, tx/*, transcript
      adsb.rs ais.rs aprs.rs apt.rs analysis.rs   per-mode read endpoints (+ adsb/flight, ais/vessel)
      sessions.rs audio.rs devices.rs modes.rs presets.rs meta.rs reserved.rs
      webui.rs          serves the embedded web bundle at /
    radio/
      mod.rs            RadioManager + run_pipeline / run_sdr / run_adsb / run_ais /
                        run_aprs / run_apt / run_analysis — the pipeline threads
      dsp.rs            FM/AM demod, resamplers, CTCSS/DCS, squelch, TX modulator
    adsb/  ais/  aprs/  apt/   per-mode decoders (demod → frames → tracker → snapshot; aprs/tx.rs)
    voice/
      mod.rs            VoiceShared: transcript ring, PTT SegAccum, continuous AudioRing, stitch
      stt.rs            Whisper worker (candle); tts.rs — espeak-ng / Piper subprocess
    analysis/           FFT panadapter + waterfall + IQ .wav recorder (iq_wav.rs)
    media/              WebRTC engine (Opus, UDPMux)
    radio/audio_rec.rs  server-side demod-audio .wav recorder  |  audio/wav.rs — WAV writer
    registry.rs registry/soapy.rs   SoapySDR device enumeration + selection
    net.rs  mdns.rs  discovery.rs   LAN address detection, mDNS responder, UDP beacon
    tls.rs              self-signed cert generation + rustls config
    flight/  vessel/    outbound adsbdb.com / vesselfinder.com proxy + TTL cache
    error.rs  util.rs   ApiError type; small shared helpers
  build.rs              embeds web/dist/index.html via include_str!

hypervisor/             lanline-hypervisor: run one server per SDR
  src/                  main.rs, supervisor.rs (child processes + backoff), alloc.rs (port blocks),
                        devices.rs (serial= resolution), discovery.rs (fleet beacon + /api/v1/fleet), config.rs

web/                    Vite + TypeScript client (built bundle embedded in the server + APK)
  src/
    main.ts             the whole UI: state, render(structKey) vs patchLive, event delegation, per-mode panels
    api.ts              typed REST client (class Api)
    types.ts            shared response types
    audio.ts            WebRTC receive + push-to-talk mic
    per-mode / support helpers: adsb.ts ais.ts aprs.ts apt.ts nwr.ts fm.ts am.ts ham.ts frs.ts
                                analysis.ts radio-options.ts geo.ts discovery.ts
    style.css
  public/               bundled station / repeater / TLE JSON

client/android/          thin WebView wrapper + Kotlin UDP beacon listener + fleet chooser
scripts/                 dev-env.sh, run-server.sh, run-fleet.sh, *.service / *.plist, fetch-*.mjs
docs/                    this file, architecture.md, rest-api.md, legal.md, beacon-protocol.md, hypervisor.md, windows-port.md
```

## Request lifecycle

```
browser / Android WebView
      │  HTTP(S) on :8730
      ▼
api::router  ──►  api/*.rs handler  ──►  AppState (RadioManager, VoiceShared, flight/vessel, sessions)
                                             │
                     PATCH /radio ───────────┤ apply_patch: live retune/gain, or bounce the pipeline
                     POST  /radio/start ─────┘
                                             ▼
                              rx-pipeline thread  (run_pipeline → run_sdr / run_adsb / …)
                                 │ SoapySDR read → DSP (dsp.rs) → demod audio
                                 ├─► Opus encode ──► media:: WebRTC ──► browser <audio>
                                 ├─► voice::stt_feed ──► Whisper worker ──► transcript ring ──► GET /radio/transcript
                                 ├─► adsb/ais/aprs trackers ──► GET /adsb/aircraft … ──► web radar scope
                                 └─► analysis:: FFT ──► GET /analysis/spectrum ──► web waterfall
```

Discovery runs alongside: `mdns.rs` answers `lanline.local` / `_lanline._tcp`
for browsers; `discovery.rs` broadcasts a UDP beacon for the Android/CLI
clients. `lanline-hypervisor` adds a `LANLINE-FLEET-BEACON` and a
`/api/v1/fleet` index over its children.

## Where a mode lives

| Mode | Pipeline fn (`radio/mod.rs`) | Decoder | Read API | Web panel |
|---|---|---|---|---|
| `nbfm` `wbfm` `am` `frs` `ham` | `run_sdr` | `radio/dsp.rs` | WebRTC audio + `/radio/status` | `main.ts` per-mode wizard + audio card |
| `adsb` | `run_adsb` | `adsb/` | `/adsb/*`, `/adsb/flight/{icao}` | radar scope (`scope`/`adsb.ts`) |
| `ais` | `run_ais` | `ais/` | `/ais/*`, `/ais/vessel/{mmsi}` | radar scope (`ais.ts`) |
| `aprs` | `run_aprs` | `aprs/` | `/aprs/*`, `POST /aprs/tx` | scope + messaging card (`aprs.ts`) |
| `apt` | `run_apt` | `apt/` | `/apt/{status,image}` | canvas (`main.ts`) |
| `analysis` | `run_analysis` | `analysis/` | `/analysis/spectrum` | waterfall (`analysis.ts`) |
| `debug_tone` | `run_pipeline` (synth) | — | WebRTC audio | tone card |

## Adding a new receive mode (rough recipe)

1. `catalog.rs` — register the mode id + its default frequency / sample rate /
   capability.
2. `radio/mod.rs` — a `SourceKind` variant + a `run_<mode>` (or extend
   `run_sdr` if it is just another demod) built from `PipelineParams::from_config`.
3. Decoder crate-module under `server/src/<mode>/` if it produces structured
   tracks; expose a `Snapshot` type in `model.rs`.
4. `api/<mode>.rs` + a route in `api/mod.rs` for the read endpoint(s).
5. `state.rs` `capabilities()` if it needs a runtime gate.
6. Web: a `<mode>.ts` helper, a `switchMode` branch, a wizard panel in
   `main.ts`, an `api.ts` method, `types.ts` types.
7. `docs/rest-api.md` + the roadmap table + `CHANGELOG.md`.
