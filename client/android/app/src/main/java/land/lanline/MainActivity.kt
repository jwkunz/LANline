package land.lanline

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.SharedPreferences
import android.content.pm.PackageManager
import android.net.http.SslError
import android.net.wifi.WifiManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Menu
import android.view.MenuItem
import android.view.ViewGroup
import android.view.WindowManager
import android.webkit.GeolocationPermissions
import android.webkit.PermissionRequest
import android.webkit.SslErrorHandler
import android.webkit.WebChromeClient
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.LinearLayout
import java.net.InetAddress
import java.net.URI
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.appcompat.widget.Toolbar
import androidx.core.content.ContextCompat

/**
 * Single-activity WebView host. Loads the bundled web client (or a configured
 * dev-server URL), bridges the native UDP beacon listener into it, and holds a
 * Wi-Fi multicast lock while visible so broadcast beacons are delivered.
 *
 * When a `lanline-hypervisor` fleet is on the LAN it also shows a small native
 * chooser (a toolbar + "Radios" menu) so the user picks which radio / port
 * the WebView points at, and can switch later.
 */
class MainActivity : AppCompatActivity() {

    private lateinit var webView: WebView
    private lateinit var toolbar: Toolbar
    private val beacon = BeaconListener()
    private var multicastLock: WifiManager.MulticastLock? = null

    private lateinit var prefs: SharedPreferences
    private val ui = Handler(Looper.getMainLooper())
    private var autoChooserDone = false
    private var pickedRadio = false
    private var chooserDialog: AlertDialog? = null

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
        prefs = getSharedPreferences("lanline", Context.MODE_PRIVATE)

        // It's an appliance you leave running: keep the screen on so the
        // WebView (and its WebRTC audio session) isn't throttled or dropped.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        webView = WebView(this)
        toolbar = Toolbar(this).apply {
            setBackgroundColor(0xFF1b2330.toInt())
            setTitleTextColor(0xFFf0f3f7.toInt())
            title = "LANline"
        }
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        root.addView(
            toolbar,
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT),
        )
        root.addView(
            webView,
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f),
        )
        setContentView(root)
        setSupportActionBar(toolbar)

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

        webView.addJavascriptInterface(
            NativeBridge(beacon) { base, label, _ -> ui.post { onWebConnected(base, label) } },
            "LanlineNative",
        )

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

        val devUrl = BuildConfig.DEV_SERVER_URL
        val savedUrl = prefs.getString(KEY_RADIO_URL, null)
        when {
            devUrl.isNotEmpty() -> {
                webView.loadUrl(devUrl)
                autoChooserDone = true
            }
            savedUrl != null -> {
                pickedRadio = true
                autoChooserDone = true
                toolbar.title = prefs.getString(KEY_RADIO_LABEL, null) ?: "LANline"
                webView.loadUrl(savedUrl)
            }
            else -> {
                // Show the bundled UI (it has its own manual/discovery picker,
                // and auto-connects to its last host). Pop the native chooser
                // only if a fleet is visible and the web client hasn't already
                // connected somewhere (onWebConnected cancels this).
                webView.loadUrl(BUNDLED_URL)
                ui.postDelayed(autoChooserPoll, 2_500)
            }
        }
    }

    override fun onCreateOptionsMenu(menu: Menu): Boolean {
        menu.add(0, MENU_SWITCH, 0, "Radios").setShowAsAction(MenuItem.SHOW_AS_ACTION_ALWAYS)
        menu.add(0, MENU_RELOAD, 1, "Reload")
        return true
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean = when (item.itemId) {
        MENU_SWITCH -> { showRadioChooser(); true }
        MENU_RELOAD -> { webView.reload(); true }
        else -> super.onOptionsItemSelected(item)
    }

    /** Poll the beacon map; the first time a fleet (or any server) shows up and
     *  the user has not picked one, pop the chooser. Gives up after ~30 s. */
    private val autoChooserPoll = object : Runnable {
        private var elapsed = 0
        override fun run() {
            if (autoChooserDone) return
            val haveSomething =
                beacon.fleetSnapshot().isNotEmpty() || beacon.standaloneServers().isNotEmpty()
            if (haveSomething) {
                autoChooserDone = true
                showRadioChooser()
                return
            }
            elapsed += 1_500
            if (elapsed < 30_000) ui.postDelayed(this, 1_500)
        }
    }

    private fun showRadioChooser() {
        data class Entry(val label: String, val url: String)

        val entries = ArrayList<Entry>()
        for (fleet in beacon.fleetSnapshot()) {
            for (r in fleet.radios.sortedBy { it.idx }) {
                val dot = if (r.running) "●" else "○"
                val dev = shortDevice(r.device)?.let { " · $it" } ?: ""
                val port = runCatching { URI(r.c2BaseUrl).port }.getOrNull()?.takeIf { it > 0 }
                val portStr = port?.let { " · :$it" } ?: ""
                entries.add(Entry("$dot ${fleet.hostname} / ${r.label}$dev$portStr", r.c2BaseUrl))
            }
        }
        for (s in beacon.standaloneServers()) {
            val name = s.label ?: s.hostname
            val dev = shortDevice(s.device)?.let { " · $it" } ?: ""
            entries.add(Entry("$name$dev · :${s.c2}", s.baseUrl.ifEmpty { "http://${s.hostname}:${s.c2}/" }))
        }

        val builder = AlertDialog.Builder(this).setTitle("Select radio")
        if (entries.isEmpty()) {
            builder.setMessage("No LANline radios found on the network yet.")
                .setPositiveButton("OK", null)
        } else {
            val labels = entries.map { it.label }.toTypedArray()
            builder.setItems(labels) { _, i -> loadRadio(entries[i].label, entries[i].url) }
                .setNegativeButton("Cancel", null)
        }
        chooserDialog?.dismiss()
        chooserDialog = builder.create().also { it.show() }
    }

    /** "HackRF One #0 f77c…" -> "HackRF One"; blank/empty -> null. */
    private fun shortDevice(d: String?): String? =
        d?.substringBefore(" #")?.trim()?.ifEmpty { null }

    private fun loadRadio(label: String, url: String) {
        pickedRadio = true
        autoChooserDone = true
        chooserDialog = null
        // Strip the leading run/stop dot from the toolbar title.
        toolbar.title = label.trimStart('●', '○', ' ')
        prefs.edit().putString(KEY_RADIO_URL, url).putString(KEY_RADIO_LABEL, toolbar.title.toString()).apply()
        webView.loadUrl(url)
    }

    /** The web client connected to a server on its own (e.g. its saved host).
     *  Stop nagging with the auto-chooser and sync the toolbar / saved pick. */
    private fun onWebConnected(base: String, label: String) {
        autoChooserDone = true
        pickedRadio = true
        ui.removeCallbacks(autoChooserPoll)
        chooserDialog?.dismiss()
        chooserDialog = null
        val title = label.ifEmpty {
            runCatching { URI(base).host }.getOrNull()?.ifEmpty { null } ?: "LANline"
        }
        toolbar.title = title
        prefs.edit().putString(KEY_RADIO_URL, base).putString(KEY_RADIO_LABEL, title).apply()
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
        ui.removeCallbacksAndMessages(null)
        chooserDialog?.dismiss()
        chooserDialog = null
        webView.destroy()
        super.onDestroy()
    }

    companion object {
        private const val BUNDLED_URL = "file:///android_asset/web/index.html"
        private const val KEY_RADIO_URL = "radio_url"
        private const val KEY_RADIO_LABEL = "radio_label"
        private const val MENU_SWITCH = 1
        private const val MENU_RELOAD = 2
    }
}
