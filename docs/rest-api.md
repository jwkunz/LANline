# SDR C2 REST API

Version: `v1` · Protocol version: `1`

The command-and-control port advertised by the [discovery beacon](beacon-protocol.md)
serves this API. All paths below are relative to
`http://<host>:<c2_port>`.

- Request and response bodies are `application/json; charset=utf-8`.
- Every path is under `/api/v1` except `GET /health`.
- Timestamps are RFC 3339 / ISO 8601 UTC strings (e.g. `2026-09-03T17:04:11Z`).
- Frequencies are integer **Hz**, gains are **dB** (float), durations are
  seconds unless a field name says `_ms`.

## Contents

- [Authentication](#authentication)
- [CORS](#cors)
- [Error model](#error-model)
- [Meta](#meta)
- [Devices](#devices)
- [Modes](#modes)
- [Presets](#presets)
- [Radio configuration & control](#radio-configuration--control)
- [Sessions](#sessions)
- [Audio — WebRTC signaling](#audio--webrtc-signaling)
- [Reserved for phase 2](#reserved-for-phase-2)
- [Data types](#data-types)

---

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

The web client is served from a different origin (the Vite dev server, or a
static host) than the API. The server sends permissive CORS headers for
`GET`/`HEAD` and echoes an allow-list of origins (configurable, default `*` in
dev) for the other methods, plus `Authorization` and `Content-Type` in
`Access-Control-Allow-Headers`. Pre-flight `OPTIONS` is handled for every route.

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
  "version": "0.1.0",
  "hostname": "bench-linux",
  "time": "2026-09-03T17:04:11Z",
  "ports": { "c2": 51847, "audio_out": 49213, "audio_in": 60731 },
  "capabilities": ["rx", "webrtc", "nbfm", "debug_tone"],
  "selected_device": {
    "id": "hackrf/0000000000000000457863c8...",
    "driver": "hackrf",
    "label": "HackRF One",
    "tx_capable": true
  }
}
```

`capabilities` is dynamic: `"tx"` appears only when the selected device
supports transmit. `selected_device` is `null` when none is selected.

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

`debug_tone` synthesizes audio internally and does **not** touch the SDR — it
works with no device selected or a device in `error`, and is the end-to-end
check for the WebRTC path.

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
| `freq_correction_ppm` | Frequency correction; requires `device.rx.has_frequency_correction` |
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
  gap where possible: `frequency_hz`, the gains, and the NBFM filter
  `mode_params` (deviation, bandwidth, de-emphasis, audio LPF, squelch) go
  live. Changing `mode`, `sample_rate_hz`, `channel`, `antenna`,
  `lo_offset_hz`, `frame_ms` or `opus_bitrate_bps` bounces the DSP chain
  (brief audio gap; the WebRTC session is preserved).
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
    "sample_overruns": 0,
    "pipeline_latency_ms": 62
  },
  "audio": {
    "encoder": "opus",
    "bitrate_bps": 24000,
    "frames_sent": 54120,
    "sample_rate_hz": 48000,
    "channels": 1
  },
  "clients": 1,
  "time": "2026-09-03T17:07:52Z"
}
```

In `debug_tone` mode the `dsp` block reports synthetic values
(`rssi_dbfs: null`, `squelch_open: true`).

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

## Reserved for phase 2

Defined now so clients and docs are stable; each returns
`501 not_implemented`.

| Endpoint | Purpose |
|----------|---------|
| `GET /api/v1/radio/tx` | Current transmit config |
| `PATCH /api/v1/radio/tx` | Configure transmit (mode, `frequency_hz`, `deviation_hz`, power) |
| `POST /api/v1/radio/tx/ptt` | `{ "key": true \| false }` — push-to-talk |
| `POST /api/v1/sessions/{id}/broadcast/offer` | WebRTC offer for an **inbound** Opus stream to be modulated and transmitted (uses the `audio_in` port) |
| `GET /api/v1/sessions/{id}/broadcast` | Inbound stream state |
| `DELETE /api/v1/sessions/{id}/broadcast` | Tear down the inbound stream |

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
