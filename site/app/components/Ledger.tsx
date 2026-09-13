"use client";

import { useEffect, useState } from "react";

type Stage = "authorized" | "paid" | "settled" | "delivered";
const STAGES: Stage[] = ["authorized", "paid", "settled", "delivered"];
const BUDGET = 8000;

type Step =
  | { kind: "ok"; ctx: string; event: Stage; detail: string; reserve?: number }
  | { kind: "denied"; ctx: string; code: string; detail: string }
  | { kind: "pending"; text: string };

/** One scripted run. Same rows the Rust engine writes; hashes are chained per context. */
const SCRIPT: Step[] = [
  { kind: "ok", ctx: "ctx_5919", event: "authorized", detail: "bigbasket.com · ₹128.00 · under mnd_8f2a", reserve: 128 },
  { kind: "ok", ctx: "ctx_5919", event: "paid", detail: "pay_Nx7 · amount matches · nonce consumed" },
  { kind: "pending", text: "need 1 confirmation, have 0" },
  { kind: "ok", ctx: "ctx_5919", event: "settled", detail: "pay_Nx7@3 · finality reached" },
  { kind: "ok", ctx: "ctx_5919", event: "delivered", detail: "BB-88121 · merchant-signed receipt" },
  { kind: "denied", ctx: "ctx_a41c", code: "SCOPE_MERCHANT_MISMATCH", detail: "amazon.in is not in the mandate" },
  { kind: "denied", ctx: "ctx_7be0", code: "NONCE_ALREADY_USED", detail: "pay_Nx7 already paid ctx_5919" },
];
const STEP_MS = 1050;

type Row = { seq: number; ctx: string; kind: "ok" | "denied"; event: string; detail: string; code?: string; hash: string };

async function sha256(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

export default function Ledger() {
  const [rows, setRows] = useState<Row[]>([]);
  const [pending, setPending] = useState<string | null>(null);
  const [stage, setStage] = useState<Stage | null>(null);
  const [reserved, setReserved] = useState(0);
  const [done, setDone] = useState(false);
  const [run, setRun] = useState(0);

  useEffect(() => {
    let cancelled = false;
    const timers: number[] = [];
    const instant = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    setRows([]); setPending(null); setStage(null); setReserved(0); setDone(false);

    const chain: Record<string, string> = {};
    let seq = 0;

    const apply = async (step: Step) => {
      if (step.kind === "pending") { setPending(step.text); return; }
      seq += 1;
      const prev = chain[step.ctx] ?? "0".repeat(64);
      const body = step.kind === "ok" ? `${step.event}|${step.detail}` : `denied|${step.code}|${step.detail}`;
      const hash = await sha256(`${prev}|${seq}|${step.ctx}|${body}`);
      if (cancelled) return;
      chain[step.ctx] = hash;
      setPending(null);
      setRows((r) => [...r, { seq, ctx: step.ctx, kind: step.kind, event: step.kind === "ok" ? step.event : "denied", detail: step.detail, code: step.kind === "denied" ? step.code : undefined, hash }]);
      if (step.kind === "ok") {
        setStage(step.event);
        if (step.reserve) setReserved((v) => v + step.reserve!);
      }
    };

    SCRIPT.forEach((step, i) => {
      timers.push(window.setTimeout(() => void apply(step), instant ? 0 : i * STEP_MS));
    });
    timers.push(window.setTimeout(() => setDone(true), instant ? 50 : SCRIPT.length * STEP_MS));

    return () => { cancelled = true; timers.forEach(clearTimeout); };
  }, [run]);

  const reached = (s: Stage) => stage !== null && STAGES.indexOf(s) <= STAGES.indexOf(stage);
  const chipState = (s: Stage) => (reached(s) ? "on" : pending && s === "settled" ? "wait" : "off");

  return (
    <div className="window ledger-window">
      <div className="window-bar">
        <span className="dots" aria-hidden="true"><i /><i /><i /></span>
        <span>ledger · mnd_8f2a · mock rail</span>
        <button className="replay" onClick={() => setRun((n) => n + 1)} disabled={!done}>Replay</button>
      </div>
      <div className="ledger-status">
        <ol className="chips" aria-label="Lifecycle state">
          {STAGES.map((s) => <li key={s} data-state={chipState(s)}>{s}</li>)}
        </ol>
        <div className="budget">
          <span>reserved</span>
          <strong>₹{reserved.toLocaleString("en-IN")}</strong>
          <span>/ ₹{BUDGET.toLocaleString("en-IN")}</span>
          <span className="meter" aria-hidden="true"><i style={{ width: `${(reserved / BUDGET) * 100}%` }} /></span>
          {pending && <span className="pending">pending · {pending}</span>}
        </div>
      </div>

      <div className="ledger" role="table" aria-label="Ledger events">
        <div className="ledger-head" role="row">
          <span>seq</span><span className="ctx">context</span><span>event</span><span>detail</span><span className="hash">hash</span>
        </div>
        {rows.length === 0 && (
          <div className="row empty" role="row">
            <span className="seq">—</span><span className="ctx" /><span className="event">waiting</span>
            <span className="detail">cart received: bigbasket.com · ₹128.00</span><span className="hash" />
          </div>
        )}
        {rows.map((r) => (
          <div key={r.seq} className="row" role="row" data-kind={r.kind}>
            <span className="seq">{r.seq}</span>
            <span className="ctx">{r.ctx}</span>
            <span className="event">{r.event}</span>
            <span className="detail">{r.code ? <><code>{r.code}</code> · {r.detail}</> : r.detail}</span>
            <span className="hash" title={`sha256:${r.hash}`}>sha256:{r.hash.slice(0, 18)}…</span>
          </div>
        ))}
      </div>
    </div>
  );
}
