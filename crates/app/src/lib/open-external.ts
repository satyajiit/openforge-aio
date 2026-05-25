import { open as openInShell } from "@tauri-apps/plugin-shell";

/**
 * Launch a URL in the user's default browser.
 *
 * In a Tauri 2 WebView2 the bare `window.open(url, "_blank")` call does NOT
 * spawn the system browser — the link silently fails because the embedded
 * webview has no concept of "new tab." We have to go through the shell plugin
 * (gated by the `shell:allow-open` capability) which calls the OS's URL
 * handler. The browser fallback is only there for Vite dev runs outside
 * Tauri; in production the plugin path is always available.
 */
export async function openExternal(url: string): Promise<void> {
  try {
    await openInShell(url);
  } catch (err) {
    // Vite-only dev run (no Tauri runtime) — fall back to native open.
    if (typeof window !== "undefined" && "open" in window) {
      window.open(url, "_blank", "noopener,noreferrer");
      return;
    }
    throw err;
  }
}
