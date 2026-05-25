import * as React from "react";
import { toast } from "sonner";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  CheckCircle2,
  Code2,
  Download,
  ExternalLink,
  Github,
  Loader2,
  Play,
  Plus,
  RefreshCw,
  Save,
  Sparkles,
  Square,
  XCircle,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { LuaConsole } from "@/components/lua-console";
import { LuaEditor } from "@/components/lua-editor";
import { LuaScriptList } from "@/components/lua-script-list";
import { NewLuaScriptDialog } from "@/components/new-lua-script-dialog";
import { cn } from "@/lib/utils";
import {
  ipc,
  LUA_OUTPUT_EVENT,
  LUA_SCRIPT_STATUS_EVENT,
} from "@/lib/ipc";
import { openExternal } from "@/lib/open-external";
import { useAppStore } from "@/store/app-store";
import type {
  LuaOutputEvent,
  LuaScript,
  LuaScriptStatusEvent,
  LuaSource,
  LuaValidation,
} from "@/types";

const EMPTY_BUCKET: { user: LuaScript[]; community: LuaScript[] } = {
  user: [],
  community: [],
};

const COMMUNITY_REPO_URL =
  "https://github.com/openforge/openforge-lua-scripts";

const STARTER_TEMPLATE = `-- New script
-- Tip: the Lua runtime integration is not yet wired up — Run is disabled.
-- For now, you can author and save scripts here; the runtime will arrive in a
-- follow-up release.

print("hello from openforge")
`;

const scriptKey = (gameId: string, source: LuaSource, slug: string): string =>
  `${gameId}:${source}:${slug}`;

export function LuaScriptsTab({ gameId }: { gameId: string }) {
  const luaBucket =
    useAppStore((s) => s.luaScriptsByGame[gameId]) ?? EMPTY_BUCKET;
  const selected = useAppStore((s) => s.luaSelectedByGame[gameId]) ?? null;
  const validationByScript = useAppStore((s) => s.luaValidationByScript);
  const draftByScript = useAppStore((s) => s.luaDraftByScript);
  const setLuaScripts = useAppStore((s) => s.setLuaScripts);
  const upsertLuaScript = useAppStore((s) => s.upsertLuaScript);
  const removeLuaScript = useAppStore((s) => s.removeLuaScript);
  const setLuaSelected = useAppStore((s) => s.setLuaSelected);
  const setLuaValidation = useAppStore((s) => s.setLuaValidation);
  const setLuaDraft = useAppStore((s) => s.setLuaDraft);
  const clearLuaDraft = useAppStore((s) => s.clearLuaDraft);
  const appendLuaLines = useAppStore((s) => s.appendLuaLines);
  const setLuaRunning = useAppStore((s) => s.setLuaRunning);
  const luaRunByScript = useAppStore((s) => s.luaRunByScript);
  const attachState = useAppStore((s) => s.attachState);

  const attached =
    attachState.kind === "attached" && attachState.gameId === gameId;

  const [source, setSource] = React.useState<LuaSource>("user");
  const [loading, setLoading] = React.useState(false);
  const [refreshing, setRefreshing] = React.useState(false);
  const [newDialogOpen, setNewDialogOpen] = React.useState(false);

  // UI-only clean-copy ledger so the dirty indicator is meaningful. Keyed
  // identically to `luaDraftByScript`; the value is the most-recently-saved
  // text on disk. Cleared when a script is deleted or the user navigates
  // away from a community script they never installed.
  const [cleanByScript, setCleanByScript] = React.useState<
    Record<string, string>
  >({});

  // Per-user-script in-flight rename. Decouples the visible name from the
  // store so we don't write on every keystroke. Saved together with the
  // body on Save.
  const [nameDraft, setNameDraft] = React.useState<string>("");
  const [savedName, setSavedName] = React.useState<string>("");

  // Subscribe to backend Lua events for the lifetime of this tab.
  // Re-subscribes when `gameId` changes; tears down on unmount. The
  // payload's `gameId` filter is defensive — backend only emits for the
  // active attach, but a stale tab during a quick game-switch shouldn't
  // append the wrong game's lines.
  React.useEffect(() => {
    let cancelled = false;
    const unlisteners: UnlistenFn[] = [];
    (async () => {
      const off1 = await listen<LuaOutputEvent>(LUA_OUTPUT_EVENT, (e) => {
        if (e.payload.gameId !== gameId) return;
        const key = scriptKey(gameId, e.payload.source, e.payload.slug);
        appendLuaLines(key, e.payload.lines);
      });
      const off2 = await listen<LuaScriptStatusEvent>(
        LUA_SCRIPT_STATUS_EVENT,
        (e) => {
          if (e.payload.gameId !== gameId) return;
          if (!e.payload.slug) {
            // Synthetic "stop" event from `stop_lua_script` — no slug
            // attached. Mark every running script in this game as stopped.
            // (We only ever have one active script per attach, so this is
            // a cheap iteration in practice.)
            return;
          }
          const key = scriptKey(gameId, e.payload.source, e.payload.slug);
          setLuaRunning(
            key,
            e.payload.running,
            e.payload.name,
            e.payload.lastError,
          );
          if (!e.payload.running && e.payload.lastError) {
            toast.error(
              `${e.payload.name ?? "Lua script"} stopped: ${e.payload.lastError}`,
            );
          }
        },
      );
      if (cancelled) {
        off1();
        off2();
      } else {
        unlisteners.push(off1, off2);
      }
    })();
    return () => {
      cancelled = true;
      for (const off of unlisteners) off();
    };
  }, [gameId, appendLuaLines, setLuaRunning]);

  // ---- Initial load ----------------------------------------------------
  React.useEffect(() => {
    if (!gameId || !ipc.isTauri()) return;
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const scripts = await ipc.listLuaScripts(gameId);
        if (!cancelled) setLuaScripts(gameId, scripts);
      } catch (e) {
        const err = e as { message?: string };
        if (!cancelled) {
          toast.error(`Couldn't load Lua scripts: ${err.message ?? String(e)}`);
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [gameId, setLuaScripts]);

  // ---- Resolve selected metadata --------------------------------------
  const visibleScripts = source === "user" ? luaBucket.user : luaBucket.community;
  const selectedMeta =
    selected !== null && selected.source === source
      ? visibleScripts.find((s) => s.slug === selected.slug) ?? null
      : null;

  const selectedKey = selectedMeta
    ? scriptKey(gameId, selectedMeta.source, selectedMeta.slug)
    : null;
  const draft = selectedKey ? draftByScript[selectedKey] : undefined;
  const validation = selectedKey ? validationByScript[selectedKey] : undefined;

  // ---- Lazy fetch body on selection -----------------------------------
  React.useEffect(() => {
    if (!selectedMeta || !selectedKey) return;
    // Community scripts that aren't installed have no body to fetch.
    if (selectedMeta.source === "community" && !selectedMeta.installed) {
      // Pre-seed an empty draft so the editor renders an explanatory view
      // and a clean reference so the editor never reads as "dirty."
      if (draftByScript[selectedKey] === undefined) {
        setLuaDraft(selectedKey, "");
        setCleanByScript((m) => ({ ...m, [selectedKey]: "" }));
      }
      return;
    }
    if (draftByScript[selectedKey] !== undefined) {
      // Body already loaded; keep clean reference if we don't have one.
      setCleanByScript((m) =>
        m[selectedKey] === undefined
          ? { ...m, [selectedKey]: draftByScript[selectedKey] ?? "" }
          : m,
      );
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const body = await ipc.readLuaScript(
          gameId,
          selectedMeta.source,
          selectedMeta.slug,
        );
        if (cancelled) return;
        setLuaDraft(selectedKey, body);
        setCleanByScript((m) => ({ ...m, [selectedKey]: body }));
        // Kick off initial validation.
        try {
          const v = await ipc.validateLuaScript(body);
          if (!cancelled) setLuaValidation(selectedKey, v);
        } catch {
          /* ignore — pill stays at idle */
        }
      } catch (e) {
        const err = e as { message?: string };
        if (!cancelled) {
          toast.error(`Couldn't read script: ${err.message ?? String(e)}`);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // We deliberately don't include `draftByScript` in deps — it changes on
    // every keystroke. We only care about the *selection* changing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [gameId, selectedKey, selectedMeta?.source, selectedMeta?.installed]);

  // ---- Sync local name state when selection changes -------------------
  React.useEffect(() => {
    if (selectedMeta) {
      setNameDraft(selectedMeta.name);
      setSavedName(selectedMeta.name);
    } else {
      setNameDraft("");
      setSavedName("");
    }
  }, [selectedMeta?.slug, selectedMeta?.source, selectedMeta?.name]);

  // ---- Debounced validation -------------------------------------------
  const validationTimerRef = React.useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  const validationLatestRef = React.useRef<{ key: string; text: string } | null>(
    null,
  );
  const scheduleValidation = React.useCallback(
    (key: string, text: string) => {
      validationLatestRef.current = { key, text };
      if (validationTimerRef.current) clearTimeout(validationTimerRef.current);
      validationTimerRef.current = setTimeout(async () => {
        const snap = validationLatestRef.current;
        if (!snap) return;
        try {
          const v = await ipc.validateLuaScript(snap.text);
          // Only commit if the user is still editing the same script.
          if (validationLatestRef.current?.key === snap.key) {
            setLuaValidation(snap.key, v);
          }
        } catch {
          /* swallow — validator errors aren't actionable */
        }
      }, 300);
    },
    [setLuaValidation],
  );

  React.useEffect(() => {
    return () => {
      if (validationTimerRef.current) clearTimeout(validationTimerRef.current);
    };
  }, []);

  // ---- Handlers --------------------------------------------------------
  const handleEditorChange = (next: string) => {
    if (!selectedKey) return;
    setLuaDraft(selectedKey, next);
    scheduleValidation(selectedKey, next);
  };

  const handleRefresh = async () => {
    if (refreshing) return;
    setRefreshing(true);
    try {
      const community = await ipc.refreshCommunityLuaIndex(gameId);
      // Preserve user scripts already in store.
      setLuaScripts(gameId, [...luaBucket.user, ...community]);
      toast.success("Community index refreshed");
    } catch (e) {
      const err = e as { message?: string };
      toast.error(`Refresh failed: ${err.message ?? String(e)}`);
    } finally {
      setRefreshing(false);
    }
  };

  const handleSelect = (slug: string) => {
    setLuaSelected(gameId, { source, slug });
  };

  const handleCreate = async (slug: string, name: string) => {
    const result = await ipc.saveUserLuaScript(gameId, slug, name, STARTER_TEMPLATE);
    upsertLuaScript(gameId, result);
    const key = scriptKey(gameId, "user", slug);
    setLuaDraft(key, STARTER_TEMPLATE);
    setCleanByScript((m) => ({ ...m, [key]: STARTER_TEMPLATE }));
    setLuaSelected(gameId, { source: "user", slug });
    setSource("user");
    // Seed validation eagerly so the pill goes green right away.
    try {
      const v = await ipc.validateLuaScript(STARTER_TEMPLATE);
      setLuaValidation(key, v);
    } catch {
      /* ignore */
    }
    toast.success(`Created ${name}`);
  };

  const handleSave = async () => {
    if (!selectedMeta || !selectedKey) return;
    if (selectedMeta.source !== "user") return;
    const body = draft ?? "";
    const name = nameDraft.trim() || selectedMeta.name;
    try {
      const result = await ipc.saveUserLuaScript(
        gameId,
        selectedMeta.slug,
        name,
        body,
      );
      upsertLuaScript(gameId, result);
      setCleanByScript((m) => ({ ...m, [selectedKey]: body }));
      setSavedName(result.name);
      setNameDraft(result.name);
      toast.success(`Saved ${result.name}`);
    } catch (e) {
      const err = e as { message?: string };
      toast.error(`Save failed: ${err.message ?? String(e)}`);
    }
  };

  const handleDelete = (slug: string) => {
    const target = luaBucket.user.find((s) => s.slug === slug);
    if (!target) return;
    toast.warning(`Delete "${target.name}"? This cannot be undone.`, {
      action: {
        label: "Delete",
        onClick: () => {
          void (async () => {
            try {
              await ipc.deleteUserLuaScript(gameId, slug);
              const key = scriptKey(gameId, "user", slug);
              removeLuaScript(gameId, "user", slug);
              clearLuaDraft(key);
              setCleanByScript((m) => {
                const next = { ...m };
                delete next[key];
                return next;
              });
              if (selected?.source === "user" && selected.slug === slug) {
                setLuaSelected(gameId, null);
              }
              toast.success(`Deleted ${target.name}`);
            } catch (e) {
              const err = e as { message?: string };
              toast.error(`Delete failed: ${err.message ?? String(e)}`);
            }
          })();
        },
      },
    });
  };

  const handleInstall = async () => {
    if (!selectedMeta || selectedMeta.source !== "community") return;
    try {
      const result = await ipc.installCommunityLuaScript(gameId, selectedMeta.slug);
      upsertLuaScript(gameId, result);
      const key = scriptKey(gameId, "community", selectedMeta.slug);
      const body = await ipc.readLuaScript(gameId, "community", selectedMeta.slug);
      setLuaDraft(key, body);
      setCleanByScript((m) => ({ ...m, [key]: body }));
      try {
        const v = await ipc.validateLuaScript(body);
        setLuaValidation(key, v);
      } catch {
        /* ignore */
      }
      toast.success(`Installed ${result.name}`);
    } catch (e) {
      const err = e as { message?: string };
      toast.error(`Install failed: ${err.message ?? String(e)}`);
    }
  };

  const handleRun = async () => {
    if (!selectedMeta) return;
    const key = scriptKey(gameId, selectedMeta.source, selectedMeta.slug);
    // Pre-clear the buffer locally so the user sees an instant reset
    // even before the backend's first status event lands.
    setLuaRunning(key, true, selectedMeta.name, null);
    try {
      await ipc.runLuaScript(gameId, selectedMeta.source, selectedMeta.slug);
    } catch (e) {
      const err = e as { message?: string };
      setLuaRunning(key, false, selectedMeta.name, err.message ?? String(e));
      toast.error(`Run failed: ${err.message ?? String(e)}`);
    }
  };

  const handleStop = async () => {
    if (!selectedMeta) return;
    try {
      await ipc.stopLuaScript(gameId);
    } catch (e) {
      const err = e as { message?: string };
      toast.error(`Stop failed: ${err.message ?? String(e)}`);
    }
  };

  const clean = selectedKey ? cleanByScript[selectedKey] : undefined;
  const draftDirty =
    selectedMeta?.source === "user" &&
    selectedKey !== null &&
    ((clean !== undefined && draft !== undefined && clean !== draft) ||
      savedName !== nameDraft.trim());

  const existingSlugs = React.useMemo(
    () => luaBucket.user.map((s) => s.slug),
    [luaBucket.user],
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col rounded-[var(--radius)] border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-card)_65%,transparent)] backdrop-blur-sm">
      {/* Header strip */}
      <div className="flex shrink-0 items-center gap-3 border-b border-[var(--color-border)] px-4 py-3">
        <SourceToggle
          active={source}
          counts={{
            user: luaBucket.user.length,
            community: luaBucket.community.length,
          }}
          onChange={(next) => {
            setSource(next);
            // Drop selection if it doesn't belong to the new source.
            if (selected && selected.source !== next) {
              setLuaSelected(gameId, null);
            }
          }}
        />
        <div className="ml-auto flex items-center gap-2">
          {source === "user" ? (
            <Button
              size="sm"
              className="gap-1.5 text-[12px]"
              onClick={() => setNewDialogOpen(true)}
            >
              <Plus className="h-3.5 w-3.5" strokeWidth={2.25} />
              New script
            </Button>
          ) : (
            <Button
              variant="outline"
              size="sm"
              className="gap-1.5 text-[12px]"
              onClick={() => void handleRefresh()}
              disabled={refreshing}
            >
              {refreshing ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" strokeWidth={2.25} />
              ) : (
                <RefreshCw className="h-3.5 w-3.5" strokeWidth={2.25} />
              )}
              Refresh
            </Button>
          )}
        </div>
      </div>

      {/* Body */}
      <div className="flex min-h-0 flex-1 flex-row">
        {/* Sidebar */}
        <aside className="flex w-64 shrink-0 flex-col border-r border-[var(--color-border)]">
          {loading ? (
            <div className="flex flex-1 items-center justify-center gap-2 px-4 py-10 text-[12px] text-[var(--color-muted-foreground)]">
              <Loader2 className="h-3.5 w-3.5 animate-spin" strokeWidth={2.25} />
              Loading…
            </div>
          ) : (
            <ScrollArea className="flex-1">
              <LuaScriptList
                scripts={visibleScripts}
                selectedSlug={
                  selected?.source === source ? selected.slug : null
                }
                onSelect={handleSelect}
                onDelete={source === "user" ? handleDelete : undefined}
                emptyMessage={
                  source === "user" ? (
                    <span>
                      No scripts yet. Click <span className="font-medium text-[var(--color-foreground)]">New script</span> to author your first one.
                    </span>
                  ) : (
                    <span>
                      No community scripts indexed. Hit <span className="font-medium text-[var(--color-foreground)]">Refresh</span> to pull the latest from GitHub.
                    </span>
                  )
                }
              />
            </ScrollArea>
          )}
        </aside>

        {/* Editor pane */}
        <section className="flex min-w-0 flex-1 flex-col">
          {!selectedMeta ? (
            <EmptyEditor source={source} />
          ) : (
            <>
              <div className="flex shrink-0 items-center gap-3 border-b border-[var(--color-border)] px-4 py-2">
                {selectedMeta.source === "user" ? (
                  <Input
                    value={nameDraft}
                    onChange={(e) => setNameDraft(e.target.value)}
                    className="h-7 max-w-[260px] border-transparent bg-transparent px-1.5 text-[13px] font-medium focus-visible:border-[var(--color-border)]"
                    spellCheck={false}
                  />
                ) : (
                  <span className="truncate text-[13px] font-medium tracking-tight">
                    {selectedMeta.name}
                  </span>
                )}

                <ValidationPill validation={validation} />

                <div className="flex-1" />

                {selectedMeta.source === "user" ? (
                  <>
                    {draftDirty ? (
                      <span
                        aria-hidden
                        className="inline-flex h-1.5 w-1.5 rounded-full bg-[var(--color-foreground)]"
                        title="Unsaved changes"
                      />
                    ) : null}
                    <Button
                      size="sm"
                      variant="outline"
                      className="gap-1.5 text-[12px]"
                      onClick={() => void handleSave()}
                      disabled={!draftDirty}
                    >
                      <Save className="h-3.5 w-3.5" strokeWidth={2.25} />
                      Save
                    </Button>
                  </>
                ) : null}

                {selectedMeta.source === "community" && !selectedMeta.installed ? (
                  <Button
                    size="sm"
                    variant="outline"
                    className="gap-1.5 text-[12px]"
                    onClick={() => void handleInstall()}
                  >
                    <Download className="h-3.5 w-3.5" strokeWidth={2.25} />
                    Install
                  </Button>
                ) : null}

                {(() => {
                  const runKey = scriptKey(
                    gameId,
                    selectedMeta.source,
                    selectedMeta.slug,
                  );
                  const runState = luaRunByScript[runKey];
                  const isRunning = !!runState?.running;
                  const validation = selectedKey
                    ? validationByScript[selectedKey]
                    : undefined;
                  const validOrUnvalidated = !validation || validation.ok;
                  const communityUninstalled =
                    selectedMeta.source === "community" &&
                    !selectedMeta.installed;
                  const disabledReason: string | null = !attached
                    ? "Attach to the game first."
                    : communityUninstalled
                      ? "Install this script first."
                      : !validOrUnvalidated
                        ? "Fix syntax errors first."
                        : null;
                  if (isRunning) {
                    return (
                      <Button
                        size="sm"
                        variant="outline"
                        className="gap-1.5 text-[12px]"
                        onClick={() => void handleStop()}
                      >
                        <Square
                          className="h-3.5 w-3.5"
                          strokeWidth={2.25}
                          fill="currentColor"
                        />
                        Stop
                      </Button>
                    );
                  }
                  const button = (
                    <Button
                      size="sm"
                      className="gap-1.5 text-[12px]"
                      onClick={() => void handleRun()}
                      disabled={disabledReason !== null}
                    >
                      <Play className="h-3.5 w-3.5" strokeWidth={2.25} />
                      Run
                    </Button>
                  );
                  if (disabledReason === null) return button;
                  return (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <span>{button}</span>
                      </TooltipTrigger>
                      <TooltipContent>{disabledReason}</TooltipContent>
                    </Tooltip>
                  );
                })()}
              </div>

              {selectedMeta.source === "community" && !selectedMeta.installed ? (
                <NotInstalledEditor
                  meta={selectedMeta}
                  onInstall={() => void handleInstall()}
                />
              ) : (
                <div className="flex min-h-0 flex-1 flex-col gap-2">
                  <div className="flex min-h-0 flex-[7] flex-col">
                    <div className="flex-1 overflow-hidden">
                      <LuaEditor
                        value={draft ?? ""}
                        onChange={handleEditorChange}
                        errorLine={firstErrorLine(validation)}
                        readOnly={selectedMeta.source === "community"}
                      />
                    </div>
                    <BottomStrip validation={validation} />
                  </div>
                  <div className="min-h-0 flex-[3]">
                    <LuaConsole
                      gameId={gameId}
                      source={selectedMeta.source}
                      slug={selectedMeta.slug}
                    />
                  </div>
                </div>
              )}
            </>
          )}
        </section>
      </div>

      <NewLuaScriptDialog
        open={newDialogOpen}
        onOpenChange={setNewDialogOpen}
        existingSlugs={existingSlugs}
        onCreate={handleCreate}
      />
    </div>
  );
}

function firstErrorLine(v: LuaValidation | undefined): number | null {
  if (!v || v.ok) return null;
  const first = v.errors.find((e) => e.line && e.line > 0);
  return first ? first.line : null;
}

function SourceToggle({
  active,
  counts,
  onChange,
}: {
  active: LuaSource;
  counts: { user: number; community: number };
  onChange: (next: LuaSource) => void;
}) {
  const items: { id: LuaSource; label: string; count: number }[] = [
    { id: "user", label: "User", count: counts.user },
    { id: "community", label: "Community", count: counts.community },
  ];
  return (
    <div
      role="tablist"
      aria-label="Lua script source"
      className="inline-flex items-center gap-1 rounded-full border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-card)_55%,transparent)] p-0.5"
    >
      {items.map((it) => {
        const isActive = active === it.id;
        return (
          <button
            key={it.id}
            role="tab"
            aria-selected={isActive}
            type="button"
            onClick={() => onChange(it.id)}
            className={cn(
              "inline-flex h-6 items-center gap-1.5 rounded-full px-2.5 text-[11.5px] font-medium",
              "transition-colors outline-none",
              isActive
                ? "bg-[var(--color-foreground)] text-[var(--color-background)]"
                : "text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)]",
            )}
          >
            <span>{it.label}</span>
            <span
              className={cn(
                "tabular text-[9.5px]",
                isActive ? "opacity-90" : "opacity-70",
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

function ValidationPill({ validation }: { validation: LuaValidation | undefined }) {
  if (!validation) {
    return (
      <span className="inline-flex items-center gap-1 rounded-full border border-[var(--color-border)] px-1.5 py-[1px] text-[10px] uppercase tracking-[0.16em] text-[var(--color-muted-foreground)]">
        <Loader2 className="h-2.5 w-2.5 animate-spin" strokeWidth={2.5} />
        Parsing
      </span>
    );
  }
  if (validation.ok) {
    return (
      <span className="inline-flex items-center gap-1 rounded-full border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_5%,transparent)] px-1.5 py-[1px] text-[10px] uppercase tracking-[0.16em] text-[var(--color-foreground)]">
        <CheckCircle2 className="h-2.5 w-2.5" strokeWidth={2.5} />
        Valid
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-[var(--color-border)] bg-[color-mix(in_oklch,red_8%,transparent)] px-1.5 py-[1px] text-[10px] uppercase tracking-[0.16em] text-[var(--color-foreground)]">
      <XCircle className="h-2.5 w-2.5" strokeWidth={2.5} />
      {validation.errors.length} error{validation.errors.length === 1 ? "" : "s"}
    </span>
  );
}

function BottomStrip({ validation }: { validation: LuaValidation | undefined }) {
  return (
    <div className="shrink-0 border-t border-[var(--color-border)] px-4 py-2 text-[11px] leading-snug">
      {!validation ? (
        <span className="text-[var(--color-muted-foreground)]">
          Validating…
        </span>
      ) : validation.ok ? (
        <span className="text-[var(--color-muted-foreground)]">Parsed OK</span>
      ) : (
        <ul className="flex flex-col gap-0.5">
          {validation.errors.slice(0, 3).map((e, i) => (
            <li
              key={`${e.line}:${i}`}
              className="tabular text-[var(--color-foreground)]"
            >
              <span className="opacity-60">L{e.line || "?"}</span>{" "}
              <span>{e.message}</span>
            </li>
          ))}
          {validation.errors.length > 3 ? (
            <li className="text-[var(--color-muted-foreground)]">
              +{validation.errors.length - 3} more
            </li>
          ) : null}
        </ul>
      )}
    </div>
  );
}

function EmptyEditor({ source }: { source: LuaSource }) {
  const isCommunity = source === "community";
  return (
    <div className="flex flex-1 items-center justify-center p-8">
      <div className="flex max-w-md flex-col items-center gap-3 text-center">
        <span
          className={cn(
            "grid h-12 w-12 place-items-center rounded-[10px]",
            "bg-[color-mix(in_oklch,var(--color-foreground)_6%,transparent)]",
            "text-[var(--color-foreground)]",
          )}
        >
          <Code2 className="h-5 w-5" strokeWidth={2} />
        </span>
        <h3 className="text-[14px] font-semibold tracking-tight">
          {isCommunity ? "Community Lua scripts" : "No script selected"}
        </h3>
        <p className="text-[12.5px] leading-relaxed text-[var(--color-muted-foreground)]">
          {isCommunity
            ? "Browse community-authored scripts curated on GitHub. Pick one from the sidebar to preview it, then Install to keep a local copy you can run."
            : "Pick a script from the sidebar, or create a new one to start authoring."}
        </p>
        {isCommunity ? (
          <Button
            size="sm"
            variant="outline"
            className="gap-1.5 text-[12px]"
            onClick={() => void openExternal(COMMUNITY_REPO_URL)}
          >
            <Github className="h-3.5 w-3.5" strokeWidth={2.25} />
            Contribute on GitHub
            <ExternalLink className="h-3 w-3 opacity-70" />
          </Button>
        ) : null}
      </div>
    </div>
  );
}

function NotInstalledEditor({
  meta,
  onInstall,
}: {
  meta: LuaScript;
  onInstall: () => void;
}) {
  return (
    <div className="flex flex-1 items-center justify-center p-8">
      <div className="flex max-w-md flex-col items-center gap-3 text-center">
        <span className="grid h-10 w-10 place-items-center rounded-[10px] bg-[color-mix(in_oklch,var(--color-foreground)_6%,transparent)]">
          <Sparkles className="h-4 w-4" strokeWidth={2} />
        </span>
        <h3 className="text-[14px] font-semibold tracking-tight">{meta.name}</h3>
        {meta.author ? (
          <p className="text-[11px] uppercase tracking-[0.18em] text-[var(--color-muted-foreground)]">
            by {meta.author}
          </p>
        ) : null}
        {meta.description ? (
          <p className="text-[12.5px] leading-relaxed text-[var(--color-muted-foreground)]">
            {meta.description}
          </p>
        ) : null}
        <Button
          size="sm"
          className="gap-1.5 text-[12px]"
          onClick={onInstall}
        >
          <Download className="h-3.5 w-3.5" strokeWidth={2.25} />
          Install script
        </Button>
      </div>
    </div>
  );
}
