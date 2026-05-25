import { AnimatePresence, motion } from "motion/react";
import { Moon, Sun } from "lucide-react";

import { useAppStore } from "@/store/app-store";

const EASE_OUT = [0.16, 1, 0.3, 1] as const;
const EASE_IN = [0.7, 0, 0.84, 0] as const;

export function ThemeTransitionOverlay() {
  const transition = useAppStore((s) => s.themeTransition);
  const goingDark = transition?.direction === "to-dark";
  const OutIcon = goingDark ? Sun : Moon;
  const InIcon = goingDark ? Moon : Sun;

  return (
    <AnimatePresence>
      {transition ? (
        <motion.div
          key="theme-overlay"
          data-slot="theme-transition-overlay"
          aria-hidden
          className="pointer-events-none fixed inset-0 z-[100] grid place-items-center overflow-hidden"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.22, ease: "easeOut" }}
        >
          <div
            className="absolute inset-0"
            style={{
              background:
                "radial-gradient(ellipse at center, color-mix(in oklch, var(--color-background) 100%, transparent) 0%, color-mix(in oklch, var(--color-background) 96%, transparent) 60%, color-mix(in oklch, var(--color-background) 88%, transparent) 100%)",
              backdropFilter: "blur(8px) saturate(120%)",
              WebkitBackdropFilter: "blur(8px) saturate(120%)",
            }}
          />

          <Halo />

          <motion.div
            key="out"
            className="absolute"
            initial={{ y: 0, opacity: 1, scale: 1, rotate: 0 }}
            animate={{
              y: goingDark ? "-65vh" : "65vh",
              opacity: 0,
              scale: 0.55,
              rotate: goingDark ? -28 : 28,
            }}
            transition={{ duration: 0.48, ease: EASE_IN }}
          >
            <IconBadge>
              <OutIcon
                strokeWidth={1.6}
                className="h-20 w-20 text-[var(--color-foreground)]"
              />
            </IconBadge>
          </motion.div>

          <motion.div
            key="in"
            className="absolute"
            initial={{
              y: goingDark ? "65vh" : "-65vh",
              opacity: 0,
              scale: 0.55,
              rotate: goingDark ? 28 : -28,
            }}
            animate={{ y: 0, opacity: 1, scale: 1, rotate: 0 }}
            transition={{ duration: 0.62, ease: EASE_OUT, delay: 0.28 }}
          >
            <IconBadge>
              <InIcon
                strokeWidth={1.6}
                className="h-20 w-20 text-[var(--color-foreground)]"
              />
            </IconBadge>
          </motion.div>
        </motion.div>
      ) : null}
    </AnimatePresence>
  );
}

function IconBadge({ children }: { children: React.ReactNode }) {
  return (
    <div
      className="grid h-36 w-36 place-items-center rounded-full"
      style={{
        background:
          "radial-gradient(circle at 50% 35%, color-mix(in oklch, var(--color-foreground) 8%, transparent), transparent 70%)",
        boxShadow:
          "0 0 0 1px color-mix(in oklch, var(--color-foreground) 10%, transparent) inset",
      }}
    >
      {children}
    </div>
  );
}

function Halo() {
  return (
    <motion.div
      aria-hidden
      className="absolute h-[60vmin] w-[60vmin] rounded-full"
      style={{
        background:
          "radial-gradient(circle, color-mix(in oklch, var(--color-foreground) 6%, transparent) 0%, transparent 60%)",
      }}
      initial={{ scale: 0.6, opacity: 0 }}
      animate={{ scale: 1.4, opacity: 1 }}
      exit={{ scale: 0.6, opacity: 0 }}
      transition={{ duration: 0.9, ease: EASE_OUT }}
    />
  );
}
