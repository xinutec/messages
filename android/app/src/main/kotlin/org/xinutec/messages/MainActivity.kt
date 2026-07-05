package org.xinutec.messages

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Color
import android.os.Bundle
import android.view.ViewGroup
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.core.content.edit
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/**
 * A full-screen [WebView] onto the messages archive — the Angular app served at
 * [MESSAGES_URL]. No address bar, no tabs, a home-screen icon: the Signal + Google
 * Chat archive viewer presented as an app, avoiding browser chrome. It's private
 * (reachable over the VPN) and behind a login; the WebView keeps the session
 * cookie, so it's a one-time sign-in.
 *
 * Deliberately tiny — a [ComponentActivity] holding one WebView, no Compose or
 * AppCompat. `configChanges` keeps the WebView (and its route + scroll) across
 * rotation.
 *
 * The WebView is inset from the system bars by padding a wrapper (see [onCreate]),
 * and the strips behind the bars are painted with the page's own surface colour.
 */
class MainActivity : ComponentActivity() {
    private lateinit var web: WebView
    private lateinit var root: FrameLayout

    // Set when a back press escapes a deep cold-start (a conversation opened with
    // no in-app history) up to the list: the list then replaces the thread as the
    // sole history entry, so the next back exits instead of bouncing into it.
    private var trimHistoryOnLoad = false

    // Modern back handling (predictive back on API 33+, opted into via the
    // manifest's enableOnBackInvokedCallback). Enabled only while back has
    // somewhere in-app to go, so at the list root the system shows its own
    // predictive "exit to launcher" gesture instead.
    private val backCallback =
        object : OnBackPressedCallback(false) {
            override fun handleOnBackPressed() {
                when {
                    web.canGoBack() -> {
                        web.goBack()
                    }

                    inConversation() -> {
                        escapeToList()
                    }

                    else -> {
                        // Nothing left in-app: hand back to the system to finish.
                        isEnabled = false
                        onBackPressedDispatcher.onBackPressed()
                    }
                }
            }
        }

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val prefs = getSharedPreferences("viewer", Context.MODE_PRIVATE)
        web =
            WebView(this).apply {
                layoutParams =
                    ViewGroup.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.MATCH_PARENT,
                    )
                settings.javaScriptEnabled = true // Angular needs JS
                settings.domStorageEnabled = true // localStorage / sessionStorage
                settings.useWideViewPort = true
                settings.loadWithOverviewMode = true
                // Keep every navigation inside this WebView — never hand off to a
                // browser — and remember the current in-app page so a cold reopen
                // returns to it (SPA route changes fire doUpdateVisitedHistory too).
                webViewClient =
                    object : WebViewClient() {
                        override fun doUpdateVisitedHistory(
                            view: WebView,
                            url: String,
                            isReload: Boolean,
                        ) {
                            super.doUpdateVisitedHistory(view, url, isReload)
                            if (url.startsWith(MESSAGES_URL)) {
                                prefs.edit { putString(KEY_LAST_URL, url) }
                            }
                            syncBackEnabled()
                        }

                        override fun onPageFinished(view: WebView, url: String) {
                            super.onPageFinished(view, url)
                            if (trimHistoryOnLoad) {
                                // Escaped a deep cold-start to the list: drop the
                                // thread entry so the list is the top of the stack.
                                trimHistoryOnLoad = false
                                view.clearHistory()
                                syncBackEnabled()
                            }
                            // Paint the strips behind the system bars with the web
                            // UI's own surface colour instead of a hardcoded black;
                            // it follows the page's light/dark theme, so read its
                            // body background.
                            view.evaluateJavascript(
                                "getComputedStyle(document.body).backgroundColor",
                            ) { result -> parseCssColor(result)?.let(root::setBackgroundColor) }
                        }
                    }
                // Black until the page loads and reports its surface colour; avoids
                // a white flash on launch.
                setBackgroundColor(Color.BLACK)
            }
        // Inset the WebView from the system bars by padding a wrapper ViewGroup
        // (WebView.setPadding() doesn't offset content under wide-viewport mode).
        // Once the WebView no longer underlaps the bars its env(safe-area-inset-*)
        // collapse to 0, so the page's own safe-area CSS adds nothing on top.
        root =
            FrameLayout(this).apply {
                addView(web)
                setBackgroundColor(Color.BLACK)
            }
        ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            WindowInsetsCompat.CONSUMED
        }
        setContentView(root)

        onBackPressedDispatcher.addCallback(this, backCallback)
        // Reopen where we left off; the hardcoded URL is only the first-run default.
        web.loadUrl(prefs.getString(KEY_LAST_URL, null) ?: MESSAGES_URL)
    }

    /** Whether the WebView is currently showing a conversation thread. */
    private fun inConversation(): Boolean = web.url?.contains("/conversation/") == true

    /** Go up to the conversation list, collapsing the deep entry (see [trimHistoryOnLoad]). */
    private fun escapeToList() {
        trimHistoryOnLoad = true
        web.loadUrl(MESSAGES_URL)
    }

    /** Intercept back only while it has somewhere in-app to go; otherwise let the
     *  system handle it (finish / predictive exit). */
    private fun syncBackEnabled() {
        backCallback.isEnabled = web.canGoBack() || inConversation()
    }

    // evaluateJavascript hands back the JSON-encoded result, e.g. the string
    // "rgb(18, 18, 18)" (with quotes) or "rgba(18, 18, 18, 1)". Pull out the RGB
    // triple; alpha is ignored (the surface is opaque). null if it can't be read.
    private fun parseCssColor(raw: String?): Int? {
        val m = raw?.let { Regex("""rgba?\((\d+),\s*(\d+),\s*(\d+)""").find(it) } ?: return null
        val (r, g, b) = m.destructured
        return Color.rgb(r.toInt(), g.toInt(), b.toInt())
    }

    companion object {
        // The messages archive viewer (HTTPS, VPN-only, behind a login).
        private const val MESSAGES_URL = "https://messages.xinutec.org/"
        private const val KEY_LAST_URL = "last_url"
    }
}
