import { ImageResponse } from "next/og";

export const alt = "mandate-ledger — five checks that never talk to each other";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

const row = (seq: number, event: string, detail: string, color: string, bg = "transparent") => (
  <div key={seq} style={{ display: "flex", gap: 28, padding: "16px 28px", background: bg, borderBottom: "2px solid #d6dad4", fontSize: 24 }}>
    <span style={{ color: "#626a65", width: 36 }}>{seq}</span>
    <span style={{ color, fontWeight: 700, width: 170 }}>{event}</span>
    <span style={{ color: "#111614" }}>{detail}</span>
  </div>
);

export default function OpenGraphImage() {
  return new ImageResponse(
    (
      <div style={{ width: "100%", height: "100%", display: "flex", flexDirection: "column", justifyContent: "space-between", padding: 60, background: "#f4f5f2", color: "#111614" }}>
        <div style={{ display: "flex", justifyContent: "space-between", fontSize: 28, fontWeight: 600 }}>
          <span>mandate-ledger</span>
          <span style={{ color: "#626a65", fontWeight: 400 }}>Rust · Apache-2.0</span>
        </div>
        <div style={{ display: "flex", fontSize: 84, fontWeight: 700, letterSpacing: -3, lineHeight: 1.02, maxWidth: 1040 }}>
          Five checks that never talk to each other.
        </div>
        <div style={{ display: "flex", flexDirection: "column", border: "3px solid #b3bbb5", background: "#ffffff", borderRadius: 4 }}>
          {row(1, "authorized", "bigbasket.com · INR 128.00 · under mnd_8f2a", "#146b4a")}
          {row(2, "settled", "pay_Nx7@3 · finality reached", "#146b4a")}
          {row(3, "denied", "SCOPE_MERCHANT_MISMATCH · amazon.in", "#9b2f2b", "#f3e1df")}
        </div>
      </div>
    ),
    size,
  );
}
