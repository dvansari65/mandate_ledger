import { ImageResponse } from "next/og";

export const alt = "mandate-ledger — the agent can spend. The ledger decides whether it may.";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

const row = (seq: number, event: string, detail: string, color: string, bg = "transparent") => (
  <div key={seq} style={{ display: "flex", gap: 28, padding: "15px 28px", background: bg, borderBottom: "1px solid #232c2a", fontSize: 22, fontFamily: "monospace" }}>
    <span style={{ color: "#7e877f", width: 36 }}>{seq}</span>
    <span style={{ color, fontWeight: 700, width: 180 }}>{event}</span>
    <span style={{ color: "#ecefe9" }}>{detail}</span>
  </div>
);

export default function OpenGraphImage() {
  return new ImageResponse(
    (
      <div style={{ width: "100%", height: "100%", display: "flex", flexDirection: "column", justifyContent: "space-between", padding: 56, background: "#0b0f0e", color: "#ecefe9" }}>
        <div style={{ display: "flex", justifyContent: "space-between", fontSize: 24, fontFamily: "monospace" }}>
          <span style={{ fontWeight: 700, color: "#e3b341" }}>mandate-ledger</span>
          <span style={{ color: "#7e877f" }}>enforcement layer for agent payments · Rust · Apache-2.0</span>
        </div>
        <div style={{ display: "flex", fontSize: 76, fontWeight: 500, letterSpacing: -2.5, lineHeight: 1.05, maxWidth: 1000, fontFamily: "serif" }}>
          The agent can spend. The ledger decides whether it may.
        </div>
        <div style={{ display: "flex", flexDirection: "column", background: "#111716", borderRadius: 14, border: "1px solid #2f3a37", overflow: "hidden" }}>
          {row(1, "authorized", "bigbasket.com · 128.00 INR · under mnd_8f2a", "#6fbf9a")}
          {row(2, "settled", "pay_Nx7@3 · finality reached · delivery permitted", "#6fbf9a")}
          {row(3, "denied", "SCOPE_MERCHANT_MISMATCH · amazon.in not in mandate scope", "#f0776a", "#1a1412")}
        </div>
      </div>
    ),
    size,
  );
}
