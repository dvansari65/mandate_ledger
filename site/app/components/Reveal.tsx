"use client";

import { motion, useReducedMotion } from "motion/react";
import type { ReactNode } from "react";

type Props = {
  children: ReactNode;
  delay?: number;
  className?: string;
  /** `mount`: animate as soon as it renders (above the fold). `view`: when scrolled into view. */
  mode?: "mount" | "view";
};

/** Fades and lifts its children into place once. */
export default function Reveal({ children, delay = 0, className, mode = "view" }: Props) {
  const reduced = useReducedMotion();
  const from = reduced ? false : { opacity: 0, y: 18 };
  const to = { opacity: 1, y: 0 };
  const transition = { duration: 0.7, delay, ease: [0.2, 0.7, 0.2, 1] as const };
  if (mode === "mount") {
    return <motion.div className={className} initial={from} animate={to} transition={transition}>{children}</motion.div>;
  }
  return (
    <motion.div className={className} initial={from} whileInView={to} viewport={{ once: true, amount: 0.2 }} transition={transition}>
      {children}
    </motion.div>
  );
}
