package de.lunarstream.uncloud.biometric

import android.app.Activity
import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

private const val TAG = "uncloud-biometric"
private const val PREFS_NAME = "uncloud_biometric_vaults"
private const val KEYSTORE_PROVIDER = "AndroidKeyStore"
private const val KEY_ALIAS_PREFIX = "uncloud.vault."
private const val GCM_TAG_BITS = 128

@InvokeArg
class VaultIdArgs {
    var userId: String? = null
    var vaultId: String? = null
}

@InvokeArg
class EnrollArgs {
    var userId: String? = null
    var vaultId: String? = null
    var secret: String? = null
    var reason: String? = null
}

@InvokeArg
class UnlockArgs {
    var userId: String? = null
    var vaultId: String? = null
    var reason: String? = null
}

@TauriPlugin
class BiometricPlugin(private val activity: Activity) : Plugin(activity) {

    private val prefs: SharedPreferences by lazy {
        activity.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
    }

    @Command
    fun status(invoke: Invoke) {
        val mgr = BiometricManager.from(activity.applicationContext)
        val result = mgr.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG)
        val obj = JSObject()
        obj.put("available", result == BiometricManager.BIOMETRIC_SUCCESS)
        if (result != BiometricManager.BIOMETRIC_SUCCESS) {
            obj.put("reason", reasonString(result))
        }
        invoke.resolve(obj)
    }

    @Command
    fun isEnrolled(invoke: Invoke) {
        val args = invoke.parseArgs(VaultIdArgs::class.java)
        val key = compositeKey(args.userId, args.vaultId) ?: run {
            invoke.reject("userId and vaultId are required")
            return
        }
        val obj = JSObject()
        obj.put("enrolled", prefs.contains(key) && keystoreContains(keyAlias(key)))
        invoke.resolve(obj)
    }

    @Command
    fun enroll(invoke: Invoke) {
        val args = invoke.parseArgs(EnrollArgs::class.java)
        val key = compositeKey(args.userId, args.vaultId) ?: run {
            invoke.reject("userId and vaultId are required")
            return
        }
        val secret = args.secret ?: run {
            invoke.reject("secret is required")
            return
        }
        val activity = this.activity as? FragmentActivity ?: run {
            invoke.reject("biometric prompt requires FragmentActivity")
            return
        }

        // Always overwrite — re-enrolment is the recovery path after
        // KeyPermanentlyInvalidatedException, so we don't want stale aliases
        // surviving from previous runs.
        deleteKey(keyAlias(key))
        val secretKey = generateKey(keyAlias(key))

        val cipher = try {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.ENCRYPT_MODE, secretKey)
            c
        } catch (e: Exception) {
            invoke.reject("cipher init failed: ${e.message}")
            return
        }

        val prompt = buildPrompt(activity, object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                val authedCipher = result.cryptoObject?.cipher ?: run {
                    invoke.reject("crypto object missing")
                    return
                }
                runCatching {
                    val ciphertext = authedCipher.doFinal(secret.toByteArray(Charsets.UTF_8))
                    val iv = authedCipher.iv
                    val blob = ByteArray(iv.size + ciphertext.size)
                    System.arraycopy(iv, 0, blob, 0, iv.size)
                    System.arraycopy(ciphertext, 0, blob, iv.size, ciphertext.size)
                    prefs.edit()
                        .putInt(ivLenKey(key), iv.size)
                        .putString(key, Base64.encodeToString(blob, Base64.NO_WRAP))
                        .apply()
                }.onSuccess {
                    invoke.resolve()
                }.onFailure { e ->
                    Log.w(TAG, "enrol doFinal failed", e)
                    invoke.reject("encrypt failed: ${e.message}")
                }
            }

            override fun onAuthenticationError(code: Int, msg: CharSequence) {
                invoke.reject("biometric error $code: $msg")
            }

            override fun onAuthenticationFailed() {
                // Single attempt failed — let the user retry within the
                // same prompt; only resolve/reject on terminal events.
            }
        })
        activity.runOnUiThread {
            prompt.authenticate(promptInfo(args.reason ?: "Enable biometric unlock"),
                BiometricPrompt.CryptoObject(cipher))
        }
    }

    @Command
    fun unlock(invoke: Invoke) {
        val args = invoke.parseArgs(UnlockArgs::class.java)
        val key = compositeKey(args.userId, args.vaultId) ?: run {
            invoke.reject("userId and vaultId are required")
            return
        }
        val activity = this.activity as? FragmentActivity ?: run {
            invoke.reject("biometric prompt requires FragmentActivity")
            return
        }

        val encoded = prefs.getString(key, null) ?: run {
            invoke.reject("not_enrolled")
            return
        }
        val ivLen = prefs.getInt(ivLenKey(key), 12)
        val blob = try {
            Base64.decode(encoded, Base64.NO_WRAP)
        } catch (e: Exception) {
            wipe(key)
            invoke.reject("corrupt_blob")
            return
        }
        if (blob.size <= ivLen) {
            wipe(key)
            invoke.reject("corrupt_blob")
            return
        }
        val iv = blob.copyOfRange(0, ivLen)
        val ciphertext = blob.copyOfRange(ivLen, blob.size)

        val secretKey = loadKey(keyAlias(key)) ?: run {
            wipe(key)
            invoke.reject("biometric_invalidated")
            return
        }
        val cipher = try {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.DECRYPT_MODE, secretKey, GCMParameterSpec(GCM_TAG_BITS, iv))
            c
        } catch (e: KeyPermanentlyInvalidatedException) {
            // User re-enrolled fingerprints / set up new biometric.
            wipe(key)
            invoke.reject("biometric_invalidated")
            return
        } catch (e: Exception) {
            wipe(key)
            invoke.reject("biometric_invalidated")
            return
        }

        val prompt = buildPrompt(activity, object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                val authedCipher = result.cryptoObject?.cipher ?: run {
                    invoke.reject("crypto object missing")
                    return
                }
                runCatching {
                    val plaintext = authedCipher.doFinal(ciphertext)
                    val obj = JSObject()
                    obj.put("secret", String(plaintext, Charsets.UTF_8))
                    invoke.resolve(obj)
                }.onFailure { e ->
                    Log.w(TAG, "unlock doFinal failed", e)
                    // Treat as invalidation: ciphertext can no longer be
                    // recovered through this key, so the user has to
                    // re-enrol with the master password.
                    wipe(key)
                    invoke.reject("biometric_invalidated")
                }
            }

            override fun onAuthenticationError(code: Int, msg: CharSequence) {
                invoke.reject("biometric error $code: $msg")
            }

            override fun onAuthenticationFailed() {
                // Per-attempt failure — let the prompt retry.
            }
        })
        activity.runOnUiThread {
            prompt.authenticate(promptInfo(args.reason ?: "Unlock vault with fingerprint"),
                BiometricPrompt.CryptoObject(cipher))
        }
    }

    @Command
    fun clear(invoke: Invoke) {
        val args = invoke.parseArgs(VaultIdArgs::class.java)
        val key = compositeKey(args.userId, args.vaultId) ?: run {
            invoke.reject("userId and vaultId are required")
            return
        }
        wipe(key)
        invoke.resolve()
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    private fun compositeKey(userId: String?, vaultId: String?): String? {
        val u = userId?.takeIf { it.isNotBlank() } ?: return null
        val v = vaultId?.takeIf { it.isNotBlank() } ?: return null
        return "$u.$v"
    }

    private fun keyAlias(compositeKey: String) = "$KEY_ALIAS_PREFIX$compositeKey"

    private fun ivLenKey(compositeKey: String) = "$compositeKey.iv_len"

    private fun reasonString(code: Int): String = when (code) {
        BiometricManager.BIOMETRIC_ERROR_NO_HARDWARE -> "no_hardware"
        BiometricManager.BIOMETRIC_ERROR_HW_UNAVAILABLE -> "hw_unavailable"
        BiometricManager.BIOMETRIC_ERROR_NONE_ENROLLED -> "none_enrolled"
        BiometricManager.BIOMETRIC_ERROR_SECURITY_UPDATE_REQUIRED -> "security_update_required"
        BiometricManager.BIOMETRIC_ERROR_UNSUPPORTED -> "unsupported"
        BiometricManager.BIOMETRIC_STATUS_UNKNOWN -> "status_unknown"
        else -> "error_$code"
    }

    private fun buildPrompt(
        activity: FragmentActivity,
        callback: BiometricPrompt.AuthenticationCallback,
    ): BiometricPrompt {
        val executor = ContextCompat.getMainExecutor(activity)
        return BiometricPrompt(activity, executor, callback)
    }

    private fun promptInfo(reason: String): BiometricPrompt.PromptInfo =
        BiometricPrompt.PromptInfo.Builder()
            .setTitle("Uncloud")
            .setSubtitle(reason)
            .setNegativeButtonText("Cancel")
            .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
            .build()

    private fun generateKey(alias: String): SecretKey {
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE_PROVIDER)
        val builder = KeyGenParameterSpec.Builder(
            alias,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setUserAuthenticationRequired(true)
            .setRandomizedEncryptionRequired(true)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            builder.setInvalidatedByBiometricEnrollment(true)
        }
        gen.init(builder.build())
        return gen.generateKey()
    }

    private fun keystoreContains(alias: String): Boolean = runCatching {
        KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }.containsAlias(alias)
    }.getOrDefault(false)

    private fun loadKey(alias: String): SecretKey? = runCatching {
        val ks = KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }
        ks.getKey(alias, null) as? SecretKey
    }.getOrNull()

    private fun deleteKey(alias: String) {
        runCatching {
            KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }.deleteEntry(alias)
        }
    }

    private fun wipe(compositeKey: String) {
        deleteKey(keyAlias(compositeKey))
        prefs.edit().remove(compositeKey).remove(ivLenKey(compositeKey)).apply()
    }
}
