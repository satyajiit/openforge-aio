import * as React from "react";
import {
  ArrowLeft,
  ArrowRight,
  CheckCircle2,
  ExternalLink,
  GitBranch,
  GitFork,
  GitPullRequest,
  Rocket,
  Sparkles,
  Terminal,
} from "lucide-react";

import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/code-block";
import { cn } from "@/lib/utils";
import { openExternal } from "@/lib/open-external";

const REPO_URL = "https://github.com/satyajiit/openforge-aio";
const FORK_URL = `${REPO_URL}/fork`;
const COMPARE_URL = `${REPO_URL}/compare`;
const AUTHORING_URL = `${REPO_URL}/blob/main/docs/GAME-AUTHORING.md`;
const COOKBOOK_URL = `${REPO_URL}/blob/main/docs/UE5-CHEAT-COOKBOOK.md`;
const CONTRIBUTING_URL = `${REPO_URL}/blob/main/docs/CONTRIBUTING.md`;

interface Step {
  id: string;
  title: string;
  icon: React.ElementType;
  body: React.ReactNode;
}

function StepperDots({ current, total }: { current: number; total: number }) {
  return (
    <div
      data-slot="stepper"
      className="flex items-center justify-center gap-1.5"
      aria-label={`Step ${current + 1} of ${total}`}
    >
      {Array.from({ length: total }).map((_, idx) => (
        <span
          key={idx}
          aria-current={idx === current ? "step" : undefined}
          className={cn(
            "h-1.5 rounded-full transition-all",
            idx === current
              ? "w-6 bg-[var(--color-foreground)]"
              : idx < current
                ? "w-1.5 bg-[var(--color-foreground)]/60"
                : "w-1.5 bg-[var(--color-border)]",
          )}
        />
      ))}
    </div>
  );
}

function StepShell({
  icon: Icon,
  title,
  children,
}: {
  icon: React.ElementType;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div data-slot="step" className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <span className="grid h-7 w-7 place-items-center rounded-[var(--radius)] bg-[var(--color-foreground)]/8 text-[var(--color-foreground)]">
          <Icon className="h-3.5 w-3.5" strokeWidth={2} />
        </span>
        <h3 className="text-[13px] font-semibold leading-none">{title}</h3>
      </div>
      <div className="flex flex-col gap-3 text-[12.5px] leading-relaxed text-[var(--color-muted-foreground)]">
        {children}
      </div>
    </div>
  );
}

const STEPS: Step[] = [
  {
    id: "welcome",
    title: "Add a game to OpenForge",
    icon: Sparkles,
    body: (
      <StepShell icon={Sparkles} title="Welcome, future contributor">
        <p>
          Every supported game in OpenForge is a folder. No engine forks, no
          monkey-patches — just a manifest, a handful of TOML signatures, and
          a per-game DLL if the game is UE5.
        </p>
        <p>
          This wizard walks you through the five steps to ship a new game. You
          can drop out at any point — every step links to a GitHub or docs page
          you can reopen later.
        </p>
        <div className="rounded-[var(--radius)] border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)] p-3 text-[11.5px]">
          <strong className="text-[var(--color-foreground)]">
            Heads up:
          </strong>{" "}
          OpenForge only ships trainers for{" "}
          <em>offline single-player</em> games. No online / PvP / anti-cheat
          titles. We will close PRs that try, regardless of how cool the cheat
          is.
        </div>
      </StepShell>
    ),
  },
  {
    id: "fork",
    title: "Fork the repo",
    icon: GitFork,
    body: (
      <StepShell icon={GitFork} title="Fork OpenForge on GitHub">
        <p>
          Fork the repo into your own GitHub account. You'll commit your new
          game to a branch on the fork, then open a pull request back here.
        </p>
        <Button
          variant="default"
          size="sm"
          className="self-start gap-2 text-[12px]"
          onClick={() => openExternal(FORK_URL)}
        >
          <GitFork className="h-3.5 w-3.5" />
          Fork on GitHub
          <ExternalLink className="h-3 w-3 opacity-70" />
        </Button>
        <p className="text-[11.5px]">
          Already forked? Skip ahead — no harm done.
        </p>
      </StepShell>
    ),
  },
  {
    id: "clone",
    title: "Clone & build",
    icon: Terminal,
    body: (
      <StepShell icon={Terminal} title="Clone your fork and verify it builds">
        <p>
          Replace <code className="text-[var(--color-foreground)]">YOU</code>{" "}
          with your GitHub username:
        </p>
        <CodeBlock
          language="powershell"
          code={"git clone https://github.com/YOU/openforge-aio.git\ncd openforge-aio\ncargo check --workspace"}
        />
        <p>
          If <code>cargo check</code> is green, you're set. If not, double-check
          your Rust toolchain is <code>1.95+</code> (edition 2024) and the
          frontend deps are installed:
        </p>
        <CodeBlock
          language="powershell"
          code={"cd crates/app\nnpx --yes pnpm@10 install"}
        />
      </StepShell>
    ),
  },
  {
    id: "scaffold",
    title: "Scaffold the new game",
    icon: Rocket,
    body: (
      <StepShell icon={Rocket} title="Generate the game crate">
        <p>
          The scaffolder copies the template, wires the crate into the bundle,
          and prints the next steps. Pick a short slug for{" "}
          <code className="text-[var(--color-foreground)]">&lt;slug&gt;</code>{" "}
          (lowercase + hyphens) and a friendly display name.
        </p>
        <CodeBlock
          language="powershell"
          code={'cargo run -p openforge-cli -- new-game --id <slug> --name "Your Game"'}
        />
        <p>
          The new crate lands at{" "}
          <code className="text-[var(--color-foreground)]">
            crates/games/&lt;slug&gt;/
          </code>{" "}
          with an empty <code>signatures/</code> folder.
        </p>
        <div className="rounded-[var(--radius)] border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)] p-3 text-[11.5px]">
          <strong className="text-[var(--color-foreground)]">Tip:</strong> add
          a 64×64 monochrome icon at{" "}
          <code>crates/games/&lt;slug&gt;/assets/icon.png</code>. The sidebar
          will use it.
        </div>
      </StepShell>
    ),
  },
  {
    id: "discover",
    title: "Discover signatures",
    icon: GitBranch,
    body: (
      <StepShell icon={GitBranch} title="Find addresses + author cheats">
        <p>
          Launch the game and use the discovery CLI to find writable memory or
          UE5 reflection targets. Each cheat becomes one{" "}
          <code className="text-[var(--color-foreground)]">.toml</code> file.
        </p>
        <CodeBlock
          language="powershell"
          code={"cargo run -p openforge-discover -- --game <slug> doctor\ncargo run -p openforge-discover -- --game <slug> attach\ncargo run -p openforge-discover -- --game <slug> scan --feature gold --type i32 --value 12345 --name \"Gold\""}
        />
        <p>Then narrow → pick → extract-aob → emit. Or for UE5 games, skip straight to reflection.</p>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            size="sm"
            className="gap-2 text-[12px]"
            onClick={() => openExternal(AUTHORING_URL)}
          >
            GAME-AUTHORING.md
            <ExternalLink className="h-3 w-3 opacity-70" />
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="gap-2 text-[12px]"
            onClick={() => openExternal(COOKBOOK_URL)}
          >
            UE5 cookbook
            <ExternalLink className="h-3 w-3 opacity-70" />
          </Button>
        </div>
      </StepShell>
    ),
  },
  {
    id: "pr",
    title: "Open a Pull Request",
    icon: GitPullRequest,
    body: (
      <StepShell icon={GitPullRequest} title="Ship it">
        <p>Before you push, run the CI gates locally:</p>
        <CodeBlock
          language="powershell"
          code={"cargo fmt --all -- --check\ncargo clippy --workspace --all-targets -- -D warnings\ncargo test --workspace --lib\ncargo run -p openforge-cli -- verify-registry"}
        />
        <p>Then push the branch and open a PR:</p>
        <CodeBlock
          language="powershell"
          code={'git checkout -b add-<slug>\ngit add -A\ngit commit -m "Add <slug> game support"\ngit push -u origin add-<slug>'}
        />
        <div className="flex flex-wrap gap-2">
          <Button
            variant="default"
            size="sm"
            className="gap-2 text-[12px]"
            onClick={() => openExternal(COMPARE_URL)}
          >
            <GitPullRequest className="h-3.5 w-3.5" />
            Open Pull Request
            <ExternalLink className="h-3 w-3 opacity-70" />
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="gap-2 text-[12px]"
            onClick={() => openExternal(CONTRIBUTING_URL)}
          >
            Contributing guide
            <ExternalLink className="h-3 w-3 opacity-70" />
          </Button>
        </div>
        <div className="flex items-start gap-2 rounded-[var(--radius)] border border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-foreground)_3%,transparent)] p-3 text-[11.5px]">
          <CheckCircle2
            className="mt-px h-3.5 w-3.5 shrink-0 text-[var(--color-foreground)]"
            strokeWidth={2.25}
          />
          <span>
            That's the whole flow. We'll review within a few days — be patient,
            we're a small project.
          </span>
        </div>
      </StepShell>
    ),
  },
];

export function AddGameDialog({ trigger }: { trigger: React.ReactNode }) {
  const [open, setOpen] = React.useState(false);
  const [step, setStep] = React.useState(0);

  React.useEffect(() => {
    if (!open) setStep(0);
  }, [open]);

  const current = STEPS[step] ?? STEPS[0]!;
  const isFirst = step === 0;
  const isLast = step === STEPS.length - 1;

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent className="max-w-xl gap-3">
        <DialogHeader className="space-y-2">
          <DialogTitle className="text-[15px]">{current.title}</DialogTitle>
          <DialogDescription className="sr-only">
            Step-by-step wizard for adding a new game to OpenForge.
          </DialogDescription>
          <StepperDots current={step} total={STEPS.length} />
        </DialogHeader>

        <div className="min-h-[260px]">{current.body}</div>

        <div
          data-slot="step-footer"
          className="flex items-center justify-between gap-2 border-t border-[var(--color-border)]/60 pt-3"
        >
          <span className="text-[10.5px] uppercase tracking-[0.18em] text-[var(--color-muted-foreground)]">
            Step {step + 1} of {STEPS.length}
          </span>
          <div className="flex items-center gap-1.5">
            {isFirst ? (
              <DialogClose asChild>
                <Button variant="ghost" size="sm" className="text-[12px]">
                  Maybe later
                </Button>
              </DialogClose>
            ) : (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setStep((s) => Math.max(0, s - 1))}
                className="gap-1 text-[12px]"
              >
                <ArrowLeft className="h-3 w-3" strokeWidth={2.25} />
                Back
              </Button>
            )}
            {isLast ? (
              <DialogClose asChild>
                <Button size="sm" className="gap-1 text-[12px]">
                  Done
                  <CheckCircle2 className="h-3 w-3" strokeWidth={2.25} />
                </Button>
              </DialogClose>
            ) : (
              <Button
                size="sm"
                onClick={() => setStep((s) => Math.min(STEPS.length - 1, s + 1))}
                className="gap-1 text-[12px]"
              >
                Next
                <ArrowRight className="h-3 w-3" strokeWidth={2.25} />
              </Button>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
