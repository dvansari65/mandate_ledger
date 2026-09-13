"use client";

import { useState } from "react";

export default function CopyChip({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard unavailable — the text is still selectable */
    }
  };
  return (
    <button type="button" className="chip" onClick={copy}>
      <code>{text}</code>
      <span className="chip-action" aria-live="polite">{copied ? "copied" : "copy"}</span>
    </button>
  );
}
