import * as React from "react";
import { ExternalLink, Gamepad2, MessageSquarePlus, Send } from "lucide-react";

import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { openExternal } from "@/lib/open-external";

const REPO_URL = "https://github.com/satyajiit/openforge-aio";

function buildIssueUrl(title: string, body: string): string {
  const params = new URLSearchParams({
    title: `[new-game] ${title.trim() || "(name your game)"}`,
    body,
    labels: "new-game",
  });
  return `${REPO_URL}/issues/new?${params.toString()}`;
}

function defaultBody(name: string, platform: string, why: string): string {
  return `**Game title:** ${name || "(please fill in)"}

**Platform / store:** ${platform || "(e.g. Steam, GOG, Epic)"}

**Single-player & offline?** Yes — OpenForge only ships trainers for offline single-player titles.

**Why this game / what you'd want:**
${why || "(a few sentences on the cheats you'd love to see — currency, infinite health, fast travel, etc.)"}

**Engine (if known):** UE5 / UE4 / Unity / native / unsure

---
_Filed via the OpenForge desktop app._`;
}

export function RequestGameDialog({ trigger }: { trigger: React.ReactNode }) {
  const [open, setOpen] = React.useState(false);
  const [name, setName] = React.useState("");
  const [platform, setPlatform] = React.useState("");
  const [why, setWhy] = React.useState("");

  React.useEffect(() => {
    if (!open) {
      setName("");
      setPlatform("");
      setWhy("");
    }
  }, [open]);

  const handleSubmit = () => {
    const url = buildIssueUrl(name, defaultBody(name, platform, why));
    void openExternal(url);
    setOpen(false);
  };

  const canSubmit = name.trim().length > 0;

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent className="max-w-md gap-3">
        <DialogHeader>
          <div className="flex items-center gap-2">
            <span className="grid h-7 w-7 place-items-center rounded-[var(--radius)] bg-[var(--color-foreground)]/8">
              <MessageSquarePlus
                className="h-3.5 w-3.5 text-[var(--color-foreground)]"
                strokeWidth={2}
              />
            </span>
            <DialogTitle className="text-[15px]">Request a game</DialogTitle>
          </div>
          <DialogDescription className="text-[12px] leading-relaxed">
            Tell us which offline single-player game you'd like OpenForge to
            support. Submitting opens a pre-filled GitHub issue in your
            browser — no account info is sent from this app.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="rg-name" className="text-[11.5px]">
              Game title
            </Label>
            <Input
              id="rg-name"
              autoFocus
              placeholder="e.g. Hogwarts Legacy"
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="h-8 text-[12.5px]"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="rg-platform" className="text-[11.5px]">
              Platform / store
            </Label>
            <Input
              id="rg-platform"
              placeholder="Steam, GOG, Epic, …"
              value={platform}
              onChange={(e) => setPlatform(e.target.value)}
              className="h-8 text-[12.5px]"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="rg-why" className="text-[11.5px]">
              What cheats would you love? <span className="text-[var(--color-muted-foreground)]">(optional)</span>
            </Label>
            <textarea
              id="rg-why"
              placeholder="Infinite stamina, unlock all skills, fast travel from anywhere…"
              value={why}
              onChange={(e) => setWhy(e.target.value)}
              rows={3}
              className="resize-none rounded-[var(--radius)] border border-[var(--color-input)] bg-transparent px-2.5 py-1.5 text-[12.5px] outline-none focus-visible:border-[var(--color-foreground)]/40 focus-visible:ring-1 focus-visible:ring-[var(--color-ring)]"
            />
          </div>

          <div className="flex items-start gap-2 rounded-[var(--radius)] border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)] p-2.5 text-[11px] leading-relaxed text-[var(--color-muted-foreground)]">
            <Gamepad2
              className="mt-px h-3.5 w-3.5 shrink-0 text-[var(--color-foreground)]"
              strokeWidth={2}
            />
            <span>
              We only ship trainers for <strong>offline single-player</strong>{" "}
              titles. Requests for online or anti-cheat-protected games will be
              politely declined.
            </span>
          </div>
        </div>

        <DialogFooter className="gap-2">
          <DialogClose asChild>
            <Button variant="ghost" size="sm" className="text-[12px]">
              Cancel
            </Button>
          </DialogClose>
          <Button
            size="sm"
            disabled={!canSubmit}
            onClick={handleSubmit}
            className="gap-1.5 text-[12px]"
          >
            <Send className="h-3 w-3" strokeWidth={2.25} />
            Open on GitHub
            <ExternalLink className="h-3 w-3 opacity-70" />
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
