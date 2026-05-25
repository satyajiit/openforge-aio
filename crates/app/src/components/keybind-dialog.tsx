import * as React from "react";
import { Check, Keyboard, Trash2 } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { ipc } from "@/lib/ipc";
import { useAppStore } from "@/store/app-store";
import { cn } from "@/lib/utils";
import type { SetKeybindResult } from "@/types";

/**
 * Build a Tauri accelerator string from a KeyboardEvent. Returns `null`
 * when the press is a bare modifier (Ctrl alone, Shift alone, …) or a
 * non-bindable key (Escape — used by Dialog to cancel).
 */
function chordFromEvent(e: React.KeyboardEvent): string | null {
  const code = e.code;
  // Bare modifier press → keep waiting for the non-modifier key.
  if (
    code === "ControlLeft" ||
    code === "ControlRight" ||
    code === "ShiftLeft" ||
    code === "ShiftRight" ||
    code === "AltLeft" ||
    code === "AltRight" ||
    code === "MetaLeft" ||
    code === "MetaRight" ||
    code === "OSLeft" ||
    code === "OSRight" ||
    code === "Escape"
  ) {
    return null;
  }

  const parts: string[] = [];
  if (e.ctrlKey || e.metaKey) parts.push("CommandOrControl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");

  let keyPart: string | null = null;
  if (code.startsWith("Key") && code.length === 4) {
    // "KeyA" → "A"
    keyPart = code.slice(3);
  } else if (code.startsWith("Digit") && code.length === 6) {
    // "Digit1" → "1"
    keyPart = code.slice(5);
  } else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) {
    keyPart = code;
  } else {
    // Friendly names for the common non-alphanumeric keys.
    const map: Record<string, string> = {
      ArrowUp: "Up",
      ArrowDown: "Down",
      ArrowLeft: "Left",
      ArrowRight: "Right",
      Space: "Space",
      Enter: "Enter",
      Tab: "Tab",
      Backspace: "Backspace",
      Delete: "Delete",
      Insert: "Insert",
      Home: "Home",
      End: "End",
      PageUp: "PageUp",
      PageDown: "PageDown",
      Minus: "-",
      Equal: "=",
      BracketLeft: "[",
      BracketRight: "]",
      Backslash: "\\",
      Semicolon: ";",
      Quote: "'",
      Comma: ",",
      Period: ".",
      Slash: "/",
      Backquote: "`",
      NumpadAdd: "NumpadAdd",
      NumpadSubtract: "NumpadSubtract",
      NumpadMultiply: "NumpadMultiply",
      NumpadDivide: "NumpadDivide",
      NumpadEnter: "NumpadEnter",
      NumpadDecimal: "NumpadDecimal",
    };
    keyPart = map[code] ?? null;
  }

  if (!keyPart) return null;
  parts.push(keyPart);
  return parts.join("+");
}

type DialogPhase =
  | { kind: "listening" }
  | {
      kind: "conflict";
      chord: string;
      existingDisplayName: string;
    }
  | { kind: "saving" }
  | { kind: "success"; chord: string }
  | { kind: "error"; message: string };

export function KeybindDialog({
  open,
  onOpenChange,
  gameId,
  featureId,
  featureDisplayName,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  gameId: string;
  featureId: string;
  featureDisplayName: string;
}) {
  const currentChord = useAppStore(
    (s) => s.keybindsByGame[gameId]?.[featureId],
  );
  const setKeybindLocal = useAppStore((s) => s.setKeybind);

  const [phase, setPhase] = React.useState<DialogPhase>({ kind: "listening" });

  // Reset to "listening" each time the dialog opens so a re-open after
  // success-auto-close doesn't briefly flash the old success state.
  React.useEffect(() => {
    if (open) setPhase({ kind: "listening" });
  }, [open]);

  // Auto-close after the success state has rendered for 3s.
  React.useEffect(() => {
    if (phase.kind !== "success") return;
    const t = setTimeout(() => onOpenChange(false), 3000);
    return () => clearTimeout(t);
  }, [phase, onOpenChange]);

  const installChord = React.useCallback(
    async (chord: string, overrideConflict: boolean) => {
      setPhase({ kind: "saving" });
      try {
        const res: SetKeybindResult = await ipc.setKeybind(
          gameId,
          featureId,
          chord,
          overrideConflict,
        );
        if (res.kind === "ok") {
          setKeybindLocal(gameId, featureId, res.chord);
          setPhase({ kind: "success", chord: res.chord });
        } else if (res.kind === "conflict") {
          setPhase({
            kind: "conflict",
            chord: res.chord,
            existingDisplayName: res.existingDisplayName,
          });
        } else if (res.kind === "osRegisterFailed") {
          // The chord is technically valid but the OS won't give it
          // to us (most commonly: plain F12 is owned by WebView2's
          // devtools accelerator, even in release builds; some chords
          // are also grabbed by other running apps). Surface as the
          // error phase with a chord-specific hint.
          setPhase({
            kind: "error",
            message: `${res.chord} is already claimed by another app or the OS. Try a different combination (e.g. add Ctrl or Shift).`,
          });
        } else {
          setPhase({ kind: "error", message: res.reason });
        }
      } catch (e) {
        const err = e as { message?: string };
        setPhase({
          kind: "error",
          message: err.message ?? String(e),
        });
      }
    },
    [gameId, featureId, setKeybindLocal],
  );

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (phase.kind !== "listening") return;
    // Dialog already handles Escape via Radix; let it bubble.
    if (e.key === "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    const chord = chordFromEvent(e);
    if (chord) void installChord(chord, false);
  };

  const handleClearBinding = async () => {
    try {
      await ipc.clearKeybind(gameId, featureId);
      setKeybindLocal(gameId, featureId, null);
      onOpenChange(false);
    } catch (e) {
      const err = e as { message?: string };
      setPhase({
        kind: "error",
        message: err.message ?? String(e),
      });
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="max-w-sm gap-3 outline-none"
        onKeyDown={handleKeyDown}
        // Auto-focus the content so keystrokes land here, not on a
        // background input.
        autoFocus
        tabIndex={-1}
      >
        <DialogHeader className="space-y-1">
          <DialogTitle className="flex items-center gap-2 text-[15px]">
            <Keyboard className="h-4 w-4" strokeWidth={2} />
            Assign hotkey
          </DialogTitle>
          <DialogDescription className="text-[12px]">
            <span className="font-medium text-[var(--color-foreground)]">
              {featureDisplayName}
            </span>
            {currentChord ? (
              <>
                {" "}— currently bound to{" "}
                <ChordBadge chord={currentChord} />
              </>
            ) : null}
          </DialogDescription>
        </DialogHeader>

        <PhaseBody
          phase={phase}
          onOverride={(chord) => void installChord(chord, true)}
          onCancel={() => setPhase({ kind: "listening" })}
          featureDisplayName={featureDisplayName}
        />

        {currentChord && phase.kind === "listening" ? (
          <div className="flex items-center justify-between border-t border-[var(--color-border)]/60 pt-3">
            <Button
              variant="ghost"
              size="sm"
              className="gap-1.5 text-[12px] text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)]"
              onClick={() => void handleClearBinding()}
            >
              <Trash2 className="h-3.5 w-3.5" strokeWidth={2} />
              Clear binding
            </Button>
            <span className="text-[10.5px] uppercase tracking-[0.16em] text-[var(--color-muted-foreground)]">
              Press a key to replace
            </span>
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function PhaseBody({
  phase,
  onOverride,
  onCancel,
  featureDisplayName,
}: {
  phase: DialogPhase;
  onOverride: (chord: string) => void;
  onCancel: () => void;
  featureDisplayName: string;
}) {
  if (phase.kind === "listening") {
    return (
      <div
        className={cn(
          "flex min-h-[88px] flex-col items-center justify-center rounded-[var(--radius)]",
          "border border-dashed border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)]",
          "px-4 py-5 text-center",
        )}
      >
        <p className="text-[12.5px] text-[var(--color-muted-foreground)]">
          Press the key combination you'd like to bind to{" "}
          <span className="font-medium text-[var(--color-foreground)]">
            {featureDisplayName}
          </span>
          .
        </p>
        <p className="mt-1.5 text-[10.5px] uppercase tracking-[0.16em] text-[var(--color-muted-foreground)]">
          Esc to cancel
        </p>
      </div>
    );
  }
  if (phase.kind === "saving") {
    return (
      <div className="flex min-h-[88px] items-center justify-center text-[12.5px] text-[var(--color-muted-foreground)]">
        Saving…
      </div>
    );
  }
  if (phase.kind === "conflict") {
    return (
      <div className="flex flex-col gap-3">
        <div
          className={cn(
            "rounded-[var(--radius)] border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)]",
            "px-3 py-2.5 text-[12.5px] leading-snug",
          )}
        >
          <ChordBadge chord={phase.chord} /> is already mapped to{" "}
          <span className="font-medium text-[var(--color-foreground)]">
            {phase.existingDisplayName}
          </span>
          . Override the existing binding?
        </div>
        <div className="flex items-center justify-end gap-2">
          <Button variant="ghost" size="sm" className="text-[12px]" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            size="sm"
            className="gap-1.5 text-[12px]"
            onClick={() => onOverride(phase.chord)}
          >
            <Check className="h-3.5 w-3.5" strokeWidth={2.25} />
            Override
          </Button>
        </div>
      </div>
    );
  }
  if (phase.kind === "success") {
    return (
      <div className="flex min-h-[88px] flex-col items-center justify-center gap-2 text-center">
        <Check
          className="h-6 w-6 text-[var(--color-foreground)]"
          strokeWidth={2}
        />
        <p className="text-[13px] font-medium">
          Bound to <ChordBadge chord={phase.chord} />
        </p>
        <p className="text-[10.5px] uppercase tracking-[0.16em] text-[var(--color-muted-foreground)]">
          Closing in a moment
        </p>
      </div>
    );
  }
  // error
  return (
    <div className="flex flex-col gap-3">
      <div
        className={cn(
          "rounded-[var(--radius)] border border-[var(--color-border)] bg-[color-mix(in_oklch,red_8%,transparent)]",
          "px-3 py-2.5 text-[12.5px] leading-snug text-[var(--color-foreground)]",
        )}
      >
        Couldn't bind that key: {phase.message}
      </div>
      <div className="flex items-center justify-end">
        <Button variant="ghost" size="sm" className="text-[12px]" onClick={onCancel}>
          Try again
        </Button>
      </div>
    </div>
  );
}

function ChordBadge({ chord }: { chord: string }) {
  return (
    <kbd
      className={cn(
        "tabular inline-flex items-center rounded border border-[var(--color-border)]",
        "bg-[color-mix(in_oklch,var(--color-foreground)_8%,transparent)]",
        "px-1.5 py-[1px] text-[10.5px] font-semibold text-[var(--color-foreground)]",
      )}
    >
      {chord}
    </kbd>
  );
}
