package dev.todotxt.desktop

import android.content.Context
import android.os.Bundle
import android.os.Environment
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Edge-to-edge is forced on recent Android: keep the page out of the status / navigation bars and
    // lift it above the on-screen keyboard (so the input bar stays visible while typing).
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { v, insets ->
      val b = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.ime())
      v.setPadding(b.left, b.top, b.right, b.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    webView.addJavascriptInterface(StorageBridge(this), "TodoAndroid")
  }
}

/** Storage locations for the todo files (used by web/app.js as `window.TodoAndroid`). */
class StorageBridge(private val ctx: Context) {
  /**
   * JSON array of `{id, label, path, removable}`: the private app storage, the app folder on the shared
   * ("device") storage and the app folder on every mounted SD card. No permission needed for any of them.
   */
  @JavascriptInterface
  fun storageDirs(): String {
    val out = JSONArray()
    fun add(id: String, label: String, path: String, removable: Boolean) {
      out.put(JSONObject().put("id", id).put("label", label).put("path", path).put("removable", removable))
    }
    add("app", "App storage (private)", ctx.filesDir.absolutePath, false)
    val dirs = ctx.getExternalFilesDirs(null)
    var sd = 0
    for ((i, f) in dirs.withIndex()) {
      if (f == null) continue
      if (Environment.getExternalStorageState(f) != Environment.MEDIA_MOUNTED) continue
      f.mkdirs()
      val removable = try { Environment.isExternalStorageRemovable(f) } catch (e: Exception) { i > 0 }
      if (removable) {
        sd++
        add("sd$sd", if (sd == 1) "SD card" else "SD card $sd", f.absolutePath, true)
      } else {
        add("device", "Device storage (visible to file managers)", f.absolutePath, false)
      }
    }
    return out.toString()
  }
}
