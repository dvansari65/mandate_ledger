/** The one diagram on the page: where the ledger sits. Static SVG, styled from the page tokens. */
export default function Diagram() {
  return (
    <figure className="diagram">
      <svg viewBox="0 0 1200 660" role="img" aria-labelledby="dg-t dg-d">
        <title id="dg-t">Where mandate-ledger sits</title>
        <desc id="dg-d">
          A wallet grants a mandate and an agent builds a cart. Both enter the ledger, which checks the signature, the
          scope and the budget, then moves the payment through authorized, paid, settled and delivered. Proof goes down
          to the payment rail and finality comes back up. The merchant is handed a settled token. Any disagreement is
          refused, and the refusal is recorded with the rest.
        </desc>
        <defs>
          <marker id="dg-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M0 1L9 5L0 9z" className="dg-head" />
          </marker>
        </defs>

        {/* the ledger */}
        <g className="dg-frame">
          <rect x="330" y="40" width="540" height="430" rx="18" />
          <text x="358" y="80" className="dg-t dg-accent">mandate-ledger</text>
          <text x="842" y="80" textAnchor="end" className="dg-s">never moves money</text>

          <text x="358" y="124" className="dg-k">checks</text>
          <g className="dg-pill">
            <rect x="358" y="138" width="128" height="36" rx="18" /><text x="422" y="161" textAnchor="middle">signature</text>
            <rect x="498" y="138" width="102" height="36" rx="18" /><text x="549" y="161" textAnchor="middle">scope</text>
            <rect x="612" y="138" width="110" height="36" rx="18" /><text x="667" y="161" textAnchor="middle">budget</text>
          </g>

          <text x="358" y="234" className="dg-k">lifecycle</text>
          <path d="M366 266H790" className="dg-rail" />
          <g className="dg-pill ok">
            <rect x="358" y="248" width="118" height="36" rx="18" /><text x="417" y="271" textAnchor="middle">authorized</text>
            <rect x="488" y="248" width="72" height="36" rx="18" /><text x="524" y="271" textAnchor="middle">paid</text>
            <rect x="572" y="248" width="92" height="36" rx="18" /><text x="618" y="271" textAnchor="middle">settled</text>
            <rect x="676" y="248" width="112" height="36" rx="18" /><text x="732" y="271" textAnchor="middle">delivered</text>
          </g>

          <g className="dg-note">
            <rect x="358" y="342" width="484" height="86" rx="12" />
            <circle cx="384" cy="374" r="4.5" className="dg-deny" />
            <text x="400" y="379" className="dg-s">Any disagreement — the step is refused,</text>
            <text x="400" y="404" className="dg-s">and the refusal is recorded with the rest.</text>
          </g>
        </g>

        {/* connections */}
        <g className="dg-lines">
          <path d="M250 153H312V145H330" />
          <path d="M250 420H286V161H330" />
          <path d="M870 266H948" />
          <path d="M546 470V556" />
          <path d="M654 556V470" />
        </g>
        <text x="278" y="140" textAnchor="middle" className="dg-lbl">mandate</text>
        <text x="272" y="296" textAnchor="middle" className="dg-lbl">cart</text>
        <text x="909" y="252" textAnchor="middle" className="dg-lbl">settled</text>
        <text x="538" y="518" textAnchor="end" className="dg-lbl">proof</text>
        <text x="662" y="518" className="dg-lbl">finality</text>

        {/* outside parties */}
        <g className="dg-box">
          <rect x="30" y="115" width="220" height="76" rx="12" />
          <text x="54" y="148" className="dg-t">Wallet</text>
          <text x="54" y="172" className="dg-s">grants a mandate</text>
        </g>
        <g className="dg-box">
          <rect x="30" y="382" width="220" height="76" rx="12" />
          <text x="54" y="415" className="dg-t">Agent</text>
          <text x="54" y="439" className="dg-s">builds a cart</text>
        </g>
        <g className="dg-box">
          <rect x="950" y="228" width="220" height="76" rx="12" />
          <text x="974" y="261" className="dg-t">Merchant</text>
          <text x="974" y="285" className="dg-s">delivers on settled</text>
        </g>
        <g className="dg-box">
          <rect x="460" y="556" width="280" height="76" rx="12" />
          <text x="484" y="589" className="dg-t">Rail</text>
          <text x="484" y="613" className="dg-s">PSP or chain — moves the money</text>
        </g>
      </svg>
    </figure>
  );
}
