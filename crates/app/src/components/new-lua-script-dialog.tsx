import * as React from "react";
import { FilePlus2, Loader2 } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

export type NewLuaScriptDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  existingSlugs: string[];
  onCreate: (slug: string, name: string) => Promise<void>;
};

function slugify(s: string): string {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9_-\s]/g, "")
    .trim()
    .replace(/\s+/g, "-")
    .slice(0, 64);
}

function sanitizeSlug(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9_-]/g, "").slice(0, 64);
}

export function NewLuaScriptDialog({
  open,
  onOpenChange,
  existingSlugs,
  onCreate,
}: NewLuaScriptDialogProps) {
  const [name, setName] = React.useState("");
  const [slug, setSlug] = React.useState("");
  const [slugDirty, setSlugDirty] = React.useState(false);
  const [submitting, setSubmitting] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);

  // Reset state every time the dialog opens.
  React.useEffect(() => {
    if (open) {
      setName("");
      setSlug("");
      setSlugDirty(false);
      setSubmitting(false);
      setError(null);
    }
  }, [open]);

  const handleNameChange = (next: string) => {
    setName(next);
    if (!slugDirty) {
      setSlug(slugify(next));
    }
  };

  const handleSlugChange = (next: string) => {
    setSlug(sanitizeSlug(next));
    setSlugDirty(true);
  };

  const trimmedName = name.trim();
  const slugConflict = existingSlugs.includes(slug);
  const slugEmpty = slug.length === 0;
  const nameEmpty = trimmedName.length === 0;
  const canSubmit = !nameEmpty && !slugEmpty && !slugConflict && !submitting;

  const submit = async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    setError(null);
    try {
      await onCreate(slug, trimmedName);
      onOpenChange(false);
    } catch (e) {
      const err = e as { message?: string };
      setError(err.message ?? String(e));
      setSubmitting(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && canSubmit) {
      e.preventDefault();
      void submit();
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="max-w-md gap-3 outline-none"
        onKeyDown={handleKeyDown}
      >
        <DialogHeader className="space-y-1">
          <DialogTitle className="flex items-center gap-2 text-[15px]">
            <FilePlus2 className="h-4 w-4" strokeWidth={2} />
            New Lua script
          </DialogTitle>
          <DialogDescription className="text-[12px]">
            Scripts are saved per game under your local data folder.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="lua-new-name" className="text-[11.5px]">
              Display name
            </Label>
            <Input
              id="lua-new-name"
              value={name}
              autoFocus
              onChange={(e) => handleNameChange(e.target.value)}
              placeholder="Give cops every weapon"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="lua-new-slug" className="text-[11.5px]">
              Slug
            </Label>
            <Input
              id="lua-new-slug"
              value={slug}
              onChange={(e) => handleSlugChange(e.target.value)}
              placeholder="give-cops-every-weapon"
              className="tabular text-[12.5px]"
              spellCheck={false}
            />
            <p className="text-[10.5px] text-[var(--color-muted-foreground)]">
              File name on disk. Lowercase letters, digits, dashes and
              underscores only.
            </p>
          </div>

          {slugConflict ? (
            <div
              className={cn(
                "rounded-[var(--radius)] border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)]",
                "px-3 py-2 text-[12px] leading-snug",
              )}
            >
              A script with slug{" "}
              <span className="font-medium text-[var(--color-foreground)]">
                {slug}
              </span>{" "}
              already exists. Choose a different slug.
            </div>
          ) : null}

          {error ? (
            <div
              className={cn(
                "rounded-[var(--radius)] border border-[var(--color-border)] bg-[color-mix(in_oklch,red_8%,transparent)]",
                "px-3 py-2 text-[12px] leading-snug",
              )}
            >
              {error}
            </div>
          ) : null}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-[var(--color-border)]/60 pt-3">
          <Button
            variant="ghost"
            size="sm"
            className="text-[12px]"
            onClick={() => onOpenChange(false)}
            disabled={submitting}
          >
            Cancel
          </Button>
          <Button
            size="sm"
            className="gap-1.5 text-[12px]"
            onClick={() => void submit()}
            disabled={!canSubmit}
          >
            {submitting ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" strokeWidth={2.25} />
            ) : (
              <FilePlus2 className="h-3.5 w-3.5" strokeWidth={2.25} />
            )}
            Create
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
