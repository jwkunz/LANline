# Architecture

## Overview

```
                                          ┌─────────────────────────────────────────────┐
   USB / IP                               │ sdr-c2-server (Rust, tokio)                  │
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

Three advertised ports (all random, chosen at startup):

| Port | Transport | Role |
|------|-----------|------|
| `c2` | TCP / HTTP | REST API + WebRTC signaling |
| `audio_out` | UDP | WebRTC media (ICE host candidate) for the receive stream |
| `audio_in` | UDP | Reserved — inbound Opus to modulate (phase 2) |

Discovery beacon: separate **fixed** UDP port, see
[beacon-protocol.md](beacon-protocol.md).

## Server module map

```
server/src/
  main.rs              wiring: parse config, probe device, start beacon + HTTP
  config.rs            CLI (clap) + resolved runtime settings
  model.rs             all REST DTOs (serde), shared with handlers
  state.rs             AppState: Arc<...> handles to RadioManager, SessionStore, beacon info
  error.rs             ApiError -> (StatusCode, Json<error model>)

  api/
    mod.rs             axum Router, /api/v1 nesting, CORS, tracing layer
    meta.rs            GET /health, GET /api/v1/server
    devices.rs         GET /devices, GET/PUT /device, GET /device/health
    modes.rs           GET /modes
    presets.rs         GET /presets, POST /presets/{id}/apply
    radio.rs           GET/PATCH /radio, start/stop, GET /radio/status
    sessions.rs        session CRUD + heartbeat
    audio.rs           WebRTC signaling: offer/ice/state/close
    reserved.rs        phase-2 endpoints -> 501

  discovery.rs         UDP beacon: pick ports, enumerate broadcast addrs, emit JSON at 1 Hz

  radio/
    mod.rs             RadioManager: owns config + pipeline task, hot-apply, telemetry
    soapy.rs           enumerate(), open(), capability probe, RX stream reader
    source.rs          Source trait: SoapySource | DebugToneSource -> IQ or audio frames
    dsp/
      mod.rs           assemble the chain from RadioConfig
      decimate.rs      integer FIR decimation stages
      resample.rs      rational resampler to land exactly on 48 kHz
      fm_demod.rs      polar discriminator (I·dQ − Q·dI)/(I²+Q²)
      deemphasis.rs    one-pole 75/50 µs de-emphasis IIR
      squelch.rs       power / noise squelch, mutes audio
    tone.rs            440 Hz (A4) generator for debug_tone

  audio/
    opus.rs            audiopus encoder wrapper (20 ms, 48k, mono)
    fanout.rs          tokio::sync::broadcast of encoded Opus frames

  webrtc/
    peer.rs            build RTCPeerConnection, attach Opus TrackLocalStaticSample,
                       pump frames from a fanout subscription, expose state
```

## Data / task model

- **One SDR, one DSP pipeline.** `RadioManager` runs the pipeline on a
  dedicated blocking task (SoapySDR reads are blocking). It produces 20 ms
  Opus frames onto a `broadcast::channel`.
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

Examples: HackRF 2 400 000 → ÷50 → 48 000 (resampler is 1/1); a NESDR at
1 024 000 → ÷16 → 64 000 → ×3/4 → 48 000.

## Phase status

Phase 1a implements: `model.rs`, `error.rs`, `state.rs`, the full `api/`
router with real `devices`/`device`/`modes`/`presets`/`sessions` and in-memory
`radio` config + stub `radio/status`; `discovery.rs`; `radio/soapy.rs`
enumeration + capability probe. The pipeline, `dsp/`, `audio/`, `webrtc/` and
the `debug_tone` source arrive in 1b–1c.
