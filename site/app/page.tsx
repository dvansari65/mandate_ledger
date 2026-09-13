import Architecture from "./components/Architecture";
import CopyChip from "./components/CopyChip";
import Ledger from "./components/Ledger";
import Reveal from "./components/Reveal";

const REPO = "https://github.com/dvansari65/mandate_ledger";
const DOCS = `${REPO}/blob/main/docs`;

export default function Page() {
  return (
    <>
      <header className="nav-wrap">
        <div className="wrap nav">
          <a href="#" className="brand">
            <svg viewBox="0 0 32 32" width="20" height="20" aria-hidden="true"><rect width="32" height="32" rx="9" fill="currentColor" /><rect x="8" y="10" width="16" height="2.4" rx="1.2" fill="#FFF2F2" /><rect x="8" y="15" width="16" height="2.4" rx="1.2" fill="#FFF2F2" /><rect x="8" y="20" width="9" height="2.4" rx="1.2" fill="#FFF2F2" /><rect x="20" y="19.5" width="4" height="4" rx="1.2" fill="#FF788D" /></svg>
            mandate-ledger
          </a>
          <nav className="pill-nav" aria-label="Sections">
            <a href="#architecture">Where it sits</a>
            <a href="#failures">What breaks</a>
            <a href="#calls">The API</a>
            <a href="#who">Integrate</a>
          </nav>
          <a className="btn sm" href={REPO}>GitHub</a>
        </div>
      </header>

      <main>
        <section className="wrap hero">
          <div className="hero-copy rise">
            <span className="tag">Enforcement layer for agent payments · Rust · Apache-2.0</span>
            <h1>Five checks that <em>never talk</em> to each other.</h1>
            <p className="lede">
              An agent’s payment is verified by five parties, none of which checks the others. mandate-ledger sits
              between them — it refuses the step when they disagree, and writes down why.
            </p>
            <div className="actions">
              <a className="btn" href={`${DOCS}/integration.md`}>Get started</a>
              <a className="btn outline" href={REPO}>Read the code</a>
            </div>
            <CopyChip text={`cargo add ml-core --git ${REPO}`} />
          </div>
          <div className="rise" style={{ animationDelay: "150ms" }}><Ledger /></div>
          <div className="rise" style={{ animationDelay: "280ms" }}>
            <dl className="stats">
              <div><dt>48</dt><dd>tests</dd></div>
              <div><dt>6,000</dt><dd>property cases on the scope lattice</dd></div>
              <div><dt>10<span>/16</span></dt><dd>threads pass against one ₹1,000 budget — exactly</dd></div>
              <div><dt>0</dt><dd>unsafe blocks</dd></div>
            </dl>
          </div>
        </section>

        <section className="band" id="architecture">
          <div className="wrap">
            <Reveal className="head">
              <span className="kicker">Where it sits</span>
              <h2>Between the agent and the money. Never holding either.</h2>
              <p>Inputs come in from the wallet and the agent. Proofs come up from the rail. The ledger decides, records, and hands the merchant a token it can’t forge.</p>
            </Reveal>
            <Reveal delay={0.1}><div className="canvas"><Architecture /></div></Reveal>
            <Reveal delay={0.15}>
              <div className="trio">
                <div><h3>Gate</h3><p>Signature and signer trust, then scope, then budget — the last one atomic with the write, so two parallel requests can’t both squeeze through.</p></div>
                <div><h3>Lifecycle</h3><p>Each stage is a token the next one requires. Wrong order doesn’t compile. Every transition, and every refusal, is appended to the hash chain.</p></div>
                <div><h3>Rail</h3><p>The rail says what “final” means — captured, N confirmations. The ledger waits for it; the merchant is never shown anything before Settled.</p></div>
              </div>
            </Reveal>
          </div>
        </section>

        <section className="wrap section" id="failures">
          <Reveal className="head">
            <span className="kicker">What goes wrong today</span>
            <h2>Every step is locally valid. Nobody checks that they refer to the same payment.</h2>
            <p>The same five failures show up across x402, MPP, AP2 and ACP — the primitive exists, the caller can skip it. Each one becomes a refusal here.</p>
          </Reveal>
          <Reveal delay={0.1}>
            <ol className="cards">
              <li><h3>The wrong cart gets paid</h3><p>You approved ₹470 at one store. A bug or a hostile page turns it into ₹4,700 somewhere else. Every signature still verifies.</p><code className="deny">CART_BINDING_MISMATCH</code></li>
              <li><h3>A retry charges twice</h3><p>The response is lost, the agent tries again, the merchant processes it again. Two charges, one refund ticket.</p><code className="ok">same context · one charge</code></li>
              <li><h3>Goods ship before the money is final</h3><p>The merchant sees a pending payment and delivers. The payment is dropped or reorganised away. Fifteen of fifteen x402 facilitators tested had this gap.</p><code className="ok">deliver requires Settled</code></li>
              <li><h3>There is no stop button</h3><p>You granted ₹8,000 a month. On day three you change your mind. AP2 has no in-protocol revocation — the mandate lives until it expires.</p><code className="deny">MANDATE_REVOKED</code></li>
              <li><h3>Parallel work overspends</h3><p>Three sub-tasks each read “₹8,000 left” in the same instant. Each spends it. Here the budget is reserved inside the store’s append, so only one gets through.</p><code className="deny">SCOPE_TOTAL_EXCEEDED</code></li>
              <li className="cite"><p>Forty issues of this shape across the four protocols.</p><a href="https://arxiv.org/abs/2609.00060">A Formal Analysis of Agent Payment Protocols — arXiv 2609.00060 →</a></li>
            </ol>
          </Reveal>
        </section>

        <section className="band" id="calls">
          <div className="wrap two">
            <Reveal>
              <span className="kicker">The API</span>
              <h2>Five calls. Each returns the token the next requires.</h2>
              <p>Call them out of order and it doesn’t compile. Call them across requests and <code>resume(ctx)</code> hands the token back.</p>
              <ol className="steps">
                <li><b>authorize</b><span>Signature, trust, revocation, scope, cap, velocity, budget. Reserves the amount.</span></li>
                <li><b>record_payment</b><span>Amount must match. Proof must bind to this context or this cart. Nonce is consumed once, for everyone.</span></li>
                <li><b>record_settlement</b><span>Final, pending, or failed — the rail decides. Failure releases the budget.</span></li>
                <li><b>record_delivery</b><span>Accepts only a <code>Settled</code>. There is no other way to make one.</span></li>
                <li><b>evidence</b><span>The whole chain, refusals included. Verifiable without your database.</span></li>
              </ol>
            </Reveal>
            <Reveal delay={0.1}>
              <pre className="code"><code><span className="k">let</span> auth    = ledger.authorize(&amp;mandate, &amp;cart, <span className="s">"order-1"</span>)?;{`
`}<span className="k">let</span> paid    = ledger.record_payment(&amp;auth, &amp;proof)?;{`
`}<span className="k">let</span> settled = <span className="k">match</span> ledger.record_settlement(&amp;paid, &amp;finality)? {"{"}{`
`}    Settlement::Settled(s)   =&gt; s,{`
`}    Settlement::Pending {"{ .. }"} =&gt; <span className="k">return</span> retry_later(),{`
`}    Settlement::Failed(f)    =&gt; <span className="k">return</span> ledger.compensate(&amp;f, None),{`
`}{"}"};{`
`}ledger.record_delivery(&amp;settled, receipt)?;{`
`}<span className="k">let</span> bundle  = ledger.evidence(auth.ctx())?;   <span className="c">// self-verifying</span></code></pre>
            </Reveal>
          </div>
        </section>

        <section className="wrap section" id="enforced">
          <Reveal className="head">
            <span className="kicker">Enforced three times</span>
            <h2>The same rule at the type level, in the engine, and in the store.</h2>
            <p>A guarantee that lives in one place is a policy. One that lives in three is a property.</p>
          </Reveal>
          <div className="two">
            <Reveal>
              <div className="trio stack">
                <div><h3>Types</h3><p><code>record_delivery</code> takes a <code>Settled</code>. Passing a <code>Paid</code> is a compile error.</p></div>
                <div><h3>Engine</h3><p>Every call re-reads the stored state before it acts. Tokens are proofs, not permissions.</p></div>
                <div><h3>Store</h3><p><code>append</code> rejects any transition the machine doesn’t allow — even a hand-built event.</p></div>
              </div>
            </Reveal>
            <Reveal delay={0.1}>
              <pre className="code"><code><span className="c">// A merchant tries to ship on a pending payment.</span>{`
`}<span className="k">let</span> paid = ledger.record_payment(&amp;auth, &amp;proof)?;{`
`}ledger.record_delivery(&amp;paid, receipt)?;{`

`}<span className="e">error[E0308]</span>: mismatched types{`
`}   <span className="c">--&gt; checkout.rs:42:28</span>{`
`}    | ledger.record_delivery(&amp;paid, receipt)?;{`
`}    |                        <span className="e">^^^^^</span> expected `&amp;Settled`, found `&amp;Paid`</code></pre>
            </Reveal>
          </div>
        </section>

        <section className="band" id="who">
          <div className="wrap">
            <Reveal className="head">
              <span className="kicker">Who plugs in</span>
              <h2>Nobody rewrites their payment code. They add two gates to it.</h2>
            </Reveal>
            <Reveal delay={0.1}>
              <div className="quad">
                <div><h3>Agents</h3><p>A hard cap on what the model can spend, whatever it decides. Prompt injection can still try; it can’t get past authorize.</p><span className="writes">one authorize call</span></div>
                <div><h3>Merchants</h3><p>Never ship or return data before the money is final. Never charged twice by a retry.</p><span className="writes">a CartAdapter; fulfilment gated on Settled</span></div>
                <div><h3>Payment gateways</h3><p>One lifecycle across every rail. Webhooks resume the context. Every agent payment leaves a dispute-ready record.</p><span className="writes">one Rail adapter per rail</span></div>
                <div><h3>Wallets</h3><p>Customers set limits, watch the budget drain, and hit stop. “The agent did it” disputes come with evidence.</p><span className="writes">mandate signing, revoke, attenuate</span></div>
              </div>
            </Reveal>
          </div>
        </section>

        <section className="wrap section" id="scope">
          <Reveal className="head">
            <span className="kicker">Honest scope</span>
            <h2>What it refuses to guess.</h2>
            <p>A scope the cart can’t satisfy is a refusal, not a pass. And some problems belong to other layers.</p>
          </Reveal>
          <Reveal delay={0.1}>
            <div className="scope">
              <ul className="yes" aria-label="Covered">
                <li>Cart approved ≠ cart paid</li>
                <li>Double charge on retry, nonce reuse across contexts</li>
                <li>Release before finality; no rollback after failed settlement</li>
                <li>Out-of-scope merchant, category, amount, currency, velocity</li>
                <li>Revoked, expired, forged, or untrusted mandates</li>
                <li>Budget races under concurrency</li>
                <li>Tampered or truncated evidence</li>
              </ul>
              <ul className="no" aria-label="Not covered">
                <li>Detecting prompt injection itself — only bounding its effect</li>
                <li>Agent identity — that’s Visa TAP, Mastercard Agent Pay, Tenuo</li>
                <li>Counterfeit storefronts inside an allowed scope</li>
                <li>Merchant non-delivery after settlement — needs escrow</li>
                <li>Chain-level asset theft, gas abuse</li>
              </ul>
            </div>
          </Reveal>
        </section>

        <section className="close">
          <div className="wrap">
            <Reveal>
              <h2>Put a gate in front of the money.</h2>
              <p>Core engine complete and tested. Protocol adapters and a SQL store are next. Read it, break it, tell us — and don’t put money behind it yet.</p>
              <div className="actions">
                <a className="btn light" href={REPO}>Read the code</a>
                <a className="btn ghost" href={`${DOCS}/threat-model.md`}>Threat model</a>
              </div>
            </Reveal>
          </div>
        </section>
      </main>

      <footer className="wrap footer">
        <span>mandate-ledger · 0.0.1 · Apache-2.0</span>
        <span><a href={REPO}>GitHub</a> · <a href={`${DOCS}/integration.md`}>Integration guide</a> · <a href="https://arxiv.org/abs/2609.00060">Formal analysis</a></span>
      </footer>
    </>
  );
}
