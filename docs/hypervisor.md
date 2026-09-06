# The hypervisor — running a fleet of radios

`lanline-server` is deliberately **one process, one SDR, one DSP pipeline**.
To run several radios at once — a 2 m repeater on a HackRF while an
ADALM-Pluto decodes APRS, say — `lanline-hypervisor` launches one server per
radio and keeps them out of each other's way.

```
lanline-hypervisor
 ├─ radio 0  "VHF"   lanline-server  --c2-port 8730 --device driver=hackrf,serial=… …
 ├─ radio 1  "APRS"  lanline-server  --c2-port 8731 --device driver=plutosdr,serial=… …
 ├─ per-child LANLINE-BEACON  ×N        (existing clients see N servers, unchanged)
 ├─ LANLINE-FLEET-BEACON                (grouping + labels + this host's address)
 ├─ _lanline._tcp mDNS  ×N              (lanline-0.local, lanline-1.local, …)
 └─ GET http://<host>:8720/api/v1/fleet
```

Each child is a completely ordinary server: its own REST API, web client,
WebRTC audio, decode feeds, transmit path. Nothing is shared in memory — the
clean base for a later cross-radio *link* or *TDOA* feature, which would
coordinate over the REST API.

## Quick start

```
scripts/run-fleet.sh --config scripts/fleet.toml
# or, no file:
scripts/run-fleet.sh --radio driver=hackrf --radio driver=plutosdr --tls
```

Then open `http://<host>:8720/` for the fleet index, or point any LANline
client at one of the child ports (`8730`, `8731`, …). The Android app shows a
native "Select radio" chooser when it hears the fleet beacon.

## Config file (`--config fleet.toml`)

```toml
[defaults]                 # optional — per-radio defaults
tls = true
enable_tx = false
freq_correction_ppm = 0.0

[ports]                    # optional — base ports; radio i gets base + i
c2 = 8730
audio_out = 8740
audio_in = 8750
beast = 30005              # 0 disables the Beast feed on every child
ais_nmea = 10110           # 0 disables the AIVDM feed
aprs = 10152               # 0 disables the TNC2 feed

[[radio]]
label = "VHF"
device = "driver=hackrf"
freq_correction_ppm = -8.9
enable_tx = true

[[radio]]
label = "APRS"
device = "driver=plutosdr"
```

`--radio <device-filter>` on the command line adds radios after any from the
file (label defaults to `radio-<i>`, everything else from `[defaults]`).

Port bases must be at least *N* apart for *N* radios; the defaults leave 10
each, so up to **8** radios never collide. Every assigned TCP port is
bind-checked before anything spawns — a clash aborts with the offending port
named.

## Device resolution

Built with the `soapy` feature (the default), the hypervisor enumerates
SoapySDR once and resolves each `device` filter to exactly one physical unit,
rewriting it to a `driver=…,serial=…` string it passes to the child. Zero
matches, multiple matches, or two radios landing on the same unit all abort
with a clear message. Without `soapy`, filters are passed through untouched
and only checked for being distinct strings.

## Per-child isolation

| Resource | How it is kept separate |
|----------|-------------------------|
| C2 / feed ports | `base + index` (bind-checked up front) |
| `audio_out` / `audio_in` UDP | `base + index` |
| IQ recordings | `--iq-dir  <state-dir>/radio-<i>` |
| TLS cert cache | `--tls-dir <state-dir>/tls-radio-<i>` |
| Identity | `--server-id <uuid>` pinned per slot (stable across restarts) |
| mDNS name | `--mdns-name lanline-<i>` |
| Label | `--instance-label <label>` → `GET /api/v1/server`, beacons |

`<state-dir>` defaults to `$XDG_STATE_HOME/lanline-hypervisor` (or
`~/.local/state/lanline-hypervisor`); override with `--state-dir`.

Children run with `--no-beacon --no-mdns` — the hypervisor owns discovery.

## Supervision

Each child that exits unexpectedly is restarted with exponential backoff
(1 s → 32 s), reset once it has stayed up more than 60 s. `--no-restart`
leaves a dead child down; `--max-restarts <n>` gives up after `n`.

On `SIGINT`/`SIGTERM` the hypervisor sends `SIGINT` to every child (the same
graceful path as Ctrl-C on a standalone server), waits up to 5 s each, then
force-kills stragglers.

Child stdout/stderr is merged into the hypervisor's log, prefixed
`[radio <i>]`.

## Discovery the hypervisor emits

- **`LANLINE-BEACON` per running child** — byte-identical in shape to what a
  standalone server sends (real ports, pinned `server_id`, device, the
  `instance_label`). Existing Android / CLI discovery lists N servers with no
  change.
- **`LANLINE-FLEET-BEACON`** — one datagram describing the whole fleet:
  `fleet_id`, `hostname`, `fleet_base_url`, and a `radios[]` array of
  `{ idx, label, server_id, c2_base_url, device, running, ports }`. See
  [beacon-protocol.md](beacon-protocol.md).
- **mDNS** — one `_lanline._tcp` instance per child.
- **`GET /api/v1/fleet`** on `--fleet-port` (default `8720`) — the same
  `radios[]` plus `restarts` / `pid` / `started_at`, always current. `GET /`
  is a plain HTML index of links.

## Flags

| Flag | Default | Meaning |
|------|---------|---------|
| `--config <file>` | — | fleet TOML |
| `--radio <filter>` | — | add one radio (repeatable) |
| `--server-bin <path>` | sibling of the hypervisor exe | `lanline-server` binary |
| `--state-dir <dir>` | `~/.local/state/lanline-hypervisor` | per-child state root |
| `--bind <addr>` | `0.0.0.0` | address children + fleet endpoints bind |
| `--advertise-host <ip>` | autodetected LAN IPv4 | host in advertised URLs |
| `--fleet-port <port>` | `8720` | fleet HTTP listener (`0` disables) |
| `--beacon-port <port>` | `50055` | aggregated discovery beacons (`--no-beacon` disables) |
| `--tls` / `--enable-tx` | off | default for every child (per-radio config overrides) |
| `--no-restart` / `--max-restarts <n>` | restart forever | supervision bounds |
| `--no-mdns` | off | skip the per-child mDNS instances |
