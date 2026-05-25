/**
 * Hotkey activation sounds. Three flavors — toggle ON (rising), toggle
 * OFF (falling), one-shot fire (single ping) — surface auditory
 * feedback when the user can't see the trainer window (game is
 * focused). Replace the .ogg files in /public/sounds/ to retune.
 *
 * Strategy: lazy singleton <Audio> per file, kept warm. Restarting an
 * in-flight play just resets `currentTime` to 0 so rapid retriggers
 * (user mashing F5) don't queue.
 */

export type HotkeySoundKind = "on" | "off" | "fire";

const SOURCES: Record<HotkeySoundKind, string> = {
  on: "/sounds/hotkey-on.mp3",
  off: "/sounds/hotkey-off.mp3",
  fire: "/sounds/hotkey-fire.mp3",
};

const cache = new Map<HotkeySoundKind, HTMLAudioElement>();

function getOrCreate(kind: HotkeySoundKind): HTMLAudioElement | null {
  if (typeof window === "undefined" || typeof Audio === "undefined") return null;
  let el = cache.get(kind);
  if (!el) {
    el = new Audio(SOURCES[kind]);
    el.preload = "auto";
    el.volume = 0.5;
    cache.set(kind, el);
  }
  return el;
}

export function playHotkeySound(kind: HotkeySoundKind): void {
  const el = getOrCreate(kind);
  if (!el) return;
  try {
    el.currentTime = 0;
  } catch {
    // Some browsers throw if the element isn't loaded yet; ignore.
  }
  // play() returns a Promise that rejects on autoplay block. We swallow
  // because the user is actively pressing keys — that satisfies the
  // gesture requirement in practice and the first call will succeed.
  void el.play().catch(() => {});
}
