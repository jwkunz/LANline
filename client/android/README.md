# LANline — Android client

A thin native wrapper around the web client (`web/`): a full-screen `WebView`
plus a Kotlin UDP **beacon listener** that feeds discovered servers to the web
app through `window.LanlineNative`.

## Build

Needs a JDK 17+ (Android Studio's bundled JBR works) and the Android SDK
(platform 35, build-tools 35).

```sh
cd web && npm install && npm run build            # produces web/dist
cd ../client/android
cp local.properties.sample local.properties       # set sdk.dir
./gradlew assembleDebug                            # -> app/build/outputs/apk/debug/
./gradlew installDebug                             # to a connected device
```

`assembleDebug` runs `syncWebAssets`, which copies `web/dist` into
`app/src/main/assets/web` — rebuild the web client whenever it changes.

Or just open `client/android/` in Android Studio and Run.

### Loading the live dev server instead of bundled assets

Set `lanline.devServerUrl=http://<your-lan-ip>:5173` in `local.properties` (start
Vite with `npm run dev` — it already binds `0.0.0.0`). Handy for iterating on
the web UI without repackaging.

## How it fits together

| Piece | Role |
|-------|------|
| `MainActivity` | Hosts the `WebView`; JS + DOM storage on; grants no capture perms (receive-only WebRTC); holds a Wi-Fi `MulticastLock` while visible |
| `BeaconListener` | Binds UDP `:50055`, parses `LANLINE-BEACON` datagrams, keeps a 5 s-TTL server map |
| `NativeBridge` | `window.LanlineNative.discoveredServers()` / `.platform()` — the web app feature-detects this |
| `network_security_config.xml` | Permits cleartext HTTP (the C2 server is plain HTTP on the LAN) |

Permissions: `INTERNET`, `ACCESS_NETWORK_STATE`, `ACCESS_WIFI_STATE`,
`CHANGE_WIFI_MULTICAST_STATE`. No microphone — the app only receives audio.

`minSdk 26`, `targetSdk / compileSdk 35`.
