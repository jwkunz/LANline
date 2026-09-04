package land.my.sdrc2

import android.annotation.SuppressLint
import android.content.Context
import android.net.wifi.WifiManager
import android.os.Bundle
import android.webkit.PermissionRequest
import android.webkit.WebChromeClient
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.OnBackPressedCallback
import androidx.appcompat.app.AppCompatActivity

/**
 * Single-activity WebView host. Loads the bundled web client (or a configured
 * dev-server URL), bridges the native UDP beacon listener into it, and holds a
 * Wi-Fi multicast lock while visible so broadcast beacons are delivered.
 */
class MainActivity : AppCompatActivity() {

    private lateinit var webView: WebView
    private val beacon = BeaconListener()
    private var multicastLock: WifiManager.MulticastLock? = null

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

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
            // The web app is a single local file that calls the plain-HTTP C2
            // server on the LAN; allow cross-origin XHR/fetch from file://.
            @Suppress("DEPRECATION")
            allowUniversalAccessFromFileURLs = true
        }
        WebView.setWebContentsDebuggingEnabled(true)

        webView.webViewClient = WebViewClient()
        webView.webChromeClient = object : WebChromeClient() {
            override fun onPermissionRequest(request: PermissionRequest) {
                // Receive-only WebRTC needs no capture permission.
                request.deny()
            }
        }

        webView.addJavascriptInterface(NativeBridge(beacon), "SdrNative")

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

    override fun onStart() {
        super.onStart()
        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicastLock = wifi.createMulticastLock("sdrc2-beacon").apply {
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
