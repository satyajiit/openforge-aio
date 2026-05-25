import { useEffect } from "react";

// Keys blocked in production to discourage casual inspection of the running
// trainer. These are not security guarantees — anyone determined can attach a
// debugger to the process — but they keep the surface tidy for end-users.
const BLOCKED_KEYS = new Set([
  "F5", // refresh
  "F7", // caret browsing
  "F11", // browser fullscreen (the app has its own window chrome)
  "F12", // devtools
]);

function isInspectShortcut(e: KeyboardEvent): boolean {
  const k = e.key.toLowerCase();
  const ctrlOrMeta = e.ctrlKey || e.metaKey;

  // Ctrl+Shift+I / J / C  → devtools / console / element-picker
  if (ctrlOrMeta && e.shiftKey && (k === "i" || k === "j" || k === "c")) {
    return true;
  }
  // Ctrl+U → view source
  if (ctrlOrMeta && !e.shiftKey && k === "u") return true;
  // Ctrl+S → save page
  if (ctrlOrMeta && !e.shiftKey && k === "s") return true;
  // Ctrl+P → print
  if (ctrlOrMeta && !e.shiftKey && k === "p") return true;
  // Ctrl+R / Ctrl+Shift+R → reload
  if (ctrlOrMeta && k === "r") return true;
  return false;
}

export function useProductionHardening() {
  useEffect(() => {
    const isProd = import.meta.env.PROD;

    const onContextMenu = (e: MouseEvent) => {
      const target = e.target as HTMLElement | null;
      // Allow native context menus only on text-entry surfaces so users can
      // paste paths/values into inputs.
      if (
        target &&
        (target.tagName === "INPUT" || target.tagName === "TEXTAREA")
      ) {
        return;
      }
      e.preventDefault();
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (!isProd) return;
      if (BLOCKED_KEYS.has(e.key) || isInspectShortcut(e)) {
        e.preventDefault();
        e.stopPropagation();
      }
    };

    const onSelectStart = (e: Event) => {
      const target = e.target as HTMLElement | null;
      if (!target) return;
      // Allow native selection inside inputs, textareas, and explicitly opted-in
      // regions (code blocks, copy-paste cells).
      if (
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.isContentEditable ||
        target.closest("[data-allow-select='true'], .allow-select, pre, code")
      ) {
        return;
      }
      e.preventDefault();
    };

    const onDragStart = (e: DragEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && target.tagName === "IMG") {
        e.preventDefault();
      }
    };

    document.addEventListener("contextmenu", onContextMenu);
    document.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("selectstart", onSelectStart);
    document.addEventListener("dragstart", onDragStart);

    return () => {
      document.removeEventListener("contextmenu", onContextMenu);
      document.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("selectstart", onSelectStart);
      document.removeEventListener("dragstart", onDragStart);
    };
  }, []);
}
