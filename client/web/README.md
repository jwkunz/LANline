# SDR C2 — web client

Vanilla TypeScript + Vite. LAN server connection, REST session with heartbeat,
live radio/telemetry display, and received-audio playback over WebRTC/Opus
(Play button — a user gesture is required to start audio).

```sh
npm install
npm run dev        # http://localhost:5173  (also served on the LAN)
```

Enter the server host from the server's startup log (e.g.
`192.168.1.50:41785`) and connect. The host is remembered in `localStorage`.

- `npm run build` — typecheck (`tsc`) + production bundle to `dist/`
- `npm run preview` — serve the built bundle

## Files

| File | Role |
|------|------|
| `src/types.ts` | TS mirrors of `server/src/model.rs` DTOs |
| `src/api.ts` | typed `fetch` wrapper + `ApiError` + host normalization |
| `src/audio.ts` | `AudioSession`: recvonly `RTCPeerConnection`, non-trickle offer |
| `src/main.ts` | app: connect flow, heartbeat + 1 Hz poll loops, rendering |
| `src/style.css` | light/dark styling |
