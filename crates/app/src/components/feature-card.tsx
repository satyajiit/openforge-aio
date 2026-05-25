import { useEffect, useState } from "react";
import {
  Check,
  Compass,
  Copy,
  Keyboard,
  Loader2,
  RefreshCw,
  Send,
  Snowflake,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { KeybindDialog } from "@/components/keybind-dialog";
import { resolveFeatureIcon } from "@/lib/feature-icon";
import { tierColor } from "@/lib/tier-colors";
import { cn } from "@/lib/utils";
import type { FreezeRuntime } from "@/store/app-store";
import type {
  ControlSpec,
  FeatureMeta,
  FeatureResolution,
  Value,
  ValueKind,
} from "@/types";

type ControlKind = "switch" | "input" | "button";

export function FeatureCard({
  gameId,
  feature,
  current,
  resolution,
  statusText,
  freezeRuntime,
  freezeOn,
  keybind,
  disabled,
  onWrite,
  onFreeze,
  onCodePatch,
  onRetry,
}: {
  gameId: string;
  feature: FeatureMeta;
  current: Value | undefined;
  resolution: FeatureResolution | undefined;
  /** Optional rich status string (e.g. "20 / 30 unlocked") rendered
   * under the tagline. Populated for SetProgressTags-class features. */
  statusText?: string | null;
  /** Per-feature freeze-loop health. Present only while the loop is
   * running; absent means freeze is OFF (or the feature isn't a freeze). */
  freezeRuntime?: FreezeRuntime;
  /** Lifted "is freeze toggled ON" state. The store is the source of
   * truth so a global-hotkey toggle (which never goes through the
   * React click handler) can drive the SwitchControl. */
  freezeOn?: boolean;
  /** Chord string ("F5", "CommandOrControl+Shift+G") if this feature
   * has a global hotkey assigned. Shown as a kbd badge inline with the
   * Assign affordance. */
  keybind?: string;
  disabled: boolean;
  onWrite: (value: Value) => Promise<void> | void;
  onFreeze: (frozen: boolean) => Promise<void> | void;
  onCodePatch: (applied: boolean) => Promise<void> | void;
  onRetry: () => Promise<void> | void;
}) {
  const controlKind = pickControlKind(feature);
  // Resolution undefined => never attempted yet (pre-attach); treat as
  // "neutral / disabled by attach state". Defined + !ok => unresolved
  // (likely scene-dependent), show Retry instead of the normal control.
  const unresolved = resolution !== undefined && !resolution.ok;

  return (
    <CardShell data-strategy={feature.strategy} data-unresolved={unresolved}>
      <FeatureGlyph feature={feature} muted={unresolved} />

      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <h3 className="truncate text-[14px] font-semibold leading-tight tracking-tight">
          {feature.displayName}
        </h3>
        {unresolved ? (
          <UnresolvedHint resolution={resolution} />
        ) : feature.tagline ? (
          <p className="text-[12px] leading-snug text-[var(--color-muted-foreground)] text-pretty">
            {feature.tagline}
          </p>
        ) : null}
        {/* Rich status line: shows for any feature whose backend
            overrides `Feature::status_text`. Currently used by
            "Unlock All Skills" to surface "20 / 30 unlocked" + the
            "All unlocked — skill bricks no longer needed" completion
            message. Hidden when statusText is null/undefined. */}
        {!unresolved && statusText ? (
          <p className="text-[11.5px] font-medium leading-snug text-[var(--color-foreground)]/80 text-pretty">
            {statusText}
          </p>
        ) : null}
        {!unresolved && controlKind === "input" ? (
          <CurrentValueLine current={current} />
        ) : null}
      </div>

      <div className="ml-auto flex shrink-0 items-center gap-2">
        <FreezeRuntimeBadge runtime={freezeRuntime} />
        {!unresolved && controlKind === "switch" ? (
          <KeybindAffordance
            gameId={gameId}
            feature={feature}
            keybind={keybind}
          />
        ) : null}
        {unresolved ? (
          <RetryControl disabled={disabled} onRetry={onRetry} />
        ) : controlKind === "switch" ? (
          <SwitchControl
            feature={feature}
            current={current}
            freezeOn={freezeOn}
            disabled={disabled}
            onCodePatch={onCodePatch}
            onWrite={onWrite}
            onFreeze={onFreeze}
          />
        ) : controlKind === "button" ? (
          <ButtonControl
            feature={feature}
            disabled={disabled}
            onWrite={onWrite}
          />
        ) : (
          <InputControl
            feature={feature}
            current={current}
            disabled={disabled}
            isFreeze={feature.strategy === "freeze"}
            onWrite={onWrite}
            onFreeze={onFreeze}
          />
        )}
      </div>
    </CardShell>
  );
}

/** Small clickable affordance shown to the right of switchable
 *  features. Renders as either a keyboard-icon button (no binding) or
 *  a kbd-style chord badge (binding present). Opens the KeybindDialog
 *  for assign / re-bind / clear. */
function KeybindAffordance({
  gameId,
  feature,
  keybind,
}: {
  gameId: string;
  feature: FeatureMeta;
  keybind?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Tooltip>
        <TooltipTrigger asChild>
          {keybind ? (
            <button
              type="button"
              onClick={() => setOpen(true)}
              className={cn(
                "tabular inline-flex items-center gap-1 rounded border",
                "border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_6%,transparent)]",
                "px-1.5 py-[2px] text-[10.5px] font-semibold text-[var(--color-foreground)]",
                "transition-colors hover:border-[var(--color-foreground)]/40",
              )}
              aria-label={`Hotkey: ${keybind}. Click to change.`}
            >
              <Keyboard className="h-3 w-3" strokeWidth={2.25} />
              {keybind}
            </button>
          ) : (
            <button
              type="button"
              onClick={() => setOpen(true)}
              className={cn(
                "inline-grid h-6 w-6 place-items-center rounded border",
                "border-[var(--color-border)]/70 text-[var(--color-muted-foreground)]",
                "transition-colors hover:border-[var(--color-foreground)]/40 hover:text-[var(--color-foreground)]",
              )}
              aria-label="Assign hotkey"
            >
              <Keyboard className="h-3 w-3" strokeWidth={2.25} />
            </button>
          )}
        </TooltipTrigger>
        <TooltipContent side="top">
          {keybind ? `Change or clear binding (${keybind})` : "Assign hotkey"}
        </TooltipContent>
      </Tooltip>
      <KeybindDialog
        open={open}
        onOpenChange={setOpen}
        gameId={gameId}
        featureId={feature.id}
        featureDisplayName={feature.displayName}
      />
    </>
  );
}

function UnresolvedHint({
  resolution,
}: {
  resolution: FeatureResolution | undefined;
}) {
  // Pending = transient (live UE5 object not loaded yet — e.g. game in main
  // menu). Backend auto-retries every ~3s; the user doesn't need to do
  // anything. Failed = permanent (TOML mismatch, missing class on this
  // build); user might Retry once they've changed something, but it
  // probably won't help on its own.
  const isPending = resolution?.status === "pending";
  return (
    <p className="text-[12px] leading-snug text-[var(--color-muted-foreground)] text-pretty">
      {isPending ? (
        <>
          <span className="mr-1">⏳</span>
          Waiting for the game world to load. Start a level — this will
          activate on its own.
        </>
      ) : (
        <>
          Not currently in memory. Open the relevant in-game screen, then
          Retry.
          {resolution?.error ? (
            <span className="ml-1 text-[10.5px] opacity-60">
              ({summarizeError(resolution.error)})
            </span>
          ) : null}
        </>
      )}
    </p>
  );
}

function summarizeError(err: string): string {
  // Trim the boilerplate prefix the runtime adds so the hint stays readable.
  const m = err.match(/heap_scan: (.+)$/);
  if (m && m[1]) return m[1].slice(0, 80);
  return err.slice(0, 80);
}

/** Small badge next to the freeze switch showing the loop's live health.
 *  Only renders for "waiting" state — we silently hide it when active so
 *  successful freezes don't clutter the card. Hover for the hint text. */
function FreezeRuntimeBadge({
  runtime,
}: {
  runtime?: FreezeRuntime;
}) {
  if (!runtime || runtime.state !== "waiting") return null;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          aria-label="Runtime is waiting for the live target to come back"
          className="inline-flex items-center gap-1 rounded-full border border-[var(--color-border)] bg-[var(--color-muted)] px-2 py-0.5 text-[10.5px] font-medium text-[var(--color-muted-foreground)]"
        >
          <Loader2 className="size-3 animate-spin" aria-hidden />
          waiting
        </span>
      </TooltipTrigger>
      <TooltipContent side="left">
        {runtime.hint ?? "Loop is retrying; resumes automatically."}
      </TooltipContent>
    </Tooltip>
  );
}

function RetryControl({
  disabled,
  onRetry,
}: {
  disabled: boolean;
  onRetry: () => Promise<void> | void;
}) {
  const [busy, setBusy] = useState(false);
  const handle = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await onRetry();
    } finally {
      setBusy(false);
    }
  };
  return (
    <Button
      size="sm"
      variant="outline"
      disabled={disabled || busy}
      onClick={() => void handle()}
      className="gap-1.5"
    >
      <RefreshCw
        className={cn("h-3.5 w-3.5", busy && "animate-spin")}
        strokeWidth={2.25}
      />
      {busy ? "Retrying..." : "Retry"}
    </Button>
  );
}

function CardShell({
  className,
  children,
  ...rest
}: React.HTMLAttributes<HTMLDivElement>) {
  // Card body stays pure monochrome. The only colored element on
  // each card is the glyph chip in FeatureGlyph — a quiet, single-
  // spot accent that reads the same in light and dark mode.
  return (
    <div
      {...rest}
      data-slot="feature-card"
      className={cn(
        "glass group/feature relative flex items-center gap-4 overflow-hidden",
        "rounded-[calc(var(--radius)+3px)] px-4 py-3.5",
        "transition-all duration-200",
        "hover:border-[var(--color-foreground)]/25 hover:-translate-y-[0.5px]",
        className,
      )}
    >
      {children}
    </div>
  );
}

function FeatureGlyph({
  feature,
  muted,
}: {
  feature: FeatureMeta;
  muted?: boolean;
}) {
  const Icon = resolveFeatureIcon(feature.icon, feature.tier);
  // Color lives ONLY on the icon stroke. The chip background and
  // border stay monochrome so the system reads as minimalist. Source
  // of truth: the signature TOML's `[meta] icon_color` field, which
  // contributors set per-feature; tier-derived hue is a fallback for
  // signatures that don't bother to pick one.
  const tint = feature.iconColor ?? tierColor(feature.tier).fg;
  return (
    <div
      aria-hidden
      className={cn(
        "relative grid h-11 w-11 shrink-0 place-items-center rounded-[calc(var(--radius)+1px)]",
        "border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-card)_75%,transparent)]",
        "transition-colors group-hover/feature:border-[var(--color-foreground)]/35",
        muted && "opacity-60",
      )}
    >
      <Icon
        className="h-5 w-5"
        style={{
          color: muted ? "var(--color-muted-foreground)" : tint,
        }}
        strokeWidth={1.75}
      />
    </div>
  );
}

function CurrentValueLine({ current }: { current: Value | undefined }) {
  return (
    <div className="flex items-baseline gap-2 text-[11px]">
      <span className="uppercase tracking-[0.14em] text-[var(--color-muted-foreground)]">
        current
      </span>
      <span className="tabular font-medium text-[var(--color-foreground)]">
        {formatValue(current)}
      </span>
    </div>
  );
}

function SwitchControl({
  feature,
  current,
  freezeOn,
  disabled,
  onCodePatch,
  onWrite,
  onFreeze,
}: {
  feature: FeatureMeta;
  current: Value | undefined;
  /** Lifted state — see FeatureCard prop comment. Source of truth for
   * the Switch's `checked` prop on freeze-strategy features. */
  freezeOn?: boolean;
  disabled: boolean;
  onCodePatch: (applied: boolean) => Promise<void> | void;
  onWrite: (value: Value) => Promise<void> | void;
  onFreeze: (frozen: boolean) => Promise<void> | void;
}) {
  if (feature.strategy === "freeze") {
    const frozen = freezeOn ?? false;
    const handleToggle = async (b: boolean) => {
      if (b) {
        // Numeric currencies (Lock Studs etc.) prime to range.max so the
        // freeze loop locks at the ceiling rather than at the live value.
        // Bool features (Lock Health) skip the prime — the freeze loop's
        // first tick (≤ interval_ms) writes the TOML-declared freeze_value
        // immediately, and `1` would deserialize as integer not boolean,
        // tripping the backend's strict Value::Bool deserializer.
        if (feature.kind !== "bool") {
          const max = feature.range.max ?? 1;
          await onWrite({ kind: feature.kind, v: max } as Value);
        }
        await onFreeze(true);
      } else {
        await onFreeze(false);
      }
    };
    return (
      <Switch
        disabled={disabled}
        checked={frozen}
        onCheckedChange={(b) => void handleToggle(b)}
        aria-label={`Toggle ${feature.displayName}`}
      />
    );
  }

  // code_patch / bool strategies: checked state reflects the read value.
  const applied = isApplied(current);
  return (
    <Switch
      disabled={disabled}
      checked={applied}
      onCheckedChange={(b) => void onCodePatch(b)}
      aria-label={`Toggle ${feature.displayName}`}
    />
  );
}

function InputControl({
  feature,
  current,
  disabled,
  isFreeze,
  onWrite,
  onFreeze,
}: {
  feature: FeatureMeta;
  current: Value | undefined;
  disabled: boolean;
  isFreeze: boolean;
  onWrite: (value: Value) => Promise<void> | void;
  onFreeze: (frozen: boolean) => Promise<void> | void;
}) {
  const [draft, setDraft] = useState<string>(formatValue(current));
  const [frozen, setFrozen] = useState(false);
  const presets = readPresets(feature.control);
  // A "discrete selector" is an Input control whose presets carry explicit
  // labels (the labeled `{ label, value }` form, not bare numbers). In that
  // mode the feature is meant to be picked from the fixed set — no free
  // numeric entry, no separate Apply step. Each button click writes its
  // value and auto-engages the freeze toggle so the choice persists; the
  // freeze toggle becomes the feature's on/off switch (off restores stock).
  const isDiscrete = isDiscreteSelector(feature.control);

  useEffect(() => {
    setDraft(formatValue(current));
  }, [current?.kind, valueScalar(current)]);

  const handleApply = async () => {
    const parsed = parseValue(feature.kind, draft);
    if (parsed === null) return;
    await onWrite(parsed);
  };

  const handlePresetClick = async (presetValue: number) => {
    setDraft(String(presetValue));
    if (!isDiscrete) return;
    const parsed = parseValue(feature.kind, String(presetValue));
    if (parsed === null) return;
    await onWrite(parsed);
    if (isFreeze && !frozen) {
      setFrozen(true);
      await onFreeze(true);
    }
  };

  const currentScalar = valueScalar(current);
  const currentNumber = typeof currentScalar === "number" ? currentScalar : undefined;

  return (
    <div className="flex items-center gap-2">
      {presets.length > 0 ? (
        <div className="flex items-center gap-1">
          {presets.map((p) => {
            const active =
              isDiscrete && currentNumber !== undefined && Math.abs(currentNumber - p.value) < 1e-6;
            return (
              <button
                key={`${p.label}-${p.value}`}
                type="button"
                disabled={disabled}
                onClick={() => void handlePresetClick(p.value)}
                title={p.label !== formatPreset(p.value) ? `${p.value}` : undefined}
                className={cn(
                  "rounded-full border px-2 py-[3px] text-[10px] tabular font-medium transition-colors",
                  "disabled:cursor-not-allowed disabled:opacity-50",
                  active
                    ? "border-[var(--color-foreground)] bg-[var(--color-foreground)]/10 text-[var(--color-foreground)]"
                    : "border-[var(--color-border)] text-[var(--color-muted-foreground)] hover:border-[var(--color-foreground)]/40 hover:text-[var(--color-foreground)]",
                )}
              >
                {p.label}
              </button>
            );
          })}
        </div>
      ) : null}
      {!isDiscrete ? (
        <>
          <Label className="sr-only" htmlFor={`feat-${feature.id}`}>
            New value
          </Label>
          <Input
            id={`feat-${feature.id}`}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            disabled={disabled}
            className="w-32 tabular"
            inputMode={feature.kind.startsWith("f") ? "decimal" : "numeric"}
          />
          <Button
            size="sm"
            disabled={disabled}
            onClick={() => void handleApply()}
            className="gap-1.5"
          >
            <Check className="h-3.5 w-3.5" strokeWidth={2.5} />
            Apply
          </Button>
        </>
      ) : null}
      {isFreeze ? (
        <div
          className={cn(
            "ml-1 flex items-center gap-1.5 rounded-full border border-[var(--color-border)] px-2 py-1 text-[11px]",
            frozen ? "bg-[var(--color-foreground)]/5" : "",
          )}
        >
          <Snowflake
            className={cn(
              "h-3 w-3 transition-colors",
              frozen
                ? "text-[var(--color-foreground)]"
                : "text-[var(--color-muted-foreground)]",
            )}
            strokeWidth={2.25}
          />
          <Switch
            checked={frozen}
            disabled={disabled}
            onCheckedChange={(b) => {
              setFrozen(b);
              void onFreeze(b);
            }}
            aria-label="Freeze"
            className="scale-90"
          />
        </div>
      ) : null}
    </div>
  );
}

/** One-shot action button. Single click fires `onWrite(Bool(true))`.
 *  No off state — used by `SetProgressTags` features whose effect can't
 *  be reversed once applied (Unlock All Skills/Outfits/Fast Travel). */
function ButtonControl({
  feature,
  disabled,
  onWrite,
}: {
  feature: FeatureMeta;
  disabled: boolean;
  onWrite: (value: Value) => Promise<void> | void;
}) {
  const label =
    feature.control?.kind === "button" && feature.control.label
      ? feature.control.label
      : "Apply";
  const [busy, setBusy] = useState(false);
  const handleClick = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await onWrite({ kind: "bool", v: true });
    } finally {
      setBusy(false);
    }
  };
  return (
    <Button
      size="sm"
      disabled={disabled || busy}
      onClick={() => void handleClick()}
      className="gap-1.5"
      aria-label={label}
    >
      <Check className="h-3.5 w-3.5" strokeWidth={2.5} />
      {busy ? "Applying…" : label}
    </Button>
  );
}

function isDiscreteSelector(control: ControlSpec | null): boolean {
  if (control?.kind !== "input") return false;
  return (control.presets ?? []).some((p) => typeof p !== "number");
}

function pickControlKind(feature: FeatureMeta): ControlKind {
  if (feature.control) {
    if (feature.control.kind === "switch") return "switch";
    if (feature.control.kind === "input") return "input";
    if (feature.control.kind === "button") return "button";
  }
  if (feature.strategy === "code_patch") return "switch";
  if (feature.kind === "bool") return "switch";
  return "input";
}

type ResolvedPreset = { label: string; value: number };

function readPresets(control: ControlSpec | null): ResolvedPreset[] {
  if (control?.kind === "input" && Array.isArray(control.presets)) {
    return control.presets.map((p) =>
      typeof p === "number"
        ? { label: formatPreset(p), value: p }
        : { label: p.label, value: p.value },
    );
  }
  return [];
}

function formatPreset(n: number): string {
  if (Math.abs(n) >= 1_000_000) return `${(n / 1_000_000).toFixed(0)}M`;
  if (Math.abs(n) >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return String(n);
}

function isApplied(v: Value | undefined): boolean {
  if (!v) return false;
  if (v.kind === "bool") return v.v;
  return v.v !== 0;
}

function valueScalar(v: Value | undefined): number | boolean | undefined {
  if (!v) return undefined;
  return v.v;
}

function formatValue(v: Value | undefined): string {
  if (!v) return "—";
  if (v.kind === "bool") return v.v ? "true" : "false";
  return String(v.v);
}

function parseValue(kind: ValueKind, raw: string): Value | null {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return null;
  if (kind === "bool") {
    if (trimmed === "true" || trimmed === "1") return { kind: "bool", v: true };
    if (trimmed === "false" || trimmed === "0")
      return { kind: "bool", v: false };
    return null;
  }
  if (kind === "f32" || kind === "f64") {
    const n = Number.parseFloat(trimmed);
    if (Number.isNaN(n)) return null;
    return { kind, v: n };
  }
  const n = Number.parseInt(trimmed, 10);
  if (Number.isNaN(n)) return null;
  return { kind, v: n };
}

export function CoordinateTeleportCard({
  features,
  values,
  resolutions,
  disabled,
  onWrite,
  onRetry,
}: {
  features: FeatureMeta[];
  values: Record<string, Value | undefined>;
  resolutions: Record<string, FeatureResolution | undefined>;
  disabled: boolean;
  onWrite: (featureId: string, value: Value) => Promise<void> | void;
  onRetry: (featureId: string) => Promise<void> | void;
}) {
  const xFeat = features.find((f) => f.id === "teleport_x");
  const yFeat = features.find((f) => f.id === "teleport_y");
  const zFeat = features.find((f) => f.id === "teleport_z");

  const xVal = values["teleport_x"];
  const yVal = values["teleport_y"];
  const zVal = values["teleport_z"];

  const xRes = resolutions["teleport_x"];
  const yRes = resolutions["teleport_y"];
  const zRes = resolutions["teleport_z"];

  const unresolved =
    (xRes && !xRes.ok) || (yRes && !yRes.ok) || (zRes && !zRes.ok);
  const isPending =
    xRes?.status === "pending" ||
    yRes?.status === "pending" ||
    zRes?.status === "pending";

  const [xDraft, setXDraft] = useState("");
  const [yDraft, setYDraft] = useState("");
  const [zDraft, setZDraft] = useState("");

  useEffect(() => {
    if (xVal !== undefined) setXDraft(formatValue(xVal));
  }, [xVal?.kind, valueScalar(xVal)]);

  useEffect(() => {
    if (yVal !== undefined) setYDraft(formatValue(yVal));
  }, [yVal?.kind, valueScalar(yVal)]);

  useEffect(() => {
    if (zVal !== undefined) setZDraft(formatValue(zVal));
  }, [zVal?.kind, valueScalar(zVal)]);

  const handleCapture = () => {
    if (xVal !== undefined) setXDraft(formatValue(xVal));
    if (yVal !== undefined) setYDraft(formatValue(yVal));
    if (zVal !== undefined) setZDraft(formatValue(zVal));
  };

  const handleTeleport = async () => {
    if (!xFeat || !yFeat || !zFeat) return;
    const px = parseValue(xFeat.kind, xDraft);
    const py = parseValue(yFeat.kind, yDraft);
    const pz = parseValue(zFeat.kind, zDraft);

    if (px === null || py === null || pz === null) return;

    await onWrite("teleport_x", px);
    await onWrite("teleport_y", py);
    await onWrite("teleport_z", pz);
  };

  const handleRetryAll = async () => {
    if (xRes && !xRes.ok) await onRetry("teleport_x");
    if (yRes && !yRes.ok) await onRetry("teleport_y");
    if (zRes && !zRes.ok) await onRetry("teleport_z");
  };

  return (
    <CardShell className="flex-col items-stretch gap-3.5 py-4">
      <div className="flex items-center gap-4">
        <div className="relative grid h-11 w-11 shrink-0 place-items-center rounded-[calc(var(--radius)+1px)] border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-card)_75%,transparent)] group-hover/feature:border-[var(--color-foreground)]/35">
          <Compass
            className="h-5 w-5"
            style={{ color: tierColor("teleportation").fg }}
            strokeWidth={1.75}
          />
        </div>

        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <div className="flex items-center gap-1.5">
            <h3 className="truncate text-[14px] font-semibold leading-tight tracking-tight">
              Manual Coordinate Teleport
            </h3>
          </div>
          <p className="text-[12px] leading-snug text-[var(--color-muted-foreground)]">
            Manually enter X, Y, Z coordinate values to instantly teleport Batman.
          </p>
          {!unresolved &&
          (xVal !== undefined || yVal !== undefined || zVal !== undefined) ? (
            <div className="flex items-center gap-2 text-[11.5px] font-medium text-[var(--color-foreground)]/80">
              <span>Current Position:</span>
              <span className="tabular font-semibold text-[var(--color-foreground)] bg-[var(--color-foreground)]/5 px-1.5 py-0.5 rounded border border-[var(--color-border)]/50">
                {xVal ? parseFloat(formatValue(xVal)).toFixed(0) : "—"},{" "}
                {yVal ? parseFloat(formatValue(yVal)).toFixed(0) : "—"},{" "}
                {zVal ? parseFloat(formatValue(zVal)).toFixed(0) : "—"}
              </span>
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    disabled={disabled}
                    onClick={handleCapture}
                    className="ml-1 inline-grid p-1 hover:bg-[var(--color-foreground)]/5 rounded text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)] transition-colors"
                  >
                    <Copy className="h-3.5 w-3.5" />
                  </button>
                </TooltipTrigger>
                <TooltipContent side="top">
                  Capture Current Coordinates
                </TooltipContent>
              </Tooltip>
            </div>
          ) : null}
        </div>
      </div>

      {unresolved ? (
        <div className="flex items-center justify-between border-t border-[var(--color-border)]/50 pt-3">
          <p className="text-[12px] leading-snug text-[var(--color-muted-foreground)]">
            {isPending ? (
              <>
                <span className="mr-1">⏳</span>
                Waiting for the game world to load. Start a level to resolve coordinates.
              </>
            ) : (
              <>Not currently in memory. Open the game, then click Retry.</>
            )}
          </p>
          <RetryControl disabled={disabled} onRetry={handleRetryAll} />
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-3 border-t border-[var(--color-border)]/50 pt-3">
          <div className="flex flex-1 items-center gap-2 min-w-[240px]">
            <div className="flex flex-1 items-center gap-1.5">
              <span className="text-[10px] uppercase font-bold tracking-wider text-[var(--color-muted-foreground)] w-3 text-right">
                X
              </span>
              <Input
                value={xDraft}
                onChange={(e) => setXDraft(e.target.value)}
                disabled={disabled}
                placeholder="X"
                className="h-8 text-xs tabular px-2"
              />
            </div>
            <div className="flex flex-1 items-center gap-1.5">
              <span className="text-[10px] uppercase font-bold tracking-wider text-[var(--color-muted-foreground)] w-3 text-right">
                Y
              </span>
              <Input
                value={yDraft}
                onChange={(e) => setYDraft(e.target.value)}
                disabled={disabled}
                placeholder="Y"
                className="h-8 text-xs tabular px-2"
              />
            </div>
            <div className="flex flex-1 items-center gap-1.5">
              <span className="text-[10px] uppercase font-bold tracking-wider text-[var(--color-muted-foreground)] w-3 text-right">
                Z
              </span>
              <Input
                value={zDraft}
                onChange={(e) => setZDraft(e.target.value)}
                disabled={disabled}
                placeholder="Z"
                className="h-8 text-xs tabular px-2"
              />
            </div>
          </div>
          <Button
            size="sm"
            disabled={disabled}
            onClick={() => void handleTeleport()}
            className="gap-1.5 px-4 h-8 shrink-0 ml-auto"
          >
            <Send className="h-3.5 w-3.5" />
            Teleport
          </Button>
        </div>
      )}
    </CardShell>
  );
}
