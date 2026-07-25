package de.lunarstream.uncloud

import android.app.DownloadManager
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.view.View
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.URLUtil
import android.webkit.WebSettings
import android.webkit.WebView
import android.widget.Toast
import androidx.core.content.FileProvider
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.KeyStore
import java.security.MessageDigest
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlin.concurrent.thread

private class DeviceAuthStore(context: Context) {
  private val prefs =
    context.applicationContext.getSharedPreferences("uncloud_device_auth", Context.MODE_PRIVATE)

  fun deviceInfo(): String {
    val id = prefs.getString("installation_id", null) ?: UUID.randomUUID().toString().also {
      prefs.edit().putString("installation_id", it).apply()
    }
    val manufacturer = Build.MANUFACTURER.trim()
    val model = Build.MODEL.trim()
    val name = listOf(manufacturer, model)
      .filter { it.isNotBlank() }
      .distinct()
      .joinToString(" ")
      .ifBlank { "Android device" }
    return "$id\t$name"
  }

  fun store(serverUrl: String, refreshToken: String): Boolean = runCatching {
    val key = serverKey(serverUrl)
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.ENCRYPT_MODE, loadOrCreateKey(alias(key)))
    val ciphertext = cipher.doFinal(refreshToken.toByteArray(Charsets.UTF_8))
    val blob = ByteArray(cipher.iv.size + ciphertext.size)
    System.arraycopy(cipher.iv, 0, blob, 0, cipher.iv.size)
    System.arraycopy(ciphertext, 0, blob, cipher.iv.size, ciphertext.size)
    prefs.edit()
      .putInt("$key.iv_len", cipher.iv.size)
      .putString(key, Base64.encodeToString(blob, Base64.NO_WRAP))
      .apply()
    true
  }.getOrDefault(false)

  fun load(serverUrl: String): String? = runCatching {
    val key = serverKey(serverUrl)
    val encoded = prefs.getString(key, null) ?: return null
    val blob = Base64.decode(encoded, Base64.NO_WRAP)
    val ivLength = prefs.getInt("$key.iv_len", 12)
    if (blob.size <= ivLength) {
      clear(serverUrl)
      return null
    }
    val secretKey = loadKey(alias(key)) ?: run {
      clear(serverUrl)
      return null
    }
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(
      Cipher.DECRYPT_MODE,
      secretKey,
      GCMParameterSpec(128, blob.copyOfRange(0, ivLength)),
    )
    String(cipher.doFinal(blob.copyOfRange(ivLength, blob.size)), Charsets.UTF_8)
  }.getOrNull()

  fun clear(serverUrl: String): Boolean = runCatching {
    val key = serverKey(serverUrl)
    prefs.edit().remove(key).remove("$key.iv_len").apply()
    KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.let { store ->
      if (store.containsAlias(alias(key))) store.deleteEntry(alias(key))
    }
    true
  }.getOrDefault(false)

  private fun serverKey(serverUrl: String): String {
    val digest = MessageDigest.getInstance("SHA-256")
      .digest(serverUrl.trimEnd('/').toByteArray(Charsets.UTF_8))
    return digest.joinToString("") { "%02x".format(it.toInt() and 0xff) }
  }

  private fun alias(key: String) = "uncloud.device-auth.$key"

  private fun loadKey(alias: String): SecretKey? = runCatching {
    KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.getKey(alias, null) as? SecretKey
  }.getOrNull()

  private fun loadOrCreateKey(alias: String): SecretKey {
    loadKey(alias)?.let { return it }
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
    generator.init(
      KeyGenParameterSpec.Builder(
        alias,
        KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
      )
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .setRandomizedEncryptionRequired(true)
        .setUserAuthenticationRequired(false)
        .build()
    )
    return generator.generateKey()
  }
}

class MainActivity : TauriActivity() {
  private var webViewConfigured = false

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

    configureWebViewWhenReady()
  }

  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    applyTheme(dark = isSystemDark(newConfig))
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    configureWebView(webView)
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

  private fun configureWebViewWhenReady(attempt: Int = 0) {
    window.decorView.postDelayed({
      val webView = findWebView(window.decorView)
      if (webView == null) {
        if (attempt < 40) configureWebViewWhenReady(attempt + 1)
        return@postDelayed
      }
      configureWebView(webView)
    }, 50L)
  }

  private fun configureWebView(webView: WebView) {
    if (webViewConfigured) return
    webViewConfigured = true
    webView.settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
    CookieManager.getInstance().setAcceptThirdPartyCookies(webView, true)
    webView.addJavascriptInterface(AndroidBridge(this), "UncloudAndroid")
    webView.setDownloadListener { url, _, contentDisposition, mimeType, _ ->
      val filename = URLUtil.guessFileName(url, contentDisposition, mimeType)
      downloadRemoteFile(url, filename, mimeType ?: "")
    }
    // Keep WebView transparent so the window background paints during
    // load instead of the WebView's default white.
    webView.setBackgroundColor(Color.TRANSPARENT)
  }

  private fun safeFilename(filename: String): String {
    val cleaned = filename.replace(Regex("[\\\\/:*?\"<>|]"), "_").trim()
    return cleaned.ifEmpty { "uncloud-file" }
  }

  private fun resolvedMimeType(filename: String, mimeType: String): String {
    if (mimeType.isNotBlank()) return mimeType
    return URLUtil.guessFileName(filename, null, null)
      .substringAfterLast('.', "")
      .takeIf { it.isNotBlank() }
      ?.let { android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(it.lowercase()) }
      ?: "application/octet-stream"
  }

  private fun openRemoteFile(url: String, filename: String, mimeType: String) {
    val safeName = safeFilename(filename)
    val resolvedMime = resolvedMimeType(safeName, mimeType)
    Toast.makeText(this, "Opening $safeName", Toast.LENGTH_SHORT).show()
    thread(name = "uncloud-open-file") {
      var connection: HttpURLConnection? = null
      try {
        val dir = File(cacheDir, "external-open").apply { mkdirs() }
        val target = File(dir, safeName)
        connection = (URL(url).openConnection() as HttpURLConnection).apply {
          instanceFollowRedirects = true
          connectTimeout = 15_000
          readTimeout = 120_000
        }
        connection.inputStream.use { input ->
          target.outputStream().use { output -> input.copyTo(output) }
        }
        runOnUiThread { openCachedFile(target, resolvedMime) }
      } catch (_: Exception) {
        runOnUiThread {
          Toast.makeText(this, "Could not open $safeName", Toast.LENGTH_LONG).show()
        }
      } finally {
        connection?.disconnect()
      }
    }
  }

  private fun openCachedFile(file: File, mimeType: String) {
    val uri = FileProvider.getUriForFile(this, "${packageName}.fileprovider", file)
    val intent = Intent(Intent.ACTION_VIEW).apply {
      setDataAndType(uri, mimeType)
      addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
    try {
      startActivity(Intent.createChooser(intent, "Open with"))
    } catch (_: ActivityNotFoundException) {
      Toast.makeText(this, "No app can open this file", Toast.LENGTH_LONG).show()
    }
  }

  private fun downloadRemoteFile(url: String, filename: String, mimeType: String) {
    val safeName = safeFilename(filename)
    val request = DownloadManager.Request(Uri.parse(url)).apply {
      setTitle(safeName)
      setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
      setDestinationInExternalPublicDir(Environment.DIRECTORY_DOWNLOADS, safeName)
      val resolvedMime = resolvedMimeType(safeName, mimeType)
      setMimeType(resolvedMime)
    }
    try {
      val manager = getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
      manager.enqueue(request)
      Toast.makeText(this, "Downloading $safeName", Toast.LENGTH_SHORT).show()
    } catch (_: Exception) {
      Toast.makeText(this, "Could not download $safeName", Toast.LENGTH_LONG).show()
    }
  }

  /** Bridge exposed as `window.UncloudAndroid` inside the WebView. */
  class AndroidBridge(private val activity: MainActivity) {
    private val deviceAuth = DeviceAuthStore(activity)

    @JavascriptInterface
    fun setTheme(dark: Boolean) {
      activity.runOnUiThread { activity.applyTheme(dark) }
    }

    @JavascriptInterface
    fun openFile(url: String, filename: String, mimeType: String) {
      activity.runOnUiThread { activity.openRemoteFile(url, filename, mimeType) }
    }

    @JavascriptInterface
    fun downloadFile(url: String, filename: String, mimeType: String) {
      activity.runOnUiThread { activity.downloadRemoteFile(url, filename, mimeType) }
    }

    @JavascriptInterface
    fun getDeviceInfo(): String = deviceAuth.deviceInfo()

    @JavascriptInterface
    fun storeRefreshToken(serverUrl: String, refreshToken: String): Boolean =
      deviceAuth.store(serverUrl, refreshToken)

    @JavascriptInterface
    fun loadRefreshToken(serverUrl: String): String? = deviceAuth.load(serverUrl)

    @JavascriptInterface
    fun clearRefreshToken(serverUrl: String): Boolean = deviceAuth.clear(serverUrl)
  }
}
