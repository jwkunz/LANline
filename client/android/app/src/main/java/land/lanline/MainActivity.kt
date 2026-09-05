package land.lanline

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.net.http.SslError
import android.net.wifi.WifiManager
import android.os.Bundle
import android.view.WindowManager
import android.webkit.GeolocationPermissions
import android.webkit.PermissionRequest
import android.webkit.SslErrorHandler
import android.webkit.WebChromeClient
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import java.net.InetAddress
import java.net.URI
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat

/**
 * Single-activity WebView host. Loads the bundled web client (or a configured
 * dev-server URL), bridges the native UDP beacon listener into it, and holds a
 * Wi-Fi multicast lock while visible so broadcast beacons are delivered.
 */
class MainActivity : AppCompatActivity() {

    private lateinit var webView: WebView
    private val beacon = BeaconListener()
    private var multicastLock: WifiManager.MulticastLock? = null

    private var pendingGeo: Pair<String, GeolocationPermissions.Callback>? = null
    private val locationPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            pendingGeo?.let { (origin, cb) -> cb.invoke(origin, granted, false) }
            pendingGeo = null
        }

    // FRS push-to-talk's mic (web/src/audio.ts's getUserMedia) — same
    // request-then-resolve shape as location above, just against
    // WebChromeClient's own PermissionRequest instead of Geolocation's.
    private var pendingAudio: PermissionRequest? = null
    private val micPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            pendingAudio?.let { req -> if (granted) req.grant(req.resources) else req.deny() }
            pendingAudio = null
        }

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // It's an appliance you leave running: keep the screen on so the
        // WebView (and its WebRTC audio session) isn't throttled or dropped.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        webView = WebView(this)
        setContentView(webView)

        with(webView.settings) {
            javaScriptEnabled = true
            domStorageEnabled = true
            // The wrapper auto-starts playback on connect, so autoplay must be
            // allowed without a gesture.
            mediaPlaybackRequiresUserGesture = false
            mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
            cacheMode = WebSettings.LOAD_DEFAULT
            // The web app is a single local file that calls the C2 server on
            // the LAN (http, or https with --tls); allow cross-origin
            // XHR/fetch from file://.
            @Suppress("DEPRECATION")
            allowUniversalAccessFromFileURLs = true
            @Suppress("DEPRECATION")
            setGeolocationEnabled(true)
        }
        WebView.setWebContentsDebuggingEnabled(true)

        webView.webViewClient = object : WebViewClient() {
            // A LANline server run with --tls serves a self-signed cert that
            // chains to nothing. This app only ever talks to servers it found
            // on the local network, so accept a bad cert from a private /
            // link-local address or a *.local name, and reject everything
            // else (a MITM on a routable address, say).
            override fun onReceivedSslError(
                view: WebView?,
                handler: SslErrorHandler,
                error: SslError,
            ) {
                if (isLocalHost(runCatching { URI(error.url).host }.getOrNull().orEmpty())) {
                    handler.proceed()
                } else {
                    handler.cancel()
                }
            }
        }
        webView.webChromeClient = object : WebChromeClient() {
            override fun onPermissionRequest(request: PermissionRequest) {
                // FRS's push-to-talk mic is the only capture this app ever
                // asks for (see acquireMicIfNeeded in web/src/main.ts) —
                // everything else stays receive-only WebRTC, denied here.
                if (request.resources.contains(PermissionRequest.RESOURCE_AUDIO_CAPTURE)) {
                    val granted = ContextCompat.checkSelfPermission(
                        this@MainActivity,
                        Manifest.permission.RECORD_AUDIO,
                    ) == PackageManager.PERMISSION_GRANTED
                    if (granted) {
                        request.grant(request.resources)
                    } else {
                        pendingAudio = request
                        micPermission.launch(Manifest.permission.RECORD_AUDIO)
                    }
                } else {
                    request.deny()
                }
            }

            override fun onGeolocationPermissionsShowPrompt(
                origin: String,
                callback: GeolocationPermissions.Callback,
            ) {
                val granted = ContextCompat.checkSelfPermission(
                    this@MainActivity,
                    Manifest.permission.ACCESS_COARSE_LOCATION,
                ) == PackageManager.PERMISSION_GRANTED
                if (granted) {
                    callback.invoke(origin, true, false)
                } else {
                    pendingGeo = origin to callback
                    locationPermission.launch(Manifest.permission.ACCESS_COARSE_LOCATION)
                }
            }
        }

        webView.addJavascriptInterface(NativeBridge(beacon), "LanlineNative")

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (webView.canGoBack()) {
                    webView.goBack()
                } else {
                    isEnabled = false
                    onBackPressedDispatcher.onBackPressed()
                }
            }
        })

        // Single-file bundle → loads fine straight from assets; `file://` →
        // `http://<lan>` API calls are not treated as mixed content.
        val url = BuildConfig.DEV_SERVER_URL.ifEmpty { "file:///android_asset/web/index.html" }
        webView.loadUrl(url)
    }

    /** A hostname/IP that can only be a machine on this LAN. */
    private fun isLocalHost(host: String): Boolean {
        if (host.isEmpty()) return false
        if (host.equals("localhost", ignoreCase = true) ||
            host.endsWith(".local", ignoreCase = true)
        ) {
            return true
        }
        // Only resolve IP literals (no DNS, so no blocking call on this
        // thread). Servers come from the beacon with numeric c2_base_urls, so
        // that's the realistic case anyway.
        if (!host.matches(Regex("^[0-9.]+$|^[0-9a-fA-F:]+$"))) return false
        return runCatching {
            val a = InetAddress.getByName(host)
            a.isSiteLocalAddress || a.isLinkLocalAddress || a.isLoopbackAddress
        }.getOrDefault(false)
    }

    override fun onStart() {
        super.onStart()
        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicastLock = wifi.createMulticastLock("lanline-beacon").apply {
            setReferenceCounted(false)
            acquire()
        }
        beacon.start()
    }

    override fun onStop() {
        beacon.stop()
        multicastLock?.let { if (it.isHeld) it.release() }
        multicastLock = null
        super.onStop()
    }

    override fun onDestroy() {
        webView.destroy()
        super.onDestroy()
    }
}
