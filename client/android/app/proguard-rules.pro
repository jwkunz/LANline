# Keep the JS bridge methods reachable from the WebView.
-keepclassmembers class land.my.sdrc2.NativeBridge {
    @android.webkit.JavascriptInterface <methods>;
}
