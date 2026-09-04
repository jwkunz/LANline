# Keep the JS bridge methods reachable from the WebView.
-keepclassmembers class land.lanline.NativeBridge {
    @android.webkit.JavascriptInterface <methods>;
}
