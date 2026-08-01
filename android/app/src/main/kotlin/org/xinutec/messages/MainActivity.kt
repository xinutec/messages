package org.xinutec.messages

import android.webkit.WebView
import org.xinutec.shell.ShellConfig
import org.xinutec.shell.WebShellActivity

/**
 * The messages archive — the Signal + Google Chat viewer, an Angular app served at
 * [MESSAGES_URL], in the fleet's shared [WebShellActivity]. It's private (reachable
 * over the VPN) and behind a login; the WebView keeps the session cookie, so it's a
 * one-time sign-in.
 *
 * What the shell does not do, and this file does: "up" out of a conversation. A
 * thread reached by a cold launch has no in-app history behind it, so back would
 * leave the app from a page that visibly has a parent.
 */
class MainActivity : WebShellActivity() {
    override val shell = ShellConfig(url = MESSAGES_URL)

    // Set when a back press escapes a deep cold-start (a conversation opened with
    // no in-app history) up to the list: the list then replaces the thread as the
    // sole history entry, so the next back exits instead of bouncing into it.
    private var trimHistoryOnLoad = false

    override fun createWebViewClient() = MessagesWebViewClient()

    inner class MessagesWebViewClient : ShellWebViewClient() {
        override fun onPageFinished(view: WebView, url: String) {
            if (trimHistoryOnLoad) {
                // Escaped a deep cold-start to the list: drop the thread entry so
                // the list is the top of the stack.
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
    }
}
