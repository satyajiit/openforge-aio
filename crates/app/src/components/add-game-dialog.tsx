import * as React from "react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
  DialogClose,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";

export function AddGameDialog({ trigger }: { trigger: React.ReactNode }) {
  return (
    <Dialog>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add a game</DialogTitle>
          <DialogDescription>
            OpenForge is community-extensible. Each game is a Rust crate under{" "}
            <code>crates/games/&lt;id&gt;/</code> with a manifest, signatures,
            and an icon.
          </DialogDescription>
        </DialogHeader>
        <div className="text-sm text-[var(--color-muted-foreground)]">
          To add a new game:
          <ol className="ml-4 mt-2 list-decimal space-y-1">
            <li>Fork the repository.</li>
            <li>
              Run <code>cargo run -p openforge-cli -- new-game --id &lt;slug&gt; --name "Your Game"</code>.
            </li>
            <li>
              Use <code>openforge-discover</code> to find a memory address and emit a signature.
            </li>
            <li>Open a pull request.</li>
          </ol>
        </div>
        <DialogFooter>
          <DialogClose asChild>
            <Button variant="outline">Close</Button>
          </DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
