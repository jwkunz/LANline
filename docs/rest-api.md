# LANline REST API

Version: `v1` · Protocol version: `1`

The command-and-control port advertised by the [discovery beacon](beacon-protocol.md)
serves this API. All paths below are relative to
`http://<host>:<c2_port>`.

The C2 port is **fixed at `8730` by default** (override with `--c2-port`, or `0`
for a random free port) so a bookmarked web-client URL survives a restart. The
same port also serves the bundled web client and is advertised over mDNS as
`http://<mdns-name>.local:<c2_port>/` (default `lanline.local`).

- Request and response bodies are `application/json; charset=utf-8`.
- Every path is under `/api/v1` except `GET /health` and the
  [web client](#web-client).
- Timestamps are RFC 3339 / ISO 8601 UTC strings (e.g. `2026-09-03T17:04:11Z`).
- Frequencies are integer **Hz**, gains are **dB** (float), durations are
  seconds unless a field name says `_ms`.

## Contents

- [Web client](#web-client)
- [Authentication](#authentication)
- [CORS](#cors)
- [Error model](#error-model)
- [Meta](#meta)
- [Devices](#devices)
- [Modes](#modes)
- [ADS-B track export](#adsb-track-export)
- [AIS vessel-track export](#ais-vessel-track-export)
- [APRS station-track export](#aprs-station-track-export)
- [NOAA APT image export](#noaa-apt-image-export)
- [Presets](#presets)
- [Radio configuration & control](#radio-configuration--control)
- [Sessions](#sessions)
- [Audio — WebRTC signaling](#audio--webrtc-signaling)
- [Reserved for phase 2](#reserved-for-phase-2)
- [Data types](#data-types)

---

## Web client

The server embeds the built web client and serves it from the C2 port, so a
browser pointed at the server's own address connects with nothing to type
(same-origin — no host field, no CORS pre-flight).

| Path | Response |
|------|----------|
| `GET /` , `GET /index.html` | `text/html` — the single-file web app |
| `GET /nwr-stations.json` | `application/json` — bundled NOAA Weather Radio transmitter directory |
| `GET /fm-stations.json` | `application/json` — bundled FCC FM broadcast station directory |
| `GET /am-stations.json` | `application/json` — bundled FCC AM broadcast station directory |
| `GET /apt-tle.json` | `application/json` — bundled NOAA-15/18/19 orbital elements (TLE), for the client-side pass predictor |
| `GET /repeaters.json` | `application/json` — bundled **nationwide (US)** amateur repeater directory (`scripts/fetch-repeaters.mjs` from hearham.com; the web client sorts/filters it by the operator's location like `fm-stations.json`. `HAM_REPEATER_REGIONS` / `HAM_REPEATER_CENTER`+`HAM_REPEATER_RADIUS_MI` build a smaller bundle if wanted) |

These are unauthenticated and cached (`Cache-Control: public, max-age=86400`).
The same bundle is what the Android APK embeds as local assets.

## Authentication

`POST /api/v1/sessions` is unauthenticated. Every other endpoint except the
[Meta](#meta) group requires a bearer token issued by that call:

```
Authorization: Bearer <session_token>
```

Tokens are opaque, random, held in server memory only, and invalidated when the
session ends or is reaped. A missing/'unknown/expired token → `401` with code
`unauthorized`.

There is no user model in phase 1 — a token identifies a *session*, not a
person. The server is expected to run on a trusted LAN.

## CORS

When the web client is served from the C2 port itself (the default), requests
are same-origin and CORS does not apply. It still matters for the Vite dev
server, a separate static host, or the Android `file://` wrapper: the server
sends permissive CORS headers for `GET`/`HEAD` and echoes an allow-list of
origins (configurable, default `*` in dev) for the other methods, plus
`Authorization` and `Content-Type` in `Access-Control-Allow-Headers`, with
`Access-Control-Max-Age: 3600` so a burst of `PATCH`es pre-flights once.
Pre-flight `OPTIONS` is handled for every route.

## Error model

Non-2xx responses carry:

```json
{
  "error": {
    "code": "invalid_parameter",
    "message": "sample_rate_hz 100000 is outside the supported ranges",
    "details": {
      "field": "tuner.sample_rate_hz",
      "supported_ranges_hz": [{ "min": 2000000, "max": 20000000, "step": 0 }]
    }
  }
}
```

| HTTP | `code` | When |
|-----:|--------|------|
| 400 | `bad_request` | Malformed JSON / wrong shape |
| 401 | `unauthorized` | Missing / invalid session token |
| 404 | `not_found` | Unknown session id, preset id, route |
| 409 | `conflict` | Operation not allowed in the current state (e.g. select a device while streaming) |
| 422 | `invalid_parameter` | Well-formed but semantically invalid (out-of-range value, unknown gain element / setting key / mode) |
| 429 | `too_many_sessions` | Session cap reached |
| 500 | `internal` | Unexpected server fault |
| 501 | `not_implemented` | Reserved phase-2 endpoint |
| 503 | `device_unavailable` | No SDR present / device in error and the operation needs it |

---

## Meta

No authentication required.

### `GET /health`

```json
{ "status": "ok", "uptime_s": 1234.5, "version": "0.1.0" }
```

### `GET /api/v1/server`

Server identity and current capability summary.

```json
{
  "server_id": "5f2b8b0e-2c1a-4d7e-9a3e-1b6d9c0a77aa",
  "protocol_version": 1,
  "version": "1.1.0",
  "hostname": "bench-linux",
  "instance_label": null,
  "fleet_url": null,
  "scheme": "http",
  "time": "2026-09-03T17:04:11Z",
  "ports": { "c2": 8730, "audio_out": 49213, "audio_in": 60731, "beast": 30005, "ais_nmea": 10110, "aprs": 10152 },
  "capabilities": ["rx", "webrtc", "nbfm", "wbfm", "am", "adsb", "ais", "debug_tone"],
  "selected_device": {
    "id": "hackrf/0000000000000000457863c8...",
    "driver": "hackrf",
    "label": "HackRF One",
    "tx_capable": true
  }
}
```

`capabilities` is dynamic: `"tx"` appears only when the selected device
supports transmit; `"tts"` / `"stt"` with the `voice-text` build + flags;
`"flight-lookup"` unless disabled with `--flight-lookup false`.
`selected_device` is `null` when none is selected.
`scheme` is `"https"` when the server was started with `--tls` (the C2 port
is then HTTPS-only); a browser served the client over HTTPS is a secure
context, so `getUserMedia` (push-to-talk mic) and the geolocation button work
from any LAN address, not just `localhost`.
`ports.beast` is the TCP port of the [Beast Mode S feed](#adsb-track-export),
`ports.ais_nmea` the TCP port of the [AIVDM feed](#ais-vessel-track-export),
`ports.aprs` the TCP port of the [TNC2 APRS feed](#aprs-station-track-export);
each is `0` when disabled (`--beast-port 0` / `--ais-nmea-port 0` /
`--aprs-port 0`).
`instance_label` is a human name for this radio, set with `--instance-label`
(or by [`lanline-hypervisor`](hypervisor.md) for each child it runs); `null`
on a plain standalone server. `fleet_url` is the hypervisor's fleet page for
this radio (`--fleet-url`); the web client shows a "⇤ Fleet" link back to it
when set. `--server-id <uuid>` pins `server_id` across restarts (the
hypervisor uses it so clients don't see a supervised restart as a new
server). The hypervisor itself exposes a styled radio picker at `GET /` and
`GET /api/v1/fleet` on its own port (default `8720`) listing every child
radio — see [hypervisor.md](hypervisor.md).

---

## Devices

A device is any SoapySDR device the server can enumerate. The server selects one
at a time for the receive pipeline.

### `GET /api/v1/devices`

```json
[
  {
    "id": "hackrf/0000000000000000457863c8...",
    "driver": "hackrf",
    "label": "HackRF One",
    "serial": "0000000000000000457863c8...",
    "soapy_args": "driver=hackrf,serial=0000000000000000457863c8...",
    "tx_capable": true,
    "available": true
  }
]
```

`id` is a stable `"<driver>/<serial>"` key (falls back to an enumeration index
when a driver reports no serial). `available` is `false` when the device is
present but already opened by another process.

### `GET /api/v1/device`

The **selected** device with its full, driver-agnostic capability model
(everything here is read straight from SoapySDR). `404` with code `not_found`
if no device is selected.

```json
{
  "id": "hackrf/0000000000000000457863c8...",
  "driver": "hackrf",
  "label": "HackRF One",
  "serial": "0000000000000000457863c8...",
  "soapy_args": "driver=hackrf,serial=0000000000000000457863c8...",
  "status": "ready",
  "tx_capable": true,
  "rx": {
    "channels": 1,
    "antennas": ["TX/RX"],
    "frequency_ranges_hz":   [{ "min": 1000000, "max": 6000000000, "step": 0 }],
    "sample_rate_ranges_hz": [{ "min": 2000000, "max": 20000000, "step": 0 }],
    "bandwidth_ranges_hz":   [{ "min": 1750000, "max": 28000000, "step": 0 }],
    "gain_elements": [
      { "name": "AMP", "range_db": { "min": 0, "max": 14, "step": 14 } },
      { "name": "LNA", "range_db": { "min": 0, "max": 40, "step": 8 } },
      { "name": "VGA", "range_db": { "min": 0, "max": 62, "step": 2 } }
    ],
    "overall_gain_range_db": { "min": 0, "max": 116, "step": 1 },
    "has_agc": false,
    "has_dc_offset_mode": true,
    "has_iq_balance_mode": true,
    "has_frequency_correction": true,
    "sensors": [],
    "setting_info": [
      {
        "key": "bias_tx",
        "name": "Antenna Bias Tee Power",
        "type": "bool",
        "default": "false",
        "description": "Enable the antenna port power (bias tee)",
        "options": null
      }
    ]
  }
}
```

`status`: `ready` · `busy` · `error` · `absent`.

### `PUT /api/v1/device`

Select the device for the receive pipeline. Body is one of:

```json
{ "id": "rtlsdr/00000001" }
```
```json
{ "soapy_args": "driver=plutosdr,uri=ip:192.168.2.1" }
```

- `409 conflict` if the pipeline is running — call `POST /api/v1/radio/stop`
  first.
- On success the server opens the device, re-probes capabilities, clamps the
  current [`radio`](#radio-configuration--control) config to the new device's
  ranges (resetting fields it cannot honour to device defaults), updates the
  [beacon](beacon-protocol.md), and returns the new `GET /api/v1/device` body.
- `503 device_unavailable` if the device cannot be opened.

### `GET /api/v1/device/health`

```json
{
  "present": true,
  "status": "ready",
  "last_error": null,
  "samples_read": 184320000,
  "overruns": 0,
  "sensors": {}
}
```

---

## Modes

Demodulation modes are device-independent. Each advertises a parameter schema
the client uses to render controls and the server uses to validate
`mode_params`.

### `GET /api/v1/modes`

```json
[
  {
    "id": "nbfm",
    "name": "Narrowband FM",
    "tx_capable": false,
    "params": {
      "deviation_hz":  { "type": "number", "default": 5000,  "min": 1000, "max": 15000, "unit": "Hz" },
      "channel_bw_hz": { "type": "number", "default": 16000, "min": 8000, "max": 25000, "unit": "Hz" },
      "deemphasis_us": { "type": "number", "default": 75, "enum": [0, 50, 75], "unit": "us" },
      "audio_lpf_hz":  { "type": "number", "default": 3400,  "min": 1000, "max": 8000, "unit": "Hz" },
      "squelch_db":    { "type": "number", "default": -80, "min": -120, "max": 0, "unit": "dBFS" },
      "noise_squelch": { "type": "number", "default": 0.18, "min": 0.02, "max": 2.0, "unit": "ratio" }
    }
  },
  {
    "id": "wbfm",
    "name": "Wideband FM (broadcast)",
    "tx_capable": false,
    "params": {
      "deviation_hz":  { "type": "number", "default": 75000,  "min": 50000,  "max": 100000, "unit": "Hz" },
      "channel_bw_hz": { "type": "number", "default": 200000, "min": 120000, "max": 256000, "unit": "Hz" },
      "deemphasis_us": { "type": "number", "default": 75, "enum": [0, 50, 75], "unit": "us" },
      "audio_lpf_hz":  { "type": "number", "default": 15000, "min": 5000, "max": 17000, "unit": "Hz" },
      "squelch_db":    { "type": "number", "default": -120, "min": -120, "max": 0, "unit": "dBFS" },
      "noise_squelch": { "type": "number", "default": 2.0, "min": 0.02, "max": 2.0, "unit": "ratio" }
    }
  },
  {
    "id": "am",
    "name": "AM (mediumwave / shortwave)",
    "tx_capable": false,
    "params": {
      "channel_bw_hz": { "type": "number", "default": 10000, "min": 6000, "max": 16000, "unit": "Hz" },
      "audio_lpf_hz":  { "type": "number", "default": 5000,  "min": 2000, "max": 8000,  "unit": "Hz" },
      "squelch_db":    { "type": "number", "default": -80, "min": -120, "max": 0, "unit": "dBFS" },
      "noise_squelch": { "type": "number", "default": 0.5, "min": 0.02, "max": 2.0, "unit": "ratio" }
    }
  },
  {
    "id": "frs",
    "name": "FRS (Family Radio Service, 462/467 MHz)",
    "tx_capable": true,
    "params": {
      "deviation_hz":  { "type": "number", "default": 4000,  "min": 1000, "max": 5000,  "unit": "Hz" },
      "channel_bw_hz": { "type": "number", "default": 14000, "min": 8000, "max": 16000, "unit": "Hz" },
      "tx_mic_gain":   { "type": "number", "default": 2.5, "min": 1.0, "max": 32.0, "unit": "x" },
      "deemphasis_us": { "type": "number", "default": 0, "enum": [0, 50, 75], "unit": "us" },
      "audio_lpf_hz":  { "type": "number", "default": 3000,  "min": 1000, "max": 4000,  "unit": "Hz" },
      "squelch_db":    { "type": "number", "default": -80, "min": -120, "max": 0, "unit": "dBFS" },
      "noise_squelch": { "type": "number", "default": 0.3, "min": 0.02, "max": 2.0, "unit": "ratio" }
    }
  },
  {
    "id": "ham",
    "name": "Amateur NBFM (VHF/UHF FM simplex & repeaters)",
    "tx_capable": true,
    "params": {
      "deviation_hz":  { "type": "number", "default": 5000,  "min": 1000, "max": 8000,  "unit": "Hz" },
      "channel_bw_hz": { "type": "number", "default": 16000, "min": 8000, "max": 25000, "unit": "Hz" },
      "deemphasis_us": { "type": "number", "default": 0, "enum": [0, 50, 75], "unit": "us" },
      "audio_lpf_hz":  { "type": "number", "default": 3400,  "min": 1000, "max": 8000,  "unit": "Hz" },
      "squelch_db":    { "type": "number", "default": -80, "min": -120, "max": 0, "unit": "dBFS" },
      "noise_squelch": { "type": "number", "default": 0.35, "min": 0.02, "max": 2.0, "unit": "ratio" },
      "tx_mic_gain":   { "type": "number", "default": 2.5, "min": 1.0, "max": 32.0, "unit": "x" },
      "ctcss_hz":      { "type": "number", "default": 0, "min": 0, "max": 260, "unit": "Hz" },
      "ctcss_squelch": { "type": "number", "default": 1, "enum": [0, 1], "unit": "bool" },
      "dcs_code":      { "type": "number", "default": 0, "min": 0, "max": 777, "unit": "octal" },
      "dcs_invert":    { "type": "number", "default": 0, "enum": [0, 1], "unit": "bool" },
      "dcs_squelch":   { "type": "number", "default": 1, "enum": [0, 1], "unit": "bool" }
    }
  },
  {
    "id": "apt",
    "name": "NOAA APT (137 MHz weather satellite)",
    "tx_capable": false,
    "params": {
      "deviation_hz":  { "type": "number", "default": 17000, "min": 10000, "max": 25000, "unit": "Hz" },
      "channel_bw_hz": { "type": "number", "default": 40000, "min": 20000, "max": 60000, "unit": "Hz" },
      "max_lines":     { "type": "number", "default": 1200,  "min": 100,   "max": 4000,  "unit": "lines" }
    }
  },
  {
    "id": "adsb",
    "name": "ADS-B (1090 MHz aircraft)",
    "tx_capable": false,
    "params": {
      "reference_lat":  { "type": "number", "default": 0, "min": -90,  "max": 90,  "unit": "deg" },
      "reference_lon":  { "type": "number", "default": 0, "min": -180, "max": 180, "unit": "deg" },
      "max_range_nm":   { "type": "number", "default": 250, "min": 10, "max": 500, "unit": "NM" },
      "trail_seconds":  { "type": "number", "default": 120, "min": 10, "max": 600, "unit": "s" },
      "forget_seconds": { "type": "number", "default": 60,  "min": 10, "max": 600, "unit": "s" },
      "fix_errors":     { "type": "number", "default": 1, "enum": [0, 1], "unit": "bool" }
    }
  },
  {
    "id": "aprs",
    "name": "APRS (144.390 MHz packet — position/status/message)",
    "tx_capable": false,
    "params": {
      "reference_lat":  { "type": "number", "default": 0, "min": -90,  "max": 90,   "unit": "deg" },
      "reference_lon":  { "type": "number", "default": 0, "min": -180, "max": 180,  "unit": "deg" },
      "max_range_km":   { "type": "number", "default": 300, "min": 10, "max": 2000,  "unit": "km" },
      "trail_seconds":  { "type": "number", "default": 1800, "min": 60, "max": 21600, "unit": "s" },
      "forget_seconds": { "type": "number", "default": 3600, "min": 300, "max": 86400, "unit": "s" }
    }
  },
  {
    "id": "ais",
    "name": "AIS (161.975 / 162.025 MHz vessels)",
    "tx_capable": false,
    "params": {
      "reference_lat":  { "type": "number", "default": 0, "min": -90,  "max": 90,  "unit": "deg" },
      "reference_lon":  { "type": "number", "default": 0, "min": -180, "max": 180, "unit": "deg" },
      "max_range_nm":   { "type": "number", "default": 60,  "min": 5,  "max": 200,  "unit": "NM" },
      "trail_seconds":  { "type": "number", "default": 600, "min": 30, "max": 3600, "unit": "s" },
      "forget_seconds": { "type": "number", "default": 900, "min": 60, "max": 3600, "unit": "s" }
    }
  },
  {
    "id": "analysis",
    "name": "Receiver Analysis (FFT panadapter + waterfall)",
    "tx_capable": false,
    "params": {
      "fft_size":      { "type": "number", "default": 4096, "enum": [1024, 2048, 4096, 8192, 16384], "unit": "bins" },
      "window":        { "type": "number", "default": 0, "enum": [0, 1, 2, 3], "unit": "code" },
      "frame_rate_hz": { "type": "number", "default": 20, "min": 1, "max": 60, "unit": "fps" },
      "averaging":     { "type": "number", "default": 0.5, "min": 0, "max": 0.98, "unit": "ratio" }
    }
  },
  {
    "id": "debug_tone",
    "name": "Debug Tone (A4 440 Hz)",
    "tx_capable": false,
    "params": {
      "tone_hz":    { "type": "number", "default": 440, "min": 20, "max": 20000, "unit": "Hz" },
      "level_dbfs": { "type": "number", "default": -12, "min": -60, "max": -1, "unit": "dBFS" }
    }
  }
]
```

`analysis` produces no audio: the server runs overlapped windowed FFTs on the
raw IQ and serves them for the web client's interactive waterfall. `window`
codes: `0` Hann, `1` Blackman, `2` Blackman-Harris, `3` rectangular. See the
`/analysis/*` endpoints below.

`nbfm` and `wbfm` share one DSP chain (LO-offset NCO → FIR decimation → polar
discriminator → de-emphasis → audio LPF → resample); the parameters scale it
between ~16 kHz voice channels and ~200 kHz broadcast channels. `wbfm` is mono
and notches the 19 kHz stereo pilot; it wants a wider `tuner.sample_rate_hz`
(e.g. 4 MHz on a HackRF).

`am` is a plain AGC'd envelope detector (`(mag − slow_avg) / slow_avg`) sharing
the same decimate/squelch/resample skeleton as the FM chain, tuned for
mediumwave/shortwave broadcast (~10 kHz channels). Whether the attached SDR
can actually pull in AM depends on its LF/MF front end — see the note in
[architecture.md](architecture.md#am-and-hackrf-mfLF-sensitivity).

`frs` receives on the exact same `FmChain` as `nbfm`/`wbfm` — no new DSP —
plus a fixed 22-channel frequency table instead of a continuous tunable
band. Deviation/channel-bandwidth defaults (4000/14000 Hz) are wider than
FRS's Part 95 narrowband mask (2.5 kHz/12.5 kHz); live-tuned against a real
handheld after the tighter, legally-narrowband defaults sounded
under-modulated on receive. It also supports push-to-talk transmit — see
[architecture.md](architecture.md#frs-and-the-transmit-question) for the
channel plan, the PTT endpoints (`/radio/tx/key`, `/radio/tx/unkey`), and
the FCC Part 95 equipment-certification caveat that applies regardless of
what this does or doesn't transmit.

`ham` is the same `FmChain` as `nbfm`/`frs` with slightly wider defaults
(5 kHz deviation / 16 kHz channel) for amateur VHF/UHF NBFM. The server side
is just another FM mode; the band-plan structure (6 m – 23 cm, simplex/calling
frequencies, and the voluntary ARRL segment map) lives entirely in the web
client (`web/src/ham.ts`) — the API only sees `mode: "ham"` plus a
`frequency_hz` anywhere in an amateur band.

`ctcss_hz` adds **sub-audible tone squelch** (CTCSS / "PL"). Set it to a
standard tone (67.0–254.1 Hz) and the channel only unmutes while a
[`CtcssDetector`](architecture.md#ctcss-tone-squelch) confirms that tone is
present; the recovered audio is also high-passed ~300 Hz so the tone isn't
an audible rumble. `ctcss_squelch: 0` detects and reports the tone (see
`dsp.ctcss_tone_hz` in `GET /radio/status`) without ever withholding audio —
"monitor" mode. `0` disables it (carrier squelch only). It's a single-tone
*presence* check keyed to the tone you select, not a 50-tone decoder bank.
`dcs_code` is the **DCS (Digital Coded Squelch)** alternative — the 3 octal
digits written in decimal (`23` = D023), `0` = off, mutually exclusive with
`ctcss_hz` (a tone wins). `dcs_invert` selects the "I" codes; `dcs_squelch`
`0` is monitor mode. A `DcsDecoder`
([architecture.md](architecture.md#dcs-digital-coded-squelch)) recovers the
134.4 bps stream and matches the Golay(23,12) codeword; `dsp.dcs_code` in
`GET /radio/status` reports it while locked.

Live-editable via `PATCH /api/v1/radio` (`mode_params` is deep-merged), so
changing the tone/code or toggling monitor mode takes effect without a
restart.

The repeater picker (a bundled nationwide directory at `GET /repeaters.json`,
distance-sorted client-side to the operator's location, plus browser-stored
manual entries) is entirely client-side: selecting a repeater issues a
`PATCH /api/v1/radio` that sets `frequency_hz` to the repeater's output and
`mode_params.ctcss_hz` to its transmitted tone, if any.
`ham` is `tx_capable` — Part 97 permits homebrew transmit equipment under an
amateur licence (firmer footing than `frs`'s Part 95 situation). PTT is still
`--enable-tx`-gated; `POST /api/v1/radio/tx/key` takes `offset_hz` plus an
encoded uplink — `tone_hz` (CTCSS) or `dcs_code`/`dcs_invert` (DCS) — to key
a repeater through its input (the web client fills these from the selected
repeater / the wizard's squelch controls).

`debug_tone` synthesizes audio internally and does **not** touch the SDR — it
works with no device selected or a device in `error`, and is the end-to-end
check for the WebRTC path.

`apt` is likewise **non-audio**: it FM-demodulates a 137 MHz NOAA POES
satellite downlink, synchronously detects its 2400 Hz AM subcarrier, resamples
to the standard 4160 words/sec APT word rate, and correlates the Sync-A
pattern to lock onto each 0.5 s scan line, accumulating a two-channel
grayscale image. Read it from [NOAA APT image
export](#noaa-apt-image-export). A satellite pass typically lasts 10–15
minutes several times a day (times/frequencies are predictable from a NORAD
TLE and your location, e.g. via `gpredict`) — outside of one, this mode
correctly reports a low `sync_quality` and an unchanging or noisy image, not
an error. `mode_params`: `deviation_hz` (peak carrier deviation, default
17000), `channel_bw_hz` (channel filter width, default 40000), `max_lines`
(how many recent scan lines the server keeps before dropping the oldest,
default 1200 ≈ 10 minutes).

`adsb` is a **non-audio** mode: it tunes 1090 MHz at 2 Msps, demodulates Mode S
Extended Squitter, and folds decoded frames into an aircraft track table. It
produces no Opus stream — read the tracks from [ADS-B track
export](#adsb-track-export) (or the Beast TCP feed). Its `mode_params`:

| param | default | meaning |
|-------|--------:|---------|
| `reference_lat`, `reference_lon` | `0`, `0` | receiver position for local CPR + range/bearing. `0,0` = unset (global CPR only, no range gate) |
| `max_range_nm` | `250` | drop positions farther than this from the reference |
| `trail_seconds` | `120` | position-trail length kept per aircraft |
| `forget_seconds` | `60` | drop an aircraft after this long with no message |
| `fix_errors` | `1` | attempt single-bit CRC correction on DF17/18 frames |

`ais` is likewise **non-audio**: it tunes 162.000 MHz at 2 Msps and runs two
9600-baud GMSK channel decoders (161.975 / 162.025 MHz) → HDLC → ITU-R M.1371
message decode → a per-MMSI vessel table. Read it from [AIS vessel-track
export](#ais-vessel-track-export) or the AIVDM TCP feed. `mode_params`:
`reference_lat`/`reference_lon` (receiver position, `0,0` = unset), `max_range_nm`
(default 60), `trail_seconds` (default 600), `forget_seconds` (default 900).

`aprs` is also **non-audio**: it defaults to 144.390 MHz (the North American
APRS channel) as NBFM but is **tunable** (`frequency_hz`, hot-retune, so the
same 1200-baud Bell 202 AFSK / AX.25 chain — RX **and** TX — can run on e.g.
the 33 cm band). Demod: AFSK → NRZI/HDLC deframer → AX.25 UI-frame decode →
APRS payload parse (uncompressed and compressed position, MIC-E, status,
message), folded into a per-callsign station table. Read it from [APRS
station-track export](#aprs-station-track-export) or the TNC2 TCP feed.
`mode_params`: `reference_lat`/`reference_lon` (receiver position, `0,0` =
unset), `max_range_km` (default 300), `trail_seconds` (default 1800),
`forget_seconds` (default 3600), `tx_gain_db` (default 30), `tx_deviation_hz`
(default 3000).

---

## ADS-B track export

Available whenever the server is built with SDR support; populated only while
the `adsb` mode pipeline runs (otherwise the tables are empty). Unauthenticated,
read-only.

### `GET /api/v1/adsb/aircraft`

```json
{
  "time": "2026-09-04T12:09:41Z",
  "mode": "adsb",
  "running": true,
  "receiver": [32.8986, -80.0405],
  "messages": 39421,
  "message_rate": 8.4,
  "aircraft_count": 7,
  "with_position": 7,
  "aircraft": [
    {
      "icao": "ad6d03",
      "callsign": "SWA3451",
      "category": "A3",
      "altitude_ft": 36875,
      "lat": 32.771, "lon": -80.05,
      "ground_speed_kt": 459.0,
      "track_deg": 205.0,
      "vertical_rate_fpm": -64,
      "rssi_dbfs": -3.1,
      "messages": 214,
      "age_s": 0.4,
      "pos_age_s": 0.9,
      "distance_nm": 8.1,
      "bearing_deg": 188.0,
      "trail": [[32.79, -80.04], [32.78, -80.045]]
    }
  ]
}
```

`aircraft` is sorted by `distance_nm` (positioned aircraft first, then by
recency). Any field except `icao`, `messages`, `age_s` and `trail` may be
`null` until the relevant message type is heard. `receiver` is `null` when no
reference position was configured.

### `GET /api/v1/adsb/messages`

A ring buffer of the most recent raw frames (hex of the 7- or 14-byte Mode S
frame), newest last:

```json
{ "time": "…", "count": 512, "messages": ["8dad6d039914c3b4184060783d2b", "…"] }
```

### `GET /api/v1/adsb/flight/{icao}`

Internet enrichment for one contact via **adsbdb.com** — tail number, type,
owner and (with `?callsign=SWA3451`) the origin → destination route.
Present only when the `flight-lookup` capability is advertised (on by
default; `--flight-lookup false` / `LANLINE_FLIGHT_LOOKUP=0` disables it and
drops the capability). `{icao}` is the 6-hex Mode S address. Results are
cached server-side (24 h for a full hit, 1 h otherwise). Always `200` — an
unavailable result carries a `reason`:

```json
{
  "available": true,
  "registration": "N628TS",
  "aircraft_type": "G650 ER",
  "icao_type": "G650",
  "manufacturer": "Gulfstream Aerospace",
  "owner": "Falcon Landing LLC",
  "owner_country": "United States",
  "photo_thumb_url": "https://airport-data.com/…/thumb.jpg",
  "airline": "United Airlines",
  "route": {
    "origin":      { "name": "…", "city": "San Francisco", "iata": "SFO", "icao": "KSFO" },
    "destination": { "name": "…", "city": "Singapore",     "iata": "SIN", "icao": "WSSS" }
  }
}
```

```json
{ "available": false, "reason": "unknown" }
```

`reason` is `disabled` (feature off), `offline` (no uplink), `unknown` (no
match), or `pending` (another client is fetching the same aircraft — retry).
Every other field is optional. **Privacy:** this is the only endpoint that
reaches the internet, and it tells adsbdb which aircraft this receiver sees.

### Beast feed (TCP `ports.beast`, default `30005`)

A raw **Beast binary** stream of every CRC-valid frame:
`0x1a <type> <6-byte 12 MHz timestamp> <1-byte signal> <frame>`, with `0x1a`
bytes in the payload doubled. `type` is `0x32` (7-byte) or `0x33` (14-byte).
Point `readsb` / `tar1090` / Virtual Radar Server at it. Disable with
`--beast-port 0`.

---

## AIS vessel-track export

Populated only while the `ais` mode pipeline runs. Unauthenticated, read-only.

### `GET /api/v1/ais/vessels`

```json
{
  "time": "2026-09-04T12:52:10Z",
  "mode": "ais",
  "running": true,
  "receiver": [32.8986, -80.0405],
  "messages": 486,
  "message_rate": 3.1,
  "vessel_count": 9,
  "with_position": 7,
  "vessels": [
    {
      "mmsi": 366999712,
      "name": "CHARLESTON PILOT",
      "callsign": null,
      "ship_type": 50, "ship_type_label": "Pilot",
      "imo": null,
      "nav_status": 0, "nav_status_label": "under way (engine)",
      "class_b": false, "aid": false,
      "lat": 32.751, "lon": -79.905,
      "sog_kt": 11.4, "cog_deg": 118.0, "heading_deg": 120.0,
      "length_m": 27, "beam_m": 7, "draught_m": 2.8,
      "destination": "CHARLESTON",
      "rssi_dbfs": -18.1,
      "messages": 42,
      "age_s": 1.2, "pos_age_s": 1.2,
      "distance_nm": 8.4, "bearing_deg": 121.0,
      "trail": [[32.76, -79.92], [32.755, -79.91]]
    }
  ]
}
```

`vessels` is sorted by `distance_nm` (positioned first, then by recency).
Position comes from message types 1/2/3 (Class A), 18/19 (Class B), 4 (base
station) and 21 (aids to navigation); name / callsign / dimensions / draught /
destination come from types 5 and 24 and are merged in as they arrive.

### `GET /api/v1/ais/messages`

Ring buffer of recent re-armored `!AIVDM` sentences (checksummed, newest last):

```json
{ "time": "…", "count": 512, "sentences": ["!AIVDM,1,1,,A,15N...,0*4B", "…"] }
```

### AIVDM feed (TCP `ports.ais_nmea`, default `10110`)

A line stream of the same `!AIVDM` sentences (CRLF-terminated). Point OpenCPN,
AIS-catcher, or `aisdispatcher` at it. Disable with `--ais-nmea-port 0`.

---

## APRS station-track export

Populated only while the `aprs` mode pipeline runs. Unauthenticated, read-only.

### `GET /api/v1/aprs/stations`

```json
{
  "time": "2026-09-06T18:20:11Z",
  "mode": "aprs",
  "running": true,
  "receiver": [29.65, -82.33],
  "packets": 1284,
  "packet_rate": 0.7,
  "station_count": 12,
  "with_position": 10,
  "stations": [
    {
      "call": "KZ4AZ-9",
      "lat": 29.6421, "lon": -82.3512,
      "course_deg": 118.0, "speed_kt": 34.0, "altitude_ft": 154.0,
      "symbol": "/>",
      "comment": "mobile",
      "last_message": null,
      "path": ["WIDE1-1", "WIDE2-1"],
      "rssi_dbfs": -12.4,
      "packets": 37,
      "age_s": 4.1, "pos_age_s": 4.1,
      "distance_km": 2.3, "bearing_deg": 201.0,
      "trail": [[29.641, -82.35], [29.6421, -82.3512]]
    }
  ]
}
```

`packets` is the total AX.25 UI frames folded in since the pipeline started;
`packet_rate` is a 30-second average (frames/s). `stations` is sorted by
`distance_km` (positioned stations first, then by recency). `symbol` is the
2-char APRS symbol (table char + code char) or `null`; `lat`/`lon`,
`course_deg`, `speed_kt`, `altitude_ft`, `last_message` and the range/bearing
pair are `null` until a packet carries them. `receiver` is `null` when no
reference position was configured (no range gate then).

### `GET /api/v1/aprs/packets`

Ring buffer of recent decoded frames as TNC2 monitor lines
(`SRC>DEST,PATH:info`, no CRLF), newest last:

```json
{ "time": "…", "count": 512, "packets": ["KZ4AZ-9>APRS,WIDE1-1:>mobile", "…"] }
```

### `POST /api/v1/aprs/tx`

Transmit one APRS packet — a **half-duplex burst** (RX drops for ~1 s). Needs
`--enable-tx` on the server, `aprs` mode, a tx-capable device, and the radio
running. `aprs` is now `tx_capable` with two extra mode params: `tx_gain_db`
(default 30) and `tx_deviation_hz` (default 3000).

Session-authenticated. Body: `source` (required callsign). Optional `dest`
(default `APZLNL` — the APRS experimental tocall space), `path` (digipeater
aliases, default direct), `gain_db`, `deviation_hz`. Exactly one payload form:

| Form | Fields | Builds |
|------|--------|--------|
| Raw | `info` | the info field verbatim |
| Message | `message_to`, `message_text` | `:<addressee padded to 9>:text` (unnumbered) |
| Position | `lat`, `lon`, `symbol`?, `comment`? | `!DDMM.hhN/DDDMM.hhW>comment` |

```json
// request
{ "source": "KZ4AZ-1", "message_to": "KZ4AZ-2", "message_text": "hi" }
// response
{ "transmitted": true, "tnc2": "KZ4AZ-1>APZLNL::KZ4AZ-2  :hi", "bytes": 30 }
```

The sender echoes its own packet into its `GET /aprs/packets` ring, tracker
and TNC2 feed (like a real TNC), and appends a `mode: "aprs"` entry (with the
TNC2 line in `detail`) to [`GET /api/v1/radio/tx/log`](#radio-configuration--control).
`403` when `--enable-tx` is off, `409` when the mode isn't `aprs` or the radio
isn't running.

### TNC2 feed (TCP `ports.aprs`, default `10152`)

A line stream of the same TNC2 monitor lines (CRLF-terminated). Point Xastir,
YAAC, or an APRS-IS gateway at it. Disable with `--aprs-port 0`.

---

## NOAA APT image export

Populated only while the `apt` mode pipeline runs; a `Stop radio` + `Start
radio` cycle starts a fresh image. Unauthenticated, read-only.

Unlike AM/FM/ADS-B/AIS, NOAA APT is a real-time downlink from one specific
137 MHz weather satellite passing overhead — there is no ambient signal to
receive between passes, so both endpoints below will legitimately report a
low `sync_quality` (or an empty image) most of the time. That is the decoder
correctly declining to claim a lock on noise, not a bug.

### `GET /api/v1/apt/status`

```json
{ "width": 1818, "height": 214, "lines": 214, "sync_quality": 8.4 }
```

`sync_quality` is a matched-filter SNR on the Sync-A line-start correlation
(peak coherent correlation over the local noise floor): real signal locks
around 6–10+, noise stays under ~3–4. `width`/`height` describe the current
raster (`width` is fixed at 1818 = two 909-pixel channel images side by
side; `height` grows one row per decoded scan line, up to `max_lines`).

### `GET /api/v1/apt/image`

Binary, not JSON (`content-type: application/octet-stream`):

```text
[u32 width LE][u32 height LE][row-major grayscale bytes, width*height]
```

### Predicting the next pass

There's no dedicated "next pass" endpoint — the web client predicts passes
entirely on its own, locally, from `GET /apt-tle.json` (the bundled NOAA-15/
18/19 orbital elements — see [above](#web-client)) via SGP4 propagation
(`satellite.js`). No live tracking/pass-prediction service is called at
runtime; regenerate the bundled TLE periodically with
`node scripts/fetch-apt-tle.mjs` (it goes stale in 1–2 weeks, unlike the
station directories).

Channel A (visible/IR depending on the satellite and time of day) occupies
columns `0..909`, channel B `909..1818`. No PNG/image crate involved on
either end — the web client reads this straight into a canvas `ImageData`.

---

## Presets

Named radio configurations. Phase 1 ships read-only built-ins.

### `GET /api/v1/presets`

```json
[
  {
    "id": "noaa-khb29",
    "name": "NOAA Weather Radio — KHB29 (162.550 MHz)",
    "builtin": true,
    "config": {
      "mode": "nbfm",
      "frequency_hz": 162550000,
      "tuner": { "sample_rate_hz": 2000000, "gain_elements_db": { "AMP": 0, "LNA": 32, "VGA": 30 } },
      "mode_params": { "deviation_hz": 5000, "channel_bw_hz": 16000, "deemphasis_us": 75, "audio_lpf_hz": 3400, "squelch_db": -80 }
    }
  }
]
```

### `POST /api/v1/presets/{id}/apply`

Deep-merges the preset's `config` onto the live [`radio`](#radio-configuration--control)
config (same validation and hot-apply rules as `PATCH /api/v1/radio`) and
returns the new full `radio` state. `404` for an unknown id.

---

## Radio configuration & control

One resource describes the whole receive chain: tuner (device-facing) +
mode + mode params + audio encoder.

### `GET /api/v1/radio`

```json
{
  "enabled": true,
  "running": false,
  "mode": "nbfm",
  "frequency_hz": 162550000,
  "tuner": {
    "device_id": "hackrf/0000000000000000457863c8...",
    "channel": 0,
    "antenna": "TX/RX",
    "sample_rate_hz": 2000000,
    "bandwidth_hz": null,
    "lo_offset_hz": 0,
    "gain_mode": "manual",
    "gain_db": null,
    "gain_elements_db": { "AMP": 0, "LNA": 32, "VGA": 30 },
    "freq_correction_ppm": 0,
    "dc_offset_correction": true,
    "iq_balance_correction": true,
    "device_settings": { "bias_tx": "false" }
  },
  "mode_params": {
    "deviation_hz": 5000,
    "channel_bw_hz": 16000,
    "deemphasis_us": 75,
    "audio_lpf_hz": 3400,
    "squelch_db": -80
  },
  "audio": {
    "sample_rate_hz": 48000,
    "channels": 1,
    "opus_bitrate_bps": 24000,
    "frame_ms": 20
  }
}
```

**tuner fields**

| Field | Meaning |
|-------|---------|
| `device_id` | Read-only echo of the selected device; change it via `PUT /api/v1/device` |
| `channel` | RX channel index |
| `antenna` | One of `device.rx.antennas` |
| `sample_rate_hz` | Must fall in a `device.rx.sample_rate_ranges_hz` range; the DSP chain decimates + rationally resamples from here to `audio.sample_rate_hz` |
| `bandwidth_hz` | Analog filter bandwidth; `null` = driver default |
| `lo_offset_hz` | Digital offset tuning: the LO is placed `lo_offset_hz` away from `frequency_hz` and mixed back in software to dodge the DC spike. Works on any device. `0` disables |
| `gain_mode` | `manual` or `agc` (only if `device.rx.has_agc`) |
| `gain_db` | Overall gain (SoapySDR `setGain`); when non-null it wins over `gain_elements_db` |
| `gain_elements_db` | Per-element gains, keys must be a subset of `device.rx.gain_elements[].name` |
| `freq_correction_ppm` | This device's crystal error, applied in software to every tuned frequency (`freq * (1 + ppm/1e6)`) on every mode — works on any device, doesn't depend on `device.rx.has_frequency_correction` (which HackRF, for one, doesn't advertise). `0` (default) applies no correction; also settable at startup via `--freq-correction-ppm`. See [architecture.md](architecture.md#hackrf-frequency-calibration) for how to determine a device's value |
| `dc_offset_correction` / `iq_balance_correction` | Toggle SoapySDR automatic correction (only if the matching `has_*_mode`) |
| `device_settings` | Raw SoapySDR `writeSetting` passthrough; keys must be a subset of `device.rx.setting_info[].key`, values are strings |

### `PATCH /api/v1/radio`

Partial, deep-merge update of any subset of the `GET` body (except read-only
`running` and `tuner.device_id`). Semantics:

- Every value is validated against the **selected device's** reported ranges,
  gain-element names, setting keys, and the active mode's parameter schema.
  Violations → `422 invalid_parameter` with the offending `field` and the
  allowed values in `details`. Nothing is applied if any field is invalid.
- Valid changes are **hot-applied** to a running pipeline without an audio
  gap where possible: `frequency_hz`, the gains, and the NBFM/AM filter
  `mode_params` (deviation, bandwidth, de-emphasis, audio LPF, squelch) go
  live. Changing `mode`, `sample_rate_hz`, `channel`, `antenna`,
  `lo_offset_hz`, `bandwidth_hz`, `freq_correction_ppm`,
  `dc_offset_correction`, `iq_balance_correction`, `device_settings`,
  `frame_ms` or `opus_bitrate_bps` bounces the DSP chain (brief audio gap;
  the WebRTC session is preserved). The web client's **Radio options** panel
  (⚙ in "Now playing") is a form over this: it reads
  `GET /api/v1/device`'s ranges and PATCHes only the controls you touch.
- Switching `mode` replaces `mode_params` with that mode's defaults unless the
  request also supplies `mode_params`.
- `503 device_unavailable` if the change needs the SDR and none is ready
  (except when `mode` is `debug_tone`).

Returns the new full `radio` state.

### `POST /api/v1/radio/start`

Start the receive pipeline (open stream, spin up DSP + Opus encoder). Idempotent
— returns `radio` state with `running: true`. `503 device_unavailable` unless
`mode` is `debug_tone`.

### `POST /api/v1/radio/stop`

Stop the pipeline. Idempotent. Active WebRTC sessions stay up but receive
silence.

### `GET /api/v1/radio/status`

Live telemetry, safe to poll at ~1 Hz.

```json
{
  "running": true,
  "mode": "nbfm",
  "frequency_hz": 162550000,
  "device_status": "ready",
  "dsp": {
    "rssi_dbfs": -41.2,
    "snr_db": 22.5,
    "squelch_open": true,
    "audio_level_dbfs": -18.0,
    "ctcss_tone_hz": null,
    "ctcss_scan_hz": null,
    "dcs_code": null,
    "sample_overruns": 0,
    "pipeline_latency_ms": 62,
    "tx_keyed": false
  },
  "audio": {
    "encoder": "opus",
    "bitrate_bps": 24000,
    "frames_sent": 54120,
    "sample_rate_hz": 48000,
    "channels": 1
  },
  "recording": { "active": false, "last": null },
  "scan": { "active": false, "parked": false, "frequency_hz": null, "label": null, "index": 0, "total": 0 },
  "clients": 1,
  "time": "2026-09-03T17:07:52Z"
}
```

`recording` reflects the server-side demod-audio recorder (see
`POST /radio/record` below): `{active, last}` where `last` is the most
recent finished recording (`filename`, `bytes`, `secs`, …) or `null`.

`scan` reflects a server-side channel scan (`POST /radio/scan`): `{active,
parked, frequency_hz, label, index, total}`. While `active`, `frequency_hz`
is the **live** tuned frequency (the top-level `frequency_hz` reflects the
stored config and is stale during a scan); `parked` is true when the scan
has stopped on a live channel and is passing its audio through.

In `debug_tone` mode the `dsp` block reports synthetic values
(`rssi_dbfs: null`, `squelch_open: true`). `ctcss_tone_hz` is the configured
CTCSS tone while it is currently detected on-channel (FM modes with
`ctcss_hz` set), else `null`. `ctcss_scan_hz` is the strongest *standard*
CTCSS tone the receiver sees regardless of what's configured — an always-on
Goertzel bank on narrowband FM, for identifying an unknown repeater's tone —
or `null` when none stands out. `dcs_code` is the configured DCS code while
it is currently decoded on-channel (FM modes with `dcs_code` set), else
`null`.

### `POST /api/v1/radio/record`

Start or stop a server-side recording of the **demodulated audio** — 48 kHz
mono 16-bit WAV, written next to the IQ recordings (`--iq-dir`, default cwd).
Auth required. Body: `{"action": "start" | "stop", "max_secs": 300}`
(`max_secs` 1–3600, default 300). Available in the audio modes
(`nbfm`/`wbfm`/`am`/`frs`/`ham`/`debug_tone`) while the radio is running —
`analysis` has its own IQ recorder instead (`409 conflict` otherwise, or if
one is already recording).

The recording auto-stops at `max_secs`, and on any pipeline bounce (mode
switch, sample-rate change, `stop`) — a finished file is always left, and
its metadata shows in `RadioStatus.recording.last`. `start` returns
`{active, filename, path, sample_rate_hz, max_secs}`; `stop` returns
`{active: false, last}`.

### `GET /api/v1/radio/recording`

Download the most recent completed audio recording as a `audio/wav`
attachment. `404` if there is none, `409` while one is still recording.
Unauthenticated (same posture as `/radio/status`).

### `POST /api/v1/radio/scan`

Start or stop a **server-side channel scan** on the running SDR pipeline —
the pipeline thread sweeps a channel list, dwelling briefly on each, and
**parks** (stops advancing, passes audio through) on any channel whose
squelch opens above an RSSI gate, resuming a set time after the signal
drops. Auth required; `nbfm`/`wbfm`/`am`/`frs`/`ham` while running.

Body: `{"action": "start" | "stop", …}`. For `start`, give the channels as
an explicit list, a range shorthand, or both:

```json
{
  "action": "start",
  "channels": [
    { "frequency_hz": 462562500, "label": "Ch 1" },
    { "frequency_hz": 462587500, "label": "Ch 2" }
  ],
  "range": { "lo_hz": 462550000, "hi_hz": 462725000, "step_hz": 12500 },
  "dwell_ms": 150,
  "hang_ms": 2500,
  "rssi_gate_dbfs": -75
}
```

`dwell_ms` (30–5000, default 150) is how long to settle on a channel before
deciding; `hang_ms` (0–30000, default 2500) is how long to stay parked after
the signal drops; `rssi_gate_dbfs` (default −75) is the floor a channel must
clear to park. At most 4000 channels. A manual `PATCH /radio` frequency
change cancels the scan. `start` returns `{scanning: true, channels: N}`;
`stop` leaves RX wherever the scan currently sits. Progress + the parked
channel are in `RadioStatus.scan`.

### `POST /api/v1/radio/tx/key`

Push-to-talk: begin transmitting live mic audio streamed up over the
session's WebRTC connection (silence if none has arrived yet). Body
optional:

- `{"gain_db": 10}` overrides the default of `0` (HackRF's TX chain runs
  0–61 dB across its VGA + AMP elements; deliberately conservative — this is
  real RF, not a simulation).
- `{"offset_hz": -600000, "tone_hz": 100.0}` (`ham` mode only) key a repeater
  through its input: `offset_hz` (±30 MHz) shifts the transmit frequency from
  the current RX frequency (a plain LO retune, restored on unkey), and
  `tone_hz` (a standard 67.0–254.1 Hz CTCSS value, else ignored) is encoded
  onto the transmission as the uplink tone. `{"dcs_code": 23, "dcs_invert":
  false}` encodes a DCS uplink instead (ignored when `tone_hz` is set). All
  default to `0`/`false` (simplex/none) and are ignored for `frs`.

Audio conditioning comes from the current mode's `mode_params`, read at
key-up: `deviation_hz` sets the peak FM deviation, and `tx_mic_gain` (a
linear multiplier, default `2.5`) boosts the decoded mic PCM before
modulation — getUserMedia audio, especially from an Android WebView, arrives
well below full scale, and without the lift the far radio is barely audible.
The gained signal is hard-limited to full scale, so peak deviation stays
capped at `deviation_hz` however hot `tx_mic_gain` is set. Change either with
`PATCH /api/v1/radio` and it takes effect on the next key-up.

Requires, in order: `--enable-tx` on the server (else `403 forbidden`
regardless of anything else below), `mode: "frs"` or `"ham"` (else `400
bad_request`), the radio already running (else `409 conflict`), and a
tx-capable device (else `503 device_unavailable`).

Response:
```json
{ "keyed": true, "gain_db": 10, "mic_gain": 2.5, "offset_hz": -600000, "tone_hz": 100.0, "dcs_code": 0, "dcs_invert": false }
```

Auto-unkeys after a hard 10s server-side cap regardless of whether
`/radio/tx/unkey` is ever called — a lost `unkey` request (dropped
connection, crashed client) can't leave the transmitter keyed indefinitely.

### `POST /api/v1/radio/tx/unkey`

Ends the current transmission early. A no-op (not an error) if nothing is
currently keyed — always safe to call on PTT release.

```json
{ "keyed": false }
```

### `GET /api/v1/radio/tx/log`

The transmit audit log — a station-log-adjacent record of every push-to-talk
key this server has served, kept in memory (bounded ring, ~200 entries),
newest first. Auth required.

```json
[
  {
    "keyed_at": "2026-09-06T00:31:12Z",
    "client": "laptop",
    "mode": "ham",
    "tx_frequency_hz": 146340000,
    "offset_hz": -600000,
    "tone_hz": 100.0,
    "gain_db": 6,
    "released_at": "2026-09-06T00:31:19Z",
    "duration_ms": 6700
  }
]
```

`released_at`/`duration_ms` are `null` when no explicit `unkey` was recorded
for that key — the transmission was auto-released at the server's 10 s cap,
or the client vanished. The log is not persisted across a server restart. An
APRS or spoken (`say`) burst appears here with `duration_ms: 0` and its text
in a `detail` field.

### `POST /api/v1/radio/tx/say`

**Text-to-speech transmit** — synthesize `text` and key it on the air.
Feature `tts` + `--tts`; `--enable-tx`; a voice mode (`nbfm`/`wbfm`/`am`/
`frs`/`ham`); the radio running; a tx-capable device. Auth required.

```jsonc
// request
{ "text": "radio check from lan line", "gain_db": 0 }
// response
{ "transmitted": true, "secs": 2.1, "text": "radio check from lan line",
  "backend": "espeak-ng" }
```

The synth backend is `espeak-ng` by default, or `piper` when the server was
started with `--tts-voice <model.onnx>`. A spoken message can run up to 30 s
(a normal PTT over is capped at 10). `403` without `--enable-tx` or `--tts`,
`409` off a voice mode or when the radio is stopped.

### `GET /api/v1/radio/transcript`

Recognized text from received transmissions (feature `stt` + `--stt`), newest
last. Unauthenticated, read-only. Each over is transcribed on a worker thread
when its squelch closes.

```json
{
  "time": "2026-09-06T22:40:11Z",
  "count": 3,
  "segments": [
    { "time": "…", "text": "meet at the north gate at fifteen hundred",
      "rssi_dbfs": -34.1, "secs": 4.2 }
  ]
}
```

`DspStatus.transcribing` (in `/radio/status`) is `true` while a segment is
being decoded. STT (`stt`) and TTS (`tts`) also appear in
`GET /api/v1/server`'s `capabilities`.

---

## Sessions

A session is required for audio and for any state-changing call. It carries the
bearer token and owns at most one WebRTC peer connection.

### `POST /api/v1/sessions`

Unauthenticated.

Request:
```json
{
  "client": {
    "name": "web",
    "user_agent": "Mozilla/5.0 ...",
    "capabilities": ["webrtc-recv"]
  }
}
```

Response `201`:
```json
{
  "session_id": "9b1f...c2",
  "token": "s_3f9a1c8e7b6d5a4f...",
  "heartbeat_interval_s": 15,
  "expires_in_s": 45
}
```

`429 too_many_sessions` if the configured cap is reached.

### `GET /api/v1/sessions`

List active sessions (observability). Tokens are never returned here.

```json
[
  {
    "session_id": "9b1f...c2",
    "client": { "name": "web", "user_agent": "Mozilla/5.0 ...", "capabilities": ["webrtc-recv"] },
    "created": "2026-09-03T17:04:11Z",
    "expires": "2026-09-03T17:04:56Z",
    "audio": { "state": "connected" }
  }
]
```

### `GET /api/v1/sessions/{id}`

Single session, same shape as a list entry. `404` if unknown/expired. A caller
may only read its own session.

### `POST /api/v1/sessions/{id}/heartbeat`

Extend the TTL. Response:
```json
{ "expires_in_s": 45 }
```
Missing three intervals → the session is reaped and its WebRTC peer closed.

### `DELETE /api/v1/sessions/{id}`

End the session now; closes the peer connection. `204 No Content`.

---

## Audio — WebRTC signaling

The browser is the **offerer** and receives audio; the server answers with a
single send-only Opus track (48 kHz, mono) fed from the shared demodulated
stream. Phase 1 uses **non-trickle** ICE: the server returns its answer only
after ICE gathering completes, with a single host candidate on the advertised
`audio_out` UDP port. No STUN/TURN — same-LAN only.

```
client                                   server
  |  create RTCPeerConnection                |
  |  addTransceiver('audio', recvonly)       |
  |  createOffer / setLocalDescription       |
  |---- POST /sessions/{id}/audio/offer ---->|  new RTCPeerConnection
  |          { sdp, type: "offer" }          |  add Opus track (sendonly)
  |                                          |  setRemote(offer)
  |                                          |  createAnswer / setLocal
  |                                          |  await ICE gathering complete
  |<--------------- 200 ---------------------|
  |          { sdp, type: "answer" }         |
  |  setRemoteDescription(answer)            |
  |  ...DTLS + SRTP, Opus flows...           |
```

### `POST /api/v1/sessions/{id}/audio/offer`

Request:
```json
{ "sdp": "v=0\r\no=- ... a=recvonly\r\n", "type": "offer" }
```
Response `200`:
```json
{ "sdp": "v=0\r\no=- ... a=sendonly\r\n", "type": "answer" }
```
`409 conflict` if the session already has an open peer connection (call
`DELETE .../audio` first). Re-offer for renegotiation is allowed once
`GET .../audio` reports `state: "connected"`.

### `POST /api/v1/sessions/{id}/audio/ice`

Trickle-ICE candidate sink. Phase 1 accepts and ignores (returns `202`); kept
so the client code path exists for later.
```json
{ "candidate": { "candidate": "candidate:...", "sdpMid": "0", "sdpMLineIndex": 0 } }
```

### `GET /api/v1/sessions/{id}/audio`

```json
{
  "state": "connected",
  "ice_state": "completed",
  "dtls_state": "connected",
  "packets_sent": 54120,
  "bytes_sent": 3245210
}
```
`state`: `idle` · `negotiating` · `connected` · `failed` · `closed`.

### `DELETE /api/v1/sessions/{id}/audio`

Close the peer connection, keep the session. `204 No Content`.

---

## Receiver Analysis

Populated only while the `analysis` mode pipeline runs. Read endpoints are
unauthenticated (same posture as `/radio/status`).

### `GET /api/v1/analysis/spectrum?since=<seq>&max_rows=<n>`

Binary FFT frame, `application/octet-stream`, little-endian:

```text
"LWF1"       magic (4 bytes)
seq          u64   total waterfall rows produced since pipeline start
center_hz    f64
span_hz      f64   = the sample rate
n_bins       u32   = fft_size
db_lo, db_hi f32   the dB window the u8 rows are quantised over (−150, +10)
n_new_rows   u32
avg_db       f32 × n_bins   EMA-averaged spectrum (the panadapter trace), DC-centred
rows         u8  × n_bins × n_new_rows   instantaneous rows since `since`, oldest first
```

Poll with `since` = the previous frame's `seq`. `max_rows` (default 256)
bounds catch-up after a stall. The client maps `u8` → colour over its own
visible floor/ceiling, so changing the dynamic range needs no round-trip.

### `GET /api/v1/analysis/status`

```json
{ "running": true, "center_hz": 100000000, "span_hz": 2000000, "n_bins": 4096,
  "seq": 5123, "rows_held": 2000, "recording": false, "last_recording": null }
```

### `POST /api/v1/analysis/record`

Auth. `{ "action": "start", "max_secs": 60 }` begins an IQ recording — a
2-channel 16-bit PCM WAV (interleaved I, Q at `tuner.sample_rate_hz`), the
format GQRX / SDR# / SDRuno read. `max_secs` is clamped 1–600 (default 60);
recording auto-stops at the cap. `{ "action": "stop" }` closes it and returns
`last_recording` metadata. `409` if analysis isn't running, or already
recording. Files land in `--iq-dir` (default: the server's working
directory); at 2 Msps that's ~8 MB/s.

### `GET /api/v1/analysis/recording`

Streams the most recently completed recording as a `.wav` attachment. `404`
if there is none, `409` while one is still in progress.

---

## Reserved for phase 2

Push-to-talk transmit shipped (`frs` mode only) — see
[`POST /api/v1/radio/tx/key`](#post-apiv1radiotxkey) above and
[architecture.md](architecture.md#frs-and-the-transmit-question) — in a
simpler shape than originally sketched here: no separate transmit-config
resource (TX gain is passed directly to `/radio/tx/key`; deviation and mic
gain are `frs` `mode_params`, live-editable via `PATCH /api/v1/radio` and
read at key-up; frequency is shared with the same mode's receive config) and
no separate inbound-stream endpoint (mic audio rides the *same* session
`POST /sessions/{id}/audio/offer` connection bidirectionally — see
[Audio — WebRTC signaling](#audio--webrtc-signaling) — rather than a second
one on the reserved `audio_in` port, which remains genuinely unused).

Still open: transmit isn't available in any mode besides `frs`, and there's
no per-session control over who's allowed to key up (any authenticated
session can call `/radio/tx/key`) — fine for this project's single-operator,
personal-use scope, but worth flagging if that ever changes.

---

## Data types

### `Range`

```json
{ "min": 0, "max": 40, "step": 8 }
```
`step` of `0` means continuous. Ranges appear in arrays because SoapySDR can
report several disjoint ranges for one parameter.

### `SettingInfo`

```json
{
  "key": "direct_samp",
  "name": "Direct Sampling",
  "type": "string",
  "default": "0",
  "description": "Enable I or Q direct sampling for HF",
  "options": ["0", "1", "2"]
}
```
`type`: `bool` · `int` · `float` · `string`. `options` is `null` unless the
driver constrains the value to an enumerated set.

### `ModeParamSpec`

```json
{ "type": "number", "default": 5000, "min": 1000, "max": 15000, "unit": "Hz" }
```
`enum` (array of allowed numbers) may appear instead of `min`/`max`.

### Session `audio.state` / WebRTC `state`

`idle` — no peer connection ·
`negotiating` — offer received, answer pending or ICE in progress ·
`connected` — SRTP flowing ·
`failed` — ICE/DTLS failure ·
`closed` — torn down.
