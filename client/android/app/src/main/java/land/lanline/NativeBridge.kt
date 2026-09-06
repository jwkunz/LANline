package land.lanline

import android.webkit.JavascriptInterface

/**
 * Exposed to the web app as `window.LanlineNative`. The web client feature-detects
 * this object; on a plain browser it is absent and the app falls back to manual
 * host entry.
 */
class NativeBridge(
    private val beacon: BeaconListener,
    private val onConnectedCb: (base: String, label: String, fleetUrl: String) -> Unit,
) {

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

    /** Called by the web client once it has connected to a server, so the
     *  wrapper can stop auto-popping the radio chooser and sync its toolbar. */
    @JavascriptInterface
    fun onConnected(base: String, label: String, fleetUrl: String) {
        onConnectedCb(base, label, fleetUrl)
    }
}
