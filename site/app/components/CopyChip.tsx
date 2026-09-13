"use client";

import { useState } from "react";
import Icon from "./Icon";

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
    <button type="button" className="chip" onClick={copy} aria-live="polite">
      <span className="chip-prompt">$</span>
      <code>{text}</code>
      <span className="chip-icon">{copied ? <Icon name="check" size={15} /> : <Icon name="copy" size={15} />}</span>
      <span className="sr-only">{copied ? "Copied" : "Copy to clipboard"}</span>
    </button>
  );
}
