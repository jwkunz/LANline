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
- **2f** — `apt` mode: `apt/demod.rs` is a fully self-contained physical-layer
  decoder (own NCO/FIR/biquad/resampler, unit-tested with a synthesized
  Sync-A/image-ramp scan line, no radio needed) — FM discriminate → synchronous
  AM detection of the 2400 Hz subcarrier → resample to the 4160 words/sec APT
  word rate → Sync-A cross-correlation locks each 2080-word scan line →
  auto-leveled 909-pixel-per-channel image, accumulated in `apt::Image` and
  exported as a small binary raster (`GET /api/v1/apt/{status,image}`, see
  [rest-api.md](rest-api.md#noaa-apt-image-export)). Web client
  (`web/src/apt.ts`) is a satellite-quick-pick + manual-frequency wizard with a
  canvas that `putImageData`s the raster directly — no image crate on the
  server, no `<img>` decode on the client. See
  [below](#apt-line-sync-confidence-metric) for how "am I locked onto a real
  pass" is decided. The wizard also predicts each satellite's next pass
  (AOS/max-elevation) entirely client-side via SGP4 (`satellite.js`) against
  a bundled TLE (`GET /apt-tle.json`, `scripts/fetch-apt-tle.mjs`) — no live
  pass-prediction API, so it keeps working with no WAN access, at the cost
  of the TLE snapshot going stale in 1-2 weeks (see
  [rest-api.md](rest-api.md#predicting-the-next-pass)).
- **2h** — `frs` mode: no new DSP at all — it's `dsp::FmChain` again (same
  one `nbfm`/`wbfm` use), just with narrower catalog defaults matching FRS's
  Part 95 emission mask, and a fixed 22-channel table
  (`web/src/frs.ts`) in place of a continuous tunable band or a
  station database (there's no directory to look up — anyone can be on any
  channel from anywhere). Receive-only; see
  [below](#frs-and-the-transmit-question) for why.

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

### APT line-sync confidence metric

`apt::demod::find_sync` reports a `sync_quality` alongside every decoded scan
line — a matched-filter SNR: the peak Sync-A correlation over the search
window, normalized by `sqrt(SYNC_LEN) * stddev(words)` (the noise floor a
matched filter of that length would produce against uncorrelated content). A
real Sync-A pulse correlates *coherently* (score grows with `SYNC_LEN`);
noise or image content correlates only by chance (score grows with
`sqrt(SYNC_LEN)`), so the ratio is large and level-independent for a true
lock and stays near a small constant for everything else. Synthetic tests put
real signal around 6–10 and noise's steady-state ceiling around 3–4; live
HackRF capture at 137.1 MHz with no satellite overhead stayed in the same
1.2–2.7 noise range, correctly never claiming a lock. 5.0 is used as the
"locked" threshold (`radio::run_apt`'s `squelch_open`, and the two
`apt::demod` unit tests).

An earlier version normalized by the *mean of the correlation curve itself*
across the search window instead of the raw words' stddev. That failed:
offsets a few words from a true sync still overlap most of its
low-pass-filtered pulse and so also score moderately high, inflating the
mean and collapsing peak/mean to ~2.3 for *both* real signal and noise —
discovered when a live 137.1 MHz capture with no pass overhead (visually
confirmed as pure static once rendered) still reported `sync_quality: 1.68`
and read as "locked" against the then-current (much lower) threshold.
Computing the noise floor from the words directly, rather than from the
correlation curve, avoids that self-contamination.

### FRS and the transmit question

FRS (Family Radio Service) is 22 fixed UHF channels — 462.5625–462.7125 MHz
(channels 1–7, shared with GMRS, ≤2 W), 467.5625–467.7125 MHz (channels 8–14,
FRS-exclusive, simplex only, ≤0.5 W), and 462.5500–462.7250 MHz (channels
15–22, interstitial, shared with GMRS, ≤2 W) — all narrowband FM (≤2.5 kHz
deviation, 12.5 kHz channel spacing). `web/src/frs.ts` hardcodes this table
directly (it's a fixed FCC Part 95 channel plan, not something that changes
or needs fetching); receive uses it exactly like `nbfm`, just retuning
`FmChain` to each channel's frequency.

There's no privacy-code (CTCSS/DCS tone squelch) matching yet — the receiver
demodulates and passes through everything on a channel regardless of tone.
That's arguably the more useful behavior for a monitor/scanner tool anyway
(privacy codes only quiet a *transmitting* radio's speaker for
non-matching traffic; they were never a privacy mechanism against a receiver
built to listen to everything), but it does mean this won't yet mimic a
consumer FRS handset's tone-based squelching if that's ever wanted.

**The regulatory reality, stated plainly and unconditionally:** FRS is a
Part 95 certified-equipment service — a general-purpose SDR like a HackRF
is not type-accepted for FRS transmission, so keying up on an FRS frequency
with one isn't compliant with FCC rules regardless of power level or
intent. That's true of everything below. It's built anyway, at the
explicit, informed request of this project's owner for personal,
non-distributed use on their own equipment — not something this codebase
decides for anyone else. Transmit is off by default (`--enable-tx`) and
requires deliberate opt-in.

#### Push-to-talk transmit

`frs` is the one mode with `tx_capable: true`. `POST /api/v1/radio/tx/key`
(see [rest-api.md](rest-api.md#post-apiv1radiotxkey)) begins a
transmission; `POST /api/v1/radio/tx/unkey` ends it early (a hard 10s
server-side cap ends it regardless, so a lost `unkey` request can't leave
the transmitter keyed indefinitely). Both are gated behind `--enable-tx` +
`frs` mode + a tx-capable device + the radio already running, checked in
that order so the server-policy gate always answers first regardless of
anything else.

**Half-duplex hand-off.** HackRF (like most low-cost SDRs) can't RX and TX
at once — confirmed directly against this project's own unit
(`SoapySDRUtil --probe`: "Full-duplex: NO" on both channels). `radio::key_tx`
owns the full hand-off inside the same pipeline thread that already owns
the device: deactivate the RX stream, retune straight to the TX frequency
(no LO-offset digital mixing on the way out, unlike RX — there's no DC-spike
concern to dodge when transmitting), open and activate a TX stream, run
until unkeyed/timed out/stopped, deactivate TX, retune back and reactivate
RX. The caller (`run_sdr`) handles the "resume RX" half; `key_tx` handles
everything from "stop RX" through "stop TX".

**Live mic audio.** The uplink rides the *same* WebRTC connection the
receive audio already uses (see
[Audio — WebRTC signaling](rest-api.md#audio--webrtc-signaling)) — when
FRS's PTT wizard has a mic stream, `web/src/audio.ts` adds it as an
outgoing track via `RTCPeerConnection.addTrack`, which negotiates the
connection bidirectionally by default; nothing else has to know or care
that direction was negotiated one way or the other. Server-side,
`media::WebrtcEngine`'s `on_track` handler decodes the incoming Opus RTP
packets (an RTP payload *is* one Opus frame — no NALU-style depacketization
needed) into PCM and pushes it into `radio::TxAudioSource`, a small global
ring buffer (only one physical radio exists, so only one transmission can
be live regardless of how many sessions are connected). `key_tx` pulls from
it every iteration, silence-padded on underrun so a network/decode hiccup
never stalls the TX loop, and feeds a `dsp::TxModulator` — a persistent
phase accumulator wrapped around the same `LinearResampler` the RX side
uses (just upsampling 48kHz→device-rate instead of decimating the other
way) — which frequency-modulates it onto a baseband carrier. `TxModulator`
is unit-tested by round-tripping ragged, irregularly-sized audio chunks
(mimicking live Opus frame arrival, not one tidy buffer) back through
`FmChain` and confirming the recovered tone survives.

**A real bug this surfaced, worth recording:** the first end-to-end test
showed the RF path working (clean keying, correct deviation on receive) but
no audio coming through — `micStream` was simply never `null`-checked into
existence in some real usage flows. The mic is acquired lazily, inside
`startAudio()`, which several call sites only invoke when
`state.audioState === "idle"` — but `AudioSession.start()` silently no-ops
if a peer connection already exists, and pressing "Start radio" (as
opposed to "Play", or an automatic connect-time autostart) never calls
`startAudio()` at all. So a real session could reach PTT with a
`RadioConfig` in `frs` mode, running, connected — and still never have
asked for a microphone. Fixed with `ensureMicAttached()`, called at the top
of `pttDown()` itself: if there's no mic yet, acquire one, and if a
(recvonly) peer connection is already up, tear it down and rebuild it with
the mic attached — a brief RX audio blip, but only the first time PTT is
used in a session. The lesson: a feature gated behind "was some *other*
code path called first" needs to also make itself work when that path
wasn't taken, not just document that it should have been.

**Two more, found the same way — live, against real hardware, after the code
"should have worked":**

- *Transmit ignored channel changes.* `run_sdr` takes its `SdrParams` by
  value and immutably; the live-retune command (`PipelineCmd::Retune`) moved
  the hardware but nothing recorded the new frequency, so `key_tx` — and the
  RX-restore after a key — both read the stale start-of-pipeline frequency.
  PTT on channel 5 transmitted on channel 1, and keying anywhere snapped RX
  back to channel 1. Fixed by tracking the live frequency in a mutable local
  in `run_sdr`, updated on every `Retune` and passed explicitly into
  `key_tx`. This was invisible before PTT existed (nothing read the value
  back) and only bites a mode with both a channel picker and transmit.

- *Android WebView needs `MODIFY_AUDIO_SETTINGS`, not just `RECORD_AUDIO`.*
  Chromium's `AudioManagerAndroid` refuses to open a capture device without
  *both* permissions ("Requires MODIFY_AUDIO_SETTINGS and RECORD_AUDIO. No
  audio device will be available for recording"), so `getUserMedia` rejected,
  `acquireMicIfNeeded()` returned `null`, and PTT keyed up transmitting
  silence — with no user-visible error. It "worked" when tested in a
  standalone browser because full Chrome ships that permission itself.
  `MODIFY_AUDIO_SETTINGS` is a normal (install-time, no-prompt) permission;
  it's now in `client/android`'s manifest alongside `RECORD_AUDIO`.

**Deviation and mic gain, live-tuned.** The catalog ships `deviation_hz: 4000`
/ `channel_bw_hz: 14000` for `frs` — wider than FRS's Part 95 narrowband mask
(2.5 kHz / 12.5 kHz). The legally-narrowband defaults were the first thing
tried; on receive, a real handheld's transmission sounded weak and
under-modulated at 2500 Hz, and 4000/14000 sounded correct. This shifts
FRS's occupied bandwidth slightly past its nominal 12.5 kHz channel
spacing — a real, known tradeoff (adjacent-channel splatter in a dense RF
environment), acceptable for a personal unit talking to one specific
handheld in relative isolation, and easy to dial back down via
`mode_params` if that ever matters.

Deviation alone wasn't enough on transmit: getUserMedia PCM (especially from
the Android WebView) comes in far below full scale, so even at 4000 Hz peak
deviation the *actual* swing on quiet mic audio was a few hundred Hz and the
far radio was barely audible. `tx_mic_gain` (a `frs` mode param, default
`2.5`) is a plain linear multiplier applied to the decoded PCM in `key_tx`
before it reaches `TxModulator`, then hard-clamped to ±1.0 so peak deviation
stays capped at `deviation_hz` no matter how hot the gain. It was dialled in
by ear against a handheld — 6× clipped and sounded harsh, 1× was the
original "too quiet", 2.5× sat right. Both `deviation_hz` and `tx_mic_gain`
are read fresh at each key-up, so `PATCH /api/v1/radio` retunes the transmit
audio without restarting anything.

No privacy-code (CTCSS/DCS) tone gets added to the transmitted audio
either — same reasoning as the receive side above; not implemented, not
currently planned.

### HackRF frequency calibration

`tuner.freq_correction_ppm` (in the radio config since phase 1, but never
actually wired to hardware until this was chased down) corrects a device's
crystal error in software: every tuned frequency, on every mode, is
multiplied by `(1 + ppm/1e6)` (`radio::apply_ppm`) before being handed to
SoapySDR. This works on any device — it doesn't depend on
`device.rx.has_frequency_correction` (a SoapySDR "CORR" tunable component,
which HackRF's driver doesn't implement; that's presumably why this field
was left unwired originally). Set it once at startup (`--freq-correction-
ppm`, `LANLINE_FREQ_CORRECTION_PPM`) or live via `PATCH /radio`'s
`tuner.freq_correction_ppm`; `0` (default) applies no correction.

This was found and fixed the hard way: a live FRS bench test (real HackRF,
real handheld, channel 1) reported a keyed carrier and a clean, non-clipped
signal chain, yet no intelligible audio came through — just what sounded
like a steady tone. Inspecting the raw pre-Opus samples (`--dump-wav`)
showed why: the discriminator output was sitting at a **steady ~‑0.82** (on
a ±1.0 scale) the entire time, nowhere near the expected near-zero average
for a real voice signal, with only a small ripple riding on top of that huge
bias. A persistent, non-oscillating discriminator bias like that means the
receiver's effective center frequency doesn't match the real carrier — at
the `deviation_hz` in use for this test (5000, widened from the shipped
default to make the bias easy to read), `bias × deviation_hz` gives the
actual offset: **≈‑4.1 kHz**, or ≈‑8.9 ppm at 462.5625 MHz. Retuning ‑4.1 kHz
lower dropped that bias to ≈0.02 and the level immediately started showing
real dynamic range (long quiet stretches, sharp louder excursions) — the
shape of actual speech, not a stuck rail.

A HackRF's stock TCXO commonly drifts by a few to a few tens of ppm — this
±8.9 ppm on the bench unit here is unremarkable as these things go. It was
invisible on every mode built before FRS (137–166 MHz, ±5–75 kHz deviation)
because a few kHz of absolute error is negligible against that much margin;
FRS's tight ±2.5–5 kHz window was the first to actually expose it. Any UHF
narrowband mode — FRS today, GMRS or a PTT transmit path later — inherits
the same exposure and the same fix.

**To determine your own device's ppm**, without needing a signal generator:
tune to *any* frequency you can reliably get a clean, on-frequency carrier
on (a nearby FM broadcast station, a NOAA weather radio transmitter, or —
as here — someone else's handheld on a known channel), widen `deviation_hz`
well beyond what should be needed so the discriminator can't saturate, hold
a steady key-up, and look at the mean of the raw samples (`--dump-wav`) or
`ChainMetrics`. A steady non-zero bias `b` (on the chain's ±1.0 scale) at
that `deviation_hz` setting means an offset of `b × deviation_hz` Hz;
convert to ppm by dividing by the tuned frequency and multiplying by 1e6.
The sign of `freq_correction_ppm` to apply isn't worth deriving from first
principles (it depends on the NCO/mixing convention) — just try it, and
flip the sign if the bias grows instead of shrinking on the next capture.
