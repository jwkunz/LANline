# Architecture

## Overview

```
                                          ┌─────────────────────────────────────────────┐
   USB / IP                               │ lanline-server (Rust, tokio)                  │
 ┌──────────┐   IQ samples   ┌───────────┐│  ┌───────────┐   audio f32 48k   ┌─────────┐ │
 │  SDR     │───────────────▶│  Soapy    ││  │  DSP      │──────────────────▶│  Opus   │ │
 │ (HackRF/ │  Complex<i16>/ │  RX source│├─▶│  chain    │                   │ encoder │ │
 │  Pluto/  │  Complex<f32>  │           ││  │           │                   └────┬────┘ │
 │  RTL)    │                └───────────┘│  └───────────┘                        │      │
 └──────────┘                             │  debug_tone ─────────────────────────▶│      │
                                          │                            broadcast::channel│
                                          │                                       │      │
                                          │   ┌───────────────────────────────────┴────┐ │
                                          │   │ per-session WebRTC PeerConnection       │ │
                                          │   │  TrackLocalStaticSample (Opus)          │ │
                                          │   └───────────────┬────────────────────────┘ │
                                          │   axum REST (C2)  │  UDP beacon              │
                                          └───────┬───────────┼──────────┬──────────────┘
                                                  │ HTTP      │ SRTP/UDP │ UDP broadcast
                                                  ▼           ▼          ▼
                                          ┌───────────────────────────────────────────┐
                                          │ web client (Vite/TS)   ·  Android (later)  │
                                          └───────────────────────────────────────────┘
```

Three advertised ports:

| Port | Transport | Role |
|------|-----------|------|
| `c2` | TCP / HTTP | REST API + WebRTC signaling + the embedded web client at `/` |
| `audio_out` | UDP | WebRTC media (ICE host candidate) for the receive stream |
| `audio_in` | UDP | Reserved — inbound Opus to modulate (phase 2) |
| `beast` | TCP | Beast binary Mode S feed (ADS-B); `0` when disabled |
| `ais_nmea` | TCP | AIVDM (NMEA 0183) marine AIS feed; `0` when disabled |

`c2` is **fixed** (default `8730`) so a bookmarked browser URL survives a
restart; `audio_out`/`audio_in` are random, chosen at startup. Pass `--c2-port 0`
for a random C2 port too.

Reaching the server:

- **Browser** — open `http://<host>:<c2>/` (served by the server, same origin,
  nothing to type) or `http://<mdns-name>.local:<c2>/` via the mDNS responder
  (`mdns.rs`, default name `lanline`, also publishes `_lanline._tcp`).
- **Android / CLI** — the UDP discovery beacon on a separate **fixed** port, see
  [beacon-protocol.md](beacon-protocol.md).

## Server module map

```
server/src/
  main.rs              wiring: parse config, probe device, start beacon + mDNS + HTTP
  config.rs            CLI (clap) + resolved runtime settings
  mdns.rs              multicast-DNS responder: <name>.local + _lanline._tcp on the C2 port
  model.rs             all REST DTOs (serde), shared with handlers
  state.rs             AppState: Arc<...> handles to RadioManager, SessionStore, beacon info
  error.rs             ApiError -> (StatusCode, Json<error model>)

  api/
    mod.rs             axum Router, /api/v1 nesting, CORS, tracing layer
    webui.rs           GET / + /*.json — the embedded single-file web client (staged by build.rs)
    meta.rs            GET /health, GET /api/v1/server
    devices.rs         GET /devices, GET/PUT /device, GET /device/health
    modes.rs           GET /modes
    presets.rs         GET /presets, POST /presets/{id}/apply
    radio.rs           GET/PATCH /radio, start/stop, GET /radio/status
    adsb.rs            GET /adsb/aircraft, GET /adsb/messages
    ais.rs             GET /ais/vessels, GET /ais/messages
    sessions.rs        session CRUD + heartbeat
    audio.rs           WebRTC signaling: offer/ice/state/close
    reserved.rs        phase-2 endpoints -> 501

  discovery.rs         UDP beacon: pick ports, enumerate broadcast addrs, emit JSON at 1 Hz

  adsb/
    mod.rs             AdsbShared: tracker + hex ring + Beast broadcast, on RadioManager
    demod.rs           2 Msps PPM demod: magnitude, preamble gate, bit slice -> CRC-valid frames
    message.rs         Mode S CRC-24 (+ 1-bit fix), DF17/18 ME decode (ident / position / velocity)
    cpr.rs             Compact Position Reporting: global (even/odd) + local decode
    tracker.rs         per-ICAO track table: CPR folding, trails, expiry, JSON snapshot
    beast.rs           Beast binary encoder + TCP fan-out server

  ais/
    mod.rs             AisShared: vessel tracker + sentence ring + AIVDM broadcast
    demod.rs           per-channel GMSK: NCO -> decimating FIR -> FM discriminator ->
                       Gardner timing -> NRZI -> HDLC de-stuff / FCS
    message.rs         ITU-R M.1371 field decode (types 1/2/3/4/5/18/19/21/24), 6-bit
                       ASCII, AIVDM armor codec, X.25 FCS, ship-type labels
    tracker.rs         per-MMSI vessel table: position + static merge, trails, expiry
    nmea.rs            AIVDM sentence builder + TCP fan-out server

  radio/
    mod.rs             RadioManager: owns config + pipeline task, hot-apply, telemetry;
                       run_sdr (nbfm/wbfm/am -> Opus, picks FmChain/AmChain via
                       the Chain enum), run_adsb and run_ais (IQ -> track table,
                       no audio)
    dsp.rs             FmChain (NBFM/WBFM) + AmChain (AM), sharing one
                       decimate/squelch/resample skeleton; Nco/FirDecimator/
                       Biquad/LinearResampler building blocks

  audio/
    opus.rs            audiopus encoder wrapper (20 ms, 48k, mono)
    fanout.rs          tokio::sync::broadcast of encoded Opus frames

  webrtc/
    peer.rs            build RTCPeerConnection, attach Opus TrackLocalStaticSample,
                       pump frames from a fanout subscription, expose state
```

## Data / task model

- **One SDR, one DSP pipeline.** `RadioManager` runs the pipeline on a
  dedicated blocking task (SoapySDR reads are blocking). FM modes produce 20 ms
  Opus frames onto a `broadcast::channel`; the `adsb` and `ais` modes instead
  feed their decoders and write a track table read over REST + a raw TCP feed
  (Beast / AIVDM) — no audio path. `run_sdr`, `run_adsb` and `run_ais` are the
  three pipeline bodies.
- **N sessions, N WebRTC peers, shared audio.** Each peer task holds a
  `broadcast::Receiver` and writes samples into its `TrackLocalStaticSample`.
  Slow/backpressured receivers drop frames (lag), they never stall the
  pipeline.
- **Config is the source of truth.** `PATCH /radio` validates against the live
  device capability snapshot, updates `RadioConfig`, and signals the pipeline
  to hot-apply or restart. `GET /radio` reflects config; `GET /radio/status`
  reflects the running pipeline.
- **Sessions** live in an in-memory `SessionStore` (`DashMap`/`Mutex<HashMap>`)
  with a background reaper task on a 5 s tick.

## Sample-rate handling

The device sample rate varies per radio and is chosen from
`device.rx.sample_rate_ranges_hz` (default: the lowest rate ≥ a mode-specific
minimum). The DSP chain:

1. integer FIR decimation by the largest factor that keeps the intermediate
   rate ≥ `channel_bw_hz` × 2 (typically to ~48–96 ksps region),
2. FM discriminator + de-emphasis + audio LPF at the intermediate rate,
3. a **rational resampler** (`L/M`) to hit exactly `audio.sample_rate_hz`
   (48 000) for the Opus encoder.

Examples: HackRF 2 000 000 → ÷40 → 50 000 → ×24/25 → 48 000; a NESDR at
1 024 000 → ÷16 → 64 000 → ×3/4 → 48 000.

## Phase status

- **1a** — `model`/`error`/`state`, the full `api/` router, `discovery.rs`,
  `registry` (SoapySDR enumeration + capability probe), in-memory `radio`
  config.
- **1b** — `audio/` (Opus frame type), `radio/` (`RadioManager`: synth source
  → Opus → broadcast fan-out, on a dedicated thread), `media/` (`WebrtcEngine`:
  UDP-muxed `audio_out`, one peer per session, send-only Opus track). `debug_tone`
  = 440 Hz A4; other modes emit silence until 1c. `radio/start|stop|status` and
  `sessions/{id}/audio/*` are live.
- **1c** — real SoapySDR RX + `radio/dsp.rs` (LO-offset NCO → windowed-sinc
  FIR decimation to ~50 kHz → polar FM discriminator → de-emphasis → audio
  low-pass → amplitude-normalized noise squelch → linear resample to 48 kHz),
  wired to `nbfm` mode with live retune (pipeline bounce). Device-range
  validation on `PATCH /radio`. `--dump-wav <path>` writes the pre-Opus audio
  for offline inspection.
- **2c** — `adsb` mode: `adsb/` (2 Msps PPM demod → Mode S CRC/DF17-18 →
  CPR → per-ICAO track table), `GET /api/v1/adsb/{aircraft,messages}`, a Beast
  TCP feed (`--beast-port`, default 30005), and the web client's radar scope
  (canvas, range rings, trails — no tiles).
- **2d** — `ais` mode: `ais/` (two-channel 9600-baud GMSK → HDLC → ITU-R
  M.1371 → per-MMSI vessel table), `GET /api/v1/ais/{vessels,messages}`, an
  AIVDM TCP feed (`--ais-nmea-port`, default 10110), sharing the radar scope.
- **2e** — `am` mode: `dsp::AmChain`, a sibling of `FmChain` sharing its
  decimate/squelch/resample machinery with a plain AGC'd envelope detector in
  place of the discriminator (`run_sdr` picks the chain via a small `Chain`
  enum). Station-picker wizard (`web/src/am.ts`, `scripts/fetch-am.mjs`) mirrors
  `wbfm`'s. See [below](#am-and-hackrf-mflf-sensitivity) for what to expect
  from a HackRF at mediumwave.

### AM and HackRF MF/LF sensitivity

The HackRF's front end has no dedicated preselection filtering or LNA below
~30 MHz, so mediumwave (0.53–1.7 MHz) sensitivity is notably worse than a
purpose-built AM/shortwave receiver — but it isn't unusable. Bench-verified: a
5 kW station ~6 mi away decoded cleanly at max gain (`AMP` 14 dB, `LNA` 40 dB,
`VGA` 62 dB) — real speech, correct band-limiting, >160 dB of quiet/loud
dynamic range in a `--dump-wav` capture, and station-to-station SNR that
tracked actual transmitter power/distance rather than sitting at a constant
floor (which would point to a self-generated spur instead of a real signal).
Expect it to favor nearby, higher-power stations; a random-wire antenna
tuned for AM will do much better than the stock 1090 MHz-ish whip.
