# Discovery beacon protocol

The server announces itself on the LAN with a small UDP broadcast so native
clients (Android, CLI) can find it without configuration. The **web client does
not use the beacon** — browsers cannot receive UDP broadcast — it takes a host
entered by the user and saved in `localStorage`.

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
  "magic": "SDR-C2-BEACON",
  "protocol_version": 1,
  "server_id": "5f2b8b0e-2c1a-4d7e-9a3e-1b6d9c0a77aa",
  "version": "0.1.0",
  "hostname": "bench-linux",
  "advertised_host": "192.168.1.42",
  "ports": { "c2": 51847, "audio_out": 49213, "audio_in": 60731 },
  "c2_base_url": "http://192.168.1.42:51847",
  "device": {
    "driver": "hackrf",
    "label": "HackRF One",
    "serial": "0000000000000000457863c8...",
    "tx_capable": true
  },
  "devices_available": 1,
  "capabilities": ["rx", "webrtc", "nbfm", "debug_tone"],
  "timestamp": "2026-09-03T17:04:11Z"
}
```

| Field | Notes |
|-------|-------|
| `magic` | Always `"SDR-C2-BEACON"`; receivers must drop datagrams that lack it |
| `protocol_version` | Bump on any breaking change to this payload or the REST contract |
| `server_id` | Stable for the lifetime of a server process (UUID v4) |
| `advertised_host` | The IPv4 address the server believes clients should dial; per-interface datagrams carry that interface's address |
| `ports.c2` | TCP port of the REST API |
| `ports.audio_out` | UDP port for the receive WebRTC media (ICE host candidate) |
| `ports.audio_in` | UDP port reserved for the phase-2 transmit stream |
| `c2_base_url` | Convenience: `http://<advertised_host>:<c2>` |
| `device` | `null` when no SDR is selected; `devices_available` still reports how many were enumerated |
| `capabilities` | Mirrors `GET /api/v1/server.capabilities` |
| `timestamp` | RFC 3339 UTC send time |

## Client guidance

1. Bind `0.0.0.0:50055` UDP, enable address reuse.
2. For each datagram: parse JSON, require `magic == "SDR-C2-BEACON"` and
   `protocol_version == 1`.
3. Key servers by `server_id`; treat a server as gone after ~5 s (5 missed
   beacons) with no datagram.
4. Connect using `c2_base_url` (or `advertised_host` + `ports.c2`), then drive
   the [REST API](rest-api.md).

## Security note

The beacon is unauthenticated and world-readable on the segment. It carries no
secrets — only how to reach an API that itself requires a session token for
anything stateful. Do not route it off the local segment.
