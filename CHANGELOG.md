# Changelog

All notable changes, newest first. LANline is versioned `MAJOR.MINOR.PATCH`;
the phase letters (`2a`, `2b`, …) in the [README roadmap](README.md#phase-roadmap)
are the development log this summarizes.

## v2.5.0 — first public release (2026-09)

Everything below, packaged for public use: MIT license, a consolidated
[Legal / spectrum-law section](docs/legal.md), install-from-source and
[background-service](README.md#running-as-a-service) instructions, a
[tested-radios table](README.md#tested-radios), and a [code map](docs/code-map.md).

### Receive
- **NBFM** — NOAA Weather Radio, FRS, amateur VHF/UHF; live retune/gain with
  no audio gap, noise squelch, CTCSS + DCS decode, always-on tone identifier.
- **Wideband FM** and **AM** broadcast with nearest-station finders and seek.
- **ADS-B** (1090 MHz Mode S), **AIS** (162 MHz), **APRS** (144.390 MHz
  AFSK/AX.25) trackers on a shared radar scope; Beast / AIVDM / TNC2 TCP feeds.
- **NOAA APT** (137 MHz) weather-satellite image, in-browser SGP4 pass
  prediction.
- **Receiver Analysis** — server-side FFT panadapter + waterfall, IQ `.wav`
  recording.
- Server-side **demod-audio recording** (any audio mode) and **channel scan**.

### Transmit (opt-in, `--enable-tx`)
- Half-duplex push-to-talk on **FRS** (personal-use experimentation only — an
  SDR is not Part 95 certified equipment) and **amateur FM** (Part 97
  homebrew; simplex or through a repeater with an encoded CTCSS/DCS uplink),
  with a transmit audit log.
- **APRS transmit** — `POST /aprs/tx` (message / position / raw).

### Voice ↔ text (optional `voice-text` feature)
- `POST /radio/tx/say` speaks typed text on the air (espeak-ng / Piper).
- Received-audio transcription via pure-Rust Whisper (`candle`):
  per-over on the PTT modes; a **45 s rolling buffer with overlapping ~26 s
  windows, text stitching, a repetition-loop guard and a junk filter** for the
  continuous carriers (NOAA Weather Radio). A **Weather text** panel in the
  `nbfm` web UI fills live, with `.txt` download + clear.

### Internet enrichment (on by default, `--flight-lookup false` to disable)
- Tapping an aircraft or vessel looks it up on **adsbdb.com** /
  **vesselfinder.com** — tail number / type / owner / route, or flag /
  tonnage / photo — via a server-side proxy with a TTL cache. The only part
  of LANline that reaches the internet.

### Platform
- One embedded web client, served from the C2 port — zero-config in a
  browser (`lanline.local:8730`) via an mDNS responder + a UDP beacon.
- **Android** WebView wrapper with native LAN discovery and a fleet chooser.
- `lanline-hypervisor` — one server per SDR, collision-free ports, aggregated
  discovery.
- TLS with an auto-generated self-signed cert (`--tls`); HackRF and
  ADALM-Pluto verified, any SoapySDR device expected to work.

## Development phases

See the [phase roadmap table](README.md#phase-roadmap) for the granular
`1a`…`2y` history (workspace + REST API, Opus/WebRTC, each receive mode, PTT,
DCS, the hypervisor, voice↔text, flight/vessel lookup, the STT rework).
