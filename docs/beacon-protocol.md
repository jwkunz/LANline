# Discovery beacon protocol

The server announces itself on the LAN with a small UDP broadcast so native
clients (Android, CLI) can find it without configuration. The **web client does
not use the beacon** — browsers cannot receive UDP broadcast. A browser instead
reaches the server either by being served from it directly (same origin, nothing
to type) or by name via **mDNS** — the server also publishes an `_lanline._tcp`
service and an A record for `<mdns-name>.local` (default `lanline.local`) on the
C2 port. As a last resort the host can be typed and is saved in `localStorage`.

## Transport

| | |
|---|---|
| Protocol | UDP, IPv4 |
| Destination port | **50055** (fixed, not one of the advertised random ports) |
| Destination address | `255.255.255.255` **and** the directed broadcast address of each non-loopback IPv4 interface (e.g. `192.168.1.255`) |
| Source port | ephemeral |
| Cadence | every **1000 ms**, plus an immediate burst of 3 (200 ms apart) whenever the selected device changes or the server starts |
| Socket options | `SO_BROADCAST`, `SO_REUSEADDR` |

A client discovers servers by binding UDP `0.0.0.0:50055` and reading
datagrams. Multiple servers on one LAN are distinguished by `server_id`.

## Payload

A single UTF-8 JSON object per datagram (no framing, fits comfortably in one
packet):

```json
{
  "magic": "LANLINE-BEACON",
  "protocol_version": 1,
  "server_id": "5f2b8b0e-2c1a-4d7e-9a3e-1b6d9c0a77aa",
  "version": "0.1.0",
  "hostname": "bench-linux",
  "advertised_host": "192.168.1.42",
  "ports": { "c2": 8730, "audio_out": 49213, "audio_in": 60731, "beast": 30005, "ais_nmea": 10110 },
  "scheme": "http",
  "c2_base_url": "http://192.168.1.42:8730",
  "device": {
    "driver": "hackrf",
    "label": "HackRF One",
    "serial": "0000000000000000457863c8...",
    "tx_capable": true
  },
  "devices_available": 1,
  "capabilities": ["rx", "webrtc", "nbfm", "wbfm", "am", "frs", "apt", "adsb", "ais", "debug_tone"],
  "timestamp": "2026-09-03T17:04:11Z"
}
```

| Field | Notes |
|-------|-------|
| `magic` | Always `"LANLINE-BEACON"`; receivers must drop datagrams that lack it |
| `protocol_version` | Bump on any breaking change to this payload or the REST contract |
| `server_id` | Stable for the lifetime of a server process (UUID v4). Under `lanline-hypervisor` it is pinned per radio, so it also survives a supervised restart |
| `instance_label` | Optional human name for the radio (`--instance-label`; the hypervisor sets it per child). `null` on a plain standalone server |
| `fleet_url` | URL of the hypervisor fleet page this radio belongs to (`--fleet-url`), or `null` for a standalone server |
| `advertised_host` | The IPv4 address the server believes clients should dial; per-interface datagrams carry that interface's address |
| `ports.c2` | TCP port of the REST API |
| `ports.audio_out` | UDP port for the receive WebRTC media (ICE host candidate) |
| `ports.audio_in` | UDP port reserved for the phase-2 transmit stream |
| `ports.beast` | TCP port of the Beast Mode S feed (ADS-B); `0` when disabled |
| `ports.ais_nmea` | TCP port of the AIVDM marine AIS feed; `0` when disabled |
| `scheme` | `"http"`, or `"https"` when the server runs with `--tls` (C2 port is then HTTPS-only) |
| `c2_base_url` | Convenience: `<scheme>://<advertised_host>:<c2>` |
| `device` | `null` when no SDR is selected; `devices_available` still reports how many were enumerated |
| `capabilities` | Mirrors `GET /api/v1/server.capabilities` |
| `timestamp` | RFC 3339 UTC send time |

## Fleet beacon (`LANLINE-FLEET-BEACON`)

[`lanline-hypervisor`](hypervisor.md) runs one `lanline-server` per radio. It
emits one ordinary `LANLINE-BEACON` per child (real ports, pinned `server_id`,
`instance_label`) **and**, on the same port `50055`, one extra datagram
describing the group:

```json
{
  "magic": "LANLINE-FLEET-BEACON",
  "protocol_version": 1,
  "fleet_id": "b1e0…",
  "hostname": "bench-linux",
  "advertised_host": "192.168.1.42",
  "fleet_base_url": "http://192.168.1.42:8720",
  "version": "2.0.0",
  "radios": [
    {
      "idx": 0, "label": "VHF",
      "server_id": "5f2b…", "scheme": "https",
      "c2_base_url": "https://192.168.1.42:8730",
      "device": "HackRF One", "device_serial": "0000…",
      "running": true, "restarts": 0, "pid": 40912, "started_at": 1757181234,
      "ports": { "c2": 8730, "audio_out": 8740, "audio_in": 8750, "beast": 30005, "ais_nmea": 10110, "aprs": 10152 }
    },
    { "idx": 1, "label": "APRS", "server_id": "77c9…", "scheme": "https",
      "c2_base_url": "https://192.168.1.42:8731", "device": "PlutoSDR",
      "running": true, "restarts": 0, "pid": 40913, "started_at": 1757181234,
      "ports": { "c2": 8731, "audio_out": 8741, "audio_in": 8751, "beast": 30006, "ais_nmea": 10111, "aprs": 10153 } }
  ],
  "timestamp": "2026-09-06T18:04:11Z"
}
```

The same `radios` array (always current, plus a `fleet_base_url`) is served at
`GET <fleet_base_url>/api/v1/fleet`. A client that only understands
`LANLINE-BEACON` ignores this datagram and still sees every child; a
fleet-aware client (the Android chooser) uses it to group the radios under
their host and offer a switch.

## Client guidance

1. Bind `0.0.0.0:50055` UDP, enable address reuse.
2. For each datagram: parse JSON, require `protocol_version == 1` and
   `magic == "LANLINE-BEACON"` (or `"LANLINE-FLEET-BEACON"` if you handle
   fleets; drop anything else).
3. Key servers by `server_id`; treat a server as gone after ~5 s (5 missed
   beacons) with no datagram.
4. Connect using `c2_base_url` (or `advertised_host` + `ports.c2`), then drive
   the [REST API](rest-api.md).

## Security note

The beacon is unauthenticated and world-readable on the segment. It carries no
secrets — only how to reach an API that itself requires a session token for
anything stateful. Do not route it off the local segment.
