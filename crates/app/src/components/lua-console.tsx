import * as React from "react";
import { ArrowDownToLine, Check, Copy, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/store/app-store";
import type { LuaConsoleLine, LuaRunState } from "@/store/app-store";
import type { LuaSource } from "@/types";

type LuaConsoleProps = {
  gameId: string;
  source: LuaSource;
  slug: string;
};

const EMPTY_RUN: LuaRunState = {
  running: false,
  lastError: null,
  startedAt: null,
  lines: [],
};

/** Hard render cap. Above this we slice the tail so the DOM doesn't get
 * thousands of nodes — CodeMirror is already paying that cost. The store
 * keeps up to 10k buffered, so the user loses *visibility* of older lines
 * (they could Copy all to recover them) but never loses data mid-buffer. */
const VISIBLE_CAP = 500;

/** How close to the bottom (px) counts as "stuck to bottom." When the user
 * scrolls back up past this, auto-scroll turns OFF; when they scroll back
 * within this distance, it turns back ON. */
const STICKY_THRESHOLD_PX = 50;

const LEVEL_DOT: Record<LuaConsoleLine["level"], string> = {
  info: "bg-[oklch(0.62_0.16_240)]",
  warn: "bg-[oklch(0.78_0.16_82)]",
  error: "bg-[oklch(0.62_0.20_25)]",
};

const LEVEL_TEXT: Record<LuaConsoleLine["level"], string> = {
  info: "text-[var(--color-foreground)]",
  warn: "text-[var(--color-foreground)]",
  error: "text-[oklch(0.55_0.22_25)] dark:text-[oklch(0.78_0.20_25)]",
};

function scriptKey(gameId: string, source: LuaSource, slug: string): string {
  return `${gameId}:${source}:${slug}`;
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

function pad3(n: number): string {
  if (n < 10) return `00${n}`;
  if (n < 100) return `0${n}`;
  return String(n);
}

/** Format `timestampMs` relative to `startedAt` as `HH:MM:SS.mmm`. When
 * startedAt is null (lines arrived without a status event for some reason),
 * falls back to wall-clock HH:MM:SS so the column never goes blank. */
function formatRelative(timestampMs: number, startedAt: number | null): string {
  if (startedAt === null) {
    const d = new Date(timestampMs);
    return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
  }
  const delta = Math.max(0, timestampMs - startedAt);
  const ms = delta % 1000;
  const totalSeconds = Math.floor(delta / 1000);
  const s = totalSeconds % 60;
  const totalMinutes = Math.floor(totalSeconds / 60);
  const m = totalMinutes % 60;
  const h = Math.floor(totalMinutes / 60);
  return `${pad2(h)}:${pad2(m)}:${pad2(s)}.${pad3(ms)}`;
}

function StatusPill({ run }: { run: LuaRunState }) {
  if (run.running) {
    return (
      <span className="inline-flex items-center gap-1.5 rounded-full border border-[var(--color-border)] bg-[color-mix(in_oklch,oklch(0.62_0.16_240)_10%,transparent)] px-2 py-[1px] text-[10px] uppercase tracking-[0.16em] text-[var(--color-foreground)]">
        <span
          className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-[oklch(0.62_0.16_240)]"
          aria-hidden
        />
        Running
      </span>
    );
  }
  if (run.lastError) {
    return (
      <span className="inline-flex items-center gap-1.5 rounded-full border border-[var(--color-border)] bg-[color-mix(in_oklch,oklch(0.62_0.20_25)_10%,transparent)] px-2 py-[1px] text-[10px] uppercase tracking-[0.16em] text-[var(--color-foreground)]">
        <span
          className="inline-block h-1.5 w-1.5 rounded-full bg-[oklch(0.62_0.20_25)]"
          aria-hidden
        />
        Error
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full border border-[var(--color-border)] px-2 py-[1px] text-[10px] uppercase tracking-[0.16em] text-[var(--color-muted-foreground)]">
      <span
        className="inline-block h-1.5 w-1.5 rounded-full bg-[var(--color-muted-foreground)] opacity-60"
        aria-hidden
      />
      Idle
    </span>
  );
}

export function LuaConsole({ gameId, source, slug }: LuaConsoleProps) {
  const key = scriptKey(gameId, source, slug);
  const run = useAppStore((s) => s.luaRunByScript[key]) ?? EMPTY_RUN;
  const clearLuaOutput = useAppStore((s) => s.clearLuaOutput);

  const [autoScroll, setAutoScroll] = React.useState(true);
  const [copied, setCopied] = React.useState(false);

  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  // Track the latest autoScroll value inside the scroll handler without
  // re-binding the listener on every toggle.
  const autoScrollRef = React.useRef(autoScroll);
  React.useEffect(() => {
    autoScrollRef.current = autoScroll;
  }, [autoScroll]);

  // Slice the tail for rendering. The store buffer can be 10k; the DOM gets
  // at most VISIBLE_CAP. Memoized so unrelated store updates don't re-slice.
  const visibleLines = React.useMemo(() => {
    if (run.lines.length <= VISIBLE_CAP) return run.lines;
    return run.lines.slice(-VISIBLE_CAP);
  }, [run.lines]);
  const droppedCount = run.lines.length - visibleLines.length;

  // Auto-scroll to bottom whenever new lines arrive — but only if the user
  // hasn't manually scrolled up.
  React.useLayoutEffect(() => {
    if (!autoScroll) return;
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [visibleLines, autoScroll]);

  // Toggle auto-scroll based on the user's scroll position.
  const handleScroll = React.useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const distanceFromBottom =
      el.scrollHeight - el.scrollTop - el.clientHeight;
    const stuckToBottom = distanceFromBottom <= STICKY_THRESHOLD_PX;
    if (stuckToBottom !== autoScrollRef.current) {
      autoScrollRef.current = stuckToBottom;
      setAutoScroll(stuckToBottom);
    }
  }, []);

  const handleCopy = React.useCallback(async () => {
    if (run.lines.length === 0) return;
    const text = run.lines
      .map(
        (l) =>
          `[${formatRelative(l.timestampMs, run.startedAt)}] ${l.level.toUpperCase()} ${l.message}`,
      )
      .join("\n");
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard may be denied in restricted webviews; silent. */
    }
  }, [run.lines, run.startedAt]);

  const handleClear = React.useCallback(() => {
    clearLuaOutput(key);
  }, [clearLuaOutput, key]);

  const handleJumpToBottom = React.useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    setAutoScroll(true);
  }, []);

  return (
    <div
      data-slot="lua-console"
      className="flex min-h-0 flex-col border-t border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)]"
    >
      <ConsoleHeader
        run={run}
        autoScroll={autoScroll}
        onToggleAutoScroll={() => {
          if (autoScroll) {
            setAutoScroll(false);
          } else {
            handleJumpToBottom();
          }
        }}
        onCopy={() => void handleCopy()}
        onClear={handleClear}
        copied={copied}
        droppedCount={droppedCount}
      />

      <div
        ref={scrollRef}
        onScroll={handleScroll}
        className={cn(
          "min-h-0 flex-1 overflow-y-auto px-3 py-2",
          "font-mono text-[11.5px] leading-[1.55]",
        )}
      >
        {run.lines.length === 0 ? (
          <EmptyConsole />
        ) : (
          <>
            {droppedCount > 0 ? (
              <div className="mb-1 select-none text-[10px] uppercase tracking-[0.18em] text-[var(--color-muted-foreground)]">
                — {droppedCount.toLocaleString()} earlier line
                {droppedCount === 1 ? "" : "s"} hidden (use Copy all to recover) —
              </div>
            ) : null}
            <ul className="allow-select flex flex-col">
              {visibleLines.map((line, idx) => (
                <ConsoleRow
                  key={`${droppedCount + idx}-${line.timestampMs}`}
                  line={line}
                  startedAt={run.startedAt}
                />
              ))}
            </ul>
            {run.lastError ? (
              <div className="mt-2 rounded-[var(--radius)] border border-[color-mix(in_oklch,oklch(0.62_0.20_25)_45%,var(--color-border))] bg-[color-mix(in_oklch,oklch(0.62_0.20_25)_8%,transparent)] px-2.5 py-1.5 text-[11px] leading-snug">
                <span className="mr-1.5 text-[9.5px] uppercase tracking-[0.18em] opacity-70">
                  Script error
                </span>
                <span className="allow-select text-[oklch(0.55_0.22_25)] dark:text-[oklch(0.82_0.20_25)]">
                  {run.lastError}
                </span>
              </div>
            ) : null}
          </>
        )}
      </div>
    </div>
  );
}

function ConsoleHeader({
  run,
  autoScroll,
  onToggleAutoScroll,
  onCopy,
  onClear,
  copied,
  droppedCount,
}: {
  run: LuaRunState;
  autoScroll: boolean;
  onToggleAutoScroll: () => void;
  onCopy: () => void;
  onClear: () => void;
  copied: boolean;
  droppedCount: number;
}) {
  const total = run.lines.length;
  return (
    <div className="flex shrink-0 items-center gap-2 border-b border-[var(--color-border)] px-3 py-1.5">
      <span className="text-[10px] uppercase tracking-[0.18em] text-[var(--color-muted-foreground)]">
        Console
      </span>
      <StatusPill run={run} />
      <span className="tabular text-[10.5px] text-[var(--color-muted-foreground)]">
        {total.toLocaleString()} line{total === 1 ? "" : "s"}
        {droppedCount > 0 ? (
          <span className="opacity-60"> ({VISIBLE_CAP} shown)</span>
        ) : null}
      </span>
      <div className="ml-auto flex items-center gap-1">
        <Button
          size="sm"
          variant="ghost"
          className={cn(
            "h-6 gap-1 px-1.5 text-[11px] font-normal text-[var(--color-muted-foreground)]",
            "hover:text-[var(--color-foreground)]",
            autoScroll && "text-[var(--color-foreground)]",
          )}
          onClick={onToggleAutoScroll}
          title={
            autoScroll
              ? "Auto-scroll on (click to pause)"
              : "Auto-scroll paused (click to follow tail)"
          }
        >
          <ArrowDownToLine
            className={cn(
              "h-3 w-3",
              autoScroll ? "opacity-100" : "opacity-50",
            )}
            strokeWidth={2.25}
          />
          Follow
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="h-6 gap-1 px-1.5 text-[11px] font-normal text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)]"
          onClick={onCopy}
          disabled={total === 0}
          title={copied ? "Copied" : "Copy all output"}
        >
          {copied ? (
            <Check className="h-3 w-3" strokeWidth={2.5} />
          ) : (
            <Copy className="h-3 w-3" strokeWidth={2} />
          )}
          {copied ? "Copied" : "Copy"}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="h-6 gap-1 px-1.5 text-[11px] font-normal text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)]"
          onClick={onClear}
          disabled={total === 0 && !run.lastError}
          title="Clear console"
        >
          <Trash2 className="h-3 w-3" strokeWidth={2} />
          Clear
        </Button>
      </div>
    </div>
  );
}

function ConsoleRow({
  line,
  startedAt,
}: {
  line: LuaConsoleLine;
  startedAt: number | null;
}) {
  return (
    <li className="grid grid-cols-[auto_auto_1fr] items-baseline gap-x-2 px-1 py-[1px] hover:bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)]">
      <span className="tabular shrink-0 select-none text-[10.5px] text-[var(--color-muted-foreground)] opacity-70">
        {formatRelative(line.timestampMs, startedAt)}
      </span>
      <span
        aria-label={line.level}
        className={cn(
          "mt-[5px] h-1.5 w-1.5 shrink-0 rounded-full",
          LEVEL_DOT[line.level],
        )}
      />
      <span
        className={cn(
          "whitespace-pre-wrap break-words",
          LEVEL_TEXT[line.level],
        )}
      >
        {line.message}
      </span>
    </li>
  );
}

function EmptyConsole() {
  return (
    <div className="flex h-full min-h-[80px] items-center justify-center">
      <p className="text-center text-[11.5px] leading-relaxed text-[var(--color-muted-foreground)]">
        No output yet. Run a script to see{" "}
        <code className="rounded-[var(--radius-sm)] bg-[color-mix(in_oklch,var(--color-foreground)_8%,transparent)] px-1 py-[1px] font-mono text-[10.5px]">
          print()
        </code>{" "}
        output here.
      </p>
    </div>
  );
}

export default LuaConsole;
