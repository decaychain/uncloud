package de.lunarstream.uncloud

import android.graphics.Color
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.WebSettings
import android.webkit.WebView
import android.widget.FrameLayout
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)

    // Ensure system bars are visible and icons are dark (for light backgrounds).
    WindowCompat.getInsetsController(window, window.decorView).let { controller ->
      controller.show(WindowInsetsCompat.Type.systemBars())
      controller.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_DEFAULT
      controller.isAppearanceLightStatusBars = true
      controller.isAppearanceLightNavigationBars = true
    }

    // Add padding so content sits below system bars, and draw a solid
    // background behind the status bar area to simulate an opaque bar.
    ViewCompat.setOnApplyWindowInsetsListener(window.decorView) { view, insets ->
      val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
      view.setPadding(systemBars.left, systemBars.top, systemBars.right, systemBars.bottom)
      WindowInsetsCompat.CONSUMED
    }

    // Set a neutral background on the root so the padded status bar area
    // isn't just a transparent gap showing the default activity background.
    window.decorView.setBackgroundColor(Color.WHITE)

    // Configure WebView: allow mixed content (the HTTPS tauri.localhost origin
    // needs to reach the user's HTTP server) and third-party cookies.
    window.decorView.post {
      findWebView(window.decorView)?.let { webView ->
        webView.settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        CookieManager.getInstance().setAcceptThirdPartyCookies(webView, true)
      }
    }
  }

  private fun findWebView(view: View): WebView? {
    if (view is WebView) return view
    if (view is ViewGroup) {
      for (i in 0 until view.childCount) {
        findWebView(view.getChildAt(i))?.let { return it }
      }
    }
    return null
  }
}
