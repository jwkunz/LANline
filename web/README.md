# LANline — web client

Vanilla TypeScript + Vite. Mode-selection wizard (NOAA Weather / FM Broadcast /
AM Radio / NOAA APT / ADS-B / AIS / Debug Tone), nearest-station finders, seek,
a shared radar scope for the ADS-B + AIS traffic trackers (canvas, no map
tiles), a canvas renderer for the NOAA APT satellite image, REST session with
heartbeat, live radio/telemetry display, and received-audio playback over
WebRTC/Opus (Play button — a user gesture is required to start audio).

`npm run build` produces a **single self-contained `dist/index.html`** (all
JS/CSS inlined, via `vite-plugin-singlefile`). That bundle plus the three
station databases in `public/` are what the `lanline-server` binary embeds and
serves at `/`, and what the Android APK ships as `file://` assets — so it must
load from a non-root base and make same-origin API calls.

## Connecting

- **Served by the server** (the normal case): the page detects it is same-origin
  with a LANline server (`GET /health` → `{"status":"ok"}`) and connects there
  with nothing to type.
- **Android wrapper**: `window.LanlineNative` supplies servers discovered via
  the UDP beacon; the app auto-connects to the first.
- **`npm run dev`**: no server origin and no native bridge, so type the host
  from the server's startup log (e.g. `lanline.local:8730`). It is remembered in
  `localStorage`.

```sh
npm install
npm run dev        # http://localhost:5173  (also served on the LAN)
npm run build      # typecheck (tsc) + single-file bundle to dist/
npm run preview    # serve the built bundle
```

## Files

| File | Role |
|------|------|
| `src/types.ts` | TS mirrors of `server/src/model.rs` DTOs |
| `src/api.ts` | typed `fetch` wrapper + `ApiError` + host normalization |
| `src/audio.ts` | `AudioSession`: recvonly `RTCPeerConnection`, non-trickle offer |
| `src/discovery.ts` | `window.LanlineNative` bridge (Android beacon discovery) |
| `src/geo.ts` | haversine nearest-N + `lat, lon` parse + XHR JSON loader (works from `file://`) |
| `src/nwr.ts` / `src/fm.ts` / `src/am.ts` | NOAA Weather Radio + FM broadcast + AM broadcast station directories, nearest-station search |
| `src/adsb.ts` / `src/ais.ts` | ADS-B + AIS track types + the equirectangular projection the shared radar scope uses |
| `src/apt.ts` | NOAA APT status/image types, binary raster decode, the NOAA-15/18/19 satellite frequency table |
| `public/nwr-stations.json` / `public/fm-stations.json` / `public/am-stations.json` | the bundled directories (served by the server too) |
| `src/main.ts` | app: origin/host connect flow, mode wizard, heartbeat + 1 Hz poll loops, seek, rendering |
| `src/style.css` | light/dark styling |

Regenerate the station data (committed so builds need no network):

```sh
node ../scripts/fetch-nwr.mjs     # ~1000 NWR transmitters, from NWS CCL.js
node ../scripts/fetch-fm.mjs      # ~11k licensed FM stations, from the FCC FM Query
node ../scripts/fetch-am.mjs      # ~4.3k licensed AM stations, from the FCC AM Query
```
