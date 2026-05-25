import * as React from "react";
import {
  AlertTriangle,
  Check,
  ExternalLink,
  EyeOff,
  Github,
  Globe,
  HeartHandshake,
  Lock,
  Network,
  Package,
  ShieldCheck,
  Sparkles,
  WifiOff,
  X,
  Youtube,
} from "lucide-react";
// `X` retained — used by the comparison table to mark closed-source rows.

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { BrandMark } from "@/components/brand-mark";
import { cn } from "@/lib/utils";
import { openExternal } from "@/lib/open-external";

const REPO_URL = "https://github.com/satyajiit/openforge-aio";
const YOUTUBE_URL = "https://youtube.com/@GamesPatch";
const WEBSITE_URL = "https://theappstack.in";
const LICENSE_URL = `${REPO_URL}/blob/main/LICENSE`;
const APP_VERSION = "0.1.0";

interface PromiseItem {
  icon: React.ElementType;
  title: string;
  body: string;
}

const PROMISES: PromiseItem[] = [
  {
    icon: ShieldCheck,
    title: "Source you can read",
    body: "Every line of OpenForge is public on GitHub — frontend, backend, the injected DLLs, the discovery CLI. You can audit, fork, or fingerprint a build before you trust it.",
  },
  {
    icon: WifiOff,
    title: "Local-only by design",
    body: "OpenForge never phones home. No telemetry, no usage pings, no cloud sync. Your script ideas and game profiles stay on your disk.",
  },
  {
    icon: Package,
    title: "No ad-supported installers",
    body: "Free download, free license, no bundled toolbars, no opt-out modal soup. The whole project is MIT.",
  },
  {
    icon: Lock,
    title: "Strict CSP + ACL",
    body: "Tauri 2's deny-by-default permission system is enabled. Only the IPC commands the trainer actually needs are unlocked — everything else is a hard rejection.",
  },
  {
    icon: EyeOff,
    title: "No inspector in release",
    body: "Production builds ship without DevTools. F12, view-source, and friends are blocked so a casual onlooker can't reverse the running session.",
  },
  {
    icon: Network,
    title: "Single-player scope only",
    body: "We never ship cheats for online or anti-cheat-protected titles. The architecture itself is offline-first; no remote dispatch, no obfuscation layer.",
  },
];

interface ComparisonRow {
  topic: string;
  closedNote: string;
  closedBad?: boolean;
  openNote: string;
}

const COMPARISON: ComparisonRow[] = [
  {
    topic: "Source code",
    closedNote: "Closed — you ship the binary your AV happens to scan",
    closedBad: true,
    openNote: "Public on GitHub — audit before you run",
  },
  {
    topic: "Networking",
    closedNote: "Often phones home, tracks usage, syncs profiles",
    closedBad: true,
    openNote: "Local-only. No outbound calls.",
  },
  {
    topic: "Updates",
    closedNote: "Mystery updater you trust by default",
    closedBad: true,
    openNote: "GitHub releases with a checksum + signed tag",
  },
  {
    topic: "Bundled extras",
    closedNote: "Sometimes ads, browser toolbars, miners — yes, really",
    closedBad: true,
    openNote: "MIT, no extras, no upsell modals",
  },
  {
    topic: "Anti-cheat handling",
    closedNote: "Vague. Some explicitly attempt evasion.",
    closedBad: true,
    openNote: "Anti-cheat games are out of scope by policy",
  },
  {
    topic: "Cheat authoring",
    closedNote: "Hidden — you can't see or change what runs",
    closedBad: true,
    openNote: "TOML signatures + Rust crates, fork-friendly",
  },
];

export function AboutDialog({ trigger }: { trigger: React.ReactNode }) {
  return (
    <Dialog>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent className="max-w-2xl max-h-[88vh] overflow-y-auto gap-0 p-0">
        <Hero />
        <div className="flex flex-col gap-6 p-5">
          <Why />
          <PromiseGrid />
          <Comparison />
          <Footer />
        </div>
      </DialogContent>
    </Dialog>
  );
}

function Hero() {
  return (
    <div
      data-slot="about-hero"
      className={cn(
        "relative overflow-hidden p-6",
        "border-b border-[var(--color-border)]",
        "bg-[color-mix(in_oklch,var(--color-foreground)_4%,transparent)]",
      )}
    >
      <div
        aria-hidden
        className="pointer-events-none absolute -right-24 -top-24 h-64 w-64 rounded-full blur-3xl"
        style={{
          background:
            "radial-gradient(closest-side, color-mix(in oklch, var(--color-foreground) 9%, transparent), transparent 70%)",
        }}
      />
      <div className="relative flex items-start gap-4">
        <div
          className={cn(
            "grid h-14 w-14 shrink-0 place-items-center rounded-[12px]",
            "bg-[var(--color-foreground)] text-[var(--color-background)]",
            "shadow-[0_8px_26px_-12px_color-mix(in_oklch,var(--color-foreground)_60%,transparent)]",
            "ring-1 ring-[var(--color-foreground)]/15",
          )}
        >
          <BrandMark className="h-7 w-7" />
        </div>
        <div className="flex flex-1 flex-col gap-2 pr-8">
          <DialogHeader className="space-y-1.5">
            <DialogTitle className="text-[17px] leading-tight">
              About OpenForge
            </DialogTitle>
            <DialogDescription className="text-[12px] leading-relaxed">
              An open-source, all-in-one trainer for offline single-player PC
              games. Built so you never have to wonder what's running on your
              machine.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-wrap items-center gap-1.5 pt-1">
            <Pill>v{APP_VERSION}</Pill>
            <Pill>MIT licensed</Pill>
            <Pill>100% open source</Pill>
            <Pill>Local-only</Pill>
          </div>
        </div>
      </div>
    </div>
  );
}

function Pill({ children }: { children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center rounded-full border border-[var(--color-border)] bg-[var(--color-background)]/40 px-2 py-0.5 text-[9.5px] uppercase tracking-[0.18em] text-[var(--color-muted-foreground)]">
      {children}
    </span>
  );
}

function Why() {
  return (
    <section data-slot="about-why" className="flex flex-col gap-3">
      <h3 className="flex items-center gap-2 text-[13px] font-semibold tracking-tight">
        <Sparkles className="h-3.5 w-3.5" strokeWidth={2.25} />
        Why this project exists
      </h3>
      <div className="space-y-2.5 text-[12.5px] leading-relaxed text-[var(--color-muted-foreground)]">
        <p>
          Most game trainers are closed-source binaries from anonymous
          authors. They ask for administrator privileges, inject DLLs into
          your processes, and run code that nobody outside their team has
          read. Some are clean. Some bundle ads or miners. Some have been
          confirmed trojans. The honest answer is: <em>you can't tell</em>.
        </p>
        <p>
          OpenForge is the version of that tool where the answer is on
          GitHub. The frontend, the backend, the per-game DLLs, every
          signature — all of it. If something looks wrong, you can read it,
          file an issue, or build your own version from the same source
          before it touches your system.
        </p>
      </div>
    </section>
  );
}

function PromiseGrid() {
  return (
    <section data-slot="about-promises" className="flex flex-col gap-3">
      <h3 className="flex items-center gap-2 text-[13px] font-semibold tracking-tight">
        <ShieldCheck className="h-3.5 w-3.5" strokeWidth={2.25} />
        What that means in practice
      </h3>
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        {PROMISES.map(({ icon: Icon, title, body }) => (
          <div
            key={title}
            className={cn(
              "flex flex-col gap-1.5 rounded-[var(--radius)] border border-[var(--color-border)]",
              "bg-[color-mix(in_oklch,var(--color-card)_55%,transparent)]",
              "p-3",
            )}
          >
            <div className="flex items-center gap-2">
              <span className="grid h-6 w-6 place-items-center rounded-[var(--radius)] bg-[var(--color-foreground)]/8 text-[var(--color-foreground)]">
                <Icon className="h-3 w-3" strokeWidth={2.25} />
              </span>
              <span className="text-[12px] font-semibold leading-none">
                {title}
              </span>
            </div>
            <p className="text-[11px] leading-relaxed text-[var(--color-muted-foreground)]">
              {body}
            </p>
          </div>
        ))}
      </div>
    </section>
  );
}

function Comparison() {
  return (
    <section data-slot="about-compare" className="flex flex-col gap-3">
      <h3 className="flex items-center gap-2 text-[13px] font-semibold tracking-tight">
        <AlertTriangle className="h-3.5 w-3.5" strokeWidth={2.25} />
        OpenForge vs typical closed-source trainers
      </h3>
      <div className="grid grid-cols-1 gap-px overflow-hidden rounded-[var(--radius)] border border-[var(--color-border)] bg-[var(--color-border)] sm:grid-cols-[140px_1fr_1fr]">
        <HeaderCell label="Topic" />
        <HeaderCell label="Closed-source trainers" tone="warning" />
        <HeaderCell label="OpenForge" tone="ok" />
        {COMPARISON.map((row) => (
          <RowCells key={row.topic} row={row} />
        ))}
      </div>
    </section>
  );
}

function HeaderCell({
  label,
  tone,
}: {
  label: string;
  tone?: "ok" | "warning";
}) {
  return (
    <div
      className={cn(
        "bg-[var(--color-card)] px-3 py-2 text-[10.5px] uppercase tracking-[0.16em]",
        tone === "warning"
          ? "text-[var(--tl-close)]"
          : tone === "ok"
            ? "text-[color-mix(in_oklch,var(--tl-max)_60%,var(--color-foreground))]"
            : "text-[var(--color-muted-foreground)]",
      )}
    >
      {label}
    </div>
  );
}

function RowCells({ row }: { row: ComparisonRow }) {
  return (
    <>
      <div className="bg-[var(--color-card)] px-3 py-2.5 text-[11.5px] font-medium leading-snug">
        {row.topic}
      </div>
      <div className="flex items-start gap-1.5 bg-[var(--color-card)] px-3 py-2.5 text-[11.5px] leading-snug text-[var(--color-muted-foreground)]">
        {row.closedBad ? (
          <X
            className="mt-px h-3 w-3 shrink-0 text-[var(--tl-close)]"
            strokeWidth={2.5}
          />
        ) : (
          <span className="w-3" />
        )}
        <span>{row.closedNote}</span>
      </div>
      <div className="flex items-start gap-1.5 bg-[var(--color-card)] px-3 py-2.5 text-[11.5px] leading-snug">
        <Check
          className="mt-px h-3 w-3 shrink-0 text-[color-mix(in_oklch,var(--tl-max)_60%,var(--color-foreground))]"
          strokeWidth={2.5}
        />
        <span>{row.openNote}</span>
      </div>
    </>
  );
}

function Footer() {
  return (
    <section
      data-slot="about-footer"
      className={cn(
        "flex flex-col gap-3 rounded-[var(--radius)] border border-[var(--color-border)]",
        "bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)]",
        "p-4",
      )}
    >
      <div className="flex items-center gap-2 text-[12.5px] font-semibold tracking-tight">
        <HeartHandshake className="h-3.5 w-3.5" strokeWidth={2.25} />
        Get involved
      </div>
      <p className="text-[11.5px] leading-relaxed text-[var(--color-muted-foreground)]">
        OpenForge grows when contributors drop new games into{" "}
        <code className="text-[var(--color-foreground)]">crates/games/</code>{" "}
        or sharpen the engine. Pick whatever path looks fun.
      </p>
      <div className="flex flex-wrap items-center gap-2 pt-1">
        <Button
          size="sm"
          variant="default"
          className="gap-1.5 text-[12px]"
          onClick={() => openExternal(REPO_URL)}
        >
          <Github className="h-3 w-3" strokeWidth={2.25} />
          Source on GitHub
          <ExternalLink className="h-3 w-3 opacity-70" />
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="gap-1.5 text-[12px]"
          onClick={() => openExternal(YOUTUBE_URL)}
        >
          <Youtube className="h-3 w-3" strokeWidth={2.25} />
          @GamesPatch
          <ExternalLink className="h-3 w-3 opacity-70" />
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="gap-1.5 text-[12px]"
          onClick={() => openExternal(WEBSITE_URL)}
        >
          <Globe className="h-3 w-3" strokeWidth={2.25} />
          theappstack.in
          <ExternalLink className="h-3 w-3 opacity-70" />
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="gap-1.5 text-[11.5px] text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)]"
          onClick={() => openExternal(LICENSE_URL)}
        >
          MIT License
        </Button>
      </div>
      <p className="text-[10.5px] leading-relaxed text-[var(--color-muted-foreground)]">
        Not affiliated with any game publisher. All trademarks belong to their
        owners. Use responsibly — single-player, offline, and only on games
        you own.
      </p>
    </section>
  );
}
