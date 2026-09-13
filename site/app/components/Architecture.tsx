"use client";

import { useEffect, useRef } from "react";

/** One loop of the story, in seconds. */
const LOOP = 10;

/** Routes in viewBox units. Packets ride the same lines the diagram draws. */
const ROUTES = {
  mandate: "M230 172 H380",
  cart: "M230 352 H280 V172 H380",
  gate: "M380 172 V352 H440",
  toPaid: "M440 352 H560",
  rail: "M560 367 V530 H680 V367",
  toSettled: "M560 352 H680",
  toDelivered: "M680 352 H800",
  toMerchant: "M800 352 H980",
  toScope: "M380 172 V222",
  bounce: "M380 222 V172 H280 V352 H230",
} as const;
type RouteId = keyof typeof ROUTES;

/** A packet rides `route` between two moments; it rests at the end unless hidden. */
type Move = { pkt: string; route: RouteId; from: number; to: number; hideAfter?: boolean };
/** `cls` is on `el` between two moments (until the loop ends if `to` is omitted). */
type Flag = { el: string; cls: string; from: number; to?: number };

const MOVES: Move[] = [
  { pkt: "p-mandate", route: "mandate", from: 0.0, to: 0.8, hideAfter: true },
  { pkt: "p-cart", route: "cart", from: 0.2, to: 1.0 },
  { pkt: "p-cart", route: "gate", from: 1.0, to: 2.1 },
  { pkt: "p-rail", route: "rail", from: 2.3, to: 3.6, hideAfter: true },
  { pkt: "p-cart", route: "toPaid", from: 2.95, to: 3.3 },
  { pkt: "p-cart", route: "toSettled", from: 3.7, to: 4.1 },
  { pkt: "p-cart", route: "toDelivered", from: 4.1, to: 4.5 },
  { pkt: "p-cart", route: "toMerchant", from: 4.5, to: 5.1, hideAfter: true },
  { pkt: "p-bad", route: "cart", from: 6.0, to: 6.8 },
  { pkt: "p-bad", route: "toScope", from: 6.8, to: 7.2 },
  { pkt: "p-bad", route: "bounce", from: 7.4, to: 8.3, hideAfter: true },
];

const FLAGS: Flag[] = [
  { el: "n-sig", cls: "ok", from: 1.3 },
  { el: "n-scope", cls: "ok", from: 1.6, to: 7.2 },
  { el: "n-budget", cls: "ok", from: 1.9 },
  { el: "s-authorized", cls: "ok", from: 2.1 },
  { el: "rail", cls: "ok", from: 2.9 },
  { el: "s-paid", cls: "ok", from: 2.95 },
  { el: "s-settled", cls: "wait", from: 3.3, to: 3.7 },
  { el: "s-settled", cls: "ok", from: 3.7 },
  { el: "s-delivered", cls: "ok", from: 4.5 },
  { el: "merchant", cls: "ok", from: 5.1 },
  { el: "n-scope", cls: "bad", from: 7.2 },
  { el: "deny", cls: "on", from: 7.2 },
];

const PACKETS = [...new Set(MOVES.map((m) => m.pkt))];
const ease = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);

export default function Architecture() {
  const ref = useRef<SVGSVGElement>(null);

  useEffect(() => {
    const svg = ref.current;
    if (!svg) return;
    const byId = (id: string) => svg.querySelector<SVGGraphicsElement>(`#${id}`);
    const routes = new Map<RouteId, SVGPathElement>();
    for (const id of Object.keys(ROUTES) as RouteId[]) {
      const p = byId(`r-${id}`);
      if (p instanceof SVGPathElement) routes.set(id, p);
    }

    const render = (t: number) => {
      for (const pkt of PACKETS) {
        const el = byId(pkt);
        if (!el) continue;
        let cur: Move | undefined;
        for (const m of MOVES) if (m.pkt === pkt && m.from <= t) cur = m;
        const path = cur && routes.get(cur.route);
        if (!cur || !path || (t >= cur.to && cur.hideAfter)) {
          el.classList.add("hidden");
          continue;
        }
        const p = t >= cur.to ? 1 : ease((t - cur.from) / (cur.to - cur.from));
        const pt = path.getPointAtLength(p * path.getTotalLength());
        el.setAttribute("transform", `translate(${pt.x.toFixed(1)} ${pt.y.toFixed(1)})`);
        el.classList.remove("hidden");
      }
      for (const f of FLAGS) byId(f.el)?.classList.toggle(f.cls, t >= f.from && t < (f.to ?? Infinity));
    };

    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      render(7.25);
      return;
    }

    let raf = 0;
    let start = 0;
    let running = false;
    const tick = (now: number) => {
      if (!start) start = now;
      render(((now - start) / 1000) % LOOP);
      raf = requestAnimationFrame(tick);
    };
    const io = new IntersectionObserver(
      ([entry]) => {
        const visible = entry?.isIntersecting ?? false;
        if (visible && !running) {
          running = true;
          raf = requestAnimationFrame(tick);
        } else if (!visible && running) {
          running = false;
          cancelAnimationFrame(raf);
        }
      },
      { threshold: 0.2 },
    );
    io.observe(svg);
    return () => {
      io.disconnect();
      cancelAnimationFrame(raf);
    };
  }, []);

  const pill = (id: string, x: number, label: string) => (
    <g id={id} className="pill">
      <rect x={x} y="336" width="110" height="32" rx="2" />
      <text x={x + 55} y="357" textAnchor="middle">{label}</text>
    </g>
  );
  const node = (id: string, y: number, label: string) => (
    <g id={id} className="node">
      <circle cx="380" cy={y} r="9" />
      <text x="400" y={y + 5}>{label}</text>
    </g>
  );

  return (
    <div className="arch">
      <svg ref={ref} viewBox="0 0 1200 580" role="img" aria-labelledby="arch-title arch-desc">
        <title id="arch-title">How mandate-ledger sits between the agent and the rail</title>
        <desc id="arch-desc">
          A mandate from the wallet and a cart from the agent enter the gate. Signature, scope and budget pass. The
          payment round-trips through the rail; settlement waits, then confirms; delivery unlocks the merchant. A
          second cart fails the scope check and is bounced back with SCOPE_MERCHANT_MISMATCH.
        </desc>

        <g className="routes">
          {(Object.keys(ROUTES) as RouteId[]).map((id) => <path key={id} id={`r-${id}`} d={ROUTES[id]} />)}
        </g>

        <text x="296" y="162" className="t-sm">mandate</text>
        <text x="290" y="270" className="t-sm">cart</text>
        <text x="572" y="458" className="t-sm">proof</text>
        <text x="692" y="458" className="t-sm">finality</text>

        <g id="wallet" className="box">
          <rect x="40" y="140" width="190" height="64" rx="2" />
          <text x="56" y="168" className="t-b">Wallet</text>
          <text x="56" y="190" className="t-sm">signs the mandate</text>
        </g>
        <g id="agent" className="box">
          <rect x="40" y="320" width="190" height="64" rx="2" />
          <text x="56" y="348" className="t-b">Agent</text>
          <text x="56" y="370" className="t-sm">builds the cart</text>
        </g>
        <g id="merchant" className="box">
          <rect x="980" y="320" width="190" height="64" rx="2" />
          <text x="996" y="348" className="t-b">Merchant</text>
          <text x="996" y="370" className="t-sm">fulfils on Settled</text>
        </g>

        <g id="ledger" className="frame">
          <rect x="320" y="80" width="600" height="400" rx="2" />
          <text x="344" y="114" className="t-b">mandate-ledger</text>
          <text x="896" y="114" className="t-sm" textAnchor="end">does not move money</text>

          <text x="344" y="146" className="t-xs">gate</text>
          {node("n-sig", 172, "signature")}
          {node("n-scope", 222, "scope")}
          {node("n-budget", 272, "budget")}
          <g id="deny" className="deny">
            <rect x="480" y="208" width="250" height="28" rx="2" />
            <text x="605" y="227" textAnchor="middle" className="t-sm">SCOPE_MERCHANT_MISMATCH</text>
          </g>

          <text x="344" y="318" className="t-xs">lifecycle</text>
          {pill("s-authorized", 385, "authorized")}
          {pill("s-paid", 505, "paid")}
          {pill("s-settled", 625, "settled")}
          {pill("s-delivered", 745, "delivered")}
        </g>

        <g id="rail" className="box">
          <rect x="500" y="504" width="240" height="52" rx="2" />
          <text x="516" y="527" className="t-b">Rail</text>
          <text x="516" y="547" className="t-sm">PSP · UPI · USDC on Base</text>
        </g>

        <g id="p-mandate" className="pkt hidden"><rect x="-9" y="-6" width="18" height="12" rx="2" /></g>
        <g id="p-cart" className="pkt hidden"><circle r="7" /></g>
        <g id="p-rail" className="pkt pkt-rail hidden"><circle r="5" /></g>
        <g id="p-bad" className="pkt pkt-bad hidden"><circle r="7" /></g>
      </svg>
    </div>
  );
}
