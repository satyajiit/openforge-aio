import { useMemo, useState } from "react";
import {
  Code2,
  Crosshair,
  Puzzle,
  Sparkles,
  type LucideIcon,
} from "lucide-react";

import { Tabs, TabsContent } from "@/components/ui/tabs";
import { ScrollArea } from "@/components/ui/scroll-area";
import { FeatureCard, CoordinateTeleportCard } from "@/components/feature-card";
import { ActivateBar } from "@/components/activate-bar";
import { EmptyState } from "@/components/empty-state";
import { GameAtmosphere } from "@/components/game-atmosphere";
import { LuaScriptsTab } from "@/components/lua-scripts-tab";
import { useFeatures } from "@/hooks/use-features";
import { tierColor } from "@/lib/tier-colors";
import { cn } from "@/lib/utils";
import type { FeatureCategory, FeatureMeta, GameMeta } from "@/types";

type TopTab = "trainer" | "mods" | "lua";

type TopTabSpec = {
  id: TopTab;
  label: string;
  category: FeatureCategory | null;
  icon: LucideIcon;
};

const TOP_TABS: TopTabSpec[] = [
  { id: "trainer", label: "Trainer", category: "trainer", icon: Crosshair },
  { id: "mods", label: "Mods", category: "mod", icon: Puzzle },
  { id: "lua", label: "Lua Scripts", category: null, icon: Code2 },
];

export function FeaturePane({ game }: { game: GameMeta }) {
  const {
    features,
    values,
    resolutions,
    statusTexts,
    freezeRuntimes,
    attached,
    write,
    setFreeze,
    setCodePatch,
    retryResolve,
  } = useFeatures(game.id);

  const byCategory = useMemo(() => {
    const trainer: FeatureMeta[] = [];
    const mods: FeatureMeta[] = [];
    for (const f of features) {
      const cat = f.category ?? "trainer";
      if (cat === "mod") mods.push(f);
      else trainer.push(f);
    }
    return { trainer, mods };
  }, [features]);

  const [topTab, setTopTab] = useState<TopTab>(
    byCategory.trainer.length > 0 ? "trainer" : "mods",
  );

  return (
    <main
      data-slot="feature-pane"
      className="relative flex h-full min-w-0 flex-1 flex-col overflow-hidden"
    >
      <GameAtmosphere gameId={game.id} attached={attached} />
      {features.length === 0 ? (
        <>
          <div className="px-6 pt-5">
            <ActivateBar game={game} />
          </div>
          <div className="flex flex-1 items-center justify-center p-6">
            <EmptyState
              title="No features yet"
              description={`Run \`openforge-discover --game ${game.id} scan ...\` to discover a memory address and emit a signature.`}
            />
          </div>
        </>
      ) : (
        <Tabs
          value={topTab}
          onValueChange={(v) => setTopTab(v as TopTab)}
          className="flex min-h-0 flex-1 flex-col"
        >
          <MainTabStrip
            active={topTab}
            onChange={(v) => setTopTab(v)}
            counts={{
              trainer: byCategory.trainer.length,
              mods: byCategory.mods.length,
              lua: null,
            }}
          />

          <TabsContent
            value="trainer"
            className="flex min-h-0 flex-1 flex-col gap-3 px-6 pb-6 pt-4"
          >
            <ActivateBar game={game} />
            <CategoryPane
              features={byCategory.trainer}
              attached={attached}
              values={values}
              resolutions={resolutions}
              statusTexts={statusTexts}
              freezeRuntimes={freezeRuntimes}
              onWrite={write}
              onFreeze={setFreeze}
              onCodePatch={setCodePatch}
              onRetry={retryResolve}
              emptyDescription="No cheats are wired up for this game yet."
            />
          </TabsContent>

          <TabsContent
            value="mods"
            className="flex min-h-0 flex-1 flex-col gap-3 px-6 pb-6 pt-4"
          >
            <ActivateBar game={game} />
            <CategoryPane
              features={byCategory.mods}
              attached={attached}
              values={values}
              resolutions={resolutions}
              statusTexts={statusTexts}
              freezeRuntimes={freezeRuntimes}
              onWrite={write}
              onFreeze={setFreeze}
              onCodePatch={setCodePatch}
              onRetry={retryResolve}
              emptyDescription="Cosmetic mods and gameplay extras land here. None shipped yet."
            />
          </TabsContent>

          <TabsContent
            value="lua"
            className="flex min-h-0 flex-1 flex-col px-6 pb-6 pt-4"
          >
            <LuaScriptsTab gameId={game.id} />
          </TabsContent>
        </Tabs>
      )}
    </main>
  );
}

/**
 * Full-width tab strip — sits flush against the header so the eye reads
 * the entire chrome as one continuous band. Each tab is a roomy click
 * target with an icon, label and live count, and the active tab is
 * marked with a thick foreground underline that bleeds to the bottom
 * border. The whole strip is the same surface as the titlebar.
 */
function MainTabStrip({
  active,
  onChange,
  counts,
}: {
  active: TopTab;
  onChange: (id: TopTab) => void;
  counts: { trainer: number; mods: number; lua: number | null };
}) {
  return (
    <div
      data-slot="main-tab-strip"
      role="tablist"
      aria-label="Top-level categories"
      className="tabstrip-surface flex shrink-0 items-stretch px-2"
    >
      {TOP_TABS.map((t) => {
        const isActive = active === t.id;
        const count = counts[t.id];
        const Icon = t.icon;
        return (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={isActive}
            onClick={() => onChange(t.id)}
            className={cn(
              "group/tab relative inline-flex items-center gap-2 px-4 py-2.5",
              "text-[13px] font-medium tracking-tight outline-none transition-colors",
              "focus-visible:bg-[color-mix(in_oklch,var(--color-foreground)_5%,transparent)]",
              isActive
                ? "text-[var(--color-foreground)]"
                : "text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)]",
            )}
          >
            <Icon
              className={cn(
                "h-4 w-4 transition-colors",
                isActive ? "opacity-100" : "opacity-65 group-hover/tab:opacity-95",
              )}
              strokeWidth={2}
            />
            <span>{t.label}</span>
            {count !== null ? (
              <span
                className={cn(
                  "tabular ml-0.5 grid h-[18px] min-w-[18px] place-items-center rounded-full px-1 text-[10px] font-semibold",
                  isActive
                    ? "bg-[var(--color-foreground)] text-[var(--color-background)]"
                    : "bg-[color-mix(in_oklch,var(--color-foreground)_8%,transparent)] text-[var(--color-muted-foreground)]",
                )}
              >
                {count}
              </span>
            ) : null}
            {/* Active underline — sits on top of the strip's bottom
                border so it reads as a continuation, not a stripe. */}
            <span
              aria-hidden
              className={cn(
                "absolute inset-x-3 bottom-[-1px] h-[2px] rounded-full transition-all duration-200",
                isActive
                  ? "bg-[var(--color-foreground)] opacity-100"
                  : "bg-[var(--color-foreground)] opacity-0",
              )}
            />
          </button>
        );
      })}
    </div>
  );
}

/** Sub-pane for one top-level category. Renders chips for tier filters
 *  (visually distinct from the main tab strip — smaller, pill-shaped,
 *  inline with content rather than as a separate band). */
function CategoryPane({
  features,
  attached,
  values,
  resolutions,
  statusTexts,
  freezeRuntimes,
  onWrite,
  onFreeze,
  onCodePatch,
  onRetry,
  emptyDescription,
}: {
  features: FeatureMeta[];
  attached: boolean;
  values: ReturnType<typeof useFeatures>["values"];
  resolutions: ReturnType<typeof useFeatures>["resolutions"];
  statusTexts: ReturnType<typeof useFeatures>["statusTexts"];
  freezeRuntimes: ReturnType<typeof useFeatures>["freezeRuntimes"];
  onWrite: ReturnType<typeof useFeatures>["write"];
  onFreeze: ReturnType<typeof useFeatures>["setFreeze"];
  onCodePatch: ReturnType<typeof useFeatures>["setCodePatch"];
  onRetry: ReturnType<typeof useFeatures>["retryResolve"];
  emptyDescription: string;
}) {
  const tiers = useMemo(() => groupByTier(features), [features]);
  const tierNames = useMemo(() => Object.keys(tiers).sort(), [tiers]);
  const [activeTier, setActiveTier] = useState<string>("__all__");

  if (features.length === 0) {
    return (
      <EmptyState title="Nothing here yet" description={emptyDescription} />
    );
  }

  const visible =
    activeTier === "__all__" ? features : (tiers[activeTier] ?? []);

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2.5">
      <TierChips
        tierNames={tierNames}
        tiers={tiers}
        total={features.length}
        active={activeTier}
        onChange={setActiveTier}
      />
      <FeatureList
        list={visible}
        attached={attached}
        values={values}
        resolutions={resolutions}
        statusTexts={statusTexts}
        freezeRuntimes={freezeRuntimes}
        onWrite={onWrite}
        onFreeze={onFreeze}
        onCodePatch={onCodePatch}
        onRetry={onRetry}
      />
    </div>
  );
}

/**
 * Tier filter as soft pills. Visually distinct from the main tab strip:
 * smaller (h-6 vs h-10), rounded full, with a colored leading dot per
 * tier. Active chip flips to a filled inverse — no underline, no large
 * type — so the eye instantly distinguishes the two levels of navigation.
 */
function TierChips({
  tierNames,
  tiers,
  total,
  active,
  onChange,
}: {
  tierNames: string[];
  tiers: Record<string, FeatureMeta[]>;
  total: number;
  active: string;
  onChange: (v: string) => void;
}) {
  const items: { id: string; label: string; count: number; dot?: string }[] = [
    { id: "__all__", label: "All", count: total },
    ...tierNames.map((t) => ({
      id: t,
      label: t,
      count: tiers[t]?.length ?? 0,
      dot: tierColor(t).fg,
    })),
  ];

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <Sparkles
        aria-hidden
        className="mr-0.5 h-3 w-3 text-[var(--color-muted-foreground)]"
        strokeWidth={2.25}
      />
      <span className="mr-1.5 text-[10px] uppercase tracking-[0.18em] text-[var(--color-muted-foreground)]">
        Filter
      </span>
      {items.map((it) => {
        const isActive = active === it.id;
        return (
          <button
            key={it.id}
            type="button"
            onClick={() => onChange(it.id)}
            className={cn(
              "group/chip inline-flex h-6 items-center gap-1.5 rounded-full px-2.5 text-[11px] font-medium",
              "border transition-all duration-150",
              isActive
                ? "border-transparent bg-[var(--color-foreground)] text-[var(--color-background)] shadow-sm"
                : "border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-card)_50%,transparent)] text-[var(--color-muted-foreground)] hover:border-[var(--color-foreground)]/40 hover:text-[var(--color-foreground)]",
            )}
          >
            {it.dot ? (
              <span
                aria-hidden
                className="inline-flex h-1.5 w-1.5 rounded-full"
                style={{
                  backgroundColor: isActive
                    ? "var(--color-background)"
                    : it.dot,
                }}
              />
            ) : (
              <span
                aria-hidden
                className={cn(
                  "inline-flex h-1.5 w-1.5 rounded-full",
                  isActive
                    ? "bg-[var(--color-background)]"
                    : "bg-[var(--color-foreground)]",
                )}
              />
            )}
            <span className="capitalize">{it.label}</span>
            <span
              className={cn(
                "tabular text-[9.5px] opacity-70",
                isActive ? "opacity-90" : "",
              )}
            >
              {it.count}
            </span>
          </button>
        );
      })}
    </div>
  );
}

function FeatureList({
  list,
  attached,
  values,
  resolutions,
  statusTexts,
  freezeRuntimes,
  onWrite,
  onFreeze,
  onCodePatch,
  onRetry,
}: {
  list: FeatureMeta[];
  attached: boolean;
  values: ReturnType<typeof useFeatures>["values"];
  resolutions: ReturnType<typeof useFeatures>["resolutions"];
  statusTexts: ReturnType<typeof useFeatures>["statusTexts"];
  freezeRuntimes: ReturnType<typeof useFeatures>["freezeRuntimes"];
  onWrite: ReturnType<typeof useFeatures>["write"];
  onFreeze: ReturnType<typeof useFeatures>["setFreeze"];
  onCodePatch: ReturnType<typeof useFeatures>["setCodePatch"];
  onRetry: ReturnType<typeof useFeatures>["retryResolve"];
}) {
  const coords = list.filter(
    (f) =>
      f.id === "teleport_x" || f.id === "teleport_y" || f.id === "teleport_z",
  );
  const otherFeatures = list.filter(
    (f) =>
      f.id !== "teleport_x" && f.id !== "teleport_y" && f.id !== "teleport_z",
  );
  const hasAllCoords = coords.length === 3;

  return (
    <ScrollArea className="flex-1 pr-2">
      <div className="flex flex-col gap-2.5">
        {otherFeatures.map((f) => (
          <FeatureCard
            key={f.id}
            feature={f}
            current={values[f.id]}
            resolution={resolutions[f.id]}
            statusText={statusTexts[f.id]}
            freezeRuntime={freezeRuntimes[f.id]}
            disabled={!attached}
            onWrite={(v) => onWrite(f.id, v)}
            onFreeze={(b) => onFreeze(f.id, b)}
            onCodePatch={(b) => onCodePatch(f.id, b)}
            onRetry={() => onRetry(f.id)}
          />
        ))}
        {!hasAllCoords &&
          coords.map((f) => (
            <FeatureCard
              key={f.id}
              feature={f}
              current={values[f.id]}
              resolution={resolutions[f.id]}
              statusText={statusTexts[f.id]}
              freezeRuntime={freezeRuntimes[f.id]}
              disabled={!attached}
              onWrite={(v) => onWrite(f.id, v)}
              onFreeze={(b) => onFreeze(f.id, b)}
              onCodePatch={(b) => onCodePatch(f.id, b)}
              onRetry={() => onRetry(f.id)}
            />
          ))}
        {hasAllCoords && (
          <CoordinateTeleportCard
            features={coords}
            values={values}
            resolutions={resolutions}
            disabled={!attached}
            onWrite={onWrite}
            onRetry={onRetry}
          />
        )}
      </div>
    </ScrollArea>
  );
}

function groupByTier(features: FeatureMeta[]): Record<string, FeatureMeta[]> {
  const out: Record<string, FeatureMeta[]> = {};
  for (const f of features) {
    const key = f.tier || "general";
    if (!out[key]) out[key] = [];
    out[key].push(f);
  }
  return out;
}
