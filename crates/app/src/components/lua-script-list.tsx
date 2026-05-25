import * as React from "react";
import { Trash2, FileCode2 } from "lucide-react";

import { cn } from "@/lib/utils";
import type { LuaScript } from "@/types";

export type LuaScriptListProps = {
  scripts: LuaScript[];
  selectedSlug: string | null;
  onSelect: (slug: string) => void;
  onDelete?: (slug: string) => void;
  emptyMessage: React.ReactNode;
};

/**
 * Human-readable "time ago" string. Buckets are coarse on purpose — the
 * sidebar is a glance, not a log. Falls back to a YYYY-MM-DD date once
 * the script is older than a week.
 */
function formatRelative(unix: number | null): string {
  if (unix === null || unix === undefined) return "—";
  const nowSec = Math.floor(Date.now() / 1000);
  const delta = nowSec - unix;
  if (delta < 30) return "just now";
  if (delta < 60) return `${delta}s ago`;
  if (delta < 60 * 60) return `${Math.floor(delta / 60)}m ago`;
  if (delta < 60 * 60 * 24) return `${Math.floor(delta / 3600)}h ago`;
  if (delta < 60 * 60 * 24 * 2) return "yesterday";
  if (delta < 60 * 60 * 24 * 7) return `${Math.floor(delta / 86400)}d ago`;
  const d = new Date(unix * 1000);
  const yyyy = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${yyyy}-${mm}-${dd}`;
}

export function LuaScriptList({
  scripts,
  selectedSlug,
  onSelect,
  onDelete,
  emptyMessage,
}: LuaScriptListProps) {
  if (scripts.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 px-4 py-10 text-center">
        <span className="grid h-9 w-9 place-items-center rounded-full bg-[color-mix(in_oklch,var(--color-foreground)_6%,transparent)] text-[var(--color-muted-foreground)]">
          <FileCode2 className="h-4 w-4" strokeWidth={2} />
        </span>
        <div className="text-[12px] leading-relaxed text-[var(--color-muted-foreground)]">
          {emptyMessage}
        </div>
      </div>
    );
  }

  return (
    <ul className="flex flex-col gap-0.5 px-1 py-1">
      {scripts.map((s) => {
        const isSelected = selectedSlug === s.slug;
        const sub =
          s.source === "user"
            ? formatRelative(s.modifiedUnixSecs)
            : s.author
              ? `by ${s.author}`
              : s.installed
                ? "installed"
                : "available";
        return (
          <li key={s.slug} className="group/script relative">
            <button
              type="button"
              onClick={() => onSelect(s.slug)}
              className={cn(
                "flex w-full flex-col items-start gap-0.5 rounded-[var(--radius)] px-2.5 py-2 text-left",
                "transition-colors outline-none",
                "focus-visible:ring-1 focus-visible:ring-[var(--color-ring)]",
                isSelected
                  ? "bg-[color-mix(in_oklch,var(--color-foreground)_8%,transparent)] text-[var(--color-foreground)]"
                  : "text-[var(--color-foreground)] hover:bg-[color-mix(in_oklch,var(--color-foreground)_4%,transparent)]",
              )}
            >
              <span className="flex w-full items-center gap-2 pr-6">
                <span className="truncate text-[13px] font-medium leading-tight">
                  {s.name}
                </span>
                {s.source === "community" && !s.installed ? (
                  <span className="ml-auto rounded-full border border-[var(--color-border)] px-1.5 py-[1px] text-[9px] uppercase tracking-[0.16em] text-[var(--color-muted-foreground)]">
                    not installed
                  </span>
                ) : null}
              </span>
              <span className="truncate text-[10.5px] text-[var(--color-muted-foreground)]">
                {sub}
              </span>
            </button>
            {onDelete ? (
              <button
                type="button"
                aria-label={`Delete ${s.name}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onDelete(s.slug);
                }}
                className={cn(
                  "absolute right-1.5 top-1.5 grid h-6 w-6 place-items-center rounded-[var(--radius)]",
                  "text-[var(--color-muted-foreground)] opacity-0 transition-opacity",
                  "hover:bg-[color-mix(in_oklch,var(--color-foreground)_8%,transparent)] hover:text-[var(--color-foreground)]",
                  "group-hover/script:opacity-100 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--color-ring)]",
                )}
              >
                <Trash2 className="h-3.5 w-3.5" strokeWidth={2} />
              </button>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}
