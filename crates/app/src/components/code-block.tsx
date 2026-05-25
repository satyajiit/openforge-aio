import { Check, Copy } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

interface CodeBlockProps {
  code: string;
  language?: string;
  className?: string;
}

export function CodeBlock({ code, language, className }: CodeBlockProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      // Clipboard access may be denied in some webviews; silent fail is fine.
    }
  };

  return (
    <div
      data-slot="code-block"
      className={cn(
        "group/code relative rounded-[var(--radius)] border border-[var(--color-border)]",
        "bg-[color-mix(in_oklch,var(--color-foreground)_4%,transparent)]",
        "font-mono text-[11.5px] leading-relaxed",
        className,
      )}
    >
      {language ? (
        <div className="flex items-center justify-between border-b border-[var(--color-border)]/50 px-2.5 py-1 text-[9.5px] uppercase tracking-[0.18em] text-[var(--color-muted-foreground)]">
          <span>{language}</span>
        </div>
      ) : null}
      <pre className="allow-select overflow-x-auto whitespace-pre-wrap break-all px-2.5 py-2 pr-9">
        <code className="allow-select">{code}</code>
      </pre>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        aria-label={copied ? "Copied" : "Copy to clipboard"}
        title={copied ? "Copied" : "Copy"}
        onClick={handleCopy}
        className={cn(
          "absolute right-1 top-1 h-6 w-6 opacity-60 transition-opacity",
          "hover:opacity-100 group-hover/code:opacity-100",
        )}
      >
        {copied ? (
          <Check className="h-3 w-3" strokeWidth={2.5} />
        ) : (
          <Copy className="h-3 w-3" strokeWidth={2} />
        )}
      </Button>
    </div>
  );
}
