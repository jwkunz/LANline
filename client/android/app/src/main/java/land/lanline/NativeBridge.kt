package land.lanline

import android.webkit.JavascriptInterface

/**
 * Exposed to the web app as `window.LanlineNative`. The web client feature-detects
 * this object; on a plain browser it is absent and the app falls back to manual
 * host entry.
 */
class NativeBridge(private val beacon: BeaconListener) {

    @JavascriptInterface
    fun platform(): String = "android"

    /** JSON array of servers currently announcing on the LAN. */
    @JavascriptInterface
    fun discoveredServers(): String = beacon.snapshotJson()
}
