package org.xinutec.messages

import android.webkit.WebView
import org.xinutec.shell.ShellConfig
import org.xinutec.shell.WebShellActivity

/**
 * The messages archive viewer at [MESSAGES_URL], in the fleet's [WebShellActivity].
 * The WebView keeps the session cookie, so sign-in is once.
 *
 * Adds "up" out of a conversation: a thread reached by a cold launch has no
 * in-app history, so back would otherwise leave the app.
 */
class MainActivity : WebShellActivity() {
    override val shell =
        ShellConfig(
            url = MESSAGES_URL,
            // The app and the Nextcloud login hop; anything else opens in the
            // browser.
            allowedHosts = setOf("messages.xinutec.org", NC_HOST),
        )

    // Set when back escapes a cold-started thread to the list, so the list
    // replaces the thread in history and the next back exits.
    private var trimHistoryOnLoad = false

    override fun createWebViewClient() = MessagesWebViewClient()

    inner class MessagesWebViewClient : ShellWebViewClient() {
        override fun onPageFinished(view: WebView, url: String) {
            if (trimHistoryOnLoad) {
                trimHistoryOnLoad = false
                view.clearHistory()
                syncBack()
            }
            super.onPageFinished(view, url)
        }
    }

    /** Once in-app history is exhausted, a conversation still has somewhere to go. */
    override fun onBackAtRoot(): Boolean {
        if (!inConversation()) return false
        escapeToList()
        return true
    }

    override fun hasExtraBackTargets(): Boolean = inConversation()

    /** Whether the WebView is currently showing a conversation thread. */
    private fun inConversation(): Boolean = web.url?.contains("/conversation/") == true

    /** Go up to the conversation list, collapsing the deep entry (see [trimHistoryOnLoad]). */
    private fun escapeToList() {
        trimHistoryOnLoad = true
        web.loadUrl(MESSAGES_URL)
    }

    companion object {
        // The messages archive viewer (HTTPS, VPN-only, behind a login).
        private const val MESSAGES_URL = "https://messages.xinutec.org/"

        // The Nextcloud identity provider the login bounces through.
        private const val NC_HOST = "dash.xinutec.org"
    }
}
