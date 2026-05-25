/**
 * Tier color mapping. This is the ONE intentional break from the otherwise
 * pure-monochrome system — tier tags get a small accent dot so users can
 * recognize a feature's category at-a-glance.
 *
 * Colors picked in OKLCH at moderate chroma (~0.12) and lightness 70–78 so
 * they read well on both light and dark backgrounds without dominating.
 */

export type TierColor = {
  /** Solid color, used for the accent dot. */
  fg: string;
  /** Same hue, transparent — used for the tag background tint. */
  bg: string;
};

export const TIER_COLORS: Record<string, TierColor> = {
  currency: {
    fg: "oklch(76% 0.13 80)",
    bg: "color-mix(in oklch, oklch(76% 0.13 80) 14%, transparent)",
  },
  combat: {
    fg: "oklch(70% 0.16 25)",
    bg: "color-mix(in oklch, oklch(70% 0.16 25) 14%, transparent)",
  },
  progression: {
    fg: "oklch(70% 0.13 255)",
    bg: "color-mix(in oklch, oklch(70% 0.13 255) 14%, transparent)",
  },
  movement: {
    fg: "oklch(72% 0.13 165)",
    bg: "color-mix(in oklch, oklch(72% 0.13 165) 14%, transparent)",
  },
  "character movement": {
    fg: "oklch(74% 0.14 140)",
    bg: "color-mix(in oklch, oklch(74% 0.14 140) 14%, transparent)",
  },
  teleportation: {
    fg: "oklch(72% 0.14 195)",
    bg: "color-mix(in oklch, oklch(72% 0.14 195) 14%, transparent)",
  },
  "combat-depth": {
    fg: "oklch(68% 0.16 305)",
    bg: "color-mix(in oklch, oklch(68% 0.16 305) 14%, transparent)",
  },
  world: {
    // Warm amber — distinct from currency's gold (hue 80) and combat's red
    // (hue 25). Reads as "environment / ambient / city" without competing
    // with the alert-y combat reds or the wealth-y currency yellows.
    fg: "oklch(72% 0.13 50)",
    bg: "color-mix(in oklch, oklch(72% 0.13 50) 14%, transparent)",
  },
  utility: {
    fg: "oklch(74% 0.10 200)",
    bg: "color-mix(in oklch, oklch(74% 0.10 200) 14%, transparent)",
  },
  general: {
    fg: "oklch(55% 0 0)",
    bg: "color-mix(in oklch, oklch(55% 0 0) 14%, transparent)",
  },
};

const FALLBACK: TierColor = {
  fg: "oklch(55% 0 0)",
  bg: "color-mix(in oklch, oklch(55% 0 0) 14%, transparent)",
};

export function tierColor(tier: string): TierColor {
  return TIER_COLORS[tier.toLowerCase().trim()] ?? FALLBACK;
}
