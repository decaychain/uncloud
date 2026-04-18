package de.lunarstream.uncloud

import android.content.res.Configuration
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.WebSettings
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)

    // System bars stay visible; their *appearance* (light vs dark icons) and
    // background colours are updated dynamically by the web app via the
    // `UncloudAndroid.setTheme()` JS bridge below.
    WindowCompat.getInsetsController(window, window.decorView).let { controller ->
      controller.show(WindowInsetsCompat.Type.systemBars())
      controller.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_DEFAULT
    }

    // Pad content so it sits below the status bar and above the nav bar.
    ViewCompat.setOnApplyWindowInsetsListener(window.decorView) { view, insets ->
      val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
      view.setPadding(systemBars.left, systemBars.top, systemBars.right, systemBars.bottom)
      WindowInsetsCompat.CONSUMED
    }

    // Initial paint follows the system dark-mode preference — the same
    // default the web app uses via `prefers-color-scheme`. If the user has
    // overridden the theme in-app the web layer will correct it on boot.
    val nightMode = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK
    applyTheme(dark = nightMode == Configuration.UI_MODE_NIGHT_YES)

    window.decorView.post {
      findWebView(window.decorView)?.let { webView ->
        webView.settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        CookieManager.getInstance().setAcceptThirdPartyCookies(webView, true)
        webView.addJavascriptInterface(AndroidBridge(this), "UncloudAndroid")
      }
    }
  }

  /**
   * Match the system bars to the current DaisyUI theme so the inlets blend
   * into the app rather than showing the OEM default bar colour.
   *
   * Colours come from DaisyUI 4's default themes:
   *   light: base-200 #F2F2F2  base-100 #FFFFFF
   *   dark:  base-200 #191E24  base-100 #1D232A
   *
   * The status bar (top) sits against the sticky `bg-base-200` navbar; the
   * navigation bar (bottom) sits against page content which is `bg-base-100`.
   */
  fun applyTheme(dark: Boolean) {
    val statusBg = if (dark) 0xFF191E24.toInt() else 0xFFF2F2F2.toInt()
    val navBg    = if (dark) 0xFF1D232A.toInt() else 0xFFFFFFFF.toInt()

    window.decorView.setBackgroundColor(statusBg)
    window.statusBarColor = statusBg
    window.navigationBarColor = navBg

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
