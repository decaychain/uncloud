package de.lunarstream.uncloud

import android.content.res.Configuration
import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.WebSettings
import android.webkit.WebView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)

    // Edge-to-edge: let the WebView render under the (transparent) system
    // bars. The web app uses `env(safe-area-inset-*)` so its own navbar and
    // page backgrounds paint the inset areas — the native layer no longer
    // needs to know or track the app theme's colour.
    WindowCompat.setDecorFitsSystemWindows(window, false)

    // Paint the window background with DaisyUI `base-100` so there's no
    // white flash before the WebView renders. Follows the system dark-mode
    // preference since the web app also defaults to that via
    // `prefers-color-scheme`. Once WebView paints, this is never visible.
    val systemDark = isSystemDark(resources.configuration)
    window.setBackgroundDrawable(
      ColorDrawable(if (systemDark) 0xFF1D232A.toInt() else 0xFFFFFFFF.toInt())
    )

    WindowCompat.getInsetsController(window, window.decorView).let { controller ->
      controller.show(WindowInsetsCompat.Type.systemBars())
      controller.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_DEFAULT
    }
    applyTheme(dark = systemDark)

    window.decorView.post {
      findWebView(window.decorView)?.let { webView ->
        webView.settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        CookieManager.getInstance().setAcceptThirdPartyCookies(webView, true)
        webView.addJavascriptInterface(AndroidBridge(this), "UncloudAndroid")
        // Keep WebView transparent so the window background paints during
        // load instead of the WebView's default white.
        webView.setBackgroundColor(Color.TRANSPARENT)
      }
    }
  }

  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    applyTheme(dark = isSystemDark(newConfig))
  }

  private fun isSystemDark(config: Configuration): Boolean =
    (config.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES

  /**
   * Set the system bar *icon* appearance (light vs dark glyphs) so they
   * contrast with the app theme. The bar *backgrounds* are painted by the
   * web layer via edge-to-edge + `env(safe-area-inset-*)`; only the icons
   * are controlled here. Called on startup, on system dark-mode toggle via
   * `onConfigurationChanged`, and imperatively from the in-app theme toggle
   * via `UncloudAndroid.setTheme(dark)`.
   */
  fun applyTheme(dark: Boolean) {
    WindowCompat.getInsetsController(window, window.decorView).let { controller ->
      controller.isAppearanceLightStatusBars = !dark
      controller.isAppearanceLightNavigationBars = !dark
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

  /** Bridge exposed as `window.UncloudAndroid` inside the WebView. */
  class AndroidBridge(private val activity: MainActivity) {
    @JavascriptInterface
    fun setTheme(dark: Boolean) {
      activity.runOnUiThread { activity.applyTheme(dark) }
    }
  }
}
