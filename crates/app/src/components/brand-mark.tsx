import { cn } from "@/lib/utils";

/**
 * OpenForge mark. A square anvil-stamp glyph: open outer frame with a
 * struck inner wedge. Geometric, monochrome, scales cleanly 16→32px.
 */
export function BrandMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden
      className={cn("h-4 w-4", className)}
    >
      <rect
        x="2.75"
        y="2.75"
        width="18.5"
        height="18.5"
        rx="2.25"
        stroke="currentColor"
        strokeWidth="1.5"
      />
      <path
        d="M7.5 15.5L12 7.5L16.5 15.5"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="square"
        strokeLinejoin="miter"
      />
      <rect x="9.5" y="14.5" width="5" height="2.25" fill="currentColor" />
    </svg>
  );
}
