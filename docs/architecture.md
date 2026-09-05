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
  main.rs              wiring: parse config, probe device, start beacon + mDNS + HTTP(S)
  config.rs            CLI (clap) + resolved runtime settings
  tls.rs               --tls: load or self-sign (rcgen) the C2 cert, cache it
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

  analysis/
    mod.rs             AnalysisShared: Spectrum (panadapter + waterfall ring) +
                       Analyzer (overlapped windowed rustfft -> dBFS, fftshift)
                       + the IQ recorder handle
    iq_wav.rs          IqRecorder: 2-channel int16 RIFF/WAVE (I,Q), header backpatch

  radio/
    mod.rs             RadioManager: owns config + pipeline task, hot-apply, telemetry;
                       run_sdr (nbfm/wbfm/am -> Opus, picks FmChain/AmChain via
                       the Chain enum), run_adsb / run_ais / run_apt / run_analysis
                       (IQ -> tracks / raster / spectrum, no audio)
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
- **2k** — `ham` mode: the fourth consumer of `dsp::FmChain` (after
  `nbfm`/`wbfm`/`frs`), with amateur-NBFM catalog defaults (5 kHz deviation /
  16 kHz channel). All the domain knowledge is client-side in
  `web/src/ham.ts`: a per-band structure for 6 m – 23 cm, each carrying its
  edges, conventional repeater offset, named simplex/calling frequencies (+
  regular simplex grids), and the **voluntary ARRL band plan** as a list of
  `{loHz, hiHz, use, fm}` segments. The wizard renders band tabs, the
  simplex list as quick-tune rows, a band-bounded manual dial, and the
  segment map with a live "you are here" marker (`patchLive` moves it as the
  dial steps, since `structKey` only rebuilds the panel when the *band*
  changes). Every VHF/UHF FM band is open to all licence classes, so there's
  no privilege gating to enforce — the "privilege vs licence" hint is
  informational. Follow-up commits added **CTCSS tone squelch**
  ([below](#ctcss-tone-squelch)) and a **repeater directory + manual entry**
  ([below](#repeater-directory)). Still receive-only; keying a repeater
  through its input (offset + uplink tone encode) and DCS are the remaining
  follow-ups. Part 97 allows homebrew/experimental transmit gear under an
  amateur licence (the operator holds Amateur Extra, KZ4AZ), so a `ham` TX
  path is on firmer regulatory footing than `frs`'s.

### CTCSS tone squelch

`FmParams::ctcss_hz` (0 = off) attaches a `CtcssDetector` to `FmChain`. It
runs on the raw discriminator output (the amplitude-normalized instantaneous
frequency, so it's independent of RF gain and total deviation): a cheap
anti-alias low-pass, decimation to ~2 kHz, then a narrow RBJ band-pass
(`Biquad::bandpass`, Q ≈ 16) at the selected tone versus a ~230 Hz low-pass
"reference band" envelope. Presence = the band-pass envelope clears a small
absolute floor **and** is a solid fraction of the reference envelope; a
~200 ms confidence smoother with hysteresis (0.6 lock / 0.35 drop) turns
that into a stable `locked` bit, which is AND-ed into the squelch-open
decision the same way `noise_env` is. When CTCSS is active the recovered
audio also gets a ~300 Hz high-pass so the sub-audible tone isn't a rumble
under the voice. `ctcss_squelch: 0` keeps the detector running and reporting
(`dsp.ctcss_tone_hz`) but never gates — "monitor" mode.

Deliberately a single-tone *presence* check keyed to the tone the operator
selects, not a 50-tone decoder bank: a repeater publishes its required tone,
so that's the one you set. It won't reliably distinguish immediate CTCSS
neighbours (they're ~2–3 % apart and adult male voice fundamentals overlap
the low end), which is why the confidence smoother is slow and the band-pass
is as narrow as settling time allows. `Chain`'s `Fm`/`Am` variants are both
`Box`ed now — `FmChain` grew past the point where an unboxed enum variant
tripped `clippy::large_enum_variant`. DCS (23-bit Golay code, ~134 Hz
sub-carrier) is a different problem and stays deferred.

### Repeater directory

A third `ham` commit added a repeater picker. It's entirely client-side and
server-free (the server still only sees `mode: "ham"` + a `frequency_hz` +
`mode_params`): a **Simplex / Repeaters** toggle in the wizard swaps the
channel list for `web/src/ham.ts`'s `Repeater` records, filtered to the
current band and distance-sorted when a location is set. Tapping a repeater
tunes its **output** (the downlink — what you receive) and, if it transmits
a tone (`tsq_hz`), sets that as RX tone squelch; the **input** (`output_hz +
offset_hz`) and uplink `tone_hz` are stashed in `state.activeRepeater` for
the future TX path and shown in "Now playing". Manual repeaters live in
`localStorage` (`lanline.repeaters.manual`), merged ahead of the bundled
list; the add form pre-fills the offset from `conventionalOffsetHz()`.

The bundled list is `GET /repeaters.json` — `scripts/fetch-repeaters.mjs`
pulls [hearham.com](https://hearham.com)'s open API (one ~9 MB global JSON,
no key), filters to a region by the trailing state code in its free-form
`city` string, keeps FM-analog + operational rows in our VHF/UHF bands, and
writes a compact per-region file. RepeaterBook's export API now needs an
account, hence hearham; its coverage is thinner, so `HAM_REPEATER_REGIONS`
takes a comma list and the wizard leans on manual entry. `build.rs` +
`webui.rs` embed the file the same way as the FM/AM/NWR station DBs.

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

### TLS and the secure-context problem

`getUserMedia` (the push-to-talk mic) and `navigator.geolocation` (the
"📍 My location" button) are gated by browsers to a **secure context** —
`https://`, or `http://localhost` / `http://127.0.0.1`. Served over plain
HTTP from a LAN address, both silently do nothing: no prompt, no error the
UI can show. `--tls` makes the C2 port HTTPS so an off-box browser gets a
secure context.

`tls.rs` produces the cert. With `--tls-cert`/`--tls-key` it just reads
those PEM files (for a `mkcert`-issued cert your devices already trust).
Otherwise it self-signs with `rcgen` (ECDSA P-256, ~397-day validity), SANs
= `localhost`, `<mdns-name>.local`, `127.0.0.1`, `::1`, and every
non-loopback **IPv4** from `if-addrs` (global IPv6 is skipped — rotating
privacy addresses would just bloat the cert). The pair is cached under a
per-user state dir (`--tls-dir` to override) so the cert — and therefore the
browser's "proceed anyway" exception, which is keyed to the exact cert —
survives restarts. Regenerated only if the cache is missing or >300 days
old. The SHA-256 fingerprint is logged at startup so you can match it
against what the browser shows.

Serving switches from `axum::serve` to `axum-server`
(`from_tcp_rustls` + a `Handle` for graceful shutdown) on the `--tls` path;
the plain-HTTP path is unchanged. `axum-server`'s `tls-rustls-no-provider`
feature plus an explicit `rustls/ring` provider keeps `aws-lc-rs` (and its
CMake/NASM build) out of the tree — `ring` is already here via `webrtc`.

The scheme is threaded into everything that emits a URL: the startup log,
the mDNS advertisement and its `scheme` TXT key, the discovery beacon's
`scheme` field and `c2_base_url`, and `GET /api/v1/server`'s `scheme`. The
web client picks it up for free — it derives its API base from
`location.protocol`/`location.host` when served by the server, and a typed
host inherits the page's scheme. The Android WebView adds an
`onReceivedSslError` that accepts a bad cert only from a private/link-local
address or a `*.local` name (the only things this app ever connects to),
rejecting anything routable.

### Multiple radios

Device enumeration, probing and selection (`registry.rs` + `registry::soapy`)
have always been driver-agnostic — `SoapySDRDevice_enumerate("")` then, per
device, `listGains` / `getGainRange` / `getSampleRateRange` /
`getBandwidthRange` / `listAntennas` / `hasDCOffsetMode` / … into the
`DeviceInfo.rx` capability model. So a second radio needed no server code:
attach an ADALM-Pluto alongside the HackRF and both show up in
`GET /api/v1/devices`; `PUT /api/v1/device` opens the chosen one (its
`soapy_args` carries whatever the driver needs — e.g. `uri=usb:1.21.5` for
the Pluto over USB).

What `PUT /device` gained: it now clears the device-specific tuner fields
(antenna name, gain elements, overall gain, bandwidth) and re-clamps the
sample rate into the new device's range, so the config can't carry a
HackRF's `TX/RX` antenna or `LNA/AMP/VGA` element names into a Pluto (one
`PGA`, `A_BALANCED`). The web client adds a **Radio** picker in the Server
card (only when >1 non-`audio` device is present), greys out modes whose
home frequency is outside the selected device's `frequency_ranges_hz` (AM's
520 kHz vs the Pluto's 70 MHz floor), and its `pickSampleRate` now handles a
*continuous* rate range (Pluto: 65 kHz–61 MHz — use the wanted rate as-is)
as well as a discrete set (HackRF: 1–20 Msps in 1 MHz steps).

### Radio options panel

`web/src/radio-options.ts` is a modal (⚙ in "Now playing") over the raw
SoapySDR tuner knobs — PPM correction, per-element gain sliders, analog
bandwidth, sample rate, antenna, DC-offset mode, and free-form
`device_settings` key/values. It reads the selected device's real ranges
from `GET /api/v1/device` (`registry::probe` — `listGains` + each
`getGainElementRange`, `getBandwidthRange`, `getSampleRateRange`,
`listAntennas`, `hasDCOffsetMode`, …) and PATCHes `/api/v1/radio` with only
the controls the user actually touched.

Most of this already worked — `PATCH /radio` has always accepted the full
`tuner` object and `apply_patch` hot-applied frequency and gain. Two gaps got
filled for the panel to be honest: `run_sdr` now actually calls
`setBandwidth` (and `setDCOffsetMode` in both states, not just "on"), and
`apply_patch` bounces the pipeline for `bandwidth_hz` / `freq_correction_ppm`
/ `dc_offset_correction` / `device_settings` changes — they're applied only
at stream open, so a live edit needs a restart. The genuinely sweepable
knobs (frequency, gain) still go live with no gap.

Not done: `device.rx.setting_info` is still empty — enumerating a driver's
settings with their types/ranges/descriptions needs a raw
`SoapySDRDevice_getSettingInfo` call the `soapysdr` crate doesn't expose, so
the panel offers `device_settings` as free-text key/value rows for now.
IQ-balance mode is read (`has_iq_balance_mode`) but not written — the crate
only exposes `setIQBalance(complex)`, not the automatic-mode toggle.

### Receiver Analysis

The `analysis` mode (`radio::run_analysis` → `analysis::AnalysisShared`) is a
spectrum analyser, not a receiver: no demod, no audio. It reads raw IQ,
runs 50 %-overlapped windowed FFTs (`rustfft`, planned once per FFT size),
converts to dBFS power and `fftshift`s so DC sits centre, then EMA-averages
into the panadapter trace and pushes a fresh instantaneous row into a
2000-deep ring — rate-limited to `frame_rate_hz` regardless of block size.

`GET /api/v1/analysis/spectrum` serves that as a compact binary frame (magic
`LWF1`): the f32 averaged spectrum plus any waterfall rows produced since the
client's `seq` cursor, the rows quantised to `u8` over a *fixed wide* dB
window (−150…+10 dBFS). The client (`web/src/analysis.ts`, `AnalysisView`)
maps `u8` → colour over its *own* visible floor/ceiling, so dragging the
dynamic range or swapping colour map is instant and local — as in GQRX. It
keeps an offscreen waterfall bitmap it scrolls and blits (never redrawing
history), and layers on wheel- and pinch-zoom / drag-pan of the frequency
axis (interpolated past FFT resolution), an explicit centre-frequency box
plus click-to-retune-centre, a max-hold trace, dB grid, frequency ticks, a
time axis (from the *measured* row rate, not the requested one), and a hover
readout. FFT size / window /
frame rate are `mode_params` — changing them bounces the pipeline (planner
rebuild), which the poll loop notices via a `seq` reset and resyncs.

**IQ recording.** `POST /api/v1/analysis/record` tees the IQ block, as it's
read, into `analysis::iq_wav::IqRecorder` — a 2-channel int16 RIFF/WAVE
(interleaved I, Q at the device rate; the de-facto SDR IQ format). The header
size fields are backpatched on close, so a crash leaves a recoverable file.
A hard `max_secs` cap (default 60, ~0.5 GB max at the ceiling) auto-stops it;
`GET /api/v1/analysis/recording` streams the finished file off disk (via
`tokio_util::io::ReaderStream`, never buffered) as an attachment.

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
