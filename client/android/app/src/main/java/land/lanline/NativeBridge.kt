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

    /** JSON array of hypervisor fleets (each with its radio list) currently
     *  announcing on the LAN. For a future tabbed web UI; the native chooser
     *  uses the typed snapshot directly. */
    @JavascriptInterface
    fun fleets(): String = beacon.fleetsJson()
}
